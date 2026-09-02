//! Compile-time interpreter for a `const mod`'s BODY.
//!
//! `expr.rs` evaluates a single expression; a `const mod` has statements, so
//! calling one at compile time needs a small tree-walking interpreter over
//! `Block`/`Stmt`, delegating every expression it meets back to
//! [`eval_expr`].
//!
//! Termination is structural, not a matter of care: Wirescript has no loop
//! construct, and WS020 (`lower/call.rs`) rejects a chip/mod re-entering its
//! own declaration, so const evaluation can never recurse into itself. This
//! interpreter therefore only ever walks straight-line code plus branching —
//! but a [`Budget`] still bounds total work, so a long chain of DISTINCT
//! `const mod`s calling one another fails with `ConstReason::BudgetExceeded`
//! (WS048) rather than a stack overflow.

use crate::ast::{Block, CallArg, ChipDecl, Expr, ExprStmt, NamedOutput, Param, Stmt};
use crate::collections::HashMap;
use crate::diagnostic::SourceRange;
use crate::ir::Literal;
use crate::lower::ConstEnv;

use super::destructure::bind_destructured;
use super::error::{ConstError, ConstReason};
use super::expr::{eval_expr, ConstCtx};

/// Ceiling on the number of statements executed across an entire
/// `eval_call` — including everything nested calls execute. Counts down
/// once per statement and is never restored.
const MAX_STEPS: u32 = 10_000;
/// Ceiling on the number of nested `eval_call`s reachable from one top-level
/// call. Wirescript has no recursion (WS020), so the only way a call chain
/// gets deep is a straight-line sequence of distinct `const mod`s calling
/// each other — a running total catches that exactly as well as a true
/// stack-depth counter would, with none of the push/pop bookkeeping, so
/// `depth` (like `steps`) only ever counts down and is never restored when a
/// nested call returns.
const MAX_DEPTH: u32 = 32;

/// Bounds on the interpreter's total work for one top-level `eval_call`.
/// Both fields count down monotonically; hitting zero on either yields
/// `ConstReason::BudgetExceeded`. Construct a fresh one with
/// [`Budget::default`].
pub struct Budget {
    pub steps: u32,
    pub depth: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { steps: MAX_STEPS, depth: MAX_DEPTH }
    }
}

