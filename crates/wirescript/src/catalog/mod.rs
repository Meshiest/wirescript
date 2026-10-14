//! Gate catalog — the authoritative registry of every wire-graph brick
//! the compiler knows about. The inventory JSON is baked into the binary
//! via `include_str!`.

pub mod arrays;
pub mod calls;
pub mod events;
pub mod gate_builtins;
pub mod maps;
pub mod methods;
pub mod operators;
pub mod storage;
pub mod value_fields;

pub use wirescript_ir::catalog::*;

/// The schema enum backing a named-arg `arg` on a builtin call OR event
/// `callee` — if `arg` is a param whose gate data field is a schema enum.
/// That includes a wireable param such as DisplayText's `justify`, whose
/// constant member names bake into the same field. Unifies the call path
/// (`CallSpec` params -> `config_field_enum_type`) with the event path
/// (`EventSpec.config_named` -> its gate field), so hover and completion
/// resolve `SweepSimple(direction = …)` and `on Clock(…)` the same way.
/// `None` for unknown args or non-enum fields.
pub fn config_enum_for_named_arg(callee: &str, arg: &str) -> Option<&'static str> {
    if let Some(spec) = crate::catalog::calls::calls().get(callee) {
        if let Some(p) = spec.params.iter().find(|p| p.name == arg) {
            return config_field_enum_type(spec.gate_class, p.port.as_str());
        }
        // Not a declared param: a data-driven config field set by its raw name.
        if scalar_config_field(spec.gate_class, arg).is_some() {
            return config_field_enum_type(spec.gate_class, arg);
        }
        return None;
    }
    if let Some(evt) = crate::catalog::events::find_event(callee) {
        let key = arg.to_ascii_lowercase();
        let field = evt
            .config_named
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, f)| *f)?;
        return config_field_enum_type(evt.gate_class, field);
    }
    None
}

/// Every schema enum type reachable from a gate or event's config surface
/// (call-spec params backed by an enum field, every gate's `scalar_config_fields`, and event
/// `config_positional`/`config_named` fields), resolved through
/// [`config_field_enum_type`] the same way [`config_enum_for_named_arg`]
/// resolves a single named arg. This is the single source of truth for which
/// schema enums become Wirescript built-in enums: nothing is hand-listed,
/// every entry is derived from the catalog + schema. Sorted and
/// de-duplicated.
pub fn config_referenced_enum_types() -> Vec<&'static str> {
    use std::collections::BTreeSet;

    let mut types: BTreeSet<&'static str> = BTreeSet::new();

    // Call specs: params backed by an enum data field, wireable or not, same
    // resolution rule as `config_enum_for_named_arg`'s call path.
    for spec in calls::calls().values() {
        for p in &spec.params {
            if let Some(ty) = config_field_enum_type(spec.gate_class, p.port.as_str()) {
                types.insert(ty);
            }
        }
    }

    // Every gate's full data-driven config surface (`<Field> = value`
    // settings), not just the fields a call spec happens to expose.
    for gate in default_catalog().entries() {
        for prop in scalar_config_fields(&gate.component.class) {
            if let Some(ty) = config_field_enum_type(&gate.component.class, &prop.name) {
                types.insert(ty);
            }
        }
    }

    // Enum properties of environments and components (`ReverbEffect`), so a
    // write can name a member (`Environment.Ambience.ReverbEffect = ReverbEffect.Cave`).
    let map = properties::property_map();
    for p in map.environments.iter().flat_map(|e| &e.properties).chain(map.components.iter().flat_map(|c| &c.properties)) {
        if let Some(e) = &p.enum_type
            && !enum_member_names(e).is_empty()
        {
            types.insert(e.as_str());
        }
    }

    // Event config args (positional + named), same resolution rule as
    // `config_enum_for_named_arg`'s event path.
    for evt in events::events().values() {
        for field in &evt.config_positional {
            if let Some(ty) = config_field_enum_type(evt.gate_class, field) {
                types.insert(ty);
            }
        }
        for (_, field) in &evt.config_named {
            if let Some(ty) = config_field_enum_type(evt.gate_class, field) {
                types.insert(ty);
            }
        }
    }

    types.into_iter().collect()
}

/// A built-in game enum, derived from a schema enum reachable from the
/// catalog's config surface (see [`config_referenced_enum_types`]). The
/// Wirescript-facing name is [`GameEnum::clean_name`]; [`GameEnum::schema_type`]
/// is the underlying brdb schema enum name used to resolve members/values.
#[derive(Clone, Debug)]
pub struct GameEnum {
    pub clean_name: String,
    pub schema_type: &'static str,
    pub variants: Vec<GameEnumVariant>,
}

/// One member of a [`GameEnum`]. `raw_member` is the schema's bare member
/// name (as accepted by [`enum_member_value`]); `disc` is its real integer
/// discriminant, read straight from the schema (never renumbered).
#[derive(Clone, Debug)]
pub struct GameEnumVariant {
    pub clean_name: String,
    pub raw_member: String,
    pub disc: i64,
}

