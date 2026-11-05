//! `wirescript test`: run each `@test` (a `fn`, `mod` or named `chip`) in the
//! gate simulator, and compare a program's chat with the `.expected` file
//! beside it.
//!
//! The verdict of a test run is [`wirescript_sim::runner`]'s. A program with a
//! `<name>.expected` file fails when its chat lines differ from the file's
//! lines. A `<name>.script` file beside either drives the scripted world
//! (players, chat commands).

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use wirescript::resolve::FsLoader;
use wirescript_sim::runner::{self, Outcome, RunSettings};
use wirescript_sim::script_world::escape_controls;

/// How every run in a test session goes.
pub(crate) struct TestArgs {
    pub(crate) ticks: u64,
    pub(crate) dt: f64,
    pub(crate) seed: Option<u64>,
    /// Also run each program under seeds 0 to `race - 1` and fail it when its
    /// chat changes with the order of handlers the game does not order.
    pub(crate) race: u64,
}

pub(crate) fn run_tests(paths: &[PathBuf], args: &TestArgs) -> Result<(), Box<dyn Error>> {
    // An explicit file is named; a directory's files are found.
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found = Vec::new();
            crate::ws_files(path, &mut found)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            files.extend(found.into_iter().map(|f| (f, false)));
        } else {
            files.push((path.clone(), true));
        }
    }
    let mut seen = std::collections::HashSet::new();
    files.retain(|(f, _)| seen.insert(fs::canonicalize(f).unwrap_or_else(|_| f.clone())));

    let (mut passed, mut failed, mut skipped) = (0usize, 0usize, 0usize);
    for (file, named) in &files {
        let name = file.to_string_lossy().to_string();
        let src = match read_text(file) {
            Ok(s) => s,
            Err(e) => {
                report(&name, &[e], &mut passed, &mut failed);
                continue;
            }
        };
        let tests = wirescript::list_tests(&src, &name);
        let expected = match read_sibling(file, "expected") {
            Ok(e) => e,
            Err(e) => {
                report(&format!("{name} (expected output)"), &[e], &mut passed, &mut failed);
                continue;
            }
        };
        if tests.is_empty() && expected.is_none() {
            if *named {
                let why = "no `@test` and no .expected file beside it".to_string();
                report(&name, &[why], &mut passed, &mut failed);
            } else {
                skipped += 1;
            }
            continue;
        }
        let script = match read_sibling(file, "script") {
            Ok(s) => s,
            Err(e) => {
                report(&name, &[e], &mut passed, &mut failed);
                continue;
            }
        };
        if let Some(expected) = expected {
            let label = format!("{name} (expected output)");
            let outcomes = run_all(&src, &name, None, script.as_deref(), args);
            let mut diff = runner::expected_problems(&expected, &outcomes[0]);
            diff.extend(runner::race_problem(&outcomes));
            report(&label, &diff, &mut passed, &mut failed);
        }
        for test in &tests {
            let label = format!("{name}::{}", test.name);
            let outcomes = run_all(&src, &name, Some(&test.name), script.as_deref(), args);
            let mut problems = outcomes[0].problems();
            problems.extend(runner::race_problem(&outcomes));
            report(&label, &problems, &mut passed, &mut failed);
        }
    }
    if skipped > 0 {
        println!("\n{skipped} file(s) with no tests skipped");
    }
    println!("\n{passed} passed; {failed} failed");
    if failed > 0 {
        return Err(format!("{failed} test(s) failed").into());
    }
    if passed == 0 {
        return Err("no tests found: no `@test` and no `.expected` file".into());
    }
    Ok(())
}

/// Compile once, then run under `args.seed` and once per race seed.
fn run_all(
    src: &str,
    file: &str,
    entry: Option<&str>,
    script: Option<&str>,
    args: &TestArgs,
) -> Vec<Outcome> {
    let emit = wirescript::emit::EmitOptions::default();
    let prepared = match runner::prepare(src, file, entry, script, &emit, &FsLoader) {
        Ok(p) => p,
        Err(e) => {
            return vec![Outcome {
                lines: Vec::new(),
                error: Some(e),
            }];
        }
    };
    let settings = RunSettings {
        ticks: args.ticks,
        dt: args.dt,
        seed: args.seed,
    };
    prepared.run_with_race(&settings, args.race)
}

/// A text file, a leading byte-order mark dropped; an error says which file.
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

/// The text of `file`'s sibling with extension `ext`, if there is one.
fn read_sibling(file: &Path, ext: &str) -> Result<Option<String>, String> {
    let path = file.with_extension(ext);
    if !path.is_file() {
        return Ok(None);
    }
    read_text(&path).map(Some)
}

fn report(label: &str, problems: &[String], passed: &mut usize, failed: &mut usize) {
    let label = escape_controls(label);
    if problems.is_empty() {
        println!("test {label} ... ok");
        *passed += 1;
    } else {
        println!("test {label} ... FAILED");
        for p in problems {
            println!("    {}", escape_controls(p));
        }
        *failed += 1;
    }
}
