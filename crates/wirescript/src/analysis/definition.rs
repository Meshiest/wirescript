use super::references::find_name_range;
use super::symbols::SymbolDef;
use super::text::word_at;
use crate::ast::*;
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;
use crate::diagnostic::SourceRange;
use crate::resolve::FileLoader;

#[derive(Clone, Debug)]
pub struct Location {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub file: Option<String>,
}

fn source_range_to_location(r: &SourceRange, file: Option<String>) -> Location {
    Location {
        start_line: r.start.line.saturating_sub(1) as usize,
        start_col: r.start.col.saturating_sub(1) as usize,
        end_line: r.end.line.saturating_sub(1) as usize,
        end_col: r.end.col.saturating_sub(1) as usize,
        file,
    }
}

fn cross_file_path(sym: &SymbolDef, current_file: &str) -> Option<String> {
    if !sym.range.file.is_empty() && &*sym.range.file != current_file {
        Some(sym.range.file.to_string())
    } else {
        None
    }
}

pub fn definition_at(
    source: &str,
    pre_resolve_ast: &Script,
    symbols: &[SymbolDef],
    current_file: &str,
    loader: &dyn FileLoader,
    line: usize,
    col: usize,
) -> Option<Location> {
    if let Some(loc) = find_import_definition(source, pre_resolve_ast, current_file, loader, line, col) {
        return Some(loc);
    }

    // Cursor on a `SendCustomEvent("name", ...)` channel-name string → jump to the
    // matching `on CustomEvent("name") -> (...)` receiver in this file.
    if let Some(loc) = custom_event_send_definition(pre_resolve_ast, source, line, col) {
        return Some(loc);
    }

    let word = word_at(source, line, col)?;

    if is_field_access(source, line, col) {
        // Variant use in a construction path (`Shape.Circle`): jump to that
        // variant's own token inside the `enum Shape` declaration. Checked
        // first - a user `enum` is never itself a registered value symbol, so
        // there is nothing else in this block that could resolve it, but a
        // dedicated resolver keeps it out of the namespace/record-field
        // fallbacks below (which look for a VALUE symbol, not a type name).
        if let Some(loc) = resolve_enum_variant_definition(pre_resolve_ast, source, &word, line, col) {
            return Some(loc);
        }
        // Namespace-qualified name (`card.drawCard` with `import * as card`):
        // resolve in the imported file. Checked before the symbol loop so a
        // same-named local decl can't shadow the qualified reference.
        if let Some(loc) = resolve_namespace_definition(
            source, pre_resolve_ast, current_file, loader, &word, line, col,
        ) {
            return Some(loc);
        }
        // Field access on a record value (e.g. cpu.cpsr): resolve the field
        // within the object's record type rather than matching standalone symbols.
        if let Some(loc) = resolve_field_definition(source, symbols, current_file, loader, &word, line, col) {
            return Some(loc);
        }
    }

    // Named payload FIELD key in an enum-variant construction
    // (`Shape.Box { w: 1.0, h: 2.0 }` - the `w`/`h`): jump to that field's own
    // declaration inside `enum Shape { Box { w: float, ... } }`. Not gated by
    // `is_field_access` above - the field key isn't preceded by a `.`.
    if let Some(loc) = resolve_enum_field_construction_definition(pre_resolve_ast, source, line, col) {
        return Some(loc);
    }

    // Named payload FIELD key in a match PATTERN (`Box { w, h }` - the `w`/`h`
    // shorthand capture): best-effort twin of the construction resolver above.
    if let Some(loc) = resolve_enum_pattern_field_definition(pre_resolve_ast, source, line, col) {
        return Some(loc);
    }

    for sym in symbols {
        if sym.name == word {
            let file = cross_file_path(sym, current_file);
            let file_source = file.as_ref().and_then(|_| loader.load(&sym.range.file, current_file).ok());
            let search_source = file_source.as_deref().unwrap_or(source);
            let r = find_name_range(search_source, &sym.range, &sym.name)
                .unwrap_or_else(|| sym.range.clone());
            return Some(source_range_to_location(&r, file));
        }
    }

    // Bare enum TYPE name (`Shape` in `var s: Shape`, or the `Shape` in a
    // `Shape.Circle` path): jump to the `enum Shape` declaration. Checked
    // AFTER the symbol loop so a value binding of the same name still wins
    // (the compiler's own shadow rule). A built-in game/prelude enum
    // (`EasingFunction`, `Option`, ...) has no source location - `find_enum_decl`
    // only matches a real `TopDecl::Enum`, so it falls through to `None` below.
    if let Some(loc) = resolve_enum_type_definition(pre_resolve_ast, source, &word) {
        return Some(loc);
    }

    if find_event(&word).is_some() || calls().get(word.as_str()).is_some() {
        return None;
    }

    None
}

