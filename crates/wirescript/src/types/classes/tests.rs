    use super::*;
    use crate::ir::Type;
    #[test]
    fn class_memberships() {
        assert!(mask_contains(&class_mask("Scalar").unwrap(), &Type::Int));
        assert!(!mask_contains(&class_mask("Scalar").unwrap(), &Type::Vector));
        assert!(mask_contains(&class_mask("Numeric").unwrap(), &Type::Vector));
        assert!(!mask_contains(&class_mask("Numeric").unwrap(), &Type::Bool));
        assert!(mask_contains(&class_mask("Variant").unwrap(), &Type::Bool));
        assert!(!mask_contains(&class_mask("Variant").unwrap(), &Type::Zone));
        assert!(class_mask("Nope").is_none());
        // Variant is the full value-variant set.
        assert_eq!(class_mask("Variant").unwrap(), variant_mask());
    }
