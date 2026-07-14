//! Gate layout pass.
//!
//! Flat topological placement: every node in the module lands in a
//! single DAG layout ordered by longest-path depth. Bricks are sized
//! per-node from the gate inventory so variable-width gates
//! (FormatText, DisplayText) don't overlap neighbours.
//!
//! Pin rules applied on top of the topo layout:
//! - `NodeKind::Input` → leftmost column.
//! - `NodeKind::Output` → rightmost column.
//! - Pseudo-storage nodes (`BrickComponentType_WireGraphPseudo_*`) →
//!   bottom row.
//!
//! The region/compose machinery (`region.rs`, `compose.rs`) is retained
//! for future block-aware layouts but currently unused.

#[allow(dead_code)]
mod compose;
mod dag;
mod region;
pub mod wall;

use std::collections::HashMap;

use crate::catalog::default_catalog;
use crate::emit::Placement;
use crate::ir::{Module, Node, NodeId, NodeKind, ROOT_SCOPE_ID, Type};

use self::dag::{LocalPlacement, RegionLayout, layout_leaf};
use self::region::Region;

/// Default half-size used for gates not found in the inventory.
/// Standard 1×1 gate bricks have half-size 5 (10×10 full).
const DEFAULT_HALF_SIZE: i32 = 5;

/// Legacy cell stride exposed for tests + the `placements_overlap`
/// helper. The real layout uses per-node sizes from the catalog.
pub const CELL_W: i32 = 10;
pub const CELL_H: i32 = 10;
pub const CELL_HALF_W: i32 = CELL_W / 2;
pub const CELL_HALF_H: i32 = CELL_H / 2;
/// Fixed Z plane for inner-chip bricks.
pub const Z_PLANE: i32 = 2;

/// True for pseudo-nodes that represent persistent STORAGE (var, buffer,
/// array). Pseudo_Literal is NOT in this bucket — literals are sources
/// that should align with their consumer, not get pinned to the bottom.
fn is_pseudo_storage(node: &Node) -> bool {
    matches!(
        node.gate_class,
        "BrickComponentType_WireGraphPseudo_Var"
            | "BrickComponentType_WireGraphPseudo_Buffer"
            | "BrickComponentType_WireGraphPseudo_BufferTicks"
            | "BrickComponentType_WireGraphPseudo_ArrayVar"
    )
}

/// Look up a node's brick half-size from the gate inventory, falling
/// back to the standard 1×1 half-size.
fn brick_half_size(node: &Node) -> (i32, i32) {
    default_catalog()
        .find_by_class(node.gate_class)
        .map(|g| (g.half_size.x, g.half_size.y))
        .unwrap_or((DEFAULT_HALF_SIZE, DEFAULT_HALF_SIZE))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntVec3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    /// Placements for every node in the root module.
    pub placements: HashMap<NodeId, Placement>,
    /// Per-chip sub-layouts keyed by chip node id.
    pub chip_layouts: HashMap<NodeId, LayoutResult>,
    pub bounds_min: IntVec3,
    pub bounds_max: IntVec3,
}

/// How to render a nested chip's interior. Mirrors builder crate's
/// `flat` flag.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ChipLayoutMode {
    /// Chip renders as a single microchip brick on the parent grid; its
    /// internal gates are placed on a separate baseplate keyed by the
    /// chip node's id. This is the Brickadia-native representation.
    #[default]
    Collapsed,
    /// Chip's internal gates are placed next to the chip node on the
    /// *parent* grid, sharing its coordinate space. Useful for debugging
    /// and small chips where you want to see everything at once. Requires
    /// emit cooperation to honor.
    AdjacentInline,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutOptions {
    pub chips: ChipLayoutMode,
}

/// Compute a layout for `module`. Recurses into each child chip.
pub fn layout(module: &Module) -> LayoutResult {
    layout_with_opts(module, &LayoutOptions::default())
}

