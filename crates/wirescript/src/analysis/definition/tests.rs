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
