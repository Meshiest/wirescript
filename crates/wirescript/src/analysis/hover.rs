use crate::collections::HashMap;
use crate::ast::{CallArg, ChipDecl, Expr, Handler, HandlerConfigArg, LetBinding, Pattern, Script, Stmt, TopDecl, Trigger, TypeExpr};
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;
use crate::diagnostic::SourceRange;
use crate::ir::{Literal, Type};
use crate::lower::ConstEnv;
use super::{TypeMap, IfContextMap, VarReadContextMap};
use super::types::{type_str, collection_kind, CollectionKind};
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
    // A `const mod` called from a const-required position is answered entirely
    // by the compile-time interpreter and contributes NO gates. The signature
    // line above already shows `const mod`, so this line only has to carry the
    // count — the caveat (a call from a NON-const position falls back to an
    // ordinary inlined mod and does emit gates) lives in the docs rather than
    // in every hover, which a reader sees hundreds of times.
    if est.is_const_eval {
        return "*const — 0 gates*".to_string();
    }
    let chips = match kind {
        EstimateKind::Chip => est.total_microchips + 1,
        _ => est.total_microchips,
    };
    let mut parts = vec![format!("~{} gates", est.gates)];
    // A `mod`/scope with no inner microchips (the common case — an inlined mod
    // whose body is plain gates, like `assert`) shows no chip clause at all; a
    // bare "0 chips" reads like a missing count. A `chip` always instantiates
    // at least itself, so its count is never zero and always shown.
    if chips > 0 {
        parts.push(format!("{} chip{}", chips, if chips == 1 { "" } else { "s" }));
    }
    if matches!(kind, EstimateKind::Mod) {
        parts.push("inlined per call".into());
    }
    format!("*{}*", parts.join(", "))
}

/// Render a [`Literal`] roughly the way its wirescript literal syntax would
/// read, for hover's `const NAME: TYPE = VALUE` display. Long
/// containers/records are capped so hover can't dump an enormous table — a
/// truncated tail says "there's more" rather than pretending to be exact.
fn literal_display(lit: &Literal) -> String {
    const MAX_CHARS: usize = 200;
    let s = literal_display_inner(lit);
    if s.chars().count() > MAX_CHARS {
        format!("{}...", s.chars().take(MAX_CHARS).collect::<String>())
    } else {
        s
    }
}

fn literal_display_inner(lit: &Literal) -> String {
    match lit {
        Literal::Bool(b) => b.to_string(),
        Literal::Int(n) => n.to_string(),
        // Same 3-decimal / trailing-zero-trimmed rendering `@label(expr)` and
        // FormatText use elsewhere — a const float hovers the way it would
        // actually display in-game, not with raw `f64` precision.
        Literal::Float(f) => {
            crate::lower::fold::eval::render_for_format(&crate::lower::fold::eval::Value::Float(*f))
        }
        Literal::String(s) => format!("{s:?}"),
        Literal::Vector { x, y, z } => format!("Vec({x}, {y}, {z})"),
        Literal::Rotator { pitch, yaw, roll } => format!("Rotation({pitch}, {yaw}, {roll})"),
        Literal::Quat { x, y, z, w } => format!("Quat({x}, {y}, {z}, {w})"),
        Literal::Color { r, g, b, a } => format!("Color({r}, {g}, {b}, {a})"),
        Literal::LinearColor { r, g, b, a } => format!("Color({r}, {g}, {b}, {a})"),
        Literal::Object => "null".to_string(),
        Literal::Array(items) => {
            let parts: Vec<String> = items.iter().map(literal_display_inner).collect();
            format!("[{}]", parts.join(", "))
        }
        Literal::Map(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{} => {}", literal_display_inner(k), literal_display_inner(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Literal::Record(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", literal_display_inner(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Literal::Asset { asset_type, asset_name } => format!("${asset_type}/{asset_name}"),
        Literal::PrefabRef { path } => format!("${path}"),
        Literal::NestedPrefab { .. } => "$```...```".to_string(),
    }
}

/// Best-effort compile-time VALUE of a `const` binding, for hover's
/// `const NAME: TYPE = VALUE` display.
///
/// Hover works from a flat [`SymbolDef`], which doesn't carry enough AST
/// structure to walk sibling declarations, so this RE-PARSES `source` (the
/// same trick [`hover_custom_event`] already uses) and locates the `LetDecl`
/// whose own range starts at `target_off` — exactly the range
/// `collect_let_symbols` stored on the `SymbolDef`, so a fresh parse of the
/// same source always lands on the same node.
///
/// A TOP-LEVEL const's value comes from [`crate::lower::build_const_env`] —
/// the very fixpoint lowering itself runs, so hover can never disagree with
/// what actually gets baked. A const declared inside a `mod`/`chip` BODY is
/// evaluated by walking that body's OWN statements up to the target in
/// source order, seeding the running environment from each earlier sibling
/// `let`/`const` that itself evaluates (best-effort: one this walk can't
/// resolve is simply left out, so a later sibling that doesn't depend on it
/// can still resolve). Entering a nested NAMED `mod`/`chip` resets the
/// environment to the module env alone (mirrors `const_eval::interp::eval_call`'s
/// own scoping); an anonymous `chip { }` keeps the running environment (it
/// shares its parent's scope, per the language).
///
/// Crucially, a `mod`/`chip`'s own PARAMETERS are never bound to anything
/// here — unlike typecheck's per-call placeholder zero
/// (`typecheck/decl.rs`), which exists only so a body can type-check with no
/// call site in hand. A body const that reads a parameter therefore simply
/// fails to evaluate (an unresolved name), and this returns `None` — never
/// the placeholder, which would be a wrong value dressed up as a real one.
fn const_hover_value(source: &str, file: &str, target_off: usize) -> Option<Literal> {
    let parsed = crate::parser::parse(source, file);
    let script = &parsed.ast;
    let enum_defs = std::sync::Arc::new(crate::typecheck::enums::build_registry(&script.decls));
    let module_env = crate::lower::build_const_env(&script.decls, &enum_defs);
    let mods = const_mod_table(&script.decls);
    let lookup_mod = |name: &str| mods.get(name).cloned();
    eval_const_at(&script.decls, target_off, &module_env, &module_env, &enum_defs, &lookup_mod)
}

/// Every top-level `const mod`, by name — mirrors the table
/// `crate::lower::build_const_env` builds internally, so a const initializer
/// that CALLS a const mod resolves here the same way it resolves for real.
fn const_mod_table(decls: &[TopDecl]) -> HashMap<String, std::sync::Arc<ChipDecl>> {
    decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Chip(c) if c.is_const => Some((c.name.clone(), std::sync::Arc::new(c.clone()))),
            _ => None,
        })
        .collect()
}

type LookupMod<'a> = dyn Fn(&str) -> Option<std::sync::Arc<ChipDecl>> + 'a;

/// Evaluate one expression against `env`/`module_env`, discarding any error —
/// hover has no diagnostic to attach a reason to, so "couldn't evaluate" and
/// "evaluated to something surprising" are both just `None`.
fn eval_one(
    expr: &Expr,
    env: &ConstEnv,
    module_env: &ConstEnv,
    enum_defs: &HashMap<String, crate::typecheck::enums::EnumDef>,
    lookup_mod: &LookupMod,
) -> Option<Literal> {
    // `enum_defs` here is a borrowed `&HashMap` (this function's own
    // parameter), not an `Arc`, so this hover-only path still pays a deep
    // clone rather than a refcount bump; hover queries are not the hot loop
    // this refactor targets.
    let cx = crate::const_eval::ConstCtx {
        consts: env.clone(),
        module_consts: module_env.clone(),
        enum_defs: std::sync::Arc::new(enum_defs.clone()),
        lookup_mod: Some(lookup_mod),
    };
    let mut budget = crate::const_eval::Budget::default();
    crate::const_eval::eval_expr(expr, &cx, &mut budget).ok()
}

fn range_contains_offset(range: &SourceRange, off: usize) -> bool {
    range.start.offset <= off && off < range.end.offset
}

/// Search `decls` (top-level, or a namespace's) for the `LetDecl` whose range
/// starts at `target_off`, evaluating const siblings along the way — see
/// [`const_hover_value`]'s doc comment for the full scoping rules.
fn eval_const_at(
    decls: &[TopDecl],
    target_off: usize,
    env0: &ConstEnv,
    module_env: &ConstEnv,
    enum_defs: &HashMap<String, crate::typecheck::enums::EnumDef>,
    lookup_mod: &LookupMod,
) -> Option<Literal> {
    let mut env = env0.clone();
    for d in decls {
        match d {
            TopDecl::Let(l) => {
                if l.range.start.offset == target_off {
                    return l
                        .is_const
                        .then(|| eval_one(&l.value, &env, module_env, enum_defs, lookup_mod))
                        .flatten();
                }
                if let LetBinding::Ident { name, .. } = &l.binding {
                    if let Some(v) = eval_one(&l.value, &env, module_env, enum_defs, lookup_mod) {
                        env.insert(name.clone(), v);
                    }
                }
            }
            // A named mod/chip body is its OWN scope: reset to the module
            // env alone (no outer locals, and — deliberately — no params).
            TopDecl::Chip(c) if range_contains_offset(&c.range, target_off) => {
                return eval_const_in_block(&c.body, target_off, module_env, module_env, enum_defs, lookup_mod);
            }
            // An anonymous chip shares its parent's scope.
            TopDecl::AnonChip(ac) if range_contains_offset(&ac.range, target_off) => {
                return eval_const_in_block(&ac.body, target_off, &env, module_env, enum_defs, lookup_mod);
            }
            TopDecl::Namespace(ns) if range_contains_offset(&ns.range, target_off) => {
                return eval_const_at(&ns.decls, target_off, env0, module_env, enum_defs, lookup_mod);
            }
            _ => {}
        }
    }
    None
}

