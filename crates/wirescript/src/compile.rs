use std::collections::HashMap;

use crate::diagnostic::{Diagnostic, Severity};
use crate::emit::Placement;
use crate::emit::{EmitError, EmitOptions, build_world, emit_brz};
use crate::ir::NodeId;
use crate::layout::layout;
use crate::lower::{LowerInput, lower};
use crate::resolve::{FsLoader, resolve};
use crate::template_cache::TemplateCache;
use crate::typecheck::typecheck;

pub struct CompileInput<'a> {
    pub source: &'a str,
    pub file: &'a str,
    pub module_name: Option<&'a str>,
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
    compile_with_opts_inner(input, opts, Some(progress))
}

pub fn compile_with_opts(
    input: CompileInput<'_>,
    opts: EmitOptions,
) -> Result<CompileResult, CompileError> {
    compile_with_opts_inner(input, opts, None)
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

    report(&progress);
    let resolved = resolve(source, file, &FsLoader);
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
    });

    let all_diags: Vec<_> = resolved
        .diagnostics
        .into_iter()
        .chain(tc.diagnostics)
        .chain(lowered.diagnostics)
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

    const EDGE_MARGIN: i32 = 5;
    const MIN_EXTENT: i32 = 5;
    let span_x = (lr.bounds_max.x - lr.bounds_min.x) / 2;
    let span_y = (lr.bounds_max.y - lr.bounds_min.y) / 2;
    let extent_x = (span_x + EDGE_MARGIN).max(MIN_EXTENT);
    let extent_y = (span_y + EDGE_MARGIN).max(MIN_EXTENT);

    opts.inner_plane_extent = brdb::IntVector {
        x: extent_x,
        y: extent_y,
        z: 2,
    };
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
    let brz = emit_brz(&lowered.module, &lr.placements, &opts, &template_cache)
        .map_err(CompileError::Emit)?;

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
    mut opts: EmitOptions,
) -> Result<CompileWorldResult, CompileError> {
    let source = input.source;
    let file = input.file;
    let module_name = input.module_name;
    let t0 = std::time::Instant::now();
    let resolved = resolve(source, file, &FsLoader);
    let tc = typecheck(&resolved.ast, file);

    let template_cache = std::sync::Arc::new(TemplateCache::new());

    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name,
        template_cache: template_cache.clone(),
    });

    let all_diags: Vec<_> = resolved
        .diagnostics
        .into_iter()
        .chain(tc.diagnostics)
        .chain(lowered.diagnostics)
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

    const EDGE_MARGIN: i32 = 5;
    const MIN_EXTENT: i32 = 5;
    let span_x = (lr.bounds_max.x - lr.bounds_min.x) / 2;
    let span_y = (lr.bounds_max.y - lr.bounds_min.y) / 2;
    opts.inner_plane_extent = brdb::IntVector {
        x: (span_x + EDGE_MARGIN).max(MIN_EXTENT),
        y: (span_y + EDGE_MARGIN).max(MIN_EXTENT),
        z: 2,
    };
    if opts.description.is_empty() {
        opts.description = format!(
            "wirescript compile: {}",
            std::path::Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }

    let world = build_world(&lowered.module, &lr.placements, &opts, &template_cache)
        .map_err(CompileError::Emit)?;
    eprintln!("[compile] total: {:.2}s", t0.elapsed().as_secs_f64());

    Ok(CompileWorldResult {
        world,
        diagnostics: all_diags,
        placements: lr.placements,
    })
}
