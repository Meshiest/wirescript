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
            vec![CallOutput::plain(WirePort::Vector, Type::Vector)],
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
            vec![CallOutput::plain(WirePort::BFrozen, Type::Bool)],
        ),
    );
    m.insert(
        "GetRotation",
        entity_exec(
            "GetRotation",
            gc::ENTITY_GET_ROTATION,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput::plain(WirePort::Rotation, Type::Rotator)],
        ),
    );
    m.insert(
        "GetLocationRotation",
        entity_exec(
            "GetLocationRotation",
            gc::ENTITY_GET_LOCATION_ROTATION,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput::plain(
                WirePort::Vector,
                Type::Record(vec![
                    ("Vector".into(), Type::Vector),
                    ("Rotation".into(), Type::Rotator),
                ]),
            )],
        ),
    );
    m.insert(
        "GetLinearVelocity",
        entity_exec(
            "GetLinearVelocity",
            gc::ENTITY_GET_LINEAR_VELOCITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput::plain(WirePort::LinearVelocity, Type::Vector)],
        ),
    );
    m.insert(
        "GetAngularVelocity",
        entity_exec(
            "GetAngularVelocity",
            gc::ENTITY_GET_ANGULAR_VELOCITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput::plain(WirePort::AngularVelocity, Type::Vector)],
        ),
    );
    m.insert(
        "GetVelocity",
        entity_exec(
            "GetVelocity",
            gc::ENTITY_GET_VELOCITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput::plain(
                WirePort::Vector,
                Type::Record(vec![
                    ("Vector".into(), Type::Vector),
                    ("Rotation".into(), Type::Rotator),
                ]),
            )],
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
    m.insert(
        "SetVisible",
        entity_exec(
            "SetVisible",
            gc::ENTITY_SET_VISIBLE,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("visible", WirePort::BVisible, Type::Bool),
            ],
            vec![],
        ),
    );
    // `visibleTo` is a player or a team entity; left unwired, everyone sees
    // it again.
    m.insert(
        "SetVisibleTo",
        entity_exec(
            "SetVisibleTo",
            gc::ENTITY_SET_VISIBLE_TO,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::opt(
                    "visibleTo",
                    WirePort::VisibleTo,
                    Type::Union(vec![Type::Controller, Type::Character, Type::Entity]),
                ),
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
            vec![CallOutput::plain(WirePort::Tag, Type::String)],
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
    // ---- Entity custom properties ----------------------------------------
    // A key/value store on an entity, player or dynamic grid: 256 keys per
    // holder, keys up to 64 characters (measured in game; longer keys are not stored). The gates save `Key` as a string.
    m.insert(
        "SetCustomProperty",
        entity_exec(
            "SetCustomProperty",
            gc::ENTITY_SET_CUSTOM_PROPERTY,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetCustomProperty",
        entity_exec(
            "GetCustomProperty",
            gc::ENTITY_GET_CUSTOM_PROPERTY,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
            ],
            vec![CallOutput::plain(
                WirePort::Value,
                Type::Record(vec![
                    ("Value".into(), Type::Opaque),
                    ("Found".into(), Type::Bool),
                ]),
            )],
        ),
    );
    m.insert(
        "RemoveCustomProperty",
        entity_exec(
            "RemoveCustomProperty",
            gc::ENTITY_REMOVE_CUSTOM_PROPERTY,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
            ],
            vec![CallOutput::plain(WirePort::BFound, Type::Bool)],
        ),
    );
    // Replaces the entity's custom properties with a copy of `source`'s.
    m.insert(
        "CopyCustomProperties",
        entity_exec(
            "CopyCustomProperties",
            gc::ENTITY_COPY_CUSTOM_PROPERTIES,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("source", WirePort::Source, Type::Entity),
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
            vec![CallOutput::plain(WirePort::LinearVelocity, Type::Vector)],
        ),
    );
    m.insert(
        "GetSpeed",
        entity_exec(
            "GetSpeed",
            gc::ENTITY_GET_SPEED,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput::plain(WirePort::Speed, Type::Float)],
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
            vec![CallOutput::plain(WirePort::Team, Type::Entity)],
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
            outputs: vec![CallOutput::plain(
                WirePort::Location,
                Type::Record(vec![
                    ("Location".into(), Type::Vector),
                    ("Rotation".into(), Type::Rotator),
                ]),
            )],
            receiver: None,
        },
    );
    // The program's own microchip brick: a var holding a brick reference,
    // baked at emit (`emit/handles.rs`), so it has no gate of its own.
    m.insert(
        "GetMicrochipBrick",
        CallSpec {
            name: "GetMicrochipBrick",
            gate_class: gc::VAR,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput::plain(WirePort::Value, Type::Brick)],
            receiver: None,
        },
    );
    // ---- Brick properties ----------------------------------------------
    // Read or write one setting of a referenced brick. `property` is a
    // constant `BrickProperty` member baked into the gate; the value's type
    // follows it (`bool` for Visible and the collision flags, `color` for
    // Color). Typecheck types the read and checks the write from the member
    // (`typecheck::sig::brick_property_type`), and lowering pins the read's
    // `T` from that result.
    m.insert(
        "GetBrickProperty",
        CallSpec {
            name: "GetBrickProperty",
            gate_class: gc::GET_BRICK_PROPERTY,
            params: vec![
                CallParam::req("brick", WirePort::Brick, Type::Brick),
                CallParam::req("property", WirePort::Property, Type::Int),
            ],
            exec: true,
            outputs: vec![CallOutput::plain(WirePort::Value, Type::Param("T".into()))],
            receiver: Some(Type::Brick),
        },
    );
    m.insert(
        "SetBrickProperty",
        CallSpec {
            name: "SetBrickProperty",
            gate_class: gc::SET_BRICK_PROPERTY,
            params: vec![
                CallParam::req("brick", WirePort::Brick, Type::Brick),
                CallParam::req("property", WirePort::Property, Type::Int),
                CallParam::req("value", WirePort::Value, Type::Any),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Brick),
        },
    );
    // ---- Components ----------------------------------------------------
    // `brick.GetComponent($Component/PointLight)` gives a reference typed by
    // the component type it names (`Component<PointLight>`, typed in
    // `typecheck::call`); `AddComponent` adds one first if the brick has none.
    // The `$Component/...` is a constant baked into the gate's Component Type.
    for (name, gate) in [
        ("GetComponent", gc::GET_COMPONENT),
        ("AddComponent", gc::ADD_COMPONENT),
    ] {
        m.insert(
            name,
            CallSpec {
                name,
                gate_class: gate,
                params: vec![
                    CallParam::req("brick", WirePort::Brick, Type::Brick),
                    CallParam::req(
                        "componentType",
                        WirePort::ComponentType,
                        Type::Component(String::new()),
                    ),
                ],
                exec: true,
                outputs: vec![CallOutput::plain(
                    WirePort::Component,
                    Type::Param("T".into()),
                )],
                receiver: Some(Type::Brick),
            },
        );
    }
    // Removes the brick's component of that type after the current update,
    // unless an `AddComponent` of its type runs first. The gate takes the
    // brick, not a component reference, and no gate gives a reference's brick.
    m.insert(
        "RemoveComponent",
        CallSpec {
            name: "RemoveComponent",
            gate_class: gc::REMOVE_COMPONENT,
            params: vec![
                CallParam::req("brick", WirePort::Brick, Type::Brick),
                CallParam::req(
                    "componentType",
                    WirePort::ComponentType,
                    Type::Component(String::new()),
                ),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Brick),
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
                CallParam::opt(
                    "spreadTowardCenter",
                    WirePort::BSpreadBiasedTowardsCenter,
                    Type::Bool,
                ),
                CallParam::opt("detectBricks", WirePort::BDetectBricks, Type::Bool),
                CallParam::opt("detectPlayers1", WirePort::BDetectPlayers1, Type::Bool),
                CallParam::opt("detectPlayers2", WirePort::BDetectPlayers2, Type::Bool),
                CallParam::opt("detectPlayers3", WirePort::BDetectPlayers3, Type::Bool),
                CallParam::opt("detectPlayers4", WirePort::BDetectPlayers4, Type::Bool),
                CallParam::opt(
                    "bodyPartsOnly",
                    WirePort::BOnlyHitPlayerBodyParts,
                    Type::Bool,
                ),
                CallParam::opt("detectPhysics", WirePort::BDetectPhysics, Type::Bool),
                CallParam::opt("detectMap", WirePort::BDetectMap, Type::Bool),
                // Wire input: collision channel (EBRSweepCollisionChannel, int).
                CallParam::opt("collisionChannel", WirePort::CollisionChannel, Type::Int),
            ],
            exec: true,
            outputs: vec![CallOutput::plain(
                WirePort::HitDistance,
                Type::Record(vec![
                    ("HitDistance".into(), Type::Float),
                    ("HitEntity".into(), Type::Entity),
                    ("HitLocation".into(), Type::Vector),
                    ("HitNormal".into(), Type::Vector),
                    ("HitColor".into(), Type::Color),
                    ("HitBrick".into(), Type::Brick),
                    ("Hit".into(), Type::Exec),
                    ("Miss".into(), Type::Exec),
                ]),
            )],
            receiver: None,
        },
    );
}
