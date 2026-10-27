//! The recursive per-module emit driver.

use std::collections::HashMap as StdMap;

use brdb::{
    AsBrdbValue, BString, BrickType, IntVector, WireConnection, WirePort as BrdbWirePort, World,
    assets::LiteralComponent, schema::WireValue,
};

use crate::catalog::default_catalog;
use crate::collections::HashMap;
use crate::intern::{Sym, intern_static, resolve, sym};
use crate::ir::port_registry::WirePort;
use crate::ir::{Literal, Module, Node, NodeId, PortRef, ProgramSettings, Type, gate_class as gc};
use crate::layout::wall::WallLayout;
use crate::layout::{BusEnd, LayoutResult};

use super::WALL_ROT;
use super::colors::color_for_node;
use super::components::{
    build_adv_inventory_component, build_function_call_component, build_gate_component,
    inlined_values, one_field_component,
};
use super::labels::{
    LABEL_LINE_HEIGHT, TEXT_DISPLAY, VAR_TAG_LINE_HEIGHT, chip_display_name, chip_is_closed,
    chip_name_label, emit_annotations, emit_plane_header, label_rotation_deg, microchip_io_label,
    text_label,
};
use super::options::{EmitError, NestedCompiler, PrefabResolver};
use super::styles::{apply_style, node_style, property_changer_port};
use super::variants::{
    array_element_type, array_from_literals, literal_to_wire_value, map_from_literals,
    map_from_type, var_type_to_wire_value,
};
use super::wires::{
    PortIndex, REROUTE_NODE_ASSET, brick_port, build_port_index, emit_bus, is_spawnable,
    rer_port_name, resolve_wire_end, wire_to_connection_indexed,
};

pub(super) struct EmitContext {
    pub(super) node_brick_ids: HashMap<NodeId, usize>,
    pub(super) class_index: HashMap<NodeId, &'static str>,
    /// Resolver for `$./file.brz` prefab references, from `EmitOptions`.
    pub(super) prefab_resolver: Option<PrefabResolver>,
    /// Compiler for inline nested-prefab blocks, from `EmitOptions`.
    pub(super) nested_compiler: Option<NestedCompiler>,
    /// (target node, target port) -> source node, accumulated across all
    /// modules so `Var_Get/Set` gates can trace `VarRef` wires that cross
    /// module boundaries (scope captures, anon-chip partitions).
    pub(super) wire_sources: HashMap<(NodeId, WirePort), NodeId>,
    /// `Pseudo_Var/ArrayVar` node -> its labelable source name. Vars are
    /// always emitted before the gates that reference them.
    pub(super) var_labels: HashMap<NodeId, String>,
    /// Module-level `@invisible`: suppresses var-tag and I/O-gate labels in
    /// `emit_module`.
    pub(super) invisible: bool,
    /// Module-level `@layout("cube")`: suppresses the per-gate name labels and
    /// var tags in `emit_module`.
    pub(super) no_gate_labels: bool,
    /// The root microchip shell brick's id. Pass 3.5 wires a module-level
    /// dynamic `@label` (`Module.root_dynamic_label`) into this brick's label
    /// `Text` port.
    pub(super) root_shell_brick_id: usize,
    /// The source already drawn into each brick input port, across the WHOLE
    /// world. See [`EmitContext::add_wire`].
    pub(super) drivers: HashMap<(usize, BString, BString), (usize, BString, BString)>,
}

impl EmitContext {
    /// An empty context; `root_shell_brick_id` is the root microchip shell's brick.
    pub(super) fn new(
        prefab_resolver: Option<PrefabResolver>,
        nested_compiler: Option<NestedCompiler>,
        settings: &ProgramSettings,
        root_shell_brick_id: usize,
    ) -> Self {
        Self {
            node_brick_ids: HashMap::default(),
            class_index: HashMap::default(),
            prefab_resolver,
            nested_compiler,
            wire_sources: HashMap::default(),
            var_labels: HashMap::default(),
            invisible: settings.invisible,
            no_gate_labels: settings.no_gate_labels,
            root_shell_brick_id,
            drivers: HashMap::default(),
        }
    }

