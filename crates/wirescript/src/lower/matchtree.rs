//! Decision-tree builder for `match` lowering (Maranget, "Compiling pattern
//! matching to good decision trees"). Turns the arms' `Pattern`s into a
//! [`Decision`]: a `Switch` tests a `__disc` down a path of payload slots, a
//! `Check` runs one arm when its value tests and guard pass, and the clause
//! matrix keeps first-match-wins. It builds the tree only; it emits no gates.

use crate::ast::{Expr, Pattern};
use crate::collections::HashMap;
use crate::ir::{Literal, Type};
use crate::typecheck::enums::{DISC_FIELD, Payload, VariantDef, slot_name};
// Shared with typecheck's usefulness engine, so the Binding-as-unit-variant
// reading, payload expansion and generic column typing agree with typecheck.
use crate::typecheck::patterns::{
    ColEnum, Enums, expand_matching, head_variant_name, is_value_test, or_alternatives,
    payload_arity, resolve_col_type, resolve_field_type,
};

/// The key a scrutinee that is a single value (not an enum) sits under in the
/// root map the lowering builds, so a test of the whole scrutinee reads it by
/// the same empty path a `Switch` on an enum root uses.
pub(crate) const VALUE_ROOT: &str = "__value";

/// One step of the path from the scrutinee root to a nested payload value:
/// navigate into a payload slot by its interned field name (the same
/// `__disc` / `__{V}_{i}` / `__{V}_{f}` keys `build_enum_fields` lays down in
/// `predeclare`, e.g. `"__Circle_0"`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PathStep {
    Field(String),
}

/// A compiled `match`: run the arm named by [`Decision::Leaf`], reached by
/// testing `__disc` values down a tree of [`Decision::Switch`] nodes and arms'
/// value tests and guards at [`Decision::Check`] nodes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Decision {
    /// The arm (index into the original `arms` slice) whose body runs.
    Leaf(usize),
    /// No arm matched. Only reachable for a non-exhaustive match (already a
    /// `WS054`); lowers to the else/zero path.
    Fail,
    /// Test the `__disc` at `path`: take the case one of whose discriminants
    /// equals it, or `default` when no case matches. Variants that lead to the
    /// same decision share one case, so `A | B => ...` builds its arm once.
    Switch {
        path: Vec<PathStep>,
        cases: Vec<(Vec<i64>, Self)>,
        default: Option<Box<Self>>,
    },
    /// Run `arm` when every value test passes and, when `guarded`, its guard
    /// is true; otherwise go on with `otherwise`, which has every later arm.
    /// Linear on purpose: splitting on a string pattern that may or may not
    /// overlap a later one would copy that later arm into both branches.
    Check {
        tests: Vec<ValueCheck>,
        arm: usize,
        guarded: bool,
        otherwise: Box<Self>,
    },
}

/// One value test of a [`Decision::Check`]: the value at `path`, of type `ty`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ValueCheck {
    pub path: Vec<PathStep>,
    pub ty: Type,
    pub test: ValueTest,
}

/// What a literal, range or string pattern requires of a value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ValueTest {
    /// Equal to this literal, already in the value's type: an `int` literal
    /// tested against a `float` is a float.
    Eq(Literal),
    /// At least the first and below the second.
    Range(Literal, Literal),
    /// A string made of these parts in order.
    Concat(Vec<ConcatPart>),
    /// Any one of these.
    Any(Vec<Self>),
}

/// One part of a string pattern: a literal run, or a hole matching any run of
/// characters. Adjacent literals are merged, so holes sit between literals.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConcatPart {
    Lit(String),
    Hole,
}

/// The test a literal, range or string pattern (or alternatives of only those)
/// makes of a value of type `ty`; `None` for any other pattern.
pub(crate) fn value_test(p: &Pattern, ty: &Type) -> Option<ValueTest> {
    match p {
        Pattern::Literal { expr, .. } => pattern_literal(expr, ty).map(ValueTest::Eq),
        Pattern::Range { lo, hi, .. } => Some(ValueTest::Range(
            pattern_literal(lo, ty)?,
            pattern_literal(hi, ty)?,
        )),
        Pattern::Concat { parts, .. } => Some(ValueTest::Concat(concat_parts(parts))),
        Pattern::Or { alts, .. } => alts
            .iter()
            .map(|a| value_test(a, ty))
            .collect::<Option<Vec<_>>>()
            .map(ValueTest::Any),
        _ => None,
    }
}

/// A literal pattern's value, in the type of the value it is tested against.
fn pattern_literal(e: &Expr, ty: &Type) -> Option<Literal> {
    Some(match e {
        Expr::IntLit { value, .. } if matches!(ty, Type::Float) => Literal::Float(*value as f64),
        Expr::IntLit { value, .. } | Expr::AtomLit { value, .. } => Literal::Int(*value),
        Expr::FloatLit { value, .. } => Literal::Float(*value),
        Expr::StringLit { value, .. } => Literal::String(value.clone()),
        Expr::BoolLit { value, .. } => Literal::Bool(*value),
        _ => return None,
    })
}

