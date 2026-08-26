//! Decision-tree builder for `match` lowering (Maranget, "Compiling pattern
//! matching to good decision trees"). Consumes the arms' `Pattern`s plus the
//! enum registry and produces a [`Decision`] the match lowering walks: every
//! `Switch` tests a `__disc` reached by a `path` of payload-slot steps, and
//! first-match-wins is guaranteed by the clause-matrix construction. This
//! module only builds the tree; it wires up no gates itself.

use crate::ast::Pattern;
use crate::collections::HashMap;
use crate::ir::{Literal, Type};
use crate::typecheck::enums::{EnumDef, Payload, VariantDef};
// Reused from the usefulness engine (Task 11) so the Binding->unit-variant
// reinterpretation, payload expansion, and generic-instantiation column typing
// stay single-sourced with typecheck.
use crate::typecheck::patterns::{ColEnum, expand_matching, head_variant_name, payload_arity, resolve_col_type};

/// One step of the path from the scrutinee root to a nested payload value:
/// navigate into a payload slot by its interned field name (the same
/// `__disc` / `__{V}_{i}` / `__{V}_{f}` keys `build_enum_fields` lays down in
/// `predeclare`, e.g. `"__Circle_0"`).
#[derive(Clone, Debug, PartialEq)]
pub enum PathStep {
    Field(String),
}

/// A compiled `match`: run the arm named by [`Decision::Leaf`], reached by
/// testing `__disc` values down a tree of [`Decision::Switch`] nodes.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// The arm (index into the original `arms` slice) whose body runs.
    Leaf(usize),
    /// No arm matched. Only reachable for a non-exhaustive match (already a
    /// `WS054`); lowers to the else/zero path.
    Fail,
    /// Test the `__disc` at `path`: take the case whose `disc == case.0`, or
    /// `default` when no case matches.
    Switch {
        path: Vec<PathStep>,
        cases: Vec<(i64, Decision)>,
        default: Option<Box<Decision>>,
    },
}

/// One clause-matrix column: the `path` from the scrutinee root to this
/// column's value, and the concrete enum instance governing it (`None` for an
/// opaque, always wildcard-only column - a non-enum payload field). The
/// instance carries the enum's generic `args` so a nested type-parameter
/// payload column resolves through the instantiation (see
/// `typecheck::patterns::ColEnum`), keeping decision-tree column typing in step
/// with the usefulness engine's.
struct Col<'a> {
    path: Vec<PathStep>,
    ty: Option<ColEnum<'a>>,
}

/// One clause-matrix row: the original arm index it belongs to, plus one
/// sub-pattern per column (parallel to the shared column list).
struct Row {
    arm: usize,
    cells: Vec<Pattern>,
}

/// Build the decision tree for `arms` matched against `scrutinee`.
pub fn build(enum_defs: &HashMap<String, EnumDef>, scrutinee: &Type, arms: &[Pattern]) -> Decision {
    let root = match scrutinee {
        Type::Enum { name, args } => {
            enum_defs.get(name).map(|edef| ColEnum { edef, args: args.clone() })
        }
        _ => None,
    };
    let cols = vec![Col { path: vec![], ty: root }];
    let rows: Vec<Row> = arms.iter().enumerate().map(|(i, p)| Row { arm: i, cells: vec![p.clone()] }).collect();
    compile(&cols, &rows, enum_defs)
}

/// Walk `decision` down to its terminal (`Leaf`/`Fail`) node using a
/// compile-time-known scrutinee `root` (a `Literal::Record` of `__disc` +
/// payload slots -- the same shape `const_eval::expr::eval_expr` folds enum
/// construction to, and the shape a lowered `Binding::Record`'s VALUES would
/// carry if they were literals). Reads each `Switch`'s `__disc` straight out
/// of the literal instead of a lowered port/gate, so this can run either
/// BEFORE anything is lowered (the const-elision fast path in
/// `lower::expr::lower_match_expr`/`lower::stmt::lower_match_stmt`) or fully
/// outside lowering (`const_eval::expr::eval_expr`'s own `MatchExpr` arm) --
/// shared here so the two callers can never pick a different arm for the same
/// const scrutinee. A path that fails to navigate `root` (should not happen
/// for a genuinely fully-literal scrutinee -- see the callers' own doc
/// comments) degrades to `Decision::Fail`, matching how a non-exhaustive
/// match already behaves rather than guessing a wrong arm.
pub fn resolve_const_leaf(decision: &Decision, root: &Literal) -> Decision {
    match decision {
        Decision::Leaf(_) | Decision::Fail => decision.clone(),
        Decision::Switch { path, cases, default } => match read_disc_literal(root, path) {
            Some(k) => match cases.iter().find(|(case_k, _)| *case_k == k) {
                Some((_, sub)) => resolve_const_leaf(sub, root),
                None => match default {
                    Some(d) => resolve_const_leaf(d, root),
                    None => Decision::Fail,
                },
            },
            None => Decision::Fail,
        },
    }
}