    /// Draw one wire, refusing a second distinct source into an input port.
    ///
    /// Two sources on one input port make the game reject the whole save at
    /// load. Keyed on the BRICK port, the one key the bus lanes, `@side`
    /// rerouters and `@label` wires share with ordinary wires. An exact
    /// duplicate is drawn once.
    pub(super) fn add_wire(
        &mut self,
        world: &mut World,
        conn: WireConnection,
    ) -> Result<(), EmitError> {
        let key = (
            conn.target.brick_id,
            conn.target.component_type.clone(),
            conn.target.port_name.clone(),
        );
        let src = (
            conn.source.brick_id,
            conn.source.component_type.clone(),
            conn.source.port_name.clone(),
        );
        check_ends(&conn)?;
        match self.drivers.get(&key) {
            Some(existing) if *existing != src => {
                return Err(EmitError::FanIn(format!(
                    "brick {} port {}.{} is driven by two sources: brick {} {}.{} and \
                     brick {} {}.{} (a lowering bug, the save would fail to load)",
                    key.0, key.1, key.2, existing.0, existing.1, existing.2, src.0, src.1, src.2,
                )));
            }
            Some(_) => return Ok(()),
            None => {
                self.drivers.insert(key, src);
            }
        }
        world.add_wire(conn);
        Ok(())
    }
}

/// Both ends of a wire must name a port the gate class actually has, on the
/// right side of it. The catalog (`data/logic_gate_inventory.simple.json`) is
/// the authority: it is the game's own dump. A class the catalog does not know
/// answers OK, matching `Catalog::is_wire_input`'s fail-open default.
///
/// The game refuses a wire naming a port it does not have ("Failed to connect
/// wire" at load), and `wires::resolve_wire_end` copies IR port names through
/// verbatim, so this is the last place to catch one.
fn check_ends(conn: &WireConnection) -> Result<(), EmitError> {
    let cat = default_catalog();
    for (is_src, ep) in [(true, &conn.source), (false, &conn.target)] {
        let class = ep.component_type.as_ref();
        let Some(spec) = cat.find_by_class(class) else {
            continue;
        };
        let port = ep.port_name.as_ref();
        // A composite sub-port (`Position.X`) is named by its parent in the
        // inventory.
        let base = port.split('.').next().unwrap_or(port);
        let is_in = spec
            .component
            .inputs
            .iter()
            .any(|p| p.name == port || p.name == base);
        let is_out = spec
            .component
            .outputs
            .iter()
            .any(|p| p.name == port || p.name == base);
        if !is_in && !is_out {
            return Err(EmitError::UnknownPort(format!(
                "{class} has no port `{port}` (catalog inputs {:?}, outputs {:?})",
                spec.component
                    .inputs
                    .iter()
                    .map(|p| &p.name)
                    .collect::<Vec<_>>(),
                spec.component
                    .outputs
                    .iter()
                    .map(|p| &p.name)
                    .collect::<Vec<_>>(),
            )));
        }
        if is_src && !is_out {
            return Err(EmitError::UnknownPort(format!(
                "{class}.{port} drives a wire but is an INPUT port"
            )));
        }
        if !is_src && !is_in {
            return Err(EmitError::UnknownPort(format!(
                "{class}.{port} is wired into but is an OUTPUT port"
            )));
        }
    }
    Ok(())
}

/// Resolve a wire source to its var/array-var label, following the source
/// through any number of `MicrochipInput`/`MicrochipOutput` boundary pins
/// (inserted by the boundary-pins pass for wires that cross a chip
/// boundary) and `Rerouter` hops back to the originating
/// `Pseudo_Var`/`Pseudo_ArrayVar` node. Bus lane rerouters have no IR node,
/// so the walk ends at one. Bounded so a malformed graph cannot spin forever.
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

/// `_Literal` values wired into each node of `module`, by target node, in
/// wire order. Emit inlines them into their consumers' data.
type InlinedByNode = HashMap<NodeId, Vec<(WirePort, Literal)>>;