/// Find a top-level `enum` decl named `name` in `decls`, recursing into
/// namespaces (mirrors `hover::find_top_level_enum_decl`).
fn find_enum_decl<'a>(decls: &'a [TopDecl], name: &str) -> Option<&'a EnumDecl> {
    for d in decls {
        match d {
            TopDecl::Enum(e) if e.name == name => return Some(e),
            TopDecl::Namespace(ns) => {
                if let Some(e) = find_enum_decl(&ns.decls, name) {
                    return Some(e);
                }
            }
            _ => {}
        }
    }
    None
}

/// Go-to-definition for a bare enum TYPE name: the `enum Shape` declaration in
/// `ast` (same file only - `ast` is this file's own pre-resolve AST, and a
/// user enum is never imported/re-exported through the namespace machinery
/// the way a mod/chip is). `None` for a built-in game/prelude enum, which has
/// no `TopDecl::Enum` anywhere to find.
fn resolve_enum_type_definition(ast: &Script, source: &str, word: &str) -> Option<Location> {
    let e = find_enum_decl(&ast.decls, word)?;
    let r = find_name_range(source, &e.range, &e.name).unwrap_or_else(|| e.range.clone());
    Some(source_range_to_location(&r, None))
}

/// Go-to-definition for a VARIANT use in a construction path (`Shape.Circle`):
/// the variant's own token inside the `enum Shape` declaration.
/// `EnumVariantDecl::range` gives each variant its own source span (unlike a
/// record field, which has none), so this resolves to the exact variant, not
/// just the enum. `field` is the word under the cursor (the variant name).
fn resolve_enum_variant_definition(
    ast: &Script,
    source: &str,
    field: &str,
    line: usize,
    col: usize,
) -> Option<Location> {
    let l = source.lines().nth(line)?;
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let field_start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    if field_start == 0 || l.as_bytes().get(field_start - 1) != Some(&b'.') {
        return None;
    }
    let dot = field_start - 1;
    let obj_start = l[..dot]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let enum_name = &l[obj_start..dot];
    if enum_name.is_empty() {
        return None;
    }
    let e = find_enum_decl(&ast.decls, enum_name)?;
    let v = e.variants.iter().find(|v| v.name == field)?;
    let r = find_name_range(source, &v.range, &v.name).unwrap_or_else(|| v.range.clone());
    Some(source_range_to_location(&r, None))
}

/// `path` (a `VariantCtor`'s `path` sub-expression) as `(enum_name,
/// variant_name)`, when it has the `Enum.Variant` `FieldAccess` shape the
/// parser always builds it from (see `Expr::VariantCtor`'s doc comment).
/// `None` for any other shape - defensive rather than assumed, in case a
/// future parser change ever produces something else here.
fn variant_ctor_path_names(path: &Expr) -> Option<(&str, &str)> {
    let Expr::FieldAccess { obj, field, .. } = path else {
        return None;
    };
    let Expr::Ident { name, .. } = obj.as_ref() else {
        return None;
    };
    Some((name.as_str(), field.as_str()))
}

/// Byte offset of a `RecordLitField`'s KEY span (`[start, end)`), regardless
/// of whether it's a `name: value` pair or a `name` shorthand. A `Named`
/// field's own `range` covers the WHOLE `key: value` (see
/// `parser::expr::parse_record_lit`), not just the key, so the key's end is
/// derived from the field name's byte length rather than read off a
/// dedicated sub-range. `None` for a `Spread` field (no single key).
fn record_lit_field_key_span(f: &RecordLitField) -> Option<(&str, usize, usize)> {
    match f {
        RecordLitField::Named { name, range, .. } => {
            Some((name.as_str(), range.start.offset, range.start.offset + name.len()))
        }
        RecordLitField::Shorthand { name, range } => Some((name.as_str(), range.start.offset, range.end.offset)),
        RecordLitField::Spread { .. } => None,
    }
}

