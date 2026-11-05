//! The values that travel on wires and sit in gate storage.

use std::fmt;

use brdb::schema::{BrdbValue, WireValue};
use wirescript_ir::semantics;

use crate::graph::GateId;

/// One wire value. The scalar and composite cases are the certified
/// evaluator's ([`semantics::Value`]); the rest are references the game
/// passes by handle.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Vector { x: f64, y: f64, z: f64 },
    Rotator { pitch: f64, yaw: f64, roll: f64 },
    Quat { x: f64, y: f64, z: f64, w: f64 },
    /// Linear-space RGBA.
    Color { r: f64, g: f64, b: f64, a: f64 },
    /// A reference to a storage gate (`VarRef`, `ArrayVarRef`, `MapVarRef`)
    /// or a function definition.
    Ref(GateId),
    /// An object handle (entity, character, controller) handed out by the
    /// world; 0 is the null object.
    Object(u64),
    /// A stored brick reference.
    Brick(brdb::schema::SavedBrickRef),
    /// A stored component reference the loader has not resolved to a gate.
    Handle(brdb::schema::SavedComponentRef),
    /// What a pure gate gave when it could not be evaluated (a gate or an
    /// operand the evaluator does not certify). Pure gates pass it on; the
    /// error is raised when an exec gate, an Exec pin or a buffer reads it.
    Poison(Poisoned),
}

/// The error a poisoned value carries. Two are equal only when they are the
/// same error.
#[derive(Clone, Debug)]
pub struct Poisoned(pub std::sync::Arc<crate::SimError>);

impl PartialEq for Poisoned {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for Value {
    fn default() -> Self {
        Self::Float(0.0)
    }
}

impl Value {
    /// The certified evaluator's view of this value, or `None` for a
    /// reference, which no certified law takes.
    pub fn to_semantic(&self) -> Option<semantics::Value> {
        Some(match self {
            Self::Int(n) => semantics::Value::Int(*n),
            Self::Float(f) => semantics::Value::Float(*f),
            Self::Bool(b) => semantics::Value::Bool(*b),
            Self::Str(s) => semantics::Value::Str(s.clone()),
            Self::Vector { x, y, z } => semantics::Value::Vector {
                x: *x,
                y: *y,
                z: *z,
            },
            Self::Rotator { pitch, yaw, roll } => semantics::Value::Rotator {
                pitch: *pitch,
                yaw: *yaw,
                roll: *roll,
            },
            Self::Quat { x, y, z, w } => semantics::Value::Quat {
                x: *x,
                y: *y,
                z: *z,
                w: *w,
            },
            Self::Color { r, g, b, a } => semantics::Value::Color {
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            },
            Self::Ref(_) | Self::Object(_) | Self::Brick(_) | Self::Handle(_) | Self::Poison(_) => {
                return None;
            }
        })
    }

    pub fn from_semantic(v: semantics::Value) -> Self {
        match v {
            semantics::Value::Int(n) => Self::Int(n),
            semantics::Value::Float(f) => Self::Float(f),
            semantics::Value::Bool(b) => Self::Bool(b),
            semantics::Value::Str(s) => Self::Str(s),
            semantics::Value::Vector { x, y, z } => Self::Vector { x, y, z },
            semantics::Value::Rotator { pitch, yaw, roll } => Self::Rotator { pitch, yaw, roll },
            semantics::Value::Quat { x, y, z, w } => Self::Quat { x, y, z, w },
            semantics::Value::Color { r, g, b, a } => Self::Color { r, g, b, a },
        }
    }

    /// The value a save stores, or `None` for one with no wire meaning
    /// (an exec marker, an enum, a null weak object).
    pub fn from_wire(w: &WireValue) -> Option<Self> {
        Some(match w {
            WireValue::Bool(b) => Self::Bool(*b),
            WireValue::Int(n) => Self::Int(*n),
            WireValue::Number(f) => Self::Float(*f),
            WireValue::LinearColor { r, g, b, a } => Self::Color {
                r: f64::from(*r),
                g: f64::from(*g),
                b: f64::from(*b),
                a: f64::from(*a),
            },
            WireValue::Vector { x, y, z } => Self::Vector {
                x: *x,
                y: *y,
                z: *z,
            },
            WireValue::Rotator { pitch, yaw, roll } => Self::Rotator {
                pitch: *pitch,
                yaw: *yaw,
                roll: *roll,
            },
            WireValue::Quat { x, y, z, w } => Self::Quat {
                x: *x,
                y: *y,
                z: *z,
                w: *w,
            },
            WireValue::Str(s) => Self::Str(s.clone()),
            WireValue::Object => Self::Object(0),
            WireValue::BrickRef(b) => Self::Brick(*b),
            WireValue::ComponentRef(c) => Self::Handle(*c),
            WireValue::Enum(e) => Self::Int(e.value),
            WireValue::Exec => return None,
        })
    }

