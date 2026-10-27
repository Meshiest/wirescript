//! Certified evaluator: implements exactly the laws the in-game probe
//! certified (data/gate_semantics.json). `None` means refuse to fold. The
//! replay test in `eval/tests.rs` checks every table case against this code.
use std::fmt::Write as _;

use crate::ir::Literal;
use crate::ir::port_registry::WirePort;
use crate::lower::fold::table::InVariant;

// `pub` for the `--fold-diff` fuzz harness (via the `fold_eval` re-export).
// `variant()` stays `pub(crate)` because it returns the crate-private
// `InVariant`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Vector {
        x: f64,
        y: f64,
        z: f64,
    },
    Rotator {
        pitch: f64,
        yaw: f64,
        roll: f64,
    },
    Quat {
        x: f64,
        y: f64,
        z: f64,
        w: f64,
    },
    /// Linear-space RGBA (0-1 components), the wire `LinearColor` variant.
    /// `ColorToHex` converts from it at a certified sRGB gamma boundary; the
    /// inverse curve (`MakeColorSRGB`/`MakeColorHex`) is unprobed and refused.
    Color {
        r: f64,
        g: f64,
        b: f64,
        a: f64,
    },
}

impl Value {
    pub(crate) fn variant(&self) -> InVariant {
        match self {
            Self::Int(_) => InVariant::Int,
            Self::Float(_) => InVariant::Float,
            Self::Bool(_) => InVariant::Bool,
            Self::Str(_) => InVariant::Str,
            Self::Vector { .. } => InVariant::Vector,
            Self::Rotator { .. } => InVariant::Rotator,
            Self::Quat { .. } => InVariant::Quat,
            Self::Color { .. } => InVariant::Color,
        }
    }
    pub fn from_literal(lit: &Literal) -> Option<Self> {
        match lit {
            Literal::Int(n) => Some(Self::Int(*n)),
            Literal::Float(f) => Some(Self::Float(*f)),
            Literal::Bool(b) => Some(Self::Bool(*b)),
            Literal::String(s) => Some(Self::Str(s.clone())),
            Literal::Vector { x, y, z } => Some(Self::Vector {
                x: *x,
                y: *y,
                z: *z,
            }),
            Literal::Rotator { pitch, yaw, roll } => Some(Self::Rotator {
                pitch: *pitch,
                yaw: *yaw,
                roll: *roll,
            }),
            Literal::Quat { x, y, z, w } => Some(Self::Quat {
                x: *x,
                y: *y,
                z: *z,
                w: *w,
            }),
            // `Literal::LinearColor` is the fold pass's own Color shape.
            Literal::LinearColor { r, g, b, a } => Some(Self::Color {
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            }),
            // `Literal::Color` (u8 brick paint) becomes a wire value by plain
            // `/255.0` in emit (`wirescript-ir` `emit/variants.rs`), not the
            // gamma curve; folding must match it or the color would change.
            Literal::Color { r, g, b, a } => Some(Self::Color {
                r: f64::from(*r) / 255.0,
                g: f64::from(*g) / 255.0,
                b: f64::from(*b) / 255.0,
                a: f64::from(*a) / 255.0,
            }),
            _ => None, // object/array/asset/prefab: not certified
        }
    }
    pub fn to_literal(&self) -> Literal {
        match self {
            Self::Int(n) => Literal::Int(*n),
            Self::Float(f) => Literal::Float(*f),
            Self::Bool(b) => Literal::Bool(*b),
            Self::Str(s) => Literal::String(s.clone()),
            Self::Vector { x, y, z } => Literal::Vector {
                x: *x,
                y: *y,
                z: *z,
            },
            Self::Rotator { pitch, yaw, roll } => Literal::Rotator {
                pitch: *pitch,
                yaw: *yaw,
                roll: *roll,
            },
            Self::Quat { x, y, z, w } => Literal::Quat {
                x: *x,
                y: *y,
                z: *z,
                w: *w,
            },
            // Always the exact linear shape, never the lossy u8 `Literal::Color`.
            Self::Color { r, g, b, a } => Literal::LinearColor {
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
        }
    }
}

// Gate short names: the suffix after the last `_` of a full
// `BrickComponentType_Expr_...` class, which `eval()` matches on. Shared by
// the match arms, the refusal lists and the tests so they cannot desync.
const G_COMPARE_EQUAL: &str = "CompareEqual";
const G_COMPARE_NOT_EQUAL: &str = "CompareNotEqual";
const G_COMPARE_LESS: &str = "CompareLess";
const G_COMPARE_LESS_OR_EQUAL: &str = "CompareLessOrEqual";
const G_COMPARE_GREATER: &str = "CompareGreater";
const G_COMPARE_GREATER_OR_EQUAL: &str = "CompareGreaterOrEqual";
const G_LOGICAL_AND: &str = "LogicalAND";
const G_LOGICAL_OR: &str = "LogicalOR";
const G_LOGICAL_XOR: &str = "LogicalXOR";
const G_LOGICAL_NOT: &str = "LogicalNOT";
const G_CONCATENATE: &str = "Concatenate";
const G_LENGTH: &str = "Length";
const G_TO_LOWER: &str = "ToLower";
const G_TO_UPPER: &str = "ToUpper";
const G_TRIM: &str = "Trim";
const G_CONTAINS: &str = "Contains";
const G_STARTS_WITH: &str = "StartsWith";
const G_ENDS_WITH: &str = "EndsWith";
const G_SUBSTRING: &str = "Substring";
const G_FIND: &str = "Find";
const G_REPLACE: &str = "Replace";
const G_PARSE_INT: &str = "ParseInt";
const G_PARSE_NUMBER: &str = "ParseNumber";
const G_MAKE_VECTOR: &str = "MakeVector";
const G_MAKE_ROTATION: &str = "MakeRotation";
const G_MAKE_QUATERNION: &str = "MakeQuaternion";
const G_MAKE_COLOR: &str = "MakeColor";
const G_MAKE_COLOR_SRGB: &str = "MakeColorSRGB";
const G_MAKE_COLOR_HEX: &str = "MakeColorHex";
const G_COLOR_TO_HEX: &str = "ColorToHex";
const G_VEC_SCALE: &str = "VecScale";
const G_VEC_DOT_PRODUCT: &str = "VecDotProduct";
const G_VEC_CROSS_PRODUCT: &str = "VecCrossProduct";
const G_VEC_MAGNITUDE_SQUARED: &str = "VecMagnitudeSquared";
const G_VEC_DISTANCE_SQUARED: &str = "VecDistanceSquared";
const G_ROTATE_VECTOR: &str = "RotateVector";
const G_INVERT_ROTATION: &str = "InvertRotation";
const G_QUAT_DOT_PRODUCT: &str = "QuatDotProduct";

// Deferred-ops family: probed but never folded (see `DEFERRED`).
const G_VEC_MAGNITUDE: &str = "VecMagnitude";
const G_VEC_NORMALIZE: &str = "VecNormalize";
const G_VEC_DISTANCE: &str = "VecDistance";
const G_QUAT_SLERP: &str = "QuatSlerp";
const G_QUAT_FROM_AXIS_ANGLE: &str = "QuatFromAxisAngle";
const G_QUAT_ANGLE_BETWEEN: &str = "QuatAngleBetween";
const G_QUAT_BETWEEN: &str = "QuatBetween";
const G_DIRECTION_TO_ROTATION: &str = "DirectionToRotation";
const G_ROTATION_TO_DIRECTION: &str = "RotationToDirection";
const G_COLOR_BLEND: &str = "ColorBlend";

// Math* family: matched by suffix, since `component_op` and `math` receive
// the full gate class string.
const G_MATH_ADD: &str = "MathAdd";
const G_MATH_SUBTRACT: &str = "MathSubtract";
const G_MATH_MULTIPLY: &str = "MathMultiply";
const G_MATH_DIVIDE: &str = "MathDivide";
const G_MATH_MODULO: &str = "MathModulo";

/// `deferredOps` chapter: probed but always refused regardless of coverage,
/// because exact slerp, normalize, axis-angle and blend math is not
/// implemented. Module-level so the tests assert against this same list.
const DEFERRED: &[&str] = &[
    G_VEC_MAGNITUDE,
    G_VEC_NORMALIZE,
    G_VEC_DISTANCE,
    G_QUAT_SLERP,
    G_QUAT_FROM_AXIS_ANGLE,
    G_QUAT_ANGLE_BETWEEN,
    G_QUAT_BETWEEN,
    G_DIRECTION_TO_ROTATION,
    G_ROTATION_TO_DIRECTION,
    G_COLOR_BLEND,
];

/// Always refused: each gate's only evidence is a case whose output renders
/// blank (rotator/quat/color never render through `FormatText`), which proves
/// nothing about the value. Certifying one needs a probe that chains it
/// through a rendering gate, as `MakeQuaternion` and 3-arg `MakeColor` are.
const BLANK_RENDER_REFUSED: &[&str] = &[
    G_MAKE_ROTATION,
    G_MAKE_COLOR_SRGB,
    G_MAKE_COLOR_HEX,
    G_INVERT_ROTATION,
];

/// Certified: non-finite floats sanitize to 0 at gate INPUTS.
pub(crate) fn sanitize(f: f64) -> f64 {
    if f.is_finite() { f } else { 0.0 }
}

/// Certified truthiness (Select/Branch chapters): int nonzero; sanitized
/// float nonzero; string not in {"", "0", "false"}; bool as-is; unwired falsy.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Int(n)) => *n != 0,
        Some(Value::Float(f)) => sanitize(*f) != 0.0,
        Some(Value::Bool(b)) => *b,
        Some(Value::Str(s)) => !matches!(s.as_str(), "" | "0" | "false"),
        // Unprobed (the Select/Branch chapters are scalar-only) and not a
        // certified condition signature; present so the match stays total.
        Some(
            Value::Vector { .. } | Value::Rotator { .. } | Value::Quat { .. } | Value::Color { .. },
        ) => false,
    }
}