/// Same walk as [`eval_const_at`], over a statement block (a mod/chip/anon-chip
/// body, or an `if` arm).
fn eval_const_in_block(
    block: &crate::ast::Block,
    target_off: usize,
    env0: &ConstEnv,
    module_env: &ConstEnv,
    enum_defs: &HashMap<String, crate::typecheck::enums::EnumDef>,
    lookup_mod: &LookupMod,
) -> Option<Literal> {
    let mut env = env0.clone();
    for s in &block.stmts {
        match s {
            Stmt::Let(l) => {
                if l.range.start.offset == target_off {
                    return l
                        .is_const
                        .then(|| eval_one(&l.value, &env, module_env, enum_defs, lookup_mod))
                        .flatten();
                }
                if let LetBinding::Ident { name, .. } = &l.binding {
                    if let Some(v) = eval_one(&l.value, &env, module_env, enum_defs, lookup_mod) {
                        env.insert(name.clone(), v);
                    }
                }
            }
            Stmt::ChipDecl(c) if range_contains_offset(&c.range, target_off) => {
                return eval_const_in_block(&c.body, target_off, module_env, module_env, enum_defs, lookup_mod);
            }
            Stmt::AnonChip(ac) if range_contains_offset(&ac.range, target_off) => {
                return eval_const_in_block(&ac.body, target_off, &env, module_env, enum_defs, lookup_mod);
            }
            Stmt::If(i) if range_contains_offset(&i.then_block.range, target_off) => {
                return eval_const_in_block(&i.then_block, target_off, &env, module_env, enum_defs, lookup_mod);
            }
            Stmt::If(i) => {
                if let Some(eb) = &i.else_block {
                    if range_contains_offset(&eb.range, target_off) {
                        return eval_const_in_block(eb, target_off, &env, module_env, enum_defs, lookup_mod);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn hover_at(
    source: &str,
    file: &str,
    symbols: &[SymbolDef],
    type_map: &TypeMap,
    doc_comments: &HashMap<usize, String>,
    if_contexts: &IfContextMap,
    var_read_contexts: &VarReadContextMap,
    dropped_ranges: &[(SourceRange, String)],
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
    col: usize,
) -> Option<String> {
    // Dropped (never type-checked) code reports FIRST, and — like the two
    // non-word hovers below — before `word_at`, which returns `None` for any
    // position that isn't an identifier. A dropped block must report at EVERY
    // position inside it, braces and whitespace and operators included: it is
    // a warning about a region, and someone skimming a block hovers the block,
    // not necessarily a name in it. Behind `word_at` the warning silently
    // vanished on `{`, `}`, `=` and every space.
    if let Some(h) = hover_dropped_range(file, dropped_ranges, line, col) {
        return Some(h);
    }

    // `$` references (prefab files / external assets) aren't identifier words,
    // so detect them from the raw line before the word-based lookups.
    if let Some(h) = hover_asset_ref(source, file, line, col) {
        return Some(h);
    }

    // `:name` atom literals aren't identifier words either — show the
    // compile-time `int` they hash to.
    if let Some(h) = hover_atom(source, file, line, col) {
        return Some(h);
    }

    let word = word_at(source, line, col)?;

    None
        .or_else(|| hover_if_keyword(source, file, &word, if_contexts, resource_estimates, line, col))
        .or_else(|| hover_named_param(source, &word, line, col))
        .or_else(|| hover_event_config_param(source, &word, line, col))
        .or_else(|| hover_data_driven_config(source, &word, line, col))
        .or_else(|| hover_config_enum_value(source, &word, line, col))
        .or_else(|| hover_collection_method(source, symbols, &word, line, col))
        .or_else(|| hover_custom_event(source, file, &word, type_map, line, col))
        .or_else(|| hover_builtin_event(&word))
        .or_else(|| hover_builtin_call(source, &word, line, col))
        .or_else(|| hover_chip_or_mod_keyword(source, &word, symbols, resource_estimates, line))
        .or_else(|| hover_on_keyword(source, &word, resource_estimates, line))
        .or_else(|| hover_record_or_type_field(source, symbols, doc_comments, &word, line, col))
        .or_else(|| hover_namespace_member(source, symbols, doc_comments, resource_estimates, &word, line, col))
        .or_else(|| hover_enum_discriminant_variant_path(source, file, &word, line, col))
        .or_else(|| hover_enum_variant_path(source, file, symbols, &word, line, col))
        .or_else(|| hover_enum_field_construction(source, file, &word, line, col))
        .or_else(|| resolve_field_hover(source, file, type_map, symbols, line, col, &word))
        .or_else(|| hover_generic_call(source, file, symbols, doc_comments, resource_estimates, type_map, &word, line, col))
        .or_else(|| hover_user_symbol(source, file, symbols, doc_comments, var_read_contexts, resource_estimates, &word, line, col))
        .or_else(|| hover_enum_type_name(source, file, &word))
        .or_else(|| hover_type_or_class(&word))
}

/// A short description for a built-in primitive type name.
fn builtin_type_desc(word: &str) -> Option<&'static str> {
    Some(match word {
        "bool" => "Boolean (`true` / `false`).",
        "int" => "64-bit signed integer.",
        "float" => "64-bit floating-point number.",
        "string" => "Text string.",
        "vector" => "3D vector (x, y, z floats).",
        "rotator" => "Euler rotation (pitch, yaw, roll).",
        "quat" => "Quaternion (x, y, z, w) — a rotation value.",
        "color" => "RGBA color (r, g, b, a).",
        "entity" => "Reference to a game entity.",
        "character" => "Reference to a player character.",
        "controller" => "Reference to a player controller.",
        "exec" => "Execution trigger signal — not a data value.",
        "zone" => "Reference to a Zone brick (rerouter-only, like a var ref).",
        "teleport" => "Reference to a Teleport Destination (rerouter-only, like a var ref).",
        "prefab" => "Reference to a prefab (a `$./file.brz` archive, a `$./file.ws` source compiled on reference, or an inline prefab block) — a compile-time constant, not stored.",
        "any" => "Wildcard type — works anywhere but erases the type; prefer a generic `<T>`.",
        "never" => "Bottom type — no value inhabits it.",
        _ => return None,
    })
}

/// Hover for a bare type word: a generic **constraint class** (`Scalar` /
/// `Numeric` / `Variant`), or a built-in primitive type (`int`, `vector`, ...).
/// Runs after user-symbol lookup so a user type alias of the same name still
/// wins; these names are otherwise not declared symbols.
fn hover_type_or_class(word: &str) -> Option<String> {
    if let Some(members) = crate::types::classes::class_mask(word) {
        let names: Vec<String> = members.iter().map(type_str).collect();
        return Some(format!(
            "```wirescript\n{word}  (generic constraint class)\n```\nA bound for a generic type parameter — `<T: {word}>` restricts `T` to one of: {}.",
            names.join(", ")
        ));
    }
    let desc = builtin_type_desc(word)?;
    Some(format!("```wirescript\n{word}\n```\n{desc}"))
}

/// Hover for a `:name` atom literal under the cursor: the compile-time `int`
/// constant it hashes to (xxHash64 of the name), shown in decimal and hex.
fn hover_atom(source: &str, file: &str, line: usize, col: usize) -> Option<String> {
    let a = super::atoms::atom_at(source, file, line, col)?;
    Some(format!(
        "**Atom** `:{name}`\n\nA compile-time **64-bit `int`** constant — the xxHash64 of the \
         name, resolved at compile time (never a runtime string). Every `:{name}` is this value:\n\n\
         - `{dec}` — the signed `int` it is in code (negative when the hash's top bit is set)\n\
         - `{hex:#018x}` — the 64-bit hash (16 hex digits)",
        name = a.name,
        dec = a.value,
        hex = a.value as u64,
    ))
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

    let kind = if path.ends_with(".ws") {
        "Compiles a `.ws` source file and embeds the result"
    } else {
        "Embeds a `.brz` archive"
    };
    let mut out = format!("**Prefab file reference**\n\n{kind} into `SpawnPrefab`.\n\n");
    out += &format!("- Reference: `${path}`\n");
    out += &format!("- Resolves to: `{}`\n", resolved.display());
    if !path.ends_with(".brz") && !path.ends_with(".ws") {
        out += "\nNote: prefab references must end in `.brz` (a prebuilt archive) or `.ws` (a source file) (WS019).\n";
    }
    #[cfg(not(target_arch = "wasm32"))]
    match std::fs::metadata(&resolved) {
        Ok(m) => out += &format!("- On disk: {} bytes\n", m.len()),
        Err(_) => out += "- Not found on disk\n",
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

/// Cursor position inside a block `if constexpr` dropped before type-checking
/// (see `TypeCheckCtx::dropped_ranges`). Runs FIRST in `hover_at` — ahead of
/// every other hover AND ahead of its `word_at` early return — because the
/// untaken branch was never type-checked at all: a stale reference or type
/// error in there is invisible to diagnostics, so this hover is the only place
/// that surfaces it. Being a warning about a whole REGION rather than about a
/// symbol, it must answer at every position in that region (`{`, `}`, `=`,
/// whitespace included), which is exactly what sitting behind `word_at` broke.
///
/// `line`/`col` are 0-based (the `hover_at` convention); `SourceRange`
/// positions are 1-based, so the cursor is converted before comparing.
/// Containment is half-open (`start <= pos < end`), matching the lexer's
/// snapshot-before/snapshot-after token convention that
/// `scoped_refs::range_contains_cursor` also follows — a block's `end` is one
/// past its closing `}`, so the brace itself is inside.
fn hover_dropped_range(
    file: &str,
    dropped_ranges: &[(SourceRange, String)],
    line: usize,
    col: usize,
) -> Option<String> {
    let pos = (line as u32 + 1, col as u32 + 1);
    dropped_ranges.iter().find_map(|(range, reason)| {
        if &*range.file != file {
            return None;
        }
        let start = (range.start.line, range.start.col);
        let end = (range.end.line, range.end.col);
        if start <= pos && pos < end {
            Some(format!("**Removed at compile time** — {reason}."))
        } else {
            None
        }
    })
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
        "```wirescript\nif (exec) -> Branch gate\n```\nExec-context conditional. Produces an **Exec_Branch** gate that routes the exec chain to the true or false arm.".to_string()
    } else {
        "```wirescript\nif (pure) -> Select gate\n```\nPure-context conditional. Produces a **Select** gate that picks one of two values based on the condition.".to_string()
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
/// The scalar kind a `Type` renders a default value as, or `None` for a
/// non-scalar (entity/vector/...) with no displayable constant default.
fn scalar_kind_of(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::Bool => Some("bool"),
        Type::Int => Some("int"),
        Type::Float => Some("float"),
        Type::String => Some("string"),
        _ => None,
    }
}

/// A gate data-struct field's registered default VALUE, rendered for display.
/// Resolves the gate's data struct (`COMPONENT_TYPE_STRUCT_PAIRS`) and reads the
/// field's default from brdb's `STRUCT_DEFAULTS` — the single source of truth the
/// emitter itself uses. An enum field shows its member name (not the stored
/// index); otherwise the value is read in its declared scalar `kind`
/// (`bool`/`int`/`float`/`string`). `None` when the gate has no data struct, the
/// field has no registered default, or the kind is non-scalar.
#[cfg(feature = "brdb-full")]
fn gate_field_default(gate_class: &str, field: &str, kind: &str) -> Option<String> {
    let strct = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(c, _)| *c == gate_class)
        .map(|(_, s)| *s)?;
    let value = brdb::component_db::STRUCT_DEFAULTS
        .iter()
        .find(|(s, _)| *s == strct)
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| *n == field))
        .map(|(_, v)| v.as_ref())?;
    // Enum-typed field: the default is an index; show the member name.
    if let Some(et) = crate::catalog::config_field_enum_type(gate_class, field) {
        if let Ok(idx) = value.as_brdb_u8() {
            let names = crate::catalog::enum_member_names(et);
            return Some(
                names
                    .into_iter()
                    .nth(idx as usize)
                    .unwrap_or_else(|| idx.to_string()),
            );
        }
    }
    match kind {
        "bool" => value.as_brdb_bool().ok().map(|b| b.to_string()),
        "int" => value.as_brdb_i64().ok().map(|i| i.to_string()),
        // Read as f32 (the stored width) so 0.05 doesn't widen to 0.05000000074.
        "float" => value.as_brdb_f32().ok().map(|f| f.to_string()),
        "string" => value.as_brdb_str().ok().map(|s| format!("{s:?}")),
        _ => None,
    }
}

#[cfg(not(feature = "brdb-full"))]
fn gate_field_default(_gate_class: &str, _field: &str, _kind: &str) -> Option<String> {
    None
}

/// A gate data-struct field's registered COMPOSITE default (vector / color /
/// rotator / quat), rendered for display. Reads the struct default the same way
/// [`gate_field_default`] reads scalars, then pulls the composite's named
/// sub-fields (`X`/`Y`/`Z`, `R`/`G`/`B`/`A`, `Pitch`/`Yaw`/`Roll`, ...) through the
/// `AsBrdbValue` struct-property accessor. Colors are stored LINEAR and shown as
/// their sRGB hex (`#181425`); vectors/rotators show a `Vec(...)`/`Rotation(...)`
/// constructor. `None` when the gate registers no such default or `ty` isn't a
/// composite the emitter can bake.
#[cfg(feature = "brdb-full")]
fn composite_field_default(gate_class: &str, field: &str, ty: &Type) -> Option<String> {
    let strct = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(c, _)| *c == gate_class)
        .map(|(_, s)| *s)?;
    let value = brdb::component_db::STRUCT_DEFAULTS
        .iter()
        .find(|(s, _)| *s == strct)
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| *n == field))
        .map(|(_, v)| v.as_ref())?;
    let schema = brdb::schemas::bricks_components_schema_max();
    // Read one named sub-field of the composite as an f64 (every numeric field
    // cross-casts, so f32-backed color channels read fine too). `struct_name`
    // only feeds the accessor's error path, so the prop id doubles for it.
    let read = |prop: &str| -> Option<f64> {
        let id = schema.intern.get(prop)?;
        value
            .as_brdb_struct_prop_value(schema, id, id)
            .ok()?
            .as_brdb_f64()
            .ok()
    };
    match ty {
        Type::Color => Some(render_color_default(
            read("R")?,
            read("G")?,
            read("B")?,
            read("A").unwrap_or(1.0),
        )),
        Type::Vector => {
            let (x, y) = (read("X")?, read("Y")?);
            Some(match read("Z") {
                Some(z) => format!("Vec({}, {}, {})", fnum(x), fnum(y), fnum(z)),
                None => format!("Vec({}, {})", fnum(x), fnum(y)),
            })
        }
        Type::Rotator => {
            // Most rotators name their axes Pitch/Yaw/Roll; a few dump as X/Y/Z.
            let (p, y, r) = match (read("Pitch"), read("Yaw"), read("Roll")) {
                (Some(p), Some(y), Some(r)) => (p, y, r),
                _ => (read("X")?, read("Y")?, read("Z")?),
            };
            Some(format!("Rotation({}, {}, {})", fnum(p), fnum(y), fnum(r)))
        }
        Type::Quat => Some(format!(
            "Quat({}, {}, {}, {})",
            fnum(read("X")?),
            fnum(read("Y")?),
            fnum(read("Z")?),
            fnum(read("W")?),
        )),
        _ => None,
    }
}

#[cfg(not(feature = "brdb-full"))]
fn composite_field_default(_gate_class: &str, _field: &str, _ty: &Type) -> Option<String> {
    None
}

/// A gate field's default rendered for hover — a `Vector2D` sub-port axis
/// (`Position.X`) via [`vector2d_subport_default`], a scalar via
/// [`gate_field_default`], or a composite (vector/color/rotator/quat) via
/// [`composite_field_default`].
fn field_default_display(gate_class: &str, field: &str, ty: &Type) -> Option<String> {
    if let Some((parent, axis)) = field.split_once('.') {
        return vector2d_subport_default(gate_class, parent, axis);
    }
    match scalar_kind_of(ty) {
        Some(kind) => gate_field_default(gate_class, field, kind),
        None => composite_field_default(gate_class, field, ty),
    }
}

/// The registered default for one axis (`"X"`/`"Y"`) of a gate's `Vector2D` data
/// field, rendered for a per-axis layout param hover (`anchorY` -> `Anchor.Y` ->
/// `0.5`). Reads the parent's composite default the same way
/// [`composite_field_default`] does.
#[cfg(feature = "brdb-full")]
fn vector2d_subport_default(gate_class: &str, parent: &str, axis: &str) -> Option<String> {
    let strct = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(c, _)| *c == gate_class)
        .map(|(_, s)| *s)?;
    let value = brdb::component_db::STRUCT_DEFAULTS
        .iter()
        .find(|(s, _)| *s == strct)
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| *n == parent))
        .map(|(_, v)| v.as_ref())?;
    let schema = brdb::schemas::bricks_components_schema_max();
    let id = schema.intern.get(axis)?;
    let f = value
        .as_brdb_struct_prop_value(schema, id, id)
        .ok()?
        .as_brdb_f64()
        .ok()?;
    Some(fnum(f))
}

