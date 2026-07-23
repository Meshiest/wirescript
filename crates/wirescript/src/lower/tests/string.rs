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

#[test]
fn identical_constant_strings_dedup_within_chip() {
    // Three copies of the constant `"PREFIX: "` — each a String_Concatenate
    // wrapper feeding a `..` — collapse to one gate that fans out to all three
    // `.. name` concats, instead of one wrapper per line.
    let src = "in pl: character\n\
               in ctrl: controller\n\
               in go: exec\n\
               on go {\n\
                 ctrl.DisplayText(\"PREFIX: \" .. pl.GetDisplayName())\n\
                 ctrl.DisplayText(\"PREFIX: \" .. pl.GetDisplayName())\n\
                 ctrl.DisplayText(\"PREFIX: \" .. pl.GetDisplayName())\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    let concat = "BrickComponentType_WireGraph_Expr_String_Concatenate";
    // 1 shared constant wrapper + 3 per-line `.. name` concats (was 6 before).
    assert_eq!(
        gate_count(&r, concat),
        4,
        "identical constant strings should share one gate"
    );
    // The constant wrapper (no incoming wire) drives all three consumers.
    let targets: std::collections::HashSet<crate::ir::NodeId> =
        r.module.wires.iter().map(|w| w.target.node_id).collect();
    let shared = r
        .module
        .nodes
        .iter()
        .find(|(id, n)| n.gate_class == concat && !targets.contains(id))
        .map(|(id, _)| *id)
        .expect("one constant concat wrapper with no wired input");
    let fanout = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == shared)
        .count();
    assert_eq!(fanout, 3, "shared constant should fan out to 3 consumers");
}

#[test]
fn string_into_bool_port_inserts_not_empty_compare() {
    // A string value assigned into a declared-`bool` var must route through
    // an inserted `CompareNotEqual(s, "")` gate — the language-level
    // string → bool coercion means exactly `s != ""` (empty is false,
    // everything else true), NOT the game's native content-aware port
    // truthiness (where "0"/"false" are also falsy). The wire shape is:
    //   s (MicrochipInput) → NE.InputA;  NE.InputB baked "";  NE.bOutput → Var_Set.Value
    let r = compile("in s: string\nin t: exec\nvar v: bool = false\non t { v = s }");
    assert_no_errors(&r);

    let ne_class = crate::ir::gate_class::COMPARE_NOT_EQUAL;
    let ne_nodes: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == ne_class)
        .collect();
    assert_eq!(ne_nodes.len(), 1, "exactly one coercion CompareNotEqual");
    let ne = ne_nodes[0];
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&Literal::String(String::new())),
        "the compare's InputB must be baked to the empty string"
    );

    let s_input = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == crate::ir::gate_class::MICROCHIP_INPUT
                && n.properties.get(&*crate::intern::sym::PORT_LABEL)
                    == Some(&Literal::String("s".into()))
        })
        .expect("`in s: string` boundary node");
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == s_input.id
                && w.target.node_id == ne.id
                && w.target.port == crate::ir::port_registry::WirePort::InputA
        }),
        "the string source must feed the compare's InputA"
    );

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::VAR_SET)
        .expect("`v = s` lowers to a Var_Set gate");
    let value_wire = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == var_set.id
                && w.target.port == crate::ir::port_registry::WirePort::Value
        })
        .expect("Var_Set's Value port must have an incoming wire");
    assert_eq!(
        value_wire.source.node_id, ne.id,
        "Var_Set.Value must be fed by the compare's output, not the raw string"
    );
    assert_eq!(
        value_wire.source.port,
        crate::ir::port_registry::WirePort::BOutput,
        "the compare feeds the bool destination from bOutput"
    );
}

#[test]
fn string_if_condition_inserts_not_empty_compare() {
    // Non-constant path: `if s { ... }` on a string input lowers a real
    // Branch whose BCond is fed by the inserted `s != ""` compare, not by
    // the raw string wire.
    let r = compile("in s: string\nin t: exec\nvar a: int = 0\non t { if s { a = 1 } }");
    assert_no_errors(&r);

    let ne = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::COMPARE_NOT_EQUAL)
        .expect("the string condition must insert a CompareNotEqual");
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&Literal::String(String::new())),
    );

    let branch = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::BRANCH)
        .expect("`if s` lowers a Branch gate");
    let bcond_wire = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == branch.id
                && w.target.port == crate::ir::port_registry::WirePort::BCond
        })
        .expect("Branch.BCond must be wired");
    assert_eq!(
        bcond_wire.source.node_id, ne.id,
        "BCond must be fed by the coercion compare, not the raw string"
    );
}

