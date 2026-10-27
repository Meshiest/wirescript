//! Component-data encoding: schema metadata in, brdb component out.

use std::collections::HashMap as StdMap;

use brdb::schema::as_brdb::BrdbArrayIter;
use brdb::schema::{BrdbInterned, BrdbSchema};
use brdb::{AsBrdbValue, BString, BrdbSchemaError, World, assets::LiteralComponent};

use crate::catalog::enum_has_value;
use crate::collections::{HashMap, HashSet};
use crate::diagnostic::{Diagnostic, SourceRange};
use crate::intern::{Sym, intern_static, sym};
use crate::ir::{GateIO, Literal, Module, NodeId, NodeKind, Type, gate_class as gc};

use super::options::{EmitError, NestedCompiler, PrefabResolver};
use super::schema_meta::{FieldKind, gate_field_meta, resolve_enum_value};
use super::variants::{
    bool_input_bakes_as_int, coerce_bool_to_int, literal_to_boxed_native, literal_to_string,
    literal_to_wire_value_or_default, reshape_literal_for_struct, var_type_to_wire_value,
};

/// Build the gate's `LiteralComponent`. A literal inlined into a wire-input port (from a
/// `_Literal` source node) is embedded in the component data struct's `WireValue` field, so
/// the game reads it on load without a separate constant gate.
pub(super) fn build_gate_component(
    gate_class: &'static str,
    ports: &GateIO,
    inlined: &HashMap<Sym, &Literal>,
    world: &mut World,
    prefab_resolver: Option<&PrefabResolver>,
    nested_compiler: Option<&NestedCompiler>,
) -> Result<LiteralComponent, EmitError> {
    match build_gate_data_map(
        gate_class,
        ports,
        inlined,
        world,
        prefab_resolver,
        nested_compiler,
    )? {
        Some(data) => Ok(LiteralComponent::new_from_data(
            gate_class,
            std::sync::Arc::new(data),
        )),
        // A dataless component: the type only, so the engine uses the brick
        // type's default data.
        None => Ok(LiteralComponent::new(gate_class)),
    }
}

