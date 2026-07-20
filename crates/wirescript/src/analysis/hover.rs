use crate::collections::HashMap;
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;
use crate::ir::Type;
use super::{TypeMap, IfContextMap, VarReadContextMap};
use super::types::type_str;
use super::text::{word_at, find_enclosing_call};
use super::symbols::SymbolDef;
use super::gate_docs::gate_docs;
use super::resource_estimate::{ResourceEstimate, lookup_estimate};

enum EstimateKind { Chip, Mod, Scope }

/// Byte offset of the start of `line` within `source`.
/// Each prior line contributes `len + 1` bytes (content + newline).
fn line_offset_at(source: &str, line: usize) -> usize {
    source.lines().take(line).map(|ln| ln.len() + 1).sum()
}

/// Given a line string and a column, find the byte offset of the start of the
/// word containing that column (word chars: alphanumeric or `_`).
fn word_start_in_line(line_str: &str, col: usize) -> usize {
    let c = col.min(line_str.len());
    line_str[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0)
}

fn format_estimate(est: &ResourceEstimate, kind: EstimateKind) -> String {
    let chips = match kind {
        EstimateKind::Chip => est.total_microchips + 1,
        _ => est.total_microchips,
    };
    let mut parts = vec![
        format!("~{} gates", est.gates),
        format!("{} chips", chips),
    ];
    if matches!(kind, EstimateKind::Mod) {
        parts.push("inlined per call".into());
    }
    format!("*{}*", parts.join(", "))
}

pub fn hover_at(
    source: &str,
    file: &str,
    symbols: &[SymbolDef],
    type_map: &TypeMap,
    doc_comments: &HashMap<usize, String>,
    if_contexts: &IfContextMap,
    var_read_contexts: &VarReadContextMap,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
    col: usize,
) -> Option<String> {
    // `$` references (prefab files / external assets) aren't identifier words,
    // so detect them from the raw line before the word-based lookups.
    if let Some(h) = hover_asset_ref(source, file, line, col) {
        return Some(h);
    }

    let word = word_at(source, line, col)?;

    None
        .or_else(|| hover_if_keyword(source, file, &word, if_contexts, resource_estimates, line, col))
        .or_else(|| hover_named_param(source, &word, line, col))
        .or_else(|| hover_array_method(source, &word, line, col))
        .or_else(|| hover_builtin_event(&word))
        .or_else(|| hover_builtin_call(source, &word, line, col))
        .or_else(|| hover_chip_or_mod_keyword(source, &word, symbols, resource_estimates, line))
        .or_else(|| hover_on_keyword(source, &word, resource_estimates, line))
        .or_else(|| hover_record_or_type_field(source, symbols, doc_comments, &word, line, col))
        .or_else(|| hover_namespace_member(source, symbols, doc_comments, resource_estimates, &word, line, col))
        .or_else(|| resolve_field_hover(source, file, type_map, symbols, line, col, &word))
        .or_else(|| hover_user_symbol(source, file, symbols, doc_comments, var_read_contexts, resource_estimates, &word, line, col))
}

/// Hover for a `$` reference token under the cursor: a prefab file reference
/// (`$./rel.brz`, `$/abs.brz`) or an external asset reference (`$Type/Name`).
/// Scans the raw line for the `$`-prefixed token spanning the cursor, since
/// the `$`, `/`, and `.` chars aren't part of identifier words.
fn hover_asset_ref(source: &str, file: &str, line: usize, col: usize) -> Option<String> {
    let r = super::text::asset_ref_at(source, line, col)?;
    Some(if r.is_file() {
        render_prefab_file_hover(&r.path, file)
    } else {
        render_asset_hover(&r.path)
    })
}

/// Markdown hover for a prefab file reference (`$./x.brz` / `$/abs.brz`),
/// resolving the path the same way [`crate::compile::disk_prefab_resolver`]
/// does and (natively) reporting whether the file is present.
fn render_prefab_file_hover(path: &str, file: &str) -> String {
    use std::path::{Path, PathBuf};
    let base = Path::new(file).parent();
    let resolved: PathBuf = if let Some(rel) = path.strip_prefix("./") {
        base.map_or_else(|| PathBuf::from(rel), |b| b.join(rel))
    } else if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        base.map_or_else(|| PathBuf::from(path), |b| b.join(path))
    };

    let mut out = String::from("**Prefab file reference**\n\nEmbeds a `.brz` archive into `SpawnPrefab`.\n\n");
    out += &format!("- Reference: `${path}`\n");
    out += &format!("- Resolves to: `{}`\n", resolved.display());
    if !path.ends_with(".brz") {
        out += "\n⚠️ Prefab references must end in `.brz` (WS019).\n";
    }
    #[cfg(not(target_arch = "wasm32"))]
    match std::fs::metadata(&resolved) {
        Ok(m) => out += &format!("- On disk: {} bytes\n", m.len()),
        Err(_) => out += "- ⚠️ Not found on disk\n",
    }
    out
}

/// Markdown hover for an external asset reference (`$Type/Name`).
fn render_asset_hover(path: &str) -> String {
    let mut out = String::from(
        "**Asset reference**\n\nAn external Brickadia asset, inlined into the gate's data.\n\n",
    );
    if let Some((ty, name)) = path.split_once('/') {
        out += &format!("- Type: `{ty}`\n- Name: `{name}`\n");
    } else {
        out += &format!("- Asset: `{path}`\n");
    }
    out
}

