//! Declaring and checking one `let`: its annotation, its bindings, and the
//! single-output alias it may carry, and which record fields hold ref values.

use super::*;
use crate::analysis::types::{type_expr_str, type_str};
use crate::ir::RefKind;
use crate::types::mono::unwrap_ref;

/// The element types of `t` as a tuple, by position. A tuple literal is a
/// record keyed by index, so `Record([("0", T0), ("1", T1)])` is
/// `Tuple([T0, T1])`. A multi-output mod's result is a name-keyed record
/// (`output_record_type` in `typecheck::call`) in the signature's declaration
/// order, which is what a positional pattern means, so it reads the same way;
/// field names are ignored.
fn as_tuple_fields(t: &Type) -> Option<Vec<Type>> {
    match t {
        Type::Tuple(fields) => Some(fields.clone()),
        Type::Record(fields) | Type::Outputs(fields) => {
            Some(fields.iter().map(|(_, ft)| ft.clone()).collect())
        }
        _ => None,
    }
}

/// Which `*T` fields of a record-typed expression carry a ref VALUE on a wire
/// rather than naming storage (a PLACE). `None` wherever it is used means every
/// field is a place. This one classification serves every route a record
/// takes (a literal, a `let` and its projections, a tuple pick, a call result,
/// a namespace member), both for typing a projection (`projection_is_place`)
/// and for building a literal from other records.
#[derive(Clone, Debug)]
pub(crate) enum LetRefValues {
    /// A map or array read, a call's result, event data: every field arrived
    /// on a wire.
    All,
    /// A record literal: the dotted field paths whose initializer names no
    /// storage (`p: &a`, `n: 1`). Never empty and never the empty path, which
    /// `normalized` turns into `None` and `All`.
    Fields(HashSet<String>),
}

impl LetRefValues {
    /// Whether the field at `path` (its segments below the binding) is a value.
    pub(crate) fn covers(&self, path: &[String]) -> bool {
        match self {
            Self::All => true,
            Self::Fields(set) => {
                let joined = path.join(".");
                set.iter().any(|p| {
                    p.is_empty()
                        || *p == joined
                        || joined
                            .strip_prefix(p.as_str())
                            .is_some_and(|r| r.starts_with('.'))
                })
            }
        }
    }

    /// The classification of the record at `path` below this one.
    fn below(&self, path: &[String]) -> Option<Self> {
        if path.is_empty() {
            return normalized(self.clone());
        }
        if self.covers(path) {
            return Some(Self::All);
        }
        let Self::Fields(set) = self else {
            return Some(Self::All);
        };
        let prefix = format!("{}.", path.join("."));
        normalized(Self::Fields(
            set.iter()
                .filter_map(|p| p.strip_prefix(prefix.as_str()).map(str::to_string))
                .collect(),
        ))
    }
}

/// A `let` name's key in `TypeCheckCtx::let_ref_values`: its declaration's
/// `(file, offset)` and the name, since a destructuring pattern declares
/// several names at one range.
pub(crate) type LetKey = (Arc<str>, usize, String);

pub(super) fn let_key(range: &SourceRange, name: &str) -> LetKey {
    (
        std::sync::Arc::clone(&range.file),
        range.start.offset,
        name.to_string(),
    )
}

fn normalized(v: LetRefValues) -> Option<LetRefValues> {
    match v {
        LetRefValues::Fields(set) if set.is_empty() => None,
        LetRefValues::Fields(set) if set.contains("") => Some(LetRefValues::All),
        other => Some(other),
    }
}

/// Record, for `let <name> = value`, which of its ref fields are values. A
/// binding with no entry bundles places, like the `{ cpsr, skip }` idiom.
pub(super) fn record_let_ref_values(ctx: &mut TypeCheckCtx<'_>, b: &LetBinding, value: &Expr) {
    let LetBinding::Ident { name, range } = b else {
        return;
    };
    let v = ref_values_of(ctx, value);
    set_let_ref_values(ctx, let_key(range, name), v);
}

