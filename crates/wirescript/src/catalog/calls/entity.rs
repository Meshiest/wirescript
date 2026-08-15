//! Entity calls: transform getters/setters, teleports, tags, and teams.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Entity getters -------------------------------------------------
    m.insert(
        "GetLocation",
        entity_exec(
            "GetLocation",
            gc::ENTITY_GET_LOCATION,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::Vector,
                ty: Type::Vector,
            }],
        ),
    );
    // Entity_IsFrozen: pure query — `entity.IsFrozen()` returns the frozen state
    // on ExecOut (non-physics and global grids read frozen). Empty data struct.
    m.insert(
        "IsFrozen",
        entity_exec(
            "IsFrozen",
            gc::ENTITY_IS_FROZEN,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
                field: None,
                port: WirePort::BFrozen,
                ty: Type::Bool,
            }],
        ),
    );
    m.insert(
        "GetRotation",
        entity_exec(
            "GetRotation",
            gc::ENTITY_GET_ROTATION,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::Rotation,
                ty: Type::Rotator,
            }],
        ),
    );
    m.insert(
        "GetLocationRotation",
        entity_exec(
            "GetLocationRotation",
            gc::ENTITY_GET_LOCATION_ROTATION,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::Vector,
                ty: Type::Record(vec![
                    ("Vector".into(), Type::Vector),
                    ("Rotation".into(), Type::Rotator),
                ]),
            }],
        ),
    );
    m.insert(
        "GetLinearVelocity",
        entity_exec(
            "GetLinearVelocity",
            gc::ENTITY_GET_LINEAR_VELOCITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::LinearVelocity,
                ty: Type::Vector,
            }],
        ),
    );
    m.insert(
        "GetAngularVelocity",
        entity_exec(
            "GetAngularVelocity",
            gc::ENTITY_GET_ANGULAR_VELOCITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::AngularVelocity,
                ty: Type::Vector,
            }],
        ),
    );
    m.insert(
        "GetVelocity",
        entity_exec(
            "GetVelocity",
            gc::ENTITY_GET_VELOCITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::Vector,
                ty: Type::Record(vec![
                    ("Vector".into(), Type::Vector),
                    ("Rotation".into(), Type::Rotator),
                ]),
            }],
        ),
    );

    // ---- Entity manipulation (exec) -------------------------------------
    m.insert(
        "SetLocation",
        CallSpec {
            name: "SetLocation",
            gate_class: gc::ENTITY_SET_LOCATION,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("pos", WirePort::Vector, Type::Vector),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "SetRotation",
        CallSpec {
            name: "SetRotation",
            gate_class: gc::ENTITY_SET_ROTATION,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("rot", WirePort::Rotation, Type::Rotator),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "SetLocationRotation",
        CallSpec {
            name: "SetLocationRotation",
            gate_class: gc::ENTITY_SET_LOCATION_ROTATION,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("pos", WirePort::Vector, Type::Vector),
                CallParam::req("rot", WirePort::Rotation, Type::Rotator),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "AddLocationRotation",
        CallSpec {
            name: "AddLocationRotation",
            gate_class: gc::ENTITY_ADD_LOCATION_ROTATION,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("pos", WirePort::Vector, Type::Vector),
                CallParam::req("rot", WirePort::Rotation, Type::Rotator),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "Teleport",
        CallSpec {
            name: "Teleport",
            gate_class: gc::ENTITY_TELEPORT,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                // A teleport point (Teleport Destination reference). Teleporting
                // to a raw position uses `SetLocation`, not this gate.
                CallParam::req("dest", WirePort::Destination, Type::Teleport),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "RelativeTeleport",
        CallSpec {
            name: "RelativeTeleport",
            gate_class: gc::ENTITY_RELATIVE_TELEPORT,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                // Source and destination are both teleport points.
                CallParam::req("source", WirePort::Source, Type::Teleport),
                CallParam::req("dest", WirePort::Destination, Type::Teleport),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "SetVelocity",
        CallSpec {
            name: "SetVelocity",
            gate_class: gc::ENTITY_SET_VELOCITY,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::opt("linear", WirePort::Vector, Type::Vector),
                CallParam::opt("angular", WirePort::Rotation, Type::Vector),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "AddVelocity",
        CallSpec {
            name: "AddVelocity",
            gate_class: gc::ENTITY_ADD_VELOCITY,
            params: vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::opt("linear", WirePort::Vector, Type::Vector),
                CallParam::opt("angular", WirePort::Rotation, Type::Vector),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "SetLinearVelocity",
        entity_exec(
            "SetLinearVelocity",
            gc::ENTITY_SET_LINEAR_VELOCITY,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("vel", WirePort::LinearVelocity, Type::Vector),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetAngularVelocity",
        entity_exec(
            "SetAngularVelocity",
            gc::ENTITY_SET_ANGULAR_VELOCITY,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("vel", WirePort::AngularVelocity, Type::Vector),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetGravityDirection",
        entity_exec(
            "SetGravityDirection",
            gc::ENTITY_SET_GRAVITY_DIRECTION,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("rot", WirePort::Rotation, Type::Rotator),
            ],
            vec![],
        ),
    );

    // ---- Entity (additional) ---------------------------------------------
    m.insert(
        "SetFrozen",
        entity_exec(
            "SetFrozen",
            gc::ENTITY_SET_FROZEN,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("frozen", WirePort::BFrozen, Type::Bool),
            ],
            vec![],
        ),
    );

    // ---- Entity tags --------------------------------------------
    m.insert(
        "GetTag",
        entity_exec(
            "GetTag",
            gc::ENTITY_GET_TAG,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::Tag,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "SetTag",
        entity_exec(
            "SetTag",
            gc::ENTITY_SET_TAG,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("tag", WirePort::Tag, Type::String),
            ],
            vec![],
        ),
    );

    // ---- Newer entity getters / setters ----------------------------------
    m.insert(
        "DestroySpawned",
        entity_exec(
            "DestroySpawned",
            gc::ENTITY_DESTROY_SPAWNED,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![],
        ),
    );
    m.insert(
        "DestroySpawnedPrefab",
        entity_exec(
            "DestroySpawnedPrefab",
            gc::ENTITY_DESTROY_SPAWNED_PREFAB,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![],
        ),
    );
    m.insert(
        "GetVelocityAtPoint",
        entity_exec(
            "GetVelocityAtPoint",
            gc::ENTITY_GET_VELOCITY_AT_POINT,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("point", WirePort::Point, Type::Vector),
            ],
            vec![CallOutput {
                field: None,
                port: WirePort::LinearVelocity,
                ty: Type::Vector,
            }],
        ),
    );
    m.insert(
        "GetSpeed",
        entity_exec(
            "GetSpeed",
            gc::ENTITY_GET_SPEED,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
                field: None,
                port: WirePort::Speed,
                ty: Type::Float,
            }],
        ),
    );
    // Entity-scoped team access. Named distinctly from the character/player
    // `GetTeam`/`SetTeam` builtins — these operate on any entity (grid, prefab).
    m.insert(
        "GetEntityTeam",
        entity_exec(
            "GetEntityTeam",
            gc::ENTITY_GET_TEAM,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
                field: None,
                port: WirePort::Team,
                ty: Type::Entity,
            }],
        ),
    );
    m.insert(
        "SetEntityTeam",
        entity_exec(
            "SetEntityTeam",
            gc::ENTITY_SET_TEAM,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("team", WirePort::Team, Type::Entity),
            ],
            vec![],
        ),
    );

    // ---- Self transform + simple sweep -----------------------------------
    m.insert(
        "GetOwnTransform",
        CallSpec {
            name: "GetOwnTransform",
            gate_class: gc::GET_OWN_TRANSFORM,
            params: vec![],
            exec: true,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::Location,
                ty: Type::Record(vec![
                    ("Location".into(), Type::Vector),
                    ("Rotation".into(), Type::Rotator),
                ]),
            }],
            receiver: None,
        },
    );
    m.insert(
        "SweepSimple",
        CallSpec {
            name: "SweepSimple",
            gate_class: gc::SWEEP_SIMPLE,
            params: vec![
                CallParam::req("distance", WirePort::Distance, Type::Float),
                CallParam::opt("radius", WirePort::Radius, Type::Float),
                CallParam::opt("spreadConeAngle", WirePort::SpreadConeAngle, Type::Float),
                // Config-only on SweepSimple (settings menu, not wire inputs).
                // `direction` is an EBrickDirection enum member.
                CallParam::opt("direction", WirePort::Direction, Type::Int),
                CallParam::opt("spreadTowardCenter", WirePort::BSpreadBiasedTowardsCenter, Type::Bool),
                CallParam::opt("detectBricks", WirePort::BDetectBricks, Type::Bool),
                CallParam::opt("detectPlayers1", WirePort::BDetectPlayers1, Type::Bool),
                CallParam::opt("detectPlayers2", WirePort::BDetectPlayers2, Type::Bool),
                CallParam::opt("detectPlayers3", WirePort::BDetectPlayers3, Type::Bool),
                CallParam::opt("detectPlayers4", WirePort::BDetectPlayers4, Type::Bool),
                CallParam::opt("bodyPartsOnly", WirePort::BOnlyHitPlayerBodyParts, Type::Bool),
                CallParam::opt("detectPhysics", WirePort::BDetectPhysics, Type::Bool),
                CallParam::opt("detectMap", WirePort::BDetectMap, Type::Bool),
                // Wire input: collision channel (EBRSweepCollisionChannel, int).
                CallParam::opt("collisionChannel", WirePort::CollisionChannel, Type::Int),
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
}
