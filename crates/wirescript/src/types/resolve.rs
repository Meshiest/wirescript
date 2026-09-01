//! The one canonical `TypeExpr → Type` resolver.
//!
//! [`primitive`] is the single primitive-name table, and [`resolve_type`] is
//! the single structural resolver; every other resolver in the crate either
//! delegates to these directly or (for `typecheck::resolve_type_expr`, which
//! is entangled with the mutable scope stack) at least shares the primitive
//! table. A resolver with its own independent copy of this mapping risks
//! drift — e.g. missing `zone`/`teleport` would silently degrade a
//! `zone`/`teleport`-annotated value to `Type::Any` once it reached lowering,
//! even though typecheck had resolved it correctly.

use crate::collections::HashMap;

use crate::ast::TypeExpr;
use crate::diagnostic::Diagnostic;
use crate::ir::Type;

/// The one primitive-name → Type table (single source of truth).
pub fn primitive(name: &str) -> Option<Type> {
    Some(match name {
        "bool" => Type::Bool,
        "int" => Type::Int,
        "float" => Type::Float,
        "string" => Type::String,
        "vector" => Type::Vector,
        "rotator" => Type::Rotator,
        "quat" => Type::Quat,
        "color" => Type::Color,
        "entity" => Type::Entity,
        "character" => Type::Character,
        "controller" => Type::Controller,
        "zone" => Type::Zone,
        "teleport" => Type::Teleport,
        "prefab" => Type::PrefabRef,
        "exec" => Type::Exec,
        // `any` (in annotation position) maps onto the *wildcard* type
        // (`Type::Opaque`, the same type `Opaque(...)` produces) rather than
        // the internal `Type::Any` error-fallback: an `any`-typed value must
        // resolve operator overloads (`catalog::operators::type_kind_matches`
        // wildcards `Opaque`) and act as a permanent fold barrier, which only
        // `Opaque` does.
        "any" => Type::Opaque,
        "never" => Type::Never,
        _ => return None,
    })
}

/// A generic type alias's template — its declared parameter names and the
/// (still parametric) `TypeExpr` body, substituted at each `Name<Args>` use
/// site. Populated from `ast::TypeAliasDecl`s whose `type_params` is
/// non-empty; the body is intentionally left unresolved at declaration time
/// (it references its own free params, which aren't bound to anything until
/// an actual use supplies arguments).
#[derive(Clone, Debug)]
pub struct GenericAlias {
    pub params: Vec<String>,
    pub body: TypeExpr,
}

/// Recursion cap for generic-alias instantiation (`type L<T> = { tail: L<T> }`
/// must error, not stack-overflow / hang). Generous enough for any legitimate
/// nesting a hand-written alias would use.
const MAX_ALIAS_DEPTH: u32 = 32;

/// Context a [`resolve_type`] call resolves names against.
pub struct ResolveCtx<'a> {
    /// In-scope generic type-parameter names (a `Name` matching one resolves
    /// to `Type::Param`).
    pub params: &'a [String],
    /// Type aliases visible to the caller (name → resolved type). Empty for
    /// the lowering-side callers; typecheck fills it from its scope so
    /// `type X = …` names still resolve.
    pub type_aliases: &'a HashMap<String, Type>,
    /// Generic type aliases visible to the caller (name → param list + raw
    /// `TypeExpr` body), instantiated by substitution when used as
    /// `Name<Args>`. Empty for lowering-side callers and any context that
    /// doesn't need generic-alias resolution — typecheck's
    /// `resolve_type_expr` is the one caller that fills it.
    pub generic_aliases: &'a HashMap<String, GenericAlias>,
}

/// Substitute type-parameter names (`T`) with concrete `TypeExpr`s throughout
/// `te`, producing a new `TypeExpr`. The lowering-side analog of the
/// `Type`-level param binding [`resolve_type`] does during instantiation: it
/// keeps the structural `TypeExpr` (field names + per-field types), which
/// lowering's record-port dissolution needs (it can't work off a flattened
/// `Type::Record` because it has to re-resolve each field's own `TypeExpr` to
/// detect `array`/`ref` sub-ports). A `Name` matching a substitution key
/// becomes that argument's `TypeExpr` verbatim.
pub fn substitute_type_expr(te: &TypeExpr, subst: &HashMap<String, TypeExpr>) -> TypeExpr {
    match te {
        TypeExpr::Name { name, .. } => match subst.get(name) {
            Some(rep) => rep.clone(),
            None => te.clone(),
        },
        TypeExpr::Ref { inner, range } => TypeExpr::Ref {
            inner: Box::new(substitute_type_expr(inner, subst)),
            range: range.clone(),
        },
        TypeExpr::Array { inner, range } => TypeExpr::Array {
            inner: Box::new(substitute_type_expr(inner, subst)),
            range: range.clone(),
        },
        TypeExpr::Tuple { fields, range } => TypeExpr::Tuple {
            fields: fields.iter().map(|f| substitute_type_expr(f, subst)).collect(),
            range: range.clone(),
        },
        TypeExpr::Union { options, range } => TypeExpr::Union {
            options: options.iter().map(|o| substitute_type_expr(o, subst)).collect(),
            range: range.clone(),
        },
        TypeExpr::Record { fields, range } => TypeExpr::Record {
            fields: fields
                .iter()
                .map(|f| crate::ast::RecordTypeField {
                    name: f.name.clone(),
                    typ: substitute_type_expr(&f.typ, subst),
                    range: f.range.clone(),
                })
                .collect(),
            range: range.clone(),
        },
        TypeExpr::Generic { name, args, range } => TypeExpr::Generic {
            name: name.clone(),
            args: args.iter().map(|a| substitute_type_expr(a, subst)).collect(),
            range: range.clone(),
        },
    }
}

