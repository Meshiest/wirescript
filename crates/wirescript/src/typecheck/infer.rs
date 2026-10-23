//! Bidirectional expression typing.
//!
//! `infer(ctx, e)` synthesizes a type for `e` and records it in
//! `ctx.type_of_expr` (the sole recorder). `check(ctx, e, expected)` infers `e`
//! and drives coercion against `expected`, emitting `WS003` on a mismatch. Both
//! are exhaustive over `Expr` — the compiler enforces full coverage of every
//! variant, no fallback.

use crate::ast::{
    ArrayElem, Block, CallArg, Expr, ForSource, InterpPart, MatchBody, Pattern, RangeExtent,
    RecordLitField, Stmt, VariantPattern,
};
use crate::catalog::calls::find_call;
use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::Type;
use crate::types::coerce::{coerce, widening_join, CoerceRule};
use crate::types::mono::unwrap_ref;

use super::{
    call_param_config_enum, call_param_wire_enum, check_args, check_stmt,
    coerces_at_a_baked_literal, is_bare_enum_member,
    is_reference_type, op_operand_type, out_scope_key, output_record_type, resolve_op,
    resolve_type_expr, sig_of_callspec, target_name, type_param_mask, type_user_symbol_call,
    CallSignature, ContainerKind, ExecMode, MethodTarget, Param, ParamKind, SymbolInfo,
    SymbolKind, TypeCheckCtx,
};

/// Bound on how deep `$./….ws` source-prefab references are followed while
/// type-checking (the editor pass). The compile pipeline's own
/// `MAX_PREFAB_WS_DEPTH` is authoritative for emission; this only stops the
/// in-editor check from recursing forever on a self- or mutually-referential
/// `.ws` prefab.
const MAX_WS_PREFAB_CHECK_DEPTH: usize = 4;

