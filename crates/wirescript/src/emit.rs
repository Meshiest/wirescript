//! IR + placement hints → brdb::World → .brz bytes OR .brdb file.
//!
//! Two output modes, same underlying pipeline:
//! - [`emit_brz`] returns the bytes of a `.brz` bundle — zstd-packed,
//!   portable, good for bundle transfer and in-memory preview.
//! - [`emit_brdb`] writes a `.brdb` SQLite database to a given path —
//!   this is what `BR.World.LoadAdditive <world_name>` accepts.
//!
//! Phase 1 scope:
//! - Flat root module: one outer Microchip brick, everything else inside.
//! - Caller supplies a `Placement` for every node (grid-space position).
//!   In Phase 2 the layout module fills these in automatically.
//! - Nested chips (`Module.chips`) are NOT yet handled.
//! - Literal properties are recorded on each node but only those the
//!   component schema actually models get baked in (others are skipped;
//!   Phase 2 adds the synthetic-upstream-Var emit path).
//!
//! The emit pipeline:
//!   Module + Placements
//!     → brdb::World (main grid = outer chip + inner grid = gates)
//!     → World::to_brz_vec() (zstd-packed .brz bytes)
//!     OR
//!     → World::write_brdb(path) (SQLite database file)

use std::collections::HashMap;
#[cfg(feature = "brdb-full")]
use std::path::Path;

use std::collections::HashMap as StdMap;

use brdb::{
    AsBrdbValue, BString, BrickType, Color, IntVector, Position, Vector3f, WireConnection,
    WirePort as BrdbWirePort, World, assets::LiteralComponent,
    schema::{WireArrayVariant, WireVariant},
};

use crate::intern::{Sym, intern_static, resolve, sym};
use crate::ir::port_registry::WirePort;
use crate::ir::{Literal, Module, Node, NodeId, NodeKind, PortRef, Type, Wire, gate_class as gc};

/// Register all component type → struct name mappings and wire port names
/// on the World so the save path can serialize component data.

/// Grid-space position of a single IR node inside its containing chip
/// (or on the global grid for the outer microchip brick).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl From<Placement> for Position {
    fn from(p: Placement) -> Self {
        Position {
            x: p.x,
            y: p.y,
            z: p.z,
        }
    }
}

/// Options for a single emit run.
#[derive(Clone, Debug)]
pub struct EmitOptions {
    /// World position of the outer deployment chip brick, in global-grid units.
    pub chip_pos: Placement,
    /// World location of the inner-grid entity in centimetres (matches the
    /// engine's `Entity.location`). A common convention is
    /// `(chip_pos * 10, z = chip_pos.z * 10 + 40)`. Phase 2 will compute this
    /// from chip_pos automatically.
    pub inner_grid_location: Vector3f,
    /// Half-extent of the inner grid in grid units (matches
    /// `BP_MicrochipBrickGridDynamicActor_C.PlaneExtent`).
    pub inner_plane_extent: IntVector,
    /// Bundle description written to the .brz metadata.
    pub description: String,
    /// When true, all microchips are emitted uncollapsed (expanded).
    pub open: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            chip_pos: Placement { x: 0, y: 0, z: 0 },
            inner_grid_location: Vector3f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            inner_plane_extent: IntVector { x: 14, y: 14, z: 2 },
            description: String::from("wirescript emit"),
            open: false,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum EmitError {
    #[error("node {0} has no placement")]
    MissingPlacement(String),
    #[error("wire references unknown node: {0}")]
    UnknownWireNode(String),
    #[error("brdb error: {0}")]
    Brdb(#[from] brdb::BrError),
}

/// IR + placements → in-memory `brdb::World`. The core build step; the two
/// public `emit_*` functions wrap this and serialise to their respective
/// on-disk format.
/// Pre-pass: move nodes tagged with `chip_id` into child Modules so the
/// existing chip emit path handles them. Cross-boundary wires are kept in
/// the parent module — the brdb writer's `add_wire` automatically creates
/// remote wire sources when source and target are on different grids.
pub fn partition_anon_chips(module: &mut Module) {
    use std::collections::HashSet;

    let layout_port = WirePort::Layout;

    let chip_ids: HashSet<NodeId> = module.nodes.values().filter_map(|n| n.chip_id).collect();
    if chip_ids.is_empty() {
        return;
    }

    for chip_id in chip_ids {
        let tagged: HashSet<NodeId> = module
            .nodes
            .iter()
            .filter(|(_, n)| n.chip_id == Some(chip_id))
            .map(|(id, _)| *id)
            .collect();
        if tagged.is_empty() {
            continue;
        }

        let mut child = Module::new(&format!("_anon_{chip_id}"));

        for nid in &tagged {
            if let Some(mut node) = module.nodes.remove(nid) {
                node.chip_id = None;
                child.nodes.insert(*nid, node);
            }
        }

        // Partition wires: internal go to child, cross-boundary stay in
        // parent as remote wires. Layout wires keep chips inline in the DAG.
        let mut parent_wires = Vec::new();
        let mut seen_layout_edges: HashSet<(NodeId, NodeId)> = HashSet::new();
        for w in std::mem::take(&mut module.wires) {
            let src_in = tagged.contains(&w.source.node_id);
            let tgt_in = tagged.contains(&w.target.node_id);
            if src_in && tgt_in {
                child.wires.push(w);
            } else {
                if src_in && !tgt_in {
                    let edge = (chip_id, w.target.node_id);
                    if seen_layout_edges.insert(edge) {
                        parent_wires.push(Wire {
                            source: PortRef {
                                node_id: chip_id,
                                port: layout_port,
                            },
                            target: PortRef {
                                node_id: w.target.node_id,
                                port: layout_port,
                            },
                        });
                    }
                } else if tgt_in && !src_in {
                    let edge = (w.source.node_id, chip_id);
                    if seen_layout_edges.insert(edge) {
                        parent_wires.push(Wire {
                            source: PortRef {
                                node_id: w.source.node_id,
                                port: layout_port,
                            },
                            target: PortRef {
                                node_id: chip_id,
                                port: layout_port,
                            },
                        });
                    }
                }
                parent_wires.push(w);
            }
        }
        module.wires = parent_wires;

        module.chips.insert(chip_id, child);
    }

    // Re-nest orphaned inner chip modules: if a child module contains a
    // Chip node whose child module is in the root's `chips` map, move it
    // into the child module's `chips` map so emit can find it.
    loop {
        let mut moves: Vec<(NodeId, NodeId)> = Vec::new();
        for (parent_id, child_mod) in module.chips.iter() {
            for (nid, n) in &child_mod.nodes {
                if n.kind == NodeKind::Chip && module.chips.contains_key(nid) {
                    moves.push((*parent_id, *nid));
                }
            }
        }
        if moves.is_empty() {
            break;
        }
        for (parent_id, inner_id) in moves {
            if let Some(inner_child) = module.chips.remove(&inner_id)
                && let Some(parent_module) = module.chips.get_mut(&parent_id)
            {
                parent_module.chips.insert(inner_id, inner_child);
            }
        }
    }
}

pub fn build_world(
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<World, EmitError> {
    let mut world = World::new();
    world.meta.bundle.description = opts.description.clone();

    let (_chip_brick_id, _root_entity_id, mut inner_pair) = world.add_microchip(
        opts.chip_pos.into(),
        opts.inner_grid_location,
        opts.inner_plane_extent,
        !opts.open,
    );

    // Push root inner grid FIRST so it gets the lowest grid ID (persistent
    // index 2). Child grids pushed during emit_module_bricks get 3, 4, etc.
    let root_grid_idx = world.grids.len();
    world.grids.push((inner_pair.0.clone(), Vec::new()));

    let mut ctx = EmitContext {
        node_brick_ids: HashMap::new(),
        class_index: HashMap::new(),
    };
    emit_module(
        &mut world,
        &mut ctx,
        module,
        placements,
        &mut inner_pair.1,
        opts.inner_grid_location,
        opts.open,
        template_cache,
    )?;

    // Replace placeholder with actual bricks (shifted by -CHUNK_HALF).
    let shifted: Vec<brdb::Brick> = inner_pair
        .1
        .into_iter()
        .map(|mut b| {
            b.position -= brdb::Position::CHUNK_HALF;
            b
        })
        .collect();
    world.grids[root_grid_idx] = (inner_pair.0, shifted);

    // Embed the full component catalog. The game's schema reader was fixed to
    // load the whole catalog, so the minimal "only used components" embed (which
    // worked around the old reader rejecting the full catalog) is no longer
    // needed. Kept commented out in case an older build needs the workaround.
    // world.register_used_components();
    world.register_all_components();

    // Emit as a prefab (type "Prefab" + Meta/Prefab.json) so it pastes like a
    // native copied selection, with bounds computed from the microchip shell.
    world.make_prefab();

    print_emit_stats();

    Ok(world)
}

/// Microchip circuitboard plate height offset (game convention from
/// reverse-engineering microchip_stack_clean.brdb).
const CHIP_PLATE_Z_OFFSET: f32 = 20.0;

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrd};
static EMIT_CLONE_NS: AtomicU64 = AtomicU64::new(0);
static EMIT_BRICK_NS: AtomicU64 = AtomicU64::new(0);
static EMIT_CHIP_FULL_NS: AtomicU64 = AtomicU64::new(0);
static EMIT_CLONE_COUNT: AtomicU64 = AtomicU64::new(0);
static EMIT_BRICK_COUNT: AtomicU64 = AtomicU64::new(0);
static EMIT_COMP_NS: AtomicU64 = AtomicU64::new(0);

pub fn print_emit_stats() {
    let clone_s = EMIT_CLONE_NS.load(AtomicOrd::Relaxed) as f64 / 1e9;
    let brick_s = EMIT_BRICK_NS.load(AtomicOrd::Relaxed) as f64 / 1e9;
    let comp_s = EMIT_COMP_NS.load(AtomicOrd::Relaxed) as f64 / 1e9;
    let chip_s = EMIT_CHIP_FULL_NS.load(AtomicOrd::Relaxed) as f64 / 1e9;
    let clones = EMIT_CLONE_COUNT.load(AtomicOrd::Relaxed);
    let bricks = EMIT_BRICK_COUNT.load(AtomicOrd::Relaxed);
    eprintln!(
        "[emit:detail] clone path: {clone_s:.2}s ({clones} clones), brick construction: {brick_s:.2}s ({bricks} bricks), component build: {comp_s:.2}s, chip full emit: {chip_s:.2}s"
    );
}

struct EmitContext {
    node_brick_ids: HashMap<NodeId, usize>,
    class_index: HashMap<NodeId, &'static str>,
}

fn emit_module(
    world: &mut World,
    ctx: &mut EmitContext,
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    bricks: &mut Vec<brdb::Brick>,
    parent_grid_origin: Vector3f,
    force_open: bool,
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
        let pos = placements
            .get(id)
            .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;
        let gate_class_str = node.gate_class;
        let brick_asset = infer_brick_for_gate(gate_class_str);
        // Offset each brick by its half-size so the brick's min corner aligns
        // with the cell grid line at (pos.x, pos.y). This keeps every brick
        // inside its own cell regardless of size (1x1, wide DisplayText, etc.)
        // and prevents overlaps between adjacent cells of different sizes.
        let catalog_entry = crate::catalog::default_catalog().find_by_class(gate_class_str);
        let (offset_x, offset_y) = match catalog_entry {
            Some(g) => (g.half_size.x, g.half_size.y),
            _ => (5, 5),
        };
        let inner_pos = brdb::Position {
            x: pos.x + offset_x,
            y: pos.y + offset_y,
            z: pos.z,
        };
        let (mut brick, brick_id) = brdb::Brick {
            asset: BrickType::from(brick_asset),
            position: inner_pos,
            color: color_for_node(node, module, &wire_target_index),
            ..Default::default()
        }
        .with_id_split();

        let mut gate_inlined: StdMap<Sym, &Literal> = StdMap::new();
        if let Some(entries) = inlined_by_node.get(id) {
            for (port_idx, lit) in entries {
                // Convert PortIndex → Sym for property key lookup
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
        let comp = match effective_class_str {
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
                    Some(Literal::Array(lits)) => {
                        wire_array_variant_from_literals(elem_ty, lits)
                    }
                    _ => empty_wire_array_variant(elem_ty),
                };
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("Value".into(), Box::new(av));
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
                    Some(Literal::Asset { asset_type, asset_name }) => {
                        let (idx, _) = world
                            .global_data
                            .external_asset_references
                            .insert_full((asset_type.clone(), asset_name.clone()));
                        Some(idx)
                    }
                    _ => None,
                };
                let brick_wrapper = LiteralComponent::new("BrickTypeNetWrapper").with_data([
                    ("BrickAsset", Box::new(BrdbValue::Asset(None)) as Box<dyn AsBrdbValue>),
                    ("ProceduralSize", Box::new(IntVector::default()) as Box<dyn AsBrdbValue>),
                ]);
                let entry_plan = LiteralComponent::new("BRInventoryEntryPlan").with_data([
                    // EBRInventoryEntryPlanType::Item = 3
                    ("Type", Box::new(3u8) as Box<dyn AsBrdbValue>),
                    ("BrickTypeIfBrick", Box::new(brick_wrapper) as Box<dyn AsBrdbValue>),
                    ("EntityTypeIfEntity", Box::new(BrdbValue::Asset(None)) as Box<dyn AsBrdbValue>),
                    ("ItemTypeIfItem", Box::new(BrdbValue::Asset(item_idx)) as Box<dyn AsBrdbValue>),
                ]);
                let entry = LiteralComponent::new("BRInventoryEntryConfig")
                    .with_data([("Item", Box::new(()) as Box<dyn AsBrdbValue>)]);
                let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
                data.insert("Slot".into(), Box::new(slot));
                data.insert("Entry".into(), Box::new(entry));
                data.insert("EntryPlan".into(), Box::new(entry_plan));
                LiteralComponent::new_from_data(effective_class_str, std::sync::Arc::new(data))
            }
            _ => build_gate_component(effective_class_str, &*node.ports, &gate_inlined, world),
        };
        // EMIT_COMP_NS.fetch_add(_ct.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
        brick.add_component_box(Box::new(comp));

        bricks.push(brick);
        ctx.node_brick_ids.insert(**id, brick_id);
        ctx.class_index.insert(**id, node.gate_class);
        // EMIT_BRICK_NS.fetch_add(_bt.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
        // EMIT_BRICK_COUNT.fetch_add(1, AtomicOrd::Relaxed);
    }

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
        let pos = placements
            .get(id)
            .ok_or_else(|| EmitError::MissingPlacement(id.to_string()))?;
        let inner_pos = brdb::Position {
            x: pos.x + 5,
            y: pos.y + 5,
            z: pos.z,
        };

