//! Certified evaluator: implements exactly the laws the in-game probe
//! certified (data/gate_semantics.json). `None` = refuse to fold. The replay
//! test at the bottom locks every table case to this implementation — table
//! and evaluator cannot drift apart without breaking the build.
use crate::ir::Literal;
use crate::lower::fold::table::InVariant;

// `pub`: reachable as `wirescript::lower::fold::eval::Value` from the
// `--fold-diff` fuzz harness (see the visibility note on `pub mod fold` in
// `lower/mod.rs`). `variant()` stays `pub(crate)` since it returns the
// crate-private `InVariant` — the harness never needs it (it re-derives
// coverage indirectly by calling `eval()`, which already gates on it).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Vector { x: f64, y: f64, z: f64 },
    Rotator { pitch: f64, yaw: f64, roll: f64 },
    Quat { x: f64, y: f64, z: f64, w: f64 },
    /// Linear-space RGBA (0-1 components) — the wire `LinearColor` variant.
    /// `ColorToHex` converts FROM this at a certified sRGB gamma boundary
    /// (round-trip-verified — see `color_to_hex`'s doc comment); the
    /// REVERSE direction used by `MakeColorSRGB`/`MakeColorHex` is the same
    /// curve's mathematical inverse, never itself independently probed (see
    /// those two functions' doc comments — both hard-refuse in production
    /// folding as a result). Every OTHER composite/color op just carries it
    /// through untouched.
    Color { r: f64, g: f64, b: f64, a: f64 },
}

impl Value {
    pub(crate) fn variant(&self) -> InVariant {
        match self {
            Value::Int(_) => InVariant::Int,
            Value::Float(_) => InVariant::Float,
            Value::Bool(_) => InVariant::Bool,
            Value::Str(_) => InVariant::Str,
            Value::Vector { .. } => InVariant::Vector,
            Value::Rotator { .. } => InVariant::Rotator,
            Value::Quat { .. } => InVariant::Quat,
            Value::Color { .. } => InVariant::Color,
        }
    }
    pub fn from_literal(lit: &Literal) -> Option<Value> {
        match lit {
            Literal::Int(n) => Some(Value::Int(*n)),
            Literal::Float(f) => Some(Value::Float(*f)),
            Literal::Bool(b) => Some(Value::Bool(*b)),
            Literal::String(s) => Some(Value::Str(s.clone())),
            Literal::Vector { x, y, z } => Some(Value::Vector { x: *x, y: *y, z: *z }),
            Literal::Rotator { pitch, yaw, roll } => {
                Some(Value::Rotator { pitch: *pitch, yaw: *yaw, roll: *roll })
            }
            Literal::Quat { x, y, z, w } => Some(Value::Quat { x: *x, y: *y, z: *z, w: *w }),
            // `Literal::LinearColor` IS this fold pass's own composite Color
            // shape (its doc comment says as much: "produced by folding a
            // constant Color(r, g, b, a?) call") — pass through untouched.
            Literal::LinearColor { r, g, b, a } => {
                Some(Value::Color { r: *r, g: *g, b: *b, a: *a })
            }
            // `Literal::Color` (u8 0-255) is a DIFFERENT, pre-existing
            // representation (brick-paint literals — see emit.rs's
            // `Literal::Color -> WireVariant::LinearColor` arm) that
            // predates this fold pass and already has its own established,
            // if approximate ("linear-ish", per that arm's own comment)
            // conversion: a plain `/255.0`, NOT the true sRGB gamma curve
            // `MakeColorSRGB`/`MakeColorHex` are certified to use below.
            // Reusing the gamma curve here would make folding a `_Literal`
            // color produce a VISIBLY DIFFERENT color than leaving it
            // unfolded — matching emit.rs's existing conversion exactly is
            // what makes folding semantics-preserving.
            Literal::Color { r, g, b, a } => Some(Value::Color {
                r: *r as f64 / 255.0,
                g: *g as f64 / 255.0,
                b: *b as f64 / 255.0,
                a: *a as f64 / 255.0,
            }),
            _ => None, // object/array/asset/prefab: not certified
        }
    }
    pub fn to_literal(&self) -> Literal {
        match self {
            Value::Int(n) => Literal::Int(*n),
            Value::Float(f) => Literal::Float(*f),
            Value::Bool(b) => Literal::Bool(*b),
            Value::Str(s) => Literal::String(s.clone()),
            Value::Vector { x, y, z } => Literal::Vector { x: *x, y: *y, z: *z },
            Value::Rotator { pitch, yaw, roll } => {
                Literal::Rotator { pitch: *pitch, yaw: *yaw, roll: *roll }
            }
            Value::Quat { x, y, z, w } => Literal::Quat { x: *x, y: *y, z: *z, w: *w },
            // The inverse of `from_literal`'s `LinearColor` arm — always
            // round-trips through the exact linear representation, never
            // the lossy u8 `Literal::Color` shape.
            Value::Color { r, g, b, a } => Literal::LinearColor { r: *r, g: *g, b: *b, a: *a },
        }
    }
}

// ============================================================================
// Gate short-name constants. Every string here is the suffix after the last
// `_` in a gate's full `BrickComponentType_WireGraph_...` class name (what
// `eval()` matches on via `gate_class.rsplit('_').next()`). Defined ONCE and
// referenced everywhere a short name would otherwise be repeated as a raw
// string literal — the `eval()` match arms below, the `DEFERRED`/
// `BLANK_RENDER_REFUSED` refusal lists (also here), the `Math*` suffix
// checks in `component_op`/`math`, and the test module's mirrored lists — so
// a rename of one can't silently desync from the others.
// ============================================================================
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

// Deferred-ops family (probed but never folded — see `DEFERRED` below).
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

// Math* family — matched by SUFFIX (`gate_class.ends_with(...)`) rather than
// exact short name in `component_op`/`math`, since those two functions
// receive the FULL gate class string, not the post-`rsplit` short name.
const G_MATH_ADD: &str = "MathAdd";
const G_MATH_SUBTRACT: &str = "MathSubtract";
const G_MATH_MULTIPLY: &str = "MathMultiply";
const G_MATH_DIVIDE: &str = "MathDivide";
const G_MATH_MODULO: &str = "MathModulo";