thread_local! {
    static WS_PREFAB_CHECK_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Type-check a `$./file.ws` source-prefab reference: read the referenced file,
/// parse + infer it as its own program, and surface each of its diagnostics on
/// the reference `range` — so `control.ws`'s own errors underline
/// `$./control.ws` in the editor. The compile pipeline separately reads +
/// compiles the same file (see `disk_prefab_resolver`); this is the in-editor
/// mirror, analogous to the inline `$```…```` block's diagnostic surfacing.
fn check_ws_prefab_ref(ctx: &mut TypeCheckCtx, path: &str, range: &SourceRange) {
    let depth = WS_PREFAB_CHECK_DEPTH.with(|d| d.get());
    if depth >= MAX_WS_PREFAB_CHECK_DEPTH {
        return;
    }
    // Resolve `./rel` / `/abs` / `rel` against the referencing file's directory,
    // the same way `disk_prefab_resolver` does at compile time.
    let full = crate::compile::resolve_prefab_path(&range.file, path);
    let src = match std::fs::read_to_string(&full) {
        Ok(s) => s,
        Err(e) => {
            ctx.emit(
                "WS019",
                format!("cannot read prefab source `${path}`: {e}"),
                range.clone(),
            );
            return;
        }
    };
    let inner_file: std::sync::Arc<str> = std::sync::Arc::from(full.to_string_lossy().as_ref());
    let inner = crate::parser::parse(&src, &inner_file);
    // A prefab that imports needs the compile driver's loader to resolve them;
    // this editor pass has none, so surface only its PARSE diagnostics (its
    // type-check would false-positive on unresolved imports). A self-contained
    // prefab (the common case) is fully checked.
    let has_imports = inner
        .ast
        .decls
        .iter()
        .any(|d| matches!(d, crate::ast::TopDecl::Import(_)));
    let inner_diags: Vec<Diagnostic> = if has_imports {
        inner.diagnostics
    } else {
        WS_PREFAB_CHECK_DEPTH.with(|d| d.set(depth + 1));
        let tc = crate::typecheck::typecheck_with_inference(&inner.ast, &inner_file).0;
        WS_PREFAB_CHECK_DEPTH.with(|d| d.set(depth));
        inner.diagnostics.into_iter().chain(tc.diagnostics).collect()
    };
    // The inner diagnostics live in a different file, so there's no in-file
    // position to shift to (unlike an inline block) — re-attribute each to the
    // reference span, keeping its code and prefixing the prefab path.
    for mut d in inner_diags {
        d.message = format!("in prefab `{path}`: {}", d.message);
        d.range = range.clone();
        ctx.diagnostics.push(d);
    }
}

pub(crate) fn infer(ctx: &mut TypeCheckCtx, e: &Expr) -> Type {
    let reported = ctx.diagnostics.len();
    let mut t = infer_node(ctx, e);
    if !ctx.narrowings.is_empty()
        && matches!(t, Type::Opaque | Type::Any)
        && let Some(narrowed) = super::narrow::narrowed(ctx, e)
    {
        t = narrowed.ty.clone();
        let r = e.range();
        ctx.narrowed_reads.insert((r.file.clone(), r.start.offset, r.end.offset), narrowed);
    }
    ctx.record_expr_type(e.range(), &t);
    if holds_any(&t) && (ctx.reads_untyped_var(e) || has_error_since(ctx, reported) || operand_error_typed(ctx, e)) {
        let r = e.range();
        ctx.error_typed.insert((r.file.clone(), r.start.offset, r.end.offset));
    }
    t
}

/// How a method's receiver is spelled: a name or a literal, or a field,
/// element or tuple element read of one (`v[0]`, `b.v`). `None` for any
/// other expression.
fn receiver_spelling(e: &Expr) -> Option<String> {
    Some(match e {
        Expr::Ident { name, .. } => name.clone(),
        Expr::IntLit { text, .. } | Expr::FloatLit { text, .. } => text.clone(),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::StringLit { value, .. } if !value.contains(['"', '\\', '\n']) => format!("\"{value}\""),
        Expr::FieldAccess { obj, field, .. } => format!("{}.{field}", receiver_spelling(obj)?),
        Expr::TuplePick { obj, index, .. } => format!("{}.{index}", receiver_spelling(obj)?),
        Expr::IndexAccess { obj, index, .. } => format!("{}[{}]", receiver_spelling(obj)?, receiver_spelling(index)?),
        _ => return None,
    })
}

/// Whether `t` is `any` or holds it (an `any[]`).
fn holds_any(t: &Type) -> bool {
    match t {
        Type::Any => true,
        Type::Ref(t) | Type::Array(t) => holds_any(t),
        Type::Map(k, v) => holds_any(k) || holds_any(v),
        _ => false,
    }
}

/// Whether an operand of `e` (an expression directly inside it) is error
/// typed.
fn operand_error_typed(ctx: &TypeCheckCtx, e: &Expr) -> bool {
    use crate::ast::walk::{Ctx, Node, walk};
    if ctx.error_typed.is_empty() {
        return false;
    }
    let mut found = false;
    walk(Node::Expr(e), Ctx::default(), &mut |n, _| {
        if let Node::Expr(x) = n {
            found = found || ctx.is_error_typed(x);
        }
        false
    });
    found
}

/// Lowering distributes an index over an if-expression object
/// (`(if c then a else b)[i]` becomes `if c then a[i] else b[i]`), so each
/// branch is an index object of its own and needs its own container kind.
fn record_if_branch_containers(ctx: &mut TypeCheckCtx, obj: &Expr) {
    let Expr::IfExpr { then_branch, else_branch, .. } = obj else {
        return;
    };
    for br in [then_branch, else_branch] {
        let r = br.range();
        if let Some(t) = ctx.type_of_expr.get(&(r.file.clone(), r.start.offset, r.end.offset)).cloned() {
            ctx.record_container_kind(br, &unwrap_ref(&t));
        }
        record_if_branch_containers(ctx, br);
    }
}

/// The value type of `ns.field` when `obj` names an `import * as ns` namespace
/// and `field` is one of its readable members. Emits nothing; `None` covers
/// every other shape, including a missing or non-value member and a namespace
/// not nameable from this file.
pub(crate) fn namespace_member_type(ctx: &TypeCheckCtx, obj: &Expr, field: &str) -> Option<Type> {
    let Expr::Ident { name: ns_name, range: ns_range } = obj else {
        return None;
    };
    if ctx.scope.lookup(ns_name).map(|s| s.kind) != Some(SymbolKind::Namespace)
        || ctx.namespace_hidden_here(ns_name, &ns_range.file)
    {
        return None;
    }
    let info = ctx.ns_members(ns_name, &ns_range.file)?.get(field)?;
    let ty = info.value_type.clone()?;
    // A destructured place reads as its var, as it does in its own module.
    if info.let_decl.as_ref().is_some_and(|k| ctx.let_places.contains(k)) {
        return Some(unwrap_ref(&ty));
    }
    // An array or map ref reads as the container, as a `var cur: *int[]` of
    // the reading file does.
    Some(ty.container_ref_as_value())
}

/// The column `field` names on a record array or record map: `pts.x` on
/// `pts: P[]` is an array of the field's type, and `m.x` on `Map<K, P>` maps
/// the same keys to it. `None` for any other object type or an undeclared field.
pub(crate) fn record_column_type(obj_ty: &Type, field: &str) -> Option<Type> {
    match unwrap_ref(obj_ty) {
        Type::Array(elem) => match *elem {
            Type::Record(fields) => fields
                .into_iter()
                .find(|(k, _)| k == field)
                .map(|(_, t)| Type::Array(Box::new(t))),
            _ => None,
        },
        Type::Map(k, val) => match *val {
            Type::Record(fields) => fields
                .into_iter()
                .find(|(n, _)| n == field)
                .map(|(_, t)| Type::Map(k, Box::new(t))),
            _ => None,
        },
        _ => None,
    }
}

/// The container a method receiver denotes: a bare `ids`, or a record field
/// chain reaching one (`g.ready`, `a.b.counts`).
///
/// Resolved by walking scope and record types rather than inferring `obj`, so a
/// receiver that turns out to be neither an array nor a map falls through to the
/// remaining call arms without having emitted anything.
///
/// The reach here must match lowering's `resolve_field_chain` (see the
/// record-resolved container methods in `lower/call/dispatch.rs`) — narrowing
/// it to a bare identifier mistypes a hit: `g.ready.sum()` would still lower
/// to a real `ArrayVar_Sum` gate but type as `any`, leaving arithmetic on the
/// result with no overload.
fn container_receiver_type(ctx: &TypeCheckCtx, e: &Expr) -> Option<Type> {
    match e {
        Expr::Ident { name, .. } => {
            let sym = ctx.scope.lookup(name)?;
            let t = unwrap_ref(&sym.ty);
            // An `array` decl whose symbol carries no element type still reads
            // as an array here; the method's element type falls back to `any`.
            if sym.kind == SymbolKind::Array && !matches!(t, Type::Array(_)) {
                return Some(Type::Array(Box::new(Type::Any)));
            }
            Some(t)
        }
        Expr::FieldAccess { obj, field, .. } => {
            // A namespaced container member (`S.scores`, `S.arr`): read the
            // member's registered container type directly. The namespace symbol
            // itself is typeless (`any`), so recursing into it would drop the
            // element/value type and mistype the method result as `any`.
            if let Expr::Ident { name: ns, range: ns_range } = obj.as_ref()
                && ctx.scope.lookup(ns).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                return ctx
                    .ns_members(ns, &ns_range.file)
                    .and_then(|m| m.get(field))
                    .and_then(|info| info.value_type.clone());
            }
            let recv = container_receiver_type(ctx, obj)?;
            // A field of a scalar record is that field's own type; a field of a
            // record ARRAY / MAP is that field's PARALLEL container (struct-of-
            // arrays access): `pts.x` is `int[]`, `m.x` is `Map<K, int>`. Modelling
            // this lets a field-array method (`pts.x.sum()`, `pts.x.min()`) resolve
            // as an array/map method instead of falling through to the no-receiver
            // builtin check (which wrongly rejected `min`/`max`).
            let field_ty = |fields: &[(String, Type)]| {
                fields
                    .iter()
                    .find(|(k, _)| k == field)
                    .map(|(_, t)| unwrap_ref(t))
            };
            match recv {
                Type::Record(fields) | Type::Outputs(fields) => field_ty(&fields),
                Type::Array(inner) => match inner.as_ref() {
                    Type::Record(fields) => {
                        field_ty(fields).map(|t| Type::Array(Box::new(t)))
                    }
                    _ => None,
                },
                Type::Map(k, v) => match v.as_ref() {
                    Type::Record(fields) => {
                        field_ty(fields).map(|t| Type::Map(k.clone(), Box::new(t)))
                    }
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

/// [`container_receiver_type`], extended to a receiver whose value is a
/// container ref (an element `rows[y]`, a call result), and recorded in
/// `type_of_expr`: lowering takes a runtime container's ref from the
/// receiver's value, so it needs the receiver's type (`rows[y].push(v)`, or
/// `cur.length()` on a `var cur: *int[]`).
///
/// A receiver that is not a container drops the inference's diagnostics, so
/// it still falls through to the remaining call arms without having reported
/// anything; the types and read contexts the inference recorded stay. A
/// container receiver's recorded type is the container itself (`int[]` for a
/// `var cur: *int[]`), which replaces the ref type the inference recorded at
/// the receiver. Called once per call node: a generic mod body is checked once
/// per type combination, and each check must infer the receiver afresh.
fn method_receiver_type(ctx: &mut TypeCheckCtx, obj: &Expr) -> Option<Type> {
    let is_container = |t: &Type| matches!(t, Type::Array(_) | Type::Map(_, _));
    // A name, or a field path from one, that names no container is not one. A
    // field path from anything else (`m.get(k).Value`, `rows[0].x`) is a
    // runtime value, typed by inferring it.
    let mut root = obj;
    while let Expr::FieldAccess { obj: base, .. } = root {
        root = base;
    }
    let ty = match container_receiver_type(ctx, obj) {
        Some(t) => t.container_ref_as_value(),
        None if matches!(root, Expr::Ident { .. }) => return None,
        None => {
            let mark = ctx.diagnostics.len();
            let t = infer(ctx, obj).container_ref_as_value();
            if !is_container(&t) {
                ctx.diagnostics.truncate(mark);
            }
            t
        }
    };
    if is_container(&ty) {
        let r = obj.range();
        ctx.record_expr_type(r, &ty);
    }
    // A column receiver (`pts.x.sort()`, `qs.p.x.sort()`): record each record
    // container it is a column of, up to the root, which is how lowering tells
    // a column from a record variable's field and finds every sibling column.
    let mut cur = obj;
    while let Expr::FieldAccess { obj: base, field, .. } = cur
        && let Some(base_ty) = container_receiver_type(ctx, base)
        && record_column_type(&base_ty, field).is_some()
    {
        ctx.record_container_kind(base, &base_ty);
        cur = base;
    }
    Some(ty)
}

/// Whether `e` — an `Expr::IndexAccess` node — is itself a compile-time
/// constant, i.e. both the receiver and the index fold to a literal. Narrows
/// WS007 (see the `Expr::IndexAccess` arm below): a *runtime* array/map read
/// needs a gate on an exec chain, but a fully constant index into a fully
/// constant array/map emits no gate at all, so the exec-context rule does not
/// apply to it.
///
/// Evaluated against [`TypeCheckCtx::const_ctx_without_placeholders`], NOT
/// `const_ctx` — this decides whether to SUPPRESS a diagnostic, and a `const`
/// PARAMETER's placeholder is a type-shaped zero seeded once before any call
/// site exists (see `crate::const_scope::Marks::placeholder`). Evaluating
/// against it would let a fictional index value silently suppress WS007 for a
/// read whose real (call-site) index is unknown at this point in the pass —
/// exactly the placeholder-deciding-something hazard `const_ctx_without_placeholders`
/// exists to close. With placeholders stripped, such a read simply fails to
/// evaluate (`NotConstant`) and WS007 still fires, over-checking exactly like
/// every other placeholder-derived construct in this pass.
///
/// The `lookup_mod` closure is built fresh here, the same way every other
/// const-evaluating site in `typecheck/` does (see `TypeCheckCtx::const_ctx`'s
/// doc comment for why it can't be stored on `ctx` itself).
///
/// Deliberately keys on SUCCESS only, not on "the failure was a genuine
/// evaluation error rather than `NotConstant`". Widening it to also suppress
/// WS007 for the genuine-error reasons (`ArrayIndexOutOfRange`, `MapKeyNotFound`,
/// `Refused`, …) would de-duplicate the WS007+WS046 pair that `const z = t[5]`
/// reports — but those two diagnostics BOTH exist only for a `const` binding.
/// A plain `let q = t[5]` reports WS007 and nothing else (the non-`const` arms
/// of the `Stmt::Let`/`TopDecl::Let` const-eval are `Err(_) => {}` by design, so
/// a failed fold just falls back to runtime lowering), so suppressing it there
/// would leave that program with no error at all — and lowering cannot fold an
/// out-of-range index either, so it would ship a dead wire reading 0. That is
/// exactly the silent-miscompile class this feature exists to close, traded for
/// a cosmetic de-duplication, so the pair is left alone. The pair also predates
/// this narrowing: WS007 fired unconditionally in pure position before it, and
/// WS046 comes from the `const` binding path, independent of this function.
fn index_access_is_const(ctx: &TypeCheckCtx, e: &Expr) -> bool {
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    crate::const_eval::eval_expr(e, &ctx.const_ctx_without_placeholders(Some(&lookup)), &mut budget).is_ok()
}

/// A container method call is exempt from the pure-context WS007 when it can
/// legitimately run there: a READ on a `const`-declared receiver (that path
/// should const-fold, a separate missing feature, so an "outside an exec
/// context" error would mislead), or ANY call carrying an explicit
/// `exec = <trigger>` arg that supplies the exec context itself (e.g.
/// `lut.get(i, exec = tick)` in a pure `out` binding). A mutation or runtime
/// receiver with no `exec =` arg is not exempt.
fn container_call_exec_exempt(
    ctx: &TypeCheckCtx,
    mutates: bool,
    obj: &Expr,
    args: &[CallArg],
) -> bool {
    let declared_at_top = |name: &str| {
        ctx.consts.declared_at_top(name) && ctx.constant(name, crate::const_scope::Admit::Declared).is_some()
    };
    let const_read = !mutates && matches!(obj, Expr::Ident { name, .. } if declared_at_top(name));
    let has_exec_arg = args
        .iter()
        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
    const_read || has_exec_arg
}

/// Whether `args` carries an explicit `exec = <trigger>` named argument, which
/// supplies the exec context an otherwise-pure call site lacks. Mirrors the
/// `has_exec_arg` test the builtin exec-call WS007 (below) and
/// `container_call_exec_exempt` (above) both apply.
fn call_has_exec_arg(args: &[CallArg]) -> bool {
    args.iter()
        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"))
}

/// A container's element/value/key type that is a ref (`(*int)[]`,
/// `Map<K, *V>`) has no auto-aliasing convention the way a declared `*T` mod
/// parameter does (Task 3's `bump(a)`): its only valid arguments are things
/// that ALREADY carry a matching ref (a var holding one, `&x`, another
/// element), which `sig::check_wire_arg`'s ref-value block already handles
/// with its own single WS003. A plain, non-ref argument (a literal, or a
/// plain-typed var) is instead the ordinary "no conversions" mismatch every
/// ref value gets elsewhere - reported here, once, as WS003, rather than the
/// lvalue-shaped WS008 `check_wire_arg` gives a non-lvalue argument to a
/// genuine mod `*T` parameter (there IS no lvalue-aliasing convention for a
/// container element to fall back on).
///
/// Neutralizes that one param's type to `Type::Any` in `params` so the
/// `check_args` call the caller makes right after (still needed for arity and
/// any other params, and for an argument that already IS ref-shaped, which
/// this leaves untouched to flow through `check_wire_arg`'s own check) does
/// not also flag the same argument.
fn precheck_ref_element_args(ctx: &mut TypeCheckCtx, elem: &Type, params: &mut [Param], args: &[CallArg]) {
    if !matches!(elem, Type::Ref(_)) {
        return;
    }
    for (i, param) in params.iter_mut().enumerate() {
        if param.ty != *elem {
            continue;
        }
        let Some(CallArg::Positional(arg_expr)) = args.get(i) else {
            continue;
        };
        if matches!(arg_expr, Expr::NullLit { .. }) {
            continue;
        }
        let r = arg_expr.range();
        let raw = ctx
            .type_of_expr
            .get(&(r.file.clone(), r.start.offset, r.end.offset))
            .cloned()
            .unwrap_or_else(|| infer(ctx, arg_expr));
        if !matches!(raw, Type::Ref(_)) {
            ctx.emit(
                "WS003",
                format!("argument '{}': expected {elem}, got {raw}", param.name),
                r.clone(),
            );
            param.ty = Type::Any;
        }
    }
}

/// The container-typed names visible INSIDE a mod body during the exec-requiring
/// scan: its parameters and its declared container body-locals.
/// `container_receiver_type` resolves receivers through the CALLER's scope, where
/// a mod's own parameters and locals do not exist, so without this a container
/// reached through a parameter (`mod f(m: Map<..>) { m.get(k) }`) or a body-local
/// went undetected and a pure call to `f` silently miscompiled. Keyed name -> the
/// name's resolved type: EVERY parameter is recorded, container or not, so a
/// non-container parameter correctly SHADOWS a same-named global container rather
/// than the scan falsely flagging the global.
type ScanLocals = crate::collections::HashMap<String, Type>;

/// `arr.sortMultiple(a, b, ..., descending?)`: the gate sorts at most seven
/// parallel arrays with the key (`ArrayVarRef2`..`ArrayVarRef8`), and only a
/// trailing `bool` is `descending`; every other positional must be an array.
fn check_sort_multiple_args(ctx: &mut TypeCheckCtx, args: &[CallArg], range: &SourceRange) {
    const MAX_PARALLEL: usize = 7;
    let positional: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            _ => None,
        })
        .collect();
    let mut arrays = 0;
    for (i, e) in positional.iter().enumerate() {
        let key = {
            let r = e.range();
            (r.file.clone(), r.start.offset, r.end.offset)
        };
        let t = match ctx.type_of_expr.get(&key).cloned() {
            Some(t) => t,
            None => infer(ctx, e),
        };
        let last = i + 1 == positional.len();
        match unwrap_ref(&t) {
            Type::Array(_) => arrays += 1,
            Type::Bool if last => {}
            Type::Any => {}
            other => ctx.emit(
                "WS003",
                format!(
                    "'sortMultiple' argument {}: expected a parallel array{}, got {}",
                    i + 1,
                    if last { " or a trailing `descending` bool" } else { "" },
                    crate::analysis::types::type_str(&other)
                ),
                e.range().clone(),
            ),
        }
    }
    if arrays > MAX_PARALLEL {
        ctx.emit(
            "WS011",
            format!(
                "'sortMultiple' sorts at most {MAX_PARALLEL} parallel arrays along with this one \
                 (the gate has {MAX_PARALLEL} array ports), got {arrays}"
            ),
            range.clone(),
        );
    }
}

fn is_scan_container(t: &Type) -> bool {
    matches!(unwrap_ref(t), Type::Array(_) | Type::Map(_, _))
}

/// Build the [`ScanLocals`] for one mod body: all parameters, plus every declared
/// container body-local (`var m: Map<..>`, `array a: T[]`, `map m: ..`) reached
/// through the body's own control-flow blocks. Type resolution shares the
/// canonical resolver (aliases + generics), so a `type IntMap = Map<..>` parameter
/// resolves too.
///
/// Residual (documented, narrow): a container reached only through a field chain
/// rooted at a parameter record (`paramRec.arr.sum()`), or through an untracked
/// `let` alias of another container, is still not detected - the same class of gap
/// the whole scan documents, traded against re-inferring `let` RHS types here.
fn build_scan_locals(ctx: &TypeCheckCtx, decl: &crate::ast::ChipDecl) -> ScanLocals {
    // The in-scope alias snapshot is an O(scope-size) frame scan, which the
    // resolver builds only when a non-primitive annotation is resolved. A
    // param-less mod (or one with only primitive params, the common case)
    // resolves nothing and never pays for it, which is what keeps the whole scan
    // from going O(N^2) again on a deep chain of primitive-typed mods.
    let resolve = super::resolve::quiet_resolver(ctx, &decl.range.file);
    let mut env: ScanLocals = crate::collections::HashMap::default();
    for p in &decl.inputs {
        env.insert(p.name.clone(), resolve(&p.typ));
    }
    collect_container_locals(&decl.body, &resolve, &mut env);
    env
}

fn collect_container_locals(
    block: &Block,
    resolve: &impl Fn(&crate::ast::TypeExpr) -> Type,
    env: &mut ScanLocals,
) {
    for s in &block.stmts {
        match s {
            Stmt::Var(v) => {
                if let Some(te) = &v.typ {
                    let t = resolve(te);
                    if is_scan_container(&t) {
                        env.insert(v.name.clone(), t);
                    }
                }
            }
            Stmt::Array(a) => {
                env.insert(
                    a.name.clone(),
                    Type::Array(Box::new(resolve(&a.element_type))),
                );
            }
            Stmt::Map(m) => {
                env.insert(
                    m.name.clone(),
                    Type::Map(
                        Box::new(resolve(&m.key_type)),
                        Box::new(resolve(&m.value_type)),
                    ),
                );
            }
            // Descend the mod's own inline control flow. Nested handlers / chips
            // are separate exec roots (see `stmt_requires_exec`), so their locals
            // are not part of this mod's pure inline flow and are not collected.
            Stmt::If(i) => {
                collect_container_locals(&i.then_block, resolve, env);
                if let Some(b) = &i.else_block {
                    collect_container_locals(b, resolve, env);
                }
            }
            Stmt::IfLet(i) => {
                collect_container_locals(&i.then_block, resolve, env);
                if let Some(b) = &i.else_block {
                    collect_container_locals(b, resolve, env);
                }
            }
            Stmt::LetElse(l) => collect_container_locals(&l.else_block, resolve, env),
            Stmt::ExprStmt(es) => {
                if let Expr::MatchExpr { arms, .. } = &es.expr {
                    for arm in arms {
                        if let MatchBody::Block(b) = &arm.body {
                            collect_container_locals(b, resolve, env);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Receiver type for the exec-requiring scan: a mod parameter or declared local
/// (from [`ScanLocals`]) shadows any same-named caller-scope binding, and only
/// when the name is unknown here does it fall back to `container_receiver_type`
/// (globals / namespace members visible at the call site).
fn scan_receiver_type(ctx: &TypeCheckCtx, locals: &ScanLocals, e: &Expr) -> Option<Type> {
    if let Expr::Ident { name, .. } = e
        && let Some(t) = locals.get(name)
    {
        return Some(unwrap_ref(t));
    }
    container_receiver_type(ctx, e)
}

/// Whether the `mod`/`chip` named `name` is EXEC-REQUIRING: its inlined direct
/// flow performs an operation that would itself demand an exec context in pure
/// position (a container read/write, an exec builtin, or a transitive call to
/// another exec-requiring mod). A mod's body is type-checked in exec mode (see
/// `decl.rs`'s `in_exec` around the chip-body check), so the per-op WS007 is
/// DEFERRED there - correct, because a mod called only from exec must stay
/// legal. But inlining such a mod at a PURE call site drops those ops into pure
/// context, where a container `.get`/`[k]` produces the container REFERENCE
/// instead of a value and silently miscompiles. The call-site check in
/// `type_user_symbol_call` uses this to turn that miscompile into a loud WS007.
///
/// Resolved by scanning each reachable mod ONCE for its DIRECT exec ops plus its
/// user-mod call edges, then propagating to a least fixpoint over that call graph
/// (a mod is exec-requiring if it is directly so or any mod it transitively calls
/// is). Every mod in the reachable set is memoized in `ctx.exec_requiring_memo`,
/// so the first query resolves the whole subgraph and later queries are O(1) - a
/// chain of N pure-called mods costs one graph walk, not the O(N^2) of a per-site
/// re-walk. The fixpoint is cycle-safe: a cycle of non-direct mods stays false
/// (unlike memoizing a single recursive query, whose cycle-cut could cache a false
/// negative, which is why the earlier form memoized only the top-level answer).
///
/// INVARIANT: the memo (and the edge set) key on the bare mod NAME, so it assumes
/// a name resolves to one mod program-wide - which `resolve_mod` and the WS013
/// no-shadowing rule currently guarantee (top-level/chip duplicates are rejected;
/// imported bodies are not independently exec-checked). If a future change ever
/// legalizes mod-name shadowing or exec-checks imported bodies, this key must
/// become scope-qualified or a stale hit could miss a WS007 (a silent miscompile)
/// or fire a spurious one.
///
/// Precision notes (this must never over-fire on valid code):
/// - A container method is recognized only when the receiver actually resolves
///   to an `Array`/`Map` here (via `container_receiver_type`), so a user
///   self-mod that merely shares a name with a catalog method (`x.sort()` on a
///   record) is NOT mistaken for a container op.
/// - The exemptions match the direct checks exactly (`container_call_exec_exempt`
///   for a const-receiver read or an `exec =` arg; `index_access_is_const` for a
///   fully constant subscript).
/// - Independently exec-rooted or separately-compiled body regions - a nested
///   `on` handler, a nested `chip`/anon-chip - are NOT scanned: their ops keep
///   their own exec context regardless of how this mod is called.
/// - A receiver that resolves inside the mod's own scope (a mod parameter or a
///   declared body-local container) IS detected: `build_scan_locals` seeds the
///   scan with those names so `mod f(m: Map<..>) { m.get(k) }` called from a pure
///   site is flagged. The residual is a container reached only through a field
///   chain rooted at a parameter record, or an untracked `let` alias (see
///   `build_scan_locals`).
///
/// Scanning another mod's body resolves its names from here, which is no use of
/// the body being checked, so the storage log (WS091) does not see it.
pub(super) fn mod_is_exec_requiring(ctx: &mut TypeCheckCtx, name: &str) -> bool {
    ctx.scope.pause_storage_log();
    let requires = exec_requiring_scan(ctx, name);
    ctx.scope.resume_storage_log();
    requires
}

fn exec_requiring_scan(ctx: &mut TypeCheckCtx, name: &str) -> bool {
    if let Some(&cached) = ctx.exec_requiring_memo.get(name) {
        return cached;
    }
    // Gather the reachable user-mod subgraph from `name`, scanning each mod ONCE
    // for its direct exec flag and its call edges; then propagate to a least
    // fixpoint. Nodes already in the memo are final and act as constant inputs, so
    // their subtrees are not re-expanded.
    let mut direct: crate::collections::HashMap<String, bool> =
        crate::collections::HashMap::default();
    let mut edges: crate::collections::HashMap<String, Vec<String>> =
        crate::collections::HashMap::default();
    let mut stack = vec![name.to_string()];
    while let Some(m) = stack.pop() {
        if direct.contains_key(&m) || ctx.exec_requiring_memo.contains_key(&m) {
            continue;
        }
        let (d, es) = match ctx.resolve_mod(&m) {
            Some(decl) => scan_mod_direct(ctx, &decl),
            None => (false, Vec::new()),
        };
        for e in &es {
            if !direct.contains_key(e) && !ctx.exec_requiring_memo.contains_key(e) {
                stack.push(e.clone());
            }
        }
        direct.insert(m.clone(), d);
        edges.insert(m, es);
    }
    // Monotone least fixpoint: a mod is exec-requiring if it is directly so, or any
    // mod it calls (in this subgraph or already memoized) is. A cycle of non-direct
    // mods never flips to true, so there is no cycle-cut hazard to guard against.
    let mut requires = direct;
    loop {
        let mut changed = false;
        let names: Vec<String> = edges.keys().cloned().collect();
        for m in names {
            if requires[&m] {
                continue;
            }
            let hit = edges[&m].iter().any(|e| {
                requires.get(e).copied().unwrap_or(false)
                    || ctx.exec_requiring_memo.get(e).copied().unwrap_or(false)
            });
            if hit {
                requires.insert(m, true);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (m, r) in &requires {
        ctx.exec_requiring_memo.insert(m.clone(), *r);
    }
    requires.get(name).copied().unwrap_or(false)
}

/// Scan one mod body for its DIRECT exec-requiring flag (a container op / exec
/// builtin in its own inlined flow) and the user-mod names it calls (the graph
/// edges the fixpoint propagates over). Does not recurse into other mods.
fn scan_mod_direct(ctx: &TypeCheckCtx, decl: &crate::ast::ChipDecl) -> (bool, Vec<String>) {
    // Resolve receivers against THIS mod's own parameters and container locals
    // (invisible in the caller scope `container_receiver_type` uses), so a
    // container reached through a parameter/local is detected.
    let locals = build_scan_locals(ctx, decl);
    let mut edges = Vec::new();
    let direct = block_has_direct_exec(ctx, &decl.body, &locals, &mut edges);
    (direct, edges)
}

fn block_has_direct_exec(
    ctx: &TypeCheckCtx,
    block: &Block,
    locals: &ScanLocals,
    edges: &mut Vec<String>,
) -> bool {
    // Collect edges from EVERY statement (do not short-circuit on the first direct
    // hit), so the call graph is complete for the fixpoint.
    let mut direct = false;
    for s in &block.stmts {
        if stmt_has_direct_exec(ctx, s, locals, edges) {
            direct = true;
        }
    }
    direct
}

fn stmt_has_direct_exec(
    ctx: &TypeCheckCtx,
    stmt: &Stmt,
    locals: &ScanLocals,
    edges: &mut Vec<String>,
) -> bool {
    // Every arm evaluates ALL of its sub-expressions (non-short-circuiting `|`),
    // never `||`, so the fixpoint sees the mod's complete call-edge set.
    match stmt {
        // Independently exec-rooted / separately compiled: not part of this
        // mod's inlined pure flow, so their ops keep their own exec context.
        Stmt::Handler(_) | Stmt::AnonChip(_) | Stmt::ChipDecl(_) | Stmt::In(_) => false,
        Stmt::Assign(a) => {
            expr_has_direct_exec(ctx, &a.target, locals, edges)
                | expr_has_direct_exec(ctx, &a.value, locals, edges)
        }
        Stmt::Emit(e) => e
            .value
            .as_ref()
            .is_some_and(|v| expr_has_direct_exec(ctx, v, locals, edges)),
        Stmt::Await(a) => {
            let v = a
                .value_expr
                .as_ref()
                .is_some_and(|v| expr_has_direct_exec(ctx, v, locals, edges));
            v | expr_has_direct_exec(ctx, &a.exec_expr, locals, edges)
        }
        Stmt::If(i) => {
            let c = expr_has_direct_exec(ctx, &i.cond, locals, edges);
            let t = block_has_direct_exec(ctx, &i.then_block, locals, edges);
            let e = i
                .else_block
                .as_ref()
                .is_some_and(|b| block_has_direct_exec(ctx, b, locals, edges));
            c | t | e
        }
        Stmt::IfLet(i) => {
            let s = expr_has_direct_exec(ctx, &i.scrutinee, locals, edges);
            let t = block_has_direct_exec(ctx, &i.then_block, locals, edges);
            let e = i
                .else_block
                .as_ref()
                .is_some_and(|b| block_has_direct_exec(ctx, b, locals, edges));
            s | t | e
        }
        Stmt::Let(l) => expr_has_direct_exec(ctx, &l.value, locals, edges),
        Stmt::LetElse(l) => {
            expr_has_direct_exec(ctx, &l.scrutinee, locals, edges)
                | block_has_direct_exec(ctx, &l.else_block, locals, edges)
        }
        Stmt::OutBinding(ob) => ob
            .value
            .as_ref()
            .is_some_and(|v| expr_has_direct_exec(ctx, v, locals, edges)),
        Stmt::ExprStmt(es) => expr_has_direct_exec(ctx, &es.expr, locals, edges),
        Stmt::Var(v) => v
            .init
            .as_ref()
            .is_some_and(|e| expr_has_direct_exec(ctx, e, locals, edges)),
        Stmt::Buffer(b) => expr_has_direct_exec(ctx, &b.init, locals, edges),
        Stmt::Array(ad) => {
            let mut d = false;
            for el in &ad.init {
                d |= expr_has_direct_exec(ctx, el.expr(), locals, edges);
            }
            d
        }
        Stmt::Map(md) => md
            .init
            .as_ref()
            .is_some_and(|e| expr_has_direct_exec(ctx, e, locals, edges)),
        Stmt::Return { value, .. } => value
            .as_ref()
            .is_some_and(|e| expr_has_direct_exec(ctx, e, locals, edges)),
        // Task 1: parser-only. Structurally a branch/block form like `if`
        // above: scan the source's expressions and the body so the call-edge
        // set stays complete, without asserting anything about whether a
        // `for` itself is exec-requiring (a later task decides that).
        Stmt::For(f) => {
            let s = match &f.source {
                ForSource::Range(r) => {
                    let mut d = match &r.extent {
                        RangeExtent::Bounds { start, end } => {
                            expr_has_direct_exec(ctx, start, locals, edges)
                                | expr_has_direct_exec(ctx, end, locals, edges)
                        }
                        RangeExtent::Count { count, from } => {
                            let mut d = expr_has_direct_exec(ctx, count, locals, edges);
                            if let Some(from) = from {
                                d |= expr_has_direct_exec(ctx, from, locals, edges);
                            }
                            d
                        }
                    };
                    if let Some(step) = &r.step {
                        d |= expr_has_direct_exec(ctx, step, locals, edges);
                    }
                    d
                }
                ForSource::Collection { expr, .. } => expr_has_direct_exec(ctx, expr, locals, edges),
            };
            let b = block_has_direct_exec(ctx, &f.body, locals, edges);
            s | b
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => false,
    }
}

fn expr_has_direct_exec(
    ctx: &TypeCheckCtx,
    e: &Expr,
    locals: &ScanLocals,
    edges: &mut Vec<String>,
) -> bool {
    match e {
        Expr::Call { callee, args, .. } => {
            // A container method call (`m.get(k)`, `arr.push(v)`): every catalog
            // method lowers to an `Exec_*` gate, so it is exec-requiring unless
            // exempt (const-receiver read / `exec =` arg). Recognized only when
            // the receiver actually resolves to a container here, so a same-named
            // user self-mod is not misread. `direct` never short-circuits the rest
            // of the traversal - callee and args are always walked so their edges
            // (and any nested op) are recorded even when this call is itself direct.
            let mut direct = false;
            let mut handled = false;
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
                let mutates = match scan_receiver_type(ctx, locals, obj) {
                    Some(Type::Array(_)) => {
                        crate::catalog::arrays::array_method(field).map(|m| m.mutates)
                    }
                    Some(Type::Map(_, _)) => {
                        crate::catalog::maps::map_method(field).map(|m| m.mutates)
                    }
                    _ => None,
                };
                if let Some(mutates) = mutates {
                    handled = true;
                    if !container_call_exec_exempt(ctx, mutates, obj, args) {
                        direct = true;
                    }
                }
            }
            if !handled {
                match callee.as_ref() {
                    // A builtin whose CallSpec is exec-form, called without an
                    // `exec =` override (mirrors the builtin WS007 below).
                    Expr::Ident { name, range } => {
                        if let Some(c) = find_call(name).filter(|_| !ctx.calls_user_mod(name, &range.file)) {
                            if c.exec && !call_has_exec_arg(args) {
                                direct = true;
                            }
                        } else {
                            // A call to another user mod: record the call-graph
                            // edge; the fixpoint propagates its exec flag.
                            edges.push(name.clone());
                        }
                    }
                    // The receiver spelling of the same two cases: an exec-form
                    // receiver builtin (`e.GetLocation()`), else a user
                    // `self`-receiver mod (`v.helper(o)`) - an edge.
                    Expr::FieldAccess { field, .. } => {
                        if let Some(c) = find_call(field).filter(|c| c.receiver.is_some()) {
                            if c.exec && !call_has_exec_arg(args) {
                                direct = true;
                            }
                        } else {
                            edges.push(field.clone());
                        }
                    }
                    _ => {}
                }
            }
            direct |= expr_has_direct_exec(ctx, callee, locals, edges);
            for a in args {
                direct |= match a {
                    CallArg::Positional(x) | CallArg::Spread(x) => {
                        expr_has_direct_exec(ctx, x, locals, edges)
                    }
                    CallArg::Named { value, .. } => expr_has_direct_exec(ctx, value, locals, edges),
                };
            }
            direct
        }
        Expr::IndexAccess { obj, index, .. } => {
            let is_container = matches!(
                scan_receiver_type(ctx, locals, obj),
                Some(Type::Array(_)) | Some(Type::Map(_, _))
            );
            let mut direct = is_container && !index_access_is_const(ctx, e);
            direct |= expr_has_direct_exec(ctx, obj, locals, edges);
            direct |= expr_has_direct_exec(ctx, index, locals, edges);
            direct
        }
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => {
            expr_has_direct_exec(ctx, obj, locals, edges)
        }
        Expr::Unsafe { inner, .. } => expr_has_direct_exec(ctx, inner, locals, edges),
        Expr::Is { value, .. } | Expr::As { value, .. } => expr_has_direct_exec(ctx, value, locals, edges),
        Expr::UnOp { operand, .. } | Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
            expr_has_direct_exec(ctx, operand, locals, edges)
        }
        Expr::BinOp { left, right, .. } => {
            expr_has_direct_exec(ctx, left, locals, edges)
                | expr_has_direct_exec(ctx, right, locals, edges)
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_has_direct_exec(ctx, cond, locals, edges)
                | expr_has_direct_exec(ctx, then_branch, locals, edges)
                | expr_has_direct_exec(ctx, else_branch, locals, edges)
        }
        Expr::BlockExpr { stmts, value, .. } => {
            let mut d = false;
            for s in stmts {
                d |= stmt_has_direct_exec(ctx, s, locals, edges);
            }
            d | expr_has_direct_exec(ctx, value, locals, edges)
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            let mut d = expr_has_direct_exec(ctx, scrutinee, locals, edges);
            for arm in arms {
                d |= match &arm.body {
                    MatchBody::Expr(x) => expr_has_direct_exec(ctx, x, locals, edges),
                    MatchBody::Block(b) => block_has_direct_exec(ctx, b, locals, edges),
                };
            }
            d
        }
        Expr::InterpLit { parts, .. } => {
            let mut d = false;
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    d |= expr_has_direct_exec(ctx, x, locals, edges);
                }
            }
            d
        }
        Expr::RecordLit { fields, .. } | Expr::VariantCtor { fields, .. } => {
            let mut d = false;
            for f in fields {
                if let RecordLitField::Named { value, .. }
                | RecordLitField::Spread { value, .. } = f
                {
                    d |= expr_has_direct_exec(ctx, value, locals, edges);
                }
            }
            d
        }
        Expr::Array { elements, .. } => {
            let mut d = false;
            for el in elements {
                d |= expr_has_direct_exec(ctx, el.expr(), locals, edges);
            }
            d
        }
        Expr::MapLit { entries, .. } => {
            let mut d = false;
            for en in entries {
                d |= expr_has_direct_exec(ctx, &en.key, locals, edges);
                d |= expr_has_direct_exec(ctx, &en.value, locals, edges);
            }
            d
        }
        // `emit` names the chain point the caller has reached, so a mod whose
        // body reads it has nothing to name at a pure call site.
        Expr::CurrentExec { .. } => true,
        Expr::IntLit { .. }
        | Expr::AtomLit { .. }
        | Expr::FloatLit { .. }
        | Expr::StringLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::Ident { .. }
        | Expr::AssetRef { .. }
        | Expr::PrefabRef { .. }
        | Expr::NestedPrefab { .. } => false,
    }
}

/// Infer every field VALUE of a `VariantCtor`'s braced body with no target
/// type to check against - used on the recovery paths (unknown variant,
/// wrong bracket form, or `path` not actually a construction) so each field
/// expression still gets typed (and any error inside it still surfaces)
/// even though there's no payload shape to validate it against. Mirrors the
/// `Expr::RecordLit` infer arm's per-field walk, minus the shorthand lookup
/// (irrelevant with no expected type to coerce against).
fn infer_record_field_values(ctx: &mut TypeCheckCtx, fields: &[RecordLitField]) {
    for f in fields {
        match f {
            RecordLitField::Named { value, .. } | RecordLitField::Spread { value, .. } => {
                infer(ctx, value);
            }
            RecordLitField::Shorthand { .. } => {}
        }
    }
}

/// Outcome of resolving `Enum.Variant` at a CONSTRUCTION site (a unit
/// reference, a positional `Enum.Variant(..)` call, or a braced
/// `Enum.Variant { .. }`). Single-sources the shadow-guard + registry lookup +
/// generic guard + variant lookup + `WS060` emission that all three sites
/// otherwise duplicate; each caller dispatches on the payload shape itself.
enum VariantResolution {
    /// `enum_name` is not a known, non-generic, unshadowed enum type here, so
    /// this is not a construction site at all - the caller should fall through
    /// to its other handling (an ordinary field access / namespace call / the
    /// braced-form fallback).
    NotConstruction,
    /// `enum_name` IS such an enum, but has no variant named `variant`. `WS060`
    /// has already been emitted; the payload carries the enum's own type so the
    /// caller can recover as it (rather than `Any`).
    UnknownVariant(Type),
    /// Resolved: a recovery enum type (its generic `args` filled with `Any`), the
    /// matched variant definition, and the enum's declared type parameters
    /// (empty for a non-generic enum). Each construction site uses the last to
    /// decide whether it must infer concrete `args` (see `infer_enum_args`)
    /// before returning, or can hand back the recovery type as-is.
    Resolved(Type, crate::typecheck::enums::VariantDef, Vec<crate::ast::TypeParam>),
}

/// The enum name an expression path's head writes: `Dir` in `Dir.N`, and for
/// the head `c.Dir` of `c.Dir.N`, where `c` is an `import * as` namespace
/// whose module declares enum `Dir`, that enum's own name (see
/// `EnumRegistry::namespaced`).
pub(super) fn enum_head<'e>(ctx: &TypeCheckCtx, head: &'e Expr) -> Option<&'e str> {
    match head {
        Expr::Ident { name, .. } => Some(name),
        Expr::FieldAccess { obj, field, .. } => {
            let Expr::Ident { name: ns, range } = obj.as_ref() else { return None };
            let namespace = ctx.scope.binding_frame(ns).is_some_and(|(_, s)| s.kind == SymbolKind::Namespace);
            (namespace && ctx.enum_defs.namespaced(&range.file, ns, field).is_some()).then_some(field.as_str())
        }
        _ => None,
    }
}

/// Resolve `enum_name . variant` for a construction site. See
/// [`VariantResolution`]. `range` is where `WS060` points if the variant is
/// unknown.
///
/// The shadow guard mirrors every enum use site: a value symbol (`var`/`let`/
/// param, or a namespace) whose name equals the enum's shadows the type
/// (nearest-first `scope.lookup`), so such a name is NOT a construction site,
/// falling out as `NotConstruction`. A GENERIC enum resolves here too; its
/// concrete `args` are inferred per site (payload-driven, or from an
/// annotation), so the returned recovery type carries `Any` args as a
/// placeholder.
fn resolve_variant_for_construction(
    ctx: &mut TypeCheckCtx,
    enum_name: &str,
    variant: &str,
    range: &SourceRange,
) -> VariantResolution {
    if matches!(
        ctx.scope.lookup(enum_name).map(|s| s.kind),
        Some(k) if k != SymbolKind::Type
    ) {
        return VariantResolution::NotConstruction;
    }
    // Resolve registry membership + the (owned) matched variant and type params
    // in one borrow so the immutable `ctx.enum_defs` borrow ends before any
    // `ctx.emit`.
    let (is_enum, type_params, variant_def) = match ctx.enum_defs.expression_path(enum_name) {
        Some(def) => (
            true,
            def.type_params.clone(),
            def.variants.iter().find(|v| v.name == variant).cloned(),
        ),
        None => (false, Vec::new(), None),
    };
    if !is_enum {
        return VariantResolution::NotConstruction;
    }
    // Recovery / placeholder type: a non-generic enum has no args; a generic one
    // gets `Any` per parameter until a site infers the real arguments.
    let enum_ty = Type::Enum {
        name: enum_name.to_string(),
        args: vec![Type::Any; type_params.len()],
    };
    // A library names the enums it declares or imports, as it names aliases.
    if !super::resolve::enum_visible_in(ctx, enum_name, &range.file) {
        let message = hidden_enum_message(ctx, enum_name, range).unwrap_or_else(|| {
            format!(
                "unknown type '{enum_name}': this file neither declares it nor imports it (the enum of \
                 that name belongs to a file importing this one)"
            )
        });
        ctx.emit("WS002", message, range.clone());
        return VariantResolution::UnknownVariant(enum_ty);
    }
    match variant_def {
        Some(vdef) => VariantResolution::Resolved(enum_ty, vdef, type_params),
        None => {
            ctx.emit(
                "WS060",
                format!("enum `{enum_name}` has no variant `{variant}`"),
                range.clone(),
            );
            VariantResolution::UnknownVariant(enum_ty)
        }
    }
}

/// Bare-name variant resolution: `Some(42)`/`None`/`Ok(1)`/`Err(2)` instead of
/// `Option.Some(42)`/`Option.None`/`Result.Ok(1)`/`Result.Err(2)`. Delegates
/// the lookup + uniqueness rule to the single-sourced
/// `enums::resolve_bare_variant_enum`, supplying typecheck's OWN shadow
/// predicate.
///
/// Typecheck uses the WIDEST shadow set of the three stages: ANY scope symbol
/// of ANY kind - a `var`/`let`/param, a mod/chip, a type alias/namespace, OR a
/// user enum's own type name - wins over a prelude variant, so such a name is
/// NOT a construction here and the caller falls through to its ordinary
/// resolution. Lowering/const-eval each shadow a SUBSET of this (their scopes
/// can't see type-only symbols); every enum name they DO see via `enum_defs`
/// is also a scope symbol here, so they never resolve a bare name this stage
/// shadowed.
///
/// The uniqueness half (no match, or more than one enum sharing a variant
/// name, yields `None`) lives in the shared helper - see its doc comment.
fn resolve_bare_variant_enum(ctx: &TypeCheckCtx, name: &str) -> Option<String> {
    crate::typecheck::enums::resolve_bare_variant_enum(&ctx.enum_defs, name, |n| {
        ctx.scope.lookup(n).is_some()
    })
    .map(|def| def.name.clone())
}

/// The type of a BARE reference to `variant` (no call syntax) once it has
/// resolved to `enum_name` - shared by the qualified `Enum.Variant`
/// `FieldAccess` site and the bare `Some`/`None`-style `Ident` site, so the
/// two check identically. Only a UNIT variant constructs a value from a bare
/// reference (`Option.None`/bare `None`); its args come from the annotation
/// or WS063 (no payload to infer from). A bare non-unit variant reference
/// (`Option.Some` used for `.Discriminant`, or bare `Some` with no call) has
/// no payload here, so it types as the enum with the `Any`-args recovery
/// rather than forcing WS063.
fn variant_reference_type(
    ctx: &mut TypeCheckCtx,
    enum_name: &str,
    vdef: &crate::typecheck::enums::VariantDef,
    enum_ty: Type,
    type_params: &[crate::ast::TypeParam],
    expected: Option<&Type>,
    range: &SourceRange,
) -> Type {
    if type_params.is_empty() {
        return enum_ty;
    }
    if matches!(vdef.payload, crate::typecheck::enums::Payload::Unit) {
        let args = infer_enum_args(ctx, enum_name, type_params, &[], &[], expected, range);
        return Type::Enum {
            name: enum_name.to_string(),
            args,
        };
    }
    enum_ty
}

/// Positional enum-variant CALL construction (`Enum.Variant(args)` /
/// `Variant(args)`) once `enum_name`/`variant` have resolved - shared by the
/// qualified `Enum.Variant(args)` `Call` site and the bare `Some(42)`-style
/// `Call` site, so `Some(42)` checks (and later lowers) identically to
/// `Option.Some(42)`. `variant_range` is where `WS060`/`WS065` point (the
/// `Enum.Variant` field-access span for the qualified form, the bare name's
/// own span for the bare form); `call_range` is where `WS022`/`check_args`
/// point (the whole call, including its parens).
///
/// Returns `None` only when `resolve_variant_for_construction` reports
/// `NotConstruction` (a non-enum / shadowed `enum_name`) - the caller falls
/// through to its own ordinary call handling. Every other outcome (a
/// resolved construction, or an unknown-variant recovery) returns `Some`.
#[allow(clippy::too_many_arguments)]
fn try_construct_variant_positional(
    ctx: &mut TypeCheckCtx,
    enum_name: &str,
    variant: &str,
    args: &[CallArg],
    positional_arg_types: &[Type],
    expected: Option<&Type>,
    variant_range: &SourceRange,
    call_range: &SourceRange,
) -> Option<Type> {
    match resolve_variant_for_construction(ctx, enum_name, variant, variant_range) {
        VariantResolution::NotConstruction => None,
        VariantResolution::UnknownVariant(enum_ty) => Some(enum_ty),
        VariantResolution::Resolved(enum_ty, vdef, type_params) => {
            let crate::typecheck::enums::Payload::Positional(payload_types) = &vdef.payload
            else {
                ctx.emit(
                    "WS065",
                    ws065_positional_form_wrong(enum_name, variant, &vdef.payload),
                    variant_range.clone(),
                );
                return Some(enum_ty);
            };
            // Arity (WS022, matching the user mod/chip call convention - see
            // `type_user_symbol_call`/the namespace-call arm, both of which
            // report their own arity this way rather than `check_args`'s
            // WS011) and per-arg types (via the shared `check_args`, so an
            // arg the caller's preamble already inferred is read back from
            // `ctx.type_of_expr` instead of re-inferring it - avoids
            // double-reporting an error already inside one of these args,
            // exactly like `check_wire_arg`'s own cache read). A generic
            // enum's payload keeps its type parameters as `Type::Param`, so
            // `check_args`'s own `type_has_param` guard skips coercing those
            // args (their types drive the parameter solve below instead).
            let bound = expected_enum_args(expected, enum_name, &type_params);
            let params: Vec<Param> = payload_types
                .iter()
                .enumerate()
                .map(|(i, ty)| Param {
                    name: i.to_string(),
                    ty: instantiated(resolve_payload_param_type(ctx, ty, &type_params), &bound),
                    optional: false,
                    kind: ParamKind::Wire,
                })
                .collect();
            let positional_count =
                args.iter().filter(|a| matches!(a, CallArg::Positional(_))).count();
            let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
            if !has_spread && positional_count != params.len() {
                ctx.emit(
                    "WS022",
                    format!(
                        "`{variant}` expects {} argument{} but {} {} given",
                        params.len(),
                        if params.len() == 1 { "" } else { "s" },
                        positional_count,
                        if positional_count == 1 { "was" } else { "were" },
                    ),
                    call_range.clone(),
                );
            }
            let param_types: Vec<Type> = params.iter().map(|p| p.ty.clone()).collect();
            let sig = CallSignature {
                name: variant.to_string(),
                params,
                config_gate: None,
            };
            check_args(ctx, &sig, args, 0, false, true, call_range);
            args_fill_float_params(ctx, &sig.params, args);
            if type_params.is_empty() {
                return Some(enum_ty);
            }
            if let Some(args) = bound {
                let args = args.into_iter().map(|(_, t)| t).collect();
                return Some(Type::Enum { name: enum_name.to_string(), args });
            }
            // The caller's preamble inferred every positional arg into
            // `positional_arg_types`; solve the enum's type params against
            // the payload's declared param types.
            let solved_args = infer_enum_args(
                ctx,
                enum_name,
                &type_params,
                &param_types,
                positional_arg_types,
                expected,
                call_range,
            );
            Some(Type::Enum {
                name: enum_name.to_string(),
                args: solved_args,
            })
        }
    }
}

/// The type arguments of the generic enum `enum_name` when `expected` is
/// that enum with every argument known: a construction where it is expected
/// is built at that type, so each payload value is checked against its
/// instantiated type (an integer given for a `float` payload is that float).
fn expected_enum_args(
    expected: Option<&Type>,
    enum_name: &str,
    type_params: &[crate::ast::TypeParam],
) -> Option<Vec<(String, Type)>> {
    match expected {
        Some(Type::Enum { name, args })
            if name == enum_name
                && !type_params.is_empty()
                && args.len() == type_params.len()
                && args.iter().all(|a| !matches!(a, Type::Any) && !super::type_has_param(a)) =>
        {
            Some(type_params.iter().map(|p| p.name.clone()).zip(args.iter().cloned()).collect())
        }
        _ => None,
    }
}

/// `t`, a payload type resolved at a construction site, with the enum's type
/// parameters replaced by `bound` when the construction is built at a known
/// instantiation.
fn instantiated(t: Type, bound: &Option<Vec<(String, Type)>>) -> Type {
    match bound {
        Some(args) => crate::types::mono::substitute(&t, &args.iter().cloned().collect()),
        None => t,
    }
}

/// Resolve a variant payload field's declared type at a CONSTRUCTION site, with
/// the enum's own type parameters in scope: a bare parameter name (`T`) becomes
/// `Type::Param("T")` so the arg-vs-payload constraint solve can bind it, and
/// any other type resolves normally (a nested enum name, a primitive). This is
/// the construction-side analog of the match-site `resolve_payload_field_type`
/// (which substitutes CONCRETE args because the scrutinee is already typed);
/// here the args are what we are inferring, so a parameter stays a `Param`.
fn resolve_payload_param_type(
    ctx: &mut TypeCheckCtx,
    te: &crate::ast::TypeExpr,
    type_params: &[crate::ast::TypeParam],
) -> Type {
    if let crate::ast::TypeExpr::Name { name, .. } = te
        && type_params.iter().any(|p| &p.name == name)
    {
        return Type::Param(name.clone());
    }
    resolve_type_expr(ctx, te)
}

/// Check a variant payload field's value against its expected type, returning
/// the value's inferred type. When the expected type still carries a
/// `Type::Param` (a generic enum whose args aren't inferred yet), the value is
/// only inferred - coercing against a bare `Param` would spuriously WS003, the
/// same reason `check_args` skips a param-typed parameter.
fn check_or_infer_payload_field(ctx: &mut TypeCheckCtx, value: &Expr, expected: &Type) -> Type {
    if super::type_has_param(expected) {
        infer(ctx, value)
    } else {
        check(ctx, value, expected)
    }
}

/// Infer a generic enum's concrete type `args` at a construction site. Each
/// parameter is solved from the payload arguments (the same arg-driven
/// `call_constraints` + `solve` a generic `mod`/`chip` call uses); a parameter
/// the payload does not pin (`None`, or a variant that doesn't mention it) is
/// taken from `expected` when an annotation supplied `Enum<..>` of this enum,
/// and otherwise is a `WS063`. Returns one type per parameter (recovering with
/// `Any` for any that stays unresolved).
fn infer_enum_args(
    ctx: &mut TypeCheckCtx,
    enum_name: &str,
    type_params: &[crate::ast::TypeParam],
    param_types: &[Type],
    arg_types: &[Type],
    expected: Option<&Type>,
    range: &SourceRange,
) -> Vec<Type> {
    let constraints = crate::types::mono::call_constraints(param_types, arg_types);
    let expected_args = match expected {
        Some(Type::Enum { name, args })
            if name == enum_name && args.len() == type_params.len() =>
        {
            Some(args)
        }
        _ => None,
    };
    // Pass 1: solve every type param independently from `constraints` (a
    // param's own payload occurrences) or the annotation. A param with
    // NEITHER - no constraint anywhere in the constructed variant's payload
    // (e.g. `Result<T, E>`'s `E` when only `Ok(T)` is constructed - `Ok`'s
    // payload never mentions `E` at all) - stays `None` here rather than
    // immediately defaulting, so pass 2 below can draw on EVERY sibling this
    // loop solves, not just the ones that happen to precede it in
    // `type_params` order (`Result<T, E>`'s `T` is declared first, so without
    // this split an unannotated `Err(2)` - which solves `E` but not `T` -
    // would see no earlier sibling to default `T` from, while the identical
    // `Ok(1)` - which solves `T` before `E` - would).
    let mut solved_out: Vec<Option<Type>> = Vec::with_capacity(type_params.len());
    for (i, tp) in type_params.iter().enumerate() {
        let mask = [(tp.name.clone(), type_param_mask(ctx, tp))];
        // An UNBOUNDED type parameter accepts any type the payload pins it to,
        // including a user enum (`Option<Inner>`) - which sits outside the
        // wire-variant mask `solve` gates against, so its `OutOfMask` result
        // still carries the pinned type. A BOUNDED parameter keeps the mask
        // check (an out-of-bound arg stays unresolved -> hint/WS063).
        let solved = match crate::types::infer::solve(&constraints, &mask) {
            Ok(s) => s.get(&tp.name).cloned(),
            Err(crate::types::infer::InferError::OutOfMask { ty, .. }) if tp.bound.is_none() => {
                Some(ty)
            }
            Err(_) => None,
        };
        match solved {
            Some(t) if !super::type_has_param(&t) => solved_out.push(Some(t)),
            _ => match expected_args {
                Some(args) => solved_out.push(Some(args[i].clone())),
                None => solved_out.push(None),
            },
        }
    }
    // Pass 2: any param still unsolved defaults to the first param ANY
    // sibling solved, order-independent - an unconstrained param takes the
    // type its siblings already settled on rather than forcing an annotation
    // on every single-branch construction (`Ok(1)` and `Err(2)` alike read as
    // `Result<int, int>`). Only when NO sibling solved either (every param
    // genuinely unpinnable, e.g. bare `Option<T>`'s `None`) does a param stay
    // unresolved -> WS063.
    let sibling_default = solved_out.iter().flatten().next().cloned();
    let mut out = Vec::with_capacity(type_params.len());
    let mut unresolved: Vec<String> = Vec::new();
    for (tp, slot) in type_params.iter().zip(solved_out) {
        match slot.or_else(|| sibling_default.clone()) {
            Some(t) => out.push(t),
            None => {
                unresolved.push(tp.name.clone());
                out.push(Type::Any);
            }
        }
    }
    if !unresolved.is_empty() {
        ctx.emit(
            "WS063",
            format!(
                "cannot infer type parameter `{}` for `{enum_name}` - annotate the target \
                 (`: {enum_name}<...>`) or use a variant whose payload determines it",
                unresolved.join("`, `")
            ),
            range.clone(),
        );
    }
    out
}

/// The `WS065` message for a braced `Enum.Variant { .. }` whose variant does
/// NOT take named fields: the correct suggested spelling depends on the actual
/// payload - a positional variant wants `(..)`, a unit variant takes no payload
/// at all (bare `Enum.Variant`).
fn ws065_named_form_wrong(
    enum_name: &str,
    variant: &str,
    payload: &crate::typecheck::enums::Payload,
) -> String {
    use crate::typecheck::enums::Payload;
    match payload {
        Payload::Unit => format!(
            "variant `{variant}` takes no payload - write it as `{enum_name}.{variant}`"
        ),
        // Named is handled by the caller (it's the valid case); only
        // Positional reaches here besides Unit.
        _ => format!(
            "variant `{variant}` does not take named fields - call it as \
             `{enum_name}.{variant}(..)`"
        ),
    }
}

/// The `WS065` message for a positional `Enum.Variant(..)` whose variant does
/// NOT take positional arguments: a named variant wants `{ .. }`, a unit
/// variant takes no payload at all (bare `Enum.Variant`).
fn ws065_positional_form_wrong(
    enum_name: &str,
    variant: &str,
    payload: &crate::typecheck::enums::Payload,
) -> String {
    use crate::typecheck::enums::Payload;
    match payload {
        Payload::Unit => format!(
            "variant `{variant}` takes no payload - write it as `{enum_name}.{variant}`"
        ),
        // Named reaches here (Positional is the valid case).
        _ => format!(
            "variant `{variant}` does not take positional arguments - call it as \
             `{enum_name}.{variant} {{ .. }}`"
        ),
    }
}

/// The source range of a pattern node, for a diagnostic that must underline
/// the pattern itself (e.g. WS010 on an unknown named field's capture).
fn pattern_range(p: &Pattern) -> &SourceRange {
    p.range()
}

/// Render a `Pattern` back to compact source-like text for a diagnostic (the
/// WS054 witness message). Mirrors the surface syntax the parser reads.
/// `pub(crate)` so `analysis::hover`'s "fill missing match arms" code action
/// can render a [`crate::typecheck::patterns::Witness`] the same
/// way the compiler's own diagnostic does: one renderer, so the arm text a
/// quickfix inserts can never read differently from what WS054 already told
/// the user is missing.
pub(crate) fn render_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(_) => "_".to_string(),
        Pattern::Binding { name, .. } => name.clone(),
        Pattern::Literal { expr, .. } => render_literal(expr),
        Pattern::Range { lo, hi, .. } => format!("{}..{}", render_literal(lo), render_literal(hi)),
        Pattern::Concat { parts, .. } => parts.iter().map(render_pattern).collect::<Vec<_>>().join(" .. "),
        Pattern::Or { alts, .. } => alts.iter().map(render_pattern).collect::<Vec<_>>().join(" | "),
        Pattern::Variant { variant, sub, .. } => match sub {
            VariantPattern::Unit => variant.clone(),
            VariantPattern::Positional(pats) => {
                let inner = pats.iter().map(render_pattern).collect::<Vec<_>>().join(", ");
                format!("{variant}({inner})")
            }
            VariantPattern::Named { fields, ignore_rest } => {
                let mut parts: Vec<String> = fields
                    .iter()
                    .map(|(n, p)| format!("{n}: {}", render_pattern(p)))
                    .collect();
                if *ignore_rest {
                    parts.push("..".to_string());
                }
                format!("{variant} {{ {} }}", parts.join(", "))
            }
        },
    }
}

/// A literal pattern's source spelling.
fn render_literal(e: &Expr) -> String {
    match e {
        Expr::IntLit { text, .. } | Expr::FloatLit { text, .. } => text.clone(),
        Expr::StringLit { value, .. } => format!("{value:?}"),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::AtomLit { name, .. } => format!(":{name}"),
        _ => "_".to_string(),
    }
}

/// The `WS065` message for a match pattern whose payload bracket form does not
/// match the variant it names (the pattern-side analog of
/// [`ws065_positional_form_wrong`]/[`ws065_named_form_wrong`]).
fn ws065_pattern_form_wrong(variant: &str, payload: &crate::typecheck::enums::Payload) -> String {
    use crate::typecheck::enums::Payload;
    match payload {
        Payload::Unit => {
            format!("variant `{variant}` takes no payload - match it as `{variant}`")
        }
        Payload::Positional(_) => {
            format!("variant `{variant}` has a positional payload - match it as `{variant}(..)`")
        }
        Payload::Named(_) => {
            format!("variant `{variant}` has named fields - match it as `{variant} {{ .. }}`")
        }
    }
}

/// Resolve a variant payload field's declared type at a match site, applying
/// the scrutinee enum's generic arguments. A bare type-parameter name maps
/// directly to the matching scrutinee arg (so `Some(x)` on an `Option<int>`
/// binds `x: int` without resolving a bare `T` out of scope, which would
/// spuriously WS002); any other type resolves normally and then has the
/// enum's parameters substituted through it.
fn resolve_payload_field_type(
    ctx: &mut TypeCheckCtx,
    te: &crate::ast::TypeExpr,
    edef: &crate::typecheck::enums::EnumDef,
    args: &[Type],
) -> Type {
    if let crate::ast::TypeExpr::Name { name, .. } = te
        && let Some(idx) = edef.type_params.iter().position(|p| &p.name == name)
    {
        return args.get(idx).cloned().unwrap_or(Type::Any);
    }
    let base = resolve_type_expr(ctx, te);
    if edef.type_params.is_empty() {
        return base;
    }
    let subst: crate::types::infer::Subst = edef
        .type_params
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.clone(), args.get(i).cloned().unwrap_or(Type::Any)))
        .collect();
    crate::types::mono::substitute(&base, &subst)
}

/// Bind every capture inside a variant sub-pattern as `any` - the recovery
/// path taken when the variant itself can't be resolved (unknown variant, a
/// bracket-form mismatch, or a nested field that isn't an enum), so the arm
/// body can still reference the captured names without cascading WS002.
fn bind_sub_as_any(ctx: &mut TypeCheckCtx, sub: &VariantPattern, mutable: bool) {
    match sub {
        VariantPattern::Unit => {}
        VariantPattern::Positional(pats) => {
            for p in pats {
                check_match_pattern(ctx, p, &Type::Any, mutable);
            }
        }
        VariantPattern::Named { fields, .. } => {
            for (_, p) in fields {
                check_match_pattern(ctx, p, &Type::Any, mutable);
            }
        }
    }
}

/// How a bare identifier pattern reads against an enum scrutinee.
enum BarePattern {
    /// Names a unit variant of the enum: a variant test that binds nothing.
    UnitVariant,
    /// Names a variant that HAS a payload: `Circle` where `Circle(_)` was meant.
    PayloadVariant,
    /// Names no variant of the enum: a typo, or a deliberate named catch-all.
    Unknown,
}

/// Validate an arm pattern against the type it matches and bind its captures
/// into the current scope. Keeps the SAME unit-variant reinterpretation the
/// usefulness engine (`patterns::head_variant_name`) uses: a bare identifier
/// naming a unit variant is a variant test that binds nothing, while any
/// other bare identifier captures the whole matched value. Emits WS060 for an
/// unknown variant, WS065 for a payload whose bracket form or arity does not
/// match the variant, and WS067 for a bare capitalised name that is no unit
/// variant of the scrutinee's enum.
///
/// `mutable` says whether the SCRUTINEE is writable storage
/// (`scrutinee_is_mutable`). Lowering binds each capture as a compile-time move
/// of the scrutinee's `__{Variant}_{field}` slot, so writing a capture off a
/// storage-backed scrutinee writes the payload in place. That is the only way
/// to mutate a payload, since the surface `e.field` spelling has no lowering at
/// all, and it needs the captures registered as `Var` or `infer_assign_target`
/// rejects the write as a `let`. Captures off anything else stay `LetBinding`,
/// where a write has no slot to land on.
pub(super) fn check_match_pattern(
    ctx: &mut TypeCheckCtx,
    pat: &Pattern,
    matched: &Type,
    mutable: bool,
) {
    use crate::typecheck::enums::Payload;
    match pat {
        Pattern::Wildcard(_) => {}
        Pattern::Binding { name, range } => {
            // Classified under the `enum_defs` borrow because the diagnostics
            // below need `ctx` mutably.
            let bare = match matched {
                Type::Enum { name: en, .. } => ctx.enum_defs.of(matched).map(|edef| {
                    let kind = match edef.variants.iter().find(|v| &v.name == name) {
                        Some(v) if matches!(v.payload, Payload::Unit) => BarePattern::UnitVariant,
                        Some(_) => BarePattern::PayloadVariant,
                        None => BarePattern::Unknown,
                    };
                    (en.clone(), kind)
                }),
                _ => None,
            };
            match bare {
                Some((_, BarePattern::UnitVariant)) => return,
                Some((_, BarePattern::PayloadVariant)) => ctx.warn(
                    "WS067",
                    format!(
                        "`{name}` names a variant that has a payload, but written bare it binds \
                         the whole value like a catch-all (which can leave later arms \
                         unreachable). Did you mean `{name}(_)`?"
                    ),
                    range.clone(),
                ),
                // A capitalised name that is no variant at all is a typo, and
                // the capture reading is irrefutable: the arm swallows every
                // value, so later arms are deleted and an `if let` loses its
                // discriminant test entirely, with no wrong-looking IR to find
                // it by. The qualified spelling of the same typo is WS060, so
                // this is an error too. Lowercase stays legal - that is how a
                // deliberate named catch-all (`other => ...`) is spelled.
                Some((en, BarePattern::Unknown)) if name.starts_with(char::is_uppercase) => {
                    ctx.emit(
                        "WS067",
                        format!(
                            "enum `{en}` has no variant `{name}`, and a bare name that is not a \
                             variant binds the whole value like a catch-all instead of testing \
                             for one - every later arm becomes unreachable"
                        ),
                        range.clone(),
                    )
                }
                _ => {}
            }
            ctx.pattern_bound.push(name.clone());
            if !mutable {
                // Remember this as a capture, so a later write to it is
                // rejected in capture terms rather than as a plain `let`.
                ctx.readonly_captures
                    .insert((range.file.clone(), range.start.offset));
            }
            // A mutable capture names its payload slot, so it is typed like a
            // var: the storage wrapping what the slot holds. Unwrapped, a `*T`
            // payload read as a `T` place instead of the ref the slot holds.
            let ty = if mutable && matches!(matched, Type::Ref(_)) {
                Type::Ref(Box::new(matched.clone()))
            } else {
                matched.clone()
            };
            // Hover finds a capture's type here, at the name it is bound by.
            ctx.record_expr_type(range, &ty);
            ctx.scope.declare(
                name,
                SymbolInfo {
                    kind: if mutable {
                        SymbolKind::Var
                    } else {
                        SymbolKind::LetBinding
                    },
                    name: name.clone(),
                    ty,
                    decl_range: range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        Pattern::Literal { expr, range } => check_literal_pattern(ctx, expr, matched, range),
        Pattern::Range { lo, hi, range } => {
            if !matches!(unwrap_ref(matched), Type::Int | Type::Float | Type::Any) {
                ctx.emit(
                    "WS120",
                    format!(
                        "a range pattern tests a number, but the value being matched is {}",
                        crate::analysis::types::type_str(matched)
                    ),
                    range.clone(),
                );
                return;
            }
            check_literal_pattern(ctx, lo, matched, range);
            check_literal_pattern(ctx, hi, matched, range);
        }
        Pattern::Concat { parts, template, range } => {
            if !matches!(unwrap_ref(matched), Type::String | Type::Any) {
                ctx.emit(
                    "WS120",
                    format!(
                        "a string pattern tests a string, but the value being matched is {}",
                        crate::analysis::types::type_str(matched)
                    ),
                    range.clone(),
                );
                return;
            }
            let mut after_hole = false;
            for part in parts {
                match part {
                    Pattern::Literal { expr, .. } if matches!(**expr, Expr::StringLit { .. }) => {
                        infer(ctx, expr);
                        after_hole = false;
                    }
                    Pattern::Binding { .. } | Pattern::Wildcard(_) => {
                        if after_hole {
                            ctx.emit(
                                "WS122",
                                "two holes side by side in a string pattern: nothing says where \
                                 one ends and the next begins. Put a string literal between them"
                                    .to_string(),
                                part.range().clone(),
                            );
                        }
                        after_hole = true;
                        // `"hi ${name}"` reads like it uses the `name` in
                        // scope, but in a pattern it binds a new one.
                        if *template
                            && let Pattern::Binding { name, range } = part
                            && ctx.scope.binding_frame(name).is_some_and(|(_, s)| {
                                !matches!(
                                    s.kind,
                                    SymbolKind::Fn
                                        | SymbolKind::Chip
                                        | SymbolKind::Event
                                        | SymbolKind::ChipInstance
                                        | SymbolKind::Namespace
                                        | SymbolKind::Type
                                )
                            })
                        {
                            ctx.warn(
                                "WS123",
                                format!(
                                    "`${{{name}}}` binds a new `{name}` to the text it matches; it \
                                     does not compare with the `{name}` already in scope. To match \
                                     that value, bind another name and add a guard: \
                                     `\"...${{s}}...\" if s == {name} =>`"
                                ),
                                range.clone(),
                            );
                        }
                        check_match_pattern(ctx, part, &Type::String, false);
                    }
                    other => ctx.emit(
                        "WS122",
                        "each part of a string pattern is a string literal or a hole: a name \
                         or `_`, written bare after `..` or inside `${}`"
                            .to_string(),
                        other.range().clone(),
                    ),
                }
            }
        }
        Pattern::Or { alts, .. } => {
            for alt in alts {
                // A bare unit-variant name binds nothing; only what the
                // alternative really binds counts.
                let before = ctx.pattern_bound.len();
                check_match_pattern(ctx, alt, matched, false);
                if let Some(name) = ctx.pattern_bound.get(before).cloned() {
                    ctx.emit(
                        "WS121",
                        format!(
                            "an alternative of `|` can't bind a name (`{name}`): only one \
                             alternative matches, so the others would leave it unset. Match it \
                             with `_`, or give the alternatives arms of their own"
                        ),
                        alt.range().clone(),
                    );
                }
            }
        }
        Pattern::Variant { enum_path, variant, sub, range } => {
            let Type::Enum { name: en, args } = matched else {
                if !matches!(unwrap_ref(matched), Type::Any) {
                    ctx.emit(
                        "WS120",
                        format!(
                            "`{variant}` is an enum variant, but the value being matched is {}",
                            crate::analysis::types::type_str(matched)
                        ),
                        range.clone(),
                    );
                }
                bind_sub_as_any(ctx, sub, mutable);
                return;
            };
            // `Shape.Circle(r)` on a `Shape` scrutinee is the qualified
            // spelling construction and `is` both require; naming a DIFFERENT
            // enum is the mistake it exists to catch, since the variant name
            // alone can belong to several enums.
            let written = enum_path.as_deref().map(|q| match q.split_once('.') {
                Some((ns, name)) if ctx.enum_defs.namespaced(&range.file, ns, name).is_some() => name,
                _ => q,
            });
            if let Some(q) = written
                && q != en
                && !q.is_empty()
            {
                ctx.emit(
                    "WS060",
                    format!(
                        "pattern names enum `{q}`, but the value being matched is a `{en}`"
                    ),
                    range.clone(),
                );
            }
            let edef = match ctx.enum_defs.of(matched) {
                Some(d) => d.clone(),
                None => {
                    bind_sub_as_any(ctx, sub, mutable);
                    return;
                }
            };
            let Some(vdef) = edef.variants.iter().find(|v| &v.name == variant).cloned() else {
                ctx.emit(
                    "WS060",
                    format!("enum `{en}` has no variant `{variant}`"),
                    range.clone(),
                );
                bind_sub_as_any(ctx, sub, mutable);
                return;
            };
            match (&vdef.payload, sub) {
                (Payload::Unit, VariantPattern::Unit) => {}
                (Payload::Positional(types), VariantPattern::Positional(pats)) => {
                    if pats.len() != types.len() {
                        ctx.emit(
                            "WS065",
                            format!(
                                "variant `{variant}` binds {} value(s), but the pattern has {}",
                                types.len(),
                                pats.len()
                            ),
                            range.clone(),
                        );
                    }
                    for (i, p) in pats.iter().enumerate() {
                        let fty = match types.get(i) {
                            Some(te) => resolve_payload_field_type(ctx, te, &edef, args),
                            None => Type::Any,
                        };
                        check_match_pattern(ctx, p, &fty, mutable);
                    }
                }
                (Payload::Named(decl_fields), VariantPattern::Named { fields, .. }) => {
                    for (fname, fpat) in fields {
                        let fty = match decl_fields.iter().find(|(n, _)| n == fname) {
                            Some((_, te)) => resolve_payload_field_type(ctx, te, &edef, args),
                            None => {
                                // Same typo the construction side flags (WS010) -
                                // a named-field capture whose name is not a field
                                // of the variant. Loud, then bind as `any` so the
                                // body does not cascade WS002 on the capture.
                                ctx.emit(
                                    "WS010",
                                    format!("variant `{variant}` has no field `{fname}`"),
                                    pattern_range(fpat).clone(),
                                );
                                Type::Any
                            }
                        };
                        check_match_pattern(ctx, fpat, &fty, mutable);
                    }
                }
                _ => {
                    ctx.emit("WS065", ws065_pattern_form_wrong(variant, &vdef.payload), range.clone());
                    bind_sub_as_any(ctx, sub, mutable);
                }
            }
        }
    }
}

/// A single value `match`, `if let` and `let ... else` can test with
/// literal, range and string patterns.
pub(crate) fn is_matchable_value(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Float | Type::Bool | Type::String)
}

/// The whole pattern of a test of a single value, beyond what
/// [`check_match_pattern`] checks: a bare capitalized name reads as an enum
/// variant (`Red`, where the value turned out to be an int, a generic member
/// say) but catches every value instead of failing to match.
pub(crate) fn check_value_pattern_root(ctx: &mut TypeCheckCtx, pattern: &Pattern, scrut_ty: &Type) {
    for alt in crate::typecheck::patterns::or_alternatives(pattern) {
        if let Pattern::Binding { name, range } = alt
            && name.starts_with(char::is_uppercase)
        {
            ctx.emit(
                "WS120",
                format!(
                    "`{name}` reads as an enum variant, but the value being matched is {}, \
                     and a bare name catches every value. A name meant to catch the rest \
                     starts lowercase",
                    crate::analysis::types::type_str(scrut_ty)
                ),
                range.clone(),
            );
        }
    }
}

/// A literal pattern `expr` against a value of type `matched`: the same type,
/// or an integer against a float (which it is then built as).
fn check_literal_pattern(ctx: &mut TypeCheckCtx, expr: &Expr, matched: &Type, range: &SourceRange) {
    let lit_ty = infer(ctx, expr);
    let matched = unwrap_ref(matched);
    let fits = match (&matched, &lit_ty) {
        (Type::Any, _) => true,
        (Type::Float, Type::Int) => {
            int_constants_in_slot(ctx, expr, &Type::Float);
            true
        }
        (m, l) => m == l,
    };
    if fits {
        return;
    }
    let message = match &matched {
        Type::Enum { name, .. } => format!(
            "`{name}` is an enum: match it by its variants, not with {} literals",
            crate::analysis::types::type_str(&lit_ty)
        ),
        _ => format!(
            "this pattern is {}, but the value being matched is {}",
            crate::analysis::types::type_str(&lit_ty),
            crate::analysis::types::type_str(&matched)
        ),
    };
    ctx.emit("WS120", message, range.clone());
}

/// Type an arm body, returning its value type for the arm-result join. An
/// expression arm contributes its inferred type; a block arm runs for its
/// side effects (statement errors surface) but contributes no value type.
/// The instantiation an `if`/`match` value checked against an enum type gives
/// each of its branches: every branch is a value of that type, so a generic
/// enum constructed in one (`Opt.Some(7)`, `Opt.None`) takes its type
/// arguments from it, as the same construction does checked against the type
/// directly.
fn enum_branch_hint(expected: Option<&Type>) -> Option<Type> {
    expected.map(unwrap_ref).filter(|t| matches!(t, Type::Enum { .. }))
}

/// `infer` with `hint` as the expected type of `e`'s own node (see
/// [`enum_branch_hint`]).
pub(crate) fn infer_hinted(ctx: &mut TypeCheckCtx, e: &Expr, hint: Option<&Type>) -> Type {
    let prev = std::mem::replace(&mut ctx.expected_ty, hint.cloned());
    let t = infer(ctx, e);
    ctx.expected_ty = prev;
    t
}

/// The enum type the parameter at `index` of the user `mod`/`chip` `callee`
/// names, when the argument `arg` for it is an `if` or `match` value whose
/// branches build that enum (see [`enum_branch_hint`]). A parameter typed by
/// one of the callee's own type parameters gives none: what it binds is
/// inferred from the arguments.
fn branch_arg_hint(ctx: &mut TypeCheckCtx, callee: &Expr, index: usize, arg: &Expr) -> Option<Type> {
    if !matches!(arg, Expr::IfExpr { .. } | Expr::MatchExpr { .. }) {
        return None;
    }
    let Expr::Ident { name, range } = callee else { return None };
    if !ctx.calls_user_mod(name, &range.file) {
        return None;
    }
    let decl = ctx.resolve_mod(name)?;
    let param = decl.inputs.get(index)?;
    if !decl.type_params.is_empty() {
        return None;
    }
    let t = resolve_type_expr(ctx, &param.typ);
    enum_branch_hint(Some(&t)).filter(|t| !super::resolve::type_has_param(t))
}

fn infer_match_body(ctx: &mut TypeCheckCtx, body: &MatchBody) -> Option<Type> {
    match body {
        MatchBody::Expr(expr) => Some(infer(ctx, expr)),
        MatchBody::Block(block) => {
            for s in &block.stmts {
                check_stmt(ctx, s);
            }
            None
        }
    }
}

/// Whether the record/tuple projection `e` (`r.f`, `r.a.b`, `t.0`) reads a
/// PLACE, so a `*T` field there names storage and reads as its target: a
/// field of a variable or parameter (a `*Record` parameter's distributed
/// fields, a record parameter's ref field bound to the caller's), or of a
/// record `let` that bundles vars (`{ cpsr, skip }`), through any projection
/// or namespace. A field that arrived on a wire (a map or array read, a call's
/// result, event data, a literal's `p: &a` or `p: h` with `h: *int`) is a
/// record VALUE whose `*T` field is the ref itself. Classified by
/// `let_binding::ref_values_at`, which lowering's record bindings follow.
pub(crate) fn projection_is_place(ctx: &TypeCheckCtx, e: &Expr) -> bool {
    let (root, segs) = flatten_access(e);
    !matches!(
        super::let_binding::ref_values_at(ctx, root, &segs),
        Some(super::let_binding::LetRefValues::All)
    )
}

/// Whether `e` names storage a `*T` record field can hold as a place: a var, a
/// `*T` parameter, or a projection or `let` that bundles vars.
fn names_a_place(ctx: &TypeCheckCtx, e: &Expr) -> bool {
    match e {
        Expr::Ident { name, .. } => ctx.scope.lookup(name).is_some_and(|sym| match sym.kind {
            SymbolKind::Var => true,
            SymbolKind::Param => matches!(sym.ty, Type::Ref(_)),
            SymbolKind::LetBinding => projection_is_place(ctx, e),
            _ => false,
        }),
        Expr::FieldAccess { .. } => super::sig::field_names_storage(ctx, e),
        _ => false,
    }
}

/// Split an `unsafe` access's inner chain into its base value and the named
/// segments applied to it. A positional payload is reached with `.0`, which
/// parses as a `TuplePick`, so both projection forms are flattened here.
pub(crate) fn flatten_access(e: &Expr) -> (&Expr, Vec<String>) {
    match e {
        Expr::FieldAccess { obj, field, .. } => {
            let (base, mut segs) = flatten_access(obj);
            segs.push(field.clone());
            (base, segs)
        }
        Expr::TuplePick { obj, index, .. } => {
            let (base, mut segs) = flatten_access(obj);
            segs.push(index.to_string());
            (base, segs)
        }
        _ => (e, Vec::new()),
    }
}

/// Type of `unsafe <value>.<Variant>.<field>…`, shared by the read and the
/// assignment-target paths so both agree on the slot and on what they reject.
///
/// The first two segments select the payload slot: the variant asserts which
/// one the value is (never checked, that is the point of the spelling) and the
/// field names its slot. Any further segments project into a record-typed
/// payload exactly as they would on a plain record.
/// `value is Enum.Variant`: an enum value on the left, a variant PATH on the
/// right, `bool` out. The right side must name a variant rather than merely be
/// enum-typed, since `a is b` compares two values and `is` deliberately does
/// not spell that. Both sides must be the same enum, because discriminants are
/// only comparable within one.
///
/// Every failure recovers as `Type::Bool` so a surrounding condition doesn't
/// cascade a second mismatch off an `Any`.
pub(super) fn infer_variant_test(
    ctx: &mut TypeCheckCtx,
    value: &Expr,
    path: &Expr,
    range: &SourceRange,
) -> Type {
    // A gate read whose first output is an enum tests that enum.
    let value_ty = first_output(&unwrap_ref(&infer(ctx, value)));
    // Infers the path first, so an unknown variant reports WS060 from the
    // shared resolution rather than the weaker "not a variant" below.
    let path_ty = infer(ctx, path);
    if !matches!(value_ty, Type::Enum { .. }) {
        ctx.emit(
            "WS066",
            format!(
                "`is` needs an enum value on the left, found `{}`",
                crate::analysis::types::type_str(&value_ty)
            ),
            value.range().clone(),
        );
        return Type::Bool;
    }
    // A variant path is a `.`-chain whose last segment names a variant of the
    // enum it resolved to (`Shape.Circle`, `ns.Shape.Circle`).
    let names_a_variant = match (path, &path_ty) {
        (Expr::FieldAccess { field, .. }, t @ Type::Enum { .. }) => {
            ctx.enum_defs.of(t).is_some_and(|def| def.variants.iter().any(|v| &v.name == field))
        }
        _ => false,
    };
    if !names_a_variant {
        // An unknown variant already reported WS060; adding a second
        // diagnostic on the same span would just be noise.
        if !matches!(path_ty, Type::Any) {
            ctx.emit(
                "WS066",
                format!(
                    "`is` needs an enum variant on the right (`Enum.Variant`), found `{}`",
                    crate::analysis::types::type_str(&path_ty)
                ),
                path.range().clone(),
            );
        }
        return Type::Bool;
    }
    if let (Type::Enum { name: vn, .. }, Type::Enum { name: pn, .. }) = (&value_ty, &path_ty)
        && vn != pn
    {
        ctx.emit(
            "WS003",
            format!("`is` needs a variant of `{vn}`, found one of `{pn}`"),
            path.range().clone(),
        );
    }
    // Lowering desugars the test into `==` over the two discriminants, carrying
    // this node's range, and `lower_binop` reads its operator rule out of this
    // map by range. Without the entry the comparison has no rule to lower with
    // and falls back to an `_Unsupported` placeholder.
    if let Some(rule) = resolve_op("==", &[Type::Int, Type::Int]) {
        ctx.op_resolutions.insert(
            (range.file.clone(), range.start.offset, range.end.offset),
            rule.clone(),
        );
    }
    Type::Bool
}

pub(super) fn infer_unsafe_access(ctx: &mut TypeCheckCtx, inner: &Expr, range: &SourceRange) -> Type {
    let (base, segs) = flatten_access(inner);
    let base_ty = unwrap_ref(&infer(ctx, base));
    let Type::Enum { name: en, args } = &base_ty else {
        ctx.emit(
            "WS066",
            format!(
                "`unsafe` payload access needs an enum value, found `{}`",
                crate::analysis::types::type_str(&base_ty)
            ),
            range.clone(),
        );
        return Type::Any;
    };
    let (Some(variant), Some(field)) = (segs.first(), segs.get(1)) else {
        ctx.emit(
            "WS070",
            format!(
                "`unsafe` needs a variant and a payload field (`unsafe <value>.<Variant>.<field>`), found `{}`",
                segs.join(".")
            ),
            range.clone(),
        );
        return Type::Any;
    };
    let Some(edef) = ctx.enum_defs.of(&base_ty).cloned() else {
        return Type::Any;
    };
    let Some(vdef) = edef.variants.iter().find(|v| &v.name == variant).cloned() else {
        ctx.emit(
            "WS060",
            format!("enum `{en}` has no variant `{variant}`"),
            range.clone(),
        );
        return Type::Any;
    };
    let args = args.clone();
    let slot_ty = match &vdef.payload {
        crate::typecheck::enums::Payload::Unit => {
            ctx.emit(
                "WS010",
                format!("variant `{variant}` of enum `{en}` carries no payload"),
                range.clone(),
            );
            return Type::Any;
        }
        crate::typecheck::enums::Payload::Named(fields) => {
            match fields.iter().find(|(n, _)| n == field) {
                Some((_, te)) => resolve_payload_field_type(ctx, te, &edef, &args),
                None => {
                    ctx.emit(
                        "WS010",
                        format!("variant `{variant}` has no field `{field}`"),
                        range.clone(),
                    );
                    return Type::Any;
                }
            }
        }
        crate::typecheck::enums::Payload::Positional(types) => {
            match field.parse::<usize>().ok().and_then(|i| types.get(i)) {
                Some(te) => resolve_payload_field_type(ctx, te, &edef, &args),
                None => {
                    ctx.emit(
                        "WS010",
                        format!(
                            "variant `{variant}` has {} payload value(s), so `.{field}` names none of them",
                            types.len()
                        ),
                        range.clone(),
                    );
                    return Type::Any;
                }
            }
        }
    };
    // Remaining segments project into a record-typed payload.
    let mut ty = slot_ty;
    for seg in &segs[2..] {
        let Type::Record(fields) = &unwrap_ref(&ty) else {
            ctx.emit(
                "WS010",
                format!(
                    "no field `{seg}` on `{}`",
                    crate::analysis::types::type_str(&ty)
                ),
                range.clone(),
            );
            return Type::Any;
        };
        match fields.iter().find(|(n, _)| n == seg) {
            Some((_, t)) => ty = t.clone(),
            None => {
                ctx.emit(
                    "WS010",
                    format!("no field `{seg}` on record"),
                    range.clone(),
                );
                return Type::Any;
            }
        }
    }
    ty
}

/// Operator typing when either operand is a reference (a `*T`, an array or
/// map ref, or a brick). `None` when neither is, so ordinary overload
/// resolution runs.
fn ref_operator_type(
    ctx: &mut TypeCheckCtx,
    op: &str,
    lt: &Type,
    rt: &Type,
    range: &SourceRange,
) -> Option<Type> {
    let (lk, rk) = (lt.ref_kind(), rt.ref_kind());
    if lk.is_none() && rk.is_none() {
        return None;
    }
    if !matches!(op, "==" | "!=") {
        ctx.emit(
            "WS004",
            format!(
                "operator '{op}' cannot be applied to a reference ({lt}, {rt}): refs and \
                 bricks only compare with `==` and `!=`"
            ),
            range.clone(),
        );
        return Some(Type::Any);
    }
    // `Any` is the generic unknown/error-fallback type, not an operator
    // wildcard (`catalog::operators::type_kind_matches`'s NOTE): many of its
    // ~150 producers emit no diagnostic of their own (e.g. an out-of-range
    // record `.9` pick), so treating a ref-vs-`Any` comparison as
    // pre-resolved here would silently swallow WS004. Fall through to
    // ordinary overload resolution instead, which reports it exactly as it
    // would for a non-ref operand (`row0 == <any>`).
    if matches!(lt, Type::Any) || matches!(rt, Type::Any) {
        return None;
    }
    let plain = |t: &Type| t.ref_kind().is_none() && !matches!(t, Type::Opaque);
    if plain(lt) || plain(rt) {
        ctx.emit(
            "WS084",
            format!(
                "cannot compare {lt} with {rt}: a ref or brick only compares with another of \
                 its own kind, and the game treats `1 == <ref>` as true, so the answer would \
                 mean nothing"
            ),
            range.clone(),
        );
        return Some(Type::Any);
    }
    if lk.is_some() && rk.is_some() && coerce(lt, rt) != CoerceRule::Same {
        ctx.emit(
            "WS003",
            format!("cannot compare {lt} with {rt}: they are refs to different types"),
            range.clone(),
        );
        return Some(Type::Any);
    }
    let rule = resolve_op(op, &[lt.clone(), rt.clone()])?;
    ctx.op_resolutions
        .insert((range.file.clone(), range.start.offset, range.end.offset), rule.clone());
    Some(rule.result.clone())
}

/// The types of a binary operator's operands. `null` compared with `==` or
/// `!=` takes the other operand's type: an unset object, an empty brick or
/// ref, or the zero of a value type.
fn infer_operands(ctx: &mut TypeCheckCtx, op: &str, left: &Expr, right: &Expr) -> (Type, Type) {
    let is_null = |e: &Expr| matches!(e, Expr::NullLit { .. });
    if !matches!(op, "==" | "!=") || is_null(left) == is_null(right) {
        return (infer(ctx, left), infer(ctx, right));
    }
    let (null, other) = if is_null(left) { (left, right) } else { (right, left) };
    let other_t = infer(ctx, other);
    let null_t = if other_t.ref_kind().is_some() {
        ctx.record_expr_type(null.range(), &other_t);
        other_t.clone()
    } else {
        check_null(ctx, null, &op_operand_type(&other_t))
    };
    if is_null(left) { (null_t, other_t) } else { (other_t, null_t) }
}

/// Node dispatch, exhaustive over every `Expr` variant.
fn infer_node(ctx: &mut TypeCheckCtx, e: &Expr) -> Type {
    // The `check`-supplied expected type applies to THIS node only; take it
    // once here so a nested inference (an operand, an arg) never reads a stale
    // hint. Only generic enum construction consumes it (`expected`, below).
    let expected = ctx.expected_ty.take();
    match e {
        Expr::Unsafe { inner, range } => infer_unsafe_access(ctx, inner, range),
        Expr::Is { value, path, range } => infer_variant_test(ctx, value, path, range),
        Expr::As { value, ty, range } => super::convert::infer_as(ctx, value, ty, range),
        Expr::IntLit { .. } => Type::Int,
        Expr::AtomLit { .. } => Type::Int,
        Expr::FloatLit { .. } => Type::Float,
        Expr::StringLit { .. } => Type::String,
        Expr::BoolLit { .. } => Type::Bool,
        // `null` is polymorphic — it has no type on its own, so a bare inferred
        // position (`let x = null`) reads as `any`. A TYPED position resolves it
        // to that type via `check`/`check_null` (var/out init, assignment, a
        // call arg, a record field), which is where `null` earns a real value.
        Expr::NullLit { .. } => Type::Any,
        // `emit` in value position is the current exec chain point. There is no
        // chain to name in pure context, so that is an error rather than a
        // silent resolution to nothing.
        Expr::CurrentExec { range } => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "`emit` as a value names the current exec chain point, and there \
                     is none outside an exec context",
                    range.clone(),
                );
            }
            Type::Exec
        }
        Expr::InterpLit { parts, .. } => {
            // Match legacy exactly: unwrap a leading `Ref` off the part's type
            // before the string-coercion check, so an interpolated var ref
            // (`${&x}`) coerces through its inner type rather than tripping ref
            // invariance. (Not routed through `check`, which skips the unwrap.)
            for p in parts {
                if let InterpPart::Expr(expr) = p {
                    let t = unwrap_ref(&infer(ctx, expr));
                    if !record_read_as_one_value(ctx, expr, &t) {
                        coerce_or_emit(ctx, &t, &Type::String, expr.range());
                    }
                }
            }
            Type::String
        }
        Expr::AssetRef { asset_type, asset_name, range } => {
            if asset_type == crate::analysis::assets::COMPONENT {
                if let Some(why) = component_ref_problem(asset_name) {
                    ctx.emit("WS110", format!("`${asset_type}/{asset_name}` {why}"), range.clone());
                    return Type::Any;
                }
                // `$Component/PointLight` is that component type; the property
                // form names a property and has no value of its own.
                return match crate::catalog::component_type(asset_name) {
                    Some(o) => Type::Component(o.class.to_string()),
                    None => Type::Any,
                };
            }
            // An external asset reference (`$Type/Name`) is an object/class
            // reference — typed `entity` so it can be compared against entity
            // values (e.g. `weapon == $BRItemBase/Weapon_Pickaxe`) and passed
            // into object/class gate ports (which accept `any`/entity anyway).
            // Validation against the asset catalog happens in analysis, except
            // a component type's, checked above.
            Type::Entity
        }
        Expr::PrefabRef { path, range } => {
            // A prefab file reference flows into a gate's `Prefab` path
            // property; typed `Type::PrefabRef` (a compile-time-constant
            // reference — reaches the `any`-typed `prefab=` param via the
            // universal `Any` coercion rule). `.brz` is a prebuilt archive
            // (resolved + embedded at emit); `.ws` is a SOURCE prefab the
            // compile path reads + compiles (see `disk_prefab_resolver`). For a
            // `.ws` reference, type-check the referenced file and surface its
            // diagnostics on THIS reference span, so an error inside
            // `control.ws` underlines the `$./control.ws` reference in the
            // editor (mirrors the inline `$```…```` block below).
            if path.ends_with(".ws") {
                check_ws_prefab_ref(ctx, path, range);
            } else if !path.ends_with(".brz") {
                ctx.emit(
                    "WS019",
                    format!(
                        "prefab reference `${path}` must end in `.brz` (a prebuilt archive) \
                         or `.ws` (a source file compiled on reference)"
                    ),
                    range.clone(),
                );
            }
            Type::PrefabRef
        }
        Expr::NestedPrefab { source, range } => {
            // Inline nested-prefab block `$``` ... ``` ` — like `PrefabRef`, it
            // flows into a gate's `Prefab` path; typed
            // `Type::PrefabRef`, and (unlike `PrefabRef`) there's no `.brz`/WS019
            // path check. Additionally, type-check the enclosed source as its own
            // isolated program and surface its diagnostics shifted into this
            // block's span in the outer file, so an error inside the block
            // underlines the real location in the editor.
            let inner = crate::parser::parse(source, &range.file);
            // Imports in a nested block resolve at compile time (the driver
            // recursively resolves + compiles it), but this in-editor pass has no
            // loader — so when the block imports, surface only its parse
            // diagnostics (skip type-check, whose unresolved imports would be
            // false positives). A self-contained block (the common case) is
            // fully checked.
            let has_imports = inner
                .ast
                .decls
                .iter()
                .any(|d| matches!(d, crate::ast::TopDecl::Import(_)));
            let inner_diags: Vec<Diagnostic> = if has_imports {
                inner.diagnostics
            } else {
                // Run the same two-phase inference the compile path uses for a
                // prefab unit, so a self-contained block's custom-event slots
                // infer from its in-block senders (and default to float + WS042
                // when uninferable) — otherwise the block type-checks cleaner
                // here than it compiles, hiding real `got float` errors until
                // emit time.
                let inner_tc = crate::typecheck::typecheck_with_inference(&inner.ast, &range.file).0;
                inner
                    .diagnostics
                    .into_iter()
                    .chain(inner_tc.diagnostics)
                    .collect()
            };
            // The inner source begins right after the 4-char `$``` ` fence, so a
            // position at inner offset `o` lands at outer offset `o + bo + 4`.
            // Its first line shares the fence's outer line; later lines are whole
            // lines of the outer file, so their columns map through directly.
            let (bo, bl, bc) = (range.start.offset, range.start.line, range.start.col);
            let shift = |p: crate::diagnostic::Pos| crate::diagnostic::Pos {
                offset: p.offset + bo + 4,
                line: if p.line <= 1 { bl } else { bl + p.line - 1 },
                col: if p.line <= 1 {
                    bc + 4 + p.col.saturating_sub(1)
                } else {
                    p.col
                },
            };
            for mut d in inner_diags {
                d.range = crate::diagnostic::SourceRange {
                    file: range.file.clone(),
                    start: shift(d.range.start),
                    end: shift(d.range.end),
                };
                ctx.diagnostics.push(d);
            }
            Type::PrefabRef
        }
        Expr::Ident { name, range } => {
            let Some(sym) = ctx.scope.lookup(name).cloned() else {
                // Bare unit-variant reference (`None` for `Option.None`, ...):
                // `name` is not a scope symbol, so try resolving it as a
                // unique variant of a registered enum before falling to the
                // unknown-identifier diagnostic below. Keyed off the SAME
                // resolution the qualified `Enum.Variant` `FieldAccess` site
                // below uses, so `None` and `Option.None` check identically.
                if let Some(enum_name) = resolve_bare_variant_enum(ctx, name)
                    && let VariantResolution::Resolved(enum_ty, vdef, type_params) =
                        resolve_variant_for_construction(ctx, &enum_name, name, range)
                {
                    return variant_reference_type(
                        ctx,
                        &enum_name,
                        &vdef,
                        enum_ty,
                        &type_params,
                        expected.as_ref(),
                        range,
                    );
                }
                // Fallback tier: an output port is readable, lowering to its
                // rerouter's `RER_Output`. Reached only when nothing in the
                // ordinary tier owns the name, which is what keeps `var count`
                // plus `out count = count` resolving to the var.
                if let Some(port) = ctx.scope.lookup(&out_scope_key(name)).cloned() {
                    return unwrap_ref(&port.ty);
                }
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                return Type::Any;
            };
            // A library names the top-level values it declares or imports, as
            // it names types and mods: one only a file importing it declares
            // is none of its own.
            if matches!(
                sym.kind,
                SymbolKind::LetBinding
                    | SymbolKind::Var
                    | SymbolKind::Array
                    | SymbolKind::Map
                    | SymbolKind::Buffer
                    | SymbolKind::In
                    | SymbolKind::Out
            ) && !range.file.is_empty()
                && !sym.decl_range.file.is_empty()
                && sym.decl_range.file != range.file
                && ctx.scope.binding_frame(name).is_some_and(|(i, _)| i == 0)
                && ctx.imports.imports_file(&sym.decl_range.file, &range.file)
                && !ctx.top_names.file_sees(&ctx.imports, &range.file, &sym.decl_range.file, name)
            {
                let base = |f: &str| f.rsplit(['/', '\\']).next().unwrap_or(f).to_string();
                ctx.emit(
                    "WS002",
                    format!(
                        "unknown identifier '{name}': `{}` neither declares nor imports it; `{}`, which imports \
                         it, does, and a library names the values it declares or imports, as it names types and mods",
                        base(&range.file),
                        base(&sym.decl_range.file)
                    ),
                    range.clone(),
                );
                return Type::Any;
            }
            #[cfg(test)]
            ctx.ident_decls.push((range.clone(), sym.decl_range.clone(), false));
            match sym.kind {
                SymbolKind::Event => Type::Exec,
                SymbolKind::Fn | SymbolKind::Chip => Type::Any,
                SymbolKind::Var => {
                    let is_exec = ctx.exec_mode() == ExecMode::Exec;
                    ctx.var_read_contexts
                        .insert((range.file.clone(), range.start.offset), is_exec);
                    unwrap_ref(&sym.ty).container_ref_as_value()
                }
                // A destructured field of a `*Record` parameter is a place: it
                // reads as the field's value, like a `*T` parameter does.
                SymbolKind::Param if matches!(sym.ty, Type::Ref(_)) => unwrap_ref(&sym.ty),
                SymbolKind::Array => sym.ty.clone(),
                // A `type` name read as a value: `let c = Box { v: n }` parses
                // as `let c = Box` and a separate `{ v: n }`. An enum's name
                // reaches its variants and stays a value.
                SymbolKind::Type if !matches!(sym.ty, Type::Enum { .. }) => {
                    let generic_record = ctx
                        .generic_type_aliases
                        .get(name.as_str())
                        .is_some_and(|a| matches!(a.body, crate::ast::TypeExpr::Record { .. }));
                    let message = if matches!(sym.ty, Type::Record(_)) || generic_record {
                        format!(
                            "`{name}` is a type, not a value, and a record type has no \
                             constructor call: `{name} {{ .. }}` reads as `{name}` and a separate \
                             `{{ .. }}`. Give the record literal the type instead: \
                             `let x: {name} = {{ .. }}`"
                        )
                    } else {
                        format!(
                            "`{name}` is a type (another name for `{}`), not a value. Write a \
                             value and give it the type: `let x: {name} = ..`",
                            crate::analysis::types::type_str(&sym.ty)
                        )
                    };
                    ctx.emit("WS100", message, range.clone());
                    Type::Any
                }
                _ => sym.ty.clone(),
            }
        }
        Expr::Deref { operand, range } => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS006",
                    format!(
                        "'*{}' deref requires exec context; use .Value for pure reads",
                        target_name(operand).unwrap_or("<expr>".into())
                    ),
                    range.clone(),
                );
            }
            let t = infer(ctx, operand);
            match t {
                Type::Ref(inner) => *inner,
                Type::Any => Type::Any,
                t @ (Type::Array(_) | Type::Map(_, _) | Type::Brick) => {
                    emit_deref_not_a_scalar_ref(ctx, &t, range);
                    Type::Any
                }
                // A bare variable name in exec context ALREADY auto-derefs, so
                // `*x` infers the value type rather than a `ref`. `*x` is
                // documented as the explicit spelling of that same read
                // (`docs/src/expressions.md`), so a deref of something
                // ref-able is the value it just produced, not a mismatch.
                other if super::is_ref_able(ctx, operand) => other,
                other => {
                    ctx.emit(
                        "WS003",
                        format!(
                            "`*` needs a reference, got {}",
                            crate::analysis::types::type_str(&other)
                        ),
                        range.clone(),
                    );
                    Type::Any
                }
            }
        }
        Expr::RefOf { operand, range } => {
            let t = infer(ctx, operand);
            if !matches!(t, Type::Ref(_)) && !super::is_ref_able(ctx, operand) {
                ctx.emit(
                    "WS008",
                    "cannot take `&` of this value: `&` needs a variable, a ref parameter or a \
                     record field of one, and a temporary value or an element of a value \
                     container has no ref",
                    range.clone(),
                );
                return Type::Any;
            }
            // A ref to a ref has no storage in the game.
            if matches!(t, Type::Ref(_)) {
                ctx.emit(
                    "WS086",
                    format!(
                        "`&` of a value that is already a ref ({t}): a ref to a ref has no \
                         storage in the game; use the ref itself"
                    ),
                    range.clone(),
                );
                return Type::Any;
            }
            // One element of an array of values has no ref of its own; only
            // the whole array does.
            if matches!(operand.as_ref(), Expr::IndexAccess { .. }) && t.ref_kind().is_none() {
                ctx.emit(
                    "WS008",
                    format!(
                        "`&` of an array or map element ({t}): an element of a value container \
                         has no ref of its own; pass the whole container"
                    ),
                    range.clone(),
                );
                return Type::Any;
            }
            // An array/map is already a reference, so `&arr` is a no-op — never
            // `*T[]` (which is redundant and drops writes).
            // A record has no single ref: `&r` is the ref of each field, the
            // form a `*Record` parameter or output already takes.
            if t.is_reference_backed() {
                t
            } else if matches!(t, Type::Record(_) | Type::Outputs(_) | Type::Tuple(_)) {
                crate::types::resolve::distribute_ref(t)
            } else {
                Type::Ref(Box::new(t))
            }
        }
        Expr::UnOp { op, operand, range } => {
            let operand_t = infer(ctx, operand);
            if matches!(operand_t, Type::Never) {
                crate::typecheck::let_binding::emit_no_value(ctx, None, operand.range());
                return Type::Any;
            }
            if operand_t.ref_kind().is_some() {
                ctx.emit(
                    "WS004",
                    format!("operator '{op}' cannot be applied to a reference ({operand_t})"),
                    range.clone(),
                );
                return Type::Any;
            }
            if record_read_as_one_value(ctx, operand, &operand_t) {
                return Type::Any;
            }
            let unwrapped = op_operand_type(&operand_t);
            // `resolve_op` maps unary `-` to the table's `-u` key internally.
            let rule = resolve_op(op.as_str(), &[unwrapped]);
            if let Some(r) = rule {
                let result = r.result.clone();
                ctx.op_resolutions.insert(
                    (range.file.clone(), range.start.offset, range.end.offset),
                    r.clone(),
                );
                // The one integer whose negation no `int` holds.
                if op == crate::catalog::operators::op::NEG
                    && r.operands.first() == Some(&Type::Int)
                    && int_constant_value(ctx, operand) == Some(i64::MIN)
                {
                    ctx.emit(
                        "WS028",
                        format!(
                            "this negates {}, which makes 9223372036854775808, and no `int` holds it: an `int` \
                             holds whole numbers from {} to {}",
                            i64::MIN,
                            i64::MIN,
                            i64::MAX
                        ),
                        range.clone(),
                    );
                }
                result
            } else if ctx.is_error_typed(operand) {
                Type::Any
            } else {
                let code = if matches!(op.as_str(), "&" | "|" | "^" | "~" | "<<" | ">>") {
                    "WS011"
                } else {
                    "WS004"
                };
                ctx.emit(
                    code,
                    format!("no overload for '{op}' on {}", crate::analysis::types::type_str(&operand_t)),
                    range.clone(),
                );
                Type::Any
            }
        }
        Expr::BinOp {
            op,
            left,
            right,
            range,
        } => {
            let (lt, rt) = infer_operands(ctx, op, left, right);
            // An operand with no value at all reports THAT, rather than a
            // missing overload naming `never` - a type the author never wrote.
            for (t, e) in [(&lt, left.as_ref()), (&rt, right.as_ref())] {
                if matches!(t, Type::Never) {
                    crate::typecheck::let_binding::emit_no_value(ctx, None, e.range());
                    return Type::Any;
                }
            }
            if let Some(t) = ref_operator_type(ctx, op, &lt, &rt, range) {
                return t;
            }
            // Comparing two whole records has no lowering: each operand's
            // first-field projection in `op_operand_type` lets `rec == rec`
            // typecheck, then both sides lower to `_Unsupported`. A record
            // compared against a scalar is left alone, since that projection is
            // the legitimate multi-output auto-unwrap (`ParseInt(s) == 42` reads
            // the gate's first output). A direct CALL operand is never a bare
            // record for this purpose; it auto-unwraps at lowering.
            let bare_record = |e: &Expr, t: &Type| {
                matches!(unwrap_ref(t), Type::Record(_)) && !matches!(e, Expr::Call { .. })
            };
            if bare_record(left.as_ref(), &lt) && bare_record(right.as_ref(), &rt) {
                ctx.emit(
                    "WS004",
                    format!("operator '{op}' cannot be applied to two record values"),
                    range.clone(),
                );
                Type::Any
            } else {
                // `|` so both operands report.
                if record_read_as_one_value(ctx, left, &lt) | record_read_as_one_value(ctx, right, &rt) {
                    return Type::Any;
                }
                let lt_u = op_operand_type(&lt);
                let rt_u = op_operand_type(&rt);
                let rule = resolve_op(op, &[lt_u, rt_u]);
                if let Some(r) = rule {
                    let result = r.result.clone();
                    ctx.op_resolutions.insert(
                        (range.file.clone(), range.start.offset, range.end.offset),
                        r.clone(),
                    );
                    result
                } else {
                    // An operand whose type is an error already reported has
                    // no overload to report.
                    if ctx.is_error_typed(left) || ctx.is_error_typed(right) {
                        return Type::Any;
                    }
                    let code = if matches!(op.as_str(), "&" | "|" | "^" | "<<" | ">>") {
                        "WS011"
                    } else {
                        "WS004"
                    };
                    ctx.emit(
                        code,
                        format!(
                            "no overload for '{op}' on {}, {}",
                            crate::analysis::types::type_str(&lt),
                            crate::analysis::types::type_str(&rt)
                        ),
                        range.clone(),
                    );
                    Type::Any
                }
            }
        }
        Expr::FieldAccess { obj, field, range } => {
            if let Some(t) = super::props::infer_environment_read(ctx, e) {
                return t;
            }
            if super::props::environment_without_property(ctx, e) {
                return Type::Any;
            }
            ctx.reserve_check(field, range);
            // A namespace that TRAVELED in privately (a pulled-in module's own
            // `import * as`) is not nameable from this file — reading through it
            // is a missing import here, reported like any unknown base rather
            // than silently resolving the leak (N11).
            if let Expr::Ident { name: ns_name, range: ns_range } = obj.as_ref()
                && ctx.namespace_hidden_here(ns_name, &ns_range.file)
            {
                ctx.emit(
                    "WS002",
                    format!(
                        "unknown identifier '{ns_name}': no namespace, variable, or \
                         value named '{ns_name}' is in scope (is an import missing?)"
                    ),
                    ns_range.clone(),
                );
                return Type::Any;
            }
            // `ns.member` on an `import * as ns` namespace: the namespace symbol
            // itself is typeless (`any`), so without this the whole reference
            // typed `any` and every use of it against a concrete type was a
            // spurious mismatch. Read the member's indexed type instead.
            // (`ns.f(args)` calls are handled in the Call arm, before this.)
            if let Some(ty) = namespace_member_type(ctx, obj, field) {
                return ty;
            }
            // A namespace member that is missing (`L.nope`) or is not a readable
            // value (`L.total`, where `total` is an `out`/`in`/buffer with no
            // value type, or a bare callable used as a value): error, matching
            // the plain/selective forms, instead of falling through to `any` and
            // lowering to `_Unsupported`.
            if let Expr::Ident { name: ns_name, range: ns_range } = obj.as_ref()
                && ctx.scope.lookup(ns_name).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                let present = ctx
                    .ns_members(ns_name, &ns_range.file)
                    .map(|m| m.contains_key(field.as_str()))
                    .unwrap_or(false);
                let msg = if present {
                    format!("'{field}' is not a readable value in namespace '{ns_name}'")
                } else {
                    format!("'{field}' not found in namespace '{ns_name}'")
                };
                ctx.emit("WS002", msg, range.clone());
                return Type::Any;
            }
            // `Enum.Variant` (unit-variant construction, e.g. `Shape.Empty`)
            // and an unknown variant on a known enum (`Shape.Nope`, WS060).
            // Checked directly against `ctx.enum_defs` rather than falling
            // into the generic `infer(ctx, obj)` below: `obj` here is a bare
            // enum TYPE name, not a value, and `ctx.enum_defs` is populated
            // by the `collect_enum_defs` pre-pass before any decl is
            // registered, so this resolves even when the `enum` itself is
            // declared later in the file: inferring `obj` as an ordinary
            // identifier would either piggyback on the scope-registered
            // symbol's type (order-dependent) or, before that registration
            // runs, misreport WS002 "unknown identifier". Positional/named
            // variant construction (`Shape.Circle(5.0)`) is a Call on this
            // same FieldAccess node used as a callee, handled by the Call
            // arm instead; this only covers a bare unit reference.
            //
            // Resolution (shadow-guard + registry + generic guard + WS060) is
            // shared with the two payload-construction sites via
            // `resolve_variant_for_construction`. A bare variant reference
            // (unit `Shape.Empty`, or a payload variant named without
            // constructing, e.g. `Shape.Circle` used for `.Discriminant`)
            // types as the enum regardless of payload shape; an unknown
            // variant is WS060 (recover as the enum type); a non-enum /
            // shadowed / generic name falls through to `infer(ctx, obj)`.
            if let Some(enum_name) = enum_head(ctx, obj) {
                match resolve_variant_for_construction(ctx, enum_name, field, range) {
                    VariantResolution::NotConstruction => {}
                    VariantResolution::UnknownVariant(_) => return Type::Any,
                    VariantResolution::Resolved(enum_ty, vdef, type_params) => {
                        return variant_reference_type(
                            ctx,
                            enum_name,
                            &vdef,
                            enum_ty,
                            &type_params,
                            expected.as_ref(),
                            range,
                        );
                    }
                }
            }
            let ot = infer(ctx, obj);
            if let Type::Component(class) = unwrap_ref(&ot) {
                return super::props::infer_component_read(ctx, &class, field, range);
            }
            // `<enum value>.Discriminant` (an enum-typed value, or a variant
            // path such as `Shape.Circle`, itself typed `Type::Enum` by the
            // block just above) always projects to its integer
            // discriminant. A non-enum target is WS066; recovers to
            // `Type::Int` so a chained use doesn't cascade a second mismatch
            // off an `Any`.
            if field == "Discriminant" {
                if matches!(ot, Type::Enum { .. }) {
                    return Type::Int;
                }
                ctx.emit(
                    "WS066",
                    format!(
                        "`.Discriminant` needs an enum value or variant, found `{}`",
                        crate::analysis::types::type_str(&ot)
                    ),
                    range.clone(),
                );
                return Type::Int;
            }
            if let Type::Ref(inner) = &ot {
                if field == "Value" || field == "prev" {
                    return inner.as_ref().clone();
                }
                if field == "VarRef" {
                    return ot.clone();
                }
            }
            // `cN.prev` where `cN` was auto-dereffed in exec context:
            // the obj's type is the inner T, not Ref(T). Look up the
            // declared var type directly. A record with a field of that name
            // reads the field.
            if (field == "Value" || field == "prev")
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && let Type::Ref(inner) = &sym.ty
                && !inner.fields().is_some_and(|fs| fs.iter().any(|(n, _)| n == field))
            {
                return inner.as_ref().clone();
            }
            // `x.Value` on a var/ref reads through to the inner type. A record
            // with its own `Value` field (a multi-output gate result such as
            // `a.pop()` -> `{ Value, IsEmpty }`) must project that field instead,
            // or `.Value` yields the whole record and every use of it mistypes.
            if (field == "value" || field == "Value") && !matches!(ot, Type::Record(_) | Type::Outputs(_)) {
                return unwrap_ref(&ot);
            }
            // `.exec` names the exec output of an event/exec-producing expression.
            // A data-carrying event (`GlobalCustomEvent(c) -> (n)`) types as a
            // record with its exec output FIRST, so `.exec` selects that field
            // explicitly and lets the event compose into `Union(...)`; on a bare
            // exec value it is the identity.
            if field == "exec"
                && (matches!(ot, Type::Exec)
                    || matches!(
                        &ot,
                        Type::Record(fs) | Type::Outputs(fs) if matches!(fs.first(), Some((_, Type::Exec)))
                    ))
            {
                return Type::Exec;
            }
            // `.exec` on a CALL names that call's own exec output, bound as an
            // `exec` value without joining the current chain, so the call runs
            // as a branch off the chain point the caller is standing on. A
            // result that already carries a field spelled `exec` (a chip
            // declaring one, or the completion field an `exec =` argument adds)
            // is projected below and keeps whatever type it declares.
            if field == "exec"
                && matches!(obj.as_ref(), Expr::Call { .. })
                && !matches!(&ot, Type::Record(fs) | Type::Outputs(fs) if fs.iter().any(|(n, _)| n == "exec"))
            {
                return Type::Exec;
            }
            // A field access on a record ARRAY or MAP is a COLUMN: `pts.x`
            // where `pts: P[]` is the `x` column (an array of the field's
            // type), and `m.x` on a record map maps keys to the field's type.
            // This types `pts.x[i]` (indexing a column) and `pts.x.sum()` (a
            // column method) as the field instead of `Any` — without it,
            // indexing a column fell to `_Unsupported` and any op on it was a
            // WS004 (Any). (R13.)
            if let Some(column) = record_column_type(&ot, field) {
                ctx.record_container_kind(obj, &ot);
                return column;
            }
            if let Type::Record(fields) | Type::Outputs(fields) = &ot {
                if let Some((_, t)) = fields.iter().find(|(k, _)| k == field) {
                    // A ref field of a record PARAMETER (`*T` on a record
                    // distributes onto its fields) is a place: it reads its
                    // target, exactly as reading a `var` does. A record VALUE
                    // (a let-bound literal, a map read, a call result) holds
                    // the ref itself, so the field stays a ref.
                    return if projection_is_place(ctx, e) {
                        unwrap_ref(t)
                    } else {
                        t.clone()
                    };
                }
                // One concise line naming the valid fields. The full,
                // syntax-COLOURED record type is rendered on hover
                // (analysis::hover) — the only place VS Code can show colour;
                // dumping the multi-line block into the diagnostic too just
                // repeats it verbatim under the hover (VS Code stacks the
                // hover-provider content and the diagnostic message together).
                let names: String = fields
                    .iter()
                    .map(|(n, _)| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                ctx.emit(
                    "WS010",
                    format!("no field `{field}` on record (has: {names})"),
                    range.clone(),
                );
                return Type::Any;
            }
            // A payload field named directly on an enum value. The slots are
            // keyed by variant, so the surface name reaches nothing; without
            // this the read fell to `any` and lowered to a placeholder.
            if let Type::Enum { name: en, .. } = &ot
                && let Some(edef) = ctx.enum_defs.of(&ot)
                && let Some(v) = edef
                    .variants
                    .iter()
                    .find(|v| match &v.payload {
                        crate::typecheck::enums::Payload::Named(fs) => {
                            fs.iter().any(|(n, _)| n == field)
                        }
                        _ => false,
                    })
                    .map(|v| v.name.clone())
            {
                ctx.emit(
                    "WS010",
                    format!(
                        "`{field}` is payload of variant `{v}`, not a field of enum `{en}`. Read it through a \
                         destructure (`if let {v} {{ {field} }} = <value> {{ ... }}`, or a `match` arm), or \
                         `unsafe <value>.{v}.{field}` to read the slot without testing the tag"
                    ),
                    range.clone(),
                );
                return Type::Any;
            }
            match (&ot, field.as_str()) {
                (t, f) if crate::catalog::value_fields::field_of(t, f).is_some() => Type::Float,
                // An array read yields the element plus a bounds flag, but it is
                // typed as the bare element (see IndexAccess), so by the time the
                // flag is projected - directly or through a `let` - the object is
                // the element type and this would fall through to Any. Lowering
                // already maps these names to the gate's bOutOfBounds port.
                (_, "OutOfBounds" | "bOutOfBounds") => Type::Bool,
                // A scalar has no fields, so `c.whatever` on an `int` is a typo.
                // The one legal exception is projecting a single-output call result by
                // its output name (`let f = Foo(); f.result`), which types as
                // the bare output; `single_output_alias` records exactly which
                // bindings those are and what name each accepts, so a genuine
                // projection passes and only a real typo is flagged. A binding
                // of unknown origin keeps the permissive `any`.
                (Type::Int | Type::Float | Type::Bool | Type::String, _) => {
                    let projectable = match obj.as_ref() {
                        Expr::Ident { name, .. } => match ctx.single_output_alias.get(name) {
                            // Unindexed origin, or the declared output name.
                            Some(None) => true,
                            Some(Some(out)) => out == field,
                            None => false,
                        },
                        // Any other shape (a param, an element, a call result
                        // used inline) has no recorded origin — stay silent
                        // rather than risk calling valid code a typo.
                        _ => true,
                    };
                    if !projectable {
                        ctx.emit(
                            "WS010",
                            format!(
                                "no field `{field}` on {}",
                                crate::analysis::types::type_str(&ot)
                            ),
                            range.clone(),
                        );
                    }
                    unwrap_ref(&ot)
                }
                // A component-typed value has a CLOSED set of component fields
                // (checked above: vector x/y/z, color r/g/b/a, rotator
                // pitch/yaw/roll, quat x/y/z/w). A field outside its own set is a typo or a
                // swizzle borrowed from another component type (`v.r`,
                // `color.x`, `rot.x`, `v.w`), and must not fall through to a
                // silent `any`. Lowering dispatches Split gates on the field
                // NAME alone, so `v.r` would feed the vector into a SplitColor:
                // a real gate with a wrong value and, until now, no diagnostic.
                // Flag it as a typo, exactly like a scalar field access.
                (Type::Vector | Type::Color | Type::Rotator | Type::Quat, _) => {
                    ctx.emit(
                        "WS010",
                        format!(
                            "no field `{field}` on {}",
                            crate::analysis::types::type_str(&ot)
                        ),
                        range.clone(),
                    );
                    Type::Any
                }
                // A call's one enum output, projected by its name (`f(c).r`
                // on `-> (r: Hue)`), is that output, as for a scalar.
                (Type::Enum { .. }, _)
                    if matches!(obj.as_ref(), Expr::Call { callee, .. }
                        if matches!(callee.as_ref(), Expr::Ident { name, range }
                            if ctx.calls_user_mod(name, &range.file)
                                && ctx
                                    .resolve_mod(name)
                                    .is_some_and(|d| d.outputs.len() == 1 && d.outputs[0].name == *field))) =>
                {
                    unwrap_ref(&ot)
                }
                _ => Type::Any,
            }
        }
        Expr::IndexAccess { obj, index, range } => {
            let ot = unwrap_ref(&infer(ctx, obj));
            ctx.record_container_kind(obj, &ot);
            record_if_branch_containers(ctx, obj);
            check_index_type(ctx, &ot, index);
            match &ot {
                Type::Array(inner) => {
                    if ctx.exec_mode() != ExecMode::Exec && !index_access_is_const(ctx, e) {
                        ctx.emit(
                            "WS007",
                            format!(
                                "array index read '{}[...]' outside an exec context",
                                target_name(obj).unwrap_or("<expr>".into())
                            ),
                            range.clone(),
                        );
                    }
                    inner.as_ref().clone()
                }
                Type::Map(_, v) => {
                    if ctx.exec_mode() != ExecMode::Exec && !index_access_is_const(ctx, e) {
                        ctx.emit(
                            "WS007",
                            format!(
                                "map index read '{}[...]' outside an exec context",
                                target_name(obj).unwrap_or("<expr>".into())
                            ),
                            range.clone(),
                        );
                    }
                    v.as_ref().clone()
                }
                // `s[i]` is the single Unicode character at `i`, so it yields a
                // string. The Substring gate behind it is pure, so this carries
                // no exec-context requirement the way a container read does.
                Type::String => Type::String,
                Type::Record(_) | Type::Outputs(_) | Type::Tuple(_) | Type::Enum { .. } => {
                    let name = target_name(obj);
                    let what = match &name {
                        Some(name) => format!("'{name}'"),
                        None => "this value".to_string(),
                    };
                    let base = name.unwrap_or_default();
                    // A tuple is a record whose fields are its positions.
                    let tuple = matches!(&ot, Type::Tuple(_))
                        || matches!(&ot, Type::Record(fields)
                            if !fields.is_empty() && fields.iter().all(|(n, _)| n.parse::<usize>().is_ok()));
                    let fix = match &ot {
                        _ if tuple => format!("read an element by its position (`{base}.0`)"),
                        Type::Record(fields) | Type::Outputs(fields) => match fields.first() {
                            Some((field, _)) => format!("read a field by its name (`{base}.{field}`)"),
                            None => "read a field by its name".to_string(),
                        },
                        _ =>"read it with `match`, `is` or `.Discriminant`".to_string(),
                    };
                    ctx.emit(
                        "WS044",
                        format!(
                            "{what} is {}, not an array, map or string, so it can't be indexed: {fix}",
                            crate::analysis::types::type_str(&ot),
                        ),
                        range.clone(),
                    );
                    Type::Any
                }
                _ => Type::Any,
            }
        }
        Expr::TuplePick { obj, index, range } => {
            let ot = infer(ctx, obj);
            let place = projection_is_place(ctx, e);
            let read = |t: &Type| if place { unwrap_ref(t) } else { t.clone() };
            match &ot {
                Type::Tuple(fields) => fields.get(*index).map(read).unwrap_or_else(|| {
                    ctx.emit(
                        "WS010",
                        format!("tuple index .{index} out of range"),
                        range.clone(),
                    );
                    Type::Any
                }),
                Type::Record(fields) | Type::Outputs(fields) => fields
                    .get(*index)
                    .map(|(_, t)| read(t))
                    .unwrap_or(Type::Any),
                _ => Type::Any,
            }
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range,
            ..
        } => {
            ctx.if_contexts
                .insert((range.file.clone(), range.start.offset), false);
            let ct = infer(ctx, cond);
            record_read_as_one_value(ctx, cond, &ct);
            let branch = enum_branch_hint(expected.as_ref());
            let narrowed = super::narrow::push_if(ctx, cond);
            let tt = infer_hinted(ctx, then_branch, branch.as_ref());
            super::narrow::pop(ctx, narrowed);
            let et = infer_hinted(ctx, else_branch, branch.as_ref());
            // A component reference (`zone`, `teleport`) or a prefab reference
            // can't flow through the Select gate an if-expr compiles to: it is
            // wired or rerouted, never a value. A var ref or a brick is a value
            // and may be selected. Flag whichever branch is such a reference.
            for (br, bty) in [(then_branch, &tt), (else_branch, &et)] {
                if matches!(bty, Type::Zone | Type::Teleport | Type::PrefabRef) {
                    ctx.emit(
                        "WS031",
                        format!(
                            "'{}' is a reference-only value and can't be used in an if-then-else: \
                             a Select routes a value, not a reference; wire or reroute it \
                             instead",
                            crate::analysis::types::type_str(bty),
                        ),
                        br.range().clone(),
                    );
                }
            }
            match widening_join(&tt, &et) {
                Some(j) => {
                    // Each branch gives a value of the joined type.
                    int_constants_in_slot(ctx, then_branch, &j);
                    int_constants_in_slot(ctx, else_branch, &j);
                    j
                }
                None => {
                    ctx.emit(
                        "WS003",
                        format!(
                            "if-then-else branch type mismatch: then is {}, else is {} (no common widening)",
                            crate::analysis::types::type_str(&tt),
                            crate::analysis::types::type_str(&et),
                        ),
                        range.clone(),
                    );
                    et
                }
            }
        }
        Expr::BlockExpr { stmts, value, .. } => {
            ctx.push_scope();
            for s in stmts {
                check_stmt(ctx, s);
            }
            let t = infer(ctx, value);
            ctx.pop_scope();
            t
        }
        Expr::MatchExpr { scrutinee, arms, range } => {
            let as_statement = std::mem::take(&mut ctx.statement_match);
            // A value `match` reads its value after the arms rejoin, so a block
            // arm, which has none, must leave through a `return`.
            if !as_statement && ctx.exec_mode() != ExecMode::Exec {
                // Outside an exec chain the arms are a pure Select, so a block
                // arm's statements, its `return` included, never run.
                if let Some(arm) = arms.iter().find(|a| matches!(a.body, MatchBody::Block(_))) {
                    ctx.emit(
                        "WS095",
                        "this `match` is used as a value outside an exec context, so this block \
                         arm never runs: its statements and its `return` need a handler's exec \
                         chain. Write the arm as a value (`A => expr`), or move the `match` into an \
                         `on` handler",
                        arm.range.clone(),
                    );
                }
            } else if !as_statement {
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body
                        && !super::stmt::block_returns(b)
                    {
                        ctx.emit(
                            "WS095",
                            "this `match` is used as a value, but this arm is a block, which has no \
                             value: a block arm of a value `match` must end in `return`. Write the arm \
                             as a value (`A => expr`), or make the `match` a statement whose arms \
                             assign a `var`",
                            arm.range.clone(),
                        );
                    }
                }
            }
            let scrut_ty = infer_scrutinee(ctx, scrutinee);
            let value_match = is_matchable_value(&scrut_ty);
            if !matches!(scrut_ty, Type::Enum { .. }) && !value_match {
                ctx.emit(
                    "WS066",
                    format!(
                        "`match` tests an enum, int, float, bool or string, but this is {}",
                        crate::analysis::types::type_str(&scrut_ty)
                    ),
                    range.clone(),
                );
                // Still walk each arm body so its own errors surface; there is
                // no type to check patterns against, so captures bind as `any`.
                for arm in arms {
                    ctx.push_scope();
                    check_match_pattern(ctx, &arm.pattern, &Type::Any, false);
                    if let Some(guard) = &arm.guard {
                        infer(ctx, guard);
                    }
                    infer_match_body(ctx, &arm.body);
                    ctx.pop_scope();
                }
                return Type::Any;
            }

            // A capture of a single value is a copy of it, never a place.
            let mutable = !value_match && crate::typecheck::stmt::scrutinee_is_mutable(ctx, scrutinee);
            let branch = enum_branch_hint(expected.as_ref());
            // `match x.getType()`: each arm sees `x` as the kind it names.
            let kind_test = matches!(&scrut_ty, Type::Enum { name, .. } if name == "Type");
            let mut tys: Vec<Type> = Vec::new();
            for arm in arms {
                ctx.push_scope();
                check_match_pattern(ctx, &arm.pattern, &scrut_ty, mutable);
                if value_match {
                    check_value_pattern_root(ctx, &arm.pattern, &scrut_ty);
                }
                // A bare `Int =>` parses as a binding and names the variant.
                let narrowed = match &arm.pattern {
                    Pattern::Variant { variant: member, sub: crate::ast::VariantPattern::Unit, .. }
                    | Pattern::Binding { name: member, .. }
                        if kind_test =>
                    {
                        super::narrow::push_arm(ctx, scrutinee, member)
                    }
                    _ => false,
                };
                if let Some(guard) = &arm.guard {
                    let t = infer(ctx, guard);
                    record_read_as_one_value(ctx, guard, &t);
                }
                if let MatchBody::Expr(x) = &arm.body {
                    tys.push(infer_hinted(ctx, x, branch.as_ref()));
                } else if let Some(t) = infer_match_body(ctx, &arm.body) {
                    tys.push(t);
                }
                super::narrow::pop(ctx, narrowed);
                ctx.pop_scope();
            }

            let result = if tys.is_empty() {
                Type::Any
            } else {
                let mut acc = tys[0].clone();
                let mut joined = true;
                for t in &tys[1..] {
                    match widening_join(&acc, t) {
                        Some(j) => acc = j,
                        None => {
                            joined = false;
                            break;
                        }
                    }
                }
                if joined {
                    for arm in arms {
                        if let MatchBody::Expr(x) = &arm.body {
                            int_constants_in_slot(ctx, x, &acc);
                        }
                    }
                    acc
                } else {
                    ctx.emit(
                        "WS003",
                        format!(
                            "match arm type mismatch: arms produce {} (no common widening)",
                            tys.iter()
                                .map(|t| crate::analysis::types::type_str(t))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        range.clone(),
                    );
                    tys[0].clone()
                }
            };

            let arm_patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
            let guarded: Vec<bool> = arms.iter().map(|a| a.guard.is_some()).collect();
            let usefulness =
                crate::typecheck::patterns::analyze(ctx.enums(), &scrut_ty, &arm_patterns, &guarded);
            for witness in &usefulness.missing {
                let message = match &witness.0 {
                    Pattern::Wildcard(_) => format!(
                        "non-exhaustive match: a `match` on {} values needs a last `_` arm \
                         (or a name) for the values no other arm matches",
                        crate::analysis::types::type_str(&scrut_ty)
                    ),
                    w => format!("non-exhaustive match: `{}` is not covered", render_pattern(w)),
                };
                ctx.emit("WS054", message, range.clone());
            }
            for &idx in &usefulness.unreachable_arms {
                ctx.warn(
                    "WS061",
                    "unreachable match arm - an earlier arm already covers every value it matches"
                        .to_string(),
                    arms[idx].range.clone(),
                );
            }
            result
        }
        Expr::RecordLit { fields, .. } => infer_record_lit(ctx, fields, None),
        Expr::Array { elements, .. } => {
            // Type each element so it lands in the type map, and take the
            // array's element type as the WIDENING JOIN of them all, not
            // element 0's. A spread contributes its source array's element
            // type, a plain item its value type. Whether the elements must be
            // constant literals is enforced at the declaration site (top
            // level), not here, since the same literal is valid with runtime
            // elements in an exec-context assignment.
            //
            // Element 0's type is the wrong answer in the direction that stays
            // silent: it only asks the rest to coerce INTO it, so `[1, 2.5]`
            // reads as `int[]` and stores the float in an int-variant gate,
            // while the equally mixed `["a", 1]` reads as `string[]`.
            //
            // An `any[]` takes elements of every kind, each keeping its own.
            if matches!(expected.as_ref().map(unwrap_ref), Some(Type::Array(inner)) if matches!(*inner, Type::Opaque)) {
                for el in elements {
                    infer(ctx, el.expr());
                    if let ArrayElem::Spread(v) = el {
                        ctx.record_container_expr(v);
                    }
                }
                return Type::Array(Box::new(Type::Opaque));
            }
            let mut elem: Option<Type> = None;
            for el in elements {
                let t = unwrap_ref(&infer(ctx, el.expr()));
                if let ArrayElem::Spread(v) = el {
                    // An exec-context assignment lowers a spread as an `append`
                    // whose source is this element.
                    ctx.record_container_expr(v);
                }
                let et = match el {
                    ArrayElem::Spread(_) => match t {
                        Type::Array(inner) => *inner,
                        other => other,
                    },
                    // An element is one value: a gate read's first output.
                    ArrayElem::Item(_) => first_output(&t),
                };
                elem = Some(match elem {
                    None => et,
                    Some(acc) if matches!(acc, Type::Any) || matches!(et, Type::Any) => {
                        if matches!(acc, Type::Any) { et } else { acc }
                    }
                    Some(acc) => match crate::types::coerce::widening_join_all([acc.clone(), et.clone()]) {
                        Some(joined) => joined,
                        None => {
                            // A literal's elements share one backing
                            // array-variant gate, so they have to have a
                            // common type. (Top-level `var` initializers are
                            // checked against the DECLARED element type in
                            // `check_top_level_array_init`; this covers
                            // exec-context assignments, which reach `infer`
                            // plus a whole-array coerce that only inspects
                            // element 0.)
                            ctx.emit(
                                "WS003",
                                format!(
                                    "array element: expected {}, got {}",
                                    crate::analysis::types::type_str(&acc),
                                    crate::analysis::types::type_str(&et),
                                ),
                                el.expr().range().clone(),
                            );
                            acc
                        }
                    },
                });
            }
            if let Some(elem) = &elem {
                for el in elements {
                    if let ArrayElem::Item(v) = el {
                        int_constants_in_slot(ctx, v, elem);
                    }
                }
            }
            Type::Array(Box::new(elem.unwrap_or(Type::Any)))
        }
        // Reached only when a map literal is used somewhere OTHER than a
        // `var` initializer or an assignment RHS to a map var — those
        // valid slots call `check_map_literal` directly (see `check_decl` /
        // `check_stmt`) and never reach this arm. Still infer each entry (so
        // key/value expressions get typed and any inner errors surface, e.g. a
        // key referencing an unknown identifier) and infer the literal's real
        // `Map<K, V>` shape from the first entry — mirroring the array
        // literal's element-type inference above — but the position itself is
        // always an error: a map literal must initialize or assign a Map.
        Expr::MapLit { entries, range } => {
            let mut kt = Type::Any;
            let mut vt = Type::Any;
            for (i, entry) in entries.iter().enumerate() {
                let k = infer(ctx, &entry.key);
                let v = infer(ctx, &entry.value);
                if i == 0 {
                    kt = k;
                    vt = v;
                }
            }
            ctx.emit(
                "WS026",
                "a map literal must initialize or assign a Map variable",
                range.clone(),
            );
            Type::Map(Box::new(kt), Box::new(vt))
        }
        // Enum payload construction, braced-named form: `Enum.Variant { f: v, ... }`.
        // The parser only ever produces this node when `path` is a
        // `FieldAccess` with a REAL record body (see `parser/expr.rs`'s
        // `looks_like_variant_ctor_body`), but that alone doesn't guarantee
        // `path` denotes a known, unshadowed enum variant - a shadowed enum
        // name, or a longer `a.b.c { .. }` chain, still reaches here. Resolution
        // is shared with the positional `Call` arm via
        // `resolve_variant_for_construction`; when `path` is NOT a
        // named-payload variant construction the fallback emits `WS065` rather
        // than silently typing as `Any` (a braced construction is only valid
        // for an enum variant with named fields).
        Expr::VariantCtor { path, fields, range } => {
            if let Expr::FieldAccess {
                obj,
                field: variant,
                range: fa_range,
            } = path.as_ref()
                && let Some(enum_name) = enum_head(ctx, obj)
            {
                match resolve_variant_for_construction(ctx, enum_name, variant, fa_range) {
                    VariantResolution::NotConstruction => {}
                    VariantResolution::UnknownVariant(enum_ty) => {
                        infer_record_field_values(ctx, fields);
                        return enum_ty;
                    }
                    VariantResolution::Resolved(enum_ty, vdef, type_params) => {
                        let annotation = expected;
                        let bound = expected_enum_args(annotation.as_ref(), enum_name, &type_params);
                        let crate::typecheck::enums::Payload::Named(named_types) = &vdef.payload
                        else {
                            ctx.emit(
                                "WS065",
                                ws065_named_form_wrong(enum_name, variant, &vdef.payload),
                                fa_range.clone(),
                            );
                            infer_record_field_values(ctx, fields);
                            return enum_ty;
                        };
                        // Each declared field's expected type, with the enum's
                        // type parameters kept as `Type::Param` so a generic
                        // payload (`W { inner: T }`) resolves without WS002 and
                        // its arg feeds the type-parameter solve below.
                        let named_ptypes: Vec<(String, Type)> = named_types
                            .iter()
                            .map(|(n, ty)| {
                                (n.clone(), instantiated(resolve_payload_param_type(ctx, ty, &type_params), &bound))
                            })
                            .collect();
                        let mut seen: Vec<&str> = Vec::new();
                        // Inferred value type per provided field, keyed by name -
                        // fed to `infer_enum_args` for a generic enum.
                        let mut field_arg_types: Vec<(String, Type)> = Vec::new();
                        for f in fields {
                            match f {
                                RecordLitField::Named { name, value, range: f_range } => {
                                    seen.push(name.as_str());
                                    match named_ptypes.iter().find(|(n, _)| n == name) {
                                        Some((_, fty)) => {
                                            let at = check_or_infer_payload_field(ctx, value, fty);
                                            field_arg_types.push((name.clone(), at));
                                        }
                                        None => {
                                            infer(ctx, value);
                                            ctx.emit(
                                                "WS010",
                                                format!("variant `{variant}` has no field `{name}`"),
                                                f_range.clone(),
                                            );
                                        }
                                    }
                                }
                                RecordLitField::Shorthand { name, range: f_range } => {
                                    seen.push(name.as_str());
                                    // `{ h }` is shorthand for `{ h: h }`: check
                                    // the value the same way a named field's is,
                                    // by synthesizing the identifier the
                                    // shorthand stands for. Routing through
                                    // `check`/`infer` gives it the ordinary
                                    // `Expr::Ident` treatment - a scalar var's
                                    // `*T` auto-derefs to `T` (so `{ w, h }` from
                                    // `var h: float` type-checks, not `*float` vs
                                    // `float`), and an undefined name reports
                                    // WS002 - instead of the raw symbol type a
                                    // direct scope read would give.
                                    let ident = Expr::Ident {
                                        name: name.clone(),
                                        range: f_range.clone(),
                                    };
                                    match named_ptypes.iter().find(|(n, _)| n == name) {
                                        Some((_, fty)) => {
                                            let at = check_or_infer_payload_field(ctx, &ident, fty);
                                            field_arg_types.push((name.clone(), at));
                                        }
                                        None => {
                                            infer(ctx, &ident);
                                            ctx.emit(
                                                "WS010",
                                                format!("variant `{variant}` has no field `{name}`"),
                                                f_range.clone(),
                                            );
                                        }
                                    }
                                }
                                // A variant's payload is always a short, fully-named
                                // field list at the call site in practice - spreading
                                // another record into it has no defined semantics
                                // here (unlike a plain `RecordLit`, there is no
                                // "extra structural fields" concept for a payload).
                                RecordLitField::Spread { value, range: f_range } => {
                                    infer(ctx, value);
                                    ctx.emit(
                                        "WS010",
                                        format!(
                                            "variant `{variant}` construction does not support \
                                             `...` spread"
                                        ),
                                        f_range.clone(),
                                    );
                                }
                            }
                        }
                        for (name, _) in &named_ptypes {
                            if !seen.contains(&name.as_str()) {
                                ctx.emit(
                                    "WS010",
                                    format!("variant `{variant}` is missing field `{name}`"),
                                    range.clone(),
                                );
                            }
                        }
                        if type_params.is_empty() {
                            return enum_ty;
                        }
                        if let Some(args) = bound {
                            return Type::Enum {
                                name: enum_name.to_string(),
                                args: args.into_iter().map(|(_, t)| t).collect(),
                            };
                        }
                        let param_types: Vec<Type> =
                            named_ptypes.iter().map(|(_, ty)| ty.clone()).collect();
                        let arg_types: Vec<Type> = named_ptypes
                            .iter()
                            .map(|(n, _)| {
                                field_arg_types
                                    .iter()
                                    .find(|(fname, _)| fname == n)
                                    .map(|(_, t)| unwrap_ref(t))
                                    .unwrap_or(Type::Any)
                            })
                            .collect();
                        let args = infer_enum_args(
                            ctx,
                            enum_name,
                            &type_params,
                            &param_types,
                            &arg_types,
                            annotation.as_ref(),
                            range,
                        );
                        return Type::Enum { name: enum_name.to_string(), args };
                    }
                }
            }
            // Fallback: `path` is not a named-payload enum-variant construction
            // (a non-`FieldAccess`/`Ident` path, a shadowed enum name, an
            // unknown/generic enum, or an `a.b.c { .. }` chain). A braced
            // `{ .. }` construction is only valid for such a variant, so this is
            // an error - emit WS065 rather than returning bare `Any` with no
            // diagnostic. `path` + every field value are still inferred so
            // nothing goes untyped (and any nested error surfaces).
            infer(ctx, path);
            infer_record_field_values(ctx, fields);
            ctx.emit(
                "WS065",
                "braced `{ .. }` construction is only valid for an enum variant with \
                 named fields"
                    .to_string(),
                range.clone(),
            );
            Type::Any
        }
        Expr::Call {
            callee,
            args,
            type_args,
            range: call_range,
        } => {
            // Resolve the target call spec (if any) so enum-typed config args
            // can be skipped below — a bare member name (`function = Bounce`)
            // is not a variable and must not be inferred as one.
            let arg_spec: Option<&'static crate::catalog::calls::CallSpec> = match callee.as_ref() {
                Expr::Ident { name, range } => find_call(name).filter(|_| !ctx.calls_user_mod(name, &range.file)),
                Expr::FieldAccess { field, .. } => {
                    find_call(field).filter(|c| c.receiver.is_some())
                }
                _ => None,
            };
            // A receiver method call normally binds the object as param 0, so
            // positional args in `args` start at param index 1. A named-target
            // receiver (`entity.SendCustomEvent(…)`) binds the object to a named
            // param instead, so its positional args still start at index 0.
            let pos_base = usize::from(
                matches!(callee.as_ref(), Expr::FieldAccess { .. })
                    && arg_spec
                        .is_some_and(|c| c.receiver.is_some() && c.receiver_target_param().is_none()),
            );
            // Side-effect: typecheck every arg, except enum-typed config args
            // and bare enum member names bound to a wire-enum param (neither is
            // a variable). `positional_arg_types` mirrors the positional args
            // in order — used below to infer a generic user mod/chip call's
            // type params from its arguments (builtin `find_call`s ignore it).
            let mut pos = 0usize;
            let mut positional_arg_types: Vec<Type> = Vec::new();
            let is_enum_name_arg = |c: &crate::catalog::calls::CallSpec,
                                    p: &crate::catalog::calls::CallParam,
                                    value: &Expr| {
                call_param_config_enum(c, p).is_some()
                    || call_param_wire_enum(c, p).is_some_and(|et| is_bare_enum_member(et, value))
            };
            for a in args {
                let is_config_enum = arg_spec.is_some_and(|c| match a {
                    CallArg::Named { name, value, .. } => match c.params.iter().find(|p| p.name == name) {
                        Some(p) => is_enum_name_arg(c, p, value),
                        // Not a declared param: a data-driven config attribute.
                        // Skip inference when it names an enum config field (the
                        // value is a bare member name, not a variable).
                        None => crate::catalog::config_field_enum_type(c.gate_class, name).is_some(),
                    },
                    CallArg::Positional(v) => c
                        .params
                        .get(pos_base + pos)
                        .is_some_and(|p| is_enum_name_arg(c, p, v)),
                    CallArg::Spread(_) => false,
                });
                if matches!(a, CallArg::Positional(_)) {
                    pos += 1;
                }
                if is_config_enum {
                    continue;
                }
                match a {
                    CallArg::Positional(v) => {
                        let hint = branch_arg_hint(ctx, callee, pos - 1, v);
                        let t = infer_hinted(ctx, v, hint.as_ref());
                        positional_arg_types.push(t);
                    }
                    CallArg::Named { value, .. } => {
                        infer(ctx, value);
                    }
                    CallArg::Spread(v) => {
                        infer(ctx, v);
                    }
                }
            }
            // `<enum value>.ToInt()` is an exact alias for `.Discriminant`: it
            // projects an enum value (or a variant path such as `Shape.Circle`)
            // to its integer discriminant. It takes no arguments. Resolved here,
            // ahead of the receiver-method and enum-construction arms below, so
            // it types identically to `.Discriminant` (see the `FieldAccess` /
            // `Discriminant` arm) - INCLUDING the WS066 on a non-enum receiver,
            // rather than letting `.ToInt()` on a non-enum decay to a silent
            // `any` through the permissive unknown-method fallback below.
            if let Expr::FieldAccess {
                obj, field, range, ..
            } = callee.as_ref()
                && field == "ToInt"
                && args.is_empty()
            {
                let recv = unwrap_ref(&infer(ctx, obj));
                if matches!(recv, Type::Enum { .. }) {
                    return Type::Int;
                }
                ctx.emit(
                    "WS066",
                    format!(
                        "`.ToInt()` needs an enum value or variant, found `{}`",
                        crate::analysis::types::type_str(&recv)
                    ),
                    range.clone(),
                );
                return Type::Int;
            }
            // Resolve callee if it's a plain identifier. A user mod or chip
            // named like a builtin is the one called when the calling file can
            // name it, as lowering calls it.
            if let Expr::Ident { name, range } = callee.as_ref() {
                if let Some(c) = find_call(name).filter(|_| !ctx.calls_user_mod(name, &range.file)) {
                    // Explicit type arguments on a builtin are ignored: a
                    // builtin's result type is derived from its arguments (the
                    // masked-generic engine), not pinned by an explicit `<...>`.
                    // Warn rather than silently dropping them.
                    if !type_args.is_empty() {
                        ctx.diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS037".into(),
                            message: format!(
                                "explicit type arguments are ignored on builtin `{name}`, since its \
                                 result type is derived from the arguments"
                            ),
                            range: range.clone(),
                        });
                    }
                    if c.exec && ctx.exec_mode() != ExecMode::Exec {
                        let has_exec_arg = args
                            .iter()
                            .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                        if !has_exec_arg {
                            ctx.emit(
                                "WS007",
                                format!("exec call '{name}' outside an exec context (pass exec = ... to override)"),
                                range.clone(),
                            );
                        }
                    }
                    // Change/Edge detectors watch a single wire VALUE. A reference
                    // or container arg (a `*T`/zone/teleport ref, or a map/array)
                    // has no value to watch — the detector observes the reference,
                    // which never "changes" — so `Change(m)` / `Changed(zone)`
                    // type-checked as `any` and lowered to an invalid, dead gate.
                    if c.gate_class == crate::ir::gate_class::CONTAINER_CHANGE_DETECTOR {
                        // The container detector is the one that DOES watch a
                        // container, and only a container: its input is the
                        // array or map's reference.
                        if let Some(CallArg::Positional(arg)) = args.first() {
                            let at = unwrap_ref(&infer(ctx, arg));
                            if !matches!(at, Type::Map(_, _) | Type::Array(_) | Type::Any) {
                                ctx.emit(
                                    "WS003",
                                    format!(
                                        "`{name}` watches an array or map, got {}",
                                        crate::analysis::types::type_str(&at)
                                    ),
                                    arg.range().clone(),
                                );
                            }
                        }
                    } else if (c.gate_class.contains("ChangeDetector")
                        || c.gate_class.contains("EdgeDetector"))
                        && let Some(CallArg::Positional(arg)) = args.first()
                    {
                        let at = infer(ctx, arg);
                        let container = matches!(unwrap_ref(&at), Type::Map(_, _) | Type::Array(_));
                        if is_reference_type(&at) || container {
                            ctx.emit(
                                "WS059",
                                format!(
                                    "`{name}` can't watch a {}: a change/edge detector \
                                     observes a single wire value, and a reference or container \
                                     (map, array, `*T`, zone, teleport) has none{}",
                                    crate::analysis::types::type_str(&unwrap_ref(&at)),
                                    if container {
                                        "; `ContainerChange(...)` fires when its contents change"
                                    } else {
                                        ""
                                    }
                                ),
                                range.clone(),
                            );
                        }
                    }
                    // Random rides the PrimMath variant like the math operators:
                    // its min/max may be a vector/rotator/quat/color (a
                    // per-component random on the same gate), and the result then
                    // matches that type rather than the int-typed CallSpec.
                    if name == "Random" {
                        let arg_tys: Vec<Type> = args
                            .iter()
                            .filter_map(|a| match a {
                                CallArg::Positional(e) => {
                                    Some(unwrap_ref(&infer(ctx, e)))
                                }
                                _ => None,
                            })
                            .collect();
                        if let Some(t) = arg_tys.into_iter().find(|t| {
                            matches!(t, Type::Vector | Type::Color | Type::Rotator | Type::Quat)
                        }) {
                            return t;
                        }
                    }
                    // `EnumToInt(value)` REQUIRES an enum argument (there is
                    // no "any enum" `Type`, so the bare `Type::Any` gate port
                    // can't express this - enforce it here, keyed on the gate).
                    // A non-enum (`EnumToInt(5)`, `EnumToInt("x")`) is a
                    // WS003 argument-type error, matching `check_args`'s own
                    // per-arg diagnostic. An already-`any` argument (an upstream
                    // error) is left alone to avoid cascading. The result is
                    // always `int`.
                    if name == "EnumToInt" {
                        match positional_arg_types.first() {
                            Some(t) => {
                                // A gate read gives its first output.
                                let ut = first_output(&unwrap_ref(t));
                                if !matches!(ut, Type::Enum { .. } | Type::Any) {
                                    ctx.emit(
                                        "WS003",
                                        format!(
                                            "argument 'value': expected an enum, got {}",
                                            crate::analysis::types::type_str(&ut)
                                        ),
                                        range.clone(),
                                    );
                                }
                            }
                            None => ctx.emit(
                                "WS011",
                                "'EnumToInt' requires 1 arg, got 0".to_string(),
                                range.clone(),
                            ),
                        }
                        return Type::Int;
                    }
                    // `IntToEnum(value, wrap?)` takes an `int` (+ optional
                    // `bool`) - validated by the ordinary `check_args` against
                    // the CallSpec params - but its RESULT is an enum whose
                    // concrete type is pinned by the use site's expected type,
                    // exactly like `Enum.FromInt`/`null`. With no enum-typed
                    // expectation there is no way to know which enum the integer
                    // names, so that is a WS063 (mirrors `FromInt`'s
                    // type-inference failure), recovering as `Any`.
                    if name == "IntToEnum" {
                        check_args(ctx, &sig_of_callspec(c), args, 0, true, true, range);
                        match expected.as_ref().map(unwrap_ref) {
                            Some(Type::Enum { name: en, args: en_args }) => {
                                return Type::Enum { name: en, args: en_args };
                            }
                            _ => {
                                ctx.emit(
                                    "WS063",
                                    "cannot infer which enum `IntToEnum` produces - annotate \
                                     the target (`: SomeEnum`) so the integer's enum type is known"
                                        .to_string(),
                                    range.clone(),
                                );
                                return Type::Any;
                            }
                        }
                    }
                    check_args(ctx, &sig_of_callspec(c), args, 0, true, true, range);
                    if let Some(t) = integer_math_result(ctx, name, args) {
                        return t;
                    }
                    if !c.outputs.is_empty() {
                        let t = output_record_type(ctx, c, args, range);
                        return variant_math_result(ctx, c, args, t);
                    }
                    if c.exec {
                        // `Never`, not `Any`: a void exec builtin has no
                        // result, so using its "result" as a value must be a
                        // type mismatch rather than silently accepted. `Any`
                        // let `v = PrintToConsole("x")` type-check clean and
                        // then wire the gate's EXEC pin into a data port. The
                        // array/map catalog states this rule at
                        // `catalog/arrays.rs:198`; three call sites here did
                        // not follow it.
                        return Type::Never;
                    }
                    return c.params.first().map(|p| p.ty.clone()).unwrap_or(Type::Any);
                }
                // An event called as an expression (`RoundStart()`,
                // `Clock(interval = 2.0)`, `CharacterSpawned()`) emits the event
                // gate: a data-less event yields its exec signal (so it composes,
                // `Union(RoundStart(), other)`); a data-carrying event yields a
                // record with the exec FIRST (a bare call auto-unwraps to exec)
                // plus each data output (`CharacterSpawned().character`). The
                // `on RoundStart() { … }` trigger form is unaffected — that path
                // never reaches this call resolution.
                if let Some(evt) = crate::catalog::events::find_event(name) {
                    if evt.data.is_empty() {
                        return Type::Exec;
                    }
                    let mut fields = vec![(evt.exec_out.to_string(), Type::Exec)];
                    for d in &evt.data {
                        fields.push((d.name.to_string(), d.ty.clone()));
                    }
                    return Type::call_result(fields);
                }
                let Some(sym) = ctx.scope.lookup(name).cloned() else {
                    // Bare positional enum-variant construction (`Some(42)`
                    // for `Option.Some(42)`, `Err(2)` for `Result.Err(2)`, ...):
                    // `name` is not a scope symbol, so try resolving it as a
                    // unique variant of a registered enum before falling to
                    // the unknown-identifier diagnostics below. Reuses the
                    // SAME construction the qualified `Enum.Variant(args)`
                    // call site below uses, keyed off the RESOLVED enum name
                    // rather than any surface qualification, so `Some(42)`
                    // and `Option.Some(42)` check (and later lower)
                    // identically.
                    if let Some(enum_name) = resolve_bare_variant_enum(ctx, name)
                        && let Some(ty) = try_construct_variant_positional(
                            ctx,
                            &enum_name,
                            name,
                            args,
                            &positional_arg_types,
                            expected.as_ref(),
                            range,
                            call_range,
                        )
                    {
                        return ty;
                    }
                    // A statement-form gate builtin (`SetArrayElement`,
                    // `SetVariable`, `IncrementVariable`) is real — completion
                    // offers it — but it desugars to an assignment only in
                    // statement position with the right arg count. Anything else
                    // (used as a value, or still being typed with too few args)
                    // lands here; describe the builtin rather than reporting a
                    // bare, misleading "unknown identifier".
                    if let Some(hint) =
                        crate::catalog::gate_builtins::statement_usage_hint(name)
                    {
                        ctx.emit("WS002", hint.to_string(), range.clone());
                        return Type::Any;
                    }
                    ctx.emit(
                        "WS002",
                        format!("unknown identifier '{name}'"),
                        range.clone(),
                    );
                    return Type::Any;
                };
                if matches!(sym.kind, SymbolKind::Fn | SymbolKind::Chip)
                    && !ctx.imports.user_mod_answers(&sym.decl_range, &range.file)
                {
                    if let Some(msg) = ctx.unnamed_mod_call(name, &sym.decl_range, &range.file, false) {
                        ctx.emit("WS002", msg, range.clone());
                    }
                    return Type::Any;
                }
                let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                let has_exec_arg = args
                    .iter()
                    .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                let positional_count = args
                    .iter()
                    .filter(|a| matches!(a, CallArg::Positional(_)))
                    .count();
                return type_user_symbol_call(
                    ctx,
                    name,
                    &sym,
                    &positional_arg_types,
                    args,
                    type_args,
                    positional_count,
                    has_spread,
                    has_exec_arg,
                    call_range,
                    range,
                );
            }
            // `Enum.FromInt(n)` builds a value of enum `Enum` whose
            // discriminant is the int `n`, every payload slot defaulted to its
            // zero value - a tag-only constructor (see
            // `docs/src/enums.md`). Recognized only when `Enum` is a
            // known, unshadowed enum type AND has no variant literally named
            // `FromInt` (a real variant of that name still wins, constructed by
            // the block below). Placed BEFORE that block because
            // `resolve_variant_for_construction` would otherwise emit WS060 for
            // the non-variant name `FromInt`. Takes exactly one int argument;
            // result types as the enum.
            // The Remove Component gate takes a brick and a constant component
            // type, and no gate gives a component reference's brick.
            if let Expr::FieldAccess { obj, field, range: fa_range } = callee.as_ref()
                && field == "Remove"
                && find_call(field).is_none()
                && let Type::Component(class) = unwrap_ref(&infer(ctx, obj))
            {
                let name = crate::catalog::component_types()
                    .iter()
                    .find(|o| o.class == class)
                    .map_or("<type>", |o| o.name.as_str());
                ctx.emit(
                    "WS110",
                    format!(
                        "a component reference has no `Remove()`: the game removes a component by its \
                         brick and type, so call `brick.RemoveComponent($Component/{name})` on the brick \
                         that holds it"
                    ),
                    fa_range.clone(),
                );
                return Type::Any;
            }
            if let Expr::FieldAccess { obj, field, range: fa_range } = callee.as_ref()
                && field == "FromInt"
                && let Some(enum_name) = enum_head(ctx, obj)
                && !matches!(
                    ctx.scope.lookup(enum_name).map(|s| s.kind),
                    Some(k) if k != SymbolKind::Type
                )
                && let Some(type_params) = ctx
                    .enum_defs
                    .expression_path(enum_name)
                    .filter(|def| !def.variants.iter().any(|v| v.name == "FromInt"))
                    .map(|def| def.type_params.clone())
            {
                let sig = CallSignature {
                    name: "FromInt".to_string(),
                    params: vec![Param {
                        name: "value".to_string(),
                        ty: Type::Int,
                        optional: false,
                        kind: ParamKind::Wire,
                    }],
                    config_gate: None,
                };
                check_args(ctx, &sig, args, 0, true, true, fa_range);
                // A generic enum's `FromInt` can't infer its type arguments from
                // a tag alone; take them from an enum-typed expectation when
                // present, else leave the `Any` recovery placeholder
                // (non-generic enums have none). The payloads default either way,
                // so the arguments only matter to a later generic annotation's
                // agreement.
                let args_ty = match expected.as_ref() {
                    Some(Type::Enum { name: en, args }) if en == enum_name => args.clone(),
                    _ => vec![Type::Any; type_params.len()],
                };
                return Type::Enum { name: enum_name.to_string(), args: args_ty };
            }
            // Enum payload construction, positional form: `Enum.Variant(args)`.
            // Resolution + construction is shared with the other construction
            // sites (the qualified bare-unit `FieldAccess` arm above, and the
            // bare `Some(42)`-style `Call` site in the `Expr::Ident` callee
            // branch above) via `try_construct_variant_positional`. `callee`
            // here is `Enum.Variant` used as a call target, not read as a
            // value, so a resolved construction is handled directly rather
            // than falling into the ordinary callee-resolution arms below
            // (which would misreport it as an unknown call); a non-enum /
            // shadowed name falls through to those arms unchanged.
            if let Expr::FieldAccess {
                obj,
                field: variant,
                range: fa_range,
            } = callee.as_ref()
                && let Some(enum_name) = enum_head(ctx, obj)
                && let Some(ty) = try_construct_variant_positional(
                    ctx,
                    enum_name,
                    variant,
                    args,
                    &positional_arg_types,
                    expected.as_ref(),
                    fa_range,
                    call_range,
                )
            {
                return ty;
            }
            // Namespace call: ns.foo(args)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Expr::Ident { name: ns_name, range: ns_range } = obj.as_ref()
                && ctx.namespace_visible(ns_name, &ns_range.file)
            {
                let member = ctx
                    .ns_members(ns_name, &ns_range.file)
                    .and_then(|ns_map| ns_map.get(field.as_str()))
                    .cloned();
                // A `mod`/`chip`/`fn` member is checked as a flat call is:
                // defaults, named args, and type params inferred from the
                // arguments.
                if let Some(info) = &member
                    && let Some((sig, decl_range)) = &info.signature
                {
                    let sym = SymbolInfo {
                        kind: info.kind,
                        name: field.clone(),
                        ty: Type::Any,
                        decl_range: decl_range.clone(),
                        signature: Some(sig.clone()),
                        event_data: None,
                    };
                    let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                    let has_exec_arg = args
                        .iter()
                        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                    let positional_count = args
                        .iter()
                        .filter(|a| matches!(a, CallArg::Positional(_)))
                        .count();
                    return type_user_symbol_call(
                        ctx,
                        field,
                        &sym,
                        &positional_arg_types,
                        args,
                        type_args,
                        positional_count,
                        has_spread,
                        has_exec_arg,
                        call_range,
                        fa_range,
                    );
                }
                let ns_lookup = member.map(|info| (info.kind, info.outputs.clone(), info.params.clone()));
                match ns_lookup {
                    Some((kind, outputs, params)) => {
                        // Route the call through the shared arg checker —
                        // skipping it risks a silent miscompile on wrong type
                        // or arity. Only `Fn`/`Chip` members are callable;
                        // a `TypeAlias` (etc.) member reached via call syntax
                        // has nothing to check here. A generic member's
                        // `params` may still carry `Type::Param` — there's no
                        // call-site `subst` at this call form to resolve
                        // them against, so `check_args`'s own
                        // `type_has_param` guard leaves those params
                        // unchecked (documented limitation; non-generic
                        // members are fully checked).
                        if matches!(kind, SymbolKind::Fn | SymbolKind::Chip) {
                            let sig = CallSignature {
                                name: field.clone(),
                                params: params
                                    .iter()
                                    .map(|p| Param {
                                        name: p.name.clone(),
                                        ty: p.ty.clone(),
                                        optional: false,
                                        kind: if p.is_const {
                                            ParamKind::Const
                                        } else {
                                            ParamKind::Wire
                                        },
                                    })
                                    .collect(),
                                config_gate: None,
                            };
                            // Named args are dropped at lowering for user
                            // Fn/Chip calls, so validate arity on POSITIONAL
                            // count only, matching the local call contract. A
                            // named-only call (`L.sub(b=2, a=10)`) otherwise
                            // passed arity here and then silently dropped every
                            // arg into `_Unsupported` pins.
                            let positional_count = args
                                .iter()
                                .filter(|a| matches!(a, CallArg::Positional(_)))
                                .count();
                            let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                            if !has_spread && positional_count != params.len() {
                                ctx.emit(
                                    "WS022",
                                    format!(
                                        "`{field}` expects {} argument{} but {} {} given",
                                        params.len(),
                                        if params.len() == 1 { "" } else { "s" },
                                        positional_count,
                                        if positional_count == 1 { "was" } else { "were" },
                                    ),
                                    fa_range.clone(),
                                );
                            }
                            check_args(ctx, &sig, args, 0, false, true, fa_range);
                            args_fill_float_params(ctx, &sig.params, args);
                        }
                        // A `mod` with no output has no result (`Never`), as
                        // the un-namespaced path says.
                        let outputs = outputs.iter().map(|(n, te)| (n.clone(), resolve_type_expr(ctx, te))).collect();
                        return Type::call_result(outputs);
                    }
                    None => {
                        ctx.emit(
                            "WS002",
                            format!("'{}' not found in namespace '{}'", field, ns_name),
                            fa_range.clone(),
                        );
                        return Type::Any;
                    }
                }
            }
            // The receiver's container type, shared by the array and map arms.
            let container_recv = match callee.as_ref() {
                Expr::FieldAccess { obj, field, .. }
                    if crate::catalog::arrays::is_array_method(field)
                        || crate::catalog::maps::is_map_method(field) =>
                {
                    method_receiver_type(ctx, obj)
                }
                _ => None,
            };
            // Array method call: arr.push(val), arr.length(), arr.pop(), etc.
            // Any array-typed receiver works — a bare `var ids: T[]`, an `array`
            // decl, or a record field holding one (`g.ready.sum()`) — gated on
            // the field actually being an array method.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(method) = crate::catalog::arrays::array_method(field)
                && let Some(Type::Array(inner)) = container_recv.clone()
            {
                let elem = inner.as_ref().clone();
                // Every array method lowers to an `Exec_*` gate (a mutation runs
                // on the trigger; a read like `length`/`find`/`sum` samples on
                // it), so a pure-context call is invalid — reject it as WS007,
                // like a pure array index read. A READ on a `const` receiver is
                // exempt: that should const-fold (a separate missing feature)
                // and an exec error would mislead.
                if ctx.exec_mode() != ExecMode::Exec
                    && !container_call_exec_exempt(ctx, method.mutates, obj, args)
                {
                    ctx.emit(
                        "WS007",
                        format!(
                            "array {} '{}.{}(...)' outside an exec context",
                            if method.mutates { "mutation" } else { "read" },
                            target_name(obj).unwrap_or("<expr>".into()),
                            field
                        ),
                        fa_range.clone(),
                    );
                }
                // sortMultiple is a true variadic (empty params -> opt out of
                // arity, else "expects at most 0, got N" would wrongly fire).
                // Every other method arity-checks normally.
                let check_arity = field.as_str() != "sortMultiple";
                let mut sig = method.signature(&elem);
                precheck_ref_element_args(ctx, &elem, &mut sig.params, args);
                check_args(
                    ctx,
                    &sig,
                    args,
                    0,
                    check_arity,
                    // sortMultiple's variadic list can't enumerate its named
                    // args either, so its named check follows arity.
                    check_arity,
                    fa_range,
                );
                args_fill_float_params(ctx, &sig.params, args);
                if !check_arity {
                    check_sort_multiple_args(ctx, args, fa_range);
                }
                ctx.record_method_target(
                    call_range,
                    MethodTarget::Container {
                        kind: ContainerKind::Array { elem: elem.clone() },
                        method: field.clone(),
                    },
                );
                ctx.record_container_kind(obj, &Type::Array(Box::new(elem.clone())));
                ctx.record_container_args(args);
                // Return type is derived from the method's gate
                // output ports (see catalog::arrays). Multi-output
                // gates (e.g. find) yield a record that auto-unwraps
                // to whichever field matches the use.
                return crate::catalog::arrays::array_return_type(field, &elem)
                    .unwrap_or(Type::Any);
            }
            // Map method call: m.get(k), m.set(k, v), m.has(k), etc. Same
            // receiver reach as the array methods above.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(method) = crate::catalog::maps::map_method(field)
                && let Some(Type::Map(k, v)) = container_recv
            {
                let (key, value) = (k.as_ref().clone(), v.as_ref().clone());
                // Same rule as the array guard above: every map method is an
                // `Exec_*` gate, so a pure-context call (mutation OR read) is
                // rejected as WS007; a read on a `const` receiver is exempt.
                if ctx.exec_mode() != ExecMode::Exec
                    && !container_call_exec_exempt(ctx, method.mutates, obj, args)
                {
                    ctx.emit(
                        "WS007",
                        format!(
                            "map {} '{}.{}(...)' outside an exec context",
                            if method.mutates { "mutation" } else { "read" },
                            target_name(obj).unwrap_or("<expr>".into()),
                            field
                        ),
                        fa_range.clone(),
                    );
                }
                let mut sig = method.signature(&key, &value);
                precheck_ref_element_args(ctx, &key, &mut sig.params, args);
                precheck_ref_element_args(ctx, &value, &mut sig.params, args);
                check_args(
                    ctx,
                    &sig,
                    args,
                    0,
                    /*check_arity=*/ true,
                    /*check_named=*/ true,
                    fa_range,
                );
                args_fill_float_params(ctx, &sig.params, args);
                ctx.record_method_target(
                    call_range,
                    MethodTarget::Container {
                        kind: ContainerKind::Map { key: key.clone(), value: value.clone() },
                        method: field.clone(),
                    },
                );
                ctx.record_container_kind(
                    obj,
                    &Type::Map(Box::new(key.clone()), Box::new(value.clone())),
                );
                ctx.record_container_args(args);
                return crate::catalog::maps::map_return_type(field, &key, &value)
                    .unwrap_or(Type::Any);
            }
            // A receiver builtin is called only on a value of its receiver type
            // or one that converts to it as a value (an int to a float, a
            // character to an entity), the same rule WS035 uses. A `self` mod
            // of that name is called on any other value, and a string method
            // on a value that is not a string (`vv.Length()` on a vector,
            // which would read the text the vector formats to) is WS003.
            let mut defer_to_self_mod = false;
            if let Expr::FieldAccess { obj, field, range: fa_range } = callee.as_ref()
                && let Some(recv) = find_call(field).and_then(|c| c.receiver.as_ref())
                && let self_mod = ctx.visible_self_mod(field, &fa_range.file).is_some()
                && (self_mod || *recv == Type::String)
            {
                let mark = ctx.diagnostics.len();
                let got = unwrap_ref(&infer(ctx, obj)).container_ref_as_value();
                let receiver_errors = ctx.diagnostics.split_off(mark);
                let fits = matches!(coerce(&got, recv), CoerceRule::Same | CoerceRule::Coerce);
                defer_to_self_mod = !fits && self_mod;
                // The receiver is typed once, here, unless a `self` mod takes
                // the call and types it again: its own errors stand.
                if !defer_to_self_mod {
                    ctx.diagnostics.extend(receiver_errors);
                }
                let container = matches!(got, Type::Array(_) | Type::Map(_, _));
                if !fits
                    && !self_mod
                    && *recv == Type::String
                    && (container || coerce(&got, recv) == CoerceRule::ViaString)
                {
                    let spelled = receiver_spelling(obj);
                    let got_str = crate::analysis::types::type_str(&got);
                    let a = if got_str.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
                    let what = spelled.as_ref().map_or("the receiver".to_string(), |s| format!("`{s}`"));
                    let message = match (ctx.unbounded_param_of(obj, &got), spelled.as_deref()) {
                        (Some(tp), _) => format!(
                            "`.{field}()` is a string method, and `{tp}` is unbounded, so {what} may be \
                             {a} `{got_str}`; bound it: `<{tp}: string>`"
                        ),
                        (None, s) => {
                            let hint = match (field.as_str(), &got, s) {
                                ("Length", Type::Vector, Some(s)) => {
                                    format!("; a vector's length is `Magnitude({s})`")
                                }
                                ("Length", Type::Vector, None) => "; a vector's length is its `Magnitude`".to_string(),
                                ("Length", Type::Array(_), Some(s)) => {
                                    format!("; an array's element count is `{s}.length()`")
                                }
                                ("Length", Type::Map(..), Some(s)) => {
                                    format!("; a map's entry count is `{s}.length()`")
                                }
                                ("Length", Type::Array(_) | Type::Map(..), None) => {
                                    "; its element count is `.length()`".to_string()
                                }
                                _ if container => String::new(),
                                (_, _, Some(s)) => format!(
                                    ", and it would read the text the {got_str} formats to. Pass it as an \
                                     argument (`{field}({s})`) to format it on purpose"
                                ),
                                (_, _, None) => format!(
                                    ", and it would read the text the {got_str} formats to. Pass it as an \
                                     argument to `{field}` to format it on purpose"
                                ),
                            };
                            format!("`.{field}()` is a string method, and {what} is {a} `{got_str}`{hint}")
                        }
                    };
                    if let (Some(tp), Some(body)) = (ctx.unbounded_param_of(obj, &got), ctx.open_bodies.last()) {
                        let hint = (fa_range.clone(), message.clone(), body.clone(), tp.to_string());
                        ctx.string_bound_hints.push(hint);
                    }
                    ctx.emit("WS003", message, fa_range.clone());
                    return Type::Any;
                }
            }
            // Receiver method call: entity.SetLocation(pos)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(c) = find_call(field)
                && c.receiver.is_some()
                && !defer_to_self_mod
            {
                let mark = ctx.diagnostics.len();
                // Named-target receiver (`entity.SendCustomEvent(…)`) binds the
                // object to `target`; positional args stay as the channel name +
                // data. Ordinary receiver binds the object as positional arg 0.
                let recv_args = if let Some(tp) = c.receiver_target_param() {
                    let mut v = args.to_vec();
                    v.push(CallArg::Named {
                        name: tp.to_string(),
                        name_range: obj.range().clone(),
                        value: obj.as_ref().clone(),
                    });
                    v
                } else {
                    let mut v = vec![CallArg::Positional(obj.as_ref().clone())];
                    v.extend(args.iter().cloned());
                    v
                };
                check_args(ctx, &sig_of_callspec(c), &recv_args, 0, true, true, fa_range);
                // A call whose arguments do not fit resolves to nothing.
                if !has_error_since(ctx, mark) {
                    ctx.record_method_target(call_range, MethodTarget::Builtin(field.clone()));
                }
                // Same exec-context rule the plain-call form applies above —
                // the receiver spelling of the same gate needs it too, or
                // `e.GetLocation()` would silently compile to a no-op in pure
                // position while `GetLocation(e)` correctly errors.
                if c.exec && ctx.exec_mode() != ExecMode::Exec {
                    let has_exec_arg = args
                        .iter()
                        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                    if !has_exec_arg {
                        ctx.emit(
                            "WS007",
                            format!(
                                "exec call '{field}' outside an exec context (pass exec = ... to override)"
                            ),
                            fa_range.clone(),
                        );
                    }
                }
                if !c.outputs.is_empty() {
                    return output_record_type(ctx, c, &recv_args, fa_range);
                }
                // Void receiver method (`a.push(x)`): `Never`, so using its
                // "result" as a value is a mismatch rather than silently
                // accepted. Same rule as the plain-call site above.
                return Type::Never;
            }
            // User `self`-receiver method call: `v.dist(o)` where `dist` is a
            // user mod/chip whose first parameter is named `self`. Desugars to
            // `dist(v, o)` — bind the receiver as positional arg 0 and type it
            // exactly like the plain `dist(v, o)` call would (generics inferred
            // through the receiver too). Placed AFTER the builtin-receiver case
            // so a builtin receiver-method of the same name wins on its own
            // receiver type (a user self-mod shadowing one is rejected as WS035
            // at its declaration).
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(sym) = ctx.visible_self_mod(field, &fa_range.file).cloned()
            {
                // A container receiver was typed above, its diagnostics with
                // it; inferring it again would report them twice.
                let recv_ty = match &container_recv {
                    Some(t) => t.clone(),
                    None => infer(ctx, obj),
                };
                let mut recv_pos_types = Vec::with_capacity(positional_arg_types.len() + 1);
                recv_pos_types.push(recv_ty);
                recv_pos_types.extend(positional_arg_types.iter().cloned());
                // Mirror `recv_pos_types`: bind the receiver as positional arg
                // 0 so it lines up with the `self` param, then the call's own
                // args in order.
                let mut recv_args = Vec::with_capacity(args.len() + 1);
                recv_args.push(CallArg::Positional(obj.as_ref().clone()));
                recv_args.extend(args.iter().cloned());
                let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                let has_exec_arg = args
                    .iter()
                    .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                let positional_count = 1 + args
                    .iter()
                    .filter(|a| matches!(a, CallArg::Positional(_)))
                    .count();
                let mark = ctx.diagnostics.len();
                let ty = type_user_symbol_call(
                    ctx,
                    field,
                    &sym,
                    &recv_pos_types,
                    &recv_args,
                    type_args,
                    positional_count,
                    has_spread,
                    has_exec_arg,
                    call_range,
                    fa_range,
                );
                if !has_error_since(ctx, mark) {
                    ctx.record_method_target(call_range, MethodTarget::SelfMod(field.clone()));
                }
                // A receiver of an unbounded type parameter is checked as each
                // type it may be; say so rather than naming that type alone.
                let recv_val = unwrap_ref(&recv_pos_types[0]).container_ref_as_value();
                if let Some(tp) = ctx.unbounded_param_of(obj, &recv_val).map(str::to_string)
                    && let Some(self_ty) = sym.signature.as_ref().and_then(|s| s.params.first()).map(|p| p.ty.clone())
                {
                    let what = target_name(obj).map_or("the receiver".to_string(), |s| format!("`{s}`"));
                    let got = crate::analysis::types::type_str(&recv_val);
                    let want = crate::analysis::types::type_str(&self_ty);
                    let a = if got.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
                    // A builtin method of the same name answers its own
                    // receiver type, so the bound admits both.
                    let bound = match find_call(field).and_then(|c| c.receiver.as_ref()) {
                        Some(r) if *r != self_ty => format!("{} | {want}", crate::analysis::types::type_str(r)),
                        _ => want.clone(),
                    };
                    for d in &mut ctx.diagnostics[mark..] {
                        if d.message.starts_with("argument 'self'") {
                            d.message = format!(
                                "`.{field}()` calls `{field}(self: {want})`, and `{tp}` is unbounded, so \
                                 {what} may be {a} `{got}`; bound it: `<{tp}: {bound}>`"
                            );
                        }
                    }
                }
                return ty;
            }
            // A method/namespace call whose base identifier resolves to nothing:
            // e.g. `card.drawLobby(...)` after an `import * as card` was removed.
            // None of the branches above matched and `card` is not a namespace,
            // variable, or value in scope. Left alone this silently lowers to an
            // `_Unsupported` gate that reads a default (does nothing) at runtime —
            // flag the dangling base, mirroring the bare-identifier WS002 above.
            // Returns so it stays the primary diagnostic for an unknown base
            // (the non-self-mod check below never double-reports over it).
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident {
                    name,
                    range: base_range,
                } = obj.as_ref()
                && (ctx.scope.lookup(name).is_none()
                    || ctx.namespace_hidden_here(name, &base_range.file))
                && find_call(name).is_none()
            {
                // A namespace hidden here is one that TRAVELED in privately (a
                // pulled-in module's own `import * as`) — naming it directly is
                // a missing import in THIS file, so it reports the same as a
                // genuinely-undefined base rather than resolving the leak (N11).
                ctx.emit(
                    "WS002",
                    format!(
                        "unknown identifier '{name}' in call `{name}.{field}(...)`: \
                         no namespace, variable, or value named '{name}' is in scope \
                         (is an import missing?)"
                    ),
                    base_range.clone(),
                );
                return Type::Any;
            }
            // A namespace call whose base is SHADOWED by a local binding:
            // `import * as ns` plus a parameter, `let`, or `var` also named
            // `ns` makes `ns.f(...)` resolve against the local value, which
            // has no member `f`. Every accepting branch above has already had
            // its turn, so the receiver genuinely cannot answer this call.
            // Left alone it types as `any` and lowers to an `_Unsupported`
            // no-op: the destructured names all read as `any`, the error (if
            // any) surfaces far downstream at whatever consumes them, and
            // `wirescript-check` reports the file clean. Name the shadowing,
            // since the fix is to rename one of the two and nothing about the
            // call site itself looks wrong.
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident {
                    name,
                    range: base_range,
                } = obj.as_ref()
                && ctx.ns_members(name, &base_range.file).is_some()
                && ctx
                    .scope
                    .lookup(name)
                    .is_some_and(|s| s.kind != SymbolKind::Namespace)
            {
                ctx.emit(
                    "WS002",
                    format!(
                        "'{name}' here is a local value, not the imported namespace, so \
                         `{name}.{field}(...)` looks for a member on that value instead of \
                         in the module. Rename the local binding or the `import * as {name}`."
                    ),
                    base_range.clone(),
                );
                return Type::Any;
            }
            // A method call `obj.field(...)` on a valid receiver whose `field`
            // names a KNOWN user mod/chip that is NOT a `self`-receiver (the
            // self-mod case above would have returned; builtin / array / map
            // receiver methods were handled earlier). Only a `self`-mod is
            // method-callable — without this, the call would silently type as
            // `any` and lower to an `_Unsupported` no-op (typecheck/lowering
            // divergence). Flag it instead of letting it disappear.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && ctx.scope.lookup(field).is_some_and(|s| {
                    s.kind == SymbolKind::Chip
                        && s.signature.as_ref().is_none_or(|sig| sig.params.first().is_none_or(|p| p.name != "self"))
                })
            {
                // Keep the receiver's type in the map (hover/goto) even though
                // the call itself doesn't resolve to a value.
                infer(ctx, obj);
                ctx.emit(
                    "WS036",
                    format!(
                        "`{field}` is not a receiver method: its first parameter is not \
                         named `self`, so it can't be called as `x.{field}(…)`. Call \
                         `{field}(<receiver>, …)` directly, or rename its first parameter \
                         to `self` to allow method syntax."
                    ),
                    fa_range.clone(),
                );
                return Type::Any;
            }
            // A method call `obj.field(...)` where `field` is a KNOWN BUILTIN
            // that takes NO receiver — `Sweep`/`SweepSimple` act on their own
            // brick, not a receiver object, so there's nowhere to bind `obj`.
            // Without this the call falls through to `any` and lowers to an
            // `_Unsupported` no-op (typecheck/lowering divergence). Flag it so
            // `x.SweepSimple(…)` fails loudly instead of silently doing nothing.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && find_call(field).is_some_and(|c| c.receiver.is_none())
            {
                infer(ctx, obj); // keep the receiver's type in the map (hover/goto)
                ctx.emit(
                    "WS036",
                    format!(
                        "`{field}` takes no receiver: it acts on its own brick, not a \
                         receiver object. Call it as `{field}(…)` directly, not \
                         `x.{field}(…)`."
                    ),
                    fa_range.clone(),
                );
                return Type::Any;
            }
            // A `self` mod this file can't name: a module's private one, present
            // only because an imported body calls it, or the importer's own.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(s) = ctx.scope.lookup(field).filter(|s| s.kind == SymbolKind::Chip).cloned()
            {
                infer(ctx, obj);
                if let Some(msg) = ctx.unnamed_mod_call(field, &s.decl_range, &fa_range.file, true) {
                    ctx.emit("WS036", msg, fa_range.clone());
                }
                return Type::Any;
            }
            // Every resolvable call form returned above. A callee whose object
            // is itself a field access (`A.B.bar(...)`) is an unresolved
            // nested-namespace path; error rather than lower to `_Unsupported`
            // with exit 0. A single-level `x.method()` stays permissive here.
            if let Expr::FieldAccess { obj, field, range } = callee.as_ref()
                && matches!(obj.as_ref(), Expr::FieldAccess { .. })
            {
                let _ = infer(ctx, obj);
                ctx.emit(
                    "WS002",
                    format!("cannot resolve call to '{field}'"),
                    range.clone(),
                );
            }
            Type::Any
        }
    }
}

