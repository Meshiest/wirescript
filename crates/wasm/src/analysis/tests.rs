use super::*;

// These test the playground's adapter: the editor's UTF-16 columns in and
// out, the `files_json` loader, the JSON shapes the editor reads, and the
// passes this entry point runs. What the analysis answers is tested in
// `wirescript::analysis`.

/// The editor (UTF-16) column of the `nth` `needle` on 0-based `line`.
fn editor_col(src: &str, line: usize, needle: &str, nth: usize) -> u32 {
    let l = src.lines().nth(line).unwrap();
    let byte = l.match_indices(needle).nth(nth).expect("needle").0;
    l[..byte].encode_utf16().count() as u32
}

fn hover_value(source: &str, line: u32, col: u32) -> Option<String> {
    let raw = hover(source, line, col, "{}")?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed
        .get("value")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
}

fn parse_items(json: &str) -> Vec<serde_json::Value> {
    serde_json::from_str(json).unwrap_or_default()
}

/// A line holding emoji (two UTF-16 units, four bytes, one char each) and
/// an accent (one unit, two bytes) gives a name three columns; the cursor
/// arrives in UTF-16 and must land on it. Four emoji put the char and
/// UTF-16 columns further apart than the name is long.
const NON_ASCII: &str = "var abc: int = 0\nvar xyz: int = 0\nin go: exec\non go {\n  BroadcastChatMessage(\"\u{1F389}\u{1F389}\u{1F389}\u{1F389}\u{e9}\" .. abc .. xyz)\n}";

#[test]
fn hover_reads_an_editor_column_on_a_non_ascii_line() {
    for (needle, want) in [("abc", "var abc: int"), ("xyz", "var xyz: int")] {
        let col = editor_col(NON_ASCII, 4, needle, 0);
        let h =
            hover_value(NON_ASCII, 4, col + 1).unwrap_or_else(|| panic!("no hover on {needle}"));
        assert!(h.contains(want), "{needle}: {h}");
    }
}

#[test]
fn diagnostics_run_lowering_and_read_the_files_json() {
    let codes = |out: &str| -> Vec<String> {
        parse_items(out)
            .iter()
            .map(|d| d["code"].as_str().unwrap_or("").to_string())
            .collect()
    };
    assert!(
        parse_items(&diagnostics("var x: int = 0", "{}")).is_empty(),
        "a clean program has no diagnostics"
    );
    assert!(codes(&diagnostics("on RoundStart() { x = 1 }", "{}")).contains(&"WS002".to_string()));
    let files = r#"{"lib.ws": "mod foo(v: *int) { v = v + 1 }"}"#;
    let items = parse_items(&diagnostics(
        "import \"lib\"\nvar x: int = 0\non RoundStart() { foo(x) }",
        files,
    ));
    assert!(
        items.is_empty(),
        "an import from the files json resolves: {items:?}"
    );
    let items = parse_items(&diagnostics("import \"nonexistent\"", "{}"));
    assert!(
        items.iter().any(|i| i["message"]
            .as_str()
            .unwrap_or("")
            .contains("cannot resolve")),
        "a missing import is reported: {items:?}"
    );
}

/// A misspelled receiver method type-checks clean and lowers to an
/// `_Unsupported` placeholder, which emit drops. This path stopped at
/// typecheck, so the editor showed nothing for a dead circuit while
/// `wirescript_compile` went on to succeed.
#[test]
fn diagnostics_report_an_unsupported_placeholder() {
    let out = diagnostics(
        "var m: Map<int, int>\nvar n: int = 0\non RoundStart() { n = m.size() }\n",
        "{}",
    );
    assert!(
        out.contains("WSP001"),
        "expected a WSP001 placeholder warning, got: {out}"
    );
}

#[test]
fn diagnostics_carry_editor_columns_on_a_non_ascii_line() {
    // `undefined` starts at byte 20 of the line, UTF-16 column 19.
    let items = parse_items(&diagnostics("let s = \"h\u{e9}llo\" .. undefined\n", "{}"));
    let d = items
        .iter()
        .find(|i| i["code"] == "WS002")
        .unwrap_or_else(|| panic!("{items:?}"));
    assert_eq!(
        (
            d["startLine"].as_u64(),
            d["startCol"].as_u64(),
            d["endCol"].as_u64()
        ),
        (Some(0), Some(19), Some(28)),
        "{d:?}"
    );
}