// Rendering: two laws. `render_for_format` is what `FormatText` prints;
// `render` matches the persisted case text, which `gen_semantics.mjs`
// canonicalizes (bools to "true"/"false", commas stripped from ints).

/// Round to 3 decimals and split into (negative, integer digits, trimmed
/// fraction), the numeric law both renderers share. Ties round to even, as
/// certified (`-0.3125 -> -0.312`, `1.5e-3 -> 0.002`).
fn round3(f: f64) -> (bool, String, String) {
    let f = sanitize(f);
    let fixed = format!("{f:.3}");
    let (neg, digits) = match fixed.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, fixed.as_str()),
    };
    let (int_part, frac_part) = digits
        .split_once('.')
        .expect("format!(\"{:.3}\", _) always emits a decimal point");
    let trimmed_frac = frac_part.trim_end_matches('0');
    // A negative that rounds to zero drops its sign: certified for -0.0
    // ("float:-0.0" -> "0"), assumed for small negatives. Vector components
    // keep the sign (see `fixed3`).
    let neg = neg && !(int_part == "0" && trimmed_frac.is_empty());
    (neg, int_part.to_string(), trimmed_frac.to_string())
}

fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        let from_end = bytes.len() - i;
        if i > 0 && from_end.is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// One composite component as Unreal's `FVector::ToString` prints it: fixed
/// 3 decimals, no grouping or trimming (`X=1.500 Y=-2.500 Z=0.750`), and a
/// near-zero negative keeps its sign (`X=-0.000`). Certified.
fn fixed3(f: f64) -> String {
    format!("{:.3}", sanitize(f))
}

/// What `FormatText` prints for a wire value, certified by the `render`
/// section of `data/gate_semantics.json`. Ints group thousands with commas
/// (`-1,000,000`). Floats round to 3 decimals, group the integer part and
/// drop trailing zeros; non-finite and -0.0 print "0". Bools print "1"/"0".
/// Vectors print `X=%.3f Y=%.3f Z=%.3f`; rotator, quat and color print blank.
pub(crate) fn render_for_format(v: &Value) -> String {
    match v {
        Value::Int(n) => {
            let (neg, digits) = match n.checked_abs() {
                Some(a) => (*n < 0, a.to_string()),
                // i64::MIN has no positive i64 abs.
                None => (true, i128::from(*n).unsigned_abs().to_string()),
            };
            let grouped = group_thousands(&digits);
            if neg { format!("-{grouped}") } else { grouped }
        }
        Value::Float(f) => {
            let (neg, int_part, frac) = round3(*f);
            let grouped = group_thousands(&int_part);
            let sign = if neg { "-" } else { "" };
            if frac.is_empty() {
                format!("{sign}{grouped}")
            } else {
                format!("{sign}{grouped}.{frac}")
            }
        }
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Str(s) => s.clone(),
        Value::Vector { x, y, z } => format!("X={} Y={} Z={}", fixed3(*x), fixed3(*y), fixed3(*z)),
        Value::Rotator { .. } | Value::Quat { .. } | Value::Color { .. } => String::new(),
    }
}

/// The persisted case text the replay test compares `eval()` against: bools
/// as "true"/"false" and ints ungrouped (as `gen_semantics.mjs` stores them);
/// every other value as `render_for_format` prints it.
#[cfg(test)]
pub(crate) fn render(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Float(f) => {
            // Stored float text is verbatim `FormatText` output, grouped
            // (`pow(2,10) -> 1,024`).
            let (neg, int_part, frac) = round3(*f);
            let grouped = group_thousands(&int_part);
            let sign = if neg { "-" } else { "" };
            if frac.is_empty() {
                format!("{sign}{grouped}")
            } else {
                format!("{sign}{grouped}.{frac}")
            }
        }
        _ => render_for_format(v),
    }
}

/// `Concatenate`'s operand-to-text law, certified to differ from both
/// renderers: a bool prints "true"/"false" (`true .. "!"` is "true!").
/// Ints and floats print plain and ungrouped, which is assumed: no probe
/// covered a float over 3 decimals or an int over 999.
fn concat_display(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
            if !f.is_finite() || *f == 0.0 {
                "0".to_string()
            } else {
                format!("{f}")
            }
        }
        Value::Str(s) => s.clone(),
        _ => render_for_format(v), // composites: never probed in Concatenate; same law as elsewhere
    }
}

