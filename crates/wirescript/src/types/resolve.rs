//! The one canonical `TypeExpr -> Type` resolver.
//!
//! [`primitive`] is the single primitive-name table, and [`resolve_type`] is
//! the single structural resolver; every other resolver in the crate either
//! delegates to these directly or (for `typecheck::resolve_type_expr`, which
//! is entangled with the mutable scope stack) at least shares the primitive
//! table. A resolver with its own copy of this mapping risks drift: a missing
//! `zone`/`teleport` would silently degrade a `zone`/`teleport`-annotated
//! value to `Type::Any` once it reached lowering, even though typecheck had
//! resolved it correctly.

use crate::collections::HashMap;

use crate::ast::TypeExpr;
use crate::diagnostic::{Diagnostic, SourceRange};
use crate::ir::Type;

/// Maps a primitive type's source spelling to its [`Type`].
///
/// This is the table typecheck resolves annotations through, and it is
/// authoritative for what the language accepts. It is NOT the only such list:
/// the LSP and the playground each hand-write their own for completion, and
/// both are short of this one, so a type that resolves here may still not be
/// offered in the editor. Adding a primitive means editing those too.
pub(crate) fn primitive(name: &str) -> Option<Type> {
    PRIMITIVES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| t.clone())
}

/// Each primitive type's source spelling and its [`Type`] (see [`primitive`]).
pub(crate) const PRIMITIVES: &[(&str, Type)] = &[
    ("bool", Type::Bool),
    ("int", Type::Int),
    ("float", Type::Float),
    ("string", Type::String),
    ("vector", Type::Vector),
    ("rotator", Type::Rotator),
    ("quat", Type::Quat),
    ("color", Type::Color),
    ("entity", Type::Entity),
    ("character", Type::Character),
    ("controller", Type::Controller),
    ("brick", Type::Brick),
    ("zone", Type::Zone),
    ("teleport", Type::Teleport),
    ("prefab", Type::PrefabRef),
    // Any component reference; `Component<PointLight>` names one type.
    ("Component", Type::Component(String::new())),
    ("exec", Type::Exec),
    // `any` (in annotation position) maps onto the *wildcard* type
    // (`Type::Opaque`, the same type `Opaque(...)` produces) rather than
    // the internal `Type::Any` error-fallback: an `any`-typed value must
    // resolve operator overloads (`catalog::operators::type_kind_matches`
    // wildcards `Opaque`) and act as a permanent fold barrier, which only
    // `Opaque` does.
    ("any", Type::Opaque),
    ("never", Type::Never),
];

/// A generic type alias's template: its declared parameter names and the
/// (still parametric) `TypeExpr` body, substituted at each `Name<Args>` use
/// site. Populated from `ast::TypeAliasDecl`s whose `type_params` is
/// non-empty; the body is left unresolved at declaration time (it references
/// its own free params, which aren't bound until a use supplies arguments).
#[derive(Clone, Debug)]
pub(crate) struct GenericAlias {
    pub params: Vec<String>,
    pub body: TypeExpr,
}

/// Recursion cap for generic-alias instantiation (`type L<T> = { tail: L<T> }`
/// must error, not stack-overflow / hang). Generous enough for any legitimate
/// nesting a hand-written alias would use.
const MAX_ALIAS_DEPTH: u32 = 32;

/// Context a [`resolve_type`] call resolves names against.
pub(crate) struct ResolveCtx<'a> {
    /// In-scope generic type-parameter names (a `Name` matching one resolves
    /// to `Type::Param`).
    pub params: &'a [String],
    /// Type aliases visible to the caller (name -> resolved type), so
    /// `type X = ...` names resolve.
    pub type_aliases: &'a HashMap<String, Type>,
    /// Generic type aliases visible to the caller (name -> param list + raw
    /// `TypeExpr` body), instantiated by substitution when used as
    /// `Name<Args>`. Empty for any context that doesn't need generic-alias
    /// resolution.
    pub generic_aliases: &'a HashMap<String, GenericAlias>,
    /// The aliases a generic alias's body reads its own names against: those
    /// of the top level it is declared at, where none of the use site's type
    /// parameters exist. `None` reads them against `type_aliases`.
    pub declaring_aliases: Option<&'a HashMap<String, Type>>,
}

