use super::*;
use crate::lower::fold::table::{CaseValue, CertifiedTable, InVariant};

const SELECT: &str = "BrickComponentType_Expr_Select";
const BRANCH: &str = "BrickComponentType_Exec_Branch";
const FORMAT_TEXT: &str = "BrickComponentType_Expr_String_FormatText";

// The extended-math / bitwise / rounding laws are gated out of `eval` by
// `covers()` until re-probed, so test the pure laws directly here.
#[test]
fn extended_math_laws() {
    let f = |x: f64| Some(Value::Float(x));
    let got = |g: &str, a: Option<Value>, b: Option<Value>, c: Option<Value>| match extended_math(
        g,
        a.as_ref(),
        b.as_ref(),
        c.as_ref(),
    ) {
        Some(Value::Float(v)) => Some(v),
        _ => None,
    };
    let approx = |a: Option<f64>, b: f64| a.is_some_and(|v| (v - b).abs() < 1e-6);
    assert!(approx(got("MathSqrt", f(9.0), None, None), 3.0));
    assert!(approx(got("MathPow", f(2.0), f(10.0), None), 1024.0));
    assert!(approx(got("MathMin", f(3.0), f(7.0), None), 3.0));
    assert!(approx(got("MathMax", f(3.0), f(7.0), None), 7.0));
    assert!(approx(got("MathAbs", f(-4.0), None, None), 4.0));
    assert!(approx(got("MathSign", f(-3.0), None, None), -1.0));
    assert!(approx(got("MathSign", f(0.0), None, None), 0.0));
    assert!(approx(got("MathClamp", f(5.0), f(0.0), f(1.0)), 1.0));
    assert!(approx(got("MathLogBase", f(8.0), f(2.0), None), 3.0));
    assert!(approx(got("MathModuloFloored", f(-1.0), f(3.0), None), 2.0)); // divisor-signed
    // Domain error -> non-finite result -> refuse.
    assert!(got("MathSqrt", f(-1.0), None, None).is_none());
    assert!(got("MathLn", f(-1.0), None, None).is_none());
}

#[test]
fn integer_operands_keep_an_integer_result() {
    let i = |n: i64| Some(Value::Int(n));
    let got = |g: &str, a: Option<Value>, b: Option<Value>, c: Option<Value>| {
        extended_math(g, a.as_ref(), b.as_ref(), c.as_ref())
    };
    assert_eq!(got("MathMax", i(3), i(7), None), Some(Value::Int(7)));
    assert_eq!(got("MathMin", i(3), i(7), None), Some(Value::Int(3)));
    assert_eq!(got("MathAbs", i(-4), None, None), Some(Value::Int(4)));
    assert_eq!(got("MathClamp", i(12), i(0), i(10)), Some(Value::Int(10)));
    assert_eq!(
        got("MathAbs", Some(Value::Bool(true)), None, None),
        Some(Value::Int(1))
    );
    assert_eq!(got("MathAbs", i(i64::MIN), None, None), None);
    assert_eq!(
        got("MathMax", i(3), Some(Value::Float(7.5)), None),
        Some(Value::Float(7.5))
    );
}

#[test]
fn bitwise_laws() {
    let i = |n: i64| Some(Value::Int(n));
    let got = |g: &str, a: Option<Value>, b: Option<Value>| match bitwise(g, a.as_ref(), b.as_ref())
    {
        Some(Value::Int(n)) => Some(n),
        _ => None,
    };
    assert_eq!(got("BitwiseAND", i(12), i(10)), Some(8));
    assert_eq!(got("BitwiseOR", i(12), i(10)), Some(14));
    assert_eq!(got("BitwiseXOR", i(12), i(10)), Some(6));
    assert_eq!(got("BitwiseNOT", i(12), None), Some(-13));
    assert_eq!(got("BitwiseNAND", i(12), i(10)), Some(!(0b1100 & 0b1010)));
    assert_eq!(got("BitwiseNOR", i(12), i(10)), Some(!(0b1100 | 0b1010)));
    assert_eq!(got("BitwiseShiftLeft", i(1), i(5)), Some(32));
    assert_eq!(got("BitwiseShiftRight", i(64), i(2)), Some(16));
    assert_eq!(got("BitwiseBitCount", i(255), None), Some(8));
    // Out-of-range shift -> refuse.
    assert_eq!(got("BitwiseShiftLeft", i(1), i(64)), None);
    assert_eq!(got("BitwiseShiftLeft", i(1), i(-1)), None);
}

