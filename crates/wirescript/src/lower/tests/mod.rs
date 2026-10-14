use super::*;
use crate::parser::parse;
use crate::template_cache::TemplateCache;
use crate::typecheck::typecheck;

mod alias_conversion;
mod annotations;
mod basic;
mod block_expr;
mod boundary_pins;
mod chip;
mod compound;
mod conditional_writes;
mod const_collections;
mod const_if;
mod const_init;
mod const_params;
mod container_methods;
mod cse;
pub(super) mod declaring_scope;
mod enums;
mod exec_fork;
mod flatten;
mod float_slots;
mod fold;
mod for_loop;
mod fusion;
mod gate_config;
mod generic_differential;
mod generics;
mod int_slots;
mod imports;
mod lexical_scope;
mod namespace_values;
mod opaque;
mod out_reads;
mod position_parity;
mod port_side;
mod purity;
mod record_assign_order;
mod record_field_columns;
mod records;
mod ref_records;
mod refs;
mod regression_bugs;
mod reported_fixes;
mod styles;
pub(in crate::lower) mod resolution;
mod returns;
mod scope;
mod shadow_differential;
mod string;
mod types;
mod varget_cache;
mod wire_completeness;

/// The compiler's front end over `src` as `file` at `fold_mode`, imports
/// loaded from `loader`: the lowering result, carrying every stage's
/// diagnostics, with an internal error beside another error dropped as the
/// compiler drops it.
pub(crate) fn lower_with(src: &str, file: &str, fold_mode: FoldMode, loader: &dyn crate::resolve::FileLoader) -> LowerResult {
    let crate::compile::FrontEnd { resolved, typecheck, mut lowered, .. } =
        crate::compile::front_end(src, file, None, fold_mode, loader);
    lowered.diagnostics.extend(resolved.diagnostics);
    lowered.diagnostics.extend(typecheck.diagnostics);
    crate::diagnostic::drop_internal_errors_beside_errors(&mut lowered.diagnostics);
    lowered
}

/// [`lower_with`] for a single file that must parse cleanly.
fn lower_single(src: &str, fold_mode: FoldMode) -> LowerResult {
    let parsed = parse(src, "test");
    assert!(parsed.diagnostics.is_empty(), "parse diags: {:?}", parsed.diagnostics);
    lower_with(src, "test", fold_mode, &crate::resolve::FsLoader)
}

/// Unfolded: lowering tests are structural microscopes that assert on real
/// gates, not on what the certified fold pass would leave behind.
/// `compile_folded` is the fold-behavior twin. ForceOff (not Auto) so this
/// stays unfolded regardless of whether `src` carries a module-level `@fold`.
pub(super) fn compile(src: &str) -> LowerResult {
    lower_single(src, FoldMode::ForceOff)
}

/// Like `compile`, but with the certified fold pass enabled (ForceOn — not
/// gated on a module-level `@fold`, since these are the fold-behavior tests
/// themselves).
pub(super) fn compile_folded(src: &str) -> LowerResult {
    lower_single(src, FoldMode::ForceOn)
}

/// Like `compile`, but with `fold_mode: FoldMode::Auto` (the production
/// default) — folds unless `src` opts out with a module-level `@nofold`.
pub(super) fn compile_auto(src: &str) -> LowerResult {
    lower_single(src, FoldMode::Auto)
}

/// Compile `entry_src` as the root file, resolving its `import`s against the
/// in-memory `deps` (each `(name, source)` is loaded as `name.ws`).
/// `fold_mode: Auto` (folds by default) — the cross-file tests that assert on
/// gate shape keep their arithmetic real with a runtime input so folding is a
/// no-op; this also doubles as the harness for
/// `imported_module_nofold_is_inert`-style entry-vs-import annotation tests.
pub(super) fn compile_multi(entry_src: &str, deps: &[(&str, &str)]) -> LowerResult {
    let loader = crate::resolve::MemLoader {
        files: deps.iter().map(|(k, v)| (format!("{k}.ws"), v.to_string())).collect(),
    };
    lower_with(entry_src, "main", FoldMode::Auto, &loader)
}

