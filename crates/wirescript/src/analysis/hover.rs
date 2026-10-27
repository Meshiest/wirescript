use std::fmt::Write as _;
use std::sync::Arc;

use super::doc_links::{Doc, doc_url, with_doc_link};
use super::format::TextKind;
use super::gate_docs::gate_docs;
use super::resource_estimate::{ResourceEstimate, lookup_estimate};
use super::symbols::SymbolDef;
use super::text::{after_char, char_col_to_byte, find_enclosing_call, line_text, word_at};
use super::types::{CollectionKind, collection_kind, type_expr_str, type_str};
use super::visit::visit_program;
use super::{IfContextMap, NarrowedReadMap, TypeMap, VarReadContextMap};
use crate::ast::{
    Block, CallArg, ChipDecl, Expr, Handler, HandlerConfigArg, ImportKind, ImportScope, LetBinding,
    LetDecl, Pattern, RecordLitField, Script, Stmt, TopDecl, Trigger, TypeExpr,
};
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;
use crate::catalog::{config_field_enum_type, enum_member_names};
use crate::collections::{HashMap, HashSet};
use crate::const_eval::{ConstEnv, DeclaredTypes};
use crate::diagnostic::SourceRange;
use crate::ir::{Literal, Type};
use crate::parser::DocComments;
use crate::typecheck::enums::{DISCRIMINANT_FIELD, EnumRegistry, build_registry};
use crate::types::mono::substitute;

#[derive(Clone, Copy)]
enum EstimateKind {
    Chip,
    Mod,
    Fn,
    Scope,
}

/// Byte offset of the start of `line` within `source`.
fn line_offset_at(source: &str, line: usize) -> usize {
    super::text::line_start_byte(source, line)
}

/// Byte offset of the start of the word (alphanumeric or `_`) containing byte
/// offset `b` in `line_str`.
fn word_start_at_byte(line_str: &str, b: usize) -> usize {
    let b = b.min(line_str.len());
    line_str[..b]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| after_char(line_str, i))
        .unwrap_or(0)
}

/// Byte offset of the start of the word containing 0-based CHAR column `col`.
/// Every cursor column arriving from an editor is a char column; the returned
/// offset is a byte offset, which is what the rest of this file slices with.
fn word_start_in_line(line_str: &str, col: usize) -> usize {
    word_start_at_byte(line_str, char_col_to_byte(line_str, col))
}

/// Byte offset one past the end of the word containing byte offset `b`.
fn word_end_at_byte(line_str: &str, b: usize) -> usize {
    let b = b.min(line_str.len());
    line_str[b..]
        .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map_or(line_str.len(), |i| b + i)
}

fn estimate_kind(sym_kind: &str) -> EstimateKind {
    match sym_kind {
        "mod" => EstimateKind::Mod,
        "fn" => EstimateKind::Fn,
        _ => EstimateKind::Chip,
    }
}

fn format_estimate(est: &ResourceEstimate, kind: EstimateKind) -> String {
    // A `const mod` called from a const-required position is evaluated at
    // compile time and emits no gates. A call from a non-const position is an
    // ordinary inlined mod; the docs carry that caveat, not this line.
    if est.is_const_eval {
        return "*const: 0 gates*".to_string();
    }
    let chips = match kind {
        EstimateKind::Chip => est.total_microchips + 1,
        _ => est.total_microchips,
    };
    let mut parts = vec![format!("~{} gates", est.gates)];
    // A `mod` with no inner microchips shows no chip clause, since "0 chips"
    // reads like a missing count. A `chip` counts itself, so it always shows one.
    if chips > 0 {
        parts.push(format!(
            "{} chip{}",
            chips,
            if chips == 1 { "" } else { "s" }
        ));
    }
    match kind {
        EstimateKind::Mod => parts.push("inlined per call".into()),
        EstimateKind::Fn => parts.push("built once, 1 gate per call".into()),
        _ => {}
    }
    format!("*{}*", parts.join(", "))
}

/// Render a [`Literal`] roughly the way its wirescript literal syntax would
/// read, for hover's `const NAME: TYPE = VALUE` display. Output is capped at
/// `MAX_CHARS` characters, with a trailing `...` marking the cut.
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
        // The 3-decimal, trailing-zero-trimmed rendering `@label(expr)` and
        // FormatText use, so a const float hovers the way it displays in game.
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
                .map(|(k, v)| {
                    format!(
                        "{} => {}",
                        literal_display_inner(k),
                        literal_display_inner(v)
                    )
                })
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
        Literal::Asset {
            asset_type,
            asset_name,
        } => format!("${asset_type}/{asset_name}"),
        Literal::PrefabRef { path } => format!("${path}"),
        Literal::NestedPrefab { .. } => "$```...```".to_string(),
        Literal::PrefabBytes(bytes) => format!("<prefab, {} bytes>", bytes.len()),
    }
}

/// The compile-time value of a `const` binding, for the `const NAME: TYPE =
/// VALUE` hover. The `LetDecl` is the one in `ast` whose range starts at
/// `target_off`.
///
/// A top-level const takes its value from `crate::const_eval::build_const_env_in`,
/// so hover shows what lowering bakes. One inside a `mod` or `chip` body is
/// evaluated over that body's statements up to it, seeded with each earlier
/// sibling that evaluates; a nested named `mod`/`chip` starts again from the
/// module's environment, an anonymous `chip { }` shares its parent's. A const
/// that reads one of the body's parameters gives `None`, not a placeholder.
/// `symbols` is the merged program's table (see [`import_shadowable`]).
fn const_hover_value(ast: &Script, symbols: &[SymbolDef], target_off: usize) -> Option<Literal> {
    let script = ast;
    let enum_defs = Arc::new(build_registry(&script.decls));
    let types = Arc::new(DeclaredTypes::for_program_in(
        &script.decls,
        &enum_defs,
        &script.imports,
    ));
    let mut module_env =
        crate::const_eval::build_const_env_in(&script.decls, &enum_defs, &script.imports, &types);
    let mods = const_mod_table(&script.decls);
    let lookup = |name: &str| mods.get(name).cloned();
    let shadowable = import_shadowable(&script.decls, symbols);
    // A constant computed through a builtin an import may replace has the
    // value the builtin gives here, which is not the one the program gets.
    let mut unknown: HashSet<String> = HashSet::default();
    loop {
        let before = unknown.len();
        for d in &script.decls {
            if let TopDecl::Let(l) = d
                && let LetBinding::Ident { name, .. } = &l.binding
                && (calls_shadowable(&l.value, shadowable.as_ref())
                    || reads_any(&l.value, &unknown))
            {
                unknown.insert(name.clone());
            }
        }
        if unknown.len() == before {
            break;
        }
    }
    for name in &unknown {
        module_env.remove(name);
    }
    let scope = EvalScope {
        lookup: &lookup,
        imports: Arc::clone(&script.imports),
        types,
        import_shadowable: shadowable,
        module_env: Arc::new(module_env),
        enum_defs,
    };
    eval_const_at(&script.decls, target_off, &scope.module_env, &scope)
}

/// The builtin names an import may replace with a user `mod` the file itself
/// does not declare (hover sees the file before its imports merge): the names a
/// named import brings, and for a whole-module import the mods and chips the
/// merged program (`symbols`) declares elsewhere. `None` means every name, for a
/// whole-module import whose declarations are unknown.
fn import_shadowable(decls: &[TopDecl], symbols: &[SymbolDef]) -> Option<HashSet<String>> {
    let mut names = HashSet::default();
    let mut whole = false;
    for d in decls {
        if let TopDecl::Import(i) = d {
            match &i.kind {
                ImportKind::All => whole = true,
                ImportKind::Named(bindings) => {
                    names.extend(
                        bindings
                            .iter()
                            .map(|b| b.alias.clone().unwrap_or_else(|| b.name.clone())),
                    );
                }
                ImportKind::Namespace(_) => {}
            }
        }
    }
    if whole {
        let own = decls
            .iter()
            .find(|d| !matches!(d, TopDecl::Import(_)))
            .map(|d| Arc::clone(&d.range().file))?;
        let foreign: Vec<&SymbolDef> = symbols.iter().filter(|s| s.range.file != own).collect();
        if foreign.is_empty() {
            return None;
        }
        names.extend(
            foreign
                .into_iter()
                .filter(|s| matches!(s.kind, "mod" | "chip" | "fn"))
                .map(|s| s.name.clone()),
        );
    }
    Some(names)
}

/// Whether `e` reads one of `names`.
fn reads_any(e: &Expr, names: &HashSet<String>) -> bool {
    use crate::ast::walk::{Node, any_below};
    if names.is_empty() {
        return false;
    }
    let mut read =
        |n: Node<'_>| matches!(n, Node::Expr(Expr::Ident { name, .. }) if names.contains(name));
    read(Node::Expr(e)) || any_below(Node::Expr(e), |_| true, &mut read)
}

/// Every top-level `const mod`, by name. Mirrors the table
/// `crate::const_eval::build_const_env_in` builds internally, so a const initializer
/// that CALLS a const mod resolves here the same way it resolves for real.
fn const_mod_table(decls: &[TopDecl]) -> HashMap<String, Arc<ChipDecl>> {
    decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Chip(c) if c.is_const => Some((c.name.clone(), Arc::new(c.clone()))),
            _ => None,
        })
        .collect()
}

/// Whether `e` calls a builtin an import may replace (`shadowable`, where
/// `None` stands for every builtin).
fn calls_shadowable(e: &Expr, shadowable: Option<&HashSet<String>>) -> bool {
    use crate::ast::walk::{Node, any_below};
    let mut hit = |n: Node<'_>| match n {
        Node::Expr(Expr::Call { callee, .. }) => {
            matches!(callee.as_ref(), Expr::Ident { name, .. }
            if crate::catalog::calls::find_call(name).is_some()
                && shadowable.is_none_or(|s| s.contains(name)))
        }
        _ => false,
    };
    hit(Node::Expr(e)) || any_below(Node::Expr(e), |_| true, &mut hit)
}

type LookupMod<'a> = dyn Fn(&str) -> Option<Arc<ChipDecl>> + 'a;

/// What a hovered constant evaluates against: the module's constants and
/// enums, and the `const mod`s it can call and which of them each file names
/// (a builtin-named one replaces the builtin only where named, as it does
/// when compiling).
struct EvalScope<'a> {
    lookup: &'a LookupMod<'a>,
    imports: Arc<ImportScope>,
    types: Arc<DeclaredTypes>,
    /// See [`import_shadowable`].
    import_shadowable: Option<HashSet<String>>,
    module_env: Arc<ConstEnv>,
    enum_defs: Arc<EnumRegistry>,
}

/// Evaluate one expression against `env` and the module constants, discarding
/// any error: hover has no diagnostic to attach a reason to, so "couldn't
/// evaluate" and "evaluated to something surprising" are both just `None`.
fn eval_one(expr: &Expr, env: &ConstEnv, scope: &EvalScope<'_>) -> Option<Literal> {
    if calls_shadowable(expr, scope.import_shadowable.as_ref()) {
        return None;
    }
    let cx = crate::const_eval::ConstCtx::new(
        crate::const_scope::ConstNames::of(Arc::new(env.clone())),
        Arc::clone(&scope.module_env),
        Arc::default(),
        Arc::clone(&scope.enum_defs),
        Arc::clone(&scope.imports),
        Arc::clone(&scope.types),
        Some(scope.lookup),
    );
    let mut budget = crate::const_eval::Budget::default();
    crate::const_eval::eval_expr(expr, &cx, &mut budget).ok()
}

fn range_contains_offset(range: &SourceRange, off: usize) -> bool {
    range.start.offset <= off && off < range.end.offset
}