/// Execute a `const mod`'s body at compile time and return its result: the
/// `return` value when the body has one, or — for a body whose value comes
/// from `out name = expr` statements instead — whatever those collected,
/// shaped by [`build_output_value`] to match EXACTLY how `typecheck::call`
/// types the same call (a bare value for one output, a `Literal::Record` for
/// several). A multi-output record is the SAME shape a `return { a: .., b:
/// .. }` body already produces (see `const_eval/expr.rs`'s `RecordLit`
/// handling) — the language already models multi-output as a record, so the
/// `out` form is just a second way to build one, not a second value shape for
/// the rest of the compiler to learn about.
///
/// `args` are already-evaluated argument literals, in `decl.inputs` order —
/// the caller (an `eval_expr` call site resolving a call through
/// `ConstCtx::lookup_mod`, or a nested call inside this same interpreter)
/// evaluates each argument expression against ITS OWN scope before calling
/// in.
///
/// The callee's environment is `cx.module_consts` — the MODULE-level
/// constants — with a parameter frame written ON TOP, so a same-named
/// parameter shadows a module constant. It is deliberately NOT built from
/// `cx.consts`: that map is the module env already flattened with the
/// CALLER's open scope frames, so seeding from it would leak the caller's
/// scope-local bindings into the callee. This mirrors `lower::call::inline`,
/// which pushes a fresh `ScopeTag::MODULE` frame for an ordinary mod's
/// parameters on top of the module scope rather than the call site's — so a
/// `const mod` body resolves exactly the names an ordinary `mod` body would,
/// no more and no less.
///
/// `cx.lookup_mod` is threaded through unchanged so a nested call inside the
/// body can resolve ITS callee the same way.
pub fn eval_call(
    decl: &ChipDecl,
    args: &[Literal],
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Result<Literal, ConstError> {
    if budget.depth == 0 {
        return Err(ConstError { reason: ConstReason::BudgetExceeded, range: decl.range.clone() });
    }
    budget.depth -= 1;

    let mut env: ConstEnv = (*cx.module_consts).clone();
    for (i, param) in decl.inputs.iter().enumerate() {
        // Zipping `inputs` with `args` would silently truncate on an arity
        // mismatch, and the unbound parameter would then surface as the
        // flatly untrue "'x' is a runtime value" from the first expression
        // that reads it. Blame the parameter with no argument instead — the
        // same error `bind_call_args` produces for a nested call.
        let Some(arg) = args.get(i) else {
            return Err(ConstError {
                reason: ConstReason::Unsupported("a call missing an argument for this parameter"),
                range: param.range.clone(),
            });
        };
        env.insert(param.name.clone(), arg.clone());
    }
    let mut call_cx = ConstCtx {
        consts: std::sync::Arc::new(env),
        module_consts: cx.module_consts.clone(),
        enum_defs: cx.enum_defs.clone(),
        lookup_mod: cx.lookup_mod,
    };

    // Collected OUTSIDE `call_cx`/`cx.consts` on purpose — see `exec_block`'s
    // doc comment on `Shadowed`. A taken `if` branch's own environment gets
    // merged back and then partially undone (its own declarations rewound);
    // an `out` collected DURING that branch must survive the undo, which it
    // can only do by living somewhere the undo never touches.
    let mut outputs: Vec<(String, Literal)> = Vec::new();
    let sig = OutputSig {
        declared: &decl.outputs,
        earliest_out: earliest_declared_out(&decl.body, &decl.outputs),
    };
    let flow = exec_block(&decl.body, &mut call_cx, &sig, &mut outputs, budget)?.0;
    match flow {
        // A `return <value>` wins outright — and cannot disagree with
        // lowering about it, because the `Stmt::Return` arm has already
        // rejected both ways that could happen: a value returned past an
        // earlier `out` (lowering would wire that `out` instead), and a
        // SCALAR value in a 2+-output mod (lowering wires nothing at all).
        Flow::Returned { value: Some(value), .. } => Ok(reorder_to_declaration(value, &decl.outputs)),
        // Either the body ran off the end or a `return` produced no value —
        // both mean "the result, if any, is whatever the `out` statements
        // collected".
        Flow::Returned { value: None, range } => {
            build_or_else(&decl.outputs, &outputs, || ConstError {
                // A body that DID hit a `return` must not be described as
                // falling off the end. The arrow form synthesises a `_`
                // output, so this message would otherwise be unreachable for
                // every `-> T` mod.
                reason: ConstReason::Unsupported("a `return` with no value"),
                range: range.clone(),
            })
        }
        Flow::Fell => build_or_else(&decl.outputs, &outputs, || ConstError {
            reason: ConstReason::Unsupported(
                "a const mod body that falls off the end without a `return`",
            ),
            range: decl.body.range.clone(),
        }),
    }
}

/// Build the `out`-collected result, or produce `empty` when there is nothing
/// to build from.
///
/// The fallback fires when the mod declares no outputs (nothing to build) or
/// when the body assigned none of them. That second case keeps the
/// pre-existing message and range for the overwhelmingly common no-`return`
/// mistake: without it, `-> int { }` would blame the anonymous `_` output
/// that `parse_chip_outputs` synthesises for the `-> type` form as "never
/// assigned", which points at the return type instead of the body.
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

/// Put a returned RECORD's fields into signature-declaration order, so the
/// `return { … }` path and the `out` path produce identically-ordered records
/// for the same signature. Fields the signature does not declare keep their
/// source order after the declared ones, and a missing field is simply absent
/// — this only ever REORDERS, never adds, drops, or rejects, so it cannot
/// turn a working record return into an error.
///
/// Not observable through `==` today (comparing two constant records is
/// WS047-refused), but a needless ordering difference between two paths that
/// build the same thing is exactly the kind of drift that becomes a bug the
/// moment something starts comparing or serialising these.
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

/// Shapes a `const mod`'s `out`-collected value the way `typecheck::call`
/// types the very same call, which is the whole correctness condition here:
///
/// - **exactly one** declared output → that output's value, BARE. This
///   mirrors `type_user_symbol_call`'s
///   `if sig.outputs.len() == 1 && !has_exec_arg { return out_ty(&sig.outputs[0].ty) }`.
///   Wrapping it in a 1-field record instead made const evaluation disagree
///   with the type system about a value's very shape, which surfaced three
///   different ways — a silently-zeroed baked array element, a WS028 that
///   called a `string` a record, and an outright `UnrepresentableLiteral`
///   emit abort.
///
///   Only the ARITY half of that condition is mirrored; `has_exec_arg` is
///   NOT, and that is a known (benign, today) disagreement rather than an
///   irrelevance. `bind_call_args` binds by walking `inputs`, silently
///   ignoring a named argument that matches no parameter, so
///   `const p = two(41, exec = go)` really does const-evaluate and bake —
///   measured. Const evaluation then yields `Record([r, s])` while
///   `typecheck::call` types that same call `Record([r, s, exec])`, having
///   appended the completion exec. Nothing reads `.exec` off a compile-time
///   constant (it is an exec signal, not a value), so the extra field is
///   unreachable rather than wrong; mirroring it here would mean inventing a
///   `Literal` for an exec edge, which has no constant form.

/// - **several** → a `Literal::Record`, built by walking `outs` in
///   DECLARATION order, deliberately NOT the order the body's `out`
///   statements ran in. `Literal::Record`'s field order is significant (it is
///   what a destructure and an equality check both see), so tying it to the
///   signature is the only order a caller can rely on —
///   `a_multi_output_const_mod_record_is_in_declaration_order` pins this with
///   a body that assigns its outputs backwards.
///
/// Either way, an output the body never assigned is
/// `ConstReason::Unsupported` blamed on that OUTPUT's own
/// `NamedOutput::range` — not the call, not the whole body — so the
/// diagnostic underlines exactly the unassigned output in the signature.
/// `collected` can only hold DECLARED names (`exec_block`'s
/// `Stmt::OutBinding` arm rejects any other), so nothing collected is ever
/// silently dropped on the floor here.
fn build_output_value(
    outs: &[NamedOutput],
    collected: &[(String, Literal)],
) -> Result<Literal, ConstError> {
    let mut fields = Vec::with_capacity(outs.len());
    for out in outs {
        let Some((_, value)) = collected.iter().find(|(n, _)| *n == out.name) else {
            return Err(ConstError {
                reason: ConstReason::UnsupportedMessage(
                    "this const mod body never assigns a value to this output",
                ),
                range: out.range.clone(),
            });
        };
        fields.push((out.name.clone(), value.clone()));
    }
    if let [(_, only)] = fields.as_slice() {
        return Ok(only.clone());
    }
    Ok(Literal::Record(fields))
}

/// What a name held immediately BEFORE a `let`/`const` in this block
/// re-bound it: `Some(lit)` when an enclosing scope already bound it (the
/// declaration shadows that value), `None` when the name was previously
/// unbound entirely (the declaration introduces it). Restoring this exact
/// snapshot when a branch ends is what confines a branch-local declaration —
/// and only the declaration — to the branch.
type Shadowed = Vec<(String, Option<Literal>)>;

/// How a block stopped running.
///
/// The two `Returned` payloads are NOT interchangeable with `Fell`, which is
/// why this is an enum rather than the `Option<Literal>` it replaced: a BARE
/// `return` (legal in the `out`-form, where the value comes from the `out`
/// statements rather than the `return`) also produces no value, so as an
/// `Option` it was indistinguishable from falling off the end — and the
/// `Stmt::If` arm, which propagates a return out of a branch by testing for
/// a value, would have let a bare `return` inside an `if` silently continue
/// executing the REST of the enclosing body.
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

/// The facts about the enclosing `const mod`'s SIGNATURE that individual
/// statement arms need. Computed once per [`eval_call`] and threaded down
/// unchanged, because both consumers reason about the whole body rather than
/// the block they appear in.
struct OutputSig<'a> {
    /// `decl.outputs` — the signature's declared outputs, in declaration
    /// order.
    declared: &'a [NamedOutput],
    /// Source offset of the EARLIEST `out` statement anywhere in the body
    /// that targets a DECLARED output, or `None` if there is none.
    ///
    /// This exists to make `Stmt::Return` agree with lowering about which
    /// assignment to an output port wins. Lowering wires the FIRST one in
    /// SOURCE order and drops the rest; const evaluation short-circuits at a
    /// `return`, so the two disagree exactly when an `out` precedes a valued
    /// `return` — verified in game-shaped output: `out r = 111` then
    /// `if n > 0 { return 222 }` bakes 222 as a `const mod` and wires 111 as
    /// a plain `mod`. Comparing source offsets is what catches that
    /// regardless of whether the `out` actually RAN, which a
    /// "was anything collected yet" test cannot do — an `out` inside an
    /// UNTAKEN branch still lowers, still wins the port, and still diverges.
    earliest_out: Option<usize>,
}

