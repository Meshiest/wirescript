use super::*;
use crate::ir::gate_class as gc;

#[test]
fn string_literal_emits_concatenate_gate() {
    let r = compile_ok("out x = \"hello\"");
    let has_concat = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == gc::STRING_CONCATENATE);
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

/// Constant string branches of a pure `if` inline straight into the Select
/// gate's wire-variant inputs: no `String_Concatenate` wrapper, no wired
/// input, and an empty-string branch inlined as `""`, not lost. Branch to
/// input slot is a Select-gate convention, so the set is checked, not a slot.
#[test]
fn string_branches_inline_into_the_select() {
    let rows: &[(&str, &[&str])] = &[
        ("out x = if true then \"yes\" else \"no\"", &["no", "yes"]),
        ("out x = if true then \"\" else \"fail\"", &["", "fail"]),
    ];
    let mut failures = Vec::new();
    for &(src, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let Some(select) = r.module.nodes.values().find(|n| n.gate_class == gc::SELECT) else {
            failures.push(format!("{src}: no Select gate, errors {errors:?}"));
            continue;
        };
        let mut strs: Vec<&str> = ["InputA", "InputB"]
            .iter()
            .filter_map(|p| match select.properties.get(&crate::intern::intern(p)) {
                Some(Literal::String(s)) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        strs.sort();
        let concat = count_class(&r.module, gc::STRING_CONCATENATE);
        let wired = r
            .module
            .wires
            .iter()
            .filter(|w| {
                w.target.node_id == select.id
                    && matches!(w.target.port, WirePort::InputA | WirePort::InputB)
            })
            .count();
        if !errors.is_empty() || strs != want || concat != 0 || wired != 0 {
            failures.push(format!(
                "{src}: errors {errors:?}, inlined {strs:?} (want {want:?}), {concat} Concatenate, {wired} wired inputs"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn string_concat_op_works() {
    let r = compile_ok("let a = \"hello\" .. \" \" .. \"world\"\nout x = a");
    let concat_count = count_class(&r.module, gc::STRING_CONCATENATE);
    assert_eq!(
        concat_count, 2,
        "one Concatenate per `..`, the constants baked, found {concat_count}"
    );
}
#[test]
fn constant_string_inlines_into_consumers() {
    // A constant string used in a wire-value context (array push, var init,
    // comparison) folds into the consumer's data as a wire-variant — no
    // `String_Concatenate` wrapper. Real concats (`a .. b`, wired inputs) are
    // unaffected (covered by `string_concat_op_works`).
    for src in [
        "var a: string[]\nin t: exec\non t { a.push(\"x\") }",
        "in t: exec\non t { var s: string = \"hi\" }",
        "in s: string\nout r = s == \"y\"",
    ] {
        let r = compile_ok(src);
        let concat = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::STRING_CONCATENATE)
            .count();
        assert_eq!(
            concat, 0,
            "constant string should inline, not spawn a Concatenate gate:\n{src}"
        );
    }
}

#[test]
fn string_equality_lowers_to_native_compare() {
    // String == / != now lower directly to the native Compare gates (which
    // accept the `Str` `WireValue` alternative), not the old
    // contains(a,b) && length(a)==length(b) workaround.
    let r = compile_ok("let a = \"x\"\nlet b = \"y\"\nout eq = a == b\nout ne = a != b");
    assert!(
        has_gate(&r, gc::COMPARE_EQUAL),
        "string == should lower to CompareEqual"
    );
    assert!(
        has_gate(&r, gc::COMPARE_NOT_EQUAL),
        "string != should lower to CompareNotEqual"
    );
    // The old workaround is gone: no String_Length / String_Contains gates,
    // and no LogicalAND/NAND stitched in for the comparison.
    assert_eq!(
        count_class(&r.module, gc::STRING_LENGTH),
        0,
        "string compare should not synthesize String_Length gates"
    );
    assert!(
        !has_gate_deep(&r, gc::STRING_CONTAINS),
        "string compare should not synthesize String_Contains gates"
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
    let r = compile_ok(src);

    const FT: &str = gc::STRING_FORMAT_TEXT;
    let ft_ids: crate::collections::HashSet<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == FT)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        ft_ids.len(),
        3,
        "9 interpolations split into 3 FormatText gates"
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
    assert_eq!(
        subst, 9,
        "all 9 interpolated values must be wired, got {subst}"
    );
}

#[test]
fn constant_string_operand_bakes_into_each_concat() {
    // Three copies of the constant `"PREFIX: "`, each feeding its own `..`.
    // A concat's `InputA` is a native `str` field read from data when unwired,
    // so every copy bakes into its consumer and no carrier gate survives: three
    // gates for three lines, not three plus a shared constant.
    let src = "in pl: character
               in ctrl: controller
               in go: exec
               on go {
                 ctrl.DisplayText(\"PREFIX: \" .. pl.GetDisplayName())
                 ctrl.DisplayText(\"PREFIX: \" .. pl.GetDisplayName())
                 ctrl.DisplayText(\"PREFIX: \" .. pl.GetDisplayName())
               }";
    let r = compile_ok(src);
    let concat = gc::STRING_CONCATENATE;
    assert_eq!(
        gate_count(&r, concat),
        3,
        "one concat per line, with the constant baked rather than carried"
    );
    let baked = r
        .module
        .nodes
        .values()
        .filter(|n| {
            n.gate_class == concat
                && n.properties.get(&*crate::intern::sym::INPUT_A)
                    == Some(&crate::ir::Literal::String("PREFIX: ".into()))
        })
        .count();
    assert_eq!(
        baked, 3,
        "each concat carries the constant in its own InputA"
    );
    // No carrier left behind: every remaining concat has a wired input.
    let targets: std::collections::HashSet<crate::ir::NodeId> =
        r.module.wires.iter().map(|w| w.target.node_id).collect();
    assert!(
        r.module
            .nodes
            .iter()
            .filter(|(_, n)| n.gate_class == concat)
            .all(|(id, _)| targets.contains(id)),
        "a constant-only concat wrapper must not survive"
    );
}

/// What feeds a coercion compare's `InputA`.
enum CoercedFrom {
    /// The `in` port of this name.
    Port(&'static str),
    /// A constant inlined onto the compare.
    Baked(&'static str),
}

/// A string reaching a bool destination routes through exactly one inserted
/// `CompareNotEqual(s, "")` (`s != ""`, not the game's port truthiness where
/// "0"/"false" are falsy). Each row: what feeds the compare's InputA, and the
/// gate (class, optional port label) and port its bOutput must feed.
#[test]
fn a_string_into_a_bool_destination_inserts_a_not_empty_compare() {
    use CoercedFrom::*;
    let rows: &[(
        &str,
        &str,
        CoercedFrom,
        (&str, Option<&str>),
        Option<WirePort>,
    )] = &[
        // s (MicrochipInput) -> NE.InputA; NE.InputB baked ""; NE.bOutput -> Var_Set.Value
        (
            "a string assigned into a bool var",
            "in s: string\nin t: exec\nvar v: bool = false\non t { v = s }",
            Port("s"),
            (gc::VAR_SET, None),
            Some(WirePort::Value),
        ),
        // A real Branch whose BCond is the compare, not the raw string wire.
        (
            "a string `if` condition",
            "in s: string\nin t: exec\nvar a: int = 0\non t { if s { a = 1 } }",
            Port("s"),
            (gc::BRANCH, None),
            Some(WirePort::BCond),
        ),
        // ArraySetAtIndex's `Value` port is typed with the ELEMENT type (it was
        // `Type::Any`, hiding the coercion). Array-set values always WIRE, so the
        // literal form is a compare carrying the inlined "0": "0" != "" is true.
        (
            "a string literal into a bool array slot",
            "in t: exec\nvar a: bool[]\non t { a.push(false)\na[0] = \"0\" }",
            Baked("0"),
            (gc::ARRAY_SET_AT_INDEX, None),
            Some(WirePort::Value),
        ),
        (
            "a string var into a bool array slot",
            "in s: string\nin t: exec\nvar a: bool[]\non t { a.push(false)\na[0] = s }",
            Port("s"),
            (gc::ARRAY_SET_AT_INDEX, None),
            Some(WirePort::Value),
        ),
        // `lower_buffer_body` once wired its initializer with a direct
        // `builder.connect`, bypassing the coercion choke point.
        (
            "a string initializer on a bool buffer",
            "in s: string\nbuffer buf: bool = s",
            Port("s"),
            (gc::BUFFER_TICKS, None),
            Some(WirePort::Input),
        ),
        // The out pin's port type comes from the ANNOTATION (bool), not the value.
        (
            "a string into an annotated bool out",
            "in s: string\nout y: bool = s",
            Port("s"),
            (gc::MICROCHIP_OUTPUT, None),
            None,
        ),
        // The chip's `MicrochipInput` pin lives in a NESTED module, so the
        // interception resolves its bool type through `module.chips`. The compare
        // belongs to the CALLER's module, feeding the boundary pin cross-module.
        (
            "a string argument into a bool chip param",
            "in s: string\nchip Gate(v: bool) -> (r: int) {\n  out r = if v then 1 else 0\n}\nout y = Gate(s)",
            Port("s"),
            (gc::MICROCHIP_INPUT, Some("v")),
            None,
        ),
    ];
    let label_is = |n: &crate::ir::Node, name: &str| {
        n.properties.get(&*crate::intern::sym::PORT_LABEL) == Some(&Literal::String(name.into()))
    };
    let mut failures = Vec::new();
    for (label, src, from, (dest_class, dest_label), dest_port) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let ne: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::COMPARE_NOT_EQUAL)
            .collect();
        if !errors.is_empty() || ne.len() != 1 {
            failures.push(format!(
                "{label}: errors {errors:?}, {} CompareNotEqual in the caller, want 1\n{src}",
                ne.len()
            ));
            continue;
        }
        let ne = ne[0];
        if ne.properties.get(&*crate::intern::sym::INPUT_B) != Some(&Literal::String(String::new()))
        {
            failures.push(format!(
                "{label}: InputB is not baked to \"\": {:?}",
                ne.properties
            ));
        }
        let fed = match from {
            Port(name) => r
                .module
                .nodes
                .values()
                .filter(|n| n.gate_class == gc::MICROCHIP_INPUT && label_is(n, name))
                .any(|s| {
                    r.module.wires.iter().any(|w| {
                        w.source.node_id == s.id
                            && w.target.node_id == ne.id
                            && w.target.port == WirePort::InputA
                    })
                }),
            Baked(v) => {
                ne.properties.get(&*crate::intern::sym::INPUT_A)
                    == Some(&Literal::String((*v).into()))
            }
        };
        if !fed {
            failures.push(format!(
                "{label}: the string does not reach the compare's InputA\n{src}"
            ));
        }
        let dests: Vec<_> = r
            .module
            .nodes_deep()
            .filter(|n| n.gate_class == *dest_class && dest_label.is_none_or(|l| label_is(n, l)))
            .map(|n| n.id)
            .collect();
        let drives = r.module.wires.iter().any(|w| {
            w.source.node_id == ne.id
                && w.source.port == WirePort::BOutput
                && dests.contains(&w.target.node_id)
                && dest_port.is_none_or(|p| w.target.port == p)
        });
        if !drives {
            failures.push(format!("{label}: the compare's bOutput does not feed {dest_class} {dest_port:?}, not the raw string\n{src}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

// ---- compile-time string -> bool on literal-BAKE paths ----
//
// Bare String literals never reach the wire choke point — they bake straight
// into component data (var/array InitialValue, call-arg data fields). Each
// bake site applies the same `!= ""` law as the runtime CompareNotEqual gate
// (see `bake_string_bool` in lower/expr.rs): a raw String left on a Bool
// destination either miscompiles (native content-aware truthiness reads "0"
// as false at load, diverging from the documented law) or crashes emit
// (UnimplementedCast("bool", "String") on a Bool data field).

/// A string literal on a bool data field bakes `Bool(!s.is_empty())`, never
/// a raw String: "0" is non-empty, so true (native truthiness would call it
/// false, the divergence this pins).
#[test]
fn a_string_literal_on_a_bool_data_field_bakes_a_bool() {
    let rows: &[(&str, &str, &str, &str, Literal)] = &[
        (
            "a string init on a bool var",
            "var v: bool = \"0\"",
            gc::VAR,
            "InitialValue",
            Literal::Bool(true),
        ),
        (
            "bool[] string inits convert element-wise",
            "var a: bool[] = [\"x\", \"\"]",
            gc::ARRAY_VAR,
            "InitialValue",
            Literal::Array(vec![Literal::Bool(true), Literal::Bool(false)]),
        ),
        // A bare-literal call argument bakes onto the gate's data field (never a
        // wire) in literal_for_property_port; Bool(true) on bSelectB selects B.
        (
            "Select's string cond",
            "out y = Select(\"0\", 1, 2)",
            gc::SELECT,
            "bSelectB",
            Literal::Bool(true),
        ),
        (
            "SetFrozen's string arg",
            "in e: entity\nin t: exec\non t { e.SetFrozen(\"yes\") }",
            gc::ENTITY_SET_FROZEN,
            "bFrozen",
            Literal::Bool(true),
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, class, field, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let got = r
            .module
            .nodes
            .values()
            .find(|n| n.gate_class == *class)
            .and_then(|n| n.properties.get(&crate::intern::intern(field)));
        if !errors.is_empty() || got != Some(want) {
            failures.push(format!(
                "{label}: errors {errors:?}, {class}.{field} = {got:?}, want {want:?}\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn string_index_lowers_to_a_substring_gate() {
    // `s[i]` is one Unicode character starting at `i`, which the game's
    // Substring gate produces directly. It is a pure `Expr_` gate, so unlike an
    // array read this needs no exec context.
    let r = compile_ok("in s: string\nin i: int\nout c: string = s[i]");
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "must not lower to a placeholder: {:?}",
        r.diagnostics
    );
    let sub = gc::STRING_SUBSTRING;
    assert_eq!(gate_count(&r, sub), 1, "one Substring gate");
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == sub)
        .expect("substring gate");
    assert_eq!(
        node.properties.get(&crate::intern::intern("Length")),
        Some(&crate::ir::Literal::Int(1)),
        "a single-character read bakes Length = 1: {:?}",
        node.properties
    );
}

#[test]
fn constant_string_index_folds_to_a_literal() {
    // Both operands constant, so the character is known at compile time and no
    // Substring gate is emitted. Indexing counts Unicode code points, matching
    // the game's own string length semantics.
    let r = compile_ok("in go: bool\nout c: string = \"foo\"[2]");
    assert_eq!(
        count_class(&r.module, gc::STRING_SUBSTRING),
        0,
        "a constant index emits no gate"
    );
    assert!(
        r.module.nodes.values().any(|n| n
            .properties
            .values()
            .any(|v| *v == crate::ir::Literal::String("o".into()))),
        "folds to the third code point: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.properties.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn string_index_counts_code_points_and_declines_out_of_range() {
    // Byte indexing would split the two-byte character; code-point indexing
    // yields it whole.
    let r = compile_ok("in go: bool\nout c: string = \"n\u{e9}o\"[1]");
    assert!(
        r.module.nodes.values().any(|n| n
            .properties
            .values()
            .any(|v| *v == crate::ir::Literal::String("\u{e9}".into()))),
        "index 1 is the whole second code point: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.properties.clone())
            .collect::<Vec<_>>()
    );
    // Past the end, the gate decides rather than the compiler guessing.
    let oob = compile_ok("in go: bool\nout c: string = \"foo\"[9]");
    assert_eq!(
        gate_count(&oob, gc::STRING_SUBSTRING),
        1,
        "an out-of-range constant index keeps the gate"
    );
}