/// Search `decls` (top-level, or a namespace's) for the `LetDecl` whose range
/// starts at `target_off`, evaluating const siblings along the way. See
/// [`const_hover_value`] for the scoping rules.
fn eval_const_at(
    decls: &[TopDecl],
    target_off: usize,
    env0: &ConstEnv,
    scope: &EvalScope<'_>,
) -> Option<Literal> {
    let mut env = env0.clone();
    for d in decls {
        match d {
            TopDecl::Let(l) => {
                if l.range.start.offset == target_off {
                    return l
                        .is_const
                        .then(|| eval_one(&l.value, &env, scope))
                        .flatten();
                }
                bind_let(l, &mut env, scope);
            }
            // A named mod/chip body is its OWN scope: reset to the module
            // env alone, with no outer locals and deliberately no params,
            // which hide the module constants they are named like.
            TopDecl::Chip(c) if range_contains_offset(&c.range, target_off) => {
                let env = body_env(c, &scope.module_env);
                return eval_const_in_block(&c.body, target_off, &env, scope);
            }
            // An anonymous chip shares its parent's scope.
            TopDecl::AnonChip(ac) if range_contains_offset(&ac.range, target_off) => {
                return eval_const_in_block(&ac.body, target_off, &env, scope);
            }
            TopDecl::Namespace(ns) if range_contains_offset(&ns.range, target_off) => {
                return eval_const_at(&ns.decls, target_off, env0, scope);
            }
            _ => {}
        }
    }
    None
}

/// Bind what `l` binds in `env`: the value of a `let` of a constant, and for
/// one that is not constant, nothing, hiding the constants its names shadow,
/// as compiling does.
fn bind_let(l: &LetDecl, env: &mut ConstEnv, scope: &EvalScope<'_>) {
    let value = eval_one(&l.value, env, scope);
    match value.and_then(|v| crate::const_eval::bind_destructured(&l.binding, v).ok()) {
        Some(pairs) => env.extend(pairs),
        None => {
            for name in crate::const_eval::bound_names(&l.binding) {
                env.remove(&name);
            }
        }
    }
}

/// The constants a `mod`/`chip` body starts with: the module's, less the
/// names its parameters take.
fn body_env(c: &ChipDecl, module_env: &ConstEnv) -> ConstEnv {
    let mut env = module_env.clone();
    for p in &c.inputs {
        env.remove(&p.name);
    }
    env
}

/// Same walk as [`eval_const_at`], over a statement block (a mod/chip/anon-chip
/// body, or an `if` arm).
fn eval_const_in_block(
    block: &Block,
    target_off: usize,
    env0: &ConstEnv,
    scope: &EvalScope<'_>,
) -> Option<Literal> {
    let mut env = env0.clone();
    for s in &block.stmts {
        match s {
            Stmt::Let(l) => {
                if l.range.start.offset == target_off {
                    return l
                        .is_const
                        .then(|| eval_one(&l.value, &env, scope))
                        .flatten();
                }
                bind_let(l, &mut env, scope);
            }
            // A `var` hides the constant it is named like.
            Stmt::Var(v) => {
                env.remove(&v.name);
            }
            Stmt::ChipDecl(c) if range_contains_offset(&c.range, target_off) => {
                let env = body_env(c, &scope.module_env);
                return eval_const_in_block(&c.body, target_off, &env, scope);
            }
            Stmt::AnonChip(ac) if range_contains_offset(&ac.range, target_off) => {
                return eval_const_in_block(&ac.body, target_off, &env, scope);
            }
            Stmt::If(i) if range_contains_offset(&i.then_block.range, target_off) => {
                return eval_const_in_block(&i.then_block, target_off, &env, scope);
            }
            Stmt::If(i) => {
                if let Some(eb) = &i.else_block
                    && range_contains_offset(&eb.range, target_off)
                {
                    return eval_const_in_block(eb, target_off, &env, scope);
                }
            }
            _ => {}
        }
    }
    None
}

/// `ast` is the file's pre-resolve parse. Hover runs per keystroke, so the
/// caller supplies it rather than this parsing again.
pub fn hover_at(
    source: &str,
    file: &str,
    ast: &Script,
    symbols: &[SymbolDef],
    type_map: &TypeMap,
    doc_comments: &DocComments,
    if_contexts: &IfContextMap,
    var_read_contexts: &VarReadContextMap,
    narrowed_reads: &NarrowedReadMap,
    dropped_ranges: &[(SourceRange, String)],
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
    col: usize,
) -> Option<String> {
    // Dropped (never type-checked) code reports first, before `word_at`, which
    // returns `None` off an identifier: the warning covers a region and must
    // answer at every position in it, braces and whitespace included.
    if let Some(h) = hover_dropped_range(file, dropped_ranges, line, col) {
        return Some(h);
    }

    // `$` references (prefab files / external assets) aren't identifier words,
    // so detect them from the raw line before the word-based lookups.
    if let Some(h) = hover_asset_ref(source, file, line, col) {
        return Some(h);
    }

    // `:name` atom literals aren't identifier words either; show the
    // compile-time `int` they hash to.
    if let Some(h) = hover_atom(source, file, line, col) {
        return Some(with_doc_link(h, Doc::Topic("atoms")));
    }

    let word = word_at(source, line, col)?;

    // A word in a comment or in a string's own text is prose, not a name,
    // even when it is spelled like one. The one exception is a quoted enum
    // setting (`function = "Bounce"`).
    match super::format::text_kind_at(source, line, col) {
        TextKind::Comment => return None,
        TextKind::Str => return hover_config_enum_value(source, &word, line, col),
        TextKind::Code => {}
    }

    // `@color`, `@material`, `@label`: the word after an `@` is the
    // annotation, never a symbol that happens to share its spelling (the
    // `color` type, a pin named `material`).
    if annotation_word_at(source, line, col) {
        return hover_annotation(&word).map(|h| with_doc_link(h, Doc::Annotation(&word)));
    }

    if let Some(h) = hover_environment_path(source, symbols, line, col) {
        return Some(h);
    }

    None.or_else(|| {
        hover_if_keyword(
            source,
            file,
            &word,
            if_contexts,
            resource_estimates,
            line,
            col,
        )
        .map(|h| with_doc_link(h, Doc::Keyword("if")))
    })
    .or_else(|| {
        hover_is_keyword(source, ast, &word, line, col)
            .map(|h| with_doc_link(h, Doc::Keyword("is")))
    })
    .or_else(|| {
        hover_loop_exit_keyword(source, ast, &word, line, col)
            .map(|h| with_doc_link(h, Doc::Keyword(&word)))
    })
    .or_else(|| hover_named_param(source, &word, line, col))
    .or_else(|| hover_event_config_param(source, &word, line, col))
    .or_else(|| hover_data_driven_config(source, &word, line, col))
    .or_else(|| hover_config_enum_value(source, &word, line, col))
    .or_else(|| {
        hover_collection_method(source, symbols, &word, line, col).map(|h| {
            let topic = if h.starts_with("**map.") {
                "mapMethods"
            } else {
                "arrayMethods"
            };
            with_doc_link(h, Doc::Topic(topic))
        })
    })
    .or_else(|| {
        hover_custom_event(source, file, ast, &word, type_map, line, col)
            .map(|h| with_doc_link(h, Doc::Topic("customEvents")))
    })
    .or_else(|| hover_builtin_event(&word).map(|h| with_doc_link(h, event_doc(&word))))
    .or_else(|| {
        hover_builtin_call(source, &word, line, col).map(|h| with_doc_link(h, Doc::Builtin(&word)))
    })
    .or_else(|| {
        hover_chip_or_mod_keyword(source, &word, symbols, resource_estimates, line)
            .map(|h| with_doc_link(h, Doc::Keyword(&word)))
    })
    .or_else(|| {
        hover_on_keyword(source, &word, resource_estimates, line)
            .map(|h| with_doc_link(h, Doc::Keyword("on")))
    })
    .or_else(|| {
        hover_record_or_type_field(
            source,
            file,
            ast,
            type_map,
            symbols,
            doc_comments,
            &word,
            line,
            col,
        )
    })
    .or_else(|| {
        hover_namespace_member(
            source,
            symbols,
            doc_comments,
            resource_estimates,
            &word,
            line,
            col,
        )
    })
    .or_else(|| hover_enum_discriminant_variant_path(source, ast, &word, line, col))
    .or_else(|| hover_enum_variant_path(source, ast, symbols, &word, line, col))
    .or_else(|| hover_enum_field_construction(source, ast, &word, line, col))
    .or_else(|| hover_component_property(source, file, type_map, line, col, &word))
    .or_else(|| {
        resolve_field_hover(
            source,
            file,
            type_map,
            narrowed_reads,
            symbols,
            line,
            col,
            &word,
        )
    })
    .or_else(|| {
        hover_generic_call(
            source,
            file,
            ast,
            symbols,
            doc_comments,
            resource_estimates,
            type_map,
            &word,
            line,
            col,
        )
    })
    .or_else(|| {
        hover_user_symbol(
            source,
            file,
            ast,
            symbols,
            doc_comments,
            var_read_contexts,
            narrowed_reads,
            resource_estimates,
            &word,
            line,
            col,
        )
    })
    .or_else(|| hover_enum_type_name(source, ast, &word))
    .or_else(|| hover_type_or_class(&word))
}

/// Hover for a segment of `Environment.<group>.<property>`: the builtin
/// itself, an environment group, or a property with the type it reads and
/// writes as. Read from the line's text like environment completion, so it
/// works inside `${...}` too. A binding of the program's own named
/// `Environment` shadows the builtin, as in the type checker.
fn hover_environment_path(
    source: &str,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
) -> Option<String> {
    use crate::catalog::properties::{PropertyType, property_type};
    use crate::typecheck::props::{ENVIRONMENT, environment_by_label, environment_names};
    let l = source.lines().nth(line)?;
    let end = word_end_at_byte(l, char_col_to_byte(l, col));
    let start = l[..end]
        .rfind(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.'))
        .map(|i| after_char(l, i))
        .unwrap_or(0);
    let segments: Vec<&str> = l[start..end].split('.').collect();
    if segments.first() != Some(&ENVIRONMENT) || symbols.iter().any(|s| s.name == ENVIRONMENT) {
        return None;
    }
    match segments.as_slice() {
        [_] => {
            let groups: Vec<String> = environment_names()
                .iter()
                .map(|n| format!("`{n}`"))
                .collect();
            let hover = format!(
                "```wirescript\n{ENVIRONMENT}\n```\nThe world's environment settings. Reading \
                 `{ENVIRONMENT}.<group>.<property>` runs a **Get Environment Property** gate and \
                 assigning it a **Set Environment Property** gate, both in an exec context.\n\n\
                 Groups: {}.",
                groups.join(", ")
            );
            Some(with_doc_link(hover, Doc::Topic("environment")))
        }
        [_, group] => {
            let e = environment_by_label(group)?;
            let names: Vec<String> = e
                .properties
                .iter()
                .map(|p| format!("`{}`", p.name))
                .collect();
            let hover = format!(
                "```wirescript\n{ENVIRONMENT}.{group}\n```\n**Environment group**, {} properties: {}.",
                e.properties.len(),
                names.join(", ")
            );
            Some(with_doc_link(hover, Doc::Environment(group)))
        }
        [_, group, property] => {
            let p = environment_by_label(group)?.property(property)?;
            let (ty, note) = match property_type(p) {
                PropertyType::Value(t) => (type_str(&t), String::new()),
                PropertyType::Enum(raw) => {
                    let en = crate::catalog::clean_game_enum_type(&raw);
                    (
                        format!("int ({en})"),
                        format!(
                            " It reads as an `int` and takes a member such as `{en}.<member>`."
                        ),
                    )
                }
                PropertyType::Unsupported(why) => (
                    "unsupported".to_string(),
                    format!(" It is {why}, so a program can't read or write it."),
                ),
            };
            let hover = format!(
                "```wirescript\n{ENVIRONMENT}.{group}.{property}: {ty}\n```\n**Environment setting.** Reading it \
                 runs a **Get Environment Property** gate, assigning it a **Set Environment Property** gate.{note}"
            );
            Some(with_doc_link(hover, Doc::Environment(group)))
        }
        _ => None,
    }
}

/// Where a built-in event's hover links: its own section when it has one
/// (`Clock`), else the section on triggering built-in events.
fn event_doc(word: &str) -> Doc<'_> {
    if doc_url(Doc::Builtin(word)).is_some() {
        Doc::Builtin(word)
    } else if matches!(word, "CustomEvent" | "GlobalCustomEvent") {
        Doc::Topic("customEvents")
    } else {
        Doc::Topic("builtinEvents")
    }
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
        "quat" => "Quaternion rotation (x, y, z, w).",
        "color" => "RGBA color (r, g, b, a).",
        "entity" => "Reference to a game entity.",
        "character" => "Reference to a player character.",
        "controller" => "Reference to a player controller.",
        "exec" => "Execution signal. Triggers gates; carries no value.",
        "zone" => "Reference to a Zone brick. Can be wired, not stored.",
        "teleport" => "Reference to a Teleport Destination. Can be wired, not stored.",
        "prefab" => {
            "Reference to a prefab: a `$./file.brz`, a `$./file.ws`, or an inline prefab block. Resolved at compile time."
        }
        "brick" => "Reference to a brick. Set it in the editor or receive it from a port or event.",
        "any" => "Accepts any type. Prefer a generic `<T>`.",
        "never" => "No value. The type of an expression that never produces one.",
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
        let hover = format!(
            "```wirescript\n{word}  (generic constraint class)\n```\nA bound for a generic type parameter: `<T: {word}>` restricts `T` to one of: {}.",
            names.join(", ")
        );
        return Some(with_doc_link(hover, Doc::Topic("constraintClass")));
    }
    let desc = builtin_type_desc(word)?;
    Some(with_doc_link(
        format!("```wirescript\n{word}\n```\n{desc}"),
        Doc::Type(word),
    ))
}

