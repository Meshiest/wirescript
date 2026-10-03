use super::*;
use crate::parse;

fn model(src: &str) -> ScopeModel {
    build_scope_model(&parse(src, "t.ws").ast)
}

fn binding_names(m: &ScopeModel, ns: RefNs) -> Vec<&str> {
    m.bindings.iter().filter(|b| b.ns == ns).map(|b| b.name.as_str()).collect()
}

#[test]
fn file_scope_and_local_value_bindings_are_registered() {
    let src = "\
var score: int = 0
mod helper(v: int) -> (r: int) { out r = v }
in go: exec
on go {
  let tmp = score + 1
}";
    let m = model(src);
    let vals = binding_names(&m, RefNs::Value);
    assert!(vals.contains(&"score")); // file-scope var
    assert!(vals.contains(&"helper")); // importable mod
    assert!(vals.contains(&"v")); // mod param (scoped to body)
    assert!(vals.contains(&"go")); // in port
    assert!(vals.contains(&"tmp")); // handler-body let
    assert!(m.bindings.iter().any(|b| b.name == "helper" && b.importable));
    assert!(m.bindings.iter().any(|b| b.name == "v" && b.scope.is_some())); // local, has extent
    assert!(m.bindings.iter().any(|b| b.name == "score" && b.scope.is_none())); // file scope
}

#[test]
fn handler_capture_is_scoped_to_its_handler_body() {
    let src = "in go: exec\non CharacterSpawned() -> (character) {\n  character.DisplayText(\"hi\")\n}";
    let m = model(src);
    let cap = m.bindings.iter().find(|b| b.name == "character" && b.kind == "capture").unwrap();
    assert!(cap.scope.is_some(), "capture must carry a lexical scope extent");
}

#[test]
fn await_binding_is_registered_as_value() {
    // `let v = await x on done` parses as an `AwaitStmt` (binding on
    // `AwaitStmt.binding`, not a `LetDecl`), so its bound name only lands in
    // the model if the await arm registers it.
    let src = "in go: exec\non go { let v = await x on done\n let y = v }";
    let m = model(src);
    let b = m
        .bindings
        .iter()
        .find(|b| b.name == "v")
        .expect("await binding registered as a value binding");
    assert_eq!(b.ns, RefNs::Value);
    assert!(b.scope.is_some(), "block-local await binding must carry a scope extent");
}

#[test]
fn match_arm_binding_is_registered() {
    // `MatchExpr` has no surface syntax that parses today (the AST node is
    // reachable only via hand-built ASTs / the template cache), so build a
    // minimal program directly: one file-scope expr-statement whose expr is a
    // `match` with a single arm binding `evt`.
    use crate::ast::*;
    use crate::diagnostic::{Pos, SourceRange};

    let dummy = SourceRange::new("t.ws", Pos::default(), Pos::default());
    let arm = MatchArm {
        pattern: Pattern::Binding { name: "evt".into(), range: dummy.clone() },
        body: MatchBody::Expr(Expr::IntLit {
            value: 0,
            text: "0".into(),
            range: dummy.clone(),
        }),
        range: dummy.clone(),
    };
    let match_expr = Expr::MatchExpr {
        scrutinee: Box::new(Expr::Ident { name: "e".into(), range: dummy.clone() }),
        arms: vec![arm],
        range: dummy.clone(),
    };
    let script = Script {
        decls: vec![TopDecl::ExprStmt(ExprStmt {
            expr: match_expr,
            range: dummy.clone(),
        })],
        range: dummy.clone(),
        ..Default::default()
    };

    let m = build_scope_model(&script);
    let b = m
        .bindings
        .iter()
        .find(|b| b.name == "evt")
        .expect("match-arm binding registered as a value binding");
    assert_eq!(b.ns, RefNs::Value);
    assert!(b.scope.is_some(), "match-arm binding must carry a scope extent");
}

#[test]
fn for_loop_two_bindings_register_both_slots() {
    let src = "\
var arr: int[]
in go: exec
on go {
  for i, v in arr {
    let x = i + v
  }
}";
    let m = model(src);
    assert!(m.bindings.iter().any(|b| b.name == "i" && b.kind == "for" && b.scope.is_some()));
    assert!(m.bindings.iter().any(|b| b.name == "v" && b.kind == "for" && b.scope.is_some()));
}

#[test]
fn for_range_brace_binding_registers_named_slots() {
    // `for { index, isLast } in 0..n` names the loop's own slots directly -
    // there is no element to destructure on a range.
    let src = "\
var n: int = 0
in go: exec
on go {
  for { index, isLast } in 0..n {
    let x = index
    let y = isLast
  }
}";
    let m = model(src);
    assert!(m.bindings.iter().any(|b| b.name == "index" && b.kind == "for"));
    assert!(m.bindings.iter().any(|b| b.name == "isLast" && b.kind == "for"));
}

#[test]
fn for_range_brace_binding_alias_binds_the_alias_not_the_slot_label() {
    let src = "\
var n: int = 0
in go: exec
on go {
  for { index: idx } in 0..n {
    let x = idx
  }
}";
    let m = model(src);
    assert!(m.bindings.iter().any(|b| b.name == "idx" && b.kind == "for"));
    assert!(
        !m.bindings.iter().any(|b| b.name == "index"),
        "the slot label itself is not a bound name once aliased"
    );
}

#[test]
fn for_range_brace_binding_unknown_slot_registers_nothing() {
    // `nope` names no slot of a range (index/ordinal/isFirst/isLast);
    // typecheck alone reports WS076 for it, so scoped_refs binds nothing
    // rather than inventing a rename target for a name that never compiles.
    let src = "\
var n: int = 0
in go: exec
on go {
  for { nope } in 0..n {
  }
}";
    let m = model(src);
    assert!(!m.bindings.iter().any(|b| b.name == "nope"));
}

#[test]
fn for_ignore_binding_registers_nothing() {
    let src = "\
var arr: int[]
in go: exec
on go {
  for _, v in arr {
    let x = v
  }
}";
    let m = model(src);
    assert!(!m.bindings.iter().any(|b| b.name == "_"));
}

#[test]
fn for_as_run_registers_a_value_binding_with_a_scope_extent() {
    let src = "\
var n: int = 0
in go: exec
on go {
  for i in 0..n as run {
    let x = i
  }
  let ok = run.aborted
}";
    let m = model(src);
    let b = m
        .bindings
        .iter()
        .find(|b| b.name == "run")
        .expect("`as run` registered as a value binding");
    assert_eq!(b.ns, RefNs::Value);
    assert!(b.scope.is_some(), "`as run` is block-local, not file scope");
}

// --- value-use collection + `references_at` -----------------------