/// Instantiate a generic-alias application (`Pair<int>`) to its concrete body
/// `TypeExpr` (`{ a: int, b: int }`), substituting each type argument for its
/// parameter. Returns `None` if `name` isn't a known generic alias or the
/// argument count doesn't match (so the caller can fall through to its
/// not-a-generic-alias path — typecheck has already flagged a real arity
/// error). Lowering uses this to dissolve a generic-alias record annotation
/// into per-field sub-ports, the same as a non-generic `type P = { … }`.
pub fn instantiate_generic_alias(
    name: &str,
    args: &[TypeExpr],
    generic_aliases: &HashMap<String, GenericAlias>,
) -> Option<TypeExpr> {
    let alias = generic_aliases.get(name)?;
    if alias.params.len() != args.len() {
        return None;
    }
    let subst: HashMap<String, TypeExpr> = alias
        .params
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    Some(substitute_type_expr(&alias.body, &subst))
}

/// Resolve a `TypeExpr` to a `Type`. Pushes a diagnostic into `diags` for an
/// unknown type name (so typecheck can surface WS002); lowering callers pass
/// a throwaway `&mut Vec::new()` and ignore it — typecheck has already
/// flagged unresolvable names by the time lowering runs.
pub fn resolve_type(te: &TypeExpr, cx: &ResolveCtx, diags: &mut Vec<Diagnostic>) -> Type {
    // `in_progress` is a path stack of `(alias_name, resolved_arg_types)`
    // currently being expanded. A body that references its own instantiation
    // (`type Tree<T> = { l: Tree<T>, r: Tree<T> }`) re-enters the same key and
    // is cut off there — without this, `depth` alone bounds only ONE self-ref
    // chain, so a doubly-self-referencing alias re-expands each occurrence and
    // blows up exponentially (2^depth) before the depth cap ever trips.
    let mut in_progress: Vec<(String, Vec<Type>)> = Vec::new();
    resolve_type_at_depth(te, cx, diags, 0, &mut in_progress)
}

/// `*T` on a RECORD or TUPLE distributes over its fields: `*{ a: A, b: B }`
/// resolves to `{ a: *A, b: *B }`, and `*(A, B)` to `(*A, *B)`.
///
/// A record has no single wire to point at - storage is one gate per field
/// (`declare_record_container`) - so a ref wrapping the whole record names
/// nothing, while the per-field form is exactly what a chip/mod boundary
/// already carries writably. Nested records recurse; a field that is already a
/// ref stays as it is rather than becoming `**T`.
fn distribute_ref(t: Type) -> Type {
    match t {
        Type::Record(fields) => Type::Record(
            fields
                .into_iter()
                .map(|(n, ft)| (n, distribute_ref(ft)))
                .collect(),
        ),
        Type::Tuple(fields) => Type::Tuple(fields.into_iter().map(distribute_ref).collect()),
        already @ Type::Ref(_) => already,
        other => Type::Ref(Box::new(other)),
    }
}

