//! Gate inventory: every wire-graph brick the compiler knows about, with its
//! ports, brick asset and size, and the schema enums behind its config
//! fields. The inventory JSON is baked into the binary via `include_str!`.

pub mod properties;

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
    Brick,
    VarRef,
    ArrayVarRef,
    Exec,
    Any,
}

impl RawPortType {
    /// The IR type of a port with this tag. The wildcard `any` port is
    /// `any_as` (an array gate's element type, say) and an exec port is
    /// `exec_as`; a reference is `any`.
    pub fn to_type(self, any_as: &crate::ir::Type, exec_as: &crate::ir::Type) -> crate::ir::Type {
        use crate::ir::Type;
        match self {
            RawPortType::Bool => Type::Bool,
            RawPortType::Int => Type::Int,
            RawPortType::Float => Type::Float,
            RawPortType::String => Type::String,
            RawPortType::Vector => Type::Vector,
            RawPortType::Rotator => Type::Rotator,
            RawPortType::Color => Type::Color,
            RawPortType::Entity => Type::Entity,
            RawPortType::Character => Type::Character,
            RawPortType::Controller => Type::Controller,
            RawPortType::Brick => Type::Brick,
            RawPortType::Any => any_as.clone(),
            RawPortType::Exec => exec_as.clone(),
            RawPortType::VarRef | RawPortType::ArrayVarRef => Type::Any,
        }
    }
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
    Event,
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
    /// The game's grouping (`Gameplay`, `Environment`, ...); empty for most
    /// logic gates.
    #[serde(default)]
    pub category: String,
    /// A player can add this component with the applicator (the CDO's
    /// `bPlayerSpawnable`).
    #[serde(default)]
    pub player_spawnable: bool,
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
    /// Sizes of a procedural gate brick (`PB_Gate_*`), smallest first. Empty
    /// for an ordinary basic-brick gate. A procedural gate is placed at
    /// exactly one variant's `half_size` (the game rejects any other size) and
    /// each size shows a different set of physical ports; every other logical
    /// port is still reachable through the omniport.
    #[serde(default)]
    pub variants: Vec<GateVariant>,
}

/// One placeable size of a procedural gate brick and the ports it shows.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateVariant {
    pub half_size: HalfSize,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
}

impl GateSpec {
    /// The smallest variant whose physical ports include every used port that
    /// any variant shows. `used` yields `(is_output, port)`; a port no variant
    /// shows (an omniport-only one such as `TicksToWait`) never affects the
    /// size. `None` for a gate without variants. When no variant shows them
    /// all, the largest is the closest fit.
    pub fn pick_variant<'a>(&self, used: impl IntoIterator<Item = (bool, &'a str)>) -> Option<usize> {
        let last = self.variants.len().checked_sub(1)?;
        let shown_by = |v: &GateVariant, out: bool, port: &str| {
            let list = if out { &v.outputs } else { &v.inputs };
            list.iter().any(|p| p == port)
        };
        let needed: Vec<(bool, &str)> = used
            .into_iter()
            .map(|(out, p)| (out, p.split('.').next().unwrap_or(p)))
            .filter(|&(out, p)| self.variants.iter().any(|v| shown_by(v, out, p)))
            .collect();
        Some(
            self.variants
                .iter()
                .position(|v| needed.iter().all(|&(out, p)| shown_by(v, out, p)))
                .unwrap_or(last),
        )
    }

    /// The half-size the brick is placed at: the chosen variant's for a
    /// procedural gate (the smallest when none was chosen or the index is
    /// stale), the catalog's own otherwise.
    pub fn placed_half_size(&self, variant: Option<usize>) -> HalfSize {
        match self.variants.as_slice() {
            [] => self.half_size,
            vs => vs.get(variant.unwrap_or(0)).unwrap_or(&vs[0]).half_size,
        }
    }

    /// A procedural gate brick (`PB_Gate_*`) rather than a basic one.
    pub fn is_procedural(&self) -> bool {
        !self.variants.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
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
/// gate class.
pub struct Catalog {
    entries: Vec<GateSpec>,
    by_class: HashMap<String, usize>,
    type_glossary: HashMap<String, String>,
}

impl Catalog {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let raw: RawInventory = serde_json::from_str(json)?;
        Ok(Self::from_raw(raw))
    }

    fn from_raw(raw: RawInventory) -> Self {
        let mut by_class = HashMap::default();
        for (i, g) in raw.entries.iter().enumerate() {
            by_class.insert(g.component.class.clone(), i);
        }
        Self {
            entries: raw.entries,
            by_class,
            type_glossary: raw.type_glossary.unwrap_or_default(),
        }
    }

    pub fn find_by_class(&self, class: &str) -> Option<&GateSpec> {
        self.by_class.get(class).map(|&i| &self.entries[i])
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
        self.has_wire_port(gate_class, crate::ir::PortSide::Input, port)
    }

    /// Is `port` a wire port on the `side` of `gate_class`, per the game's
    /// own dump? A composite sub-port (`Position.X`) is one when its parent
    /// (`Position`) is a composite port listing that sub-port: Vector2D
    /// layout ports expose each axis as an individually-wireable float.
    /// Unknown classes answer `true`, as in [`Catalog::is_wire_input`].
    pub fn has_wire_port(&self, gate_class: &str, side: crate::ir::PortSide, port: &str) -> bool {
        let Some(spec) = self.find_by_class(gate_class) else { return true };
        let ports = match side {
            crate::ir::PortSide::Input => &spec.component.inputs,
            crate::ir::PortSide::Output => &spec.component.outputs,
        };
        ports.iter().any(|p| p.name == port)
            || port.split_once('.').is_some_and(|(base, sub)| {
                ports.iter().any(|p| {
                    p.name == base && p.composite.as_ref().is_some_and(|c| c.sub_ports.iter().any(|s| s == sub))
                })
            })
    }
}

