//! Queries against the brdb component schema: a gate's data struct,
//! its field kinds, and which ports accept an inlined value.

use crate::collections::HashMap;
use crate::intern::{Sym, intern_static};
use crate::ir::Literal;
use crate::ir::port_registry::WirePort;

/// Returns `(struct_name, field_names, use_wire_variant)` for gates whose component data
/// struct must be serialized.
///
/// Both the struct and its field list come from [`derived_gate_data`]; a class absent there
/// has no data component. `use_wire_variant` is always false: `build_gate_component` decides
/// variant vs native handling per field from the schema.
pub(crate) fn data_struct_for_gate(
    gate_class: &str,
) -> Option<(&'static str, &'static [&'static str], bool)> {
    derived_gate_data()
        .get(gate_class)
        .map(|(s, f)| (*s, f.as_slice(), false))
}

/// (struct name, full field list) per component class, derived from
/// `brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS` and the max schema.
pub(super) fn derived_gate_data()
-> &'static HashMap<&'static str, (&'static str, Vec<&'static str>)> {
    static MAP: std::sync::OnceLock<HashMap<&'static str, (&'static str, Vec<&'static str>)>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut m = HashMap::default();
        for (comp, strct) in brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS {
            let Some(s) = schema.get_struct(strct) else {
                continue;
            };
            let fields: Vec<&'static str> = s
                .keys()
                .filter_map(|k| schema.intern.lookup_ref(*k))
                .collect();
            m.insert(*comp, (*strct, fields));
        }
        m
    })
}

/// Per-field emit classification, resolved once per gate data struct so emit does not
/// re-query the schema for every field of every brick.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldKind {
    /// `WireValue`
    WireValue,
    /// Asset reference (`class` / `object`)
    AssetRef,
    /// `str` (also the embedded-prefab path on prefab spawners)
    Str,
    /// Schema enum type (payload = the enum's type name in the schema)
    Enum(&'static str),
    /// A native `Vector`/`Rotator`/`Quat` struct field (payload = the schema
    /// struct name), which the inlined literal is reshaped to fit by
    /// [`super::variants::reshape_literal_for_struct`].
    NativeStruct(&'static str),
    /// A native integer field (payload = its schema type, `i8` .. `u64`)
    Integer(&'static str),
    /// A native float field (payload = its schema type, `f32` or `f64`)
    Real(&'static str),
    /// A native `bool` field
    Boolean,
    /// Anything else, serialized as a native literal
    Native,
}

pub(super) struct FieldMeta {
    pub(super) name: &'static str,
    /// `name` pre-interned, so the per-brick inlined-literal lookup skips
    /// the interner.
    pub(super) sym: Sym,
    pub(super) kind: FieldKind,
}

/// Field metadata for a gate's component data struct: same source as
/// [`derived_gate_data`] (pair table × max schema), with each field's
/// emit classification and interned name computed once.
pub(super) fn gate_field_meta(gate_class: &str) -> Option<&'static [FieldMeta]> {
    use brdb::schema::BrdbSchemaStructProperty;
    static MAP: std::sync::OnceLock<HashMap<&'static str, Vec<FieldMeta>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut m = HashMap::default();
        for (comp, strct) in brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS {
            let Some(s) = schema.get_struct(strct) else {
                continue;
            };
            let fields: Vec<FieldMeta> = s
                .iter()
                .filter_map(|(k, prop)| {
                    let name = schema.intern.lookup_ref(*k)?;
                    let kind = match prop {
                        BrdbSchemaStructProperty::Type(t) => match schema.intern.lookup_ref(*t) {
                            Some("WireValue") => FieldKind::WireValue,
                            Some("class" | "object") => FieldKind::AssetRef,
                            Some("str") => FieldKind::Str,
                            Some(n) if schema.get_enum(n).is_some() => FieldKind::Enum(n),
                            Some(n @ ("Vector" | "Rotator" | "Quat")) => FieldKind::NativeStruct(n),
                            Some(
                                n @ ("i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"),
                            ) => FieldKind::Integer(n),
                            Some(n @ ("f32" | "f64")) => FieldKind::Real(n),
                            Some("bool") => FieldKind::Boolean,
                            _ => FieldKind::Native,
                        },
                        // Array/FlatArray/Map fields take the native literal path.
                        _ => FieldKind::Native,
                    };
                    Some(FieldMeta {
                        name,
                        sym: intern_static(name),
                        kind,
                    })
                })
                .collect();
            m.insert(*comp, fields);
        }
        m
    })
    .get(gate_class)
    .map(std::vec::Vec::as_slice)
}

/// The schema type of `field` on `struct_name`, rendered as a string, if both exist.
pub(super) fn schema_field_type_str(struct_name: &str, field: &str) -> Option<String> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let s = schema.get_struct(struct_name)?;
    let field_id = schema.intern.get(field)?;
    let prop = s.get(&field_id)?;
    Some(prop.as_string(schema))
}

