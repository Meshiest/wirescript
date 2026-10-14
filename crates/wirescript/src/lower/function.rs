//! `fn` lowering: one function definition plane per `fn`, and a Function
//! Call gate at each call site.
//!
//! The plane is a `FunctionDefinition` shell on the builder's root grid whose
//! inner grid holds a `CallableEntry_Function`, one `FunctionInput` per
//! parameter slot, the body, and one `FunctionOutput` per result. The body is
//! lowered by the `mod` inline path with each parameter bound to its input's
//! value, so everything a `mod` body may do (returns, refs, records, early
//! exits through storage) lowers the same way here.
//!
//! Nothing but a call's definition wire crosses the plane's wall. A body that
//! reads or writes outer storage draws a wire from that storage into the
//! plane; `capture_through_handles` turns each such wire into a handle var
//! inside the plane, which emit bakes with a reference to the storage. A call
//! to a `fn` inside a plane (itself included) keeps its wire from the shell.

use super::*;
use crate::ast::{CallArg, ChipDecl, Expr};
use crate::ir::{Literal, Wire};

/// What a call needs of a lowered `fn`.
#[derive(Clone, Debug)]
pub(crate) struct FnPlane {
    pub(crate) shell: NodeId,
}

/// The most input or output slots a Function Call has (`Input0..7`).
pub(crate) const MAX_SLOTS: usize = 8;

/// The call pin a slot binds to. A slot's `FunctionInput` / `FunctionOutput`
/// carries the same number as its `PortOrder`.
///
/// Unverified in game: the 2026-09-07 tests found the order positional and
/// reversed, without settling whether `PortOrder` or brick position is the key
/// (fn-probes `02_port_order`). This is the one place to change if it is.
pub(crate) fn slot_pin(slot: usize) -> usize {
    slot
}

/// One input slot: the parameter it belongs to, the record field for a
/// record parameter, and the slot's type.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    pub(crate) param: usize,
    pub(crate) field: Option<String>,
    pub(crate) ty: Type,
}

/// The input slots of `decl`, in order: one per scalar parameter, one per
/// field of a record parameter.
pub(super) fn input_slots(ctx: &LowerCtx, decl: &ChipDecl) -> Vec<Slot> {
    let mut out = Vec::new();
    for (i, p) in decl.inputs.iter().enumerate() {
        match ctx.record_fields_of(&p.typ) {
            Some(fields) => {
                for f in fields {
                    out.push(Slot { param: i, field: Some(f.name.clone()), ty: ctx.resolve_local_type(&f.typ) });
                }
            }
            None => out.push(Slot { param: i, field: None, ty: ctx.resolve_local_type(&p.typ) }),
        }
    }
    out
}

/// The value slot `slot` of `param` takes when a call omits the argument: its
/// default, or that field of a record default.
fn param_default(ctx: &LowerCtx, param: &crate::ast::Param, slot: &Slot) -> Option<Literal> {
    let e = param.default.as_ref()?;
    let lookup = |n: &str| ctx.resolve_mod(n);
    let cx = ctx.const_ctx(Some(&lookup));
    let mut budget = crate::const_eval::Budget::default();
    let lit = crate::const_eval::eval_expr(e, &cx, &mut budget).ok()?;
    let lit = match (&slot.field, lit) {
        (Some(f), Literal::Record(fields)) => fields.into_iter().find(|(n, _)| n == f)?.1,
        (None, lit) => lit,
        _ => return None,
    };
    Some(crate::const_eval::coerce_const_to_type(lit, &slot.ty, ctx.consts.types()))
}

fn decl_key(decl: &ChipDecl) -> (std::sync::Arc<str>, usize) {
    (decl.range.file.clone(), decl.range.start.offset)
}

