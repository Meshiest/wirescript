//! Compile-time interpreter for a `const mod`'s body: a tree walker over
//! `Block`/`Stmt` that hands every expression to [`eval_expr`].
//!
//! It runs `let`/`const`, `return`, `if`, `out` and call statements; any other
//! statement (a `for` loop included) is unsupported, so a body is straight-line
//! code plus branches. A [`Budget`] bounds total work, so a deep or recursive
//! chain of `const mod` calls fails with `ConstReason::BudgetExceeded` (WS048)
//! instead of overflowing the stack.

use crate::ast::{Block, CallArg, ChipDecl, Expr, ExprStmt, NamedOutput, Param, Stmt, TypeExpr};
use crate::diagnostic::SourceRange;
use crate::ir::{Literal, Type};
use crate::types::coerce::widening_join_all;
use crate::types::infer::Subst;

use super::destructure::bind_destructured;
use super::error::{ConstError, ConstReason};
use super::expr::{ConstCtx, eval_expr};

/// Ceiling on the statements executed across one top-level `eval_call`,
/// nested calls included. Counts down once per statement, never restored.
const MAX_STEPS: u32 = 10_000;
/// Ceiling on the `eval_call`s reachable from one top-level call. A running
/// total, never restored when a nested call returns, so it bounds a call
/// chain's total length rather than its stack depth.
const MAX_DEPTH: u32 = 32;

/// Bounds on the interpreter's total work for one top-level `eval_call`.
/// Both bounds count down monotonically; hitting zero on either yields
/// `ConstReason::BudgetExceeded`. Construct a fresh one with
/// [`Budget::default`].
pub(crate) struct Budget {
    pub steps: u32,
    pub depth: u32,
    /// Set once an evaluation under this budget applies an integer operation
    /// the certified fold table has no result for (an overflow, a division of
    /// mixed signs with a remainder): the value is the evaluator's, not the
    /// gate's (see [`super::integer_ops_certified`]).
    pub uncertified: bool,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            steps: MAX_STEPS,
            depth: MAX_DEPTH,
            uncertified: false,
        }
    }
}

