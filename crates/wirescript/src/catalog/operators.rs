//! Operator resolution table.
//!
//! Maps surface-level operators (`&&`, `+`, `<`, etc.) to the concrete
//! gate classes and port mappings they lower to. Also drives typecheck:
//! given an operator and its operand types (after coercion), return the
//! matching rule.
//!

use std::sync::OnceLock;

use crate::ir::Type;
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;

/// Port-name layout for a lowered operator gate.
#[derive(Copy, Clone, Debug)]
pub struct OpPorts {
    pub inputs: &'static [WirePort],
    pub output: WirePort,
}

#[derive(Clone, Debug)]
pub struct OpRule {
    pub operands: &'static [Type],
    pub result: Type,
    pub gate_class: &'static str,
    pub ports: OpPorts,
}

#[derive(Clone, Debug)]
pub struct OpSpec {
    pub op: &'static str,
    pub arity: u8,
    pub rules: Vec<OpRule>,
}

// Canonical port layouts reused across rules.
const BINARY_PORTS: OpPorts = OpPorts {
    inputs: &[WirePort::InputA, WirePort::InputB],
    output: WirePort::Output,
};
const UNARY_PORTS: OpPorts = OpPorts {
    inputs: &[WirePort::Input],
    output: WirePort::Output,
};
const BOOL_BINARY_PORTS: OpPorts = OpPorts {
    inputs: &[WirePort::BInputA, WirePort::BInputB],
    output: WirePort::BOutput,
};
const BOOL_UNARY_PORTS: OpPorts = OpPorts {
    inputs: &[WirePort::BInput],
    output: WirePort::BOutput,
};
const COMPARE_PORTS: OpPorts = OpPorts {
    inputs: &[WirePort::InputA, WirePort::InputB],
    output: WirePort::BOutput,
};

fn math_binary(op: &'static str, class_math: &'static str, vec: bool) -> OpSpec {
    let mut rules = vec![
        OpRule {
            operands: &[Type::Float, Type::Float],
            result: Type::Float,
            gate_class: class_math,
            ports: BINARY_PORTS,
        },
        OpRule {
            operands: &[Type::Int, Type::Int],
            result: Type::Int,
            gate_class: class_math,
            ports: BINARY_PORTS,
        },
        OpRule {
            operands: &[Type::Float, Type::Int],
            result: Type::Float,
            gate_class: class_math,
            ports: BINARY_PORTS,
        },
        OpRule {
            operands: &[Type::Int, Type::Float],
            result: Type::Float,
            gate_class: class_math,
            ports: BINARY_PORTS,
        },
        // bool → int promotion (engine coerces bool wires to 0/1 on int ports).
        OpRule {
            operands: &[Type::Int, Type::Bool],
            result: Type::Int,
            gate_class: class_math,
            ports: BINARY_PORTS,
        },
        OpRule {
            operands: &[Type::Bool, Type::Int],
            result: Type::Int,
            gate_class: class_math,
            ports: BINARY_PORTS,
        },
    ];
    if vec {
        // Vector math runs on the same gate: MathAdd/Subtract/Multiply/Divide/
        // Modulo take WireGraphPrimMathVariant inputs, whose member set includes
        // Vector, f64 and i64. So vec⊕vec lowers component-wise, and mixing a
        // vector with a scalar broadcasts the scalar across the components
        // (e.g. `v * 2.0` scales) — all on the same `class_math` gate. The
        // result is always a vector.
        rules.extend([
            OpRule {
                operands: &[Type::Vector, Type::Vector],
                result: Type::Vector,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Vector, Type::Float],
                result: Type::Vector,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Vector, Type::Int],
                result: Type::Vector,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Float, Type::Vector],
                result: Type::Vector,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Int, Type::Vector],
                result: Type::Vector,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
        ]);
        // Colors run on the same PrimMath gate too: the variant member set
        // includes LinearColor, so `c1 + c2` operates RGBA channel-wise and
        // mixing a color with a scalar broadcasts it across the channels
        // (`c * 2.0` scales every channel). The result is always a color.
        rules.extend([
            OpRule {
                operands: &[Type::Color, Type::Color],
                result: Type::Color,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Color, Type::Float],
                result: Type::Color,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Color, Type::Int],
                result: Type::Color,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Float, Type::Color],
                result: Type::Color,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Int, Type::Color],
                result: Type::Color,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
        ]);
        // Rotation family (quat / rotator) on the same PrimMath gate: the
        // variant member set also covers rotations, so `q1 * q2` composes two
        // rotations, etc. quat↔rotator are interchangeable rotation values, so
        // same-type keeps its type and a mix yields a quat (the canonical
        // rotation-math result, freely coercible back to a rotator).
        rules.extend([
            OpRule {
                operands: &[Type::Quat, Type::Quat],
                result: Type::Quat,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Rotator, Type::Rotator],
                result: Type::Rotator,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Quat, Type::Rotator],
                result: Type::Quat,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Rotator, Type::Quat],
                result: Type::Quat,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
        ]);
    }
    // Object operands (players, entities, bricks…) no longer coerce directly to
    // an int on a math gate. Accept them here; `lower_binop` routes an object
    // operand through `(obj || false)` so it still reduces to an int the gate
    // takes — i.e. `1 + player` lowers to `add(1, or(player, false))`.
    const OBJECT_TYPES: &[Type] = &[
        Type::Entity,
        Type::Controller,
        Type::Character,
        Type::Brick,
        Type::Prefab,
    ];
    for o in OBJECT_TYPES {
        rules.extend([
            OpRule {
                operands: Box::leak(Box::new([Type::Int, o.clone()])),
                result: Type::Int,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: Box::leak(Box::new([o.clone(), Type::Int])),
                result: Type::Int,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: Box::leak(Box::new([Type::Float, o.clone()])),
                result: Type::Float,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: Box::leak(Box::new([o.clone(), Type::Float])),
                result: Type::Float,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: Box::leak(Box::new([o.clone(), o.clone()])),
                result: Type::Int,
                gate_class: class_math,
                ports: BINARY_PORTS,
            },
        ]);
    }
    OpSpec {
        op,
        arity: 2,
        rules,
    }
}

