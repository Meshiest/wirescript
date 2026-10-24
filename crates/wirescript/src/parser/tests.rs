use super::*;

/// The first `on` handler in a parsed script, for a test that only declares
/// one. `what` names it in the panic so a parse regression says which
/// handler went missing.
fn first_handler<'a>(s: &'a Script, what: &str) -> &'a Handler {
    s.decls
        .iter()
        .find_map(|d| match d {
            TopDecl::Handler(h) => Some(h),
            _ => None,
        })
        .expect(what)
}

fn parse_ok(src: &str) -> Script {
    let r = parse(src, "test");
    assert!(
        r.diagnostics.is_empty(),
        "unexpected diags: {:?}",
        r.diagnostics
    );
    r.ast
}

/// Programs that parse with no diagnostic at all, and how many top-level
/// declarations they produce where the row pins it.
#[test]
fn parses_clean_corpus() {
    let rows: &[(&str, &str, Option<usize>)] = &[
        ("empty source", "", Some(0)),
        (
            "simple counter",
            "in tick: exec\nvar n: int = 0\non tick {\n  n = n + 1\n}\nout count = n",
            Some(4),
        ),
        ("out binding: bare name", "out foo", None),
        ("out binding: typed", "out foo: int", None),
        ("out binding: expression", "out foo = 1 + 2", None),
        ("out binding: typed with value", "out foo: int = 3", None),
        (
            "out binding: inside a chip",
            "chip {\n  out foo = 1\n}",
            None,
        ),
        // A declaration may end after its type.
        ("uninitialized var", "var x: int\n", None),
        (
            "uninitialized var then initialized",
            "var y: bool\nvar z: int = 0\n",
            None,
        ),
        ("uninitialized var in a chip", "chip { var q: int }\n", None),
        (
            "const binding inside a mod body with an annotation",
            "mod f() { const n: int = 3 }",
            None,
        ),
        // A trailing `{ ... }` after an `if` condition ending in a field
        // access is the `if` body, not braced variant construction.
        (
            "if condition: bare name",
            "in go: exec\nvar flag: bool = false\non go {\n  if flag { }\n}\n",
            None,
        ),
        (
            "if condition: field access, empty body",
            "in go: exec\ntype R = { bar: bool }\nvar flag: bool = false\non go {\n  let r: R = { bar: true }\n  if r.bar { }\n}\n",
            None,
        ),
        (
            "if condition: field access, assigning body",
            "in go: exec\ntype R = { bar: bool }\nvar n: int = 0\non go {\n  let r: R = { bar: true }\n  if r.bar { n = 1 }\n}\n",
            None,
        ),
        (
            "if let",
            "enum Option { Some(int), None }\nin o: Option\non go {\n  if let Some(x) = o { emit r = x } else { emit r = 0 }\n}\nout r: int\n",
            None,
        ),
        (
            "let else with a variant pattern",
            "enum Option { Some(int), None }\nin o: Option\nmod f() -> int {\n  let Some(x) = o else { return 0 }\n  return x\n}\n",
            None,
        ),
        (
            "let else starts with a value pattern",
            "in s: string\nin go: exec\non go {\n  let \"go \" .. d = s else { return }\n  let name .. \".ws\" = s else { return }\n  let \"[${team}] ${who}\" = s else { return }\n}\n",
            None,
        ),
        // `is` is the operator only when a name follows it.
        (
            "`is` as a variable name",
            "var is: int = 0\non start {\n  is = is + 1\n}\nin start: exec\n",
            None,
        ),
        (
            "a parenthesized conversion is a for source",
            "in go: exec\non go { for i in 0..(n as int) { } }",
            None,
        ),
        (
            "`..` continues across a line break outside a for source",
            "let s = \"a\" ..\n \"b\"",
            None,
        ),
        // Any expression may open a map entry; these misparsed as block
        // expressions under an allow-list brace precheck.
        ("brace: record", "let r = { x: 1, y: 2 }\n", None),
        ("brace: map", "var m: Map<int,int> = { 1 => 2 }\n", None),
        (
            "brace: block expression",
            "let b = { let x = 1\n x + 1 }\n",
            None,
        ),
        (
            "brace: unary-key map",
            "var m: Map<int,int> = { -1 => 1 }\n",
            None,
        ),
        (
            "brace: paren-key map",
            "var m: Map<int,int> = { (1) => 1 }\n",
            None,
        ),
        (
            "brace: bool-key map",
            "var m: Map<int,int> = { true => 1 }\n",
            None,
        ),
        (
            "brace: binop-key map",
            "var m: Map<int,int> = { 1 + 1 => 2 }\n",
            None,
        ),
        // A `=>` or a key that is not a bare name makes a map; `{ name: v }`
        // is a record.
        (
            "brace: two-entry map",
            "var m: Map<int,int> = { 1 => 2, 3 => 4 }\n",
            None,
        ),
        (
            "brace: string-key colon map",
            "var m: Map<string,int> = { \"a\": 1 }\n",
            None,
        ),
        (
            "brace: computed-key colon map",
            "in k: int\nvar m: Map<int,int> = { [k]: 9 }\n",
            None,
        ),
        (
            "brace: annotated record",
            "type P = { x: int }\nlet p: P = { x: 1 }\n",
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, decls) in rows {
        let r = parse(src, "test");
        if !r.diagnostics.is_empty() {
            failures.push(format!("{label}: {src:?}\n  diags: {:?}", r.diagnostics));
        } else if decls.is_some_and(|n| n != r.ast.decls.len()) {
            failures.push(format!(
                "{label}: {} decls, want {decls:?}",
                r.ast.decls.len()
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
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
    assert!(
        !md.contains("chip doc"),
        "module doc must not merge the chip doc: {md:?}"
    );
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
    // No blank line -> the block documents the first decl, so
    // `module_doc` is None and the first decl carries it.
    let src = "/// first decl doc\nvar x: int = 0";
    let r = parse(src, "test");
    assert!(
        r.ast.module_doc.is_none(),
        "adjacent block is not a module doc: {:?}",
        r.ast.module_doc
    );
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
    assert!(
        docs.iter().any(|d| d.contains("the x coordinate")),
        "x doc missing: {docs:?}"
    );
    assert!(
        docs.iter().any(|d| d.contains("the y coordinate")),
        "y doc missing: {docs:?}"
    );
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
        r.diagnostics.iter().any(|d| d
            .message
            .contains("event `RoundStart` must be called: write `on RoundStart()`")),
        "no-parens event trigger should error: {:?}",
        r.diagnostics
    );
    assert!(
        crate::parser::parse("on RoundStart() { }", "test")
            .diagnostics
            .is_empty()
    );
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
    assert!(
        crate::parser::parse("let s = on RoundStart()\non s { }", "test")
            .diagnostics
            .is_empty()
    );
    // The bare no-parens capture must error, matching the handler form.
    let r = crate::parser::parse("let s = on RoundStart\non s { }", "test");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.message.contains("must be called")),
        "no-parens event capture should error: {:?}",
        r.diagnostics
    );
}

/// How an `on` head parses: an expression or builtin-call trigger becomes a
/// synthetic `let _on_expr_0 = ...` plus a handler on it; an event (with or
/// without config args) and a bare `.field` trigger stay plain.
#[test]
fn handler_trigger_desugaring() {
    // (label, src, decl count, index of the synthetic let, trigger)
    let rows: &[(&str, &str, usize, Option<usize>, &str)] = &[
        (
            "`on a && b` desugars to let + handler",
            "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }",
            5,
            Some(3),
            "ident _on_expr_0",
        ),
        // A builtin call used as a trigger is the value-capture pattern
        // `let t = ServerUptime(); on t`, not the event-with-args form.
        (
            "`on ServerUptime()` desugars to let + handler",
            "on ServerUptime() { BroadcastChatMessage(\"tick\") }",
            2,
            Some(0),
            "ident _on_expr_0",
        ),
        // Its name IS an event, so the call-trigger path must not take it.
        (
            "`on Clock(enabled = true)` stays an event handler",
            "on Clock(enabled = true) { }",
            1,
            None,
            "ident Clock",
        ),
        // The call's trailing `(` must not be read as event-config args.
        (
            "`on a.Dot(b) > 0.0` desugars to let + handler",
            "in a: vector\nin b: vector\nvar x: int = 0\non a.Dot(b) > 0.0 { x = 1 }",
            5,
            Some(3),
            "ident _on_expr_0",
        ),
        // A bare `.field` head is a field trigger, not a value expression.
        (
            "`on split.Jump` stays a field trigger",
            "on split.Jump { }",
            1,
            None,
            "field split.Jump",
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, n, synthetic, want) in rows {
        let r = parse(src, "test");
        let decls = &r.ast.decls;
        let mut bad = Vec::new();
        if !r.diagnostics.is_empty() {
            bad.push(format!("diags: {:?}", r.diagnostics));
        }
        if decls.len() != n {
            bad.push(format!("{} decls, want {n}", decls.len()));
        }
        let lets: Vec<(usize, &str)> = decls
            .iter()
            .enumerate()
            .filter_map(|(i, d)| match d {
                TopDecl::Let(l) => match &l.binding {
                    LetBinding::Ident { name, .. } if name.starts_with("_on_expr_") => {
                        Some((i, name.as_str()))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let want_lets: Vec<(usize, &str)> =
            synthetic.map(|i| (i, "_on_expr_0")).into_iter().collect();
        if lets != want_lets {
            bad.push(format!("synthetic lets {lets:?}, want {want_lets:?}"));
        }
        match decls.last() {
            Some(TopDecl::Handler(h)) => {
                let got = match &h.trigger {
                    Trigger::Ident { name, .. } => format!("ident {name}"),
                    Trigger::Field { obj, field, .. } => format!("field {obj}.{field}"),
                    other => format!("{other:?}"),
                };
                if got != want {
                    bad.push(format!("trigger {got}, want {want}"));
                }
            }
            other => bad.push(format!("last decl is not a handler: {other:?}")),
        }
        if !bad.is_empty() {
            failures.push(format!("{label}\n  {}", bad.join("\n  ")));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn call_with_kwargs() {
    let s = parse_ok("var x = vec(x = 1, y = 2, z = 3)");
    match &s.decls[0] {
        TopDecl::Var(v) => match v.init.as_ref().unwrap() {
            Expr::Call { args, .. } => {
                let names: Vec<&str> = args
                    .iter()
                    .map(|a| match a {
                        CallArg::Named { name, .. } => name.as_str(),
                        other => panic!("expected a named arg, got {other:?}"),
                    })
                    .collect();
                assert_eq!(names, ["x", "y", "z"]);
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

fn float_init(src: &str) -> f64 {
    let s = parse_ok(src);
    match &s.decls[0] {
        TopDecl::Var(v) => match v.init.as_ref().unwrap() {
            Expr::FloatLit { value, .. } => *value,
            other => panic!("expected FloatLit, got {other:?}"),
        },
        _ => panic!("expected Var"),
    }
}

#[test]
fn an_error_at_the_end_of_an_interpolation_slot_reports_in_file_coordinates() {
    // "unexpected token '' in expression" is reported against the slot's
    // Eof token, which was the one token the lexer pushed without mapping
    // fragment coordinates back into the containing file, so a line-6
    // error landed on line 1.
    let src = "let a = 1\nlet b = 2\nlet c = 3\nlet d = 4\nlet e = 5\nlet f = \"v: ${ a + }\"\n";
    let r = parse(src, "test");
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.message.contains("in expression"))
        .unwrap_or_else(|| panic!("expected a slot diagnostic: {:?}", r.diagnostics));
    assert_eq!(d.range.start.line, 6, "{d:?}");
}

#[test]
fn float_literals_keep_their_digit_separators() {
    assert_eq!(float_init("var x = 1_000.5"), 1000.5);
    // The negative arm parsed the raw token text, so `_` made `f64`'s
    // parser fail and `unwrap_or(0.0)` baked `-0.0` with no diagnostic.
    assert_eq!(float_init("var x = -1_000.5"), -1000.5);
    // The lexer stopped the exponent at the first `_`, splitting this into
    // `2.5e1` and an identifier `_0`.
    assert_eq!(float_init("var x = 2.5e1_0"), 2.5e10);
    assert_eq!(float_init("var x = -2.5e1_0"), -2.5e10);
}

#[test]
fn unrepresentable_float_literals_report_instead_of_baking_a_value() {
    for src in [
        "var x = 1e",
        "var x = -1e",
        "var x = 1e999",
        "var x = -1e999",
    ] {
        let r = parse(src, "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("float")),
            "{src} produced no float diagnostic: {:?}",
            r.diagnostics
        );
    }
}

#[test]
fn tuple_index_reports_instead_of_silently_picking_slot_zero() {
    let r = parse("let t = (1, 2)\nlet a = t.0x1", "test");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.message.contains("tuple index")),
        "expected a tuple-index diagnostic: {:?}",
        r.diagnostics
    );
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
fn fn_decl_takes_the_mod_grammar() {
    let r = crate::parser::parse("fn add(a: int, b: int = 1) -> int { return a + b }", "test");
    assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    let crate::ast::TopDecl::Chip(c) = &r.ast.decls[0] else {
        panic!("a fn is a chip decl: {:?}", r.ast.decls[0]);
    };
    assert!(c.is_fn && c.inline && !c.is_const);
    assert!(c.inputs[0].default.is_none());
    assert!(matches!(
        c.inputs[1].default,
        Some(crate::ast::Expr::IntLit { value: 1, .. })
    ));
    // The old expression-bodied form is not the grammar.
    let old = crate::parser::parse("fn add(a: int, b: int) -> int = a + b", "test");
    assert!(!old.diagnostics.is_empty());
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
        crate::parser::parse(s, "t")
            .ast
            .decls
            .into_iter()
            .find_map(|d| {
                if let TopDecl::Chip(c) = d {
                    Some(c)
                } else {
                    None
                }
            })
            .expect("a mod/chip")
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
    let ta = ast
        .decls
        .iter()
        .find_map(|d| {
            if let TopDecl::TypeAlias(t) = d {
                Some(t)
            } else {
                None
            }
        })
        .expect("alias");
    assert_eq!(ta.type_params.len(), 1);
    assert_eq!(ta.type_params[0].name, "T");

    for s in [
        "mod pick<T>(a: T) -> T { return a }\n",
        "mod plain(a: int) -> int { return a }\n",
        "type Grid<T> = T[]\n",
    ] {
        assert!(
            crate::parser::parse(s, "t")
                .diagnostics
                .iter()
                .all(|d| d.severity != crate::diagnostic::Severity::Error),
            "should parse cleanly: {s}"
        );
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

/// The kind of a top-level declaration, for table rows.
fn decl_kind(d: &TopDecl) -> &'static str {
    match d {
        TopDecl::Var(_) => "var",
        TopDecl::In(_) => "in",
        TopDecl::Out(_) => "out",
        TopDecl::Chip(c) if c.inline => "mod",
        TopDecl::Chip(_) => "chip",
        TopDecl::AnonChip(_) => "anon chip",
        _ => "other",
    }
}

/// A misused declaration annotation is reported and the declaration under
/// it still parses. Each message substring must appear in some diagnostic.
#[test]
fn decl_annotation_errors() {
    // (label, src, message substrings, first decl kind or "" for unchecked)
    let rows: &[(&str, &str, &[&str], &str)] = &[
        (
            "unknown annotation word lists every accepted word",
            "@middle in a: bool",
            &[
                "unknown annotation '@middle'",
                "expected @left, @right, @top, @bottom, @label, @color, @material, @closed, @invisible, @nofold, or @sliced",
            ],
            "in",
        ),
        (
            "a side annotation before a non-port decl",
            "@left var x: int = 1",
            &["must be followed by an 'in', 'out', or chip declaration"],
            "var",
        ),
        // `@invisible` is a port annotation, so pairing it with `@nofold`
        // before a plain `var` is not the bare-@nofold case.
        (
            "@invisible @nofold before a var",
            "@invisible @nofold var x: int = 0",
            &["must be followed by an 'in', 'out', or chip declaration"],
            "var",
        ),
        (
            "duplicate side annotation",
            "@left @right in a: bool",
            &["duplicate side annotation"],
            "in",
        ),
        (
            "@closed on a port",
            "@closed in t: exec",
            &["@closed is not allowed on 'in'/'out'"],
            "in",
        ),
        (
            "@label without an argument",
            "@label chip { }",
            &["@label requires a string argument"],
            "",
        ),
        (
            "@label with empty text",
            "@label(\"\") chip { }",
            &["@label text must not be empty"],
            "",
        ),
        (
            "two string @labels",
            "@label(\"a\") @label(\"b\") chip { }",
            &["duplicate @label"],
            "",
        ),
        (
            "a string @label and an expression @label",
            "@label(\"a\") @label(x) chip { }",
            &["duplicate @label"],
            "",
        ),
        (
            "an annotation on a mod",
            "@label(\"x\") mod inc(v: int) -> int { return v + 1 }",
            &["annotations are not allowed on 'mod'"],
            "mod",
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, msgs, kind) in rows {
        let r = parse(src, "test");
        let mut bad = Vec::new();
        for m in msgs {
            if !r.diagnostics.iter().any(|d| d.message.contains(m)) {
                bad.push(format!("no diagnostic contains {m:?}"));
            }
        }
        let got = r.ast.decls.first().map_or("none", decl_kind);
        if !kind.is_empty() && got != kind {
            bad.push(format!("first decl is {got}, want {kind}"));
        }
        if !bad.is_empty() {
            failures.push(format!(
                "{label}: {src:?}\n  {}\n  diags: {:?}",
                bad.join("\n  "),
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());

    // An unknown word is the only diagnostic and leaves no side.
    let r = parse("@middle in a: bool", "test");
    assert_eq!(r.diagnostics.len(), 1, "diags: {:?}", r.diagnostics);
    let TopDecl::In(i) = &r.ast.decls[0] else {
        panic!()
    };
    assert_eq!(i.side, None);
    // The first of two side annotations wins.
    let r = parse("@left @right in a: bool", "test");
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
    assert!(!ac.closed, "an anonymous chip is open by default");
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
    let TopDecl::Chip(c) = &r.ast.decls[0] else {
        panic!()
    };
    assert!(c.closed);
    for i in 1..=2 {
        let TopDecl::AnonChip(ac) = &r.ast.decls[i] else {
            panic!("decl {i}: {:?}", r.ast.decls[i])
        };
        assert!(ac.closed, "decl {i} should be closed");
    }
    let TopDecl::AnonChip(ac) = &r.ast.decls[3] else {
        panic!()
    };
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
    let TopDecl::In(i) = &r.ast.decls[0] else {
        panic!()
    };
    assert_eq!(i.side, Some(PortSide::Left));
    assert_eq!(i.label.as_deref(), Some("Fire!"));
    let TopDecl::Out(o) = &r.ast.decls[1] else {
        panic!()
    };
    assert_eq!(o.side, Some(PortSide::Right));
    assert_eq!(o.label.as_deref(), Some("Total"));
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
fn open_is_an_ordinary_identifier() {
    let src = "@left in open: bool\nvar opened: bool = open\n\
                   mod pass(open: int) -> int { return open }\n\
                   chip { var open: int = pass(1) }";
    let r = parse(src, "test");
    assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    let TopDecl::In(i) = &r.ast.decls[0] else {
        panic!("decl 0: {:?}", r.ast.decls[0])
    };
    assert_eq!(i.name, "open");
}

#[test]
fn chip_annotations_parse_at_statement_level() {
    let r = parse(
        "chip Outer(x: int) { @closed chip { var a: int = 0 } }",
        "test",
    );
    assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    let TopDecl::Chip(c) = &r.ast.decls[0] else {
        panic!()
    };
    let Stmt::AnonChip(ac) = &c.body.stmts[0] else {
        panic!("stmt: {:?}", c.body.stmts[0])
    };
    assert!(ac.closed);
}

/// What a module-annotation row expects. A `None` flag is unchecked.
struct ModuleAnns {
    layout: Option<crate::ast::LayoutName>,
    flat: Option<bool>,
    fold: Option<bool>,
    no_fold: Option<bool>,
    invisible: Option<bool>,
    module_label: Option<bool>,
    /// The first decl is a var with no string label and this `label_expr` presence.
    var_label_expr: Option<bool>,
    /// Each substring is in exactly one diagnostic.
    diags: &'static [&'static str],
    /// No diagnostic contains any of these.
    absent: &'static [&'static str],
    /// The total diagnostic count.
    total: Option<usize>,
}

/// Module-level annotations: a run at the top of the file followed by a
/// blank line sets module flags; without the blank line they are
/// decl-scoped.
#[test]
fn module_annotations() {
    use crate::ast::LayoutName::{Code, Cube};
    const M: ModuleAnns = ModuleAnns {
        layout: None,
        flat: None,
        fold: None,
        no_fold: None,
        invisible: None,
        module_label: None,
        var_label_expr: None,
        diags: &[],
        absent: &[],
        total: None,
    };
    const CLEAN: ModuleAnns = ModuleAnns {
        total: Some(0),
        ..M
    };
    let rows = [
        (
            "@layout(\"code\") sets the layout",
            "@layout(\"code\")\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Code),
                ..CLEAN
            },
        ),
        (
            "@layout mixes with a @fold run",
            "@fold\n@layout(\"code\")\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Code),
                fold: Some(true),
                ..CLEAN
            },
        ),
        (
            "@layout(\"cube\") selects the cube",
            "@layout(\"cube\")\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Cube),
                ..CLEAN
            },
        ),
        // No engine outranks another, so the file is told which one survived.
        (
            "two layouts warn once and the last wins",
            "@layout(\"code\")\n@layout(\"cube\")\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Cube),
                diags: &["set twice"],
                ..M
            },
        ),
        (
            "repeating one layout is not a conflict",
            "@layout(\"cube\")\n@layout(\"cube\")\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Cube),
                ..CLEAN
            },
        ),
        (
            "unknown layout name",
            "@layout(\"grid\")\n\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["unknown layout"],
                total: Some(1),
                ..M
            },
        ),
        (
            "@layout without an argument",
            "@layout\n\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["requires a string argument"],
                total: Some(1),
                ..M
            },
        ),
        (
            "malformed layout argument: unclosed paren",
            "@layout(\n\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["requires a string argument"],
                total: Some(1),
                ..M
            },
        ),
        (
            "malformed layout argument: a number",
            "@layout(5)\n\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["requires a string argument"],
                total: Some(1),
                ..M
            },
        ),
        (
            "malformed layout argument: string without a closing paren",
            "@layout(\"code\"\n\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["requires a string argument"],
                total: Some(1),
                ..M
            },
        ),
        // The argument tokens must not leak into the declaration parser.
        (
            "decl-scoped malformed layout argument is fully consumed",
            "@layout(5)\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["module-level only"],
                absent: &["unexpected token"],
                ..M
            },
        ),
        (
            "decl-scoped @layout errors",
            "@layout(\"code\")\nvar x: int = 0\n",
            ModuleAnns {
                diags: &["module-level only"],
                ..M
            },
        ),
        (
            "module annotations may share one line",
            "@layout(\"code\") @fold\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Code),
                fold: Some(true),
                ..CLEAN
            },
        ),
        (
            "module annotations on one line, reverse order",
            "@fold @layout(\"code\")\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Code),
                fold: Some(true),
                ..CLEAN
            },
        ),
        (
            "@flat sets the flag",
            "@flat\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                ..CLEAN
            },
        ),
        // `@flat` is independent of the layout choice: both orders, one line or two.
        (
            "@flat then @layout on two lines",
            "@flat\n@layout(\"cube\")\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                layout: Some(Cube),
                ..CLEAN
            },
        ),
        (
            "@layout then @flat on two lines",
            "@layout(\"cube\")\n@flat\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                layout: Some(Cube),
                ..CLEAN
            },
        ),
        (
            "@flat then @layout on one line",
            "@flat @layout(\"cube\")\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                layout: Some(Cube),
                ..CLEAN
            },
        ),
        (
            "@layout then @flat on one line",
            "@layout(\"cube\") @flat\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                layout: Some(Cube),
                ..CLEAN
            },
        ),
        (
            "@fold @flat",
            "@fold @flat\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                fold: Some(true),
                ..CLEAN
            },
        ),
        // The run's opening test and its same-line continuation read one
        // allowlist, so an annotation after `@flat` on its line stays module-level.
        (
            "a module annotation after @flat on one line",
            "@flat @nofold\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                no_fold: Some(true),
                ..CLEAN
            },
        ),
        (
            "repeating @flat is not a conflict",
            "@flat\n@flat\n\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(true),
                ..CLEAN
            },
        ),
        (
            "decl-scoped @flat errors",
            "@flat\nvar x: int = 0\n",
            ModuleAnns {
                flat: Some(false),
                diags: &["'@flat' is module-level only"],
                ..M
            },
        ),
        // @fold + @nofold still conflict-warn, exactly once.
        (
            "mixed same and separate lines",
            "@fold @layout(\"code\")\n@nofold\n\nvar x: int = 0\n",
            ModuleAnns {
                layout: Some(Code),
                no_fold: Some(true),
                diags: &["conflict"],
                ..M
            },
        ),
        (
            "same-line decl-scoped annotations still hand off",
            "@nofold @left\nin x: exec\n",
            ModuleAnns {
                no_fold: Some(false),
                ..M
            },
        ),
        // A blank-line-separated top `@label` labels the root chip, not the var.
        (
            "a blank-line-separated @label labels the root",
            "@label(title)\n\nvar title: string = \"hi\"\n",
            ModuleAnns {
                module_label: Some(true),
                var_label_expr: Some(false),
                ..CLEAN
            },
        ),
        // The run finishes and the `@label` is claimed: no lost-annotation error.
        (
            "a module run hands off to a module @label",
            "@invisible\n@label(title)\n\nvar title: string = \"hi\"\n",
            ModuleAnns {
                invisible: Some(true),
                module_label: Some(true),
                ..CLEAN
            },
        ),
        (
            "an attached @label stays decl-level",
            "@label(title)\nvar title: string = \"hi\"\n",
            ModuleAnns {
                module_label: Some(false),
                var_label_expr: Some(true),
                ..M
            },
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in &rows {
        let p = parse(src, "t");
        let mut bad = Vec::new();
        if p.ast.layout != want.layout {
            bad.push(format!("layout {:?}, want {:?}", p.ast.layout, want.layout));
        }
        for (name, got, want) in [
            ("flat", p.ast.flat, want.flat),
            ("fold", p.ast.fold, want.fold),
            ("no_fold", p.ast.no_fold, want.no_fold),
            ("invisible", p.ast.invisible, want.invisible),
            (
                "module_label",
                p.ast.module_label.is_some(),
                want.module_label,
            ),
        ] {
            if want.is_some_and(|w| w != got) {
                bad.push(format!("{name} is {got}"));
            }
        }
        if let Some(w) = want.var_label_expr {
            match p.ast.decls.first() {
                Some(TopDecl::Var(v)) if v.label.is_none() && v.label_expr.is_some() == w => {}
                other => bad.push(format!("first decl label is wrong: {other:?}")),
            }
        }
        for s in want.diags {
            let n = p
                .diagnostics
                .iter()
                .filter(|d| d.message.contains(s))
                .count();
            if n != 1 {
                bad.push(format!("{n} diagnostics contain {s:?}"));
            }
        }
        for s in want.absent {
            if p.diagnostics.iter().any(|d| d.message.contains(s)) {
                bad.push(format!("a diagnostic contains {s:?}"));
            }
        }
        if want.total.is_some_and(|t| t != p.diagnostics.len()) {
            bad.push(format!(
                "{} diagnostics, want {:?}",
                p.diagnostics.len(),
                want.total
            ));
        }
        if !bad.is_empty() {
            failures.push(format!(
                "{label}: {src:?}\n  {}\n  diags: {:?}",
                bad.join("\n  "),
                p.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
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
fn an_unbalanced_brace_in_expression_position_terminates() {
    // The brace precheck must stop quickly, not scan to EOF.
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
    assert!(
        r.diagnostics.is_empty(),
        "chain should parse: {:?}",
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

#[test]
fn a_leading_star_on_a_new_line_starts_a_new_statement() {
    // `*` at the START of a line (reached only by skipping a newline to
    // find it) is a deref statement, not a continuation of the previous
    // line's expression: `n = 5` ends at the newline, and `*m = 1` is its
    // own assignment through a deref target. Contrast with the next test,
    // where the operator trails the FIRST line instead.
    let body =
        handler_body("var n: int = 0\nvar m: int = 0\nin go: exec\non go {\nn = 5\n*m = 1\n}\n");
    assert_eq!(body.len(), 2, "two statements, not one: {body:?}");
    match &body[0] {
        Stmt::Assign(a) => {
            assert!(matches!(&a.target, Expr::Ident { name, .. } if name == "n"));
            assert!(
                matches!(&a.value, Expr::IntLit { value: 5, .. }),
                "not continued into a multiply: {:?}",
                a.value
            );
        }
        other => panic!("expected an Assign, got {other:?}"),
    }
    match &body[1] {
        Stmt::Assign(a) => {
            assert!(
                matches!(&a.target, Expr::Deref { .. }),
                "expected a Deref target, got {:?}",
                a.target
            );
        }
        other => panic!("expected an Assign, got {other:?}"),
    }
}

#[test]
fn a_trailing_star_still_continues_across_the_newline() {
    // The operator trails the FIRST line here, so this is genuine line
    // continuation (`let x = a +\n  b`'s `*` sibling), unlike the leading-
    // `*` case above.
    let body =
        handler_body("var a: int = 0\nvar b: int = 0\nin go: exec\non go {\nlet x = a *\n  b\n}\n");
    let Some(Stmt::Let(l)) = body.first() else {
        panic!("expected a let, got {:?}", body.first())
    };
    assert!(
        matches!(&l.value, Expr::BinOp { op, .. } if op == "*"),
        "expected a continued `a * b`, got {:?}",
        l.value
    );
}

// ---- `on <Event> -> <pattern>` output capture -----

/// What an `on ... -> pattern` row expects.
struct Arrow {
    /// Substrings one diagnostic must all contain; empty means a clean parse.
    diag: &'static [&'static str],
    /// The bound param names; a trailing `*` matches a prefix.
    params: Option<&'static [&'static str]>,
    /// Every param has no type annotation.
    untyped: bool,
    /// Every param binds by position, not by source field name.
    positional: bool,
}

#[test]
fn on_arrow_bindings() {
    const A: Arrow = Arrow {
        diag: &[],
        params: None,
        untyped: false,
        positional: false,
    };
    let rows = [
        // Tuple `( )` binds positionally for ANY event, named events included.
        (
            "a tuple on a named event binds positionally",
            "on CharacterDied() -> (character, killer) { }",
            Arrow {
                params: Some(&["character", "killer"]),
                positional: true,
                ..A
            },
        ),
        // Record `{ }` is only valid for named-data (built-in) events.
        (
            "a record on a custom event",
            "on CustomEvent(\"x\") -> { a } { }",
            Arrow {
                diag: &["named-data", "( )"],
                ..A
            },
        ),
        (
            "an unknown record field lists the valid names",
            "on CharacterDied() -> { foo } { }",
            Arrow {
                diag: &["CharacterDied", "foo", "killer"],
                ..A
            },
        ),
        // A leftover inline param errors even with a `->` capture, which still binds.
        (
            "inline params and `->` both present",
            "on CustomEvent(\"dmg\", amount: int) -> (amount) { }",
            Arrow {
                diag: &["->"],
                params: Some(&["amount"]),
                ..A
            },
        ),
        (
            "an inline typed custom-event param",
            "on CustomEvent(\"dmg\", amount: int) { }",
            Arrow { diag: &["->"], ..A },
        ),
        (
            "no `->` binds nothing",
            "on RoundStart() { }",
            Arrow {
                params: Some(&[]),
                ..A
            },
        ),
        // CharacterDied's data is (character, killer, ...): slot 0 gets a
        // synthesized unused name and nothing past `killer` is materialized.
        (
            "a record binds positionally with gap fill",
            "on CharacterDied() -> { killer } { }",
            Arrow {
                params: Some(&["_arrow_unused_*", "killer"]),
                ..A
            },
        ),
        (
            "a record rename binds the alias",
            "on CharacterSpawned() -> { character: c } { }",
            Arrow {
                params: Some(&["c"]),
                ..A
            },
        ),
        // `-> p` is shorthand for `-> (p)`, on named and custom events alike.
        (
            "a bare single capture on a named event",
            "on CharacterDied() -> character { }",
            Arrow {
                params: Some(&["character"]),
                untyped: true,
                positional: true,
                ..A
            },
        ),
        (
            "a bare single capture on a custom event",
            "on CustomEvent(\"dmg\") -> amount { }",
            Arrow {
                params: Some(&["amount"]),
                ..A
            },
        ),
        // A type on the bare form needs parens; the `:` is reported and swallowed.
        (
            "a typed bare capture steers to parens",
            "on CustomEvent(\"dmg\") -> amount: int { }",
            Arrow {
                diag: &["parens"],
                ..A
            },
        ),
        (
            "an untyped tuple slot",
            "on CustomEvent(\"dmg\") -> (amount) { }",
            Arrow {
                params: Some(&["amount"]),
                untyped: true,
                ..A
            },
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in &rows {
        let r = parse(src, "test");
        let mut bad = Vec::new();
        if want.diag.is_empty() {
            if !r.diagnostics.is_empty() {
                bad.push("expected a clean parse".to_string());
            }
        } else if !r
            .diagnostics
            .iter()
            .any(|d| want.diag.iter().all(|s| d.message.contains(s)))
        {
            bad.push(format!("no diagnostic contains all of {:?}", want.diag));
        }
        let handler = r.ast.decls.iter().find_map(|d| match d {
            TopDecl::Handler(h) => Some(h),
            _ => None,
        });
        match (handler, want.params) {
            (None, Some(_)) => bad.push("no handler".to_string()),
            (Some(h), Some(names)) => {
                let got: Vec<&str> = h.params.iter().map(|p| p.name.as_str()).collect();
                let matches = got.len() == names.len()
                    && got
                        .iter()
                        .zip(names.iter())
                        .all(|(g, w)| match w.strip_suffix('*') {
                            Some(prefix) => g.starts_with(prefix),
                            None => g == w,
                        });
                if !matches {
                    bad.push(format!("params {got:?}, want {names:?}"));
                }
                if want.untyped && h.params.iter().any(|p| p.ty.is_some()) {
                    bad.push("a param is typed".to_string());
                }
                if want.positional && h.params.iter().any(|p| p.source_field.is_some()) {
                    bad.push("a param binds by field name".to_string());
                }
            }
            _ => {}
        }
        if !bad.is_empty() {
            failures.push(format!(
                "{label}: {src:?}\n  {}\n  diags: {:?}",
                bad.join("\n  "),
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
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
    let handler = first_handler(&s, "a handler");
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
        dec.diagnostics
            .iter()
            .any(|d| d.message.contains("out of range")),
        "a decimal overflow must be reported: {:?}",
        dec.diagnostics
    );
    let hex = parse("let h = 0xFFFFFFFFFFFFFFFFF", "test");
    assert!(
        hex.diagnostics
            .iter()
            .any(|d| d.message.contains("out of range")),
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
    assert_eq!(
        consts,
        vec![true, false],
        "const flag must follow the keyword used"
    );
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
    assert!(
        c.inputs.iter().all(|p| p.is_const),
        "const mod implies const params"
    );
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
    let TopDecl::Enum(e) = script
        .decls
        .iter()
        .find(|d| matches!(d, TopDecl::Enum(_)))
        .expect("enum decl")
    else {
        unreachable!()
    };
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
        r.ast
            .decls
            .iter()
            .any(|d| matches!(d, TopDecl::Var(v) if v.name == "enum")),
        "`enum` must still bind as a variable name"
    );
}

#[test]
fn parses_enum_explicit_discriminant() {
    let src = "enum Status { Idle = 0, Running = 5, Done }\n";
    let r = crate::parse(src, "t.ws");
    let (script, diags) = (&r.ast, &r.diagnostics);
    assert!(diags.is_empty(), "{diags:?}");
    let TopDecl::Enum(e) = &script.decls[0] else {
        panic!()
    };
    assert_eq!(e.variants[0].explicit_disc, Some(0));
    assert_eq!(e.variants[1].explicit_disc, Some(5));
    assert_eq!(e.variants[2].explicit_disc, None);
}

#[test]
fn parses_enum_multiline_positional_payload() {
    let src =
        "enum Shape {\n  Circle(\n    float\n  ),\n  Rect(\n    float,\n    float,\n  ),\n}\n";
    let r = crate::parse(src, "t.ws");
    let (script, diags) = (&r.ast, &r.diagnostics);
    assert!(diags.is_empty(), "{diags:?}");
    let TopDecl::Enum(e) = &script.decls[0] else {
        panic!()
    };
    assert_eq!(e.variants.len(), 2);
    assert!(matches!(&e.variants[0].payload, EnumPayloadDecl::Positional(t) if t.len() == 1));
    assert!(matches!(&e.variants[1].payload, EnumPayloadDecl::Positional(t) if t.len() == 2));
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
        (
            "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w, h }\n",
            2,
        ),
        (
            "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w, h: 2.0 }\n",
            2,
        ),
    ] {
        let r = crate::parser::parse(src, "t.ws");
        assert!(
            r.diagnostics.is_empty(),
            "src={src:?} diags={:?}",
            r.diagnostics
        );
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
        assert_eq!(
            fields.len(),
            nfields,
            "field count: src={src:?} fields={fields:?}"
        );
    }
}

#[test]
fn empty_braced_construction_in_value_position_does_not_split() {
    // Even an empty `{}` after `A.B` in a value position must be a
    // VariantCtor (which typecheck then errors on), never a silent split.
    let src = "enum Shape { Box { w: float } }\nout b = Shape.Box {}\n";
    let r = crate::parser::parse(src, "t.ws");
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert_eq!(
        r.ast.decls.len(),
        2,
        "empty-body construction must not split"
    );
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
            .find_map(|d| {
                if let TopDecl::Out(o) = d {
                    Some(o)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| panic!("expected an out decl: src={src:?} decls={:?}", r.ast.decls));
        assert!(
            matches!(&out.value, Some(Expr::VariantCtor { fields, .. }) if fields.is_empty()),
            "value should be a committed VariantCtor with a consumed body: src={src:?} value={:?}",
            out.value
        );
        // No trailing block/exprstmt decl split off after the `out`.
        assert!(
            !r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::ExprStmt(_))),
            "malformed body must not split into a separate ExprStmt decl: src={src:?} decls={:?}",
            r.ast.decls
        );
    }
}

#[test]
fn parses_nested_and_named_patterns() {
    let p = crate::parser::parse_pattern_str("Node(Some(x))");
    let Pattern::Variant {
        variant,
        sub: VariantPattern::Positional(inner),
        ..
    } = p
    else {
        panic!()
    };
    assert_eq!(variant, "Node");
    assert!(matches!(&inner[0], Pattern::Variant { variant, .. } if variant == "Some"));

    let q = crate::parser::parse_pattern_str("Box { w, h }");
    assert!(
        matches!(q, Pattern::Variant { sub: VariantPattern::Named { ref fields, ignore_rest: false }, .. } if fields.len() == 2)
    );

    let r = crate::parser::parse_pattern_str("Box { w, .. }");
    assert!(matches!(
        r,
        Pattern::Variant {
            sub: VariantPattern::Named {
                ignore_rest: true,
                ..
            },
            ..
        }
    ));

    assert!(matches!(
        crate::parser::parse_pattern_str("_"),
        Pattern::Wildcard(_)
    ));
    assert!(matches!(
        crate::parser::parse_pattern_str("v"),
        Pattern::Binding { .. }
    ));
}

#[test]
fn parses_value_patterns() {
    let p = crate::parser::parse_pattern_str;
    for lit in [r#""+""#, "3", "-1", "2.5", "-0.5", "true", ":red"] {
        assert!(matches!(p(lit), Pattern::Literal { .. }), "{lit}");
    }
    assert!(matches!(p("0..10"), Pattern::Range { .. }));
    assert!(matches!(p("-5..5"), Pattern::Range { .. }));
    let Pattern::Concat { parts, .. } = p(r#"key .. "=" .. value"#) else {
        panic!()
    };
    assert!(matches!(
        parts.as_slice(),
        [
            Pattern::Binding { .. },
            Pattern::Literal { .. },
            Pattern::Binding { .. }
        ]
    ));
    let Pattern::Concat {
        parts,
        template: true,
        ..
    } = p(r#""give ${item} to ${_}""#)
    else {
        panic!()
    };
    assert!(matches!(
        parts.as_slice(),
        [
            Pattern::Literal { .. },
            Pattern::Binding { .. },
            Pattern::Literal { .. },
            Pattern::Wildcard(_)
        ]
    ));
    assert!(matches!(p(r#""${x}""#), Pattern::Binding { .. }));
    let Pattern::Or { alts, .. } = p(r#""a" | "b" | _"#) else {
        panic!()
    };
    assert_eq!(alts.len(), 3);
    let Pattern::Or { alts, .. } = p("A\n  | B") else {
        panic!()
    };
    assert_eq!(alts.len(), 2);
    let Pattern::Variant {
        sub: VariantPattern::Positional(subs),
        ..
    } = p(r#"Cmd("go " .. d | "stop")"#)
    else {
        panic!()
    };
    assert!(matches!(subs.as_slice(), [Pattern::Or { .. }]));
}

#[test]
fn parses_a_match_guard() {
    let src = "in x: int\nout o = match x {\n  n if n > 3 => 1,\n  _ => 2,\n}\n";
    let r = parse(src, "t.ws");
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let TopDecl::Out(o) = &r.ast.decls[1] else {
        panic!()
    };
    let Some(Expr::MatchExpr { arms, .. }) = &o.value else {
        panic!()
    };
    assert!(arms[0].guard.is_some());
    assert!(arms[1].guard.is_none());
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
    let Some(Expr::MatchExpr {
        scrutinee, arms, ..
    }) = &o.value
    else {
        panic!("out value should be a MatchExpr, got {:?}", o.value)
    };
    assert!(
        matches!(scrutinee.as_ref(), Expr::FieldAccess { field, .. } if field == "field"),
        "scrutinee should be obj.field: {scrutinee:?}"
    );
    assert_eq!(arms.len(), 2, "{arms:?}");
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
        panic!(
            "expected an IfLet statement, got {:?}",
            handler.body.stmts[0]
        )
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
        panic!(
            "expected an IfLet statement, got {:?}",
            handler.body.stmts[0]
        )
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
    assert!(
        !r.diagnostics.is_empty(),
        "expected a diagnostic for the missing 'else'"
    );
}

// `is` variant test

#[test]
fn is_parses_as_a_variant_test() {
    // `value is Enum.Variant` is its own expression node, holding the value
    // and the variant path it names.
    let s =
        parse_ok("enum Shape { Empty, Circle(float) }\nin s: Shape\nout b = s is Shape.Circle\n");
    let out = s
        .decls
        .iter()
        .find_map(|d| match d {
            TopDecl::Out(o) => Some(o),
            _ => None,
        })
        .expect("out decl");
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
    let out = s
        .decls
        .iter()
        .find_map(|d| match d {
            TopDecl::Out(o) => Some(o),
            _ => None,
        })
        .expect("out decl");
    let Some(Expr::BinOp { op, left, .. }) = out.value.as_ref() else {
        panic!("expected a top-level BinOp, got {:?}", out.value)
    };
    assert_eq!(op, "&&");
    assert!(matches!(left.as_ref(), Expr::Is { .. }), "left: {:?}", left);
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
    let body = handler_body(
        "in go: exec
on go {
  let d = SleepTicks(emit, delay = 2)
}
",
    );
    let Some(Stmt::Let(l)) = body.first() else {
        panic!("expected a let, got {:?}", body.first())
    };
    let Expr::Call { args, .. } = &l.value else {
        panic!("expected a call, got {:?}", l.value)
    };
    assert!(
        matches!(
            args.first(),
            Some(CallArg::Positional(Expr::CurrentExec { .. }))
        ),
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

/// A program nested past `MAX_NEST_DEPTH` is refused with one diagnostic
/// instead of overflowing the stack.
///
/// The three shapes each reach depth a different way: nested parens by
/// parser recursion, a `+` chain by left-nesting one `BinOp` per operand
/// with no deep recursion at all, and an `else if` ladder by nesting an
/// `If` per arm. All three overflow a 2 MiB stack, the size tokio gives
/// the worker an editor's analysis runs on, and an overflow aborts the
/// process: `catch_unwind` cannot turn it into a diagnostic.
#[test]
fn nesting_past_the_depth_limit_is_a_diagnostic_not_a_crash() {
    let n = crate::parser::MAX_NEST_DEPTH + 50;
    let deep = [
        format!("on t() {{\n  o = {}1{}\n}}\n", "(".repeat(n), ")".repeat(n)),
        format!(
            "on t() {{\n  o = {}\n}}\n",
            (0..n)
                .map(|i| (i % 7).to_string())
                .collect::<Vec<_>>()
                .join(" + ")
        ),
        format!(
            "in c: int\non t() {{\n{}{{ o = 0 }}\n}}\n",
            (0..n)
                .map(|i| format!("  if c == {i} {{ o = {i} }} else "))
                .collect::<String>()
        ),
    ];
    for src in &deep {
        let p = parse(src, "t");
        let depth_errs: Vec<_> = p
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("nests more than"))
            .collect();
        assert_eq!(
            depth_errs.len(),
            1,
            "one message, not a cascade: {:?}",
            p.diagnostics
        );
    }
    // Just under the limit still parses clean.
    let ok = format!(
        "on t() {{\n  o = {}\n}}\n",
        (0..crate::parser::MAX_NEST_DEPTH - 10)
            .map(|i| (i % 7).to_string())
            .collect::<Vec<_>>()
            .join(" + ")
    );
    assert!(
        parse(&ok, "t").diagnostics.is_empty(),
        "{:?}",
        parse(&ok, "t").diagnostics
    );
}

/// An interpolation slot holds ONE expression, and says so when it does
/// not: a sub-parser that reads one expression and stops silently discards
/// the rest of the slot.
#[test]
fn leftover_tokens_in_an_interpolation_slot_are_reported() {
    let r = parse("in n: int\nout s = \"x ${n GARBAGE} y\"\n", "t");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.message.contains("after the expression in `${...}`")),
        "{:?}",
        r.diagnostics
    );
    // A well-formed slot is untouched.
    assert!(
        parse("in n: int\nout s = \"x ${n + 1} y\"\n", "t")
            .diagnostics
            .is_empty()
    );
}

/// A span inside `${...}` is the span in the FILE, whatever shape the
/// sub-expression is.
///
/// The alternative is lexing the slot on its own and walking the tree
/// afterwards to shift its positions, which needs an arm per expression
/// shape: a record literal, an `is`, an `unsafe`, a block expression or a
/// match-arm body that the walker misses reports at line 1.
#[test]
fn spans_inside_an_interpolation_are_file_positions() {
    // Each body is a shape the old shifting walker missed.
    for body in ["{ a: n }", "n is E.A", "unsafe n.A.f", "{ let t = n; t }"] {
        let src = format!("in n: int\n\n\n\nout s = \"v ${{ {body} }}\"\n");
        let r = parse(&src, "t");
        for d in &r.diagnostics {
            assert_eq!(
                d.range.start.line, 5,
                "`{body}` reported off its own line: {d:?}"
            );
        }
        let Some(TopDecl::Out(o)) = r.ast.decls.iter().find(|d| matches!(d, TopDecl::Out(_)))
        else {
            panic!("expected the `out` decl for `{body}`")
        };
        let Some(Expr::InterpLit { parts, .. }) = o.value.as_ref() else {
            panic!("expected an interpolation for `{body}`")
        };
        let slot = parts
            .iter()
            .find_map(|p| match p {
                crate::ast::InterpPart::Expr(e) => Some(e),
                _ => None,
            })
            .expect("the `${...}` slot");
        assert_eq!(
            slot.range().start.line,
            5,
            "`{body}` span is not on its own line"
        );
    }
}

/// A `${...}` slot is lexed at its position in the file, and the lexer's
/// own source slicing stays fragment-local.
///
/// Seeding the lexer's reported positions is what makes an interpolation's
/// spans right; seeding the offsets it INDEXES WITH aborts the compile,
/// since a slot late in a file would slice its short fragment at the
/// file offset.
#[test]
fn an_interpolation_slot_containing_a_string_lexes() {
    let pad = "// filler\n".repeat(30);
    for body in ["s.Split(\"\\n\").Left", "\"inner\"", "f(\"a\", \"b\")"] {
        let src = format!("{pad}in s: string\nout m = \"x ${{{body}}} y\"\n");
        let r = parse(&src, "t");
        let interp = r.diagnostics.iter().find(|d| d.code == "WSP001");
        assert!(interp.is_none(), "`{body}`: {:?}", r.diagnostics);
    }
}

/// A dangling operator before a closing token does not swallow the rest of
/// the file.
///
/// `parse_primary`'s error arm consumed whatever token it failed on,
/// including the `}` that closes the block around it: the block then ran
/// to EOF looking for its terminator and every declaration after it was
/// never parsed at all, so nothing in the rest of the file was checked.
#[test]
fn a_dangling_operator_does_not_swallow_the_rest_of_the_file() {
    let r = parse(
        "in go: exec\nvar n: int\non go {\n  n = 1 +\n}\nmod later() -> (r: int) { return 42 }\n",
        "t",
    );
    assert!(
        r.ast
            .decls
            .iter()
            .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "later")),
        "`mod later` must still parse: {:?}",
        r.ast.decls.len()
    );
    // And the recovery reports the operator alone, not a phantom missing
    // brace at EOF.
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.message.contains("expected RBrace")),
        "{:?}",
        r.diagnostics
    );
}

/// Leaving a closing token in place must not let any recovery loop spin.
///
/// Every prefix AND every suffix of a program with a dangling operator
/// before each bracket kind is parsed. The suffixes are the half that can
/// reach the top-level loop: a prefix of a valid program always begins
/// with a well-formed declaration, so `parse_script` is never handed a
/// stray closing token in first position, and a prefix-only sweep stayed
/// green while 139 of these suffixes spun.
///
/// The sweep runs on its own thread with a deadline because the failure
/// mode is a loop that allocates a declaration per iteration: without one
/// this reports as a CI job that never finishes and names no test.
#[test]
fn recovery_terminates_on_every_truncated_prefix_and_suffix() {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let sweep = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let programs = [
                    "in go: exec\nvar n: int\non go {\n  n = 1 +\n}\nmod m() -> (r: int) { return 1 }\n",
                    "in a: int\nout r = f(a +\n)\n",
                    "var xs: int[] = [1, 2 +\n]\n",
                    "type P = { x: int }\nvar p: P\nin go: exec\non go { p = { x: 1 + } }\n",
                    "in go: exec\non go { let m = { 1 => 2 + } }\n",
                    "enum E { A }\nin go: exec\nvar s: E\nvar n: int\non go { n = match s { E.A => 1 + } }\n",
                    // Shapes a user types by accident, each of which spun the
                    // top-level loop: a lone closer, one brace too many, a
                    // match in a top-level `out`, and a typed parameter on a
                    // name that is not a catalog event (which parses as a call
                    // expression and leaves the `)` in place).
                    ")\n",
                    "on Clock() {}\n}\n",
                    "out y = match 1 { 1 => 2 }\n",
                    "on Foo(x: int) {}\n",
                    // `{` in VALUE position reaches `parse_block_expr`, a
                    // third loop in this family that none of the programs
                    // above touch: every `{` in them opens a block, record,
                    // map or match. Each of these is balanced, so no
                    // truncation is needed to spin it: a `,` where the
                    // block expression wants `}` consumes nothing.
                    "{,",
                    "in go: exec\non go { let y = { 1, 2 } }\n",
                    "mod m() -> (r: int) { enum E { A { f: bool }, B(bool) } }\n",
                ];
                for src in programs {
                    for cut in 0..=src.len() {
                        if !src.is_char_boundary(cut) {
                            continue;
                        }
                        let _ = parse(&src[..cut], "t");
                        let _ = parse(&src[cut..], "t");
                    }
                }
                let _ = done_tx.send(());
            })
            .expect("spawn sweep thread");
    let finished = done_rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .is_ok();
    assert!(
        finished,
        "a parser recovery loop did not terminate within 20s"
    );
    sweep.join().expect("sweep thread panicked");
}

// `for` / `break` grammar

/// Pull the single `for` statement out of the first handler in `src`.
fn only_for(src: &str) -> crate::ast::ForStmt {
    let parsed = crate::parser::parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    for d in &parsed.ast.decls {
        if let crate::ast::TopDecl::Handler(h) = d {
            for s in &h.body.stmts {
                if let Stmt::For(f) = s {
                    return f.clone();
                }
            }
        }
    }
    panic!("no `for` statement parsed from: {src}");
}

#[test]
fn for_range_binds_one_slot() {
    let f = only_for("in go: exec\non go { for i in 0..n { } }");
    assert_eq!(f.bindings.len(), 1);
    assert!(matches!(f.source, ForSource::Range(_)));
    assert!(f.result.is_none());
}

#[test]
fn for_range_step_and_rev_parse() {
    let f = only_for("in go: exec\non go { for i in (a..b).step(2) { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range")
    };
    assert!(r.step.is_some());
    assert!(!r.reversed);

    let f = only_for("in go: exec\non go { for i in (0..n).rev() { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range")
    };
    assert!(r.reversed);
}

/// Every `WS083` a parse of `src` reports, message included.
fn range_method_diags(src: &str) -> Vec<String> {
    crate::parser::parse(src, "test")
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "WS083")
        .map(|d| d.message)
        .collect()
}

#[test]
fn a_range_method_on_a_collection_is_ws083() {
    // Peeling the wrapper discards it, so without this the loop would run
    // forward with no diagnostic anywhere.
    for (src, method) in [
        ("in go: exec\non go { for v in xs.rev() { } }", "rev"),
        ("in go: exec\non go { for v in xs.step(2) { } }", "step"),
        ("in go: exec\non go { for k, v in m.rev() { } }", "rev"),
    ] {
        let diags = range_method_diags(src);
        assert_eq!(diags.len(), 1, "expected one WS083 for {src}: {diags:?}");
        assert!(
            diags[0].contains(&format!("`.{method}()`")),
            "the message must name the method that does not apply: {diags:?}"
        );
    }
}

#[test]
fn a_repeated_range_method_is_ws083() {
    let diags = range_method_diags("in go: exec\non go { for i in (0..10).step(2).step(3) { } }");
    assert_eq!(diags.len(), 1, "expected one WS083: {diags:?}");
    assert!(diags[0].contains("one `.step()`"), "{diags:?}");

    let diags = range_method_diags("in go: exec\non go { for i in (0..10).rev().rev() { } }");
    assert_eq!(diags.len(), 1, "expected one WS083: {diags:?}");
    assert!(diags[0].contains("one `.rev()`"), "{diags:?}");
}

#[test]
fn a_range_method_on_a_range_is_accepted() {
    for src in [
        "in go: exec\non go { for i in (0..10).step(2) { } }",
        "in go: exec\non go { for i in (0..10).rev() { } }",
        "in go: exec\non go { for i in (0..10).step(2).rev() { } }",
    ] {
        assert!(
            range_method_diags(src).is_empty(),
            "a range method on a range must be accepted: {src}"
        );
    }
}

/// `..` is tied with `+`/`-` at precedence 10 and is left-associative, so
/// parsing a `for` source's bare range the ordinary way groups `0..n-1`
/// as `(0..n) - 1`. The range then never forms and falls back to
/// `ForSource::Collection`.
#[test]
fn for_range_bound_arithmetic_groups_before_the_range_splits() {
    let f = only_for("in go: exec\non go { for i in 0..n-1 { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range, got {:?}", f.source)
    };
    let RangeExtent::Bounds { start, end } = &r.extent else {
        panic!("expected bounds")
    };
    assert!(matches!(start, crate::ast::Expr::IntLit { value: 0, .. }));
    assert!(matches!(end, crate::ast::Expr::BinOp { op, .. } if op == "-"));

    let f = only_for("in go: exec\non go { for i in start+1..end-1 { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range, got {:?}", f.source)
    };
    let RangeExtent::Bounds { start, end } = &r.extent else {
        panic!("expected bounds")
    };
    assert!(matches!(start, crate::ast::Expr::BinOp { op, .. } if op == "+"));
    assert!(matches!(end, crate::ast::Expr::BinOp { op, .. } if op == "-"));
}

#[test]
fn for_count_spelling_parses() {
    let f = only_for("in go: exec\non go { for i in count(n, from = 5, step = 2) { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range")
    };
    assert!(matches!(r.extent, RangeExtent::Count { .. }));
    assert!(r.step.is_some());
}

