//! Completion: what may be typed at a position. Each editor front end maps
//! these items to its own completion type.

use super::{
    CollectionKind, SymbolDef, asset_names, asset_type_for_port, asset_types, char_col_to_byte,
    collection_kind, component_member_completions, cursor_byte_offset, environment_completions,
    find_enclosing_call, member_receiver_at, named_arg_value, param_names, receiver_methods,
    record_field_names, resolve_symbol, swizzle_fields, type_str, user_receiver_methods,
};
use crate::catalog::arrays::ARRAY_METHODS;
use crate::catalog::calls::calls;
use crate::catalog::events::events;
use crate::catalog::maps::MAP_METHODS;
use crate::collections::HashMap;
use crate::lexer::KEYWORDS;
use crate::typecheck::enums::{
    DISCRIMINANT_FIELD, EnumDef, EnumRegistry, build_registry, game_enum_defs,
};

/// How an editor finds the enum named by the text it completes. Only an
/// editor's build has one, so the engine is handed it.
pub type EnumLookup = dyn for<'r> Fn(&'r EnumRegistry, &str) -> Option<&'r EnumDef>;

/// What a completion offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompletionKind {
    #[default]
    Text,
    Method,
    Function,
    Field,
    Variable,
    Class,
    Module,
    Property,
    Keyword,
    Constant,
    EnumMember,
    Event,
    File,
}

/// One completion item.
#[derive(Clone, Debug, Default)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    /// What to insert, when it is not the label.
    pub insert_text: Option<String>,
    /// What the editor filters by, when it is not the label.
    pub filter_text: Option<String>,
    /// The byte column on the cursor's line from which the label replaces the
    /// text up to the cursor, for a completion that spans `.` and `/`.
    pub replace_from: Option<usize>,
}