/// Hover for a `:name` atom literal under the cursor: the compile-time `int`
/// constant it hashes to (xxHash64 of the name), shown in decimal and hex.
fn hover_atom(source: &str, file: &str, line: usize, col: usize) -> Option<String> {
    let a = super::atoms::atom_at(source, file, line, col)?;
    Some(format!(
        "**Atom** `:{name}`\n\nA compile-time **64-bit `int`** constant: the xxHash64 of the \
         name, resolved at compile time (never a runtime string). Every `:{name}` is this value:\n\n\
         - `{dec}`: the signed `int` it is in code (negative when the hash's top bit is set)\n\
         - `{hex:#018x}`: the 64-bit hash (16 hex digits)",
        name = a.name,
        dec = a.value,
        hex = a.value as u64,
    ))
}

/// Whether the identifier under the cursor is an annotation's name: the
/// character right before it is `@`.
fn annotation_word_at(source: &str, line: usize, col: usize) -> bool {
    let Some(l) = source.lines().nth(line) else {
        return false;
    };
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1);
    start.is_some_and(|s| s > 0 && l.as_bytes()[s - 1] == b'@')
}

/// Hover for an annotation by name: what it does and where it goes.
fn hover_annotation(name: &str) -> Option<String> {
    let (sig, text) = match name {
        "left" | "right" | "top" | "bottom" => (
            format!("@{name}"),
            "Puts a rerouter pin for this top-level `in` or `out` on that side of the compiled \
             microchip, so the chip can be wired like a part.",
        ),
        "label" => (
            "@label(text)".to_string(),
            "Overrides the display text on a chip, a `var`, a port or the root microchip (at the top of \
             the file). A constant is baked in; on the module and top-level `var`s a runtime value is \
             wired live.",
        ),
        "color" => (
            "@color(color)".to_string(),
            "Sets the color of a chip's, a `var`'s or the root microchip's brick (at the top of the \
             file). A constant is set on the brick itself. On the module and top-level `var`s a \
             runtime value adds a Brick Property Changer to the brick and is wired into it live.",
        ),
        "material" => (
            "@material(BrickMaterial)".to_string(),
            "Sets the material of a chip's, a `var`'s or the root microchip's brick (at the top of the \
             file): `Plastic`, `Glass`, `TranslucentPlastic`, `Glow`, `Metal` or `Hologram`, bare or as \
             `BrickMaterial.Glow`. A constant is set on the brick itself. On the module and top-level \
             `var`s a runtime value adds a Brick Property Changer to the brick and is wired into it live.",
        ),
        "closed" => (
            "@closed".to_string(),
            "Compiles this chip collapsed instead of open.",
        ),
        "invisible" => (
            "@invisible".to_string(),
            "Hides a brick and turns off its collision: at the top of the file the root microchip, on \
             a port its rerouter pin.",
        ),
        "nofold" => (
            "@nofold".to_string(),
            "Keeps these gates out of constant folding.",
        ),
        "fold" => ("@fold".to_string(), "Constant folding, on by default."),
        "layout" => (
            "@layout(\"code\" | \"cube\")".to_string(),
            "At the top of the file: places gates the way the source reads (`\"code\"`) or packs them \
             into a compact cube (`\"cube\"`).",
        ),
        "flat" => (
            "@flat".to_string(),
            "At the top of the file: inlines every chip onto one grid.",
        ),
        "sliced" => (
            "@sliced".to_string(),
            "Spreads a `for` loop's iterations across ticks.",
        ),
        _ => return None,
    };
    Some(format!(
        "```wirescript\n{sig}\n```\n\n**Annotation.** {text}"
    ))
}

/// Hover for a `$` reference token under the cursor: a prefab file reference
/// (`$./rel.brz`, `$/abs.brz`) or an external asset reference (`$Type/Name`).
/// Scans the raw line for the `$`-prefixed token spanning the cursor, since
/// the `$`, `/`, and `.` chars aren't part of identifier words.
fn hover_asset_ref(source: &str, file: &str, line: usize, col: usize) -> Option<String> {
    let r = super::text::asset_ref_at(source, line, col)?;
    Some(if r.is_file() {
        with_doc_link(
            render_prefab_file_hover(&r.path, file),
            Doc::Topic("prefabs"),
        )
    } else {
        with_doc_link(render_asset_hover(&r.path), Doc::Topic("assets"))
    })
}

/// Markdown hover for a prefab file reference (`$./x.brz` / `$/abs.brz`),
/// resolving the path the same way [`crate::compile::disk_prefab_resolver`]
/// does and (natively) reporting whether the file is present.
fn render_prefab_file_hover(path: &str, file: &str) -> String {
    let resolved = crate::compile::resolve_prefab_path(file, path);

    let kind = if path.ends_with(".ws") {
        "Compiles a `.ws` source file and embeds the result"
    } else {
        "Embeds a `.brz` archive"
    };
    let mut out = format!("**Prefab file reference**\n\n{kind} into `SpawnPrefab`.\n\n");
    let _ = writeln!(out, "- Reference: `${path}`");
    let _ = writeln!(out, "- Resolves to: `{}`", resolved.display());
    if !path.ends_with(".brz") && !path.ends_with(".ws") {
        out += "\nNote: prefab references must end in `.brz` (a prebuilt archive) or `.ws` (a source file) (WS019).\n";
    }
    #[cfg(not(target_arch = "wasm32"))]
    match std::fs::metadata(&resolved) {
        Ok(m) => {
            let _ = writeln!(out, "- On disk: {} bytes", m.len());
        }
        Err(_) => out += "- Not found on disk\n",
    }
    out
}

/// Markdown hover for an external asset reference (`$Type/Name`).
fn render_asset_hover(path: &str) -> String {
    if let Some((super::assets::COMPONENT, name)) = path.split_once('/') {
        return render_component_type_hover(name);
    }
    let mut out = String::from(
        "**Asset reference**\n\nAn external Brickadia asset, inlined into the gate's data.\n\n",
    );
    if let Some((ty, name)) = path.split_once('/') {
        let _ = writeln!(out, "- Type: `{ty}`\n- Name: `{name}`");
    } else {
        let _ = writeln!(out, "- Asset: `{path}`");
    }
    out
}

/// Markdown hover for a `$Component/<name>` or `$Component/<name>/<property>`
/// reference.
fn render_component_type_hover(path: &str) -> String {
    let (name, property) = match path.split_once('/') {
        Some((n, p)) => (n, Some(p)),
        None => (path, None),
    };
    let Some(o) = crate::catalog::component_type(name) else {
        return format!(
            "**Component type** `{name}`\n\nNot a component type the component gates take (WS110)."
        );
    };
    if let Some(property) = property {
        return match crate::catalog::component_properties(o.class)
            .into_iter()
            .find(|p| p.name == property)
        {
            Some(p) => format!(
                "**Component property** `{}.{}`: `{}`\n\nA property of the {} component. The list \
                 comes from the save schema, so it may hold fields the game's own picker does \
                 not offer (unverified).",
                o.name, p.name, p.ty, o.display_name
            ),
            None => format!(
                "**Component property** `{}.{property}`\n\nNot a property of {} (WS110).",
                o.name, o.display_name
            ),
        };
    }
    let mut out = format!("**Component type** `{}`\n\n", o.display_name);
    if !o.description.is_empty() {
        let _ = writeln!(out, "{}\n", o.description);
    }
    let _ = writeln!(out, "- Class: `{}`", o.class);
    out
}

/// Hover inside an `if` block dropped before type checking because its
/// condition is constant (`TypeCheckCtx::dropped_ranges`). Diagnostics never see
/// that branch, so this hover is where its errors show, at every position in it;
/// `hover_at` runs it before `word_at`. `line`/`col` are 0-based against 1-based
/// ranges, half-open as in `scoped_refs::range_contains_cursor`, so the closing
/// `}` is inside.
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
            Some(format!("**Removed at compile time**: {reason}."))
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
    if word != "if" {
        return None;
    }

    let offset = line_offset_at(source, line) + word_start_in_line(source.lines().nth(line)?, col);
    let f: Arc<str> = file.into();
    let &is_exec = if_contexts.get(&(f, offset))?;

    let mut hover = if is_exec {
        "```wirescript\nif (exec) -> Branch gate\n```\nExec-context conditional. Produces an **Exec_Branch** gate that routes the exec chain to the true or false arm.".to_string()
    } else {
        "```wirescript\nif (pure) -> Select gate\n```\nPure-context conditional. Produces a **Select** gate that picks one of two values based on the condition.".to_string()
    };
    if let Some(est) = resource_estimates.get(&format!("@{offset}")) {
        let _ = write!(hover, "\n\n{}", format_estimate(est, EstimateKind::Scope));
    }
    Some(hover)
}

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

/// The default value brdb registers for one field of a gate's data struct:
/// the gate class resolves to its struct through `COMPONENT_TYPE_STRUCT_PAIRS`,
/// then the field is read out of `STRUCT_DEFAULTS`, the table the emitter bakes
/// from. `None` when the gate has no data struct, or that struct has no such field.
fn struct_field_default(gate_class: &str, field: &str) -> Option<&'static dyn brdb::AsBrdbValue> {
    let strct = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(c, _)| *c == gate_class)
        .map(|(_, s)| *s)?;
    brdb::component_db::STRUCT_DEFAULTS
        .iter()
        .find(|(s, _)| *s == strct)
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| *n == field))
        .map(|(_, v)| v.as_ref())
}