/// Source offset of the earliest `out` in `block` (recursively, including
/// both arms of every nested `if`) that targets one of `declared`. See
/// [`OutputSig::earliest_out`] for why this is a SYNTACTIC scan rather than a
/// record of what executed.
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

/// Runs `block`'s statements against `cx`, mutating it with every
/// `let`/`const` binding the block introduces, and every
/// `t.push(…)`-shaped mutation of an already-bound const array/map (see
/// [`exec_call_stmt`]). Returns `Ok((Flow::Returned(..), shadowed))` the
/// moment a `return` is hit (short-circuiting the rest of the block, exactly
/// like a real `return`), `Ok((Flow::Fell, shadowed))` if every statement ran
/// without one, or `Err` naming the specific statement (or the
/// sub-expression inside it) that could not be evaluated.
///
/// `shadowed` records, per top-level `let`/`const` in `block` and IN
/// DECLARATION ORDER, what the bound name held at the moment that
/// declaration ran (see [`Shadowed`]). It exists purely for the `Stmt::If`
/// arm's own caller — `exec_block` writes straight into whatever `cx` it is
/// given, so scoping a branch is entirely the CALLER's job: clone `cx`
/// before recursing, merge everything back, then undo exactly the
/// declarations by replaying these snapshots in REVERSE.
///
/// Nested `if`s inside `block` do NOT contribute here: such a branch has
/// already undone its own declarations against `cx` before its `Stmt::If`
/// arm returns, so by the time this function returns there is nothing left
/// of them to undo.
///
/// `outputs` accumulates every `out name = expr` this call's body statements
/// run (see the `Stmt::OutBinding` arm below), across every nested block —
/// it is the SAME `Vec` passed all the way down through a taken `if`
/// branch's own recursive `exec_block` call, never a fresh one. That is
/// deliberate and load-bearing: it must NOT live in `cx.consts`, because the
/// `Stmt::If` arm undoes exactly the branch's own `let`/`const` declarations
/// against `cx` when the branch ends (see this comment's own discussion of
/// `shadowed` above), and an `out` inside a taken branch has to survive that
/// undo — `eval_call` still needs it once this whole call returns.
/// `outputs` living outside `cx` entirely is what makes that automatic: the
/// undo never touches it, so there is nothing extra to do for the branch
/// case (an `out` inside an untaken branch simply never runs, exactly like
/// every other statement in an untaken branch). Pinned by
/// `an_out_inside_a_taken_branch_is_collected`, which fails if that
/// recursive call is handed a fresh `Vec` instead.
///
/// `sig` carries the enclosing `const mod`'s signature facts (see
/// [`OutputSig`]) purely so the `Stmt::Return` arm can reject the two shapes
/// where a returned value would disagree with what lowering wires. It needs
/// the DECLARATION, not the collected values, so neither check can be made in
/// `eval_call` after the fact without losing the offending statement's own
/// range.
fn exec_block(
    block: &Block,
    cx: &mut ConstCtx,
    sig: &OutputSig,
    outputs: &mut Vec<(String, Literal)>,
    budget: &mut Budget,
) -> Result<(Flow, Shadowed), ConstError> {
    let mut shadowed: Shadowed = Vec::new();
    for stmt in &block.stmts {
        if budget.steps == 0 {
            return Err(ConstError { reason: ConstReason::BudgetExceeded, range: stmt.range().clone() });
        }
        budget.steps -= 1;

        match stmt {
            Stmt::Let(l) => {
                // Works uniformly for `const x = …` and a plain `let x = …`:
                // there is no "fall back to a gate" path in this
                // interpreter, so a plain `let` inside a const-evaluated
                // body must ALSO have a constant initializer, and the error
                // eval_expr produces already blames the right sub-expression.
                let lit = eval_expr(&l.value, cx, budget)?;
                // Splits `lit` into every `(name, value)` pair `l.binding`
                // introduces — one pair for a plain `Ident`, several for a
                // record destructure (`bind_destructured` propagates its own
                // error, e.g. a tuple destructure or a missing field).
                let pairs = bind_destructured(&l.binding, lit)?;
                // A destructure binds SEVERAL names from ONE statement, and
                // each needs its OWN shadow snapshot taken immediately
                // before ITS OWN insert — not one snapshot for the whole
                // statement. One snapshot would remember only what a SINGLE
                // name held, so undoing a branch-local destructure on branch
                // exit (see this function's doc comment) would restore that
                // one name and leave every OTHER name the same destructure
                // bound (or clobbered) lingering past the branch.
                for (name, value) in pairs {
                    // Snapshot BEFORE the insert, and only ever record what
                    // the name held AT THIS MOMENT — which, for a name
                    // mutated earlier in this same block, is the
                    // already-mutated value. That is the whole point:
                    // restoring it later preserves everything done to the
                    // outer binding before the shadow appeared, while still
                    // dropping the shadow itself.
                    shadowed.push((name.clone(), cx.consts.get(name.as_str()).cloned()));
                    std::sync::Arc::make_mut(&mut cx.consts).insert(name, value);
                }
            }
            Stmt::Return { value, range } => {
                let Some(e) = value else {
                    // A BARE `return` is the natural early exit for the
                    // `out` form — the value comes from the `out`
                    // statements, so there is nothing to return — and it
                    // stops the block so `eval_call` can build from whatever
                    // was collected.
                    return Ok((
                        Flow::Returned { value: None, range: range.clone() },
                        shadowed,
                    ));
                };
                // An `out` EARLIER IN THE SOURCE than this `return` wins the
                // output port in lowering, which wires the first assignment
                // and drops the rest — while const evaluation short-circuits
                // here and hands back the returned value instead. That is a
                // straight const-vs-runtime value disagreement, measured:
                // `out r = 111` then `if n > 0 { return 222 }` bakes 222 as a
                // `const mod` and wires 111 as a plain `mod`, both reporting
                // no errors.
                //
                // Checked POSITIONALLY (and before evaluating `e`) rather
                // than by asking whether an `out` has run yet: an `out` in an
                // UNTAKEN branch never runs, yet still lowers and still wins
                // the port, so a "has anything been collected" test would
                // miss it and let the same divergence through. The converse
                // ordering — `return` first, `out` after — genuinely AGREES
                // (both sides yield the returned value, also measured), so it
                // stays legal.
                if let Some(out_offset) = sig.earliest_out
                    && out_offset < range.start.offset
                {
                    return Err(ConstError {
                        reason: ConstReason::UnsupportedMessage(
                            "this `return` of a value follows an earlier `out` in the same \
                             const mod — the wire graph keeps that first assignment and drops \
                             this one, so the two would disagree",
                        ),
                        range: range.clone(),
                    });
                }
                let lit = eval_expr(e, cx, budget)?;
                // A SCALAR `return` in a mod declaring SEVERAL named outputs
                // has no meaning anywhere: `lower::stmt`'s `Stmt::Return`
                // arm wires a plain returned value only through
                // `output_count() == 1`, so with 2+ outputs lowering
                // silently drops it. Letting it win outright here would hand
                // every consumer a bare scalar where the type system
                // promised a record: a field read off that scalar (`c.a`)
                // would lower to a `SplitColor` gate, reinterpreting the
                // field as a colour channel with no diagnostic at all.
                //
                // A RECORD return is exempt because it IS the documented
                // multi-output mechanism: the same `lower::stmt` arm stashes
                // a `RecordLit` as `pending_return_record` and forwards it
                // per-field. One declared output is exempt too — that is
                // exactly the `output_count() == 1` case lowering wires, and
                // it covers every ordinary `-> int { return x }` mod.
                if sig.declared.len() > 1 && !matches!(lit, Literal::Record(_)) {
                    return Err(ConstError {
                        reason: ConstReason::UnsupportedMessage(
                            "this const mod declares several named outputs, so a `return` of a \
                             single value has nowhere to go — assign each output with \
                             `out name = …`, or return a record",
                        ),
                        range: range.clone(),
                    });
                }
                return Ok((
                    Flow::Returned { value: Some(lit), range: range.clone() },
                    shadowed,
                ));
            }
            Stmt::If(if_stmt) => {
                let cond = eval_expr(&if_stmt.cond, cx, budget)?;
                let Literal::Bool(take_then) = cond else {
                    return Err(ConstError {
                        reason: ConstReason::Unsupported(
                            "an `if` condition that is not a compile-time bool",
                        ),
                        range: if_stmt.cond.range().clone(),
                    });
                };
                // Evaluate the condition, then recurse into the TAKEN block
                // only — on a private copy of the environment, so a `const`
                // the branch declares does not leak past the `if`, and the
                // UNTAKEN block is never touched (does not even need to be
                // evaluable).
                let branch = if take_then { Some(&if_stmt.then_block) } else { if_stmt.else_block.as_ref() };
                if let Some(taken) = branch {
                    let mut branch_cx = ConstCtx {
                        consts: cx.consts.clone(),
                        module_consts: cx.module_consts.clone(),
                        enum_defs: cx.enum_defs.clone(),
                        lookup_mod: cx.lookup_mod,
                    };
                    let (result, branch_shadowed) =
                        exec_block(taken, &mut branch_cx, sig, outputs, budget)?;
                    // Take the branch's environment WHOLESALE — every
                    // mutation it made to a binding from an enclosing scope
                    // must survive (`if cond { t.push(x) }` has to leave `t`
                    // changed, exactly like a real `if` would) — and then
                    // undo precisely the branch's own DECLARATIONS by
                    // replaying their snapshots.
                    //
                    // Replayed in REVERSE declaration order so that when one
                    // name is declared more than once in the branch, the
                    // EARLIEST snapshot (the true pre-branch value) is
                    // applied last and therefore wins.
                    //
                    // Restoring a snapshot rather than skipping the name is
                    // what makes a shadow declared AFTER a mutation correct:
                    // `t.push(2)` then `const t = [9]` restores `t` to the
                    // ALREADY-PUSHED `[1, 2]`, not to the pre-branch `[1]`.
                    // Skipping every declared name (this code's first form)
                    // silently discarded that earlier mutation — see
                    // `a_shadow_declared_after_a_mutation_keeps_the_mutation`
                    // in `tests.rs`.
                    cx.consts = branch_cx.consts;
                    for (name, prev) in branch_shadowed.into_iter().rev() {
                        match prev {
                            // Previously bound in an enclosing scope: put
                            // that value back (mutations included).
                            Some(value) => {
                                std::sync::Arc::make_mut(&mut cx.consts).insert(name, value)
                            }
                            // Previously unbound: the declaration introduced
                            // the name, so it must vanish entirely rather
                            // than linger as an empty/default value.
                            None => {
                                std::sync::Arc::make_mut(&mut cx.consts).remove(name.as_str())
                            }
                        };
                    }
                    // A `return` inside the branch stops the ENCLOSING body
                    // too, bare or valued alike — matching on `Flow` rather
                    // than on "did it produce a value" is what makes the
                    // bare case propagate instead of silently running on.
                    if let Flow::Returned { value, range } = result {
                        return Ok((Flow::Returned { value, range }, shadowed));
                    }
                }
            }
            // `out name = expr`: a multi-output `const mod`'s way of setting
            // one of its `decl.outputs` (the `return`-less alternative to
            // `return { a: .., b: .. }` — see `eval_call`'s doc comment).
            // Evaluated and recorded under `name` in `outputs`, OVERWRITING
            // an earlier assignment to the same name — mirroring what a real
            // `out` port does when written more than once (last write
            // wins) — so `eval_call` can assemble the final record once the
            // whole body has run. A bare `out name` with no value (legal for
            // a physical output PORT, which can be left unwired) has nothing
            // to evaluate, so it is `Unsupported`, the same shape
            // `Stmt::Return`'s own no-value case already refuses above.
            Stmt::OutBinding(o) => {
                // An `out` naming something the SIGNATURE does not declare is
                // deliberately NOT rejected: declaring an output in the body
                // is a supported form (`lower/mod.rs` and
                // `lower/call/instance_body.rs` both route it to
                // `pre_declare_output`), so the identical body compiles clean
                // as a plain `mod`. It is also not part of this call's RESULT
                // — `typecheck::call` types a signature-bearing mod's call
                // from the signature alone, reporting
                // `no field 'zz' on record (has: a, b)` for a body-declared
                // output — so evaluating it and letting `build_output_value`
                // (which walks the signature) ignore it is exactly the shape
                // typecheck already promises. There is no signal that
                // distinguishes a typo from an intentional body-declared
                // output, so this cannot be narrowed to "typos only".
                let Some(value_expr) = &o.value else {
                    return Err(ConstError {
                        reason: ConstReason::Unsupported("an `out` with no value"),
                        range: o.range.clone(),
                    });
                };
                let lit = eval_expr(value_expr, cx, budget)?;
                match outputs.iter_mut().find(|(n, _)| *n == o.name) {
                    Some(slot) => slot.1 = lit,
                    None => outputs.push((o.name.clone(), lit)),
                }
            }
            Stmt::ExprStmt(es) => exec_call_stmt(es, cx, budget)?,
            other => {
                return Err(ConstError {
                    reason: ConstReason::Unsupported("this statement"),
                    range: other.range().clone(),
                });
            }
        }
    }
    Ok((Flow::Fell, shadowed))
}