/// [`check`] for a stored record's initializer or assigned value: a map literal
/// may fill a `Map` field of a record literal there (see
/// `TypeCheckCtx::record_lit_map_fields`).
pub(crate) fn check_stored_value(ctx: &mut TypeCheckCtx, e: &Expr, expected: &Type) -> Type {
    ctx.record_lit_map_fields = matches!(unwrap_ref(expected), Type::Record(_));
    check(ctx, e, expected)
}

pub(crate) fn check(ctx: &mut TypeCheckCtx, e: &Expr, expected: &Type) -> Type {
    // `null` is polymorphic: it takes the expected type and lowers to that type's
    // zero/default. Resolve it here (before the generic infer+coerce), recording
    // the resolved type so lowering emits the right default literal.
    // Taken on entry so the allowance reaches only this literal's own fields
    // (and nested record literals), never a call argument below them.
    let map_fields = std::mem::take(&mut ctx.record_lit_map_fields);
    if let Expr::NullLit { .. } = e {
        return check_null(ctx, e, expected);
    }
    // `{}` where a map is expected is the empty map literal (the parser reads
    // it as an empty record literal), so it is valid exactly where a map
    // literal is: here, a map field of a stored record's literal.
    if let Expr::RecordLit { fields, range } = e
        && fields.is_empty()
        && let Type::Map(k, v) = unwrap_ref(expected)
    {
        if !map_fields {
            ctx.emit(
                "WS026",
                "a map literal must initialize or assign a Map variable",
                range.clone(),
            );
        }
        let map = Type::Map(k, v);
        ctx.record_expr_type(range, &map);
        return map;
    }
    // Push a record type into a record literal so each field value is CHECKED
    // against its target type (a `null` field resolves to it) rather than
    // inferred blind. Guarded to a plain, exactly-matching set of named fields;
    // any spread / shorthand / arity mismatch defers to the infer+coerce below,
    // which reports the structural error unchanged.
    // A shorthand field `{ x }` is the named field `{ x: x }`.
    if let Expr::RecordLit { fields, .. } = e
        && let Type::Record(ftypes) = unwrap_ref(expected)
        && let Some(named) = fields
            .iter()
            .map(|f| match f {
                RecordLitField::Named { name, value, .. } => Some((name.clone(), std::borrow::Cow::Borrowed(value))),
                RecordLitField::Shorthand { name, range } => Some((
                    name.clone(),
                    std::borrow::Cow::Owned(Expr::Ident {
                        name: name.clone(),
                        range: range.clone(),
                    }),
                )),
                RecordLitField::Spread { .. } => None,
            })
            .collect::<Option<Vec<_>>>()
        && named.len() == ftypes.len()
        && ftypes
            .iter()
            .all(|(fname, _)| named.iter().any(|(k, _)| k == fname))
    {
        for (fname, ftype) in &ftypes {
            let val: &Expr = named.iter().find(|(k, _)| k == fname).map(|(_, v)| v.as_ref()).unwrap();
            // A place (a var, or a `*T` parameter) fills a `*T` field by
            // naming its var, and reads as that var's value.
            let ftype = match ftype {
                Type::Ref(inner) if names_a_place(ctx, val) => inner.as_ref(),
                other => other,
            };
            match (super::decl::map_init_entries(val), unwrap_ref(ftype)) {
                (Some(entries), Type::Map(k, v)) if map_fields => {
                    super::decl::check_map_literal(ctx, entries, &k, &v);
                    let r = val.range();
                    ctx.record_expr_type(r, &Type::Map(k, v));
                }
                _ => {
                    ctx.record_lit_map_fields = map_fields && matches!(val, Expr::RecordLit { .. });
                    check(ctx, val, ftype);
                }
            }
        }
        let r = e.range();
        let rec = Type::Record(ftypes);
        ctx.record_expr_type(r, &rec);
        return rec;
    }
    // Any other shape of a stored record's literal (a spread, a shorthand) is
    // typed as `infer` types it, except that a map literal may fill a `Map`
    // field (`h = { ...h2, m: { "a": 1 } }`).
    if map_fields
        && let Expr::RecordLit { fields, range } = e
        && let Type::Record(ftypes) = unwrap_ref(expected)
    {
        let t = infer_record_lit(ctx, fields, Some(&ftypes));
        ctx.record_expr_type(range, &t);
        coerce_or_emit(ctx, &t, expected, range);
        return t;
    }
    // PROTOTYPE: push an expected element type into an array literal, so
    // `var a: float[] = [1, 2]` / `var a: int[] = []` check element-wise
    // instead of inferring `int[]`/`any[]` blind and failing a whole-array
    // coerce.
    if let Expr::Array { elements, .. } = e
        && let Type::Array(want) = unwrap_ref(expected)
    {
        for el in elements {
            match el {
                ArrayElem::Item(v) => {
                    let got = check(ctx, v, &want);
                    // `check` applies the ASSIGNMENT rule, which allows
                    // `ViaString` because an assignment has a wire for the
                    // `FormatText` gate to sit on. An array element is baked
                    // into the array's constant list instead of wired, so there
                    // is no such gate and emit renders the value as `""` - the
                    // same reason a map literal entry rejects it. Only the
                    // EXTRA rejection is emitted here; `check` already reported
                    // an outright mismatch.
                    if !coerces_at_a_baked_literal(&got, &want)
                        && coerce(&got, &want) != CoerceRule::Mismatch
                    {
                        ctx.emit(
                            "WS003",
                            format!(
                                "array element: expected {}, got {}; an array element can't \
                                 be string-formatted, so write a matching-type value",
                                crate::analysis::types::type_str(&want),
                                crate::analysis::types::type_str(&got),
                            ),
                            v.range().clone(),
                        );
                    }
                }
                ArrayElem::Spread(v) => {
                    check(ctx, v, &Type::Array(want.clone()));
                    ctx.record_container_expr(v);
                }
            }
        }
        let r = e.range();
        let arr = Type::Array(want.clone());
        ctx.record_expr_type(r, &arr);
        return arr;
    }
    // Push the expected type down for the single node `infer` is about to type,
    // so a generic enum construction (`n: Option<int> = None`) can take its `T`
    // from the annotation. `infer_node` `take()`s it immediately, so it never
    // reaches a nested sub-expression; restore the prior hint for a `check`
    // running inside another `check`.
    let prev = ctx.expected_ty.replace(expected.clone());
    let t = infer(ctx, e);
    ctx.expected_ty = prev;
    coerce_value_or_emit(ctx, e, &t, expected);
    int_constants_in_slot(ctx, e, expected);
    t
}

