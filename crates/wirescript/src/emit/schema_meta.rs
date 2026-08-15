//! Queries against the brdb component schema: a gate's data struct,
//! its field kinds, and which ports accept an inlined value.

use super::*;

/// Returns `(struct_name, field_names, use_wire_variant)` for gates whose
/// component data struct must be serialized.
///
/// Fully derived: `brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS` (the
/// game-extracted component→struct table, exhaustive over the game's
/// components) supplies the struct, the max schema supplies the field list —
/// see [`derived_gate_data`]. A class in neither place has no data component.
/// Per-field schema checks in `build_gate_component` decide variant vs native
/// handling, so the `use_wire_variant` flag is always false here.
pub(super) fn data_struct_for_gate(gate_class: &str) -> Option<(&'static str, &'static [&'static str], bool)> {
    derived_gate_data()
        .get(gate_class)
        .map(|(s, f)| (*s, f.as_slice(), false))
}

/// (struct name, full field list) per component class, derived from
/// `brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS` + the max schema.
/// Hand-written arms in [`data_struct_for_gate`] take precedence — they
/// encode the deliberate exceptions (wire-only gates, struct overrides,
/// classes absent from the pair table).
pub(super) fn derived_gate_data() -> &'static StdMap<&'static str, (&'static str, Vec<&'static str>)> {
    static MAP: std::sync::OnceLock<StdMap<&'static str, (&'static str, Vec<&'static str>)>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut m = StdMap::new();
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

/// Per-field emit classification, resolved once per gate data struct
/// instead of re-querying the schema (several interner probes plus a
/// `String` allocation per predicate) for every field of every brick.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldKind {
    /// `WireGraphVariant`
    WireVariant,
    /// `WireGraphPrimMathVariant` (no bool member — bools coerce to int)
    PrimMathVariant,
    /// Asset reference (`class` / `object`)
    AssetRef,
    /// `bundle_path_ref` (embedded prefab path)
    BundlePathRef,
    /// `str`
    Str,
    /// Schema enum type (payload = the enum's type name in the schema)
    Enum(&'static str),
    /// Anything else — serialized as a native literal
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
    static MAP: std::sync::OnceLock<StdMap<&'static str, Vec<FieldMeta>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut m = StdMap::new();
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
                            Some("WireGraphVariant") => FieldKind::WireVariant,
                            Some("WireGraphPrimMathVariant") => FieldKind::PrimMathVariant,
                            Some("class") | Some("object") => FieldKind::AssetRef,
                            Some("bundle_path_ref") => FieldKind::BundlePathRef,
                            Some("str") => FieldKind::Str,
                            Some(n) if schema.get_enum(n).is_some() => FieldKind::Enum(n),
                            _ => FieldKind::Native,
                        },
                        // Array/FlatArray/Map fields have no special emit
                        // handling — the native literal path covers them.
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
    .map(|f| f.as_slice())
}

/// Check if `schema_str` contains `"field_name: type_name"` as an exact type match.
pub(super) fn schema_field_type_str(struct_name: &str, field: &str) -> Option<String> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let s = schema.get_struct(struct_name)?;
    let field_id = schema.intern.get(field)?;
    let prop = s.get(&field_id)?;
    Some(prop.as_string(schema))
}

/// The enum variant names a gate's data `field` accepts, if that field is an
/// enum — e.g. DisplayText's `Justification` → `["Left", "Center", "Right"]`.
/// Names are returned bare (the `EnumType::` prefix stripped). Used by the
/// editor to complete enum-valued named args like `justify = Center`.
pub fn field_enum_values(gate_class: &str, field: &str) -> Option<Vec<String>> {
    // Delegate to the catalog's schema-backed enum helpers, which drop the
    // trailing `<Enum>_MAX` / bare `MAX` sentinel (not a selectable member).
    let members = crate::catalog::enum_member_names(field_enum_type(gate_class, field)?);
    if members.is_empty() { None } else { Some(members) }
}

/// The schema enum type name of a gate's data field, if it is an enum (e.g.
/// `ColorBlend.BlendSpace` → `EBRColorSpace`). Used to name the enum in
/// completions/hover for a config param that is surfaced as a plain int.
pub fn field_enum_type(gate_class: &str, field: &str) -> Option<&'static str> {
    crate::catalog::config_field_enum_type(gate_class, field)
}

/// True if the field's schema type is any wire-graph variant — plain
/// (`WireGraphVariant`) or prim-math (`WireGraphPrimMathVariant`).
fn schema_field_is_wire_variant(struct_name: &str, field: &str) -> bool {
    matches!(
        schema_field_type_str(struct_name, field).as_deref(),
        Some("WireGraphVariant" | "WireGraphPrimMathVariant")
    )
}

/// Can a folded constant (`Vec/Rotation/Color` on literal args, lowered to a
/// `_Literal` node) be delivered to this (gate, port) sink as inlined
/// component data? True for wire-variant fields and for native
/// `Vector`/`Rotator`/`Quat` struct fields (the gate stores an unwired
/// input's value in its data — entity `Set*` gates, Sweep, …). Everything
/// else — `LinearColor` fields, `Split*` inputs, chip IO, unmapped gates —
/// must keep a real `Make*` gate, which the lowering pass materializes on
/// demand.
pub(crate) fn port_accepts_inline_variant(gate_class: &str, port: WirePort) -> bool {
    let Some((struct_name, fields, use_wire_variant)) = data_struct_for_gate(gate_class) else {
        return false;
    };
    let field = port.as_str();
    if !fields.contains(&field) {
        return false;
    }
    if use_wire_variant || schema_field_is_wire_variant(struct_name, field) {
        return true;
    }
    // Split* gates keep materialized Make* inputs — not yet verified that
    // they read an unwired input from data like the Set* gates do.
    if gate_class.contains("_Expr_Split") {
        return false;
    }
    matches!(
        schema_field_type_str(struct_name, field).as_deref(),
        Some("Vector" | "Rotator" | "Quat")
    )
}

/// Whether the constant `lit` may be BAKED into this gate's data field named by
/// `port` as a NATIVE scalar (dropping the carrying wire), i.e. `port` names a
/// plain scalar field on the gate's data struct AND `lit`'s type matches the
/// field's storage type. A matching constant is written into the gate's data and
/// read from the unwired input — this is a native-scalar inline, distinct from
/// the wire-variant inline in [`port_accepts_inline_variant`] (a plain `i64` /
/// `bool` field can hold a native constant but NOT a wire variant). A TYPE
/// MISMATCH (e.g. a float into an `i64` field) is rejected here, so it falls
/// through to a carrier gate that supplies — and converts — the value over a
/// wire. Enum / object / class / composite fields are not baked here.
pub(crate) fn port_accepts_inline_scalar(gate_class: &str, port: WirePort, lit: &Literal) -> bool {
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

/// Resolve a literal against a schema enum type by name — exact match
/// first (`EBRDisplayTextJustification::Left`), then bare suffix (`Left`).
pub(super) fn resolve_enum_value(type_name: &str, lit: &Literal) -> Option<u8> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let enum_def = schema.get_enum(type_name)?;
    match lit {
        Literal::Int(n) => Some(*n as u8),
        Literal::String(s) => {
            // Try exact match first ("EBRDisplayTextJustification::Left"),
            // then bare suffix ("Left").
            if let Some(id) = schema.intern.get(s) {
                if let Some(&v) = enum_def.get(&id) {
                    return Some(v as u8);
                }
            }
            let prefixed = format!("{type_name}::{s}");
            let id = schema.intern.get(&prefixed)?;
            Some(*enum_def.get(&id)? as u8)
        }
        Literal::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}
