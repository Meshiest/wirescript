//! Storage initializers: what a declaration bakes into its gate's
//! `InitialValue`, and the initializers that are dropped with a warning.

use super::*;
use crate::ir::Literal;

/// The `InitialValue` each declaration bakes into the storage gate named
/// `name`. The declared type picks the literal (bool/int/float coerce both
/// ways), so an Int literal on a `float` var bakes a Float; constant
/// constructors fold into the gate; an unannotated type is inferred.
#[test]
fn scalar_var_init_bakes_in_the_declared_type() {
    let int = Literal::Int;
    let rows: Vec<(&str, &str, &str, Literal)> = vec![
        (
            "var v: float = 0\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Float(0.0),
        ),
        (
            "var v: float = true\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Float(1.0),
        ),
        ("var v: int = 1.5\nout o = v\n", gc::VAR, "v", int(1)),
        ("var v: int = true\nout o = v\n", gc::VAR, "v", int(1)),
        (
            "var v: bool = 1\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Bool(true),
        ),
        (
            "var v: bool = 0.0\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Bool(false),
        ),
        (
            "var v: bool = \"x\"\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Bool(true),
        ),
        (
            "var v: bool = \"\"\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Bool(false),
        ),
        // A record field's backing Var gate widens the same way.
        (
            "type P = { x: float }\nvar p: P = { x: 3 }\nout o = p.x\n",
            gc::VAR,
            "p.x",
            Literal::Float(3.0),
        ),
        // `out y: float = 0` seeds the BACKING var rather than wiring a driver
        // (a direct wire would fan in with the var's own feed); that var is
        // unlabelled.
        (
            "in start: exec\nout y: float = 0\non start {\n  emit y = 1.5\n}\n",
            gc::VAR,
            "",
            Literal::Float(0.0),
        ),
        (
            "var s = \"\"\nout ready = s == \"go\"\n",
            gc::VAR,
            "s",
            Literal::String(String::new()),
        ),
        (
            "var v = Vec(1.0, 2.0, 3.0)\nout o = v\n",
            gc::VAR,
            "v",
            Literal::Vector {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
        ),
        (
            "var rot = Rotation(0.0, 90.0, 0.0)\nout o = rot\n",
            gc::VAR,
            "rot",
            Literal::Rotator {
                pitch: 0.0,
                yaw: 90.0,
                roll: 0.0,
            },
        ),
        // Color(r, g, b) is linear 0-1 with alpha defaulting to opaque.
        (
            "var tint = Color(1.0, 0.5, 0.0)\nout o = tint\n",
            gc::VAR,
            "tint",
            Literal::LinearColor {
                r: 1.0,
                g: 0.5,
                b: 0.0,
                a: 1.0,
            },
        ),
        (
            "var q: quat\nout o = q\n",
            gc::VAR,
            "q",
            Literal::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
        ),
        // `:red` is the int constant atom_hash("red").
        (
            "static var team: int = :red\n",
            gc::VAR,
            "team",
            int(crate::hash::atom_hash("red")),
        ),
        // `null` adopts its target type and bakes that type's default.
        (
            "var e: entity = null\nvar i: int = null\nvar b: bool = null\nout oe: entity = e",
            gc::VAR,
            "e",
            Literal::Object,
        ),
        (
            "var e: entity = null\nvar i: int = null\nvar b: bool = null\nout oe: entity = e",
            gc::VAR,
            "i",
            int(0),
        ),
        (
            "var e: entity = null\nvar i: int = null\nvar b: bool = null\nout oe: entity = e",
            gc::VAR,
            "b",
            Literal::Bool(false),
        ),
        (
            "var a: int[] = [1, 2, -3]\n",
            gc::ARRAY_VAR,
            "a",
            Literal::Array(vec![int(1), int(2), int(-3)]),
        ),
        (
            "var foo = [10, 20, 30]\n",
            gc::ARRAY_VAR,
            "foo",
            Literal::Array(vec![int(10), int(20), int(30)]),
        ),
        (
            "var pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]\n",
            gc::ARRAY_VAR,
            "pts",
            Literal::Array(vec![
                Literal::Vector {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                Literal::Vector {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
            ]),
        ),
    ];
    let label = |n: &crate::ir::Node| match n.properties.get(&crate::intern::sym::NAME_LABEL) {
        Some(Literal::String(s)) => s.clone(),
        _ => String::new(),
    };
    let mut failures = Vec::new();
    for (src, class, name, want) in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let gates = nodes_of(&r.module, class);
        let got = gates.iter().find(|n| label(n) == *name).map(|n| {
            n.properties
                .get(&crate::intern::intern("InitialValue"))
                .cloned()
        });
        if !errors.is_empty() || has_unsupported(&r.module) || got != Some(Some(want.clone())) {
            let labels: Vec<_> = gates.iter().map(|n| label(n)).collect();
            failures.push(format!("{src:?} `{name}`: errors {errors:?}, got {got:?}, want {want:?}; {class} labels {labels:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
    // A typed `==` on the inferred string var resolves to a string compare.
    assert!(has_gate(
        &compile("var s = \"\"\nout ready = s == \"go\"\n"),
        gc::COMPARE_EQUAL
    ));
    // A constant `Vec` initializer folds; it spawns no MakeVector.
    assert!(!has_gate_deep(
        &compile("var v = Vec(1.0, 2.0, 3.0)\nout o = v\n"),
        gc::MAKE_VECTOR
    ));
}

/// A var initializer the compiler cannot keep warns WSP001 ("dropped"): a
/// non-constant top-level or chip-pure one, a `static var` one even in exec,
/// and a non-literal array one in exec. A constant initializer, or a runtime
/// one on a handler `var`, is kept with no warning.
#[test]
fn dropped_var_initializers_warn() {
    let rows: &[(&str, usize, Option<&str>)] = &[
        (
            "in a: int\nvar x: int = a + 1\nout r = x",
            1,
            Some("'var x'"),
        ),
        (
            "var x: int = 5\nvar v: vector = Vec(1.0, 2.0, 3.0)\nout r = x",
            0,
            None,
        ),
        (
            "chip g(e: bool) {\n  var x: bool = e\n}\nvar e: bool = true\ng(e)",
            1,
            None,
        ),
        (
            "in start: exec\nin a: int\non start { var x: int = a + 1\nx = x }",
            0,
            None,
        ),
        (
            "in start: exec\nin a: int\non start { static var x: int = a + 1\nx = x }",
            1,
            Some("static var"),
        ),
        (
            "in start: exec\non start { var f: int[] = [1, 2]\nvar g2: int[] = f\ng2.push(3) }",
            0,
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (src, count, says) in rows {
        let r = compile(src);
        let warns: Vec<&str> = r
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == crate::diagnostic::Severity::Warning && d.message.contains("dropped")
            })
            .map(|d| d.message.as_str())
            .collect();
        if warns.len() != *count || says.is_some_and(|s| !warns[0].contains(s)) {
            failures.push(format!(
                "{src:?}: want {count} x {says:?}, diags: {:?}",
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
