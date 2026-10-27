//! Enum registry: discriminant assignment (auto-numbering + duplicate
//! detection, `WS064`) and the `EnumDef`/`VariantDef`/`Payload` tables that
//! record what `collect_enum_defs` found. Consumed later (type resolution,
//! lowering) via `ctx.enum_defs`; this module only builds the table.

use super::*;
use crate::analysis::types::type_str;

/// The field of an enum value that holds its discriminant.
pub(crate) const DISC_FIELD: &str = "__disc";

/// The field a program reads an enum value's discriminant through (`e.Discriminant`).
pub(crate) const DISCRIMINANT_FIELD: &str = "Discriminant";

/// [`DISC_FIELD`], interned.
pub(crate) static DISC: std::sync::LazyLock<crate::intern::Sym> =
    std::sync::LazyLock::new(|| crate::intern::intern_static(DISC_FIELD));

/// The field of an enum value that holds `variant`'s payload slot `slot` (a
/// field name, or a position for a positional payload).
pub(crate) fn slot_name(variant: &str, slot: impl std::fmt::Display) -> String {
    format!("__{variant}_{slot}")
}

/// The program's enums by name: the prelude's, the game's and every
/// `TopDecl::Enum`. A lookup takes a resolved type ([`of`](Self::of)). A type
/// written in the source names an enum only through the alias-aware resolvers
/// (`typecheck::resolve::resolve_type_expr`, `const_eval::DeclaredTypes`), which
/// read an alias, a type parameter or another file's declaration first; the
/// lookup by written name ([`type_named`](Self::type_named)) is theirs alone
/// (`alias_conversion.rs` guards that).
#[derive(Clone, Debug, Default)]
pub struct EnumRegistry {
    defs: HashMap<String, EnumDef>,
    /// The enums each `import * as` module declares, by the importing file
    /// and the namespace's name. Such an enum is registered under its own
    /// name, which its module writes; the importer writes `ns.Name`.
    namespaced: HashMap<(std::sync::Arc<str>, String), Vec<String>>,
}

impl EnumRegistry {
    /// The enum the resolved type `t` is.
    pub fn of(&self, t: &Type) -> Option<&EnumDef> {
        match t {
            Type::Enum { name, .. } => self.defs.get(name),
            _ => None,
        }
    }

    /// The enum an expression path names (`Dir.N`, `Shape.Circle(r)`,
    /// `Dir.FromInt(n)`, a bare variant's enum): a path writes the enum's own
    /// name, since an alias of an enum names no variants (`WS060`). Only the
    /// expression-path sites call it (`alias_conversion.rs` guards that).
    pub(crate) fn expression_path(&self, name: &str) -> Option<&EnumDef> {
        self.defs.get(name)
    }

    /// The editor's lookup of the enum named by the text it completes or
    /// hovers, which no type resolution has read. Only the editor's build
    /// (the `editor` feature, the LSP's) has it, so the compiler cannot call
    /// it (`alias_conversion.rs` guards that too).
    #[cfg(feature = "editor")]
    pub fn editor_lookup(&self, name: &str) -> Option<&EnumDef> {
        self.defs.get(name)
    }

    /// Resolution only: the enum declared under the written type name `name`,
    /// once it is known to be no alias and no type parameter where it is
    /// written.
    pub(crate) fn written_name(&self, name: &str) -> Option<&EnumDef> {
        self.defs.get(name)
    }

    /// Resolution only: the type the written name `name`, applied to `args`,
    /// stands for when it names an enum here (see
    /// [`written_name`](Self::written_name)).
    pub(crate) fn type_named(&self, name: &str, args: Vec<Type>) -> Option<Type> {
        self.defs.contains_key(name).then(|| Type::Enum {
            name: name.to_string(),
            args,
        })
    }

    /// Register `def` under its name, replacing an enum of that name.
    pub fn insert(&mut self, def: EnumDef) {
        self.defs.insert(def.name.clone(), def);
    }

    /// The enum `name` of the `import * as ns` namespace that `file` imports:
    /// the head `ns.Name` of a path or type written there.
    pub fn namespaced(&self, file: &std::sync::Arc<str>, ns: &str, name: &str) -> Option<&EnumDef> {
        let declares = self
            .namespaced
            .get(&(std::sync::Arc::clone(file), ns.to_string()))?
            .iter()
            .any(|n| n == name);
        declares.then(|| self.defs.get(name)).flatten()
    }