/// `deferredOps` chapter: probed but NEVER folded — hard refusal regardless
/// of signature coverage (allowlisted wholesale in the replay test below).
/// These gates' math is meaningfully harder to get exactly right (slerp,
/// normalize, arbitrary-axis construction, blend) and weren't asked to be
/// implemented by this task. Module-level (not local to `eval()`) so the
/// test module's `deferred_ops_always_refuse`/`replay_every_certified_case`
/// assert against the SAME list `eval()` actually refuses against, rather
/// than a hand-copied mirror that could silently drift.
const DEFERRED: &[&str] = &[
    G_VEC_MAGNITUDE, G_VEC_NORMALIZE, G_VEC_DISTANCE, G_QUAT_SLERP,
    G_QUAT_FROM_AXIS_ANGLE, G_QUAT_ANGLE_BETWEEN, G_QUAT_BETWEEN,
    G_DIRECTION_TO_ROTATION, G_ROTATION_TO_DIRECTION, G_COLOR_BLEND,
];

/// MUST-REFUSE — blank-render-only evidence, ZERO transitive certification:
/// each of these gates' only table evidence is a case whose output renders
/// blank (rotator/quat/color never render through FormatText — certified,
/// see `render_for_format`'s doc comment), and "blank==blank" proves nothing
/// about the correctness of the actual computed value. Hard-refused
/// regardless of signature coverage. Output unobservable in-game (composites
/// render blank); certification pending a probe wave that chains these
/// through a rendering gate (`ColorToHex` / `RotateVector` / a future
/// `Split*`).
///
/// NOT here — fold-eligible via TRANSITIVE certification through a
/// different, value-bearing gate (see each function's own doc comment for
/// the specific evidence): `MakeQuaternion` (certified via `RotateVector`'s
/// 3 value-bearing cases + `QuatDotProduct`'s 0.707 case) and `MakeColor`'s
/// 3-arg RGB form (certified via `ColorToHex("FFBC00")`; its 4-arg
/// alpha-carrying form still hard-refuses, see `make_color`).
///
/// Module-level for the same reason as `DEFERRED` — see
/// `blank_render_gates_always_refuse` in the test module below.
const BLANK_RENDER_REFUSED: &[&str] = &[
    G_MAKE_ROTATION, G_MAKE_COLOR_SRGB, G_MAKE_COLOR_HEX, G_INVERT_ROTATION,
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
        // Never probed (Select/Branch chapters are scalar-only) and no
        // composite-producing gate ever feeds a condition port in this
        // table — dead in practice, but `truthy` must stay total.
        Some(Value::Vector { .. } | Value::Rotator { .. }
            | Value::Quat { .. } | Value::Color { .. }) => false,
    }
}

// ============================================================================
// Rendering — TWO certified laws, deliberately kept separate (see the v3
// probe's own comment on why they differ: `data/gate_semantics.json`'s case
// outputs for the pre-v3 scalar chapters were recorded through the exact
// same interpolation path as everything else, but gen_semantics.mjs
// CANONICALIZES bool-output gates to "true"/"false" text and STRIPS commas
// from int-classified outputs before persisting them — so the persisted
// case text and the game's literal on-screen text are not always the same
// string, even though both trace back to one underlying render law).
// ============================================================================

/// Round to 3 decimals and split into (sign, integer digits, trimmed
/// fractional digits) — the core numeric law both renderers share. Ties
/// round to even (`format!("{:.3}", ..)`'s native behavior, certified
/// against e.g. `1.0/3.0 -> 0.333`, `VecDotProduct(...) -> -0.312` from
/// the exact value `-0.3125`, and `1.5e-3 -> 0.002`).
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
    // A negative value that rounds all the way to zero (exact -0.0, or any
    // sufficiently small negative magnitude) has no meaningful sign — certified
    // for the exact -0.0 case (`render` section: "float:-0.0" -> "0"); the
    // same collapse is applied to a near-zero negative for consistency (not
    // itself probed, but there is no probed case where a signed near-zero
    // SCALAR survives to the render step un-collapsed — unlike vector
    // components, see `fixed3` below, which DO show a preserved sign).
    let neg = neg && !(int_part == "0" && trimmed_frac.is_empty());
    (neg, int_part.to_string(), trimmed_frac.to_string())
}

fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        let from_end = bytes.len() - i;
        if i > 0 && from_end % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// One vector/rotator/quat/color COMPONENT, as printed by Unreal's native
/// `FVector::ToString`-style formatter: fixed 3 decimals, no grouping, no
/// trimming — certified directly (`MakeVector(1.5,-2.5,0.75)` renders
/// `X=1.500 Y=-2.500 Z=0.750`, not `X=1.5 ...`), including a PRESERVED sign
/// on a signed near-zero result (`RotateVector` case 1 renders `X=-0.000`).
fn fixed3(f: f64) -> String {
    format!("{:.3}", sanitize(f))
}

/// The certified PRODUCTION render law (`data/gate_semantics.json`'s
/// `render` table section, probeVersion 3): reproduces exactly what
/// FormatText prints for a given wire value. Folding correctness for
/// `FormatText` template substitution depends on this being exact — every
/// entry in the certified `render` section is validated against it
/// (`table::tests` / `eval::tests::render_for_format_matches_certified_table`).
///
/// Ints: comma-grouped every 3 digits from 1,000 up, sign outside the
/// grouping (`-1,000,000`). Floats: rounded to 3 decimals (ties to even),
/// comma-grouped integer part, trailing fractional zeros (and a bare
/// trailing `.`) dropped; non-finite and -0.0 render `"0"`. Bools render
/// `"1"`/`"0"` (certified — NOT `"true"`/`"false"`, see `render` below for
/// the case where that string DOES appear). Vectors render
/// `X=%.3f Y=%.3f Z=%.3f` (no grouping, no trimming). Rotator/Color/Quat
/// NEVER render through FormatText — certified blank (`render` section:
/// every rotator/color/quat entry maps to `""`, matching every
/// `MakeRotation`/`MakeQuaternion`/`MakeColor*`/`InvertRotation` case
/// output too).
pub(crate) fn render_for_format(v: &Value) -> String {
    match v {
        Value::Int(n) => {
            let (neg, digits) = match n.checked_abs() {
                Some(a) => (*n < 0, a.to_string()),
                // i64::MIN has no positive abs — its digits are already
                // unsigned-safe via the unsigned formatting below.
                None => (true, (*n as i128).unsigned_abs().to_string()),
            };
            let grouped = group_thousands(&digits);
            if neg { format!("-{grouped}") } else { grouped }
        }
        Value::Float(f) => {
            let (neg, int_part, frac) = round3(*f);
            let grouped = group_thousands(&int_part);
            let sign = if neg { "-" } else { "" };
            if frac.is_empty() { format!("{sign}{grouped}") } else { format!("{sign}{grouped}.{frac}") }
        }
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Str(s) => s.clone(),
        Value::Vector { x, y, z } => format!("X={} Y={} Z={}", fixed3(*x), fixed3(*y), fixed3(*z)),
        Value::Rotator { .. } | Value::Quat { .. } | Value::Color { .. } => String::new(),
    }
}