fn set_let_ref_values(ctx: &mut TypeCheckCtx<'_>, key: LetKey, v: Option<LetRefValues>) {
    match v {
        Some(v) => {
            ctx.let_ref_values.insert(key, v);
        }
        None => {
            ctx.let_ref_values.remove(&key);
        }
    }
}

/// Which fields of the record `value` evaluates to are ref values.
pub(crate) fn ref_values_of(ctx: &TypeCheckCtx<'_>, value: &Expr) -> Option<LetRefValues> {
    match value {
        Expr::RecordLit { fields, .. } => {
            let mut set = HashSet::default();
            collect_value_fields(ctx, fields, "", &mut set);
            normalized(LetRefValues::Fields(set))
        }
        Expr::Ident { .. } | Expr::FieldAccess { .. } | Expr::TuplePick { .. } => {
            let (root, segs) = super::infer::flatten_access(value);
            ref_values_at(ctx, root, &segs)
        }
        _ => Some(LetRefValues::All),
    }
}

/// Which fields of `root.segs` are ref values, for a `root` expression and the
/// field path projected off it. A call's result is a value whatever the callee
/// returned, as a chip's output pins are: a place it returns leaves as that
/// place's ref.
pub(crate) fn ref_values_at(
    ctx: &TypeCheckCtx<'_>,
    root: &Expr,
    segs: &[String],
) -> Option<LetRefValues> {
    let Expr::Ident { name, range } = root else {
        return Some(LetRefValues::All);
    };
    let sym = ctx.scope.lookup(name)?;
    let whole = segs.is_empty();
    let record = matches!(
        unwrap_ref(&sym.ty),
        Type::Record(_) | Type::Outputs(_) | Type::Tuple(_)
    );
    let of_let =
        |key: &LetKey, rest: &[String]| ctx.let_ref_values.get(key).and_then(|v| v.below(rest));
    match sym.kind {
        // A var holding a ref is a value; any other var (a `*T` parameter is
        // bound as one) names storage, as each field of a record var does.
        SymbolKind::Var if whole && matches!(unwrap_ref(&sym.ty), Type::Ref(_)) => {
            Some(LetRefValues::All)
        }
        SymbolKind::Var => None,
        SymbolKind::Array | SymbolKind::Map if whole => None,
        // A record parameter's fields are places bound to the caller's; a
        // scalar parameter is a value, unless it is a destructured `*T` field.
        SymbolKind::Param if whole && !record && !matches!(&sym.ty, Type::Ref(_)) => {
            Some(LetRefValues::All)
        }
        SymbolKind::Param => None,
        SymbolKind::In if whole && matches!(&sym.ty, Type::Array(_)) => None,
        SymbolKind::LetBinding if !record => Some(LetRefValues::All),
        SymbolKind::LetBinding => of_let(&let_key(&sym.decl_range, name), segs),
        SymbolKind::Namespace => {
            let (member, rest) = segs.split_first()?;
            let info = ctx.ns_members(name, &range.file)?.get(member.as_str())?;
            let record = matches!(
                &info.value_type,
                Some(Type::Record(_) | Type::Outputs(_) | Type::Tuple(_))
            );
            match info.kind {
                SymbolKind::LetBinding if rest.is_empty() && !record => match &info.let_decl {
                    Some(key) if ctx.let_places.contains(key) => None,
                    _ => Some(LetRefValues::All),
                },
                SymbolKind::LetBinding => match &info.let_decl {
                    Some(key) => of_let(key, rest),
                    None => Some(LetRefValues::All),
                },
                SymbolKind::Var
                    if rest.is_empty() && matches!(&info.value_type, Some(Type::Ref(_))) =>
                {
                    Some(LetRefValues::All)
                }
                SymbolKind::Var | SymbolKind::Array | SymbolKind::Map => None,
                _ => Some(LetRefValues::All),
            }
        }
        _ => Some(LetRefValues::All),
    }
}

fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() || name.is_empty() {
        format!("{prefix}{name}")
    } else {
        format!("{prefix}.{name}")
    }
}

fn collect_value_fields(
    ctx: &TypeCheckCtx<'_>,
    fields: &[RecordLitField],
    prefix: &str,
    set: &mut HashSet<String>,
) {
    for f in fields {
        match f {
            RecordLitField::Named { name, value, .. } => {
                let path = join_path(prefix, name);
                match value {
                    Expr::RecordLit { fields: inner, .. } => {
                        collect_value_fields(ctx, inner, &path, set);
                    }
                    _ => merge_field_source(ref_values_of(ctx, value), &path, set),
                }
            }
            RecordLitField::Shorthand { name, range } => merge_field_source(
                ref_values_of(
                    ctx,
                    &Expr::Ident {
                        name: name.clone(),
                        range: range.clone(),
                    },
                ),
                &join_path(prefix, name),
                set,
            ),
            RecordLitField::Spread { value, .. } => {
                merge_field_source(ref_values_of(ctx, value), prefix, set);
            }
        }
    }
}

/// One literal field's source, as `ref_values_of` classified it, filed under
/// `path`: a place adds nothing, a value adds the path, and a record-valued
/// source adds its own value paths below it.
fn merge_field_source(kinds: Option<LetRefValues>, path: &str, set: &mut HashSet<String>) {
    match kinds {
        Some(LetRefValues::All) => {
            set.insert(path.to_string());
        }
        Some(LetRefValues::Fields(inner)) => {
            set.extend(inner.iter().map(|p| join_path(path, p)));
        }
        None => {}
    }
}

