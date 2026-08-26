//! Usefulness / witness engine for `match` patterns (Maranget, "Warnings for
//! pattern matching"): the shared U/M pattern-matrix algorithm that answers
//! two questions about a set of arms matched against an enum-typed
//! scrutinee - is every value covered (exhaustiveness), and does every arm
//! ever get reached (unreachable-arm detection). Both the compiler
//! exhaustiveness diagnostic and the LSP "fill missing arms" action consume
//! this module's output, so it takes only pure inputs (no `TypeCheckCtx`)
//! and never emits a diagnostic itself.
//!
//! The working representation is a "pattern matrix": each row is one arm's
//! pattern expanded to a `Vec<Pattern>` of column entries, and a parallel
//! `types` slice records the enum (if any) governing each column. A column
//! whose type does not resolve to a registered enum is "opaque" - the
//! language's pattern grammar only allows a wildcard/binding there, so it is
//! always covered and never contributes a missing witness.

use crate::ast::{Pattern, TypeExpr, VariantPattern};
use crate::collections::{HashMap, HashSet};
use crate::diagnostic::SourceRange;
use crate::ir::Type;

use super::enums::{EnumDef, Payload, VariantDef};

/// A concrete pattern demonstrating one value the arms do not cover.
#[derive(Clone, Debug)]
pub struct Witness(pub Pattern);

/// The result of analyzing a `match`'s arms against its scrutinee type.
#[derive(Clone, Debug, Default)]
pub struct Usefulness {
    /// Non-empty means the arms are not exhaustive (WS054); each entry is
    /// one concrete uncovered value.
    pub missing: Vec<Witness>,
    /// Indices into `arms` of arms that are unreachable (WS061): not useful
    /// with respect to the arms that precede them.
    pub unreachable_arms: Vec<usize>,
}

/// Run the usefulness algorithm for `arms` matched against `scrutinee`.
/// `enum_defs` is the full enum registry (needed to resolve nested enum
/// payload fields, not just the scrutinee's own enum).
pub fn analyze(enum_defs: &HashMap<String, EnumDef>, scrutinee: &Type, arms: &[Pattern]) -> Usefulness {
    let col = match scrutinee {
        Type::Enum { name, args } => {
            enum_defs.get(name).map(|edef| ColEnum { edef, args: args.clone() })
        }
        _ => None,
    };
    // Without a resolved enum for the scrutinee there is no constructor set to
    // reason about: a registry desync (enum named but not registered) or a
    // non-enum scrutinee would otherwise collapse to a single opaque column
    // and mark every arm after the first unreachable, which is actively
    // misleading. Report nothing rather than a wrong diagnostic.
    if col.is_none() {
        return Usefulness::default();
    }
    let types = [col];

    let mut unreachable_arms = Vec::new();
    for i in 0..arms.len() {
        let matrix: Vec<Vec<Pattern>> = arms[..i].iter().map(|p| vec![p.clone()]).collect();
        let row = [arms[i].clone()];
        if !is_useful(&matrix, &row, &types, enum_defs) {
            unreachable_arms.push(i);
        }
    }

    let full_matrix: Vec<Vec<Pattern>> = arms.iter().map(|p| vec![p.clone()]).collect();
    let missing = useful_witnesses(&full_matrix, &types, enum_defs)
        .into_iter()
        .map(|mut row| Witness(row.remove(0)))
        .collect();

    Usefulness { missing, unreachable_arms }
}

// ---------- constructor / column helpers ----------

/// The variant name a pattern's head names, restricted to variants that
/// actually belong to `edef`. `Pattern::Variant` matches a member by name
/// (any payload shape); `Pattern::Binding` matches only a UNIT member (the
/// parser's dumb bare-identifier reading of a unit-variant reference,
/// reinterpreted here now that the column's enum is known). A `Wildcard`, an
/// ordinary capture `Binding` (names no member), and - crucially - a
/// `Variant` whose name is NOT a member of `edef` (a stale/mismatched name in
/// a mid-edit LSP buffer) all return `None`, so an unknown constructor is
/// treated uniformly as an inert wildcard by every matrix operation rather
/// than crashing a downstream lookup.
pub(crate) fn head_variant_name<'a>(p: &'a Pattern, edef: &EnumDef) -> Option<&'a str> {
    match p {
        Pattern::Variant { variant, .. } => {
            if edef.variants.iter().any(|v| &v.name == variant) {
                Some(variant.as_str())
            } else {
                None
            }
        }
        Pattern::Binding { name, .. } => {
            if edef.variants.iter().any(|v| &v.name == name && matches!(v.payload, Payload::Unit)) {
                Some(name.as_str())
            } else {
                None
            }
        }
        Pattern::Wildcard(_) => None,
    }
}

