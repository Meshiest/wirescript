//! Whole-record assignment.
//!
//! A whole-record assignment is a parallel assignment: every source is read
//! before any field is written, in source order, and each container is copied
//! before anything replaces it. [`assign_literal`] writes a literal in the
//! order it has always been written (container elements and nested literals
//! lowered where their field is written) whenever that order reads the same
//! values, and otherwise evaluates first and orders the container writes.

use std::borrow::Cow;

use super::*;
use crate::intern::Sym;
use crate::lower::ordered::{
    Held, Later, Ordered, Reads, State, Writes, binding_reads, copied, hold, live_reads,
    storage_leaves,
};
use crate::lower::stmt::{
    ElemValue, Supplied, assign_record_fields, copy_container, lower_container_assign,
    rebuild_container, same_storage, set_scalar_var,
};

/// Whether evaluating `e` only reads: no call, no container method, nothing
/// that runs. Such a value may be lowered later than its source position, with
/// only its reads to account for.
fn inert(ctx: &LowerCtx, e: &Expr) -> bool {
    match e {
        Expr::IntLit { .. }
        | Expr::AtomLit { .. }
        | Expr::FloatLit { .. }
        | Expr::StringLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::Ident { .. } => true,
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => {
            resolve_field_chain(ctx, e).is_some() || inert(ctx, obj)
        }
        Expr::UnOp { operand, .. } => inert(ctx, operand),
        Expr::Is { value, .. } => inert(ctx, value),
        Expr::BinOp { left, right, .. } => inert(ctx, left) && inert(ctx, right),
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => inert(ctx, cond) && inert(ctx, then_branch) && inert(ctx, else_branch),
        Expr::RecordLit { fields, .. } => inert_fields(ctx, fields),
        Expr::Array { elements, .. } => elements.iter().all(|el| inert(ctx, el.expr())),
        Expr::MapLit { entries, .. } => entries
            .iter()
            .all(|en| inert(ctx, &en.key) && inert(ctx, &en.value)),
        Expr::InterpLit { parts, .. } => parts.iter().all(|p| match p {
            InterpPart::Expr(x) => inert(ctx, x),
            _ => true,
        }),
        _ => false,
    }
}

/// [`inert`] for a record literal's fields; a spread only of a record naming
/// storage, whose fields are known without lowering it.
fn inert_fields(ctx: &LowerCtx, fields: &[RecordLitField]) -> bool {
    fields.iter().all(|f| match f {
        RecordLitField::Named { value, .. } => inert(ctx, value),
        RecordLitField::Shorthand { .. } => true,
        RecordLitField::Spread { value, .. } => {
            matches!(resolve_field_chain(ctx, value), Some(Binding::Record(_)))
        }
    })
}

/// What an [`inert`] expression reads.
fn inert_reads(ctx: &LowerCtx, e: &Expr) -> Reads {
    if let Some(b) = resolve_field_chain(ctx, e) {
        return binding_reads(ctx, b);
    }
    let mut r = Reads::default();
    let mut all = |es: &mut dyn Iterator<Item = &Expr>| es.for_each(|x| r.add(inert_reads(ctx, x)));
    match e {
        Expr::Ident { name, .. } => {
            if let Some(o) = ctx.lookup_output(name) {
                return live_reads(ctx, o.node_id.port(WirePort::RerOutput));
            }
        }
        Expr::FieldAccess { obj, .. }
        | Expr::TuplePick { obj, .. }
        | Expr::UnOp { operand: obj, .. }
        | Expr::Is { value: obj, .. } => all(&mut std::iter::once(obj.as_ref())),
        Expr::BinOp { left, right, .. } => all(&mut [left.as_ref(), right.as_ref()].into_iter()),
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => all(&mut [cond.as_ref(), then_branch.as_ref(), else_branch.as_ref()].into_iter()),
        Expr::RecordLit { fields, .. } => all(&mut fields.iter().filter_map(|f| match f {
            RecordLitField::Named { value, .. } | RecordLitField::Spread { value, .. } => {
                Some(value)
            }
            RecordLitField::Shorthand { .. } => None,
        })),
        Expr::Array { elements, .. } => all(&mut elements.iter().map(|el| el.expr())),
        Expr::MapLit { entries, .. } => {
            all(&mut entries.iter().flat_map(|en| [&en.key, &en.value]))
        }
        Expr::InterpLit { parts, .. } => all(&mut parts.iter().filter_map(|p| match p {
            InterpPart::Expr(x) => Some(x.as_ref()),
            _ => None,
        })),
        _ => {}
    }
    for f in shorthand_names(e) {
        if let Some(b) = ctx.scope.get(f) {
            r.add(binding_reads(ctx, b));
        }
    }
    r
}

