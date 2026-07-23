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
use crate::diagnostic::SourceRange;
use crate::intern::{intern_static, resolve, sym};
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{
    GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, ScopeId, Type, Wire,
};
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
    /// The node's own constant value, when it's already a `_Literal` (any
    /// certified variant, scalar or composite — `Value::from_literal`
    /// handles Vector/Rotator/Quat/Color/LinearColor uniformly, so a
    /// composite `_Literal` produced by AST-level `Vec(...)`-on-literals
    /// folding, OR by THIS pass's own `BecomeLiteral` action on a prior
    /// fixpoint round, seeds `known` exactly like a scalar one).
    lit: Option<Value>,
    /// `STRING_FORMAT_TEXT`'s `FormatString` property (the template text) —
    /// `None` for every other gate class, and also `None` for a FormatText
    /// node whose "format" argument was itself non-literal (wired as a real
    /// port instead of inlined as a property — never foldable, see
    /// `try_resolve_format_text`).
    format_string: Option<String>,
    /// The node's own raw properties — needed by `resolve_data_input` to
    /// read a call-argument literal that `lower/call.rs::
    /// literal_for_property_port` baked directly onto THIS node instead of
    /// wiring a separate `_Literal`/`Make*` source (the common shape for a
    /// composite, and some scalar, argument to a builtin CALL like
    /// `Dot(...)`/`ScaleVec(...)` — never used by `lower_binop`, which
    /// always wires a real literal source, so ordinary `a + b` expressions
    /// don't need this). An `Arc` clone, not a copy — cheap, mirrors
    /// `Node.properties`'s own representation.
    properties: Arc<crate::collections::HashMap<crate::intern::Sym, Literal>>,
    /// Data (non-`Exec`) input ports, in `ports.inputs` declaration order —
    /// exactly the slice `eval` expects.
    data_inputs: Vec<WirePort>,
    /// A foldable/propagatable node (literal, certified gate, or chip
    /// boundary) always has exactly one output port; anything else (Branch,
    /// Swap, multi-output gates) is left alone by this pass.
    single_output: bool,
    /// Placement metadata `cleanup_boundary_feeds`'s Rule B needs when it
    /// mints a fresh `_Literal` for a rewired boundary consumer: the new
    /// node is placed like the CONSUMER it feeds (same chip/row/scope), not
    /// like the boundary node it replaces as a source, so it renders where
    /// it's read rather than off in the chip the value originated from.
    chip_id: Option<NodeId>,
    chain_id: Option<u32>,
    scope_id: ScopeId,
    source_range: SourceRange,
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

    // `plan` alone under-counts "did anything resolve": a chip boundary
    // node's pass-through resolution (`try_resolve`'s MICROCHIP_INPUT/
    // MICROCHIP_OUTPUT case) and an already-existing `_Literal`/portless-
    // string-literal's own resolution (`info.lit.clone()`) both populate
    // `known` WITHOUT ever touching `plan` — e.g. `chip Inc(v: int) -> (r:
    // int) { out r = v }` called as `Inc(4)` (a bare-literal argument, no
    // arithmetic anywhere) resolves `v` and `r` end to end without a single
    // `BecomeLiteral`/`ShortCircuit`/`Truncate` action. `cleanup_boundary_feeds`
    // below needs exactly that case (a Known boundary node whose feed is
    // otherwise unreachable through `plan`), so the early-out must cover
    // both maps, not just `plan`.
    if plan.is_empty() && known.is_empty() {
        return;
    }

    // ---------------- apply ----------------
    apply(root, &plan);

    // ---------------- boundary-delivery cleanup ----------------
    // Two structural rules over chip-instance boundary nodes
    // (`MicrochipInput`/`MicrochipOutput`) that neither the fixpoint above
    // nor the sweeps below ever clean up on their own, because a boundary
    // node is NEVER itself a planned node — it's the chip's visual port
    // marker, kept in place even once a Known value has propagated straight
    // through it (see the MICROCHIP_INPUT/MICROCHIP_OUTPUT case in
    // `try_resolve`). Run before the sweeps (not after) so whatever either
    // rule orphans is still there to be collected by `sweep_dead_pure`/
    // `sweep_dead_exec` in the SAME pass, instead of surviving all the way
    // to `materialize_unfoldable_constants` (a separate call, made later in
    // `lower/mod.rs`, well outside this function) as a pointless
    // `MathAdd(n, 0)`-style carrier feeding a boundary port nobody reads.
    cleanup_boundary_feeds(root, &known, &infos);

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
        // `STRING_FORMAT_TEXT`'s template lives in the `FormatString`
        // property (`lower/ops.rs::build_format_text` / the `Fmt(...)`
        // builtin's literal-argument inlining — both use the same property
        // name), never as a wire input port — see `try_resolve_format_text`.
        let format_string = if n.gate_class == gc::STRING_FORMAT_TEXT {
            match n.properties.get(&intern_static("FormatString")) {
                Some(Literal::String(s)) => Some(s.clone()),
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
                format_string,
                properties: n.properties.clone(),
                data_inputs,
                single_output,
                chip_id: n.chip_id,
                chain_id: n.chain_id,
                scope_id: n.scope_id,
                source_range: n.source_range.clone(),
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

/// Like `resolve_input`, but for a port that may ALSO arrive as a baked,
/// wireless node property instead of a wire: an ordinary data input, a
/// Select/Branch condition, and a FormatText substitution slot are ALL
/// call-argument ports subject to the exact same baking (`Select`'s `cond`
/// and `Fmt`'s `a..g` are declared `CallParam`s exactly like any other
/// builtin's, so a bare-literal argument to any of them inlines the same
/// way — see `resolve_condition` and `try_resolve_format_text`, both of
/// which route through here too). An unwired port additionally falls back
/// to the node's own baked properties
/// (`lower/call.rs::literal_for_property_port`'s inlining — a composite, or
/// some scalar, CALL argument the emitted gate's data struct can carry
/// directly, with no separate `_Literal`/`Make*` source or wire at all).
/// Without this fallback, any certified gate reached via a CALL (as opposed
/// to a binary operator — `lower_binop` always wires a real literal source,
/// never bakes one) with such an argument is invisible to `known` and can
/// never fold, no matter how determined its value actually is — confirmed
/// via the `--fold-diff` fuzz harness (`Dot(Vec(1,0,0), Vec(0,1,0))` alone,
/// no chip/opaque involved, reproduces it) and fixed here rather than in the
/// harness, since the harness's OWN independent predictor (`predict()` in
/// `examples/fuzz_programs.rs`) already implements this exact fallback —
/// this driver was the one side actually missing it. (A follow-up review
/// pass found `resolve_condition`/`try_resolve_format_text` still on plain
/// `resolve_input` at the time — `Select(true, a, b)`'s baked `true` and
/// `Fmt(...)`'s baked slot arguments were the same gap, just not yet routed
/// through here.)
fn resolve_data_input(
    target: NodeId,
    port: WirePort,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    properties: &HashMap<crate::intern::Sym, Literal>,
) -> Resolved {
    match resolve_input(target, port, in_wires, known) {
        Resolved::Unwired => {
            match properties.get(&intern_static(port.as_str())).and_then(Value::from_literal) {
                Some(v) => Resolved::Known(v),
                None => Resolved::Unwired,
            }
        }
        other => other,
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
/// never certified for this gate (permanent refusal). Routes through
/// `resolve_data_input` (not plain `resolve_input`) so a bare-literal
/// condition argument — `Select(true, a, b)`'s `true` bakes onto `BSelectB`
/// exactly like any other call-argument literal, see that function's doc —
/// resolves instead of silently reading back as `Unwired`.
fn resolve_condition(
    id: NodeId,
    cond_port: WirePort,
    gate_class: &'static str,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    properties: &HashMap<crate::intern::Sym, Literal>,
    table: &CertifiedTable,
) -> Option<bool> {
    let cond_val: Option<Value> = match resolve_data_input(id, cond_port, in_wires, known, properties) {
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
    properties: &HashMap<crate::intern::Sym, Literal>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let truthy =
        resolve_condition(id, WirePort::BSelectB, gc::SELECT, in_wires, known, properties, table)?;
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
    properties: &HashMap<crate::intern::Sym, Literal>,
    table: &CertifiedTable,
    plan: &mut HashMap<NodeId, PlanAction>,
) {
    let Some(truthy) =
        resolve_condition(id, WirePort::BCond, gc::BRANCH, in_wires, known, properties, table)
    else {
        return;
    };
    let taken_port = if truthy { WirePort::ExecOutA } else { WirePort::ExecOutB };
    let taken_targets = out_wires.get(&(id, taken_port)).cloned().unwrap_or_default();
    plan.insert(id, PlanAction::Truncate { taken_targets });
}

/// `FormatText`'s substitution slots, `InputA..InputG`, in template-index
/// order (`{0}`..`{6}`) — mirrors `lower/ops.rs::FORMAT_SLOTS`.
const FORMAT_SLOTS: [WirePort; 7] = [
    WirePort::InputA, WirePort::InputB, WirePort::InputC,
    WirePort::InputD, WirePort::InputE, WirePort::InputF,
    WirePort::InputG,
];

/// `FormatText`: foldable when the node carries a `FormatString` property
/// (always true for `${...}`-interpolation lowering; a `Fmt(...)` call whose
/// template argument is itself non-literal wires `FormatString` as a real
/// port instead and carries no property here — never foldable) AND every
/// substituted slot (wired OR baked — see `resolve_data_input`'s doc: `Fmt`'s
/// `a..g` are ordinary `CallParam`s, so a bare-literal argument bakes onto
/// InputA..InputG with no wire at all, same as any other builtin CALL
/// argument) resolves to a known value. A slot that's neither wired nor
/// baked (no argument passed for it at all) does NOT block the fold —
/// certified by the probed `fmtUnwiredSlot` case (`Fmt("{0}{1}", Opaque(1))`
/// folds fine, rendering "0" for the unbound slot 1 — see
/// `eval::format_text`'s own unwired/out-of-range handling), so it's passed
/// through as `None` exactly like any other gate's unwired data input. A
/// wired-but-not-yet-known (or fan-in) slot returns `Resolved::Unresolved`,
/// which refuses the whole node for this round — the fixpoint retries it
/// once that source resolves.
///
/// Requires at least one slot to be genuinely SUBSTITUTED — wired OR baked,
/// not just declared (`Fmt(...)`'s optional `a..g` params are always
/// declared ports regardless of whether an argument was passed, see
/// `lower/call.rs::lower_builtin_call`) — before even attempting the fold. A
/// template with NO substituted slots at all (`Fmt("literal{a}brace")`, no
/// operands passed) is technically fully determined by the certified render
/// law too, but is left alone here deliberately: `probes/gate_semantics.ws`'s
/// `fmtLiteralBrace` is exactly this shape with zero `Opaque(...)` armor
/// (every OTHER FormatText probe case wires its substitution operand(s)
/// through `Opaque`, which never resolves — see the probe's own comment on
/// why), so folding a zero-substituted-slot template is the one way this
/// pass could visibly diverge `tests/fold_invariants.rs`'s structural-no-op
/// guarantee without ever "seeing through" an `Opaque` value. A BAKED slot
/// still satisfies this guard (unlike the zero-slot case, it means a real
/// argument was passed and the template genuinely substitutes): with no
/// argument at all, `fmtLiteralBrace` has neither a wire NOR a baked
/// property for any slot, so it's unaffected by counting baked slots here.
fn try_resolve_format_text(
    id: NodeId,
    info: &Info,
    in_wires: &HashMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &HashMap<NodeId, Value>,
    plan: &mut HashMap<NodeId, PlanAction>,
) -> Option<Value> {
    let template = info.format_string.as_ref()?;
    let any_substituted = FORMAT_SLOTS.iter().any(|&port| {
        in_wires.get(&(id, port)).is_some_and(|srcs| !srcs.is_empty())
            || info
                .properties
                .get(&intern_static(port.as_str()))
                .and_then(Value::from_literal)
                .is_some()
    });
    if !any_substituted {
        return None;
    }
    let mut inputs: Vec<Option<Value>> = Vec::with_capacity(FORMAT_SLOTS.len());
    for &port in &FORMAT_SLOTS {
        match resolve_data_input(id, port, in_wires, known, &info.properties) {
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

    // `FormatText`'s template lives in a PROPERTY, not a wire input — the
    // certified table's only recorded signature for this gate is the
    // synthetic `[Tmpl]` marker (see `table::InVariant::Tmpl`'s doc
    // comment), which no real `Value`-derived signature can ever match, so
    // the generic `table.covers` path below would refuse this gate
    // unconditionally regardless of how `info.data_inputs` are wired.
    // Special-cased: resolve each substitution slot through the SAME
    // wire-propagation machinery as any other gate's data inputs, then
    // substitute via the certified `eval::format_text` law directly.
    if info.gate_class == gc::STRING_FORMAT_TEXT {
        return try_resolve_format_text(id, info, in_wires, known, plan);
    }

    if info.gate_class == gc::SELECT {
        return try_resolve_select(id, in_wires, known, &info.properties, table, plan);
    }

    if info.gate_class == gc::BRANCH {
        try_resolve_branch(id, in_wires, out_wires, known, &info.properties, table, plan);
        return None;
    }

    // Certified gate candidate: a real gate with exactly one data output.
    if info.kind != NodeKind::Gate || !info.single_output {
        return None;
    }

    let mut states: Vec<Resolved> = Vec::with_capacity(info.data_inputs.len());
    let mut all_resolved = true;
    for &port in &info.data_inputs {
        let st = resolve_data_input(id, port, in_wires, known, &info.properties);
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

/// The `(properties, ports)` a `_Literal` node carrying `value` must have —
/// shared by `apply`'s in-place `BecomeLiteral` conversion (an EXISTING
/// node becomes a literal) and `cleanup_boundary_feeds`'s Rule B (a BRAND
/// NEW node is minted as a literal), so the two never drift apart.
///
/// Task-3 minimal deviation from that task's "table.rs + eval.rs only"
/// scope: `Value` (fold/eval.rs) grew four composite variants (Vector/
/// Rotator/Quat/Color) there, and this match must stay exhaustive over the
/// FULL enum for the crate to compile at all — there is no way to add
/// those variants without touching every exhaustive `match value` site,
/// and this is the only one in the crate (verified via `grep -rn
/// "Value::" src/`; `fuzz_programs.rs`'s uses are all `matches!`/tuple
/// patterns, not exhaustive matches).
fn literal_properties_and_ports(
    value: &Value,
) -> (
    Arc<crate::collections::HashMap<crate::intern::Sym, Literal>>,
    Arc<GateIO>,
) {
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
    (
        Arc::new(properties),
        Arc::new(GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty,
            }],
        }),
    )
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

/// Every `MicrochipInput`/`MicrochipOutput` id that belongs to a NAMED CHIP
/// INSTANCE's own boundary — i.e. a `module.chips[...]` child's
/// `inputs`/`outputs` list, recursively (a chip calling another chip nests
/// arbitrarily deep). Deliberately does NOT include `root`'s own top-level
/// `inputs`/`outputs`: the ROOT script's `in`/`out` declarations use the
/// EXACT SAME node shape (same gate classes, same `add_input`/`add_output`
/// builder calls — see `ir/build.rs`) but interface with the OUTSIDE
/// WORLD, not a caller/callee this pass fully controls. A root-level `out`
/// is terminal BY DESIGN (nothing in this compiled module ever wires FROM
/// it) — so "zero outgoing wires" is the ORDINARY, expected shape for one,
/// never evidence it's dead. Only excluding the caller's own top-level
/// `module.inputs`/`module.outputs` (and recursing into `module.chips`
/// instead of also collecting `module`'s own list) keeps the two shapes
/// apart: this fires once per RECURSION STEP on the CHILD's lists, never
/// on the `module` passed in at any given call.
fn collect_call_boundary_ids(module: &Module, ids: &mut HashSet<NodeId>) {
    for child in module.chips.values() {
        ids.extend(child.inputs.iter().copied());
        ids.extend(child.outputs.iter().copied());
        collect_call_boundary_ids(child, ids);
    }
}

/// Boundary-delivery cleanup: Rule B, then Rule A (order matters — Rule B
/// routinely creates the very "zero outgoing wires" state Rule A looks
/// for, on the `Output` side in particular; see each rule's own doc for
/// why). Both rules act only on a chip-instance CALL boundary node — a
/// `MicrochipInput`/`MicrochipOutput` belonging to a NAMED CHIP INSTANCE's
/// own child module (`collect_call_boundary_ids` below) — whose id is ALSO
/// a key in `known`; that's the SAME barrier `info.nofold` already
/// enforces one level up (a `_nofold` node's `try_resolve` returns `None`
/// before ever reaching the pass-through case, so it can never enter
/// `known` in the first place — `rewire_boundary_consumers` asserts this
/// instead of just assuming it).
fn cleanup_boundary_feeds(root: &mut Module, known: &HashMap<NodeId, Value>, infos: &HashMap<NodeId, Info>) {
    let mut boundary_ids: HashSet<NodeId> = HashSet::default();
    collect_call_boundary_ids(root, &mut boundary_ids);
    if boundary_ids.is_empty() {
        return;
    }

    rewire_boundary_consumers(root, known, infos, &boundary_ids);

    // Recomputed fresh, globally, AFTER Rule B: a wire counted here could
    // have lived anywhere in the tree, and Rule B may just have removed the
    // last one an `Output`-kind node had (an `Input`-kind node can also
    // reach zero on its own, purely from the main fixpoint above — see
    // `rewire_boundary_consumers`'s doc).
    let mut outgoing: HashMap<NodeId, usize> = HashMap::default();
    tally_boundary_outgoing(root, &boundary_ids, &mut outgoing);
    let dead_feeds: HashSet<NodeId> = boundary_ids
        .into_iter()
        .filter(|id| known.contains_key(id) && outgoing.get(id).copied().unwrap_or(0) == 0)
        .collect();
    if !dead_feeds.is_empty() {
        drop_boundary_feeds(root, &dead_feeds);
    }
}

/// Rule B: for every wire whose SOURCE is a Known chip-boundary node,
/// splice in a fresh `_Literal` carrying that value and re-home the wire
/// onto it, bypassing the boundary node's rerouter wire entirely. The new
/// literal is inserted into whichever module currently owns the wire being
/// rewritten — the same "wherever the wire happens to live" convention
/// `apply`'s own wire-fixup above already relies on for cross-scope wires —
/// so it lands same-module as the wire it replaces, and typically
/// same-module as the consumer it feeds; placement metadata (chip/row/
/// scope) is copied from the CONSUMER (via `infos`), not the boundary node,
/// so the new literal renders where it's read.
///
/// Safe for every surviving consumer, not just the "uncertified" ones this
/// rule exists for: any wire still sourced from a Known boundary node at
/// this point already survived the ENTIRE fixpoint above — a certified
/// consumer whose other inputs were also known would already have folded
/// itself there (dropping this very wire via `rewrite_wires`, same as any
/// other `BecomeLiteral` target), so whatever's left here is either a gate
/// `try_resolve` never attempts at all (not in the certified table — a
/// `DisplayText` field, an entity-call argument, an array method's index,
/// ...) or a certified gate still waiting on some OTHER not-yet-known
/// input. Either way, handing it the value directly is behaviorally
/// identical to the live rerouter wire it replaces — just no longer routed
/// through a dataless boundary port that later forces
/// `materialize_unfoldable_constants` to fabricate a carrier gate nobody
/// upstream of it actually needed.
fn rewire_boundary_consumers(
    module: &mut Module,
    known: &HashMap<NodeId, Value>,
    infos: &HashMap<NodeId, Info>,
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
        debug_assert!(
            !infos.get(&w.source.node_id).is_some_and(|i| i.nofold),
            "cleanup_boundary_feeds: a _nofold boundary node must never enter `known` \
             (try_resolve's info.nofold check must return None before the pass-through case)"
        );
        let meta = infos
            .get(&w.target.node_id)
            .expect("wire target must have been snapshotted by collect_infos");
        let (properties, ports) = literal_properties_and_ports(value);
        let lit_id = NodeId::fresh();
        module.nodes.insert(
            lit_id,
            Node {
                id: lit_id,
                kind: NodeKind::Gate,
                gate_class: gc::LITERAL,
                properties,
                ports,
                source_range: meta.source_range.clone(),
                chip_id: meta.chip_id,
                chain_id: meta.chain_id,
                scope_id: meta.scope_id,
                note: Some("boundary-delivery cleanup literal"),
            },
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
        rewire_boundary_consumers(child, known, infos, boundary_ids);
    }
}

fn tally_boundary_outgoing(
    module: &Module,
    boundary_ids: &HashSet<NodeId>,
    out: &mut HashMap<NodeId, usize>,
) {
    for w in &module.wires {
        if boundary_ids.contains(&w.source.node_id) {
            *out.entry(w.source.node_id).or_default() += 1;
        }
    }
    for child in module.chips.values() {
        tally_boundary_outgoing(child, boundary_ids, out);
    }
}

/// Rule A: a boundary node with zero remaining outgoing wires anywhere in
/// the tree has nothing left to deliver to — drop its own incoming feed
/// wire(s) too, wherever THEY live, so the now-orphaned source (a literal,
/// or the real gate chain that produced one via `BecomeLiteral`) demand-
/// sweeps via the ordinary `sweep_dead_pure`/`sweep_dead_exec` passes right
/// after `cleanup_boundary_feeds` returns, instead of surviving all the way
/// to `materialize_unfoldable_constants` as a `MathAdd(n, 0)`-style carrier
/// feeding a port nobody reads. Applies uniformly to `MicrochipInput` (an
/// argument the callee's body never ends up reading once folded) and
/// `MicrochipOutput` (a result nothing outside the chip ends up reading —
/// typically only zero-outgoing as a RESULT of Rule B above, since an
/// `Output` node's exterior consumers are what Rule B just finished
/// rewiring away). The boundary node ITSELF is never removed — it's the
/// chip's visual port marker; only its feed wire goes.
fn drop_boundary_feeds(module: &mut Module, dead_feeds: &HashSet<NodeId>) {
    let before = module.wires.len();
    module.wires.retain(|w| {
        !(dead_feeds.contains(&w.target.node_id) && w.target.port == WirePort::RerInput)
    });
    if module.wires.len() != before {
        module.template_key = None;
    }
    for child in module.chips.values_mut() {
        drop_boundary_feeds(child, dead_feeds);
    }
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