/// `Stmt::ExprStmt` is meaningful here in two shapes: a call to another
/// `const mod` (its result discarded — there is no other observable effect a
/// pure compile-time interpreter could have), or a mutating method call
/// (`t.push(…)`, `t.set(…)`, …) on a LOCAL const array/map binding, handled
/// by [`exec_mutating_method_call`]. Anything else — not a call, an
/// unresolvable callee, or a call to a non-`const` mod — is
/// `ConstReason::Unsupported("this statement")`, blamed on the statement
/// itself via `es.range`.
fn exec_call_stmt(es: &ExprStmt, cx: &mut ConstCtx, budget: &mut Budget) -> Result<(), ConstError> {
    let unsupported = || ConstError {
        reason: ConstReason::Unsupported("this statement"),
        range: es.range.clone(),
    };

    let Expr::Call { callee, args, .. } = &es.expr else {
        return Err(unsupported());
    };

    // A method-call callee (`t.push(10)`) is never the plain-`Ident` shape a
    // nested const-mod call has, so it's checked first and handled entirely
    // separately.
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
        return Err(ConstError { reason: ConstReason::BudgetExceeded, range: es.range.clone() });
    }
    eval_call(&target, &arg_lits, &*cx, budget)?;
    Ok(())
}

/// `t.push(…)` / `t.set(…)` / … — a mutating method call whose receiver `obj`
/// names a LOCAL const binding. The mutation is applied to a fresh owned
/// copy of the current value and written back into `cx.consts` under the
/// same name — there is no other storage a `const` binding has, which is
/// exactly why this can only ever run here: [`exec_call_stmt`] is reachable
/// only from `exec_block`, which runs only inside [`eval_call`] evaluating a
/// `const mod` body. No other stage of the compiler calls into this
/// interpreter, so a const collection cannot be mutated anywhere outside a
/// const mod body — not because of a check, but because nothing else knows
/// how to interpret `.push()` as a write.
///
/// `obj` must be a plain local name (not a nested field/index/call) — the
/// write-back is by name, so there is nothing to write an arbitrary
/// expression's result back into — and that name must already be bound to a
/// `Literal::Array`/`Literal::Map` in `cx.consts`. Anything else (an
/// unresolved name, a non-collection value, a non-Ident receiver) falls
/// through to the same generic "this statement" error every other
/// unsupported `ExprStmt` gets, since it isn't a const-collection mutation
/// at all.
fn exec_mutating_method_call(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    es: &ExprStmt,
    cx: &mut ConstCtx,
    budget: &mut Budget,
) -> Result<(), ConstError> {
    let unsupported = || ConstError {
        reason: ConstReason::Unsupported("this statement"),
        range: es.range.clone(),
    };

    let Expr::Ident { name, .. } = obj else {
        return Err(unsupported());
    };
    let Some(current) = cx.consts.get(name.as_str()).cloned() else {
        return Err(unsupported());
    };

    let updated = match current {
        Literal::Array(items) => Literal::Array(mutate_array(items, field, args, es, &*cx, budget)?),
        Literal::Map(pairs) => Literal::Map(mutate_map(pairs, field, args, es, &*cx, budget)?),
        _ => return Err(unsupported()),
    };
    std::sync::Arc::make_mut(&mut cx.consts).insert(name.clone(), updated);
    Ok(())
}

