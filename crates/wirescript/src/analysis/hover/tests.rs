use super::*;
use crate::analysis::collect_symbols_for_file;
use crate::resolve::{FsLoader, resolve};
use crate::typecheck::typecheck;
fn hover_for(source: &str, line: usize, col: usize) -> Option<String> {
    hover_with_loader(source, "test", &FsLoader, line, col)
}

/// The hover at `(line, col)` of `source`, compiled as `file` with its
/// imports read through `loader`.
fn hover_with_loader(
    source: &str,
    file: &str,
    loader: &dyn crate::resolve::FileLoader,
    line: usize,
    col: usize,
) -> Option<String> {
    let resolved = resolve(source, file, loader);
    let tc = typecheck(&resolved.ast, file, &crate::typecheck::CeSlotMap::default());
    let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some(file));
    let estimates = crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, file);
    hover_at(
        source,
        file,
        &crate::parser::parse(source, file).ast,
        &symbols,
        &tc.type_of_expr,
        &resolved.doc_comments,
        &tc.if_contexts,
        &tc.var_read_contexts,
        &tc.narrowed_reads,
        &tc.dropped_ranges,
        &estimates,
        line,
        col,
    )
}

/// Hovering a record field written as an assignment target through an array
/// index (`tk[i].phase = v`) resolves the field's type. The assignment-target
/// inference used to type the field access `any` without inferring `tk[i]`,
/// so the type map had no entry for the object and the field hover failed.
#[test]
fn hover_field_on_array_index_assignment_target() {
    let src = "type Slot = { phase: int, wantStart: bool }\nvar tk: Slot[]\nin go: exec\non go { tk[0].phase = 5 }\n";
    let col = src.lines().nth(3).unwrap().find("phase").unwrap() + 1;
    let h = hover_for(src, 3, col).expect("hover on tk[0].phase should resolve");
    assert!(h.contains("phase") && h.contains("int"), "got: {h}");
}

#[test]
fn fill_record_generates_missing_fields_recursively() {
    let src = "type Card = { foo: string, bar: { baz: int }, baq: { fred: string } }\nlet c: Card = {\n\n}";
    let resolved = resolve(src, "test", &FsLoader);
    let tc = typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
    // Cursor on the empty line (0-based line 2) inside the `{ }`.
    let fill = fill_record_at(src, &symbols, 2, 0).expect("fill available inside record literal");
    assert!(
        fill.text.contains("foo: \"\""),
        "foo default: {}",
        fill.text
    );
    assert!(
        fill.text.contains("bar: { baz: 0 }"),
        "nested record filled: {}",
        fill.text
    );
    assert!(
        fill.text.contains("baq: { fred: \"\" }"),
        "nested record filled: {}",
        fill.text
    );
}

#[test]
fn fill_record_skips_already_present_fields() {
    let src = "type Card = { foo: string, bar: int }\nlet c: Card = {\n  foo: \"x\",\n\n}";
    let resolved = resolve(src, "test", &FsLoader);
    let tc = typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
    let fill = fill_record_at(src, &symbols, 3, 0).expect("fill for a partial literal");
    assert!(
        !fill.text.contains("foo:"),
        "present field foo skipped: {}",
        fill.text
    );
    assert!(
        fill.text.contains("bar: 0"),
        "missing field bar filled: {}",
        fill.text
    );
}

/// A read of an `any` a kind test narrowed hovers as the narrowed type and
/// names the test; a read outside the test stays `any` with no note.
#[test]
fn narrowed_any_hovers_as_its_kind_and_names_the_test() {
    let src = "var xs: any[] = []\n\
                   in go: exec\n\
                   on go {\n\
                   \x20 let a0 = xs[0]\n\
                   \x20 var sum: int = 0\n\
                   \x20 match a0.getType() {\n\
                   \x20   Int => { sum = a0 + 1 }\n\
                   \x20   _ => {}\n\
                   \x20 }\n\
                   \x20 let p = ReadBrickGrid().GetCustomProperty(\"k\")\n\
                   \x20 if p.Value.getType() is Type.Int { sum = p.Value }\n\
                   \x20 let after = a0\n\
                   }\n";
    let at = |needle: &str, nth: usize| {
        let (line, l) = src
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(needle))
            .unwrap();
        (line, l.match_indices(needle).nth(nth).unwrap().0 + 1)
    };

    let (line, col) = at("sum = a0", 0);
    let h = hover_for(src, line, col + "sum = ".len()).expect("hover on the narrowed a0");
    assert!(h.contains("a0: int"), "narrowed type: {h}");
    assert!(
        h.contains(
            "Declared `any`, narrowed to `int` here by the `Int` arm of `match a0.getType()`"
        ),
        "{h}"
    );

    let (line, col) = at("sum = p.Value", 0);
    let h = hover_for(src, line, col + "sum = p.".len()).expect("hover on the narrowed field");
    assert!(h.contains("field Value: int"), "narrowed field type: {h}");
    assert!(h.contains("by `if p.Value.getType() is Type.Int`"), "{h}");

    let (line, col) = at("after = a0", 0);
    let h = hover_for(src, line, col + "after = ".len()).expect("hover outside the test");
    assert!(h.contains("a0: any") && !h.contains("narrowed"), "{h}");
}

/// `is` hovers as a variant test; on a kind test it names what the tested
/// value narrows to.
#[test]
fn is_keyword_hovers_as_a_test() {
    let src = "enum Shape { Empty, Circle(float) }\n\
                   static var s: Shape = Shape.Empty\n\
                   var xs: any[] = []\n\
                   out round = s is Shape.Circle\n\
                   in go: exec\n\
                   on go {\n\
                   \x20 let a0 = xs[0]\n\
                   \x20 if a0.getType() is Type.Int { xs.push(1) }\n\
                   }\n";
    let at = |needle: &str| {
        let (line, l) = src
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(needle))
            .unwrap();
        (line, l.find(needle).unwrap() + 1)
    };

    let (line, col) = at("is Shape");
    let h = hover_for(src, line, col).expect("hover on a variant test");
    assert!(
        h.contains("s is Shape.Circle") && h.contains("**Variant test**"),
        "{h}"
    );
    assert!(h.contains("enums.html#is"), "links the docs: {h}");

    let (line, col) = at("is Type.Int");
    let h = hover_for(src, line, col).expect("hover on a kind test");
    assert!(
        h.contains("a0.getType() is Type.Int") && h.contains("`a0` is narrowed to `int`"),
        "{h}"
    );
}

/// `break` and `continue` hover with what they do to the loop, and link
/// their reference sections.
#[test]
fn loop_exit_keywords_hover() {
    let src = "var t: int = 0\nin go: exec\non go {\n  for i in 0..9 {\n    if i == 2 { continue }\n    if i == 5 { break }\n    t += i\n  }\n}\n";
    for (line, word, says, link) in [
        (
            4usize,
            "continue",
            "**Skip to the next iteration**",
            "statements.html#continue",
        ),
        (5, "break", "**Stop the loop**", "statements.html#break"),
    ] {
        let col = src.lines().nth(line).unwrap().find(word).unwrap() + 1;
        let h = hover_for(src, line, col).unwrap_or_else(|| panic!("{word} should hover"));
        assert!(h.contains(says) && h.contains(link), "{word}: {h}");
    }
}

/// A builtin's hover says what the gate does, in the game's own words from
/// the gate inventory, even for a gate the older `gate_docs.json` lacks.
#[test]
fn builtin_hover_explains_the_gate_from_the_inventory() {
    let src = "in crate: entity\nin v: any\nin go: exec\non go {\n  crate.SetVisibleTo()\n  let k = GetValueType(v)\n}";
    for (line, name, says) in [
        (
            4usize,
            "SetVisibleTo",
            "Shows a physics object or dynamic brick grid only to one player or team",
        ),
        (
            5,
            "GetValueType",
            "Outputs which type of value is on the wire",
        ),
    ] {
        assert!(
            gate_docs().get(calls()[name].gate_class).is_none(),
            "{name} is meant to be missing from gate_docs.json"
        );
        let col = src.lines().nth(line).unwrap().find(name).unwrap();
        let h = hover_for(src, line, col).unwrap_or_else(|| panic!("{name} should hover"));
        assert!(h.contains(says), "{name}: {h}");
    }
}

#[test]
fn builtin_hover_shows_its_defaults_table() {
    // A gate hover lists its registered defaults (brdb STRUCT_DEFAULTS) in
    // a table: DisplayText's FontSize, its per-axis Vector2D layout
    // sub-ports as float rows, its colors as sRGB hex.
    check_hovers(&[hov(
        "DisplayText's defaults table",
        "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\")\n}",
        3,
        ("DisplayText", 0, 0),
        &[
            "**Defaults:**",
            "| Parameter | Type | Default |",
            "fontSize",
            "anchorY",
            "pivotX",
            "#181425",
            "#ffffff",
        ],
    )]);
}

#[test]
fn named_param_hover_shows_its_default() {
    check_hovers(&[
        // Hovering a named arg (`fontSize = …`) shows that param's default.
        hov(
            "a named arg shows its default",
            "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\", fontSize = 20)\n}",
            3,
            ("fontSize", 0, 0),
            &["Default: `"],
        ),
        // Hovering the `anchorY =` named arg shows that axis of the parent
        // Anchor Vector2D default (0.5). A real param name never trips the
        // WS041 check.
        hov(
            "a sub-port arg shows that axis of the parent default",
            "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\", anchorY = 0.9)\n}",
            3,
            ("anchorY", 0, 0),
            &["Default: `0.5`"],
        ),
    ]);
}

