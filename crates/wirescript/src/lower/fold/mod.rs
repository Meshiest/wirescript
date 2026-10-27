//! Certified constant-fold driver: propagates known values through the
//! lowered module tree, across chip boundaries, and turns any node the
//! certified table fully determines into a `_Literal` in place. It also
//! shorts constant `Select`s, truncates constant `Branch`es, and sweeps the
//! dead nodes and chips both leave behind.
//!
//! Two-phase (analyze -> apply) because `Module` is a tree of owned maps: the
//! analyze phase only reads, and a second tree walk mutates using the finished
//! plan. The cleanup sweeps that follow each run to a fixpoint, in the order
//! exec-reachability, pure-demand, union, whole-chip elision.
// `pub` because the `--fold-diff` fuzz harness reaches `eval::eval` and
// `eval::Value` through the `fold_eval` re-export in `lib.rs`. `table` has no
// external consumer.
#[doc(hidden)]
pub mod eval;
pub(crate) mod table;

use std::sync::Arc;

use crate::collections::{HashMap, HashSet};
use crate::intern::{Sym, intern_static, resolve, sym};
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, Type, Wire};
use eval::Value;
use table::{AnnihilatorKind, CertifiedTable, InVariant};

/// Read-only facts about one node, gathered by the snapshot walk: the node
/// borrowed from the `Module` tree (every borrow ends before `apply` mutates)
/// plus fields the fixpoint reads on every attempt.
struct Info<'m> {
    /// The node itself, including the properties `resolve_data_input` reads a
    /// baked call-argument literal from (see `expr.rs::literal_for_property_port`).
    node: &'m Node,
    /// The `_nofold` pseudo-property bars both folding this node and
    /// propagating a known value through it.
    nofold: bool,
    /// The node's own value when it is already a constant: a `_Literal` of any
    /// certified variant, scalar or composite, or a portless string literal.
    lit: Option<Value>,
    /// `STRING_FORMAT_TEXT`'s `FormatString` property (the template). `None`
    /// for every other gate class, and for a `FormatText` whose template is
    /// wired as a port, which never folds.
    format_string: Option<String>,
    /// Data (non-`Exec`) input ports in `ports.inputs` order, the slice `eval`
    /// expects. Derived once per distinct `Arc<GateIO>`, since every instance
    /// of a gate template shares it.
    data_inputs: Arc<[WirePort]>,
    /// A foldable/propagatable node (literal, certified gate, or chip
    /// boundary) always has exactly one output port; anything else (Branch,
    /// Swap, multi-output gates) is left alone by this pass.
    single_output: bool,
}

/// One wire-index bucket. Almost every (node, port) key carries exactly one
/// entry, so the single-entry case is stored inline with no heap allocation;
/// only genuine fan-in/fan-out spills to a `Vec`.
enum Feeds<T> {
    One(T),
    Many(Vec<T>),
}

impl<T: Copy> Feeds<T> {
    fn push(&mut self, v: T) {
        match self {
            Self::One(first) => *self = Self::Many(vec![*first, v]),
            Self::Many(vs) => vs.push(v),
        }
    }

    fn as_slice(&self) -> &[T] {
        match self {
            Self::One(v) => std::slice::from_ref(v),
            Self::Many(vs) => vs,
        }
    }
}

/// (node, port) -> wire endpoints on the other side of every wire touching
/// that port (sources for `in_wires`, targets for `out_wires`).
type WireIndex = HashMap<(NodeId, WirePort), Feeds<PortRef>>;

/// Resolution state of one data input port, from a single read-only pass
/// over the wire list plus the `known` map built so far.
enum Resolved {
    /// Zero incoming wires: the gate reads its port default.
    Unwired,
    Known(Value),
    /// Fan-in (more than one wire, which never folds) or a single wire whose
    /// source is not known yet (it may resolve later in the fixpoint).
    Unresolved,
}

/// What the apply phase does with one planned node id.
enum PlanAction {
    /// Convert the node in place to a `_Literal` carrying `Value`.
    BecomeLiteral(Value),
    /// A `Select` whose `BSelectB` condition resolved: remove the node and
    /// rewire every consumer of its `Output` straight to `chosen_src`.
    ShortCircuit { chosen_src: PortRef },
    /// A `Branch` whose `BCond` condition resolved: remove the node and
    /// splice every exec source feeding it directly onto `taken_targets`
    /// (the wire targets of the taken `ExecOutA`/`ExecOutB`).
    Truncate { taken_targets: Vec<PortRef> },
}

