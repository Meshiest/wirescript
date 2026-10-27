use std::fmt::Write as _;

use super::*;
use crate::intern::{intern, resolve};
use crate::ir::{Literal, NodeId, NodeKind, Program, Type};

const AND: &str = r#"; wsir 1
source_filename = "and.ws"

module @and {
  %a = in bool
  %b = in bool
  %g = Expr_LogicalAND(bInputA: %a, bInputB: %b)
  out %y bool = %g
}
"#;

fn parse(src: &str) -> Program {
    parse_program(src, "test.wsir").unwrap_or_else(|e| panic!("{e}"))
}

fn parse_err(src: &str) -> ParseError {
    match parse_program(src, "test.wsir") {
        Ok(_) => panic!("expected a parse error"),
        Err(e) => e,
    }
}

fn node(p: &Program, id: u32) -> &crate::ir::Node {
    &p.module.nodes[&NodeId(id)]
}

/// Text -> program -> text -> program -> text: the second write must match
/// the first.
fn fixed_point(src: &str) -> String {
    let once = program_to_string(&parse(src)).unwrap();
    let twice = program_to_string(&parse(&once)).unwrap();
    assert_eq!(once, twice, "writing the parsed program changed the text");
    once
}

#[test]
fn no_short_class_name_collides_with_another_class() {
    let names = short_class_names();
    assert!(
        names.len() > 300,
        "expected the inventory's classes, got {}",
        names.len()
    );
    let catalog = crate::catalog::default_catalog();
    for (short, full) in &names {
        assert_eq!(full.strip_prefix(defaults::CLASS_PREFIX), Some(*short));
        assert!(
            catalog.find_by_class(short).is_none(),
            "`{short}` is also a full class name"
        );
        assert_eq!(defaults::resolve_class(short), *full);
        assert_eq!(
            defaults::resolve_class(full),
            *full,
            "the full name `{full}` must still be accepted"
        );
    }
    let mut shorts: Vec<&str> = names.iter().map(|(s, _)| *s).collect();
    shorts.dedup();
    assert_eq!(shorts.len(), names.len(), "two classes share a short name");
}

#[test]
fn the_and_example_parses_with_inventory_ports() {
    let p = parse(AND);
    let m = &p.module;
    assert_eq!(resolve(m.name), "and");
    assert_eq!(p.settings.source_file.as_deref(), Some("and.ws"));
    assert_eq!(m.nodes.len(), 4);
    // Ids follow definition order.
    let (a, b, g, y) = (node(&p, 1), node(&p, 2), node(&p, 3), node(&p, 4));
    assert_eq!(
        (a.kind, b.kind, g.kind, y.kind),
        (
            NodeKind::Input,
            NodeKind::Input,
            NodeKind::Gate,
            NodeKind::Output
        )
    );
    assert_eq!(g.gate_class, "BrickComponentType_Expr_LogicalAND");
    assert_eq!(
        a.properties.get(&intern("PortLabel")),
        Some(&Literal::String("a".into()))
    );
    assert_eq!(a.ports.outputs[0].ty, Type::Bool);
    let inputs: Vec<&str> = g.ports.inputs.iter().map(|p| resolve(p.name)).collect();
    assert_eq!(inputs, ["bInputA", "bInputB"]);
    let wires: Vec<String> = m
        .wires
        .iter()
        .map(|w| {
            format!(
                "{}.{}->{}.{}",
                w.source.node_id, w.source.port, w.target.node_id, w.target.port
            )
        })
        .collect();
    assert_eq!(
        wires,
        [
            "n1.RER_Output->n3.bInputA",
            "n2.RER_Output->n3.bInputB",
            "n3.bOutput->n4.RER_Input"
        ]
    );
    assert_eq!(m.inputs, [NodeId(1), NodeId(2)]);
    assert_eq!(m.outputs, [NodeId(4)]);
}

#[test]
fn the_and_example_writes_back_as_written() {
    // Hand-written text in the canonical shape, apart from the comment and
    // metadata the writer adds for a node order it cannot see.
    let out = fixed_point(AND);
    assert!(
        out.contains("%g = Expr_LogicalAND(bInputA: %a, bInputB: %b)"),
        "{out}"
    );
    assert!(out.contains("out %y bool = %g"), "{out}");
    assert!(
        !out.contains("!id"),
        "sequential ids need no metadata:\n{out}"
    );
}

