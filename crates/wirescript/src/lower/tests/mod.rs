use super::*;
use crate::ir::gate_class as gc;
use crate::parser::parse;
use crate::template_cache::TemplateCache;
use crate::typecheck::typecheck;

mod alias_conversion;
mod annotations;
mod arrays;
mod basic;
mod block_expr;
mod boundary_pins;
mod builtins;
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
mod enum_values;
mod enums;
mod events;
mod exec_fork;
mod flatten;
mod float_slots;
mod fold;
mod for_loop;
mod fusion;
mod gate_config;
mod generic_differential;
mod generics;
mod imports;
mod int_slots;
mod lexical_scope;
mod maps;
mod namespace_values;
mod nofold;
mod opaque;
mod out_reads;
mod port_side;
mod position_parity;
mod purity;
mod record_assign_order;
mod record_field_columns;
mod records;
mod ref_records;
mod refs;
mod regression_bugs;
mod reported_fixes;
pub(in crate::lower) mod resolution;
mod returns;
mod scope;
mod shadow_differential;
mod shape;
mod signals;
mod string;
mod styles;
mod types;
mod value_match;
mod var_init;
mod varget_cache;
mod wire_completeness;

/// The compiler's front end over `src` as `file` at `fold_mode`, imports
/// loaded from `loader`: the lowering result, carrying every stage's
/// diagnostics, with an internal error beside another error dropped as the
/// compiler drops it.
pub(crate) fn lower_with(
    src: &str,
    file: &str,
    fold_mode: FoldMode,
    loader: &dyn crate::resolve::FileLoader,
) -> LowerResult {
    let crate::compile::FrontEnd {
        resolved,
        typecheck,
        mut lowered,
        ..
    } = crate::compile::front_end(src, file, None, fold_mode, loader);
    lowered.diagnostics.extend(resolved.diagnostics);
    lowered.diagnostics.extend(typecheck.diagnostics);
    crate::diagnostic::drop_internal_errors_beside_errors(&mut lowered.diagnostics);
    lowered
}

/// [`lower_with`] for a single file that must parse cleanly.
fn lower_single(src: &str, fold_mode: FoldMode) -> LowerResult {
    let parsed = parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    lower_with(src, "test", fold_mode, &crate::resolve::FsLoader)
}

/// Unfolded: lowering tests are structural microscopes that assert on real
/// gates, not on what the certified fold pass would leave behind.
/// `compile_folded` is the fold-behavior twin. `ForceOff` (not Auto) so this
/// stays unfolded regardless of whether `src` carries a module-level `@fold`.
pub(super) fn compile(src: &str) -> LowerResult {
    lower_single(src, FoldMode::ForceOff)
}

/// Like `compile`, but with the certified fold pass enabled: `ForceOn`, not gated
/// on a module-level `@fold`, since these are the fold-behavior tests themselves.
pub(super) fn compile_folded(src: &str) -> LowerResult {
    lower_single(src, FoldMode::ForceOn)
}

/// Like `compile`, but with `fold_mode: FoldMode::Auto` (the production
/// default): folds unless `src` opts out with a module-level `@nofold`.
pub(super) fn compile_auto(src: &str) -> LowerResult {
    lower_single(src, FoldMode::Auto)
}

/// [`compile`], asserting no errors.
#[track_caller]
pub(super) fn compile_ok(src: &str) -> LowerResult {
    let r = compile(src);
    assert_no_errors(&r);
    r
}

/// [`compile_folded`], asserting no errors.
#[track_caller]
pub(super) fn compile_ok_folded(src: &str) -> LowerResult {
    let r = compile_folded(src);
    assert_no_errors(&r);
    r
}

/// [`compile_auto`], asserting no errors.
#[track_caller]
pub(super) fn compile_ok_auto(src: &str) -> LowerResult {
    let r = compile_auto(src);
    assert_no_errors(&r);
    r
}

/// The module [`compile_ok`] lowers `src` to: unfolded, so a gate that is
/// absent was removed by const evaluation, never by the optimizer.
#[track_caller]
pub(crate) fn lower_ok(src: &str) -> crate::ir::Module {
    compile_ok(src).module
}