/// `if` keyword: show exec (Branch gate) vs pure (Select gate) context.
fn hover_if_keyword(
    source: &str,
    file: &str,
    word: &str,
    if_contexts: &IfContextMap,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
    col: usize,
) -> Option<String> {
    if word != "if" { return None; }

    let offset = line_offset_at(source, line) + word_start_in_line(source.lines().nth(line)?, col);
    let f: std::sync::Arc<str> = file.into();
    let &is_exec = if_contexts.get(&(f, offset))?;

    let mut hover = if is_exec {
        "```wirescript\nif (exec) → Branch gate\n```\nExec-context conditional. Produces an **Exec_Branch** gate that routes the exec chain to the true or false arm.".to_string()
    } else {
        "```wirescript\nif (pure) → Select gate\n```\nPure-context conditional. Produces a **Select** gate that picks one of two values based on the condition.".to_string()
    };
    if let Some(est) = resource_estimates.get(&format!("@{offset}")) {
        hover += &format!("\n\n{}", format_estimate(est, EstimateKind::Scope));
    }
    Some(hover)
}

/// Named parameter inside a builtin call (e.g. `delay` in `Sleep(_, delay = 1.0)`).
/// Only fires in arg-name position — the word followed by a single `=` — so a
/// value expression that shares a param's name (`delay = delay`) hovers as the
/// symbol it is, not as the param docs.
fn hover_named_param(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_named_arg_name(source, line, col) {
        return None;
    }
    let call_name = find_enclosing_call(source, line, col)?;
    let spec = calls().get(call_name.as_str())?;
    let p = spec.params.iter().find(|p| p.name == word)?;

    let gdocs = gate_docs();
    let gate_doc = gdocs.get(spec.gate_class);
    let port_doc = gate_doc.and_then(|g| g.inputs.get(p.port.as_str()));
    let display = port_doc.map(|pd| pd.display_name.as_str()).unwrap_or(p.name);
    let tooltip = port_doc.map(|pd| pd.tooltip.as_str()).unwrap_or("");

    let mut v = format!("**{}** `{}: {}`", display, p.name, type_str(&p.ty));
    if p.optional { v += " *(optional)*"; }
    if !tooltip.is_empty() { v += &format!("\n\n{}", tooltip); }
    Some(v)
}

/// Is the hovered word in named-argument-name position — followed (modulo
/// spaces) by a single `=` (not `==`)? Inside call parens `name = value` can
/// only be a named arg, while a value identifier is never followed by a bare
/// `=`, so this cleanly separates the two sides of `delay = delay`.
fn word_is_named_arg_name(source: &str, line: usize, col: usize) -> bool {
    let Some(l) = source.lines().nth(line) else {
        return false;
    };
    let c = col.min(l.len());
    let word_end = l[c..]
        .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| c + i)
        .unwrap_or(l.len());
    let rest = l[word_end..].trim_start();
    rest.starts_with('=') && !rest.starts_with("==")
}

/// Array methods like `push`, `pop`, `length`, etc. Only fires on a `.method`
/// access (the hovered word is immediately preceded by `.`), so a user symbol
/// that happens to share a method name — e.g. `var sum = 0` — still hovers as
/// itself rather than as `array.sum`.
fn hover_array_method(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    let l = source.lines().nth(line)?;
    let start = word_start_in_line(l, col);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let m = crate::catalog::arrays::ARRAY_METHODS
        .iter()
        .find(|m| m.name == word)?;
    Some(format!("**array.{}**\n\n{}{} - {}", m.name, m.name, m.signature, m.doc))
}

/// Built-in event names like `RoundStart`, `CharacterSpawned`, etc.
fn hover_builtin_event(word: &str) -> Option<String> {
    let evt = find_event(word)?;
    let params: Vec<String> = evt.data.iter().map(|d| format!("{}: {}", d.name, type_str(&d.ty))).collect();
    let sig = if params.is_empty() { String::new() } else { format!("({})", params.join(", ")) };
    Some(format!("```wirescript\non {}{}\n```", evt.surface_name, sig))
}

/// Is the hovered word actually being used as a call or method access — i.e.
/// preceded by `.` (`recv.method`) or immediately followed by `(` (`call(...)`)?
/// Call/method hovers only fire in these positions, so a plain identifier that
/// merely shares a builtin's name (`var Teleport = 0`) hovers as itself.
fn word_is_call_or_method(source: &str, line: usize, col: usize) -> bool {
    let Some(l) = source.lines().nth(line) else {
        return false;
    };
    let start = word_start_in_line(l, col);
    // Method access: the word is preceded by `.`.
    if start > 0 && l.as_bytes()[start - 1] == b'.' {
        return true;
    }
    // Call position: the next non-space char after the word is `(`.
    let c = col.min(l.len());
    let word_end = l[c..]
        .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| c + i)
        .unwrap_or(l.len());
    l[word_end..].trim_start().starts_with('(')
}