    /// Note that the `import * as ns` written in `file` declares enum `name`.
    fn note_namespaced(&mut self, file: &std::sync::Arc<str>, ns: &str, name: &str) {
        self.namespaced
            .entry((std::sync::Arc::clone(file), ns.to_string()))
            .or_default()
            .push(name.to_string());
    }

    /// The one enum that declares every variant the arms of a `match` cite
    /// by variant alone; `None` when none does or several do, or no arm
    /// cites one.
    pub(crate) fn citing_arms(&self, arms: &[MatchArm]) -> Option<&EnumDef> {
        let mut cited: Vec<&str> = Vec::new();
        for arm in arms {
            if let Pattern::Variant { variant, .. } = &arm.pattern
                && !cited.contains(&variant.as_str())
            {
                cited.push(variant.as_str());
            }
        }
        if cited.is_empty() {
            return None;
        }
        self.declaring_variants(&cited)
    }

    /// The one enum that declares every variant in `variants` (a bare
    /// variant's, or a `match` whose arms name variants alone); `None` when
    /// none does or several do.
    fn declaring_variants(&self, variants: &[&str]) -> Option<&EnumDef> {
        // A prelude or user enum declaring the names wins over a game enum:
        // game enums come from the catalog, and a new build adding a member
        // named `None` must not take `None` from `Option`. Within either
        // group, two declaring enums make the name ambiguous.
        let declaring = |game: bool| {
            let mut found: Option<&EnumDef> = None;
            for def in self.defs.values() {
                if (crate::catalog::game_enum_schema_type(&def.name).is_some()) != game {
                    continue;
                }
                if game && !crate::catalog::resolves_bare_members(&def.name) {
                    continue;
                }
                if variants
                    .iter()
                    .all(|name| def.variants.iter().any(|v| v.name == *name))
                {
                    if found.is_some() {
                        return Err(());
                    }
                    found = Some(def);
                }
            }
            Ok(found)
        };
        match declaring(false) {
            Ok(Some(def)) => Some(def),
            Ok(None) => declaring(true).ok().flatten(),
            Err(()) => None,
        }
    }

    /// Every enum's name and every variant's.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.defs.values().flat_map(|d| {
            std::iter::once(d.name.as_str()).chain(d.variants.iter().map(|v| v.name.as_str()))
        })
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
}

/// A registered `enum` declaration, discriminants already assigned. One per
/// `TopDecl::Enum`, keyed by name in `ctx.enum_defs`.
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    /// Declaration-side generic type parameters (`enum Option<T> { ... }`).
    /// Empty for non-generic enums; instantiation against a use site's
    /// concrete args is a later phase, not this registry's job.
    pub type_params: Vec<TypeParam>,
    pub variants: Vec<VariantDef>,
}

impl EnumDef {
    /// The registered form of `decl`, with its variants already numbered.
    pub fn from_decl(decl: &EnumDecl, variants: Vec<VariantDef>) -> Self {
        Self {
            name: decl.name.clone(),
            type_params: decl.type_params.clone(),
            variants,
        }
    }
}

/// One variant of a registered enum, with its final (auto-numbered or
/// explicit) discriminant.
#[derive(Clone, Debug)]
pub struct VariantDef {
    pub name: String,
    pub discriminant: i64,
    pub payload: Payload,
}

/// A variant's payload shape. Mirrors `ast::EnumPayloadDecl`, minus the
/// per-field source ranges (the registry only needs the shape; diagnostics
/// on a field's type still point at the field's own `TypeExpr` range).
#[derive(Clone, Debug)]
pub enum Payload {
    Unit,
    Positional(Vec<TypeExpr>),
    Named(Vec<(String, TypeExpr)>),
}

/// `EnumPayloadDecl` -> `Payload`.
fn lower_payload(p: &EnumPayloadDecl) -> Payload {
    match p {
        EnumPayloadDecl::Unit => Payload::Unit,
        EnumPayloadDecl::Positional(types) => Payload::Positional(types.clone()),
        EnumPayloadDecl::Named(fields) => Payload::Named(
            fields
                .iter()
                .map(|(name, typ, _range)| (name.clone(), typ.clone()))
                .collect(),
        ),
    }
}