/// Record `let f = Foo(...)` -> the sole output's name, when `Foo` is a user
/// chip/mod (plain or namespaced) with exactly one output. A single-output call
/// types as the bare output value, so `f.result` would otherwise be
/// indistinguishable from a typo; see `TypeCheckCtx::single_output_alias`.
/// Also propagates through a plain re-binding (`let g = f`) so an aliased
/// result keeps its projectable name.
pub(super) fn record_single_output_alias(ctx: &mut TypeCheckCtx<'_>, b: &LetBinding, value: &Expr) {
    let LetBinding::Ident { name, .. } = b else {
        return;
    };
    // `Some(entry)` = this binding IS a single-output call result;
    // `entry` names the one legal projection, or `None` if unindexed.
    let entry: Option<Option<String>> = match value {
        // `let g = f`: inherit whatever `f` may be projected by.
        Expr::Ident { name: src, .. } => ctx.single_output_alias.get(src).cloned(),
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident {
                name: callee_name, ..
            } => ctx
                .scope
                .lookup(callee_name)
                .and_then(|s| s.signature.as_ref())
                .filter(|sig| sig.outputs.len() == 1)
                .map(|sig| Some(sig.outputs[0].name.clone())),
            // `let r = ns.Foo(...)`: a multi-output member returns a record, whose fields
            // are checked normally; a single-output member returns the output type, whose
            // name is not indexed, so it is marked unindexed.
            Expr::FieldAccess { obj, field, .. } => match obj.as_ref() {
                Expr::Ident {
                    name: ns,
                    range: ns_range,
                } => ctx
                    .ns_members(ns, &ns_range.file)
                    .and_then(|m| m.get(field.as_str()))
                    .and_then(|info| info.return_type.as_ref())
                    .and_then(|te| match te {
                        TypeExpr::Record { .. } => None,
                        _ => Some(None),
                    }),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    if let Some(out) = entry {
        ctx.single_output_alias.insert(name.clone(), out);
    }
}

/// After a `const` declaration's initializer evaluated and was recorded, every
/// name it introduced must read back as a constant here; if one does not, that
/// is WS046 at the binding. This is not the evaluation-failure check (the
/// `Err(err) if l.is_const` arms in `typecheck::decl` and `typecheck::stmt`
/// run first) but its opposite: a value that evaluated and landed nowhere a
/// later statement can see, which would otherwise surface at a use, blamed on
/// another binding, or not at all in a constant-only slot.
///
/// The test is plain `const_lookup` ("is a value readable here"), not a
/// const-declared read: `typecheck::decl`'s `TopDecl::Let` arm records without
/// the `const` mark, which is harmless at the top level but not inside the scope
/// `TopDecl::Namespace` checks members in, where the strict test would reject
/// every namespace import of a module with a top-level `const`.
///
/// Call it only after the success arm recorded, on the success path. At the top
/// level the recording that counts is `build_const_env`'s fixpoint, already
/// converged, so a name missing from it is settled.
pub(super) fn check_const_recorded(ctx: &mut TypeCheckCtx<'_>, l: &LetDecl) {
    // A `const` written with no value was reported where it was declared.
    if !l.is_const || !l.has_initializer() {
        return;
    }
    let env = ctx.const_lookup();
    let missing: Vec<String> = crate::const_eval::bound_names(&l.binding)
        .into_iter()
        .filter(|n| !env.contains_key(n))
        .collect();
    for name in missing {
        ctx.emit(
            "WS046",
            format!(
                "not a compile-time constant: '{name}' is declared `const` and its value \
                 evaluates, but it is not recorded as a constant in this scope. Reading it \
                 would give a runtime value, and passing it to a constant-only slot would \
                 bake an empty one"
            ),
            l.range.clone(),
        );
    }
}

/// The type a `let`/`const` binds: its annotation where the value is a
/// constant the annotation makes a float or an integer, or a literal built to
/// the annotation's shape (`infer::literal_fits`); the value's own type
/// otherwise. `let L: float = 9` binds a float and `let I: int = 2.5` an
/// integer, as `var` does.
pub(super) fn let_binding_type(ctx: &mut TypeCheckCtx<'_>, l: &LetDecl, inferred: &Type) -> Type {
    let Some(te) = &l.typ else {
        return inferred.clone();
    };
    let annotated = ctx.discarding_diagnostics(|ctx| resolve_type_expr(ctx, te));
    let annotated_number = unwrap_ref(&annotated);
    let number_constant = matches!(annotated_number, Type::Float | Type::Int)
        && ctx.recorded_type(l.value.range()) == Some(&annotated_number);
    let fitted_literal = coerce(inferred, &annotated) == CoerceRule::Mismatch
        && infer::literal_fits(ctx, &l.value, &annotated);
    // A record the annotation rejects is reported there, once.
    let rejected = infer::rejected_where_one_value_goes(inferred, &annotated);
    if number_constant || fitted_literal || rejected {
        annotated
    } else {
        inferred.clone()
    }
}

pub(super) fn bind_let(ctx: &mut TypeCheckCtx<'_>, b: &LetBinding, t: &Type, value: &Expr) {
    // Which fields of a destructured source are ref values, taken before any
    // name the pattern binds can shadow one the source names.
    let src = match b {
        LetBinding::Ident { .. } => None,
        _ => ref_values_of(ctx, value),
    };
    let src = src.as_ref();
    // A record parameter's plain fields are values bound by the caller, so
    // only a `*T` field of one is a place (as `field_names_storage` has it).
    let scalar_places = !matches!(
        super::infer::flatten_access(value).0,
        Expr::Ident { name, .. } if ctx.scope.lookup(name).is_some_and(|s| s.kind == SymbolKind::Param)
    );
    match b {
        LetBinding::Ident { name, range } => {
            // A `let` may shadow another value in the same scope (`const a = 1` then
            // `let a = ...`), but not an `in`/`out` port of the same name: `in go: exec`
            // then `let go = 5` would make `on go` bind the constant and leave the exec
            // input dead. `declare` returns the previous symbol only from the same frame,
            // so child-scope shadowing stays legal.
            declare_let(ctx, name, t.clone(), range);
        }
        LetBinding::Tuple { names, rest, range } => {
            // `rest` takes everything past `names`, so the source only has to
            // be AT LEAST as wide as the named positions when one is present.
            if let Some(fields) = as_tuple_fields(t)
                && (fields.len() == names.len() || (rest.is_some() && fields.len() >= names.len()))
            {
                // The source's own key for each position.
                let key = |i: usize| match t {
                    Type::Record(fs) | Type::Outputs(fs) => fs[i].0.clone(),
                    _ => i.to_string(),
                };
                if let Some(rest_name) = rest {
                    let tail: Vec<(String, Type)> = fields[names.len()..]
                        .iter()
                        .enumerate()
                        .map(|(i, ft)| (i.to_string(), ft.clone()))
                        .collect();
                    declare_let(ctx, rest_name, Type::Record(tail), range);
                    let moved: Vec<(String, String)> = (names.len()..fields.len())
                        .map(|i| (key(i), (i - names.len()).to_string()))
                        .collect();
                    set_let_ref_values(ctx, let_key(range, rest_name), rekeyed(src, &moved));
                }
                for (i, (n, ft)) in names.iter().zip(fields.iter()).enumerate() {
                    declare_field(ctx, n, &key(i), ft.clone(), range, src, scalar_places);
                }
                return;
            }
            let got = match as_tuple_fields(t) {
                Some(fields) => format!("{} value(s)", fields.len()),
                None => format!("{t:?}, which has no positional fields"),
            };
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: this pattern binds {} position(s){}, but the value has {got}",
                    names.len(),
                    if rest.is_some() { " plus a rest" } else { "" },
                ),
                range.clone(),
            );
            for n in names.iter().chain(rest.iter()) {
                declare_let(ctx, n, Type::Any, range);
            }
        }
        LetBinding::RecordDestruct { fields, range } => {
            // The field names a `Named` has consumed so far, grown as the pattern is
            // walked, as `const_eval::bind_destructured` builds its own `consumed`: a
            // `Named` after a `Rest` is not excluded from that rest by either. The parser
            // forces `...rest` last today, but the two must agree on any pattern.
            let mut consumed: HashSet<&str> = HashSet::default();
            for field in fields {
                let (name, ty, path) = match field {
                    RecordDestructField::Named { name, alias, .. } => {
                        let bind_name = alias.as_ref().unwrap_or(name);
                        let field_ty =
                            if let Type::Record(rec_fields) | Type::Outputs(rec_fields) = t {
                                rec_fields
                                    .iter()
                                    .find(|(k, _)| k == name)
                                    .map(|(_, t)| t.clone())
                                    .unwrap_or(Type::Any)
                            } else {
                                Type::Any
                            };
                        consumed.insert(name.as_str());
                        (bind_name.clone(), field_ty, name.as_str())
                    }
                    RecordDestructField::Rest { name, .. } => {
                        // `...rest` collects every field no `Named` consumed, so its type is the record
                        // of exactly those fields in the source's order, as `bind_destructured`'s value
                        // rule has it; as `Any`, `rest.y == 222` would be WS004. Only a known
                        // `Type::Record` source gives a precise type; anything else stays `Any`.
                        let rest_ty =
                            if let Type::Record(rec_fields) | Type::Outputs(rec_fields) = t {
                                Type::Record(
                                    rec_fields
                                        .iter()
                                        .filter(|(k, _)| !consumed.contains(k.as_str()))
                                        .cloned()
                                        .collect(),
                                )
                            } else {
                                Type::Any
                            };
                        declare_let(ctx, name, rest_ty, range);
                        set_let_ref_values(ctx, let_key(range, name), src.cloned());
                        continue;
                    }
                };
                declare_field(ctx, &name, path, ty, range, src, scalar_places);
            }
        }
    }
}

