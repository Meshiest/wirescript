use super::TypeMap;
use crate::ast::TypeExpr;
use crate::ir::Type;

/// The source spelling of a type (`*int`, `int[]`, `Map<string, int>`): the
/// `Display` of [`Type`] (in `crate::ir`), named for the many call sites that
/// read better than `.to_string()`.
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
    tmap.get(&r.key()).map(type_str)
}

/// Map a primitive type name (as produced by [`type_str`]) back to a [`Type`].
/// Complex or unknown names (records, arrays, refs, `any`, `never`) return
/// `None`. Delegates to the crate's single primitive-name table
/// (`types::resolve::primitive`), filtering out `any`/`never` to preserve
/// this function's narrower "real, storable primitive" contract.
pub fn type_from_name(s: &str) -> Option<Type> {
    crate::types::resolve::primitive(s).filter(|t| !matches!(t, Type::Opaque | Type::Never))
}

/// Receiver methods for a value of the named primitive type, for `.` member
/// completion, as `(name, "(params)")` pairs. A method applies when the receiver
/// accepts the value's type without a string-format coercion, so a `string`
/// shows only string methods. Empty for a non-primitive or unknown name.
pub fn receiver_methods(type_name: &str) -> Vec<(&'static str, String)> {
    use crate::types::coerce::{CoerceRule, coerce};
    let Some(var_ty) = type_from_name(type_name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, spec) in crate::catalog::calls::calls() {
        let Some(recv) = &spec.receiver else { continue };
        if matches!(coerce(&var_ty, recv), CoerceRule::Same | CoerceRule::Coerce) {
            // Hide the receiver-bound param from the displayed signature: the
            // first param normally, or the named `target` param for a
            // named-target receiver (`entity.SendCustomEvent(...)`).
            let skip_name = spec.receiver_target_param();
            let params: Vec<String> = spec
                .params
                .iter()
                .enumerate()
                .filter(|(i, p)| match skip_name {
                    Some(tp) => p.name != tp,
                    None => *i != 0,
                })
                .map(|(_, p)| {
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

/// Split a rendered mod or chip signature (`"<G>(p0: T0, p1: T1) -> R"`) into
/// its first parameter's `(name, type)` and the remaining parameters as one
/// comma-separated string. Nesting-aware, so record, tuple and generic types
/// (`{ x: int }`, `(a, b)`, `Map<K, V>`) split correctly. `None` without
/// parameters.
fn split_signature_params(sig: &str) -> Option<(&str, &str, String)> {
    let bytes = sig.as_bytes();
    let mut i = 0;
    // Skip a leading `<...>` generic prefix.
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

/// The in-scope user `self` mods and chips callable as receiver methods on a
/// value of the named type, for `.` completion: the user counterpart of
/// [`receiver_methods`]. One applies when its first parameter is `self` and that
/// type equals `type_name` or coerces as a builtin receiver would (`Same` or
/// `Coerce`, never a string-format coercion). A generic `self: T` is not offered,
/// since its type is unknown at completion time. Pairs are
/// `(name, "(params)")` with `self` left out.
pub fn user_receiver_methods(
    type_name: &str,
    symbols: &[crate::analysis::SymbolDef],
) -> Vec<(String, String)> {
    use crate::types::coerce::{CoerceRule, coerce};
    let recv_prim = type_from_name(type_name);
    let mut out = Vec::new();
    for sym in symbols {
        if !matches!(sym.kind, "mod" | "chip" | "fn") {
            continue;
        }
        // Namespaced members (`ns.name`) are never receiver-method calls.
        if sym.name.contains('.') {
            continue;
        }
        let Some(sig) = sym.ty.as_deref() else {
            continue;
        };
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

/// The kind of collection a receiver's declared type is, which picks the method
/// table for `.method` hover and completion. Element types do not matter there,
/// so there is no payload; hover shows the declared type string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Array,
    Map,
}

/// A receiver's declared type string as a [`CollectionKind`], following
/// `type Name = <body>` aliases from the `type` symbols (bounded, so an alias
/// cycle ends). Reads both `T[]` and `Array<T>`/`Map<K, V>`, and a generic
/// alias instance (`Grid<int>`) by its base name, since only the shape matters.
/// `None` for any other type.
pub fn collection_kind(ty: &str, symbols: &[crate::analysis::SymbolDef]) -> Option<CollectionKind> {
    let mut cur = ty.trim().to_string();
    for _ in 0..16 {
        let c = cur.trim();
        if c.ends_with("[]") || c.starts_with("Array<") {
            return Some(CollectionKind::Array);
        }
        if c.starts_with("Map<") {
            return Some(CollectionKind::Map);
        }
        // A bare name (`Scores`) or a generic-alias instance (`Grid<int>`): follow
        // the matching `type` alias by its base name and continue with its body.
        let base = c.split_once('<').map_or(c, |(b, _)| b.trim());
        let body = symbols
            .iter()
            .find(|s| s.kind == "type" && s.name == base)
            .and_then(|s| s.ty.clone())?;
        cur = body;
    }
    None
}

#[cfg(test)]
mod tests;