fn shorthand_names(e: &Expr) -> impl Iterator<Item = &String> {
    let fields: &[RecordLitField] = match e {
        Expr::RecordLit { fields, .. } => fields,
        _ => &[],
    };
    fields.iter().filter_map(|f| match f {
        RecordLitField::Shorthand { name, .. } => Some(name),
        _ => None,
    })
}

/// Whether a field of `target` given `value` is rebuilt from a literal or a
/// named constant (or, for a plain container, reported as WS027) rather than
/// written from a value.
pub(super) fn rebuilt(ctx: &LowerCtx, target: Option<&Binding>, value: &Expr) -> bool {
    match (target, value) {
        (Some(Binding::Var(v)), _) => matches!(v.storage, VarStorage::Array | VarStorage::Map),
        (
            Some(Binding::Record(_)),
            Expr::Array { .. } | Expr::MapLit { .. } | Expr::RecordLit { .. },
        ) => true,
        (Some(Binding::Record(_)), value) => {
            crate::lower::predeclare::const_container_init(ctx, value).is_some()
        }
        _ => false,
    }
}

/// The name and value of a named or shorthand record-literal field.
pub(super) fn named_field(f: &RecordLitField) -> Option<(&String, Cow<'_, Expr>)> {
    match f {
        RecordLitField::Named { name, value, .. } => Some((name, Cow::Borrowed(value))),
        RecordLitField::Shorthand { name, range } => Some((
            name,
            Cow::Owned(Expr::Ident {
                name: name.clone(),
                range: range.clone(),
            }),
        )),
        RecordLitField::Spread { .. } => None,
    }
}

/// A record literal being assigned, every source planned.
struct Lit<'e> {
    target: HashMap<Sym, Binding>,
    path: String,
    /// Every source in source order, including the ones a later source
    /// overrides.
    parts: Vec<Part<'e>>,
    /// Which part supplies each field last.
    latest: HashMap<Sym, usize>,
    /// The field sources wait for the field this literal is nested in: a
    /// nested literal that runs nothing is lowered where it is written.
    deferred: bool,
}

enum Part<'e> {
    /// A field value or spread, in a slot holding the record of what it
    /// supplies, and the watch on each container it copies.
    Src {
        slot: usize,
        keys: Vec<Sym>,
        watches: Vec<(Sym, usize)>,
    },
    /// A container field rebuilt from a literal: per element the slots of its
    /// values, in the order the rebuild lowers them.
    Rebuild {
        target: Binding,
        path: String,
        value: Cow<'e, Expr>,
        elems: Vec<Vec<usize>>,
    },
    Nested(Lit<'e>),
}