#[test]
fn every_literal_spelling_round_trips() {
    let src = r#"module @lits {
  %v = Var {
    a = true, b = -3, c = 2.5, d = 1e30, e = -0.0, f = inf, g = -inf, h = nan,
    i = "q\"\\\n\t\u{1}", j = vec(1.0, -2.0, 3.5), k = rot(0.0, 90.0, 180.0),
    l = quat(0.0, 0.0, 0.0, 1.0), m = #ff8000ff, n = lrgba(1.0, 0.5, 0.0, 1.0),
    o = null, p = [1, 2, [3]], q = map[1 => "x", 2 => "y"], r = rec{x: 1, "y z": 2},
    s = asset("Sound", "Boom"), t = prefab("a/b.brz"), u = nested_prefab("in a: int"),
    w = prefab_bytes("AAEC/w==")
  }
}
"#;
    let p = parse(src);
    let props = &node(&p, 1).properties;
    let get = |k: &str| props.get(&intern(k)).unwrap_or_else(|| panic!("no {k}"));
    assert_eq!(get("b"), &Literal::Int(-3));
    assert!(matches!(get("e"), Literal::Float(f) if *f == 0.0 && f.is_sign_negative()));
    assert!(matches!(get("h"), Literal::Float(f) if f.is_nan()));
    assert_eq!(get("i"), &Literal::String("q\"\\\n\t\u{1}".into()));
    assert_eq!(
        get("m"),
        &Literal::Color {
            r: 255,
            g: 128,
            b: 0,
            a: 255
        }
    );
    assert_eq!(get("w"), &Literal::PrefabBytes(vec![0u8, 1, 2, 255].into()));
    let out = fixed_point(src);
    for spelled in [
        "1e30",
        "-0.0",
        "-inf",
        "nan",
        "#ff8000ff",
        "map[1 => \"x\"",
        "rec{x: 1, \"y z\": 2}",
        "\\u{1}",
    ] {
        assert!(out.contains(spelled), "missing {spelled} in\n{out}");
    }
}

#[test]
fn every_type_spelling_round_trips() {
    let types = [
        "bool",
        "int",
        "float",
        "string",
        "vector",
        "rotator",
        "quat",
        "color",
        "entity",
        "character",
        "controller",
        "brick",
        "zone",
        "teleport",
        "prefab",
        "exec",
        "any",
        "opaque",
        "never",
        "*int",
        "int[]",
        "*int[]",
        "array<*int>",
        "map<string, int[]>",
        "union<int, string>",
        "tuple<int, bool>",
        "{x: int, y: float}",
        "outputs{min: int, max: int}",
        "param<\"T\">",
        "enum<\"Option\", int>",
        "component<\"Component_PointLight\">",
    ];
    for t in types {
        let src = format!("module @t {{\n  %x = Thing : (A: {t}) -> ()\n}}\n");
        let p = parse(&src);
        let out = program_to_string(&p).unwrap();
        assert!(out.contains(&format!("(A: {t})")), "{t} wrote as\n{out}");
    }
    let p = parse("module @t {\n  %x = Thing : (A: *int[], B: array<*int>) -> ()\n}\n");
    let ins = &node(&p, 1).ports.inputs;
    assert_eq!(
        ins[0].ty,
        Type::Ref(Box::new(Type::Array(Box::new(Type::Int))))
    );
    assert_eq!(
        ins[1].ty,
        Type::Array(Box::new(Type::Ref(Box::new(Type::Int))))
    );
}

#[test]
fn ids_are_implicit_explicit_or_numeric() {
    // a = 1, b = 10 (explicit), c = 11, %20 = 20 (numeric), d = 21.
    let p = parse(
        "module @m {\n  %a = Thing\n  %b = Thing !id 10\n  %c = Thing\n  %20 = Thing\n  %d = Thing\n}\n",
    );
    let mut all: Vec<u32> = p.module.nodes.keys().map(|k| k.0).collect();
    all.sort_unstable();
    assert_eq!(all, [1, 10, 11, 20, 21]);
    let e = parse_err("module @m {\n  %a = Thing !id 5\n  %5 = Thing\n}\n");
    assert!(e.message.contains("already taken"), "{e}");
}

#[test]
fn a_chip_body_and_its_pins() {
    let src = r"module @chip {
  %go = in exec
  %n = Var : () -> (Value: int, VarRef: *int)
  %c = chip { closed } !layout_after(%n) {
    %p = in exec(%go)
    %r = in *int(%n.VarRef)
    %inc = Exec_Var_Increment(Exec: %p, VarRef: %r)
  }
}
";
    let p = parse(src);
    let m = &p.module;
    let chip = &m.chips[&NodeId(3)];
    assert_eq!(resolve(chip.name), "_anon_n3");
    assert_eq!(
        m.nodes[&NodeId(3)].properties.get(&intern("_closed")),
        Some(&Literal::Bool(true))
    );
    // Parent -> pin wires live in the parent; the pin -> gate wires in the chip.
    assert_eq!(m.wires.len(), 3, "{:?}", m.wires);
    assert_eq!(chip.wires.len(), 2);
    assert_eq!(chip.inputs, [NodeId(4), NodeId(5)]);
    fixed_point(src);
}