/// A string pattern's parts, adjacent literals merged.
pub(crate) fn concat_parts(parts: &[Pattern]) -> Vec<ConcatPart> {
    let mut out: Vec<ConcatPart> = Vec::new();
    for p in parts {
        match p {
            Pattern::Literal { expr, .. } => {
                let Expr::StringLit { value, .. } = &**expr else {
                    out.push(ConcatPart::Hole);
                    continue;
                };
                match out.last_mut() {
                    Some(ConcatPart::Lit(prev)) => prev.push_str(value),
                    _ => out.push(ConcatPart::Lit(value.clone())),
                }
            }
            _ => out.push(ConcatPart::Hole),
        }
    }
    out
}

/// Match `s` against a string pattern's parts, returning what each hole
/// matched. A literal after a hole is found at its first occurrence, except
/// the last literal when nothing follows it, which must end the string; the
/// first literal, when no hole leads, must start it. Characters are counted
/// as code points, as the game's string gates count them.
pub(crate) fn match_concat(parts: &[ConcatPart], s: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut pos = 0;
    let mut holes = Vec::new();
    let mut pending = false;
    for (j, part) in parts.iter().enumerate() {
        match part {
            ConcatPart::Hole => {
                if pending {
                    holes.push(String::new());
                }
                pending = true;
            }
            ConcatPart::Lit(l) => {
                let lit: Vec<char> = l.chars().collect();
                let at = |i: usize| i + lit.len() <= n && chars[i..i + lit.len()] == lit[..];
                if !pending {
                    if !at(pos) {
                        return None;
                    }
                    pos += lit.len();
                } else if concat_lit_ends(parts, j) {
                    let start = n.checked_sub(lit.len())?;
                    if start < pos || !at(start) {
                        return None;
                    }
                    holes.push(chars[pos..start].iter().collect());
                    pos = n;
                } else {
                    let i = (pos..=n.saturating_sub(lit.len())).find(|&i| at(i))?;
                    holes.push(chars[pos..i].iter().collect());
                    pos = i + lit.len();
                }
                pending = false;
            }
        }
    }
    if pending {
        holes.push(chars[pos..].iter().collect());
        pos = n;
    }
    (pos == n).then_some(holes)
}

/// Whether the literal at `j` must end the string: a hole leads it and nothing
/// but literals (already merged into it) follows.
pub(crate) fn concat_lit_ends(parts: &[ConcatPart], j: usize) -> bool {
    !parts[j + 1..].iter().any(|p| matches!(p, ConcatPart::Hole))
}

/// Whether the compile-time value `v` passes `test`; `None` when the two are
/// not comparable (a mistyped program).
pub(crate) fn test_literal(test: &ValueTest, v: &Literal) -> Option<bool> {
    fn num(l: &Literal) -> Option<f64> {
        match l {
            Literal::Int(i) => Some(*i as f64),
            Literal::Float(f) => Some(*f),
            _ => None,
        }
    }
    Some(match test {
        ValueTest::Eq(want) => match (want, v) {
            (Literal::Int(a), Literal::Int(b)) => a == b,
            (Literal::String(a), Literal::String(b)) => a == b,
            (Literal::Bool(a), Literal::Bool(b)) => a == b,
            _ => num(want)? == num(v)?,
        },
        ValueTest::Range(lo, hi) => match (lo, hi, v) {
            (Literal::Int(lo), Literal::Int(hi), Literal::Int(x)) => lo <= x && x < hi,
            _ => {
                let x = num(v)?;
                num(lo)? <= x && x < num(hi)?
            }
        },
        ValueTest::Concat(parts) => {
            let Literal::String(s) = v else { return None };
            match_concat(parts, s).is_some()
        }
        ValueTest::Any(tests) => {
            let mut any = false;
            for t in tests {
                any |= test_literal(t, v)?;
            }
            any
        }
    })
}

/// A compiled `match` arm's name bindings, each read off the scrutinee.
#[derive(Clone, Debug)]
pub(crate) struct Capture {
    pub name: String,
    /// The payload-slot keys from the scrutinee root to the value the name is
    /// bound from (`__{Variant}_{i}` / `__{Variant}_{f}`). Empty for the whole
    /// scrutinee.
    pub path: Vec<String>,
    /// For a name standing for a hole of a string pattern: that pattern's
    /// parts, and which hole (counting holes only) it is.
    pub hole: Option<(Vec<ConcatPart>, usize)>,
}

