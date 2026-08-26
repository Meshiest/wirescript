    use super::*;
    use crate::analysis::collect_symbols_for_file;
    use crate::resolve::{resolve, FsLoader};
    use crate::typecheck::typecheck;
    fn hover_for(source: &str, line: usize, col: usize) -> Option<String> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
        let estimates = crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, "test");
        hover_at(
            source,
            "test",
            &symbols,
            &tc.type_of_expr,
            &resolved.doc_comments,
            &tc.if_contexts,
            &tc.var_read_contexts,
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
        let tc = typecheck(&resolved.ast, "test", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
        // Cursor on the empty line (0-based line 2) inside the `{ }`.
        let fill = fill_record_at(src, &symbols, 2, 0).expect("fill available inside record literal");
        assert!(fill.text.contains("foo: \"\""), "foo default: {}", fill.text);
        assert!(fill.text.contains("bar: { baz: 0 }"), "nested record filled: {}", fill.text);
        assert!(fill.text.contains("baq: { fred: \"\" }"), "nested record filled: {}", fill.text);
    }

    #[test]
    fn fill_record_skips_already_present_fields() {
        let src = "type Card = { foo: string, bar: int }\nlet c: Card = {\n  foo: \"x\",\n\n}";
        let resolved = resolve(src, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
        let fill = fill_record_at(src, &symbols, 3, 0).expect("fill for a partial literal");
        assert!(!fill.text.contains("foo:"), "present field foo skipped: {}", fill.text);
        assert!(fill.text.contains("bar: 0"), "missing field bar filled: {}", fill.text);
    }

    #[test]
    fn builtin_hover_shows_default_values_table() {
        // A gate hover surfaces its registered defaults (from brdb
        // STRUCT_DEFAULTS) in a table — e.g. DisplayText's FontSize default.
        let src = "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\")\n}";
        let call_line = 3usize;
        let col = src.lines().nth(call_line).unwrap().find("DisplayText").unwrap();
        let h = hover_for(src, call_line, col).expect("DisplayText should hover");
        assert!(h.contains("**Defaults:**"), "expected a defaults table: {h}");
        assert!(h.contains("| Parameter | Type | Default |"), "expected table header: {h}");
        assert!(h.contains("fontSize"), "expected FontSize default row: {h}");
    }

    #[test]
    fn builtin_event_hover_uses_arrow_capture_form() {
        // Hovering a built-in event name shows the call + capture signature
        // `on CharacterSpawned() -> (character: character)`, NOT the old inline
        // `on CharacterSpawned(character: character)` form.
        let src = "on CharacterSpawned() -> (ch) {\n}\n";
        let col = src.lines().next().unwrap().find("CharacterSpawned").unwrap();
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
        assert!(h.contains("isObject"), "config arg shown display-cased: {h}");
        assert!(!h.contains("isobject"), "config arg not lowercased: {h}");
        assert!(
            !h.contains("EventName"),
            "channel shown once as \"name\", not also as EventName: {h}"
        );
        assert!(h.contains("on CustomEvent(\"name\""), "channel placeholder: {h}");
        assert!(h.contains("data1: any"), "untyped data slots: {h}");
    }

    #[test]
    fn named_param_hover_shows_default_value() {
        // Hovering a named arg (`fontSize = …`) shows that param's default.
        let src = "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\", fontSize = 20)\n}";
        let call_line = 3usize;
        let col = src.lines().nth(call_line).unwrap().find("fontSize").unwrap();
        let h = hover_for(src, call_line, col).expect("fontSize named arg should hover");
        assert!(h.contains("Default: `"), "expected a default line: {h}");
    }

    #[test]
    fn builtin_hover_shows_layout_and_color_defaults() {
        // DisplayText's layout ports are per-axis Vector2D sub-ports; the defaults
        // table lists them as float rows, and the colors still render as sRGB hex
        // (from their LinearColor defaults).
        let src = "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\")\n}";
        let call_line = 3usize;
        let col = src.lines().nth(call_line).unwrap().find("DisplayText").unwrap();
        let h = hover_for(src, call_line, col).expect("DisplayText should hover");
        assert!(h.contains("anchorY"), "per-axis layout param row: {h}");
        assert!(h.contains("pivotX"), "per-axis layout param row: {h}");
        assert!(h.contains("#181425"), "outline color as sRGB hex: {h}");
        assert!(h.contains("#ffffff"), "font color white: {h}");
    }

    #[test]
    fn named_param_hover_shows_subport_axis_default() {
        // Hovering the `anchorY =` named arg shows that axis of the parent Anchor
        // Vector2D default (0.5). A real param name never trips the WS041 check.
        let src = "in c: controller\nin go: exec\non go {\n  DisplayText(c, \"hi\", anchorY = 0.9)\n}";
        let call_line = 3usize;
        let col = src.lines().nth(call_line).unwrap().find("anchorY").unwrap();
        let h = hover_for(src, call_line, col).expect("anchorY named arg should hover");
        assert!(h.contains("Default: `0.5`"), "expected Anchor.Y default 0.5: {h}");
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
        assert!(h.contains("on CustomEvent(\"dmg\""), "channel name in sig: {h}");
        assert!(h.contains("amount: int"), "declared slot type: {h}");
        assert!(h.contains("attacker: character"), "sender-filled slot type: {h}");
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
        assert!(h.contains("character[]"), "hover should show the in-scope array type: {h}");
        assert!(!h.contains("string"), "must not show the shadowed string type: {h}");
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
        assert!(h.contains("SendCustomEvent(\"dmg\""), "channel in send hover: {h}");
        assert!(h.contains("amount: int"), "receiver-declared type shown: {h}");
        assert!(h.contains("attacker: character"), "second declared type shown: {h}");
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
        let resolved = resolve(src, "main", &loader);
        let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main"));
        let estimates =
            crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, "main");
        let line1 = src.lines().nth(1).unwrap();
        let col = line1.find("drawCard").unwrap();
        let text = hover_at(
            src,
            "main",
            &symbols,
            &tc.type_of_expr,
            &resolved.doc_comments,
            &tc.if_contexts,
            &tc.var_read_contexts,
            &tc.dropped_ranges,
            &estimates,
            1,
            col,
        )
        .expect("hover on a namespace member should return something");
        assert!(text.contains("drawCard"), "should name the member, got: {text}");
        assert!(text.contains("n: int"), "should show the signature, got: {text}");
        assert!(!text.contains("unknown"), "must not show `unknown`, got: {text}");
    }

    #[test]
    fn generic_mod_hover_shows_type_params() {
        // Hovering a generic mod's name shows its `<T: Numeric>` generics, not
        // just `(v: T) -> T`.
        let src = "mod square<T: Numeric>(v: T) -> T { return v * v }\n";
        let col = src.find("square").unwrap() + 1;
        let h = hover_for(src, 0, col).expect("hover on generic mod");
        assert!(h.contains("<T: Numeric>"), "hover should show generics: {h}");
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
        assert!(h.contains("square<int>"), "angle brackets resolved to int: {h}");
        assert!(h.contains("v: int"), "param type substituted: {h}");
        assert!(h.contains("-> int"), "return type substituted: {h}");
        assert!(!h.contains("<T:"), "must not show the generic form at a call: {h}");
        assert!(h.contains("`T` = `int`"), "resolution note present: {h}");
    }

    #[test]
    fn generic_call_hover_uses_explicit_type_args() {
        // An explicit `f<float>(...)` pins the type param even when the argument
        // would infer something narrower.
        let src = "mod id<T>(v: T) -> T { return v }\n\
                   on ReadBrickGrid() {\n  let n = id<float>(1.0)\n}\n";
        let call_line = 2usize;
        let col = src.lines().nth(call_line).unwrap().find("id<float>").unwrap() + 1;
        let h = hover_for(src, call_line, col).expect("hover on explicit generic call");
        assert!(h.contains("id<float>"), "explicit type arg shown: {h}");
        assert!(h.contains("v: float"), "param substituted from explicit arg: {h}");
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
        assert!(h.contains("want: string") && h.contains("got: string"), "params substituted: {h}");
        assert!(h.contains("label: string"), "non-generic param unchanged: {h}");
    }

    #[test]
    fn constraint_class_hovers() {
        // Hovering the bound `Numeric` shows the class + its members.
        let src = "mod square<T: Numeric>(v: T) -> T { return v * v }\n";
        let col = src.find("Numeric").unwrap() + 1;
        let h = hover_for(src, 0, col).expect("hover on Numeric");
        assert!(h.contains("constraint class"), "Numeric hover: {h}");
        assert!(h.contains("vector"), "Numeric should list members incl vector: {h}");
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
        assert!(h.contains("vector"), "bounded T hover should expand class members: {h}");
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
        assert!(h.contains("int") && h.contains("float"), "Scalar should expand to int/float: {h}");
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
        let resolved = resolve(src, "main", &loader);
        let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main"));
        let estimates =
            crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, "main");
        let col = src.find("card").unwrap();
        let text = hover_at(
            src,
            "main",
            &symbols,
            &tc.type_of_expr,
            &resolved.doc_comments,
            &tc.if_contexts,
            &tc.var_read_contexts,
            &tc.dropped_ranges,
            &estimates,
            0,
            col,
        )
        .expect("hover on a namespace alias should return something");
        assert!(text.contains("namespace card"), "should show namespace, got: {text}");
        assert!(!text.contains("unknown"), "must not show `unknown`, got: {text}");
        assert!(text.contains("drawCard"), "should list members, got: {text}");
    }

    #[test]
    fn destructured_mod_param_shows_type() {
        let src = "\
type State = { counter: *int, step: int }
mod bump({ counter, step }: State) { counter = counter + step }";
        // "counter" starts at col 11, "step" at col 20 on line 1
        let h = hover_for(src, 1, 11);
        assert!(h.is_some(), "hover on destructured param 'counter' should return something");
        let text = h.unwrap();
        assert!(
            text.contains("*int"),
            "hover should show *int for counter, got: {text}"
        );

        let h2 = hover_for(src, 1, 20);
        assert!(h2.is_some(), "hover on destructured param 'step' should return something");
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
    fn user_var_named_like_array_method_hovers_as_var() {
        // A variable named after an array method (`sum`) must hover as the
        // variable, not as `array.sum`. The array-method hover only applies to a
        // `.method` access, not a bare identifier that happens to share the name.
        let src = "var sum: int = 0";
        // "sum" occupies cols 4..=6
        let h = hover_for(src, 0, 5).expect("hover on var 'sum' should return something");
        assert!(
            !h.contains("array.sum"),
            "hover on the variable `sum` must not show the array method, got: {h}"
        );
        assert!(
            h.contains("var sum"),
            "hover on the variable `sum` should show the variable declaration, got: {h}"
        );
    }

    #[test]
    fn user_var_named_like_builtin_method_hovers_as_var() {
        // `Teleport` is a builtin receiver-method; a variable sharing that name
        // must hover as the variable, not the method. Method/call hovers only
        // fire in actual call/method position.
        let src = "var Teleport: int = 0";
        // "Teleport" occupies cols 4..=11
        let h = hover_for(src, 0, 6).expect("hover on var 'Teleport' should return something");
        assert!(
            h.contains("var Teleport"),
            "hover on the variable `Teleport` should show the variable, got: {h}"
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
        assert!(hg.contains("Map<"), "map .get shows the concrete map type: {hg}");

        let hh = hover_for(src, 4, col_in(4, ".has")).expect("map .has should hover");
        assert!(hh.contains("map.has"), "map .has hover: {hh}");

        let hc = hover_for(src, 5, col_in(5, ".clear")).expect("map .clear should hover");
        assert!(hc.contains("map.clear"), "shared name resolves to map: {hc}");
        assert!(!hc.contains("array.clear"), "must not be the array hover: {hc}");
    }

    #[test]
    fn map_method_via_type_alias_hovers_as_map() {
        // A receiver whose declared type is a type ALIAS of a map (`type Scores =
        // Map<...>`) must still dispatch to the map table — the symbol carries the
        // alias name, so resolution has to see through it.
        let src = "\
type Scores = Map<string, int>
var s: Scores
in t: exec
on t { let a = s.get(\"x\") }";
        let l = src.lines().nth(3).unwrap();
        let col = l.find(".get").unwrap() + 2;
        let h = hover_for(src, 3, col);
        assert!(
            h.as_deref().is_some_and(|h| h.contains("map.get")),
            "aliased map `.get` should hover as a map method, got: {h:?}"
        );
    }

    #[test]
    fn map_method_via_generic_alias_instance_hovers_as_map() {
        // A generic type alias instantiated to a map (`type Grid<T> = Map<string,
        // T>`; `var g: Grid<int>`) resolves by its base name, so `.get` dispatches
        // to the map table and the hover shows the declared type `Grid<int>`.
        let src = "\
type Grid<T> = Map<string, T>
var g: Grid<int>
in t: exec
on t { let a = g.get(\"x\") }";
        let l = src.lines().nth(3).unwrap();
        let col = l.find(".get").unwrap() + 2;
        let h = hover_for(src, 3, col).expect("generic-aliased map `.get` should hover");
        assert!(h.contains("map.get"), "should hover as a map method: {h}");
        assert!(h.contains("Grid<int>"), "should show the declared type: {h}");
    }

    #[test]
    fn record_field_hover() {
        let src = "\
type Point = { x: int, y: int }
let p: Point = { x: 1, y: 2 }
let v = p.x";
        // hover on "x" in "p.x" (line 2, col 10)
        let h = hover_for(src, 2, 10);
        assert!(h.is_some(), "hover on record field 'x' should return something");
        let text = h.unwrap();
        assert!(
            text.contains("int"),
            "hover on p.x should show int, got: {text}"
        );
    }

    #[test]
    fn prefab_file_reference_hover() {
        // `$` at col 8; token spans cols 8..=25.
        let src = "let p = $./prefab_1x1.brz";
        for col in [8usize, 12, 24] {
            let h = hover_for(src, 0, col);
            assert!(h.is_some(), "hover on prefab ref at col {col} should return");
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
        assert!(h.contains("bool"), "destructured Found should be bool, got: {h}");
        let h2 = hover_for(src, 2, line2.find("Index").unwrap()).expect("hover on Index");
        assert!(h2.contains("int"), "destructured Index should be int, got: {h2}");
        // Usages resolve through the same symbol.
        let line3 = src.lines().nth(3).unwrap();
        let h3 = hover_for(src, 3, line3.find("Found").unwrap()).expect("hover on Found use");
        assert!(h3.contains("bool"), "Found usage should be bool, got: {h3}");
    }

    #[test]
    fn enum_config_param_hovers_with_enum_name_and_members() {
        // Hovering the config arg name `blendSpace` names its schema enum
        // (EBRColorSpace) instead of `int`, and lists the member names.
        let src = "in a: color\nin b: color\nin t: float\nlet c = ColorBlend(a, b, t, blendSpace = Oklab)";
        let line = src.lines().nth(3).unwrap();
        let h = hover_for(src, 3, line.find("blendSpace").unwrap())
            .expect("hover on blendSpace");
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
        let h = hover_for(src, 2, line.find("X_Negative").unwrap())
            .expect("hover on X_Negative");
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
        // Hovering an event config arg name (`pulseOn`) identifies it as Clock
        // config and names the gate field it sets (`enabled` is a wire input,
        // not config).
        let src = "static var n: int = 0\non Clock(interval = 2.0, pulseOn = false) {\n  n = n + 1\n}";
        let line = src.lines().nth(1).unwrap();
        let h = hover_for(src, 1, line.find("pulseOn").unwrap()).expect("hover on pulseOn");
        assert!(h.contains("Clock"), "should mention the event: {h}");
        assert!(h.contains("bPulseOn"), "should name the gate field: {h}");
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
        assert!(h.contains("```wirescript"), "must be a fenced (coloured) block: {h}");
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

    // ---------- enum hover: user enums ----------

    const SHAPE_ENUM_SRC: &str = "enum Shape { Empty, Circle(float), Rect(float, float) }\nvar s: Shape = Shape.Empty\nin go: exec\non go {\n  let d = s.Discriminant\n  let c = Shape.Circle\n  let cd = Shape.Circle.Discriminant\n}\n";

    #[test]
    fn hover_user_enum_type_name_in_var_decl_shows_enum_and_variants() {
        // Hovering the TYPE name in `var s: Shape` shows it is an
        // enum and lists its variants (with payload shapes).
        let l = SHAPE_ENUM_SRC.lines().nth(1).unwrap();
        let col = l.find("Shape").unwrap() + 1;
        let h = hover_for(SHAPE_ENUM_SRC, 1, col).expect("enum type name should hover");
        assert!(h.contains("enum Shape"), "should say it's an enum: {h}");
        assert!(h.contains("Empty"), "should list Empty: {h}");
        assert!(h.contains("Circle(float)"), "should show Circle's payload shape: {h}");
        assert!(h.contains("Rect(float, float)"), "should show Rect's payload shape: {h}");
    }

    #[test]
    fn hover_user_enum_type_name_in_construction_path_shows_enum_and_variants() {
        // Hovering the `Shape` half of `Shape.Circle` also resolves
        // to the enum (not the variant - that's a separate word/case).
        let l = SHAPE_ENUM_SRC.lines().nth(5).unwrap();
        let col = l.find("Shape").unwrap() + 1;
        let h = hover_for(SHAPE_ENUM_SRC, 5, col).expect("enum type name in ctor path should hover");
        assert!(h.contains("enum Shape"), "should say it's an enum: {h}");
        assert!(h.contains("Empty") && h.contains("Circle") && h.contains("Rect"), "should list all variants: {h}");
    }

    #[test]
    fn hover_user_enum_variant_in_construction_path_shows_discriminant() {
        // Hovering the `Circle` in `Shape.Circle` shows
        // the variant + its discriminant (Empty=0, Circle=1 by declaration order).
        let l = SHAPE_ENUM_SRC.lines().nth(5).unwrap();
        let col = l.find("Circle").unwrap() + 1;
        let h = hover_for(SHAPE_ENUM_SRC, 5, col).expect("variant should hover");
        assert!(h.contains("Shape.Circle"), "should name the owning enum + variant: {h}");
        assert!(h.contains("(float)"), "should show the payload shape: {h}");
        assert!(h.contains("Discriminant") && h.contains('1'), "should show discriminant 1: {h}");
    }

    #[test]
    fn hover_enum_value_discriminant_yields_int() {
        // `s.Discriminant` where `s: Shape` already
        // resolves through the generic field-type hover (`type_map` types
        // `.Discriminant` as `Type::Int` regardless of the object), which is
        // useful as-is - it says the projection yields `int`.
        let l = SHAPE_ENUM_SRC.lines().nth(4).unwrap();
        let col = l.find("Discriminant").unwrap() + 1;
        let h = hover_for(SHAPE_ENUM_SRC, 4, col).expect(".Discriminant on an enum value should hover");
        assert!(h.contains("Discriminant") && h.contains("int"), "should show it yields int: {h}");
    }

    #[test]
    fn hover_enum_variant_path_discriminant_shows_compile_time_constant() {
        // `Shape.Circle.Discriminant` is a compile-time
        // CONSTANT equal to Circle's discriminant (1), not just `int`.
        let l = SHAPE_ENUM_SRC.lines().nth(6).unwrap();
        let col = l.find("Discriminant").unwrap() + 1;
        let h = hover_for(SHAPE_ENUM_SRC, 6, col).expect("variant-path .Discriminant should hover");
        assert!(h.contains("Shape.Circle.Discriminant"), "should name the full path: {h}");
        assert!(h.contains("= 1"), "should show the constant value 1: {h}");
        assert!(h.to_lowercase().contains("compile-time constant"), "should say it's a compile-time constant: {h}");
    }

    #[test]
    fn hover_enum_typed_variable_shows_its_type() {
        // Hovering an enum-typed var already shows
        // its declared type via the ordinary var hover.
        let l = SHAPE_ENUM_SRC.lines().nth(1).unwrap();
        let col = l.find(" s:").unwrap() + 1;
        let h = hover_for(SHAPE_ENUM_SRC, 1, col).expect("var s should hover");
        assert!(h.contains("var s: Shape"), "should show the enum type: {h}");
    }

    // ---------- enum hover: named payload field keys ----------

    const BOX_ENUM_SRC: &str = "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w: 1.0, h: 2.0 }\n";

    #[test]
    fn hover_construction_field_key_shows_declared_type() {
        // Hovering `w` in `Shape.Box { w: 1.0, h: 2.0 }` shows its declared
        // type from `enum Shape { Box { w: float, h: float } }`.
        let l = BOX_ENUM_SRC.lines().nth(1).unwrap();
        let col = l.find("w:").unwrap() + 1;
        let h = hover_for(BOX_ENUM_SRC, 1, col).expect("construction field key should hover");
        assert!(h.contains("w: float"), "should show the field's declared type: {h}");
        assert!(h.contains("Shape.Box"), "should name the owning enum + variant: {h}");
    }

    #[test]
    fn hover_construction_second_field_key_shows_its_own_type() {
        let l = BOX_ENUM_SRC.lines().nth(1).unwrap();
        let col = l.find("h:").unwrap() + 1;
        let h = hover_for(BOX_ENUM_SRC, 1, col).expect("second field key should hover");
        assert!(h.contains("h: float"), "should show h's own declared type: {h}");
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

    // ---------- enum hover: built-in game enums ----------

    const EASING_SRC: &str = "var e: EasingFunction = EasingFunction.Bounce\nin go: exec\non go {\n  let d = e.Discriminant\n  let cd = EasingFunction.Bounce.Discriminant\n}\n";

    #[test]
    fn hover_builtin_enum_type_name_notes_built_in_game_enum() {
        // Hovering `EasingFunction` says it's a built-in
        // game enum and lists its variants.
        let l = EASING_SRC.lines().next().unwrap();
        let col = l.find("EasingFunction").unwrap() + 1;
        let h = hover_for(EASING_SRC, 0, col).expect("built-in enum type name should hover");
        assert!(h.contains("enum EasingFunction"), "should say it's an enum: {h}");
        assert!(h.to_lowercase().contains("built-in game enum"), "should note it's built-in: {h}");
        assert!(h.contains("Bounce"), "should list Bounce among the variants: {h}");
    }

    #[test]
    fn hover_builtin_enum_type_name_in_construction_path() {
        // The `EasingFunction` half of
        // `EasingFunction.Bounce` also resolves to the enum.
        let l = EASING_SRC.lines().next().unwrap();
        let col = l.rfind("EasingFunction").unwrap() + 1;
        let h = hover_for(EASING_SRC, 0, col).expect("built-in enum type name in ctor path should hover");
        assert!(h.contains("enum EasingFunction"), "should say it's an enum: {h}");
    }

    #[test]
    fn hover_builtin_enum_variant_shows_real_schema_discriminant() {
        // Hovering `Bounce` in `EasingFunction.Bounce`
        // shows the REAL schema discriminant from the catalog, not an
        // auto-numbered index.
        let want = crate::catalog::enum_member_value("EBREasingFunction", "Bounce")
            .expect("Bounce is a real EBREasingFunction member");
        let l = EASING_SRC.lines().next().unwrap();
        let col = l.rfind("Bounce").unwrap() + 1;
        let h = hover_for(EASING_SRC, 0, col).expect("built-in variant should hover");
        assert!(h.contains("EasingFunction.Bounce"), "should name the owning enum + variant: {h}");
        assert!(
            h.contains(&format!("`{want}`")) || h.contains(&format!("{want}")),
            "should show the real schema discriminant {want}: {h}"
        );
    }

    #[test]
    fn hover_builtin_enum_variant_path_discriminant_shows_real_constant() {
        // `EasingFunction.Bounce.Discriminant`
        // shows the real schema constant, matching `enum_member_value`.
        let want = crate::catalog::enum_member_value("EBREasingFunction", "Bounce")
            .expect("Bounce is a real EBREasingFunction member");
        let l = EASING_SRC.lines().nth(4).unwrap();
        let col = l.find("Discriminant").unwrap() + 1;
        let h = hover_for(EASING_SRC, 4, col).expect("built-in variant-path .Discriminant should hover");
        assert!(h.contains("EasingFunction.Bounce.Discriminant"), "should name the full path: {h}");
        assert!(h.contains(&format!("= {want}")), "should show the real constant {want}: {h}");
    }

    #[test]
    fn hover_builtin_enum_typed_variable_shows_its_type() {
        // Hovering an enum-typed var
        // already shows its declared type via the ordinary var hover.
        let l = EASING_SRC.lines().next().unwrap();
        let col = l.find(" e:").unwrap() + 1;
        let h = hover_for(EASING_SRC, 0, col).expect("var e should hover");
        assert!(h.contains("var e: EasingFunction"), "should show the enum type: {h}");
    }

    #[test]
    fn hover_builtin_enum_value_discriminant_yields_int() {
        // `e.Discriminant` where `e:
        // EasingFunction` already resolves through the generic field-type
        // hover, showing it yields `int`.
        let l = EASING_SRC.lines().nth(3).unwrap();
        let col = l.find("Discriminant").unwrap() + 1;
        let h = hover_for(EASING_SRC, 3, col).expect(".Discriminant on a built-in enum value should hover");
        assert!(h.contains("Discriminant") && h.contains("int"), "should show it yields int: {h}");
    }
