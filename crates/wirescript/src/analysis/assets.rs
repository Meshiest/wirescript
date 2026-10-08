//! External asset catalog access, sourced from the brdb-embedded asset table
//! (`brdb::assets::external`). Used for `$AssetType/AssetName` completion and
//! validation.

/// The `$` reference type for a component type (`$Component/PointLight`), the
/// setting the component gates take, and for one of its properties
/// (`$Component/PointLight/Brightness`). Its names come from the gate catalog
/// (`catalog::component_types`), not the external asset table.
pub const COMPONENT: &str = "Component";

/// All known external asset type names (`BRItemBase`, `BrickAudioDescriptor`, …),
/// and [`COMPONENT`].
pub fn asset_types() -> Vec<&'static str> {
    brdb::assets::external::ASSET_TYPES
        .iter()
        .map(|(ty, _)| *ty)
        .chain(std::iter::once(COMPONENT))
        .collect()
}

/// Asset names of a given type, or empty if the type is unknown. For
/// [`COMPONENT`] the "type" may also be `Component/<name>`, whose names are
/// that component's properties.
pub fn asset_names(asset_type: &str) -> Vec<&'static str> {
    if asset_type == COMPONENT {
        return crate::catalog::component_types().iter().map(|o| o.name.as_str()).collect();
    }
    if let Some(name) = asset_type.strip_prefix(COMPONENT).and_then(|r| r.strip_prefix('/')) {
        return match crate::catalog::component_type(name) {
            Some(o) => crate::catalog::component_properties(o.class).into_iter().map(|p| p.name).collect(),
            None => Vec::new(),
        };
    }
    brdb::assets::external::ASSET_TYPES
        .iter()
        .find(|(ty, _)| *ty == asset_type)
        .map(|(_, names)| names.iter().map(|n| n.as_ref()).collect())
        .unwrap_or_default()
}

pub fn asset_type_exists(asset_type: &str) -> bool {
    asset_type == COMPONENT
        || brdb::assets::external::ASSET_TYPES
            .iter()
            .any(|(ty, _)| *ty == asset_type)
}

pub fn asset_exists(asset_type: &str, asset_name: &str) -> bool {
    match asset_name.split_once('/') {
        Some((component, property)) if asset_type == COMPONENT => {
            asset_names(&format!("{COMPONENT}/{component}")).iter().any(|n| *n == property)
        }
        _ => asset_names(asset_type).iter().any(|n| *n == asset_name),
    }
}

/// The external asset type an asset-ref config param expects, keyed by the gate
/// PORT the param wires into — so `font = <here>` / `weapon = <here>` can
/// complete the correct `$Type/Name` refs without the author knowing the type
/// name. Only ports whose asset type is present in the catalog are mapped.
pub fn asset_type_for_port(port: &str) -> Option<&'static str> {
    Some(match port {
        "Font" => "BrickFontDescriptor",
        "ItemType" | "ItemTypeIfItem" | "Item" => "BRItemBase",
        "ProjectileOverride" => "BRWeaponProjectile",
        "AudioDescriptor" => "BrickOneShotAudioDescriptor",
        "ComponentType" => COMPONENT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;

/// Completions for an `Environment.<env>.<property>` path being typed at
/// `(line, col)`: the environment names after `Environment.`, and that
/// environment's properties (with their types) after `Environment.<env>.`.
/// `None` when the cursor is not in such a path. Each item is
/// `(label, detail)`.
pub fn environment_completions(source: &str, line: usize, col: usize) -> Option<Vec<(String, String)>> {
    use crate::catalog::properties::{self, PropertyType};
    use crate::typecheck::props::{ENVIRONMENT, environment_by_label, environment_names};
    let l = source.lines().nth(line)?;
    let col_idx = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let before = &l[..col_idx];
    let start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let segments: Vec<&str> = before[start..].split('.').collect();
    match segments.as_slice() {
        [ENVIRONMENT, _typed] => Some(
            environment_names()
                .into_iter()
                .map(|name| {
                    let detail = environment_by_label(&name).map_or(String::new(), |e| format!("environment `{}`", e.option));
                    (name, detail)
                })
                .collect(),
        ),
        [ENVIRONMENT, env, _typed] => {
            let e = environment_by_label(env)?;
            Some(
                e.properties
                    .iter()
                    .map(|p| {
                        let detail = match properties::property_type(p) {
                            PropertyType::Value(t) => crate::analysis::types::type_str(&t),
                            PropertyType::Enum(en) => format!("int ({en})"),
                            PropertyType::Unsupported(why) => format!("unsupported: {why}"),
                        };
                        (p.name.clone(), detail)
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Member completions on a component reference of type `ty`
/// (`Component<PointLight>`): its properties, with their types.
/// `None` for any other type. Each item is `(label, detail)`.
pub fn component_member_completions(ty: &str) -> Option<Vec<(String, String)>> {
    use crate::catalog::properties::{self, PropertyType};
    let name = ty.strip_prefix("Component<")?.strip_suffix('>')?;
    let class = crate::catalog::component_type(name)?.class;
    let c = properties::component(class)?;
    let out: Vec<(String, String)> = c
        .properties
        .iter()
        .map(|p| {
            let detail = match properties::property_type(p) {
                PropertyType::Value(t) => crate::analysis::types::type_str(&t),
                PropertyType::Enum(en) => format!("int ({en})"),
                PropertyType::Unsupported(why) => format!("unsupported: {why}"),
            };
            (p.name.clone(), detail)
        })
        .collect();
    Some(out)
}