/// Case-output comparison law — used ONLY by the replay harness below to
/// compare `eval()`'s result against the certified table's PERSISTED case
/// text, which is not always the literal in-game text (see the module-level
/// comment on why this differs from `render_for_format`): a bool-output
/// gate's case is canonicalized by `gen_semantics.mjs` to `"true"`/`"false"`
/// regardless of what the console showed, and an int-classified output has
/// already had its thousands separators stripped for storage. Everything
/// else (float rounding, vector/rotator/quat/color layout) goes through the
/// exact same law as `render_for_format` — every probed float-valued case
/// (including the composite-chapter scalar outputs like `VecDotProduct`)
/// is itself post-round, post-trim text, and no probed int-valued GATE case
/// (as opposed to the dedicated `render` section) ever exceeds 999.
pub(crate) fn render(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Float(f) => {
            let (neg, int_part, frac) = round3(*f);
            let sign = if neg { "-" } else { "" };
            if frac.is_empty() { format!("{sign}{int_part}") } else { format!("{sign}{int_part}.{frac}") }
        }
        _ => render_for_format(v),
    }
}

/// String-concatenation's OWN operand-to-text law — certified to differ from
/// both renderers above: a bool operand stringifies `"true"`/`"false"`
/// (`Concatenate(bool:true, str:"!") -> "true!"`, not `"1!"`), matching a
/// generic Blueprint `ToString` conversion rather than FormatText's
/// Text-formatting system. No probed Concatenate case exercises a
/// >3-decimal float or a >999 int, so the int/float half of this law is a
/// reasoned (not directly probed) extrapolation: plain `Display`-style
/// (`format!("{f}")`), NOT rounded/grouped, matching the same generic
/// `ToString` convention the bool case already proves is in play here.
fn concat_display(v: &Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
            if !f.is_finite() || *f == 0.0 { "0".to_string() } else { format!("{f}") }
        }
        Value::Str(s) => s.clone(),
        _ => render_for_format(v), // composites: never probed in Concatenate; same law as elsewhere
    }
}

/// i64 -> f64 is exact only within ±2^53; beyond that, refuse mixed-domain
/// numeric work (never probed at that magnitude).
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
        Some(Value::Bool(b)) => *b as i64 as f64,
        Some(Value::Str(s)) => s.parse::<f64>().map(sanitize).unwrap_or(0.0),
        Some(Value::Vector { .. } | Value::Rotator { .. }
            | Value::Quat { .. } | Value::Color { .. }) => return None, // never probed
    })
}

