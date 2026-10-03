//! Annotation-position type resolution and the predicates that guard what a
//! declaration may store.

use super::*;

// ---------- type expression resolution ----------
//
// `any` (in annotation position) maps onto the *wildcard* type
// (`Type::Opaque`, the same type `Opaque(...)` produces) rather than the
// internal `Type::Any` error-fallback — see `reject_any_storage` below for
// why: `any` can flow through ports/lets/params but can't back a
// var/array/buffer's storage gate. That mapping lives in the crate's single
// primitive-name table, `types::resolve::primitive`.

/// Resolve a type annotation against the current scope. Delegates to the
/// crate's single canonical resolver (`types::resolve::resolve_type`): the
/// scope's currently-visible `SymbolKind::Type` aliases are snapshotted (scope
/// doesn't mutate during a type expression's resolution, so this is
/// equivalent to the old per-`Name`-node live `ctx.scope.lookup`) and handed
/// in as `type_aliases`, and any WS002 the resolver raises is forwarded to
/// `ctx.diagnostics` — preserving both the alias-resolution and diagnostic
/// behavior of the resolver this replaced.
pub(super) fn resolve_type_expr(ctx: &mut TypeCheckCtx, t: &TypeExpr) -> Type {
    reserve_record_fields(ctx, t);
    // Fast path: a primitive name needs no alias snapshot. Primitives always win
    // over aliases in `resolve_type` (it checks `primitive` before the alias
    // map), so this is behaviour-identical — but it skips building the full
    // in-scope alias map (an O(D) frame scan + deep `Type` clones) on the
    // overwhelmingly common case, where annotations are `int`/`float`/`bool`/…
    // Without it that snapshot was rebuilt for every annotation, incl. per
    // cartesian combo inside a generic body — an accidental O(D²).
    if let TypeExpr::Name { name, .. } = t
        && let Some(prim) = crate::types::resolve::primitive(name)
    {
        return prim;
    }
    // A known non-generic enum name resolves directly to `Type::Enum`,
    // ahead of the alias-map fallback below. `ctx.enum_defs` is populated
    // by the `collect_enum_defs` pre-pass before any decl is registered, so
    // this resolves regardless of where the `enum` itself sits in the file
    // (the scope-declared `SymbolKind::Type` symbol registration order is
    // NOT a safe substitute for that). A generic enum (`enum Option<T>`)
    // falls through to the ordinary alias/WS002 path here: instantiating
    // it against concrete type args is a later phase, not this fast path.
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
    // A known enum applied to type arguments (`Option<int>`) resolves to a
    // concrete `Type::Enum { args }`, arity-checked against the enum's declared
    // type parameters (mirrors the generic type-alias arity check in the
    // canonical resolver). Handled here rather than in `types::resolve`, which
    // carries no enum registry - same reason the non-generic enum name resolves
    // in this fast path above. `Array`/`Map`/`Ref` and a generic type alias
    // aren't enums, so they fall through to the canonical resolver unchanged.
    if let TypeExpr::Generic { name, args, range } = t
        && let Some(arity) = ctx.enum_defs.written_name(name).map(|d| d.type_params.len())
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
            return Type::Enum { name: name.clone(), args: vec![Type::Any; arity] };
        }
        // An argument that needs container storage would land in a payload
        // slot, which cannot hold one (see `enums::check_payload_storage`).
        // The declaration check can't catch this: there the field is the bare
        // parameter. Reported here, at the instantiation that picks the type.
        if let Some(def) = ctx.enum_defs.written_name(name).cloned()
            && let Some(param) = crate::typecheck::enums::instantiation_stores_container(
                &def,
                &resolved_args,
            )
        {
            ctx.emit(
                "WS069",
                format!(
                    "`{name}<...>` binds `{param}` to a type holding an array or a map, and an enum payload cannot hold one. A payload slot is filled by constructing the variant, and there is no way to construct a container into one, so the value would read back empty. Keep the container in its own `var` and put an index, a key, or a length in the payload instead"
                ),
                range.clone(),
            );
        }
        return Type::Enum { name: name.clone(), args: resolved_args };
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
        if !ctx.diagnostics.iter().any(|e| e.code == d.code && e.range == d.range && e.message == d.message) {
            ctx.diagnostics.push(d);
        }
    }
    result
}

/// Whether `name` is, where it is read, a type parameter (or what one is bound
/// to) rather than the enum of that name, which the parameter hides.
fn names_a_type_parameter(ctx: &TypeCheckCtx, name: &str) -> bool {
    ctx.scope
        .lookup(name)
        .is_some_and(|s| s.kind == SymbolKind::Type && !matches!(&s.ty, Type::Enum { name: n, .. } if n == name))
}

