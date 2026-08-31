//! Enum registry: discriminant assignment (auto-numbering + duplicate
//! detection, `WS064`) and the `EnumDef`/`VariantDef`/`Payload` tables that
//! record what `collect_enum_defs` found. Consumed later (type resolution,
//! lowering) via `ctx.enum_defs`; this module only builds the table.

use super::*;

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

/// Assign every variant's discriminant in declaration order: the running
/// value starts at 0; an explicit `= N` sets it to `N`; the next implicit
/// value is always `previous + 1` (so `A = 2, B, C` numbers `B` as 3, not
/// 1).
///
/// The SINGLE implementation of enum discriminant numbering: typecheck reaches
/// it through [`assign_discriminants`] (which layers `WS064` on top), and
/// lowering reaches it through [`build_registry`]. Neither re-derives the tag,
/// so the two layers can never disagree on a value the compiled circuit's
/// correctness depends on. Diagnostic-free by construction.
pub fn variant_defs(decl: &EnumDecl) -> Vec<VariantDef> {
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
pub fn assign_discriminants(decl: &EnumDecl, ctx: &mut TypeCheckCtx) -> Vec<VariantDef> {
    let out = variant_defs(decl);
    let mut seen: crate::collections::HashMap<i64, String> = Default::default();
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
fn type_expr_mentions(te: &TypeExpr, param: &str) -> bool {
    match te {
        TypeExpr::Name { name, .. } => name == param,
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            type_expr_mentions(inner, param)
        }
        TypeExpr::Tuple { fields, .. } => fields.iter().any(|f| type_expr_mentions(f, param)),
        TypeExpr::Union { options, .. } => options.iter().any(|o| type_expr_mentions(o, param)),
        TypeExpr::Record { fields, .. } => {
            fields.iter().any(|f| type_expr_mentions(&f.typ, param))
        }
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
            Payload::Named(fields) => {
                fields.iter().any(|(_, te)| type_expr_mentions(te, &p.name))
            }
        });
        if stored {
            return Some(p.name.clone());
        }
    }
    None
}

/// Reject an enum payload field that would need container storage (`WS069`).
///
/// A payload slot is one storage gate, filled by the variant's CONSTRUCTION,
/// and no construction path can fill a container: a declaration initializer
/// bakes an `InitialValue` (there is none for an array), and a runtime
/// `E.B { xs: [1, 2, 3] }` lowers the literal to a placeholder and copies from
/// an empty temporary. The slot still allocates an `ArrayVar`/`MapVar`, so a
/// captured `xs.push(..)` emits a real gate against storage that construction
/// never fills - the value silently reads back empty. Rejecting the
/// declaration kills every downstream use at once, instead of leaving each
/// construction site to warn (or not).
///
/// Runs AFTER top-level registration so a field naming a record alias
/// (`type R = { xs: int[] }`) resolves. Resolution diagnostics are discarded:
/// a payload's declared type is not otherwise a checked position (an unknown
/// type there is silent today), and this pass must test storage shape without
/// becoming a new emit site for everything else.
pub fn check_payload_storage(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
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
                let before = ctx.diagnostics.len();
                let ty = resolve_type_expr(ctx, te);
                ctx.diagnostics.truncate(before);
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
                        crate::analysis::types::type_str(&ty)
                    ),
                    range.clone(),
                );
            }
        }
    }
}