#[test]
fn rounding_laws() {
    // Round/Floor/Ceil route through `eval`'s explicit arms, but `covers()`
    // gates them; exercise the underlying `as_float`-fed f64 methods here.
    assert_eq!(
        as_float(Some(&Value::Float(2.6))).map(f64::round),
        Some(3.0)
    );
    assert_eq!(
        as_float(Some(&Value::Float(2.9))).map(f64::floor),
        Some(2.0)
    );
    assert_eq!(as_float(Some(&Value::Float(2.1))).map(f64::ceil), Some(3.0));
}

fn case_value(ci: &crate::lower::fold::table::CaseInput) -> Option<Value> {
    let v = ci.value.as_ref()?;
    Some(match (ci.variant, v) {
        (InVariant::Int, CaseValue::Scalar(s)) => Value::Int(s.parse().expect("int case value")),
        (InVariant::Float, CaseValue::Scalar(s)) => Value::Float(match s.as_str() {
            "NaN" => f64::NAN,
            "inf" => f64::INFINITY,
            "-inf" => f64::NEG_INFINITY,
            _ => s.parse().expect("float case value"),
        }),
        (InVariant::Bool, CaseValue::Scalar(s)) => Value::Bool(s == "true"),
        (InVariant::Str, CaseValue::Scalar(s)) => Value::Str(s.clone()),
        (InVariant::Vector, CaseValue::Vector { x, y, z }) => Value::Vector {
            x: *x,
            y: *y,
            z: *z,
        },
        (InVariant::Rotator, CaseValue::Rotator { pitch, yaw, roll }) => Value::Rotator {
            pitch: *pitch,
            yaw: *yaw,
            roll: *roll,
        },
        (InVariant::Quat, CaseValue::Quat { x, y, z, w }) => Value::Quat {
            x: *x,
            y: *y,
            z: *z,
            w: *w,
        },
        (InVariant::Color, CaseValue::Color { r, g, b, a }) => Value::Color {
            r: *r,
            g: *g,
            b: *b,
            a: *a,
        },
        // FormatText's synthetic tmpl label; the replay loop skips FormatText.
        (InVariant::Tmpl, _) => return None,
        (InVariant::Unwired, _) => unreachable!(),
        _ => unreachable!("variant/CaseValue shape mismatch — table.rs bug"),
    })
}

/// Signatures and values eval refuses on purpose. Every table case must
/// replay exactly or match one of these; the counts are asserted so a law
/// cannot quietly turn into a refusal.
fn is_expected_refusal(gate: &str, case: &crate::lower::fold::table::Case) -> bool {
    let sig: Vec<InVariant> = case.inputs.iter().map(|i| i.variant).collect();
    if gate.contains("_Math") && sig.contains(&InVariant::Str) {
        return true;
    }
    // Multibyte string operands: the four "π≈3" cases (the string family is
    // ASCII-only).
    gate.contains("_String_")
        && case.inputs.iter().any(|i| match &i.value {
            Some(CaseValue::Scalar(s)) if i.variant == InVariant::Str => !s.is_ascii(),
            _ => false,
        })
}

/// Composite constructors whose own cases render blank, which proves nothing
/// about the value, so they are counted rather than replayed. `MakeQuaternion`
/// and `MakeColor` are certified through other gates; the rest are
/// `BLANK_RENDER_REFUSED`.
const BLANK_RENDER_ONLY: &[&str] = &[
    G_MAKE_ROTATION,
    G_MAKE_QUATERNION,
    G_MAKE_COLOR,
    G_MAKE_COLOR_SRGB,
    G_MAKE_COLOR_HEX,
    G_INVERT_ROTATION,
];

