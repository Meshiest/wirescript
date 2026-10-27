//! Annotation-position type resolution and the predicates that guard what a
//! declaration may store.

use super::*;
use crate::analysis::types::type_str;
use crate::types::resolve::{GenericAlias, primitive};

// Type expression resolution
//
// `any` in an annotation is the wildcard `Type::Opaque` (what `Opaque(...)`
// produces), not the error-fallback `Type::Any`: it can flow through ports,
// lets and parameters but cannot back a var, array or buffer's storage gate
// (`reject_any_storage`). The mapping is in `types::resolve::primitive`.

/// Resolve a type annotation against the current scope through
/// `types::resolve::resolve_type`, with the scope's visible `SymbolKind::Type`
/// aliases snapshotted as `type_aliases` (the scope does not change while one
/// type expression resolves); its WS002 diagnostics go to `ctx.diagnostics`.
pub(super) fn resolve_type_expr(ctx: &mut TypeCheckCtx<'_>, t: &TypeExpr) -> Type {
    reserve_record_fields(ctx, t);
    // A primitive name needs no alias snapshot: `resolve_type` checks primitives
    // before aliases, so this skips building the in-scope alias map (a frame scan
    // and deep clones) for the common annotation, including per combination in a
    // generic body.
    if let TypeExpr::Name { name, .. } = t
        && let Some(prim) = primitive(name)
    {
        return prim;
    }
    // A known non-generic enum name resolves to `Type::Enum` before the alias
    // fallback. `ctx.enum_defs` is filled before any declaration registers, so
    // this works wherever the `enum` is declared (scope registration order would
    // not). A generic enum falls through to the alias and WS002 path.
    if let TypeExpr::Name { name, range } = t
        && let Some(def) = ctx.enum_defs.written_name(name)
        && def.type_params.is_empty()
        && !names_a_type_parameter(ctx, name)
        && enum_visible_in(ctx, name, &range.file)
    {
        return Type::Enum {
            name: name.clone(),
            args: vec![],
        };
    }
    // A known enum with type arguments (`Option<int>`) resolves to
    // `Type::Enum { args }`, arity-checked against its type parameters as generic
    // aliases are. Done here because `types::resolve` has no enum registry.
    // `Array`, `Map`, `Ref` and generic aliases are not enums and go to the
    // canonical resolver.
    if let TypeExpr::Generic { name, args, range } = t
        && let Some(arity) = ctx
            .enum_defs
            .written_name(name)
            .map(|d| d.type_params.len())
        && enum_visible_in(ctx, name, &range.file)
    {
        let resolved_args: Vec<Type> = args.iter().map(|a| resolve_type_expr(ctx, a)).collect();
        if resolved_args.len() != arity {
            ctx.emit(
                "WS002",
                format!(
                    "`{name}` expects {arity} type argument(s), got {}",
                    resolved_args.len()
                ),
                range.clone(),
            );
            return Type::Enum {
                name: name.clone(),
                args: vec![Type::Any; arity],
            };
        }
        // An argument that needs container storage would land in a payload
        // slot, which cannot hold one (see `enums::check_payload_storage`).
        // The declaration check can't catch this: there the field is the bare
        // parameter. Reported here, at the instantiation that picks the type.
        if let Some(def) = ctx.enum_defs.written_name(name).cloned()
            && let Some(param) =
                crate::typecheck::enums::instantiation_stores_container(&def, &resolved_args)
        {
            ctx.emit(
                "WS069",
                format!(
                    "`{name}<...>` binds `{param}` to a type holding an array or a map, and an enum payload cannot hold one. A payload slot is filled by constructing the variant, and there is no way to construct a container into one, so the value would read back empty. Keep the container in its own `var` and put an index, a key, or a length in the payload instead"
                ),
                range.clone(),
            );
        }
        return Type::Enum {
            name: name.clone(),
            args: resolved_args,
        };
    }
    let file = &t.range().file;
    let aliases = aliases_for(ctx, file);
    let generic = generic_aliases_for(ctx, file);
    let generic = generic.as_deref().unwrap_or(&ctx.generic_type_aliases);
    let declaring = names_generic_alias(t, generic).then(|| declaring_aliases(ctx, file));
    let mut diags = unknown_in_own_aliases(ctx, file, t, &aliases);
    let result = {
        let cx = crate::types::resolve::ResolveCtx {
            params: &[],
            type_aliases: &aliases,
            generic_aliases: generic,
            declaring_aliases: declaring.as_ref(),
        };
        crate::types::resolve::resolve_type(t, &cx, &mut diags)
    };
    // One annotation can be resolved more than once (a mod parameter, for its
    // signature and again for its body): report each problem in it once.
    for d in diags {
        if !ctx
            .diagnostics
            .iter()
            .any(|e| e.code == d.code && e.range == d.range && e.message == d.message)
        {
            ctx.diagnostics.push(d);
        }
    }
    result
}