/// EQ/NE: certified per variant pair. Canonical-string for int-vs-str; exact
/// for str-str; numeric for int/float/bool mixes; certified exact
/// component-wise for composite-vs-same-composite (`CompareEqual`'s
/// `compositeOps` cases). Unprobed pairs return None (the coverage gate
/// blocks them anyway — this is belt and suspenders).
fn eq(a: Option<&Value>, b: Option<&Value>) -> Option<bool> {
    use Value::*;
    // Certified: unwired behaves as the other operand's domain default. No
    // composite+unwired pair is ever a certified signature (never reached in
    // practice — the coverage gate blocks it before `eq` runs), so these
    // defaults are unreachable filler, present only so the match stays total.
    let default_for = |other: &Value| -> Value {
        match other {
            Int(_) => Int(0),
            Float(_) => Float(0.0),
            Bool(_) => Bool(false),
            Str(_) => Str(String::new()),
            Vector { .. } => Vector { x: 0.0, y: 0.0, z: 0.0 },
            Rotator { .. } => Rotator { pitch: 0.0, yaw: 0.0, roll: 0.0 },
            Quat { .. } => Quat { x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
            Color { .. } => Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
        }
    };
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        (Some(a), None) => { let d = default_for(a); (a.clone(), d) }
        (None, Some(b)) => { let d = default_for(b); (d, b.clone()) }
        (None, None) => return None, // not in table
    };
    Some(match (&a, &b) {
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) => sanitize(*x) == sanitize(*y),
        (Int(x), Float(y)) | (Float(y), Int(x)) => int_as_f64(*x)? == sanitize(*y),
        (Int(x), Bool(y)) | (Bool(y), Int(x)) => *x == *y as i64,
        (Bool(x), Bool(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        // Certified (int:N str:S direction): canonical int rendering compared
        // as a string ("1"=="1" T, ""=="0"'s int-0 F, "1.0" F).
        (Int(x), Str(s)) | (Str(s), Int(x)) => &x.to_string() == s,
        (Vector { x: x1, y: y1, z: z1 }, Vector { x: x2, y: y2, z: z2 }) => {
            sanitize(*x1) == sanitize(*x2) && sanitize(*y1) == sanitize(*y2)
                && sanitize(*z1) == sanitize(*z2)
        }
        (Rotator { pitch: p1, yaw: y1, roll: r1 }, Rotator { pitch: p2, yaw: y2, roll: r2 }) => {
            sanitize(*p1) == sanitize(*p2) && sanitize(*y1) == sanitize(*y2)
                && sanitize(*r1) == sanitize(*r2)
        }
        (Quat { x: x1, y: y1, z: z1, w: w1 }, Quat { x: x2, y: y2, z: z2, w: w2 }) => {
            sanitize(*x1) == sanitize(*x2) && sanitize(*y1) == sanitize(*y2)
                && sanitize(*z1) == sanitize(*z2) && sanitize(*w1) == sanitize(*w2)
        }
        (Color { r: r1, g: g1, b: b1, a: a1 }, Color { r: r2, g: g2, b: b2, a: a2 }) => {
            sanitize(*r1) == sanitize(*r2) && sanitize(*g1) == sanitize(*g2)
                && sanitize(*b1) == sanitize(*b2) && sanitize(*a1) == sanitize(*a2)
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
enum MathIn { I(i64), F(f64) }
fn math_in(v: Option<&Value>) -> Option<MathIn> {
    Some(match v {
        None => MathIn::I(0),
        Some(Value::Int(n)) => MathIn::I(*n),
        Some(Value::Bool(b)) => MathIn::I(*b as i64),
        Some(Value::Float(f)) => MathIn::F(sanitize(*f)),
        Some(Value::Str(_)) => return None,
        Some(Value::Vector { .. } | Value::Rotator { .. }
            | Value::Quat { .. } | Value::Color { .. }) => return None, // routed to composite_math instead
    })
}

/// Scalar broadcast partner for a `[Vector, X]`/`[X, Vector]` composite math
/// op — certified for float/int (`compositeMath`'s `float`/`int` broadcast
/// cases); bool/unwired are reasoned extrapolations (never probed alongside
/// a vector operand) mirroring the plain scalar `math_in` promotion rule.
fn broadcast_scalar(v: Option<&Value>) -> Option<f64> {
    match v {
        None => Some(0.0),
        Some(Value::Int(n)) => int_as_f64(*n).map(sanitize),
        Some(Value::Float(f)) => Some(sanitize(*f)),
        Some(Value::Bool(b)) => Some(*b as i64 as f64),
        _ => None,
    }
}

/// Certified per-component op (`compositeMath`): `+ - * /` are the plain f64
/// operator; `%` is Rust's `%` (== C `fmod`, truncated-towards-zero
/// remainder, dividend's sign) applied UNCONDITIONALLY — certified directly
/// (`Vec(0.5,0.25,-0.75) % Vec(0.25,0.5,0.75) -> Z=-0.000`, a genuine mixed-
/// sign case the plain scalar `math()` below would refuse). Composite math
/// does NOT inherit the scalar path's mixed-sign modulo refusal.
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

/// `[Vector,Vector]` (component-wise) / `[Vector,scalar]` / `[scalar,Vector]`
/// (broadcast) Math* — certified (`compositeMath` chapter, 22 cases, all
/// five Math* gates). Per-component NaN/inf sanitize happens BEFORE the op
/// (certified: `Vec(NaN,1,2) + Vec(1,1,1) -> X=1.000`, i.e. NaN sanitizes to
/// 0 then adds, giving 1 — not a NaN-poisoned/refused result). A non-finite
/// RESULT component (e.g. a divide-by-zero component — never probed) is
/// refused rather than baked, mirroring `fold/mod.rs`'s belt-and-suspenders
/// non-finite guard for the plain scalar path (which only inspects
/// `Value::Float`, not composite fields, so this is implemented locally).
fn composite_math(gate: &str, a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let finite3 = |x: f64, y: f64, z: f64| (x.is_finite() && y.is_finite() && z.is_finite())
        .then_some(Value::Vector { x, y, z });
    match (a, b) {
        (Some(Value::Vector { x: x1, y: y1, z: z1 }), Some(Value::Vector { x: x2, y: y2, z: z2 })) => {
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
                if y == 0 { 0 } // certified: div by zero -> 0
                else {
                    // checked_rem first: None on i64::MIN % -1 (the same
                    // overflow that would make raw `x / y` panic below).
                    let rem = x.checked_rem(y)?;
                    // Truncation direction unprobed for mixed signs with a
                    // remainder — refuse rather than guess trunc vs floor.
                    if (x < 0) != (y < 0) && rem != 0 { return None; }
                    x.checked_div(y)?
                }
            }
            g if g.ends_with(G_MATH_MODULO) => {
                if y == 0 { 0 } // certified: mod by zero -> 0
                else {
                    // checked_rem first: None on i64::MIN % -1 refuses before
                    // any raw `%` can panic (the guard itself must not panic).
                    let rem = x.checked_rem(y)?;
                    if (x < 0 || y < 0) && rem != 0 { return None; }
                    rem
                }
            }
            _ => return None,
        }));
    }
    let to_f = |m: MathIn| -> Option<f64> {
        Some(match m { MathIn::I(n) => int_as_f64(n)?, MathIn::F(f) => f })
    };
    let (x, y) = (to_f(x)?, to_f(y)?);
    if gate.ends_with(G_MATH_MODULO) {
        let r = x % y;
        // Mirrors the int-path mixed-sign refusal: truncation direction is
        // unprobed for mixed signs with a nonzero (finite) remainder.
        if (x < 0.0) != (y < 0.0) && r != 0.0 && r.is_finite() {
            return None;
        }
        return Some(Value::Float(r));
    }
    Some(Value::Float(match gate {
        g if g.ends_with(G_MATH_ADD) => x + y,
        g if g.ends_with(G_MATH_SUBTRACT) => x - y,
        g if g.ends_with(G_MATH_MULTIPLY) => x * y,
        g if g.ends_with(G_MATH_DIVIDE) => x / y,   // non-finite result: fold
        _ => return None,                          // renders as "0"
    }))
}

// ============================================================================
// String family (v3 `strings` chapter). Every law below refuses non-ASCII
// operands/results — certified multibyte behavior was only ever probed with
// BMP characters (`"π≈3"`), which is consistent with EITHER a char-count or
// a UTF-16-code-unit model (both give the same answer for that string), so
// there is no way to certify which model the game actually uses; the 4
// multibyte cases (Length/ToLower/ToUpper/Trim) are allowlisted refusals in
// the replay test below rather than silently trusting Rust's `char`-based
// std methods for un-probed non-ASCII input.
// ============================================================================

const MAX_FOLDED_STRING_LEN: usize = 8192;
/// Certified: FormatText/string folding refuses any float operand whose
/// magnitude exceeds this — the game cannot print `1e20` (two independent
/// probe runs silently dropped the console line entirely). `1e15` itself IS
/// certified to render fine (`render` section: `float:1e15` ->
/// `"1,000,000,000,000,000"`), so the bound is exclusive.
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
        None | Some(Value::Int(_)) | Some(Value::Bool(_)) => true,
        Some(Value::Str(s)) => s.is_ascii(),
        Some(Value::Float(f)) => f.abs() <= MAX_ABS_FLOAT_FOR_STRING_FOLD,
        _ => false, // composites never appear in this family
    })
}

fn string_result_ok(s: &str) -> Option<Value> {
    (s.is_ascii() && s.len() <= MAX_FOLDED_STRING_LEN).then(|| Value::Str(s.to_string()))
}