/// Collect every name `pattern` binds, with where its value comes from.
pub(crate) fn collect_captures(
    pattern: &Pattern,
    prefix: &mut Vec<String>,
    out: &mut Vec<Capture>,
) {
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range { .. } => {}
        // An alternative binds no names (typecheck's WS121).
        Pattern::Or { .. } => {}
        Pattern::Binding { name, .. } => out.push(Capture {
            name: name.clone(),
            path: prefix.clone(),
            hole: None,
        }),
        Pattern::Concat { parts, .. } => {
            let merged = concat_parts(parts);
            let mut hole = 0;
            for p in parts {
                match p {
                    Pattern::Literal { expr, .. } if matches!(**expr, Expr::StringLit { .. }) => {}
                    Pattern::Binding { name, .. } => {
                        out.push(Capture {
                            name: name.clone(),
                            path: prefix.clone(),
                            hole: Some((merged.clone(), hole)),
                        });
                        hole += 1;
                    }
                    _ => hole += 1,
                }
            }
        }
        Pattern::Variant { variant, sub, .. } => match sub {
            crate::ast::VariantPattern::Unit => {}
            crate::ast::VariantPattern::Positional(subs) => {
                for (i, sp) in subs.iter().enumerate() {
                    prefix.push(slot_name(variant, i));
                    collect_captures(sp, prefix, out);
                    prefix.pop();
                }
            }
            crate::ast::VariantPattern::Named { fields, .. } => {
                for (fname, sp) in fields {
                    prefix.push(slot_name(variant, fname));
                    collect_captures(sp, prefix, out);
                    prefix.pop();
                }
            }
        },
    }
}

/// Every name `pattern` binds, in order.
pub(crate) fn capture_names(pattern: &Pattern) -> Vec<String> {
    let mut out = Vec::new();
    collect_captures(pattern, &mut Vec::new(), &mut out);
    out.into_iter().map(|c| c.name).collect()
}

/// One clause-matrix column: the `path` from the scrutinee root to its value,
/// the concrete enum instance governing it (`None` for a non-enum column, which
/// only value tests and catch-alls constrain), and its type. The instance
/// carries the enum's generic `args`, so a type-parameter payload column
/// resolves through the instantiation as `typecheck::patterns::ColEnum` does.
struct Col<'a> {
    path: Vec<PathStep>,
    ty: Option<ColEnum<'a>>,
    raw: Type,
}

/// One clause-matrix row: the original arm index it belongs to, whether that
/// arm has a guard, plus one sub-pattern per column (parallel to the shared
/// column list).
#[derive(Clone)]
struct Row {
    arm: usize,
    guarded: bool,
    cells: Vec<Pattern>,
}

/// Build the decision tree for `arms` matched against `scrutinee`.
pub(crate) fn build(enums: Enums<'_>, scrutinee: &Type, arms: &[Pattern]) -> Decision {
    build_guarded(enums, scrutinee, arms, &[])
}

/// [`build`], where `guarded[i]` says arm `i` has an `if` guard.
pub(crate) fn build_guarded(
    enums: Enums<'_>,
    scrutinee: &Type,
    arms: &[Pattern],
    guarded: &[bool],
) -> Decision {
    let root = match scrutinee {
        Type::Enum { args, .. } => enums.defs.of(scrutinee).map(|edef| ColEnum {
            edef,
            args: args.clone(),
        }),
        _ => None,
    };
    let cols = vec![Col {
        path: vec![],
        ty: root,
        raw: scrutinee.clone(),
    }];
    let rows: Vec<Row> = arms
        .iter()
        .enumerate()
        .map(|(i, p)| Row {
            arm: i,
            guarded: guarded.get(i).copied().unwrap_or(false),
            cells: vec![p.clone()],
        })
        .collect();
    compile(&cols, &rows, enums)
}

/// The terminal (`Leaf`/`Fail`) node `decision` reaches for a compile-time
/// scrutinee `root`: a `Literal::Record` of `__disc` plus payload slots (the
/// shape const-eval folds an enum construction to), or the value itself for a
/// non-enum. Used by match const elision before anything is lowered. A path
/// that does not navigate `root` gives `Fail`. `None` when the arm depends on
/// more than the scrutinee: a guard, or a value test this cannot compare.
pub(crate) fn resolve_const_leaf(decision: &Decision, root: &Literal) -> Option<Decision> {
    match resolve_const(decision, root) {
        ConstOutcome::Arm(i) => Some(Decision::Leaf(i)),
        ConstOutcome::Fail => Some(Decision::Fail),
        ConstOutcome::Guard { .. } | ConstOutcome::Unknown => None,
    }
}

/// Where a compile-time scrutinee leads in a decision tree.
pub(crate) enum ConstOutcome<'d> {
    /// This arm runs.
    Arm(usize),
    /// No arm matches.
    Fail,
    /// `arm` matches if its guard holds; otherwise matching goes on at
    /// `otherwise`.
    Guard { arm: usize, otherwise: &'d Decision },
    /// A value test this cannot decide.
    Unknown,
}

