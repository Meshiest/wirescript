//!
//! Mirrors Brickadia's `PortsAreCompatible` behavior (bidirectional
//! numeric coercion; everything-to-string via `Expr_String_FormatText`;
//! pulsing wires coerce into exec inputs), plus our source-language
//! rules on top (ref invariance).

use crate::ir::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoerceRule {
    /// Wire-compatible without a coercion gate.
    Same,
    /// Accepted but with an implicit coercion (e.g. int → float).
    Coerce,
    /// Routed through an `Expr_String_FormatText` gate inserted by emit.
    ViaString,
    /// Not assignable.
    Mismatch,
}

fn same_ref_inner(a: &Type, b: &Type) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b) && type_eq(a, b)
}

/// Strip one layer of `Ref`, so ref-ness compares as an exposure mode rather
/// than a distinct value type (matching how params/out/assignment treat it).
fn peel_ref(t: &Type) -> &Type {
    match t {
        Type::Ref(inner) => inner,
        other => other,
    }
}

/// The ordered element types of `t` viewed as a tuple: `Tuple([T0,T1])`
/// directly, or an index-keyed `Record([("0",T0),("1",T1)])` — the form a tuple
/// LITERAL desugars to (a tuple annotation resolves to `Tuple`). Field names
/// can't begin with a digit, so an all-index-keyed record is unambiguously a
/// tuple, never a user record.
fn as_tuple_elems(t: &Type) -> Option<Vec<&Type>> {
    match t {
        Type::Tuple(fs) => Some(fs.iter().collect()),
        Type::Record(fs)
            if !fs.is_empty() && fs.iter().enumerate().all(|(i, (k, _))| *k == i.to_string()) =>
        {
            Some(fs.iter().map(|(_, v)| v).collect())
        }
        _ => None,
    }
}

fn type_eq(a: &Type, b: &Type) -> bool {
    use Type::*;
    match (a, b) {
        (Bool, Bool)
        | (Int, Int)
        | (Float, Float)
        | (String, String)
        | (Vector, Vector)
        | (Rotator, Rotator)
        | (Quat, Quat)
        | (Color, Color)
        | (Entity, Entity)
        | (Character, Character)
        | (Controller, Controller)
        | (Zone, Zone)
        | (Teleport, Teleport)
        | (PrefabRef, PrefabRef)
        | (Exec, Exec)
        | (Any, Any)
        | (Opaque, Opaque)
        | (Never, Never) => true,
        (Ref(ai), Ref(bi)) => type_eq(ai, bi),
        (Array(ai), Array(bi)) => type_eq(ai, bi),
        (Map(ak, av), Map(bk, bv)) => type_eq(ak, bk) && type_eq(av, bv),
        (Union(ax), Union(bx)) => {
            ax.len() == bx.len()
                && ax
                    .iter()
                    .all(|at| bx.iter().any(|bt| type_eq(at, bt)))
        }
        (Tuple(ax), Tuple(bx)) => {
            ax.len() == bx.len() && ax.iter().zip(bx).all(|(a, b)| type_eq(a, b))
        }
        (Record(ax), Record(bx)) => {
            ax.len() == bx.len()
                && ax.iter().all(|(k1, t1)| {
                    bx.iter().any(|(k2, t2)| k1 == k2 && type_eq(t1, t2))
                })
        }
        // A param is invariant/nominal — it doesn't coerce to anything; this
        // arm is defensive since substitution normally removes params before
        // coercion.
        (Param(a), Param(b)) => a == b,
        _ => false,
    }
}

