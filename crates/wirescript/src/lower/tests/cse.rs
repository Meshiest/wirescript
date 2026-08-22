use super::*;

fn count_class(m: &crate::ir::Module, class: &str) -> usize {
    let mut n = m.nodes.values().filter(|x| x.gate_class == class).count();
    for c in m.chips.values() {
        n += count_class(c, class);
    }
    n
}

/// Every wire in the tree whose source is `node` (any output port) — the
/// consumer count a merged gate must still feed.
fn fanout(m: &crate::ir::Module, node: crate::ir::NodeId) -> usize {
    let mut n = m.wires.iter().filter(|w| w.source.node_id == node).count();
    for c in m.chips.values() {
        n += fanout(c, node);
    }
    n
}

fn find_one(m: &crate::ir::Module, class: &str) -> Option<crate::ir::NodeId> {
    m.nodes
        .iter()
        .find(|(_, n)| n.gate_class == class)
        .map(|(id, _)| *id)
        .or_else(|| m.chips.values().find_map(|c| find_one(c, class)))
}

fn no_errors(r: &LowerResult) {
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
}

const ADD: &str = "BrickComponentType_WireGraph_Expr_MathAdd";
const MUL: &str = "BrickComponentType_WireGraph_Expr_MathMultiply";
const SUB: &str = "BrickComponentType_WireGraph_Expr_MathSubtract";
const GT: &str = "BrickComponentType_WireGraph_Expr_CompareGreater";
const EQ: &str = "BrickComponentType_WireGraph_Expr_CompareEqual";
const AND: &str = "BrickComponentType_WireGraph_Expr_LogicalAND";
const FMT: &str = "BrickComponentType_WireGraph_Expr_String_FormatText";
const CHANGE: &str = "BrickComponentType_WireGraph_Expr_ChangeDetector";
const EDGE: &str = "BrickComponentType_WireGraph_Expr_EdgeDetector";

// A handler wrapper for exec-context programs.
fn go(body: &str) -> String {
    format!("in go: exec\non go {{\n{body}\n}}")
}

// ---------------------------------------------------------------------------
// Positive — identical pure subexpressions collapse to one gate.
// ---------------------------------------------------------------------------

#[test]
fn three_uses_merge_to_one_and_fan_out_to_all() {
    // The merge must be behavior-preserving: the surviving gate feeds every
    // original consumer, so no output loses its value.
    let r = compile("var x: int = 5\nout y = x + 1\nout z = x + 1\nout w = x + 1");
    no_errors(&r);
    assert_eq!(count_class(&r.module, ADD), 1, "three `x + 1` collapse to one");
    let add = find_one(&r.module, ADD).unwrap();
    assert_eq!(fanout(&r.module, add), 3, "the keeper feeds all three outputs");
}

#[test]
fn deep_expression_merges_at_every_layer() {
    // The pass is a fixpoint, so it scales to any depth: it merges the leaves
    // first, which makes their consumers structurally equal, which merge next
    // round, up the whole tree. A fully-duplicated five-deep chain collapses
    // every single layer, not just the first.
    let e = "((((x + 1) * 2) - 3) % 7) / 4";
    let r = compile(&format!("var x: int = 5\nout a = {e}\nout b = {e}"));
    no_errors(&r);
    for (class, name) in [
        (ADD, "add"),
        (MUL, "multiply"),
        (SUB, "subtract"),
        ("BrickComponentType_WireGraph_Expr_MathModulo", "modulo"),
        ("BrickComponentType_WireGraph_Expr_MathDivide", "divide"),
    ] {
        assert_eq!(count_class(&r.module, class), 1, "the {name} layer merges");
    }
}

#[test]
fn shared_deep_subexpression_merges_under_divergent_tops() {
    // Two expressions that share a deep subtree but diverge at the top: the
    // whole common subtree collapses, and only the parts that actually differ
    // stay separate. `(x + 1)` is shared and merges to one; the two multiplies
    // that consume it keep their distinct constants.
    let r = compile("var x: int = 5\nout a = (x + 1) * 2\nout b = (x + 1) * 3");
    no_errors(&r);
    assert_eq!(count_class(&r.module, ADD), 1, "the shared `x + 1` merges");
    assert_eq!(count_class(&r.module, MUL), 2, "the divergent tops stay separate");
}

#[test]
fn string_interpolation_merges() {
    let r = compile("var s: string\nout a = \"hi ${s}\"\nout b = \"hi ${s}\"");
    no_errors(&r);
    assert_eq!(count_class(&r.module, FMT), 1, "identical FormatText merges");
}

#[test]
fn string_equality_merges() {
    let r = compile("var s: string\nout a = s == \"x\"\nout b = s == \"x\"");
    no_errors(&r);
    assert_eq!(count_class(&r.module, EQ), 1);
}