pub(crate) fn payload_arity(payload: &Payload) -> usize {
    match payload {
        Payload::Unit => 0,
        Payload::Positional(types) => types.len(),
        Payload::Named(fields) => fields.len(),
    }
}

/// The concrete enum instance governing one match column: its declaration plus
/// the generic type arguments it was instantiated with (empty for a
/// non-generic enum). Threading `args` is what lets a nested type-parameter
/// payload column resolve THROUGH the scrutinee's instantiation - `Option<Inner>`'s
/// `Some(T)` slot is recognized as `Inner`, not an opaque `T` - so nested
/// exhaustiveness over a generic enum is correct rather than silently
/// over-approximated. Consistent with `typecheck::infer::resolve_payload_field_type`
/// (the binding-side resolver), which substitutes the same args.
#[derive(Clone)]
pub(crate) struct ColEnum<'a> {
    pub edef: &'a EnumDef,
    pub args: Vec<Type>,
}

/// Resolve a payload field's declared `TypeExpr` to a concrete `Type`, seen
/// through the PARENT enum instance's generic arguments: a bare type parameter
/// of the parent becomes the corresponding parent argument, and a generic enum
/// name carries its own arguments (each itself resolved through the parent).
/// Only the enum structure matters to the usefulness engine, so any non-enum
/// field collapses to `Type::Any` (an opaque column).
fn resolve_field_type(te: &TypeExpr, parent: &ColEnum, enum_defs: &HashMap<String, EnumDef>) -> Type {
    match te {
        TypeExpr::Name { name, .. } => {
            if let Some(idx) = parent.edef.type_params.iter().position(|p| &p.name == name) {
                return parent.args.get(idx).cloned().unwrap_or(Type::Any);
            }
            if enum_defs.contains_key(name) {
                return Type::Enum { name: name.clone(), args: vec![] };
            }
            Type::Any
        }
        TypeExpr::Generic { name, args, .. } => {
            if enum_defs.contains_key(name) {
                return Type::Enum {
                    name: name.clone(),
                    args: args.iter().map(|a| resolve_field_type(a, parent, enum_defs)).collect(),
                };
            }
            Type::Any
        }
        _ => Type::Any,
    }
}

/// Resolve a payload field's declared type to the concrete enum instance
/// governing it, seen through its PARENT enum's instantiation. A field that is
/// not (or does not instantiate to) a registered enum - a scalar, a type
/// parameter bound to a non-enum, ... - resolves to `None`, an opaque column.
pub(crate) fn resolve_col_type<'a>(
    te: &TypeExpr,
    parent: &ColEnum<'a>,
    enum_defs: &'a HashMap<String, EnumDef>,
) -> Option<ColEnum<'a>> {
    match resolve_field_type(te, parent, enum_defs) {
        Type::Enum { name, args } => enum_defs.get(&name).map(|edef| ColEnum { edef, args }),
        _ => None,
    }
}

/// The column type of each of `payload`'s fields, in DECLARED order (named
/// payloads are positional here, same as everywhere else in this module),
/// resolved through the `parent` enum instance whose variant owns them.
fn payload_col_types<'a>(
    payload: &Payload,
    parent: &ColEnum<'a>,
    enum_defs: &'a HashMap<String, EnumDef>,
) -> Vec<Option<ColEnum<'a>>> {
    match payload {
        Payload::Unit => vec![],
        Payload::Positional(types) => {
            types.iter().map(|t| resolve_col_type(t, parent, enum_defs)).collect()
        }
        Payload::Named(fields) => {
            fields.iter().map(|(_, t)| resolve_col_type(t, parent, enum_defs)).collect()
        }
    }
}