/// The half-size `node`'s brick is placed at, or `None` for a class the
/// catalog does not know. For a procedural gate this is the variant
/// `lower::gate_variants` recorded on the node, so layout (which reserves the
/// cell) and emit (which sizes and centers the brick) can never disagree.
pub fn node_half_size(node: &crate::ir::Node) -> Option<HalfSize> {
    let spec = default_catalog().find_by_class(node.gate_class)?;
    Some(spec.placed_half_size(node_variant(node)))
}

/// The procedural-gate variant recorded on `node`, if any.
pub fn node_variant(node: &crate::ir::Node) -> Option<usize> {
    match node.properties.get(&*crate::intern::sym::GATE_VARIANT) {
        Some(crate::ir::Literal::Int(i)) => usize::try_from(*i).ok(),
        _ => None,
    }
}

/// A component type the component gates (Add/Get Component, Get/Set Component
/// Property) can be set to.
#[derive(Clone, Debug)]
pub struct ComponentTypeOption {
    /// The name a `$Component/<name>` reference spells: the display name
    /// without its spaces (`PointLight`, `OneShotAudioPlayer`).
    pub name: String,
    /// The component class, as the game names it (`Component_PointLight`).
    pub class: &'static str,
    pub display_name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
}

/// The component types the component gates' Component Type setting offers,
/// sorted by display name: the property map's components (taken from the
/// gates' own picker), with each one's category and description from the
/// gate inventory.
pub fn component_types() -> &'static [ComponentTypeOption] {
    use std::sync::OnceLock;
    static LIST: OnceLock<Vec<ComponentTypeOption>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut out: Vec<ComponentTypeOption> = properties::property_map()
            .components
            .iter()
            .map(|c| {
                let inv = default_catalog().find_by_class(&c.option).map(|g| &g.component);
                ComponentTypeOption {
                    name: c.label.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect(),
                    class: &c.option,
                    display_name: &c.label,
                    category: inv.map_or("", |c| c.category.as_str()),
                    description: inv.map_or("", |c| c.description.as_str()),
                }
            })
            .collect();
        out.sort_by(|a, b| a.display_name.cmp(b.display_name));
        out
    })
}

/// The name a program spells component `class` by (`PointLight` for
/// `Component_PointLight`); the class itself for one the gates don't offer.
pub fn component_name(class: &str) -> String {
    component_types()
        .iter()
        .find(|o| o.class == class)
        .map_or_else(|| class.to_string(), |o| o.name.clone())
}

/// The component type `name` (a `$Component/<name>` spelling) names, if the
/// component gates accept it.
pub fn component_type(name: &str) -> Option<&'static ComponentTypeOption> {
    component_types().iter().find(|o| o.name == name)
}

/// A property of a component, for the Get/Set Component Property gates'
/// Property Name setting.
#[derive(Clone, Debug)]
pub struct ComponentProperty {
    pub name: &'static str,
    /// The property's type as the property map gives it (`float`, `color`,
    /// an enum's name, ...).
    pub ty: String,
}

/// The properties of component `class`, from the property map. The map reads
/// them from the save schema, so the list is a superset of the in-game picker:
/// it can hold internal fields and flattened per-team flags.
pub fn component_properties(class: &str) -> Vec<ComponentProperty> {
    let Some(c) = properties::component(class) else {
        return Vec::new();
    };
    let mut out: Vec<ComponentProperty> = c
        .properties
        .iter()
        .map(|p| ComponentProperty {
            name: &p.name,
            ty: p.enum_type.clone().unwrap_or_else(|| p.ty.clone()),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(b.name));
    out
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
/// a schema enum (e.g. `MathEasing.Function` -> `EBREasingFunction`). Returns
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
        let json = include_str!("../data/logic_gate_inventory.simple.json");
        Catalog::from_json(json).expect("default inventory JSON parses")
    })
}