/// Built-in function/gate calls like `Sleep`, `SetLocation`, etc.
/// Title and description for a builtin whose *gate* documentation does not
/// describe what the builtin is for. `Opaque` is the plain Rerouter gate, so
/// the catalog blurb ("a node wires can be routed through") says nothing about
/// the fold and type behaviour that is the entire point of calling it.
fn call_doc_override(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "Opaque" => Some((
            "Opaque",
            "Passes `value` through a rerouter unchanged. Two effects, both deliberate:\n\n\
             - **Hidden from constant folding** — the value stays a live wire, so a probe \
             circuit measures the gate's real behaviour instead of a folded constant.\n\
             - **Type erased for operator resolution** — `Opaque(a) + Opaque(b)` type-checks \
             for combinations that are otherwise rejected (`string + int`), which is how the \
             gate-semantics probes record what the hardware actually does.\n\n\
             The result is untyped, so use the plain value wherever you do not need those two \
             effects.",
        )),
        _ => None,
    }
}

fn hover_builtin_call(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_call_or_method(source, line, col) {
        return None;
    }
    let spec = calls().get(word)?;
    let gdocs = gate_docs();
    let gate_doc = gdocs.get(spec.gate_class);
    let override_doc = call_doc_override(spec.name);
    let title = override_doc
        .map(|(t, _)| t)
        .or_else(|| gate_doc.map(|g| g.display_name.as_str()))
        .unwrap_or(spec.name);

    let mut params: Vec<String> = Vec::new();
    if spec.exec { params.push("exec".into()); }
    params.extend(spec.params.iter().map(|p| {
        if p.optional { format!("{}?: {}", p.name, type_str(&p.ty)) } else { format!("{}: {}", p.name, type_str(&p.ty)) }
    }));

    let out = match spec.outputs.len() {
        0 => String::new(),
        1 => format!(" -> {}", type_str(&spec.outputs[0].ty)),
        _ => format!(" -> ({})", spec.outputs.iter().map(|o| format!("{}: {}", o.port.as_str(), type_str(&o.ty))).collect::<Vec<_>>().join(", ")),
    };

    let mut parts = vec![format!("### {}\n```wirescript\n{}({}){}\n```", title, spec.name, params.join(", "), out)];
    if let Some((_, doc)) = override_doc {
        parts.push(doc.to_string());
        return Some(parts.join("\n\n"));
    }
    if let Some(g) = gate_doc {
        if !g.description.is_empty() { parts.push(g.description.clone()); }
        let param_docs: Vec<String> = spec.params.iter().filter_map(|p| {
            g.inputs.get(p.port.as_str()).filter(|pd| !pd.tooltip.is_empty()).map(|pd| format!("- **{}** - {}", pd.display_name, pd.tooltip))
        }).collect();
        if !param_docs.is_empty() { parts.push(format!("**Parameters:**\n{}", param_docs.join("\n"))); }
    }
    Some(parts.join("\n\n"))
}

/// `chip` or `mod` keyword: show exec/pure context and resource estimate.
fn hover_chip_or_mod_keyword(
    source: &str,
    word: &str,
    symbols: &[SymbolDef],
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
) -> Option<String> {
    if word != "chip" && word != "mod" { return None; }

    let lo = line_offset_at(source, line);
    let line_end = lo + source.lines().nth(line).map_or(0, |l| l.len() + 1);

    // Find the nearest symbol at this line that's a chip/mod
    for sym in symbols {
        if (sym.kind == "chip" || sym.kind == "mod")
            && sym.range.start.offset >= lo
            && sym.range.start.offset < line_end
        {
            let context = if sym.exec { "exec" } else { "pure" };
            let name = if sym.name.is_empty() || sym.name.starts_with('_') { "(anonymous)" } else { &sym.name };
            let mut hover = format!(
                "```wirescript\n{} {} ({})\n```\n\n{} context - {}",
                sym.kind, name, context,
                if sym.exec { "Exec" } else { "Pure" },
                if sym.exec { "body runs as sequential exec chain" } else { "body is evaluated as signal-flow (combinational)" },
            );
            if let Some(est) = lookup_estimate(resource_estimates, &sym.name, sym.range.start.offset) {
                let ek = if sym.kind == "mod" { EstimateKind::Mod } else { EstimateKind::Chip };
                hover += &format!("\n\n{}", format_estimate(est, ek));
            }
            return Some(hover);
        }
    }
    None
}

/// `on` keyword: show handler resource estimate.
fn hover_on_keyword(
    source: &str,
    word: &str,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
) -> Option<String> {
    if word != "on" { return None; }

    let l = source.lines().nth(line)?;
    let offset = line_offset_at(source, line) + l.find("on").unwrap_or(0);
    let est = resource_estimates.get(&format!("@{offset}"))?;

    let mut hover = "```wirescript\non handler (exec)\n```".to_string();
    hover += &format!("\n\n{}", format_estimate(est, EstimateKind::Scope));
    Some(hover)
}