/// The error CODES a source type-checks to, in order. Typecheck only: these
/// tests are about what the checker rejects, so they must not depend on
/// lowering running at all.
pub(super) fn errs(s: &str) -> Vec<String> {
    crate::typecheck::typecheck(
        &crate::parser::parse(s, "t").ast,
        "t",
        &crate::typecheck::CeSlotMap::default(),
    )
    .diagnostics
    .into_iter()
    .filter(|d| d.severity == crate::diagnostic::Severity::Error)
    .map(|d| d.code.to_string())
    .collect()
}

/// The one child module a single-chip-instance program lowers to. `what` names
/// it in the panic, so a lowering regression says which instance went missing
/// rather than "unwrap on None".
pub(super) fn only_chip<'a>(r: &'a LowerResult, what: &str) -> &'a crate::ir::Module {
    r.module.chips.values().next().expect(what)
}

/// The module emits to a `.brz` without error. Lowering can produce a graph the
/// emitter then rejects (an unknown gate class, a port that does not exist), so
/// a lowering test that stops at `assert_no_errors` has not shown the program
/// would load.
pub(super) fn assert_emits(module: &crate::ir::Module, layout: &crate::layout::LayoutResult) {
    let brz = crate::emit::emit_brz(
        module,
        &Default::default(),
        layout,
        &crate::emit::EmitOptions::default(),
    );
    assert!(brz.is_ok(), "should emit valid brz: {:?}", brz.err());
}

/// Asserts a program lowered with no ERROR-severity diagnostic, typecheck's
/// included. The `compile*` helpers above must therefore merge typecheck's
/// errors, not only its warnings, or none of the 460-odd call sites of this
/// function can see one.
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

/// Whether `r` reports the error `code`.
pub(super) fn has_error(r: &LowerResult, code: &str) -> bool {
    r.diagnostics.iter().any(|d| d.code == code && d.severity == crate::diagnostic::Severity::Error)
}

/// Whether `m`, or a chip module below it, holds an `_Unsupported`
/// placeholder.
pub(super) fn has_unsupported(m: &crate::ir::Module) -> bool {
    count_class(m, crate::ir::gate_class::UNSUPPORTED) > 0
}

/// The class of root-module node `id`.
pub(super) fn class_of(r: &LowerResult, id: crate::ir::NodeId) -> &'static str {
    r.module.nodes[&id].gate_class
}

/// The node wired into `node`'s `port`.
pub(super) fn source_of(
    r: &LowerResult,
    node: crate::ir::NodeId,
    port: crate::ir::port_registry::WirePort,
) -> Option<crate::ir::NodeId> {
    r.module.wires.iter().find(|w| w.target.node_id == node && w.target.port == port).map(|w| w.source.node_id)
}

/// Whether `to` runs after `from` in `m`: reachable along exec wires.
pub(super) fn exec_reaches(m: &crate::ir::Module, from: crate::ir::NodeId, to: crate::ir::NodeId) -> bool {
    let mut seen = crate::collections::HashSet::default();
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if seen.insert(n) {
            let next = m.wires.iter().filter(|w| w.source.node_id == n && w.target.port.as_str().starts_with("Exec"));
            stack.extend(next.map(|w| w.target.node_id));
        }
    }
    false
}

/// Whether store `id` writes the integer `v`, held inline or wired from a
/// literal.
pub(super) fn stores_value(m: &crate::ir::Module, id: crate::ir::NodeId, v: i64) -> bool {
    use crate::ir::port_registry::WirePort;
    let is_v = |n: &crate::ir::Node| {
        matches!(n.properties.get(&*crate::intern::sym::VALUE), Some(crate::ir::Literal::Int(x)) if *x == v)
    };
    is_v(&m.nodes[&id])
        || m.wires
            .iter()
            .filter(|w| w.target.node_id == id && w.target.port == WirePort::Value)
            .any(|w| m.nodes.get(&w.source.node_id).is_some_and(|n| n.gate_class == crate::ir::gate_class::LITERAL && is_v(n)))
}