    /// A value as a save stores it in a field or an array element: a
    /// `WireValue`, a typed scalar, or a composite struct. `None` for a
    /// shape no wire carries.
    pub fn from_brdb(v: &BrdbValue) -> Option<Self> {
        if let Ok(w) = WireValue::try_from(v) {
            return Self::from_wire(&w);
        }
        Some(match v {
            BrdbValue::Bool(b) => Self::Bool(*b),
            BrdbValue::U8(n) => Self::Int(i64::from(*n)),
            BrdbValue::U16(n) => Self::Int(i64::from(*n)),
            BrdbValue::U32(n) => Self::Int(i64::from(*n)),
            BrdbValue::U64(n) => Self::Int(i64::try_from(*n).ok()?),
            BrdbValue::I8(n) => Self::Int(i64::from(*n)),
            BrdbValue::I16(n) => Self::Int(i64::from(*n)),
            BrdbValue::I32(n) => Self::Int(i64::from(*n)),
            BrdbValue::I64(n) => Self::Int(*n),
            BrdbValue::F32(f) => Self::Float(f64::from(*f)),
            BrdbValue::F64(f) => Self::Float(*f),
            BrdbValue::String(s) => Self::Str(s.clone()),
            BrdbValue::Enum(e) => Self::Int(i64::try_from(e.value).ok()?),
            BrdbValue::Struct(s) => {
                let f = |n: &str| match s.get(n)? {
                    BrdbValue::F64(f) => Some(*f),
                    BrdbValue::F32(f) => Some(f64::from(*f)),
                    _ => None,
                };
                if let (Some(x), Some(y), Some(z)) = (f("X"), f("Y"), f("Z")) {
                    return Some(match f("W") {
                        Some(w) => Self::Quat { x, y, z, w },
                        None => Self::Vector { x, y, z },
                    });
                }
                if let (Some(pitch), Some(yaw), Some(roll)) = (f("Pitch"), f("Yaw"), f("Roll")) {
                    return Some(Self::Rotator { pitch, yaw, roll });
                }
                if let (Some(r), Some(g), Some(b)) = (f("R"), f("G"), f("B")) {
                    return Some(Self::Color {
                        r,
                        g,
                        b,
                        a: f("A").unwrap_or(1.0),
                    });
                }
                return None;
            }
            _ => return None,
        })
    }

    /// Certified truthiness, as an Exec pin and a branch condition read it.
    pub fn truthy(&self) -> bool {
        match self {
            Self::Ref(_) | Self::Brick(_) | Self::Handle(_) => true,
            Self::Poison(_) => false,
            Self::Object(id) => *id != 0,
            v => semantics::truthy(v.to_semantic().as_ref()),
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Float(f) => Some(semantics::sanitize(*f) as i64),
            Self::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Int(n) => Some(*n as f64),
            Self::Float(f) => Some(semantics::sanitize(*f)),
            Self::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }

    /// What `FormatText` prints for this value.
    pub fn render(&self) -> String {
        match self.to_semantic() {
            Some(v) => semantics::render_for_format(&v),
            None => String::new(),
        }
    }

    /// A short name for the value's variant, for error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Bool(_) => "bool",
            Self::Str(_) => "string",
            Self::Vector { .. } => "vector",
            Self::Rotator { .. } => "rotator",
            Self::Quat { .. } => "quat",
            Self::Color { .. } => "color",
            Self::Ref(_) => "reference",
            Self::Object(_) => "object",
            Self::Brick(_) => "brick",
            Self::Handle(_) => "handle",
            Self::Poison(_) => "uncertified value",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Str(s) => write!(f, "{s:?}"),
            Self::Ref(g) => write!(f, "ref(#{g})"),
            Self::Object(id) => write!(f, "object({id})"),
            Self::Brick(b) => write!(
                f,
                "brick(grid {} {:?} #{})",
                b.grid_persistent_index, b.chunk_index, b.brick_index_in_chunk
            ),
            Self::Handle(h) => write!(f, "handle(type {})", h.component_type_index),
            Self::Poison(p) => write!(f, "uncertified({})", p.0),
            v => f.write_str(&v.render()),
        }
    }
}