/// [`int_constants_in_slot`] for each argument against the parameter it
/// binds. Only for parameters whose type is the slot's own type (a user
/// `mod`/`chip` parameter, a container element): a builtin's `float`-declared
/// math port takes any number as written. A spread's arguments are not
/// matched to parameters here.
pub(crate) fn args_fill_float_params(ctx: &mut TypeCheckCtx, params: &[Param], args: &[CallArg]) {
    let mut positional = params.iter();
    for a in args {
        match a {
            CallArg::Positional(e) => match positional.next() {
                Some(p) => int_constants_in_slot(ctx, e, &p.ty),
                None => break,
            },
            CallArg::Named { name, value, .. } => {
                if let Some(p) = params.iter().find(|p| &p.name == name) {
                    int_constants_in_slot(ctx, value, &p.ty);
                }
            }
            CallArg::Spread(_) => break,
        }
    }
}

/// `min`/`max`/`clamp`, `abs`/`sign` and the roundings over integers give an
/// integer: the call types as the join of its operands, `int` when every one
/// is an `int` (the catalog's `float` result is the join once one is a
/// float). `None` leaves the catalog's type.
fn integer_math_result(ctx: &TypeCheckCtx, name: &str, args: &[CallArg]) -> Option<Type> {
    const JOINED: [&str; 8] = ["min", "max", "clamp", "abs", "sign", "floor", "ceil", "round"];
    if !JOINED.contains(&name) || args.is_empty() {
        return None;
    }
    let all_int = args.iter().all(|a| {
        let e = match a {
            CallArg::Positional(e) | CallArg::Named { value: e, .. } => e,
            CallArg::Spread(_) => return false,
        };
        let r = e.range();
        matches!(
            ctx.type_of_expr.get(&(r.file.clone(), r.start.offset, r.end.offset)).map(unwrap_ref),
            Some(Type::Int)
        )
    });
    all_int.then_some(Type::Int)
}

