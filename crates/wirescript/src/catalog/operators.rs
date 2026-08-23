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

/// The operator spellings, named.
///
/// Match on `op::SHL` rather than `"<<"`: a mistyped string literal is a match
/// arm that silently never fires, while a mistyped constant does not compile.
/// Prefer the qualified `op::NAME` form in patterns — a bare imported name in
/// pattern position reads as a binding if it ever falls out of scope, which
/// would turn one arm into an irrefutable catch-all.
pub mod op {
    // Arithmetic
    pub const ADD: &str = "+";
    pub const SUB: &str = "-";
    pub const MUL: &str = "*";
    pub const DIV: &str = "/";
    pub const REM: &str = "%";
    pub const POW: &str = "**";
    /// Unary negation — the same spelling as [`SUB`], distinguished by arity.
    pub const NEG: &str = "-";
    /// String concatenation.
    pub const CONCAT: &str = "..";
    // Bitwise
    pub const BIT_AND: &str = "&";
    pub const BIT_OR: &str = "|";
    pub const BIT_XOR: &str = "^";
    pub const SHL: &str = "<<";
    pub const SHR: &str = ">>";
    pub const BIT_NOT: &str = "~";
    // Logical
    pub const AND: &str = "&&";
    pub const OR: &str = "||";
    pub const XOR: &str = "^^";
    pub const NOT: &str = "!";
    // Comparison
    pub const EQ: &str = "==";
    pub const NE: &str = "!=";
    pub const LT: &str = "<";
    pub const LE: &str = "<=";
    pub const GT: &str = ">";
    pub const GE: &str = ">=";
}

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
        // Both operands promote, so bool⊕bool is int-valued too — matching the
        // mixed forms below and the bitwise ops, which already allow it.
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
        OpRule {
            operands: &[Type::Bool, Type::Bool],
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
    // Object operands (players, entities, bricks…) don't coerce directly to an
    // int on a math gate. Accept them here; `lower_binop` routes an object
    // operand through `(obj || false)` so it still reduces to an int the gate
    // takes — i.e. `1 + player` lowers to `add(1, or(player, false))`.
    const OBJECT_TYPES: &[Type] = &[
        Type::Entity,
        Type::Controller,
        Type::Character,
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
        Bool, Int, Float, Exec, String, Entity, Controller, Character,
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
        Int, Float, Bool, String, Entity, Controller, Character,
    ];
    // Equality (`==`/`!=`) additionally compares the composite value variants by
    // content: the game's CompareEqual/CompareNotEqual gates are certified for
    // same-type vector/rotator/quat/color operands (`data/gate_semantics.json`).
    // Ordering (`<`/`>`/`<=`/`>=`) stays scalar-only — those composites have no
    // certified ordering cases.
    const EQ_ONLY_TYPES: &[Type] = &[Vector, Rotator, Quat, Color];
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
    if matches!(op, "==" | "!=") {
        for t in EQ_ONLY_TYPES {
            rules.push(OpRule {
                operands: Box::leak(Box::new([t.clone(), t.clone()])),
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
                Bool, Int, Float, Exec, String, Entity, Controller, Character,
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
                Character,
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
    // `Type::Opaque` (an `Opaque(...)` probe result) is a wildcard: it
    // matches whatever concrete type the rule expects at that operand
    // position, so operators still resolve to a real gate instead of
    // erroring on an operand the typechecker deliberately can't narrow.
    //
    // NOTE: `Type::Any` deliberately does *not* get this treatment. `Any`
    // is the codebase's generic unknown/error-fallback type (~150
    // producers: void array methods like `arr.clear()`, unresolved
    // namespace calls, dynamic-access fallbacks) — treating it as an
    // operator wildcard would silently defeat WS004 for all of them
    // (e.g. `arr.clear() + 3` would compile into a broken circuit instead
    // of erroring). Only the dedicated `Opaque` type gets the wildcard.
    if matches!(want, Type::Opaque) || matches!(got, Type::Opaque) {
        return true;
    }
    std::mem::discriminant(want) == std::mem::discriminant(got)
}

/// Resolve `op` given operand types. Picks the first matching rule.
/// Numeric promotion is handled explicitly by rule order (float-first
/// for mixed-type arithmetic).
pub fn resolve_op(op: &str, arg_types: &[Type]) -> Option<&'static OpRule> {
    let arity = arg_types.len() as u8;
    // Unary `-` is keyed `-u` in the table (binary `-` is subtract). Reconcile the
    // AST spelling here, at the single resolution point, so every caller resolves
    // negate rather than only the ones that remember to remap. A `-x` inside a
    // generic mod otherwise resolved to nothing and fell through to a stale
    // fallback type: a silent miscompile.
    let op = if op == "-" && arity == 1 { "-u" } else { op };
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
mod tests;