/// Build the component data map (schema field name -> serializable value) for a
/// gate that has a data struct, or `None` for a dataless gate. Each field is
/// classified once via [`gate_field_meta`] and serialized from its inlined
/// literal, falling back to typed wire-variant defaults / `STRUCT_DEFAULTS` for
/// unset fields. Shared by [`build_gate_component`] and the advanced-inventory
/// composite builder.
fn build_gate_data_map(
    gate_class: &'static str,
    ports: &GateIO,
    inlined: &HashMap<Sym, &Literal>,
    world: &mut World,
    prefab_resolver: Option<&PrefabResolver>,
    nested_compiler: Option<&NestedCompiler>,
) -> Result<Option<StdMap<BString, Box<dyn AsBrdbValue>>>, EmitError> {
    // Always write the data struct when the gate type has one, even with no
    // inlined fields: every instance of a component type must carry matching
    // data or the reader misaligns.
    let Some(fields) = gate_field_meta(gate_class) else {
        return Ok(None);
    };
    let bool_as_int = bool_input_bakes_as_int(gate_class);
    let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    for fm in fields {
        let field = fm.name;
        let is_wire_value = fm.kind == FieldKind::WireValue;
        let val: Box<dyn AsBrdbValue> = match inlined.get(&fm.sym) {
            Some(lit) if is_wire_value => {
                let wv = literal_to_wire_value_or_default(lit, ports, field);
                Box::new(if bool_as_int {
                    coerce_bool_to_int(wv)
                } else {
                    wv
                })
            }
            Some(lit) => match fm.kind {
                FieldKind::AssetRef => {
                    // Asset-reference field (AudioDescriptor, Item and so on):
                    // register the `$Type/Name` in the world's external
                    // asset table and store the index.
                    use brdb::schema::BrdbValue;
                    let idx = match lit {
                        Literal::Asset {
                            asset_type,
                            asset_name,
                        } => Some(
                            world
                                .global_data
                                .external_asset_references
                                .insert_full((asset_type.clone(), asset_name.clone()))
                                .0,
                        ),
                        _ => None,
                    };
                    Box::new(BrdbValue::Asset(idx))
                }
                FieldKind::Str
                    if matches!(
                        lit,
                        Literal::PrefabRef { .. }
                            | Literal::NestedPrefab { .. }
                            | Literal::PrefabBytes(_)
                    ) =>
                {
                    Box::new(embed_prefab(lit, world, prefab_resolver, nested_compiler)?)
                }
                FieldKind::Enum(type_name) => {
                    match resolve_enum_value(type_name, &in_native_field(&fm.kind, lit)) {
                        Some(ev) => Box::new(ev),
                        None => literal_to_boxed_native(lit).ok_or_else(|| {
                            EmitError::UnrepresentableLiteral {
                                field: field.to_string(),
                                literal: (**lit).clone(),
                            }
                        })?,
                    }
                }
                FieldKind::Str => Box::new(literal_to_string(lit).ok_or_else(|| {
                    EmitError::UnrepresentableLiteral {
                        field: field.to_string(),
                        literal: (**lit).clone(),
                    }
                })?),
                // A native struct field takes one representation, so a rotation constant bound for a
                // quaternion field converts here (see `reshape_literal_for_struct`).
                FieldKind::NativeStruct(struct_ty) => {
                    let reshaped = reshape_literal_for_struct(struct_ty, lit);
                    let lit = reshaped.as_ref().unwrap_or(lit);
                    literal_to_boxed_native(lit).ok_or_else(|| {
                        EmitError::UnrepresentableLiteral {
                            field: field.to_string(),
                            literal: lit.clone(),
                        }
                    })?
                }
                _ => literal_to_boxed_native(&in_native_field(&fm.kind, lit)).ok_or_else(|| {
                    EmitError::UnrepresentableLiteral {
                        field: field.to_string(),
                        literal: (**lit).clone(),
                    }
                })?,
            },
            // No inlined value. A wire-typed field gets a default whose
            // `WireValue` variant matches the port type (component_db carries
            // no wire values). Native fields are omitted: the brdb writer fills
            // them from component_db's `STRUCT_DEFAULTS`, else a type-zero.
            None if is_wire_value => {
                // Convert's setting is a value of the type it converts to:
                // the variant stored in `TargetType` picks the output.
                let port_sym = if gate_class == gc::CONVERT && field == "TargetType" {
                    *sym::OUTPUT
                } else {
                    fm.sym
                };
                let port_ty = ports.find_port(port_sym).map(|p| &p.ty);
                let wv = var_type_to_wire_value(port_ty);
                Box::new(if bool_as_int {
                    coerce_bool_to_int(wv)
                } else {
                    wv
                })
            }
            None if field == "bChangeType" && sets_any_var(gate_class, ports) => Box::new(true),
            None => continue,
        };
        data.insert(BString::Static(field), val);
    }
    Ok(Some(data))
}

/// A Set Variable writing an `any` value. It needs Change Type on: without it
/// the game converts the value to the kind the var already holds.
pub(super) fn sets_any_var(gate_class: &str, ports: &GateIO) -> bool {
    gate_class == gc::VAR_SET
        && ports
            .inputs
            .iter()
            .any(|p| p.name == *sym::VALUE && p.ty == Type::Opaque)
}

/// A constant `lit` as a native scalar field of `kind` holds it: an integer,
/// float or bool constant for a field of another of those kinds converts as
/// the value would on a wire (a float truncates, a non-finite float is 0, a
/// number is `true` when non-zero), and any other constant is unchanged.
fn in_native_field<'l>(kind: &FieldKind, lit: &'l Literal) -> std::borrow::Cow<'l, Literal> {
    use std::borrow::Cow;
    let truncated = |f: f64| if f.is_finite() { f as i64 } else { 0 };
    match (kind, lit) {
        (FieldKind::Integer(_) | FieldKind::Enum(_), Literal::Float(f)) => {
            Cow::Owned(Literal::Int(truncated(*f)))
        }
        (FieldKind::Integer(_), Literal::Bool(b)) => Cow::Owned(Literal::Int(i64::from(*b))),
        (FieldKind::Real(_), Literal::Int(n)) => Cow::Owned(Literal::Float(*n as f64)),
        (FieldKind::Real(_), Literal::Bool(b)) => {
            Cow::Owned(Literal::Float(if *b { 1.0 } else { 0.0 }))
        }
        (FieldKind::Boolean, Literal::Int(n)) => Cow::Owned(Literal::Bool(*n != 0)),
        (FieldKind::Boolean, Literal::Float(f)) => Cow::Owned(Literal::Bool(*f != 0.0)),
        _ => Cow::Borrowed(lit),
    }
}