/// `abs`/`min`/`max`/`clamp`/`Dampen` give the type of their math-value
/// arguments (`float` when that is not one math value), so an integer or bool
/// constant among them takes the call's numeric type: `clamp(f, 0, 10)` with a
/// `float` `f` bakes `0.0` and `10.0`. A fixed-type argument (`Dampen`'s
/// `smoothTime`) keeps its own type.
fn variant_math_result(ctx: &mut TypeCheckCtx, c: &crate::catalog::calls::CallSpec, args: &[CallArg], result: Type) -> Type {
    use crate::ir::gate_class as gc;
    if !matches!(c.gate_class, gc::MATH_ABS | gc::MATH_MIN | gc::MATH_MAX | gc::MATH_CLAMP | gc::DAMPEN) {
        return result;
    }
    let result = match op_operand_type(&result) {
        t @ (Type::Int | Type::Float | Type::Vector | Type::Rotator | Type::Quat | Type::Color) => t,
        _ => Type::Float,
    };
    if matches!(result, Type::Int | Type::Float) {
        let mut pos = 0;
        for a in args {
            let (param, e) = match a {
                CallArg::Positional(e) => {
                    pos += 1;
                    (c.params.get(pos - 1), e)
                }
                CallArg::Named { name, value, .. } => (c.params.iter().find(|p| p.name == name.as_str()), value),
                CallArg::Spread(_) => continue,
            };
            if param.is_some_and(|p| matches!(p.ty, Type::Union(_))) {
                int_constants_in_slot(ctx, e, &result);
            }
        }
    }
    result
}