/// `String_Concatenate`: certified `Separator` is always `""` (the compiler
/// never lowers `..` with a non-empty separator — see `lower/expr.rs`), so
/// this is a plain two-operand join using Concatenate's OWN stringification
/// law (`concat_display`, NOT `render`/`render_for_format` — certified to
/// differ on bools).
fn concatenate(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    if !string_operands_foldable(&[a, b]) { return None; }
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
/// Certified: an EMPTY needle is never "contained"/a prefix/a suffix — a
/// deliberate deviation from Rust's `str::contains("")`/`starts_with("")`/
/// `ends_with("")`, which default to `true`.
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
/// Certified: `start` indexes from the END when negative (`Substring("hello",
/// -1, 3) -> "o"`, i.e. `actual_start = max(0, len + start)`), and both
/// `start` and `start+length` clamp to the string's bounds rather than
/// erroring (`Substring("hello", 10, 3) -> ""`, `Substring("hello", 1, 100)
/// -> "ello"`). A negative LENGTH is unprobed and explicitly refused rather
/// than guessed.
fn string_substring(s: Option<&Value>, start: Option<&Value>, len: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    let (Some(Value::Int(start)), Some(Value::Int(len))) = (start, len) else { return None };
    if *len < 0 { return None; } // unprobed — refuse rather than guess
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let actual_start = if *start < 0 { (n + *start).max(0) } else { (*start).min(n) };
    let end = (actual_start + *len).min(n);
    let slice: String = chars[actual_start as usize..end as usize].iter().collect();
    string_result_ok(&slice)
}
/// Certified: an EMPTY needle is never found (`Find(s, "") -> -1`, matching
/// `Contains`'s empty-needle law rather than Rust's `str::find("")` (which
/// finds index 0)).
fn string_find(s: Option<&Value>, needle: Option<&Value>) -> Option<Value> {
    let (s, needle) = (ascii_str(s?)?, ascii_str(needle?)?);
    if needle.is_empty() { return Some(Value::Int(-1)); }
    Some(Value::Int(s.find(needle).map_or(-1, |i| i as i64)))
}
/// Certified: an EMPTY search string is a no-op (`Replace(s, "", x) -> s`
/// unchanged — a deliberate deviation from Rust's `str::replace("", x)`,
/// which inserts `x` between every character). Replace-first vs
/// replace-all is unprobed beyond 0/1 occurrences (every probed case has
/// exactly one or zero matches, where the two strategies agree) — refuse
/// when the search string appears more than once rather than guess.
fn string_replace(s: Option<&Value>, search: Option<&Value>, repl: Option<&Value>) -> Option<Value> {
    let (s, search, repl) = (ascii_str(s?)?, ascii_str(search?)?, ascii_str(repl?)?);
    if search.is_empty() { return string_result_ok(s); }
    if s.matches(search).count() >= 2 { return None; } // replace-first vs -all unprobed
    string_result_ok(&s.replace(search, repl))
}
/// Certified: whitespace-trimmed, strict-integer-only text parses; anything
/// else (including a syntactically-numeric float like `"1.5"`) -> 0.
fn string_parse_int(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    Some(Value::Int(s.trim().parse::<i64>().unwrap_or(0)))
}
/// Certified: ALWAYS parses through f64 first (even a no-dot integer-
/// looking string loses precision beyond 2^53 —
/// `"9007199254740993" -> 9007199254740992`, exactly the nearest f64,
/// proving there's no separate exact-integer fast path). The input text's
/// own shape (contains `.`/`e`/`E`) then decides whether the WIRE result is
/// tagged int (cast the parsed double to i64) or float (kept as-is) —
/// independent of whether the parsed value happens to be whole. `"inf"`/
/// `"nan"`-shaped text is refused: Rust's `f64::from_str` accepts those
/// spellings but the game's parser was never probed on them, and blindly
/// reusing Rust's acceptance here is a real divergence risk (an `as i64`
/// cast of infinity would silently saturate rather than replicate whatever
/// the game does).
fn string_parse_number(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();
    let lower = lower.strip_prefix(['+', '-']).unwrap_or(&lower);
    if lower == "inf" || lower == "infinity" || lower == "nan" { return None; }
    let has_dot_or_exp = trimmed.contains(['.', 'e', 'E']);
    let f = trimmed.parse::<f64>().unwrap_or(0.0);
    if !f.is_finite() { return None; } // belt-and-suspenders alongside the inf/nan text guard
    Some(if has_dot_or_exp { Value::Float(sanitize(f)) } else { Value::Int(f as i64) })
}

/// FormatText's real certified law: substitute `{0}`..`{6}` with
/// `render_for_format()` of the corresponding input (an unbound slot — out
/// of range, non-numeric like `{a}`, or wired-but-unwired — renders `"0"`,
/// certified by the `unwiredslot`/`literalbrace` probe cases), and decode
/// `{{`/`}}` as escaped literal braces (matching `lower/ops.rs::lower_interp`'s
/// OWN escaping convention for literal template text — not itself a
/// probed-case fact, but required for consistency with how a real
/// `${...}`-interpolated template is actually built).
///
/// NOT reachable via the generic per-node `eval()` dispatch above: the
/// format template lives in the node's `FormatString` PROPERTY
/// (`lower/ops.rs::build_format_text`), never as a wire input port, and
/// separately, this gate's only ever-recorded table SIGNATURE is `[Tmpl]`
/// (one synthetic label per probe case), which no real `Value`-derived
/// signature can ever match, so `covers()` refuses it unconditionally
/// regardless. Production folding calls this function directly instead,
/// after resolving the template property + wired substitution slots itself
/// (`fold/mod.rs::try_resolve_format_text`). Tested directly here against
/// the templates+operands recovered from `probes/gate_semantics.ws` (the
/// PERSISTED table only carries a synthetic label per FormatText case, not
/// the real template/operands — see `replay_every_certified_case`'s
/// FormatText allowlist comment).
///
/// `pub` (not `pub(crate)`): reachable as `wirescript::lower::fold::eval::format_text`
/// from the `--fold-diff` fuzz harness's own `predict_format_text`, which
/// mirrors `try_resolve_format_text`'s template/slot resolution to predict
/// `${...}`-interpolation the same way `eval`/`Value` are already exposed
/// for the rest of the predictor — see the visibility note on `pub mod fold`
/// in `lower/mod.rs`.
pub fn format_text(template: &str, inputs: &[Option<Value>]) -> Option<String> {
    if !template.is_ascii() { return None; }
    if !string_operands_foldable(&inputs.iter().map(Option::as_ref).collect::<Vec<_>>()) {
        return None;
    }
    let bytes = template.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if template[i..].starts_with("{{") { out.push('{'); i += 2; continue; }
        if template[i..].starts_with("}}") { out.push('}'); i += 2; continue; }
        if bytes[i] == b'{' {
            let Some(rel) = template[i + 1..].find('}') else { return None }; // unterminated — unprobed
            let tag = &template[i + 1..i + 1 + rel];
            let rendered = match tag.parse::<usize>() {
                Ok(slot) if slot < 7 => match inputs.get(slot) {
                    Some(Some(v)) => render_for_format(v),
                    _ => "0".to_string(), // unwired/out-of-range slot — certified
                },
                _ => "0".to_string(), // non-numeric tag (e.g. "{a}") — certified
            };
            out.push_str(&rendered);
            i += rel + 2;
            continue;
        }
        out.push(bytes[i] as char); // safe: template.is_ascii() already checked
        i += 1;
    }
    if out.len() > MAX_FOLDED_STRING_LEN || !out.is_ascii() { return None; }
    Some(out)
}

