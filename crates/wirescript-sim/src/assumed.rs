//! Laws for pure gates no probe has measured yet, written from the game's
//! description of each gate. `projects/sim/ROADMAP.md` lists them under
//! "Assumed, not measured", and `probes/20_assumed_gates.ws` checks them in
//! game.

use std::f64::consts::PI;
use std::fmt::Write as _;

use wirescript_ir::semantics;

use crate::graph::Gate;
use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Law {
    Blend,
    Easing,
    Remap,
    ShiftRightLogical,
    NearlyEqual,
    Swap,
    CharacterToCodepoint,
    CodepointToCharacter,
    Split,
    QuatToAxisAngle,
    SplitQuaternion,
    SplitRotation,
    SplitVector,
    Convert,
    SplitColor,
    SplitColorSrgb,
    ColorConvert,
    GetValueType,
    FormatDate,
    ItemToPickup,
}

type Outputs = Vec<(&'static str, Option<Value>)>;

impl Law {
    /// The law for a class name without its `BrickComponentType_` prefix.
    pub fn of(short: &str) -> Option<Self> {
        Some(match short {
            "Expr_MathBlend" => Self::Blend,
            "Expr_MathEasing" => Self::Easing,
            "Expr_Remap" => Self::Remap,
            "Expr_BitwiseShiftRightLogical" => Self::ShiftRightLogical,
            "Expr_NearlyEqual" => Self::NearlyEqual,
            "Expr_Swap" => Self::Swap,
            "Expr_String_CharacterToCodepoint" => Self::CharacterToCodepoint,
            "Expr_String_CodepointToCharacter" => Self::CodepointToCharacter,
            "Expr_String_Split" => Self::Split,
            "Expr_QuatToAxisAngle" => Self::QuatToAxisAngle,
            "Expr_SplitQuaternion" => Self::SplitQuaternion,
            "Expr_SplitRotation" => Self::SplitRotation,
            "Expr_SplitVector" => Self::SplitVector,
            "Expr_Convert" => Self::Convert,
            "Expr_SplitColor" => Self::SplitColor,
            "Expr_SplitColorSRGB" => Self::SplitColorSrgb,
            "Expr_ColorConvert" => Self::ColorConvert,
            "Expr_GetValueType" => Self::GetValueType,
            "Expr_FormatDate" => Self::FormatDate,
            "Expr_ItemToPickup" => Self::ItemToPickup,
            _ => return None,
        })
    }

    /// The input ports `eval` reads, in the order it takes them.
    pub fn inputs(self) -> &'static [&'static str] {
        match self {
            Self::Blend | Self::Easing => &["InputA", "InputB", "Blend"],
            Self::Remap => &["Value", "InputMin", "InputMax", "OutputMin", "OutputMax"],
            Self::ShiftRightLogical => &["InputA", "InputB"],
            Self::NearlyEqual => &["InputA", "InputB", "Tolerance"],
            Self::Swap => &["InputA", "InputB", "bSwap"],
            Self::CharacterToCodepoint => &["Character"],
            Self::CodepointToCharacter => &["Codepoint"],
            Self::Split => &["Input", "Delimiter", "Occurrence", "bCaseSensitive"],
            Self::QuatToAxisAngle
            | Self::SplitQuaternion
            | Self::SplitRotation
            | Self::SplitVector
            | Self::Convert
            | Self::SplitColor
            | Self::SplitColorSrgb
            | Self::ColorConvert
            | Self::GetValueType
            | Self::ItemToPickup => &["Input"],
            Self::FormatDate => &["UnixEpoch", "Format", "bUseUTC"],
        }
    }
}