#[cfg(not(feature = "brdb-full"))]
fn vector2d_subport_default(_gate_class: &str, _parent: &str, _axis: &str) -> Option<String> {
    None
}

/// Render a float without a trailing `.0`, matching the scalar-default style:
/// `1.0 -> "1"`, `0.5 -> "0.5"`, `-1.0 -> "-1"`.
#[cfg(feature = "brdb-full")]
fn fnum(f: f64) -> String {
    f.to_string()
}

/// Linear-light 0–1 component -> sRGB byte (the standard piecewise encode).
#[cfg(feature = "brdb-full")]
fn linear_to_srgb_u8(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Render a stored LINEAR RGBA default as sRGB hex (`#rrggbb`), the form the
/// color appears as in-editor; a non-opaque alpha is noted after it. Alpha is
/// stored linearly (not gamma-encoded), so it scales straight to a byte.
#[cfg(feature = "brdb-full")]
fn render_color_default(r: f64, g: f64, b: f64, a: f64) -> String {
    let (r, g, b) = (
        linear_to_srgb_u8(r),
        linear_to_srgb_u8(g),
        linear_to_srgb_u8(b),
    );
    let hex = format!("#{r:02x}{g:02x}{b:02x}");
    let a8 = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    if a8 == 255 {
        hex
    } else {
        format!("{hex} (alpha {a8})")
    }
}

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

    // A config param surfaced as a plain int but backed by a schema enum shows
    // the enum's name (and its members below) instead of `int`.
    let config_enum = (!crate::catalog::is_wire_input(spec.gate_class, p.port.as_str()))
        .then(|| crate::catalog::config_field_enum_type(spec.gate_class, p.port.as_str()))
        .flatten();
    let ty_label = config_enum
        .map(str::to_string)
        .unwrap_or_else(|| type_str(&p.ty));
    let mut v = format!("**{}** `{}: {}`", display, p.name, ty_label);
    if p.optional { v += " *(optional)*"; }
    if !tooltip.is_empty() { v += &format!("\n\n{}", tooltip); }
    if let Some(et) = config_enum {
        let members = crate::catalog::enum_member_names(et).join(", ");
        if !members.is_empty() {
            v += &format!("\n\none of: {members}");
        }
    }
    // The gate's registered default for this field, if any — scalar, or a
    // composite vector/color (enum params show the member name via
    // `gate_field_default`'s enum resolution).
    if let Some(d) = field_default_display(spec.gate_class, p.port.as_str(), &p.ty) {
        v += &format!("\n\nDefault: `{d}`");
    }
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

/// Collection methods (`arr.push`, `m.get`, ...). Only fires on a `.method`
/// access (the hovered word is immediately preceded by `.`), so a user symbol
/// that happens to share a method name — e.g. `var sum = 0` — still hovers as
/// itself rather than as `array.sum`.
///
/// Which table the method comes from depends on the RECEIVER's type: a map's
/// `length`/`remove`/`clear`/`copyFrom` are distinct from the identically-named
/// array methods, and map-only names (`get`/`set`/`has`/`keys`/`values`) exist
/// on no array. So we resolve the object's type first and dispatch to the right
/// catalog. When the receiver's type can't be recovered (imported var whose span
/// the local `type_map` never keyed), we fall back to the name-based array lookup
/// — the historical behavior — rather than showing nothing.
fn hover_collection_method(
    source: &str,
    symbols: &[SymbolDef],
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    let l = source.lines().nth(line)?;
    let start = word_start_in_line(l, col);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    // Dispatch on the RECEIVER's declared type. The receiver identifier of a
    // method call is NOT recorded as its own expression in `type_map` — only the
    // whole call's result type is, and at the same start offset — so a span
    // lookup there would grab the call's type (e.g. `get`'s `{ Value, Found }`)
    // rather than the receiver's. The symbol table keys type by name, which is
    // exactly the receiver here, and covers both top-level and handler-local vars.
    let obj_end = start - 1;
    let obj_start = word_start_in_line(l, obj_end);
    let obj_name = &l[obj_start..obj_end];

    // The receiver's DECLARED type string (`Map<string, int>`, `Grid<int>`, ...)
    // drives dispatch and is what the map hover displays — the type the user wrote.
    let declared = symbols
        .iter()
        .find(|s| s.name == obj_name)
        .and_then(|s| s.ty.as_deref());
    match declared.map(|ty| collection_kind(ty, symbols)) {
        Some(Some(CollectionKind::Map)) => hover_map_method(word, declared.unwrap()),
        Some(Some(CollectionKind::Array)) => hover_array_method_named(word),
        // A known receiver of a non-collection type (record, scalar, ...): `.word`
        // isn't a collection method on it, so let the field/builtin hovers later
        // in the chain handle it instead of claiming a same-named array method.
        Some(None) => None,
        // Receiver isn't a named symbol we can type (a call/index result, or a name
        // the symbol table doesn't carry): fall back to matching array method names only.
        None => hover_array_method_named(word),
    }
}

/// Render the array-method hover for `word`, or `None` if it isn't one.
fn hover_array_method_named(word: &str) -> Option<String> {
    let m = crate::catalog::arrays::ARRAY_METHODS
        .iter()
        .find(|m| m.name == word)?;
    Some(format!("**array.{}**\n\n{}{} - {}", m.name, m.name, m.signature, m.doc))
}

/// Render the map-method hover for `word` on a receiver whose type displays as
/// `map_display` (e.g. `Map<string, int>`), or `None` if `word` isn't a map
/// method. The concrete key/value types are surfaced so the hover reflects the
/// receiver, not a generic `Map<K, V>`.
fn hover_map_method(word: &str, map_display: &str) -> Option<String> {
    let m = crate::catalog::maps::map_method(word)?;
    Some(format!(
        "**map.{}**\n\n{}{} - {}\n\n*{}*",
        m.name, m.name, m.signature, m.doc, map_display,
    ))
}

/// Built-in event names like `RoundStart`, `CharacterSpawned`, `Clock`, etc.
/// Shows the call's config/input args in the parens and, when the event
/// carries data, the `-> (...)` tuple capture that binds it.
fn hover_builtin_event(word: &str) -> Option<String> {
    let evt = find_event(word)?;
    // Config/inputs are the only things allowed inside the call parens; event
    // data outputs are bound via the trailing `-> (...)` tuple capture.
    let is_custom = matches!(evt.surface_name, "CustomEvent" | "GlobalCustomEvent");
    let mut cfg_parts: Vec<String> = Vec::new();
    if is_custom {
        // Custom events lead with a positional channel-name string, shown as the
        // `"name"` placeholder. It IS the `EventName` config_positional slot, so
        // skip that below to avoid rendering the channel twice.
        cfg_parts.push("\"name\"".to_string());
    }
    cfg_parts.extend(evt.input_named.iter().map(|(s, _, _)| (*s).to_string()));
    if !is_custom {
        cfg_parts.extend(evt.config_positional.iter().map(|s| (*s).to_string()));
    }
    cfg_parts.extend(evt.config_named.iter().map(|(s, _)| (*s).to_string()));
    let call_sig = format!("({})", cfg_parts.join(", "));

    let data_parts: Vec<String> = evt
        .data
        .iter()
        .map(|d| format!("{}: {}", d.name, type_str(&d.ty)))
        .collect();
    // e.g. `on CustomEvent("name") -> (data1: any, ...)`.
    let arrow = if data_parts.is_empty() {
        String::new()
    } else {
        format!(" -> ({})", data_parts.join(", "))
    };
    let mut out = format!("```wirescript\non {}{}{}\n```", evt.surface_name, call_sig, arrow);
    if !evt.input_named.is_empty() {
        let wired: Vec<&str> = evt.input_named.iter().map(|(s, _, _)| *s).collect();
        out += &format!("\n\n**Wired input:** {}", wired.join(", "));
    }
    if !evt.config_named.is_empty() {
        let cfg: Vec<&str> = evt.config_named.iter().map(|(s, _)| *s).collect();
        out += &format!("\n\n**Config:** {} *(constant-only)*", cfg.join(", "));
    }
    Some(out)
}

/// Context-aware hover for the custom-event channel words — both the receiver
/// TRIGGER (`on CustomEvent` / `on GlobalCustomEvent`) and the SEND call
/// (`SendCustomEvent` / `SendGlobalCustomEvent`, including the receiver form
/// `e.SendCustomEvent(...)`). Resolves the channel's data slots (names + types)
/// from every receiver declaration and matching sender in the file, and renders
/// the full typed signature — e.g. `on CustomEvent("init") -> (p: character)` or
/// `SendCustomEvent("init", p: character)`. Returns `None` when the word is not a
/// CE word with a resolvable channel under the cursor, so the generic hovers
/// handle that case.
fn hover_custom_event(
    source: &str,
    file: &str,
    word: &str,
    type_map: &TypeMap,
    line: usize,
    col: usize,
) -> Option<String> {
    // (is_send, receiver-namespace word). Both the trigger and the send call for
    // one namespace resolve against the SAME receivers + senders.
    let (is_send, ns_word) = match word {
        "CustomEvent" => (false, "CustomEvent"),
        "GlobalCustomEvent" => (false, "GlobalCustomEvent"),
        "SendCustomEvent" => (true, "CustomEvent"),
        "SendGlobalCustomEvent" => (true, "GlobalCustomEvent"),
        _ => return None,
    };
    let line_str = source.lines().nth(line)?;
    let word_off = line_offset_at(source, line) + word_start_in_line(line_str, col);

    // Re-parse the same source: identical byte offsets, so `type_map` (keyed by
    // (file, start, end)) still resolves each sender arg's inferred type.
    let parsed = crate::parser::parse(source, file);
    let script = &parsed.ast;

    let channel = if is_send {
        ce_send_channel_at(script, word, word_off)?
    } else {
        ce_trigger_channel_at(script, ns_word, word_off)?
    };
    let slots = resolve_ce_channel_slots(script, ns_word, &channel, type_map, file);

    let data_parts: Vec<String> = slots.iter().map(|(name, ty)| format!("{name}: {ty}")).collect();
    let sig = if is_send {
        // A send call is a plain call: the data values stay in the parens
        // alongside the channel name, e.g. `SendCustomEvent("dmg", amount)`.
        let mut parts = vec![format!("\"{channel}\"")];
        parts.extend(data_parts);
        format!("{word}({})", parts.join(", "))
    } else {
        // A trigger's parens hold config/inputs only (here, just the channel
        // name); the data slots bind via the `-> (...)` tuple capture.
        let arrow = if data_parts.is_empty() {
            String::new()
        } else {
            format!(" -> ({})", data_parts.join(", "))
        };
        format!("on {word}(\"{channel}\"){arrow}")
    };
    Some(format!(
        "```wirescript\n{sig}\n```\n\n\
         *Data slot names/types resolved from this channel's receivers and senders in the file.*"
    ))
}

/// The literal channel name of the `send_name`
/// (`SendCustomEvent`/`SendGlobalCustomEvent`) CALL whose callee identifier
/// contains byte offset `off` — handles both the plain call and the receiver
/// form `e.SendCustomEvent(...)`.
fn ce_send_channel_at(script: &Script, send_name: &str, off: usize) -> Option<String> {
    let mut channel = None;
    {
        let mut on_handler = |_: &Handler| {};
        let mut on_call = |call: &Expr| {
            if channel.is_some() {
                return;
            }
            let Expr::Call { callee, args, .. } = call else {
                return;
            };
            let (cn, crange) = match callee.as_ref() {
                Expr::Ident { name, range } => (name.as_str(), range),
                Expr::FieldAccess { field, range, .. } => (field.as_str(), range),
                _ => return,
            };
            if cn != send_name || off < crange.start.offset || off > crange.end.offset {
                return;
            }
            channel = ce_send_channel(args);
        };
        super::visit::visit_program(script, &mut on_handler, &mut on_call);
    }
    channel
}

/// The literal channel name of the `word` (`CustomEvent`/`GlobalCustomEvent`)
/// receiver handler whose trigger identifier contains byte offset `off`.
fn ce_trigger_channel_at(script: &Script, word: &str, off: usize) -> Option<String> {
    let mut channel = None;
    {
        let mut on_handler = |h: &Handler| {
            if channel.is_some() {
                return;
            }
            let Trigger::Ident { name, range } = &h.trigger else {
                return;
            };
            if name != word || off < range.start.offset || off > range.end.offset {
                return;
            }
            channel = ce_handler_channel(h);
        };
        let mut on_call = |_: &Expr| {};
        super::visit::visit_program(script, &mut on_handler, &mut on_call);
    }
    channel
}

/// The channel a CE receiver handler listens on: its `config`'s named
/// `eventName = "x"` if present, else its first positional string literal.
fn ce_handler_channel(h: &Handler) -> Option<String> {
    for c in &h.config {
        if let HandlerConfigArg::Named { name, value: Expr::StringLit { value, .. } } = c {
            if name.eq_ignore_ascii_case("eventname") {
                return Some(value.clone());
            }
        }
    }
    for c in &h.config {
        if let HandlerConfigArg::Positional(Expr::StringLit { value, .. }) = c {
            return Some(value.clone());
        }
    }
    None
}

/// Resolve a CE channel's data slots to `(name, type)` display strings by
/// merging every receiver declaration (names + declared types) and matching
/// sender call (inferred arg types fill untyped slots) in `script`. Receiver
/// declarations win for both name and type; senders fill slots the receivers
/// left untyped.
fn resolve_ce_channel_slots(
    script: &Script,
    trigger_word: &str,
    channel: &str,
    type_map: &TypeMap,
    file: &str,
) -> Vec<(String, String)> {
    let send_name = if trigger_word == "GlobalCustomEvent" {
        "SendGlobalCustomEvent"
    } else {
        "SendCustomEvent"
    };
    let file_arc: std::sync::Arc<str> = file.into();

    // Per slot: first receiver-declared name, first receiver-declared type,
    // first sender-inferred type. `on_handler` and `on_call` touch disjoint
    // vectors, so neither captures the other's state.
    let mut names: Vec<Option<String>> = Vec::new();
    let mut decl_types: Vec<Option<String>> = Vec::new();
    let mut send_types: Vec<Option<String>> = Vec::new();

    {
        let mut on_handler = |h: &Handler| {
            if !matches!(&h.trigger, Trigger::Ident { name, .. } if name == trigger_word) {
                return;
            }
            if ce_handler_channel(h).as_deref() != Some(channel) {
                return;
            }
            for (i, p) in h.params.iter().enumerate() {
                if names.len() <= i {
                    names.resize(i + 1, None);
                    decl_types.resize(i + 1, None);
                }
                if names[i].is_none() {
                    names[i] = Some(p.name.clone());
                }
                if decl_types[i].is_none() {
                    if let Some(te) = &p.ty {
                        decl_types[i] = Some(crate::analysis::types::type_expr_str(te));
                    }
                }
            }
        };
        let mut on_call = |call: &Expr| {
            let Expr::Call { callee, args, .. } = call else {
                return;
            };
            let cn = match callee.as_ref() {
                Expr::Ident { name, .. } => name.as_str(),
                Expr::FieldAccess { field, .. } => field.as_str(),
                _ => return,
            };
            if cn != send_name || ce_send_channel(args).as_deref() != Some(channel) {
                return;
            }
            for (slot, expr) in ce_send_data_slots(args) {
                if send_types.len() <= slot {
                    send_types.resize(slot + 1, None);
                }
                if send_types[slot].is_none() {
                    let r = expr.range();
                    if let Some(t) =
                        type_map.get(&(file_arc.clone(), r.start.offset, r.end.offset))
                    {
                        if !matches!(t, Type::Any | Type::Opaque) {
                            send_types[slot] = Some(type_str(t));
                        }
                    }
                }
            }
        };
        super::visit::visit_program(script, &mut on_handler, &mut on_call);
    }

    let n = names.len().max(decl_types.len()).max(send_types.len());
    (0..n)
        .map(|i| {
            let name = names
                .get(i)
                .and_then(|o| o.clone())
                .unwrap_or_else(|| format!("data{}", i + 1));
            let ty = decl_types
                .get(i)
                .and_then(|o| o.clone())
                .or_else(|| send_types.get(i).and_then(|o| o.clone()))
                .unwrap_or_else(|| "any".to_string());
            (name, ty)
        })
        .collect()
}

/// The channel name a `SendCustomEvent`-family call targets: named `eventName`
/// if present, else the first positional string literal.
fn ce_send_channel(args: &[CallArg]) -> Option<String> {
    for a in args {
        if let CallArg::Named { name, value: Expr::StringLit { value, .. }, .. } = a {
            if name.eq_ignore_ascii_case("eventname") {
                return Some(value.clone());
            }
        }
    }
    for a in args {
        if let CallArg::Positional(Expr::StringLit { value, .. }) = a {
            return Some(value.clone());
        }
    }
    None
}

/// Map a `SendCustomEvent`-family call's data args to `(0-based slot, value)`.
/// The channel occupies the first positional (unless a named `eventName` was
/// given); the remaining positionals are data slots 0.., and `dataN` names slot
/// N-1. A `target` named arg is neither the channel nor a data slot.
fn ce_send_data_slots(args: &[CallArg]) -> Vec<(usize, &Expr)> {
    let has_named_channel = args
        .iter()
        .any(|a| matches!(a, CallArg::Named { name, .. } if name.eq_ignore_ascii_case("eventname")));
    let mut out = Vec::new();
    let mut pos_idx = 0usize;
    for a in args {
        match a {
            CallArg::Positional(e) => {
                let is_channel = !has_named_channel && pos_idx == 0;
                if !is_channel {
                    let slot = if has_named_channel { pos_idx } else { pos_idx - 1 };
                    out.push((slot, e));
                }
                pos_idx += 1;
            }
            CallArg::Named { name, value, .. } => {
                if let Some(n) = name.strip_prefix("data").and_then(|s| s.parse::<usize>().ok()) {
                    if n >= 1 {
                        out.push((n - 1, value));
                    }
                }
            }
            CallArg::Spread(_) => {}
        }
    }
    out
}

/// Hover for an event handler's config-arg NAME (`enabled` in
/// `on Clock(enabled = true)`) — the call-param hover's event counterpart.
/// Fires only in named-arg-name position, and only when the enclosing trigger
/// is a known event whose config/input args include `word`.
fn hover_event_config_param(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_named_arg_name(source, line, col) {
        return None;
    }
    let trigger = find_enclosing_call(source, line, col)?;
    let evt = find_event(&trigger)?;
    if let Some((_, field)) = evt.config_named.iter().find(|(k, _)| k.eq_ignore_ascii_case(word)) {
        let enum_ty = crate::catalog::config_field_enum_type(evt.gate_class, field);
        let ty_label = enum_ty.unwrap_or("config value");
        let mut v = format!(
            "**{}** `{}: {}` *(event config, constant-only)*\n\nSets `{}` on the `{}` gate.",
            word, word, ty_label, field, evt.surface_name
        );
        if let Some(et) = enum_ty {
            let members = crate::catalog::enum_member_names(et).join(", ");
            if !members.is_empty() {
                v += &format!("\n\none of: {members}");
            }
        }
        return Some(v);
    }
    if evt.input_named.iter().any(|(s, _, _)| s.eq_ignore_ascii_case(word)) {
        return Some(format!(
            "**{}** *(wired input on the `{}` event)*",
            word, evt.surface_name
        ));
    }
    None
}

/// Hover for a data-driven config attribute NAME — a raw settings-menu field
/// (`bOnlyHitPlayerBodyParts`, `FontSize`, `Function`) set by its inventory name
/// rather than a declared param. Fires only in named-arg-name position for a
/// scalar config field the enclosing gate exposes.
fn hover_data_driven_config(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_named_arg_name(source, line, col) {
        return None;
    }
    let callee = find_enclosing_call(source, line, col)?;
    let spec = calls().get(callee.as_str())?;
    // Declared params (friendly aliases) are handled by hover_named_param.
    if spec.params.iter().any(|p| p.name == word) {
        return None;
    }
    let cfg = crate::catalog::scalar_config_field(spec.gate_class, word)?;
    let enum_ty = crate::catalog::config_field_enum_type(spec.gate_class, word);
    let ty_label = enum_ty.unwrap_or(cfg.ty.as_str());
    let mut v = format!("**{word}** `{word}: {ty_label}` *(gate config, constant-only)*");
    if !cfg.display_name.is_empty() {
        v += &format!("\n\n{}", cfg.display_name);
    }
    if let Some(et) = enum_ty {
        let members = crate::catalog::enum_member_names(et).join(", ");
        if !members.is_empty() {
            v += &format!("\n\none of: {members}");
        }
    }
    Some(v)
}

/// Hover for a config enum-member VALUE (`X_Negative` in
/// `direction = X_Negative`, whether on a builtin call or an event): names the
/// schema enum and lists its members.
fn hover_config_enum_value(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    let (param, _value) = super::text::named_arg_value(source, line, col)?;
    let callee = find_enclosing_call(source, line, col)?;
    let et = crate::catalog::config_enum_for_named_arg(&callee, &param)?;
    // The hovered word must actually be a member of that enum (not some other
    // value written in the slot).
    crate::catalog::enum_member_value(et, word)?;
    let members = crate::catalog::enum_member_names(et).join(", ");
    Some(format!(
        "**{word}** — `{et}` member\n\none of: {members}"
    ))
}

// ---------- language-level `enum` hover (user + built-in game enums) ----------
//
// The functions in this block hover a Wirescript `enum` *type* (a
// [`crate::typecheck::enums::EnumDef`]) - distinct from `hover_config_enum_value`
// above, which hovers a raw brdb *schema* enum member written as a gate config
// value (`direction = X_Negative`). A user `enum` re-parses `source` to build
// the same [`crate::typecheck::enums::build_registry`] the compiler itself
// resolves against (the same trick [`hover_custom_event`] and
// [`const_hover_value`] use); a built-in GAME enum (`EasingFunction`, ...) is
// resolved straight from the catalog, since it needs no source at all.

/// A variant's payload shape, rendered the way its construction site would
/// read: empty for a unit variant, `(float, float)` for positional, `{ x:
/// float, y: float }` for named. Mirrors `infer::render_pattern`'s bracket
/// choice, but for the variant's DECLARED type shape rather than a matched
/// pattern.
fn render_variant_payload(v: &crate::typecheck::enums::VariantDef) -> String {
    use crate::typecheck::enums::Payload;
    match &v.payload {
        Payload::Unit => String::new(),
        Payload::Positional(types) => {
            let parts: Vec<String> = types.iter().map(super::types::type_expr_str).collect();
            format!("({})", parts.join(", "))
        }
        Payload::Named(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{n}: {}", super::types::type_expr_str(t)))
                .collect();
            format!(" {{ {} }}", parts.join(", "))
        }
    }
}

/// Hover for a bare enum TYPE name (`Shape` in `var s: Shape`, or the `Shape`
/// in a `Shape.Circle` variant path): what it is (a user `enum` vs a built-in
/// game enum) and its variant list. Runs after [`hover_user_symbol`] in the
/// dispatch chain, so a value symbol of the same name (a `var`/`let`/mod, ...)
/// always wins - an enum type name is never itself a registered value symbol.
fn hover_enum_type_name(source: &str, file: &str, word: &str) -> Option<String> {
    // Built-in game enum: resolved straight from the catalog's memoized table
    // (a cheap linear scan, no reparse) - checked first since it applies to
    // every file, unlike a user `enum`.
    if let Some(v) = hover_builtin_game_enum_type(word) {
        return Some(v);
    }
    // Prelude (`Option`/`Result`): cheap to build (two entries), no reparse.
    if let Some(def) = crate::typecheck::enums::prelude_enum_defs().into_iter().find(|d| d.name == word) {
        return Some(render_user_or_prelude_enum_hover(&def, false));
    }
    // A user `enum`: only worth a reparse when the file could plausibly
    // declare one (mirrors the LSP's own `enum_registry_from_source` fast path).
    if !source.contains("enum ") {
        return None;
    }
    let parsed = crate::parser::parse(source, file);
    let e = find_top_level_enum_decl(&parsed.ast.decls, word)?;
    let def = crate::typecheck::enums::EnumDef {
        name: e.name.clone(),
        type_params: e.type_params.clone(),
        variants: crate::typecheck::enums::variant_defs(e),
    };
    Some(render_user_or_prelude_enum_hover(&def, true))
}

/// Find a top-level `TopDecl::Enum` named `name`, recursing into namespaces.
fn find_top_level_enum_decl<'a>(decls: &'a [crate::ast::TopDecl], name: &str) -> Option<&'a crate::ast::EnumDecl> {
    for d in decls {
        match d {
            TopDecl::Enum(e) if e.name == name => return Some(e),
            TopDecl::Namespace(ns) => {
                if let Some(e) = find_top_level_enum_decl(&ns.decls, name) {
                    return Some(e);
                }
            }
            _ => {}
        }
    }
    None
}