/// Lower a call to the `fn` `decl`: build its plane on first use, then a
/// Function Call bound to it at this point of the exec chain.
pub(super) fn lower_fn_call(ctx: &mut LowerCtx, decl: &ChipDecl, args: &[CallArg], range: &SourceRange) -> PortRef {
    // `exec = trigger` runs the call off the given trigger, from pure context
    // too. The trigger is the caller's expression, lowered in its scope.
    let exec_arg = args.iter().find_map(|a| match a {
        CallArg::Named { name, value, .. } if name == "exec" => Some(value),
        _ => None,
    });
    let exec_src = exec_arg.map(|e| lower_expr(ctx, e));
    let Some(exec) = exec_src.or(ctx.current_exec) else {
        ctx.error(
            "WS111",
            format!(
                "`{}` is a `fn`, which runs when its call fires, so it needs an exec context: \
                 call it from a handler or a `mod`/`fn` body, or pass `exec = trigger`",
                decl.name
            ),
            range,
        );
        return crate::lower::expr::synthesise_unsupported_no_warn(ctx, range);
    };
    // A call to a `fn` whose body is being lowered reaches itself, directly
    // or through other `fn`s. The game does not run a recursive call.
    if let Some(at) = ctx.fn_building.iter().position(|k| *k == decl_key(decl)) {
        let through = ctx.fn_building.len() - at > 1;
        ctx.error(
            "WS020",
            format!(
                "`{}` calls itself{}: a `fn` cannot recurse, as the game does not run a recursive \
                 Function Call. Use a `for` loop, or a back-edge loop over ticks",
                decl.name,
                if through { " through another `fn`" } else { "" }
            ),
            range,
        );
        return crate::lower::expr::synthesise_unsupported_no_warn(ctx, range);
    }
    let plane = match ctx.fn_planes.get(&decl_key(decl)) {
        Some(p) => p.clone(),
        None => build_plane(ctx, decl),
    };

    // Arguments, in the caller's scope and on its chain.
    let expanded = crate::lower::call::expand_spread_args(ctx, args);
    let positional = crate::lower::call::positional_args(&expanded);
    let slots = input_slots(ctx, decl);
    let mut sources: Vec<Option<PortRef>> = vec![None; slots.len()];
    for (i, param) in decl.inputs.iter().enumerate() {
        let named = expanded.iter().find_map(|a| match a {
            CallArg::Named { name, value, .. } if *name == param.name => Some(value),
            _ => None,
        });
        let Some(arg) = positional.get(i).copied().or(named) else { continue };
        let param_slots: Vec<usize> = (0..slots.len()).filter(|s| slots[*s].param == i).collect();
        if slots[param_slots[0]].field.is_some() {
            let Some(fields) = record_arg(ctx, arg) else {
                ctx.error(
                    "WS112",
                    format!("this argument to `{}` gives `{}` no record fields to pass", decl.name, param.name),
                    arg.range(),
                );
                continue;
            };
            for s in param_slots {
                let name = crate::intern::intern(slots[s].field.as_deref().unwrap_or_default());
                if let Some(b) = fields.get(&name).cloned() {
                    sources[s] = crate::lower::access::binding_to_port(ctx, &b, arg.range());
                }
            }
        } else {
            sources[param_slots[0]] = Some(value_arg(ctx, decl, param, arg));
        }
    }

    let call = ctx.add_gate(AddNodeOpts {
        gate_class: gc::FUNCTION_CALL,
        source_range: range.clone(),
        ports: call_io(),
        properties: {
            let mut p = HashMap::default();
            let given: Vec<Literal> = (0..MAX_SLOTS)
                .map(|pin| {
                    let slot = (0..slots.len()).find(|s| slot_pin(*s) == pin);
                    Literal::Int(slot.is_some_and(|s| sources[s].is_some()) as i64)
                })
                .collect();
            p.insert(crate::intern::intern("InputSources"), Literal::Array(given));
            p.insert(*sym::NAME_LABEL, Literal::String(decl.name.clone()));
            p
        },
        note: Some("fn call"),
        ..Default::default()
    });
    let caller_exec = ctx.current_exec;
    let exec = exec_src.or(caller_exec).unwrap_or(exec);
    ctx.connect(exec, call.port(WirePort::Exec));
    ctx.connect(plane.shell.port(WirePort::Definition), call.port(WirePort::Definition));
    for (s, src) in sources.iter().enumerate() {
        if let Some(src) = src {
            ctx.connect(*src, call.port(input_pin(slot_pin(s))));
        }
    }
    // The body may have written any storage it reaches through a handle.
    reset_var_get_caches(ctx);

    // An explicit trigger's chain does not continue the caller's: the call
    // hands back its `Completed` as an `exec` field instead.
    if exec_src.is_some() {
        ctx.current_exec = caller_exec;
        let mut record: HashMap<crate::intern::Sym, Binding> = decl
            .outputs
            .iter()
            .enumerate()
            .map(|(k, o)| {
                (crate::intern::intern(&o.name), Binding::Local(LocalRecord { port: call.port(output_pin(slot_pin(k))) }))
            })
            .collect();
        record.entry(crate::intern::intern("exec")).or_insert(Binding::Local(LocalRecord { port: call.port(WirePort::Completed) }));
        ctx.pending_inline_record = Some(record);
        return match decl.outputs.is_empty() {
            true => call.port(WirePort::Completed),
            false => call.port(output_pin(slot_pin(0))),
        };
    }
    ctx.current_exec = Some(call.port(WirePort::Completed));

    let outputs: Vec<&crate::ast::NamedOutput> = decl.outputs.iter().collect();
    match outputs.as_slice() {
        [] => call.port(WirePort::Completed),
        [_] => call.port(output_pin(slot_pin(0))),
        many => {
            let record = many
                .iter()
                .enumerate()
                .map(|(k, o)| {
                    (
                        crate::intern::intern(&o.name),
                        Binding::Local(LocalRecord { port: call.port(output_pin(slot_pin(k))) }),
                    )
                })
                .collect();
            ctx.pending_inline_record = Some(record);
            call.port(output_pin(slot_pin(0)))
        }
    }
}

