//! The `mod` inline-expansion path: bind params into the caller's scope and
//! lower the body in place.

use super::*;

/// What a parameter binds of a gate read given to it as its outputs.
enum ReadArg {
    /// The first output, one value.
    Port(PortRef),
    /// The first output, an enum's tag and payload slots.
    Record(Fields),
    /// Every output, for a parameter that takes them all.
    Whole(Fields),
}

/// `fields`, the value of the argument at `at`, as a parameter of type
/// `param_ty` binds it. A gate read (held, or a choice between reads) given to
/// a parameter of one value is its first output, an enum parameter included,
/// as it is in every other place for one value; anything else binds whole.
fn one_value_of_read(
    ctx: &mut LowerCtx<'_>,
    param_ty: Option<&Type>,
    fields: Fields,
    at: &SourceRange,
) -> ReadArg {
    let Some(ty) = param_ty.filter(|t| takes_one_value(t)) else {
        return ReadArg::Whole(fields);
    };
    match held_output(ctx, &fields, at, OutputPick::First) {
        Some(Binding::Record(first)) => ReadArg::Record(first),
        Some(first) if holds_one_value(&unwrap_ref(ty)) => match binding_to_port(ctx, &first, at) {
            Some(port) => ReadArg::Port(port),
            None => ReadArg::Whole(fields),
        },
        _ => ReadArg::Whole(fields),
    }
}

/// How a parameter takes its argument, installed into the body's scope once
/// the body's frame is open.
enum ParamBinding {
    /// A `const` parameter's value, its type, and whether its fold was uncertified.
    Const {
        lit: Literal,
        ty: Type,
        uncertified: bool,
    },
    /// A container argument naming the caller's storage.
    Ref(VarRecord),
    /// A container argument forwarded as a ref value.
    Dyn(DynRefRecord),
    /// A container argument naming one of the caller's input pins.
    Input(NodeRecord),
    /// A value argument, as the port it lowered to.
    Val(PortRef),
    /// A record argument, as its fields.
    Record(Fields),
}

impl ParamBinding {
    /// Where the binding installs among the others: constants, refs, forwarded
    /// refs, inputs, values, then records.
    fn install_rank(&self) -> u8 {
        match self {
            Self::Const { .. } => 0,
            Self::Ref(_) => 1,
            Self::Dyn(_) => 2,
            Self::Input(_) => 3,
            Self::Val(_) => 4,
            Self::Record(_) => 5,
        }
    }
}

/// The bindings of a call's parameters, in parameter order.
#[derive(Default)]
struct ArgBindings {
    list: Vec<(String, ParamBinding)>,
    /// Record parameters that bind the caller's own storage.
    aliases: HashSet<String>,
}

impl ArgBindings {
    fn push(&mut self, name: &str, binding: ParamBinding) {
        self.list.push((name.to_string(), binding));
    }
}

