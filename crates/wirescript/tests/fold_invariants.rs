//! The probe and verifier circuits are saturated with Opaque/@nofold —
//! the fold pass must be a structural no-op on them. This is the standing
//! proof that the optimizer cannot touch the instruments that certify it.
use std::sync::Arc;

use wirescript::ir::Module;
use wirescript::lower::{lower, FoldMode, LowerInput};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::typecheck;
use wirescript::{resolve, FsLoader, Severity};

/// Recursively sum (node count, wire count) over `m` and every nested chip
/// module — the fold pass operates tree-wide (chip boundaries included), so
/// the invariant must hold over the whole tree, not just the root module.
fn count_module(m: &Module) -> (usize, usize) {
    let mut nodes = m.nodes.len();
    let mut wires = m.wires.len();
    for chip in m.chips.values() {
        let (n, w) = count_module(chip);
        nodes += n;
        wires += w;
    }
    (nodes, wires)
}

/// Resolve + typecheck + lower `file` with the fold pass forced on or off
/// (`fold_mode`), then count the resulting IR tree. This mirrors the CLI's
/// `--dump-ir` path (`resolve` -> `typecheck` -> `lower`), the same public
/// entry points the CLI uses to reach the lowered `Module` — the `compile*`
/// entries in `src/compile.rs` only return emitted bytes/world, not the
/// intermediate `Module`, so there's nothing further to count there.
///
/// Deliberately `ForceOn`/`ForceOff`, NOT `Auto`: the probe/verifier files
/// carry no module-level `@fold`, so under `Auto` both calls below would
/// skip folding identically and this test would be trivially green without
/// ever exercising the pass — defeating the whole point of the invariant.
fn counts(file: &str, fold_mode: FoldMode) -> (usize, usize) {
    let source = std::fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("cannot read probe file {file}: {e}"));
    let resolved = resolve(&source, file, &FsLoader);
    assert!(
        resolved.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "resolve errors in {file}: {:?}",
        resolved.diagnostics
    );
    let tc = typecheck(&resolved.ast, file);
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors in {file}: {:?}",
        tc.diagnostics
    );
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode,
    });
    assert!(
        lowered.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "lower errors in {file} (fold_mode={fold_mode:?}): {:?}",
        lowered.diagnostics
    );
    count_module(&lowered.module)
}

#[test]
fn probe_is_fold_invariant() {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/probes/gate_semantics.ws");
    assert_eq!(counts(p, FoldMode::ForceOff), counts(p, FoldMode::ForceOn));
}

#[test]
fn verifier_is_fold_invariant() {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/probes/verify_semantics.ws");
    assert_eq!(counts(p, FoldMode::ForceOff), counts(p, FoldMode::ForceOn));
}