/// A variable named after a method hovers as the variable: method hovers
/// apply only to a `.method` access or a call, not a bare identifier that
/// happens to share the name.
#[test]
fn a_var_named_like_a_method_hovers_as_the_var() {
    check_hovers(&[
        hov(
            "a var named like the array method `sum`",
            "var sum: int = 0",
            0,
            ("", 0, 5),
            &["var sum"],
        )
        .lacks(&["array.sum"]),
        hov(
            "a var named like the builtin receiver method `Teleport`",
            "var Teleport: int = 0",
            0,
            ("", 0, 6),
            &["var Teleport"],
        ),
    ]);
}

/// A map receiver whose declared type is an alias still dispatches to the
/// map table: the symbol carries the alias name, so resolution has to see
/// through it.
#[test]
fn a_map_method_through_an_alias_hovers_as_a_map_method() {
    check_hovers(&[
        hov(
            "`type Scores = Map<...>`",
            "type Scores = Map<string, int>\nvar s: Scores\nin t: exec\non t { let a = s.get(\"x\") }",
            3,
            (".get", 0, 2),
            &["map.get"],
        ),
        // A generic alias resolves by its base name, and the hover shows
        // the declared type.
        hov(
            "`type Grid<T> = Map<string, T>`; `var g: Grid<int>`",
            "type Grid<T> = Map<string, T>\nvar g: Grid<int>\nin t: exec\non t { let a = g.get(\"x\") }",
            3,
            (".get", 0, 2),
            &["map.get", "Grid<int>"],
        ),
    ]);
}

#[test]
fn builtin_event_hover_uses_arrow_capture_form() {
    // Hovering a built-in event name shows the call + capture signature
    // `on CharacterSpawned() -> (character: character)`, NOT the old inline
    // `on CharacterSpawned(character: character)` form.
    let src = "on CharacterSpawned() -> (ch) {\n}\n";
    let col = src
        .lines()
        .next()
        .unwrap()
        .find("CharacterSpawned")
        .unwrap();
    let h = hover_for(src, 0, col).expect("event should hover");
    assert!(
        h.contains("on CharacterSpawned() -> (character: character)"),
        "event hover should use the arrow capture form: {h}"
    );
    assert!(
        !h.contains("on CharacterSpawned(character"),
        "event hover must not use the old inline form: {h}"
    );
}

#[test]
fn custom_event_generic_hover_config_naming() {
    // The generic (channel-less) CustomEvent hover shows the config-cased
    // `isObject`, the channel as a single `"name"` placeholder (NOT also the
    // internal `EventName` positional), and the untyped data slots.
    let h = hover_builtin_event("CustomEvent").expect("CustomEvent should hover");
    assert!(
        h.contains("isObject"),
        "config arg shown display-cased: {h}"
    );
    assert!(!h.contains("isobject"), "config arg not lowercased: {h}");
    assert!(
        !h.contains("EventName"),
        "channel shown once as \"name\", not also as EventName: {h}"
    );
    assert!(
        h.contains("on CustomEvent(\"name\""),
        "channel placeholder: {h}"
    );
    assert!(h.contains("data1: any"), "untyped data slots: {h}");
}

#[test]
fn custom_event_hover_resolves_channel_and_types() {
    // Hovering the `CustomEvent` trigger shows the channel name plus each
    // data slot's name/type: declared by the receiver, and filled from a
    // matching sender for the slot the receiver left untyped (`attacker`).
    let src = "on CustomEvent(\"dmg\") -> (amount: int, attacker) {\n  let x = amount\n}\n\
                   on CharacterSpawned() -> (ch) {\n  SendCustomEvent(\"dmg\", 5, ch)\n}\n";
    let line0 = src.lines().next().unwrap();
    let col = line0.find("CustomEvent").unwrap();
    let h = hover_for(src, 0, col).expect("CustomEvent trigger should hover");
    assert!(
        h.contains("on CustomEvent(\"dmg\""),
        "channel name in sig: {h}"
    );
    assert!(h.contains("amount: int"), "declared slot type: {h}");
    assert!(
        h.contains("attacker: character"),
        "sender-filled slot type: {h}"
    );
}

#[test]
fn hover_resolves_shadowed_var_by_scope() {
    // A file-scope `players: string` and a handler-local `players:
    // character[]`. Hovering the array declaration must resolve to that
    // in-scope array, not the file-scope string a flat lookup finds first.
    let src = "var players: string = \"\"\non t {\n  var players: character[]\n}";
    let decl_line = 2usize; // `  var players: character[]`
    let c = src.lines().nth(decl_line).unwrap().find("players").unwrap();
    let h = hover_for(src, decl_line, c).expect("array decl should hover");
    assert!(
        h.contains("character[]"),
        "hover should show the in-scope array type: {h}"
    );
    assert!(
        !h.contains("string"),
        "must not show the shadowed string type: {h}"
    );
}

#[test]
fn global_custom_event_hover_uses_global_senders() {
    // The global namespace resolves against `SendGlobalCustomEvent` senders,
    // not personal ones (separate channel namespaces), and fills an untyped
    // receiver slot from the sender's inferred arg type.
    let src = "on GlobalCustomEvent(\"score\") -> (points) {\n}\n\
                   on go {\n  SendGlobalCustomEvent(\"score\", 10)\n}\nin go: exec\n";
    let l0 = src.lines().next().unwrap();
    let c = l0.find("GlobalCustomEvent").unwrap();
    let h = hover_for(src, 0, c).expect("GlobalCustomEvent trigger should hover");
    assert!(h.contains("on GlobalCustomEvent(\"score\""), "channel: {h}");
    assert!(h.contains("points: int"), "int sender fills the slot: {h}");
}

#[test]
fn send_custom_event_call_hover_shows_channel_typings() {
    // Hovering `SendCustomEvent` on the SEND call shows the channel's typed
    // fields (resolved from the matching receiver declaration).
    let src = "on CustomEvent(\"dmg\") -> (amount: int, attacker: character) {\n}\n\
                   on go {\n  SendCustomEvent(\"dmg\", 5, ch)\n}\nin go: exec\n";
    let send_line = 3usize; // the `SendCustomEvent(...)` line
    let l = src.lines().nth(send_line).unwrap();
    let c = l.find("SendCustomEvent").unwrap();
    let h = hover_for(src, send_line, c).expect("SendCustomEvent call should hover");
    assert!(
        h.contains("SendCustomEvent(\"dmg\""),
        "channel in send hover: {h}"
    );
    assert!(
        h.contains("amount: int"),
        "receiver-declared type shown: {h}"
    );
    assert!(
        h.contains("attacker: character"),
        "second declared type shown: {h}"
    );
}