/// Every output `law` gives for `inputs` (read in [`Law::inputs`] order), by
/// port name.
pub fn eval(law: Law, gate: &Gate, inputs: &[Option<Value>]) -> Result<Outputs, String> {
    let at = |i: usize| inputs.get(i).cloned().flatten();
    let arg = |i: usize| inputs.get(i).and_then(Option::as_ref);
    Ok(match law {
        Law::Blend => {
            let mut t = num(arg(2))?;
            if gate.field_bool("bClampAlpha").unwrap_or(false) {
                t = t.clamp(0.0, 1.0);
            }
            vec![("Output", Some(lerp(at(0), at(1), t)?))]
        }
        Law::Easing => {
            let t = ease(easing_of(gate), num(arg(2))?.clamp(0.0, 1.0));
            vec![("Output", Some(lerp(at(0), at(1), t)?))]
        }
        Law::Remap => {
            let (v, in_min, in_max) = (num(arg(0))?, num(arg(1))?, num(arg(2))?);
            let (out_min, out_max) = (num(arg(3))?, num(arg(4))?);
            let t = if in_max == in_min { 0.0 } else { (v - in_min) / (in_max - in_min) };
            // P20a: 150 in 0..100 with Quad In gives 1.5, so the curve shapes
            // only the range itself and a value past it continues in a line.
            let shaped = if (0.0..=1.0).contains(&t) { ease(easing_of(gate), t) } else { t };
            let r = out_min + shaped * (out_max - out_min);
            vec![("Output", Some(Value::Float(semantics::sanitize(r))))]
        }
        Law::ShiftRightLogical => {
            let s = int(arg(1))?;
            if !(0..64).contains(&s) {
                return Err(format!("a shift of {s} is outside 0 to 63"));
            }
            vec![("Output", Some(Value::Int(((int(arg(0))? as u64) >> s) as i64)))]
        }
        Law::NearlyEqual => {
            let tol = num(arg(2))?.max(0.0);
            let (a, b) = (components(at(0), arg(1))?, components(at(1), arg(0))?);
            if a.len() != b.len() {
                return Err("Nearly Equal of two different kinds of value".into());
            }
            let equal = a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= tol);
            vec![("bOutput", Some(Value::Bool(equal)))]
        }
        Law::Swap => {
            let (a, b) = (at(0), at(1));
            let swap = at(2).is_some_and(|v| v.truthy());
            let (first, second) = if swap { (b, a) } else { (a, b) };
            vec![("Output", first), ("OutputB", second)]
        }
        Law::CharacterToCodepoint => {
            let c = text(arg(0))?.chars().next();
            vec![
                ("Codepoint", Some(Value::Int(c.map_or(0, |c| i64::from(u32::from(c)))))),
                ("bSuccess", Some(Value::Bool(c.is_some()))),
            ]
        }
        Law::CodepointToCharacter => {
            let c = u32::try_from(int(arg(0))?)
                .ok()
                .filter(|&n| n != 0)
                .and_then(char::from_u32);
            vec![
                ("Character", Some(Value::Str(c.map(String::from).unwrap_or_default()))),
                ("bSuccess", Some(Value::Bool(c.is_some()))),
            ]
        }
        Law::Split => {
            let s = text(arg(0))?;
            let delim = text(arg(1))?;
            let occurrence = int(arg(2))?;
            let case_sensitive = at(3).is_some_and(|v| v.truthy());
            match split(&s, &delim, occurrence, case_sensitive) {
                Some((l, r)) => vec![
                    ("Left", Some(Value::Str(l))),
                    ("Right", Some(Value::Str(r))),
                    ("bFound", Some(Value::Bool(true))),
                ],
                // P20d: not found leaves the whole text on the left.
                None => vec![
                    ("Left", Some(Value::Str(s))),
                    ("Right", Some(Value::Str(String::new()))),
                    ("bFound", Some(Value::Bool(false))),
                ],
            }
        }
        Law::QuatToAxisAngle => {
            let (x, y, z, w) = quat(arg(0))?;
            let len = (x * x + y * y + z * z + w * w).sqrt();
            let (x, y, z, w) = if len > 0.0 { (x / len, y / len, z / len, w / len) } else { (0.0, 0.0, 0.0, 1.0) };
            let angle = 2.0 * w.clamp(-1.0, 1.0).acos();
            let s = (1.0 - w * w).max(0.0).sqrt();
            let axis = if s >= 1e-4 {
                Value::Vector { x: x / s, y: y / s, z: z / s }
            } else {
                Value::Vector { x: 1.0, y: 0.0, z: 0.0 }
            };
            vec![("Angle", Some(Value::Float(angle.to_degrees()))), ("Axis", Some(axis))]
        }
        Law::SplitQuaternion => {
            let (x, y, z, w) = quat(arg(0))?;
            vec![
                ("X", Some(Value::Float(x))),
                ("Y", Some(Value::Float(y))),
                ("Z", Some(Value::Float(z))),
                ("W", Some(Value::Float(w))),
            ]
        }
        Law::SplitRotation => {
            let (pitch, yaw, roll) = match at(0) {
                None => (0.0, 0.0, 0.0),
                Some(Value::Rotator { pitch, yaw, roll }) => (pitch, yaw, roll),
                Some(v) => return Err(format!("Split Rotation of a {}", v.kind())),
            };
            vec![
                ("Pitch", Some(Value::Float(pitch))),
                ("Yaw", Some(Value::Float(yaw))),
                ("Roll", Some(Value::Float(roll))),
            ]
        }
        Law::SplitVector => {
            let (x, y, z) = match at(0) {
                None => (0.0, 0.0, 0.0),
                Some(Value::Vector { x, y, z }) => (x, y, z),
                Some(v) => return Err(format!("Split Vector of a {}", v.kind())),
            };
            vec![
                ("X", Some(Value::Float(x))),
                ("Y", Some(Value::Float(y))),
                ("Z", Some(Value::Float(z))),
            ]
        }
        Law::Convert => vec![("Output", convert(at(0), gate.field_value("TargetType"))?)],
        Law::SplitColor => {
            let (r, g, b, a) = color(arg(0))?;
            vec![
                ("R", Some(Value::Float(r))),
                ("G", Some(Value::Float(g))),
                ("B", Some(Value::Float(b))),
                ("A", Some(Value::Float(a))),
            ]
        }
        Law::SplitColorSrgb => {
            let (r, g, b, a) = color(arg(0))?;
            let byte = |c: f64| Some(Value::Int((semantics::linear_to_srgb(c) * 255.0).round() as i64));
            vec![
                ("R", byte(r)),
                ("G", byte(g)),
                ("B", byte(b)),
                ("A", Some(Value::Int((a.clamp(0.0, 1.0) * 255.0).round() as i64))),
            ]
        }
        Law::ColorConvert => {
            let c = color(arg(0))?;
            let from = gate.field_i64("FromSpace").unwrap_or(0);
            let to = gate.field_i64("ToSpace").unwrap_or(0);
            let (r, g, b, a) = from_linear(to, to_linear(from, c));
            vec![("Output", Some(Value::Color { r, g, b, a }))]
        }
        // The pickup of an item is named by the item itself.
        Law::ItemToPickup => vec![("Output", at(0))],
        Law::GetValueType => vec![("Type", Some(Value::Int(wire_kind(at(0).as_ref()))))],
        Law::FormatDate => {
            let epoch = int(arg(0))?;
            let format = text(arg(1))?;
            vec![
                ("Output", Some(Value::Str(format_date(epoch, &format)))),
                ("bSuccess", Some(Value::Bool(true))),
            ]
        }
    })
}

