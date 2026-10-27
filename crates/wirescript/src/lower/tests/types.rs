use std::fmt::Write as _;

use super::*;
use crate::ir::gate_class as gc;

#[test]
fn typed_vars_carry_correct_default_literal() {
    use crate::ir::Literal;
    // A var of any variant member type gets a type-matched initial value,
    // not a numeric default (which would mis-tag the `WireValue`).
    let initial = |src: &str| {
        let r = compile_ok(src);
        r.module
            .nodes
            .values()
            .find(|n| n.gate_class == gc::VAR)
            .and_then(|n| {
                n.properties
                    .get(&crate::intern::intern_static("InitialValue"))
            })
            .cloned()
    };
    assert!(matches!(
        initial("var v: vector"),
        Some(Literal::Vector { .. })
    ));
    assert!(matches!(initial("var s: string"), Some(Literal::String(_))));
    assert!(matches!(initial("var b: bool"), Some(Literal::Bool(false))));
    assert!(matches!(initial("var i: int"), Some(Literal::Int(0))));
}

/// Bitwise operators take bool and float operands as well as ints, and an
/// infix `&` stays a bitwise AND beside the `&x` ref syntax. Each row gives
/// exact counts of the named gates.
#[test]
fn bitwise_operators_accept_bool_and_float_operands() {
    const SHL: &str = "BrickComponentType_Expr_BitwiseShiftLeft";
    const OR: &str = "BrickComponentType_Expr_BitwiseOR";
    const AND: &str = "BrickComponentType_Expr_BitwiseAND";
    let rows: &[(&str, &str, &[(&str, usize)])] = &[
        (
            "bool << int",
            "in flag: bool\nlet x = flag << 3\nout result = x",
            &[(SHL, 1)],
        ),
        (
            "bool | int",
            "in a: bool\nlet c = a | 0xFF\nout result = c",
            &[(OR, 1)],
        ),
        (
            "bool & bool",
            "in a: bool\nin b: bool\nlet c = a & b\nout result = c",
            &[(AND, 1)],
        ),
        (
            "float << int",
            "in f: float\nlet x = f << 2\nout result = x",
            &[(SHL, 1)],
        ),
        (
            "float & float",
            "in a: float\nin b: float\nlet c = a & b\nout result = c",
            &[(AND, 1)],
        ),
        (
            "~bool",
            "in a: bool\nlet c = ~a\nout result = c",
            &[(gc::BITWISE_NOT, 1)],
        ),
        (
            "bools packed into a bitmask",
            "in a: bool\nin b: bool\nin c: bool\nlet mask = a | (b << 1) | (c << 2)\nout result = mask",
            &[(OR, 2), (SHL, 2)],
        ),
        (
            "infix & is still bitwise AND",
            "in a: int\nin b: int\nlet c = a & b\nout result = c",
            &[(AND, 1)],
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}\n{src}"));
            continue;
        }
        for &(class, n) in want {
            let got = count_class(&r.module, class);
            if got != n {
                failures.push(format!("{label}: {got} x {class}, want {n}\n{src}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// WS016 warns when a `let` annotation names a type its value cannot become,
/// and only then; the warning names the annotated type.
#[test]
fn ws016_warns_only_on_a_let_annotation_its_value_cannot_meet() {
    let rows: &[(&str, &str, usize)] = &[
        (
            "a vector can't become an int",
            "let x: int = Vec(1.0, 2.0, 3.0)\nout result = x",
            1,
        ),
        // Everything primitive casts to string: an intentional format, not a type lie.
        (
            "a string annotation casts a number",
            "let x: string = 42\nout result = x",
            0,
        ),
        (
            "int coerces to float",
            "let x: float = 42\nout result = x",
            0,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let warns: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS016").collect();
        let severities_ok = warns
            .iter()
            .all(|d| d.severity == crate::diagnostic::Severity::Warning);
        let names_int = want == 0 || warns.iter().all(|d| d.message.contains("int"));
        if warns.len() != want || !severities_ok || !names_int {
            failures.push(format!(
                "{label}: {} WS016, want {want}: {:?}\n{src}",
                warns.len(),
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// WS017 warns on an `out` bound straight to a `var` with no port type,
/// offering the ref form; an annotated port, a `let` or an expression is fine.
#[test]
fn ws017_warns_on_an_untyped_out_bound_to_a_var() {
    let rows: &[(&str, &str, usize)] = &[
        (
            "out from a var without a type",
            "var x: int = 0\nout result = x",
            1,
        ),
        (
            "out from a var with a type",
            "var x: int = 0\nout result: int = x",
            0,
        ),
        ("out from a let", "let x = 42\nout result = x", 0),
        ("out from an expression", "in a: int\nout result = a + 1", 0),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let got = r.diagnostics.iter().filter(|d| d.code == "WS017").count();
        if got != want {
            failures.push(format!(
                "{label}: {got} WS017, want {want}: {:?}\n{src}",
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());

    let r = compile("var x: int = 0\nout result = x");
    let w = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS017")
        .expect("WS017");
    assert!(
        w.message.contains("int"),
        "warning should mention the var's type"
    );
    assert!(
        w.message.contains("`out result: *int = &x`"),
        "{}",
        w.message
    );
    // A var holding a ref is offered only the ref it holds: a ref to it is a
    // `**int`, which is WS086.
    let r = compile("var x: int = 0\nvar h: *int = &x\nout result = h");
    let w = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS017")
        .expect("WS017");
    assert!(
        w.message.contains("`out result: *int = h`") && !w.message.contains("**"),
        "{}",
        w.message
    );
}

#[test]
fn deref_in_exec_produces_var_get() {
    let r = compile_ok("var x: int = 0\nin tick: exec\non tick { let v = *x }");
    assert_eq!(
        count_class(&r.module, gc::VAR_GET),
        1,
        "one Var_Get for the deref"
    );
}

/// `null` compares with an object, a brick or a ref through `CompareEqual` /
/// `CompareNotEqual` on either side, and an annotated `null` let or const is its
/// annotated type. Each row lists `.wsir` lines by the needles one line must
/// all contain.
#[test]
fn null_compares_with_objects_bricks_and_refs() {
    let rows: &[(&str, &str, &[&[&str]])] = &[
        (
            "an object compares with null",
            "var it: character = null\n@left in go: exec\n@left in e: entity\n@left in p: controller\n\
             on go {\n  if it != null { BroadcastChatMessage(\"set\") }\n  if null == it { BroadcastChatMessage(\"unset\") }\n}\n\
             @right out noE = e == null\n@right out someP = p != null\n",
            &[
                &["Expr_CompareNotEqual", "InputB: character", "null"],
                &["Expr_CompareEqual", "InputA: character", "null"],
                &["Expr_CompareEqual", "%e", "null"],
                &["Expr_CompareNotEqual", "%p", "null"],
            ],
        ),
        (
            "a brick or ref compares with null",
            "@left in b: brick\n@right out none = b == null\nvar a: int = 0\nvar h: *int\n\
             @left in go: exec\non go { if h == null { h = &a } }\n",
            &[
                &["Expr_CompareEqual", "%b"],
                &["Expr_CompareEqual", "brick"],
            ],
        ),
        (
            "an annotated null let or const is its type",
            "let n: character = null\nconst N: entity = null\n@right out o = n\n@right out q = N\n",
            &[
                &["%o = ", "(RER_Input: character)"],
                &["%q = ", "(RER_Input: entity)"],
            ],
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, lines) in rows {
        let (text, diags) = wsir_and_diags(src);
        let ws: Vec<_> = diags.iter().filter(|d| d.starts_with("WS")).collect();
        if !ws.is_empty() {
            failures.push(format!("{label}: diagnostics {ws:?}"));
        }
        if text.contains("_Unsupported") {
            failures.push(format!("{label}: a placeholder in:\n{text}"));
        }
        for needles in lines {
            if !text.lines().any(|l| needles.iter().all(|n| l.contains(n))) {
                failures.push(format!("{label}: no line with {needles:?} in:\n{text}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Lowering's type resolution (`resolve_local_type`) keeps a `zone`/`teleport`
/// annotation on an `in` port rather than degrading it to `Type::Any`.
/// `pre_declare_input` feeds that type straight into the `MicrochipInput` node's
/// output port, so an `Any` drift is observable there.
#[test]
fn zone_and_teleport_annotations_keep_their_type_through_lowering() {
    let r = compile_ok("in z: zone\nin p: teleport");
    let port_ty = |name: &str| {
        r.module
            .nodes
            .values()
            .find(|n| {
                n.gate_class == crate::ir::gate_class::MICROCHIP_INPUT
                    && n.properties.get(&*crate::intern::sym::PORT_LABEL)
                        == Some(&Literal::String(name.into()))
            })
            .map(|n| n.ports.outputs[0].ty.clone())
    };
    assert_eq!(
        port_ty("z"),
        Some(Type::Zone),
        "`in z: zone` must stay Type::Zone through lowering, not degrade to Type::Any"
    );
    assert_eq!(
        port_ty("p"),
        Some(Type::Teleport),
        "`in p: teleport` must stay Type::Teleport through lowering, not degrade to Type::Any"
    );
}

/// A self-referential record alias used as STORAGE terminates: typecheck reports
/// `WS002 recursive type alias`, and lowering's per-field decomposition of the
/// record var must bound its recursion. The compile runs on its own thread with
/// a deadline, so a hang fails the test instead of wedging the suite.
#[test]
fn a_self_referential_record_alias_used_as_storage_terminates() {
    for src in [
        "type L<T> = { tail: L<T> }\nvar l: L<int>",
        "type L = { tail: L }\nvar l: L",
        "type L<T> = { tail: L<T> }\nin l: L<int>",
        "type P<T> = { a: Q<T> }\ntype Q<T> = { b: P<T> }\nvar p: P<int>",
    ] {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(compile(src).diagnostics.len());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(20)).is_ok(),
            "lowering did not terminate for a recursive record alias:\n{src}"
        );
    }
}

/// A local `var` initialized from an array literal of runtime values, with no
/// annotation, is an array of the elements' type, as its annotated twin is:
/// a string array for strings, never the `f64` default.
#[test]
fn an_unannotated_array_literal_of_runtime_values_takes_their_type() {
    let head = "var k: int = 3\nvar n: int = 0\nvar so: string = \"\"\nvar st: string = \"x\"\n\
                var vv: vector = Vec(1.0, 2.0, 3.0)\nvar vo: vector = Vec(0.0, 0.0, 0.0)\nin go: exec\n";
    for (untyped, typed) in [
        (
            "var x = [st, st]\n so = x[0]",
            "var x: string[] = [st, st]\n so = x[0]",
        ),
        ("var y = [k]\n n = y[0]", "var y: int[] = [k]\n n = y[0]"),
        (
            "var z = [vv, vv]\n vo = z[1]",
            "var z: vector[] = [vv, vv]\n vo = z[1]",
        ),
        (
            "var w = [k, 2]\n n = w.length()",
            "var w: int[] = [k, 2]\n n = w.length()",
        ),
    ] {
        let program = |body: &str| format!("{head}on go {{ {body} }}\n");
        for ((mode, a), (_, b)) in super::generics::both_folds(&program(untyped))
            .into_iter()
            .zip(super::generics::both_folds(&program(typed)))
        {
            assert_no_errors(&a);
            assert_eq!(
                super::generic_differential::shape(&a),
                super::generic_differential::shape(&b),
                "`{untyped}`, {mode}"
            );
        }
    }
}

/// An unannotated array literal of enums is an enum array: each compiles as
/// its annotated twin does.
#[test]
fn an_unannotated_enum_array_literal_is_its_annotated_twin() {
    let head = "enum Hue { Red, Green, Blue }\nvar c: Hue = Hue.Green\nvar d: Hue = Hue.Red\nin go: exec\n";
    let cases = [
        (
            "var x = [c, d]\n d = x[0]",
            "var x: Hue[] = [c, d]\n d = x[0]",
        ),
        (
            "var x = [Hue.Red, c]\n d = x[1]",
            "var x: Hue[] = [Hue.Red, c]\n d = x[1]",
        ),
    ];
    for (untyped, typed) in cases {
        let a = super::generics::both_folds(&format!("{head}on go {{ {untyped} }}"));
        let b = super::generics::both_folds(&format!("{head}on go {{ {typed} }}"));
        for ((mode, ra), (_, rb)) in a.into_iter().zip(b) {
            assert_no_errors(&ra);
            assert_eq!(
                super::generic_differential::shape(&ra),
                super::generic_differential::shape(&rb),
                "`{untyped}`, {mode}"
            );
        }
    }
}

/// An unannotated `var` initialized with an empty `[]` or `{}` is WS098,
/// whatever the program writes to it, and the message asks for an
/// annotation. A body checked more than once reports it once.
#[test]
fn an_unannotated_empty_container_is_ws098() {
    let head = "type P = { a: int }\nvar k: int = 3\nvar n: int = 0\nvar fl: float = 1.5\nin go: exec\n\
                mod addf(a: float[], v: float) { a.push(v) }\n";
    for body in [
        "var x = []\n n = x.length()",
        "var x = []\n x.push(k)\n n = x[0]",
        "var x = []\n x.push(k)\n x.push(fl)",
        "var x = []\n x[0] = k",
        "var x = []\n x.push({ a: 1 })",
        "static var x = []\n x.push(k)",
        "var m = {}\n m.set(k, k)\n n = m.get(k).Value",
        // Only spreads of empty literals give no element type either.
        "var x = [...[]]\n x.push(k)\n n = x[0]",
        "var x = [...[], ...[]]\n x.push(k)",
        "var x = [...[...[]]]\n x.push(k)",
        // What the var's unknown element type leads to is not reported again.
        "var x = []\n x.push(k)\n fl = x[0] + fl",
        "var x = []\n x.push(k)\n x[0] += 2.5\n fl = x[0]",
        "var x = []\n x.push(k)\n addf(x, 2.5)",
        "var x = []\n let y: float[] = x",
        "var x = []\n var xss: float[][]\n xss.push(x)",
    ] {
        for src in [
            format!("{head}on go {{ {body} }}"),
            format!("{head}chip {{ on go {{ {body} }} }}"),
        ] {
            assert_eq!(errs(&src), ["WS098"], "`{src}`");
        }
    }
    assert_eq!(
        errs(&format!(
            "{head}var x = []\non go {{ x.push(k)\n n = x[0] }}"
        )),
        ["WS098"]
    );
    assert_eq!(
        errs(&format!("{head}var m = {{}}\non go {{ m.set(k, k) }}")),
        ["WS098"]
    );
    let generic = format!(
        "{head}var so: string = \"\"\nvar st: string = \"s\"\n\
         mod first<T>(v: T) -> (r: T) {{ var x = []\n x.push(v)\n return x[0] }}\n\
         on go {{ n = first(k)\n so = first(st) }}"
    );
    assert_eq!(errs(&generic), ["WS098"]);
    let r = compile(&format!("{head}on go {{ var x = []\n x.push(k) }}"));
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS098")
        .expect("WS098");
    assert!(
        d.message.contains("Annotate the declaration") && d.message.contains("`var x: int[] = []`"),
        "{}",
        d.message
    );
    let r = compile(&format!("{head}on go {{ var m = {{}} }}"));
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS098")
        .expect("WS098");
    assert!(
        d.message.contains("`var m: Map<string, int> = {}`"),
        "{}",
        d.message
    );
    // The annotated twins compile.
    for body in [
        "var x: int[] = []\n x.push(k)\n n = x[0]",
        "var m: Map<int, int> = {}\n m.set(k, k)",
    ] {
        assert_no_errors(&compile(&format!("{head}on go {{ {body} }}")));
    }
    // A whole compile reports the declaration alone: a hundred of them give a
    // hundred WS098, and an enum read from one is not lowering's `any` WS003.
    let many = (0..100).fold(String::new(), |mut many, i| {
        let _ = writeln!(many, "var a{i} = []");
        many
    });
    let uses = (0..100).fold(String::new(), |mut uses, i| {
        let _ = writeln!(uses, " a{i}.push(k)\n fl = a{i}[0] + fl");
        uses
    });
    let r = compile(&format!("{head}{many}on go {{\n{uses}}}"));
    let codes: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["WS098"; 100]);
    let r = compile(&format!(
        "{head}enum Hue {{ Red, Blue }}\nvar h: Hue = Hue.Red\non go {{ var xs = []\n xs.push(Hue.Blue)\n h = xs[0] }}"
    ));
    let codes: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["WS098"]);
    // An error that does not follow from the var's element type is still
    // reported, in the same statement, expression or `match` arm.
    let head = format!(
        "{head}var so: string = \"\"\nmod two(a: int, b: int) -> int {{ return a + b }}\nenum E {{ A, B }}\nvar e: E = E.A\n"
    );
    for (body, also) in [
        ("n = x.length() + nosuch", "WS002"),
        ("fl = x.length() + nosuchfn(2)", "WS002"),
        ("n = two(x.length())", "WS022"),
        ("n = x.length() + two(1, 2, 3)", "WS022"),
        ("n = if x.length() > 0 then \"str\" else 2", "WS003"),
        ("var y: int = x.length() + \"s\"", "WS004"),
        ("let r = x.push(k)", "WS072"),
        ("x.push(k)\n so = \"a\" + x[0] + nosuch", "WS002"),
    ] {
        let src = format!("{head}var x = []\non go {{ {body} }}");
        assert_eq!(errs(&src), ["WS098", also], "`{body}`");
    }
    let src = format!(
        "{head}var x = []\non go {{ let r = match e {{ A => {{ fl = nosuch\n x.length() }} B => 0 }} }}"
    );
    assert_eq!(errs(&src), ["WS098", "WS095", "WS002"]);
}

/// An operand whose own error leaves it `any` does not also make its operator
/// `WS004` (`no overload for '+' on Float, Any`).
#[test]
fn an_operand_reported_once_is_not_reported_again_by_its_operator() {
    let src = "var vv: vector = Vec(3.0, 4.0, 0.0)\nvar fo: float = 0.0\nin go: exec\n\
               on go { fo = 1.5 + vv.Length()\n fo = vv.Length() * 2.0 }";
    assert_eq!(errs(src), ["WS003", "WS003"]);
}

/// An unannotated array literal with elements takes the join of its
/// elements (`[1, 2.5]` is a `float[]`), at the top level too, where a named
/// constant element counts by its value. Each compiles as its annotated twin.
#[test]
fn an_unannotated_array_literal_joins_its_elements_like_its_annotated_twin() {
    let head =
        "var k: int = 3\nvar fl: float = 1.5\nvar n: int = 0\nvar fo: float = 0.0\nin go: exec\n";
    let cases = [
        (
            "var x = [1, 2.5]\n fo = x[1]",
            "var x: float[] = [1, 2.5]\n fo = x[1]",
        ),
        (
            "var x = [1, fl]\n fo = x[1]",
            "var x: float[] = [1, fl]\n fo = x[1]",
        ),
    ];
    for (untyped, typed) in cases {
        let a = super::generics::both_folds(&format!("{head}on go {{ {untyped} }}"));
        let b = super::generics::both_folds(&format!("{head}on go {{ {typed} }}"));
        for ((mode, ra), (_, rb)) in a.into_iter().zip(b) {
            assert_no_errors(&ra);
            assert_eq!(
                super::generic_differential::shape(&ra),
                super::generic_differential::shape(&rb),
                "`{untyped}`, {mode}"
            );
        }
    }
    // Top level: literals typed by every element, a named constant by its
    // value even when read above the declaration.
    let top = [
        (
            "const K = 2.5\non go { fo = x[1] }\nvar x = [1, K]",
            "float[]",
        ),
        ("var x = [1, 2.5]\non go { fo = x[1] }", "float[]"),
    ];
    for (untyped, elem) in top {
        let typed = untyped.replacen("var x = ", &format!("var x: {elem} = "), 1);
        for (mode, (ra, rb)) in [
            (
                "fold off",
                (
                    compile(&format!("{head}{untyped}")),
                    compile(&format!("{head}{typed}")),
                ),
            ),
            (
                "fold auto",
                (
                    compile_auto(&format!("{head}{untyped}")),
                    compile_auto(&format!("{head}{typed}")),
                ),
            ),
        ] {
            assert_no_errors(&ra);
            assert_eq!(
                super::generic_differential::shape(&ra),
                super::generic_differential::shape(&rb),
                "`{untyped}`, {mode}"
            );
        }
    }
    // Each instantiation of a generic body has its own element type.
    let generic = |decl: &str| {
        format!(
            "var k: int = 3\nvar st: string = \"s\"\nvar n: int = 0\nvar so: string = \"\"\nin go: exec\n\
             mod first<T>(v: T) -> (r: T) {{ {decl}\n x.push(v)\n return x[1] }}\n\
             on go {{ n = first(k)\n so = first(st) }}"
        )
    };
    let a = compile(&generic("var x = [v]"));
    let b = compile(&generic("var x: T[] = [v]"));
    assert_no_errors(&a);
    assert_eq!(
        super::generic_differential::shape(&a),
        super::generic_differential::shape(&b)
    );
}
