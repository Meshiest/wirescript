//! Completion: what may be typed at a position. Each editor front end maps
//! these items to its own completion type.

use super::{
    CollectionKind, SymbolDef, char_col_to_byte, collection_kind, cursor_byte_offset,
    find_enclosing_call, member_receiver_at, named_arg_value, param_names, receiver_methods,
    record_field_names, resolve_symbol, swizzle_fields, type_str, user_receiver_methods,
};
use crate::catalog::arrays::ARRAY_METHODS;
use crate::catalog::calls::calls;
use crate::catalog::events::events;
use crate::catalog::maps::MAP_METHODS;
use crate::lexer::KEYWORDS;

/// How an editor finds the enum named by the text it completes. Only an
/// editor's build has one, so the engine is handed it.
pub type EnumLookup = dyn for<'r> Fn(
    &'r crate::typecheck::enums::EnumRegistry,
    &str,
) -> Option<&'r crate::typecheck::enums::EnumDef>;

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

/// Completions for `receiver.` — array methods, var fields, record fields, or
/// the receiver methods valid for a typed value. Returns only the members of the
/// receiver (possibly empty); it never falls through to the global
/// keyword/function list, so e.g. a `string` receiver shows only string methods.
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
        .and_then(crate::analysis::component_member_completions)
    {
        return found
            .into_iter()
            .map(|(label, detail)| Completion {
                kind: if label == "Remove" {
                    CompletionKind::Method
                } else {
                    CompletionKind::Property
                },
                label,
                detail: Some(detail),
                ..Default::default()
            })
            .collect();
    }

    let enum_registry = enum_registry_from_source(source);

    // `arr[i].` — an indexed read, not the array itself. Its members are the
    // array-get gate's outputs (the element `Value` and the `OutOfBounds`
    // flag), never the array's methods.
    if let Some(base) = var_name.strip_suffix("[]") {
        let sym = resolve_symbol(symbols, source, base, line, col);
        let elem = sym
            .and_then(|s| s.ty.as_deref())
            .and_then(|t| t.strip_suffix("[]"))
            .unwrap_or("");
        for (name, ty) in [("Value", elem), ("OutOfBounds", "bool")] {
            items.push(Completion {
                label: name.to_string(),
                kind: CompletionKind::Field,
                detail: Some(if ty.is_empty() {
                    "field".to_string()
                } else {
                    format!("{name}: {ty}")
                }),
                insert_text: Some(name.to_string()),
                ..Default::default()
            });
        }
        return items;
    }

    let sym = resolve_symbol(symbols, source, var_name, line, col);

    // Field name (record field / swizzle component) completion item. Declared
    // ahead of the bare enum-type receiver check below so that check's bare-
    // variant fallback can also reach `push_type_members` (which closes over
    // `field_item`).
    let field_item = |name: String| Completion {
        label: name.clone(),
        kind: CompletionKind::Field,
        detail: Some("field".to_string()),
        insert_text: Some(name),
        ..Default::default()
    };
    // Method + swizzle members valid for a typed value: an enum-typed value's
    // `.Discriminant`, swizzle fields, builtin receiver-methods, then
    // in-scope user `self`-mods whose receiver matches.
    let push_type_members = |ty: &str, items: &mut Vec<Completion>| {
        if lookup(&enum_registry, ty).is_some() {
            items.push(Completion {
                label: "Discriminant".to_string(),
                kind: CompletionKind::Field,
                detail: Some(format!("{ty} discriminant (int)")),
                insert_text: Some("Discriminant".to_string()),
                ..Default::default()
            });
        }
        for f in swizzle_fields(ty) {
            items.push(field_item(f.to_string()));
        }
        for (name, sig) in receiver_methods(ty) {
            items.push(Completion {
                label: name.to_string(),
                kind: CompletionKind::Method,
                detail: Some(sig),
                ..Default::default()
            });
        }
        for (name, sig) in user_receiver_methods(ty, symbols) {
            items.push(Completion {
                label: name.clone(),
                kind: CompletionKind::Method,
                detail: Some(format!("{name}{sig}")),
                insert_text: Some(name),
                ..Default::default()
            });
        }
    };

    // Bare enum-type receiver (`Shape.<here>`): a variant PATH ready for
    // construction or `.Discriminant`, not a value. Only when the name is NOT
    // shadowed by a value binding: `collect_symbols_for_file` emits no symbol
    // for an `enum` decl itself, so an unshadowed enum name resolves to `None`
    // (or, defensively, a `type` symbol), whereas a `var`/`let`/param/mod of
    // the same name resolves to that value here and must win. This mirrors the
    // compiler's own shadow guard (`infer.rs`'s
    // `resolve_variant_for_construction`: a value symbol whose name equals the
    // enum's shadows the type, so the name is NOT a construction site).
    if sym.is_none_or(|s| s.kind == "type") {
        if let Some(def) = lookup(&enum_registry, var_name) {
            push_variant_completions(&mut items, def);
            return items;
        }
        // A bare variant name used as a value receiver (`EasingFunction.
        // Bounce.`, or any `Enum.Variant.` chain) arrives here as just
        // `Variant`, because `member_receiver_at` only reports the identifier
        // directly before the dot, discarding the `Enum.` qualifier. Resolve
        // it the same way the compiler resolves a bare variant
        // (`resolve_bare_variant_enum`): on a UNIQUE owning enum, treat the
        // receiver as a value of that enum type, offering `.Discriminant`
        // (via `push_type_members`) exactly as a directly-typed value would.
        // An ambiguous or unknown bare name yields no owner and falls through
        // unchanged.
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

    // Collection methods come from the receiver's declared type (resolved through
    // type aliases): a `Map<K, V>` gets the map table, `T[]` or an `array` decl
    // the array table. The tables are distinct — a map's `length`/`clear`/
    // `copyFrom` are its own, and `get`/`set`/`has`/`keys`/`values` exist on no
    // array — so this dispatches on type, never on the bare method name.
    let collection = sym.and_then(|s| {
        s.ty.as_deref()
            .and_then(|ty| collection_kind(ty, symbols))
            .or_else(|| (s.kind == "array").then_some(CollectionKind::Array))
    });
    if let Some(kind) = collection {
        let method_item = |name: &str, signature: &str, doc: &str| Completion {
            label: name.to_string(),
            kind: CompletionKind::Method,
            detail: Some(format!("{name}{signature}")),
            documentation: Some(doc.to_string()),
            ..Default::default()
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
                items.push(Completion {
                    label: member.to_string(),
                    kind: namespace_member_kind(m.kind),
                    insert_text: Some(member.to_string()),
                    ..Default::default()
                });
            }
        }
        return items;
    }

    // Vars (mutable `var` / `static var`) expose `.Value`/`.prev`, plus any
    // method/swizzle valid for the var's element type (`pos.Normalize()`,
    // `pos.x`).
    if sym.is_some_and(|s| matches!(s.kind, "var" | "static var")) {
        for (name, detail) in &crate::catalog::VAR_PSEUDO_FIELDS {
            items.push(Completion {
                label: name.to_string(),
                kind: CompletionKind::Field,
                detail: Some(detail.to_string()),
                insert_text: Some(name.to_string()),
                ..Default::default()
            });
        }
        if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
            push_type_members(ty, &mut items);
        }
        return items;
    }

    // Record-typed value (e.g. `let split = pl.InputReader()` -> {Forward, Right,
    // Jump}, or a multi-output mod result): offer the record's field names, so
    // `split.<here>` / `on split.<here>` completes `Forward`/`Right`/`Jump`. The
    // type may be an inline `{…}` string or a named `type` alias resolved here.
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

