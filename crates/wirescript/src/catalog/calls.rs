//! Built-in call catalog.
//!
//! Maps source-level call names (e.g. `displayText`, `sin`, `vec`) to the
//! concrete gate class they lower to, their port-wiring shape, and whether
//! the call is exec-form (chains into `currentExec`) or pure-expression
//! (returns a value via an output port).
//!
//! hand-authored, so we keep the Rust form structurally identical for
//! easy cross-checking.

use crate::collections::HashMap;
use std::sync::OnceLock;

use crate::ir::Type;
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;

#[derive(Clone, Debug)]
pub struct CallParam {
    /// Source-level parameter name (used for named-arg form).
    pub name: &'static str,
    /// Port name on the target gate to wire this argument into.
    pub port: WirePort,
    /// Accepted type. `Character` and `Controller` are interchangeable at a
    /// param port — they wire directly into each other in Brickadia, so no
    /// adapter gate is inserted.
    pub ty: Type,
    /// When true, callers may omit the argument; the gate's default stays.
    pub optional: bool,
}

impl CallParam {
    pub const fn req(name: &'static str, port: WirePort, ty: Type) -> Self {
        Self {
            name,
            port,
            ty,
            optional: false,
        }
    }
    pub const fn opt(name: &'static str, port: WirePort, ty: Type) -> Self {
        Self {
            name,
            port,
            ty,
            optional: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CallOutput {
    pub port: WirePort,
    pub ty: Type,
    /// Record field this output binds to when the call returns a record
    /// (e.g. `Edge`'s `rising`). Named outputs make the call result bind as
    /// a field→port record, so field access resolves through the spec
    /// instead of port-name matching. None for single-value outputs.
    pub field: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct CallSpec {
    pub name: &'static str,
    pub gate_class: &'static str,
    pub params: Vec<CallParam>,
    /// If true, the call is exec-form: chains into the current exec and
    /// advances it via the gate's `ExecOut`. If false, the call is pure
    /// and `output` identifies the value-producing port.
    pub exec: bool,
    pub outputs: Vec<CallOutput>,
    /// The type of the first param when this call can be used as a receiver
    /// method: `entity.SetLocation(pos)` instead of `SetLocation(entity, pos)`.
    /// None means no receiver form.
    pub receiver: Option<Type>,
}

fn math_unary(name: &'static str, gate_class: &'static str) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params: vec![CallParam::req("x", WirePort::Input, Type::Float)],
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: WirePort::Output,
            ty: Type::Float,
        }],
        receiver: None,
    }
}

fn vec_expr(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    out_port: WirePort,
    out_ty: Type,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: out_port,
            ty: out_ty,
        }],
        receiver: None,
    }
}

/// A pure gate returning a record: one named output per field. The spec's
/// return type is the record derived from `fields`, and lowering binds the
/// result as a field→port record (no port-name matching).
fn vec_expr_record(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    fields: Vec<(&'static str, WirePort, Type)>,
) -> CallSpec {
    let record_ty = Type::Record(
        fields
            .iter()
            .map(|(f, _, ty)| (f.to_string(), ty.clone()))
            .collect(),
    );
    let mut outputs: Vec<CallOutput> = fields
        .into_iter()
        .map(|(f, port, ty)| CallOutput {
            field: Some(f),
            port,
            ty,
        })
        .collect();
    // The first output doubles as the call's primary value; it carries the
    // record type so a bare (non-field) use typechecks as the record.
    if let Some(first) = outputs.first_mut() {
        first.ty = record_ty;
    }
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs,
        receiver: None,
    }
}

fn vec_recv(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    out_port: WirePort,
    out_ty: Type,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: out_port,
            ty: out_ty,
        }],
        receiver: Some(Type::Vector),
    }
}

/// The value types a math-variant port accepts, matching the game's
/// `WireGraphPrimMathVariant` (`f64, i64, Vector, Rotator, Quat, LinearColor`).
/// `Blend`/`lerp`/`Tween` interpolate any of these, not just floats.
fn blend_variant() -> Type {
    Type::Union(vec![
        Type::Float,
        Type::Int,
        Type::Vector,
        Type::Rotator,
        Type::Quat,
        Type::Color,
    ])
}

/// Pure (non-exec) expression gate whose first param is the method receiver.
fn expr_recv(
    name: &'static str,
    gate_class: &'static str,
    receiver: Type,
    params: Vec<CallParam>,
    out_port: WirePort,
    out_ty: Type,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: out_port,
            ty: out_ty,
        }],
        receiver: Some(receiver),
    }
}

fn entity_exec(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    outputs: Vec<CallOutput>,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: true,
        outputs,
        receiver: Some(Type::Entity),
    }
}

fn controller_exec(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    outputs: Vec<CallOutput>,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: true,
        outputs,
        receiver: Some(Type::Controller),
    }
}

fn character_exec(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    outputs: Vec<CallOutput>,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: true,
        outputs,
        receiver: Some(Type::Character),
    }
}