/// i64 -> f64 is exact only within +/-2^53; beyond that, mixed-domain numeric
/// work refuses (never probed at that magnitude).
fn int_as_f64(n: i64) -> Option<f64> {
    const LIMIT: i64 = 1 << 53;
    (-LIMIT..=LIMIT).contains(&n).then_some(n as f64)
}

/// Numeric coercion for ORDERED compares only (certified parse-or-zero,
/// incl. str: "10" vs "9" compares 10 > 9; "a" -> 0).
fn ord_num(v: Option<&Value>) -> Option<f64> {
    Some(match v {
        None => 0.0,
        Some(Value::Int(n)) => int_as_f64(*n)?,
        Some(Value::Float(f)) => sanitize(*f),
        Some(Value::Bool(b)) => i64::from(*b) as f64,
        Some(Value::Str(s)) => s.parse::<f64>().map(sanitize).unwrap_or(0.0),
        Some(
            Value::Vector { .. } | Value::Rotator { .. } | Value::Quat { .. } | Value::Color { .. },
        ) => return None, // never probed
    })
}

/// EQ/NE, certified per variant pair: int vs str compares the int's
/// canonical text; str-str exact; int/float/bool mixes numerically; same-type
/// composites component-wise. Unprobed pairs return `None`.
fn eq(a: Option<&Value>, b: Option<&Value>) -> Option<bool> {
    use Value::{Bool, Color, Float, Int, Quat, Rotator, Str, Vector};
    // Certified: unwired acts as the other operand's domain default. The
    // composite defaults are never a certified signature; they keep the
    // match total.
    let default_for = |other: &Value| -> Value {
        match other {
            Int(_) => Int(0),
            Float(_) => Float(0.0),
            Bool(_) => Bool(false),
            Str(_) => Str(String::new()),
            Vector { .. } => Vector {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            Rotator { .. } => Rotator {
                pitch: 0.0,
                yaw: 0.0,
                roll: 0.0,
            },
            Quat { .. } => Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            },
            Color { .. } => Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            },
        }
    };
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        (Some(a), None) => {
            let d = default_for(a);
            (a.clone(), d)
        }
        (None, Some(b)) => {
            let d = default_for(b);
            (d, b.clone())
        }
        (None, None) => return None, // not in table
    };
    Some(match (&a, &b) {
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) => sanitize(*x) == sanitize(*y),
        (Int(x), Float(y)) | (Float(y), Int(x)) => int_as_f64(*x)? == sanitize(*y),
        (Int(x), Bool(y)) | (Bool(y), Int(x)) => *x == i64::from(*y),
        (Bool(x), Bool(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        // Certified (int:N str:S direction): canonical int rendering compared
        // as a string ("1"=="1" T, ""=="0"'s int-0 F, "1.0" F).
        (Int(x), Str(s)) | (Str(s), Int(x)) => &x.to_string() == s,
        (
            Vector {
                x: x1,
                y: y1,
                z: z1,
            },
            Vector {
                x: x2,
                y: y2,
                z: z2,
            },
        ) => {
            sanitize(*x1) == sanitize(*x2)
                && sanitize(*y1) == sanitize(*y2)
                && sanitize(*z1) == sanitize(*z2)
        }
        (
            Rotator {
                pitch: p1,
                yaw: y1,
                roll: r1,
            },
            Rotator {
                pitch: p2,
                yaw: y2,
                roll: r2,
            },
        ) => {
            sanitize(*p1) == sanitize(*p2)
                && sanitize(*y1) == sanitize(*y2)
                && sanitize(*r1) == sanitize(*r2)
        }
        (
            Quat {
                x: x1,
                y: y1,
                z: z1,
                w: w1,
            },
            Quat {
                x: x2,
                y: y2,
                z: z2,
                w: w2,
            },
        ) => {
            sanitize(*x1) == sanitize(*x2)
                && sanitize(*y1) == sanitize(*y2)
                && sanitize(*z1) == sanitize(*z2)
                && sanitize(*w1) == sanitize(*w2)
        }
        (
            Color {
                r: r1,
                g: g1,
                b: b1,
                a: a1,
            },
            Color {
                r: r2,
                g: g2,
                b: b2,
                a: a2,
            },
        ) => {
            sanitize(*r1) == sanitize(*r2)
                && sanitize(*g1) == sanitize(*g2)
                && sanitize(*b1) == sanitize(*b2)
                && sanitize(*a1) == sanitize(*a2)
        }
        _ => return None, // float-str / bool-str / float-bool / cross-composite: unprobed
    })
}

fn cmp(a: Option<&Value>, b: Option<&Value>) -> Option<std::cmp::Ordering> {
    // Pure-int stays in i64 (64-bit compares are certified past 2^53).
    if let (Some(Value::Int(x)), Some(Value::Int(y))) = (a, b) {
        return Some(x.cmp(y));
    }
    ord_num(a)?.partial_cmp(&ord_num(b)?)
}

/// Math domain: Int/Bool/unwired stay in exact i64 (checked); any Float
/// switches to f64 with input sanitize. Strings refuse (certified outputs
/// contradict every parse model: "a"+1 recorded 0, not 1).
enum MathIn {
    I(i64),
    F(f64),
}
fn math_in(v: Option<&Value>) -> Option<MathIn> {
    Some(match v {
        None => MathIn::I(0),
        Some(Value::Int(n)) => MathIn::I(*n),
        Some(Value::Bool(b)) => MathIn::I(i64::from(*b)),
        Some(Value::Float(f)) => MathIn::F(sanitize(*f)),
        Some(Value::Str(_)) => return None,
        Some(
            Value::Vector { .. } | Value::Rotator { .. } | Value::Quat { .. } | Value::Color { .. },
        ) => return None, // routed to composite_math instead
    })
}

/// Scalar partner of a `[Vector, X]`/`[X, Vector]` broadcast. Certified for
/// float and int; bool and unwired follow `math_in`'s promotion, unprobed.
fn broadcast_scalar(v: Option<&Value>) -> Option<f64> {
    match v {
        None => Some(0.0),
        Some(Value::Int(n)) => int_as_f64(*n).map(sanitize),
        Some(Value::Float(f)) => Some(sanitize(*f)),
        Some(Value::Bool(b)) => Some(i64::from(*b) as f64),
        _ => None,
    }
}

/// Certified per-component op (`compositeMath`): plain f64 `+ - * /`, and
/// `%` as C `fmod` (dividend's sign) even for mixed signs, which the scalar
/// `math()` refuses (`Vec(0.5,0.25,-0.75) % Vec(0.25,0.5,0.75)` is `Z=-0.000`).
fn component_op(gate: &str, x: f64, y: f64) -> Option<f64> {
    Some(match gate {
        g if g.ends_with(G_MATH_ADD) => x + y,
        g if g.ends_with(G_MATH_SUBTRACT) => x - y,
        g if g.ends_with(G_MATH_MULTIPLY) => x * y,
        g if g.ends_with(G_MATH_DIVIDE) => x / y,
        g if g.ends_with(G_MATH_MODULO) => x % y,
        _ => return None,
    })
}

