//! Monomorphization helpers shared between typecheck and lowering.
//!
//! Generic `mod`/`chip<T, ...>` calls are resolved by arg-driven inference:
//! [`collect`] walks a declared param type (which may contain `Type::Param`s)
//! against the caller's inferred argument type in lockstep, recording an
//! equality [`Constraint`](crate::types::infer::Constraint) everywhere the
//! param side names a `Type::Param`; [`crate::types::infer::solve`] turns those
//! constraints into a [`Subst`]; [`substitute`] then replaces every
//! `Type::Param` with its solved binding.
//!
//! Typecheck uses these at the call site to type the result; lowering
//! re-runs the same inference at the inline site to monomorphize the
//! body — so `pick<int>` emits int gates and `pick<vector>` emits vector gates
//! instead of leaking a `Type::Param` to emit. Keeping them here (rather than
//! private to `typecheck.rs`) lets both consumers share one implementation.

use crate::ast::TypeExpr;
use crate::ir::Type;
use crate::types::infer::{Constraint, Subst, solve};
use crate::collections::HashMap;

/// Strip a single leading `Ref` layer. A `*T` param resolves to
/// `Ref(Param(T))`, but a var passed to it infers to its already-auto-derefed
/// inner type, so the two are asymmetric — unwrap both before collecting.
pub fn unwrap_ref(t: &Type) -> Type {
    match t {
        Type::Ref(inner) => inner.as_ref().clone(),
        other => other.clone(),
    }
}

/// Walk `p` (a param's declared type, possibly containing `Type::Param`s) and
/// `a` (the caller's inferred argument type) in lockstep, pushing an
/// `Eq(name, a')` constraint for every `Type::Param(name)` found in `p`
/// (`a'` being the corresponding sub-type of `a` at that position). A
/// structural mismatch (e.g. `p` is `T[]` but `a` isn't an array) contributes
/// nothing — that mismatch is a type error the ordinary arg-type checking
/// already surfaces elsewhere, not this pass's job. A concrete (non-`Param`,
/// non-compound) `p` also contributes nothing: there's nothing to infer from
/// it.
pub fn collect(p: &Type, a: &Type, out: &mut Vec<Constraint>) {
    match p {
        Type::Param(n) => out.push(Constraint::Eq(n.clone(), a.clone())),
        Type::Array(pi) => {
            if let Type::Array(ai) = a {
                collect(pi, ai, out);
            }
        }
        Type::Ref(pi) => {
            if let Type::Ref(ai) = a {
                collect(pi, ai, out);
            }
        }
        Type::Map(pk, pv) => {
            if let Type::Map(ak, av) = a {
                collect(pk, ak, out);
                collect(pv, av, out);
            }
        }
        Type::Tuple(ps) => {
            if let Type::Tuple(as_) = a {
                for (pi, ai) in ps.iter().zip(as_.iter()) {
                    collect(pi, ai, out);
                }
            }
        }
        Type::Record(pf) => {
            if let Type::Record(af) = a {
                for (name, pt) in pf {
                    if let Some((_, at)) = af.iter().find(|(n, _)| n == name) {
                        collect(pt, at, out);
                    }
                }
            }
        }
        // `Option<T>` against `Option<int>` binds `T=int`, one constraint per
        // arg position. A mismatched name or arity contributes nothing: that
        // mismatch is a type error surfaced elsewhere (WS033/WS003).
        Type::Enum { name: pn, args: pa } => {
            if let Type::Enum { name: an, args: aa } = a
                && pn == an
                && pa.len() == aa.len()
            {
                for (pi, ai) in pa.iter().zip(aa.iter()) {
                    collect(pi, ai, out);
                }
            }
        }
        _ => {}
    }
}

/// Replace every `Type::Param(n)` in `t` with `s[n]`, recursing through
/// compound types; a param missing from `s` (shouldn't happen once `solve`
/// has succeeded — every param it was asked to solve is in the result) is
/// left as-is rather than panicking. Non-param types pass through unchanged.
pub fn substitute(t: &Type, s: &Subst) -> Type {
    match t {
        Type::Param(n) => s.get(n).cloned().unwrap_or_else(|| t.clone()),
        Type::Array(inner) => Type::Array(Box::new(substitute(inner, s))),
        Type::Ref(inner) => Type::Ref(Box::new(substitute(inner, s))),
        Type::Map(k, v) => Type::Map(Box::new(substitute(k, s)), Box::new(substitute(v, s))),
        Type::Union(opts) => Type::Union(opts.iter().map(|o| substitute(o, s)).collect()),
        Type::Tuple(fs) => Type::Tuple(fs.iter().map(|f| substitute(f, s)).collect()),
        Type::Record(fs) => Type::Record(
            fs.iter()
                .map(|(n, ft)| (n.clone(), substitute(ft, s)))
                .collect(),
        ),
        Type::Enum { name, args } => Type::Enum {
            name: name.clone(),
            args: args.iter().map(|a| substitute(a, s)).collect(),
        },
        other => other.clone(),
    }
}

