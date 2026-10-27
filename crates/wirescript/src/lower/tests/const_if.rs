//! Tree-shaking happens by not building the branch, never by deleting one afterwards.
//! `lower_if` snapshots and restores the `Var_Get` cache around branch boundaries
//! (`lower/stmt.rs`): a read cached while lowering the THEN block is valid only on that
//! exec chain. Const-condition elision lowers straight into the parent scope and never
//! creates that machinery, so it must stay on that path.
use super::*;
use crate::ir::gate_class as gc;

/// How many Branch gates each program lowers to, across its whole module tree.
/// Elision fires only for a condition built from names declared `const`, never
/// a plain `let` that merely folds (as in `typecheck::tests::
/// a_plain_let_condition_keeps_its_branch_and_checks_both_blocks`).
#[test]
fn a_condition_keeps_its_branch_unless_it_is_built_from_consts() {
    let rows: &[(&str, &str, usize)] = &[
        (
            "a const condition drops the untaken block entirely",
            "const N = 2\nvar x: int = 0\nin go: exec\non go { if N > 1 { x = 1 } else { x = 2 } }",
            0,
        ),
        (
            "a runtime condition still emits a branch",
            "in live: bool\nvar x: int = 0\nin go: exec\non go { if live { x = 1 } }",
            1,
        ),
        (
            "a plain `let` (no `const` keyword anywhere) does not gain the widened elision",
            "let A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { x = 1 } else { x = 2 } }",
            1,
        ),
        (
            "the same shape with `const` instead of `let` drops the branch",
            "const A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { x = 1 } else { x = 2 } }",
            0,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let got = count_class(&r.module, gc::BRANCH);
        if !errors.is_empty() || got != want {
            failures.push(format!(
                "{label}: errors {errors:?}, {got} Branch gate(s), want {want}\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