/// Plan the literal `fields` assigned to `target`: every source that runs
/// code is lowered now, in source order, and an [`inert`] one waits.
#[allow(clippy::too_many_arguments)]
fn plan<'e>(
    ctx: &mut LowerCtx,
    ord: &mut Ordered<'e>,
    target: HashMap<Sym, Binding>,
    path: String,
    fields: &'e [RecordLitField],
    slots: Slots,
    deferred: bool,
    range: &SourceRange,
) -> Lit<'e> {
    let mut lit = Lit {
        target,
        path,
        parts: Vec::new(),
        latest: HashMap::default(),
        deferred,
    };
    for (i, f) in fields.iter().enumerate() {
        let key = named_field(f).map(|(name, _)| crate::intern::intern(name));
        let field_slots = named_field(f).map_or(Slots::Unknown, |(name, _)| slots.field(name));
        let t = key.and_then(|k| lit.target.get(&k)).cloned();
        let part = match (named_field(f), t) {
            (Some((name, value)), Some(t)) if rebuilt(ctx, Some(&t), &value) => {
                let field_path = format!("{}.{name}", lit.path);
                match (t, f) {
                    (
                        Binding::Record(sub),
                        RecordLitField::Named {
                            name,
                            value: Expr::RecordLit { fields: inner, .. },
                            ..
                        },
                    ) => {
                        let d = deferred || inert_fields(ctx, inner);
                        let slot = slots.of(name);
                        let inner_slots = slot.as_ref().map_or(Slots::Unknown, Slots::Of);
                        Part::Nested(plan(
                            ctx,
                            ord,
                            sub,
                            field_path,
                            inner,
                            inner_slots,
                            d,
                            range,
                        ))
                    }
                    (t, RecordLitField::Named { value, .. }) => {
                        plan_rebuild(ctx, ord, t, field_path, value, deferred, range)
                    }
                    // A shorthand names storage, never a literal: WS027.
                    (t, _) => Part::Rebuild {
                        target: t,
                        path: field_path,
                        value,
                        elems: Vec::new(),
                    },
                }
            }
            _ => {
                let t = Binding::Record(lit.target.clone());
                let mut rec = None;
                let lowered = (!deferred).then(|| {
                    // Fields a later source supplies again are never read, so
                    // never held.
                    let later = crate::lower::decl::supplied_by(ctx, &fields[i + 1..]);
                    let target = &lit.target;
                    ord.source(ctx, &lit.path, Some(&t), range, |ctx| {
                        let full = match f {
                            RecordLitField::Spread { value, .. } => {
                                branch_record_fields(ctx, value).unwrap_or_default()
                            }
                            _ => crate::lower::decl::lower_record_lit(
                                ctx,
                                std::slice::from_ref(f),
                                field_slots,
                            ),
                        };
                        let mut read = full.clone();
                        read.retain(|k, b| {
                            !later.contains(k) && !target.get(k).is_some_and(|t| same_storage(t, b))
                        });
                        rec = Some(full);
                        Some(Binding::Record(read))
                    })
                });
                // What a waiting source names, known without lowering it.
                let named = match &rec {
                    Some(rec) => Some(rec.clone()),
                    None => Later::Field(f, field_slots).named(ctx),
                };
                let keys = match (f, &named) {
                    (RecordLitField::Spread { .. }, Some(r)) => r.keys().copied().collect(),
                    _ => key.into_iter().collect(),
                };
                let mut named = named.unwrap_or_default();
                named.retain(|k, b| !lit.target.get(k).is_some_and(|t| same_storage(t, b)));
                let watches = named
                    .iter()
                    .filter_map(|(k, b)| {
                        let path = format!("{}.{}", lit.path, crate::intern::resolve(*k));
                        Some((*k, ord.watch(path, copied(b), range)?))
                    })
                    .collect();
                let slot = match lowered.flatten() {
                    Some(slot) => slot,
                    None => {
                        let reads = match f {
                            RecordLitField::Named { value, .. }
                                if resolve_field_chain(ctx, value).is_none() =>
                            {
                                inert_reads(ctx, value)
                            }
                            _ => binding_reads(ctx, &Binding::Record(named)),
                        };
                        let later = State::Later(Later::Field(f, field_slots));
                        ord.add(later, lit.path.clone(), Some(t), range, Some(reads))
                    }
                };
                Part::Src {
                    slot,
                    keys,
                    watches,
                }
            }
        };
        let supplied: Vec<Sym> = match &part {
            Part::Src { keys, .. } => keys.clone(),
            _ => key.into_iter().collect(),
        };
        for k in supplied {
            if let Some(&prev) = lit.latest.get(&k)
                && let Part::Src { slot, watches, .. } = &lit.parts[prev]
            {
                ord.drop_key(*slot, k);
                watches
                    .iter()
                    .filter(|(w, _)| *w == k)
                    .for_each(|(_, w)| ord.forget(*w));
            }
            lit.latest.insert(k, lit.parts.len());
        }
        lit.parts.push(part);
    }
    lit
}