/// Like [`layout`] but does NOT recurse into child chips
/// (`chip_layouts` is left empty). The emit pipeline lays out each chip
/// exactly once at the level that emits it, so eager recursion here
/// would redo every descendant's layout only to throw it away.
pub fn layout_root(module: &Module) -> LayoutResult {
    layout_impl(module, &LayoutOptions::default(), false)
}

/// Fast 3D grid layout for large modules — places nodes in a cube arrangement
/// using actual brick sizes from the inventory, skipping expensive DAG analysis.
/// The resulting brick mass is centered around the origin so it sits in the
/// middle of the microchip plane, not offset to a corner.
pub fn layout_grid(module: &Module, opts: &LayoutOptions) -> LayoutResult {
    layout_grid_impl(module, opts, true)
}

fn layout_grid_impl(module: &Module, opts: &LayoutOptions, recurse: bool) -> LayoutResult {
    let spawnable: Vec<(&NodeId, &Node)> = module
        .nodes
        .iter()
        .filter(|(_, n)| {
            matches!(
                n.kind,
                NodeKind::Gate
                    | NodeKind::Event
                    | NodeKind::Input
                    | NodeKind::Output
                    | NodeKind::Chip
            )
        })
        .collect();
    let count = spawnable.len();
    let side = (count as f64).cbrt().ceil() as usize;

    let mut placements: HashMap<NodeId, Placement> = HashMap::new();
    let mut x = 0i32;
    let mut y = 0i32;
    let mut z = Z_PLANE;
    let mut row_height = 0i32;
    let mut col = 0usize;
    let mut row_in_layer = 0usize;
    let mut raw_max_x = 0i32;
    let mut raw_max_y = 0i32;
    let mut raw_max_z = z;
    let z_step = 12;

    for (id, node) in &spawnable {
        let (hsx, hsy) = brick_half_size(node);
        let fw = hsx * 2;
        let fh = hsy * 2;

        placements.insert(**id, Placement { x, y, z });
        raw_max_x = raw_max_x.max(x + fw);
        raw_max_y = raw_max_y.max(y + fh);
        raw_max_z = raw_max_z.max(z);
        row_height = row_height.max(fh);
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
                z += z_step;
            }
        }
    }

    let half_x = raw_max_x / 2;
    let half_y = raw_max_y / 2;
    for p in placements.values_mut() {
        p.x -= half_x;
        p.y -= half_y;
    }

    LayoutResult {
        placements,
        chip_layouts: if recurse {
            recurse_chips(module, opts)
        } else {
            HashMap::new()
        },
        bounds_min: IntVec3 {
            x: -half_x,
            y: -half_y,
            z: Z_PLANE,
        },
        bounds_max: IntVec3 {
            x: raw_max_x - half_x,
            y: raw_max_y - half_y,
            z: raw_max_z,
        },
    }
}

const GRID_LAYOUT_THRESHOLD: usize = 5000;

/// Like [`layout`] but with explicit options.
pub fn layout_with_opts(module: &Module, opts: &LayoutOptions) -> LayoutResult {
    layout_impl(module, opts, true)
}