/// A variant's declared payload fields, in declared order, each paired with
/// the sub-pattern a MATCHING `VariantPattern` supplies for it - or a
/// wildcard when the field's sub-pattern is absent (a named payload's
/// `ignore_rest`, or a field the pattern simply did not list).
fn variant_pattern_columns(payload: &Payload, sub: &VariantPattern) -> Vec<Pattern> {
    match (payload, sub) {
        (Payload::Unit, _) => vec![],
        (Payload::Positional(_), VariantPattern::Positional(pats)) => pats.clone(),
        (Payload::Named(decl_fields), VariantPattern::Named { fields, .. }) => decl_fields
            .iter()
            .map(|(fname, _)| {
                fields
                    .iter()
                    .find(|(n, _)| n == fname)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_else(|| Pattern::Wildcard(SourceRange::default()))
            })
            .collect(),
        _ => vec![Pattern::Wildcard(SourceRange::default()); payload_arity(payload)],
    }
}

/// Expand a pattern already known to match constructor `v` (via
/// `head_variant_name`) into its sub-columns. Anything that is not itself a
/// `Variant` (a `Wildcard`, or a `Binding` reinterpreted as `v`'s unit
/// variant) carries no sub-patterns of its own, so it expands to wildcards.
pub(crate) fn expand_matching(pattern: &Pattern, v: &VariantDef) -> Vec<Pattern> {
    let arity = payload_arity(&v.payload);
    match pattern {
        Pattern::Variant { sub, .. } => {
            let cols = variant_pattern_columns(&v.payload, sub);
            if cols.len() == arity {
                cols
            } else {
                vec![Pattern::Wildcard(SourceRange::default()); arity]
            }
        }
        _ => vec![Pattern::Wildcard(SourceRange::default()); arity],
    }
}

fn build_variant_pattern(v: &VariantDef, subs: Vec<Pattern>) -> Pattern {
    let sub = match &v.payload {
        Payload::Unit => VariantPattern::Unit,
        Payload::Positional(_) => VariantPattern::Positional(subs),
        Payload::Named(fields) => VariantPattern::Named {
            fields: fields.iter().zip(subs).map(|((name, _), p)| (name.clone(), p)).collect(),
            ignore_rest: false,
        },
    };
    Pattern::Variant { variant: v.name.clone(), sub, range: SourceRange::default() }
}

fn used_constructors(matrix: &[Vec<Pattern>], edef: &EnumDef) -> HashSet<String> {
    matrix
        .iter()
        .filter_map(|row| head_variant_name(&row[0], edef).map(|s| s.to_string()))
        .collect()
}

// ---------- matrix operations ----------

/// `Specialize(v, matrix)`: rows whose head names a DIFFERENT constructor
/// are dropped; a wildcard-like row (matches every constructor) expands to
/// `arity` wildcards; a row that already names `v` expands to its own
/// sub-patterns.
fn specialize(matrix: &[Vec<Pattern>], edef: &EnumDef, v: &VariantDef, arity: usize) -> Vec<Vec<Pattern>> {
    let mut out = Vec::with_capacity(matrix.len());
    for row in matrix {
        match head_variant_name(&row[0], edef) {
            None => {
                let mut new_row = vec![Pattern::Wildcard(SourceRange::default()); arity];
                new_row.extend(row[1..].iter().cloned());
                out.push(new_row);
            }
            Some(name) if name == v.name => {
                let mut new_row = expand_matching(&row[0], v);
                new_row.extend(row[1..].iter().cloned());
                out.push(new_row);
            }
            Some(_) => {}
        }
    }
    out
}

/// `Default(matrix)`: keep only wildcard-like rows, dropping their head
/// column - the rows that impose no constraint on this column at all.
fn default_matrix(matrix: &[Vec<Pattern>], edef: &EnumDef) -> Vec<Vec<Pattern>> {
    matrix
        .iter()
        .filter(|row| head_variant_name(&row[0], edef).is_none())
        .map(|row| row[1..].to_vec())
        .collect()
}

// ---------- usefulness ----------