pub(in crate::lower) fn lower_chip_call_inline(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    type_args: &[TypeExpr],
    range: &SourceRange,
) -> PortRef {
    // Expand `...tuple` spreads into per-element positional args before binding.
    let expanded = expand_spread_args(ctx, args);
    let args = &expanded[..];
    // This call's output nodes don't exist yet, so any wire touching them
    // lands at an index >= this. The output-source lookups and the
    // output-node removal below scan only this tail, which keeps deep
    // inline-call chains linear.
    let wire_start = ctx.builder.module.wires.len();
    let positional_args = positional_args(args);

    // Generic mod (`mod pick<T>(...)`): push the call's type substitution as
    // typecheck recorded it, so the body's `T`-typed annotations monomorphize
    // (see `resolve_local_type`). The stack cannot grow unbounded: the WS020
    // recursion guard in `lower_chip_call` blocks any transitive self-call.
    // The frame is lexically merged with an enclosing generic body's params
    // (see `body_frame`).
    let own_frame = if chip_decl.type_params.is_empty() {
        None
    } else {
        let Some(frame) = mono_frame(ctx, chip_decl, range, &positional_args, type_args) else {
            return report_unchecked_instantiation(ctx, chip_decl, range);
        };
        Some(frame)
    };
    // The body's resolution key, set only while the body itself is lowered:
    // arguments and `exec = ...` are the caller's expressions and keep its key.
    let body_subst = ctx.body_resolution_subst(chip_decl, own_frame.as_ref());
    if !instantiation_checked(ctx, chip_decl, &body_subst) {
        return report_unchecked_instantiation(ctx, chip_decl, range);
    }
    // The frame the body lowers under: a generic mod's own merged with its
    // enclosing one, else the enclosing generic body's (lexical, not the
    // caller's, whose shadowing `T` would leak in), else `None`. Pushed only
    // after the arguments, which lower under the caller's frame.
    let lexical_frame = match &own_frame {
        Some(own) => Some(
            ctx.body_frame(chip_decl, Some(own))
                .unwrap_or_else(|| own.clone()),
        ),
        None => ctx.body_frame(chip_decl, None),
    };

    // Collect param bindings first (before mutating ctx) so ref lookups
    // see the caller's vars.
    let bindings = bind_inline_args(
        ctx,
        chip_decl,
        &positional_args,
        lexical_frame.as_ref(),
        range,
    );
    // `exec = trigger` named arg: run this mod's body off the given trigger
    // when the caller is outside an exec context. The trigger is the caller's
    // expression, so it lowers here, in the caller's scope and frame.
    let exec_src = args.exec_arg().map(|e| lower_expr(ctx, e));
    ctx.open_call_body(chip_decl);
    if let Some(f) = &lexical_frame {
        ctx.mono_stack.push(f.clone());
    }

    // Typecheck checks a `mod` body once, in its declaration's scope, so the
    // caller's block frames (and constant stacks) are set aside for the body;
    // otherwise a name the caller shadows (`on go { let q = 7 bump() }`)
    // binds differently than typecheck checked. Everything the body takes
    // from the call (arguments, `const` values, the `exec` trigger) is bound
    // above. Restored after `pop_scope`; the body's pushes and pops balance.
    let decl_scope = ctx.enter_decl_scope(chip_decl);
    // `out <name> = <record>` bindings are per-body: the caller's are set
    // aside so this expansion starts empty, and restored after the body,
    // before this call's record is assembled.
    let saved_out_records = std::mem::take(&mut ctx.pending_out_records);

    ctx.push_scope(crate::scope::ScopeTag::MODULE);

    // A mod declared inside an `import * as ns` module resolves its siblings,
    // vars and consts by bare name, so that module's members go into the body
    // frame first; params inserted below shadow them. See `LowerCtx::ns_mod_scopes`.
    let ns_key = chip_decl.range.start_key();
    let ns_scope = ctx.ns_mod_scopes.get(&ns_key).cloned();
    if let Some(ns_scope) = &ns_scope {
        for (name, binding) in ns_scope.iter() {
            ctx.scope.insert(name, binding.clone());
        }
    }

    install_param_bindings(ctx, chip_decl, bindings, range);
    install_param_patterns(ctx, chip_decl);
    install_ns_consts(ctx, chip_decl, ns_scope.as_deref());
    pre_declare_body(ctx, chip_decl, &body_subst);

    // Install this mod's output nodes (for `return value`).
    // Track their IDs so cleanup only removes these, not parent outputs.
    let mod_output_ids = declare_mod_outputs(ctx, chip_decl);

    let frame = ModBodyFrame::enter(ctx, chip_decl, exec_src);
    let body_has_return = block_contains_return(&chip_decl.body);

    let plan = plan_return_storage(ctx, chip_decl);
    let stored = &plan.stored;
    let num_return_values = plan.num_return_values;
    ctx.mod_emit_storage = stored.iter().cloned().collect();
    alloc_return_storage(ctx, chip_decl, &plan);

    let body_diag_mark = ctx.diagnostics.len();
    let body_has_exec = ctx.current_exec.is_some();
    let unsequenced = unsequenced_outputs(&plan, body_has_exec);
    ctx.mod_unsequenced = unsequenced.iter().cloned().collect();
    let saved_subst = std::mem::replace(&mut ctx.resolution_subst, body_subst);
    lower_block(ctx, &chip_decl.body);
    ctx.resolution_subst = saved_subst;
    // Several outputs (or one enum or record output, stored per field),
    // several returns and no exec chain: a branch holding a `return` is never
    // lowered and its storage never written. The one scalar output case is
    // reported below, after the call's value is resolved.
    if !body_has_exec
        && num_return_values > 1
        && (chip_decl.outputs.len() > 1 || ctx.mod_return_record.is_some())
    {
        report_branch_return(ctx, chip_decl, range);
    }

    if body_has_return {
        let fallthrough = ctx.current_exec.take();
        let ret_path = ctx.mod_return_exec.take();
        ctx.current_exec = merge_exec(ctx, fallthrough, ret_path, &chip_decl.body.range);
    }

    // A multi-return mod reads its return value after the union.
    let ret_var_clone = ctx.mod_return_var.clone();
    let multi_return_port = match (&ret_var_clone, ctx.current_exec) {
        (Some(ret_var), Some(exec)) => Some(read_return_value(ctx, ret_var, exec, true)),
        _ => None,
    };

    // Taken before the caller's is restored, to be handed back as the call's
    // value below.
    let ret_record_clone = ctx.mod_return_record.clone();
    let output_types = frame.leave(ctx);

    let inline_output_ids = &mod_output_ids;
    // A single-output mod's value source, taken before its output node goes.
    let return_output_port = if multi_return_port.is_some() {
        multi_return_port
    } else if chip_decl.outputs.len() == 1 {
        output_wire_source(ctx, wire_start, inline_output_ids[0])
    } else {
        None
    };

    ctx.pop_scope();

    // Hand the caller back its own frames and constants (see
    // `enter_decl_scope` above `push_scope`).
    ctx.leave_decl_scope(decl_scope);

    // The body may have written vars passed through records, which the
    // caller's own Var bindings still have cached reads of; clear every cache
    // so later reads make fresh Var_Gets.
    reset_var_get_caches(ctx);

    // Multi-output inline mod: capture each output's value source into a record
    // so `let s = mod(...); s.field` resolves to the right port (the output
    // nodes are internal and removed below). Always set for this call, so a
    // nested multi-output argument call cannot leak its record.
    let return_record = ctx.pending_return_record.take();
    // Records bound by `out <name> = <record>` in THIS body, swapping the
    // caller's own map back in (see `saved_out_records` above).
    let out_records = std::mem::replace(&mut ctx.pending_out_records, saved_out_records);
    assemble_call_record(
        ctx,
        chip_decl,
        &CallRecordSources {
            stored: ret_record_clone.as_ref(),
            returned: return_record,
            out_records: &out_records,
            output_ids: inline_output_ids,
            wire_start,
        },
        range,
    );

    // A pure call's outputs whose writes could not run: the first read of one
    // reports WS007 for the call (see `unsequenced_reads`). Several returns
    // already reported above.
    if !unsequenced.is_empty()
        && chip_decl.outputs.len() > 1
        && num_return_values <= 1
        && let Some(rec) = &ret_record_clone
    {
        let call = ctx.unsequenced_calls;
        ctx.unsequenced_calls += 1;
        for name in &unsequenced {
            if let Some(binding) = rec.get(&intern(name)) {
                register_unsequenced(ctx, binding, name, range, call);
            }
        }
    }

    remove_inline_outputs(ctx, inline_output_ids, wire_start);

    for (i, param) in chip_decl.inputs.iter().enumerate() {
        if ctx.ref_or_array(Some(chip_decl), &param.typ)
            && let Some(arg_expr) = positional_args.get(i)
            && let Expr::Ident { name, .. } = arg_expr
            && let Some(id) = ctx.lookup_var(name.as_str()).map(|v| v.node_id)
        {
            ctx.var_gets.remove(&id);
        }
    }

    if lexical_frame.is_some() {
        ctx.mono_stack.pop();
    }

    select_return_port(
        ctx,
        chip_decl,
        &BodyOutcome {
            direct_port: return_output_port,
            num_return_values,
            stored,
            unsequenced: &unsequenced,
            ret_var: ret_var_clone,
            diag_mark: body_diag_mark,
            output_types: &output_types,
        },
        range,
    )
}

/// Bind each parameter of `chip_decl` to its argument, in the caller's scope
/// and frame, and fix every argument at its value.
fn bind_inline_args(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    positional_args: &[&Expr],
    lexical_frame: Option<&MonoFrame>,
    range: &SourceRange,
) -> Vec<(String, ParamBinding)> {
    // Each argument is fixed at its value before a later argument runs code
    // that may change what it reads. A record argument naming the caller's
    // storage binds that storage, as a ref does.
    let mut in_order = ordered::ArgOrder::default();
    let mut bound = ArgSlots::default();
    let mut out = ArgBindings::default();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        bound.bind(&mut in_order, &out, range);
        in_order.next(ctx);
        // An omitted argument takes the parameter's default value.
        let Some(arg_expr) = positional_args.get(i).copied().or(param.default.as_ref()) else {
            continue;
        };
        bind_param_arg(ctx, chip_decl, param, arg_expr, lexical_frame, &mut out);
    }

    // A `...rest` variadic parameter captures every positional arg past the
    // fixed params into a compile-time tuple keyed `"0"`, `"1"` and so on, as a
    // tuple literal lowers. A `...rest` in the body splats it back through
    // `expand_spread_args`. Built in the caller's scope so its elements
    // reference the caller's vars.
    if let Some(rest_name) = &chip_decl.rest {
        let start = chip_decl.inputs.len().min(positional_args.len());
        let rest_fields: Vec<RecordLitField> = positional_args[start..]
            .iter()
            .enumerate()
            .map(|(i, e)| RecordLitField::Named {
                name: i.to_string(),
                value: (*e).clone(),
                range: e.range().clone(),
            })
            .collect();
        bound.bind(&mut in_order, &out, range);
        in_order.next(ctx);
        let record = lower_record_lit(ctx, &rest_fields, Slots::Unknown);
        out.push(rest_name, ParamBinding::Record(record));
    }
    bound.bind(&mut in_order, &out, range);
    let held = in_order.finish(ctx);
    bound.fixed(&held, &mut out.list);
    out.list
}