fn num(v: Option<&Value>) -> Result<f64, String> {
    match v {
        None => Ok(0.0),
        Some(v) => v.as_float().ok_or_else(|| format!("a {} where a number goes", v.kind())),
    }
}

fn int(v: Option<&Value>) -> Result<i64, String> {
    match v {
        None => Ok(0),
        Some(v) => v.as_int().ok_or_else(|| format!("a {} where an integer goes", v.kind())),
    }
}

fn text(v: Option<&Value>) -> Result<String, String> {
    match v {
        None => Ok(String::new()),
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(v) => Err(format!("a {} where text goes", v.kind())),
    }
}

fn color(v: Option<&Value>) -> Result<(f64, f64, f64, f64), String> {
    match v {
        None => Ok((0.0, 0.0, 0.0, 1.0)),
        Some(Value::Color { r, g, b, a }) => Ok((*r, *g, *b, *a)),
        Some(v) => Err(format!("a {} where a color goes", v.kind())),
    }
}

/// The schema's `EWireKind` for a value: Bool 0, Integer 1, Double 2, Color 3,
/// Vector 4, Rotator 5, Quat 6, String 7, Object 9, Brick 10, Component 11.
/// An unwired input holds the save's default number.
fn wire_kind(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Bool(_)) => 0,
        Some(Value::Int(_)) => 1,
        None | Some(Value::Float(_)) => 2,
        Some(Value::Color { .. }) => 3,
        Some(Value::Vector { .. }) => 4,
        Some(Value::Rotator { .. }) => 5,
        Some(Value::Quat { .. }) => 6,
        Some(Value::Str(_)) => 7,
        Some(Value::Object(_)) => 9,
        Some(Value::Brick(_)) => 10,
        Some(Value::Ref(_) | Value::Handle(_)) => 11,
        Some(Value::Poison(_)) => 2,
    }
}

