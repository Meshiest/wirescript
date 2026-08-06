use crate::ast::TypeExpr;
use crate::ir::Type;
use super::TypeMap;

/// The source-language spelling of a type (`*int`, `int[]`, `Dict<string, int>`,
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
/// Complex or unknown names (records, arrays, refs, `any`, `never`) return
/// `None`. Delegates to the crate's single primitive-name table
/// (`types::resolve::primitive`), filtering out `any`/`never` to preserve
/// this function's narrower "real, storable primitive" contract.
pub fn type_from_name(s: &str) -> Option<Type> {
    crate::types::resolve::primitive(s).filter(|t| !matches!(t, Type::Opaque | Type::Never))
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

/// Split a rendered mod/chip signature — `"<G>(p0: T0, p1: T1, …) -> R"` — into
/// its first parameter's `(name, type)` plus the remaining params rendered back
/// as a comma-separated string. Depth-aware so record/tuple/generic parameter
/// types (`{ x: int }`, `(a, b)`, `Dict<K, V>`) don't confuse the split.
/// Returns `None` when there is no parameter list or it is empty.
fn split_signature_params(sig: &str) -> Option<(&str, &str, String)> {
    let bytes = sig.as_bytes();
    let mut i = 0;
    // Skip a leading `<…>` generic prefix.
    if bytes.first() == Some(&b'<') {
        let mut depth = 0i32;
        while i < bytes.len() {
            match bytes[i] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    // Advance to the param-list `(`.
    while i < bytes.len() && bytes[i] != b'(' {
        i += 1;
    }
    let open = i;
    if open >= bytes.len() {
        return None;
    }
    // Find its matching `)` (tracking every bracket kind).
    let mut depth = 0i32;
    let mut close = None;
    for (j, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(j);
                    break;
                }
            }
            _ => {}
        }
    }
    let params = &sig[open + 1..close?];
    if params.trim().is_empty() {
        return None;
    }
    // First param up to the top-level comma; the remainder is the rest.
    let comma = top_level_index(params, b',');
    let (first, rest) = match comma {
        Some(c) => (&params[..c], params[c + 1..].trim().to_string()),
        None => (params, String::new()),
    };
    let colon = top_level_index(first, b':')?;
    Some((first[..colon].trim(), first[colon + 1..].trim(), rest))
}

/// Byte index of the first `needle` in `s` at bracket depth 0, if any.
fn top_level_index(s: &str, needle: u8) -> Option<usize> {
    let mut depth = 0i32;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        match b {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            _ if b == needle && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// In-scope user `self`-mods/chips callable as receiver methods on a value of
/// the named type, for `.` member completion — the user-defined analog of
/// [`receiver_methods`]. A mod/chip opts in when its first parameter is named
/// `self`; it applies when the `self` type equals `type_name` or coerces the
/// same way a builtin receiver-method would (`Same`/`Coerce`, never a
/// string-format coercion). A generic `self: T` receiver is not offered here —
/// its concrete type is unknown at completion time. Returns `(name, "(params)")`
/// pairs with the `self` parameter dropped from the displayed signature.
pub fn user_receiver_methods(
    type_name: &str,
    symbols: &[crate::analysis::SymbolDef],
) -> Vec<(String, String)> {
    use crate::types::coerce::{coerce, CoerceRule};
    let recv_prim = type_from_name(type_name);
    let mut out = Vec::new();
    for sym in symbols {
        if sym.kind != "mod" && sym.kind != "chip" {
            continue;
        }
        // Namespaced members (`ns.name`) are never receiver-method calls.
        if sym.name.contains('.') {
            continue;
        }
        let Some(sig) = sym.ty.as_deref() else { continue };
        let Some((first_name, first_ty, rest)) = split_signature_params(sig) else {
            continue;
        };
        if first_name != "self" {
            continue;
        }
        let applies = first_ty == type_name
            || match (recv_prim.as_ref(), type_from_name(first_ty)) {
                (Some(rv), Some(pt)) => {
                    matches!(coerce(rv, &pt), CoerceRule::Same | CoerceRule::Coerce)
                }
                _ => false,
            };
        if applies {
            out.push((sym.name.clone(), format!("({rest})")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{receiver_methods, split_signature_params};

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

    #[test]
    fn split_signature_params_handles_generics_records_and_returns() {
        // Plain params + return.
        let (n, t, rest) = split_signature_params("(self: vector, o: vector) -> float").unwrap();
        assert_eq!((n, t, rest.as_str()), ("self", "vector", "o: vector"));
        // Leading generic prefix is skipped; single param leaves an empty rest.
        let (n, t, rest) = split_signature_params("<T>(self: T) -> T").unwrap();
        assert_eq!((n, t, rest.as_str()), ("self", "T", ""));
        // Nested commas/colons inside a record or generic type stay with param 0.
        let (n, t, rest) =
            split_signature_params("(self: { x: int, y: int }, k: Dict<string, int>)").unwrap();
        assert_eq!(n, "self");
        assert_eq!(t, "{ x: int, y: int }");
        assert_eq!(rest, "k: Dict<string, int>");
        // No params → None.
        assert!(split_signature_params("() -> int").is_none());
    }

    #[test]
    fn user_receiver_methods_match_by_type() {
        use super::user_receiver_methods;
        use crate::analysis::symbols::collect_symbols_for_file;
        use crate::resolve::{resolve, FsLoader};
        let src = "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
                   mod inc(self: int) -> int { return self + 1 }\n\
                   mod plain(a: vector) -> float { return 0.0 }\n";
        let resolved = resolve(src, "test", &FsLoader);
        let tc = crate::typecheck::typecheck(&resolved.ast, "test");
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));

        let on_vec: Vec<String> = user_receiver_methods("vector", &symbols)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(on_vec.contains(&"dist".to_string()), "vector should offer dist: {on_vec:?}");
        assert!(!on_vec.contains(&"inc".to_string()), "int-receiver inc must not appear on vector");
        assert!(!on_vec.contains(&"plain".to_string()), "a non-self mod must never appear");

        let on_int: Vec<String> = user_receiver_methods("int", &symbols)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(on_int.contains(&"inc".to_string()), "int should offer inc: {on_int:?}");
        assert!(!on_int.contains(&"dist".to_string()), "vector-receiver dist must not appear on int");
    }
}
