//! Statement checking and assignment-target resolution.

use super::*;
use crate::analysis::types::{type_expr_str, type_str};
use crate::const_eval::bind_destructured;
use crate::const_scope::Admit;
use crate::ir::Literal;
use crate::ir::plane::{ForSlot, LoopKind, for_slots, plane_spec, slot_types};
use crate::types::mono::unwrap_ref;

/// WS082: a `for` loop's array/map source has a record-typed element, key or
/// value. Shared between the whole-element and destructured binding forms so
/// both produce byte-identical wording for the one real limitation.
const RECORD_ELEMENT_MESSAGE: &str = "a record-typed array or map is stored as one container \
     per field, so this loop has no single reference for its driver pin. Iterate the index \
     instead (`for i in 0..arr.length()`) and read the record's fields inside the body.";

pub(super) fn check_block(ctx: &mut TypeCheckCtx<'_>, block: &Block) {
    ctx.push_scope();
    for s in &block.stmts {
        check_stmt(ctx, s);
    }
    ctx.pop_scope();
}

/// What a `let`'s initializer evaluates to, split across the names its binding
/// introduces, and whether that value derives from a PLACEHOLDER (see
/// [`crate::const_scope::Admit::NoPlaceholders`]).
pub(super) struct LetValue {
    pairs: Result<Vec<(String, Literal)>, crate::const_eval::ConstError>,
    from_placeholder: bool,
    /// Computed through an integer operation the certified fold table has no
    /// result for (see `crate::const_scope::Marks::uncertified`).
    uncertified: bool,
}

/// Evaluate `l`'s initializer before its binding exists, so it reads the names
/// around the `let`, never the ones it binds. A value that evaluates is probed
/// again without placeholders: placeholder-ness is transitive (`const t = m + 1`
/// with a `const` parameter `m` evaluates only through `m`'s type-shaped zero),
/// and re-evaluating catches any read of a placeholder, however nested.
pub(super) fn evaluate_let(ctx: &TypeCheckCtx<'_>, l: &LetDecl) -> LetValue {
    let pairs = ctx
        .const_eval_let(l)
        .and_then(|lit| bind_destructured(&l.binding, lit));
    let from_placeholder = pairs.is_ok()
        && ctx
            .const_eval(&l.value, Admit::NoPlaceholders)
            .and_then(|lit| bind_destructured(&l.binding, lit))
            .is_err();
    let uncertified = pairs.is_ok() && !ctx.integer_ops_certified(&l.value);
    LetValue {
        pairs,
        from_placeholder,
        uncertified,
    }
}

/// Record what `l` bound, now that the binding exists: each name bound to a
/// constant goes in the innermost frame, so a constant-only config argument in
/// this scope resolves it as a top-level `let` does; every other bound name is
/// forgotten there. A `let` folds when it can and is otherwise a runtime value;
/// a `const` must fold, or it reports the evaluator's error (WS046, WS047,
/// WS048).
///
/// Only a single name or a `const` destructure is recorded, as `lower::decl`'s
/// `is_const` gate does: lowering's narrow non-`const` path cannot fold the
/// record a plain `let` destructure reads, so recording it here would let
/// `let { chan } = { chan: "evt" }` satisfy `SendCustomEvent(chan, v)`'s
/// constant slot and emit a gate with an empty channel.
pub(super) fn record_let(ctx: &mut TypeCheckCtx<'_>, l: &LetDecl, value: LetValue) {
    let single = matches!(l.binding, LetBinding::Ident { .. });
    match value.pairs {
        Ok(pairs) if single || l.is_const => {
            let marks = crate::const_scope::Marks {
                declared: l.is_const,
                placeholder: value.from_placeholder,
                uncertified: value.uncertified,
            };
            for (name, lit) in pairs {
                ctx.consts.record(&name, lit, marks);
            }
            check_const_recorded(ctx, l);
        }
        Ok(pairs) => {
            for (name, _) in pairs {
                ctx.consts.forget(&name);
            }
        }
        Err(err) => {
            for name in crate::const_eval::bound_names(&l.binding) {
                ctx.consts.forget(&name);
            }
            if l.is_const && !err.reported_elsewhere() {
                ctx.emit(err.code(), err.message(), err.range.clone());
            }
        }
    }
}

/// WS116 (a warning): `on a || b` desugars to `let _on_expr_N = a || b`
/// (`parse_handler`). With an object operand that reads as "either event", but
/// `||` makes one value, and an object is true as long as it exists, so
/// `on reset || rider` (a `character` pin) stays true once a rider is wired and
/// never fires on `reset`; `Union(a, b)` fires on either. `&&` with an object
/// is the ordinary "is set" guard, and an `exec` operand pulses for a tick
/// (`on reset || start` works), so neither is reported.
pub(super) fn reject_event_logic_trigger(ctx: &mut TypeCheckCtx<'_>, l: &LetDecl) {
    let LetBinding::Ident { name, .. } = &l.binding else {
        return;
    };
    if !name.starts_with("_on_expr_") {
        return;
    }
    let Expr::BinOp {
        op,
        left,
        right,
        range,
    } = &l.value
    else {
        return;
    };
    if op != "||" {
        return;
    }
    let object = |e: &Expr| {
        matches!(
            ctx.recorded_type(e.range()),
            Some(Type::Entity | Type::Controller | Type::Character)
        )
    };
    if object(left) || object(right) {
        ctx.warn(
            "WS116",
            "`||` here makes one value, it does not wait for either side: an object is true for as \
             long as it exists, so this handler fires once, when the object appears, and never again. \
             To fire on either, write `on Union(a, b)`",
            range.clone(),
        );
    }
}

/// Whether `block` diverges on every path, so exec cannot fall through it (the
/// `else` of a `let ... else` must, WS062). Conservative: its last statement is
/// a `return` or an `emit`, or an `if`, `if let` or `match` whose branches all
/// diverge. Anything else, an empty block included, does not.
fn block_diverges(block: &Block) -> bool {
    block_ends(block, true)
}

/// Does `block` `return` on every path: [`block_diverges`] without counting an
/// `emit`, after which the chain goes on.
pub(super) fn block_returns(block: &Block) -> bool {
    block_ends(block, false)
}

fn block_ends(block: &Block, emit_ends: bool) -> bool {
    match block.stmts.last() {
        Some(Stmt::Return { .. }) => true,
        Some(Stmt::Emit(_)) => emit_ends,
        Some(Stmt::If(i)) => i
            .else_block
            .as_ref()
            .is_some_and(|eb| block_ends(&i.then_block, emit_ends) && block_ends(eb, emit_ends)),
        Some(Stmt::IfLet(i)) => i
            .else_block
            .as_ref()
            .is_some_and(|eb| block_ends(&i.then_block, emit_ends) && block_ends(eb, emit_ends)),
        Some(Stmt::ExprStmt(es)) => match &es.expr {
            Expr::MatchExpr { arms, .. } => {
                !arms.is_empty()
                    && arms.iter().all(|a| match &a.body {
                        MatchBody::Block(b) => block_ends(b, emit_ends),
                        MatchBody::Expr(_) => false,
                    })
            }
            _ => false,
        },
        _ => false,
    }
}