/// Resolve a type-param bound (`T: Bound`) to its mask — the set of concrete
/// types the param may be instantiated to. `None` (unbounded `T`) is the full
/// `Variant` mask. A bare class name (`Numeric`, `Scalar`, `Variant`) uses the
/// matching built-in class mask; otherwise the bound is resolved structurally
/// via the canonical resolver (`aliases` covers `type X = …` names), a `Union`
/// becoming its options list and a single concrete type a one-element mask.
/// Anything unresolvable falls back to the unbounded `Variant` mask — v1
/// bound-checking only needs to recognize class names and unions, not diagnose
/// a malformed bound (covered elsewhere). Diagnostics are discarded.
pub fn mask_for_param(bound: Option<&TypeExpr>, aliases: &HashMap<String, Type>) -> Vec<Type> {
    let Some(te) = bound else {
        return crate::types::classes::variant_mask();
    };
    if let TypeExpr::Name { name, .. } = te
        && let Some(mask) = crate::types::classes::class_mask(name)
    {
        return mask;
    }
    let empty_generic: HashMap<String, crate::types::resolve::GenericAlias> = HashMap::default();
    let cx = crate::types::resolve::ResolveCtx {
        params: &[],
        type_aliases: aliases,
        generic_aliases: &empty_generic,
    };
    match crate::types::resolve::resolve_type(te, &cx, &mut Vec::new()) {
        Type::Union(opts) => opts,
        Type::Any => crate::types::classes::variant_mask(),
        other => vec![other],
    }
}

/// Collect an equality constraint from every (declared param type, inferred
/// arg type) pair in lockstep — the shared first half of generic-call
/// inference (arg-driven type-param solving), used by both the typecheck-side
/// call-site inference and the lowering-side monomorphizer (see module docs).
/// Ref-aligns each pair before collecting: a `*T` param resolves to
/// `Ref(Param(T))`, but a var passed to it infers to its already-auto-derefed
/// inner type (`Int`, not `Ref(Int)`), so the Ref layers are asymmetric —
/// [`unwrap_ref`] strips a leading Ref off BOTH so `*T` vs `int` collects as
/// `Param(T)` vs `int` → `Eq(T, int)`, exactly like a value `T`.
pub fn call_constraints(param_types: &[Type], arg_types: &[Type]) -> Vec<Constraint> {
    let mut constraints = Vec::new();
    for (p, a) in param_types.iter().zip(arg_types.iter()) {
        collect(&unwrap_ref(p), &unwrap_ref(a), &mut constraints);
    }
    constraints
}

/// Rebuild the call-site substitution for a generic `mod`/`chip` from the
/// declared param types (which may contain `Type::Param`s), the caller's
/// concrete argument types, and the type params (name + mask). Mirrors the
/// typecheck-side inference: [`call_constraints`] collects the equality
/// constraints (ref-aligning each param/arg pair), then [`solve`].
///
/// On solver failure — which for a program that already type-checked can only
/// mean an exotic/uninferable shape the caller has already been warned about
/// (WS033) — fall back to a best-effort direct build from the collected
/// constraints, so a valid, fully-applied generic call never leaves a
/// `Type::Param` unresolved at emit.
pub fn infer_call_subst(
    param_types: &[Type],
    arg_types: &[Type],
    params: &[(String, Vec<Type>)],
) -> Subst {
    let constraints = call_constraints(param_types, arg_types);
    if let Ok(s) = solve(&constraints, params) {
        return s;
    }
    // Best-effort fallback: first non-dynamic constraint per param wins.
    let mut s = Subst::new();
    for (name, _mask) in params {
        for Constraint::Eq(cvar, cty) in &constraints {
            if cvar == name && !matches!(cty, Type::Any | Type::Opaque) {
                s.entry(name.clone()).or_insert_with(|| cty.clone());
            }
        }
    }
    s
}

#[cfg(test)]
mod tests;
