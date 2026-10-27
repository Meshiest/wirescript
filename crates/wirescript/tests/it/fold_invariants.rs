//! The probe and verifier circuits are saturated with Opaque/@nofold, so the
//! fold pass must be a structural no-op on them: the optimizer must not touch
//! the circuits that certify it.

use crate::common;

use wirescript::FoldMode;
use wirescript::Severity;
use wirescript::ir::Module;

/// Recursively sum (node count, wire count) over `m` and every nested chip
/// module. The fold pass operates tree-wide, so the invariant must hold over
/// the whole tree, not just the root module.
fn count_module(m: &Module) -> (usize, usize) {
    let (mut nodes, mut wires) = (0, 0);
    m.walk(&mut |_, sub| {
        nodes += sub.nodes.len();
        wires += sub.wires.len();
    });
    (nodes, wires)
}

/// Resolve + typecheck + lower `file` with the fold pass forced on or off
/// (`fold_mode`), then count the resulting IR tree. This follows the CLI's
/// `--dump-ir` path (`resolve`, `typecheck`, `lower`), since the `compile*`
/// entries in `src/compile.rs` return only emitted bytes or a world, not the
/// intermediate `Module`.
///
/// Callers pass `ForceOn` and `ForceOff`, never `Auto`: `Auto` folds by default,
/// so using it on both sides would run the pass on both and never compare
/// folded to unfolded.
fn counts(file: &str, fold_mode: FoldMode) -> (usize, usize) {
    let source = std::fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("cannot read probe file {file}: {e}"));
    let (resolved, tc, lowered) = common::run_stages(&source, file, fold_mode);
    assert!(
        resolved
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "resolve errors in {file}: {:?}",
        resolved.diagnostics
    );
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors in {file}: {:?}",
        tc.diagnostics
    );
    assert!(
        lowered
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
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
