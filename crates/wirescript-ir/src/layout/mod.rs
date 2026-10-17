//! Gate layout pass.
//!
//! Flat topological placement: every node in the module lands in a
//! single DAG layout ordered by longest-path depth. Bricks are sized
//! per-node from the gate inventory so variable-width gates
//! (FormatText, DisplayText) don't overlap neighbours.
//!
//! Pin rules applied on top of the topo layout:
//! - `NodeKind::Input` -> leftmost column.
//! - `NodeKind::Output` -> rightmost column.
//! - Storage nodes (Var, ArrayVar, BufferTicks) -> bottom row.
//!
//! The region tree (`region.rs`) is built only by the layout tests.

pub mod bus;
pub mod code;
mod dag;
mod region;
pub mod wall;

pub use bus::{BusEnd, BusLayout, BusNode, BusNodeId, BusRole, BusWire};

/// Layout internals for the layout tests in `wirescript`, which lower real
/// source and so cannot live in this crate. Not a stable API.
#[cfg(feature = "test-internals")]
#[doc(hidden)]
pub mod internals {
    pub use super::code::test_internals::*;
    pub use super::region::{Region, build_region_tree, region_node_count, region_scope_count};
    pub use super::*;
}

use crate::collections::HashMap;

use crate::emit::Placement;
use crate::ir::{Module, Node, NodeId, NodeKind, ROOT_SCOPE_ID, Type};

use self::dag::{LocalPlacement, RegionLayout, layout_leaf};
use self::region::Region;

/// Default half-size used for gates not found in the inventory.
/// Standard 1×1 gate bricks have half-size 5 (10×10 full).
const DEFAULT_HALF_SIZE: i32 = 5;

/// Fixed Z plane for inner-chip bricks.
pub const Z_PLANE: i32 = 2;

/// True for pseudo-nodes that represent persistent STORAGE (var, buffer,
/// array). Pseudo_Literal is NOT in this bucket — literals are sources
/// that should align with their consumer, not get pinned to the bottom.
fn is_pseudo_storage(node: &Node) -> bool {
    matches!(
        node.gate_class,
        "BrickComponentType_Var"
            | "BrickComponentType_BufferTicks"
            | "BrickComponentType_ArrayVar"
    )
}

/// Look up a node's brick half-size from the gate inventory (a procedural
/// gate's chosen variant), falling back to the standard 1×1 half-size.
pub fn brick_half_size(node: &Node) -> (i32, i32) {
    crate::catalog::node_half_size(node)
        .map(|h| (h.x, h.y))
        .unwrap_or((DEFAULT_HALF_SIZE, DEFAULT_HALF_SIZE))
}

