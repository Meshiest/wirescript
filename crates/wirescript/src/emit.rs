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

use crate::collections::HashMap;
#[cfg(feature = "brdb-full")]
use std::path::Path;

use std::collections::HashMap as StdMap;

use brdb::{
    AsBrdbValue, BString, BrickType, Collision, Color, IntVector, Position, Vector3f,
    WireConnection, WirePort as BrdbWirePort, World,
    assets::LiteralComponent,
    schema::{
        WireArrayVariant, WireMapKey, WireMapKeyData, WireMapValue, WireMapValueData,
        WireMapVariant, WireVariant,
    },
};

use crate::intern::{Sym, intern_static, resolve, sym};
use crate::ir::port_registry::WirePort;
use crate::layout::wall::WallLayout;
use crate::layout::{BusEnd, BusLayout, LayoutResult, NodeRotation};
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

/// Compiles an inline nested-prefab block (`$``` ... ``` `) to `.brz` bytes to
/// embed, mirroring [`PrefabResolver`]. The argument is the inner source text
/// and the current nesting depth (1 for a block written directly in the root
/// source); `Err` carries a human-readable reason surfaced as an emit error.
#[derive(Clone)]
pub struct NestedCompiler(
    pub std::sync::Arc<dyn Fn(&str, usize) -> Result<Vec<u8>, String> + Send + Sync>,
);

impl NestedCompiler {
    pub fn new(f: impl Fn(&str, usize) -> Result<Vec<u8>, String> + Send + Sync + 'static) -> Self {
        NestedCompiler(std::sync::Arc::new(f))
    }
    fn compile(&self, src: &str, depth: usize) -> Result<Vec<u8>, String> {
        (self.0)(src, depth)
    }
}

impl std::fmt::Debug for NestedCompiler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NestedCompiler(..)")
    }
}

/// Options for a single emit run.
#[derive(Clone, Debug)]
pub struct EmitOptions {
    /// World position of the outer deployment chip brick, in global-grid units.
    pub chip_pos: Placement,
    /// Bundle description written to the .brz metadata.
    pub description: String,
    /// When true, the root microchip is emitted uncollapsed (expanded).
    /// Non-root chips are always open unless annotated `@closed`.
    pub open: bool,
    /// Resolves `$./file.brz` / `$/abs/file.brz` prefab references to bytes.
    /// `None` makes any prefab reference an emit error.
    pub prefab_resolver: Option<PrefabResolver>,
    /// Compiles inline nested-prefab blocks (`$``` ... ``` `) to bytes.
    /// `None` makes any nested-prefab block an emit error.
    pub nested_compiler: Option<NestedCompiler>,
    /// Doc comment rendered under the root plane's title (module-level `///`
    /// block — the doc attached to the file's first declaration, mirroring
    /// how namespace imports derive their module doc).
    pub module_doc: Option<String>,
    /// Module-level `@invisible` — the emitted top-level microchip shell is
    /// hidden, non-colliding, and carries no labels (root name, root plane
    /// header, var tags, I/O gate labels).
    pub invisible: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            chip_pos: Placement { x: 0, y: 0, z: 0 },
            description: String::from("wirescript emit"),
            open: false,
            prefab_resolver: None,
            nested_compiler: None,
            module_doc: None,
            invisible: false,
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
    /// A gutter-bus wire whose endpoint could not be resolved to a brick port.
    ///
    /// Fatal, unlike the module-wire equivalent: the bus SUPPRESSED the module
    /// wire this one replaces, so there is no second path to the consumer. A
    /// dropped bus wire means the value never arrives, and nothing downstream
    /// can tell — the save loads, pastes, and reads zero.
    #[error("bus wire {from} → {to} could not be drawn: {cause}")]
    BusWireUnresolved {
        from: String,
        to: String,
        cause: String,
    },
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

    // node -> owning anon chip, from the chip_id tags (one scan of nodes).
    let assignment: HashMap<NodeId, NodeId> = module
        .nodes
        .iter()
        .filter_map(|(id, n)| n.chip_id.map(|c| (*id, c)))
        .collect();
    // Sorted Vec, NOT a std HashSet: iteration order decides the intern order
    // of the `_anon_{id}` module names (and, before the single-pass wire
    // partition, decided the wire structure itself) — random order made
    // emitted wire counts and Sym numbering nondeterministic run-to-run.
    let mut chip_ids: Vec<NodeId> = assignment.values().copied().collect();
    chip_ids.sort_unstable();
    chip_ids.dedup();

    // Anon chips with an entirely empty functional body (e.g. `@label("...")
    // chip on trigger { }`) tag no descendant nodes at all, so the partition
    // loop below never sees them: they'd stay bare orphan `Chip` nodes with
    // no `module.chips` entry, and emit skips those — silently discarding
    // the `@label`/`@closed`/doc annotations along with them. Give them an
    // empty child module so the (labelled/collapsed) shell still reaches
    // emit. Named chip instances already have a populated module by this
    // point (see `lower/call.rs`), so this only ever catches anon chips.
    let empty_annotated: Vec<NodeId> = module
        .nodes
        .iter()
        .filter(|(id, n)| {
            n.kind == NodeKind::Chip
                && chip_ids.binary_search(id).is_err()
                && !module.chips.contains_key(id)
                && (n.properties.contains_key(&*sym::NAME_LABEL)
                    || n.properties.contains_key(&*sym::CHIP_CLOSED)
                    || n.properties.contains_key(&*sym::DOC_TEXT))
        })
        .map(|(id, _)| *id)
        .collect();
    for id in empty_annotated {
        module.chips.insert(id, Module::new(&format!("_anon_{id}")));
    }

    if chip_ids.is_empty() {
        return;
    }

    // Parent-side Literal nodes we clone into chips (below); cleaned up after.
    let mut cloned_literal_sources: HashSet<NodeId> = HashSet::default();

    // Child module per anon chip; tagged nodes move into their chip's child.
    let mut children: HashMap<NodeId, Module> = chip_ids
        .iter()
        .map(|c| (*c, Module::new(&format!("_anon_{c}"))))
        .collect();
    for (&nid, &cid) in &assignment {
        if let Some(mut node) = module.nodes.remove(&nid) {
            node.chip_id = None;
            children
                .get_mut(&cid)
                .expect("child module for tagged node")
                .nodes
                .insert(nid, node);
        }
    }