/// Plan a container field rebuilt from the literal `value`: an [`inert`]
/// element waits for the rebuild, any other is lowered now.
fn plan_rebuild<'e>(
    ctx: &mut LowerCtx,
    ord: &mut Ordered<'e>,
    target: Binding,
    path: String,
    value: &'e Expr,
    deferred: bool,
    range: &SourceRange,
) -> Part<'e> {
    let columns = matches!(target, Binding::Record(_));
    let literal = literal_for(&target, value);
    let mut own = HashSet::default();
    storage_leaves(&target, &mut own);
    let mut elems = Vec::new();
    let parts: Vec<Vec<Later<'e>>> = match value {
        Expr::Array { elements, .. } if literal => elements
            .iter()
            .map(|el| match el {
                ArrayElem::Item(e) => vec![Later::Elem(e, columns)],
                ArrayElem::Spread(e) => {
                    if let Some(b) = resolve_field_chain(ctx, e) {
                        let mut copies = copied(b);
                        copies
                            .containers
                            .retain(|(n, _)| !n.is_some_and(|n| own.contains(&n)));
                        ord.watch(path.clone(), copies, range);
                    }
                    Vec::new()
                }
            })
            .collect(),
        Expr::MapLit { entries, .. } if literal && columns => entries
            .iter()
            .map(|en| vec![Later::Elem(&en.value, true), Later::Key(&en.key)])
            .collect(),
        Expr::MapLit { entries, .. } if literal => entries
            .iter()
            .map(|en| vec![Later::Key(&en.key), Later::Elem(&en.value, false)])
            .collect(),
        _ => Vec::new(),
    };
    for values in parts {
        let wait = deferred
            || values.iter().all(|l| match l {
                Later::Elem(e, _) | Later::Key(e) => inert(ctx, e),
                Later::Field(..) => false,
            });
        let mut slots = Vec::new();
        for l in values {
            let t = match l {
                Later::Key(_) => None,
                _ => Some(target.clone()),
            };
            if wait {
                let reads = match l {
                    Later::Elem(e, _) | Later::Key(e) => inert_reads(ctx, e),
                    Later::Field(..) => Reads::default(),
                };
                slots.push(ord.add(State::Later(l), path.clone(), t, range, Some(reads)));
            } else {
                match ord.source(ctx, &path, t.as_ref(), range, |ctx| l.lower(ctx)) {
                    Some(slot) => slots.push(slot),
                    None => {
                        slots.push(ord.add(State::Missing, path.clone(), None, range, None));
                        break;
                    }
                }
            }
        }
        elems.push(slots);
    }
    Part::Rebuild {
        target,
        path,
        value: Cow::Borrowed(value),
        elems,
    }
}

