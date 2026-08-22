use crate::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::emit::Placement;
use crate::emit::{EmitError, EmitOptions, PrefabResolver, build_world, emit_brz};
use crate::ir::NodeId;
use crate::layout::{layout_options_for, layout_with_opts};
use crate::lower::{LowerInput, lower};
use crate::resolve::{FsLoader, resolve};
use crate::template_cache::TemplateCache;
use crate::typecheck::typecheck_with_inference;

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
/// touches.
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

/// Maximum depth of `$./file.ws` source-prefab compilation before it refuses
/// (a runaway guard against a `.ws` prefab that references itself).
const MAX_PREFAB_WS_DEPTH: usize = 8;

/// A [`PrefabResolver`] that resolves `$./file` prefab references from disk.
/// A `.brz` path is read as raw prefab bytes; a `.ws` path is **compiled on the
/// spot** (its own source → `.brz` bytes), so `$./control.ws` embeds the
/// compiled result of `control.ws` exactly like `$./control.brz` embeds a
/// prebuilt one. `$./rel` resolves relative to `entry_file`'s directory;
/// `$/abs` is filesystem-absolute. (Relative refs in imported files also
/// resolve against the entry file's directory.)
pub fn disk_prefab_resolver(entry_file: &str, fold_mode: FoldMode) -> PrefabResolver {
    disk_prefab_resolver_depth(entry_file, fold_mode, 1)
}

fn disk_prefab_resolver_depth(entry_file: &str, fold_mode: FoldMode, depth: usize) -> PrefabResolver {
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
        // A `.ws` reference is a SOURCE prefab: compile it here and embed the
        // result, mirroring `default_nested_compiler` for inline `$```…````
        // blocks (its own `$./…` refs resolve relative to the `.ws` file).
        if full.extension().is_some_and(|e| e.eq_ignore_ascii_case("ws")) {
            if depth > MAX_PREFAB_WS_DEPTH {
                return Err(format!(
                    "`$./….ws` source-prefab references are nested too deeply (limit {MAX_PREFAB_WS_DEPTH})"
                ));
            }
            let src = std::fs::read_to_string(&full)
                .map_err(|e| format!("cannot read prefab source {}: {e}", full.display()))?;
            let full_str = full.to_string_lossy().into_owned();
            let inner_opts = EmitOptions {
                prefab_resolver: Some(disk_prefab_resolver_depth(&full_str, fold_mode, depth + 1)),
                nested_compiler: Some(default_nested_compiler(1, full_str.clone(), fold_mode)),
                ..Default::default()
            };
            let result = compile_to_world(
                CompileInput { source: &src, file: &full_str, module_name: None, fold_mode },
                inner_opts,
            )
            .map_err(|e| format!("prefab {} failed to compile: {e}", full.display()))?;
            return result
                .world
                .to_brz_vec()
                .map_err(|e| format!("prefab {} .brz encode failed: {e}", full.display()));
        }
        std::fs::read(&full).map_err(|e| format!("cannot read {}: {e}", full.display()))
    })
}

/// Maximum `$```…``` nesting depth before compilation refuses (runaway guard).
const MAX_NESTED_DEPTH: usize = 8;

/// A default nested-prefab compiler for blocks at `depth` levels of nesting.
/// Recursively compiles the inner source into prefab `.brz` bytes. The closure
/// ignores the `depth` argument the emit layer passes (always 1) and uses its
/// own captured `depth` for the guard and for the next level's compiler.
fn default_nested_compiler(
    depth: usize,
    file: String,
    fold_mode: FoldMode,
) -> crate::emit::NestedCompiler {
    crate::emit::NestedCompiler::new(move |inner_src: &str, _embed_depth: usize| {
        if depth > MAX_NESTED_DEPTH {
            return Err(format!(
                "nested prefab blocks are nested too deeply (limit {MAX_NESTED_DEPTH})"
            ));
        }
        // Compile the inner source as its own isolated program. Its OWN nested
        // blocks get the next depth level's compiler (this is what accumulates
        // the depth — NOT the emit arg). Imports resolve relative to the outer
        // file (reuse `file`).
        let inner_opts = EmitOptions {
            nested_compiler: Some(default_nested_compiler(depth + 1, file.clone(), fold_mode)),
            ..Default::default()
        };
        let result = compile_to_world(
            CompileInput { source: inner_src, file: &file, module_name: None, fold_mode },
            inner_opts,
        )
        .map_err(|e| format!("nested prefab failed to compile: {e}"))?;
        result
            .world
            .to_brz_vec()
            .map_err(|e| format!("nested prefab .brz encode failed: {e}"))
    })
}

#[derive(Clone, Debug)]
pub struct CompileProgress {
    pub step: u32,
    pub total: u32,
    pub done: bool,
}