pub(super) fn check_anon_chip_stmts(
    ctx: &mut TypeCheckCtx<'_>,
    stmts: &[Stmt],
    pre_registered: bool,
) {
    // At the top level, a declaration a name the chip declares shares is
    // what the name means again after the chip, and inside the chip no read
    // takes the name for a top-level constant (see `scope::top`).
    let top_level = ctx.scope.depth() == 1;
    let names = if top_level {
        crate::scope::top::declared_names(stmts)
    } else {
        Vec::new()
    };
    let within = |r: &SourceRange| {
        stmts.first().zip(stmts.last()).is_some_and(|(a, b)| {
            let (a, b) = (a.range(), b.range());
            r.file == a.file && a.start.offset <= r.start.offset && r.end.offset <= b.end.offset
        })
    };
    let earlier: Vec<(String, SymbolInfo)> = names
        .iter()
        .filter_map(|name| {
            let now = ctx
                .scope
                .binding_frame(name)
                .map(|(_, s)| s)
                .filter(|now| !within(&now.decl_range))?;
            Some((name.clone(), now.clone()))
        })
        .collect();
    let hidden = top_level.then(|| ctx.consts.hide_at_top(names.iter().cloned()));
    if !pre_registered {
        register_anon_chip_decls(ctx, stmts);
    }
    // The chip's own storage is what its statements name, whatever the top
    // level declared of the name before the chip.
    for s in stmts {
        if let Some((name, range)) = super::register::anon_chip_storage_decl(s)
            && let Some(sym) = ctx.anon_chip_storage.get(&range.start_key()).cloned()
            && ctx
                .scope
                .lookup(name)
                .is_none_or(|now| now.decl_range != *range)
        {
            ctx.scope.declare(name, sym);
        }
    }
    // An anonymous chip is a physical microchip, so its `in`/`out` cross
    // ports even inside a mod body.
    let saved_inline = std::mem::replace(&mut ctx.inline_body_depth, 0);
    for s in stmts {
        // A chip's pure statements (`let`, `var`, `out`, `buffer`, declarations) are
        // continuous signal flow, so they are checked in pure context and
        // `var_read_contexts` records them as pure, as `lower_chip_body` lowers them.
        // That holds inside an exec-wrapped chip too (a top-level chip after a
        // handler), whose exec statements stay valid, except for a declaration whose
        // initializer calls something inside an exec body: a call has no continuous
        // reading, so it runs on the chain.
        let joins_chain = ctx.exec_mode() == ExecMode::Exec && chip_stmt_init_calls(s);
        if !joins_chain
            && matches!(
                s,
                Stmt::Let(_)
                    | Stmt::Var(_)
                    | Stmt::Buffer(_)
                    | Stmt::OutBinding(_)
                    | Stmt::Array(_)
                    | Stmt::In(_)
            )
        {
            ctx.in_pure(|ctx| check_one_chip_stmt(ctx, s));
        } else {
            check_one_chip_stmt(ctx, s);
        }
    }
    ctx.inline_body_depth = saved_inline;
    for (name, sym) in earlier {
        ctx.scope.declare(&name, sym);
    }
    if let Some(mark) = hidden {
        ctx.consts.unhide_at_top(mark);
    }
}

/// Whether a chip-body declaration's initializer contains a call. Must agree
/// with lowering's `chip_stmt_init_calls`, or one phase emits a gate the other
/// refuses to check.
fn chip_stmt_init_calls(s: &Stmt) -> bool {
    let has_call = crate::analysis::visit::contains_call;
    match s {
        Stmt::Let(l) => has_call(&l.value),
        Stmt::Var(v) => v.init.as_ref().is_some_and(has_call),
        Stmt::OutBinding(o) => o.value.as_ref().is_some_and(has_call),
        _ => false,
    }
}

fn check_one_chip_stmt(ctx: &mut TypeCheckCtx<'_>, s: &Stmt) {
    match s {
        Stmt::Var(v) => check_decl(ctx, &TopDecl::Var(v.clone())),
        Stmt::Buffer(b) => check_decl(ctx, &TopDecl::Buffer(b.clone())),
        Stmt::Array(a) => check_decl(ctx, &TopDecl::Array(a.clone())),
        Stmt::In(i) => check_decl(ctx, &TopDecl::In(i.clone())),
        other => check_stmt(ctx, other),
    }
}

/// Whether this statement carries `@nofold`: the statements
/// `lower::stmt::lower_stmt` wraps in `ctx.with_nofold` (Await, `OutBinding`,
/// Handler, Let, Var) plus `ChipDecl`, whose body `build_chip_module` seeds. A
/// statement without a `no_fold` field reports `false`.
fn stmt_no_fold(s: &Stmt) -> bool {
    match s {
        Stmt::Await(a) => a.no_fold,
        Stmt::OutBinding(b) => b.no_fold,
        Stmt::Handler(h) => h.no_fold,
        Stmt::Let(l) => l.no_fold,
        Stmt::Var(v) => v.no_fold,
        Stmt::ChipDecl(c) => c.no_fold,
        _ => false,
    }
}

pub(super) fn check_stmt(ctx: &mut TypeCheckCtx<'_>, s: &Stmt) {
    ctx.with_nofold(stmt_no_fold(s), |ctx| check_stmt_inner(ctx, s));
    // A `let` declared with a type naming `T`, or taking a value declared
    // so, is declared with that type parameter too.
    if let Stmt::Let(l) = s
        && let LetBinding::Ident { name, .. } = &l.binding
        && !ctx.unbounded_type_params.is_empty()
    {
        let tp = match &l.typ {
            Some(te) => ctx
                .unbounded_type_params
                .iter()
                .find(|p| super::enums::type_expr_mentions(te, p))
                .cloned(),
            None => ctx.unbounded_root_of(&l.value).map(str::to_string),
        };
        if let Some(tp) = tp
            && let Some(decl) = ctx.scope.lookup(name).map(|sym| sym.decl_range.clone())
        {
            ctx.unbounded_typed_params.push((decl, tp));
        }
    }
}