fn layout_impl(module: &Module, opts: &LayoutOptions, recurse: bool) -> LayoutResult {
    if module.nodes.len() > GRID_LAYOUT_THRESHOLD {
        return layout_grid_impl(module, opts, recurse);
    }

    // Flat DAG layout over the whole module — block structure is
    // ignored for placement purposes in this pass.
    let root_info = module
        .scopes
        .get(&ROOT_SCOPE_ID)
        .expect("module must have a root scope");
    let whole_module = Region {
        id: ROOT_SCOPE_ID,
        info: root_info,
        own_nodes: module.nodes.values().collect(),
        children: Vec::new(),
    };
    let mut laid = layout_leaf(&whole_module, &module.wires);

    let mut placements: HashMap<NodeId, Placement> = HashMap::new();
    if laid.local.is_empty() {
        return LayoutResult {
            placements,
            chip_layouts: if recurse {
                recurse_chips(module, opts)
            } else {
                HashMap::new()
            },
            bounds_min: IntVec3::default(),
            bounds_max: IntVec3::default(),
        };
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

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for (id, (px, py)) in sized {
        let x = px + shift_x;
        let y = py + shift_y;
        // Bounds reflect actual brick extents (min corner → max corner)
        // so the CLI can size the chip plane to wrap every brick.
        let (hsx, hsy) = module
            .nodes
            .get(&id)
            .map(brick_half_size)
            .unwrap_or((DEFAULT_HALF_SIZE, DEFAULT_HALF_SIZE));
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + hsx * 2);
        max_y = max_y.max(y + hsy * 2);
        placements.insert(id, Placement { x, y, z: Z_PLANE });
    }

    LayoutResult {
        placements,
        chip_layouts: if recurse {
            recurse_chips(module, opts)
        } else {
            HashMap::new()
        },
        bounds_min: IntVec3 {
            x: min_x,
            y: min_y,
            z: Z_PLANE,
        },
        bounds_max: IntVec3 {
            x: max_x,
            y: max_y,
            z: Z_PLANE,
        },
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
    let mut consumers: HashMap<NodeId, NodeId> = HashMap::new();
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
    let mut occupied: HashMap<(i32, i32), u32> = HashMap::new();
    for p in laid.local.values() {
        *occupied.entry((p.dx, p.dy)).or_insert(0) += 1;
    }

    let ids: Vec<NodeId> = laid.local.keys().cloned().collect();
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
/// each cell is `CELL_W = 10` world units, so 128 columns ≈ 1280 world
/// units — well inside a single 2048-unit chunk.
const MAX_COLS_PER_ROW: i32 = 128;
/// Empty-cell gutter between super-rows.
const WRAP_GUTTER_ROWS: i32 = 1;

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
    let new_w = laid.local.values().map(|p| p.dx).max().unwrap_or(0) + 1;
    let new_h = laid.local.values().map(|p| p.dy).max().unwrap_or(0) + 1;
    laid.bbox = (new_w, new_h);
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
    let w = laid.local.values().map(|p| p.dx).max().unwrap_or(0) + 1;
    let h = laid.local.values().map(|p| p.dy).max().unwrap_or(0) + 1;
    laid.bbox = (w, h);
}

/// Pull chip I/O and variable-storage nodes out of the DAG layout and
/// repin them at canonical edges:
/// - `NodeKind::Input` → column `dx = 0` (left).
/// - `NodeKind::Output` → rightmost column.
/// - Pseudo-storage nodes → bottom row.
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
    for (i, id) in inputs.iter().enumerate() {
        laid.local.insert(
            *id,
            LocalPlacement {
                dx: 0,
                dy: i as i32 + shift_down,
            },
        );
    }
    let rightmost = laid.local.values().map(|p| p.dx).max().unwrap_or(-1) + 1;
    for (i, id) in outputs.iter().enumerate() {
        laid.local.insert(
            *id,
            LocalPlacement {
                dx: rightmost,
                dy: i as i32 + shift_down,
            },
        );
    }
    // Vars land at dy = 0 (smallest Placement.x → visually the bottom of
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

    let w = laid.local.values().map(|p| p.dx).max().unwrap_or(0) + 1;
    let h = laid.local.values().map(|p| p.dy).max().unwrap_or(0) + 1;
    laid.bbox = (w, h);
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
    let mut row_thickness: HashMap<i32, i32> = HashMap::new(); // dy → Placement.x thickness
    let mut col_thickness: HashMap<i32, i32> = HashMap::new(); // dx → Placement.y thickness
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
    let mut row_off: HashMap<i32, i32> = HashMap::new();
    let mut total_x = 0i32;
    for dy in &sorted_dys {
        row_off.insert(*dy, total_x);
        total_x += row_thickness[dy];
    }

    let mut sorted_dxs: Vec<i32> = col_thickness.keys().copied().collect();
    sorted_dxs.sort();
    let mut col_off: HashMap<i32, i32> = HashMap::new();
    let mut total_y = 0i32;
    for dx in &sorted_dxs {
        col_off.insert(*dx, total_y);
        total_y += col_thickness[dx];
    }

    let mut out: HashMap<NodeId, (i32, i32)> = HashMap::new();
    for (id, p) in &laid.local {
        let px = row_off[&p.dy];
        let py = col_off[&p.dx];
        out.insert(*id, (px, py));
    }
    (out, total_x, total_y)
}