fn resolve_type_at_depth(
    te: &TypeExpr,
    cx: &ResolveCtx,
    diags: &mut Vec<Diagnostic>,
    depth: u32,
    in_progress: &mut Vec<(String, Vec<Type>)>,
) -> Type {
    match te {
        TypeExpr::Name { name, range } => {
            if cx.params.iter().any(|p| p == name) {
                return Type::Param(name.clone());
            }
            if let Some(prim) = primitive(name) {
                return prim;
            }
            // A generic alias used bare (no `<Args>`) is never a valid type —
            // it isn't fully applied, so there's nothing to substitute `T`
            // with. Caught here (before the plain alias-map fallback) so it
            // gets a targeted message instead of silently resolving through
            // `cx.type_aliases`' placeholder entry (see `typecheck.rs`'s
            // registration of a generic alias's scope symbol).
            if cx.generic_aliases.contains_key(name) {
                diags.push(Diagnostic::error(
                    "WS002",
                    format!("'{name}' is a generic type; write '{name}<...>'"),
                    range.clone(),
                ));
                return Type::Any;
            }
            if let Some(t) = cx.type_aliases.get(name) {
                return t.clone();
            }
            diags.push(Diagnostic::error(
                "WS002",
                format!("unknown type '{name}'"),
                range.clone(),
            ));
            Type::Any
        }
        TypeExpr::Ref { inner, .. } => {
            distribute_ref(resolve_type_at_depth(inner, cx, diags, depth, in_progress))
        }
        TypeExpr::Array { inner, .. } => {
            Type::Array(Box::new(resolve_type_at_depth(inner, cx, diags, depth, in_progress)))
        }
        TypeExpr::Tuple { fields, .. } => Type::Tuple(
            fields
                .iter()
                .map(|f| resolve_type_at_depth(f, cx, diags, depth, in_progress))
                .collect(),
        ),
        TypeExpr::Union { options, .. } => Type::Union(
            options
                .iter()
                .map(|f| resolve_type_at_depth(f, cx, diags, depth, in_progress))
                .collect(),
        ),
        TypeExpr::Record { fields, .. } => Type::Record(
            fields
                .iter()
                .map(|f| (f.name.clone(), resolve_type_at_depth(&f.typ, cx, diags, depth, in_progress)))
                .collect(),
        ),
        TypeExpr::Generic { name, args, range } => {
            if let Some(alias) = cx.generic_aliases.get(name) {
                if args.len() != alias.params.len() {
                    diags.push(Diagnostic::error(
                        "WS002",
                        format!(
                            "'{name}' expects {} type argument(s), got {}",
                            alias.params.len(),
                            args.len()
                        ),
                        range.clone(),
                    ));
                    for a in args {
                        resolve_type_at_depth(a, cx, diags, depth, in_progress);
                    }
                    return Type::Any;
                }
                // Resolve each argument in the OUTER ctx (args are the caller's
                // types, not the alias body's) — this is also the identity we
                // key cycle-detection on.
                let resolved_args: Vec<Type> = args
                    .iter()
                    .map(|a| resolve_type_at_depth(a, cx, diags, depth, in_progress))
                    .collect();
                // Cycle guard: this exact instantiation is already expanding
                // higher on the path → self-referential, stop. Handles both a
                // single self-ref (`L<T> = { tail: L<T> }`) and, crucially, a
                // doubly-self-referencing body (`Tree<T> = { l: Tree<T>, r:
                // Tree<T> }`) which a depth counter alone lets blow up
                // exponentially. The `depth` cap below stays as a backstop for
                // any pathological non-cyclic nesting.
                if in_progress
                    .iter()
                    .any(|(n, a)| n == name && *a == resolved_args)
                    || depth >= MAX_ALIAS_DEPTH
                {
                    diags.push(Diagnostic::error(
                        "WS002",
                        format!("recursive type alias '{name}'"),
                        range.clone(),
                    ));
                    return Type::Any;
                }
                // Bind each param to its resolved arg and resolve the alias
                // body in a child ctx carrying those bindings. A `Name` in the
                // body matching a param resolves through this child alias map
                // (params are filtered out of the child's `params` list so they
                // can't win the `Type::Param` fast-path ahead of the
                // substitution).
                let mut child_aliases = cx.type_aliases.clone();
                for (p, resolved_arg) in alias.params.iter().zip(resolved_args.iter()) {
                    child_aliases.insert(p.clone(), resolved_arg.clone());
                }
                let filtered_params: Vec<String> = cx
                    .params
                    .iter()
                    .filter(|p| !alias.params.contains(p))
                    .cloned()
                    .collect();
                let child_cx = ResolveCtx {
                    params: &filtered_params,
                    type_aliases: &child_aliases,
                    generic_aliases: cx.generic_aliases,
                };
                in_progress.push((name.clone(), resolved_args));
                let result =
                    resolve_type_at_depth(&alias.body, &child_cx, diags, depth + 1, in_progress);
                in_progress.pop();
                return result;
            }
            match (name.as_str(), args.as_slice()) {
                ("Array", [v]) => {
                    Type::Array(Box::new(resolve_type_at_depth(v, cx, diags, depth, in_progress)))
                }
                ("Ref", [v]) => {
                    Type::Ref(Box::new(resolve_type_at_depth(v, cx, diags, depth, in_progress)))
                }
                ("Map", [k, v]) => {
                    let key = resolve_type_at_depth(k, cx, diags, depth, in_progress);
                    let val = resolve_type_at_depth(v, cx, diags, depth, in_progress);
                    // A map is keyed by a hashed slot, so only int, string, and
                    // object (entity/character/controller) keys have a
                    // representation. A generic param key (`Map<K, V>`) is
                    // validated per concrete instantiation; an already-errored
                    // `any` key is left alone.
                    if !matches!(
                        key,
                        Type::Int
                            | Type::String
                            | Type::Entity
                            | Type::Character
                            | Type::Controller
                            | Type::Any
                            | Type::Param(_)
                    ) {
                        diags.push(Diagnostic::error(
                            "WS039",
                            format!(
                                "map key type must be int, string, or an object \
                                 (entity/character/controller), got {key}"
                            ),
                            k.range().clone(),
                        ));
                    }
                    Type::Map(Box::new(key), Box::new(val))
                }
                _ => {
                    diags.push(Diagnostic::error(
                        "WS002",
                        format!("unknown generic type '{name}'"),
                        range.clone(),
                    ));
                    for a in args {
                        resolve_type_at_depth(a, cx, diags, depth, in_progress);
                    }
                    Type::Any
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