/// The value a scalar, ref or container parameter receives: the argument's
/// value, or, for a `*T` or container parameter given storage, that storage's
/// ref, which the body reads and writes through.
fn value_arg(ctx: &mut LowerCtx, decl: &ChipDecl, param: &crate::ast::Param, arg: &Expr) -> PortRef {
    use crate::lower::call::{RefArg, classify_ref_arg};
    let storage = ctx.ref_storage(Some(decl), &param.typ);
    if storage.is_none() {
        return lower_expr(ctx, arg);
    }
    match classify_ref_arg(ctx, storage, arg) {
        RefArg::Static(v) => v.node_id.port(match v.storage {
            VarStorage::Array => WirePort::ArrayVarRef,
            VarStorage::Map => WirePort::MapVarRef,
            _ => WirePort::VarRef,
        }),
        RefArg::Input(inp) => inp.node_id.port(WirePort::RerOutput),
        RefArg::Forward(d) => d.port,
        RefArg::Value | RefArg::Unbound => lower_expr(ctx, arg),
    }
}

/// A record argument's fields.
fn record_arg(ctx: &mut LowerCtx, arg: &Expr) -> Option<HashMap<crate::intern::Sym, Binding>> {
    if let Expr::RecordLit { fields, .. } = arg {
        let ty = ctx.type_of(arg);
        return Some(crate::lower::decl::lower_record_lit(ctx, fields, crate::lower::decl::Slots::Of(&ty)));
    }
    if let Some(Binding::Record(f)) = crate::lower::access::resolve_field_chain(ctx, arg).cloned() {
        return Some(f);
    }
    if let Some(f) = crate::lower::stmt::value_record_fields(ctx, arg) {
        return Some(f);
    }
    ctx.pending_inline_record = None;
    lower_expr(ctx, arg);
    ctx.pending_inline_record.take()
}

fn input_pin(k: usize) -> WirePort {
    WirePort::from_name(&format!("Input{k}"))
}

fn output_pin(k: usize) -> WirePort {
    WirePort::from_name(&format!("Output{k}"))
}

fn call_io() -> GateIO {
    let mut inputs = vec![PortSpec::new(WirePort::Definition.sym(), Type::Any), PortSpec::exec(*sym::EXEC)];
    let mut outputs = vec![
        PortSpec::exec(WirePort::Completed.sym()),
        PortSpec::new(WirePort::BAborted.sym(), Type::Bool),
        PortSpec::new(WirePort::IterationsExecuted.sym(), Type::Int),
    ];
    for k in 0..MAX_SLOTS {
        inputs.push(PortSpec::new(input_pin(k).sym(), Type::Any));
        outputs.push(PortSpec::new(output_pin(k).sym(), Type::Any));
    }
    GateIO { inputs, outputs }
}