// ============================================================================
// Composite constructors/ops (v3 `compositeOps` chapter).
// ============================================================================

/// sRGB (gamma-encoded, 0-1) -> linear (0-1) — certified via `ColorToHex`'s
/// exact inverse (see `linear_to_srgb`'s doc comment for the round-trip
/// check: linear 0.5 -> sRGB byte 188/0xBC, standard IEC 61966-2-1 sRGB).
fn srgb_to_linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
/// Linear (0-1) -> sRGB (gamma-encoded, 0-1). Certified: `ColorToHex(Color(
/// 1.0, 0.5, 0.0)) -> "FFBC00"` — linear 0.5 -> 0.7354 -> ×255 -> 187.53 ->
/// rounds to 188 = 0xBC, exactly the standard sRGB transfer function.
fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// FOLD-ELIGIBLE — directly certified: unlike the rotator/quat/color
/// constructors below, `MakeVector`'s own case output is value-bearing
/// (vectors render `X=.. Y=.. Z=..` through FormatText, never blank — see
/// `render_for_format`), so the recorded case (`MakeVector(1.5,-2.5,0.75)`
/// -> `X=1.500 Y=-2.500 Z=0.750`) directly certifies this mapping (plain
/// field packing, arg order preserved) with no need for a transitive proof
/// through another gate.
fn make_vector(a: Option<&Value>, b: Option<&Value>, c: Option<&Value>) -> Option<Value> {
    let f = |v: Option<&Value>| match v { Some(Value::Float(f)) => Some(sanitize(*f)), _ => None };
    let (x, y, z) = (f(a)?, f(b)?, f(c)?);
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
/// MUST-REFUSE in production (see `BLANK_RENDER_REFUSED` in `eval()`, which
/// short-circuits before this function is ever actually called there).
/// `MakeRotation`'s only table evidence is a single case whose output
/// renders blank — rotators never render through FormatText (certified —
/// see `render_for_format`) — so replaying it only proves blank==blank,
/// regardless of whether this formula (plain field packing) is right. And
/// unlike `MakeQuaternion` below, no OTHER certified gate consumes a
/// `Rotator` operand in a way that would verify it transitively either.
/// Kept implemented (and the `eval()` match arm stays wired to it) so it's
/// ready the moment a future probe wave chains rotators through a
/// rendering gate — see `BLANK_RENDER_REFUSED`'s doc comment.
fn make_rotation(p: Option<&Value>, y: Option<&Value>, r: Option<&Value>) -> Option<Value> {
    let f = |v: Option<&Value>| match v { Some(Value::Float(f)) => Some(sanitize(*f)), _ => None };
    let (pitch, yaw, roll) = (f(p)?, f(y)?, f(r)?);
    (pitch.is_finite() && yaw.is_finite() && roll.is_finite())
        .then_some(Value::Rotator { pitch, yaw, roll })
}
/// FOLD-ELIGIBLE — certified TRANSITIVELY, not directly: `MakeQuaternion`'s
/// own case output renders blank (quats never render through FormatText),
/// but two OTHER certified gates consume a quat operand and produce a
/// value-bearing (non-blank) result: `RotateVector` (3 cases, including
/// the non-axis-aligned 45-degree case) and `QuatDotProduct` (the 0.707
/// case). A wrong arg order or transposed component here would visibly
/// diverge those gates' certified outputs, so their exact replay certifies
/// this constructor by proxy even though its own case output can't.
fn make_quaternion(a: Option<&Value>, b: Option<&Value>, c: Option<&Value>, d: Option<&Value>) -> Option<Value> {
    let f = |v: Option<&Value>| match v { Some(Value::Float(f)) => Some(sanitize(*f)), _ => None };
    let (x, y, z, w) = (f(a)?, f(b)?, f(c)?, f(d)?);
    (x.is_finite() && y.is_finite() && z.is_finite() && w.is_finite())
        .then_some(Value::Quat { x, y, z, w })
}
/// FOLD-ELIGIBLE (IN ISOLATION), 3-ARG FORM ONLY — certified TRANSITIVELY
/// via `ColorToHex(Color(1.0,0.5,0.0)) -> "FFBC00"`: that round-trip
/// recovers the exact r/g/b channel order and magnitude through a
/// value-bearing (string) result, even though `MakeColor`'s own case output
/// renders blank. Alpha has NO such evidence — `ColorToHex` drops it
/// entirely from its output — so the 4-arg form (alpha explicitly wired)
/// hard-refuses below rather than folding an unconfirmed channel.
///
/// CORRECTION (T3 review): "3-arg form is fold-eligible" does NOT mean a
/// real, wired 3-arg `Color(r, g, b)` call actually folds in production —
/// it never does. `catalog/calls.rs`'s `Color` CallSpec declares `a` as an
/// OPTIONAL 4th param, and `lower/call.rs::lower_builtin_call` always
/// declares a port for every param regardless of whether an argument was
/// passed (see `try_resolve_format_text`'s doc comment in `fold/mod.rs` for
/// the same mechanic on `Fmt`), so a real 3-arg call's MakeColor gate always
/// presents a FOUR-input signature with the alpha slot `Unwired` —
/// `[Float, Float, Float, Unwired]`. The certified table's ONLY MakeColor
/// case is the 4-arg form (`Color(1.0, 0.5, 0.25, 0.75)`, all four wired);
/// `[Float, Float, Float, Unwired]` was never probed and so is never
/// `covers()`-eligible, meaning the driver's coverage gate (`fold/mod.rs`'s
/// `try_resolve`, checked BEFORE `eval()` is ever called) refuses every
/// real 3-arg `Color(...)` call before this function's own "alpha unwired"
/// branch is ever reached. Safe over-refusal — not a bug, just dead code in
/// production until/unless a future probe wave certifies the 3-input
/// signature directly.
fn make_color(a: Option<&Value>, b: Option<&Value>, c: Option<&Value>, d: Option<&Value>) -> Option<Value> {
    if d.is_some() { return None; } // 4-arg (explicit alpha): unconfirmed, refuse
    let f = |v: Option<&Value>| match v { Some(Value::Float(f)) => Some(sanitize(*f)), _ => None };
    let (r, g, b_) = (f(a)?, f(b)?, f(c)?);
    (r.is_finite() && g.is_finite() && b_.is_finite())
        .then_some(Value::Color { r, g, b: b_, a: 1.0 })
}
/// MUST-REFUSE in production (see `BLANK_RENDER_REFUSED` in `eval()`, which
/// short-circuits before this function is ever actually called there).
/// `MakeColorSRGB`'s only table evidence is a single case whose output
/// renders blank (colors never render through FormatText). Its formula
/// (`srgb_to_linear`) is the mathematical inverse of `color_to_hex`'s
/// certified `linear_to_srgb` curve, but that's a REASONED inverse, not an
/// independently probed one — no certified case round-trips SRGB bytes IN
/// through this gate and back OUT through `ColorToHex` the way `MakeColor`
/// is certified via that exact round-trip. Kept implemented so it's ready
/// once such a round-trip case exists.
fn make_color_srgb(a: Option<&Value>, b: Option<&Value>, c: Option<&Value>, d: Option<&Value>) -> Option<Value> {
    let byte = |v: Option<&Value>| match v {
        Some(Value::Int(n)) if (0..=255).contains(n) => Some(*n as f64 / 255.0),
        _ => None, // out-of-range byte: unprobed, refuse rather than clamp/guess
    };
    let (r, g, b_, a_) = (byte(a)?, byte(b)?, byte(c)?, byte(d)?);
    Some(Value::Color { r: srgb_to_linear(r), g: srgb_to_linear(g), b: srgb_to_linear(b_), a: a_ })
}
/// MUST-REFUSE in production (see `BLANK_RENDER_REFUSED` in `eval()`, which
/// short-circuits before this function is ever actually called there).
/// `MakeColorHex`'s only table evidence is a single case whose output
/// renders blank (colors never render through FormatText) — its INPUT
/// shape (`"#RRGGBB"`, alpha defaulting to opaque) is certified by that
/// case's operand, but the resulting *Color value* is not, for the same
/// reason `make_color_srgb` isn't: no certified case round-trips this
/// gate's output back out through `ColorToHex`. Kept implemented so it's
/// ready once such a round-trip case exists.
fn make_color_hex(s: Option<&Value>) -> Option<Value> {
    let s = ascii_str(s?)?;
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| v as f64 / 255.0);
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    Some(Value::Color { r: srgb_to_linear(r), g: srgb_to_linear(g), b: srgb_to_linear(b), a: 1.0 })
}
/// Certified: `ColorToHex(Color(1.0,0.5,0.0)) -> "FFBC00"` — RGB only
/// (alpha dropped), uppercase, 2 hex digits per channel, gamma-encoded
/// (`linear_to_srgb`) and rounded to the nearest byte.
fn color_to_hex(c: Option<&Value>) -> Option<Value> {
    let Some(Value::Color { r, g, b, .. }) = c else { return None };
    let byte = |c: f64| -> Option<u8> {
        let v = (linear_to_srgb(sanitize(c)) * 255.0).round();
        (0.0..=255.0).contains(&v).then_some(v as u8)
    };
    let (r, g, b) = (byte(*r)?, byte(*g)?, byte(*b)?);
    Some(Value::Str(format!("{r:02X}{g:02X}{b:02X}")))
}

