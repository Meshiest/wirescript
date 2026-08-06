//! The one canonical `TypeExpr → Type` resolver.
//!
//! Before this module, ~5 call sites independently walked a `TypeExpr` (or
//! matched a bare type name) into an `ir::Type`, and they drifted: the
//! lowering-side resolvers were missing `zone`/`teleport`, so a
//! `zone`/`teleport`-annotated value silently degraded to `Type::Any` once it
//! reached lowering (even though typecheck had resolved it correctly).
//! [`primitive`] is now the single primitive-name table, and [`resolve_type`]
//! is the single structural resolver; every other resolver in the crate
//! either delegates to these directly or (for `typecheck::resolve_type_expr`,
//! which is entangled with the mutable scope stack) at least shares the
//! primitive table.

use crate::collections::HashMap;

use crate::ast::TypeExpr;
use crate::diagnostic::Diagnostic;
use crate::ir::Type;

/// The one primitive-name → Type table (single source of truth). Mirrors the
/// full mapping formerly duplicated across `typecheck::primitive_name`,
/// `analysis::types::type_from_name`, `lower::predeclare::type_of_type_expr`,
/// and `lower::handler::type_expr_to_type`.
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
    /// to `Type::Param`). Empty until generic parsing lands (P2).
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
            Type::Ref(Box::new(resolve_type_at_depth(inner, cx, diags, depth, in_progress)))
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
                ("Dict", [k, v]) => {
                    let key = resolve_type_at_depth(k, cx, diags, depth, in_progress);
                    let val = resolve_type_at_depth(v, cx, diags, depth, in_progress);
                    // A dict is keyed by a hashed slot, so only int, string, and
                    // object (entity/character/controller) keys have a
                    // representation. A generic param key (`Dict<K, V>`) is
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
                                "dict key type must be int, string, or an object \
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
mod tests {
    use super::*;
    use crate::diagnostic::{Pos, SourceRange};

    fn range() -> SourceRange {
        SourceRange::new("test.ws", Pos::default(), Pos::default())
    }

    fn name_te(name: &str) -> TypeExpr {
        TypeExpr::Name {
            name: name.to_string(),
            range: range(),
        }
    }

    fn array_te(inner: &str) -> TypeExpr {
        TypeExpr::Array {
            inner: Box::new(name_te(inner)),
            range: range(),
        }
    }

    #[test]
    fn resolver_handles_primitives_params_and_refs() {
        let params = vec!["T".to_string()];
        let aliases: HashMap<String, Type> = HashMap::default();
        let generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        assert_eq!(resolve_type(&name_te("zone"), &cx, &mut d), Type::Zone);
        assert_eq!(
            resolve_type(&name_te("teleport"), &cx, &mut d),
            Type::Teleport
        );
        assert_eq!(
            resolve_type(&name_te("T"), &cx, &mut d),
            Type::Param("T".into())
        );
        assert_eq!(
            resolve_type(&array_te("T"), &cx, &mut d),
            Type::Array(Box::new(Type::Param("T".into())))
        );
        assert!(d.is_empty(), "known names emit no diagnostics");
    }

    #[test]
    fn resolver_reports_unknown_names_and_generics() {
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        assert_eq!(resolve_type(&name_te("Bogus"), &cx, &mut d), Type::Any);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "WS002");
        assert!(d[0].message.contains("unknown type 'Bogus'"));
    }

    #[test]
    fn resolver_resolves_type_aliases() {
        let params: Vec<String> = Vec::new();
        let mut aliases: HashMap<String, Type> = HashMap::default();
        aliases.insert("Point".to_string(), Type::Vector);
        let generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        assert_eq!(resolve_type(&name_te("Point"), &cx, &mut d), Type::Vector);
        assert!(d.is_empty());
    }

    fn generic_te(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Generic {
            name: name.to_string(),
            args,
            range: range(),
        }
    }

    fn record_te(fields: &[(&str, TypeExpr)]) -> TypeExpr {
        TypeExpr::Record {
            fields: fields
                .iter()
                .map(|(n, t)| crate::ast::RecordTypeField {
                    name: n.to_string(),
                    typ: t.clone(),
                    range: range(),
                })
                .collect(),
            range: range(),
        }
    }

    #[test]
    fn resolver_instantiates_generic_alias() {
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let mut generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        // type Pair<T> = { a: T, b: T }
        generic_aliases.insert(
            "Pair".to_string(),
            GenericAlias {
                params: vec!["T".to_string()],
                body: record_te(&[("a", name_te("T")), ("b", name_te("T"))]),
            },
        );
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        // Pair<int> -> { a: int, b: int }
        let resolved = resolve_type(&generic_te("Pair", vec![name_te("int")]), &cx, &mut d);
        assert_eq!(
            resolved,
            Type::Record(vec![("a".into(), Type::Int), ("b".into(), Type::Int)])
        );
        assert!(d.is_empty(), "clean instantiation emits no diagnostics: {d:?}");

        // Bare `Pair` (no args) errors — not fully applied.
        let mut d2 = Vec::new();
        assert_eq!(resolve_type(&name_te("Pair"), &cx, &mut d2), Type::Any);
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].code, "WS002");

        // Wrong arity errors.
        let mut d3 = Vec::new();
        let bad = generic_te("Pair", vec![name_te("int"), name_te("float")]);
        assert_eq!(resolve_type(&bad, &cx, &mut d3), Type::Any);
        assert_eq!(d3.len(), 1);
        assert_eq!(d3[0].code, "WS002");
    }

    #[test]
    fn resolver_rejects_recursive_generic_alias() {
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let mut generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        // type L<T> = { head: T, tail: L<T> }
        generic_aliases.insert(
            "L".to_string(),
            GenericAlias {
                params: vec!["T".to_string()],
                body: record_te(&[
                    ("head", name_te("T")),
                    ("tail", generic_te("L", vec![name_te("T")])),
                ]),
            },
        );
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        // Terminates (cut off by the in-progress cycle guard) rather than
        // hanging, and flags the self-reference. The returned `Type` isn't the
        // contract here, only that resolution terminates and reports an error.
        let _resolved = resolve_type(&generic_te("L", vec![name_te("int")]), &cx, &mut d);
        assert!(
            d.iter().any(|diag| diag.code == "WS002"),
            "recursive alias should be flagged, not hang: {d:?}"
        );
    }

    #[test]
    fn resolver_rejects_doubly_recursive_generic_alias() {
        // A body that references its own instantiation TWICE would re-expand
        // each occurrence, so a depth-only guard blows up as 2^depth before the
        // cap trips. The in-progress cycle guard must cut this at the first
        // re-entry, keeping it linear and terminating promptly.
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let mut generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        // type Tree<T> = { l: Tree<T>, r: Tree<T> }
        generic_aliases.insert(
            "Tree".to_string(),
            GenericAlias {
                params: vec!["T".to_string()],
                body: record_te(&[
                    ("l", generic_te("Tree", vec![name_te("T")])),
                    ("r", generic_te("Tree", vec![name_te("T")])),
                ]),
            },
        );
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        let _resolved = resolve_type(&generic_te("Tree", vec![name_te("int")]), &cx, &mut d);
        assert!(
            d.iter().any(|diag| diag.code == "WS002"),
            "doubly-recursive alias should be flagged, not hang: {d:?}"
        );
    }
}