/// Record literal field or type declaration field.
/// Checked before general symbol lookup so `counter` in `{ counter: score }`
/// shows as a field, not as a param.
fn hover_record_or_type_field(
    source: &str,
    symbols: &[SymbolDef],
    doc_comments: &HashMap<usize, String>,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // Record literal field (e.g. `{ counter: score }`)
    if let Some(v) = resolve_record_lit_field(source, symbols, word, line) {
        return Some(v);
    }

    // Type declaration field: check if cursor is inside a type definition's range
    for sym in symbols {
        if sym.kind == "type"
            && sym.range.start.line.saturating_sub(1) as usize <= line
            && sym.range.end.line.saturating_sub(1) as usize >= line
        {
            if let Some(ref ty_str) = sym.ty {
                if let Some(field_type) = extract_record_field_type(ty_str, word) {
                    let mut hover = format!("```wirescript\n{}.{}: {}\n```", sym.name, word, field_type);
                    // Field `///` doc comment, stored by the parser keyed by the
                    // field name's offset.
                    let field_off = line_offset_at(source, line)
                        + word_start_in_line(source.lines().nth(line)?, col);
                    if let Some(doc) = doc_comments.get(&field_off) {
                        hover += &format!("\n\n{doc}");
                    }
                    return Some(hover);
                }
            }
        }
    }
    None
}

/// User-defined symbol: var, let, buffer, in, out, mod, chip, fn, type, etc.
fn hover_user_symbol(
    source: &str,
    file: &str,
    symbols: &[SymbolDef],
    doc_comments: &HashMap<usize, String>,
    var_read_contexts: &VarReadContextMap,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    let sym = symbols.iter().find(|s| s.name == word)?;

    // Namespace alias (`import * as card`): it has no type — show it as a
    // namespace and list the members it brings in (its qualified `card.*`
    // symbols), rather than falling through to `namespace card: unknown`.
    if sym.kind == "namespace" {
        let prefix = format!("{}.", sym.name);
        let members: Vec<&str> = symbols
            .iter()
            .filter_map(|s| s.name.strip_prefix(&prefix))
            .filter(|m| !m.contains('.'))
            .collect();
        let mut v = format!("```wirescript\nnamespace {}\n```", sym.name);
        if !members.is_empty() {
            v += &format!(
                "\n\n{} member{}: {}",
                members.len(),
                if members.len() == 1 { "" } else { "s" },
                members.join(", ")
            );
        }
        return Some(v);
    }

    let mut v = render_decl_hover(sym, doc_comments, resource_estimates);

    // For var reads: show exec/pure context at the hovered location
    if sym.kind == "var" {
        let l = source.lines().nth(line)?;
        let offset = line_offset_at(source, line) + word_start_in_line(l, col);
        let f: std::sync::Arc<str> = file.into();
        if let Some(&is_exec) = var_read_contexts.get(&(f, offset)) {
            if is_exec {
                v += "\n\n*(exec) reads current value via Var\\_Get*";
            } else {
                v += "\n\n*(pure) reads previous tick's value via Value field*";
            }
        }
    }

    Some(v)
}

/// Render a declaration symbol's hover card: its signature line (mods/chips/fns
/// show `(exec, params) -> ret`; everything else `kind name: type`), followed by
/// its doc comment and, for callables, a resource estimate. Shared by plain
/// symbol hover and namespace-member hover.
fn render_decl_hover(
    sym: &SymbolDef,
    doc_comments: &HashMap<usize, String>,
    resource_estimates: &HashMap<String, ResourceEstimate>,
) -> String {
    let ty_str = sym.ty.as_deref().unwrap_or("unknown");
    let mut v = match sym.kind {
        "mod" | "chip" | "fn" => {
            let sig = if sym.exec {
                if ty_str.starts_with('(') && ty_str.len() > 2 { format!("(exec, {}", &ty_str[1..]) } else { "(exec)".into() }
            } else { ty_str.to_string() };
            format!("```wirescript\n{} {}{}\n```", sym.kind, sym.name, sig)
        }
        _ => format!("```wirescript\n{} {}: {}\n```", sym.kind, sym.name, ty_str),
    };
    if let Some(doc) = doc_comments.get(&sym.range.start.offset) {
        v += &format!("\n\n{}", doc);
    }
    if matches!(sym.kind, "mod" | "chip" | "fn") {
        if let Some(est) = lookup_estimate(resource_estimates, &sym.name, sym.range.start.offset) {
            let ek = if sym.kind == "mod" { EstimateKind::Mod } else { EstimateKind::Chip };
            v += &format!("\n\n{}", format_estimate(est, ek));
        }
    }
    v
}

/// Hover for the member in a namespace-qualified reference — the `drawTopText`
/// in `card.drawTopText` where `card` is an `import * as card`. The member is
/// stored in `symbols` under its qualified `card.drawTopText` name, so the plain
/// bare-word lookup in [`hover_user_symbol`] misses it; form the qualified name
/// here and render its signature (go-to-definition already resolved this path).
fn hover_namespace_member(
    source: &str,
    symbols: &[SymbolDef],
    doc_comments: &HashMap<usize, String>,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // The cursor must be on the `member` half of an `obj.member` access.
    let l = source.lines().nth(line)?;
    let c = col.min(l.len());
    let start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let obj_end = start - 1;
    let obj_start = l[..obj_end]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let obj_name = &l[obj_start..obj_end];
    // `obj` must be a namespace alias for this to be a namespace-member access.
    if !symbols.iter().any(|s| s.name == obj_name && s.kind == "namespace") {
        return None;
    }
    let qualified = format!("{obj_name}.{word}");
    let sym = symbols.iter().find(|s| s.name == qualified)?;
    Some(render_decl_hover(sym, doc_comments, resource_estimates))
}