/// Record field names for a receiver type string: an inline `{…}` record, or a
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

/// The `enum` declarations visible in `source`, keyed by name, plus the
/// built-in `Option`/`Result` prelude - reparsed on demand so enum-aware
/// completions don't require threading a cached AST/type-checked document
/// through every completion path. Gated by a fast substring check: a file
/// with no `enum ` keyword at all cannot contain a `TopDecl::Enum`, so
/// skipping the reparse in that case returns the exact same registry
/// (`build_registry` only reads `TopDecl::Enum` entries out of `decls`) that
/// a full parse would, at zero cost for the common no-enum file.
fn enum_registry_from_source(source: &str) -> crate::typecheck::enums::EnumRegistry {
    if !source.contains("enum ") {
        return crate::typecheck::enums::build_registry(&[]);
    }
    let ast = crate::on_big_stack(|| crate::parse(source, "completion")).ast;
    crate::typecheck::enums::build_registry(&ast.decls)
}

/// Push a completion item for each variant of `def` - the user-enum analog of
/// `push_enum_member_completions` below, used both for `match`-arm-head
/// pattern completion and for a bare `Enum.<here>` variant-path receiver.
fn push_variant_completions(items: &mut Vec<Completion>, def: &crate::typecheck::enums::EnumDef) {
    for v in &def.variants {
        items.push(Completion {
            label: v.name.clone(),
            kind: CompletionKind::EnumMember,
            detail: Some(format!("{} variant", def.name)),
            insert_text: Some(v.name.clone()),
            ..Default::default()
        });
    }
}

