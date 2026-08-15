//! AST rewrites that are not grammar: gate-builtin call forms and the
//! source-offset fixup for re-parsed string interpolations.

use super::*;

fn shift_pos(p: &mut Pos, origin: &Pos) {
    p.offset += origin.offset;
    p.line = p.line.saturating_sub(1) + origin.line;
    if p.line == origin.line {
        p.col = p.col.saturating_sub(1) + origin.col;
    }
}

/// Rewrite an EXPRESSION-form gate builtin into its method-call / read form:
/// `GetVariable(v)` → `v`; `GetMapElement(m, k)` → `m.get(k)`. The container is
/// the first positional argument (resolved by name to its ref downstream, like a
/// method receiver). Returns the original `Call` unchanged for non-builtins or a
/// malformed shape (leaving the normal call path to diagnose it).
pub(super) fn desugar_gate_call(
    callee: Box<Expr>,
    args: Vec<CallArg>,
    type_args: Vec<TypeExpr>,
    range: SourceRange,
) -> Expr {
    use crate::catalog::gate_builtins as gb;
    let name = match callee.as_ref() {
        Expr::Ident { name, .. } if type_args.is_empty() => name.clone(),
        _ => return Expr::Call { callee, args, type_args, range },
    };
    // `GetVariable(v)` reads `v` — desugar to the bare receiver expression.
    if name == gb::GET_VARIABLE && args.len() == 1 && matches!(args[0], CallArg::Positional(_)) {
        if let Some(CallArg::Positional(v)) = args.into_iter().next() {
            return v;
        }
        unreachable!();
    }
    if let Some(method) = gb::method_for(&name)
        && matches!(args.first(), Some(CallArg::Positional(_)))
    {
        let mut it = args.into_iter();
        let container = match it.next() {
            Some(CallArg::Positional(c)) => c,
            _ => unreachable!(),
        };
        let rest: Vec<CallArg> = it.collect();
        let fa = Expr::FieldAccess {
            obj: Box::new(container),
            field: method.to_string(),
            range: range.clone(),
        };
        return Expr::Call { callee: Box::new(fa), args: rest, type_args: Vec::new(), range };
    }
    Expr::Call { callee, args, type_args, range }
}

/// Rewrite a STATEMENT-form gate builtin into the equivalent assignment:
/// `SetVariable(v, x)` → `v = x`; `SetArrayElement(a, i, x)` → `a[i] = x`;
/// `IncrementVariable(v, x)` → `v = v + x`. Returns `None` for anything else.
pub(super) fn gate_builtin_assign(e: &Expr) -> Option<Assign> {
    use crate::catalog::gate_builtins as gb;
    let Expr::Call { callee, args, range, type_args } = e else { return None };
    if !type_args.is_empty() {
        return None;
    }
    let Expr::Ident { name, .. } = callee.as_ref() else { return None };
    let pos = |i: usize| match args.get(i) {
        Some(CallArg::Positional(x)) => Some(x.clone()),
        _ => None,
    };
    let assign = |target: Expr, value: Expr| Assign { target, value, range: range.clone() };
    match name.as_str() {
        gb::SET_VARIABLE if args.len() == 2 => Some(assign(pos(0)?, pos(1)?)),
        gb::SET_ARRAY_ELEMENT if args.len() == 3 => {
            let target = Expr::IndexAccess {
                obj: Box::new(pos(0)?),
                index: Box::new(pos(1)?),
                range: range.clone(),
            };
            Some(assign(target, pos(2)?))
        }
        gb::INCREMENT_VARIABLE if args.len() == 2 => {
            let v = pos(0)?;
            let value = Expr::BinOp {
                op: "+".into(),
                left: Box::new(v.clone()),
                right: Box::new(pos(1)?),
                range: range.clone(),
            };
            Some(assign(v, value))
        }
        _ => None,
    }
}

pub(super) fn shift_expr_offsets(expr: &mut Expr, origin: Pos) {
    {
        let r = expr.range_mut();
        shift_pos(&mut r.start, &origin);
        shift_pos(&mut r.end, &origin);
    }
    match expr {
        Expr::FieldAccess { obj, .. } => shift_expr_offsets(obj, origin),
        Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
            shift_expr_offsets(operand, origin);
        }
        Expr::IndexAccess { obj, index, .. } => {
            shift_expr_offsets(obj, origin);
            shift_expr_offsets(index, origin);
        }
        Expr::TuplePick { obj, .. } => shift_expr_offsets(obj, origin),
        Expr::UnOp { operand, .. } => shift_expr_offsets(operand, origin),
        Expr::BinOp { left, right, .. } => {
            shift_expr_offsets(left, origin);
            shift_expr_offsets(right, origin);
        }
        Expr::Call { callee, args, .. } => {
            shift_expr_offsets(callee, origin);
            for a in args {
                match a {
                    CallArg::Positional(e) => shift_expr_offsets(e, origin),
                    CallArg::Named { value, .. } => shift_expr_offsets(value, origin),
                    CallArg::Spread(e) => shift_expr_offsets(e, origin),
                }
            }
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            shift_expr_offsets(cond, origin);
            shift_expr_offsets(then_branch, origin);
            shift_expr_offsets(else_branch, origin);
        }
        Expr::MatchExpr { scrutinee, .. } => {
            shift_expr_offsets(scrutinee, origin);
        }
        Expr::Array { elements, .. } => {
            for el in elements {
                shift_expr_offsets(el.expr_mut(), origin);
            }
        }
        Expr::MapLit { entries, .. } => {
            for e in entries.iter_mut() {
                shift_expr_offsets(&mut e.key, origin);
                shift_expr_offsets(&mut e.value, origin);
            }
        }
        _ => {}
    }
}
