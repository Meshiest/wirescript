    use super::*;

    fn mem(files: &[(&str, &str)]) -> MemLoader {
        MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn import_all() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "foo"))
        );
    }

    #[test]
    fn import_named() {
        let loader = mem(&[(
            "lib.ws",
            "mod foo(x: *int) { x = x + 1 }\nmod bar(x: *int) { x = x - 1 }",
        )]);
        let r = resolve(r#"import { foo } from "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "foo"))
        );
        assert!(
            !r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "bar"))
        );
    }

    #[test]
    fn import_alias() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import { foo as inc } from "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "inc"))
        );
    }

    #[test]
    fn import_namespace() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import * as myLib from "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Namespace(n) if n.name == "myLib"))
        );
    }

    #[test]
    fn circular_import_error() {
        let loader = mem(&[("a.ws", r#"import "b""#), ("b.ws", r#"import "a""#)]);
        let r = resolve(r#"import "a""#, "main.ws", &loader);
        assert!(r.diagnostics.iter().any(|d| d.message.contains("circular")));
    }

    #[test]
    fn missing_file_error() {
        let loader = mem(&[]);
        let r = resolve(r#"import "nonexistent""#, "main.ws", &loader);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("cannot resolve"))
        );
    }

    #[test]
    fn missing_symbol_error() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import { bar } from "lib""#, "main.ws", &loader);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("not found"))
        );
    }

    #[test]
    fn var_and_mod_both_importable() {
        let loader = mem(&[("lib.ws", "var x: int = 0\nmod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(r.ast.decls.iter().any(|d| matches!(d, TopDecl::Var(v) if v.name == "x")));
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "foo"))
        );
    }

    #[test]
    fn implicit_ws_extension() {
        let loader = mem(&[("utils.ws", "fn double(x: int) -> int = x * 2")]);
        let r = resolve(r#"import "utils""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Fn(f) if f.name == "double"))
        );
    }

    #[test]
    fn type_import_used_in_param_not_unused() {
        let loader = mem(&[(
            "types.ws",
            "type Cpu = { regs: int[], cpsr: *int }",
        )]);
        let r = resolve(
            "import { Cpu } from \"types\"\nmod foo(cpu: Cpu) { cpu.regs.push(0) }",
            "main.ws",
            &loader,
        );
        let ws014: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS014").collect();
        assert!(
            ws014.is_empty(),
            "type used in param annotation should not trigger unused import: {:?}",
            ws014
        );
    }

    #[test]
    fn import_var_alias_renames() {
        let loader = mem(&[("lib.ws", "var counter: int = 0")]);
        let r = resolve(
            "import { counter as cnt } from \"lib\"",
            "main.ws",
            &loader,
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}", r.diagnostics
        );
        assert!(
            r.ast.decls.iter().any(|d| matches!(d, TopDecl::Var(v) if v.name == "cnt")),
            "var should be renamed to 'cnt'"
        );
    }

    #[test]
    fn transitive_import_decls_precede_importing_files_own_decls() {
        // A file's own declarations must come AFTER the ones it imports, at
        // every level of the import graph — chips/mods register in source
        // order during lowering, so the reverse ordering makes a call to a
        // transitively imported helper a use-before-declaration (WS021).
        let loader = mem(&[
            ("util.ws", "mod helper(x: *int) { x = x + 1 }"),
            (
                "game.ws",
                "import \"util\"\nmod game_step(x: *int) { helper(x) }",
            ),
            ("container.ws", "import \"game\""),
        ]);
        let r = resolve(
            "import \"container\"\nvar n: int = 0\nin go: exec\non go { game_step(n) }",
            "main.ws",
            &loader,
        );
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        let pos = |name: &str| {
            r.ast
                .decls
                .iter()
                .position(|d| decl_name(d) == Some(name))
                .unwrap_or_else(|| panic!("'{name}' missing from resolved decls"))
        };
        assert!(
            pos("helper") < pos("game_step"),
            "'helper' must precede its caller 'game_step'; decls: {:?}",
            r.ast.decls.iter().filter_map(decl_name).collect::<Vec<_>>()
        );
        let tc = crate::typecheck::typecheck(&r.ast, "main.ws");
        assert!(
            !tc.diagnostics.iter().any(|d| d.code == "WS021"),
            "transitive import must not produce use-before-declaration: {:?}",
            tc.diagnostics
        );
    }

    #[test]
    fn type_alias_not_leaked_transitively() {
        let loader = mem(&[
            ("types.ws", "type Cpu = { regs: int[], cpsr: *int }"),
            ("cpu.ws", "import { Cpu } from \"types\"\nmod cpu_init(cpu: Cpu) { cpu.regs.push(0) }"),
        ]);
        let r = resolve(
            "import { cpu_init } from \"cpu\"",
            "main.ws",
            &loader,
        );
        let has_type_alias = r.ast.decls.iter().any(|d| matches!(d, TopDecl::TypeAlias(t) if t.name == "Cpu"));
        assert!(
            !has_type_alias,
            "TypeAlias 'Cpu' should NOT be pulled transitively into the importing file's AST"
        );
    }
