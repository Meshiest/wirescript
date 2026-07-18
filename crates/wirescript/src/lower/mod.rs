//! Lowering: AST + typecheck annotations → IR Module.
//!
//! Strategy: walk each top-level declaration. Vars and I/O become IR
//! nodes with deterministic names. Handlers become one event node feeding
//! an exec chain; statements thread a `current_exec` PortRef through each
//! step. Expressions produce gate nodes whose value output is threaded
//! into their consumer.

use crate::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{ChipDecl, *};
use crate::catalog::calls::find_call;
use crate::catalog::events::find_event;
use crate::catalog::operators::OpRule;
use crate::diagnostic::{Diagnostic, SourceRange};
use crate::intern::{intern, intern_static, sym};
use crate::ir::build::{AddNodeOpts, IdAllocator, ModuleBuilder, port_ref};
use crate::ir::gate_class as gc;
use crate::ir::{
    GateIO, Literal, Module, NodeId, NodeKind, PortRef, PortSpec, ROOT_SCOPE_ID, ScopeId,
    ScopeInfo, ScopeKind, Type, port_registry::WirePort,
};
use crate::template_cache::TemplateCache;
use crate::typecheck::TypeCheckResult;

mod context;
use context::*;

mod predeclare;
pub use predeclare::expr_to_literal;
use predeclare::*;

mod decl;
use decl::*;

mod handler;
use handler::*;

mod stmt;
use stmt::*;

mod expr;
use expr::*;

mod ops;
use ops::*;

mod call;
use call::*;

mod access;
use access::*;

// ---------- result ----------

pub struct LowerResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct LowerInput<'a> {
    pub ast: &'a Script,
    pub type_of_expr: &'a HashMap<(Arc<str>, usize, usize), Type>,
    pub op_resolutions: &'a HashMap<(Arc<str>, usize, usize), OpRule>,
    pub file: &'a str,
    pub module_name: Option<&'a str>,
    pub template_cache: Arc<TemplateCache>,
    pub doc_comments: &'a HashMap<usize, String>,
}

pub fn lower(input: LowerInput<'_>) -> LowerResult {
    let ids = IdAllocator::default();
    // Root module name is on the top-level chip's emitted text label.
    // Explicit `module_name` wins; otherwise use the entry file's stem.
    let file_stem = std::path::Path::new(input.file)
        .file_stem()
        .and_then(|s| s.to_str());
    let builder = ModuleBuilder::new(input.module_name.or(file_stem).unwrap_or("main"));
    let diagnostics: Vec<Diagnostic> = Vec::new();

    let mut ctx = LowerCtx {
        builder,
        ids,
        diagnostics,
        type_of_expr: input.type_of_expr,
        op_resolutions: input.op_resolutions,
        file: input.file.to_string(),
        scope: crate::scope::Scope::new(),
        handler_end_execs: Vec::new(),
        current_exec: None,
        handler_entry_exec: None,
        captured_events: HashMap::default(),
        next_chain_id: 0,
        current_anon_chip: None,
        mod_return_exec: None,
        mod_return_var: None,
        type_aliases: {
            let mut m = HashMap::default();
            for d in &input.ast.decls {
                if let crate::ast::TopDecl::TypeAlias(ta) = d {
                    m.insert(ta.name.clone(), ta.typ.clone());
                }
            }
            m
        },
        pending_emits: HashMap::default(),
        exec_signal_hubs: HashMap::default(),
        exec_signal_keys: HashMap::default(),
        next_scope_id: ROOT_SCOPE_ID + 1,
        template_cache: input.template_cache.clone(),
        await_armed_port: None,
        signal_awaits: HashMap::default(),
        exec_branch_depth: 0,
        exec_signal_payloads: HashMap::default(),
        pending_inline_record: None,
        pending_return_record: None,
        chip_call_stack: Vec::new(),
        known_fn_names: Arc::new(collect_fn_names(input.ast)),
        is_root_module: true,
        doc_comments: input.doc_comments,
    };

    // Pass 1: register I/O + vars + buffers.
    for d in &input.ast.decls {
        pre_declare_decl(&mut ctx, d);
    }
    // Pass 2: lower bodies.
    for d in &input.ast.decls {
        // Flush handler end execs before non-handler declarations so that
        // code after `on` blocks chains from the combined handler exits.
        // Anon chips whose body is a single handler count as handlers for
        // this purpose — they're just visually grouped handlers.
        if !ctx.handler_end_execs.is_empty() && !is_handler_like(d) {
            flush_handler_end_execs(&mut ctx);
        }
        lower_decl(&mut ctx, d);
    }

    flush_pending_emits(&mut ctx);

    let ids_unused = ctx.ids; // move consumed
    let _ = ids_unused;
    let mut module = ctx.builder.module;
    prune_dead_exec_unions(&mut module);
    // Before literal-inlining/materialization: a real computation still has its
    // operand WIRES here, so only genuinely wireless orphans (e.g. a top-level
    // constant duplicated by a namespace + named import of the same module) are
    // pruned — not a user's connected-but-unused `let y = x * 2`.
    prune_dead_pure_gates(&mut module, false);
    materialize_unfoldable_constants(&mut module);
    inline_orphan_literals(&mut module);
    // Folding an operand into its consumer can leave a bare `_Literal` wired to
    // nothing; sweep those literal orphans too (always safe — a literal has no
    // inputs, so it can never be a user computation whose operands were inlined).
    prune_dead_pure_gates(&mut module, true);
    // Merge the constant gates that survived inlining (a `"PREFIX: " ..` concat
    // wrapper repeated per use, a multi-consumer literal) so an identical
    // constant is emitted once and fans out, per chip.
    dedup_constant_gates(&mut module);
    crate::emit::partition_anon_chips(&mut module);
    LowerResult {
        module,
        diagnostics: ctx.diagnostics,
    }
}

