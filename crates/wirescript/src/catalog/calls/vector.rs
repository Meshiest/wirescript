//! Vector, rotation, and quaternion calls.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
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

    // ---- Rotation / quaternion -----------------------------
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
                // Config-only (settings menu, not wire inputs).
                CallParam::opt("shortestPath", WirePort::BShortestPath, Type::Bool),
                CallParam::opt("clampAlpha", WirePort::BClampAlpha, Type::Bool),
            ],
            WirePort::Output,
            Type::Quat,
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
    // (Former `RotToDir` builtin removed: its gate `Expr_VecRotationToDirection`
    // no longer exists in the build. Use `ToDirection` (RotationToDirection).)

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
}