/// Build `decl`'s definition plane, on the builder's root grid.
fn build_plane(ctx: &mut LowerCtx, decl: &ChipDecl) -> FnPlane {
    let saved_chip = ctx.current_anon_chip.take();
    let saved_exec = ctx.current_exec.take();
    let saved_caches = snapshot_var_caches(ctx);
    reset_var_get_caches(ctx);
    let saved_record = ctx.pending_inline_record.take();

    let spec = crate::ir::plane::function_plane_spec();
    let shell = ctx.add_gate(AddNodeOpts {
        gate_class: spec.shell_class,
        source_range: decl.range.clone(),
        ports: GateIO { inputs: vec![], outputs: vec![PortSpec::new(WirePort::Definition.sym(), Type::Any)] },
        properties: {
            let mut p = HashMap::default();
            p.insert(*sym::NAME_LABEL, Literal::String(decl.name.clone()));
            p
        },
        ..Default::default()
    });
    if let Some(n) = ctx.builder.module.nodes.get_mut(&shell) {
        n.kind = NodeKind::Chip;
    }
    let plane = FnPlane { shell };
    ctx.fn_planes.insert(decl_key(decl), plane.clone());
    ctx.fn_building.push(decl_key(decl));

    ctx.current_anon_chip = Some(shell);
    let entry = ctx.add_gate(AddNodeOpts {
        gate_class: spec.entry_class,
        source_range: decl.range.clone(),
        ports: GateIO { inputs: vec![], outputs: vec![PortSpec::exec(WirePort::Body.sym())] },
        ..Default::default()
    });

    // One input per slot, bound under a name no source can spell.
    let slots = input_slots(ctx, decl);
    ctx.push_scope(crate::scope::ScopeTag::BLOCK);
    let mut args = Vec::new();
    for (i, param) in decl.inputs.iter().enumerate() {
        let mut fields: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        let mut whole = None;
        for (s, slot) in slots.iter().enumerate().filter(|(_, s)| s.param == i) {
            let label = match &slot.field {
                Some(f) => format!("{}.{f}", param.name),
                None => param.name.clone(),
            };
            let input = ctx.add_gate(AddNodeOpts {
                gate_class: gc::FUNCTION_INPUT,
                source_range: param.range.clone(),
                ports: GateIO { inputs: vec![], outputs: vec![PortSpec::new(*sym::VALUE, slot.ty.clone())] },
                properties: {
                    let mut p = HashMap::default();
                    p.insert(*sym::NAME_LABEL, Literal::String(label.clone()));
                    p.insert(*sym::PORT_LABEL, Literal::String(label));
                    p.insert(crate::intern::intern("PortOrder"), Literal::Int(slot_pin(s) as i64));
                    let default = param_default(ctx, param, slot)
                        .or_else(|| crate::lower::predeclare::default_literal_for_var_type(&slot.ty));
                    if let Some(d) = default {
                        p.insert(crate::intern::intern("DefaultValue"), d);
                    }
                    p
                },
                ..Default::default()
            });
            let port = input.port(WirePort::Value);
            match &slot.field {
                Some(f) => {
                    fields.insert(crate::intern::intern(f), Binding::Local(LocalRecord { port }));
                }
                None => whole = Some(port),
            }
        }
        let name = format!("#fnarg{i}");
        let binding = match whole {
            Some(port) => Binding::Local(LocalRecord { port }),
            None => Binding::Record(fields),
        };
        ctx.scope.insert(&name, binding);
        args.push(CallArg::Positional(Expr::Ident { name, range: param.range.clone() }));
    }

    ctx.current_exec = Some(entry.port(WirePort::Body));
    let result = crate::lower::call::lower_chip_call_inline(ctx, decl, &args, &[], &decl.range);
    let record = ctx.pending_inline_record.take();
    let mut results: Vec<Option<PortRef>> = Vec::new();
    match decl.outputs.len() {
        0 => {}
        1 => results.push(Some(result)),
        _ => {
            for o in &decl.outputs {
                let b = record.as_ref().and_then(|r| r.get(&crate::intern::intern(&o.name)).cloned());
                results.push(b.and_then(|b| crate::lower::access::binding_to_port(ctx, &b, &o.range)));
            }
        }
    }
    for (k, (o, src)) in decl.outputs.iter().zip(results).enumerate() {
        let ty = ctx.resolve_local_type(&o.typ);
        let label = if o.name == "_" { "return".to_string() } else { o.name.clone() };
        let out = ctx.add_gate(AddNodeOpts {
            gate_class: gc::FUNCTION_OUTPUT,
            source_range: o.range.clone(),
            ports: GateIO { inputs: vec![PortSpec::new(*sym::VALUE, ty.clone())], outputs: vec![] },
            properties: {
                let mut p = HashMap::default();
                p.insert(*sym::NAME_LABEL, Literal::String(label.clone()));
                p.insert(*sym::PORT_LABEL, Literal::String(label));
                p.insert(crate::intern::intern("PortOrder"), Literal::Int(slot_pin(k) as i64));
                if let Some(d) = crate::lower::predeclare::default_literal_for_var_type(&ty) {
                    p.insert(crate::intern::intern("DefaultValue"), d);
                }
                p
            },
            ..Default::default()
        });
        if let Some(src) = src {
            ctx.connect(src, out.port(WirePort::Value));
        }
    }
    ctx.pop_scope();
    ctx.fn_building.pop();

    ctx.pending_inline_record = saved_record;
    restore_var_caches(ctx, &saved_caches);
    ctx.current_exec = saved_exec;
    ctx.current_anon_chip = saved_chip;
    plane
}