/// Fold every node whose output the certified table fully determines. Known
/// values pass through `MicrochipInput`/`MicrochipOutput`, so a constant
/// argument folds gates inside a callee and its result folds gates outside.
/// Constant `Select`/`Branch` are rewritten in the same fixpoint, then the
/// dead nodes and empty named chips are swept.
///
/// A change anywhere inside a chip clears that chip module's `template_key`,
/// since its grid then differs from the shared template.
pub(crate) fn fold_certified_constants(root: &mut Module) {
    let table = CertifiedTable::certified();

    // Snapshot (read-only)
    let mut infos: HashMap<NodeId, Info<'_>> = HashMap::default();
    let mut ports_memo: HashMap<*const GateIO, (Arc<[WirePort]>, bool)> = HashMap::default();
    root.walk(&mut |_, m| collect_infos(m, &mut infos, &mut ports_memo));

    let mut wires: Vec<Wire> = Vec::new();
    collect_wires(root, &mut wires);

    // (target node, target port) -> every wire source feeding it. Global
    // across the tree, since a wire can connect nodes in different modules
    // (a parent literal feeding a chip's MicrochipInput).
    let mut in_wires: WireIndex = HashMap::default();
    // (source node, source port) -> every wire target it drives; finds a
    // truncated Branch's taken-side targets.
    let mut out_wires: WireIndex = HashMap::default();
    // producer node -> consumer node ids, to drive the fixpoint worklist.
    let mut consumers: HashMap<NodeId, Feeds<NodeId>> = HashMap::default();
    for w in &wires {
        in_wires
            .entry((w.target.node_id, w.target.port))
            .and_modify(|f| f.push(w.source))
            .or_insert(Feeds::One(w.source));
        out_wires
            .entry((w.source.node_id, w.source.port))
            .and_modify(|f| f.push(w.target))
            .or_insert(Feeds::One(w.target));
        consumers
            .entry(w.source.node_id)
            .and_modify(|f| f.push(w.target.node_id))
            .or_insert(Feeds::One(w.target.node_id));
    }

    // Propagate to a fixpoint
    let mut known: HashMap<NodeId, Value> = HashMap::default();
    // Nodes the apply phase rewrites. `known` also holds boundary pass-through
    // values (MicrochipInput/Output), which are never planned or rewritten.
    let mut plan: HashMap<NodeId, PlanAction> = HashMap::default();

    // `infos` is an FxHash map whose iteration order follows NodeId allocation
    // (a process-global counter), so the seed order is sorted to make the
    // fixpoint's processing order reproducible.
    let mut sorted_ids: Vec<NodeId> = infos.keys().copied().collect();
    sorted_ids.sort_unstable();

    let mut worklist: std::collections::VecDeque<NodeId> = sorted_ids.iter().copied().collect();
    let mut queued: HashSet<NodeId> = sorted_ids.iter().copied().collect();

    while let Some(id) = worklist.pop_front() {
        queued.remove(&id);
        if known.contains_key(&id) {
            continue;
        }
        let Some(v) = try_resolve(id, &infos, &in_wires, &out_wires, &known, table, &mut plan)
        else {
            continue;
        };
        known.insert(id, v);
        if let Some(cs) = consumers.get(&id) {
            let mut targets: Vec<NodeId> = cs.as_slice().to_vec();
            targets.sort_unstable();
            for c in targets {
                if !known.contains_key(&c) && queued.insert(c) {
                    worklist.push_back(c);
                }
            }
        }
    }

    // Boundary pass-through and existing literals fill `known` without
    // touching `plan` (`Inc(4)` on `chip Inc(v) -> (r) { out r = v }`), and
    // `cleanup_boundary_feeds` still has work then, so both maps must be empty.
    if plan.is_empty() && known.is_empty() {
        return;
    }

    // Apply
    root.walk_mut(&mut |_, m| apply(m, &plan));

    // Boundary-delivery cleanup. A boundary node is the chip's visual port marker and is never planned,
    // so its feeds need their own cleanup. It runs before the sweeps so what
    // it orphans is swept in this pass; otherwise each orphan survives to
    // `materialize_unfoldable_constants` as a carrier gate nobody reads.
    cleanup_boundary_feeds(root, &known);

    // Runtime `@label(<expr>)` sources are read only by a wire emit adds later
    // (into the label's `Component_TextDisplay.Text`), so in the IR they have
    // no outgoing wire. Protected, or the demand sweep would delete the label.
    let mut protected: HashSet<NodeId> = HashSet::default();
    root.walk(&mut |_, m| collect_label_source_nodes(m, &mut protected));

    // Cleanup sweeps. Order matters: a truncated Branch can strand its untaken side (exec
    // cleanup), which can orphan a pure feeder that only fed the stranded
    // chain (demand sweep), and Branch removal can degrade a downstream
    // Union (union cleanup) before a named-chip instance emptied by all of
    // the above is considered for whole-chip elision.
    sweep_dead_exec(root);
    sweep_dead_pure(root, &protected);
    super::prune_dead_exec_unions(root);
    elide_empty_chips(root);
}

/// Node ids that must survive the demand sweep despite having no outgoing IR
/// wire, because their only consumer is a label or style wire that emit adds
/// (the `dynamic_labels` pass in `wirescript-ir`'s `emit/module.rs`).
fn collect_label_source_nodes(module: &Module, out: &mut HashSet<NodeId>) {
    out.extend(module.dynamic_labels.values().map(|p| p.node_id));
    // A runtime `@color` / `@material` source is consumed the same way.
    out.extend(module.style_sources().map(|p| p.node_id));
    if let Some(p) = &module.root_dynamic_label {
        out.insert(p.node_id);
    }
}

