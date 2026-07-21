//! Certified constant-fold driver: propagates known values through the
//! lowered module tree (including across chip boundaries) and converts any
//! node whose result is fully determined by the certified table/evaluator
//! into a `_Literal`, in place. Also shorts a constant-conditioned `Select`
//! to its chosen data source, truncates a constant-conditioned `Branch` to
//! its taken exec side, and sweeps the structural debris both leave behind
//! (dead exec chains, orphaned pure feeders, degenerate unions, emptied
//! named-chip instances).
//!
//! Two-phase (analyze → apply) so the propagation walk never has to fight
//! `Module`'s tree-of-owned-maps borrows: the analyze phase only reads, the
//! apply phase is a second, independent tree walk that mutates using the
//! finished plan. The cleanup sweeps that follow are their own read/mutate
//! passes, each run to a fixpoint, in the order the mechanics require:
//! exec-reachability → pure-demand → union → whole-chip elision.
// `pub` (not `pub(crate)`): the `--fold-diff` fuzz harness needs `eval::eval`
// + `eval::Value` — see the visibility note on `pub mod fold` in
// `lower/mod.rs`. `table` has no external consumer and stays crate-private.
#[doc(hidden)]
pub mod eval;
pub(crate) mod table;

use std::sync::Arc;

use crate::collections::{HashMap, HashSet};
use crate::intern::{resolve, sym};
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{GateIO, Literal, Module, NodeId, NodeKind, PortRef, PortSpec, Type, Wire};
use eval::Value;
use table::{AnnihilatorKind, CertifiedTable, InVariant};

/// Read-only facts about one node, gathered by the snapshot walk. Enough to
/// decide foldability without holding a borrow of the `Module` tree.
struct Info {
    gate_class: &'static str,
    kind: NodeKind,
    /// `properties` carries the `_nofold` pseudo-property — a barrier both
    /// to folding this node AND to propagating a known value through it.
    nofold: bool,
    /// The node's own constant value, when it's already a `_Literal` (and
    /// that literal is a certified scalar variant — vector/array/etc. are
    /// never propagated by this pass).
    lit: Option<Value>,
    /// Data (non-`Exec`) input ports, in `ports.inputs` declaration order —
    /// exactly the slice `eval` expects.
    data_inputs: Vec<WirePort>,
    /// A foldable/propagatable node (literal, certified gate, or chip
    /// boundary) always has exactly one output port; anything else (Branch,
    /// Swap, multi-output gates) is left alone by this pass.
    single_output: bool,
}

/// Resolution state of one data input port, from a single read-only pass
/// over the wire list plus the `known` map built so far.
enum Resolved {
    /// Zero incoming wires — the gate reads its port default.
    Unwired,
    Known(Value),
    /// Fan-in (>1 wire — invalid anyway, never folds) or a single wire whose
    /// source isn't known yet (may still resolve later in the fixpoint).
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
    /// (the wire targets the taken `ExecOutA`/`ExecOutB` used to drive).
    Truncate { taken_targets: Vec<PortRef> },
}

