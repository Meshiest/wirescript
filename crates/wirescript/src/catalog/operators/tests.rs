    use super::*;

    #[test]
    fn add_int_int_returns_int() {
        let r = resolve_op("+", &[Type::Int, Type::Int]).unwrap();
        assert!(matches!(r.result, Type::Int));
        assert_eq!(r.gate_class, "BrickComponentType_WireGraph_Expr_MathAdd");
    }

    #[test]
    fn add_bool_bool_promotes_to_int() {
        // bool coerces to 0/1 on int ports, so bool⊕bool is int-valued -- same
        // as bool⊕int and the bitwise ops. Covers `(a && b) + (c && d)`.
        for op in ["+", "-", "*", "/", "%"] {
            let r = resolve_op(op, &[Type::Bool, Type::Bool])
                .unwrap_or_else(|| panic!("no rule for bool {op} bool"));
            assert!(matches!(r.result, Type::Int), "{op} bool/bool should be int");
        }
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
    fn negate_resolves_by_ast_spelling() {
        // The AST carries unary negate as "-" (same spelling as binary subtract).
        // resolve_op maps it to the table's "-u" key by arity, so every caller
        // resolves it, not only the ones that remember to remap (an un-remapped
        // `-x` in a generic mod otherwise fell through to a stale fallback type).
        let ri = resolve_op("-", &[Type::Int]).expect("unary - on int");
        assert!(matches!(ri.result, Type::Int));
        assert_eq!(ri.gate_class, "BrickComponentType_WireGraph_Expr_MathNegate");
        let rf = resolve_op("-", &[Type::Float]).expect("unary - on float");
        assert!(matches!(rf.result, Type::Float));
        // Binary "-" (arity 2) stays subtract; the remap is arity-gated.
        let sub = resolve_op("-", &[Type::Int, Type::Int]).expect("binary -");
        assert_eq!(sub.gate_class, "BrickComponentType_WireGraph_Expr_MathSubtract");
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