/// Declare `name`, bound to field `path` of a destructured record whose ref
/// values are `src`. A scalar field that names storage (a `*T` field, or with
/// `scalar_places` any scalar, as `let (p, q) = (x, y)` binds) is a place,
/// which reads as its target and binds a `*T` parameter to it, exactly as a
/// destructured `*T` parameter does; a record-valued field keeps its own
/// fields' classification.
fn declare_field(
    ctx: &mut TypeCheckCtx<'_>,
    name: &str,
    path: &str,
    ty: Type,
    range: &SourceRange,
    src: Option<&LetRefValues>,
    scalar_places: bool,
) {
    let sub = src.and_then(|v| v.below(&[path.to_string()]));
    let ref_place = ty.ref_kind() == Some(RefKind::Var);
    if sub.is_none() && (ref_place || (scalar_places && ty.is_ref_target())) {
        ctx.let_places.insert(let_key(range, name));
        let ty = if ref_place {
            ty
        } else {
            Type::Ref(Box::new(ty))
        };
        declare_let(ctx, name, ty, range);
        if let Some(sym) = ctx.scope.lookup(name).cloned() {
            ctx.scope.declare(
                name,
                SymbolInfo {
                    kind: SymbolKind::Param,
                    ..sym
                },
            );
        }
        return;
    }
    let record = matches!(
        unwrap_ref(&ty),
        Type::Record(_) | Type::Outputs(_) | Type::Tuple(_)
    );
    declare_let(ctx, name, ty, range);
    if record {
        set_let_ref_values(ctx, let_key(range, name), sub);
    }
}