/// Is `row` useful with respect to `matrix` (i.e. does some value it
/// matches escape everything `matrix` already matches)? `types[i]` is the
/// enum governing column `i`, or `None` for an opaque (always-wildcard)
/// column. Structurally parallel to [`useful_witnesses`] (same
/// opaque/complete/incomplete-signature split over the same
/// specialize/default operations); an edit to one almost certainly needs the
/// mirror edit in the other.
fn is_useful(
    matrix: &[Vec<Pattern>],
    row: &[Pattern],
    types: &[Option<ColEnum>],
    enum_defs: &HashMap<String, EnumDef>,
) -> bool {
    let Some((head, rest_types)) = types.split_first() else {
        return matrix.is_empty();
    };
    match head {
        None => {
            let sub_matrix: Vec<Vec<Pattern>> = matrix.iter().map(|r| r[1..].to_vec()).collect();
            is_useful(&sub_matrix, &row[1..], rest_types, enum_defs)
        }
        // A name from `head_variant_name` is already restricted to members of
        // `edef`, so this `find` cannot miss for well-formed input; folding it
        // into the dispatch keeps an unknown/mismatched constructor on the
        // wildcard path instead of panicking.
        Some(col) => match head_variant_name(&row[0], col.edef).and_then(|name| col.edef.variants.iter().find(|v| v.name == name)) {
            Some(v) => {
                let arity = payload_arity(&v.payload);
                let mut new_types = payload_col_types(&v.payload, col, enum_defs);
                new_types.extend(rest_types.iter().cloned());
                let specialized = specialize(matrix, col.edef, v, arity);
                let mut new_row = expand_matching(&row[0], v);
                new_row.extend(row[1..].iter().cloned());
                is_useful(&specialized, &new_row, &new_types, enum_defs)
            }
            None => {
                let used = used_constructors(matrix, col.edef);
                let is_complete = col.edef.variants.iter().all(|v| used.contains(&v.name));
                if is_complete {
                    col.edef.variants.iter().any(|v| {
                        let arity = payload_arity(&v.payload);
                        let mut new_types = payload_col_types(&v.payload, col, enum_defs);
                        new_types.extend(rest_types.iter().cloned());
                        let specialized = specialize(matrix, col.edef, v, arity);
                        let mut new_row = vec![Pattern::Wildcard(SourceRange::default()); arity];
                        new_row.extend(row[1..].iter().cloned());
                        is_useful(&specialized, &new_row, &new_types, enum_defs)
                    })
                } else {
                    let dmat = default_matrix(matrix, col.edef);
                    is_useful(&dmat, &row[1..], rest_types, enum_defs)
                }
            }
        },
    }
}