#[test]
fn for_two_bindings_over_a_collection() {
    let f = only_for("var arr: int[]\nin go: exec\non go { for i, v in arr { } }");
    assert_eq!(f.bindings.len(), 2);
    assert!(matches!(f.source, ForSource::Collection { .. }));
}

#[test]
fn for_record_pattern_and_ignore_slot() {
    let f = only_for("var pts: Point[]\nin go: exec\non go { for _, { x, y } in pts { } }");
    assert_eq!(f.bindings.len(), 2);
    assert!(matches!(f.bindings[0], crate::ast::ForPat::Ignore(_)));
    assert!(matches!(f.bindings[1], crate::ast::ForPat::Record { .. }));
}

#[test]
fn let_for_binds_a_result_name() {
    let f = only_for("in go: exec\non go { let run = for i in 0..n { } }");
    assert_eq!(f.result.as_ref().map(|(n, _)| n.as_str()), Some("run"));
}

#[test]
fn the_retired_for_as_form_says_how_to_write_it() {
    let parsed = crate::parser::parse("in go: exec\non go { for i in 0..n as run { } }", "test");
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("let run = for")),
        "parse diags: {:?}",
        parsed.diagnostics
    );
}

#[test]
fn a_bitwise_or_after_as_stays_an_operator() {
    use crate::ast::{Expr, TopDecl};
    let parsed = crate::parser::parse("let y = a as int | mask", "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let TopDecl::Let(d) = &parsed.ast.decls[0] else {
        panic!("a let")
    };
    assert!(
        matches!(&d.value, Expr::BinOp { op, .. } if op == "|"),
        "{:?}",
        d.value
    );
}