/// A type param's mask — the set of concrete types it may be inferred to.
/// Unbound (`T`, no `: Bound`) is the full `Variant` mask; a bound narrows it
/// (see `types::mono::mask_for_param`).
pub(super) fn type_param_mask(ctx: &TypeCheckCtx, tp: &TypeParam) -> Vec<Type> {
    // The bound → mask resolution lives in the shared `types::mono` module so
    // the lowering-side monomorphizer can rebuild the same masks.
    crate::types::mono::mask_for_param(tp.bound.as_ref(), &quiet_resolver(ctx, &tp.range.file))
}

/// What a type written in `file` names, every alias read as `file` reads it,
/// with no diagnostics: for a pass that reads annotations [`resolve_type_expr`]
/// has already reported on. The aliases are gathered (an O(scope) scan) only
/// once a name that is not a primitive is resolved.
pub(super) fn quiet_resolver<'c>(ctx: &'c TypeCheckCtx, file: &Arc<str>) -> impl Fn(&TypeExpr) -> Type + 'c {
    type Views = (Arc<HashMap<String, Type>>, Option<Arc<HashMap<String, crate::types::resolve::GenericAlias>>>);
    let file = file.clone();
    let views: std::cell::OnceCell<Views> = std::cell::OnceCell::new();
    move |te: &TypeExpr| {
        if let TypeExpr::Name { name, .. } = te
            && let Some(prim) = crate::types::resolve::primitive(name)
        {
            return prim;
        }
        let (aliases, generic) = views.get_or_init(|| (aliases_for(ctx, &file), generic_aliases_for(ctx, &file)));
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
    generic: Arc<HashMap<String, crate::types::resolve::GenericAlias>>,
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
pub(super) fn aliases_for(ctx: &TypeCheckCtx, file: &Arc<str>) -> Arc<HashMap<String, Type>> {
    if !ctx.imports.aliases.contains_key(file) {
        return ctx.scope.type_aliases();
    }
    let generation = ctx.scope.alias_generation();
    let mut cache = ctx.file_aliases.borrow_mut();
    let entry = file_entry(ctx, &mut cache, file);
    if let Some((at, view)) = &entry.view
        && *at == generation
    {
        return view.clone();
    }
    let base = ctx.scope.type_aliases();
    let stack = ctx.scope.stack();
    let declared = |name: &str| {
        let sym = crate::intern::intern(name);
        (0..stack.depth()).rev().find_map(|i| stack.get_in(i, sym).filter(|s| s.kind == SymbolKind::Type))
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
                    s.decl_range.file.is_empty() || s.decl_range.file == *file || ctx.imports.sees(file, &s.decl_range)
                });
        if visible {
            view.insert(name.clone(), ty.clone());
        }
    }
    for (name, ty) in entry.own.iter() {
        view.entry(name.clone()).or_insert_with(|| ty.clone());
    }
    let view = Arc::new(view);
    entry.view = Some((generation, view.clone()));
    view
}

/// Whether `name`, written in `file`, names the enum of that name. The
/// program's own file names every enum. An imported file names the enums it
/// declares and those its imports bring, as it does aliases, unless one of
/// its own aliases takes the name.
pub(super) fn enum_visible_in(ctx: &TypeCheckCtx, name: &str, file: &Arc<str>) -> bool {
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
    let declared =
        (0..stack.depth()).rev().find_map(|i| stack.get_in(i, sym).filter(|s| s.kind == SymbolKind::Type));
    declared.is_none_or(|s| {
        s.decl_range.file.is_empty() || s.decl_range.file == *file || ctx.imports.sees(file, &s.decl_range)
    })
}

/// The generic aliases a type written in `file` can name, when `file` is an
/// imported file; `None` for the program's own, which reads them all.
pub(super) fn generic_aliases_for(
    ctx: &TypeCheckCtx,
    file: &Arc<str>,
) -> Option<Arc<HashMap<String, crate::types::resolve::GenericAlias>>> {
    if !ctx.imports.aliases.contains_key(file) {
        return None;
    }
    let mut cache = ctx.file_aliases.borrow_mut();
    Some(file_entry(ctx, &mut cache, file).generic.clone())
}