/// Execute a `const mod`'s body at compile time and return its result: the
/// `return` value, or else what its `out name = expr` statements collected,
/// shaped as `typecheck::call` types the call (see [`build_output_value`]).
///
/// `args` are evaluated literals in `decl.inputs` order, each evaluated in the
/// caller's own scope. `type_args` are the written type arguments
/// (`f<float>(5)`), empty when there are none.
///
/// The body sees the callee module's constants with the parameters on top,
/// never the caller's local bindings, so it resolves the same names an
/// ordinary inlined `mod` body would.
pub(super) fn eval_call(
    decl: &ChipDecl,
    args: &[Literal],
    type_args: &[Type],
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Literal, ConstError> {
    if budget.depth == 0 {
        return Err(ConstError::new(ConstReason::BudgetExceeded, &decl.range));
    }
    budget.depth -= 1;

    let module = cx.module_of(decl);
    let types = cx.types.in_file(&decl.range.file);
    let bound = type_params_of_call(decl, args, type_args, types);
    let declared = |te: &TypeExpr| types.resolve_in(te, &bound);
    let mut env = crate::const_scope::ConstNames::of(std::sync::Arc::clone(&module));
    for (i, param) in decl.inputs.iter().enumerate() {
        // A parameter with no argument is blamed here; left unbound, it would
        // surface as a false "'x' is a runtime value" at its first read.
        let Some(arg) = args.get(i) else {
            return Err(ConstError::unsupported(
                "a call missing an argument for this parameter",
                &param.range,
            ));
        };
        env.insert(
            &param.name,
            super::coerce_const_to_type(arg.clone(), &declared(&param.typ), types),
        );
    }
    let mut call_cx = ConstCtx {
        module_consts: module,
        type_args: bound.clone(),
        ..cx.with_consts(env)
    };

    // Kept outside `call_cx` so an `out` inside a taken `if` branch survives
    // the branch's declaration rewind (see `exec_block`).
    let mut outputs: Vec<(String, Literal)> = Vec::new();
    let sig = OutputSig {
        declared: &decl.outputs,
        earliest_out: earliest_declared_out(&decl.body, &decl.outputs),
    };
    let flow = exec_block(&decl.body, &mut call_cx, &sig, &mut outputs, budget)?.0;
    let result = match decl.outputs.as_slice() {
        [only] => declared(&only.typ),
        outs => Type::Record(
            outs.iter()
                .map(|o| (o.name.clone(), declared(&o.typ)))
                .collect(),
        ),
    };
    let value = match flow {
        // The `Stmt::Return` arm has already rejected the returns lowering
        // would wire differently, so a returned value wins outright.
        Flow::Returned {
            value: Some(value), ..
        } => Ok(reorder_to_declaration(value, &decl.outputs)),
        // With no returned value, the result is what the `out`s collected.
        Flow::Returned { value: None, range } => {
            // Blamed on the `return`, not as falling off the end: the arrow
            // form synthesises a `_` output, which would hide that message.
            build_or_else(&decl.outputs, &outputs, || {
                ConstError::unsupported("a `return` with no value", &range)
            })
        }
        Flow::Fell => build_or_else(&decl.outputs, &outputs, || {
            ConstError::unsupported(
                "a const mod body that falls off the end without a `return`",
                &decl.body.range,
            )
        }),
    };
    value.map(|v| super::coerce_const_to_type(v, &result, types))
}

/// The types a call's written type arguments name, read in the calling file
/// with the caller's own type parameters bound.
pub(super) fn written_type_args(type_args: &[TypeExpr], cx: &ConstCtx<'_>) -> Vec<Type> {
    type_args
        .iter()
        .map(|te| {
            cx.types
                .in_file(&te.range().file)
                .resolve_in(te, &cx.type_args)
        })
        .collect()
}

/// What each type parameter of `decl` stands for in a call with `args`: the
/// written type arguments when there are as many as parameters, otherwise
/// what the arguments bind it to, joined as typecheck joins them (an integer
/// and a float bind `float`). A parameter neither pins is `any`, so every
/// parameter is bound and a declared type naming one never reads an alias,
/// enum or record of the same name.
fn type_params_of_call(
    decl: &ChipDecl,
    args: &[Literal],
    type_args: &[Type],
    types: &super::DeclaredTypes,
) -> Subst {
    let names: Vec<&String> = decl.type_params.iter().map(|p| &p.name).collect();
    if type_args.len() == names.len() {
        return names
            .into_iter()
            .cloned()
            .zip(type_args.iter().cloned())
            .collect();
    }
    let open: Subst = names
        .iter()
        .map(|n| ((*n).clone(), Type::Param((*n).clone())))
        .collect();
    let mut constraints = Vec::new();
    for (param, arg) in decl.inputs.iter().zip(args) {
        crate::types::mono::collect(
            &types.resolve_in(&param.typ, &open),
            &literal_type(arg),
            &mut constraints,
        );
    }
    names
        .into_iter()
        .map(|n| {
            let bound = constraints
                .iter()
                .map(|crate::types::infer::Constraint::Eq(v, t)| (v, t))
                .filter(|(v, t)| *v == n && !matches!(t, Type::Any))
                .map(|(_, t)| t.clone());
            let joined = widening_join_all(bound);
            (n.clone(), joined.unwrap_or(Type::Any))
        })
        .collect()
}

/// The type of the constant `lit`, as far as its value shows it.
fn literal_type(lit: &Literal) -> Type {
    let join = |ts: Vec<Type>| widening_join_all(ts).unwrap_or(Type::Any);
    match lit {
        Literal::Bool(_) => Type::Bool,
        Literal::Int(_) => Type::Int,
        Literal::Float(_) => Type::Float,
        Literal::String(_) => Type::String,
        Literal::Vector { .. } => Type::Vector,
        Literal::Rotator { .. } => Type::Rotator,
        Literal::Quat { .. } => Type::Quat,
        Literal::Color { .. } | Literal::LinearColor { .. } => Type::Color,
        Literal::Array(items) => {
            Type::Array(Box::new(join(items.iter().map(literal_type).collect())))
        }
        Literal::Map(pairs) => Type::Map(
            Box::new(join(pairs.iter().map(|(k, _)| literal_type(k)).collect())),
            Box::new(join(pairs.iter().map(|(_, v)| literal_type(v)).collect())),
        ),
        Literal::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(n, l)| (n.clone(), literal_type(l)))
                .collect(),
        ),
        _ => Type::Any,
    }
}