/// A gate data-struct field's registered default value, rendered for display.
/// An enum field shows its member name (not the stored index); otherwise
/// [`struct_field_default`]'s value is read in its declared scalar `kind`
/// (`bool`/`int`/`float`/`string`). `None` when the gate registers no such
/// default, or the kind is non-scalar.
fn gate_field_default(gate_class: &str, field: &str, kind: &str) -> Option<String> {
    let value = struct_field_default(gate_class, field)?;
    // Enum-typed field: the default is an index; show the member name.
    if let Some(et) = config_field_enum_type(gate_class, field)
        && let Ok(idx) = value.as_brdb_u8()
    {
        let names = enum_member_names(et);
        return Some(
            names
                .into_iter()
                .nth(idx as usize)
                .unwrap_or_else(|| idx.to_string()),
        );
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

/// A gate data-struct field's registered composite default (vector, color,
/// rotator, quat) as display text: the struct default read as
/// [`gate_field_default`] reads scalars, then its named sub-fields. Colors are
/// stored linear and shown as sRGB hex (`#181425`); vectors and rotators as
/// `Vec(...)`/`Rotation(...)`. `None` when there is no such default or `ty` is
/// not a composite the emitter bakes.
fn composite_field_default(gate_class: &str, field: &str, ty: &Type) -> Option<String> {
    let value = struct_field_default(gate_class, field)?;
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

/// A gate field's default rendered for hover: a `Vector2D` sub-port axis
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
fn vector2d_subport_default(gate_class: &str, parent: &str, axis: &str) -> Option<String> {
    let value = struct_field_default(gate_class, parent)?;
    let schema = brdb::schemas::bricks_components_schema_max();
    let id = schema.intern.get(axis)?;
    let f = value
        .as_brdb_struct_prop_value(schema, id, id)
        .ok()?
        .as_brdb_f64()
        .ok()?;
    Some(fnum(f))
}

/// Render a float without a trailing `.0`, matching the scalar-default style:
/// `1.0 -> "1"`, `0.5 -> "0.5"`, `-1.0 -> "-1"`.
fn fnum(f: f64) -> String {
    f.to_string()
}

/// Linear-light 0 to 1 component -> sRGB byte (the standard piecewise encode).
fn linear_to_srgb_u8(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Render a stored LINEAR RGBA default as sRGB hex (`#rrggbb`), the form the
/// color appears as in-editor; a non-opaque alpha is noted after it. Alpha is
/// stored linearly (not gamma-encoded), so it scales straight to a byte.
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

/// Named parameter inside a builtin call (e.g. `delay` in `Sleep(_, delay = 1.0)`).
/// Only fires in arg-name position (the word followed by a single `=`), so a
/// value that shares a param's name (`delay = delay`) hovers as its symbol.
fn hover_named_param(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_named_arg_name(source, line, col) {
        return None;
    }
    let call_name = find_enclosing_call(source, line, col)?;
    let spec = calls().get(call_name.as_str())?;
    let p = spec.params.iter().find(|p| p.name == word)?;

    let port_doc = input_port_doc(spec.gate_class, p.port.as_str());
    let display = port_doc
        .map(|(d, _)| d)
        .filter(|d| !d.is_empty())
        .unwrap_or(p.name);
    let tooltip = port_doc.map(|(_, t)| t).unwrap_or("");

    // A param surfaced as a plain int but backed by a schema enum shows the
    // enum's name (and its members below) instead of `int`, wireable or not.
    let config_enum = config_field_enum_type(spec.gate_class, p.port.as_str());
    let ty_label = config_enum
        .map(str::to_string)
        .unwrap_or_else(|| type_str(&p.ty));
    let mut v = format!("**{}** `{}: {}`", display, p.name, ty_label);
    if p.optional {
        v += " *(optional)*";
    }
    if !tooltip.is_empty() {
        let _ = write!(v, "\n\n{tooltip}");
    }
    if let Some(et) = config_enum {
        let members = enum_member_names(et).join(", ");
        if !members.is_empty() {
            let _ = write!(v, "\n\none of: {members}");
        }
    }
    // The gate's registered default for this field, if any: a scalar or a
    // composite (an enum parameter shows its member name through
    // `gate_field_default`).
    if let Some(d) = field_default_display(spec.gate_class, p.port.as_str(), &p.ty) {
        let _ = write!(v, "\n\nDefault: `{d}`");
    }
    Some(v)
}

/// Whether the hovered word is a named argument's name: followed, after spaces,
/// by a single `=` (not `==`). A value identifier is never followed by a bare
/// `=`, so this separates the two sides of `delay = delay`.
fn word_is_named_arg_name(source: &str, line: usize, col: usize) -> bool {
    let Some(l) = source.lines().nth(line) else {
        return false;
    };
    let rest = l[word_end_at_byte(l, char_col_to_byte(l, col))..].trim_start();
    rest.starts_with('=') && !rest.starts_with("==")
}

/// The `obj` of the `obj.member` access the cursor sits on. `None` unless the
/// word under the cursor is immediately preceded by a `.`; an empty string when
/// the dot itself is (`.foo`), which every caller rejects on its own terms.
///
/// Textual, not AST-driven, because hover runs on the buffer as typed, which is
/// routinely mid-edit and unparseable.
fn receiver_at(source: &str, line: usize, col: usize) -> Option<&str> {
    let l = source.lines().nth(line)?;
    let start = word_start_in_line(l, col);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let obj_end = start - 1;
    Some(&l[word_start_at_byte(l, obj_end)..obj_end])
}

/// Collection methods (`arr.push`, `m.get`). Fires only on a `.method` access,
/// so a user symbol sharing a method's name (`var sum = 0`) hovers as itself.
///
/// The table follows the receiver's type: a map's `length`, `remove`, `clear`
/// and `copyFrom` differ from the array methods of the same names, and `get`,
/// `set`, `has`, `keys` and `values` exist on maps only. When the receiver's type
/// is unknown (an imported var whose span `type_map` has no entry for), the
/// array table by name is the fallback.
fn hover_collection_method(
    source: &str,
    symbols: &[SymbolDef],
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // Dispatch on the receiver's declared type. `type_map` has no entry for a
    // method call's receiver, only for the whole call at the same start offset, so
    // a span lookup would return the call's type; the symbol table keys by name,
    // which is the receiver here, top-level or handler-local.
    let obj_name = receiver_at(source, line, col)?;

    // The receiver's declared type string (`Map<string, int>`, `Grid<int>`)
    // drives dispatch and is what the map hover shows.
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
    Some(format!(
        "**array.{}**\n\n{}{} - {}",
        m.name,
        m.name,
        m.shown_signature(),
        m.doc
    ))
}

/// Render the map-method hover for `word` on a receiver whose type displays as
/// `map_display` (e.g. `Map<string, int>`), or `None` if `word` isn't a map
/// method. The concrete key/value types are surfaced so the hover reflects the
/// receiver, not a generic `Map<K, V>`.
fn hover_map_method(word: &str, map_display: &str) -> Option<String> {
    let m = crate::catalog::maps::map_method(word)?;
    Some(format!(
        "**map.{}**\n\n{}{} - {}\n\n*{}*",
        m.name,
        m.name,
        m.shown_signature(),
        m.doc,
        map_display,
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
    let mut out = format!(
        "```wirescript\non {}{}{}\n```",
        evt.surface_name, call_sig, arrow
    );
    if !evt.input_named.is_empty() {
        let wired: Vec<&str> = evt.input_named.iter().map(|(s, _, _)| *s).collect();
        let _ = write!(out, "\n\n**Wired input:** {}", wired.join(", "));
    }
    if !evt.config_named.is_empty() {
        let cfg: Vec<&str> = evt.config_named.iter().map(|(s, _)| *s).collect();
        let _ = write!(out, "\n\n**Config:** {} *(constant-only)*", cfg.join(", "));
    }
    Some(out)
}

/// Hover for the custom-event channel words: the trigger (`on CustomEvent`,
/// `on GlobalCustomEvent`) and the send call (`SendCustomEvent`,
/// `SendGlobalCustomEvent`, also as `e.SendCustomEvent(...)`). Resolves the
/// channel's data slots from every receiver and matching sender in the file and
/// renders the typed signature, such as `on CustomEvent("init") -> (p: character)`.
/// `None` when the word is not one of these with a resolvable channel.
fn hover_custom_event(
    source: &str,
    file: &str,
    ast: &Script,
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

    // `ast` has the same byte offsets as `type_map`'s keys, so each sender
    // argument's inferred type resolves.
    let script = ast;

    let channel = if is_send {
        ce_send_channel_at(script, word, word_off)?
    } else {
        ce_trigger_channel_at(script, ns_word, word_off)?
    };
    let slots = resolve_ce_channel_slots(script, ns_word, &channel, type_map, file);

    let data_parts: Vec<String> = slots
        .iter()
        .map(|(name, ty)| format!("{name}: {ty}"))
        .collect();
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

/// The literal channel name of the `send_name` call (`SendCustomEvent` or
/// `SendGlobalCustomEvent`, plain or as a method) whose callee contains byte
/// offset `off`.
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
        visit_program(script, &mut on_handler, &mut on_call);
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
        visit_program(script, &mut on_handler, &mut on_call);
    }
    channel
}

/// The channel a CE receiver handler listens on: its `config`'s named
/// `eventName = "x"` if present, else its first positional string literal.
fn ce_handler_channel(h: &Handler) -> Option<String> {
    for c in &h.config {
        if let HandlerConfigArg::Named {
            name,
            value: Expr::StringLit { value, .. },
        } = c
            && name.eq_ignore_ascii_case("eventname")
        {
            return Some(value.clone());
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
    let file_arc: Arc<str> = file.into();

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
                if decl_types[i].is_none()
                    && let Some(te) = &p.ty
                {
                    decl_types[i] = Some(crate::analysis::types::type_expr_str(te));
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
                        type_map.get(&(Arc::clone(&file_arc), r.start.offset, r.end.offset))
                        && !matches!(t, Type::Any | Type::Opaque)
                    {
                        send_types[slot] = Some(type_str(t));
                    }
                }
            }
        };
        visit_program(script, &mut on_handler, &mut on_call);
    }

    let n = names.len().max(decl_types.len()).max(send_types.len());
    (0..n)
        .map(|i| {
            let name = names
                .get(i)
                .and_then(std::clone::Clone::clone)
                .unwrap_or_else(|| format!("data{}", i + 1));
            let ty = decl_types
                .get(i)
                .and_then(std::clone::Clone::clone)
                .or_else(|| send_types.get(i).and_then(std::clone::Clone::clone))
                .unwrap_or_else(|| "any".to_string());
            (name, ty)
        })
        .collect()
}

/// The channel name a `SendCustomEvent`-family call targets: named `eventName`
/// if present, else the first positional string literal.
fn ce_send_channel(args: &[CallArg]) -> Option<String> {
    for a in args {
        if let CallArg::Named {
            name,
            value: Expr::StringLit { value, .. },
            ..
        } = a
            && name.eq_ignore_ascii_case("eventname")
        {
            return Some(value.clone());
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
    let has_named_channel = args.iter().any(
        |a| matches!(a, CallArg::Named { name, .. } if name.eq_ignore_ascii_case("eventname")),
    );
    let mut out = Vec::new();
    let mut pos_idx = 0usize;
    for a in args {
        match a {
            CallArg::Positional(e) => {
                let is_channel = !has_named_channel && pos_idx == 0;
                if !is_channel {
                    let slot = if has_named_channel {
                        pos_idx
                    } else {
                        pos_idx - 1
                    };
                    out.push((slot, e));
                }
                pos_idx += 1;
            }
            CallArg::Named { name, value, .. } => {
                if let Some(n) = name
                    .strip_prefix("data")
                    .and_then(|s| s.parse::<usize>().ok())
                    && n >= 1
                {
                    out.push((n - 1, value));
                }
            }
            CallArg::Spread(_) => {}
        }
    }
    out
}

/// Hover for an event handler's config argument name (`enabled` in
/// `on Clock(enabled = true)`), the event counterpart of the call-parameter
/// hover. Fires only in named-argument position, for a known event whose config
/// or input arguments include `word`.
fn hover_event_config_param(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_named_arg_name(source, line, col) {
        return None;
    }
    let trigger = find_enclosing_call(source, line, col)?;
    let evt = find_event(&trigger)?;
    if let Some((_, field)) = evt
        .config_named
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(word))
    {
        let enum_ty = config_field_enum_type(evt.gate_class, field);
        let ty_label = enum_ty.unwrap_or("config value");
        let mut v = format!(
            "**{}** `{}: {}` *(event config, constant-only)*\n\nSets `{}` on the `{}` gate.",
            word, word, ty_label, field, evt.surface_name
        );
        if let Some(et) = enum_ty {
            let members = enum_member_names(et).join(", ");
            if !members.is_empty() {
                let _ = write!(v, "\n\none of: {members}");
            }
        }
        return Some(v);
    }
    if evt
        .input_named
        .iter()
        .any(|(s, _, _)| s.eq_ignore_ascii_case(word))
    {
        return Some(format!(
            "**{}** *(wired input on the `{}` event)*",
            word, evt.surface_name
        ));
    }
    None
}

/// Hover for a data-driven config attribute name: a raw settings-menu field
/// (`bOnlyHitPlayerBodyParts`, `FontSize`, `Function`) set by its inventory
/// name. Fires only in named-argument position, for a scalar config field of the
/// enclosing gate.
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
    let enum_ty = config_field_enum_type(spec.gate_class, word);
    let ty_label = enum_ty.unwrap_or(cfg.ty.as_str());
    let mut v = format!("**{word}** `{word}: {ty_label}` *(gate config, constant-only)*");
    if !cfg.display_name.is_empty() {
        let _ = write!(v, "\n\n{}", cfg.display_name);
    }
    if let Some(et) = enum_ty {
        let members = enum_member_names(et).join(", ");
        if !members.is_empty() {
            let _ = write!(v, "\n\none of: {members}");
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
    let members = enum_member_names(et).join(", ");
    Some(format!("**{word}**: `{et}` member\n\none of: {members}"))
}

// Language-level `enum` hover (user and built-in game enums)
//
// These hover an `enum` type ([`crate::typecheck::enums::EnumDef`]), unlike
// `hover_config_enum_value`, which hovers a brdb schema enum member written as
// a gate config value (`direction = X_Negative`). A user enum is looked up in
// the registry [`crate::typecheck::enums::build_registry`] builds from the
// parsed `ast`; a built-in game enum (`EasingFunction`) comes from the catalog.

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
            let parts: Vec<String> = types.iter().map(type_expr_str).collect();
            format!("({})", parts.join(", "))
        }
        Payload::Named(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{n}: {}", type_expr_str(t)))
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
fn hover_enum_type_name(source: &str, ast: &Script, word: &str) -> Option<String> {
    // A built-in game enum comes from the catalog's memoized table, checked first
    // since it applies to every file.
    if let Some(v) = hover_builtin_game_enum_type(word) {
        return Some(v);
    }
    // The prelude (`Option`, `Result`).
    if let Some(def) = crate::typecheck::enums::prelude_enum_defs()
        .into_iter()
        .find(|d| d.name == word)
    {
        return Some(render_user_or_prelude_enum_hover(&def, false));
    }
    // A user `enum`, only when the file could declare one.
    if !source.contains("enum ") {
        return None;
    }
    let e = find_top_level_enum_decl(&ast.decls, word)?;
    let def =
        crate::typecheck::enums::EnumDef::from_decl(e, crate::typecheck::enums::variant_defs(e));
    Some(render_user_or_prelude_enum_hover(&def, true))
}

/// Find a top-level `TopDecl::Enum` named `name`, recursing into namespaces.
fn find_top_level_enum_decl<'a>(
    decls: &'a [crate::ast::TopDecl],
    name: &str,
) -> Option<&'a crate::ast::EnumDecl> {
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
fn render_user_or_prelude_enum_hover(
    def: &crate::typecheck::enums::EnumDef,
    is_user: bool,
) -> String {
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
    let _ = write!(out, "\n\nVariants: {}", variants.join(", "));
    if !is_user {
        out += "\n\n*Built-in prelude enum.*";
    }
    out
}

/// Hover for a built-in game enum type name (`EasingFunction`, `Direction`),
/// from the catalog's schema-derived table
/// ([`crate::catalog::game_enum_schema_type`]): each cleaned variant name with
/// its schema discriminant ([`crate::catalog::enum_member_value`]).
fn hover_builtin_game_enum_type(word: &str) -> Option<String> {
    let schema_type = crate::catalog::game_enum_schema_type(word)?;
    let variants: Vec<String> = enum_member_names(schema_type)
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

/// Hover for a variant in a construction path (`Shape.Circle`,
/// `EasingFunction.Bounce`), on the variant token: its enum, payload shape and
/// discriminant. The registry is `build_registry` over `ast` (user enums, the
/// prelude and the game's enums), the table the compiler resolves
/// `Enum.Variant` against, so built-in and user enums hover alike.
///
/// Fires only when the identifier before the `.` is not a value symbol, as in
/// `resolve_variant_for_construction`: a value named like an enum wins.
fn hover_enum_variant_path(
    source: &str,
    ast: &Script,
    symbols: &[SymbolDef],
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    let enum_name = receiver_at(source, line, col)?;
    if enum_name.is_empty()
        || super::resolve_symbol(symbols, source, enum_name, line, col).is_some()
    {
        return None;
    }

    let registry = build_registry(&ast.decls);
    let def = registry.expression_path(enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == word)?;
    let is_user = find_top_level_enum_decl(&ast.decls, enum_name).is_some();

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

/// Hover for a named payload field's key in a variant construction
/// (`Shape.Box { w: 1.0, h: 2.0 }`, on `w` or `h`): its type from the enum
/// declaration. A `RecordLitField::Named` range spans the whole `key: value`, so
/// the key span comes from the field name's byte length, as in
/// `definition::resolve_enum_field_construction_definition`.
fn hover_enum_field_construction(
    source: &str,
    ast: &Script,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    let line_str = source.lines().nth(line)?;
    let word_off = line_offset_at(source, line) + word_start_in_line(line_str, col);

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
                    RecordLitField::Named { name, range, .. } => (
                        name.as_str(),
                        range.start.offset,
                        range.start.offset + name.len(),
                    ),
                    RecordLitField::Shorthand { name, range } => {
                        (name.as_str(), range.start.offset, range.end.offset)
                    }
                    RecordLitField::Spread { .. } => continue,
                };
                if name != word || word_off < key_start || word_off > key_end {
                    continue;
                }
                let Expr::FieldAccess {
                    obj,
                    field: variant,
                    ..
                } = path.as_ref()
                else {
                    continue;
                };
                let Expr::Ident {
                    name: enum_name, ..
                } = obj.as_ref()
                else {
                    continue;
                };
                hit = Some((enum_name.clone(), variant.clone()));
            }
        };
        visit_program(ast, &mut on_handler, &mut on_call);
    }
    let (enum_name, variant_name) = hit?;

    let registry = build_registry(&ast.decls);
    let def = registry.expression_path(&enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == variant_name)?;
    let crate::typecheck::enums::Payload::Named(field_defs) = &vdef.payload else {
        return None;
    };
    let (_, ty) = field_defs.iter().find(|(n, _)| n == word)?;
    Some(format!(
        "```wirescript\n{word}: {}\n```\n\nNamed payload field of `{enum_name}.{variant_name}`.",
        type_expr_str(ty)
    ))
}

/// Hover for `.Discriminant` on a variant path (`Shape.Circle.Discriminant`):
/// the variant is named, so its discriminant is a constant and the hover shows
/// the integer. A value's `.Discriminant` (`s.Discriminant`) is a two-part chain
/// and goes to [`resolve_field_hover`], which shows `field Discriminant: int`.
fn hover_enum_discriminant_variant_path(
    source: &str,
    ast: &Script,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    if word != DISCRIMINANT_FIELD {
        return None;
    }
    let l = source.lines().nth(line)?;
    let start = word_start_in_line(l, col);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let variant_end = start - 1;
    let variant_start = word_start_at_byte(l, variant_end);
    if variant_start == 0 || l.as_bytes()[variant_start - 1] != b'.' {
        return None; // a two-identifier chain (`s.Discriminant`) - not a variant path.
    }
    let variant_name = &l[variant_start..variant_end];
    let enum_end = variant_start - 1;
    let enum_start = word_start_at_byte(l, enum_end);
    let enum_name = &l[enum_start..enum_end];
    if enum_name.is_empty() {
        return None;
    }

    let registry = build_registry(&ast.decls);
    let def = registry.expression_path(enum_name)?;
    let vdef = def.variants.iter().find(|v| v.name == variant_name)?;
    Some(format!(
        "```wirescript\n{enum_name}.{variant_name}.Discriminant: int = {}\n```\n\n\
         Compile-time constant - the discriminant of `{enum_name}.{variant_name}`.",
        vdef.discriminant
    ))
}

/// Whether the hovered word is a call or method access: preceded by `.` or
/// followed by `(`. Call hovers fire only there, so a plain identifier sharing a
/// builtin's name (`var Teleport = 0`) hovers as itself.
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
    let word_end = word_end_at_byte(l, char_col_to_byte(l, col));
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

/// The game's own entry for `gate_class` in the gate inventory: its display
/// name, what it does, and its ports' names and tooltips. The inventory is
/// regenerated each game build, so it is the hover's first source;
/// `gate_docs.json` is older and covers only some gates.
fn gate_inventory(gate_class: &str) -> Option<&'static crate::catalog::ComponentSpec> {
    crate::catalog::default_catalog()
        .find_by_class(gate_class)
        .map(|g| &g.component)
}

/// The display name and tooltip of input `port` on `gate_class`, from the
/// inventory, else from `gate_docs.json`.
fn input_port_doc(gate_class: &str, port: &str) -> Option<(&'static str, &'static str)> {
    let from_inventory = gate_inventory(gate_class)
        .and_then(|c| c.inputs.iter().find(|p| p.name == port))
        .filter(|p| !p.display_name.is_empty() || !p.tooltip.is_empty())
        .map(|p| (p.display_name.as_str(), p.tooltip.as_str()));
    from_inventory.or_else(|| {
        gate_docs()
            .get(gate_class)
            .and_then(|g| g.inputs.get(port))
            .map(|pd| (pd.display_name.as_str(), pd.tooltip.as_str()))
    })
}

fn hover_builtin_call(source: &str, word: &str, line: usize, col: usize) -> Option<String> {
    if !word_is_call_or_method(source, line, col) {
        return None;
    }
    let spec = calls().get(word)?;
    let gdocs = gate_docs();
    let gate_doc = gdocs.get(spec.gate_class);
    let inventory = gate_inventory(spec.gate_class);
    let override_doc = call_doc_override(spec.name);
    let title = override_doc
        .map(|(t, _)| t)
        .or_else(|| {
            inventory
                .map(|c| c.display_name.as_str())
                .filter(|n| !n.is_empty())
        })
        .or_else(|| gate_doc.map(|g| g.display_name.as_str()))
        .unwrap_or(spec.name);

    let mut params: Vec<String> = Vec::new();
    if spec.exec {
        params.push("exec".into());
    }
    params.extend(spec.params.iter().map(|p| {
        if p.optional {
            format!("{}?: {}", p.name, type_str(&p.ty))
        } else {
            format!("{}: {}", p.name, type_str(&p.ty))
        }
    }));

    let out = match spec.outputs.len() {
        0 => String::new(),
        1 => format!(" -> {}", type_str(&spec.outputs[0].ty)),
        _ => format!(
            " -> ({})",
            spec.outputs
                .iter()
                .map(|o| format!("{}: {}", o.port.as_str(), type_str(&o.ty)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    let mut parts = vec![format!(
        "### {}\n```wirescript\n{}({}){}\n```",
        title,
        spec.name,
        params.join(", "),
        out
    )];
    // The game's SearchTags keywords for this gate, from the inventory dump, so
    // hover also says what to search for.
    let tags_line = crate::catalog::default_catalog()
        .find_by_class(spec.gate_class)
        .map(|g| g.component.search_tags.trim())
        .filter(|t| !t.is_empty())
        .map(|t| {
            format!(
                "*Keywords: {}*",
                t.split_whitespace().collect::<Vec<_>>().join(", ")
            )
        });
    if let Some((_, doc)) = override_doc {
        parts.push(doc.to_string());
        if let Some(t) = tags_line {
            parts.push(t);
        }
        return Some(parts.join("\n\n"));
    }
    let description = inventory
        .map(|c| c.description.as_str())
        .filter(|d| !d.is_empty())
        .or_else(|| {
            gate_doc
                .map(|g| g.description.as_str())
                .filter(|d| !d.is_empty())
        });
    if let Some(d) = description {
        parts.push(d.to_string());
    }
    let param_docs: Vec<String> = spec
        .params
        .iter()
        .filter_map(|p| input_port_doc(spec.gate_class, p.port.as_str()))
        .filter(|(_, tooltip)| !tooltip.is_empty())
        .map(|(display, tooltip)| format!("- **{display}** - {tooltip}"))
        .collect();
    if !param_docs.is_empty() {
        parts.push(format!("**Parameters:**\n{}", param_docs.join("\n")));
    }
    if let Some(table) = defaults_table(spec) {
        parts.push(table);
    }
    if let Some(t) = tags_line {
        parts.push(t);
    }
    Some(parts.join("\n\n"))
}

/// A markdown table of a gate's parameter and config defaults (from
/// `gate_field_default`), or `None` when it registers none. Covers the named
/// parameters and the settings-menu config fields that are not parameters.
fn defaults_table(spec: &crate::catalog::calls::CallSpec) -> Option<String> {
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for p in &spec.params {
        if let Some(d) = field_default_display(spec.gate_class, p.port.as_str(), &p.ty) {
            // An enum-backed config param shows its enum type, not bare `int`
            // (matching the value, which is rendered as the member name).
            let ty_label = config_field_enum_type(spec.gate_class, p.port.as_str())
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
            let ty_label = config_field_enum_type(spec.gate_class, &cfg.name)
                .map(str::to_string)
                .unwrap_or_else(|| cfg.ty.clone());
            rows.push((cfg.name.clone(), ty_label, d));
        }
    }
    if rows.is_empty() {
        return None;
    }
    let mut t =
        String::from("**Defaults:**\n\n| Parameter | Type | Default |\n| --- | --- | --- |");
    for (n, ty, d) in &rows {
        let _ = write!(t, "\n| {n} | {ty} | {d} |");
    }
    Some(t)
}

/// `chip`, `mod` or `fn` keyword: show exec/pure context and resource estimate.
fn hover_chip_or_mod_keyword(
    source: &str,
    word: &str,
    symbols: &[SymbolDef],
    resource_estimates: &HashMap<String, ResourceEstimate>,
    line: usize,
) -> Option<String> {
    if !matches!(word, "chip" | "mod" | "fn") {
        return None;
    }

    let lo = line_offset_at(source, line);
    let line_end = lo + line_text(source, line).len() + 1;

    for sym in symbols {
        if matches!(sym.kind, "chip" | "mod" | "fn")
            && sym.range.start.offset >= lo
            && sym.range.start.offset < line_end
        {
            let context = if sym.exec { "exec" } else { "pure" };
            let name = if sym.name.is_empty() || sym.name.starts_with('_') {
                "(anonymous)"
            } else {
                &sym.name
            };
            let const_kw = if sym.is_const { "const " } else { "" };
            let mut hover = format!(
                "```wirescript\n{const_kw}{} {} ({})\n```\n\n{} context - {}",
                sym.kind,
                name,
                context,
                if sym.exec { "Exec" } else { "Pure" },
                if sym.exec {
                    "body runs as sequential exec chain"
                } else {
                    "body is evaluated as signal-flow (combinational)"
                },
            );
            if let Some(est) =
                lookup_estimate(resource_estimates, &sym.name, sym.range.start.offset)
            {
                let ek = estimate_kind(sym.kind);
                let _ = write!(hover, "\n\n{}", format_estimate(est, ek));
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
    if word != "on" {
        return None;
    }

    let l = source.lines().nth(line)?;
    let offset = line_offset_at(source, line) + l.find("on").unwrap_or(0);
    let est = resource_estimates.get(&format!("@{offset}"))?;

    let mut hover = "```wirescript\non handler (exec)\n```".to_string();
    let _ = write!(hover, "\n\n{}", format_estimate(est, EstimateKind::Scope));
    Some(hover)
}

/// Record literal field or type declaration field.
/// Checked before general symbol lookup so `counter` in `{ counter: score }`
/// shows as a field, not as a param.
fn hover_record_or_type_field(
    source: &str,
    file: &str,
    ast: &Script,
    type_map: &TypeMap,
    symbols: &[SymbolDef],
    doc_comments: &DocComments,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // Record literal field, resolved through the type map. Tried before the
    // line-scanning resolver below, which only recognizes one written shape.
    if let Some(v) = resolve_record_lit_field_typed(source, file, ast, type_map, word, line, col) {
        return Some(v);
    }

    // Record literal field (e.g. `{ counter: score }`)
    if let Some(v) = resolve_record_lit_field(source, symbols, word, line) {
        return Some(v);
    }

    // Type declaration field: check if cursor is inside a type definition's range
    for sym in symbols {
        if sym.kind == "type"
            && sym.range.start.line.saturating_sub(1) as usize <= line
            && sym.range.end.line.saturating_sub(1) as usize >= line
            && let Some(ref ty_str) = sym.ty
            && let Some(field_type) = extract_record_field_type(ty_str, word)
        {
            let mut hover = format!("```wirescript\n{}.{}: {}\n```", sym.name, word, field_type);
            // Field `///` doc comment, stored by the parser keyed by the
            // field name's offset.
            let field_off =
                line_offset_at(source, line) + word_start_in_line(source.lines().nth(line)?, col);
            if let Some(doc) = doc_comments.get(&(Arc::clone(&sym.range.file), field_off)) {
                let _ = write!(hover, "\n\n{doc}");
            }
            return Some(hover);
        }
    }
    None
}

/// Hover for a call of a generic `mod` or `chip`, with the type arguments
/// resolved for this call: `mod assert<int>(want: int, got: int, label: string)`
/// rather than the declaration's `mod assert<T: int | float | string>(...)`.
///
/// Finds the `Expr::Call` in `ast` whose callee spans the cursor, its generic
/// declaration, and binds each type parameter through the call-site inference
/// ([`crate::types::mono::infer_call_subst`]) or explicit type arguments. `None`
/// when the word is not a generic call's callee, so declarations and other calls
/// fall through to [`hover_user_symbol`].
fn hover_generic_call(
    source: &str,
    file: &str,
    ast: &Script,
    symbols: &[SymbolDef],
    doc_comments: &DocComments,
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

    let script = ast;

    // The `Expr::Call` whose callee identifier `word` spans the cursor.
    let (args, type_args) = generic_call_at(script, word, word_off)?;

    // The callee's declaration must be a generic `mod`/`chip` (only those carry
    // `type_params`; `fn`s and builtins never do).
    let decl = find_generic_chip_decl(&script.decls, word)?;
    if decl.type_params.is_empty() {
        return None;
    }

    // Resolve the declared parameter / output types with the decl's own type
    // params in scope, so a `T` becomes `Type::Param("T")`, and every alias
    // read where the declaration's file declares it.
    let param_names: Vec<String> = decl.type_params.iter().map(|tp| tp.name.clone()).collect();
    let enum_defs = Arc::new(build_registry(&script.decls));
    let types = DeclaredTypes::for_program_in(&script.decls, &enum_defs, &script.imports);
    let types = types.in_file(&decl.range.file);
    let own_param = |name: &str, _: &crate::diagnostic::SourceRange| {
        param_names
            .iter()
            .any(|p| p == name)
            .then(|| Type::Param(name.to_string()))
    };
    let resolve = |te: &TypeExpr| types.resolve_scoped(te, &own_param);
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
        let file_arc: Arc<str> = file.into();
        let arg_types: Vec<Type> = positional_arg_types(&args, type_map, &file_arc);
        let params: Vec<(String, Vec<Type>)> = decl
            .type_params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    crate::types::mono::mask_for_param(tp.bound.as_ref(), &|te| types.resolve(te)),
                )
            })
            .collect();
        crate::types::mono::infer_call_subst(&param_types, &arg_types, &params)
    };

    // Nothing resolved (e.g. a `T` only in the return type, called without
    // explicit args): leave the declaration hover to show the generic form.
    if decl
        .type_params
        .iter()
        .all(|tp| !subst.contains_key(&tp.name))
    {
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
            let ty = type_str(&substitute(pt, &subst));
            if p.is_const {
                format!("{}: const {}", p.name, ty)
            } else {
                format!("{}: {}", p.name, ty)
            }
        })
        .collect();
    let ret_suffix = match decl.outputs.as_slice() {
        [] => String::new(),
        [single] => format!(
            " -> {}",
            type_str(&substitute(&resolve(&single.typ), &subst))
        ),
        multiple => {
            let fields: Vec<String> = multiple
                .iter()
                .map(|o| {
                    format!(
                        "{}: {}",
                        o.name,
                        type_str(&substitute(&resolve(&o.typ), &subst))
                    )
                })
                .collect();
            format!(" -> ({})", fields.join(", "))
        }
    };

    // The declaration symbol carries the kind/exec/const flags and the offsets
    // the doc-comment and estimate lookups key on; only its `ty` changes.
    let sym = super::resolve_symbol(symbols, source, word, line, col)?;
    if !matches!(sym.kind, "mod" | "chip" | "fn") {
        return None;
    }
    let synth = SymbolDef {
        ty: Some(format!(
            "{}({}){}",
            header,
            params_str.join(", "),
            ret_suffix
        )),
        ..sym.clone()
    };
    let mut out = render_decl_hover(
        &synth,
        doc_comments,
        resource_estimates,
        Some((symbols, ast)),
    );

    let bindings: Vec<String> = decl
        .type_params
        .iter()
        .filter_map(|tp| {
            subst
                .get(&tp.name)
                .map(|t| format!("`{}` = `{}`", tp.name, type_str(t)))
        })
        .collect();
    if !bindings.is_empty() {
        let _ = write!(
            out,
            "\n\n*Generic call: {} for this call.*",
            bindings.join(", ")
        );
    }
    Some(out)
}

/// The `(args, type_args)` of the `Expr::Call` whose callee is the identifier
/// `word` and whose callee range contains byte offset `off`. Walks the whole
/// program (handler/mod/chip bodies included) via the shared visitor.
fn generic_call_at(
    script: &Script,
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
            let Expr::Call {
                callee,
                args,
                type_args,
                ..
            } = call
            else {
                return;
            };
            let Expr::Ident { name, range } = callee.as_ref() else {
                return;
            };
            if name == word && range.start.offset <= off && off < range.end.offset {
                found = Some((args.clone(), type_args.clone()));
            }
        };
        visit_program(script, &mut on_handler, &mut on_call);
    }
    found
}

/// The generic `mod` or `chip` named `name`, top-level or in a namespace; the
/// first match is enough for its parameter shape and bounds.
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
fn positional_arg_types(args: &[CallArg], type_map: &TypeMap, file: &Arc<str>) -> Vec<Type> {
    let mut out = Vec::new();
    for a in args {
        let CallArg::Positional(e) = a else { break };
        let r = e.range();
        let ty = type_map
            .get(&(Arc::clone(file), r.start.offset, r.end.offset))
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
    ast: &Script,
    symbols: &[SymbolDef],
    doc_comments: &DocComments,
    var_read_contexts: &VarReadContextMap,
    narrowed_reads: &NarrowedReadMap,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // The declaration in scope at the cursor, so a name reused across scopes
    // shows the visible one (`players` in `var players: character[]` is that array,
    // not a file-scope `players: string`).
    let sym = super::resolve_symbol(symbols, source, word, line, col)?;

    // A namespace alias (`import * as card`) has no type: show it as a namespace
    // with the members it brings (its `card.*` symbols).
    if sym.kind == "namespace" {
        let prefix = format!("{}.", sym.name);
        let members: Vec<&str> = symbols
            .iter()
            .filter_map(|s| s.name.strip_prefix(&prefix))
            .filter(|m| !m.contains('.'))
            .collect();
        let mut v = format!("```wirescript\nnamespace {}\n```", sym.name);
        if !members.is_empty() {
            let _ = write!(
                v,
                "\n\n{} member{}: {}",
                members.len(),
                if members.len() == 1 { "" } else { "s" },
                members.join(", ")
            );
        }
        return Some(v);
    }

    // A read an `any` kind test narrowed shows the type it has here.
    let narrowed = source.lines().nth(line).and_then(|l| {
        let start = line_offset_at(source, line) + word_start_in_line(l, col);
        narrowed_reads.get(&(Arc::from(file), start, start + word.len()))
    });
    let mut v = match narrowed {
        Some(n) => {
            let here = SymbolDef {
                ty: Some(type_str(&n.ty)),
                ..sym.clone()
            };
            let mut v = render_decl_hover(
                &here,
                doc_comments,
                resource_estimates,
                Some((symbols, ast)),
            );
            v += &narrowing_note(sym.ty.as_deref().unwrap_or("any"), n);
            v
        }
        None => render_decl_hover(sym, doc_comments, resource_estimates, Some((symbols, ast))),
    };

    // For var reads: show exec/pure context at the hovered location
    if matches!(sym.kind, "var" | "static var") {
        let l = source.lines().nth(line)?;
        let offset = line_offset_at(source, line) + word_start_in_line(l, col);
        let f: Arc<str> = file.into();
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

/// A declaration's hover card: its signature (callables show `(exec, params)
/// -> ret`, everything else `kind name: type`), its doc comment, and for a
/// callable its resource estimate. Used by plain and namespace-member hover.
///
/// `value_ctx` is `(symbols, file)` of the file the declaration is in, for a
/// `const`'s value ([`const_hover_value`]). Namespace-member hover passes `None`:
/// that declaration is in another file.
fn render_decl_hover(
    sym: &SymbolDef,
    doc_comments: &DocComments,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    value_ctx: Option<(&[SymbolDef], &Script)>,
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
            } else {
                ty_str.to_string()
            };
            format!(
                "```wirescript\n{const_kw}{} {}{}\n```",
                sym.kind, sym.name, sig
            )
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
                            format!(", one of: {}", names.join(", "))
                        })
                        .unwrap_or_default();
                    (format!(": {b}"), members)
                }
                None => (String::new(), String::new()),
            };
            format!(
                "```wirescript\n{}{}  (generic type parameter)\n```\nA generic type parameter, resolved to a concrete type per call site{detail}.",
                sym.name, bound
            )
        }
        // `name: const string`: the keyword sits before the type, as in the
        // signature.
        "param" => {
            let ty_display = if sym.is_const {
                format!("const {ty_str}")
            } else {
                ty_str.to_string()
            };
            format!("```wirescript\nparam {}: {}\n```", sym.name, ty_display)
        }
        // A `const` binding shows `const`, not `let`, and its computed value when
        // `value_ctx` allows; a value that needs a call site or fails to evaluate is
        // left out.
        "let" if sym.is_const => {
            let value = value_ctx
                .and_then(|(symbols, ast)| const_hover_value(ast, symbols, sym.range.start.offset))
                .map(|lit| literal_display(&lit));
            match value {
                Some(val) => format!(
                    "```wirescript\nconst {}: {} = {}\n```",
                    sym.name, ty_str, val
                ),
                None => format!("```wirescript\nconst {}: {}\n```", sym.name, ty_str),
            }
        }
        _ => format!("```wirescript\n{} {}: {}\n```", sym.kind, sym.name, ty_str),
    };
    if let Some(doc) = doc_comments.get(&crate::parser::doc_key(&sym.range)) {
        let _ = write!(v, "\n\n{doc}");
    }
    if matches!(sym.kind, "mod" | "chip" | "fn")
        && let Some(est) = lookup_estimate(resource_estimates, &sym.name, sym.range.start.offset)
    {
        let ek = estimate_kind(sym.kind);
        let _ = write!(v, "\n\n{}", format_estimate(est, ek));
    }
    v
}