pub(super) fn emit_module(
    world: &mut World,
    ctx: &mut EmitContext,
    module: &Module,
    layout: &LayoutResult,
    bricks: &mut Vec<brdb::Brick>,
    wall: &WallLayout,
) -> Result<(), EmitError> {
    let value_sym = *sym::VALUE;
    let mut inlined_by_node: InlinedByNode = HashMap::default();
    let mut wire_target_index: HashMap<(NodeId, WirePort), NodeId> = HashMap::default();
    for w in &module.wires {
        let src_node = module.nodes.get(&w.source.node_id);
        if src_node.map(|n| n.gate_class) == Some(gc::LITERAL)
            && let Some(lit) = src_node.and_then(|n| n.properties.get(&value_sym)).cloned()
        {
            inlined_by_node
                .entry(w.target.node_id)
                .or_default()
                .push((w.target.port, lit));
        }
        wire_target_index.insert((w.target.node_id, w.target.port), w.source.node_id);
        ctx.wire_sources
            .insert((w.target.node_id, w.target.port), w.source.node_id);
    }

    let nodes: Vec<(&NodeId, &Node)> = module.nodes_in_order().collect();

    // Register ALL nodes in class_index (including _Literal, _Unsupported)
    // so the wire filter in pass 3 can identify and skip them. Only spawnable
    // nodes get bricks.
    for (id, node) in &nodes {
        ctx.class_index.insert(**id, node.gate_class);
    }

    // Pass 1: emit all non-chip gates.
    // PseudoVar/PseudoArrayVar bricks are registered before any child
    // module tries to wire to them via scope captures.
    for &(id, node) in &nodes {
        if is_spawnable(node.kind, node.gate_class) {
            emit_gate(
                world,
                ctx,
                module,
                layout,
                bricks,
                &inlined_by_node,
                &wire_target_index,
                *id,
                node,
            )?;
        }
    }

    emit_annotations(world, bricks, &layout.annotations);
    // Lane bricks here; their wires wait for pass 3b, after the chip-port
    // index a `BusEnd::Node` tap may need has been built.
    let bus_brick_ids = emit_bus(bricks, &layout.bus, module, &wire_target_index);

    // Pass 2: recursively emit chip children.
    for &(id, node) in &nodes {
        if node.kind != crate::ir::NodeKind::Chip {
            continue;
        }
        let Some(child_module) = module.chips.get(id) else {
            continue;
        };
        emit_chip(
            world,
            ctx,
            module,
            layout,
            bricks,
            wall,
            &inlined_by_node,
            *id,
            node,
            child_module,
        )?;
    }

    // Pass 3: emit this module's wires.
    let port_index = build_port_index(module, &ctx.node_brick_ids);
    emit_module_wires(world, ctx, module, layout, &port_index)?;

    // Pass 3.5: dynamic label wires. A runtime `@label(expr)` wires its
    // string value into the label `Component_TextDisplay`'s `Text` input; a
    // module-level one targets the root shell's label from `build_world`.
    // Skipped under `@invisible`, which emits no labels to target.
    if !ctx.invisible {
        // In host id order: the wires' order reaches the save, so it must not
        // depend on how the map was built.
        let mut labels: Vec<(&NodeId, &PortRef)> = module.dynamic_labels.iter().collect();
        labels.sort_unstable_by_key(|(host, _)| **host);
        for (host, src) in labels {
            let Some(source) = label_source(ctx, *src, &port_index) else {
                continue;
            };
            // FATAL, like a dropped module wire: a `@label` that silently
            // gets no runtime value is a miscompile.
            let source = source.map_err(|e| {
                EmitError::DroppedWire(format!("@label source for {}: {e}", src.node_id))
            })?;
            let Some(&host_brick) = ctx.node_brick_ids.get(host) else {
                continue;
            };
            ctx.add_wire(
                world,
                WireConnection {
                    source,
                    target: label_text_port(host_brick),
                },
            )?;
        }
        if let Some(src) = &module.root_dynamic_label
            && let Some(source) = label_source(ctx, *src, &port_index)
        {
            // FATAL: a root `@label` that loses its runtime source would
            // ship a chip with a blank title and no diagnostic.
            let source =
                source.map_err(|e| EmitError::DroppedWire(format!("root @label source: {e}")))?;
            let target = label_text_port(ctx.root_shell_brick_id);
            ctx.add_wire(world, WireConnection { source, target })?;
        }
    }

    // Pass 3.5, continued: runtime `@color` / `@material` wires into the
    // Brick Property Changer pass 1 put on the host's brick (or `build_world`
    // on the root shell). Not skipped under `@invisible`: the changer exists
    // either way. An inlined source was baked into the changer instead.
    let mut styles: Vec<(NodeId, &'static str, &PortRef)> = module
        .dynamic_colors
        .iter()
        .map(|(host, src)| (*host, "Color", src))
        .chain(
            module
                .dynamic_materials
                .iter()
                .map(|(host, src)| (*host, "Material", src)),
        )
        .collect();
    styles.sort_unstable_by_key(|(host, port, _)| (*host, *port));
    for (host, port, src) in styles {
        let Some(source) = label_source(ctx, *src, &port_index) else {
            continue;
        };
        // FATAL for the same reason as a lost `@label` source.
        let source = source.map_err(|e| {
            EmitError::DroppedWire(format!("@{} source for {host}: {e}", port.to_lowercase()))
        })?;
        let Some(&host_brick) = ctx.node_brick_ids.get(&host) else {
            continue;
        };
        ctx.add_wire(
            world,
            WireConnection {
                source,
                target: property_changer_port(host_brick, port),
            },
        )?;
    }
    for (port, value) in [
        ("Color", &module.root_style.color),
        ("Material", &module.root_style.material),
    ] {
        if let Some(src) = value.as_ref().and_then(crate::ir::StyleValue::wired)
            && let Some(source) = label_source(ctx, *src, &port_index)
        {
            let source = source.map_err(|e| {
                EmitError::DroppedWire(format!("root @{} source: {e}", port.to_lowercase()))
            })?;
            let target = property_changer_port(ctx.root_shell_brick_id, port);
            ctx.add_wire(world, WireConnection { source, target })?;
        }
    }

    // Pass 3b: the bus lanes' own wires.
    emit_bus_wires(world, ctx, layout, &bus_brick_ids, &port_index)
}

/// Pass 1 for one node: its gate brick, its component, and its floating
/// label.
#[expect(
    clippy::too_many_arguments,
    reason = "one gate needs the module, layout and every emit-wide index"
)]
fn emit_gate(
    world: &mut World,
    ctx: &mut EmitContext,
    module: &Module,
    layout: &LayoutResult,
    bricks: &mut Vec<brdb::Brick>,
    inlined_by_node: &InlinedByNode,
    wire_target_index: &HashMap<(NodeId, WirePort), NodeId>,
    id: NodeId,
    node: &Node,
) -> Result<(), EmitError> {
    let pos = layout
        .placements
        .get(&id)
        .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;
    // Brick asset and half-size come from one catalog entry. Unknown classes
    // (synthetic IR-only nodes) fall back to a reroute node.
    let catalog_entry = default_catalog().find_by_class(node.gate_class);
    let brick_asset: &'static str = catalog_entry
        .map(|g| g.brick_asset.as_str())
        .unwrap_or(REROUTE_NODE_ASSET);
    // Offset each brick by its half-size so its min corner sits at the cell
    // corner (pos.x, pos.y), keeping bricks of any size inside their cells.
    // A procedural gate (`PB_Gate_*`) is sized to the variant lowering chose;
    // the game rejects these assets at any size that is not a variant.
    let placed = catalog_entry.map(|g| g.placed_half_size(crate::catalog::node_variant(node)));
    let half = placed.map_or((5, 5), |h| (h.x, h.y));
    let asset = match (catalog_entry, placed) {
        (Some(g), Some(h)) if g.is_procedural() => {
            BrickType::from((brick_asset, (h.x as u16, h.y as u16, h.z as u16)))
        }
        _ => BrickType::from(brick_asset),
    };
    // A quarter-turned brick swaps its footprint, as layout reserved it.
    // `brdb::Brick::local_bounds()` is rotation-blind, so a mismatch here goes
    // uncaught and the game drops the overlapping bricks at load.
    let rotation = layout.rotations.get(&id).copied().unwrap_or_default();
    let (offset_x, offset_y) = rotation.footprint(half);
    let inner_pos = brdb::Position {
        x: pos.x + offset_x,
        y: pos.y + offset_y,
        z: pos.z,
    };
    let (mut brick, brick_id) = brdb::Brick {
        asset,
        position: inner_pos,
        rotation: rotation.into(),
        color: color_for_node(node, module, wire_target_index),
        ..Default::default()
    }
    .with_id_split();

    // Inject node properties (e.g. InitialValue, Value) into inlined
    // so data structs carry the correct wire_graph_variant type.
    let gate_inlined = node_inlined(inlined_by_node, id, node);
    brick.add_component_box(node_component(world, ctx, node, &gate_inlined)?);

    if let Some((text, line_height)) = node_label(ctx, module, id, node) {
        // `no_gate_labels` (cube layout) drops these labels. A runtime
        // `@label(expr)` is exempt: pass 3.5 wires into this component, and a
        // dangling wire fails at load.
        let dynamic = module.dynamic_labels.contains_key(&id);
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

    if let Some((color, material, wired)) = node_style(module, id, node) {
        apply_style(&mut brick, color.as_ref(), material.as_ref(), wired);
    }

    bricks.push(brick);
    ctx.node_brick_ids.insert(id, brick_id);
    ctx.class_index.insert(id, node.gate_class);
    Ok(())
}

/// The literals inlined into `id`'s data: the `_Literal` values wired into
/// it, then its own properties for the names no wire fills.
fn node_inlined<'a>(
    inlined_by_node: &'a InlinedByNode,
    id: NodeId,
    node: &'a Node,
) -> HashMap<Sym, &'a Literal> {
    inlined_values(
        inlined_by_node
            .get(&id)
            .into_iter()
            .flatten()
            .map(|(port, lit)| (crate::intern::intern(port.as_str()), lit)),
        node.properties.iter().map(|(k, v)| (*k, v)),
    )
}