impl Lit<'_> {
    /// The fields' sources, each from the last source supplying it, less any
    /// that would copy a field onto its own storage. `get` gives a slot's
    /// record.
    fn sources(&self, get: &mut dyn FnMut(usize) -> Option<Binding>) -> HashMap<Sym, Binding> {
        let mut src = HashMap::default();
        for (k, &p) in &self.latest {
            if let Part::Src { slot, .. } = &self.parts[p]
                && let Some(Binding::Record(rec)) = get(*slot)
                && let Some(b) = rec.get(k)
                && !self.target.get(k).is_some_and(|t| same_storage(t, b))
            {
                src.insert(*k, b.clone());
            }
        }
        src
    }

    /// The rebuilds and nested literals that supply a field last, in source
    /// order.
    fn rebuilds(&self) -> impl Iterator<Item = &Part<'_>> {
        self.parts
            .iter()
            .enumerate()
            .filter(|(i, p)| !matches!(p, Part::Src { .. }) && self.latest.values().any(|v| v == i))
            .map(|(_, p)| p)
    }
}

/// One write of a record assignment.
enum Write<'a> {
    Set {
        target: VarRecord,
        value: Binding,
    },
    Copy {
        target: VarRecord,
        source: Binding,
        path: String,
    },
    Rebuild {
        target: &'a Binding,
        path: &'a str,
        value: &'a Expr,
        elems: &'a [Vec<usize>],
    },
}

/// The writes of `lit`, in the order it is written: rebuilds in source order
/// (a nested literal's writes in its place), then the fields.
fn flat<'a>(
    lit: &'a Lit<'a>,
    get: &mut dyn FnMut(usize) -> Option<Binding>,
    out: &mut Vec<Write<'a>>,
) {
    for p in lit.rebuilds() {
        match p {
            Part::Rebuild {
                target,
                path,
                value,
                elems,
            } => out.push(Write::Rebuild {
                target,
                path,
                value,
                elems,
            }),
            Part::Nested(sub) => flat(sub, get, out),
            Part::Src { .. } => {}
        }
    }
    field_writes(&lit.target, &lit.sources(get), &lit.path, out);
}

/// The writes that put `src` into `target`, one per leaf, in `target`'s
/// iteration order: a scalar leaf is set and a container leaf copied.
fn field_writes(
    target: &HashMap<Sym, Binding>,
    src: &HashMap<Sym, Binding>,
    path: &str,
    out: &mut Vec<Write<'_>>,
) {
    for (fname, tbind) in crate::lower::stmt::in_field_order(target) {
        let Some(sbind) = src.get(&fname) else {
            continue;
        };
        let field_path = format!("{path}.{}", crate::intern::resolve(fname));
        match tbind {
            Binding::Record(tf) => {
                if let Binding::Record(sf) = sbind {
                    field_writes(tf, sf, &field_path, out);
                }
            }
            Binding::Var(v) if v.storage == VarStorage::Var => out.push(Write::Set {
                target: v.clone(),
                value: sbind.clone(),
            }),
            Binding::Var(v) => out.push(Write::Copy {
                target: v.clone(),
                source: sbind.clone(),
                path: field_path,
            }),
            _ => {}
        }
    }
}

/// The containers `w` replaces, and the containers it reads when it runs (a
/// copy's source, or a rebuild's spreads of other containers).
fn container_io(ctx: &LowerCtx, w: &Write) -> (HashSet<NodeId>, HashSet<NodeId>) {
    let (mut dsts, mut reads) = (HashSet::default(), HashSet::default());
    match w {
        Write::Set { .. } => {}
        Write::Copy { target, source, .. } => {
            dsts.insert(target.node_id);
            storage_leaves(source, &mut reads);
        }
        Write::Rebuild { target, value, .. } => {
            storage_leaves(target, &mut dsts);
            if let Expr::Array { elements, .. } = value {
                for el in elements {
                    if let ArrayElem::Spread(e) = el
                        && let Some(b) = resolve_field_chain(ctx, e)
                    {
                        storage_leaves(b, &mut reads);
                    }
                }
            }
            reads.retain(|n| !dsts.contains(n));
        }
    }
    (dsts, reads)
}

