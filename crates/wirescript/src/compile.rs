use crate::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::emit::Placement;
use crate::emit::{EmitError, EmitOptions, PrefabResolver, build_world, emit_brz};
use crate::ir::NodeId;
use crate::layout::layout;
use crate::lower::{LowerInput, lower};
use crate::resolve::{FsLoader, resolve};
use crate::template_cache::TemplateCache;
use crate::typecheck::typecheck;

pub use crate::lower::FoldMode;

pub struct CompileInput<'a> {
    pub source: &'a str,
    pub file: &'a str,
    pub module_name: Option<&'a str>,
    /// Whether the certified constant-fold pass runs — see [`FoldMode`].
    pub fold_mode: FoldMode,
}

pub struct CompileResult {
    pub brz: Vec<u8>,
    pub diagnostics: Vec<Diagnostic>,
    pub placements: HashMap<NodeId, Placement>,
}

#[derive(Debug)]
pub enum CompileError {
    HasErrors(Vec<Diagnostic>),
    Emit(EmitError),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::HasErrors(diags) => {
                for d in diags {
                    write!(f, "[{}] {} ", d.code, d.message)?;
                }
                Ok(())
            }
            CompileError::Emit(e) => write!(f, "emit: {:?}", e),
        }
    }
}

pub fn compile(input: CompileInput<'_>) -> Result<CompileResult, CompileError> {
    compile_with_opts(input, EmitOptions::default())
}

/// Stack reserved for a compile run. Some compile recursion still scales
/// with program structure (lowering/emit), and callers routinely invoke
/// compile from small-stack threads — the LSP's compile command runs on a
/// ~2 MiB tokio blocking thread, which large programs overflowed, aborting
/// the whole process. Reserved address space only — pages are committed as
/// used, so the worker costs nothing beyond what the compile actually
/// touches. (The former deepest site, the Tarjan SCC walk in
/// `analyze_cycles`, is now iterative and needs no stack headroom at all.)
const COMPILE_STACK_SIZE: usize = 256 * 1024 * 1024; // 256 MiB reserved

/// Run `f` on a dedicated big-stack worker thread so every compile entry
/// point is safe regardless of the caller's stack. Scoped, so borrowed
/// inputs (`CompileInput<'_>`) work without `'static` bounds.
#[cfg(not(target_arch = "wasm32"))]
fn on_compile_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        let handle = std::thread::Builder::new()
            .name("wirescript-compile".into())
            .stack_size(COMPILE_STACK_SIZE)
            .spawn_scoped(s, f)
            .expect("failed to spawn compile worker thread");
        match handle.join() {
            Ok(v) => v,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

/// wasm32 has no threads — run inline (wasm callers control their own stack).
#[cfg(target_arch = "wasm32")]
fn on_compile_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    f()
}

/// A [`PrefabResolver`] that reads `.brz` files from disk. `$./rel.brz`
/// resolves relative to `entry_file`'s directory; `$/abs.brz` is a
/// filesystem-absolute path. (Relative refs in imported files also resolve
/// against the entry file's directory.)
pub fn disk_prefab_resolver(entry_file: &str) -> PrefabResolver {
    let base = std::path::Path::new(entry_file)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    PrefabResolver::new(move |path: &str| {
        let full = if let Some(rel) = path.strip_prefix("./") {
            base.join(rel)
        } else if path.starts_with('/') {
            std::path::PathBuf::from(path)
        } else {
            base.join(path)
        };
        std::fs::read(&full).map_err(|e| format!("cannot read {}: {e}", full.display()))
    })
}

#[derive(Clone, Debug)]
pub struct CompileProgress {
    pub step: u32,
    pub total: u32,
    pub done: bool,
}

pub type ProgressCallback = Box<dyn Fn(CompileProgress) + Send>;

pub fn compile_with_progress(
    input: CompileInput<'_>,
    opts: EmitOptions,
    progress: ProgressCallback,
) -> Result<CompileResult, CompileError> {
    on_compile_stack(move || compile_with_opts_inner(input, opts, Some(progress)))
}

pub fn compile_with_opts(
    input: CompileInput<'_>,
    opts: EmitOptions,
) -> Result<CompileResult, CompileError> {
    on_compile_stack(move || compile_with_opts_inner(input, opts, None))
}

