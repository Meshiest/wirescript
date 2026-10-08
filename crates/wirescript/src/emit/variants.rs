//! IR `Literal`/`Type` to brdb `WireValue` conversion, plus the dynamic
//! array/map containers that `ArrayVar`/`MapVar` store.

use super::*;

use brdb::schema::as_brdb::{BrdbArrayIter, BrdbMapIter};

/// Gates whose wire-value inputs were numeric-only in the previous save
/// format: the `Expr_Math*` family, the ordered compares, `NearlyEqual` and
/// `Exec_Random`. A constant `bool` baked into one of their inputs is written
/// as an int, the form these gates have always been given.
pub(super) fn bool_input_bakes_as_int(gate_class: &str) -> bool {
    gate_class.starts_with("BrickComponentType_Expr_Math")
        || matches!(
            gate_class,
            "BrickComponentType_Expr_CompareGreater"
                | "BrickComponentType_Expr_CompareGreaterOrEqual"
                | "BrickComponentType_Expr_CompareLess"
                | "BrickComponentType_Expr_CompareLessOrEqual"
                | gc::NEARLY_EQUAL
                | gc::RANDOM
        )
}

/// Bool → Int, for [`bool_input_bakes_as_int`] gates.
pub(super) fn coerce_bool_to_int(wv: WireValue) -> WireValue {
    match wv {
        WireValue::Bool(b) => WireValue::Int(if b { 1 } else { 0 }),
        other => other,
    }
}

/// The game's own unset brick ref, as it writes into an unwired ref field:
/// brick index `u32::MAX` in chunk (0, 0, 0) of grid 0.
pub(super) fn null_brick_ref() -> brdb::schema::SavedBrickRef {
    brdb::schema::SavedBrickRef {
        brick_index_in_chunk: u32::MAX,
        ..Default::default()
    }
}

/// The game's own unset component ref: the null brick, component type 0.
pub(super) fn null_component_ref() -> brdb::schema::SavedComponentRef {
    brdb::schema::SavedComponentRef {
        brick: null_brick_ref(),
        component_type_index: 0,
    }
}

pub(super) fn var_type_to_wire_value(ty: Option<&crate::ir::Type>) -> WireValue {
    use crate::ir::Type;
    debug_assert!(
        !matches!(ty, Some(Type::Param(_))),
        "Type::Param reached emit: monomorphization must substitute it first"
    );
    match ty {
        Some(Type::Bool) => WireValue::Bool(false),
        Some(Type::Int) => WireValue::Int(0),
        Some(Type::Controller | Type::Character | Type::Entity) => WireValue::Object,
        Some(Type::String) => WireValue::Str(String::new()),
        Some(Type::Vector) => WireValue::Vector {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        Some(Type::Rotator) => WireValue::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        },
        Some(Type::Quat) => WireValue::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        Some(Type::Color) => WireValue::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        // A var ref, and an array or map ref (a `VarRef` of an ArrayVar /
        // MapVar), is a component ref, as is a `Component<X>` from Get/Add
        // Component; unset, it is the game's null one.
        Some(Type::Ref(_) | Type::Array(_) | Type::Map(_, _) | Type::Component(_)) => {
            WireValue::ComponentRef(null_component_ref())
        }
        Some(Type::Brick) => WireValue::BrickRef(null_brick_ref()),
        _ => WireValue::Number(0.0),
    }
}

/// The schema `types` entry a dynamic array/map stores values of `ty` as.
/// Unknown and float types store as `f64`, the game's own default for a new
/// array or map variable.
pub(super) fn dynamic_type_name(ty: Option<&crate::ir::Type>) -> &'static str {
    use crate::ir::Type;
    match ty {
        Some(Type::Int) => "i64",
        Some(Type::Bool) => "Bool",
        Some(Type::String) => "Str",
        Some(Type::Vector) => "Vector",
        Some(Type::Rotator) => "Rotator",
        Some(Type::Quat) => "Quat",
        Some(Type::Color) => "LinearColor",
        Some(Type::Controller | Type::Character | Type::Entity) => "weak_object",
        // Handle storage is kind-strict in game: a component_handle container
        // keeps var, array and map refs and components; a brick_handle one
        // keeps bricks.
        Some(Type::Ref(_) | Type::Array(_) | Type::Map(_, _) | Type::Component(_)) => "component_handle",
        Some(Type::Brick) => "brick_handle",
        _ => "f64",
    }
}

/// A `dynamic_array` field value: an element type (a schema `types` entry)
/// and the elements, each already in that type's `WireValue` form.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DynArray {
    pub(super) element_type: &'static str,
    pub(super) values: Vec<WireValue>,
}

impl AsBrdbValue for DynArray {
    fn as_brdb_dynamic_array(&self) -> Result<(&str, BrdbArrayIter<'_>), brdb::BrdbSchemaError> {
        Ok((
            self.element_type,
            Box::new(self.values.iter().map(|v| v as &dyn AsBrdbValue)),
        ))
    }
}

