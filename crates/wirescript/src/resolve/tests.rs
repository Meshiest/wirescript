use super::*;

/// Resolution produced no hard error. Most of these tests then assert what
/// the import PULLED IN, which is meaningless if resolution failed first.
fn assert_resolves(r: &ResolveResult) {
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.severity == crate::diagnostic::Severity::Error),
        "errors: {:?}",
        r.diagnostics
    );
}

fn mem(files: &[(&str, &str)]) -> MemLoader {
    MemLoader {
        files: files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[derive(Debug, Clone, Copy)]
enum Decl {
    Chip,
    Var,
    Namespace,
    Enum,
}

fn has_decl(r: &ResolveResult, kind: Decl, name: &str) -> bool {
    r.ast.decls.iter().any(|d| match (kind, d) {
        (Decl::Chip, TopDecl::Chip(c)) => c.name == name,
        (Decl::Var, TopDecl::Var(v)) => v.name == name,
        (Decl::Namespace, TopDecl::Namespace(n)) => n.name == name,
        (Decl::Enum, TopDecl::Enum(e)) => e.name == name,
        _ => false,
    })
}

#[test]
fn each_import_spelling_brings_in_what_it_names() {
    use Decl::*;
    const FOO: &str = "mod foo(x: *int) { x = x + 1 }";
    const SHAPE: &str = "enum Shape { Empty, Circle(float) }\n";
    // (label, lib.ws, entry, declared, not declared)
    let rows: &[(&str, &str, &str, &[(Decl, &str)], &[(Decl, &str)])] = &[
        (
            "whole module: a var and a mod, `.ws` implied",
            "var x: int = 0\nmod foo(x: *int) { x = x + 1 }",
            r#"import "lib""#,
            &[(Var, "x"), (Chip, "foo")],
            &[],
        ),
        (
            "named",
            "mod foo(x: *int) { x = x + 1 }\nmod bar(x: *int) { x = x - 1 }",
            r#"import { foo } from "lib""#,
            &[(Chip, "foo")],
            &[(Chip, "bar")],
        ),
        (
            "aliased mod",
            FOO,
            r#"import { foo as inc } from "lib""#,
            &[(Chip, "inc")],
            &[],
        ),
        (
            "namespace",
            FOO,
            r#"import * as myLib from "lib""#,
            &[(Namespace, "myLib")],
            &[],
        ),
        (
            "aliased var",
            "var counter: int = 0",
            r#"import { counter as cnt } from "lib""#,
            &[(Var, "cnt")],
            &[],
        ),
        // An `enum` has to be in both `is_importable` and `decl_name`, or a
        // library can declare one but never hand it out.
        (
            "whole module enum",
            SHAPE,
            r#"import "lib""#,
            &[(Enum, "Shape")],
            &[],
        ),
        (
            "named enum",
            SHAPE,
            r#"import { Shape } from "lib""#,
            &[(Enum, "Shape")],
            &[],
        ),
    ];
    let mut failures = Vec::new();
    for (label, lib, entry, present, absent) in rows {
        let r = resolve(entry, "main.ws", &mem(&[("lib.ws", *lib)]));
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        let missing: Vec<_> = present
            .iter()
            .filter(|(k, n)| !has_decl(&r, *k, n))
            .collect();
        let leaked: Vec<_> = absent.iter().filter(|(k, n)| has_decl(&r, *k, n)).collect();
        if !errors.is_empty() || !missing.is_empty() || !leaked.is_empty() {
            failures.push(format!(
                "{label}: errors {errors:?}, missing {missing:?}, leaked {leaked:?}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn import_errors_are_reported() {
    // (label, files, entry, code, message fragment)
    let rows: &[(&str, &[(&str, &str)], &str, &str, &str)] = &[
        (
            "circular",
            &[("a.ws", r#"import "b""#), ("b.ws", r#"import "a""#)],
            r#"import "a""#,
            "WS012",
            "circular",
        ),
        (
            "missing file",
            &[],
            r#"import "nonexistent""#,
            "WS012",
            "cannot resolve",
        ),
        (
            "missing symbol",
            &[("lib.ws", "mod foo(x: *int) { x = x + 1 }")],
            r#"import { bar } from "lib""#,
            "WS012",
            "not found",
        ),
        // A parse error in an imported module is reported as it is for an
        // entry file.
        (
            "parse error in an imported file",
            &[("broken.ws", "array xs: int[]")],
            r#"import "broken""#,
            "WSP001",
            "`array` declarations have been removed",
        ),
    ];
    let mut failures = Vec::new();
    for (label, files, entry, code, fragment) in rows {
        let r = resolve(entry, "main.ws", &mem(files));
        let hit = r.diagnostics.iter().any(|d| {
            d.code == *code
                && d.message.contains(fragment)
                && d.severity == crate::diagnostic::Severity::Error
        });
        if !hit {
            failures.push(format!(
                "{label}: no {code} error containing {fragment:?}: {:?}",
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// An import read only in one of these places still counts as used, so it
/// is not WS014 "unused import" (which Organize Imports acts on by deleting
/// the import).
#[test]
fn an_import_read_anywhere_is_not_reported_unused() {
    const CHIP_HANDLER: &str = "chip Inner() -> (n: int) {\n\
             var m: int = 0\n\
             on CustomEvent(CH) -> (v: int) { m = v }\n\
             out n: int = m\n\
             }\n\
             let i = Inner()\n\
             out n: int = i.n";
    // (label, lib.ws, entry, names WS014 must report, names that must travel)
    let rows: Vec<(&str, &str, String, &[&str], &[&str])> = vec![
            (
                "a type used in a param annotation",
                "type Cpu = { regs: int[], cpsr: *int }",
                "import { Cpu } from \"lib\"\nmod foo(cpu: Cpu) { cpu.regs.push(0) }".into(),
                &[],
                &[],
            ),
            // The variant test is the only mention of `o`, so the usage scan has to
            // walk into it to see the read.
            (
                "read only by a variant test",
                "static var o: Option<int> = None",
                "import { o } from \"lib\"\nout hit = o is Option.Some".into(),
                &[],
                &[],
            ),
            // `on Clock(interval = TICK)` reads TICK in the handler's CONFIG args,
            // which the usage scan skipped (it only walked the handler body).
            (
                "read in an event config arg",
                "let TICK = 0.15",
                "import { TICK } from \"lib\"\non Clock(interval = TICK) { }".into(),
                &[],
                &[],
            ),
            // The same, nested in a chip body: the STATEMENT arm of the scan walked
            // only the body, so a channel constant used by a handler inside a chip
            // warned while the identical module-level handler did not.
            (
                "read in an event config arg inside a chip",
                "const CH = \"chan.a\"",
                format!("import {{ CH }} from \"lib\"\n{CHIP_HANDLER}"),
                &[],
                &[],
            ),
            // The wider scan must still catch a genuinely unused import.
            (
                "a genuinely unused import beside a chip handler",
                "const CH = \"chan.a\"\nconst UNUSED = 5",
                format!("import {{ CH, UNUSED }} from \"lib\"\n{CHIP_HANDLER}"),
                &["UNUSED"],
                &[],
            ),
            // The import walkers reached `if` and nothing else, so a name used only
            // in a `match` arm or an `if let` reported WS014, and the dependency
            // closure left the callee behind so the call lowered to `_Unsupported`.
            (
                "used only in a match arm",
                "const K = 7\nmod helper() -> (r: int) { return 1 }\n",
                "import { K, helper } from \"lib\"\nenum E { A, B }\nin go: exec\n\
                 var s: E\nvar n: int\non go {\n  s = E.A\n  n = match s { E.A => K, E.B => helper() }\n}\n"
                    .into(),
                &[],
                &["K", "helper"],
            ),
            (
                "used only in an if let",
                "const K = 7\nmod helper() -> (r: int) { return 1 }\n",
                "import { K, helper } from \"lib\"\nenum E { A, B }\nin go: exec\n\
                 var s: E\nvar n: int\non go {\n  s = E.A\n  if let E.B = s { n = K } else { n = helper() }\n}\n"
                    .into(),
                &[],
                &["K", "helper"],
            ),
        ];
    let mut failures = Vec::new();
    for (label, lib, entry, unused, travels) in &rows {
        let r = resolve(entry, "main.ws", &mem(&[("lib.ws", *lib)]));
        let ws014: Vec<&str> = r
            .diagnostics
            .iter()
            .filter(|d| d.code == "WS014")
            .map(|d| d.message.as_str())
            .collect();
        let names_each = unused.iter().all(|n| ws014.iter().any(|m| m.contains(n)));
        if ws014.len() != unused.len() || !names_each {
            failures.push(format!(
                "{label}: WS014 {ws014:?}, want one naming each of {unused:?}"
            ));
        }
        let stayed: Vec<_> = travels
            .iter()
            .filter(|n| !r.ast.decls.iter().any(|d| decl_binds(d, n)))
            .collect();
        if !stayed.is_empty() {
            failures.push(format!("{label}: {stayed:?} should have travelled"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
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
    assert_resolves(&r);
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
    let tc =
        crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
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
        (
            "cpu.ws",
            "import { Cpu } from \"types\"\nmod cpu_init(cpu: Cpu) { cpu.regs.push(0) }",
        ),
    ]);
    let r = resolve("import { cpu_init } from \"cpu\"", "main.ws", &loader);
    // The import itself must succeed, or the absence below proves nothing.
    assert_resolves(&r);
    assert!(
        r.ast.decls.iter().any(|d| decl_binds(d, "cpu_init")),
        "cpu_init was imported"
    );
    let has_type_alias = r
        .ast
        .decls
        .iter()
        .any(|d| matches!(d, TopDecl::TypeAlias(t) if t.name == "Cpu"));
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
        (
            "mid.ws",
            "import { deep } from \"deep\"\nlet mid = deep + 1",
        ),
        ("deep.ws", "let deep = 7"),
    ]);
    let r = resolve(
        "import { mid } from \"mid\"\nout o = mid",
        "main.ws",
        &loader,
    );
    assert!(
        r.diagnostics.is_empty(),
        "resolve diags: {:?}",
        r.diagnostics
    );
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
    let r = resolve(
        "import * as o from \"lib\"",
        "main.ws",
        &mem(&[("lib.ws", lib)]),
    );

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

    // Mutual recursion: `A` -> `B` -> back to `A`, which stops.
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

/// A named import brings only the names it lists, so a module's `on`
/// handlers stay behind, and it says so.
///
/// The spelling matters: a test harness imports a module's pure helpers
/// precisely so it does NOT install that module's event surface, while a
/// library written to install behaviour looks identical at the import
/// site. Hence a warning rather than either behaviour changing.
#[test]
fn a_named_import_leaves_handlers_behind_and_says_so() {
    let lib = "var counter: int = 0\non RoundStart() {\n  counter = 1\n}\n";
    // `import * as L` wraps everything it brought in a `Namespace`, so
    // the count has to look inside one.
    fn count(decls: &[TopDecl]) -> usize {
        decls
            .iter()
            .map(|d| match d {
                TopDecl::Handler(_) => 1,
                TopDecl::Namespace(n) => count(&n.decls),
                _ => 0,
            })
            .sum()
    }
    let resolved = |src: &str| {
        let loader = mem(&[("lib.ws", lib)]);
        resolve(src, "main.ws", &loader)
    };

    // The whole-module spellings install it.
    assert_eq!(count(&resolved(r#"import "lib""#).ast.decls), 1);
    assert_eq!(count(&resolved(r#"import * as L from "lib""#).ast.decls), 1);

    // The named one does not, and warns.
    let r = resolved(r#"import { counter } from "lib""#);
    assert_eq!(count(&r.ast.decls), 0);
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS014" && d.message.contains("`on` handler")),
        "the skipped handlers must be reported: {:?}",
        r.diagnostics
    );
}

/// A module reached by two import paths installs its handler once. The
/// diamond dedup is keyed on the names a declaration binds, and a handler
/// binds none, so it needs its own key.
#[test]
fn a_diamond_import_installs_a_handler_once() {
    let loader = mem(&[
        (
            "lib.ws",
            "var counter: int = 0\non RoundStart() {\n  counter = 1\n}\n",
        ),
        (
            "a.ws",
            "import \"lib\"\nmod ha() -> (r: int) { return counter }\n",
        ),
        (
            "b.ws",
            "import \"lib\"\nmod hb() -> (r: int) { return counter }\n",
        ),
    ]);
    let r = resolve("import \"a\"\nimport \"b\"\n", "main.ws", &loader);
    let handlers = r
        .ast
        .decls
        .iter()
        .filter(|d| matches!(d, TopDecl::Handler(_)))
        .count();
    assert_eq!(handlers, 1, "decls: {:?}", r.ast.decls.len());
}

/// Two modules that each bind the same top-level `let` collide, as they
/// already did for a `mod`/`chip`. Imports merge into one flat scope, so
/// otherwise one replaces the other and every reference in both files
/// reads whichever came last.
#[test]
fn a_cross_file_duplicate_let_is_a_diagnostic() {
    let loader = mem(&[
        ("a.ws", "let K = 1\nmod ga() -> (r: int) { return K }\n"),
        ("b.ws", "let K = 2\nmod gb() -> (r: int) { return K }\n"),
    ]);
    let r = resolve("import \"a\"\nimport \"b\"\n", "main.ws", &loader);
    let tc =
        crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
    assert!(
        tc.diagnostics
            .iter()
            .any(|d| d.code == "WS013" && d.message.contains("'K'")),
        "expected WS013 for the duplicate `let K`: {:?}",
        tc.diagnostics
    );
}

/// A top-level redeclaration (`const a` then `let a`) hides the earlier
/// declaration from its position on and warns `WS108`, naming both lines;
/// a `let` in a body may still shadow one before it, silently.
#[test]
fn a_top_level_redeclaration_is_ws108_and_a_body_let_may_shadow() {
    let loader = mem(&[]);
    let codes = |src: &str| {
        let r = resolve(src, "main.ws", &loader);
        let tc =
            crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        tc.diagnostics
            .iter()
            .map(|d| d.code.clone())
            .collect::<Vec<_>>()
    };
    for src in [
        "const a = 1\nlet a = 2\n",
        "let a = 1\nlet a = 2\n",
        "const a = 1\nconst a = 2\n",
    ] {
        assert_eq!(codes(src), ["WS108"], "{src}");
        let r = resolve(src, "main.ws", &loader);
        let tc =
            crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        let msg = &tc.diagnostics[0].message;
        assert!(
            msg.contains("line 1") && tc.diagnostics[0].range.start.line == 2,
            "{msg}"
        );
    }
    let body = "in go: exec\nvar n: int = 0\non go {\n let a = 1\n let a = a + 1\n n = a\n}\n";
    assert!(codes(body).is_empty(), "{:?}", codes(body));
}

/// Doc comments from different files do not overwrite one another.
///
/// Imports merge several parses into one map, so a key of byte offset
/// alone lets two declarations at the same offset in their own files
/// collide. A chip's doc comment is baked into the world as its header
/// text, so the loser comes out wearing the winner's title.
#[test]
fn doc_comments_from_two_files_do_not_collide() {
    // Both `mod` declarations start at the same byte offset in their own
    // file, which is exactly what an offset-only key cannot tell apart.
    let loader = mem(&[(
        "lib.ws",
        "/// FROM LIB\nmod helper() -> (r: int) { return 1 }\n",
    )]);
    let r = resolve(
        "import \"lib\"\n/// FROM MAIN\nmod other() -> (r: int) { return 2 }\n",
        "main.ws",
        &loader,
    );
    let docs: Vec<&String> = r.doc_comments.values().collect();
    assert!(
        docs.iter().any(|d| d.contains("FROM LIB")),
        "the library's doc survived: {docs:?}"
    );
    assert!(
        docs.iter().any(|d| d.contains("FROM MAIN")),
        "the entry file's doc survived: {docs:?}"
    );
    // Each is filed against its own declaration, not merely present.
    for d in &r.ast.decls {
        if let TopDecl::Chip(c) = d {
            let want = if c.name == "helper" {
                "FROM LIB"
            } else {
                "FROM MAIN"
            };
            let got = r.doc_comments.get(&crate::parser::doc_key(&c.range));
            assert!(
                got.is_some_and(|s| s.contains(want)),
                "`{}` should carry {want}, got {got:?}",
                c.name
            );
        }
    }
}

/// Two independently-authored modules that each define a private helper
/// under one name are a genuine collision, not a diamond. The dependency
/// closure used to skip the second module's helper because the FIRST
/// module's was already bound under that name, so `useB` silently called
/// `ma`'s `helper` and the answer depended on import order.
#[test]
fn dependency_closure_keeps_two_modules_same_named_helpers_apart() {
    let loader = mem(&[
        (
            "ma.ws",
            "mod helper(x: int) -> int { return x + 100 }\n\
                 mod useA(x: int) -> int { return helper(x) }",
        ),
        (
            "mb.ws",
            "mod helper(x: int) -> int { return x * 2 }\n\
                 mod useB(x: int) -> int { return helper(x) }",
        ),
    ]);
    let r = resolve(
        "import { useA } from \"ma\"\n\
             import { useB } from \"mb\"\n\
             out ra: int = useA(3)\n\
             out rb: int = useB(3)",
        "main.ws",
        &loader,
    );
    let mut helper_files: Vec<String> = r
        .ast
        .decls
        .iter()
        .filter(|d| decl_binds(d, "helper"))
        .map(|d| d.range().file.to_string())
        .collect();
    helper_files.sort();
    assert_eq!(
        helper_files,
        vec!["ma.ws".to_string(), "mb.ws".to_string()],
        "each module's own private helper must travel with it"
    );
    let tc =
        crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
    assert!(
        tc.diagnostics
            .iter()
            .any(|d| d.code == "WS013" && d.message.contains("helper")),
        "two modules' helpers collide in the flat merged scope, which must \
             be a loud WS013 rather than a silent alias: {:?}",
        tc.diagnostics
    );
}

/// Two libraries that each privately wrote `import * as U` cannot both be
/// pulled in flat: every `NamespaceDecl` shares one merged list keyed by
/// name, so lowering binds `U` once for the whole program and one
/// library's internal `U.f()` silently calls the other library's module.
#[test]
fn two_libraries_private_namespace_aliases_collide_and_report() {
    let loader = mem(&[
        ("u2.ws", "mod f(x: int) -> int { return x * 3 }"),
        ("u3.ws", "mod f(x: int) -> int { return x - 7 }"),
        (
            "mid.ws",
            "import * as U from \"u2\"\n\
                 mod g(x: int) -> int { return U.f(x) }",
        ),
        (
            "mid3.ws",
            "import * as U from \"u3\"\n\
                 mod h(x: int) -> int { return U.f(x) }",
        ),
    ]);
    let r = resolve(
        "import { g } from \"mid\"\n\
             import { h } from \"mid3\"\n\
             out a: int = g(1)\n\
             out b: int = h(1)",
        "main.ws",
        &loader,
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS012"
            && d.message.contains("namespace alias 'U'")
            && d.message.contains("u2")
            && d.message.contains("u3")),
        "expected one WS012 naming both modules, got {:?}",
        r.diagnostics
    );
    assert_eq!(
        r.diagnostics.iter().filter(|d| d.code == "WS012").count(),
        1,
        "one report per colliding alias, not one per fixpoint sweep: {:?}",
        r.diagnostics
    );
}

/// The same collision reached the other way round: the importer writes the
/// alias itself and a library it imports privately uses the same one.
/// Reported in either import order, because neither ordering makes the two
/// coexist.
#[test]
fn a_travelled_namespace_alias_collides_with_the_importers_own() {
    let files = &[
        ("u1.ws", "mod f(x: int) -> int { return x + 1000 }"),
        ("u2.ws", "mod f(x: int) -> int { return x * 3 }"),
        (
            "mid.ws",
            "import * as U from \"u2\"\n\
                 mod g(x: int) -> int { return U.f(x) }",
        ),
    ];
    for src in [
        "import * as U from \"u1\"\n\
             import { g } from \"mid\"\n\
             out a: int = U.f(1)\n\
             out b: int = g(1)",
        "import { g } from \"mid\"\n\
             import * as U from \"u1\"\n\
             out a: int = U.f(1)\n\
             out b: int = g(1)",
    ] {
        let loader = mem(files);
        let r = resolve(src, "main.ws", &loader);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS012" && d.message.contains("'U'")),
            "expected a WS012 for the alias collision in:\n{src}\ngot {:?}",
            r.diagnostics
        );
    }
}

/// The boundary of the check above: `import * as` wraps what it pulls in a
/// `NamespaceDecl`, and lowering scopes a namespaced mod's body against its
/// own module's aliases first, so the importer and the module it imports
/// genuinely may spell their alias the same. Reporting that would be a
/// false error on a program that works today.
#[test]
fn a_namespace_import_may_reuse_the_alias_its_own_module_uses() {
    let loader = mem(&[
        ("leaf.ws", "let value = 10"),
        (
            "mid.ws",
            "import * as Other from \"leaf\"\n\
                 let value = Other.value",
        ),
    ]);
    let r = resolve(
        "import * as Other from \"mid\"\n\
             out z: int = Other.value",
        "main.ws",
        &loader,
    );
    assert!(
        r.diagnostics.is_empty(),
        "a namespaced module's own alias is scoped to its bodies: {:?}",
        r.diagnostics
    );
}

/// One module reached under one alias from two different files is a
/// redundancy, not a collision: both aliases name the same module, so
/// nothing can retarget.
#[test]
fn one_module_under_one_alias_from_two_files_is_not_a_conflict() {
    let loader = mem(&[
        ("u2.ws", "mod f(x: int) -> int { return x * 3 }"),
        (
            "mid.ws",
            "import * as U from \"u2\"\n\
                 mod g(x: int) -> int { return U.f(x) }",
        ),
    ]);
    let r = resolve(
        "import * as U from \"u2\"\n\
             import { g } from \"mid\"\n\
             out a: int = U.f(1)\n\
             out b: int = g(1)",
        "main.ws",
        &loader,
    );
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS012"),
        "one module under one alias is redundant, not a collision: {:?}",
        r.diagnostics
    );
}

/// `import { addK, K as KK }` must not unbind the module's own `K`. The
/// alias renames the single clone that gets pushed, so the exporting
/// module's `x + K` has nothing left to resolve against unless the closure
/// pass also pulls the original, which it can only do when the "already
/// contributed" set is keyed on the post-alias name.
#[test]
fn aliasing_an_imported_constant_keeps_the_modules_own_binding() {
    let loader = mem(&[(
        "klib.ws",
        "let K = 5\n\
             mod addK(x: int) -> int { return x + K }",
    )]);
    let r = resolve(
        "import { addK, K as KK } from \"klib\"\n\
             out r: int = addK(1)\n\
             out kk: int = KK",
        "main.ws",
        &loader,
    );
    assert!(
        r.ast.decls.iter().any(|d| decl_binds(d, "K")),
        "the module's own `K` must stay bound for its `x + K`"
    );
    assert!(
        r.ast.decls.iter().any(|d| decl_binds(d, "KK")),
        "the aliased view must be bound too"
    );
    let tc =
        crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
    let errors: Vec<_> = tc
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {errors:?}");
}

/// The silent half of the same bug: with the module's `K` unbound, an
/// importer that happens to declare its own `K` captured the library's
/// internals and the program compiled clean with the wrong wiring. Pulling
/// the module's `K` back in makes the real collision visible instead.
#[test]
fn aliasing_an_imported_constant_does_not_capture_the_importers_name() {
    let loader = mem(&[(
        "klib.ws",
        "let K = 5\n\
             mod addK(x: int) -> int { return x + K }",
    )]);
    let r = resolve(
        "import { addK, K as KK } from \"klib\"\n\
             in K: int\n\
             out r: int = addK(1)\n\
             out kk: int = KK",
        "main.ws",
        &loader,
    );
    let tc =
        crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS013"),
        "the importer's `K` and the module's `K` collide and must be \
             reported, not silently wired together: {:?}",
        tc.diagnostics
    );
}

/// `ws-ignore` suppression directives. A named import that leaves a
/// module's `on` handler behind warns WS014; that is the intended spelling
/// for a test harness, so the warning has to be silenceable per line.
#[test]
fn ws_ignore_directives_silence_what_they_name() {
    const HELPER: &[(&str, &str)] = &[(
        "lib.ws",
        "mod helper(x: *int) { x = x + 1 }\non ReadBrickGrid() { }",
    )];
    const TWO: &[(&str, &str)] = &[
        (
            "a.ws",
            "mod ha(x: *int) { x = x + 1 }\non ReadBrickGrid() { }",
        ),
        (
            "b.ws",
            "mod hb(x: *int) { x = x + 2 }\non ReadBrickGrid() { }",
        ),
    ];
    // (label, files, entry, WS014 count left; None means no diagnostic at all)
    let rows: &[(&str, &[(&str, &str)], &str, Option<usize>)] = &[
        (
            "ws-ignore-line trailing its line",
            HELPER,
            "import { helper } from \"lib\" // ws-ignore-line:WS014\nvar n: int = 0\non ReadBrickGrid() { helper(n) }",
            Some(0),
        ),
        (
            "ws-ignore-line on its own line covers the line below",
            HELPER,
            "// ws-ignore-line:WS014\nimport { helper } from \"lib\"\nvar n: int = 0\non ReadBrickGrid() { helper(n) }",
            Some(0),
        ),
        // Two imports, one annotated: the other's warning must survive.
        (
            "ws-ignore-line does not reach other lines",
            TWO,
            "import { ha } from \"a\" // ws-ignore-line:WS014\nimport { hb } from \"b\"\nvar n: int = 0\non ReadBrickGrid() { ha(n) hb(n) }",
            Some(1),
        ),
        (
            "ws-ignore-file covers every line",
            TWO,
            "// ws-ignore-file:WS014\nimport { ha } from \"a\"\nimport { hb } from \"b\"\nvar n: int = 0\non ReadBrickGrid() { ha(n) hb(n) }",
            Some(0),
        ),
        // Both the "handlers not included" warning and the "unused import" one.
        (
            "bare ws-ignore-file covers every code",
            HELPER,
            "// ws-ignore-file\nimport { helper } from \"lib\"\n",
            None,
        ),
        (
            "ws-ignore-line covers only the codes it names",
            HELPER,
            "import { helper } from \"lib\" // ws-ignore-line:WSP001\nvar n: int = 0\non ReadBrickGrid() { helper(n) }",
            Some(1),
        ),
        (
            "a code list after trailing prose",
            HELPER,
            "import { helper } from \"lib\" // handlers stay behind on purpose ws-ignore-line:WSP001,WS014\nvar n: int = 0\non ReadBrickGrid() { helper(n) }",
            Some(0),
        ),
        // `ws-ignore-line:` reads as a typo, not as "ignore everything here".
        (
            "an empty code list is inert",
            HELPER,
            "import { helper } from \"lib\" // ws-ignore-line:\nvar n: int = 0\non ReadBrickGrid() { helper(n) }",
            Some(1),
        ),
        // The directive lives in the imported module and the diagnostic is
        // reported against that module's file, so the entry file's own
        // suppressions have nothing to do with it.
        (
            "an imported file carries its own directives",
            &[
                (
                    "mid.ws",
                    "// ws-ignore-file:WS014\nimport { leaf } from \"leaf\"\nmod mid_helper(x: *int) { leaf(x) }",
                ),
                (
                    "leaf.ws",
                    "mod leaf(x: *int) { x = x + 1 }\non ReadBrickGrid() { }",
                ),
            ],
            "import { mid_helper } from \"mid\"\nvar n: int = 0\non ReadBrickGrid() { mid_helper(n) }",
            Some(0),
        ),
    ];
    let mut failures = Vec::new();
    for (label, files, entry, want) in rows {
        let r = resolve(entry, "main.ws", &mem(files));
        let ws014 = r.diagnostics.iter().filter(|d| d.code == "WS014").count();
        let ok = match want {
            Some(n) => ws014 == *n,
            None => r.diagnostics.is_empty(),
        };
        if !ok {
            failures.push(format!(
                "{label}: want {want:?} WS014, got {:?}",
                r.diagnostics
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A mod resolves the module-level `let` its body reads, whether the let
/// is imported beside it (in either order), pulled in by the dependency
/// closure, or declared after the mod in the same file.
#[test]
fn a_mod_sees_the_module_lets_its_body_reads() {
    let triple = "mod Triple(v: int) -> (r: int) {\n  out r = v * K\n}\n";
    let lib = format!("let K = 3\n{triple}");
    let body = "in x: int\nlet t = Triple(x)\nout y: int = t.r\n";
    let rows = [
        (
            "the let imported after the mod",
            format!("import {{ Triple, K }} from \"lib\"\n{body}"),
            true,
        ),
        (
            "the let imported before the mod",
            format!("import {{ K, Triple }} from \"lib\"\n{body}"),
            true,
        ),
        (
            "the let pulled in by the closure",
            format!("import {{ Triple }} from \"lib\"\n{body}"),
            true,
        ),
        (
            "the let declared after the mod in one file",
            format!("{triple}let K = 3\n{body}"),
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (label, main, uses_lib) in &rows {
        let files: &[(&str, &str)] = if *uses_lib { &[("lib.ws", &lib)] } else { &[] };
        let r = resolve(main, "main.ws", &mem(files));
        let tc =
            crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        let diags: Vec<String> = r
            .diagnostics
            .iter()
            .chain(&tc.diagnostics)
            .map(|d| format!("[{}] {}", d.code, d.message))
            .collect();
        if !diags.is_empty() {
            failures.push(format!("{label}: {diags:#?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
