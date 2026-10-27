//! Code-shaped layout: source row = source line, indent = source column.
//!
//! Nodes with a real range in the module's most common source file place
//! literally: earlier lines sit higher, deeper columns sit further right.
//! Nodes without a usable range in that file (foreign-file operands and
//! synthetic default-range nodes) adopt the row of whichever node
//! consumes or produces them, transitively through other homeless nodes.
//! Anything unreachable lands on an overflow row below the last source
//! line. Within one line, nodes order by dependency depth over that
//! line's own wires: values flow left to right, so every node lands right
//! of the nodes feeding it, and nodes sharing a depth stack into
//! sub-rows. Chip I/O ports skip the row model entirely: every Input stacks
//! on the left edge and every Output on the right edge of its page,
//! descending from the page's top row in signature order.
//!
//! Three wrapping tiers keep large modules inside their budgets: lines
//! that exceed `line_width` soft-wrap into indented continuation
//! sub-rows; lines stack into vertical bands capped at `band_height`,
//! placed left->right with a gutter; bands that exceed `plane_width`
//! spill onto a new page stacked `PAGE_Z_STEP` higher in z. Each page is
//! flipped and centered independently so it reads top-down on its own.

use std::cmp::Reverse;
use std::sync::Arc;

use crate::collections::HashMap;
use crate::emit::Placement;
use crate::ir::{Module, Node, NodeId};

use super::{
    BusLayout, Extent, LayoutOptions, LayoutResult, NodeRotation, TextAnnotation, Z_PLANE,
    chip_layouts_of,
};

mod adjacency;
mod comments;
mod gutter;
mod lines;
mod nodes;
mod pages;

use adjacency::{adopt_line, build_adjacency};
use comments::{assign_comment_owners, claimed_comments, source_indent};
use gutter::{Body, chip_owners, lay_bus, lay_mini_bus, plan_bus};
use lines::{LinePlan, plan_line};
use nodes::{anchor_file, cell_half_size, has_range, is_edge_pin, is_spawnable, rotation_of};
use pages::{PageInfo, assemble_bands, assemble_pages, place_edge_pins};

/// Horizontal shift applied per source column of a line's head node.
pub const INDENT_UNIT: i32 = 5;
/// Vertical gap contributed by a run of blank source lines (clamped at
/// `MAX_BLANK_RUN_ROWS` rows' worth).
pub const EMPTY_LINE_HEIGHT: i32 = 4;
/// Gap between wrapped bands, and between a page's edge and its stacked
/// boundary pins.
pub const BAND_GUTTER: i32 = 20;
/// Z step between stacked pages.
pub const PAGE_Z_STEP: i32 = 50;
/// Extra indent applied to a soft-wrapped continuation sub-row.
pub const CONTINUATION_INDENT: i32 = 2 * INDENT_UNIT;
/// Blank-line runs longer than this contribute the same gap as this many.
const MAX_BLANK_RUN_ROWS: i32 = 2;
/// Footprint of a comment label's carrier brick: a 1×1 plate, 10 units along
/// both axes.
pub const ANNOTATION_SIZE: i32 = 10;
/// Half-extent of a rerouter brick (`B_1x1_Reroute_Node` is 2×2×2).
pub const REROUTER_HALF: i32 = 1;
/// Horizontal distance between adjacent bus lanes. One rerouter wide, so
/// lanes stand flush against each other.
pub const LANE_PITCH: i32 = 2 * REROUTER_HALF;
/// Gap between the bus band and the code body.
///
/// Must stay `>= TAP_RESERVE`: a gate-side tap rerouter reaches back to
/// `y - TAP_RESERVE` from its column, so a smaller gutter puts it in the
/// innermost lane's cell on the leftmost brick of a row.
pub const BUS_BAND_GUTTER: i32 = 6;
/// Gap between a gate-side tap rerouter and the gate it feeds.
pub const TAP_GAP: i32 = 3;
/// Height a row's pair of lane bricks claims in its column: the
/// down-pointing lane rerouter stacked on the right-pointing tap rerouter
/// that carries the value out to the row.
pub const LANE_PAIR_HEIGHT: i32 = 4 * REROUTER_HALF;
/// Body rows a module must occupy before its bus is kept on size alone
/// (together with [`BUS_MIN_GROUPS`]).
const BUS_MIN_ROWS: usize = 8;
/// Bussed values a module must carry before its bus is kept on size alone
/// (together with [`BUS_MIN_ROWS`]).
const BUS_MIN_GROUPS: usize = 3;

