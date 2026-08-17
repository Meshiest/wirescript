//! Tree-shaking must happen by NOT BUILDING the branch, never by deleting one
//! afterwards. `lower_if` snapshots and restores the Var_Get cache around
//! branch boundaries (`lower/stmt.rs:679-775`); a read cached while lowering
//! the THEN block is only valid on that exec chain. The existing literal-bool
//! elision is safe precisely because it lowers straight into the parent scope
//! and never creates that machinery — this extension must stay on that path.
use super::*;

/// Resolve, typecheck and lower `src`, asserting no errors, and return the IR.
fn lower_ok(src: &str) -> crate::ir::Module {
    let r = compile(src);
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    r.module
}

/// Every node of gate class `gate_class::BRANCH` in `m` and its nested chip
/// modules.
fn branch_gate_count(m: &crate::ir::Module) -> usize {
    let mut n = m
        .nodes
        .values()
        .filter(|node| node.gate_class == crate::ir::gate_class::BRANCH)
        .count();
    for chip in m.chips.values() {
        n += branch_gate_count(chip);
    }
    n
}

#[test]
fn a_const_condition_drops_the_untaken_block_entirely() {
    let taken = lower_ok(
        "const N = 2\nvar x: int = 0\nin go: exec\non go { if N > 1 { x = 1 } else { x = 2 } }",
    );
    assert_eq!(branch_gate_count(&taken), 0, "a const condition must not emit a Branch gate");
}

#[test]
fn a_runtime_condition_still_emits_a_branch() {
    let m = lower_ok("in live: bool\nvar x: int = 0\nin go: exec\non go { if live { x = 1 } }");
    assert_eq!(branch_gate_count(&m), 1);
}

// IMPORTANT: the widened elision (any const-eval-decidable condition, not
// just the old bare `true`/`false`/ident-bound-to-a-literal-bool-gate cases)
// must fire ONLY for a condition built from names DECLARED `const` — never a
// plain `let` that merely happens to fold. This is the feature's own first
// design principle: a program using no `const` keyword must compile
// identically to before the feature existed. Before this restriction, `let
// A = 1` here newly dropped the Branch (measured: 3 nodes instead of a
// Branch plus both blocks) — a behavior change for a program with no
// `const` anywhere. Mirrors `typecheck::tests::a_plain_let_condition_keeps_its_branch_and_checks_both_blocks`.
#[test]
fn a_plain_let_condition_still_emits_a_branch() {
    let m = lower_ok(
        "let A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { x = 1 } else { x = 2 } }",
    );
    assert_eq!(
        branch_gate_count(&m),
        1,
        "a plain `let` (no `const` keyword anywhere) must not gain the widened elision"
    );
}

// The same shape, but with `const` instead of `let`, must still tree-shake —
// proving the restriction above is keyed on the `const` keyword, not on some
// accidental difference between the two programs.
#[test]
fn a_const_condition_with_the_same_shape_drops_the_branch() {
    let m = lower_ok(
        "const A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { x = 1 } else { x = 2 } }",
    );
    assert_eq!(branch_gate_count(&m), 0, "a real `const` must still elide the branch");
}