fn file_entry<'c>(
    ctx: &TypeCheckCtx,
    cache: &'c mut HashMap<Arc<str>, FileAliases>,
    file: &Arc<str>,
) -> &'c mut FileAliases {
    cache.entry(file.clone()).or_insert_with(|| {
        let decls = ctx.imports.aliases.get(file).map(Vec::as_slice).unwrap_or(&[]);
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
    ctx: &TypeCheckCtx,
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
                out.push(Diagnostic::error("WS002", format!("unknown type '{name}'"), range.clone()));
            }
        };
        match t {
            TypeExpr::Name { name, range } => name_at(name, range, out),
            TypeExpr::Generic { name, args, range } => {
                if !matches!(name.as_str(), "Map" | "Array" | "Ref") {
                    name_at(name, range, out);
                }
                args.iter().for_each(|a| walk(a, params, known, unknown, out));
            }
            TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => walk(inner, params, known, unknown, out),
            TypeExpr::Tuple { fields, .. } => fields.iter().for_each(|f| walk(f, params, known, unknown, out)),
            TypeExpr::Union { options, .. } => options.iter().for_each(|o| walk(o, params, known, unknown, out)),
            TypeExpr::Record { fields, .. } => fields.iter().for_each(|f| walk(&f.typ, params, known, unknown, out)),
        }
    }
    let known = |name: &str| {
        crate::types::resolve::primitive(name).is_some()
            || types.alias(name).is_some()
            || ctx.enum_defs.written_name(name).is_some()
    };
    let mut unknown: HashMap<String, Vec<Diagnostic>> = HashMap::default();
    let each = |key: String, t: &crate::ast::TypeAliasDecl, unknown: &mut HashMap<String, Vec<Diagnostic>>| {
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
    ctx: &TypeCheckCtx,
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
        (entry.own.clone(), entry.unknown.clone())
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
                args.iter().for_each(|a| names_in(a, out));
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
fn names_generic_alias(t: &TypeExpr, generic: &HashMap<String, crate::types::resolve::GenericAlias>) -> bool {
    if generic.is_empty() {
        return false;
    }
    match t {
        TypeExpr::Name { .. } => false,
        TypeExpr::Generic { name, args, .. } => {
            generic.contains_key(name) || args.iter().any(|a| names_generic_alias(a, generic))
        }
        TypeExpr::Array { inner, .. } | TypeExpr::Ref { inner, .. } => names_generic_alias(inner, generic),
        TypeExpr::Tuple { fields, .. } => fields.iter().any(|f| names_generic_alias(f, generic)),
        TypeExpr::Union { options, .. } => options.iter().any(|o| names_generic_alias(o, generic)),
        TypeExpr::Record { fields, .. } => fields.iter().any(|f| names_generic_alias(&f.typ, generic)),
    }
}

/// The aliases a type written in `file` reads at the top level, where a
/// generic alias's body is written: none of the type parameters of the body
/// being checked, which hide a top-level alias of their name only where that
/// body writes them.
fn declaring_aliases(ctx: &TypeCheckCtx, file: &Arc<str>) -> HashMap<String, Type> {
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

/// Cartesian product of per-type-param masks: each inner
/// `Vec<Type>` in `masks` is one type param's candidate concrete members;
/// the result has one `Vec<Type>` per combination, index-aligned with
/// `masks` (and so with the decl's `type_params`). Materializes the full
/// result — callers must cap the product size (e.g. against
/// `MAX_BODY_CHECK_COMBOS`) before calling this.
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

/// The type of a constant-literal expression, used to infer an unannotated
/// var's (or array var's element) type at registration, before the full type
/// map exists. Returns `None` for anything that isn't a compile-time literal.
pub(super) fn literal_expr_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::IntLit { .. } => Some(Type::Int),
        Expr::AtomLit { .. } => Some(Type::Int),
        Expr::FloatLit { .. } => Some(Type::Float),
        Expr::BoolLit { .. } => Some(Type::Bool),
        Expr::StringLit { .. } | Expr::InterpLit { .. } => Some(Type::String),
        Expr::UnOp { op, operand, .. } if op == "-" => literal_expr_type(operand),
        // Constructor calls type by name alone — the value folds to a constant
        // later only if the args are constant, but the type holds regardless.
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

/// Types a Variable gate can hold as a wire variant — the safe targets when
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

/// `any` (`Type::Opaque`) is fine anywhere a value just flows through a wire
/// (ports, `let`s, mod/chip params) — but a var/array/buffer's storage gate
/// needs a concrete wire variant to hold, so an explicit `any` annotation
/// there is rejected (WS025). The same holds for the reference types
/// (`zone`/`teleport`/`prefab`): like a var ref, they can only be wired or
/// rerouted, never held in a storage gate. Only fires on an *explicit*
/// annotation: an unannotated declaration's inferred placeholder is
/// `Type::Any`, never `Type::Opaque`, so it never reaches this check.
pub(super) fn reject_any_storage(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange, what: &str) {
    match resolved {
        Type::Zone | Type::Teleport | Type::PrefabRef => {
            let n = crate::analysis::types::type_str(resolved);
            ctx.emit(
                "WS025",
                format!(
                    "'{n}' is a reference and cannot be stored: {what} needs a concrete \
                     wire type; a '{n}' can only be wired or rerouted (like a var ref)"
                ),
                range,
            );
        }
        Type::Opaque => ctx.emit(
            "WS025",
            format!(
                "'any' cannot be stored: {what} needs a concrete wire type \
                 (int/float/bool/string/vector/...)"
            ),
            range,
        ),
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
            let n = crate::analysis::types::type_str(resolved);
            let plain = crate::analysis::types::type_str(inner);
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
            let n = crate::analysis::types::type_str(resolved);
            ctx.emit(
                "WS025",
                format!("'{n}' cannot be stored: a ref names one var, and no var holds a '{inner}'"),
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
            if matches!(elem.as_ref(), Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }) {
                let n = crate::analysis::types::type_str(resolved);
                let (kind, parts) = match elem.as_ref() {
                    Type::Record(_) => ("records", "one container per field"),
                    Type::Tuple(_) => ("tuples", "one container per element"),
                    _ => ("enums", "one container for the tag and one per payload field"),
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

/// A map key is hashed by value, and a ref or brick has none to hash.
pub(super) fn reject_ref_key(ctx: &mut TypeCheckCtx, key: &Type, range: SourceRange) {
    if key.ref_kind().is_some() {
        let n = crate::analysis::types::type_str(key);
        ctx.emit(
            "WS025",
            format!("'{n}' cannot be a map key: a key needs a plain value (int, string, bool or an object)"),
            range,
        );
    }
}

/// True if `t` is, or structurally contains, the wildcard `any` type
/// (`Type::Opaque`). A `*any`/`any[]`/`Map<_, any>`/`{ a: any }` annotation
/// wraps the `Opaque` under a `Ref`/`Array`/`Map`/`Record`/…, so a shallow
/// `matches!(t, Type::Opaque)` would miss it — recurse through every compound
/// so `warn_any_annotation` fires on the whole annotation regardless of nesting.
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

/// True if `t` is, or structurally contains, a `Type::Param` — i.e. an
/// unresolved generic type parameter. A call whose param still carries one
/// (its `T` couldn't be inferred) is left to the WS033 inference diagnostics,
/// not the concrete arg-vs-param coercion check.
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

/// Warns (does not error) when a user's *non-storage* type annotation — an
/// `in`/`out` port, a mod/chip param or output, a handler's typed destructure
/// param, a `let`, or a `type X = ...` alias body — resolves to `any`
/// (`Type::Opaque`), including when the `any` is wrapped in a compound
/// (`*any`, `any[]`, `Map<_, any>`, `{ a: any }`, …). `any` still works there
/// (the value just flows through as a wildcard), but a generic type parameter
/// would let the type flow instead of erasing it. Storage positions
/// (var/array/buffer/map — see `reject_any_storage` above) are deliberately NOT
/// routed through this: an explicit `any` there is already a hard error
/// (WS025), and warning on top would double-fire the same annotation. Fires
/// once per annotation (at the top-level annotation range), not once per nested
/// `Opaque`.
pub(super) fn warn_any_annotation(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange) {
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

/// Note every record field name in `t` for WS093 (`TypeCheckCtx::reserve_check`).
fn reserve_record_fields(ctx: &mut TypeCheckCtx, t: &TypeExpr) {
    match t {
        TypeExpr::Record { fields, .. } => {
            for f in fields {
                ctx.reserve_check(&f.name, &f.range);
                reserve_record_fields(ctx, &f.typ);
            }
        }
        TypeExpr::Array { inner, .. } | TypeExpr::Ref { inner, .. } => reserve_record_fields(ctx, inner),
        TypeExpr::Tuple { fields, .. } => fields.iter().for_each(|f| reserve_record_fields(ctx, f)),
        TypeExpr::Union { options, .. } => options.iter().for_each(|f| reserve_record_fields(ctx, f)),
        TypeExpr::Generic { args, .. } => args.iter().for_each(|f| reserve_record_fields(ctx, f)),
        _ => {}
    }
}