/// Build the `out`-collected result, or return `empty()` when the mod declares
/// no outputs or the body assigned none. The second case keeps `-> int { }`
/// blaming the body rather than the synthesised `_` output in the signature.
fn build_or_else(
    declared: &[NamedOutput],
    collected: &[(String, Literal)],
    empty: impl Fn() -> ConstError,
) -> Result<Literal, ConstError> {
    if declared.is_empty() || collected.is_empty() {
        return Err(empty());
    }
    build_output_value(declared, collected)
}

/// Put a returned record's fields into signature declaration order, matching
/// the record the `out` path builds. Undeclared fields follow in source order;
/// this only reorders, never adds, drops or rejects a field.
fn reorder_to_declaration(value: Literal, declared: &[NamedOutput]) -> Literal {
    let Literal::Record(fields) = value else {
        return value;
    };
    if declared.len() < 2 {
        return Literal::Record(fields);
    }
    let mut ordered: Vec<(String, Literal)> = Vec::with_capacity(fields.len());
    for d in declared {
        if let Some((n, v)) = fields.iter().find(|(n, _)| *n == d.name) {
            ordered.push((n.clone(), v.clone()));
        }
    }
    for (n, v) in &fields {
        if !declared.iter().any(|d| d.name == *n) {
            ordered.push((n.clone(), v.clone()));
        }
    }
    Literal::Record(ordered)
}

/// Shape a `const mod`'s `out`-collected value the way `typecheck::call`
/// types the same call: one declared output gives that value bare, several
/// give a `Literal::Record` in declaration order (not assignment order).
///
/// The completion `exec` field typecheck adds for an `exec =` call is not
/// mirrored: an exec edge has no constant form, and nothing reads it off a
/// constant. An output the body never assigned is an error on that output's
/// own range; collected names the signature does not declare are ignored.
fn build_output_value(
    outs: &[NamedOutput],
    collected: &[(String, Literal)],
) -> Result<Literal, ConstError> {
    let mut fields = Vec::with_capacity(outs.len());
    for out in outs {
        let Some((_, value)) = collected.iter().find(|(n, _)| *n == out.name) else {
            return Err(ConstError::new(
                ConstReason::UnsupportedMessage(
                    "this const mod body never assigns a value to this output",
                ),
                &out.range,
            ));
        };
        fields.push((out.name.clone(), value.clone()));
    }
    if let [(_, only)] = fields.as_slice() {
        return Ok(only.clone());
    }
    Ok(Literal::Record(fields))
}

/// What each name a `let`/`const` in this block bound held just before it:
/// `Some(lit)` when an enclosing scope bound it, `None` when it was unbound.
/// Restoring these when a branch ends confines its declarations to the branch.
type Shadowed = Vec<(String, Option<Literal>)>;

/// How a block stopped running. A bare `return` produces no value yet must
/// still stop the enclosing body, so it is distinct from `Fell`.
enum Flow {
    /// Ran off the end of the block.
    Fell,
    /// A `return` stopped the block. `value` is `None` for a bare `return`;
    /// `range` is that statement's own, so `eval_call` can blame the actual
    /// `return` when the body turns out to produce nothing.
    Returned {
        value: Option<Literal>,
        range: SourceRange,
    },
}

/// The enclosing `const mod`'s signature facts that statement arms need,
/// computed once per [`eval_call`] because they concern the whole body.
struct OutputSig<'a> {
    /// The signature's declared outputs, in declaration order.
    declared: &'a [NamedOutput],
    /// Source offset of the earliest `out` anywhere in the body that targets
    /// a declared output. Lowering wires the first `out` in source order, even
    /// one in an untaken branch, so a valued `return` after it would disagree.
    earliest_out: Option<usize>,
}

