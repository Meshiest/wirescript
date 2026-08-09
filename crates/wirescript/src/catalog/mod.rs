//! Gate catalog — the authoritative registry of every wire-graph brick
//! the compiler knows about. The inventory JSON is baked into the binary
//! via `include_str!`.

pub mod arrays;
pub mod calls;
pub mod events;
pub mod gate_builtins;
pub mod maps;
pub mod operators;

use crate::collections::HashMap;

use serde::Deserialize;

/// Raw port-type tags as they appear in the JSON. The typecheck phase
/// (Phase 3) maps these to the `Type` ADT.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RawPortType {
    Bool,
    Int,
    Float,
    String,
    Vector,
    Rotator,
    Color,
    Entity,
    Character,
    Controller,
    VarRef,
    ArrayVarRef,
    Exec,
    Any,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CompositeKind {
    Vector,
    Color,
    Rotator,
    Struct,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompositeShape {
    pub kind: CompositeKind,
    #[serde(rename = "subPorts")]
    pub sub_ports: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Port {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(rename = "type")]
    pub ty: RawPortType,
    #[serde(default)]
    pub composite: Option<CompositeShape>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ComponentKind {
    Expr,
    Exec,
    Pseudo,
    Internal,
    Fake,
    Auto,
    Wiregraph,
    /// JSON sometimes uses `?` for unclassified components.
    #[serde(rename = "?")]
    Unknown,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentSpec {
    pub class: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// Space-separated search keywords for the gate picker (from the game's
    /// SearchTags metadata); empty for gates the game hasn't tagged.
    #[serde(default)]
    pub search_tags: String,
    pub kind: ComponentKind,
    #[serde(default)]
    pub family: String,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    /// Settings-menu (non-wire) config properties the game exposes on this gate
    /// — e.g. Sweep's `bOnlyHitPlayerBodyParts`. Not wireable; set via the
    /// gate's editor settings. Empty for gates without any.
    #[serde(default)]
    pub config: Vec<ConfigProperty>,
}

/// A non-wire config property on a component (settings-menu field).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProperty {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    /// Simple type category: bool | int | float | string | enum | struct |
    /// array | object | any.
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateSpec {
    pub brick_asset: String,
    pub brick_display_name: String,
    #[serde(default)]
    pub brick_summary: String,
    pub half_size: HalfSize,
    pub component: ComponentSpec,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct HalfSize {
    #[serde(rename = "X")]
    pub x: i32,
    #[serde(rename = "Y")]
    pub y: i32,
    #[serde(rename = "Z")]
    pub z: i32,
}

#[derive(Clone, Debug, Deserialize)]
struct RawInventory {
    entries: Vec<GateSpec>,
    #[serde(default)]
    type_glossary: Option<HashMap<String, String>>,
}

/// Read-only catalog view. Built once at startup; the compiler queries by
/// display-name / class / family / kind.
pub struct Catalog {
    entries: Vec<GateSpec>,
    by_display: HashMap<String, usize>,
    by_class: HashMap<String, usize>,
    by_family: HashMap<String, Vec<usize>>,
    by_kind: HashMap<ComponentKind, Vec<usize>>,
    type_glossary: HashMap<String, String>,
}

impl Catalog {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let raw: RawInventory = serde_json::from_str(json)?;
        Ok(Self::from_raw(raw))
    }

    fn from_raw(raw: RawInventory) -> Self {
        let mut by_display = HashMap::default();
        let mut by_class = HashMap::default();
        let mut by_family: HashMap<String, Vec<usize>> = HashMap::default();
        let mut by_kind: HashMap<ComponentKind, Vec<usize>> = HashMap::default();
        for (i, g) in raw.entries.iter().enumerate() {
            by_display.insert(g.brick_display_name.clone(), i);
            by_class.insert(g.component.class.clone(), i);
            by_family
                .entry(g.component.family.clone())
                .or_default()
                .push(i);
            by_kind.entry(g.component.kind).or_default().push(i);
        }
        Self {
            entries: raw.entries,
            by_display,
            by_class,
            by_family,
            by_kind,
            type_glossary: raw.type_glossary.unwrap_or_default(),
        }
    }

    pub fn find_by_display_name(&self, name: &str) -> Option<&GateSpec> {
        self.by_display.get(name).map(|&i| &self.entries[i])
    }
    pub fn find_by_class(&self, class: &str) -> Option<&GateSpec> {
        self.by_class.get(class).map(|&i| &self.entries[i])
    }
    pub fn all_of_family(&self, family: &str) -> impl Iterator<Item = &GateSpec> {
        self.by_family
            .get(family)
            .into_iter()
            .flat_map(|ixs| ixs.iter().map(|&i| &self.entries[i]))
    }
    pub fn all_of_kind(&self, kind: ComponentKind) -> impl Iterator<Item = &GateSpec> {
        self.by_kind
            .get(&kind)
            .into_iter()
            .flat_map(|ixs| ixs.iter().map(|&i| &self.entries[i]))
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn type_glossary(&self) -> &HashMap<String, String> {
        &self.type_glossary
    }
    pub fn entries(&self) -> &[GateSpec] {
        &self.entries
    }

    /// Is `port` a wire input on `gate_class`, per the game's own dump?
    ///
    /// A gate can carry settable values that are *not* wireable — e.g.
    /// `DisplayText.FontSize` is a data field with a default but has no input
    /// port. Wiring one produces a file the game rejects at load, so a constant
    /// bound to such a port has to be written as data instead.
    ///
    /// Unknown classes (pseudo gates, internals) answer `true`: they are not in
    /// the inventory, and treating them as wireable keeps existing behavior.
    pub fn is_wire_input(&self, gate_class: &str, port: &str) -> bool {
        match self.find_by_class(gate_class) {
            Some(spec) => spec.component.inputs.iter().any(|p| p.name == port),
            None => true,
        }
    }
}

/// [`Catalog::is_wire_input`] against the bundled inventory.
pub fn is_wire_input(gate_class: &str, port: &str) -> bool {
    default_catalog().is_wire_input(gate_class, port)
}

/// The gate's non-wire config property named `field`, if it exists AND is a
/// type this compiler can bake into a data-struct field: `bool | int | float |
/// string | enum`. Complex config (`struct | array | object | any`) is
/// intentionally excluded — those are deferred or already handled by other
/// mechanisms (asset refs, `WeaponAmmoOverride`, `Value` literal init, …), and
/// filtering by type means we never double-model them. Drives the data-driven
/// config-attribute path: any such field is settable via `<FieldName> = value`.
pub fn scalar_config_field(gate_class: &str, field: &str) -> Option<&'static ConfigProperty> {
    let prop = default_catalog()
        .find_by_class(gate_class)?
        .component
        .config
        .iter()
        .find(|c| c.name == field)?;
    matches!(prop.ty.as_str(), "bool" | "int" | "float" | "string" | "enum").then_some(prop)
}

/// A gate's settable scalar config properties (bool/int/float/string/enum), for
/// completion/hover. Empty for gates without config or not in the inventory.
pub fn scalar_config_fields(gate_class: &str) -> Vec<&'static ConfigProperty> {
    default_catalog()
        .find_by_class(gate_class)
        .map(|gs| {
            gs.component
                .config
                .iter()
                .filter(|c| matches!(c.ty.as_str(), "bool" | "int" | "float" | "string" | "enum"))
                .collect()
        })
        .unwrap_or_default()
}

/// The schema enum type name of a gate's data-struct `field`, if that field is
/// a schema enum (e.g. `MathEasing.Function` → `EBREasingFunction`). Returns
/// `None` when the gate has no data struct, the field is absent, or the field
/// is a non-enum type. Used to validate/resolve enum-typed config args.
pub fn config_field_enum_type(gate_class: &str, field: &str) -> Option<&'static str> {
    use brdb::schema::BrdbSchemaStructProperty;
    let schema = brdb::schemas::bricks_components_schema_max();
    let struct_name = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(comp, _)| *comp == gate_class)
        .map(|(_, strct)| *strct)?;
    let s = schema.get_struct(struct_name)?;
    let field_id = schema.intern.get(field)?;
    let BrdbSchemaStructProperty::Type(ty_id) = s.get(&field_id)? else {
        return None;
    };
    let ty = schema.intern.lookup_ref(*ty_id)?;
    schema.get_enum(ty).is_some().then_some(ty)
}

