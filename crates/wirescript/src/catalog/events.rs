//! Built-in event registry. Maps PascalCase surface names (used in
//! `on X { ... }`) to gate classes + bound data outputs.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::ir::Type;

#[derive(Clone, Debug)]
pub struct EventDataBinding {
    /// Source-level name (the binding identifier in `on X(name)`).
    pub name: &'static str,
    /// Actual gate output port name — usually PascalCase.
    pub port: &'static str,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct EventSpec {
    pub surface_name: &'static str,
    pub gate_class: &'static str,
    pub data: Vec<EventDataBinding>,
    /// Positional config args (`on E("a", "b")`) → gate data-struct field
    /// names, in order. Empty for events that take no config.
    pub config_positional: Vec<&'static str>,
    /// Named config args (`on E(Name = v)`, matched case-insensitively) → gate
    /// data-struct field name.
    pub config_named: Vec<(&'static str, &'static str)>,
}

/// Every built-in event. Order matches the TS reference for deterministic
/// iteration.
fn build_events() -> HashMap<&'static str, EventSpec> {
    let mk = |surface: &'static str, class: &'static str, data: Vec<EventDataBinding>| {
        (
            surface,
            EventSpec {
                surface_name: surface,
                gate_class: class,
                data,
                config_positional: vec![],
                config_named: vec![],
            },
        )
    };
    // Like `mk`, but for events that also accept config args (e.g. ChatCommand).
    let mk_cfg = |surface: &'static str,
                  class: &'static str,
                  data: Vec<EventDataBinding>,
                  config_positional: Vec<&'static str>,
                  config_named: Vec<(&'static str, &'static str)>| {
        (
            surface,
            EventSpec {
                surface_name: surface,
                gate_class: class,
                data,
                config_positional,
                config_named,
            },
        )
    };
    let character = |name, port| EventDataBinding {
        name,
        port,
        ty: Type::Character,
    };
    let controller = |name, port| EventDataBinding {
        name,
        port,
        ty: Type::Controller,
    };
    let brick = |name, port| EventDataBinding {
        name,
        port,
        ty: Type::Brick,
    };
    let entity = |name, port| EventDataBinding {
        name,
        port,
        ty: Type::Entity,
    };

    let entries = vec![
        mk(
            "RoundStart",
            "BrickComponentType_WireGraph_Fake_Gamemode_RoundStartEvent",
            vec![],
        ),
        mk(
            "RoundEnd",
            "BrickComponentType_WireGraph_Fake_Gamemode_RoundEndEvent",
            vec![],
        ),
        mk(
            "CharacterSpawned",
            "BrickComponentType_WireGraph_Fake_Gamemode_CharacterSpawnedEvent",
            vec![character("character", "Character")],
        ),
        mk(
            "CharacterDied",
            "BrickComponentType_WireGraph_Fake_Gamemode_CharacterDiedEvent",
            vec![character("character", "Character")],
        ),
        mk(
            "ControllerJoined",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerJoinedEvent",
            vec![controller("controller", "Controller")],
        ),
        mk(
            "ControllerLeft",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerLeftEvent",
            vec![controller("controller", "Controller")],
        ),
        mk(
            "ZoneEntered",
            "BrickComponentType_Internal_CharacterZoneEvent_Entered",
            vec![character("character", "Character")],
        ),
        mk(
            "ZoneLeft",
            "BrickComponentType_Internal_CharacterZoneEvent_Left",
            vec![character("character", "Character")],
        ),
        mk(
            "BrickChanged",
            "BrickComponentType_Internal_ZoneEvent_BrickChanged",
            vec![brick("brick", "Brick")],
        ),
        mk(
            "BrickRemoved",
            "BrickComponentType_Internal_ZoneEvent_BrickRemoved",
            vec![brick("brick", "Brick")],
        ),
        mk(
            "CharacterDamaged",
            "BrickComponentType_WireGraph_Fake_Gamemode_CharacterDamagedEvent",
            vec![
                character("character", "Character"),
                EventDataBinding {
                    name: "damage",
                    port: "Damage",
                    ty: Type::Float,
                },
                entity("attacker", "Attacker"),
                entity("attackerWeapon", "AttackerWeapon"),
                EventDataBinding {
                    name: "attackerWeaponName",
                    port: "AttackerWeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk(
            "EntityZoneEntered",
            "BrickComponentType_Internal_EntityZoneEvent_Entered",
            vec![entity("entity", "Entity")],
        ),
        mk(
            "EntityZoneLeft",
            "BrickComponentType_Internal_EntityZoneEvent_Left",
            vec![entity("entity", "Entity")],
        ),
        mk(
            "ProjectileZoneEntered",
            "BrickComponentType_Internal_ProjectileZoneEvent_Entered",
            vec![
                character("character", "Character"),
                entity("projectile", "Projectile"),
                entity("weapon", "Weapon"),
                EventDataBinding {
                    name: "weaponName",
                    port: "WeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk(
            "ProjectileZoneLeft",
            "BrickComponentType_Internal_ProjectileZoneEvent_Left",
            vec![
                character("character", "Character"),
                entity("projectile", "Projectile"),
                entity("weapon", "Weapon"),
                EventDataBinding {
                    name: "weaponName",
                    port: "WeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk_cfg(
            "ChatCommand",
            "BrickComponentType_WireGraph_Exec_ChatCommand",
            vec![
                controller("controller", "Controller"),
                EventDataBinding { name: "arguments", port: "Arguments", ty: Type::String },
            ],
            // `on ChatCommand("greet", "Greets you", player, args)`
            vec!["CommandName", "HelpText"],
            // `on ChatCommand("greet", Description = "Greets you")`
            vec![("description", "HelpText"), ("helptext", "HelpText")],
        ),
    ];

    entries.into_iter().collect()
}

pub fn events() -> &'static HashMap<&'static str, EventSpec> {
    static INSTANCE: OnceLock<HashMap<&'static str, EventSpec>> = OnceLock::new();
    INSTANCE.get_or_init(build_events)
}

pub fn find_event(surface_name: &str) -> Option<&'static EventSpec> {
    events().get(surface_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_events_registered() {
        assert_eq!(events().len(), 16);
        assert!(find_event("RoundStart").is_some());
        assert!(find_event("CharacterSpawned").is_some());
        assert!(find_event("ChatCommand").is_some());
        assert!(find_event("CharacterDamaged").is_some());
        assert!(find_event("EntityZoneEntered").is_some());
        assert!(find_event("ProjectileZoneLeft").is_some());
        assert!(find_event("Nonexistent").is_none());
    }

    #[test]
    fn character_spawned_has_character_binding() {
        let e = find_event("CharacterSpawned").unwrap();
        assert_eq!(e.data.len(), 1);
        assert_eq!(e.data[0].name, "character");
        assert_eq!(e.data[0].port, "Character");
        assert!(matches!(e.data[0].ty, Type::Character));
    }
}