impl Completion {
    pub fn new(label: impl Into<String>, kind: CompletionKind) -> Self {
        Self {
            label: label.into(),
            kind,
            ..Default::default()
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn documentation(mut self, doc: impl Into<String>) -> Self {
        self.documentation = Some(doc.into());
        self
    }

    #[must_use]
    pub fn insert_text(mut self, text: impl Into<String>) -> Self {
        self.insert_text = Some(text.into());
        self
    }

    #[must_use]
    pub fn filter_text(mut self, text: impl Into<String>) -> Self {
        self.filter_text = Some(text.into());
        self
    }

    #[must_use]
    pub fn replace_from(mut self, byte_col: usize) -> Self {
        self.replace_from = Some(byte_col);
        self
    }
}

/// Completions for `receiver.`: array methods, var fields, record fields or the
/// receiver methods of a typed value. Only the receiver's members, possibly none;
/// it never falls through to the global list, so a `string` receiver shows only
/// string methods.
fn member_completions(
    var_name: &str,
    symbols: &[SymbolDef],
    source: &str,
    line: usize,
    col: usize,
    lookup: &EnumLookup,
) -> Vec<Completion> {
    let mut items = Vec::new();

    // A component reference: its properties and `Remove`.
    if let Some(found) = resolve_symbol(symbols, source, var_name, line, col)
        .and_then(|s| s.ty.as_deref())
        .and_then(component_member_completions)
    {
        return found
            .into_iter()
            .map(|(label, detail)| {
                let kind = if label == "Remove" {
                    CompletionKind::Method
                } else {
                    CompletionKind::Property
                };
                Completion::new(label, kind).detail(detail)
            })
            .collect();
    }

    let enum_registry = enum_registry_from_source(source);

    // `arr[i].` is an indexed read: its members are the array-get gate's outputs
    // (the element `Value` and the `OutOfBounds` flag), not the array's methods.
    if let Some(base) = var_name.strip_suffix("[]") {
        let sym = resolve_symbol(symbols, source, base, line, col);
        let elem = sym
            .and_then(|s| s.ty.as_deref())
            .and_then(|t| t.strip_suffix("[]"))
            .unwrap_or("");
        for (name, ty) in [("Value", elem), ("OutOfBounds", "bool")] {
            items.push(
                Completion::new(name, CompletionKind::Field)
                    .detail(if ty.is_empty() {
                        "field".to_string()
                    } else {
                        format!("{name}: {ty}")
                    })
                    .insert_text(name),
            );
        }
        return items;
    }

    let sym = resolve_symbol(symbols, source, var_name, line, col);

    // Field name (record field / swizzle component) completion item. Declared
    // ahead of the bare enum-type receiver check below so that check's bare-
    // variant fallback can also reach `push_type_members` (which closes over
    // `field_item`).
    let field_item = |name: String| {
        Completion::new(name.clone(), CompletionKind::Field)
            .detail("field")
            .insert_text(name)
    };
    // Method + swizzle members valid for a typed value: an enum-typed value's
    // `.Discriminant`, swizzle fields, builtin receiver-methods, then
    // in-scope user `self`-mods whose receiver matches.
    let push_type_members = |ty: &str, items: &mut Vec<Completion>| {
        if lookup(&enum_registry, ty).is_some() {
            items.push(
                Completion::new(DISCRIMINANT_FIELD, CompletionKind::Field)
                    .detail(format!("{ty} discriminant (int)"))
                    .insert_text(DISCRIMINANT_FIELD),
            );
        }
        for f in swizzle_fields(ty) {
            items.push(field_item(f.to_string()));
        }
        for (name, sig) in receiver_methods(ty) {
            items.push(Completion::new(name, CompletionKind::Method).detail(sig));
        }
        for (name, sig) in user_receiver_methods(ty, symbols) {
            items.push(
                Completion::new(name.clone(), CompletionKind::Method)
                    .detail(format!("{name}{sig}"))
                    .insert_text(name),
            );
        }
    };

    // A bare enum-type receiver (`Shape.<here>`) is a variant path, ready for
    // construction or `.Discriminant`, unless a value binding shadows the name.
    // `collect_symbols_for_file` emits no symbol for an `enum` declaration, so an
    // unshadowed enum name resolves to `None` (or a `type` symbol), while a value
    // of that name resolves here and wins, as in `resolve_variant_for_construction`.
    if sym.is_none_or(|s| s.kind == "type") {
        if let Some(def) = lookup(&enum_registry, var_name) {
            push_variant_completions(&mut items, def);
            return items;
        }
        // A bare variant used as a value receiver (`EasingFunction.Bounce.`) arrives
        // as just `Variant`, since `member_receiver_at` reports only the identifier
        // before the dot. Resolved as the compiler resolves a bare variant
        // (`resolve_bare_variant_enum`): a unique owning enum makes it a value of that
        // type, offering `.Discriminant`; an ambiguous or unknown name falls through.
        if let Some(owner) =
            crate::typecheck::enums::resolve_bare_variant_enum(&enum_registry, var_name, |n| {
                lookup(&enum_registry, n).is_some()
            })
        {
            push_type_members(&owner.name, &mut items);
            if !items.is_empty() {
                return items;
            }
        }
    }

    // Collection methods follow the receiver's declared type, through aliases: a
    // `Map<K, V>` gets the map table, `T[]` or an `array` declaration the array
    // table. A map's `length`, `clear` and `copyFrom` are its own, and `get`,
    // `set`, `has`, `keys` and `values` are map-only.
    let collection = sym.and_then(|s| {
        s.ty.as_deref()
            .and_then(|ty| collection_kind(ty, symbols))
            .or_else(|| (s.kind == "array").then_some(CollectionKind::Array))
    });
    if let Some(kind) = collection {
        let method_item = |name: &str, signature: &str, doc: &str| {
            Completion::new(name, CompletionKind::Method)
                .detail(format!("{name}{signature}"))
                .documentation(doc)
        };
        match kind {
            CollectionKind::Array => {
                for m in ARRAY_METHODS {
                    items.push(method_item(m.name, &m.shown_signature(), m.doc));
                }
            }
            CollectionKind::Map => {
                for m in MAP_METHODS {
                    items.push(method_item(m.name, &m.shown_signature(), m.doc));
                }
            }
        }
        return items;
    }

    // Namespace alias (`import * as u`): offer its qualified `u.member` symbols.
    if sym.is_some_and(|s| s.kind == "namespace") {
        let prefix = format!("{var_name}.");
        for m in symbols {
            if let Some(member) = m.name.strip_prefix(&prefix) {
                if member.contains('.') {
                    continue; // deeper nesting isn't a direct member
                }
                items.push(
                    Completion::new(member, namespace_member_kind(m.kind)).insert_text(member),
                );
            }
        }
        return items;
    }

    // Vars (mutable `var` / `static var`) expose `.Value`/`.prev`, plus any
    // method/swizzle valid for the var's element type (`pos.Normalize()`,
    // `pos.x`).
    if sym.is_some_and(|s| matches!(s.kind, "var" | "static var")) {
        for (name, detail) in &crate::catalog::VAR_PSEUDO_FIELDS {
            items.push(
                Completion::new(*name, CompletionKind::Field)
                    .detail(*detail)
                    .insert_text(*name),
            );
        }
        if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
            push_type_members(ty, &mut items);
        }
        return items;
    }

    // Record-typed value (e.g. `let split = pl.InputReader()` -> {Forward, Right,
    // Jump}, or a multi-output mod result): offer the record's field names, so
    // `split.<here>` / `on split.<here>` completes `Forward`/`Right`/`Jump`. The
    // type may be an inline `{...}` string or a named `type` alias resolved here.
    if let Some(fields) = sym
        .and_then(|s| s.ty.as_deref())
        .and_then(|ty| resolve_record_fields(ty, symbols))
    {
        for f in fields {
            items.push(field_item(f));
        }
        return items;
    }

    // Any other typed value: methods (e.g. string methods on a string) + swizzle.
    if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
        push_type_members(ty, &mut items);
    }

