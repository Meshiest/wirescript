    use super::*;

    #[test]
    fn loads_real_table_v4() {
        let t = CertifiedTable::certified();
        assert_eq!(t.gate_classes().count(), 95);
        assert_eq!(t.render_laws().len(), 32);
        // v4: extendedMath / bitwise / rounding chapters.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathSin", &[InVariant::Float]));
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathClamp",
            &[InVariant::Float, InVariant::Float, InVariant::Float]));
        assert!(t.covers("BrickComponentType_WireGraph_Expr_BitwiseAND",
            &[InVariant::Int, InVariant::Int]));
        assert!(t.covers("BrickComponentType_WireGraph_Expr_Round", &[InVariant::Float]));
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathAdd",
            &[InVariant::Int, InVariant::Int]));
        // Probed one direction only — the reverse signature must NOT be covered.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_CompareEqual",
            &[InVariant::Int, InVariant::Str]));
        assert!(!t.covers("BrickComponentType_WireGraph_Expr_CompareEqual",
            &[InVariant::Str, InVariant::Int]));
        // Unwired is part of the signature.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathAdd",
            &[InVariant::Int, InVariant::Unwired]));
        // Select/Branch signatures are 1-ary (condition only).
        assert!(t.covers("BrickComponentType_WireGraph_Expr_Select", &[InVariant::Str]));
        assert!(t.covers("BrickComponentType_WireGraph_Exec_Branch", &[InVariant::Unwired]));
        // v3: composite math is a SHARED signature with the scalar chapter —
        // MathAdd covers both, keyed purely by operand variant.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathAdd",
            &[InVariant::Vector, InVariant::Vector]));
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathAdd",
            &[InVariant::Vector, InVariant::Float]));
        // v3: composite compare.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_CompareEqual",
            &[InVariant::Quat, InVariant::Quat]));
        // v3: FormatText's only recorded signature is the synthetic Tmpl
        // marker — never producible by a real Value, so never coverable by a
        // live fold query (see `InVariant::Tmpl`'s doc comment).
        assert!(t.covers("BrickComponentType_WireGraph_Expr_String_FormatText",
            &[InVariant::Tmpl]));
    }

    #[test]
    fn annihilators_are_exactly_and_false_or_true() {
        let t = CertifiedTable::certified();
        assert!(matches!(
            t.annihilator("BrickComponentType_WireGraph_Expr_LogicalAND"),
            Some(AnnihilatorKind::AndFalse)));
        assert!(matches!(
            t.annihilator("BrickComponentType_WireGraph_Expr_LogicalOR"),
            Some(AnnihilatorKind::OrTrue)));
        assert!(t.annihilator("BrickComponentType_WireGraph_Expr_LogicalXOR").is_none());
        assert!(t.annihilator("BrickComponentType_WireGraph_Expr_MathAdd").is_none());
    }

    #[test]
    fn unwired_case_inputs_parse_as_unwired() {
        let t = CertifiedTable::certified();
        let cases = t.cases("BrickComponentType_WireGraph_Expr_CompareEqual");
        assert!(cases.iter().any(|c| c.inputs.len() == 2
            && c.inputs[1].variant == InVariant::Unwired
            && c.inputs[1].value.is_none()));
    }

    #[test]
    fn composite_operands_parse_structurally() {
        let t = CertifiedTable::certified();
        let cases = t.cases("BrickComponentType_WireGraph_Expr_VecCrossProduct");
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].inputs[0].variant, InVariant::Vector);
        assert_eq!(
            cases[0].inputs[0].value,
            Some(CaseValue::Vector { x: 0.5, y: 0.25, z: -0.75 })
        );
        let quat_cases = t.cases("BrickComponentType_WireGraph_Expr_QuatDotProduct");
        assert_eq!(
            quat_cases[0].inputs[0].value,
            Some(CaseValue::Quat {
                x: 0.0, y: 0.0, z: 0.7071067811865476, w: 0.7071067811865476
            })
        );
        // 3-arg Color(...) defaults alpha to 1.0.
        let hex_cases = t.cases("BrickComponentType_WireGraph_Expr_ColorToHex");
        assert_eq!(
            hex_cases[0].inputs[0].value,
            Some(CaseValue::Color { r: 1.0, g: 0.5, b: 0.0, a: 1.0 })
        );
    }

    #[test]
    fn render_laws_hold_the_certified_calibration_entries() {
        let t = CertifiedTable::certified();
        assert_eq!(t.render_laws().get("int:1000").map(String::as_str), Some("1,000"));
        assert_eq!(t.render_laws().get("bool:true").map(String::as_str), Some("1"));
        assert_eq!(t.render_laws().get("bool:false").map(String::as_str), Some("0"));
        assert_eq!(
            t.render_laws().get("vector:Vec(1.0,2.0,3.0)").map(String::as_str),
            Some("X=1.000 Y=2.000 Z=3.000")
        );
        // rotator/color/quat never render through FormatText — certified blank.
        assert_eq!(
            t.render_laws().get("rotator:Rotation(0.0,90.0,45.5)").map(String::as_str),
            Some("")
        );
    }