fn collect_infos<'m>(
    module: &'m Module,
    infos: &mut HashMap<NodeId, Info<'m>>,
    ports_memo: &mut HashMap<*const GateIO, (Arc<[WirePort]>, bool)>,
) {
    for (id, n) in &module.nodes {
        let nofold = n.properties.contains_key(&*sym::NO_FOLD);
        let lit = if n.gate_class == gc::LITERAL {
            n.properties.get(&*sym::VALUE).and_then(Value::from_literal)
        } else if n.gate_class == gc::STRING_CONCATENATE && n.ports.inputs.is_empty() {
            // A portless `String_Concatenate` is a bare string literal
            // (`expr.rs::literal_node` emits string constants this way, since
            // they cannot be wire_graph_variant immediates); a real `..`
            // declares its inputs. Read here so `"a" == "b"` can fold.
            match n.properties.get(&*sym::INPUT_A) {
                Some(Literal::String(s)) => Some(Value::Str(s.clone())),
                _ => None,
            }
        } else {
            None
        };
        // A foldable `STRING_FORMAT_TEXT` carries its template in the
        // `FormatString` property (`ops.rs::build_format_text`, or a literal
        // `Fmt(...)` template baked by the call path).
        let format_string = if n.gate_class == gc::STRING_FORMAT_TEXT {
            match n.properties.get(&intern_static("FormatString")) {
                Some(Literal::String(s)) => Some(s.clone()),
                _ => None,
            }
        } else {
            None
        };
        // `ports` is Arc-shared across every instance of a gate template, so
        // the derived port list is memoized per distinct GateIO allocation.
        let (data_inputs, single_output) = ports_memo
            .entry(Arc::as_ptr(&n.ports))
            .or_insert_with(|| {
                let inputs: Arc<[WirePort]> = n
                    .ports
                    .inputs
                    .iter()
                    .filter(|p| p.ty != Type::Exec)
                    .map(|p| WirePort::from_name(resolve(p.name)))
                    .collect();
                (inputs, n.ports.outputs.len() == 1)
            })
            .clone();
        infos.insert(
            *id,
            Info {
                node: n,
                nofold,
                lit,
                format_string,
                data_inputs,
                single_output,
            },
        );
    }
}

fn collect_wires(module: &Module, wires: &mut Vec<Wire>) {
    wires.extend(module.wires.iter().copied());
    for child in module.chips.values() {
        collect_wires(child, wires);
    }
}

/// Resolve one data input port: how many wires feed it, and whether the
/// (single) source is already known.
fn resolve_input(
    target: NodeId,
    port: WirePort,
    in_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
) -> Resolved {
    match in_wires.get(&(target, port)).map(Feeds::as_slice) {
        None => Resolved::Unwired,
        Some([]) => Resolved::Unwired,
        Some([src]) => match known.get(&src.node_id) {
            Some(v) => Resolved::Known(v.clone()),
            None => Resolved::Unresolved,
        },
        Some(_) => Resolved::Unresolved, // fan-in: never folds
    }
}

/// Like `resolve_input`, but an unwired port falls back to the node's baked
/// property: a literal call argument (`expr.rs::literal_for_property_port`)
/// has no wire. This covers data inputs, Select/Branch conditions and
/// `FormatText` slots; without it `Dot(Vec(1,0,0), Vec(0,1,0))` never folds.
fn resolve_data_input(
    target: NodeId,
    port: WirePort,
    in_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
    properties: &HashMap<Sym, Literal>,
) -> Resolved {
    match resolve_input(target, port, in_wires, known) {
        Resolved::Unwired => match properties.get(&port.sym()).and_then(Value::from_literal) {
            Some(v) => Resolved::Known(v),
            None => Resolved::Unwired,
        },
        other => other,
    }
}

/// The wire source feeding `port` on `target` when there is exactly one.
/// Select uses it to find its chosen source without needing that source's
/// value, so an opaque source can still be shorted to.
fn single_wire_source(target: NodeId, port: WirePort, in_wires: &WireIndex) -> Option<PortRef> {
    match in_wires.get(&(target, port)).map(Feeds::as_slice) {
        Some([src]) => Some(*src),
        _ => None,
    }
}

/// Truthiness of Select's `BSelectB` or Branch's `BCond`, wired or baked
/// (`Select(true, a, b)` bakes `true`). `None` if the condition is not known
/// yet (retried later) or its signature is not certified for this gate.
fn resolve_condition(
    id: NodeId,
    cond_port: WirePort,
    gate_class: &'static str,
    in_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
    properties: &HashMap<Sym, Literal>,
    table: &CertifiedTable,
) -> Option<bool> {
    let cond_val: Option<Value> =
        match resolve_data_input(id, cond_port, in_wires, known, properties) {
            Resolved::Unresolved => return None,
            Resolved::Unwired => None,
            Resolved::Known(v) => Some(v),
        };
    let sig = [cond_val.as_ref().map_or(InVariant::Unwired, Value::variant)];
    if !table.covers(gate_class, &sig) {
        return None;
    }
    Some(eval::truthy(cond_val.as_ref()))
}

/// `Select` shorting: `BSelectB` truthy picks `InputB`, falsy `InputA`. The
/// chosen port must have exactly one wire (an unwired chosen input was never
/// probed). Plans a `ShortCircuit` once the condition resolves, and returns
/// the chosen source's value when it is known so consumers keep folding.
fn try_resolve_select(
    id: NodeId,
    in_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
    properties: &HashMap<Sym, Literal>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let truthy = resolve_condition(
        id,
        WirePort::BSelectB,
        gc::SELECT,
        in_wires,
        known,
        properties,
        table,
    )?;
    let chosen_port = if truthy {
        WirePort::InputB
    } else {
        WirePort::InputA
    };
    let chosen_src = single_wire_source(id, chosen_port, in_wires)?;
    plan.insert(id, PlanAction::ShortCircuit { chosen_src });
    known.get(&chosen_src.node_id).cloned()
}