        let child_layout = crate::layout::layout(child_module);
        let half_x = (child_layout.bounds_max.x - child_layout.bounds_min.x) / 2;
        let half_y = (child_layout.bounds_max.y - child_layout.bounds_min.y) / 2;
        let child_extent = IntVector {
            x: (half_x + 5).max(5),
            y: (half_y + 5).max(5),
            z: 0,
        };

        let chip_entity_id = brdb::Brick::next_id();
        let child_location = Vector3f {
            x: parent_grid_origin.x + inner_pos.x as f32,
            y: parent_grid_origin.y + inner_pos.y as f32,
            z: parent_grid_origin.z + inner_pos.z as f32 + CHIP_PLATE_Z_OFFSET,
        };
        let child_entity = brdb::Entity {
            asset: brdb::assets::entities::MICROCHIP_GRID,
            id: Some(chip_entity_id),
            location: child_location,
            frozen: true,
            data: brdb::assets::entities::microchip_grid_entity(
                !force_open
                    && !node
                        .properties
                        .get(&intern_static("_open"))
                        .map(|l| matches!(l, Literal::Bool(true)))
                        .unwrap_or(false),
                IntVector { x: 0, y: 0, z: 0 },
                child_extent,
            ),
            ..Default::default()
        };

        let mut child_bricks: Vec<brdb::Brick> = Vec::new();
        emit_module(
            world,
            ctx,
            child_module,
            &child_layout.placements,
            &mut child_bricks,
            child_location,
            force_open,
            template_cache,
        )?;

        world.add_brick_grid(child_entity, child_bricks);

        let (chip_brick, chip_brick_id) = brdb::Brick {
            asset: brdb::assets::bricks::B_MICROCHIP,
            position: inner_pos,
            ..Default::default()
        }
        .with_component_box(Box::new(LiteralComponent::new(
            "Component_Internal_Microchip",
        )))
        .with_id_split();
        bricks.push(chip_brick);
        ctx.node_brick_ids.insert(**id, chip_brick_id);
        ctx.class_index.insert(**id, node.gate_class);
        world.register_microchip_link(chip_brick_id, chip_entity_id);

        // EMIT_CHIP_FULL_NS.fetch_add(_ft.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
    }

    // ── Pass 3: emit this module's wires ──
    let layout_port_id = WirePort::Layout;
    let port_index = build_port_index(module, &ctx.node_brick_ids);
    for w in &module.wires {
        if w.source.port == layout_port_id || w.target.port == layout_port_id {
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
        match wire_to_connection_indexed(w, &ctx.node_brick_ids, &ctx.class_index, &port_index) {
            Ok(conn) => world.add_wire(conn),
            Err(e) => {
                eprintln!(
                    "[wire] dropped: {} → {} (port {}→{}): {e:?}",
                    w.source.node_id,
                    w.target.node_id,
                    w.source.port.as_str(),
                    w.target.port.as_str()
                );
            }
        }
    }

    Ok(())
}

/// Pre-build index: (chip_node_id, port_name) → (brick_id, component_class, remapped_port)
fn build_port_index(
    module: &Module,
    node_brick_ids: &HashMap<NodeId, usize>,
) -> HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)> {
    let port_label_sym = *sym::PORT_LABEL;
    let mut idx = HashMap::new();
    for (chip_nid, node) in &module.nodes {
        if node.kind != NodeKind::Chip {
            continue;
        }
        let child = match module.chips.get(chip_nid) {
            Some(c) => c,
            None => continue,
        };
        for (child_nid, child_node) in &child.nodes {
            let is_output = child_node.kind == NodeKind::Output;
            let is_input = child_node.kind == NodeKind::Input;
            if !is_output && !is_input {
                continue;
            }
            let label: &'static str =
                match child_node.properties.get(&port_label_sym).and_then(|l| {
                    if let Literal::String(s) = l {
                        Some(resolve(crate::intern::intern(s)))
                    } else {
                        None
                    }
                }) {
                    Some(l) => l,
                    None => continue,
                };
            if label.is_empty() {
                continue;
            }
            if let Some(&brick_id) = node_brick_ids.get(child_nid) {
                let class: &'static str = if is_output {
                    "BrickComponentType_Internal_MicrochipOutput"
                } else {
                    "BrickComponentType_Internal_MicrochipInput"
                };
                let remap_port: &'static str = if is_output { "RER_Output" } else { "RER_Input" };
                idx.insert((*chip_nid, label), (brick_id, class, remap_port));
            }
        }
    }
    idx
}