fn check_stmt_inner(ctx: &mut TypeCheckCtx<'_>, s: &Stmt) {
    match s {
        Stmt::Var(v) => {
            // `var x = <init reading x>`: the initializer reads the `x` around
            // the declaration, as a `let`'s does, so the var is declared under
            // a name of its own and `x` names it once it exists.
            let here = ctx.scope.depth() - 1;
            if ctx
                .scope
                .binding_frame(&v.name)
                .is_none_or(|(i, _)| i != here)
                && let Some(apart) = v.declared_apart()
            {
                let mark = ctx.diagnostics.len();
                check_stmt_inner(ctx, &Stmt::Var(apart.clone()));
                // Nothing around the declaration has the name it reads.
                let unknown = format!("unknown identifier '{}'", v.name);
                // The declaration as written, or with the type its initializer
                // gives it when that type is known.
                let ty = match &v.typ {
                    Some(te) => Some(type_expr_str(te)),
                    None => ctx
                        .scope
                        .lookup(&apart.name)
                        .map(|sym| unwrap_ref(&sym.ty))
                        .filter(|t| !matches!(t, Type::Any | Type::Opaque | Type::Never))
                        .map(|t| type_str(&t)),
                };
                let declared = ty.map_or(String::new(), |ty| {
                    format!(
                        " (`{}var {}: {ty}`)",
                        if v.is_static { "static " } else { "" },
                        v.name
                    )
                });
                for d in &mut ctx.diagnostics[mark..] {
                    if d.code == "WS002" && d.message == unknown {
                        d.message = format!(
                            "{unknown}: `{}` is not declared before this `var`, and a `var`'s initializer \
                             cannot read the variable it declares. Declare it without one{declared}, \
                             or initialize it from another value",
                            v.name
                        );
                    }
                }
                if let Some(mut info) = ctx.scope.lookup(&apart.name).cloned() {
                    info.name.clone_from(&v.name);
                    ctx.scope.declare(&v.name, info);
                }
                ctx.consts.forget(&v.name);
                return;
            }
            register_decl(ctx, &TopDecl::Var(v.clone()));
            ctx.consts.forget(&v.name);
            if let Some(ty) = ctx.scope.lookup(&v.name).map(|s| s.ty.clone()) {
                check_aggregate_record(ctx, &ty, &v.range);
            }
            // A local `var` resets every run, and a ref has no literal to reset
            // to, so without an initializer it would keep the last run's ref.
            if !v.is_static
                && v.init.is_none()
                && ctx.exec_mode() == ExecMode::Exec
                && let Some(SymbolInfo {
                    ty: Type::Ref(held),
                    ..
                }) = ctx.scope.lookup(&v.name).cloned()
                && matches!(held.as_ref(), Type::Ref(_) | Type::Brick)
            {
                ctx.emit(
                    "WS025",
                    format!(
                        "'{}' holds a {}, and a var declared here resets every run to its \
                        initializer; a ref has none to reset to. Give it one \
                        (`var {} = ...`) or make it `static var`",
                        v.name, held, v.name
                    ),
                    v.range.clone(),
                );
            }
            // Statement-level vars inherit the current exec context (not forced pure)
            // so that `var x: int = arr[i]` works inside handlers.
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                if let Some(entries) = map_init_entries(init)
                    && let Type::Map(k, v_ty) = &inner
                {
                    // A map-valued `var` initializer inside a handler: validate
                    // entries directly (the valid initializer slot) instead of
                    // routing through the generic `infer` position guard.
                    // An empty `{}` is an empty-map init.
                    check_map_literal(ctx, entries, k, v_ty);
                    ctx.record_expr_type(init.range(), &inner);
                } else {
                    // An unannotated array literal with an element that is not
                    // a literal registers from its literal elements alone (or
                    // as `any[]`); its elements' own types decide it here.
                    let untyped_array = v.typ.is_none()
                        && (inner == Type::Array(Box::new(Type::Any))
                            || matches!(init, Expr::Array { elements, .. } if elements.iter().any(|el| {
                                !matches!(el, ArrayElem::Item(e) if literal_expr_type(e).is_some())
                            })));
                    let inner = if untyped_array { Type::Any } else { inner };
                    let t = infer::check_stored_value(ctx, init, &inner);
                    // Reject a void-mutation `Never` init on an unannotated var
                    // (annotated already mismatches via `check`), same as the
                    // top-level decl and `let` paths.
                    if v.typ.is_none() {
                        reject_never_value(ctx, &unwrap_ref(&t), init.range(), &v.name);
                    }
                    // Refine an unannotated var's placeholder `any` from a
                    // non-literal init, same as the top-level decl path.
                    if v.typ.is_none() && matches!(inner, Type::Any) {
                        // A ref value is what the var holds, not a place to read.
                        let u = if matches!(t, Type::Ref(_)) {
                            t
                        } else {
                            unwrap_ref(&t).held_by_var()
                        };
                        let storable = match &u {
                            Type::Array(elem) => {
                                untyped_array && super::untyped_arrays::inferable_element(elem)
                            }
                            other => super::resolve::unannotated_var_holds(other),
                        };
                        if storable {
                            ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                        }
                    }
                }
            }
            super::untyped_arrays::declare(ctx, v);
        }
        Stmt::Buffer(b) => {
            register_decl(ctx, &TopDecl::Buffer(b.clone()));
            check_decl(ctx, &TopDecl::Buffer(b.clone()));
        }
        Stmt::Array(a) => {
            register_decl(ctx, &TopDecl::Array(a.clone()));
            check_decl(ctx, &TopDecl::Array(a.clone()));
        }
        Stmt::Map(m) => {
            register_decl(ctx, &TopDecl::Map(m.clone()));
            check_decl(ctx, &TopDecl::Map(m.clone()));
        }
        Stmt::Let(l) => {
            let t = infer_let_init(ctx, l);
            reject_event_logic_trigger(ctx, l);
            if !reject_never_binding(ctx, l, &t) {
                check_let_type_annotation(ctx, l, &t);
            }
            record_single_output_alias(ctx, &l.binding, &l.value);
            record_let_ref_values(ctx, &l.binding, &l.value);
            let value = evaluate_let(ctx, l);
            let bound = let_binding_type(ctx, l, &t);
            bind_let(ctx, &l.binding, &bound, &l.value);
            record_let(ctx, l, value);
        }
        Stmt::LetElse(l) => {
            // A refutable pattern can never be `const`: `let <pattern> = e else`
            // may fail to match, so its binding is not a compile-time
            // constant. The parser accepts the `const` and drops it, so the
            // rejection has to happen here.
            if l.is_const {
                ctx.emit(
                    "WS046",
                    "a refutable pattern cannot be `const`: `let <pattern> = ... else` \
                     may fail to match, so its binding is not a compile-time constant",
                    l.range.clone(),
                );
            }
            let scrut_ty = infer::infer_scrutinee(ctx, &l.scrutinee);
            let value_match = infer::is_matchable_value(&scrut_ty);
            if !matches!(scrut_ty, Type::Enum { .. }) && !value_match {
                ctx.emit(
                    "WS066",
                    format!(
                        "`let ... else` tests an enum, int, float, bool or string, but this is {}",
                        type_str(&scrut_ty)
                    ),
                    l.scrutinee.range().clone(),
                );
            }
            // The `else` block runs only when the pattern fails; its captures are
            // NOT in scope there. Check it in its own scope, then require it to
            // diverge on every path - the binding is unavailable past this point
            // on the non-match path, so a fall-through is WS062.
            check_block(ctx, &l.else_block);
            if !block_diverges(&l.else_block) {
                ctx.emit(
                    "WS062",
                    "the `else` block of a `let ... else` must diverge on every path \
                     (end in `return`/`emit`, or an `if`/`match` whose arms all diverge)",
                    l.else_block.range.clone(),
                );
            }
            // Bind the pattern's captures into the CURRENT scope (the
            // continuation), so the rest of the enclosing block can read them.
            let mutable = !value_match && scrutinee_is_mutable(ctx, &l.scrutinee);
            infer::check_match_pattern(ctx, &l.pattern, &scrut_ty, mutable);
            if value_match {
                infer::check_value_pattern_root(ctx, &l.pattern, &scrut_ty);
            }
            for name in crate::lower::matchtree::capture_names(&l.pattern) {
                ctx.consts.forget(&name);
            }
        }
        Stmt::Assign(a) => {
            ctx.require_exec(
                format_args!(
                    "var write '{}'",
                    target_name(&a.target).unwrap_or("<expr>".into())
                ),
                &a.range,
            );
            if super::props::check_property_write(ctx, &a.target, &a.value) {
                // A property write, checked against the property map.
            } else {
                let (target_ty, through_ref) = match &a.target {
                    Expr::Deref { operand, range } => infer_deref_target(ctx, operand, range),
                    target => (infer_assign_target(ctx, target), false),
                };
                if let Some(entries) = map_init_entries(&a.value)
                    && let Type::Map(k, v) = &target_ty
                {
                    // A map literal assigned to a map var is the other valid slot: its entries
                    // are validated directly rather than through `infer`'s position guard. An
                    // empty `{}` empties the map.
                    check_map_literal(ctx, entries, k, v);
                    ctx.record_expr_type(a.value.range(), &target_ty);
                } else {
                    infer::check_stored_value(ctx, &a.value, &target_ty);
                    if through_ref {
                        check_deref_write_is_exact(ctx, &a.value, &target_ty);
                    }
                }
            }
        }
        Stmt::OutBinding(b) => {
            // A port of the enclosing chip's signature had its type checked where
            // it was declared; only a port this statement declares is new.
            let boundary = ctx.out_ctx.len() <= 1;
            let declared = !boundary
                && ctx
                    .out_ctx
                    .last()
                    .is_some_and(|o| o.iter().any(|f| f.name == b.name));
            // An anon-chip's statement-level `out` carries a type annotation
            // too (`chip { @bottom out done: any = 5 }`). Like the top-level
            // `TopDecl::Out` it is a port, which cannot take a type parameter,
            // so `any` there is no WS032.
            if let Some(te) = &b.typ {
                let resolved = resolve_type_expr(ctx, te);
                if let Some(value) = &b.value {
                    // An annotated `out` must accept its value (WS003 on a mismatch; coercions
                    // such as string to bool and primitive to string pass). A `*T` port carries a
                    // ref, never the value it points at, as for `TopDecl::Out`.
                    let value_ty = infer::infer(ctx, value);
                    if ctx.inline_body_depth == 0 && !declared {
                        check_port_record(ctx, &b.name, &resolved, &b.range, boundary);
                    }
                    infer::coerce_or_emit(
                        ctx,
                        &value_ty.clone().container_ref_as_value(),
                        &resolved.clone().container_ref_as_value(),
                        value.range(),
                    );
                    // Two more things must hold, neither implied by the annotation check: what
                    // the site claims to deliver must fit the port (`out r: string = v` on an
                    // `int` port would leave the conversion unwired), and the value must fit the
                    // port on any leg the annotation does not cover.
                    check_annotated_port_write(ctx, &b.name, &resolved, &value_ty, value.range());
                    check_port_declaration(ctx, &b.name, &resolved, &type_expr_range(te));
                    infer::int_constants_in_slot(ctx, value, &resolved);
                }
            } else if let Some(value) = &b.value {
                // No annotation of its own, so the port's declared type is the
                // only thing the value answers to.
                let value_ty = infer::infer(ctx, value);
                if ctx.inline_body_depth == 0 && !declared {
                    check_port_record(ctx, &b.name, &value_ty, &b.range, boundary);
                }
                check_port_write_value(ctx, &b.name, value, &value_ty);
                check_untyped_port_write(ctx, &b.name, value, &value_ty);
                port_write_literals(ctx, &b.name, value);
            }
        }
        Stmt::Emit(e) => {
            if e.value.is_none() {
                ctx.require_exec(format_args!("emit '{}'", e.name), &e.range);
            }
            if let Some(ref val) = e.value {
                let t = infer::infer(ctx, val);
                // An `emit` with a payload drives its output's backing
                // variable exactly the way an `out` binding does, so the
                // payload answers to the same declared type. A local exec
                // signal has no declared type and passes straight through.
                check_port_write_value(ctx, &e.name, val, &t);
                check_untyped_port_write(ctx, &e.name, val, &t);
                port_write_literals(ctx, &e.name, val);
                // Remember the ferried payload type so a later
                // `let { .. } = await sig` can type its destructured fields.
                // A gate read ferries its first output, as does each field
                // of a record literal, which ferries field by field.
                let payload = match &t {
                    Type::Record(fs) if matches!(val, Expr::RecordLit { .. }) => Type::Record(
                        fs.iter()
                            .map(|(n, ft)| (n.clone(), infer::first_output(ft)))
                            .collect(),
                    ),
                    other => infer::first_output(other),
                };
                ctx.signal_payload_types.insert(e.name.clone(), payload);
            }
        }
        Stmt::Await(a) => {
            // A loop body is one synchronous sweep within a single tick, so
            // nothing in it can suspend across ticks - every spelling of
            // `await` (a bare wait, a captured value, `await Sleep(...)`)
            // reaches this same node.
            if ctx.loop_depth > 0 {
                ctx.emit(
                    "WS079",
                    "a loop body runs synchronously within one tick, so it cannot \
                     suspend. Move the `await` outside the loop, or use a back-edge \
                     loop (`emit` + `await`) if you need work spread across ticks.",
                    a.range.clone(),
                );
            }
            ctx.require_exec("await", &a.range);
            // Push scope with `_` as Bool (the armed flag) for exec expression
            ctx.push_scope();
            ctx.scope
                .declare_new(SymbolKind::LetBinding, "_", Type::Bool, a.range.clone());
            let exec_ty = infer::infer(ctx, &a.exec_expr);
            ctx.pop_scope();
            let val_ty = if let Some(ref val) = a.value_expr {
                infer::infer(ctx, val)
            } else {
                exec_ty
            };
            // Awaiting a CustomEvent or GlobalCustomEvent captures its data outputs
            // (`DataOut1..8`): `let x = await CustomEvent("c")` binds the first, and
            // `let (p, t) = ...` binds them by position. The gate's data ports are `any`
            // unless typed, so an untyped capture is a float wire that mis-delivers other
            // data; `let x: T` types it, and an untyped one gets the WS055 nudge.
            // `let v = await x on CustomEvent(...)` is the plain trigger form.
            let event_capture = is_custom_event_call(&a.exec_expr) && a.value_expr.is_none();

            if let Some(ref binding) = a.binding {
                let ty = if event_capture {
                    match &a.binding_type {
                        Some(te) => resolve_type_expr(ctx, te),
                        None => {
                            ctx.warn(
                                "WS055",
                                format!(
                                    "the awaited event's data type can't be inferred here - annotate it \
                                     (`let {binding}: T = await CustomEvent(\"...\")`); an untyped capture \
                                     wires as a float and mis-delivers non-float data"
                                ),
                                a.range.clone(),
                            );
                            Type::Any
                        }
                    }
                } else {
                    val_ty
                };
                ctx.scope
                    .declare_new(SymbolKind::LetBinding, binding, ty, a.range.clone());
            }
            // `let (p, t) = await CustomEvent(...)`: positional data-output capture.
            // No per-field annotation surface, so each is `any` with one WS055 nudge.
            if let Some(ref names) = a.tuple_destructure {
                if event_capture {
                    ctx.warn(
                        "WS055",
                        "tuple-captured event data is untyped (each field wires as a float) - capture typed \
                         values one at a time (`let p: T = await CustomEvent(\"...\")`), or receive them with \
                         a handler (`on CustomEvent(\"...\") -> (p: T, ...)`)",
                        a.range.clone(),
                    );
                }
                for local in names {
                    ctx.scope.declare_new(
                        SymbolKind::LetBinding,
                        local,
                        Type::Any,
                        a.range.clone(),
                    );
                }
            }
            // `let { a, b } = await sig`: type each destructured local from the
            // signal's recorded ferried-payload record (Any when unknown).
            if let Some(ref fields) = a.destructure {
                let payload_ty = match &a.exec_expr {
                    Expr::Ident { name, .. } => ctx.signal_payload_types.get(name).cloned(),
                    _ => None,
                };
                for (field, local) in fields {
                    let fty = match &payload_ty {
                        Some(Type::Record(fs) | Type::Outputs(fs)) => fs
                            .iter()
                            .find(|(n, _)| n == field)
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Type::Any),
                        _ => Type::Any,
                    };
                    ctx.scope
                        .declare_new(SymbolKind::LetBinding, local, fty, a.range.clone());
                }
            }
        }
        Stmt::If(i) => {
            ctx.require_exec("'if' statement", &i.range);
            ctx.if_contexts
                .insert(i.range.start_key(), ctx.exec_mode() == ExecMode::Exec);
            let ct = infer::infer(ctx, &i.cond);
            infer::record_read_as_one_value(ctx, &i.cond, &ct);
            // A constant condition is resolved here with the evaluator `lower_if` uses,
            // so both stages agree on which conditions are decidable and which way they
            // go (see `dropped_ranges`). The untaken block is not type-checked: it need
            // not be valid for this value, which is the point. Dropping a block here that
            // lowering still emits would ship unchecked code, so four guards keep the two
            // in step:
            //
            // 1. `nofold_depth == 0`, as in `lower_if`: under `@nofold` lowering builds a
            //    real `Branch` and lowers both blocks.
            // 2. No placeholders: a `const` parameter is a type-shaped zero while a mod
            //    body is checked, and lowering inlines the real argument, which may go the
            //    other way. Without placeholders such a condition does not evaluate, and
            //    both blocks are checked.
            // 3. `const`-declared names only (`Admit::Declared`): a plain `let` that
            //    happens to fold does not get this elision, so a program without `const`
            //    compiles as if the feature did not exist.
            // 4. Scope: a parameter, local, loop variable or pattern binding named like
            //    an outer constant is that binding (`crate::const_scope`), so the
            //    condition does not evaluate.
            let cond_result = if ctx.nofold_depth > 0 {
                None
            } else {
                ctx.const_eval(&i.cond, Admit::Declared).ok()
            };
            // `if x.getType() is Type.Int { ... }`: the block sees `x` as an int.
            let check_then = |ctx: &mut TypeCheckCtx<'_>| {
                let narrowed = super::narrow::push_if(ctx, &i.cond);
                check_block(ctx, &i.then_block);
                super::narrow::pop(ctx, narrowed);
            };
            match cond_result {
                Some(Literal::Bool(true)) => {
                    check_then(ctx);
                    if let Some(else_b) = &i.else_block {
                        ctx.dropped_ranges.push((
                            else_b.range.clone(),
                            format!("`{}` is true here", describe_cond(&i.cond)),
                        ));
                    }
                }
                Some(Literal::Bool(false)) => {
                    ctx.dropped_ranges.push((
                        i.then_block.range.clone(),
                        format!("`{}` is false here", describe_cond(&i.cond)),
                    ));
                    if let Some(else_b) = &i.else_block {
                        check_block(ctx, else_b);
                    }
                }
                _ => {
                    check_then(ctx);
                    if let Some(else_b) = &i.else_block {
                        check_block(ctx, else_b);
                    }
                }
            }
        }
        Stmt::IfLet(i) => {
            let scrut_ty = infer::infer_scrutinee(ctx, &i.scrutinee);
            let value_match = infer::is_matchable_value(&scrut_ty);
            if !matches!(scrut_ty, Type::Enum { .. }) && !value_match {
                ctx.emit(
                    "WS066",
                    format!(
                        "`if let` tests an enum, int, float, bool or string, but this is {}",
                        type_str(&scrut_ty)
                    ),
                    i.scrutinee.range().clone(),
                );
            }
            // The captures are scoped to the THEN block only. Bind them in a
            // fresh scope, then check the then-block's statements in it (mirrors
            // how `Expr::MatchExpr` scopes an arm's captures to its body).
            let mutable = !value_match && scrutinee_is_mutable(ctx, &i.scrutinee);
            ctx.push_scope();
            infer::check_match_pattern(ctx, &i.pattern, &scrut_ty, mutable);
            if value_match {
                infer::check_value_pattern_root(ctx, &i.pattern, &scrut_ty);
            }
            for s in &i.then_block.stmts {
                check_stmt(ctx, s);
            }
            ctx.pop_scope();
            // The `else` block never sees the captures - a plain block scope.
            if let Some(else_b) = &i.else_block {
                check_block(ctx, else_b);
            }
        }
        Stmt::ExprStmt(es) => {
            // A line that starts with `*` is a new statement, a deref, never
            // the continuation of the line above (`n = a` / `* b`), so on its
            // own it reads a value nothing uses.
            if let Expr::Deref { operand, range } = &es.expr {
                ctx.warn(
                    "WSP001",
                    format!(
                        "`*{}` reads a value that nothing uses: a line that starts with `*` \
                         begins a new statement (a deref), it does not continue the line \
                         above. To multiply across lines, end the first line with `*`",
                        target_name(operand).unwrap_or("<expr>".into())
                    ),
                    range.clone(),
                );
            }
            // A `match` statement runs its arms for their effects, so an arm
            // that is only a value computes it for nothing.
            if let Expr::MatchExpr { arms, .. } = &es.expr {
                for arm in arms {
                    if let MatchBody::Expr(v) = &arm.body
                        && is_plain_value(v)
                    {
                        ctx.emit(
                            "WS092",
                            "this `match` arm is a value in statement position, which does \
                             nothing: in a `match` used as a statement each arm runs for its \
                             effects. Write the arm as a block (`A => { ... }`), or use the \
                             `match` as a value (`let r = match ...`)",
                            v.range().clone(),
                        );
                    }
                }
            }
            ctx.statement_match = matches!(es.expr, Expr::MatchExpr { .. });
            infer::infer(ctx, &es.expr);
            ctx.statement_match = false;
        }
        Stmt::In(i) => {
            register_decl(ctx, &TopDecl::In(i.clone()));
            if ctx.inline_body_depth == 0
                && let Some(ty) = ctx.scope.lookup(&i.name).map(|s| s.ty.clone())
            {
                check_port_record(ctx, &i.name, &ty, &i.range, ctx.out_ctx.len() <= 1);
            }
        }
        Stmt::Handler(h) => {
            ctx.push_scope();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body));
            ctx.pop_scope();
        }
        Stmt::AnonChip(ac) => {
            // An anonymous chip shares its parent's scope: register and check it inline.
            check_anon_chip_stmts(ctx, &ac.body.stmts, false);
        }
        Stmt::ChipDecl(c) => {
            register_decl(ctx, &TopDecl::Chip(c.clone()));
            check_decl(ctx, &TopDecl::Chip(c.clone()));
        }
        Stmt::Return { value, range } => {
            // A loop body is its own microchip plane, so `return` there would
            // have to leave the plane rather than the enclosing mod/handler,
            // and there is no gate that does that. Non-fatal, so the rest still gets checked.
            if ctx.loop_depth > 0 {
                ctx.emit(
                    "WS078",
                    "`return` cannot cross a loop plane: this `for` body is its own microchip, \
                     so `return` here would have to leave the loop rather than the enclosing \
                     mod/handler. Write the value to a `var` declared before the loop and \
                     `return` it after the loop ends.",
                    range.clone(),
                );
            }
            if value.is_none() {
                ctx.require_exec("'return' (without value)", range);
            }
            if let Some(expr) = value
                && ctx.out_ctx.len() <= 1
            {
                // Outside every mod and chip there is no output for the value:
                // the module's `out` ports are written by name.
                infer::infer(ctx, expr);
                ctx.emit(
                    "WS099",
                    "`return <value>` outside a `mod` or `chip` has no output to give the value \
                     to: only a mod or chip returns one. Write it to an `out` port by name \
                     (`emit o = value`), then `return` with no value.",
                    range.clone(),
                );
            } else if let Some(expr) = value {
                // `return <value>` is a port write: lowering wires it straight
                // into the enclosing boundary's output when there is exactly
                // one. That includes a handler-declared port, which is why the
                // output is resolved by `check_return_value` rather than read
                // off the current frame.
                check_return_value(ctx, expr);
            }
        }
        Stmt::For(f) => {
            // WS074: the loop shell needs an `Exec` pin, so a `for` is
            // exec-only. Non-fatal, like every other exec-context violation:
            // the rest of the loop still gets checked, so a real problem
            // inside it is not hidden behind this one.
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS074",
                    "`for` runs on an exec chain, so it must sit inside an `on` \
                     handler or an exec `mod`/`chip` body.",
                    f.range.clone(),
                );
            }

            // Slot names, types and the single-binding rule all come from
            // `ir::plane`, shared with lowering. A second copy could disagree
            // silently: the wire would still be drawn, just typed wrong.
            let (kind, elem): (LoopKind, Vec<Type>) = match &f.source {
                ForSource::Range(r) => {
                    check_for_range(ctx, r);
                    (LoopKind::CountFor, Vec::new())
                }
                ForSource::Collection { expr, range } => {
                    let ty = unwrap_ref(&infer::infer(ctx, expr));
                    match ty {
                        // A record-typed array/map has no single reference
                        // for the loop's driver pin, in either binding form,
                        // so this is caught once here for both.
                        Type::Array(elem) if is_variable_ref(ctx, expr) => {
                            if matches!(*elem, Type::Record(_)) {
                                ctx.emit("WS082", RECORD_ELEMENT_MESSAGE, range.clone());
                                return;
                            }
                            ctx.record_container_kind(expr, &Type::Array(elem.clone()));
                            (LoopKind::ForEachArray, vec![*elem])
                        }
                        Type::Map(k, v) if is_variable_ref(ctx, expr) => {
                            if matches!(*k, Type::Record(_)) || matches!(*v, Type::Record(_)) {
                                ctx.emit("WS082", RECORD_ELEMENT_MESSAGE, range.clone());
                                return;
                            }
                            ctx.record_container_kind(expr, &Type::Map(k.clone(), v.clone()));
                            (LoopKind::ForEachMap, vec![*k, *v])
                        }
                        _ => {
                            ctx.emit(
                                "WS075",
                                "`for` iterates an array or map VARIABLE, not a value. \
                                 Take the collection as a `T[]`/`Map<K, V>` parameter, \
                                 which reads the caller's own storage, or fill a declared \
                                 `var` array/map and iterate that.",
                                range.clone(),
                            );
                            return;
                        }
                    }
                }
            };
            let spec = plane_spec(kind);
            let slot_tys = slot_types(kind, &elem);
            let slots = for_slots(kind, &f.bindings);

            // WS076: more names than the loop has slots to bind them to.
            if slots.contains(&ForSlot::OverBound) {
                ctx.emit(
                    "WS076",
                    format!(
                        "this loop binds {} name(s) but has only {} slot(s) ({}).",
                        f.bindings.len(),
                        spec.slot_labels.len(),
                        spec.slot_labels.join(", ")
                    ),
                    f.range.clone(),
                );
                return;
            }

            ctx.push_scope();
            // A name taken from a container declared with a type naming an
            // unbounded `T` is declared with it too.
            let over = match &f.source {
                ForSource::Collection { expr, .. } => {
                    ctx.unbounded_root_of(expr).map(str::to_string)
                }
                ForSource::Range(_) => None,
            };
            for (pat, slot) in f.bindings.iter().zip(&slots) {
                match slot {
                    ForSlot::RangeSlotNames => {
                        if let ForPat::Record { fields, .. } = pat {
                            bind_range_slot_names(ctx, fields, spec, &slot_tys);
                        }
                    }
                    ForSlot::Slot(i) => bind_for_pat(ctx, pat, &slot_tys[*i]),
                    ForSlot::OverBound => {}
                }
                if let (Some(tp), ForPat::Name(name, _)) = (&over, pat)
                    && let Some(decl) = ctx.scope.lookup(name).map(|sym| sym.decl_range.clone())
                {
                    ctx.unbounded_typed_params.push((decl, tp.clone()));
                }
            }
            if f.sliced && ctx.loop_depth > 0 {
                ctx.warn(
                    "WS119",
                    crate::lower::for_loop::SLICED_NESTED.to_string(),
                    f.range.clone(),
                );
            }
            ctx.loop_depth += 1;
            check_block(ctx, &f.body);
            ctx.loop_depth -= 1;
            ctx.pop_scope();

            // `let run = for ...` binds the outcome record AFTER the loop, in
            // the scope the `for` statement itself sits in, not the body scope
            // just popped above.
            if let Some((name, name_range)) = &f.result {
                declare_for_binding(ctx, name, loop_result_type(), name_range);
            }
        }
        Stmt::Break { range } => {
            if ctx.loop_depth == 0 {
                ctx.emit(
                    "WS078",
                    "`break` outside a loop: there is no enclosing `for` for it to stop.",
                    range.clone(),
                );
            }
        }
        Stmt::Continue { range } => {
            if ctx.loop_depth == 0 {
                ctx.emit(
                    "WS078",
                    "`continue` outside a loop: there is no enclosing `for` whose iteration it could end.",
                    range.clone(),
                );
            }
        }
    }
}

