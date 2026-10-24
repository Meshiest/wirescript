use super::*;
use crate::ir::gate_class as gc;

// In each program `v` is read for `w = v`, `w` once for the condition, and `v`
// again for `u = v`. When the branch does NOT write `v`, the two reads of `v`
// share one Var_Get (2 total); when it DOES, the post-`if` read must be fresh
// (3 total) or it would see the pre-branch value whenever the writing branch
// was skipped.
const PRELUDE: &str = "var v: int = 5\nvar w: int = 0\nvar u: int = 0\nin go: exec\n";

/// How many Var_Gets a handler's reads lower to, across the whole module tree.
#[test]
fn a_cached_var_read_is_reused_until_something_may_have_written_the_var() {
    let rows: Vec<(&str, String, usize)> = vec![
        (
            "`v` isn't written in the branch, so both reads share one Var_Get",
            format!("{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ w = 1 }}\n  u = v\n}}"),
            2,
        ),
        (
            "`v` is written in THEN, so the post-`if` read is a fresh Var_Get",
            format!("{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ v = 9 }}\n  u = v\n}}"),
            3,
        ),
        (
            "`v` is written in ELSE, so the post-`if` read is a fresh Var_Get",
            format!("{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ }} else {{ v = 9 }}\n  u = v\n}}"),
            3,
        ),
        (
            "a write nested one level deep still forces a fresh post-`if` read",
            format!("{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ if w > 1 {{ v = 9 }} }}\n  u = v\n}}"),
            3,
        ),
        // The inlined `mod` writes the caller's `v` through a ref param; the
        // state-diff catches its blanket cache reset even though it never calls
        // the per-var invalidation directly.
        (
            "a ref-param write inside the branch forces a fresh post-`if` read",
            format!("mod setv(x: *int) {{ x = 9 }}\n{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ setv(v) }}\n  u = v\n}}"),
            3,
        ),
        // A chip INSTANCE runs in a separate module, so its writes to the global
        // `g` don't flow through the caller's per-write invalidation; the caller
        // drops its cached read of `g` after the call. The chip's `g = g + 1` is
        // a Var_Increment, so both Var_Gets are the caller's reads of `g`.
        (
            "the read of `g` after a chip call that writes it is fresh",
            "var g: int = 0\nvar a: int = 0\nvar b: int = 0\nchip Bump() { g = g + 1 }\nin go: exec\non go {\n  a = g + 1\n  Bump()\n  b = g + 1\n}"
                .to_string(),
            2,
        ),
        // A Var_Get emitted inside a branch only fires when that branch is taken,
        // so a read after the join gets its own Var_Get on the unconditional
        // chain. Bug: `gamblersTotal`, first read inside one `if phase == ...`
        // block and re-read in a later one, came back stale 0 and skipped a phase.
        (
            "a read after the join does not reuse the in-branch Var_Get",
            "in go: exec\nin t: bool\n\
             static var a: int = 0\nstatic var b: int = 0\nstatic var c: int = 0\n\
             on go { if t { b = a }\n c = a }"
                .to_string(),
            2,
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let got = count_class(&r.module, gc::VAR_GET);
        if !errors.is_empty() || got != *want {
            failures.push(format!(
                "{label}: errors {errors:?}, {got} Var_Get(s), want {want}\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