/// Navigate `root` along `path`'s `PathStep::Field`s to the sub-record's
/// `__disc`, the literal-value twin of `lower::expr::read_disc_at_path`.
fn read_disc_literal(root: &Literal, path: &[PathStep]) -> Option<i64> {
    let mut cur = root;
    for PathStep::Field(f) in path {
        let Literal::Record(entries) = cur else { return None };
        cur = &entries.iter().find(|(n, _)| n == f)?.1;
    }
    let Literal::Record(entries) = cur else { return None };
    match entries.iter().find(|(n, _)| n == "__disc")?.1 {
        Literal::Int(v) => Some(v),
        _ => None,
    }
}

/// Navigate a compile-time scrutinee literal along a capture's slot path
/// (`lower::expr::collect_pattern_captures`'s output -- `__{Variant}_{i}` /
/// `__{Variant}_{f}` keys) to the captured value: the literal-value twin of
/// `lower::expr::navigate_capture`, which does the identical walk over a
/// lowered `Binding::Record`. An empty path names the whole scrutinee; a
/// missing or non-record intermediate is `None`.
pub fn navigate_capture_literal(root: &Literal, path: &[String]) -> Option<Literal> {
    let mut cur = root;
    for seg in path {
        let Literal::Record(entries) = cur else { return None };
        cur = &entries.iter().find(|(n, _)| n == seg)?.1;
    }
    Some(cur.clone())
}

/// The head constructor a cell names in its column's enum, or `None` for a
/// wildcard-like cell (opaque column, capture binding, or a stale name).
fn cell_head<'a>(cell: &'a Pattern, ty: Option<&ColEnum>) -> Option<&'a str> {
    ty.and_then(|c| head_variant_name(cell, c.edef))
}

/// The payload-slot columns a variant contributes when its column is switched
/// on: `__{V}_{i}` for positional field `i`, `__{V}_{f}` for named field `f`
/// (the exact keys `build_enum_fields` lays down in `predeclare`). Each new
/// column's path extends the switched column's `parent` path by that step, and
/// each field's type is resolved through the switched column's `parent_ty`
/// instantiation (so a nested generic payload keeps its concrete enum).
fn payload_columns<'a>(
    parent: &[PathStep],
    parent_ty: &ColEnum<'a>,
    v: &VariantDef,
    enum_defs: &'a HashMap<String, EnumDef>,
) -> Vec<Col<'a>> {
    let mut cols = Vec::new();
    let mut push = |slot: String, te| {
        let mut path = parent.to_vec();
        path.push(PathStep::Field(slot));
        cols.push(Col { path, ty: resolve_col_type(te, parent_ty, enum_defs) });
    };
    match &v.payload {
        Payload::Unit => {}
        Payload::Positional(types) => {
            for (i, te) in types.iter().enumerate() {
                push(format!("__{}_{}", v.name, i), te);
            }
        }
        Payload::Named(fields) => {
            for (fname, te) in fields {
                push(format!("__{}_{}", v.name, fname), te);
            }
        }
    }
    cols
}

/// `Specialize(v, matrix)` over the decision-tree representation: the switched
/// column becomes `v`'s payload columns, rows naming a different constructor
/// are dropped, and a matching or wildcard-like row expands its cell to `v`'s
/// sub-patterns (wildcards for a wildcard-like cell).
fn specialize<'a>(
    cols: &[Col<'a>],
    rows: &[Row],
    idx: usize,
    v: &VariantDef,
    enum_defs: &'a HashMap<String, EnumDef>,
) -> (Vec<Col<'a>>, Vec<Row>) {
    let arity = payload_arity(&v.payload);
    let parent_ty = cols[idx].ty.as_ref().expect("a switched column has a governing enum");
    let mut new_cols = Vec::with_capacity(cols.len() + arity);
    new_cols.extend(clone_cols(&cols[..idx]));
    new_cols.extend(payload_columns(&cols[idx].path, parent_ty, v, enum_defs));
    new_cols.extend(clone_cols(&cols[idx + 1..]));

    let mut new_rows = Vec::new();
    for row in rows {
        match cell_head(&row.cells[idx], cols[idx].ty.as_ref()) {
            Some(name) if name != v.name => {}
            _ => {
                let mut cells = row.cells[..idx].to_vec();
                cells.extend(expand_matching(&row.cells[idx], v));
                cells.extend(row.cells[idx + 1..].iter().cloned());
                new_rows.push(Row { arm: row.arm, cells });
            }
        }
    }
    (new_cols, new_rows)
}