/// Assign every variant's discriminant in declaration order: the running value
/// starts at 0, an explicit `= N` sets it, and the next implicit one is the
/// previous plus one (`A = 2, B, C` numbers `B` as 3). The one numbering: typecheck
/// reaches it through [`assign_discriminants`] (adding WS064) and lowering
/// through [`build_registry`], so the two never disagree on a tag. It reports
/// nothing itself.
pub(crate) fn variant_defs(decl: &EnumDecl) -> Vec<VariantDef> {
    let mut next: i64 = 0;
    let mut out = Vec::with_capacity(decl.variants.len());
    for v in &decl.variants {
        let disc = v.explicit_disc.unwrap_or(next);
        next = disc + 1;
        out.push(VariantDef {
            name: v.name.clone(),
            discriminant: disc,
            payload: lower_payload(&v.payload),
        });
    }
    out
}

/// [`variant_defs`] plus duplicate-discriminant detection: two variants that
/// resolve to the same integer are a collision, and `WS064` fires on the
/// SECOND one, naming the variant that already claimed it. The numbering
/// itself is not repeated here.
pub(crate) fn assign_discriminants(decl: &EnumDecl, ctx: &mut TypeCheckCtx<'_>) -> Vec<VariantDef> {
    let out = variant_defs(decl);
    let mut seen: HashMap<i64, String> = HashMap::default();
    for (v, vdecl) in out.iter().zip(&decl.variants) {
        if let Some(prev) = seen.insert(v.discriminant, v.name.clone()) {
            ctx.emit(
                "WS064",
                format!(
                    "duplicate discriminant {} in enum `{}` (already used by `{prev}`)",
                    v.discriminant, decl.name
                ),
                vdecl.range.clone(),
            );
        }
    }
    out
}

/// Whether a payload field of this type would need container storage anywhere
/// inside it. An `ArrayVar`/`MapVar` is the whole trigger; a record is checked
/// per leaf, and a generic argument is checked so `W(Box<int[]>)` is caught at
/// the field that hosts it. A nested ENUM is not descended into: its own
/// declaration is checked by the same pass, so it reports its own field.
fn needs_container_storage(t: &Type) -> bool {
    match t {
        Type::Array(_) | Type::Map(_, _) => true,
        Type::Ref(inner) => needs_container_storage(inner),
        Type::Record(fields) => fields.iter().any(|(_, ft)| needs_container_storage(ft)),
        Type::Tuple(elems) | Type::Union(elems) => elems.iter().any(needs_container_storage),
        Type::Enum { args, .. } => args.iter().any(needs_container_storage),
        _ => false,
    }
}

/// Whether `te` names `param` anywhere, so a generic argument substituted for
/// it would land in this payload field. Lets the instantiation check reject
/// `Option<int[]>` (whose `T` IS stored, by `Some(T)`) while leaving a phantom
/// parameter that no variant stores alone.
pub(super) fn type_expr_mentions(te: &TypeExpr, param: &str) -> bool {
    match te {
        TypeExpr::Name { name, .. } => name == param,
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            type_expr_mentions(inner, param)
        }
        TypeExpr::Tuple { fields, .. } => fields.iter().any(|f| type_expr_mentions(f, param)),
        TypeExpr::Union { options, .. } => options.iter().any(|o| type_expr_mentions(o, param)),
        TypeExpr::Record { fields, .. } => fields.iter().any(|f| type_expr_mentions(&f.typ, param)),
        TypeExpr::Generic { args, .. } => args.iter().any(|a| type_expr_mentions(a, param)),
    }
}

/// Whether instantiating `def` with `args` would put a container in a payload
/// slot: some argument needs container storage AND the parameter it replaces is
/// actually named by a payload field. Drives the `WS069` at an instantiation
/// (`Option<int[]>`), which the declaration check cannot see because the field
/// there is the bare parameter.
pub(super) fn instantiation_stores_container(def: &EnumDef, args: &[Type]) -> Option<String> {
    for (i, p) in def.type_params.iter().enumerate() {
        let Some(arg) = args.get(i) else { continue };
        if !needs_container_storage(arg) {
            continue;
        }
        let stored = def.variants.iter().any(|v| match &v.payload {
            Payload::Unit => false,
            Payload::Positional(types) => types.iter().any(|te| type_expr_mentions(te, &p.name)),
            Payload::Named(fields) => fields.iter().any(|(_, te)| type_expr_mentions(te, &p.name)),
        });
        if stored {
            return Some(p.name.clone());
        }
    }
    None
}