/// The gates and wires of `m`, ignoring node ids: each wire by the classes and
/// ports it joins (a literal source by its value), and each gate class.
pub(super) fn wiring_shape(m: &crate::ir::Module) -> Vec<String> {
    use crate::intern::sym;
    let name = |id: &crate::ir::NodeId| {
        let n = &m.nodes[id];
        match n.properties.get(&*sym::VALUE) {
            Some(v) if n.gate_class == crate::ir::gate_class::LITERAL => format!("{}={v:?}", n.gate_class),
            _ => n.gate_class.to_string(),
        }
    };
    let mut out: Vec<String> = m
        .wires
        .iter()
        .map(|w| format!("{}.{:?} -> {}.{:?}", name(&w.source.node_id), w.source.port, name(&w.target.node_id), w.target.port))
        .collect();
    out.extend(m.nodes.keys().map(name));
    out.sort();
    out
}

pub(super) fn has_gate(r: &LowerResult, class: &str) -> bool {
    r.module.nodes.values().any(|n| n.gate_class == class)
}

/// Every node of class `class` in `m` and its chip modules, a module's own
/// before its chips'.
pub(super) fn nodes_of<'a>(m: &'a crate::ir::Module, class: &str) -> Vec<&'a crate::ir::Node> {
    let mut out = Vec::new();
    m.walk(&mut |_, sub| out.extend(sub.nodes.values().filter(|n| n.gate_class == class)));
    out
}

/// How many nodes of class `class` are in `m` and its chip modules.
pub(super) fn count_class(m: &crate::ir::Module, class: &str) -> usize {
    nodes_of(m, class).len()
}

pub(super) fn gate_count(r: &LowerResult, class: &str) -> usize {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .count()
}

/// Is `to` reachable from `from` by following wires source->target?
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

/// The `NAME_LABEL`s of the `Pseudo_Var` gates that some `Exec_Var_Set` writes
/// through its `VarRef` port. Proves a payload slot is actually WRITTEN, which
/// neither a gate count nor a label lookup can: a slot that is allocated by
/// `build_enum_fields` but never assigned still has its `Pseudo_Var` and its
/// label, so a count-only check passes on a construction that dropped the
/// value entirely.
pub(super) fn written_var_labels(r: &LowerResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.port == WirePort::VarRef)
        .filter(|w| {
            r.module
                .nodes
                .get(&w.target.node_id)
                .is_some_and(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
        })
        .filter_map(|w| r.module.nodes.get(&w.source.node_id))
        .filter(|n| n.gate_class == "BrickComponentType_Var")
        .filter_map(|n| match n.properties.get(&crate::intern::sym::NAME_LABEL) {
            Some(crate::ir::Literal::String(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// The `Value` baked into the `Exec_Var_Set` that writes the `Pseudo_Var`
/// labelled `label`. A constant assigned to a var inlines onto the Var_Set as
/// component data (see `constant_vec_inlines_into_var_set`), so this reads back
/// the VALUE a slot actually receives, which `written_var_labels` cannot see.
pub(super) fn written_value(r: &LowerResult, label: &str) -> Option<crate::ir::Literal> {
    let var = r.module.nodes.iter().find(|(_, n)| {
        n.gate_class == "BrickComponentType_Var"
            && matches!(
                n.properties.get(&crate::intern::sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == label
            )
    })?;
    let set = r.module.wires.iter().find(|w| {
        w.source.node_id == *var.0
            && w.target.port == WirePort::VarRef
            && r.module
                .nodes
                .get(&w.target.node_id)
                .is_some_and(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
    })?;
    r.module.nodes[&set.target.node_id]
        .properties
        .get(&crate::intern::sym::VALUE)
        .cloned()
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