/// Render hover for a user-declared or prelude `EnumDef`: its signature plus a
/// `Variants:` list (each with its payload shape).
fn render_user_or_prelude_enum_hover(def: &crate::typecheck::enums::EnumDef, is_user: bool) -> String {
    let generics = if def.type_params.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = def.type_params.iter().map(|tp| tp.name.as_str()).collect();
        format!("<{}>", names.join(", "))
    };
    let mut out = format!("```wirescript\nenum {}{generics}\n```", def.name);
    let variants: Vec<String> = def
        .variants
        .iter()
        .map(|v| format!("`{}{}`", v.name, render_variant_payload(v)))
        .collect();
    out += &format!("\n\nVariants: {}", variants.join(", "));
    if !is_user {
        out += "\n\n*Built-in prelude enum.*";
    }
    out
}

/// Hover for a built-in GAME enum type name (`EasingFunction`, `Direction`,
/// ...): resolved directly from the catalog's schema-derived table
/// ([`crate::catalog::game_enum_schema_type`], a cheap lookup with no
/// reparse), listing each cleaned variant name with its real schema
/// discriminant ([`crate::catalog::enum_member_value`]).
fn hover_builtin_game_enum_type(word: &str) -> Option<String> {
    let schema_type = crate::catalog::game_enum_schema_type(word)?;
    let variants: Vec<String> = crate::catalog::enum_member_names(schema_type)
        .into_iter()
        .map(|raw| {
            let clean = crate::catalog::clean_game_enum_variant(&raw);
            let disc = crate::catalog::enum_member_value(schema_type, &raw).unwrap_or(0);
            format!("`{clean}` = {disc}")
        })
        .collect();
    Some(format!(
        "```wirescript\nenum {word}\n```\n\n*Built-in game enum* (schema `{schema_type}`).\n\nVariants: {}",
        variants.join(", ")
    ))
}