type Rgba = (f64, f64, f64, f64);

/// `c` in color space `space` (`EBRColorSpace`: Linear 0, `sRGB` 1, `OkLab` 2,
/// HSV 3) as linear RGB. P22e: HSV is of the linear channels, its hue a
/// fraction of a turn in R.
fn to_linear(space: i64, (r, g, b, a): Rgba) -> Rgba {
    match space {
        1 => (semantics::srgb_to_linear(r), semantics::srgb_to_linear(g), semantics::srgb_to_linear(b), a),
        2 => {
            let l_ = r + 0.396_337_777_4 * g + 0.215_803_757_3 * b;
            let m_ = r - 0.105_561_345_8 * g - 0.063_854_172_8 * b;
            let s_ = r - 0.089_484_177_5 * g - 1.291_485_548 * b;
            let (l, m, s) = (l_.powi(3), m_.powi(3), s_.powi(3));
            (
                4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
                -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
                -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
                a,
            )
        }
        3 => {
            let (h, s, v) = (r.rem_euclid(1.0) * 6.0, g, b);
            let c = v * s;
            let x = c * (1.0 - (h % 2.0 - 1.0).abs());
            let (r1, g1, b1) = match h as i64 {
                0 => (c, x, 0.0),
                1 => (x, c, 0.0),
                2 => (0.0, c, x),
                3 => (0.0, x, c),
                4 => (x, 0.0, c),
                _ => (c, 0.0, x),
            };
            let m = v - c;
            (r1 + m, g1 + m, b1 + m, a)
        }
        _ => (r, g, b, a),
    }
}

/// Linear RGB as color space `space`, the inverse of [`to_linear`].
fn from_linear(space: i64, (r, g, b, a): Rgba) -> Rgba {
    match space {
        // P22e: 0.5 gives 188 / 255, so the channels pass through a byte.
        1 => {
            let byte = |c: f64| (semantics::linear_to_srgb(c) * 255.0).round() / 255.0;
            (byte(r), byte(g), byte(b), a)
        }
        2 => {
            let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
            let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
            let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
            (
                0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
                1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
                0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
                a,
            )
        }
        3 => {
            let max = r.max(g).max(b);
            let d = max - r.min(g).min(b);
            let h = if d == 0.0 {
                0.0
            } else if max == r {
                ((g - b) / d).rem_euclid(6.0) / 6.0
            } else if max == g {
                ((b - r) / d + 2.0) / 6.0
            } else {
                ((r - g) / d + 4.0) / 6.0
            };
            let s = if max == 0.0 { 0.0 } else { d / max };
            (h, s, max, a)
        }
        _ => (r, g, b, a),
    }
}

/// `epoch` (seconds, UTC) through `strftime`-style placeholders: %Y %y %m %d
/// %e %H %I %l %M %S %p %a %A %b %B %j %w %%. Server local time is taken as
/// UTC.
fn format_date(epoch: i64, format: &str) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ];
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (hour, minute, second) = (secs / 3600, secs / 60 % 60, secs % 60);
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let before = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][(month - 1) as usize];
    let yday = before + day + i64::from(leap && month > 2);
    let weekday = (days + 4).rem_euclid(7) as usize;
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out += &year.to_string(),
            Some('y') => {
                let _ = write!(out, "{:02}", year.rem_euclid(100));
            }
            Some('m') => {
                let _ = write!(out, "{month:02}");
            }
            Some('d') => {
                let _ = write!(out, "{day:02}");
            }
            Some('e') => out += &day.to_string(),
            Some('H') => {
                let _ = write!(out, "{hour:02}");
            }
            Some('I') => {
                let _ = write!(out, "{hour12:02}");
            }
            Some('l') => out += &hour12.to_string(),
            Some('M') => {
                let _ = write!(out, "{minute:02}");
            }
            Some('S') => {
                let _ = write!(out, "{second:02}");
            }
            Some('p') => out += if hour < 12 { "AM" } else { "PM" },
            Some('a') => out += &DAYS[weekday][..3],
            Some('A') => out += DAYS[weekday],
            Some('b') => out += &MONTHS[(month - 1) as usize][..3],
            Some('B') => out += MONTHS[(month - 1) as usize],
            Some('j') => {
                let _ = write!(out, "{yday:03}");
            }
            Some('w') => out += &weekday.to_string(),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

