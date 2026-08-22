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
    /// is WIRED into a gate INPUT port (surface name → gate input port name →
    /// the value's expected type). e.g. `zone = zoneBrick` on the zone events
    /// wires a `zone` reference into the `Zone` port. Empty for most events.
    pub input_named: Vec<(&'static str, &'static str, Type)>,
    /// The gate's exec OUTPUT port name — the port the handler body chains from.
    /// Most events use `ExecOut`; the internal zone-event gates name it `Exec`.
    pub exec_out: &'static str,
}

/// How a NAMED handler arg (`on E(name = value)`) classifies against an event's
/// declared slots. The single source of truth for "is this arg name meaningful",
/// so typecheck (diagnostics), lowering (wire vs bake), and hover agree instead
/// of each re-deriving the answer and drifting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventArgKind {
    /// Wires the value into a gate INPUT port (may be dynamic).
    /// `(surface name, gate port name)`.
    InputWire(&'static str, &'static str),
    /// Bakes the value into a constant-only settings field.
    /// `(surface name, gate data-struct field)`.
    ConfigField(&'static str, &'static str),
    /// Matches no declared slot — a typo. Nothing lowers it, so it silently
    /// no-ops; typecheck flags it (WS041).
    Unknown,
}

impl EventSpec {
    /// Classify a NAMED handler arg (case-insensitive) as an input wire, a
    /// constant config field, or unknown.
    pub fn classify_arg(&self, name: &str) -> EventArgKind {
        if let Some((surf, port, _)) = self
            .input_named
            .iter()
            .find(|(surf, _, _)| surf.eq_ignore_ascii_case(name))
        {
            return EventArgKind::InputWire(surf, port);
        }
        if let Some((surf, field)) = self
            .config_named
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            return EventArgKind::ConfigField(surf, field);
        }
        EventArgKind::Unknown
    }
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
                input_named: vec![("zone", "Zone", Type::Zone)],
                exec_out: "Exec",
            },
        )
    };
    // Like `mk_zone`, but the event also exposes a `tagFilter = <value>` named
    // arg wiring into the gate's `TagFilter` input, restricting the event to
    // tagged entities. Only the character/entity zone enter/leave events have it.
    let mk_zone_tag = |surface: &'static str, class: &'static str, data: Vec<EventDataBinding>| {
        (
            surface,
            EventSpec {
                surface_name: surface,
                gate_class: class,
                data,
                config_positional: vec![],
                config_named: vec![],
                input_named: vec![
                    ("zone", "Zone", Type::Zone),
                    ("tagFilter", "TagFilter", Type::String),
                ],
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
            vec![EventDataBinding {
                name: "roundNumber",
                port: "RoundNumber",
                ty: Type::Int,
            }],
        ),
        mk(
            "RoundEnd",
            "BrickComponentType_WireGraph_Fake_Gamemode_RoundEndEvent",
            vec![EventDataBinding {
                name: "roundNumber",
                port: "RoundNumber",
                ty: Type::Int,
            }],
        ),
        mk(
            "CharacterSpawned",
            "BrickComponentType_WireGraph_Fake_Gamemode_CharacterSpawnedEvent",
            vec![character("character", "Character")],
        ),
        mk(
            "CharacterDied",
            "BrickComponentType_WireGraph_Fake_Gamemode_CharacterDiedEvent",
            vec![
                character("character", "Character"),
                // The killer is a player character; the weapon is an item entity.
                character("killer", "Killer"),
                entity("killerWeapon", "KillerWeapon"),
                EventDataBinding {
                    name: "killerWeaponName",
                    port: "KillerWeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk(
            "ControllerJoined",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerJoinedEvent",
            vec![
                controller("controller", "PlayerState"),
                string("userId", "UserId"),
                string("userName", "UserName"),
            ],
        ),
        mk(
            "ControllerLeft",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerLeftEvent",
            vec![
                controller("controller", "PlayerState"),
                string("userId", "UserId"),
                string("userName", "UserName"),
            ],
        ),
        // Team join/leave: like ControllerJoined/Left, but fired when a player
        // joins/leaves a team, and additionally exposing the `team` entity. The
        // PlayerState is entity-typed here, unlike the plain join/leave events
        // above.
        mk(
            "ControllerJoinedTeam",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerJoinedTeamEvent",
            vec![
                entity("controller", "PlayerState"),
                entity("team", "Team"),
                string("userId", "UserId"),
                string("userName", "UserName"),
            ],
        ),
        mk(
            "ControllerLeftTeam",
            "BrickComponentType_WireGraph_Fake_Gamemode_ControllerLeftTeamEvent",
            vec![
                entity("controller", "PlayerState"),
                entity("team", "Team"),
                string("userId", "UserId"),
                string("userName", "UserName"),
            ],
        ),
        mk_zone_tag(
            "ZoneEntered",
            "BrickComponentType_Internal_CharacterZoneEvent_Entered",
            vec![character("character", "Character")],
        ),
        mk_zone_tag(
            "ZoneLeft",
            "BrickComponentType_Internal_CharacterZoneEvent_Left",
            vec![character("character", "Character")],
        ),
        // The brick zone events carry no data payload — the gate exposes only
        // its exec pulse (no `Brick` output), so there's no `brick` binding.
        mk_zone(
            "BrickChanged",
            "BrickComponentType_Internal_ZoneEvent_BrickChanged",
            vec![],
        ),
        mk_zone(
            "BrickRemoved",
            "BrickComponentType_Internal_ZoneEvent_BrickRemoved",
            vec![],
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
                // The attacker is a player character; the weapon is an item
                // entity, matched by entity-typed asset refs.
                character("attacker", "Attacker"),
                entity("attackerWeapon", "AttackerWeapon"),
                EventDataBinding {
                    name: "attackerWeaponName",
                    port: "AttackerWeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk(
            "CharacterFiredWeapon",
            "BrickComponentType_WireGraph_Fake_Gamemode_CharacterFiredWeaponEvent",
            vec![
                character("character", "Character"),
                EventDataBinding {
                    name: "direction",
                    port: "Direction",
                    ty: Type::Vector,
                },
                EventDataBinding {
                    name: "start",
                    port: "Start",
                    ty: Type::Vector,
                },
                entity("weapon", "Weapon"),
                EventDataBinding {
                    name: "weaponName",
                    port: "WeaponName",
                    ty: Type::String,
                },
            ],
        ),
        mk_zone_tag(
            "EntityZoneEntered",
            "BrickComponentType_Internal_EntityZoneEvent_Entered",
            vec![entity("entity", "Entity")],
        ),
        mk_zone_tag(
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
                controller("controller", "PlayerState"),
                EventDataBinding {
                    name: "arguments",
                    port: "Arguments",
                    ty: Type::String,
                },
            ],
            // `on ChatCommand("greet", "Greets you") -> (player, args)`
            vec!["CommandName", "HelpText"],
            // `on ChatCommand("greet", Description = "Greets you")`
            vec![("description", "HelpText"), ("helptext", "HelpText")],
        ),
        // Whole-grid interaction/targeting events. These `WireGraph_Exec_*` gates
        // have NO `ExecOut`; their trigger is a differently-named output that acts
        // as the exec (like Clock's `Pulse`): `WholeGridInteracted` fires from
        // `Character` (the interacting character) — which is ALSO bound as the
        // `character` data param — exposing `held` (`bHeld`, true while held);
        // `WholeGridTargeted` fires from `Targeted` (typed `any` in the dump,
        // semantically the exec) exposing the hit character/damage/weapon. The
        // exec-only port (`WholeGridTargeted.Targeted`) is in EVENT_ALLOWED_GAPS.
        (
            "WholeGridInteracted",
            EventSpec {
                surface_name: "WholeGridInteracted",
                gate_class: "BrickComponentType_WireGraph_Exec_WholeGridInteracted",
                data: vec![
                    character("character", "Character"),
                    EventDataBinding {
                        name: "held",
                        port: "bHeld",
                        ty: Type::Bool,
                    },
                ],
                config_positional: vec![],
                config_named: vec![],
                input_named: vec![],
                exec_out: "Character",
            },
        ),
        (
            "WholeGridTargeted",
            EventSpec {
                surface_name: "WholeGridTargeted",
                gate_class: "BrickComponentType_WireGraph_Exec_WholeGridTargeted",
                data: vec![
                    character("character", "CharacterThatJustHit"),
                    EventDataBinding {
                        name: "damage",
                        port: "Damage",
                        ty: Type::Float,
                    },
                    entity("weapon", "WeaponThatJustHit"),
                    EventDataBinding {
                        name: "weaponName",
                        port: "WeaponNameThatJustHit",
                        ty: Type::String,
                    },
                ],
                config_positional: vec![],
                config_named: vec![],
                input_named: vec![],
                exec_out: "Targeted",
            },
        ),
        // The Clock event auto-emits an exec pulse at a configured interval; the
        // handler body chains from its `Pulse` output. `interval` and `enabled`
        // wire into the gate's `IntervalSeconds` / `bEnabled` inputs (so they may
        // be dynamic); pulseOn/onTime/offTime are settings-menu constant config.
        (
            "Clock",
            EventSpec {
                surface_name: "Clock",
                gate_class: "BrickComponentType_Clock",
                data: vec![],
                config_positional: vec![],
                config_named: vec![
                    ("pulseon", "bPulseOn"),
                    ("ontime", "OnTimeSeconds"),
                    ("offtime", "OffTimeSeconds"),
                ],
                input_named: vec![
                    ("interval", "IntervalSeconds", Type::Float),
                    ("enabled", "bEnabled", Type::Bool),
                ],
                exec_out: "Pulse",
            },
        ),
        // (Personal) Custom Event: pulses when a matching same-owner
        // `SendCustomEvent` fires, exposing the up-to-8 data values it carried.
        // The leading positional is the channel name (baked into `EventName` when
        // constant); the remaining params are the TYPED data outputs, whose
        // annotations type the gate's WireGraphVariant ports (the game can't store
        // them as `any`). Unused slots default to float. `isObject` is constant
        // config that scopes the event to a specific grid/object instead of firing
        // grid-wide. Delivery is same-owner only — the ownership-agnostic
        // counterpart is `on GlobalCustomEvent(...)`.
        // `on CustomEvent("dmg") -> (amount: int, source: character) { last = amount }`.
        (
            "CustomEvent",
            EventSpec {
                surface_name: "CustomEvent",
                gate_class: "BrickComponentType_WireGraphPseudo_CustomEvent",
                data: vec![
                    EventDataBinding { name: "data1", port: "DataOut1", ty: Type::Any },
                    EventDataBinding { name: "data2", port: "DataOut2", ty: Type::Any },
                    EventDataBinding { name: "data3", port: "DataOut3", ty: Type::Any },
                    EventDataBinding { name: "data4", port: "DataOut4", ty: Type::Any },
                    EventDataBinding { name: "data5", port: "DataOut5", ty: Type::Any },
                    EventDataBinding { name: "data6", port: "DataOut6", ty: Type::Any },
                    EventDataBinding { name: "data7", port: "DataOut7", ty: Type::Any },
                    EventDataBinding { name: "data8", port: "DataOut8", ty: Type::Any },
                ],
                config_positional: vec!["EventName"],
                config_named: vec![("isObject", "bIsObjectEvent")],
                input_named: vec![],
                exec_out: "ExecOut",
            },
        ),
        // Global Custom Event: the ownership-agnostic counterpart of Custom Event
        // — fires for a matching `SendGlobalCustomEvent` regardless of owner. Same
        // shape (channel-name positional, up to 8 typed data outputs, `isObject`
        // config).
        (
            "GlobalCustomEvent",
            EventSpec {
                surface_name: "GlobalCustomEvent",
                gate_class: "BrickComponentType_WireGraphPseudo_CustomEvent_Global",
                data: vec![
                    EventDataBinding { name: "data1", port: "DataOut1", ty: Type::Any },
                    EventDataBinding { name: "data2", port: "DataOut2", ty: Type::Any },
                    EventDataBinding { name: "data3", port: "DataOut3", ty: Type::Any },
                    EventDataBinding { name: "data4", port: "DataOut4", ty: Type::Any },
                    EventDataBinding { name: "data5", port: "DataOut5", ty: Type::Any },
                    EventDataBinding { name: "data6", port: "DataOut6", ty: Type::Any },
                    EventDataBinding { name: "data7", port: "DataOut7", ty: Type::Any },
                    EventDataBinding { name: "data8", port: "DataOut8", ty: Type::Any },
                ],
                config_positional: vec!["EventName"],
                config_named: vec![("isObject", "bIsObjectEvent")],
                input_named: vec![],
                exec_out: "ExecOut",
            },
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
mod tests;