fn resolve_record_lit_field(source: &str, symbols: &[SymbolDef], field: &str, line: usize) -> Option<String> {
    // Walk backwards from the current line to find a `let name: TypeName = {` pattern
    for scan_line in (0..=line).rev() {
        let l = source.lines().nth(scan_line)?;
        let trimmed = l.trim();

        if let Some(rest) = trimmed.strip_prefix("let ")
            && let Some(colon_pos) = rest.find(':')
        {
            let after_colon = rest[colon_pos + 1..].trim();
            let type_name = after_colon.split(|c: char| c == '=' || c.is_whitespace()).next()?;
            let type_name = type_name.trim();
            if type_name.is_empty() { continue; }

            // Find this type in symbols and parse its field list
            for sym in symbols {
                if sym.kind == "type" && sym.name == type_name
                    && let Some(ref ty_str) = sym.ty
                {
                    // Parse "{name: type, name: type}" into field pairs
                    if let Some(field_type) = extract_record_field_type(ty_str, field) {
                        return Some(format!("```wirescript\n{}.{}: {}\n```", type_name, field, field_type));
                    }
                }
            }
        }

        // Stop scanning if this line can't be part of a record literal.
        // Lines that ARE part of a record literal are: empty, comments, spreads,
        // key-value pairs (contain `:`), trailing commas, or brace delimiters.
        let is_record_interior = trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("...")
            || trimmed.contains(':')
            || trimmed.contains(',')
            || trimmed.ends_with('{')
            || trimmed.ends_with('}');
        if !is_record_interior {
            break;
        }
    }
    None
}

/// Extract a field's type from a record type string like `{counter: *int, step: int}`.
///
/// This operates on stringified type representations rather than the `Type` enum because
/// cross-file imported symbols only carry their serialized type string (`SymbolDef.ty`),
/// not a resolved `Type`. When hovering a field on an imported record, the actual `Type`
/// may not be available in the current file's type_map, so we fall back to parsing the
/// string form that the symbol exporter produced.
fn extract_record_field_type(ty_str: &str, field: &str) -> Option<String> {
    let inner = ty_str.strip_prefix('{')?.strip_suffix('}')?;
    for part in split_record_fields(inner) {
        let part = part.trim();
        if let Some(colon) = part.find(':') {
            let name = part[..colon].trim();
            let typ = part[colon + 1..].trim();
            if name == field {
                return Some(typ.to_string());
            }
        }
    }
    None
}

/// Split record fields respecting nested braces/brackets (e.g. `{a: {x: int}, b: int}`).
fn split_record_fields(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        parts.push(&s[start..]);
    }
    parts
}

pub(super) fn resolve_record_param_field_type(script: &crate::ast::Script, param_type: &crate::ast::TypeExpr, field: &str) -> Option<String> {
    let record_fields = match param_type {
        crate::ast::TypeExpr::Record { fields, .. } => fields,
        crate::ast::TypeExpr::Name { name, .. } => {
            for d in &script.decls {
                if let crate::ast::TopDecl::TypeAlias(ta) = d
                    && ta.name == *name
                        && let crate::ast::TypeExpr::Record { fields, .. } = &ta.typ {
                            return fields.iter()
                                .find(|f| f.name == field)
                                .map(|f| super::types::type_expr_str(&f.typ));
                        }
            }
            return None;
        }
        _ => return None,
    };
    record_fields.iter()
        .find(|f| f.name == field)
        .map(|f| super::types::type_expr_str(&f.typ))
}

fn resolve_field_hover(source: &str, file: &str, type_map: &TypeMap, symbols: &[SymbolDef], line: usize, col: usize, field: &str) -> Option<String> {
    let l = source.lines().nth(line)?;
    let c = col.min(l.len());
    let start = l[..c].rfind(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| i + 1).unwrap_or(0);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let obj_end = start - 1;
    let obj_start = l[..obj_end].rfind(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| i + 1).unwrap_or(0);
    let obj_name = &l[obj_start..obj_end];
    let lo = line_offset_at(source, line);
    let field_end_col = l[c..].find(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| c + i).unwrap_or(l.len());

    let f: std::sync::Arc<str> = file.into();
    let fmt_field = |ty_display: String| format!("```wirescript\nfield {}: {}\n```", field, ty_display);

    // Layer 1: Full expression span (obj.field) in type_map - best case, typechecker
    // recorded the type of the entire dotted expression.
    if let Some(ty) = type_map.get(&(f.clone(), lo + obj_start, lo + field_end_col)) {
        return Some(fmt_field(type_str(ty)));
    }

    // Layer 2: Object type from type_map - look up the object's type and resolve
    // the field structurally (records, vectors, colors, rotators, refs).
    if let Some(ft) = find_obj_type(type_map, &f, lo + obj_start, lo + obj_end)
        .and_then(|obj_ty| resolve_field_in_type(&obj_ty, field))
    {
        return Some(fmt_field(type_str(&ft)));
    }

    // Layer 2.5: Non-identifier object - a call/index result like
    // `arr.find(x).Found`, where the backwards text scan above lands on `)`
    // and can't name the object. The typechecker still recorded the object
    // expression's span: the innermost type_map entry ending exactly at the
    // `.` is the object, and its record type carries the field.
    if let Some(ft) = type_map
        .iter()
        .filter(|((f2, _, e), _)| **f2 == *f && *e == lo + obj_end)
        .max_by_key(|((_, s, _), _)| *s)
        .and_then(|(_, obj_ty)| resolve_field_in_type(obj_ty, field))
    {
        return Some(fmt_field(type_str(&ft)));
    }

    // Layer 3: Symbol-based fallback - look up the object name in symbols, find
    // its type declaration, and resolve the field from the type's string form.
    // This handles imported files where type_map offsets don't match the current source.
    if !obj_name.is_empty() {
        return resolve_field_via_symbols(symbols, obj_name, field).map(fmt_field);
    }

    None
}