    items
}

/// Record field names for a receiver type string: an inline `{...}` record, or a
/// named `type` alias resolved through the `type` symbol it points at.
fn resolve_record_fields(ty: &str, symbols: &[SymbolDef]) -> Option<Vec<String>> {
    if let Some(fields) = record_field_names(ty) {
        return Some(fields);
    }
    let alias = symbols.iter().find(|s| s.name == ty && s.kind == "type")?;
    record_field_names(alias.ty.as_deref()?)
}

/// The completion-item kind for a namespace member of the given symbol kind.
fn namespace_member_kind(kind: &str) -> CompletionKind {
    match kind {
        "mod" | "chip" | "fn" => CompletionKind::Function,
        "let" => CompletionKind::Constant,
        "type" => CompletionKind::Class,
        "event" => CompletionKind::Event,
        _ => CompletionKind::Field,
    }
}

/// The `enum` declarations in `source` by name, plus the `Option` and `Result`
/// prelude, parsed on demand so enum-aware completion needs no cached AST. A
/// file without the text `enum ` declares none, so it skips the parse and gets
/// the same registry.
fn enum_registry_from_source(source: &str) -> EnumRegistry {
    if !source.contains("enum ") {
        return build_registry(&[]);
    }
    let ast = crate::on_big_stack(|| crate::parse(source, "completion")).ast;
    build_registry(&ast.decls)
}

/// Push a completion item for each variant of `def` - the user-enum analog of
/// `push_enum_member_completions` below, used both for `match`-arm-head
/// pattern completion and for a bare `Enum.<here>` variant-path receiver.
fn push_variant_completions(items: &mut Vec<Completion>, def: &EnumDef) {
    for v in &def.variants {
        items.push(
            Completion::new(v.name.clone(), CompletionKind::EnumMember)
                .detail(format!("{} variant", def.name))
                .insert_text(v.name.clone()),
        );
    }
}

