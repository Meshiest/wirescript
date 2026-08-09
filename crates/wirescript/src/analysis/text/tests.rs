    use super::*;

    #[test]
    fn word_at_survives_multibyte_neighbors() {
        // Regression: the word boundary search used `rfind(...) + 1`, which
        // lands INSIDE a multi-byte char (`─` is 3 bytes) and panicked the
        // whole LSP on hover over comments like `// ── Tunables ──…`.
        let l = "// ── Tunables ──";
        assert_eq!(word_at(l, 0, 6).as_deref(), Some("Tunables"));
        // Cursor ON the box-drawing char: no word, no panic.
        assert_eq!(word_at(l, 0, 3), None);
        // Same +1 pattern in the other line scanners.
        assert_eq!(
            find_enclosing_call("let x = ─f(a, ", 0, 13).as_deref(),
            Some("f")
        );
        assert!(named_arg_value("  ─x = ", 0, 7).is_some());
        assert_eq!(member_receiver_at("─a.b", 0, 4).as_deref(), Some("a"));
    }

    #[test]
    fn named_arg_value_detects_value_slot() {
        // In the value of `justify = ...`.
        let (n, v) = named_arg_value("  justify = ", 0, 12).unwrap();
        assert_eq!(n, "justify");
        assert!(!v.contains('"'));
        // Inside an opened quote.
        let (n2, v2) = named_arg_value("  justify = \"Le", 0, 15).unwrap();
        assert_eq!(n2, "justify");
        assert!(v2.contains('"'));
        // Not a value slot (fresh arg / no '=').
        assert_eq!(named_arg_value("  fontSize", 0, 10), None);
        // `==` is not a named arg.
        assert_eq!(named_arg_value("if a == ", 0, 8), None);
    }

    #[test]
    fn finds_prefab_and_asset_refs() {
        let src = "let a = $./p.brz\nlet b = SpawnPrefab(prefab = $/abs/x.brz)\nlet c = $Weapon/Sword";
        let refs = find_asset_refs(src);
        assert_eq!(refs.len(), 3);
        assert!(refs[0].is_file() && refs[0].path == "./p.brz" && refs[0].line == 0);
        assert!(refs[1].is_file() && refs[1].path == "/abs/x.brz");
        assert!(!refs[2].is_file() && refs[2].path == "Weapon/Sword");
        // start_col is the '$' column.
        assert_eq!(refs[0].start_col, 8);
    }

    #[test]
    fn skips_refs_in_strings_and_comments() {
        // `$./x.brz` inside a string, a line comment, and a `${}` interpolation
        // must NOT be reported; the real ref on the last line must be.
        let src = "let s = \"visit $./page.brz now\"\n// see $./notes.brz\nlet t = \"${x}\"\nlet r = $./real.brz";
        let refs = find_asset_refs(src);
        assert_eq!(refs.len(), 1, "got {refs:?}");
        assert_eq!(refs[0].path, "./real.brz");
        assert_eq!(refs[0].line, 3);
    }

    #[test]
    fn enclosing_call_single_line() {
        // Cursor inside `f(a, |)` resolves to `f`.
        let src = "let x = f(a, b)";
        assert_eq!(find_enclosing_call(src, 0, 13).as_deref(), Some("f"));
        // Receiver call: `.`-qualified name resolves to the method name.
        let src2 = "on t { ctrl.DisplayText(\"hi\", fontSize = 20) }";
        assert_eq!(find_enclosing_call(src2, 0, 40).as_deref(), Some("DisplayText"));
        // Outside any call → None.
        assert_eq!(find_enclosing_call("let x = 1", 0, 8), None);
    }

    #[test]
    fn enclosing_call_multiline() {
        // A call spread across lines: the cursor on a continuation line must
        // still resolve to the call whose `(` opened lines above.
        let src = "on t {\n\
                   ctrl.DisplayText(\"hi\",\n\
                   fontSize = 20,\n\
                   outlineSize = 0,\n\
                   )\n\
                   }";
        // line 3 (`outlineSize = 0,`), cursor at end of the name.
        assert_eq!(find_enclosing_call(src, 3, 11).as_deref(), Some("DisplayText"));
        // A `(` inside a string on an earlier arg line must not break the count.
        let src2 = "f(\n\"text with ( paren\",\ng = 1\n)";
        assert_eq!(find_enclosing_call(src2, 2, 3).as_deref(), Some("f"));
    }

    #[test]
    fn asset_ref_at_pinpoints_cursor() {
        let src = "let a = $./p.brz";
        assert!(asset_ref_at(src, 0, 8).is_some()); // on '$'
        assert!(asset_ref_at(src, 0, 12).is_some()); // inside path
        assert!(asset_ref_at(src, 0, 3).is_none()); // on 'a'
    }
