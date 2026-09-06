//! The probe and verifier circuits are saturated with Opaque/@nofold —
//! the fold pass must be a structural no-op on them. This is the standing
//! proof that the optimizer cannot touch the instruments that certify it.

mod common;

use wirescript::ir::Module;
use wirescript::lower::FoldMode;
use wirescript::Severity;

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
/// Deliberately `ForceOn`/`ForceOff` on the two sides, NOT `Auto`: the
/// invariant is that folding is a structural no-op on these files, so the two
/// sides must differ in whether the pass runs. `Auto` now folds by default
/// (identical to `ForceOn`), so using it for both calls would run the pass on
/// both sides and the test would be trivially green without ever comparing
/// folded to unfolded — defeating the whole point of the invariant.
fn counts(file: &str, fold_mode: FoldMode) -> (usize, usize) {
    let source = std::fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("cannot read probe file {file}: {e}"));
    let (resolved, tc, lowered) = common::run_stages(&source, file, fold_mode);
    assert!(
        resolved.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "resolve errors in {file}: {:?}",
        resolved.diagnostics
    );
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors in {file}: {:?}",
        tc.diagnostics
    );
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
