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
    WirePort as BrdbWirePort, World,
    assets::LiteralComponent,
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

/// Resolves a prefab file reference (`$./file.brz` / `$/abs/file.brz`) to the
/// raw `.brz` bytes to embed. The argument is the source-level path (after the
/// `$`). Frontends supply this: the CLI reads from disk relative to the source
/// file; the wasm/playground sandbox looks up dragged-in files. `Err` carries a
/// human-readable reason (missing file, read error) surfaced as an emit error.
#[derive(Clone)]
pub struct PrefabResolver(
    pub std::sync::Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>,
);

impl PrefabResolver {
    pub fn new(f: impl Fn(&str) -> Result<Vec<u8>, String> + Send + Sync + 'static) -> Self {
        PrefabResolver(std::sync::Arc::new(f))
    }
    fn resolve(&self, path: &str) -> Result<Vec<u8>, String> {
        (self.0)(path)
    }
}

impl std::fmt::Debug for PrefabResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrefabResolver(..)")
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
    /// Resolves `$./file.brz` / `$/abs/file.brz` prefab references to bytes.
    /// `None` makes any prefab reference an emit error.
    pub prefab_resolver: Option<PrefabResolver>,
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
            prefab_resolver: None,
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
    #[error("prefab reference `${0}`: {1}")]
    PrefabResolve(String, String),
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

    // Parent-side Literal nodes we clone into chips (below); cleaned up after.
    let mut cloned_literal_sources: HashSet<NodeId> = HashSet::new();

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
        // Per-chip dedupe of Literal nodes cloned into this chip.
        let mut literal_clones: HashMap<NodeId, NodeId> = HashMap::new();
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
                    // A parent-side constant `Literal` feeding a node that moved
                    // into this chip: keeping the plain data wire in the parent
                    // leaves it dangling (its target is now in the child), so the
                    // chip-side input silently reads the port default (0). Vars
                    // cross the boundary via a Ref port; a Literal has none — so
                    // clone it into the child and keep the wire internal.
                    let is_literal = module
                        .nodes
                        .get(&w.source.node_id)
                        .map(|n| n.gate_class == gc::LITERAL)
                        .unwrap_or(false);
                    if is_literal {
                        let existing = literal_clones.get(&w.source.node_id).copied();
                        let clone_id = if let Some(id) = existing {
                            id
                        } else {
                            let mut cl = module.nodes[&w.source.node_id].clone();
                            let nid = NodeId::fresh();
                            cl.id = nid;
                            cl.chip_id = None;
                            child.nodes.insert(nid, cl);
                            literal_clones.insert(w.source.node_id, nid);
                            cloned_literal_sources.insert(w.source.node_id);
                            nid
                        };
                        let mut w2 = w;
                        w2.source.node_id = clone_id;
                        child.wires.push(w2);
                        continue;
                    }
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

    // Drop parent-side Literal nodes that were fully cloned into chips and now
    // have no remaining parent consumer, so they don't emit as stray gates.
    for lit_id in cloned_literal_sources {
        if !module.wires.iter().any(|w| w.source.node_id == lit_id) {
            module.nodes.remove(&lit_id);
        }
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

    // Top-level chip label: the root module's name (entry file stem, or an
    // explicit module_name override). The chip brick is the one
    // `add_microchip` just pushed onto the main grid.
    let root_label = resolve(module.name);
    if !root_label.is_empty() {
        let label = text_label(&mut world, root_label, 0.0, -0.5, LABEL_LINE_HEIGHT);
        if let Some(chip_brick) = world.bricks.last_mut() {
            chip_brick.add_component_box(Box::new(label));
        }
    }

    // Push root inner grid FIRST so it gets the lowest grid ID (persistent
    // index 2). Child grids pushed during emit_module_bricks get 3, 4, etc.
    let root_grid_idx = world.grids.len();
    world.grids.push((inner_pair.0.clone(), Vec::new()));

    let mut ctx = EmitContext {
        node_brick_ids: HashMap::new(),
        class_index: HashMap::new(),
        prefab_resolver: opts.prefab_resolver.clone(),
        wire_sources: HashMap::new(),
        var_labels: HashMap::new(),
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

/// Name labels on chips, vars, and I/O gates.
const LABEL_LINE_HEIGHT: f32 = 2.4;
/// Smaller tag on Var_Get/Set-style gates naming the variable they touch.
const VAR_TAG_LINE_HEIGHT: f32 = 1.2;

/// Floating text-label component (`Component_TextDisplay`) attached as a
/// second component on chip / variable / I/O-gate bricks, showing the
/// element's name. Fields left unset (colors, outline widths, sharp
/// corners, …) are filled from brdb's `STRUCT_DEFAULTS`.
fn text_label(
    world: &mut World,
    text: &str,
    rotation_deg: f32,
    offset_z: f32,
    line_height: f32,
) -> LiteralComponent {
    use brdb::schema::BrdbValue;
    let (font_idx, _) = world.global_data.external_asset_references.insert_full((
        "BrickFontDescriptor".to_string(),
        "IosevkaTerm".to_string(),
    ));
    let anchor = LiteralComponent::new("Vector2f").with_data([
        ("X", Box::new(0.5f32) as Box<dyn AsBrdbValue>),
        ("Y", Box::new(0.5f32)),
    ]);
    LiteralComponent::new("Component_TextDisplay").with_data([
        ("Text", Box::new(text.to_string()) as Box<dyn AsBrdbValue>),
        ("Font", Box::new(BrdbValue::Asset(Some(font_idx)))),
        ("Rotation", Box::new(rotation_deg)),
        ("LineHeight", Box::new(line_height)),
        ("Anchor", Box::new(anchor)),
        (
            "Offset",
            Box::new(Vector3f {
                x: 0.0,
                y: 0.0,
                z: offset_z,
            }),
        ),
        // Top face of the brick (enum default 0 is X_Positive).
        ("Face", Box::new(4u8)),
        // EBRTextOutline::Outlined; the enum default (None) hides the
        // outline entirely, and 4px reads better than the template's 2.
        ("Outline", Box::new(2u8)),
        ("OutlineWidth", Box::new(4.0f32)),
    ])
}

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
    /// Resolver for `$./file.brz` prefab references, from `EmitOptions`.
    prefab_resolver: Option<PrefabResolver>,
    /// (target node, target port) → source node, accumulated across all
    /// modules so Var_Get/Set gates can trace `VarRef` wires that cross
    /// module boundaries (scope captures, anon-chip partitions).
    wire_sources: HashMap<(NodeId, WirePort), NodeId>,
    /// Pseudo_Var/ArrayVar node → its labelable source name. Vars are
    /// always emitted before the gates that reference them.
    var_labels: HashMap<NodeId, String>,
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
        let pos = placements
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
                    Some(Literal::Array(lits)) => wire_array_variant_from_literals(elem_ty, lits),
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
                let entry = LiteralComponent::new("BRInventoryEntryConfig")
                    .with_data([("Item", Box::new(()) as Box<dyn AsBrdbValue>)]);
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
            )?,
        };
        // EMIT_COMP_NS.fetch_add(_ct.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
        brick.add_component_box(Box::new(comp));

        // Second component: floating name label on I/O gates and variables.
        // (Chip bricks get theirs in pass 2 / build_world.)
        // All kinds float the label above the gate brick (Offset.z +0.5;
        // the chip-shell template value of -0.5 sinks it into these bricks).
        // `_`-prefixed names are synthesized plumbing (e.g. a chip's
        // `_exec_in`/`_exec_out` ports) — not worth a label.
        let named = |l: &Literal| match l {
            Literal::String(s) if !s.is_empty() && !s.starts_with('_') => Some(s.clone()),
            _ => None,
        };
        let label_spec = match effective_class_str {
            "BrickComponentType_Internal_MicrochipInput"
            | "BrickComponentType_Internal_MicrochipOutput" => node
                .properties
                .get(&port_label_sym)
                .and_then(named)
                .map(|s| (s, LABEL_LINE_HEIGHT)),
            "BrickComponentType_WireGraphPseudo_Var"
            | "BrickComponentType_WireGraphPseudo_ArrayVar" => {
                let label = node.properties.get(&*sym::NAME_LABEL).and_then(named);
                if let Some(name) = &label {
                    ctx.var_labels.insert(**id, name.clone());
                }
                label.map(|s| (s, LABEL_LINE_HEIGHT))
            }
            // Var/ArrayVar exec gates: a smaller tag naming the variable
            // they access, traced through the gate's (Array)VarRef wire.
            // The var node is always emitted first, so its label is known.
            c if c.starts_with("BrickComponentType_WireGraph_Exec_Var_")
                || c.starts_with("BrickComponentType_WireGraph_Exec_ArrayVar_") =>
            {
                node.ports
                    .inputs
                    .iter()
                    .find(|p| matches!(resolve(p.name), "VarRef" | "ArrayVarRef"))
                    .and_then(|p| {
                        let port = WirePort::from_name(resolve(p.name));
                        ctx.wire_sources.get(&(node.id, port))
                    })
                    .and_then(|src| ctx.var_labels.get(src))
                    .map(|s| (s.clone(), VAR_TAG_LINE_HEIGHT))
            }
            _ => None,
        };
        if let Some((text, line_height)) = label_spec {
            brick.add_component_box(Box::new(text_label(world, &text, -45.0, 0.5, line_height)));
        }

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

        let child_layout = crate::layout::layout_root(child_module);
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

        let (mut chip_brick, chip_brick_id) = brdb::Brick {
            asset: brdb::assets::bricks::B_MICROCHIP,
            position: inner_pos,
            ..Default::default()
        }
        .with_component_box(Box::new(LiteralComponent::new(
            "Component_Internal_Microchip",
        )))
        .with_id_split();
        // Named chips get a floating name label; anonymous groupings
        // (ModuleRoot-scoped partitions) stay unlabeled.
        if let Some(crate::ir::ScopeInfo {
            kind: crate::ir::ScopeKind::ChipBody { name },
            ..
        }) = child_module.scopes.get(&crate::ir::ROOT_SCOPE_ID)
            && !name.is_empty()
        {
            chip_brick.add_component_box(Box::new(text_label(
                world,
                name,
                -45.0,
                -0.5,
                LABEL_LINE_HEIGHT,
            )));
        }
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
    prefab_resolver: Option<&PrefabResolver>,
) -> Result<LiteralComponent, EmitError> {
    // For gates whose component data struct carries wire_graph_variant fields,
    // look up the struct schema and build the component with embedded values.
    // Non-inlined fields get a default (Int(0)) so the struct is always complete.
    // Always write the data struct when the gate type has one — even if no
    // fields are inlined. This ensures ALL instances of the same component
    // type have matching data, preventing reader misalignment.
    if let Some(fields) = gate_field_meta(gate_class) {
        let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
        for fm in fields {
            let field = fm.name;
            let is_variant = matches!(
                fm.kind,
                FieldKind::WireVariant | FieldKind::PrimMathVariant
            );
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
                            // A non-prefab literal here can't happen via the
                            // front end (the port only accepts `$…` refs).
                            _ => Box::new(String::new()),
                        }
                    }
                    FieldKind::Enum(type_name) => match resolve_enum_value(type_name, lit) {
                        Some(ev) => Box::new(ev),
                        None => literal_to_boxed_native(lit),
                    },
                    FieldKind::Str => Box::new(literal_to_string(lit)),
                    _ => literal_to_boxed_native(lit),
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
        return Ok(LiteralComponent::new_from_data(
            gate_class,
            std::sync::Arc::new(data),
        ));
    }

    // Default: dataless component stub — registers component type only,
    // no struct data (engine uses the default from the brick type).
    Ok(LiteralComponent::new(gate_class))
}