#[test]
fn as_binds_tighter_than_binary_operators() {
    use crate::ast::{Expr, TopDecl};
    let parsed = crate::parser::parse("let y = a + -b as float * 2.0", "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let TopDecl::Let(d) = &parsed.ast.decls[0] else {
        panic!("a let")
    };
    // a + ((-b as float) * 2.0)
    let Expr::BinOp { op, right, .. } = &d.value else {
        panic!("{:?}", d.value)
    };
    assert_eq!(op, "+");
    let Expr::BinOp { op, left, .. } = right.as_ref() else {
        panic!("{right:?}")
    };
    assert_eq!(op, "*");
    let Expr::As { value, .. } = left.as_ref() else {
        panic!("{left:?}")
    };
    assert!(
        !matches!(value.as_ref(), Expr::BinOp { .. } | Expr::As { .. }),
        "{value:?}"
    );
}

/// A bare range endpoint gets the same newline skip as any other infix
/// operator, so a `for` header can split at the range like a long `+`/`-` chain.
#[test]
fn for_range_bare_dotdot_continues_across_a_line_break() {
    fn endpoints(src: &str) -> (crate::ast::Expr, crate::ast::Expr) {
        let f = only_for(src);
        let ForSource::Range(r) = f.source else {
            panic!("expected a range, got {:?}", f.source)
        };
        let RangeExtent::Bounds { start, end } = r.extent else {
            panic!("expected bounds")
        };
        (start, end)
    }

    // Position spans differ across the three forms below, so compare the
    // endpoints' shape rather than the whole node.
    let (s, e) = endpoints("in go: exec\non go { for i in start..end { } }");
    assert!(matches!(s, crate::ast::Expr::Ident { name, .. } if name == "start"));
    assert!(matches!(e, crate::ast::Expr::Ident { name, .. } if name == "end"));

    // Split before `..` - the shape the formatter actually produces.
    let (s, e) = endpoints("in go: exec\non go { for i in start\n  ..end { } }");
    assert!(matches!(s, crate::ast::Expr::Ident { name, .. } if name == "start"));
    assert!(matches!(e, crate::ast::Expr::Ident { name, .. } if name == "end"));

    // Split after `..`.
    let (s, e) = endpoints("in go: exec\non go { for i in start..\n  end { } }");
    assert!(matches!(s, crate::ast::Expr::Ident { name, .. } if name == "start"));
    assert!(matches!(e, crate::ast::Expr::Ident { name, .. } if name == "end"));
}

