//! Compile-time expression evaluation.
//!
//! Phase 1 delegates to `lower::expr_to_literal_in`, which already evaluates
//! literals, named constants, operators and the `Vec`/`Rotation`/`Color`
//! constructors. This wrapper adds what `const` needs and `let` did not: a
//! REASON when evaluation fails, so the diagnostic can name the offending
//! value instead of silently falling back to gates.

use std::sync::Arc;

use crate::ast::{ArrayElem, CallArg, ChipDecl, Expr, InterpPart, RecordLitField};
use crate::collections::HashMap;
use crate::diagnostic::SourceRange;
use crate::ir::Literal;
use crate::lower::fold::eval::{self as fold_eval, Value};
use crate::lower::{
    eval_const_binop, eval_const_unop, expr_to_literal_in, fold_constructor, ConstEnv,
};

use super::error::{ConstError, ConstReason};
use super::interp::Budget;

// NOTE: two evaluators, two laws. `expr_to_literal_in` computes mixed-sign
// integer division and maps a non-finite result to 0; `fold::eval` REFUSES
// both, because the in-game probe never certified them. Existing surfaces stay
// on `expr_to_literal_in` so array initializers that bake today keep baking;
// everything added here (string interpolation, certified method calls) goes
// through the certified evaluator, whose refusals become WS047
// (`ConstReason::Refused`). Do not "unify" these without re-certifying — see
// `both_evaluators_agree_where_both_are_defined` in `tests.rs`, which is the
// only thing keeping this seam safe.

/// Call names `expr_to_literal_in` folds itself instead of treating as a mod
/// call. Source of truth: `lower::predeclare::fold_constructor`, shared by
/// `expr_to_literal_lit`'s syntactic folding and this file's own constructor
/// arm (see the `Expr::Call` arm of `eval_expr`). Every name here must ALSO be
/// a `catalog::calls` entry, since [`bind_constructor_args`] binds arguments
/// through that catalog's parameter list. `ColorSRGB` is deliberately absent —
/// it folds via a separate path (`fold_srgb_color`) that const evaluation
/// never reaches.
/// Blaming one of these on "not declared `const mod`" would be a lie: they are
/// builtins, so a failure with constant arguments is the constructor declining
/// them. `foldable_constructors_are_all_really_foldable` guards against drift.
pub(super) const FOLDABLE_CONSTRUCTORS: &[&str] = &["Vec", "Rotation", "Color"];

/// The three refusal messages this file produces from TWO places each — once
/// where `eval_expr` actually declines a value, and once where `reason_for`
/// explains a failure it did not itself evaluate. Sharing the text keeps a
/// user-visible message from drifting between the two paths for the same
/// cause.
fn binop_refused(op: &str) -> String {
    format!("the `{op}` operator has no certified constant result for these operands")
}

fn unop_refused(op: &str) -> String {
    format!("the `{op}` operator has no certified constant result for this operand")
}

fn constructor_refused(name: &str) -> String {
    format!("`{name}` has no constant form for these arguments")
}

/// Everything const evaluation needs to resolve an expression.
pub struct ConstCtx<'a> {
    /// Every constant visible AT THE CURRENT POINT: the module-level
    /// constants overlaid by every currently-open scope frame (built by
    /// `TypeCheckCtx::const_lookup` / `LowerCtx::const_lookup`). This is what
    /// an ordinary expression resolves names against.
    pub consts: ConstEnv,
    /// The MODULE-LEVEL constants ONLY — the top-level `const`/`let`
    /// bindings, with no scope-local frames merged in. Kept separate from
    /// `consts` because a `const mod` BODY (see `interp::eval_call`) is
    /// evaluated against the module's constants plus its own parameters, and
    /// must NOT see the caller's scope-local bindings — a mod body is its own
    /// scope, so a caller's local `const` is simply not in it. Mirrors how
    /// `lower::call::inline` lowers an ordinary mod: it pushes a fresh
    /// `ScopeTag::MODULE` frame holding the parameters ON TOP of the module
    /// scope rather than inheriting the call site's frames.
    pub module_consts: ConstEnv,
    /// Resolves a call's callee NAME to its declaration, so `eval_expr`'s
    /// `Expr::Call` arm can tell whether it's calling a `const mod` (and if
    /// so, get the `ChipDecl` `interp::eval_call` needs its body/params from).
    /// `None` resolves to "not found" — an undeclared name, or one that
    /// resolves to something other than a chip/mod — exactly like `NotAConstMod`
    /// would report for a plain non-const mod, so callers that never expect a
    /// call (`const_eval::tests`'s standalone expression probes) can pass
    /// `None` here with no other change in behavior.
    ///
    /// Built FRESH at every const-evaluating call site (`TypeCheckCtx::const_ctx`,
    /// `LowerCtx::const_ctx`) rather than stored on `ConstCtx` itself: a
    /// closure borrowing `self` can't be packaged into a value a `&self`
    /// method returns (the closure would have to outlive the call that builds
    /// it), so each site builds its own short-lived closure over `ctx` and
    /// passes it in alongside the rest of `ConstCtx`.
    pub lookup_mod: Option<&'a dyn Fn(&str) -> Option<Arc<ChipDecl>>>,
}

