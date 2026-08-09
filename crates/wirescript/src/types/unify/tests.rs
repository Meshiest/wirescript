    use super::*;

    #[test]
    fn int_and_float_promote() {
        assert_eq!(unify_glb(&Type::Int, &Type::Float), Some(Type::Float));
    }
    #[test]
    fn bool_and_int_promote() {
        assert_eq!(unify_glb(&Type::Bool, &Type::Int), Some(Type::Int));
    }
    #[test]
    fn any_sides_propagate() {
        assert_eq!(unify_glb(&Type::Any, &Type::Int), Some(Type::Int));
        assert_eq!(unify_glb(&Type::Float, &Type::Any), Some(Type::Float));
    }
    #[test]
    fn opaque_sides_propagate_like_any() {
        assert_eq!(unify_glb(&Type::Opaque, &Type::Int), Some(Type::Int));
        assert_eq!(unify_glb(&Type::Float, &Type::Opaque), Some(Type::Float));
    }
    #[test]
    fn unrelated_returns_none() {
        assert!(unify_glb(&Type::String, &Type::Vector).is_none());
    }