/// `Branch` truncation: `BCond` truthy takes `ExecOutA`, falsy `ExecOutB`.
/// Plans a `Truncate` carrying every target of the taken port; the untaken
/// side loses its incoming wire and is left to the exec-reachability sweep.
fn try_resolve_branch(
    id: NodeId,
    in_wires: &WireIndex,
    out_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
    properties: &HashMap<Sym, Literal>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) {
    let Some(truthy) = resolve_condition(
        id,
        WirePort::BCond,
        gc::BRANCH,
        in_wires,
        known,
        properties,
        table,
    ) else {
        return;
    };
    let taken_port = if truthy {
        WirePort::ExecOutA
    } else {
        WirePort::ExecOutB
    };
    let taken_targets = out_wires
        .get(&(id, taken_port))
        .map(|f| f.as_slice().to_vec())
        .unwrap_or_default();
    plan.insert(id, PlanAction::Truncate { taken_targets });
}

/// `FormatText`: folds when the node carries a `FormatString` property and
/// every wired or baked slot resolves; a slot with no argument renders as
/// unwired (probed `fmtUnwiredSlot`: "0"). At least one slot must be wired or
/// baked, so the zero-slot probe `fmtLiteralBrace` stays unfolded and
/// `tests/it/fold_invariants.rs` sees it unchanged.
fn try_resolve_format_text(
    id: NodeId,
    info: &Info<'_>,
    in_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let template = info.format_string.as_ref()?;
    let any_substituted = eval::FORMAT_SLOTS.iter().any(|&port| {
        in_wires.get(&(id, port)).is_some()
            || info
                .node
                .properties
                .get(&port.sym())
                .and_then(Value::from_literal)
                .is_some()
    });
    if !any_substituted {
        return None;
    }
    let mut inputs: Vec<Option<Value>> = Vec::with_capacity(eval::FORMAT_SLOTS.len());
    for &port in &eval::FORMAT_SLOTS {
        match resolve_data_input(id, port, in_wires, known, &info.node.properties) {
            Resolved::Unwired => inputs.push(None),
            Resolved::Known(v) => inputs.push(Some(v)),
            Resolved::Unresolved => return None,
        }
    }
    let out = eval::format_text(template, &inputs)?;
    let v = Value::Str(out);
    plan.insert(id, PlanAction::BecomeLiteral(v.clone()));
    Some(v)
}

/// Try to resolve node `id` to a value given the `known` set so far. `_Literal`
/// and chip-boundary pass-through nodes just report their (already-settled)
/// value; a certified gate that resolves plans `BecomeLiteral` as a side
/// effect (recorded into `plan`) and reports the folded value too, so its own
/// consumers can chain off it in the same fixpoint. `Select`/`Branch` plan
/// their own structural rewrite (`ShortCircuit`/`Truncate`) instead.
fn try_resolve(
    id: NodeId,
    infos: &HashMap<NodeId, Info<'_>>,
    in_wires: &WireIndex,
    out_wires: &WireIndex,
    known: &HashMap<NodeId, Value>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let info = infos.get(&id)?;
    if info.nofold {
        return None;
    }

    // Already a constant: nothing to plan.
    if info.node.gate_class == gc::LITERAL {
        return info.lit.clone();
    }

    // A portless `String_Concatenate` is a bare string literal (see
    // `collect_infos`); a real `..` has `lit == None` and falls through.
    if info.node.gate_class == gc::STRING_CONCATENATE && info.lit.is_some() {
        return info.lit.clone();
    }

    // Chip-instance boundary: a known value flows through. The node itself is
    // the chip's visual port marker and is never folded.
    if info.node.gate_class == gc::MICROCHIP_INPUT || info.node.gate_class == gc::MICROCHIP_OUTPUT {
        return match resolve_input(id, WirePort::RerInput, in_wires, known) {
            Resolved::Known(v) => Some(v),
            _ => None,
        };
    }

    // `FormatText`'s table signature is the synthetic `[Tmpl]` marker, which
    // no `Value` signature matches, so the generic `table.covers` path would
    // always refuse it; its slots are resolved and rendered separately.
    if info.node.gate_class == gc::STRING_FORMAT_TEXT {
        return try_resolve_format_text(id, info, in_wires, known, plan);
    }

    if info.node.gate_class == gc::SELECT {
        return try_resolve_select(id, in_wires, known, &info.node.properties, table, plan);
    }

    if info.node.gate_class == gc::BRANCH {
        try_resolve_branch(
            id,
            in_wires,
            out_wires,
            known,
            &info.node.properties,
            table,
            plan,
        );
        return None;
    }

    // Certified gate candidate: a real gate with exactly one data output.
    if info.node.kind != NodeKind::Gate || !info.single_output {
        return None;
    }

    let mut states: Vec<Resolved> = Vec::with_capacity(info.data_inputs.len());
    let mut all_resolved = true;
    for &port in info.data_inputs.iter() {
        let st = resolve_data_input(id, port, in_wires, known, &info.node.properties);
        if matches!(st, Resolved::Unresolved) {
            all_resolved = false;
        }
        states.push(st);
    }

    if all_resolved {
        let sig: Vec<InVariant> = states
            .iter()
            .map(|s| match s {
                Resolved::Unwired => InVariant::Unwired,
                Resolved::Known(v) => v.variant(),
                Resolved::Unresolved => unreachable!("all_resolved excludes Unresolved"),
            })
            .collect();
        if table.covers(info.node.gate_class, &sig) {
            let eval_inputs: Vec<Option<Value>> = states
                .iter()
                .map(|s| match s {
                    Resolved::Unwired => None,
                    Resolved::Known(v) => Some(v.clone()),
                    Resolved::Unresolved => unreachable!("all_resolved excludes Unresolved"),
                })
                .collect();
            // `eval::eval` is the pure certified-table evaluator in
            // `fold/eval.rs`, not dynamic code execution.
            if let Some(v) = eval::eval(info.node.gate_class, &eval_inputs) {
                // A covered signature can still yield a non-finite float
                // (float `x % 0.0` is NaN); a NaN/inf literal is never baked.
                let non_finite = matches!(&v, Value::Float(f) if !f.is_finite());
                if !non_finite {
                    plan.insert(id, PlanAction::BecomeLiteral(v.clone()));
                    return Some(v);
                }
            }
        }
    }

    // Annihilator fallback: LogicalAND folds on a known `false` and LogicalOR
    // on a known `true` at either input, even if the other input never
    // resolves (an `Opaque(...)` probe).
    if let Some(kind) = table.annihilator(info.node.gate_class) {
        let want = matches!(kind, AnnihilatorKind::OrTrue);
        for s in &states {
            if let Resolved::Known(Value::Bool(b)) = s
                && *b == want
            {
                plan.insert(id, PlanAction::BecomeLiteral(Value::Bool(want)));
                return Some(Value::Bool(want));
            }
        }
    }

    None
}