/// Type-check a `for` range source's endpoints and stride, and enforce WS077.
///
/// Each bound and count is an `int` place. The stride is not: a float stride
/// steps by its value. Only the stride's SIGN is checked: a zero, negative or
/// NaN one silently loops the wrong way rather than producing a wrong value
/// in an otherwise well-typed program.
fn check_for_range(ctx: &mut TypeCheckCtx<'_>, r: &RangeSource) {
    // Each bound and count is an `int` place.
    let int_place = |ctx: &mut TypeCheckCtx<'_>, e: &Expr| {
        infer::infer(ctx, e);
        infer::int_constants_in_slot(ctx, e, &Type::Int);
    };
    match &r.extent {
        RangeExtent::Bounds { start, end } => {
            int_place(ctx, start);
            int_place(ctx, end);
        }
        RangeExtent::Count { count, from } => {
            int_place(ctx, count);
            if let Some(from) = from {
                int_place(ctx, from);
            }
        }
    }
    let Some(step) = &r.step else { return };
    infer::infer(ctx, step);

    // Only a compile-time-constant step can be checked here; a runtime step
    // is the program's own responsibility (same "check what we can, at
    // compile time" stance as `Stmt::If`'s const-elision).
    let got = match ctx.const_eval(step, Admit::All) {
        Ok(Literal::Int(n)) if n <= 0 => Some(n.to_string()),
        Ok(Literal::Float(f)) if f.is_nan() || f <= 0.0 => Some(stride_as_written(step, f)),
        _ => None,
    };
    if let Some(got) = got {
        ctx.emit(
            "WS077",
            format!(
                "a loop stride must be greater than zero, got {got}. To count \
                 down, use `.rev()`."
            ),
            r.range.clone(),
        );
    }
}