/// A `dynamic_map` field value: key and value types (schema `types` entries)
/// and the entries, each already in its type's `WireValue` form.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DynMap {
    pub(super) key_type: &'static str,
    pub(super) value_type: &'static str,
    pub(super) entries: Vec<(WireValue, WireValue)>,
}

impl AsBrdbValue for DynMap {
    fn as_brdb_dynamic_map(&self) -> Result<(&str, &str, BrdbMapIter<'_>), brdb::BrdbSchemaError> {
        Ok((
            self.key_type,
            self.value_type,
            Box::new(
                self.entries
                    .iter()
                    .map(|(k, v)| (k as &dyn AsBrdbValue, v as &dyn AsBrdbValue)),
            ),
        ))
    }
}

/// `lit` read as a value of dynamic type `ty` (a [`dynamic_type_name`]).
/// Numeric types read across literal kinds so a raw literal that reaches emit
/// still bakes correctly instead of hitting the type's zero; anything that
/// cannot convert (and every object reference, which a save cannot hold)
/// becomes the type's default.
fn wire_value_as(ty: &str, lit: &Literal) -> WireValue {
    match (ty, lit) {
        ("i64", Literal::Int(n)) => WireValue::Int(*n),
        ("i64", Literal::Float(f)) => WireValue::Int(*f as i64),
        ("i64", Literal::Bool(b)) => WireValue::Int(*b as i64),
        ("f64", Literal::Float(f)) => WireValue::Number(*f),
        ("f64", Literal::Int(n)) => WireValue::Number(*n as f64),
        ("f64", Literal::Bool(b)) => WireValue::Number(*b as i64 as f64),
        ("Bool", Literal::Bool(b)) => WireValue::Bool(*b),
        ("Bool", Literal::Int(n)) => WireValue::Bool(*n != 0),
        ("Str", Literal::String(s)) => WireValue::Str(s.clone()),
        ("Vector", Literal::Vector { .. })
        | ("Rotator", Literal::Rotator { .. })
        | ("Quat", Literal::Quat { .. })
        | ("LinearColor", Literal::LinearColor { .. } | Literal::Color { .. }) => {
            literal_to_wire_value(lit).expect("composite literal has a wire value")
        }
        ("i64", _) => WireValue::Int(0),
        ("Bool", _) => WireValue::Bool(false),
        ("Str", _) => WireValue::Str(String::new()),
        ("Vector", _) => WireValue::Vector {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        ("Rotator", _) => WireValue::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        },
        ("Quat", _) => WireValue::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        ("LinearColor", _) => WireValue::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        ("weak_object", _) => WireValue::Object,
        _ => WireValue::Number(0.0),
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

/// The empty `dynamic_map` a `MapVar` brick serializes, typed from the
/// declared `Map<K, V>` on its `MapVarRef` output port. Keys are
/// int/string/object (a bool key stores as an int); values cover the
/// wire-storable scalars. Defaults to `int -> float` for an unknown or
/// degenerate type.
pub(super) fn map_from_type(ty: &crate::ir::Type) -> DynMap {
    use crate::ir::Type;
    let inner = match ty {
        Type::Ref(inner) => inner.as_ref(),
        other => other,
    };
    let (key_type, value_type) = match inner {
        Type::Map(k, v) => {
            let key_type = match k.as_ref() {
                Type::Int | Type::Bool => "i64",
                Type::String => "Str",
                Type::Brick => "brick_handle",
                _ => "weak_object",
            };
            (key_type, dynamic_type_name(Some(v.as_ref())))
        }
        _ => ("i64", "f64"),
    };
    DynMap {
        key_type,
        value_type,
        entries: Vec::new(),
    }
}

/// `map` populated from a map's constant entries, each literal read in the
/// map's key/value type.
pub(super) fn map_from_literals(map: DynMap, entries: &[(Literal, Literal)]) -> DynMap {
    let entries = entries
        .iter()
        .map(|(k, v)| (wire_value_as(map.key_type, k), wire_value_as(map.value_type, v)))
        .collect();
    DynMap { entries, ..map }
}

/// The `dynamic_array` an `ArrayVar` brick serializes, typed by the array's
/// element type and holding its constant initial elements (none for an array
/// that starts empty). Object references start null: a save cannot hold one.
pub(super) fn array_from_literals(elem: Option<&crate::ir::Type>, lits: &[Literal]) -> DynArray {
    let element_type = dynamic_type_name(elem);
    DynArray {
        element_type,
        values: lits.iter().map(|l| wire_value_as(element_type, l)).collect(),
    }
}

/// Convert a Literal to a `WireValue`, falling back to the port type's
/// default for a literal with no wire form.
pub(super) fn literal_to_wire_value_or_default(
    lit: &Literal,
    ports: &crate::ir::GateIO,
    field: &str,
) -> WireValue {
    if let Some(wv) = literal_to_wire_value(lit) {
        return wv;
    }
    let port_ty = ports
        .inputs
        .iter()
        .chain(ports.outputs.iter())
        .find(|p| resolve(p.name) == field)
        .map(|p| &p.ty);
    var_type_to_wire_value(port_ty)
}

/// Convert a Literal to its native string representation (str/i32/f64/bool as text).
///
/// `None` for a literal with no native-string representation — a `const`
/// record, which typecheck's validators are supposed to keep out of a
/// `FieldKind::Str` field entirely (see the caller in `components.rs`, which
/// turns a `None` here into `EmitError::UnrepresentableLiteral` instead of
/// panicking).
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
            n => Err(brdb::BrdbSchemaError::MissingStructField(
                "Vector".into(),
                n.into(),
            )),
        }
    }
}

