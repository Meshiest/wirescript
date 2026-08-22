//! End-to-end test for `@layout("code")`: a real `compile()` call must
//! dispatch to the code-shaped layout engine (row = source line) instead of
//! the default flat DAG layout, and the dispatch must be deterministic.

use wirescript::compile::{CompileInput, FoldMode, compile, compile_to_world};

/// Carries own-line comments and a chip, so the geometry comparison below
/// covers the annotation and nested-plane paths as well as the gate rows.
/// `CompileResult.placements` holds no annotations and no chip interiors, so
/// those two are covered through the emitted brick counts instead.
const SRC: &str = r#"@layout("code")

// counts ticks and wraps at ten
var total: int = 0
in tick: exec

on tick {
  // bump first
  total = total + 1
  if total > 10 {
    total = 0
  }
}

chip Report(t: exec) {
  var seen: int = 0
  // the chip keeps its own tally
  on t { seen = seen + total }
}

let report = Report(tick)
"#;

/// Bricks per grid in the emitted world: the root grid first, then one
/// entry per chip grid. Comment labels and chip interiors are bricks but
/// not root placements, so this is what catches them drifting.
fn brick_counts(source: &str, file: &str) -> Vec<usize> {
    let r = compile_to_world(
        CompileInput {
            source,
            file,
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        wirescript::EmitOptions::default(),
    )
    .expect("compile to world");
    let mut out = vec![r.world.bricks.len()];
    out.extend(r.world.grids.iter().map(|(_, bricks)| bricks.len()));
    out
}

#[test]
fn code_layout_compiles_and_orders_lines_top_down() {
    let r = compile(CompileInput {
        source: SRC,
        file: "code_layout_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("compile");
    assert!(!r.brz.is_empty());
    // Placements occupy >= 4 distinct Placement.x rows (4+ occupied source
    // lines) under code layout — the Dag layout of this program yields a
    // different, wider shape. Then assert determinism: a second compile
    // gives identical placements.
    let rows: std::collections::HashSet<i32> = r.placements.values().map(|p| p.x).collect();
    assert!(rows.len() >= 4, "expected line-stacked rows, got {}", rows.len());
    let r2 = compile(CompileInput {
        source: SRC,
        file: "code_layout_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("compile 2");
    assert_eq!(
        r.placements.len(),
        r2.placements.len(),
        "same shape both runs"
    );
    // NodeIds aren't stable across compiles, so compare geometry as sorted
    // coordinate multisets: both runs must place the same set of positions.
    let coords = |r: &wirescript::CompileResult| -> Vec<(i32, i32, i32)> {
        let mut v: Vec<(i32, i32, i32)> =
            r.placements.values().map(|p| (p.x, p.y, p.z)).collect();
        v.sort();
        v
    };
    assert_eq!(
        coords(&r),
        coords(&r2),
        "identical placement geometry both runs"
    );

    // The root placements skip both the comment labels and the chip's
    // interior, so pin those down through the brick counts.
    let counts = brick_counts(SRC, "code_layout_emit.ws");
    assert_eq!(
        counts,
        brick_counts(SRC, "code_layout_emit.ws"),
        "identical brick counts both runs"
    );

    // …and confirm the fixture's comments actually reach the plane, or the
    // comparison above would be blind to the annotation path.
    let stripped: String = SRC
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let plain_counts = brick_counts(&stripped, "code_layout_emit_uncommented.ws");
    let total = |v: &[usize]| -> usize { v.iter().sum() };
    assert!(
        total(&counts) > total(&plain_counts),
        "the fixture's comments must emit label bricks: {counts:?} vs {plain_counts:?}"
    );
}

/// Same program, minus the `@layout("code")` annotation — the node set
/// (placement count) must be identical (layout mode changes geometry, never
/// which nodes get placed), and the Dag layout's row count must NOT reach
/// the code-layout threshold above, so the two modes are observably
/// different for the same source.
#[test]
fn without_annotation_dag_layout_is_unchanged() {
    let dag_src = SRC.strip_prefix("@layout(\"code\")\n\n").expect("prefix");

    let annotated = compile(CompileInput {
        source: SRC,
        file: "code_layout_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("compile annotated");
    let plain = compile(CompileInput {
        source: dag_src,
        file: "code_layout_emit_plain.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("compile plain");

    assert_eq!(
        annotated.placements.len(),
        plain.placements.len(),
        "annotation must change layout geometry, not the placed node set"
    );

    let dag_rows: std::collections::HashSet<i32> =
        plain.placements.values().map(|p| p.x).collect();
    assert!(
        dag_rows.len() < 4,
        "Dag layout must NOT stack this program into 4+ distinct rows \
         (got {}) — otherwise this test can't tell the two modes apart",
        dag_rows.len()
    );
}