/// Hover for the member of a namespace-qualified reference (`drawTopText` in
/// `card.drawTopText`, where `card` is `import * as card`). It is stored under
/// its qualified name, which the bare-word lookup in [`hover_user_symbol`] does
/// not form.
fn hover_namespace_member(
    source: &str,
    symbols: &[SymbolDef],
    doc_comments: &DocComments,
    resource_estimates: &HashMap<String, ResourceEstimate>,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    // The cursor must be on the `member` half of an `obj.member` access.
    let obj_name = receiver_at(source, line, col)?;
    // `obj` must be a namespace alias for this to be a namespace-member access.
    if !symbols
        .iter()
        .any(|s| s.name == obj_name && s.kind == "namespace")
    {
        return None;
    }
    let qualified = format!("{obj_name}.{word}");
    let sym = symbols.iter().find(|s| s.name == qualified)?;
    Some(render_decl_hover(
        sym,
        doc_comments,
        resource_estimates,
        None,
    ))
}

/// A record-literal field key resolved through `type_map`, which has a type
/// for every `Expr::RecordLit` keyed by its range, so no shape is recognized
/// from text as [`resolve_record_lit_field`] does. The innermost literal wins
/// (`{value: {value: 10}}` spells both keys alike). The key span comes from the
/// field name's length, as in [`hover_enum_field_construction`].
fn resolve_record_lit_field_typed(
    source: &str,
    file: &str,
    ast: &Script,
    type_map: &TypeMap,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    let line_str = source.lines().nth(line)?;
    let word_off = line_offset_at(source, line) + word_start_in_line(line_str, col);

    let mut best: Option<(usize, usize)> = None;
    {
        let mut on_handler = |_: &Handler| {};
        let mut on_expr = |e: &Expr| {
            let Expr::RecordLit { fields, range } = e else {
                return;
            };
            for f in fields {
                let (name, key_start, key_end) = match f {
                    RecordLitField::Named { name, range, .. } => (
                        name.as_str(),
                        range.start.offset,
                        range.start.offset + name.len(),
                    ),
                    RecordLitField::Shorthand { name, range } => {
                        (name.as_str(), range.start.offset, range.end.offset)
                    }
                    RecordLitField::Spread { .. } => continue,
                };
                if name != word || word_off < key_start || word_off > key_end {
                    continue;
                }
                let span = (range.start.offset, range.end.offset);
                if best.is_none_or(|(bs, be)| span.1 - span.0 < be - bs) {
                    best = Some(span);
                }
            }
        };
        visit_program(ast, &mut on_handler, &mut on_expr);
    }
    let (start, end) = best?;
    let ty = type_map.get(&(Arc::from(file), start, end))?;
    let (Type::Record(fields) | Type::Outputs(fields)) = ty else {
        return None;
    };
    let (_, field_ty) = fields.iter().find(|(n, _)| n == word)?;
    Some(format!(
        "```wirescript\nfield {word}: {}\n```",
        super::types::type_str(field_ty)
    ))
}