/// When the cursor is at an arm head of `match <scrutinee> { <here> }` (inside
/// the match's own braces, after the last arm separator and before any `=>`),
/// the scrutinee's source text, trimmed. `None` inside a pattern's parens, an
/// arm's body, or outside any match.
///
/// One scan of the prefix up to the cursor, skipping strings and comments, with
/// a frame stack for brace, paren and bracket nesting. Only the `{ }` opened by
/// `match <expr>` is `MatchArms`, an arm's block body (a `{` right after `=>`)
/// is `ArmBlock`, and everything else is `Other`. Popping an `ArmBlock` returns
/// its arm to `Head`, since a block arm may omit its comma. A scrutinee that
/// contains a `{` (`match f(){x:1} { ... }`) is not handled.
pub fn match_arm_head_scrutinee_at(source: &str, line: usize, col: usize) -> Option<String> {
    let offset = cursor_byte_offset(source, line, col);
    let prefix = &source[..offset];
    let bytes = prefix.as_bytes();

    // Per-arm position within a `MatchArms` frame: `Head` is pattern position
    // (the only state that offers variant completions); `AwaitingBody` is the
    // gap right after `=>` before the body's first token (which decides block
    // vs expression body); `InBody` is anywhere in an expression body.
    #[derive(PartialEq)]
    enum ArmState {
        Head,
        AwaitingBody,
        InBody,
    }

    enum Frame {
        MatchArms { scrutinee: String, state: ArmState },
        Other,
        // The `{ }` block body of a match arm; popping it returns the
        // enclosing arm to `Head` (a block arm may drop its trailing comma).
        ArmBlock,
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut pending_match_start: Option<usize> = None;
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = in_string {
            if c == b'\\' {
                i += 2; // skip the escaped char
            } else {
                if c == q {
                    in_string = None;
                }
                i += 1;
            }
            continue;
        }
        // The first non-trivia token after `=>` fixes the body shape: a `{`
        // opens a block body (handled below), anything else is an expression
        // body, so a later `{` in it (a record literal) is not the arm block.
        if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'{')
            && let Some(Frame::MatchArms { state, .. }) = stack.last_mut()
            && *state == ArmState::AwaitingBody
        {
            *state = ArmState::InBody;
        }
        match c {
            b'"' | b'\'' => {
                in_string = Some(c);
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                in_line_comment = true;
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                in_block_comment = true;
                i += 2;
            }
            b'{' => {
                if let Some(start) = pending_match_start.take() {
                    let scrutinee = prefix[start..i].trim().to_string();
                    stack.push(Frame::MatchArms {
                        scrutinee,
                        state: ArmState::Head,
                    });
                } else if matches!(
                    stack.last(),
                    Some(Frame::MatchArms {
                        state: ArmState::AwaitingBody,
                        ..
                    })
                ) {
                    stack.push(Frame::ArmBlock);
                } else {
                    stack.push(Frame::Other);
                }
                i += 1;
            }
            b'}' => {
                let popped = stack.pop();
                if matches!(popped, Some(Frame::ArmBlock))
                    && let Some(Frame::MatchArms { state, .. }) = stack.last_mut()
                {
                    *state = ArmState::Head;
                }
                i += 1;
            }
            b'(' | b'[' => {
                stack.push(Frame::Other);
                i += 1;
            }
            b')' | b']' => {
                stack.pop();
                i += 1;
            }
            b',' => {
                if let Some(Frame::MatchArms { state, .. }) = stack.last_mut() {
                    *state = ArmState::Head;
                }
                i += 1;
            }
            b'=' if bytes.get(i + 1) == Some(&b'>') => {
                if let Some(Frame::MatchArms { state, .. }) = stack.last_mut() {
                    *state = ArmState::AwaitingBody;
                }
                i += 2;
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if &prefix[start..i] == "match" {
                    pending_match_start = Some(i);
                }
            }
            _ => i += 1,
        }
    }

    match stack.last() {
        Some(Frame::MatchArms {
            scrutinee,
            state: ArmState::Head,
        }) if !scrutinee.is_empty() => Some(scrutinee.clone()),
        _ => None,
    }
}

/// Push a completion item for each member of enum `et`. `filter_text` is set to
/// whatever is already typed (`value_so_far`) so VS Code keeps showing every
/// sibling even when the cursor sits at the end of a complete member. Shared by
/// the `CallSpec` and `EventSpec` named-arg value paths.
fn push_enum_member_completions(items: &mut Vec<Completion>, et: &str, value_so_far: &str) {
    let filter = value_so_far.trim().to_string();
    for v in crate::catalog::enum_member_names(et) {
        items.push(
            Completion::new(v.clone(), CompletionKind::EnumMember)
                .detail(format!("{et} member"))
                .insert_text(v)
                .filter_text(filter.clone()),
        );
    }
}