/// The program's `.wsir` text, as `compile --dump-ir` prints it, and every
/// diagnostic as `CODE message`, compiled at `Auto`. Panics when `src` has errors.
#[track_caller]
pub(super) fn wsir_and_diags(src: &str) -> (String, Vec<String>) {
    let input = crate::CompileInput {
        source: src,
        file: "t.ws",
        module_name: None,
        fold_mode: crate::FoldMode::Auto,
    };
    let result = match crate::compile_to_world(input, crate::emit::EmitOptions::default()) {
        Ok(r) => r,
        Err(crate::CompileError::HasErrors(ds)) => {
            panic!(
                "{:?}\n{src}",
                ds.iter()
                    .map(|d| format!("{} {}", d.code, d.message))
                    .collect::<Vec<_>>()
            )
        }
        Err(e) => panic!("{e:?}\n{src}"),
    };
    let diags = result
        .diagnostics
        .iter()
        .map(|d| format!("{} {}", d.code, d.message))
        .collect();
    let mut program = result.program;
    crate::prepare_program_for_text(
        &mut program,
        "t.ws",
        std::path::Path::new("t.wsir"),
        crate::FoldMode::Auto,
    )
    .expect("prepare");
    (
        crate::wsir::program_to_string(&program).expect("write"),
        diags,
    )
}

/// The first line of `.wsir` `text` containing `needle`.
#[track_caller]
pub(super) fn wsir_line<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
}

/// Whether output `name` of `m` is driven once, by its backing var.
pub(super) fn backed(m: &crate::ir::Module, name: &str) -> bool {
    let out = m.nodes.iter().find(|(_, n)| {
        n.gate_class == gc::MICROCHIP_OUTPUT
            && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == name)
    });
    let Some((id, _)) = out else { return false };
    let drivers: Vec<_> = m
        .wires
        .iter()
        .filter(|w| w.target.node_id == *id && w.target.port == WirePort::RerInput)
        .collect();
    drivers.len() == 1 && m.nodes[&drivers[0].source.node_id].note == Some("out_backing")
}

/// Compile `entry_src` as the root file, resolving its `import`s against the
/// in-memory `deps` (each `(name, source)` is loaded as `name.ws`), at
/// `fold_mode: Auto`. Cross-file gate-shape tests keep their arithmetic real with
/// a runtime input, so folding is a no-op for them.
pub(super) fn compile_multi(entry_src: &str, deps: &[(&str, &str)]) -> LowerResult {
    let loader = crate::resolve::MemLoader {
        files: deps
            .iter()
            .map(|(k, v)| (format!("{k}.ws"), v.to_string()))
            .collect(),
    };
    lower_with(entry_src, "main", FoldMode::Auto, &loader)
}

/// The error CODES a source type-checks to, in order. Typecheck only: these
/// tests are about what the checker rejects, so they must not depend on
/// lowering running at all.
pub(super) fn errs(s: &str) -> Vec<String> {
    crate::typecheck::typecheck(
        &crate::parser::parse(s, "t").ast,
        &crate::typecheck::CeSlotMap::default(),
    )
    .diagnostics
    .into_iter()
    .filter(|d| d.severity == crate::diagnostic::Severity::Error)
    .map(|d| d.code)
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
        &crate::ir::ProgramSettings::default(),
        layout,
        &crate::emit::EmitOptions::default(),
    );
    assert!(brz.is_ok(), "should emit valid brz: {:?}", brz.err());
}

/// Asserts a program lowered with no ERROR-severity diagnostic, typecheck's
/// included. The `compile*` helpers above must therefore merge typecheck's
/// errors, not only its warnings, or this cannot see one.
#[track_caller]
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
    r.diagnostics
        .iter()
        .any(|d| d.code == code && d.severity == crate::diagnostic::Severity::Error)
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
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == node && w.target.port == port)
        .map(|w| w.source.node_id)
}