/// Collect every chip/mod name declared anywhere in the (import-merged) program
/// — top-level and nested. Lets lowering tell a use-before-declaration call
/// (a real chip/mod, just registered later → WS021) from a call to a name that
/// is not a function at all (e.g. an unimplemented builtin → placeholder).
fn collect_fn_names(ast: &Script) -> HashSet<String> {
    fn walk_block(block: &Block, names: &mut HashSet<String>) {
        for s in &block.stmts {
            match s {
                Stmt::ChipDecl(c) => {
                    names.insert(c.name.clone());
                    walk_block(&c.body, names);
                }
                Stmt::AnonChip(ac) => walk_block(&ac.body, names),
                Stmt::Handler(h) => walk_block(&h.body, names),
                Stmt::If(i) => {
                    walk_block(&i.then_block, names);
                    if let Some(eb) = &i.else_block {
                        walk_block(eb, names);
                    }
                }
                _ => {}
            }
        }
    }
    let mut names = HashSet::default();
    for d in &ast.decls {
        match d {
            TopDecl::Chip(c) => {
                names.insert(c.name.clone());
                walk_block(&c.body, &mut names);
            }
            TopDecl::AnonChip(ac) => walk_block(&ac.body, &mut names),
            TopDecl::Handler(h) => walk_block(&h.body, &mut names),
            _ => {}
        }
    }
    names
}

/// Constant `Vec/Rotation/Color` calls lower to `_Literal` nodes so consumers
/// inline them as component data. That only works for sinks that store the
/// value as a wire-variant data field; for every other consumer — entity
/// gates whose struct-typed inputs must be wired, `Split*` inputs, chip IO,
/// unmapped gates — this pass materializes the real `Make*` gate (component
/// values baked into its data struct) and re-points those wires at it, so a
/// folded constant is never silently dropped. Recurses into chip sub-modules.
fn materialize_unfoldable_constants(module: &mut Module) {
    use crate::ir::{Node, NodeId, NodeKind, PortRef, PortSpec};
    let value_sym = *sym::VALUE;
    let mut make_nodes: Vec<Node> = Vec::new();
    let mut make_for: HashMap<NodeId, NodeId> = HashMap::default();
    let mut rewires: Vec<(usize, NodeId)> = Vec::new();

    for (i, w) in module.wires.iter().enumerate() {
        let Some(src) = module.nodes.get(&w.source.node_id) else {
            continue;
        };
        if src.gate_class != gc::LITERAL {
            continue;
        }
        let recipe = match src.properties.get(&value_sym) {
            Some(Literal::Vector { x, y, z }) => Some((
                gc::MAKE_VECTOR,
                Type::Vector,
                vec![
                    (WirePort::X, Literal::Float(*x)),
                    (WirePort::Y, Literal::Float(*y)),
                    (WirePort::Z, Literal::Float(*z)),
                ],
            )),
            Some(Literal::Rotator { pitch, yaw, roll }) => Some((
                gc::MAKE_ROTATION,
                Type::Rotator,
                vec![
                    (WirePort::Pitch, Literal::Float(*pitch)),
                    (WirePort::Yaw, Literal::Float(*yaw)),
                    (WirePort::Roll, Literal::Float(*roll)),
                ],
            )),
            Some(Literal::LinearColor { r, g, b, a }) => Some((
                gc::MAKE_COLOR,
                Type::Color,
                vec![
                    (WirePort::R, Literal::Float(*r)),
                    (WirePort::G, Literal::Float(*g)),
                    (WirePort::B, Literal::Float(*b)),
                    (WirePort::A, Literal::Float(*a)),
                ],
            )),
            _ => None,
        };
        let Some((gate_class, out_ty, fields)) = recipe else {
            continue;
        };
        let target_ok = module
            .nodes
            .get(&w.target.node_id)
            .is_some_and(|t| crate::emit::port_accepts_inline_variant(t.gate_class, w.target.port));
        if target_ok {
            continue;
        }
        let make_id = *make_for.entry(w.source.node_id).or_insert_with(|| {
            let id = NodeId::fresh();
            let properties: HashMap<crate::intern::Sym, Literal> = fields
                .iter()
                .map(|(port, lit)| (intern(port.as_str()), lit.clone()))
                .collect();
            make_nodes.push(Node {
                id,
                kind: NodeKind::Gate,
                gate_class,
                properties: std::sync::Arc::new(properties),
                ports: std::sync::Arc::new(GateIO {
                    inputs: vec![],
                    outputs: vec![PortSpec {
                        name: *sym::OUTPUT,
                        ty: out_ty.clone(),
                    }],
                }),
                source_range: src.source_range.clone(),
                chip_id: src.chip_id,
                chain_id: src.chain_id,
                scope_id: src.scope_id,
                note: Some("materialized constant"),
            });
            id
        });
        rewires.push((i, make_id));
    }

    for n in make_nodes {
        module.nodes.insert(n.id, n);
    }
    for (i, make_id) in rewires {
        module.wires[i].source = PortRef {
            node_id: make_id,
            port: WirePort::Output,
        };
    }
    for child_module in module.chips.values_mut() {
        materialize_unfoldable_constants(child_module);
    }
}

