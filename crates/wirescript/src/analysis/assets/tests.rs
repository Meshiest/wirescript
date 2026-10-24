use super::*;

#[test]
fn asset_type_for_port_maps_known_asset_params() {
    assert_eq!(asset_type_for_port("Font"), Some("BrickFontDescriptor"));
    assert_eq!(asset_type_for_port("ItemTypeIfItem"), Some("BRItemBase"));
    assert_eq!(
        asset_type_for_port("ProjectileOverride"),
        Some("BRWeaponProjectile")
    );
    // Wire-input / non-asset ports have no mapping.
    assert_eq!(asset_type_for_port("IgnoreEntity"), None);
    assert_eq!(asset_type_for_port("Team"), None);
    // Every mapped type is actually present in the catalog.
    for port in ["Font", "ItemType", "ProjectileOverride", "AudioDescriptor"] {
        let ty = asset_type_for_port(port).unwrap();
        assert!(asset_type_exists(ty), "{ty} not in catalog");
        assert!(!asset_names(ty).is_empty(), "{ty} has no names");
    }
}

/// The component gates' Component Type setting, as the game's dropdown lists
/// it (every component a player can add, minus Input Splitter, which has no
/// category), spelled as its display name without spaces.
const DROPDOWN: [&str; 27] = [
    "AttachedZone",
    "Audio",
    "BotSpawn",
    "BrickPropertyChanger",
    "Button",
    "CheckPoint",
    "Damage",
    "DirectionalAerodynamicSurface",
    "ExplosionSpawner",
    "GoalPoint",
    "Gyroscope",
    "Interactable",
    "ItemSpawn",
    "OneShotAudioPlayer",
    "PointLight",
    "PrefabSpawn",
    "ProjectileSpawner",
    "Seat",
    "SpawnPoint",
    "SpotLight",
    "Switch",
    "Target",
    "TeleportDestination",
    "TextDecal",
    "Thruster",
    "Touch",
    "Weight",
];

#[test]
fn dollar_completion_offers_components() {
    assert!(asset_types().contains(&COMPONENT));
    assert!(asset_type_exists(COMPONENT));
    assert_eq!(asset_type_for_port("ComponentType"), Some(COMPONENT));
}

#[test]
fn component_names_are_exactly_the_dropdown() {
    let mut got = asset_names(COMPONENT);
    let mut want = DROPDOWN.to_vec();
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want);
    assert!(!asset_exists(COMPONENT, "InputSplitter"));
    assert!(asset_exists(COMPONENT, "PointLight"));
}

/// `$Component/PointLight/` completes the light's properties.
#[test]
fn a_component_path_completes_its_properties() {
    let props = asset_names("Component/PointLight");
    for p in ["Brightness", "Color", "Radius", "bEnabled"] {
        assert!(props.contains(&p), "{p} in {props:?}");
    }
    assert!(asset_exists(COMPONENT, "PointLight/Brightness"));
    assert!(!asset_exists(COMPONENT, "PointLight/Volume"));
    assert!(asset_names("Component/NotAThing").is_empty());
}

#[test]
fn environment_paths_complete_names_then_properties() {
    let labels = |src: &str| -> Vec<String> {
        let col = src.chars().count();
        environment_completions(src, 0, col)
            .unwrap_or_default()
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    };
    let envs = labels("let t = Environment.");
    assert_eq!(
        envs,
        vec!["Ambience", "Ground", "Sky", "Lighting", "Universe", "Water"]
    );
    let sky = environment_completions("let t = Environment.Sky.Ti", 0, 26).unwrap();
    assert!(
        sky.iter().any(|(l, d)| l == "TimeOfDay" && d == "float"),
        "{sky:?}"
    );
    assert!(
        sky.iter().any(|(l, d)| l == "SkyColor" && d == "color"),
        "{sky:?}"
    );
    assert!(environment_completions("let t = other.Sky.", 0, 18).is_none());
}

#[test]
fn a_component_reference_completes_its_properties() {
    let items = component_member_completions("Component<PointLight>").unwrap();
    assert!(
        items.iter().any(|(l, d)| l == "Brightness" && d == "float"),
        "{items:?}"
    );
    assert!(
        !items.iter().any(|(l, _)| l == "Remove"),
        "the gate removes by brick, not by reference"
    );
    assert!(component_member_completions("Component<Lamp>").is_none());
    assert!(component_member_completions("int").is_none());
}