/// Vector Math*, component-wise or with a broadcast scalar (certified by the
/// `compositeMath` chapter). Inputs sanitize before the op (`Vec(NaN,1,2) +
/// Vec(1,1,1)` is `X=1.000`); a non-finite result component is refused.
fn composite_math(gate: &str, a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let finite3 = |x: f64, y: f64, z: f64| {
        (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
    };
    match (a, b) {
        (
            Some(Value::Vector {
                x: x1,
                y: y1,
                z: z1,
            }),
            Some(Value::Vector {
                x: x2,
                y: y2,
                z: z2,
            }),
        ) => {
            let x = component_op(gate, sanitize(*x1), sanitize(*x2))?;
            let y = component_op(gate, sanitize(*y1), sanitize(*y2))?;
            let z = component_op(gate, sanitize(*z1), sanitize(*z2))?;
            finite3(x, y, z)
        }
        (Some(Value::Vector { x, y, z }), other) => {
            let s = broadcast_scalar(other)?;
            let x = component_op(gate, sanitize(*x), s)?;
            let y = component_op(gate, sanitize(*y), s)?;
            let z = component_op(gate, sanitize(*z), s)?;
            finite3(x, y, z)
        }
        (other, Some(Value::Vector { x, y, z })) => {
            let s = broadcast_scalar(other)?;
            let x = component_op(gate, s, sanitize(*x))?;
            let y = component_op(gate, s, sanitize(*y))?;
            let z = component_op(gate, s, sanitize(*z))?;
            finite3(x, y, z)
        }
        _ => None,
    }
}

fn math(gate: &str, a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    if matches!(a, Some(Value::Vector { .. })) || matches!(b, Some(Value::Vector { .. })) {
        return composite_math(gate, a, b);
    }
    let (x, y) = (math_in(a)?, math_in(b)?);
    // Mixed int/float promotes to float (certified: 1 + 0.5 = 1.5).
    if let (MathIn::I(x), MathIn::I(y)) = (&x, &y) {
        let (x, y) = (*x, *y);
        return Some(Value::Int(match gate {
            g if g.ends_with(G_MATH_ADD) => x.checked_add(y)?,
            g if g.ends_with(G_MATH_SUBTRACT) => x.checked_sub(y)?,
            g if g.ends_with(G_MATH_MULTIPLY) => x.checked_mul(y)?,
            g if g.ends_with(G_MATH_DIVIDE) => {
                if y == 0 {
                    0
                }
                // certified: div by zero -> 0
                else {
                    // `checked_rem` refuses i64::MIN % -1, which would panic.
                    let rem = x.checked_rem(y)?;
                    // Truncation direction is unprobed for mixed signs with a
                    // remainder, so refuse.
                    if (x < 0) != (y < 0) && rem != 0 {
                        return None;
                    }
                    x.checked_div(y)?
                }
            }
            g if g.ends_with(G_MATH_MODULO) => {
                if y == 0 {
                    0
                }
                // certified: mod by zero -> 0
                else {
                    // `checked_rem` refuses i64::MIN % -1, which would panic.
                    let rem = x.checked_rem(y)?;
                    if (x < 0 || y < 0) && rem != 0 {
                        return None;
                    }
                    rem
                }
            }
            _ => return None,
        }));
    }
    let to_f = |m: MathIn| -> Option<f64> {
        Some(match m {
            MathIn::I(n) => int_as_f64(n)?,
            MathIn::F(f) => f,
        })
    };
    let (x, y) = (to_f(x)?, to_f(y)?);
    if gate.ends_with(G_MATH_MODULO) {
        let r = x % y;
        // As on the int path: truncation direction is unprobed for mixed
        // signs with a nonzero finite remainder.
        if (x < 0.0) != (y < 0.0) && r != 0.0 && r.is_finite() {
            return None;
        }
        return Some(Value::Float(r));
    }
    Some(Value::Float(match gate {
        g if g.ends_with(G_MATH_ADD) => x + y,
        g if g.ends_with(G_MATH_SUBTRACT) => x - y,
        g if g.ends_with(G_MATH_MULTIPLY) => x * y,
        g if g.ends_with(G_MATH_DIVIDE) => x / y, // non-finite result: fold
        _ => return None,                         // renders as "0"
    }))
}

// Extended math, bitwise and rounding
// One law per pure gate, inert until the certified table covers the gate's
// signature; the replay test then checks it against the game. Probes use
// finite in-domain inputs, so edge cases (domain errors, shifts of 64 or
// more, rounding ties) stay unprobed and unfolded. Float laws refuse a
// non-finite result.
const EXTENDED_MATH: &[&str] = &[
    "MathSin",
    "MathCos",
    "MathTan",
    "MathAsin",
    "MathAcos",
    "MathAtan",
    "MathSinh",
    "MathCosh",
    "MathTanh",
    "MathAsinh",
    "MathAcosh",
    "MathAtanh",
    "MathExp",
    "MathLn",
    "MathSqrt",
    "MathAbs",
    "MathSign",
    "MathNegate",
    "MathDegreesToRadians",
    "MathRadiansToDegrees",
    "MathAtan2",
    "MathPow",
    "MathMin",
    "MathMax",
    "MathLogBase",
    "MathClamp",
    "MathModuloFloored",
];

fn as_float(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Float(f) => Some(*f),
        Value::Int(n) => int_as_f64(*n),
        _ => None,
    }
}

fn as_int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Int(n) => Some(*n),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// The extended-math gates whose result is the variant of their operands: an
/// integer in, an integer out.
const VARIANT_PRESERVING: &[&str] = &["MathAbs", "MathMin", "MathMax", "MathClamp"];

fn integer_math(
    short: &str,
    a: Option<&Value>,
    b: Option<&Value>,
    c: Option<&Value>,
) -> Option<Value> {
    Some(Value::Int(match short {
        "MathAbs" => as_int(a)?.checked_abs()?,
        "MathMin" => as_int(a)?.min(as_int(b)?),
        "MathMax" => as_int(a)?.max(as_int(b)?),
        "MathClamp" => as_int(a)?.max(as_int(b)?).min(as_int(c)?),
        _ => return None,
    }))
}

