use super::*;

#[test]
fn fuse_logical_nand() {
    let r = compile("var a: bool = false\nvar b: bool = false\nout r = !(a && b)");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNAND"),
        "!(&&) should fuse to LogicalNAND"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalAND"),
        "should not have separate AND"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNOT"),
        "should not have separate NOT"
    );
}

#[test]
fn fuse_logical_nor() {
    let r = compile("var a: bool = false\nvar b: bool = false\nout r = !(a || b)");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNOR"),
        "!(||) should fuse to LogicalNOR"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalOR"),
        "should not have separate OR"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNOT"),
        "should not have separate NOT"
    );
}

#[test]
fn fuse_bitwise_nand() {
    let r = compile("var a: int = 0\nvar b: int = 0\nout r = ~(a & b)");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseNAND"),
        "~(&) should fuse to BitwiseNAND"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseAND"),
        "should not have separate AND"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseNOT"),
        "should not have separate NOT"
    );
}

#[test]
fn fuse_bitwise_nor() {
    let r = compile("var a: int = 0\nvar b: int = 0\nout r = ~(a | b)");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseNOR"),
        "~(|) should fuse to BitwiseNOR"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseOR"),
        "should not have separate OR"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseNOT"),
        "should not have separate NOT"
    );
}

#[test]
fn logical_xor_operator() {
    let r = compile("var a: bool = false\nvar b: bool = false\nout r = a ^^ b");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalXOR"),
        "^^ should produce LogicalXOR"
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_LogicalXOR"),
        1
    );
}

#[test]
fn fuse_nand_self_referential_buffer() {
    let r = compile("var e: bool = true\nbuffer t: bool = !(t && e)\nout r = t");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNAND"),
        "buffer t = !(t && e) should fuse to LogicalNAND; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalAND"),
        "should not have separate AND"
    );
}

#[test]
fn fuse_nand_buffer_in_chip_with_param() {
    let r = compile(
        "chip g(e: bool) {\n  buffer t: bool = !(t && e)\n}\nvar e: bool = true\ng(e)",
    );
    let child = r
        .module
        .chips
        .values()
        .next()
        .expect("chip instance should produce a child module");
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
            .any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_LogicalNAND"),
        "buffer initializer inside chip should fuse to LogicalNAND; gates: {child_gates:?}"
    );
    assert!(
        child
            .wires
            .iter()
            .any(|w| w.target.node_id == buffer.id),
        "buffer's Input must be wired from its initializer expression"
    );
}

#[test]
fn namespace_import_buffer_initializer_is_wired() {
    use crate::resolve::{MemLoader, resolve};
    let lib_src = "var e: bool = true\nbuffer t: bool = !(t && e)";
    let main_src = "import * as lib from \"lib\"\nout r = 1";
    let mut files = std::collections::HashMap::new();
    files.insert("lib.ws".to_string(), lib_src.into());
    let loader = MemLoader { files };
    let resolved = resolve(main_src, "test", &loader);
    assert!(
        resolved.diagnostics.is_empty(),
        "import should resolve: {:?}",
        resolved.diagnostics
    );
    let tc = crate::typecheck::typecheck(&resolved.ast, "test");
    let r = crate::lower::lower(crate::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
    });
    let gates: Vec<_> = r.module.nodes.values().map(|n| n.gate_class).collect();
    let buffer = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class.contains("Buffer"))
        .unwrap_or_else(|| panic!("ns buffer gate should exist; gates: {gates:?}"));
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNAND"),
        "ns buffer initializer should lower (and fuse to NAND); gates: {gates:?}"
    );
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == buffer.id),
        "ns buffer's Input must be wired from its initializer"
    );
}

#[test]
fn unfused_not_still_works() {
    let r = compile("var a: bool = false\nout r = !a");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNOT"),
        "plain !a should still produce NOT"
    );
}