#[test]
fn namespace_member_hovers_with_signature() {
    // Hovering the member in `card.drawCard` (a namespace-qualified call)
    // must show its signature, not nothing. The member is stored under the
    // qualified `card.drawCard` symbol name, which the bare-word lookup in
    // hover_user_symbol misses — go-to-definition worked but hover didn't.
    use crate::resolve::MemLoader;
    let loader = MemLoader {
        files: [(
            "display.ws".to_string(),
            "mod drawCard(n: int, label: string) {}".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let src = "import * as card from \"display\"\non RoundStart() { card.drawCard(1, \"hi\") }";
    let col = src.lines().nth(1).unwrap().find("drawCard").unwrap();
    let text = hover_with_loader(src, "main", &loader, 1, col)
        .expect("hover on a namespace member should return something");
    assert!(
        text.contains("drawCard"),
        "should name the member, got: {text}"
    );
    assert!(
        text.contains("n: int"),
        "should show the signature, got: {text}"
    );
    assert!(
        !text.contains("unknown"),
        "must not show `unknown`, got: {text}"
    );
}

#[test]
fn generic_mod_hover_shows_type_params() {
    // Hovering a generic mod's name shows its `<T: Numeric>` generics, not
    // just `(v: T) -> T`.
    let src = "mod square<T: Numeric>(v: T) -> T { return v * v }\n";
    let col = src.find("square").unwrap() + 1;
    let h = hover_for(src, 0, col).expect("hover on generic mod");
    assert!(
        h.contains("<T: Numeric>"),
        "hover should show generics: {h}"
    );
    assert!(h.contains("v: T"), "hover should still show params: {h}");
}

#[test]
fn generic_call_hover_shows_resolved_type_args() {
    // Hovering the CALL site of a generic mod shows the type args resolved
    // for THAT call (`<int>`, `v: int`, `-> int`), not the declaration's
    // `<T: Numeric>`.
    let src = "mod square<T: Numeric>(v: T) -> T { return v * v }\n\
                   on ReadBrickGrid() {\n  let n = square(5)\n}\n";
    let call_line = 2usize;
    let col = src.lines().nth(call_line).unwrap().find("square").unwrap() + 1;
    let h = hover_for(src, call_line, col).expect("hover on generic call");
    assert!(
        h.contains("square<int>"),
        "angle brackets resolved to int: {h}"
    );
    assert!(h.contains("v: int"), "param type substituted: {h}");
    assert!(h.contains("-> int"), "return type substituted: {h}");
    assert!(
        !h.contains("<T:"),
        "must not show the generic form at a call: {h}"
    );
    assert!(h.contains("`T` = `int`"), "resolution note present: {h}");
}

#[test]
fn generic_call_hover_uses_explicit_type_args() {
    // An explicit `f<float>(...)` pins the type param even when the argument
    // would infer something narrower.
    let src = "mod id<T>(v: T) -> T { return v }\n\
                   on ReadBrickGrid() {\n  let n = id<float>(1.0)\n}\n";
    let call_line = 2usize;
    let col = src
        .lines()
        .nth(call_line)
        .unwrap()
        .find("id<float>")
        .unwrap()
        + 1;
    let h = hover_for(src, call_line, col).expect("hover on explicit generic call");
    assert!(h.contains("id<float>"), "explicit type arg shown: {h}");
    assert!(
        h.contains("v: float"),
        "param substituted from explicit arg: {h}"
    );
}

#[test]
fn generic_call_hover_resolves_per_argument() {
    // A `mod assert<T: int | float | string>(...)` called with a string
    // argument resolves `T` to `string` for that call.
    let src = "mod assert<T: int | float | string>(want: T, got: T, label: string) { if want == got { BroadcastChatMessage(label) } }\non ReadBrickGrid() {\n  assert(\"a\", \"a\", \"eq\")\n}\n";
    let call_line = 2usize;
    let col = src.lines().nth(call_line).unwrap().find("assert").unwrap() + 1;
    let h = hover_for(src, call_line, col).expect("hover on assert call");
    assert!(h.contains("assert<string>"), "T resolved to string: {h}");
    assert!(
        h.contains("want: string") && h.contains("got: string"),
        "params substituted: {h}"
    );
    assert!(
        h.contains("label: string"),
        "non-generic param unchanged: {h}"
    );
}

#[test]
fn constraint_class_hovers() {
    // Hovering the bound `Numeric` shows the class + its members.
    let src = "mod square<T: Numeric>(v: T) -> T { return v * v }\n";
    let col = src.find("Numeric").unwrap() + 1;
    let h = hover_for(src, 0, col).expect("hover on Numeric");
    assert!(h.contains("constraint class"), "Numeric hover: {h}");
    assert!(
        h.contains("vector"),
        "Numeric should list members incl vector: {h}"
    );
}

#[test]
fn builtin_type_hovers() {
    // Hovering a primitive type annotation shows its description.
    let src = "mod f(v: int) -> int { return v }\n";
    let col = src.find(": int").unwrap() + 2; // on the `int` after `v: `
    let h = hover_for(src, 0, col).expect("hover on int");
    assert!(h.contains("64-bit signed integer"), "int hover: {h}");
}

#[test]
fn type_param_hovers() {
    // Hovering the `T` in `v: T` resolves to the generic parameter (with its
    // bound), rather than falling through to nothing.
    let src = "mod square<T: Numeric>(v: T) -> T { return v * v }\n";
    let col = src.find(": T").unwrap() + 2; // on the `T` in `(v: T)`
    let h = hover_for(src, 0, col).expect("hover on type parameter T");
    assert!(h.contains("generic type parameter"), "T hover: {h}");
    assert!(h.contains("T: Numeric"), "T hover should show bound: {h}");
    // The bound is a constraint class, so its concrete members are expanded.
    assert!(
        h.contains("vector"),
        "bounded T hover should expand class members: {h}"
    );
}

#[test]
fn type_param_hovers_through_ref_and_shows_scalar_members() {
    // Hovering the `T` inside a `*T` ref param (a real example shape) still
    // resolves to the generic parameter, and a `Scalar` bound expands to
    // its members (int, float).
    let src = "mod inc<T: Scalar>(v: *T) { v = v + 1 }\n";
    let col = src.find("*T").unwrap() + 1; // on the `T` in `*T`
    let h = hover_for(src, 0, col).expect("hover on T inside *T");
    assert!(h.contains("generic type parameter"), "ref-T hover: {h}");
    assert!(h.contains("T: Scalar"), "should show the Scalar bound: {h}");
    assert!(
        h.contains("int") && h.contains("float"),
        "Scalar should expand to int/float: {h}"
    );
}

#[test]
fn record_type_field_doc_comment_shows_on_hover() {
    let src = "type Point = {\n  /// the x coordinate\n  x: int,\n  y: int,\n}";
    // `x` is on line 2 (0-based); hover it.
    let col_x = src.lines().nth(2).unwrap().find('x').unwrap();
    let hx = hover_for(src, 2, col_x).expect("hover on documented field x");
    assert!(hx.contains("Point.x: int"), "field type missing: {hx}");
    assert!(hx.contains("the x coordinate"), "field doc missing: {hx}");
    // The undocumented field `y` shows no doc.
    let col_y = src.lines().nth(3).unwrap().find('y').unwrap();
    let hy = hover_for(src, 3, col_y).expect("hover on field y");
    assert!(hy.contains("Point.y: int"), "y type missing: {hy}");
    assert!(!hy.contains("coordinate"), "y should have no doc: {hy}");
}

#[test]
fn namespace_alias_hovers_with_members_not_unknown() {
    use crate::resolve::MemLoader;
    let loader = MemLoader {
        files: [(
            "display.ws".to_string(),
            "mod drawCard(n: int) {}\nlet WIDTH = 10".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let src = "import * as card from \"display\"";
    let col = src.find("card").unwrap();
    let text = hover_with_loader(src, "main", &loader, 0, col)
        .expect("hover on a namespace alias should return something");
    assert!(
        text.contains("namespace card"),
        "should show namespace, got: {text}"
    );
    assert!(
        !text.contains("unknown"),
        "must not show `unknown`, got: {text}"
    );
    assert!(
        text.contains("drawCard"),
        "should list members, got: {text}"
    );
}

#[test]
fn destructured_mod_param_shows_type() {
    let src = "\
type State = { counter: *int, step: int }
mod bump({ counter, step }: State) { counter = counter + step }";
    // "counter" starts at col 11, "step" at col 20 on line 1
    let h = hover_for(src, 1, 11);
    assert!(
        h.is_some(),
        "hover on destructured param 'counter' should return something"
    );
    let text = h.unwrap();
    assert!(
        text.contains("*int"),
        "hover should show *int for counter, got: {text}"
    );

    let h2 = hover_for(src, 1, 20);
    assert!(
        h2.is_some(),
        "hover on destructured param 'step' should return something"
    );
    let text2 = h2.unwrap();
    assert!(
        text2.contains("int") && !text2.contains("*int"),
        "hover should show int for step, got: {text2}"
    );
}

#[test]
fn named_arg_value_sharing_param_name_hovers_as_symbol() {
    // `Sleep(_, delay = delay)`: only the LHS is the named arg; the RHS is
    // a user symbol that merely shares the param's name and must hover as
    // the symbol, not as the param docs.
    let src = "let delay = 1.0\non RoundStart() { await Sleep(_, delay = delay) }";
    let line1 = src.lines().nth(1).unwrap();
    let lhs = line1.find("delay").unwrap();
    let rhs = line1.rfind("delay").unwrap();

    let hl = hover_for(src, 1, lhs).expect("hover on the arg name should return something");
    assert!(
        hl.starts_with("**"),
        "arg-name hover should be the named-param docs, got: {hl}"
    );
    let hr = hover_for(src, 1, rhs).expect("hover on the value should return something");
    assert!(
        hr.contains("let delay"),
        "value hover must be the user symbol, not the named-param docs, got: {hr}"
    );

    // Same on a continuation line of a multi-line call.
    let src2 = "let delay = 1.0\non RoundStart() {\n  await Sleep(_,\n    delay = delay,\n  )\n}";
    let line3 = src2.lines().nth(3).unwrap();
    let hl2 = hover_for(src2, 3, line3.find("delay").unwrap())
        .expect("hover on the multi-line arg name should return something");
    assert!(
        hl2.starts_with("**"),
        "multi-line arg-name hover should be the named-param docs, got: {hl2}"
    );
    let hr2 = hover_for(src2, 3, line3.rfind("delay").unwrap())
        .expect("hover on the multi-line value should return something");
    assert!(
        hr2.contains("let delay"),
        "multi-line value hover must be the user symbol, got: {hr2}"
    );
}

#[test]
fn self_receiver_method_hovers_as_mod() {
    // Hovering `.dist` in `a.dist(b)` shows the user `self`-mod's signature
    // (the bare-word symbol lookup resolves `dist` to its declaration).
    let src = "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
                   in a: vector\nin b: vector\nin go: exec\non go { let d = a.dist(b) }";
    let line = 4;
    let l = src.lines().nth(line).unwrap();
    let col = l.find(".dist").unwrap() + 2; // inside `dist`
    let h = hover_for(src, line, col).expect("hover on .dist should return the mod");
    assert!(h.contains("dist"), "hover should name the mod: {h}");
    assert!(
        h.contains("self: vector"),
        "hover should show the self-mod signature: {h}"
    );
}

#[test]
fn builtin_call_in_call_position_still_hovers() {
    // A builtin used as an actual call still hovers as the call.
    let src = "in t: exec\non t { PrintToConsole(\"hi\") }";
    // "PrintToConsole" starts at col 7 on line 1
    let h = hover_for(src, 1, 10).expect("hover on PrintToConsole call should return");
    assert!(
        h.contains("PrintToConsole"),
        "call-position builtin should still hover, got: {h}"
    );
}

#[test]
fn array_method_access_still_hovers_as_method() {
    // The `.sum` access must still show the array method hover.
    let src = "\
var fa: int[] = [5, 10, 15]
on load { let s = fa.sum() }";
    // "sum" in "fa.sum()" starts at col 20 on line 1
    let h = hover_for(src, 1, 21).expect("hover on `.sum` should return something");
    assert!(
        h.contains("array.sum"),
        "hover on `fa.sum()` should show the array method, got: {h}"
    );
}

#[test]
fn map_method_access_hovers_as_map_not_array() {
    // Method hovers dispatch on the RECEIVER's type. On a `Map` receiver:
    //  - map-only names (`get`, `has`) that no array has must hover;
    //  - shared names (`clear`) must show the MAP method, not the array one;
    //  - the concrete key/value types appear in the hover.
    let src = "\
var scores: Map<string, int>
in t: exec
on t {
  let a = scores.get(\"x\")
  let h = scores.has(\"x\")
  scores.clear()
}";
    let col_in = |line: usize, needle: &str| {
        let l = src.lines().nth(line).unwrap();
        l.find(needle).unwrap() + 2 // land inside the method name
    };

    let hg = hover_for(src, 3, col_in(3, ".get")).expect("map .get should hover");
    assert!(hg.contains("map.get"), "map .get hover: {hg}");
    assert!(
        hg.contains("Map<"),
        "map .get shows the concrete map type: {hg}"
    );

    let hh = hover_for(src, 4, col_in(4, ".has")).expect("map .has should hover");
    assert!(hh.contains("map.has"), "map .has hover: {hh}");

    let hc = hover_for(src, 5, col_in(5, ".clear")).expect("map .clear should hover");
    assert!(
        hc.contains("map.clear"),
        "shared name resolves to map: {hc}"
    );
    assert!(
        !hc.contains("array.clear"),
        "must not be the array hover: {hc}"
    );
}

#[test]
fn record_field_hover() {
    let src = "\
type Point = { x: int, y: int }
let p: Point = { x: 1, y: 2 }
let v = p.x";
    // hover on "x" in "p.x" (line 2, col 10)
    let h = hover_for(src, 2, 10);
    assert!(
        h.is_some(),
        "hover on record field 'x' should return something"
    );
    let text = h.unwrap();
    assert!(
        text.contains("x: int"),
        "hover on p.x should show x: int, got: {text}"
    );
}

#[test]
fn prefab_file_reference_hover() {
    // `$` at col 8; token spans cols 8..=25.
    let src = "let p = $./prefab_1x1.brz";
    for col in [8usize, 12, 24] {
        let h = hover_for(src, 0, col);
        assert!(
            h.is_some(),
            "hover on prefab ref at col {col} should return"
        );
        let text = h.unwrap();
        assert!(
            text.contains("Prefab file reference") && text.contains("Resolves to"),
            "col {col} got: {text}"
        );
    }
}

#[test]
fn asset_reference_hover() {
    // `$Weapon/Sword`: `$` at col 8, "Weapon" 9-15, "Sword" 16-21.
    let src = "let w = $Weapon/Sword";
    let h = hover_for(src, 0, 11).expect("hover on asset ref should return");
    assert!(
        h.contains("Asset reference") && h.contains("Weapon") && h.contains("Sword"),
        "got: {h}"
    );
}
#[test]
fn array_read_out_of_bounds_field_is_bool() {
    // `arr[i]` is typed as the bare element, so once it is bound to a `let`
    // the bounds flag has no record to resolve against and used to fall
    // through to Any - which is universal, so nothing downstream complained.
    let src = "var names: string[]
in go: exec
on go {
  let n = names[0]
  let b = n.OutOfBounds
}";
    let line4 = src.lines().nth(4).unwrap();
    let col = line4.find("OutOfBounds").unwrap();
    let h = hover_for(src, 4, col).expect("hover on .OutOfBounds should return something");
    assert!(
        h.contains("bool"),
        "array-read bounds flag should type as bool, got: {h}"
    );
}

#[test]
fn call_result_field_access_hover_shows_field_type() {
    // `arr.find(x).Found` - the object is a call result, not an
    // identifier, so the field type must resolve from the call
    // expression's record type in the type map.
    let src = "var ids: string[]
chip a(uid: string) -> int {
  return if ids.find(uid).Found then 1 else 0
}";
    let line2 = src.lines().nth(2).unwrap();
    let col = line2.find("Found").unwrap();
    let h = hover_for(src, 2, col).expect("hover on .Found should return something");
    assert!(
        h.contains("field Found: bool"),
        "call-result field hover should type from the record, got: {h}"
    );
}

#[test]
fn record_destructured_let_hover_shows_field_types() {
    // `let { Found, Index } = ids.find(uid)` - each destructured name
    // takes its field's type from the initializer's record type.
    let src = "var ids: string[]
chip a(uid: string) -> int {
  let { Found, Index } = ids.find(uid)
  return if Found then Index else -1
}";
    let line2 = src.lines().nth(2).unwrap();
    let h = hover_for(src, 2, line2.find("Found").unwrap()).expect("hover on Found");
    assert!(
        h.contains("bool"),
        "destructured Found should be bool, got: {h}"
    );
    let h2 = hover_for(src, 2, line2.find("Index").unwrap()).expect("hover on Index");
    assert!(
        h2.contains("int"),
        "destructured Index should be int, got: {h2}"
    );
    // Usages resolve through the same symbol.
    let line3 = src.lines().nth(3).unwrap();
    let h3 = hover_for(src, 3, line3.find("Found").unwrap()).expect("hover on Found use");
    assert!(h3.contains("bool"), "Found usage should be bool, got: {h3}");
}

#[test]
fn enum_config_param_hovers_with_enum_name_and_members() {
    // Hovering the config arg name `blendSpace` names its schema enum
    // (EBRColorSpace) instead of `int`, and lists the member names.
    let src =
        "in a: color\nin b: color\nin t: float\nlet c = ColorBlend(a, b, t, blendSpace = Oklab)";
    let line = src.lines().nth(3).unwrap();
    let h = hover_for(src, 3, line.find("blendSpace").unwrap()).expect("hover on blendSpace");
    assert!(h.contains("EBRColorSpace"), "should name the enum: {h}");
    assert!(!h.contains(": int"), "should not show int: {h}");
    assert!(h.contains("Oklab"), "should list members: {h}");
}

#[test]
fn enum_member_value_hovers_with_enum_type() {
    // Hovering the enum member VALUE (`X_Negative`) names its enum and lists
    // the siblings.
    let src = "in go: exec\non go {\n  SweepSimple(500.0, direction = X_Negative)\n}";
    let line = src.lines().nth(2).unwrap();
    let h = hover_for(src, 2, line.find("X_Negative").unwrap()).expect("hover on X_Negative");
    assert!(h.contains("EBrickDirection"), "should name the enum: {h}");
    assert!(h.contains("X_Positive"), "should list sibling members: {h}");
}

#[test]
fn event_name_hovers_with_config_params() {
    // Hovering an event that carries config (`Clock`) lists its config args.
    let src = "static var n: int = 0\non Clock(interval = 2.0, enabled = true) {\n  n = n + 1\n}";
    let line = src.lines().nth(1).unwrap();
    let h = hover_for(src, 1, line.find("Clock").unwrap()).expect("hover on Clock");
    assert!(h.contains("on Clock"), "should show the event: {h}");
    assert!(h.contains("enabled"), "should list config args: {h}");
}

#[test]
fn event_config_param_hovers() {
    // Hovering an event config arg name (`isObject`) identifies it as
    // CustomEvent config and names the gate field it sets (Clock's
    // `interval` is a wire input, not config).
    let src = "static var n: int = 0\non CustomEvent(\"dmg\", isObject = true) -> (amount: int) {\n  n = amount\n}";
    let line = src.lines().nth(1).unwrap();
    let h = hover_for(src, 1, line.find("isObject").unwrap()).expect("hover on isObject");
    assert!(h.contains("CustomEvent"), "should mention the event: {h}");
    assert!(
        h.contains("bIsObjectEvent"),
        "should name the gate field: {h}"
    );
}

#[test]
fn data_driven_config_field_name_hovers() {
    // Hovering a raw config field name marks it as gate config with its type.
    let src = "in go: exec\non go {\n  SweepSimple(500.0, bOnlyHitPlayerBodyParts = true)\n}";
    let line = src.lines().nth(2).unwrap();
    let h = hover_for(src, 2, line.find("bOnlyHitPlayerBodyParts").unwrap())
        .expect("hover on raw config field");
    assert!(h.contains("gate config"), "should mark as config: {h}");
    assert!(h.contains("bool"), "should show the type: {h}");
}

#[test]
fn data_driven_enum_config_field_hovers_with_enum() {
    // A raw enum config field names its schema enum + members.
    let src = "in go: exec\non go {\n  SweepSimple(500.0, Direction = X_Negative)\n}";
    let line = src.lines().nth(2).unwrap();
    let h = hover_for(src, 2, line.find("Direction ").unwrap())
        .expect("hover on raw enum config field");
    assert!(h.contains("EBrickDirection"), "should name the enum: {h}");
    assert!(h.contains("Y_Positive"), "should list members: {h}");
}

#[test]
fn invalid_field_on_record_hovers_the_record_type() {
    // Hovering an erroring `.z` on a record-typed value surfaces the record
    // type in a fenced (VS-Code-coloured) block listing its valid fields —
    // the editor-native way to get a coloured type, since the diagnostic
    // message that reports the same error is plain text.
    let src = "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet v = p.z";
    let h = hover_for(src, 2, 10).expect("hover on invalid field"); // `z` in `p.z`
    assert!(
        h.contains("```wirescript"),
        "must be a fenced (coloured) block: {h}"
    );
    assert!(
        h.contains("x: int") && h.contains("y: int"),
        "must list the record's fields: {h}"
    );
}

/// The dropped-code warning covers a REGION, so it must answer everywhere
/// in that region — not only on identifiers. It originally sat behind
/// `hover_at`'s `word_at(...)?` early return, which returns `None` for any
/// non-identifier position, so the warning silently vanished on exactly the
/// characters someone skimming a block is most likely to point at: the
/// braces, the spaces, and the `=`. Those three are the regression guard.
#[test]
fn a_dropped_block_reports_at_braces_whitespace_and_operators_not_just_identifiers() {
    let src = "const MODE = 1\nvar x: int = 0\nin go: exec\non go {\n  if MODE == 1 { x = 1 } else { x = 2 }\n}\n";
    let line = src.lines().nth(4).unwrap();
    // The dropped ELSE block: `{ x = 2 }`, indices 0..=8 from its `{`.
    let open = line.find("{ x = 2 }").expect("else block in the fixture");
    for (offset, what) in [
        (0, "the opening brace `{`"),
        (1, "the space after `{`"),
        (2, "the `x` identifier"),
        (3, "the space before `=`"),
        (4, "the `=` operator"),
        (6, "the `2` literal"),
        (7, "the space before `}`"),
        (8, "the closing brace `}`"),
    ] {
        let col = open + offset;
        let h = hover_for(src, 4, col)
            .unwrap_or_else(|| panic!("no hover at all on {what} (col {col})"));
        assert!(
            h.contains("Removed at compile time"),
            "hovering {what} (col {col}) must report the dropped block, got: {h}"
        );
    }
}

/// The other half of the same property: the warning must be confined to the
/// dropped range. A containment check that always fired would pass the test
/// above while marking live code as removed.
#[test]
fn the_dropped_warning_does_not_leak_onto_the_taken_block() {
    let src = "const MODE = 1\nvar x: int = 0\nin go: exec\non go {\n  if MODE == 1 { x = 1 } else { x = 2 }\n}\n";
    let line = src.lines().nth(4).unwrap();
    let taken = line.find("{ x = 1 }").expect("then block in the fixture");
    for offset in 0..=8 {
        let col = taken + offset;
        let h = hover_for(src, 4, col).unwrap_or_default();
        assert!(
            !h.contains("Removed at compile time"),
            "the TAKEN block is live code; col {col} must not be marked removed, got: {h}"
        );
    }
}

/// Mirror of the two tests above, for the opposite branch direction: a
/// const-FALSE condition drops the THEN block and keeps the `else`. The
/// two directions are recorded by separate arms of `Stmt::If`'s
/// const-elision (one pushes `else_block.range`, the other
/// `then_block.range`), so a bug in the arm that is never exercised —
/// pushing the wrong range, or not pushing at all — would be invisible to
/// a suite that only ever tests a true condition.
#[test]
fn a_const_false_condition_marks_the_then_block_and_leaves_the_else_alone() {
    // MODE is 1, so `MODE == 2` is const-FALSE: `then` goes, `else` stays.
    let src = "const MODE = 1\nvar x: int = 0\nin go: exec\non go {\n  if MODE == 2 { x = 1 } else { x = 2 }\n}\n";
    let line = src.lines().nth(4).unwrap();

    let dropped = line.find("{ x = 1 }").expect("then block in the fixture");
    for (offset, what) in [
        (0, "the opening brace `{`"),
        (1, "the space after `{`"),
        (2, "the `x` identifier"),
        (4, "the `=` operator"),
        (6, "the `1` literal"),
        (8, "the closing brace `}`"),
    ] {
        let col = dropped + offset;
        let h = hover_for(src, 4, col)
            .unwrap_or_else(|| panic!("no hover at all on {what} (col {col})"));
        assert!(
            h.contains("Removed at compile time"),
            "the THEN block is dropped here; hovering {what} (col {col}) must say so, got: {h}"
        );
        assert!(
            h.contains("is false here"),
            "the reason must name the condition's const value, got: {h}"
        );
    }

    let taken = line.find("{ x = 2 }").expect("else block in the fixture");
    for offset in 0..=8 {
        let col = taken + offset;
        let h = hover_for(src, 4, col).unwrap_or_default();
        assert!(
            !h.contains("Removed at compile time"),
            "the ELSE block is the TAKEN one here; col {col} must not be marked removed, got: {h}"
        );
    }
}

/// One hover expectation: the cursor sits `offset` columns into the
/// `nth` occurrence of `needle` on 0-based `line` (an empty needle puts
/// it at column `offset`), and the hover must contain every `has`
/// string, every `has_lower` string once lowercased, and no `lacks` one.
struct Hov {
    label: &'static str,
    src: &'static str,
    line: usize,
    needle: &'static str,
    nth: usize,
    offset: usize,
    has: Vec<String>,
    has_lower: &'static [&'static str],
    lacks: &'static [&'static str],
}

fn hov(
    label: &'static str,
    src: &'static str,
    line: usize,
    at: (&'static str, usize, usize),
    has: &[&str],
) -> Hov {
    let (needle, nth, offset) = at;
    Hov {
        label,
        src,
        line,
        needle,
        nth,
        offset,
        has: has.iter().map(|s| s.to_string()).collect(),
        has_lower: &[],
        lacks: &[],
    }
}

impl Hov {
    fn lacks(mut self, lacks: &'static [&'static str]) -> Self {
        self.lacks = lacks;
        self
    }
    fn has_lower(mut self, has_lower: &'static [&'static str]) -> Self {
        self.has_lower = has_lower;
        self
    }
    fn also(mut self, s: String) -> Self {
        self.has.push(s);
        self
    }
}

/// Run every row, then report all that failed.
#[track_caller]
fn check_hovers(rows: &[Hov]) {
    let mut failures = Vec::new();
    for r in rows {
        let text = r
            .src
            .lines()
            .nth(r.line)
            .unwrap_or_else(|| panic!("{}: no line {}", r.label, r.line));
        let Some((at, _)) = text.match_indices(r.needle).nth(r.nth) else {
            failures.push(format!(
                "{}: no `{}` #{} on line {}",
                r.label, r.needle, r.nth, r.line
            ));
            continue;
        };
        let Some(h) = hover_for(r.src, r.line, at + r.offset) else {
            failures.push(format!("{}: no hover", r.label));
            continue;
        };
        let lower = h.to_lowercase();
        let missing: Vec<&str> = r
            .has
            .iter()
            .map(String::as_str)
            .filter(|s| !h.contains(s))
            .collect();
        let missing_lower: Vec<&&str> = r
            .has_lower
            .iter()
            .filter(|s| !lower.contains(**s))
            .collect();
        let present: Vec<&&str> = r.lacks.iter().filter(|s| h.contains(**s)).collect();
        if !missing.is_empty() || !missing_lower.is_empty() || !present.is_empty() {
            failures.push(format!(
                    "{}: missing {missing:?}, missing (any case) {missing_lower:?}, unexpected {present:?} in:\n{h}",
                    r.label
                ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

const SHAPE_ENUM_SRC: &str = "enum Shape { Empty, Circle(float), Rect(float, float) }\nvar s: Shape = Shape.Empty\nin go: exec\non go {\n  let d = s.Discriminant\n  let c = Shape.Circle\n  let cd = Shape.Circle.Discriminant\n}\n";
const BOX_ENUM_SRC: &str =
    "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w: 1.0, h: 2.0 }\n";
const EASING_SRC: &str = "var e: EasingFunction = EasingFunction.Bounce\nin go: exec\non go {\n  let d = e.Discriminant\n  let cd = EasingFunction.Bounce.Discriminant\n}\n";

#[test]
fn enum_names_variants_and_discriminants_hover() {
    let bounce = crate::catalog::enum_member_value("EBREasingFunction", "Bounce")
        .expect("Bounce is a real EBREasingFunction member");
    check_hovers(&[
        // Hovering the TYPE name in `var s: Shape` shows it is an enum and
        // lists its variants (with payload shapes).
        hov(
            "user enum type name in a var decl",
            SHAPE_ENUM_SRC,
            1,
            ("Shape", 0, 1),
            &["enum Shape", "Empty", "Circle(float)", "Rect(float, float)"],
        ),
        // The `Shape` half of `Shape.Circle` also resolves to the enum
        // (not the variant - that's a separate word/case).
        hov(
            "user enum type name in a construction path",
            SHAPE_ENUM_SRC,
            5,
            ("Shape", 0, 1),
            &["enum Shape", "Empty", "Circle", "Rect"],
        ),
        // The `Circle` in `Shape.Circle` shows the variant + its
        // discriminant (Empty=0, Circle=1 by declaration order).
        hov(
            "user enum variant shows its discriminant",
            SHAPE_ENUM_SRC,
            5,
            ("Circle", 0, 1),
            &["Shape.Circle", "(float)", "Discriminant", "1"],
        ),
        // `s.Discriminant` where `s: Shape` resolves through the generic
        // field-type hover (`type_map` types `.Discriminant` as
        // `Type::Int` regardless of the object): the projection yields
        // `int`.
        hov(
            "a user enum value's .Discriminant yields int",
            SHAPE_ENUM_SRC,
            4,
            ("Discriminant", 0, 1),
            &["Discriminant", "int"],
        ),
        // `Shape.Circle.Discriminant` is a compile-time CONSTANT equal to
        // Circle's discriminant (1), not just `int`.
        hov(
            "a user variant path's .Discriminant is a compile-time constant",
            SHAPE_ENUM_SRC,
            6,
            ("Discriminant", 0, 1),
            &["Shape.Circle.Discriminant", "= 1"],
        )
        .has_lower(&["compile-time constant"]),
        // An enum-typed var shows its declared type via the ordinary var
        // hover.
        hov(
            "a user enum-typed variable shows its type",
            SHAPE_ENUM_SRC,
            1,
            (" s:", 0, 1),
            &["var s: Shape"],
        ),
        // `w` in `Shape.Box { w: 1.0, h: 2.0 }` shows its declared type
        // from `enum Shape { Box { w: float, h: float } }`.
        hov(
            "a construction field key shows its declared type",
            BOX_ENUM_SRC,
            1,
            ("w:", 0, 1),
            &["w: float", "Shape.Box"],
        ),
        hov(
            "a second construction field key shows its own type",
            BOX_ENUM_SRC,
            1,
            ("h:", 0, 1),
            &["h: float"],
        ),
        // `EasingFunction` is a built-in game enum; its hover lists the
        // variants.
        hov(
            "a built-in enum type name notes the game enum",
            EASING_SRC,
            0,
            ("EasingFunction", 0, 1),
            &["enum EasingFunction", "Bounce"],
        )
        .has_lower(&["built-in game enum"]),
        // The `EasingFunction` half of `EasingFunction.Bounce` also
        // resolves to the enum.
        hov(
            "a built-in enum type name in a construction path",
            EASING_SRC,
            0,
            ("EasingFunction", 1, 1),
            &["enum EasingFunction"],
        ),
        // `Bounce` in `EasingFunction.Bounce` shows the REAL schema
        // discriminant from the catalog, not an auto-numbered index.
        hov(
            "a built-in variant shows the real schema discriminant",
            EASING_SRC,
            0,
            ("Bounce", 0, 1),
            &["EasingFunction.Bounce"],
        )
        .also(format!("{bounce}")),
        // `EasingFunction.Bounce.Discriminant` shows the real schema
        // constant, matching `enum_member_value`.
        hov(
            "a built-in variant path's .Discriminant is the real constant",
            EASING_SRC,
            4,
            ("Discriminant", 0, 1),
            &["EasingFunction.Bounce.Discriminant"],
        )
        .also(format!("= {bounce}")),
        hov(
            "a built-in enum-typed variable shows its type",
            EASING_SRC,
            0,
            (" e:", 0, 1),
            &["var e: EasingFunction"],
        ),
        // `e.Discriminant` resolves through the generic field-type hover,
        // showing it yields `int`.
        hov(
            "a built-in enum value's .Discriminant yields int",
            EASING_SRC,
            3,
            ("Discriminant", 0, 1),
            &["Discriminant", "int"],
        ),
    ]);
}

#[test]
fn hover_construction_field_value_does_not_show_field_hover() {
    // The VALUE side (`1.0`) is not a field key; falls through to the
    // ordinary literal/no-hover path rather than the field-key hover.
    let l = BOX_ENUM_SRC.lines().nth(1).unwrap();
    let col = l.find("1.0").unwrap();
    let h = hover_for(BOX_ENUM_SRC, 1, col);
    assert!(
        h.is_none_or(|h| !h.contains("Named payload field")),
        "the value side should not show the field-key hover"
    );
}

/// A record-literal key hovers wherever the literal is written: a `var`, an
/// array element type, and a literal nested inside another.
#[test]
fn hover_on_record_literal_keys_in_a_var_array_initializer() {
    let src = "type V = {value: int}\n\
                   type H = {value: V}\n\
                   var hs: H[] = [{value: {value: 10}}]";
    let l = src.lines().nth(2).unwrap();
    let outer = l.find("{value:").unwrap() + 1;
    let inner = l.rfind("value:").unwrap();
    assert_eq!(
        hover_for(src, 2, outer).as_deref(),
        Some("```wirescript\nfield value: {value: int}\n```"),
        "outer key of a nested literal in a var array initializer"
    );
    // The two keys are spelled the same, so the innermost literal
    // containing the cursor is the one that must answer.
    assert_eq!(
        hover_for(src, 2, inner).as_deref(),
        Some("```wirescript\nfield value: int\n```"),
        "inner key of a nested literal"
    );
}

/// The shape the line-scanning resolver did handle must keep working.
#[test]
fn hover_on_a_record_literal_key_in_a_let_still_works() {
    let src = "type V = {value: int}\n\
                   in go: bool\n\
                   let v: V = {value: 10}";
    let l = src.lines().nth(2).unwrap();
    let col = l.find("{value").unwrap() + 1;
    let h = hover_for(src, 2, col).expect("a `let` record literal key should hover");
    assert!(h.contains("value") && h.contains("int"), "hover text: {h}");
}

/// A port read hovers with the port's name and type, wherever it is declared.
#[test]
fn a_port_read_hovers_with_the_ports_type() {
    check_hovers(&[
        // The `y` inside `y * 2`.
        hov(
            "a top-level port",
            "in a: int\nout y: int = a + 1\nout z: int = y * 2",
            2,
            ("", 0, 13),
            &["y: int"],
        ),
        // `flash` is a boundary port hoisted out of the `on Clock` handler, so
        // a read of it from an unrelated handler must still resolve.
        hov(
            "a handler-declared port",
            "on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}\nin go: exec\non go {\nvar b: bool = flash\n}",
            5,
            ("", 0, 14),
            &["flash: bool"],
        ),
        // Same hoisting, for a file-scope anon chip's `out` binding.
        hov(
            "an anon-chip-declared port",
            "chip { out shared: int = 1 }\nin go: exec\non go {\nvar v: int = shared\n}",
            3,
            ("", 0, 13),
            &["shared: int"],
        ),
    ]);
}

/// A `for` binding hovers with the type of the slot it names: a range's
/// index, an array's element (a single binding means the VALUE, not the
/// index), a map's key and value, and a range's named slot.
#[test]
fn for_bindings_hover_with_their_slot_types() {
    check_hovers(&[
        hov(
            "a range binding",
            "var n: int = 0\nin go: exec\non go {\n  for i in 0..n {\n    let x = i\n  }\n}",
            3,
            ("for i", 0, 4),
            &["let i: int"],
        ),
        hov(
            "an array's single binding",
            "var arr: int[]\nin go: exec\non go {\n  for v in arr {\n    let x = v\n  }\n}",
            3,
            ("for v", 0, 4),
            &["let v: int"],
        ),
        hov(
            "a map's key binding",
            "var m: Map<string, int>\nin go: exec\non go {\n  for k, v in m {\n    let x = v\n  }\n}",
            3,
            ("k", 0, 0),
            &["let k: string"],
        ),
        hov(
            "a map's value binding",
            "var m: Map<string, int>\nin go: exec\non go {\n  for k, v in m {\n    let x = v\n  }\n}",
            3,
            ("v in", 0, 0),
            &["let v: int"],
        ),
        hov(
            "a range's `index` slot",
            "var n: int = 0\nin go: exec\non go {\n  for { index } in 0..n {\n    let a = index\n  }\n}",
            3,
            ("index", 0, 0),
            &["let index: int"],
        ),
    ]);
}

/// Every hover column on a line holding non-ASCII text must answer or
/// decline, never panic. This file's resolvers slice with a BYTE offset
/// while the incoming column is a CHAR one, and a mid-character slice
/// kills the LSP request task and poisons the wasm module until reload.
#[test]
fn hover_column_sweep_survives_non_ascii() {
    let src = "// ─── grid ─── ünïcode héré 🙂\n\
                   type Slot = { phase: int }\n\
                   var tk: Slot[]\n\
                   in go: exec\n\
                   on go { tk[0].phase = 5 }\n";
    for (line, text) in src.lines().enumerate() {
        for col in 0..=text.chars().count() + 2 {
            let _ = hover_for(src, line, col);
        }
    }
}

/// The same sweep over a CRLF file, where a line offset summed as
/// `len + 1` loses the `\r` and every type-map lookup past line 1 misses.
#[test]
fn hover_column_sweep_survives_crlf() {
    let src = "type Slot = { phase: int }\r\n\
                   var tk: Slot[]\r\n\
                   in go: exec\r\n\
                   on go { tk[0].phase = 5 }\r\n";
    let col = src.lines().nth(3).unwrap().find("phase").unwrap();
    let h = hover_for(src, 3, col + 1).expect("hover on tk[0].phase in a CRLF file");
    assert!(h.contains("phase") && h.contains("int"), "got: {h}");
    for (line, text) in src.lines().enumerate() {
        for col in 0..=text.chars().count() + 2 {
            let _ = hover_for(src, line, col);
        }
    }
}

#[test]
fn let_for_hovers_as_the_outcome_record() {
    let src = "var n: int = 0\nin go: exec\non go {\n  let run = for i in 0..n {\n  }\n  let ok = run.aborted\n}";
    let col = src.lines().nth(5).unwrap().find("run").unwrap();
    let h = hover_for(src, 5, col).expect("`let run = for` should hover after the loop");
    assert!(
        h.contains("aborted: bool") && h.contains("iterations: int") && !h.contains("abortCode"),
        "got: {h}"
    );
}

/// A `const` computed through a builtin that an import may replace with a
/// user `const mod` shows no value: hover has the file before its imports
/// are merged, so it would show the builtin's answer, not the program's. With
/// no such import, the value shows.
#[test]
fn a_const_through_a_builtin_an_import_may_replace_shows_no_value() {
    use crate::resolve::MemLoader;
    let hover_k = |src: &str| {
        let lib = "const mod Vec(a: float, b: float, c: float) -> float { return 9.0 }";
        let other = "mod twice(v: float) -> float { return v * 2.0 }
const Z = 4";
        let loader = MemLoader {
            files: [
                ("lib.ws".to_string(), lib.to_string()),
                ("other.ws".to_string(), other.to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let (line, text) = src
            .lines()
            .enumerate()
            .find(|(_, l)| l.starts_with("const K"))
            .unwrap();
        hover_with_loader(src, "main", &loader, line, text.find('K').unwrap() + 1)
            .expect("hover on K")
    };
    let h = hover_k(
        "const K = Vec(1.0, 2.0, 3.0)
out o = K",
    );
    assert!(h.contains(" = "), "{h}");
    for src in [
        "import { Vec } from \"lib\"
const K = Vec(1.0, 2.0, 3.0)
out o = K",
        "import { Vec } from \"lib\"
const A = Vec(1.0, 2.0, 3.0)
const K = A + 1.0
out o = K",
        "import \"lib\"
const K = Vec(1.0, 2.0, 3.0)
out o = K",
    ] {
        let h = hover_k(src);
        assert!(h.contains("const K") && !h.contains(" = "), "{src}: {h}");
    }
    // A whole-module import that declares no mod of that name replaces nothing.
    let h = hover_k(
        "import \"other\"
const K = Vec(1.0, 2.0, 3.0)
out o = K",
    );
    assert!(h.contains(" = "), "{h}");
}

/// A `const` in a body shows no value when the name it reads is a parameter,
/// `var` or non-constant `let` hiding the outer constant: compiling reports
/// it as no constant (`WS046`), so the outer value would be a wrong one.
#[test]
fn a_const_reading_a_name_that_hides_a_constant_shows_no_value() {
    let at_q = |src: &str| {
        let (line, text) = src
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains("const q"))
            .unwrap();
        hover_for(src, line, text.find('q').unwrap() + 1).expect("hover on q")
    };
    for body in [
        "mod g(K: int) -> int {\n  const q = K + 1\n  return q\n}",
        "mod g(a: int) -> int {\n  var K: int = a\n  const q = K + 1\n  return q\n}",
        "mod g(a: int) -> int {\n  let K = a\n  const q = K + 1\n  return q\n}",
    ] {
        let h = at_q(&format!("const K = 4\n{body}\n"));
        assert!(h.contains("const q") && !h.contains(" = "), "{body}: {h}");
    }
    let h = at_q("const K = 4\nmod g(a: int) -> int {\n  const q = K + 1\n  return q + a\n}\n");
    assert!(h.contains("= 5"), "the constant itself: {h}");
}

#[test]
fn a_component_type_ref_hovers_with_its_name_and_class() {
    let src = "let t = $Component/PointLight\n";
    let col = src.find("Point").unwrap();
    let h = hover_for(src, 0, col).expect("a component type ref hovers");
    assert!(
        h.contains("Point Light") && h.contains("Component_PointLight"),
        "got: {h}"
    );
    // The dump's category is not the one the game shows.
    assert!(!h.contains("Category"), "got: {h}");
}

/// A property on a component reference hovers like its `$Component/...` form,
/// on a stored var and on a call result alike.
#[test]
fn a_property_on_a_component_reference_hovers_with_its_type() {
    let src = "in go: exec\nin target: brick\nvar light: Component<PointLight>\non go {\n  light = target.AddComponent($Component/PointLight)\n  let a = light.Brightness\n  let b = target.GetComponent($Component/PointLight).Radius\n}\n";
    let at = |needle: &str, word: &str| {
        let line = src.lines().position(|l| l.contains(needle)).unwrap();
        (line, src.lines().nth(line).unwrap().find(word).unwrap())
    };
    let (l, c) = at("let a", "Brightness");
    let h = hover_for(src, l, c).expect("light.Brightness hovers");
    assert!(
        h.contains("PointLight.Brightness") && h.contains("float"),
        "got: {h}"
    );
    let (l, c) = at("let b", "Radius");
    let h = hover_for(src, l, c).expect("a call result's property hovers");
    assert!(h.contains("PointLight.Radius"), "got: {h}");
}

#[test]
fn a_component_property_ref_hovers_with_its_type() {
    let src = "let t = $Component/PointLight/Brightness\n";
    let col = src.find("Bright").unwrap();
    let h = hover_for(src, 0, col).expect("a component property ref hovers");
    assert!(
        h.contains("PointLight.Brightness") && h.contains("float") && h.contains("unverified"),
        "got: {h}"
    );
}

#[test]
fn rotator_and_quat_components_hover_as_floats() {
    assert_eq!(
        resolve_field_in_type(&Type::Rotator, "Pitch"),
        Some(Type::Float)
    );
    assert_eq!(resolve_field_in_type(&Type::Quat, "w"), Some(Type::Float));
}

/// A `fn` hovers as a `fn`, with its own cost line: it is compiled once and
/// each call is one Function Call gate, where a `mod` is inlined per call.
#[test]
fn a_fn_hovers_as_fn_not_mod() {
    let src = "fn add(x: int, y: int = 1) -> int {
  return x + y
}
in go: exec
on go {
  let a = add(3)
}
";
    for (line, col) in [(0, 4), (5, 11)] {
        let h = hover_for(src, line, col).expect("hover on a fn");
        assert!(h.contains("fn add("), "{h}");
        assert!(
            !h.contains("mod add") && !h.contains("inlined per call"),
            "{h}"
        );
        assert!(h.contains("built once, 1 gate per call"), "{h}");
    }
    let kw = hover_for(src, 0, 0).expect("hover on the fn keyword");
    assert!(kw.contains("fn add"), "{kw}");
}

/// An annotation's name hovers as the annotation, not as a symbol that
/// shares its spelling: `@color` is not the `color` type and `@material`
/// is not a pin named `material`.
#[test]
fn annotations_hover_as_annotations() {
    let src = "@right out material = 1\n@color(Color(1.0, 0.0, 0.0)) @material(Glow)\nvar heat: int = 0\n";
    let color = hover_for(src, 1, 2).expect("a hover on @color");
    assert!(
        color.contains("@color(color)") && color.contains("Annotation"),
        "{color}"
    );
    let material = hover_for(src, 1, 32).expect("a hover on @material");
    assert!(material.contains("@material(BrickMaterial)"), "{material}");
    // Without the `@` the same word is still the pin.
    let pin = hover_for(src, 0, 12).expect("a hover on the pin");
    assert!(!pin.contains("Annotation"), "{pin}");
}

/// Hovers on language features end with a link to their docs section; a
/// user's own names get none.
#[test]
fn hovers_link_to_the_docs() {
    let src =
        "@layout(\"code\")\n\nin go: exec\nvar count: int = 0\non go {\n  count = abs(count)\n}\n";
    let docs = "https://wirescript.brickadia.dev/docs/";
    let annotation = hover_for(src, 0, 3).expect("annotation hover");
    assert!(
        annotation.ends_with(&format!(
            "[Docs]({docs}statements.html#layoutcode-source-shaped-gate-layout)"
        )),
        "{annotation}"
    );
    let ty = hover_for(src, 3, 12).expect("type hover");
    assert!(
        ty.ends_with(&format!("[Docs]({docs}types.html#primitive-types)")),
        "{ty}"
    );
    let call = hover_for(src, 5, 11).expect("builtin hover");
    assert!(
        call.ends_with(&format!(
            "[Docs]({docs}builtins.html#math--trigonometry-pure)"
        )),
        "{call}"
    );
    let on = hover_for(src, 4, 0).expect("`on` keyword hover");
    assert!(
        on.ends_with(&format!("[Docs]({docs}statements.html#on-event-handler)")),
        "{on}"
    );
    let var = hover_for(src, 3, 5).expect("variable hover");
    assert!(!var.contains("[Docs]"), "{var}");
}

/// Every segment of `Environment.<group>.<property>` hovers, read or
/// written, and inside a string's `${...}`; what follows the property
/// (`.ToHex()`) and the enum on the right of `=` keep their own hovers.
#[test]
fn environment_paths_hover() {
    let src = "in go: exec\non go {\n  let before = Environment.Sky.TimeOfDay\n  Environment.Sky.TimeOfDay = 12.5\n  Environment.Ambience.ReverbEffect = ReverbEffect.LongHallway\n  BroadcastChatMessage(\"${Environment.Sky.SkyColor.ToHex()}\")\n}\n";
    let lines: Vec<&str> = src.lines().collect();
    let at = |line: usize, needle: &str, nth: usize| {
        let col = lines[line]
            .match_indices(needle)
            .nth(nth)
            .expect("needle")
            .0
            + 1;
        hover_for(src, line, col).unwrap_or_default()
    };
    let sky = "environment.html#environmentsky";
    let read = at(2, "TimeOfDay", 0);
    assert!(
        read.contains("Environment.Sky.TimeOfDay: float") && read.contains(sky),
        "{read}"
    );
    let write = at(3, "TimeOfDay", 0);
    assert!(
        write.contains("Environment.Sky.TimeOfDay: float"),
        "{write}"
    );
    let group = at(3, "Sky", 0);
    assert!(
        group.contains("**Environment group**") && group.contains("`TimeOfDay`"),
        "{group}"
    );
    let root = at(2, "Environment", 0);
    assert!(root.contains("Groups:") && root.contains("`Sky`"), "{root}");
    let enum_prop = at(4, "ReverbEffect", 0);
    assert!(
        enum_prop.contains("Environment.Ambience.ReverbEffect: int (ReverbEffect)"),
        "{enum_prop}"
    );
    let enum_value = at(4, "ReverbEffect", 1);
    assert!(!enum_value.contains("Environment setting"), "{enum_value}");
    let in_string = at(5, "SkyColor", 0);
    assert!(
        in_string.contains("Environment.Sky.SkyColor: color"),
        "{in_string}"
    );
    let method = at(5, "ToHex", 0);
    assert!(!method.contains("Environment setting"), "{method}");
}

/// A string's own text and a comment are prose: a word in them that is
/// spelled like a name gets no hover. `${...}` inside a string is code.
#[test]
fn no_hover_in_string_text_or_comments() {
    let src = "in go: exec\non go {\n  // ReverbEffect in a comment\n  BroadcastChatMessage(\"SEE Ambience.ReverbEffect => ${Environment.Ambience.ReverbEffect}\")\n}\n";
    let lines: Vec<&str> = src.lines().collect();
    let at = |line: usize, needle: &str, nth: usize| {
        let col = lines[line]
            .match_indices(needle)
            .nth(nth)
            .expect("needle")
            .0
            + 1;
        hover_for(src, line, col)
    };
    assert_eq!(at(2, "ReverbEffect", 0), None, "comment");
    assert_eq!(at(3, "ReverbEffect", 0), None, "string text");
    let interp = at(3, "ReverbEffect", 1).expect("interpolation hovers");
    assert!(
        interp.contains("Environment.Ambience.ReverbEffect"),
        "{interp}"
    );
    let call = at(3, "BroadcastChatMessage", 0).expect("call hovers");
    assert!(
        call.contains(
            "[Docs](https://wirescript.brickadia.dev/docs/builtins.html#broadcast-messaging-exec)"
        ),
        "{call}"
    );
}

/// A string pattern's holes are names like any capture: hovering one, where
/// it is bound (inside the pattern, in either spelling) or where it is read,
/// shows it.
#[test]
fn hover_names_a_string_pattern_hole() {
    let src = "in s: string\nvar n: int = 0\nin go: exec\non go {\n  if let \"tp ${where}\" = s { n = where.Length() }\n  if let \"go \" .. dest = s { n = dest.Length() }\n  match s {\n    \"${k}=${v}\" => { n = v.Length() }\n    _ => {}\n  }\n}\n";
    let at = |line: usize, word: &str, nth: usize| {
        let text = src.lines().nth(line).unwrap();
        let col = text.match_indices(word).nth(nth).unwrap().0 + 1;
        hover_for(src, line, col)
    };
    let mut bad = Vec::new();
    for (line, word, nth) in [
        (4, "where", 0),
        (4, "where", 1),
        (5, "dest", 0),
        (5, "dest", 1),
        (7, "k", 0),
        (7, "v", 0),
        (7, "v", 1),
    ] {
        match at(line, word, nth) {
            Some(h) if h.contains(word) && h.contains("string") => {}
            other => bad.push(format!("{word}#{nth}@{line}: {other:?}")),
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// An enum payload capture hovers as the payload's type; a bare unit
/// variant in a pattern is no capture.
#[test]
fn hover_names_an_enum_capture() {
    let src = "enum Jar { Full(int), Empty }\nvar j: Jar = Jar.Empty\nvar n: int = 0\nin go: exec\non go {\n  match j {\n    Full(count) => { n = count }\n    Empty => {}\n  }\n}\n";
    let line = 6;
    let col = src.lines().nth(line).unwrap().find("count").unwrap() + 1;
    let h = hover_for(src, line, col).expect("hover on the capture");
    assert!(h.contains("count") && h.contains("int"), "{h}");
    let line = 7;
    let col = src.lines().nth(line).unwrap().find("Empty").unwrap() + 1;
    assert!(!hover_for(src, line, col).is_some_and(|h| h.contains("let Empty")));
}

/// A declaration's hover shows its kind, name and signature, from the
/// declaration and from a use alike.
#[test]
fn declarations_hover_with_their_signature() {
    check_hovers(&[
        hov(
            "var",
            "var count: int = 0",
            0,
            ("", 0, 4),
            &["var count: int"],
        ),
        hov("an inferred var", "var x = 42", 0, ("", 0, 4), &["var x"]),
        hov(
            "let",
            "var x: int = 0\nlet y = x + 1",
            1,
            ("", 0, 4),
            &["let y"],
        ),
        hov(
            "buffer",
            "var x: int = 0\nbuffer prev = x",
            1,
            ("", 0, 7),
            &["buffer prev"],
        ),
        hov(
            "in",
            "in trigger: exec",
            0,
            ("", 0, 3),
            &["in trigger: exec"],
        ),
        hov(
            "mod",
            "mod inc(v: *int) { v = v + 1 }",
            0,
            ("", 0, 4),
            &["mod inc", "v: *int"],
        ),
        hov(
            "chip",
            "chip Foo(x: int) -> (result: int) {\n  out result = x\n}",
            0,
            ("", 0, 5),
            &["chip Foo"],
        ),
        hov(
            "fn",
            "fn double(x: int) -> int = x * 2",
            0,
            ("", 0, 3),
            &["fn double", "x: int"],
        ),
        hov(
            "a doc comment shows on its mod",
            "/// Increments a value.\nmod inc(v: *int) { v = v + 1 }",
            1,
            ("", 0, 4),
            &["mod inc", "Increments a value."],
        ),
        hov(
            "a record field of a call result",
            "in player: character\nlet input = InputReader(player)\nlet fwd = input.Forward",
            2,
            ("", 0, 16),
            &["Forward: float"],
        ),
        // Hovering anywhere within the event name shows the same info.
        hov(
            "an event, mid-name",
            "on CharacterDied() -> (a) { }",
            0,
            ("", 0, 10),
            &["CharacterDied", "character"],
        ),
        hov(
            "a pure call shows its record output",
            "in ch: character\nlet input = InputReader(ch)",
            1,
            ("", 0, 12),
            &["InputReader", "Forward"],
        ),
    ]);
}

/// A binding initialized from a chained receiver call hovers with the call's
/// result type.
#[test]
fn chained_receiver_calls_hover_with_their_result_type() {
    check_hovers(&[
        hov(
            "Vec.Normalize() is a vector",
            "let foo = Vec(0.0, 1.0, 2.0).Normalize()\nout r = foo",
            1,
            ("", 0, 8),
            &["foo: vector"],
        ),
        hov(
            "Vec.Magnitude() is a float",
            "let foo = Vec(1.0, 2.0, 3.0).Magnitude()\nout r = foo",
            1,
            ("", 0, 8),
            &["foo: float"],
        ),
        hov(
            "a.Dot(b) is a float",
            "let a = Vec(1.0, 0.0, 0.0)\nlet b = Vec(0.0, 1.0, 0.0)\nlet d = a.Dot(b)\nout r = d",
            3,
            ("", 0, 8),
            &["d: float"],
        ),
        hov(
            "a.Cross(b) is a vector",
            "let a = Vec(1.0, 0.0, 0.0)\nlet c = a.Cross(Vec(0.0, 1.0, 0.0))\nout r = c",
            2,
            ("", 0, 8),
            &["c: vector"],
        ),
        hov(
            "Vec.Distance() is a float",
            "let d = Vec(0.0, 0.0, 0.0).Distance(Vec(1.0, 1.0, 1.0))\nout r = d",
            1,
            ("", 0, 8),
            &["d: float"],
        ),
        hov(
            "Vec.ScaleVec() is a vector",
            "let s = Vec(1.0, 2.0, 3.0).ScaleVec(2.0)\nout r = s",
            1,
            ("", 0, 8),
            &["s: vector"],
        ),
        hov(
            "Vec.SplitVec() is {x, y, z}",
            "let p = Vec(1.0, 2.0, 3.0).SplitVec()\nout r = p",
            1,
            ("", 0, 8),
            &["x", "y", "z"],
        ),
        hov(
            "Vec.MagnitudeSq() is a float",
            "let m = Vec(1.0, 2.0, 3.0).MagnitudeSq()\nout r = m",
            1,
            ("", 0, 8),
            &["m: float"],
        ),
        hov(
            "s.Contains() is a bool",
            "var s: string = \"hello\"\nlet r = s.Contains(\"ell\")\nout o = r",
            2,
            ("", 0, 8),
            &["r: bool"],
        ),
        hov(
            "s.ToLower() is a string",
            "var s: string = \"HELLO\"\nlet r = s.ToLower()\nout o = r",
            2,
            ("", 0, 8),
            &["r: string"],
        ),
        hov(
            "s.Length() is an int",
            "var s: string = \"hello\"\nlet r = s.Length()\nout o = r",
            2,
            ("", 0, 8),
            &["r: int"],
        ),
        hov(
            "s.Split() is {Left, Right}",
            "var s: string = \"a,b\"\nlet r = s.Split(\",\")\nout o = r",
            2,
            ("", 0, 8),
            &["Left", "Right"],
        ),
        hov(
            "s.Find() is an int",
            "var s: string = \"hello\"\nlet r = s.Find(\"ll\")\nout o = r",
            2,
            ("", 0, 8),
            &["r: int"],
        ),
        hov(
            "c.SplitColor() is {r, g, b, a}",
            "let c = Color(1.0, 0.0, 0.0)\nlet p = c.SplitColor()\nout r = p",
            2,
            ("", 0, 8),
            &["r", "g", "b"],
        ),
    ]);
}
