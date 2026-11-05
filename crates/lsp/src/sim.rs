//! Tests in the editor. A "Run test" code lens sits above each `@test`;
//! `wirescript.listTests` tells a test explorer what a file holds, and
//! `wirescript.runTest` runs one test, or a program against its `.expected`
//! file, and returns the gate simulator's verdict (the same one
//! `wirescript test` gives).

use std::path::Path;

use serde_json::{Value, json};
use tower_lsp::lsp_types::{CodeLens, Command, Url};
use wirescript::analysis::text::Ruler;
use wirescript::resolve::FsLoader;
use wirescript::TestKind;
use wirescript_sim::runner::{self, RunSettings};

use crate::range_to_lsp;

/// `[uri, test name or null, { race, quiet }]`: run a test, or with no name
/// the whole program against its `.expected` file.
pub(crate) const RUN_TEST: &str = "wirescript.runTest";

/// `[uri]`: the file's tests, and whether it has a `.expected` file.
pub(crate) const LIST_TESTS: &str = "wirescript.listTests";

/// One "Run test" lens per `@test` in `source`.
pub(crate) fn test_lenses(source: &str, file: &str, uri: &Url) -> Vec<CodeLens> {
    let ruler = Ruler::new(source);
    wirescript::list_tests(source, file)
        .into_iter()
        .map(|t| CodeLens {
            range: range_to_lsp(&ruler, &t.range),
            command: Some(Command {
                title: "Run test".into(),
                command: RUN_TEST.into(),
                arguments: Some(vec![uri.to_string().into(), t.name.into()]),
            }),
            data: None,
        })
        .collect()
}

/// `{ tests: [{ name, kind, range }], expected }` for `source`.
pub(crate) fn list_tests(source: &str, file: &str) -> Value {
    let ruler = Ruler::new(source);
    let tests: Vec<Value> = wirescript::list_tests(source, file)
        .into_iter()
        .map(|t| {
            let kind = match t.kind {
                TestKind::Fn => "fn",
                TestKind::Mod => "mod",
                TestKind::Chip => "chip",
            };
            json!({ "name": t.name, "kind": kind, "range": range_to_lsp(&ruler, &t.range) })
        })
        .collect();
    let expected = Path::new(file).with_extension("expected").is_file();
    json!({ "tests": tests, "expected": expected })
}

/// The verdict on one run: whether it passed, why not, and what it printed.
pub(crate) struct Verdict {
    pub(crate) passed: bool,
    pub(crate) problems: Vec<String>,
    pub(crate) lines: Vec<(u64, String)>,
}

impl Verdict {
    pub(crate) fn to_json(&self) -> Value {
        let lines: Vec<Value> = self
            .lines
            .iter()
            .map(|(tick, text)| json!({ "tick": tick, "text": text }))
            .collect();
        json!({ "passed": self.passed, "problems": self.problems, "lines": lines })
    }
}

fn sibling(file: &str, ext: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(file).with_extension(ext)).ok()
}

/// Run test `name` in `file`, or with no name the whole program against its
/// `.expected` file; with `race`, also under that many seeded handler orders.
pub(crate) fn run(source: &str, file: &str, name: Option<&str>, race: u64) -> Verdict {
    let fail = |problems: Vec<String>| Verdict {
        passed: false,
        problems,
        lines: Vec::new(),
    };
    let expected = match name {
        Some(_) => None,
        None => match sibling(file, "expected") {
            Some(e) => Some(e),
            None => return fail(vec![format!("{file} has no .expected file beside it")]),
        },
    };
    let script = sibling(file, "script");
    let emit = wirescript::emit::EmitOptions::default();
    let prepared = match runner::prepare(source, file, name, script.as_deref(), &emit, &FsLoader) {
        Ok(p) => p,
        Err(e) => return fail(vec![e]),
    };
    let settings = RunSettings {
        ticks: 600,
        dt: 1.0 / 60.0,
        seed: None,
    };
    let outcomes = prepared.run_with_race(&settings, race);
    let mut problems = match &expected {
        Some(e) => runner::expected_problems(e, &outcomes[0]),
        None => outcomes[0].problems(),
    };
    problems.extend(runner::race_problem(&outcomes));
    Verdict {
        passed: problems.is_empty(),
        problems,
        lines: outcomes.into_iter().next().map(|o| o.lines).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "fn double(n: int) -> int {\n  return n * 2\n}\n\n@test fn doubles() {\n  if double(2) != 4 {\n    BroadcastChatMessage(\"FAIL: double(2) is ${double(2)}\")\n  }\n}\n\n@test fn wrong() {\n  if double(2) != 5 {\n    BroadcastChatMessage(\"FAIL: double(2) is not 5\")\n  }\n}\n";

    #[test]
    fn a_lens_runs_each_test() {
        let uri = Url::parse("file:///t.ws").expect("uri");
        let lenses = test_lenses(SOURCE, "t.ws", &uri);
        let names: Vec<_> = lenses
            .iter()
            .map(|l| l.command.as_ref().expect("command").arguments.clone().expect("args")[1].clone())
            .collect();
        assert_eq!(names, ["doubles", "wrong"]);
        assert_eq!(lenses[0].range.start.line, 4);
    }

    #[test]
    fn the_list_names_each_test_and_its_kind() {
        let list = list_tests(SOURCE, "t.ws");
        assert_eq!(list["tests"][1]["name"], "wrong");
        assert_eq!(list["tests"][1]["kind"], "fn");
        assert_eq!(list["tests"][0]["range"]["start"]["line"], 4);
        assert_eq!(list["expected"], false);
    }

    #[test]
    fn a_test_passes_or_says_why_it_failed() {
        let ok = run(SOURCE, "t.ws", Some("doubles"), 0);
        assert!(ok.passed, "{:?}", ok.problems);
        let bad = run(SOURCE, "t.ws", Some("wrong"), 0);
        assert!(!bad.passed);
        assert_eq!(bad.problems, ["FAIL: double(2) is not 5"]);
    }

    #[test]
    fn a_program_without_an_expected_file_cannot_run_as_a_test() {
        let v = run(SOURCE, "t.ws", None, 0);
        assert!(!v.passed);
        assert!(v.problems[0].contains("no .expected file"), "{:?}", v.problems);
    }
}
