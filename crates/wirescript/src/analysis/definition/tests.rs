    use super::*;
    use crate::analysis::symbols::collect_symbols_for_file;
    use crate::resolve::{MemLoader, resolve};

    fn goto(main: &str, display: &str, line: usize, col: usize) -> Option<Location> {
        let mut files = crate::collections::HashMap::default();
        files.insert("display.ws".to_string(), display.to_string());
        let loader = MemLoader { files };
        let pre = crate::parse(main, "main.ws");
        let resolved = resolve(main, "main.ws", &loader);
        let tc = crate::typecheck::typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main.ws"));
        definition_at(main, &pre.ast, &symbols, "main.ws", &loader, line, col)
    }

    #[test]
    fn self_receiver_method_goes_to_mod_decl() {
        // Go-to-definition on the `.dist` of `a.dist(b)` jumps to the user
        // `self`-mod declaration (the bare-word symbol fallback resolves it).
        let main = "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
                    in a: vector\nin b: vector\nin go: exec\non go { let d = a.dist(b) }\n";
        let call_line = 4;
        let l = main.lines().nth(call_line).unwrap();
        let col = l.find(".dist").unwrap() + 2; // inside `dist`
        let loc = goto(main, "", call_line, col).expect("goto on .dist should resolve");
        assert_eq!(loc.file, None, "same-file mod: {loc:?}");
        assert_eq!(loc.start_line, 0, "mod dist is declared on line 0, got {loc:?}");
    }

    #[test]
    fn namespaced_call_goes_to_imported_decl_not_local_shadow() {
        // `card.drawCard` must jump to display.ws's drawCard even though a
        // local `mod drawCard` shares the name.
        let display = "mod drawCard(x: int) {\n  let unused = x\n}\n";
        let main = "import * as card from \"display\"\n\nmod drawCard(y: int) {\n  card.drawCard(y)\n}\n";
        let call_line = 3;
        let col = main.lines().nth(call_line).unwrap().find(".drawCard").unwrap() + 2;
        let loc = goto(main, display, call_line, col).expect("definition should resolve");
        assert_eq!(
            loc.file.as_deref(),
            Some("display.ws"),
            "qualified name must resolve in the imported file, got {loc:?}"
        );
        assert_eq!(loc.start_line, 0, "display.ws drawCard is on its line 0");
    }

    #[test]
    fn multiline_named_import_resolves_the_clicked_specifier() {
        // Regression: go-to-definition on a named-import specifier ignored the
        // cursor column and returned the FIRST binding that resolved via
        // `top_decl_name`. With `var` specifiers before `mod` ones, clicking
        // `onCheckpoint` jumped to the earlier `doReset` instead.
        let display = "var cfg: string = \"\"\nmod doReset() { }\nmod onCheckpoint() { }\n";
        let main = "import { cfg, doReset,\n  onCheckpoint } from \"display\"\n";
        let click_line = 1; // the continuation line, where doReset used to win
        let l = main.lines().nth(click_line).unwrap();
        let col = l.find("onCheckpoint").unwrap() + 2; // inside `onCheckpoint`
        let loc = goto(main, display, click_line, col).expect("goto on import specifier");
        assert_eq!(loc.file.as_deref(), Some("display.ws"));
        assert_eq!(
            loc.start_line, 2,
            "onCheckpoint is on display.ws line 2 (0-based); a jump to line 1 is doReset: {loc:?}"
        );
    }

    #[test]
    fn named_import_resolves_a_var_specifier() {
        // `top_decl_name` used to cover only mod/chip/let/event, so clicking an
        // imported `var` specifier fell through to the file top. It now resolves
        // to the var's own decl.
        let display = "var cfg: string = \"\"\nmod doReset() { }\n";
        let main = "import { cfg, doReset } from \"display\"\n";
        let col = main.lines().next().unwrap().find("cfg").unwrap() + 1;
        let loc = goto(main, display, 0, col).expect("goto on the `cfg` specifier");
        assert_eq!(loc.file.as_deref(), Some("display.ws"));
        assert_eq!(loc.start_line, 0, "cfg is declared on display.ws line 0, got {loc:?}");
    }

    #[test]
    fn send_custom_event_name_jumps_to_receiver() {
        // Cursor on the channel-name string of a `SendCustomEvent(...)` jumps to
        // the matching `on CustomEvent(...)` receiver in the same file.
        let main = "on CustomEvent(\"dmg\") -> (amount: int) {\n  let x = amount\n}\nin go: exec\non go {\n  SendCustomEvent(\"dmg\", 5)\n}\n";
        let send_line = 5; // the SendCustomEvent line (0-based)
        let line_str = main.lines().nth(send_line).unwrap();
        let col = line_str.find("dmg").unwrap() + 1; // inside the "dmg" string
        let loc = goto(main, "", send_line, col).expect("should jump to the receiver");
        assert_eq!(loc.file, None, "receiver is in the same file: {loc:?}");
        assert_eq!(
            loc.start_line, 0,
            "receiver `on CustomEvent(\"dmg\") -> (…)` is on line 0, got {loc:?}"
        );
    }

    #[test]
    fn send_global_custom_event_jumps_to_global_receiver_not_personal() {
        // A `SendGlobalCustomEvent(...)` channel jumps to `on GlobalCustomEvent`
        // (line 0), NOT the same-named personal `on CustomEvent` (line 2) — they
        // are separate namespaces.
        let main = "on GlobalCustomEvent(\"score\") -> (pts: int) {\n}\non CustomEvent(\"score\") -> (x: int) {\n}\nin go: exec\non go {\n  SendGlobalCustomEvent(\"score\", 5)\n}\n";
        let send_line = 6; // the SendGlobalCustomEvent line (0-based)
        let line_str = main.lines().nth(send_line).unwrap();
        let col = line_str.find("score").unwrap() + 1;
        let loc = goto(main, "", send_line, col).expect("should jump to the global receiver");
        assert_eq!(
            loc.start_line, 0,
            "SendGlobalCustomEvent should jump to `on GlobalCustomEvent` (line 0), not personal (line 2): {loc:?}"
        );
    }

    #[test]
    fn send_custom_event_name_without_receiver_does_not_jump() {
        // No matching receiver — the send-site string yields no navigation.
        let main = "in go: exec\non go {\n  SendCustomEvent(\"nope\", 5)\n}\n";
        let send_line = 2;
        let line_str = main.lines().nth(send_line).unwrap();
        let col = line_str.find("nope").unwrap() + 1;
        assert!(
            goto(main, "", send_line, col).is_none(),
            "unmatched channel name should not navigate"
        );
    }

    #[test]
    fn unqualified_call_still_goes_to_local_decl() {
        // Bare `drawCard(...)` keeps resolving to the local mod.
        let display = "mod drawCard(x: int) {\n  let unused = x\n}\n";
        let main = "import * as card from \"display\"\n\nmod drawCard(y: int) {\n  let z = y\n}\n\nmod use1(w: int) {\n  drawCard(w)\n}\n";
        let call_line = 7;
        let col = main.lines().nth(call_line).unwrap().find("drawCard").unwrap() + 1;
        let loc = goto(main, display, call_line, col).expect("definition should resolve");
        assert_eq!(loc.file, None, "bare name resolves to the local decl, got {loc:?}");
        assert_eq!(loc.start_line, 2, "local drawCard is on line 2");
    }

    // ---------- enum goto ----------

    const SHAPE_ENUM_MAIN: &str = "enum Shape { Empty, Circle(float), Rect(float, float) }\nvar s: Shape = Shape.Empty\nin go: exec\non go {\n  let c = Shape.Circle\n}\n";

    #[test]
    fn goto_enum_type_name_jumps_to_enum_decl() {
        // `Shape` in `var s: Shape` jumps to the `enum Shape` decl
        // (line 0), landing on the `Shape` name token itself.
        let l = SHAPE_ENUM_MAIN.lines().nth(1).unwrap();
        let col = l.find("Shape").unwrap() + 1;
        let loc = goto(SHAPE_ENUM_MAIN, "", 1, col).expect("goto on the enum type name should resolve");
        assert_eq!(loc.file, None, "same-file enum: {loc:?}");
        assert_eq!(loc.start_line, 0, "enum Shape is declared on line 0, got {loc:?}");
        let decl_line = SHAPE_ENUM_MAIN.lines().next().unwrap();
        let name_col = decl_line.find("Shape").unwrap();
        assert_eq!(loc.start_col, name_col, "should land on the `Shape` name token, got {loc:?}");
    }

    #[test]
    fn goto_enum_variant_use_jumps_to_variant_token() {
        // `Circle` in `Shape.Circle` jumps to that variant's own
        // token inside the `enum Shape` declaration (not just the enum).
        let l = SHAPE_ENUM_MAIN.lines().nth(4).unwrap();
        let col = l.find("Circle").unwrap() + 1;
        let loc = goto(SHAPE_ENUM_MAIN, "", 4, col).expect("goto on the variant use should resolve");
        assert_eq!(loc.file, None, "same-file enum: {loc:?}");
        assert_eq!(loc.start_line, 0, "the variant is declared on line 0 (same line as `enum Shape`), got {loc:?}");
        let decl_line = SHAPE_ENUM_MAIN.lines().next().unwrap();
        // Confirm the location lands on `Circle`'s own token in the decl line,
        // not just somewhere on the (correct) line.
        let variant_col = decl_line.find("Circle").unwrap();
        assert_eq!(loc.start_col, variant_col, "should land on the `Circle` variant token, got {loc:?}");
    }

    #[test]
    fn goto_builtin_enum_type_name_returns_none_without_panicking() {
        // A built-in game enum has no source location - goto must
        // gracefully report nothing, not panic.
        let main = "var e: EasingFunction = EasingFunction.Bounce\n";
        let col = main.find("EasingFunction").unwrap() + 1;
        assert!(
            goto(main, "", 0, col).is_none(),
            "built-in enum type name has no source location"
        );
    }

    #[test]
    fn goto_builtin_enum_variant_returns_none_without_panicking() {
        // Same, for the variant half of a built-in
        // enum's construction path.
        let main = "var e: EasingFunction = EasingFunction.Bounce\n";
        let col = main.rfind("Bounce").unwrap() + 1;
        assert!(
            goto(main, "", 0, col).is_none(),
            "built-in enum variant has no source location"
        );
    }

    // ---------- named payload field goto ----------

    const BOX_ENUM_MAIN: &str = "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { w: 1.0, h: 2.0 }\n";

    #[test]
    fn goto_construction_field_key_jumps_to_field_decl() {
        // Cursor on `w` in `Shape.Box { w: 1.0, h: 2.0 }` jumps to the `w`
        // token in the `Box { w: float, h: float }` payload decl.
        let l = BOX_ENUM_MAIN.lines().nth(1).unwrap();
        let col = l.find("w:").unwrap() + 1; // inside `w`
        let loc = goto(BOX_ENUM_MAIN, "", 1, col).expect("goto on the construction field key should resolve");
        assert_eq!(loc.file, None, "same-file enum: {loc:?}");
        assert_eq!(loc.start_line, 0, "Box's payload is declared on line 0, got {loc:?}");
        let decl_line = BOX_ENUM_MAIN.lines().next().unwrap();
        let field_col = decl_line.find("w:").unwrap();
        assert_eq!(loc.start_col, field_col, "should land on the `w` field token, got {loc:?}");
    }

    #[test]
    fn goto_construction_field_key_second_field_jumps_to_its_own_decl() {
        // `h` (the second field) must resolve to its OWN token, not `w`'s.
        let l = BOX_ENUM_MAIN.lines().nth(1).unwrap();
        let col = l.find("h:").unwrap() + 1; // inside `h`
        let loc = goto(BOX_ENUM_MAIN, "", 1, col).expect("goto on the `h` field key should resolve");
        assert_eq!(loc.start_line, 0);
        let decl_line = BOX_ENUM_MAIN.lines().next().unwrap();
        let field_col = decl_line.find("h:").unwrap();
        assert_eq!(loc.start_col, field_col, "should land on the `h` field token, got {loc:?}");
    }

    #[test]
    fn goto_construction_field_value_does_not_resolve_to_field_decl() {
        // Cursor on the VALUE side (`1.0`) is not a field key - no jump.
        let l = BOX_ENUM_MAIN.lines().nth(1).unwrap();
        let col = l.find("1.0").unwrap() + 1;
        assert!(
            goto(BOX_ENUM_MAIN, "", 1, col).is_none(),
            "the field's value is not a key - should not resolve"
        );
    }

    #[test]
    fn goto_construction_nonexistent_field_returns_none() {
        // A field name not declared on the variant (a WS041-worthy typo)
        // must not resolve - and must not panic.
        let main = "enum Shape { Box { w: float, h: float } }\nout b = Shape.Box { q: 1.0 }\n";
        let l = main.lines().nth(1).unwrap();
        let col = l.find("q:").unwrap() + 1;
        assert!(
            goto(main, "", 1, col).is_none(),
            "an undeclared field name should not resolve"
        );
    }

    #[test]
    fn goto_pattern_shorthand_field_jumps_to_field_decl() {
        // Cursor on `w` in `match s { Box { w, h } => w, ... }` (the
        // shorthand capture) jumps to the `w` token in the payload decl.
        let main = "enum Shape { Box { w: float, h: float } }\n\
                    var s: Shape = Shape.Box { w: 1.0, h: 2.0 }\n\
                    out r = match s { Box { w, h } => w, _ => 0.0 }\n";
        let l = main.lines().nth(2).unwrap();
        let col = l.find("{ w, h }").unwrap() + 2; // inside the pattern's `w`
        let loc = goto(main, "", 2, col).expect("goto on the pattern field key should resolve");
        assert_eq!(loc.file, None, "same-file enum: {loc:?}");
        assert_eq!(loc.start_line, 0, "Box's payload is declared on line 0, got {loc:?}");
        let decl_line = main.lines().next().unwrap();
        let field_col = decl_line.find("w:").unwrap();
        assert_eq!(loc.start_col, field_col, "should land on the `w` field token, got {loc:?}");
    }

    #[test]
    fn goto_pattern_field_ambiguous_variant_name_returns_none() {
        // Two different enums both declare a `Box` variant with named
        // fields - the variant name alone doesn't uniquely resolve an enum
        // without the scrutinee's real type, so this must not guess.
        let main = "enum Shape { Box { w: float, h: float } }\n\
                    enum Other { Box { n: int } }\n\
                    var s: Shape = Shape.Box { w: 1.0, h: 2.0 }\n\
                    out r = match s { Box { w, h } => w, _ => 0.0 }\n";
        let l = main.lines().nth(3).unwrap();
        let col = l.find("{ w, h }").unwrap() + 2;
        assert!(
            goto(main, "", 3, col).is_none(),
            "an ambiguous variant name across two enums must not resolve"
        );
    }

    #[test]
    fn goto_pattern_binding_does_not_resolve_to_field_decl() {
        // The bound value `w` used in the arm BODY (not the pattern's own key)
        // is a separate feature (local-capture goto) this task doesn't cover -
        // must return None rather than panic or misresolve.
        let main = "enum Shape { Box { w: float, h: float } }\n\
                    var s: Shape = Shape.Box { w: 1.0, h: 2.0 }\n\
                    out r = match s { Box { w, h } => w, _ => 0.0 }\n";
        let l = main.lines().nth(2).unwrap();
        let col = l.rfind("=> w").unwrap() + 3; // the `w` in the arm body
        assert!(
            goto(main, "", 2, col).is_none(),
            "arm-body capture use is a different feature; must not panic or misresolve"
        );
    }

    #[test]
    fn goto_unit_variant_construction_does_not_panic() {
        // A unit variant has no payload fields at all - the construction/
        // pattern field resolvers must be a no-op here, not panic. `Empty` is
        // still handled by the existing variant-name resolver, so this jumps
        // to the variant token, not `None`.
        let l = SHAPE_ENUM_MAIN.lines().nth(1).unwrap();
        let col = l.find("Empty").unwrap() + 1;
        let loc = goto(SHAPE_ENUM_MAIN, "", 1, col).expect("unit variant name still resolves via the existing resolver");
        assert_eq!(loc.start_line, 0);
    }