fn build_calls() -> HashMap<&'static str, CallSpec> {
    let mut m: HashMap<&'static str, CallSpec> = HashMap::default();

    // ---- Controller --------------------------------------------------------
    m.insert(
        "DisplayText",
        CallSpec {
            name: "DisplayText",
            gate_class: gc::CONTROLLER_DISPLAY_TEXT,
            params: vec![
                CallParam::req("target", WirePort::Controller, Type::Controller),
                CallParam::req("text", WirePort::Text, Type::Any),
                CallParam::opt("positionX", WirePort::PositionX, Type::Float),
                CallParam::opt("positionY", WirePort::PositionY, Type::Float),
                CallParam::opt("anchorX", WirePort::AnchorX, Type::Float),
                CallParam::opt("anchorY", WirePort::AnchorY, Type::Float),
                CallParam::opt("scaleX", WirePort::ScaleX, Type::Float),
                CallParam::opt("scaleY", WirePort::ScaleY, Type::Float),
                CallParam::opt("angle", WirePort::Angle, Type::Float),
                CallParam::opt("fontSize", WirePort::FontSize, Type::Float),
                CallParam::opt("outlineSize", WirePort::OutlineSize, Type::Float),
                CallParam::opt("justify", WirePort::Justification, Type::Int),
                CallParam::opt("lifetime", WirePort::Lifetime, Type::Float),
                CallParam::opt("transition", WirePort::Transition, Type::Float),
                CallParam::opt("easing", WirePort::Easing, Type::Int),
                CallParam::opt("textId", WirePort::TextId, Type::Int),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Controller),
        },
    );

    // ---- Character / Controller conversions ------------------------------
    m.insert(
        "ControllerOf",
        entity_exec(
            "ControllerOf",
            gc::CONTROLLER_GET_FROM_ENTITY,
            vec![CallParam::req("entity", WirePort::Entity, Type::Entity)],
            vec![CallOutput {
            field: None,
                port: WirePort::Controller,
                ty: Type::Controller,
            }],
        ),
    );
    m.insert(
        "CharacterOf",
        controller_exec(
            "CharacterOf",
            gc::CHARACTER_GET_FROM_CONTROLLER,
            vec![CallParam::req("controller", WirePort::Controller, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::Character,
                ty: Type::Character,
            }],
        ),
    );

    // ---- Camera / aim ---------------------------------------------------
    // Single GetAim gate exposing both outputs as a record:
    // `char.GetAim().Origin` / `.Direction`.
    m.insert(
        "GetAim",
        character_exec(
            "GetAim",
            gc::CHARACTER_GET_AIM,
            vec![CallParam::req("character", WirePort::Character, Type::Character)],
            vec![CallOutput {
            field: None,
                port: WirePort::Origin,
                ty: Type::Record(vec![
                    ("Origin".into(), Type::Vector),
                    ("Direction".into(), Type::Vector),
                ]),
            }],
        ),
    );
    m.insert(
        "InputReader",
        CallSpec {
            name: "InputReader",
            gate_class: gc::INPUT_SPLITTER,
            params: vec![CallParam::req("character", WirePort::Character, Type::Character)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::InputForward,
                ty: Type::Record(vec![
                    ("Forward".into(), Type::Float),
                    ("Right".into(), Type::Float),
                    ("Jump".into(), Type::Bool),
                ]),
            }],
            receiver: Some(Type::Character),
        },
    );

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

    // ---- Sleep (pure, delayed pass-through) ------------------------------
    m.insert(
        "Sleep",
        CallSpec {
            name: "Sleep",
            gate_class: gc::BUFFER_SECONDS,
            params: vec![
                CallParam::req("input", WirePort::Input, Type::Any),
                CallParam::opt("delay", WirePort::SecondsToWait, Type::Float),
                CallParam::opt("hold", WirePort::ZeroSecondsToWait, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Any,
            }],
            receiver: None,
        },
    );
    m.insert(
        "SleepTicks",
        CallSpec {
            name: "SleepTicks",
            gate_class: gc::BUFFER_TICKS,
            params: vec![
                CallParam::req("input", WirePort::Input, Type::Any),
                CallParam::opt("delay", WirePort::TicksToWait, Type::Int),
                CallParam::opt("hold", WirePort::ZeroTicksToWait, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Any,
            }],
            receiver: None,
        },
    );

    // ---- Trig / math (pure expression) ----------------------------------
    m.insert("sin", math_unary("sin", gc::MATH_SIN));
    m.insert("cos", math_unary("cos", gc::MATH_COS));
    m.insert("asin", math_unary("asin", gc::MATH_ASIN));
    m.insert("acos", math_unary("acos", gc::MATH_ACOS));
    m.insert("atan", math_unary("atan", gc::MATH_ATAN));

    // ---- Vector constructors / ops (pure expression) --------------------
    m.insert(
        "Vec",
        vec_expr(
            "Vec",
            gc::MAKE_VECTOR,
            vec![
                CallParam::req("x", WirePort::X, Type::Float),
                CallParam::req("y", WirePort::Y, Type::Float),
                CallParam::req("z", WirePort::Z, Type::Float),
            ],
            WirePort::Output,
            Type::Vector,
        ),
    );
    m.insert(
        "Dot",
        vec_recv(
            "Dot",
            gc::VEC_DOT,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Vector),
                CallParam::req("b", WirePort::InputB, Type::Vector),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "Cross",
        vec_recv(
            "Cross",
            gc::VEC_CROSS,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Vector),
                CallParam::req("b", WirePort::InputB, Type::Vector),
            ],
            WirePort::Output,
            Type::Vector,
        ),
    );
    m.insert(
        "Normalize",
        vec_recv(
            "Normalize",
            gc::VEC_NORMALIZE,
            vec![CallParam::req("v", WirePort::Input, Type::Vector)],
            WirePort::Output,
            Type::Vector,
        ),
    );
    m.insert(
        "Magnitude",
        vec_recv(
            "Magnitude",
            gc::VEC_MAGNITUDE,
            vec![CallParam::req("v", WirePort::Input, Type::Vector)],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "Distance",
        vec_recv(
            "Distance",
            gc::VEC_DISTANCE,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Vector),
                CallParam::req("b", WirePort::InputB, Type::Vector),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "ScaleVec",
        vec_recv(
            "ScaleVec",
            gc::VEC_SCALE,
            vec![
                CallParam::req("v", WirePort::Input, Type::Vector),
                CallParam::req("scalar", WirePort::Scalar, Type::Float),
            ],
            WirePort::Output,
            Type::Vector,
        ),
    );

    // ---- Rotation / quaternion (cl14428+) -----------------------------
    // `rotator` = euler (Pitch/Yaw/Roll, used by entity rotation); `quat` =
    // quaternion produced by the conversion gates. Concise, display-name-based.
    m.insert(
        "Rotation",
        vec_expr(
            "Rotation",
            gc::MAKE_ROTATION,
            vec![
                CallParam::req("pitch", WirePort::Pitch, Type::Float),
                CallParam::req("yaw", WirePort::Yaw, Type::Float),
                CallParam::req("roll", WirePort::Roll, Type::Float),
            ],
            WirePort::Output,
            Type::Rotator,
        ),
    );
    m.insert(
        "ToEuler",
        expr_recv(
            "ToEuler",
            gc::SPLIT_ROTATION,
            Type::Rotator,
            vec![CallParam::req("r", WirePort::Input, Type::Rotator)],
            WirePort::Pitch,
            Type::Record(vec![
                ("Pitch".into(), Type::Float),
                ("Yaw".into(), Type::Float),
                ("Roll".into(), Type::Float),
            ]),
        ),
    );
    m.insert(
        "ToRotation",
        expr_recv(
            "ToRotation",
            gc::DIRECTION_TO_ROTATION,
            Type::Vector,
            vec![CallParam::req("direction", WirePort::Direction, Type::Vector)],
            WirePort::Output,
            Type::Quat,
        ),
    );
    m.insert(
        "ToDirection",
        expr_recv(
            "ToDirection",
            gc::ROTATION_TO_DIRECTION,
            Type::Quat,
            vec![CallParam::req("rotation", WirePort::Rotation, Type::Quat)],
            WirePort::Output,
            Type::Vector,
        ),
    );
    m.insert(
        "Rotate",
        expr_recv(
            "Rotate",
            gc::ROTATE_VECTOR,
            Type::Vector,
            vec![
                CallParam::req("v", WirePort::Vector, Type::Vector),
                CallParam::req("rotation", WirePort::Rotation, Type::Quat),
            ],
            WirePort::Output,
            Type::Vector,
        ),
    );
    m.insert(
        "Invert",
        expr_recv(
            "Invert",
            gc::INVERT_ROTATION,
            Type::Quat,
            vec![CallParam::req("q", WirePort::Input, Type::Quat)],
            WirePort::Output,
            Type::Quat,
        ),
    );
    m.insert(
        "RotationTo",
        expr_recv(
            "RotationTo",
            gc::QUAT_BETWEEN,
            Type::Vector,
            vec![
                CallParam::req("from", WirePort::From, Type::Vector),
                CallParam::req("to", WirePort::To, Type::Vector),
            ],
            WirePort::Output,
            Type::Quat,
        ),
    );
    m.insert(
        "AngleTo",
        expr_recv(
            "AngleTo",
            gc::QUAT_ANGLE_BETWEEN,
            Type::Quat,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Quat),
                CallParam::req("b", WirePort::InputB, Type::Quat),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "RotationByAngle",
        expr_recv(
            "RotationByAngle",
            gc::QUAT_FROM_AXIS_ANGLE,
            Type::Vector,
            vec![
                CallParam::req("axis", WirePort::Axis, Type::Vector),
                CallParam::req("angle", WirePort::Angle, Type::Float),
            ],
            WirePort::Output,
            Type::Quat,
        ),
    );
    m.insert(
        "ToAxisAngle",
        expr_recv(
            "ToAxisAngle",
            gc::QUAT_TO_AXIS_ANGLE,
            Type::Quat,
            vec![CallParam::req("q", WirePort::Input, Type::Quat)],
            WirePort::Axis,
            Type::Record(vec![
                ("Axis".into(), Type::Vector),
                ("Angle".into(), Type::Float),
            ]),
        ),
    );
    m.insert(
        "Slerp",
        expr_recv(
            "Slerp",
            gc::QUAT_SLERP,
            Type::Quat,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Quat),
                CallParam::req("b", WirePort::InputB, Type::Quat),
                CallParam::req("alpha", WirePort::Alpha, Type::Float),
            ],
            WirePort::Output,
            Type::Quat,
        ),
    );

    // ---- sRGB / hex color (cl14428+) ----------------------------------
    m.insert(
        "ColorSRGB",
        vec_expr(
            "ColorSRGB",
            gc::MAKE_COLOR_SRGB,
            vec![
                CallParam::req("r", WirePort::R, Type::Int),
                CallParam::req("g", WirePort::G, Type::Int),
                CallParam::req("b", WirePort::B, Type::Int),
                CallParam::req("a", WirePort::A, Type::Int),
            ],
            WirePort::Output,
            Type::Color,
        ),
    );
    m.insert(
        "ColorHex",
        vec_expr(
            "ColorHex",
            gc::MAKE_COLOR_HEX,
            vec![CallParam::req("hex", WirePort::Hex, Type::String)],
            WirePort::Output,
            Type::Color,
        ),
    );
    m.insert(
        "ToSRGB",
        expr_recv(
            "ToSRGB",
            gc::SPLIT_COLOR_SRGB,
            Type::Color,
            vec![CallParam::req("c", WirePort::Input, Type::Color)],
            WirePort::R,
            Type::Record(vec![
                ("R".into(), Type::Int),
                ("G".into(), Type::Int),
                ("B".into(), Type::Int),
                ("A".into(), Type::Int),
            ]),
        ),
    );
    m.insert(
        "ToHex",
        expr_recv(
            "ToHex",
            gc::COLOR_TO_HEX,
            Type::Color,
            vec![CallParam::req("c", WirePort::Input, Type::Color)],
            WirePort::Hex,
            Type::String,
        ),
    );
    // `Blend` is the math blend gate — an alias for `lerp`, taking any of the
    // math variants. `ColorBlend` below is a DIFFERENT gate (it carries a
    // colour-space selection), so both stay reachable.
    m.insert(
        "Blend",
        expr_recv(
            "Blend",
            gc::MATH_BLEND,
            blend_variant(),
            vec![
                CallParam::req("a", WirePort::InputA, blend_variant()),
                CallParam::req("b", WirePort::InputB, blend_variant()),
                CallParam::req("alpha", WirePort::Blend, Type::Float),
            ],
            WirePort::Output,
            blend_variant(),
        ),
    );
    m.insert(
        "ColorBlend",
        expr_recv(
            "ColorBlend",
            gc::COLOR_BLEND,
            Type::Color,
            vec![
                CallParam::req("a", WirePort::ColorA, Type::Color),
                CallParam::req("b", WirePort::ColorB, Type::Color),
                CallParam::req("alpha", WirePort::Alpha, Type::Float),
            ],
            WirePort::Output,
            Type::Color,
        ),
    );

    // ---- Controller role check (cl14428+) -----------------------------
    // `ctrl.HasRole("Admin")` — RoleName is a config string, returns a bool.
    m.insert(
        "HasRole",
        controller_exec(
            "HasRole",
            gc::CONTROLLER_HAS_ROLE,
            vec![
                CallParam::req("target", WirePort::Controller, Type::Controller),
                CallParam::req("role", WirePort::RoleName, Type::String),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::BHasRole,
                ty: Type::Bool,
            }],
        ),
    );

    // ---- Character inventory (cl14428+) -------------------------------
    // `char.GiveWeapon($BRItemBase/Weapon_Pistol, slot)` — sets an inventory
    // slot to an item asset. The weapon asset is carried as the nested
    // EntryPlan.ItemTypeIfItem; the emitter builds the EntryPlan struct.
    m.insert(
        "GiveWeapon",
        character_exec(
            "GiveWeapon",
            gc::CHARACTER_SET_INVENTORY_ENTRY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("weapon", WirePort::ItemTypeIfItem, Type::Any),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
            ],
            vec![],
        ),
    );

    // ---- Stateful exec value gates (cl14428+) -------------------------
    // Advance per exec pulse: Cycle returns 0..Count-1, Toggle flips a bool.
    m.insert(
        "Cycle",
        CallSpec {
            name: "Cycle",
            gate_class: gc::EXEC_CYCLE,
            params: vec![CallParam::req("count", WirePort::Count, Type::Int)],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: None,
        },
    );
    m.insert(
        "Toggle",
        CallSpec {
            name: "Toggle",
            gate_class: gc::EXEC_TOGGLE,
            params: vec![],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: None,
        },
    );

    // ---- Player input (Seat Control Splitter) -------------------------
    // ---- Random number ------------------------------------------------
    m.insert(
        "Random",
        CallSpec {
            name: "Random",
            gate_class: gc::RANDOM,
            params: vec![
                CallParam::req("min", WirePort::Min, Type::Int),
                CallParam::req("max", WirePort::Max, Type::Int),
            ],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: None,
        },
    );

    // ---- Format text ---------------------------------------------------
    // Wraps the FormatText gate. Format string may be wired or literal.
    // Up to 7 inputs (InputA-G). Output is the formatted string.
    m.insert(
        "Fmt",
        CallSpec {
            name: "Fmt",
            gate_class: gc::STRING_FORMAT_TEXT,
            params: vec![
                CallParam::req("format", WirePort::FormatString, Type::Any),
                CallParam::opt("a", WirePort::InputA, Type::Any),
                CallParam::opt("b", WirePort::InputB, Type::Any),
                CallParam::opt("c", WirePort::InputC, Type::Any),
                CallParam::opt("d", WirePort::InputD, Type::Any),
                CallParam::opt("e", WirePort::InputE, Type::Any),
                CallParam::opt("f", WirePort::InputF, Type::Any),
                CallParam::opt("g", WirePort::InputG, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: None,
        },
    );

    // ---- Math (hyperbolic, exp/ln, sign, round, min/max) -----------------
    m.insert("sinh", math_unary("sinh", gc::MATH_SINH));
    m.insert("cosh", math_unary("cosh", gc::MATH_COSH));
    m.insert("tanh", math_unary("tanh", gc::MATH_TANH));
    m.insert("asinh", math_unary("asinh", gc::MATH_ASINH));
    m.insert("acosh", math_unary("acosh", gc::MATH_ACOSH));
    m.insert("atanh", math_unary("atanh", gc::MATH_ATANH));
    m.insert("exp", math_unary("exp", gc::MATH_EXP));
    m.insert("ln", math_unary("ln", gc::MATH_LN));
    m.insert("sign", math_unary("sign", gc::MATH_SIGN));
    m.insert("round", math_unary("round", gc::ROUND));
    m.insert("floor", math_unary("floor", gc::FLOOR));
    m.insert("ceil", math_unary("ceil", gc::CEIL));
    m.insert("abs", math_unary("abs", gc::MATH_ABS));
    m.insert("sqrt", math_unary("sqrt", gc::MATH_SQRT));
    m.insert("Deg2Rad", math_unary("Deg2Rad", gc::DEG2RAD));
    m.insert("Rad2Deg", math_unary("Rad2Deg", gc::RAD2DEG));
    m.insert(
        "min",
        vec_expr(
            "min",
            gc::MATH_MIN,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Float),
                CallParam::req("b", WirePort::InputB, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "max",
        vec_expr(
            "max",
            gc::MATH_MAX,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Float),
                CallParam::req("b", WirePort::InputB, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "pow",
        vec_expr(
            "pow",
            gc::MATH_POW,
            vec![
                CallParam::req("x", WirePort::Input, Type::Float),
                CallParam::req("exponent", WirePort::Exponent, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "clamp",
        vec_expr(
            "clamp",
            gc::MATH_CLAMP,
            vec![
                CallParam::req("x", WirePort::Input, Type::Float),
                CallParam::req("min", WirePort::Min, Type::Float),
                CallParam::req("max", WirePort::Max, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "atan2",
        vec_expr(
            "atan2",
            gc::MATH_ATAN2,
            vec![
                CallParam::req("y", WirePort::Y, Type::Float),
                CallParam::req("x", WirePort::X, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );

    // ---- Bitwise --------------------------------------------------------
    m.insert(
        "BitCount",
        vec_expr(
            "BitCount",
            gc::BITWISE_BIT_COUNT,
            vec![CallParam::req("x", WirePort::Input, Type::Int)],
            WirePort::Output,
            Type::Int,
        ),
    );

    // ---- Select / Swap --------------------------------------------------
    m.insert(
        "Select",
        CallSpec {
            name: "Select",
            gate_class: gc::SELECT,
            params: vec![
                CallParam::req("cond", WirePort::BSelectB, Type::Bool),
                CallParam::req("a", WirePort::InputA, Type::Any),
                CallParam::req("b", WirePort::InputB, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Any,
            }],
            receiver: None,
        },
    );
    m.insert(
        "Swap",
        CallSpec {
            name: "Swap",
            gate_class: gc::SWAP,
            params: vec![
                CallParam::req("cond", WirePort::BSwap, Type::Bool),
                CallParam::req("a", WirePort::InputA, Type::Any),
                CallParam::req("b", WirePort::InputB, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Record(vec![("a".into(), Type::Any), ("b".into(), Type::Any)]),
            }],
            receiver: None,
        },
    );

    // ---- Color ----------------------------------------------------------
    m.insert(
        "Color",
        vec_expr(
            "Color",
            gc::MAKE_COLOR,
            vec![
                CallParam::req("r", WirePort::R, Type::Float),
                CallParam::req("g", WirePort::G, Type::Float),
                CallParam::req("b", WirePort::B, Type::Float),
                CallParam::opt("a", WirePort::A, Type::Float),
            ],
            WirePort::Output,
            Type::Color,
        ),
    );

    // ---- Vector (additional) --------------------------------------------
    m.insert(
        "DistanceSq",
        vec_recv(
            "DistanceSq",
            gc::VEC_DISTANCE_SQ,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Vector),
                CallParam::req("b", WirePort::InputB, Type::Vector),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "MagnitudeSq",
        vec_recv(
            "MagnitudeSq",
            gc::VEC_MAGNITUDE_SQ,
            vec![CallParam::req("v", WirePort::Input, Type::Vector)],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "RotToDir",
        vec_recv(
            "RotToDir",
            gc::VEC_ROT_TO_DIR,
            vec![CallParam::req("rot", WirePort::Input, Type::Vector)],
            WirePort::Output,
            Type::Vector,
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
                CallParam::req("dest", WirePort::Destination, Type::Any),
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
                CallParam::req("source", WirePort::Source, Type::Any),
                CallParam::req("dest", WirePort::Destination, Type::Any),
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

    // ---- Gamemode -------------------------------------------------------
    m.insert(
        "SetLeaderboard",
        controller_exec(
            "SetLeaderboard",
            gc::GAMEMODE_SET_LEADERBOARD,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "IncLeaderboard",
        controller_exec(
            "IncLeaderboard",
            gc::GAMEMODE_INC_LEADERBOARD,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetLeaderboard",
        controller_exec(
            "GetLeaderboard",
            gc::GAMEMODE_GET_LEADERBOARD,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("key", WirePort::Key, Type::String),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Int,
            }],
        ),
    );
    m.insert(
        "GetTeam",
        character_exec(
            "GetTeam",
            gc::GAMEMODE_GET_TEAM,
            vec![CallParam::req("character", WirePort::Character, Type::Character)],
            vec![CallOutput {
            field: None,
                port: WirePort::Team,
                ty: Type::Any,
            }],
        ),
    );

    // ---- String operations -----------------------------------------------
    m.insert(
        "Length",
        CallSpec {
            name: "Length",
            gate_class: gc::STRING_LENGTH,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Contains",
        CallSpec {
            name: "Contains",
            gate_class: gc::STRING_CONTAINS,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("search", WirePort::Search, Type::String),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "StartsWith",
        CallSpec {
            name: "StartsWith",
            gate_class: gc::STRING_STARTS_WITH,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("prefix", WirePort::Prefix, Type::String),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "EndsWith",
        CallSpec {
            name: "EndsWith",
            gate_class: gc::STRING_ENDS_WITH,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("suffix", WirePort::Suffix, Type::String),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Substring",
        CallSpec {
            name: "Substring",
            gate_class: gc::STRING_SUBSTRING,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("start", WirePort::Start, Type::Int),
                CallParam::req("length", WirePort::Length, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Replace",
        CallSpec {
            name: "Replace",
            gate_class: gc::STRING_REPLACE,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("search", WirePort::Search, Type::String),
                CallParam::req("replacement", WirePort::Replacement, Type::String),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Find",
        CallSpec {
            name: "Find",
            gate_class: gc::STRING_FIND,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("search", WirePort::Search, Type::String),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Split",
        CallSpec {
            name: "Split",
            gate_class: gc::STRING_SPLIT,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("delimiter", WirePort::Delimiter, Type::String),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Left,
                ty: Type::Record(vec![
                    ("Left".into(), Type::String),
                    ("Right".into(), Type::String),
                ]),
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "ToLower",
        CallSpec {
            name: "ToLower",
            gate_class: gc::STRING_TO_LOWER,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "ToUpper",
        CallSpec {
            name: "ToUpper",
            gate_class: gc::STRING_TO_UPPER,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Trim",
        CallSpec {
            name: "Trim",
            gate_class: gc::STRING_TRIM,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );

    // ---- Math (additional) -----------------------------------------------
    m.insert("tan", math_unary("tan", gc::MATH_TAN));
    m.insert(
        "log",
        vec_expr(
            "log",
            gc::MATH_LOG_BASE,
            vec![
                CallParam::req("x", WirePort::Input, Type::Float),
                CallParam::req("base", WirePort::Base, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );
    m.insert(
        "lerp",
        vec_expr(
            "lerp",
            gc::MATH_BLEND,
            vec![
                CallParam::req("a", WirePort::InputA, blend_variant()),
                CallParam::req("b", WirePort::InputB, blend_variant()),
                CallParam::req("t", WirePort::Blend, Type::Float),
            ],
            WirePort::Output,
            blend_variant(),
        ),
    );
    m.insert(
        "fmod",
        vec_expr(
            "fmod",
            gc::MATH_MOD_FLOORED,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Float),
                CallParam::req("b", WirePort::InputB, Type::Float),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );

    // ---- Vector (split) --------------------------------------------------
    m.insert(
        "SplitVec",
        CallSpec {
            name: "SplitVec",
            gate_class: gc::SPLIT_VECTOR,
            params: vec![CallParam::req("v", WirePort::Input, Type::Vector)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::X,
                ty: Type::Record(vec![
                    ("x".into(), Type::Float),
                    ("y".into(), Type::Float),
                    ("z".into(), Type::Float),
                ]),
            }],
            receiver: Some(Type::Vector),
        },
    );
    m.insert(
        "SplitColor",
        CallSpec {
            name: "SplitColor",
            gate_class: gc::SPLIT_COLOR,
            params: vec![CallParam::req("c", WirePort::Input, Type::Color)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::R,
                ty: Type::Record(vec![
                    ("r".into(), Type::Float),
                    ("g".into(), Type::Float),
                    ("b".into(), Type::Float),
                    ("a".into(), Type::Float),
                ]),
            }],
            receiver: Some(Type::Color),
        },
    );

    // ---- Edge detector ---------------------------------------------------
    m.insert(
        "Edge",
        vec_expr_record(
            "Edge",
            gc::EDGE_DETECTOR,
            vec![CallParam::req("input", WirePort::Input, Type::Bool)],
            vec![
                ("Rising", WirePort::BPulseOnRisingEdge, Type::Bool),
                ("Falling", WirePort::BPulseOnFallingEdge, Type::Bool),
            ],
        ),
    );
    // "Edge Detector (Exec)": exec pulses when a float input rises/falls,
    // for `on e.Rising { }` / `await e.Falling` (Timer.Expired-style).
    m.insert(
        "EdgeExec",
        vec_expr_record(
            "EdgeExec",
            gc::EDGE_DETECTOR_EXEC,
            vec![CallParam::req("input", WirePort::Input, Type::Float)],
            vec![
                ("Rising", WirePort::OnRisingEdge, Type::Exec),
                ("Falling", WirePort::OnFallingEdge, Type::Exec),
            ],
        ),
    );

    // ---- Gamemode (additional) -------------------------------------------
    // The old imperative `EndRound` gate is gone; a round now ends by
    // declaring a winner via PlayerWins / TeamWins.
    m.insert(
        "PlayerWins",
        CallSpec {
            name: "PlayerWins",
            gate_class: gc::GAMEMODE_PLAYER_WINS,
            params: vec![
                CallParam::req("player", WirePort::Player, Type::Controller),
                CallParam::opt("teamWinsInstead", WirePort::BTeamWinsInstead, Type::Bool),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Controller),
        },
    );
    m.insert(
        "TeamWins",
        CallSpec {
            name: "TeamWins",
            gate_class: gc::GAMEMODE_TEAM_WINS,
            params: vec![CallParam::req("team", WirePort::Team, Type::Entity)],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "GetCurrentRound",
        CallSpec {
            name: "GetCurrentRound",
            gate_class: gc::GAMEMODE_GET_CURRENT_ROUND,
            params: vec![],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::RoundNumber,
                ty: Type::Int,
            }],
            receiver: None,
        },
    );
    m.insert(
        "GetTeamByName",
        CallSpec {
            name: "GetTeamByName",
            gate_class: gc::GAMEMODE_GET_TEAM_BY_NAME,
            params: vec![CallParam::req("name", WirePort::TeamName, Type::String)],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Team,
                ty: Type::Any,
            }],
            receiver: None,
        },
    );

    // ---- Character (additional) ------------------------------------------
    m.insert(
        "ShowHint",
        character_exec(
            "ShowHint",
            gc::CHARACTER_SHOW_HINT,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("title", WirePort::HintTitle, Type::String),
                CallParam::req("text", WirePort::HintText, Type::String),
            ],
            vec![],
        ),
    );

    m.insert(
        "GetDamage",
        character_exec(
            "GetDamage",
            gc::CHARACTER_GET_DAMAGE,
            vec![CallParam::req("character", WirePort::Character, Type::Character)],
            vec![CallOutput {
            field: None,
                port: WirePort::Damage,
                ty: Type::Float,
            }],
        ),
    );
    m.insert(
        "SetDamage",
        character_exec(
            "SetDamage",
            gc::CHARACTER_SET_DAMAGE,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("damage", WirePort::Damage, Type::Float),
            ],
            vec![],
        ),
    );
    m.insert(
        "IncDamage",
        character_exec(
            "IncDamage",
            gc::CHARACTER_INC_DAMAGE,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("amount", WirePort::Amount, Type::Float),
            ],
            vec![],
        ),
    );

    // ---- Controller (additional) -----------------------------------------
    m.insert(
        "ShowStatusMessage",
        controller_exec(
            "ShowStatusMessage",
            gc::CONTROLLER_SHOW_STATUS,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("message", WirePort::Message, Type::String),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetUserName",
        controller_exec(
            "GetUserName",
            gc::CONTROLLER_GET_USER_NAME,
            vec![CallParam::req("controller", WirePort::Controller, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::UserName,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "GetUserId",
        controller_exec(
            "GetUserId",
            gc::CONTROLLER_GET_USER_ID,
            vec![CallParam::req("controller", WirePort::Controller, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::UserId,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "GetDisplayName",
        controller_exec(
            "GetDisplayName",
            gc::CONTROLLER_GET_DISPLAY_NAME,
            vec![CallParam::req("controller", WirePort::Controller, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::DisplayName,
                ty: Type::String,
            }],
        ),
    );
    m.insert(
        "IsTrusted",
        controller_exec(
            "IsTrusted",
            gc::CONTROLLER_IS_TRUSTED,
            vec![CallParam::req("controller", WirePort::Controller, Type::Controller)],
            vec![CallOutput {
            field: None,
                port: WirePort::BIsTrusted,
                ty: Type::Bool,
            }],
        ),
    );
    m.insert(
        "SetCanRespawn",
        controller_exec(
            "SetCanRespawn",
            gc::CONTROLLER_SET_CAN_RESPAWN,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("canRespawn", WirePort::BCanRespawn, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "HasPermission",
        controller_exec(
            "HasPermission",
            gc::CONTROLLER_HAS_PERMISSION,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("permission", WirePort::PermissionName, Type::String),
            ],
            vec![CallOutput {
            field: None,
                port: WirePort::BHasPermission,
                ty: Type::Bool,
            }],
        ),
    );
    m.insert(
        "SetTempPermission",
        character_exec(
            "SetTempPermission",
            gc::CHARACTER_SET_TEMP_PERMISSION,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("permission", WirePort::PermissionTagStr, Type::String),
                CallParam::req("enable", WirePort::BPermissionEnable, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetTeamPinned",
        controller_exec(
            "SetTeamPinned",
            gc::GAMEMODE_SET_TEAM_PINNED,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("pinned", WirePort::BPinned, Type::Bool),
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

    // ---- String parsing (pure) -------------------------------------------
    m.insert(
        "ParseInt",
        CallSpec {
            name: "ParseInt",
            gate_class: gc::STRING_PARSE_INT,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Int,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "ParseNumber",
        CallSpec {
            name: "ParseNumber",
            gate_class: gc::STRING_PARSE_NUMBER,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Float,
            }],
            receiver: Some(Type::String),
        },
    );

    // ---- Gamemode teams (additional) -------------------------------------
    m.insert(
        "SetTeam",
        controller_exec(
            "SetTeam",
            gc::GAMEMODE_SET_TEAM,
            vec![
                CallParam::req("controller", WirePort::Controller, Type::Controller),
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::opt("pin", WirePort::BPinPlayerToTeam, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "GetTeamName",
        CallSpec {
            name: "GetTeamName",
            gate_class: gc::GAMEMODE_GET_TEAM_NAME,
            params: vec![CallParam::req("team", WirePort::Team, Type::Entity)],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Name,
                ty: Type::String,
            }],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "GetTeamLeaderboardValue",
        CallSpec {
            name: "GetTeamLeaderboardValue",
            gate_class: gc::GAMEMODE_GET_TEAM_LEADERBOARD,
            params: vec![
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
            ],
            exec: true,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Int,
            }],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "SetTeamLeaderboardValue",
        CallSpec {
            name: "SetTeamLeaderboardValue",
            gate_class: gc::GAMEMODE_SET_TEAM_LEADERBOARD,
            params: vec![
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Int),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );
    m.insert(
        "IncrementTeamLeaderboardValue",
        CallSpec {
            name: "IncrementTeamLeaderboardValue",
            gate_class: gc::GAMEMODE_INC_TEAM_LEADERBOARD,
            params: vec![
                CallParam::req("team", WirePort::Team, Type::Entity),
                CallParam::req("key", WirePort::Key, Type::String),
                CallParam::req("value", WirePort::Value, Type::Int),
            ],
            exec: true,
            outputs: vec![],
            receiver: Some(Type::Entity),
        },
    );

    // ---- Misc value / exec gates -----------------------------------------
    m.insert(
        "PrintToConsole",
        CallSpec {
            name: "PrintToConsole",
            gate_class: gc::PRINT_TO_CONSOLE,
            params: vec![CallParam::req("text", WirePort::Text, Type::Any)],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );
    // Opaque — identity rerouter. Blocks constant folding: its output is
    // permanently Unknown to the (future) fold pass, and typecheck treats
    // the result as `any`, so probe circuits can drive real gates with
    // known values (`Opaque(2) + 3` emits a real MathAdd).
    m.insert(
        "Opaque",
        CallSpec {
            name: "Opaque",
            gate_class: gc::REROUTER,
            params: vec![CallParam::req("value", WirePort::RerInput, Type::Any)],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::RerOutput,
                ty: Type::Opaque,
            }],
            receiver: None,
        },
    );
    m.insert(
        "DeltaTime",
        CallSpec {
            name: "DeltaTime",
            gate_class: gc::DELTA_TIME,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::DeltaTime,
                ty: Type::Float,
            }],
            receiver: None,
        },
    );
    m.insert(
        "ServerUptime",
        CallSpec {
            name: "ServerUptime",
            gate_class: gc::SERVER_UPTIME,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Uptime,
                ty: Type::Float,
            }],
            receiver: None,
        },
    );
    // Read Brick Grid — the brick grid this gate's microchip is placed on.
    m.insert(
        "ReadBrickGrid",
        CallSpec {
            name: "ReadBrickGrid",
            gate_class: gc::READ_BRICK_GRID,
            params: vec![],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::BrickGrid,
                ty: Type::Entity,
            }],
            receiver: None,
        },
    );
    m.insert(
        "NearlyEqual",
        CallSpec {
            name: "NearlyEqual",
            gate_class: gc::NEARLY_EQUAL,
            params: vec![
                CallParam::req("a", WirePort::InputA, Type::Float),
                CallParam::req("b", WirePort::InputB, Type::Float),
                CallParam::req("tolerance", WirePort::Tolerance, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::BOutput,
                ty: Type::Bool,
            }],
            receiver: None,
        },
    );
    m.insert(
        "Dampen",
        CallSpec {
            name: "Dampen",
            gate_class: gc::PSEUDO_DAMPEN,
            params: vec![
                CallParam::req("target", WirePort::Target, Type::Float),
                CallParam::req("smoothTime", WirePort::SmoothTime, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Float,
            }],
            receiver: None,
        },
    );
    // Easing: interpolate a..b by blend with an easing curve. `function` and
    // `direction` accept an int or an easing-enum name literal ("Quad",
    // "InOut", ...) resolved against EBREasingFunction / EBREasingDirection.
    m.insert(
        "Easing",
        CallSpec {
            name: "Easing",
            gate_class: gc::MATH_EASING,
            params: vec![
                CallParam::req("a", WirePort::InputA, blend_variant()),
                CallParam::req("b", WirePort::InputB, blend_variant()),
                CallParam::req("blend", WirePort::Blend, Type::Float),
                CallParam::opt("function", WirePort::Function, Type::Any),
                CallParam::opt("direction", WirePort::Direction, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: blend_variant(),
            }],
            receiver: None,
        },
    );
    // Tween: stateful eased interpolation toward `target` over `duration`.
    m.insert(
        "Tween",
        CallSpec {
            name: "Tween",
            gate_class: gc::PSEUDO_TWEEN,
            params: vec![
                CallParam::req("target", WirePort::Target, blend_variant()),
                CallParam::req("duration", WirePort::Duration, Type::Float),
                CallParam::opt("function", WirePort::Function, Type::Any),
                CallParam::opt("direction", WirePort::Direction, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: blend_variant(),
            }],
            receiver: None,
        },
    );
    // Timer: stateful countdown. `restart`/`pause`/`resume` are optional exec
    // signals that drive it; returns `{ Time: float, Expired: exec }`.
    m.insert(
        "Timer",
        CallSpec {
            name: "Timer",
            gate_class: gc::PSEUDO_TIMER,
            params: vec![
                CallParam::req("limit", WirePort::Limit, Type::Float),
                CallParam::opt("restart", WirePort::Restart, Type::Exec),
                CallParam::opt("pause", WirePort::Pause, Type::Exec),
                CallParam::opt("resume", WirePort::Resume, Type::Exec),
            ],
            exec: false,
            outputs: vec![
                CallOutput {
            field: None,
                    port: WirePort::Time,
                    ty: Type::Float,
                },
                CallOutput {
            field: None,
                    port: WirePort::Expired,
                    ty: Type::Exec,
                },
            ],
            receiver: None,
        },
    );

    // ---- Bitwise (additional) --------------------------------------------
    m.insert(
        "BitNand",
        vec_expr(
            "BitNand",
            gc::BITWISE_NAND,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Int),
                CallParam::req("b", WirePort::InputB, Type::Int),
            ],
            WirePort::Output,
            Type::Int,
        ),
    );
    m.insert(
        "BitNor",
        vec_expr(
            "BitNor",
            gc::BITWISE_NOR,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Int),
                CallParam::req("b", WirePort::InputB, Type::Int),
            ],
            WirePort::Output,
            Type::Int,
        ),
    );

    // ---- Prefab spawner -------------------------------------------------
    m.insert(
        "SpawnPrefab",
        CallSpec {
            name: "SpawnPrefab",
            gate_class: gc::PREFAB_SPAWNER,
            params: vec![
                // The prefab to spawn: a `$./file.brz` / `$/abs/file.brz`
                // reference. Embedded into the bundle at emit; the gate's
                // `Prefab` bundle_path_ref property gets the resulting path.
                CallParam::opt("prefab", WirePort::Prefab, Type::Any),
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

    // ---- Sweep (raycasting) ---------------------------------------------
    m.insert(
        "Sweep",
        CallSpec {
            name: "Sweep",
            gate_class: gc::SWEEP,
            params: vec![
                CallParam::req("origin", WirePort::Origin, Type::Vector),
                CallParam::req("direction", WirePort::Direction, Type::Vector),
                CallParam::req("Distance", WirePort::Distance, Type::Float),
                CallParam::opt("radius", WirePort::Radius, Type::Float),
                CallParam::opt("relative", WirePort::BRelative, Type::Bool),
                CallParam::opt("ignore", WirePort::IgnoreEntity, Type::Entity),
                // cl14477: what the sweep detects (each defaults off in-engine).
                CallParam::opt("detectBricks", WirePort::BDetectBricks, Type::Bool),
                CallParam::opt("detectPlayers1", WirePort::BDetectPlayers1, Type::Bool),
                CallParam::opt("detectPlayers2", WirePort::BDetectPlayers2, Type::Bool),
                CallParam::opt("detectPlayers3", WirePort::BDetectPlayers3, Type::Bool),
                CallParam::opt("detectPlayers4", WirePort::BDetectPlayers4, Type::Bool),
                CallParam::opt("detectPhysics", WirePort::BDetectPhysics, Type::Bool),
                CallParam::opt("detectMap", WirePort::BDetectMap, Type::Bool),
                CallParam::opt("ignoreOwningGrid", WirePort::BIgnoreOwningGrid, Type::Bool),
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
                    ("Hit".into(), Type::Exec),
                    ("Miss".into(), Type::Exec),
                ]),
            }],
            receiver: None,
        },
    );

    // ---- Messaging --------------------------------------------
    m.insert(
        "ShowChatMessage",
        controller_exec(
            "ShowChatMessage",
            gc::CONTROLLER_SHOW_CHAT,
            vec![
                CallParam::req("target", WirePort::Controller, Type::Controller),
                CallParam::req("message", WirePort::Message, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "ShowMessageBox",
        controller_exec(
            "ShowMessageBox",
            gc::CONTROLLER_SHOW_MESSAGE_BOX,
            vec![
                CallParam::req("target", WirePort::Controller, Type::Controller),
                CallParam::req("message", WirePort::Message, Type::Any),
                CallParam::opt("title", WirePort::Title, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "BroadcastChatMessage",
        CallSpec {
            name: "BroadcastChatMessage",
            gate_class: gc::GAMEMODE_BROADCAST_CHAT,
            params: vec![CallParam::req("message", WirePort::Message, Type::Any)],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );
    m.insert(
        "BroadcastStatusMessage",
        CallSpec {
            name: "BroadcastStatusMessage",
            gate_class: gc::GAMEMODE_BROADCAST_STATUS,
            params: vec![
                CallParam::req("message", WirePort::Message, Type::Any),
                CallParam::opt("flash", WirePort::BFlashIfUnchanged, Type::Bool),
            ],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
    );

    // ---- Audio -------------------------------------------------------------
    // The audio asset is a `$BrickOneShotAudioDescriptor/...` reference,
    // inlined into the gate's AudioDescriptor data field (like GiveWeapon).
    m.insert(
        "PlayAudioAt",
        entity_exec(
            "PlayAudioAt",
            gc::PLAY_AUDIO_AT,
            vec![
                CallParam::req("entity", WirePort::Entity, Type::Entity),
                CallParam::req("audio", WirePort::AudioDescriptor, Type::Any),
                CallParam::opt("volume", WirePort::VolumeMultiplier, Type::Float),
                CallParam::opt("pitch", WirePort::PitchMultiplier, Type::Float),
                CallParam::opt("innerRadius", WirePort::InnerRadius, Type::Float),
                CallParam::opt("maxDistance", WirePort::MaxDistance, Type::Float),
                CallParam::opt("spatialized", WirePort::BSpatialization, Type::Bool),
            ],
            vec![],
        ),
    );
    m.insert(
        "PlayGlobalAudio",
        CallSpec {
            name: "PlayGlobalAudio",
            gate_class: gc::PLAY_GLOBAL_AUDIO,
            params: vec![
                CallParam::req("audio", WirePort::AudioDescriptor, Type::Any),
                CallParam::opt("volume", WirePort::VolumeMultiplier, Type::Float),
                CallParam::opt("pitch", WirePort::PitchMultiplier, Type::Float),
            ],
            exec: true,
            outputs: vec![],
            receiver: None,
        },
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
                CallParam::req("tag", WirePort::Tag, Type::Any),
            ],
            vec![],
        ),
    );

    // ---- Player lookup (exec gate) ------------------------------------------
    // Has Exec/ExecOut ports and emits the found player's character.
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
                ty: Type::Character,
            }],
            receiver: None,
        },
    );

    // ---- Change detectors ----------------------------------------
    // `Change` pulses its input value through when it changes — that's the
    // "Change Detector (Exec)" gate (its OnChanged output moved there when
    // the game split the detectors in cl14860). The plain "Change Detector"
    // emits a bool pulse instead, exposed as `Changed`.
    m.insert(
        "Change",
        vec_expr(
            "Change",
            gc::CHANGE_DETECTOR_EXEC,
            vec![CallParam::req("input", WirePort::Input, Type::Any)],
            WirePort::OnChanged,
            Type::Exec,
        ),
    );
    m.insert(
        "Changed",
        vec_expr(
            "Changed",
            gc::CHANGE_DETECTOR,
            vec![CallParam::req("input", WirePort::Input, Type::Any)],
            WirePort::BPulseOnChange,
            Type::Bool,
        ),
    );

    // ---- Quaternion make/split/dot -------------------------------
    m.insert(
        "Quat",
        vec_expr(
            "Quat",
            gc::MAKE_QUATERNION,
            vec![
                CallParam::req("x", WirePort::X, Type::Float),
                CallParam::req("y", WirePort::Y, Type::Float),
                CallParam::req("z", WirePort::Z, Type::Float),
                CallParam::req("w", WirePort::W, Type::Float),
            ],
            WirePort::Output,
            Type::Quat,
        ),
    );
    m.insert(
        "SplitQuat",
        CallSpec {
            name: "SplitQuat",
            gate_class: gc::SPLIT_QUATERNION,
            params: vec![CallParam::req("q", WirePort::Input, Type::Quat)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::X,
                ty: Type::Record(vec![
                    ("X".into(), Type::Float),
                    ("Y".into(), Type::Float),
                    ("Z".into(), Type::Float),
                    ("W".into(), Type::Float),
                ]),
            }],
            receiver: Some(Type::Quat),
        },
    );
    m.insert(
        "QuatDot",
        expr_recv(
            "QuatDot",
            gc::QUAT_DOT_PRODUCT,
            Type::Quat,
            vec![
                CallParam::req("a", WirePort::InputA, Type::Quat),
                CallParam::req("b", WirePort::InputB, Type::Quat),
            ],
            WirePort::Output,
            Type::Float,
        ),
    );

    // ---- Character inventory family ------------------------------
    // Asset args ($BRItemBase/..., $BrickTypeAsset/..., entity types) inline
    // into the gate's class/object data fields (like GiveWeapon).
    m.insert(
        "AddInventoryItem",
        character_exec(
            "AddInventoryItem",
            gc::CHARACTER_ADD_INVENTORY_ITEM,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::Item, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryItem",
        character_exec(
            "SetInventoryItem",
            gc::CHARACTER_SET_INVENTORY_ITEM,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::Item, Type::Any),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "AddInventoryBrick",
        character_exec(
            "AddInventoryBrick",
            gc::CHARACTER_ADD_INVENTORY_BRICK,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("brick", WirePort::BrickAsset, Type::Any),
                CallParam::opt("size", WirePort::ProceduralSize, Type::Vector),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryBrick",
        character_exec(
            "SetInventoryBrick",
            gc::CHARACTER_SET_INVENTORY_BRICK,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("brick", WirePort::BrickAsset, Type::Any),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
                CallParam::opt("size", WirePort::ProceduralSize, Type::Vector),
            ],
            vec![],
        ),
    );
    m.insert(
        "AddInventoryEntity",
        character_exec(
            "AddInventoryEntity",
            gc::CHARACTER_ADD_INVENTORY_ENTITY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("entityType", WirePort::EntityType, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryEntity",
        character_exec(
            "SetInventoryEntity",
            gc::CHARACTER_SET_INVENTORY_ENTITY,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("entityType", WirePort::EntityType, Type::Any),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
            ],
            vec![],
        ),
    );
    m.insert(
        "AddInventoryItemAdv",
        character_exec(
            "AddInventoryItemAdv",
            gc::CHARACTER_ADD_INVENTORY_ITEM_ADV,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::ItemType, Type::Any),
                CallParam::opt("damage", WirePort::DamageMultiplier, Type::Float),
                CallParam::opt("speed", WirePort::WeaponSpeedMultiplier, Type::Float),
                CallParam::opt("scale", WirePort::ItemScale, Type::Float),
                CallParam::opt("itemName", WirePort::ItemNameOverride, Type::String),
                CallParam::opt("projectile", WirePort::ProjectileOverride, Type::Any),
            ],
            vec![],
        ),
    );
    m.insert(
        "SetInventoryItemAdv",
        character_exec(
            "SetInventoryItemAdv",
            gc::CHARACTER_SET_INVENTORY_ITEM_ADV,
            vec![
                CallParam::req("character", WirePort::Character, Type::Character),
                CallParam::req("item", WirePort::ItemType, Type::Any),
                CallParam::opt("slot", WirePort::Slot, Type::Int),
                CallParam::opt("damage", WirePort::DamageMultiplier, Type::Float),
                CallParam::opt("speed", WirePort::WeaponSpeedMultiplier, Type::Float),
                CallParam::opt("scale", WirePort::ItemScale, Type::Float),
                CallParam::opt("itemName", WirePort::ItemNameOverride, Type::String),
                CallParam::opt("projectile", WirePort::ProjectileOverride, Type::Any),
            ],
            vec![],
        ),
    );

    m
}

pub fn calls() -> &'static HashMap<&'static str, CallSpec> {
    static INSTANCE: OnceLock<HashMap<&'static str, CallSpec>> = OnceLock::new();
    INSTANCE.get_or_init(build_calls)
}

pub fn find_call(name: &str) -> Option<&'static CallSpec> {
    calls().get(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_text_exec_form() {
        let c = find_call("DisplayText").unwrap();
        assert!(c.exec);
        assert_eq!(c.params[0].name, "target");
        assert!(matches!(c.params[0].ty, Type::Controller));
    }

    #[test]
    fn sin_pure_form() {
        let c = find_call("sin").unwrap();
        assert!(!c.exec);
        assert_eq!(c.outputs[0].port, WirePort::Output);
    }

    #[test]
    fn vec_has_three_params() {
        assert_eq!(find_call("Vec").unwrap().params.len(), 3);
    }

    #[test]
    fn unknown_call_returns_none() {
        assert!(find_call("doesNotExist").is_none());
    }

    #[test]
    fn leaderboard_getters_return_int() {
        // The inventory dump types both leaderboard `Value` outputs as `int`;
        // `GetLeaderboard` was declared `Any`, so arithmetic on its result had
        // no operator overload.
        for name in ["GetLeaderboard", "GetTeamLeaderboardValue"] {
            let c = find_call(name).unwrap();
            assert!(
                matches!(c.outputs[0].ty, Type::Int),
                "{name} should return int, got {:?}",
                c.outputs[0].ty
            );
        }
    }

    /// Params that name a settable field the gate does not expose as a wire
    /// input. These only ever work as constants — lowering writes them into the
    /// component's data and drops the wire; a computed value has nowhere to go.
    ///
    /// This list pins what the current inventory reports. It should shrink, not
    /// grow: a new entry means either a mistyped port or a gate whose ports
    /// changed in a game update.
    const DATA_ONLY_PARAMS: &[&str] = &[
        "DisplayText.easing -> Easing",
        "DisplayText.fontSize -> FontSize",
        "DisplayText.justify -> Justification",
        "Easing.direction -> Direction",
        "Easing.function -> Function",
        "GiveWeapon.weapon -> ItemTypeIfItem",
        "HasPermission.permission -> PermissionName",
        "HasRole.role -> RoleName",
        "PlayAudioAt.audio -> AudioDescriptor",
        "PlayGlobalAudio.audio -> AudioDescriptor",
        "SetTempPermission.permission -> PermissionTagStr",
        "ShowHint.text -> HintText",
        "ShowHint.title -> HintTitle",
        "SpawnPrefab.prefab -> Prefab",
    ];

    /// Every catalog param must name a real wire input on its gate, or be a
    /// known data-only field. A param pointing at a field the gate has no input
    /// port for wires to a slot the game does not have.
    #[test]
    fn every_call_param_targets_a_real_wire_input() {
        let mut found: Vec<String> = Vec::new();
        for (name, spec) in calls().iter() {
            // Pseudo/internal gates are absent from the inventory dump.
            if crate::catalog::default_catalog()
                .find_by_class(spec.gate_class)
                .is_none()
            {
                continue;
            }
            for p in &spec.params {
                if !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str()) {
                    found.push(format!("{name}.{} -> {}", p.name, p.port.as_str()));
                }
            }
        }
        found.sort();
        let expected: Vec<String> = DATA_ONLY_PARAMS.iter().map(|s| s.to_string()).collect();
        let new: Vec<&String> = found.iter().filter(|f| !expected.contains(f)).collect();
        assert!(new.is_empty(), "params bound to non-wire ports: {new:#?}");
        let gone: Vec<&String> = expected.iter().filter(|e| !found.contains(e)).collect();
        assert!(
            gone.is_empty(),
            "these are wireable now — drop them from DATA_ONLY_PARAMS: {gone:#?}"
        );
    }

    #[test]
    fn font_size_is_not_a_wire_input() {
        let g = gc::CONTROLLER_DISPLAY_TEXT;
        assert!(!crate::catalog::is_wire_input(g, "FontSize"));
        assert!(crate::catalog::is_wire_input(g, "Text"));
        assert!(crate::catalog::is_wire_input(g, "PositionX"));
    }
}