/// Fold standalone `_Literal` bricks whose value is only used once into a
/// property on the consumer gate, then delete the literal. Avoids having
/// rows of constant-value bricks cluttering the chip for things like
/// `n + 1`, `n > 10`, etc. Recurses into chip sub-modules.
fn inline_orphan_literals(module: &mut Module) {
    let value_sym = *sym::VALUE;
    // Per-source (count, first target): the single-consumer check only ever
    // needs the one target wire, so no per-node target Vecs. Buffers are
    // reused across fixpoint iterations.
    let mut outgoing: HashMap<NodeId, (usize, PortRef)> =
        HashMap::with_capacity_and_hasher(module.nodes.len(), Default::default());
    let mut incoming_count: HashMap<NodeId, usize> =
        HashMap::with_capacity_and_hasher(module.nodes.len(), Default::default());
    let mut lit_ids: Vec<NodeId> = Vec::new();
    let mut concat_ids: Vec<NodeId> = Vec::new();
    let mut removed: HashSet<NodeId> = HashSet::default();
    // Built once, then maintained incrementally: a fold only ever removes the
    // folded node and its single outgoing wire, so surviving nodes' entries
    // never change except the target's incoming count.
    for w in &module.wires {
        let e = outgoing.entry(w.source.node_id).or_insert((0, w.target));
        e.0 += 1;
        *incoming_count.entry(w.target.node_id).or_default() += 1;
    }
    loop {
        lit_ids.clear();
        lit_ids.extend(
            module
                .nodes
                .iter()
                .filter(|(_, n)| n.gate_class == gc::LITERAL)
                .map(|(id, _)| *id),
        );
        let mut changed = false;
        for &lit_id in &lit_ids {
            let Some(&(out_len, target)) = outgoing.get(&lit_id) else {
                continue;
            };
            if out_len != 1 {
                continue;
            }
            if incoming_count.get(&lit_id).copied().unwrap_or(0) != 0 {
                continue;
            }
            let (target_id, target_port) = (target.node_id, target.port);
            let value = match module
                .nodes
                .get(&lit_id)
                .and_then(|n| n.properties.get(&value_sym).cloned())
            {
                Some(v) => v,
                None => continue,
            };
            // Convert PortIndex → Sym for use as a property key
            let target_port_sym = intern(target_port.as_str());
            if let Some(target) = module.nodes.get_mut(&target_id) {
                std::sync::Arc::make_mut(&mut target.properties)
                    .entry(target_port_sym)
                    .or_insert(value);
            }
            module.nodes.remove(&lit_id);
            removed.insert(lit_id);
            outgoing.remove(&lit_id);
            if let Some(c) = incoming_count.get_mut(&target_id) {
                *c = c.saturating_sub(1);
            }
            changed = true;
        }

        // Fold pure constant-string `String_Concatenate` wrappers (the legacy
        // way a string literal became a wire, before inline wire-variant
        // support) into consumers that accept an inline string variant. Unlike
        // `_Literal`, this is gated on `port_accepts_inline_variant` — a string
        // can't fill a wire-only port, so those keep the real concat gate.
        concat_ids.clear();
        concat_ids.extend(
            module
                .nodes
                .iter()
                .filter(|(id, n)| n.gate_class == gc::STRING_CONCATENATE && !removed.contains(id))
                .map(|(id, _)| *id),
        );
        for &cid in &concat_ids {
            if incoming_count.get(&cid).copied().unwrap_or(0) != 0 {
                continue;
            }
            let Some(&(1, target)) = outgoing.get(&cid) else {
                continue;
            };
            let (target_id, target_port) = (target.node_id, target.port);
            // Only a single constant string (INPUT_A set, INPUT_B + Separator
            // empty) — real 2-input concats have wired inputs (incoming != 0).
            let text = {
                let Some(node) = module.nodes.get(&cid) else {
                    continue;
                };
                let Some(Literal::String(text)) = node.properties.get(&*sym::INPUT_A).cloned()
                else {
                    continue;
                };
                let is_empty = |k| match node.properties.get(&k) {
                    None => true,
                    Some(Literal::String(s)) => s.is_empty(),
                    _ => false,
                };
                if !is_empty(*sym::INPUT_B) || !is_empty(intern("Separator")) {
                    continue;
                }
                text
            };
            let accepts = module.nodes.get(&target_id).is_some_and(|t| {
                crate::emit::port_accepts_inline_variant(t.gate_class, target_port)
            });
            if !accepts {
                continue;
            }
            let target_port_sym = intern(target_port.as_str());
            if let Some(t) = module.nodes.get_mut(&target_id) {
                std::sync::Arc::make_mut(&mut t.properties)
                    .entry(target_port_sym)
                    .or_insert(Literal::String(text));
            }
            module.nodes.remove(&cid);
            removed.insert(cid);
            outgoing.remove(&cid);
            if let Some(c) = incoming_count.get_mut(&target_id) {
                *c = c.saturating_sub(1);
            }
            changed = true;
        }

        if !changed {
            break;
        }
    }
    // Removed nodes had no incoming wires, so only source-side wires can
    // reference them — one sweep at the end covers every iteration's folds.
    if !removed.is_empty() {
        module
            .wires
            .retain(|w| !removed.contains(&w.source.node_id));
    }
    for child_module in module.chips.values_mut() {
        inline_orphan_literals(child_module);
    }
}