/// Whether `to` runs after `from` in `m`: reachable along exec wires.
pub(super) fn exec_reaches(
    m: &crate::ir::Module,
    from: crate::ir::NodeId,
    to: crate::ir::NodeId,
) -> bool {
    let mut seen = crate::collections::HashSet::default();
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if seen.insert(n) {
            let next = m
                .wires
                .iter()
                .filter(|w| w.source.node_id == n && w.target.port.as_str().starts_with("Exec"));
            stack.extend(next.map(|w| w.target.node_id));
        }
    }
    false
}

/// Whether store `id` writes the integer `v`, held inline or wired from a
/// literal.
pub(super) fn stores_value(m: &crate::ir::Module, id: crate::ir::NodeId, v: i64) -> bool {
    use crate::ir::port_registry::WirePort;
    let is_v = |n: &crate::ir::Node| matches!(n.properties.get(&*crate::intern::sym::VALUE), Some(crate::ir::Literal::Int(x)) if *x == v);
    is_v(&m.nodes[&id])
        || m.wires
            .iter()
            .filter(|w| w.target.node_id == id && w.target.port == WirePort::Value)
            .any(|w| {
                m.nodes
                    .get(&w.source.node_id)
                    .is_some_and(|n| n.gate_class == crate::ir::gate_class::LITERAL && is_v(n))
            })
}

/// The gates and wires of `m`, ignoring node ids: each wire by the classes and
/// ports it joins (a literal source by its value), and each gate class.
pub(super) fn wiring_shape(m: &crate::ir::Module) -> Vec<String> {
    use crate::intern::sym;
    let name = |id: &crate::ir::NodeId| {
        let n = &m.nodes[id];
        match n.properties.get(&*sym::VALUE) {
            Some(v) if n.gate_class == crate::ir::gate_class::LITERAL => {
                format!("{}={v:?}", n.gate_class)
            }
            _ => n.gate_class.to_string(),
        }
    };
    let mut out: Vec<String> = m
        .wires
        .iter()
        .map(|w| {
            format!(
                "{}.{:?} -> {}.{:?}",
                name(&w.source.node_id),
                w.source.port,
                name(&w.target.node_id),
                w.target.port
            )
        })
        .collect();
    out.extend(m.nodes.keys().map(name));
    out.sort();
    out
}

/// Whether the root module (not its chips) has a node of class `class`. An
/// absence check must use [`has_gate_deep`], or a gate inside a chip passes it.
pub(super) fn has_gate(r: &LowerResult, class: &str) -> bool {
    r.module.nodes.values().any(|n| n.gate_class == class)
}

/// Whether `r`'s module or any chip module below it has a node of class `class`.
pub(super) fn has_gate_deep(r: &LowerResult, class: &str) -> bool {
    r.module.nodes_deep().any(|n| n.gate_class == class)
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

/// How many nodes of class `class` the root module (not its chips) has;
/// [`count_class`] counts the whole tree.
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

/// The `NAME_LABEL`s of the `Var` gates that some `Exec_Var_Set` writes
/// through its `VarRef` port. This proves a payload slot is WRITTEN: a slot
/// `build_enum_fields` allocates but never assigns still has its `Var` gate and
/// label, so a count or label check alone passes on a dropped value.
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
                .is_some_and(|n| n.gate_class == gc::VAR_SET)
        })
        .filter_map(|w| r.module.nodes.get(&w.source.node_id))
        .filter(|n| n.gate_class == gc::VAR)
        .filter_map(
            |n| match n.properties.get(&crate::intern::sym::NAME_LABEL) {
                Some(crate::ir::Literal::String(s)) => Some(s.clone()),
                _ => None,
            },
        )
        .collect();
    out.sort();
    out
}

/// The `Value` baked into the `Exec_Var_Set` that writes the `Var` gate
/// labelled `label`. A constant assigned to a var inlines onto the `Var_Set` as
/// component data (see `constant_vec_inlines_into_var_set`), so this reads back
/// the VALUE a slot actually receives, which `written_var_labels` cannot see.
pub(super) fn written_value(r: &LowerResult, label: &str) -> Option<crate::ir::Literal> {
    let var = r.module.nodes.iter().find(|(_, n)| {
        n.gate_class == gc::VAR
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
                .is_some_and(|n| n.gate_class == gc::VAR_SET)
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