/// The playground completes with the language server's engine, enum
/// lookup included: a match arm offers the scrutinee's variants.
#[test]
fn completion_matches_the_language_server() {
    let src = "enum Hue { Red, Green }
var h: Hue
in go: exec
on go { let n = match h {  } }";
    let col = src.lines().nth(3).unwrap().find("{  }").unwrap() as u32 + 2;
    let items = parse_items(&completions(src, 3, col, "{}", &[]));
    assert!(
        items.iter().any(|i| i["label"] == "Green"),
        "got: {items:?}"
    );
}

/// The playground reads `insertText`: an optional argument inserts
/// `name = `, not its bare label.
#[test]
fn completion_items_carry_insert_text_as_the_editor_reads_it() {
    let src = "in go: exec
on go { SendCustomEvent( }";
    let col = src.lines().nth(1).unwrap().find('(').unwrap() as u32 + 1;
    let items = parse_items(&completions(src, 1, col, "{}", &[]));
    let data1 = items
        .iter()
        .find(|i| i["label"] == "data1 = ")
        .unwrap_or_else(|| panic!("{items:?}"));
    assert_eq!(data1["insertText"], "data1 = ", "{data1:?}");
}

// ---- workspace symbols ----
#[test]
fn workspace_symbols_lists_exports() {
    let files =
        r#"{"utils.ws": "mod inc(v: *int) { v = v + 1 }\nfn double(x: int) -> int = x * 2"}"#;
    let items = parse_items(&workspace_symbols(files));
    assert!(
        items
            .iter()
            .any(|i| i["name"] == "inc" && i["kind"] == "mod")
    );
    assert!(
        items
            .iter()
            .any(|i| i["name"] == "double" && i["kind"] == "fn")
    );
}

// ---- definition ----

/// Where go-to-definition lands, as the editor reads it.
#[derive(Debug, PartialEq)]
struct Landing {
    file: Option<String>,
    line: u64,
    start: u64,
    end: Option<u64>,
}

fn landing(file: Option<&str>, line: u64, start: u64, end: Option<u64>) -> Landing {
    Landing {
        file: file.map(String::from),
        line,
        start,
        end,
    }
}

/// Go-to-definition from each row's editor position; `want` is where it
/// lands (`end: None` leaves the end column unchecked), `None` nowhere.
#[test]
fn definition_lands_on_the_name_in_editor_columns() {
    const LIB: &str = r#"{"lib.ws": "mod foo(v: *int) { v = v + 1 }"}"#;
    const ADD_LIB: &str =
        r#"{"lib.ws": "chip Add(a: int, b: int) -> (result: int) {\n  out result = a + b\n}"}"#;
    const MATH_LIB: &str =
        r#"{"math.ws": "chip Add(a: int, b: int) -> (result: int) {\n  out result = a + b\n}"}"#;
    const EMOJI_DECL: &str =
        "/* \u{1F389} */ var abc: int = 0\non RoundStart() {\n  /* \u{e9}\u{1F389} */ abc = 1\n}";
    let emoji_use = editor_col(EMOJI_DECL, 2, "abc", 0);
    let emoji_decl = u64::from(editor_col(EMOJI_DECL, 0, "abc", 0));
    let nx = |needle: &str| editor_col(NON_ASCII, 4, needle, 0);
    let rows: Vec<(&str, &str, &str, u32, u32, Option<Landing>)> = vec![
        (
            "a var lands on its name, not `var`",
            "var x: int = 0\non RoundStart() { x = 1 }",
            "{}",
            1,
            18,
            Some(landing(None, 0, 4, Some(5))),
        ),
        (
            "a let",
            "var x: int = 0\nlet y = x + 1\nout r = y",
            "{}",
            2,
            8,
            Some(landing(None, 1, 4, Some(5))),
        ),
        (
            "a buffer",
            "var x: int = 0\nbuffer prev = x\nout r = prev",
            "{}",
            2,
            8,
            Some(landing(None, 1, 7, Some(11))),
        ),
        (
            "an in port",
            "in trigger: exec\non trigger { }",
            "{}",
            1,
            3,
            Some(landing(None, 0, 3, Some(10))),
        ),
        (
            "a mod",
            "mod inc(v: *int) { v = v + 1 }\non RoundStart() { inc(x) }",
            "{}",
            1,
            18,
            Some(landing(None, 0, 4, Some(7))),
        ),
        (
            "a chip",
            "chip Foo(x: int) -> (r: int) {\n  out r = x\n}\nlet f = Foo(1)",
            "{}",
            3,
            8,
            Some(landing(None, 0, 5, Some(8))),
        ),
        (
            "a fn",
            "fn double(x: int) -> int = x * 2\nlet y = double(21)",
            "{}",
            1,
            8,
            Some(landing(None, 0, 3, Some(9))),
        ),
        (
            "an array",
            "var items: int[]\non RoundStart() { items.push(1) }",
            "{}",
            1,
            18,
            Some(landing(None, 0, 4, Some(9))),
        ),
        (
            "a mod param",
            "mod inc(v: *int) { v = v + 1 }",
            "{}",
            0,
            19,
            Some(landing(None, 0, 8, Some(9))),
        ),
        (
            "a builtin has no source definition",
            "on RoundStart() { }",
            "{}",
            0,
            3,
            None,
        ),
        (
            "an imported symbol is in the imported file",
            "import \"lib\"\nvar x: int = 0\non RoundStart() { foo(x) }",
            LIB,
            2,
            18,
            Some(landing(Some("lib.ws"), 0, 4, None)),
        ),
        (
            "an import path jumps to the file's top",
            "import \"lib\"\nvar x: int = 0",
            LIB,
            0,
            8,
            Some(landing(Some("lib.ws"), 0, 0, None)),
        ),
        (
            "a named import specifier jumps to the symbol",
            "import { Add } from \"lib\"\nlet r = Add(1, 2)",
            ADD_LIB,
            0,
            10,
            Some(landing(Some("lib.ws"), 0, 5, None)),
        ),
        (
            "an imported use jumps across files",
            "import { Add } from \"math\"\nlet r = Add(1, 2)",
            MATH_LIB,
            1,
            9,
            Some(landing(Some("math.ws"), 0, 5, None)),
        ),
        // Both the cursor and the landing are on lines whose UTF-16,
        // char and byte columns all differ.
        (
            "a use and a declaration on non-ASCII lines",
            EMOJI_DECL,
            "{}",
            2,
            emoji_use + 1,
            Some(landing(None, 0, emoji_decl, Some(emoji_decl + 3))),
        ),
        (
            "the second of two names after emoji",
            NON_ASCII,
            "{}",
            4,
            nx("xyz") + 1,
            Some(landing(None, 1, 4, Some(7))),
        ),
        (
            "the first of two names after emoji",
            NON_ASCII,
            "{}",
            4,
            nx("abc"),
            Some(landing(None, 0, 4, Some(7))),
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, files, line, col, want) in &rows {
        let raw = if *files == "{}" {
            definition(src, *line, *col)
        } else {
            definition_with_files(src, *line, *col, files)
        };
        let got = raw
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|d| Landing {
                file: d["file"].as_str().map(String::from),
                line: d["startLine"].as_u64().unwrap(),
                start: d["startCol"].as_u64().unwrap(),
                end: want.as_ref().and_then(|w| w.end).and(d["endCol"].as_u64()),
            });
        if got != *want {
            failures.push(format!("{label}: want {want:?}, got {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}

// ---- references ----

/// Every reference at `(line, col)` as `(line, start, end)` editor
/// columns, sorted.
fn refs(source: &str, line: u32, col: u32) -> Vec<(u64, u64, u64)> {
    let r = references(source, line, col).expect("a target under the cursor");
    let mut out: Vec<(u64, u64, u64)> = parse_items(&r)
        .iter()
        .map(|l| {
            (
                l["startLine"].as_u64().unwrap(),
                l["startCol"].as_u64().unwrap(),
                l["endCol"].as_u64().unwrap(),
            )
        })
        .collect();
    out.sort_unstable();
    out
}

/// Each site is narrowed to the name token, so a rename rewrites exactly
/// the name and nothing beside it.
#[test]
fn references_are_the_name_tokens_in_editor_columns() {
    let capture = "in target: character\n// character comment\non CharacterSpawned() -> (character) {\n  character.DisplayText(\"hi\")\n}";
    let abc = u64::from(editor_col(NON_ASCII, 4, "abc", 0));
    let xyz = u64::from(editor_col(NON_ASCII, 4, "xyz", 0));
    let rows: &[(&str, &str, u32, u32, Vec<(u64, u64, u64)>)] = &[
        (
            "a var: the decl and both uses",
            "var x: int = 0\non RoundStart() { x = x + 1 }",
            0,
            4,
            vec![(0, 4, 5), (1, 18, 19), (1, 22, 23)],
        ),
        (
            "a mod: the decl and the call",
            "mod inc(v: *int) { v = v + 1 }\non RoundStart() { inc(x) }",
            0,
            4,
            vec![(0, 4, 7), (1, 18, 21)],
        ),
        (
            "a let: the decl and both reads",
            "let y = 42\nout a = y\nout b = y",
            0,
            4,
            vec![(0, 4, 5), (1, 8, 9), (2, 8, 9)],
        ),
        // The `character` capture resolves only within its own handler:
        // not the `: character` type annotation (a separate, type-position
        // namespace) and not a `// character` comment (not AST at all).
        (
            "a capture is scope-aware, not textual",
            capture,
            2,
            29,
            vec![(2, 26, 35), (3, 2, 11)],
        ),
        (
            "names after emoji: the first",
            NON_ASCII,
            0,
            4,
            vec![(0, 4, 7), (4, abc, abc + 3)],
        ),
        (
            "names after emoji: from the use",
            NON_ASCII,
            4,
            xyz as u32 + 1,
            vec![(1, 4, 7), (4, xyz, xyz + 3)],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, line, col, want) in rows {
        let got = refs(src, *line, *col);
        if got != *want {
            failures.push(format!("{label}: want {want:?}, got {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}

// ---- inlay hints and format ----

#[test]
fn inlay_hints_carry_editor_columns() {
    // Line 1: the hint after `x` is char column 14 and UTF-16 column 15.
    let src = "let s = \"\u{1F389}\"\n/* \u{e9}\u{1F389} */ let x = 42\n";
    let hints = parse_items(&inlay_hints(src, "{}"));
    let at = |line: u64| {
        hints
            .iter()
            .find(|h| h["line"] == line)
            .unwrap_or_else(|| panic!("no hint on line {line}: {hints:?}"))
    };
    let (s, x) = (at(0), at(1));
    assert_eq!(
        (s["col"].as_u64(), s["kind"].as_str()),
        (Some(5), Some("type")),
        "{s:?}"
    );
    assert!(s["label"].as_str().unwrap().contains("string"), "{s:?}");
    assert_eq!(x["col"].as_u64(), Some(15), "{x:?}");
    assert!(x["label"].as_str().unwrap().contains("int"), "{x:?}");
}

#[test]
fn format_indents_with_the_editors_tab_settings() {
    let src = "on go {\nx = 1\n}\n";
    assert_eq!(format(src, 4, false), "on go {\n    x = 1\n}\n");
    assert_eq!(format(src, 2, true), "on go {\n\tx = 1\n}\n");
}

#[test]
fn every_completion_kind_has_the_editors_name() {
    use wirescript::analysis::CompletionKind as K;
    let want = [
        (K::Text, "text"),
        (K::Method, "method"),
        (K::Function, "function"),
        (K::Field, "field"),
        (K::Variable, "variable"),
        (K::Class, "type"),
        (K::Module, "module"),
        (K::Property, "property"),
        (K::Keyword, "keyword"),
        (K::Constant, "constant"),
        (K::EnumMember, "enum"),
        (K::Event, "event"),
        (K::File, "file"),
    ];
    for (kind, name) in want {
        assert_eq!(kind_name(kind), name, "{kind:?}");
    }
}

#[test]
fn hover_on_blank_space_is_none() {
    assert_eq!(hover("var a: int = 0\n\nvar b: int = 1", 1, 0, "{}"), None);
}