/// The type a `match`, `if let` or `let ... else` reads from `scrutinee`: a
/// gate read whose first output is an enum (`hs.pop()`, `m.get(k)`, a call
/// of a `mod` with several outputs) is that enum, as it is wherever one value
/// is expected.
pub(super) fn infer_scrutinee(ctx: &mut TypeCheckCtx, scrutinee: &Expr) -> Type {
    let t = unwrap_ref(&infer(ctx, scrutinee));
    if let Type::Outputs(fields) = &t
        && let Some((_, first)) = fields.first()
        && let first @ Type::Enum { .. } = unwrap_ref(first)
    {
        return first;
    }
    t
}

/// An integer constant takes the numeric type of the slot it flows into: the
/// float a `float` slot names, the integer an `int` slot names. A float
/// constant that is the whole value of an `int` slot is the integer it
/// truncates to (`WS106` when that drops a fraction), as a `var` declared `int`
/// stores it; a float constant giving one branch of an `if`/`match` keeps the
/// join's type, and the slot converts the joined value. This is the
/// one place that rule lives (its literal form is
/// `const_eval::coerce_const_to_type`): every route a value takes into a
/// typed slot (`check`, user `mod`/`chip` and container-method arguments, a
/// namespaced call, an enum payload, an annotated `let`/`const`, an output
/// write, a `return`, a map or array literal, a record or tuple literal's
/// fields, the join of an `if`/`match`) calls it with the slot's type, and the
/// outermost slot is applied last, so it decides. It records the type of each
/// integer leaf of `e`: a literal (or a negated one), a name bound to an
/// integer constant, the value an `if`/`match` arm or a block gives, and, for
/// a record, tuple, array or map slot, the fields, elements and entries of a
/// literal of that shape. Lowering builds a float for every integer constant
/// recorded `float`, so the folded and runtime values agree.
pub(crate) fn int_constants_in_slot(ctx: &mut TypeCheckCtx, e: &Expr, slot: &Type) {
    constants_in_slot(ctx, e, slot, true)
}

