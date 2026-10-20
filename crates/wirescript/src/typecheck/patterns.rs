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
/// payload fields, not just the scrutinee's own enum). `guarded[i]` says arm
/// `i` has an `if` guard: it can still be unreachable, but it covers nothing,
/// since its guard may be false.
pub fn analyze(enums: Enums<'_>, scrutinee: &Type, arms: &[Pattern], guarded: &[bool]) -> Usefulness {
    let is_guarded = |i: usize| guarded.get(i).copied().unwrap_or(false);
    let col = match scrutinee {
        Type::Enum { args, .. } => enums.defs.of(scrutinee).map(|edef| ColEnum { edef, args: args.clone() }),
        Type::Int | Type::Float | Type::String | Type::Bool => return analyze_value(enums, scrutinee, arms, guarded),
        _ => None,
    };
    // Without a resolved enum for the scrutinee there is no constructor set to
    // reason about: a registry desync (enum named but not registered) or a
    // scrutinee of a type no pattern can test would otherwise collapse to a
    // single opaque column and mark every arm after the first unreachable,
    // which is actively misleading. Report nothing rather than a wrong
    // diagnostic.
    let Some(col) = col else {
        return Usefulness::default();
    };
    let edef = col.edef;

    // Specialized top-level driver: the matrix has exactly one column (the
    // scrutinee), so instead of rebuilding an i-row matrix per arm and letting
    // `is_useful` re-scan it (quadratic in arms, with a linear variant-name
    // scan inside), group arms by head constructor once and keep one
    // pre-specialized sub-matrix per constructor. Semantics are identical to
    // running `is_useful`/`useful_witnesses` over the full matrix.
    let vmap: HashMap<&str, &VariantDef> =
        edef.variants.iter().map(|v| (v.name.as_str(), v)).collect();
    // Same reading as `head_variant_name`: a `Variant` names any member, a
    // bare `Binding` only a UNIT member; anything else is wildcard-like.
    let head_of = |p: &Pattern| -> Option<&VariantDef> {
        match p {
            Pattern::Variant { variant, .. } => vmap.get(variant.as_str()).copied(),
            Pattern::Binding { name, .. } => vmap
                .get(name.as_str())
                .copied()
                .filter(|v| matches!(v.payload, Payload::Unit)),
            _ => None,
        }
    };

    struct Spec<'a> {
        types: Vec<Option<ColEnum<'a>>>,
        rows: Vec<Vec<Pattern>>,
    }
    let mut spec: HashMap<&str, Spec> = HashMap::default();
    // A wildcard-like arm is a full catch-all in a single-column matrix: every
    // arm after it is unreachable, and nothing can be missing past it.
    let mut seen_catchall = false;
    let mut unreachable_arms = Vec::new();

    for (i, arm) in arms.iter().enumerate() {
        // An arm of alternatives is reachable when one of them is, and each
        // alternative covers what it would alone.
        let mut reachable = false;
        for alt in or_alternatives(arm) {
            match head_of(alt) {
                Some(v) => {
                    let row = expand_matching(alt, v);
                    let useful = match spec.get(v.name.as_str()) {
                        Some(entry) => is_useful(&entry.rows, &row, &entry.types, enums),
                        None => is_useful(&[], &row, &payload_col_types(&v.payload, &col, enums), enums),
                    };
                    reachable |= !seen_catchall && useful;
                    // A guarded arm covers nothing: its guard may be false.
                    if !is_guarded(i) {
                        spec.entry(v.name.as_str())
                            .or_insert_with(|| Spec { types: payload_col_types(&v.payload, &col, enums), rows: Vec::new() })
                            .rows
                            .push(row);
                    }
                }
                // A value test against an enum is a type error of its own; it
                // covers nothing and is not reported again here.
                None if is_value_test(alt) => reachable = true,
                None => {
                    // The first catch-all is always reachable: a runtime tag
                    // that names no variant (`Enum.FromInt(n)` out of range, a
                    // pin) matches no variant arm and runs it. The next
                    // wildcard-like arm short-circuits on `seen_catchall`.
                    reachable |= !seen_catchall;
                    if !is_guarded(i) {
                        seen_catchall = true;
                    }
                }
            }
        }
        if !reachable {
            unreachable_arms.push(i);
        }
    }

    // Witnesses, mirroring `useful_witnesses`' top-level split exactly: a
    // catch-all row covers everything; an INCOMPLETE signature reports only
    // the untested constructors (payload holes of tested ones are not
    // descended into); a complete one recurses into each constructor.
    let mut missing = Vec::new();
    if !seen_catchall {
        let all_used = edef.variants.iter().all(|v| spec.contains_key(v.name.as_str()));
        if all_used {
            for v in &edef.variants {
                let s = &spec[v.name.as_str()];
                for w in useful_witnesses(&s.rows, &s.types, enums) {
                    missing.push(Witness(build_variant_pattern(v, w)));
                }
            }
        } else {
            for v in &edef.variants {
                if !spec.contains_key(v.name.as_str()) {
                    let arity = payload_arity(&v.payload);
                    missing.push(Witness(build_variant_pattern(
                        v,
                        vec![Pattern::Wildcard(SourceRange::default()); arity],
                    )));
                }
            }
        }
    }

    Usefulness { missing, unreachable_arms }
}