fn vec_scale(v: Option<&Value>, s: Option<&Value>) -> Option<Value> {
    let (Some(Value::Vector { x, y, z }), Some(Value::Float(s))) = (v, s) else { return None };
    let s = sanitize(*s);
    let (x, y, z) = (sanitize(*x) * s, sanitize(*y) * s, sanitize(*z) * s);
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
fn vec_dot(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (Some(Value::Vector { x: x1, y: y1, z: z1 }), Some(Value::Vector { x: x2, y: y2, z: z2 })) = (a, b)
    else { return None };
    let d = sanitize(*x1) * sanitize(*x2) + sanitize(*y1) * sanitize(*y2) + sanitize(*z1) * sanitize(*z2);
    d.is_finite().then_some(Value::Float(d))
}
fn vec_cross(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (Some(Value::Vector { x: x1, y: y1, z: z1 }), Some(Value::Vector { x: x2, y: y2, z: z2 })) = (a, b)
    else { return None };
    let (x1, y1, z1) = (sanitize(*x1), sanitize(*y1), sanitize(*z1));
    let (x2, y2, z2) = (sanitize(*x2), sanitize(*y2), sanitize(*z2));
    let (x, y, z) = (y1 * z2 - z1 * y2, z1 * x2 - x1 * z2, x1 * y2 - y1 * x2);
    (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Value::Vector { x, y, z })
}
fn vec_magnitude_sq(v: Option<&Value>) -> Option<Value> {
    let Some(Value::Vector { x, y, z }) = v else { return None };
    let (x, y, z) = (sanitize(*x), sanitize(*y), sanitize(*z));
    let m = x * x + y * y + z * z;
    m.is_finite().then_some(Value::Float(m))
}
fn vec_distance_sq(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (Some(Value::Vector { x: x1, y: y1, z: z1 }), Some(Value::Vector { x: x2, y: y2, z: z2 })) = (a, b)
    else { return None };
    let (dx, dy, dz) = (sanitize(*x1) - sanitize(*x2), sanitize(*y1) - sanitize(*y2), sanitize(*z1) - sanitize(*z2));
    let d = dx * dx + dy * dy + dz * dz;
    d.is_finite().then_some(Value::Float(d))
}
/// Quaternion vector rotation, using Unreal's OWN `FQuat::RotateVector`
/// operation order (`T = 2*cross(u,v); result = v + s*T + cross(u,T)`,
/// where `u` = the quaternion's `(x,y,z)` and `s` = `w`) rather than the
/// (algebraically equivalent) textbook `v*(s^2-dot(u,u)) + u*2*dot(u,v) +
/// cross(u,v)*2*s` form: the two formulas round differently in f64, and
/// only Unreal's own operand order reproduces the certified 90-degree case
/// EXACTLY, including its rounding noise (`Vec(1,0,0)` by `Quat(0,0,
/// 0.7071...,0.7071...)` renders `X=-0.000`, i.e. a tiny negative residual
/// from cancellation — the textbook form computes a clean `0.0` there
/// instead, which prints `X=0.000` and fails replay). Certified exact
/// against all 3 `RotateVector` probe cases: that 90-degree case, a
/// 180-degree case, AND the non-axis-aligned 45-degree case (`Vec(1,0,0)`
/// by `Quat(0,0,0.3826...,0.9238...)` -> `X=0.707 Y=0.707 Z=0.000`) — so
/// this is NOT restricted to axis-aligned rotations.
fn rotate_vector(v: Option<&Value>, q: Option<&Value>) -> Option<Value> {
    let (Some(Value::Vector { x: vx, y: vy, z: vz }), Some(Value::Quat { x: qx, y: qy, z: qz, w: qw })) = (v, q)
    else { return None };
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
/// MUST-REFUSE in production (see `BLANK_RENDER_REFUSED` in `eval()`, which
/// short-circuits before this function is ever actually called there).
/// Quaternion inverse: `conjugate / |q|^2` — the mathematically-standard
/// formula, not a probed law. `InvertRotation`'s case output — like every
/// rotator/quat case output — renders blank (see `render_for_format`'s doc
/// comment), and unlike `MakeQuaternion`, no certified case chains this
/// gate's OUTPUT into a value-bearing gate (e.g. feeding an inverted quat
/// into `RotateVector`) either, so there is zero transitive evidence.
fn invert_rotation(q: Option<&Value>) -> Option<Value> {
    let Some(Value::Quat { x, y, z, w }) = q else { return None };
    let (x, y, z, w) = (sanitize(*x), sanitize(*y), sanitize(*z), sanitize(*w));
    let norm_sq = x * x + y * y + z * z + w * w;
    if norm_sq == 0.0 || !norm_sq.is_finite() { return None; }
    let (ix, iy, iz, iw) = (-x / norm_sq, -y / norm_sq, -z / norm_sq, w / norm_sq);
    (ix.is_finite() && iy.is_finite() && iz.is_finite() && iw.is_finite())
        .then_some(Value::Quat { x: ix, y: iy, z: iz, w: iw })
}
fn quat_dot(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (Some(Value::Quat { x: x1, y: y1, z: z1, w: w1 }), Some(Value::Quat { x: x2, y: y2, z: z2, w: w2 })) = (a, b)
    else { return None };
    let d = sanitize(*x1) * sanitize(*x2) + sanitize(*y1) * sanitize(*y2)
        + sanitize(*z1) * sanitize(*z2) + sanitize(*w1) * sanitize(*w2);
    d.is_finite().then_some(Value::Float(d))
}

// `pub`: the `--fold-diff` fuzz harness's predictor calls this directly
// instead of re-implementing the certified value laws — see the visibility
// note on `pub mod fold` in `lower/mod.rs`.
pub fn eval(gate_class: &str, inputs: &[Option<Value>]) -> Option<Value> {
    // Coverage gate: refuse any input-shape the in-game probe never actually
    // observed for this gate class (unwired operands on unprobed gates, the
    // un-probed direction of an asymmetric compare, Bool in ordered compares,
    // NOT on non-bool, ...). This makes eval exactly as permissive as the
    // certified table, no more — every law below only ever sees a signature
    // that was actually certified. (A Task-3 driver-side coverage check is a
    // deliberately redundant second layer — belt and suspenders by design.)
    let sig: Vec<InVariant> = inputs
        .iter()
        .map(|v| v.as_ref().map_or(InVariant::Unwired, Value::variant))
        .collect();
    if !crate::lower::fold::table::CertifiedTable::certified().covers(gate_class, &sig) {
        return None;
    }

    // `deferredOps` chapter: probed but NEVER folded — hard refusal
    // regardless of signature coverage (allowlisted wholesale in the replay
    // test below). See `DEFERRED`'s own doc comment above for the rationale
    // and why the list lives at module scope.
    let short = gate_class.rsplit('_').next().unwrap_or("");
    if DEFERRED.contains(&short) {
        return None;
    }

    // MUST-REFUSE — blank-render-only evidence, ZERO transitive
    // certification. See `BLANK_RENDER_REFUSED`'s own doc comment above for
    // the rationale (incl. which gates are transitively fold-eligible
    // despite ALSO having a blank own-case output: `MakeQuaternion` /
    // `MakeColor`'s 3-arg form).
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
        G_MAKE_VECTOR => make_vector(a, b, c)?, // fold-eligible: directly certified, see make_vector
        G_MAKE_ROTATION => make_rotation(a, b, c)?, // unreachable: refused above by BLANK_RENDER_REFUSED
        G_MAKE_QUATERNION => make_quaternion(a, b, c, d)?, // fold-eligible: transitively certified, see make_quaternion
        G_MAKE_COLOR => make_color(a, b, c, d)?, // fold-eligible (3-arg only): transitively certified, see make_color
        G_MAKE_COLOR_SRGB => make_color_srgb(a, b, c, d)?, // unreachable: refused above by BLANK_RENDER_REFUSED
        G_MAKE_COLOR_HEX => make_color_hex(a)?, // unreachable: refused above by BLANK_RENDER_REFUSED
        G_COLOR_TO_HEX => color_to_hex(a)?,
        G_VEC_SCALE => vec_scale(a, b)?,
        G_VEC_DOT_PRODUCT => vec_dot(a, b)?,
        G_VEC_CROSS_PRODUCT => vec_cross(a, b)?,
        G_VEC_MAGNITUDE_SQUARED => vec_magnitude_sq(a)?,
        G_VEC_DISTANCE_SQUARED => vec_distance_sq(a, b)?,
        G_ROTATE_VECTOR => rotate_vector(a, b)?,
        G_INVERT_ROTATION => invert_rotation(a)?, // unreachable: refused above by BLANK_RENDER_REFUSED
        G_QUAT_DOT_PRODUCT => quat_dot(a, b)?,
        // FormatText's only recorded signature ([Tmpl]) is never producible
        // by a real Value, so `covers()` already refused above — this arm
        // is unreachable, kept only so the match's `_` fallback below stays
        // honest about what's actually dispatched.
        _ => return None, // Select/Branch handled by the driver; rest unknown
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::fold::table::{CaseValue, CertifiedTable, InVariant};

    const SELECT: &str = "BrickComponentType_WireGraph_Expr_Select";
    const BRANCH: &str = "BrickComponentType_WireGraph_Exec_Branch";
    const FORMAT_TEXT: &str = "BrickComponentType_WireGraph_Expr_String_FormatText";

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
        assert_eq!(replayed + refused + blank, 360, "table case count changed — re-audit");
        assert_eq!(replayed, 326);
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
}
