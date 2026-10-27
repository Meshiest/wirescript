use super::*;
use crate::analysis::symbols::collect_symbols_for_file;
use crate::resolve::{MemLoader, resolve};

fn goto(main: &str, display: &str, line: usize, col: usize) -> Option<Location> {
    let mut files = crate::collections::HashMap::default();
    files.insert("display.ws".to_string(), display.to_string());
    let loader = MemLoader { files };
    let pre = crate::parse(main, "main.ws");
    let resolved = resolve(main, "main.ws", &loader);
    let tc = crate::typecheck::typecheck(&resolved.ast, &crate::typecheck::CeSlotMap::default());
    let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main.ws"));
    definition_at(main, &pre.ast, &symbols, "main.ws", &loader, line, col)
}

const SHAPE_ENUM_MAIN: &str = "enum Shape { Empty, Circle(float), Rect(float, float) }\nvar s: Shape = Shape.Empty\nin go: exec\non go {\n  let c = Shape.Circle\n}\n";
const BOX_ENUM_MAIN: &str =
    "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w: 1.0, h: 2.0 }\n";
const EASING_MAIN: &str = "var e: EasingFunction = EasingFunction.Bounce\n";
const DRAW_CARD_LIB: &str = "mod drawCard(x: int) {\n  let unused = x\n}\n";

/// 0-based column of the `nth` `needle` on 0-based `line` of `src`.
fn col_of(src: &str, line: usize, needle: &str, nth: usize) -> usize {
    src.lines()
        .nth(line)
        .unwrap()
        .match_indices(needle)
        .nth(nth)
        .expect("needle")
        .0
}