/// The schema enum backing a config named-arg `arg` on a builtin call OR event
/// `callee` — if `arg` is a non-wire config param whose field is a schema enum.
/// Unifies the call path (`CallSpec` params → `config_field_enum_type`) with the
/// event path (`EventSpec.config_named` → its gate field), so hover and
/// completion resolve `SweepSimple(direction = …)` and `on Clock(…)` the same
/// way. `None` for wire inputs, unknown args, or non-enum config.
pub fn config_enum_for_named_arg(callee: &str, arg: &str) -> Option<&'static str> {
    if let Some(spec) = crate::catalog::calls::calls().get(callee) {
        if let Some(p) = spec.params.iter().find(|p| p.name == arg) {
            if is_wire_input(spec.gate_class, p.port.as_str()) {
                return None;
            }
            return config_field_enum_type(spec.gate_class, p.port.as_str());
        }
        // Not a declared param: a data-driven config field set by its raw name.
        if scalar_config_field(spec.gate_class, arg).is_some() {
            return config_field_enum_type(spec.gate_class, arg);
        }
        return None;
    }
    if let Some(evt) = crate::catalog::events::find_event(callee) {
        let key = arg.to_ascii_lowercase();
        let field = evt
            .config_named
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, f)| *f)?;
        return config_field_enum_type(evt.gate_class, field);
    }
    None
}

