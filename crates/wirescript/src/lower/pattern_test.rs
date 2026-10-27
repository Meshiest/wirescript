//! The gates behind a `match` arm's value tests (literals, ranges, string
//! patterns) and the names a string pattern binds.

use super::*;
use crate::lower::matchtree::{
    Capture, ConcatPart, PathStep, VALUE_ROOT, ValueCheck, ValueTest, concat_lit_ends,
};

/// One string pattern lowered against one value: whether it matched, and what
/// each hole holds when it did.
#[derive(Clone, Debug)]
pub(super) struct ConcatLowered {
    value: PortRef,
    parts: Vec<ConcatPart>,
    cond: PortRef,
    holes: Vec<PortRef>,
}

/// The value a `match` tests: an enum's `__disc` + payload-slot record, or for
/// a scrutinee that is a single value, that value under [`VALUE_ROOT`].
pub(super) fn match_root(
    ctx: &mut LowerCtx<'_>,
    scrutinee: &Expr,
    scrut_ty: &Type,
) -> Option<Fields> {
    match scrut_ty {
        Type::Int | Type::Float | Type::Bool | Type::String => {
            let port = lower_expr(ctx, scrutinee);
            let mut root = HashMap::default();
            root.insert(intern(VALUE_ROOT), Binding::Local(LocalRecord { port }));
            Some(root)
        }
        _ => match_scrutinee_record(ctx, scrutinee),
    }
}

/// The value at `path` in the match root, as a port.
pub(super) fn read_value_at_path(
    ctx: &mut LowerCtx<'_>,
    root: &Fields,
    path: &[PathStep],
    range: &SourceRange,
) -> Option<PortRef> {
    let Some((last, init)) = path.split_last() else {
        let value = root.get(&intern(VALUE_ROOT))?.clone();
        return binding_to_port(ctx, &value, range);
    };
    let mut cur = root;
    for PathStep::Field(f) in init {
        match cur.get(&intern(f)) {
            Some(Binding::Record(sub)) => cur = sub,
            _ => return None,
        }
    }
    let PathStep::Field(f) = last;
    let value = cur.get(&intern(f))?.clone();
    binding_to_port(ctx, &value, range)
}

/// The condition that every one of `tests` passes, or `None` for no tests.
pub(super) fn lower_value_checks(
    ctx: &mut LowerCtx<'_>,
    root: &Fields,
    tests: &[ValueCheck],
    range: &SourceRange,
) -> Option<PortRef> {
    let mut conds = Vec::with_capacity(tests.len());
    for check in tests {
        let value = read_value_at_path(ctx, root, &check.path, range)
            .unwrap_or_else(|| synthesise_unsupported_range(ctx, "a matched value", range));
        conds.push(lower_value_test(ctx, value, &check.ty, &check.test, range));
    }
    all_of(ctx, conds, range)
}

/// `a && b && ...`, or `None` for no conditions.
pub(super) fn all_of(
    ctx: &mut LowerCtx<'_>,
    conds: Vec<PortRef>,
    range: &SourceRange,
) -> Option<PortRef> {
    conds
        .into_iter()
        .reduce(|a, b| emit_op(ctx, "&&", (a, Type::Bool), (b, Type::Bool), range))
}

/// Whether `value`, of type `ty`, passes `test`.
fn lower_value_test(
    ctx: &mut LowerCtx<'_>,
    value: PortRef,
    ty: &Type,
    test: &ValueTest,
    range: &SourceRange,
) -> PortRef {
    match test {
        ValueTest::Eq(lit) => {
            let lit = literal_node_range(ctx, range, ty.clone(), lit.clone());
            emit_op(ctx, "==", (value, ty.clone()), (lit, ty.clone()), range)
        }
        ValueTest::Range(lo, hi) => {
            let lo = literal_node_range(ctx, range, ty.clone(), lo.clone());
            let hi = literal_node_range(ctx, range, ty.clone(), hi.clone());
            let above = emit_op(ctx, ">=", (value, ty.clone()), (lo, ty.clone()), range);
            let below = emit_op(ctx, "<", (value, ty.clone()), (hi, ty.clone()), range);
            emit_op(ctx, "&&", (above, Type::Bool), (below, Type::Bool), range)
        }
        ValueTest::Concat(parts) => lower_concat(ctx, value, parts, range).cond,
        ValueTest::Any(tests) => {
            let conds: Vec<PortRef> = tests
                .iter()
                .map(|t| lower_value_test(ctx, value, ty, t, range))
                .collect();
            conds
                .into_iter()
                .reduce(|a, b| emit_op(ctx, "||", (a, Type::Bool), (b, Type::Bool), range))
                .unwrap_or_else(|| literal_node_range(ctx, range, Type::Bool, Literal::Bool(false)))
        }
    }
}

