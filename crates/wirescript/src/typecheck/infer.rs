//! Bidirectional expression typing.
//!
//! `infer(ctx, e)` synthesizes a type for `e` and records it in
//! `ctx.type_of_expr` (the sole recorder). `check(ctx, e, expected)` infers `e`
//! and drives coercion against `expected`, emitting `WS003` on a mismatch. Both
//! are exhaustive over `Expr` — the compiler enforces full coverage of every
//! variant, no fallback.

use crate::ast::{ArrayElem, CallArg, Expr, InterpPart, MatchBody, RecordLitField};
use crate::catalog::calls::find_call;
use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::Type;
use crate::types::coerce::{coerce, widening_join, CoerceRule};
use crate::types::mono::unwrap_ref;

use super::{
    call_param_config_enum, check_args, check_stmt, is_reference_type, op_operand_type,
    output_record_type, resolve_op, resolve_type_expr, sig_of_callspec, target_name,
    type_user_symbol_call, CallSignature, ExecMode, Param, ParamKind, SymbolKind, TypeCheckCtx,
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

/// Node dispatch, exhaustive over every `Expr` variant.
fn infer_node(ctx: &mut TypeCheckCtx, e: &Expr) -> Type {
    match e {
        Expr::IntLit { .. } => Type::Int,
        Expr::AtomLit { .. } => Type::Int,
        Expr::FloatLit { .. } => Type::Float,
        Expr::StringLit { .. } => Type::String,
        Expr::BoolLit { .. } => Type::Bool,
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
            let op_key = if op == "-" { "-u" } else { op.as_str() };
            let unwrapped = op_operand_type(&operand_t);
            let rule = resolve_op(op_key, &[unwrapped]);
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
        Expr::FieldAccess { obj, field, range } => {
            let ot = infer(ctx, obj);
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
                (Type::Rotator, "pitch" | "yaw" | "roll") => Type::Float,
                // An array read yields the element plus a bounds flag, but it is
                // typed as the bare element (see IndexAccess), so by the time the
                // flag is projected - directly or through a `let` - the object is
                // the element type and this would fall through to Any. Lowering
                // already maps these names to the gate's bOutOfBounds port.
                (_, "OutOfBounds" | "bOutOfBounds") => Type::Bool,
                // NOTE (P0-16c, deferred): an unknown field on a scalar (e.g. a
                // typo `c.whatever` on an `int`) silently types `Any` and
                // lowering reads the whole base value. Flagging it here would
                // also break the single-output projection compat (`let f =
                // Foo(); f.result`, where `f` is typed as the bare output but
                // `.result` rides this same lenient passthrough) — the two are
                // indistinguishable without tracking each value's call origin.
                // Catching the typo needs origin-aware field validation, not a
                // blanket scalar reject.
                _ => Type::Any,
            }
        }
        Expr::IndexAccess { obj, index, range } => {
            let ot = unwrap_ref(&infer(ctx, obj));
            infer(ctx, index);
            match &ot {
                Type::Array(inner) => {
                    if ctx.exec_mode() != ExecMode::Exec {
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
                    if ctx.exec_mode() != ExecMode::Exec {
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
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            infer(ctx, scrutinee);
            let mut tys: Vec<Type> = Vec::new();
            for arm in arms {
                if let MatchBody::Expr(expr) = &arm.body {
                    tys.push(infer(ctx, expr));
                }
            }
            if tys.is_empty() {
                Type::Any
            } else if tys
                .iter()
                .all(|t| std::mem::discriminant(t) == std::mem::discriminant(&tys[0]))
            {
                tys[0].clone()
            } else {
                Type::Union(tys)
            }
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
            // Namespace call: ns.foo(args)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Expr::Ident { name: ns_name, .. } = obj.as_ref()
                && ctx.scope.lookup(ns_name).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                let ns_lookup = ctx
                    .namespaces
                    .get(ns_name.as_str())
                    .and_then(|ns_map| ns_map.get(field.as_str()))
                    .map(|info| (info.kind, info.return_type.clone(), info.params.clone()));
                match ns_lookup {
                    Some((kind, ret, params)) => {
                        // Route the call through the shared arg checker —
                        // namespaced calls previously did NO argument
                        // checking at all (silent miscompile on wrong type
                        // or arity). Only `Fn`/`Chip` members are callable;
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
                                        kind: ParamKind::Wire,
                                    })
                                    .collect(),
                                config_gate: None,
                            };
                            check_args(ctx, &sig, args, 0, true, true, fa_range);
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
            // Any array-typed value works (an `array` decl or a `var ids: T[]`),
            // gated on the field actually being an array method.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && (sym.kind == SymbolKind::Array || matches!(unwrap_ref(&sym.ty), Type::Array(_)))
                && let Some(method) = crate::catalog::arrays::array_method(field)
            {
                let elem = match unwrap_ref(&sym.ty) {
                    Type::Array(inner) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
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
            // Map method call: m.get(k), m.set(k, v), m.has(k), etc.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && matches!(unwrap_ref(&sym.ty), Type::Map(_, _))
                && let Some(method) = crate::catalog::maps::map_method(field)
            {
                let (key, value) = match unwrap_ref(&sym.ty) {
                    Type::Map(k, v) => (k.as_ref().clone(), v.as_ref().clone()),
                    _ => (Type::Any, Type::Any),
                };
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
                && ctx.scope.lookup(name).is_none()
                && find_call(name).is_none()
            {
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
            Type::Any
        }
    }
}

pub(crate) fn check(ctx: &mut TypeCheckCtx, e: &Expr, expected: &Type) -> Type {
    let t = infer(ctx, e);
    coerce_or_emit(ctx, &t, expected, e.range());
    t
}

/// The shared WS003 sink (formerly `typecheck::expect_coerce`).
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