/// Is `bare` the trailing `<Enum>_MAX` / bare `MAX` sentinel — the enum's count
/// bound, not a real selectable member? (Most enums spell it `<Enum>_MAX`; a few,
/// e.g. EBrickDirection / EBrickAxis, spell it bare `MAX`.)
fn is_enum_sentinel(bare: &str) -> bool {
    bare == "MAX" || bare.ends_with("_MAX")
}

/// The integer value of enum member `member` in schema enum `enum_type`
/// (accepting either the bare member name or the `EnumType::Member` form), or
/// `None` if the enum/member is unknown. The trailing `_MAX` sentinel is not a
/// selectable member and resolves to `None`.
pub fn enum_member_value(enum_type: &str, member: &str) -> Option<i64> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let enum_def = schema.get_enum(enum_type)?;
    for (id, v) in enum_def.iter() {
        let Some(full) = schema.intern.lookup_ref(*id) else {
            continue;
        };
        let bare = full.rsplit("::").next().unwrap_or(full);
        if is_enum_sentinel(bare) {
            continue;
        }
        if full == member || bare == member {
            return Some(*v as i64);
        }
    }
    None
}

/// The bare member names of schema enum `enum_type` (the `EnumType::` prefix
/// stripped, the `_MAX` sentinel dropped), for diagnostics; empty when the enum
/// is unknown.
pub fn enum_member_names(enum_type: &str) -> Vec<String> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let Some(enum_def) = schema.get_enum(enum_type) else {
        return Vec::new();
    };
    enum_def
        .keys()
        .filter_map(|k| schema.intern.lookup_ref(*k))
        .map(|name| name.rsplit("::").next().unwrap_or(name))
        .filter(|bare| !is_enum_sentinel(bare))
        .map(|bare| bare.to_string())
        .collect()
}

/// True if `value` is a real (non-sentinel) member value of schema enum
/// `enum_type`.
pub fn enum_has_value(enum_type: &str, value: i64) -> bool {
    let schema = brdb::schemas::bricks_components_schema_max();
    let Some(enum_def) = schema.get_enum(enum_type) else {
        return false;
    };
    enum_def.iter().any(|(id, v)| {
        *v as i64 == value
            && schema
                .intern
                .lookup_ref(*id)
                .map(|full| full.rsplit("::").next().unwrap_or(full))
                .is_some_and(|bare| !is_enum_sentinel(bare))
    })
}