fn resolve_record_lit_field(
    source: &str,
    symbols: &[SymbolDef],
    field: &str,
    line: usize,
) -> Option<String> {
    // Walk backwards from the current line to find a `let name: TypeName = {` pattern
    for scan_line in (0..=line).rev() {
        let l = source.lines().nth(scan_line)?;
        let trimmed = l.trim();

        if let Some(rest) = trimmed.strip_prefix("let ")
            && let Some(colon_pos) = rest.find(':')
        {
            let after_colon = rest[colon_pos + 1..].trim();
            let type_name = after_colon
                .split(|c: char| c == '=' || c.is_whitespace())
                .next()?;
            let type_name = type_name.trim();
            if type_name.is_empty() {
                continue;
            }

            for sym in symbols {
                if sym.kind == "type"
                    && sym.name == type_name
                    && let Some(ref ty_str) = sym.ty
                    && let Some(field_type) = extract_record_field_type(ty_str, field)
                {
                    return Some(format!(
                        "```wirescript\n{type_name}.{field}: {field_type}\n```"
                    ));
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

/// A field's type from a record type string like `{counter: *int, step: int}`.
/// An imported symbol carries only its type string (`SymbolDef.ty`), so a field
/// of an imported record is read from that string.
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
                Some(format!(
                    "{}: {}",
                    f[..c].trim(),
                    default_for_type(f[c + 1..].trim())
                ))
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
    if lines.is_empty() {
        return None;
    }
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

/// The innermost `MatchExpr` in `ast` containing the 1-based `(line, col)`
/// cursor. The closure's parameter is annotated with `'a` so the collected
/// `&'a Expr`s outlive [`super::visit::visit_program`]'s call (an inferred
/// closure lifetime cannot escape, E0521).
fn enclosing_match_expr<'a>(ast: &'a Script, line: u32, col: u32) -> Option<&'a Expr> {
    let mut candidates: Vec<&'a Expr> = Vec::new();
    let mut on_handler = |_: &'a Handler| {};
    let mut on_call = |e: &'a Expr| {
        if matches!(e, Expr::MatchExpr { .. }) {
            candidates.push(e);
        }
    };
    visit_program(ast, &mut on_handler, &mut on_call);
    candidates
        .into_iter()
        .filter(|e| range_contains_1based(e.range(), line, col))
        .min_by_key(|e| {
            let r = e.range();
            r.end.offset.saturating_sub(r.start.offset)
        })
}

/// The smallest `value is Enum.Variant` test in `ast` whose range contains
/// the 1-based `(line, col)` cursor.
fn enclosing_is_expr<'a>(ast: &'a Script, line: u32, col: u32) -> Option<&'a Expr> {
    use crate::ast::walk::{self, Node};
    fn collect<'a>(node: Node<'a>, out: &mut Vec<&'a Expr>) {
        if let Node::Expr(e @ Expr::Is { .. }) = node {
            out.push(e);
        }
        walk::children(node, &mut |c, _| collect(c, out));
    }
    let mut tests: Vec<&'a Expr> = Vec::new();
    walk::script_children(ast, &mut |n, _| collect(n, &mut tests));
    tests
        .into_iter()
        .filter(|e| range_contains_1based(e.range(), line, col))
        .min_by_key(|e| {
            let r = e.range();
            r.end.offset.saturating_sub(r.start.offset)
        })
}