/// The `(properties, ports)` of a `_Literal` carrying `value`. Shared by
/// `apply`'s in-place `BecomeLiteral` and Rule B's freshly minted literals so
/// the two shapes cannot drift apart.
fn literal_properties_and_ports(value: &Value) -> (Arc<HashMap<Sym, Literal>>, Arc<GateIO>) {
    let ty = match value {
        Value::Int(_) => Type::Int,
        Value::Float(_) => Type::Float,
        Value::Bool(_) => Type::Bool,
        Value::Str(_) => Type::String,
        Value::Vector { .. } => Type::Vector,
        Value::Rotator { .. } => Type::Rotator,
        Value::Quat { .. } => Type::Quat,
        Value::Color { .. } => Type::Color,
    };
    let mut properties = HashMap::default();
    properties.insert(*sym::VALUE, value.to_literal());
    (Arc::new(properties), Arc::new(GateIO::source(ty)))
}

/// Mutate `module` (and every nested chip) per `plan`: convert each planned
/// `BecomeLiteral` node in place to a `_Literal`, remove each `ShortCircuit`
/// (`Select`) / `Truncate` (`Branch`) node outright, and fix up every wire
/// touching a planned node so consumers land on the right, still-live
/// source.
fn apply(module: &mut Module, plan: &HashMap<NodeId, PlanAction>) {
    let mut mutated = false;

    let mut ids: Vec<NodeId> = module.nodes.keys().copied().collect();
    ids.sort_unstable();
    for id in &ids {
        match plan.get(id) {
            Some(PlanAction::BecomeLiteral(value)) => {
                let (properties, ports) = literal_properties_and_ports(value);
                let node = module
                    .nodes
                    .get_mut(id)
                    .expect("node id came from module.nodes.keys()");
                node.gate_class = gc::LITERAL;
                node.properties = properties;
                node.ports = ports;
                mutated = true;
            }
            Some(PlanAction::ShortCircuit { .. } | PlanAction::Truncate { .. }) => {
                module.nodes.remove(id);
                mutated = true;
            }
            None => {}
        }
    }

    // Wires touching a planned node are rewritten in whichever module stores
    // them: a cross-scope wire lives in either endpoint's module. A folded
    // node's output becomes `_Literal`'s "Output"; Select/Branch are gone.
    let wires_touched = module
        .wires
        .iter()
        .any(|w| plan.contains_key(&w.source.node_id) || plan.contains_key(&w.target.node_id));
    if wires_touched {
        let taken = std::mem::take(&mut module.wires);
        module.wires = rewrite_wires(taken, plan);
    }

    if mutated || wires_touched {
        module.template_key = None;
    }
}

/// Rewrite one module's wire list per `plan`, looping to a local fixpoint so
/// a chain of removed nodes (a Select shorted into another Select, a Branch
/// truncated into another Branch) leaves no wire to a deleted node.
fn rewrite_wires(wires: Vec<Wire>, plan: &HashMap<NodeId, PlanAction>) -> Vec<Wire> {
    let mut wires = wires;
    loop {
        let mut changed = false;
        let mut next: Vec<Wire> = Vec::with_capacity(wires.len());
        for w in wires {
            // Target side: does this wire feed a node the plan converts or
            // removes?
            match plan.get(&w.target.node_id) {
                Some(PlanAction::BecomeLiteral(_)) => {
                    changed = true;
                    continue; // a literal has no inputs
                }
                Some(PlanAction::ShortCircuit { .. }) => {
                    changed = true;
                    continue; // the Select is gone; its own inputs vanish
                }
                Some(PlanAction::Truncate { taken_targets }) => {
                    changed = true;
                    if w.target.port == WirePort::Exec {
                        for t in taken_targets {
                            next.push(Wire {
                                source: w.source,
                                target: *t,
                            });
                        }
                    }
                    // `BCond`'s own feed wire (or any other) just drops.
                    continue;
                }
                None => {}
            }
            // Source side: does this wire originate from a node the plan
            // redirects/removes?
            let mut w = w;
            match plan.get(&w.source.node_id) {
                Some(PlanAction::BecomeLiteral(_)) => {
                    if w.source.port != WirePort::Output {
                        w.source.port = WirePort::Output;
                        changed = true;
                    }
                }
                Some(PlanAction::ShortCircuit { chosen_src }) => {
                    if w.source != *chosen_src {
                        w.source = *chosen_src;
                        changed = true;
                    }
                }
                Some(PlanAction::Truncate { .. }) => {
                    // Wires out of the Branch drop: the taken side's targets
                    // were re-homed by the target-side arm above, and the
                    // untaken side is left to the exec-reachability sweep.
                    changed = true;
                    continue;
                }
                None => {}
            }
            next.push(w);
        }
        wires = next;
        if !changed {
            break;
        }
    }
    wires
}