// ---- compile-time string → bool on literal-BAKE paths ----
//
// Bare String literals never reach the wire choke point — they bake straight
// into component data (var/array InitialValue, call-arg data fields). Each
// bake site applies the same `!= ""` law as the runtime CompareNotEqual gate
// (see `bake_string_bool` in lower/expr.rs): a raw String left on a Bool
// destination either miscompiles (native content-aware truthiness reads "0"
// as false at load, diverging from the documented law) or crashes emit
// (UnimplementedCast("bool", "String") on a Bool data field).

#[test]
fn var_bool_string_literal_init_bakes_bool() {
    // "0" is non-empty → true under the `!= ""` law (native truthiness
    // would call it false — the exact divergence this pin guards).
    let r = compile("var v: bool = \"0\"");
    assert_no_errors(&r);
    let var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::PSEUDO_VAR)
        .expect("var gate");
    assert_eq!(
        var.properties.get(&*crate::intern::sym::INITIAL_VALUE),
        Some(&Literal::Bool(true)),
        "string literal init on a bool var must bake Bool(!s.is_empty()), not a raw String"
    );
}

#[test]
fn array_bool_string_literal_inits_bake_elementwise() {
    let r = compile("array a: bool[] = [\"x\", \"\"]");
    assert_no_errors(&r);
    let arr = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::PSEUDO_ARRAY_VAR)
        .expect("array var gate");
    assert_eq!(
        arr.properties.get(&crate::intern::intern_static("InitialValue")),
        Some(&Literal::Array(vec![Literal::Bool(true), Literal::Bool(false)])),
        "bool[] string inits must convert element-wise: \"x\" → true, \"\" → false"
    );
}

#[test]
fn select_string_literal_cond_bakes_bool() {
    // A bare-literal call argument bakes onto the gate's data field (never
    // a wire), so the conversion must happen in literal_for_property_port.
    // Bool(true) baked on bSelectB IS the truthy-side selection: "0" is
    // non-empty → true under the `!= ""` law.
    let r = compile("out y = Select(\"0\", 1, 2)");
    assert_no_errors(&r);
    let select = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::SELECT)
        .expect("Select gate");
    assert_eq!(
        select
            .properties
            .get(&crate::intern::intern_static("bSelectB")),
        Some(&Literal::Bool(true)),
        "Select's string cond must bake as Bool(true), not a raw String on a Bool field"
    );
}

#[test]
fn set_frozen_string_literal_arg_bakes_bool() {
    let r = compile("in e: entity\nin t: exec\non t { e.SetFrozen(\"yes\") }");
    assert_no_errors(&r);
    let gate = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::ENTITY_SET_FROZEN)
        .expect("SetFrozen gate");
    assert_eq!(
        gate.properties.get(&crate::intern::intern_static("bFrozen")),
        Some(&Literal::Bool(true)),
        "SetFrozen's string arg must bake as Bool(true)"
    );
}

#[test]
fn string_literal_into_bool_array_slot_inserts_compare() {
    // `a[i] = "0"` on a `bool[]`: ArraySetAtIndex's `Value` port was typed
    // `Type::Any`, hiding the coercion from the connect choke point — a
    // raw String written into a bool slot. The port is now typed with the
    // ELEMENT type, so the `!= ""` compare inserts. Array-set values always
    // WIRE (there is no property-bake path for `Value`), so the literal
    // form's exact shape is an NE gate whose InputA carries the inlined
    // constant "0" and whose InputB is the baked "" — evaluating
    // "0" != "" = true at runtime, per the law (native truthiness would
    // have read a raw "0" as false).
    let r = compile("in t: exec\narray a: bool[]\non t { a.push(false)\na[0] = \"0\" }");
    assert_no_errors(&r);

    let ne_nodes: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::COMPARE_NOT_EQUAL)
        .collect();
    assert_eq!(ne_nodes.len(), 1, "exactly one coercion CompareNotEqual");
    let ne = ne_nodes[0];
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&Literal::String("0".into())),
        "the constant \"0\" inlines onto the compare's InputA"
    );
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&Literal::String(String::new())),
    );

    let set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::ARRAY_SET_AT_INDEX)
        .expect("a[0] = ... lowers to ArrayVar_SetAtIndex");
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == ne.id
                && w.source.port == crate::ir::port_registry::WirePort::BOutput
                && w.target.node_id == set.id
                && w.target.port == crate::ir::port_registry::WirePort::Value
        }),
        "the compare's bOutput must feed the set gate's Value, not the raw string"
    );
}