/// `budget` bounds a `const mod` CALL reached from anywhere inside `e` — a
/// direct call, one nested inside a method-call argument, or one inside a
/// string interpolation slot. It is threaded through every recursive
/// `eval_expr` call in this file (rather than each site making its own fresh
/// [`Budget`]) so a call-chain reached through argument evaluation counts
/// against the SAME ceiling as the call it's an argument to; a top-level
/// caller with no ambient budget (every `const_ctx()` call site) makes one
/// fresh [`Budget::default`] per outer `eval_expr` invocation.
pub fn eval_expr(e: &Expr, cx: &ConstCtx, budget: &mut Budget) -> Result<Literal, ConstError> {
    if let Some(lit) = expr_to_literal_in(e, &cx.consts) {
        return Ok(lit);
    }
    // New surface, certified evaluator (see the seam NOTE at the top of this
    // file): string interpolation always routes here — there is no other
    // form it could take — and a method call routes here only when its
    // callee resolves to a certified, non-exec receiver method; anything
    // else (an unknown method, a user self-receiver mod) falls through to
    // `reason_for` below. Array/map LITERALS, indexing into one, and
    // `.length()` on one are handled directly below — `expr_to_literal_in`
    // never folds these (baking them is `predeclare.rs`'s job, not this
    // evaluator's), so they always reach this match.
    match e {
        Expr::InterpLit { parts, range } => return eval_interp(parts, range, cx, budget),
        // `[a, b, ...c]`: every element must itself be constant. A spread has
        // no constant form (arrays only support spread as an exec-context
        // assignment RHS, never as a value in its own right), so it refuses
        // rather than silently dropping or inlining the spread source.
        Expr::Array { elements, .. } => {
            let mut items = Vec::with_capacity(elements.len());
            for el in elements {
                match el {
                    ArrayElem::Item(item) => items.push(eval_expr(item, cx, budget)?),
                    ArrayElem::Spread(_) => {
                        return Err(ConstError {
                            reason: ConstReason::Unsupported("a spread"),
                            range: el.range().clone(),
                        });
                    }
                }
            }
            return Ok(Literal::Array(items));
        }
        // `{ k => v, ... }`: both the key and the value of every entry must
        // be constant. Unlike `Expr::Array`, map literals have no spread
        // form at all, so there is no analogous refusal to make here.
        Expr::MapLit { entries, .. } => {
            let mut pairs = Vec::with_capacity(entries.len());
            for entry in entries {
                let key = eval_expr(&entry.key, cx, budget)?;
                let value = eval_expr(&entry.value, cx, budget)?;
                pairs.push((key, value));
            }
            return Ok(Literal::Map(pairs));
        }
        // `{ k: v, ...rest }`: a compile-time-only record — see
        // `ir::Literal::Record`'s doc comment for why it has no wire form.
        // Fields are evaluated and merged in SOURCE ORDER, matching
        // `typecheck::infer`'s `Type::Record` inference for the same syntax:
        // a `Named`/`Shorthand` field overwrites any earlier value under the
        // same key (from an earlier field or an earlier spread), and a
        // `Spread` merges in every field of its (record-valued) source,
        // itself overwritten by anything that follows it. A `Spread` whose
        // value isn't itself a record has no defined merge, so it refuses
        // rather than silently dropping or flattening non-record fields into
        // the result.
        Expr::RecordLit { fields, .. } => {
            let mut entries: Vec<(String, Literal)> = Vec::new();
            let upsert = |entries: &mut Vec<(String, Literal)>, name: String, value: Literal| {
                match entries.iter_mut().find(|(n, _)| *n == name) {
                    Some(existing) => existing.1 = value,
                    None => entries.push((name, value)),
                }
            };
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        let v = eval_expr(value, cx, budget)?;
                        upsert(&mut entries, name.clone(), v);
                    }
                    // `{ x }` is sugar for `{ x: x }` — resolve `name` as a
                    // plain identifier the same way the expanded form would.
                    RecordLitField::Shorthand { name, range } => {
                        let ident = Expr::Ident { name: name.clone(), range: range.clone() };
                        let v = eval_expr(&ident, cx, budget)?;
                        upsert(&mut entries, name.clone(), v);
                    }
                    RecordLitField::Spread { value, .. } => match eval_expr(value, cx, budget)? {
                        Literal::Record(spread) => {
                            for (k, v) in spread {
                                upsert(&mut entries, k, v);
                            }
                        }
                        _ => {
                            return Err(ConstError {
                                reason: ConstReason::Unsupported("spreading a non-record constant"),
                                range: value.range().clone(),
                            });
                        }
                    },
                }
            }
            return Ok(Literal::Record(entries));
        }
        // `rec.field`: a compile-time constant record's field. `obj` is
        // evaluated through the FULL `eval_expr` (not `expr_to_literal_in`),
        // so a `const mod` call standing as `obj` (`f().field`) still
        // resolves — unlike the compound forms `expr_to_literal_in` folds on
        // its own, this arm recurses back into this very function. A missing
        // field is `RecordFieldNotFound`, distinct from `NotConstant`: the
        // record itself evaluated fine, the name just isn't one of its
        // fields. Field access on anything that isn't a record (a runtime
        // binding, or a const scalar/array/map) is not this evaluator's
        // concern — runtime record field access is resolved entirely in
        // `lower::access` via scope bindings, never through `Literal`.
        Expr::FieldAccess { obj, field, range } => {
            // A namespaced constant member (`Other.value`, `Other` an
            // `import * as` alias): seeded into the env under `"Other.value"` by
            // `build_const_env`. Resolved before evaluating `obj`, which is a
            // namespace (not a value) and would otherwise fail as NotConstant.
            // Skipped when a local const of the same name shadows the alias, so
            // that record's field wins (matching runtime resolution).
            if let Expr::Ident { name: ns, .. } = obj.as_ref()
                && !cx.consts.contains_key(ns.as_str())
                && let Some(v) = cx.consts.get(&format!("{ns}.{field}"))
            {
                return Ok(v.clone());
            }
            return match eval_expr(obj, cx, budget)? {
                Literal::Record(entries) => match entries.into_iter().find(|(n, _)| n == field) {
                    Some((_, v)) => Ok(v),
                    None => Err(ConstError {
                        reason: ConstReason::RecordFieldNotFound(field.clone()),
                        range: range.clone(),
                    }),
                },
                _ => Err(ConstError {
                    reason: ConstReason::Unsupported("field access on this constant"),
                    range: obj.range().clone(),
                }),
            };
        }
        // `t[i]` / `m[k]` over a const array/map. Unlike a runtime array
        // read — which keeps the gate's stale PREVIOUS value when the index
        // is out of range, because there is always a prior tick to fall back
        // on — a compile-time evaluation has no previous value to offer, so
        // an out-of-range index or missing key is refused outright rather
        // than guessed at (see `ConstReason::ArrayIndexOutOfRange`/
        // `MapKeyNotFound`, both WS046 with a message that names the actual
        // problem instead of the generic "not a compile-time constant").
        Expr::IndexAccess { obj, index, range } => {
            let base = eval_expr(obj, cx, budget)?;
            let idx = eval_expr(index, cx, budget)?;
            return match base {
                Literal::Array(items) => match idx {
                    Literal::Int(i) if i >= 0 && (i as usize) < items.len() => {
                        Ok(items[i as usize].clone())
                    }
                    Literal::Int(i) => Err(ConstError {
                        reason: ConstReason::ArrayIndexOutOfRange {
                            index: i,
                            len: items.len(),
                        },
                        range: range.clone(),
                    }),
                    _ => Err(ConstError {
                        reason: ConstReason::Unsupported("a non-integer array index"),
                        range: index.range().clone(),
                    }),
                },
                Literal::Map(pairs) => match pairs.into_iter().find(|(k, _)| *k == idx) {
                    Some((_, v)) => Ok(v),
                    None => Err(ConstError {
                        reason: ConstReason::MapKeyNotFound,
                        range: range.clone(),
                    }),
                },
                _ => Err(ConstError {
                    reason: ConstReason::Unsupported("indexing this constant"),
                    range: obj.range().clone(),
                }),
            };
        }
        // `if <cond> then a else b`: evaluate `cond` and return ONLY the
        // taken arm's value — the untaken arm is never evaluated, so it may
        // reference runtime-only names or fail to evaluate on its own (that
        // is the whole point of a const `if` expression: it lets one
        // constant-folding site serve cases that don't share an evaluable
        // form). A failing `cond` propagates verbatim via `?`, same as every
        // other compound form in this file.
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            return match eval_expr(cond, cx, budget)? {
                Literal::Bool(true) => eval_expr(then_branch, cx, budget),
                Literal::Bool(false) => eval_expr(else_branch, cx, budget),
                _ => Err(ConstError {
                    reason: ConstReason::Unsupported("a non-boolean `if` condition"),
                    range: cond.range().clone(),
                }),
            };
        }
        // `a + b` where a part is a `const mod` call: `expr_to_literal_in`
        // folded neither operand, because it cannot see mod calls at all.
        // Recurse through THIS function (which can), then apply the exact
        // same literal->literal folding function `expr_to_literal_impl` uses
        // (`eval_const_binop`) — so the operator law is unchanged by
        // construction and the two-evaluator seam (see the NOTE at the top of
        // this file) is not crossed: this is still the SAME law, just fed an
        // operand `expr_to_literal_in` could not produce on its own.
        Expr::BinOp {
            op,
            left,
            right,
            range,
        } => {
            let l = eval_expr(left, cx, budget)?;
            let r = eval_expr(right, cx, budget)?;
            return eval_const_binop(op, l, r).ok_or_else(|| ConstError {
                reason: ConstReason::Refused(binop_refused(op)),
                range: range.clone(),
            });
        }
        // `-a` / `!a` / `~a` where `a` is a `const mod` call: same rationale
        // as `Expr::BinOp` above, via `eval_const_unop`. Unlike
        // `expr_to_literal_impl` (which special-cases `NEG` in a SEPARATE arm
        // purely so literal negation still works with no environment at all,
        // e.g. plain `expr_to_literal`), there is no such separate arm here —
        // `eval_expr` always has a real `ConstCtx` — so `eval_const_unop`
        // handles every unary operator uniformly, `NEG` included; its `NEG`
        // law is byte-identical to `expr_to_literal_impl`'s own, so this
        // cannot disagree with the fast path above, only reach further than
        // it (through a mod call the fast path can't see).
        Expr::UnOp {
            op,
            operand,
            range,
        } => {
            let v = eval_expr(operand, cx, budget)?;
            return eval_const_unop(op, v).ok_or_else(|| ConstError {
                reason: ConstReason::Refused(unop_refused(op)),
                range: range.clone(),
            });
        }
        Expr::Call {
            callee,
            args,
            range,
            ..
        } => {
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
                // `.length()` on a const array/map: not part of the
                // certified call catalog `eval_method_call` consults below
                // (array/map methods are a special "pseudo var" surface,
                // never a `catalog::calls` entry — see `lower/access.rs`),
                // so it gets its own tiny dispatch instead of a catalog spec.
                if let Some(result) = eval_collection_length(obj, field, args, cx, budget) {
                    return result;
                }
                if let Some(result) = eval_method_call(obj, field, args, range, cx, budget) {
                    return result;
                }
            }
            // `Vec`/`Rotation`/`Color`: `expr_to_literal_in` folds these only
            // when every argument is ALREADY a literal on its own face — it
            // knows nothing about a `const mod` call inside an argument
            // (`Vec(f(1.0), 2.0, 3.0)`). Bind the arguments to the
            // constructor's catalog parameters and recurse each through THIS
            // function (which resolves a mod call), then hand the resulting
            // PARAMETER-ORDERED literals to the SAME `fold_constructor` the
            // syntactic path (`lower::predeclare::expr_to_literal_lit`) uses,
            // so both agree on what these builtins accept.
            if let Expr::Ident { name, .. } = callee.as_ref()
                && FOLDABLE_CONSTRUCTORS.contains(&name.as_str())
            {
                let lits = bind_constructor_args(name, args, range, cx, budget)?;
                return match fold_constructor(name, &lits) {
                    Some(lit) => Ok(lit),
                    None => Err(ConstError {
                        reason: ConstReason::Refused(constructor_refused(name)),
                        range: range.clone(),
                    }),
                };
            }
            // A plain-name call whose callee resolves (via `cx.lookup_mod`)
            // to a declaration marked `const mod`: evaluate every argument
            // against the CALLER's `cx` (exactly like `bind_call_args`'s
            // other caller, `exec_call_stmt`), then delegate to
            // `interp::eval_call`, which builds the CALLEE's own environment
            // (module constants + these arguments) and runs its body.
            // A non-const decl (or no decl at all) falls through to
            // `reason_for` below, which already reports `NotAConstMod` for
            // any non-constructor named call.
            if let Expr::Ident { name, .. } = callee.as_ref()
                && let Some(lookup) = cx.lookup_mod
                && let Some(decl) = lookup(name)
                && decl.is_const
            {
                let arg_lits = super::interp::bind_call_args(&decl.inputs, args, cx, budget)?;
                return super::interp::eval_call(&decl, &arg_lits, cx, budget);
            }
        }
        _ => {}
    }
    let (reason, range) = reason_for(e, cx);
    Err(ConstError { reason, range })
}

