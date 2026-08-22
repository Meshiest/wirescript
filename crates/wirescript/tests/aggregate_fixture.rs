//! Compile-surface guard for the aggregate-storage self-check fixture
//! (`fixtures/records/aggregate.ws`). The fixture asserts its own RUNTIME values
//! in game; this test guards the compile side: the comprehensive record
//! variable/array/map program must type-check and lower with no errors and, in
//! particular, no `_Unsupported` placeholder (WSP001) — the exact failure mode
//! the aggregate feature exists to remove. A record op that regresses to a
//! placeholder shows up here even though the program still "compiles".

use wirescript::{compile, CompileInput, FoldMode};

#[test]
fn aggregate_fixture_compiles_without_placeholders() {
    let src = include_str!("fixtures/records/aggregate.ws");
    let r = compile(CompileInput {
        source: src,
        file: "aggregate",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("aggregate.ws must compile without errors");
    // A clean example: no warnings at all, and specifically no WSP001
    // (`_Unsupported`) — a record op that silently fell back to a placeholder.
    assert!(
        r.diagnostics.iter().all(|d| d.code != "WSP001"),
        "aggregate.ws lowered a record op to an _Unsupported placeholder: {:?}",
        r.diagnostics
            .iter()
            .filter(|d| d.code == "WSP001")
            .collect::<Vec<_>>()
    );
    assert!(
        r.diagnostics.is_empty(),
        "aggregate.ws should compile with no diagnostics: {:?}",
        r.diagnostics
    );
}