/// Lower one argument into `out` as the binding its parameter takes.
fn bind_param_arg(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    param: &Param,
    arg_expr: &Expr,
    lexical_frame: Option<&MonoFrame>,
    out: &mut ArgBindings,
) {
    let arg_expr = &*ctx.with_map_literals(arg_expr);
    // A record typecheck rejected for a parameter of one value (WS003)
    // gives it nothing, so the body reports nothing more about it.
    let r = arg_expr.range();
    if ctx.resolutions.rejected_record_args.contains(&r.key()) {
        let port = synthesise_unsupported_no_warn(ctx, r);
        out.push(&param.name, ParamBinding::Val(port));
        return;
    }
    if bind_record_arg(ctx, chip_decl, param, arg_expr, lexical_frame, out) {
        return;
    }
    match &param.typ {
        pt if ctx.ref_storage(Some(chip_decl), pt).is_some() => {
            bind_container_arg(ctx, chip_decl, param, arg_expr, lexical_frame, out);
        }
        _ => bind_value_arg(ctx, chip_decl, param, arg_expr, lexical_frame, out),
    }
}

/// Bind an argument that is a record: a literal, the caller's own record
/// binding, or a record value the instance path's resolver reads. False when
/// the argument is none of those.
fn bind_record_arg(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    param: &Param,
    arg_expr: &Expr,
    lexical_frame: Option<&MonoFrame>,
    out: &mut ArgBindings,
) -> bool {
    // A record literal arg lowers to a Binding::Record (as `let x = {..}`
    // does), so record and destructured params receive their fields.
    if let Expr::RecordLit { fields, .. } = arg_expr {
        let lit = ctx.type_of(arg_expr);
        let record = lower_record_lit(ctx, fields, Slots::Of(&lit));
        out.push(&param.name, ParamBinding::Record(record));
        return true;
    }
    // `&p` binds the same record as bare `p`: the param receives the
    // caller's own per-field `Binding::Var`s, so writes reach the caller.
    // Unwrap before the lookup: `resolve_field_chain` on an `Expr::RefOf`
    // names no binding, and the container arm below cannot resolve a record.
    let record_arg = match arg_expr {
        Expr::RefOf { operand, .. } => operand.as_ref(),
        other => other,
    };
    if let Some(Binding::Record(fields)) = resolve_field_chain(ctx, record_arg).cloned() {
        let param_ty = ctx.declared_type(chip_decl, &param.typ, lexical_frame);
        match one_value_of_read(ctx, param_ty.as_ref(), fields, record_arg.range()) {
            ReadArg::Port(port) => out.push(&param.name, ParamBinding::Val(port)),
            ReadArg::Record(first) => out.push(&param.name, ParamBinding::Record(first)),
            ReadArg::Whole(fields) => {
                // A gate read held in a field the parameter's type holds
                // one value in is its first output there.
                let fields = match (param_ty, ctx.checked_type_at(record_arg.range())) {
                    (Some(slot @ (Type::Record(_) | Type::Tuple(_))), Some(checked)) => {
                        held_reads_where_one_value(checked, fields, &Slot::Typed(slot))
                    }
                    _ => fields,
                };
                out.aliases.insert(param.name.clone());
                out.push(&param.name, ParamBinding::Record(fields));
            }
        }
        return true;
    }
    // Record value forms the arms above miss (a container element, a
    // nested field of one, a record-valued `if`/`match`) go through the
    // microchip-instance path's resolver (`call::wiring`); calls and enum
    // constructions go to the `wants_record` arm. Gated on a record, tuple
    // or enum shape. A choice between gate reads given to a one-value
    // parameter is one value.
    let one_value_choice = matches!(record_arg, Expr::IfExpr { .. } | Expr::MatchExpr { .. })
        && ctx.gate_outputs(record_arg.range()).is_some()
        && ctx
            .declared_type(chip_decl, &param.typ, lexical_frame)
            .is_some_and(|t| takes_one_value(&t) && holds_one_value(&unwrap_ref(&t)));
    if !one_value_choice
        && matches!(
            record_arg,
            Expr::IndexAccess { .. }
                | Expr::FieldAccess { .. }
                | Expr::TuplePick { .. }
                | Expr::IfExpr { .. }
                | Expr::MatchExpr { .. }
                | Expr::As { .. }
        )
        && (ctx.record_or_tuple_fields(&param.typ).is_some()
            || matches!(
                unwrap_ref(&ctx.type_of(record_arg)),
                Type::Enum { .. } | Type::Record(_) | Type::Tuple(_)
            ))
        && let Some(fields) = value_record_fields(ctx, record_arg)
    {
        let param_ty = ctx.declared_type(chip_decl, &param.typ, lexical_frame);
        match one_value_of_read(ctx, param_ty.as_ref(), fields, record_arg.range()) {
            ReadArg::Port(port) => out.push(&param.name, ParamBinding::Val(port)),
            ReadArg::Record(fields) | ReadArg::Whole(fields) => {
                out.push(&param.name, ParamBinding::Record(fields));
            }
        }
        return true;
    }
    false
}