/// [`int_constants_in_slot`] for `e`, which is the whole value of the slot
/// when `whole` is set and one branch of a join otherwise.
fn constants_in_slot(ctx: &mut TypeCheckCtx, e: &Expr, slot: &Type, whole: bool) {
    let slot = unwrap_ref(slot);
    let number = matches!(slot, Type::Float | Type::Int);
    let recorded = |ctx: &TypeCheckCtx, r: &SourceRange| {
        ctx.type_of_expr.get(&(r.file.clone(), r.start.offset, r.end.offset)).cloned()
    };
    match (e, &slot) {
        (Expr::IntLit { range, value, .. }, t) if number => {
            ctx.record_expr_type(range, t);
            warn_inexact_float(ctx, *value, t, range);
        }
        // An integer literal given to a `bool` place is the bool it names, and
        // a bool literal given to an `int` place the integer.
        (Expr::IntLit { range, .. }, Type::Bool) if whole => ctx.record_expr_type(range, &Type::Bool),
        (Expr::BoolLit { range, .. }, Type::Int) if whole => ctx.record_expr_type(range, &Type::Int),
        (Expr::FloatLit { range, value, text }, Type::Int) if whole => {
            ctx.record_expr_type(range, &Type::Int);
            report_float_made_int(ctx, *value, range, Some(text));
        }
        (Expr::UnOp { op, operand, range }, Type::Int)
            if whole && op == crate::catalog::operators::op::NEG && matches!(operand.as_ref(), Expr::FloatLit { .. }) =>
        {
            if let Expr::FloatLit { range: inner, value, text } = operand.as_ref() {
                ctx.record_expr_type(inner, &Type::Int);
                ctx.record_expr_type(range, &Type::Int);
                report_float_made_int(ctx, -*value, range, Some(&format!("-{text}")));
            }
        }
        (Expr::UnOp { op, operand, range }, t)
            if number
                && op == crate::catalog::operators::op::NEG
                && super::stmt::int_literal(operand).is_some() =>
        {
            ctx.record_expr_type(range, t);
            int_constants_in_slot(ctx, operand, &slot);
        }
        // A name, an operator, a call, an index or a field read is typed by
        // its operands; only an integer constant's value changes, and only
        // away from the type it already has (`N + 2`, `-N`, `A[1]`, `R.a`
        // with every name a constant).
        (
            Expr::Ident { range, .. }
            | Expr::BinOp { range, .. }
            | Expr::UnOp { range, .. }
            | Expr::Call { range, .. }
            | Expr::IndexAccess { range, .. }
            | Expr::FieldAccess { range, .. }
            | Expr::TuplePick { range, .. },
            t,
        ) if number && recorded(ctx, range).is_some_and(|r| matches!(r, Type::Int | Type::Float) && r != *t) => {
            if let Some(value) = int_constant_value(ctx, e) {
                ctx.record_expr_type(range, t);
                warn_inexact_float(ctx, value, t, range);
            } else if whole
                && *t == Type::Int
                && let Some(value) = float_constant_value(ctx, e)
            {
                ctx.record_expr_type(range, t);
                report_float_made_int(ctx, value, range, None);
            }
        }
        (Expr::IfExpr { then_branch, else_branch, .. }, _) => {
            constants_in_slot(ctx, then_branch, &slot, false);
            constants_in_slot(ctx, else_branch, &slot, false);
        }
        (Expr::MatchExpr { arms, .. }, _) => {
            for arm in arms {
                if let MatchBody::Expr(x) = &arm.body {
                    constants_in_slot(ctx, x, &slot, false);
                }
            }
        }
        (Expr::BlockExpr { value, .. }, _) => constants_in_slot(ctx, value, &slot, whole),
        (Expr::RecordLit { fields, .. }, Type::Record(_) | Type::Tuple(_)) => {
            for f in fields {
                if let RecordLitField::Named { name, value, .. } = f
                    && let Some(ft) = field_slot(&slot, name)
                {
                    int_constants_in_slot(ctx, value, &ft);
                }
            }
        }
        (Expr::Array { elements, range }, Type::Array(elem)) => {
            for el in elements {
                if let ArrayElem::Item(v) = el {
                    int_constants_in_slot(ctx, v, elem);
                }
            }
            container_literal_in_slot(ctx, range, &slot);
        }
        (Expr::MapLit { entries, range }, Type::Map(k, v)) => {
            for en in entries {
                int_constants_in_slot(ctx, &en.key, k);
                int_constants_in_slot(ctx, &en.value, v);
            }
            container_literal_in_slot(ctx, range, &slot);
        }
        _ => {}
    }
}