/// Substitute type-parameter names (`T`) with concrete `TypeExpr`s throughout
/// `te`, producing a new `TypeExpr`. The lowering-side analog of the
/// `Type`-level param binding [`resolve_type`] does during instantiation: it
/// keeps the structural `TypeExpr` (field names + per-field types), which
/// lowering's record-port dissolution needs (it can't work off a flattened
/// `Type::Record` because it has to re-resolve each field's own `TypeExpr` to
/// detect `array`/`ref` sub-ports). A `Name` matching a substitution key
/// becomes that argument's `TypeExpr` verbatim.
pub(crate) fn substitute_type_expr(te: &TypeExpr, subst: &HashMap<String, TypeExpr>) -> TypeExpr {
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
            fields: fields
                .iter()
                .map(|f| substitute_type_expr(f, subst))
                .collect(),
            range: range.clone(),
        },
        TypeExpr::Union { options, range } => TypeExpr::Union {
            options: options
                .iter()
                .map(|o| substitute_type_expr(o, subst))
                .collect(),
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
            args: args
                .iter()
                .map(|a| substitute_type_expr(a, subst))
                .collect(),
            range: range.clone(),
        },
    }
}

/// Instantiate a generic-alias application (`Pair<int>`) to its concrete body
/// `TypeExpr` (`{ a: int, b: int }`), substituting each type argument for its
/// parameter. Returns `None` if `name` isn't a known generic alias or the
/// argument count doesn't match (so the caller can fall through to its
/// not-a-generic-alias path; typecheck has already flagged a real arity
/// error). Lowering uses this to dissolve a generic-alias record annotation
/// into per-field sub-ports, the same as a non-generic `type P = { ... }`.
pub(crate) fn instantiate_generic_alias(
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

/// What an alias name means where a type is written (see
/// [`TypeNames::alias`]).
pub(crate) enum AliasDef<'a> {
    /// A type already resolved.
    Resolved(Type),
    /// A non-generic alias's body, read where the alias is declared.
    Body(&'a TypeExpr),
    /// A generic alias: its parameters and its body.
    Generic(&'a [String], &'a TypeExpr),
}

/// What the names a type is written with mean where it is written: the
/// table [`resolve_with`] reads them against.
pub(crate) trait TypeNames {
    /// The type the type parameter `name`, written at `range`, stands for.
    fn param(&self, name: &str, range: &SourceRange) -> Option<Type>;
    /// The alias `name`.
    fn alias(&self, name: &str) -> Option<AliasDef<'_>>;
    /// The enum `name` applied to `args`.
    fn enum_named(&self, name: &str, args: Vec<Type>) -> Option<Type>;
    /// The names the body of an alias read here reads: those where the alias
    /// is declared, with `bindings` for its parameters `params`.
    fn body_scope<'s>(
        &'s self,
        bindings: &'s HashMap<String, Type>,
        params: &'s [String],
    ) -> Box<dyn TypeNames + 's>;
}

impl TypeNames for ResolveCtx<'_> {
    fn param(&self, name: &str, _: &SourceRange) -> Option<Type> {
        self.params
            .iter()
            .any(|p| p == name)
            .then(|| Type::Param(name.to_string()))
    }

    fn alias(&self, name: &str) -> Option<AliasDef<'_>> {
        if let Some(g) = self.generic_aliases.get(name) {
            return Some(AliasDef::Generic(&g.params, &g.body));
        }
        self.type_aliases
            .get(name)
            .map(|t| AliasDef::Resolved(t.clone()))
    }

    fn enum_named(&self, _: &str, _: Vec<Type>) -> Option<Type> {
        None
    }

    fn body_scope<'s>(
        &'s self,
        bindings: &'s HashMap<String, Type>,
        params: &'s [String],
    ) -> Box<dyn TypeNames + 's> {
        // A body reads the aliases where it is declared, with none of the
        // use site's type parameters, when typecheck says where that is;
        // otherwise this scope's, less the parameters the alias binds.
        let parent: Box<dyn TypeNames + 's> = match self.declaring_aliases {
            Some(declaring) => Box::new(ResolveCtx {
                params: &[],
                type_aliases: declaring,
                generic_aliases: self.generic_aliases,
                declaring_aliases: self.declaring_aliases,
            }),
            None => Box::new(ResolveCtx { ..*self }),
        };
        Box::new(BoundAliases {
            parent,
            bindings,
            shadowed: params,
            declaring: self.declaring_aliases.is_some(),
        })
    }
}