/// Source offset of the earliest `out` in `block`, both arms of nested `if`s
/// included, that targets one of `declared`. A syntactic scan, not a record of
/// what ran (see [`OutputSig::earliest_out`]).
fn earliest_declared_out(block: &Block, declared: &[NamedOutput]) -> Option<usize> {
    fn min_opt(a: Option<usize>, b: Option<usize>) -> Option<usize> {
        match (a, b) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        }
    }
    let mut best = None;
    for stmt in &block.stmts {
        let here = match stmt {
            Stmt::OutBinding(o) if declared.iter().any(|d| d.name == o.name) => {
                Some(o.range.start.offset)
            }
            Stmt::If(i) => min_opt(
                earliest_declared_out(&i.then_block, declared),
                i.else_block
                    .as_ref()
                    .and_then(|b| earliest_declared_out(b, declared)),
            ),
            _ => None,
        };
        best = min_opt(best, here);
    }
    best
}

/// Run `block`'s statements against `cx`, applying every `let`/`const` it
/// declares and every mutation of a bound const array or map (see
/// [`exec_call_stmt`]). Stops at a `return`; `Err` names the statement or
/// sub-expression that could not be evaluated.
///
/// The returned [`Shadowed`] lists this block's own declarations in order.
/// `exec_block` writes straight into `cx`, so the `Stmt::If` arm scopes a
/// branch by replaying these in reverse.
///
/// `outputs` collects every `out name = expr` that runs, nested blocks
/// included. The same `Vec` is passed into every branch and lives outside
/// `cx`, so the rewind never drops an `out` from a taken branch.
fn exec_block(
    block: &Block,
    cx: &mut ConstCtx<'_>,
    sig: &OutputSig<'_>,
    outputs: &mut Vec<(String, Literal)>,
    budget: &mut Budget,
) -> Result<(Flow, Shadowed), ConstError> {
    let mut shadowed: Shadowed = Vec::new();
    for stmt in &block.stmts {
        if budget.steps == 0 {
            return Err(ConstError::new(ConstReason::BudgetExceeded, stmt.range()));
        }
        budget.steps -= 1;

        match stmt {
            Stmt::Let(l) => {
                // A plain `let` in a const body also needs a constant
                // initializer: nothing here can fall back to a gate.
                let lit = super::eval_let_value(l, cx, budget)?;
                let pairs = bind_destructured(&l.binding, lit)?;
                // One snapshot per bound name, taken just before its insert,
                // so a rewind restores every name a destructure bound,
                // including mutations made to it earlier in this block.
                for (name, value) in pairs {
                    shadowed.push((name.clone(), cx.consts.get(name.as_str()).cloned()));
                    cx.consts.insert(&name, value);
                }
            }
            Stmt::Return { value, range } => {
                let Some(e) = value else {
                    // A bare `return` is the `out` form's early exit:
                    // `eval_call` builds the result from what was collected.
                    return Ok((
                        Flow::Returned {
                            value: None,
                            range: range.clone(),
                        },
                        shadowed,
                    ));
                };
                // Lowering wires the first `out` in source order, so a valued
                // `return` after any `out` (taken or not) would bake a value
                // the plain `mod` does not produce. `return` before `out`
                // agrees and stays legal.
                if let Some(out_offset) = sig.earliest_out
                    && out_offset < range.start.offset
                {
                    return Err(ConstError::new(
                        ConstReason::UnsupportedMessage(
                            "this `return` of a value follows an earlier `out` in the same \
                             const mod. The wire graph keeps that first assignment and drops \
                             this one, so the two would disagree",
                        ),
                        range,
                    ));
                }
                let lit = eval_expr(e, cx, budget)?;
                // Lowering wires a scalar `return` only when the mod has one
                // output, so with several it is dropped and a field read off
                // it would miscompile. A record return sets each output.
                if sig.declared.len() > 1 && !matches!(lit, Literal::Record(_)) {
                    return Err(ConstError::new(
                        ConstReason::UnsupportedMessage(
                            "this const mod declares several named outputs, so a `return` of a \
                             single value has nowhere to go. Assign each output with \
                             `out name = …`, or return a record",
                        ),
                        range,
                    ));
                }
                return Ok((
                    Flow::Returned {
                        value: Some(lit),
                        range: range.clone(),
                    },
                    shadowed,
                ));
            }
            Stmt::If(if_stmt) => {
                let cond = eval_expr(&if_stmt.cond, cx, budget)?;
                let Literal::Bool(take_then) = cond else {
                    return Err(ConstError::unsupported(
                        "an `if` condition that is not a compile-time bool",
                        if_stmt.cond.range(),
                    ));
                };
                // Only the taken block runs, on a copy of the environment;
                // the untaken block need not be evaluable.
                let branch = if take_then {
                    Some(&if_stmt.then_block)
                } else {
                    if_stmt.else_block.as_ref()
                };
                if let Some(taken) = branch {
                    let mut branch_cx = cx.with_consts(cx.consts.clone());
                    let (result, branch_shadowed) =
                        exec_block(taken, &mut branch_cx, sig, outputs, budget)?;
                    // Keep the branch's mutations of outer bindings
                    // (`if c { t.push(x) }`), then rewind its declarations in
                    // reverse so a name declared twice gets its pre-branch
                    // value. `t.push(2)` then `const t = [9]` restores the
                    // pushed `t`.
                    cx.consts = branch_cx.consts;
                    for (name, prev) in branch_shadowed.into_iter().rev() {
                        match prev {
                            Some(value) => cx.consts.insert(&name, value),
                            // The declaration introduced the name: remove it.
                            None => cx.consts.remove(name.as_str()),
                        }
                    }
                    // A `return` in the branch, bare or valued, stops the
                    // enclosing body too.
                    if let Flow::Returned { value, range } = result {
                        return Ok((Flow::Returned { value, range }, shadowed));
                    }
                }
            }
            // `out name = expr` records a value under `name`; a later write
            // to the same name wins. A bare `out name` has no value to
            // evaluate and is unsupported.
            Stmt::OutBinding(o) => {
                // An `out` the signature does not declare is a legal
                // body-declared output. It is evaluated but not part of the
                // result, since typecheck types the call from the signature.
                let Some(value_expr) = &o.value else {
                    return Err(ConstError::unsupported("an `out` with no value", &o.range));
                };
                let lit = eval_expr(value_expr, cx, budget)?;
                match outputs.iter_mut().find(|(n, _)| *n == o.name) {
                    Some(slot) => slot.1 = lit,
                    None => outputs.push((o.name.clone(), lit)),
                }
            }
            Stmt::ExprStmt(es) => exec_call_stmt(es, cx, budget)?,
            other => {
                return Err(ConstError::unsupported("this statement", other.range()));
            }
        }
    }
    Ok((Flow::Fell, shadowed))
}

