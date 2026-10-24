//! The named float components of the struct-valued types: a `vector`'s
//! x/y/z, a `color`'s r/g/b/a, a `rotator`'s pitch/yaw/roll and a `quat`'s
//! x/y/z/w. A program writes each with its first letter in either case (`x`
//! or `X`, `pitch` or `Pitch`). Typecheck, lowering, hover and completion read
//! them here; lowering builds the Split gate each one reads through.

use crate::ir::Type;

/// Each struct-valued type's source name and its components, lowercase.
pub const VALUE_FIELDS: &[(&str, &[&str])] = &[
    ("vector", &["x", "y", "z"]),
    ("color", &["r", "g", "b", "a"]),
    ("rotator", &["pitch", "yaw", "roll"]),
    ("quat", &["x", "y", "z", "w"]),
];

/// The components of the type a program names `ty` (`"vector"`), or none.
pub fn fields_of_named(ty: &str) -> &'static [&'static str] {
    VALUE_FIELDS
        .iter()
        .find(|(n, _)| *n == ty)
        .map_or(&[], |(_, fields)| fields)
}

/// The components of `ty`, or none.
pub fn fields_of(ty: &Type) -> &'static [&'static str] {
    fields_of_named(match ty {
        Type::Vector => "vector",
        Type::Color => "color",
        Type::Rotator => "rotator",
        Type::Quat => "quat",
        _ => return &[],
    })
}

/// Whether `name` spells the component `field`: the same word, its first
/// letter in either case.
fn spells(name: &str, field: &str) -> bool {
    let (Some(n), Some(f)) = (name.chars().next(), field.chars().next()) else {
        return false;
    };
    (n == f || n == f.to_ascii_uppercase()) && name[n.len_utf8()..] == field[f.len_utf8()..]
}

/// The component of `ty` that `name` spells, lowercase.
pub fn field_of(ty: &Type, name: &str) -> Option<&'static str> {
    fields_of(ty).iter().copied().find(|f| spells(name, f))
}

/// Whether `name` spells a component of any struct-valued type.
pub fn is_field_name(name: &str) -> bool {
    VALUE_FIELDS
        .iter()
        .any(|(_, fields)| fields.iter().any(|f| spells(name, f)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_component_is_spelled_with_its_first_letter_in_either_case() {
        assert_eq!(field_of(&Type::Rotator, "Pitch"), Some("pitch"));
        assert_eq!(field_of(&Type::Rotator, "pitch"), Some("pitch"));
        assert_eq!(field_of(&Type::Rotator, "PITCH"), None);
        assert_eq!(field_of(&Type::Quat, "W"), Some("w"));
        assert_eq!(field_of(&Type::Vector, "w"), None);
        assert_eq!(field_of(&Type::Int, "x"), None);
        assert!(is_field_name("Yaw") && is_field_name("a") && !is_field_name("q"));
    }
}