/// Returns `(struct_name, field_names, use_wire_variant)` for gates whose
/// component data struct must be serialized.
///
/// Fully derived: `brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS` (the
/// game-extracted component→struct table, exhaustive over the game's
/// components) supplies the struct, the max schema supplies the field list —
/// see [`derived_gate_data`]. A class in neither place has no data component.
/// Per-field schema checks in `build_gate_component` decide variant vs native
/// handling, so the `use_wire_variant` flag is always false here.
fn data_struct_for_gate(gate_class: &str) -> Option<(&'static str, &'static [&'static str], bool)> {
    derived_gate_data()
        .get(gate_class)
        .map(|(s, f)| (*s, f.as_slice(), false))
}

/// (struct name, full field list) per component class, derived from
/// `brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS` + the max schema.
/// Hand-written arms in [`data_struct_for_gate`] take precedence — they
/// encode the deliberate exceptions (wire-only gates, struct overrides,
/// classes absent from the pair table).
fn derived_gate_data() -> &'static StdMap<&'static str, (&'static str, Vec<&'static str>)> {
    static MAP: std::sync::OnceLock<StdMap<&'static str, (&'static str, Vec<&'static str>)>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut m = StdMap::new();
        for (comp, strct) in brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS {
            let Some(s) = schema.get_struct(strct) else {
                continue;
            };
            let fields: Vec<&'static str> = s
                .keys()
                .filter_map(|k| schema.intern.lookup_ref(*k))
                .collect();
            m.insert(*comp, (*strct, fields));
        }
        m
    })
}