fn math_unary_op(op: &'static str, class_math: &'static str) -> OpSpec {
    OpSpec {
        op,
        arity: 1,
        rules: vec![
            OpRule {
                operands: &[Type::Float],
                result: Type::Float,
                gate_class: class_math,
                ports: UNARY_PORTS,
            },
            OpRule {
                operands: &[Type::Int],
                result: Type::Int,
                gate_class: class_math,
                ports: UNARY_PORTS,
            },
        ],
    }
}

fn logical_binary(op: &'static str, gate_class: &'static str) -> OpSpec {
    use Type::*;
    const LOGICAL_TYPES: &[Type] = &[
        Bool, Int, Float, Exec, String, Entity, Controller, Character, Brick, Prefab,
    ];
    let mut rules = Vec::new();
    for a in LOGICAL_TYPES {
        for b in LOGICAL_TYPES {
            rules.push(OpRule {
                operands: Box::leak(Box::new([a.clone(), b.clone()])),
                result: Bool,
                gate_class,
                ports: BOOL_BINARY_PORTS,
            });
        }
    }
    OpSpec {
        op,
        arity: 2,
        rules,
    }
}

fn bitwise_binary(op: &'static str, gate_class: &'static str) -> OpSpec {
    OpSpec {
        op,
        arity: 2,
        rules: vec![
            OpRule {
                operands: &[Type::Int, Type::Int],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Int, Type::Bool],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Bool, Type::Int],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Bool, Type::Bool],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Float, Type::Int],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Int, Type::Float],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Float, Type::Float],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Float, Type::Bool],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
            OpRule {
                operands: &[Type::Bool, Type::Float],
                result: Type::Int,
                gate_class,
                ports: BINARY_PORTS,
            },
        ],
    }
}

fn compare_binary(op: &'static str, gate_class: &'static str) -> OpSpec {
    use Type::*;
    const VARIANT_TYPES: &[Type] = &[
        Int, Float, Bool, String, Entity, Controller, Character, Brick, Prefab,
    ];
    let mut rules = Vec::new();
    for a in VARIANT_TYPES {
        for b in VARIANT_TYPES {
            rules.push(OpRule {
                operands: Box::leak(Box::new([a.clone(), b.clone()])),
                result: Bool,
                gate_class,
                ports: COMPARE_PORTS,
            });
        }
    }
    OpSpec {
        op,
        arity: 2,
        rules,
    }
}