/// Whether `name` is, where it is read, a type parameter (or what one is bound
/// to) rather than the enum of that name, which the parameter hides.
fn names_a_type_parameter(ctx: &TypeCheckCtx<'_>, name: &str) -> bool {
    ctx.scope.lookup(name).is_some_and(|s| {
        s.kind == SymbolKind::Type && !matches!(&s.ty, Type::Enum { name: n, .. } if n == name)
    })
}

/// A type parameter's mask: the concrete types it may be inferred to. Unbound
/// (`T`) is the full `Variant` mask; a bound narrows it
/// (`types::mono::mask_for_param`).
pub(super) fn type_param_mask(ctx: &TypeCheckCtx<'_>, tp: &TypeParam) -> Vec<Type> {
    // The bound -> mask resolution lives in the shared `types::mono` module so
    // the lowering-side monomorphizer can rebuild the same masks.
    crate::types::mono::mask_for_param(tp.bound.as_ref(), &quiet_resolver(ctx, &tp.range.file))
}

/// What a type written in `file` names, every alias read as `file` reads it,
/// with no diagnostics: for a pass that reads annotations [`resolve_type_expr`]
/// has already reported on. The aliases are gathered (an O(scope) scan) only
/// once a name that is not a primitive is resolved.
pub(super) fn quiet_resolver<'c>(
    ctx: &'c TypeCheckCtx<'_>,
    file: &Arc<str>,
) -> impl Fn(&TypeExpr) -> Type + 'c {
    type Views = (
        Arc<HashMap<String, Type>>,
        Option<Arc<HashMap<String, GenericAlias>>>,
    );
    let file = std::sync::Arc::clone(file);
    let views: std::cell::OnceCell<Views> = std::cell::OnceCell::new();
    move |te: &TypeExpr| {
        if let TypeExpr::Name { name, .. } = te
            && let Some(prim) = primitive(name)
        {
            return prim;
        }
        let (aliases, generic) =
            views.get_or_init(|| (aliases_for(ctx, &file), generic_aliases_for(ctx, &file)));
        let cx = crate::types::resolve::ResolveCtx {
            params: &[],
            type_aliases: aliases,
            generic_aliases: generic.as_deref().unwrap_or(&ctx.generic_type_aliases),
            declaring_aliases: None,
        };
        crate::types::resolve::resolve_type(te, &cx, &mut Vec::new())
    }
}

/// What an imported file can name of the type aliases: the ones it declares
/// or its own imports bring in, resolved where they are declared, and the
/// view a type written there was last resolved against.
#[derive(Default)]
pub(crate) struct FileAliases {
    own: Arc<HashMap<String, Type>>,
    generic: Arc<HashMap<String, GenericAlias>>,
    /// The `WS002` each of its own aliases' bodies has, by alias: a name the
    /// file can name as no type (also through another of its aliases).
    unknown: Arc<HashMap<String, Vec<Diagnostic>>>,
    view: Option<(u64, Arc<HashMap<String, Type>>)>,
}