/// Run an expression statement: a call to another `const mod` (result
/// discarded) or a mutating method call on a local const array or map (see
/// [`exec_mutating_method_call`]). Anything else is unsupported, blamed on
/// the statement.
fn exec_call_stmt(
    es: &ExprStmt,
    cx: &mut ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<(), ConstError> {
    let unsupported = || ConstError::unsupported("this statement", &es.range);

    let Expr::Call {
        callee,
        args,
        type_args,
        ..
    } = &es.expr
    else {
        return Err(unsupported());
    };

    if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
        return exec_mutating_method_call(obj, field, args, es, cx, budget);
    }

    let Expr::Ident { name, .. } = callee.as_ref() else {
        return Err(unsupported());
    };
    let Some(lookup) = cx.lookup_mod else {
        return Err(unsupported());
    };
    let Some(target) = lookup(name) else {
        return Err(unsupported());
    };
    if !target.is_const {
        return Err(unsupported());
    }

    let arg_lits = bind_call_args(&target.inputs, args, cx, budget)?;

    if budget.depth == 0 {
        return Err(ConstError::new(ConstReason::BudgetExceeded, &es.range));
    }
    let written = written_type_args(type_args, cx);
    eval_call(&target, &arg_lits, &written, &*cx, budget)?;
    Ok(())
}