fn build_operators() -> Vec<OpSpec> {
    vec![
        // Logical
        logical_binary("&&", "BrickComponentType_WireGraph_Expr_LogicalAND"),
        logical_binary("||", "BrickComponentType_WireGraph_Expr_LogicalOR"),
        {
            use Type::*;
            const NOT_TYPES: &[Type] = &[
                Bool, Int, Float, Exec, String, Entity, Controller, Character, Brick, Prefab,
            ];
            OpSpec {
                op: "!",
                arity: 1,
                rules: NOT_TYPES
                    .iter()
                    .map(|t| OpRule {
                        operands: Box::leak(Box::new([t.clone()])),
                        result: Bool,
                        gate_class: gc::LOGICAL_NOT,
                        ports: BOOL_UNARY_PORTS,
                    })
                    .collect(),
            }
        },
        logical_binary("^^", "BrickComponentType_WireGraph_Expr_LogicalXOR"),
        // Bitwise
        bitwise_binary("&", "BrickComponentType_WireGraph_Expr_BitwiseAND"),
        bitwise_binary("|", "BrickComponentType_WireGraph_Expr_BitwiseOR"),
        bitwise_binary("^", "BrickComponentType_WireGraph_Expr_BitwiseXOR"),
        bitwise_binary("<<", "BrickComponentType_WireGraph_Expr_BitwiseShiftLeft"),
        bitwise_binary(">>", "BrickComponentType_WireGraph_Expr_BitwiseShiftRight"),
        OpSpec {
            op: "~",
            arity: 1,
            rules: vec![
                OpRule {
                    operands: &[Type::Int],
                    result: Type::Int,
                    gate_class: gc::BITWISE_NOT,
                    ports: UNARY_PORTS,
                },
                OpRule {
                    operands: &[Type::Bool],
                    result: Type::Int,
                    gate_class: gc::BITWISE_NOT,
                    ports: UNARY_PORTS,
                },
                OpRule {
                    operands: &[Type::Float],
                    result: Type::Int,
                    gate_class: gc::BITWISE_NOT,
                    ports: UNARY_PORTS,
                },
            ],
        },
        // Arithmetic
        math_binary("+", "BrickComponentType_WireGraph_Expr_MathAdd", true),
        math_binary("-", "BrickComponentType_WireGraph_Expr_MathSubtract", true),
        math_binary("*", "BrickComponentType_WireGraph_Expr_MathMultiply", true),
        math_binary("/", "BrickComponentType_WireGraph_Expr_MathDivide", true),
        math_binary("%", "BrickComponentType_WireGraph_Expr_MathModulo", true),
        {
            const POW_PORTS: OpPorts = OpPorts {
                inputs: &[WirePort::Input, WirePort::Exponent],
                output: WirePort::Output,
            };
            use Type::*;
            OpSpec {
                op: "**",
                arity: 2,
                rules: vec![
                    OpRule {
                        operands: &[Float, Float],
                        result: Float,
                        gate_class: gc::MATH_POW,
                        ports: POW_PORTS,
                    },
                    OpRule {
                        operands: &[Int, Int],
                        result: Int,
                        gate_class: gc::MATH_POW,
                        ports: POW_PORTS,
                    },
                    OpRule {
                        operands: &[Float, Int],
                        result: Float,
                        gate_class: gc::MATH_POW,
                        ports: POW_PORTS,
                    },
                    OpRule {
                        operands: &[Int, Float],
                        result: Float,
                        gate_class: gc::MATH_POW,
                        ports: POW_PORTS,
                    },
                ],
            }
        },
        math_unary_op("-u", "BrickComponentType_WireGraph_Expr_MathNegate"),
        // Comparison
        compare_binary("==", "BrickComponentType_WireGraph_Expr_CompareEqual"),
        compare_binary("!=", "BrickComponentType_WireGraph_Expr_CompareNotEqual"),
        compare_binary("<", "BrickComponentType_WireGraph_Expr_CompareLess"),
        compare_binary("<=", "BrickComponentType_WireGraph_Expr_CompareLessOrEqual"),
        compare_binary(">", "BrickComponentType_WireGraph_Expr_CompareGreater"),
        compare_binary(
            ">=",
            "BrickComponentType_WireGraph_Expr_CompareGreaterOrEqual",
        ),
        // String concat. The game's String_Concatenate gate auto-converts any
        // wire-variant input (numbers, bools, vectors, entities, characters,
        // controllers, …) to a string, so accept every variant-able primitive
        // on either side.
        {
            use Type::*;
            const CONCAT_TYPES: &[Type] = &[
                String, Int, Float, Bool, Vector, Rotator, Quat, Color, Entity, Controller,
                Character, Brick, Prefab,
            ];
            let mut rules = Vec::new();
            for a in CONCAT_TYPES {
                for b in CONCAT_TYPES {
                    rules.push(OpRule {
                        operands: Box::leak(Box::new([a.clone(), b.clone()])),
                        result: String,
                        gate_class: gc::STRING_CONCATENATE,
                        ports: BINARY_PORTS,
                    });
                }
            }
            OpSpec {
                op: "..",
                arity: 2,
                rules,
            }
        },
    ]
}