/// If the cursor sits in a `match <scrutinee> { <here> }` arm-head position -
/// directly inside the match's own arm-list braces, at or past the last arm
/// separator (a `,`, or a comma-less block arm's closing `}`, or the opening
/// brace) and before any `=>` of an in-progress arm - return the scrutinee's
/// raw source text (trimmed). `None` inside a pattern's own parens
/// (`Circle(<here>)`), inside an arm's body (`Empty => { <here> }` or past
/// its `=>`), or outside any match at all.
///
/// Scans the whole prefix up to the cursor once, like `find_enclosing_call`
/// above, skipping strings/comments and tracking brace/paren/bracket nesting
/// with a small frame stack. Only the innermost `{ }` opened directly by a
/// `match <expr>` is tagged `MatchArms`; every other brace/paren/bracket is
/// an opaque `Other` frame, except the block body of an arm (a `{` whose arm
/// is `AwaitingBody`, the state right after `=>`), which is tagged `ArmBlock`
/// so that popping it returns the enclosing arm to `Head`. That mirrors the
/// parser: a block-bodied arm may omit the trailing comma
/// (`parser/expr.rs`), so the cursor after a comma-less `Empty => { .. }` is
/// still an arm head. A scrutinee expression that itself contains a raw `{`
/// (a record literal, e.g. `match f(){x:1} { ... }`) is not handled - the
/// `{` would be mistaken for the arms brace - but that shape is rare enough
/// not to be worth the extra bookkeeping here.
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
        items.push(Completion {
            label: v.clone(),
            kind: CompletionKind::EnumMember,
            detail: Some(format!("{et} member")),
            insert_text: Some(v),
            filter_text: Some(filter.clone()),
            ..Default::default()
        });
    }
}