/// The aliases and enums a type written in `file` reads its names against.
/// The program's own file reads the scope as it stands. An imported file reads
/// the type parameters in scope there and the aliases it can name itself
/// (see [`FileAliases`]), never one its importer declares, so a library's
/// `F` is the library's whatever the program calls `F`, and a name the
/// library never declared is unknown there.
pub(super) fn aliases_for(ctx: &TypeCheckCtx<'_>, file: &Arc<str>) -> Arc<HashMap<String, Type>> {
    if !ctx.imports.aliases.contains_key(file) {
        return ctx.scope.type_aliases();
    }
    let generation = ctx.scope.alias_generation();
    let mut cache = ctx.file_aliases.borrow_mut();
    let entry = file_entry(ctx, &mut cache, file);
    if let Some((at, view)) = &entry.view
        && *at == generation
    {
        return std::sync::Arc::clone(view);
    }
    let base = ctx.scope.type_aliases();
    let stack = ctx.scope.stack();
    let declared = |name: &str| {
        let sym = crate::intern::intern(name);
        (0..stack.depth())
            .rev()
            .find_map(|i| stack.get_in(i, sym).filter(|s| s.kind == SymbolKind::Type))
    };
    let mut view: HashMap<String, Type> = HashMap::default();
    for (name, ty) in base.iter() {
        // An enum is named as an alias is: where the file declares it or its
        // imports bring it, and never over one of the file's own aliases.
        let own = entry.own.contains_key(name) || entry.generic.contains_key(name);
        let visible = matches!(ty, Type::Param(_))
            || name.contains('.')
            || !own
                && declared(name).is_none_or(|s| {
                    s.decl_range.file.is_empty()
                        || s.decl_range.file == *file
                        || ctx.imports.sees(file, &s.decl_range)
                });
        if visible {
            view.insert(name.clone(), ty.clone());
        }
    }
    for (name, ty) in entry.own.iter() {
        view.entry(name.clone()).or_insert_with(|| ty.clone());
    }
    let view = Arc::new(view);
    entry.view = Some((generation, std::sync::Arc::clone(&view)));
    view
}

/// Whether `name`, written in `file`, names the enum of that name. The
/// program's own file names every enum. An imported file names the enums it
/// declares and those its imports bring, as it does aliases, unless one of
/// its own aliases takes the name.
pub(super) fn enum_visible_in(ctx: &TypeCheckCtx<'_>, name: &str, file: &Arc<str>) -> bool {
    if !ctx.imports.aliases.contains_key(file) {
        return true;
    }
    {
        let mut cache = ctx.file_aliases.borrow_mut();
        let entry = file_entry(ctx, &mut cache, file);
        if entry.own.contains_key(name) || entry.generic.contains_key(name) {
            return false;
        }
    }
    let sym = crate::intern::intern(name);
    let stack = ctx.scope.stack();
    let declared = (0..stack.depth())
        .rev()
        .find_map(|i| stack.get_in(i, sym).filter(|s| s.kind == SymbolKind::Type));
    declared.is_none_or(|s| {
        s.decl_range.file.is_empty()
            || s.decl_range.file == *file
            || ctx.imports.sees(file, &s.decl_range)
    })
}

/// The generic aliases a type written in `file` can name, when `file` is an
/// imported file; `None` for the program's own, which reads them all.
pub(super) fn generic_aliases_for(
    ctx: &TypeCheckCtx<'_>,
    file: &Arc<str>,
) -> Option<Arc<HashMap<String, GenericAlias>>> {
    if !ctx.imports.aliases.contains_key(file) {
        return None;
    }
    let mut cache = ctx.file_aliases.borrow_mut();
    Some(std::sync::Arc::clone(
        &file_entry(ctx, &mut cache, file).generic,
    ))
}

fn file_entry<'c>(
    ctx: &TypeCheckCtx<'_>,
    cache: &'c mut HashMap<Arc<str>, FileAliases>,
    file: &Arc<str>,
) -> &'c mut FileAliases {
    cache.entry(std::sync::Arc::clone(file)).or_insert_with(|| {
        let decls = ctx
            .imports
            .aliases
            .get(file)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let types = crate::const_eval::DeclaredTypes::for_program(decls, &ctx.enum_defs);
        FileAliases {
            own: Arc::new(types.plain_aliases().collect()),
            generic: Arc::new(types.generic_aliases().collect()),
            unknown: Arc::new(unknown_alias_names(ctx, decls, &types)),
            view: None,
        }
    })
}

