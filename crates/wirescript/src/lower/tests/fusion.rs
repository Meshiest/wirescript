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
fn unfused_not_still_works() {
    let r = compile("var a: bool = false\nout r = !a");
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalNOT"),
        "plain !a should still produce NOT"
    );
}