/// Per-field emit classification, resolved once per gate data struct
/// instead of re-querying the schema (several interner probes plus a
/// `String` allocation per predicate) for every field of every brick.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    /// `WireGraphVariant`
    WireVariant,
    /// `WireGraphPrimMathVariant` (no bool member — bools coerce to int)
    PrimMathVariant,
    /// Asset reference (`class` / `object`)
    AssetRef,
    /// `bundle_path_ref` (embedded prefab path)
    BundlePathRef,
    /// `str`
    Str,
    /// Schema enum type (payload = the enum's type name in the schema)
    Enum(&'static str),
    /// Anything else — serialized as a native literal
    Native,
}

struct FieldMeta {
    name: &'static str,
    /// `name` pre-interned, so the per-brick inlined-literal lookup skips
    /// the interner.
    sym: Sym,
    kind: FieldKind,
}

/// Field metadata for a gate's component data struct: same source as
/// [`derived_gate_data`] (pair table × max schema), with each field's
/// emit classification and interned name computed once.
fn gate_field_meta(gate_class: &str) -> Option<&'static [FieldMeta]> {
    use brdb::schema::BrdbSchemaStructProperty;
    static MAP: std::sync::OnceLock<StdMap<&'static str, Vec<FieldMeta>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut m = StdMap::new();
        for (comp, strct) in brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS {
            let Some(s) = schema.get_struct(strct) else {
                continue;
            };
            let fields: Vec<FieldMeta> = s
                .iter()
                .filter_map(|(k, prop)| {
                    let name = schema.intern.lookup_ref(*k)?;
                    let kind = match prop {
                        BrdbSchemaStructProperty::Type(t) => {
                            match schema.intern.lookup_ref(*t) {
                                Some("WireGraphVariant") => FieldKind::WireVariant,
                                Some("WireGraphPrimMathVariant") => FieldKind::PrimMathVariant,
                                Some("class") | Some("object") => FieldKind::AssetRef,
                                Some("bundle_path_ref") => FieldKind::BundlePathRef,
                                Some("str") => FieldKind::Str,
                                Some(n) if schema.get_enum(n).is_some() => FieldKind::Enum(n),
                                _ => FieldKind::Native,
                            }
                        }
                        // Array/FlatArray/Map fields have no special emit
                        // handling — the native literal path covers them.
                        _ => FieldKind::Native,
                    };
                    Some(FieldMeta {
                        name,
                        sym: intern_static(name),
                        kind,
                    })
                })
                .collect();
            m.insert(*comp, fields);
        }
        m
    })
    .get(gate_class)
    .map(|f| f.as_slice())
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
        Some(Type::Vector) => WireVariant::Vector(Vector3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        Some(Type::Rotator) => WireVariant::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        },
        Some(Type::Quat) => WireVariant::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        Some(Type::Color) => WireVariant::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
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
                .map(|l| {
                    matches!(l, Literal::Bool(true)) || matches!(l, Literal::Int(n) if *n != 0)
                })
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
                    _ => Vector3f {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
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
        Literal::Vector { x, y, z } => Box::new(VectorValue { x: *x, y: *y, z: *z }),
        Literal::Rotator { pitch, yaw, roll } => Box::new(RotatorValue {
            pitch: *pitch,
            yaw: *yaw,
            roll: *roll,
        }),
        Literal::Quat { x, y, z, w } => Box::new(QuatValue {
            x: *x,
            y: *y,
            z: *z,
            w: *w,
        }),
        _ => Box::new(0i64),
    }
}