/// Go-to-definition from the cursor `offset` columns into the `nth`
/// `needle` on `line` of main.ws (display.ws holds `display`). `want` is
/// where it lands: the file (`None` for main.ws), the 0-based line, and,
/// when given, the column; `None` means it goes nowhere.
struct Goto {
    label: &'static str,
    main: &'static str,
    display: &'static str,
    line: usize,
    at: (&'static str, usize, usize),
    want: Option<(Option<&'static str>, usize, Option<usize>)>,
}

#[track_caller]
fn check_gotos(rows: &[Goto]) {
    let mut failures = Vec::new();
    for r in rows {
        let (needle, nth, offset) = r.at;
        let col = col_of(r.main, r.line, needle, nth) + offset;
        let got = goto(r.main, r.display, r.line, col);
        let ok = match (&r.want, &got) {
            (None, None) => true,
            (Some((file, line, want_col)), Some(loc)) => {
                loc.file.as_deref() == *file
                    && loc.start_line == *line
                    && want_col.is_none_or(|c| loc.start_col == c)
            }
            _ => false,
        };
        if !ok {
            failures.push(format!("{}: want {:?}, got {got:?}", r.label, r.want));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn definition_lands_on_the_declaration() {
    let g = |label, main, display, line, at, want| Goto {
        label,
        main,
        display,
        line,
        at,
        want,
    };
    check_gotos(&[
        // The `.dist` of `a.dist(b)` jumps to the user `self`-mod
        // declaration (the bare-word symbol fallback resolves it).
        g(
            "a self-receiver method goes to its mod",
            "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\nin a: vector\nin b: vector\nin go: exec\non go { let d = a.dist(b) }\n",
            "",
            4,
            (".dist", 0, 2),
            Some((None, 0, None)),
        ),
        // `card.drawCard` resolves in the imported file even though a
        // local `mod drawCard` shares the name.
        g(
            "a namespaced call goes to the imported decl, not a local shadow",
            "import * as card from \"display\"\n\nmod drawCard(y: int) {\n  card.drawCard(y)\n}\n",
            DRAW_CARD_LIB,
            3,
            (".drawCard", 0, 2),
            Some((Some("display.ws"), 0, None)),
        ),
        // Bare `drawCard(...)` keeps resolving to the local mod.
        g(
            "an unqualified call still goes to the local decl",
            "import * as card from \"display\"\n\nmod drawCard(y: int) {\n  let z = y\n}\n\nmod use1(w: int) {\n  drawCard(w)\n}\n",
            DRAW_CARD_LIB,
            7,
            ("drawCard", 0, 1),
            Some((None, 2, None)),
        ),
        // A named-import specifier resolves the one under the cursor, not
        // the first that resolves: `onCheckpoint` on the continuation line
        // must not jump to the earlier `doReset` (line 1).
        g(
            "a multi-line named import resolves the clicked specifier",
            "import { cfg, doReset,\n  onCheckpoint } from \"display\"\n",
            "var cfg: string = \"\"\nmod doReset() { }\nmod onCheckpoint() { }\n",
            1,
            ("onCheckpoint", 0, 2),
            Some((Some("display.ws"), 2, None)),
        ),
        // An imported `var` specifier resolves to the var, not the file top.
        g(
            "a named import resolves a var specifier",
            "import { cfg, doReset } from \"display\"\n",
            "var cfg: string = \"\"\nmod doReset() { }\n",
            0,
            ("cfg", 0, 1),
            Some((Some("display.ws"), 0, None)),
        ),
        // The channel-name string of a `SendCustomEvent(...)` jumps to the
        // matching `on CustomEvent(...)` receiver in the same file.
        g(
            "a SendCustomEvent name jumps to its receiver",
            "on CustomEvent(\"dmg\") -> (amount: int) {\n  let x = amount\n}\nin go: exec\non go {\n  SendCustomEvent(\"dmg\", 5)\n}\n",
            "",
            5,
            ("dmg", 0, 1),
            Some((None, 0, None)),
        ),
        // `on GlobalCustomEvent` (line 0), NOT the same-named personal
        // `on CustomEvent` (line 2): they are separate namespaces.
        g(
            "a SendGlobalCustomEvent name jumps to the global receiver, not the personal one",
            "on GlobalCustomEvent(\"score\") -> (pts: int) {\n}\non CustomEvent(\"score\") -> (x: int) {\n}\nin go: exec\non go {\n  SendGlobalCustomEvent(\"score\", 5)\n}\n",
            "",
            6,
            ("score", 0, 1),
            Some((None, 0, None)),
        ),
        // `Shape` in `var s: Shape` lands on the `Shape` name token of the
        // `enum Shape` decl.
        g(
            "an enum type name jumps to its decl",
            SHAPE_ENUM_MAIN,
            "",
            1,
            ("Shape", 0, 1),
            Some((None, 0, Some(col_of(SHAPE_ENUM_MAIN, 0, "Shape", 0)))),
        ),
        // `Circle` in `Shape.Circle` lands on that variant's own token
        // inside the `enum Shape` declaration, not just the enum's line.
        g(
            "an enum variant use jumps to its variant token",
            SHAPE_ENUM_MAIN,
            "",
            4,
            ("Circle", 0, 1),
            Some((None, 0, Some(col_of(SHAPE_ENUM_MAIN, 0, "Circle", 0)))),
        ),
        // A unit variant has no payload fields: the construction/pattern
        // field resolvers must be a no-op here, not panic, and the
        // variant-name resolver still jumps to the variant.
        g(
            "a unit variant construction resolves without panicking",
            SHAPE_ENUM_MAIN,
            "",
            1,
            ("Empty", 0, 1),
            Some((None, 0, None)),
        ),
        // `w` in `Shape.Box { w: 1.0, h: 2.0 }` lands on the `w` token in
        // the `Box { w: float, h: float }` payload decl.
        g(
            "a construction field key jumps to its field decl",
            BOX_ENUM_MAIN,
            "",
            1,
            ("w:", 0, 1),
            Some((None, 0, Some(col_of(BOX_ENUM_MAIN, 0, "w:", 0)))),
        ),
        // `h` (the second field) resolves to its OWN token, not `w`'s.
        g(
            "a second construction field key jumps to its own decl",
            BOX_ENUM_MAIN,
            "",
            1,
            ("h:", 0, 1),
            Some((None, 0, Some(col_of(BOX_ENUM_MAIN, 0, "h:", 0)))),
        ),
        // `w` in `match s { Box { w, h } => w, ... }` (the shorthand
        // capture) jumps to the `w` token in the payload decl.
        g(
            "a pattern shorthand field jumps to its field decl",
            "enum Shape { Box { w: float, h: float } }\nvar s: Shape = Shape.Box { w: 1.0, h: 2.0 }\nout r = match s { Box { w, h } => w, _ => 0.0 }\n",
            "",
            2,
            ("{ w, h }", 0, 2),
            Some((None, 0, Some(col_of(BOX_ENUM_MAIN, 0, "w:", 0)))),
        ),
        g(
            "a port read lands on its declaration",
            "in a: int\nout y: int = a + 1\nout z: int = y * 2\n",
            "",
            2,
            ("", 0, 13),
            Some((None, 1, None)),
        ),
        // `flash` is a boundary port hoisted out of the `on Clock`
        // handler; a read from an unrelated handler lands on the
        // declaration inside it.
        g(
            "a handler-declared port read lands on its declaration",
            "on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}\nin go: exec\non go {\nvar b: bool = flash\n}",
            "",
            5,
            ("", 0, 14),
            Some((None, 1, None)),
        ),
        // Same hoisting, for a file-scope anon chip's `out` binding.
        g(
            "an anon-chip-declared port read lands on its declaration",
            "chip { out shared: int = 1 }\nin go: exec\non go {\nvar v: int = shared\n}",
            "",
            3,
            ("", 0, 13),
            Some((None, 0, None)),
        ),
    ]);
}

#[test]
fn definition_goes_nowhere_without_a_source_declaration() {
    let g = |label, main, line, at| Goto {
        label,
        main,
        display: "",
        line,
        at,
        want: None,
    };
    check_gotos(&[
        // No matching receiver: the send-site string yields no navigation.
        g(
            "a SendCustomEvent name without a receiver",
            "in go: exec\non go {\n  SendCustomEvent(\"nope\", 5)\n}\n",
            2,
            ("nope", 0, 1),
        ),
        // A built-in game enum has no source location: goto reports
        // nothing rather than panicking.
        g(
            "a built-in enum type name",
            EASING_MAIN,
            0,
            ("EasingFunction", 0, 1),
        ),
        g("a built-in enum variant", EASING_MAIN, 0, ("Bounce", 0, 1)),
        // The VALUE side (`1.0`) is not a field key.
        g(
            "a construction field's value",
            BOX_ENUM_MAIN,
            1,
            ("1.0", 0, 1),
        ),
        // A field name not declared on the variant (a WS041-worthy typo)
        // does not resolve, and does not panic.
        g(
            "an undeclared construction field",
            "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { q: 1.0 }\n",
            1,
            ("q:", 0, 1),
        ),
    ]);
}

#[test]
fn goto_pattern_field_ambiguous_variant_name_returns_none() {
    // Two different enums both declare a `Box` variant with named
    // fields - the variant name alone doesn't uniquely resolve an enum
    // without the scrutinee's real type, so this must not guess a field.
    // The shorthand `w` is also the capture it binds, which is where it
    // may go: itself.
    let main = "enum Shape { Box { w: float, h: float } }\n\
                    enum Other { Box { n: int } }\n\
                    var s: Shape = Shape.Box { w: 1.0, h: 2.0 }\n\
                    out r = match s { Box { w, h } => w, _ => 0.0 }\n";
    let l = main.lines().nth(3).unwrap();
    let col = l.find("{ w, h }").unwrap() + 2;
    let loc = goto(main, "", 3, col);
    assert!(
        loc.as_ref().is_none_or(|l| l.start_line == 3),
        "an ambiguous variant name across two enums must not resolve to a field: {loc:?}"
    );
}

#[test]
fn goto_pattern_binding_does_not_resolve_to_field_decl() {
    // The bound value `w` used in the arm BODY (not the pattern's own key)
    // goes to the capture that binds it, in the pattern, never to the
    // variant's field declaration.
    let main = "enum Shape { Box { w: float, h: float } }\n\
                    var s: Shape = Shape.Box { w: 1.0, h: 2.0 }\n\
                    out r = match s { Box { w, h } => w, _ => 0.0 }\n";
    let l = main.lines().nth(2).unwrap();
    let col = l.rfind("=> w").unwrap() + 3; // the `w` in the arm body
    let loc = goto(main, "", 2, col).expect("an arm-body capture use goes to its capture");
    let capture_col = l.find("{ w, h }").unwrap() + 2;
    assert_eq!((loc.start_line, loc.start_col), (2, capture_col), "{loc:?}");
}

#[test]
fn annotation_name_is_not_a_symbol() {
    // `@left` must not jump to a variable named `left`; the variable's own
    // uses still resolve.
    let main = "var left: int = 0\n@left in go: exec\non go { left = 1 }\n";
    let at = main.lines().nth(1).unwrap().find("left").unwrap() + 1;
    assert!(goto(main, "", 1, at).is_none(), "annotation resolved");
    let use_col = main.lines().nth(2).unwrap().find("left").unwrap() + 1;
    let loc = goto(main, "", 2, use_col).expect("the variable resolves");
    assert_eq!(loc.start_line, 0);
}
