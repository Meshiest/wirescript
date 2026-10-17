    use super::format_wirescript;

    fn fmt(src: &str) -> String {
        format_wirescript(src, "  ")
    }

    #[test]
    fn a_labelled_loop_keeps_its_annotations() {
        let src = "on go {
@label(\"Rows\") @sliced
for i in 0..3 {
t += i
}
}
";
        let want = "on go {
  @label(\"Rows\") @sliced
  for i in 0..3 {
    t += i
  }
}
";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn multi_line_array_literal_indents_elements() {
        let src = "var names: string[] = [\n\"A\",\n\"B\",\n]\nlet x = 1\n";
        let want = "var names: string[] = [\n  \"A\",\n  \"B\",\n]\nlet x = 1\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn array_literal_inside_handler_stacks_with_block_indent() {
        let src = "on t {\nfoo = [\n1,\n...base,\n2\n]\ndone = true\n}\n";
        let want = "on t {\n  foo = [\n    1,\n    ...base,\n    2\n  ]\n  done = true\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn single_line_arrays_and_index_reads_unaffected() {
        let src = "var base: int[] = [1, 2, 3]\nlet v = arr[i]\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn brackets_in_strings_and_comments_ignored() {
        let src =
            "var a: string[] = [\n\"[not a bracket]\",\n// comment with ] and [\n\"end\",\n]\n";
        let want =
            "var a: string[] = [\n  \"[not a bracket]\",\n  // comment with ] and [\n  \"end\",\n]\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn paren_continuation_still_indents() {
        let src = "on t {\nctrl.DisplayText(\"hi\",\npositionX = 0.0,\n)\n}\n";
        let want = "on t {\n  ctrl.DisplayText(\"hi\",\n    positionX = 0.0,\n  )\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn call_opening_record_literal_indents_once() {
        // `addRole(next, {` opens a paren AND a brace on one line — the
        // record fields indent ONE level, and `})` returns to the opener's
        // level.
        let src = "on init {
emit NONE = addRole(next, {
name: \"S\",
cond: 0
})
done = true
}
";
        let want = "on init {
  emit NONE = addRole(next, {
    name: \"S\",
    cond: 0
  })
  done = true
}
";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn formatting_is_idempotent() {
        let src = "on t {\nfoo = [\n1,\n2\n]\n}\nvar n: string[] = [\n\"x\",\n]\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once);
    }

    use super::{format_with, FormatOptions, LineEnding};

    #[test]
    fn brackets_in_a_multi_line_block_comment_do_not_indent() {
        let src = "var x: int = 0\n/* opens { and [ (\n   still the comment\n*/\non go {\nx = 1\n}\n";
        let want = "var x: int = 0\n/* opens { and [ (\n   still the comment\n*/\non go {\n  x = 1\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn a_bracket_in_an_inline_block_comment_does_not_close() {
        let src = "on go {\nlet a = [\n1,\n2, /* second ] */\n3\n]\n}\n";
        let want = "on go {\n  let a = [\n    1,\n    2, /* second ] */\n    3\n  ]\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn quotes_and_brackets_inside_an_interpolation_are_string() {
        let src = "on go {\nSay(\"${if armed then \"<b>W</> {\" else \"off (\"}\")\nlet y = 1\n}\n";
        let want = "on go {\n  Say(\"${if armed then \"<b>W</> {\" else \"off (\"}\")\n  let y = 1\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn else_on_its_own_line_joins_the_closing_brace() {
        let src = "on go {\nif a {\nx = 1\n}\nelse {\nx = 2\n}\n}\n";
        let want = "on go {\n  if a {\n    x = 1\n  } else {\n    x = 2\n  }\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn else_if_stays_on_its_own_line() {
        let src = "on go {\n  if a {\n    x = 1\n  }\n  else if b {\n    x = 2\n  }\n}\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn fmt_ignore_keeps_lines_as_written() {
        let src = "on go {\n// fmt-ignore\nlet   t   =   [1,2,3]\nlet  u  =  [4,5,6] // fmt-ignore\nlet  v  =  [7,8,9]\n}\n";
        let want = "on go {\n  // fmt-ignore\nlet   t   =   [1,2,3]\nlet  u  =  [4,5,6] // fmt-ignore\n  let v = [7, 8, 9]\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn spacing_is_normalized_outside_strings_and_comments_only() {
        let src = "let a = f(1 ,2,3)  // x ,y  z\nlet s = \"a ,b  c\"\n";
        let want = "let a = f(1, 2, 3) // x ,y  z\nlet s = \"a ,b  c\"\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn consecutive_top_level_declarations_lose_the_blank_line() {
        let src = "in a: exec\n\nin b: exec\n\nvar x: int = 0\n\nvar y: int = 0\n";
        assert_eq!(fmt(src), "in a: exec\nin b: exec\n\nvar x: int = 0\nvar y: int = 0\n");
    }

    #[test]
    fn continuation_lines_go_one_level_deeper() {
        let src = "on go {\nlet a = b\n&& c\nlet d = x\n.Method()\nlet e = if p\nthen 1\nelse 2\n}\n";
        let want = "on go {\n  let a = b\n    && c\n  let d = x\n    .Method()\n  let e = if p\n    then 1\n    else 2\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn a_long_expression_breaks_before_its_loosest_operators_in_one_run() {
        // The first piece is still too wide after breaking at `&&`, so it breaks
        // again at `==`; a second run must change nothing.
        let long = "x".repeat(90);
        let src = format!("on go {{\nlet ok = f(1) == \"{long}\" && g(2) != \"y\"\n}}\n");
        let want = format!("on go {{\n  let ok = f(1)\n    == \"{long}\"\n    && g(2) != \"y\"\n}}\n");
        let once = fmt(&src);
        assert_eq!(once, want);
        assert_eq!(fmt(&once), once);
    }

    #[test]
    fn operators_inside_strings_and_atoms_are_not_break_points() {
        let src = format!("let s = \"{} && || + -\" .. :my-long-atom-name\n", "y".repeat(90));
        let out = fmt(&src);
        assert!(out.lines().all(|l| !l.trim_start().starts_with("&&")), "{out}");
        assert!(out.contains("\n  .. :my-long-atom-name"), "{out}");
    }

    #[test]
    fn a_long_import_fills_its_names_across_lines() {
        let names: Vec<String> = (0..12).map(|i| format!("some_name_{i}")).collect();
        let src = format!("import {{ {} }} from \"lib\"\n", names.join(", "));
        let out = fmt(&src);
        assert!(out.starts_with("import {\n  some_name_0, some_name_1,"), "{out}");
        assert!(out.ends_with("some_name_11,\n} from \"lib\"\n"), "{out}");
        assert!(out.lines().all(|l| l.len() <= 100), "{out}");
        assert_eq!(fmt(&out), out);
    }

    #[test]
    fn a_nested_prefab_block_is_kept_as_written() {
        let src = "on go {\nlet m = SpawnPrefab($```\nin x: exec\n   on x {}\n  ```, offset = here)\nlet y = 1\n}\n";
        let want = "on go {\n  let m = SpawnPrefab($```\nin x: exec\n   on x {}\n  ```, offset = here)\n  let y = 1\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn line_endings_follow_the_input_unless_set() {
        let src = "on go {\r\nx = 1\r\n}\r\n";
        assert_eq!(fmt(src), "on go {\r\n  x = 1\r\n}\r\n");
        let lf = FormatOptions { end_of_line: Some(LineEnding::Lf), ..FormatOptions::default() };
        assert_eq!(format_with(src, &lf), "on go {\n  x = 1\n}\n");
    }

    #[test]
    fn no_print_width_never_splits() {
        let src = format!("let a = {} + {}\n", "b".repeat(80), "c".repeat(80));
        let opts = FormatOptions { print_width: None, ..FormatOptions::default() };
        assert_eq!(format_with(&src, &opts), src);
    }

    #[test]
    fn without_insert_final_newline_the_input_decides() {
        let opts = FormatOptions { insert_final_newline: false, ..FormatOptions::default() };
        assert_eq!(format_with("let a = 1", &opts), "let a = 1");
        assert_eq!(format_with("let a = 1\n", &opts), "let a = 1\n");
    }

    #[test]
    fn editorconfig_sets_the_options() {
        let dir = std::env::temp_dir().join(format!("ws-editorconfig-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = space\nindent_size = 4\n\n[*.ws]\nmax_line_length = 60\nend_of_line = crlf\ninsert_final_newline = false\n\n[sub/*.ws]\nindent_style = tab\nmax_line_length = off\n",
        )
        .unwrap();
        let top = FormatOptions::default().with_editorconfig(&dir.join("main.ws"));
        assert_eq!(top, FormatOptions { indent: "    ".into(), print_width: Some(60), end_of_line: Some(LineEnding::Crlf), insert_final_newline: false });
        let sub = FormatOptions::default().with_editorconfig(&dir.join("sub").join("lib.ws"));
        assert_eq!(sub.indent, "\t");
        assert_eq!(sub.print_width, None);
        let other = FormatOptions::default().with_editorconfig(&dir.join("notes.txt"));
        assert_eq!(other.indent, "    ");
        assert_eq!(other.print_width, Some(100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_editorconfig_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("ws-no-editorconfig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".editorconfig"), "root = true\n").unwrap();
        let opts = FormatOptions::default().indent_with(true, 4);
        assert_eq!(opts.clone().with_editorconfig(&dir.join("a.ws")), opts);
        std::fs::remove_dir_all(&dir).unwrap();
    }