/// How a constant lands in a native setting field.
enum Landing {
    /// As written, or converted without changing its value (`2.0` into an
    /// integer field, `1` into a bool field, `true` into the number field of
    /// a setting whose pin takes a bool).
    Exact,
    /// Converted to a different value: a fraction dropped, a number made a
    /// bool, a bool made a number. Carries the value stored.
    Inexact(String),
    /// Not storable at all: outside the field's range, or no value of its
    /// enum. Carries why.
    Invalid(String),
}

/// How the constant `lit` lands in a field of `kind`. `bool_pin` is set when
/// the setting's pin takes a bool, so a bool stored as a number there is the
/// field's own encoding.
fn landing(kind: &FieldKind, lit: &Literal, bool_pin: bool) -> Landing {
    let int_range = |ty: &str| -> (i128, i128) {
        match ty {
            "i8" => (i128::from(i8::MIN), i128::from(i8::MAX)),
            "i16" => (i128::from(i16::MIN), i128::from(i16::MAX)),
            "i32" => (i128::from(i32::MIN), i128::from(i32::MAX)),
            "u8" => (0, i128::from(u8::MAX)),
            "u16" => (0, i128::from(u16::MAX)),
            "u32" => (0, i128::from(u32::MAX)),
            "u64" => (0, i128::from(u64::MAX)),
            _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
        }
    };
    let whole = |f: f64| -> Option<i128> { f.is_finite().then(|| f.trunc() as i128) };
    match (kind, lit) {
        (FieldKind::Integer(ty), Literal::Int(n)) => {
            let (lo, hi) = int_range(ty);
            if (lo..=hi).contains(&i128::from(*n)) {
                Landing::Exact
            } else {
                Landing::Invalid(format!("its field holds a whole number from {lo} to {hi}"))
            }
        }
        (FieldKind::Integer(ty), Literal::Float(f)) => {
            let (lo, hi) = int_range(ty);
            match whole(*f) {
                Some(n) if (lo..=hi).contains(&n) && n as f64 == *f => Landing::Exact,
                Some(n) if (lo..=hi).contains(&n) => Landing::Inexact(n.to_string()),
                _ => Landing::Invalid(format!("its field holds a whole number from {lo} to {hi}")),
            }
        }
        (FieldKind::Integer(_), Literal::Bool(b)) if !bool_pin => {
            Landing::Inexact(i64::from(*b).to_string())
        }
        (FieldKind::Real(ty), Literal::Int(n)) => {
            let stored = if *ty == "f32" {
                f64::from(*n as f32)
            } else {
                *n as f64
            };
            if stored as i128 == i128::from(*n) {
                Landing::Exact
            } else {
                Landing::Inexact(format!("{stored}"))
            }
        }
        (FieldKind::Real("f32"), Literal::Float(f))
            if f.is_finite() && f.abs() > f64::from(f32::MAX) =>
        {
            Landing::Invalid("its field holds a 32-bit float".to_string())
        }
        (FieldKind::Real(_), Literal::Bool(b)) if !bool_pin => {
            Landing::Inexact(if *b { "1.0" } else { "0.0" }.to_string())
        }
        (FieldKind::Boolean, Literal::Int(n)) if *n != 0 && *n != 1 => {
            Landing::Inexact("true".to_string())
        }
        (FieldKind::Boolean, Literal::Float(f)) if *f != 0.0 && *f != 1.0 => {
            Landing::Inexact((*f != 0.0).to_string())
        }
        (FieldKind::Enum(ty), Literal::Int(n)) => {
            if enum_has_value(ty, *n) {
                Landing::Exact
            } else {
                Landing::Invalid(enum_choices(ty))
            }
        }
        (FieldKind::Enum(ty), Literal::Float(f)) => match whole(*f) {
            Some(n) if i64::try_from(n).is_ok_and(|n| enum_has_value(ty, n)) => {
                if n as f64 == *f {
                    Landing::Exact
                } else {
                    Landing::Inexact(n.to_string())
                }
            }
            _ => Landing::Invalid(enum_choices(ty)),
        },
        (FieldKind::Enum(ty), Literal::Bool(b)) => {
            if enum_has_value(ty, i64::from(*b)) {
                Landing::Inexact(i64::from(*b).to_string())
            } else {
                Landing::Invalid(enum_choices(ty))
            }
        }
        _ => Landing::Exact,
    }
}

