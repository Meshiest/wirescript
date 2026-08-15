//! Spawning, explosions, custom events, sweeps, and player lookup.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Prefab spawner -------------------------------------------------
    m.insert(
        "SpawnPrefab",
        CallSpec {
            name: "SpawnPrefab",
            gate_class: gc::PREFAB_SPAWNER,
            params: vec![
                // The prefab to spawn: a `$./file.brz` / `$/abs/file.brz`
                // reference (or nested `$``` … ``` `). Embedded into the bundle at
                // emit; the gate's `Prefab` bundle_path_ref property gets the
                // resulting path. Compile-time reference-only, hence `PrefabRef`.
                CallParam::opt("prefab", WirePort::Prefab, Type::PrefabRef),
                CallParam::opt("offset", WirePort::SpawnOffset, Type::Vector),
                CallParam::opt("rotation", WirePort::SpawnOffsetRotation, Type::Rotator),
                CallParam::opt("velocity", WirePort::SpawnVelocity, Type::Vector),
                CallParam::opt("lifetime", WirePort::Lifetime, Type::Float),
                CallParam::opt("limit", WirePort::Limit, Type::Int),
                // Wire an exec pulse here to destroy every prefab this gate has
                // spawned (exposes the gate's existing `DestroyAll` input port).
                CallParam::opt("destroyAll", WirePort::DestroyAll, Type::Exec),
            ],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Entity,
                ty: Type::Entity,
            }],
            receiver: None,
        },
    );
    // Spawn an explosion of a given projectile/explosion class at an offset.
    m.insert(
        "SpawnExplosion",
        CallSpec {
            name: "SpawnExplosion",
            gate_class: gc::EXEC_SPAWN_EXPLOSION,
            params: vec![
                // The explosion type — a class/projectile asset reference
                // (`$…` inlines as data; a wired value also works).
                CallParam::req("projectileType", WirePort::ProjectileType, Type::Entity),
                // Who caused it (wire-only input, not baked in the data struct).
                CallParam::opt("instigator", WirePort::Instigator, Type::Entity),
                CallParam::opt("offset", WirePort::SpawnOffset, Type::Vector),
                CallParam::opt("scale", WirePort::ScaleMultiplier, Type::Float),
                CallParam::opt("damage", WirePort::DamageMultiplier, Type::Float),
            ],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );

    // Send (Personal) Custom Event: pulses every same-owner `CustomEvent` gate
    // listening on the same `eventName`, passing up to 8 data values. `eventName`
    // is the constant channel name, baked into the gate's data. `target` is an
    // optional entity whose grid receives the matching object events. Delivery is
    // same-owner only — the ownership-agnostic counterpart is SendGlobalCustomEvent.
    m.insert(
        "SendCustomEvent",
        CallSpec {
            name: "SendCustomEvent",
            gate_class: gc::PSEUDO_SEND_CUSTOM_EVENT,
            params: vec![
                CallParam::req("eventName", WirePort::EventName, Type::String),
                CallParam::opt("data1", WirePort::DataIn1, Type::Any),
                CallParam::opt("data2", WirePort::DataIn2, Type::Any),
                CallParam::opt("data3", WirePort::DataIn3, Type::Any),
                CallParam::opt("data4", WirePort::DataIn4, Type::Any),
                CallParam::opt("data5", WirePort::DataIn5, Type::Any),
                CallParam::opt("data6", WirePort::DataIn6, Type::Any),
                CallParam::opt("data7", WirePort::DataIn7, Type::Any),
                CallParam::opt("data8", WirePort::DataIn8, Type::Any),
                CallParam::opt("target", WirePort::Target, Type::Entity),
            ],
            exec: true,
            outputs: vec![],
            // Receiver form: `entity.SendCustomEvent("x", …)` auto-binds the
            // entity to `target` (see `receiver_target_param`), so the object
            // event reaches that entity's grid.
            receiver: Some(Type::Entity),
        },
    );

    // Send Global Custom Event: the ownership-agnostic counterpart of
    // SendCustomEvent — delivers to every matching `GlobalCustomEvent` receiver
    // regardless of owner. Same shape (constant `eventName`, up to 8 data values,
    // optional `target` entity for object-scoped delivery).
    m.insert(
        "SendGlobalCustomEvent",
        CallSpec {
            name: "SendGlobalCustomEvent",
            gate_class: gc::PSEUDO_SEND_CUSTOM_EVENT_GLOBAL,
            params: vec![
                CallParam::req("eventName", WirePort::EventName, Type::String),
                CallParam::opt("data1", WirePort::DataIn1, Type::Any),
                CallParam::opt("data2", WirePort::DataIn2, Type::Any),
                CallParam::opt("data3", WirePort::DataIn3, Type::Any),
                CallParam::opt("data4", WirePort::DataIn4, Type::Any),
                CallParam::opt("data5", WirePort::DataIn5, Type::Any),
                CallParam::opt("data6", WirePort::DataIn6, Type::Any),
                CallParam::opt("data7", WirePort::DataIn7, Type::Any),
                CallParam::opt("data8", WirePort::DataIn8, Type::Any),
                CallParam::opt("target", WirePort::Target, Type::Entity),
            ],
            exec: true,
            outputs: vec![],
            // Receiver form: `entity.SendCustomEvent("x", …)` auto-binds the
            // entity to `target` (see `receiver_target_param`), so the object
            // event reaches that entity's grid.
            receiver: Some(Type::Entity),
        },
    );

    // Spawn Explosion At: like SpawnExplosion but at an absolute WORLD position
    // instead of an offset from the gate's brick.
    m.insert(
        "SpawnExplosionAt",
        CallSpec {
            name: "SpawnExplosionAt",
            gate_class: gc::EXEC_SPAWN_EXPLOSION_AT,
            params: vec![
                CallParam::req("worldPosition", WirePort::WorldPosition, Type::Vector),
                // The explosion type — a class/projectile asset reference
                // (`$…` inlines as data; a wired value also works).
                CallParam::req("projectileType", WirePort::ProjectileType, Type::Entity),
                // Who caused it (wire-only input, not baked in the data struct).
                CallParam::opt("instigator", WirePort::Instigator, Type::Entity),
                CallParam::opt("scale", WirePort::ScaleMultiplier, Type::Float),
                CallParam::opt("damage", WirePort::DamageMultiplier, Type::Float),
            ],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );

    // ---- Sweep (raycasting) ---------------------------------------------
    m.insert(
        "Sweep",
        CallSpec {
            name: "Sweep",
            gate_class: gc::SWEEP,
            params: vec![
                CallParam::req("origin", WirePort::Origin, Type::Vector),
                CallParam::req("direction", WirePort::Direction, Type::Vector),
                CallParam::req("distance", WirePort::Distance, Type::Float),
                CallParam::opt("radius", WirePort::Radius, Type::Float),
                CallParam::opt("relative", WirePort::BRelative, Type::Bool),
                CallParam::opt("ignore", WirePort::IgnoreEntity, Type::Entity),
                // An array var of additional entities to ignore (wired as its
                // ArrayVarRef), on top of the single `ignore` entity.
                CallParam::opt(
                    "ignoreList",
                    WirePort::AdditionalIgnoredEntities,
                    Type::Array(Box::new(Type::Entity)),
                ),
                // what the sweep detects (each defaults off in-engine).
                CallParam::opt("detectBricks", WirePort::BDetectBricks, Type::Bool),
                CallParam::opt("detectPlayers1", WirePort::BDetectPlayers1, Type::Bool),
                CallParam::opt("detectPlayers2", WirePort::BDetectPlayers2, Type::Bool),
                CallParam::opt("detectPlayers3", WirePort::BDetectPlayers3, Type::Bool),
                CallParam::opt("detectPlayers4", WirePort::BDetectPlayers4, Type::Bool),
                CallParam::opt("detectPhysics", WirePort::BDetectPhysics, Type::Bool),
                CallParam::opt("detectMap", WirePort::BDetectMap, Type::Bool),
                CallParam::opt("ignoreOwningGrid", WirePort::BIgnoreOwningGrid, Type::Bool),
                // Wire input selecting the sweep's collision channel
                // (EBRSweepCollisionChannel: 0 Physics, 1 Weapon, 2 Interaction,
                // 3 Tool, 4-7 Player1-4, 8 NoAdditionalRestriction — int at the wire).
                CallParam::opt("collisionChannel", WirePort::CollisionChannel, Type::Int),
                // Config-only (settings menu, not a wire input on Sweep).
                CallParam::opt("bodyPartsOnly", WirePort::BOnlyHitPlayerBodyParts, Type::Bool),
            ],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::HitDistance,
                ty: Type::Record(vec![
                    ("HitDistance".into(), Type::Float),
                    ("HitEntity".into(), Type::Entity),
                    ("HitLocation".into(), Type::Vector),
                    ("HitNormal".into(), Type::Vector),
                    ("HitColor".into(), Type::Color),
                    ("Hit".into(), Type::Exec),
                    ("Miss".into(), Type::Exec),
                ]),
            }],
            receiver: None,
        },
    );

    // ---- Player lookup (exec gate) ------------------------------------------
    // Has Exec/ExecOut ports and emits the found player on its `Player` output —
    // the game's persistent PlayerState, modeled here as `controller` (same type
    // the join/left events and every PlayerState gate use), NOT a character. Get
    // the body from it with `CharacterOf(FindPlayer(...))`.
    m.insert(
        "FindPlayer",
        CallSpec {
            name: "FindPlayer",
            gate_class: gc::FIND_PLAYER,
            params: vec![CallParam::req("query", WirePort::Query, Type::Any)],
            exec: true,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::Player,
                ty: Type::Controller,
            }],
            receiver: None,
        },
    );
}
