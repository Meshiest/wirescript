//! Built-in event registry. Maps PascalCase surface names (used in
//! `on X { ... }`) to gate classes + bound data outputs.

use crate::collections::HashMap;
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
    /// Named args (`on E(name = value)`, matched case-insensitively) whose value
    /// is WIRED into a gate INPUT port (surface name → gate input port name).
    /// e.g. `zone = zoneBrick` on the zone events. Empty for most events.
    pub input_named: Vec<(&'static str, &'static str)>,
    /// The gate's exec OUTPUT port name — the port the handler body chains from.
    /// Most events use `ExecOut`; the internal zone-event gates name it `Exec`.
    pub exec_out: &'static str,
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
                input_named: vec![],
                exec_out: "ExecOut",
            },
        )
    };
    // Like `mk`, but the event also exposes a `zone = <value>` named arg that
    // wires its value into the gate's `Zone` input port. The internal zone-event
    // gates name their exec output `Exec` (not `ExecOut`).
    let mk_zone = |surface: &'static str, class: &'static str, data: Vec<EventDataBinding>| {
        (
            surface,
            EventSpec {
                surface_name: surface,
                gate_class: class,
                data,
                config_positional: vec![],
                config_named: vec![],
                input_named: vec![("zone", "Zone")],
                exec_out: "Exec",
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
                input_named: vec![],
                exec_out: "ExecOut",
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
    // The player-join/left gates expose a `UserId` output (gate type `any`);
    // surface it as `string` so it compares, interpolates, and keys leaderboards
    // like `GetUserId()`. Useful for disconnect cleanup, where the `controller`
    // reference may already be torn down but the id is still stable.
    let string = |name, port| EventDataBinding {
        name,
        port,
        ty: Type::String,
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
            vec![
                controller("controller", "Controller"),
                string("userId", "UserId"),
            ],
        ),
        mk(
            "ControllerLeft",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerLeftEvent",
            vec![
                controller("controller", "Controller"),
                string("userId", "UserId"),
            ],
        ),
        mk_zone(
            "ZoneEntered",
            "BrickComponentType_Internal_CharacterZoneEvent_Entered",
            vec![character("character", "Character")],
        ),
        mk_zone(
            "ZoneLeft",
            "BrickComponentType_Internal_CharacterZoneEvent_Left",
            vec![character("character", "Character")],
        ),
        mk_zone(
            "BrickChanged",
            "BrickComponentType_Internal_ZoneEvent_BrickChanged",
            vec![brick("brick", "Brick")],
        ),
        mk_zone(
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
                // The attacker is a player character. The weapon stays
                // `entity`. it's an item, matched by entity-typed asset refs.
                character("attacker", "Attacker"),
                entity("attackerWeapon", "AttackerWeapon"),
                EventDataBinding {
                    name: "attackerWeaponName",
                    port: "AttackerWeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk_zone(
            "EntityZoneEntered",
            "BrickComponentType_Internal_EntityZoneEvent_Entered",
            vec![entity("entity", "Entity")],
        ),
        mk_zone(
            "EntityZoneLeft",
            "BrickComponentType_Internal_EntityZoneEvent_Left",
            vec![entity("entity", "Entity")],
        ),
        mk_zone(
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
        mk_zone(
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
                EventDataBinding {
                    name: "arguments",
                    port: "Arguments",
                    ty: Type::String,
                },
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