/// A mutating method call (`t.push(x)`, `t.set(k, v)`) on a const binding:
/// the updated copy is written back into `cx.consts` under the same name.
/// Only a const mod body runs this, so it is the only place a const
/// collection can be mutated.
///
/// `obj` must be a plain name already bound to an array or map; anything else
/// is the generic "this statement" error.
fn exec_mutating_method_call(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    es: &ExprStmt,
    cx: &mut ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<(), ConstError> {
    let unsupported = || ConstError::unsupported("this statement", &es.range);

    let Expr::Ident { name, .. } = obj else {
        return Err(unsupported());
    };
    let Some(current) = cx.consts.get(name.as_str()).cloned() else {
        return Err(unsupported());
    };

    let updated = match current {
        Literal::Array(items) => {
            Literal::Array(mutate_array(items, field, args, es, &*cx, budget)?)
        }
        Literal::Map(pairs) => Literal::Map(mutate_map(pairs, field, args, es, &*cx, budget)?),
        _ => return Err(unsupported()),
    };
    cx.consts.insert(name, updated);
    Ok(())
}

/// Apply `push`, `set`, `clear` or `append` to a const array, mirroring the
/// runtime array gates. `set(i, v)` exists only here, standing in for
/// `arr[i] = v`. Any other method is `ConstReason::UnsupportedMethod`.
fn mutate_array(
    mut items: Vec<Literal>,
    field: &str,
    args: &[CallArg],
    es: &ExprStmt,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Vec<Literal>, ConstError> {
    match field {
        "push" => {
            let [value] = method_args(&["value"], args, es)?[..] else {
                unreachable!()
            };
            items.push(eval_expr(value, cx, budget)?);
            Ok(items)
        }
        "set" => {
            let [index, value] = method_args(&["index", "value"], args, es)?[..] else {
                unreachable!()
            };
            let idx_lit = eval_expr(index, cx, budget)?;
            let val_lit = eval_expr(value, cx, budget)?;
            match idx_lit {
                Literal::Int(i) if i >= 0 && (i as usize) < items.len() => {
                    items[i as usize] = val_lit;
                    Ok(items)
                }
                Literal::Int(i) => Err(ConstError::new(
                    ConstReason::ArrayIndexOutOfRange {
                        index: i,
                        len: items.len(),
                    },
                    &es.range,
                )),
                _ => Err(ConstError::unsupported(
                    "a non-integer array index",
                    index.range(),
                )),
            }
        }
        "clear" => {
            items.clear();
            Ok(items)
        }
        "append" => {
            let [source] = method_args(&["source"], args, es)?[..] else {
                unreachable!()
            };
            match eval_expr(source, cx, budget)? {
                Literal::Array(mut more) => {
                    items.append(&mut more);
                    Ok(items)
                }
                _ => Err(ConstError::unsupported(
                    "appending a non-array value",
                    source.range(),
                )),
            }
        }
        other => Err(unsupported_collection_method(
            other,
            "array",
            "push, set, clear, and append",
            es,
        )),
    }
}

/// Apply `set`, `remove` or `clear` to a const map, mirroring the runtime
/// map gates one to one. Any other method is `ConstReason::UnsupportedMethod`.
fn mutate_map(
    mut pairs: Vec<(Literal, Literal)>,
    field: &str,
    args: &[CallArg],
    es: &ExprStmt,
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Vec<(Literal, Literal)>, ConstError> {
    match field {
        "set" => {
            let [key, value] = method_args(&["key", "value"], args, es)?[..] else {
                unreachable!()
            };
            let key_lit = eval_expr(key, cx, budget)?;
            let val_lit = eval_expr(value, cx, budget)?;
            match pairs.iter_mut().find(|(k, _)| *k == key_lit) {
                Some(entry) => entry.1 = val_lit,
                None => pairs.push((key_lit, val_lit)),
            }
            Ok(pairs)
        }
        "remove" => {
            let [key] = method_args(&["key"], args, es)?[..] else {
                unreachable!()
            };
            let key_lit = eval_expr(key, cx, budget)?;
            pairs.retain(|(k, _)| *k != key_lit);
            Ok(pairs)
        }
        "clear" => {
            pairs.clear();
            Ok(pairs)
        }
        other => Err(unsupported_collection_method(
            other,
            "map",
            "set, remove, and clear",
            es,
        )),
    }
}

/// How [`bind_args`] treats an argument that fits no parameter.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Arity {
    /// An extra positional argument, a name no parameter has, a second
    /// argument for one parameter, or a spread is refused.
    Strict,
    /// Such arguments are skipped; of two arguments for one parameter the
    /// later wins.
    Lenient,
}

/// Why [`bind_args`] refused under [`Arity::Strict`].
pub(super) enum Misfit<'a> {
    /// An argument fits no parameter, or a parameter got two.
    Argument,
    /// A spread, which has no constant form.
    Spread(&'a Expr),
}

/// Bind `args` to the parameters `names`: positional arguments fill them left
/// to right from index `first` (the ones before it are bound by the caller,
/// such as a method's receiver), named arguments by name. One slot per
/// parameter, `None` where no argument was given.
pub(super) fn bind_args<'a>(
    names: &[&str],
    first: usize,
    args: &'a [CallArg],
    arity: Arity,
) -> Result<Vec<Option<&'a Expr>>, Misfit<'a>> {
    let mut slots: Vec<Option<&'a Expr>> = vec![None; names.len()];
    let mut next_pos = first;
    for a in args {
        let (slot, value) = match a {
            CallArg::Positional(v) => {
                next_pos += 1;
                ((next_pos - 1 < names.len()).then_some(next_pos - 1), v)
            }
            CallArg::Named { name, value, .. } => (names.iter().position(|n| n == name), value),
            CallArg::Spread(v) => match arity {
                Arity::Strict => return Err(Misfit::Spread(v)),
                Arity::Lenient => continue,
            },
        };
        match (slot, arity) {
            (Some(i), Arity::Strict) if slots[i].is_some() => return Err(Misfit::Argument),
            (Some(i), _) => slots[i] = Some(value),
            (None, Arity::Strict) => return Err(Misfit::Argument),
            (None, Arity::Lenient) => {}
        }
    }
    Ok(slots)
}

/// The arguments of a mutating method on a const array or map, bound to the
/// method's parameter names; each one is required.
fn method_args<'a>(
    names: &[&str],
    args: &'a [CallArg],
    es: &ExprStmt,
) -> Result<Vec<&'a Expr>, ConstError> {
    let Ok(slots) = bind_args(names, 0, args, Arity::Lenient) else {
        return Err(arity_error(es));
    };
    slots
        .into_iter()
        .map(|s| s.ok_or_else(|| arity_error(es)))
        .collect()
}