#[test]
fn replay_every_certified_case() {
    let t = CertifiedTable::certified();
    let (mut replayed, mut refused, mut blank) = (0usize, 0usize, 0usize);
    for gate in t.gate_classes() {
        let short = gate.rsplit('_').next().unwrap_or("");
        for case in t.cases(gate) {
            if BLANK_RENDER_ONLY.contains(&short) {
                blank += 1;
                continue;
            }
            // The table stores only a label per FormatText case (`tmpl:0str`),
            // not the template or operands; `format_text` is checked against
            // the probe's templates in `format_text_matches_recovered_probe_cases`.
            if gate == FORMAT_TEXT {
                refused += 1;
                continue;
            }
            // `deferredOps`: probed but always refused.
            if DEFERRED.contains(&short) {
                refused += 1;
                continue;
            }
            let inputs: Vec<Option<Value>> = case.inputs.iter().map(case_value).collect();
            if gate == SELECT || gate == BRANCH {
                // Truthiness gates: recorded output encodes which side won.
                let want_truthy = match case.output_value.as_str() {
                    "111" | "A" => true,
                    "222" | "B" => false,
                    other => panic!("{gate}: unexpected output {other:?}"),
                };
                assert_eq!(
                    truthy(inputs[0].as_ref()),
                    want_truthy,
                    "{gate} {:?}",
                    case.inputs
                );
                replayed += 1;
                continue;
            }
            match eval(gate, &inputs) {
                Some(v) => {
                    assert_eq!(
                        render(&v),
                        case.output_value,
                        "{gate} {:?} evaluated {v:?}",
                        case.inputs
                    );
                    replayed += 1;
                }
                None => {
                    assert!(
                        is_expected_refusal(gate, case),
                        "{gate} {:?}: unexpected refusal",
                        case.inputs
                    );
                    // Every refused math-with-string case recorded 0. The
                    // multibyte string cases recorded real output that the
                    // evaluator declines to reproduce.
                    if gate.contains("_Math") {
                        assert_eq!(case.output_value, "0");
                    }
                    refused += 1;
                }
            }
        }
    }
    assert_eq!(
        replayed + refused + blank,
        413,
        "table case count changed - re-audit"
    );
    // Includes 30 extendedMath, 9 bitwise, 6 rounding and 8 integer-operand
    // extended-math cases.
    assert_eq!(replayed, 379);
    assert_eq!(
        refused, 28,
        "3 math-with-string + 11 FormatText + 4 multibyte + 10 deferredOps"
    );
    assert_eq!(
        blank, 6,
        "MakeRotation/MakeQuaternion/MakeColor/MakeColorSRGB/MakeColorHex/\
            InvertRotation — blank==blank proves nothing, see BLANK_RENDER_ONLY"
    );
}

/// Each `fmt*` probe's template and `Opaque(...)` operands, copied from
/// `probes/gate_semantics.ws`, with its recorded `tmpl:LABEL` output from
/// `data/gate_semantics.json`.
#[test]
fn format_text_matches_recovered_probe_cases() {
    let i = |v: Value| Some(v);
    assert_eq!(
        format_text("{0}", &[i(Value::Str("hi".into()))]),
        Some("hi".into())
    );
    assert_eq!(
        format_text("a{0}b", &[i(Value::Str("X".into()))]),
        Some("aXb".into())
    );
    assert_eq!(
        format_text(
            "{0}{1}",
            &[i(Value::Str("A".into())), i(Value::Str("B".into()))]
        ),
        Some("AB".into())
    );
    assert_eq!(
        format_text(
            "{1}-{0}",
            &[i(Value::Str("A".into())), i(Value::Str("B".into()))]
        ),
        Some("B-A".into())
    );
    assert_eq!(format_text("{0}", &[i(Value::Int(42))]), Some("42".into()));
    assert_eq!(
        format_text("{0}", &[i(Value::Float(0.5))]),
        Some("0.5".into())
    );
    assert_eq!(
        format_text("{0}", &[i(Value::Bool(true))]),
        Some("1".into())
    );
    assert_eq!(
        format_text("{0}", &[i(Value::Str("s".into()))]),
        Some("s".into())
    );
    assert_eq!(
        format_text(
            "{0}-{1}-{2}",
            &[
                i(Value::Str("A".into())),
                i(Value::Str("B".into())),
                i(Value::Str("C".into()))
            ]
        ),
        Some("A-B-C".into())
    );
    // `Fmt("literal{a}brace")`: `{a}` is not a numbered slot, so it renders
    // "0" like any other unbound tag.
    assert_eq!(
        format_text("literal{a}brace", &[]),
        Some("literal0brace".into())
    );
    // `Fmt("{0}{1}", Opaque(1))`: slot 1 has no operand.
    assert_eq!(
        format_text("{0}{1}", &[i(Value::Int(1))]),
        Some("10".into())
    );
}