/// Bind a container param (`T[]`, `Map<K,V>`, `ref T`): it captures the
/// caller's var/input binding by reference, so the mod body resolves the param
/// to the same ArrayVar/MapVar ref; a scalar value would fail every container
/// method.
fn bind_container_arg(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    param: &Param,
    arg_expr: &Expr,
    lexical_frame: Option<&MonoFrame>,
    out: &mut ArgBindings,
) {
    let pt = &param.typ;
    // A `const` array/map argument gets its runtime form so the
    // callee binds a real container ref (otherwise every use is
    // WS044). The binding carries the same gate node, so mutation
    // is still rejected (`access::reject_const_container_mutation`).
    let named = deref_arg(arg_expr);
    if let Expr::Ident { .. } = named {
        materialize_const_container(ctx, named);
    }
    match classify_ref_arg(ctx, ctx.ref_storage(Some(chip_decl), pt), arg_expr) {
        RefArg::Static(var_rec) => {
            // The body reads its param afresh, not through a read
            // the caller cached before the call.
            ctx.var_gets.remove(&var_rec.node_id);
            out.push(&param.name, ParamBinding::Ref(var_rec));
        }
        RefArg::Input(inp) => out.push(&param.name, ParamBinding::Input(inp)),
        RefArg::Forward(d) => out.push(&param.name, ParamBinding::Dyn(d)),
        RefArg::Value => {
            let port = lower_expr(ctx, arg_expr);
            // The param's type is the callee's, so it resolves
            // under the body's frame.
            let t = ctx.with_mono_frame(lexical_frame.cloned(), |ctx| ctx.resolve_local_type(pt));
            let (storage, inner_type) =
                super::context::container_binding(ctx.ref_storage(Some(chip_decl), pt), &t)
                    .expect("gated by ref_storage");
            out.push(
                &param.name,
                ParamBinding::Dyn(DynRefRecord {
                    port,
                    storage,
                    inner_type,
                }),
            );
        }
        // An array/map literal: its immutable container gate when
        // it is constant, otherwise nothing the body could use.
        RefArg::Unbound => {
            match materialize_literal_container(ctx, deref_arg(arg_expr), &param.name) {
                Some(rec) => out.push(&param.name, ParamBinding::Ref(rec)),
                None => ctx.error(
                    "WS044",
                    format!(
                        "argument `{}` is an array/map literal with non-constant \
                         elements, which names no storage for the container \
                         parameter to use, so operations on it would be silently \
                         dropped. Pass an array or map `var`",
                        param.name
                    ),
                    arg_expr.range(),
                ),
            }
        }
    }
}

/// Bind a scalar, `const` or record-returning-call argument.
fn bind_value_arg(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    param: &Param,
    arg_expr: &Expr,
    lexical_frame: Option<&MonoFrame>,
    out: &mut ArgBindings,
) {
    // A `const` parameter's value is the argument's, recorded in
    // the body frame's constants so it resolves through
    // `ctx.const_lookup()` like a named constant. Evaluated before
    // `lower_expr`, which would expand a `const mod` call argument
    // into real gates. Typecheck reports a failing argument (WS046).
    if param.is_const
        && let (Ok(lit), budget) = ctx.const_eval_budget(arg_expr, ConstScope::Plain)
    {
        let (lit, ty) = match ctx.declared_type(chip_decl, &param.typ, lexical_frame) {
            Some(ty) => (
                crate::const_eval::coerce_const_to_type(lit, &ty, ctx.consts.types()),
                ty,
            ),
            None => (lit, unwrap_ref(&ctx.type_of(arg_expr))),
        };
        out.push(
            &param.name,
            ParamBinding::Const {
                lit,
                ty: unwrap_ref(&ty),
                uncertified: budget.uncertified,
            },
        );
        return;
    }
    // Whether this param wants a record: an annotated record type,
    // a destructuring pattern, or an enum value (a `__disc` +
    // payload-slots record). Computed before lowering, since
    // `record_fields_of` borrows `ctx`.
    let wants_record = param.pattern.is_some()
        || ctx.record_fields_of(&param.typ).is_some()
        || matches!(ctx.type_of(arg_expr), Type::Enum { .. });
    // Clear any leftover inline record so only THIS arg's lowering can
    // set it (an enum construction / record-returning call stashes it).
    ctx.pending_inline_record = None;
    let val_port = lower_expr(ctx, arg_expr);
    // A record-returning call as an argument (`Take(Make())`)
    // stashed its field->source-port record, as for `let m =
    // Make()`; consume it so the record param receives its fields.
    // Gated on `wants_record` (a multi-output call into a scalar
    // param unwraps to its first output via `val_port`) and on a
    // record-producing form, so a stale stash is never picked up.
    if wants_record
        && (matches!(arg_expr, Expr::Call { .. } | Expr::VariantCtor { .. })
            || matches!(ctx.type_of(arg_expr), Type::Enum { .. }))
        && let Some(record) = ctx.pending_inline_record.take()
    {
        // A single-output call's stash is its value; a multi-output
        // result is passed whole.
        let unwrapped = unwrap_call_record(ctx, arg_expr, record);
        out.push(&param.name, ParamBinding::Record(unwrapped));
        return;
    }
    out.push(&param.name, ParamBinding::Val(val_port));
}

/// Insert the bound arguments into the body's frame, shadowing what the
/// namespace members put there.
fn install_param_bindings(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    mut bindings: Vec<(String, ParamBinding)>,
    range: &SourceRange,
) {
    bindings.sort_by_key(|(_, b)| b.install_rank());
    for (name, binding) in bindings {
        match binding {
            ParamBinding::Const {
                lit,
                ty,
                uncertified,
            } => {
                // Every entry is a `const` parameter; marked declared so an `if`
                // condition in the body naming it is eligible for the widened elision.
                let marks = crate::const_scope::Marks {
                    declared: true,
                    placeholder: false,
                    uncertified,
                };
                ctx.consts.record(&name, lit.clone(), marks);
                bind_shadowing_const_param(ctx, &name, lit, &ty, range, &chip_decl.body);
            }
            ParamBinding::Ref(rec) => {
                ctx.scope.insert(&name, Binding::Var(rec));
            }
            ParamBinding::Dyn(rec) => {
                ctx.scope.insert(&name, Binding::DynRef(rec));
            }
            ParamBinding::Input(rec) => {
                ctx.scope.insert(&name, Binding::Input(rec));
            }
            ParamBinding::Val(port) => {
                ctx.scope
                    .insert(&name, Binding::Local(LocalRecord { port }));
            }
            ParamBinding::Record(mut fields) => {
                if let Some(param) = chip_decl.inputs.iter().find(|p| p.name == name) {
                    ref_fields::rebind_ref_fields(
                        ctx,
                        &param.typ,
                        &mut fields,
                        ref_fields::FieldKind::Place,
                        range,
                    );
                }
                ctx.scope.insert(&name, Binding::Record(fields));
            }
        }
    }
}