/// A float stride's value for a diagnostic: a literal as written, anything
/// else as the value it evaluates to.
fn stride_as_written(step: &Expr, value: f64) -> String {
    match step {
        Expr::FloatLit { text, .. } => text.clone(),
        Expr::UnOp { op, operand, .. } if op == "-" => match operand.as_ref() {
            Expr::FloatLit { text, .. } => format!("-{text}"),
            _ => crate::const_eval::written_float(value),
        },
        _ => crate::const_eval::written_float(value),
    }
}

/// Whether `e` denotes an array or map variable a `for` loop can iterate by
/// reference, rather than a computed value. A plain `T[]`/`Map<K, V>`
/// parameter already reads the caller's storage (containers are references),
/// which `is_ref_able` does not see, since it recognizes only a `*T` parameter.
fn is_variable_ref(ctx: &TypeCheckCtx<'_>, e: &Expr) -> bool {
    match e {
        Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
            Some(s) => {
                matches!(
                    s.kind,
                    SymbolKind::Var | SymbolKind::Array | SymbolKind::Map
                ) || (matches!(s.kind, SymbolKind::LetBinding | SymbolKind::Param)
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)))
                    || (s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_)))
            }
            None => false,
        },
        Expr::FieldAccess { obj, field, .. } => {
            if let Expr::Ident {
                name: ns,
                range: ns_range,
            } = obj.as_ref()
                && ctx.scope.lookup(ns).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                return ctx
                    .ns_members(ns, &ns_range.file)
                    .and_then(|m| m.get(field))
                    .is_some_and(|info| {
                        matches!(
                            info.kind,
                            SymbolKind::Var | SymbolKind::Array | SymbolKind::Map
                        )
                    });
            }
            is_variable_ref(ctx, obj)
        }
        _ => false,
    }
}