/// The order to run `writes` in: each container copied or spread before any
/// write replaces it, and otherwise as listed. Containers that read each
/// other in a cycle have no such order; those come last, and are the second
/// list.
fn write_order(ctx: &LowerCtx, writes: &[Write]) -> (Vec<usize>, Vec<usize>) {
    let io: Vec<(HashSet<NodeId>, HashSet<NodeId>)> =
        writes.iter().map(|w| container_io(ctx, w)).collect();
    // `before[j]`: every other write that reads a container write `j` replaces.
    let before: Vec<Vec<usize>> = (0..writes.len())
        .map(|j| {
            (0..writes.len())
                .filter(|&i| i != j && !io[i].1.is_disjoint(&io[j].0))
                .collect()
        })
        .collect();
    let mut order = Vec::with_capacity(writes.len());
    let mut done = vec![false; writes.len()];
    while order.len() < writes.len() {
        let Some(next) =
            (0..writes.len()).find(|&j| !done[j] && before[j].iter().all(|&i| done[i]))
        else {
            let stuck: Vec<usize> = (0..writes.len()).filter(|&j| !done[j]).collect();
            order.extend(&stuck);
            return (order, stuck);
        };
        done[next] = true;
        order.push(next);
    }
    (order, Vec::new())
}

/// Run `writes` in `order`.
fn run_writes(
    ctx: &mut LowerCtx,
    ord: &mut Ordered,
    writes: Vec<Write>,
    order: &[usize],
    range: &SourceRange,
) {
    let mut writes: Vec<Option<Write>> = writes.into_iter().map(Some).collect();
    for &i in order {
        match writes[i].take() {
            Some(Write::Set { target, value }) => {
                if let Some(port) = binding_to_port(ctx, &value, range) {
                    set_scalar_var(ctx, &target, port, range);
                }
            }
            Some(Write::Copy { target, source, .. }) => {
                copy_container(ctx, &target, &source, range)
            }
            Some(Write::Rebuild {
                target,
                path,
                value,
                elems,
            }) => rebuild(ctx, ord, target, path, value, elems, range),
            None => {}
        }
    }
}

/// Rebuild the container `target` from the literal `value`, each element from
/// its slots.
fn rebuild(
    ctx: &mut LowerCtx,
    ord: &mut Ordered,
    target: &Binding,
    path: &str,
    value: &Expr,
    elems: &[Vec<usize>],
    range: &SourceRange,
) {
    let columns = matches!(target, Binding::Record(_));
    if !literal_for(target, value)
        && let Some(lit) = crate::lower::predeclare::const_container_init(ctx, value)
    {
        crate::lower::stmt::rebuild_from_constant(ctx, target, path, value, lit, range);
        return;
    }
    if let Binding::Var(v) = target
        && !literal_for(target, value)
    {
        lower_container_assign(ctx, v, path, value, range);
        return;
    }
    let is_map = matches!(value, Expr::MapLit { .. });
    rebuild_container(ctx, target, path, value, range, &mut |ctx, i| {
        let slots = &elems[i];
        if slots.is_empty() || (is_map && columns && ctx.current_exec.is_none()) {
            return Supplied::Skip;
        }
        let port = |ctx: &mut LowerCtx, b: Option<Binding>| {
            b.and_then(|b| binding_to_port(ctx, &b, range))
        };
        let first = ord.value(ctx, slots[0]);
        match (is_map, columns, first) {
            (false, true, Some(Binding::Record(r))) => Supplied::Item(ElemValue::Record(r)),
            (false, false, b) => {
                port(ctx, b).map_or(Supplied::Missing, |p| Supplied::Item(ElemValue::Scalar(p)))
            }
            (true, true, Some(Binding::Record(r))) => {
                let key = ord.value(ctx, slots[1]);
                port(ctx, key).map_or(Supplied::Missing, |k| {
                    Supplied::Entry(k, ElemValue::Record(r))
                })
            }
            (true, false, key) => {
                let value = ord.value(ctx, slots[1]);
                match (port(ctx, key), port(ctx, value)) {
                    (Some(k), Some(v)) => Supplied::Entry(k, ElemValue::Scalar(v)),
                    _ => Supplied::Missing,
                }
            }
            _ => Supplied::Missing,
        }
    });
}