/// The members of the schema enum `ty`, as a WS028 lists them.
fn enum_choices(ty: &str) -> String {
    format!(
        "it is not a valid {ty} value; expected one of: {}",
        crate::catalog::enum_member_names(ty).join(", ")
    )
}

/// The written form of a constant as a diagnostic quotes it.
fn written(lit: &Literal) -> String {
    match lit {
        Literal::Float(f) => crate::ir::written_float(*f),
        Literal::Int(n) => n.to_string(),
        Literal::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// A node's inlined values by name: each value wired into a port (of two
/// into one port, the later wins), then the node's own properties for every
/// name no wire fills.
pub(super) fn inlined_values<V>(
    wired: impl IntoIterator<Item = (Sym, V)>,
    own: impl IntoIterator<Item = (Sym, V)>,
) -> HashMap<Sym, V> {
    let mut out: HashMap<Sym, V> = HashMap::default();
    for (k, v) in wired {
        out.insert(k, v);
    }
    for (k, v) in own {
        out.entry(k).or_insert(v);
    }
    out
}

/// A component of `class` whose data is the one field `field`.
pub(super) fn one_field_component(
    class: &'static str,
    field: &'static str,
    value: Box<dyn AsBrdbValue>,
) -> LiteralComponent {
    let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    data.insert(field.into(), value);
    LiteralComponent::new_from_data(class, std::sync::Arc::new(data))
}

/// Check every constant a gate stores in a native setting field (a number, bool or enum
/// field of its component data, such as `SleepTicks`' tick count or `DisplayText`'s font
/// size) against the field it lands in, across `root` and every chip module under it. A
/// constant the field cannot hold is the error `WS028`; one stored as a different value is
/// the warning `WS106`.
///
/// `param_of(note, field)` names the source-level call and parameter that set `field` on a
/// gate whose [`crate::ir::Node::note`] is `note`, so the diagnostic can quote the program;
/// `None` falls back to the field's own name.
pub fn check_native_settings(
    root: &Module,
    diagnostics: &mut Vec<Diagnostic>,
    param_of: &SettingParamOf<'_>,
) {
    let mut seen: HashSet<(SourceRange, String)> = HashSet::default();
    check_module_settings(root, &mut seen, diagnostics, param_of);
}

/// See [`check_native_settings`]: `(note, field)` to `(call, parameter)`.
pub type SettingParamOf<'a> = dyn Fn(&str, &str) -> Option<(&'static str, &'static str)> + 'a;

fn check_module_settings(
    m: &Module,
    seen: &mut HashSet<(SourceRange, String)>,
    diagnostics: &mut Vec<Diagnostic>,
    param_of: &SettingParamOf<'_>,
) {
    let value_sym = *sym::VALUE;
    let mut inlined: HashMap<NodeId, Vec<(Sym, &Literal, &SourceRange)>> = HashMap::default();
    for w in &m.wires {
        if let Some(src) = m.nodes.get(&w.source.node_id)
            && src.gate_class == gc::LITERAL
            && let Some(lit) = src.properties.get(&value_sym)
        {
            let port = crate::intern::intern(w.target.port.as_str());
            inlined
                .entry(w.target.node_id)
                .or_default()
                .push((port, lit, &src.source_range));
        }
    }
    for (id, node) in m.nodes_in_order() {
        if node.kind == NodeKind::Chip {
            continue;
        }
        let Some(fields) = gate_field_meta(node.gate_class) else {
            continue;
        };
        let values = inlined_values(
            inlined
                .get(id)
                .into_iter()
                .flatten()
                .map(|(port, lit, range)| (*port, (*lit, *range))),
            node.properties
                .iter()
                .map(|(prop, lit)| (*prop, (lit, &node.source_range))),
        );
        for fm in fields {
            if matches!(fm.kind, FieldKind::WireValue) {
                continue;
            }
            let Some((lit, lit_range)) = values.get(&fm.sym) else {
                continue;
            };
            let bool_pin = node
                .ports
                .inputs
                .iter()
                .any(|p| p.name == fm.sym && p.ty == Type::Bool);
            let landed = landing(&fm.kind, lit, bool_pin);
            if matches!(landed, Landing::Exact) {
                continue;
            }
            let param = node.note.and_then(|note| param_of(note, fm.name));
            // An input of a pure expression gate (`sqrt(x)`, `BitCount(x)`) is
            // an operand the program passes, not a setting of the gate.
            let what = if gc::is_pure_expr(node.gate_class)
                && crate::catalog::is_wire_input(node.gate_class, fm.name)
            {
                "argument"
            } else {
                "setting"
            };
            let setting = match param {
                Some((call, name)) if name == fm.name => format!("`{call}` {what} `{name}`"),
                Some((call, name)) => format!("`{call}` {what} `{name}` ({})", fm.name),
                None => format!("{what} `{}`", fm.name),
            };
            let range = if lit_range.file.is_empty() {
                node.source_range.clone()
            } else {
                (*lit_range).clone()
            };
            let range = if range.file.is_empty() {
                node.source_range.clone()
            } else {
                range
            };
            let diagnostic = match landed {
                Landing::Exact => continue,
                Landing::Inexact(stored) => Diagnostic::warning(
                    "WS106",
                    format!(
                        "{setting} is stored as {stored}, not the {} written: the field cannot hold that value exactly. Write {stored} if that is what you mean",
                        written(lit)
                    ),
                    range,
                ),
                Landing::Invalid(why) => Diagnostic::error(
                    "WS028",
                    format!("{setting} cannot be {}: {why}", written(lit)),
                    range,
                ),
            };
            if seen.insert((diagnostic.range.clone(), diagnostic.message.clone())) {
                diagnostics.push(diagnostic);
            }
        }
    }
    for child in m.chips.values() {
        check_module_settings(child, seen, diagnostics, param_of);
    }
}

/// Embed a prefab literal into `world`, content-addressed via `add_prefab`,
/// and return the `Prefabs/Uploads/...` path a spawner's `Prefab` string holds.
/// A file reference (`$./file.brz`) is resolved to its raw bytes; an inline
/// nested-prefab block (`$``` ... ``` `) is compiled to `.brz` bytes first.
fn embed_prefab(
    lit: &Literal,
    world: &mut World,
    prefab_resolver: Option<&PrefabResolver>,
    nested_compiler: Option<&NestedCompiler>,
) -> Result<String, EmitError> {
    let bytes = match lit {
        Literal::PrefabRef { path } => {
            let resolver = prefab_resolver.ok_or_else(|| {
                EmitError::PrefabResolve(
                    path.clone(),
                    "no prefab resolver configured for this compile".into(),
                )
            })?;
            resolver
                .resolve(path)
                .map_err(|e| EmitError::PrefabResolve(path.clone(), e))?
        }
        Literal::NestedPrefab { source } => {
            let compiler = nested_compiler.ok_or_else(|| {
                EmitError::PrefabResolve(
                    "<nested>".into(),
                    "no nested compiler configured for this compile".into(),
                )
            })?;
            compiler
                .compile(source, 1)
                .map_err(|e| EmitError::PrefabResolve("<nested>".into(), e))?
        }
        Literal::PrefabBytes(bytes) => bytes.to_vec(),
        _ => return Ok(String::new()),
    };
    Ok(world.add_prefab(bytes))
}

// Advanced-inventory composite config (MeshColors, WeaponAmmoOverride).
//
// brdb's `LiteralComponent` errors on a stored value in an array-typed struct field, but the
// advanced-inventory gates need `MeshColors: Color[]` and the `WeaponAmmoOverride` struct (a
// bool and a `Resources` array of structs), so the two `AsBrdbValue` helpers below serialize
// arrays and structs.

/// A native (non-wire-variant) array field value. When the containing
/// [`NativeStruct`] delegates an array-prop request to it, it yields its boxed
/// elements (it only ever backs one field, so the prop name is irrelevant).
struct NativeArray(Vec<Box<dyn AsBrdbValue>>);

impl AsBrdbValue for NativeArray {
    fn as_brdb_struct_prop_array(
        &self,
        _schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        _prop_name: BrdbInterned,
    ) -> Result<BrdbArrayIter<'_>, BrdbSchemaError> {
        Ok(Box::new(
            self.0.iter().map(|v| v.as_ref() as &dyn AsBrdbValue),
        ))
    }
}

/// A map-backed struct/component value that serializes array-typed fields (unlike brdb's
/// `LiteralComponent`). Top-level component for the advanced-inventory gates and for their
/// nested `Color` / `BRInventoryEntryWeaponAmmoOverride` /
/// `BRInventoryEntryWeaponResourceAmounts` structs. Scalar fields behave like
/// `LiteralComponent`; array fields delegate to the stored [`NativeArray`].
#[derive(Clone)]
pub(super) struct NativeStruct {
    name: &'static str,
    data: std::sync::Arc<StdMap<BString, Box<dyn AsBrdbValue>>>,
}

impl NativeStruct {
    fn new(name: &'static str, data: StdMap<BString, Box<dyn AsBrdbValue>>) -> Self {
        Self {
            name,
            data: std::sync::Arc::new(data),
        }
    }
}

impl AsBrdbValue for NativeStruct {
    fn has_brdb_struct_prop(
        &self,
        schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> bool {
        prop_name
            .get(schema)
            .is_some_and(|name| self.data.contains_key(name))
    }

    fn as_brdb_struct_prop_value(
        &self,
        schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, BrdbSchemaError> {
        let name = prop_name.get(schema).unwrap();
        self.data
            .get(name)
            .map(std::convert::AsRef::as_ref)
            .ok_or_else(|| {
                BrdbSchemaError::MissingStructField(self.name.to_string(), name.to_string())
            })
    }

    fn as_brdb_struct_prop_array(
        &self,
        schema: &BrdbSchema,
        struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> Result<BrdbArrayIter<'_>, BrdbSchemaError> {
        let name = prop_name.get(schema).unwrap();
        match self.data.get(name) {
            Some(v) => v.as_brdb_struct_prop_array(schema, struct_name, prop_name),
            None => Err(BrdbSchemaError::MissingStructField(
                self.name.to_string(),
                name.to_string(),
            )),
        }
    }
}

impl brdb::BrdbComponent for NativeStruct {
    fn component_type(&self) -> Option<BString> {
        Some(BString::Static(self.name))
    }

    fn component_type_ref(&self) -> Option<&str> {
        Some(self.name)
    }
}

/// The schema `Color { B, G, R, A: u8 }` value for one `MeshColors` element.
/// `Literal::Color` is RGBA sRGB bytes and the struct is BGRA sRGB, so the
/// bytes map straight across with no gamma.
fn color_bgra_struct(r: u8, g: u8, b: u8, a: u8) -> NativeStruct {
    let mut m: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    m.insert("B".into(), Box::new(b));
    m.insert("G".into(), Box::new(g));
    m.insert("R".into(), Box::new(r));
    m.insert("A".into(), Box::new(a));
    NativeStruct::new("Color", m)
}

/// `BRInventoryEntryWeaponAmmoOverride::Resources` is a fixed-size array in the
/// engine (the text schema shows a dynamic `[]`). Any other element count fails
/// the load with `FixedArraySizeInvalid: ... does not match expected length of
/// 8`, so it ALWAYS carries this many, padded with zero `{ Loaded, Reserve }`.
const WEAPON_AMMO_RESOURCE_SLOTS: usize = 8;

/// One `BRInventoryEntryWeaponResourceAmounts` struct.
fn weapon_resource_struct(loaded: i32, reserve: i32) -> NativeStruct {
    let mut m: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    m.insert("Loaded".into(), Box::new(loaded));
    m.insert("Reserve".into(), Box::new(reserve));
    NativeStruct::new("BRInventoryEntryWeaponResourceAmounts", m)
}

/// A `Resources` value of exactly [`WEAPON_AMMO_RESOURCE_SLOTS`] entries: the
/// supplied resources (capped) followed by zero-padding.
fn weapon_resource_array(resources: &[(i32, i32)]) -> NativeArray {
    let mut res_elems: Vec<Box<dyn AsBrdbValue>> = Vec::with_capacity(WEAPON_AMMO_RESOURCE_SLOTS);
    for &(loaded, reserve) in resources.iter().take(WEAPON_AMMO_RESOURCE_SLOTS) {
        res_elems.push(Box::new(weapon_resource_struct(loaded, reserve)));
    }
    while res_elems.len() < WEAPON_AMMO_RESOURCE_SLOTS {
        res_elems.push(Box::new(weapon_resource_struct(0, 0)));
    }
    NativeArray(res_elems)
}

/// The no-op default written whenever the user omits `ammoOverride`: overriding
/// off, with the fixed-count zero-padded `Resources` the game load requires.
fn default_weapon_ammo_override() -> NativeStruct {
    let mut m: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    m.insert("bOverrideStartingAmmo".into(), Box::new(false));
    m.insert("Resources".into(), Box::new(weapon_resource_array(&[])));
    NativeStruct::new("BRInventoryEntryWeaponAmmoOverride", m)
}

/// `MeshColors` is likewise a fixed-size array the game rejects at any other
/// length. Always emit exactly this many colors, padded with white
/// (`bOverrideColors` gates whether they apply; extra slots are ignored).
const MESH_COLOR_SLOTS: usize = 8;

/// A `MeshColors` value of exactly [`MESH_COLOR_SLOTS`] entries: the supplied
/// colors (capped) followed by white padding.
fn mesh_colors_array(colors: &[Literal]) -> NativeArray {
    let mut elems: Vec<Box<dyn AsBrdbValue>> = Vec::with_capacity(MESH_COLOR_SLOTS);
    for c in colors.iter().take(MESH_COLOR_SLOTS) {
        if let Literal::Color { r, g, b, a } = c {
            elems.push(Box::new(color_bgra_struct(*r, *g, *b, *a)));
        }
    }
    while elems.len() < MESH_COLOR_SLOTS {
        elems.push(Box::new(color_bgra_struct(255, 255, 255, 255)));
    }
    NativeArray(elems)
}

/// Decode a folded `ammoOverride` literal (the `Array[Bool, Array[Array[Int,
/// Int], ...]]` shape produced by `predeclare::fold_ammo_override`) into the
/// `BRInventoryEntryWeaponAmmoOverride` struct value, or `None` for any other shape.
fn build_weapon_ammo_override(lit: &Literal) -> Option<NativeStruct> {
    let Literal::Array(parts) = lit else {
        return None;
    };
    let [Literal::Bool(override_starting), Literal::Array(resources)] = parts.as_slice() else {
        return None;
    };
    let mut pairs: Vec<(i32, i32)> = Vec::with_capacity(resources.len());
    for r in resources {
        let Literal::Array(pair) = r else {
            return None;
        };
        let [Literal::Int(loaded), Literal::Int(reserve)] = pair.as_slice() else {
            return None;
        };
        pairs.push((*loaded as i32, *reserve as i32));
    }
    let mut m: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    m.insert("bOverrideStartingAmmo".into(), Box::new(*override_starting));
    m.insert("Resources".into(), Box::new(weapon_resource_array(&pairs)));
    Some(NativeStruct::new("BRInventoryEntryWeaponAmmoOverride", m))
}

/// Build the component for `AddInventoryItemAdv` / `SetInventoryItemAdv`. Scalar
/// fields reuse the shared [`build_gate_data_map`] path; the composite
/// `MeshColors: Color[]` and `WeaponAmmoOverride` struct fields are overwritten
/// with native array/struct values the schema writer can serialize.
pub(super) fn build_adv_inventory_component(
    gate_class: &'static str,
    ports: &GateIO,
    inlined: &HashMap<Sym, &Literal>,
    world: &mut World,
    prefab_resolver: Option<&PrefabResolver>,
    nested_compiler: Option<&NestedCompiler>,
) -> Result<NativeStruct, EmitError> {
    let mut data = build_gate_data_map(
        gate_class,
        ports,
        inlined,
        world,
        prefab_resolver,
        nested_compiler,
    )?
    .unwrap_or_default();

    // MeshColors: always written with exactly MESH_COLOR_SLOTS entries, even
    // when omitted, or the game load fails.
    let colors: &[Literal] = match inlined.get(&intern_static("MeshColors")).copied() {
        Some(Literal::Array(cs)) => cs.as_slice(),
        _ => &[],
    };
    data.insert("MeshColors".into(), Box::new(mesh_colors_array(colors)));

    // WeaponAmmoOverride: always written, as the zero-padded no-op default when
    // `ammoOverride` is omitted; the schema writer's empty array fails the load.
    let ammo = inlined
        .get(&intern_static("WeaponAmmoOverride"))
        .copied()
        .and_then(build_weapon_ammo_override)
        .unwrap_or_else(default_weapon_ammo_override);
    data.insert("WeaponAmmoOverride".into(), Box::new(ammo));

    Ok(NativeStruct::new(gate_class, data))
}

/// One `InputSources` entry per `Input0..7` pin. The game writes all eight
/// even on a call with nothing wired, as it does for the other fixed-size
/// arrays above.
const CALLABLE_INPUT_SLOTS: usize = 8;

/// A Function Call: its pin values plus `InputSources`, the
/// `ECallableInputSource[]` that says per input whether the call supplies the
/// value (`Override`, 1) or the definition's `DefaultValue` does
/// (`DefinitionDefault`, 0). Read from an `InputSources` array of ints; any
/// slot it leaves out is `DefinitionDefault`.
pub(super) fn build_function_call_component(
    ports: &GateIO,
    inlined: &HashMap<Sym, &Literal>,
    world: &mut World,
    prefab_resolver: Option<&PrefabResolver>,
    nested_compiler: Option<&NestedCompiler>,
) -> Result<NativeStruct, EmitError> {
    let mut data = build_gate_data_map(
        gc::FUNCTION_CALL,
        ports,
        inlined,
        world,
        prefab_resolver,
        nested_compiler,
    )?
    .unwrap_or_default();
    let given: &[Literal] = match inlined.get(&*sym::INPUT_SOURCES).copied() {
        Some(Literal::Array(v)) => v.as_slice(),
        _ => &[],
    };
    let mut sources: Vec<Box<dyn AsBrdbValue>> = Vec::with_capacity(CALLABLE_INPUT_SLOTS);
    for i in 0..CALLABLE_INPUT_SLOTS {
        let v = u8::from(matches!(given.get(i), Some(Literal::Int(1))));
        sources.push(Box::new(v));
    }
    data.insert("InputSources".into(), Box::new(NativeArray(sources)));
    Ok(NativeStruct::new(gc::FUNCTION_CALL, data))
}

/// Test-only: build the advanced-inventory component for `node`, round-trip its
/// data struct through the max schema's `write_brdb` (emit's own path), and
/// return the decoded struct so tests can assert on the serialized data.
#[cfg(any(test, feature = "test-internals"))]
#[doc(hidden)]
pub fn roundtrip_adv_inventory_component(node: &crate::ir::Node) -> brdb::schema::BrdbValue {
    use brdb::schema::ReadBrdbSchema;
    let mut world = World::new();
    let mut inlined: HashMap<Sym, &Literal> = HashMap::default();
    for (k, v) in node.properties.as_ref() {
        inlined.insert(*k, v);
    }
    let comp = build_adv_inventory_component(
        node.gate_class,
        &node.ports,
        &inlined,
        &mut world,
        None,
        None,
    )
    .expect("build advanced-inventory component");
    let schema = brdb::schemas::bricks_components_schema_max();
    let struct_name = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(c, _)| *c == node.gate_class)
        .map(|(_, s)| *s)
        .expect("gate class has a data struct");
    let bytes = schema
        .write_brdb(struct_name, &comp)
        .expect("serialize component data");
    let schema_arc = std::sync::Arc::new(schema.clone());
    let mut cursor = &bytes[..];
    cursor
        .read_brdb(&schema_arc, struct_name)
        .expect("read component data back")
}
