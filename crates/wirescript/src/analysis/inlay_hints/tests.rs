    use super::*;
    use crate::resolve::{FsLoader, resolve};
    use crate::typecheck::typecheck;

    fn hints_for(source: &str) -> Vec<InlayHintInfo> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test", &crate::typecheck::CeSlotMap::default());
        collect_inlay_hints(source, &resolved.ast, &tc.type_of_expr, "test")
    }

    #[test]
    fn let_without_annotation_gets_hint() {
        let hints = hints_for("let x = 42");
        assert!(!hints.is_empty(), "should produce a type hint");
        assert!(
            hints[0].label.contains("int"),
            "should infer int, got {}",
            hints[0].label
        );
    }

    #[test]
    fn let_with_annotation_no_hint() {
        let hints = hints_for("let x: int = 42");
        assert!(hints.is_empty(), "should not hint when type is annotated");
    }

    #[test]
    fn let_bool_hint() {
        let hints = hints_for("let flag = true");
        assert!(!hints.is_empty());
        assert!(hints[0].label.contains("bool"), "got {}", hints[0].label);
    }

    #[test]
    fn let_float_hint() {
        let hints = hints_for("let x = 3.14");
        assert!(!hints.is_empty());
        assert!(hints[0].label.contains("float"), "got {}", hints[0].label);
    }

    #[test]
    fn let_string_hint() {
        let hints = hints_for("let s = \"hello\"");
        assert!(!hints.is_empty());
        assert!(hints[0].label.contains("string"), "got {}", hints[0].label);
    }

    #[test]
    fn let_expr_hint() {
        let hints = hints_for("let x = 1 + 2");
        assert!(!hints.is_empty());
        assert!(hints[0].label.contains("int"), "got {}", hints[0].label);
    }

    #[test]
    fn var_with_annotation_no_hint() {
        let hints = hints_for("var x: int = 0");
        let type_hints: Vec<_> = hints
            .iter()
            .filter(|h| h.kind == InlayHintKind::Type)
            .collect();
        assert!(type_hints.is_empty(), "should not hint var with annotation");
    }

    #[test]
    fn let_inside_handler() {
        let src = "in start: exec\non start { let x = 42 }";
        let hints = hints_for(src);
        assert!(!hints.is_empty(), "should hint let inside handler");
        assert!(hints[0].label.contains("int"), "got {}", hints[0].label);
    }

    #[test]
    fn let_inside_chip() {
        let src = "chip Foo(a: int) -> (r: int) {\n  let doubled = a + a\n  out r = doubled\n}";
        let hints = hints_for(src);
        assert!(!hints.is_empty(), "should hint let inside chip");
        assert!(hints[0].label.contains("int"), "got {}", hints[0].label);
    }

    #[test]
    fn tuple_shows_tuple_syntax() {
        let hints = hints_for("let pair = (42, true)");
        assert!(!hints.is_empty(), "should hint tuple");
        let label = &hints[0].label;
        assert!(
            label.contains("(") && label.contains("int") && label.contains("bool"),
            "should show tuple syntax, got {}",
            label
        );
        assert!(
            !label.contains("{"),
            "should not use record syntax, got {}",
            label
        );
    }

    #[test]
    fn multiple_lets_multiple_hints() {
        let src = "let a = 1\nlet b = true\nlet c = 3.14";
        let hints = hints_for(src);
        assert_eq!(
            hints.len(),
            3,
            "should produce 3 hints, got {}",
            hints.len()
        );
    }

    #[test]
    fn hint_position_is_after_name() {
        let hints = hints_for("let x = 42");
        assert!(!hints.is_empty());
        // "let x" — x ends at col 5, hint should be at col 5
        assert_eq!(hints[0].line, 0);
        assert!(
            hints[0].col >= 4 && hints[0].col <= 6,
            "hint col should be near end of 'x', got {}",
            hints[0].col
        );
    }
