use super::*;
use crate::parser::parse;
use crate::template_cache::TemplateCache;
use crate::typecheck::{typecheck, typecheck_with_inference};

mod annotations;
mod basic;
mod block_expr;
mod boundary_pins;
mod chip;
mod compound;
mod const_collections;
mod const_if;
mod const_init;
mod const_params;
mod cse;
mod flatten;
mod fold;
mod fusion;
mod gate_config;
mod generics;
mod imports;
mod namespace_values;
mod opaque;
mod port_side;
mod purity;
mod records;
mod regression_bugs;
mod returns;
mod scope;
mod string;
mod types;
mod varget_cache;
mod wire_completeness;

pub(super) fn compile(src: &str) -> LowerResult {
    let parsed = parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    // Two-pass inference (Task 3), like the real `compile` pipeline, so CE
    // receiver tests (e.g. `gate_config.rs`) see sender-inferred slot types
    // instead of the always-empty default map.
    let (tc, ce_slots) = typecheck_with_inference(&parsed.ast, "test");
    // Unfolded: existing lowering tests are structural microscopes that
    // assert on real gates, not on what the certified fold pass would leave
    // behind. `compile_folded` below is the fold-behavior twin. ForceOff
    // (not Auto) so this stays unfolded regardless of whether `src` happens
    // to carry a module-level `@fold`.
    let mut r = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
        fold_mode: FoldMode::ForceOff,
        ce_slots: &ce_slots,
    });
    r.diagnostics.extend(
        tc.diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Warning),
    );
    r
}

/// Like `compile`, but with the certified fold pass enabled (ForceOn — not
/// gated on a module-level `@fold`, since these are the fold-behavior tests
/// themselves). Use for fold-behavior tests; `compile` stays unfolded so
/// structural lowering tests keep asserting on real gates.
pub(super) fn compile_folded(src: &str) -> LowerResult {
    let parsed = parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    let mut r = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
        fold_mode: FoldMode::ForceOn,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    });
    r.diagnostics.extend(
        tc.diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Warning),
    );
    r
}

/// Like `compile`, but with `fold_mode: FoldMode::Auto` (the production
/// default) — folds unless `src` opts out with a module-level `@nofold`.
pub(super) fn compile_auto(src: &str) -> LowerResult {
    let parsed = parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    let mut r = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
        fold_mode: FoldMode::Auto,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    });
    r.diagnostics.extend(
        tc.diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Warning),
    );
    r
}

/// Compile `entry_src` as the root file, resolving its `import`s against the
/// in-memory `deps` (each `(name, source)` is loaded as `name.ws`).
/// `fold_mode: Auto` (folds by default) — the cross-file tests that assert on
/// gate shape keep their arithmetic real with a runtime input so folding is a
/// no-op; this also doubles as the harness for
/// `imported_module_nofold_is_inert`-style entry-vs-import annotation tests.
pub(super) fn compile_multi(entry_src: &str, deps: &[(&str, &str)]) -> LowerResult {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: deps
            .iter()
            .map(|(k, v)| (format!("{k}.ws"), v.to_string()))
            .collect(),
    };
    let resolved = resolve(entry_src, "main", &loader);
    let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
    lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "main",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::Auto,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    })
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
    let mut seen = crate::collections::HashSet::default();
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