/// Every boundary node id of a named chip instance (each `module.chips`
/// child's `inputs`/`outputs`, recursively). The root's own `in`/`out` have
/// the same shape but face the outside world, where an `out` with no outgoing
/// wire is normal, so they are excluded.
fn collect_call_boundary_ids(root: &Module) -> HashSet<NodeId> {
    root.modules_deep()
        .into_iter()
        .skip(1)
        .flat_map(|chip| chip.inputs.iter().chain(&chip.outputs).copied())
        .collect()
}

/// Boundary-delivery cleanup on named-chip boundary nodes whose value is in
/// `known`: Rule B, then Rule A, since Rule B creates the zero-outgoing-wire
/// state Rule A removes. A `_nofold` boundary never enters `known`.
fn cleanup_boundary_feeds(root: &mut Module, known: &HashMap<NodeId, Value>) {
    let boundary_ids = collect_call_boundary_ids(root);
    if boundary_ids.is_empty() {
        return;
    }

    // Read-only pre-pass: Rule B copies its new literal's placement metadata
    // from the CONSUMER of each rewired wire, and that consumer can live in a
    // different module than the wire (or the mutation cursor), so snapshot
    // exactly the consumers Rule B will touch before any mutation starts.
    let consumer_ids = collect_boundary_consumer_ids(root, known, &boundary_ids);
    let meta = collect_consumer_meta(root, &consumer_ids);
    #[cfg(debug_assertions)]
    assert_no_nofold_known_boundary(root, known, &boundary_ids);

    rewire_boundary_consumers(root, known, &meta, &boundary_ids);

    // Counted over the whole tree after Rule B, which may have removed a
    // node's last outgoing wire.
    let outgoing = tally_boundary_outgoing(root, &boundary_ids);
    let dead_feeds: HashSet<NodeId> = boundary_ids
        .into_iter()
        .filter(|id| known.contains_key(id) && outgoing.get(id).copied().unwrap_or(0) == 0)
        .collect();
    if !dead_feeds.is_empty() {
        root.walk_mut(&mut |_, m| drop_boundary_feeds(m, &dead_feeds));
    }
}

/// Rule B: re-source every wire out of a known boundary node from a fresh
/// `_Literal` with that value, inserted in the module that stores the wire.
/// The literal copies the consumer's placement (`metas`) so it renders where
/// it is read. The consumer receives the same value either way, so this is
/// behaviour-preserving for every consumer, certified or not.
fn rewire_boundary_consumers(
    module: &mut Module,
    known: &HashMap<NodeId, Value>,
    metas: &HashMap<NodeId, Node>,
    boundary_ids: &HashSet<NodeId>,
) {
    let mut mutated = false;
    for w in &mut module.wires {
        if !boundary_ids.contains(&w.source.node_id) {
            continue;
        }
        let Some(value) = known.get(&w.source.node_id) else {
            continue;
        };
        let meta = metas
            .get(&w.target.node_id)
            .expect("wire target must have been snapshotted by collect_consumer_meta");
        let (properties, ports) = literal_properties_and_ports(value);
        let lit_id = NodeId::fresh();
        module.nodes.insert(
            lit_id,
            Node::new(
                lit_id,
                NodeKind::Gate,
                gc::LITERAL,
                ports,
                properties,
                Some(meta),
                Some("boundary-delivery cleanup literal"),
            ),
        );
        w.source = PortRef {
            node_id: lit_id,
            port: WirePort::Output,
        };
        mutated = true;
    }
    if mutated {
        module.template_key = None;
    }
    for child in module.chips.values_mut() {
        rewire_boundary_consumers(child, known, metas, boundary_ids);
    }
}

/// The target node id of every wire Rule B will rewire (source is a Known
/// call-boundary node): the exact set `collect_consumer_meta` must cover.
fn collect_boundary_consumer_ids(
    root: &Module,
    known: &HashMap<NodeId, Value>,
    boundary_ids: &HashSet<NodeId>,
) -> HashSet<NodeId> {
    root.wires_deep()
        .filter(|w| {
            boundary_ids.contains(&w.source.node_id) && known.contains_key(&w.source.node_id)
        })
        .map(|w| w.target.node_id)
        .collect()
}