/// The value a capture binds, read off the match root: a payload slot (or the
/// whole scrutinee), or a hole of a string pattern.
pub(super) fn bind_capture(
    ctx: &mut LowerCtx<'_>,
    root: &Fields,
    capture: &Capture,
    range: &SourceRange,
) -> Option<Binding> {
    let binding = navigate_capture(root, &capture.path)?;
    let Some((parts, k)) = &capture.hole else {
        return Some(binding);
    };
    let value = binding_to_port(ctx, &binding, range)?;
    let port = *lower_concat(ctx, value, parts, range).holes.get(*k)?;
    Some(Binding::Local(LocalRecord { port }))
}

/// A string pattern against `value`, mirroring `matchtree::match_concat`: a
/// literal no hole leads must start the string (`StartsWith`), the last
/// literal a hole leads with nothing after it must end it (`EndsWith`), and any
/// other literal is found at its first occurrence after the one before it
/// (`Find`). Each hole is the `Substring` between its neighbours. The string
/// gates are case-sensitive, as `==` is.
fn lower_concat(
    ctx: &mut LowerCtx<'_>,
    value: PortRef,
    parts: &[ConcatPart],
    range: &SourceRange,
) -> ConcatLowered {
    if let Some(hit) = ctx
        .concat_cache
        .iter()
        .find(|c| c.value == value && c.parts == parts)
    {
        return hit.clone();
    }
    let mut conds = Vec::new();
    let mut holes = Vec::new();
    if !parts.iter().any(|p| matches!(p, ConcatPart::Hole)) {
        let text: String = parts
            .iter()
            .map(|p| match p {
                ConcatPart::Lit(l) => l.as_str(),
                ConcatPart::Hole => "",
            })
            .collect();
        let lit = literal_node_range(ctx, range, Type::String, Literal::String(text));
        conds.push(emit_op(
            ctx,
            "==",
            (value, Type::String),
            (lit, Type::String),
            range,
        ));
    } else {
        let mut length = None;
        let mut pos = Offset::Const(0);
        let mut pending = false;
        for (j, part) in parts.iter().enumerate() {
            match part {
                ConcatPart::Hole => {
                    if pending {
                        holes.push(literal_node_range(
                            ctx,
                            range,
                            Type::String,
                            Literal::String(String::new()),
                        ));
                    }
                    pending = true;
                }
                ConcatPart::Lit(l) => {
                    let n = l.chars().count() as i64;
                    if !pending {
                        conds.push(string_test(
                            ctx,
                            gc::STRING_STARTS_WITH,
                            WirePort::Prefix,
                            value,
                            l,
                            None,
                            range,
                        ));
                        pos = pos.plus(ctx, n, range);
                    } else if concat_lit_ends(parts, j) {
                        conds.push(string_test(
                            ctx,
                            gc::STRING_ENDS_WITH,
                            WirePort::Suffix,
                            value,
                            l,
                            None,
                            range,
                        ));
                        let len = string_length(ctx, value, &mut length, range);
                        let start = Offset::Port(len).plus(ctx, -n, range);
                        if pos != Offset::Const(0) {
                            let (s, p) = (start.port(ctx, range), pos.port(ctx, range));
                            conds.push(emit_op(ctx, ">=", (s, Type::Int), (p, Type::Int), range));
                        }
                        holes.push(substring(ctx, value, pos, start, range));
                        pos = Offset::Port(len);
                    } else {
                        let start = pos.port(ctx, range);
                        let at = string_test(
                            ctx,
                            gc::STRING_FIND,
                            WirePort::Search,
                            value,
                            l,
                            Some(start),
                            range,
                        );
                        let zero = literal_node_range(ctx, range, Type::Int, Literal::Int(0));
                        conds.push(emit_op(
                            ctx,
                            ">=",
                            (at, Type::Int),
                            (zero, Type::Int),
                            range,
                        ));
                        holes.push(substring(ctx, value, pos, Offset::Port(at), range));
                        pos = Offset::Port(at).plus(ctx, n, range);
                    }
                    pending = false;
                }
            }
        }
        if pending {
            let len = string_length(ctx, value, &mut length, range);
            holes.push(substring(ctx, value, pos, Offset::Port(len), range));
        }
    }
    let cond = all_of(ctx, conds, range)
        .unwrap_or_else(|| literal_node_range(ctx, range, Type::Bool, Literal::Bool(true)));
    let lowered = ConcatLowered {
        value,
        parts: parts.to_vec(),
        cond,
        holes,
    };
    ctx.concat_cache.push(lowered.clone());
    lowered
}

/// A character offset into a string: known, or on a wire.
#[derive(Clone, Copy, PartialEq)]
enum Offset {
    Const(i64),
    Port(PortRef),
}

impl Offset {
    fn plus(self, ctx: &mut LowerCtx<'_>, n: i64, range: &SourceRange) -> Self {
        match self {
            _ if n == 0 => self,
            Self::Const(k) => Self::Const(k + n),
            Self::Port(p) => {
                let (op, k) = if n < 0 { ("-", -n) } else { ("+", n) };
                let k = literal_node_range(ctx, range, Type::Int, Literal::Int(k));
                Self::Port(emit_op(ctx, op, (p, Type::Int), (k, Type::Int), range))
            }
        }
    }