/// The record `let run = for` binds after the loop: `{ aborted, iterations }`.
/// Built from `ir::plane::result_fields`, the same table lowering wires each
/// field's shell pin from.
fn loop_result_type() -> Type {
    Type::Record(
        crate::ir::plane::result_fields()
            .into_iter()
            .map(|(name, _port, ty)| (name.to_string(), ty))
            .collect(),
    )
}

/// Declare one name a `for` loop binds (a slot pattern name, or the
/// `let run = for` result) as a plain read-only local, the same `SymbolKind` a `let` capture
/// uses.
fn declare_for_binding(ctx: &mut TypeCheckCtx<'_>, name: &str, ty: Type, range: &SourceRange) {
    ctx.scope
        .declare_new(SymbolKind::LetBinding, name, ty, range.clone());
}

/// Bind a `{ }` pattern that names a counted loop's own SLOTS rather than
/// destructuring a value: `for { index } in 0..n`.
///
/// Field names resolve against `spec.slot_labels` and take `slot_tys` at the
/// same index, keeping this in step with the entry-gate ports lowering looks
/// up from `spec.entry_slots` at that index.
fn bind_range_slot_names(
    ctx: &mut TypeCheckCtx<'_>,
    fields: &[RecordDestructField],
    spec: &'static crate::ir::plane::PlaneSpec,
    slot_tys: &[Type],
) {
    for field in fields {
        match field {
            RecordDestructField::Named { name, alias, range } => {
                match spec.slot_labels.iter().position(|l| l == name) {
                    Some(idx) => declare_for_binding(
                        ctx,
                        alias.as_ref().unwrap_or(name),
                        slot_tys[idx].clone(),
                        range,
                    ),
                    None => ctx.emit(
                        "WS076",
                        format!(
                            "`{name}` is not a slot of this loop. Available: {}.",
                            spec.slot_labels.join(", ")
                        ),
                        range.clone(),
                    ),
                }
            }
            // A range has no record for `...` to collect leftovers from, and
            // binding it to "every slot not named" would invent a value with
            // no wire behind it.
            RecordDestructField::Rest { range, .. } => ctx.emit(
                "WS076",
                format!(
                    "`...` has nothing to collect: a `{{ }}` binding on a range names the \
                     loop's own slots ({}), not the fields of a value.",
                    spec.slot_labels.join(", ")
                ),
                range.clone(),
            ),
        }
    }
}