fn quat(v: Option<&Value>) -> Result<(f64, f64, f64, f64), String> {
    match v {
        None => Ok((0.0, 0.0, 0.0, 1.0)),
        Some(Value::Quat { x, y, z, w }) => Ok((*x, *y, *z, *w)),
        Some(v) => Err(format!("a {} where a quaternion goes", v.kind())),
    }
}

/// The numbers a value compares by, component by component. An unwired
/// operand reads as zero of its partner's kind.
fn components(v: Option<Value>, partner: Option<&Value>) -> Result<Vec<f64>, String> {
    let v = match v {
        Some(v) => v,
        None if partner.is_none() => return Ok(vec![0.0]),
        None => return Ok(vec![0.0; components(partner.cloned(), None)?.len()]),
    };
    Ok(match v {
        Value::Vector { x, y, z } => vec![x, y, z],
        Value::Rotator { pitch, yaw, roll } => vec![pitch, yaw, roll],
        Value::Quat { x, y, z, w } => vec![x, y, z, w],
        Value::Color { r, g, b, a } => vec![r, g, b, a],
        v => vec![v.as_float().ok_or_else(|| format!("Nearly Equal of a {}", v.kind()))?],
    })
}

/// `a` to `b` by `t`: integers stay integers (cut toward zero), rotators take
/// the shorter way round each axis, quaternions slerp the shorter way.
pub(crate) fn lerp(a: Option<Value>, b: Option<Value>, t: f64) -> Result<Value, String> {
    let zero_like = |v: &Option<Value>| match v {
        Some(Value::Vector { .. }) => Value::Vector { x: 0.0, y: 0.0, z: 0.0 },
        Some(Value::Rotator { .. }) => Value::Rotator { pitch: 0.0, yaw: 0.0, roll: 0.0 },
        Some(Value::Quat { .. }) => Value::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        Some(Value::Color { .. }) => Value::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
        Some(Value::Float(_)) => Value::Float(0.0),
        _ => Value::Int(0),
    };
    let a = a.unwrap_or_else(|| zero_like(&b));
    let b = b.unwrap_or_else(|| zero_like(&Some(a.clone())));
    let mix = |x: f64, y: f64| x + t * (y - x);
    Ok(match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => Value::Int(semantics::sanitize(mix(*x as f64, *y as f64)) as i64),
        (Value::Vector { x, y, z }, Value::Vector { x: x2, y: y2, z: z2 }) => Value::Vector {
            x: mix(*x, *x2),
            y: mix(*y, *y2),
            z: mix(*z, *z2),
        },
        (Value::Color { r, g, b, a }, Value::Color { r: r2, g: g2, b: b2, a: a2 }) => Value::Color {
            r: mix(*r, *r2),
            g: mix(*g, *g2),
            b: mix(*b, *b2),
            a: mix(*a, *a2),
        },
        (Value::Rotator { pitch, yaw, roll }, Value::Rotator { pitch: p2, yaw: y2, roll: r2 }) => {
            let turn = |x: f64, y: f64| x + t * wrap_degrees(y - x);
            Value::Rotator {
                pitch: turn(*pitch, *p2),
                yaw: turn(*yaw, *y2),
                roll: turn(*roll, *r2),
            }
        }
        (Value::Quat { x, y, z, w }, Value::Quat { x: x2, y: y2, z: z2, w: w2 }) => {
            slerp((*x, *y, *z, *w), (*x2, *y2, *z2, *w2), t)
        }
        (x, y) => match (x.as_float(), y.as_float()) {
            (Some(x), Some(y)) => Value::Float(semantics::sanitize(mix(x, y))),
            _ => return Err(format!("blending a {} with a {}", x.kind(), y.kind())),
        },
    })
}

