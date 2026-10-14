use crate::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::emit::Placement;
use crate::emit::{Assembly, EmitError, EmitOptions, PrefabResolver, assemble_with_phases};
use crate::ir::{LayoutMode, NodeId, Program, ProgramSettings};
use crate::lower::{LowerInput, lower};
use crate::resolve::{FileLoader, FsLoader, resolve};
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

impl std::error::Error for CompileError {}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::HasErrors(diags) => {
                for d in diags {
                    write!(f, "{d} ")?;
                }
                Ok(())
            }
            CompileError::Emit(e) => write!(f, "emit: {}", e),
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
            .spawn_scoped(s, || {
                let _ids = crate::ir::NodeId::compile_scope();
                f()
            })
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
    let _ids = crate::ir::NodeId::compile_scope();
    f()
}

/// Run `f` on the same big-stack worker [`compile`] uses.
///
/// Front-end recursion scales with program structure, and a stack overflow is
/// not a panic: it aborts the process, so `catch_unwind` cannot turn it into a
/// diagnostic. An editor answers each request on a ~2 MiB tokio worker, which
/// a legal but deeply nested program outgrows, so every entry point that walks
/// a user AST should come through here.
///
/// Reserved address space only: pages are committed as touched, and the spawn
/// itself is around 100 us.
pub fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    on_compile_stack(f)
}

/// Maximum depth of `$./file.ws` source-prefab compilation before it refuses
/// (a runaway guard against a `.ws` prefab that references itself).
const MAX_PREFAB_WS_DEPTH: usize = 8;

/// Where the prefab file reference `path` (what follows the `$`) points, read
/// from a file in `dir`: `./rel` and a bare `rel` relative to `dir`, `/abs`
/// as written.
pub fn prefab_path_in(dir: &std::path::Path, path: &str) -> std::path::PathBuf {
    match path.strip_prefix("./") {
        Some(rel) => dir.join(rel),
        None if path.starts_with('/') => std::path::PathBuf::from(path),
        None => dir.join(path),
    }
}

/// [`prefab_path_in`] for a reference written in the file `entry_file`.
pub fn resolve_prefab_path(entry_file: &str, path: &str) -> std::path::PathBuf {
    prefab_path_in(std::path::Path::new(entry_file).parent().unwrap_or(std::path::Path::new("")), path)
}

/// A [`PrefabResolver`] that resolves `$./file` prefab references from disk.
/// A `.brz` path is read as raw prefab bytes; a `.ws` path is **compiled on the
/// spot** (its own source -> `.brz` bytes), so `$./control.ws` embeds the
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
    // Per-resolver memo: a program that spawns the same `$./file` prefab from
    // many sites resolves it once; repeat references reuse the bytes (the
    // embed step already dedups identical bytes by content hash, so this
    // also keeps a multiply-referenced `.ws` prefab to a single embedded
    // blob instead of one per nondeterministic recompile).
    let cache: std::sync::Mutex<HashMap<std::path::PathBuf, Vec<u8>>> =
        std::sync::Mutex::new(HashMap::default());
    PrefabResolver::new(move |path: &str| {
        let full = prefab_path_in(&base, path);
        if let Some(bytes) = cache.lock().unwrap().get(&full) {
            return Ok(bytes.clone());
        }
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
            let bytes = result
                .world
                .to_brz_vec()
                .map_err(|e| format!("prefab {} .brz encode failed: {e}", full.display()))?;
            cache.lock().unwrap().insert(full, bytes.clone());
            return Ok(bytes);
        }
        let bytes =
            std::fs::read(&full).map_err(|e| format!("cannot read {}: {e}", full.display()))?;
        cache.lock().unwrap().insert(full, bytes.clone());
        Ok(bytes)
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
    /// The phase this step is about to run ("resolve", "lower", "layout",
    /// "emit", "prefab"), or "done" on the terminal event. Reported so a
    /// caller's progress UI and logs can name the phase rather than showing a
    /// bare step counter -- which is all you got when a compile appeared stuck.
    pub label: &'static str,
}