/// Build the gate's `LiteralComponent`. For wire-input ports that have an
/// inlined literal (from a `_Literal` source node), we embed the value into
/// the component data struct using the wire_graph_variant field so the game
/// reads it on load without needing a separate constant source gate.
fn build_gate_component(
    gate_class: &'static str,
    ports: &crate::ir::GateIO,
    inlined: &StdMap<Sym, &Literal>,
    world: &mut World,
) -> LiteralComponent {
    // For gates whose component data struct carries wire_graph_variant fields,
    // look up the struct schema and build the component with embedded values.
    // Non-inlined fields get a default (Int(0)) so the struct is always complete.
    // Always write the data struct when the gate type has one — even if no
    // fields are inlined. This ensures ALL instances of the same component
    // type have matching data, preventing reader misalignment.
    if let Some((struct_name, field_names, use_wire_variant)) = data_struct_for_gate(gate_class) {
        let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
        for field in field_names {
            let val: Box<dyn AsBrdbValue> = match inlined.get(&intern_static(field)) {
                Some(lit) if use_wire_variant => {
                    if schema_field_is(struct_name, field, "wire_graph_prim_math_variant") {
                        if let Literal::Bool(b) = lit {
                            Box::new(WireVariant::Int(if *b { 1 } else { 0 }))
                        } else {
                            literal_to_boxed_wire_variant(lit, ports, field)
                        }
                    } else {
                        literal_to_boxed_wire_variant(lit, ports, field)
                    }
                }
                Some(lit) => {
                    if schema_field_is(struct_name, field, "wire_graph_variant")
                        || schema_field_is(struct_name, field, "wire_graph_prim_math_variant")
                    {
                        literal_to_boxed_wire_variant(lit, ports, field)
                    } else if schema_field_is(struct_name, field, "class")
                        || schema_field_is(struct_name, field, "object")
                    {
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
                    } else if let Some(ev) = try_resolve_enum(struct_name, field, lit) {
                        Box::new(ev)
                    } else if schema_field_is(struct_name, field, "str") {
                        Box::new(literal_to_string(lit))
                    } else {
                        literal_to_boxed_native(lit)
                    }
                }
                None if use_wire_variant => {
                    let port_ty = ports
                        .inputs
                        .iter()
                        .chain(ports.outputs.iter())
                        .find(|p| resolve(p.name) == *field)
                        .map(|p| &p.ty);
                    let wv = var_type_to_wire_variant(port_ty);
                    // prim_math_variant doesn't support Bool — coerce to Int
                    let wv = if schema_field_is(struct_name, field, "wire_graph_prim_math_variant")
                    {
                        coerce_for_prim_math(wv)
                    } else {
                        wv
                    };
                    Box::new(wv)
                }
                // No inlined value. Wire-typed ports still need a typed variant
                // default so the variant member matches the port type (the
                // component_db defaults don't carry wire variants). Every other
                // (native) field is omitted from the data map entirely: the brdb
                // writer fills missing struct fields from component_db's
                // STRUCT_DEFAULTS — the single source of truth for gate defaults
                // (e.g. DisplayText FontSize=16, Lifetime=5) — falling back to a
                // type-zero when no default is registered.
                None => {
                    if schema_field_is(struct_name, field, "wire_graph_variant")
                        || schema_field_is(struct_name, field, "wire_graph_prim_math_variant")
                    {
                        let port_ty = ports
                            .inputs
                            .iter()
                            .chain(ports.outputs.iter())
                            .find(|p| resolve(p.name) == *field)
                            .map(|p| &p.ty);
                        let wv = var_type_to_wire_variant(port_ty);
                        let wv =
                            if schema_field_is(struct_name, field, "wire_graph_prim_math_variant") {
                                coerce_for_prim_math(wv)
                            } else {
                                wv
                            };
                        Box::new(wv)
                    } else {
                        continue;
                    }
                }
            };
            data.insert(BString::from(field.to_string()), val);
        }
        return LiteralComponent::new_from_data(gate_class, std::sync::Arc::new(data));
    }

    // Default: dataless component stub — registers component type only,
    // no struct data (engine uses the default from the brick type).
    LiteralComponent::new(gate_class.to_string())
}

