//! The self-checking fixtures run in game by pasting (`records/aggregate.ws`,
//! `readable_outputs.ws`, the `refs/`, `match/` and `silent/` sets): in both
//! fold modes each lowers and emits with no diagnostic, placeholder gate or
//! dangling wire, which a plausible in-game tally cannot show.

use crate::common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wirescript::ir::{Module, NodeId};
use wirescript::{CompileInput, FoldMode};

const MODES: [(FoldMode, &str); 2] = [
    (FoldMode::ForceOff, "fold off"),
    (FoldMode::Auto, "fold auto"),
];

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = vec![
        dir.join("records/aggregate.ws"),
        dir.join("readable_outputs.ws"),
    ];
    for set in ["refs", "match", "silent"] {
        out.extend(common::ws_fixtures(set));
    }
    out
}

/// Every way `src` falls short of a clean compile in `mode`, one line each.
fn compile_failures(file: &str, src: &str, mode: FoldMode) -> Vec<String> {
    let mut out = Vec::new();
    let (resolved, tc, lowered) = common::run_stages(src, file, mode);
    out.extend(
        resolved
            .diagnostics
            .iter()
            .chain(&tc.diagnostics)
            .chain(&lowered.diagnostics)
            .map(|d| {
                format!(
                    "{:?} [{}] {} at {}:{}",
                    d.severity, d.code, d.message, d.range.start.line, d.range.start.col
                )
            }),
    );

    let mut known: HashSet<NodeId> = HashSet::new();
    lowered
        .module
        .walk(&mut |_, m: &Module| known.extend(m.nodes.keys().chain(m.chips.keys()).copied()));
    lowered.module.walk(&mut |_, m: &Module| {
        let placeholders = m
            .nodes
            .values()
            .filter(|n| n.gate_class == "_Unsupported")
            .count();
        if placeholders > 0 {
            out.push(format!("{placeholders} _Unsupported placeholder gate(s)"));
        }
        for w in &m.wires {
            if !known.contains(&w.source.node_id) || !known.contains(&w.target.node_id) {
                out.push(format!("wire {w:?} has an endpoint no module holds"));
            }
        }
    });

    match wirescript::compile(CompileInput::new(src, file).fold_mode(mode)) {
        Ok(r) => {
            out.extend(
                r.diagnostics
                    .iter()
                    .map(|d| format!("emit: {:?} [{}] {}", d.severity, d.code, d.message)),
            );
            if r.brz.is_empty() {
                out.push("empty .brz".to_string());
            }
        }
        Err(e) => out.push(format!("failed to emit: {e:?}")),
    }
    out
}

#[test]
fn every_paste_fixture_compiles_clean() {
    let mut failures = Vec::new();
    for path in fixtures() {
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let file = path.to_str().unwrap();
        for (mode, label) in MODES {
            for f in compile_failures(file, &src, mode) {
                failures.push(format!("{} ({label}): {f}", path.display()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