/// A generic alias's body as typecheck reads it: its parameters bound like
/// aliases, after the primitives and any generic alias of the same name.
struct BoundAliases<'s> {
    parent: Box<dyn TypeNames + 's>,
    bindings: &'s HashMap<String, Type>,
    /// The alias's own parameters, which hide a use-site parameter.
    shadowed: &'s [String],
    /// Whether the body reads the declaring scope rather than the use site's.
    declaring: bool,
}

impl TypeNames for BoundAliases<'_> {
    fn param(&self, name: &str, range: &SourceRange) -> Option<Type> {
        if self.shadowed.iter().any(|p| p == name) {
            return None;
        }
        self.parent.param(name, range)
    }

    fn alias(&self, name: &str) -> Option<AliasDef<'_>> {
        match self.parent.alias(name) {
            generic @ Some(AliasDef::Generic(..)) => generic,
            other => self
                .bindings
                .get(name)
                .map(|t| AliasDef::Resolved(t.clone()))
                .or(other),
        }
    }

    fn enum_named(&self, name: &str, args: Vec<Type>) -> Option<Type> {
        self.parent.enum_named(name, args)
    }

    fn body_scope<'s>(
        &'s self,
        bindings: &'s HashMap<String, Type>,
        params: &'s [String],
    ) -> Box<dyn TypeNames + 's> {
        if self.declaring {
            self.parent.body_scope(bindings, params)
        } else {
            Box::new(BoundAliases {
                parent: Box::new(Borrowed(self)),
                bindings,
                shadowed: params,
                declaring: false,
            })
        }
    }
}

/// A scope borrowed where an owned one is expected.
struct Borrowed<'a>(&'a dyn TypeNames);

impl TypeNames for Borrowed<'_> {
    fn param(&self, name: &str, range: &SourceRange) -> Option<Type> {
        self.0.param(name, range)
    }

    fn alias(&self, name: &str) -> Option<AliasDef<'_>> {
        self.0.alias(name)
    }

    fn enum_named(&self, name: &str, args: Vec<Type>) -> Option<Type> {
        self.0.enum_named(name, args)
    }

    fn body_scope<'s>(
        &'s self,
        bindings: &'s HashMap<String, Type>,
        params: &'s [String],
    ) -> Box<dyn TypeNames + 's> {
        self.0.body_scope(bindings, params)
    }
}

/// Resolve a `TypeExpr` to a `Type`. Pushes a diagnostic into `diags` for an
/// unknown type name (so typecheck can surface WS002); a caller that has
/// already reported unresolvable names passes a throwaway `&mut Vec::new()`.
pub(crate) fn resolve_type(
    te: &TypeExpr,
    cx: &ResolveCtx<'_>,
    diags: &mut Vec<Diagnostic>,
) -> Type {
    resolve_with(te, cx, diags)
}

/// Resolve `te` against `names`, reporting into `diags`.
pub(crate) fn resolve_with(
    te: &TypeExpr,
    names: &dyn TypeNames,
    diags: &mut Vec<Diagnostic>,
) -> Type {
    // `in_progress` is a path stack of `(alias_name, resolved_arg_types)`
    // currently being expanded. A body that references its own instantiation
    // (`type Tree<T> = { l: Tree<T>, r: Tree<T> }`) re-enters the same key and
    // is cut off there; `depth` alone bounds only one self-ref chain, so a
    // doubly-self-referencing alias would re-expand each occurrence and blow up
    // exponentially (2^depth) before the depth cap ever trips.
    let mut in_progress: Vec<(String, Vec<Type>)> = Vec::new();
    resolve_type_at_depth(te, names, diags, 0, &mut in_progress)
}

