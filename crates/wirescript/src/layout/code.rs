//! Code-shaped layout: source row = source line, indent = source column.
//!
//! Nodes with a real range in the module's most common source file place
//! literally: earlier lines sit higher, deeper columns sit further right.
//! Nodes without a usable range in that file — foreign-file operands and
//! synthetic default-range nodes — adopt the row of whichever node
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
//! placed left→right with a gutter; bands that exceed `plane_width`
//! spill onto a new page stacked `PAGE_Z_STEP` higher in z. Each page is
//! flipped and centered independently so it reads top-down on its own.

use std::cmp::Reverse;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use crate::ast::SourceComment;
use crate::collections::{HashMap, HashSet};
use crate::emit::Placement;
use crate::intern::sym;
use crate::ir::port_registry::WirePort;
use crate::ir::{Literal, Module, Node, NodeId, NodeKind, PortRef, Type, gate_class};

use super::bus::{BusDemand, allocate_lanes};
use super::{
    BusEnd, BusLayout, BusNode, BusNodeId, BusRole, BusWire, IntVec3, LayoutOptions, LayoutResult,
    NodeRotation, TextAnnotation, Z_PLANE, brick_half_size, recurse_chips,
};

mod nodes;
use nodes::*;
mod adjacency;
use adjacency::*;
mod comments;
use comments::*;
mod lines;
use lines::*;
mod pages;
use pages::*;
mod gutter;
use gutter::*;

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
const CONTINUATION_INDENT: i32 = 2 * INDENT_UNIT;
/// Blank-line runs longer than this contribute the same gap as this many.
const MAX_BLANK_RUN_ROWS: i32 = 2;
/// Footprint of a comment label's carrier brick — a 1×1 plate, so the same
/// 10 units along the row and the column axis.
const ANNOTATION_SIZE: i32 = 10;
/// Half-extent of a rerouter brick (`B_1x1_Reroute_Node` is 2×2×2).
const REROUTER_HALF: i32 = 1;
/// Horizontal distance between adjacent bus lanes. One rerouter wide, so
/// lanes stand flush against each other.
const LANE_PITCH: i32 = 2 * REROUTER_HALF;
/// Gap between the bus band and the code body.
///
/// Must stay `>= TAP_RESERVE`. The innermost lane sits at
/// `body_min_y - BUS_BAND_GUTTER - LANE_PITCH`, and a gate-side tap rerouter
/// reaches back to `y - TAP_RESERVE` from its column, so a smaller gutter
/// stands the two in the same cell on the leftmost brick of a row. Today
/// that holds with one unit of slack.
const BUS_BAND_GUTTER: i32 = 6;
/// Gap between a gate-side tap rerouter and the gate it feeds.
const TAP_GAP: i32 = 3;
/// Height a row's pair of lane bricks claims in its column: the
/// down-pointing lane rerouter stacked on the right-pointing tap rerouter
/// that carries the value out to the row.
const LANE_PAIR_HEIGHT: i32 = 4 * REROUTER_HALF;
/// Body rows a module must occupy before its bus is kept on size alone.
///
/// Paired with [`BUS_MIN_GROUPS`]: a body this tall carrying this many bussed
/// values is one where the lanes are doing real work, and it keeps them
/// whatever they cost. Below it the bus has to pay for itself brick by brick.
const BUS_MIN_ROWS: usize = 8;
/// Bussable values a module must carry before its bus is kept on size alone.
/// See [`BUS_MIN_ROWS`] — a tall body carrying one value is a long lane, not a
/// bus, and it is the several-values case the band exists to untangle.
const BUS_MIN_GROUPS: usize = 3;

/// Is a module's bus worth the bricks it costs?
///
/// A lane replaces one long diagonal per consumer with a chain of rerouters,
/// which is a clear win on a body with many rows and many values crossing
/// them. On a handful of gates it is the opposite: the rerouters outnumber the
/// gates they serve and the plane reads worse for having them.
///
/// Two ways to keep it, checked in order:
///
/// 1. SIZE. A body of at least [`BUS_MIN_ROWS`] rows carrying at least
///    [`BUS_MIN_GROUPS`] bussed values keeps its bus regardless of cost —
///    that is the shape the band was built for, and the diagonals it removes
///    there are exactly the ones that make a big plane unreadable.
/// 2. COST. Otherwise the bus is kept only while it adds no more rerouters
///    than the module has real gates. One brick of routing per brick of logic
///    is the most a small body can carry before the routing IS the picture.
///
/// The caller must treat the answer as ALL-OR-NOTHING: a module that drops its
/// bus has to end up with an entirely empty [`BusLayout`], suppression
/// included. Suppression left behind without the lanes to honour it would drop
/// those wires at emit and strand their consumers silently.
///
/// The BUS is fully undone; the module is not byte-identical to one laid out
/// before the feature existed. `TAP_RESERVE` has already widened every column
/// and `place_edge_pins` has already shifted the pin stack clear of a band
/// that will not be built, and both happen before the cost is knowable.
/// Nothing is stranded by that — it is spacing, not routing — but the plane is
/// a little wider than it needs to be.
///
/// `rows` is the module's total row count across ALL pages, while a bus is
/// allocated per page: a module of three short pages can clear [`BUS_MIN_ROWS`]
/// without any single page being that deep. `gates` likewise counts every
/// spawnable node, chip bricks and edge pins included.
fn bus_is_worth_its_bricks(rows: usize, groups: usize, gates: usize, nodes: usize) -> bool {
    if rows >= BUS_MIN_ROWS && groups >= BUS_MIN_GROUPS {
        return true;
    }
    nodes <= gates
}

