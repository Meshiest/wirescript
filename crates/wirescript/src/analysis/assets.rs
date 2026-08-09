//! External asset catalog access, sourced from the brdb-embedded asset table
//! (`brdb::assets::external`). Used for `$AssetType/AssetName` completion and
//! validation.

/// All known external asset type names (`BRItemBase`, `BrickAudioDescriptor`, …).
pub fn asset_types() -> Vec<&'static str> {
    brdb::assets::external::ASSET_TYPES
        .iter()
        .map(|(ty, _)| *ty)
        .collect()
}

/// Asset names of a given type, or empty if the type is unknown.
pub fn asset_names(asset_type: &str) -> Vec<&'static str> {
    brdb::assets::external::ASSET_TYPES
        .iter()
        .find(|(ty, _)| *ty == asset_type)
        .map(|(_, names)| names.iter().map(|n| n.as_ref()).collect())
        .unwrap_or_default()
}

pub fn asset_type_exists(asset_type: &str) -> bool {
    brdb::assets::external::ASSET_TYPES
        .iter()
        .any(|(ty, _)| *ty == asset_type)
}

pub fn asset_exists(asset_type: &str, asset_name: &str) -> bool {
    asset_names(asset_type).iter().any(|n| *n == asset_name)
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
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
