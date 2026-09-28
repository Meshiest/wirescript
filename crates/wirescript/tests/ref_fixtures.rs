//! The `tests/fixtures/refs/` set: self-checking programs for ref values, run
//! in game by pasting each one. This guard holds the compile side. Every fixture
//! compiles with no diagnostics at all, so a construct that lowered to a
//! placeholder (a WSP001 warning) cannot hide behind a passing run.

mod common;

use wirescript::{CompileInput, FoldMode};

fn fixtures() -> Vec<std::path::PathBuf> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/refs");
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .expect("fixtures/refs must exist")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("ws"))
        .collect();
    out.sort();
    assert!(!out.is_empty(), "no fixtures found; the suite must not silently pass empty");
    out
}

#[test]
fn every_ref_fixture_compiles_without_diagnostics() {
    for path in fixtures() {
        let file = path.to_str().unwrap();
        let src = std::fs::read_to_string(&path).unwrap();
        for (mode, label) in [(FoldMode::ForceOff, "fold off"), (FoldMode::Auto, "fold auto")] {
            let (resolved, tc, lowered) = common::run_stages(&src, file, mode);
            let diagnostics: Vec<String> = resolved
                .diagnostics
                .iter()
                .chain(tc.diagnostics.iter())
                .chain(lowered.diagnostics.iter())
                .map(|d| format!("{:?} [{}] {}", d.severity, d.code, d.message))
                .collect();
            assert!(
                diagnostics.is_empty(),
                "{file} ({label}) must compile with no diagnostics, got {diagnostics:?}"
            );
        }
    }
}

#[test]
fn every_ref_fixture_emits_a_brz() {
    for path in fixtures() {
        let src = std::fs::read_to_string(&path).unwrap();
        wirescript::compile::compile(CompileInput {
            source: &src,
            file: path.to_str().unwrap(),
            module_name: None,
            fold_mode: FoldMode::Auto,
        })
        .unwrap_or_else(|e| panic!("{} failed to emit: {e:?}", path.display()));
    }
}