/// An array or map literal at `range` in a container slot of type `slot`
/// holds the slot's element types: the container lowering builds for it is
/// declared by the slot, as a `var` of that type would be, when the two
/// differ only by an integer the slot holds as a float (or an element type
/// an empty literal leaves open).
fn container_literal_in_slot(ctx: &mut TypeCheckCtx, range: &SourceRange, slot: &Type) {
    fn held_as(from: &Type, to: &Type) -> bool {
        match (from, to) {
            (Type::Any, b) => !matches!(b, Type::Any | Type::Param(_)),
            (Type::Int, Type::Float) => true,
            (Type::Array(a), Type::Array(b)) => held_as(a, b),
            (Type::Map(ka, va), Type::Map(kb, vb)) => held_as(ka, kb) && held_as(va, vb),
            (Type::Record(a), Type::Record(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|((na, ta), (nb, tb))| na == nb && held_as(ta, tb))
            }
            (Type::Tuple(a), Type::Tuple(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| held_as(x, y)),
            (a, b) => crate::types::coerce::type_eq(a, b),
        }
    }
    let key = (range.file.clone(), range.start.offset, range.end.offset);
    if let Some(recorded) = ctx.type_of_expr.get(&key)
        && recorded != slot
        && held_as(recorded, slot)
    {
        ctx.record_expr_type(range, slot);
    }
}

/// Whether an error was reported after the first `mark` diagnostics.
fn has_error_since(ctx: &TypeCheckCtx, mark: usize) -> bool {
    ctx.diagnostics[mark..].iter().any(|d| d.severity == crate::diagnostic::Severity::Error)
}

/// The type of field `name` of a record slot, or of element `name` (`"0"`,
/// `"1"`, ...) of a tuple slot.
fn field_slot(slot: &Type, name: &str) -> Option<Type> {
    match slot {
        Type::Record(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone()),
        Type::Tuple(elems) => name.parse::<usize>().ok().and_then(|i| elems.get(i).cloned()),
        _ => None,
    }
}

/// The `WS002` for the enum `name` a file writes at `range` and the program
/// does not let it name, when another enum of the name is declared where the
/// file sees it: a duplicate of the program's (`WS013`), not a missing import.
pub(super) fn hidden_enum_message(ctx: &TypeCheckCtx, name: &str, range: &SourceRange) -> Option<String> {
    let file_name = |f: &str| f.rsplit(['/', '\\']).next().unwrap_or(f).to_string();
    let from = ctx.enum_decls.get(name).and_then(|decls| {
        decls.iter().find(|r| r.file == range.file || ctx.imports.sees(&range.file, r)).map(|r| file_name(&r.file))
    })?;
    Some(format!(
        "unknown type '{name}': the enum `{name}` this file names (declared in {from}) is not the program's, \
         which declares another enum of that name (`WS013`). Rename one"
    ))
}

/// The value of `e` when it is an integer compile-time constant (a name bound
/// to one by a `const` or a `let` of a constant, or an expression of such
/// names and literals), whose value lowering bakes as a literal. Each name
/// resolves through the scope (`certified_const_ctx`): a parameter or local of
/// the same name is not that constant. An integer operation the certified
/// fold table has no result for (an overflow), in `e`, in a `const mod` it
/// calls or behind a name it reads, leaves the value to the gate.
pub(super) fn int_constant_value(ctx: &TypeCheckCtx, e: &Expr) -> Option<i64> {
    let lookup = |n: &str| ctx.resolve_mod(n);
    let cx = ctx.certified_const_ctx(Some(&lookup));
    let mut budget = crate::const_eval::Budget::default();
    match crate::const_eval::eval_expr(e, &cx, &mut budget) {
        Ok(crate::ir::Literal::Int(n)) if !budget.uncertified => Some(n),
        _ => None,
    }
}

/// The value of `e` when it is a float constant every operation of which the
/// certified fold table gives.
pub(super) fn float_constant_value(ctx: &TypeCheckCtx, e: &Expr) -> Option<f64> {
    let lookup = |n: &str| ctx.resolve_mod(n);
    let cx = ctx.certified_const_ctx(Some(&lookup));
    let mut budget = crate::const_eval::Budget::default();
    match crate::const_eval::eval_expr(e, &cx, &mut budget) {
        Ok(crate::ir::Literal::Float(f)) if !budget.uncertified => Some(f),
        _ => None,
    }
}

/// A float constant made an integer (`const_eval::float_as_int`): one that
/// drops a fraction doing so is stored as the integer it truncates to
/// (`WS106`), and one no `int` holds is `WS028`; once per expression.
fn report_float_made_int(ctx: &mut TypeCheckCtx, value: f64, range: &SourceRange, text: Option<&str>) {
    if ctx.diagnostics.iter().any(|d| matches!(&*d.code, "WS106" | "WS028") && d.range == *range) {
        return;
    }
    // A literal is quoted as written.
    let written = text.map_or_else(|| crate::const_eval::written_float(value), str::to_string);
    match crate::const_eval::float_as_int(value) {
        // A literal no float holds exactly reads as the nearest float, which
        // may be whole where the literal is not, or another whole number.
        Some((stored, true)) if text.is_some_and(|t| !crate::const_eval::literal_is_integer(t, stored)) => ctx.warn(
            "WS106",
            format!(
                "{written} goes into an `int` here, and no float holds it exactly, so it is stored as \
                 {stored}, not the value written. Write {stored} if that is what you mean"
            ),
            range.clone(),
        ),
        Some((_, true)) => {}
        Some((stored, false)) => ctx.warn(
            "WS106",
            format!(
                "{written} goes into an `int` here, so it is stored as {stored}: the fraction is dropped. \
                 Write {stored} if that is what you mean"
            ),
            range.clone(),
        ),
        None => {
            // A literal written within the `int` range is a float beyond it.
            let nearest = text
                .filter(|t| !t.contains(['e', 'E']))
                .and_then(|t| t.trim_start_matches('-').split('.').next()?.parse::<i128>().ok())
                .filter(|n| i64::try_from(*n).is_ok())
                .map(|_| format!(", where it is the float {value:.0} (no float holds the value written exactly)"))
                .unwrap_or_default();
            let that = if nearest.is_empty() { "it" } else { "that" };
            ctx.emit(
                "WS028",
                format!(
                    "{written} goes into an `int` here{nearest}, and no `int` holds {that}: an `int` holds whole \
                     numbers from {} to {}",
                    i64::MIN,
                    i64::MAX
                ),
                range.clone(),
            )
        }
    }
}

/// An integer constant made a float that a float cannot hold exactly (beyond
/// 2^53 in magnitude, where floats skip integers) is stored as the nearest
/// float: `WS104`, once per expression.
fn warn_inexact_float(ctx: &mut TypeCheckCtx, value: i64, slot: &Type, range: &SourceRange) {
    if !matches!(slot, Type::Float) || (value as f64) as i128 == value as i128 {
        return;
    }
    if ctx.diagnostics.iter().any(|d| d.code == "WS104" && d.range == *range) {
        return;
    }
    ctx.warn(
        "WS104",
        format!(
            "{value} goes into a `float` here, and a float cannot hold it exactly: it is stored as {}. \
             Integers beyond 2^53 (9007199254740992) need an `int` to keep every digit",
            value as f64
        ),
        range.clone(),
    );
}
/// Whether `null` can stand in for `t`: any stored VALUE type — a number, bool,
/// string, vector/rotator/quat/color, or an entity/character/controller (the
/// unset object). Records and containers have their own empty forms (`{}` / `[]`),
/// and reference-only / exec types carry no value. `Any` is permitted so an
/// already-unknown target doesn't pile on a second error.
fn null_typeable(t: &Type) -> bool {
    matches!(
        t,
        Type::Int
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Vector
            | Type::Rotator
            | Type::Quat
            | Type::Color
            | Type::Entity
            | Type::Character
            | Type::Controller
            | Type::Any
    )
}

fn check_null(ctx: &mut TypeCheckCtx, e: &Expr, expected: &Type) -> Type {
    let t = unwrap_ref(expected);
    if !null_typeable(&t) {
        ctx.emit(
            "WS051",
            format!(
                "`null` has no value for type `{}`: it is only valid for a number, bool, \
                 string, vector/rotator/color, or an entity/character/controller",
                crate::analysis::type_str(&t)
            ),
            e.range().clone(),
        );
    }
    let r = e.range();
    ctx.record_expr_type(r, &t);
    t
}

/// The shared WS003 sink.
/// Type an `xs[i]` / `m[k]` subscript's index, and check it against what the
/// container is keyed by: an array (and a string) by `int`, a map by its own
/// key type.
///
/// The method spelling `m.set(k, v)` goes through the ordinary argument path;
/// the `m[k]` subscript, which is the only spelling an array has, reaches here
/// instead, on both sides of an assignment.
pub(crate) fn check_index_type(ctx: &mut TypeCheckCtx, container: &Type, index: &Expr) {
    let got = unwrap_ref(&infer(ctx, index));
    let want = match container {
        Type::Array(_) | Type::String => Type::Int,
        Type::Map(k, _) => k.as_ref().clone(),
        // Anything else is a shape lowering resolves dynamically (or an
        // already-reported error); there is no key type to check against.
        _ => return,
    };
    coerce_or_emit(ctx, &got, &want, index.range());
    if want == Type::Int {
        int_constants_in_slot(ctx, index, &want);
    }
}

/// Whether a record, tuple, array or map literal `e` fits `slot` part by
/// part: each field, element or entry has its slot's type, is an integer
/// where the slot is a `float`, or is a float constant where the slot is an
/// `int`. A literal is built in place, so an integer part is the float a
/// `float` slot names and a float constant part the integer an `int` slot
/// truncates it to; a record or container VALUE keeps its own types
/// (`coerce` holds them to their field and element types).
pub(crate) fn literal_fits(ctx: &TypeCheckCtx, e: &Expr, slot: &Type) -> bool {
    let slot = unwrap_ref(slot);
    let part_fits = |x: &Expr, t: &Type| {
        let r = x.range();
        literal_fits(ctx, x, t)
            || ctx
                .type_of_expr
                .get(&(r.file.clone(), r.start.offset, r.end.offset))
                .is_some_and(|xt| {
                    let (xt, t) = (unwrap_ref(xt), unwrap_ref(t));
                    let xt = if matches!(t, Type::Record(_) | Type::Tuple(_)) { xt } else { first_output(&xt) };
                    coerce(&xt, &t) == CoerceRule::Same
                        || matches!((&xt, &t), (Type::Int, Type::Float))
                        || matches!((xt, t), (Type::Float, Type::Int)) && float_constant_value(ctx, x).is_some()
                })
    };
    match (e, &slot) {
        (Expr::RecordLit { fields, .. }, Type::Record(_) | Type::Tuple(_)) => {
            let width = match &slot {
                Type::Record(f) => f.len(),
                Type::Tuple(t) => t.len(),
                _ => 0,
            };
            fields.len() == width
                && fields.iter().all(|f| match f {
                    RecordLitField::Named { name, value, .. } => {
                        field_slot(&slot, name).is_some_and(|ft| part_fits(value, &ft))
                    }
                    RecordLitField::Shorthand { .. } | RecordLitField::Spread { .. } => false,
                })
        }
        (Expr::Array { elements, .. }, Type::Array(elem)) => elements.iter().all(|el| match el {
            ArrayElem::Item(v) => part_fits(v, elem),
            ArrayElem::Spread(_) => false,
        }),
        (Expr::MapLit { entries, .. }, Type::Map(k, v)) => {
            entries.iter().all(|en| part_fits(&en.key, k) && part_fits(&en.value, v))
        }
        _ => false,
    }
}

/// The type a gate read `t` gives a place for one value: its first output.
/// Any other type is itself.
pub(crate) fn first_output(t: &Type) -> Type {
    match t {
        Type::Outputs(fields) if !fields.is_empty() => unwrap_ref(&fields[0].1),
        other => other.clone(),
    }
}

/// [`coerce_or_emit`] for the value `e`: a literal that fits the target part
/// by part ([`literal_fits`]) is accepted, typed as the target, and its
/// integer parts in `float` slots become floats.
pub(crate) fn coerce_value_or_emit(ctx: &mut TypeCheckCtx, e: &Expr, from: &Type, to: &Type) {
    if record_into_one_value(ctx, e, from, to) {
        return;
    }
    literal_in_slot(ctx, e, from, to);
    if coerce(from, to) == CoerceRule::Mismatch && literal_fits(ctx, e, to) {
        ctx.record_expr_type(e.range(), &unwrap_ref(to));
        int_constants_in_slot(ctx, e, to);
        return;
    }
    coerce_or_emit(ctx, from, to, e.range());
}

/// A record or tuple literal is built in its slot: where a gate read fills
/// one of its fields, the literal is typed as the slot, so that field gives
/// the slot what it holds (one value its first output).
pub(crate) fn literal_in_slot(ctx: &mut TypeCheckCtx, e: &Expr, from: &Type, to: &Type) {
    let slot = unwrap_ref(to);
    if matches!(e, Expr::RecordLit { .. })
        && from.holds_outputs()
        && matches!(slot, Type::Record(_) | Type::Tuple(_))
        && !super::resolve::type_has_param(&slot)
        && coerce(from, &slot) != CoerceRule::Mismatch
    {
        ctx.record_expr_type(e.range(), &slot);
    }
}

/// WS003 for a record (a record literal, or a value of a record type: a
/// parameter, a `var`, a field, an element, a `mod` with one record output)
/// given where one value goes. `coerce` reads a gate read there as its first
/// output (`x = Edge(b)`, `n = fs.max()`, a call of a `mod` with several
/// outputs: a [`Type::Outputs`]); a record has no output to wire, so lowering
/// would wire something else. Whether it reported.
pub(crate) fn record_into_one_value(ctx: &mut TypeCheckCtx, e: &Expr, from: &Type, to: &Type) -> bool {
    if !rejected_where_one_value_goes(from, to) {
        return false;
    }
    report_record_as_one_value(ctx, e, from, &crate::analysis::types::type_str(&unwrap_ref(to)));
    true
}

/// [`record_into_one_value`] where no type is written for the one value: an
/// operand, a condition, an interpolated part. Whether it reported.
pub(crate) fn record_read_as_one_value(ctx: &mut TypeCheckCtx, e: &Expr, t: &Type) -> bool {
    if record_read_as_one(t).is_none() || ctx.is_error_typed(e) {
        return false;
    }
    report_record_as_one_value(ctx, e, t, "one value");
    true
}

/// WS003 for `e`, of type `t`, a record read where `expected` (one value)
/// goes.
fn report_record_as_one_value(ctx: &mut TypeCheckCtx, e: &Expr, t: &Type, expected: &str) {
    let Some(value) = record_read_as_one(t) else { return };
    // A field to read instead, named after the value when it is a name.
    let field = match (&value, receiver_spelling(e)) {
        (Type::Record(fields), Some(name)) if matches!(unwrap_ref(t), Type::Record(_)) => {
            fields.iter().find(|(f, _)| !f.starts_with("__")).map(|(f, _)| format!(" (`{name}.{f}`)"))
        }
        _ => None,
    };
    // A gate read whose first output is a record names that output.
    let first = match unwrap_ref(t) {
        Type::Outputs(fields) => format!(" (its first output `{}`)", fields[0].0),
        _ => String::new(),
    };
    // A record a bounded type parameter stands for in this check: a field of
    // it is no field of the bound's other members, so name the bound to
    // narrow instead.
    let narrow = ctx
        .bounded_type_params
        .iter()
        .rev()
        .find(|(_, _, now)| crate::types::coerce::type_eq(&unwrap_ref(now), &value))
        .and_then(|(tp, mask, _)| {
            let rest: Vec<String> = mask
                .iter()
                .filter(|m| record_read_as_one(m).is_none())
                .map(crate::analysis::types::type_str)
                .collect();
            (!rest.is_empty()).then(|| {
                format!(
                    "`{tp}` may be that record here, and {}; {}`<{tp}: {}>`",
                    crate::diagnostic::RECORD_INTO_ONE_VALUE,
                    crate::diagnostic::NARROW_THE_BOUND,
                    rest.join(" | ")
                )
            })
        });
    let hint = match narrow {
        Some(narrow) => narrow,
        None => format!(
            "{}, so read one of its fields{}",
            crate::diagnostic::RECORD_INTO_ONE_VALUE,
            field.unwrap_or_default()
        ),
    };
    ctx.emit(
        "WS003",
        format!("expected {expected}, got record {}{first}: {hint}", record_spelling(ctx, e, &value)),
        e.range().clone(),
    );
}

/// The record a value of type `t` is where one value is read: a record of
/// the program's own, or a gate read's first output when that output is
/// one. A gate read gives its first output once, never a field of it.
pub(crate) fn record_read_as_one(t: &Type) -> Option<Type> {
    match unwrap_ref(t) {
        r @ Type::Record(_) => Some(r),
        Type::Outputs(fields) => match unwrap_ref(&fields.first()?.1) {
            r @ Type::Record(_) => Some(r),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a value of type `from` is a record where one value of type `to`
/// goes (see [`record_into_one_value`]).
pub(crate) fn rejected_where_one_value_goes(from: &Type, to: &Type) -> bool {
    record_read_as_one(from)
        .is_some_and(|value| holds_one_value(to) && coerce(&value, &unwrap_ref(to)) != CoerceRule::Mismatch)
}

/// Whether a place of type `t` holds one value: not a record, a tuple, a
/// generic parameter or `any`.
fn holds_one_value(t: &Type) -> bool {
    !matches!(
        unwrap_ref(t),
        Type::Record(_) | Type::Outputs(_) | Type::Tuple(_) | Type::Any | Type::Opaque | Type::Param(_)
    )
}

/// How a message names the record type `t` of `e`: the alias that names
/// exactly it, or its fields, as for a record literal, which names none.
fn record_spelling(ctx: &TypeCheckCtx, e: &Expr, t: &Type) -> String {
    if matches!(e, Expr::RecordLit { .. }) {
        return format!("`{}`", crate::analysis::types::type_str(t));
    }
    let aliases = ctx.scope.type_aliases();
    let mut names: Vec<&String> = aliases.iter().filter(|(_, a)| unwrap_ref(a) == *t).map(|(n, _)| n).collect();
    names.sort();
    match names.first() {
        Some(n) => format!("`{n}`"),
        None => format!("`{}`", crate::analysis::types::type_str(t)),
    }
}

pub(crate) fn coerce_or_emit(ctx: &mut TypeCheckCtx, from: &Type, to: &Type, range: &SourceRange) {
    // `Never` is not a mismatched value type, it is the ABSENCE of one, so
    // "expected int, got never" describes the wrong problem. Route it to the
    // same WS072 message the binding rejection uses. Checked WITHOUT consulting
    // `coerce`, which lets `Never` through an `any` sink under the universal
    // any-coerces-to-anything rule - an `any` PORT still needs a wire, and a
    // `Never` source has none, so `PrintToConsole(noret(1))` must report too.
    if matches!(from, Type::Never) && !matches!(to, Type::Never) {
        crate::typecheck::let_binding::emit_no_value(ctx, None, range);
        return;
    }
    if coerce(from, to) == CoerceRule::Mismatch {
        ctx.emit(
            "WS003",
            format!(
                "expected {}, got {}",
                crate::analysis::types::type_str(to),
                crate::analysis::types::type_str(from),
            ),
            range.clone(),
        );
    }
}

/// WS085: `*` on a reference that has no value behind it to read.
pub(crate) fn emit_deref_not_a_scalar_ref(ctx: &mut TypeCheckCtx, t: &Type, range: &SourceRange) {
    let why = if matches!(t, Type::Brick) {
        "a brick is a value, and nothing reads through one".to_string()
    } else {
        format!("an array or map ref is used directly, with no `*` (`rows[y].push(v)`); this one is {t}")
    };
    ctx.emit("WS085", format!("`*` does not apply here: {why}"), range.clone());
}

#[cfg(test)]
mod tests;

/// The type of a record literal, field by field, later fields overriding
/// earlier ones (a spread supplies every field of its record).
/// `map_field_types` is the stored record type a map literal field may fill.
fn infer_record_lit(
    ctx: &mut TypeCheckCtx,
    fields: &[RecordLitField],
    map_field_types: Option<&[(String, Type)]>,
) -> Type {
    let mut rec_fields: Vec<(String, Type)> = Vec::new();
    for f in fields {
        if let RecordLitField::Named { name, range, .. } | RecordLitField::Shorthand { name, range } = f {
            ctx.reserve_check(name, range);
        }
        match f {
            RecordLitField::Named { name, value, .. } => {
                // A map literal filling a `Map` field of a stored record's literal
                // (see `TypeCheckCtx::record_lit_map_fields`) is checked against
                // that field's type instead of reaching the `MapLit` guard.
                let map_field = match (super::decl::map_init_entries(value), map_field_types) {
                    (Some(entries), Some(types)) => types
                        .iter()
                        .find(|(n, _)| n == name)
                        .and_then(|(_, t)| match unwrap_ref(t) {
                            Type::Map(k, v) => Some((entries, k, v)),
                            _ => None,
                        }),
                    _ => None,
                };
                let ty = match map_field {
                    Some((entries, k, v)) => {
                        super::decl::check_map_literal(ctx, entries, &k, &v);
                        let map = Type::Map(k, v);
                        let r = value.range();
                        ctx.record_expr_type(r, &map);
                        map
                    }
                    None => infer(ctx, value),
                };
                // Override if field already exists (from spread)
                if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                    existing.1 = ty;
                } else {
                    rec_fields.push((name.clone(), ty));
                }
            }
            RecordLitField::Shorthand { name, range } => {
                // `{ x }` is `{ x: x }`: a place reads as its var's value.
                let ty = infer(
                    ctx,
                    &Expr::Ident {
                        name: name.clone(),
                        range: range.clone(),
                    },
                );
                if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                    existing.1 = ty;
                } else {
                    rec_fields.push((name.clone(), ty));
                }
            }
            RecordLitField::Spread { value, .. } => {
                let spread_ty = infer(ctx, value);
                if let Type::Record(spread_fields) | Type::Outputs(spread_fields) = spread_ty {
                    for (fname, fty) in spread_fields {
                        if let Some(existing) =
                            rec_fields.iter_mut().find(|(n, _)| *n == fname)
                        {
                            existing.1 = fty;
                        } else {
                            rec_fields.push((fname, fty));
                        }
                    }
                }
            }
        }
    }
    Type::Record(rec_fields)
}

/// Why `$Component/<name>[/<property>]` names nothing the component gates
/// take, or `None` when it is a component type they offer (and, if given, one
/// of its properties).
fn component_ref_problem(path: &str) -> Option<String> {
    let (name, property) = match path.split_once('/') {
        Some((n, p)) => (n, Some(p)),
        None => (path, None),
    };
    let Some(component) = crate::catalog::component_type(name) else {
        // The same component under another spelling: its class, or its
        // display name.
        let squash = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
        let found = crate::catalog::default_catalog()
            .entries()
            .iter()
            .find(|g| g.component.class == name || squash(&g.component.display_name) == name);
        return Some(match found {
            Some(g) if crate::catalog::component_types().iter().any(|o| o.class == g.component.class) => {
                let o = crate::catalog::component_types().iter().find(|o| o.class == g.component.class)?;
                format!("names a component by its class; write `$Component/{}`", o.name)
            }
            Some(g) if !g.component.player_spawnable => {
                "is not a component type the component gates take: a player can't add it to a brick".to_string()
            }
            Some(_) => "is not a component type the component gates take: their Component Type setting doesn't offer it".to_string(),
            None => "is not a component type the component gates take: no component has that name".to_string(),
        });
    };
    let property = property?;
    let props = crate::catalog::component_properties(component.class);
    if props.iter().any(|p| p.name == property) {
        return None;
    }
    let names: Vec<&str> = props.iter().map(|p| p.name).collect();
    Some(format!("names no property of {}: it has {}", component.name, names.join(", ")))
}