/// `*T` on a RECORD or TUPLE distributes over its fields: `*{ a: A, b: B }`
/// resolves to `{ a: *A, b: *B }`, and `*(A, B)` to `(*A, *B)`.
///
/// A record has no single wire to point at - storage is one gate per field
/// (`declare_record_container`) - so a ref wrapping the whole record names
/// nothing, while the per-field form is exactly what a chip/mod boundary
/// already carries writably. Nested records recurse; a field that is already a
/// ref stays as it is rather than becoming `**T`.
pub(crate) fn distribute_ref(t: Type) -> Type {
    match t {
        Type::Record(fields) | Type::Outputs(fields) => Type::Record(
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
    cx: &dyn TypeNames,
    diags: &mut Vec<Diagnostic>,
    depth: u32,
    in_progress: &mut Vec<(String, Vec<Type>)>,
) -> Type {
    match te {
        TypeExpr::Name { name, range } => {
            if let Some(t) = cx.param(name, range) {
                return t;
            }
            if let Some(prim) = primitive(name) {
                return prim;
            }
            // A generic alias used bare (no `<Args>`) is never a valid type:
            // it isn't fully applied, so there's nothing to substitute `T`
            // with. Caught here (before the plain alias-map fallback) so it
            // gets a targeted message instead of silently resolving through
            // `cx.type_aliases`' placeholder entry for a generic alias's scope
            // symbol.
            match cx.alias(name) {
                Some(AliasDef::Generic(..)) => {
                    diags.push(Diagnostic::error(
                        "WS002",
                        format!("'{name}' is a generic type; write '{name}<...>'"),
                        range.clone(),
                    ));
                    return Type::Any;
                }
                Some(AliasDef::Resolved(t)) => return t,
                Some(AliasDef::Body(body)) => {
                    if depth >= MAX_ALIAS_DEPTH {
                        diags.push(Diagnostic::error(
                            "WS002",
                            format!("recursive type alias '{name}'"),
                            range.clone(),
                        ));
                        return Type::Any;
                    }
                    let none = HashMap::default();
                    let scope = cx.body_scope(&none, &[]);
                    return resolve_type_at_depth(
                        body,
                        scope.as_ref(),
                        diags,
                        depth + 1,
                        in_progress,
                    );
                }
                None => {}
            }
            if let Some(t) = cx.enum_named(name, Vec::new()) {
                return t;
            }
            diags.push(Diagnostic::error(
                "WS002",
                format!("unknown type '{name}'"),
                range.clone(),
            ));
            Type::Any
        }
        TypeExpr::Ref { inner, range } => {
            let resolved = resolve_type_at_depth(inner, cx, diags, depth, in_progress);
            if matches!(resolved, Type::Ref(_)) {
                diags.push(Diagnostic::error(
                    "WS086",
                    format!(
                        "'*{resolved}' is a ref to a ref, which has no storage in the game: \
                         hold the ref itself, a '{resolved}'"
                    ),
                    range.clone(),
                ));
            }
            distribute_ref(resolved)
        }
        TypeExpr::Array { inner, .. } => Type::Array(Box::new(resolve_type_at_depth(
            inner,
            cx,
            diags,
            depth,
            in_progress,
        ))),
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
                .map(|f| {
                    (
                        f.name.clone(),
                        resolve_type_at_depth(&f.typ, cx, diags, depth, in_progress),
                    )
                })
                .collect(),
        ),
        TypeExpr::Generic { name, args, range } => {
            if let Some(AliasDef::Generic(params, body)) = cx.alias(name) {
                if args.len() != params.len() {
                    diags.push(Diagnostic::error(
                        "WS002",
                        format!(
                            "'{name}' expects {} type argument(s), got {}",
                            params.len(),
                            args.len()
                        ),
                        range.clone(),
                    ));
                    for a in args {
                        resolve_type_at_depth(a, cx, diags, depth, in_progress);
                    }
                    return Type::Any;
                }
                // Resolve each argument in the outer ctx (args are the caller's
                // types, not the alias body's); this is also the identity
                // cycle detection keys on.
                let resolved_args: Vec<Type> = args
                    .iter()
                    .map(|a| resolve_type_at_depth(a, cx, diags, depth, in_progress))
                    .collect();
                // Cycle guard: this exact instantiation is already expanding
                // higher on the path, so it is self-referential; stop. Handles a
                // single self-ref (`L<T> = { tail: L<T> }`) and a
                // doubly-self-referencing body (`Tree<T> = { l: Tree<T>, r:
                // Tree<T> }`) which a depth counter alone lets blow up
                // exponentially. The `depth` cap below is a backstop for any
                // pathological non-cyclic nesting.
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
                // body in the scope the alias is declared in, where those
                // bindings stand for its parameters.
                let bindings: HashMap<String, Type> = params
                    .iter()
                    .cloned()
                    .zip(resolved_args.iter().cloned())
                    .collect();
                let scope = cx.body_scope(&bindings, params);
                in_progress.push((name.clone(), resolved_args));
                let result =
                    resolve_type_at_depth(body, scope.as_ref(), diags, depth + 1, in_progress);
                in_progress.pop();
                return result;
            }
            if let Some(t) = enum_args(cx, name, args, diags, depth, in_progress) {
                return t;
            }
            match (name.as_str(), args.as_slice()) {
                ("Array", [v]) => Type::Array(Box::new(resolve_type_at_depth(
                    v,
                    cx,
                    diags,
                    depth,
                    in_progress,
                ))),
                ("Ref", [v]) => Type::Ref(Box::new(resolve_type_at_depth(
                    v,
                    cx,
                    diags,
                    depth,
                    in_progress,
                ))),
                // `Component<PointLight>`: a reference to a component of that type.
                (
                    "Component",
                    [
                        TypeExpr::Name {
                            name: c,
                            range: c_range,
                        },
                    ],
                ) => match crate::catalog::component_type(c) {
                    Some(o) => Type::Component(o.class.to_string()),
                    None => {
                        diags.push(Diagnostic::error(
                                "WS110",
                                format!("`Component<{c}>`: `{c}` is not a component type the component gates take"),
                                c_range.clone(),
                            ));
                        Type::Any
                    }
                },
                ("Map", [k, v]) => {
                    let key = resolve_type_at_depth(k, cx, diags, depth, in_progress);
                    let val = resolve_type_at_depth(v, cx, diags, depth, in_progress);
                    // A map is keyed by a hashed slot, so only int, string,
                    // object (entity/character/controller) and brick keys have
                    // a representation. A generic param key (`Map<K, V>`) is
                    // validated per concrete instantiation; an already-errored
                    // `any` key is left alone.
                    if !matches!(
                        key,
                        Type::Int
                            | Type::String
                            | Type::Entity
                            | Type::Character
                            | Type::Controller
                            | Type::Brick
                            | Type::Any
                            | Type::Param(_)
                    ) {
                        diags.push(Diagnostic::error(
                            "WS039",
                            format!(
                                "map key type must be int, string, an object \
                                 (entity/character/controller) or a brick, got {key}"
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

/// The enum `name` applied to `args`, when the table names one: its
/// arguments resolved only then.
fn enum_args(
    cx: &dyn TypeNames,
    name: &str,
    args: &[TypeExpr],
    diags: &mut Vec<Diagnostic>,
    depth: u32,
    in_progress: &mut Vec<(String, Vec<Type>)>,
) -> Option<Type> {
    cx.enum_named(name, Vec::new())?;
    let args = args
        .iter()
        .map(|a| resolve_type_at_depth(a, cx, diags, depth, in_progress))
        .collect();
    cx.enum_named(name, args)
}

#[cfg(test)]
mod tests;
