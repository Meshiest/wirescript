//! Bidirectional expression typing.
//!
//! `infer(ctx, e)` synthesizes a type for `e` and records it in
//! `ctx.type_of_expr` (the sole recorder). `check(ctx, e, expected)` infers `e`
//! and drives coercion against `expected`, emitting `WS003` on a mismatch. Both
//! are exhaustive over `Expr` — the compiler enforces full coverage of every
//! variant, no fallback.

use crate::ast::{
    ArrayElem, Block, CallArg, Expr, InterpPart, MatchBody, Pattern, RecordLitField, Stmt,
    VariantPattern,
};
use crate::catalog::calls::find_call;
use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::Type;
use crate::types::coerce::{coerce, widening_join, CoerceRule};
use crate::types::mono::unwrap_ref;

use super::{
    call_param_config_enum, check_args, check_stmt, is_reference_type, op_operand_type,
    output_record_type, resolve_op, resolve_type_expr, sig_of_callspec, target_name,
    type_param_mask, type_user_symbol_call, CallSignature, ExecMode, Param, ParamKind, SymbolInfo,
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
    let base = std::path::Path::new(&*range.file)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let full = if let Some(rel) = path.strip_prefix("./") {
        base.join(rel)
    } else if path.starts_with('/') {
        std::path::PathBuf::from(path)
    } else {
        base.join(path)
    };
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
    let t = infer_node(ctx, e);
    let r = e.range();
    ctx.type_of_expr
        .insert((r.file.clone(), r.start.offset, r.end.offset), t.clone());
    t
}

