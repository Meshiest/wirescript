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
        | (Brick, Brick)
        | (Prefab, Prefab)
        | (Exec, Exec)
        | (Any, Any)
        | (Never, Never) => true,
        (Ref(ai), Ref(bi)) => type_eq(ai, bi),
        (Array(ai), Array(bi)) => type_eq(ai, bi),
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
        _ => false,
    }
}

/// Is `from` pulsing — i.e. does a value-changed edge on this wire trip
/// downstream execs?
fn is_pulsing(t: &Type) -> bool {
    matches!(t, Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
}

fn is_numeric(t: &Type) -> bool {
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
            | Type::Brick
            | Type::Prefab
            | Type::String
    )
}

pub fn coerce(from: &Type, to: &Type) -> CoerceRule {
    use Type::*;
    // `any` is the universal target and source.
    if matches!(from, Any) || matches!(to, Any) {
        return CoerceRule::Same;
    }
    if type_eq(from, to) {
        return CoerceRule::Same;
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
    // Character ↔ Controller: bidirectional coercion (auto ControllerOf / CharacterOf).
    if matches!((from, to), (Character, Controller) | (Controller, Character)) {
        return CoerceRule::Coerce;
    }
    // Entity → Character/Controller: wires carry plain object refs and an
    // entity can be a player (e.g. Sweep's HitEntity), so the downcast is
    // implicit — it wires directly, like character ↔ controller (no adapter).
    if matches!((from, to), (Entity, Character) | (Entity, Controller)) {
        return CoerceRule::Coerce;
    }

    // Rotator ↔ Quat: a rotation and a quaternion are interchangeable rotation
    // values at the wire level (the engine's rotation gates accept either), so a
    // rotation converts to a quat and back. Enables rotating a vector by an
    // entity's `GetRotation()` rotator, or `Rotation(p,y,r).Invert()`.
    if matches!((from, to), (Rotator, Quat) | (Quat, Rotator)) {
        return CoerceRule::Coerce;
    }

    // Numeric ↔ numeric (bidirectional: bool, int, float).
    if is_numeric(from) && is_numeric(to) {
        return CoerceRule::Coerce;
    }

    // Anything primitive → string via Expr_String_FormatText.
    if matches!(to, String) && formats_to_string(from) {
        return CoerceRule::ViaString;
    }

    // "numeric → vector" is explicitly disallowed (broadcast only happens
    // inside specific gates at the wire level, not at the type-coercion level).
    let _ = same_ref_inner; // keep helper alive for future extensions

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
mod tests {
    use super::*;

    #[test]
    fn same_types_same() {
        assert_eq!(coerce(&Type::Int, &Type::Int), CoerceRule::Same);
    }
    #[test]
    fn any_is_universal() {
        assert_eq!(coerce(&Type::Any, &Type::Int), CoerceRule::Same);
        assert_eq!(coerce(&Type::Int, &Type::Any), CoerceRule::Same);
    }
    #[test]
    fn record_auto_unwraps_to_matching_field() {
        let rec = Type::Record(vec![
            ("Index".into(), Type::Int),
            ("Found".into(), Type::Bool),
        ]);
        // unwraps to whichever field matches the target
        assert_ne!(coerce(&rec, &Type::Int), CoerceRule::Mismatch);
        assert_ne!(coerce(&rec, &Type::Bool), CoerceRule::Mismatch);
        // no field matches → still a mismatch
        assert_eq!(coerce(&rec, &Type::Vector), CoerceRule::Mismatch);
        // a record target is not unwrapped
        assert_eq!(coerce(&rec, &Type::Record(vec![])), CoerceRule::Mismatch);
    }
    #[test]
    fn entity_downcasts_to_character_and_controller() {
        // A Sweep's HitEntity (or any entity wire) can be a player — wires
        // carry plain object refs, so the downcast is implicit in-game.
        assert_eq!(coerce(&Type::Entity, &Type::Character), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Entity, &Type::Controller), CoerceRule::Coerce);
    }
    #[test]
    fn numeric_coerces_both_ways() {
        assert_eq!(coerce(&Type::Int, &Type::Float), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Float, &Type::Int), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Bool, &Type::Int), CoerceRule::Coerce);
    }
    #[test]
    fn pulsing_into_exec() {
        assert_eq!(coerce(&Type::Bool, &Type::Exec), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Int, &Type::Exec), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Vector, &Type::Exec), CoerceRule::Coerce);
    }
    #[test]
    fn string_via_format() {
        assert_eq!(coerce(&Type::Int, &Type::String), CoerceRule::ViaString);
        assert_eq!(coerce(&Type::Vector, &Type::String), CoerceRule::ViaString);
    }
    #[test]
    fn everything_primitive_casts_to_string() {
        for t in [
            Type::Bool,
            Type::Float,
            Type::Entity,
            Type::Character,
            Type::Controller,
            Type::Brick,
            Type::Prefab,
            Type::Rotator,
            Type::Color,
        ] {
            assert_eq!(
                coerce(&t, &Type::String),
                CoerceRule::ViaString,
                "{t:?} should cast to string"
            );
        }
    }
    #[test]
    fn ref_invariance() {
        let r_int = Type::Ref(Box::new(Type::Int));
        let r_float = Type::Ref(Box::new(Type::Float));
        assert_eq!(coerce(&r_int, &r_float), CoerceRule::Mismatch);
        assert_eq!(coerce(&r_int, &r_int.clone()), CoerceRule::Same);
    }
    #[test]
    fn numeric_not_to_vector() {
        assert_eq!(coerce(&Type::Int, &Type::Vector), CoerceRule::Mismatch);
    }
    #[test]
    fn character_to_controller() {
        assert_eq!(coerce(&Type::Character, &Type::Controller), CoerceRule::Coerce);
    }
    #[test]
    fn controller_to_character() {
        assert_eq!(coerce(&Type::Controller, &Type::Character), CoerceRule::Coerce);
    }
    #[test]
    fn controller_to_entity() {
        assert_eq!(coerce(&Type::Controller, &Type::Entity), CoerceRule::Coerce);
    }
    #[test]
    fn rotator_and_quat_interconvert() {
        assert_eq!(coerce(&Type::Rotator, &Type::Quat), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Quat, &Type::Rotator), CoerceRule::Coerce);
    }
    #[test]
    fn character_pulses_to_exec() {
        assert_eq!(coerce(&Type::Character, &Type::Exec), CoerceRule::Coerce);
    }
    #[test]
    fn controller_pulses_to_exec() {
        assert_eq!(coerce(&Type::Controller, &Type::Exec), CoerceRule::Coerce);
    }
}
