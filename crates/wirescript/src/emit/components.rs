//! Component-data encoding: schema metadata in, brdb component out.

use super::*;

/// Build the gate's `LiteralComponent`. For wire-input ports that have an
/// inlined literal (from a `_Literal` source node), we embed the value into
/// the component data struct using the wire_graph_variant field so the game
/// reads it on load without needing a separate constant source gate.
pub(super) fn build_gate_component(
    gate_class: &'static str,
    ports: &crate::ir::GateIO,
    inlined: &StdMap<Sym, &Literal>,
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
        // Default: dataless component stub — registers component type only,
        // no struct data (engine uses the default from the brick type).
        None => Ok(LiteralComponent::new(gate_class)),
    }
}

/// Build the component data map (schema field name → serializable value) for a
/// gate that has a data struct, or `None` for a dataless gate. Each field is
/// classified once via [`gate_field_meta`] and serialized from its inlined
/// literal, falling back to typed wire-variant defaults / STRUCT_DEFAULTS for
/// unset fields. Shared by [`build_gate_component`] and the advanced-inventory
/// composite builder.
fn build_gate_data_map(
    gate_class: &'static str,
    ports: &crate::ir::GateIO,
    inlined: &StdMap<Sym, &Literal>,
    world: &mut World,
    prefab_resolver: Option<&PrefabResolver>,
    nested_compiler: Option<&NestedCompiler>,
) -> Result<Option<StdMap<BString, Box<dyn AsBrdbValue>>>, EmitError> {
    // For gates whose component data struct carries wire_graph_variant fields,
    // look up the struct schema and build the component with embedded values.
    // Non-inlined fields get a default (Int(0)) so the struct is always complete.
    // Always write the data struct when the gate type has one — even if no
    // fields are inlined. This ensures ALL instances of the same component
    // type have matching data, preventing reader misalignment.
    let Some(fields) = gate_field_meta(gate_class) else {
        return Ok(None);
    };
    let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    for fm in fields {
            let field = fm.name;
            let is_variant = matches!(fm.kind, FieldKind::WireVariant | FieldKind::PrimMathVariant);
            let val: Box<dyn AsBrdbValue> = match inlined.get(&fm.sym) {
                Some(lit) if is_variant => {
                    // prim_math_variant doesn't support Bool — coerce to Int
                    if fm.kind == FieldKind::PrimMathVariant
                        && let Literal::Bool(b) = lit
                    {
                        Box::new(WireVariant::Int(if *b { 1 } else { 0 }))
                    } else {
                        literal_to_boxed_wire_variant(lit, ports, field)
                    }
                }
                Some(lit) => match fm.kind {
                    FieldKind::AssetRef => {
                        // Asset-reference field (AudioDescriptor, Item, …):
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
                    FieldKind::BundlePathRef => {
                        // Prefab file reference (`$./file.brz`): resolve to raw
                        // bytes, embed content-addressed via add_prefab, and
                        // store the resulting `Prefabs/Uploads/…` path.
                        match lit {
                            Literal::PrefabRef { path } => {
                                let resolver = prefab_resolver.ok_or_else(|| {
                                    EmitError::PrefabResolve(
                                        path.clone(),
                                        "no prefab resolver configured for this compile".into(),
                                    )
                                })?;
                                let bytes = resolver
                                    .resolve(path)
                                    .map_err(|e| EmitError::PrefabResolve(path.clone(), e))?;
                                let embedded = world.add_prefab(bytes);
                                Box::new(embedded)
                            }
                            // Inline nested-prefab block (`$``` ... ``` `):
                            // compile the inner source to `.brz` bytes and
                            // embed it the same way a resolved on-disk prefab
                            // is embedded above.
                            Literal::NestedPrefab { source } => {
                                let compiler = nested_compiler.ok_or_else(|| {
                                    EmitError::PrefabResolve(
                                        "<nested>".into(),
                                        "no nested compiler configured for this compile".into(),
                                    )
                                })?;
                                let bytes = compiler
                                    .compile(source, 1)
                                    .map_err(|e| EmitError::PrefabResolve("<nested>".into(), e))?;
                                let embedded = world.add_prefab(bytes);
                                Box::new(embedded)
                            }
                            // A non-prefab literal here can't happen via the
                            // front end (the port only accepts `$…` refs).
                            _ => Box::new(String::new()),
                        }
                    }
                    FieldKind::Enum(type_name) => match resolve_enum_value(type_name, lit) {
                        Some(ev) => Box::new(ev),
                        None => literal_to_boxed_native(lit).ok_or_else(|| {
                            EmitError::UnrepresentableLiteral {
                                field: field.to_string(),
                                literal: (**lit).clone(),
                            }
                        })?,
                    },
                    FieldKind::Str => Box::new(literal_to_string(lit).ok_or_else(|| {
                        EmitError::UnrepresentableLiteral {
                            field: field.to_string(),
                            literal: (**lit).clone(),
                        }
                    })?),
                    // A native struct field takes one representation, so a
                    // rotation constant bound for a quaternion field converts
                    // here rather than at runtime (see
                    // `reshape_literal_for_struct`).
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
                    _ => literal_to_boxed_native(lit).ok_or_else(|| EmitError::UnrepresentableLiteral {
                        field: field.to_string(),
                        literal: (**lit).clone(),
                    })?,
                },
                // No inlined value. Wire-typed ports still need a typed variant
                // default so the variant member matches the port type (the
                // component_db defaults don't carry wire variants). Every other
                // (native) field is omitted from the data map entirely: the brdb
                // writer fills missing struct fields from component_db's
                // STRUCT_DEFAULTS — the single source of truth for gate defaults
                // (e.g. DisplayText FontSize=16, Lifetime=5) — falling back to a
                // type-zero when no default is registered.
                None if is_variant => {
                    let port_ty = ports
                        .inputs
                        .iter()
                        .chain(ports.outputs.iter())
                        .find(|p| p.name == fm.sym)
                        .map(|p| &p.ty);
                    let wv = var_type_to_wire_variant(port_ty);
                    // prim_math_variant doesn't support Bool — coerce to Int
                    let wv = if fm.kind == FieldKind::PrimMathVariant {
                        coerce_for_prim_math(wv)
                    } else {
                        wv
                    };
                    Box::new(wv)
                }
                None => continue,
            };
            data.insert(BString::Static(field), val);
        }
    Ok(Some(data))
}

// --- Advanced-inventory composite config (MeshColors, WeaponAmmoOverride) ---
//
// brdb's `LiteralComponent` deliberately errors when an *array*-typed struct
// field has a stored value ("literal gate data only carries scalars"). The two
// advanced-inventory gates need real native composites — `MeshColors: Color[]`
// and the `WeaponAmmoOverride` struct (a bool + a `Resources` array of structs)
// — so they use the two small `AsBrdbValue` helpers below, which provide the
// array + struct serialization the schema writer drives, without touching brdb.

/// A native (non-wire-variant) array field value. When the containing
/// [`NativeStruct`] delegates an array-prop request to it, it yields its boxed
/// elements (it only ever backs one field, so the prop name is irrelevant).
struct NativeArray(Vec<Box<dyn AsBrdbValue>>);

impl AsBrdbValue for NativeArray {
    fn as_brdb_struct_prop_array(
        &self,
        _schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        _prop_name: brdb::schema::BrdbInterned,
    ) -> Result<brdb::schema::as_brdb::BrdbArrayIter<'_>, brdb::BrdbSchemaError> {
        Ok(Box::new(self.0.iter().map(|v| v.as_ref() as &dyn AsBrdbValue)))
    }
}