/// `src` with each `(from, to)` field moved to key `to`, for a pattern that
/// re-keys what it collects (a tuple `...rest` counts from 0).
fn rekeyed(src: Option<&LetRefValues>, moved: &[(String, String)]) -> Option<LetRefValues> {
    let src = src?;
    let mut set = HashSet::default();
    for (from, to) in moved {
        merge_field_source(src.below(std::slice::from_ref(from)), to, &mut set);
    }
    normalized(LetRefValues::Fields(set))
}

/// Bind one `let` name and report the collisions the plain `let x = ...` arm has
/// always reported. Every `LetBinding` shape reaches this, so a destructuring
/// pattern cannot hijack an in/out port or silently replace another file's
/// top-level binding the way `let (go, r) = ...` did while `let go = ...` errored.
fn declare_let(ctx: &mut TypeCheckCtx<'_>, name: &str, ty: Type, range: &SourceRange) {
    let prev = ctx
        .scope
        .declare_new(SymbolKind::LetBinding, name, ty, range.clone());
    match &prev {
        Some(p) if matches!(p.kind, SymbolKind::In | SymbolKind::Out) => {
            ctx.emit(
                "WS013",
                format!("'{name}' shadows the in/out port of the same name; rename one"),
                range.clone(),
            );
        }
        // In an anonymous chip a name is declared once (see `scope::top`). A
        // chip's own declaration beside the top level's is the shadowing
        // rule, and two at the top level itself are read by position
        // (`WS108`).
        Some(p)
            if ctx.scope.depth() == 1
                && p.decl_range.file == range.file
                && !p.decl_range.file.is_empty()
                && p.decl_range != *range
                && ctx.top_names.same_level(&p.decl_range, range)
                && ctx.top_names.chips_around(range).next().is_some()
                && !name.starts_with("_on_expr_") =>
        {
            ctx.emit(
                "WS013",
                format!(
                    "duplicate declaration of '{name}': this chip already declares it (line {}), and a \
                     name is declared once in a chip. Rename one",
                    p.decl_range.start.line
                ),
                range.clone(),
            );
        }
        // Shadowing is a same-file idiom. Two files each binding `name` at the top
        // level is not shadowing: imports merge into one flat scope, so one module's
        // constant replaces the other's in both (`mod`/`chip` report the same
        // collision). Compiler-generated names are exempt: `on Foo(...)` over a
        // non-event desugars to `let _on_expr_N = Foo(...)`, the counter restarts per
        // file, and lowering pairs each `on` with its own file's `let`, so a shared
        // `_on_expr_0` resolves correctly.
        Some(p)
            if !p.decl_range.file.is_empty()
                && !range.file.is_empty()
                && p.decl_range.file != range.file
                && !name.starts_with("_on_expr_") =>
        {
            ctx.emit(
                "WS013",
                format!(
                    "duplicate declaration of '{name}': also declared in '{}'. Imports \
                     merge into one flat scope, so rename one or import it with `as`",
                    p.decl_range.file
                ),
                range.clone(),
            );
        }
        _ => {}
    }
}