pub type ProgressCallback = std::sync::Arc<dyn Fn(CompileProgress) + Send + Sync>;

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
    use std::sync::atomic::{AtomicU32, Ordering};
    // Four fixed phases (resolve, lower, layout, emit), plus one step per embedded
    // prefab — each is its own sub-compile during emit, so the total grows to
    // `4 + <nested prefab count>` once the AST is parsed (below). Shared via Arc
    // so the per-prefab wrappers (which must be `Send + Sync`) can advance it too.
    const BASE_STEPS: u32 = 4;
    let step = std::sync::Arc::new(AtomicU32::new(0));
    let total = std::sync::Arc::new(AtomicU32::new(BASE_STEPS));
    let report = {
        let step = step.clone();
        let total = total.clone();
        let progress = progress.clone();
        move || {
            let s = step.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(cb) = &progress {
                cb(CompileProgress {
                    step: s,
                    total: total.load(Ordering::Relaxed),
                    done: false,
                });
            }
        }
    };

    let source = input.source;
    let file = input.file;
    let module_name = input.module_name;

    // Default to disk-backed prefab resolution unless a caller (e.g. the wasm
    // sandbox) supplied its own resolver.
    if opts.prefab_resolver.is_none() {
        opts.prefab_resolver = Some(disk_prefab_resolver(file, input.fold_mode));
    }
    if opts.nested_compiler.is_none() {
        opts.nested_compiler = Some(default_nested_compiler(1, file.to_string(), input.fold_mode));
    }

    report();
    let resolved = resolve(source, file, &FsLoader);
    // Grow the progress total by the number of embedded prefabs (each `$./file`
    // reference and each inline `$```…``` ` block is a sub-compile the emit step
    // does), so the bar advances through them instead of stalling on emit.
    {
        let mut n_prefabs = 0u32;
        crate::analysis::visit_program(&resolved.ast, &mut |_| {}, &mut |e| {
            if matches!(
                e,
                crate::ast::Expr::PrefabRef { .. } | crate::ast::Expr::NestedPrefab { .. }
            ) {
                n_prefabs += 1;
            }
        });
        total.store(BASE_STEPS + n_prefabs, Ordering::Relaxed);
    }
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
    // Top-of-file `@invisible` hides the emitted shell — see `EmitOptions::invisible`.
    opts.invisible = resolved.ast.invisible;
    // `@layout("cube")` packs gates into an unreadable block, so the per-gate
    // labels are dropped there as pure payload. See `EmitOptions::no_gate_labels`.
    opts.no_gate_labels = resolved.ast.layout == Some(crate::ast::LayoutName::Cube);
    let (tc, ce_slots) = typecheck_with_inference(&resolved.ast, file);

    let template_cache = {
        let cache = TemplateCache::new();
        std::sync::Arc::new(cache)
    };

    report();
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name,
        template_cache: template_cache.clone(),
        doc_comments: &resolved.doc_comments,
        fold_mode: input.fold_mode,
        ce_slots: &ce_slots,
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

    report();
    let lopts = layout_options_for(&resolved.ast, Some(resolved.source_map.clone()));
    let lr = layout_with_opts(&lowered.module, &lopts);

    if opts.description.is_empty() {
        opts.description = format!(
            "wirescript compile: {}",
            std::path::Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }

    // Advance the bar once per embedded prefab as emit compiles/reads each one.
    // Guarded on progress being active so a plain compile keeps its resolvers
    // (and their exact behavior) untouched. Clamp to the total in case emit
    // resolves a prefab more often than the source references it.
    if progress.is_some() {
        let tick: std::sync::Arc<dyn Fn() + Send + Sync> = {
            let step = step.clone();
            let total = total.clone();
            let progress = progress.clone();
            std::sync::Arc::new(move || {
                if let Some(cb) = &progress {
                    let tot = total.load(Ordering::Relaxed);
                    let s = (step.fetch_add(1, Ordering::Relaxed) + 1).min(tot);
                    cb(CompileProgress { step: s, total: tot, done: false });
                }
            })
        };
        if let Some(inner) = opts.nested_compiler.take() {
            let tick = tick.clone();
            opts.nested_compiler = Some(crate::emit::NestedCompiler::new(move |src, depth| {
                tick();
                (inner.0)(src, depth)
            }));
        }
        if let Some(inner) = opts.prefab_resolver.take() {
            let tick = tick.clone();
            opts.prefab_resolver = Some(PrefabResolver::new(move |path| {
                tick();
                (inner.0)(path)
            }));
        }
    }

    report();
    let brz = emit_brz(&lowered.module, &lr, &opts, &template_cache).map_err(CompileError::Emit)?;

    if let Some(ref cb) = progress {
        let tot = total.load(Ordering::Relaxed);
        cb(CompileProgress {
            step: tot,
            total: tot,
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
        opts.prefab_resolver = Some(disk_prefab_resolver(file, input.fold_mode));
    }
    if opts.nested_compiler.is_none() {
        opts.nested_compiler = Some(default_nested_compiler(1, file.to_string(), input.fold_mode));
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
    // Top-of-file `@invisible` hides the emitted shell — see `EmitOptions::invisible`.
    opts.invisible = resolved.ast.invisible;
    // `@layout("cube")` packs gates into an unreadable block, so the per-gate
    // labels are dropped there as pure payload. See `EmitOptions::no_gate_labels`.
    opts.no_gate_labels = resolved.ast.layout == Some(crate::ast::LayoutName::Cube);
    let (tc, ce_slots) = typecheck_with_inference(&resolved.ast, file);

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
        ce_slots: &ce_slots,
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

    let lopts = layout_options_for(&resolved.ast, Some(resolved.source_map.clone()));
    let lr = layout_with_opts(&lowered.module, &lopts);

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
mod tests;