/// The built-in `Option<T>`/`Result<T, E>` prelude, in the exact `EnumDef`
/// shape `variant_defs`/`collect_enum_defs` produce for a user-declared enum -
/// same discriminant convention (declaration order, auto-numbered from 0), so
/// a use of either can't tell the difference from a hand-written `enum
/// Option<T> { Some(T), None }`. Seeded into every registry that resolves an
/// enum by name: `register::register_builtin_enums` (typecheck's
/// `ctx.enum_defs` + the `Option`/`Result` type-name scope symbols) and
/// [`build_registry`] (the single lowering/const-eval seed), so `Option`/
/// `Result` and their bare variants (`Some`/`None`/`Ok`/`Err`) work with no
/// `enum` declaration anywhere in the program.
pub fn prelude_enum_defs() -> Vec<EnumDef> {
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

/// The built-in game enums (`catalog::builtin_game_enums`), in the same
/// `EnumDef` shape [`prelude_enum_defs`] produces - except the discriminant is
/// NOT auto-numbered: it is the real schema integer
/// (`GameEnumVariant::disc`), since these values round-trip through saved
/// component data and a renumbered tag would silently write the wrong enum
/// value to the game. Every variant is `Payload::Unit` - the game's schema
/// enums carry no per-variant payload. Seeded into every registry that
/// resolves an enum by name, same as the prelude: `register::
/// register_builtin_enums` (typecheck's `ctx.enum_defs` + each enum's
/// type-name scope symbol) and [`build_registry`] (the single lowering/
/// const-eval seed).
pub fn game_enum_defs() -> Vec<EnumDef> {
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

/// Build the `name -> EnumDef` registry from the built-in prelude
/// ([`prelude_enum_defs`]), the built-in game enums ([`game_enum_defs`]), plus
/// every top-level `enum` in `decls`, numbering each user enum via the shared
/// [`variant_defs`]. A user enum that redeclares a prelude or game-enum name
/// (`enum Option { ... }`) overwrites the earlier entry here -
/// `register_decl`'s `declare_or_dup` still reports the redeclaration as
/// `WS013` (the type-name scope symbol collides), so this is "the user's own
/// definition wins the registry, but the redeclaration is still flagged", not
/// silent shadowing. Diagnostic-free - the lowering-side registry seed
/// (`lower::lower`) calls this, and typecheck has already reported any
/// `WS064` through its own `collect_enum_defs` pass, so re-emitting here would
/// duplicate it. This is the one place lowering reads the tag from, so it can
/// never diverge from typecheck's.
pub fn build_registry(decls: &[TopDecl]) -> crate::collections::HashMap<String, EnumDef> {
    let mut m: crate::collections::HashMap<String, EnumDef> = Default::default();
    for def in prelude_enum_defs() {
        m.insert(def.name.clone(), def);
    }
    for def in game_enum_defs() {
        m.insert(def.name.clone(), def);
    }
    for d in decls {
        if let TopDecl::Enum(e) = d {
            m.insert(
                e.name.clone(),
                EnumDef {
                    name: e.name.clone(),
                    type_params: e.type_params.clone(),
                    variants: variant_defs(e),
                },
            );
        }
    }
    m
}

/// Resolve a BARE variant name (`Some`/`None`/`Ok`/`Err`, unqualified) to the
/// enum that uniquely declares it - the single source of truth all three
/// stages (typecheck, lowering, const-eval) share for bare-variant lookup, so
/// the "which enum does a bare name mean" rule cannot drift between the stage
/// that type-checks a program and the stages that lower/fold it. Each stage
/// wraps this with its OWN `is_shadowed` closure (its scope has a different
/// shape); the lookup + uniqueness rule itself lives here, once.
///
/// `is_shadowed(name)` is the caller's shadow predicate: a user binding of the
/// same name wins outright, so a shadowed name is NOT a bare variant here and
/// the caller falls back to its ordinary resolution. Every enum - prelude OR
/// user - registers a scope/type symbol under its OWN name (see
/// `register_builtin_enums` and `TopDecl::Enum` registration), so a caller
/// whose scope can't see type-only symbols may add an `enum_defs.contains_key`
/// term to its predicate to still shadow a user enum named after a variant
/// (`enum Some { .. }`): that term is always a SUBSET of typecheck's
/// full-scope shadow set, so lowering/const-eval can never RESOLVE a bare name
/// typecheck would have shadowed - the invariant that keeps the circuit and
/// the type checker agreeing on what a bare name means.
///
/// Returns the owning enum's name only on a UNIQUE match; no match, or more
/// than one (two enums sharing a variant name), yields `None` - an ambiguous
/// bare name never auto-resolves.
pub fn resolve_bare_variant_enum<'d>(
    enum_defs: &'d crate::collections::HashMap<String, EnumDef>,
    name: &str,
    is_shadowed: impl Fn(&str) -> bool,
) -> Option<&'d str> {
    if is_shadowed(name) {
        return None;
    }
    let mut found: Option<&str> = None;
    for def in enum_defs.values() {
        if def.variants.iter().any(|v| v.name == name) {
            if found.is_some() {
                return None;
            }
            found = Some(def.name.as_str());
        }
    }
    found
}

