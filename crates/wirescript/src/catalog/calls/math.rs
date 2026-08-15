//! Scalar math calls: trig, unary math gates, min/max/pow/clamp, and bitwise ops.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Trig / math (pure expression) ----------------------------------
    m.insert("sin", math_unary("sin", gc::MATH_SIN));
    m.insert("cos", math_unary("cos", gc::MATH_COS));
    m.insert("asin", math_unary("asin", gc::MATH_ASIN));
    m.insert("acos", math_unary("acos", gc::MATH_ACOS));
    m.insert("atan", math_unary("atan", gc::MATH_ATAN));

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

    m.insert(
        "LogicalShiftRight",
        CallSpec {
            name: "LogicalShiftRight",
            gate_class: gc::EXPR_BITWISE_SHR_LOGICAL,
            params: vec![
                CallParam::req("a", WirePort::InputA, Type::Int),
                CallParam::req("b", WirePort::InputB, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput { field: None, port: WirePort::Output, ty: Type::Int }],
            receiver: None,
        },
    );
    m.insert(
        "Remap",
        CallSpec {
            name: "Remap",
            gate_class: gc::EXPR_REMAP,
            params: vec![
                CallParam::req("value", WirePort::Value, Type::Float),
                CallParam::req("inMin", WirePort::InputMin, Type::Float),
                CallParam::req("inMax", WirePort::InputMax, Type::Float),
                CallParam::req("outMin", WirePort::OutputMin, Type::Float),
                CallParam::req("outMax", WirePort::OutputMax, Type::Float),
            ],
            exec: false,
            outputs: vec![CallOutput { field: None, port: WirePort::Output, ty: Type::Float }],
            receiver: None,
        },
    );
}