/// Bind a foldable constructor's call arguments to its CATALOG parameters
/// (`catalog::calls::find_call(name).params`) and evaluate each, producing the
/// PARAMETER-ORDERED literal list [`fold_constructor`] expects.
///
/// Binding by name rather than by source position is the whole point:
/// `Vec(z = 3.0, x = 1.0, y = 2.0)` must put `1.0` on `x`. Pushing arguments
/// in the order they were written and letting `fold_constructor` assign them
/// positionally silently produced `Vector { x: 3.0, y: 1.0, z: 2.0 }` — a
/// wrong constant baked into the gate with no diagnostic.
///
/// Uses the same positional-then-named binding convention as its two siblings
/// in this crate — `eval_method_call` below and `interp::bind_call_args` —
/// but is STRICTER than either about arguments that bind nothing: a
/// positional argument past the last parameter, a named one matching no
/// parameter, or two arguments claiming one parameter are all `Refused`
/// rather than dropped.
///
/// The reason is NOT that this matches the runtime path — it deliberately
/// does not. `lower::call::builtin::lower_builtin_call` binds
/// last-write-wins and silently ignores an argument that matches no
/// parameter, so `Vec(1.0, x = 2.0, y = 3.0)` lowers there to a `MakeVector`
/// with `x=2, y=3` and `z` unwired, while this function refuses it. The
/// reason is that every one of these shapes was ALREADY an error before
/// constructor arguments were evaluated here — `expr_to_literal_lit` bails on
/// any non-positional argument, and `fold_constructor`'s arity match rejects
/// a too-long list — so accepting one now would turn a former error into a
/// value folded from whichever arguments happened to bind. That is precisely
/// the silent miscompile this binding exists to prevent, so erroring is the
/// right direction even where the runtime path is laxer.
///
/// A parameter with no argument ENDS the list rather than erroring: `Color`'s
/// 4th parameter (`a`) is optional, and `fold_constructor` distinguishes the
/// 3-argument (alpha defaults to opaque) form from the 4-argument one by
/// length. A HOLE — an unbound parameter with a bound one after it — therefore
/// yields a SHORT list that `fold_constructor` declines. That is sound only
/// because every optional parameter of a foldable constructor is in LAST
/// position: a fold needs `len ∈ {3, 4}` (Color) or `{3}` (Vec/Rotation), and
/// with the optional params trailing, any such length forces every earlier
/// parameter to have been bound — so a hole can never yield a foldable list
/// with values shifted onto the wrong slots.
/// `foldable_constructors_are_all_really_foldable` asserts that
/// trailing-optionals property directly against the catalog, because it is
/// what this `break` rests on.
///
/// KNOWN TRADEOFF (accepted, do not file as a bug): arguments past a hole are
/// never evaluated, so a call that has BOTH a hole and a non-constant
/// argument reports only this function's own `Refused` at the whole call's
/// range — e.g. `Vec(x = 1.0, z = live)` says "'Vec' has no constant form for
/// these arguments" (WS047) rather than `reason_for`'s more precise "'live'
/// is a runtime value" (WS046) pointing at `live`. This is a documented
/// exception to `reason_for`'s "blame the offending sub-expression" contract:
/// recovering the precise blame would mean evaluating exactly the arguments
/// this design skips. The shape is an error either way, and WS011 fires
/// alongside it.
fn bind_constructor_args(
    name: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Result<Vec<Literal>, ConstError> {
    let refuse = || ConstError {
        reason: ConstReason::Refused(constructor_refused(name)),
        range: range.clone(),
    };
    // Guaranteed present for every `FOLDABLE_CONSTRUCTORS` entry (see its doc
    // comment); refusing rather than asserting keeps a catalog rename a
    // diagnostic instead of a panic — `foldable_constructors_are_all_really_foldable`
    // is what actually catches the drift.
    let Some(spec) = crate::catalog::calls::find_call(name) else {
        return Err(refuse());
    };

    let mut bound: HashMap<&str, &Expr> = HashMap::default();
    let mut next_pos = 0usize;
    for a in args {
        let (param, value) = match a {
            CallArg::Positional(v) => {
                let Some(p) = spec.params.get(next_pos) else {
                    return Err(refuse()); // more positional args than parameters
                };
                next_pos += 1;
                (p.name, v)
            }
            CallArg::Named { name, value, .. } => {
                let Some(p) = spec.params.iter().find(|p| p.name == name) else {
                    return Err(refuse()); // names no parameter of this constructor
                };
                (p.name, value)
            }
            // A spread has no constant form. Refused explicitly, like
            // `Expr::Array`'s spread ELEMENT above — unwrapping it as an
            // ordinary positional argument would bind an array literal to a
            // scalar axis and lean on `fold_constructor` to reject it by
            // accident.
            CallArg::Spread(v) => {
                return Err(ConstError {
                    reason: ConstReason::Unsupported("a spread"),
                    range: v.range().clone(),
                });
            }
        };
        if bound.insert(param, value).is_some() {
            return Err(refuse()); // two arguments for one parameter
        }
    }

    let mut lits = Vec::with_capacity(spec.params.len());
    for p in &spec.params {
        // `break`, NOT `continue`: skipping an unbound parameter would CLOSE
        // the hole, sliding every later argument one slot left onto the wrong
        // axis — `Color(0.5, 0.75, a = 0.25)` would fold to
        // `LinearColor { r: 0.5, g: 0.75, b: 0.25, a: 1.0 }`, silently landing
        // the alpha value on blue. Stopping instead yields a short list that
        // `fold_constructor` declines (see this function's doc comment for why
        // that is sound). Guarded by
        // `a_hole_in_a_constructors_arguments_is_refused_not_closed`.
        let Some(&arg) = bound.get(p.name) else { break };
        lits.push(eval_expr(arg, cx, budget)?);
    }
    Ok(lits)
}

/// `t.length()` on a const array/map. `None` means this isn't a candidate at
/// all — the field isn't `length`, there are arguments, or `obj` evaluates to
/// something other than a `Literal::Array`/`Literal::Map` — so the caller
/// falls back to `eval_method_call`/`reason_for` exactly as before this
/// function existed. Once `field == "length"` with no arguments AND `obj`
/// evaluates, the result is always `Some`: a failing `obj` propagates its own
/// error verbatim (`Err`), a successful array/map yields its element count.
fn eval_collection_length(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    if field != "length" || !args.is_empty() {
        return None;
    }
    match eval_expr(obj, cx, budget) {
        Ok(Literal::Array(items)) => Some(Ok(Literal::Int(items.len() as i64))),
        Ok(Literal::Map(pairs)) => Some(Ok(Literal::Int(pairs.len() as i64))),
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    }
}

/// `Expr::Call` with a `FieldAccess` callee: a method-call form like
/// `"hi".ToUpper()` or `a.Dot(b)`. `None` means this isn't a certified-fold
/// candidate at all — an unknown method, one with no receiver form, or an
/// exec-form call like `entity.SetLocation(..)` — so the caller falls back to
/// `reason_for`'s ordinary call diagnostics instead of a misleading `Refused`.
/// Once a candidate IS found, the result is always `Some`: `Ok` on success,
/// `Err` either propagated verbatim from a receiver/argument that is itself
/// not constant, or a `Refused` (WS047) naming the method when the certified
/// evaluator declines the operands.
///
/// Binds `obj`/`args` to `spec.params` the same way
/// `lower::call::builtin::lower_builtin_call` does — the receiver fills the
/// first param, then positional/named args fill the rest in declaration
/// order — so the `Value` slice handed to `fold::eval::eval` matches the
/// exact per-param signature the in-game probe recorded. An unbound OPTIONAL
/// param still gets its own slot (`None`, i.e. unwired) rather than being
/// omitted, matching a real gate's port always existing whether or not the
/// call supplied it (see `fold::eval::format_text`'s doc comment for the
/// same mechanic on `Fmt`).
fn eval_method_call(
    obj: &Expr,
    field: &str,
    args: &[CallArg],
    range: &SourceRange,
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Option<Result<Literal, ConstError>> {
    let spec = crate::catalog::calls::find_call(field)?;
    if spec.exec || spec.receiver.is_none() {
        return None;
    }
    let first = spec.params.first()?;

    let mut bound: HashMap<&str, &Expr> = HashMap::default();
    bound.insert(first.name, obj);
    let mut next_pos = 1usize;
    for a in args {
        match a {
            CallArg::Named { name, value, .. } => {
                if spec.params.iter().any(|p| p.name == name) {
                    bound.insert(name.as_str(), value);
                }
            }
            CallArg::Positional(value) => {
                if let Some(p) = spec.params.get(next_pos) {
                    bound.insert(p.name, value);
                }
                next_pos += 1;
            }
            CallArg::Spread(_) => {} // no constant form; unbound slot below refuses
        }
    }

    let refuse = |why: String| {
        Some(Err(ConstError {
            reason: ConstReason::Refused(why),
            range: range.clone(),
        }))
    };

    let mut inputs: Vec<Option<Value>> = Vec::with_capacity(spec.params.len());
    for p in &spec.params {
        let Some(&arg_expr) = bound.get(p.name) else {
            inputs.push(None); // unwired, exactly like an omitted optional arg
            continue;
        };
        let lit = match eval_expr(arg_expr, cx, budget) {
            Ok(lit) => lit,
            Err(err) => return Some(Err(err)),
        };
        let Some(value) = Value::from_literal(&lit) else {
            return refuse(format!(
                "`.{field}(…)` has no certified constant result for these operands"
            ));
        };
        inputs.push(Some(value));
    }

    // `fold_eval::eval` is `lower::fold::eval::eval` — a pure Rust table
    // lookup that computes ONE wire gate's certified value law (e.g. string
    // ToUpper/Trim) from already-evaluated operands. It does not parse or
    // execute source text; the name just mirrors the gate-graph "evaluator"
    // vocabulary used throughout `lower::fold`. Not a code-eval security
    // surface.
    match fold_eval::eval(spec.gate_class, &inputs) {
        Some(v) => Some(Ok(v.to_literal())),
        None => refuse(format!(
            "`.{field}(…)` has no certified constant result for these operands"
        )),
    }
}

/// `Expr::InterpLit` (`"a${1 + 1}b"`): builds the EXACT same `{N}`-slot
/// template `lower_interp` (`lower/ops.rs:344-379`) builds — literal chunks
/// with `{`/`}` escaped to `{{`/`}}`, each `${expr}` a `{N}` placeholder in
/// substitution order — and renders it through `fold::eval::format_text`, so
/// a baked constant reads byte-identical to what the FormatText gate the
/// unfolded form would have emitted actually prints. Substitutions chunk at 7
/// per group (a FormatText gate has 7 inputs, `InputA`..`InputG`) exactly
/// like `lower_interp`'s own grouping, restarting `{0}` each group; the
/// per-group rendered strings are then joined with plain Rust concatenation,
/// which is safe because `Concatenate`'s own certified law for two string
/// operands (`concat_display`) IS plain string concatenation — the same
/// operation `lower_interp`'s `concat_string_ports` wires a real `Concatenate`
/// gate to perform on group outputs that are already-rendered strings.
fn eval_interp(
    parts: &[InterpPart],
    range: &SourceRange,
    cx: &ConstCtx,
    budget: &mut Budget,
) -> Result<Literal, ConstError> {
    const MAX_SLOTS: usize = 7; // mirrors lower/ops.rs::FORMAT_SLOTS.len()

    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    let mut cur_fmt = String::new();
    let mut cur_slots: Vec<Value> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Lit(s) => {
                cur_fmt.push_str(&s.replace('{', "{{").replace('}', "}}"));
            }
            InterpPart::Expr(expr) => {
                if cur_slots.len() == MAX_SLOTS {
                    groups.push((std::mem::take(&mut cur_fmt), std::mem::take(&mut cur_slots)));
                }
                // A failing part propagates its OWN error verbatim — it
                // already blames the right sub-range via the normal
                // eval_expr -> reason_for path, so this function must not
                // re-wrap or re-explain it.
                let lit = eval_expr(expr, cx, budget)?;
                let Some(value) = Value::from_literal(&lit) else {
                    return Err(ConstError {
                        reason: ConstReason::Refused(
                            "this interpolated value has no certified constant text form"
                                .to_string(),
                        ),
                        range: expr.range().clone(),
                    });
                };
                cur_fmt.push_str(&format!("{{{}}}", cur_slots.len()));
                cur_slots.push(value);
            }
        }
    }
    groups.push((cur_fmt, cur_slots));

    let mut out = String::new();
    for (fmt, slots) in groups {
        let inputs: Vec<Option<Value>> = slots.into_iter().map(Some).collect();
        match fold_eval::format_text(&fmt, &inputs) {
            Some(s) => out.push_str(&s),
            None => {
                return Err(ConstError {
                    reason: ConstReason::Refused(
                        "this string interpolation has no certified constant result".to_string(),
                    ),
                    range: range.clone(),
                });
            }
        }
    }
    Ok(Literal::String(out))
}