#[test]
fn comparison_and_boolean_and_vector_ops_merge() {
    let cmp = compile("var x: int = 5\nout a = x > 3\nout b = x > 3");
    assert_eq!(count_class(&cmp.module, GT), 1, "comparison merges");

    let b = compile("var p: bool\nvar q: bool\nout a = p && q\nout b = p && q");
    assert_eq!(count_class(&b.module, AND), 1, "boolean op merges");

    let v = compile(
        "var v: vector\nout a = v + Vec(1.0, 0.0, 0.0)\nout b = v + Vec(1.0, 0.0, 0.0)",
    );
    assert_eq!(count_class(&v.module, ADD), 1, "vector op merges");
}

#[test]
fn commutative_gate_same_operand_order_merges() {
    let r = compile("var x: int = 5\nvar y: int = 3\nout a = x - y\nout b = x - y");
    assert_eq!(count_class(&r.module, SUB), 1);
}

#[test]
fn duplicate_inline_mod_body_is_collapsed() {
    // Two `dbl(x)` inline two identical `v * 2` bodies (same module) → merged.
    let r = compile(
        "var x: int = 5\nmod dbl(v: int) -> int { return v * 2 }\nout y = dbl(x)\nout z = dbl(x)",
    );
    no_errors(&r);
    assert_eq!(count_class(&r.module, MUL), 1);
}

// ---------------------------------------------------------------------------
// Negative — gates that only LOOK alike must stay separate.
// ---------------------------------------------------------------------------

#[test]
fn different_constant_operand_not_merged() {
    let r = compile("var x: int = 5\nout a = x + 1\nout b = x + 2");
    assert_eq!(count_class(&r.module, ADD), 2, "different baked constant");
}

#[test]
fn different_variable_operand_not_merged() {
    let r = compile("var x: int = 5\nvar y: int = 3\nout a = x + 1\nout b = y + 1");
    assert_eq!(count_class(&r.module, ADD), 2, "different source var");
}

#[test]
fn noncommutative_operand_order_not_merged() {
    // `x - y` and `y - x` bind different sources to InputA/InputB — the key is
    // port-keyed, so they must not merge (subtraction isn't commutative).
    let r = compile("var x: int = 5\nvar y: int = 3\nout a = x - y\nout b = y - x");
    assert_eq!(count_class(&r.module, SUB), 2);
}

#[test]
fn mutation_between_reads_is_not_merged() {
    // `x` is mutated between the two `x + 1`, so the second reads a FRESH
    // Var_Get (different source node) — the two adds must NOT merge, or `b`
    // would compute from the pre-mutation value.
    let full = format!(
        "var x: int = 5\nvar a: int = 0\nvar b: int = 0\n{}",
        go("  a = x + 1\n  x = 10\n  b = x + 1")
    );
    let r = compile(&full);
    no_errors(&r);
    assert_eq!(
        count_class(&r.module, ADD),
        2,
        "a mutation between reads must keep the two adds distinct"
    );
}

#[test]
fn mutation_inside_a_branch_is_not_merged() {
    // Same, but the write hides in a conditional: the join invalidates `x`'s
    // read cache, so the post-branch `x + 1` is a fresh gate.
    let full = format!(
        "var x: int = 5\nvar a: int = 0\nvar b: int = 0\n{}",
        go("  a = x + 1\n  if a > 0 { x = 99 }\n  b = x + 1")
    );
    let r = compile(&full);
    no_errors(&r);
    assert_eq!(count_class(&r.module, ADD), 2);
}

#[test]
fn stateful_change_detector_not_merged() {
    // `Changed`/`Edge` outputs depend on eval history, not just current inputs,
    // so two of them are excluded from CSE even with identical inputs.
    let r = compile("var x: int = 5\nout a = Changed(x)\nout b = Changed(x)");
    no_errors(&r);
    assert_eq!(count_class(&r.module, CHANGE), 2, "change detectors stay separate");
}

#[test]
fn stateful_edge_detector_not_merged() {
    let r = compile("var x: float = 5.0\nout a = Edge(x).Rising\nout b = Edge(x).Rising");
    no_errors(&r);
    assert_eq!(count_class(&r.module, EDGE), 2, "edge detectors stay separate");
}

#[test]
fn nofold_barriered_gates_not_merged() {
    // A `@nofold` declaration marks its gates with the `_nofold` barrier
    // property; CSE respects it and leaves them alone, even next to a normal
    // pair that DOES merge.
    let r = compile("var x: int = 5\nout a = x + 1\nout b = x + 1\n@nofold out c = x + 1");
    no_errors(&r);
    assert_eq!(
        count_class(&r.module, ADD),
        2,
        "a and b merge to one; the @nofold c stays separate = 2 total"
    );
}

#[test]
fn identical_gates_in_distinct_chips_not_merged() {
    // Two chip INSTANCES partition their bodies into distinct `chip_id` modules;
    // merging across them would land a keeper in the wrong grid, so the `v + 1`
    // in each instance is kept.
    let r = compile(
        "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nvar x: int = 5\nlet p = Inc(x)\nlet q = Inc(x)\nout oa = p\nout ob = q",
    );
    no_errors(&r);
    assert_eq!(
        count_class(&r.module, ADD),
        2,
        "each chip instance keeps its own body"
    );
}