/// Is `from` pulsing — i.e. does a value-changed edge on this wire trip
/// downstream execs?
fn is_pulsing(t: &Type) -> bool {
    matches!(t, Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
}

/// Narrower than the `Numeric` constraint class (`class_mask("Numeric")` also
/// includes vector/rotator/quat/color) — just the three primitive scalars
/// that widen/coerce numerically among themselves.
fn is_prim_number(t: &Type) -> bool {
    matches!(t, Type::Bool | Type::Int | Type::Float)
}

/// Phase-1 primitives that format into text for the string coercion path.
fn formats_to_string(t: &Type) -> bool {
    matches!(
        t,
        Type::Bool
            | Type::Int
            | Type::Float
            | Type::Vector
            | Type::Rotator
            | Type::Quat
            | Type::Color
            | Type::Entity
            | Type::Character
            | Type::Controller
            | Type::String
    )
}

pub fn coerce(from: &Type, to: &Type) -> CoerceRule {
    use Type::*;
    // `any` (and `Opaque`, which behaves exactly like `any` outside of
    // operator resolution) is the universal target and source.
    if matches!(from, Any | Opaque) || matches!(to, Any | Opaque) {
        return CoerceRule::Same;
    }
    if type_eq(from, to) {
        return CoerceRule::Same;
    }

    // A union target accepts anything one of its options accepts. Used for the
    // variant gates (`Blend`, `lerp`, `Tween`), whose ports take any of the
    // math variants rather than a single concrete type. Prefer an exact option
    // over one needing a conversion, so `int` into `float|int` stays an int.
    if let Union(opts) = to {
        let mut best = CoerceRule::Mismatch;
        for opt in opts {
            match coerce(from, opt) {
                CoerceRule::Same => return CoerceRule::Same,
                r if !matches!(r, CoerceRule::Mismatch) => best = r,
                _ => {}
            }
        }
        return best;
    }

    // Ref invariance — no coercion through ref types.
    if let (Ref(fi), Ref(ti)) = (from, to) {
        return if type_eq(fi, ti) {
            CoerceRule::Same
        } else {
            CoerceRule::Mismatch
        };
    }
    if matches!(from, Ref(_)) || matches!(to, Ref(_)) {
        return CoerceRule::Mismatch;
    }

    // Pulsing → exec: bool/int/float/vector trip an exec input when their value changes.
    if matches!(to, Exec) && (is_pulsing(from) || matches!(from, Exec)) {
        return CoerceRule::Coerce;
    }
    if matches!((from, to), (Exec, Exec)) {
        return CoerceRule::Same;
    }

    // Character → Entity: character is a subtype of entity in Brickadia.
    if matches!((from, to), (Character, Entity)) {
        return CoerceRule::Coerce;
    }
    // Controller → Entity: controller coerces to entity.
    if matches!((from, to), (Controller, Entity)) {
        return CoerceRule::Coerce;
    }
    // Character <-> Controller: bidirectional coercion (auto ControllerOf / CharacterOf).
    if matches!((from, to), (Character, Controller) | (Controller, Character)) {
        return CoerceRule::Coerce;
    }
    // Entity → Character/Controller: wires carry plain object refs and an
    // entity can be a player (e.g. Sweep's HitEntity), so the downcast is
    // implicit — it wires directly, like character <-> controller (no adapter).
    if matches!((from, to), (Entity, Character) | (Entity, Controller)) {
        return CoerceRule::Coerce;
    }

    // Rotator <-> Quat: a rotation and a quaternion are interchangeable rotation
    // values at the wire level (the engine's rotation gates accept either), so a
    // rotation converts to a quat and back. Enables rotating a vector by an
    // entity's `GetRotation()` rotator, or `Rotation(p,y,r).Invert()`.
    if matches!((from, to), (Rotator, Quat) | (Quat, Rotator)) {
        return CoerceRule::Coerce;
    }

    // Numeric <-> numeric (bidirectional: bool, int, float).
    if is_prim_number(from) && is_prim_number(to) {
        return CoerceRule::Coerce;
    }

    // String → Bool: the language-level coercion means exactly `s != ""` —
    // lowering inserts a `CompareNotEqual(s, "")` gate wherever a
    // string-typed source wires into a bool-typed destination (see
    // `LowerCtx::wrap_string_to_bool` in lower/context.rs). Empty is false,
    // EVERYTHING else is true — deliberately NOT the game's native
    // content-aware port truthiness (where "0" and "false" are also falsy;
    // certified in `data/gate_semantics.json`'s Branch/Select/AND chapters),
    // which stays reachable by wiring through `any` or the logical
    // operators' native string overloads. Unidirectional: bool → string
    // stays the existing `ViaString` format-text path below (renders
    // "true"/"false" text).
    if matches!((from, to), (String, Bool)) {
        return CoerceRule::Coerce;
    }

    // Anything primitive → string via Expr_String_FormatText.
    if matches!(to, String) && formats_to_string(from) {
        return CoerceRule::ViaString;
    }

    // "numeric → vector" is explicitly disallowed (broadcast only happens
    // inside specific gates at the wire level, not at the type-coercion level).
    let _ = same_ref_inner; // keep helper alive for future extensions

    // Two records match field-wise treating ref-ness as an exposure mode, the
    // same way params/out/assignment unwrap refs before comparing (`out y: *int
    // = x` compares int to int). A record literal built from vars exposes scalar
    // fields as refs and array fields plainly, so a `*T` field and a plain `T`
    // field are interchangeable at the boundary — this is what lets a
    // `let cpu: Cpu = { regs, cpsr }` (mixed plain-array + ref-scalar) pass to a
    // mod taking `Cpu`. Field sets must match and each pair must be equal AFTER
    // unwrapping refs, so genuine value-type mismatches (`int` vs `string`
    // fields) are still rejected. Reached only when the exact `type_eq` above
    // already failed, i.e. some field differs solely in ref exposure.
    if let (Record(fa), Record(fb)) = (from, to)
        && as_tuple_elems(from).is_none()
    {
        // Named records: match by field name, ref-insensitively.
        if fa.len() == fb.len()
            && fa.iter().all(|(k1, t1)| {
                fb.iter()
                    .any(|(k2, t2)| k1 == k2 && type_eq(peel_ref(t1), peel_ref(t2)))
            })
        {
            return CoerceRule::Same;
        }
    }

    // Tuples match by position, ref-insensitively — and a tuple LITERAL (an
    // index-keyed record `{"0":T0,"1":T1}`) is interchangeable with a tuple TYPE
    // annotation (`Tuple([T0,T1])`), which `type_str` renders identically. Both
    // sides normalize to their element list; lengths and element types (after
    // unwrapping refs) must match, so a genuine arity/element mismatch still
    // fails.
    if let (Some(ea), Some(eb)) = (as_tuple_elems(from), as_tuple_elems(to)) {
        if ea.len() == eb.len()
            && ea
                .iter()
                .zip(&eb)
                .all(|(a, b)| type_eq(peel_ref(a), peel_ref(b)))
        {
            return CoerceRule::Same;
        }
    }

    // A record auto-unwraps to a member when used where a non-record value is
    // expected: it coerces to `to` if any field does. Lets a multi-output gate
    // result (e.g. `find`'s `{ Index, Found, Value }`) be used directly as the
    // field that matches the context (here, the `int` Index). First match wins.
    if let Record(fields) = from
        && !matches!(to, Record(_))
    {
        for (_, ft) in fields {
            let rule = coerce(ft, to);
            if rule != CoerceRule::Mismatch {
                return rule;
            }
        }
    }

    CoerceRule::Mismatch
}

/// The least-upper-bound of `a` and `b` over a **widening-only** lattice —
/// used by generic-inference joins (`types::infer::solve`): both `a` and
/// `b` cast into the result via a single non-lossy widening, never a
/// narrowing. Returns `None` when there's no common widening (a genuine
/// incompatibility, e.g. int vs vector).
///
/// Only the leaf/value variants below widen; compound types (`Ref`/`Array`/
/// `Map`/`Union`/`Tuple`/`Record`) only join with themselves via the `a == b`
/// fast path — a mismatched compound pair returns `None`, this function
/// never tries to widen through a compound's structure.
pub fn widening_join(a: &Type, b: &Type) -> Option<Type> {
    use Type::*;
    if type_eq(a, b) {
        return Some(a.clone());
    }
    // `any`/`Opaque` is neutral — it widens to whatever the other side is.
    if matches!(a, Any | Opaque) {
        return Some(b.clone());
    }
    if matches!(b, Any | Opaque) {
        return Some(a.clone());
    }
    // Numerics: bool ⊏ int ⊏ float (widest wins).
    if is_prim_number(a) && is_prim_number(b) {
        return Some(if matches!(a, Float) || matches!(b, Float) {
            Float
        } else if matches!(a, Int) || matches!(b, Int) {
            Int
        } else {
            Bool
        });
    }
    // Objects: character/controller ⊏ entity.
    let is_obj = |t: &Type| matches!(t, Character | Controller | Entity);
    if is_obj(a) && is_obj(b) {
        return Some(Entity);
    }
    // Rotator/Quat are interchangeable rotation values at the wire level
    // (see `coerce`'s Rotator<->Quat rule) -> one canonical representative.
    if matches!((a, b), (Rotator, Quat) | (Quat, Rotator)) {
        return Some(Rotator);
    }
    None
}

/// Left-to-right fold of [`widening_join`] over `types` — each item widens
/// into the running accumulator, so the result is the least-upper-bound of
/// the whole sequence. `None` if `types` is empty (there's no join of
/// nothing), or if any step has no common widening with the accumulator so
/// far (mirrors `widening_join`'s own `None` — a genuine incompatibility).
/// Shared by the two call-site folds that need this (`types::infer::solve`
/// joining a type param's constraint types; `typecheck::union_output_type`
/// joining a builtin's union-typed operand/arg types) so the widening
/// semantics can't drift between them.
pub fn widening_join_all(types: impl IntoIterator<Item = Type>) -> Option<Type> {
    let mut iter = types.into_iter();
    let first = iter.next()?;
    iter.try_fold(first, |acc, t| widening_join(&acc, &t))
}

/// Return the list of primitives from which `to` is reachable via at
/// most one coercion rule. Used by the typechecker for "did you mean"
/// hints.
pub fn reachable_from(to: &Type) -> Vec<Type> {
    let candidates = [
        Type::Bool,
        Type::Int,
        Type::Float,
        Type::String,
        Type::Vector,
        Type::Rotator,
        Type::Quat,
        Type::Color,
        Type::Entity,
        Type::Character,
        Type::Controller,
        Type::Exec,
    ];
    candidates
        .into_iter()
        .filter(|k| coerce(k, to) != CoerceRule::Mismatch)
        .collect()
}

#[cfg(test)]
mod tests;