/// Hover on the `is` of a variant test. On a kind test
/// (`x.getType() is Type.Int`) it also says what `x` narrows to.
fn hover_is_keyword(
    source: &str,
    ast: &Script,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    use crate::typecheck::narrow;
    if word != "is" {
        return None;
    }
    let cursor_line = (line + 1) as u32;
    let cursor_col = (char_col_to_byte(line_text(source, line), col) + 1) as u32;
    let Expr::Is { value, path, range } = enclosing_is_expr(ast, cursor_line, cursor_col)? else {
        return None;
    };
    // On the operator, not on a name spelled `is` inside either side.
    if range_contains_1based(value.range(), cursor_line, cursor_col)
        || range_contains_1based(path.range(), cursor_line, cursor_col)
    {
        return None;
    }
    if let (Some(subject), Some(test), Some(member)) = (
        narrow::test_subject_text(value),
        narrow::describe_test(value),
        narrow::type_member(path),
    ) {
        let mut v = format!(
            "```wirescript\n{test} is Type.{member}\n```\n**Kind test**: `true` while `{subject}` holds a `{member}` value."
        );
        if let Some(ty) = narrow::member_type(member) {
            let _ = write!(
                v,
                " When this test is a whole `if` condition, `{subject}` is narrowed to `{}` inside the block.",
                type_str(&ty)
            );
        }
        return Some(v);
    }
    let written = source
        .get(range.start.offset..range.end.offset)
        .filter(|t| !t.contains('\n'))
        .unwrap_or("value is Enum.Variant");
    Some(format!(
        "```wirescript\n{written}\n```\n**Variant test**: `true` while the value holds that variant. It compares \
         tags only; use `match` or `if let` to read a payload."
    ))
}