/// Go-to-definition for a named payload FIELD's KEY in an enum-variant
/// CONSTRUCTION (`Shape.Box { w: 1.0, h: 2.0 }` - the `w`/`h`): jumps to that
/// field's own `SourceRange` inside `enum Shape { Box { w: float, ... } }`.
///
/// AST-based rather than the text-heuristic style the resolvers above use:
/// the enum + variant names are only recoverable from the `VariantCtor`'s own
/// `path` sub-expression, so this walks the AST (via `visit::visit_program`,
/// which fires `on_call` on `Expr::VariantCtor` itself) looking for
/// the field whose key span (see [`record_lit_field_key_span`]) contains the
/// cursor.
fn resolve_enum_field_construction_definition(ast: &Script, source: &str, line: usize, col: usize) -> Option<Location> {
    let off = cursor_byte_offset(source, line, col);
    let mut hit: Option<(&Expr, &str)> = None; // (Enum.Variant path, field name)
    super::visit::visit_program(
        ast,
        &mut |_h| {},
        &mut |e| {
            if hit.is_some() {
                return;
            }
            let Expr::VariantCtor { path, fields, .. } = e else {
                return;
            };
            for f in fields {
                let Some((name, key_start, key_end)) = record_lit_field_key_span(f) else {
                    continue;
                };
                if key_start <= off && off <= key_end {
                    hit = Some((path, name));
                }
            }
        },
    );
    let (path, field_name) = hit?;
    let (enum_name, variant_name) = variant_ctor_path_names(path)?;
    let e = find_enum_decl(&ast.decls, enum_name)?;
    let v = e.variants.iter().find(|v| v.name == variant_name)?;
    let EnumPayloadDecl::Named(decl_fields) = &v.payload else {
        return None;
    };
    let (_, _, range) = decl_fields.iter().find(|(n, _, _)| n == field_name)?;
    let r = find_name_range(source, range, field_name).unwrap_or_else(|| range.clone());
    Some(source_range_to_location(&r, None))
}

/// Go-to-definition for a named payload FIELD's KEY in a match PATTERN
/// (`match s { Box { w, h } => ... }` - the `w`/`h`): jumps to that field's
/// declaration inside the owning `enum`. Best-effort, for two reasons:
///
/// - **Enum resolution.** `definition_at` has no typechecked scrutinee type
///   available here (only `symbols`/`pre_resolve_ast` - no `type_of_expr`
///   map is threaded through), so the owning enum can't be read off the
///   `match`'s scrutinee the way a real type-directed resolver would. Instead
///   this resolves the variant name the same way lowering/const-eval resolve
///   a BARE variant name: [`crate::typecheck::enums::resolve_bare_variant_enum`],
///   which only succeeds when exactly one enum in the file declares a variant
///   with this name. Two enums sharing a variant name makes this genuinely
///   ambiguous without the scrutinee's real type, so it returns `None` rather
///   than guessing.
/// - **Field-key detection.** The parser only keeps a source range for a
///   named pattern-field's bound `Pattern`, not for its key token (see
///   `parser::pattern::parse_pattern`) - and in the SHORTHAND branch
///   (`Box { w, h }`) the bound pattern's range IS the key's own token range
///   (`Pattern::Binding { name: field_tok.text.clone(), range: <field_tok's
///   range> }`). An explicit rebinding (`Box { w: renamed }`) has no AST
///   range for the `w` key at all, so only the shorthand form resolves here.
fn resolve_enum_pattern_field_definition(ast: &Script, source: &str, line: usize, col: usize) -> Option<Location> {
    let off = cursor_byte_offset(source, line, col);
    let mut hit: Option<(&str, &str)> = None; // (variant name, field name)
    super::visit::visit_program(
        ast,
        &mut |_h| {},
        &mut |e| {
            if hit.is_some() {
                return;
            }
            let Expr::MatchExpr { arms, .. } = e else {
                return;
            };
            for arm in arms {
                let Pattern::Variant {
                    variant,
                    sub: VariantPattern::Named { fields, .. },
                    ..
                } = &arm.pattern
                else {
                    continue;
                };
                for (key, sub) in fields {
                    let Pattern::Binding { name, range } = sub else {
                        continue;
                    };
                    if name != key {
                        continue; // an explicit rebind (`w: renamed`) has no key range to click.
                    }
                    if range.start.offset <= off && off <= range.end.offset {
                        hit = Some((variant.as_str(), key.as_str()));
                    }
                }
            }
        },
    );
    let (variant_name, field_name) = hit?;

    let registry = crate::typecheck::enums::build_registry(&ast.decls);
    let enum_name = crate::typecheck::enums::resolve_bare_variant_enum(&registry, variant_name, |_| false)?;
    let e = find_enum_decl(&ast.decls, enum_name)?;
    let v = e.variants.iter().find(|v| v.name == variant_name)?;
    let EnumPayloadDecl::Named(decl_fields) = &v.payload else {
        return None;
    };
    let (_, _, range) = decl_fields.iter().find(|(n, _, _)| n == field_name)?;
    let r = find_name_range(source, range, field_name).unwrap_or_else(|| range.clone());
    Some(source_range_to_location(&r, None))
}