fn extended_math(
    short: &str,
    a: Option<&Value>,
    b: Option<&Value>,
    c: Option<&Value>,
) -> Option<Value> {
    if VARIANT_PRESERVING.contains(&short)
        && [a, b, c]
            .into_iter()
            .flatten()
            .all(|v| matches!(v, Value::Int(_) | Value::Bool(_)))
    {
        return integer_math(short, a, b, c);
    }
    let r: f64 = match short {
        "MathSin" => as_float(a)?.sin(),
        "MathCos" => as_float(a)?.cos(),
        "MathTan" => as_float(a)?.tan(),
        "MathAsin" => as_float(a)?.asin(),
        "MathAcos" => as_float(a)?.acos(),
        "MathAtan" => as_float(a)?.atan(),
        "MathSinh" => as_float(a)?.sinh(),
        "MathCosh" => as_float(a)?.cosh(),
        "MathTanh" => as_float(a)?.tanh(),
        "MathAsinh" => as_float(a)?.asinh(),
        "MathAcosh" => as_float(a)?.acosh(),
        "MathAtanh" => as_float(a)?.atanh(),
        "MathExp" => as_float(a)?.exp(),
        "MathLn" => as_float(a)?.ln(),
        "MathSqrt" => as_float(a)?.sqrt(),
        "MathAbs" => as_float(a)?.abs(),
        "MathSign" => {
            let x = as_float(a)?;
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        "MathNegate" => -as_float(a)?,
        "MathDegreesToRadians" => as_float(a)?.to_radians(),
        "MathRadiansToDegrees" => as_float(a)?.to_degrees(),
        "MathAtan2" => as_float(a)?.atan2(as_float(b)?),
        "MathPow" => as_float(a)?.powf(as_float(b)?),
        "MathMin" => as_float(a)?.min(as_float(b)?),
        "MathMax" => as_float(a)?.max(as_float(b)?),
        "MathLogBase" => as_float(a)?.log(as_float(b)?),
        // clamp(x, lo, hi): raise to lo, then cap at hi.
        "MathClamp" => as_float(a)?.max(as_float(b)?).min(as_float(c)?),
        // Floored modulo: result carries the divisor's sign.
        "MathModuloFloored" => {
            let (x, y) = (as_float(a)?, as_float(b)?);
            x - y * (x / y).floor()
        }
        _ => return None,
    };
    r.is_finite().then_some(Value::Float(r))
}

fn bitwise(short: &str, a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    Some(Value::Int(match short {
        "BitwiseAND" => as_int(a)? & as_int(b)?,
        "BitwiseOR" => as_int(a)? | as_int(b)?,
        "BitwiseXOR" => as_int(a)? ^ as_int(b)?,
        "BitwiseNOT" => !as_int(a)?,
        "BitwiseNAND" => !(as_int(a)? & as_int(b)?),
        "BitwiseNOR" => !(as_int(a)? | as_int(b)?),
        // Shift amounts outside 0..63 are unprobed and overflow in Rust.
        "BitwiseShiftLeft" => {
            let s = as_int(b)?;
            if !(0..64).contains(&s) {
                return None;
            }
            as_int(a)? << s
        }
        "BitwiseShiftRight" => {
            let s = as_int(b)?;
            if !(0..64).contains(&s) {
                return None;
            }
            as_int(a)? >> s
        }
        "BitwiseBitCount" => i64::from(as_int(a)?.count_ones()),
        _ => return None,
    }))
}

// String family (`strings` chapter). Every law refuses non-ASCII operands and
// results: the multibyte probe (`"π≈3"`) cannot tell code points from UTF-16
// units, so those four cases are allowlisted refusals in the replay test.

const MAX_FOLDED_STRING_LEN: usize = 8192;
/// String folding refuses a float operand above this magnitude: the game
/// prints nothing for `1e20`, while `1e15` is certified to render
/// `"1,000,000,000,000,000"`.
const MAX_ABS_FLOAT_FOR_STRING_FOLD: f64 = 1e15;

fn ascii_str(v: &Value) -> Option<&str> {
    match v {
        Value::Str(s) if s.is_ascii() => Some(s),
        _ => None,
    }
}

/// Shared value-level refusal gate for the whole string family: any
/// non-ASCII string operand, or any float operand beyond the certified
/// printable magnitude, refuses regardless of signature coverage.
fn string_operands_foldable(vs: &[Option<&Value>]) -> bool {
    vs.iter().all(|v| match v {
        None | Some(Value::Int(_) | Value::Bool(_)) => true,
        Some(Value::Str(s)) => s.is_ascii(),
        Some(Value::Float(f)) => f.abs() <= MAX_ABS_FLOAT_FOR_STRING_FOLD,
        _ => false, // composites never appear in this family
    })
}

fn string_result_ok(s: &str) -> Option<Value> {
    (s.is_ascii() && s.len() <= MAX_FOLDED_STRING_LEN).then(|| Value::Str(s.to_string()))
}

/// `String_Concatenate` with the empty `Separator` that `..` always lowers
/// with: a two-operand join using `concat_display`, which is certified to
/// differ from `render_for_format` on bools.
fn concatenate(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    if !string_operands_foldable(&[a, b]) {
        return None;
    }
    let render_operand = |v: Option<&Value>| match v {
        None => String::new(),
        Some(v) => concat_display(v),
    };
    string_result_ok(&(render_operand(a) + &render_operand(b)))
}

fn string_length(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    Some(Value::Int(s.chars().count() as i64))
}
fn string_to_lower(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    string_result_ok(&s.to_ascii_lowercase())
}
fn string_to_upper(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    string_result_ok(&s.to_ascii_uppercase())
}
fn string_trim(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    string_result_ok(s.trim())
}
/// Certified: an empty needle is never contained, a prefix or a suffix,
/// unlike Rust's `str::contains("")` and friends, which return `true`.
fn string_contains(s: Option<&Value>, needle: Option<&Value>) -> Option<Value> {
    let (s, needle) = (ascii_str(s?)?, ascii_str(needle?)?);
    Some(Value::Bool(!needle.is_empty() && s.contains(needle)))
}
fn string_starts_with(s: Option<&Value>, pre: Option<&Value>) -> Option<Value> {
    let (s, pre) = (ascii_str(s?)?, ascii_str(pre?)?);
    Some(Value::Bool(!pre.is_empty() && s.starts_with(pre)))
}
fn string_ends_with(s: Option<&Value>, suf: Option<&Value>) -> Option<Value> {
    let (s, suf) = (ascii_str(s?)?, ascii_str(suf?)?);
    Some(Value::Bool(!suf.is_empty() && s.ends_with(suf)))
}
/// Certified: a negative `start` counts from the end (`("hello", -1, 3)` is
/// "o"), and `start` and `start + length` clamp to the string (`(.., 10, 3)`
/// is "", `(.., 1, 100)` is "ello"). A negative length is unprobed: refused.
fn string_substring(
    s: Option<&Value>,
    start: Option<&Value>,
    len: Option<&Value>,
) -> Option<Value> {
    let s = ascii_str(s?)?;
    let (Some(Value::Int(start)), Some(Value::Int(len))) = (start, len) else {
        return None;
    };
    if *len < 0 {
        return None;
    } // unprobed
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let actual_start = if *start < 0 {
        (n + *start).max(0)
    } else {
        (*start).min(n)
    };
    // Saturating: a near-i64::MAX length must clamp to the string end, not
    // overflow to a negative `end` that panics the slice below.
    let end = actual_start.saturating_add(*len).min(n);
    let slice: String = chars[actual_start as usize..end as usize].iter().collect();
    string_result_ok(&slice)
}
/// Certified: an empty needle is never found (`Find(s, "")` is -1), unlike
/// Rust's `str::find("")`, which returns 0.
fn string_find(s: Option<&Value>, needle: Option<&Value>) -> Option<Value> {
    let (s, needle) = (ascii_str(s?)?, ascii_str(needle?)?);
    if needle.is_empty() {
        return Some(Value::Int(-1));
    }
    Some(Value::Int(s.find(needle).map_or(-1, |i| i as i64)))
}
/// Certified: an empty search string leaves `s` unchanged (Rust's
/// `str::replace("", x)` inserts `x` everywhere). Probes had at most one
/// match, so replace-first vs replace-all is unknown: two or more refuse.
fn string_replace(
    s: Option<&Value>,
    search: Option<&Value>,
    repl: Option<&Value>,
) -> Option<Value> {
    let (s, search, repl) = (ascii_str(s?)?, ascii_str(search?)?, ascii_str(repl?)?);
    if search.is_empty() {
        return string_result_ok(s);
    }
    if s.matches(search).count() >= 2 {
        return None;
    } // replace-first vs -all unprobed
    string_result_ok(&s.replace(search, repl))
}
/// Certified: whitespace-trimmed, strict-integer-only text parses; anything
/// else (including a syntactically-numeric float like `"1.5"`) -> 0.
fn string_parse_int(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    Some(Value::Int(s.trim().parse::<i64>().unwrap_or(0)))
}
/// Certified: always parses through f64 (`"9007199254740993"` gives
/// 9007199254740992). Text containing `.`/`e`/`E` yields a float, otherwise
/// the double cast to int. `inf`/`nan` spellings are unprobed and refused.
fn string_parse_number(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();
    let lower = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    if lower == "inf" || lower == "infinity" || lower == "nan" {
        return None;
    }
    let has_dot_or_exp = trimmed.contains(['.', 'e', 'E']);
    let f = trimmed.parse::<f64>().unwrap_or(0.0);
    if !f.is_finite() {
        return None;
    } // overflowing text such as "1e400"
    Some(if has_dot_or_exp {
        Value::Float(sanitize(f))
    } else {
        Value::Int(f as i64)
    })
}

/// `FormatText`'s substitution slots, `InputA..InputG`, in template-index
/// order (`{0}`..`{6}`).
pub(crate) const FORMAT_SLOTS: [WirePort; 7] = [
    WirePort::InputA,
    WirePort::InputB,
    WirePort::InputC,
    WirePort::InputD,
    WirePort::InputE,
    WirePort::InputF,
    WirePort::InputG,
];

/// The `FormatText` templates an interpolation `parts` renders through: each
/// literal chunk with `{`/`}` escaped, each `${expr}` a `{N}` slot numbered
/// from 0 within its group, paired with what `value` gives for that
/// expression. A group holds at most [`FORMAT_SLOTS`] substitutions; one more
/// starts the next.
pub(crate) fn interp_groups<T, E>(
    parts: &[crate::ast::InterpPart],
    mut value: impl FnMut(&crate::ast::Expr) -> Result<T, E>,
) -> Result<Vec<(String, Vec<T>)>, E> {
    use crate::ast::InterpPart;
    let mut groups: Vec<(String, Vec<T>)> = Vec::new();
    let mut fmt = String::new();
    let mut slots: Vec<T> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Lit(s) => fmt.push_str(&s.replace('{', "{{").replace('}', "}}")),
            InterpPart::Expr(expr) => {
                if slots.len() == FORMAT_SLOTS.len() {
                    groups.push((std::mem::take(&mut fmt), std::mem::take(&mut slots)));
                }
                let v = value(expr)?;
                let _ = write!(fmt, "{{{}}}", slots.len());
                slots.push(v);
            }
        }
    }
    groups.push((fmt, slots));
    Ok(groups)
}