/// Whether `value` is a literal of the container `target` holds.
fn literal_for(target: &Binding, value: &Expr) -> bool {
    match (target, value) {
        (Binding::Var(v), Expr::Array { .. }) => v.storage == VarStorage::Array,
        (Binding::Var(v), Expr::MapLit { .. }) => v.storage == VarStorage::Map,
        (Binding::Record(_), Expr::Array { .. } | Expr::MapLit { .. }) => true,
        _ => false,
    }
}

/// Write `lit` as it lowers: a waiting literal's field sources where it is
/// written, then its rebuilds in source order, their waiting elements lowered
/// in place, then its fields.
fn emit_in_place(ctx: &mut LowerCtx, ord: &mut Ordered, lit: &Lit, range: &SourceRange) {
    if lit.deferred {
        for p in &lit.parts {
            if let Part::Src { slot, .. } = p {
                ord.value(ctx, *slot);
            }
        }
    }
    for p in lit.rebuilds() {
        match p {
            Part::Rebuild {
                target,
                path,
                value,
                elems,
            } => rebuild(ctx, ord, target, path, value, elems, range),
            Part::Nested(sub) => emit_in_place(ctx, ord, sub, range),
            Part::Src { .. } => {}
        }
    }
    let src = lit.sources(&mut |s| ord.value(ctx, s));
    assign_record_fields(ctx, &lit.target, &src, range);
}

/// WS087: the writes left undone each read a container another one replaces.
fn report_cycle(ctx: &mut LowerCtx, writes: &[Write], stuck: &[usize], range: &SourceRange) {
    let mut paths: Vec<String> = stuck
        .iter()
        .filter_map(|&i| match &writes[i] {
            Write::Copy { path, .. } => Some(format!("`{path}`")),
            Write::Rebuild { path, .. } => Some(format!("`{path}`")),
            Write::Set { .. } => None,
        })
        .collect();
    paths.sort();
    paths.dedup();
    ctx.error(
        "WS087",
        format!(
            "this assignment copies {} each from another of them. A container is copied when \
             its write runs rather than read up front, so one of them would copy contents \
             already overwritten. Copy one side into a separate variable first and assign \
             from that",
            paths.join(", ")
        ),
        range,
    );
}

/// The storage `lit` writes: every rebuilt container, and every field a
/// source supplies other than one naming the field's own storage. `named`
/// gives a slot's record as far as it names storage.
fn lit_written(
    ctx: &LowerCtx,
    lit: &Lit,
    named: &mut dyn FnMut(usize) -> Option<Binding>,
    w: &mut Writes,
) {
    for p in lit.rebuilds() {
        match p {
            Part::Rebuild { target, .. } => w.add(ctx, target),
            Part::Nested(sub) => lit_written(ctx, sub, named, w),
            Part::Src { .. } => {}
        }
    }
    for (k, &p) in &lit.latest {
        if let (Part::Src { slot, .. }, Some(t)) = (&lit.parts[p], lit.target.get(k)) {
            let own = matches!(named(*slot), Some(Binding::Record(r)) if r.get(k).is_some_and(|b| same_storage(t, b)));
            if !own {
                w.add(ctx, t);
            }
        }
    }
}

/// The storage `writes` write.
fn written(ctx: &LowerCtx, writes: &[Write]) -> Writes {
    let mut w = Writes::default();
    for write in writes {
        match write {
            Write::Set { target, .. } | Write::Copy { target, .. } => {
                w.add(ctx, &Binding::Var(target.clone()))
            }
            Write::Rebuild { target, .. } => w.add(ctx, target),
        }
    }
    w
}