/// The enum variant names a gate's data `field` accepts, if that field is an enum, e.g.
/// `DisplayText`'s `Justification` -> `["Left", "Center", "Right"]`. Names are bare (the
/// `EnumType::` prefix stripped).
pub fn field_enum_values(gate_class: &str, field: &str) -> Option<Vec<String>> {
    // Delegate to the catalog's schema-backed enum helpers, which drop the
    // trailing `<Enum>_MAX` / bare `MAX` sentinel (not a selectable member).
    let members = crate::catalog::enum_member_names(field_enum_type(gate_class, field)?);
    if members.is_empty() {
        None
    } else {
        Some(members)
    }
}

/// The schema enum type name of a gate's data field, if it is an enum (e.g.
/// `ColorBlend.BlendSpace` -> `EBRColorSpace`).
pub fn field_enum_type(gate_class: &str, field: &str) -> Option<&'static str> {
    crate::catalog::config_field_enum_type(gate_class, field)
}

/// True if the field's schema type is the `WireValue` variant.
fn schema_field_is_wire_value(struct_name: &str, field: &str) -> bool {
    schema_field_type_str(struct_name, field).as_deref() == Some("WireValue")
}

/// Whether a folded constant (`Vec/Rotation/Color` on literal args, lowered to a `_Literal`
/// node) can be delivered to this (gate, port) sink as inlined component data.
///
/// True for `WireValue` fields and for a native `Vector`/`Rotator`/`Quat` struct field whose
/// shape `lit` matches or that [`super::variants::reshape_literal_for_struct`] can produce
/// (a `Rotator` literal is accepted for a `Quat` field). Everything else (`LinearColor`
/// fields, `Split*` inputs, chip IO, unmapped gates, shape mismatches) keeps a real `Make*`
/// gate, which lowering materializes on demand.
pub fn port_accepts_inline_variant(gate_class: &str, port: WirePort, lit: &Literal) -> bool {
    let Some((struct_name, fields, use_wire_variant)) = data_struct_for_gate(gate_class) else {
        return false;
    };
    let field = port.as_str();
    if !fields.contains(&field) {
        return false;
    }
    if use_wire_variant || schema_field_is_wire_value(struct_name, field) {
        return true;
    }
    // Split* gates keep materialized Make* inputs (not verified to read an unwired input
    // from data).
    if gate_class.contains("_Expr_Split") {
        return false;
    }
    matches!(
        (schema_field_type_str(struct_name, field).as_deref(), lit),
        (Some("Vector"), Literal::Vector { .. })
            | (Some("Rotator"), Literal::Rotator { .. })
            | (Some("Quat"), Literal::Quat { .. } | Literal::Rotator { .. })
    )
}

/// Whether the constant `lit` can be baked into this gate's data field `port` as a native
/// scalar, dropping the carrying wire: `port` must name a plain scalar field and `lit`'s
/// type must match the field's storage type. A mismatch (a float into an `i64` field) falls
/// through to a carrier gate that converts the value over a wire. Enum, object, class and
/// composite fields are not baked here; wire-variant fields go through
/// [`port_accepts_inline_variant`].
pub fn port_accepts_inline_scalar(gate_class: &str, port: WirePort, lit: &Literal) -> bool {
    let Some((struct_name, fields, _)) = data_struct_for_gate(gate_class) else {
        return false;
    };
    let field = port.as_str();
    if !fields.contains(&field) {
        return false;
    }
    matches!(
        (schema_field_type_str(struct_name, field).as_deref(), lit),
        (Some("bool"), Literal::Bool(_))
            | (Some("f32" | "f64"), Literal::Float(_))
            | (
                Some("i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"),
                Literal::Int(_),
            )
            | (Some("str"), Literal::String(_))
    )
}

/// If the field's schema type is an enum, resolve `lit` to its integer
/// discriminant. Accepts both `Literal::Int` (passthrough) and
/// `Literal::String` (looked up by variant name, with or without the
/// enum-name prefix).
#[cfg(test)]
pub(super) fn try_resolve_enum(struct_name: &str, field: &str, lit: &Literal) -> Option<u8> {
    let type_name = schema_field_type_str(struct_name, field)?;
    resolve_enum_value(&type_name, lit)
}

/// Resolve a literal against a schema enum type by name: exact match first
/// (`EBRDisplayTextJustification::Left`), then bare suffix (`Left`).
pub(super) fn resolve_enum_value(type_name: &str, lit: &Literal) -> Option<u8> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let enum_def = schema.get_enum(type_name)?;
    match lit {
        Literal::Int(n) => Some(*n as u8),
        Literal::String(s) => {
            if let Some(id) = schema.intern.get(s)
                && let Some(&v) = enum_def.get(&id)
            {
                return Some(v as u8);
            }
            let prefixed = format!("{type_name}::{s}");
            let id = schema.intern.get(&prefixed)?;
            Some(*enum_def.get(&id)? as u8)
        }
        Literal::Bool(b) => Some(u8::from(*b)),
        _ => None,
    }
}
