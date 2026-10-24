//! The property map: every environment and component property the Get/Set
//! Environment Property and Get/Set Component Property gates can name, with its
//! type. Generated per game build by brickadia-zoo (`just property-map`) and
//! baked in like the gate inventory; never hand-edited.
//!
//! An environment's `option` and a component's `option` are the exact strings
//! the gates store (`Sky`, `Component_PointLight`), confirmed from the gates'
//! own pickers. Component property lists come from the save schema, so they are
//! a superset of the in-game picker (internal fields, flattened per-team
//! bools); the environment lists of Sky, GroundPlate, Water and Ambience match
//! the game's environment preset.

use serde::Deserialize;

use crate::ir::Type;

#[derive(Debug, Deserialize)]
pub struct PropertyMap {
    pub environments: Vec<Environment>,
    pub components: Vec<Component>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    /// What the gate's Environment setting stores (`Sky`, `GroundPlate`).
    pub option: String,
    /// What the game's picker shows (`Sky`, `Ground`).
    pub label: String,
    pub class: String,
    #[serde(default)]
    pub preset_group: Option<String>,
    pub properties: Vec<Property>,
}

#[derive(Debug, Deserialize)]
pub struct Component {
    /// What the gates' Component Type setting names (`Component_PointLight`).
    pub option: String,
    /// What the game's picker shows (`Point Light`).
    pub label: String,
    /// The component's data struct in the save schema.
    #[serde(rename = "struct")]
    pub data_struct: String,
    pub properties: Vec<Property>,
}

#[derive(Debug, Deserialize)]
pub struct Property {
    /// What the gate's Property Name setting stores.
    pub name: String,
    /// float | int | bool | string | color | vector | rotator | quat | enum |
    /// object | class | struct | array
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(rename = "enum", default)]
    pub enum_type: Option<String>,
    #[serde(rename = "struct", default)]
    pub struct_type: Option<String>,
    #[serde(default)]
    pub class: Option<String>,
}

/// The baked property map, parsed on first use.
pub fn property_map() -> &'static PropertyMap {
    use std::sync::OnceLock;
    static MAP: OnceLock<PropertyMap> = OnceLock::new();
    MAP.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/property_map.json"))
            .expect("property map JSON parses")
    })
}

/// How a property's value reaches a program.
#[derive(Clone, Debug, PartialEq)]
pub enum PropertyType {
    /// Read and written as this type.
    Value(Type),
    /// A schema enum, read and written as its member (the game enum
    /// `schema_enum` registers as).
    Enum(String),
    /// Not readable or writable from a program yet, and why.
    Unsupported(String),
}

/// The type a program reads and writes `p` as.
pub fn property_type(p: &Property) -> PropertyType {
    use PropertyType::*;
    match p.ty.as_str() {
        "float" => Value(Type::Float),
        "int" => Value(Type::Int),
        "bool" => Value(Type::Bool),
        "string" => Value(Type::String),
        "vector" => Value(Type::Vector),
        "rotator" => Value(Type::Rotator),
        "quat" => Value(Type::Quat),
        // Environment colors are LinearColor and component colors Color; the
        // gate converts a wire color to either.
        "color" => Value(Type::Color),
        "enum" => match &p.enum_type {
            Some(e) => Enum(e.clone()),
            None => Unsupported("an enum the map does not name".into()),
        },
        "struct" => match p.struct_type.as_deref() {
            Some("Rotator3f") => Value(Type::Rotator),
            Some("Vector3f") => Value(Type::Vector),
            Some(s) => Unsupported(format!("a `{s}` struct, which has no Wirescript type")),
            None => Unsupported("a struct the map does not name".into()),
        },
        "object" | "class" => Unsupported(format!(
            "an asset reference ({}), which a property gate cannot carry yet",
            p.class.as_deref().unwrap_or(&p.ty)
        )),
        "array" => Unsupported("an array, which a property gate cannot carry".into()),
        other => Unsupported(format!("an unknown property type `{other}`")),
    }
}

/// The environment whose stored option is `option`.
pub fn environment(option: &str) -> Option<&'static Environment> {
    property_map()
        .environments
        .iter()
        .find(|e| e.option == option)
}

/// The component whose stored option (its class) is `option`.
pub fn component(option: &str) -> Option<&'static Component> {
    property_map()
        .components
        .iter()
        .find(|c| c.option == option)
}

impl Environment {
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.name == name)
    }
}

impl Component {
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every property in the map is either typed or rejected with a reason,
    /// and each kind of type lands where the mapping says.
    #[test]
    fn every_property_is_typed_or_rejected() {
        let map = property_map();
        let mut typed = 0;
        let mut rejected = 0;
        for (owner, props) in map
            .environments
            .iter()
            .map(|e| (&e.option, &e.properties))
            .chain(map.components.iter().map(|c| (&c.option, &c.properties)))
        {
            for p in props {
                match property_type(p) {
                    PropertyType::Value(_) | PropertyType::Enum(_) => typed += 1,
                    PropertyType::Unsupported(why) => {
                        assert!(
                            !why.contains("unknown property type"),
                            "{owner}/{}: {why}",
                            p.name
                        );
                        rejected += 1;
                    }
                }
            }
        }
        assert!(
            typed > 0 && rejected > 0,
            "{typed} typed, {rejected} rejected"
        );
        let get = |env: &str, name: &str| {
            property_type(environment(env).unwrap().property(name).unwrap())
        };
        assert_eq!(get("Sky", "TimeOfDay"), PropertyType::Value(Type::Float));
        assert_eq!(get("Sky", "SkyColor"), PropertyType::Value(Type::Color));
        assert_eq!(
            get("Ambience", "ReverbEffect"),
            PropertyType::Enum("EBRReverbEffect".into())
        );
        assert!(matches!(
            get("Studio", "Scenarios"),
            PropertyType::Unsupported(_)
        ));
        let light = component("Component_PointLight").unwrap();
        assert_eq!(
            property_type(light.property("Brightness").unwrap()),
            PropertyType::Value(Type::Float)
        );
        assert_eq!(
            property_type(light.property("Color").unwrap()),
            PropertyType::Value(Type::Color)
        );
    }

    /// Every enum a property names is one the save schema defines, so its
    /// members resolve.
    #[test]
    fn every_property_enum_is_in_the_schema() {
        let map = property_map();
        for p in map
            .environments
            .iter()
            .flat_map(|e| &e.properties)
            .chain(map.components.iter().flat_map(|c| &c.properties))
        {
            if let PropertyType::Enum(e) = property_type(p) {
                assert!(
                    !crate::catalog::enum_member_names(&e).is_empty(),
                    "{}: enum {e} has no members",
                    p.name
                );
            }
        }
    }
}