/// Merge structurally-identical constant gates within each chip so a repeated
/// constant is emitted once and fans out to its consumers, instead of once per
/// use (e.g. a `"PREFIX: " .. name` chip that repeats `"PREFIX: "` — a constant
/// `String_Concatenate` wrapper — on every line, or a multi-consumer `_Literal`
/// that inlining left behind).
///
/// A "constant" here is a pure gate (`_Literal` or `Expr_*`) with NO incoming
/// data wire: its output is then fully determined by its config properties, so
/// any two with the same gate class + properties are interchangeable. `Random`
/// is exec-classed (never `Expr_*`), and the stateful edge/change detectors
/// always have a wired input, so neither is ever a candidate.
///
/// Dedup is per-module: a constant lives in the chip that uses it, so merging
/// within each module avoids rewiring across microchip boundaries. Node ids are
/// globally unique, so redirecting a dropped dup's consumers is a tree-wide wire
/// rewrite. Recurses into chip sub-modules.
fn dedup_constant_gates(root: &mut Module) {
    // Canonical key for a constant gate: class + its properties (sorted, so the
    // key is order-independent). `Debug` on `Literal` is deterministic per value.
    fn const_key(n: &crate::ir::Node) -> String {
        let mut kv: Vec<(&str, String)> = n
            .properties
            .iter()
            .map(|(k, v)| (crate::intern::resolve(*k), format!("{v:?}")))
            .collect();
        kv.sort_unstable();
        format!("{}\u{1}{kv:?}", n.gate_class)
    }

    // dup node id -> keeper node id (same class ⇒ same output ports, so each
    // consumer keeps its own `source.port` and only the node id changes).
    let mut redirect: HashMap<NodeId, NodeId> = HashMap::default();

    fn collect(module: &mut Module, redirect: &mut HashMap<NodeId, NodeId>) {
        let has_incoming: HashSet<NodeId> = module
            .wires
            .iter()
            .filter(|w| w.target.port != WirePort::Layout)
            .map(|w| w.target.node_id)
            .collect();
        // Key by (owning anon-chip, const_key): this pass runs BEFORE
        // `partition_anon_chips` splits each `chip_id`-tagged group into its own
        // emit module, so merging across `chip_id` tags would redirect a
        // consumer's wire to a keeper that lands in a DIFFERENT chip. Partition
        // keeps such a cross-chip data wire in the parent without cloning the
        // literal into the child (only parent→chip literals are cloned), so
        // emit can't inline it and the operand silently reads its port default
        // (0). Deduping within a chip_id group keeps every keeper in the same
        // future module as its consumers.
        let mut groups: HashMap<(Option<NodeId>, String), Vec<NodeId>> = HashMap::default();
        for (id, n) in &module.nodes {
            let is_pure = n.gate_class == gc::LITERAL
                || n.gate_class
                    .starts_with("BrickComponentType_WireGraph_Expr_");
            if n.kind == NodeKind::Gate && is_pure && !has_incoming.contains(id) {
                groups.entry((n.chip_id, const_key(n))).or_default().push(*id);
            }
        }
        for mut group in groups.into_values() {
            if group.len() < 2 {
                continue;
            }
            group.sort_unstable(); // deterministic keeper (lowest id)
            let keeper = group[0];
            for dup in &group[1..] {
                redirect.insert(*dup, keeper);
                module.nodes.remove(dup);
            }
        }
        for child in module.chips.values_mut() {
            collect(child, redirect);
        }
    }

    collect(root, &mut redirect);
    if redirect.is_empty() {
        return;
    }

    fn apply(module: &mut Module, redirect: &HashMap<NodeId, NodeId>) {
        module.wires.retain_mut(|w| {
            if let Some(&keeper) = redirect.get(&w.source.node_id) {
                w.source.node_id = keeper;
            }
            // A removed dup has no incoming data wire, so it can only appear as a
            // target via a (not-yet-created) Layout edge — drop such a wire.
            !redirect.contains_key(&w.target.node_id)
        });
        // Redirection can collapse two wires onto the same source→target pair.
        let mut seen: HashSet<(PortRef, PortRef)> = HashSet::default();
        module.wires.retain(|w| seen.insert((w.source, w.target)));
        // A dropped dup referenced as an external capture becomes its keeper.
        if !module.scope_captures.is_empty() {
            let mut seen_caps = HashSet::default();
            module.scope_captures = module
                .scope_captures
                .iter()
                .map(|id| *redirect.get(id).unwrap_or(id))
                .filter(|id| seen_caps.insert(*id))
                .collect();
        }
        for child in module.chips.values_mut() {
            apply(child, redirect);
        }
    }
    apply(root, &redirect);
}