/// A node's brick half-height. The cube layout is the only caller: it is the
/// only engine that stacks bricks along `z`, so it is the only one that can
/// collide along it.
fn brick_half_height(node: &Node) -> i32 {
    crate::catalog::node_half_size(node)
        .map(|h| h.z)
        .unwrap_or(DEFAULT_HALF_SIZE)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntVec3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// The bounding box of a set of boxes, grown one box at a time.
#[derive(Clone, Copy, Debug)]
pub struct Extent {
    pub min: IntVec3,
    pub max: IntVec3,
}

impl Extent {
    /// Covers nothing; the first box sets it.
    pub const EMPTY: Extent = Extent {
        min: IntVec3 { x: i32::MAX, y: i32::MAX, z: i32::MAX },
        max: IntVec3 { x: i32::MIN, y: i32::MIN, z: i32::MIN },
    };

    /// Covers the one point `(x, y, z)`.
    pub fn point(x: i32, y: i32, z: i32) -> Self {
        Extent { min: IntVec3 { x, y, z }, max: IntVec3 { x, y, z } }
    }

    /// Grow to cover the box from `(x, y, z)` that is `w` by `h` by `d`.
    pub fn add(&mut self, (x, y, z): (i32, i32, i32), (w, h, d): (i32, i32, i32)) {
        self.min = IntVec3 { x: self.min.x.min(x), y: self.min.y.min(y), z: self.min.z.min(z) };
        self.max = IntVec3 { x: self.max.x.max(x + w), y: self.max.y.max(y + h), z: self.max.z.max(z + d) };
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    /// Moved by `dx`, `dy` in the plane.
    pub fn shifted(self, dx: i32, dy: i32) -> Self {
        Extent {
            min: IntVec3 { x: self.min.x + dx, y: self.min.y + dy, z: self.min.z },
            max: IntVec3 { x: self.max.x + dx, y: self.max.y + dy, z: self.max.z },
        }
    }
}

/// A free-floating text label the layout wants on the plane, in the same
/// coordinate space as the module's [`Placement`]s (`x`/`y` are the
/// carrier brick's min corner). Only [`LayoutMode::Code`] produces these —
/// one per own-line source comment.
#[derive(Clone, Debug)]
pub struct TextAnnotation {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub text: String,
}

/// Turns applied to a placed brick, and the axis it faces.
///
/// A rerouter faces what it FEEDS, so the turn is chosen from the direction
/// the value travels next: `Deg0` faces `+Y` (screen right), `Deg90` faces
/// down, `Deg180` faces `−Y` (screen left), `Deg270` faces up.
///
/// FOOTPRINT: only the QUARTER turns swap a brick's x and y half-sizes.
/// `Deg90` and `Deg270` swap; `Deg0` and `Deg180` do not — a half turn lands
/// the brick the way round it started. Every site that measures or reserves a
/// cell must spell out all four arms rather than leaning on a catch-all,
/// because which side of that split a facing falls on is exactly the mistake
/// the overlap sweeps exist to catch.
///
/// `brdb::Brick::local_bounds()` does NOT account for rotation at all. Layout
/// owns this decision precisely so it can reserve the swapped cell; emit must
/// apply the identical swap when centering the brick. Read the footprint
/// through [`Placement`]-space helpers rather than the raw catalog half-size
/// whenever a node may be rotated.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum NodeRotation {
    /// Faces `+Y` — screen right. Default gate orientation, and the gutter
    /// bus's taps, whose runs go rightward into the body.
    #[default]
    Deg0,
    /// Faces down: a line's exec spine, and the chain of a gutter lane whose
    /// taps sit below its source. Footprint swapped.
    Deg90,
    /// Faces `−Y` — screen left. The mini-bus corners, whose runs go leftward
    /// into the statement gate at their line's indent column.
    Deg180,
    /// Faces up: the chain of a gutter lane whose taps ALL sit above its
    /// source, so the value really does travel upward. Footprint swapped, the
    /// same as `Deg90`.
    Deg270,
}

impl NodeRotation {
    /// The `(x, y)` half-extents of a brick turned this way, from its
    /// unturned ones. Only the QUARTER turns swap them: a half turn lands the
    /// brick the way round it started, so Deg180 measures exactly like Deg0
    /// and Deg270 exactly like Deg90.
    pub fn footprint(self, (x, y): (i32, i32)) -> (i32, i32) {
        match self {
            NodeRotation::Deg0 | NodeRotation::Deg180 => (x, y),
            NodeRotation::Deg90 | NodeRotation::Deg270 => (y, x),
        }
    }
}

impl From<NodeRotation> for brdb::Rotation {
    fn from(r: NodeRotation) -> Self {
        match r {
            NodeRotation::Deg0 => brdb::Rotation::Deg0,
            NodeRotation::Deg90 => brdb::Rotation::Deg90,
            NodeRotation::Deg180 => brdb::Rotation::Deg180,
            NodeRotation::Deg270 => brdb::Rotation::Deg270,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    /// Placements for every node in the root module.
    pub placements: HashMap<NodeId, Placement>,
    /// Per-chip sub-layouts keyed by chip node id.
    pub chip_layouts: HashMap<NodeId, LayoutResult>,
    /// Text labels with no node of their own. Empty outside
    /// [`LayoutMode::Code`].
    pub annotations: Vec<TextAnnotation>,
    /// Per-node quarter-turns. Only [`LayoutMode::Code`] populates this;
    /// a node absent from the map is [`NodeRotation::Deg0`]. Every entry
    /// here has already had its swapped footprint reserved by the layout,
    /// so emit must honour it exactly.
    pub rotations: HashMap<NodeId, NodeRotation>,
    /// Gutter rerouter lanes carrying values to their consumers, with the
    /// module wires they replace. Empty outside [`LayoutMode::Code`].
    pub bus: BusLayout,
    pub bounds_min: IntVec3,
    pub bounds_max: IntVec3,
}

impl LayoutResult {
    /// A module with nothing of its own to place. Its chips may still have
    /// content, so their layouts are passed in; everything else is the zero,
    /// `rotations` and `bus` included, since only Code mode ever fills those.
    pub(super) fn empty(chip_layouts: HashMap<NodeId, LayoutResult>) -> Self {
        Self {
            placements: HashMap::default(),
            chip_layouts,
            annotations: Vec::new(),
            rotations: HashMap::default(),
            bus: BusLayout::default(),
            bounds_min: IntVec3::default(),
            bounds_max: IntVec3::default(),
        }
    }
}

pub use crate::ir::LayoutMode;

#[derive(Clone, Debug, Default)]
pub struct LayoutOptions {
    pub mode: LayoutMode,
    /// The entry file's source map, when the caller has one. Only
    /// [`LayoutMode::Code`] reads it — for a line's true indentation.
    /// `None` falls back to the first node's source column.
    pub source_map: Option<std::sync::Arc<crate::ir::SourceMap>>,
    /// Set by [`recurse_chips`] while laying out a chip's interior. Only the
    /// outermost module takes ownership of source comments that fall outside
    /// every row, so a file's leading notes land on one plane, not all of them.
    pub nested: bool,
    /// Which plane renders each own-line comment, keyed by source line; the
    /// value names the chip node that owns it, or `None` for the outermost
    /// module.
    ///
    /// Decided ONCE for the whole tree, because it cannot be decided locally:
    /// a module's line span is a min/max envelope over rows that may be
    /// sparse, so sibling envelopes overlap and each sibling would claim the
    /// same comment. Built by the outermost `layout_code` and handed down.
    pub comment_owner: Option<std::sync::Arc<HashMap<i32, Option<NodeId>>>>,
    /// The chip node this module was reached through; `None` at the root.
    /// Matched against [`Self::comment_owner`] to decide what this plane
    /// renders.
    pub self_chip: Option<NodeId>,
}

impl LayoutOptions {
    /// The options a program's own settings ask for: its layout engine and,
    /// for code layout, its source map.
    pub fn for_program(settings: &crate::ir::ProgramSettings) -> Self {
        Self {
            mode: settings.layout,
            source_map: settings.source_map.clone(),
            ..Default::default()
        }
    }
}

/// Compute a layout for `module`. Recurses into each child chip.
pub fn layout(module: &Module) -> LayoutResult {
    layout_with_opts(module, &LayoutOptions::default())
}

/// Fast 3D grid layout for large modules — places nodes in a cube arrangement
/// using actual brick sizes from the inventory, skipping expensive DAG analysis.
/// The resulting brick mass is centered around the origin so it sits in the
/// middle of the microchip plane, not offset to a corner.
fn layout_grid_impl(module: &Module, opts: &LayoutOptions, recurse: bool) -> LayoutResult {
    let spawnable: Vec<(&NodeId, &Node)> = module
        .nodes_in_order()
        .filter(|(_, n)| n.kind.has_brick())
        .collect();
    let count = spawnable.len();
    let side = (count as f64).cbrt().ceil() as usize;

    let mut placements: HashMap<NodeId, Placement> = HashMap::default();
    let mut x = 0i32;
    let mut y = 0i32;
    // The BOTTOM plane of the current layer. `Placement::z` is a brick CENTRE
    // (emit offsets x and y by their half-extents but passes z straight
    // through), so each brick sits half its own height above this. Sharing one
    // centre across a layer hangs a tall brick down into the layer below, and
    // the game drops intersecting bricks at load with no error.
    let mut layer_base = Z_PLANE;
    let mut row_height = 0i32;
    let mut col = 0usize;
    let mut row_in_layer = 0usize;
    let mut extent = Extent::point(0, 0, layer_base);
    // Layers are spaced by the tallest brick actually in the layer, floored at
    // the nominal step. A fixed step would be silently wrong for the few bricks
    // taller than it — and bricks that intersect are DROPPED by the game with
    // no error, so the clearance has to come from the contents, not a constant.
    const Z_STEP_MIN: i32 = 12;
    let mut layer_depth = 0i32;

    for (id, node) in &spawnable {
        let (hsx, hsy) = brick_half_size(node);
        let fw = hsx * 2;
        let fh = hsy * 2;
        let hsz = brick_half_height(node);

        placements.insert(**id, Placement { x, y, z: layer_base + hsz });
        extent.add((x, y, layer_base), (fw, fh, 2 * hsz));
        row_height = row_height.max(fh);
        layer_depth = layer_depth.max(2 * hsz);
        x += fw;
        col += 1;

        if col >= side {
            col = 0;
            x = 0;
            y += row_height;
            row_height = 0;
            row_in_layer += 1;

            if row_in_layer >= side {
                row_in_layer = 0;
                y = 0;
                layer_base += Z_STEP_MIN.max(layer_depth);
                layer_depth = 0;
            }
        }
    }

    let half_x = extent.max.x / 2;
    let half_y = extent.max.y / 2;
    for p in placements.values_mut() {
        p.x -= half_x;
        p.y -= half_y;
    }
    let extent = extent.shifted(-half_x, -half_y);

    LayoutResult {
        placements,
        chip_layouts: chip_layouts_of(module, opts, recurse),
        annotations: Vec::new(),
        // Only Code mode rotates bricks.
        rotations: HashMap::default(),
        // Only Code mode buses values through the gutter.
        bus: BusLayout::default(),
        bounds_min: extent.min,
        bounds_max: extent.max,
    }
}

const GRID_LAYOUT_THRESHOLD: usize = 5000;

/// Like [`layout`] but with explicit options.
pub fn layout_with_opts(module: &Module, opts: &LayoutOptions) -> LayoutResult {
    let mut result = layout_impl(module, opts, true);
    order_function_ports(module, &mut result);
    result
}

/// Put each function plane among `module`'s chips' inputs, and its results,
/// top to bottom in slot order. Every chip's own layout comes from
/// [`layout_with_opts`], which does this for the planes inside it.
///
/// A Function Call maps its `Input0..7` and `Output0..7` onto the plane's
/// `FunctionInput` / `FunctionOutput` bricks by position, not `PortOrder`:
/// the brick with the highest local X (the top of the board) is slot 0
/// (measured in game 2026-10-08). The bricks are all 1x1, so this permutes
/// the positions layout gave them among themselves, highest X to the lowest
/// slot.
fn order_function_ports(module: &Module, result: &mut LayoutResult) {
    use crate::ir::gate_class as gc;
    let port_order = crate::intern::intern("PortOrder");
    for (id, child) in &module.chips {
        let Some(sub) = result.chip_layouts.get_mut(id) else { continue };
        if module.nodes.get(id).is_some_and(|n| n.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION) {
            for class in [gc::FUNCTION_INPUT, gc::FUNCTION_OUTPUT] {
                let mut ports: Vec<(i64, NodeId)> = child
                    .nodes
                    .values()
                    .filter(|n| n.gate_class == class && n.kind == NodeKind::Gate)
                    .filter_map(|n| match n.properties.get(&port_order) {
                        Some(crate::ir::Literal::Int(k)) => Some((*k, n.id)),
                        _ => None,
                    })
                    .collect();
                ports.sort();
                let mut spots: Vec<Placement> =
                    ports.iter().filter_map(|(_, n)| sub.placements.get(n).copied()).collect();
                if spots.len() != ports.len() {
                    continue;
                }
                spots.sort_by_key(|p| (std::cmp::Reverse(p.x), p.y, p.z));
                for ((_, n), spot) in ports.iter().zip(spots) {
                    sub.placements.insert(*n, spot);
                }
            }
        }
    }
}

fn layout_impl(module: &Module, opts: &LayoutOptions, recurse: bool) -> LayoutResult {
    match opts.mode {
        LayoutMode::Code => return code::layout_code(module, opts, recurse),
        // Asking for the cube gets it at any size, including the empty module —
        // the size fallback below is a ceiling on DAG cost, not the only route.
        LayoutMode::Cube => return layout_grid_impl(module, opts, recurse),
        LayoutMode::Dag => {}
    }
    if module.nodes.len() > GRID_LAYOUT_THRESHOLD {
        return layout_grid_impl(module, opts, recurse);
    }

    // Flat DAG layout over the whole module — block structure is
    // ignored for placement purposes in this pass.
    assert!(
        module.scopes.contains_key(&ROOT_SCOPE_ID),
        "module must have a root scope"
    );
    let whole_module = Region {
        id: ROOT_SCOPE_ID,
        #[cfg(any(test, feature = "test-internals"))]
        info: &module.scopes[&ROOT_SCOPE_ID],
        own_nodes: module.nodes_in_order().map(|(_, n)| n).collect(),
        #[cfg(any(test, feature = "test-internals"))]
        children: Vec::new(),
    };
    let mut laid = layout_leaf(&whole_module, &module.wires);

    let mut placements: HashMap<NodeId, Placement> = HashMap::default();
    if laid.local.is_empty() {
        return LayoutResult::empty(chip_layouts_of(module, opts, recurse));
    }

    // Order matters: pin first so Vars move to the bottom strip and free
    // up rank-0 for the rest. Then align source-like nodes to their
    // consumers' rows so sparse source columns collapse. Compact the
    // axes to remove any gaps left behind. Finally wrap if the depth
    // axis would otherwise overflow the chip plane.
    pin_specials(&mut laid, module);
    align_sources_to_consumers(&mut laid, module);
    compact_rank_axis(&mut laid);
    wrap_long_rows(&mut laid);

    // Convert (dx, dy) cells into world-unit Placements using per-row
    // / per-column sizes driven by each node's inventory half-size.
    let (sized, total_w, total_h) = resolve_variable_sizes(&laid, module);

    // Center around (0, 0) so the inner plane extent is symmetric.
    let shift_x = -total_w / 2;
    let shift_y = -total_h / 2;

    let mut extent = Extent::EMPTY;
    for (id, (px, py)) in sized {
        let x = px + shift_x;
        let y = py + shift_y;
        // Bounds reflect actual brick extents (min corner -> max corner)
        // so the CLI can size the chip plane to wrap every brick.
        let (hsx, hsy) = module
            .nodes
            .get(&id)
            .map(brick_half_size)
            .unwrap_or((DEFAULT_HALF_SIZE, DEFAULT_HALF_SIZE));
        extent.add((x, y, Z_PLANE), (hsx * 2, hsy * 2, 0));
        placements.insert(id, Placement { x, y, z: Z_PLANE });
    }

    LayoutResult {
        placements,
        chip_layouts: chip_layouts_of(module, opts, recurse),
        annotations: Vec::new(),
        // Only Code mode rotates bricks.
        rotations: HashMap::default(),
        // Only Code mode buses values through the gutter.
        bus: BusLayout::default(),
        bounds_min: IntVec3 { z: Z_PLANE, ..extent.min },
        bounds_max: IntVec3 { z: Z_PLANE, ..extent.max },
    }
}

/// Compact the rank axis by pulling source nodes (no predecessors in the
/// layout) to the same `dy` as their primary consumer, so sources don't
/// stack into their own long vertical strip. Skips pinned-to-be-pinned
/// kinds (inputs / outputs / pseudo-vars) — those get their own row.
/// Runs a few passes so downstream moves propagate back.
fn align_sources_to_consumers(laid: &mut RegionLayout, module: &Module) {
    fn is_pinned(node: &Node) -> bool {
        matches!(node.kind, NodeKind::Input | NodeKind::Output) || is_pseudo_storage(node)
    }

    // The set of laid-out nodes and the wire topology never change between
    // passes (only placements move), so resolve each node's primary
    // consumer once: first consumer in the layout by source order, wire
    // order breaking ties.
    let mut consumers: HashMap<NodeId, NodeId> = HashMap::default();
    for w in &module.wires {
        if !laid.local.contains_key(&w.target.node_id) {
            continue;
        }
        let offset = |c: &NodeId| {
            module
                .nodes
                .get(c)
                .map(|n| n.source_range.start.offset)
                .unwrap_or(usize::MAX)
        };
        consumers
            .entry(w.source.node_id)
            .and_modify(|best| {
                if offset(&w.target.node_id) < offset(best) {
                    *best = w.target.node_id;
                }
            })
            .or_insert(w.target.node_id);
    }

    // Occupancy count per cell, maintained incrementally across moves so
    // collision checks are O(1) instead of a scan over every placement.
    let mut occupied: HashMap<(i32, i32), u32> = HashMap::default();
    for p in laid.local.values() {
        *occupied.entry((p.dx, p.dy)).or_insert(0) += 1;
    }

    // A move claims a cell its later rivals then skip, so the visit order
    // picks the winner. Id order, not map order: map order differs between
    // 32-bit and 64-bit targets, and the wasm builds would lay out differently.
    let mut ids: Vec<NodeId> = laid.local.keys().cloned().collect();
    ids.sort_unstable();
    for _ in 0..4 {
        let mut changed = false;
        for id in &ids {
            let Some(node) = module.nodes.get(id) else {
                continue;
            };
            if is_pinned(node) {
                continue;
            }
            // Eligible: nodes that don't consume exec (literals,
            // source events, pure expressions). Exec-taking gates
            // stay in their topological column so the exec chain
            // reads linearly across the chip.
            let takes_exec = node
                .ports
                .inputs
                .iter()
                .any(|p| matches!(p.ty, Type::Exec));
            if takes_exec {
                continue;
            }
            let Some(target_id) = consumers.get(id) else {
                continue;
            };
            let target = laid.local[target_id];
            let me = laid.local[id];
            // Try two candidate placements in order of preference:
            //   1. One column left of the consumer, same row — so the
            //      source sits immediately feeding into it.
            //   2. Same column as today, but at the consumer's row —
            //      keeps the source in its natural depth if (1) collides.
            // First candidate without a collision wins.
            let candidates: [(i32, i32); 2] = [(target.dx - 1, target.dy), (me.dx, target.dy)];
            let mut moved = false;
            for (cx, cy) in candidates {
                if cx < 0 {
                    continue;
                }
                if cx == me.dx && cy == me.dy {
                    break;
                }
                // The candidate is never this node's own cell (checked
                // above), so any occupant is a collision.
                if occupied.get(&(cx, cy)).is_some_and(|&c| c > 0) {
                    continue;
                }
                let entry = laid.local.get_mut(id).unwrap();
                entry.dx = cx;
                entry.dy = cy;
                if let Some(c) = occupied.get_mut(&(me.dx, me.dy)) {
                    *c -= 1;
                }
                *occupied.entry((cx, cy)).or_insert(0) += 1;
                moved = true;
                break;
            }
            if moved {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Max columns a single row of the layout may span before we wrap the
/// excess into a new "super-row" below. Measured in cell units where
/// each cell is 10 world units, so 128 columns ≈ 1280 world
/// units — well inside a single 2048-unit chunk.
const MAX_COLS_PER_ROW: i32 = 128;
/// Empty-cell gutter between super-rows.
const WRAP_GUTTER_ROWS: i32 = 1;

/// Size `laid`'s bounding box, in cells, to the nodes it holds.
fn fit_bbox(laid: &mut RegionLayout) {
    let w = laid.local.values().map(|p| p.dx).max().unwrap_or(0) + 1;
    let h = laid.local.values().map(|p| p.dy).max().unwrap_or(0) + 1;
    laid.bbox = (w, h);
}

/// If the layout has more than `MAX_COLS_PER_ROW` columns, fold the
/// excess columns into new super-rows so the chip stays inside a single
/// chunk. Each super-row is `max_dy + WRAP_GUTTER_ROWS` tall. Empty rows
/// between super-rows separate the groups visually.
fn wrap_long_rows(laid: &mut RegionLayout) {
    let (w, h) = laid.bbox;
    if w <= MAX_COLS_PER_ROW {
        return;
    }
    let band = h + WRAP_GUTTER_ROWS;
    let total_super_rows = (w + MAX_COLS_PER_ROW - 1) / MAX_COLS_PER_ROW;
    for p in laid.local.values_mut() {
        // Invert super-row order so the earliest (most-top-level) chain
        // of the chip ends up at the bottom of the grid and later
        // wraps stack above it.
        let super_row = p.dx / MAX_COLS_PER_ROW;
        let flipped = total_super_rows - 1 - super_row;
        p.dx %= MAX_COLS_PER_ROW;
        p.dy += flipped * band;
    }
    fit_bbox(laid);
}

/// Collapse gaps in both axes after source-alignment moves. Nodes retain
/// relative order, but empty rows/columns disappear so the chip plane
/// isn't padded by them.
fn compact_rank_axis(laid: &mut RegionLayout) {
    use std::collections::BTreeSet;

    let used_dys: BTreeSet<i32> = laid.local.values().map(|p| p.dy).collect();
    let dy_remap: HashMap<i32, i32> = used_dys
        .iter()
        .copied()
        .enumerate()
        .map(|(i, v)| (v, i as i32))
        .collect();

    let used_dxs: BTreeSet<i32> = laid.local.values().map(|p| p.dx).collect();
    let dx_remap: HashMap<i32, i32> = used_dxs
        .iter()
        .copied()
        .enumerate()
        .map(|(i, v)| (v, i as i32))
        .collect();

    for p in laid.local.values_mut() {
        p.dy = dy_remap[&p.dy];
        p.dx = dx_remap[&p.dx];
    }
    fit_bbox(laid);
}

/// Pull chip I/O and variable-storage nodes out of the DAG layout and
/// repin them at canonical edges:
/// - `NodeKind::Input` -> column `dx = 0` (left).
/// - `NodeKind::Output` -> rightmost column.
/// - Pseudo-storage nodes -> bottom row.
fn pin_specials(laid: &mut RegionLayout, module: &Module) {
    let mut inputs: Vec<NodeId> = Vec::new();
    let mut outputs: Vec<NodeId> = Vec::new();
    let mut vars: Vec<NodeId> = Vec::new();
    for id in laid.local.keys().cloned().collect::<Vec<_>>() {
        let Some(node) = module.nodes.get(&id) else {
            continue;
        };
        match node.kind {
            NodeKind::Input => inputs.push(id),
            NodeKind::Output => outputs.push(id),
            _ if is_pseudo_storage(node) => vars.push(id),
            _ => {}
        }
    }
    let src_offset = |id: &NodeId| -> usize {
        module
            .nodes
            .get(id)
            .map(|n| n.source_range.start.offset)
            .unwrap_or(0)
    };
    inputs.sort_by_key(|id| (src_offset(id), id.0));
    outputs.sort_by_key(|id| (src_offset(id), id.0));
    vars.sort_by_key(|id| (src_offset(id), id.0));

    for id in inputs.iter().chain(&outputs).chain(&vars) {
        laid.local.remove(id);
    }

    // Shift normal nodes right by one column if we have inputs to pin,
    // and shift them down by one row to make room for the vars strip at
    // the bottom of the layout (dy = 0).
    let shift_right = if inputs.is_empty() { 0 } else { 1 };
    let shift_down = if vars.is_empty() { 0 } else { 1 };
    if shift_right != 0 || shift_down != 0 {
        for p in laid.local.values_mut() {
            p.dx += shift_right;
            p.dy += shift_down;
        }
    }
    // A larger dy is higher on the plane, so the first-declared port takes
    // the top row of its stack and the rest step down in declaration order,
    // lining up with the chip's outer side pins.
    for (i, id) in inputs.iter().rev().enumerate() {
        laid.local.insert(
            *id,
            LocalPlacement {
                dx: 0,
                dy: i as i32 + shift_down,
            },
        );
    }
    let rightmost = laid.local.values().map(|p| p.dx).max().unwrap_or(-1) + 1;
    for (i, id) in outputs.iter().rev().enumerate() {
        laid.local.insert(
            *id,
            LocalPlacement {
                dx: rightmost,
                dy: i as i32 + shift_down,
            },
        );
    }
    // Vars land at dy = 0 (smallest Placement.x -> visually the bottom of
    // the chip) spread left-to-right through the columns.
    for (i, id) in vars.iter().enumerate() {
        laid.local.insert(
            *id,
            LocalPlacement {
                dx: i as i32,
                dy: 0,
            },
        );
    }

    fit_bbox(laid);
}

/// Turn cell coordinates `(dx, dy)` into placement-unit coordinates
/// `(px, py)` using per-column / per-row sizes from the inventory.
///
/// Each column's width is `max(half_size.y * 2)` across its nodes;
/// each row's height is `max(half_size.x * 2)`. Offsets accumulate
/// deterministically in ascending `dx` / `dy` order. Returned tuple:
/// `(placements, total_width_in_placement_units, total_height_in_placement_units)`.
fn resolve_variable_sizes(
    laid: &RegionLayout,
    module: &Module,
) -> (HashMap<NodeId, (i32, i32)>, i32, i32) {
    let mut row_thickness: HashMap<i32, i32> = HashMap::default(); // dy -> Placement.x thickness
    let mut col_thickness: HashMap<i32, i32> = HashMap::default(); // dx -> Placement.y thickness
    for (id, p) in &laid.local {
        let (hsx, hsy) = module
            .nodes
            .get(id)
            .map(brick_half_size)
            .unwrap_or((DEFAULT_HALF_SIZE, DEFAULT_HALF_SIZE));
        let full_x = hsx * 2;
        let full_y = hsy * 2;
        row_thickness
            .entry(p.dy)
            .and_modify(|v| *v = (*v).max(full_x))
            .or_insert(full_x);
        col_thickness
            .entry(p.dx)
            .and_modify(|v| *v = (*v).max(full_y))
            .or_insert(full_y);
    }

    let mut sorted_dys: Vec<i32> = row_thickness.keys().copied().collect();
    sorted_dys.sort();
    let mut row_off: HashMap<i32, i32> = HashMap::default();
    let mut total_x = 0i32;
    for dy in &sorted_dys {
        row_off.insert(*dy, total_x);
        total_x += row_thickness[dy];
    }

    let mut sorted_dxs: Vec<i32> = col_thickness.keys().copied().collect();
    sorted_dxs.sort();
    let mut col_off: HashMap<i32, i32> = HashMap::default();
    let mut total_y = 0i32;
    for dx in &sorted_dxs {
        col_off.insert(*dx, total_y);
        total_y += col_thickness[dx];
    }

    let mut out: HashMap<NodeId, (i32, i32)> = HashMap::default();
    for (id, p) in &laid.local {
        let px = row_off[&p.dy];
        let py = col_off[&p.dx];
        out.insert(*id, (px, py));
    }
    (out, total_x, total_y)
}

/// Sub-layouts for the module's chips, or nothing when the caller wanted a
/// single level. Every `LayoutResult` in the crate fills `chip_layouts` this
/// way; going through one function is what keeps a new layout mode from
/// forgetting the `recurse` flag.
pub(super) fn chip_layouts_of(
    module: &Module,
    opts: &LayoutOptions,
    recurse: bool,
) -> HashMap<NodeId, LayoutResult> {
    if recurse {
        recurse_chips(module, opts)
    } else {
        HashMap::default()
    }
}

fn recurse_chips(module: &Module, opts: &LayoutOptions) -> HashMap<NodeId, LayoutResult> {
    let mut chip_layouts: HashMap<NodeId, LayoutResult> = HashMap::default();
    if module.chips.is_empty() {
        return chip_layouts;
    }
    for (chip_id, child_module) in &module.chips {
        let child_opts = LayoutOptions {
            nested: true,
            self_chip: Some(*chip_id),
            ..opts.clone()
        };
        chip_layouts.insert(*chip_id, layout_with_opts(child_module, &child_opts));
    }
    chip_layouts
}

#[cfg(test)]
mod tests;
