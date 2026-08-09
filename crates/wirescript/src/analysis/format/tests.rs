    use super::format_wirescript;

    fn fmt(src: &str) -> String {
        format_wirescript(src, "  ")
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
        // level (previously double-indented).
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