/// The gate's lettered inputs, colon options and `|` word forms are not
/// probed, so a template using them must stay a runtime gate.
#[test]
fn format_text_refuses_unprobed_gate_syntax() {
    let i = |v: Value| Some(v);
    assert_eq!(format_text("{A} apples", &[i(Value::Int(5))]), None);
    assert_eq!(format_text("{G}", &[]), None);
    assert_eq!(format_text("{A:,}", &[i(Value::Int(1_234_567))]), None);
    assert_eq!(format_text("{0:.2} m/s", &[i(Value::Float(1.2345))]), None);
    assert_eq!(
        format_text("Door: {A:Open=Closed}", &[i(Value::Bool(true))]),
        None
    );
    assert_eq!(
        format_text(
            "{0} {0}|plural(one=apple,other=apples)",
            &[i(Value::Int(3))]
        ),
        None
    );
    assert_eq!(
        format_text(
            "{A}{A}|ordinal(one=st,two=nd,few=rd,other=th)",
            &[i(Value::Int(2))]
        ),
        None
    );
    // A lowercase or longer tag is not an input name and keeps its certified "0".
    assert_eq!(format_text("{a}", &[]), Some("0".into()));
    assert_eq!(format_text("{Apple}", &[]), Some("0".into()));
    // A `|` that is not a word form is ordinary text, even right after a tag.
    assert_eq!(
        format_text("a | {0}", &[i(Value::Int(1))]),
        Some("a | 1".into())
    );
    assert_eq!(
        format_text("{0}|{0}", &[i(Value::Int(3))]),
        Some("3|3".into())
    );
    assert_eq!(
        format_text("{0}|x", &[i(Value::Int(3))]),
        Some("3|x".into())
    );
}