/// Bind one `for` slot pattern to `ty`. A `{ .. }` pattern destructures a
/// record-typed slot; an unknown field or a non-record slot is WS076.
///
/// Unlike `bind_let`'s `RecordDestruct` arm, an unresolvable field is an
/// error rather than quietly typed `Any`: WS076 exists to police a `for`
/// slot's shape.
fn bind_for_pat(ctx: &mut TypeCheckCtx<'_>, pat: &ForPat, ty: &Type) {
    match pat {
        ForPat::Ignore(_) => {}
        ForPat::Name(name, range) => declare_for_binding(ctx, name, ty.clone(), range),
        ForPat::Record { fields, range } => {
            // An already-erroneous slot (e.g. WS075 already fired for this
            // loop, so there is no real type to destructure): bind every
            // name `Any` rather than piling on a second diagnostic.
            if matches!(ty, Type::Any) {
                for field in fields {
                    match field {
                        RecordDestructField::Named { name, alias, range } => {
                            declare_for_binding(
                                ctx,
                                alias.as_ref().unwrap_or(name),
                                Type::Any,
                                range,
                            );
                        }
                        RecordDestructField::Rest { name, range } => {
                            declare_for_binding(ctx, name, Type::Any, range);
                        }
                    }
                }
                return;
            }
            let Type::Record(rec) = ty else {
                ctx.emit(
                    "WS076",
                    format!(
                        "a `{{ ... }}` binding destructures the value in its slot, and this \
                         slot is `{ty}`, not a record. Name the slot instead (`for v in ...`); \
                         a record-typed array or map cannot be iterated at all."
                    ),
                    range.clone(),
                );
                return;
            };
            // Field names a `Named` has consumed so far, so a trailing
            // `...rest` collects exactly what is left.
            let mut consumed: HashSet<&str> = HashSet::default();
            for field in fields {
                match field {
                    RecordDestructField::Named { name, alias, range } => {
                        let bind_name = alias.as_ref().unwrap_or(name);
                        match rec.iter().find(|(k, _)| k == name) {
                            Some((_, fty)) => {
                                declare_for_binding(ctx, bind_name, fty.clone(), range);
                            }
                            None => ctx.emit(
                                "WS076",
                                format!(
                                    "`{name}` is not a field of `{ty}`. Available: {}.",
                                    rec.iter()
                                        .map(|(n, _)| n.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                                range.clone(),
                            ),
                        }
                        consumed.insert(name.as_str());
                    }
                    RecordDestructField::Rest { name, range } => {
                        let rest_ty = Type::Record(
                            rec.iter()
                                .filter(|(k, _)| !consumed.contains(k.as_str()))
                                .cloned()
                                .collect(),
                        );
                        declare_for_binding(ctx, name, rest_ty, range);
                    }
                }
            }
        }
    }
}

pub(super) fn target_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// Whether a `match`, `if let` or `let else` scrutinee is writable storage, so
/// its payload captures can be written. A capture binds the scrutinee's
/// `__{Variant}_{field}` slot, a real `Pseudo_Var` gate, only when the
/// scrutinee is storage the compiler can name: a `var`, a `*T` parameter, or a
/// chain of record fields over one.
///
/// Not `is_ref_able`, which recurses through an index: an element scrutinee
/// (`arr[i]`, `m[k]`) lowers to a read with no write-back, so a write to its
/// capture would do nothing. A namespaced member (`S.e`) is left out as well,
/// since an imported enum var does not type as an enum in scrutinee position;
/// `false` errs toward a diagnostic rather than a dropped write.
pub(crate) fn scrutinee_is_mutable(ctx: &TypeCheckCtx<'_>, scrutinee: &Expr) -> bool {
    match scrutinee {
        Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
            Some(s) => {
                s.kind == SymbolKind::Var
                    || (s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)))
            }
            None => false,
        },
        // Each record field is its own storage gate, so an enum-typed field is
        // writable exactly when the record it hangs off is.
        Expr::FieldAccess { obj, .. } => scrutinee_is_mutable(ctx, obj),
        _ => false,
    }
}

/// Whether `e` is one of the writable target shapes `infer_assign_target`
/// accepts, for checking `&` and `ref` operands (WS008). Reports nothing; the
/// caller decides.
pub(crate) fn is_ref_able(ctx: &TypeCheckCtx<'_>, e: &Expr) -> bool {
    match e {
        Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
            Some(s) => {
                matches!(s.kind, SymbolKind::Var | SymbolKind::Array | SymbolKind::Map)
                // A `let` is only writable/ref-able when it's reference-backed
                // (an array/map binding); a scalar `let` is a computed wire, so
                // `&scalarLet` (and `scalarLet = ...`) would silently no-op.
                || (s.kind == SymbolKind::LetBinding
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)))
                || (s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)))
                || (s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_)))
            }
            None => false,
        },
        Expr::IndexAccess { obj, .. } => is_ref_able(ctx, obj),
        // A field of a record `let` (a namespace member included) that names
        // a var is that var: `&s.cpsr` on `let s = { cpsr }` is cpsr's ref.
        Expr::FieldAccess { .. } | Expr::TuplePick { .. } if let_place(ctx, e) => true,
        Expr::TuplePick { .. } => false,
        // A namespaced state member (`S.g`): ref-able when the member is a
        // var/array/map, exactly like its plain-imported form. Lowering wires
        // `&S.g` correctly, so without this arm the `&` check would wrongly
        // reject it as WS008.
        Expr::FieldAccess { obj, field, .. } => {
            if let Expr::Ident {
                name: ns,
                range: ns_range,
            } = obj.as_ref()
                && ctx.scope.lookup(ns).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                return ctx
                    .ns_members(ns, &ns_range.file)
                    .and_then(|m| m.get(field))
                    .map(|info| {
                        matches!(
                            info.kind,
                            SymbolKind::Var | SymbolKind::Array | SymbolKind::Map
                        )
                    })
                    .unwrap_or(false);
            }
            // An ordinary record field (`p.a`, `o.i.m`): each field is its own
            // storage gate, so it is writable and ref-able exactly when the
            // record it hangs off is. Recursing keeps the conservatism, since a
            // field of a computed scalar `let` fails the `Ident` arm's guard and
            // stays rejected. A field of a container element (`pts[i].x`) is an
            // element of a value column, which has no ref of its own.
            !matches!(infer::flatten_access(obj).0, Expr::IndexAccess { .. })
                && is_ref_able(ctx, obj)
        }
        _ => false,
    }
}

/// Whether `e` projects a place off a record `let`, directly or as a
/// namespace member.
fn let_place(ctx: &TypeCheckCtx<'_>, e: &Expr) -> bool {
    let (root, segs) = infer::flatten_access(e);
    let Expr::Ident { name, range } = root else {
        return false;
    };
    let is_let = match ctx.scope.lookup(name).map(|s| s.kind) {
        Some(SymbolKind::LetBinding) => true,
        Some(SymbolKind::Namespace) => segs.first().is_some_and(|m| {
            ctx.ns_members(name, &range.file)
                .and_then(|ms| ms.get(m.as_str()))
                .is_some_and(|info| info.kind == SymbolKind::LetBinding)
        }),
        _ => false,
    };
    is_let && infer::projection_is_place(ctx, e)
}