/// The component a gate brick carries: its class's data, built from the
/// node's ports and the literals inlined into it.
fn node_component(
    world: &mut World,
    ctx: &EmitContext,
    node: &Node,
    gate_inlined: &HashMap<Sym, &Literal>,
) -> Result<Box<dyn brdb::BrdbComponent>, EmitError> {
    let class = node.gate_class;
    // The advanced-inventory gates carry composite (array/struct) config
    // (`MeshColors: Color[]`, `WeaponAmmoOverride`) that a `LiteralComponent`
    // can't serialize, so they are built as an array-capable `NativeStruct`.
    if class == gc::CHARACTER_ADD_INVENTORY_ITEM_ADV
        || class == gc::CHARACTER_SET_INVENTORY_ITEM_ADV
    {
        return Ok(Box::new(build_adv_inventory_component(
            class,
            &node.ports,
            gate_inlined,
            world,
            ctx.prefab_resolver.as_ref(),
            ctx.nested_compiler.as_ref(),
        )?));
    }
    if class == gc::FUNCTION_CALL {
        return Ok(Box::new(build_function_call_component(
            &node.ports,
            gate_inlined,
            world,
            ctx.prefab_resolver.as_ref(),
            ctx.nested_compiler.as_ref(),
        )?));
    }
    Ok(Box::new(match class {
        // Var: a `WireValue` typed by the Value port.
        gc::VAR => {
            let value_ty = node.ports.find_port(*sym::VALUE).map(|p| &p.ty);
            let wv: WireValue = match node
                .properties
                .get(&*sym::INITIAL_VALUE)
                .or_else(|| node.properties.get(&*sym::VALUE))
            {
                Some(lit) => literal_to_wire_value(lit).unwrap_or(WireValue::Number(0.0)),
                None => var_type_to_wire_value(value_ty),
            };
            one_field_component(class, "Value", Box::new(wv))
        }
        // ArrayVar: a `dynamic_array` whose element type follows the
        // declared element type, so the array stores the right scalar kind
        // (int/bool/string/vector/object) instead of defaulting to doubles.
        gc::ARRAY_VAR => {
            let elem_ty = node
                .ports
                .outputs
                .iter()
                .find(|p| p.name == *sym::ARRAY_VAR_REF)
                .and_then(|p| array_element_type(&p.ty));
            // A constant initializer is carried as an `InitialValue` list
            // literal; otherwise the array starts empty.
            let av = match node.properties.get(&*sym::INITIAL_VALUE) {
                Some(Literal::Array(lits)) => array_from_literals(elem_ty, lits),
                _ => array_from_literals(elem_ty, &[]),
            };
            one_field_component(class, "Value", Box::new(av))
        }
        // MapVar: a `dynamic_map` whose key/value types follow the
        // declared `Map<K, V>` on the MapVarRef port. A constant
        // initializer is carried as an `InitialValue` map literal and
        // populates the entries; otherwise the map starts empty (runtime
        // `set`s populate it).
        gc::MAP_VAR => {
            let empty = node
                .ports
                .outputs
                .iter()
                .find(|p| p.name == *sym::MAP_VAR_REF)
                .map(|p| map_from_type(&p.ty))
                .unwrap_or_else(|| map_from_type(&Type::Any));
            let mv = match node.properties.get(&*sym::INITIAL_VALUE) {
                Some(Literal::Map(entries)) => map_from_literals(&empty, entries),
                _ => empty,
            };
            one_field_component(class, "Value", Box::new(mv))
        }
        // MicrochipInput/Output: PortLabel string.
        gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT => {
            let label = match node.properties.get(&*sym::PORT_LABEL) {
                Some(Literal::String(s)) => s.clone(),
                _ => String::new(),
            };
            one_field_component(class, "PortLabel", Box::new(label))
        }
        // SetInventoryEntry: the weapon is a nested asset in
        // EntryPlan.ItemTypeIfItem. Register the asset in the world's
        // external-asset table and build the EntryPlan struct (Type=Item).
        // NOTE: the exact binary encoding (enum repr, asset-in-class field,
        // nested defaults) needs in-game verification.
        gc::CHARACTER_SET_INVENTORY_ENTRY => set_inventory_entry_component(world, node),
        _ => build_gate_component(
            class,
            &node.ports,
            gate_inlined,
            world,
            ctx.prefab_resolver.as_ref(),
            ctx.nested_compiler.as_ref(),
        )?,
    }))
}