/// Look up `obj_name` in symbols, find its type declaration, and resolve `field`
/// from the type's string representation.
fn resolve_field_via_symbols(symbols: &[SymbolDef], obj_name: &str, field: &str) -> Option<String> {
    let sym = symbols.iter().find(|s| s.name == obj_name)?;
    let ty_name = sym.ty.as_deref()?;

    // Try named type: find the type declaration and extract the field
    symbols.iter()
        .find(|ts| ts.kind == "type" && ts.name == ty_name)
        .and_then(|ts| ts.ty.as_deref())
        .and_then(|ty_str| extract_record_field_type(ty_str, field))
        // If the symbol's type is an inline record literal (starts with `{`),
        // parse it directly
        .or_else(|| {
            if ty_name.starts_with('{') {
                extract_record_field_type(ty_name, field)
            } else {
                None
            }
        })
}

/// Find the type of an object expression at the given span in the type_map.
///
/// The typechecker records expression spans that may not exactly match the byte
/// offsets computed from source text (off-by-one in end position is common due to
/// how the parser vs. hover module count trailing characters). We handle this with
/// a 3-tier lookup:
///
/// 1. **Exact span** - `(file, obj_start, obj_end)` matches directly.
/// 2. **Fuzzy end** - same start, but end offset is +/-1 from what we computed.
///    This catches the most common parser/hover offset mismatch.
/// 3. **Start-only scan** - any entry with a matching `(file, obj_start, _)`.
///    Last resort when the end offset is completely different.
fn find_obj_type(type_map: &TypeMap, file: &std::sync::Arc<str>, obj_start: usize, obj_end: usize) -> Option<Type> {
    // Tier 1: exact span
    if let Some(ty) = type_map.get(&(file.clone(), obj_start, obj_end)) {
        return Some(ty.clone());
    }

    // Tier 2: fuzzy end offset (+/-1)
    for end in [obj_end.wrapping_sub(1), obj_end + 1] {
        if let Some(ty) = type_map.get(&(file.clone(), obj_start, end)) {
            return Some(ty.clone());
        }
    }

    // Tier 3: scan for any entry starting at obj_start in this file
    for ((f, s, _e), ty) in type_map.iter() {
        if **f == **file && *s == obj_start {
            return Some(ty.clone());
        }
    }

    None
}