/// `FormatText`'s certified law: `{0}`..`{6}` become `render_for_format` of
/// that input, an unbound, out-of-range or non-numeric slot renders "0", and
/// `{{`/`}}` are literal braces (the escaping `interp_groups` writes).
/// `eval()` never reaches it (signature `[Tmpl]`); `try_resolve_format_text`
/// and the fuzz harness call it directly.
pub fn format_text(template: &str, inputs: &[Option<Value>]) -> Option<String> {
    if !template.is_ascii() {
        return None;
    }
    if !string_operands_foldable(&inputs.iter().map(Option::as_ref).collect::<Vec<_>>()) {
        return None;
    }
    let bytes = template.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if template[i..].starts_with("{{") {
            out.push('{');
            i += 2;
            continue;
        }
        if template[i..].starts_with("}}") {
            out.push('}');
            i += 2;
            continue;
        }
        if bytes[i] == b'{' {
            let rel = template[i + 1..].find('}')?; // unterminated: unprobed
            let tag = &template[i + 1..i + 1 + rel];
            // The gate's own syntax, documented by its in-game tooltip but
            // never probed: `{A}`..`{G}` name an input, `:` adds an option
            // (`{A:,}`, `{A:.2}`, `{A:On=Off}`), and `|` after a tag picks a
            // word form (`{A}|plural(...)`). Leave those to the game.
            let names_input = tag
                .as_bytes()
                .first()
                .is_some_and(|c| (b'A'..=b'G').contains(c))
                && (tag.len() == 1 || tag.as_bytes()[1] == b':');
            let word_form = template[i + rel + 2..]
                .strip_prefix('|')
                .is_some_and(|rest| {
                    let name = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
                    name > 0 && rest.as_bytes().get(name) == Some(&b'(')
                });
            if names_input || tag.contains(':') || word_form {
                return None;
            }
            let rendered = match tag.parse::<usize>() {
                Ok(slot) if slot < 7 => match inputs.get(slot) {
                    Some(Some(v)) => render_for_format(v),
                    _ => "0".to_string(), // unwired or out-of-range slot: certified
                },
                _ => "0".to_string(), // non-numeric tag such as "{a}": certified
            };
            out.push_str(&rendered);
            i += rel + 2;
            continue;
        }
        out.push(bytes[i] as char); // safe: template.is_ascii() already checked
        i += 1;
    }
    if out.len() > MAX_FOLDED_STRING_LEN || !out.is_ascii() {
        return None;
    }
    Some(out)
}

// Composite constructors and ops (`compositeOps` chapter).

