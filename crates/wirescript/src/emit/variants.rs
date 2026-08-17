//! IR `Literal`/`Type` to brdb wire-variant conversion.

use super::*;

/// Bool → Int for the prim-math wire variant, which has no `bool` member: an
/// uncoerced `WireVariant::Bool` is rejected by the brdb schema writer.
pub(super) fn coerce_for_prim_math(wv: WireVariant) -> WireVariant {
    match wv {
        WireVariant::Bool(b) => WireVariant::Int(if b { 1 } else { 0 }),
        other => other,
    }
}

pub(super) fn var_type_to_wire_variant(ty: Option<&crate::ir::Type>) -> WireVariant {
    use crate::ir::Type;
    debug_assert!(
        !matches!(ty, Some(Type::Param(_))),
        "Type::Param reached emit — monomorphization must substitute it first"
    );
    match ty {
        Some(Type::Bool) => WireVariant::Bool(false),
        Some(Type::Int) => WireVariant::Int(0),
        Some(Type::Controller | Type::Character | Type::Entity) => {
            WireVariant::Object(None)
        }
        Some(Type::String) => WireVariant::Str(String::new()),
        Some(Type::Vector) => WireVariant::Vector(Vector3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        Some(Type::Rotator) => WireVariant::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        },
        Some(Type::Quat) => WireVariant::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        Some(Type::Color) => WireVariant::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        _ => WireVariant::Number(0.0),
    }
}

/// Unwrap a `ref array<T>` (or bare `array<T>`) port type down to its element
/// type `T`.
pub(super) fn array_element_type(ty: &crate::ir::Type) -> Option<&crate::ir::Type> {
    use crate::ir::Type;
    let inner = match ty {
        Type::Ref(inner) => inner.as_ref(),
        other => other,
    };
    match inner {
        Type::Array(elem) => Some(elem.as_ref()),
        _ => None,
    }
}

/// The `WireMapVariant` (empty map) an empty `MapVar` brick serializes, chosen
/// from the declared `Map<K, V>` type on its `MapVarRef` output port. Keys are
/// int/string/object; values cover the wire-storable scalars. Defaults to
/// `int -> float` for an unknown/degenerate type.
pub(super) fn map_variant_from_type(ty: &crate::ir::Type) -> WireMapVariant {
    use crate::ir::Type;
    let inner = match ty {
        Type::Ref(inner) => inner.as_ref(),
        other => other,
    };
    let (k, v) = match inner {
        Type::Map(k, v) => (k.as_ref(), v.as_ref()),
        _ => return WireMapVariant { key: WireMapKey::Int64, value: WireMapValue::Number, entries: vec![] },
    };
    let key = match k {
        Type::Int | Type::Bool => WireMapKey::Int64,
        Type::String => WireMapKey::Str,
        // Entity-family references are weak-object keys.
        _ => WireMapKey::Object,
    };
    let value = match v {
        Type::Int => WireMapValue::Int64,
        Type::Float => WireMapValue::Number,
        Type::Bool => WireMapValue::Bool,
        Type::String => WireMapValue::Str,
        Type::Vector => WireMapValue::Vector,
        Type::Rotator => WireMapValue::Rotator,
        Type::Quat => WireMapValue::Quat,
        Type::Color => WireMapValue::LinearColor,
        Type::Entity | Type::Character | Type::Controller => {
            WireMapValue::Object
        }
        _ => WireMapValue::Number,
    };
    WireMapVariant { key, value, entries: vec![] }
}