/// For each alias `decls` declares (an `import * as` module's by its
/// qualified name), the `WS002` for each name its body writes that is no type
/// the file can name: not a builtin type, one of its aliases or type
/// parameters, or an enum. An alias naming such an alias has its errors too.
fn unknown_alias_names(
    ctx: &TypeCheckCtx<'_>,
    decls: &[TopDecl],
    types: &crate::const_eval::DeclaredTypes,
) -> HashMap<String, Vec<Diagnostic>> {
    fn walk(
        t: &TypeExpr,
        params: &[String],
        known: &dyn Fn(&str) -> bool,
        unknown: &HashMap<String, Vec<Diagnostic>>,
        out: &mut Vec<Diagnostic>,
    ) {
        let name_at = |name: &str, range: &SourceRange, out: &mut Vec<Diagnostic>| {
            if let Some(inner) = unknown.get(name) {
                out.extend(inner.iter().cloned());
            } else if !params.iter().any(|p| p == name) && !known(name) {
                out.push(Diagnostic::error(
                    "WS002",
                    format!("unknown type '{name}'"),
                    range.clone(),
                ));
            }
        };
        match t {
            TypeExpr::Name { name, range } => name_at(name, range, out),
            TypeExpr::Generic { name, args, range } => {
                if !matches!(name.as_str(), "Map" | "Array" | "Ref") {
                    name_at(name, range, out);
                }
                for a in args {
                    walk(a, params, known, unknown, out);
                }
            }
            TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
                walk(inner, params, known, unknown, out);
            }
            TypeExpr::Tuple { fields, .. } => fields
                .iter()
                .for_each(|f| walk(f, params, known, unknown, out)),
            TypeExpr::Union { options, .. } => options
                .iter()
                .for_each(|o| walk(o, params, known, unknown, out)),
            TypeExpr::Record { fields, .. } => fields
                .iter()
                .for_each(|f| walk(&f.typ, params, known, unknown, out)),
        }
    }
    let known = |name: &str| {
        primitive(name).is_some()
            || types.alias(name).is_some()
            || ctx.enum_defs.written_name(name).is_some()
    };
    let mut unknown: HashMap<String, Vec<Diagnostic>> = HashMap::default();
    let each = |key: String,
                t: &crate::ast::TypeAliasDecl,
                unknown: &mut HashMap<String, Vec<Diagnostic>>| {
        let params: Vec<String> = t.type_params.iter().map(|p| p.name.clone()).collect();
        let mut out = Vec::new();
        walk(&t.typ, &params, &known, unknown, &mut out);
        if !out.is_empty() {
            unknown.insert(key, out);
        }
    };
    for d in decls {
        match d {
            TopDecl::TypeAlias(t) => each(t.name.clone(), t, &mut unknown),
            TopDecl::Namespace(ns) => {
                for m in &ns.decls {
                    if let TopDecl::TypeAlias(t) = m {
                        each(format!("{}.{}", ns.name, t.name), t, &mut unknown);
                    }
                }
            }
            _ => {}
        }
    }
    unknown
}

/// The errors in the bodies of the aliases of `file` (an imported file) that
/// `t`, written there, names through `aliases` (see [`unknown_alias_names`]).
fn unknown_in_own_aliases(
    ctx: &TypeCheckCtx<'_>,
    file: &Arc<str>,
    t: &TypeExpr,
    aliases: &HashMap<String, Type>,
) -> Vec<Diagnostic> {
    if !ctx.imports.aliases.contains_key(file) {
        return Vec::new();
    }
    let (own, unknown) = {
        let mut cache = ctx.file_aliases.borrow_mut();
        let entry = file_entry(ctx, &mut cache, file);
        (
            std::sync::Arc::clone(&entry.own),
            std::sync::Arc::clone(&entry.unknown),
        )
    };
    if unknown.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut names = Vec::new();
    fn names_in<'t>(t: &'t TypeExpr, out: &mut Vec<&'t String>) {
        match t {
            TypeExpr::Name { name, .. } => out.push(name),
            TypeExpr::Generic { name, args, .. } => {
                out.push(name);
                for a in args {
                    names_in(a, out);
                }
            }
            TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => names_in(inner, out),
            TypeExpr::Tuple { fields, .. } => fields.iter().for_each(|f| names_in(f, out)),
            TypeExpr::Union { options, .. } => options.iter().for_each(|o| names_in(o, out)),
            TypeExpr::Record { fields, .. } => fields.iter().for_each(|f| names_in(&f.typ, out)),
        }
    }
    names_in(t, &mut names);
    for name in names {
        if let Some(errors) = unknown.get(name)
            && own.get(name).is_some_and(|o| aliases.get(name) == Some(o))
        {
            out.extend(errors.iter().cloned());
        }
    }
    out
}

