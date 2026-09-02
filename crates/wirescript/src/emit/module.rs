//! The recursive per-module emit driver.

use super::*;

pub(super) struct EmitContext {
    pub(super) node_brick_ids: HashMap<NodeId, usize>,
    pub(super) class_index: HashMap<NodeId, &'static str>,
    /// Resolver for `$./file.brz` prefab references, from `EmitOptions`.
    pub(super) prefab_resolver: Option<PrefabResolver>,
    /// Compiler for inline nested-prefab blocks, from `EmitOptions`.
    pub(super) nested_compiler: Option<NestedCompiler>,
    /// (target node, target port) → source node, accumulated across all
    /// modules so Var_Get/Set gates can trace `VarRef` wires that cross
    /// module boundaries (scope captures, anon-chip partitions).
    pub(super) wire_sources: HashMap<(NodeId, WirePort), NodeId>,
    /// Pseudo_Var/ArrayVar node → its labelable source name. Vars are
    /// always emitted before the gates that reference them.
    pub(super) var_labels: HashMap<NodeId, String>,
    /// Module-level `@invisible`, from `EmitOptions` — suppresses var-tag
    /// and I/O-gate label emission in `emit_module`.
    pub(super) invisible: bool,
    /// Module-level `@layout("cube")`, from `EmitOptions` — suppresses the
    /// per-gate name labels and var tags in `emit_module`. See
    /// `EmitOptions::no_gate_labels`.
    pub(super) no_gate_labels: bool,
    /// The root microchip shell brick's id. Pass 3.5 wires a module-level
    /// dynamic `@label` (`Module.root_dynamic_label`) into this brick's label
    /// `Text` port.
    pub(super) root_shell_brick_id: usize,
}

/// Resolve a wire source to its var/array-var label, following the source
/// through any number of `MicrochipInput`/`MicrochipOutput` boundary pins
/// (inserted by the boundary-pins pass for wires that cross a chip
/// boundary) and `Rerouter` hops back to the originating
/// `Pseudo_Var`/`Pseudo_ArrayVar` node. Without this, a global written from
/// inside a named chip, or one reached through a rerouter, resolves only as
/// far as the hop and loses its tag. Bus lane rerouters have no IR node, so
/// `wire_sources` holds no entry for them and the walk ends there. Bounded
/// so a malformed graph can't spin forever.
pub(super) fn resolve_var_label(ctx: &EmitContext, mut src: NodeId) -> Option<&String> {
    const MAX_HOPS: usize = 64;
    for _ in 0..MAX_HOPS {
        if let Some(label) = ctx.var_labels.get(&src) {
            return Some(label);
        }
        let class = *ctx.class_index.get(&src)?;
        if class != gc::MICROCHIP_INPUT && class != gc::MICROCHIP_OUTPUT && class != gc::REROUTER {
            return None;
        }
        src = *ctx.wire_sources.get(&(src, WirePort::RerInput))?;
    }
    None
}

