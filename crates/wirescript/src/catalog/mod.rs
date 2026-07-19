//! Gate catalog — the authoritative registry of every wire-graph brick
//! the compiler knows about. The inventory JSON is baked into the binary
//! via `include_str!`.

pub mod arrays;
pub mod calls;
pub mod events;
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
    pub kind: ComponentKind,
    #[serde(default)]
    pub family: String,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
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
}