/// Resolve the binding under `(line, col)` (0-based, LSP-style — matches
/// `references_at`'s own contract) and return its name plus every site's
/// `(line, col)`, sorted. Sites come back as 1-based `Pos` fields (the AST's
/// native coordinates); subtract 1 on both so callers can compare directly
/// against 0-based positions in the source fixture, the same conversion
/// `analysis::definition` uses when turning a `SourceRange` into a
/// (0-based) `Location`.
fn rename_sites(src: &str, line: usize, col: usize) -> (String, Vec<(usize, usize)>) {
    let (t, sites) = references_at(&parse(src, "t.ws").ast, src, "t.ws", line, col).expect("target");
    let mut cols: Vec<(usize, usize)> = sites
        .iter()
        .map(|s| {
            (
                s.range.start.line.saturating_sub(1) as usize,
                s.range.start.col.saturating_sub(1) as usize,
            )
        })
        .collect();
    cols.sort();
    (t.name, cols)
}

#[test]
fn capture_rename_stays_inside_its_handler() {
    // The reported bug: renaming the `character` capture must NOT touch a
    // same-named capture in another handler.
    let src = "\
in a: exec
in b: exec
on ZoneEntered(zone = a) -> (character) {
  character.DisplayText(\"A\")
}
on ZoneLeft(zone = b) -> (character) {
  character.DisplayText(\"B\")
}";
    // cursor on the FIRST handler's capture `character` in `-> (character)`
    // (0-based line 2, col 29 is where the name token starts).
    let (name, sites) = rename_sites(src, 2, 30);
    assert_eq!(name, "character");
    // decl (line 2) + one body use (line 3) in THIS handler only = 2 sites.
    assert_eq!(sites.len(), 2);
    assert!(
        sites.iter().all(|(l, _)| *l == 2 || *l == 3),
        "sites leaked outside the first handler: {sites:?}"
    );
}

#[test]
fn inner_shadow_wins_and_outer_is_untouched() {
    let src = "\
var x: int = 0
in go: exec
on go {
  let x = 5
  let y = x + 1
}";
    // cursor on the inner `x` use in `x + 1` (0-based line 4, col 10).
    let (name, sites) = rename_sites(src, 4, 10);
    assert_eq!(name, "x");
    // the inner `let x = 5` decl (line 3) + its one use (line 4) = 2 sites;
    // the file-scope `var x` (line 0) is excluded entirely.
    assert_eq!(sites.len(), 2);
    assert!(
        sites.iter().all(|(l, _)| *l >= 3),
        "file-scope `var x` leaked into the inner shadow's sites: {sites:?}"
    );
}

/// A `var` or `let` initializer, and a read above the declaration, read what
/// the name meant before it, as the compiler reads them: renaming the outer
/// name renames those reads, and renaming the inner one leaves them.
#[test]
fn a_read_before_a_local_declaration_is_the_outer_name() {
    let src = "\
var x: int = 0
in go: exec
on go {
  let q = x
  var x: int = x + 1
  let y = x
  let x = x * 2
}";
    // The outer `var x` (line 0): its decl, the read above (line 3) and both
    // initializers (lines 4 and 6), which read it and the inner `var`.
    let (name, sites) = rename_sites(src, 0, 4);
    assert_eq!(name, "x");
    assert_eq!(sites, vec![(0, 0), (3, 10), (4, 15)], "{sites:?}");
    // The inner `var x` (line 4): its decl, the read after it, and the read in
    // the `let`'s initializer below it.
    let (_, sites) = rename_sites(src, 5, 10);
    assert_eq!(sites, vec![(4, 2), (5, 10), (6, 10)], "{sites:?}");
}

/// The storage an anonymous `chip { }` declares is named where the compiler
/// names it: a top-level chip's from anywhere in the file, a handler above
/// the declaration in the chip and one above the chip included; a chip in a
/// handler's from the chip on, so a read above it is the outer name.
#[test]
fn anon_chip_storage_is_named_where_the_compiler_names_it() {
    let src = "in go: exec
var n: int = 0
on go { n = cnt }
chip {
  on go { cnt = cnt + 1
    n = cnt }
  var cnt: int = 0
}";
    let (name, sites) = rename_sites(src, 6, 6);
    assert_eq!(name, "cnt");
    assert_eq!(sites, vec![(2, 12), (4, 10), (4, 16), (5, 8), (6, 2)], "{sites:?}");
    let src = "var cnt: int = 7
in go: exec
var n: int = 0
on go {
  let q = cnt
  chip { var cnt: int = 5
    cnt = cnt + 1 }
  n = cnt + q
}";
    // The chip's `var cnt` (line 5): its decl and the reads from the chip on.
    let (_, sites) = rename_sites(src, 5, 13);
    assert_eq!(sites, vec![(5, 9), (6, 4), (6, 10), (7, 6)], "{sites:?}");
    // The top-level `var cnt`: its decl and the read above the chip.
    let (_, sites) = rename_sites(src, 0, 4);
    assert_eq!(sites, vec![(0, 0), (4, 10)], "{sites:?}");
    // A chip nested in a top-level chip: its storage reaches a top-level
    // handler after the outer chip.
    let src = "in go: exec
in go2: exec
var n: int = 0
chip { chip { var y: int = 3
  on go { y = y + 1 } } }
on go2 { n = y }";
    let (name, sites) = rename_sites(src, 3, 18);
    assert_eq!(name, "y");
    assert_eq!(sites, vec![(3, 14), (4, 10), (4, 14), (5, 13)], "{sites:?}");
    // A top-level `const` and a top-level chip's `var` of one name: the chip
    // names its `var`, the rest of the file the `const`.
    let src = "const ch = \"evt\"
in go: exec
in go2: exec
var s: string = \"\"
chip { var ch: string = \"x\"
  on go { ch = \"y\" } }
on go2 { s = ch }";
    let (_, sites) = rename_sites(src, 4, 11);
    assert_eq!(sites, vec![(4, 7), (5, 10)], "{sites:?}");
    let (_, sites) = rename_sites(src, 0, 6);
    assert_eq!(sites, vec![(0, 6), (6, 13)], "{sites:?}");
    // A top-level `let` is named from its declaration on, as the compiler
    // reads it: above it, the chip's storage of the name is what reads mean.
    let src = "in go: exec
in go2: exec
var n2: int = 0
chip { var x: int = 1
 on go { x += 1 } }
on go2 { n2 = x }
let x = 7
on go { n2 = x }";
    let (name, sites) = rename_sites(src, 3, 11);
    assert_eq!(name, "x");
    assert_eq!(sites, vec![(3, 7), (4, 9), (4, 9), (5, 14)], "{sites:?}");
    let (_, sites) = rename_sites(src, 6, 4);
    assert_eq!(sites, vec![(6, 4), (7, 13)], "{sites:?}");
    // A `mod` written in the chip names the chip's storage wherever it is
    // called from, and a chip's `const` is the chip's inside it.
    let src = "let x = 4
const ch = \"evt\"
in go: exec
var n: int = 0
var s: string = \"\"
chip { var x: int = 1
 const ch = \"inner\"
 mod rd() -> int { return x * 10 }
 on go { s = ch } }
on go { n = rd() + x
 s = ch }";
    let (_, sites) = rename_sites(src, 5, 11);
    assert_eq!(sites, vec![(5, 7), (7, 26)], "{sites:?}");
    let (_, sites) = rename_sites(src, 0, 4);
    assert_eq!(sites, vec![(0, 4), (9, 19)], "{sites:?}");
    let (_, sites) = rename_sites(src, 6, 7);
    assert_eq!(sites, vec![(6, 7), (8, 13)], "{sites:?}");
    let (_, sites) = rename_sites(src, 1, 6);
    assert_eq!(sites, vec![(1, 6), (10, 5)], "{sites:?}");
}

#[test]
fn for_loop_binding_rename_touches_binding_and_body_use_only() {
    let src = "\
var total: int = 0
var n: int = 0
in go: exec
on go {
  for i in 0..n {
    total += i
  }
  total = 0
}";
    // cursor on the `i` binding in `for i in 0..n` (0-based line 4).
    let col = src.lines().nth(4).unwrap().find("for i").unwrap() + 4;
    let (name, sites) = rename_sites(src, 4, col);
    assert_eq!(name, "i");
    // decl (line 4) + the one body use in `total += i` (line 5) = 2 sites;
    // nothing outside the loop (the pre/post `total` statements) is touched.
    assert_eq!(sites.len(), 2);
    assert!(
        sites.iter().all(|(l, _)| *l == 4 || *l == 5),
        "sites leaked outside the loop: {sites:?}"
    );
}

#[test]
fn for_binding_shadow_resolves_inside_the_body_only() {
    let src = "\
var i: int = 0
in go: exec
on go {
  for i in 0..3 {
    let y = i
  }
  let z = i
}";
    // cursor on the loop-body use of `i` inside `let y = i` (0-based line 4).
    let col = src.lines().nth(4).unwrap().find("= i").unwrap() + 2;
    let (name, sites) = rename_sites(src, 4, col);
    assert_eq!(name, "i");
    // decl (line 3) + the body use (line 4) = 2 sites; the outer `var i`
    // (line 0) and the post-loop use in `let z = i` (line 6) are untouched.
    assert_eq!(sites.len(), 2);
    assert!(
        sites.iter().all(|(l, _)| *l == 3 || *l == 4),
        "outer `i` leaked into the loop binding's rename: {sites:?}"
    );
}

#[test]
fn for_as_run_resolves_after_the_loop_but_not_inside_it() {
    let src = "\
var n: int = 0
in go: exec
on go {
  for i in 0..n as run {
    let bad = run
  }
  let ok = run.aborted
}";
    // cursor on the post-loop use `run.aborted` (0-based line 6).
    let col = src.lines().nth(6).unwrap().find("run").unwrap();
    let (name, sites) = rename_sites(src, 6, col);
    assert_eq!(name, "run");
    assert!(
        sites.iter().any(|(l, _)| *l == 6),
        "the post-loop use must resolve: {sites:?}"
    );
    assert!(
        sites.iter().all(|(l, _)| *l != 4),
        "`run` used inside the loop body must not resolve to the `as run` binding: {sites:?}"
    );

    // The in-body use never resolved to anything, so it's a dead end for
    // `references_at` too - clicking it must not find `run`'s outer binding.
    let bad_col = src.lines().nth(4).unwrap().find("run").unwrap();
    assert!(
        references_at(&parse(src, "t.ws").ast, src, "t.ws", 4, bad_col).is_none(),
        "the in-body `run` use must not resolve at all"
    );
}

#[test]
fn interpolation_use_is_included() {
    // `"${name}"` must be a real use, not textually inert — the whole point
    // of walking `InterpLit`'s `Expr` parts rather than skipping them.
    let src = "\
in go: exec
on go {
  let name = \"world\"
  let msg = \"hi ${name}!\"
}";
    // cursor on the `name` declaration (0-based line 2, col 6).
    let (n, sites) = rename_sites(src, 2, 6);
    assert_eq!(n, "name");
    // decl (line 2) + the interpolation use (line 3) = 2 sites.
    assert_eq!(sites.len(), 2);
    assert!(
        sites.iter().any(|(l, _)| *l == 3),
        "interpolation use on line 3 missing from sites: {sites:?}"
    );
}

#[test]
fn cursor_on_a_use_resolves_to_the_same_target_as_the_decl() {
    // Clicking a USE (not the decl) must resolve to the same binding —
    // exercises the "cursor lands on a Use, look up its `resolved` id"
    // branch of dispatch, not just the "cursor lands on a Binding" branch.
    let src = "\
in go: exec
on go {
  let v = 1
  let w = v
}";
    // cursor on the `v` use inside `let w = v` (0-based line 3, col 10).
    let (name, sites) = rename_sites(src, 3, 10);
    assert_eq!(name, "v");
    assert_eq!(sites.len(), 2);
    assert!(sites.iter().all(|(l, _)| *l == 2 || *l == 3));
}

#[test]
fn file_scope_let_target_is_exported() {
    let src = "let helper = 1\nin go: exec\non go { let x = helper }";
    // cursor on the file-scope `let helper` decl (0-based line 0, col 4).
    let (t, sites) = references_at(&parse(src, "t.ws").ast, src, "t.ws", 0, 4).expect("target");
    assert_eq!(t.name, "helper");
    assert!(matches!(t.cross_file, CrossFile::Exported { ref export_name } if export_name == "helper"));
    assert_eq!(sites.len(), 2);
}

#[test]
fn file_scope_var_target_is_local_not_exported() {
    // `var`/`in`/`out`/`buffer`/`array`/`map` at file scope are NOT
    // importable, so even though they have no lexical scope extent
    // (`scope: None`, same as an importable `let`), they must classify as
    // `Local`, never `Exported`.
    let src = "var score: int = 0\nin go: exec\non go { score = score + 1 }";
    let (t, _sites) = references_at(&parse(src, "t.ws").ast, src, "t.ws", 0, 4).expect("target");
    assert_eq!(t.name, "score");
    assert!(matches!(t.cross_file, CrossFile::Local));
}

#[test]
fn cursor_on_second_tuple_name_resolves_to_that_name() {
    // `let (aa, bb) = …` registers `aa` and `bb` with ONE shared container
    // `name_range` — clicking the `bb` token must resolve to `bb` (via the
    // word-under-cursor tie-break), not the first-registered `aa`.
    let src = "\
in go: exec
on go {
  let (aa, bb) = SomeTuple()
  let z = bb
}";
    // cursor ON `bb` in the tuple pattern (0-based line 2, col 11).
    let (name, sites) = rename_sites(src, 2, 11);
    assert_eq!(name, "bb", "cursor on `bb` must resolve to `bb`, not `aa`");
    // the `bb` use in `let z = bb` (line 3) is among the sites.
    assert!(
        sites.iter().any(|(l, _)| *l == 3),
        "the `bb` use on line 3 must be a site: {sites:?}"
    );
    // decl (line 2) + use (line 3) = 2 sites; `aa` and `z` are not touched.
    assert_eq!(sites.len(), 2);
}

#[test]
fn destructure_shorthand_decl_is_coarse_and_shorthand() {
    // `let { x } = p` binds a local `x` at a decl site spanning `x ` (not the
    // name alone): the decl RefSite must be coarse (narrowed downstream) AND
    // shorthand (rename expands to `{ x: new }`, keeping the field `x`).
    let src = "\
in go: exec
on go { let { x } = p
 let z = x }";
    // cursor on the decl `x` inside `{ x }` (0-based line 1, col 14).
    let (t, sites) = references_at(&parse(src, "t.ws").ast, src, "t.ws", 1, 14).expect("target");
    assert_eq!(t.name, "x");
    let decl = sites
        .iter()
        .find(|s| s.range == t.decl_name_range)
        .expect("decl site present");
    assert!(decl.coarse, "shorthand destructure decl site must be coarse");
    assert!(decl.is_shorthand, "non-alias destructure decl site must be shorthand");
}

#[test]
fn destructure_alias_decl_is_coarse_not_shorthand() {
    // `let { src: bound } = p` binds `bound`: the decl RefSite is coarse
    // (range spans `src: bound `, narrowed to `bound` downstream) but NOT
    // shorthand — rename plain-replaces the `bound` token → `{ src: new }`.
    let src = "\
let { src: bound } = p
in go: exec
on go { let z = bound }";
    // cursor on the decl `bound` (0-based line 0, col 11).
    let (t, sites) = references_at(&parse(src, "t.ws").ast, src, "t.ws", 0, 11).expect("target");
    assert_eq!(t.name, "bound");
    let decl = sites
        .iter()
        .find(|s| s.range == t.decl_name_range)
        .expect("decl site present");
    assert!(decl.coarse, "aliased destructure decl site must be coarse");
    assert!(!decl.is_shorthand, "aliased destructure decl site must NOT be shorthand");
    // the `bound` use in `let z = bound` (line 2) is also a resolved site.
    assert!(
        sites.iter().any(|s| s.range.start.line == 3),
        "the `bound` use must resolve as a site"
    );
}

// --- type namespace + type-position resolution --------------------

#[test]
fn capture_named_like_a_type_does_not_touch_type_annotations() {
    let src = "\
in go: exec
in target: character
on CharacterSpawned() -> (character) {
  character.DisplayText(\"hi\")
}";
    // cursor on the capture `character` (value binding) in `-> (character)`
    // (0-based line 2, col 33).
    let (_n, sites) = rename_sites(src, 2, 33);
    // ONLY the capture decl + its body use — NOT `in target: character`
    // (type-position, line 1).
    assert!(sites.iter().all(|(l, _)| *l >= 2), "type annotation must be untouched: {sites:?}");
    assert_eq!(sites.len(), 2);
}

#[test]
fn renaming_a_type_alias_touches_only_type_positions() {
    let src = "\
type Point = { x: int, y: int }
in p: Point
let q: Point = p";
    // cursor on `Point` in the alias decl (0-based line 0, col 5).
    let (name, sites) = rename_sites(src, 0, 5);
    assert_eq!(name, "Point");
    // decl (line 0) + `in p: Point` (line 1) + `let q: Point` (line 2) = 3
    // type-position sites.
    assert_eq!(sites.len(), 3);
    assert!(sites.iter().any(|(l, _)| *l == 1), "the `in p: Point` use must be a site: {sites:?}");
    assert!(sites.iter().any(|(l, _)| *l == 2), "the `let q: Point` use must be a site: {sites:?}");
}

#[test]
fn builtin_type_name_never_resolves_to_a_shadowing_user_type_alias() {
    // `character`, `int`, … are plain identifiers (not lexer keywords, see
    // `lexer::KEYWORDS`), so `type character = …` parses — but a builtin
    // type-position use must NEVER resolve to it; the builtin always wins
    // over a same-named user alias.
    let src = "\
type character = { x: int }
in p: character";
    // cursor on the alias decl name `character` (0-based line 0, col 5).
    let (name, sites) = rename_sites(src, 0, 5);
    assert_eq!(name, "character");
    // only the decl itself — the builtin-type-position `in p: character`
    // (line 1) must NOT be pulled in as a site.
    assert_eq!(
        sites.len(),
        1,
        "a builtin type-position use must not resolve to the shadowing alias: {sites:?}"
    );
}

#[test]
fn emit_target_is_a_reference_of_the_out() {
    // `emit r = <expr>` writes the `out r` port — renaming `r` must update
    // the emit site too, or the renamed program stops compiling.
    let src = "\
out r = 0
in go: exec
on go { emit r = 5 }";
    // cursor on the `out r` decl (0-based line 0, col 4).
    let (name, sites) = rename_sites(src, 0, 4);
    assert_eq!(name, "r");
    // the `emit r = 5` site (line 2) is included alongside the decl (line 0).
    assert!(
        sites.iter().any(|(l, _)| *l == 2),
        "the emit site on line 2 must be included: {sites:?}"
    );
    assert_eq!(sites.len(), 2);
}

// --- cursor dispatch, target classification, refusals -------------

#[test]
fn builtin_type_position_is_not_renameable() {
    let src = "in x: character\nin go: exec\non go { }";
    // cursor on `character` the builtin type (line 0)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 0, 6).is_none());
}

#[test]
fn record_field_name_is_refused_for_now() {
    let src = "type P = { x: int }\nlet p: P = { x: 1 }\nlet a = p.x";
    // cursor on `.x` field access (line 2) → refuse (deferred feature)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 2, 10).is_none());
}

#[test]
fn ordinary_local_is_renameable() {
    let src = "in go: exec\non go { let v = 1\n let w = v }";
    // cursor on the `v` use inside `let w = v` (0-based line 2, col 9 — the
    // start of the single-char token; see `inner_shadow_wins_and_outer_is_untouched`
    // for the same "cursor at token start" convention).
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 2, 9).is_some());
}

#[test]
fn keyword_position_is_not_renameable() {
    // Regression: `var`'s own binding carries a COARSE `name_range` running
    // from the `var` keyword through the whole initializer (`parser.rs`'s
    // `parse_var_decl`), so without an explicit keyword refusal the cursor
    // sitting on the keyword itself would resolve as if it were on `x`.
    let src = "var x: int = 0\nin go: exec\non go { }";
    // cursor on the `var` keyword (0-based line 0, col 1)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 0, 1).is_none());
}

#[test]
fn field_access_inside_coarse_var_init_is_refused() {
    // Regression: a `var`'s coarse decl range spans its WHOLE initializer,
    // so a field-access cursor deep inside it (`p.field`) must not be
    // swallowed by the coarse `var x` binding and rename `x` instead.
    let src = "type P = { field: int }\nin p: P\nvar x: int = p.field";
    // cursor inside `field` of `p.field` (0-based line 2, col 17)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 2, 17).is_none());
}

#[test]
fn record_lit_field_key_inside_coarse_var_init_is_refused() {
    // Same hazard as above, for the KEY half of a record-literal field: the
    // `x` in `{ x: 1 }` must not be swallowed by the coarse `var p` binding.
    let src = "type P = { x: int }\nvar p: P = { x: 1 }";
    // cursor on the `x` key (0-based line 1, col 13)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 1, 13).is_none());
}

#[test]
fn unresolved_call_target_is_not_renameable() {
    // A call to a name with no matching `chip`/`mod`/`fn` binding (a builtin
    // gate call) leaves its `Use` unresolved — `references_at` itself
    // already returns `None` for that, with no extra check needed.
    let src = "in go: exec\non go { SomeBuiltinCall() }";
    // cursor on the callee `SomeBuiltinCall` (0-based line 1, col 10)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 1, 10).is_none());
}

#[test]
fn record_type_field_name_is_refused_for_now() {
    // The Global Constraints list a record TYPE field's own name as a third
    // refusal case alongside `FieldAccess.field` / `RecordLitField::Named`.
    let src = "type P = { x: int }";
    // cursor on the record type's own field name `x` (0-based line 0, col 11)
    assert!(prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 0, 11).is_none());
}

// --- cross-file resolution (`references_to_export`) ---------------

#[test]
fn import_specifier_and_uses_are_found_not_shadows() {
    let importer = "\
import { helper } from \"lib\"
in go: exec
on go { helper(1) }";
    let sites = references_to_export(&parse(importer, "main.ws").ast, "main.ws", "helper", RefNs::Value);
    // the `{ helper }` specifier + the `helper(1)` call = 2 sites.
    assert_eq!(sites.len(), 2, "sites: {sites:?}");
}

#[test]
fn a_local_shadow_is_excluded_from_export_refs() {
    let importer = "\
in go: exec
on go { let helper = 1
  let x = helper }";
    // local `helper`, no import — must NOT be reported as an export ref.
    let sites = references_to_export(&parse(importer, "main.ws").ast, "main.ws", "helper", RefNs::Value);
    assert!(sites.is_empty(), "sites: {sites:?}");
}

#[test]
fn imported_name_use_classifies_as_imported() {
    let importer = "\
import { helper } from \"lib\"
in go: exec
on go { helper(1) }";
    // cursor on the `helper` call site (0-based line 2, col 8 — the start of
    // `helper` in `on go { helper(1) }`).
    let (t, _sites) = references_at(&parse(importer, "main.ws").ast, importer, "main.ws", 2, 8).expect("target");
    assert!(
        matches!(t.cross_file, CrossFile::Imported { ref export_name } if export_name == "helper"),
        "cross_file: {:?}",
        t.cross_file
    );
}

#[test]
fn aliased_import_use_classifies_as_local() {
    // `import { orig as name }` — the alias is a purely FILE-LOCAL name.
    // Renaming it must stay in this file (rename the alias specifier token +
    // its uses), never reach the defining file whose decl still spells
    // `orig`. So an aliased import target classifies as `Local`, NOT
    // `Imported` — otherwise the cross-file path would try to narrow
    // `mod orig(){…}`'s decl range against the local alias and corrupt it.
    // (A NON-aliased import stays `Imported`; see
    // `imported_name_use_classifies_as_imported`.)
    let importer = "\
import { orig as aliased } from \"lib\"
in go: exec
on go { aliased(1) }";
    // cursor on the `aliased` call site (0-based line 2, col 8).
    let (t, _sites) = references_at(&parse(importer, "main.ws").ast, importer, "main.ws", 2, 8).expect("target");
    assert!(
        matches!(t.cross_file, CrossFile::Local),
        "aliased import must be file-local, got: {:?}",
        t.cross_file
    );
}

#[test]
fn references_to_export_respects_namespace() {
    // A named import is registered in BOTH namespaces (source file not
    // resolved locally); `references_to_export` must only return the
    // binding matching the requested `ns`, not both.
    let importer = "\
import { helper } from \"lib\"
in go: exec
on go { helper(1) }";
    let ast = parse(importer, "main.ws").ast;
    let value_sites = references_to_export(&ast, "main.ws", "helper", RefNs::Value);
    let type_sites = references_to_export(&ast, "main.ws", "helper", RefNs::Type);
    assert_eq!(value_sites.len(), 2, "value sites: {value_sites:?}");
    // only the import specifier itself resolves in the type namespace — no
    // type-position use of `helper` exists in this program.
    assert_eq!(type_sites.len(), 1, "type sites: {type_sites:?}");
}

#[test]
fn references_to_export_unknown_export_is_empty() {
    let importer = "\
import { helper } from \"lib\"
in go: exec
on go { helper(1) }";
    let sites = references_to_export(&parse(importer, "main.ws").ast, "main.ws", "nope", RefNs::Value);
    assert!(sites.is_empty());
}

#[test]
fn references_to_export_aliased_returns_specifier_only_not_alias_uses() {
    // For an ALIASED importer (`import { helper as x }` + `x(1)`), an EXPORT
    // rename of `helper` must touch ONLY the specifier — as a coarse whole-
    // `import …` span the LSP narrows to the original `helper` token — and
    // NOT the alias uses (`x(1)`), which keep the alias name.
    let importer = "\
import { helper as x } from \"lib\"
in go: exec
on go { x(1) }";
    let sites =
        references_to_export(&parse(importer, "main.ws").ast, "main.ws", "helper", RefNs::Value);
    assert_eq!(
        sites.len(),
        1,
        "aliased import: the specifier only, no alias uses: {sites:?}"
    );
    assert!(
        sites[0].coarse,
        "the specifier site must be coarse so the wiring layer narrows it to `helper`"
    );
}

#[test]
fn rename_exec_port_includes_its_on_trigger() {
    // `register_handler` previously never walked `h.trigger`,
    // so `on go` produced no `Use` of the `in go` binding at all — renaming
    // the port left every handler that triggers on it dangling, and the
    // trigger token itself wasn't even a valid rename START point. Both the
    // decl and the trigger must now resolve to the SAME two-site target.
    let src = "\
in go: exec
on go { emit r = 1 }
out r = 0";
    let decl_col = src.lines().next().unwrap().find("go").unwrap();
    let (name, sites) = rename_sites(src, 0, decl_col);
    assert_eq!(name, "go");
    assert_eq!(sites.len(), 2, "sites: {sites:?}");
    assert!(sites.iter().any(|(l, _)| *l == 0), "decl site missing: {sites:?}");
    assert!(sites.iter().any(|(l, _)| *l == 1), "trigger site missing: {sites:?}");

    // Cursor ON the trigger token itself must resolve to the same target.
    let trigger_col = src.lines().nth(1).unwrap().find("go").unwrap();
    let (name2, sites2) = rename_sites(src, 1, trigger_col);
    assert_eq!(name2, "go");
    assert_eq!(sites2.len(), 2, "sites2: {sites2:?}");
}

#[test]
fn user_event_trigger_is_renameable() {
    // A user `event` (captured via `let Name = on Trigger`) is a real
    // `Binding` (`TopDecl::Event`, importable) — `on Name { … }` must be a
    // value use of it, just like a builtin-port trigger, so renaming the
    // event also updates every handler that fires on it.
    let src = "\
in go: exec
let MyEvent = on go
on MyEvent { emit r = 1 }
out r = 0";
    let decl_col = src.lines().nth(1).unwrap().find("MyEvent").unwrap();
    let (name, sites) = rename_sites(src, 1, decl_col);
    assert_eq!(name, "MyEvent");
    assert_eq!(sites.len(), 2, "sites: {sites:?}");
    assert!(sites.iter().any(|(l, _)| *l == 1), "decl site missing: {sites:?}");
    assert!(sites.iter().any(|(l, _)| *l == 2), "trigger site missing: {sites:?}");
}

#[test]
fn trigger_field_object_is_a_use_and_field_half_is_refused() {
    // `on split.Forward` — the OBJECT half (`split`) is a value use of
    // whatever binds that name; the FIELD half (`Forward`) is a member
    // reference and must never be a rename target (mirrors
    // `Expr::FieldAccess` field-refusal handling).
    let src = "\
in go: exec
let split = go
on split.Forward { }";
    let decl_col = src.lines().nth(1).unwrap().find("split").unwrap();
    let (name, sites) = rename_sites(src, 1, decl_col);
    assert_eq!(name, "split");
    assert_eq!(sites.len(), 2, "sites: {sites:?}");
    assert!(sites.iter().any(|(l, _)| *l == 2), "trigger object site missing: {sites:?}");

    // The field half (`Forward`) must be refused for rename.
    let field_col = src.lines().nth(2).unwrap().find("Forward").unwrap();
    assert!(
        prepare_rename_at(&parse(src, "t.ws").ast, src, "t.ws", 2, field_col).is_none(),
        "trigger field half must be refused, like Expr::FieldAccess"
    );
}

// --- retire the textual scanner + regression suite -----------------

#[test]
fn consolidated_regression_comment_string_type_and_sibling_scope_excluded() {
    // Renaming a `character` VALUE capture must not touch: (a) a `//
    // character` comment, (b) a `"character"` string literal, (c) a builtin
    // `: character` TYPE-position annotation (a different namespace), or (d)
    // a same-named capture in a SIBLING handler (a different binding). These
    // are exactly the categories the old textual `find_all_references`
    // scanner used to catch (it matched on name text alone, with no notion
    // of comments, strings, namespaces, or lexical scope).
    let src = "\
in a: exec
in b: exec
in foo: character
on ZoneEntered(zone = a) -> (character) {
  // character comment mentioning character
  let s = \"character\"
  character.DisplayText(\"A\")
}
on ZoneLeft(zone = b) -> (character) {
  character.DisplayText(\"B\")
}";
    let decl_line = 3;
    let decl_col = src.lines().nth(decl_line).unwrap().find("character").unwrap();
    let (name, sites) = rename_sites(src, decl_line, decl_col);
    assert_eq!(name, "character");
    // decl (line 3) + the one body use (line 6) in THIS handler only.
    assert_eq!(sites.len(), 2, "sites: {sites:?}");
    assert!(
        sites.iter().all(|(l, _)| *l == decl_line || *l == 6),
        "leaked into comment/string/type-annotation/sibling-scope: {sites:?}"
    );
}

#[test]
fn original_repro_capture_in_nested_chip_excludes_comment_and_type_annotation() {
    // Manual end-to-end confirmation of the originally reported bug: a
    // `character` capture on a `ZoneEntered` handler NESTED INSIDE an
    // anonymous `chip { … }` block (not a top-level handler), alongside a
    // `// character` comment and an `in foo: character` builtin-type
    // annotation. Renaming the capture must touch ONLY the capture decl plus
    // its one body use — never the comment or the annotation.
    let src = "\
in zoneM: exec
in foo: character
chip {
  on ZoneEntered(zone = zoneM) -> (character) {
    // character
    character.DisplayText(\"hi\")
  }
}";
    let decl_line = 3;
    let decl_col = src.lines().nth(decl_line).unwrap().find("character").unwrap();
    let (name, sites) = rename_sites(src, decl_line, decl_col);
    assert_eq!(name, "character");
    assert_eq!(sites.len(), 2, "sites: {sites:?}");
    assert!(
        sites.iter().all(|(l, _)| *l == decl_line || *l == 5),
        "leaked outside the capture's own handler body: {sites:?}"
    );
}

// --- semantic-token overrides -----------------------------------------

/// Find the [`SemSpan`] whose start lands exactly at 0-based `(line, col)`
/// (mirrors the LSP's own coordinates — the AST's 1-based `Pos` is converted
/// the same way `rename_sites` above does).
fn span_at<'a>(spans: &'a [SemSpan], line: usize, col: usize) -> Option<&'a SemSpan> {
    spans
        .iter()
        .find(|s| s.range.start.line == line as u32 + 1 && s.range.start.col == col as u32 + 1)
}

#[test]
fn capture_named_like_a_type_is_a_variable_token_not_type() {
    // A `character` handler capture and its body use share their spelling
    // with the builtin `character` TYPE — the grammar's blanket
    // `support.type` regex can't tell them apart from an ACTUAL `: character`
    // type annotation. The resolver must: keep the annotation `Type`, but
    // reclassify the capture decl + its use as a plain `Variable` (every user
    // value identifier gets one uniform token, so highlighting is consistent).
    let src = "\
in go: exec
in foo: character
on CharacterSpawned() -> (character) {
  character.DisplayText(\"hi\")
}";
    let ast = parse(src, "t.ws").ast;
    let spans = semantic_tokens(&ast);

    // `: character` on `in foo: character` (line 1, 0-based) — TYPE position.
    let type_col = src.lines().nth(1).unwrap().find("character").unwrap();
    let type_span = span_at(&spans, 1, type_col).expect("type annotation span missing");
    assert_eq!(type_span.kind, SemTokenKind::Type);

    // The capture decl `-> (character)` on line 2 (0-based) — VALUE position.
    let decl_col = src.lines().nth(2).unwrap().find("character").unwrap();
    let decl_span = span_at(&spans, 2, decl_col).expect("capture decl span missing");
    assert_eq!(decl_span.kind, SemTokenKind::Variable);

    // Its body use `character.DisplayText(...)` on line 3 (0-based).
    let use_col = src.lines().nth(3).unwrap().find("character").unwrap();
    let use_span = span_at(&spans, 3, use_col).expect("capture use span missing");
    assert_eq!(use_span.kind, SemTokenKind::Variable);
}

#[test]
fn value_binding_named_like_a_builtin_is_a_variable_token() {
    // `round` is a builtin math function, so the grammar colors every `round`
    // as `support.function`. A value binding named `round` must be reclassified
    // to a plain `Variable` — the same uniform treatment a type-keyword-named
    // binding gets — so it doesn't read as a builtin call. An ordinary-named
    // binding (`ctrl`) gets the SAME `Variable`, so highlighting is consistent.
    let src = "\
in go: exec
on go {
  let round = 3
  let ctrl = 1
  let x = round + ctrl
}";
    let ast = parse(src, "t.ws").ast;
    let spans = semantic_tokens(&ast);

    for (line, needle) in [(2usize, "round"), (3, "ctrl"), (4, "round"), (4, "ctrl")] {
        let col = src.lines().nth(line).unwrap().find(needle).unwrap();
        let span = span_at(&spans, line, col)
            .unwrap_or_else(|| panic!("no span for {needle} on line {line}"));
        assert_eq!(span.kind, SemTokenKind::Variable, "{needle}@{line} should be Variable");
    }
}

#[test]
fn user_type_alias_use_is_a_type_token() {
    // `Point` isn't a builtin type keyword, so the grammar's `support.type`
    // coloring depends entirely on it being capitalized — this confirms the
    // resolver ALSO gives it the `Type` token (a use in TYPE position always
    // does, builtin or user-defined), the case the grammar's fixed builtin
    // list can't cover on its own.
    let src = "type Point = { x: int }\nin p: Point";
    let ast = parse(src, "t.ws").ast;
    let spans = semantic_tokens(&ast);

    let use_col = src.lines().nth(1).unwrap().find("Point").unwrap();
    let use_span = span_at(&spans, 1, use_col).expect("type alias use span missing");
    assert_eq!(use_span.kind, SemTokenKind::Type);
    assert_eq!(use_span.name, "Point");
}


#[test]
fn general_expr_trigger_call_is_not_recolored_by_a_synthetic_token() {
    // `on ReadBrickGrid()` desugars to a synthetic `let _on_expr_0 = ReadBrickGrid()`.
    // Semantic tokens must NOT emit a token for `_on_expr_0` (its range spans the
    // whole `ReadBrickGrid()` call), so the call keeps the grammar's function
    // coloring — consistent with a plain `let foo = ReadBrickGrid()` call.
    let src = "in go: exec\non ReadBrickGrid() {\n  let foo = ReadBrickGrid()\n}";
    let spans = semantic_tokens(&parse(src, "t.ws").ast);
    assert!(
        !spans.iter().any(|s| s.name.starts_with("_on_expr_")),
        "synthetic `_on_expr_N` must not be tokenized; got {:?}",
        spans.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn renaming_an_output_port_also_renames_its_reads() {
    // A rename that moves the declaration but leaves the reads behind produces
    // a silently dangling port, so this is a test rather than a manual check.
    let src = "in a: int\n\
               out y: int = a + 1\n\
               out z: int = y * 2\n\
               out w: int = y + 3";
    // `y` is declared on line 1; the raw site for a coarse decl like `out` is
    // the whole statement (col 0, the `out` keyword) since `references_at`
    // leaves narrowing to the name token to the LSP wiring layer's
    // `find_name_range` (see `prepare_rename_at`), not to this function.
    // Both reads sit at col 13.
    let (name, sites) = rename_sites(src, 1, 4);
    assert_eq!(name, "y");
    assert_eq!(
        sites,
        vec![(1, 0), (2, 13), (3, 13)],
        "declaration plus both reads"
    );
}

#[test]
fn a_shadowed_port_rename_stays_on_the_var() {
    // `count` names a var and a port. Every read belongs to the var, so
    // renaming from a read must not pull the port's declaration in.
    let src = "var count: int = 0\n\
               in go: bool\n\
               out count: int = count";
    let (name, sites) = rename_sites(src, 2, 17);
    assert_eq!(name, "count");
    // (0, 0) is the var's raw coarse decl site (the whole `var count: int =
    // 0` statement, starting at the `var` keyword) - see the narrowing note
    // above.
    assert!(
        sites.contains(&(0, 0)),
        "the read must resolve to the var declaration: {sites:?}"
    );
}

#[test]
fn renaming_a_handler_declared_port_reaches_a_cross_handler_read() {
    // A port declared inside a top-level `on` handler is hoisted into a real
    // file-scope boundary port, so a read of it from
    // an unrelated handler must be part of the same rename, not left dangling
    // outside the declaring handler's lexical block.
    let src = "on Clock(interval = 0.2) {\n\
               @top out flash: bool = Toggle()\n\
               }\n\
               in go: exec\n\
               on go {\n\
               var b: bool = flash\n\
               }";
    // `flash` is declared on line 1; the raw coarse decl site starts at col 5
    // (the `out` keyword, past the `@top` side annotation) - see the
    // narrowing note in `renaming_an_output_port_also_renames_its_reads`.
    // The cross-handler read sits on line 5 col 14.
    let (name, sites) = rename_sites(src, 1, 9);
    assert_eq!(name, "flash");
    assert_eq!(
        sites,
        vec![(1, 5), (5, 14)],
        "declaration plus the cross-handler read"
    );
}

#[test]
fn renaming_an_anon_chip_declared_port_reaches_a_cross_chip_read() {
    // A file-scope anon chip's `out` binding is the same kind of hoisted
    // boundary port as a handler-declared one, so a read outside the chip
    // body must be part of the rename too.
    let src = "chip { out shared: int = 1 }\n\
               in go: exec\n\
               on go {\n\
               var v: int = shared\n\
               }";
    // `shared` is declared on line 0; the raw coarse decl site starts at
    // col 7 (the `out` keyword, past `chip { `) - see the narrowing note in
    // `renaming_an_output_port_also_renames_its_reads`. The cross-chip read
    // sits on line 3 col 13.
    let (name, sites) = rename_sites(src, 0, 11);
    assert_eq!(name, "shared");
    assert_eq!(
        sites,
        vec![(0, 7), (3, 13)],
        "declaration plus the cross-chip read"
    );
}

#[test]
fn renaming_a_statement_level_anon_chip_port_reaches_a_later_read() {
    // A `chip { }` written as a STATEMENT inside a handler declares a real
    // boundary port too (`typecheck::stmt::check_anon_chip_stmts` registers it
    // into the handler's frame, `lower_block_inner` pre-declares it), so a read
    // later in that handler is part of the same rename rather than dangling
    // outside the chip's own block.
    let src = "in go: exec
               on go {
               chip { out shared: int = 1 }
               var v: int = shared
               }";
    let (name, sites) = rename_sites(src, 2, 26);
    assert_eq!(name, "shared");
    assert_eq!(
        sites,
        vec![(2, 22), (3, 28)],
        "declaration plus the later in-handler read"
    );
}

#[test]
fn renaming_a_nested_anon_chip_port_reaches_a_read_outside_both() {
    // An anon chip pushes no scope frame, so one nested inside a file-scope
    // anon chip declares a file-wide port exactly as its parent does.
    let src = "chip { chip { out shared: int = 1 } }
               in go: exec
               on go {
               var v: int = shared
               }";
    let (name, sites) = rename_sites(src, 0, 18);
    assert_eq!(name, "shared");
    assert_eq!(
        sites,
        vec![(0, 14), (3, 28)],
        "declaration plus the cross-chip read"
    );
}

#[test]
fn atom_hash_is_a_64_bit_i64() {
    // Every atom value is `xxh64(name) as i64` — a bit-preserving reinterpret of
    // a u64, so it always fits an i64 (high-bit-set names read as negative).
    for name in ["bomber", "the-black", "seer", "x"] {
        let v = crate::hash::atom_hash(name);
        // Round-trips through u64 with no loss (proves it's exactly 64 bits).
        assert_eq!(v, (v as u64) as i64);
    }
}

// --- column conventions: char column in, byte columns out ---------

/// 0-based CHAR column of `needle` on 0-based `line`: the coordinate every
/// `analysis` entry point takes, which is NOT `str::find`'s byte offset once
/// the line holds a non-ASCII character.
fn char_col_of(src: &str, line: usize, needle: &str) -> usize {
    let l = src.lines().nth(line).unwrap();
    l[..l.find(needle).unwrap()].chars().count()
}

#[test]
fn a_non_ascii_character_earlier_on_the_line_does_not_shift_the_cursor() {
    // `Pos::col` counts bytes and the cursor arrives as a char column, so
    // without the conversion the two emoji slid the cursor 6 columns left
    // onto `abc`, and rename rewrote that declaration instead of `xyz`'s.
    let src = "\
var abc: int = 0
var xyz: int = 0
in go: exec
on go {
  Log(\"\u{1F389}\u{1F389}\" .. abc .. xyz)
}";
    let ast = parse(src, "t.ws").ast;
    let col = char_col_of(src, 4, "xyz");
    let (target, _) = references_at(&ast, src, "t.ws", 4, col).expect("target under cursor");
    assert_eq!(target.name, "xyz");
    let (_, name) = prepare_rename_at(&ast, src, "t.ws", 4, col).expect("renameable");
    assert_eq!(name, "xyz");

    // Two plain BMP accents are enough as well (one extra byte each); there
    // the old behaviour was a silent no-op rather than a wrong answer.
    let src = "\
var xyz: int = 0
in go: exec
on go {
  BroadcastChatMessage(\"h\u{e9}llo w\u{f6}rld\" .. xyz)
}";
    let ast = parse(src, "t.ws").ast;
    let col = char_col_of(src, 3, "xyz");
    let (target, _) = references_at(&ast, src, "t.ws", 3, col).expect("target under cursor");
    assert_eq!(target.name, "xyz");
}

#[test]
fn field_name_at_reads_a_char_column() {
    let src = "\
type R = {hp: int}
let r: R = {hp: 1}
in go: exec
on go {
  BroadcastChatMessage(\"\u{e9}${r.hp}\")
}";
    let ast = parse(src, "t.ws").ast;
    let col = char_col_of(src, 4, "hp}");
    assert!(field_name_at(&ast, src, 4, col), "the cursor is on the `hp` field name");
    // The receiver right before it is not a field name.
    let recv = char_col_of(src, 4, "r.hp");
    assert!(!field_name_at(&ast, src, 4, recv));
}

/// A name the top level declares twice is read by position, as the compiler
/// reads it: each read names the latest declaration written before it, and a
/// `mod` body the latest one before the `mod`.
#[test]
fn a_top_level_name_declared_twice_is_named_by_position() {
    let src = "in go: exec
in go2: exec
var n: int = 0
const k = 1
mod m() -> int { return k }
on go { n = k }
const k = 2
on go2 { n = k + m() }";
    let (name, sites) = rename_sites(src, 3, 6);
    assert_eq!(name, "k");
    assert_eq!(sites, vec![(3, 6), (4, 24), (5, 12)], "{sites:?}");
    let (_, sites) = rename_sites(src, 7, 13);
    assert_eq!(sites, vec![(6, 6), (7, 13)], "{sites:?}");
}

/// Each name `k` written in `src`, by its (0-based) line and column.
fn sites_of_k(src: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (line, text) in src.lines().enumerate() {
        let bytes = text.as_bytes();
        for (col, c) in bytes.iter().enumerate() {
            let ident = |i: usize| bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            if *c == b'k' && (col == 0 || !ident(col - 1)) && !ident(col + 1) {
                out.push((line, col));
            }
        }
    }
    out
}

/// The editor names a duplicated top-level name the declaration the compiler
/// reads at each place: storage above every declaration, a `let` from its end,
/// a `mod` or `chip` body as read where it is declared (the first declaration
/// when it is above all of them), a self-reference the earlier declaration,
/// and a library file by its own positions. Each row lists the reads that name
/// one declaration, by the order of the `k`s written.
#[test]
fn a_top_level_name_declared_twice_is_named_where_the_compiler_reads_it() {
    let head = "in go: exec\nin go2: exec\nin go3: exec\nvar n: int = 0\n";
    let rows: [(&str, &[&[usize]]); 9] = [
        (
            "on go { n = k }\nvar k: int = 1\non go2 { n = k }\nlet k = 2\non go3 { n = k }",
            &[&[0, 1, 2], &[3, 4]],
        ),
        (
            "on go { n = k }\nlet k = 2\non go2 { n = k }\nvar k: int = 1\non go3 { n = k }",
            &[&[0, 3, 4], &[1, 2]],
        ),
        ("buffer k: int = k + 2\non go { n = k }\nlet k = 5\non go2 { n = k }", &[&[0, 1, 2], &[3, 4]]),
        ("const k = 1\nlet k = k + 1\non go { n = k }", &[&[0, 2], &[1, 3]]),
        ("const k = 1\nchip C() { on go { n = k } }\nconst k = 2\nC()\non go2 { n = k }", &[&[0, 1], &[2, 3]]),
        ("mod f() -> int { return k }\nconst k = 1\nconst k = 2\non go { n = f() + k }", &[&[0, 1], &[2, 3]]),
        ("mod f() -> int { return k }\nlet k = 1\nvar k: int = 2\non go { n = f() + k }", &[&[0, 2, 3], &[1]]),
        ("let k = 5\nvar k: int = k\non go { n = k }", &[&[0, 2], &[1, 3]]),
        ("const k = 5\nstatic var k: int = k + 1\non go { n = k }", &[&[0, 2], &[1, 3]]),
    ];
    // A site is compared by its line: a storage declaration's site starts at
    // its keyword.
    let lines = |sites: &[(usize, usize)]| {
        let mut l: Vec<usize> = sites.iter().map(|s| s.0).collect();
        l.dedup();
        l
    };
    for (body, groups) in rows {
        let src = format!("{head}{body}");
        let ks = sites_of_k(&src);
        for group in groups {
            let want: Vec<(usize, usize)> = group.iter().map(|i| ks[*i]).collect();
            for (line, col) in &want {
                let (name, sites) = rename_sites(&src, *line, *col);
                assert_eq!(name, "k");
                assert_eq!(lines(&sites), lines(&want), "from {line}:{col}\n{src}");
            }
        }
    }
    // A library file the editor opens on its own reads by position too.
    let lib = "const k = 2\nmod fa() -> int { return k }\nconst k = 30\nmod fb() -> int { return k }";
    let ks = sites_of_k(lib);
    assert_eq!(rename_sites(lib, ks[1].0, ks[1].1).1, vec![ks[0], ks[1]]);
    assert_eq!(rename_sites(lib, ks[3].0, ks[3].1).1, vec![ks[2], ks[3]]);
}