/// Remove pure, side-effect-free expression gates that are fully disconnected —
/// no data wire touches them on either side.
///
/// Such orphans arise when a module is imported via BOTH a namespace
/// (`import * as x`) and a named import: the named path materializes a top-level
/// `let` wired to its consumers, while the namespace path materializes the SAME
/// `let` again (every importable decl is carried in the namespace), and that copy
/// is referenced by nothing — so a constant ships as a gate wired to nothing.
/// `inline_orphan_literals` only folds literals with exactly one consumer, so a
/// zero-connection orphan slips through. Dropping these changes no behavior.
///
/// The criterion is deliberately "no data wires at all", NOT "output feeds
/// nothing": a user's connected-but-unused computation (`let y = x * 2` with `y`
/// unused) still has input wires, and this project's cleanups prune only
/// compiler-generated cruft, never user computations. A duplicated top-level
/// constant has neither inputs (it's a literal) nor outputs, so it is caught
/// while `let y = x * 2` is left intact.
///
/// Connectivity is computed across the WHOLE module tree at once: a gate in one
/// module can be wired to a node in another via a cross-scope wire, so the
/// per-module recursion the neighbouring cleanups use would wrongly drop a gate
/// wired only to a sibling/child module.
///
/// `literals_only` restricts pruning to `_Literal` gates — used for the pass run
/// AFTER literal-inlining, where an `Expr_*` gate can be wireless simply because
/// its operands were folded into properties (a real, if unused, computation),
/// whereas a wireless literal is always a dead constant.
fn prune_dead_pure_gates(module: &mut Module, literals_only: bool) {
    let is_pure = |gate_class: &str| -> bool {
        gate_class == gc::LITERAL
            || (!literals_only && gate_class.starts_with("BrickComponentType_WireGraph_Expr_"))
    };
    // Node ids touched by a real (non-Layout) data wire, as source OR target.
    fn collect_connected(module: &Module, connected: &mut HashSet<NodeId>) {
        for w in &module.wires {
            if w.source.port != WirePort::Layout {
                connected.insert(w.source.node_id);
            }
            if w.target.port != WirePort::Layout {
                connected.insert(w.target.node_id);
            }
        }
        for child in module.chips.values() {
            collect_connected(child, connected);
        }
    }
    fn collect_orphans(
        module: &Module,
        connected: &HashSet<NodeId>,
        is_pure: &impl Fn(&str) -> bool,
        dead: &mut HashSet<NodeId>,
    ) {
        for (id, n) in &module.nodes {
            if n.kind == NodeKind::Gate && is_pure(n.gate_class) && !connected.contains(id) {
                dead.insert(*id);
            }
        }
        for child in module.chips.values() {
            collect_orphans(child, connected, is_pure, dead);
        }
    }
    fn remove_dead(module: &mut Module, dead: &HashSet<NodeId>) {
        module.nodes.retain(|id, _| !dead.contains(id));
        // Only Layout wires can touch a dead node (it has no data wires); drop them too.
        module
            .wires
            .retain(|w| !dead.contains(&w.source.node_id) && !dead.contains(&w.target.node_id));
        module.scope_captures.retain(|id| !dead.contains(id));
        for child in module.chips.values_mut() {
            remove_dead(child, dead);
        }
    }
    let mut connected = HashSet::default();
    collect_connected(module, &mut connected);
    let mut dead = HashSet::default();
    collect_orphans(module, &connected, &is_pure, &mut dead);
    if !dead.is_empty() {
        remove_dead(module, &dead);
    }
}

