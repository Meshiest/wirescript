use crate::collections::HashMap;
use crate::ast::{CallArg, Expr, Handler, HandlerConfigArg, Script, Trigger};
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;
use crate::ir::Type;
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
        .or_else(|| resolve_field_hover(source, file, type_map, symbols, line, col, &word))
        .or_else(|| hover_user_symbol(source, file, symbols, doc_comments, var_read_contexts, resource_estimates, &word, line, col))
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
        "prefab" => "Reference to a prefab (a `$./file.brz` file or an inline prefab block) — a compile-time constant, not stored.",
        "any" => "Wildcard type — works anywhere but erases the type; prefer a generic `<T>`.",
        "never" => "Bottom type — no value inhabits it.",
        _ => return None,
    })
}

/// Hover for a bare type word: a generic **constraint class** (`Scalar` /
/// `Numeric` / `Variant`), or a built-in primitive type (`int`, `vector`, …).
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
        out += "\nNote: prefab references must end in `.brz` (WS019).\n";
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
/// non-scalar (entity/vector/…) with no displayable constant default.
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
/// sub-fields (`X`/`Y`/`Z`, `R`/`G`/`B`/`A`, `Pitch`/`Yaw`/`Roll`, …) through the
/// `AsBrdbValue` struct-property accessor. Colors are stored LINEAR and shown as
/// their sRGB hex (`#181425`); vectors/rotators show a `Vec(…)`/`Rotation(…)`
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
        // the symbol table doesn't carry): preserve the pre-type-aware behavior of
        // matching array method names only.
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
/// carries data, the `-> (…)` tuple capture that binds it.
fn hover_builtin_event(word: &str) -> Option<String> {
    let evt = find_event(word)?;
    // Config/inputs are the only things allowed inside the call parens now;
    // event data outputs are bound via the trailing `-> (…)` tuple capture.
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
    // e.g. `on CustomEvent("name") -> (data1: any, …)`.
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
/// `e.SendCustomEvent(…)`). Resolves the channel's data slots (names + types)
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
        // name); the data slots bind via the `-> (…)` tuple capture.
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
/// form `e.SendCustomEvent(…)`.
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
            format!("```wirescript\n{} {}{}\n```", sym.kind, sym.name, sig)
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
mod tests;