/// Best available explanation for a failure, paired with the range of the
/// sub-expression it actually blames (NOT necessarily `e`'s own range) — so a
/// diagnostic built from this underlines `runtimeThing` in
/// `Vec(runtimeThing, 1.0, 2.0)`, not the whole call. Walks to the first
/// sub-expression that is itself unevaluable, so `a + 1` blames `a` rather
/// than the `+`.
///
/// A compound form only recurses into a part that genuinely fails to evaluate.
/// When every part IS constant and the whole still isn't, the operator or
/// constructor itself declined the operands (`1 << 100` — a shift distance
/// outside `0..64`), which is `Refused`, not a claim that a literal is somehow
/// a runtime value — and is blamed on the whole compound expression, since no
/// single part is at fault.
fn reason_for(e: &Expr, cx: &ConstCtx) -> (ConstReason, SourceRange) {
    let fails = |e: &Expr| expr_to_literal_in(e, &cx.consts).is_none();
    match e {
        Expr::Ident { name, .. } if !cx.consts.contains_key(name.as_str()) => {
            (ConstReason::NotConstant(name.clone()), e.range().clone())
        }
        Expr::BinOp {
            op, left, right, ..
        } => {
            if fails(left) {
                reason_for(left, cx)
            } else if fails(right) {
                reason_for(right, cx)
            } else {
                (ConstReason::Refused(binop_refused(op)), e.range().clone())
            }
        }
        Expr::UnOp { op, operand, .. } => {
            if fails(operand) {
                reason_for(operand, cx)
            } else {
                (ConstReason::Refused(unop_refused(op)), e.range().clone())
            }
        }
        Expr::Call { callee, args, .. } => {
            // An argument is the likelier culprit than the callee, and naming
            // the callee first would misreport `Vec(runtimeThing, 1.0, 2.0)`.
            for a in args {
                let value = match a {
                    CallArg::Positional(v) | CallArg::Spread(v) => v,
                    CallArg::Named { value, .. } => value,
                };
                if fails(value) {
                    return reason_for(value, cx);
                }
            }
            match callee.as_ref() {
                Expr::Ident { name, .. } if FOLDABLE_CONSTRUCTORS.contains(&name.as_str()) => (
                    ConstReason::Refused(constructor_refused(name)),
                    e.range().clone(),
                ),
                // Resolve the callee before blaming it. Reaching `reason_for`
                // means `eval_expr`'s own `Expr::Call` arm did NOT evaluate
                // this call, and that has two very different causes: the
                // callee genuinely isn't a `const mod` (blame that), or it IS
                // one but sits inside a SURROUNDING expression that has no
                // compile-time form — most often an argument to a call whose
                // own callee is not a `const mod` — so evaluation never
                // descended to it. This distinction must be checked rather
                // than assumed.
                Expr::Ident { name, .. } => {
                    let is_const_mod = cx
                        .lookup_mod
                        .and_then(|lookup| lookup(name))
                        .is_some_and(|decl| decl.is_const);
                    let reason = if is_const_mod {
                        ConstReason::NestedConstModCall(name.clone())
                    } else {
                        ConstReason::NotAConstMod(name.clone())
                    };
                    (reason, e.range().clone())
                }
                _ => (ConstReason::Unsupported("this call"), e.range().clone()),
            }
        }
        Expr::InterpLit { .. } => {
            (ConstReason::Unsupported("string interpolation"), e.range().clone())
        }
        Expr::IndexAccess { .. } => (ConstReason::Unsupported("indexing"), e.range().clone()),
        Expr::IfExpr { .. } => {
            (ConstReason::Unsupported("an `if` expression"), e.range().clone())
        }
        Expr::RecordLit { .. } => {
            (ConstReason::Unsupported("a record literal"), e.range().clone())
        }
        Expr::FieldAccess { .. } => (ConstReason::Unsupported("field access"), e.range().clone()),
        _ => (ConstReason::Unsupported("this expression"), e.range().clone()),
    }
}