fn find_import_definition(
    source: &str,
    ast: &Script,
    current_file: &str,
    loader: &dyn FileLoader,
    line: usize,
    col: usize,
) -> Option<Location> {
    let cursor_line = (line + 1) as u32;
    // The specifier under the cursor. A named import lists many bindings on one
    // (possibly multi-line) statement, so the column is what disambiguates
    // WHICH one is clicked — without it, every click resolved to the first
    // binding that happened to have a matching decl (e.g. `onCheckpoint`
    // jumping to an earlier `doReset`).
    let cursor_word = word_at(source, line, col);

    for d in &ast.decls {
        let TopDecl::Import(imp) = d else { continue };
        if cursor_line < imp.range.start.line || cursor_line > imp.range.end.line {
            continue;
        }

        let resolved_path = loader.canonical_path(&imp.path, current_file);
        let import_path = if resolved_path.ends_with(".ws") {
            resolved_path
        } else {
            format!("{}.ws", imp.path)
        };

        if let ImportKind::Named(bindings) = &imp.kind {
            if let Ok(file_src) = loader.load(&imp.path, current_file) {
                let target_ast = crate::parse(&file_src, &import_path);
                for b in bindings {
                    // Only the binding the cursor is actually on. For
                    // `orig as alias`, clicking either name resolves `orig`'s
                    // decl. If the cursor isn't on a specifier (the path, a
                    // keyword, whitespace), no binding matches and we fall
                    // through to the file location below.
                    let surface = b.alias.as_deref().unwrap_or(&b.name);
                    if cursor_word.as_deref() != Some(surface)
                        && cursor_word.as_deref() != Some(b.name.as_str())
                    {
                        continue;
                    }
                    for td in &target_ast.ast.decls {
                        if top_decl_name(td) == Some(&b.name) {
                            let r = find_name_range(&file_src, td.range(), &b.name)
                                .unwrap_or_else(|| td.range().clone());
                            return Some(source_range_to_location(&r, Some(import_path.clone())));
                        }
                    }
                    // The clicked specifier's decl kind isn't resolvable here
                    // (e.g. an imported `var`); stop rather than falling on to a
                    // later binding, and jump to the file below.
                    break;
                }
            }
        }

        return Some(Location {
            start_line: 0, start_col: 0, end_line: 0, end_col: 0,
            file: Some(import_path),
        });
    }
    None
}

fn is_field_access(source: &str, line: usize, col: usize) -> bool {
    let Some(l) = source.lines().nth(line) else { return false };
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    start > 0 && l.as_bytes().get(start - 1) == Some(&b'.')
}