/// The vertical distance a lane must cover to be worth building.
///
/// A lane costs a rerouter out in the gutter and another coming back to the
/// gate, stacked as a pair `LANE_PAIR_HEIGHT` tall. If the value travels less
/// than those bricks span, the lane is taller than the journey it makes: the
/// wire it replaced was already an inline hop, and routing it through the
/// gutter is strictly more bricks over a strictly longer path.
///
/// Measured rather than guessed. Lane extents are sharply bimodal — on a large
/// program 40% of groups travel EXACTLY 0 and the next populated extent is 8,
/// with three groups in between — so any floor in 1..=7 gives the same answer
/// and the exact value is not load-bearing.
const MIN_LANE_TRAVEL: i32 = LANE_PAIR_HEIGHT + 2 * REROUTER_HALF;

/// Vertical step between two runs that would otherwise leave at the same
/// level. One rerouter tall, so staggered runs stack flush — legal, since
/// brick separation is a strict inequality — while still giving each run a
/// line of its own to be drawn on.
const STAGGER_STEP: i32 = 2 * REROUTER_HALF;
/// Space held open in front of every column of a line for the gutter bus's
/// gate-side rerouter — the tap gap plus the rerouter's own footprint.
///
/// Without it a row packs its columns flush and the cell a tap needs is the
/// cell its left-hand neighbour already stands in, so most taps would be
/// left out and their consumers would read from further back along the row
/// instead. Reserving it makes the cell exist by construction, on every
/// column, whether or not the bus ends up using it.
///
/// The cost is paid by every column of every line: a code-layout line runs
/// roughly 30–50% wider than the bricks on it need, since a 5-unit gate
/// column becomes a 10-unit one. That is accepted deliberately — a reserve
/// handed out only where a tap turns out to land cannot be computed before
/// the body is placed, and the body's width is what decides where the taps
/// land. Paying it everywhere is what makes per-column taps land at all.
const TAP_RESERVE: i32 = TAP_GAP + 2 * REROUTER_HALF;

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
        .nodes
        .iter()
        .filter(|(_, n)| is_spawnable(n))
        .collect();

    if spawnable.is_empty() {
        return LayoutResult {
            placements: HashMap::default(),
            chip_layouts: if recurse {
                recurse_chips(module, opts)
            } else {
                HashMap::default()
            },
            annotations: Vec::new(),
            rotations: HashMap::default(),
            bus: BusLayout::default(),
            bounds_min: IntVec3::default(),
            bounds_max: IntVec3::default(),
        };
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
        match adopt_line(id, module, &adjacency, &literal_line) {
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
            (n.source_range.file.clone(), n.source_range.start.offset, *id)
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
            (n.source_range.file.clone(), n.source_range.start.offset, *id)
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
    let mut node_page: HashMap<NodeId, usize> = HashMap::default();
    // The row a body node sits in, named by `(page, sub-row offset)` — the
    // assignment the bus taps into.
    // `page_w`/`page_h` and the `PageInfo`s built here are final: the bus is
    // planned after this and never widens them. See the KNOWN DEVIATION note
    // on [`gutter::plan_bus`] for what that costs.
    let mut node_row: HashMap<NodeId, (usize, i32)> = HashMap::default();
    let mut page_infos: Vec<PageInfo> = Vec::new();
    for (pi, page) in pages.iter().enumerate() {
        let z = Z_PLANE + pi as i32 * PAGE_Z_STEP;
        let mut page_h = 0i32;
        let mut page_w = 0i32;
        for &(id, right, down) in &page.nodes {
            let (hsx, hsy) = cell_half_size(&module.nodes[&id], rotation_of(&rotations, &id));
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
            let (hsx, hsy) = cell_half_size(&module.nodes[&id], rotation_of(&rotations, &id));
            let x = page_h - down - hsx * 2 - page_h / 2;
            let y = right - page_w / 2;
            placements.insert(id, Placement { x, y, z });
            node_page.insert(id, pi);
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

    // The gutter lanes read the body's placement, so they are planned once
    // the body is final. They also claim the band the input-pin stack has to
    // clear, so the plan precedes the edge pins — and the lanes themselves
    // follow, since a lane headed by an edge pin stands at that pin's row.
    let mut plan = plan_bus(module, &placements, &rotations, &node_row, &page_infos);

    place_edge_pins(
        module,
        &adjacency,
        edge_pins,
        &node_page,
        &plan.band_widths,
        &mut page_infos,
        &mut placements,
    );

    // Counted by `lay_bus`, which is where the zero-travel filter runs, so
    // the size override weighs the lanes actually laid rather than the
    // groups that merely applied for one.
    let (mut bus, bussable_groups) = lay_bus(&placements, &page_infos, &mut plan);
    lay_mini_bus(
        &mut bus,
        module,
        &placements,
        &rotations,
        &node_row,
        &node_line,
        &page_infos,
        &mut plan,
    );

    // ...and now decide whether it was worth building. The plan is what makes
    // the cost knowable — the node count is not estimable from the module
    // alone, since a lane's length depends on where the body landed — so the
    // bus is built and then discarded whole if it did not earn its bricks.
    //
    // Discarding is all-or-nothing by construction: replacing the layout drops
    // its nodes, its wires AND its suppression together, which is what leaves
    // the module byte-identical to one laid out before the bus existed.
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
    if !placements.is_empty() || !annotations.is_empty() || !bus.nodes.is_empty() {
        let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for (id, p) in placements.iter() {
            let (hsx, hsy) = cell_half_size(&module.nodes[id], rotation_of(&rotations, id));
            lo_x = lo_x.min(p.x);
            hi_x = hi_x.max(p.x + hsx * 2);
            lo_y = lo_y.min(p.y);
            hi_y = hi_y.max(p.y + hsy * 2);
        }
        for a in &annotations {
            lo_x = lo_x.min(a.x);
            hi_x = hi_x.max(a.x + ANNOTATION_SIZE);
            lo_y = lo_y.min(a.y);
            hi_y = hi_y.max(a.y + ANNOTATION_SIZE);
        }
        for n in &bus.nodes {
            lo_x = lo_x.min(n.x);
            hi_x = hi_x.max(n.x + REROUTER_HALF * 2);
            lo_y = lo_y.min(n.y);
            hi_y = hi_y.max(n.y + REROUTER_HALF * 2);
        }
        let shift_x = -(lo_x + hi_x) / 2;
        let shift_y = -(lo_y + hi_y) / 2;
        if shift_x != 0 || shift_y != 0 {
            for p in placements.values_mut() {
                p.x += shift_x;
                p.y += shift_y;
            }
            for a in annotations.iter_mut() {
                a.x += shift_x;
                a.y += shift_y;
            }
            for n in bus.nodes.iter_mut() {
                n.x += shift_x;
                n.y += shift_y;
            }
        }
    }

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut min_z = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    let mut max_z = i32::MIN;
    for (&id, p) in &placements {
        let (hsx, hsy) = cell_half_size(&module.nodes[&id], rotation_of(&rotations, &id));
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        min_z = min_z.min(p.z);
        max_x = max_x.max(p.x + hsx * 2);
        max_y = max_y.max(p.y + hsy * 2);
        max_z = max_z.max(p.z);
    }
    for a in &annotations {
        min_x = min_x.min(a.x);
        min_y = min_y.min(a.y);
        min_z = min_z.min(a.z);
        max_x = max_x.max(a.x + ANNOTATION_SIZE);
        max_y = max_y.max(a.y + ANNOTATION_SIZE);
        max_z = max_z.max(a.z);
    }
    for n in &bus.nodes {
        min_x = min_x.min(n.x);
        min_y = min_y.min(n.y);
        min_z = min_z.min(n.z);
        max_x = max_x.max(n.x + REROUTER_HALF * 2);
        max_y = max_y.max(n.y + REROUTER_HALF * 2);
        max_z = max_z.max(n.z);
    }

    LayoutResult {
        placements,
        chip_layouts: if recurse {
            recurse_chips(module, opts)
        } else {
            HashMap::default()
        },
        annotations,
        rotations,
        bus,
        bounds_min: IntVec3 {
            x: min_x,
            y: min_y,
            z: min_z,
        },
        bounds_max: IntVec3 {
            x: max_x,
            y: max_y,
            z: max_z,
        },
    }
}

#[cfg(test)]
mod tests;