/// [`analyze`] for a scrutinee that is a single value (int, float, string or
/// bool), over the general matrix: one column, one row per arm.
fn analyze_value(enums: Enums<'_>, scrutinee: &Type, arms: &[Pattern], guarded: &[bool]) -> Usefulness {
    let types = vec![value_col(scrutinee)];
    let mut matrix: Vec<Vec<Pattern>> = Vec::new();
    let mut unreachable_arms = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        let row = vec![arm.clone()];
        if !is_useful(&matrix, &row, &types, enums) {
            unreachable_arms.push(i);
        }
        if !guarded.get(i).copied().unwrap_or(false) {
            matrix.push(row);
        }
    }
    let missing = useful_witnesses(&matrix, &types, enums)
        .into_iter()
        .filter_map(|mut w| w.pop().map(Witness))
        .collect();
    Usefulness { missing, unreachable_arms }
}

/// The two values of a `bool`, as the constructors of an enum, so coverage
/// treats `true` and `false` as a complete set.
pub(crate) fn bool_enum() -> &'static EnumDef {
    static BOOL: std::sync::OnceLock<EnumDef> = std::sync::OnceLock::new();
    BOOL.get_or_init(|| EnumDef {
        name: "bool".to_string(),
        type_params: vec![],
        variants: ["false", "true"]
            .iter()
            .enumerate()
            .map(|(i, n)| VariantDef { name: n.to_string(), discriminant: i as i64, payload: Payload::Unit })
            .collect(),
    })
}

/// The coverage column for a value of type `t` that is not an enum: a bool
/// has two constructors; any other value is opaque.
fn value_col(t: &Type) -> Option<ColEnum<'static>> {
    matches!(t, Type::Bool).then(|| ColEnum { edef: bool_enum(), args: vec![] })
}

/// `p`'s alternatives, nested `|` flattened; a pattern that is no `|` is its
/// own single alternative.
pub(crate) fn or_alternatives(p: &Pattern) -> Vec<&Pattern> {
    match p {
        Pattern::Or { alts, .. } => alts.iter().flat_map(or_alternatives).collect(),
        _ => vec![p],
    }
}

/// A literal, range or string pattern (or alternatives of only those): it
/// tests a value rather than naming a constructor or catching everything.
pub(crate) fn is_value_test(p: &Pattern) -> bool {
    match p {
        Pattern::Literal { .. } | Pattern::Range { .. } | Pattern::Concat { .. } => true,
        Pattern::Or { alts, .. } => alts.iter().all(is_value_test),
        _ => false,
    }
}

