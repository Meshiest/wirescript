use super::*;

fn count_class(m: &crate::ir::Module, class: &str) -> usize {
    let mut n = m.nodes.values().filter(|x| x.gate_class == class).count();
    for c in m.chips.values() {
        n += count_class(c, class);
    }
    n
}

const VAR_GET: &str = "BrickComponentType_WireGraph_Exec_Var_Get";

// In each program `v` is read for `w = v`, `w` once for the condition, and `v`
// again for `u = v`. When the branch does NOT write `v`, the two reads of `v`
// share one Var_Get (2 total); when it DOES, the post-`if` read must be fresh
// (3 total) or it would see the pre-branch value whenever the writing branch
// was skipped.
const PRELUDE: &str = "var v: int = 5\nvar w: int = 0\nvar u: int = 0\nin go: exec\n";

#[test]
fn unwritten_var_is_reused_across_an_if() {
    let r = compile(&format!("{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ w = 1 }}\n  u = v\n}}"));
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    assert_eq!(
        count_class(&r.module, VAR_GET),
        2,
        "`v` isn't written in the branch, so both reads share one Var_Get"
    );
}

#[test]
fn var_written_in_then_is_reread_fresh() {
    let r = compile(&format!("{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ v = 9 }}\n  u = v\n}}"));
    assert_eq!(
        count_class(&r.module, VAR_GET),
        3,
        "`v` is written in THEN, so the post-`if` read must be a fresh Var_Get"
    );
}

#[test]
fn var_written_in_else_is_reread_fresh() {
    let r = compile(&format!(
        "{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ }} else {{ v = 9 }}\n  u = v\n}}"
    ));
    assert_eq!(
        count_class(&r.module, VAR_GET),
        3,
        "`v` is written in ELSE, so the post-`if` read must be a fresh Var_Get"
    );
}

#[test]
fn var_written_in_nested_if_is_reread_fresh() {
    let r = compile(&format!(
        "{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ if w > 1 {{ v = 9 }} }}\n  u = v\n}}"
    ));
    assert_eq!(
        count_class(&r.module, VAR_GET),
        3,
        "a write nested one level deep still forces a fresh post-`if` read"
    );
}

#[test]
fn global_written_by_chip_instance_is_reread_fresh() {
    // A chip INSTANCE runs in a separate module, so its writes don't flow
    // through the caller's per-write invalidation. It writes the global `g`
    // directly (not via a param), so the caller's cached read of `g` must be
    // dropped after the call — otherwise the read after it sees the pre-call
    // value. (`v` here is a distraction that stays reused; only `g` re-reads.)
    let r = compile(
        "var g: int = 0\nvar a: int = 0\nvar b: int = 0\nchip Bump() { g = g + 1 }\nin go: exec\non go {\n  a = g + 1\n  Bump()\n  b = g + 1\n}",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    // Two reads of `g` straddling the chip call must be distinct gates.
    let g_reads = count_class(&r.module, VAR_GET);
    assert!(
        g_reads >= 2,
        "the post-call read of `g` must be fresh, not the stale pre-call read (got {g_reads} Var_Gets)"
    );
}

#[test]
fn var_written_via_mod_ref_in_branch_is_reread_fresh() {
    // The inlined `mod` writes the caller's `v` through a ref param; its cache
    // reset must propagate so the post-`if` read is fresh — the state-diff
    // catches the mod's blanket reset even though it never calls the
    // per-var invalidation directly.
    let r = compile(&format!(
        "mod setv(x: *int) {{ x = 9 }}\n{PRELUDE}on go {{\n  w = v\n  if w > 0 {{ setv(v) }}\n  u = v\n}}"
    ));
    assert_eq!(
        count_class(&r.module, VAR_GET),
        3,
        "a ref-param write inside the branch must force a fresh post-`if` read"
    );
}