/// Hover on a `break` or `continue` statement in a loop body.
fn hover_loop_exit_keyword(
    source: &str,
    ast: &Script,
    word: &str,
    line: usize,
    col: usize,
) -> Option<String> {
    use crate::ast::walk::{self, Node};
    if word != "break" && word != "continue" {
        return None;
    }
    fn found(node: Node<'_>, line: u32, col: u32) -> bool {
        let hit = match node {
            Node::Stmt(Stmt::Break { range } | Stmt::Continue { range }) => {
                range_contains_1based(range, line, col)
            }
            _ => false,
        };
        let mut below = false;
        walk::children(node, &mut |c, _| below = below || found(c, line, col));
        hit || below
    }
    let cursor_line = (line + 1) as u32;
    let cursor_col = (char_col_to_byte(line_text(source, line), col) + 1) as u32;
    let mut hit = false;
    walk::script_children(ast, &mut |n, _| {
        hit = hit || found(n, cursor_line, cursor_col);
    });
    if !hit {
        return None;
    }
    let what = if word == "break" {
        "**Stop the loop**: nothing after it in the body runs, and no later iteration starts. The code after \
         the loop runs next."
    } else {
        "**Skip to the next iteration**: nothing after it in the body runs this time round, and the loop carries \
         on with its next element or number. It adds no gate: it ends this iteration's exec chain."
    };
    Some(format!("```wirescript\n{word}\n```\n{what}"))
}

/// The "fill missing match arms" edit for a cursor on or inside a `match` that
/// does not cover its enum: the innermost `MatchExpr` in `ast` at `(line, col)`,
/// its scrutinee's `Type::Enum` from `type_map`, and the arms the witness engine
/// ([`crate::typecheck::patterns::analyze`], as WS054 uses) finds missing. Each
/// renders through [`crate::typecheck::infer::render_pattern`] as
/// `  {pattern} => todo,`, inserted before the closing `}` at the last arm's
/// indentation (the match's own line plus two spaces when it has none).
///
/// `None` when the cursor is not on a `match`, the scrutinee is not a registered
/// enum, or every variant is covered.
pub fn fill_match_arms_at(
    source: &str,
    type_map: &TypeMap,
    ast: &Script,
    line: usize,
    col: usize,
) -> Option<MatchArmsFill> {
    let cursor_line = (line + 1) as u32;
    // AST ranges carry the lexer's BYTE columns, so the cursor's char column
    // has to be converted before it is compared against one.
    let cursor_col = (char_col_to_byte(line_text(source, line), col) + 1) as u32;

    let best = enclosing_match_expr(ast, cursor_line, cursor_col)?;
    let Expr::MatchExpr {
        scrutinee,
        arms,
        range,
    } = best
    else {
        return None;
    };

    let sr = scrutinee.range();
    let scrut_ty = type_map.get(&sr.key())?;
    if !matches!(
        scrut_ty,
        Type::Enum { .. } | Type::Int | Type::Float | Type::Bool | Type::String
    ) {
        return None;
    }

    let enum_defs = Arc::new(build_registry(&ast.decls));
    let types = DeclaredTypes::for_program_in(&ast.decls, &enum_defs, &ast.imports);
    let arm_patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
    let enums = crate::typecheck::patterns::Enums {
        defs: &enum_defs,
        types: &types,
    };
    let guarded: Vec<bool> = arms.iter().map(|a| a.guard.is_some()).collect();
    let usefulness = crate::typecheck::patterns::analyze(enums, scrut_ty, &arm_patterns, &guarded);
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

    // Each missing arm gets a bare `todo` body: it parses (as an undefined
    // identifier, WS002) and marks what to fill in. The server has no snippet
    // support, so a `${1:todo}` tab-stop would land as literal text, and a bare `$`
    // does not parse.
    let mut text = usefulness
        .missing
        .iter()
        .fold(String::new(), |mut text, w| {
            let pat = crate::typecheck::infer::render_pattern(&w.0);
            let _ = writeln!(text, "{indent}{pat} => todo,");
            text
        });

    // The match's own `range.end` is one past its closing `}` (the parser
    // sets it from the `}` token's own `end`), so the `}` itself sits at
    // `range.end.col - 2` (0-based) on `range.end.line - 1` (0-based): the
    // insertion point that keeps the new arms inside the braces.
    let close_line = (range.end.line - 1) as usize;
    let close_byte = (range.end.col as usize).saturating_sub(2);
    // A `}` sharing its line with the last arm (`match s { Circle(r) => 1.0 }`)
    // needs its own line break before the inserted arms; a `}` already alone
    // on its line (the common, formatted case) does not.
    let close_line_str = lines.get(close_line).copied().unwrap_or("");
    let close_byte = close_byte.min(close_line_str.len());
    let before_close = close_line_str.get(..close_byte).unwrap_or(close_line_str);
    if !before_close.trim().is_empty() {
        text.insert(0, '\n');
    }
    // The caller places an edit at this column, so hand back a char column
    // rather than the lexer's byte one.
    let close_col = super::text::byte_to_char_col(close_line_str, close_byte);
    Some(MatchArmsFill {
        line: close_line,
        col: close_col,
        text,
    })
}

pub(super) fn resolve_record_param_field_type(
    script: &crate::ast::Script,
    param_type: &TypeExpr,
    field: &str,
) -> Option<String> {
    let record_fields = match param_type {
        TypeExpr::Record { fields, .. } => fields,
        TypeExpr::Name { name, .. } => {
            for d in &script.decls {
                if let crate::ast::TopDecl::TypeAlias(ta) = d
                    && ta.name == *name
                    && let TypeExpr::Record { fields, .. } = &ta.typ
                {
                    return fields
                        .iter()
                        .find(|f| f.name == field)
                        .map(|f| type_expr_str(&f.typ));
                }
            }
            return None;
        }
        _ => return None,
    };
    record_fields
        .iter()
        .find(|f| f.name == field)
        .map(|f| type_expr_str(&f.typ))
}

/// A property read or written on a component reference, `light.Brightness`:
/// the same hover as `$Component/PointLight/Brightness`. The object is the
/// name before the `.`, or the call or index expression that ends there.
fn hover_component_property(
    source: &str,
    file: &str,
    type_map: &TypeMap,
    line: usize,
    col: usize,
    field: &str,
) -> Option<String> {
    let l = source.lines().nth(line)?;
    let c = char_col_to_byte(l, col);
    let start = word_start_at_byte(l, c);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let obj_end = start - 1;
    let obj_start = word_start_at_byte(l, obj_end);
    let lo = line_offset_at(source, line);
    let f: Arc<str> = file.into();
    let named = (obj_start < obj_end)
        .then(|| find_obj_type(type_map, &f, lo + obj_start, lo + obj_end))
        .flatten();
    let obj_ty = named.or_else(|| {
        type_map
            .iter()
            .filter(|((f2, _, e), _)| **f2 == *f && *e == lo + obj_end)
            .max_by_key(|((_, s, _), _)| *s)
            .map(|(_, t)| t.clone())
    })?;
    let Type::Component(class) = crate::types::mono::unwrap_ref(&obj_ty) else {
        return None;
    };
    if class.is_empty() {
        return None;
    }
    Some(render_component_type_hover(&format!(
        "{}/{field}",
        crate::catalog::component_name(&class)
    )))
}

/// The line a hover adds for a read a kind test narrowed.
fn narrowing_note(declared: &str, n: &crate::typecheck::NarrowedRead) -> String {
    format!(
        "\n\nDeclared `{declared}`, narrowed to `{}` here by {}.",
        type_str(&n.ty),
        n.by
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the hover tables are separate maps the caller already holds"
)]
fn resolve_field_hover(
    source: &str,
    file: &str,
    type_map: &TypeMap,
    narrowed_reads: &NarrowedReadMap,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
    field: &str,
) -> Option<String> {
    let l = source.lines().nth(line)?;
    let c = char_col_to_byte(l, col);
    let start = word_start_at_byte(l, c);
    if start == 0 || l.as_bytes()[start - 1] != b'.' {
        return None;
    }
    let obj_end = start - 1;
    let obj_start = word_start_at_byte(l, obj_end);
    let obj_name = &l[obj_start..obj_end];
    let lo = line_offset_at(source, line);
    let field_end_col = word_end_at_byte(l, c);

    let f: Arc<str> = file.into();
    let fmt_field = |ty_display: String| format!("```wirescript\nfield {field}: {ty_display}\n```");

    // Layer 1: Full expression span (obj.field) in type_map - best case, typechecker
    // recorded the type of the entire dotted expression. Skip a bare `any`: for a
    // `.field` that's the error-fallback type of a field that didn't resolve, so
    // fall through to structural resolution / the record-type fallback below
    // rather than showing an unhelpful `field z: any`.
    let span = (Arc::clone(&f), lo + obj_start, lo + field_end_col);
    if let Some(n) = narrowed_reads.get(&span) {
        return Some(fmt_field(type_str(&n.ty)) + &narrowing_note("any", n));
    }
    if let Some(ty) = type_map.get(&span)
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

    // The field did not resolve; if the object is a record, show its whole type,
    // one field per line in a fenced `wirescript` block (VS Code colors it), so
    // hovering an erroring `x.Jump` lists the valid fields. `type_map` first (a real
    // `Type::Record`), then the symbol table (a type string for a named alias or an
    // `in` port).
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
        if let Type::Record(fields) | Type::Outputs(fields) = rec {
            let body = fields.iter().fold(String::new(), |mut body, (n, t)| {
                let _ = write!(body, "\n  {n}: {},", type_str(t));
                body
            });
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

/// The object's record type string (`{ x: int, y: int }`) from the symbol
/// table: an inline record type, or a named alias resolved to its `type`
/// declaration. `None` when the object is not a record.
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
    let inner = rec
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();
    let body =
        inner
            .split(", ")
            .filter(|f| !f.trim().is_empty())
            .fold(String::new(), |mut body, f| {
                let _ = write!(body, "\n  {},", f.trim());
                body
            });
    format!("```wirescript\n{{{body}\n}}\n```")
}

/// Look up `obj_name` in symbols, find its type declaration, and resolve `field`
/// from the type's string representation.
fn resolve_field_via_symbols(symbols: &[SymbolDef], obj_name: &str, field: &str) -> Option<String> {
    let sym = symbols.iter().find(|s| s.name == obj_name)?;
    let ty_name = sym.ty.as_deref()?;

    symbols
        .iter()
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

/// The type of the object expression at a span, from `type_map`. The parser's
/// end offsets can differ from the ones computed here from text, so the lookup
/// tries the exact span, then the same start with the end off by one, then any
/// entry with the same start.
fn find_obj_type(
    type_map: &TypeMap,
    file: &Arc<str>,
    obj_start: usize,
    obj_end: usize,
) -> Option<Type> {
    // Tier 1: exact span
    if let Some(ty) = type_map.get(&(Arc::clone(file), obj_start, obj_end)) {
        return Some(ty.clone());
    }

    // Tier 2: fuzzy end offset (+/-1)
    for end in [obj_end.wrapping_sub(1), obj_end + 1] {
        if let Some(ty) = type_map.get(&(Arc::clone(file), obj_start, end)) {
            return Some(ty.clone());
        }
    }

    // Tier 3: scan for any entry starting at obj_start in this file
    for ((f, s, _e), ty) in type_map {
        if **f == **file && *s == obj_start {
            return Some(ty.clone());
        }
    }

    None
}

fn resolve_field_in_type(ty: &Type, field: &str) -> Option<Type> {
    match ty {
        Type::Record(fields) | Type::Outputs(fields) => fields
            .iter()
            .find(|(k, _)| k == field)
            .map(|(_, t)| t.clone()),
        Type::Ref(inner) => {
            if crate::catalog::is_var_pseudo_field(field) || field.eq_ignore_ascii_case("VarRef") {
                return Some(inner.as_ref().clone());
            }
            resolve_field_in_type(inner, field)
        }
        _ => crate::catalog::value_fields::field_of(ty, field).map(|_| Type::Float),
    }
}

#[cfg(test)]
mod tests;