fn infer_assign_target(ctx: &mut TypeCheckCtx<'_>, e: &Expr) -> Type {
    if let Expr::Ident { name, range } = e {
        if name == "_" {
            return Type::Any;
        }
        let sym = ctx.scope.lookup(name).cloned();
        #[cfg(test)]
        if let Some(s) = &sym {
            ctx.ident_decls
                .push((range.clone(), s.decl_range.clone(), true));
        }
        match sym {
            None => {
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::Var => unwrap_ref(&s.ty).container_ref_as_value(),
            Some(s) if s.kind == SymbolKind::Array => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Map => unwrap_ref(&s.ty),
            // A `let` is a writable target only when it is reference-backed (an array or
            // map binding it aliases). A scalar `let` is a computed wire: a write to it
            // (`y = 5` on `let y = x + 1`) would emit no gate. A `const` container is the
            // one reference-backed `let` that is not writable: it is a compile-time value
            // and a runtime container, and `xs[0] = 99` would leave `const z = xs[0]`
            // folding to the original element.
            Some(s)
                if s.kind == SymbolKind::LetBinding
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _))
                    && ctx.is_const_container(name) =>
            {
                ctx.emit(
                    "WS007",
                    format!(
                        "'{name}' is a `const` array/map and can't be written: a `const` \
                         container is immutable so its compile-time value and its runtime \
                         contents can never disagree; declare it `var` to make it mutable"
                    ),
                    range.clone(),
                );
                Type::Any
            }
            Some(s)
                if s.kind == SymbolKind::LetBinding
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)) =>
            {
                unwrap_ref(&s.ty)
            }
            // A capture is not a `let` the user can redeclare: it names a slot
            // of the matched value, writable only when the scrutinee is storage.
            Some(s)
                if s.kind == SymbolKind::LetBinding
                    && ctx.readonly_captures.contains(&s.decl_range.start_key()) =>
            {
                ctx.emit(
                    "WS007",
                    format!(
                        "'{name}' is a read-only capture and can't be assigned. Writing a capture writes the matched value's payload in place, so it needs a scrutinee that is storage: a `var`, a `*T` parameter, or a field of one. A container element (`arr[i]`, `m[k]`) is read by value, so copy it into a `var`, match on that, then store it back"
                    ),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::LetBinding => {
                ctx.emit(
                    "WS007",
                    format!(
                        "'{name}' is a `let` binding and can't be assigned; declare it `var` to make it mutable"
                    ),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)) => {
                unwrap_ref(&s.ty)
            }
            Some(s) if s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_)) => {
                unwrap_ref(&s.ty)
            }
            _ => {
                ctx.emit(
                    "WS007",
                    format!("'{name}' isn't a writable target"),
                    range.clone(),
                );
                Type::Any
            }
        }
    } else if let Expr::IndexAccess { obj, index, .. } = e {
        let obj_ty = infer_assign_target(ctx, obj);
        // Recorded so lowering can resolve a runtime inner container
        // (`rows[y][x] = v`) from the OBJECT's type, not just the element's.
        {
            let r = obj.range();
            ctx.record_expr_type(r, &obj_ty);
        }
        ctx.record_container_kind(obj, &obj_ty);
        infer::check_index_type(ctx, &obj_ty, index);
        match obj_ty {
            Type::Array(inner) => *inner,
            // A map subscript write's value type is the map's value type, as for the array
            // arm above and the read-position `Expr::IndexAccess` arm in `infer.rs`, so
            // `m[k] = <wrong type>` is reported.
            Type::Map(_, v) => *v,
            _ => Type::Any,
        }
    } else if let Expr::Unsafe { inner, range } = e {
        // The slot's own type, so a wrong-typed write is caught here rather
        // than silently coercing into the payload gate.
        infer::infer_unsafe_access(ctx, inner, range)
    } else if let Expr::FieldAccess { obj, field, .. } = e {
        // A record field target (`p.x = 5`, `pts[i].inner.a = v`) types as the field,
        // so a wrong-typed write is reported (as `any`, a scalar written into a
        // record field would be dropped by lowering). Inferring the object records its
        // type, which hover reads for a field written as a target (`tk[i].phase = v`).
        // A non-record object or an undeclared field stays permissive: lowering
        // resolves those dynamically (a swizzle, `.Value`, a payload slot), and the
        // read path reports an unknown field. A namespace member (`u.xs[1] = 2`) types
        // as the member.
        if let Some(ty) = infer::namespace_member_type(ctx, obj, field) {
            // The namespace name keeps its hover type, as on a read.
            ctx.discarding_diagnostics(|ctx| infer::infer(ctx, obj));
            return unwrap_ref(&ty);
        }
        let obj_ty = unwrap_ref(&infer::infer(ctx, obj));
        // A record array's or map's column (`pts.x[0] = 5`, and `h.pts.x[0] = 5`
        // on a record array field) types as the column, as it does when read.
        // A column of a column (`qs.p.x`) is still a column.
        if let Some(column) = infer::record_column_type(&obj_ty, field) {
            ctx.record_container_kind(obj, &obj_ty);
            return column;
        }
        match &obj_ty {
            // A `*T` field aliases the caller's storage, so writing through
            // one writes the pointee: the target's type is what it points at.
            Type::Record(fields) | Type::Outputs(fields) => fields
                .iter()
                .find(|(n, _)| n == field)
                .map(|(_, t)| unwrap_ref(t))
                .unwrap_or(Type::Any),
            _ => Type::Any,
        }
    } else if let Expr::TuplePick { obj, index, .. } = e {
        // The tuple/record slot's own type, for the same reason.
        let obj_ty = unwrap_ref(&infer::infer(ctx, obj));
        match &obj_ty {
            Type::Tuple(fields) => fields.get(*index).map(unwrap_ref).unwrap_or(Type::Any),
            Type::Record(fields) | Type::Outputs(fields) => fields
                .get(*index)
                .map(|(_, t)| unwrap_ref(t))
                .unwrap_or(Type::Any),
            _ => Type::Any,
        }
    } else if let Expr::Deref { operand, range } = e {
        infer_deref_target(ctx, operand, range).0
    } else {
        // Any other target (a call result `f() = 5`, a literal, an operator
        // expression) is not an lvalue; as `any` it would be dropped by lowering, so
        // it is rejected.
        ctx.emit(
            "WS007",
            "this isn't a writable target; only a variable, an array/map \
             element, or a record field can be assigned"
                .to_string(),
            e.range().clone(),
        );
        Type::Any
    }
}

/// The type `*operand = v` writes, and whether it writes through a ref value
/// `operand` holds (`*h = v` with `h: *int`) rather than to a var `*x` names.
fn infer_deref_target(
    ctx: &mut TypeCheckCtx<'_>,
    operand: &Expr,
    range: &SourceRange,
) -> (Type, bool) {
    let t = infer::infer(ctx, operand);
    match t {
        Type::Ref(inner) => (*inner, true),
        Type::Any => (Type::Any, false),
        t @ (Type::Array(_) | Type::Map(_, _) | Type::Brick) => {
            infer::emit_deref_not_a_scalar_ref(ctx, &t, range);
            (Type::Any, false)
        }
        _ if is_ref_able(ctx, operand) => (infer_assign_target(ctx, operand), false),
        other => {
            ctx.emit(
                "WS003",
                format!("`*` needs a reference, got {}", type_str(&other)),
                range.clone(),
            );
            (Type::Any, false)
        }
    }
}

/// A write through a dynamic ref (`*h = v`) wires into whatever `Var_Set` gate
/// `h` points at, with no coercion adapter (the reason for `*T` argument
/// invariance in `sig::check_wire_arg`), so a numeric widen or narrow through a
/// ref is a mismatch, unlike `n = 1.5`. Called only for a real ref write
/// (`infer_deref_target`); `*x = v` on an auto-deref var takes the lenient path.
fn check_deref_write_is_exact(ctx: &mut TypeCheckCtx<'_>, value: &Expr, target_ty: &Type) {
    let value_ty = ctx
        .recorded_type(value.range())
        .cloned()
        .unwrap_or(Type::Any);
    if matches!(value_ty, Type::Any) || matches!(target_ty, Type::Any) {
        return;
    }
    // A float constant is not narrowed through a ref either, though it is
    // recorded as the integer an `int` slot would store.
    let value_ty = match (target_ty, super::infer::float_constant_value(ctx, value)) {
        (Type::Int, Some(_)) => Type::Float,
        _ => value_ty,
    };
    // An integer literal is written as the float it names, as it is into a
    // `float` var.
    if matches!(target_ty, Type::Float) && int_literal(value).is_some() {
        return;
    }
    if coerce(&value_ty, target_ty) == CoerceRule::Coerce {
        ctx.emit(
            "WS003",
            format!(
                "`*` write needs exactly {target_ty}, got {value_ty} - a write through a ref \
                 carries no coercion"
            ),
            value.range().clone(),
        );
    }
}

/// The value of an integer literal, optionally negated (`0`, `-2`).
pub(crate) fn int_literal(e: &Expr) -> Option<i64> {
    match e {
        Expr::IntLit { value, .. } => Some(*value),
        Expr::UnOp { op, operand, .. } if op == "-" => int_literal(operand).map(i64::wrapping_neg),
        _ => None,
    }
}

/// A rendering of a constant `if` condition for the dropped-block reason
/// (`` `N > 1` is true here ``): a bare name, a literal, a unary or binary
/// operator, or a `.field` read; anything else is a generic placeholder. It is
/// explanatory text only, never parsed back.
fn describe_cond(e: &Expr) -> String {
    match e {
        Expr::Ident { name, .. } => name.clone(),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::IntLit { text, .. } => text.clone(),
        Expr::FloatLit { text, .. } => text.clone(),
        Expr::StringLit { value, .. } => format!("\"{value}\""),
        Expr::UnOp { op, operand, .. } => format!("{op}{}", describe_cond(operand)),
        Expr::BinOp {
            op, left, right, ..
        } => {
            format!("{} {op} {}", describe_cond(left), describe_cond(right))
        }
        Expr::FieldAccess { obj, field, .. } => format!("{}.{field}", describe_cond(obj)),
        _ => "the condition".to_string(),
    }
}

/// True when `e` is a `CustomEvent("name")` or `GlobalCustomEvent("name")`
/// trigger call, the receiver side of a channel, which exposes `DataOut1..8`.
/// Types an `await CustomEvent(...)` data capture.
fn is_custom_event_call(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Call { callee, .. }
            if matches!(callee.as_ref(), Expr::Ident { name, .. } if name == "CustomEvent" || name == "GlobalCustomEvent")
    )
}

/// A value whose evaluation has no effect: a literal, a name, a field of one,
/// or a record or tuple built from them.
fn is_plain_value(e: &Expr) -> bool {
    match e {
        Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::StringLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::Ident { .. } => true,
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => is_plain_value(obj),
        Expr::RecordLit { fields, .. } => fields.iter().all(|f| match f {
            RecordLitField::Named { value, .. } | RecordLitField::Spread { value, .. } => {
                is_plain_value(value)
            }
            RecordLitField::Shorthand { .. } => true,
        }),
        _ => false,
    }
}