/// `d` degrees brought into -180 to 180.
fn wrap_degrees(d: f64) -> f64 {
    let r = (d + 180.0).rem_euclid(360.0) - 180.0;
    if r == -180.0 { 180.0 } else { r }
}

fn slerp(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64), t: f64) -> Value {
    let mut dot = a.0 * b.0 + a.1 * b.1 + a.2 * b.2 + a.3 * b.3;
    let b = if dot < 0.0 {
        dot = -dot;
        (-b.0, -b.1, -b.2, -b.3)
    } else {
        b
    };
    let (sa, sb) = if dot > 0.9999 {
        (1.0 - t, t)
    } else {
        let omega = dot.clamp(-1.0, 1.0).acos();
        let s = omega.sin();
        (((1.0 - t) * omega).sin() / s, (t * omega).sin() / s)
    };
    let (x, y, z, w) = (
        sa * a.0 + sb * b.0,
        sa * a.1 + sb * b.1,
        sa * a.2 + sb * b.2,
        sa * a.3 + sb * b.3,
    );
    let len = (x * x + y * y + z * z + w * w).sqrt();
    if len > 0.0 {
        Value::Quat { x: x / len, y: y / len, z: z / len, w: w / len }
    } else {
        Value::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }
    }
}

/// The saved easing `Function` and `Direction` (`EBREasingFunction`,
/// `EBREasingDirection`).
fn easing_of(gate: &Gate) -> (i64, i64) {
    (gate.field_i64("Function").unwrap_or(0), gate.field_i64("Direction").unwrap_or(0))
}

/// The standard easing curves: `In` is the curve, `Out` its mirror
/// `1 - in(1 - t)`, `InOut` the two halves joined at 0.5.
pub(crate) fn ease((function, direction): (i64, i64), t: f64) -> f64 {
    let curve = |t: f64| -> f64 {
        const C1: f64 = 1.70158;
        match function {
            1 => 1.0 - (t * PI / 2.0).cos(),
            2 => t.powi(2),
            3 => t.powi(3),
            4 => t.powi(4),
            5 => t.powi(5),
            6 if t == 0.0 => 0.0,
            6 => 2f64.powf(10.0 * t - 10.0),
            7 => 1.0 - (1.0 - t * t).max(0.0).sqrt(),
            8 => (C1 + 1.0) * t.powi(3) - C1 * t.powi(2),
            9 if t == 0.0 || t == 1.0 => t,
            9 => -(2f64.powf(10.0 * t - 10.0)) * ((10.0 * t - 10.75) * (2.0 * PI / 3.0)).sin(),
            10 => 1.0 - bounce_out(1.0 - t),
            _ => t,
        }
    };
    if function == 0 {
        return t;
    }
    match direction {
        1 => 1.0 - curve(1.0 - t),
        2 if t < 0.5 => curve(2.0 * t) / 2.0,
        2 => 1.0 - curve(2.0 - 2.0 * t) / 2.0,
        _ => curve(t),
    }
}

fn bounce_out(t: f64) -> f64 {
    const N: f64 = 7.5625;
    const D: f64 = 2.75;
    if t < 1.0 / D {
        N * t * t
    } else if t < 2.0 / D {
        let t = t - 1.5 / D;
        N * t * t + 0.75
    } else if t < 2.5 / D {
        let t = t - 2.25 / D;
        N * t * t + 0.9375
    } else {
        let t = t - 2.625 / D;
        N * t * t + 0.984_375
    }
}