/// Whether `t` applies a generic alias of `generic` anywhere in it.
fn names_generic_alias(t: &TypeExpr, generic: &HashMap<String, GenericAlias>) -> bool {
    if generic.is_empty() {
        return false;
    }
    match t {
        TypeExpr::Name { .. } => false,
        TypeExpr::Generic { name, args, .. } => {
            generic.contains_key(name) || args.iter().any(|a| names_generic_alias(a, generic))
        }
        TypeExpr::Array { inner, .. } | TypeExpr::Ref { inner, .. } => {
            names_generic_alias(inner, generic)
        }
        TypeExpr::Tuple { fields, .. } => fields.iter().any(|f| names_generic_alias(f, generic)),
        TypeExpr::Union { options, .. } => options.iter().any(|o| names_generic_alias(o, generic)),
        TypeExpr::Record { fields, .. } => {
            fields.iter().any(|f| names_generic_alias(&f.typ, generic))
        }
    }
}

/// The aliases a type written in `file` reads at the top level, where a
/// generic alias's body is written: none of the type parameters of the body
/// being checked, which hide a top-level alias of their name only where that
/// body writes them.
fn declaring_aliases(ctx: &TypeCheckCtx<'_>, file: &Arc<str>) -> HashMap<String, Type> {
    let mut top: HashMap<String, Type> = ctx
        .scope
        .stack()
        .frame_entries(0)
        .filter(|(_, s)| s.kind == SymbolKind::Type)
        .filter(|(_, s)| {
            !ctx.imports.aliases.contains_key(file)
                || matches!(&s.ty, Type::Enum { name, .. } if *name == s.name)
                || s.decl_range.file.is_empty()
                || s.decl_range.file == *file
                || ctx.imports.sees(file, &s.decl_range)
        })
        .map(|(_, s)| (s.name.clone(), s.ty.clone()))
        .collect();
    if ctx.imports.aliases.contains_key(file) {
        let mut cache = ctx.file_aliases.borrow_mut();
        for (name, ty) in file_entry(ctx, &mut cache, file).own.iter() {
            top.entry(name.clone()).or_insert_with(|| ty.clone());
        }
    }
    top
}

/// The cartesian product of per-parameter masks: each inner `Vec<Type>` of
/// `masks` is one parameter's candidates, and the result has one `Vec<Type>`
/// per combination, aligned with `masks`. It materializes the whole product, so
/// callers cap its size first (`MAX_BODY_CHECK_COMBOS`).
pub(super) fn cartesian_product(masks: &[Vec<Type>]) -> Vec<Vec<Type>> {
    let mut result: Vec<Vec<Type>> = vec![Vec::new()];
    for mask in masks {
        let mut next = Vec::with_capacity(result.len() * mask.len());
        for combo in &result {
            for ty in mask {
                let mut extended = combo.clone();
                extended.push(ty.clone());
                next.push(extended);
            }
        }
        result = next;
    }
    result
}

