    #[test]
    fn type_param_displays_as_its_name() {
        assert_eq!(crate::ir::Type::Param("T".into()).to_string(), "T");
        assert_eq!(
            crate::ir::Type::Array(Box::new(crate::ir::Type::Param("T".into()))).to_string(),
            "T[]"
        );
    }