/// Clean up degenerate `Exec_Union` nodes, repeating to a fixpoint (each
/// removal can degrade another union). Recurses into chip sub-modules.
///
/// - **No outgoing wires** (sink): remove the union and its incoming wires.
/// - **No incoming wires** (dead source, e.g. an if-join whose branches both
///   terminated via `return`/final `emit`): remove it and its outgoing wires —
///   whatever it fed keeps its other sources only.
/// - **Exactly one incoming wire** (pass-through): splice it out, rewiring its
///   consumers straight to the single source.
fn prune_dead_exec_unions(module: &mut Module) {
    /// Chase a source through spliced unions to the node actually carrying
    /// its wires now. Returns `None` for a pure splice cycle (unions feeding
    /// only each other — dead code whose wires all drop).
    fn resolve_src(
        spliced: &HashMap<NodeId, crate::ir::PortRef>,
        start: &crate::ir::PortRef,
    ) -> Option<crate::ir::PortRef> {
        let mut cur = start.clone();
        let mut hops = 0usize;
        while let Some(next) = spliced.get(&cur.node_id) {
            cur = next.clone();
            hops += 1;
            if hops > spliced.len() {
                return None;
            }
        }
        Some(cur)
    }

    // Degrees + adjacency computed once and maintained incrementally via a
    // worklist. The old version rebuilt counts over every wire and spliced
    // one union per full rebuild — O(unions × wires) on union-heavy modules.
    let queue: Vec<NodeId> = module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::UNION)
        .map(|(id, _)| *id)
        .collect();
    if queue.is_empty() {
        for child_module in module.chips.values_mut() {
            prune_dead_exec_unions(child_module);
        }
        return;
    }
    let is_union: HashSet<NodeId> = queue.iter().copied().collect();
    let mut queue = queue;

    // Counts and adjacency are only ever READ for union nodes (the queue is
    // union-only and every requeue is guarded by `is_union`), so only track
    // union entries — decrements on other nodes fall through `get_mut`.
    let mut in_count: HashMap<NodeId, usize> = HashMap::default();
    let mut out_count: HashMap<NodeId, usize> = HashMap::default();
    let mut in_edges: HashMap<NodeId, Vec<crate::ir::PortRef>> = HashMap::default();
    let mut out_edges: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    for w in &module.wires {
        if is_union.contains(&w.source.node_id) {
            *out_count.entry(w.source.node_id).or_default() += 1;
            out_edges
                .entry(w.source.node_id)
                .or_default()
                .push(w.target.node_id);
        }
        if is_union.contains(&w.target.node_id) {
            *in_count.entry(w.target.node_id).or_default() += 1;
            in_edges
                .entry(w.target.node_id)
                .or_default()
                .push(w.source.clone());
        }
    }

    let mut removed: HashSet<NodeId> = HashSet::default();
    let mut spliced: HashMap<NodeId, crate::ir::PortRef> = HashMap::default();

    while let Some(id) = queue.pop() {
        if removed.contains(&id) || spliced.contains_key(&id) {
            continue;
        }
        let ins = in_count.get(&id).copied().unwrap_or(0);
        let outs = out_count.get(&id).copied().unwrap_or(0);
        if ins == 0 || outs == 0 {
            // Dead sink/source: remove the union; its live edges die with it,
            // so decrement each live neighbor and requeue affected unions.
            removed.insert(id);
            for s in in_edges.get(&id).into_iter().flatten() {
                let Some(src) = resolve_src(&spliced, s) else {
                    continue;
                };
                if removed.contains(&src.node_id) {
                    continue; // edge already died with its source
                }
                if let Some(c) = out_count.get_mut(&src.node_id) {
                    *c = c.saturating_sub(1);
                }
                if is_union.contains(&src.node_id) {
                    queue.push(src.node_id);
                }
            }
            for t in out_edges.get(&id).into_iter().flatten() {
                if removed.contains(t) || spliced.contains_key(t) {
                    continue; // edge already accounted for at the other end
                }
                if let Some(c) = in_count.get_mut(t) {
                    *c = c.saturating_sub(1);
                }
                if is_union.contains(t) {
                    queue.push(*t);
                }
            }
        } else if ins == 1 {
            // Pass-through: splice out. Consumers keep their in-degree (the
            // wires just change source); the carrier gains this union's
            // out-edges and loses the one edge that fed it.
            let raw = in_edges
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|s| resolve_src(&spliced, s))
                .find(|s| !removed.contains(&s.node_id))
                .expect("counted one incoming wire");
            spliced.insert(id, raw.clone());
            let transferred = out_edges.remove(&id).unwrap_or_default();
            // Only LIVE transferred edges count toward the carrier's degree —
            // edges to removed/spliced targets were already discounted.
            let live = transferred
                .iter()
                .filter(|t| !removed.contains(t) && !spliced.contains_key(t))
                .count();
            if let Some(c) = out_count.get_mut(&raw.node_id) {
                *c += live;
                *c = c.saturating_sub(1);
            }
            // Adjacency is only read back for unions; drop transfers to
            // non-union carriers.
            if is_union.contains(&raw.node_id) {
                out_edges
                    .entry(raw.node_id)
                    .or_default()
                    .extend(transferred);
            }
        }
    }

    if !removed.is_empty() || !spliced.is_empty() {
        for id in removed.iter().chain(spliced.keys()) {
            module.nodes.remove(id);
        }
        module.wires.retain_mut(|w| {
            // Wires into a removed union die; wires into a spliced union are
            // superseded by the rewired consumer edges.
            if removed.contains(&w.target.node_id) || spliced.contains_key(&w.target.node_id) {
                return false;
            }
            if removed.contains(&w.source.node_id) {
                return false;
            }
            if spliced.contains_key(&w.source.node_id) {
                let Some(src) = resolve_src(&spliced, &w.source) else {
                    return false;
                };
                if removed.contains(&src.node_id) {
                    return false;
                }
                w.source = src;
            }
            true
        });
    }

    for child_module in module.chips.values_mut() {
        prune_dead_exec_unions(child_module);
    }
}

