    use super::*;

    fn parse_ok(src: &str) -> Script {
        let r = parse(src, "test");
        assert!(
            r.diagnostics.is_empty(),
            "unexpected diags: {:?}",
            r.diagnostics
        );
        r.ast
    }

    #[test]
    fn empty_source_parses() {
        let s = parse_ok("");
        assert!(s.decls.is_empty());
    }

    #[test]
    fn out_binding_rejects_trailing_tokens() {
        // `out aw(wa)` reads like an anonymous output of a call, but an output
        // port is always `out NAME` / `out NAME = expr` — trailing tokens like
        // `(wa)` must not be silently dropped and re-parsed as their own decl.
        let r = parse("let wa = (1, 2)\nout aw(wa)", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("out")),
            "trailing tokens after an output port name must be reported: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn out_binding_forms_still_parse() {
        parse_ok("out foo");
        parse_ok("out foo: int");
        parse_ok("out foo = 1 + 2");
        parse_ok("out foo: int = 3");
        parse_ok("chip {\n  out foo = 1\n}");
    }

    #[test]
    fn top_doc_block_before_blank_is_module_doc_not_merged() {
        // A `///` block at the top, separated from the first decl by a blank
        // line (or a `//` comment), is the module doc — it must NOT merge into
        // the following declaration's own doc comment.
        let src = "/// mod line 1\n/// mod line 2\n\n//\n\n/// chip doc\nchip {\n  var x: int = 0\n}";
        let r = parse(src, "test");
        let md = r.ast.module_doc.as_deref().unwrap_or("<none>");
        assert!(
            md.contains("mod line 1") && md.contains("mod line 2"),
            "module doc should hold the top block: {md:?}"
        );
        assert!(!md.contains("chip doc"), "module doc must not merge the chip doc: {md:?}");
        assert!(
            r.doc_comments.values().any(|d| d == "chip doc"),
            "chip doc must remain its own comment: {:?}",
            r.doc_comments
        );
        assert!(
            r.doc_comments.values().all(|d| !d.contains("mod line")),
            "the module doc must not attach to a declaration: {:?}",
            r.doc_comments
        );
    }

    #[test]
    fn top_doc_block_adjacent_to_decl_is_decl_doc_not_module() {
        // No blank line → the block documents the first decl, so
        // `module_doc` is None and the first decl carries it.
        let src = "/// first decl doc\nvar x: int = 0";
        let r = parse(src, "test");
        assert!(r.ast.module_doc.is_none(), "adjacent block is not a module doc: {:?}", r.ast.module_doc);
        assert!(
            r.doc_comments.values().any(|d| d == "first decl doc"),
            "adjacent block documents the decl: {:?}",
            r.doc_comments
        );
    }

    #[test]
    fn record_type_field_doc_comments_parse_and_store() {
        let r = parse(
            "type Point = {\n  /// the x coordinate\n  x: int,\n  /// the y coordinate\n  y: int,\n}",
            "test",
        );
        assert!(
            r.diagnostics.is_empty(),
            "record field doc comments should parse: {:?}",
            r.diagnostics
        );
        let docs: Vec<&String> = r.doc_comments.values().collect();
        assert!(docs.iter().any(|d| d.contains("the x coordinate")), "x doc missing: {docs:?}");
        assert!(docs.iter().any(|d| d.contains("the y coordinate")), "y doc missing: {docs:?}");
    }

    #[test]
    fn var_int_literal() {
        let s = parse_ok("var x = 42");
        assert_eq!(s.decls.len(), 1);
        match &s.decls[0] {
            TopDecl::Var(v) => {
                assert_eq!(v.name, "x");
                assert!(v.typ.is_none());
                match &v.init {
                    Some(Expr::IntLit { value, .. }) => assert_eq!(*value, 42),
                    _ => panic!("expected IntLit init"),
                }
            }
            _ => panic!("expected Var decl"),
        }
    }

    #[test]
    fn var_typed() {
        let s = parse_ok("var x: int = 1");
        match &s.decls[0] {
            TopDecl::Var(v) => match &v.typ {
                Some(TypeExpr::Name { name, .. }) => assert_eq!(name, "int"),
                _ => panic!("expected typed VarDecl"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn in_out_decls() {
        let s = parse_ok("in trigger: exec\nout count = 3");
        assert!(matches!(s.decls[0], TopDecl::In(_)));
        assert!(matches!(s.decls[1], TopDecl::Out(_)));
    }

    #[test]
    fn binary_precedence() {
        let s = parse_ok("var x = a + b * c");
        match &s.decls[0] {
            TopDecl::Var(v) => match v.init.as_ref().unwrap() {
                Expr::BinOp { op, right, .. } => {
                    assert_eq!(op, "+");
                    match right.as_ref() {
                        Expr::BinOp { op, .. } => assert_eq!(op, "*"),
                        _ => panic!("expected right = BinOp *"),
                    }
                }
                _ => panic!("expected BinOp +"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn handler_with_param() {
        let s = parse_ok("on CharacterDied() -> (char) { emit died }");
        match &s.decls[0] {
            TopDecl::Handler(h) => {
                assert_eq!(h.params.len(), 1);
                assert_eq!(h.params[0].name, "char");
                match &h.trigger {
                    Trigger::Ident { name, .. } => assert_eq!(name, "CharacterDied"),
                    _ => panic!("expected TrigIdent"),
                }
            }
            _ => panic!("expected Handler"),
        }
    }

    #[test]
    fn event_trigger_requires_parens() {
        // An event trigger is a CALL -- `on RoundStart { }` (no
        // parens) must error, steering to `on RoundStart() { }`.
        let r = crate::parser::parse("on RoundStart { }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("must be called") || d.message.contains("()")),
            "no-parens event trigger should error: {:?}",
            r.diagnostics
        );
        assert!(crate::parser::parse("on RoundStart() { }", "test")
            .diagnostics
            .is_empty());
        // a non-event value trigger is unaffected
        assert!(
            crate::parser::parse("var x: int = 0\non x { }", "test")
                .diagnostics
                .is_empty()
        );
    }

    #[test]
    fn let_capture_event_requires_parens() {
        // The captured-event form calls the event too: `let x = on RoundStart()`.
        assert!(crate::parser::parse("let s = on RoundStart()\non s { }", "test")
            .diagnostics
            .is_empty());
        // The bare no-parens capture must error, matching the handler form.
        let r = crate::parser::parse("let s = on RoundStart\non s { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("must be called")),
            "no-parens event capture should error: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn handler_expr_trigger_desugars_to_let_plus_handler() {
        // `on a && b { x = 1 }` should desugar into:
        //   let _on_expr_0 = a && b
        //   on _on_expr_0 { x = 1 }
        let src = "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }";
        let s = parse_ok(src);
        // Expected: In(a), In(b), Var(x), Let(_on_expr_0), Handler(_on_expr_0)
        assert_eq!(
            s.decls.len(),
            5,
            "decls: {:?}",
            s.decls.iter().map(|d| d.range()).collect::<Vec<_>>()
        );
        match &s.decls[3] {
            TopDecl::Let(l) => match &l.binding {
                LetBinding::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident binding"),
            },
            d => panic!("expected Let, got {:?}", d),
        }
        match &s.decls[4] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident trigger"),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn handler_call_trigger_desugars_to_let_plus_handler() {
        // `on ServerUptime() { … }` — a builtin CALL used as a trigger — desugars
        // to `let _on_expr_0 = ServerUptime()` + `on _on_expr_0 { … }`, exactly
        // like the value-capture pattern `let t = ServerUptime(); on t`. This must
        // NOT be mistaken for the event-with-args form (`on Clock(...)`).
        let src = "on ServerUptime() { BroadcastChatMessage(\"tick\") }";
        let s = parse_ok(src);
        // Expected: Let(_on_expr_0 = ServerUptime()), Handler(_on_expr_0).
        assert_eq!(s.decls.len(), 2, "decls: {:?}", s.decls);
        match &s.decls[0] {
            TopDecl::Let(l) => match &l.binding {
                LetBinding::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident binding"),
            },
            d => panic!("expected Let, got {:?}", d),
        }
        match &s.decls[1] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident trigger"),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn handler_event_with_args_is_not_a_call_trigger() {
        // `on Clock(enabled = true)` stays a plain event handler — its name IS an
        // event, so the call-trigger path must not hijack it into an expr trigger.
        let src = "on Clock(enabled = true) { }";
        let s = parse_ok(src);
        assert_eq!(s.decls.len(), 1, "no synthetic let for an event: {:?}", s.decls);
        match &s.decls[0] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "Clock"),
                _ => panic!("expected Ident trigger"),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn handler_method_call_trigger_desugars_to_let_plus_handler() {
        // `on a.Dot(b) > 0.0 { … }` — a method call in the trigger head. The
        // trailing `(` of the call must NOT be read as event-config args; the
        // whole thing is an expression trigger, desugared to a synthetic let.
        let src = "in a: vector\nin b: vector\nvar x: int = 0\non a.Dot(b) > 0.0 { x = 1 }";
        let s = parse_ok(src);
        // In(a), In(b), Var(x), Let(_on_expr_0), Handler(_on_expr_0)
        assert_eq!(s.decls.len(), 5, "decls: {:?}", s.decls);
        match &s.decls[3] {
            TopDecl::Let(l) => match &l.binding {
                LetBinding::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident binding"),
            },
            d => panic!("expected Let, got {:?}", d),
        }
        match &s.decls[4] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident trigger"),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn handler_bare_field_trigger_stays_plain() {
        // A bare `.field` head (`on split.Jump { }`) is a field trigger, NOT a
        // value expression — the method-call classifier must not desugar it.
        let src = "on split.Jump { }";
        let s = parse_ok(src);
        assert_eq!(s.decls.len(), 1, "no synthetic let for a field trigger: {:?}", s.decls);
        match &s.decls[0] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Field { obj, field, .. } => {
                    assert_eq!(obj, "split");
                    assert_eq!(field, "Jump");
                }
                t => panic!("expected Field trigger, got {:?}", t),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn simple_counter_program() {
        let src = "in tick: exec\nvar n: int = 0\non tick {\n  n = n + 1\n}\nout count = n";
        let s = parse_ok(src);
        assert_eq!(s.decls.len(), 4);
    }

    #[test]
    fn call_with_kwargs() {
        let s = parse_ok("var x = vec(x = 1, y = 2, z = 3)");
        match &s.decls[0] {
            TopDecl::Var(v) => match v.init.as_ref().unwrap() {
                Expr::Call { args, .. } => {
                    assert_eq!(args.len(), 3);
                    matches!(&args[0], CallArg::Named { name, .. } if name == "x");
                }
                _ => panic!("expected Call"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn hex_literal() {
        let s = parse_ok("var x = 0xff");
        match &s.decls[0] {
            TopDecl::Var(v) => match v.init.as_ref().unwrap() {
                Expr::IntLit { value, .. } => assert_eq!(*value, 255),
                _ => panic!("expected IntLit"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn array_and_map_decl_keywords_are_rejected() {
        // The `array`/`map` declaration keywords were removed in favor of
        // `var NAME: T[]` / `var NAME: Map<K,V>` (identical storage). Using them
        // is a parse error pointing at the `var` replacement.
        let ra = crate::parser::parse("array xs: int[]", "test");
        assert!(
            ra.diagnostics
                .iter()
                .any(|d| d.message.contains("`array` declarations have been removed")),
            "array decl must be rejected: {:?}",
            ra.diagnostics
        );
        let rm = crate::parser::parse("map m: Map<string, int>", "test");
        assert!(
            rm.diagnostics
                .iter()
                .any(|d| d.message.contains("`map` declarations have been removed")),
            "map decl must be rejected: {:?}",
            rm.diagnostics
        );
        let rv = crate::parser::parse("var xs: int[]", "test");
        assert!(rv.diagnostics.is_empty(), "var array: {:?}", rv.diagnostics);
        match &rv.ast.decls[0] {
            TopDecl::Var(v) => assert_eq!(v.name, "xs"),
            d => panic!("expected Var, got {:?}", d),
        }
        let rvm = crate::parser::parse("var m: Map<string, int>", "test");
        assert!(rvm.diagnostics.is_empty(), "var map: {:?}", rvm.diagnostics);
    }

    #[test]
    fn parse_chip_decl() {
        let src = "chip Counter(bump: exec, reset: exec) -> (value: int, overflow: bool) {\n  var n: int = 0\n}";
        let r = crate::parser::parse(src, "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert_eq!(c.name, "Counter");
                assert_eq!(c.inputs.len(), 2);
                assert_eq!(c.outputs.len(), 2);
                assert_eq!(c.outputs[0].name, "value");
            }
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn fn_decl_is_removed() {
        // The `fn` declaration form was removed in favor of
        // `mod NAME(params) -> T { return <expr> }`. Using it is a parse error
        // (parsing still recovers the rest of the file).
        let r = crate::parser::parse("fn add(a: int, b: int) -> int = a + b", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("`fn` declarations have been removed")),
            "fn decl must be rejected: {:?}",
            r.diagnostics
        );
        let rm = crate::parser::parse("mod add(a: int, b: int) -> int { return a + b }", "test");
        assert!(rm.diagnostics.is_empty(), "mod replacement: {:?}", rm.diagnostics);
    }

    #[test]
    fn parse_anonymous_output_defaults_to_underscore() {
        let r = crate::parser::parse("chip Double(x: int) -> int { out _ = x * 2 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert_eq!(c.outputs.len(), 1);
                assert_eq!(c.outputs[0].name, "_");
            }
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn parse_mod_with_output() {
        let r = crate::parser::parse("mod clamp(v: int) -> (r: int) { return v }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert!(c.inline);
                assert_eq!(c.outputs.len(), 1);
                assert_eq!(c.outputs[0].name, "r");
            }
            d => panic!("expected Chip (mod), got {:?}", d),
        }
    }

    #[test]
    fn parse_mod_anonymous_output_defaults_to_underscore() {
        let r = crate::parser::parse(
            "mod abs(v: int) -> int { if v < 0 { return 0 - v } return v }",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert!(c.inline);
                assert_eq!(c.outputs.len(), 1);
                assert_eq!(c.outputs[0].name, "_");
            }
            d => panic!("expected Chip (mod), got {:?}", d),
        }
    }

    #[test]
    fn parses_generic_decl_headers() {
        use crate::ast::TopDecl;
        let mod_of = |s: &str| {
            crate::parser::parse(s, "t").ast.decls.into_iter()
                .find_map(|d| if let TopDecl::Chip(c) = d { Some(c) } else { None }).expect("a mod/chip")
        };
        let c = mod_of("mod pick<T>(c: bool, a: T, b: T) -> T { return a }\n");
        assert_eq!(c.type_params.len(), 1);
        assert_eq!(c.type_params[0].name, "T");
        assert!(c.type_params[0].bound.is_none());

        let c2 = mod_of("mod clamp<T: Numeric>(v: T) -> T { return v }\n");
        assert_eq!(c2.type_params.len(), 1);
        assert!(c2.type_params[0].bound.is_some(), "T: Numeric has a bound");

        let c3 = mod_of("mod two<T, U>(a: T, b: U) { }\n");
        assert_eq!(c3.type_params.len(), 2);
        assert_eq!(c3.type_params[1].name, "U");

        let ast = crate::parser::parse("type Pair<T> = { a: T, b: T }\n", "t").ast;
        let ta = ast.decls.iter().find_map(|d| if let TopDecl::TypeAlias(t) = d { Some(t) } else { None }).expect("alias");
        assert_eq!(ta.type_params.len(), 1);
        assert_eq!(ta.type_params[0].name, "T");

        for s in ["mod pick<T>(a: T) -> T { return a }\n", "mod plain(a: int) -> int { return a }\n",
                  "type Grid<T> = T[]\n"] {
            assert!(crate::parser::parse(s, "t").diagnostics.iter()
                .all(|d| d.severity != crate::diagnostic::Severity::Error), "should parse cleanly: {s}");
        }
    }

    #[test]
    fn parse_return_value() {
        let r = crate::parser::parse("mod foo() -> int { return 42 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => match &c.body.stmts[0] {
                Stmt::Return { value: Some(_), .. } => {}
                s => panic!("expected Return with value, got {:?}", s),
            },
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn parse_return_no_value() {
        let r = crate::parser::parse("mod foo(x: *int) { return }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => match &c.body.stmts[0] {
                Stmt::Return { value: None, .. } => {}
                s => panic!("expected Return without value, got {:?}", s),
            },
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn side_annotation_same_line_and_line_above() {
        let r = parse("@left in a: bool\n@right\nout b = a", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(i.side, Some(PortSide::Left));
        let TopDecl::Out(o) = &r.ast.decls[1] else {
            panic!("decl 1: {:?}", r.ast.decls[1])
        };
        assert_eq!(o.side, Some(PortSide::Right));
    }

    #[test]
    fn unannotated_ports_have_no_side() {
        let r = parse("in a: bool\nout b = a", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!()
        };
        assert_eq!(i.side, None);
    }

    #[test]
    fn unknown_annotation_word_errors() {
        let r = parse("@middle in a: bool", "test");
        assert_eq!(r.diagnostics.len(), 1, "diags: {:?}", r.diagnostics);
        assert!(r.diagnostics[0].message.contains("unknown annotation '@middle'"));
        // Declaration still parses, just without a side.
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!()
        };
        assert_eq!(i.side, None);
    }

    #[test]
    fn annotation_before_non_port_decl_errors() {
        let r = parse("@left var x: int = 1", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("must be followed by an 'in', 'out', or chip declaration")),
            "diags: {:?}",
            r.diagnostics
        );
        // The var itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::Var(_)));
    }

    #[test]
    fn invisible_before_non_port_decl_errors() {
        // `@invisible` must participate in the bare-@nofold validity guard
        // exactly like `@closed`: it is a port annotation, so pairing it with
        // `@nofold` before a plain `var` is not the special bare-@nofold case
        // and must be diagnosed (not silently discarded).
        let r = parse("@invisible @nofold var x: int = 0", "test");
        assert!(
            r.diagnostics.iter().any(|d| d
                .message
                .contains("must be followed by an 'in', 'out', or chip declaration")),
            "diags: {:?}",
            r.diagnostics
        );
        // The var itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::Var(_)));
    }

    #[test]
    fn duplicate_annotation_errors_first_wins() {
        let r = parse("@left @right in a: bool", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("duplicate side annotation")),
            "diags: {:?}",
            r.diagnostics
        );
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!()
        };
        assert_eq!(i.side, Some(PortSide::Left));
    }

    #[test]
    fn annotation_parses_at_statement_level() {
        // Inside a chip body it must PARSE (lowering rejects it later with WS023).
        let r = parse("chip { @left in a: bool }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else {
            panic!()
        };
        let Stmt::In(i) = &ac.body.stmts[0] else {
            panic!("stmt: {:?}", ac.body.stmts[0])
        };
        assert_eq!(i.side, Some(PortSide::Left));
    }

    #[test]
    fn label_annotation_on_anon_chip() {
        let r = parse("@label(\"Score Tracker\") chip { var a: int = 0 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(ac.label.as_deref(), Some("Score Tracker"));
        assert!(!ac.closed);
    }

    #[test]
    fn closed_annotation_on_named_chip_and_chip_forms() {
        let r = parse(
            "@closed chip Foo(x: int) { }\n\
             @closed chip on t { }\n\
             @closed chip let a = 1\n\
             @label(\"consts\") @closed chip { }",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Chip(c) = &r.ast.decls[0] else { panic!() };
        assert!(c.closed);
        for i in 1..=2 {
            let TopDecl::AnonChip(ac) = &r.ast.decls[i] else {
                panic!("decl {i}: {:?}", r.ast.decls[i])
            };
            assert!(ac.closed, "decl {i} should be closed");
        }
        let TopDecl::AnonChip(ac) = &r.ast.decls[3] else { panic!() };
        assert!(ac.closed);
        assert_eq!(ac.label.as_deref(), Some("consts"));
    }

    #[test]
    fn label_stacks_with_side_on_ports_any_order() {
        let r = parse(
            "@left @label(\"Fire!\") in t: exec\n@label(\"Total\") @right out s = t",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::In(i) = &r.ast.decls[0] else { panic!() };
        assert_eq!(i.side, Some(PortSide::Left));
        assert_eq!(i.label.as_deref(), Some("Fire!"));
        let TopDecl::Out(o) = &r.ast.decls[1] else { panic!() };
        assert_eq!(o.side, Some(PortSide::Right));
        assert_eq!(o.label.as_deref(), Some("Total"));
    }

    #[test]
    fn closed_on_port_errors() {
        let r = parse("@closed in t: exec", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@closed is not allowed on 'in'/'out'")),
            "diags: {:?}",
            r.diagnostics
        );
        // The port itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::In(_)));
    }

    #[test]
    fn label_argument_errors() {
        let r = parse("@label chip { }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@label requires a string argument")),
            "diags: {:?}",
            r.diagnostics
        );
        let r = parse("@label(\"\") chip { }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@label text must not be empty")),
            "diags: {:?}",
            r.diagnostics
        );
        let r = parse("@label(\"a\") @label(\"b\") chip { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("duplicate @label")),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn label_expr_annotation_parses_general_expressions() {
        // Anything besides a bare string literal parses as a general
        // expression, stored separately from the string form. Const-folding
        // it to display text happens at lowering (typecheck rejects a
        // non-constant one).
        let r = parse("@label(1 + 2) chip { }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert!(ac.label.is_none());
        assert!(matches!(ac.label_expr, Some(Expr::BinOp { .. })));
    }

    #[test]
    fn label_expr_and_label_string_are_mutually_exclusive() {
        let r = parse("@label(\"a\") @label(x) chip { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("duplicate @label")),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn label_annotation_is_allowed_on_var() {
        let r = parse("@label(\"HP\") var hp: int = 0", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Var(v) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(v.label.as_deref(), Some("HP"));
    }

    #[test]
    fn label_and_nofold_stack_on_var() {
        let r = parse("@label(\"HP\") @nofold var hp: int = 0", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Var(v) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(v.label.as_deref(), Some("HP"));
        assert!(v.no_fold);
    }

    #[test]
    fn closed_open_chip_contradiction_errors() {
        let r = parse("@closed open chip { var a: int = 0 }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@closed cannot be combined with 'open chip'")),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn annotation_on_mod_errors() {
        let r = parse("@label(\"x\") mod inc(v: int) -> int { return v + 1 }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("annotations are not allowed on 'mod'")),
            "diags: {:?}",
            r.diagnostics
        );
        // The mod itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::Chip(c) if c.inline));
    }

    #[test]
    fn unknown_annotation_lists_all_words() {
        let r = parse("@middle in a: bool", "test");
        assert!(
            r.diagnostics[0].message.contains(
                "expected @left, @right, @top, @bottom, @label, @closed, @invisible, or @nofold"
            ),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn open_chip_still_parses_as_noop() {
        let r = parse("open chip { var a: int = 0 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else { panic!() };
        assert!(ac.open);
        assert!(!ac.closed);
    }

    #[test]
    fn chip_annotations_parse_at_statement_level() {
        let r = parse("chip Outer(x: int) { @closed chip { var a: int = 0 } }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Chip(c) = &r.ast.decls[0] else { panic!() };
        let Stmt::AnonChip(ac) = &c.body.stmts[0] else {
            panic!("stmt: {:?}", c.body.stmts[0])
        };
        assert!(ac.closed);
    }

    #[test]
    fn module_layout_annotation_sets_flag() {
        let p = crate::parser::parse("@layout(\"code\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_layout_annotation_mixes_with_fold_run() {
        let p = crate::parser::parse("@fold\n@layout(\"code\")\n\nvar x: int = 0\n", "t");
        assert!(p.ast.fold);
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_layout_annotation_selects_the_cube() {
        let p = crate::parser::parse("@layout(\"cube\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// No engine outranks another, so the file is told which one survived
    /// rather than silently getting whichever the fold order happened to keep.
    #[test]
    fn two_layout_annotations_warn_and_the_last_wins() {
        let p =
            crate::parser::parse("@layout(\"code\")\n@layout(\"cube\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube));
        assert_eq!(
            p.diagnostics.iter().filter(|d| d.message.contains("set twice")).count(),
            1,
            "{:?}",
            p.diagnostics
        );
    }

    #[test]
    fn var_initializer_without_equals_is_an_error() {
        // `var x: type LITERAL` (missing `=`) must not silently drop the value.
        for src in ["var test: string \"hello\"\n", "var n: int 5\n"] {
            let p = crate::parser::parse(src, "t");
            assert!(
                p.diagnostics
                    .iter()
                    .any(|d| d.message.contains("missing `=`")),
                "missing `=` before an initializer must error; src {src:?} gave {:?}",
                p.diagnostics
            );
        }
    }

    #[test]
    fn var_without_initializer_is_allowed() {
        // A bare `var x: int` (declaration ends after the type) is valid.
        for src in ["var x: int\n", "var y: bool\nvar z: int = 0\n", "chip { var q: int }\n"] {
            let p = crate::parser::parse(src, "t");
            assert!(
                p.diagnostics.is_empty(),
                "an uninitialized var is valid; src {src:?} gave {:?}",
                p.diagnostics
            );
        }
    }

    #[test]
    fn module_label_blank_line_separated_labels_the_root() {
        // `@label(expr)` at the top of the file, separated from the first decl
        // by a blank line, is a MODULE-level label (root chip) — not attached
        // to the var below it.
        let p = crate::parser::parse("@label(title)\n\nvar title: string = \"hi\"\n", "t");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        assert!(
            p.ast.module_label.is_some(),
            "a blank-line-separated top @label is module-level"
        );
        match &p.ast.decls[0] {
            TopDecl::Var(v) => assert!(
                v.label.is_none() && v.label_expr.is_none(),
                "the var must NOT also carry the module @label"
            ),
            other => panic!("expected var, got {other:?}"),
        }
    }

    #[test]
    fn module_annotation_run_hands_off_to_module_label() {
        // `@invisible` directly above a blank-line-separated module `@label`
        // keeps BOTH: the run finishes (module stays invisible) and the `@label`
        // is claimed as the root label — no lost-annotation error.
        let p =
            crate::parser::parse("@invisible\n@label(title)\n\nvar title: string = \"hi\"\n", "t");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        assert!(
            p.ast.invisible,
            "@invisible must survive the hand-off to a following module @label"
        );
        assert!(
            p.ast.module_label.is_some(),
            "the @label is still module-level"
        );
    }

    #[test]
    fn attached_label_stays_decl_level() {
        // No blank line: `@label(expr)` attaches to the var below it (the
        // declaration-level self-label), and there is no module-level label.
        let p = crate::parser::parse("@label(title)\nvar title: string = \"hi\"\n", "t");
        assert!(
            p.ast.module_label.is_none(),
            "an attached top @label is NOT module-level"
        );
        match &p.ast.decls[0] {
            TopDecl::Var(v) => assert!(
                v.label_expr.is_some(),
                "the var carries the attached @label expression"
            ),
            other => panic!("expected var, got {other:?}"),
        }
    }

    #[test]
    fn repeating_one_layout_annotation_is_not_a_conflict() {
        let p =
            crate::parser::parse("@layout(\"cube\")\n@layout(\"cube\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// The diagnostic is generated from the accepted spellings, so a new
    /// engine cannot ship with an error message that omits it.
    #[test]
    fn unknown_layout_error_offers_every_accepted_name() {
        let p = crate::parser::parse("@layout(\"spiral\")\n\nvar x: int = 0\n", "t");
        let msg = &p.diagnostics[0].message;
        for (name, _) in crate::ast::LayoutName::ALL {
            assert!(msg.contains(name), "{name} missing from {msg:?}");
        }
    }

    #[test]
    fn unknown_layout_name_errors() {
        let p = crate::parser::parse("@layout(\"grid\")\n\nvar x: int = 0\n", "t");
        assert!(p.ast.layout.is_none());
        assert!(p.diagnostics.iter().any(|d| d.message.contains("unknown layout")));
        assert_eq!(p.diagnostics.len(), 1, "{:?}", p.diagnostics);
    }

    #[test]
    fn layout_without_argument_errors() {
        let p = crate::parser::parse("@layout\n\nvar x: int = 0\n", "t");
        assert!(p.ast.layout.is_none());
        assert!(p.diagnostics.iter().any(|d| d.message.contains("requires a string argument")));
        assert_eq!(p.diagnostics.len(), 1, "{:?}", p.diagnostics);
    }

    #[test]
    fn malformed_layout_argument_reports_one_diagnostic() {
        for src in [
            "@layout(\n\nvar x: int = 0\n",
            "@layout(5)\n\nvar x: int = 0\n",
            "@layout(\"code\"\n\nvar x: int = 0\n",
        ] {
            let p = crate::parser::parse(src, "t");
            assert!(p.ast.layout.is_none(), "{src:?}");
            assert_eq!(p.diagnostics.len(), 1, "{src:?} -> {:?}", p.diagnostics);
            assert!(
                p.diagnostics[0].message.contains("requires a string argument"),
                "{src:?} -> {:?}",
                p.diagnostics
            );
        }
    }

    #[test]
    fn decl_scoped_malformed_layout_argument_is_fully_consumed() {
        let p = crate::parser::parse("@layout(5)\nvar x: int = 0\n", "t");
        assert!(p.ast.layout.is_none());
        assert!(
            p.diagnostics.iter().any(|d| d.message.contains("module-level only")),
            "{:?}",
            p.diagnostics
        );
        assert!(
            p.diagnostics.iter().all(|d| !d.message.contains("unexpected token")),
            "argument tokens must not leak into the declaration parser: {:?}",
            p.diagnostics
        );
    }

    #[test]
    fn decl_scoped_layout_errors() {
        let p = crate::parser::parse("@layout(\"code\")\nvar x: int = 0\n", "t");
        // No blank line → decl-scoped → module-level-only error.
        assert!(p.ast.layout.is_none());
        assert!(p.diagnostics.iter().any(|d| d.message.contains("module-level only")));
    }

    #[test]
    fn module_annotations_may_share_one_line() {
        let p = crate::parser::parse("@layout(\"code\") @fold\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.ast.fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_annotations_same_line_reverse_order() {
        let p = crate::parser::parse("@fold @layout(\"code\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.ast.fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_flat_annotation_sets_flag() {
        let p = crate::parser::parse("@flat\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// `@flat` is independent of the layout choice — both spellings, both
    /// orders, and on one line or two.
    #[test]
    fn module_flat_composes_with_layout_and_fold() {
        for src in [
            "@flat\n@layout(\"cube\")\n\nvar x: int = 0\n",
            "@layout(\"cube\")\n@flat\n\nvar x: int = 0\n",
            "@flat @layout(\"cube\")\n\nvar x: int = 0\n",
            "@layout(\"cube\") @flat\n\nvar x: int = 0\n",
        ] {
            let p = crate::parser::parse(src, "t");
            assert!(p.ast.flat, "{src:?}");
            assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube), "{src:?}");
            assert!(p.diagnostics.is_empty(), "{src:?} -> {:?}", p.diagnostics);
        }
        let p = crate::parser::parse("@fold @flat\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat && p.ast.fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// The run's opening test and its same-line continuation test read one
    /// allowlist. If `@flat` were only in the first, the run would stop at it
    /// and everything after it on the line would parse as decl-scoped.
    #[test]
    fn a_module_annotation_after_flat_on_one_line_stays_module_level() {
        let p = crate::parser::parse("@flat @nofold\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat);
        assert!(p.ast.no_fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn repeating_flat_is_not_a_conflict() {
        let p = crate::parser::parse("@flat\n@flat\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn decl_scoped_flat_errors() {
        // No blank line → decl-scoped → module-level-only error.
        let p = crate::parser::parse("@flat\nvar x: int = 0\n", "t");
        assert!(!p.ast.flat);
        assert!(
            p.diagnostics.iter().any(|d| {
                d.message.contains("'@flat'") && d.message.contains("module-level only")
            }),
            "{:?}",
            p.diagnostics
        );
    }

    #[test]
    fn mixed_same_and_separate_lines() {
        let p = crate::parser::parse("@fold @layout(\"code\")\n@nofold\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.ast.no_fold);
        // @fold + @nofold still conflict-warn, exactly once.
        assert_eq!(
            p.diagnostics.iter().filter(|d| d.message.contains("conflict")).count(),
            1
        );
    }

    #[test]
    fn same_line_decl_scoped_annotations_still_hand_off() {
        // No blank line before the declaration → decl-scoped, module flags unset.
        let p = crate::parser::parse("@nofold @left\nin x: exec\n", "t");
        assert!(!p.ast.no_fold);
    }

    #[test]
    fn brace_disambiguation_preserved_and_bounded() {
        let no_err = |s: &str| crate::parser::parse(s, "t").diagnostics.iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error);
        assert!(no_err("let r = { x: 1, y: 2 }\n"), "record should parse");
        assert!(no_err("var m: Map<int,int> = { 1 => 2 }\n"), "map should parse");
        assert!(no_err("let b = { let x = 1\n x + 1 }\n"), "block-expr should parse");
        // Non-literal / non-trivial key expressions are valid maps — the key
        // is `parse_expr()`, so any expression may open an entry. These all
        // regressed under an allow-list precheck (misparsed as block-exprs,
        // spurious WSP001 on the `=>`); the reject-list precheck must let them
        // fall through to the `=>` scan and classify as maps.
        assert!(no_err("var m: Map<int,int> = { -1 => 1 }\n"), "unary-key map should parse");
        assert!(no_err("var m: Map<int,int> = { (1) => 1 }\n"), "paren-key map should parse");
        assert!(no_err("var m: Map<int,int> = { true => 1 }\n"), "bool-key map should parse");
        assert!(no_err("var m: Map<int,int> = { 1 + 1 => 2 }\n"), "binop-key map should parse");
        // an unbalanced expression-position brace must terminate quickly, not scan to EOF
        let big = format!("let x = {{ {}", "a a a a ".repeat(5000));
        let _ = crate::parser::parse(&big, "t"); // must simply COMPLETE (no hang)
    }

    #[test]
    fn parse_invisible_port_annotation() {
        let r = crate::parser::parse("@left @invisible in go: exec", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::In(p) => {
                assert!(p.invisible, "@invisible should set InDecl.invisible");
                assert_eq!(p.side, Some(crate::ast::PortSide::Left));
            }
            other => panic!("expected In decl, got {other:?}"),
        }
    }

    /// Walks a parsed script for the first `Expr::NestedPrefab`, returning its
    /// captured inner source. Only recurses through the handful of expression
    /// and statement kinds needed to reach a call argument inside a handler
    /// body — enough for this test, not a general-purpose AST visitor.
    fn find_nested_prefab_source(script: &Script) -> Option<String> {
        fn walk_expr(e: &Expr) -> Option<String> {
            match e {
                Expr::NestedPrefab { source, .. } => Some(source.clone()),
                Expr::Call { callee, args, .. } => walk_expr(callee).or_else(|| {
                    args.iter().find_map(|a| match a {
                        CallArg::Positional(e) | CallArg::Spread(e) => walk_expr(e),
                        CallArg::Named { value, .. } => walk_expr(value),
                    })
                }),
                _ => None,
            }
        }
        fn walk_stmt(s: &Stmt) -> Option<String> {
            match s {
                Stmt::Let(l) => walk_expr(&l.value),
                Stmt::ExprStmt(e) => walk_expr(&e.expr),
                Stmt::Assign(a) => walk_expr(&a.value),
                Stmt::Handler(h) => h.body.stmts.iter().find_map(walk_stmt),
                _ => None,
            }
        }
        script.decls.iter().find_map(|d| match d {
            TopDecl::Handler(h) => h.body.stmts.iter().find_map(walk_stmt),
            _ => None,
        })
    }

    #[test]
    fn parse_nested_prefab_expr() {
        let r = crate::parser::parse(
            "in go: exec\non go { let e = SpawnPrefab($```in a: exec```) }\n",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        assert_eq!(
            find_nested_prefab_source(&r.ast),
            Some("in a: exec".to_string()),
            "should parse a NestedPrefab carrying the inner source"
        );
    }

    #[test]
    fn method_chain_continues_across_newline() {
        // A `.method(...)` on the line after its receiver continues the chain
        // (a leading-dot continuation), rather than parsing as two statements /
        // a stray-`.` error.
        let src = "in go: exec\nin obj: entity\non go {\n  obj\n    .SendCustomEvent(\"x\", 1)\n}\n";
        let r = crate::parser::parse(src, "test");
        assert!(r.diagnostics.is_empty(), "chain should parse: {:?}", r.diagnostics);
        let handler = r
            .ast
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(
            handler.body.stmts.len(),
            1,
            "the chain is one statement, not two: {:?}",
            handler.body.stmts
        );
        match &handler.body.stmts[0] {
            Stmt::ExprStmt(es) => match &es.expr {
                Expr::Call { callee, .. } => assert!(
                    matches!(callee.as_ref(), Expr::FieldAccess { field, .. } if field == "SendCustomEvent"),
                    "callee should be a chained .SendCustomEvent, got {:?}",
                    es.expr
                ),
                other => panic!("expected a Call, got {other:?}"),
            },
            other => panic!("expected an ExprStmt, got {other:?}"),
        }
    }

    // ---- `on <Event> -> <pattern>` output capture -----

    #[test]
    fn on_arrow_tuple_on_named_event_binds_positionally() {
        // Tuple `( )` binds positionally for ANY event, named events included —
        // the cleaner form for `on CharacterDied() -> (character, killer)`.
        let r = crate::parser::parse("on CharacterDied() -> (character, killer) { }", "test");
        assert!(
            r.diagnostics.is_empty(),
            "tuple on a named event should parse cleanly: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn on_arrow_record_on_custom_event_is_error() {
        // Record `{ }` is only valid for named-data (built-in) events.
        let r = crate::parser::parse("on CustomEvent(\"x\") -> { a } { }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("named-data") && d.message.contains("( )")),
            "record on a custom event should error: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn on_arrow_record_unknown_field_is_error() {
        let r = crate::parser::parse("on CharacterDied() -> { foo } { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("CharacterDied")
                && d.message.contains("foo")
                && d.message.contains("killer")),
            "unknown record field should error and list valid names: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn on_arrow_and_inline_params_both_present_is_error() {
        // Inline data params are unconditionally an error. Regression coverage
        // that a leftover inline param still errors even when a `->` capture
        // is also present, and that the `->` capture still binds correctly
        // despite the leftover.
        let r = crate::parser::parse(
            "on CustomEvent(\"dmg\", amount: int) -> (amount) { }",
            "test",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("->")),
            "inline data param combined with `->` should error: {:?}",
            r.diagnostics
        );
        let handler = r
            .ast
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(
            handler.params.len(),
            1,
            "the `->` capture should still bind params despite the inline error: {:?}",
            handler.params
        );
        assert_eq!(handler.params[0].name, "amount");
    }

    #[test]
    fn inline_event_data_param_is_error_with_arrow_hint() {
        let r = crate::parser::parse("on CharacterDied(character) { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("->")),
            "inline output binding should error and point to `->`: {:?}",
            r.diagnostics
        );
        let r2 = crate::parser::parse("on CustomEvent(\"dmg\", amount: int) { }", "test");
        assert!(
            r2.diagnostics.iter().any(|d| d.message.contains("->")),
            "inline typed custom-event param should error: {:?}",
            r2.diagnostics
        );
    }

    /// An identifier filling a POSITIONAL CONFIG slot is a config value, not
    /// the removed inline output-binding form. `CustomEvent`'s channel name is
    /// such a slot, so `on CustomEvent(CH)` (for a `const CH`) — and any
    /// computed expression in the same slot — must parse, not hit the
    /// steer-to-`->` heuristic.
    #[test]
    fn an_identifier_in_a_positional_config_slot_is_not_an_inline_output_bind() {
        for src in [
            "const CH = \"evt_died\"\non CustomEvent(CH) -> (v: int) { }",
            "const PREFIX = \"evt_\"\non CustomEvent(PREFIX .. \"died\") -> (v: int) { }",
            "const CH = \"evt_died\"\non GlobalCustomEvent(CH) -> (v: int) { }",
        ] {
            let r = crate::parser::parse(src, "test");
            assert!(
                r.diagnostics.is_empty(),
                "a positional config slot must accept an identifier/expression, \
                 got {:?} for {src:?}",
                r.diagnostics
            );
        }
        // …and it really lands as a positional CONFIG arg, not as a param.
        let s = parse_ok("const CH = \"evt_died\"\non CustomEvent(CH) -> (v: int) { }");
        let handler = s
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(handler.config.len(), 1, "one positional config arg");
        assert!(
            matches!(
                &handler.config[0],
                crate::ast::HandlerConfigArg::Positional(crate::ast::Expr::Ident { name, .. })
                    if name == "CH"
            ),
            "the channel must be a positional config Ident, got {:?}",
            handler.config[0]
        );
    }

    /// The guard against over-broad suppression: an event with NO positional
    /// config slot must keep the steer-to-`->` heuristic and its original
    /// message, since there an identifier really is the removed inline
    /// output-binding form.
    #[test]
    fn an_event_without_positional_config_still_gets_the_arrow_hint() {
        for src in [
            "on CharacterDied(character) { }",
            "on CharacterSpawned(character) { }",
            "on ControllerJoined(controller) { }",
            // Past the ONE slot CustomEvent has, the heuristic applies again.
            "on CustomEvent(\"dmg\", amount) { }",
        ] {
            let r = crate::parser::parse(src, "test");
            assert!(
                r.diagnostics
                    .iter()
                    .any(|d| d.code == "WSP001"
                        && d.message.contains("bind event outputs with `-> (a, b)`")),
                "the inline-output-bind diagnostic must survive for {src:?}, got {:?}",
                r.diagnostics
            );
        }
    }

    #[test]
    fn on_arrow_absent_binds_nothing() {
        // No `->` present — no params bound.
        let s = parse_ok("on RoundStart() { }");
        let handler = s
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert!(handler.params.is_empty());
    }

    #[test]
    fn on_arrow_record_binds_positionally_with_gap_fill() {
        // `-> { killer }` on CharacterDied (data: character, killer, killerWeapon,
        // killerWeaponName) fills slot 0 with a synthesized unused name and binds
        // `killer` at slot 1; nothing past slot 1 is materialized.
        let s = parse_ok("on CharacterDied() -> { killer } { }");
        let handler = s
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(handler.params.len(), 2, "params: {:?}", handler.params);
        assert!(handler.params[0].name.starts_with("_arrow_unused_"));
        assert_eq!(handler.params[1].name, "killer");
    }

    #[test]
    fn on_arrow_record_rename_binds_alias() {
        let s = parse_ok("on CharacterSpawned() -> { character: c } { }");
        let handler = s
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(handler.params.len(), 1);
        assert_eq!(handler.params[0].name, "c");
    }

    #[test]
    fn on_arrow_bare_single_capture_parses() {
        // `-> p` (no parens) is shorthand for `-> (p)`: one untyped positional
        // capture. Works the same on named and custom events.
        let s = parse_ok("on CharacterDied() -> character { }");
        let handler = s
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(handler.params.len(), 1);
        assert_eq!(handler.params[0].name, "character");
        assert!(handler.params[0].ty.is_none());
        assert!(
            handler.params[0].source_field.is_none(),
            "bare capture binds positionally like a tuple, not by field name"
        );

        let s2 = parse_ok("on CustomEvent(\"dmg\") -> amount { }");
        let h2 = s2
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(h2.params.len(), 1);
        assert_eq!(h2.params[0].name, "amount");
    }

    #[test]
    fn on_arrow_bare_typed_single_capture_steers_to_parens() {
        // A type on the bare form needs parens (`-> (name: T)`); the `:` is
        // reported and swallowed so the handler body still parses.
        let r = crate::parser::parse("on CustomEvent(\"dmg\") -> amount: int { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("parens")),
            "typed bare capture should steer to parens: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn on_arrow_tuple_untyped_slot_parses() {
        let s = parse_ok("on CustomEvent(\"dmg\") -> (amount) { }");
        let handler = s
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(handler.params.len(), 1);
        assert_eq!(handler.params[0].name, "amount");
        assert!(handler.params[0].ty.is_none());
    }

    // A captured event `let x = on E { }` at statement level must report one
    // diagnostic that names the cause, not parse then silently drop its
    // trailing tokens into unrelated errors.
    #[test]
    fn captured_event_as_statement_reports_top_level_only() {
        let r = parse(
            "in go: exec\non go {\n  let tick = on Clock(1.0) { BroadcastChatMessage(\"hi\") }\n}",
            "test",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("captured events")),
            "a statement-level captured event must be reported clearly: {:?}",
            r.diagnostics
        );
        // No unrelated fallthrough errors should remain.
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.message.contains("unknown identifier ''")),
            "no garbage tail re-parse should remain: {:?}",
            r.diagnostics
        );
    }

    // An integer literal out of i64 range must be reported, not silently
    // compiled to 0.
    #[test]
    fn int_literal_overflow_is_reported() {
        let dec = parse("let big = 99999999999999999999", "test");
        assert!(
            dec.diagnostics.iter().any(|d| d.message.contains("out of range")),
            "a decimal overflow must be reported: {:?}",
            dec.diagnostics
        );
        let hex = parse("let h = 0xFFFFFFFFFFFFFFFFF", "test");
        assert!(
            hex.diagnostics.iter().any(|d| d.message.contains("out of range")),
            "a hex overflow must be reported: {:?}",
            hex.diagnostics
        );
        // i64::MIN still folds cleanly (its positive magnitude overflows i64).
        let min = parse("let m = -9223372036854775808", "test");
        assert!(
            min.diagnostics.is_empty(),
            "i64::MIN must still parse: {:?}",
            min.diagnostics
        );
    }

    // `const` bindings parse exactly like `let`, carrying `LetDecl.is_const`
    // set from the keyword used.
    #[test]
    fn const_binding_parses_as_a_let_with_the_const_flag() {
        let p = parse("const x = 1 << 4\nlet y = 2", "test");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        let consts: Vec<bool> = p
            .ast
            .decls
            .iter()
            .filter_map(|d| match d {
                TopDecl::Let(l) => Some(l.is_const),
                _ => None,
            })
            .collect();
        assert_eq!(consts, vec![true, false], "const flag must follow the keyword used");
    }

    #[test]
    fn const_binding_parses_inside_a_mod_body_with_an_annotation() {
        let p = parse("mod f() { const n: int = 3 }", "test");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    // `const mod`, per-parameter `const`, and the rejection of `const chip`.
    #[test]
    fn const_mod_marks_every_parameter_const() {
        let p = parse("const mod f(a: int, b: string) -> int { return a }", "test");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        let TopDecl::Chip(c) = &p.ast.decls[0] else {
            panic!("expected a chip decl")
        };
        assert!(c.is_const);
        assert!(c.inputs.iter().all(|p| p.is_const), "const mod implies const params");
    }

    #[test]
    fn a_plain_mod_may_mix_const_and_wired_parameters() {
        let p = parse("mod g(name: const string, v: int) { }", "test");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        let TopDecl::Chip(c) = &p.ast.decls[0] else {
            panic!("expected a chip decl")
        };
        assert!(!c.is_const);
        assert_eq!(
            c.inputs.iter().map(|p| p.is_const).collect::<Vec<_>>(),
            vec![true, false]
        );
    }

    #[test]
    fn const_params_are_allowed_on_a_chip_but_const_chip_is_not() {
        let ok = parse(
            "chip C(name: const string, v: int) -> (r: int) { out r = v }",
            "test",
        );
        assert!(ok.diagnostics.is_empty(), "{:?}", ok.diagnostics);

        let bad = parse("const chip C() -> (r: int) { out r = 1 }", "test");
        assert!(
            bad.diagnostics
                .iter()
                .any(|d| d.message.contains("`const chip`")),
            "expected a parse error naming `const chip`, got {:?}",
            bad.diagnostics
        );
    }

    #[test]
    fn parses_enum_decl_with_mixed_variants() {
        let src = "enum Shape {\n  Empty,\n  Circle(float),\n  Rect(float, float),\n  Box { w: float, h: float },\n}\n";
        let r = crate::parse(src, "t.ws");
        let (script, diags) = (&r.ast, &r.diagnostics);
        assert!(diags.is_empty(), "{diags:?}");
        let TopDecl::Enum(e) = script.decls.iter().find(|d| matches!(d, TopDecl::Enum(_))).expect("enum decl") else { unreachable!() };
        assert_eq!(e.name, "Shape");
        assert_eq!(e.variants.len(), 4);
        assert!(matches!(e.variants[0].payload, EnumPayloadDecl::Unit));
        assert!(matches!(&e.variants[1].payload, EnumPayloadDecl::Positional(t) if t.len() == 1));
        assert!(matches!(&e.variants[2].payload, EnumPayloadDecl::Positional(t) if t.len() == 2));
        assert!(matches!(&e.variants[3].payload, EnumPayloadDecl::Named(f) if f.len() == 2));
    }

    #[test]
    fn enum_is_contextual_and_still_usable_as_a_name() {
        // `enum` opens a declaration only when an identifier follows it, so
        // programs written before enums existed keep their `enum` variables,
        // parameters and record fields.
        let src = "enum Shape { Empty }\n\
                   var enum: int = 0\n\
                   type Rec = { enum: int }\n\
                   mod f(enum: int) -> int { return enum + 1 }\n";
        let r = crate::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(
            r.ast.decls.iter().any(|d| matches!(d, TopDecl::Enum(_))),
            "the declaration form must still parse"
        );
        assert!(
            r.ast.decls.iter().any(|d| matches!(d, TopDecl::Var(v) if v.name == "enum")),
            "`enum` must still bind as a variable name"
        );
    }

    #[test]
    fn parses_enum_explicit_discriminant() {
        let src = "enum Status { Idle = 0, Running = 5, Done }\n";
        let r = crate::parse(src, "t.ws");
        let (script, diags) = (&r.ast, &r.diagnostics);
        assert!(diags.is_empty(), "{diags:?}");
        let TopDecl::Enum(e) = &script.decls[0] else { panic!() };
        assert_eq!(e.variants[0].explicit_disc, Some(0));
        assert_eq!(e.variants[1].explicit_disc, Some(5));
        assert_eq!(e.variants[2].explicit_disc, None);
    }

    #[test]
    fn parses_enum_multiline_positional_payload() {
        let src = "enum Shape {\n  Circle(\n    float\n  ),\n  Rect(\n    float,\n    float,\n  ),\n}\n";
        let r = crate::parse(src, "t.ws");
        let (script, diags) = (&r.ast, &r.diagnostics);
        assert!(diags.is_empty(), "{diags:?}");
        let TopDecl::Enum(e) = &script.decls[0] else { panic!() };
        assert_eq!(e.variants.len(), 2);
        assert!(matches!(&e.variants[0].payload, EnumPayloadDecl::Positional(t) if t.len() == 1));
        assert!(matches!(&e.variants[1].payload, EnumPayloadDecl::Positional(t) if t.len() == 2));
    }

    #[test]
    fn if_condition_field_access_body_is_a_block_not_variant_ctor() {
        // A trailing `{ ... }` after an `if` condition that ends in a field
        // access is the `if` body, NOT braced variant construction. All three
        // shapes an empty/short block can take must parse with zero diagnostics.
        for src in [
            "in go: exec\nvar flag: bool = false\non go {\n  if flag { }\n}\n",
            "in go: exec\ntype R = { bar: bool }\nvar flag: bool = false\non go {\n  let r: R = { bar: true }\n  if r.bar { }\n}\n",
            "in go: exec\ntype R = { bar: bool }\nvar n: int = 0\non go {\n  let r: R = { bar: true }\n  if r.bar { n = 1 }\n}\n",
        ] {
            let r = crate::parser::parse(src, "t.ws");
            assert!(
                r.diagnostics.is_empty(),
                "if-condition body must parse as a block: src={src:?} diags={:?}",
                r.diagnostics
            );
        }
    }

    #[test]
    fn braced_construction_still_parses_in_value_position() {
        // `Enum.Variant { f: v, ... }` in a value/assignment position parses as
        // an `Expr::VariantCtor` with the given fields (the header suppression
        // only applies to condition/header positions, not `out x = ...`).
        let src = "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w: 1.0, h: 2.0 }\n";
        let r = crate::parser::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let TopDecl::Out(o) = &r.ast.decls[1] else {
            panic!("decl 1: {:?}", r.ast.decls[1])
        };
        let Some(Expr::VariantCtor { path, fields, .. }) = &o.value else {
            panic!("out value should be a VariantCtor, got {:?}", o.value)
        };
        assert!(
            matches!(path.as_ref(), Expr::FieldAccess { field, .. } if field == "Box"),
            "path should be Shape.Box: {path:?}"
        );
        assert_eq!(fields.len(), 2, "two named fields: {fields:?}");
    }

    #[test]
    fn shorthand_braced_construction_is_a_single_variant_ctor_not_a_split() {
        // Regression: a shorthand-first braced body (`{ w, h }`) after `A.B` in
        // a value position must parse as ONE `Expr::VariantCtor`, NOT silently
        // split into a bare `A.B` plus a separate `{ w, h }` block decl (which
        // dropped the field values with zero diagnostics).
        for (src, nfields) in [
            ("enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w, h }\n", 2),
            ("enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w, h: 2.0 }\n", 2),
        ] {
            let r = crate::parser::parse(src, "t.ws");
            assert!(r.diagnostics.is_empty(), "src={src:?} diags={:?}", r.diagnostics);
            assert_eq!(
                r.ast.decls.len(),
                2,
                "must not split into extra top-level decls: src={src:?} decls={:?}",
                r.ast.decls
            );
            let TopDecl::Out(o) = &r.ast.decls[1] else {
                panic!("decl 1: {:?}", r.ast.decls[1])
            };
            let Some(Expr::VariantCtor { fields, .. }) = &o.value else {
                panic!("out value should be a VariantCtor, got {:?}", o.value)
            };
            assert_eq!(fields.len(), nfields, "field count: src={src:?} fields={fields:?}");
        }
    }

    #[test]
    fn empty_braced_construction_in_value_position_does_not_split() {
        // Even an empty `{}` after `A.B` in a value position must be a
        // VariantCtor (which typecheck then errors on), never a silent split.
        let src = "enum Shape { Box { w: float } }\nout b = Shape.Box {}\n";
        let r = crate::parser::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.ast.decls.len(), 2, "empty-body construction must not split");
        let TopDecl::Out(o) = &r.ast.decls[1] else {
            panic!("decl 1: {:?}", r.ast.decls[1])
        };
        assert!(
            matches!(&o.value, Some(Expr::VariantCtor { .. })),
            "value should be a VariantCtor: {:?}",
            o.value
        );
    }

    #[test]
    fn malformed_braced_body_commits_with_a_diagnostic_not_a_silent_split() {
        // A non-record brace body after `A.B` in a value position (the most
        // plausible real typo: braces instead of parens for a positional
        // variant, or a stray block) must COMMIT to a single VariantCtor with a
        // parse diagnostic - NEVER silently split into two decls with the body
        // dropped.
        for src in [
            "enum S { Circle(float) }\nout c = S.Circle { 5.0 }\n",
            "out b = E.V { let x = 1 }\n",
            "out b = Shape.Box { computeW() }\n",
        ] {
            let r = crate::parser::parse(src, "t.ws");
            assert!(
                !r.diagnostics.is_empty(),
                "malformed braced construction must emit a diagnostic: src={src:?}"
            );
            // The `out` decl's value is a single VariantCtor whose malformed body
            // was consumed (zero recovered fields) - the decl list is NOT the
            // extra-decl "split" shape.
            let out = r
                .ast
                .decls
                .iter()
                .find_map(|d| if let TopDecl::Out(o) = d { Some(o) } else { None })
                .unwrap_or_else(|| panic!("expected an out decl: src={src:?} decls={:?}", r.ast.decls));
            assert!(
                matches!(&out.value, Some(Expr::VariantCtor { fields, .. }) if fields.is_empty()),
                "value should be a committed VariantCtor with a consumed body: src={src:?} value={:?}",
                out.value
            );
            // No trailing block/exprstmt decl split off after the `out`.
            assert!(
                !r.ast.decls.iter().any(|d| matches!(d, TopDecl::ExprStmt(_))),
                "malformed body must not split into a separate ExprStmt decl: src={src:?} decls={:?}",
                r.ast.decls
            );
        }
    }

    #[test]
    fn parses_nested_and_named_patterns() {
        let p = crate::parser::parse_pattern_str("Node(Some(x))");
        let Pattern::Variant { variant, sub: VariantPattern::Positional(inner), .. } = p else { panic!() };
        assert_eq!(variant, "Node");
        assert!(matches!(&inner[0], Pattern::Variant { variant, .. } if variant == "Some"));

        let q = crate::parser::parse_pattern_str("Box { w, h }");
        assert!(
            matches!(q, Pattern::Variant { sub: VariantPattern::Named { ref fields, ignore_rest: false }, .. } if fields.len() == 2)
        );

        let r = crate::parser::parse_pattern_str("Box { w, .. }");
        assert!(matches!(r, Pattern::Variant { sub: VariantPattern::Named { ignore_rest: true, .. }, .. }));

        assert!(matches!(crate::parser::parse_pattern_str("_"), Pattern::Wildcard(_)));
        assert!(matches!(crate::parser::parse_pattern_str("v"), Pattern::Binding { .. }));
    }

    #[test]
    fn parses_match_expression_and_statement() {
        let e = "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
                 out area = match sh {\n  Circle(r) => 3.14 * r * r,\n  Rect(w, h) => w * h,\n  Empty => 0.0,\n}\n";
        let r = crate::parse(e, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let TopDecl::Out(o) = &r.ast.decls[1] else {
            panic!("decl 1: {:?}", r.ast.decls[1])
        };
        let Some(Expr::MatchExpr { arms, .. }) = &o.value else {
            panic!("out value should be a MatchExpr, got {:?}", o.value)
        };
        assert_eq!(arms.len(), 3, "{arms:?}");
        assert!(matches!(&arms[0].body, MatchBody::Expr(_)));

        let st = "enum Shape { Empty, Circle(float) }\non go {\n  match sh {\n    Circle(r) => { emit out2 = r }\n    _ => { emit out2 = 0.0 }\n  }\n}\n";
        let r2 = crate::parse(st, "t.ws");
        assert!(r2.diagnostics.is_empty(), "{:?}", r2.diagnostics);
    }

    #[test]
    fn match_scrutinee_field_access_is_a_match_not_variant_ctor() {
        // The match scrutinee is a header context, same as `if`'s condition
        // (`parse_expr_no_brace_construct`): a trailing `{` after a
        // field-access scrutinee must open the match body, not be stolen as
        // braced variant construction (`obj.field { ... }` -> VariantCtor).
        let src = "enum Shape { Empty, Circle(float) }\n\
                   out area = match obj.field {\n  Circle(r) => r,\n  Empty => 0.0,\n}\n";
        let r = crate::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let TopDecl::Out(o) = &r.ast.decls[1] else {
            panic!("decl 1: {:?}", r.ast.decls[1])
        };
        let Some(Expr::MatchExpr { scrutinee, arms, .. }) = &o.value else {
            panic!("out value should be a MatchExpr, got {:?}", o.value)
        };
        assert!(
            matches!(scrutinee.as_ref(), Expr::FieldAccess { field, .. } if field == "field"),
            "scrutinee should be obj.field: {scrutinee:?}"
        );
        assert_eq!(arms.len(), 2, "{arms:?}");
    }

    #[test]
    fn parses_if_let_and_let_else() {
        let a = "enum Option { Some(int), None }\nin o: Option\non go {\n  if let Some(x) = o { emit r = x } else { emit r = 0 }\n}\nout r: int\n";
        let ra = crate::parse(a, "t.ws");
        assert!(ra.diagnostics.is_empty(), "{:?}", ra.diagnostics);
        let b = "enum Option { Some(int), None }\nin o: Option\nmod f() -> int {\n  let Some(x) = o else { return 0 }\n  return x\n}\n";
        let rb = crate::parse(b, "t.ws");
        assert!(rb.diagnostics.is_empty(), "{:?}", rb.diagnostics);
    }

    #[test]
    fn if_let_scrutinee_field_access_is_not_a_variant_ctor() {
        // Same header-context rule as `match`'s scrutinee: a trailing `{`
        // after a field-access scrutinee opens the then-block, not braced
        // variant construction (`obj.field { ... }` -> VariantCtor).
        let src = "enum Option { Some(int), None }\nin o: Option\non go {\n  if let Some(x) = o.field { emit r = x } else { emit r = 0 }\n}\nout r: int\n";
        let r = crate::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let handler = r.ast.decls.iter().find_map(|d| match d {
            TopDecl::Handler(h) => Some(h),
            _ => None,
        });
        let handler = handler.expect("expected a handler decl");
        let Stmt::IfLet(if_let) = &handler.body.stmts[0] else {
            panic!("expected an IfLet statement, got {:?}", handler.body.stmts[0])
        };
        assert!(
            matches!(&if_let.scrutinee, Expr::FieldAccess { field, .. } if field == "field"),
            "scrutinee should be o.field: {:?}",
            if_let.scrutinee
        );
        assert_eq!(if_let.then_block.stmts.len(), 1);
        assert!(if_let.else_block.is_some());
    }

    #[test]
    fn else_if_let_chains() {
        // `parse_if_else_tail`'s recursive `else if` handling must accept a
        // chained `if let` (not just a plain `if`) as the nested branch.
        let src = "enum Option { Some(int), None }\nin o: Option\nin p: Option\non go {\n  if let Some(x) = o { emit r = x } else if let Some(y) = p { emit r = y } else { emit r = 0 }\n}\nout r: int\n";
        let r = crate::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let handler = r.ast.decls.iter().find_map(|d| match d {
            TopDecl::Handler(h) => Some(h),
            _ => None,
        });
        let handler = handler.expect("expected a handler decl");
        let Stmt::IfLet(outer) = &handler.body.stmts[0] else {
            panic!("expected an IfLet statement, got {:?}", handler.body.stmts[0])
        };
        let inner_block = outer.else_block.as_ref().expect("outer else block");
        assert_eq!(inner_block.stmts.len(), 1);
        assert!(matches!(&inner_block.stmts[0], Stmt::IfLet(_)));
    }

    #[test]
    fn let_else_without_else_recovers_with_a_diagnostic() {
        // A refutable pattern head commits to the LetElse path; a missing
        // `else` is a parse error (via `expect`), not a panic.
        let src = "enum Option { Some(int), None }\nin o: Option\nmod f() -> int {\n  let Some(x) = o\n  return x\n}\n";
        let r = crate::parse(src, "t.ws");
        assert!(!r.diagnostics.is_empty(), "expected a diagnostic for the missing 'else'");
    }

    // `is` variant test

    #[test]
    fn is_parses_as_a_variant_test() {
        // `value is Enum.Variant` is its own expression node, holding the value
        // and the variant path it names.
        let s = parse_ok("enum Shape { Empty, Circle(float) }\nin s: Shape\nout b = s is Shape.Circle\n");
        let out = s.decls.iter().find_map(|d| match d {
            TopDecl::Out(o) => Some(o),
            _ => None,
        }).expect("out decl");
        let Some(Expr::Is { value, path, .. }) = out.value.as_ref() else {
            panic!("expected Expr::Is, got {:?}", out.value)
        };
        assert!(matches!(value.as_ref(), Expr::Ident { name, .. } if name == "s"));
        let Expr::FieldAccess { obj, field, .. } = path.as_ref() else {
            panic!("expected a variant path, got {:?}", path)
        };
        assert_eq!(field, "Circle");
        assert!(matches!(obj.as_ref(), Expr::Ident { name, .. } if name == "Shape"));
    }

    #[test]
    fn is_binds_like_an_equality_operator() {
        // `a is E.V && b` groups as `(a is E.V) && b`, matching `==`.
        let s = parse_ok("enum E { A, B }\nin e: E\nin b: bool\nout r = e is E.A && b\n");
        let out = s.decls.iter().find_map(|d| match d {
            TopDecl::Out(o) => Some(o),
            _ => None,
        }).expect("out decl");
        let Some(Expr::BinOp { op, left, .. }) = out.value.as_ref() else {
            panic!("expected a top-level BinOp, got {:?}", out.value)
        };
        assert_eq!(op, "&&");
        assert!(matches!(left.as_ref(), Expr::Is { .. }), "left: {:?}", left);
    }

    #[test]
    fn is_stays_usable_as_a_name() {
        // Contextual, like `enum` and `unsafe`: `is` is only the operator when a
        // name follows it, so a variable spelled `is` still parses.
        parse_ok("var is: int = 0\non start {\n  is = is + 1\n}\nin start: exec\n");
    }

    /// The statements of the one top-level handler in `src`.
    fn handler_body(src: &str) -> Vec<Stmt> {
        let s = parse_ok(src);
        s.decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h.body.stmts.clone()),
                _ => None,
            })
            .expect("handler")
    }

    #[test]
    fn emit_in_value_position_is_the_current_exec_atom() {
        // `emit` names the current exec chain point when an expression is
        // expected, the same dual-role shape `if`/`match` already have.
        let body = handler_body("in go: exec
on go {
  let d = SleepTicks(emit, delay = 2)
}
");
        let Some(Stmt::Let(l)) = body.first() else {
            panic!("expected a let, got {:?}", body.first())
        };
        let Expr::Call { args, .. } = &l.value else {
            panic!("expected a call, got {:?}", l.value)
        };
        assert!(
            matches!(args.first(), Some(CallArg::Positional(Expr::CurrentExec { .. }))),
            "`emit` as an argument must parse as Expr::CurrentExec: {args:?}"
        );
    }

    #[test]
    fn emit_statement_still_wins_inside_a_block_expression() {
        // A block expression's statement list keeps reading `emit name` as an
        // emit STATEMENT; only a bare `emit` is the current-exec atom.
        let body = handler_body(
            "out done: exec
var n: int = 0
in go: exec
on go {
  n = {
    emit done
    5
  }
}
",
        );
        let Some(Stmt::Assign(a)) = body.first() else {
            panic!("expected an assignment, got {:?}", body.first())
        };
        let Expr::BlockExpr { stmts, .. } = &a.value else {
            panic!("expected a block expression, got {:?}", a.value)
        };
        assert!(
            matches!(stmts.as_slice(), [Stmt::Emit(e)] if e.name == "done"),
            "`emit done` in a block expr is an emit statement: {stmts:?}"
        );
    }