fn set_inventory_entry_component(world: &mut World, node: &Node) -> LiteralComponent {
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
    // member struct -> just its variant tag).
    let entry = LiteralComponent::new("BRInventoryEntryConfig").with_data([(
        "Item",
        Box::new(LiteralComponent::new("BRInventoryEntryNothing")) as Box<dyn AsBrdbValue>,
    )]);
    let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    data.insert("Slot".into(), Box::new(slot));
    data.insert("Entry".into(), Box::new(entry));
    data.insert("EntryPlan".into(), Box::new(entry_plan));
    LiteralComponent::new_from_data(node.gate_class, std::sync::Arc::new(data))
}

/// The floating name label a gate brick carries, as `(text, line height)`:
/// on I/O gates and variables their name, on a Function Call its `fn`, and on
/// a container access gate a smaller tag naming the container. Records the
/// names var access gates later tag themselves with.
///
/// The label floats above the gate brick (Offset.z +0.5; the chip-shell value
/// of -0.5 sinks it into these bricks). Chip bricks get theirs in pass 2 and
/// `build_world`. `_`-prefixed names are synthesized plumbing: unlabeled,
/// except well-known chip I/O names (`microchip_io_label`).
fn node_label(
    ctx: &mut EmitContext,
    module: &Module,
    id: NodeId,
    node: &Node,
) -> Option<(String, f32)> {
    let named = |l: &Literal| match l {
        Literal::String(s) if !s.is_empty() && !s.starts_with('_') => Some(s.clone()),
        _ => None,
    };
    match node.gate_class {
        gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT | gc::FUNCTION_INPUT | gc::FUNCTION_OUTPUT => {
            let label = microchip_io_label(node);
            // Record the pin's name so a container access gate on a
            // reference-passed input (`in xs: T[]`) is tagged with it.
            if let Some(name) = &label {
                ctx.var_labels.insert(id, name.clone());
            }
            label.map(|s| (s, LABEL_LINE_HEIGHT))
        }
        gc::VAR | gc::ARRAY_VAR | gc::MAP_VAR => {
            let label = node.properties.get(&*sym::NAME_LABEL).and_then(named);
            // The var's own name still drives the small tags on its
            // Var_Get/Set gates (via var_labels), even when its own big
            // label is a dynamic, wire-driven one below.
            if let Some(name) = &label {
                ctx.var_labels.insert(id, name.clone());
            }
            if module.dynamic_labels.contains_key(&id) {
                // A dynamic `@label(expr)` wires this label (pass 3.5), so
                // the text component must exist even when the name is
                // suppressed; a wire into a missing component fails at load.
                Some((String::new(), LABEL_LINE_HEIGHT))
            } else {
                label.map(|s| (s, LABEL_LINE_HEIGHT))
            }
        }
        // A Function Call: the name of the `fn` it runs.
        gc::FUNCTION_CALL => node
            .properties
            .get(&*sym::NAME_LABEL)
            .and_then(named)
            .map(|s| (s, LABEL_LINE_HEIGHT)),
        // Var/ArrayVar exec gates: a smaller tag naming the variable
        // they access, traced through the gate's (Array)VarRef wire.
        // The var node is always emitted first, so its label is known.
        c if gc::is_container_access(c) => node
            .ports
            .inputs
            .iter()
            .find(|p| [*sym::VAR_REF, *sym::ARRAY_VAR_REF, *sym::MAP_VAR_REF].contains(&p.name))
            .and_then(|p| {
                let port = WirePort::from_name(resolve(p.name));
                ctx.wire_sources.get(&(node.id, port))
            })
            .and_then(|src| resolve_var_label(ctx, *src))
            .map(|s| (s.clone(), VAR_TAG_LINE_HEIGHT)),
        _ => None,
    }
}

