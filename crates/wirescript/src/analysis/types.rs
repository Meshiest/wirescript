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
            // Hide the receiver-bound param from the displayed signature: the
            // first param normally, or the named `target` param for a
            // named-target receiver (`entity.SendCustomEvent(…)`).
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

/// Split a rendered mod/chip signature — `"<G>(p0: T0, p1: T1, …) -> R"` — into
/// its first parameter's `(name, type)` plus the remaining params rendered back
/// as a comma-separated string. Depth-aware so record/tuple/generic parameter
/// types (`{ x: int }`, `(a, b)`, `Map<K, V>`) don't confuse the split.
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

/// What kind of collection a receiver's declared type resolves to. Drives
/// `.method` hover and completion dispatch onto the right method table. The
/// concrete element types aren't needed for dispatch (both wire to opaque
/// gates), so no payload — hovers display the receiver's declared type string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Array,
    Map,
}

/// Resolve a receiver's declared type STRING to a [`CollectionKind`], following
/// `type Name = <body>` aliases recorded as `type` symbols (bounded to guard
/// against alias cycles). Recognizes both the postfix (`T[]`) and generic
/// (`Array<T>`/`Map<K, V>`) spellings, and resolves a generic-alias instance
/// like `Grid<int>` by its BASE name (`Grid`) — only the collection SHAPE
/// matters here, so the alias body's still-unsubstituted params are irrelevant.
/// Returns `None` for any non-collection type.
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