/// Unreal's `FRotator::Quaternion()`: the euler (Pitch/Yaw/Roll, degrees) to
/// quaternion conversion the engine applies itself when a rotation reaches a
/// quaternion sink. Pitch turns about Y, Yaw about Z, Roll about X, and the
/// sign pattern is UE's own, not the textbook one; angles are unwound to a
/// single turn first, as `FRotator::Quaternion` does.
///
/// Cross-checked against the certified rotation cases in
/// `data/gate_semantics.json`: a pure +45 degree yaw gives
/// `Quat(0, 0, 0.38268, 0.92388)`, the exact quaternion `RotateVector`'s
/// probe was run with (see `lower/fold/eval.rs::rotate_vector`).
pub(super) fn rotator_to_quat(pitch: f64, yaw: f64, roll: f64) -> (f64, f64, f64, f64) {
    let half = |deg: f64| (deg % 360.0).to_radians() / 2.0;
    let (sp, cp) = half(pitch).sin_cos();
    let (sy, cy) = half(yaw).sin_cos();
    let (sr, cr) = half(roll).sin_cos();
    (
        cr * sp * sy - sr * cp * cy,
        -cr * sp * cy - sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    )
}

/// Reshape a constant so it can be BAKED into a native `struct_ty`
/// (`Vector`/`Rotator`/`Quat`) field, whose properties are written one by one
/// and therefore admit exactly one representation. `None` when the literal
/// already fits, or when no conversion applies — the caller keeps the literal
/// as-is, and `port_accepts_inline_variant` has already refused to bake the
/// shapes that would not serialize.
///
/// Only euler -> quaternion is converted, because only it is reachable: a
/// `Rotation(p, y, r)` constant folds to a `Literal::Rotator` and flows into
/// quaternion sinks such as `RotateVector.Rotation`. The reverse never occurs
/// (no constructor folds to a `Literal::Quat` on its own), so a `Quat`
/// constant bound for a `Rotator` field keeps its `MakeQuaternion` carrier
/// and lets the game convert, rather than carrying unexercised code here.
pub(super) fn reshape_literal_for_struct(struct_ty: &str, lit: &Literal) -> Option<Literal> {
    match (struct_ty, lit) {
        ("Quat", Literal::Rotator { pitch, yaw, roll }) => {
            let (x, y, z, w) = rotator_to_quat(*pitch, *yaw, *roll);
            Some(Literal::Quat { x, y, z, w })
        }
        _ => None,
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
            n => Err(brdb::BrdbSchemaError::MissingStructField(
                "Rotator".into(),
                n.into(),
            )),
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
            n => Err(brdb::BrdbSchemaError::MissingStructField(
                "Quat".into(),
                n.into(),
            )),
        }
    }
}

pub(super) fn literal_to_wire_value(lit: &Literal) -> Option<WireValue> {
    match lit {
        Literal::Int(n) => Some(WireValue::Int(*n)),
        Literal::Float(f) => Some(WireValue::Number(*f)),
        Literal::Bool(b) => Some(WireValue::Bool(*b)),
        Literal::Object => Some(WireValue::Object),
        Literal::String(s) => Some(WireValue::Str(s.clone())),
        Literal::Vector { x, y, z } => Some(WireValue::Vector {
            x: *x,
            y: *y,
            z: *z,
        }),
        Literal::Rotator { pitch, yaw, roll } => Some(WireValue::Rotator {
            pitch: *pitch,
            yaw: *yaw,
            roll: *roll,
        }),
        Literal::Quat { x, y, z, w } => Some(WireValue::Quat {
            x: *x,
            y: *y,
            z: *z,
            w: *w,
        }),
        Literal::LinearColor { r, g, b, a } => Some(WireValue::LinearColor {
            r: *r as f32,
            g: *g as f32,
            b: *b as f32,
            a: *a as f32,
        }),
        // sRGB byte color (brick paint) → linear-ish 0–1. Only reached if a
        // paint literal ends up on a wire-value port.
        Literal::Color { r, g, b, a } => Some(WireValue::LinearColor {
            r: *r as f32 / 255.0,
            g: *g as f32 / 255.0,
            b: *b as f32 / 255.0,
            a: *a as f32 / 255.0,
        }),
        // No wire-value form. `Record` belongs here with its siblings:
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