/// Pass 2 for one chip: its body's grid (emitted recursively), the plane
/// header, and the chip brick in this module with its shell component and
/// name label.
#[expect(
    clippy::too_many_arguments,
    reason = "one chip needs the module, layout, wall and every emit-wide index"
)]
fn emit_chip(
    world: &mut World,
    ctx: &mut EmitContext,
    module: &Module,
    layout: &LayoutResult,
    bricks: &mut Vec<brdb::Brick>,
    wall: &WallLayout,
    inlined_by_node: &InlinedByNode,
    id: NodeId,
    node: &Node,
    child_module: &Module,
) -> Result<(), EmitError> {
    let missing = || EmitError::MissingPlacement(id.to_string());
    let pos = layout.placements.get(&id).ok_or_else(missing)?;
    let inner_pos = brdb::Position {
        x: pos.x + 5,
        y: pos.y + 5,
        z: pos.z,
    };
    let child_layout = layout.chip_layouts.get(&id).ok_or_else(missing)?;
    let slot = wall.chips.get(&id).ok_or_else(missing)?;

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

    // The game rejects an entry gate placed on a plain microchip, so a
    // loop shell needs both its own brick asset and its own component
    // class; getting either one wrong still fails the load.
    let spec = crate::ir::plane::plane_spec_for_class(node.gate_class);
    let (shell_asset, shell_component) = match spec {
        Some(s) => (BrickType::from(s.brick_asset), node.gate_class),
        None => (brdb::assets::bricks::B_MICROCHIP, gc::MICROCHIP_ALT),
    };

    // A loop shell has data pins where a plain microchip has none, so a
    // bare component would leave every constant pin at its default. A
    // single-consumer literal is folded into the target's properties and its
    // wire deleted before emit, so the properties are where a constant pin
    // usually arrives.
    let shell_comp: Box<dyn brdb::BrdbComponent> = if spec.is_some() {
        Box::new(build_gate_component(
            shell_component,
            &node.ports,
            &node_inlined(inlined_by_node, id, node),
            world,
            ctx.prefab_resolver.as_ref(),
            ctx.nested_compiler.as_ref(),
        )?)
    } else {
        Box::new(LiteralComponent::new(shell_component))
    };

    let (mut chip_brick, chip_brick_id) = brdb::Brick {
        asset: shell_asset,
        position: inner_pos,
        ..Default::default()
    }
    .with_component_box(shell_comp)
    .with_id_split();
    // Named chips get a floating name label (the `@label` override wins);
    // anonymous partitions stay unlabeled. Dropped under `no_gate_labels`
    // like gate labels; the plane header above is kept.
    if let Some(name) = chip_display_name(node, child_module).filter(|_| !ctx.no_gate_labels) {
        chip_brick.add_component_box(Box::new(chip_name_label(world, &name)));
    }
    if let Some((color, material, wired)) = node_style(module, id, node) {
        apply_style(&mut chip_brick, color.as_ref(), material.as_ref(), wired);
    }
    bricks.push(chip_brick);
    ctx.node_brick_ids.insert(id, chip_brick_id);
    ctx.class_index.insert(id, node.gate_class);
    world.register_microchip_link(chip_brick_id, chip_entity_id);
    Ok(())
}

