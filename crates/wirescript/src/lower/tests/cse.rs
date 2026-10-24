use super::*;
use crate::ir::gate_class as gc;

const ADD: &str = "BrickComponentType_Expr_MathAdd";
const MUL: &str = "BrickComponentType_Expr_MathMultiply";
const SUB: &str = "BrickComponentType_Expr_MathSubtract";
const GT: &str = "BrickComponentType_Expr_CompareGreater";
const AND: &str = "BrickComponentType_Expr_LogicalAND";

// A handler wrapper for exec-context programs.
fn go(body: &str) -> String {
    format!("in go: exec\non go {{\n{body}\n}}")
}

#[test]
fn three_uses_merge_to_one_and_fan_out_to_all() {
    // The merge must be behavior-preserving: the surviving gate feeds every
    // original consumer, so no output loses its value.
    let r = compile_ok("var x: int = 5\nout y = x + 1\nout z = x + 1\nout w = x + 1");
    assert_eq!(
        count_class(&r.module, ADD),
        1,
        "three `x + 1` collapse to one"
    );
    let add = nodes_of(&r.module, ADD)[0].id;
    assert_eq!(
        r.module
            .wires_deep()
            .filter(|w| w.source.node_id == add)
            .count(),
        3,
        "the keeper feeds all three outputs"
    );
}

/// Identical pure subexpressions collapse to one gate; gates that only LOOK
/// alike stay separate. Each row gives exact counts over the whole module tree.
#[test]
fn identical_pure_gates_merge_and_lookalikes_do_not() {
    let rows: Vec<(&str, String, &[(&str, usize)])> = vec![
        // The pass is a fixpoint: it merges the leaves first, which makes their
        // consumers structurally equal, which merge next round, up the tree.
        (
            "a fully-duplicated five-deep chain merges at every layer",
            "var x: int = 5\nout a = ((((x + 1) * 2) - 3) % 7) / 4\nout b = ((((x + 1) * 2) - 3) % 7) / 4".into(),
            &[(ADD, 1), (MUL, 1), (SUB, 1), ("BrickComponentType_Expr_MathModulo", 1), ("BrickComponentType_Expr_MathDivide", 1)],
        ),
        (
            "a shared deep `x + 1` merges under divergent tops, which stay separate",
            "var x: int = 5\nout a = (x + 1) * 2\nout b = (x + 1) * 3".into(),
            &[(ADD, 1), (MUL, 2)],
        ),
        ("identical FormatText merges", "var s: string\nout a = \"hi ${s}\"\nout b = \"hi ${s}\"".into(), &[(gc::STRING_FORMAT_TEXT, 1)]),
        ("string equality merges", "var s: string\nout a = s == \"x\"\nout b = s == \"x\"".into(), &[(gc::COMPARE_EQUAL, 1)]),
        ("comparison merges", "var x: int = 5\nout a = x > 3\nout b = x > 3".into(), &[(GT, 1)]),
        ("boolean op merges", "var p: bool\nvar q: bool\nout a = p && q\nout b = p && q".into(), &[(AND, 1)]),
        ("vector op merges", "var v: vector\nout a = v + Vec(1.0, 0.0, 0.0)\nout b = v + Vec(1.0, 0.0, 0.0)".into(), &[(ADD, 1)]),
        ("same operand order merges", "var x: int = 5\nvar y: int = 3\nout a = x - y\nout b = x - y".into(), &[(SUB, 1)]),
        (
            "two inlined `dbl(x)` bodies in one module merge",
            "var x: int = 5\nmod dbl(v: int) -> int { return v * 2 }\nout y = dbl(x)\nout z = dbl(x)".into(),
            &[(MUL, 1)],
        ),
        ("a different baked constant keeps both", "var x: int = 5\nout a = x + 1\nout b = x + 2".into(), &[(ADD, 2)]),
        ("a different source var keeps both", "var x: int = 5\nvar y: int = 3\nout a = x + 1\nout b = y + 1".into(), &[(ADD, 2)]),
        // The key is port-keyed: `x - y` and `y - x` bind different sources to InputA/InputB.
        (
            "swapped operands of a non-commutative gate keep both",
            "var x: int = 5\nvar y: int = 3\nout a = x - y\nout b = y - x".into(),
            &[(SUB, 2)],
        ),
        // The second `x + 1` reads a FRESH Var_Get after the write, or `b` would
        // compute from the pre-mutation value.
        (
            "a mutation between two reads keeps both adds",
            format!("var x: int = 5\nvar a: int = 0\nvar b: int = 0\n{}", go("  a = x + 1\n  x = 10\n  b = x + 1")),
            &[(ADD, 2)],
        ),
        // The join invalidates `x`'s read cache, so the post-branch `x + 1` is fresh.
        (
            "a mutation inside a branch keeps both adds",
            format!("var x: int = 5\nvar a: int = 0\nvar b: int = 0\n{}", go("  a = x + 1\n  if a > 0 { x = 99 }\n  b = x + 1")),
            &[(ADD, 2)],
        ),
        // Change/Edge outputs depend on eval history, not just current inputs.
        ("change detectors stay separate", "var x: int = 5\nout a = Changed(x)\nout b = Changed(x)".into(), &[(gc::CHANGE_DETECTOR, 2)]),
        ("edge detectors stay separate", "var x: float = 5.0\nout a = Edge(x).Rising\nout b = Edge(x).Rising".into(), &[(gc::EDGE_DETECTOR, 2)]),
        // A `@nofold` declaration marks its gates with the `_nofold` barrier, which CSE respects.
        (
            "a and b merge to one; the @nofold c stays separate",
            "var x: int = 5\nout a = x + 1\nout b = x + 1\n@nofold out c = x + 1".into(),
            &[(ADD, 2)],
        ),
        // Merging across chip instances would land a keeper in the wrong grid.
        (
            "each chip instance keeps its own body",
            "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nvar x: int = 5\nlet p = Inc(x)\nlet q = Inc(x)\nout oa = p\nout ob = q".into(),
            &[(ADD, 2)],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}\n{src}"));
            continue;
        }
        for &(class, n) in *want {
            let got = count_class(&r.module, class);
            if got != n {
                failures.push(format!("{label}: {got} x {class}, want {n}\n{src}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