/// Hover for a VARIANT in a construction path (`Shape.Circle`,
/// `EasingFunction.Bounce`) - the variant token itself, not the enum name
/// before it: its owning enum, payload shape, and real discriminant integer.
/// Reparses `source` to build the full registry (user enums + prelude +
/// built-in game enums, exactly `crate::typecheck::enums::build_registry`,
/// the same table the compiler resolves `Enum.Variant` against), so a
/// built-in and a user enum hover identically here.
///
/// Only fires on a `.`-preceded word whose preceding identifier is NOT
/// shadowed by a value symbol (`var`/`let`/mod/...) - mirrors the compiler's own
/// shadow rule in `resolve_variant_for_construction`: a value binding of the
/// same name as an enum wins, so that name is not a construction site.
fn hover_enum_variant_path(
    source: &str,
    file: &str,
    symbols: &[SymbolDef],
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    let l = source.lines().nth(line)?;
    let start = word_start_in_line(l, col);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let obj_end = start - 1;
    let obj_start = word_start_in_line(l, obj_end);
    let enum_name = &l[obj_start..obj_end];
    if enum_name.is_empty() || super::resolve_symbol(symbols, enum_name, line, col).is_some() {
        return None;
    }

    let parsed = crate::parser::parse(source, file);
    let registry = crate::typecheck::enums::build_registry(&parsed.ast.decls);
    let def = registry.get(enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == word)?;
    let is_user = find_top_level_enum_decl(&parsed.ast.decls, enum_name).is_some();

    let mut out = format!(
        "```wirescript\n{enum_name}.{}{}\n```\n\n**Discriminant:** `{}`",
        vdef.name,
        render_variant_payload(vdef),
        vdef.discriminant
    );
    if !is_user {
        out += "\n\n*Built-in enum member.*";
    }
    Some(out)
}

/// Hover for a named payload FIELD's KEY in an enum-variant CONSTRUCTION
/// (`Shape.Box { w: 1.0, h: 2.0 }` - the `w`/`h`): its declared type, read
/// off `enum Shape { Box { w: float, ... } }`.
///
/// Reparses `source` like its sibling enum hovers ([`hover_enum_type_name`],
/// [`hover_enum_variant_path`]) rather than taking a pre-resolved AST. The
/// field-key-span derivation mirrors
/// `analysis::definition::resolve_enum_field_construction_definition`'s: a
/// `RecordLitField::Named`'s `range` spans the WHOLE `key: value` (see
/// `parser::expr::parse_record_lit`), not just the key, so the key span is
/// derived from the field name's byte length rather than read off a
/// dedicated sub-range.
fn hover_enum_field_construction(source: &str, file: &str, word: &str, line: usize, col: usize) -> Option<String> {
    let line_str = source.lines().nth(line)?;
    let word_off = line_offset_at(source, line) + word_start_in_line(line_str, col);

    let parsed = crate::parser::parse(source, file);
    let mut hit: Option<(String, String)> = None; // (enum name, variant name)
    {
        let mut on_handler = |_: &Handler| {};
        let mut on_call = |e: &Expr| {
            if hit.is_some() {
                return;
            }
            let Expr::VariantCtor { path, fields, .. } = e else {
                return;
            };
            for f in fields {
                let (name, key_start, key_end) = match f {
                    crate::ast::RecordLitField::Named { name, range, .. } => {
                        (name.as_str(), range.start.offset, range.start.offset + name.len())
                    }
                    crate::ast::RecordLitField::Shorthand { name, range } => {
                        (name.as_str(), range.start.offset, range.end.offset)
                    }
                    crate::ast::RecordLitField::Spread { .. } => continue,
                };
                if name != word || word_off < key_start || word_off > key_end {
                    continue;
                }
                let Expr::FieldAccess { obj, field: variant, .. } = path.as_ref() else {
                    continue;
                };
                let Expr::Ident { name: enum_name, .. } = obj.as_ref() else {
                    continue;
                };
                hit = Some((enum_name.clone(), variant.clone()));
            }
        };
        super::visit::visit_program(&parsed.ast, &mut on_handler, &mut on_call);
    }
    let (enum_name, variant_name) = hit?;

    let registry = crate::typecheck::enums::build_registry(&parsed.ast.decls);
    let def = registry.get(&enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == variant_name)?;
    let crate::typecheck::enums::Payload::Named(field_defs) = &vdef.payload else {
        return None;
    };
    let (_, ty) = field_defs.iter().find(|(n, _)| n == word)?;
    Some(format!(
        "```wirescript\n{word}: {}\n```\n\nNamed payload field of `{enum_name}.{variant_name}`.",
        super::types::type_expr_str(ty)
    ))
}