/// Pass 3: this module's wires.
fn emit_module_wires(
    world: &mut World,
    ctx: &mut EmitContext,
    module: &Module,
    layout: &LayoutResult,
    port_index: &PortIndex,
) -> Result<(), EmitError> {
    let inline_only = |ctx: &EmitContext, id: &NodeId| {
        ctx.class_index
            .get(id)
            .is_some_and(|c| gc::is_inline_only(c))
    };
    for w in &module.wires {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
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
        if inline_only(ctx, &w.source.node_id) || inline_only(ctx, &w.target.node_id) {
            continue;
        }
        // `EmitContext::add_wire` refuses fan-in, which load-fails the save.
        match wire_to_connection_indexed(w, &ctx.node_brick_ids, &ctx.class_index, port_index) {
            Ok(conn) => ctx.add_wire(world, conn)?,
            Err(e) => {
                // FATAL (see `EmitError::DroppedWire`): a value that never
                // arrives is undetectable downstream. `seen` tells "node got no
                // brick" apart from "its module was never emitted".
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
                    "{} -> {} (port {}->{}): {e} (src: {}, dst: {})",
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
    Ok(())
}

/// The source end of a runtime `@label`'s wire, or `None` when its value is a
/// literal or placeholder that emit inlines, with nothing to wire.
fn label_source(
    ctx: &EmitContext,
    src: PortRef,
    port_index: &PortIndex,
) -> Option<Result<BrdbWirePort, EmitError>> {
    if ctx
        .class_index
        .get(&src.node_id)
        .is_some_and(|c| gc::is_inline_only(c))
    {
        return None;
    }
    Some(resolve_wire_end(
        src.node_id,
        src.port,
        &ctx.node_brick_ids,
        &ctx.class_index,
        port_index,
    ))
}

/// The `Text` input of the label on `brick_id`.
fn label_text_port(brick_id: usize) -> BrdbWirePort {
    brick_port(brick_id, TEXT_DISPLAY, "Text")
}

/// Pass 3b: the bus lanes' own wires. A lane hop reads a rerouter's
/// `RER_Output` and drives the next one's `RER_Input`; a `BusEnd::Node` end
/// resolves exactly like a module wire end, so a tap on a chip port goes
/// through the same remap.
fn emit_bus_wires(
    world: &mut World,
    ctx: &mut EmitContext,
    layout: &LayoutResult,
    bus_brick_ids: &[usize],
    port_index: &PortIndex,
) -> Result<(), EmitError> {
    // Takes the two index maps rather than `ctx`, so the loop below can still
    // borrow `ctx` mutably to draw the wire it resolves.
    let bus_end = |e: BusEnd,
                   as_source: bool,
                   node_brick_ids: &HashMap<NodeId, usize>,
                   class_index: &HashMap<NodeId, &'static str>|
     -> Result<BrdbWirePort, EmitError> {
        match e {
            BusEnd::Bus(i) => {
                let brick_id = *bus_brick_ids
                    .get(i)
                    .ok_or_else(|| EmitError::UnknownWireNode(format!("bus node {i}")))?;
                Ok(brick_port(brick_id, gc::REROUTER, rer_port_name(as_source)))
            }
            BusEnd::Node(p) => {
                resolve_wire_end(p.node_id, p.port, node_brick_ids, class_index, port_index)
            }
        }
    };
    // Name an end the way a reader can act on: a lane brick by its index,
    // rotation and cell, a real end by its node and port.
    let describe = |e: BusEnd, as_source: bool| -> String {
        match e {
            BusEnd::Bus(i) => match layout.bus.nodes.get(i) {
                Some(n) => format!(
                    "bus node {i} .{} ({:?} at {},{},{})",
                    rer_port_name(as_source),
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
        // An unresolvable end is FATAL: the module wire it replaces was
        // suppressed, so dropping it would strand the consumer.
        let ends = (
            bus_end(bw.source, true, &ctx.node_brick_ids, &ctx.class_index),
            bus_end(bw.target, false, &ctx.node_brick_ids, &ctx.class_index),
        );
        let (source, target) = match ends {
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
        ctx.add_wire(world, WireConnection { source, target })?;
    }
    Ok(())
}