/// sRGB (gamma-encoded, 0-1) -> linear (0-1): the IEC 61966-2-1 inverse of
/// `linear_to_srgb`. The curve is certified only in that direction.
fn srgb_to_linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
/// Linear (0-1) -> sRGB (gamma-encoded, 0-1), the standard transfer function.
/// Certified: `ColorToHex(Color(1.0, 0.5, 0.0))` is "FFBC00" (0.5 encodes to
/// 0.7354, which is byte 188).
fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Certified directly, since vectors render through `FormatText`:
/// `MakeVector(1.5,-2.5,0.75)` prints `X=1.500 Y=-2.500 Z=0.750`.
fn make_vector(a: Option<&Value>, b: Option<&Value>, c: Option<&Value>) -> Option<Value> {
    let f = |v: Option<&Value>| match v {
        Some(Value::Float(f)) => Some(sanitize(*f)),
        _ => None,
    };
    let (x, y, z) = (f(a)?, f(b)?, f(c)?);
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
/// Refused by `BLANK_RENDER_REFUSED`: the only case renders blank and no
/// certified gate consumes a rotator, so this field packing is unverified.
/// Implemented for when a probe chains rotators through a rendering gate.
fn make_rotation(p: Option<&Value>, y: Option<&Value>, r: Option<&Value>) -> Option<Value> {
    let f = |v: Option<&Value>| match v {
        Some(Value::Float(f)) => Some(sanitize(*f)),
        _ => None,
    };
    let (pitch, yaw, roll) = (f(p)?, f(y)?, f(r)?);
    (pitch.is_finite() && yaw.is_finite() && roll.is_finite()).then_some(Value::Rotator {
        pitch,
        yaw,
        roll,
    })
}
/// Certified transitively: its own case renders blank, but `RotateVector`'s
/// three cases and `QuatDotProduct`'s 0.707 case consume its quats, so a
/// wrong component order would fail their replay.
fn make_quaternion(
    a: Option<&Value>,
    b: Option<&Value>,
    c: Option<&Value>,
    d: Option<&Value>,
) -> Option<Value> {
    let f = |v: Option<&Value>| match v {
        Some(Value::Float(f)) => Some(sanitize(*f)),
        _ => None,
    };
    let (x, y, z, w) = (f(a)?, f(b)?, f(c)?, f(d)?);
    (x.is_finite() && y.is_finite() && z.is_finite() && w.is_finite()).then_some(Value::Quat {
        x,
        y,
        z,
        w,
    })
}
/// RGB certified transitively by `ColorToHex(Color(1.0,0.5,0.0))` giving
/// "FFBC00"; alpha has no evidence (`ColorToHex` drops it), so a wired alpha
/// refuses. A real 3-arg `Color(r, g, b)` still never folds: its signature
/// `[Float, Float, Float, Unwired]` is unprobed (the only case is 4-arg).
fn make_color(
    a: Option<&Value>,
    b: Option<&Value>,
    c: Option<&Value>,
    d: Option<&Value>,
) -> Option<Value> {
    if d.is_some() {
        return None;
    } // explicit alpha: unconfirmed
    let f = |v: Option<&Value>| match v {
        Some(Value::Float(f)) => Some(sanitize(*f)),
        _ => None,
    };
    let (r, g, b_) = (f(a)?, f(b)?, f(c)?);
    (r.is_finite() && g.is_finite() && b_.is_finite()).then_some(Value::Color {
        r,
        g,
        b: b_,
        a: 1.0,
    })
}
/// Refused by `BLANK_RENDER_REFUSED`: the only case renders blank, and the
/// `srgb_to_linear` curve is unprobed in this direction. Implemented for when
/// a probe round-trips this gate through `ColorToHex`.
fn make_color_srgb(
    a: Option<&Value>,
    b: Option<&Value>,
    c: Option<&Value>,
    d: Option<&Value>,
) -> Option<Value> {
    let byte = |v: Option<&Value>| match v {
        Some(Value::Int(n)) if (0..=255).contains(n) => Some(*n as f64 / 255.0),
        _ => None, // out-of-range byte: unprobed
    };
    let (r, g, b_, a_) = (byte(a)?, byte(b)?, byte(c)?, byte(d)?);
    Some(Value::Color {
        r: srgb_to_linear(r),
        g: srgb_to_linear(g),
        b: srgb_to_linear(b_),
        a: a_,
    })
}
/// Refused by `BLANK_RENDER_REFUSED`: the case certifies the input shape
/// (`"#RRGGBB"`, opaque alpha) but its output renders blank, so the color
/// value is unverified. Implemented for a future `ColorToHex` round-trip.
fn make_color_hex(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    Some(Value::Color {
        r: srgb_to_linear(r),
        g: srgb_to_linear(g),
        b: srgb_to_linear(b),
        a: 1.0,
    })
}
/// Certified (`ColorToHex(Color(1.0,0.5,0.0))` is "FFBC00"): RGB only,
/// uppercase, 2 hex digits per channel, gamma-encoded and rounded per byte.
fn color_to_hex(c: Option<&Value>) -> Option<Value> {
    let Some(Value::Color { r, g, b, .. }) = c else {
        return None;
    };
    let byte = |c: f64| -> Option<u8> {
        let v = (linear_to_srgb(sanitize(c)) * 255.0).round();
        (0.0..=255.0).contains(&v).then_some(v as u8)
    };
    let (r, g, b) = (byte(*r)?, byte(*g)?, byte(*b)?);
    Some(Value::Str(format!("{r:02X}{g:02X}{b:02X}")))
}

fn vec_scale(v: Option<&Value>, s: Option<&Value>) -> Option<Value> {
    let (Some(Value::Vector { x, y, z }), Some(Value::Float(s))) = (v, s) else {
        return None;
    };
    let s = sanitize(*s);
    let (x, y, z) = (sanitize(*x) * s, sanitize(*y) * s, sanitize(*z) * s);
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
fn vec_dot(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (
        Some(Value::Vector {
            x: x1,
            y: y1,
            z: z1,
        }),
        Some(Value::Vector {
            x: x2,
            y: y2,
            z: z2,
        }),
    ) = (a, b)
    else {
        return None;
    };
    let d = sanitize(*x1) * sanitize(*x2)
        + sanitize(*y1) * sanitize(*y2)
        + sanitize(*z1) * sanitize(*z2);
    d.is_finite().then_some(Value::Float(d))
}
fn vec_cross(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (
        Some(Value::Vector {
            x: x1,
            y: y1,
            z: z1,
        }),
        Some(Value::Vector {
            x: x2,
            y: y2,
            z: z2,
        }),
    ) = (a, b)
    else {
        return None;
    };
    let (x1, y1, z1) = (sanitize(*x1), sanitize(*y1), sanitize(*z1));
    let (x2, y2, z2) = (sanitize(*x2), sanitize(*y2), sanitize(*z2));
    let (x, y, z) = (y1 * z2 - z1 * y2, z1 * x2 - x1 * z2, x1 * y2 - y1 * x2);
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
fn vec_magnitude_sq(v: Option<&Value>) -> Option<Value> {
    let Some(Value::Vector { x, y, z }) = v else {
        return None;
    };
    let (x, y, z) = (sanitize(*x), sanitize(*y), sanitize(*z));
    let m = x * x + y * y + z * z;
    m.is_finite().then_some(Value::Float(m))
}
fn vec_distance_sq(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (
        Some(Value::Vector {
            x: x1,
            y: y1,
            z: z1,
        }),
        Some(Value::Vector {
            x: x2,
            y: y2,
            z: z2,
        }),
    ) = (a, b)
    else {
        return None;
    };
    let (dx, dy, dz) = (
        sanitize(*x1) - sanitize(*x2),
        sanitize(*y1) - sanitize(*y2),
        sanitize(*z1) - sanitize(*z2),
    );
    let d = dx * dx + dy * dy + dz * dz;
    d.is_finite().then_some(Value::Float(d))
}
/// Unreal's `FQuat::RotateVector` order (`T = 2*cross(u,v); v + w*T +
/// cross(u,T)`), which reproduces the game's rounding: the 90-degree case
/// prints `X=-0.000`, where the textbook formula gives `X=0.000`. Certified
/// against all three cases (90, 180 and 45 degrees).
fn rotate_vector(v: Option<&Value>, q: Option<&Value>) -> Option<Value> {
    let (
        Some(Value::Vector {
            x: vx,
            y: vy,
            z: vz,
        }),
        Some(Value::Quat {
            x: qx,
            y: qy,
            z: qz,
            w: qw,
        }),
    ) = (v, q)
    else {
        return None;
    };
    let (vx, vy, vz) = (sanitize(*vx), sanitize(*vy), sanitize(*vz));
    let (ux, uy, uz, s) = (sanitize(*qx), sanitize(*qy), sanitize(*qz), sanitize(*qw));
    let (c1x, c1y, c1z) = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
    let (tx, ty, tz) = (2.0 * c1x, 2.0 * c1y, 2.0 * c1z);
    let (c2x, c2y, c2z) = (uy * tz - uz * ty, uz * tx - ux * tz, ux * ty - uy * tx);
    let x = vx + s * tx + c2x;
    let y = vy + s * ty + c2y;
    let z = vz + s * tz + c2z;
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
/// Quaternion inverse, `conjugate / |q|^2`. Refused by `BLANK_RENDER_REFUSED`:
/// its case renders blank and no certified case feeds its output onward.
fn invert_rotation(q: Option<&Value>) -> Option<Value> {
    let Some(Value::Quat { x, y, z, w }) = q else {
        return None;
    };
    let (x, y, z, w) = (sanitize(*x), sanitize(*y), sanitize(*z), sanitize(*w));
    let norm_sq = x * x + y * y + z * z + w * w;
    if norm_sq == 0.0 || !norm_sq.is_finite() {
        return None;
    }
    let (ix, iy, iz, iw) = (-x / norm_sq, -y / norm_sq, -z / norm_sq, w / norm_sq);
    (ix.is_finite() && iy.is_finite() && iz.is_finite() && iw.is_finite()).then_some(Value::Quat {
        x: ix,
        y: iy,
        z: iz,
        w: iw,
    })
}
fn quat_dot(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (
        Some(Value::Quat {
            x: x1,
            y: y1,
            z: z1,
            w: w1,
        }),
        Some(Value::Quat {
            x: x2,
            y: y2,
            z: z2,
            w: w2,
        }),
    ) = (a, b)
    else {
        return None;
    };
    let d = sanitize(*x1) * sanitize(*x2)
        + sanitize(*y1) * sanitize(*y2)
        + sanitize(*z1) * sanitize(*z2)
        + sanitize(*w1) * sanitize(*w2);
    d.is_finite().then_some(Value::Float(d))
}

/// Evaluate one pure gate on certified inputs; `None` refuses the fold.
/// `pub` for the `--fold-diff` fuzz harness's predictor.
pub fn eval(gate_class: &str, inputs: &[Option<Value>]) -> Option<Value> {
    // Refuse any input signature the probe never observed for this class, so
    // every law below sees only certified shapes.
    let sig: Vec<InVariant> = inputs
        .iter()
        .map(|v| v.as_ref().map_or(InVariant::Unwired, Value::variant))
        .collect();
    if !crate::lower::fold::table::CertifiedTable::certified().covers(gate_class, &sig) {
        return None;
    }

    let short = gate_class.rsplit('_').next().unwrap_or("");
    if DEFERRED.contains(&short) {
        return None;
    }

    if BLANK_RENDER_REFUSED.contains(&short) {
        return None;
    }

    let one = |i: usize| inputs.get(i).and_then(|v| v.as_ref());
    let (a, b, c, d) = (one(0), one(1), one(2), one(3));
    Some(match short {
        G_COMPARE_EQUAL => Value::Bool(eq(a, b)?),
        G_COMPARE_NOT_EQUAL => Value::Bool(!eq(a, b)?),
        G_COMPARE_LESS => Value::Bool(cmp(a, b)? == std::cmp::Ordering::Less),
        G_COMPARE_LESS_OR_EQUAL => Value::Bool(cmp(a, b)? != std::cmp::Ordering::Greater),
        G_COMPARE_GREATER => Value::Bool(cmp(a, b)? == std::cmp::Ordering::Greater),
        G_COMPARE_GREATER_OR_EQUAL => Value::Bool(cmp(a, b)? != std::cmp::Ordering::Less),
        G_LOGICAL_AND => Value::Bool(truthy(a) && truthy(b)),
        G_LOGICAL_OR => Value::Bool(truthy(a) || truthy(b)),
        G_LOGICAL_XOR => Value::Bool(truthy(a) != truthy(b)),
        G_LOGICAL_NOT => Value::Bool(!truthy(a)),
        "Round" => Value::Float(as_float(a)?.round()),
        "Floor" => Value::Float(as_float(a)?.floor()),
        "Ceil" => Value::Float(as_float(a)?.ceil()),
        s if EXTENDED_MATH.contains(&s) => extended_math(s, a, b, c)?,
        s if s.starts_with("Bitwise") => bitwise(s, a, b)?,
        s if s.starts_with("Math") => math(gate_class, a, b)?,
        G_CONCATENATE => concatenate(a, b)?,
        G_LENGTH => string_length(a)?,
        G_TO_LOWER => string_to_lower(a)?,
        G_TO_UPPER => string_to_upper(a)?,
        G_TRIM => string_trim(a)?,
        G_CONTAINS => string_contains(a, b)?,
        G_STARTS_WITH => string_starts_with(a, b)?,
        G_ENDS_WITH => string_ends_with(a, b)?,
        G_SUBSTRING => string_substring(a, b, c)?,
        G_FIND => string_find(a, b)?,
        G_REPLACE => string_replace(a, b, c)?,
        G_PARSE_INT => string_parse_int(a)?,
        G_PARSE_NUMBER => string_parse_number(a)?,
        G_MAKE_VECTOR => make_vector(a, b, c)?,
        G_MAKE_ROTATION => make_rotation(a, b, c)?, // refused above: BLANK_RENDER_REFUSED
        G_MAKE_QUATERNION => make_quaternion(a, b, c, d)?,
        G_MAKE_COLOR => make_color(a, b, c, d)?,
        G_MAKE_COLOR_SRGB => make_color_srgb(a, b, c, d)?, // refused above: BLANK_RENDER_REFUSED
        G_MAKE_COLOR_HEX => make_color_hex(a)?,            // refused above: BLANK_RENDER_REFUSED
        G_COLOR_TO_HEX => color_to_hex(a)?,
        G_VEC_SCALE => vec_scale(a, b)?,
        G_VEC_DOT_PRODUCT => vec_dot(a, b)?,
        G_VEC_CROSS_PRODUCT => vec_cross(a, b)?,
        G_VEC_MAGNITUDE_SQUARED => vec_magnitude_sq(a)?,
        G_VEC_DISTANCE_SQUARED => vec_distance_sq(a, b)?,
        G_ROTATE_VECTOR => rotate_vector(a, b)?,
        G_INVERT_ROTATION => invert_rotation(a)?, // refused above: BLANK_RENDER_REFUSED
        G_QUAT_DOT_PRODUCT => quat_dot(a, b)?,
        // FormatText (signature `[Tmpl]`) was refused by `covers()` above;
        // Select/Branch are handled by the driver.
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