/// A map-backed struct/component value that DOES serialize array-typed fields
/// (unlike brdb's `LiteralComponent`). Used as the top-level component for the
/// advanced-inventory gates and for their nested `Color` /
/// `BRInventoryEntryWeaponAmmoOverride` / `BRInventoryEntryWeaponResourceAmounts`
/// structs. Scalar fields behave exactly like `LiteralComponent`; array fields
/// delegate to the stored [`NativeArray`].
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
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> bool {
        prop_name
            .get(schema)
            .is_some_and(|name| self.data.contains_key(name))
    }

    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        let name = prop_name.get(schema).unwrap();
        self.data.get(name).map(|v| v.as_ref()).ok_or_else(|| {
            brdb::BrdbSchemaError::MissingStructField(self.name.to_string(), name.to_string())
        })
    }

    fn as_brdb_struct_prop_array(
        &self,
        schema: &brdb::schema::BrdbSchema,
        struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<brdb::schema::as_brdb::BrdbArrayIter<'_>, brdb::BrdbSchemaError> {
        let name = prop_name.get(schema).unwrap();
        match self.data.get(name) {
            Some(v) => v.as_brdb_struct_prop_array(schema, struct_name, prop_name),
            None => Err(brdb::BrdbSchemaError::MissingStructField(
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
}

/// The schema `Color { B, G, R, A: u8 }` value for one `MeshColors` element.
/// `Literal::Color` is semantic RGBA; the struct is BGRA, and Brickadia stores
/// brick colours sRGB-direct — so the bytes map straight across, no gamma.
fn color_bgra_struct(r: u8, g: u8, b: u8, a: u8) -> NativeStruct {
    let mut m: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    m.insert("B".into(), Box::new(b));
    m.insert("G".into(), Box::new(g));
    m.insert("R".into(), Box::new(r));
    m.insert("A".into(), Box::new(a));
    NativeStruct::new("Color", m)
}

/// Decode a folded `ammoOverride` literal (the `Array[Bool, Array[Array[Int,
/// Int], …]]` shape produced by `predeclare::fold_ammo_override`) into the
/// `BRInventoryEntryWeaponAmmoOverride` struct value.
/// The engine reflects `BRInventoryEntryWeaponAmmoOverride::Resources` as a
/// fixed-size array (a `TStaticArray`), even though the text schema flattens it
/// to a dynamic `[]`. Writing any other element count fails the game load with
/// `FixedArraySizeInvalid: … does not match expected length of 8`, so the field
/// must ALWAYS carry exactly this many entries — including when the user omits
/// `ammoOverride` entirely (see [`build_adv_inventory_component`]). Unspecified
/// slots are padded with a zero `{ Loaded, Reserve }` resource.
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
    let mut res_elems: Vec<Box<dyn AsBrdbValue>> =
        Vec::with_capacity(WEAPON_AMMO_RESOURCE_SLOTS);
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
/// length (`FixedArraySizeInvalid … expected length of 8`). Always emit exactly
/// this many colours, padding unspecified slots with white (`bOverrideColors`
/// gates whether they apply at all, and a mesh with fewer material slots ignores
/// the extras).
const MESH_COLOR_SLOTS: usize = 8;

/// A `MeshColors` value of exactly [`MESH_COLOR_SLOTS`] entries: the supplied
/// colours (capped) followed by white padding.
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
    ports: &crate::ir::GateIO,
    inlined: &StdMap<Sym, &Literal>,
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

    // MeshColors: a fixed-size Color[] (like WeaponAmmoOverride.Resources).
    // Always written with exactly MESH_COLOR_SLOTS entries — the user's colours
    // padded with white — even when omitted, or the game load fails.
    let colors: &[Literal] = match inlined.get(&intern_static("MeshColors")).copied() {
        Some(Literal::Array(cs)) => cs.as_slice(),
        _ => &[],
    };
    data.insert("MeshColors".into(), Box::new(mesh_colors_array(colors)));

    // WeaponAmmoOverride: bOverrideStartingAmmo + a fixed-count Resources array.
    // Always written — its Resources is a fixed-size array the game rejects at
    // any other length, so even when the user omits `ammoOverride` we must emit
    // the zero-padded no-op default rather than let the schema writer fall back
    // to an empty (length-0) array.
    let ammo = inlined
        .get(&intern_static("WeaponAmmoOverride"))
        .copied()
        .and_then(build_weapon_ammo_override)
        .unwrap_or_else(default_weapon_ammo_override);
    data.insert("WeaponAmmoOverride".into(), Box::new(ammo));

    Ok(NativeStruct::new(gate_class, data))
}

/// Test-only: build the advanced-inventory component for `node` and round-trip
/// its data struct through the live max schema — the exact `write_brdb` path
/// emit uses for component data — returning the decoded struct so tests can
/// assert on the serialized bytes (not just the IR properties).
#[cfg(test)]
pub(crate) fn roundtrip_adv_inventory_component(node: &Node) -> brdb::schema::BrdbValue {
    use brdb::schema::ReadBrdbSchema;
    let mut world = World::new();
    let mut inlined: StdMap<Sym, &Literal> = StdMap::new();
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