/// A binding whose initializer has no value (`Type::Never`) has nothing to
/// bind: report it and return `true`. Only a void container mutation
/// (`a.push(x)`, `m.set(k, v)`) and a mod or chip call with no output produce
/// `Never`. Shared by `let`, `var` and `out`.
pub(super) fn reject_never_value(
    ctx: &mut TypeCheckCtx<'_>,
    inferred: &Type,
    range: &SourceRange,
    name: &str,
) -> bool {
    if !matches!(inferred, Type::Never) {
        return false;
    }
    emit_no_value(ctx, Some(name), range);
    true
}

/// The shared WS072 message for a `Never`-typed expression read as a value,
/// naming the binding when there is one. Also the sink `infer::coerce_or_emit`
/// routes a `Never` source to, so the assignment / argument / output spellings
/// report the same thing as a binding rather than a bare type mismatch.
pub(crate) fn emit_no_value(ctx: &mut TypeCheckCtx<'_>, name: Option<&str>, range: &SourceRange) {
    let subject = match name {
        Some(n) => format!("`{n}` is bound to an expression that"),
        None => "this expression".to_string(),
    };
    ctx.emit(
        "WS072",
        format!(
            "{subject} produces no value: a void mutation such as `a.push(x)` or \
             `m.set(k, v)`, or a `mod`/`chip` with no output. Use it as a statement, \
             not a value; a `mod` that should hand back a value needs an output \
             (`mod f() -> (r: int)`)"
        ),
        range.clone(),
    );
}

/// `reject_never_value` for a `let`; `true` lets the caller skip the annotation
/// check, which would add a redundant WS016. Exempt: `let inst = SomeChip(t)` on
/// a chip with no outputs, the documented way to place a named chip
/// (`docs/src/chips.md`, "Exec Chips"). The name still carries `Never`, so a later
/// use is checked.
pub(super) fn reject_never_binding(
    ctx: &mut TypeCheckCtx<'_>,
    l: &LetDecl,
    inferred: &Type,
) -> bool {
    if is_user_chip_call(ctx, &l.value) {
        return false;
    }
    let name = match &l.binding {
        LetBinding::Ident { name, .. } => name.as_str(),
        _ => "<binding>",
    };
    reject_never_value(ctx, inferred, l.value.range(), name)
}

/// Is `e` a direct call to a user `mod`/`chip` by name (`Foo(...)`, not a
/// method call, builtin, or namespaced member)? Used only for the
/// instantiation exemption above.
fn is_user_chip_call(ctx: &TypeCheckCtx<'_>, e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. }
        if matches!(callee.as_ref(), Expr::Ident { name, range }
            if ctx.calls_user_mod(name, &range.file)))
}

