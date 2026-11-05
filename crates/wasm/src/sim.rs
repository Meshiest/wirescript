//! The gate simulator in the browser: compile against the in-memory files,
//! run the program as the save it compiles to, and return its chat; list a
//! file's `@test`s and run one for the verdict `wirescript test` gives.

use serde::Serialize;
use wirescript::TestKind;
use wirescript::analysis::text::Ruler;
use wirescript::emit::EmitOptions;
use wirescript_sim::runner::{self, RunSettings};
use wirescript_sim::{Dt, Loaded, ScriptWorld, SimOptions};

/// The ticks a test runs for: ten seconds, as in `wirescript test`.
const TEST_TICKS: u64 = 600;

#[derive(Serialize)]
struct Line {
    tick: u64,
    text: String,
}

/// What a run printed, and the error that stopped it, if one did.
#[derive(Serialize)]
pub(crate) struct Report {
    lines: Vec<Line>,
    error: Option<String>,
}

/// The settings a simulation runs with.
pub(crate) struct SimRequest<'a> {
    pub(crate) ticks: u64,
    pub(crate) dt: f64,
    pub(crate) seed: Option<u64>,
    pub(crate) test_entry: Option<&'a str>,
    pub(crate) script: Option<&'a str>,
}

pub(crate) fn simulate(
    source: &str,
    file: &str,
    files_json: &str,
    emit: EmitOptions,
    req: &SimRequest<'_>,
) -> Report {
    let fail = |error: String| Report {
        lines: Vec::new(),
        error: Some(error),
    };
    if !(req.dt.is_finite() && req.dt > 0.0) {
        return fail(format!("dt must be a positive number of seconds, not {}", req.dt));
    }
    let world = match req.script.map(ScriptWorld::from_script).transpose() {
        Ok(w) => w.unwrap_or_default(),
        Err(e) => return fail(e),
    };
    let loader = crate::make_loader(files_json);
    let mut input = wirescript::CompileInput::new(source, file).test_build(true);
    if let Some(entry) = req.test_entry {
        input = input.test_entry(entry);
    }
    let result = match wirescript::compile_to_world_with_loader(input, emit, &loader) {
        Ok(r) => r,
        Err(e) => return fail(format!("compile: {e}")),
    };
    let loaded = match Loaded::from_world(&result.world, &result.node_bricks, &result.program) {
        Ok(l) => l,
        Err(e) => return fail(e.to_string()),
    };
    let opts = SimOptions {
        seed: req.seed,
        dt: Dt::Constant(req.dt),
    };
    let mut sim = loaded.start(Box::new(world), opts);
    let error = sim.run_until(req.ticks).err().map(|e| e.to_string());
    Report {
        lines: sim
            .chat()
            .iter()
            .map(|l| Line {
                tick: l.tick,
                text: l.text.clone(),
            })
            .collect(),
        error,
    }
}

/// One `@test` of a file, its lines 0-based and its columns the editor's.
#[derive(Serialize)]
pub(crate) struct TestOut {
    name: String,
    kind: &'static str,
    #[serde(rename = "startLine")]
    start_line: usize,
    #[serde(rename = "startCol")]
    start_col: usize,
    #[serde(rename = "endLine")]
    end_line: usize,
    #[serde(rename = "endCol")]
    end_col: usize,
}

/// The `@test` declarations of `source`, in source order.
pub(crate) fn list_tests(source: &str, file: &str) -> Vec<TestOut> {
    let ruler = Ruler::new(source);
    let pos = |p: &wirescript::Pos| {
        let line = p.line.saturating_sub(1) as usize;
        let col = ruler.byte_col_to_editor(line, p.col.saturating_sub(1) as usize);
        (line, col)
    };
    wirescript::list_tests(source, file)
        .into_iter()
        .map(|t| {
            let (start_line, start_col) = pos(&t.range.start);
            let (end_line, end_col) = pos(&t.range.end);
            TestOut {
                name: t.name,
                kind: match t.kind {
                    TestKind::Fn => "fn",
                    TestKind::Mod => "mod",
                    TestKind::Chip => "chip",
                },
                start_line,
                start_col,
                end_line,
                end_col,
            }
        })
        .collect()
}

/// The verdict on one test: whether it passed, why not, and what it printed.
#[derive(Serialize)]
pub(crate) struct TestReport {
    name: String,
    passed: bool,
    problems: Vec<String>,
    lines: Vec<Line>,
}

/// Run test `name` of `source` once on load for [`TEST_TICKS`] ticks.
pub(crate) fn run_test(
    source: &str,
    file: &str,
    files_json: &str,
    emit: &EmitOptions,
    name: &str,
) -> TestReport {
    let loader = crate::make_loader(files_json);
    let settings = RunSettings {
        ticks: TEST_TICKS,
        dt: 1.0 / 60.0,
        seed: None,
    };
    let (problems, lines) = match runner::prepare(source, file, Some(name), None, emit, &loader) {
        Ok(prepared) => {
            let outcome = prepared.run(&settings);
            (outcome.problems(), outcome.lines)
        }
        Err(e) => (vec![e], Vec::new()),
    };
    TestReport {
        name: name.to_string(),
        passed: problems.is_empty(),
        problems,
        lines: lines
            .into_iter()
            .map(|(tick, text)| Line { tick, text })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "import { double } from \"lib\"\n\n@test fn doubles() {\n  testing.assertEq(double(2), 4)\n}\n\n@test mod wrong() {\n  testing.log(\"checking\")\n  testing.assertEq(double(2), 5)\n}\n";
    const FILES: &str = r#"{"lib.ws":"fn double(n: int) -> int {\n  return n * 2\n}\n"}"#;

    fn run(name: &str) -> serde_json::Value {
        let report = run_test(SOURCE, "main", FILES, &EmitOptions::default(), name);
        serde_json::to_value(&report).expect("json")
    }

    #[test]
    fn the_list_gives_each_test_its_kind_and_line() {
        let list = serde_json::to_value(list_tests(SOURCE, "main")).expect("json");
        assert_eq!(list[0]["name"], "doubles");
        assert_eq!(list[0]["kind"], "fn");
        assert_eq!(list[0]["startLine"], 2);
        assert_eq!(list[1]["name"], "wrong");
        assert_eq!(list[1]["kind"], "mod");
        assert_eq!(list[1]["startLine"], 6);
    }

    #[test]
    fn a_test_passes_or_says_why_it_failed() {
        let ok = run("doubles");
        assert_eq!(ok["passed"], true, "{ok}");
        assert_eq!(ok["problems"], serde_json::json!([]));

        let bad = run("wrong");
        assert_eq!(bad["passed"], false);
        let problems = bad["problems"].as_array().expect("problems");
        assert_eq!(problems.len(), 1, "{bad}");
        assert!(problems[0].as_str().unwrap_or("").starts_with("FAIL main:9:"), "{bad}");
        let lines: Vec<&str> = bad["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .filter_map(|l| l["text"].as_str())
            .collect();
        assert!(lines[0].starts_with("LOG main:8:"), "{lines:?}");
        assert!(!lines.contains(&wirescript::TEST_DONE), "{lines:?}");
    }

    #[test]
    fn a_test_that_does_not_compile_fails_with_the_error() {
        let report = run("missing");
        assert_eq!(report["passed"], false);
        assert_eq!(report["problems"].as_array().map(Vec::len), Some(1), "{report}");
    }
}