/// Is a module's bus worth the bricks it costs? Kept on SIZE (at least
/// [`BUS_MIN_ROWS`] rows and [`BUS_MIN_GROUPS`] bussed values) or on COST (no
/// more rerouters than the module has gates).
///
/// The caller must treat the answer as ALL-OR-NOTHING: a dropped bus must
/// leave an entirely empty [`BusLayout`], suppression included, or emit drops
/// the suppressed wires and strands their consumers. The spacing reserved for
/// it (`TAP_RESERVE`, the pin stack's offset) stays either way.
///
/// `rows` counts rows across ALL pages and `gates` counts every spawnable
/// node, chip bricks and edge pins included.
fn bus_is_worth_its_bricks(rows: usize, groups: usize, gates: usize, nodes: usize) -> bool {
    if rows >= BUS_MIN_ROWS && groups >= BUS_MIN_GROUPS {
        return true;
    }
    nodes <= gates
}

/// The vertical distance a lane must cover to be worth building: a value
/// travelling less than its own lane bricks span was already an inline hop.
pub const MIN_LANE_TRAVEL: i32 = LANE_PAIR_HEIGHT + 2 * REROUTER_HALF;

/// Vertical step between two runs that would otherwise leave at the same
/// level. One rerouter tall, so staggered runs stack flush (touching bricks
/// are legal) while each run gets its own line.
const STAGGER_STEP: i32 = 2 * REROUTER_HALF;
/// Space held open in front of every column of a line for the gutter bus's
/// gate-side rerouter: the tap gap plus the rerouter's own footprint.
///
/// Reserved on every column, used or not: where taps land depends on the
/// body's width, so the reserve cannot be placed only where it is needed.
pub const TAP_RESERVE: i32 = TAP_GAP + 2 * REROUTER_HALF;

/// Width/height budgets driving line, band, and page wrapping.
#[derive(Clone, Copy, Debug)]
pub struct CodeBudgets {
    pub line_width: i32,
    pub band_height: i32,
    pub plane_width: i32,
}

impl Default for CodeBudgets {
    fn default() -> Self {
        Self {
            line_width: 300,
            band_height: 2000,
            plane_width: 2000,
        }
    }
}

/// Compute a code-shaped layout for `module` using the default budgets.
pub fn layout_code(module: &Module, opts: &LayoutOptions, recurse: bool) -> LayoutResult {
    layout_code_with_budgets(module, opts, recurse, &CodeBudgets::default())
}