/// The type of a constant literal expression, to infer an unannotated var's
/// (or array var's element) type at registration, before the type map exists.
/// `None` for anything that is not a compile-time literal.
pub(super) fn literal_expr_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::IntLit { .. } => Some(Type::Int),
        Expr::AtomLit { .. } => Some(Type::Int),
        Expr::FloatLit { .. } => Some(Type::Float),
        Expr::BoolLit { .. } => Some(Type::Bool),
        Expr::StringLit { .. } | Expr::InterpLit { .. } => Some(Type::String),
        Expr::UnOp { op, operand, .. } if op == "-" => literal_expr_type(operand),
        // A constructor call types by its name alone; whether its value folds depends
        // on its arguments, but the type does not.
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident { name, .. } => match name.as_str() {
                "Vec" => Some(Type::Vector),
                "Rotation" => Some(Type::Rotator),
                "Color" | "ColorSRGB" | "ColorHex" => Some(Type::Color),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// The types a Variable gate can hold as a wire variant: the safe targets for
/// refining an unannotated `var`'s placeholder `any` from its initializer.
pub(super) fn var_storable(t: &Type) -> bool {
    matches!(
        t,
        Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Vector
            | Type::Rotator
            | Type::Quat
            | Type::Color
            | Type::Entity
            | Type::Character
            | Type::Controller
            | Type::Brick
            | Type::Component(_)
    ) || matches!(t, Type::Ref(inner) if inner.is_ref_target())
}

/// Whether a `var` written with no type holds a value of type `t` as that
/// type: one it stores, or a record or tuple, held field by field.
pub(super) fn unannotated_var_holds(t: &Type) -> bool {
    var_storable(t) || matches!(t, Type::Record(_) | Type::Tuple(_))
}

pub(super) fn type_expr_range(t: &TypeExpr) -> SourceRange {
    t.range().clone()
}

/// The types a var/array/map/buffer's storage gate cannot hold (WS025). `any`
/// (`Type::Opaque`) is storable: a var already holds a `WireValue`, and an
/// `any` array or map stores `WireValue` elements. The reference types
/// (`zone`/`teleport`/`prefab`) are not: like a var ref, they can only be
/// wired or rerouted. Only fires on an *explicit* annotation.
pub(super) fn reject_any_storage(
    ctx: &mut TypeCheckCtx<'_>,
    resolved: &Type,
    range: SourceRange,
    what: &str,
) {
    match resolved {
        Type::Zone | Type::Teleport | Type::PrefabRef => {
            let n = type_str(resolved);
            ctx.emit(
                "WS025",
                format!(
                    "'{n}' is a reference and cannot be stored: {what} needs a concrete \
                     wire type; a '{n}' can only be wired or rerouted (like a var ref)"
                ),
                range,
            );
        }
        // An exec edge is control flow, not a value: `var c: exec` wires an
        // exec rerouter's output into the storage gate's data `.Value` pin,
        // which the game will not load.
        Type::Exec => ctx.emit(
            "WS025",
            format!(
                "'exec' is control flow and cannot be stored: {what} needs a value type, \
                 and an exec edge runs a gate rather than carrying one"
            ),
            range,
        ),
        // `never` is the type of an expression that produces nothing, and has
        // no wire variant, so the gate falls back to a number.
        Type::Never => ctx.emit(
            "WS025",
            format!("'never' cannot be stored: {what} needs a concrete wire type"),
            range,
        ),
        // A `**T` is reported where the type resolves (WS086).
        Type::Ref(inner) if matches!(inner.as_ref(), Type::Ref(_)) => {}
        // An array ref is already a reference, so an array of array refs is
        // `int[][]`; `(*int[])[]` would name the same value twice over.
        Type::Ref(inner)
            if matches!(inner.as_ref(), Type::Array(_) | Type::Map(_, _)) && nested(what) =>
        {
            let n = type_str(resolved);
            let plain = type_str(inner);
            ctx.emit(
                "WS025",
                format!("'{n}' cannot be an element: an array or map is already a ref, so write '{plain}'"),
                range,
            );
        }
        // A `*T` names one var, so `T` must be a type one var gate holds, or a
        // container (a var holding a container ref).
        Type::Ref(inner)
            if !inner.is_ref_target()
                && !matches!(
                    inner.as_ref(),
                    Type::Array(_) | Type::Map(_, _) | Type::Any | Type::Opaque | Type::Param(_)
                ) =>
        {
            let n = type_str(resolved);
            ctx.emit(
                "WS025",
                format!(
                    "'{n}' cannot be stored: a ref names one var, and no var holds a '{inner}'"
                ),
                range,
            );
        }
        // A container is storable as an array element or map value: the slot
        // holds its ArrayVarRef / MapVarRef. A container of records, tuples or
        // enums is several parallel containers with no single ref.
        Type::Array(elem) | Type::Map(_, elem) if nested(what) => {
            if let Type::Map(key, _) = resolved {
                reject_ref_key(ctx, key, range.clone());
            }
            if matches!(
                elem.as_ref(),
                Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }
            ) {
                let n = type_str(resolved);
                let (kind, parts) = match elem.as_ref() {
                    Type::Record(_) => ("records", "one container per field"),
                    Type::Tuple(_) => ("tuples", "one container per element"),
                    _ => (
                        "enums",
                        "one container for the tag and one per payload field",
                    ),
                };
                ctx.emit(
                    "WS025",
                    format!(
                        "'{n}' cannot be nested: its elements are {kind}, stored as {parts}, \
                         so there is no single ref to hold"
                    ),
                    range,
                );
            } else {
                let inner_what = if matches!(resolved, Type::Array(_)) {
                    "an array's element type"
                } else {
                    "a map's value type"
                };
                reject_any_storage(ctx, elem, range, inner_what);
            }
        }
        _ => {}
    }
}