/// Push a qualified item (`EasingFunction.Bounce`) for each variant of the
/// built-in game enum backed by schema enum `et`, beside
/// [`push_enum_member_completions`]'s bare names, so a config slot offers both
/// forms. `et` is the raw schema type from `config_enum_for_named_arg`; at most
/// one game enum maps to it ([`crate::catalog::game_enum_schema_type`]).
fn push_qualified_builtin_game_enum_variant_completions(
    items: &mut Vec<Completion>,
    et: &str,
    value_so_far: &str,
) {
    let filter = value_so_far.trim().to_string();
    let Some(def) = game_enum_defs()
        .into_iter()
        .find(|def| crate::catalog::game_enum_schema_type(&def.name) == Some(et))
    else {
        return;
    };
    for v in &def.variants {
        let label = format!("{}.{}", def.name, v.name);
        items.push(
            Completion::new(label.clone(), CompletionKind::EnumMember)
                .detail(format!("{} variant", def.name))
                .insert_text(label)
                .filter_text(filter.clone()),
        );
    }
}

/// The prefab file reference (`$./file.brz`, `$/abs/file.brz`) the cursor at
/// `line`/`col` (a character column) is typing: the byte column of its `$` and
/// the text after it. `None` anywhere else, so a frontend can skip listing
/// files on disk for every other completion.
pub fn prefab_path_fragment(source: &str, line: usize, col: usize) -> Option<(usize, &str)> {
    let l = source.lines().nth(line)?;
    let before = &l[..char_col_to_byte(l, col)];
    let dollar = before.rfind('$')?;
    let frag = &before[dollar + 1..];
    let is_prefab_frag = (frag.starts_with('.') || frag.starts_with('/'))
        && frag
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | '.' | '-'));
    is_prefab_frag.then_some((dollar, frag))
}