/// A `_nofold` node's `try_resolve` returns `None` before ever reaching
/// the boundary pass-through case, so it can never enter `known`, checked
/// here instead of just assumed.
#[cfg(debug_assertions)]
fn assert_no_nofold_known_boundary(
    root: &Module,
    known: &HashMap<NodeId, Value>,
    boundary_ids: &HashSet<NodeId>,
) {
    for n in root.nodes_deep() {
        debug_assert!(
            !(boundary_ids.contains(&n.id)
                && known.contains_key(&n.id)
                && n.properties.contains_key(&*sym::NO_FOLD)),
            "cleanup_boundary_feeds: a _nofold boundary node must never enter `known` \
             (try_resolve's info.nofold check must return None before the pass-through case)"
        );
    }
}

/// A copy of each CONSUMER of a rewired boundary wire, whose placement Rule B
/// gives the fresh `_Literal` it mints, so the new node renders where it's
/// read (same chip/row/scope as the consumer), not off in the chip the value
/// originated from.
fn collect_consumer_meta(root: &Module, consumer_ids: &HashSet<NodeId>) -> HashMap<NodeId, Node> {
    root.nodes_deep()
        .filter(|n| consumer_ids.contains(&n.id))
        .map(|n| (n.id, n.clone()))
        .collect()
}

fn tally_boundary_outgoing(
    root: &Module,
    boundary_ids: &HashSet<NodeId>,
) -> HashMap<NodeId, usize> {
    let mut out: HashMap<NodeId, usize> = HashMap::default();
    for w in root.wires_deep() {
        if boundary_ids.contains(&w.source.node_id) {
            *out.entry(w.source.node_id).or_default() += 1;
        }
    }
    out
}

/// Rule A: drop the feed wire of each boundary node in `dead_feeds` (no
/// outgoing wire left anywhere), so its orphaned source is swept instead of
/// becoming a carrier gate for a port nobody reads. The boundary node itself
/// stays as the chip's visual port marker.
fn drop_boundary_feeds(module: &mut Module, dead_feeds: &HashSet<NodeId>) {
    let before = module.wires.len();
    module.wires.retain(|w| {
        !(dead_feeds.contains(&w.target.node_id) && w.target.port == WirePort::RerInput)
    });
    if module.wires.len() != before {
        module.template_key = None;
    }
}

/// Whether `gate_class` produces exec without being driven: every exec input
/// its call spec declares is optional, so an unwired exec pin is the normal
/// state rather than a missing trigger. Derived from the catalog; a class with
/// no call spec (`Var_Set`, boundary pins, rerouters) is a chain step.
fn self_firing(gate_class: &str) -> bool {
    static SELF_FIRING: std::sync::LazyLock<HashSet<&'static str>> =
        std::sync::LazyLock::new(|| {
            crate::catalog::calls::calls()
                .values()
                .filter(|spec| {
                    let exec_in: Vec<_> =
                        spec.params.iter().filter(|p| p.ty == Type::Exec).collect();
                    !exec_in.is_empty()
                        && exec_in.iter().all(|p| p.optional)
                        && spec.outputs.iter().any(|o| o.ty == Type::Exec)
                })
                .map(|spec| spec.gate_class)
                .collect()
        });
    SELF_FIRING.contains(gate_class)
}

/// Exec-reachability cleanup: remove a gate whose exec inputs together carry
/// no wire (it lost its trigger), transitively along its exec successors.
/// Events, boundary IO, Variables, rerouters, self-firing gates and `_nofold`
/// nodes survive; a `Union` survives while any exec input is wired.
fn sweep_dead_exec(root: &mut Module) {
    // Pass 1: each gate's exec input ports (memoized per Arc-shared `GateIO`)
    // and which gates are sweep-eligible.
    let mut ports_memo: HashMap<*const GateIO, Arc<[WirePort]>> = HashMap::default();
    let mut exec_ports: HashMap<NodeId, Arc<[WirePort]>> = HashMap::default();
    let mut eligible: HashSet<NodeId> = HashSet::default();
    root.walk(&mut |_, module| {
        for (id, n) in &module.nodes {
            if n.kind != NodeKind::Gate
                || n.properties.contains_key(&*sym::NO_FOLD)
                || n.gate_class == gc::REROUTER
            {
                continue;
            }
            // A gate whose exec inputs are all optional fires on its own
            // (`Timer` counts down with `restart`/`pause`/`resume` unwired).
            // Sweeping it would also delete its `on t.Expired` handler.
            if self_firing(n.gate_class) {
                continue;
            }
            let ports = Arc::clone(ports_memo.entry(Arc::as_ptr(&n.ports)).or_insert_with(|| {
                n.ports
                    .inputs
                    .iter()
                    .filter(|p| p.ty == Type::Exec)
                    .map(|p| WirePort::from_name(resolve(p.name)))
                    .collect()
            }));
            if !ports.is_empty() {
                eligible.insert(*id);
                exec_ports.insert(*id, ports);
            }
        }
    });

    // Pass 2: exec-in total per gate, and for each gate the successors it feeds
    // exec into (so a removal can decrement their totals).
    let mut exec_total: HashMap<NodeId, usize> = HashMap::default();
    let mut exec_succ: HashMap<NodeId, Feeds<NodeId>> = HashMap::default();
    for w in root.wires_deep() {
        if exec_ports
            .get(&w.target.node_id)
            .is_some_and(|ports| ports.contains(&w.target.port))
        {
            *exec_total.entry(w.target.node_id).or_default() += 1;
            exec_succ
                .entry(w.source.node_id)
                .and_modify(|f| f.push(w.target.node_id))
                .or_insert(Feeds::One(w.target.node_id));
        }
    }

    let mut dead: HashSet<NodeId> = HashSet::default();
    let mut work: Vec<NodeId> = eligible
        .iter()
        .filter(|id| exec_total.get(id).copied().unwrap_or(0) == 0)
        .copied()
        .collect();
    while let Some(d) = work.pop() {
        if !dead.insert(d) {
            continue;
        }
        if let Some(succs) = exec_succ.get(&d) {
            for &t in succs.as_slice() {
                if let Some(c) = exec_total.get_mut(&t) {
                    *c = c.saturating_sub(1);
                    if *c == 0 && eligible.contains(&t) && !dead.contains(&t) {
                        work.push(t);
                    }
                }
            }
        }
    }
    if !dead.is_empty() {
        root.remove_nodes_tree(&dead);
    }
}

