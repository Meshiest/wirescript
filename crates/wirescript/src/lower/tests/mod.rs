use super::*;
use crate::parser::parse;
use crate::template_cache::TemplateCache;
use crate::typecheck::typecheck;

mod basic;
mod block_expr;
mod chip;
mod compound;
mod fusion;
mod purity;
mod records;
mod returns;
mod scope;
mod string;
mod types;

pub(super) fn compile(src: &str) -> LowerResult {
    let parsed = parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = typecheck(&parsed.ast, "test");
    let mut r = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
    });
    r.diagnostics.extend(
        tc.diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Warning),
    );
    r
}

pub(super) fn assert_no_errors(r: &LowerResult) {
    assert!(
        r.diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .count()
            == 0,
        "unexpected errors: {:?}",
        r.diagnostics
    );
}

pub(super) fn has_gate(r: &LowerResult, class: &str) -> bool {
    r.module.nodes.values().any(|n| n.gate_class == class)
}

pub(super) fn gate_count(r: &LowerResult, class: &str) -> usize {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .count()
}

/// Is `to` reachable from `from` by following wires source→target?
pub(super) fn wired_reachable(
    r: &LowerResult,
    from: crate::ir::NodeId,
    to: crate::ir::NodeId,
) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if !seen.insert(n) {
            continue;
        }
        for w in &r.module.wires {
            if w.source.node_id == n {
                stack.push(w.target.node_id);
            }
        }
    }
    false
}

/// The single node with the given gate class (panics if absent/ambiguous).
pub(super) fn find_gate(r: &LowerResult, class: &str) -> crate::ir::NodeId {
    let matches: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == class)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one {class}, found {}",
        matches.len()
    );
    matches[0]
}