/// Returns `(struct_name, field_names, use_wire_variant)` for gates whose
/// component data struct must be serialized.
///
/// Source of truth: `brdb/crates/brdb/schemas/BRSavedComponentChunkSoA_max.schema`
/// and `brdb/crates/brdb/src/assets/gates/logic.rs` for component->struct mapping.
///
/// Every component type the game recognizes MUST appear here with the correct
/// struct name, even if the struct has 0 fields. A missing entry causes the
/// game to skip reading data for that type--misaligning all subsequent
/// component data.
fn data_struct_for_gate(gate_class: &str) -> Option<(&'static str, &'static [&'static str], bool)> {
    match gate_class {
        // ── Variables ──────────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Var_Set"
        | "BrickComponentType_WireGraph_Exec_Var_Increment"
        | "BrickComponentType_WireGraph_Exec_Var_Get" => Some((
            "BrickComponentData_WireGraph_Exec_Var_EditOrGet",
            &["Value"],
            true,
        )),
        "BrickComponentType_WireGraphPseudo_Var" => {
            Some(("BrickComponentData_WireGraphPseudo_Var", &["Value"], true))
        }
        "BrickComponentType_WireGraphPseudo_ArrayVar" => {
            Some(("BrickComponentData_WireGraphPseudo_ArrayVar", &[], false))
        }

        // ── Array variables ────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_ArrayVar_SetAtIndex" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_ElementOp",
            &["Index", "Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Get" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Get",
            &["Index", "Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Push" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Push",
            &["Value"],
            true,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Pop" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Pop",
            &["Value"],
            true,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_RemoveAtIndex" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_RemoveAtIndex",
            &["Index"],
            false,
        )),
        // Value/index args of these gates are read from component data, so a
        // literal argument must be inlined here (else it defaults to 0).
        "BrickComponentType_WireGraph_Exec_ArrayVar_Find" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Find",
            &["Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Insert" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Insert",
            &["Index", "Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Resize" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Resize",
            &["Size", "Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Swap" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Swap",
            &["IndexA", "IndexB"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_GetLength" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_GetLength",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ArrayVar_Clear"
        | "BrickComponentType_WireGraph_Exec_ArrayVar_Shuffle" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Base",
            &[],
            false,
        )),
        // Legacy alias
        "BrickComponentType_WireGraph_Exec_ArrayVar_Base" => Some((
            "BrickComponentData_WireGraph_Exec_ArrayVar_Base",
            &[],
            false,
        )),

        // ── Entity manipulation ────────────────────────────────────────────
        // Entity gates have Vector/Rotator struct fields. Values come from
        // wires so no data struct is needed.
        "BrickComponentType_WireGraph_Exec_Entity_SetLocation"
        | "BrickComponentType_WireGraph_Exec_Entity_SetRotation"
        | "BrickComponentType_WireGraph_Exec_Entity_SetLocationRotation"
        | "BrickComponentType_WireGraph_Exec_Entity_AddLocationRotation"
        | "BrickComponentType_WireGraph_Exec_Entity_SetVelocity"
        | "BrickComponentType_WireGraph_Exec_Entity_AddVelocity"
        | "BrickComponentType_WireGraph_Exec_Entity_SetLinearVelocity"
        | "BrickComponentType_WireGraph_Exec_Entity_SetAngularVelocity"
        | "BrickComponentType_WireGraph_Exec_Entity_SetGravityDirection"
        | "BrickComponentType_WireGraph_Exec_Entity_Teleport"
        | "BrickComponentType_WireGraph_Exec_Entity_RelativeTeleport" => None,

        // ── Gamemode ──────────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Gamemode_SetLeaderboardValue"
        | "BrickComponentType_WireGraph_Exec_Gamemode_IncrementLeaderboardValue" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_LeaderboardValue",
            &["Key", "Value"],
            false,
        )),

        // ── Prefab spawner ────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_PrefabSpawner" => Some((
            "BrickComponentData_WireGraph_Exec_PrefabSpawner",
            &["Lifetime", "Limit"],
            false,
        )),

        // ── Sweep (raycasting) ────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Sweep" => Some((
            "BrickComponentData_WireGraph_Exec_Sweep",
            &[
                "Distance",
                "Radius",
                "bDetectBricks",
                "bDetectPlayers1",
                "bDetectPlayers2",
                "bDetectPlayers3",
                "bDetectPlayers4",
                "bDetectPhysics",
                "bDetectMap",
                "bRelative",
                "bIgnoreOwningGrid",
            ],
            false,
        )),

        // ── Vector / Color constructors ───────────────────────────────────
        // Literal X/Y/Z args are inlined as properties, so without this entry
        // they're dropped and the vector defaults to (0,0,0).
        "BrickComponentType_WireGraph_Expr_MakeVector" => Some((
            "BrickComponentData_WireGraph_Expr_MakeVector",
            &["X", "Y", "Z"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_MakeColor" => Some((
            "BrickComponentData_WireGraph_Expr_MakeColor",
            &["R", "G", "B", "A"],
            false,
        )),

        // ── Rotation / quaternion (cl14428+) ──────────────────────────────
        // Scalar fields (Pitch/Yaw/Roll/Angle/Alpha) inline literal args; the
        // struct-typed inputs (Vector/Quat/Rotator) come from wires and are
        // filled from defaults. Each must list its struct so component data
        // stays aligned.
        // InputA/InputB are prim-math variants (wrapped per-field via the schema
        // check), Tolerance is a plain f64. Without this entry a literal `b` or
        // tolerance arg (e.g. `NearlyEqual(x, 1.0, 0.001)`) drops to 0.
        "BrickComponentType_WireGraph_Expr_NearlyEqual" => Some((
            "BrickComponentData_WireGraph_Expr_NearlyEqual",
            &["InputA", "InputB", "Tolerance"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_MakeRotation" => Some((
            "BrickComponentData_WireGraph_Expr_MakeRotation",
            &["Pitch", "Yaw", "Roll"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_SplitRotation" => Some((
            "BrickComponentData_WireGraph_Expr_SplitRotation",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_RotateVector" => Some((
            "BrickComponentData_WireGraph_Expr_RotateVector",
            &["Rotation", "Vector"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_InvertRotation" => Some((
            "BrickComponentData_WireGraph_Expr_InvertRotation",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_DirectionToRotation" => Some((
            "BrickComponentData_WireGraph_Expr_DirectionToRotation",
            &["Direction"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_RotationToDirection" => Some((
            "BrickComponentData_WireGraph_Expr_RotationToDirection",
            &["Rotation"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_QuatBetween" => Some((
            "BrickComponentData_WireGraph_Expr_QuatBetween",
            &["From", "To"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_QuatAngleBetween" => Some((
            "BrickComponentData_WireGraph_Expr_QuatAngleBetween",
            &["InputA", "InputB"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_QuatFromAxisAngle" => Some((
            "BrickComponentData_WireGraph_Expr_QuatFromAxisAngle",
            &["Axis", "Angle"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_QuatToAxisAngle" => Some((
            "BrickComponentData_WireGraph_Expr_QuatToAxisAngle",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_QuatSlerp" => Some((
            "BrickComponentData_WireGraph_Expr_QuatSlerp",
            &["InputA", "InputB", "Alpha", "bShortestPath"],
            false,
        )),

        // ── Controller role check (cl14428+) ──────────────────────────────
        "BrickComponentType_WireGraph_Exec_Controller_HasRole" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_HasRole",
            &["RoleName"],
            false,
        )),

        // ── Stateful exec value gates (cl14428+) ──────────────────────────
        // Count inlines a literal; Value is persisted cycle/toggle state.
        "BrickComponentType_WireGraph_Exec_Cycle" => Some((
            "BrickComponentData_WireGraph_Exec_Cycle",
            &["Count", "Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Toggle" => Some((
            "BrickComponentData_WireGraph_Exec_Toggle",
            &["Value"],
            false,
        )),

        // ── sRGB / hex color (cl14428+) ───────────────────────────────────
        "BrickComponentType_WireGraph_Expr_MakeColorSRGB" => Some((
            "BrickComponentData_WireGraph_Expr_MakeColorSRGB",
            &["R", "G", "B", "A"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_SplitColorSRGB" => Some((
            "BrickComponentData_WireGraph_Expr_SplitColorSRGB",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_MakeColorHex" => Some((
            "BrickComponentData_WireGraph_Expr_MakeColorHex",
            &["Hex"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_ColorToHex" => Some((
            "BrickComponentData_WireGraph_Expr_ColorToHex",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_ColorBlend" => Some((
            "BrickComponentData_WireGraph_Expr_ColorBlend",
            &["ColorA", "ColorB", "Alpha", "BlendSpace"],
            false,
        )),

        // ── Exec flow ──────────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Branch" => {
            Some(("BrickComponentData_WireGraph_ExecBranch", &[], false))
        }
        "BrickComponentType_WireGraph_Exec_Union" => {
            Some(("BrickComponentData_WireGraph_ExecUnion", &[], false))
        }

        // ── Select / Swap ──────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Expr_Select" => Some((
            "BrickComponentData_WireGraph_Expr_Select",
            &["bSelectB", "InputA", "InputB"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_Swap" => Some((
            "BrickComponentData_WireGraph_Expr_Swap",
            &["bSwap", "InputA", "InputB"],
            false,
        )),

        // ── Random ─────────────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Random" => Some((
            "BrickComponentData_WireGraph_Exec_Random",
            &["Seed", "Min", "Max"],
            false,
        )),

        // ── Math (binary, prim_math_variant) ───────────────────────────────
        "BrickComponentType_WireGraph_Expr_MathAdd"
        | "BrickComponentType_WireGraph_Expr_MathSubtract"
        | "BrickComponentType_WireGraph_Expr_MathMultiply"
        | "BrickComponentType_WireGraph_Expr_MathDivide"
        | "BrickComponentType_WireGraph_Expr_MathModulo"
        | "BrickComponentType_WireGraph_Expr_MathModuloFloored" => Some((
            "BrickComponentData_WireGraph_Expr_PrimMathVariantPrimMathVariant_PrimMathVariant",
            &["InputA", "InputB"],
            true,
        )),
        // MathPow has its own struct (f64, not prim_math_variant)
        "BrickComponentType_WireGraph_Expr_MathPow" => Some((
            "BrickComponentData_WireGraph_Expr_MathPow",
            &["Input", "Exponent"],
            false,
        )),
        // Math (unary, prim_math_variant) -- Negate, Abs, etc.
        "BrickComponentType_WireGraph_Expr_MathNegate"
        | "BrickComponentType_WireGraph_Expr_MathAbs" => Some((
            "BrickComponentData_WireGraph_Expr_PrimMathVariant_PrimMathVariant",
            &["Input"],
            true,
        )),
        // Math (unary, f64 -> f64) -- Ceil, Floor, Sqrt, Sin, Cos, Tan, Asin, Acos, Atan, Log
        "BrickComponentType_WireGraph_Expr_Ceil"
        | "BrickComponentType_WireGraph_Expr_Floor"
        | "BrickComponentType_WireGraph_Expr_MathSqrt"
        | "BrickComponentType_WireGraph_Expr_MathSin"
        | "BrickComponentType_WireGraph_Expr_MathCos"
        | "BrickComponentType_WireGraph_Expr_MathTan"
        | "BrickComponentType_WireGraph_Expr_MathAsin"
        | "BrickComponentType_WireGraph_Expr_MathAcos"
        | "BrickComponentType_WireGraph_Expr_MathAtan"
        | "BrickComponentType_WireGraph_Expr_MathLog" => Some((
            "BrickComponentData_WireGraph_Expr_Float_Float",
            &["Input"],
            false,
        )),
        // MathAtan2 (two f64 inputs)
        "BrickComponentType_WireGraph_Expr_MathAtan2" => Some((
            "BrickComponentData_WireGraph_Expr_MathAtan2",
            &["Y", "X"],
            false,
        )),
        // MathLogBase (f64 + f64)
        "BrickComponentType_WireGraph_Expr_MathLogBase" => Some((
            "BrickComponentData_WireGraph_Expr_MathLogBase",
            &["Input", "Base"],
            false,
        )),
        // MathBlend (f64 + two prim_math_variants)
        "BrickComponentType_WireGraph_Expr_MathBlend" => Some((
            "BrickComponentData_WireGraph_Expr_MathBlend",
            &["Blend", "InputA", "InputB"],
            false,
        )),
        // MathEasing: two easing enums + two prim_math_variants + f64 blend.
        // use_wire_variant must be false (like MathBlend) so the per-field
        // schema check writes InputA/InputB as variants while the enum fields
        // (Function/Direction) still go through try_resolve_enum.
        "BrickComponentType_WireGraph_Expr_MathEasing" => Some((
            "BrickComponentData_WireGraph_Expr_MathEasing",
            &["Function", "Direction", "InputA", "InputB", "Blend"],
            false,
        )),
        // Tween (Pseudo): variant target, f64 duration, two easing enums.
        "BrickComponentType_WireGraphPseudo_Tween" => Some((
            "BrickComponentData_WireGraphPseudo_Tween",
            &["Target", "Duration", "Function", "Direction"],
            false,
        )),
        // Timer (Pseudo): f64 limit + persisted Time/bRunning state.
        "BrickComponentType_WireGraphPseudo_Timer" => Some((
            "BrickComponentData_WireGraphPseudo_Timer",
            &["Limit", "Time", "bRunning"],
            false,
        )),
        // MathClamp (three prim_math_variants)
        "BrickComponentType_WireGraph_Expr_MathClamp" => Some((
            "BrickComponentData_WireGraph_Expr_MathClamp",
            &["Min", "Max", "Input"],
            true,
        )),

        // ── Compare ────────────────────────────────────────────────────────
        // Eq/Neq use wire_graph_variant (can compare any type)
        "BrickComponentType_WireGraph_Expr_CompareEqual"
        | "BrickComponentType_WireGraph_Expr_CompareNotEqual" => Some((
            "BrickComponentData_WireGraph_Expr_Compare",
            &["InputA", "InputB"],
            true,
        )),
        // Lt/Leq/Gt/Geq use wire_graph_prim_math_variant (numbers only)
        "BrickComponentType_WireGraph_Expr_CompareLess"
        | "BrickComponentType_WireGraph_Expr_CompareLessOrEqual"
        | "BrickComponentType_WireGraph_Expr_CompareGreater"
        | "BrickComponentType_WireGraph_Expr_CompareGreaterOrEqual" => Some((
            "BrickComponentData_WireGraph_Expr_MathCompare",
            &["InputA", "InputB"],
            true,
        )),

        // ── Logic (bool) ──────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Expr_LogicalAND"
        | "BrickComponentType_WireGraph_Expr_LogicalOR"
        | "BrickComponentType_WireGraph_Expr_LogicalXOR"
        | "BrickComponentType_WireGraph_Expr_LogicalNAND"
        | "BrickComponentType_WireGraph_Expr_LogicalNOR" => Some((
            "BrickComponentData_WireGraph_Expr_BoolBool_Bool",
            &["bInputA", "bInputB"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_LogicalNOT" => Some((
            "BrickComponentData_WireGraph_Expr_Bool_Bool",
            &["bInput"],
            false,
        )),

        // ── Bitwise (i64) ─────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Expr_BitwiseAND"
        | "BrickComponentType_WireGraph_Expr_BitwiseOR"
        | "BrickComponentType_WireGraph_Expr_BitwiseXOR"
        | "BrickComponentType_WireGraph_Expr_BitwiseNAND"
        | "BrickComponentType_WireGraph_Expr_BitwiseNOR"
        | "BrickComponentType_WireGraph_Expr_BitwiseShiftLeft"
        | "BrickComponentType_WireGraph_Expr_BitwiseShiftRight" => Some((
            "BrickComponentData_WireGraph_Expr_IntInt_Int",
            &["InputA", "InputB"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_BitwiseNOT" => Some((
            "BrickComponentData_WireGraph_Expr_Int_Int",
            &["Input"],
            false,
        )),

        // ── String ops ─────────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Expr_String_FormatText" => Some((
            "BrickComponentData_WireGraph_Expr_String_FormatText",
            &[
                "FormatString",
                "InputA",
                "InputB",
                "InputC",
                "InputD",
                "InputE",
                "InputF",
                "InputG",
            ],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Concatenate" => Some((
            "BrickComponentData_WireGraph_Expr_String_Concatenate",
            &["InputA", "InputB", "Separator"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Length" => Some((
            "BrickComponentData_WireGraph_Expr_String_Length",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Contains" => Some((
            "BrickComponentData_WireGraph_Expr_String_Contains",
            &["Input", "Search"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Find" => Some((
            "BrickComponentData_WireGraph_Expr_String_Find",
            &["Input", "Search", "bCaseSensitive"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Replace" => Some((
            "BrickComponentData_WireGraph_Expr_String_Replace",
            &["Input", "Search", "Replacement"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Split" => Some((
            "BrickComponentData_WireGraph_Expr_String_Split",
            &["Input", "Delimiter"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Substring" => Some((
            "BrickComponentData_WireGraph_Expr_String_Substring",
            &["Input", "Start", "Length"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_StartsWith" => Some((
            "BrickComponentData_WireGraph_Expr_String_StartsWith",
            &["Input", "Prefix"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_EndsWith" => Some((
            "BrickComponentData_WireGraph_Expr_String_EndsWith",
            &["Input", "Suffix"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_ToLower" => Some((
            "BrickComponentData_WireGraph_Expr_String_ToLower",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_ToUpper" => Some((
            "BrickComponentData_WireGraph_Expr_String_ToUpper",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_String_Trim" => Some((
            "BrickComponentData_WireGraph_Expr_String_Trim",
            &["Input"],
            false,
        )),

        // ── Controller / Character / Entity (exec) ─────────────────────────
        "BrickComponentType_WireGraph_Exec_Controller_DisplayText" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_DisplayText",
            &[
                "Text",
                "AnchorX",
                "AnchorY",
                "PositionX",
                "PositionY",
                "Angle",
                "ScaleX",
                "ScaleY",
                "FontSize",
                "OutlineSize",
                "Justification",
                "Transition",
                "Lifetime",
                "TextId",
            ],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Controller_ShowStatusMessage" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_ShowStatusMessage",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_ShowHint" => Some((
            "BrickComponentData_WireGraph_Exec_Character_ShowHint",
            &["HintTitle", "HintText"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_SetTempPermission" => Some((
            "BrickComponentData_WireGraph_Exec_Character_SetTempPermission",
            &["PermissionTagStr", "bPermissionEnable"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_ChatCommand" => Some((
            "BrickComponentData_WireGraph_Exec_ChatCommand",
            &["CommandName", "HelpText"],
            false,
        )),

        // ── Messaging ───────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Controller_ShowChatMessage" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_ShowChatMessage",
            &["Message"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Controller_ShowMessageBox" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_ShowMessageBox",
            &["Title", "Message"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_BroadcastChatMessage",
            &["Message"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastStatusMessage" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_BroadcastStatusMessage",
            &["Message", "bFlashIfUnchanged"],
            false,
        )),

        // ── Audio ──────────────────────────────────────────────────────────
        // AudioDescriptor is a class/object field: an inlined `$…` asset ref
        // is registered in the external-asset table by build_gate_component.
        "Component_WireGraph_PlayAudioAt" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_PlayAudioAt",
            &[
                "AudioDescriptor",
                "VolumeMultiplier",
                "PitchMultiplier",
                "InnerRadius",
                "MaxDistance",
                "bSpatialization",
            ],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_PlayGlobalAudio" => Some((
            "BrickComponentData_WireGraph_Exec_PlayGlobalAudio",
            &["AudioDescriptor", "VolumeMultiplier", "PitchMultiplier"],
            false,
        )),

        // ── Entity tags ─────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Entity_GetTag" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetTag",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_SetTag" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_SetTag",
            &["Tag"],
            false,
        )),

        // ── Player lookup ──────────────────────────────────────────────────
        "BrickComponentType_WireGraph_FindPlayer" => Some((
            "BrickComponentData_WireGraph_FindPlayer",
            &["Query"],
            false,
        )),

        // ── Change detector ─────────────────────────────────────
        "BrickComponentType_WireGraph_Expr_ChangeDetector" => Some((
            "BrickComponentData_WireGraph_Expr_ChangeDetector",
            &["Input"],
            true,
        )),

        // ── Quaternion make/split/dot ───────────────────────────
        "BrickComponentType_WireGraph_Expr_MakeQuaternion" => Some((
            "BrickComponentData_WireGraph_Expr_MakeQuaternion",
            &["X", "Y", "Z", "W"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_SplitQuaternion" => Some((
            "BrickComponentData_WireGraph_Expr_SplitQuaternion",
            &["Input"],
            false,
        )),
        "BrickComponentType_WireGraph_Expr_QuatDotProduct" => Some((
            "BrickComponentData_WireGraph_Expr_QuatDotProduct",
            &["InputA", "InputB"],
            false,
        )),

        // ── Character inventory family ──────────────────────────
        // Item / EntityType / BrickAsset / ItemType / ProjectileOverride are
        // class/object asset fields (see build_gate_component).
        "BrickComponentType_WireGraph_Exec_Character_AddInventoryItem" => Some((
            "BrickComponentData_WireGraph_Exec_Character_AddInventoryItem",
            &["Item"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_SetInventoryItem" => Some((
            "BrickComponentData_WireGraph_Exec_Character_SetInventoryItem",
            &["Slot", "Item"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_AddInventoryBrick" => Some((
            "BrickComponentData_WireGraph_Exec_Character_AddInventoryBrick",
            &["BrickAsset", "ProceduralSize"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_SetInventoryBrick" => Some((
            "BrickComponentData_WireGraph_Exec_Character_SetInventoryBrick",
            &["Slot", "BrickAsset", "ProceduralSize"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_AddInventoryEntity" => Some((
            "BrickComponentData_WireGraph_Exec_Character_AddInventoryEntity",
            &["EntityType"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntity" => Some((
            "BrickComponentData_WireGraph_Exec_Character_SetInventoryEntity",
            &["Slot", "EntityType"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_AddInventoryItemAdv" => Some((
            "BrickComponentData_WireGraph_Exec_Character_AddInventoryItemAdv",
            &[
                "ItemType",
                "DamageMultiplier",
                "WeaponSpeedMultiplier",
                "ItemScale",
                "ItemNameOverride",
                "ProjectileOverride",
            ],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_SetInventoryItemAdv" => Some((
            "BrickComponentData_WireGraph_Exec_Character_SetInventoryItemAdv",
            &[
                "Slot",
                "ItemType",
                "DamageMultiplier",
                "WeaponSpeedMultiplier",
                "ItemScale",
                "ItemNameOverride",
                "ProjectileOverride",
            ],
            false,
        )),

        // Empty-data-struct gates: the game still expects the struct to be
        // registered (reads 0 bytes per instance).
        "BrickComponentType_WireGraph_Exec_Controller_GetFromEntity" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_GetFromEntity",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_GetFromController" => Some((
            "BrickComponentData_WireGraph_Exec_Character_GetFromController",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Character_GetAim" => Some((
            "BrickComponentData_WireGraph_Exec_Character_GetAim",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_GetLocation" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetLocation",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_GetRotation" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetRotation",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_GetLocationRotation" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetLocationRotation",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_GetLinearVelocity" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetLinearVelocity",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_GetAngularVelocity" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetAngularVelocity",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Entity_GetVelocity" => Some((
            "BrickComponentData_WireGraph_Exec_Entity_GetVelocity",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Gamemode_GetTeam" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_GetTeam",
            &[],
            false,
        )),
        // Permission gates carry the permission name as a `str` property.
        // (SetTempPermission already has an entry above.)
        "BrickComponentType_WireGraph_Exec_Controller_HasPermission" => Some((
            "BrickComponentData_WireGraph_Exec_Controller_HasPermission",
            &["PermissionName"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Gamemode_SetTeamPinned" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_SetTeamPinned",
            &["bPinned"],
            false,
        )),
        "Component_Internal_InputSplitter" => {
            Some(("BrickComponentData_Internal_InputSplitter_V2", &[], false))
        }

        // ── Variant conversion ─────────────────────────────────────────────
        "BrickComponentType_WireGraph_Expr_Variant_ToVariant"
        | "BrickComponentType_WireGraph_Expr_Variant_FromVariant" => Some((
            "BrickComponentData_WireGraph_Expr_Variant_Variant",
            &["Input"],
            true,
        )),

        // ── Gamemode ───────────────────────────────────────────────────────
        "BrickComponentType_WireGraph_Exec_Gamemode_GetLeaderboardValue" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_GetLeaderboardValue",
            &["Key"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Gamemode_LeaderboardValue" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_LeaderboardValue",
            &["Key", "Value"],
            false,
        )),
        "BrickComponentType_WireGraph_Exec_Gamemode_GetTeamByName" => Some((
            "BrickComponentData_WireGraph_Exec_Gamemode_GetTeamByName",
            &["TeamName"],
            false,
        )),

        // ── Buffer ─────────────────────────────────────────────────────────
        "BrickComponentType_WireGraphPseudo_BufferTicks" => Some((
            "BrickComponentData_WireGraphPseudo_BufferTicks",
            &[
                "TicksToWait",
                "ZeroTicksToWait",
                "CurrentTicks",
                "Input",
                "Output",
                "Buffered",
                "bHasQueued",
                "bIsOffTimer",
            ],
            false,
        )),

        // ── Edge detector ──────────────────────────────────────────────────
        "BrickComponent_WireGraph_Expr_EdgeDetector" => Some((
            "BrickComponentData_WireGraph_Expr_EdgeDetector",
            &["Input", "bPulseOnRisingEdge", "bPulseOnFallingEdge"],
            false,
        )),

        // ── Fake events (character/controller/round) ───────────────────────
        "BrickComponentType_WireGraph_Fake_CharacterEvent" => Some((
            "BrickComponentData_WireGraph_Fake_CharacterEvent",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Fake_ControllerEvent" => Some((
            "BrickComponentData_WireGraph_Fake_ControllerEvent",
            &[],
            false,
        )),
        "BrickComponentType_WireGraph_Fake_RoundEvent" => Some((
            "BrickComponentData_WireGraph_Fake_RoundEvent",
            &["RoundNumber"],
            false,
        )),

        _ => None,
    }
}

fn coerce_for_prim_math(wv: WireVariant) -> WireVariant {
    match wv {
        WireVariant::Bool(b) => WireVariant::Int(if b { 1 } else { 0 }),
        other => other,
    }
}

fn var_type_to_wire_variant(ty: Option<&crate::ir::Type>) -> WireVariant {
    use crate::ir::Type;
    match ty {
        Some(Type::Bool) => WireVariant::Bool(false),
        Some(Type::Int) => WireVariant::Int(0),
        Some(Type::Controller | Type::Character | Type::Entity | Type::Brick | Type::Prefab) => {
            WireVariant::Object(None)
        }
        Some(Type::String) => WireVariant::Str(String::new()),
        Some(Type::Vector) => WireVariant::Vector(Vector3f { x: 0.0, y: 0.0, z: 0.0 }),
        Some(Type::Rotator) => WireVariant::Rotator { pitch: 0.0, yaw: 0.0, roll: 0.0 },
        Some(Type::Quat) => WireVariant::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        Some(Type::Color) => WireVariant::LinearColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
        _ => WireVariant::Number(0.0),
    }
}

/// Unwrap a `ref array<T>` (or bare `array<T>`) port type down to its element
/// type `T`.
fn array_element_type(ty: &crate::ir::Type) -> Option<&crate::ir::Type> {
    use crate::ir::Type;
    let inner = match ty {
        Type::Ref(inner) => inner.as_ref(),
        other => other,
    };
    match inner {
        Type::Array(elem) => Some(elem.as_ref()),
        _ => None,
    }
}

/// Empty `WireGraphArrayVariant` member matching the array's element type, so
/// the ArrayVar gate is created as the correct array kind.
fn empty_wire_array_variant(elem: Option<&crate::ir::Type>) -> WireArrayVariant {
    use crate::ir::Type;
    match elem {
        Some(Type::Int) => WireArrayVariant::Int64Array(Vec::new()),
        Some(Type::Bool) => WireArrayVariant::BoolArray(Vec::new()),
        Some(Type::String) => WireArrayVariant::StringArray(Vec::new()),
        Some(Type::Vector) => WireArrayVariant::VectorArray(Vec::new()),
        Some(Type::Rotator) => WireArrayVariant::RotatorArray(Vec::new()),
        Some(Type::Quat) => WireArrayVariant::QuatArray(Vec::new()),
        Some(Type::Color) => WireArrayVariant::LinearColorArray(Vec::new()),
        Some(Type::Controller | Type::Character | Type::Entity | Type::Brick | Type::Prefab) => {
            WireArrayVariant::ObjectArray(Vec::new())
        }
        _ => WireArrayVariant::DoubleArray(Vec::new()), // float + default
    }
}

/// Build a populated array variant from an array's constant initial elements.
/// The backing variant is chosen by the element type (matching
/// [`empty_wire_array_variant`]); each literal is read in that type.
fn wire_array_variant_from_literals(
    elem: Option<&crate::ir::Type>,
    lits: &[Literal],
) -> WireArrayVariant {
    use crate::ir::Type;
    let as_i64 = |l: &Literal| match l {
        Literal::Int(n) => *n,
        Literal::Float(f) => *f as i64,
        Literal::Bool(b) => *b as i64,
        _ => 0,
    };
    let as_f64 = |l: &Literal| match l {
        Literal::Float(f) => *f,
        Literal::Int(n) => *n as f64,
        Literal::Bool(b) => *b as i64 as f64,
        _ => 0.0,
    };
    match elem {
        Some(Type::Int) => WireArrayVariant::Int64Array(lits.iter().map(as_i64).collect()),
        Some(Type::Bool) => WireArrayVariant::BoolArray(
            lits.iter()
                .map(|l| matches!(l, Literal::Bool(true)) || matches!(l, Literal::Int(n) if *n != 0))
                .collect(),
        ),
        Some(Type::String) => WireArrayVariant::StringArray(
            lits.iter()
                .map(|l| match l {
                    Literal::String(s) => s.clone(),
                    _ => String::new(),
                })
                .collect(),
        ),
        Some(Type::Vector) => WireArrayVariant::VectorArray(
            lits.iter()
                .map(|l| match l {
                    Literal::Vector { x, y, z } => Vector3f {
                        x: *x as f32,
                        y: *y as f32,
                        z: *z as f32,
                    },
                    _ => Vector3f { x: 0.0, y: 0.0, z: 0.0 },
                })
                .collect(),
        ),
        Some(Type::Rotator) => WireArrayVariant::RotatorArray(
            lits.iter()
                .map(|l| match l {
                    Literal::Rotator { pitch, yaw, roll } => (*pitch, *yaw, *roll),
                    _ => (0.0, 0.0, 0.0),
                })
                .collect(),
        ),
        Some(Type::Quat) => WireArrayVariant::QuatArray(
            lits.iter()
                .map(|l| match l {
                    Literal::Quat { x, y, z, w } => (*x, *y, *z, *w),
                    _ => (0.0, 0.0, 0.0, 1.0),
                })
                .collect(),
        ),
        Some(Type::Color) => WireArrayVariant::LinearColorArray(
            lits.iter()
                .map(|l| match l {
                    Literal::LinearColor { r, g, b, a } => {
                        (*r as f32, *g as f32, *b as f32, *a as f32)
                    }
                    Literal::Color { r, g, b, a } => (
                        *r as f32 / 255.0,
                        *g as f32 / 255.0,
                        *b as f32 / 255.0,
                        *a as f32 / 255.0,
                    ),
                    _ => (1.0, 1.0, 1.0, 1.0),
                })
                .collect(),
        ),
        // Object arrays can't be initialised from literals.
        Some(Type::Controller | Type::Character | Type::Entity | Type::Brick | Type::Prefab) => {
            WireArrayVariant::ObjectArray(vec![None; lits.len()])
        }
        _ => WireArrayVariant::DoubleArray(lits.iter().map(as_f64).collect()), // float + default
    }
}

/// Convert a Literal to a Box<dyn AsBrdbValue> using the most appropriate
/// type. For wire_graph_variant fields, wraps in WireVariant. For plain
/// typed fields, uses the native type directly.
/// Convert a Literal to a Box<dyn AsBrdbValue> as a WireVariant.
fn literal_to_boxed_wire_variant(
    lit: &Literal,
    ports: &crate::ir::GateIO,
    field: &str,
) -> Box<dyn AsBrdbValue> {
    if let Some(wv) = literal_to_wire_variant(lit) {
        return Box::new(wv);
    }
    let port_ty = ports
        .inputs
        .iter()
        .chain(ports.outputs.iter())
        .find(|p| resolve(p.name) == field)
        .map(|p| &p.ty);
    Box::new(var_type_to_wire_variant(port_ty))
}

/// Convert a Literal to a Box<dyn AsBrdbValue> using native types (str/i32/f64/bool).
fn literal_to_string(lit: &Literal) -> String {
    match lit {
        Literal::String(s) => s.clone(),
        Literal::Int(n) => n.to_string(),
        Literal::Float(f) => f.to_string(),
        Literal::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn literal_to_boxed_native(lit: &Literal) -> Box<dyn AsBrdbValue> {
    match lit {
        Literal::String(s) => Box::new(s.clone()),
        Literal::Int(n) => Box::new(*n),
        Literal::Float(f) => Box::new(*f),
        Literal::Bool(b) => Box::new(*b),
        _ => Box::new(0i64),
    }
}

fn literal_to_wire_variant(lit: &Literal) -> Option<WireVariant> {
    match lit {
        Literal::Int(n) => Some(WireVariant::Int(*n)),
        Literal::Float(f) => Some(WireVariant::Number(*f)),
        Literal::Bool(b) => Some(WireVariant::Bool(*b)),
        Literal::Object => Some(WireVariant::Object(None)),
        Literal::String(s) => Some(WireVariant::Str(s.clone())),
        Literal::Vector { x, y, z } => Some(WireVariant::Vector(Vector3f {
            x: *x as f32,
            y: *y as f32,
            z: *z as f32,
        })),
        Literal::Rotator { pitch, yaw, roll } => Some(WireVariant::Rotator {
            pitch: *pitch,
            yaw: *yaw,
            roll: *roll,
        }),
        Literal::Quat { x, y, z, w } => Some(WireVariant::Quat {
            x: *x,
            y: *y,
            z: *z,
            w: *w,
        }),
        Literal::LinearColor { r, g, b, a } => Some(WireVariant::LinearColor {
            r: *r as f32,
            g: *g as f32,
            b: *b as f32,
            a: *a as f32,
        }),
        // sRGB byte color (brick paint) → linear-ish 0–1. Only reached if a
        // paint literal ends up on a wire-variant port.
        Literal::Color { r, g, b, a } => Some(WireVariant::LinearColor {
            r: *r as f32 / 255.0,
            g: *g as f32 / 255.0,
            b: *b as f32 / 255.0,
            a: *a as f32 / 255.0,
        }),
        Literal::Array(_) | Literal::Asset { .. } => None,
    }
}

/// Emit `.brz` bundle bytes — zstd-packed, portable, good for bundle
/// transfer and in-memory preview. `BR.World.LoadAdditive` doesn't accept
/// these directly; use [`emit_brdb`] for that.
pub fn emit_brz(
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<Vec<u8>, EmitError> {
    let world = build_world(module, placements, opts, template_cache)?;
    Ok(world.to_brz_vec()?)
}

/// Emit a `.brdb` SQLite database to `path`. This is the format
/// `BR.World.LoadAdditive <name>` reads from `Saved/Worlds/<name>.brdb`.
#[cfg(feature = "brdb-full")]
pub fn emit_brdb(
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
    path: impl AsRef<Path>,
) -> Result<(), EmitError> {
    let world = build_world(module, placements, opts, template_cache)?;
    world.write_brdb(path)?;
    Ok(())
}

fn is_spawnable(kind: NodeKind, gate_class: &str) -> bool {
    if gate_class == gc::LITERAL || gate_class == gc::UNSUPPORTED {
        return false;
    }
    matches!(
        kind,
        NodeKind::Gate | NodeKind::Event | NodeKind::Input | NodeKind::Output
    )
}

/// Map a component class name to the brick asset that hosts it.
///
/// Uses the bundled catalog for the authoritative mapping. Falls back to
/// `B_1x1_Reroute_Node` for any class not found in the catalog (e.g.,
/// synthetic IR-only nodes that shouldn't reach here).
fn infer_brick_for_gate(component_class: &str) -> String {
    crate::catalog::default_catalog()
        .find_by_class(component_class)
        .map(|g| g.brick_asset.clone())
        .unwrap_or_else(|| "B_1x1_Reroute_Node".to_string())
}

fn wire_to_connection_indexed(
    w: &Wire,
    node_brick_ids: &HashMap<NodeId, usize>,
    class_index: &HashMap<NodeId, &'static str>,
    port_index: &HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)>,
) -> Result<WireConnection, EmitError> {
    let resolve_end = |node_id: NodeId,
                       port_idx: WirePort|
     -> Result<(usize, &'static str, &'static str), EmitError> {
        let port_str: &'static str = port_idx.as_str();
        let key = (node_id, port_str);
        if let Some(&(bid, cls, port)) = port_index.get(&key) {
            return Ok((bid, cls, port));
        }
        let bid = *node_brick_ids
            .get(&node_id)
            .ok_or_else(|| EmitError::UnknownWireNode(node_id.to_string()))?;
        let cls = *class_index
            .get(&node_id)
            .ok_or_else(|| EmitError::UnknownWireNode(node_id.to_string()))?;
        Ok((bid, cls, port_str))
    };

    let (src_brick, src_class, src_port) = resolve_end(w.source.node_id, w.source.port)?;
    let (tgt_brick, tgt_class, tgt_port) = resolve_end(w.target.node_id, w.target.port)?;

    Ok(WireConnection {
        source: BrdbWirePort {
            brick_id: src_brick,
            component_type: BString::Static(src_class),
            port_name: BString::Static(src_port),
        },
        target: BrdbWirePort {
            brick_id: tgt_brick,
            component_type: BString::Static(tgt_class),
            port_name: BString::Static(tgt_port),
        },
    })
}

// ---------- semantic colouring ----------

// Brickadia renders brick colours in linear RGB, so the raw bytes here
// are perceptually much brighter than the same sRGB values would be
// elsewhere. These values are sRGB targets passed through γ=2.2 to
// approximate the intended perceived colour in-game.
const C_YELLOW: Color = Color {
    r: 124,
    g: 74,
    b: 1,
}; // triggers + chip I/O
const C_WHITE: Color = Color {
    r: 124,
    g: 124,
    b: 124,
}; // branch / union / select
const C_GREY: Color = Color {
    r: 16,
    g: 16,
    b: 16,
}; // exec-taking statements
const C_INT: Color = Color {
    r: 4,
    g: 124,
    b: 148,
}; // int — cyan
const C_FLOAT: Color = Color { r: 4, g: 74, b: 16 }; // float — green
const C_BOOL: Color = Color { r: 113, g: 4, b: 4 }; // bool — red
const C_STRING: Color = Color {
    r: 124,
    g: 93,
    b: 2,
}; // string — yellow
const C_CHARACTER: Color = Color { r: 1, g: 2, b: 66 }; // character — deep blue
const C_STRUCT: Color = Color {
    r: 124,
    g: 39,
    b: 2,
}; // vector/struct/entity — orange

/// Choose a brick colour for `node` following the scheme:
/// - Events + chip I/O → yellow
/// - Branch / union / select → white
/// - Pseudo-storage vars → coloured by inner type
/// - Var_Get / Var_Set / Var_Increment → coloured by the var they touch
/// - Other exec-taking statement gates → grey
/// - Pure expressions → coloured by their output type
fn color_for_node(
    node: &Node,
    module: &Module,
    wire_target_index: &StdMap<(NodeId, WirePort), NodeId>,
) -> Color {
    if matches!(
        node.kind,
        NodeKind::Event | NodeKind::Input | NodeKind::Output
    ) {
        return C_YELLOW;
    }
    if node.gate_class.contains("Exec_Branch")
        || node.gate_class.contains("Exec_Union")
        || node.gate_class.contains("Expr_Select")
    {
        return C_WHITE;
    }
    let is_pseudo = node
        .gate_class
        .starts_with("BrickComponentType_WireGraphPseudo");
    if is_pseudo {
        if let Some(t) = node
            .ports
            .outputs
            .iter()
            .find(|p| {
                let pn = resolve(p.name);
                pn == "Value" || pn == "Output"
            })
            .map(|p| &p.ty)
        {
            return color_for_type(t);
        }
        return C_STRUCT;
    }
    if node.gate_class.contains("Exec_Var_") || node.gate_class.contains("Exec_ArrayVar_") {
        if let Some(ty) = var_ref_target_type(node, module, wire_target_index) {
            return color_for_type(&ty);
        }
        if let Some(ref_port) = node.ports.inputs.iter().find(|p| {
            let pn = resolve(p.name);
            pn == "VarRef" || pn == "ArrayVarRef"
        }) {
            return color_for_type(&ref_port.ty);
        }
    }
    let takes_exec = node.ports.inputs.iter().any(|p| matches!(p.ty, Type::Exec));
    if takes_exec {
        return C_GREY;
    }
    node.ports
        .outputs
        .iter()
        .find(|p| !matches!(p.ty, Type::Exec))
        .map(|p| color_for_type(&p.ty))
        .unwrap_or(C_GREY)
}

/// For a Var_Get/Var_Set style gate, follow its `VarRef` / `ArrayVarRef`
/// input wire back to the Pseudo_Var source and return that var's inner
/// type. Uses pre-built wire_target_index for O(1) lookup.
fn var_ref_target_type(
    node: &Node,
    module: &Module,
    wire_target_index: &StdMap<(NodeId, WirePort), NodeId>,
) -> Option<Type> {
    let ref_port_sym = node
        .ports
        .inputs
        .iter()
        .find(|p| {
            let pn = resolve(p.name);
            pn == "VarRef" || pn == "ArrayVarRef"
        })
        .map(|p| p.name)?;
    let ref_port_idx = WirePort::from_name(resolve(ref_port_sym));
    let src = wire_target_index.get(&(node.id, ref_port_idx))?;
    let var_node = module.nodes.get(src)?;
    var_node
        .ports
        .outputs
        .iter()
        .find(|p| {
            let pn = resolve(p.name);
            pn == "Value" || pn == "Output"
        })
        .map(|p| p.ty.clone())
}

fn color_for_type(t: &Type) -> Color {
    match t {
        Type::Int => C_INT,
        Type::Float => C_FLOAT,
        Type::Bool => C_BOOL,
        Type::String => C_STRING,
        Type::Character => C_CHARACTER,
        // Ref/Array wrappers: unwrap and recurse so a `Ref<Int>` still
        // colours as int.
        Type::Ref(inner) | Type::Array(inner) => color_for_type(inner),
        // Everything else (Vector, Rotator, Color, Entity, Controller,
        // Brick, Prefab, Record, Tuple, Union, Any, Never, Exec) falls
        // back to the struct-ish light-orange bucket.
        _ => C_STRUCT,
    }
}

/// Check if `schema_str` contains `"field_name: type_name"` as an exact type match.
fn schema_field_type_str(struct_name: &str, field: &str) -> Option<String> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let s = schema.get_struct(struct_name)?;
    let field_id = schema.intern.get(field)?;
    let prop = s.get(&field_id)?;
    Some(prop.as_string(schema))
}

fn schema_field_is(struct_name: &str, field: &str, type_name: &str) -> bool {
    schema_field_type_str(struct_name, field).as_deref() == Some(type_name)
}

/// Can a folded constant (`Vec/Rotation/Color` on literal args, lowered to a
/// `_Literal` node) be delivered to this (gate, port) sink as inlined
/// component data? True only for fields `build_gate_component` writes as wire
/// variants. Everything else — entity gates with plain struct fields,
/// `Split*` inputs, chip IO, unmapped gates — must keep a real `Make*` gate,
/// which the lowering pass materializes on demand.
pub(crate) fn port_accepts_inline_variant(gate_class: &str, port: WirePort) -> bool {
    let Some((struct_name, fields, use_wire_variant)) = data_struct_for_gate(gate_class) else {
        return false;
    };
    let field = port.as_str();
    if !fields.contains(&field) {
        return false;
    }
    use_wire_variant
        || matches!(
            schema_field_type_str(struct_name, field).as_deref(),
            Some(
                "wire_graph_variant"
                    | "wire_graph_prim_math_variant"
                    | "WireGraphVariant"
                    | "WireGraphPrimMathVariant"
            )
        )
}

/// If the field's schema type is an enum, resolve `lit` to its integer
/// discriminant. Accepts both `Literal::Int` (passthrough) and
/// `Literal::String` (looked up by variant name, with or without the
/// enum-name prefix).
fn try_resolve_enum(struct_name: &str, field: &str, lit: &Literal) -> Option<u8> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let type_name = schema_field_type_str(struct_name, field)?;
    let enum_def = schema.get_enum(&type_name)?;
    match lit {
        Literal::Int(n) => Some(*n as u8),
        Literal::String(s) => {
            // Try exact match first ("EBRDisplayTextJustification::Left"),
            // then bare suffix ("Left").
            if let Some(id) = schema.intern.get(s) {
                if let Some(&v) = enum_def.get(&id) {
                    return Some(v as u8);
                }
            }
            let prefixed = format!("{type_name}::{s}");
            let id = schema.intern.get(&prefixed)?;
            Some(*enum_def.get(&id)? as u8)
        }
        Literal::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY_TEXT: &str = "BrickComponentData_WireGraph_Exec_Controller_DisplayText";

    #[test]
    fn var_values_cover_all_variant_members() {
        use crate::ir::Type;
        // A var can hold any WireGraphVariant member, defaulted by its type.
        assert!(matches!(var_type_to_wire_variant(Some(&Type::Bool)), WireVariant::Bool(false)));
        assert!(matches!(var_type_to_wire_variant(Some(&Type::Int)), WireVariant::Int(0)));
        assert!(matches!(var_type_to_wire_variant(Some(&Type::Float)), WireVariant::Number(_)));
        assert!(matches!(var_type_to_wire_variant(Some(&Type::String)), WireVariant::Str(_)));
        assert!(matches!(var_type_to_wire_variant(Some(&Type::Vector)), WireVariant::Vector(_)));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Rotator)),
            WireVariant::Rotator { .. }
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Quat)),
            WireVariant::Quat { w, .. } if w == 1.0
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Color)),
            WireVariant::LinearColor { .. }
        ));
        assert!(matches!(var_type_to_wire_variant(Some(&Type::Entity)), WireVariant::Object(None)));
        // Literal initializers convert to the matching variant member.
        assert!(matches!(literal_to_wire_variant(&Literal::String("x".into())), Some(WireVariant::Str(_))));
        assert!(matches!(
            literal_to_wire_variant(&Literal::Vector { x: 1.0, y: 2.0, z: 3.0 }),
            Some(WireVariant::Vector(_))
        ));
    }

    #[test]
    fn array_element_type_maps_to_array_variant() {
        use crate::ir::Type;
        let r = |t: Type| Type::Ref(Box::new(Type::Array(Box::new(t))));
        // element type is unwrapped through `ref array<T>`
        assert!(matches!(array_element_type(&r(Type::Int)), Some(Type::Int)));
        // each scalar element type selects the matching array variant member
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Int)),
            WireArrayVariant::Int64Array(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Float)),
            WireArrayVariant::DoubleArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Bool)),
            WireArrayVariant::BoolArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::String)),
            WireArrayVariant::StringArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Vector)),
            WireArrayVariant::VectorArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Entity)),
            WireArrayVariant::ObjectArray(_)
        ));
        // unknown / missing element type falls back to a double array
        assert!(matches!(
            empty_wire_array_variant(None),
            WireArrayVariant::DoubleArray(_)
        ));
    }

    #[test]
    fn make_vector_has_data_struct_so_literals_persist() {
        // Regression: without this entry the inlined X/Y/Z literals of
        // `Vec(1.0, 2.0, 3.0)` are dropped at emit and the vector reads (0,0,0).
        let entry = data_struct_for_gate(crate::ir::gate_class::MAKE_VECTOR);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Expr_MakeVector",
                ["X", "Y", "Z"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn nearly_equal_has_data_struct_so_literals_persist() {
        // Regression: without this entry a literal `b`/tolerance arg of
        // `NearlyEqual(x, 1.0, 0.001)` drops to 0, so comparisons against any
        // non-zero constant always fail.
        let entry = data_struct_for_gate(crate::ir::gate_class::NEARLY_EQUAL);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Expr_NearlyEqual",
                ["InputA", "InputB", "Tolerance"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn enum_resolve_bare_name() {
        let v = try_resolve_enum(
            DISPLAY_TEXT,
            "Justification",
            &Literal::String("Left".into()),
        );
        assert_eq!(v, Some(0));
    }

    #[test]
    fn enum_resolve_prefixed_name() {
        let v = try_resolve_enum(
            DISPLAY_TEXT,
            "Justification",
            &Literal::String("EBRDisplayTextJustification::Center".into()),
        );
        assert_eq!(v, Some(1));
    }

    #[test]
    fn enum_resolve_int_passthrough() {
        let v = try_resolve_enum(DISPLAY_TEXT, "Justification", &Literal::Int(2));
        assert_eq!(v, Some(2));
    }

    #[test]
    fn enum_resolve_unknown_name_returns_none() {
        let v = try_resolve_enum(
            DISPLAY_TEXT,
            "Justification",
            &Literal::String("Nonsense".into()),
        );
        assert_eq!(v, None);
    }

    #[test]
    fn enum_resolve_easing_function_and_direction() {
        const EASING: &str = "BrickComponentData_WireGraph_Expr_MathEasing";
        // Named easing functions/directions resolve to their engine enum ints.
        assert_eq!(try_resolve_enum(EASING, "Function", &Literal::String("Quad".into())), Some(2));
        assert_eq!(try_resolve_enum(EASING, "Function", &Literal::String("Cubic".into())), Some(3));
        assert_eq!(try_resolve_enum(EASING, "Direction", &Literal::String("InOut".into())), Some(2));
        assert_eq!(try_resolve_enum(EASING, "Direction", &Literal::String("Out".into())), Some(1));
        // ints pass through
        assert_eq!(try_resolve_enum(EASING, "Function", &Literal::Int(5)), Some(5));
    }

    #[test]
    fn enum_resolve_non_enum_field_returns_none() {
        let v = try_resolve_enum(DISPLAY_TEXT, "FontSize", &Literal::String("Left".into()));
        assert_eq!(v, None);
    }

    #[test]
    fn enum_resolve_easing_field_is_f64_not_enum() {
        assert_eq!(
            try_resolve_enum(
                DISPLAY_TEXT,
                "Transition",
                &Literal::String("Linear".into())
            ),
            None,
        );
    }

    #[test]
    fn enum_resolve_brick_direction() {
        let item_spawn = "BrickComponentData_ItemSpawn";
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupOffsetDirection",
                &Literal::String("X_Positive".into())
            ),
            Some(0),
        );
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupOffsetDirection",
                &Literal::String("Z_Negative".into())
            ),
            Some(5),
        );
    }

    #[test]
    fn enum_resolve_brick_axis() {
        let item_spawn = "BrickComponentData_ItemSpawn";
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupAnimationAxis",
                &Literal::String("X".into())
            ),
            Some(0),
        );
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupAnimationAxis",
                &Literal::String("Z".into())
            ),
            Some(2),
        );
    }

    #[test]
    fn enum_resolve_collision_channel() {
        let bot_spawn = "BrickComponentData_BotSpawn";
        assert_eq!(
            try_resolve_enum(
                bot_spawn,
                "TeamCollisionChannel",
                &Literal::String("Channel1".into())
            ),
            Some(0),
        );
        assert_eq!(
            try_resolve_enum(
                bot_spawn,
                "TeamCollisionChannel",
                &Literal::String("Channel4".into())
            ),
            Some(3),
        );
    }
}