/// A mutating method call missing a required argument, blamed on the whole
/// statement. Typecheck does not arity-check these methods.
fn arity_error(es: &ExprStmt) -> ConstError {
    ConstError::unsupported("a call with the wrong number of arguments", &es.range)
}

/// A method that is not a supported mutation of a const `kind` ("array" or
/// "map"), naming the method.
fn unsupported_collection_method(
    field: &str,
    kind: &'static str,
    supported: &'static str,
    es: &ExprStmt,
) -> ConstError {
    ConstError::new(
        ConstReason::UnsupportedMethod(format!(
            "`.{field}(…)` is not a supported compile-time {kind} mutation; only {supported} are"
        )),
        &es.range,
    )
}

/// Bind `args` to `inputs` leniently with [`bind_args`] and evaluate each in
/// the caller's `cx`; an omitted argument takes the parameter's default,
/// also evaluated in the caller's `cx`. A parameter with neither is
/// `Unsupported`, blamed on the parameter.
///
/// Arguments share the caller's `budget`, so a recursive `const mod` reached
/// through an argument still hits `BudgetExceeded` instead of a stack overflow.
pub(super) fn bind_call_args(
    inputs: &[Param],
    args: &[CallArg],
    cx: &ConstCtx<'_>,
    budget: &mut Budget,
) -> Result<Vec<Literal>, ConstError> {
    let names: Vec<&str> = inputs.iter().map(|p| p.name.as_str()).collect();
    let Ok(slots) = bind_args(&names, 0, args, Arity::Lenient) else {
        unreachable!("lenient binding refuses nothing")
    };
    let mut out = Vec::with_capacity(inputs.len());
    for (p, slot) in inputs.iter().zip(slots) {
        let Some(expr) = slot.or(p.default.as_ref()) else {
            return Err(ConstError::unsupported(
                "a call missing an argument for this parameter",
                &p.range,
            ));
        };
        out.push(eval_expr(expr, cx, budget)?);
    }
    Ok(out)
}
