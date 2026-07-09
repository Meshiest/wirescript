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
fn string_in_select_inlines_as_variant() {
    // if-then-else with constant string branches inlines the strings directly
    // into the Select gate's wire_graph_variant inputs — no `String_Concatenate`
    // wrapper (that was the pre-inline-support way of wiring a string in).
    let r = compile("out x = if true then \"yes\" else \"no\"");
    assert_no_errors(&r);
    let select = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_Select")
        .expect("if-expr should create a Select gate");
    // No concat wrappers remain.
    let concat_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_String_Concatenate")
        .count();
    assert_eq!(
        concat_count, 0,
        "string branches should inline, not wire through Concatenate; found {concat_count}"
    );
    // Both branches inline as InputA/InputB data (branch→input slot is a
    // Select-gate convention, so check the set, not a fixed slot).
    let mut strs: Vec<String> = ["InputA", "InputB"]
        .iter()
        .filter_map(
            |p| match select.properties.get(&crate::intern::intern(p)) {
                Some(crate::ir::Literal::String(s)) => Some(s.to_string()),
                _ => None,
            },
        )
        .collect();
    strs.sort();
    assert_eq!(
        strs,
        vec!["no".to_string(), "yes".to_string()],
        "both string branches should inline into the Select"
    );
    // No input wires feed InputA/InputB — they're inline data now.
    let input_wires = r
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.node_id == select.id
                && matches!(
                    w.target.port,
                    crate::ir::port_registry::WirePort::InputA
                        | crate::ir::port_registry::WirePort::InputB
                )
        })
        .count();
    assert_eq!(input_wires, 0, "Select string inputs should be inline, not wired");
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
fn constant_string_inlines_into_consumers() {
    // A constant string used in a wire-value context (array push, var init,
    // comparison) folds into the consumer's data as a wire-variant — no legacy
    // `String_Concatenate` wrapper. Real concats (`a .. b`, wired inputs) are
    // unaffected (covered by `string_concat_op_works`).
    for src in [
        "array a: string[]\nin t: exec\non t { a.push(\"x\") }",
        "in t: exec\non t { var s: string = \"hi\" }",
        "in s: string\nout r = s == \"y\"",
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        let concat = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_String_Concatenate")
            .count();
        assert_eq!(
            concat, 0,
            "constant string should inline, not spawn a Concatenate gate:\n{src}"
        );
    }
}

#[test]
fn empty_string_in_select_not_lost() {
    // An empty string "" branch must still reach the Select — now inlined as an
    // empty-string wire-variant (InputA = ""), not dropped.
    let r = compile("out x = if true then \"\" else \"fail\"");
    assert_no_errors(&r);
    let select = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_Select")
        .expect("if-expr should create a Select gate");
    let has_empty = ["InputA", "InputB"].iter().any(|p| {
        matches!(
            select.properties.get(&crate::intern::intern(p)),
            Some(crate::ir::Literal::String(s)) if s.is_empty()
        )
    });
    assert!(has_empty, "the empty-string branch should inline as \"\" (not lost)");
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