/// The referenceable name a top-level declaration binds, if any. Covers every
/// importable / namespaced member kind so go-to-definition resolves to the
/// specifier's own decl — not just `mod`/`chip`/`let`, but also `var`/`array`/
/// `map`/`buffer`, root `in`/`out` ports, and `type` aliases.
fn top_decl_name(td: &TopDecl) -> Option<&str> {
    match td {
        TopDecl::Chip(c) => Some(&c.name),
        TopDecl::Fn(f) => Some(&f.name),
        TopDecl::Let(l) => match &l.binding {
            LetBinding::Ident { name, .. } => Some(name),
            _ => None,
        },
        TopDecl::Event(e) => Some(&e.name),
        TopDecl::Var(v) => Some(&v.name),
        TopDecl::Array(a) => Some(&a.name),
        TopDecl::Map(m) => Some(&m.name),
        TopDecl::Buffer(b) => Some(&b.name),
        TopDecl::In(i) => Some(&i.name),
        TopDecl::Out(o) => Some(&o.name),
        TopDecl::TypeAlias(t) => Some(&t.name),
        _ => None,
    }
}

/// Definition of `ns.name` where `ns` is a star-import alias
/// (`import * as ns from "file"`): the decl named `name` in that file.
fn resolve_namespace_definition(
    source: &str,
    ast: &Script,
    current_file: &str,
    loader: &dyn FileLoader,
    name: &str,
    line: usize,
    col: usize,
) -> Option<Location> {
    // Identifier immediately before the `.` the cursor's word follows.
    let l = source.lines().nth(line)?;
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let field_start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    if field_start == 0 || l.as_bytes().get(field_start - 1) != Some(&b'.') {
        return None;
    }
    let dot = field_start - 1;
    let obj_start = l[..dot]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ns = &l[obj_start..dot];
    if ns.is_empty() {
        return None;
    }

    for d in &ast.decls {
        let TopDecl::Import(imp) = d else { continue };
        let ImportKind::Namespace(alias) = &imp.kind else {
            continue;
        };
        if alias != ns {
            continue;
        }
        let file_src = loader.load(&imp.path, current_file).ok()?;
        let resolved_path = loader.canonical_path(&imp.path, current_file);
        let import_path = if resolved_path.ends_with(".ws") {
            resolved_path
        } else {
            format!("{}.ws", imp.path)
        };
        let target_ast = crate::parse(&file_src, &import_path);
        for td in &target_ast.ast.decls {
            if top_decl_name(td) == Some(name) {
                let r = find_name_range(&file_src, td.range(), name)
                    .unwrap_or_else(|| td.range().clone());
                return Some(source_range_to_location(&r, Some(import_path)));
            }
        }
        // The alias matched but the member doesn't exist in that file:
        // report nothing rather than letting a same-named local decl
        // swallow the jump.
        return None;
    }
    None
}

fn resolve_field_definition(
    source: &str,
    symbols: &[SymbolDef],
    current_file: &str,
    loader: &dyn FileLoader,
    field: &str,
    line: usize,
    col: usize,
) -> Option<Location> {
    let l = source.lines().nth(line)?;
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let field_start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    if field_start == 0 || l.as_bytes().get(field_start - 1) != Some(&b'.') {
        return None;
    }
    let dot = field_start - 1;
    let obj_start = l[..dot]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let obj_name = &l[obj_start..dot];
    if obj_name.is_empty() {
        return None;
    }

    let obj_sym = symbols.iter().find(|s| s.name == obj_name)?;
    let ty_name = obj_sym.ty.as_deref()?;
    let type_sym = symbols.iter().find(|s| s.kind == "type" && s.name == ty_name)?;
    let file = cross_file_path(type_sym, current_file);
    let type_source = file.as_ref().and_then(|_| loader.load(&type_sym.range.file, current_file).ok());
    let search_src = type_source.as_deref().unwrap_or(source);

    let start_line = type_sym.range.start.line.saturating_sub(1) as usize;
    let end_line = (type_sym.range.end.line as usize).min(search_src.lines().count());
    for line_idx in start_line..end_line {
        if let Some(line_str) = search_src.lines().nth(line_idx) {
            if let Some(pos) = line_str.find(field) {
                let before = if pos > 0 { line_str.as_bytes()[pos - 1] } else { b' ' };
                let after = line_str.as_bytes().get(pos + field.len()).copied().unwrap_or(b' ');
                if !before.is_ascii_alphanumeric() && before != b'_'
                    && !after.is_ascii_alphanumeric() && after != b'_'
                {
                    return Some(Location {
                        start_line: line_idx, start_col: pos,
                        end_line: line_idx, end_col: pos + field.len(),
                        file,
                    });
                }
            }
        }
    }

    // Fallback: jump to the type declaration itself
    Some(source_range_to_location(&type_sym.range, file))
}