// Folded literals embedded into native f64 struct fields (the schema's
// `Vector`/`Rotator`/`Quat` structs) — brdb's Vector3f/Quat4f are f32, so
// these mirror its AsBrdbValue impl at full precision.
struct VectorValue {
    x: f64,
    y: f64,
    z: f64,
}
impl AsBrdbValue for VectorValue {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            n => unimplemented!("unimplemented Vector field {n}"),
        }
    }
}

struct RotatorValue {
    pitch: f64,
    yaw: f64,
    roll: f64,
}
impl AsBrdbValue for RotatorValue {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "Pitch" => Ok(&self.pitch),
            "Yaw" => Ok(&self.yaw),
            "Roll" => Ok(&self.roll),
            n => unimplemented!("unimplemented Rotator field {n}"),
        }
    }
}

struct QuatValue {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}
impl AsBrdbValue for QuatValue {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &brdb::schema::BrdbSchema,
        _struct_name: brdb::schema::BrdbInterned,
        prop_name: brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, brdb::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            "W" => Ok(&self.w),
            n => unimplemented!("unimplemented Quat field {n}"),
        }
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
        Literal::Array(_) | Literal::Asset { .. } | Literal::PrefabRef { .. } => None,
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

// Brickadia renders stored brick-colour bytes as sRGB directly (a raw
// paint value like 60,160,240 shows up as that same bright blue in-game),
// so these are the perceived sRGB colours we want, used verbatim. (They
// were previously pre-darkened by γ=2.2 on the assumption the game decoded
// them from linear — that double-darkened every gate brick.)
const C_YELLOW: Color = Color {
    r: 184,
    g: 145,
    b: 21,
}; // triggers + chip I/O
const C_WHITE: Color = Color {
    r: 184,
    g: 184,
    b: 184,
}; // branch / union / select
const C_GREY: Color = Color {
    r: 72,
    g: 72,
    b: 72,
}; // exec-taking statements
const C_INT: Color = Color {
    r: 39,
    g: 184,
    b: 199,
}; // int — cyan
const C_FLOAT: Color = Color {
    r: 39,
    g: 145,
    b: 72,
}; // float — green
const C_BOOL: Color = Color {
    r: 176,
    g: 39,
    b: 39,
}; // bool — red
const C_STRING: Color = Color {
    r: 184,
    g: 161,
    b: 28,
}; // string — yellow
const C_CHARACTER: Color = Color {
    r: 21,
    g: 28,
    b: 138,
}; // character — deep blue
const C_STRUCT: Color = Color {
    r: 184,
    g: 109,
    b: 28,
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


/// The enum variant names a gate's data `field` accepts, if that field is an
/// enum — e.g. DisplayText's `Justification` → `["Left", "Center", "Right"]`.
/// Names are returned bare (the `EnumType::` prefix stripped). Used by the
/// editor to complete enum-valued named args like `justify = Center`.
pub fn field_enum_values(gate_class: &str, field: &str) -> Option<Vec<String>> {
    let (struct_name, _, _) = data_struct_for_gate(gate_class)?;
    let schema = brdb::schemas::bricks_components_schema_max();
    let type_name = schema_field_type_str(struct_name, field)?;
    let enum_def = schema.get_enum(&type_name)?;
    let mut out: Vec<String> = enum_def
        .keys()
        .filter_map(|k| schema.intern.lookup_ref(*k))
        .map(|name| name.rsplit("::").next().unwrap_or(name).to_string())
        .collect();
    out.dedup();
    if out.is_empty() { None } else { Some(out) }
}

/// True if the field's schema type is the prim-math wire variant, which the
/// current brdb schema interns as the named variant `WireGraphPrimMathVariant`
/// (the legacy `wire_graph_prim_math_variant` primitive spelling is no longer
/// used). The emit's Bool→Int coercion hangs off this predicate: the variant
/// has no `bool` member, so a missed match writes a `WireVariant::Bool` that the
/// brdb schema writer rejects.

/// True if the field's schema type is any wire-graph variant — plain
/// (`WireGraphVariant`) or prim-math (`WireGraphPrimMathVariant`).
fn schema_field_is_wire_variant(struct_name: &str, field: &str) -> bool {
    matches!(
        schema_field_type_str(struct_name, field).as_deref(),
        Some("WireGraphVariant" | "WireGraphPrimMathVariant")
    )
}

/// Can a folded constant (`Vec/Rotation/Color` on literal args, lowered to a
/// `_Literal` node) be delivered to this (gate, port) sink as inlined
/// component data? True for wire-variant fields and for native
/// `Vector`/`Rotator`/`Quat` struct fields (the gate stores an unwired
/// input's value in its data — entity `Set*` gates, Sweep, …). Everything
/// else — `LinearColor` fields, `Split*` inputs, chip IO, unmapped gates —
/// must keep a real `Make*` gate, which the lowering pass materializes on
/// demand.
pub(crate) fn port_accepts_inline_variant(gate_class: &str, port: WirePort) -> bool {
    let Some((struct_name, fields, use_wire_variant)) = data_struct_for_gate(gate_class) else {
        return false;
    };
    let field = port.as_str();
    if !fields.contains(&field) {
        return false;
    }
    if use_wire_variant || schema_field_is_wire_variant(struct_name, field) {
        return true;
    }
    // Split* gates keep materialized Make* inputs — not yet verified that
    // they read an unwired input from data like the Set* gates do.
    if gate_class.contains("_Expr_Split") {
        return false;
    }
    matches!(
        schema_field_type_str(struct_name, field).as_deref(),
        Some("Vector" | "Rotator" | "Quat")
    )
}

/// If the field's schema type is an enum, resolve `lit` to its integer
/// discriminant. Accepts both `Literal::Int` (passthrough) and
/// `Literal::String` (looked up by variant name, with or without the
/// enum-name prefix).
#[cfg(test)]
fn try_resolve_enum(struct_name: &str, field: &str, lit: &Literal) -> Option<u8> {
    let type_name = schema_field_type_str(struct_name, field)?;
    resolve_enum_value(&type_name, lit)
}

/// Resolve a literal against a schema enum type by name — exact match
/// first (`EBRDisplayTextJustification::Left`), then bare suffix (`Left`).
fn resolve_enum_value(type_name: &str, lit: &Literal) -> Option<u8> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let enum_def = schema.get_enum(type_name)?;
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
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Bool)),
            WireVariant::Bool(false)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Int)),
            WireVariant::Int(0)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Float)),
            WireVariant::Number(_)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::String)),
            WireVariant::Str(_)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Vector)),
            WireVariant::Vector(_)
        ));
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
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Entity)),
            WireVariant::Object(None)
        ));
        // Literal initializers convert to the matching variant member.
        assert!(matches!(
            literal_to_wire_variant(&Literal::String("x".into())),
            Some(WireVariant::Str(_))
        ));
        assert!(matches!(
            literal_to_wire_variant(&Literal::Vector {
                x: 1.0,
                y: 2.0,
                z: 3.0
            }),
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
    fn every_gate_data_field_serializes_a_literal() {
        // Exhaustive write audit: one node per derived gate class, with a
        // schema-typed literal in EVERY representable field, emitted through
        // the real brz writer. Catches any field whose inlined literal can't
        // be boxed/serialized (the `min/max` and Vector→0i64 bug class) for
        // every component in the game, present and future.
        use crate::ir::Literal;
        use crate::ir::build::{AddNodeOpts, IdAllocator, ModuleBuilder};

        let schema = brdb::schemas::bricks_components_schema_max();
        let mut builder = ModuleBuilder::new("audit");
        builder.module.scopes.insert(
            crate::ir::ROOT_SCOPE_ID,
            crate::ir::ScopeInfo {
                kind: crate::ir::ScopeKind::ModuleRoot,
                source_range: crate::diagnostic::SourceRange::default(),
                parent: None,
            },
        );
        let mut ids = IdAllocator::default();
        let mut filled = 0usize;
        let mut gates = 0usize;

        for (class, (struct_name, fields)) in derived_gate_data() {
            // Special-cased emit branches with their own property contracts.
            if matches!(
                *class,
                "BrickComponentType_WireGraphPseudo_Var"
                    | "BrickComponentType_WireGraphPseudo_ArrayVar"
                    | "BrickComponentType_Internal_MicrochipInput"
                    | "BrickComponentType_Internal_MicrochipOutput"
                    | "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry"
            ) {
                continue;
            }
            let mut props: std::collections::HashMap<crate::intern::Sym, Literal> =
                std::collections::HashMap::new();
            for field in fields {
                let Some(ty) = schema_field_type_str(struct_name, field) else {
                    continue;
                };
                let lit = if schema.get_enum(&ty).is_some() {
                    Some(Literal::Int(0))
                } else {
                    match ty.as_str() {
                        "str" => Some(Literal::String("x".into())),
                        "bool" => Some(Literal::Bool(true)),
                        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => {
                            Some(Literal::Int(1))
                        }
                        "f32" | "f64" => Some(Literal::Float(1.5)),
                        "WireGraphVariant" | "WireGraphPrimMathVariant" => {
                            Some(Literal::Float(2.5))
                        }
                        "Vector" => Some(Literal::Vector { x: 1.0, y: 2.0, z: 3.0 }),
                        "Rotator" => Some(Literal::Rotator { pitch: 1.0, yaw: 2.0, roll: 3.0 }),
                        "Quat" => Some(Literal::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }),
                        "class" | "object" => Some(Literal::Asset {
                            asset_type: "BRItemBase".into(),
                            asset_name: "Weapon_Pickaxe".into(),
                        }),
                        // arrays, composite structs, bundle_path_ref: not
                        // literal-representable — writer fills defaults.
                        _ => None,
                    }
                };
                if let Some(l) = lit {
                    props.insert(crate::intern::intern(field), l);
                    filled += 1;
                }
            }
            builder.add_gate(
                &mut ids,
                AddNodeOpts {
                    gate_class: class,
                    properties: props,
                    ..Default::default()
                },
            );
            gates += 1;
        }
        assert!(gates > 150, "sweep should cover the whole pair table, got {gates}");
        assert!(filled > 200, "sweep should fill real fields, got {filled}");

        let module = builder.module;
        let placements = crate::layout::layout(&module).placements;
        let brz = emit_brz(
            &module,
            &placements,
            &EmitOptions::default(),
            &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
        );
        assert!(
            brz.is_ok(),
            "every gate's data fields should serialize inlined literals: {:?}",
            brz.err()
        );
    }

    #[test]
    fn unlisted_gates_derive_data_structs_from_pair_table() {
        // Gates without a hand-written entry derive their (struct, full field
        // list) from brdb's game-extracted pair table + the schema, so a new
        // game gate embeds literals without a table edit.
        let entry = data_struct_for_gate("BrickComponentType_Internal_CharacterZoneEvent_Entered");
        let (s, fields, uwv) = entry.expect("zone event should derive from the pair table");
        assert_eq!(s, "BrickComponentData_Internal_CharacterZoneEvent");
        assert!(
            fields.contains(&"bCollisionEnabled_Player"),
            "derived fields should be the full struct: {fields:?}"
        );
        assert!(!uwv, "derived entries rely on per-field variant detection");
    }

    #[test]
    fn literal_params_with_schema_fields_are_mapped() {
        // Guard for the missing-data-mapping bug class (MakeVector,
        // EdgeDetector, ShowStatusMessage, Sleep, ...): a literal arg to a
        // builtin call is inlined into the gate's data properties at lowering,
        // but build_gate_component only writes fields listed in
        // data_struct_for_gate — an unlisted field silently drops the value.
        // For every call param that can carry a literal, if the gate's schema
        // data struct has a matching field, the mapping must list it.
        //
        // Not covered: gates whose data struct name isn't derivable from the
        // class name (checked via their mapping entry when present), and
        // gates with no schema struct at all (wire-only inputs — literals
        // there are a separate lowering concern).
        use crate::ir::Type;
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut findings: Vec<String> = Vec::new();
        for (_, spec) in crate::catalog::calls::calls().iter() {
            for p in spec.params.iter() {
                if !matches!(
                    p.ty,
                    Type::String | Type::Int | Type::Float | Type::Bool | Type::Any
                ) {
                    continue;
                }
                let field = p.port.as_str();
                let entry = data_struct_for_gate(spec.gate_class);
                let (struct_name, listed) = match entry {
                    Some((s, f, _)) => (s.to_string(), Some(f)),
                    None => {
                        // Resolve the gate's data struct via the game-extracted
                        // pair table — many gates share structs (PrimMath,
                        // Float_Float, …) whose names aren't derivable from the
                        // class name. Not in the table → no data component.
                        match brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
                            .iter()
                            .find(|(t, _)| *t == spec.gate_class)
                        {
                            Some((_, s)) => (s.to_string(), None),
                            None => continue,
                        }
                    }
                };
                let covered = listed.is_some_and(|f| f.contains(&field));
                if covered {
                    continue;
                }
                // Allowlist. SetInventoryEntry builds its data in a dedicated
                // emit branch; the Teleport gates' Destination/Source are
                // composite TeleportDestination structs, deliberately
                // unmapped (wire-only — a scalar literal can't fill them).
                if matches!(
                    spec.gate_class,
                    "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry"
                        | "BrickComponentType_WireGraph_Exec_Entity_Teleport"
                        | "BrickComponentType_WireGraph_Exec_Entity_RelativeTeleport"
                ) {
                    continue;
                }
                let has_field = schema
                    .get_struct(&struct_name)
                    .zip(schema.intern.get(field))
                    .is_some_and(|(s, id)| s.get(&id).is_some());
                if has_field {
                    findings.push(format!(
                        "{}({}) class={} field {}",
                        spec.name, p.name, spec.gate_class, field,
                    ));
                }
            }
        }
        findings.sort();
        assert!(
            findings.is_empty(),
            "literal args to these params are silently dropped at emit — \
             add the field to the gate's data_struct_for_gate entry:\n{}",
            findings.join("\n")
        );
    }

    #[test]
    fn show_status_message_data_struct_includes_message() {
        // Regression: the entry existed but with an empty field list, so the
        // inlined message of `ShowStatusMessage(ctrl, "hi")` was dropped at
        // emit — the gate pasted with an empty internal Message and no wire.
        let entry = data_struct_for_gate(crate::ir::gate_class::CONTROLLER_SHOW_STATUS);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Exec_Controller_ShowStatusMessage",
                ["Message"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn field_enum_values_lists_justification() {
        let vals = field_enum_values(
            "BrickComponentType_WireGraph_Exec_Controller_DisplayText",
            "Justification",
        )
        .expect("justify maps to an enum field");
        for expected in ["Left", "Center", "Right"] {
            assert!(
                vals.iter().any(|v| v == expected),
                "missing {expected}: {vals:?}"
            );
        }
        // Names must be bare (no `EnumType::` prefix).
        assert!(vals.iter().all(|v| !v.contains("::")), "prefixed: {vals:?}");
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
        assert_eq!(
            try_resolve_enum(EASING, "Function", &Literal::String("Quad".into())),
            Some(2)
        );
        assert_eq!(
            try_resolve_enum(EASING, "Function", &Literal::String("Cubic".into())),
            Some(3)
        );
        assert_eq!(
            try_resolve_enum(EASING, "Direction", &Literal::String("InOut".into())),
            Some(2)
        );
        assert_eq!(
            try_resolve_enum(EASING, "Direction", &Literal::String("Out".into())),
            Some(1)
        );
        // ints pass through
        assert_eq!(
            try_resolve_enum(EASING, "Function", &Literal::Int(5)),
            Some(5)
        );
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