/// Pure demand sweep: remove a literal or pure `Expr_*` gate (not `_nofold`,
/// not `protected`) with no outgoing non-`Layout` wire, transitively up its
/// feeders. Rerouters and boundary nodes are never `Expr_*`, so an `Opaque`
/// barrier survives.
fn sweep_dead_pure(root: &mut Module, protected: &HashSet<NodeId>) {
    let mut out_deg: HashMap<NodeId, usize> = HashMap::default();
    let mut in_sources: HashMap<NodeId, Feeds<NodeId>> = HashMap::default();
    let mut pure_eligible: HashSet<NodeId> = HashSet::default();

    for w in root.wires_deep() {
        if w.source.port != WirePort::Layout {
            *out_deg.entry(w.source.node_id).or_default() += 1;
            in_sources
                .entry(w.target.node_id)
                .and_modify(|f| f.push(w.source.node_id))
                .or_insert(Feeds::One(w.source.node_id));
        }
    }
    for n in root.nodes_deep() {
        let is_pure_class = n.gate_class == gc::LITERAL || gc::is_pure_expr(n.gate_class);
        if n.kind == NodeKind::Gate
            && !n.properties.contains_key(&*sym::NO_FOLD)
            && !protected.contains(&n.id)
            && is_pure_class
        {
            pure_eligible.insert(n.id);
        }
    }

    let mut dead: HashSet<NodeId> = HashSet::default();
    let mut work: Vec<NodeId> = pure_eligible
        .iter()
        .filter(|id| out_deg.get(id).copied().unwrap_or(0) == 0)
        .copied()
        .collect();
    while let Some(d) = work.pop() {
        if !dead.insert(d) {
            continue;
        }
        if let Some(srcs) = in_sources.get(&d) {
            for &s in srcs.as_slice() {
                if let Some(c) = out_deg.get_mut(&s) {
                    *c = c.saturating_sub(1);
                    if *c == 0 && pure_eligible.contains(&s) && !dead.contains(&s) {
                        work.push(s);
                    }
                }
            }
        }
    }
    if !dead.is_empty() {
        root.remove_nodes_tree(&dead);
    }
}

/// Whole-chip elision, bottom-up: remove a named chip instance whose module
/// holds only boundary IO with no wire anywhere in the tree, unless the chip
/// node carries a label, closed, doc, color, material or `_nofold` property.
/// Anon chips are not `module.chips` entries yet; their swept nodes simply
/// never become a chip at partition.
fn elide_empty_chips(root: &mut Module) {
    // A boundary wire may live in either endpoint's module, so endpoints are
    // collected tree-wide once; elision only removes wires on elided chips,
    // which cannot change another node's status.
    let touched: HashSet<NodeId> = root
        .wires_deep()
        .flat_map(|w| [w.source.node_id, w.target.node_id])
        .collect();
    elide_empty_chips_rec(root, &touched);
}

fn elide_empty_chips_rec(module: &mut Module, touched: &HashSet<NodeId>) {
    for child in module.chips.values_mut() {
        elide_empty_chips_rec(child, touched);
    }

    let mut chip_ids: Vec<NodeId> = module.chips.keys().copied().collect();
    chip_ids.sort_unstable();
    for chip_id in chip_ids {
        let Some(child) = module.chips.get(&chip_id) else {
            continue;
        };
        let all_boundary = child
            .nodes
            .keys()
            .all(|id| child.inputs.contains(id) || child.outputs.contains(id));
        if !all_boundary {
            continue;
        }
        let boundary_wire_free = child
            .inputs
            .iter()
            .chain(child.outputs.iter())
            .all(|id| !touched.contains(id));
        if !boundary_wire_free {
            continue;
        }
        let Some(chip_node) = module.nodes.get(&chip_id) else {
            continue;
        };
        let annotated = chip_node.properties.contains_key(&*sym::NAME_LABEL)
            || chip_node.properties.contains_key(&*sym::CHIP_CLOSED)
            || chip_node.properties.contains_key(&*sym::DOC_TEXT)
            || chip_node.properties.contains_key(&*sym::BRICK_COLOR)
            || chip_node.properties.contains_key(&*sym::BRICK_MATERIAL)
            || chip_node.properties.contains_key(&*sym::NO_FOLD);
        if annotated {
            continue;
        }
        module.nodes.remove(&chip_id);
        module.chips.remove(&chip_id);
        module
            .wires
            .retain(|w| w.source.node_id != chip_id && w.target.node_id != chip_id);
    }
}
