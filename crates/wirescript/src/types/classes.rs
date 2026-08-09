//! Built-in generic constraint classes (masks). A mask is the set of wire
//! *value* variants a masked type parameter may be instantiated to. Masks are
//! value variants only — never a ref/array-ref/map-ref, zone, teleport, or exec.
//! `Scalar ⊆ Numeric ⊆ Variant`.

use crate::ir::Type;

/// The members of a named constraint class, or `None` if the name isn't a
/// built-in class. `Scalar` (int/float), `Numeric` (prim-math variants),
/// `Variant` (all value variants; = unbounded `<T>` = what `any` erases to).
pub fn class_mask(name: &str) -> Option<Vec<Type>> {
    Some(match name {
        "Scalar" => vec![Type::Int, Type::Float],
        "Numeric" => vec![
            Type::Int, Type::Float, Type::Vector, Type::Rotator, Type::Quat, Type::Color,
        ],
        "Variant" => variant_mask(),
        _ => return None,
    })
}

/// All wire value variants — the maximal mask (`Variant` / unbounded `<T>`).
pub fn variant_mask() -> Vec<Type> {
    vec![
        Type::Bool, Type::Int, Type::Float, Type::String, Type::Vector, Type::Rotator,
        Type::Quat, Type::Color, Type::Entity, Type::Character, Type::Controller,
    ]
}

/// Is the ground type `t` a member of `mask`? (Masks hold ground value
/// variants, so this is plain equality membership.)
pub fn mask_contains(mask: &[Type], t: &Type) -> bool {
    mask.iter().any(|m| m == t)
}

#[cfg(test)]
mod tests;