fn resolve_field_in_type(ty: &Type, field: &str) -> Option<Type> {
    match ty {
        Type::Record(fields) => {
            fields.iter().find(|(k, _)| k == field).map(|(_, t)| t.clone())
        }
        Type::Ref(inner) => {
            if field == "Value" || field == "prev" || field == "VarRef" {
                return Some(inner.as_ref().clone());
            }
            resolve_field_in_type(inner, field)
        }
        Type::Vector => match field {
            "x" | "X" | "y" | "Y" | "z" | "Z" => Some(Type::Float),
            _ => None,
        },
        Type::Color => match field {
            "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A" => Some(Type::Float),
            _ => None,
        },
        Type::Rotator => match field {
            "pitch" | "yaw" | "roll" => Some(Type::Float),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::collect_symbols_for_file;
    use crate::resolve::{resolve, FsLoader};
    use crate::typecheck::typecheck;
    fn hover_for(source: &str, line: usize, col: usize) -> Option<String> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test");
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
        let estimates = crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, "test");
        hover_at(
            source,
            "test",
            &symbols,
            &tc.type_of_expr,
            &resolved.doc_comments,
            &tc.if_contexts,
            &tc.var_read_contexts,
            &estimates,
            line,
            col,
        )
    }

    #[test]
    fn namespace_member_hovers_with_signature() {
        // Hovering the member in `card.drawCard` (a namespace-qualified call)
        // must show its signature, not nothing. The member is stored under the
        // qualified `card.drawCard` symbol name, which the bare-word lookup in
        // hover_user_symbol misses — go-to-definition worked but hover didn't.
        use crate::resolve::MemLoader;
        let loader = MemLoader {
            files: [(
                "display.ws".to_string(),
                "mod drawCard(n: int, label: string) {}".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        let src = "import * as card from \"display\"\non RoundStart { card.drawCard(1, \"hi\") }";
        let resolved = resolve(src, "main", &loader);
        let tc = typecheck(&resolved.ast, "main");
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main"));
        let estimates =
            crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, "main");
        let line1 = src.lines().nth(1).unwrap();
        let col = line1.find("drawCard").unwrap();
        let text = hover_at(
            src,
            "main",
            &symbols,
            &tc.type_of_expr,
            &resolved.doc_comments,
            &tc.if_contexts,
            &tc.var_read_contexts,
            &estimates,
            1,
            col,
        )
        .expect("hover on a namespace member should return something");
        assert!(text.contains("drawCard"), "should name the member, got: {text}");
        assert!(text.contains("n: int"), "should show the signature, got: {text}");
        assert!(!text.contains("unknown"), "must not show `unknown`, got: {text}");
    }

    #[test]
    fn record_type_field_doc_comment_shows_on_hover() {
        let src = "type Point = {\n  /// the x coordinate\n  x: int,\n  y: int,\n}";
        // `x` is on line 2 (0-based); hover it.
        let col_x = src.lines().nth(2).unwrap().find('x').unwrap();
        let hx = hover_for(src, 2, col_x).expect("hover on documented field x");
        assert!(hx.contains("Point.x: int"), "field type missing: {hx}");
        assert!(hx.contains("the x coordinate"), "field doc missing: {hx}");
        // The undocumented field `y` shows no doc.
        let col_y = src.lines().nth(3).unwrap().find('y').unwrap();
        let hy = hover_for(src, 3, col_y).expect("hover on field y");
        assert!(hy.contains("Point.y: int"), "y type missing: {hy}");
        assert!(!hy.contains("coordinate"), "y should have no doc: {hy}");
    }

    #[test]
    fn namespace_alias_hovers_with_members_not_unknown() {
        use crate::resolve::MemLoader;
        let loader = MemLoader {
            files: [(
                "display.ws".to_string(),
                "mod drawCard(n: int) {}\nlet WIDTH = 10".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        let src = "import * as card from \"display\"";
        let resolved = resolve(src, "main", &loader);
        let tc = typecheck(&resolved.ast, "main");
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main"));
        let estimates =
            crate::analysis::resource_estimate::collect_estimates(&resolved.ast, &tc, "main");
        let col = src.find("card").unwrap();
        let text = hover_at(
            src,
            "main",
            &symbols,
            &tc.type_of_expr,
            &resolved.doc_comments,
            &tc.if_contexts,
            &tc.var_read_contexts,
            &estimates,
            0,
            col,
        )
        .expect("hover on a namespace alias should return something");
        assert!(text.contains("namespace card"), "should show namespace, got: {text}");
        assert!(!text.contains("unknown"), "must not show `unknown`, got: {text}");
        assert!(text.contains("drawCard"), "should list members, got: {text}");
    }

    #[test]
    fn destructured_mod_param_shows_type() {
        let src = "\
type State = { counter: *int, step: int }
mod bump({ counter, step }: State) { counter = counter + step }";
        // "counter" starts at col 11, "step" at col 20 on line 1
        let h = hover_for(src, 1, 11);
        assert!(h.is_some(), "hover on destructured param 'counter' should return something");
        let text = h.unwrap();
        assert!(
            text.contains("*int"),
            "hover should show *int for counter, got: {text}"
        );

        let h2 = hover_for(src, 1, 20);
        assert!(h2.is_some(), "hover on destructured param 'step' should return something");
        let text2 = h2.unwrap();
        assert!(
            text2.contains("int") && !text2.contains("*int"),
            "hover should show int for step, got: {text2}"
        );
    }

    #[test]
    fn named_arg_value_sharing_param_name_hovers_as_symbol() {
        // `Sleep(_, delay = delay)`: only the LHS is the named arg; the RHS is
        // a user symbol that merely shares the param's name and must hover as
        // the symbol, not as the param docs.
        let src = "let delay = 1.0\non RoundStart { await Sleep(_, delay = delay) }";
        let line1 = src.lines().nth(1).unwrap();
        let lhs = line1.find("delay").unwrap();
        let rhs = line1.rfind("delay").unwrap();

        let hl = hover_for(src, 1, lhs).expect("hover on the arg name should return something");
        assert!(
            hl.starts_with("**"),
            "arg-name hover should be the named-param docs, got: {hl}"
        );
        let hr = hover_for(src, 1, rhs).expect("hover on the value should return something");
        assert!(
            hr.contains("let delay"),
            "value hover must be the user symbol, not the named-param docs, got: {hr}"
        );

        // Same on a continuation line of a multi-line call.
        let src2 = "let delay = 1.0\non RoundStart {\n  await Sleep(_,\n    delay = delay,\n  )\n}";
        let line3 = src2.lines().nth(3).unwrap();
        let hl2 = hover_for(src2, 3, line3.find("delay").unwrap())
            .expect("hover on the multi-line arg name should return something");
        assert!(
            hl2.starts_with("**"),
            "multi-line arg-name hover should be the named-param docs, got: {hl2}"
        );
        let hr2 = hover_for(src2, 3, line3.rfind("delay").unwrap())
            .expect("hover on the multi-line value should return something");
        assert!(
            hr2.contains("let delay"),
            "multi-line value hover must be the user symbol, got: {hr2}"
        );
    }

    #[test]
    fn user_var_named_like_array_method_hovers_as_var() {
        // A variable named after an array method (`sum`) must hover as the
        // variable, not as `array.sum`. The array-method hover only applies to a
        // `.method` access, not a bare identifier that happens to share the name.
        let src = "var sum: int = 0";
        // "sum" occupies cols 4..=6
        let h = hover_for(src, 0, 5).expect("hover on var 'sum' should return something");
        assert!(
            !h.contains("array.sum"),
            "hover on the variable `sum` must not show the array method, got: {h}"
        );
        assert!(
            h.contains("var sum"),
            "hover on the variable `sum` should show the variable declaration, got: {h}"
        );
    }

    #[test]
    fn user_var_named_like_builtin_method_hovers_as_var() {
        // `Teleport` is a builtin receiver-method; a variable sharing that name
        // must hover as the variable, not the method. Method/call hovers only
        // fire in actual call/method position.
        let src = "var Teleport: int = 0";
        // "Teleport" occupies cols 4..=11
        let h = hover_for(src, 0, 6).expect("hover on var 'Teleport' should return something");
        assert!(
            h.contains("var Teleport"),
            "hover on the variable `Teleport` should show the variable, got: {h}"
        );
    }

    #[test]
    fn builtin_call_in_call_position_still_hovers() {
        // A builtin used as an actual call still hovers as the call.
        let src = "in t: exec\non t { PrintToConsole(\"hi\") }";
        // "PrintToConsole" starts at col 7 on line 1
        let h = hover_for(src, 1, 10).expect("hover on PrintToConsole call should return");
        assert!(
            h.contains("PrintToConsole"),
            "call-position builtin should still hover, got: {h}"
        );
    }

    #[test]
    fn array_method_access_still_hovers_as_method() {
        // The `.sum` access must still show the array method hover.
        let src = "\
array fa: int[] = [5, 10, 15]
on load { let s = fa.sum() }";
        // "sum" in "fa.sum()" starts at col 20 on line 1
        let h = hover_for(src, 1, 21).expect("hover on `.sum` should return something");
        assert!(
            h.contains("array.sum"),
            "hover on `fa.sum()` should show the array method, got: {h}"
        );
    }

    #[test]
    fn record_field_hover() {
        let src = "\
type Point = { x: int, y: int }
let p: Point = { x: 1, y: 2 }
let v = p.x";
        // hover on "x" in "p.x" (line 2, col 10)
        let h = hover_for(src, 2, 10);
        assert!(h.is_some(), "hover on record field 'x' should return something");
        let text = h.unwrap();
        assert!(
            text.contains("int"),
            "hover on p.x should show int, got: {text}"
        );
    }

    #[test]
    fn prefab_file_reference_hover() {
        // `$` at col 8; token spans cols 8..=25.
        let src = "let p = $./prefab_1x1.brz";
        for col in [8usize, 12, 24] {
            let h = hover_for(src, 0, col);
            assert!(h.is_some(), "hover on prefab ref at col {col} should return");
            let text = h.unwrap();
            assert!(
                text.contains("Prefab file reference") && text.contains("Resolves to"),
                "col {col} got: {text}"
            );
        }
    }

    #[test]
    fn asset_reference_hover() {
        // `$Weapon/Sword`: `$` at col 8, "Weapon" 9-15, "Sword" 16-21.
        let src = "let w = $Weapon/Sword";
        let h = hover_for(src, 0, 11).expect("hover on asset ref should return");
        assert!(
            h.contains("Asset reference") && h.contains("Weapon") && h.contains("Sword"),
            "got: {h}"
        );
    }
    #[test]
    fn array_read_out_of_bounds_field_is_bool() {
        // `arr[i]` is typed as the bare element, so once it is bound to a `let`
        // the bounds flag has no record to resolve against and used to fall
        // through to Any - which is universal, so nothing downstream complained.
        let src = "array names: string[]
in go: exec
on go {
  let n = names[0]
  let b = n.OutOfBounds
}";
        let line4 = src.lines().nth(4).unwrap();
        let col = line4.find("OutOfBounds").unwrap();
        let h = hover_for(src, 4, col).expect("hover on .OutOfBounds should return something");
        assert!(
            h.contains("bool"),
            "array-read bounds flag should type as bool, got: {h}"
        );
    }

    #[test]
    fn call_result_field_access_hover_shows_field_type() {
        // `arr.find(x).Found` - the object is a call result, not an
        // identifier, so the field type must resolve from the call
        // expression's record type in the type map.
        let src = "array ids: string[]
chip a(uid: string) -> int {
  return if ids.find(uid).Found then 1 else 0
}";
        let line2 = src.lines().nth(2).unwrap();
        let col = line2.find("Found").unwrap();
        let h = hover_for(src, 2, col).expect("hover on .Found should return something");
        assert!(
            h.contains("field Found: bool"),
            "call-result field hover should type from the record, got: {h}"
        );
    }

    #[test]
    fn record_destructured_let_hover_shows_field_types() {
        // `let { Found, Index } = ids.find(uid)` - each destructured name
        // takes its field's type from the initializer's record type.
        let src = "array ids: string[]
chip a(uid: string) -> int {
  let { Found, Index } = ids.find(uid)
  return if Found then Index else -1
}";
        let line2 = src.lines().nth(2).unwrap();
        let h = hover_for(src, 2, line2.find("Found").unwrap()).expect("hover on Found");
        assert!(h.contains("bool"), "destructured Found should be bool, got: {h}");
        let h2 = hover_for(src, 2, line2.find("Index").unwrap()).expect("hover on Index");
        assert!(h2.contains("int"), "destructured Index should be int, got: {h2}");
        // Usages resolve through the same symbol.
        let line3 = src.lines().nth(3).unwrap();
        let h3 = hover_for(src, 3, line3.find("Found").unwrap()).expect("hover on Found use");
        assert!(h3.contains("bool"), "Found usage should be bool, got: {h3}");
    }

}
