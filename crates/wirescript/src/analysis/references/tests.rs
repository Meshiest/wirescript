    use super::*;

    #[test]
    fn rename_edit_text_shorthand_expands_to_field_colon_new_name() {
        let site = TextRange {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 3,
            is_shorthand: true,
        };
        assert_eq!(rename_edit_text(&site, "foo", "bar"), "foo: bar");
    }

    #[test]
    fn rename_edit_text_plain_site_replaces_outright() {
        let site = TextRange {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 3,
            is_shorthand: false,
        };
        assert_eq!(rename_edit_text(&site, "foo", "bar"), "bar");
    }

    fn range(file: &str, start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> SourceRange {
        SourceRange {
            file: file.into(),
            start: crate::diagnostic::Pos { offset: 0, line: start_line, col: start_col },
            end: crate::diagnostic::Pos { offset: 0, line: end_line, col: end_col },
        }
    }

    #[test]
    fn find_name_range_is_word_boundary_aware() {
        // `helper` must not match inside `helperX` or `myhelper` — only the
        // whole-token occurrence counts.
        let source = "let helperX = 1\nlet myhelper = 2\nlet helper = 3\n";
        let decl_range = range("t.ws", 1, 1, 3, 1); // spans the whole snippet
        let found = find_name_range(source, &decl_range, "helper").expect("match found");
        assert_eq!(found.start.line, 3, "must skip helperX/myhelper and land on the real token");
        assert_eq!(found.start.col, 5); // "let " is 4 chars, 1-based col 5
    }

    #[test]
    fn find_name_range_searches_every_line_of_a_multiline_range() {
        // A coarse decl/import range can span several lines; the name may
        // not be on the range's FIRST line.
        let source = "import {\n  helperX,\n  helper\n} from \"lib\"\n";
        let decl_range = range("t.ws", 1, 1, 4, 14); // the whole `import { … }` span
        let found = find_name_range(source, &decl_range, "helper").expect("match found");
        assert_eq!(found.start.line, 3, "must search past line 1 to find the real token");
        assert_eq!(found.start.col, 3);
        assert_eq!(found.end.col, 9);
    }