/// `h = { ...h2, xs: [..], pts: [..], m: { .. }, n: 2 }`: each field takes its
/// value from the last source naming it (a spread supplies every field of its
/// record), every source is read before anything is written, in source order,
/// and each container is copied before anything replaces it.
///
/// Each source that runs code is lowered in source order. An [`inert`]
/// container element, and a nested literal that is inert throughout, is
/// lowered where its field is written, as long as nothing written earlier (a
/// later source's call, or this assignment) changes what it reads; otherwise
/// it is read first. A field naming storage the assignment writes is read
/// before the first write. When a container would be copied after something
/// replaced it, every source is read first and the container writes are
/// ordered; a cycle is WS087.
pub(super) fn assign_literal(
    ctx: &mut LowerCtx,
    target: &HashMap<Sym, Binding>,
    path: &str,
    fields: &[RecordLitField],
    slots: Slots,
    range: &SourceRange,
) {
    let mut ord = Ordered::default();
    let lit = plan(
        ctx,
        &mut ord,
        target.clone(),
        path.to_string(),
        fields,
        slots,
        false,
        range,
    );
    let (changed, reorder) = {
        let mut named = |s: usize| match &ord.slots[s].state {
            State::Live(b) | State::Fixed(b) => Some(b.clone()),
            State::Later(l) => l.named(ctx).map(Binding::Record),
            _ => None,
        };
        let mut writes = Vec::new();
        flat(&lit, &mut named, &mut writes);
        let (order, stuck) = write_order(ctx, &writes);
        let mut changed = Writes::default();
        lit_written(ctx, &lit, &mut named, &mut changed);
        (
            changed,
            !stuck.is_empty() || order.iter().enumerate().any(|(i, j)| i != *j),
        )
    };
    ord.fix_changed(ctx, &changed);
    if !reorder {
        emit_in_place(ctx, &mut ord, &lit, range);
        return;
    }
    let mut held = Held::default();
    for i in 0..ord.slots.len() {
        if matches!(ord.slots[i].state, State::Later(_)) {
            ord.fix(ctx, i, &mut held);
        }
    }
    let slots: Vec<Option<Binding>> = (0..ord.slots.len()).map(|s| ord.value(ctx, s)).collect();
    let mut writes = Vec::new();
    flat(&lit, &mut |s| slots[s].clone(), &mut writes);
    order_and_run(ctx, &mut ord, writes, range);
}

/// Run `writes` in [`write_order`], reporting a cycle.
fn order_and_run(ctx: &mut LowerCtx, ord: &mut Ordered, writes: Vec<Write>, range: &SourceRange) {
    let (order, stuck) = write_order(ctx, &writes);
    if !stuck.is_empty() {
        report_cycle(ctx, &writes, &stuck, range);
    }
    run_writes(ctx, ord, writes, &order, range);
}

/// Write the lowered record value `src` into `target`, each field that reads
/// storage the assignment writes read first, and each container copied before
/// anything replaces it.
pub(super) fn assign_value(
    ctx: &mut LowerCtx,
    target: &HashMap<Sym, Binding>,
    path: &str,
    mut src: HashMap<Sym, Binding>,
    range: &SourceRange,
) {
    let mut writes = Vec::new();
    field_writes(target, &src, path, &mut writes);
    let w = written(ctx, &writes);
    let mut keys: Vec<Sym> = src.keys().copied().collect();
    keys.sort_by_key(|k| crate::intern::resolve(*k));
    let mut held = Held::default();
    let mut changed = false;
    for k in keys {
        if w.changes(&binding_reads(ctx, &src[&k])) {
            let label = format!("{path}.{}", crate::intern::resolve(k));
            let now = hold(ctx, &src[&k], target.get(&k), &label, range, &mut held);
            src.insert(k, now);
            changed = true;
        }
    }
    if changed {
        writes.clear();
        field_writes(target, &src, path, &mut writes);
    }
    order_and_run(ctx, &mut Ordered::default(), writes, range);
}