pub type ProgressCallback = std::sync::Arc<dyn Fn(CompileProgress) + Send + Sync>;

pub fn compile_with_progress(
    input: CompileInput<'_>,
    opts: EmitOptions,
    progress: ProgressCallback,
) -> Result<CompileResult, CompileError> {
    on_compile_stack(move || compile_with_opts_inner(input, opts, Some(progress), &FsLoader))
}

pub fn compile_with_opts(
    input: CompileInput<'_>,
    opts: EmitOptions,
) -> Result<CompileResult, CompileError> {
    compile_with_loader(input, opts, &FsLoader)
}

/// [`compile_with_opts`] against a caller-supplied import loader. The browser
/// serves imports from an in-memory file map and the editor from unsaved
/// buffers; without this they had to rebuild the whole pipeline to swap the
/// loader, and each copy drifted.
pub fn compile_with_loader(
    input: CompileInput<'_>,
    opts: EmitOptions,
    loader: &(dyn FileLoader + Sync),
) -> Result<CompileResult, CompileError> {
    on_compile_stack(move || compile_with_opts_inner(input, opts, None, loader))
}

/// What the front end makes of one source file: each stage's result.
pub struct FrontEnd {
    pub resolved: crate::resolve::ResolveResult,
    pub typecheck: crate::typecheck::TypeCheckResult,
    /// The custom-event slot types inference found, which lowering read.
    pub ce_slots: crate::typecheck::CeSlotMap,
    pub lowered: crate::lower::LowerResult,
}

/// The front end every compile runs: resolve `source` as `file` through
/// `loader`, type-check it with custom-event slot inference, and lower it with
/// the slot types that inference found. Runs on the caller's stack; a caller
/// on a small one goes through [`on_big_stack`].
pub fn front_end(
    source: &str,
    file: &str,
    module_name: Option<&str>,
    fold_mode: FoldMode,
    loader: &dyn FileLoader,
) -> FrontEnd {
    front_end_staged(source, file, module_name, fold_mode, loader, &mut |_| {})
}

/// A stage of the front end about to run.
pub enum FrontEndStage<'a> {
    Resolve,
    Typecheck,
    /// Lowering `resolved`, which type checking has seen.
    Lower(&'a crate::resolve::ResolveResult),
}

/// [`front_end`], calling `stage` before each stage runs: for a caller that
/// reports progress, or that names the stage a panic came from.
pub fn front_end_staged(
    source: &str,
    file: &str,
    module_name: Option<&str>,
    fold_mode: FoldMode,
    loader: &dyn FileLoader,
    stage: &mut dyn FnMut(FrontEndStage<'_>),
) -> FrontEnd {
    stage(FrontEndStage::Resolve);
    let resolved = resolve(source, file, loader);
    stage(FrontEndStage::Typecheck);
    let (typecheck, ce_slots) = typecheck_with_inference(&resolved.ast, file);
    stage(FrontEndStage::Lower(&resolved));
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &typecheck.type_of_expr,
        op_resolutions: &typecheck.op_resolutions,
        resolutions: &typecheck.resolutions,
        file,
        module_name,
        template_cache: std::sync::Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode,
        ce_slots: &ce_slots,
    });
    FrontEnd { resolved, typecheck, ce_slots, lowered }
}