pub fn operators() -> &'static [OpSpec] {
    static INSTANCE: OnceLock<Vec<OpSpec>> = OnceLock::new();
    INSTANCE.get_or_init(build_operators)
}

fn type_kind_matches(want: &Type, got: &Type) -> bool {
    std::mem::discriminant(want) == std::mem::discriminant(got)
}

/// Resolve `op` given operand types. Picks the first matching rule.
/// Numeric promotion is handled explicitly by rule order (float-first
/// for mixed-type arithmetic).
pub fn resolve_op(op: &str, arg_types: &[Type]) -> Option<&'static OpRule> {
    let arity = arg_types.len() as u8;
    let spec = operators()
        .iter()
        .find(|s| s.op == op && s.arity == arity)?;
    for rule in &spec.rules {
        if rule.operands.len() != arg_types.len() {
            continue;
        }
        if rule
            .operands
            .iter()
            .zip(arg_types.iter())
            .all(|(want, got)| type_kind_matches(want, got))
        {
            return Some(rule);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_int_int_returns_int() {
        let r = resolve_op("+", &[Type::Int, Type::Int]).unwrap();
        assert!(matches!(r.result, Type::Int));
        assert_eq!(r.gate_class, "BrickComponentType_WireGraph_Expr_MathAdd");
    }

    #[test]
    fn add_mixed_promotes_to_float() {
        let r = resolve_op("+", &[Type::Int, Type::Float]).unwrap();
        assert!(matches!(r.result, Type::Float));
    }

    #[test]
    fn concat_accepts_all_variant_primitives() {
        for t in [
            Type::Bool,
            Type::Float,
            Type::Vector,
            Type::Entity,
            Type::Controller,
            Type::Character,
        ] {
            for operands in [[Type::String, t.clone()], [t.clone(), Type::String]] {
                let r = resolve_op("..", &operands)
                    .unwrap_or_else(|| panic!(".. on {operands:?} should resolve"));
                assert!(matches!(r.result, Type::String));
            }
        }
    }

    #[test]
    fn vector_arithmetic_resolves_to_math_gates() {
        for (op, gate) in [
            ("+", "BrickComponentType_WireGraph_Expr_MathAdd"),
            ("-", "BrickComponentType_WireGraph_Expr_MathSubtract"),
            ("*", "BrickComponentType_WireGraph_Expr_MathMultiply"),
            ("/", "BrickComponentType_WireGraph_Expr_MathDivide"),
            ("%", "BrickComponentType_WireGraph_Expr_MathModulo"),
        ] {
            // vec⊗vec and vec⊗scalar (both directions) all lower to the same
            // math gate and produce a vector.
            for operands in [
                [Type::Vector, Type::Vector],
                [Type::Vector, Type::Float],
                [Type::Vector, Type::Int],
                [Type::Float, Type::Vector],
                [Type::Int, Type::Vector],
            ] {
                let r = resolve_op(op, &operands)
                    .unwrap_or_else(|| panic!("{op} {operands:?} should resolve"));
                assert!(
                    matches!(r.result, Type::Vector),
                    "{op} {operands:?} -> vector"
                );
                assert_eq!(r.gate_class, gate, "{op} {operands:?} uses {gate}");
            }
        }
    }

    #[test]
    fn color_arithmetic_resolves_to_math_gates() {
        for (op, gate) in [
            ("+", "BrickComponentType_WireGraph_Expr_MathAdd"),
            ("-", "BrickComponentType_WireGraph_Expr_MathSubtract"),
            ("*", "BrickComponentType_WireGraph_Expr_MathMultiply"),
            ("/", "BrickComponentType_WireGraph_Expr_MathDivide"),
            ("%", "BrickComponentType_WireGraph_Expr_MathModulo"),
        ] {
            // color*color and color*scalar (both directions) lower to the same
            // math gate and produce a color (RGBA channel-wise).
            for operands in [
                [Type::Color, Type::Color],
                [Type::Color, Type::Float],
                [Type::Color, Type::Int],
                [Type::Float, Type::Color],
                [Type::Int, Type::Color],
            ] {
                let r = resolve_op(op, &operands)
                    .unwrap_or_else(|| panic!("{op} {operands:?} should resolve"));
                assert!(
                    matches!(r.result, Type::Color),
                    "{op} {operands:?} -> color"
                );
                assert_eq!(r.gate_class, gate, "{op} {operands:?} uses {gate}");
            }
        }
    }

    #[test]
    fn rotation_arithmetic_resolves_to_math_gates() {
        // quat / rotator operands ride the same PrimMath gates (e.g. `q1 * q2`
        // composes two rotations). Same-type keeps its type; a mix yields a quat.
        for (op, gate) in [
            ("+", "BrickComponentType_WireGraph_Expr_MathAdd"),
            ("*", "BrickComponentType_WireGraph_Expr_MathMultiply"),
        ] {
            let qq = resolve_op(op, &[Type::Quat, Type::Quat]).unwrap();
            assert!(matches!(qq.result, Type::Quat));
            assert_eq!(qq.gate_class, gate);
            let rr = resolve_op(op, &[Type::Rotator, Type::Rotator]).unwrap();
            assert!(matches!(rr.result, Type::Rotator));
            let mix = resolve_op(op, &[Type::Rotator, Type::Quat]).unwrap();
            assert!(matches!(mix.result, Type::Quat));
        }
    }

    #[test]
    fn logical_and_coerces_all_types() {
        assert!(resolve_op("&&", &[Type::Bool, Type::Bool]).is_some());
        assert!(resolve_op("&&", &[Type::Int, Type::Bool]).is_some());
        assert!(resolve_op("&&", &[Type::Bool, Type::Int]).is_some());
        assert!(resolve_op("&&", &[Type::Exec, Type::Bool]).is_some());
        assert!(resolve_op("&&", &[Type::Entity, Type::Bool]).is_some());
        assert!(resolve_op("&&", &[Type::Controller, Type::Entity]).is_some());
        assert!(resolve_op("&&", &[Type::Float, Type::Exec]).is_some());
        assert!(resolve_op("&&", &[Type::Vector, Type::Bool]).is_none());
    }

    #[test]
    fn unary_negate_accepts_int_and_float() {
        assert!(resolve_op("-u", &[Type::Int]).is_some());
        assert!(resolve_op("-u", &[Type::Float]).is_some());
        assert!(resolve_op("-u", &[Type::Bool]).is_none());
    }

    #[test]
    fn bitwise_coerces_bool_to_int() {
        let r = resolve_op("<<", &[Type::Bool, Type::Int]).unwrap();
        assert!(matches!(r.result, Type::Int));
        assert!(resolve_op("&", &[Type::Bool, Type::Bool]).is_some());
        assert!(resolve_op("|", &[Type::Int, Type::Bool]).is_some());
        assert!(resolve_op("^", &[Type::Bool, Type::Int]).is_some());
    }

    #[test]
    fn bitwise_coerces_float_to_int() {
        let r = resolve_op("<<", &[Type::Float, Type::Int]).unwrap();
        assert!(matches!(r.result, Type::Int));
        assert!(resolve_op("&", &[Type::Float, Type::Float]).is_some());
        assert!(resolve_op("~", &[Type::Float]).is_some());
        assert!(resolve_op("~", &[Type::Bool]).is_some());
    }
}