/// The default catalog, parsed from the bundled inventory JSON on first
/// call and reused on subsequent calls.
pub fn default_catalog() -> &'static Catalog {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Catalog> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let json = include_str!("../../data/logic_gate_inventory.simple.json");
        Catalog::from_json(json).expect("default inventory JSON parses")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_catalog_loads() {
        let cat = default_catalog();
        assert!(cat.len() > 50, "catalog should have real entries");
    }

    #[test]
    fn lookup_by_class_roundtrips() {
        let cat = default_catalog();
        let g = cat
            .find_by_class("Component_Internal_Rerouter")
            .expect("rerouter exists in catalog");
        assert_eq!(g.brick_asset, "B_1x1_Reroute_Node");
    }

    #[test]
    fn config_field_enum_type_reads_schema() {
        // Enum-typed data fields resolve to their schema enum; non-enum and
        // unknown fields resolve to None. Enum member values/names come from the
        // schema (verified against exact values in the gate-config lowering tests).
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathEasing", "Function")
                .is_some(),
            "MathEasing.Function is an enum config field"
        );
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Exec_SweepSimple", "Direction")
                .is_some(),
            "SweepSimple.Direction is an enum config field"
        );
        // A bool data field is not an enum.
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathBlend", "bClampAlpha")
                .is_none()
        );
        // A field the struct does not have.
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathBlend", "Nonexistent")
                .is_none()
        );
        // Enum member lookups are schema-backed and consistent both ways.
        if let Some(et) =
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathEasing", "Function")
        {
            assert!(!enum_member_names(et).is_empty());
            if let Some(v) = enum_member_value(et, "Bounce") {
                assert!(enum_has_value(et, v));
            }
        }
        assert!(!enum_has_value("EBREasingFunction", 9999));
    }

    #[test]
    fn config_enum_for_named_arg_resolves_calls_and_events() {
        // Call path: SweepSimple's `direction` config → EBrickDirection.
        assert_eq!(
            config_enum_for_named_arg("SweepSimple", "direction"),
            Some("EBrickDirection")
        );
        // Event path: Clock's config is all bool/float — no enum.
        assert_eq!(config_enum_for_named_arg("Clock", "enabled"), None);
        // A wired input (not config) never resolves as a config enum.
        assert_eq!(config_enum_for_named_arg("Clock", "interval"), None);
        // Unknown callee / arg.
        assert_eq!(config_enum_for_named_arg("Nonexistent", "x"), None);
        assert_eq!(config_enum_for_named_arg("SweepSimple", "notAParam"), None);
    }

    #[test]
    fn enum_members_exclude_sentinel() {
        // EBRColorSpace: Linear=0, Srgb=1, Oklab=2, Hsv=3, _MAX=4 (sentinel dropped).
        let names = enum_member_names("EBRColorSpace");
        assert_eq!(names, vec!["Linear", "Srgb", "Oklab", "Hsv"]);
        assert_eq!(enum_member_value("EBRColorSpace", "Srgb"), Some(1));
        assert_eq!(enum_member_value("EBRColorSpace", "EBRColorSpace::Oklab"), Some(2));
        assert_eq!(enum_member_value("EBRColorSpace", "Nope"), None);
        assert!(enum_has_value("EBRColorSpace", 3));
        assert!(!enum_has_value("EBRColorSpace", 99));
        // The `_MAX` sentinel is not a selectable member — by name or by value.
        assert_eq!(enum_member_value("EBRColorSpace", "EBRColorSpace_MAX"), None);
        assert!(!enum_has_value("EBRColorSpace", 4)); // 4 == the sentinel value
        // EBrickDirection spells its sentinel bare `MAX` — also dropped.
        let dirs = enum_member_names("EBrickDirection");
        assert!(!dirs.iter().any(|d| d == "MAX"));
        assert_eq!(dirs.first().map(String::as_str), Some("X_Positive"));
        assert_eq!(enum_member_value("EBrickDirection", "MAX"), None);
    }
}