// ---------- custom-event send-site → receiver navigation ----------

/// Byte offset of the cursor, matching the lexer's source-offset convention
/// (the same one `analysis::text` uses for other cursor queries).
fn cursor_byte_offset(source: &str, line: usize, col: usize) -> usize {
    let line_start: usize = source.lines().take(line).map(|l| l.len() + 1).sum();
    let line_str = source.lines().nth(line).unwrap_or("");
    let bc = line_str
        .char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(line_str.len());
    line_start + bc
}

/// If `call` is a `SendCustomEvent(...)` / `SendGlobalCustomEvent(...)` send and
/// `off` sits on its channel-name string literal, return that channel name plus
/// the RECEIVER trigger word (`CustomEvent` / `GlobalCustomEvent`) — the two are
/// separate channel namespaces, so a send only jumps to its own kind of receiver.
fn send_event_name_at<'a>(call: &'a Expr, off: usize) -> Option<(&'a str, &'static str)> {
    let Expr::Call { callee, args, .. } = call else {
        return None;
    };
    // Both `SendCustomEvent("x", ...)` and the receiver form
    // `entity.SendCustomEvent("x", ...)` carry the channel name in their args.
    let send = match callee.as_ref() {
        Expr::Ident { name, .. } => name.as_str(),
        Expr::FieldAccess { field, .. } => field.as_str(),
        _ => return None,
    };
    let trigger_word = match send {
        "SendCustomEvent" => "CustomEvent",
        "SendGlobalCustomEvent" => "GlobalCustomEvent",
        _ => return None,
    };
    // The channel name is a named `eventName = ...`, else the first positional arg.
    let name_expr = args
        .iter()
        .find_map(|a| match a {
            CallArg::Named { name, value, .. } if name == "eventName" => Some(value),
            _ => None,
        })
        .or_else(|| {
            args.iter().find_map(|a| match a {
                CallArg::Positional(e) => Some(e),
                _ => None,
            })
        })?;
    match name_expr {
        Expr::StringLit { value, range }
            if range.start.offset <= off && off <= range.end.offset =>
        {
            Some((value, trigger_word))
        }
        _ => None,
    }
}

/// The channel-name string-literal range of a handler that is a `trigger_word`
/// (`CustomEvent` / `GlobalCustomEvent`) receiver for `name`, if it is one.
fn receiver_name_range(h: &Handler, name: &str, trigger_word: &str) -> Option<SourceRange> {
    if !matches!(&h.trigger, Trigger::Ident { name: n, .. } if n == trigger_word) {
        return None;
    }
    h.config.iter().find_map(|c| match c {
        HandlerConfigArg::Positional(Expr::StringLit { value, range }) if value == name => {
            Some(range.clone())
        }
        _ => None,
    })
}

fn custom_event_send_definition(
    ast: &Script,
    source: &str,
    line: usize,
    col: usize,
) -> Option<Location> {
    let off = cursor_byte_offset(source, line, col);
    // 1. Resolve the channel name under the cursor (must be a SendCustomEvent
    //    channel-name string).
    let mut name: Option<(String, &'static str)> = None;
    super::visit::visit_program(
        ast,
        &mut |_h| {},
        &mut |call| {
            if name.is_none()
                && let Some((n, tw)) = send_event_name_at(call, off)
            {
                name = Some((n.to_string(), tw));
            }
        },
    );
    let (name, trigger_word) = name?;
    // 2. Find the matching receiver handler in the SAME namespace in this file.
    let mut target: Option<SourceRange> = None;
    super::visit::visit_program(
        ast,
        &mut |h| {
            if target.is_none()
                && let Some(r) = receiver_name_range(h, &name, trigger_word)
            {
                target = Some(r);
            }
        },
        &mut |_c| {},
    );
    target.map(|r| source_range_to_location(&r, None))
}

#[cfg(test)]
mod tests;