#[test]
fn errors_point_at_the_problem() {
    let e = parse_err("module @m {\n  %a = in bool\n  %g = Expr_LogicalAND(bNope: %a)\n}\n");
    assert_eq!((e.line, e.col), (3, 24));
    assert!(
        e.message.contains("bInputA") && e.message.contains("bInputB"),
        "{e}"
    );

    let e = parse_err(
        "module @m {\n  %v = Var : () -> (Value: int, VarRef: *int)\n  out %y int = %v\n}\n",
    );
    assert!(
        e.message.contains("2 output ports") && e.message.contains("Value, VarRef"),
        "{e}"
    );

    let e = parse_err("module @m {\n  out %y int = %ghost\n}\n");
    assert!(
        e.message.contains("never defined") && e.found.contains("%ghost"),
        "{e}"
    );

    let e = parse_err("module @m {\n  %a = in bool\n  %a = in bool\n}\n");
    assert!(e.message.contains("already defined"), "{e}");

    let e = parse_err("module @m {\n  %a = in bool loc(\"x\":1)\n}\n");
    assert_eq!(e.line, 2);
    assert!(e.message.contains("expected `:`"), "{e}");
    assert!(e.to_string().starts_with("test.wsir:2:"), "{e}");
}

#[test]
fn a_negative_nan_keeps_its_sign() {
    let p = parse("module @m {\n  %v = Var { a = -nan, b = nan }\n}\n");
    let props = &node(&p, 1).properties;
    assert!(matches!(props[&intern("a")], Literal::Float(f) if f.is_nan() && f.is_sign_negative()));
    assert!(matches!(props[&intern("b")], Literal::Float(f) if f.is_nan() && f.is_sign_positive()));
    let out = fixed_point("module @m {\n  %v = Var { a = -nan, b = nan }\n}\n");
    assert!(out.contains("a = -nan") && out.contains("b = nan"), "{out}");
    // A NaN is the same IR as itself.
    assert_eq!(p.diff(&parse(&out)), None);
}

#[test]
fn a_side_spelled_like_a_literal_stays_a_string() {
    for word in ["true", "nan", "null", "inf"] {
        let src = format!("module @m {{\n  %g = Expr_LogicalAND {{ side = \"{word}\" }}\n}}\n");
        let p = parse(&src);
        let side = &node(&p, 1).properties[&intern("_reroute_side")];
        assert_eq!(side, &Literal::String(word.into()));
        let out = fixed_point(&src);
        assert_eq!(p.diff(&parse(&out)), None, "{out}");
    }
    assert!(
        fixed_point("module @m {\n  %g = Expr_LogicalAND { side = left }\n}\n")
            .contains("side = left")
    );
}

#[test]
fn an_empty_record_pin_type_round_trips() {
    let src = "module @m {\n  %a = in {}\n  out %y {} = %a\n}\n";
    let p = parse(src);
    assert_eq!(node(&p, 1).ports.inputs[0].ty, Type::Record(Vec::new()));
    assert_eq!(node(&p, 2).ports.inputs[0].ty, Type::Record(Vec::new()));
    fixed_point(src);
}

#[test]
fn an_untyped_pin_before_an_out_line() {
    // `out` starts the next item; it is never the pin's class.
    let p = parse("module @m {\n  %a = in\n  out %y bool = %a\n  out %z\n  out %w bool = %a\n}\n");
    assert_eq!(p.module.nodes.len(), 4);
    assert!(p.module.nodes.values().all(|n| n.gate_class != "out"));
}

#[test]
fn a_gate_fed_by_an_untyped_pin_takes_the_pin_type() {
    // `AudioDescriptor` is an `any` input; the pin `%p` takes `int` from `%x`.
    let p = parse(
        "module @m {\n  %x = in int\n  %p = in(%x)\n  %g = Component_AudioEmitter(AudioDescriptor: %p)\n}\n",
    );
    let g = node(&p, 3);
    let port = g
        .ports
        .inputs
        .iter()
        .find(|q| resolve(q.name) == "AudioDescriptor")
        .unwrap();
    assert_eq!(port.ty, Type::Int);
}

#[test]
fn a_chip_module_named_by_an_empty_string_keeps_it() {
    let src = "module @m {\n  %c = chip @\"\" {\n  }\n}\n";
    let p = parse(src);
    assert_eq!(resolve(p.module.chips[&NodeId(1)].name), "");
    assert_eq!(
        resolve(parse("module @m {\n  %c = chip {\n  }\n}\n").module.chips[&NodeId(1)].name),
        "_anon_n1"
    );
    fixed_point(src);
}

#[test]
fn no_fold_is_a_directive() {
    let p = parse("no_fold = true\nmodule @m {\n}\n");
    assert!(p.settings.no_fold);
    assert!(fixed_point("no_fold = true\nmodule @m {\n}\n").contains("no_fold = true"));
    assert!(!parse("module @m {\n}\n").settings.no_fold);
}

#[test]
fn a_long_chain_of_untyped_pins_takes_its_source_type() {
    // Written last-to-first, so each pass of the parser's inference resolves
    // one more pin.
    let mut src = String::from("module @m {\n");
    for i in (1..=12).rev() {
        let _ = writeln!(src, "  %p{i} = in(%p{})", i - 1);
    }
    src += "  %p0 = in int\n}\n";
    let p = parse(&src);
    for n in p.module.nodes.values() {
        assert_eq!(
            n.ports.outputs[0].ty,
            Type::Int,
            "{}",
            resolve(p.module.value_names[&n.id])
        );
    }
}