/// Where a handle var points, set on the handle by [`capture_through_handles`]
/// and baked by emit once bricks have their addresses: the id of the storage
/// gate or `fn` shell it names. `MICROCHIP_BRICK` marks the var
/// `GetMicrochipBrick()` lowers to.
pub(crate) use crate::ir::{HANDLE_TARGET, MICROCHIP_BRICK};

type ModPath = Vec<NodeId>;

/// A node's own name, for a diagnostic or a handle's label.
fn node_name(n: &crate::ir::Node) -> Option<String> {
    [*sym::NAME_LABEL, *sym::PORT_LABEL, *sym::BINDING_NAME].iter().find_map(|k| match n.properties.get(k) {
        Some(Literal::String(s)) if !s.is_empty() && !s.starts_with('_') => Some(s.clone()),
        _ => None,
    })
}

/// The wire binding a Function Call to its definition: out of a `fn` shell's
/// `Definition`. The one wire allowed across a function plane's wall.
pub(crate) fn is_definition_wire(src: &crate::ir::Node, w: &Wire) -> bool {
    src.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION && w.source.port == WirePort::Definition
}

/// Every wire with exactly one end inside a `fn` plane becomes a handle: a
/// wire from outer storage (its `VarRef`/`ArrayVarRef`/`MapVarRef`) is rewired
/// from a var inside the plane whose value emit bakes with a reference to that
/// storage. A call's definition wire (see [`is_definition_wire`]) stays. Any
/// other such wire cannot exist in game, and is WS113.
///
/// Runs after partition, before boundary pins, so no pin is ever made on a
/// function plane's wall.
pub(crate) fn capture_through_handles(root: &mut Module, diagnostics: &mut Vec<crate::diagnostic::Diagnostic>) {
    let mut owner: HashMap<NodeId, ModPath> = HashMap::default();
    let mut planes: Vec<ModPath> = Vec::new();
    // Every node's module path, and the path of every `fn` plane.
    root.walk(&mut |path, m| {
        for (id, n) in &m.nodes {
            owner.insert(*id, path.to_vec());
            if n.kind == NodeKind::Chip && n.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION && m.chips.contains_key(id) {
                planes.push([path, &[*id]].concat());
            }
        }
    });
    if planes.is_empty() {
        return;
    }
    // A node's plane: of the planes its module path starts with, the first
    // in `planes`.
    let plane_index: HashMap<&[NodeId], usize> = planes.iter().enumerate().map(|(i, p)| (p.as_slice(), i)).collect();
    let plane_of = |id: &NodeId| {
        let p = owner.get(id)?;
        (1..=p.len()).filter_map(|k| plane_index.get(&p[..k]).copied()).min().map(|i| planes[i].clone())
    };
    let fn_name = |root: &Module, plane: &ModPath| {
        node_name(&root.at(&plane[..plane.len() - 1]).nodes[plane.last().unwrap()]).unwrap_or_default()
    };

    let mut all = Vec::new();
    root.walk(&mut |path, m| all.extend(m.wires.iter().enumerate().map(|(i, w)| (path.to_vec(), i, *w))));
    let mut handles: HashMap<(ModPath, PortRef), NodeId> = HashMap::default();
    let mut removed: HashMap<ModPath, Vec<usize>> = HashMap::default();
    let mut added: Vec<(ModPath, Wire)> = Vec::new();
    for (holder, at, w) in all {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        let (sp, tp) = (plane_of(&w.source.node_id), plane_of(&w.target.node_id));
        if sp == tp {
            continue;
        }
        let src = root.at(&owner[&w.source.node_id]).nodes[&w.source.node_id].clone();
        let tgt = root.at(&owner[&w.target.node_id]).nodes[&w.target.node_id].clone();
        if src.gate_class == gc::LITERAL {
            continue;
        }
        // A call binds its definition by a wire from the shell, which the game
        // accepts into a plane (a value held in a var is refused there). So a
        // call inside a plane, a recursive one included, keeps that wire.
        if is_definition_wire(&src, &w) {
            continue;
        }
        let storage_ref = matches!(src.gate_class, gc::VAR | gc::ARRAY_VAR | gc::MAP_VAR)
            && matches!(w.source.port, WirePort::VarRef | WirePort::ArrayVarRef | WirePort::MapVarRef);
        let Some(plane) = tp.clone().filter(|_| sp.is_none() && storage_ref) else {
            let what = node_name(&src).map(|n| format!("`{n}`")).unwrap_or_else(|| "a value".into());
            let (message, range) = match (&sp, &tp) {
                (Some(inside), None) => (
                    format!(
                        "`fn {}` sends {what} out of its plane, which a function cannot do: its \
                         results leave only through its return values",
                        fn_name(root, inside)
                    ),
                    src.source_range.clone(),
                ),
                (_, Some(inside)) => (
                    format!(
                        "`fn {}` reads {what} from outside its plane. A function reaches outer storage \
                         (a `var`, array or map) and constants, not a computed value: pass it as a parameter",
                        fn_name(root, inside)
                    ),
                    tgt.source_range.clone(),
                ),
                (None, None) => continue,
            };
            diagnostics.push(crate::diagnostic::Diagnostic::error("WS113", message, range));
            continue;
        };
        let key = (plane.clone(), w.source);
        let handle = match handles.get(&key) {
            Some(h) => *h,
            None => {
                let ty = src.output_type(w.source.port).unwrap_or(Type::Any);
                let id = NodeId::fresh();
                let mut props = HashMap::default();
                props.insert(*sym::NAME_LABEL, Literal::String(format!("&{}", node_name(&src).unwrap_or_default())));
                props.insert(crate::intern::intern(HANDLE_TARGET), Literal::Int(src.id.0 as i64));
                let node = crate::ir::Node {
                    source_range: tgt.source_range.clone(),
                    ..crate::ir::Node::new(id, NodeKind::Gate, gc::VAR, GateIO::var_storage(ty), props, None, Some("fn handle"))
                };
                root.at_mut(&plane).nodes.insert(id, node);
                handles.insert(key, id);
                id
            }
        };
        removed.entry(holder).or_default().push(at);
        added.push((plane, Wire { source: PortRef { node_id: handle, port: WirePort::Value }, target: w.target }));
    }
    for (path, at) in removed {
        let at: HashSet<usize> = at.into_iter().collect();
        let m = root.at_mut(&path);
        let mut i = 0;
        m.wires.retain(|_| {
            i += 1;
            !at.contains(&(i - 1))
        });
    }
    for (path, w) in added {
        root.at_mut(&path).wires.push(w);
    }
}