/// Build a populated `WireMapVariant` from a map's constant entries. `kinds`
/// gives the key/value member kinds (from [`map_variant_from_type`] on the
/// declared `Map<K, V>` port type); each literal is read in that kind.
pub(super) fn wire_map_variant_from_literals(
    kinds: WireMapVariant,
    entries: &[(Literal, Literal)],
) -> WireMapVariant {
    // Numeric key/value arms read across literal kinds (like arrays'
    // `as_i64`/`as_f64`) so a raw literal that ever reaches emit still bakes
    // correctly instead of hitting the zero fallback — defense in depth
    // behind the fold-time coercion in `bake_map_init`/`coerce_literal_to_type`.
    let key_of = |l: &Literal| match (kinds.key, l) {
        (WireMapKey::Int64, Literal::Int(n)) => WireMapKeyData::Int64(*n),
        (WireMapKey::Int64, Literal::Float(f)) => WireMapKeyData::Int64(*f as i64),
        (WireMapKey::Int64, Literal::Bool(b)) => WireMapKeyData::Int64(*b as i64),
        (WireMapKey::Str, Literal::String(s)) => WireMapKeyData::Str(s.clone()),
        (WireMapKey::Object, _) => WireMapKeyData::Object(None),
        (WireMapKey::Int64, _) => WireMapKeyData::Int64(0),
        (WireMapKey::Str, _) => WireMapKeyData::Str(String::new()),
    };
    let val_of = |l: &Literal| match (kinds.value, l) {
        (WireMapValue::Number, Literal::Float(f)) => WireMapValueData::Number(*f),
        (WireMapValue::Number, Literal::Int(n)) => WireMapValueData::Number(*n as f64),
        (WireMapValue::Number, Literal::Bool(b)) => WireMapValueData::Number(*b as i64 as f64),
        (WireMapValue::Int64, Literal::Int(n)) => WireMapValueData::Int64(*n),
        (WireMapValue::Int64, Literal::Float(f)) => WireMapValueData::Int64(*f as i64),
        (WireMapValue::Int64, Literal::Bool(b)) => WireMapValueData::Int64(*b as i64),
        (WireMapValue::Bool, Literal::Bool(b)) => WireMapValueData::Bool(*b),
        (WireMapValue::Bool, Literal::Int(n)) => WireMapValueData::Bool(*n != 0),
        (WireMapValue::Str, Literal::String(s)) => WireMapValueData::Str(s.clone()),
        (WireMapValue::Vector, Literal::Vector { x, y, z }) => WireMapValueData::Vector(Vector3f {
            x: *x as f32,
            y: *y as f32,
            z: *z as f32,
        }),
        (WireMapValue::Rotator, Literal::Rotator { pitch, yaw, roll }) => {
            WireMapValueData::Rotator(*pitch, *yaw, *roll)
        }
        (WireMapValue::Quat, Literal::Quat { x, y, z, w }) => {
            WireMapValueData::Quat(*x, *y, *z, *w)
        }
        (WireMapValue::LinearColor, Literal::LinearColor { r, g, b, a }) => {
            WireMapValueData::LinearColor(*r as f32, *g as f32, *b as f32, *a as f32)
        }
        (WireMapValue::Object, _) => WireMapValueData::Object(None),
        (WireMapValue::Number, _) => WireMapValueData::Number(0.0),
        (WireMapValue::Int64, _) => WireMapValueData::Int64(0),
        (WireMapValue::Bool, _) => WireMapValueData::Bool(false),
        (WireMapValue::Str, _) => WireMapValueData::Str(String::new()),
        (WireMapValue::Vector, _) => WireMapValueData::Vector(Vector3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        (WireMapValue::Rotator, _) => WireMapValueData::Rotator(0.0, 0.0, 0.0),
        (WireMapValue::Quat, _) => WireMapValueData::Quat(0.0, 0.0, 0.0, 1.0),
        (WireMapValue::LinearColor, _) => WireMapValueData::LinearColor(0.0, 0.0, 0.0, 1.0),
    };
    WireMapVariant {
        key: kinds.key,
        value: kinds.value,
        entries: entries.iter().map(|(k, v)| (key_of(k), val_of(v))).collect(),
    }
}

/// Empty `WireGraphArrayVariant` member matching the array's element type, so
/// the ArrayVar gate is created as the correct array kind.
pub(super) fn empty_wire_array_variant(elem: Option<&crate::ir::Type>) -> WireArrayVariant {
    use crate::ir::Type;
    match elem {
        Some(Type::Int) => WireArrayVariant::Int64Array(Vec::new()),
        Some(Type::Bool) => WireArrayVariant::BoolArray(Vec::new()),
        Some(Type::String) => WireArrayVariant::StringArray(Vec::new()),
        Some(Type::Vector) => WireArrayVariant::VectorArray(Vec::new()),
        Some(Type::Rotator) => WireArrayVariant::RotatorArray(Vec::new()),
        Some(Type::Quat) => WireArrayVariant::QuatArray(Vec::new()),
        Some(Type::Color) => WireArrayVariant::LinearColorArray(Vec::new()),
        Some(Type::Controller | Type::Character | Type::Entity) => {
            WireArrayVariant::ObjectArray(Vec::new())
        }
        _ => WireArrayVariant::DoubleArray(Vec::new()), // float + default
    }
}

/// Build a populated array variant from an array's constant initial elements.
/// The backing variant is chosen by the element type (matching
/// [`empty_wire_array_variant`]); each literal is read in that type.
pub(super) fn wire_array_variant_from_literals(
    elem: Option<&crate::ir::Type>,
    lits: &[Literal],
) -> WireArrayVariant {
    use crate::ir::Type;
    let as_i64 = |l: &Literal| match l {
        Literal::Int(n) => *n,
        Literal::Float(f) => *f as i64,
        Literal::Bool(b) => *b as i64,
        _ => 0,
    };
    let as_f64 = |l: &Literal| match l {
        Literal::Float(f) => *f,
        Literal::Int(n) => *n as f64,
        Literal::Bool(b) => *b as i64 as f64,
        _ => 0.0,
    };
    match elem {
        Some(Type::Int) => WireArrayVariant::Int64Array(lits.iter().map(as_i64).collect()),
        Some(Type::Bool) => WireArrayVariant::BoolArray(
            lits.iter()
                .map(|l| {
                    matches!(l, Literal::Bool(true)) || matches!(l, Literal::Int(n) if *n != 0)
                })
                .collect(),
        ),
        Some(Type::String) => WireArrayVariant::StringArray(
            lits.iter()
                .map(|l| match l {
                    Literal::String(s) => s.clone(),
                    _ => String::new(),
                })
                .collect(),
        ),
        Some(Type::Vector) => WireArrayVariant::VectorArray(
            lits.iter()
                .map(|l| match l {
                    Literal::Vector { x, y, z } => Vector3f {
                        x: *x as f32,
                        y: *y as f32,
                        z: *z as f32,
                    },
                    _ => Vector3f {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                })
                .collect(),
        ),
        Some(Type::Rotator) => WireArrayVariant::RotatorArray(
            lits.iter()
                .map(|l| match l {
                    Literal::Rotator { pitch, yaw, roll } => (*pitch, *yaw, *roll),
                    _ => (0.0, 0.0, 0.0),
                })
                .collect(),
        ),
        Some(Type::Quat) => WireArrayVariant::QuatArray(
            lits.iter()
                .map(|l| match l {
                    Literal::Quat { x, y, z, w } => (*x, *y, *z, *w),
                    _ => (0.0, 0.0, 0.0, 1.0),
                })
                .collect(),
        ),
        Some(Type::Color) => WireArrayVariant::LinearColorArray(
            lits.iter()
                .map(|l| match l {
                    Literal::LinearColor { r, g, b, a } => {
                        (*r as f32, *g as f32, *b as f32, *a as f32)
                    }
                    Literal::Color { r, g, b, a } => (
                        *r as f32 / 255.0,
                        *g as f32 / 255.0,
                        *b as f32 / 255.0,
                        *a as f32 / 255.0,
                    ),
                    _ => (1.0, 1.0, 1.0, 1.0),
                })
                .collect(),
        ),
        // Object arrays can't be initialised from literals.
        Some(Type::Controller | Type::Character | Type::Entity) => {
            WireArrayVariant::ObjectArray(vec![None; lits.len()])
        }
        _ => WireArrayVariant::DoubleArray(lits.iter().map(as_f64).collect()), // float + default
    }
}

/// Convert a Literal to a Box<dyn AsBrdbValue> using the most appropriate
/// type. For wire_graph_variant fields, wraps in WireVariant. For plain
/// typed fields, uses the native type directly.
/// Convert a Literal to a Box<dyn AsBrdbValue> as a WireVariant.
pub(super) fn literal_to_boxed_wire_variant(
    lit: &Literal,
    ports: &crate::ir::GateIO,
    field: &str,
) -> Box<dyn AsBrdbValue> {
    if let Some(wv) = literal_to_wire_variant(lit) {
        return Box::new(wv);
    }
    let port_ty = ports
        .inputs
        .iter()
        .chain(ports.outputs.iter())
        .find(|p| resolve(p.name) == field)
        .map(|p| &p.ty);
    Box::new(var_type_to_wire_variant(port_ty))
}

/// Convert a Literal to a Box<dyn AsBrdbValue> using native types (str/i32/f64/bool).
///
/// `None` for a literal with no native-string representation — a `const`
/// record, which typecheck's validators are supposed to keep out of a
/// `FieldKind::Str` field entirely (see the caller in `components.rs`, which
/// turns a `None` here into `EmitError::UnrepresentableLiteral` rather than
/// the process-aborting `unreachable!()` this used to be).
pub(super) fn literal_to_string(lit: &Literal) -> Option<String> {
    match lit {
        Literal::String(s) => Some(s.clone()),
        Literal::Int(n) => Some(n.to_string()),
        Literal::Float(f) => Some(f.to_string()),
        Literal::Bool(b) => Some(b.to_string()),
        Literal::Record(_) => None,
        _ => Some(String::new()),
    }
}

/// `None` for a literal with no native `AsBrdbValue` representation — same
/// `Literal::Record` case and the same reasoning as [`literal_to_string`].
pub(super) fn literal_to_boxed_native(lit: &Literal) -> Option<Box<dyn AsBrdbValue>> {
    match lit {
        Literal::String(s) => Some(Box::new(s.clone())),
        Literal::Int(n) => Some(Box::new(*n)),
        Literal::Float(f) => Some(Box::new(*f)),
        Literal::Bool(b) => Some(Box::new(*b)),
        Literal::Vector { x, y, z } => Some(Box::new(VectorValue {
            x: *x,
            y: *y,
            z: *z,
        })),
        Literal::Rotator { pitch, yaw, roll } => Some(Box::new(RotatorValue {
            pitch: *pitch,
            yaw: *yaw,
            roll: *roll,
        })),
        Literal::Quat { x, y, z, w } => Some(Box::new(QuatValue {
            x: *x,
            y: *y,
            z: *z,
            w: *w,
        })),
        Literal::Record(_) => None,
        _ => Some(Box::new(0i64)),
    }
}

// Folded literals embedded into native f64 struct fields (the schema's
// `Vector`/`Rotator`/`Quat` structs) — brdb's Vector3f/Quat4f are f32, so
// these mirror its AsBrdbValue impl at full precision.
struct VectorValue {
    x: f64,
    y: f64,
    z: f64,
}

impl AsBrdbValue for VectorValue {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            n => unimplemented!("unimplemented Vector field {n}"),
        }
    }
}