/// Run the front-end through lowering (resolve -> typecheck -> lower -> cycle
/// analysis) and return EVERY diagnostic, without emitting. This is the diagnostic
/// surface `wirescript-check` reports: it includes the lowering-stage `WSP001`
/// placeholder warnings a typecheck-only pass can't see. A program that lowers to
/// an `_Unsupported` gate is "no errors" to typecheck yet a real miscompile, so a
/// check that stops at typecheck silently passes it. Runs on the big compile
/// stack like the other entry points.
pub fn diagnostics_only(input: CompileInput<'_>, loader: &(dyn FileLoader + Sync)) -> Vec<Diagnostic> {
    on_compile_stack(move || {
        let FrontEnd { resolved, typecheck: tc, lowered, .. } =
            front_end(input.source, input.file, input.module_name, input.fold_mode, loader);
        let cycles = crate::analyze::analyze_cycles(&lowered.module);
        let mut all: Vec<_> = resolved
            .diagnostics
            .into_iter()
            .chain(tc.diagnostics)
            .chain(lowered.diagnostics)
            .chain(cycles.diagnostics)
            .collect();
        // `// ws-ignore-line` / `// ws-ignore-file` cover the stages after
        // resolve too, so they are applied to the whole set rather than to
        // resolve's own diagnostics alone. Errors survive either way, a
        // placeholder promoted to one included, so they are applied last.
        crate::diagnostic::drop_internal_errors_beside_errors(&mut all);
        crate::diagnostic::promote_placeholders(&mut all);
        resolved.suppressions.apply(&mut all);
        all
    })
}

/// The module-level settings a resolved program asks for: its `@layout`
/// engine, `@invisible`, the root plane's doc, and, for `@layout("code")`,
/// the entry file's source map.
pub fn program_settings(resolved: &crate::resolve::ResolveResult, file: &str) -> ProgramSettings {
    let ast = &resolved.ast;
    let layout = match ast.layout {
        Some(crate::ast::LayoutName::Code) => LayoutMode::Code,
        Some(crate::ast::LayoutName::Cube) => LayoutMode::Cube,
        None => LayoutMode::Dag,
    };
    ProgramSettings {
        source_file: Some(file.to_string()),
        layout,
        invisible: ast.invisible,
        // `@layout("cube")` packs gates into an unreadable block, so the
        // per-gate labels are dropped there as pure payload.
        no_gate_labels: layout == LayoutMode::Cube,
        // A compile option, not the source's: the compile sets it.
        no_fold: false,
        // An explicit top-of-file module doc (a `///` block separated from the
        // first decl by a blank line) is the root header; otherwise fall back
        // to the first declaration's doc comment.
        module_doc: ast.module_doc.clone().or_else(|| {
            ast.decls
                .first()
                .and_then(|d| resolved.doc_comments.get(&crate::parser::doc_key(d.range())))
                .cloned()
        }),
        source_map: (layout == LayoutMode::Code).then(|| resolved.source_map.clone()),
    }
}

