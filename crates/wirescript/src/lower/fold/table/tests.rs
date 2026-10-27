use super::*;

#[test]
fn loads_real_table_v4() {
    let t = CertifiedTable::certified();
    assert_eq!(t.gate_classes().count(), 95);
    assert_eq!(t.render_laws().len(), 32);
    // extendedMath, bitwise and rounding chapters.
    assert!(t.covers("BrickComponentType_Expr_MathSin", &[InVariant::Float]));
    assert!(t.covers(
        "BrickComponentType_Expr_MathClamp",
        &[InVariant::Float, InVariant::Float, InVariant::Float]
    ));
    assert!(t.covers(
        "BrickComponentType_Expr_BitwiseAND",
        &[InVariant::Int, InVariant::Int]
    ));
    assert!(t.covers("BrickComponentType_Expr_Round", &[InVariant::Float]));
    assert!(t.covers(
        "BrickComponentType_Expr_MathAdd",
        &[InVariant::Int, InVariant::Int]
    ));
    // Probed in one direction only, so the reverse signature is not covered.
    assert!(t.covers(
        "BrickComponentType_Expr_CompareEqual",
        &[InVariant::Int, InVariant::Str]
    ));
    assert!(!t.covers(
        "BrickComponentType_Expr_CompareEqual",
        &[InVariant::Str, InVariant::Int]
    ));
    // Unwired is part of the signature.
    assert!(t.covers(
        "BrickComponentType_Expr_MathAdd",
        &[InVariant::Int, InVariant::Unwired]
    ));
    // Select/Branch signatures are 1-ary (condition only).
    assert!(t.covers("BrickComponentType_Expr_Select", &[InVariant::Str]));
    assert!(t.covers("BrickComponentType_Exec_Branch", &[InVariant::Unwired]));
    // Composite and scalar math share one gate class, keyed by operand variant.
    assert!(t.covers(
        "BrickComponentType_Expr_MathAdd",
        &[InVariant::Vector, InVariant::Vector]
    ));
    assert!(t.covers(
        "BrickComponentType_Expr_MathAdd",
        &[InVariant::Vector, InVariant::Float]
    ));
    // Composite compare.
    assert!(t.covers(
        "BrickComponentType_Expr_CompareEqual",
        &[InVariant::Quat, InVariant::Quat]
    ));
    // FormatText's only recorded signature is the synthetic Tmpl marker,
    // which no real Value produces.
    assert!(t.covers(
        "BrickComponentType_Expr_String_FormatText",
        &[InVariant::Tmpl]
    ));
}

#[test]
fn annihilators_are_exactly_and_false_or_true() {
    let t = CertifiedTable::certified();
    assert!(matches!(
        t.annihilator("BrickComponentType_Expr_LogicalAND"),
        Some(AnnihilatorKind::AndFalse)
    ));
    assert!(matches!(
        t.annihilator("BrickComponentType_Expr_LogicalOR"),
        Some(AnnihilatorKind::OrTrue)
    ));
    assert!(
        t.annihilator("BrickComponentType_Expr_LogicalXOR")
            .is_none()
    );
    assert!(t.annihilator("BrickComponentType_Expr_MathAdd").is_none());
}

#[test]
fn unwired_case_inputs_parse_as_unwired() {
    let t = CertifiedTable::certified();
    let cases = t.cases("BrickComponentType_Expr_CompareEqual");
    assert!(cases.iter().any(|c| c.inputs.len() == 2
        && c.inputs[1].variant == InVariant::Unwired
        && c.inputs[1].value.is_none()));
}

#[test]
fn composite_operands_parse_structurally() {
    let t = CertifiedTable::certified();
    let cases = t.cases("BrickComponentType_Expr_VecCrossProduct");
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].inputs[0].variant, InVariant::Vector);
    assert_eq!(
        cases[0].inputs[0].value,
        Some(CaseValue::Vector {
            x: 0.5,
            y: 0.25,
            z: -0.75
        })
    );
    let quat_cases = t.cases("BrickComponentType_Expr_QuatDotProduct");
    // The probe writes 0.7071067811865476, the shortest round-trip spelling
    // of FRAC_1_SQRT_2; both name the same f64.
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(
        quat_cases[0].inputs[0].value,
        Some(CaseValue::Quat {
            x: 0.0,
            y: 0.0,
            z: h,
            w: h
        })
    );
    // 3-arg Color(...) defaults alpha to 1.0.
    let hex_cases = t.cases("BrickComponentType_Expr_ColorToHex");
    assert_eq!(
        hex_cases[0].inputs[0].value,
        Some(CaseValue::Color {
            r: 1.0,
            g: 0.5,
            b: 0.0,
            a: 1.0
        })
    );
}

#[test]
fn render_laws_hold_the_certified_calibration_entries() {
    let t = CertifiedTable::certified();
    assert_eq!(
        t.render_laws().get("int:1000").map(String::as_str),
        Some("1,000")
    );
    assert_eq!(
        t.render_laws().get("bool:true").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        t.render_laws().get("bool:false").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        t.render_laws()
            .get("vector:Vec(1.0,2.0,3.0)")
            .map(String::as_str),
        Some("X=1.000 Y=2.000 Z=3.000")
    );
    // Certified: rotator, color and quat render blank through FormatText.
    assert_eq!(
        t.render_laws()
            .get("rotator:Rotation(0.0,90.0,45.5)")
            .map(String::as_str),
        Some("")
    );
}