/// Apply destructuring patterns: for each param with a pattern, look up the
/// synthetic binding just inserted and expand it into the named fields.
fn install_param_patterns(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) {
    for param in &chip_decl.inputs {
        let Some(pattern) = &param.pattern else {
            continue;
        };
        let base_binding = ctx.scope.get(&param.name).cloned();
        match pattern {
            ParamPattern::Record { fields, .. } => {
                let record_map = match &base_binding {
                    Some(Binding::Record(m)) => Some(m.clone()),
                    _ => None,
                };
                if let Some(src) = record_map {
                    install_record_destruct(ctx, &src, fields);
                }
            }
            ParamPattern::Tuple { names, rest } => {
                // A tuple literal argument (or a `let` bound to one) arrives as
                // a record keyed by element index, so read the names out of it.
                if let Some(Binding::Record(src)) = &base_binding {
                    let src = src.clone();
                    // Index keys: a tuple-pattern parameter is only fed a
                    // tuple literal (or a `let` bound to one). A `let (a, b)`
                    // source may instead be a name-keyed multi-output result.
                    let order: Vec<String> = (0..names.len().max(src.len()))
                        .map(|i| i.to_string())
                        .collect();
                    install_tuple_destruct(ctx, &src, names, rest.as_ref(), &order);
                }
                // For tuple patterns, extract by index from the local binding.
                if let Some(Binding::Local(local)) = &base_binding {
                    let source_node = ctx.builder.module.nodes.get(&local.port.node_id).cloned();
                    if let Some(node) = source_node {
                        let outputs: Vec<_> = node.ports.outputs.iter().collect();
                        for (i, name) in names.iter().enumerate() {
                            if let Some(port) = outputs.get(i) {
                                ctx.scope.insert(
                                    name,
                                    Binding::Local(LocalRecord {
                                        port: port_ref(node.id, resolve(port.name)),
                                    }),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The module's constants beside its members, each one a parameter does not
/// rebind. A constant container has no member binding until a runtime read
/// builds its gate, so it is recorded wherever the frame binds nothing else
/// of its name.
fn install_ns_consts(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    ns_scope: Option<&HashMap<String, Binding>>,
) {
    let ns_key = chip_decl.range.start_key();
    if let (Some(ns_scope), Some(ns_consts)) = (ns_scope, ctx.ns_mod_consts.get(&ns_key).cloned()) {
        for (name, lit, marks) in ns_consts.iter() {
            let kept = match (ns_scope.get(name), ctx.scope.get_current(name)) {
                (Some(member), Some(now)) => same_binding(member, now),
                (None, now) => now.is_none(),
                (Some(_), None) => false,
            };
            if kept {
                ctx.consts.record(name, lit.clone(), *marks);
                own_constant_container(ctx, name, lit, &chip_decl.range);
            }
        }
    }
}

/// Pre-declare the body's top-level var/array/map/buffer so they are
/// registered in ctx.vars. A nested chip/mod declaration goes into
/// `pass1_chips`'s innermost frame (opened by `push_scope`, dropped by
/// `pop_scope`), so it shadows an outer one for the whole body, chips
/// instantiated inside it included.
fn pre_declare_body(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl, body_subst: &Arc<[Type]>) {
    let saved_subst = std::mem::replace(&mut ctx.resolution_subst, Arc::clone(body_subst));
    record_later_storage(ctx, &chip_decl.body);
    pre_declare_block_vars(ctx, &chip_decl.body);
    ctx.resolution_subst = saved_subst;
}

/// Pre-declare the storage and nested chip names of a mod body's top level.
fn pre_declare_block_vars(ctx: &mut LowerCtx<'_>, block: &Block) {
    let stmts = &Mentions::new(&block.stmts);
    for (at, s) in block.stmts.iter().enumerate() {
        match s {
            // A nested declaration shadows a same-named outer one for any
            // later initializer in this body (see `pre_declare_chip_name`).
            Stmt::ChipDecl(c) => pre_declare_chip_name(ctx, c),
            // A name the body already binds (a parameter) is declared by
            // its statement instead (see `hoist_or_defer`).
            Stmt::Var(v) if hoist_or_defer(ctx, stmts, at, &v.name, &v.range) => {
                ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
            }
            Stmt::Array(a) if hoist_or_defer(ctx, stmts, at, &a.name, &a.range) => {
                pre_declare_array(ctx, a);
            }
            Stmt::Map(m) if hoist_or_defer(ctx, stmts, at, &m.name, &m.range) => {
                pre_declare_map(ctx, m);
            }
            Stmt::Buffer(b) if hoist_or_defer(ctx, stmts, at, &b.name, &b.range) => {
                pre_declare_buffer(ctx, b);
            }
            // Nested blocks contribute chip names only. Storage inside an
            // `if` is block-scoped and declared in its own frame when the
            // block lowers (see `needs_declaration`); hoisting it would
            // collide with a same-named body-level var.
            Stmt::If(i) => {
                pre_declare_block_chip_names(ctx, &i.then_block);
                if let Some(eb) = &i.else_block {
                    pre_declare_block_chip_names(ctx, eb);
                }
            }
            _ => {}
        }
    }
}

/// Pre-declare the nested `chip`/`mod` names of a block, recursively, for
/// forward references. Storage is skipped.
fn pre_declare_block_chip_names(ctx: &mut LowerCtx<'_>, block: &Block) {
    for s in &block.stmts {
        match s {
            Stmt::ChipDecl(c) => pre_declare_chip_name(ctx, c),
            Stmt::If(i) => {
                pre_declare_block_chip_names(ctx, &i.then_block);
                if let Some(eb) = &i.else_block {
                    pre_declare_block_chip_names(ctx, eb);
                }
            }
            _ => {}
        }
    }
}

/// Declare the mod's output nodes and give back their ids, which are marked
/// inline so cleanup removes only these.
fn declare_mod_outputs(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for out in &chip_decl.outputs {
        pre_declare_output(
            ctx,
            &out.name,
            None,
            Some(&out.typ),
            None,
            None,
            None,
            false,
            false,
            &out.range,
        );
        if let Some(r) = ctx.lookup_output(&out.name) {
            ids.push(r.node_id);
            ctx.inline_outputs.insert(r.node_id);
        }
    }
    ids
}

/// The caller's return and output state, set aside while a mod body lowers.
struct ModBodyFrame {
    return_exec: Option<PortRef>,
    return_var: Option<VarRecord>,
    return_record: Option<Fields>,
    output_names: Vec<String>,
    output_types: HashMap<String, Type>,
    chip_return_outputs: Vec<(String, bool)>,
    emit_storage: HashSet<String>,
    unsequenced: HashSet<String>,
    caller_exec: Option<PortRef>,
    explicit_exec: bool,
}

impl ModBodyFrame {
    /// Set the caller's state aside and give the body its own outputs. An
    /// `exec_src` becomes the body's chain.
    fn enter(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl, exec_src: Option<PortRef>) -> Self {
        let caller_exec = ctx.current_exec;
        if let Some(src) = exec_src {
            ctx.current_exec = Some(src);
        }
        let return_exec = ctx.mod_return_exec.take();
        let return_var = ctx.mod_return_var.take();
        let return_record = ctx.mod_return_record.take();
        let output_names = std::mem::replace(
            &mut ctx.mod_output_names,
            chip_decl.outputs.iter().map(|o| o.name.clone()).collect(),
        );
        let types = chip_decl
            .outputs
            .iter()
            .map(|o| {
                let ty = if is_exec_type(ctx, &o.typ) {
                    Type::Exec
                } else {
                    ctx.resolve_local_type(&o.typ)
                };
                (o.name.clone(), ty)
            })
            .collect();
        let output_types = std::mem::replace(&mut ctx.mod_output_types, types);
        let chip_return_outputs = std::mem::take(&mut ctx.chip_return_outputs);
        let emit_storage = std::mem::take(&mut ctx.mod_emit_storage);
        let unsequenced = std::mem::take(&mut ctx.mod_unsequenced);
        Self {
            return_exec,
            return_var,
            return_record,
            output_names,
            output_types,
            chip_return_outputs,
            emit_storage,
            unsequenced,
            caller_exec,
            explicit_exec: exec_src.is_some(),
        }
    }

    /// Hand the caller its state back. Gives the body's output types. An
    /// explicit trigger's chain must not leak into the caller's context.
    fn leave(self, ctx: &mut LowerCtx<'_>) -> HashMap<String, Type> {
        ctx.mod_return_exec = self.return_exec;
        ctx.mod_return_var = self.return_var;
        ctx.mod_return_record = self.return_record;
        ctx.mod_output_names = self.output_names;
        let output_types = std::mem::replace(&mut ctx.mod_output_types, self.output_types);
        ctx.chip_return_outputs = self.chip_return_outputs;
        ctx.mod_emit_storage = self.emit_storage;
        ctx.mod_unsequenced = self.unsequenced;
        if self.explicit_exec {
            ctx.current_exec = self.caller_exec;
        }
        output_types
    }
}

/// Which of a mod's value outputs go through return storage, and how its
/// returns write them.
struct ReturnPlan {
    num_return_values: usize,
    /// Whether any storage exists for this call.
    use_storage: bool,
    /// The outputs the storage holds.
    stored: Vec<String>,
    writes: HashMap<String, OutputWrites>,
}

/// Decide the return storage of a mod body from how it returns and writes its
/// outputs.
fn plan_return_storage(ctx: &LowerCtx<'_>, chip_decl: &ChipDecl) -> ReturnPlan {
    // A multi-return mod with an output gets storage for the return value.
    // Each `return expr` writes it; after the return union the caller reads it
    // (a scalar Var_Get, or the storage record itself for an enum or record).
    let num_return_values = count_return_values(&chip_decl.body);
    // Returns in a nested handler store too, on the handler's own trigger, and
    // the caller reads the value the handler last stored.
    let handler_returns = count_handler_return_values(&chip_decl.body);
    // An output written in more than one place (`emit`/`out` twice, or beside
    // a `return`), under a branch or loop, or inside a nested handler, takes
    // storage: every write stores, and the caller reads the one the path that
    // ran made. An output written once, unconditionally, keeps its direct wire.
    let value_outputs: Vec<String> = chip_decl
        .outputs
        .iter()
        .filter(|o| !is_exec_type(ctx, &o.typ))
        .map(|o| o.name.clone())
        .collect();
    let mut writes: HashMap<String, OutputWrites> = HashMap::default();
    count_output_writes(&chip_decl.body, &value_outputs, false, false, &mut writes);
    let needs = |o: &String| {
        writes.get(o).is_some_and(|w| {
            let n = w.emits + w.outs;
            n >= 1 && (n + num_return_values + handler_returns >= 2 || w.branch || w.handler)
        })
    };
    let use_storage =
        num_return_values > 1 || handler_returns >= 1 || value_outputs.iter().any(needs);
    // The outputs the storage holds: every value output when a `return` writes
    // them all, otherwise only the ones that need it. Any other output keeps
    // its own wire (or record) beside them.
    let stored: Vec<String> = if !use_storage {
        Vec::new()
    } else if num_return_values + handler_returns >= 1 {
        value_outputs.clone()
    } else {
        value_outputs.iter().filter(|o| needs(o)).cloned().collect()
    };
    ReturnPlan {
        num_return_values,
        use_storage,
        stored,
        writes,
    }
}

/// Allocate the storage `plan` calls for: one field per stored output when
/// there are several outputs, otherwise the one output's own storage.
fn alloc_return_storage(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl, plan: &ReturnPlan) {
    let stored = &plan.stored;
    if plan.use_storage && chip_decl.outputs.len() > 1 {
        // Several outputs: one storage field per output, keyed by its name, so
        // each `return (..)` stores every output and the caller reads the
        // runtime-selected return's values.
        let fields: Vec<RecordTypeField> = chip_decl
            .outputs
            .iter()
            .filter(|o| stored.contains(&o.name))
            .map(|o| RecordTypeField {
                name: o.name.clone(),
                typ: o.typ.clone(),
                range: o.range.clone(),
            })
            .collect();
        let fields = build_record_fields(
            ctx,
            VarStorage::Var,
            &fields,
            "__ret",
            None,
            None,
            &chip_decl.body.range,
        );
        ctx.mod_return_record = Some(fields);
    } else if plan.use_storage && chip_decl.outputs.len() == 1 {
        let out_type = ctx.resolve_local_type(&chip_decl.outputs[0].typ);
        // An enum or record output stores per field/slot, so each `return`
        // stores every field and the caller reads the runtime-selected branch.
        // A scalar Var holds one value and would lose the payload.
        if let Type::Enum { .. } = &out_type
            && let Some(def) = ctx.enum_defs.of(&out_type).cloned()
        {
            let Type::Enum { args, .. } = &out_type else {
                unreachable!()
            };
            let args = args.clone();
            let fields =
                build_enum_fields(ctx, &def, &args, "__ret", None, None, &chip_decl.body.range);
            ctx.mod_return_record = Some(fields);
        } else if let Some(rfields) = ctx.record_fields_of(&chip_decl.outputs[0].typ) {
            // A record output, inline `{ x, y }` or a `type P = { .. }` alias
            // (`record_fields_of` follows the alias and is `Some` only for
            // record types).
            let fields = build_record_fields(
                ctx,
                VarStorage::Var,
                &rfields,
                "__ret",
                None,
                None,
                &chip_decl.body.range,
            );
            ctx.mod_return_record = Some(fields);
        } else {
            ctx.mod_return_var = Some(return_value_var(ctx, out_type, &chip_decl.body.range));
        }
    }
}

/// The stored outputs a pure call cannot write: they are written on the body's
/// own path (or by a `return`), which has no exec chain to run on. One written
/// only inside a nested handler reads its storage directly.
fn unsequenced_outputs(plan: &ReturnPlan, body_has_exec: bool) -> Vec<String> {
    if body_has_exec {
        return Vec::new();
    }
    plan.stored
        .iter()
        .filter(|o| {
            plan.num_return_values >= 1 || plan.writes.get(*o).is_some_and(|w| w.plain || w.branch)
        })
        .cloned()
        .collect()
}

/// WS007 for a call whose returns sit in branches that need an exec context
/// to choose between.
fn report_branch_return(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl, range: &SourceRange) {
    ctx.error(
        "WS007",
        format!(
            "`{}` returns from inside a branch, so choosing between its returns needs \
             an exec context: call it from a handler or a `mod` body, or pass \
             `exec = ...`",
            chip_decl.name
        ),
        range,
    );
}

/// What a finished mod body left for the call's value.
struct CallRecordSources<'a> {
    /// The return storage record.
    stored: Option<&'a Fields>,
    /// A `return { ... }` record literal's fields.
    returned: Option<Fields>,
    /// Records bound by `out <name> = <record>`.
    out_records: &'a HashMap<String, Fields>,
    /// The internal output nodes.
    output_ids: &'a [NodeId],
    wire_start: usize,
}

/// Set `pending_inline_record` to the call's value, as a record a caller
/// resolves fields through; `None` for a call with no record value.
fn assemble_call_record(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    src: &CallRecordSources<'_>,
    range: &SourceRange,
) {
    let CallRecordSources {
        stored,
        returned,
        out_records,
        output_ids,
        wire_start,
    } = src;
    // Keyed by the record type's own fields rather than by output name.
    let by_field = chip_decl.outputs.len() == 1
        && (stored.is_some()
            || returned.is_some()
            || out_records.contains_key(&chip_decl.outputs[0].name));
    ctx.pending_inline_record = if let Some(rec) = stored
        && chip_decl.outputs.len() > 1
    {
        // Several outputs with storage: the stored ones read the storage, and
        // an output bound only by `out` keeps its own source.
        let mut record = gathered_outputs(ctx, chip_decl, out_records, output_ids, *wire_start);
        record.extend((*rec).clone());
        Some(record)
    } else if let Some(rec) = stored {
        // One stored enum or record output: each `return` stored its slots
        // here, so the caller reads the runtime-selected branch's value.
        Some((*rec).clone())
    } else if let Some(rec) = returned {
        // A `return { ... }` record literal into one record-typed output: its
        // fields arrive as a field->binding map, not a wire into the node.
        Some(rec.clone())
    } else if chip_decl.outputs.len() == 1
        && let Some(rec) = chip_decl
            .outputs
            .first()
            .and_then(|o| out_records.get(&o.name))
    {
        // A single record or enum output (`mod f() -> (o: Rec) { out o = rec }`)
        // is the call's value, as typecheck types it (`f().x` is the record's
        // field), so the stash is the record itself, as a returned one is.
        Some(rec.clone())
    } else if chip_decl.outputs.len() > 1 {
        Some(gathered_outputs(
            ctx,
            chip_decl,
            out_records,
            output_ids,
            *wire_start,
        ))
    } else {
        None
    };
    // A call's result is a value, as a chip's output pins are: a place the
    // body returns in a `*T` field leaves as its ref.
    if let Some(mut rec) = ctx.pending_inline_record.take() {
        use crate::lower::ref_fields::{FieldKind, rebind_fields, rebind_ref_fields};
        if by_field {
            rebind_ref_fields(
                ctx,
                &chip_decl.outputs[0].typ,
                &mut rec,
                FieldKind::Value,
                range,
            );
        } else {
            let outputs: Vec<RecordTypeField> = chip_decl
                .outputs
                .iter()
                .map(|o| RecordTypeField {
                    name: o.name.clone(),
                    typ: o.typ.clone(),
                    range: o.range.clone(),
                })
                .collect();
            rebind_fields(ctx, &outputs, &mut rec, FieldKind::Value, range);
        }
        ctx.pending_inline_record = Some(rec);
    }
}

/// Inline mod outputs are internal: remove the `MicrochipOutput` nodes. Their
/// wires all live in the tail added during this call, compacted in place in
/// order.
fn remove_inline_outputs(ctx: &mut LowerCtx<'_>, output_ids: &[NodeId], wire_start: usize) {
    if output_ids.is_empty() {
        return;
    }
    for id in output_ids {
        ctx.inline_outputs.remove(id);
        ctx.builder.module.nodes.remove(id);
        ctx.builder.module.outputs.retain(|o| o != id);
    }
    let wires = &mut ctx.builder.module.wires;
    let mut write = wire_start;
    for read in wire_start..wires.len() {
        let w = wires[read];
        if !output_ids.contains(&w.source.node_id) && !output_ids.contains(&w.target.node_id) {
            wires[write] = w;
            write += 1;
        }
    }
    wires.truncate(write);
}

/// What the body of an inlined mod left, for choosing the call's value port.
struct BodyOutcome<'a> {
    /// The value source found for the call, when there is one.
    direct_port: Option<PortRef>,
    num_return_values: usize,
    stored: &'a [String],
    unsequenced: &'a [String],
    /// The return storage var of a single-output mod.
    ret_var: Option<VarRecord>,
    /// How many diagnostics existed before the body lowered.
    diag_mark: usize,
    output_types: &'a HashMap<String, Type>,
}

/// The port a call to an inlined mod evaluates to.
fn select_return_port(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    outcome: &BodyOutcome<'_>,
    range: &SourceRange,
) -> PortRef {
    if let Some(out_port) = outcome.direct_port {
        return out_port;
    }
    let stored = outcome.stored;
    let num_return_values = outcome.num_return_values;
    // One scalar output, no wire into it, and no chain to continue from: the
    // body's value comes from a `return` inside a branch, and selecting
    // between branches needs an exec context the caller does not have. The
    // sentinel below would instead fail at emit as a `DroppedWire` naming `n0`.
    if chip_decl.outputs.len() == 1
        && ctx.current_exec.is_none()
        && ctx.pending_inline_record.is_none()
    {
        // A stored output written only inside a nested handler: its storage
        // holds the value the handler last wrote.
        if num_return_values == 0
            && !stored.is_empty()
            && outcome.unsequenced.is_empty()
            && let Some(var) = &outcome.ret_var
        {
            return var.node_id.port(WirePort::Value);
        }
        // A stored output with no chain to choose its write on: say so once,
        // in those words, rather than as a branch `return`.
        if num_return_values == 0 && !stored.is_empty() {
            if !ctx.diagnostics[outcome.diag_mark..]
                .iter()
                .any(|d| d.code == "WS007")
            {
                report_unsequenced_output_write(ctx, &stored[0], range);
            }
            return synthesise_unsupported_no_warn(ctx, range);
        }
        report_branch_return(ctx, chip_decl, range);
        return synthesise_unsupported_no_warn(ctx, range);
    }
    // A multi-output call read as ONE value (`m = f()`) is its first value
    // output, the field typecheck unwraps it to; never the exec chain.
    if chip_decl.outputs.len() > 1
        && ctx.record_result_call.as_ref() != Some(range)
        && let Some(first) = chip_decl
            .outputs
            .iter()
            .find(|o| !is_exec_type(ctx, &o.typ) && ctx.record_or_tuple_fields(&o.typ).is_none())
        && let Some(binding) = ctx
            .pending_inline_record
            .as_ref()
            .and_then(|r| r.get(&intern(&first.name)).cloned())
        // Not a read the caller made: an output this pure call could not
        // write reports only when something reads it.
        && !matches!(&binding, Binding::Var(v) if ctx.unsequenced_reads.contains_key(&v.node_id))
        // A payload-free enum output is one value, its tag.
        && let binding = match &binding {
            Binding::Record(rec) => outcome
                .output_types
                .get(&first.name)
                .and_then(|t| enum_tag(ctx, rec, t))
                .cloned()
                .unwrap_or(binding),
            _ => binding,
        }
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }
    // A record or multi-output result travels in `pending_inline_record`, not
    // a scalar port, so this return value is never read. `NodeId(0)` is never
    // allocated, so a caller that does wire it gets a dangling reference that
    // emit rejects.
    ctx.current_exec.unwrap_or(PortRef {
        node_id: NodeId(0),
        port: WirePort::ExecOut,
    })
}

/// Where each argument sits among a call's bound arguments (see
/// `ordered::ArgOrder`); a record naming the caller's storage is an alias and
/// is not bound.
#[derive(Default)]
struct ArgSlots {
    slots: Vec<Option<usize>>,
}

impl ArgSlots {
    /// Bind every argument added since the last call.
    fn bind(&mut self, args: &mut ordered::ArgOrder, bindings: &ArgBindings, range: &SourceRange) {
        for (name, binding) in &bindings.list[self.slots.len()..] {
            let slot = match binding {
                ParamBinding::Val(port) => Some(args.bind(
                    Binding::Local(LocalRecord { port: *port }),
                    name,
                    range,
                    None,
                )),
                ParamBinding::Record(rec) => (!bindings.aliases.contains(name))
                    .then(|| args.bind(Binding::Record(rec.clone()), name, range, None)),
                _ => None,
            };
            self.slots.push(slot);
        }
    }

    /// Each argument as it was held.
    fn fixed(&self, held: &[Option<Binding>], list: &mut [(String, ParamBinding)]) {
        for ((_, binding), slot) in list.iter_mut().zip(&self.slots) {
            match binding {
                ParamBinding::Val(port) => {
                    if let Some(Binding::Local(l)) = slot.and_then(|s| held[s].as_ref()) {
                        *port = l.port;
                    }
                }
                ParamBinding::Record(fields) => {
                    if let Some(Binding::Record(now)) = slot.and_then(|s| held[s].as_ref()) {
                        fields.clone_from(now);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The value wired into `output_id`'s internal output node during this call.
fn output_wire_source(ctx: &LowerCtx<'_>, wire_start: usize, output_id: NodeId) -> Option<PortRef> {
    ctx.builder.module.wires[wire_start..]
        .iter()
        .find(|w| w.target.node_id == output_id && w.target.port == WirePort::RerInput)
        .map(|w| w.source)
}

/// Whether `t` is the `exec` type, written so or through a `type` alias.
pub(in crate::lower) fn is_exec_type(ctx: &LowerCtx<'_>, t: &TypeExpr) -> bool {
    match t {
        TypeExpr::Name { name, .. } if name == "exec" => true,
        TypeExpr::Name { name, range } => {
            matches!(ctx.alias_at(name, range), Some(TypeExpr::Name { name, .. }) if name == "exec")
        }
        _ => false,
    }
}

/// How one value output of an inline mod is written by name: the counts of
/// `emit name = v` and `out name = v`, and where those writes sit.
#[derive(Default, Clone, Copy)]
struct OutputWrites {
    emits: usize,
    outs: usize,
    /// A write under an `if`/`if let`/`let else`/`match` arm or a loop body,
    /// or after a statement that can end the body early.
    branch: bool,
    /// A write inside a nested `on` handler.
    handler: bool,
    /// A write on the body's own path that always runs.
    plain: bool,
}

/// Per value output, the `emit name = v` and `out name = v` writes anywhere in
/// the body, nested handlers, anonymous chips and loops included (not a
/// nested named chip's body).
fn count_output_writes(
    block: &Block,
    names: &[String],
    in_branch: bool,
    in_handler: bool,
    writes: &mut HashMap<String, OutputWrites>,
) {
    let start = walk::Ctx {
        branch: in_branch,
        handler: in_handler,
        ..walk::Ctx::default()
    };
    walk::walk(walk::Node::Block(block), start, &mut |n, cx| {
        let (name, emit) = match n {
            walk::Node::Stmt(Stmt::Emit(e)) if e.value.is_some() => (&e.name, true),
            walk::Node::Stmt(Stmt::OutBinding(o)) if o.value.is_some() => (&o.name, false),
            walk::Node::Stmt(Stmt::ChipDecl(_)) => return false,
            _ => return true,
        };
        if !names.contains(name) {
            return true;
        }
        let w = writes.entry(name.clone()).or_default();
        if emit {
            w.emits += 1;
        } else {
            w.outs += 1;
        }
        w.branch |= cx.branch && !cx.handler;
        w.handler |= cx.handler;
        w.plain |= !cx.branch && !cx.handler;
        true
    });
}

/// The caller's record of a multi-output inline call from its outputs' own
/// sources: a record output's field map (`out_records`), otherwise the value
/// wired into its internal output node.
fn gathered_outputs(
    ctx: &LowerCtx<'_>,
    chip_decl: &ChipDecl,
    out_records: &HashMap<String, Fields>,
    inline_output_ids: &[NodeId],
    wire_start: usize,
) -> Fields {
    let mut record: Fields = HashMap::default();
    for (i, out) in chip_decl.outputs.iter().enumerate() {
        // A record-typed output among several carries no wire into its
        // output node (a record has no single port), so take its field map
        // and nest it: `r.card.cardtype` then resolves through both hops.
        if let Some(rec) = out_records.get(&out.name) {
            record.insert(intern(&out.name), Binding::Record(rec.clone()));
            continue;
        }
        let Some(&out_id) = inline_output_ids.get(i) else {
            continue;
        };
        if let Some(src) = output_wire_source(ctx, wire_start, out_id) {
            record.insert(intern(&out.name), Binding::Local(LocalRecord { port: src }));
        }
    }
    record
}

/// Record every storage var of an output a pure call could not write, so its
/// first read reports WS007.
fn register_unsequenced(
    ctx: &mut LowerCtx<'_>,
    binding: &Binding,
    name: &str,
    range: &SourceRange,
    call: usize,
) {
    match binding {
        Binding::Var(var) => {
            ctx.unsequenced_reads
                .insert(var.node_id, (name.to_string(), range.clone(), call));
        }
        Binding::Record(fields) => {
            for b in fields.values() {
                register_unsequenced(ctx, b, name, range, call);
            }
        }
        _ => {}
    }
}
