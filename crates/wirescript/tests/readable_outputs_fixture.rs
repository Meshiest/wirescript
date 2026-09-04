//! Compile-surface guard for the readable-output self-check fixture
//! (`fixtures/readable_outputs.ws`). The fixture asserts its own RUNTIME
//! values in game; this test guards the compile side. A port read that
//! regressed to an `_Unsupported` placeholder (WSP001) would still "compile"
//! and would still print a plausible tally, so the placeholder check is the
//! part that cannot be replaced by running the circuit.

use wirescript::{CompileInput, FoldMode, compile};

#[test]
fn readable_outputs_fixture_compiles_without_placeholders() {
    let src = include_str!("fixtures/readable_outputs.ws");
    let r = compile(CompileInput {
        source: src,
        file: "readable_outputs",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("readable_outputs.ws must compile without errors");
    assert!(
        r.diagnostics.iter().all(|d| d.code != "WSP001"),
        "a port read lowered to an _Unsupported placeholder: {:?}",
        r.diagnostics
            .iter()
            .filter(|d| d.code == "WSP001")
            .collect::<Vec<_>>()
    );
    assert!(
        r.diagnostics.is_empty(),
        "readable_outputs.ws should compile with no diagnostics: {:?}",
        r.diagnostics
    );
}