/// The text either side of occurrence `occurrence` of `delim`, counting
/// from 1 (the save's default), or `None` when there is no such occurrence.
/// P20d: 0 and -1 find nothing.
fn split(s: &str, delim: &str, occurrence: i64, case_sensitive: bool) -> Option<(String, String)> {
    if delim.is_empty() {
        return None;
    }
    let same = |a: char, b: char| {
        a == b || (!case_sensitive && a.to_lowercase().eq(b.to_lowercase()))
    };
    let mut hits = Vec::new();
    let mut from = 0;
    while from < s.len() {
        let rest = &s[from..];
        let mut chars = rest.chars();
        let matched = delim.chars().all(|d| chars.next().is_some_and(|c| same(c, d)));
        if matched {
            let end = s.len() - chars.as_str().len();
            hits.push((from, end));
            from = end;
        } else {
            from += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    let i = usize::try_from(occurrence).ok()?.checked_sub(1)?;
    let &(start, end) = hits.get(i)?;
    Some((s[..start].to_string(), s[end..].to_string()))
}

/// `v` as the kind of value `target` is, where the game converts on its own:
/// between numbers and bools, and anything to text.
fn convert(v: Option<Value>, target: Option<Value>) -> Result<Option<Value>, String> {
    let (Some(v), Some(target)) = (v, target) else {
        return Ok(None);
    };
    if std::mem::discriminant(&v) == std::mem::discriminant(&target) {
        return Ok(Some(v));
    }
    let numeric = matches!(v, Value::Int(_) | Value::Float(_) | Value::Bool(_));
    Ok(Some(match target {
        Value::Int(_) if numeric => Value::Int(v.as_int().unwrap_or(0)),
        Value::Float(_) if numeric => Value::Float(v.as_float().unwrap_or(0.0)),
        Value::Bool(_) if numeric || matches!(v, Value::Str(_)) => Value::Bool(v.truthy()),
        Value::Str(_) => match v.to_semantic() {
            Some(s) => Value::Str(semantics::concat_display(&s)),
            None => return Err(format!("Convert of a {} to text", v.kind())),
        },
        t => return Err(format!("Convert of a {} to a {}", v.kind(), t.kind())),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_counts_occurrences_from_one() {
        let s = "a b c";
        assert_eq!(split(s, " ", 1, true), Some(("a".into(), "b c".into())));
        assert_eq!(split(s, " ", 2, true), Some(("a b".into(), "c".into())));
        assert_eq!(split(s, " ", -1, true), None);
        assert_eq!(split(s, " ", 3, true), None);
        assert_eq!(split(s, " ", 0, true), None);
        assert_eq!(split("aXb", "x", 1, false), Some(("a".into(), "b".into())));
        assert_eq!(split("aXb", "x", 1, true), None);
        assert_eq!(split("é-é", "-", 1, true), Some(("é".into(), "é".into())));
    }

    #[test]
    fn easing_ends_at_its_endpoints() {
        for function in 0..=10 {
            for direction in 0..=2 {
                let e = |t| ease((function, direction), t);
                assert!(e(0.0).abs() < 1e-9, "{function} {direction}");
                assert!((e(1.0) - 1.0).abs() < 1e-9, "{function} {direction}");
            }
        }
        assert!((ease((2, 0), 0.5) - 0.25).abs() < 1e-12);
        assert!((ease((2, 1), 0.5) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn dates_format_in_utc() {
        assert_eq!(format_date(0, "%Y-%m-%d %H:%M:%S"), "1970-01-01 00:00:00");
        assert_eq!(format_date(1_767_225_600, "%A %e %B %Y, %l %p"), "Thursday 1 January 2026, 12 AM");
        assert_eq!(format_date(951_782_400, "%j %d/%m/%y"), "060 29/02/00");
        assert_eq!(format_date(-1, "%Y %H:%M:%S"), "1969 23:59:59");
    }

    #[test]
    fn colors_convert_there_and_back() {
        for space in 1..=3 {
            let c = (0.2, 0.5, 0.8, 1.0);
            let back = to_linear(space, from_linear(space, c));
            // sRGB passes through a byte, so it comes back within a step.
            let tol = if space == 1 { 5e-3 } else { 1e-6 };
            assert!((back.0 - c.0).abs() < tol && (back.1 - c.1).abs() < tol && (back.2 - c.2).abs() < tol, "{space}");
        }
        let (h, s, v, _) = from_linear(3, (0.5, 0.25, 1.0, 1.0));
        assert!((h - 0.722_222).abs() < 1e-6 && (s - 0.75).abs() < 1e-9 && (v - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rotators_blend_the_short_way() {
        let a = Some(Value::Rotator { pitch: 0.0, yaw: 170.0, roll: 0.0 });
        let b = Some(Value::Rotator { pitch: 0.0, yaw: -170.0, roll: 0.0 });
        let Value::Rotator { yaw, .. } = lerp(a, b, 0.5).unwrap() else { panic!() };
        assert!((yaw - 180.0).abs() < 1e-9);
    }
}
