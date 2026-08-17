    use super::*;

    fn tok_kinds(src: &str) -> Vec<TokenKind> {
        let r = lex(src, "test");
        assert!(r.diagnostics.is_empty(), "unexpected diags: {:?}", r.diagnostics);
        r.tokens.iter().map(|t| t.kind).collect()
    }

    #[test]
    fn empty_source_is_just_eof() {
        assert_eq!(tok_kinds(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn multibyte_chars_do_not_panic() {
        // A stray multi-byte char (outside a string) must not panic the
        // byte-slicing punct reader — it errors gracefully. Multi-byte chars
        // inside strings lex normally.
        let _ = lex("▲", "test"); // no panic
        let _ = lex("let x = ▲", "test"); // no panic
        let r = lex("\"▲ up ▼\"", "test");
        assert!(r.diagnostics.is_empty(), "string with multibyte: {:?}", r.diagnostics);
    }

    #[test]
    fn multibyte_string_value_roundtrips() {
        // The lexed literal must equal the source char-for-char; a multi-byte
        // char (e.g. `█` = E2 96 88) must NOT be split into three Latin-1 chars
        // (which would re-encode to garbage bytes on emit).
        for lit in ["█", "a█b", "▲ up ▼", "░▒▓█"] {
            let src = format!("\"{lit}\"");
            let r = lex(&src, "t");
            assert!(r.diagnostics.is_empty(), "{lit}: {:?}", r.diagnostics);
            match &r.tokens[0].value {
                Some(TokenValue::Str(s)) => assert_eq!(
                    s, lit,
                    "lexed value must match source exactly (bytes: {:?} vs {:?})",
                    s.as_bytes(),
                    lit.as_bytes()
                ),
                other => panic!("expected Str value, got {other:?}"),
            }
        }
    }

    #[test]
    fn var_decl_tokens() {
        use TokenKind::*;
        assert_eq!(
            tok_kinds("var x: int = 42"),
            vec![Kw, Ident, Colon, Ident, Op, Int, Eof]
        );
    }

    #[test]
    fn string_literal() {
        let r = lex(r#""hello""#, "t");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens[0].kind, TokenKind::Str);
        match &r.tokens[0].value {
            Some(TokenValue::Str(s)) => assert_eq!(s, "hello"),
            _ => panic!("expected Str value"),
        }
    }

    #[test]
    fn interpolated_string() {
        let r = lex(r#""hi ${name}""#, "t");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens[0].kind, TokenKind::StrInterp);
        match &r.tokens[0].value {
            Some(TokenValue::Interp(parts)) => {
                assert_eq!(parts.len(), 2);
                matches!(&parts[0], InterpPart::Lit(s) if s == "hi ");
                matches!(&parts[1], InterpPart::Expr { .. });
            }
            _ => panic!("expected Interp value"),
        }
    }

    #[test]
    fn operators_two_char() {
        use TokenKind::*;
        let r = lex("a && b || c", "t");
        assert!(r.diagnostics.is_empty());
        let kinds: Vec<TokenKind> = r.tokens.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![Ident, Op, Ident, Op, Ident, Eof]);
        assert_eq!(r.tokens[1].text, "&&");
        assert_eq!(r.tokens[3].text, "||");
    }

    #[test]
    fn hex_bin_oct_literals() {
        let r = lex("0xff 0b1010 0o77", "t");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens[0].kind, TokenKind::Int);
        assert_eq!(r.tokens[0].text, "0xff");
        assert_eq!(r.tokens[1].kind, TokenKind::Int);
        assert_eq!(r.tokens[1].text, "0b1010");
        assert_eq!(r.tokens[2].kind, TokenKind::Int);
        assert_eq!(r.tokens[2].text, "0o77");
    }

    #[test]
    fn float_literal() {
        let r = lex("3.14", "t");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens[0].kind, TokenKind::Float);
    }

    #[test]
    fn newlines_are_tokens() {
        use TokenKind::*;
        let r = lex("a\nb", "t");
        let kinds: Vec<TokenKind> = r.tokens.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![Ident, Newline, Ident, Eof]);
    }

    #[test]
    fn block_comment_skipped() {
        let r = lex("a /* b */ c", "t");
        let kinds: Vec<TokenKind> = r.tokens.iter().map(|t| t.kind).collect();
        use TokenKind::*;
        assert_eq!(kinds, vec![Ident, Ident, Eof]);
    }

    #[test]
    fn line_comments_are_captured_with_position_and_ownership() {
        let r = lex("// header\nvar a: int = 0 // trailing\n", "t");
        let c = &r.source_map.comments;
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].line, c[0].col), (1, 1));
        assert_eq!(c[0].text, "header");
        assert!(c[0].own_line);
        assert_eq!((c[1].line, c[1].col), (2, 16));
        assert_eq!(c[1].text, "trailing");
        assert!(!c[1].own_line);
        // Comments produce no tokens.
        let kinds: Vec<TokenKind> = r.tokens.iter().map(|t| t.kind).collect();
        use TokenKind::*;
        assert_eq!(
            kinds,
            vec![Newline, Kw, Ident, Colon, Ident, Op, Int, Newline, Eof]
        );
    }

    #[test]
    fn a_slash_slash_inside_a_string_is_not_a_comment() {
        let r = lex("let s = \"a // b\"\n", "t");
        assert!(r.source_map.comments.is_empty());
    }

    #[test]
    fn doc_comments_stay_tokens_and_are_not_captured() {
        let r = lex("/// doc\nvar a: int = 0\n", "t");
        assert!(r.source_map.comments.is_empty());
        assert_eq!(r.tokens[0].kind, TokenKind::DocComment);
        assert_eq!(r.tokens[0].text, "doc");
    }

    #[test]
    fn line_indent_counts_leading_whitespace_zero_based() {
        let r = lex("a\n  b\n\t\tc\n\nd\n   \n", "t");
        assert_eq!(r.source_map.line_indent[0], 0);
        assert_eq!(r.source_map.line_indent[1], 2);
        assert_eq!(r.source_map.line_indent[2], 2);
        // blank and whitespace-only lines report no indent
        assert_eq!(r.source_map.line_indent[3], 0);
        assert_eq!(r.source_map.line_indent[4], 0);
        assert_eq!(r.source_map.line_indent[5], 0);
        // `Pos::col` is 1-based, so an indent of N is column N+1
        let b = r.tokens.iter().find(|t| t.text == "b").unwrap();
        assert_eq!(b.start.col, r.source_map.line_indent[1] + 1);
    }

    #[test]
    fn crlf_line_endings_leave_no_carriage_return_in_a_comment() {
        let r = lex("// hi\r\nvar a: int = 0\r\n", "t");
        assert_eq!(r.source_map.comments[0].text, "hi");
        assert_eq!(r.source_map.line_indent[1], 0);
    }

    #[test]
    fn keyword_vs_ident() {
        let r = lex("var xyz", "t");
        assert_eq!(r.tokens[0].kind, TokenKind::Kw);
        assert_eq!(r.tokens[0].text, "var");
        assert_eq!(r.tokens[1].kind, TokenKind::Ident);
    }

    #[test]
    fn all_keywords_recognized() {
        for kw in KEYWORDS {
            let r = lex(kw, "t");
            assert_eq!(r.tokens[0].kind, TokenKind::Kw, "{kw} should be recognized as keyword");
            assert_eq!(&r.tokens[0].text, kw);
        }
    }

    #[test]
    fn keyword_set_matches_array() {
        let set = keyword_set();
        assert_eq!(set.len(), KEYWORDS.len(), "keyword set should contain all keywords");
        for kw in KEYWORDS {
            assert!(set.contains(kw), "{kw} missing from keyword set");
        }
    }

    #[test]
    fn annotation_token_lexes() {
        let r = lex("@left in x: bool", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        assert_eq!(r.tokens[0].kind, TokenKind::Annotation);
        assert_eq!(r.tokens[0].text, "left");
        assert_eq!(r.tokens[1].kind, TokenKind::Kw);
        assert_eq!(r.tokens[1].text, "in");
    }

    #[test]
    fn bare_at_is_still_an_error() {
        let r = lex("@ left", "test");
        assert_eq!(r.diagnostics.len(), 1, "diags: {:?}", r.diagnostics);
        assert!(
            r.diagnostics[0].message.contains("unexpected character '@'"),
            "got: {}",
            r.diagnostics[0].message
        );
    }

    #[test]
    fn atom_in_value_position_lexes() {
        use TokenKind::*;
        // After `=`, `(`, `,`, `[`, `=>` an ident-colon is an atom.
        let r = lex("let x = :red", "t");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let a = r.tokens.iter().find(|t| t.kind == Atom).expect("atom token");
        assert_eq!(a.text, ":red");
        match &a.value {
            Some(TokenValue::Str(s)) => assert_eq!(s, "red"),
            other => panic!("expected atom name, got {other:?}"),
        }
    }

    #[test]
    fn atom_allows_hyphens() {
        let r = lex("f(:my-text)", "t");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let a = r.tokens.iter().find(|t| t.kind == TokenKind::Atom).unwrap();
        assert_eq!(a.text, ":my-text");
    }

    #[test]
    fn colon_after_value_stays_a_colon() {
        use TokenKind::*;
        // Type annotation, record field, and map string/atom key separators must
        // NOT become atoms.
        for src in ["var x:int = 0", "{ foo:1 }", "{ \"k\":v }", "{ :red:1 }"] {
            let r = lex(src, "t");
            assert!(
                r.tokens.iter().any(|t| t.kind == Colon),
                "`{src}` should keep a Colon token"
            );
        }
        // `var x:int` produces NO atom.
        let r = lex("var x:int = 0", "t");
        assert!(!r.tokens.iter().any(|t| t.kind == TokenKind::Atom));
    }

    #[test]
    fn lex_nested_prefab_block() {
        let r = lex("$```in a: exec\non a { }\n```", "t");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let t = r
            .tokens
            .iter()
            .find(|t| t.kind == TokenKind::NestedPrefab)
            .expect("a NestedPrefab token");
        match &t.value {
            Some(TokenValue::Str(s)) => assert_eq!(s, "in a: exec\non a { }\n"),
            other => panic!("expected Str value, got {other:?}"),
        }
    }

    #[test]
    fn lex_nested_prefab_nesting_and_strings() {
        // A triple-backtick inside an inner string must not close the block, and a
        // nested `$```…``` must be balanced (the OUTER block owns the whole span).
        let src = "$```on x { let s = \"```\" \n let e = $```in q: exec```\n }```";
        let r = lex(src, "t");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        assert_eq!(
            r.tokens
                .iter()
                .filter(|t| t.kind == TokenKind::NestedPrefab)
                .count(),
            1,
            "outer block owns the whole span; the inner $``` is part of its text"
        );
    }

    #[test]
    fn const_lexes_as_a_keyword() {
        let r = lex("const x = 1", "test");
        assert_eq!(r.tokens[0].kind, TokenKind::Kw);
        assert_eq!(r.tokens[0].text, "const");
    }
