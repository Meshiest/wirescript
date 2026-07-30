use crate::ast::TypeExpr;
use crate::ir::Type;
use super::TypeMap;

/// The source-language spelling of a type (`*int`, `int[]`, `Map<string, int>`,
/// …). A thin alias over the `Display` impl on [`Type`] (in `crate::ir`), kept
/// as a named helper for the many call sites that read better than `.to_string()`.
pub fn type_str(t: &Type) -> String {
    t.to_string()
}

pub fn type_expr_str(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Name { name, .. } => name.clone(),
        TypeExpr::Ref { inner, .. } => format!("*{}", type_expr_str(inner)),
        TypeExpr::Array { inner, .. } => format!("{}[]", type_expr_str(inner)),
        TypeExpr::Tuple { fields, .. } => {
            let f: Vec<String> = fields.iter().map(type_expr_str).collect();
            format!("({})", f.join(", "))
        }
        TypeExpr::Union { options, .. } => {
            let f: Vec<String> = options.iter().map(type_expr_str).collect();
            f.join(" | ")
        }
        TypeExpr::Record { fields, .. } => {
            let f: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, type_expr_str(&f.typ)))
                .collect();
            format!("{{{}}}", f.join(", "))
        }
        TypeExpr::Generic { name, args, .. } => {
            let a: Vec<String> = args.iter().map(type_expr_str).collect();
            format!("{}<{}>", name, a.join(", "))
        }
    }
}

pub fn infer_expr_type(expr: &crate::ast::Expr, tmap: &TypeMap) -> Option<String> {
    let r = expr.range();
    tmap.get(&(r.file.clone(), r.start.offset, r.end.offset)).map(type_str)
}

/// Map a primitive type name (as produced by [`type_str`]) back to a [`Type`].
/// Complex or unknown names (records, arrays, refs, `any`) return `None`.
pub fn type_from_name(s: &str) -> Option<Type> {
    Some(match s {
        "int" => Type::Int,
        "float" => Type::Float,
        "bool" => Type::Bool,
        "string" => Type::String,
        "entity" => Type::Entity,
        "controller" => Type::Controller,
        "character" => Type::Character,
        "vector" => Type::Vector,
        "rotator" => Type::Rotator,
        "quat" => Type::Quat,
        "color" => Type::Color,
        "brick" => Type::Brick,
        "prefab" => Type::Prefab,
        "zone" => Type::Zone,
        "teleport" => Type::Teleport,
        "exec" => Type::Exec,
        _ => return None,
    })
}

/// Receiver methods applicable to a value of the named primitive type, for `.`
/// member completion. Returns `(name, "(params)")` pairs. A method applies when
/// the value's type is accepted by the receiver without a string-format
/// coercion — so a `string` value shows only string methods, not everything
/// that happens to format into text. Empty for non-primitive/unknown names.
pub fn receiver_methods(type_name: &str) -> Vec<(&'static str, String)> {
    use crate::types::coerce::{coerce, CoerceRule};
    let Some(var_ty) = type_from_name(type_name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, spec) in crate::catalog::calls::calls().iter() {
        let Some(recv) = &spec.receiver else { continue };
        if matches!(coerce(&var_ty, recv), CoerceRule::Same | CoerceRule::Coerce) {
            // Skip the receiver (first) param in the displayed signature.
            let params: Vec<String> = spec
                .params
                .iter()
                .skip(1)
                .map(|p| {
                    if p.optional {
                        format!("{}?", p.name)
                    } else {
                        p.name.to_string()
                    }
                })
                .collect();
            out.push((*name, format!("({})", params.join(", "))));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::receiver_methods;

    #[test]
    fn string_receiver_methods_are_string_only() {
        let names: Vec<&str> = receiver_methods("string").iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"Contains"), "string should have Contains: {names:?}");
        assert!(names.contains(&"Length"), "string should have Length");
        // vector/entity methods must not appear on a string.
        assert!(!names.contains(&"Dot"), "Dot leaked onto string");
        assert!(!names.contains(&"GetAim"), "GetAim leaked onto string");
    }

    #[test]
    fn unknown_type_has_no_methods() {
        assert!(receiver_methods("{ x: int }").is_empty());
        assert!(receiver_methods("nonsense").is_empty());
    }

    #[test]
    fn quat_and_color_receiver_methods() {
        let quat: Vec<&str> = receiver_methods("quat").iter().map(|(n, _)| *n).collect();
        for m in ["ToDirection", "Invert", "AngleTo", "Slerp", "ToAxisAngle"] {
            assert!(quat.contains(&m), "quat should have {m}: {quat:?}");
        }
        let color: Vec<&str> = receiver_methods("color").iter().map(|(n, _)| *n).collect();
        for m in ["ToHex", "ToSRGB", "Blend"] {
            assert!(color.contains(&m), "color should have {m}: {color:?}");
        }
        // A vector exposes the direction→quat conversions but not quat-only ops.
        let vector: Vec<&str> = receiver_methods("vector").iter().map(|(n, _)| *n).collect();
        assert!(vector.contains(&"ToRotation"), "vector should have ToRotation");
        assert!(!vector.contains(&"Slerp"), "Slerp leaked onto vector");
    }
}