/// Front end through emit, shared by every entry point that produces output.
///
/// `progress` is `None` for a plain compile; when it is `Some`, the phase
/// callbacks fire and the prefab resolvers are wrapped so each embedded prefab
/// advances the bar. The terminal `done` event is NOT sent here: `compile`
/// sends it after the `.brz` encode, which is work this function does not do.
fn compile_to_world_inner(
    input: CompileInput<'_>,
    mut opts: EmitOptions,
    progress: Option<ProgressCallback>,
    loader: &dyn FileLoader,
) -> Result<CompileWorldResult, CompileError> {
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
        move |label: &'static str| {
            let s = step.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(cb) = &progress {
                cb(CompileProgress {
                    step: s,
                    total: total.load(Ordering::Relaxed),
                    done: false,
                    label,
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

    let fe = front_end_staged(source, file, module_name, input.fold_mode, loader, &mut |stage| match stage {
        FrontEndStage::Resolve => report("resolve"),
        FrontEndStage::Typecheck => {}
        FrontEndStage::Lower(resolved) => {
            // Grow the progress total by the number of embedded prefabs (each `$./file`
            // reference and each inline `$```…``` ` block is a sub-compile the emit step
            // does), so the bar advances through them instead of stalling on emit. Only
            // `total`, which nothing reads unless a callback is running, so the walk is
            // skipped entirely when it is.
            if progress.is_some() {
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
            report("lower");
        }
    });
    let FrontEnd { resolved, typecheck: tc, lowered, .. } = fe;
    let mut settings = program_settings(&resolved, file);
    settings.no_fold = input.fold_mode == FoldMode::ForceOff;

    // Every wire-graph cycle must cross a tick barrier (Buffer/Queue) — an
    // unbarriered cycle (e.g. an emit/await loop back-edge without `buffer`)
    // would retrigger within a single tick (WS005).
    let cycles = crate::analyze::analyze_cycles(&lowered.module);

    let mut all_diags: Vec<_> = resolved
        .diagnostics
        .into_iter()
        .chain(tc.diagnostics)
        .chain(lowered.diagnostics)
        .chain(cycles.diagnostics)
        .collect();
    // Same as `diagnostics_only`: the `ws-ignore` directives cover every
    // stage, and are applied after the promotion, which they cannot undo.
    crate::diagnostic::drop_internal_errors_beside_errors(&mut all_diags);
    crate::diagnostic::promote_placeholders(&mut all_diags);
    resolved.suppressions.apply(&mut all_diags);

    let errors: Vec<_> = all_diags
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error))
        .cloned()
        .collect();
    if !errors.is_empty() {
        return Err(CompileError::HasErrors(errors));
    }

    let program = Program { module: lowered.module, settings };

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
                    cb(CompileProgress { step: s, total: tot, done: false, label: "prefab" });
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

    let Assembly { world, layout } =
        assemble_with_phases(&program, &opts, &mut |phase| report(phase)).map_err(CompileError::Emit)?;

    Ok(CompileWorldResult {
        world,
        diagnostics: all_diags,
        layout,
        program,
        progress: progress.map(|cb| (cb, total)),
    })
}

fn compile_with_opts_inner(
    input: CompileInput<'_>,
    opts: EmitOptions,
    progress: Option<ProgressCallback>,
    loader: &dyn FileLoader,
) -> Result<CompileResult, CompileError> {
    let built = compile_to_world_inner(input, opts, progress, loader)?;
    let brz = built
        .world
        .to_brz_vec()
        .map_err(|e| CompileError::Emit(EmitError::Brdb(e)))?;

    if let Some((cb, total)) = built.progress {
        let tot = total.load(std::sync::atomic::Ordering::Relaxed);
        cb(CompileProgress { step: tot, total: tot, done: true, label: "done" });
    }

    Ok(CompileResult {
        brz,
        diagnostics: built.diagnostics,
        placements: built.layout.placements,
    })
}

pub struct CompileWorldResult {
    pub world: brdb::World,
    pub diagnostics: Vec<Diagnostic>,
    /// Where the world's bricks were laid out: every node's cell, chip
    /// interiors and the bus included.
    pub layout: crate::layout::LayoutResult,
    /// The program the world was assembled from.
    pub program: Program,
    /// Set only when the caller asked for progress, carrying the callback and
    /// the final step total so `compile` can send the terminal `done` event
    /// after the `.brz` encode rather than before it.
    progress: Option<(ProgressCallback, std::sync::Arc<std::sync::atomic::AtomicU32>)>,
}

pub fn compile_to_world(
    input: CompileInput<'_>,
    opts: EmitOptions,
) -> Result<CompileWorldResult, CompileError> {
    on_compile_stack(move || compile_to_world_inner(input, opts, None, &FsLoader))
}

/// Make `program`, compiled from `entry_file`, stand on its own as text
/// written to `ir_path`: every nested prefab block is compiled to its `.brz`
/// bytes, so assembling it needs no front end, and every prefab file path is
/// rewritten relative to `ir_path`'s directory, which is where `assemble`
/// resolves it.
pub fn prepare_program_for_text(
    program: &mut Program,
    entry_file: &str,
    ir_path: &std::path::Path,
    fold_mode: FoldMode,
) -> Result<(), String> {
    let nested = default_nested_compiler(1, entry_file.to_string(), fold_mode);
    // A bare file name's parent is the empty path, which has no absolute
    // form; it means the current directory.
    let dir_of = |p: &std::path::Path| match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let source_dir = dir_of(std::path::Path::new(entry_file));
    let ir_dir = dir_of(ir_path);
    let ctx = PrefabRewrite { nested, source_dir, ir_dir };
    on_compile_stack(|| rewrite_module_prefabs(&mut program.module, &ctx))
}

struct PrefabRewrite {
    nested: crate::emit::NestedCompiler,
    source_dir: std::path::PathBuf,
    ir_dir: std::path::PathBuf,
}

fn rewrite_module_prefabs(m: &mut crate::ir::Module, ctx: &PrefabRewrite) -> Result<(), String> {
    for node in m.nodes.values_mut() {
        if !node.properties.values().any(has_prefab) {
            continue;
        }
        for lit in std::sync::Arc::make_mut(&mut node.properties).values_mut() {
            rewrite_literal(lit, ctx)?;
        }
    }
    for child in m.chips.values_mut() {
        rewrite_module_prefabs(child, ctx)?;
    }
    Ok(())
}

fn has_prefab(lit: &crate::ir::Literal) -> bool {
    use crate::ir::Literal;
    match lit {
        Literal::PrefabRef { .. } | Literal::NestedPrefab { .. } => true,
        Literal::Array(items) => items.iter().any(has_prefab),
        Literal::Map(entries) => entries.iter().any(|(k, v)| has_prefab(k) || has_prefab(v)),
        Literal::Record(fields) => fields.iter().any(|(_, v)| has_prefab(v)),
        _ => false,
    }
}

fn rewrite_literal(lit: &mut crate::ir::Literal, ctx: &PrefabRewrite) -> Result<(), String> {
    use crate::ir::Literal;
    match lit {
        Literal::NestedPrefab { source } => {
            let bytes = (ctx.nested.0)(source, 1).map_err(|e| format!("nested prefab: {e}"))?;
            *lit = Literal::PrefabBytes(bytes.into());
        }
        Literal::PrefabRef { path } => {
            // Absolute references stay as written; the rest resolve against
            // the source file's directory, exactly as `disk_prefab_resolver`
            // does, and are re-expressed from the IR file's directory.
            if !path.starts_with('/') {
                let full = prefab_path_in(&ctx.source_dir, path);
                *path = relative_path(&ctx.ir_dir, &full)?;
            }
        }
        Literal::Array(items) => {
            for it in items {
                rewrite_literal(it, ctx)?;
            }
        }
        Literal::Map(entries) => {
            for (k, v) in entries {
                rewrite_literal(k, ctx)?;
                rewrite_literal(v, ctx)?;
            }
        }
        Literal::Record(fields) => {
            for (_, v) in fields {
                rewrite_literal(v, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// `target` as a path relative to the directory `from`, with `/` separators.
fn relative_path(from: &std::path::Path, target: &std::path::Path) -> Result<String, String> {
    use std::path::Component;
    // The absolute path as its components, with `.` and `..` folded away.
    fn parts(p: &std::path::Path) -> Result<Vec<std::ffi::OsString>, String> {
        let abs = std::path::absolute(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mut out = Vec::new();
        let mut anchor = 0;
        for c in abs.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    if out.len() > anchor {
                        out.pop();
                    }
                }
                Component::Prefix(_) | Component::RootDir => {
                    out.push(c.as_os_str().to_os_string());
                    anchor = out.len();
                }
                Component::Normal(s) => out.push(s.to_os_string()),
            }
        }
        Ok(out)
    }
    let from = parts(from)?;
    let target = parts(target)?;
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if common == 0 {
        // Different drives: no relative path exists.
        return Ok(std::path::absolute(target.iter().collect::<std::path::PathBuf>())
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned());
    }
    let mut out: Vec<String> = vec!["..".to_string(); from.len() - common];
    out.extend(target[common..].iter().map(|c| c.to_string_lossy().into_owned()));
    Ok(out.join("/"))
}

/// Emit options for assembling a program read from `ir_file`: prefab paths
/// resolve against its directory, and a nested prefab block left as source
/// is compiled by the front end.
pub fn assemble_options(ir_file: &str, fold_mode: FoldMode) -> EmitOptions {
    EmitOptions {
        prefab_resolver: Some(disk_prefab_resolver(ir_file, fold_mode)),
        nested_compiler: Some(default_nested_compiler(1, ir_file.to_string(), fold_mode)),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