struct RotatorValue {
    pitch: f64,
    yaw: f64,
    roll: f64,
}

impl AsBrdbValue for RotatorValue {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "Pitch" => Ok(&self.pitch),
            "Yaw" => Ok(&self.yaw),
            "Roll" => Ok(&self.roll),
            n => unimplemented!("unimplemented Rotator field {n}"),
        }
    }
}

struct QuatValue {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

impl AsBrdbValue for QuatValue {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            "W" => Ok(&self.w),
            n => unimplemented!("unimplemented Quat field {n}"),
        }
    }
}

pub(super) fn literal_to_wire_variant(lit: &Literal) -> Option<WireVariant> {
    match lit {
        Literal::Int(n) => Some(WireVariant::Int(*n)),
        Literal::Float(f) => Some(WireVariant::Number(*f)),
        Literal::Bool(b) => Some(WireVariant::Bool(*b)),
        Literal::Object => Some(WireVariant::Object(None)),
        Literal::String(s) => Some(WireVariant::Str(s.clone())),
        Literal::Vector { x, y, z } => Some(WireVariant::Vector(Vector3f {
            x: *x as f32,
            y: *y as f32,
            z: *z as f32,
        })),
        Literal::Rotator { pitch, yaw, roll } => Some(WireVariant::Rotator {
            pitch: *pitch,
            yaw: *yaw,
            roll: *roll,
        }),
        Literal::Quat { x, y, z, w } => Some(WireVariant::Quat {
            x: *x,
            y: *y,
            z: *z,
            w: *w,
        }),
        Literal::LinearColor { r, g, b, a } => Some(WireVariant::LinearColor {
            r: *r as f32,
            g: *g as f32,
            b: *b as f32,
            a: *a as f32,
        }),
        // sRGB byte color (brick paint) → linear-ish 0–1. Only reached if a
        // paint literal ends up on a wire-variant port.
        Literal::Color { r, g, b, a } => Some(WireVariant::LinearColor {
            r: *r as f32 / 255.0,
            g: *g as f32 / 255.0,
            b: *b as f32 / 255.0,
            a: *a as f32 / 255.0,
        }),
        // No wire-variant form. `Record` belongs here with its siblings:
        // this function's contract IS "None when there is no wire
        // representation", so panicking for one such variant would
        // contradict what every caller is told to expect. The `unreachable!`s
        // live only in the two converters below, whose contract really is
        // "this always yields a value" and which therefore have no honest way
        // to decline.
        Literal::Array(_)
        | Literal::Map(_)
        | Literal::Record(_)
        | Literal::Asset { .. }
        | Literal::PrefabRef { .. }
        | Literal::NestedPrefab { .. } => None,
    }
}
