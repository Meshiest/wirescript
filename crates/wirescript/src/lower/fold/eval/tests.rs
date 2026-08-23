    use super::*;
    use crate::lower::fold::table::{CaseValue, CertifiedTable, InVariant};

    const SELECT: &str = "BrickComponentType_WireGraph_Expr_Select";
    const BRANCH: &str = "BrickComponentType_WireGraph_Exec_Branch";
    const FORMAT_TEXT: &str = "BrickComponentType_WireGraph_Expr_String_FormatText";

    // The extended-math / bitwise / rounding laws are gated out of `eval` by
    // `covers()` until re-probed, so test the pure laws directly here.
    #[test]
    fn extended_math_laws() {
        let f = |x: f64| Some(Value::Float(x));
        let got = |g: &str, a: Option<Value>, b: Option<Value>, c: Option<Value>| {
            match extended_math(g, a.as_ref(), b.as_ref(), c.as_ref()) {
                Some(Value::Float(v)) => Some(v),
                _ => None,
            }
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
        // Domain error → non-finite result → refuse.
        assert!(got("MathSqrt", f(-1.0), None, None).is_none());
        assert!(got("MathLn", f(-1.0), None, None).is_none());
    }

    #[test]
    fn bitwise_laws() {
        let i = |n: i64| Some(Value::Int(n));
        let got = |g: &str, a: Option<Value>, b: Option<Value>| match bitwise(g, a.as_ref(), b.as_ref()) {
            Some(Value::Int(n)) => Some(n),
            _ => None,
        };
        assert_eq!(got("BitwiseAND", i(12), i(10)), Some(8));
        assert_eq!(got("BitwiseOR", i(12), i(10)), Some(14));
        assert_eq!(got("BitwiseXOR", i(12), i(10)), Some(6));
        assert_eq!(got("BitwiseNOT", i(12), None), Some(-13));
        assert_eq!(got("BitwiseNAND", i(12), i(10)), Some(!(12 & 10)));
        assert_eq!(got("BitwiseNOR", i(12), i(10)), Some(!(12 | 10)));
        assert_eq!(got("BitwiseShiftLeft", i(1), i(5)), Some(32));
        assert_eq!(got("BitwiseShiftRight", i(64), i(2)), Some(16));
        assert_eq!(got("BitwiseBitCount", i(255), None), Some(8));
        // Out-of-range shift → refuse.
        assert_eq!(got("BitwiseShiftLeft", i(1), i(64)), None);
        assert_eq!(got("BitwiseShiftLeft", i(1), i(-1)), None);
    }

    #[test]
    fn rounding_laws() {
        // Round/Floor/Ceil route through `eval`'s explicit arms, but `covers()`
        // gates them; exercise the underlying `as_float`-fed f64 methods here.
        assert_eq!(as_float(Some(&Value::Float(2.6))).map(f64::round), Some(3.0));
        assert_eq!(as_float(Some(&Value::Float(2.9))).map(f64::floor), Some(2.0));
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
            (InVariant::Vector, CaseValue::Vector { x, y, z }) => {
                Value::Vector { x: *x, y: *y, z: *z }
            }
            (InVariant::Rotator, CaseValue::Rotator { pitch, yaw, roll }) => {
                Value::Rotator { pitch: *pitch, yaw: *yaw, roll: *roll }
            }
            (InVariant::Quat, CaseValue::Quat { x, y, z, w }) => {
                Value::Quat { x: *x, y: *y, z: *z, w: *w }
            }
            (InVariant::Color, CaseValue::Color { r, g, b, a }) => {
                Value::Color { r: *r, g: *g, b: *b, a: *a }
            }
            // FormatText's synthetic tmpl label — not a real value, never
            // reached by the replay loop (FormatText is allowlisted whole).
            (InVariant::Tmpl, _) => return None,
            (InVariant::Unwired, _) => unreachable!(),
            _ => unreachable!("variant/CaseValue shape mismatch — table.rs bug"),
        })
    }

    /// Signatures/values eval deliberately refuses (Global Constraints).
    /// Every table case must either replay exactly or match one of these;
    /// the counts are asserted so laws can't silently rot into refusals.
    fn is_expected_refusal(gate: &str, case: &crate::lower::fold::table::Case) -> bool {
        let sig: Vec<InVariant> = case.inputs.iter().map(|i| i.variant).collect();
        if gate.contains("_Math") && sig.contains(&InVariant::Str) {
            return true;
        }
        // Multibyte string operands (ASCII-only string family, see
        // `ascii_str`'s doc comment) — the 4 "π≈3" cases.
        gate.contains("_String_") && case.inputs.iter().any(|i| match &i.value {
            Some(CaseValue::Scalar(s)) if i.variant == InVariant::Str => !s.is_ascii(),
            _ => false,
        })
    }

    /// Composite-constructor gates whose ONLY table evidence is "renders
    /// blank" (rotator/quat/color case outputs are ALWAYS `""` through
    /// FormatText — certified, see `render_for_format`'s doc comment).
    /// Replaying blank==blank proves nothing about whether `eval()`
    /// computed the right VALUE, so these 6 cases are tallied separately
    /// rather than asserted at all. Two of the six (`MakeQuaternion`,
    /// `MakeColor`'s 3-arg form) DO still fold in production — they're
    /// certified TRANSITIVELY through a different, value-bearing gate (see
    /// `make_quaternion`'s / `make_color`'s doc comments in `eval.rs`
    /// above) — but the replay harness has no way to check that here; the
    /// transitive-certification comment on each `make_*` function is the
    /// actual justification, not this test. The other 4 (`MakeRotation`,
    /// `MakeColorSRGB`, `MakeColorHex`, `InvertRotation`) hard-refuse in
    /// production (see `eval()`'s `BLANK_RENDER_REFUSED`).
    const BLANK_RENDER_ONLY: &[&str] = &[
        G_MAKE_ROTATION, G_MAKE_QUATERNION, G_MAKE_COLOR, G_MAKE_COLOR_SRGB,
        G_MAKE_COLOR_HEX, G_INVERT_ROTATION,
    ];

    #[test]
    fn replay_every_certified_case() {
        let t = CertifiedTable::certified();
        let (mut replayed, mut refused, mut blank) = (0usize, 0usize, 0usize);
        for gate in t.gate_classes() {
            let short = gate.rsplit('_').next().unwrap_or("");
            for case in t.cases(gate) {
                // Blank-render-only composites: see BLANK_RENDER_ONLY's doc
                // comment — blank==blank proves nothing, so these are
                // neither replayed nor treated as an expected refusal; just
                // counted.
                if BLANK_RENDER_ONLY.contains(&short) {
                    blank += 1;
                    continue;
                }
                // FormatText: the persisted table only carries a synthetic
                // per-case label (`tmpl:0str`, ...), not the real template
                // text or the real substitution operands — those only exist
                // in `probes/gate_semantics.ws` (not loaded at runtime), so
                // none of these 11 cases are replayable from the table
                // alone. `format_text()` is instead validated directly
                // against the recovered templates in
                // `format_text_matches_recovered_probe_cases` below.
                if gate == FORMAT_TEXT {
                    refused += 1;
                    continue;
                }
                // `deferredOps` chapter: certified but never folded — always
                // refuse, allowlisted wholesale (see the module-level
                // `DEFERRED` list `eval()` itself refuses against).
                if DEFERRED.contains(&short) {
                    refused += 1;
                    continue;
                }
                let inputs: Vec<Option<Value>> =
                    case.inputs.iter().map(case_value).collect();
                if gate == SELECT || gate == BRANCH {
                    // Truthiness gates: recorded output encodes which side won.
                    let want_truthy = match case.output_value.as_str() {
                        "111" | "A" => true,
                        "222" | "B" => false,
                        other => panic!("{gate}: unexpected output {other:?}"),
                    };
                    assert_eq!(truthy(inputs[0].as_ref()), want_truthy,
                        "{gate} {:?}", case.inputs);
                    replayed += 1;
                    continue;
                }
                match eval(gate, &inputs) {
                    Some(v) => {
                        assert_eq!(render(&v), case.output_value,
                            "{gate} {:?} evaluated {v:?}", case.inputs);
                        replayed += 1;
                    }
                    None => {
                        assert!(is_expected_refusal(gate, case),
                            "{gate} {:?}: unexpected refusal", case.inputs);
                        // Every refused MATH-with-string observation recorded
                        // 0 — if a future probe contradicts that, this
                        // screams. The multibyte string refusals carry a
                        // real (non-"0") recorded output since the game DID
                        // compute something for them — we simply decline to
                        // reproduce it (unicode model unconfirmed).
                        if gate.contains("_Math") {
                            assert_eq!(case.output_value, "0");
                        }
                        refused += 1;
                    }
                }
            }
        }
        assert_eq!(replayed + refused + blank, 413, "table case count changed - re-audit");
        assert_eq!(replayed, 379); // +45 v4: 30 extendedMath + 9 bitwise + 6 rounding; +8 v5: int abs/sign*3/negate/min/max/clamp (float output)
        assert_eq!(refused, 28, "3 math-with-string + 11 FormatText + 4 multibyte + 10 deferredOps");
        assert_eq!(blank, 6, "MakeRotation/MakeQuaternion/MakeColor/MakeColorSRGB/MakeColorHex/\
            InvertRotation — blank==blank proves nothing, see BLANK_RENDER_ONLY");
    }

    /// FormatText's real template+operands are unrecoverable from the
    /// persisted table (see the allowlist comment above); recovered instead
    /// by reading `probes/gate_semantics.ws` directly (allowed to READ,
    /// never edit) — every `fmt*` probe mod's exact template literal and
    /// `Opaque(...)` operand, cross-checked against its `tmpl:LABEL` case
    /// output in `data/gate_semantics.json`.
    #[test]
    fn format_text_matches_recovered_probe_cases() {
        let i = |v: Value| Some(v);
        assert_eq!(format_text("{0}", &[i(Value::Str("hi".into()))]), Some("hi".into()));
        assert_eq!(format_text("a{0}b", &[i(Value::Str("X".into()))]), Some("aXb".into()));
        assert_eq!(
            format_text("{0}{1}", &[i(Value::Str("A".into())), i(Value::Str("B".into()))]),
            Some("AB".into())
        );
        assert_eq!(
            format_text("{1}-{0}", &[i(Value::Str("A".into())), i(Value::Str("B".into()))]),
            Some("B-A".into())
        );
        assert_eq!(format_text("{0}", &[i(Value::Int(42))]), Some("42".into()));
        assert_eq!(format_text("{0}", &[i(Value::Float(0.5))]), Some("0.5".into()));
        assert_eq!(format_text("{0}", &[i(Value::Bool(true))]), Some("1".into()));
        assert_eq!(format_text("{0}", &[i(Value::Str("s".into()))]), Some("s".into()));
        assert_eq!(
            format_text("{0}-{1}-{2}", &[
                i(Value::Str("A".into())), i(Value::Str("B".into())), i(Value::Str("C".into()))
            ]),
            Some("A-B-C".into())
        );
        // `Fmt("literal{a}brace")` — `{a}` is not a numbered slot, so it
        // renders "0" like any other unbound tag.
        assert_eq!(format_text("literal{a}brace", &[]), Some("literal0brace".into()));
        // `Fmt("{0}{1}", Opaque(1))` — slot 1 has no operand at all.
        assert_eq!(format_text("{0}{1}", &[i(Value::Int(1))]), Some("10".into()));
    }

    #[test]
    fn render_for_format_matches_certified_table() {
        let t = CertifiedTable::certified();
        let laws = t.render_laws();
        let check = |label: &str, v: Value| {
            assert_eq!(
                render_for_format(&v), laws[label],
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
        check("int:999999", Value::Int(999999));
        check("int:-1000000", Value::Int(-1000000));
        check("int:9007199254740993", Value::Int(9007199254740993));
        check("float:1.0", Value::Float(1.0));
        check("float:-1.0", Value::Float(-1.0));
        check("float:0.5", Value::Float(0.5));
        check("float:1.0/3.0", Value::Float(1.0 / 3.0));
        check("float:0.1+0.2", Value::Float(0.1 + 0.2));
        check("float:2.0/3.0", Value::Float(2.0 / 3.0));
        check("float:123456.789", Value::Float(123456.789));
        check("float:1e-7", Value::Float(1e-7));
        check("float:-0.0", Value::Float(-0.0));
        check("float:1e15", Value::Float(1e15));
        check("float:1.5e-3", Value::Float(1.5e-3));
        check("bool:true", Value::Bool(true));
        check("bool:false", Value::Bool(false));
        check("str:empty", Value::Str(String::new()));
        check("str:a_b", Value::Str("a b".into()));
        check("str:multibyte", Value::Str("π≈3".into()));
        check("vector:Vec(1.0,2.0,3.0)", Value::Vector { x: 1.0, y: 2.0, z: 3.0 });
        check(
            "vector:Vec(0.5,-1.25,1.0/3.0)",
            Value::Vector { x: 0.5, y: -1.25, z: 1.0 / 3.0 },
        );
        check(
            "rotator:Rotation(0.0,90.0,45.5)",
            Value::Rotator { pitch: 0.0, yaw: 90.0, roll: 45.5 },
        );
        check("color:Color(1.0,0.5,0.25)", Value::Color { r: 1.0, g: 0.5, b: 0.25, a: 1.0 });
        check(
            "color:Color(1.0,0.5,0.25,0.5)",
            Value::Color { r: 1.0, g: 0.5, b: 0.25, a: 0.5 },
        );
        check(
            "quat:Quat(0.0,0.0,0.7071067811865476,0.7071067811865476)",
            Value::Quat { x: 0.0, y: 0.0, z: 0.7071067811865476, w: 0.7071067811865476 },
        );
    }

    #[test]
    fn concat_stringifies_bool_natively_not_via_format_law() {
        // Certified: Concatenate's own operand stringification differs from
        // render_for_format's — "true"/"false", not "1"/"0".
        assert_eq!(
            eval(
                "BrickComponentType_WireGraph_Expr_String_Concatenate",
                &[Some(Value::Bool(true)), Some(Value::Str("!".into()))]
            ),
            Some(Value::Str("true!".into()))
        );
    }

    #[test]
    fn multibyte_string_operands_refuse() {
        let len = "BrickComponentType_WireGraph_Expr_String_Length";
        assert!(eval(len, &[Some(Value::Str("π≈3".into()))]).is_none());
    }

    #[test]
    fn oversized_float_refuses_string_fold() {
        let concat = "BrickComponentType_WireGraph_Expr_String_Concatenate";
        // Signature coverage only cares about variant shape, so this is
        // reachable even though no case probed this exact magnitude.
        assert!(string_operands_foldable(&[
            Some(&Value::Str("x".into())),
            Some(&Value::Float(1e16)),
        ]) == false);
        let _ = concat; // documents which gate family this guard protects
    }

    #[test]
    fn oversized_string_result_refuses_fold() {
        let concat = "BrickComponentType_WireGraph_Expr_String_Concatenate";
        // Concatenating two 5000-char strings would produce 10000 chars, exceeding
        // MAX_FOLDED_STRING_LEN (8192), so it must refuse.
        let s5000 = Some(Value::Str("a".repeat(5000)));
        assert!(eval(concat, &[s5000.clone(), s5000.clone()]).is_none(),
            "oversized result (10000 chars) must refuse");
        // Concatenating two 4096-char strings produces 8192 chars, exactly at the
        // limit, so it should fold.
        let s4096 = Some(Value::Str("a".repeat(4096)));
        assert_eq!(eval(concat, &[s4096.clone(), s4096.clone()]),
            Some(Value::Str("a".repeat(8192))),
            "result at limit (8192 chars) must fold");
    }

    #[test]
    fn deferred_ops_always_refuse() {
        for short in DEFERRED {
            let gate = format!("BrickComponentType_WireGraph_Expr_{short}");
            let t = CertifiedTable::certified();
            for case in t.cases(&gate) {
                let inputs: Vec<Option<Value>> = case.inputs.iter().map(case_value).collect();
                assert!(eval(&gate, &inputs).is_none(), "{gate} must always refuse");
            }
        }
    }

    /// Mirrors `deferred_ops_always_refuse`: `MakeRotation`/`MakeColorSRGB`/
    /// `MakeColorHex`/`InvertRotation` are hard-refused by `eval()`'s
    /// `BLANK_RENDER_REFUSED` list regardless of signature coverage (their
    /// only table evidence is a case whose output renders blank — see
    /// `BLANK_RENDER_REFUSED`'s doc comment) — replayed here on their own
    /// certified/covered signatures to lock that `eval()` never silently
    /// starts folding them.
    #[test]
    fn blank_render_gates_always_refuse() {
        for short in BLANK_RENDER_REFUSED {
            let gate = format!("BrickComponentType_WireGraph_Expr_{short}");
            let t = CertifiedTable::certified();
            let cases = t.cases(&gate);
            assert!(!cases.is_empty(), "{gate}: expected at least one certified case");
            for case in cases {
                let inputs: Vec<Option<Value>> = case.inputs.iter().map(case_value).collect();
                assert!(eval(&gate, &inputs).is_none(), "{gate} must always refuse");
            }
        }
    }

    #[test]
    fn rotate_vector_replays_the_45_degree_case_exactly() {
        let gate = "BrickComponentType_WireGraph_Expr_RotateVector";
        let v = Some(Value::Vector { x: 1.0, y: 0.0, z: 0.0 });
        let q = Some(Value::Quat {
            x: 0.0, y: 0.0, z: 0.3826834323650898, w: 0.9238795325112867,
        });
        let got = eval(gate, &[v, q]).expect("covered signature must fold");
        assert_eq!(render(&got), "X=0.707 Y=0.707 Z=0.000");
    }

    #[test]
    fn refusal_overflow_and_mixed_sign_div() {
        let add = "BrickComponentType_WireGraph_Expr_MathAdd";
        let div = "BrickComponentType_WireGraph_Expr_MathDivide";
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let i = |n: i64| Some(Value::Int(n));
        assert!(eval(add, &[i(i64::MAX), i(1)]).is_none(), "overflow refuses");
        assert!(eval(div, &[i(-7), i(2)]).is_none(), "trunc-vs-floor unprobed");
        assert!(eval(div, &[i(-4), i(2)]).is_some(), "zero remainder is safe");
        assert!(eval(md, &[i(-7), i(2)]).is_none());
        assert_eq!(eval(div, &[i(7), i(0)]), Some(Value::Int(0)), "div0 certified");
    }

    #[test]
    fn overflow_min_div_neg_one_refuses_no_panic() {
        let div = "BrickComponentType_WireGraph_Expr_MathDivide";
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let i = |n: i64| Some(Value::Int(n));
        // i64::MIN / -1 (and MIN % -1) overflow i64 unconditionally in Rust;
        // must refuse via checked ops rather than panic.
        assert!(eval(div, &[i(i64::MIN), i(-1)]).is_none());
        assert!(eval(md, &[i(i64::MIN), i(-1)]).is_none());
    }

    #[test]
    fn float_modulo_mixed_sign_refuses() {
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let f = |n: f64| Some(Value::Float(n));
        assert!(eval(md, &[f(-3.5), f(2.0)]).is_none());
    }

    #[test]
    fn composite_modulo_mixed_sign_does_not_refuse() {
        // Certified deviation from the scalar path: compositeMath's Modulo
        // computes unconditionally (Rust `%`, truncated remainder), even
        // mixed-sign — `Vec(0.5,0.25,-0.75) % Vec(0.25,0.5,0.75) -> Z=-0.000`.
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let v1 = Some(Value::Vector { x: 0.5, y: 0.25, z: -0.75 });
        let v2 = Some(Value::Vector { x: 0.25, y: 0.5, z: 0.75 });
        let got = eval(md, &[v1, v2]).expect("composite modulo must not refuse on mixed sign");
        assert_eq!(render(&got), "X=0.000 Y=0.250 Z=-0.000");
    }

    #[test]
    fn uncovered_signatures_refuse() {
        let ne = "BrickComponentType_WireGraph_Expr_CompareNotEqual";
        let eq_gate = "BrickComponentType_WireGraph_Expr_CompareEqual";
        let lt = "BrickComponentType_WireGraph_Expr_CompareLess";
        let not = "BrickComponentType_WireGraph_Expr_LogicalNOT";
        // (Str, Unwired): CompareNotEqual was only probed at (int,int)/(int,str).
        assert!(eval(ne, &[Some(Value::Str("x".into())), None]).is_none());
        // Reverse of the probed (int,str) direction — (str,int) unprobed.
        assert!(eval(eq_gate,
            &[Some(Value::Str("1".into())), Some(Value::Int(1))]).is_none());
        // Bool never appears as the second operand of an ordered compare.
        assert!(eval(lt, &[Some(Value::Int(1)), Some(Value::Bool(true))]).is_none());
        // NOT was only ever probed on Bool.
        assert!(eval(not, &[Some(Value::Int(1))]).is_none());
    }

    #[test]
    fn covered_signature_still_folds() {
        let eq_gate = "BrickComponentType_WireGraph_Expr_CompareEqual";
        assert_eq!(
            eval(eq_gate, &[Some(Value::Int(1)), Some(Value::Str("1".into()))]),
            Some(Value::Bool(true))
        );
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