/// An array or map is already a reference (it wires `ArrayVarRef`/`MapVarRef`),
/// so a `Ref` around one is redundant — and `*T[]` is actively broken (it
/// silently drops writes). Collapse `Ref(Array)`/`Ref(Map)` to the container;
/// a scalar `*T` (a ref to a mutable cell) is meaningful and left untouched.
fn collapse_container_ref(t: Type) -> Type {
    match t {
        Type::Ref(inner) if matches!(&*inner, Type::Array(_) | Type::Map(_, _)) => *inner,
        other => other,
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
            if let Expr::Ident { name: ns, .. } = obj.as_ref()
                && ctx.scope.lookup(ns).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                return ctx
                    .namespaces
                    .get(ns)
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
                Type::Record(fields) => field_ty(&fields),
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
/// site exists (see `scoped_const_placeholders`'s doc comment). Evaluating
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
    let const_read =
        !mutates && matches!(obj, Expr::Ident { name, .. } if ctx.const_declared.contains(name));
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
    // The in-scope alias snapshot is an O(scope-size) frame scan, so build it
    // LAZILY - only when a non-primitive annotation is actually resolved. A
    // param-less mod (or one with only primitive params, the common case)
    // resolves nothing and never pays for it, which is what keeps the whole scan
    // from going O(N^2) again on a deep chain of primitive-typed mods.
    let alias_map: std::cell::RefCell<Option<crate::collections::HashMap<String, Type>>> =
        std::cell::RefCell::new(None);
    let resolve = |te: &crate::ast::TypeExpr| -> Type {
        if let crate::ast::TypeExpr::Name { name, .. } = te
            && let Some(prim) = crate::types::resolve::primitive(name)
        {
            return prim;
        }
        let mut slot = alias_map.borrow_mut();
        let aliases = slot.get_or_insert_with(|| ctx.scope.type_aliases());
        let cx = crate::types::resolve::ResolveCtx {
            params: &[],
            type_aliases: aliases,
            generic_aliases: &ctx.generic_type_aliases,
        };
        crate::types::resolve::resolve_type(te, &cx, &mut Vec::new())
    };
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
pub(super) fn mod_is_exec_requiring(ctx: &mut TypeCheckCtx, name: &str) -> bool {
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
                    Expr::Ident { name, .. } => {
                        if let Some(c) = find_call(name) {
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
    let (is_enum, type_params, variant_def) = match ctx.enum_defs.get(enum_name) {
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
    .map(str::to_string)
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
            let params: Vec<Param> = payload_types
                .iter()
                .enumerate()
                .map(|(i, ty)| Param {
                    name: i.to_string(),
                    ty: resolve_payload_param_type(ctx, ty, &type_params),
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
            if type_params.is_empty() {
                return Some(enum_ty);
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
    match p {
        Pattern::Wildcard(r) => r,
        Pattern::Binding { range, .. } => range,
        Pattern::Variant { range, .. } => range,
    }
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
fn bind_sub_as_any(ctx: &mut TypeCheckCtx, sub: &VariantPattern) {
    match sub {
        VariantPattern::Unit => {}
        VariantPattern::Positional(pats) => {
            for p in pats {
                check_match_pattern(ctx, p, &Type::Any);
            }
        }
        VariantPattern::Named { fields, .. } => {
            for (_, p) in fields {
                check_match_pattern(ctx, p, &Type::Any);
            }
        }
    }
}

/// Validate an arm pattern against the type it matches and bind its captures
/// into the current scope. Keeps the SAME unit-variant reinterpretation the
/// usefulness engine (`patterns::head_variant_name`) uses: a bare identifier
/// naming a unit variant is a variant test that binds nothing, while any
/// other bare identifier captures the whole matched value. Emits WS060 for an
/// unknown variant and WS065 for a payload whose bracket form or arity does
/// not match the variant.
pub(super) fn check_match_pattern(ctx: &mut TypeCheckCtx, pat: &Pattern, matched: &Type) {
    use crate::typecheck::enums::Payload;
    match pat {
        Pattern::Wildcard(_) => {}
        Pattern::Binding { name, range } => {
            if let Type::Enum { name: en, .. } = matched
                && ctx.enum_defs.get(en).is_some_and(|edef| {
                    edef.variants
                        .iter()
                        .any(|v| &v.name == name && matches!(v.payload, Payload::Unit))
                })
            {
                return;
            }
            ctx.scope.declare(
                name,
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: name.clone(),
                    ty: matched.clone(),
                    decl_range: range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        Pattern::Variant { variant, sub, range } => {
            let Type::Enum { name: en, args } = matched else {
                bind_sub_as_any(ctx, sub);
                return;
            };
            let edef = match ctx.enum_defs.get(en) {
                Some(d) => d.clone(),
                None => {
                    bind_sub_as_any(ctx, sub);
                    return;
                }
            };
            let Some(vdef) = edef.variants.iter().find(|v| &v.name == variant).cloned() else {
                ctx.emit(
                    "WS060",
                    format!("enum `{en}` has no variant `{variant}`"),
                    range.clone(),
                );
                bind_sub_as_any(ctx, sub);
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
                        check_match_pattern(ctx, p, &fty);
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
                        check_match_pattern(ctx, fpat, &fty);
                    }
                }
                _ => {
                    ctx.emit("WS065", ws065_pattern_form_wrong(variant, &vdef.payload), range.clone());
                    bind_sub_as_any(ctx, sub);
                }
            }
        }
    }
}

/// Type an arm body, returning its value type for the arm-result join. An
/// expression arm contributes its inferred type; a block arm runs for its
/// side effects (statement errors surface) but contributes no value type.
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

/// Node dispatch, exhaustive over every `Expr` variant.
fn infer_node(ctx: &mut TypeCheckCtx, e: &Expr) -> Type {
    // The `check`-supplied expected type applies to THIS node only; take it
    // once here so a nested inference (an operand, an arg) never reads a stale
    // hint. Only generic enum construction consumes it (`expected`, below).
    let expected = ctx.expected_ty.take();
    match e {
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
        Expr::InterpLit { parts, .. } => {
            // Match legacy exactly: unwrap a leading `Ref` off the part's type
            // before the string-coercion check, so an interpolated var ref
            // (`${&x}`) coerces through its inner type rather than tripping ref
            // invariance. (Not routed through `check`, which skips the unwrap.)
            for p in parts {
                if let InterpPart::Expr(expr) = p {
                    let t = unwrap_ref(&infer(ctx, expr));
                    coerce_or_emit(ctx, &t, &Type::String, expr.range());
                }
            }
            Type::String
        }
        Expr::AssetRef { .. } => {
            // An external asset reference (`$Type/Name`) is an object/class
            // reference — typed `entity` so it can be compared against entity
            // values (e.g. `weapon == $BRItemBase/Weapon_Pickaxe`) and passed
            // into object/class gate ports (which accept `any`/entity anyway).
            // Validation against the asset catalog happens in analysis.
            Type::Entity
        }
        Expr::PrefabRef { path, range } => {
            // A prefab file reference flows into a `bundle_path_ref` gate
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
            // flows into a `bundle_path_ref` gate property; typed
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
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                return Type::Any;
            };
            match sym.kind {
                SymbolKind::Event => Type::Exec,
                SymbolKind::Fn | SymbolKind::Chip => Type::Any,
                SymbolKind::Var => {
                    let is_exec = ctx.exec_mode() == ExecMode::Exec;
                    ctx.var_read_contexts
                        .insert((range.file.clone(), range.start.offset), is_exec);
                    unwrap_ref(&sym.ty)
                }
                SymbolKind::Array => sym.ty.clone(),
                _ => sym.ty.clone(),
            }
        }
        Expr::Deref { operand, range } => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS006",
                    format!(
                        "'*{}' deref requires exec context — use .Value for pure reads",
                        target_name(operand).unwrap_or("<expr>".into())
                    ),
                    range.clone(),
                );
            }
            let t = infer(ctx, operand);
            match t {
                Type::Ref(inner) => *inner,
                Type::Any => Type::Any,
                other => {
                    ctx.emit(
                        "WS003",
                        format!("expected ref T, got {:?}", other),
                        range.clone(),
                    );
                    Type::Any
                }
            }
        }
        Expr::RefOf { operand, range } => {
            if !super::is_ref_able(ctx, operand) {
                ctx.emit(
                    "WS008",
                    "cannot take `&` of a non-reference — `&`/`ref` needs a variable, ref parameter, \
                     or array/map element, not a temporary value",
                    range.clone(),
                );
            }
            let t = infer(ctx, operand);
            // An array/map is already a reference, so `&arr` is a no-op — never
            // `*T[]` (which is redundant and drops writes).
            if t.is_reference_backed() {
                t
            } else {
                Type::Ref(Box::new(t))
            }
        }
        Expr::UnOp { op, operand, range } => {
            let operand_t = infer(ctx, operand);
            let unwrapped = op_operand_type(&operand_t);
            // `resolve_op` maps unary `-` to the table's `-u` key internally.
            let rule = resolve_op(op.as_str(), &[unwrapped]);
            if let Some(r) = rule {
                let result = r.result.clone();
                ctx.op_resolutions.insert(
                    (range.file.clone(), range.start.offset, range.end.offset),
                    r.clone(),
                );
                result
            } else {
                let code = if matches!(op.as_str(), "&" | "|" | "^" | "~" | "<<" | ">>") {
                    "WS011"
                } else {
                    "WS004"
                };
                ctx.emit(
                    code,
                    format!("no overload for '{op}' on {:?}", operand_t),
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
            let lt = infer(ctx, left);
            let rt = infer(ctx, right);
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
                    let code = if matches!(op.as_str(), "&" | "|" | "^" | "<<" | ">>") {
                        "WS011"
                    } else {
                        "WS004"
                    };
                    ctx.emit(
                        code,
                        format!("no overload for '{op}' on {:?}, {:?}", lt, rt),
                        range.clone(),
                    );
                    Type::Any
                }
            }
        }
        Expr::FieldAccess { obj, field, range } => {
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
                        "unknown identifier '{ns_name}' — no namespace, variable, or \
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
            if let Expr::Ident { name: ns_name, .. } = obj.as_ref()
                && ctx.scope.lookup(ns_name).map(|s| s.kind) == Some(SymbolKind::Namespace)
                && let Some(ty) = ctx
                    .namespaces
                    .get(ns_name.as_str())
                    .and_then(|m| m.get(field.as_str()))
                    .and_then(|info| info.value_type.clone())
            {
                return ty;
            }
            // A namespace member that is missing (`L.nope`) or is not a readable
            // value (`L.total`, where `total` is an `out`/`in`/buffer with no
            // value type, or a bare callable used as a value): error, matching
            // the plain/selective forms, instead of falling through to `any` and
            // lowering to `_Unsupported`.
            if let Expr::Ident { name: ns_name, .. } = obj.as_ref()
                && ctx.scope.lookup(ns_name).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                let present = ctx
                    .namespaces
                    .get(ns_name.as_str())
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
            if let Expr::Ident { name: enum_name, .. } = obj.as_ref() {
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
            // declared var type directly.
            if (field == "Value" || field == "prev")
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && let Type::Ref(inner) = &sym.ty
            {
                return inner.as_ref().clone();
            }
            // `x.Value` on a var/ref reads through to the inner type. A record
            // with its own `Value` field (a multi-output gate result such as
            // `a.pop()` → `{ Value, IsEmpty }`) must project that field instead,
            // or `.Value` yields the whole record and every use of it mistypes.
            if (field == "value" || field == "Value") && !matches!(ot, Type::Record(_)) {
                return unwrap_ref(&ot);
            }
            // `.exec` names the exec output of an event/exec-producing expression.
            // A data-carrying event (`GlobalCustomEvent(c) -> (n)`) types as a
            // record with its exec output FIRST, so `.exec` selects that field
            // explicitly and lets the event compose into `Union(...)`; on a bare
            // exec value it is the identity.
            if field == "exec"
                && (matches!(ot, Type::Exec)
                    || matches!(&ot, Type::Record(fs) if matches!(fs.first(), Some((_, Type::Exec)))))
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
            {
                let base = unwrap_ref(&ot);
                if let Type::Array(elem) = &base
                    && let Type::Record(fields) = elem.as_ref()
                    && let Some((_, t)) = fields.iter().find(|(k, _)| k == field)
                {
                    return Type::Array(Box::new(t.clone()));
                }
                if let Type::Map(k, val) = &base
                    && let Type::Record(fields) = val.as_ref()
                    && let Some((_, t)) = fields.iter().find(|(kk, _)| kk == field)
                {
                    return Type::Map(k.clone(), Box::new(t.clone()));
                }
            }
            if let Type::Record(fields) = &ot {
                if let Some((_, t)) = fields.iter().find(|(k, _)| k == field) {
                    return t.clone();
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
            match (&ot, field.as_str()) {
                (Type::Vector, "x" | "X" | "y" | "Y" | "z" | "Z") => Type::Float,
                (Type::Color, "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A") => Type::Float,
                (Type::Rotator, "pitch" | "Pitch" | "yaw" | "Yaw" | "roll" | "Roll") => {
                    Type::Float
                }
                (Type::Quat, "x" | "X" | "y" | "Y" | "z" | "Z" | "w" | "W") => Type::Float,
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
                _ => Type::Any,
            }
        }
        Expr::IndexAccess { obj, index, range } => {
            let ot = unwrap_ref(&infer(ctx, obj));
            infer(ctx, index);
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
                _ => Type::Any,
            }
        }
        Expr::TuplePick { obj, index, range } => {
            let ot = infer(ctx, obj);
            match &ot {
                Type::Tuple(fields) => fields.get(*index).cloned().unwrap_or_else(|| {
                    ctx.emit(
                        "WS010",
                        format!("tuple index .{index} out of range"),
                        range.clone(),
                    );
                    Type::Any
                }),
                Type::Record(fields) => fields
                    .get(*index)
                    .map(|(_, t)| t.clone())
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
            infer(ctx, cond);
            let tt = infer(ctx, then_branch);
            let et = infer(ctx, else_branch);
            // A reference (a var ref, or a `zone`/`teleport` component ref) can't
            // flow through the Select gate an if-expr compiles to — Select routes
            // a value, not a reference. Flag whichever branch is a reference.
            for (br, bty) in [(then_branch, &tt), (else_branch, &et)] {
                if is_reference_type(bty) {
                    ctx.emit(
                        "WS031",
                        format!(
                            "'{}' is a reference and can't be used in an if-then-else — a Select \
                             routes a value, not a reference; wire or reroute it instead",
                            crate::analysis::types::type_str(bty),
                        ),
                        br.range().clone(),
                    );
                }
            }
            match widening_join(&tt, &et) {
                Some(j) => j,
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
            let scrut_ty = unwrap_ref(&infer(ctx, scrutinee));
            if !matches!(scrut_ty, Type::Enum { .. }) {
                ctx.emit(
                    "WS066",
                    format!(
                        "`match` requires an enum scrutinee, but this is {}",
                        crate::analysis::types::type_str(&scrut_ty)
                    ),
                    range.clone(),
                );
                // Still walk each arm body so its own errors surface; there is
                // no enum to type captures against, so they bind as `any`.
                for arm in arms {
                    ctx.push_scope();
                    check_match_pattern(ctx, &arm.pattern, &scrut_ty);
                    infer_match_body(ctx, &arm.body);
                    ctx.pop_scope();
                }
                return Type::Any;
            }

            let mut tys: Vec<Type> = Vec::new();
            for arm in arms {
                ctx.push_scope();
                check_match_pattern(ctx, &arm.pattern, &scrut_ty);
                if let Some(t) = infer_match_body(ctx, &arm.body) {
                    tys.push(t);
                }
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
            let usefulness =
                crate::typecheck::patterns::analyze(&ctx.enum_defs, &scrut_ty, &arm_patterns);
            for witness in &usefulness.missing {
                ctx.emit(
                    "WS054",
                    format!(
                        "non-exhaustive match: `{}` is not covered",
                        render_pattern(&witness.0)
                    ),
                    range.clone(),
                );
            }
            for &idx in &usefulness.unreachable_arms {
                ctx.warn(
                    "WS061",
                    "unreachable match arm - an earlier arm already covers every value it matches"
                        .to_string(),
                    arms[idx].range.clone(),
                );
            }
            // Lint (WS067): a bare variant name for a variant that HAS a payload
            // (`Circle` instead of `Circle(_)`) parses as a catch-all capture, so
            // it silently swallows every value and can make later arms dead. A
            // unit variant is correctly written bare, so only a payload variant of
            // the scrutinee's own enum is flagged. Suspects are collected under the
            // `enum_defs` borrow, then warned after it drops.
            let mut bare_payload_variants: Vec<(String, SourceRange)> = Vec::new();
            if let Type::Enum { name: enum_name, .. } = &scrut_ty
                && let Some(edef) = ctx.enum_defs.get(enum_name)
            {
                for arm in arms {
                    if let Pattern::Binding {
                        name: bname,
                        range: brange,
                    } = &arm.pattern
                        && edef.variants.iter().any(|v| {
                            &v.name == bname
                                && !matches!(v.payload, crate::typecheck::enums::Payload::Unit)
                        })
                    {
                        bare_payload_variants.push((bname.clone(), brange.clone()));
                    }
                }
            }
            for (bname, brange) in bare_payload_variants {
                ctx.warn(
                    "WS067",
                    format!(
                        "`{bname}` names a variant that has a payload, but written bare it binds \
                         the whole value like a catch-all (which can leave later arms \
                         unreachable). Did you mean `{bname}(_)`?"
                    ),
                    brange,
                );
            }
            result
        }
        Expr::RecordLit { fields, .. } => {
            let mut rec_fields: Vec<(String, Type)> = Vec::new();
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        let ty = infer(ctx, value);
                        // Override if field already exists (from spread)
                        if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                            existing.1 = ty;
                        } else {
                            rec_fields.push((name.clone(), ty));
                        }
                    }
                    RecordLitField::Shorthand { name, .. } => {
                        // `{ arr }` captures the array/map var by its container
                        // ref, not `*T[]` (a var's symbol type is `Ref(T)`, but
                        // an array/map is already a ref). A scalar `*int` stays.
                        let ty = ctx
                            .scope
                            .lookup(name)
                            .map(|s| collapse_container_ref(s.ty.clone()))
                            .unwrap_or(Type::Any);
                        if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                            existing.1 = ty;
                        } else {
                            rec_fields.push((name.clone(), ty));
                        }
                    }
                    RecordLitField::Spread { value, .. } => {
                        let spread_ty = infer(ctx, value);
                        if let Type::Record(spread_fields) = spread_ty {
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
        Expr::Array { elements, .. } => {
            // Type each element so it lands in the type map; the array's element
            // type is taken from the first element. A spread contributes its
            // source array's element type, a plain item its value type. Whether
            // the elements must be constant literals is enforced at the
            // declaration site (top level) — not here, since the same literal is
            // valid with runtime elements in an exec-context assignment.
            let mut elem = Type::Any;
            for (i, el) in elements.iter().enumerate() {
                let t = unwrap_ref(&infer(ctx, el.expr()));
                let et = match el {
                    ArrayElem::Spread(_) => match t {
                        Type::Array(inner) => *inner,
                        other => other,
                    },
                    ArrayElem::Item(_) => t,
                };
                if i == 0 {
                    elem = et;
                } else if !matches!(elem, Type::Any)
                    && coerce(&et, &elem) == CoerceRule::Mismatch
                {
                    // A literal's elements must be homogeneous — every element
                    // has to coerce into the first element's type, because they
                    // all share one backing array-variant gate. Without this a
                    // `[1, "hello", 2]` typed the array `int[]` from element 0
                    // and pushed the string constant into the int variant with
                    // no diagnostic. (Top-level `var` initializers are checked
                    // against the DECLARED element type in
                    // `check_top_level_array_init`; this covers exec-context
                    // assignments, which reach `infer` + a whole-array coerce
                    // that only inspects element 0.)
                    ctx.emit(
                        "WS003",
                        format!(
                            "array element: expected {}, got {}",
                            crate::analysis::types::type_str(&elem),
                            crate::analysis::types::type_str(&et),
                        ),
                        el.expr().range().clone(),
                    );
                }
            }
            Type::Array(Box::new(elem))
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
                && let Expr::Ident { name: enum_name, .. } = obj.as_ref()
            {
                match resolve_variant_for_construction(ctx, enum_name, variant, fa_range) {
                    VariantResolution::NotConstruction => {}
                    VariantResolution::UnknownVariant(enum_ty) => {
                        infer_record_field_values(ctx, fields);
                        return enum_ty;
                    }
                    VariantResolution::Resolved(enum_ty, vdef, type_params) => {
                        let annotation = expected;
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
                            .map(|(n, ty)| (n.clone(), resolve_payload_param_type(ctx, ty, &type_params)))
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
                        return Type::Enum { name: enum_name.clone(), args };
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
                Expr::Ident { name, .. } => find_call(name),
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
            // Side-effect: typecheck every arg, except enum-typed config args.
            // `positional_arg_types` mirrors the positional args in order —
            // used below to infer a generic user mod/chip call's type params
            // from its arguments (builtin `find_call`s ignore it).
            let mut pos = 0usize;
            let mut positional_arg_types: Vec<Type> = Vec::new();
            for a in args {
                let is_config_enum = arg_spec.is_some_and(|c| match a {
                    CallArg::Named { name, .. } => match c.params.iter().find(|p| p.name == name) {
                        Some(p) => call_param_config_enum(c, p).is_some(),
                        // Not a declared param: a data-driven config attribute.
                        // Skip inference when it names an enum config field (the
                        // value is a bare member name, not a variable).
                        None => crate::catalog::config_field_enum_type(c.gate_class, name).is_some(),
                    },
                    CallArg::Positional(_) => c
                        .params
                        .get(pos_base + pos)
                        .is_some_and(|p| call_param_config_enum(c, p).is_some()),
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
                        let t = infer(ctx, v);
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
            // Resolve callee if it's a plain identifier.
            if let Expr::Ident { name, range } = callee.as_ref() {
                if let Some(c) = find_call(name) {
                    // Explicit type arguments on a builtin are ignored: a
                    // builtin's result type is derived from its arguments (the
                    // masked-generic engine), not pinned by an explicit `<...>`.
                    // Warn rather than silently dropping them.
                    if !type_args.is_empty() {
                        ctx.diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS037".into(),
                            message: format!(
                                "explicit type arguments are ignored on builtin `{name}` — its \
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
                            ctx.emit("WS007", format!("exec call '{name}' outside an exec context (pass exec = ... to override)"), range.clone());
                        }
                    }
                    // Change/Edge detectors watch a single wire VALUE. A reference
                    // or container arg (a `*T`/zone/teleport ref, or a map/array)
                    // has no value to watch — the detector observes the reference,
                    // which never "changes" — so `Change(m)` / `Changed(zone)`
                    // type-checked as `any` and lowered to an invalid, dead gate.
                    if (c.gate_class.contains("ChangeDetector")
                        || c.gate_class.contains("EdgeDetector"))
                        && let Some(CallArg::Positional(arg)) = args.first()
                    {
                        let at = infer(ctx, arg);
                        if is_reference_type(&at)
                            || matches!(unwrap_ref(&at), Type::Map(_, _) | Type::Array(_))
                        {
                            ctx.emit(
                                "WS059",
                                format!(
                                    "`{name}` can't watch a {} — a change/edge detector \
                                     observes a single wire value, and a reference or container \
                                     (map, array, `*T`, zone, teleport) has none",
                                    crate::analysis::types::type_str(&unwrap_ref(&at))
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
                                let ut = unwrap_ref(t);
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
                    if !c.outputs.is_empty() {
                        return output_record_type(ctx, c, args, range);
                    }
                    if c.exec {
                        return Type::Any;
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
                    return Type::Record(fields);
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
            // `docs/wirescript/enums.md`). Recognized only when `Enum` is a
            // known, unshadowed enum type AND has no variant literally named
            // `FromInt` (a real variant of that name still wins, constructed by
            // the block below). Placed BEFORE that block because
            // `resolve_variant_for_construction` would otherwise emit WS060 for
            // the non-variant name `FromInt`. Takes exactly one int argument;
            // result types as the enum.
            if let Expr::FieldAccess { obj, field, range: fa_range } = callee.as_ref()
                && field == "FromInt"
                && let Expr::Ident { name: enum_name, .. } = obj.as_ref()
                && !matches!(
                    ctx.scope.lookup(enum_name).map(|s| s.kind),
                    Some(k) if k != SymbolKind::Type
                )
                && let Some(type_params) = ctx
                    .enum_defs
                    .get(enum_name)
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
                return Type::Enum { name: enum_name.clone(), args: args_ty };
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
                && let Expr::Ident { name: enum_name, .. } = obj.as_ref()
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
                let ns_lookup = ctx
                    .namespaces
                    .get(ns_name.as_str())
                    .and_then(|ns_map| ns_map.get(field.as_str()))
                    .map(|info| (info.kind, info.return_type.clone(), info.params.clone()));
                match ns_lookup {
                    Some((kind, ret, params)) => {
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
                        }
                        match ret {
                            Some(ret) => return resolve_type_expr(ctx, &ret),
                            None => return Type::Any,
                        }
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
                && let Some(Type::Array(inner)) = container_receiver_type(ctx, obj)
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
                // sortMultiple is a true variadic (empty params → opt out of
                // arity, else "expects at most 0, got N" would wrongly fire).
                // Every other method arity-checks normally.
                let check_arity = field.as_str() != "sortMultiple";
                check_args(
                    ctx,
                    &method.signature(&elem),
                    args,
                    0,
                    check_arity,
                    // sortMultiple's variadic list can't enumerate its named
                    // args either, so its named check follows arity.
                    check_arity,
                    fa_range,
                );
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
                && let Some(Type::Map(k, v)) = container_receiver_type(ctx, obj)
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
                check_args(
                    ctx,
                    &method.signature(&key, &value),
                    args,
                    0,
                    /*check_arity=*/ true,
                    /*check_named=*/ true,
                    fa_range,
                );
                return crate::catalog::maps::map_return_type(field, &key, &value)
                    .unwrap_or(Type::Any);
            }
            // Receiver method call: entity.SetLocation(pos)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(c) = find_call(field)
                && c.receiver.is_some()
            {
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
                return Type::Any;
            }
            // User `self`-receiver method call: `v.dist(o)` where `dist` is a
            // user mod/chip whose first parameter is named `self`. Desugars to
            // `dist(v, o)` — bind the receiver as positional arg 0 and type it
            // exactly like the plain `dist(v, o)` call would (generics inferred
            // through the receiver too). Placed AFTER the builtin-receiver case
            // so a builtin receiver-method of the same name wins (a user
            // self-mod shadowing one is rejected as WS035 at its declaration).
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(sym) = ctx.scope.lookup(field).cloned()
                && sym.kind == SymbolKind::Chip
                && sym
                    .signature
                    .as_ref()
                    .is_some_and(|s| s.params.first().is_some_and(|p| p.name == "self"))
            {
                let recv_ty = infer(ctx, obj);
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
                return type_user_symbol_call(
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
                        "unknown identifier '{name}' in call `{name}.{field}(...)` — \
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
                && ctx.namespaces.contains_key(name.as_str())
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
                && ctx
                    .scope
                    .lookup(field)
                    .is_some_and(|s| s.kind == SymbolKind::Chip)
            {
                // Keep the receiver's type in the map (hover/goto) even though
                // the call itself doesn't resolve to a value.
                infer(ctx, obj);
                ctx.emit(
                    "WS036",
                    format!(
                        "`{field}` is not a receiver method — its first parameter is not \
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
                        "`{field}` takes no receiver — it acts on its own brick, not a \
                         receiver object. Call it as `{field}(…)` directly, not \
                         `x.{field}(…)`."
                    ),
                    fa_range.clone(),
                );
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

pub(crate) fn check(ctx: &mut TypeCheckCtx, e: &Expr, expected: &Type) -> Type {
    // `null` is polymorphic: it takes the expected type and lowers to that type's
    // zero/default. Resolve it here (before the generic infer+coerce), recording
    // the resolved type so lowering emits the right default literal.
    if let Expr::NullLit { .. } = e {
        return check_null(ctx, e, expected);
    }
    // Push a record type into a record literal so each field value is CHECKED
    // against its target type (a `null` field resolves to it) rather than
    // inferred blind. Guarded to a plain, exactly-matching set of named fields;
    // any spread / shorthand / arity mismatch defers to the infer+coerce below,
    // which reports the structural error unchanged.
    if let Expr::RecordLit { fields, .. } = e
        && let Type::Record(ftypes) = unwrap_ref(expected)
        && let Some(named) = fields
            .iter()
            .map(|f| match f {
                RecordLitField::Named { name, value, .. } => Some((name.clone(), value)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
        && named.len() == ftypes.len()
        && ftypes
            .iter()
            .all(|(fname, _)| named.iter().any(|(k, _)| k == fname))
    {
        for (fname, ftype) in &ftypes {
            let val = named.iter().find(|(k, _)| k == fname).map(|(_, v)| *v).unwrap();
            check(ctx, val, ftype);
        }
        let r = e.range();
        let rec = Type::Record(ftypes);
        ctx.type_of_expr
            .insert((r.file.clone(), r.start.offset, r.end.offset), rec.clone());
        return rec;
    }
    // Push the expected type down for the single node `infer` is about to type,
    // so a generic enum construction (`n: Option<int> = None`) can take its `T`
    // from the annotation. `infer_node` `take()`s it immediately, so it never
    // reaches a nested sub-expression; restore the prior hint for a `check`
    // running inside another `check`.
    let prev = ctx.expected_ty.replace(expected.clone());
    let t = infer(ctx, e);
    ctx.expected_ty = prev;
    coerce_or_emit(ctx, &t, expected, e.range());
    t
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
                "`null` has no value for type `{}` — it is only valid for a number, bool, \
                 string, vector/rotator/color, or an entity/character/controller",
                crate::analysis::type_str(&t)
            ),
            e.range().clone(),
        );
    }
    let r = e.range();
    ctx.type_of_expr
        .insert((r.file.clone(), r.start.offset, r.end.offset), t.clone());
    t
}

/// The shared WS003 sink.
pub(crate) fn coerce_or_emit(ctx: &mut TypeCheckCtx, from: &Type, to: &Type, range: &SourceRange) {
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

#[cfg(test)]
mod tests;