/// Reject an enum payload field needing container storage (WS069). A payload
/// slot is one storage gate filled by the construction, and no construction
/// fills a container: a declaration bakes an `InitialValue` (an array has none),
/// and a runtime `E.B { xs: [1, 2, 3] }` copies from an empty temporary, so the
/// slot's `ArrayVar`/`MapVar` would always read back empty. Rejecting the
/// declaration covers every use at once.
///
/// Runs after top-level registration, so a field naming a record alias
/// (`type R = { xs: int[] }`) resolves. Resolution diagnostics are discarded: a
/// payload's declared type is not otherwise a checked position.
pub(crate) fn check_payload_storage(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    for d in decls {
        let TopDecl::Enum(e) = d else { continue };
        for v in &e.variants {
            let fields: Vec<(Option<&str>, &TypeExpr, &SourceRange)> = match &v.payload {
                EnumPayloadDecl::Unit => continue,
                EnumPayloadDecl::Positional(types) => {
                    types.iter().map(|te| (None, te, &v.range)).collect()
                }
                EnumPayloadDecl::Named(fs) => fs
                    .iter()
                    .map(|(n, te, r)| (Some(n.as_str()), te, r))
                    .collect(),
            };
            for (name, te, range) in fields {
                let ty = ctx.discarding_diagnostics(|ctx| resolve_type_expr(ctx, te));
                if !needs_container_storage(&ty) {
                    continue;
                }
                let what = match name {
                    Some(n) => format!("field `{n}` of variant `{}`", v.name),
                    None => format!("a payload value of variant `{}`", v.name),
                };
                ctx.emit(
                    "WS069",
                    format!(
                        "{what} in enum `{}` is `{}`, and an enum payload cannot hold an array or a map. A payload slot is filled by constructing the variant, and there is no way to construct a container into one, so the value would read back empty. Store the container in a `var` beside the enum and keep an index, a key, or a length in the payload instead",
                        e.name,
                        type_str(&ty)
                    ),
                    range.clone(),
                );
            }
        }
    }
}

/// The built-in `Option<T>`/`Result<T, E>` prelude, in the `EnumDef` shape
/// `variant_defs`/`collect_enum_defs` give a user enum (declaration-order
/// discriminants from 0), so it behaves as a written `enum Option<T> { Some(T),
/// None }`. Seeded into every registry that resolves enums by name
/// (`register::register_builtin_enums` for typecheck, [`build_registry`] for
/// lowering and const evaluation), so `Option`, `Result` and their bare
/// variants work with no declaration.
pub(crate) fn prelude_enum_defs() -> Vec<EnumDef> {
    let range = SourceRange::default();
    let type_param = |name: &str| TypeParam {
        name: name.to_string(),
        bound: None,
        range: range.clone(),
    };
    let type_name = |name: &str| TypeExpr::Name {
        name: name.to_string(),
        range: range.clone(),
    };
    vec![
        EnumDef {
            name: "Option".to_string(),
            type_params: vec![type_param("T")],
            variants: vec![
                VariantDef {
                    name: "Some".to_string(),
                    discriminant: 0,
                    payload: Payload::Positional(vec![type_name("T")]),
                },
                VariantDef {
                    name: "None".to_string(),
                    discriminant: 1,
                    payload: Payload::Unit,
                },
            ],
        },
        EnumDef {
            name: "Result".to_string(),
            type_params: vec![type_param("T"), type_param("E")],
            variants: vec![
                VariantDef {
                    name: "Ok".to_string(),
                    discriminant: 0,
                    payload: Payload::Positional(vec![type_name("T")]),
                },
                VariantDef {
                    name: "Err".to_string(),
                    discriminant: 1,
                    payload: Payload::Positional(vec![type_name("E")]),
                },
            ],
        },
    ]
}

/// The built-in game enums (`catalog::builtin_game_enums`) in the shape of
/// [`prelude_enum_defs`], except that each discriminant is the schema's integer
/// (`GameEnumVariant::disc`): these values round-trip through saved component
/// data, so a renumbered tag would write the wrong value to the game. Every
/// variant is `Payload::Unit`. Seeded like the prelude
/// (`register::register_builtin_enums`, [`build_registry`]).
pub(crate) fn game_enum_defs() -> Vec<EnumDef> {
    crate::catalog::builtin_game_enums()
        .into_iter()
        .map(|e| EnumDef {
            name: e.clean_name,
            type_params: vec![],
            variants: e
                .variants
                .into_iter()
                .map(|v| VariantDef {
                    name: v.clean_name,
                    discriminant: v.disc,
                    payload: Payload::Unit,
                })
                .collect(),
        })
        .collect()
}