pub(super) fn emit_module(
    world: &mut World,
    ctx: &mut EmitContext,
    module: &Module,
    layout: &LayoutResult,
    bricks: &mut Vec<brdb::Brick>,
    wall: &WallLayout,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<(), EmitError> {
    let value_sym = *sym::VALUE;
    let mut inlined_by_node: StdMap<NodeId, Vec<(WirePort, Literal)>> = StdMap::new();
    for w in &module.wires {
        let src_node = module.nodes.get(&w.source.node_id);
        if src_node.map(|n| n.gate_class) == Some(gc::LITERAL) {
            let lit = src_node.and_then(|n| n.properties.get(&value_sym)).cloned();
            if let Some(lit) = lit {
                inlined_by_node
                    .entry(w.target.node_id)
                    .or_default()
                    .push((w.target.port, lit));
            }
        }
    }

    let mut wire_target_index: StdMap<(NodeId, WirePort), NodeId> = StdMap::new();
    for w in &module.wires {
        wire_target_index.insert((w.target.node_id, w.target.port), w.source.node_id);
        ctx.wire_sources
            .insert((w.target.node_id, w.target.port), w.source.node_id);
    }

    let mut sorted_ids: Vec<&NodeId> = module.nodes.keys().collect();
    sorted_ids.sort_by_key(|id| id.0);

    // Register ALL nodes in class_index (including _Literal, _Unsupported)
    // so the wire filter in pass 3 can identify and skip them. Only spawnable
    // nodes get bricks.
    for (id, node) in &module.nodes {
        ctx.class_index.insert(*id, node.gate_class);
    }

    // ── Pass 1: emit all non-chip gates ──
    // PseudoVar/PseudoArrayVar bricks are registered before any child
    // module tries to wire to them via scope captures.
    for id in &sorted_ids {
        let node = &module.nodes[id];
        if node.kind == crate::ir::NodeKind::Chip {
            continue;
        }
        if !is_spawnable(node.kind, node.gate_class) {
            continue;
        }
        let pos = layout
            .placements
            .get(id)
            .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;
        let gate_class_str = node.gate_class;
        // One catalog lookup per brick: brick asset + half-size both come
        // from the same entry. Unknown classes (synthetic IR-only nodes)
        // fall back to a reroute node. The catalog is 'static, so the
        // asset name needs no per-brick String clone.
        let catalog_entry = crate::catalog::default_catalog().find_by_class(gate_class_str);
        let brick_asset: &'static str = catalog_entry
            .map(|g| g.brick_asset.as_str())
            .unwrap_or("B_1x1_Reroute_Node");
        // Offset each brick by its half-size so the brick's min corner aligns
        // with the cell grid line at (pos.x, pos.y). This keeps every brick
        // inside its own cell regardless of size (1x1, wide DisplayText, etc.)
        // and prevents overlaps between adjacent cells of different sizes.
        let (half_x, half_y) = match catalog_entry {
            Some(g) => (g.half_size.x, g.half_size.y),
            _ => (5, 5),
        };
        // A quarter-turned brick swaps its footprint, so its center sits at
        // the swapped half-extent from the same min corner. The layout has
        // already reserved the cell that way; `brdb::Brick::local_bounds()`
        // is rotation-blind, so nothing downstream would catch a mismatch
        // here — the game would just drop the overlapping bricks at load.
        let rotation = layout.rotations.get(*id).copied().unwrap_or_default();
        let (offset_x, offset_y) = match rotation {
            // A half turn lands the brick the way round it started, so only
            // the QUARTER turns swap. Deg180 measures exactly like Deg0, and
            // Deg270 exactly like Deg90.
            NodeRotation::Deg0 | NodeRotation::Deg180 => (half_x, half_y),
            NodeRotation::Deg90 | NodeRotation::Deg270 => (half_y, half_x),
        };
        let inner_pos = brdb::Position {
            x: pos.x + offset_x,
            y: pos.y + offset_y,
            z: pos.z,
        };
        let (mut brick, brick_id) = brdb::Brick {
            asset: BrickType::from(brick_asset),
            position: inner_pos,
            rotation: match rotation {
                NodeRotation::Deg0 => brdb::Rotation::Deg0,
                NodeRotation::Deg90 => brdb::Rotation::Deg90,
                NodeRotation::Deg180 => brdb::Rotation::Deg180,
                NodeRotation::Deg270 => brdb::Rotation::Deg270,
            },
            color: color_for_node(node, module, &wire_target_index),
            ..Default::default()
        }
        .with_id_split();

        let mut gate_inlined: StdMap<Sym, &Literal> = StdMap::new();
        if let Some(entries) = inlined_by_node.get(id) {
            for (port_idx, lit) in entries {
                let port_sym = crate::intern::intern(port_idx.as_str());
                gate_inlined.insert(port_sym, lit);
            }
        }
        // Inject node properties (e.g. InitialValue, Value) into inlined
        // so data structs carry the correct wire_graph_variant type.
        for (prop_name, lit) in node.properties.as_ref() {
            gate_inlined.entry(*prop_name).or_insert(lit);
        }

        let effective_class_str = node.gate_class;
        let port_label_sym = *sym::PORT_LABEL;
        // The advanced-inventory gates carry composite (array/struct) config
        // (`MeshColors: Color[]`, `WeaponAmmoOverride`) that a `LiteralComponent`
        // can't serialize — build them with an array-capable `NativeStruct`.
        let comp_boxed: Box<dyn brdb::BrdbComponent> = if effective_class_str
            == gc::CHARACTER_ADD_INVENTORY_ITEM_ADV
            || effective_class_str == gc::CHARACTER_SET_INVENTORY_ITEM_ADV
        {
            Box::new(build_adv_inventory_component(
                effective_class_str,
                &node.ports,
                &gate_inlined,
                world,
                ctx.prefab_resolver.as_ref(),
                ctx.nested_compiler.as_ref(),
            )?)
        } else {
            Box::new(match effective_class_str {
            // Pseudo_Var: WireGraphVariant typed by the Value port.
            "BrickComponentType_WireGraphPseudo_Var" => {
                let value_ty = node
                    .ports
                    .inputs
                    .iter()
                    .chain(node.ports.outputs.iter())
                    .find(|p| resolve(p.name) == "Value")
                    .map(|p| &p.ty);
                let wv: WireVariant = match node
                    .properties
                    .get(&*sym::INITIAL_VALUE)
                    .or_else(|| node.properties.get(&value_sym))
                {
                    Some(lit) => literal_to_wire_variant(lit).unwrap_or(WireVariant::Number(0.0)),
                    None => var_type_to_wire_variant(value_ty),
                };
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("Value".into(), Box::new(wv));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            // Pseudo_ArrayVar: WireGraphArrayVariant, member chosen by the
            // declared element type so the array stores the right scalar kind
            // (int/bool/string/vector/object) instead of defaulting to doubles.
            "BrickComponentType_WireGraphPseudo_ArrayVar" => {
                let elem_ty = node
                    .ports
                    .outputs
                    .iter()
                    .find(|p| resolve(p.name) == "ArrayVarRef")
                    .and_then(|p| array_element_type(&p.ty));
                // A constant initializer is carried as an `InitialValue` list
                // literal; otherwise the array starts empty.
                let av = match node.properties.get(&intern_static("InitialValue")) {
                    Some(Literal::Array(lits)) => wire_array_variant_from_literals(elem_ty, lits),
                    _ => empty_wire_array_variant(elem_ty),
                };
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("Value".into(), Box::new(av));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            // Pseudo_MapVar: WireGraphMapVariant, key/value kinds chosen from the
            // declared `Map<K, V>` on the MapVarRef port. A constant initializer
            // is carried as an `InitialValue` map literal and populates the
            // variant's `entries`; otherwise the map starts empty (runtime
            // `set`s populate it).
            "BrickComponentType_WireGraphPseudo_MapVar" => {
                let kinds = node
                    .ports
                    .outputs
                    .iter()
                    .find(|p| resolve(p.name) == "MapVarRef")
                    .map(|p| map_variant_from_type(&p.ty))
                    .unwrap_or(WireMapVariant {
                        key: WireMapKey::Int64,
                        value: WireMapValue::Number,
                        entries: vec![],
                    });
                let mv = match node.properties.get(&intern_static("InitialValue")) {
                    Some(Literal::Map(entries)) => wire_map_variant_from_literals(kinds, entries),
                    _ => kinds,
                };
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("Value".into(), Box::new(mv));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            // MicrochipInput/Output: PortLabel string.
            "BrickComponentType_Internal_MicrochipInput" => {
                let label = node
                    .properties
                    .get(&port_label_sym)
                    .and_then(|l| {
                        if let Literal::String(s) = l {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("PortLabel".into(), Box::new(label));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            "BrickComponentType_Internal_MicrochipOutput" => {
                let label = node
                    .properties
                    .get(&port_label_sym)
                    .and_then(|l| {
                        if let Literal::String(s) = l {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("PortLabel".into(), Box::new(label));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            // SetInventoryEntry: the weapon is a nested asset in
            // EntryPlan.ItemTypeIfItem. Register the asset in the world's
            // external-asset table and build the EntryPlan struct (Type=Item).
            // NOTE: the exact binary encoding (enum repr, asset-in-class field,
            // nested defaults) needs in-game verification.
            "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry" => {
                use brdb::schema::BrdbValue;
                let slot = match node.properties.get(&intern_static("Slot")) {
                    Some(Literal::Int(n)) => *n as i32,
                    _ => 0,
                };
                let item_idx = match node.properties.get(&intern_static("ItemTypeIfItem")) {
                    Some(Literal::Asset {
                        asset_type,
                        asset_name,
                    }) => {
                        let (idx, _) = world
                            .global_data
                            .external_asset_references
                            .insert_full((asset_type.clone(), asset_name.clone()));
                        Some(idx)
                    }
                    _ => None,
                };
                let brick_wrapper = LiteralComponent::new("BrickTypeNetWrapper").with_data([
                    (
                        "BrickAsset",
                        Box::new(BrdbValue::Asset(None)) as Box<dyn AsBrdbValue>,
                    ),
                    (
                        "ProceduralSize",
                        Box::new(IntVector::default()) as Box<dyn AsBrdbValue>,
                    ),
                ]);
                let entry_plan = LiteralComponent::new("BRInventoryEntryPlan").with_data([
                    // EBRInventoryEntryPlanType::Item = 3
                    ("Type", Box::new(3u8) as Box<dyn AsBrdbValue>),
                    (
                        "BrickTypeIfBrick",
                        Box::new(brick_wrapper) as Box<dyn AsBrdbValue>,
                    ),
                    (
                        "EntityTypeIfEntity",
                        Box::new(BrdbValue::Asset(None)) as Box<dyn AsBrdbValue>,
                    ),
                    (
                        "ItemTypeIfItem",
                        Box::new(BrdbValue::Asset(item_idx)) as Box<dyn AsBrdbValue>,
                    ),
                ]);
                // `Item` is a BRInventoryEntryVariant (the entry's current
                // contents); a freshly-planned entry is `Nothing` (an empty
                // member struct → just its variant tag).
                let entry = LiteralComponent::new("BRInventoryEntryConfig").with_data([(
                    "Item",
                    Box::new(LiteralComponent::new("BRInventoryEntryNothing"))
                        as Box<dyn AsBrdbValue>,
                )]);
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("Slot".into(), Box::new(slot));
                data.insert("Entry".into(), Box::new(entry));
                data.insert("EntryPlan".into(), Box::new(entry_plan));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            _ => build_gate_component(
                effective_class_str,
                &*node.ports,
                &gate_inlined,
                world,
                ctx.prefab_resolver.as_ref(),
                ctx.nested_compiler.as_ref(),
            )?,
            })
        };
        brick.add_component_box(comp_boxed);

        // Second component: floating name label on I/O gates and variables.
        // (Chip bricks get theirs in pass 2 / build_world.)
        // All kinds float the label above the gate brick (Offset.z +0.5;
        // the chip-shell template value of -0.5 sinks it into these bricks).
        // `_`-prefixed names are synthesized plumbing — chip I/O maps the
        // well-known ones to friendly labels (see microchip_io_label);
        // everywhere else they stay unlabeled.
        let named = |l: &Literal| match l {
            Literal::String(s) if !s.is_empty() && !s.starts_with('_') => Some(s.clone()),
            _ => None,
        };
        let label_spec = match effective_class_str {
            "BrickComponentType_Internal_MicrochipInput"
            | "BrickComponentType_Internal_MicrochipOutput" => {
                let label = microchip_io_label(node);
                // A container access gate (Var/ArrayVar/MapVar_*) reads its small
                // tag by tracing its ref wire back to the source's label. When the
                // source is an INPUT PIN rather than a `Pseudo_*Var` — a
                // reference-passed `in xs: T[]` / `in m: Map<..>` — record the
                // pin's name here too, so a get/set on an input-backed container
                // is tagged with the input's name instead of nothing.
                if let Some(name) = &label {
                    ctx.var_labels.insert(**id, name.clone());
                }
                label.map(|s| (s, LABEL_LINE_HEIGHT))
            }
            "BrickComponentType_WireGraphPseudo_Var"
            | "BrickComponentType_WireGraphPseudo_ArrayVar"
            | "BrickComponentType_WireGraphPseudo_MapVar" => {
                let label = node.properties.get(&*sym::NAME_LABEL).and_then(named);
                // The var's own name still drives the small tags on its
                // Var_Get/Set gates (via var_labels), even when its own big
                // label is a dynamic, wire-driven one below.
                if let Some(name) = &label {
                    ctx.var_labels.insert(**id, name.clone());
                }
                if module.dynamic_labels.contains_key(*id) {
                    // A dynamic `@label(expr)` drives this var's label text by
                    // wire (Pass 3.5). Force the text component to exist as the
                    // wire's target — with an empty placeholder the wire
                    // overrides at runtime — REGARDLESS of the name-suppression
                    // filter. Without this, a var whose name is empty or
                    // `_`-prefixed would emit a wire into a Component_TextDisplay
                    // that was never added: a dangling target that fails to
                    // connect at load.
                    Some((String::new(), LABEL_LINE_HEIGHT))
                } else {
                    label.map(|s| (s, LABEL_LINE_HEIGHT))
                }
            }
            // Var/ArrayVar exec gates: a smaller tag naming the variable
            // they access, traced through the gate's (Array)VarRef wire.
            // The var node is always emitted first, so its label is known.
            c if c.starts_with("BrickComponentType_WireGraph_Exec_Var_")
                || c.starts_with("BrickComponentType_WireGraph_Exec_ArrayVar_")
                || c.starts_with("BrickComponentType_WireGraph_Exec_MapVar_") =>
            {
                node.ports
                    .inputs
                    .iter()
                    .find(|p| matches!(resolve(p.name), "VarRef" | "ArrayVarRef" | "MapVarRef"))
                    .and_then(|p| {
                        let port = WirePort::from_name(resolve(p.name));
                        ctx.wire_sources.get(&(node.id, port))
                    })
                    .and_then(|src| resolve_var_label(ctx, *src))
                    .map(|s| (s.clone(), VAR_TAG_LINE_HEIGHT))
            }
            _ => None,
        };
        if let Some((text, line_height)) = label_spec {
            // A cube packs gates shoulder to shoulder, so these labels are
            // unreadable there and cost one `Component_TextDisplay` per gate,
            // which is the majority of a cube's components. A runtime
            // `@label(expr)` is exempt: its text is a value the program
            // computes, and Pass 3.5 wires into this exact component, so
            // dropping it would leave a dangling wire that fails at load.
            let dynamic = module.dynamic_labels.contains_key(*id);
            if !ctx.invisible && (!ctx.no_gate_labels || dynamic) {
                brick.add_component_box(Box::new(text_label(
                    world,
                    &text,
                    label_rotation_deg(rotation),
                    0.5,
                    line_height,
                    0.5,
                    0.5,
                )));
            }
        }

        bricks.push(brick);
        ctx.node_brick_ids.insert(**id, brick_id);
        ctx.class_index.insert(**id, node.gate_class);
    }

    emit_annotations(world, bricks, &layout.annotations);
    // Lane bricks now; their wires wait for pass 3, where the chip-port
    // index a `BusEnd::Node` tap may need has been built.
    let bus_brick_ids = emit_bus(bricks, &layout.bus, module, &wire_target_index);

    // ── Pass 2: recursively emit chip children ──
    for id in &sorted_ids {
        let node = &module.nodes[id];
        if node.kind != crate::ir::NodeKind::Chip {
            continue;
        }
        let child_module = match module.chips.get(id) {
            Some(m) => m,
            None => continue,
        };
        let pos = layout
            .placements
            .get(id)
            .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;
        let inner_pos = brdb::Position {
            x: pos.x + 5,
            y: pos.y + 5,
            z: pos.z,
        };

        let child_layout = layout
            .chip_layouts
            .get(id)
            .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;
        let slot = wall
            .chips
            .get(id)
            .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;

        let chip_entity_id = brdb::Brick::next_id();
        let child_entity = brdb::Entity {
            asset: brdb::assets::entities::MICROCHIP_GRID,
            id: Some(chip_entity_id),
            location: slot.location,
            rotation: WALL_ROT,
            frozen: true,
            data: brdb::assets::entities::microchip_grid_entity(
                chip_is_closed(node),
                IntVector { x: 0, y: 0, z: 0 },
                slot.extent,
            ),
            ..Default::default()
        };

        let mut child_bricks: Vec<brdb::Brick> = Vec::new();
        emit_module(
            world,
            ctx,
            child_module,
            child_layout,
            &mut child_bricks,
            wall,
            template_cache,
        )?;

        let header_doc = match node.properties.get(&*sym::DOC_TEXT) {
            Some(Literal::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        emit_plane_header(
            world,
            &mut child_bricks,
            slot.extent,
            chip_display_name(node, child_module).as_deref(),
            header_doc.as_deref(),
        );

        world.add_brick_grid(child_entity, child_bricks);

        let (mut chip_brick, chip_brick_id) = brdb::Brick {
            asset: brdb::assets::bricks::B_MICROCHIP,
            position: inner_pos,
            ..Default::default()
        }
        .with_component_box(Box::new(LiteralComponent::new(
            "Component_Internal_Microchip",
        )))
        .with_id_split();
        // Named chips get a floating name label (the `@label` override wins
        // over the declared name); anonymous groupings (ModuleRoot-scoped
        // partitions) stay unlabeled. Dropped under `no_gate_labels` for the
        // same reason as the gate labels above: this one rides the chip's own
        // brick out in the packed block. The plane header emitted just above is
        // NOT dropped, since it titles the chip's inner plane and stays legible
        // when the chip is opened.
        if let Some(name) = chip_display_name(node, child_module).filter(|_| !ctx.no_gate_labels) {
            chip_brick.add_component_box(Box::new(text_label(
                world,
                &name,
                LABEL_ROTATION_DEG,
                -0.5,
                LABEL_LINE_HEIGHT,
                0.5,
                0.5,
            )));
        }
        bricks.push(chip_brick);
        ctx.node_brick_ids.insert(**id, chip_brick_id);
        ctx.class_index.insert(**id, node.gate_class);
        world.register_microchip_link(chip_brick_id, chip_entity_id);

    }

    // ── Pass 3: emit this module's wires ──
    let layout_port_id = WirePort::Layout;
    let port_index = build_port_index(module, &ctx.node_brick_ids);
    // Single-driver invariant: a real input port accepts exactly one source.
    // Records the source already drawn into each (target, port) so a second,
    // DISTINCT source is caught as a fan-in below.
    let mut driver: HashMap<(NodeId, WirePort), (NodeId, WirePort)> = HashMap::default();
    for w in &module.wires {
        if w.source.port == layout_port_id || w.target.port == layout_port_id {
            continue;
        }
        // A bus lane already carries this value to the port; drawing the
        // direct wire too would fan in and the game would reject one.
        if layout
            .bus
            .suppressed
            .contains(&(w.target.node_id, w.target.port))
        {
            continue;
        }
        let src_class = ctx.class_index.get(&w.source.node_id);
        if matches!(src_class, Some(c) if *c == gc::LITERAL || *c == gc::UNSUPPORTED) {
            continue;
        }
        let dst_class = ctx.class_index.get(&w.target.node_id);
        if matches!(dst_class, Some(c) if *c == gc::LITERAL || *c == gc::UNSUPPORTED) {
            continue;
        }
        // Two DISTINCT sources into one input port load-fail the whole save. An
        // exact-duplicate wire (same source) is drawn as before; bus-lane fan-in
        // is already suppressed above, and literal/unsupported sources were
        // skipped (they inline, not wire).
        let tgt_key = (w.target.node_id, w.target.port);
        let src_key = (w.source.node_id, w.source.port);
        match driver.get(&tgt_key) {
            Some(&existing) if existing != src_key => {
                return Err(EmitError::FanIn(format!(
                    "{}.{} is driven by two sources: {}.{} and {}.{} (a lowering bug — \
                     the save would fail to load)",
                    w.target.node_id,
                    w.target.port.as_str(),
                    existing.0,
                    existing.1.as_str(),
                    w.source.node_id,
                    w.source.port.as_str(),
                )));
            }
            Some(_) => {}
            None => {
                driver.insert(tgt_key, src_key);
            }
        }
        match wire_to_connection_indexed(w, &ctx.node_brick_ids, &ctx.class_index, &port_index) {
            Ok(conn) => world.add_wire(conn),
            Err(e) => {
                // FATAL — see `EmitError::DroppedWire`. A wire that can't be
                // drawn means a value never arrives and nothing downstream can
                // tell; better a compile error than a silently-wrong `.brz`.
                // `seen` distinguishes "node exists but never got a brick"
                // (its module was visited; the node is non-spawnable or
                // skipped) from "its module was never emitted at all" —
                // the key discriminator when diagnosing dropped wires.
                let seen = |id: &NodeId| {
                    if ctx.node_brick_ids.contains_key(id) {
                        "brick"
                    } else if ctx.class_index.contains_key(id) {
                        "seen-no-brick"
                    } else {
                        "never-visited"
                    }
                };
                return Err(EmitError::DroppedWire(format!(
                    "{} → {} (port {}→{}): {e} (src: {}, dst: {})",
                    w.source.node_id,
                    w.target.node_id,
                    w.source.port.as_str(),
                    w.target.port.as_str(),
                    seen(&w.source.node_id),
                    seen(&w.target.node_id),
                )));
            }
        }
    }

    // ── Pass 3.5: dynamic label wires ──
    // A runtime `@label(expr)` on a var drives its floating name label instead
    // of baking a static string: wire the (already string-coerced) value into
    // the label `Component_TextDisplay`'s `Text` input port. The label brick was
    // emitted normally (its baked name is the pre-wire placeholder). Skipped
    // under `@invisible`, which emits no labels for the wire to target.
    if !ctx.invisible {
        for (host, src) in &module.dynamic_labels {
            if matches!(ctx.class_index.get(&src.node_id), Some(c) if *c == gc::LITERAL || *c == gc::UNSUPPORTED)
            {
                continue;
            }
            let source = match resolve_wire_end(
                src.node_id,
                src.port,
                &ctx.node_brick_ids,
                &ctx.class_index,
                &port_index,
            ) {
                Ok(s) => s,
                // FATAL — same class as a dropped module wire: a `@label` that
                // asks for a runtime value but silently gets none is exactly
                // the miscompile-laundering `EmitError::DroppedWire` guards.
                Err(e) => {
                    return Err(EmitError::DroppedWire(format!(
                        "@label source for {}: {e}",
                        src.node_id
                    )));
                }
            };
            let Some(&host_brick) = ctx.node_brick_ids.get(host) else {
                continue;
            };
            world.add_wire(WireConnection {
                source,
                target: BrdbWirePort {
                    brick_id: host_brick,
                    component_type: BString::Static("Component_TextDisplay"),
                    port_name: BString::Static("Text"),
                },
            });
        }
        // Module-level dynamic `@label`: wire the value into the ROOT shell
        // brick's label `Text` port (the shell hosts a placeholder-empty
        // TextDisplay emitted in `build_world`). Root module only — chip
        // sub-modules leave `root_dynamic_label` unset.
        if let Some(src) = &module.root_dynamic_label {
            if !matches!(ctx.class_index.get(&src.node_id), Some(c) if *c == gc::LITERAL || *c == gc::UNSUPPORTED)
            {
                match resolve_wire_end(
                    src.node_id,
                    src.port,
                    &ctx.node_brick_ids,
                    &ctx.class_index,
                    &port_index,
                ) {
                    Ok(source) => world.add_wire(WireConnection {
                        source,
                        target: BrdbWirePort {
                            brick_id: ctx.root_shell_brick_id,
                            component_type: BString::Static("Component_TextDisplay"),
                            port_name: BString::Static("Text"),
                        },
                    }),
                    // FATAL — a root `@label` that loses its runtime source
                    // would ship a chip with a blank title and no diagnostic.
                    Err(e) => {
                        return Err(EmitError::DroppedWire(format!(
                            "root @label source: {e}"
                        )));
                    }
                }
            }
        }
    }

    // ── Pass 3b: the bus lanes' own wires ──
    // A lane hop reads a rerouter's `RER_Output` and drives the next one's
    // `RER_Input`; a `BusEnd::Node` end resolves exactly like a module wire
    // end, so a tap on a chip port goes through the same remap.
    let bus_end = |e: BusEnd, as_source: bool| -> Result<BrdbWirePort, EmitError> {
        match e {
            BusEnd::Bus(i) => {
                let brick_id = *bus_brick_ids
                    .get(i)
                    .ok_or_else(|| EmitError::UnknownWireNode(format!("bus node {i}")))?;
                Ok(BrdbWirePort {
                    brick_id,
                    component_type: BString::Static(gc::REROUTER),
                    port_name: BString::Static(if as_source {
                        "RER_Output"
                    } else {
                        "RER_Input"
                    }),
                })
            }
            BusEnd::Node(p) => resolve_wire_end(
                p.node_id,
                p.port,
                &ctx.node_brick_ids,
                &ctx.class_index,
                &port_index,
            ),
        }
    };
    // Name an end the way a reader can act on: a lane brick by its index,
    // rotation and cell, a real end by its node and port.
    let describe = |e: BusEnd, as_source: bool| -> String {
        match e {
            BusEnd::Bus(i) => match layout.bus.nodes.get(i) {
                Some(n) => format!(
                    "bus node {i} .{} ({:?} at {},{},{})",
                    if as_source { "RER_Output" } else { "RER_Input" },
                    n.rotation,
                    n.x,
                    n.y,
                    n.z
                ),
                None => format!("bus node {i} (out of range)"),
            },
            BusEnd::Node(p) => format!("{} .{}", p.node_id, p.port.as_str()),
        }
    };
    for bw in &layout.bus.wires {
        // Unlike pass 3, an unresolvable end here is FATAL. Pass 3's wire was
        // going to be drawn anyway, so dropping it loses a wire the reader can
        // see missing; a bus wire's original was suppressed and will never be
        // redrawn, so dropping it silently strands the consumer forever.
        let (source, target) = match (bus_end(bw.source, true), bus_end(bw.target, false)) {
            (Ok(s), Ok(t)) => (s, t),
            (src, tgt) => {
                let cause = src.err().or(tgt.err());
                return Err(EmitError::BusWireUnresolved {
                    from: describe(bw.source, true),
                    to: describe(bw.target, false),
                    cause: match cause {
                        Some(e) => e.to_string(),
                        None => "unknown".to_string(),
                    },
                });
            }
        };
        world.add_wire(WireConnection { source, target });
    }

    Ok(())
}
