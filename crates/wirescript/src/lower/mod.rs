//! Lowering: AST + typecheck annotations → IR Module.
//!
//! Strategy: walk each top-level declaration. Vars and I/O become IR
//! nodes with deterministic names. Handlers become one event node feeding
//! an exec chain; statements thread a `current_exec` PortRef through each
//! step. Expressions produce gate nodes whose value output is threaded
//! into their consumer.

use std::collections::{HashMap, HashSet};
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
}

pub fn lower(input: LowerInput<'_>) -> LowerResult {
    let ids = IdAllocator::default();
    let builder = ModuleBuilder::new(input.module_name.unwrap_or("main"));
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
        captured_events: HashMap::new(),
        next_chain_id: 0,
        current_anon_chip: None,
        mod_return_exec: None,
        mod_return_var: None,
        type_aliases: {
            let mut m = HashMap::new();
            for d in &input.ast.decls {
                if let crate::ast::TopDecl::TypeAlias(ta) = d {
                    m.insert(ta.name.clone(), ta.typ.clone());
                }
            }
            m
        },
        pending_emits: HashMap::new(),
        exec_signal_hubs: HashMap::new(),
        exec_signal_keys: HashMap::new(),
        next_scope_id: ROOT_SCOPE_ID + 1,
        template_cache: input.template_cache.clone(),
        await_armed_port: None,
        signal_awaits: HashMap::new(),
        exec_branch_depth: 0,
        exec_signal_payloads: HashMap::new(),
        pending_inline_record: None,
        chip_call_stack: Vec::new(),
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
    materialize_unfoldable_constants(&mut module);
    inline_orphan_literals(&mut module);
    crate::emit::partition_anon_chips(&mut module);
    LowerResult {
        module,
        diagnostics: ctx.diagnostics,
    }
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
    let mut make_for: HashMap<NodeId, NodeId> = HashMap::new();
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
    loop {
        let mut outgoing: HashMap<NodeId, Vec<(NodeId, WirePort)>> = HashMap::new();
        let mut incoming_count: HashMap<NodeId, usize> = HashMap::new();
        for w in &module.wires {
            outgoing
                .entry(w.source.node_id)
                .or_default()
                .push((w.target.node_id, w.target.port));
            *incoming_count.entry(w.target.node_id).or_default() += 1;
        }

        let lit_ids: Vec<NodeId> = module
            .nodes
            .iter()
            .filter(|(_, n)| n.gate_class == gc::LITERAL)
            .map(|(id, _)| *id)
            .collect();
        let mut changed = false;
        let mut removed: HashSet<NodeId> = HashSet::new();
        for lit_id in lit_ids {
            let out = outgoing.get(&lit_id);
            let out_len = out.map_or(0, |v| v.len());
            if out_len != 1 {
                continue;
            }
            if incoming_count.get(&lit_id).copied().unwrap_or(0) != 0 {
                continue;
            }
            let (target_id, target_port) = out.unwrap()[0];
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
            changed = true;
        }

        // Fold pure constant-string `String_Concatenate` wrappers (the legacy
        // way a string literal became a wire, before inline wire-variant
        // support) into consumers that accept an inline string variant. Unlike
        // `_Literal`, this is gated on `port_accepts_inline_variant` — a string
        // can't fill a wire-only port, so those keep the real concat gate.
        let concat_ids: Vec<NodeId> = module
            .nodes
            .iter()
            .filter(|(id, n)| n.gate_class == gc::STRING_CONCATENATE && !removed.contains(id))
            .map(|(id, _)| *id)
            .collect();
        for cid in concat_ids {
            if incoming_count.get(&cid).copied().unwrap_or(0) != 0 {
                continue;
            }
            let Some(out) = outgoing.get(&cid).filter(|v| v.len() == 1) else {
                continue;
            };
            let (target_id, target_port) = out[0];
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
            changed = true;
        }

        if !changed {
            break;
        }
        module
            .wires
            .retain(|w| !removed.contains(&w.source.node_id));
    }
    for child_module in module.chips.values_mut() {
        inline_orphan_literals(child_module);
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
    loop {
        let mut in_count: HashMap<NodeId, usize> = HashMap::new();
        let mut out_count: HashMap<NodeId, usize> = HashMap::new();
        for w in &module.wires {
            *out_count.entry(w.source.node_id).or_default() += 1;
            *in_count.entry(w.target.node_id).or_default() += 1;
        }
        let unions: Vec<NodeId> = module
            .nodes
            .iter()
            .filter(|(_, n)| n.gate_class == gc::UNION)
            .map(|(id, _)| *id)
            .collect();

        // Dead sinks/sources: no outgoing, or no incoming.
        let dead: Vec<NodeId> = unions
            .iter()
            .copied()
            .filter(|id| {
                out_count.get(id).copied().unwrap_or(0) == 0
                    || in_count.get(id).copied().unwrap_or(0) == 0
            })
            .collect();
        if !dead.is_empty() {
            let dead_set: HashSet<NodeId> = dead.iter().copied().collect();
            for id in &dead {
                module.nodes.remove(id);
            }
            module.wires.retain(|w| {
                !dead_set.contains(&w.source.node_id) && !dead_set.contains(&w.target.node_id)
            });
            continue;
        }

        // Pass-throughs: exactly one input — splice.
        let Some(&splice) = unions
            .iter()
            .find(|id| in_count.get(id).copied().unwrap_or(0) == 1)
        else {
            break;
        };
        let src = module
            .wires
            .iter()
            .find(|w| w.target.node_id == splice)
            .map(|w| w.source.clone())
            .expect("counted one incoming wire");
        for w in module.wires.iter_mut() {
            if w.source.node_id == splice {
                w.source = src.clone();
            }
        }
        module.wires.retain(|w| w.target.node_id != splice);
        module.nodes.remove(&splice);
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
        captured_events: HashMap::new(),
        next_chain_id: 0,
        current_anon_chip: None,
        mod_return_exec: None,
        mod_return_var: None,
        type_aliases: HashMap::new(),
        pending_emits: HashMap::new(),
        exec_signal_hubs: HashMap::new(),
        exec_signal_keys: HashMap::new(),
        next_scope_id: ROOT_SCOPE_ID + 1,
        template_cache: cache.clone(),
        await_armed_port: None,
        signal_awaits: HashMap::new(),
        exec_branch_depth: 0,
        exec_signal_payloads: HashMap::new(),
        pending_inline_record: None,
        chip_call_stack: if chip_decl.name.is_empty() {
            Vec::new()
        } else {
            vec![chip_decl.name.clone()]
        },
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
            let mut record_fields = HashMap::new();
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
                .insert(inp.name.clone(), Binding::Record(record_fields));
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
                inp.name.clone(),
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
                inp.name.clone(),
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
            out.name.clone(),
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
