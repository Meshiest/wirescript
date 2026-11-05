//! The order a certified gate's inputs reach the evaluator: the order its law
//! names its operands (`Substring(s, start, length)`), which the compiler's
//! call specs use too. The inventory lists ports alphabetically, so a gate
//! whose order differs from that is listed here.

use crate::graph::Gate;

const ORDERS: &[(&str, &[&str])] = &[
    ("Expr_String_Concatenate", &["InputA", "InputB"]),
    ("Expr_String_Contains", &["Input", "Search", "bCaseSensitive"]),
    ("Expr_String_StartsWith", &["Input", "Prefix", "bCaseSensitive"]),
    ("Expr_String_EndsWith", &["Input", "Suffix", "bCaseSensitive"]),
    ("Expr_String_Substring", &["Input", "Start", "Length"]),
    ("Expr_String_Find", &["Input", "Search", "bCaseSensitive", "Start"]),
    (
        "Expr_String_Replace",
        &["Input", "Search", "Replacement", "bCaseSensitive", "MaxReplacements", "Start"],
    ),
    ("Expr_MakeRotation", &["Pitch", "Yaw", "Roll"]),
    ("Expr_MakeQuaternion", &["X", "Y", "Z", "W"]),
    ("Expr_MakeColor", &["R", "G", "B", "A"]),
    ("Expr_MakeColorSRGB", &["R", "G", "B", "A"]),
    ("Expr_RotateVector", &["Vector", "Rotation"]),
    ("Expr_QuatSlerp", &["InputA", "InputB", "Alpha"]),
    ("Expr_QuatFromAxisAngle", &["Axis", "Angle"]),
    ("Expr_QuatBetween", &["From", "To"]),
    ("Expr_ColorBlend", &["ColorA", "ColorB", "Alpha"]),
    ("Expr_MathAtan2", &["Y", "X"]),
    ("Expr_MathPow", &["Input", "Exponent"]),
    ("Expr_MathLogBase", &["Input", "Base"]),
    ("Expr_MathClamp", &["Input", "Min", "Max"]),
];

/// The input ports of certified gate `gate`, in evaluator order.
pub fn eval_ports(gate: &Gate) -> Vec<String> {
    let short = gate
        .class
        .strip_prefix("BrickComponentType_")
        .unwrap_or(&gate.class);
    if let Some((_, order)) = ORDERS.iter().find(|(c, _)| *c == short) {
        return order.iter().map(|s| (*s).to_string()).collect();
    }
    gate.inputs
        .iter()
        .filter(|p| !p.exec_pin)
        .map(|p| p.name.clone())
        .collect()
}