    // Map each chip-instance boundary pin (MicrochipInput/Output of a called
    // `chip`'s module) to the anon chip its instance node is tagged to. A wire
    // targeting such a pin belongs INSIDE that anon chip's module (one boundary
    // from the instance). Without this it stays at the root as a wire whose
    // endpoint is nested several grids deep — the game can route a root wire
    // one grid in, but an exec pulse can't cross into an instance grid that
    // sits inside another anon chip, so the called chip silently never fires.
    let mut pin_chip: HashMap<NodeId, NodeId> = HashMap::default();
    for (chip_node, inst) in &module.chips {
        if let Some(&cid) = assignment.get(chip_node) {
            for pin in inst.inputs.iter().chain(inst.outputs.iter()) {
                pin_chip.insert(*pin, cid);
            }
        }
    }
    let chip_of = |id: &NodeId| assignment.get(id).or_else(|| pin_chip.get(id)).copied();

    // Partition wires in ONE pass: internal wires go to their chip's child,
    // cross-boundary wires stay in the parent as remote wires with Layout
    // edges keeping the chips inline in the DAG. (The old per-chip loop
    // re-scanned and rebuilt the full wire list once per chip.) A wire that
    // crosses between two chips gets the same edge set the sequential passes
    // produced: chip->chip, chip->inner-node, and inner-node->chip.
    let layout_edge = |a: NodeId, b: NodeId| Wire {
        source: PortRef {
            node_id: a,
            port: layout_port,
        },
        target: PortRef {
            node_id: b,
            port: layout_port,
        },
    };
    let mut parent_wires: Vec<Wire> = Vec::with_capacity(module.wires.len());
    let mut seen_layout_edges: HashSet<(NodeId, NodeId)> = HashSet::default();
    // Dedupe of Literal nodes cloned into a chip, per (chip, literal).
    let mut literal_clones: HashMap<(NodeId, NodeId), NodeId> = HashMap::default();
    for w in std::mem::take(&mut module.wires) {
        let src_chip = chip_of(&w.source.node_id);
        let tgt_chip = chip_of(&w.target.node_id);
        match (src_chip, tgt_chip) {
            (Some(a), Some(b)) if a == b => {
                children.get_mut(&a).expect("chip module").wires.push(w);
            }
            (None, None) => parent_wires.push(w),
            (Some(a), None) => {
                if seen_layout_edges.insert((a, w.target.node_id)) {
                    parent_wires.push(layout_edge(a, w.target.node_id));
                }
                parent_wires.push(w);
            }
            (None, Some(b)) => {
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
                    let child = children.get_mut(&b).expect("chip module");
                    let clone_id = *literal_clones
                        .entry((b, w.source.node_id))
                        .or_insert_with(|| {
                            let mut cl = module.nodes[&w.source.node_id].clone();
                            let nid = NodeId::fresh();
                            cl.id = nid;
                            cl.chip_id = None;
                            child.nodes.insert(nid, cl);
                            cloned_literal_sources.insert(w.source.node_id);
                            nid
                        });
                    let mut w2 = w;
                    w2.source.node_id = clone_id;
                    child.wires.push(w2);
                    continue;
                }
                if seen_layout_edges.insert((w.source.node_id, b)) {
                    parent_wires.push(layout_edge(w.source.node_id, b));
                }
                parent_wires.push(w);
            }
            (Some(a), Some(b)) => {
                if seen_layout_edges.insert((a, b)) {
                    parent_wires.push(layout_edge(a, b));
                }
                if seen_layout_edges.insert((a, w.target.node_id)) {
                    parent_wires.push(layout_edge(a, w.target.node_id));
                }
                if seen_layout_edges.insert((w.source.node_id, b)) {
                    parent_wires.push(layout_edge(w.source.node_id, b));
                }
                parent_wires.push(w);
            }
        }
    }
    module.wires = parent_wires;
    for (cid, child) in children {
        module.chips.insert(cid, child);
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
    layout: &LayoutResult,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<World, EmitError> {
    let mut world = World::new();
    world.meta.bundle.description = opts.description.clone();

    let wall = crate::layout::wall::assign_wall_slots(
        module,
        layout,
        (opts.chip_pos.x, opts.chip_pos.y, opts.chip_pos.z),
    );

    let (chip_brick_id, _root_entity_id, mut inner_pair) = world.add_microchip(
        opts.chip_pos.into(),
        wall.root.location,
        wall.root.extent,
        !opts.open,
    );
    inner_pair.0.rotation = WALL_ROT;

    // Module-level `@invisible`: hide the shell brick `add_microchip` just
    // pushed and drop all its collision, mirroring the `@invisible` port
    // rerouter treatment in `emit_port_rerouters`.
    if opts.invisible {
        if let Some(chip_brick) = world.bricks.last_mut() {
            chip_brick.visible = false;
            chip_brick.collision = Collision {
                player: false,
                player1: Some(false),
                player2: Some(false),
                player3: Some(false),
                weapon: false,
                interact: false,
                tool: false,
                physics: false,
            };
        }
    }

    // Top-level chip label. The default is the root module's name (entry file
    // stem, or an explicit module_name override); a module-level `@label`
    // overrides it — a constant with baked text, a runtime value with an empty
    // placeholder that Pass 3.5 drives by wire. The chip brick is the one
    // `add_microchip` just pushed onto the main grid.
    if !opts.invisible {
        let dynamic_root = module.root_dynamic_label.is_some();
        let root_label: &str = if dynamic_root {
            // Wire-driven — bake an empty placeholder (the wire supplies the
            // text). `named`-style suppression doesn't apply to the root label.
            ""
        } else {
            module
                .root_label_override
                .as_deref()
                .unwrap_or_else(|| resolve(module.name))
        };
        if dynamic_root || !root_label.is_empty() {
            let label = text_label(
                &mut world,
                root_label,
                LABEL_ROTATION_DEG,
                -0.5,
                LABEL_LINE_HEIGHT,
                0.5,
                0.5,
            );
            if let Some(chip_brick) = world.bricks.last_mut() {
                chip_brick.add_component_box(Box::new(label));
            }
        }
    }

    // Push root inner grid FIRST so it gets the lowest grid ID (persistent
    // index 2). Child grids pushed during emit_module_bricks get 3, 4, etc.
    let root_grid_idx = world.grids.len();
    world.grids.push((inner_pair.0.clone(), Vec::new()));

    let mut ctx = EmitContext {
        node_brick_ids: HashMap::default(),
        class_index: HashMap::default(),
        prefab_resolver: opts.prefab_resolver.clone(),
        nested_compiler: opts.nested_compiler.clone(),
        wire_sources: HashMap::default(),
        var_labels: HashMap::default(),
        invisible: opts.invisible,
        root_shell_brick_id: chip_brick_id,
    };
    emit_module(
        &mut world,
        &mut ctx,
        module,
        layout,
        &mut inner_pair.1,
        &wall,
        template_cache,
    )?;

    if !opts.invisible {
        // The plane header title stays static text: a constant module `@label`
        // override, else the module name. (A runtime module `@label` drives the
        // outer shell label by wire; the inner header keeps the module name.)
        let header = module
            .root_label_override
            .clone()
            .unwrap_or_else(|| resolve(module.name).to_string());
        let root_title = (!header.is_empty()).then_some(header);
        emit_plane_header(
            &mut world,
            &mut inner_pair.1,
            wall.root.extent,
            root_title.as_deref(),
            opts.module_doc.as_deref(),
        );
    }

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

    // Outer rerouters for `@side`-annotated root ports, wired through the
    // chip wall (remote wires — see save.rs add_wire).
    emit_port_rerouters(&mut world, &ctx, module, opts);

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

/// Shared upright rotation for every chip grid entity, PINNED IN-GAME via a
/// quat sampler (a pure −90° about Y). MEASURED mapping of grid-local axes
/// (edge-marker sampler, facing the pane from the chip's bottom-port side):
/// local +X → world up (dataflow runs bottom→top), local +Y → viewer-right,
/// local +Z (board front) → toward the viewer (the chip's bottom-port side).
/// Everything geometric hangs off this: the pane's TOP edge is the local +X
/// edge (headers go there), its horizontal half-span is `extent.y`, and its
/// vertical half-span is `extent.x` (see layout/wall.rs packing).
const WALL_ROT: brdb::Quat4f = brdb::Quat4f {
    x: 0.0,
    y: -std::f32::consts::FRAC_1_SQRT_2,
    z: 0.0,
    w: std::f32::consts::FRAC_1_SQRT_2,
};

/// Name labels on chips, vars, and I/O gates.
const LABEL_LINE_HEIGHT: f32 = 2.4;
/// Smaller tag on Var_Get/Set-style gates naming the variable they touch.
const VAR_TAG_LINE_HEIGHT: f32 = 1.2;
/// On-screen angle every name label and variable tag reads at.
const LABEL_ROTATION_DEG: f32 = -45.0;

/// The `Rotation` to write on a name label riding a brick placed at
/// `rotation`.
///
/// A label's rotation is brick-local — it rides the brick's yaw — so a
/// quarter-turned brick would read its tag a quarter-turn off from every
/// other tag on the plane. Taking the yaw back out lands the text at the
/// same on-screen angle regardless of how the brick under it is turned.
fn label_rotation_deg(rotation: NodeRotation) -> f32 {
    match rotation {
        NodeRotation::Deg0 => LABEL_ROTATION_DEG,
        NodeRotation::Deg90 => LABEL_ROTATION_DEG - 90.0,
        NodeRotation::Deg180 => LABEL_ROTATION_DEG - 180.0,
        NodeRotation::Deg270 => LABEL_ROTATION_DEG - 270.0,
    }
}

/// Grid-unit offset from the chip brick's centre to a flush outer rerouter's
/// centre: chip half-extent (5) + rerouter half-extent (1).
const REROUTER_EDGE_OFFSET: i32 = 6;
/// Spacing between adjacent rerouter centres along a side.
const REROUTER_PITCH: i32 = 2;
/// Along-edge coordinate of the first pin: flush at the top/left corner of the
/// chip's 10-wide edge (chip half-extent 5 − rerouter half-extent 1). Pins run
/// inward from here (top→bottom, left→right) rather than centred on the edge.
const REROUTER_RUN_START: i32 = 4;
/// Rerouter centre sits 1 unit below the chip centre so both bases rest on
/// the same plane (chip half-height 2, rerouter half-height 1).
const REROUTER_Z_OFFSET: i32 = -1;
/// Rerouter name labels use the smaller tag size, not the full gate-label size.
const REROUTER_LABEL_LINE_HEIGHT: f32 = 1.2;

/// Yaw applied to an outer rerouter brick so its wire nub faces away from the
/// chip. Output pins face outward; input pins are flipped 180° so an input and
/// an output on the same side read opposite ways (as requested). The reroute
/// node's default yaw (`Deg0`) points +Y — "screen right" — so the sides step
/// around from there. The floating text label rides the brick's yaw (its own
/// TextDisplay rotation stays 0), giving the in/out 180° text flip for free.
///
/// World<->screen mapping, confirmed in-game: +X = up, +Y = right, and `Deg90`
/// yaws toward −X — so the per-side values below point each output pin outward.
fn rerouter_orientation(side: &str, is_input: bool) -> brdb::Rotation {
    use brdb::Rotation::*;
    // Outward-facing yaw per side (output pins).
    // Deg0=+Y, Deg90=−X, Deg180=−Y, Deg270=+X.
    let outward = match side {
        "right" => Deg0,   // faces +Y (screen right)
        "bottom" => Deg90, // faces −X (screen down)
        "left" => Deg180,  // faces −Y (screen left)
        _ => Deg270,       // top, faces +X (screen up)
    };
    if is_input {
        match outward {
            Deg0 => Deg180,
            Deg90 => Deg270,
            Deg180 => Deg0,
            Deg270 => Deg90,
        }
    } else {
        outward
    }
}

/// TextDisplay rotation (degrees) for a rerouter pin's name label. Calibrated
/// in-game: on each side the input and output labels read opposite ways.
/// left/top edges → input 0°, output 180°; right/bottom edges → input 180°,
/// output 0°.
fn rerouter_label_rotation(side: &str, is_input: bool) -> f32 {
    let left_or_top = matches!(side, "left" | "top");
    if left_or_top == is_input { 0.0 } else { 180.0 }
}

/// TextDisplay horizontal anchor for a rerouter pin's name label. Calibrated
/// in-game: right/bottom edges anchor at 0, left/top at 1, so the name hangs
/// off the pin's outer end on each edge.
fn rerouter_label_anchor_x(side: &str) -> f32 {
    if matches!(side, "right" | "bottom") {
        0.0
    } else {
        1.0
    }
}

/// `@closed` marks a chip's inner grid collapsed; absent = open. Non-root
/// chips default open.
fn chip_is_closed(node: &Node) -> bool {
    matches!(
        node.properties.get(&*sym::CHIP_CLOSED),
        Some(Literal::Bool(true))
    )
}

/// Display name for a chip: the `@label` override wins, else the chip's
/// declared name (anonymous partitions have none).
fn chip_display_name(node: &Node, child_module: &Module) -> Option<String> {
    if let Some(Literal::String(s)) = node.properties.get(&*sym::NAME_LABEL) {
        if !s.is_empty() {
            return Some(s.clone());
        }
    }
    match child_module.scopes.get(&crate::ir::ROOT_SCOPE_ID) {
        Some(crate::ir::ScopeInfo {
            kind: crate::ir::ScopeKind::ChipBody { name },
            ..
        }) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// Floating-label text for a microchip I/O gate, from its `PortLabel`
/// property. User-given names label as themselves. Synthesized plumbing
/// maps to friendly labels: the auto exec ports (`_exec_in`/`_exec_out`)
/// read `exec`, and the anonymous `-> type` return output (`_`) reads
/// `return`. Any other `_`-prefixed name stays unlabeled. `@label("…")`
/// overrides all of the above (covers both pass-1 I/O gate labels and
/// outer-rerouter labels — both call this).
fn microchip_io_label(node: &Node) -> Option<String> {
    if let Some(Literal::String(s)) = node.properties.get(&*sym::NAME_LABEL) {
        if !s.is_empty() {
            return Some(s.clone());
        }
    }
    match node.properties.get(&*sym::PORT_LABEL)? {
        Literal::String(s) if s == "_exec_in" || s == "_exec_out" => Some("exec".to_string()),
        Literal::String(s) if s == "_" => Some("return".to_string()),
        Literal::String(s) if !s.is_empty() && !s.starts_with('_') => Some(s.clone()),
        _ => None,
    }
}

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
    anchor_x: f32,
    anchor_y: f32,
) -> LiteralComponent {
    use brdb::schema::BrdbValue;
    let (font_idx, _) = world
        .global_data
        .external_asset_references
        .insert_full(("BrickFontDescriptor".to_string(), "IosevkaTerm".to_string()));
    let anchor = LiteralComponent::new("Vector2f").with_data([
        ("X", Box::new(anchor_x) as Box<dyn AsBrdbValue>),
        ("Y", Box::new(anchor_y)),
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

/// Header block for an opened plane: `<size="96">{title}</>` then the doc
/// comment on the following lines. A documented but nameless chip renders the
/// doc alone; nothing at all → no header. Text passes through raw (rich-text
/// tags in names/docs are a feature, not escaped).
fn chip_header_text(title: Option<&str>, doc: Option<&str>) -> Option<String> {
    match (title, doc) {
        // Blank line between the big title and the doc so the doc isn't cramped
        // right under the size-96 heading.
        (Some(t), Some(d)) => Some(format!("<size=\"96\">{t}</>\n\n{d}")),
        (Some(t), None) => Some(format!("<size=\"96\">{t}</>")),
        (None, Some(d)) => Some(d.to_string()),
        (None, None) => None,
    }
}

/// Grid units the header brick's centre sits BEYOND the plane's top edge —
/// which under the measured `WALL_ROT` mapping is the local +X edge (the
/// brick is 1×1 → half-size 5, so at +5 its near face rests exactly on the
/// edge). It must never sit inside the plane: gates reach `extent.x - 5`
/// (extent = layout half-span + 5), and the game DROPS overlapping bricks at
/// load — orphaning the dropped brick's components and dangling every wire
/// into it. Pinned during in-game verification.
const HEADER_EDGE_LIFT: i32 = 5;

/// Invisible 1×1 carrier brick floating just above the plane's top-centre —
/// local (extent.x + lift, 0) under the measured `WALL_ROT` mapping (local
/// +X = world up, local Y = world horizontal). Text is centred and flows
/// downward from the top edge (`Anchor = (0.5, 0)`).
fn emit_plane_header(
    world: &mut World,
    bricks: &mut Vec<brdb::Brick>,
    extent: IntVector,
    title: Option<&str>,
    doc: Option<&str>,
) {
    let Some(text) = chip_header_text(title, doc) else {
        return;
    };
    // A 1x1F procedural default brick (10x10x4 cm -> half-extents 5,5,2).
    let mut brick = brdb::Brick {
        asset: brdb::BrickType::from((brdb::assets::bricks::PB_DEFAULT_BRICK, (5, 5, 2))),
        position: brdb::Position {
            x: extent.x + HEADER_EDGE_LIFT,
            y: 0,
            z: 2,
        },
        visible: false,
        ..Default::default()
    };
    brick.add_component_box(Box::new(text_label(
        world,
        &text,
        0.0,
        0.5,
        LABEL_LINE_HEIGHT,
        0.5,
        0.0,
    )));
    bricks.push(brick);
}

/// One invisible 1×1 carrier brick per layout text annotation — the source's
/// own-line `//` comments under the code-shaped layout. Same font, outline and
/// face treatment as a plane header, but anchored on the label's LEFT edge so
/// the text runs rightward from the row's indent, the way the comment reads in
/// the source. The annotation's position is the carrier's min corner, matching
/// the gate-brick convention, so a comment sits on a row of its own.
fn emit_annotations(
    world: &mut World,
    bricks: &mut Vec<brdb::Brick>,
    annotations: &[crate::layout::TextAnnotation],
) {
    for ann in annotations {
        // A 1x1F procedural default brick (10x10x4 cm -> half-extents 5,5,2).
        let mut brick = brdb::Brick {
            asset: brdb::BrickType::from((brdb::assets::bricks::PB_DEFAULT_BRICK, (5, 5, 2))),
            position: brdb::Position {
                x: ann.x + 5,
                y: ann.y + 5,
                z: ann.z,
            },
            visible: false,
            ..Default::default()
        };
        brick.add_component_box(Box::new(text_label(
            world,
            &ann.text,
            0.0,
            0.5,
            LABEL_LINE_HEIGHT,
            0.0,
            0.5,
        )));
        bricks.push(brick);
    }
}

/// One rerouter brick per [`crate::layout::BusNode`], in `BusNodeId` order —
/// the gutter lanes that carry a value to its consumers in place of one long
/// diagonal wire each. Returns each lane node's brick id, indexed by
/// `BusNodeId`, for the wire pass to address.
///
/// Positions are min-corner like every other brick the layout hands over, so
/// the brick centre sits a rerouter half-extent (1) in on both axes.
fn emit_bus(
    bricks: &mut Vec<brdb::Brick>,
    bus: &BusLayout,
    module: &Module,
    wire_target_index: &StdMap<(NodeId, WirePort), NodeId>,
) -> Vec<usize> {
    let mut brick_ids = Vec::with_capacity(bus.nodes.len());
    for node in &bus.nodes {
        let (mut brick, brick_id) = brdb::Brick {
            asset: BrickType::from("B_1x1_Reroute_Node"),
            position: Position {
                x: node.x + 1,
                y: node.y + 1,
                z: node.z,
            },
            rotation: match node.rotation {
                NodeRotation::Deg0 => brdb::Rotation::Deg0,
                NodeRotation::Deg90 => brdb::Rotation::Deg90,
                NodeRotation::Deg180 => brdb::Rotation::Deg180,
                NodeRotation::Deg270 => brdb::Rotation::Deg270,
            },
            color: node
                .color_of
                .map(|id| match module.nodes.get(&id) {
                    Some(n) => color_for_node(n, module, wire_target_index),
                    // The allocator named a node this module doesn't own, so
                    // the lane can't mirror its colour — say so rather than
                    // leaving a silently default-coloured lane to explain.
                    None => {
                        eprintln!(
                            "[bus] colour source {id} is not a node of module {}; using the default",
                            resolve(module.name)
                        );
                        Color::default()
                    }
                })
                .unwrap_or_default(),
            ..Default::default()
        }
        .with_id_split();
        // Saved worlds use the component's instance name; the rerouter's
        // data struct is empty.
        brick.add_component_box(Box::new(LiteralComponent::new(gc::REROUTER)));
        bricks.push(brick);
        brick_ids.push(brick_id);
    }
    brick_ids
}

/// Place one outer-grid rerouter per `@side`-annotated root port, flush
/// against the chip brick's edge, pre-wired to the port's inner
/// MicrochipInput/Output gate. The brdb writer serialises the cross-grid
/// wires as remote wire sources automatically.
fn emit_port_rerouters(world: &mut World, ctx: &EmitContext, module: &Module, opts: &EmitOptions) {
    let side_sym = *sym::REROUTE_SIDE;

    // side → [(source offset, node id, is_input)], later sorted per side so
    // ins and outs interleave in declaration order (the spec's ordering rule).
    let mut by_side: StdMap<&'static str, Vec<(usize, NodeId, bool)>> = StdMap::new();
    for (ids, is_input) in [(&module.inputs, true), (&module.outputs, false)] {
        for id in ids.iter() {
            let Some(node) = module.nodes.get(id) else {
                continue;
            };
            let Some(Literal::String(side)) = node.properties.get(&side_sym) else {
                continue;
            };
            let side: &'static str = match side.as_str() {
                "left" => "left",
                "right" => "right",
                "top" => "top",
                _ => "bottom",
            };
            by_side
                .entry(side)
                .or_default()
                .push((node.source_range.start.offset, *id, is_input));
        }
    }

    // Fixed side order for deterministic brick/wire output.
    for side in ["left", "right", "top", "bottom"] {
        let Some(mut ports) = by_side.remove(side) else {
            continue;
        };
        ports.sort_by_key(|(off, id, _)| (*off, id.0));
        for (i, (_, node_id, is_input)) in ports.into_iter().enumerate() {
            let Some(&io_brick_id) = ctx.node_brick_ids.get(&node_id) else {
                continue;
            };
            let node = &module.nodes[&node_id];

            // Pins run from the top/left corner inward at REROUTER_PITCH spacing,
            // first-declared first: `run` steps from REROUTER_RUN_START toward
            // the far end.
            let run = REROUTER_RUN_START - REROUTER_PITCH * i as i32;
            // World<->screen pinned in-game: +X = up, +Y = right. Edges: left =
            // −Y, right = +Y, top = +X, bottom = −X. Left/right pins run down
            // the X axis from the top (+X); top/bottom pins run along Y from the
            // left (−Y), so their `run` is negated.
            let (dx, dy) = match side {
                "left" => (run, -REROUTER_EDGE_OFFSET),
                "right" => (run, REROUTER_EDGE_OFFSET),
                "top" => (REROUTER_EDGE_OFFSET, -run),
                _ => (-REROUTER_EDGE_OFFSET, -run), // bottom
            };
            let position = Position {
                x: opts.chip_pos.x + dx,
                y: opts.chip_pos.y + dy,
                z: opts.chip_pos.z + REROUTER_Z_OFFSET,
            };

            let (mut brick, rer_brick_id) = brdb::Brick {
                asset: BrickType::from("B_1x1_Reroute_Node"),
                position,
                color: io_node_color(node), // matches the inner chip-I/O gate colour
                rotation: rerouter_orientation(side, is_input),
                ..Default::default()
            }
            .with_id_split();
            // Saved worlds use the component's instance name; the rerouter's
            // data struct is empty.
            brick.add_component_box(Box::new(LiteralComponent::new(gc::REROUTER)));

            // `@invisible` ports: hide the rerouter, drop all collision so it
            // doesn't block players/weapons/tools, and skip its name label.
            let invisible = matches!(
                node.properties.get(&*sym::REROUTE_INVISIBLE),
                Some(Literal::Bool(true))
            );
            if invisible {
                brick.visible = false;
                brick.collision = Collision {
                    player: false,
                    player1: Some(false),
                    player2: Some(false),
                    player3: Some(false),
                    weapon: false,
                    interact: false,
                    tool: false,
                    physics: false,
                };
            }

            if !invisible {
                if let Some(name) = microchip_io_label(node) {
                    // Rotation + anchor are calibrated per side/direction (see
                    // rerouter_label_rotation / rerouter_label_anchor_x).
                    let label = text_label(
                        world,
                        &name,
                        rerouter_label_rotation(side, is_input),
                        0.5,
                        REROUTER_LABEL_LINE_HEIGHT,
                        rerouter_label_anchor_x(side),
                        0.5,
                    );
                    brick.add_component_box(Box::new(label));
                }
            }
            world.bricks.push(brick);

            // in:  rerouter.RER_Output → MicrochipInput.RER_Input
            // out: MicrochipOutput.RER_Output → rerouter.RER_Input
            // (mirrors the chip-port remap in build_port_index)
            let rer_port = |port: &'static str| BrdbWirePort {
                brick_id: rer_brick_id,
                component_type: BString::Static(gc::REROUTER),
                port_name: BString::Static(port),
            };
            let io_class = if is_input {
                gc::MICROCHIP_INPUT
            } else {
                gc::MICROCHIP_OUTPUT
            };
            let io_port = |port: &'static str| BrdbWirePort {
                brick_id: io_brick_id,
                component_type: BString::Static(io_class),
                port_name: BString::Static(port),
            };
            let conn = if is_input {
                WireConnection {
                    source: rer_port("RER_Output"),
                    target: io_port("RER_Input"),
                }
            } else {
                WireConnection {
                    source: io_port("RER_Output"),
                    target: rer_port("RER_Input"),
                }
            };
            world.add_wire(conn);
        }
    }
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
    /// Compiler for inline nested-prefab blocks, from `EmitOptions`.
    nested_compiler: Option<NestedCompiler>,
    /// (target node, target port) → source node, accumulated across all
    /// modules so Var_Get/Set gates can trace `VarRef` wires that cross
    /// module boundaries (scope captures, anon-chip partitions).
    wire_sources: HashMap<(NodeId, WirePort), NodeId>,
    /// Pseudo_Var/ArrayVar node → its labelable source name. Vars are
    /// always emitted before the gates that reference them.
    var_labels: HashMap<NodeId, String>,
    /// Module-level `@invisible`, from `EmitOptions` — suppresses var-tag
    /// and I/O-gate label emission in `emit_module`.
    invisible: bool,
    /// The root microchip shell brick's id. Pass 3.5 wires a module-level
    /// dynamic `@label` (`Module.root_dynamic_label`) into this brick's label
    /// `Text` port.
    root_shell_brick_id: usize,
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
fn resolve_var_label(ctx: &EmitContext, mut src: NodeId) -> Option<&String> {
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

fn emit_module(
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
        // EMIT_COMP_NS.fetch_add(_ct.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
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
                microchip_io_label(node).map(|s| (s, LABEL_LINE_HEIGHT))
            }
            "BrickComponentType_WireGraphPseudo_Var"
            | "BrickComponentType_WireGraphPseudo_ArrayVar" => {
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
                    .and_then(|src| resolve_var_label(ctx, *src))
                    .map(|s| (s.clone(), VAR_TAG_LINE_HEIGHT))
            }
            _ => None,
        };
        if let Some((text, line_height)) = label_spec {
            if !ctx.invisible {
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
        // EMIT_BRICK_NS.fetch_add(_bt.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
        // EMIT_BRICK_COUNT.fetch_add(1, AtomicOrd::Relaxed);
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
        // partitions) stay unlabeled.
        if let Some(name) = chip_display_name(node, child_module) {
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

        // EMIT_CHIP_FULL_NS.fetch_add(_ft.elapsed().as_nanos() as u64, AtomicOrd::Relaxed);
    }

    // ── Pass 3: emit this module's wires ──
    let layout_port_id = WirePort::Layout;
    let port_index = build_port_index(module, &ctx.node_brick_ids);
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
        match wire_to_connection_indexed(w, &ctx.node_brick_ids, &ctx.class_index, &port_index) {
            Ok(conn) => world.add_wire(conn),
            Err(e) => {
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
                eprintln!(
                    "[wire] dropped: {} → {} (port {}→{}): {e:?} (src: {}, dst: {})",
                    w.source.node_id,
                    w.target.node_id,
                    w.source.port.as_str(),
                    w.target.port.as_str(),
                    seen(&w.source.node_id),
                    seen(&w.target.node_id),
                );
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
                Err(e) => {
                    eprintln!("[label-wire] dropped source for {}: {e:?}", src.node_id);
                    continue;
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
                    Err(e) => eprintln!("[label-wire] dropped root label source: {e:?}"),
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

/// Pre-build index: (chip_node_id, port_name) → (brick_id, component_class, remapped_port)
fn build_port_index(
    module: &Module,
    node_brick_ids: &HashMap<NodeId, usize>,
) -> HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)> {
    let port_label_sym = *sym::PORT_LABEL;
    let mut idx = HashMap::default();
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
struct NativeStruct {
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
fn build_adv_inventory_component(
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
                        BrdbSchemaStructProperty::Type(t) => match schema.intern.lookup_ref(*t) {
                            Some("WireGraphVariant") => FieldKind::WireVariant,
                            Some("WireGraphPrimMathVariant") => FieldKind::PrimMathVariant,
                            Some("class") | Some("object") => FieldKind::AssetRef,
                            Some("bundle_path_ref") => FieldKind::BundlePathRef,
                            Some("str") => FieldKind::Str,
                            Some(n) if schema.get_enum(n).is_some() => FieldKind::Enum(n),
                            _ => FieldKind::Native,
                        },
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
    debug_assert!(
        !matches!(ty, Some(Type::Param(_))),
        "Type::Param reached emit — monomorphization must substitute it first"
    );
    match ty {
        Some(Type::Bool) => WireVariant::Bool(false),
        Some(Type::Int) => WireVariant::Int(0),
        Some(Type::Controller | Type::Character | Type::Entity) => {
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

/// The `WireMapVariant` (empty map) an empty `MapVar` brick serializes, chosen
/// from the declared `Map<K, V>` type on its `MapVarRef` output port. Keys are
/// int/string/object; values cover the wire-storable scalars. Defaults to
/// `int -> float` for an unknown/degenerate type.
fn map_variant_from_type(ty: &crate::ir::Type) -> WireMapVariant {
    use crate::ir::Type;
    let inner = match ty {
        Type::Ref(inner) => inner.as_ref(),
        other => other,
    };
    let (k, v) = match inner {
        Type::Map(k, v) => (k.as_ref(), v.as_ref()),
        _ => return WireMapVariant { key: WireMapKey::Int64, value: WireMapValue::Number, entries: vec![] },
    };
    let key = match k {
        Type::Int | Type::Bool => WireMapKey::Int64,
        Type::String => WireMapKey::Str,
        // Entity-family references are weak-object keys.
        _ => WireMapKey::Object,
    };
    let value = match v {
        Type::Int => WireMapValue::Int64,
        Type::Float => WireMapValue::Number,
        Type::Bool => WireMapValue::Bool,
        Type::String => WireMapValue::Str,
        Type::Vector => WireMapValue::Vector,
        Type::Rotator => WireMapValue::Rotator,
        Type::Quat => WireMapValue::Quat,
        Type::Color => WireMapValue::LinearColor,
        Type::Entity | Type::Character | Type::Controller => {
            WireMapValue::Object
        }
        _ => WireMapValue::Number,
    };
    WireMapVariant { key, value, entries: vec![] }
}

/// Build a populated `WireMapVariant` from a map's constant entries. `kinds`
/// gives the key/value member kinds (from [`map_variant_from_type`] on the
/// declared `Map<K, V>` port type); each literal is read in that kind.
fn wire_map_variant_from_literals(
    kinds: WireMapVariant,
    entries: &[(Literal, Literal)],
) -> WireMapVariant {
    // Numeric key/value arms read across literal kinds (like arrays'
    // `as_i64`/`as_f64`) so a raw literal that ever reaches emit still bakes
    // correctly instead of hitting the zero fallback — defense in depth
    // behind the fold-time coercion in `bake_map_init`/`coerce_literal_to_type`.
    let key_of = |l: &Literal| match (kinds.key, l) {
        (WireMapKey::Int64, Literal::Int(n)) => WireMapKeyData::Int64(*n),
        (WireMapKey::Int64, Literal::Float(f)) => WireMapKeyData::Int64(*f as i64),
        (WireMapKey::Int64, Literal::Bool(b)) => WireMapKeyData::Int64(*b as i64),
        (WireMapKey::Str, Literal::String(s)) => WireMapKeyData::Str(s.clone()),
        (WireMapKey::Object, _) => WireMapKeyData::Object(None),
        (WireMapKey::Int64, _) => WireMapKeyData::Int64(0),
        (WireMapKey::Str, _) => WireMapKeyData::Str(String::new()),
    };
    let val_of = |l: &Literal| match (kinds.value, l) {
        (WireMapValue::Number, Literal::Float(f)) => WireMapValueData::Number(*f),
        (WireMapValue::Number, Literal::Int(n)) => WireMapValueData::Number(*n as f64),
        (WireMapValue::Number, Literal::Bool(b)) => WireMapValueData::Number(*b as i64 as f64),
        (WireMapValue::Int64, Literal::Int(n)) => WireMapValueData::Int64(*n),
        (WireMapValue::Int64, Literal::Float(f)) => WireMapValueData::Int64(*f as i64),
        (WireMapValue::Int64, Literal::Bool(b)) => WireMapValueData::Int64(*b as i64),
        (WireMapValue::Bool, Literal::Bool(b)) => WireMapValueData::Bool(*b),
        (WireMapValue::Bool, Literal::Int(n)) => WireMapValueData::Bool(*n != 0),
        (WireMapValue::Str, Literal::String(s)) => WireMapValueData::Str(s.clone()),
        (WireMapValue::Vector, Literal::Vector { x, y, z }) => WireMapValueData::Vector(Vector3f {
            x: *x as f32,
            y: *y as f32,
            z: *z as f32,
        }),
        (WireMapValue::Rotator, Literal::Rotator { pitch, yaw, roll }) => {
            WireMapValueData::Rotator(*pitch, *yaw, *roll)
        }
        (WireMapValue::Quat, Literal::Quat { x, y, z, w }) => {
            WireMapValueData::Quat(*x, *y, *z, *w)
        }
        (WireMapValue::LinearColor, Literal::LinearColor { r, g, b, a }) => {
            WireMapValueData::LinearColor(*r as f32, *g as f32, *b as f32, *a as f32)
        }
        (WireMapValue::Object, _) => WireMapValueData::Object(None),
        (WireMapValue::Number, _) => WireMapValueData::Number(0.0),
        (WireMapValue::Int64, _) => WireMapValueData::Int64(0),
        (WireMapValue::Bool, _) => WireMapValueData::Bool(false),
        (WireMapValue::Str, _) => WireMapValueData::Str(String::new()),
        (WireMapValue::Vector, _) => WireMapValueData::Vector(Vector3f {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        (WireMapValue::Rotator, _) => WireMapValueData::Rotator(0.0, 0.0, 0.0),
        (WireMapValue::Quat, _) => WireMapValueData::Quat(0.0, 0.0, 0.0, 1.0),
        (WireMapValue::LinearColor, _) => WireMapValueData::LinearColor(0.0, 0.0, 0.0, 1.0),
    };
    WireMapVariant {
        key: kinds.key,
        value: kinds.value,
        entries: entries.iter().map(|(k, v)| (key_of(k), val_of(v))).collect(),
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
        Some(Type::Controller | Type::Character | Type::Entity) => {
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
        Some(Type::Controller | Type::Character | Type::Entity) => {
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
        Literal::Vector { x, y, z } => Box::new(VectorValue {
            x: *x,
            y: *y,
            z: *z,
        }),
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
        Literal::Array(_)
        | Literal::Map(_)
        | Literal::Asset { .. }
        | Literal::PrefabRef { .. }
        | Literal::NestedPrefab { .. } => None,
    }
}

/// Emit `.brz` bundle bytes — zstd-packed, portable, good for bundle
/// transfer and in-memory preview. `BR.World.LoadAdditive` doesn't accept
/// these directly; use [`emit_brdb`] for that.
pub fn emit_brz(
    module: &Module,
    layout: &LayoutResult,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<Vec<u8>, EmitError> {
    let world = build_world(module, layout, opts, template_cache)?;
    Ok(world.to_brz_vec()?)
}

/// Emit a `.brdb` SQLite database to `path`. This is the format
/// `BR.World.LoadAdditive <name>` reads from `Saved/Worlds/<name>.brdb`.
#[cfg(feature = "brdb-full")]
pub fn emit_brdb(
    module: &Module,
    layout: &LayoutResult,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
    path: impl AsRef<Path>,
) -> Result<(), EmitError> {
    let world = build_world(module, layout, opts, template_cache)?;
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

/// Resolve one wire end — an IR node plus the port on it — to the emitted
/// brick port, applying the chip-port remap in `port_index` when the node is
/// a chip and the port one of its declared pins.
fn resolve_wire_end(
    node_id: NodeId,
    port_idx: WirePort,
    node_brick_ids: &HashMap<NodeId, usize>,
    class_index: &HashMap<NodeId, &'static str>,
    port_index: &HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)>,
) -> Result<BrdbWirePort, EmitError> {
    let port_str: &'static str = port_idx.as_str();
    if let Some(&(brick_id, cls, remapped)) = port_index.get(&(node_id, port_str)) {
        return Ok(BrdbWirePort {
            brick_id,
            component_type: BString::Static(cls),
            port_name: BString::Static(remapped),
        });
    }
    let brick_id = *node_brick_ids
        .get(&node_id)
        .ok_or_else(|| EmitError::UnknownWireNode(node_id.to_string()))?;
    let cls = *class_index
        .get(&node_id)
        .ok_or_else(|| EmitError::UnknownWireNode(node_id.to_string()))?;
    Ok(BrdbWirePort {
        brick_id,
        component_type: BString::Static(cls),
        port_name: BString::Static(port_str),
    })
}

fn wire_to_connection_indexed(
    w: &Wire,
    node_brick_ids: &HashMap<NodeId, usize>,
    class_index: &HashMap<NodeId, &'static str>,
    port_index: &HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)>,
) -> Result<WireConnection, EmitError> {
    Ok(WireConnection {
        source: resolve_wire_end(
            w.source.node_id,
            w.source.port,
            node_brick_ids,
            class_index,
            port_index,
        )?,
        target: resolve_wire_end(
            w.target.node_id,
            w.target.port,
            node_brick_ids,
            class_index,
            port_index,
        )?,
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
    if matches!(node.kind, NodeKind::Event) {
        return C_YELLOW;
    }
    // Microchip I/O gates colour by their port's value type so the type reads
    // at a glance; exec (trigger) ports keep the neutral yellow.
    if matches!(node.kind, NodeKind::Input | NodeKind::Output) {
        return io_node_color(node);
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

/// Colour for a microchip I/O gate (and its outer rerouter pin), taken from
/// the port's declared value type. Both the `RER_Input` and `RER_Output` ports
/// carry that type; exec (trigger) ports keep the neutral yellow.
fn io_node_color(node: &Node) -> Color {
    let ty = node
        .ports
        .outputs
        .iter()
        .chain(node.ports.inputs.iter())
        .map(|p| &p.ty)
        .next();
    match ty {
        Some(Type::Exec) | None => C_YELLOW,
        Some(t) => color_for_type(t),
    }
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
        // Brick, Record, Tuple, Union, Any, Never, Exec) falls
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
    // Delegate to the catalog's schema-backed enum helpers, which drop the
    // trailing `<Enum>_MAX` / bare `MAX` sentinel (not a selectable member).
    let members = crate::catalog::enum_member_names(field_enum_type(gate_class, field)?);
    if members.is_empty() { None } else { Some(members) }
}

/// The schema enum type name of a gate's data field, if it is an enum (e.g.
/// `ColorBlend.BlendSpace` → `EBRColorSpace`). Used to name the enum in
/// completions/hover for a config param that is surfaced as a plain int.
pub fn field_enum_type(gate_class: &str, field: &str) -> Option<&'static str> {
    crate::catalog::config_field_enum_type(gate_class, field)
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

    const DISPLAY_TEXT: &str = "BrickComponentData_WireGraph_Exec_PlayerState_DisplayText";

    fn empty_ctx() -> EmitContext {
        EmitContext {
            node_brick_ids: HashMap::default(),
            class_index: HashMap::default(),
            prefab_resolver: None,
            nested_compiler: None,
            wire_sources: HashMap::default(),
            var_labels: HashMap::default(),
            invisible: false,
            root_shell_brick_id: 0,
        }
    }

    /// A gate reading a var through a rerouter must still resolve the var's
    /// name for its tag, and a rerouter with nothing upstream (a bus lane
    /// node, which has no IR node and so no `wire_sources` entry) must end
    /// the walk rather than spin.
    #[test]
    fn var_label_walk_follows_rerouter_hops() {
        let var = NodeId::fresh();
        let hop = NodeId::fresh();
        let second_hop = NodeId::fresh();
        let mut ctx = empty_ctx();
        ctx.var_labels.insert(var, "count".to_string());
        ctx.class_index.insert(hop, gc::REROUTER);
        ctx.class_index.insert(second_hop, gc::REROUTER);
        ctx.wire_sources.insert((hop, WirePort::RerInput), var);
        ctx.wire_sources
            .insert((second_hop, WirePort::RerInput), hop);

        assert_eq!(
            resolve_var_label(&ctx, second_hop).map(String::as_str),
            Some("count")
        );

        let dangling = NodeId::fresh();
        ctx.class_index.insert(dangling, gc::REROUTER);
        assert_eq!(resolve_var_label(&ctx, dangling), None);
    }

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
            let mut props: crate::collections::HashMap<crate::intern::Sym, Literal> =
                std::collections::HashMap::default();
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
                        "Vector" => Some(Literal::Vector {
                            x: 1.0,
                            y: 2.0,
                            z: 3.0,
                        }),
                        "Rotator" => Some(Literal::Rotator {
                            pitch: 1.0,
                            yaw: 2.0,
                            roll: 3.0,
                        }),
                        "Quat" => Some(Literal::Quat {
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                            w: 1.0,
                        }),
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
        assert!(
            gates > 150,
            "sweep should cover the whole pair table, got {gates}"
        );
        assert!(filled > 200, "sweep should fill real fields, got {filled}");

        let module = builder.module;
        let lr = crate::layout::layout(&module);
        let brz = emit_brz(
            &module,
            &lr,
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
        let entry = data_struct_for_gate(crate::ir::gate_class::PLAYERSTATE_SHOW_STATUS);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Exec_PlayerState_ShowStatusMessage",
                ["Message"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn field_enum_values_lists_justification() {
        let vals = field_enum_values(
            "BrickComponentType_WireGraph_Exec_PlayerState_DisplayText",
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
    #[test]
    fn microchip_io_labels_map_synthesized_names() {
        fn node_with_label(label: &str) -> Node {
            let mut props = HashMap::default();
            props.insert(*sym::PORT_LABEL, Literal::String(label.into()));
            Node {
                id: NodeId::fresh(),
                kind: crate::ir::NodeKind::Input,
                gate_class: "BrickComponentType_Internal_MicrochipInput",
                properties: std::sync::Arc::new(props),
                ports: std::sync::Arc::new(crate::ir::GateIO::default()),
                source_range: crate::diagnostic::SourceRange::default(),
                chip_id: None,
                chain_id: None,
                scope_id: crate::ir::ROOT_SCOPE_ID,
                note: None,
            }
        }
        // Synthesized exec plumbing reads "exec"; the anonymous return "_"
        // reads "return"; user names pass through; other underscore-prefixed
        // plumbing stays unlabeled.
        let get = |l: &str| microchip_io_label(&node_with_label(l));
        assert_eq!(get("_exec_in").as_deref(), Some("exec"));
        assert_eq!(get("_exec_out").as_deref(), Some("exec"));
        assert_eq!(get("_").as_deref(), Some("return"));
        assert_eq!(get("speed").as_deref(), Some("speed"));
        assert_eq!(get("_hidden"), None);
        assert_eq!(get(""), None);
    }

    #[test]
    fn chip_is_closed_reads_the_closed_prop() {
        let mut node = Node {
            id: NodeId::fresh(),
            kind: crate::ir::NodeKind::Chip,
            gate_class: gc::MICROCHIP,
            properties: std::sync::Arc::new(HashMap::default()),
            ports: std::sync::Arc::new(crate::ir::GateIO::default()),
            source_range: crate::diagnostic::SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        };
        assert!(!chip_is_closed(&node), "default is open");
        std::sync::Arc::make_mut(&mut node.properties)
            .insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        assert!(chip_is_closed(&node));
    }
}