/// Hover for `.Discriminant` on a variant PATH (`Shape.Circle.Discriminant`,
/// `EasingFunction.Bounce.Discriminant`): the discriminant is a compile-time
/// CONSTANT here (the variant is named, not merely typed), so this reports
/// the actual integer rather than just `int`.
///
/// An ordinary enum VALUE's `.Discriminant` (`s.Discriminant` where `s:
/// Shape`) is a two-identifier chain, not three (`Enum.Variant.Discriminant`),
/// so it does not match here and is left to the generic field-type hover in
/// [`resolve_field_hover`], which already resolves it to `field Discriminant:
/// int` via `type_map` (the typechecker types `.Discriminant` as `Type::Int`
/// regardless of whether the object is a bare value or a variant path - see
/// `infer.rs`'s `field == "Discriminant"` arm).
fn hover_enum_discriminant_variant_path(source: &str, file: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if word != "Discriminant" {
        return None;
    }
    let l = source.lines().nth(line)?;
    let start = word_start_in_line(l, col);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let variant_end = start - 1;
    let variant_start = word_start_in_line(l, variant_end);
    if variant_start == 0 || l.as_bytes()[variant_start - 1] != b'.' {
        return None; // a two-identifier chain (`s.Discriminant`) - not a variant path.
    }
    let variant_name = &l[variant_start..variant_end];
    let enum_end = variant_start - 1;
    let enum_start = word_start_in_line(l, enum_end);
    let enum_name = &l[enum_start..enum_end];
    if enum_name.is_empty() {
        return None;
    }

    let parsed = crate::parser::parse(source, file);
    let registry = crate::typecheck::enums::build_registry(&parsed.ast.decls);
    let def = registry.get(enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == variant_name)?;
    Some(format!(
        "```wirescript\n{enum_name}.{variant_name}.Discriminant: int = {}\n```\n\n\
         Compile-time constant - the discriminant of `{enum_name}.{variant_name}`.",
        vdef.discriminant
    ))
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
             - **Hidden from constant folding** - the value stays a live wire, so a probe \
             circuit measures the gate's real behaviour instead of a folded constant.\n\
             - **Type erased for operator resolution** - `Opaque(a) + Opaque(b)` type-checks \
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
    // The game's own SearchTags keywords for this gate (from the inventory dump)
    // — surfaced so hover doubles as a "what would I search for this?" hint.
    let tags_line = crate::catalog::default_catalog()
        .find_by_class(spec.gate_class)
        .map(|g| g.component.search_tags.trim())
        .filter(|t| !t.is_empty())
        .map(|t| format!("*Keywords: {}*", t.split_whitespace().collect::<Vec<_>>().join(", ")));
    if let Some((_, doc)) = override_doc {
        parts.push(doc.to_string());
        if let Some(t) = tags_line { parts.push(t); }
        return Some(parts.join("\n\n"));
    }
    if let Some(g) = gate_doc {
        if !g.description.is_empty() { parts.push(g.description.clone()); }
        let param_docs: Vec<String> = spec.params.iter().filter_map(|p| {
            g.inputs.get(p.port.as_str()).filter(|pd| !pd.tooltip.is_empty()).map(|pd| format!("- **{}** - {}", pd.display_name, pd.tooltip))
        }).collect();
        if !param_docs.is_empty() { parts.push(format!("**Parameters:**\n{}", param_docs.join("\n"))); }
    }
    if let Some(table) = defaults_table(spec) { parts.push(table); }
    if let Some(t) = tags_line { parts.push(t); }
    Some(parts.join("\n\n"))
}

