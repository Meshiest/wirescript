//! Generic-inference solver: collects constraints on type variables and
//! solves them into a substitution. **Safe widening**, not strict equality —
//! when several constraints pin the same var, the resolved type is their
//! join over a non-lossy, widening-only lattice
//! ([`crate::types::coerce::widening_join`]), and each contributing argument
//! casts to the joined type at its port (`pick(flag, 1, 2.0)` infers
//! `T = float`, the int arg casting in). `any`/`Opaque` contributes nothing
//! (gradual: consistent with everything, pins nothing); unpinnable,
//! out-of-mask, or no-common-widening are errors.

use crate::ir::Type;
use std::collections::HashMap;

/// A resolved substitution: type-variable name → concrete type.
pub type Subst = HashMap<String, Type>;

/// One inference constraint.
#[derive(Clone, Debug, PartialEq)]
pub enum Constraint {
    /// The type variable named `.0` must be able to widen from `.1` — i.e.
    /// `.1` is a lower bound on the var, not necessarily its exact type.
    /// `solve` folds every surviving `.1` for a given var with
    /// `widening_join`; the var resolves to their join.
    Eq(String, Type),
}

#[derive(Clone, Debug, PartialEq)]
pub enum InferError {
    /// Two constraints pin the same var to types with no common widening
    /// (e.g. int vs vector — genuinely incompatible, not just "different").
    Conflict { var: String, a: Type, b: Type },
    /// A var has no (non-`any`) constraint — can't be inferred (annotate, or
    /// pass an explicit type argument).
    Unpinnable(String),
    /// The resolved (joined) type isn't a member of the var's mask.
    OutOfMask { var: String, ty: Type, mask: Vec<Type> },
}

/// `any`/`Opaque`-typed constraint values are the dynamic type: consistent
/// with everything, they pin nothing and are dropped before solving.
fn is_dynamic(ty: &Type) -> bool {
    matches!(ty, Type::Any | Type::Opaque)
}

/// Solve `constraints` for the type variables in `params` (each var paired with
/// its mask). Rules:
///  - An `any`/`Opaque`-typed constraint value contributes NOTHING (gradual:
///    consistent with everything, pins nothing) — drop it before solving.
///  - For each var in `params`: fold all its surviving `Eq` constraint types
///    with [`crate::types::coerce::widening_join`] (least-upper-bound over
///    the widening-only lattice); if any fold step has no common widening
///    (`widening_join` returns `None`) → `Conflict`. The var's resolved type
///    is the final join (e.g. int + float folds to float).
///  - A var with no surviving constraint → `Unpinnable`.
///  - The resolved (joined) type must satisfy `mask_contains(mask, &ty)` →
///    else `OutOfMask`.
///  - Success → a `Subst` mapping every var in `params` to its resolved type.
pub fn solve(
    constraints: &[Constraint],
    params: &[(String, Vec<Type>)],
) -> Result<Subst, InferError> {
    let mut subst = Subst::new();

    for (var, mask) in params {
        let mut resolved: Option<Type> = None;

        for c in constraints {
            let Constraint::Eq(cvar, cty) = c;
            if cvar != var || is_dynamic(cty) {
                continue;
            }
            resolved = Some(match resolved {
                None => cty.clone(),
                Some(prev) => match crate::types::coerce::widening_join_all([prev.clone(), cty.clone()]) {
                    Some(joined) => joined,
                    None => {
                        return Err(InferError::Conflict {
                            var: var.clone(),
                            a: prev,
                            b: cty.clone(),
                        });
                    }
                },
            });
        }

        let ty = match resolved {
            Some(ty) => ty,
            None => return Err(InferError::Unpinnable(var.clone())),
        };

        if !crate::types::classes::mask_contains(mask, &ty) {
            return Err(InferError::OutOfMask { var: var.clone(), ty, mask: mask.clone() });
        }

        subst.insert(var.clone(), ty);
    }

    Ok(subst)
}

#[cfg(test)]
mod tests;