pub(super) fn check_let_type_annotation(ctx: &mut TypeCheckCtx<'_>, l: &LetDecl, inferred: &Type) {
    if let Some(ref te) = l.typ {
        // Record literals: validate field names against the expected record type.
        // Point errors at the specific field/spread that introduced the mismatch.
        if let Expr::RecordLit { fields, .. } = &l.value {
            let expected = resolve_annotation(ctx, te);
            // `{}` for a map is the empty map literal, which no `let` binds.
            if fields.is_empty() && matches!(expected, Type::Map(..)) {
                ctx.emit(
                    "WS026",
                    "a map literal must initialize or assign a Map variable",
                    l.value.range().clone(),
                );
                return;
            }
            let before = ctx.diagnostics.len();
            if let Type::Record(expected_fields) = &expected {
                let type_name = type_expr_str(te);
                for f in fields {
                    match f {
                        RecordLitField::Named { name, range, .. } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{name}' not in type {type_name}"),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Shorthand { name, range } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{name}' not in type {type_name}"),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Spread { value, range } => {
                            let spread_ty = infer::infer(ctx, value);
                            if let Type::Record(spread_fields) | Type::Outputs(spread_fields) =
                                &spread_ty
                            {
                                let extras: Vec<&str> = spread_fields
                                    .iter()
                                    .filter(|(n, _)| !expected_fields.iter().any(|(en, _)| en == n))
                                    .map(|(n, _)| n.as_str())
                                    .collect();
                                if !extras.is_empty() {
                                    ctx.emit(
                                        "WS003",
                                        format!(
                                            "spread introduces fields not in {}: {}",
                                            type_name,
                                            extras.join(", ")
                                        ),
                                        range.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
                // A missing field isn't caused by any one field/spread, so
                // point the error at the whole literal.
                if let Type::Record(inferred_fields) = inferred {
                    for (fname, _) in expected_fields {
                        if !inferred_fields.iter().any(|(n, _)| n == fname) {
                            ctx.emit(
                                "WS003",
                                format!("missing field '{fname}' for type {type_name}"),
                                l.range.clone(),
                            );
                        }
                    }
                }
            }
            // A record literal is also how a tuple literal parses (index-keyed fields), so
            // `expected` may be a `Type::Tuple` the checks above skip, and for a record
            // target they cover only field presence. If none fired, the full structural
            // coercion runs, catching a wrong field or element type, a tuple arity, or a
            // record against a non-record target (WS003); the diagnostic count guard keeps
            // a field error from being repeated as a whole-type mismatch.
            if ctx.diagnostics.len() == before {
                if infer::literal_fits(ctx, &l.value, &expected) {
                    infer::coerce_value_or_emit(ctx, &l.value, inferred, &expected);
                } else {
                    infer::coerce_or_emit(ctx, inferred, &expected, &l.range);
                }
            }
            return;
        }
        let expected = resolve_annotation(ctx, te);
        if infer::record_into_one_value(ctx, &l.value, inferred, &expected) {
            return;
        }
        let rule = coerce(inferred, &expected);
        // A literal built to the annotation's shape (`[1, 2]` for a
        // `float[]`) is what the annotation says.
        if rule == CoerceRule::Mismatch && infer::literal_fits(ctx, &l.value, &expected) {
            infer::coerce_value_or_emit(ctx, &l.value, inferred, &expected);
            return;
        }
        // `ViaString` is fine: anything primitive casts to string, so
        // `let s: string = 5` is an intentional format, not a type lie.
        if rule == CoerceRule::Mismatch {
            let name = match &l.binding {
                LetBinding::Ident { name, .. } => name.clone(),
                _ => "<binding>".into(),
            };
            let message = format!(
                "let '{}' annotated as {}, but expression has type {}",
                name,
                type_expr_str(te),
                type_str(inferred),
            );
            // A `*T` var ref or a `brick` on either side has no conversion into a
            // mismatched annotation (refs are invariant), so this can never wire: a hard
            // error, unlike a primitive mismatch (`let n: int = someString`), which stays
            // the lenient warning below. An array or map ref (`let a1: float[] = row0`)
            // keeps WS016.
            let is_var_or_brick_ref =
                |t: &Type| matches!(t.ref_kind(), Some(RefKind::Var | RefKind::Brick));
            if is_var_or_brick_ref(inferred) || is_var_or_brick_ref(&expected) {
                ctx.emit("WS003", message, l.range.clone());
            } else {
                ctx.diagnostics.push(crate::Diagnostic {
                    severity: Severity::Warning,
                    code: "WS016".into(),
                    message,
                    range: l.range.clone(),
                });
            }
        }
    }
}