/// Compute a code-shaped layout for `module` with explicit budgets.
pub fn layout_code_with_budgets(
    module: &Module,
    opts: &LayoutOptions,
    recurse: bool,
    budgets: &CodeBudgets,
) -> LayoutResult {
    let spawnable: Vec<(&NodeId, &Node)> = module
        .nodes_in_order()
        .filter(|(_, n)| is_spawnable(n))
        .collect();

    if spawnable.is_empty() {
        return LayoutResult::empty(chip_layouts_of(module, opts, recurse));
    }

    let anchor = anchor_file(&spawnable);

    // Comment ownership is a whole-tree decision (see `assign_comment_owners`),
    // so the outermost plane settles it and every plane below reads the answer
    // out of the options it inherits.
    let owned_opts;
    let opts = if opts.comment_owner.is_none() && !opts.nested {
        owned_opts = LayoutOptions {
            comment_owner: Some(Arc::new(assign_comment_owners(module, opts, &anchor))),
            ..opts.clone()
        };
        &owned_opts
    } else {
        opts
    };

    // literal_line: anchor-file nodes with a real range, keyed to their
    // source line. Synthesized boundary pins are held out for the edge
    // stacks. Everything else (foreign-file, synthetic default range) is
    // homeless and must adopt a line.
    let mut literal_line: HashMap<NodeId, i32> = HashMap::default();
    let mut homeless: Vec<NodeId> = Vec::new();
    let mut edge_pins: Vec<NodeId> = Vec::new();
    for (id, n) in &spawnable {
        if is_edge_pin(n) {
            edge_pins.push(**id);
        } else if has_range(n) && n.source_range.file == anchor {
            literal_line.insert(**id, n.source_range.start.line as i32);
        } else {
            homeless.push(**id);
        }
    }

    let adjacency = build_adjacency(module);
    let mut homeless_sorted = homeless;
    homeless_sorted.sort();
    let mut adopted: HashMap<NodeId, i32> = HashMap::default();
    let mut overflow: Vec<NodeId> = Vec::new();
    for id in homeless_sorted {
        match adopt_line(id, &adjacency, &literal_line) {
            Some(line) => {
                adopted.insert(id, line);
            }
            None => overflow.push(id),
        }
    }

    // Per-line entry order: literal entries first (by source position,
    // widest span first on ties), adopted entries after (by their own
    // file/position, for determinism only).
    let mut per_line: HashMap<i32, (Vec<NodeId>, Vec<NodeId>)> = HashMap::default();
    for (&id, &line) in &literal_line {
        per_line.entry(line).or_default().0.push(id);
    }
    for (&id, &line) in &adopted {
        per_line.entry(line).or_default().1.push(id);
    }
    for (lit, ado) in per_line.values_mut() {
        lit.sort_by_key(|id| {
            let n = &module.nodes[id];
            (
                n.source_range.start.offset,
                Reverse(n.source_range.end.offset),
                *id,
            )
        });
        ado.sort_by_key(|id| {
            let n = &module.nodes[id];
            (
                Arc::clone(&n.source_range.file),
                n.source_range.start.offset,
                *id,
            )
        });
    }

    // Own-line comments occupy a row of their own, so the row range spans
    // both the module's nodes and the comments it claims.
    let node_span = literal_line
        .values()
        .min()
        .copied()
        .zip(literal_line.values().max().copied());
    let comments = claimed_comments(module, opts, &anchor);
    let comment_span = comments
        .keys()
        .min()
        .copied()
        .zip(comments.keys().max().copied());
    let row_span = match (node_span, comment_span) {
        (Some((lo, hi)), Some((clo, chi))) => Some((lo.min(clo), hi.max(chi))),
        (Some(s), None) | (None, Some(s)) => Some(s),
        (None, None) => None,
    };

    // Measure each occupied line into a LinePlan (sub-rows already
    // resolved), tracking the blank-run gap that precedes it.
    let mut lines: Vec<LinePlan> = Vec::new();
    let mut ann_texts: Vec<String> = Vec::new();
    let mut blank_run = 0i32;
    let mut started = false;
    // Filled by `line_groups` as each line is planned; every measurement
    // below reads footprints through it.
    let mut rotations: HashMap<NodeId, NodeRotation> = HashMap::default();
    // Which line's block each node was planned into. The mini-bus is strictly
    // intra-line, and a node's own `source_range` cannot say this: a homeless
    // node ADOPTS a line, so its range names a different file or line than the
    // block it actually stands in.
    let mut node_line: HashMap<NodeId, usize> = HashMap::default();

    if let Some((min_line, max_line)) = row_span {
        for line in min_line..=max_line {
            let entry = per_line.get(&line);
            let comment = comments.get(&line);
            if entry.is_none() && comment.is_none() {
                if started {
                    blank_run += 1;
                }
                continue;
            }
            let gap_before = if started {
                blank_run.min(MAX_BLANK_RUN_ROWS) * EMPTY_LINE_HEIGHT
            } else {
                0
            };
            blank_run = 0;
            started = true;

            let head_col = source_indent(opts, &anchor, line)
                .or_else(|| comment.map(|c| c.col.saturating_sub(1)))
                .unwrap_or_else(|| {
                    entry
                        .and_then(|(lit, _)| lit.first())
                        .map(|id| module.nodes[id].source_range.start.col)
                        .unwrap_or(0)
                });
            let mut entries: Vec<NodeId> = Vec::new();
            if let Some((lit, ado)) = entry {
                entries.extend(lit.iter().copied());
                entries.extend(ado.iter().copied());
            }
            let annotation = comment.map(|c| {
                ann_texts.push(c.text.clone());
                ann_texts.len() - 1
            });
            for id in &entries {
                node_line.insert(*id, lines.len());
            }
            lines.push(plan_line(
                &entries,
                module,
                &adjacency,
                budgets,
                head_col,
                gap_before,
                annotation,
                &mut rotations,
            ));
        }
    }

    if !overflow.is_empty() {
        overflow.sort_by_key(|id| {
            let n = &module.nodes[id];
            (
                Arc::clone(&n.source_range.file),
                n.source_range.start.offset,
                *id,
            )
        });
        for id in &overflow {
            node_line.insert(*id, lines.len());
        }
        lines.push(plan_line(
            &overflow,
            module,
            &adjacency,
            budgets,
            0,
            0,
            None,
            &mut rotations,
        ));
    }

    let bands = assemble_bands(lines, budgets);
    let pages = assemble_pages(bands, budgets);

    // Emit each page independently: flip vertically and center around
    // its own extents, stepping z per page.
    let mut placements: HashMap<NodeId, Placement> = HashMap::default();
    let mut annotations: Vec<TextAnnotation> = Vec::new();
    // The row a body node sits in, as `(page, sub-row offset)`, which the bus
    // taps into. Page sizes and `PageInfo`s are final here; the bus never
    // widens them (see KNOWN DEVIATION on `gutter::plan_bus`).
    let mut node_row: HashMap<NodeId, (usize, i32)> = HashMap::default();
    let mut page_infos: Vec<PageInfo> = Vec::new();
    for (pi, page) in pages.iter().enumerate() {
        let z = Z_PLANE + pi as i32 * PAGE_Z_STEP;
        let mut page_h = 0i32;
        let mut page_w = 0i32;
        for &(id, right, down) in &page.nodes {
            let (hsx, hsy) = cell_half_size(&module.nodes[&id], rotation_of(&rotations, id));
            page_h = page_h.max(down + hsx * 2);
            page_w = page_w.max(right + hsy * 2);
        }
        for &(_, right, down) in &page.anns {
            page_h = page_h.max(down + ANNOTATION_SIZE);
            page_w = page_w.max(right + ANNOTATION_SIZE);
        }
        let mut info = PageInfo {
            min_y: i32::MAX,
            max_y: i32::MIN,
            top_x: i32::MIN,
            z,
        };
        for &(id, right, down) in &page.nodes {
            let (hsx, hsy) = cell_half_size(&module.nodes[&id], rotation_of(&rotations, id));
            let x = page_h - down - hsx * 2 - page_h / 2;
            let y = right - page_w / 2;
            placements.insert(id, Placement { x, y, z });
            node_row.insert(id, (pi, down));
            info.min_y = info.min_y.min(y);
            info.max_y = info.max_y.max(y + hsy * 2);
            info.top_x = info.top_x.max(x);
        }
        for &(idx, right, down) in &page.anns {
            let x = page_h - down - ANNOTATION_SIZE - page_h / 2;
            let y = right - page_w / 2;
            annotations.push(TextAnnotation {
                x,
                y,
                z,
                text: ann_texts[idx].clone(),
            });
            info.min_y = info.min_y.min(y);
            info.max_y = info.max_y.max(y + ANNOTATION_SIZE);
            info.top_x = info.top_x.max(x);
        }
        page_infos.push(info);
    }

    // The bus is planned once the body is final and before the edge pins,
    // whose stack must clear the band; it is laid after them, since a lane
    // headed by an edge pin stands at that pin's row.
    let owners = chip_owners(module);
    let body = Body {
        module,
        placements: &placements,
        rotations: &rotations,
        node_row: &node_row,
        page_infos: &page_infos,
        owners: &owners,
    };
    let mut plan = plan_bus(&body);

    place_edge_pins(
        module,
        &adjacency,
        edge_pins,
        &node_row,
        &plan.pages,
        &mut page_infos,
        &mut placements,
    );
    let body = Body {
        module,
        placements: &placements,
        rotations: &rotations,
        node_row: &node_row,
        page_infos: &page_infos,
        owners: &owners,
    };

    // Counted by `lay_bus` after its travel filter, so the size check weighs
    // the lanes actually laid.
    let (mut bus, bussable_groups) = lay_bus(&body, &mut plan);
    lay_mini_bus(&mut bus, &body, &node_line, &mut plan);

    // The bus's cost is only known once it is built, so it is built and then
    // discarded whole if it did not earn its bricks. Replacing the layout
    // drops its nodes, wires and suppression together.
    if !bus_is_worth_its_bricks(
        plan.row_index.len(),
        bussable_groups,
        spawnable.len(),
        bus.nodes.len(),
    ) {
        bus = BusLayout::default();
    }

    // Placements must straddle the origin: the emitted plane is centered on
    // PlaneCenter (0, 0, 0) and sized from the bounds span, so an asymmetric
    // bounding box puts bricks outside the plane. Comment labels and bus
    // rerouters carry a brick each, so they shift and measure with
    // everything else.
    let mut extent = Extent::EMPTY;
    for (id, p) in &placements {
        let (hsx, hsy) = cell_half_size(&module.nodes[id], rotation_of(&rotations, *id));
        extent.add((p.x, p.y, p.z), (hsx * 2, hsy * 2, 0));
    }
    for a in &annotations {
        extent.add((a.x, a.y, a.z), (ANNOTATION_SIZE, ANNOTATION_SIZE, 0));
    }
    for n in &bus.nodes {
        extent.add((n.x, n.y, n.z), (REROUTER_HALF * 2, REROUTER_HALF * 2, 0));
    }
    if !extent.is_empty() {
        let shift_x = -(extent.min.x + extent.max.x) / 2;
        let shift_y = -(extent.min.y + extent.max.y) / 2;
        if shift_x != 0 || shift_y != 0 {
            for p in placements.values_mut() {
                p.x += shift_x;
                p.y += shift_y;
            }
            for a in &mut annotations {
                a.x += shift_x;
                a.y += shift_y;
            }
            for n in &mut bus.nodes {
                n.x += shift_x;
                n.y += shift_y;
            }
        }
        extent = extent.shifted(shift_x, shift_y);
    }

    LayoutResult {
        placements,
        chip_layouts: chip_layouts_of(module, opts, recurse),
        annotations,
        rotations,
        bus,
        bounds_min: extent.min,
        bounds_max: extent.max,
    }
}

/// What the code-layout tests in `wirescript` check. Not a stable API.
#[cfg(feature = "test-internals")]
#[doc(hidden)]
pub mod test_internals {
    pub use super::adjacency::{takes_exec_input, targets_exec};
    pub use super::comments::{assign_comment_owners, claimed_comments};
    pub use super::gutter::chip_owners;
    pub use super::nodes::{
        cell_half_size, has_range, is_edge_pin, is_spawnable, label_sort_key, rotation_of,
    };
    pub use super::*;
}