/// Supports `push`, `set`, `clear`, `append` — mirroring the runtime array
/// gates (`ARRAY_PUSH`/`ARRAY_SET_AT_INDEX`/`ARRAY_CLEAR`/`ARRAY_APPEND`)
/// closely enough to read the same, even though `set` has no call-syntax
/// counterpart at runtime (the real language spells element assignment
/// `arr[i] = value`, which lowers via `Stmt::Assign` — a `const` binding has
/// no gate for that statement to target, so `set` is offered as a method
/// here instead). Any other method name is
/// `ConstReason::UnsupportedMethod`, naming the method so the user learns
/// which call is the problem rather than reading a generic "not constant".
fn mutate_array(
    mut items: Vec<Literal>,
    field: &str,
    args: &[CallArg],
    es: &ExprStmt,
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Result<Vec<Literal>, ConstError> {
    match field {
        "push" => {
            let value = positional_arg(args, 0).ok_or_else(|| arity_error(es))?;
            items.push(eval_expr(value, cx, budget)?);
            Ok(items)
        }
        "set" => {
            let index = positional_arg(args, 0).ok_or_else(|| arity_error(es))?;
            let value = positional_arg(args, 1).ok_or_else(|| arity_error(es))?;
            let idx_lit = eval_expr(index, cx, budget)?;
            let val_lit = eval_expr(value, cx, budget)?;
            match idx_lit {
                Literal::Int(i) if i >= 0 && (i as usize) < items.len() => {
                    items[i as usize] = val_lit;
                    Ok(items)
                }
                Literal::Int(i) => Err(ConstError {
                    reason: ConstReason::ArrayIndexOutOfRange { index: i, len: items.len() },
                    range: es.range.clone(),
                }),
                _ => Err(ConstError {
                    reason: ConstReason::Unsupported("a non-integer array index"),
                    range: index.range().clone(),
                }),
            }
        }
        "clear" => {
            items.clear();
            Ok(items)
        }
        "append" => {
            let source = positional_arg(args, 0).ok_or_else(|| arity_error(es))?;
            match eval_expr(source, cx, budget)? {
                Literal::Array(mut more) => {
                    items.append(&mut more);
                    Ok(items)
                }
                _ => Err(ConstError {
                    reason: ConstReason::Unsupported("appending a non-array value"),
                    range: source.range().clone(),
                }),
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

/// Supports `set`, `remove`, `clear` — mirroring the runtime map gates
/// (`MAP_SET`/`MAP_REMOVE`/`MAP_CLEAR`) 1:1 (unlike arrays, a map's `set` IS
/// the real call-syntax method — `lower::access::lower_map_method`'s `"set"`
/// arm lowers the exact same `t.set(k, v)` shape to `MAP_SET`). Any other
/// method name is `ConstReason::UnsupportedMethod`, naming the method.
fn mutate_map(
    mut pairs: Vec<(Literal, Literal)>,
    field: &str,
    args: &[CallArg],
    es: &ExprStmt,
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Result<Vec<(Literal, Literal)>, ConstError> {
    match field {
        "set" => {
            let key = positional_arg(args, 0).ok_or_else(|| arity_error(es))?;
            let value = positional_arg(args, 1).ok_or_else(|| arity_error(es))?;
            let key_lit = eval_expr(key, cx, budget)?;
            let val_lit = eval_expr(value, cx, budget)?;
            match pairs.iter_mut().find(|(k, _)| *k == key_lit) {
                Some(entry) => entry.1 = val_lit,
                None => pairs.push((key_lit, val_lit)),
            }
            Ok(pairs)
        }
        "remove" => {
            let key = positional_arg(args, 0).ok_or_else(|| arity_error(es))?;
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

/// The `i`th POSITIONAL argument only — a mutating method call has no named
/// or optional parameters, so this is simpler than `bind_call_args`'s
/// name-based binding (which exists for user mod/chip parameters, not for
/// these fixed built-in shapes).
fn positional_arg(args: &[CallArg], i: usize) -> Option<&Expr> {
    args.iter()
        .filter_map(|a| match a {
            CallArg::Positional(v) => Some(v),
            _ => None,
        })
        .nth(i)
}

/// A mutating method call missing a required positional argument (typecheck
/// does not arity-check these — they are not real `catalog` call specs, see
/// `mutate_array`'s doc comment on `set`), blamed on the whole statement
/// since no single argument sub-expression is at fault.
fn arity_error(es: &ExprStmt) -> ConstError {
    ConstError {
        reason: ConstReason::Unsupported("a call with the wrong number of arguments"),
        range: es.range.clone(),
    }
}

/// A method name that is not one of the mutations this evaluator implements
/// for `kind` ("array"/"map"), naming the actual method so the user learns
/// which call is the problem instead of reading a generic "not constant".
fn unsupported_collection_method(
    field: &str,
    kind: &'static str,
    supported: &'static str,
    es: &ExprStmt,
) -> ConstError {
    ConstError {
        reason: ConstReason::UnsupportedMethod(format!(
            "`.{field}(…)` is not a supported compile-time {kind} mutation — only {supported} are"
        )),
        range: es.range.clone(),
    }
}

/// Binds `args` to `inputs` by name (positional args fill left-to-right,
/// named args target a parameter by name — the same convention
/// `eval_method_call` in `expr.rs` uses for builtin receiver calls), then
/// evaluates each bound expression against the CALLER's `cx` before handing
/// the resulting literals to `eval_call`, which seeds a fresh environment
/// from them. An unbound parameter (arity mismatch — typecheck's WS022
/// normally prevents this before const evaluation ever runs) is
/// `Unsupported`, blamed on the missing parameter's own declaration.
///
/// `budget` is threaded through to `eval_expr` (rather than each argument
/// getting its own fresh one) so a call-chain reached from ARGUMENT
/// evaluation counts against the SAME depth/step ceiling as the call it's
/// an argument to — otherwise a self-referential or mutually-recursive
/// `const mod` reached this way would reset its budget every level and
/// overflow the native Rust stack instead of hitting `BudgetExceeded`.
pub(super) fn bind_call_args(
    inputs: &[Param],
    args: &[CallArg],
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Result<Vec<Literal>, ConstError> {
    let mut bound: HashMap<&str, &Expr> = HashMap::default();
    let mut next_pos = 0usize;
    for a in args {
        match a {
            CallArg::Positional(v) => {
                if let Some(p) = inputs.get(next_pos) {
                    bound.insert(p.name.as_str(), v);
                }
                next_pos += 1;
            }
            CallArg::Named { name, value, .. } => {
                bound.insert(name.as_str(), value);
            }
            CallArg::Spread(_) => {} // no constant form; unbound parameter below is Unsupported
        }
    }

    let mut out = Vec::with_capacity(inputs.len());
    for p in inputs {
        let Some(&expr) = bound.get(p.name.as_str()) else {
            return Err(ConstError {
                reason: ConstReason::Unsupported("a call missing an argument for this parameter"),
                range: p.range.clone(),
            });
        };
        out.push(eval_expr(expr, cx, budget)?);
    }
    Ok(out)
}