#[test]
fn render_for_format_matches_certified_table() {
    let t = CertifiedTable::certified();
    let laws = t.render_laws();
    let check = |label: &str, v: Value| {
        assert_eq!(
            render_for_format(&v),
            laws[label],
            "render law mismatch for {label}"
        );
    };
    check("int:0", Value::Int(0));
    check("int:7", Value::Int(7));
    check("int:-7", Value::Int(-7));
    check("int:999", Value::Int(999));
    check("int:1000", Value::Int(1000));
    check("int:9999", Value::Int(9999));
    check("int:10000", Value::Int(10000));
    check("int:999999", Value::Int(999_999));
    check("int:-1000000", Value::Int(-1_000_000));
    check("int:9007199254740993", Value::Int(9_007_199_254_740_993));
    check("float:1.0", Value::Float(1.0));
    check("float:-1.0", Value::Float(-1.0));
    check("float:0.5", Value::Float(0.5));
    check("float:1.0/3.0", Value::Float(1.0 / 3.0));
    check("float:0.1+0.2", Value::Float(0.1 + 0.2));
    check("float:2.0/3.0", Value::Float(2.0 / 3.0));
    check("float:123456.789", Value::Float(123_456.789));
    check("float:1e-7", Value::Float(1e-7));
    check("float:-0.0", Value::Float(-0.0));
    check("float:1e15", Value::Float(1e15));
    check("float:1.5e-3", Value::Float(1.5e-3));
    check("bool:true", Value::Bool(true));
    check("bool:false", Value::Bool(false));
    check("str:empty", Value::Str(String::new()));
    check("str:a_b", Value::Str("a b".into()));
    check("str:multibyte", Value::Str("π≈3".into()));
    check(
        "vector:Vec(1.0,2.0,3.0)",
        Value::Vector {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
    );
    check(
        "vector:Vec(0.5,-1.25,1.0/3.0)",
        Value::Vector {
            x: 0.5,
            y: -1.25,
            z: 1.0 / 3.0,
        },
    );
    check(
        "rotator:Rotation(0.0,90.0,45.5)",
        Value::Rotator {
            pitch: 0.0,
            yaw: 90.0,
            roll: 45.5,
        },
    );
    check(
        "color:Color(1.0,0.5,0.25)",
        Value::Color {
            r: 1.0,
            g: 0.5,
            b: 0.25,
            a: 1.0,
        },
    );
    check(
        "color:Color(1.0,0.5,0.25,0.5)",
        Value::Color {
            r: 1.0,
            g: 0.5,
            b: 0.25,
            a: 0.5,
        },
    );
    // 0.7071067811865476 is the shortest round-trip spelling of FRAC_1_SQRT_2,
    // so the probe text and the constant name the same f64.
    let h = std::f64::consts::FRAC_1_SQRT_2;
    check(
        "quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476)",
        Value::Quat {
            x: 0.0,
            y: 0.0,
            z: h,
            w: h,
        },
    );
}

#[test]
fn concat_stringifies_bool_natively_not_via_format_law() {
    // Certified: Concatenate prints bools "true"/"false", where
    // render_for_format prints "1"/"0".
    assert_eq!(
        eval(
            "BrickComponentType_Expr_String_Concatenate",
            &[Some(Value::Bool(true)), Some(Value::Str("!".into()))]
        ),
        Some(Value::Str("true!".into()))
    );
}

/// What `eval` answers for an input the table never certified, or one
/// whose answer would differ between Rust and the game: each row's gate
/// short name, inputs, and the answer (`None` refuses to fold).
#[test]
fn eval_refuses_what_the_game_was_not_probed_on() {
    let i = |n: i64| Some(Value::Int(n));
    let f = |n: f64| Some(Value::Float(n));
    let s = |t: &str| Some(Value::Str(t.into()));
    // `Some(None)` means it folds, to any value.
    let rows: Vec<(&str, &str, Vec<Option<Value>>, Option<Option<Value>>)> = vec![
        (
            "multibyte string operands refuse",
            "String_Length",
            vec![s("π≈3")],
            None,
        ),
        (
            "integer overflow refuses",
            "MathAdd",
            vec![i(i64::MAX), i(1)],
            None,
        ),
        (
            "a mixed-sign division is trunc-vs-floor unprobed",
            "MathDivide",
            vec![i(-7), i(2)],
            None,
        ),
        (
            "a zero remainder is safe",
            "MathDivide",
            vec![i(-4), i(2)],
            Some(None),
        ),
        (
            "a mixed-sign int modulo refuses",
            "MathModulo",
            vec![i(-7), i(2)],
            None,
        ),
        (
            "division by zero is certified",
            "MathDivide",
            vec![i(7), i(0)],
            Some(Some(Value::Int(0))),
        ),
        // i64::MIN / -1 (and MIN % -1) overflow i64 unconditionally in
        // Rust; they must refuse via checked ops rather than panic.
        (
            "MIN / -1 refuses without a panic",
            "MathDivide",
            vec![i(i64::MIN), i(-1)],
            None,
        ),
        (
            "MIN % -1 refuses without a panic",
            "MathModulo",
            vec![i(i64::MIN), i(-1)],
            None,
        ),
        (
            "a mixed-sign float modulo refuses",
            "MathModulo",
            vec![f(-3.5), f(2.0)],
            None,
        ),
        // CompareNotEqual was only probed at (int,int)/(int,str).
        (
            "(Str, Unwired) CompareNotEqual is uncovered",
            "CompareNotEqual",
            vec![s("x"), None],
            None,
        ),
        // The reverse of the probed (int,str) direction.
        (
            "(str, int) CompareEqual is uncovered",
            "CompareEqual",
            vec![s("1"), i(1)],
            None,
        ),
        // Bool never appears as the second operand of an ordered compare.
        (
            "a bool second operand of CompareLess is uncovered",
            "CompareLess",
            vec![i(1), Some(Value::Bool(true))],
            None,
        ),
        // NOT was only ever probed on Bool.
        (
            "LogicalNOT on an int is uncovered",
            "LogicalNOT",
            vec![i(1)],
            None,
        ),
        (
            "the covered (int, str) CompareEqual still folds",
            "CompareEqual",
            vec![i(1), s("1")],
            Some(Some(Value::Bool(true))),
        ),
    ];
    let mut failures = Vec::new();
    for (label, short, inputs, want) in &rows {
        let gate = format!("BrickComponentType_Expr_{short}");
        let got = eval(&gate, inputs);
        let ok = match want {
            None => got.is_none(),
            Some(None) => got.is_some(),
            Some(Some(v)) => got.as_ref() == Some(v),
        };
        if !ok {
            failures.push(format!(
                "{label}: {gate}{inputs:?} gave {got:?}, want {want:?}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn oversized_float_refuses_string_fold() {
    let concat = "BrickComponentType_Expr_String_Concatenate";
    // Coverage checks only variant shape, so this magnitude reaches the guard.
    assert!(!string_operands_foldable(&[
        Some(&Value::Str("x".into())),
        Some(&Value::Float(1e16)),
    ]));
    let _ = concat; // documents which gate family this guard protects
}

#[test]
fn oversized_string_result_refuses_fold() {
    let concat = "BrickComponentType_Expr_String_Concatenate";
    // Concatenating two 5000-char strings would produce 10000 chars, exceeding
    // MAX_FOLDED_STRING_LEN (8192), so it must refuse.
    let s5000 = Some(Value::Str("a".repeat(5000)));
    assert!(
        eval(concat, &[s5000.clone(), s5000]).is_none(),
        "oversized result (10000 chars) must refuse"
    );
    // Concatenating two 4096-char strings produces 8192 chars, exactly at the
    // limit, so it should fold.
    let s4096 = Some(Value::Str("a".repeat(4096)));
    assert_eq!(
        eval(concat, &[s4096.clone(), s4096]),
        Some(Value::Str("a".repeat(8192))),
        "result at limit (8192 chars) must fold"
    );
}

#[test]
fn deferred_ops_always_refuse() {
    for short in DEFERRED {
        let gate = format!("BrickComponentType_Expr_{short}");
        let t = CertifiedTable::certified();
        for case in t.cases(&gate) {
            let inputs: Vec<Option<Value>> = case.inputs.iter().map(case_value).collect();
            assert!(eval(&gate, &inputs).is_none(), "{gate} must always refuse");
        }
    }
}

/// Every `BLANK_RENDER_REFUSED` gate refuses even on its own covered
/// signatures.
#[test]
fn blank_render_gates_always_refuse() {
    for short in BLANK_RENDER_REFUSED {
        let gate = format!("BrickComponentType_Expr_{short}");
        let t = CertifiedTable::certified();
        let cases = t.cases(&gate);
        assert!(
            !cases.is_empty(),
            "{gate}: expected at least one certified case"
        );
        for case in cases {
            let inputs: Vec<Option<Value>> = case.inputs.iter().map(case_value).collect();
            assert!(eval(&gate, &inputs).is_none(), "{gate} must always refuse");
        }
    }
}

#[test]
fn rotate_vector_replays_the_45_degree_case_exactly() {
    let gate = "BrickComponentType_Expr_RotateVector";
    let v = Some(Value::Vector {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    });
    let q = Some(Value::Quat {
        x: 0.0,
        y: 0.0,
        z: 0.382_683_432_365_089_8,
        w: 0.923_879_532_511_286_7,
    });
    let got = eval(gate, &[v, q]).expect("covered signature must fold");
    assert_eq!(render(&got), "X=0.707 Y=0.707 Z=0.000");
}

#[test]
fn composite_modulo_mixed_sign_does_not_refuse() {
    // Certified: unlike the scalar path, vector modulo computes the truncated
    // remainder even for mixed signs.
    let md = "BrickComponentType_Expr_MathModulo";
    let v1 = Some(Value::Vector {
        x: 0.5,
        y: 0.25,
        z: -0.75,
    });
    let v2 = Some(Value::Vector {
        x: 0.25,
        y: 0.5,
        z: 0.75,
    });
    let got = eval(md, &[v1, v2]).expect("composite modulo must not refuse on mixed sign");
    assert_eq!(render(&got), "X=0.000 Y=0.250 Z=-0.000");
}

#[test]
fn render_matches_formattext() {
    assert_eq!(render(&Value::Float(1.0)), "1");
    assert_eq!(render(&Value::Float(0.75)), "0.75");
    assert_eq!(render(&Value::Float(f64::INFINITY)), "0");
    assert_eq!(render(&Value::Float(f64::NAN)), "0");
    assert_eq!(render(&Value::Float(-0.0)), "0");
    assert_eq!(render(&Value::Bool(true)), "true");
    assert_eq!(render(&Value::Int(-5)), "-5");
}