/// The witness (`I`) construction: the set of pattern rows (parallel to
/// `types`) that an all-wildcard probe would match but `matrix` does not -
/// i.e. exactly the missing patterns. Empty means `matrix` is exhaustive
/// over `types`. Structurally parallel to [`is_useful`] (same
/// opaque/complete/incomplete-signature split over the same
/// specialize/default operations); an edit to one almost certainly needs the
/// mirror edit in the other.
fn useful_witnesses(
    matrix: &[Vec<Pattern>],
    types: &[Option<ColEnum>],
    enum_defs: &HashMap<String, EnumDef>,
) -> Vec<Vec<Pattern>> {
    let Some((head, rest_types)) = types.split_first() else {
        return if matrix.is_empty() { vec![vec![]] } else { vec![] };
    };
    match head {
        None => {
            let sub_matrix: Vec<Vec<Pattern>> = matrix.iter().map(|row| row[1..].to_vec()).collect();
            useful_witnesses(&sub_matrix, rest_types, enum_defs)
                .into_iter()
                .map(|mut w| {
                    w.insert(0, Pattern::Wildcard(SourceRange::default()));
                    w
                })
                .collect()
        }
        Some(col) => {
            let edef = col.edef;
            let used = used_constructors(matrix, edef);
            let missing: Vec<&VariantDef> = edef.variants.iter().filter(|v| !used.contains(&v.name)).collect();
            if missing.is_empty() {
                // Complete signature: every constructor appears somewhere in
                // the matrix, so recurse into each one - this is where a
                // NESTED hole (a present constructor whose own payload isn't
                // fully covered) surfaces, e.g. `Node(None)`.
                let mut out = Vec::new();
                for v in &edef.variants {
                    let arity = payload_arity(&v.payload);
                    let mut new_types = payload_col_types(&v.payload, col, enum_defs);
                    new_types.extend(rest_types.iter().cloned());
                    let specialized = specialize(matrix, edef, v, arity);
                    for w in useful_witnesses(&specialized, &new_types, enum_defs) {
                        let (sub, tail) = w.split_at(arity);
                        let mut row = vec![build_variant_pattern(v, sub.to_vec())];
                        row.extend(tail.iter().cloned());
                        out.push(row);
                    }
                }
                out
            } else {
                // Incomplete signature: every constructor NOT tested at all
                // is entirely missing (wildcard sub-patterns - nothing
                // constrains its payload), combined with whatever the rest
                // of the columns still need.
                let dmat = default_matrix(matrix, edef);
                let rest_witnesses = useful_witnesses(&dmat, rest_types, enum_defs);
                let mut out = Vec::new();
                for v in &missing {
                    let arity = payload_arity(&v.payload);
                    let pat = build_variant_pattern(v, vec![Pattern::Wildcard(SourceRange::default()); arity]);
                    for tail in &rest_witnesses {
                        let mut row = vec![pat.clone()];
                        row.extend(tail.iter().cloned());
                        out.push(row);
                    }
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TypeParam;
    use crate::parser::parse_pattern_str;

    fn dummy_range() -> SourceRange {
        SourceRange::default()
    }

    fn enum_ty(name: &str) -> Type {
        Type::Enum { name: name.to_string(), args: vec![] }
    }

    fn type_name(name: &str) -> TypeExpr {
        TypeExpr::Name { name: name.to_string(), range: dummy_range() }
    }

    fn variant(name: &str, disc: i64, payload: Payload) -> VariantDef {
        VariantDef { name: name.to_string(), discriminant: disc, payload }
    }

    fn shape_enum_def() -> EnumDef {
        EnumDef {
            name: "Shape".to_string(),
            type_params: vec![],
            variants: vec![
                variant("Empty", 0, Payload::Unit),
                variant("Circle", 1, Payload::Positional(vec![type_name("float")])),
                variant("Rect", 2, Payload::Positional(vec![type_name("float"), type_name("float")])),
            ],
        }
    }

    fn option_enum_def() -> EnumDef {
        EnumDef {
            name: "Option".to_string(),
            type_params: vec![TypeParam { name: "T".to_string(), bound: None, range: dummy_range() }],
            variants: vec![
                variant("Some", 0, Payload::Positional(vec![type_name("T")])),
                variant("None", 1, Payload::Unit),
            ],
        }
    }

    fn enums() -> HashMap<String, EnumDef> {
        let mut m: HashMap<String, EnumDef> = Default::default();
        m.insert("Shape".to_string(), shape_enum_def());
        m.insert("Option".to_string(), option_enum_def());
        m
    }

    fn tree_enum_def() -> EnumDef {
        EnumDef {
            name: "Tree".to_string(),
            type_params: vec![],
            variants: vec![
                variant("Leaf", 0, Payload::Positional(vec![type_name("int")])),
                variant(
                    "Node",
                    1,
                    Payload::Positional(vec![TypeExpr::Generic {
                        name: "Option".to_string(),
                        args: vec![type_name("int")],
                        range: dummy_range(),
                    }]),
                ),
            ],
        }
    }

    fn tree_enums() -> HashMap<String, EnumDef> {
        let mut m = enums();
        m.insert("Tree".to_string(), tree_enum_def());
        m
    }

    fn box_enum_def() -> EnumDef {
        EnumDef {
            name: "Box".to_string(),
            type_params: vec![],
            variants: vec![variant(
                "Dims",
                0,
                Payload::Named(vec![
                    ("w".to_string(), type_name("float")),
                    ("h".to_string(), type_name("float")),
                ]),
            )],
        }
    }

    fn box_enums() -> HashMap<String, EnumDef> {
        let mut m: HashMap<String, EnumDef> = Default::default();
        m.insert("Box".to_string(), box_enum_def());
        m
    }

    fn pats(strs: &[&str]) -> Vec<Pattern> {
        strs.iter().map(|s| parse_pattern_str(s)).collect()
    }

    #[test]
    fn missing_variant_is_a_witness() {
        let u = analyze(&enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "Empty"]));
        // Rect is unmatched.
        assert!(u.missing.iter().any(|w| matches!(&w.0, Pattern::Variant { variant, .. } if variant == "Rect")));
    }

    #[test]
    fn full_cover_has_no_witness() {
        let u = analyze(&enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "Rect(_, _)", "Empty"]));
        assert!(u.missing.is_empty());
        assert!(u.unreachable_arms.is_empty());
    }

    #[test]
    fn wildcard_covers_the_rest() {
        let u = analyze(&enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "_"]));
        assert!(u.missing.is_empty());
    }

    #[test]
    fn arm_after_wildcard_is_unreachable() {
        let u = analyze(&enums(), &enum_ty("Shape"), &pats(&["_", "Empty"]));
        assert_eq!(u.unreachable_arms, vec![1]);
    }

    #[test]
    fn nested_missing_is_a_nested_witness() {
        // enum Tree { Leaf(int), Node(Option<int>) }; match Node(Some(_)) +
        // Leaf(_): Node(None) is missing.
        let u = analyze(&tree_enums(), &enum_ty("Tree"), &pats(&["Node(Some(_))", "Leaf(_)"]));
        assert!(u.missing.iter().any(|w| format!("{:?}", w.0).contains("None")));
    }

    #[test]
    fn unknown_variant_name_does_not_panic() {
        // A pattern naming a variant that is NOT a member of the scrutinee
        // enum (e.g. a stale name in a mid-edit LSP buffer) must degrade
        // gracefully rather than crash the missing-variant lookup. The
        // regression guard is that this call returns at all; the unknown
        // `Triangle` is treated as an inert wildcard (catch-all), so no
        // witness is produced.
        let u = analyze(&enums(), &enum_ty("Shape"), &pats(&["Triangle(_)", "Empty"]));
        assert!(u.missing.is_empty());
    }

    fn inner_enum_def() -> EnumDef {
        EnumDef {
            name: "Inner".to_string(),
            type_params: vec![],
            variants: vec![variant("On", 0, Payload::Unit), variant("Off", 1, Payload::Unit)],
        }
    }

    fn opt_inner_enums() -> HashMap<String, EnumDef> {
        let mut m = enums();
        m.insert("Inner".to_string(), inner_enum_def());
        m
    }

    fn opt_inner_ty() -> Type {
        Type::Enum { name: "Option".to_string(), args: vec![enum_ty("Inner")] }
    }

    #[test]
    fn generic_scrutinee_resolves_nested_payload_enum() {
        // match o: Option<Inner> { Some(On) => .., None => .. }: `Some(T)`'s
        // slot must resolve THROUGH the instantiation to `Inner`, so `Some(Off)`
        // is a missing witness. Without threading the args the payload is opaque
        // and this wrongly reports exhaustive.
        let u = analyze(&opt_inner_enums(), &opt_inner_ty(), &pats(&["Some(On)", "None"]));
        assert!(
            u.missing.iter().any(|w| format!("{:?}", w.0).contains("Off")),
            "expected a Some(Off) witness, got {:?}",
            u.missing
        );
    }

    #[test]
    fn generic_scrutinee_full_cover_has_no_witness() {
        let u = analyze(
            &opt_inner_enums(),
            &opt_inner_ty(),
            &pats(&["Some(On)", "Some(Off)", "None"]),
        );
        assert!(u.missing.is_empty(), "{:?}", u.missing);
    }

    #[test]
    fn named_payload_witness_has_named_sub() {
        // enum Box { Dims { w: float, h: float } } with no arms: the single
        // missing witness is `Dims` carrying two wildcard fields.
        let u = analyze(&box_enums(), &enum_ty("Box"), &[]);
        assert!(u.missing.iter().any(|w| matches!(
            &w.0,
            Pattern::Variant { variant, sub: VariantPattern::Named { fields, .. }, .. }
                if variant == "Dims" && fields.len() == 2
        )));
    }
}
