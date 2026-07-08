use super::*;

#[test]
fn string_literal_emits_concatenate_gate() {
    // A bare string literal should produce a Concatenate gate, not a _Literal
    let r = compile("out x = \"hello\"");
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let has_concat = r.module.nodes.values().any(|n| {
        n.gate_class
            == "BrickComponentType_WireGraph_Expr_String_Concatenate"
    });
    assert!(
        has_concat,
        "string literal should lower to String_Concatenate gate"
    );
    let has_str_literal = r.module.nodes.values().any(|n| {
        n.gate_class == "_Literal"
            && matches!(
                n.properties.get(&crate::intern::intern_static("Value")),
                Some(Literal::String(_))
            )
    });
    assert!(!has_str_literal, "no _Literal string nodes should remain");
}

#[test]
fn string_in_select_wires_through_concatenate() {
    // if-then-else with string branches must wire through Concatenate gates
    // into the Select gate's wire_graph_variant inputs, not inline them
    let r = compile("out x = if true then \"yes\" else \"no\"");
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let has_select = r.module.nodes.values().any(|n| {
        n.gate_class == "BrickComponentType_WireGraph_Expr_Select"
    });
    assert!(has_select, "if-expr should create Select gate");
    // Both branches should be Concatenate gates
    let concat_count = r
        .module
        .nodes
        .values()
        .filter(|n| {
            n.gate_class
                == "BrickComponentType_WireGraph_Expr_String_Concatenate"
        })
        .count();
    assert!(
        concat_count >= 2,
        "both string branches should be Concatenate gates, found {concat_count}"
    );
    // Select's InputA and InputB should be wired from Concatenate outputs, not inlined
    let select_id = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.gate_class == "BrickComponentType_WireGraph_Expr_Select"
        })
        .map(|(id, _)| id.clone())
        .unwrap();
    let wires_into_select: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.node_id == select_id
                && (w.target.port == crate::ir::port_registry::WirePort::InputA
                    || w.target.port == crate::ir::port_registry::WirePort::InputB)
        })
        .collect();
    assert_eq!(
        wires_into_select.len(),
        2,
        "Select should have 2 input wires (from Concatenate gates), found {}",
        wires_into_select.len()
    );
}

#[test]
fn string_concat_op_works() {
    // String concatenation with .. operator should produce Concatenate gates
    let r = compile("let a = \"hello\" .. \" \" .. \"world\"\nout x = a");
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let concat_count = r
        .module
        .nodes
        .values()
        .filter(|n| {
            n.gate_class
                == "BrickComponentType_WireGraph_Expr_String_Concatenate"
        })
        .count();
    // 3 string literals become 3 Concatenate gates, plus 2 concat ops = 5 total
    // (or the literal Concatenates may be reused by the concat ops)
    assert!(
        concat_count >= 2,
        "string concat should produce Concatenate gates, found {concat_count}"
    );
}

#[test]
fn empty_string_in_select_not_lost() {
    // Empty string "" in if-then-else should produce a Concatenate gate, not be lost
    let r = compile("out x = if true then \"\" else \"fail\"");
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let concat_count = r
        .module
        .nodes
        .values()
        .filter(|n| {
            n.gate_class
                == "BrickComponentType_WireGraph_Expr_String_Concatenate"
        })
        .count();
    assert!(
        concat_count >= 2,
        "both branches (including empty string) should be Concatenate gates, found {concat_count}"
    );
}

#[test]
fn string_equality_lowers_to_native_compare() {
    // String == / != now lower directly to the native Compare gates (which
    // accept the `str` WireGraphVariant member), not the old
    // contains(a,b) && length(a)==length(b) workaround.
    let r = compile("let a = \"x\"\nlet b = \"y\"\nout eq = a == b\nout ne = a != b");
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"),
        "string == should lower to CompareEqual"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareNotEqual"),
        "string != should lower to CompareNotEqual"
    );
    // The old workaround is gone: no String_Length / String_Contains gates,
    // and no LogicalAND/NAND stitched in for the comparison.
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_String_Length"),
        0,
        "string compare should not synthesize String_Length gates"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_String_Contains"),
        "string compare should not synthesize String_Contains gates"
    );
}

#[test]
fn string_var_stores_and_assigns() {
    // Strings can be stored in vars now (WireGraphVariant `str`).
    let r = compile("static var s: string = \"init\"\non RoundStart { s = \"hello\" }");
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraphPseudo_Var"),
        "string var should lower to a Var gate"
    );
}

#[test]
fn long_interpolation_splits_across_format_text_gates() {
    // A template with more than 7 `${...}` values must not silently drop the
    // extras — it splits into several FormatText gates (7 substitution inputs
    // each) whose outputs are concatenated. `in` ports make each substitution a
    // real wire (not an inlinable literal), so we can count them.
    let src = "in a: int\nin b: int\nin c: int\nin d: int\nin e: int\n\
               in f: int\nin g: int\nin h: int\nin i: int\n\
               out s = \"${a}${b}${c}${d}${e}${f}${g}${h}${i}\"";
    let r = compile(src);
    assert_no_errors(&r);

    const FT: &str = "BrickComponentType_WireGraph_Expr_String_FormatText";
    let ft_ids: std::collections::HashSet<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == FT)
        .map(|(id, _)| *id)
        .collect();
    assert!(
        ft_ids.len() >= 2,
        "9 interpolations should split into >=2 FormatText gates, got {}",
        ft_ids.len()
    );

    use crate::ir::port_registry::WirePort::*;
    // Substitution wires: into a FormatText input slot from a NON-FormatText
    // source (this excludes the concat wires between FormatText gates). All 9
    // interpolated values must be wired — none dropped.
    let subst = r
        .module
        .wires
        .iter()
        .filter(|w| {
            ft_ids.contains(&w.target.node_id)
                && matches!(
                    w.target.port,
                    InputA | InputB | InputC | InputD | InputE | InputF | InputG
                )
                && !ft_ids.contains(&w.source.node_id)
        })
        .count();
    assert_eq!(subst, 9, "all 9 interpolated values must be wired, got {subst}");
}

#[test]
fn numeric_literal_still_uses_literal_node() {
    // Non-string literals should still use the _Literal path (inlineable)
    let r = compile("out x = 42");
    let _has_literal = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "_Literal");
    // _Literal may be inlined away, but if present it should not be a Concatenate
    let has_str_concat = r.module.nodes.values().any(|n| {
        n.gate_class
            == "BrickComponentType_WireGraph_Expr_String_Concatenate"
    });
    assert!(
        !has_str_concat,
        "numeric literal should not produce a Concatenate gate"
    );
}