/// Walk `decision` with the compile-time scrutinee `root` as far as the value
/// alone decides: to an arm, to no arm, or to a guard.
pub(crate) fn resolve_const<'d>(decision: &'d Decision, root: &Literal) -> ConstOutcome<'d> {
    match decision {
        Decision::Leaf(i) => ConstOutcome::Arm(*i),
        Decision::Fail => ConstOutcome::Fail,
        Decision::Switch {
            path,
            cases,
            default,
        } => match read_disc_literal(root, path) {
            Some(k) => match cases.iter().find(|(keys, _)| keys.contains(&k)) {
                Some((_, sub)) => resolve_const(sub, root),
                None => match default {
                    Some(d) => resolve_const(d, root),
                    None => ConstOutcome::Fail,
                },
            },
            None => ConstOutcome::Fail,
        },
        Decision::Check {
            tests,
            arm,
            guarded,
            otherwise,
        } => {
            for check in tests {
                let passed = read_value_literal(root, &check.path)
                    .and_then(|v| test_literal(&check.test, &v));
                match passed {
                    Some(true) => {}
                    Some(false) => return resolve_const(otherwise, root),
                    None => return ConstOutcome::Unknown,
                }
            }
            match guarded {
                true => ConstOutcome::Guard {
                    arm: *arm,
                    otherwise,
                },
                false => ConstOutcome::Arm(*arm),
            }
        }
    }
}

/// Navigate `root` along `path`'s `PathStep::Field`s to the sub-record's
/// `__disc`, the literal-value twin of `lower::expr::read_disc_at_path`.
fn read_disc_literal(root: &Literal, path: &[PathStep]) -> Option<i64> {
    let Some(Literal::Record(entries)) = read_value_literal(root, path) else {
        return None;
    };
    match entries.iter().find(|(n, _)| n == DISC_FIELD)?.1 {
        Literal::Int(v) => Some(v),
        _ => None,
    }
}

/// The value at `path` in `root`: `root` itself for the empty path.
fn read_value_literal(root: &Literal, path: &[PathStep]) -> Option<Literal> {
    let mut cur = root;
    for PathStep::Field(f) in path {
        let Literal::Record(entries) = cur else {
            return None;
        };
        cur = &entries.iter().find(|(n, _)| n == f)?.1;
    }
    Some(cur.clone())
}

/// The compile-time value a capture binds out of the scrutinee literal `root`:
/// the literal-value twin of `lower::pattern_test::bind_capture`, which reads the same
/// slot off a lowered `Binding::Record`. A missing or non-record intermediate,
/// or a string pattern that does not match, is `None`.
pub(crate) fn capture_literal(root: &Literal, capture: &Capture) -> Option<Literal> {
    let mut cur = root;
    for seg in &capture.path {
        let Literal::Record(entries) = cur else {
            return None;
        };
        cur = &entries.iter().find(|(n, _)| n == seg)?.1;
    }
    match &capture.hole {
        None => Some(cur.clone()),
        Some((parts, k)) => {
            let Literal::String(s) = cur else { return None };
            match_concat(parts, s)?
                .into_iter()
                .nth(*k)
                .map(Literal::String)
        }
    }
}

/// The head constructor a cell names in its column's enum, or `None` for a
/// wildcard-like cell (opaque column, capture binding, or a stale name).
fn cell_head<'a>(cell: &'a Pattern, ty: Option<&ColEnum<'_>>) -> Option<&'a str> {
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
    enums: Enums<'a>,
) -> Vec<Col<'a>> {
    let mut cols = Vec::new();
    let mut push = |slot: String, te| {
        let mut path = parent.to_vec();
        path.push(PathStep::Field(slot));
        cols.push(Col {
            path,
            ty: resolve_col_type(te, parent_ty, enums),
            raw: resolve_field_type(te, parent_ty, enums),
        });
    };
    match &v.payload {
        Payload::Unit => {}
        Payload::Positional(types) => {
            for (i, te) in types.iter().enumerate() {
                push(slot_name(&v.name, i), te);
            }
        }
        Payload::Named(fields) => {
            for (fname, te) in fields {
                push(slot_name(&v.name, fname), te);
            }
        }
    }
    cols
}