/// A markdown table of a gate's parameter/config defaults (from
/// `gate_field_default`), or `None` if the gate registers none. Covers both the
/// named parameters and the extra settings-menu config fields that aren't
/// surfaced as params — the limits/sweep-style values you'd otherwise look up
/// in-game.
fn defaults_table(spec: &crate::catalog::calls::CallSpec) -> Option<String> {
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for p in &spec.params {
        if let Some(d) = field_default_display(spec.gate_class, p.port.as_str(), &p.ty) {
            // An enum-backed config param shows its enum type, not bare `int`
            // (matching the value, which is rendered as the member name).
            let ty_label = crate::catalog::config_field_enum_type(spec.gate_class, p.port.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| type_str(&p.ty));
            rows.push((p.name.to_string(), ty_label, d));
        }
    }
    // Settings-menu config fields not already listed as a param port.
    for cfg in crate::catalog::scalar_config_fields(spec.gate_class) {
        if spec.params.iter().any(|p| p.port.as_str() == cfg.name) {
            continue;
        }
        if let Some(d) = gate_field_default(spec.gate_class, &cfg.name, &cfg.ty) {
            let ty_label = crate::catalog::config_field_enum_type(spec.gate_class, &cfg.name)
                .map(str::to_string)
                .unwrap_or_else(|| cfg.ty.clone());
            rows.push((cfg.name.clone(), ty_label, d));
        }
    }
    if rows.is_empty() {
        return None;
    }
    let mut t = String::from("**Defaults:**\n\n| Parameter | Type | Default |\n| --- | --- | --- |");
    for (n, ty, d) in &rows {
        t += &format!("\n| {n} | {ty} | {d} |");
    }
    Some(t)
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

    for sym in symbols {
        if (sym.kind == "chip" || sym.kind == "mod")
            && sym.range.start.offset >= lo
            && sym.range.start.offset < line_end
        {
            let context = if sym.exec { "exec" } else { "pure" };
            let name = if sym.name.is_empty() || sym.name.starts_with('_') { "(anonymous)" } else { &sym.name };
            let const_kw = if sym.is_const { "const " } else { "" };
            let mut hover = format!(
                "```wirescript\n{const_kw}{} {} ({})\n```\n\n{} context - {}",
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

/// Hover for a USAGE (call site) of a generic `mod`/`chip`: show the type
/// arguments *resolved for this call* in the angle brackets — e.g.
/// `mod assert<int>(want: int, got: int, label: string)` instead of the
/// declaration's `mod assert<T: int | float | string>(want: T, ...)`.
///
/// Re-parses `source` (identical byte offsets, so `type_map` still resolves
/// each argument's inferred type — the same trick [`hover_custom_event`] uses),
/// finds the `Expr::Call` whose callee identifier spans the cursor, locates the
/// callee's generic declaration, and re-runs the shared call-site inference
/// ([`crate::types::mono::infer_call_subst`], or the caller's explicit
/// `assert<int>(...)` type arguments when present) to bind each type parameter.
///
/// Returns `None` when the word is not the callee of a generic call — so the
/// declaration site and every non-generic call still fall through to
/// [`hover_user_symbol`]'s declaration-signature hover.
fn hover_generic_call(
    source: &str,
    file: &str,
    symbols: &[SymbolDef],
    doc_comments: &HashMap<usize, String>,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    type_map: &TypeMap,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // Only a call whose callee is exactly the hovered word is in play; a bare
    // reference or the declaration itself has no enclosing call here.
    let l = source.lines().nth(line)?;
    let word_off = line_offset_at(source, line) + word_start_in_line(l, col);

    let parsed = crate::parser::parse(source, file);
    let script = &parsed.ast;

    // The `Expr::Call` whose callee identifier `word` spans the cursor.
    let (args, type_args) = generic_call_at(script, word, word_off)?;

    // The callee's declaration must be a generic `mod`/`chip` (only those carry
    // `type_params`; `fn`s and builtins never do).
    let decl = find_generic_chip_decl(&script.decls, word)?;
    if decl.type_params.is_empty() {
        return None;
    }

    // Resolve the declared parameter / output types with the decl's own type
    // params in scope, so a `T` becomes `Type::Param("T")` (aliases aren't
    // needed: only params that mention a `T` can constrain the inference).
    let param_names: Vec<String> = decl.type_params.iter().map(|tp| tp.name.clone()).collect();
    let aliases: HashMap<String, Type> = HashMap::default();
    let generic_aliases: HashMap<String, crate::types::resolve::GenericAlias> = HashMap::default();
    let rcx = crate::types::resolve::ResolveCtx {
        params: &param_names,
        type_aliases: &aliases,
        generic_aliases: &generic_aliases,
    };
    let resolve = |te: &TypeExpr| crate::types::resolve::resolve_type(te, &rcx, &mut Vec::new());
    let param_types: Vec<Type> = decl.inputs.iter().map(|p| resolve(&p.typ)).collect();

    // Bind each type parameter. Explicit `assert<int>(...)` type arguments pin
    // them directly; otherwise infer from the argument types the same way the
    // compiler does at the call site.
    let subst = if !type_args.is_empty() {
        if type_args.len() != decl.type_params.len() {
            return None;
        }
        let mut s = crate::types::infer::Subst::new();
        for (tp, te) in decl.type_params.iter().zip(type_args.iter()) {
            s.insert(tp.name.clone(), resolve(te));
        }
        s
    } else {
        let file_arc: std::sync::Arc<str> = file.into();
        let arg_types: Vec<Type> = positional_arg_types(&args, type_map, &file_arc);
        let params: Vec<(String, Vec<Type>)> = decl
            .type_params
            .iter()
            .map(|tp| (tp.name.clone(), crate::types::mono::mask_for_param(tp.bound.as_ref(), &aliases)))
            .collect();
        crate::types::mono::infer_call_subst(&param_types, &arg_types, &params)
    };

    // Nothing resolved (e.g. a `T` only in the return type, called without
    // explicit args): leave the declaration hover to show the generic form.
    if decl.type_params.iter().all(|tp| !subst.contains_key(&tp.name)) {
        return None;
    }

    // Rebuild the signature string with the resolved types, then reuse
    // `render_decl_hover` so the exec marker, doc comment, and resource
    // estimate render exactly as they do on the declaration.
    let header = {
        let parts: Vec<String> = decl
            .type_params
            .iter()
            .map(|tp| match subst.get(&tp.name) {
                Some(t) => type_str(t),
                None => tp.name.clone(),
            })
            .collect();
        format!("<{}>", parts.join(", "))
    };
    let params_str: Vec<String> = decl
        .inputs
        .iter()
        .zip(param_types.iter())
        .map(|(p, pt)| {
            let ty = type_str(&crate::types::mono::substitute(pt, &subst));
            if p.is_const {
                format!("{}: const {}", p.name, ty)
            } else {
                format!("{}: {}", p.name, ty)
            }
        })
        .collect();
    let ret_suffix = match decl.outputs.as_slice() {
        [] => String::new(),
        [single] => format!(" -> {}", type_str(&crate::types::mono::substitute(&resolve(&single.typ), &subst))),
        multiple => {
            let fields: Vec<String> = multiple
                .iter()
                .map(|o| format!("{}: {}", o.name, type_str(&crate::types::mono::substitute(&resolve(&o.typ), &subst))))
                .collect();
            format!(" -> ({})", fields.join(", "))
        }
    };

    // The declaration symbol carries the kind/exec/const flags and the offsets
    // the doc-comment and estimate lookups key on; only its `ty` changes.
    let sym = super::resolve_symbol(symbols, word, line, col)?;
    if sym.kind != "mod" && sym.kind != "chip" {
        return None;
    }
    let synth = SymbolDef {
        name: sym.name.clone(),
        kind: sym.kind,
        range: sym.range.clone(),
        ty: Some(format!("{}({}){}", header, params_str.join(", "), ret_suffix)),
        exec: sym.exec,
        is_const: sym.is_const,
    };
    let mut out = render_decl_hover(&synth, doc_comments, resource_estimates, Some((source, file)));

    let bindings: Vec<String> = decl
        .type_params
        .iter()
        .filter_map(|tp| subst.get(&tp.name).map(|t| format!("`{}` = `{}`", tp.name, type_str(t))))
        .collect();
    if !bindings.is_empty() {
        out += &format!(
            "\n\n*Generic call — {} for this call.*",
            bindings.join(", ")
        );
    }
    Some(out)
}

/// The `(args, type_args)` of the `Expr::Call` whose callee is the identifier
/// `word` and whose callee range contains byte offset `off`. Walks the whole
/// program (handler/mod/chip bodies included) via the shared visitor.
fn generic_call_at<'a>(
    script: &'a Script,
    word: &str,
    off: usize,
) -> Option<(Vec<CallArg>, Vec<TypeExpr>)> {
    let mut found: Option<(Vec<CallArg>, Vec<TypeExpr>)> = None;
    {
        let mut on_handler = |_: &Handler| {};
        let mut on_call = |call: &Expr| {
            if found.is_some() {
                return;
            }
            let Expr::Call { callee, args, type_args, .. } = call else {
                return;
            };
            let Expr::Ident { name, range } = callee.as_ref() else {
                return;
            };
            if name == word && range.start.offset <= off && off < range.end.offset {
                found = Some((args.clone(), type_args.clone()));
            }
        };
        super::visit::visit_program(script, &mut on_handler, &mut on_call);
    }
    found
}

/// Find the generic `mod`/`chip` declaration named `name` (top-level or inside
/// a namespace). Returns the first match — enough for hover, which only needs
/// the parameter shape and type-param bounds.
fn find_generic_chip_decl<'a>(decls: &'a [TopDecl], name: &str) -> Option<&'a ChipDecl> {
    for d in decls {
        match d {
            TopDecl::Chip(c) if c.name == name && !c.type_params.is_empty() => return Some(c),
            TopDecl::Namespace(ns) => {
                if let Some(c) = find_generic_chip_decl(&ns.decls, name) {
                    return Some(c);
                }
            }
            _ => {}
        }
    }
    None
}

/// The inferred types of a call's leading POSITIONAL arguments, read from the
/// `type_map` by each argument expression's source range. A named/spread arg
/// stops the run (positional inference lines up by position); a missing entry
/// becomes `Type::Any`, which contributes no constraint.
fn positional_arg_types(
    args: &[CallArg],
    type_map: &TypeMap,
    file: &std::sync::Arc<str>,
) -> Vec<Type> {
    let mut out = Vec::new();
    for a in args {
        let CallArg::Positional(e) = a else { break };
        let r = e.range();
        let ty = type_map
            .get(&(file.clone(), r.start.offset, r.end.offset))
            .cloned()
            .unwrap_or(Type::Any);
        out.push(ty);
    }
    out
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
    // Resolve to the declaration in scope at the cursor, so hovering (or reading)
    // a name reused across scopes shows the one actually visible here — e.g.
    // hovering `players` in `var players: character[]` resolves to that array,
    // not a file-scope `players: string`.
    let sym = super::resolve_symbol(symbols, word, line, col)?;

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

    let mut v = render_decl_hover(sym, doc_comments, resource_estimates, Some((source, file)));

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
///
/// `value_ctx`, when given, is `(source, file)` of the file the symbol's OWN
/// declaration lives in — used only to look up a `const` `let`'s computed
/// VALUE (see [`const_hover_value`]). Namespace-member hover passes `None`:
/// the member's declaration lives in a DIFFERENT (imported) file, so
/// re-parsing the CURRENT file could never find it — showing no value there
/// is correct, not a shortcut.
fn render_decl_hover(
    sym: &SymbolDef,
    doc_comments: &HashMap<usize, String>,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    value_ctx: Option<(&str, &str)>,
) -> String {
    let ty_str = sym.ty.as_deref().unwrap_or("unknown");
    let const_kw = if sym.is_const { "const " } else { "" };
    let mut v = match sym.kind {
        "mod" | "chip" | "fn" => {
            // The signature may carry a leading `<T>` generics prefix, so insert
            // `exec` after the FIRST `(`, not at the start of the string.
            let sig = if sym.exec {
                match ty_str.find('(') {
                    Some(i) => {
                        let (head, rest) = ty_str.split_at(i + 1); // head ends with `(`
                        if rest.starts_with(')') {
                            format!("{head}exec{rest}")
                        } else {
                            format!("{head}exec, {rest}")
                        }
                    }
                    None => ty_str.to_string(),
                }
            } else { ty_str.to_string() };
            format!("```wirescript\n{const_kw}{} {}{}\n```", sym.kind, sym.name, sig)
        }
        "typeparam" => {
            // Show the bound; if it's a named constraint class (`Scalar` /
            // `Numeric` / `Variant`), expand it to the concrete types it admits
            // so the reader learns what `T` may be without hovering the bound.
            let (bound, detail) = match sym.ty.as_deref() {
                Some(b) => {
                    let members = crate::types::classes::class_mask(b)
                        .map(|m| {
                            let names: Vec<String> = m.iter().map(type_str).collect();
                            format!(" — one of: {}", names.join(", "))
                        })
                        .unwrap_or_default();
                    (format!(": {b}"), members)
                }
                None => (String::new(), String::new()),
            };
            format!(
                "```wirescript\n{}{}  (generic type parameter)\n```\nA generic type parameter — resolved to a concrete type per call site{detail}.",
                sym.name, bound
            )
        }
        // `name: const string` — the keyword sits before the TYPE, mirroring
        // how the parameter is spelled in the signature itself.
        "param" => {
            let ty_display = if sym.is_const { format!("const {ty_str}") } else { ty_str.to_string() };
            format!("```wirescript\nparam {}: {}\n```", sym.name, ty_display)
        }
        // A `const` binding: show `const`, not `let`, and — the single most
        // useful thing hover can say about a compile-time constant — its
        // computed VALUE, when `value_ctx` lets us attempt one. A value that
        // can't be determined at hover time (needs a call site, or genuinely
        // fails to evaluate) is simply omitted rather than guessed at.
        "let" if sym.is_const => {
            let value = value_ctx
                .and_then(|(source, file)| const_hover_value(source, file, sym.range.start.offset))
                .map(|lit| literal_display(&lit));
            match value {
                Some(val) => format!("```wirescript\nconst {}: {} = {}\n```", sym.name, ty_str, val),
                None => format!("```wirescript\nconst {}: {}\n```", sym.name, ty_str),
            }
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
    Some(render_decl_hover(sym, doc_comments, resource_estimates, None))
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

            for sym in symbols {
                if sym.kind == "type" && sym.name == type_name
                    && let Some(ref ty_str) = sym.ty
                {
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

/// A "fill record fields" code-action result: the missing field lines to insert
/// (`text`) and the 0-based `line`/`col` to insert them at (the cursor).
pub struct RecordFill {
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// A type-checking default literal for a record field of type `ty` (a stringified
/// type, e.g. `int`, `{baz: int}`, `int[]`). Nested records recurse; containers
/// are empty; scalars get their zero/empty literal. Unknown types fall back to
/// `0` (a placeholder the user replaces, like Rust's `todo!()`).
fn default_for_type(ty: &str) -> String {
    let ty = ty.trim().trim_start_matches('*').trim();
    if let Some(inner) = ty.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let fields: Vec<String> = split_record_fields(inner)
            .iter()
            .filter_map(|f| {
                let f = f.trim();
                let c = f.find(':')?;
                Some(format!("{}: {}", f[..c].trim(), default_for_type(f[c + 1..].trim())))
            })
            .collect();
        return if fields.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", fields.join(", "))
        };
    }
    if ty.ends_with("[]") || ty.starts_with("Array<") {
        return "[]".to_string();
    }
    if ty.starts_with("Map<") {
        return "{}".to_string();
    }
    match ty {
        "string" => "\"\"".to_string(),
        "float" => "0.0".to_string(),
        "bool" => "false".to_string(),
        "vector" => "Vec(0.0, 0.0, 0.0)".to_string(),
        _ => "0".to_string(),
    }
}

/// Field names already present in the record literal opened on `brace_line`
/// (a top-level `name:` per line until the matching close brace).
fn present_field_names(lines: &[&str], brace_line: usize) -> Vec<String> {
    let mut present = Vec::new();
    let mut depth = 0i32;
    for l in lines.iter().skip(brace_line) {
        let trimmed = l.trim();
        // A top-level `name:` field entry (depth 1, inside the outer braces).
        if depth == 1
            && let Some(colon) = trimmed.find(':')
            && trimmed[..colon]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
            && !trimmed[..colon].is_empty()
        {
            present.push(trimmed[..colon].trim().to_string());
        }
        for c in l.chars() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return present;
                    }
                }
                _ => {}
            }
        }
    }
    present
}

/// Compute a "fill record fields" edit for a cursor inside a record literal whose
/// expected type is a record. Scans upward for `let name: Type = {` (an inline
/// `{ ... }` record type or a named alias resolved via `symbols`), then returns the
/// missing fields (present ones are skipped, so partial literals complete too),
/// each with a type-appropriate default. `None` if the cursor isn't inside such a
/// literal or every field is already present.
pub fn fill_record_at(
    source: &str,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
) -> Option<RecordFill> {
    let lines: Vec<&str> = source.lines().collect();
    // Find the enclosing `let name: Type = {` at or above the cursor.
    let mut record_ty: Option<String> = None;
    let mut brace_line = 0usize;
    for scan in (0..=line.min(lines.len().saturating_sub(1))).rev() {
        let trimmed = lines[scan].trim();
        if let Some(rest) = trimmed.strip_prefix("let ")
            && let Some(colon) = rest.find(':')
        {
            let type_part = rest[colon + 1..].split('=').next()?.trim();
            let resolved = if type_part.starts_with('{') {
                Some(type_part.to_string())
            } else {
                let tn = type_part.split_whitespace().next().unwrap_or("");
                symbols
                    .iter()
                    .find(|s| s.kind == "type" && s.name == tn)
                    .and_then(|s| s.ty.clone())
            };
            if let Some(rs) = resolved
                && rs.trim_start().starts_with('{')
            {
                record_ty = Some(rs);
                brace_line = scan;
                break;
            }
        }
        // Stop once a line can't be part of the record literal interior.
        let interior = trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("...")
            || trimmed.contains(':')
            || trimmed.contains(',')
            || trimmed.ends_with('{')
            || trimmed.ends_with('}');
        if !interior {
            return None;
        }
    }
    let record_ty = record_ty?;
    let inner = record_ty.trim().strip_prefix('{')?.strip_suffix('}')?;
    let expected: Vec<(String, String)> = split_record_fields(inner)
        .iter()
        .filter_map(|f| {
            let f = f.trim();
            let c = f.find(':')?;
            Some((f[..c].trim().to_string(), f[c + 1..].trim().to_string()))
        })
        .collect();
    if expected.is_empty() {
        return None;
    }
    let present = present_field_names(&lines, brace_line);
    let missing: Vec<&(String, String)> = expected
        .iter()
        .filter(|(n, _)| !present.contains(n))
        .collect();
    if missing.is_empty() {
        return None;
    }
    let base_indent = lines[brace_line].len() - lines[brace_line].trim_start().len();
    let indent = " ".repeat(base_indent + 2);
    let text = missing
        .iter()
        .map(|(n, t)| format!("{indent}{n}: {},", default_for_type(t)))
        .collect::<Vec<_>>()
        .join("\n");
    Some(RecordFill { line, col, text })
}

/// A "fill missing match arms" code-action result: the missing arm lines to
/// insert (`text`, one `{pattern} => todo,` per witness) and the 0-based
/// `line`/`col` to insert them at (just before the match's closing `}`).
pub struct MatchArmsFill {
    pub line: usize,
    pub col: usize,
    pub text: String,
}

/// True if 1-based `(line, col)` falls within `[r.start, r.end)` (half-open,
/// matching the lexer's snapshot-before/snapshot-after token convention:
/// `end` is one past the last consumed char). Cursor coordinates are 1-based
/// here because `Pos::line`/`Pos::col` are; callers convert the LSP 0-based
/// request first.
fn range_contains_1based(r: &SourceRange, line: u32, col: u32) -> bool {
    (r.start.line, r.start.col) <= (line, col) && (line, col) < (r.end.line, r.end.col)
}

/// The smallest `MatchExpr` in `ast` whose range contains the 1-based
/// `(line, col)` cursor: the innermost enclosing one when matches nest (an
/// arm's body can itself be a match). The closure's parameter type is
/// annotated with the function's own `'a` (rather than left for inference)
/// so the collected `&'a Expr`s can outlive [`super::visit::visit_program`]'s
/// call. A plain inferred closure type ties `e` to a fresh, closure-local
/// lifetime that cannot escape (E0521).
fn enclosing_match_expr<'a>(ast: &'a Script, line: u32, col: u32) -> Option<&'a Expr> {
    let mut candidates: Vec<&'a Expr> = Vec::new();
    let mut on_handler = |_: &'a Handler| {};
    let mut on_call = |e: &'a Expr| {
        if matches!(e, Expr::MatchExpr { .. }) {
            candidates.push(e);
        }
    };
    super::visit::visit_program(ast, &mut on_handler, &mut on_call);
    candidates
        .into_iter()
        .filter(|e| range_contains_1based(e.range(), line, col))
        .min_by_key(|e| {
            let r = e.range();
            r.end.offset.saturating_sub(r.start.offset)
        })
}