/// Rows with a `|` in their first column split into one row per alternative,
/// which together cover exactly what the original row did.
fn expand_or_heads(matrix: &[Vec<Pattern>]) -> std::borrow::Cow<'_, [Vec<Pattern>]> {
    if !matrix.iter().any(|r| matches!(r.first(), Some(Pattern::Or { .. }))) {
        return std::borrow::Cow::Borrowed(matrix);
    }
    let mut out = Vec::with_capacity(matrix.len());
    for row in matrix {
        for alt in or_alternatives(&row[0]) {
            let mut r = vec![alt.clone()];
            r.extend(row[1..].iter().cloned());
            out.push(r);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// A key equal for two value tests that match exactly the same values, as far
/// as their spelling shows: the same literal, the same range, the same string
/// parts. `None` for anything else.
fn value_test_key(p: &Pattern) -> Option<String> {
    fn lit(e: &crate::ast::Expr) -> Option<String> {
        use crate::ast::Expr;
        Some(match e {
            Expr::IntLit { value, .. } | Expr::AtomLit { value, .. } => format!("i{value}"),
            Expr::FloatLit { value, .. } => format!("f{value}"),
            Expr::StringLit { value, .. } => format!("s{value:?}"),
            Expr::BoolLit { value, .. } => format!("b{value}"),
            _ => return None,
        })
    }
    match p {
        Pattern::Literal { expr, .. } => lit(expr),
        Pattern::Range { lo, hi, .. } => Some(format!("{}..{}", lit(lo)?, lit(hi)?)),
        Pattern::Concat { parts, .. } => {
            let mut key = String::from("c");
            for part in parts {
                match part {
                    Pattern::Literal { expr, .. } => key.push_str(&lit(expr)?),
                    _ => key.push('*'),
                }
            }
            Some(key)
        }
        _ => None,
    }
}

/// Does an opaque-column cell `m` match every value the probe `q` matches?
/// Only a catch-all, or the very same test, is known to.
fn opaque_covers(m: &Pattern, q: &Pattern) -> bool {
    if is_catchall(m) {
        return true;
    }
    if is_catchall(q) {
        return false;
    }
    matches!((value_test_key(m), value_test_key(q)), (Some(a), Some(b)) if a == b)
}

/// A pattern that matches any value of an opaque column.
fn is_catchall(p: &Pattern) -> bool {
    match p {
        Pattern::Wildcard(_) | Pattern::Binding { .. } => true,
        Pattern::Or { alts, .. } => alts.iter().any(is_catchall),
        _ => false,
    }
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
        // `true`/`false` are the constructors of `bool_enum`, and of no
        // declared enum.
        Pattern::Literal { expr, .. } => match **expr {
            crate::ast::Expr::BoolLit { value, .. } if std::ptr::eq(edef, bool_enum()) => {
                Some(if value { "true" } else { "false" })
            }
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn payload_arity(payload: &Payload) -> usize {
    match payload {
        Payload::Unit => 0,
        Payload::Positional(types) => types.len(),
        Payload::Named(fields) => fields.len(),
    }
}

/// The enum registry, with the type declarations a payload field's type is
/// read against: those of the file declaring the enum, aliases included.
#[derive(Clone, Copy)]
pub struct Enums<'a> {
    pub defs: &'a crate::typecheck::enums::EnumRegistry,
    pub types: &'a crate::const_eval::DeclaredTypes,
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
pub(crate) fn resolve_field_type(te: &TypeExpr, parent: &ColEnum, enums: Enums<'_>) -> Type {
    let params = &parent.edef.type_params;
    let param = |name: &str, _: &SourceRange| {
        params.iter().position(|p| p.name == name).map(|i| parent.args.get(i).cloned().unwrap_or(Type::Any))
    };
    // Read where the enum declares the field: through that file's aliases.
    enums.types.in_file(&te.range().file).resolve_scoped(te, &param)
}

/// Resolve a payload field's declared type to the concrete enum instance
/// governing it, seen through its PARENT enum's instantiation. A field that is
/// not (or does not instantiate to) a registered enum - a scalar, a type
/// parameter bound to a non-enum, ... - resolves to `None`, an opaque column.
pub(crate) fn resolve_col_type<'a>(
    te: &TypeExpr,
    parent: &ColEnum<'a>,
    enums: Enums<'a>,
) -> Option<ColEnum<'a>> {
    let t = resolve_field_type(te, parent, enums);
    let edef = enums.defs.of(&t)?;
    let Type::Enum { args, .. } = t else { return None };
    Some(ColEnum { edef, args })
}

/// The column type of each of `payload`'s fields, in DECLARED order (named
/// payloads are positional here, same as everywhere else in this module),
/// resolved through the `parent` enum instance whose variant owns them.
fn payload_col_types<'a>(
    payload: &Payload,
    parent: &ColEnum<'a>,
    enums: Enums<'a>,
) -> Vec<Option<ColEnum<'a>>> {
    // A `bool` field is a column of two constructors here; the decision tree
    // tests it as a value instead, so this is not `resolve_col_type`'s job.
    let col = |te: &TypeExpr| match resolve_field_type(te, parent, enums) {
        Type::Bool => value_col(&Type::Bool),
        _ => resolve_col_type(te, parent, enums),
    };
    match payload {
        Payload::Unit => vec![],
        Payload::Positional(types) => types.iter().map(col).collect(),
        Payload::Named(fields) => fields.iter().map(|(_, t)| col(t)).collect(),
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
    Pattern::Variant { enum_path: None, variant: v.name.clone(), sub, range: SourceRange::default() }
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
            // A value test in an enum column matches no constructor.
            None if is_value_test(&row[0]) => {}
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
        .filter(|row| head_variant_name(&row[0], edef).is_none() && !is_value_test(&row[0]))
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
    enums: Enums<'_>,
) -> bool {
    let Some((head, rest_types)) = types.split_first() else {
        return matrix.is_empty();
    };
    // Alternatives: useful when any one of them is.
    if let Pattern::Or { alts, .. } = &row[0] {
        return alts.iter().any(|alt| {
            let mut r = vec![alt.clone()];
            r.extend(row[1..].iter().cloned());
            is_useful(matrix, &r, types, enums)
        });
    }
    let expanded = expand_or_heads(matrix);
    let matrix = &*expanded;
    match head {
        // An opaque column: literals, ranges and string patterns test it, but
        // only a catch-all, or the very same test, covers what the probe
        // matches. Anything else might miss some of it, so it is left out,
        // which can only call an arm reachable, never wrongly unreachable.
        None => {
            let sub_matrix: Vec<Vec<Pattern>> =
                matrix.iter().filter(|r| opaque_covers(&r[0], &row[0])).map(|r| r[1..].to_vec()).collect();
            is_useful(&sub_matrix, &row[1..], rest_types, enums)
        }
        // A value test in an enum column is a type error of its own.
        Some(col) if is_value_test(&row[0]) && head_variant_name(&row[0], col.edef).is_none() => true,
        // A name from `head_variant_name` is already restricted to members of
        // `edef`, so this `find` cannot miss for well-formed input; folding it
        // into the dispatch keeps an unknown/mismatched constructor on the
        // wildcard path instead of panicking.
        Some(col) => match head_variant_name(&row[0], col.edef).and_then(|name| col.edef.variants.iter().find(|v| v.name == name)) {
            Some(v) => {
                let arity = payload_arity(&v.payload);
                let mut new_types = payload_col_types(&v.payload, col, enums);
                new_types.extend(rest_types.iter().cloned());
                let specialized = specialize(matrix, col.edef, v, arity);
                let mut new_row = expand_matching(&row[0], v);
                new_row.extend(row[1..].iter().cloned());
                is_useful(&specialized, &new_row, &new_types, enums)
            }
            None => {
                let used = used_constructors(matrix, col.edef);
                let is_complete = col.edef.variants.iter().all(|v| used.contains(&v.name));
                if is_complete {
                    col.edef.variants.iter().any(|v| {
                        let arity = payload_arity(&v.payload);
                        let mut new_types = payload_col_types(&v.payload, col, enums);
                        new_types.extend(rest_types.iter().cloned());
                        let specialized = specialize(matrix, col.edef, v, arity);
                        let mut new_row = vec![Pattern::Wildcard(SourceRange::default()); arity];
                        new_row.extend(row[1..].iter().cloned());
                        is_useful(&specialized, &new_row, &new_types, enums)
                    })
                } else {
                    let dmat = default_matrix(matrix, col.edef);
                    is_useful(&dmat, &row[1..], rest_types, enums)
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
    enums: Enums<'_>,
) -> Vec<Vec<Pattern>> {
    let Some((head, rest_types)) = types.split_first() else {
        return if matrix.is_empty() { vec![vec![]] } else { vec![] };
    };
    let expanded = expand_or_heads(matrix);
    let matrix = &*expanded;
    match head {
        // Only a catch-all covers an opaque column: there is always a value
        // that no literal, range or string pattern matches.
        None => {
            let sub_matrix: Vec<Vec<Pattern>> =
                matrix.iter().filter(|row| is_catchall(&row[0])).map(|row| row[1..].to_vec()).collect();
            useful_witnesses(&sub_matrix, rest_types, enums)
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
                    let mut new_types = payload_col_types(&v.payload, col, enums);
                    new_types.extend(rest_types.iter().cloned());
                    let specialized = specialize(matrix, edef, v, arity);
                    for w in useful_witnesses(&specialized, &new_types, enums) {
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
                let rest_witnesses = useful_witnesses(&dmat, rest_types, enums);
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
    use crate::typecheck::enums::EnumRegistry;

    /// A test registry with the type declarations its payload fields are
    /// read against.
    struct Reg {
        defs: std::sync::Arc<EnumRegistry>,
        types: crate::const_eval::DeclaredTypes,
    }

    impl Reg {
        fn new(defs: EnumRegistry) -> Self {
            let defs = std::sync::Arc::new(defs);
            let types = crate::const_eval::DeclaredTypes::for_program(&[], &defs);
            Self { defs, types }
        }

        fn enums(&self) -> Enums<'_> {
            Enums { defs: &self.defs, types: &self.types }
        }
    }

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

    fn enums() -> EnumRegistry {
        let mut m: EnumRegistry = Default::default();
        m.insert(shape_enum_def());
        m.insert(option_enum_def());
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

    fn tree_enums() -> EnumRegistry {
        let mut m = enums();
        m.insert(tree_enum_def());
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

    fn box_enums() -> EnumRegistry {
        let mut m: EnumRegistry = Default::default();
        m.insert(box_enum_def());
        m
    }

    fn pats(strs: &[&str]) -> Vec<Pattern> {
        strs.iter().map(|s| parse_pattern_str(s)).collect()
    }

    #[test]
    fn missing_variant_is_a_witness() {
        let u = analyze(Reg::new(enums()).enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "Empty"]), &[]);
        // Rect is unmatched.
        assert!(u.missing.iter().any(|w| matches!(&w.0, Pattern::Variant { variant, .. } if variant == "Rect")));
    }

    #[test]
    fn full_cover_has_no_witness() {
        let u = analyze(Reg::new(enums()).enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "Rect(_, _)", "Empty"]), &[]);
        assert!(u.missing.is_empty());
        assert!(u.unreachable_arms.is_empty());
    }

    #[test]
    fn wildcard_covers_the_rest() {
        let u = analyze(Reg::new(enums()).enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "_"]), &[]);
        assert!(u.missing.is_empty());
    }

    #[test]
    fn arm_after_wildcard_is_unreachable() {
        let u = analyze(Reg::new(enums()).enums(), &enum_ty("Shape"), &pats(&["_", "Empty"]), &[]);
        assert_eq!(u.unreachable_arms, vec![1]);
    }

    #[test]
    fn nested_missing_is_a_nested_witness() {
        // enum Tree { Leaf(int), Node(Option<int>) }; match Node(Some(_)) +
        // Leaf(_): Node(None) is missing.
        let u = analyze(Reg::new(tree_enums()).enums(), &enum_ty("Tree"), &pats(&["Node(Some(_))", "Leaf(_)"]), &[]);
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
        let u = analyze(Reg::new(enums()).enums(), &enum_ty("Shape"), &pats(&["Triangle(_)", "Empty"]), &[]);
        assert!(u.missing.is_empty());
    }

    fn inner_enum_def() -> EnumDef {
        EnumDef {
            name: "Inner".to_string(),
            type_params: vec![],
            variants: vec![variant("On", 0, Payload::Unit), variant("Off", 1, Payload::Unit)],
        }
    }

    fn opt_inner_enums() -> EnumRegistry {
        let mut m = enums();
        m.insert(inner_enum_def());
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
        let u = analyze(Reg::new(opt_inner_enums()).enums(), &opt_inner_ty(), &pats(&["Some(On)", "None"]), &[]);
        assert!(
            u.missing.iter().any(|w| format!("{:?}", w.0).contains("Off")),
            "expected a Some(Off) witness, got {:?}",
            u.missing
        );
    }

    #[test]
    fn generic_scrutinee_full_cover_has_no_witness() {
        let u = analyze(
            Reg::new(opt_inner_enums()).enums(),
            &opt_inner_ty(),
            &pats(&["Some(On)", "Some(Off)", "None"]),
            &[],
        );
        assert!(u.missing.is_empty(), "{:?}", u.missing);
    }

    #[test]
    fn named_payload_witness_has_named_sub() {
        // enum Box { Dims { w: float, h: float } } with no arms: the single
        // missing witness is `Dims` carrying two wildcard fields.
        let u = analyze(Reg::new(box_enums()).enums(), &enum_ty("Box"), &[], &[]);
        assert!(u.missing.iter().any(|w| matches!(
            &w.0,
            Pattern::Variant { variant, sub: VariantPattern::Named { fields, .. }, .. }
                if variant == "Dims" && fields.len() == 2
        )));
    }

    fn render(u: &Usefulness) -> Vec<String> {
        u.missing.iter().map(|w| crate::typecheck::infer::render_pattern(&w.0)).collect()
    }

    #[test]
    fn literals_never_cover_a_string_so_it_needs_a_catch_all() {
        let reg = Reg::new(enums());
        let u = analyze(reg.enums(), &Type::String, &pats(&[r#""a""#, r#""b" | "c""#]), &[]);
        assert_eq!(render(&u), vec!["_"]);
        let u = analyze(reg.enums(), &Type::String, &pats(&[r#""a""#, "rest"]), &[]);
        assert!(u.missing.is_empty() && u.unreachable_arms.is_empty(), "{u:?}");
    }

    #[test]
    fn true_and_false_cover_a_bool() {
        let reg = Reg::new(enums());
        assert!(analyze(reg.enums(), &Type::Bool, &pats(&["true", "false"]), &[]).missing.is_empty());
        assert_eq!(render(&analyze(reg.enums(), &Type::Bool, &pats(&["true"]), &[])), vec!["false"]);
        // A bool payload, too.
        let mut m: EnumRegistry = Default::default();
        m.insert(EnumDef {
            name: "Flag".to_string(),
            type_params: vec![],
            variants: vec![variant("On", 0, Payload::Positional(vec![type_name("bool")]))],
        });
        let reg = Reg::new(m);
        assert_eq!(render(&analyze(reg.enums(), &enum_ty("Flag"), &pats(&["On(true)"]), &[])), vec!["On(false)"]);
    }

    #[test]
    fn a_repeated_test_is_unreachable_but_an_overlapping_one_is_not() {
        let reg = Reg::new(enums());
        let u = analyze(reg.enums(), &Type::String, &pats(&[r#""a""#, r#""a""#, r#""a" .. x"#, "_"]), &[]);
        assert_eq!(u.unreachable_arms, vec![1]);
        let u = analyze(reg.enums(), &Type::Int, &pats(&["1..5", "1..5", "3", "_", "_"]), &[]);
        assert_eq!(u.unreachable_arms, vec![1, 4]);
    }

    #[test]
    fn a_guarded_arm_covers_nothing() {
        let reg = Reg::new(enums());
        let u = analyze(reg.enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "Rect(_, _)", "Empty"]), &[false, false, true]);
        assert_eq!(render(&u), vec!["Empty"]);
        let u = analyze(reg.enums(), &Type::Int, &pats(&["n", "_"]), &[true, false]);
        assert!(u.missing.is_empty() && u.unreachable_arms.is_empty(), "{u:?}");
    }

    #[test]
    fn an_arm_of_alternatives_covers_each_one() {
        let reg = Reg::new(enums());
        let u = analyze(reg.enums(), &enum_ty("Shape"), &pats(&["Circle(_) | Rect(_, _)", "Empty"]), &[]);
        assert!(u.missing.is_empty() && u.unreachable_arms.is_empty(), "{u:?}");
        let u = analyze(reg.enums(), &enum_ty("Shape"), &pats(&["Circle(_) | Rect(_, _)", "Rect(_, _) | Circle(_)", "_"]), &[]);
        assert_eq!(u.unreachable_arms, vec![1]);
    }
}