fn compile_with_opts_inner(
    input: CompileInput<'_>,
    mut opts: EmitOptions,
    progress: Option<ProgressCallback>,
) -> Result<CompileResult, CompileError> {
    const TOTAL_STEPS: u32 = 4;
    let step = std::cell::Cell::new(0u32);
    let report = |progress: &Option<ProgressCallback>| {
        let s = step.get() + 1;
        step.set(s);
        if let Some(cb) = progress {
            cb(CompileProgress {
                step: s,
                total: TOTAL_STEPS,
                done: false,
            });
        }
    };

    let source = input.source;
    let file = input.file;
    let module_name = input.module_name;

    // Default to disk-backed prefab resolution unless a caller (e.g. the wasm
    // sandbox) supplied its own resolver.
    if opts.prefab_resolver.is_none() {
        opts.prefab_resolver = Some(disk_prefab_resolver(file));
    }

    report(&progress);
    let resolved = resolve(source, file, &FsLoader);
    // An explicit top-of-file module doc (a `///` block separated from the first
    // decl by a blank line) is the root header; otherwise fall back to the first
    // declaration's doc comment.
    opts.module_doc = resolved.ast.module_doc.clone().or_else(|| {
        resolved
            .ast
            .decls
            .first()
            .and_then(|d| resolved.doc_comments.get(&d.range().start.offset))
            .cloned()
    });
    let tc = typecheck(&resolved.ast, file);

    let template_cache = {
        let cache = TemplateCache::new();
        std::sync::Arc::new(cache)
    };

    report(&progress);
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name,
        template_cache: template_cache.clone(),
        doc_comments: &resolved.doc_comments,
        fold_mode: input.fold_mode,
    });

    // Every wire-graph cycle must cross a tick barrier (Buffer/Queue) — an
    // unbarriered cycle (e.g. an emit/await loop back-edge without `buffer`)
    // would retrigger within a single tick (WS005).
    let cycles = crate::analyze::analyze_cycles(&lowered.module);

    let all_diags: Vec<_> = resolved
        .diagnostics
        .into_iter()
        .chain(tc.diagnostics)
        .chain(lowered.diagnostics)
        .chain(cycles.diagnostics)
        .collect();

    let errors: Vec<_> = all_diags
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error))
        .cloned()
        .collect();
    if !errors.is_empty() {
        return Err(CompileError::HasErrors(errors));
    }

    report(&progress);
    let lr = layout(&lowered.module);

    if opts.description.is_empty() {
        opts.description = format!(
            "wirescript compile: {}",
            std::path::Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }

    report(&progress);
    let brz = emit_brz(&lowered.module, &lr, &opts, &template_cache).map_err(CompileError::Emit)?;

    if let Some(ref cb) = progress {
        cb(CompileProgress {
            step: TOTAL_STEPS,
            total: TOTAL_STEPS,
            done: true,
        });
    }

    Ok(CompileResult {
        brz,
        diagnostics: all_diags,
        placements: lr.placements,
    })
}

pub struct CompileWorldResult {
    pub world: brdb::World,
    pub diagnostics: Vec<Diagnostic>,
    pub placements: HashMap<NodeId, Placement>,
}

pub fn compile_to_world(
    input: CompileInput<'_>,
    opts: EmitOptions,
) -> Result<CompileWorldResult, CompileError> {
    on_compile_stack(move || compile_to_world_inner(input, opts))
}

fn compile_to_world_inner(
    input: CompileInput<'_>,
    mut opts: EmitOptions,
) -> Result<CompileWorldResult, CompileError> {
    let source = input.source;
    let file = input.file;
    let module_name = input.module_name;
    if opts.prefab_resolver.is_none() {
        opts.prefab_resolver = Some(disk_prefab_resolver(file));
    }
    let t0 = std::time::Instant::now();
    let resolved = resolve(source, file, &FsLoader);
    opts.module_doc = resolved.ast.module_doc.clone().or_else(|| {
        resolved
            .ast
            .decls
            .first()
            .and_then(|d| resolved.doc_comments.get(&d.range().start.offset))
            .cloned()
    });
    let tc = typecheck(&resolved.ast, file);

    let template_cache = std::sync::Arc::new(TemplateCache::new());

    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name,
        template_cache: template_cache.clone(),
        doc_comments: &resolved.doc_comments,
        fold_mode: input.fold_mode,
    });

    // Unbarriered wire-graph cycles error (WS005) — see compile_with_opts.
    let cycles = crate::analyze::analyze_cycles(&lowered.module);

    let all_diags: Vec<_> = resolved
        .diagnostics
        .into_iter()
        .chain(tc.diagnostics)
        .chain(lowered.diagnostics)
        .chain(cycles.diagnostics)
        .collect();

    let errors: Vec<_> = all_diags
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error))
        .cloned()
        .collect();
    if !errors.is_empty() {
        return Err(CompileError::HasErrors(errors));
    }

    let lr = layout(&lowered.module);

    if opts.description.is_empty() {
        opts.description = format!(
            "wirescript compile: {}",
            std::path::Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }

    let world =
        build_world(&lowered.module, &lr, &opts, &template_cache).map_err(CompileError::Emit)?;
    eprintln!("[compile] total: {:.2}s", t0.elapsed().as_secs_f64());

    Ok(CompileWorldResult {
        world,
        diagnostics: all_diags,
        placements: lr.placements,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile recursion depth scales with program size (Tarjan SCC walk in
    /// analyze_cycles, lowering/emit recursion), and real callers invoke
    /// compile from small-stack threads — the LSP's compile command runs on a
    /// tokio blocking thread (2 MiB). The entry points must be safe no matter
    /// how small the caller's stack is. A stack overflow aborts the whole
    /// process, so without the internal big-stack worker this test crashes
    /// the test run rather than failing an assertion.
    #[test]
    fn compile_survives_small_caller_stack() {
        let mut src = String::from("in x: int\nlet a0 = x + 1\n");
        for i in 1..8000 {
            src.push_str(&format!("let a{i} = a{} + 1\n", i - 1));
        }
        src.push_str("out result = a7999\n");
        let out = std::thread::Builder::new()
            .stack_size(384 * 1024)
            .spawn(move || {
                compile(CompileInput {
                    source: &src,
                    file: "small_stack_test",
                    module_name: None,
                    fold_mode: FoldMode::Auto,
                })
                .map(|r| r.brz.len())
            })
            .expect("spawn small-stack caller")
            .join()
            .expect("small-stack compile panicked");
        assert!(
            out.is_ok(),
            "compile failed: {:?}",
            out.err().map(|e| e.to_string())
        );
    }
}