/// Compute a "fill missing match arms" edit for a cursor on/inside a `match`
/// whose written arms don't cover its scrutinee enum. Locates the smallest
/// enclosing `MatchExpr` in the pre-resolve `ast` containing `(line, col)`,
/// resolves the scrutinee's type through `type_map` (it must be a registered
/// `Type::Enum`), and asks the shared witness engine
/// ([`crate::typecheck::patterns::analyze`], Task 11, the same one the
/// compiler's WS054 exhaustiveness diagnostic uses) which arms the ones
/// already written don't cover. Each witness renders through
/// [`crate::typecheck::infer::render_pattern`] (the same renderer WS054
/// uses) as one `  {pattern} => todo,` line, inserted right before the
/// match's closing `}` with the indentation of its last arm (or, for an
/// empty match, the match's own line plus two spaces).
///
/// `None` when the cursor isn't on a `match`, its scrutinee didn't resolve to
/// a registered enum, or the arms already cover every variant.
pub fn fill_match_arms_at(
    source: &str,
    _symbols: &[SymbolDef],
    type_map: &TypeMap,
    ast: &Script,
    line: usize,
    col: usize,
) -> Option<MatchArmsFill> {
    let cursor_line = (line + 1) as u32;
    let cursor_col = (col + 1) as u32;

    let best = enclosing_match_expr(ast, cursor_line, cursor_col)?;
    let Expr::MatchExpr { scrutinee, arms, range } = best else {
        return None;
    };

    let sr = scrutinee.range();
    let scrut_ty = type_map.get(&(sr.file.clone(), sr.start.offset, sr.end.offset))?;
    if !matches!(scrut_ty, Type::Enum { .. }) {
        return None;
    }

    let enum_defs = crate::typecheck::enums::build_registry(&ast.decls);
    let arm_patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
    let usefulness = crate::typecheck::patterns::analyze(&enum_defs, scrut_ty, &arm_patterns);
    if usefulness.missing.is_empty() {
        return None;
    }

    let lines: Vec<&str> = source.lines().collect();
    let line_indent = |idx: usize| -> String {
        let l = lines.get(idx).copied().unwrap_or("");
        l[..l.len() - l.trim_start().len()].to_string()
    };
    let indent = match arms.last() {
        Some(last) => line_indent((last.range.start.line - 1) as usize),
        None => format!("{}  ", line_indent((range.start.line - 1) as usize)),
    };

    // Each missing arm gets a bare `todo` placeholder body. `todo` is a plain
    // identifier, so the inserted text PARSES: it lands as an undefined
    // identifier (WS002), which is exactly the "fill me in" state the author
    // then replaces. LSP snippet syntax (`${1:todo}` tab-stops) is deliberately
    // NOT used here: this server advertises no snippet capability and the
    // lsp-types version in use has no SnippetTextEdit, so a snippet placeholder
    // would land in the buffer as literal `${1:todo}` characters, and a bare
    // `$` is a hard parse error (WSP001). Real tab-stops can be restored if the
    // LSP library gains SnippetTextEdit and the client advertises the capability.
    let mut text = usefulness
        .missing
        .iter()
        .map(|w| {
            let pat = crate::typecheck::infer::render_pattern(&w.0);
            format!("{indent}{pat} => todo,\n")
        })
        .collect::<String>();

    // The match's own `range.end` is one past its closing `}` (the parser
    // sets it from the `}` token's own `end`), so the `}` itself sits at
    // `range.end.col - 2` (0-based) on `range.end.line - 1` (0-based): the
    // insertion point that keeps the new arms inside the braces.
    let close_line = (range.end.line - 1) as usize;
    let close_col = (range.end.col as usize).saturating_sub(2);
    // A `}` sharing its line with the last arm (`match s { Circle(r) => 1.0 }`)
    // needs its own line break before the inserted arms; a `}` already alone
    // on its line (the common, formatted case) does not.
    let close_line_str = lines.get(close_line).copied().unwrap_or("");
    let before_close = &close_line_str[..close_col.min(close_line_str.len())];
    if !before_close.trim().is_empty() {
        text.insert(0, '\n');
    }
    Some(MatchArmsFill { line: close_line, col: close_col, text })
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
    // recorded the type of the entire dotted expression. Skip a bare `any`: for a
    // `.field` that's the error-fallback type of a field that didn't resolve, so
    // fall through to structural resolution / the record-type fallback below
    // rather than showing an unhelpful `field z: any`.
    if let Some(ty) = type_map.get(&(f.clone(), lo + obj_start, lo + field_end_col))
        && !matches!(ty, Type::Any)
    {
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
    if !obj_name.is_empty()
        && let Some(hit) = resolve_field_via_symbols(symbols, obj_name, field).map(fmt_field)
    {
        return Some(hit);
    }

    // Fallback: the field didn't resolve, but if the object IS a record, show
    // its whole type — one field per line in a fenced `wirescript` block, which
    // VS Code syntax-COLOURS. Hovering an erroring `x.Jump` then lists the valid
    // fields, coloured. (The diagnostic message reporting the same error stays
    // plain text — VS Code diagnostics don't support markup/colour.) Try the
    // typed `type_map` first (gives a real `Type::Record` for a multi-line
    // render), then the symbol table (a record TYPE STRING for named aliases /
    // `in` ports the type_map didn't key at this span).
    let obj_ty = find_obj_type(type_map, &f, lo + obj_start, lo + obj_end).or_else(|| {
        type_map
            .iter()
            .filter(|((f2, _, e), _)| **f2 == *f && *e == lo + obj_end)
            .max_by_key(|((_, s, _), _)| *s)
            .map(|(_, t)| t.clone())
    });
    if let Some(ty) = &obj_ty {
        let rec = match ty {
            Type::Ref(inner) => inner.as_ref(),
            other => other,
        };
        if let Type::Record(fields) = rec {
            let body: String = fields
                .iter()
                .map(|(n, t)| format!("\n  {n}: {},", type_str(t)))
                .collect();
            return Some(format!("```wirescript\n{{{body}\n}}\n```"));
        }
    }
    if !obj_name.is_empty()
        && let Some(rec) = resolve_object_record_string(symbols, obj_name)
    {
        return Some(render_record_type_string_hover(&rec));
    }

    None
}

/// The object's resolved record TYPE STRING (`{ x: int, y: int }`) from the
/// symbol table — either an inline record type on the symbol itself, or a named
/// alias resolved to its `type` declaration. `None` if the object isn't a record.
fn resolve_object_record_string(symbols: &[SymbolDef], obj_name: &str) -> Option<String> {
    let sym = symbols.iter().find(|s| s.name == obj_name)?;
    let ty_name = sym.ty.as_deref()?;
    if ty_name.starts_with('{') {
        return Some(ty_name.to_string());
    }
    symbols
        .iter()
        .find(|ts| ts.kind == "type" && ts.name == ty_name)
        .and_then(|ts| ts.ty.clone())
        .filter(|s| s.trim_start().starts_with('{'))
}

/// Reformat a single-line record type string (`{ x: int, y: int }`) into a
/// fenced, one-field-per-line `wirescript` block so VS Code colours it.
fn render_record_type_string_hover(rec: &str) -> String {
    let inner = rec.trim().trim_start_matches('{').trim_end_matches('}').trim();
    let body: String = inner
        .split(", ")
        .filter(|f| !f.trim().is_empty())
        .map(|f| format!("\n  {},", f.trim()))
        .collect();
    format!("```wirescript\n{{{body}\n}}\n```")
}

/// Look up `obj_name` in symbols, find its type declaration, and resolve `field`
/// from the type's string representation.
fn resolve_field_via_symbols(symbols: &[SymbolDef], obj_name: &str, field: &str) -> Option<String> {
    let sym = symbols.iter().find(|s| s.name == obj_name)?;
    let ty_name = sym.ty.as_deref()?;

    symbols.iter()
        .find(|ts| ts.kind == "type" && ts.name == ty_name)
        .and_then(|ts| ts.ty.as_deref())
        .and_then(|ty_str| extract_record_field_type(ty_str, field))
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
mod tests;