/// Fold every node whose output is fully determined by the certified table:
/// literal-only expression chains collapse to a single `_Literal`, and a
/// known value crosses chip-instance boundaries via `MicrochipInput`/
/// `MicrochipOutput` pass-through so a constant argument can fold gates
/// inside a callee (and its result can fold gates back outside). A
/// constant-conditioned `Select`/`Branch` is shorted/truncated in the SAME
/// fixpoint, and the structural debris that leaves behind (dead exec
/// chains, orphaned pure feeders, degenerate unions, emptied named-chip
/// instances) is swept away afterward.
///
/// `root` is walked with every nested chip module; a fold anywhere inside a
/// chip clears that chip module's `template_key` (its emitted grid diverged
/// from the shared template).
pub(crate) fn fold_certified_constants(root: &mut Module) {
    let table = CertifiedTable::certified();

    // ---------------- snapshot (read-only) ----------------
    let mut infos: HashMap<NodeId, Info> = HashMap::default();
    collect_infos(root, &mut infos);

    let mut wires: Vec<Wire> = Vec::new();
    collect_wires(root, &mut wires);

    // (target node, target port) -> every wire source feeding it. Global
    // across the whole tree — a wire can connect nodes in different modules
    // (e.g. a literal in the parent feeding a chip's MicrochipInput), so
    // resolution must not be scoped to one module's own `wires` Vec.
    let mut in_wires: HashMap<(NodeId, WirePort), Vec<PortRef>> = HashMap::default();
    // (source node, source port) -> every wire target it drives. Needed to
    // find a truncated Branch's taken-side targets (an OUTPUT-port lookup,
    // the mirror of `in_wires`).
    let mut out_wires: HashMap<(NodeId, WirePort), Vec<PortRef>> = HashMap::default();
    // producer node -> consumer node ids, to drive the fixpoint worklist.
    let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    for w in &wires {
        in_wires
            .entry((w.target.node_id, w.target.port))
            .or_default()
            .push(w.source);
        out_wires
            .entry((w.source.node_id, w.source.port))
            .or_default()
            .push(w.target);
        consumers.entry(w.source.node_id).or_default().push(w.target.node_id);
    }

    // ---------------- propagate to fixpoint ----------------
    let mut known: HashMap<NodeId, Value> = HashMap::default();
    // Nodes planned for the apply phase. `known` also holds boundary
    // pass-through values (MicrochipInput/Output) that are never planned —
    // those nodes stay exactly as they are; only `plan` members get rewritten.
    let mut plan: HashMap<NodeId, PlanAction> = HashMap::default();

    // Deterministic seed order: `infos`/`in_wires` are FxHash maps, so their
    // natural iteration order shifts with NodeId allocation noise (fresh ids
    // are a process-global counter shared across every concurrently-running
    // test). Sorting makes the fixpoint's *processing order* reproducible;
    // the fixpoint's *result* is order-independent by construction (each
    // node's resolution depends only on the current `known` set), but we
    // still sort — same lesson as `partition_anon_chips`.
    let mut sorted_ids: Vec<NodeId> = infos.keys().copied().collect();
    sorted_ids.sort_unstable();

    let mut worklist: std::collections::VecDeque<NodeId> =
        sorted_ids.iter().copied().collect();
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
            let mut targets: Vec<NodeId> = cs.clone();
            targets.sort_unstable();
            for c in targets {
                if !known.contains_key(&c) && queued.insert(c) {
                    worklist.push_back(c);
                }
            }
        }
    }

    if plan.is_empty() {
        return;
    }

    // ---------------- apply ----------------
    apply(root, &plan);

    // ---------------- cleanup sweeps ----------------
    // Order matters: a truncated Branch can strand its untaken side (exec
    // cleanup), which can orphan a pure feeder that only fed the stranded
    // chain (demand sweep), and Branch removal can degrade a downstream
    // Union (union cleanup) before a named-chip instance emptied by all of
    // the above is considered for whole-chip elision.
    sweep_dead_exec(root);
    sweep_dead_pure(root);
    super::prune_dead_exec_unions(root);
    elide_empty_chips(root);
}