#[test]
fn for_range_step_and_rev_continue_across_a_line_break() {
    let f = only_for("in go: exec\non go { for i in (a\n  ..b).step(2) { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range, got {:?}", f.source)
    };
    assert!(r.step.is_some());

    let f = only_for("in go: exec\non go { for i in (0\n  ..n).rev() { } }");
    let ForSource::Range(r) = &f.source else {
        panic!("expected a range, got {:?}", f.source)
    };
    assert!(r.reversed);
}

#[test]
fn for_single_binding_over_a_collection_on_one_line_is_unaffected() {
    let f = only_for("var arr: int[]\nin go: exec\non go { for v in arr { } }");
    assert_eq!(f.bindings.len(), 1);
    assert!(matches!(f.bindings[0], crate::ast::ForPat::Name(ref n, _) if n == "v"));
    assert!(matches!(f.source, ForSource::Collection { .. }));
}

/// `t` written back as source, for comparing a parsed type's structure.
fn type_shape(t: &TypeExpr) -> String {
    let list = |ts: &[TypeExpr], sep: &str| ts.iter().map(type_shape).collect::<Vec<_>>().join(sep);
    match t {
        TypeExpr::Name { name, .. } => name.clone(),
        TypeExpr::Ref { inner, .. } => format!("ref {}", type_shape(inner)),
        TypeExpr::Array { inner, .. } => format!("{}[]", type_shape(inner)),
        TypeExpr::Tuple { fields, .. } => format!("({})", list(fields, ", ")),
        TypeExpr::Union { options, .. } => list(options, " | "),
        TypeExpr::Generic { name, args, .. } => format!("{name}<{}>", list(args, ", ")),
        TypeExpr::Record { .. } => "{record}".to_string(),
    }
}

