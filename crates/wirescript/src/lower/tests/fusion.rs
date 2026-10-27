use super::*;
use crate::ir::gate_class as gc;

/// A NOT over AND/OR fuses into the one NAND/NOR gate; `^^` is one `LogicalXOR`;
/// a lone NOT stays a NOT. Each row: the gate present exactly once, and the
/// unfused gates that must be absent.
#[test]
fn not_over_and_or_fuses_into_one_gate() {
    const LOGICAL_AND: &str = "BrickComponentType_Expr_LogicalAND";
    const LOGICAL_OR: &str = "BrickComponentType_Expr_LogicalOR";
    const BITWISE_AND: &str = "BrickComponentType_Expr_BitwiseAND";
    const BITWISE_OR: &str = "BrickComponentType_Expr_BitwiseOR";
    const BOOLS: &str = "var a: bool = false\nvar b: bool = false\n";
    const INTS: &str = "var a: int = 0\nvar b: int = 0\n";
    let rows: &[(&str, &str, &str, &[&str])] = &[
        (
            "!(a && b)",
            BOOLS,
            "BrickComponentType_Expr_LogicalNAND",
            &[LOGICAL_AND, gc::LOGICAL_NOT],
        ),
        (
            "!(a || b)",
            BOOLS,
            "BrickComponentType_Expr_LogicalNOR",
            &[LOGICAL_OR, gc::LOGICAL_NOT],
        ),
        (
            "~(a & b)",
            INTS,
            gc::BITWISE_NAND,
            &[BITWISE_AND, gc::BITWISE_NOT],
        ),
        (
            "~(a | b)",
            INTS,
            gc::BITWISE_NOR,
            &[BITWISE_OR, gc::BITWISE_NOT],
        ),
        ("a ^^ b", BOOLS, "BrickComponentType_Expr_LogicalXOR", &[]),
        ("!a", "var a: bool = false\n", gc::LOGICAL_NOT, &[]),
    ];
    let mut failures = Vec::new();
    for &(expr, decls, fused, absent) in rows {
        let src = format!("{decls}out r = {expr}");
        let r = compile(&src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let n = count_class(&r.module, fused);
        let present: Vec<_> = absent
            .iter()
            .filter(|c| count_class(&r.module, c) > 0)
            .collect();
        if !errors.is_empty() || n != 1 || !present.is_empty() {
            failures.push(format!("{expr}: errors {errors:?}, {n} x {fused} (want 1), unfused gates present {present:?}\n{src}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn fuse_nand_self_referential_buffer() {
    let r = compile("var e: bool = true\nbuffer t: bool = !(t && e)\nout r = t");
    assert!(
        has_gate(&r, "BrickComponentType_Expr_LogicalNAND"),
        "buffer t = !(t && e) should fuse to LogicalNAND; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    assert!(
        !has_gate_deep(&r, "BrickComponentType_Expr_LogicalAND"),
        "should not have separate AND"
    );
}

#[test]
fn fuse_nand_buffer_in_chip_with_param() {
    let r = compile("chip g(e: bool) {\n  buffer t: bool = !(t && e)\n}\nvar e: bool = true\ng(e)");
    let child = only_chip(&r, "chip instance should produce a child module");
    let child_gates: Vec<_> = child.nodes.values().map(|n| n.gate_class).collect();
    let buffer = child
        .nodes
        .values()
        .find(|n| n.gate_class.contains("Buffer"))
        .unwrap_or_else(|| panic!("child should contain the buffer gate; gates: {child_gates:?}"));
    assert!(
        child
            .nodes
            .values()
            .any(|n| n.gate_class == "BrickComponentType_Expr_LogicalNAND"),
        "buffer initializer inside chip should fuse to LogicalNAND; gates: {child_gates:?}"
    );
    assert!(
        child.wires.iter().any(|w| w.target.node_id == buffer.id),
        "buffer's Input must be wired from its initializer expression"
    );
}

#[test]
fn namespace_import_buffer_initializer_is_wired() {
    use crate::resolve::MemLoader;
    let lib_src = "var e: bool = true\nbuffer t: bool = !(t && e)";
    let main_src = "import * as lib from \"lib\"\nout r = 1";
    let mut files = crate::collections::HashMap::default();
    files.insert("lib.ws".to_string(), lib_src.into());
    let loader = MemLoader { files };
    let crate::compile::FrontEnd {
        resolved,
        lowered: r,
        ..
    } = crate::compile::front_end(main_src, "test", None, FoldMode::Auto, &loader);
    assert!(
        resolved.diagnostics.is_empty(),
        "import should resolve: {:?}",
        resolved.diagnostics
    );
    let gates: Vec<_> = r.module.nodes.values().map(|n| n.gate_class).collect();
    let buffer = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class.contains("Buffer"))
        .unwrap_or_else(|| panic!("ns buffer gate should exist; gates: {gates:?}"));
    assert!(
        has_gate(&r, "BrickComponentType_Expr_LogicalNAND"),
        "ns buffer initializer should lower (and fuse to NAND); gates: {gates:?}"
    );
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == buffer.id),
        "ns buffer's Input must be wired from its initializer"
    );
}