fn collect_infos(module: &Module, infos: &mut HashMap<NodeId, Info>) {
    for (id, n) in &module.nodes {
        let nofold = n.properties.contains_key(&*sym::NO_FOLD);
        let lit = if n.gate_class == gc::LITERAL {
            n.properties
                .get(&*sym::VALUE)
                .and_then(Value::from_literal)
        } else if n.gate_class == gc::STRING_CONCATENATE && n.ports.inputs.is_empty() {
            // A portless `String_Concatenate` (no declared input ports at
            // all — a REAL `..` concatenation always declares InputA/
            // InputB/Separator) is how a bare string literal is actually
            // represented: `lower/expr.rs::literal_node` emits string
            // constants this way instead of `_Literal` ("String literals
            // can't be inlined as wire_graph_variant immediate values on
            // consumer gates"), and `materialize_unfoldable_constants` uses
            // the identical shape for a string routed through a dataless
            // sink. Without this, a plain string constant is invisible to
            // `known` — `"a" == "b"` (certified: string equality IS
            // covered) would never fold, even standing completely alone.
            match n.properties.get(&*sym::INPUT_A) {
                Some(Literal::String(s)) => Some(Value::Str(s.clone())),
                _ => None,
            }
        } else {
            None
        };
        let data_inputs: Vec<WirePort> = n
            .ports
            .inputs
            .iter()
            .filter(|p| p.ty != Type::Exec)
            .map(|p| WirePort::from_name(resolve(p.name)))
            .collect();
        let single_output = n.ports.outputs.len() == 1;
        infos.insert(
            *id,
            Info {
                gate_class: n.gate_class,
                kind: n.kind,
                nofold,
                lit,
                data_inputs,
                single_output,
            },
        );
    }
    for child in module.chips.values() {
        collect_infos(child, infos);
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
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
) -> Resolved {
    match in_wires.get(&(target, port)) {
        None => Resolved::Unwired,
        Some(srcs) if srcs.is_empty() => Resolved::Unwired,
        Some(srcs) if srcs.len() > 1 => Resolved::Unresolved, // fan-in: never folds
        Some(srcs) => match known.get(&srcs[0].node_id) {
            Some(v) => Resolved::Known(v.clone()),
            None => Resolved::Unresolved,
        },
    }
}

/// The single wire source feeding `port` on `target`, iff there's exactly
/// one (zero = unwired, refuse; >1 = fan-in, refuse) — used by Select to
/// find its chosen data source without requiring that source's VALUE to be
/// known (an opaque, never-folding source can still be shorted to).
fn single_wire_source(
    target: NodeId,
    port: WirePort,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
) -> Option<PortRef> {
    match in_wires.get(&(target, port)) {
        Some(srcs) if srcs.len() == 1 => Some(srcs[0]),
        _ => None,
    }
}

/// Certified truthiness signature check + truthy/falsy resolution shared by
/// Select's `BSelectB` and Branch's `BCond`. Returns `None` if the condition
/// isn't resolvable yet (retry later in the fixpoint) or its signature was
/// never certified for this gate (permanent refusal).
fn resolve_condition(
    id: NodeId,
    cond_port: WirePort,
    gate_class: &'static str,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    table: &CertifiedTable,
) -> Option<bool> {
    let cond_val: Option<Value> = match resolve_input(id, cond_port, in_wires, known) {
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

/// `Select` shorting: `BSelectB` truthy -> `InputB`, falsy -> `InputA`. The
/// chosen port must have exactly one wire (an unwired chosen input is never
/// probed — refuse). Plans a `ShortCircuit` as soon as the condition
/// resolves, independent of whether the chosen source's own value is known
/// (an opaque source is still a valid short target); if that source IS
/// known, the Select's own entry in `known` is set too so its consumers can
/// keep folding in the SAME worklist pass.
fn try_resolve_select(
    id: NodeId,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let truthy = resolve_condition(id, WirePort::BSelectB, gc::SELECT, in_wires, known, table)?;
    let chosen_port = if truthy { WirePort::InputB } else { WirePort::InputA };
    let chosen_src = single_wire_source(id, chosen_port, in_wires)?;
    plan.insert(id, PlanAction::ShortCircuit { chosen_src });
    known.get(&chosen_src.node_id).cloned()
}

/// `Branch` truncation: `BCond` truthy -> taken `ExecOutA`, falsy -> taken
/// `ExecOutB`. Plans a `Truncate` carrying every current target of the
/// taken exec-out port (the untaken side's targets are left to the
/// exec-reachability sweep — their incoming wire simply disappears once the
/// Branch is removed). A Branch never has a scalar "value" of its own, so
/// this always reports `None` regardless of outcome.
fn try_resolve_branch(
    id: NodeId,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    out_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) {
    let Some(truthy) = resolve_condition(id, WirePort::BCond, gc::BRANCH, in_wires, known, table)
    else {
        return;
    };
    let taken_port = if truthy { WirePort::ExecOutA } else { WirePort::ExecOutB };
    let taken_targets = out_wires.get(&(id, taken_port)).cloned().unwrap_or_default();
    plan.insert(id, PlanAction::Truncate { taken_targets });
}

/// Try to resolve node `id` to a value given the `known` set so far. `_Literal`
/// and chip-boundary pass-through nodes just report their (already-settled)
/// value; a certified gate that resolves plans `BecomeLiteral` as a side
/// effect (recorded into `plan`) and reports the folded value too, so its own
/// consumers can chain off it in the same fixpoint. `Select`/`Branch` plan
/// their own structural rewrite (`ShortCircuit`/`Truncate`) instead.
fn try_resolve(
    id: NodeId,
    infos: &HashMap<NodeId, Info>,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    out_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let info = infos.get(&id)?;
    if info.nofold {
        return None;
    }

    // Already a constant — nothing to plan, just report its value.
    if info.gate_class == gc::LITERAL {
        return info.lit.clone();
    }

    // A portless `String_Concatenate` is `collect_infos`'s recognized
    // stand-in for a bare string literal (see the `Info.lit` comment there)
    // — report it the same way as `_Literal`: nothing to plan (it's already
    // in its canonical form), just make its value visible to `known` so
    // e.g. `"a" == "b"` can fold. `info.lit` is `None` for every OTHER
    // `String_Concatenate` (a real, wired `..`), so this is a no-op for
    // those — they fall through to the generic path below unaffected.
    if info.gate_class == gc::STRING_CONCATENATE && info.lit.is_some() {
        return info.lit.clone();
    }

    // Chip-instance boundary: a known value flows straight through. The
    // MicrochipInput/Output node itself is never folded (it's the visual
    // port marker), only used to carry `known` across the module split.
    if info.gate_class == gc::MICROCHIP_INPUT || info.gate_class == gc::MICROCHIP_OUTPUT {
        return match resolve_input(id, WirePort::RerInput, in_wires, known) {
            Resolved::Known(v) => Some(v),
            _ => None,
        };
    }

    if info.gate_class == gc::SELECT {
        return try_resolve_select(id, in_wires, known, table, plan);
    }

    if info.gate_class == gc::BRANCH {
        try_resolve_branch(id, in_wires, out_wires, known, table, plan);
        return None;
    }

    // Certified gate candidate: a real gate with exactly one data output.
    if info.kind != NodeKind::Gate || !info.single_output {
        return None;
    }

    let mut states: Vec<Resolved> = Vec::with_capacity(info.data_inputs.len());
    let mut all_resolved = true;
    for &port in &info.data_inputs {
        let st = resolve_input(id, port, in_wires, known);
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
        if table.covers(info.gate_class, &sig) {
            let eval_inputs: Vec<Option<Value>> = states
                .iter()
                .map(|s| match s {
                    Resolved::Unwired => None,
                    Resolved::Known(v) => Some(v.clone()),
                    Resolved::Unresolved => unreachable!("all_resolved excludes Unresolved"),
                })
                .collect();
            // `eval::eval` is this crate's pure certified-table lookup (see
            // fold/eval.rs) — a match over gate-class strings against
            // probed truth tables, NOT a dynamic code-execution eval.
            if let Some(v) = eval::eval(info.gate_class, &eval_inputs) {
                // Belt-and-suspenders: eval can return a non-finite float for
                // a covered signature (e.g. float `x % 0.0` -> NaN). Never
                // plan a fold that would bake a NaN/inf literal.
                let non_finite = matches!(&v, Value::Float(f) if !f.is_finite());
                if !non_finite {
                    plan.insert(id, PlanAction::BecomeLiteral(v.clone()));
                    return Some(v);
                }
            }
        }
    }

    // Annihilator fallback: LogicalAND short-circuits on a known `false`,
    // LogicalOR on a known `true`, on EITHER input — the other input may
    // stay opaque/unresolved forever (e.g. an `Opaque(...)` probe) without
    // blocking the fold.
    if let Some(kind) = table.annihilator(info.gate_class) {
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
                let ty = match value {
                    Value::Int(_) => Type::Int,
                    Value::Float(_) => Type::Float,
                    Value::Bool(_) => Type::Bool,
                    Value::Str(_) => Type::String,
                };
                let mut properties = HashMap::default();
                properties.insert(*sym::VALUE, value.to_literal());
                let node = module
                    .nodes
                    .get_mut(id)
                    .expect("node id came from module.nodes.keys()");
                node.gate_class = gc::LITERAL;
                node.properties = Arc::new(properties);
                node.ports = Arc::new(GateIO {
                    inputs: vec![],
                    outputs: vec![PortSpec {
                        name: *sym::OUTPUT,
                        ty,
                    }],
                });
                mutated = true;
            }
            Some(PlanAction::ShortCircuit { .. }) | Some(PlanAction::Truncate { .. }) => {
                module.nodes.remove(id);
                mutated = true;
            }
            None => {}
        }
    }

    // A folded/shorted/truncated node no longer has a matching operand-feed
    // wire, its original output port name (a folded node's is always
    // renamed to the canonical `_Literal` "Output" pin), or, for
    // Select/Branch, exists at all — fixed up wherever the wire happens to
    // live, not just in the planned node's own module (cross-scope wires
    // are stored in either endpoint's owning module).
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

    for child in module.chips.values_mut() {
        apply(child, plan);
    }
}

/// Rewrite one module's wire list per `plan`, looping to a local fixpoint so
/// a chain of removed nodes (e.g. a Select shorted straight into another
/// Select's chosen input, or a Branch truncated straight into another
/// Branch's exec-in) resolves correctly instead of leaving a wire that
/// references an already-deleted node. `BecomeLiteral` never removes a
/// node, so its rewrite is always stable on the first pass; only
/// `ShortCircuit`/`Truncate` chains need more than one round.
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
                    // ExecOutA/ExecOutB wires sourced from the branch drop —
                    // their targets were already re-homed via the
                    // target-side arm above (when this SAME branch was the
                    // wire's target on the OTHER end of the original
                    // exec-source wire), or they're the untaken side, whose
                    // targets the exec-reachability sweep will collect.
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

/// Exec-reachability cleanup: repeat to a fixpoint — a `NodeKind::Gate` node
/// declaring at least one `Type::Exec` input port whose TOTAL incoming exec
/// wire count (summed across every such port) is zero has lost its trigger;
/// remove it and every wire touching it. This eats a dead chain
/// transitively (each removal can strand its own successor). Events, chip
/// boundary IO (`Input`/`Output` kinds), Variables (no exec input port),
/// and `_nofold` nodes survive by construction — the total-across-all-ports
/// rule also keeps a `Union` alive as long as ANY of its several exec inputs
/// still has a wire (only `prune_dead_exec_unions` may splice/remove those).
fn sweep_dead_exec(root: &mut Module) {
    loop {
        let mut in_counts: HashMap<(NodeId, WirePort), usize> = HashMap::default();
        tally_incoming(root, &mut in_counts);
        let mut dead: HashSet<NodeId> = HashSet::default();
        collect_dead_exec(root, &in_counts, &mut dead);
        if dead.is_empty() {
            break;
        }
        remove_dead_nodes(root, &dead);
    }
}

fn tally_incoming(module: &Module, counts: &mut HashMap<(NodeId, WirePort), usize>) {
    for w in &module.wires {
        *counts.entry((w.target.node_id, w.target.port)).or_default() += 1;
    }
    for child in module.chips.values() {
        tally_incoming(child, counts);
    }
}

fn collect_dead_exec(
    module: &Module,
    in_counts: &HashMap<(NodeId, WirePort), usize>,
    dead: &mut HashSet<NodeId>,
) {
    for (id, n) in &module.nodes {
        if n.kind != NodeKind::Gate || n.properties.contains_key(&*sym::NO_FOLD) {
            continue;
        }
        // Rerouters (e.g. `Opaque`'s identity carrier) are currently
        // protected from this sweep only IMPLICITLY: a Rerouter declares no
        // `Type::Exec` input port at all, so the `peek().is_none()` check
        // below already skips it before the exec-trigger accounting runs.
        // Exclude the gate class explicitly too, so a future Rerouter
        // variant that DOES grow an Exec input port doesn't silently start
        // getting swept as "untriggered" by this pass.
        if n.gate_class == gc::REROUTER {
            continue;
        }
        let mut exec_ports = n.ports.inputs.iter().filter(|p| p.ty == Type::Exec).peekable();
        if exec_ports.peek().is_none() {
            continue;
        }
        let total: usize = exec_ports
            .map(|p| {
                in_counts
                    .get(&(*id, WirePort::from_name(resolve(p.name))))
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        if total == 0 {
            dead.insert(*id);
        }
    }
    for child in module.chips.values() {
        collect_dead_exec(child, in_counts, dead);
    }
}

/// Pure demand sweep: repeat to a fixpoint — a `NodeKind::Gate` node that is
/// a literal or a pure `Expr_*` gate, not `_nofold`, with zero outgoing
/// non-`Layout` wires has no consumer left; remove it and its incoming
/// wires. Rerouters/boundary nodes are never `Expr_*`-classed, so the
/// opaque-probe barrier is preserved automatically.
fn sweep_dead_pure(root: &mut Module) {
    loop {
        let mut out_counts: HashMap<NodeId, usize> = HashMap::default();
        tally_outgoing(root, &mut out_counts);
        let mut dead: HashSet<NodeId> = HashSet::default();
        collect_dead_pure(root, &out_counts, &mut dead);
        if dead.is_empty() {
            break;
        }
        remove_dead_nodes(root, &dead);
    }
}

fn tally_outgoing(module: &Module, counts: &mut HashMap<NodeId, usize>) {
    for w in &module.wires {
        if w.source.port != WirePort::Layout {
            *counts.entry(w.source.node_id).or_default() += 1;
        }
    }
    for child in module.chips.values() {
        tally_outgoing(child, counts);
    }
}

fn collect_dead_pure(
    module: &Module,
    out_counts: &HashMap<NodeId, usize>,
    dead: &mut HashSet<NodeId>,
) {
    for (id, n) in &module.nodes {
        if n.kind != NodeKind::Gate || n.properties.contains_key(&*sym::NO_FOLD) {
            continue;
        }
        let is_pure_class =
            n.gate_class == gc::LITERAL || n.gate_class.starts_with("BrickComponentType_WireGraph_Expr_");
        if !is_pure_class {
            continue;
        }
        if out_counts.get(id).copied().unwrap_or(0) == 0 {
            dead.insert(*id);
        }
    }
    for child in module.chips.values() {
        collect_dead_pure(child, out_counts, dead);
    }
}

/// Remove every id in `dead` (and any wire touching one) from `module` and
/// every nested chip.
fn remove_dead_nodes(module: &mut Module, dead: &HashSet<NodeId>) {
    module.nodes.retain(|id, _| !dead.contains(id));
    module
        .wires
        .retain(|w| !dead.contains(&w.source.node_id) && !dead.contains(&w.target.node_id));
    module.scope_captures.retain(|id| !dead.contains(id));
    for child in module.chips.values_mut() {
        remove_dead_nodes(child, dead);
    }
}

/// Whole-chip elision: bottom-up, remove a NAMED chip instance (a
/// `module.chips` entry) whose child module's `nodes` are entirely boundary
/// IO (or empty), none of that boundary IO carries a wire ANYWHERE in the
/// tree, and the chip node itself carries none of `NAME_LABEL`/
/// `CHIP_CLOSED`/`DOC_TEXT`/`_nofold` (the same annotations that keep an
/// empty ANON chip's shell alive through `emit::partition_anon_chips` —
/// they mean the (possibly-empty) shell must still reach emit). Recurses
/// bottom-up so a chip emptied by an inner elision can elide too.
///
/// Anon chips are NOT `module.chips` entries yet at this point in the
/// pipeline (`partition_anon_chips` runs after this whole pass) — their
/// elision is automatic: a fully-folded anon chip's tagged nodes are swept
/// by `sweep_dead_pure`/`sweep_dead_exec` above like any other dead nodes,
/// so partition never creates a child for them at all.
fn elide_empty_chips(root: &mut Module) {
    // A boundary node's wire could, in principle, live anywhere in the tree
    // (wires are stored in either endpoint's owning module) — computed once
    // up front against the untouched wire graph; later removals in this
    // same pass can only ever remove wires ON nodes we've already decided
    // to elide, which can't retroactively change another node's touch
    // status.
    let mut touched: HashSet<NodeId> = HashSet::default();
    collect_wire_endpoints(root, &mut touched);
    elide_empty_chips_rec(root, &touched);
}

fn collect_wire_endpoints(module: &Module, touched: &mut HashSet<NodeId>) {
    for w in &module.wires {
        touched.insert(w.source.node_id);
        touched.insert(w.target.node_id);
    }
    for child in module.chips.values() {
        collect_wire_endpoints(child, touched);
    }
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