fn recurse_chips(module: &Module, opts: &LayoutOptions) -> HashMap<NodeId, LayoutResult> {
    let mut chip_layouts: HashMap<NodeId, LayoutResult> = HashMap::new();
    for (chip_id, child_module) in &module.chips {
        chip_layouts.insert(*chip_id, layout_with_opts(child_module, opts));
    }
    chip_layouts
}

/// True if two placements' AABBs (with `CELL_W`/`CELL_H` footprint) overlap.
/// Used by tests to guard against regressions.
pub fn placements_overlap(a: Placement, b: Placement) -> bool {
    let half_x = CELL_W / 2;
    let half_y = CELL_H / 2;
    (a.x - b.x).abs() < 2 * half_x && (a.y - b.y).abs() < 2 * half_y && a.z == b.z
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{GateIO, Module, Node, NodeKind, PortSpec, SourceRange, Type};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn gate(_label: &str) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class: crate::ir::gate_class::REROUTER,
            properties: Arc::new(HashMap::new()),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec { name: *crate::intern::sym::RER_INPUT, ty: Type::Any }],
                outputs: vec![PortSpec { name: *crate::intern::sym::RER_OUTPUT, ty: Type::Any }],
            }),
            source_range: SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        }
    }

    #[test]
    fn empty_module_layout_empty() {
        let m = Module::new("empty");
        let l = layout(&m);
        assert!(l.placements.is_empty());
    }

    #[test]
    fn layout_output_is_deterministic() {
        let mut m = Module::new("det");
        for id in ["a", "b", "c"] {
            m.add_node(gate(id));
        }
        let a = layout(&m);
        let b = layout(&m);
        assert_eq!(a.placements, b.placements);
    }

    #[test]
    fn every_node_gets_a_placement() {
        let mut m = Module::new("coverage");
        for id in ["a", "b", "c", "d"] {
            m.add_node(gate(id));
        }
        let l = layout(&m);
        assert_eq!(l.placements.len(), m.nodes.len());
    }

    #[test]
    fn nested_chip_gets_its_own_layout() {
        // Parent has one chip node; the child module has its own gates.
        let mut parent = Module::new("parent");
        let mut chip_node = gate("my_chip");
        chip_node.kind = NodeKind::Chip;
        chip_node.gate_class = crate::ir::gate_class::MICROCHIP;
        let chip_id = chip_node.id;
        parent.add_node(chip_node);

        let mut child = Module::new_chip_body("child", "my_chip");
        let inner_node = gate("inner");
        let inner_id = inner_node.id;
        child.add_node(inner_node);
        parent.chips.insert(chip_id, child);

        let l = layout(&parent);
        assert!(
            l.placements.contains_key(&chip_id),
            "parent must place the chip node"
        );
        let child_l = l
            .chip_layouts
            .get(&chip_id)
            .expect("chip layout must exist");
        assert!(
            child_l.placements.contains_key(&inner_id),
            "child module must place its inner gate"
        );
    }

    #[test]
    fn layout_options_passed_through_to_chip_recursion() {
        // Smoke test: different options don't alter the collapsed-mode
        // output (the only mode we currently implement). AdjacentInline
        // is a no-op placeholder today.
        let mut parent = Module::new("parent");
        parent.add_node(gate("a"));
        let default_out = layout(&parent);
        let inline_out = layout_with_opts(
            &parent,
            &LayoutOptions {
                chips: ChipLayoutMode::AdjacentInline,
            },
        );
        assert_eq!(default_out.placements, inline_out.placements);
    }
}