/// The `name -> EnumDef` registry: the prelude ([`prelude_enum_defs`]), the game
/// enums ([`game_enum_defs`]) and every top-level `enum` in `decls`, numbered by
/// [`variant_defs`]. A user enum redeclaring a prelude or game name overwrites it
/// here, while `register_decl`'s `declare_or_dup` still reports WS013. Reports
/// nothing: lowering (`lower::lower`) calls this after typecheck reported any
/// WS064, and it is the one place lowering reads tags from.
pub(crate) fn build_registry(decls: &[TopDecl]) -> EnumRegistry {
    let mut m = EnumRegistry::default();
    for def in prelude_enum_defs() {
        m.insert(def);
    }
    for def in game_enum_defs() {
        m.insert(def);
    }
    for d in decls {
        if let TopDecl::Enum(e) = d {
            m.insert(EnumDef::from_decl(e, variant_defs(e)));
        }
    }
    for (ns, e) in namespaced_enums(decls) {
        m.note_namespaced(&ns.range.file, &ns.name, &e.name);
        if m.defs.contains_key(&e.name) {
            continue;
        }
        m.insert(EnumDef::from_decl(e, variant_defs(e)));
    }
    m
}

/// Each enum an `import * as` module of `decls` declares, with its namespace.
fn namespaced_enums(decls: &[TopDecl]) -> Vec<(&NamespaceDecl, &EnumDecl)> {
    let mut out = Vec::new();
    for d in decls {
        let TopDecl::Namespace(ns) = d else { continue };
        for nd in &ns.decls {
            if let TopDecl::Enum(e) = nd {
                out.push((ns, e));
            }
        }
    }
    out
}

/// The enum that uniquely declares the bare variant name (`Some`, `None`, `Ok`,
/// `Err`): the one lookup typecheck, lowering and const evaluation share, each
/// with its own `is_shadowed` predicate (their scopes differ).
///
/// A shadowed name is not a bare variant, so the caller resolves it normally.
/// Every enum registers a type symbol under its own name, so a caller whose
/// scope cannot see type symbols may add `enum_defs.contains_key` to its
/// predicate to still shadow a user enum named like a variant (`enum Some`);
/// that term is a subset of typecheck's shadow set, so lowering never resolves
/// a name typecheck shadowed.
///
/// `None` for no match or several (two enums sharing a variant name). A builtin
/// call name is never a bare variant either: a game build can add an enum
/// member that spells a builtin (`EBRBrickProperty::Color` would turn
/// `Color(1.0, 0.5, 0.0)` into the integer 1).
pub(crate) fn resolve_bare_variant_enum<'d>(
    enum_defs: &'d EnumRegistry,
    name: &str,
    is_shadowed: impl Fn(&str) -> bool,
) -> Option<&'d EnumDef> {
    if is_shadowed(name) || crate::catalog::calls::find_call(name).is_some() {
        return None;
    }
    enum_defs.declaring_variants(&[name])
}