/// Push a QUALIFIED completion item (`EasingFunction.Bounce`) for each variant
/// of the built-in game enum backed by schema enum `et`, if any, alongside
/// [`push_enum_member_completions`]'s bare-name items, so a config value slot
/// resolves to a schema enum offers both the bare member and the qualified
/// form an author would use to construct the value directly. `et` is the raw
/// schema type (`config_enum_for_named_arg`'s return); at most one built-in
/// game enum maps to it, found via [`crate::catalog::game_enum_schema_type`].
/// A no-op for a schema enum with no built-in game-enum counterpart.
fn push_qualified_builtin_game_enum_variant_completions(
    items: &mut Vec<Completion>,
    et: &str,
    value_so_far: &str,
) {
    let filter = value_so_far.trim().to_string();
    let Some(def) = crate::typecheck::enums::game_enum_defs()
        .into_iter()
        .find(|def| crate::catalog::game_enum_schema_type(&def.name) == Some(et))
    else {
        return;
    };
    for v in &def.variants {
        let label = format!("{}.{}", def.name, v.name);
        items.push(Completion {
            label: label.clone(),
            kind: CompletionKind::EnumMember,
            detail: Some(format!("{} variant", def.name)),
            insert_text: Some(label),
            filter_text: Some(filter.clone()),
            ..Default::default()
        });
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
    // text edit over the whole `$…` fragment keeps `.`/`/` filtering robust.
    if let Some((dollar, frag)) = prefab_path_fragment(source, line, col) {
        for path in prefab_paths {
            if path.starts_with(frag) {
                items.push(Completion {
                    label: path.clone(),
                    kind: CompletionKind::File,
                    replace_from: Some(dollar + 1),
                    ..Default::default()
                });
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
                    for name in crate::analysis::asset_names(&frag[..slash]) {
                        items.push(Completion {
                            label: name.to_string(),
                            kind: CompletionKind::Constant,
                            ..Default::default()
                        });
                    }
                } else {
                    for ty in crate::analysis::asset_types() {
                        items.push(Completion {
                            label: ty.to_string(),
                            kind: CompletionKind::Class,
                            insert_text: Some(format!("{ty}/")),
                            ..Default::default()
                        });
                    }
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // Member access `receiver.partial` — return only the receiver's members.
    // Checked before call-param completion so `Call(arg = recv.<here>` shows
    // recv's methods, not the enclosing call's params. A plain `Call(` (the dot
    // belongs to the callee, cursor at an arg boundary) yields no receiver here
    // and falls through to param completion below.
    // `Environment.<env>.<property>`: environment names, then properties.
    let env_items = if symbols.iter().any(|s| s.name == "Environment") {
        None
    } else {
        crate::analysis::environment_completions(source, line, col)
    };
    if let Some(items) = env_items {
        return items
            .into_iter()
            .map(|(label, detail)| Completion {
                label,
                kind: CompletionKind::Property,
                detail: Some(detail),
                ..Default::default()
            })
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
                // Enum config value — works for both a hand-coded param
                // (`justify = …`) and a raw config field (`Justification = …`),
                // resolved through the unified call+event helper.
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
                    // Asset-ref config param (`font = <here>`, `weapon = <here>`):
                    // offer full `$Type/Name` refs for the param's asset type, so
                    // the author needn't know the type name. (Once they type `$`,
                    // the `$Type/` block above takes over.) Constant-only params
                    // only — a wire-input Entity port takes a live value.
                    if !crate::catalog::is_wire_input(spec.gate_class, param.port.as_str())
                        && let Some(asset_ty) =
                            crate::analysis::asset_type_for_port(param.port.as_str())
                    {
                        for name in crate::analysis::asset_names(asset_ty) {
                            let full = format!("${asset_ty}/{name}");
                            items.push(Completion {
                                label: full.clone(),
                                kind: CompletionKind::Constant,
                                detail: Some(format!("{asset_ty} asset")),
                                insert_text: Some(full),
                                ..Default::default()
                            });
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
                        items.push(Completion {
                            label: format!("{} = ", p.name),
                            kind: CompletionKind::Field,
                            detail: Some(format!("{ty_label} (optional)")),
                            insert_text: Some(format!("{} = ", p.name)),
                            ..Default::default()
                        });
                    } else {
                        items.push(Completion {
                            label: p.name.to_string(),
                            kind: CompletionKind::Field,
                            detail: Some(format!("{ty_label} (required)")),
                            ..Default::default()
                        });
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
                    items.push(Completion {
                        label: format!("{} = ", cfg.name),
                        kind: CompletionKind::Field,
                        detail: Some(format!("{ty_label} (config)")),
                        insert_text: Some(format!("{} = ", cfg.name)),
                        ..Default::default()
                    });
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
                    items.push(Completion {
                        label: format!("{surf} = "),
                        kind: CompletionKind::Field,
                        detail: Some("wired input".to_string()),
                        insert_text: Some(format!("{surf} = ")),
                        ..Default::default()
                    });
                }
                for (surf, field) in &evt.config_named {
                    let ty_label = crate::catalog::config_field_enum_type(evt.gate_class, field)
                        .map(str::to_string)
                        .unwrap_or_else(|| "config".to_string());
                    items.push(Completion {
                        label: format!("{surf} = "),
                        kind: CompletionKind::Field,
                        detail: Some(format!("{ty_label} (config)")),
                        insert_text: Some(format!("{surf} = ")),
                        ..Default::default()
                    });
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
                    items.push(Completion {
                        label: name.clone(),
                        kind: CompletionKind::Field,
                        insert_text: Some(name),
                        ..Default::default()
                    });
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // User symbols. Qualified namespace members (`u.member`) are addressed only
    // through `u.` member completion, so keep them out of the bare-identifier list.
    // Dedupe by name: when a name is declared in several scopes, offer only the
    // one in scope at the cursor (nearest enclosing/preceding declaration), so a
    // handler-local `players: character[]` doesn't leak into file scope where a
    // different `players` is visible. Forward references to top-level symbols
    // still appear (`resolve_symbol` falls back to the first declaration).
    // Precompute the nearest-preceding (and first-seen) declaration per name in
    // ONE pass over the symbol table, then resolve each unique name by O(1)
    // lookup below. `resolve_symbol` re-scans every symbol per call, so calling
    // it once per unique name made this loop O(n²). This mirrors its rule: the
    // nearest declaration at/before the cursor wins, else the first declaration.
    let (cl, cc) = ((line + 1) as u32, (col + 1) as u32);
    let mut first: crate::collections::HashMap<&str, &SymbolDef> =
        crate::collections::HashMap::default();
    let mut best: crate::collections::HashMap<&str, (&SymbolDef, (u32, u32))> =
        crate::collections::HashMap::default();
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
            label: chosen.name.clone(),
            kind,
            detail: chosen.ty.clone(),
            ..Default::default()
        });
    }

    for kw in KEYWORDS {
        items.push(Completion {
            label: kw.to_string(),
            kind: CompletionKind::Keyword,
            ..Default::default()
        });
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
        items.push(Completion {
            label: ann.to_string(),
            kind: CompletionKind::Keyword,
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }

    // Built-in events (RoundStart, ChatCommand, CharacterSpawned, ...).
    for (name, evt) in events().iter() {
        let params: Vec<&str> = evt.data.iter().map(|d| d.name).collect();
        items.push(Completion {
            label: name.to_string(),
            kind: CompletionKind::Event,
            detail: if params.is_empty() {
                None
            } else {
                Some(format!("({})", params.join(", ")))
            },
            ..Default::default()
        });
    }

    for (name, spec) in calls().iter() {
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
        items.push(Completion {
            label: name.to_string(),
            kind: CompletionKind::Function,
            detail: Some(format!("({})", params_str.join(", "))),
            ..Default::default()
        });
    }

    // Callable gate builtins (GetMapElement, PushToArray, SetVariable, …) — they
    // desugar to the method/assignment forms, so they aren't in `calls()`.
    for name in crate::catalog::gate_builtins::all() {
        items.push(Completion {
            label: name.to_string(),
            kind: CompletionKind::Function,
            detail: Some("gate builtin".to_string()),
            ..Default::default()
        });
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
        items.push(Completion {
            label: ty.to_string(),
            kind: CompletionKind::Class,
            ..Default::default()
        });
    }

    // Built-in game enum type names (`EasingFunction` and friends). Unlike a
    // user-declared `enum`, these have no `TopDecl::Enum` and so never appear
    // in `symbols`; offer them here so `var e: <here>` completes them the
    // same as any hardcoded scalar type above.
    for def in crate::typecheck::enums::game_enum_defs() {
        items.push(Completion {
            label: def.name.clone(),
            kind: CompletionKind::Class,
            detail: Some("game enum".to_string()),
            insert_text: Some(def.name),
            ..Default::default()
        });
    }

    items
}

#[cfg(test)]
mod tests;