/// `Default(matrix)`: drop the switched column, keeping only the rows that do
/// not name a constructor there (they impose no constraint on it).
fn default_matrix<'a>(cols: &[Col<'a>], rows: &[Row], idx: usize) -> (Vec<Col<'a>>, Vec<Row>) {
    let mut new_cols = clone_cols(&cols[..idx]);
    new_cols.extend(clone_cols(&cols[idx + 1..]));
    let new_rows = rows
        .iter()
        .filter(|row| cell_head(&row.cells[idx], cols[idx].ty.as_ref()).is_none())
        .map(|row| {
            let mut cells = row.cells[..idx].to_vec();
            cells.extend(row.cells[idx + 1..].iter().cloned());
            Row { arm: row.arm, cells }
        })
        .collect();
    (new_cols, new_rows)
}

fn clone_cols<'a>(cols: &[Col<'a>]) -> Vec<Col<'a>> {
    cols.iter().map(|c| Col { path: c.path.clone(), ty: c.ty.clone() }).collect()
}

/// The core Maranget recursion. No row means no arm matched (`Fail`); an
/// all-wildcard first row is `Leaf(arm)` (first-match-wins). Otherwise switch
/// the leftmost column the first row constrains: one case per constructor
/// present, plus a `default` (the recursion on the default matrix - which is
/// `Fail` when nothing wildcard-like remains) whenever the cases do not cover
/// every variant.
fn compile(cols: &[Col], rows: &[Row], enum_defs: &HashMap<String, EnumDef>) -> Decision {
    let Some(first) = rows.first() else {
        return Decision::Fail;
    };
    let pick = (0..cols.len()).find(|&i| cell_head(&first.cells[i], cols[i].ty.as_ref()).is_some());
    let Some(idx) = pick else {
        return Decision::Leaf(first.arm);
    };
    let edef = cols[idx].ty.as_ref().expect("a constrained column has a governing enum").edef;

    let mut used: Vec<&str> = Vec::new();
    for row in rows {
        if let Some(name) = cell_head(&row.cells[idx], cols[idx].ty.as_ref()) {
            if !used.contains(&name) {
                used.push(name);
            }
        }
    }

    let mut cases = Vec::new();
    for v in &edef.variants {
        if used.contains(&v.name.as_str()) {
            let (sub_cols, sub_rows) = specialize(cols, rows, idx, v, enum_defs);
            cases.push((v.discriminant, compile(&sub_cols, &sub_rows, enum_defs)));
        }
    }

    let complete = edef.variants.iter().all(|v| used.contains(&v.name.as_str()));
    let default = if complete {
        None
    } else {
        let (def_cols, def_rows) = default_matrix(cols, rows, idx);
        Some(Box::new(compile(&def_cols, &def_rows, enum_defs)))
    };

    Decision::Switch { path: cols[idx].path.clone(), cases, default }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{TypeExpr, TypeParam};
    use crate::diagnostic::SourceRange;
    use crate::parser::parse_pattern_str;
    use crate::typecheck::enums::{Payload, VariantDef};

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

    fn wrap_enum_def() -> EnumDef {
        EnumDef {
            name: "Wrap".to_string(),
            type_params: vec![],
            variants: vec![variant(
                "W",
                0,
                Payload::Named(vec![(
                    "inner".to_string(),
                    TypeExpr::Generic {
                        name: "Option".to_string(),
                        args: vec![type_name("int")],
                        range: dummy_range(),
                    },
                )]),
            )],
        }
    }

    fn shape_enums() -> HashMap<String, EnumDef> {
        let mut m: HashMap<String, EnumDef> = Default::default();
        m.insert("Shape".to_string(), shape_enum_def());
        m
    }

    fn tree_enums() -> HashMap<String, EnumDef> {
        let mut m: HashMap<String, EnumDef> = Default::default();
        m.insert("Tree".to_string(), tree_enum_def());
        m.insert("Option".to_string(), option_enum_def());
        m
    }

    fn wrap_enums() -> HashMap<String, EnumDef> {
        let mut m: HashMap<String, EnumDef> = Default::default();
        m.insert("Wrap".to_string(), wrap_enum_def());
        m.insert("Option".to_string(), option_enum_def());
        m
    }

    fn pats(strs: &[&str]) -> Vec<Pattern> {
        strs.iter().map(|s| parse_pattern_str(s)).collect()
    }

    fn any_switch_path_eq(d: &Decision, want: &[&str]) -> bool {
        match d {
            Decision::Switch { path, cases, default } => {
                let names: Vec<&str> = path.iter().map(|PathStep::Field(f)| f.as_str()).collect();
                names == want
                    || cases.iter().any(|(_, c)| any_switch_path_eq(c, want))
                    || default.as_deref().map_or(false, |c| any_switch_path_eq(c, want))
            }
            _ => false,
        }
    }

    #[test]
    fn flat_match_builds_a_single_switch() {
        let d = build(&shape_enums(), &enum_ty("Shape"), &pats(&["Circle(_)", "Rect(_, _)", "Empty"]));
        let Decision::Switch { path, cases, .. } = d else { panic!("{d:?}") };
        assert!(path.is_empty());
        assert_eq!(cases.len(), 3);
        assert!(cases.iter().all(|(_, c)| matches!(c, Decision::Leaf(_))));
        // Pin each (discriminant -> arm) pair, not merely the count: the case key
        // is the registry DISCRIMINANT (Empty=0, Circle=1, Rect=2), and each maps
        // to the SOURCE-ORDER arm it came from (arm 0 Circle, arm 1 Rect, arm 2
        // Empty). A disc-vs-positional-index regression would still pass a
        // count-only check but fails here.
        assert!(cases.contains(&(0, Decision::Leaf(2))));
        assert!(cases.contains(&(1, Decision::Leaf(0))));
        assert!(cases.contains(&(2, Decision::Leaf(1))));
    }

    #[test]
    fn nested_match_switches_on_sub_disc() {
        let d = build(&tree_enums(), &enum_ty("Tree"), &pats(&["Node(Some(_))", "Node(None)", "Leaf(_)"]));
        fn has_nested_switch(d: &Decision) -> bool {
            match d {
                Decision::Switch { cases, default, .. } => {
                    cases.iter().any(|(_, c)| matches!(c, Decision::Switch { path, .. } if !path.is_empty()))
                        || cases.iter().any(|(_, c)| has_nested_switch(c))
                        || default.as_deref().map_or(false, has_nested_switch)
                }
                _ => false,
            }
        }
        assert!(has_nested_switch(&d));
    }

    #[test]
    fn nonexhaustive_adds_a_fail_default() {
        let d = build(&shape_enums(), &enum_ty("Shape"), &pats(&["Circle(_)"]));
        let Decision::Switch { cases, default, .. } = d else { panic!("{d:?}") };
        assert_eq!(cases.len(), 1);
        assert_eq!(default.as_deref(), Some(&Decision::Fail));
    }

    #[test]
    fn nested_positional_slot_uses_indexed_key() {
        let d = build(&tree_enums(), &enum_ty("Tree"), &pats(&["Node(Some(_))", "Node(None)", "Leaf(_)"]));
        assert!(any_switch_path_eq(&d, &["__Node_0"]));
    }

    #[test]
    fn nested_named_slot_uses_field_key() {
        let d = build(&wrap_enums(), &enum_ty("Wrap"), &pats(&["W { inner: Some(_) }", "W { inner: None }"]));
        assert!(any_switch_path_eq(&d, &["__W_inner"]));
    }

    fn inner_enum_def() -> EnumDef {
        EnumDef {
            name: "Inner".to_string(),
            type_params: vec![],
            variants: vec![variant("On", 0, Payload::Unit), variant("Off", 1, Payload::Unit)],
        }
    }

    fn opt_inner_enums() -> HashMap<String, EnumDef> {
        let mut m: HashMap<String, EnumDef> = Default::default();
        m.insert("Option".to_string(), option_enum_def());
        m.insert("Inner".to_string(), inner_enum_def());
        m
    }

    fn opt_inner_ty() -> Type {
        Type::Enum { name: "Option".to_string(), args: vec![enum_ty("Inner")] }
    }

    #[test]
    fn generic_scrutinee_switches_on_nested_instantiated_enum() {
        // Option<Inner>: `Some(T)`'s payload column must resolve to `Inner`
        // through the instantiation, so the decision tree switches on the
        // nested `__Some_0` disc to tell `Some(On)` from `Some(Off)`. A
        // still-opaque payload would collapse both into one wildcard leaf.
        let d = build(&opt_inner_enums(), &opt_inner_ty(), &pats(&["Some(On)", "Some(Off)", "None"]));
        assert!(any_switch_path_eq(&d, &["__Some_0"]), "{d:?}");
    }
}