/// `Specialize(v, matrix)` over the decision-tree representation: the switched
/// column becomes `v`'s payload columns, rows naming a different constructor
/// are dropped, and a matching or wildcard-like row expands its cell to `v`'s
/// sub-patterns (wildcards for a wildcard-like cell).
fn specialize_rows<'a>(
    cols: &[Col<'a>],
    rows: &[Row],
    idx: usize,
    v: &VariantDef,
    matching: &[usize],
    wilds: &[usize],
    enums: Enums<'a>,
) -> (Vec<Col<'a>>, Vec<Row>) {
    let arity = payload_arity(&v.payload);
    let parent_ty = cols[idx]
        .ty
        .as_ref()
        .expect("a switched column has a governing enum");
    let mut new_cols = Vec::with_capacity(cols.len() + arity);
    new_cols.extend(clone_cols(&cols[..idx]));
    new_cols.extend(payload_columns(&cols[idx].path, parent_ty, v, enums));
    new_cols.extend(clone_cols(&cols[idx + 1..]));

    // `matching` and `wilds` are ascending row indices; their merge preserves
    // original row order, so first-match-wins is untouched. `expand_matching`
    // gives a wildcard-like row `arity` wildcards, same as a matrix filter.
    let mut new_rows = Vec::with_capacity(matching.len() + wilds.len());
    for r in merge_asc(matching, wilds) {
        let row = &rows[r];
        let mut cells = row.cells[..idx].to_vec();
        cells.extend(expand_matching(&row.cells[idx], v));
        cells.extend(row.cells[idx + 1..].iter().cloned());
        new_rows.push(Row {
            arm: row.arm,
            guarded: row.guarded,
            cells,
        });
    }
    (new_cols, new_rows)
}

fn merge_asc(a: &[usize], b: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] < b[j] {
            out.push(a[i]);
            i += 1;
        } else {
            out.push(b[j]);
            j += 1;
        }
    }
    out.extend_from_slice(&a[i..]);
    out.extend_from_slice(&b[j..]);
    out
}

/// `Default(matrix)`: drop the switched column, keeping only the rows that do
/// not name a constructor there (the precomputed `wilds` index list).
fn default_matrix_rows<'a>(
    cols: &[Col<'a>],
    rows: &[Row],
    idx: usize,
    wilds: &[usize],
) -> (Vec<Col<'a>>, Vec<Row>) {
    let mut new_cols = clone_cols(&cols[..idx]);
    new_cols.extend(clone_cols(&cols[idx + 1..]));
    let new_rows = wilds
        .iter()
        .map(|&r| {
            let row = &rows[r];
            let mut cells = row.cells[..idx].to_vec();
            cells.extend(row.cells[idx + 1..].iter().cloned());
            Row {
                arm: row.arm,
                guarded: row.guarded,
                cells,
            }
        })
        .collect();
    (new_cols, new_rows)
}

fn clone_cols<'a>(cols: &[Col<'a>]) -> Vec<Col<'a>> {
    cols.iter()
        .map(|c| Col {
            path: c.path.clone(),
            ty: c.ty.clone(),
            raw: c.raw.clone(),
        })
        .collect()
}

