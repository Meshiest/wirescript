//!
//! Phase 1 stub: no `tvar` in `Type` yet, so unification reduces to
//! structural equality + numeric-pair glb. The full Hindley-Milner-ish
//! unifier arrives in Phase 4b alongside typecheck generics.

use crate::ir::Type;
use crate::types::coerce::{coerce, CoerceRule};

#[derive(Debug)]
pub struct UnifyError {
    pub message: String,
}

/// Greatest lower bound: the tightest type that both `a` and `b` can
/// flow into. Returns `None` if no rule matches.
pub fn unify_glb(a: &Type, b: &Type) -> Option<Type> {
    use Type::*;
    // `Opaque` (an `Opaque(...)` probe result) behaves exactly like `Any`
    // here — it propagates the other side's type rather than participating
    // in numeric-rank promotion.
    if matches!(a, Any | Opaque) {
        return Some(b.clone());
    }
    if matches!(b, Any | Opaque) {
        return Some(a.clone());
    }
    if coerce(a, b) == CoerceRule::Same {
        return Some(a.clone());
    }

    // Numeric promotion: int+float → float, bool+int → int, etc.
    let num_rank = |t: &Type| -> Option<u8> {
        match t {
            Bool => Some(0),
            Int => Some(1),
            Float => Some(2),
            _ => None,
        }
    };
    if let (Some(ra), Some(rb)) = (num_rank(a), num_rank(b)) {
        let r = ra.max(rb);
        return Some(match r {
            0 => Bool,
            1 => Int,
            _ => Float,
        });
    }

    None
}

/// Least upper bound — placeholder for Phase 4b. For now mirrors
/// `unify_glb` since Phase 1 has no tvars.
pub fn unify_lub(a: &Type, b: &Type) -> Option<Type> {
    unify_glb(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_and_float_promote() {
        assert_eq!(unify_glb(&Type::Int, &Type::Float), Some(Type::Float));
    }
    #[test]
    fn bool_and_int_promote() {
        assert_eq!(unify_glb(&Type::Bool, &Type::Int), Some(Type::Int));
    }
    #[test]
    fn any_sides_propagate() {
        assert_eq!(unify_glb(&Type::Any, &Type::Int), Some(Type::Int));
        assert_eq!(unify_glb(&Type::Float, &Type::Any), Some(Type::Float));
    }
    #[test]
    fn opaque_sides_propagate_like_any() {
        assert_eq!(unify_glb(&Type::Opaque, &Type::Int), Some(Type::Int));
        assert_eq!(unify_glb(&Type::Float, &Type::Opaque), Some(Type::Float));
    }
    #[test]
    fn unrelated_returns_none() {
        assert!(unify_glb(&Type::String, &Type::Vector).is_none());
    }
}