/// Populate `ctx.enum_defs` from every `enum` declaration in `decls`,
/// assigning discriminants (and emitting `WS064` for any collision) along
/// the way. Run this BEFORE decl registration/checking, same as
/// `collect_generic_aliases`, so a use of the enum name sees a fully built
/// `EnumDef` regardless of where in the file the enum itself is declared.
pub fn collect_enum_defs(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    for d in decls {
        if let TopDecl::Enum(e) = d {
            let variants = assign_discriminants(e, ctx);
            Arc::make_mut(&mut ctx.enum_defs).insert(
                e.name.clone(),
                EnumDef {
                    name: e.name.clone(),
                    type_params: e.type_params.clone(),
                    variants,
                },
            );
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
    fn registry_with_user_enum_named_some() -> crate::collections::HashMap<String, EnumDef> {
        let mut m = build_registry(&[]);
        m.insert(
            "Some".to_string(),
            EnumDef {
                name: "Some".to_string(),
                type_params: vec![],
                variants: vec![VariantDef {
                    name: "A".to_string(),
                    discriminant: 0,
                    payload: Payload::Unit,
                }],
            },
        );
        m
    }

    #[test]
    fn bare_variant_resolves_to_prelude_when_unshadowed() {
        let reg = build_registry(&[]);
        assert_eq!(resolve_bare_variant_enum(&reg, "Some", |_| false), Some("Option"));
        assert_eq!(resolve_bare_variant_enum(&reg, "None", |_| false), Some("Option"));
        assert_eq!(resolve_bare_variant_enum(&reg, "Ok", |_| false), Some("Result"));
        assert_eq!(resolve_bare_variant_enum(&reg, "Err", |_| false), Some("Result"));
        assert_eq!(resolve_bare_variant_enum(&reg, "Nope", |_| false), None);
    }

    #[test]
    fn a_shadow_predicate_hit_blocks_resolution() {
        let reg = build_registry(&[]);
        // Whatever a stage's scope reports as a binding of this name wins.
        assert_eq!(resolve_bare_variant_enum(&reg, "Some", |n| n == "Some"), None);
    }

    // The alignment guard for the single-sourced helper: lowering's and
    // const-eval's `enum_defs.contains_key(n)` shadow term must block a bare
    // `Some` when a user enum is NAMED `Some`, matching typecheck (which shadows
    // via that enum's own registered type-name scope symbol). If a future edit
    // ever dropped that term from one stage's predicate, that stage would
    // resolve `Some` to the prelude while typecheck did not - the exact silent
    // typecheck/lower disagreement this test forbids.
    #[test]
    fn enum_name_collision_is_shadowed_by_the_registry_predicate() {
        let reg = registry_with_user_enum_named_some();
        // The prelude-only (no enum_defs term) predicate would still resolve to
        // the prelude - it only checks value bindings, of which there are none.
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |_| false),
            Some("Option"),
            "control: without the registry term, the enum-name collision is invisible"
        );
        // Lowering / const-eval OR in `enum_defs.contains_key`, so the user enum
        // named `Some` shadows the bare prelude variant - aligning them with
        // typecheck.
        assert_eq!(
            resolve_bare_variant_enum(&reg, "Some", |n| reg.contains_key(n)),
            None,
            "the registry-term predicate must shadow a user enum named after a variant"
        );
    }

    #[test]
    fn registry_includes_builtin_game_enums() {
        let reg = build_registry(&[]);
        let easing = reg.get("EasingFunction").expect("EasingFunction is a built-in");
        // Discriminant is the schema integer, not an auto-numbered index.
        let bounce = easing.variants.iter().find(|v| v.name == "Bounce").expect("Bounce");
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
        reg.insert(
            "Dup".to_string(),
            EnumDef {
                name: "Dup".to_string(),
                type_params: vec![],
                variants: vec![VariantDef {
                    name: "Some".to_string(),
                    discriminant: 0,
                    payload: Payload::Unit,
                }],
            },
        );
        assert_eq!(resolve_bare_variant_enum(&reg, "Some", |_| false), None);
    }
}