#[test]
fn string_var_into_bool_array_slot_inserts_compare() {
    // Wired form of the same hole: `a[i] = s` with a string input.
    let r = compile(
        "in s: string\nin t: exec\narray a: bool[]\non t { a.push(false)\na[0] = s }",
    );
    assert_no_errors(&r);

    let ne_nodes: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::COMPARE_NOT_EQUAL)
        .collect();
    assert_eq!(ne_nodes.len(), 1, "exactly one coercion CompareNotEqual");
    let ne = ne_nodes[0];

    let s_input = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == crate::ir::gate_class::MICROCHIP_INPUT
                && n.properties.get(&*crate::intern::sym::PORT_LABEL)
                    == Some(&Literal::String("s".into()))
        })
        .expect("`in s: string` boundary node");
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == s_input.id
                && w.target.node_id == ne.id
                && w.target.port == crate::ir::port_registry::WirePort::InputA
        }),
        "the string source must feed the compare's InputA"
    );

    let set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::ARRAY_SET_AT_INDEX)
        .expect("a[0] = s lowers to ArrayVar_SetAtIndex");
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == ne.id
                && w.source.port == crate::ir::port_registry::WirePort::BOutput
                && w.target.node_id == set.id
                && w.target.port == crate::ir::port_registry::WirePort::Value
        }),
        "the compare's bOutput must feed the set gate's Value"
    );
}

#[test]
fn string_into_bool_buffer_inserts_compare() {
    // `lower_buffer_body` wired its initializer with a direct
    // `builder.connect`, bypassing the coercion choke point — a string
    // initializer on a bool buffer reached the Bool `Input` port raw. Now
    // routed through `ctx.connect`: exactly one `!= ""` compare between
    // the string source and the buffer.
    let r = compile("in s: string\nbuffer buf: bool = s");
    assert_no_errors(&r);

    let ne_nodes: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::COMPARE_NOT_EQUAL)
        .collect();
    assert_eq!(ne_nodes.len(), 1, "exactly one coercion CompareNotEqual");
    let ne = ne_nodes[0];
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&Literal::String(String::new())),
    );

    let buffer = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::BUFFER_TICKS)
        .expect("`buffer buf` lowers to a Buffer_Ticks gate");
    let input_wire = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == buffer.id
                && w.target.port == crate::ir::port_registry::WirePort::Input
        })
        .expect("the buffer's Input must be wired");
    assert_eq!(
        input_wire.source.node_id, ne.id,
        "the buffer's bool Input must be fed by the compare, not the raw string"
    );
}

#[test]
fn string_into_annotated_bool_out_inserts_compare() {
    // `out y: bool = s` — the out pin's port type must come from the
    // ANNOTATION (bool), not the value (string); deriving it from the
    // value made this a string pin and silently skipped the coercion.
    let r = compile("in s: string\nout y: bool = s");
    assert_no_errors(&r);

    let ne = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::COMPARE_NOT_EQUAL)
        .expect("the annotated bool out must insert a CompareNotEqual");
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&Literal::String(String::new())),
    );

    let out_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::MICROCHIP_OUTPUT)
        .expect("out y's boundary node");
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == ne.id
                && w.source.port == crate::ir::port_registry::WirePort::BOutput
                && w.target.node_id == out_node.id
        }),
        "the compare's bOutput must feed the out pin, not the raw string"
    );
}

#[test]
fn string_arg_into_bool_chip_param_inserts_compare_at_call_site() {
    // Chip boundary: the instance's `MicrochipInput` pin lives in a NESTED
    // chip module, so the coercion interception must resolve the pin's
    // bool type through `module.chips` — a plain top-level node lookup
    // misses it and silently wires the raw string in (the exact hole this
    // test pins). The compare belongs to the CALLER's module (coercion at
    // the call site), feeding the chip's boundary pin cross-module.
    let r = compile(
        "in s: string\nchip Gate(v: bool) -> (r: int) {\n  out r = if v then 1 else 0\n}\nout y = Gate(s)",
    );
    assert_no_errors(&r);

    let ne = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::COMPARE_NOT_EQUAL)
        .expect("coercion compare must live in the caller's module");
    assert_eq!(
        ne.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&Literal::String(String::new())),
    );

    let child = r.module.chips.values().next().expect("one Gate instance");
    let v_pin = child
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::MICROCHIP_INPUT)
        .expect("the chip has a v boundary pin");
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == ne.id
                && w.source.port == crate::ir::port_registry::WirePort::BOutput
                && w.target.node_id == v_pin.id
        }),
        "the compare's bOutput must feed the chip's bool param pin, not the raw string"
    );
}