/// Populate `ctx.enum_defs` from every `enum` declaration in `decls`,
/// assigning discriminants (and emitting `WS064` for any collision) along
/// the way. Run this BEFORE decl registration/checking, same as
/// `collect_generic_aliases`, so a use of the enum name sees a fully built
/// `EnumDef` regardless of where in the file the enum itself is declared.
pub(crate) fn collect_enum_defs(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    for d in decls {
        if let TopDecl::Enum(e) = d {
            ctx.enum_decls
                .entry(e.name.clone())
                .or_default()
                .push(e.range.clone());
            let variants = assign_discriminants(e, ctx);
            Arc::make_mut(&mut ctx.enum_defs).insert(EnumDef::from_decl(e, variants));
        }
    }
    // An enum of an `import * as` module is registered under its own name, as
    // a flat import's is: one enum of a name per program.
    for (ns, e) in namespaced_enums(decls) {
        Arc::make_mut(&mut ctx.enum_defs).note_namespaced(&ns.range.file, &ns.name, &e.name);
        match ctx
            .enum_decls
            .get(&e.name)
            .and_then(|ds| ds.iter().find(|r| **r != e.range))
        {
            Some(other) => {
                let file = other
                    .file
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&other.file)
                    .to_string();
                ctx.emit(
                    "WS013",
                    format!(
                        "duplicate declaration of enum '{}': the module imported as `{}` declares it, and so \
                         does {file}; a program has one enum of a name. Rename one",
                        e.name, ns.name
                    ),
                    e.range.clone(),
                );
            }
            None if ctx.enum_decls.contains_key(&e.name) => {}
            None => {
                ctx.enum_decls
                    .entry(e.name.clone())
                    .or_default()
                    .push(e.range.clone());
                let variants = assign_discriminants(e, ctx);
                Arc::make_mut(&mut ctx.enum_defs).insert(EnumDef::from_decl(e, variants));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prelude registry (`Option`/`Result`) plus one user enum NAMED after
    /// a prelude variant (`enum Some { A }`) - the exact shape that could make
    /// the three stages disagree on a bare `Some`, if their shadow predicates
    /// ever diverged.
    fn registry_with_user_enum_named_some() -> EnumRegistry {
        let mut m = build_registry(&[]);
        m.insert(EnumDef {
            name: "Some".to_string(),
            type_params: vec![],
            variants: vec![VariantDef {
                name: "A".to_string(),
                discriminant: 0,
                payload: Payload::Unit,
            }],
        });
        m
    }

    #[test]
    fn bare_variant_resolves_to_prelude_when_unshadowed() {
        let reg = build_registry(&[]);
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |_| false).map(|d| d.name.as_str()),
            Some("Option")
        );
        assert_eq!(
            resolve_bare_variant_enum(&reg, "None", |_| false).map(|d| d.name.as_str()),
            Some("Option")
        );
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Ok", |_| false).map(|d| d.name.as_str()),
            Some("Result")
        );
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Err", |_| false).map(|d| d.name.as_str()),
            Some("Result")
        );
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Nope", |_| false).map(|d| d.name.as_str()),
            None
        );
    }

    #[test]
    fn a_shadow_predicate_hit_blocks_resolution() {
        let reg = build_registry(&[]);
        // Whatever a stage's scope reports as a binding of this name wins.
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |n| n == "Some").map(|d| d.name.as_str()),
            None
        );
    }

    // Lowering's and const evaluation's `enum_defs.contains_key(n)` shadow term
    // must block a bare `Some` when a user enum is named `Some`, as typecheck
    // does through that enum's type symbol; without it, those stages would
    // resolve `Some` to the prelude while typecheck did not.
    #[test]
    fn enum_name_collision_is_shadowed_by_the_registry_predicate() {
        let reg = registry_with_user_enum_named_some();
        // The prelude-only (no enum_defs term) predicate would still resolve to
        // the prelude - it only checks value bindings, of which there are none.
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |_| false).map(|d| d.name.as_str()),
            Some("Option"),
            "control: without the registry term, the enum-name collision is invisible"
        );
        // Lowering / const-eval OR in `enum_defs.contains_key`, so the user enum
        // named `Some` shadows the bare prelude variant - aligning them with
        // typecheck.
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |n| reg.expression_path(n).is_some())
                .map(|d| d.name.as_str()),
            None,
            "the registry-term predicate must shadow a user enum named after a variant"
        );
    }

    #[test]
    fn registry_includes_builtin_game_enums() {
        let reg = build_registry(&[]);
        let easing = reg
            .expression_path("EasingFunction")
            .expect("EasingFunction is a built-in");
        // Discriminant is the schema integer, not an auto-numbered index.
        let bounce = easing
            .variants
            .iter()
            .find(|v| v.name == "Bounce")
            .expect("Bounce");
        assert_eq!(
            bounce.discriminant,
            crate::catalog::enum_member_value("EBREasingFunction", "Bounce").unwrap()
        );
        assert!(matches!(bounce.payload, Payload::Unit));
    }

    #[test]
    fn a_variant_name_shared_by_two_enums_is_ambiguous() {
        let mut reg = build_registry(&[]);
        // A user enum that ALSO declares a `Some` variant - now `Some` is a
        // variant of both `Option` and `Dup`, so no unique owner.
        reg.insert(EnumDef {
            name: "Dup".to_string(),
            type_params: vec![],
            variants: vec![VariantDef {
                name: "Some".to_string(),
                discriminant: 0,
                payload: Payload::Unit,
            }],
        });
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |_| false).map(|d| d.name.as_str()),
            None
        );
    }
}