/// `>>` closes two nested type-argument lists, as `> >` does, in a type
/// annotation, a type-parameter bound and an explicit call type argument,
/// while `>>` between two operands stays the shift operator.
#[test]
fn a_double_angle_closes_nested_type_arguments() {
    let s = parse_ok(
        "var m: Map<string, Map<string, int>>\n\
             var a: Array<Array<int>>\n\
             mod f<T: Map<int, Array<int>>>(x: T) -> int { return 1 }\n\
             var s = 8 >> 1\n\
             var c = a < b >> 2\n\
             var n = g<Array<int>>(a)",
    );
    let typ = |i: usize| match &s.decls[i] {
        TopDecl::Var(v) => type_shape(v.typ.as_ref().expect("typed")),
        _ => panic!("expected Var"),
    };
    assert_eq!(typ(0), "Map<string, Map<string, int>>");
    assert_eq!(typ(1), "int[][]");
    let TopDecl::Chip(f) = &s.decls[2] else {
        panic!("expected the mod: {:?}", s.decls[2])
    };
    assert_eq!(
        f.type_params[0].bound.as_ref().map(type_shape).as_deref(),
        Some("Map<int, int[]>")
    );
    let init = |i: usize| match &s.decls[i] {
        TopDecl::Var(v) => v.init.clone().expect("init"),
        _ => panic!("expected Var"),
    };
    assert!(matches!(init(3), Expr::BinOp { op, .. } if op == ">>"));
    match init(4) {
        Expr::BinOp { op, right, .. } => {
            assert_eq!(op, "<");
            assert!(matches!(*right, Expr::BinOp { ref op, .. } if op == ">>"));
        }
        other => panic!("expected `a < (b >> 2)`, got {other:?}"),
    }
    assert!(matches!(init(5), Expr::Call { ref type_args, .. } if type_args.len() == 1));
}