/// Rows with a `|` in an enum column split into one row per alternative, so
/// each alternative names its constructor to the switch. Alternatives bind no
/// names, so the copies run the same arm with the same captures.
fn expand_enum_ors(cols: &[Col<'_>], rows: &[Row]) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    let mut stack: Vec<Row> = rows.iter().rev().cloned().collect();
    while let Some(row) = stack.pop() {
        let at = (0..cols.len())
            .find(|&i| cols[i].ty.is_some() && matches!(row.cells[i], Pattern::Or { .. }));
        match at {
            None => out.push(row),
            Some(i) => {
                for alt in or_alternatives(&row.cells[i]).into_iter().rev() {
                    let mut r = row.clone();
                    r.cells[i] = alt.clone();
                    stack.push(r);
                }
            }
        }
    }
    out
}

/// The core Maranget recursion. No row gives `Fail`; an all-wildcard first row
/// gives `Leaf(arm)`, or a guarded `Check` falling through to the rest; a first
/// row constraining only values gives a `Check` of its tests. Otherwise it
/// switches the leftmost enum column the first row constrains, one case per
/// constructor present, plus a `default` (the default matrix) when the cases
/// miss a variant or a wildcard-like row remains to catch a stray tag.
fn compile(cols: &[Col<'_>], rows: &[Row], enums: Enums<'_>) -> Decision {
    let expanded;
    let rows = if rows.iter().any(|r| {
        r.cells
            .iter()
            .zip(cols)
            .any(|(c, col)| col.ty.is_some() && matches!(c, Pattern::Or { .. }))
    }) {
        expanded = expand_enum_ors(cols, rows);
        &expanded[..]
    } else {
        rows
    };
    let Some(first) = rows.first() else {
        return Decision::Fail;
    };
    let enum_pick = (0..cols.len()).find(|&i| {
        cols[i].ty.is_some()
            && (cell_head(&first.cells[i], cols[i].ty.as_ref()).is_some()
                || is_value_test(&first.cells[i]))
    });
    let Some(idx) = enum_pick else {
        let tests: Vec<ValueCheck> = (0..cols.len())
            .filter_map(|i| {
                value_test(&first.cells[i], &cols[i].raw).map(|test| ValueCheck {
                    path: cols[i].path.clone(),
                    ty: cols[i].raw.clone(),
                    test,
                })
            })
            .collect();
        if tests.is_empty() && !first.guarded {
            return Decision::Leaf(first.arm);
        }
        return Decision::Check {
            tests,
            arm: first.arm,
            guarded: first.guarded,
            otherwise: Box::new(compile(cols, &rows[1..], enums)),
        };
    };
    let edef = cols[idx]
        .ty
        .as_ref()
        .expect("a constrained column has a governing enum")
        .edef;

    // Group rows by head constructor, read as `head_variant_name` does: a
    // `Variant` names any member, a bare `Binding` only a unit member. A
    // wildcard-like row joins every case and the default. A value test in an
    // enum column (a type error) matches nothing and joins no group.
    let vmap: HashMap<&str, usize> = edef
        .variants
        .iter()
        .enumerate()
        .map(|(i, v)| (v.name.as_str(), i))
        .collect();
    let head_idx = |cell: &Pattern| -> Option<usize> {
        match cell {
            Pattern::Variant { variant, .. } => vmap.get(variant.as_str()).copied(),
            Pattern::Binding { name, .. } => vmap
                .get(name.as_str())
                .copied()
                .filter(|&i| matches!(edef.variants[i].payload, Payload::Unit)),
            _ => None,
        }
    };
    let mut by_variant: Vec<Vec<usize>> = vec![Vec::new(); edef.variants.len()];
    let mut wilds: Vec<usize> = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        match head_idx(&row.cells[idx]) {
            Some(vi) => by_variant[vi].push(r),
            None if is_value_test(&row.cells[idx]) => {}
            None => wilds.push(r),
        }
    }

    let mut cases: Vec<(Vec<i64>, Decision)> = Vec::new();
    for (vi, v) in edef.variants.iter().enumerate() {
        if by_variant[vi].is_empty() {
            continue;
        }
        let (sub_cols, sub_rows) =
            specialize_rows(cols, rows, idx, v, &by_variant[vi], &wilds, enums);
        let sub = compile(&sub_cols, &sub_rows, enums);
        match cases.iter_mut().find(|(_, d)| *d == sub) {
            Some((keys, _)) => keys.push(v.discriminant),
            None => cases.push((vec![v.discriminant], sub)),
        }
    }

    // A wildcard-like row after cases covering every variant still catches a
    // runtime tag that names none of them.
    let complete = by_variant.iter().all(|g| !g.is_empty());
    let default = if complete && wilds.is_empty() {
        None
    } else {
        let (def_cols, def_rows) = default_matrix_rows(cols, rows, idx, &wilds);
        Some(Box::new(compile(&def_cols, &def_rows, enums)))
    };

    Decision::Switch {
        path: cols[idx].path.clone(),
        cases,
        default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{TypeExpr, TypeParam};
    use crate::diagnostic::SourceRange;
    use crate::parser::parse_pattern_str;
    use crate::typecheck::enums::{EnumDef, EnumRegistry, Payload, VariantDef};

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
            Enums {
                defs: &self.defs,
                types: &self.types,
            }
        }
    }

    fn dummy_range() -> SourceRange {
        SourceRange::default()
    }

    fn enum_ty(name: &str) -> Type {
        Type::Enum {
            name: name.to_string(),
            args: vec![],
        }
    }

    fn type_name(name: &str) -> TypeExpr {
        TypeExpr::Name {
            name: name.to_string(),
            range: dummy_range(),
        }
    }

    fn variant(name: &str, disc: i64, payload: Payload) -> VariantDef {
        VariantDef {
            name: name.to_string(),
            discriminant: disc,
            payload,
        }
    }

    fn shape_enum_def() -> EnumDef {
        EnumDef {
            name: "Shape".to_string(),
            type_params: vec![],
            variants: vec![
                variant("Empty", 0, Payload::Unit),
                variant("Circle", 1, Payload::Positional(vec![type_name("float")])),
                variant(
                    "Rect",
                    2,
                    Payload::Positional(vec![type_name("float"), type_name("float")]),
                ),
            ],
        }
    }

    fn option_enum_def() -> EnumDef {
        EnumDef {
            name: "Option".to_string(),
            type_params: vec![TypeParam {
                name: "T".to_string(),
                bound: None,
                range: dummy_range(),
            }],
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

    fn shape_enums() -> EnumRegistry {
        let mut m: EnumRegistry = EnumRegistry::default();
        m.insert(shape_enum_def());
        m
    }

    fn tree_enums() -> EnumRegistry {
        let mut m: EnumRegistry = EnumRegistry::default();
        m.insert(tree_enum_def());
        m.insert(option_enum_def());
        m
    }

    fn wrap_enums() -> EnumRegistry {
        let mut m: EnumRegistry = EnumRegistry::default();
        m.insert(wrap_enum_def());
        m.insert(option_enum_def());
        m
    }

    fn pats(strs: &[&str]) -> Vec<Pattern> {
        strs.iter().map(|s| parse_pattern_str(s)).collect()
    }

    fn any_switch_path_eq(d: &Decision, want: &[&str]) -> bool {
        match d {
            Decision::Switch {
                path,
                cases,
                default,
            } => {
                let names: Vec<&str> = path.iter().map(|PathStep::Field(f)| f.as_str()).collect();
                names == want
                    || cases.iter().any(|(_, c)| any_switch_path_eq(c, want))
                    || default
                        .as_deref()
                        .is_some_and(|c| any_switch_path_eq(c, want))
            }
            _ => false,
        }
    }

    #[test]
    fn flat_match_builds_a_single_switch() {
        let d = build(
            Reg::new(shape_enums()).enums(),
            &enum_ty("Shape"),
            &pats(&["Circle(_)", "Rect(_, _)", "Empty"]),
        );
        let Decision::Switch { path, cases, .. } = d else {
            panic!("{d:?}")
        };
        assert!(path.is_empty());
        assert_eq!(cases.len(), 3);
        assert!(cases.iter().all(|(_, c)| matches!(c, Decision::Leaf(_))));
        // Case keys are registry discriminants (Empty=0, Circle=1, Rect=2), each
        // mapped to its source-order arm, so keying by variant position fails.
        assert!(cases.contains(&(vec![0], Decision::Leaf(2))));
        assert!(cases.contains(&(vec![1], Decision::Leaf(0))));
        assert!(cases.contains(&(vec![2], Decision::Leaf(1))));
    }

    #[test]
    fn nested_match_switches_on_sub_disc() {
        let d = build(
            Reg::new(tree_enums()).enums(),
            &enum_ty("Tree"),
            &pats(&["Node(Some(_))", "Node(None)", "Leaf(_)"]),
        );
        fn has_nested_switch(d: &Decision) -> bool {
            match d {
                Decision::Switch { cases, default, .. } => {
                    cases.iter().any(
                        |(_, c)| matches!(c, Decision::Switch { path, .. } if !path.is_empty()),
                    ) || cases.iter().any(|(_, c)| has_nested_switch(c))
                        || default.as_deref().is_some_and(has_nested_switch)
                }
                _ => false,
            }
        }
        assert!(has_nested_switch(&d));
    }

    #[test]
    fn nonexhaustive_adds_a_fail_default() {
        let d = build(
            Reg::new(shape_enums()).enums(),
            &enum_ty("Shape"),
            &pats(&["Circle(_)"]),
        );
        let Decision::Switch { cases, default, .. } = d else {
            panic!("{d:?}")
        };
        assert_eq!(cases.len(), 1);
        assert_eq!(default.as_deref(), Some(&Decision::Fail));
    }

    #[test]
    fn nested_positional_slot_uses_indexed_key() {
        let d = build(
            Reg::new(tree_enums()).enums(),
            &enum_ty("Tree"),
            &pats(&["Node(Some(_))", "Node(None)", "Leaf(_)"]),
        );
        assert!(any_switch_path_eq(&d, &["__Node_0"]));
    }

    #[test]
    fn nested_named_slot_uses_field_key() {
        let d = build(
            Reg::new(wrap_enums()).enums(),
            &enum_ty("Wrap"),
            &pats(&["W { inner: Some(_) }", "W { inner: None }"]),
        );
        assert!(any_switch_path_eq(&d, &["__W_inner"]));
    }

    fn inner_enum_def() -> EnumDef {
        EnumDef {
            name: "Inner".to_string(),
            type_params: vec![],
            variants: vec![
                variant("On", 0, Payload::Unit),
                variant("Off", 1, Payload::Unit),
            ],
        }
    }

    fn opt_inner_enums() -> EnumRegistry {
        let mut m: EnumRegistry = EnumRegistry::default();
        m.insert(option_enum_def());
        m.insert(inner_enum_def());
        m
    }

    fn opt_inner_ty() -> Type {
        Type::Enum {
            name: "Option".to_string(),
            args: vec![enum_ty("Inner")],
        }
    }

    #[test]
    fn generic_scrutinee_switches_on_nested_instantiated_enum() {
        // Option<Inner>: `Some(T)`'s payload column must resolve to `Inner`
        // through the instantiation, so the decision tree switches on the
        // nested `__Some_0` disc to tell `Some(On)` from `Some(Off)`. A
        // still-opaque payload would collapse both into one wildcard leaf.
        let d = build(
            Reg::new(opt_inner_enums()).enums(),
            &opt_inner_ty(),
            &pats(&["Some(On)", "Some(Off)", "None"]),
        );
        assert!(any_switch_path_eq(&d, &["__Some_0"]), "{d:?}");
    }

    fn parts(p: &str) -> Vec<ConcatPart> {
        let Pattern::Concat { parts, .. } = parse_pattern_str(p) else {
            panic!("{p} is no string pattern")
        };
        concat_parts(&parts)
    }

    #[test]
    fn a_string_pattern_cuts_its_holes_where_its_literals_fall() {
        let holes = |p: &str, s: &str| match_concat(&parts(p), s);
        let v = |xs: &[&str]| {
            Some(
                xs.iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(holes(r#""say " .. rest"#, "say hi"), v(&["hi"]));
        assert_eq!(holes(r#""say " .. rest"#, "sa"), None);
        assert_eq!(holes(r#"name .. ".ws""#, "a.ws.ws"), v(&["a.ws"]));
        assert_eq!(holes(r#"name .. ".ws""#, ".w"), None);
        // A middle literal is found at its first occurrence.
        assert_eq!(holes(r#"k .. "=" .. v"#, "a=b=c"), v(&["a", "b=c"]));
        assert_eq!(holes(r#"k .. "=" .. v"#, "abc"), None);
        // Anchored at both ends, with room for both literals.
        assert_eq!(holes(r#""<" .. x .. ">""#, "<>"), v(&[""]));
        assert_eq!(holes(r#""<" .. x .. ">""#, ">"), None);
        assert_eq!(holes(r#""ab" .. x .. "ba""#, "aba"), None);
        // The last literal must end the string even after a middle one.
        assert_eq!(holes(r#"a .. "-" .. b .. "!""#, "x-y-z!"), v(&["x", "y-z"]));
        // Code points, not bytes.
        assert_eq!(holes(r#""é" .. rest"#, "éa"), v(&["a"]));
        // Adjacent literals merge: no hole between them.
        assert_eq!(holes(r#""a" .. "b" .. rest"#, "abc"), v(&["c"]));
    }

    #[test]
    fn literal_arms_become_a_chain_of_checks() {
        let d = build(
            Reg::new(shape_enums()).enums(),
            &Type::String,
            &pats(&[r#""+""#, r#""-" | "<""#, "_"]),
        );
        let Decision::Check {
            tests,
            arm: 0,
            guarded: false,
            otherwise,
        } = &d
        else {
            panic!("{d:?}")
        };
        assert_eq!(tests[0].test, ValueTest::Eq(Literal::String("+".into())));
        let Decision::Check {
            tests,
            arm: 1,
            otherwise,
            ..
        } = &**otherwise
        else {
            panic!("{d:?}")
        };
        assert!(matches!(tests[0].test, ValueTest::Any(ref alts) if alts.len() == 2));
        assert_eq!(**otherwise, Decision::Leaf(2));
    }

    #[test]
    fn a_guarded_catch_all_falls_through_to_the_next_arm() {
        let d = build_guarded(
            Reg::new(shape_enums()).enums(),
            &Type::Int,
            &pats(&["n", "_"]),
            &[true, false],
        );
        assert_eq!(
            d,
            Decision::Check {
                tests: vec![],
                arm: 0,
                guarded: true,
                otherwise: Box::new(Decision::Leaf(1))
            }
        );
    }

    #[test]
    fn variants_of_one_alternative_share_a_case() {
        let d = build(
            Reg::new(shape_enums()).enums(),
            &enum_ty("Shape"),
            &pats(&["Circle(_) | Rect(_, _)", "Empty"]),
        );
        let Decision::Switch { cases, .. } = d else {
            panic!("{d:?}")
        };
        assert!(
            cases.contains(&(vec![1, 2], Decision::Leaf(0))),
            "{cases:?}"
        );
        assert!(cases.contains(&(vec![0], Decision::Leaf(1))), "{cases:?}");
    }

    #[test]
    fn a_literal_in_a_payload_checks_that_slot() {
        let d = build(
            Reg::new(tree_enums()).enums(),
            &enum_ty("Tree"),
            &pats(&["Leaf(0)", "Leaf(n)", "Node(_)"]),
        );
        let Decision::Switch { cases, .. } = &d else {
            panic!("{d:?}")
        };
        let leaf = cases
            .iter()
            .find(|(k, _)| k == &vec![0])
            .map(|(_, d)| d)
            .expect("Leaf case");
        let Decision::Check {
            tests,
            arm: 0,
            otherwise,
            ..
        } = leaf
        else {
            panic!("{leaf:?}")
        };
        assert_eq!(tests[0].path, vec![PathStep::Field("__Leaf_0".into())]);
        assert_eq!(tests[0].test, ValueTest::Eq(Literal::Int(0)));
        assert_eq!(**otherwise, Decision::Leaf(1));
    }
}
