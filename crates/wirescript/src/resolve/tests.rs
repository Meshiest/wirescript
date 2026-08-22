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
        let loader = mem(&[("utils.ws", "mod double(x: int) -> int { return x * 2 }")]);
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
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "double"))
        );
    }

    #[test]
    fn imported_file_parse_error_surfaces() {
        // A parse error in an imported module must NOT be swallowed — it used to
        // compile clean while the identical source errored as an entry file.
        let loader = mem(&[("broken.ws", "array xs: int[]")]);
        let r = resolve(r#"import "broken""#, "main.ws", &loader);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "an imported file's parse error must surface, got: {:?}",
            r.diagnostics
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
    fn import_used_in_event_config_not_unused() {
        // `on Clock(interval = TICK)` reads TICK in the handler's CONFIG args,
        // which the usage scan skipped — it only walked the handler body — so a
        // constant used ONLY to configure the event gate looked unused and
        // warned WS014. Inlining the literal to silence it is the wrong fix: the
        // value IS consumed.
        let loader = mem(&[("lib.ws", "let TICK = 0.15")]);
        let r = resolve(
            "import { TICK } from \"lib\"\non Clock(interval = TICK) { }",
            "main.ws",
            &loader,
        );
        let ws014: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS014").collect();
        assert!(
            ws014.is_empty(),
            "an import read in an event config arg must not be reported unused: {:?}",
            ws014
        );
    }

    #[test]
    fn import_used_in_event_config_inside_chip_not_unused() {
        // Same as above, but the handler is nested in a chip body, which
        // reaches the STATEMENT arm of the scan rather than the top-level one.
        // That arm walked only the body, so a channel constant used by a
        // handler inside a chip warned while the identical handler at module
        // level did not. Organize Imports acts on WS014, so the fallout was a
        // deleted import and a handler left naming nothing.
        let loader = mem(&[("lib.ws", "const CH = \"chan.a\"")]);
        let r = resolve(
            "import { CH } from \"lib\"\n\
             chip Inner() -> (n: int) {\n\
             var m: int = 0\n\
             on CustomEvent(CH) -> (v: int) { m = v }\n\
             out n: int = m\n\
             }\n\
             let i = Inner()\n\
             out n: int = i.n",
            "main.ws",
            &loader,
        );
        let ws014: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS014").collect();
        assert!(
            ws014.is_empty(),
            "an import read in an event config arg inside a chip must not be reported unused: {:?}",
            ws014
        );
    }

    #[test]
    fn genuinely_unused_import_still_warns_alongside_chip_handler() {
        // The wider usage scan above must still catch a genuinely unused
        // import: UNUSED is imported and never read anywhere.
        let loader = mem(&[("lib.ws", "const CH = \"chan.a\"\nconst UNUSED = 5")]);
        let r = resolve(
            "import { CH, UNUSED } from \"lib\"\n\
             chip Inner() -> (n: int) {\n\
             var m: int = 0\n\
             on CustomEvent(CH) -> (v: int) { m = v }\n\
             out n: int = m\n\
             }\n\
             let i = Inner()\n\
             out n: int = i.n",
            "main.ws",
            &loader,
        );
        let ws014: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS014").collect();
        assert_eq!(
            ws014.len(),
            1,
            "exactly the unused import should warn: {:?}",
            ws014
        );
        assert!(
            ws014[0].message.contains("UNUSED"),
            "the warning should name UNUSED: {:?}",
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
        let tc = crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
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

    /// `imported_files` must list every file a resolve pulled in, TRANSITIVELY,
    /// keyed the same way `canonical_path` keys them — the LSP compares a
    /// changed file's canonical path against this to decide which open
    /// documents still need re-analysis.
    #[test]
    fn imported_files_lists_the_transitive_import_set() {
        let loader = mem(&[
            ("mid.ws", "import { deep } from \"deep\"\nlet mid = deep + 1"),
            ("deep.ws", "let deep = 7"),
        ]);
        let r = resolve("import { mid } from \"mid\"\nout o = mid", "main.ws", &loader);
        assert!(r.diagnostics.is_empty(), "resolve diags: {:?}", r.diagnostics);
        let canon_mid = loader.canonical_path("mid", "main.ws");
        let canon_deep = loader.canonical_path("deep", "mid.ws");
        assert!(
            r.imported_files.contains(&canon_mid),
            "direct import missing from {:?}",
            r.imported_files
        );
        assert!(
            r.imported_files.contains(&canon_deep),
            "TRANSITIVE import missing from {:?} — an edit to it must still \
             re-analyze the importer",
            r.imported_files
        );
    }

    /// Resolve imports, then type-check the result — the errors a user sees.
    fn check(main: &str, files: &[(&str, &str)]) -> Vec<crate::diagnostic::Diagnostic> {
        let r = resolve(main, "main.ws", &mem(files));
        let tc =
            crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        r.diagnostics
            .into_iter()
            .chain(tc.diagnostics)
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect()
    }

    /// An alias body naming another alias (`type Rect = { a: Point, … }`) has
    /// to expand all the way down. A namespace import declares the module's
    /// aliases to the importer only as `Ns.Rect`, so a bare `Point` left inside
    /// the body resolved to nothing and the field silently typed `any` — every
    /// use of it then failed with "no overload for '==' on Any, Float".
    #[test]
    fn nested_alias_bodies_expand_through_a_namespace_import() {
        let lib = "type Point = { x: float, y: float }\n\
                   type Rect = { a: Point, b: Point }\n\
                   let corner: Point = { x: 1.0, y: 2.0 }\n\
                   let rect: Rect = { a: corner, b: corner }\n\
                   mod mk(p: Point) -> (r: Rect) { return { a: p, b: p } }";
        let errs = check(
            "import * as o from \"lib\"\n\
             let a = o.rect.a.x == 1.0\n\
             let b = o.mk({ x: 1.0, y: 2.0 }).b.y == 2.0",
            &[("lib.ws", lib)],
        );
        assert!(errs.is_empty(), "expected no errors, got {errs:?}");
    }

    /// A namespace member with no annotation used to type as `any` unless its
    /// initializer was a bare literal, and a destructuring `let` was not
    /// indexed at all — so `ns.origin.x` read as `any` even though lowering
    /// resolved the field and emitted the right value.
    #[test]
    fn unannotated_and_destructured_namespace_members_keep_their_types() {
        let lib = "let origin = { x: 1.0, y: 2.0 }\n\
                   let shifted = { ...origin, y: 9.0 }\n\
                   let ox = origin.x\n\
                   let { x, y } = origin";
        let errs = check(
            "import * as o from \"lib\"\n\
             let a = o.origin.x == 1.0\n\
             let b = o.shifted.y == 9.0\n\
             let c = o.ox == 1.0\n\
             let d = o.x == 1.0\n\
             let e = o.y == 2.0",
            &[("lib.ws", lib)],
        );
        assert!(errs.is_empty(), "expected no errors, got {errs:?}");
    }

    /// Expansion substitutes alias bodies into one another, so a
    /// self-referential or mutually recursive alias has to stop rather than
    /// expand forever. Reaching the assertions at all is most of the test:
    /// without the cycle guard this hangs or overflows the stack first.
    ///
    /// The recursive field is left unexpanded and so still fails to resolve in
    /// the importing module (WS002). What must hold is that everything
    /// *around* the cycle still types.
    #[test]
    fn recursive_alias_expansion_terminates() {
        let lib = "type Node = { v: int, next: Node }\n\
                   type A = { b: B, n: int }\n\
                   type B = { a: A, m: int }\n\
                   mod useNode(n: Node) -> (v: int) { return n.v }\n\
                   mod useA(x: A) -> (v: int) { return x.n }";
        let r = resolve("import * as o from \"lib\"", "main.ws", &mem(&[("lib.ws", lib)]));

        // The param of a namespaced `mod`, by name.
        let param = |mod_name: &str| {
            r.ast
                .decls
                .iter()
                .find_map(|d| match d {
                    TopDecl::Namespace(ns) => Some(&ns.decls),
                    _ => None,
                })
                .expect("namespace")
                .iter()
                .find_map(|d| match d {
                    TopDecl::Chip(c) if c.name == mod_name => Some(c.inputs[0].typ.clone()),
                    _ => None,
                })
                .expect("mod")
        };
        let field = |t: &TypeExpr, name: &str| match t {
            TypeExpr::Record { fields, .. } => fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.typ.clone())
                .expect("field"),
            other => panic!("expected a record, got {other:?}"),
        };

        // One level of `Node`, then the cycle stops with the name intact.
        let node = param("useNode");
        assert!(matches!(field(&node, "v"), TypeExpr::Name { ref name, .. } if name == "int"));
        assert!(
            matches!(field(&node, "next"), TypeExpr::Name { ref name, .. } if name == "Node"),
            "recursive field should be left unexpanded, got {:?}",
            field(&node, "next")
        );

        // Mutual recursion: `A` → `B` → back to `A`, which stops.
        let a = param("useA");
        let b = field(&a, "b");
        assert!(matches!(field(&b, "m"), TypeExpr::Name { ref name, .. } if name == "int"));
        assert!(
            matches!(field(&b, "a"), TypeExpr::Name { ref name, .. } if name == "A"),
            "mutual cycle should stop at the repeated name, got {:?}",
            field(&b, "a")
        );
    }

    /// A file with no imports reports an empty set, so an unrelated open
    /// document is never re-analyzed on someone else's keystroke.
    #[test]
    fn imported_files_is_empty_without_imports() {
        let loader = mem(&[]);
        let r = resolve("let x = 1\nout o = x", "main.ws", &loader);
        assert!(
            r.imported_files.is_empty(),
            "expected no imports, got {:?}",
            r.imported_files
        );
    }