/// Compile a standalone chip declaration into an isolated [`Module`] suitable
/// for wrapping in a [`CompiledTemplate`].  This replicates the child-context
/// creation logic from `lower_chip_call_instance` without any parent-side
/// wiring.
pub fn compile_chip_template(
    chip_decl: &ChipDecl,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
) -> Module {
    use crate::ast::*;
    use crate::ir::build::{IdAllocator, ModuleBuilder};

    let template_name = &chip_decl.name;

    let mut builder = ModuleBuilder::new(template_name);
    builder.module.scopes.insert(
        ROOT_SCOPE_ID,
        ScopeInfo {
            kind: ScopeKind::ChipBody {
                name: chip_decl.name.clone(),
            },
            source_range: chip_decl.range.clone(),
            parent: None,
        },
    );

    // Resource-estimation templates don't need real doc text (DOC_TEXT is
    // display-only) — an empty map keeps the LowerCtx lifetime local.
    let empty_docs: HashMap<usize, String> = HashMap::default();

    let mut ctx = LowerCtx {
        builder,
        ids: IdAllocator::default(),
        diagnostics: Vec::new(),
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: file.to_string(),
        scope: crate::scope::Scope::new(),
        handler_end_execs: Vec::new(),
        current_exec: None,
        handler_entry_exec: None,
        captured_events: HashMap::default(),
        next_chain_id: 0,
        current_anon_chip: None,
        mod_return_exec: None,
        mod_return_var: None,
        type_aliases: HashMap::default(),
        pending_emits: HashMap::default(),
        exec_signal_hubs: HashMap::default(),
        exec_signal_keys: HashMap::default(),
        next_scope_id: ROOT_SCOPE_ID + 1,
        template_cache: cache.clone(),
        await_armed_port: None,
        signal_awaits: HashMap::default(),
        exec_branch_depth: 0,
        exec_signal_payloads: HashMap::default(),
        pending_inline_record: None,
        pending_return_record: None,
        chip_call_stack: if chip_decl.name.is_empty() {
            Vec::new()
        } else {
            vec![chip_decl.range.clone()]
        },
        known_fn_names: Arc::new(HashSet::default()),
        is_root_module: false,
        doc_comments: &empty_docs,
    };

    // Create input ports
    for inp in &chip_decl.inputs {
        let resolved_record = match &inp.typ {
            TypeExpr::Record { fields, .. } => Some(fields.clone()),
            TypeExpr::Name { name, .. } => {
                ctx.type_aliases.get(name.as_str()).and_then(|te| match te {
                    TypeExpr::Record { fields, .. } => Some(fields.clone()),
                    _ => None,
                })
            }
            _ => None,
        };
        if let Some(fields) = &resolved_record {
            let mut record_fields = HashMap::default();
            for field in fields {
                let port_name = format!("{}_{}", inp.name, field.name);
                let ft = type_of_type_expr(&field.typ);
                let is_array = matches!(&field.typ, TypeExpr::Array { .. });
                let is_ref = matches!(&field.typ, TypeExpr::Ref { .. });
                let node_id = ctx.builder.add_input(
                    &mut ctx.ids,
                    &port_name,
                    ft.clone(),
                    chip_decl.range.clone(),
                );
                let binding = if is_array {
                    let inner = match &ft {
                        Type::Array(inner) => inner.as_ref().clone(),
                        Type::Ref(inner) => match inner.as_ref() {
                            Type::Array(inner) => inner.as_ref().clone(),
                            _ => ft.clone(),
                        },
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: VarStorage::Array,
                    })
                } else if is_ref {
                    let inner = match &ft {
                        Type::Ref(inner) => inner.as_ref().clone(),
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: VarStorage::Var,
                    })
                } else {
                    Binding::Input(NodeRecord {
                        node_id,
                        ty: ft.clone(),
                    })
                };
                record_fields.insert(crate::intern::intern(&field.name), binding);
            }
            ctx.scope
                .insert(&inp.name, Binding::Record(record_fields));
        } else if matches!(&inp.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
            let t = type_of_type_expr(&inp.typ);
            let is_array = matches!(&inp.typ, TypeExpr::Array { .. });
            let inner = match &t {
                Type::Ref(inner) => inner.as_ref().clone(),
                Type::Array(inner) => inner.as_ref().clone(),
                _ => t.clone(),
            };
            let node_id =
                ctx.builder
                    .add_input(&mut ctx.ids, &inp.name, t.clone(), chip_decl.range.clone());
            ctx.scope.insert(
                &inp.name,
                Binding::Var(VarRecord {
                    node_id,
                    inner_type: inner,
                    get_node_for_handler: None,
                    storage: if is_array {
                        VarStorage::Array
                    } else {
                        VarStorage::Var
                    },
                }),
            );
        } else {
            let t = type_of_type_expr(&inp.typ);
            let node_id =
                ctx.builder
                    .add_input(&mut ctx.ids, &inp.name, t.clone(), chip_decl.range.clone());
            ctx.scope.insert(
                &inp.name,
                Binding::Input(NodeRecord { node_id, ty: t }),
            );
        }
    }

    // Create output ports
    for out in &chip_decl.outputs {
        let t = type_of_type_expr(&out.typ);
        let node_id =
            ctx.builder
                .add_output(&mut ctx.ids, &out.name, t.clone(), chip_decl.range.clone());
        ctx.scope.insert(
            &crate::lower::context::output_scope_key(&out.name),
            Binding::Output(NodeRecord { node_id, ty: t }),
        );
    }

    // Pre-declare + lower body
    let sig_output_names: HashSet<&str> =
        chip_decl.outputs.iter().map(|n| n.name.as_ref()).collect();
    for stmt in &chip_decl.body.stmts {
        match stmt {
            Stmt::In(i) => pre_declare_input(&mut ctx, i),
            Stmt::Var(v) => pre_declare_var(&mut ctx, v),
            Stmt::Buffer(b) => pre_declare_buffer(&mut ctx, b),
            Stmt::Array(a) => pre_declare_array(&mut ctx, a),
            Stmt::OutBinding(o) if !sig_output_names.contains(&o.name.as_ref()) => {
                pre_declare_output(
                    &mut ctx,
                    &o.name,
                    o.value.as_ref(),
                    o.typ.as_ref(),
                    o.side,
                    o.label.as_deref(),
                    &o.range,
                );
            }
            _ => {}
        }
    }
    for stmt in &chip_decl.body.stmts {
        lower_stmt(&mut ctx, stmt);
    }

    ctx.builder.module
}

#[cfg(test)]
mod tests;
