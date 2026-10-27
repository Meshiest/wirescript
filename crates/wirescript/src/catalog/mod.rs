//! Gate catalog: the registry of every wire-graph brick the compiler knows
//! about. The inventory JSON is baked into the binary via `include_str!`.

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

/// The schema enum backing named arg `arg` of builtin call or event `callee`,
/// when `arg`'s gate data field is a schema enum (wireable params such as
/// `DisplayText`'s `justify` included). Hover and completion use it for calls
/// and events alike. `None` for unknown args or non-enum fields.
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

/// Every schema enum type reachable from a gate's, event's or property's config
/// surface, resolved as [`config_enum_for_named_arg`] resolves one arg. These
/// are the schema enums that become built-in enums. Sorted and de-duplicated.
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
    for p in map
        .environments
        .iter()
        .flat_map(|e| &e.properties)
        .chain(map.components.iter().flat_map(|c| &c.properties))
    {
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
/// is the brdb schema enum name that resolves its members and values.
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
/// Strips a leading `E`, then a leading `BR` or `Brick`, and keeps the rest.
/// Longer prefixes (`DisplayText`, `Text`, `Easing`) stay: stripping them
/// would collide names (`EasingDirection` onto `Direction`).
pub fn clean_game_enum_type(raw: &str) -> String {
    let s = raw.strip_prefix('E').unwrap_or(raw);
    let s = s
        .strip_prefix("BR")
        .or_else(|| s.strip_prefix("Brick"))
        .unwrap_or(s);
    s.to_string()
}

/// Clean a bare schema enum member name (e.g. `X_Positive`) into the
/// Wirescript-facing variant name (`XPositive`).
///
/// Splits on `_`, capitalizes each segment's first character and joins them,
/// without reordering (`X_Positive` is `XPositive`, not `PositiveX`).
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

/// `Type`, what `GetValueType` and `.getType()` give: the schema's `EWireKind`,
/// cut to the kinds a wire value can hold (its first 13 members, the save's
/// `WireValue` variants) and named as Wirescript names its types. Its members
/// never resolve bare: `Int`, `String` and `Color` would collide with too much.
const WIRE_KIND_ENUM: &str = "Type";
/// The members of [`WIRE_KIND_ENUM`]: each pair is the schema member and the
/// Wirescript name.
const WIRE_KIND_MEMBERS: &[(&str, &str)] = &[
    ("Bool", "Bool"),
    ("Integer", "Int"),
    ("Double", "Float"),
    ("Color", "Color"),
    ("Vector", "Vector"),
    ("Rotator", "Rotator"),
    ("Quat", "Quat"),
    ("String", "String"),
    ("Enum", "Enum"),
    ("Object", "Object"),
    ("Brick", "Brick"),
    ("Component", "Component"),
    ("Exec", "Exec"),
];

/// Whether a bare member name of built-in game enum `clean_name` resolves on
/// its own, anywhere. An [`ANNOTATION_ENUMS`] member is bare only inside its
/// annotation's argument: `BrickMaterial` shares `Plastic`, `Glass`,
/// `TranslucentPlastic` and `Glow` with `TextMaterial`, and claiming them
/// everywhere would turn those bare names, which already resolve, ambiguous.
pub fn resolves_bare_members(clean_name: &str) -> bool {
    clean_name != WIRE_KIND_ENUM && !ANNOTATION_ENUMS.iter().any(|(_, name)| *name == clean_name)
}

/// Every built-in game enum: [`config_referenced_enum_types`], the
/// [`ANNOTATION_ENUMS`] and the wire-kind `Type`, with each member's real schema
/// discriminant. Sorted by `clean_name`; an enum with no non-sentinel members is
/// skipped.
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
                        let disc =
                            enum_member_value(schema_type, &raw_member).expect("known member");
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
        enums.push(GameEnum {
            clean_name: WIRE_KIND_ENUM.to_string(),
            schema_type: "EWireKind",
            variants: WIRE_KIND_MEMBERS
                .iter()
                .map(|(raw, clean)| GameEnumVariant {
                    clean_name: clean.to_string(),
                    raw_member: raw.to_string(),
                    disc: enum_member_value("EWireKind", raw).expect("EWireKind member"),
                })
                .collect(),
        });
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
/// Case-insensitive, matching typecheck, which accepts every capitalisation. A
/// spelling this misses drops an `on v.Prev { ... }` handler with no diagnostic.
pub fn is_var_pseudo_field(field: &str) -> bool {
    VAR_PSEUDO_FIELDS
        .iter()
        .any(|(name, _)| field.eq_ignore_ascii_case(name))
}