    fn port(self, ctx: &mut LowerCtx<'_>, range: &SourceRange) -> PortRef {
        match self {
            Self::Const(k) => literal_node_range(ctx, range, Type::Int, Literal::Int(k)),
            Self::Port(p) => p,
        }
    }
}

/// `value`'s length, built once per string pattern.
fn string_length(
    ctx: &mut LowerCtx<'_>,
    value: PortRef,
    cached: &mut Option<PortRef>,
    range: &SourceRange,
) -> PortRef {
    if let Some(len) = *cached {
        return len;
    }
    let len = emit_gate(
        ctx,
        gc::STRING_LENGTH,
        &[(WirePort::Input, value, Type::String)],
        Type::Int,
        range,
    );
    *cached = Some(len);
    len
}

/// The characters of `value` from `start` up to `end`.
fn substring(
    ctx: &mut LowerCtx<'_>,
    value: PortRef,
    start: Offset,
    end: Offset,
    range: &SourceRange,
) -> PortRef {
    let count = match (start, end) {
        (Offset::Const(a), Offset::Const(b)) => Offset::Const(b - a),
        (Offset::Const(0), e) => e,
        _ => {
            let (s, e) = (start.port(ctx, range), end.port(ctx, range));
            Offset::Port(emit_op(ctx, "-", (e, Type::Int), (s, Type::Int), range))
        }
    };
    let start = start.port(ctx, range);
    let count = count.port(ctx, range);
    emit_gate(
        ctx,
        gc::STRING_SUBSTRING,
        &[
            (WirePort::Input, value, Type::String),
            (WirePort::Start, start, Type::Int),
            (WirePort::Length, count, Type::Int),
        ],
        Type::String,
        range,
    )
}

/// A case-sensitive `StartsWith`/`EndsWith` (a bool) or `Find` (an index from
/// `start`, `-1` when absent) of the literal `text` in `value`.
fn string_test(
    ctx: &mut LowerCtx<'_>,
    class: &'static str,
    text_port: WirePort,
    value: PortRef,
    text: &str,
    start: Option<PortRef>,
    range: &SourceRange,
) -> PortRef {
    let text = literal_node_range(ctx, range, Type::String, Literal::String(text.to_string()));
    let case = literal_node_range(ctx, range, Type::Bool, Literal::Bool(true));
    let mut inputs = vec![
        (WirePort::Input, value, Type::String),
        (text_port, text, Type::String),
        (WirePort::BCaseSensitive, case, Type::Bool),
    ];
    let out = match start {
        Some(s) => {
            inputs.push((WirePort::Start, s, Type::Int));
            Type::Int
        }
        None => Type::Bool,
    };
    emit_gate(ctx, class, &inputs, out, range)
}

/// One pure gate of `class`, its inputs wired from `inputs`, its `Output` the
/// result.
fn emit_gate(
    ctx: &mut LowerCtx<'_>,
    class: &'static str,
    inputs: &[(WirePort, PortRef, Type)],
    out: Type,
    range: &SourceRange,
) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: class,
        source_range: range.clone(),
        ports: GateIO {
            inputs: inputs
                .iter()
                .map(|(p, _, t)| PortSpec::new(p.sym(), t.clone()))
                .collect(),
            outputs: vec![PortSpec::new(*sym::OUTPUT, out)],
        },
        ..Default::default()
    });
    for (p, src, _) in inputs {
        ctx.connect(*src, node_id.port(*p));
    }
    node_id.port(WirePort::Output)
}

/// The gate of the binary operator `op` over two values of the given types, as
/// the operator catalog resolves it.
pub(super) fn emit_op(
    ctx: &mut LowerCtx<'_>,
    op: &str,
    (a, a_ty): (PortRef, Type),
    (b, b_ty): (PortRef, Type),
    range: &SourceRange,
) -> PortRef {
    let Some(rule) = crate::catalog::operators::resolve_op(op, &[a_ty, b_ty]) else {
        return synthesise_unsupported_range(ctx, "a pattern test", range);
    };
    rule_gate(
        ctx,
        rule,
        &[Some(a), Some(b)],
        rule.operands,
        Some("match pattern test"),
        range,
    )
}

/// `disc` equals one of `keys`.
pub(super) fn emit_disc_in(
    ctx: &mut LowerCtx<'_>,
    disc: PortRef,
    keys: &[i64],
    range: &SourceRange,
) -> PortRef {
    let conds: Vec<PortRef> = keys
        .iter()
        .map(|&k| emit_disc_eq(ctx, disc, k, range))
        .collect();
    conds
        .into_iter()
        .reduce(|a, b| emit_op(ctx, "||", (a, Type::Bool), (b, Type::Bool), range))
        .unwrap_or_else(|| literal_node_range(ctx, range, Type::Bool, Literal::Bool(false)))
}