fn nested(what: &str) -> bool {
    what == "an array's element type" || what == "a map's value type"
}

/// A map key is hashed by value, and a var, array or map ref has none to hash.
/// A brick is the exception: the game keys a map by the brick it names.
pub(super) fn reject_ref_key(ctx: &mut TypeCheckCtx<'_>, key: &Type, range: SourceRange) {
    if key
        .ref_kind()
        .is_some_and(|k| k != crate::ir::RefKind::Brick)
    {
        let n = type_str(key);
        ctx.emit(
            "WS025",
            format!("'{n}' cannot be a map key: a key needs a plain value (int, string, bool or an object)"),
            range,
        );
    }
}

/// True if `t` is or contains the wildcard `any` (`Type::Opaque`): `*any`,
/// `any[]`, `Map<_, any>` and `{ a: any }` wrap it, so this recurses through
/// every compound and `warn_any_annotation` fires however deep it is.
fn contains_opaque(t: &Type) -> bool {
    match t {
        Type::Opaque => true,
        Type::Ref(inner) | Type::Array(inner) => contains_opaque(inner),
        Type::Map(k, v) => contains_opaque(k) || contains_opaque(v),
        Type::Tuple(fields) | Type::Union(fields) => fields.iter().any(contains_opaque),
        Type::Record(fields) => fields.iter().any(|(_, ft)| contains_opaque(ft)),
        _ => false,
    }
}

/// True if `t` is or contains a `Type::Param`, an unresolved generic type
/// parameter. A call parameter still carrying one is left to WS033, not the
/// concrete argument coercion.
pub(crate) fn type_has_param(t: &Type) -> bool {
    match t {
        Type::Param(_) => true,
        Type::Ref(inner) | Type::Array(inner) => type_has_param(inner),
        Type::Map(k, v) => type_has_param(k) || type_has_param(v),
        Type::Tuple(fields) | Type::Union(fields) => fields.iter().any(type_has_param),
        Type::Record(fields) => fields.iter().any(|(_, ft)| type_has_param(ft)),
        _ => false,
    }
}

/// Warns when a non-storage annotation (a mod or chip parameter or output, a
/// handler's typed destructure parameter, a `let`, a `type` alias body)
/// resolves to `any`, wrapped or not (`*any`, `any[]`, `Map<_, any>`): it works
/// as a wildcard, but a type parameter would keep the type. Once per
/// annotation, at its range. Storage positions (var, array, buffer, map) are
/// WS025 errors instead (`reject_any_storage`), and `in`/`out` ports are skipped,
/// since a port cannot take a type parameter.
pub(super) fn warn_any_annotation(ctx: &mut TypeCheckCtx<'_>, resolved: &Type, range: SourceRange) {
    if contains_opaque(resolved) {
        ctx.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: "WS032".into(),
            message: "'any' works, but prefer a generic type parameter here: it lets the type \
                       flow instead of erasing it"
                .into(),
            range,
        });
    }
}

/// Resolve the annotation `te`, with [`warn_any_annotation`]'s WS032.
pub(super) fn resolve_annotation(ctx: &mut TypeCheckCtx<'_>, te: &TypeExpr) -> Type {
    let ty = resolve_type_expr(ctx, te);
    warn_any_annotation(ctx, &ty, type_expr_range(te));
    ty
}

/// Note every record field name in `t` for WS093 (`TypeCheckCtx::reserve_check`).
fn reserve_record_fields(ctx: &mut TypeCheckCtx<'_>, t: &TypeExpr) {
    match t {
        TypeExpr::Record { fields, .. } => {
            for f in fields {
                ctx.reserve_check(&f.name, &f.range);
                reserve_record_fields(ctx, &f.typ);
            }
        }
        TypeExpr::Array { inner, .. } | TypeExpr::Ref { inner, .. } => {
            reserve_record_fields(ctx, inner);
        }
        TypeExpr::Tuple { fields, .. } => fields.iter().for_each(|f| reserve_record_fields(ctx, f)),
        TypeExpr::Union { options, .. } => {
            for f in options {
                reserve_record_fields(ctx, f);
            }
        }
        TypeExpr::Generic { args, .. } => args.iter().for_each(|f| reserve_record_fields(ctx, f)),
        TypeExpr::Name { .. } => {}
    }
}