/// A type-argument list closed right before an initializer's `=` lexes as
/// `>=` or `>>=`: the closing `>` splits off, and the rest is the `=`.
/// Expression `>=` and `>>=` are untouched.
#[test]
fn an_angle_before_an_initializer_closes_the_type_arguments() {
    let s = parse_ok(
        "var m: Map<string, Map<string, int>>= {}\n\
             var n: Map<string, int>= {}\n\
             var a: int[] = []\n\
             var b = a < c >= d\n\
             in go: exec\n\
             on go { x >>= 1 }",
    );
    let typ = |i: usize| match &s.decls[i] {
        TopDecl::Var(v) => type_shape(v.typ.as_ref().expect("typed")),
        _ => panic!("expected Var"),
    };
    assert_eq!(typ(0), "Map<string, Map<string, int>>");
    assert_eq!(typ(1), "Map<string, int>");
    let init = |i: usize| match &s.decls[i] {
        TopDecl::Var(v) => v.init.clone().expect("init"),
        _ => panic!("expected Var"),
    };
    assert!(matches!(init(0), Expr::RecordLit { .. }));
    match init(3) {
        Expr::BinOp { op, left, .. } => {
            assert_eq!(op, ">=");
            assert!(matches!(*left, Expr::BinOp { ref op, .. } if op == "<"));
        }
        other => panic!("expected `(a < c) >= d`, got {other:?}"),
    }
}

#[test]
fn comparison_as_a_bitwise_operand_is_ws117() {
    // `&`, `|` and `^` bind looser than a comparison, so the comparison runs
    // first: `code & 4 == 4` is `code & (4 == 4)`.
    // A warning, not an error: the code still means something.
    for src in [
        "out o = code & 4 == 4",
        "out o = code == 4 | 1",
        "out o = a ^ b != c",
        "out o = a & b < c",
    ] {
        let r = crate::parser::parse(src, "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS117" && d.severity == crate::diagnostic::Severity::Warning),
            "{src}: {:?}",
            r.diagnostics
        );
    }
    // Parentheses say which was meant, either way.
    for src in [
        "out o = (code & 4) == 4",
        "out o = code & (4 == 4)",
        "out o = (a == 1) | (a == 2)",
        "out o = a == b && c",
        "out o = a & b",
    ] {
        let r = crate::parser::parse(src, "test");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS117"),
            "{src}: {:?}",
            r.diagnostics
        );
    }
}