/// The completions at `line`/`col` (a character column) of `source`, whose
/// symbols are `symbols`; `prefab_paths` are the `.brz` paths a `$./` reference
/// may complete to, and `lookup` finds an enum by the text completed.
pub fn completions(
    source: &str,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
    prefab_paths: &[String],
    lookup: &EnumLookup,
) -> Vec<Completion> {
    let mut items = Vec::new();

    // Prefab file reference `$./file.brz` / `$/abs/file.brz`: complete from the
    // candidate paths the frontend supplied (disk scan / drag registry). A
    // text edit over the whole `$...` fragment keeps `.`/`/` filtering robust.
    if let Some((dollar, frag)) = prefab_path_fragment(source, line, col) {
        for path in prefab_paths {
            if path.starts_with(frag) {
                items.push(
                    Completion::new(path.clone(), CompletionKind::File).replace_from(dollar + 1),
                );
            }
        }
        if !items.is_empty() {
            return items;
        }
    }

    // Asset reference `$AssetType/AssetName`: complete types after `$`, names
    // after `$Type/`.
    if let Some(l) = source.lines().nth(line) {
        let col_idx = char_col_to_byte(l, col);
        let before = &l[..col_idx];
        if let Some(dollar) = before.rfind('$') {
            let frag = &before[dollar + 1..];
            if frag
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '/')
            {
                if let Some(slash) = frag.rfind('/') {
                    for name in asset_names(&frag[..slash]) {
                        items.push(Completion::new(name, CompletionKind::Constant));
                    }
                } else {
                    for ty in asset_types() {
                        items.push(
                            Completion::new(ty, CompletionKind::Class)
                                .insert_text(format!("{ty}/")),
                        );
                    }
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // Member access `receiver.partial`: only the receiver's members. Checked
    // before call-parameter completion, so `Call(arg = recv.<here>` shows `recv`'s
    // methods; a plain `Call(` yields no receiver and falls through.
    // `Environment.<env>.<property>`: environment names, then properties.
    let env_items = if symbols.iter().any(|s| s.name == "Environment") {
        None
    } else {
        environment_completions(source, line, col)
    };
    if let Some(items) = env_items {
        return items
            .into_iter()
            .map(|(label, detail)| Completion::new(label, CompletionKind::Property).detail(detail))
            .collect();
    }
    if let Some(var_name) = member_receiver_at(source, line, col) {
        return member_completions(&var_name, symbols, source, line, col, lookup);
    }

    // Match-arm-head position (`match <scrutinee> { <here> }`): if the
    // scrutinee resolves to a registered enum, offer its variant names as
    // patterns (`Circle`, `Empty`, ...). Checked before call-param
    // completion for the same reason as member access above - a `match`
    // isn't itself a call, but its arm head can sit inside one lexically
    // (`foo(x = match s { <here> })`), and the arm-head reading must win.
    if let Some(scrutinee) = match_arm_head_scrutinee_at(source, line, col)
        && let Some(ty) =
            resolve_symbol(symbols, source, &scrutinee, line, col).and_then(|s| s.ty.as_deref())
    {
        let registry = enum_registry_from_source(source);
        if let Some(def) = lookup(&registry, ty) {
            push_variant_completions(&mut items, def);
            if !items.is_empty() {
                return items;
            }
        }
    }

    // Named params inside a function call: `Call(<here>)`.
    if let Some(call_name) = find_enclosing_call(source, line, col) {
        if let Some(spec) = calls().get(call_name.as_str()) {
            // Enum-valued named arg (e.g. `justify = Center`): complete the
            // enum's member names when the cursor is in the value slot. Members
            // insert bare (the idiomatic form; a quoted string also works).
            let value_ctx = named_arg_value(source, line, col);
            if let Some((param_name, value_so_far)) = value_ctx.as_ref() {
                // An enum config value, for a declared parameter (`justify = ...`) or a raw
                // config field (`Justification = ...`), through the shared call and event
                // helper.
                if let Some(et) =
                    crate::catalog::config_enum_for_named_arg(call_name.as_str(), param_name)
                {
                    push_enum_member_completions(&mut items, et, value_so_far);
                    push_qualified_builtin_game_enum_variant_completions(
                        &mut items,
                        et,
                        value_so_far,
                    );
                    if !items.is_empty() {
                        return items;
                    }
                }
                if let Some(param) = spec.params.iter().find(|p| p.name == param_name) {
                    // An asset config parameter (`font = <here>`, `weapon = <here>`): offer full
                    // `$Type/Name` references for its asset type (once `$` is typed, the
                    // `$Type/` block above takes over). Constant-only parameters only, since a
                    // wire-input Entity port takes a live value.
                    if !crate::catalog::is_wire_input(spec.gate_class, param.port.as_str())
                        && let Some(asset_ty) = asset_type_for_port(param.port.as_str())
                    {
                        for name in asset_names(asset_ty) {
                            let full = format!("${asset_ty}/{name}");
                            items.push(
                                Completion::new(full.clone(), CompletionKind::Constant)
                                    .detail(format!("{asset_ty} asset"))
                                    .insert_text(full),
                            );
                        }
                        if !items.is_empty() {
                            return items;
                        }
                    }
                }
            }
            // Argument-NAME completions: only when NOT completing a value. In a
            // `name = <here>` value slot that wasn't an enum/asset value, skip the
            // arg names and fall through to the in-scope identifier list below.
            if value_ctx.is_none() {
                // The param a receiver binds is already supplied via the
                // `x.Method(` syntax: the first one, or the one the call names
                // (`SendCustomEvent`'s `target`). Don't offer it.
                let receiver_param = spec
                    .receiver
                    .is_some()
                    .then(|| {
                        spec.receiver_target_param()
                            .or(spec.params.first().map(|p| p.name))
                    })
                    .flatten();
                for p in &spec.params {
                    if Some(p.name) == receiver_param {
                        continue;
                    }
                    // A config param surfaced as a plain int but backed by a schema
                    // enum shows the enum's name instead of `int`.
                    let ty_label = crate::field_enum_type(spec.gate_class, p.port.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| type_str(&p.ty));
                    if p.optional {
                        items.push(
                            Completion::new(format!("{} = ", p.name), CompletionKind::Field)
                                .detail(format!("{ty_label} (optional)"))
                                .insert_text(format!("{} = ", p.name)),
                        );
                    } else {
                        items.push(
                            Completion::new(p.name, CompletionKind::Field)
                                .detail(format!("{ty_label} (required)")),
                        );
                    }
                }
                // Data-driven config attributes: the gate's raw settings-menu field
                // names (those without a hand-coded alias param).
                for cfg in crate::catalog::scalar_config_fields(spec.gate_class) {
                    if spec.params.iter().any(|p| p.name == cfg.name) {
                        continue;
                    }
                    let ty_label =
                        crate::catalog::config_field_enum_type(spec.gate_class, &cfg.name)
                            .map(str::to_string)
                            .unwrap_or_else(|| cfg.ty.clone());
                    items.push(
                        Completion::new(format!("{} = ", cfg.name), CompletionKind::Field)
                            .detail(format!("{ty_label} (config)"))
                            .insert_text(format!("{} = ", cfg.name)),
                    );
                }
                if !items.is_empty() {
                    return items;
                }
            }
        } else if let Some(evt) = crate::catalog::events::find_event(call_name.as_str()) {
            // Event trigger config (`on Clock(<here>)`): the call-param path has
            // no CallSpec, so resolve names/enum values from the EventSpec.
            let value_ctx = named_arg_value(source, line, col);
            if let Some((param_name, value_so_far)) = value_ctx.as_ref()
                && let Some(et) =
                    crate::catalog::config_enum_for_named_arg(call_name.as_str(), param_name)
            {
                push_enum_member_completions(&mut items, et, value_so_far);
                push_qualified_builtin_game_enum_variant_completions(&mut items, et, value_so_far);
                if !items.is_empty() {
                    return items;
                }
            }
            // Argument-NAME completions only when NOT completing a value; in a
            // value slot, fall through to the in-scope identifier list below.
            if value_ctx.is_none() {
                for (surf, _, _) in &evt.input_named {
                    items.push(
                        Completion::new(format!("{surf} = "), CompletionKind::Field)
                            .detail("wired input")
                            .insert_text(format!("{surf} = ")),
                    );
                }
                for (surf, field) in &evt.config_named {
                    let ty_label = crate::catalog::config_field_enum_type(evt.gate_class, field)
                        .map(str::to_string)
                        .unwrap_or_else(|| "config".to_string());
                    items.push(
                        Completion::new(format!("{surf} = "), CompletionKind::Field)
                            .detail(format!("{ty_label} (config)"))
                            .insert_text(format!("{surf} = ")),
                    );
                }
                if !items.is_empty() {
                    return items;
                }
            }
        } else if let Some(sig) = symbols
            .iter()
            .find(|s| s.name == call_name.as_str() && matches!(s.kind, "mod" | "chip" | "fn"))
            .and_then(|s| s.ty.as_deref())
        {
            // User-defined mod/chip/fn call: complete its parameter names,
            // parsed from the signature string in the symbol's type.
            if let Some(names) = param_names(sig) {
                for name in names {
                    items.push(
                        Completion::new(name.clone(), CompletionKind::Field).insert_text(name),
                    );
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // User symbols. Qualified namespace members (`u.member`) complete only after
    // `u.`, so they stay out of this list. A name declared in several scopes is
    // offered once, as the declaration in scope at the cursor (the nearest one at
    // or before it, else the first), so a handler-local `players` stays out of
    // file scope. The per-name choice is computed in one pass over the symbols,
    // with `resolve_symbol`'s rule.
    let byte_col = super::text::char_col_to_byte(super::text::line_text(source, line), col);
    let (cl, cc) = ((line + 1) as u32, (byte_col + 1) as u32);
    let mut first: HashMap<&str, &SymbolDef> = HashMap::default();
    let mut best: HashMap<&str, (&SymbolDef, (u32, u32))> = HashMap::default();
    for s in symbols {
        if s.name.contains('.') {
            continue;
        }
        first.entry(s.name.as_str()).or_insert(s);
        let p = (s.range.start.line, s.range.start.col);
        let precedes = p.0 < cl || (p.0 == cl && p.1 <= cc);
        if precedes && best.get(s.name.as_str()).is_none_or(|(_, bp)| p > *bp) {
            best.insert(s.name.as_str(), (s, p));
        }
    }
    let mut seen: crate::collections::HashSet<&str> = crate::collections::HashSet::default();
    for sym in symbols {
        if sym.name.contains('.') {
            continue;
        }
        if !seen.insert(sym.name.as_str()) {
            continue;
        }
        let chosen = best
            .get(sym.name.as_str())
            .map(|(s, _)| *s)
            .or_else(|| first.get(sym.name.as_str()).copied())
            .unwrap_or(sym);
        let kind = match chosen.kind {
            "var" | "static var" | "buffer" | "array" => CompletionKind::Variable,
            "fn" | "mod" | "chip" => CompletionKind::Function,
            "in" => CompletionKind::Field,
            "let" => CompletionKind::Constant,
            "event" => CompletionKind::Event,
            "namespace" => CompletionKind::Module,
            "type" => CompletionKind::Class,
            _ => CompletionKind::Text,
        };
        items.push(Completion {
            detail: chosen.ty.clone(),
            ..Completion::new(chosen.name.clone(), kind)
        });
    }

    for kw in KEYWORDS {
        items.push(Completion::new(*kw, CompletionKind::Keyword));
    }

    // Annotations: `@side` port pins, the chip annotations, and the
    // module-level run that opens a file.
    for (ann, detail) in [
        ("@left", "outer rerouter pin"),
        ("@right", "outer rerouter pin"),
        ("@top", "outer rerouter pin"),
        ("@bottom", "outer rerouter pin"),
        ("@label", "display-text override"),
        ("@color", "brick color (Brick Property Changer)"),
        ("@material", "brick material (Brick Property Changer)"),
        ("@closed", "compile chip collapsed"),
        ("@sliced", "let a for loop run across ticks"),
        ("@fold", "module-level: fold constant expressions"),
        ("@nofold", "module-level: never fold"),
        ("@layout", "module-level: placement engine (code/cube)"),
        ("@flat", "module-level: inline every chip onto one grid"),
    ] {
        items.push(Completion::new(ann, CompletionKind::Keyword).detail(detail));
    }

    // Built-in events (RoundStart, ChatCommand, CharacterSpawned, ...).
    for (name, evt) in events() {
        let params: Vec<&str> = evt.data.iter().map(|d| d.name).collect();
        items.push(Completion {
            detail: if params.is_empty() {
                None
            } else {
                Some(format!("({})", params.join(", ")))
            },
            ..Completion::new(*name, CompletionKind::Event)
        });
    }

    for (name, spec) in calls() {
        let params_str: Vec<String> = spec
            .params
            .iter()
            .map(|p| {
                if p.optional {
                    format!("{}?", p.name)
                } else {
                    p.name.to_string()
                }
            })
            .collect();
        items.push(
            Completion::new(*name, CompletionKind::Function)
                .detail(format!("({})", params_str.join(", "))),
        );
    }

    // Callable gate builtins (`GetMapElement`, `PushToArray`, `SetVariable`)
    // desugar to method and assignment forms, so they are not in `calls()`.
    for name in crate::catalog::gate_builtins::all() {
        items.push(Completion::new(name, CompletionKind::Function).detail("gate builtin"));
    }

    for ty in &[
        "int",
        "float",
        "bool",
        "string",
        "entity",
        "controller",
        "character",
        "vector",
        "rotator",
        "color",
        "brick",
        "exec",
    ] {
        items.push(Completion::new(*ty, CompletionKind::Class));
    }

    // Built-in game enum type names (`EasingFunction` and friends). Unlike a
    // user-declared `enum`, these have no `TopDecl::Enum` and so never appear
    // in `symbols`; offer them here so `var e: <here>` completes them the
    // same as any hardcoded scalar type above.
    for def in game_enum_defs() {
        items.push(
            Completion::new(def.name.clone(), CompletionKind::Class)
                .detail("game enum")
                .insert_text(def.name),
        );
    }

    items
}

#[cfg(test)]
mod tests;