/// Clean a raw schema enum type name (e.g. `EBREasingFunction`) into the
/// Wirescript-facing type name (`EasingFunction`).
///
/// Rule: strip a leading `E`, then strip a leading `BR` or `Brick` from what
/// remains, keeping the rest verbatim. Deliberately does NOT also strip
/// `DisplayText` / `Text` / `Easing`. Those verbose segments are what keep
/// the cleaned set collision-free (e.g. `EasingDirection` must not collapse
/// onto `Direction`).
pub fn clean_game_enum_type(raw: &str) -> String {
    let s = raw.strip_prefix('E').unwrap_or(raw);
    let s = s.strip_prefix("BR").or_else(|| s.strip_prefix("Brick")).unwrap_or(s);
    s.to_string()
}

/// Clean a bare schema enum member name (e.g. `X_Positive`) into the
/// Wirescript-facing variant name (`XPositive`).
///
/// Rule: split on `_`, capitalize each segment's first character, join with
/// no separator. No reordering, so `X_Positive` stays `XPositive`, not
/// `PositiveX`.
pub fn clean_game_enum_variant(bare: &str) -> String {
    bare.split('_')
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Schema enums that are built-in game enums because a compiler annotation
/// takes one, not a gate's config, each with the name Wirescript spells it.
/// `@material` takes an `EBrickMaterial`, which the cleaning rule would name
/// the bare `Material`, too generic a type name to claim.
const ANNOTATION_ENUMS: &[(&str, &str)] = &[("EBrickMaterial", "BrickMaterial")];

/// Whether a bare member name of built-in game enum `clean_name` resolves on
/// its own, anywhere. An [`ANNOTATION_ENUMS`] member is bare only inside its
/// annotation's argument: `BrickMaterial` shares `Plastic`, `Glass`,
/// `TranslucentPlastic` and `Glow` with `TextMaterial`, and claiming them
/// everywhere would turn those bare names, which already resolve, ambiguous.
pub fn resolves_bare_members(clean_name: &str) -> bool {
    !ANNOTATION_ENUMS.iter().any(|(_, name)| *name == clean_name)
}

/// Every built-in game enum, derived from [`config_referenced_enum_types`]
/// plus the [`ANNOTATION_ENUMS`]:
/// each schema enum type is cleaned into a [`GameEnum`] whose variants come
/// from [`enum_member_names`] (already sentinel-filtered), cleaned via
/// [`clean_game_enum_variant`] and paired with their real schema
/// discriminant via [`enum_member_value`]. Sorted by `clean_name`. A schema
/// enum with zero non-sentinel members is skipped rather than panicking.
/// Memoized in the same table [`game_enum_schema_type`] reads.
pub fn builtin_game_enums() -> Vec<GameEnum> {
    game_enum_table().clone()
}

/// The schema enum type backing built-in game enum `clean_name`, if any.
pub fn game_enum_schema_type(clean_name: &str) -> Option<&'static str> {
    game_enum_table()
        .iter()
        .find(|e| e.clean_name == clean_name)
        .map(|e| e.schema_type)
}

fn game_enum_table() -> &'static Vec<GameEnum> {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Vec<GameEnum>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut schema_types = config_referenced_enum_types();
        for (schema_type, _) in ANNOTATION_ENUMS {
            if !schema_types.contains(schema_type) {
                schema_types.push(schema_type);
            }
        }
        let mut enums: Vec<GameEnum> = schema_types
            .into_iter()
            .filter_map(|schema_type| {
                let variants: Vec<GameEnumVariant> = enum_member_names(schema_type)
                    .into_iter()
                    .map(|raw_member| {
                        let disc = enum_member_value(schema_type, &raw_member)
                            .expect("known member");
                        GameEnumVariant {
                            clean_name: clean_game_enum_variant(&raw_member),
                            raw_member,
                            disc,
                        }
                    })
                    .collect();
                if variants.is_empty() {
                    return None;
                }
                let clean_name = match ANNOTATION_ENUMS.iter().find(|(ty, _)| *ty == schema_type) {
                    Some((_, name)) => name.to_string(),
                    None => clean_game_enum_type(schema_type),
                };
                Some(GameEnum {
                    clean_name,
                    schema_type,
                    variants,
                })
            })
            .collect();
        enums.sort_by(|a, b| a.clean_name.cmp(&b.clean_name));
        enums
    })
}

#[cfg(test)]
mod tests;

/// The pseudo-fields every `var` exposes, in the spelling the editor offers.
/// Both read the storage gate's `Value` port; `prev` reads it as it stood at
/// the start of the tick.
pub const VAR_PSEUDO_FIELDS: [(&str, &str); 2] = [
    ("Value", "Read current value (pure)"),
    ("prev", "Read previous tick's value"),
];

/// Whether `field` names one of [`VAR_PSEUDO_FIELDS`].
///
/// Case-insensitive, because typecheck accepts every capitalisation and the
/// docs teach `.Value` capitalised while `.prev` is lowercase. A spelling this
/// misses does not degrade gracefully: `on v.Prev { … }` matched no arm and
/// the whole handler was dropped with no diagnostic at all.
pub fn is_var_pseudo_field(field: &str) -> bool {
    VAR_PSEUDO_FIELDS
        .iter()
        .any(|(name, _)| field.eq_ignore_ascii_case(name))
}
