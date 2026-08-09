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

/// The source text's own indentation for a 1-based `line`, when a source
/// map is attached and covers that line. This is the statement's indent;
/// the first node on a line sits wherever its subexpression starts (the
/// RHS of a `let`, say), which is why the fallback over-indents.
///
/// The map only describes the entry file. A plane anchored on some other
/// file — an imported chip's body, or a root whose inlined `mod` bodies
/// outnumber its own statements — numbers its lines against that file, so
/// the map's rows would be someone else's; `None` sends those planes to
/// the node-column fallback.
fn source_indent(opts: &LayoutOptions, anchor: &Arc<str>, line: i32) -> Option<u32> {
    let map = opts.source_map.as_ref()?;
    if map.file != *anchor {
        return None;
    }
    let idx = usize::try_from(line - 1).ok()?;
    map.line_indent.get(idx).copied()
}

/// The source-line span a plane covers in `anchor`: the first and last
/// line of the nodes it places literally, plus the spans of every chip
/// nested inside it — a chip's own rows stop where its children's begin,
/// so without the descendants the parent's span would have holes its
/// grandchildren fill. `None` when the plane has no such rows at all (a
/// ports-only chip, or one lowered from another file).
fn line_span(module: &Module, anchor: &Arc<str>) -> Option<(i32, i32)> {
    let mut lo = i32::MAX;
    let mut hi = i32::MIN;
    for n in module.nodes.values() {
        if !is_spawnable(n) || is_edge_pin(n) || !has_range(n) || n.source_range.file != *anchor {
            continue;
        }
        let line = n.source_range.start.line as i32;
        lo = lo.min(line);
        hi = hi.max(line);
    }
    for child in module.chips.values() {
        if let Some((clo, chi)) = line_span(child, anchor) {
            lo = lo.min(clo);
            hi = hi.max(chi);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// Decide, once for the whole tree, which plane renders each own-line comment.
///
/// It cannot be decided locally. [`line_span`] is a min/max ENVELOPE over a
/// module's rows, and those rows are often sparse — a `mod` inlined at two
/// distant call sites, an anon chip partitioned out of a long handler — so a
/// module's window can cover most of the file while it occupies a handful of
/// lines in it. Sibling windows then overlap almost entirely, and since a
/// module's claim excludes only its own CHILDREN, every sibling renders every
/// comment in the overlap. On a real program that turned 951 source comments
/// into 2958 comment bricks.
///
/// The rule keeps the old eligibility test and adds a tie-break, so nothing
/// that already landed on the right plane moves:
///
/// * ELIGIBLE: the planes whose span brackets the line, exactly as before. A
///   plane can only claim a comment inside its own region, so a nested plane
///   can never reach back past its parent for a file's leading note.
/// * CHOSEN: among those, the plane whose next ROW comes soonest after the
///   comment — an own-line comment documents the code that follows it. Ties go
///   to the deeper plane (the more specific one), then to the lower chip id so
///   the answer never depends on map iteration order.
/// * FALLBACK: a comment no plane brackets belongs to the outermost one, which
///   is what keeps a file's leading and trailing notes.
fn assign_comment_owners(
    root: &Module,
    opts: &LayoutOptions,
    anchor: &Arc<str>,
) -> HashMap<i32, Option<NodeId>> {
    let mut out: HashMap<i32, Option<NodeId>> = HashMap::default();
    let Some(map) = opts.source_map.as_ref() else {
        return out;
    };
    if map.comments.is_empty() || map.file != *anchor {
        return out;
    }

    // Every module in the tree with the anchor-file lines it actually
    // occupies — the rows themselves, not their envelope.
    fn walk(
        m: &Module,
        key: Option<NodeId>,
        depth: usize,
        anchor: &Arc<str>,
        out: &mut Vec<(Option<NodeId>, usize, Vec<i32>, Option<(i32, i32)>)>,
    ) {
        let mut lines: Vec<i32> = m
            .nodes
            .values()
            .filter(|n| {
                is_spawnable(n)
                    && !is_edge_pin(n)
                    && has_range(n)
                    && n.source_range.file == *anchor
            })
            .map(|n| n.source_range.start.line as i32)
            .collect();
        lines.sort_unstable();
        lines.dedup();
        out.push((key, depth, lines, line_span(m, anchor)));
        let mut ids: Vec<NodeId> = m.chips.keys().copied().collect();
        ids.sort();
        for id in ids {
            walk(&m.chips[&id], Some(id), depth + 1, anchor, out);
        }
    }
    let mut modules: Vec<(Option<NodeId>, usize, Vec<i32>, Option<(i32, i32)>)> = Vec::new();
    walk(root, None, 0, anchor, &mut modules);

    for c in &map.comments {
        if !c.own_line || c.text.is_empty() || c.in_array {
            continue;
        }
        let line = c.line as i32;
        // (distance to the next row, deeper first, then chip id).
        // Rank: nearest row wins, then the deeper plane, then the lower id.
        let rank = |dist: i32, depth: usize, key: Option<NodeId>| {
            (dist, std::cmp::Reverse(depth), key)
        };
        // Only the planes that bracket the line are in the running.
        let eligible: Vec<&(Option<NodeId>, usize, Vec<i32>, Option<(i32, i32)>)> = modules
            .iter()
            .filter(|(_, _, _, span)| span.is_some_and(|(lo, hi)| line >= lo && line <= hi))
            .collect();
        let mut best: Option<(i32, std::cmp::Reverse<usize>, Option<NodeId>)> = None;
        for (key, depth, lines, _) in &eligible {
            if let Some(&next) = lines.iter().find(|&&l| l > line) {
                let r = rank(next - line, *depth, *key);
                if best.is_none_or(|b| r < b) {
                    best = Some(r);
                }
            }
        }
        if best.is_none() {
            for (key, depth, lines, _) in &eligible {
                if let Some(&prev) = lines.iter().rev().find(|&&l| l < line) {
                    let r = rank(line - prev, *depth, *key);
                    if best.is_none_or(|b| r < b) {
                        best = Some(r);
                    }
                }
            }
        }
        out.insert(line, best.and_then(|(_, _, k)| k));
    }
    out
}

/// The own-line `//` comments this module renders, keyed by source line.
///
/// A comment belongs to the innermost plane whose rows bracket its line, so
/// lines inside a child chip's body are left to that chip. The outermost
/// module additionally claims comments outside every row's span — a file's
/// leading and trailing notes would otherwise be dropped. Trailing comments
/// (code then `//` on the same line) are never claimed: they would need a
/// row the code already occupies.
///
/// Planes anchored on a file other than the map's claim nothing: their rows
/// are lines of a file the map never saw, and bracketing a comment against
/// them would render it a second time on the wrong plane.
fn claimed_comments<'a>(
    module: &Module,
    opts: &'a LayoutOptions,
    anchor: &Arc<str>,
) -> HashMap<i32, &'a SourceComment> {
    let mut out: HashMap<i32, &SourceComment> = HashMap::default();
    let Some(map) = opts.source_map.as_ref() else {
        return out;
    };
    if map.comments.is_empty() || map.file != *anchor {
        return out;
    }
    // Ownership was settled for the whole tree before any plane was laid
    // out, because sibling envelopes overlap and no plane can tell locally
    // whether a comment is really its own.
    if let Some(owners) = opts.comment_owner.as_ref() {
        for c in &map.comments {
            if !c.own_line || c.text.is_empty() || c.in_array {
                continue;
            }
            let line = c.line as i32;
            if owners.get(&line) == Some(&opts.self_chip) {
                out.entry(line).or_insert(c);
            }
        }
        return out;
    }

    // The outermost plane claims by exclusion, so it needs no window of its
    // own; a nested plane with no rows can bracket nothing.
    let window = if opts.nested {
        match line_span(module, anchor) {
            Some(span) => Some(span),
            None => return out,
        }
    } else {
        None
    };
    let child_spans: Vec<(i32, i32)> = module
        .chips
        .values()
        .filter_map(|child| line_span(child, anchor))
        .collect();

    for c in &map.comments {
        if !c.own_line || c.text.is_empty() || c.in_array {
            continue;
        }
        let line = c.line as i32;
        if child_spans.iter().any(|&(lo, hi)| line >= lo && line <= hi) {
            continue;
        }
        let mine = match window {
            Some((lo, hi)) => line >= lo && line <= hi,
            None => true,
        };
        if mine {
            out.entry(line).or_insert(c);
        }
    }
    out
}

fn is_spawnable(node: &Node) -> bool {
    matches!(
        node.kind,
        NodeKind::Gate | NodeKind::Event | NodeKind::Input | NodeKind::Output | NodeKind::Chip
    )
}

/// A node has a usable source range if it carries a real end offset or a
/// non-empty file — synthetic nodes built with `SourceRange::default()`
/// have neither.
fn has_range(n: &Node) -> bool {
    n.source_range.end.offset > 0 || !n.source_range.file.is_empty()
}

/// Every chip I/O port stacks on a plane edge — declared `in`/`out` ports
/// and the lowering pass's synthesized boundary pins alike — so the plane
/// reads like the chip's signature regardless of where the ports were
/// written in the source.
fn is_edge_pin(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Input | NodeKind::Output)
}

/// Order within one edge stack: declared ports first in signature order
/// (every port of a signature shares its declaration offset, so the id —
/// allocated left to right across the signature — is what separates them),
/// then synthesized boundary pins by label.
fn edge_stack_key(n: &Node) -> (u8, usize, String, Option<u64>, NodeId) {
    if has_range(n) {
        (0, n.source_range.start.offset, String::new(), None, n.id)
    } else {
        let (stem, num) = label_sort_key(&port_label(n));
        (1, 0, stem, num, n.id)
    }
}

/// A label split into its non-numeric stem and trailing number, so a stack
/// of `ext1 ext2 … ext10` orders by count rather than by digit — plain
/// string order puts `ext10` between `ext1` and `ext2`.
fn label_sort_key(label: &str) -> (String, Option<u64>) {
    let stem_len = label.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let digits = &label[stem_len..];
    // A run too long for u64 keeps the whole label as its stem, which is
    // still a total order — just the string one.
    match digits.parse::<u64>() {
        Ok(num) => (label[..stem_len].to_string(), Some(num)),
        Err(_) => (label.to_string(), None),
    }
}

fn port_label(n: &Node) -> String {
    match n.properties.get(&*sym::PORT_LABEL) {
        Some(Literal::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The most frequent file among spawnable nodes with a real range, ties
/// broken lexically for determinism.
fn anchor_file(spawnable: &[(&NodeId, &Node)]) -> Arc<str> {
    let mut counts: HashMap<Arc<str>, usize> = HashMap::default();
    for (_, n) in spawnable {
        if has_range(n) {
            *counts.entry(n.source_range.file.clone()).or_insert(0) += 1;
        }
    }
    let mut files: Vec<(Arc<str>, usize)> = counts.into_iter().collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    files
        .into_iter()
        .next()
        .map(|(f, _)| f)
        .unwrap_or_else(|| "".into())
}

struct Adjacency {
    /// node -> nodes that consume its output (wire targets).
    consumers: HashMap<NodeId, Vec<NodeId>>,
    /// node -> nodes that produce into it (wire sources).
    producers: HashMap<NodeId, Vec<NodeId>>,
    /// `producers` minus every wire landing on an exec input — the operand
    /// graph, without the sequencing graph. See [`line_groups`].
    value_producers: HashMap<NodeId, Vec<NodeId>>,
}

/// True when the node has an exec input at all — the node-side reading of
/// the same port test [`targets_exec`] applies to a wire.
fn takes_exec_input(node: &Node) -> bool {
    node.ports.inputs.iter().any(|p| p.ty == Type::Exec)
}

/// True when a node belongs to its line's EXEC SPINE: it takes an exec
/// input AND nothing on its own line reads the value it produces, so it is
/// the statement's sink rather than a step in an expression.
///
/// `in_line_consumers` is the node's in-line value consumers — the entry
/// indices `line_groups` derives from `value_producers` restricted to the
/// line's own entries.
///
/// The second clause is what keeps a variable read out of the spine. An
/// `Exec_Var_Get` inside an expression takes an exec input too, but its
/// value feeds an operator on the same line, so it belongs to the value
/// flow: it stays in the depth columns and stays horizontal. A `Var_Set`,
/// `PrintToConsole` or `ArrayVar_Push` reads nothing back in line and heads
/// its statement on the left.
///
/// The column pin and the rotation decision both read this, so they cannot
/// drift apart.
fn on_exec_spine(node: &Node, in_line_consumers: &[usize]) -> bool {
    takes_exec_input(node) && in_line_consumers.is_empty()
}

/// True for the gates that form a line's exec spine and so face down.
///
/// Deliberately narrower than [`on_exec_spine`] alone: a `Chip` node can
/// also take an exec input, but the microchip shell is emitted through a
/// separate path that hardcodes its 1×1 offsets, and its interior is a
/// distinct grid entity carrying its own transform. Rotating it there
/// would put layout and emit out of step — the one failure this whole
/// mechanism exists to avoid — for a brick with no facing to speak of.
fn is_spine_exec_gate(node: &Node, in_line_consumers: &[usize]) -> bool {
    node.kind == NodeKind::Gate && on_exec_spine(node, in_line_consumers)
}

/// A node's quarter-turn, defaulting to `Deg0` for anything unrotated.
fn rotation_of(rotations: &HashMap<NodeId, NodeRotation>, id: &NodeId) -> NodeRotation {
    rotations.get(id).copied().unwrap_or_default()
}

/// The footprint a node reserves, as `(half extent down the rows, half
/// extent across the columns)` — the same order [`brick_half_size`]
/// returns.
///
/// A `Deg90` brick is turned a quarter-turn within the plane, so its two
/// half-sizes swap. EVERY site that measures a cell must go through this
/// rather than `brick_half_size`, and emit must apply the same swap when
/// it centers the brick: `brdb::Brick::local_bounds()` is rotation-blind,
/// so a disagreement is invisible to the overlap checker and shows up
/// only as bricks silently dropped by the game at load.
fn cell_half_size(node: &Node, rotation: NodeRotation) -> (i32, i32) {
    let (hsx, hsy) = brick_half_size(node);
    match rotation {
        // Only the QUARTER turns swap. A half turn lands the brick the way
        // round it started, so Deg180 reserves exactly the Deg0 cell and
        // Deg270 exactly the Deg90 one.
        NodeRotation::Deg0 | NodeRotation::Deg180 => (hsx, hsy),
        NodeRotation::Deg90 | NodeRotation::Deg270 => (hsy, hsx),
    }
}

/// True when `target` names an exec input on its node — an exec-chain edge
/// rather than an operand edge.
fn targets_exec(module: &Module, target: &PortRef) -> bool {
    module
        .nodes
        .get(&target.node_id)
        .is_some_and(|n| {
            n.ports.inputs.iter().any(|p| {
                p.ty == Type::Exec && crate::intern::resolve(p.name) == target.port.as_str()
            })
        })
}

fn build_adjacency(module: &Module) -> Adjacency {
    let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    let mut producers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    let mut value_producers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    for w in &module.wires {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        consumers
            .entry(w.source.node_id)
            .or_default()
            .push(w.target.node_id);
        producers
            .entry(w.target.node_id)
            .or_default()
            .push(w.source.node_id);
        if !targets_exec(module, &w.target) {
            value_producers
                .entry(w.target.node_id)
                .or_default()
                .push(w.source.node_id);
        }
    }
    Adjacency {
        consumers,
        producers,
        value_producers,
    }
}

/// BFS from a homeless node over the wire graph — consumers before
/// producers, both sorted by `(start.offset, id)` — stopping at the
/// first node with a literal line. Traverses through other homeless
/// nodes transitively; a visited set guards against cycles.
fn adopt_line(
    start: NodeId,
    module: &Module,
    adjacency: &Adjacency,
    literal_line: &HashMap<NodeId, i32>,
) -> Option<i32> {
    let offset_of = |id: &NodeId| {
        module
            .nodes
            .get(id)
            .map(|n| n.source_range.start.offset)
            .unwrap_or(usize::MAX)
    };

    let mut visited: HashSet<NodeId> = HashSet::default();
    visited.insert(start);
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    queue.push_back(start);

    while let Some(cur) = queue.pop_front() {
        let mut neighbors: Vec<NodeId> = Vec::new();
        if let Some(cs) = adjacency.consumers.get(&cur) {
            let mut v = cs.clone();
            v.sort_by_key(|id| (offset_of(id), *id));
            neighbors.extend(v);
        }
        if let Some(ps) = adjacency.producers.get(&cur) {
            let mut v = ps.clone();
            v.sort_by_key(|id| (offset_of(id), *id));
            neighbors.extend(v);
        }
        for nb in neighbors {
            if !visited.insert(nb) {
                continue;
            }
            if let Some(&line) = literal_line.get(&nb) {
                return Some(line);
            }
            queue.push_back(nb);
        }
    }
    None
}

struct SubRow {
    height: i32,
    entries: Vec<(NodeId, i32)>,
}

/// One connected run of a line's nodes, arranged as a column per
/// dependency depth: the nodes feeding a column sit in the column to its
/// left, and a column's own nodes stack into rows.
struct LineGroup {
    width: i32,
    /// `(node, x offset inside the group, row inside the group)`.
    nodes: Vec<(NodeId, i32, usize)>,
}

/// Split a line's entries into connected groups, each arranged as
/// columns.
///
/// Column 0 is the exec spine: every node [`on_exec_spine`] accepts is
/// PINNED there, whatever its position in the value graph. A statement's
/// sink — the `Var_Set` under an expression, say — therefore heads its
/// line on the left and takes its value from the right. An exec gate whose
/// value IS read on the line, such as a variable read inside an
/// expression, is not a sink and stays in the value columns.
///
/// Everything else is a value node, columned by dependency depth. The
/// depth graph runs along the wire, from a node to what it feeds, so a
/// value node's depth is the longest path from a source and it always
/// lands right of everything feeding it: operands flow left to right into
/// the consumer that reads them. Those columns start one step right of the
/// pinned column, so a value node never shares column 0 with the spine.
/// Only wires with both endpoints on this line take part, so a line with
/// no nesting comes back as a run of single-node groups: the flat
/// left-to-right row.
///
/// Exec wires are left out of the depth graph. They run from a statement
/// to the one after it, so treating them as operand edges would chain a
/// multi-statement line into one group and push each statement a column
/// right of the last. Without them, sequenced statements are separate
/// groups and keep source order, while their operands still tree out to
/// the right.
///
/// The rotation decision is made here, once the columns are known: a gate
/// on the spine faces down. The record happens before the column is
/// measured, so the swapped footprint is what gets reserved.
fn line_groups(
    entries: &[NodeId],
    module: &Module,
    adjacency: &Adjacency,
    rotations: &mut HashMap<NodeId, NodeRotation>,
) -> Vec<LineGroup> {
    let n = entries.len();
    let ord: HashMap<NodeId, usize> = entries.iter().enumerate().map(|(i, id)| (*id, i)).collect();

    // `operands[i]`: the nodes feeding entry i — its depth predecessors.
    // `consumers[i]`: the nodes entry i feeds, which sit right of it.
    let mut operands: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut seen: HashSet<(usize, usize)> = HashSet::default();
    for (ci, id) in entries.iter().enumerate() {
        for p in adjacency.value_producers.get(id).into_iter().flatten() {
            let Some(&pi) = ord.get(p) else { continue };
            if pi == ci || !seen.insert((ci, pi)) {
                continue;
            }
            operands[ci].push(pi);
            consumers[pi].push(ci);
        }
    }

    // Longest-path depth over that graph, swept in entry order.
    let mut waiting: Vec<usize> = operands.iter().map(Vec::len).collect();
    let mut depth: Vec<i32> = vec![0; n];
    let mut placed: Vec<bool> = vec![false; n];
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| waiting[i] == 0).collect();
    let mut done = 0usize;
    while done < n {
        let i = match ready.iter().next().copied() {
            Some(i) => i,
            // A cycle among a line's own wires would stall the sweep. Break
            // it at the earliest node still waiting so the pass terminates;
            // the edges still holding that node back are ignored for depth.
            None => (0..n).find(|&i| !placed[i]).expect("a node is unplaced"),
        };
        ready.remove(&i);
        let d = operands[i]
            .iter()
            .filter(|&&o| placed[o])
            .map(|&o| depth[o] + 1)
            .max()
            .unwrap_or(0);
        depth[i] = d;
        placed[i] = true;
        done += 1;
        for &c in &consumers[i] {
            if placed[c] {
                continue;
            }
            waiting[c] -= 1;
            if waiting[c] == 0 {
                ready.insert(c);
            }
        }
    }

    // Depth becomes a column: the exec spine is pinned to column 0 and
    // every value node sits one step right of it, ordered by its own
    // depth. A spine node's depth is discarded — it is a sink in the value
    // graph and would otherwise land right of the operands it reads.
    let column: Vec<usize> = (0..n)
        .map(|i| {
            if on_exec_spine(&module.nodes[&entries[i]], &consumers[i]) {
                0
            } else {
                depth[i] as usize + 1
            }
        })
        .collect();

    // Connected groups, so unrelated runs on the same line never share a
    // column and can pack side by side instead.
    let mut root: Vec<usize> = (0..n).collect();
    for (i, ops) in operands.iter().enumerate() {
        for &o in ops {
            let (a, b) = (uf_find(&mut root, i), uf_find(&mut root, o));
            if a != b {
                root[a.max(b)] = a.min(b);
            }
        }
    }
    let mut buckets: HashMap<usize, Vec<usize>> = HashMap::default();
    for i in 0..n {
        buckets.entry(uf_find(&mut root, i)).or_default().push(i);
    }
    let mut members: Vec<Vec<usize>> = buckets.into_values().collect();
    members.sort_by_key(|m| m[0]);

    let mut row_of: Vec<usize> = vec![0; n];
    let mut groups: Vec<LineGroup> = Vec::with_capacity(members.len());
    for m in members {
        let deepest = m.iter().map(|&i| column[i]).max().unwrap_or(0);
        let mut cols: Vec<Vec<usize>> = vec![Vec::new(); deepest + 1];
        for &i in &m {
            cols[column[i]].push(i);
        }
        let mut nodes = Vec::with_capacity(m.len());

        // Pass 1 — the horizontal frame, in column order. Column 0 keeps the
        // group's left edge whether or not its nodes end up on the top row,
        // so the drop below is purely vertical and the spine stays a straight
        // column down the left margin.
        //
        // A group with no spine node leaves column 0 empty; an empty column
        // claims nothing, so its value nodes still start flush at the group's
        // left edge — bar the tap reserve every occupied column opens in
        // front of itself.
        let mut x = 0i32;
        let mut col_x: Vec<i32> = vec![0; cols.len()];
        for (col_idx, col) in cols.iter().enumerate() {
            if col.is_empty() {
                continue;
            }
            x += TAP_RESERVE;
            col_x[col_idx] = x;
            let mut col_w = 0i32;
            for &i in col.iter() {
                let node = &module.nodes[&entries[i]];
                let rot = if col_idx == 0 && is_spine_exec_gate(node, &consumers[i]) {
                    rotations.insert(entries[i], NodeRotation::Deg90);
                    NodeRotation::Deg90
                } else {
                    NodeRotation::Deg0
                };
                col_w = col_w.max(cell_half_size(node, rot).1 * 2);
            }
            x += col_w;
        }

        // Pass 2 — the value columns take the group's UPPER rows, filling
        // left to right. A node follows the row of the operands feeding it,
        // so a subtree stays together rather than interleaving with its
        // siblings'; those operands sit in a column already filled.
        let mut value_rows = 0usize;
        for (col_idx, col) in cols.iter_mut().enumerate().skip(1) {
            if col.is_empty() {
                continue;
            }
            col.sort_by_key(|&i| (operands[i].iter().map(|&o| row_of[o]).min().unwrap_or(0), i));
            for (row, &i) in col.iter().enumerate() {
                row_of[i] = row;
                nodes.push((entries[i], col_x[col_idx], row));
            }
            value_rows = value_rows.max(col.len());
        }

        // Pass 3 — and the statement's own gates drop BELOW all of it, still
        // in column 0. Values flow left to right into a sink, so a sink level
        // with its operands is read by a run coming back leftward across the
        // line; from the bottom row the same run is a descent.
        //
        // `value_rows` is 0 for a group that is nothing but its statement —
        // a call whose arguments all inlined, say — so that sink stays on the
        // top row and the line does not grow a row for nothing. A spine node
        // reads nothing back in line (that is what `on_exec_spine` means), so
        // it never appears as another node's operand and no value column's
        // row ordering depends on where it lands.
        cols[0].sort_unstable();
        for (k, &i) in cols[0].iter().enumerate() {
            let row = value_rows + k;
            row_of[i] = row;
            nodes.push((entries[i], col_x[0], row));
        }
        groups.push(LineGroup { width: x, nodes });
    }
    groups
}

fn uf_find(root: &mut [usize], mut x: usize) -> usize {
    while root[x] != x {
        root[x] = root[root[x]];
        x = root[x];
    }
    x
}

/// Lay out one line's groups left to right, soft-wrapping into a new
/// indented band of sub-rows whenever the next group would push the
/// current band past `budgets.line_width`.
fn measure_line(
    entries: &[NodeId],
    module: &Module,
    adjacency: &Adjacency,
    budgets: &CodeBudgets,
    head_col: u32,
    rotations: &mut HashMap<NodeId, NodeRotation>,
) -> Vec<SubRow> {
    let indent_px = head_col as i32 * INDENT_UNIT;
    let mut rows: Vec<Vec<(NodeId, i32)>> = Vec::new();
    let mut cursor = indent_px;
    let mut band_base = 0usize;
    let mut band_has_entry = false;

    for group in line_groups(entries, module, adjacency, rotations) {
        if band_has_entry && cursor + group.width - indent_px > budgets.line_width {
            band_base = rows.len();
            cursor = indent_px + CONTINUATION_INDENT;
        }
        for (id, dx, row) in group.nodes {
            let row = band_base + row;
            while rows.len() <= row {
                rows.push(Vec::new());
            }
            rows[row].push((id, cursor + dx));
        }
        cursor += group.width;
        band_has_entry = true;
    }

    rows.into_iter()
        .map(|entries| {
            // Every group on this line has already recorded its rotations,
            // so a rotated gate contributes its swapped height here.
            let height = entries
                .iter()
                .map(|(id, _)| cell_half_size(&module.nodes[id], rotation_of(rotations, id)).0 * 2)
                .max()
                .unwrap_or(0);
            SubRow { height, entries }
        })
        .collect()
}

/// One measured source line: its blank-run gap, total height across
/// sub-rows, width (max x extent incl. indent), and per-node offsets
/// relative to the line's own top-left. `anns` holds the same offsets for
/// the line's comment label, keyed by index into the layout's text list.
struct LinePlan {
    gap_before: i32,
    height: i32,
    width: i32,
    nodes: Vec<(NodeId, i32, i32)>,
    anns: Vec<(usize, i32, i32)>,
}

fn plan_line(
    entries: &[NodeId],
    module: &Module,
    adjacency: &Adjacency,
    budgets: &CodeBudgets,
    head_col: u32,
    gap_before: i32,
    annotation: Option<usize>,
    rotations: &mut HashMap<NodeId, NodeRotation>,
) -> LinePlan {
    let subrows = measure_line(entries, module, adjacency, budgets, head_col, rotations);
    let mut nodes = Vec::new();
    let mut height = 0i32;
    let mut width = 0i32;
    for subrow in &subrows {
        for &(id, x) in &subrow.entries {
            let (_, hsy) = cell_half_size(&module.nodes[&id], rotation_of(rotations, &id));
            nodes.push((id, x, height));
            width = width.max(x + hsy * 2);
        }
        height += subrow.height;
    }
    // The label starts at the line's own indent and takes a sub-row of its
    // own below any gates, so it never lands on top of one.
    let mut anns = Vec::new();
    if let Some(idx) = annotation {
        let x = head_col as i32 * INDENT_UNIT;
        anns.push((idx, x, height));
        height += ANNOTATION_SIZE;
        width = width.max(x + ANNOTATION_SIZE);
    }
    LinePlan {
        gap_before,
        height,
        width,
        nodes,
        anns,
    }
}

/// A vertical stack of lines capped at `band_height`; per-node offsets
/// relative to the band's top-left.
struct Band {
    height: i32,
    width: i32,
    nodes: Vec<(NodeId, i32, i32)>,
    anns: Vec<(usize, i32, i32)>,
}

impl Band {
    fn new() -> Self {
        Band {
            height: 0,
            width: 0,
            nodes: Vec::new(),
            anns: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.anns.is_empty()
    }
}

/// Stack lines into bands greedily: a line whose bottom would exceed the
/// band budget closes the band (bands never split a line). Blank-run
/// gaps only count inside a band — a line opening a new band starts at
/// its top.
fn assemble_bands(lines: Vec<LinePlan>, budgets: &CodeBudgets) -> Vec<Band> {
    let mut bands: Vec<Band> = Vec::new();
    let mut band = Band::new();
    for line in lines {
        let mut gap = if band.is_empty() { 0 } else { line.gap_before };
        if !band.is_empty() && band.height + gap + line.height > budgets.band_height {
            bands.push(std::mem::replace(&mut band, Band::new()));
            gap = 0;
        }
        let base = band.height + gap;
        for (id, x, down) in line.nodes {
            band.nodes.push((id, x, base + down));
        }
        for (idx, x, down) in line.anns {
            band.anns.push((idx, x, base + down));
        }
        band.height = base + line.height;
        band.width = band.width.max(line.width);
    }
    if !band.is_empty() {
        bands.push(band);
    }
    bands
}

/// One page's worth of bands placed left→right with `BAND_GUTTER` gaps;
/// per-node offsets relative to the page's top-left.
struct PagePlan {
    width: i32,
    nodes: Vec<(NodeId, i32, i32)>,
    anns: Vec<(usize, i32, i32)>,
}

impl PagePlan {
    fn new() -> Self {
        PagePlan {
            width: 0,
            nodes: Vec::new(),
            anns: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.anns.is_empty()
    }
}

/// Pack bands into pages greedily: a band whose right edge would exceed
/// the plane budget closes the page.
fn assemble_pages(bands: Vec<Band>, budgets: &CodeBudgets) -> Vec<PagePlan> {
    let mut pages: Vec<PagePlan> = Vec::new();
    let mut page = PagePlan::new();
    for band in bands {
        let mut gutter = if page.is_empty() { 0 } else { BAND_GUTTER };
        if !page.is_empty() && page.width + gutter + band.width > budgets.plane_width {
            pages.push(std::mem::replace(&mut page, PagePlan::new()));
            gutter = 0;
        }
        let origin = page.width + gutter;
        for (id, x, down) in band.nodes {
            page.nodes.push((id, origin + x, down));
        }
        for (idx, x, down) in band.anns {
            page.anns.push((idx, origin + x, down));
        }
        page.width = origin + band.width;
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// Post-centering extents of one emitted page, for edge-pin placement.
struct PageInfo {
    min_y: i32,
    max_y: i32,
    top_x: i32,
    z: i32,
}

/// Stack every chip I/O port on its page's edges: Inputs one gutter left
/// of the page body's left edge, Outputs one gutter right of its right
/// edge. Each stack starts at the page's top row and descends one port
/// height at a time, in `edge_stack_key` order, so the column reads like
/// the chip's signature. A port lands on the page its first placed
/// neighbor occupies (page 0 when it has none), which keeps ports beside
/// their own body on paginated layouts.
///
/// `band_widths` is the gutter bus band already claimed on each page. The
/// input stack clears it, so the left edge reads input pins, then bus
/// lanes, then the body.
fn place_edge_pins(
    module: &Module,
    adjacency: &Adjacency,
    mut pins: Vec<NodeId>,
    node_page: &HashMap<NodeId, usize>,
    band_widths: &[i32],
    page_infos: &mut Vec<PageInfo>,
    placements: &mut HashMap<NodeId, Placement>,
) {
    if pins.is_empty() {
        return;
    }
    pins.sort_by_key(|id| edge_stack_key(&module.nodes[id]));
    if page_infos.is_empty() {
        page_infos.push(PageInfo {
            min_y: 0,
            max_y: 0,
            top_x: 0,
            z: Z_PLANE,
        });
    }

    let offset_of = |id: &NodeId| {
        module
            .nodes
            .get(id)
            .map(|n| n.source_range.start.offset)
            .unwrap_or(usize::MAX)
    };

    // Descent cursor per (page, edge): the x of the last port placed there.
    let mut cursors: HashMap<(usize, bool), i32> = HashMap::default();
    for pin_id in pins {
        let node = &module.nodes[&pin_id];
        let is_output = node.kind == NodeKind::Output;
        let mut neighbors: Vec<NodeId> = if is_output {
            adjacency.producers.get(&pin_id).cloned()
        } else {
            adjacency.consumers.get(&pin_id).cloned()
        }
        .unwrap_or_default();
        neighbors.sort_by_key(|id| (offset_of(id), *id));

        let page_idx = neighbors
            .iter()
            .find(|id| node_page.contains_key(id))
            .map(|id| node_page[id])
            .unwrap_or(0);
        let info = &page_infos[page_idx];

        // Edge pins never join a line group, so nothing records a rotation
        // for them; the measurement still routes through `cell_half_size`
        // so this stays the one contract for a placed cell's footprint.
        let (hsx, hsy) = cell_half_size(node, NodeRotation::Deg0);
        let pin_h = hsx * 2;
        let pin_w = hsy * 2;
        let y = if is_output {
            info.max_y + BAND_GUTTER
        } else {
            info.min_y - BAND_GUTTER - pin_w - band_widths.get(page_idx).copied().unwrap_or(0)
        };

        // First port of a stack sits on the page's top row; each later one
        // drops by its own height.
        let cursor = cursors
            .entry((page_idx, is_output))
            .or_insert(info.top_x + pin_h);
        *cursor -= pin_h;
        placements.insert(
            pin_id,
            Placement {
                x: *cursor,
                y,
                z: info.z,
            },
        );
    }
}

/// One module wire a lane replaces, and where its target sits.
struct BusTap {
    /// The port the replaced wire lands on.
    target: PortRef,
    /// Row index within the page, ascending top-down.
    row: usize,
    /// The target's placement, in the pre-recenter body space.
    x: i32,
    y: i32,
}

/// Every wire leaving one source port for one page's rows.
struct BusGroup {
    source: PortRef,
    /// The brick this module sees the value come out of: the source node
    /// itself, or the chip it leaves when the source pin lives in a child
    /// module. The lane head stands beside this one.
    source_anchor: NodeId,
    /// The row that anchor stands in on this page, where it stands in one.
    /// The source-side rerouter claims a cell on that row, so a producer
    /// without one — an edge pin, placed later and stacked on the far side
    /// of the band — has no cell to claim and takes no such brick.
    source_row: Option<usize>,
    page: usize,
    taps: Vec<BusTap>,
    /// The value crosses a chip wall — it leaves one, or some tap delivers
    /// into one — rather than running gate to gate across this body.
    crosses_chip: bool,
    /// Position of the group's first wire in `module.wires` — a stable
    /// tiebreak for lane allocation.
    order: usize,
}

/// True for the values a lane is worth building whatever their fan-out: a
/// chip input, persistent storage, or a synthesized boundary pin. Every
/// other value earns a lane only by reaching more than one row.
fn is_bus_worthy_source(n: &Node) -> bool {
    n.kind == NodeKind::Input
        || n.note == Some("boundary_pin")
        || matches!(
            n.gate_class,
            gate_class::PSEUDO_VAR
                | gate_class::PSEUDO_ARRAY_VAR
                | gate_class::BUFFER
                | gate_class::BUFFER_TICKS
        )
}

/// Emit drops every wire touching these classes — a literal is inlined into
/// its target's data, and a rerouter holds no data — so a lane must not
/// claim one.


/// Every node standing inside a chip nested under `module`, mapped to the
/// chip node that represents it HERE.
///
/// A wire out of this module's body into a chip targets a boundary pin in
/// the CHILD module, which holds no row of its own here. The chip brick the
/// value enters through does, so that is what the wire's tap anchors on.
/// Node ids are unique to one module, so the walk order cannot change the
/// result.
fn chip_owners(module: &Module) -> ChipSubtree {
    let mut out = ChipSubtree::default();
    let mut stack: Vec<(NodeId, &Module)> = module.chips.iter().map(|(id, c)| (*id, c)).collect();
    while let Some((chip_id, child)) = stack.pop() {
        for (id, n) in &child.nodes {
            out.owner.insert(*id, chip_id);
            out.class.insert(*id, n.gate_class);
        }
        for grandchild in child.chips.values() {
            stack.push((chip_id, grandchild));
        }
    }
    out
}

/// The chip subtree under a module, indexed two ways.
///
/// `owner` is the chip brick THIS module sees a foreign node through — what a
/// wire's tap anchors on. `class` is that node's real gate class, which the
/// anchor cannot stand in for: a chip node is always bussable, so checking the
/// anchor's class says nothing about whether the endpoint is a literal.
#[derive(Default)]
struct ChipSubtree {
    owner: HashMap<NodeId, NodeId>,
    class: HashMap<NodeId, &'static str>,
}

impl ChipSubtree {
    /// True when a node may carry a bus wire — it is neither inlined into its
    /// consumer's data (a literal) nor unrepresented (unsupported). Resolves
    /// through the subtree, so a FOREIGN endpoint is judged on its own class
    /// rather than on the chip brick it is reached through.
    ///
    /// A node this module has never heard of is not bussable: emit gives it no
    /// brick, so a lane claiming it would leave pass 3b with nothing to wire.
    fn is_bussable(&self, module: &Module, id: NodeId) -> bool {
        let class = match module.nodes.get(&id) {
            Some(n) => Some(n.gate_class),
            None => self.class.get(&id).copied(),
        };
        class.is_some_and(|c| c != gate_class::LITERAL && c != gate_class::UNSUPPORTED)
    }
}

/// Group the module's value wires by `(page, source port)`.
///
/// Exec edges are left out only when they are SPINE SEQUENCING — one body
/// brick handing the chain to the next. That sequences adjacent statements
/// rather than fanning a value across rows, and re-routing it through the
/// gutter would take the spine off its own line.
///
/// Everything else carrying exec is a DELIVERY and buses like any value: a
/// wire into a chip (a crossing out of this body), and a wire out of an
/// input port or a boundary pin (a crossing INTO the body, from a brick
/// stacked on the page's edge). The test is on the wire's source, not on
/// its target's port type.
fn collect_bus_groups(
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    node_row: &HashMap<NodeId, (usize, i32)>,
    row_index: &HashMap<(usize, i32), usize>,
    owners: &ChipSubtree,
) -> Vec<BusGroup> {
    let mut index: HashMap<(usize, PortRef), usize> = HashMap::default();
    let mut groups: Vec<BusGroup> = Vec::new();
    for (order, w) in module.wires.iter().enumerate() {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        if w.source.node_id == w.target.node_id {
            continue;
        }
        // The brick this wire lands on AS THIS MODULE SEES IT: the target
        // itself for a wire staying in the body, or the chip it enters for a
        // wire delivering into one. Only the anchor holds a row and a
        // placement here; the wire's own target is untouched, and emit
        // resolves it through the chip's port index either way.
        let anchor = match module.nodes.get(&w.target.node_id) {
            Some(_) => w.target.node_id,
            None => match owners.owner.get(&w.target.node_id) {
                Some(&chip_id) => chip_id,
                None => continue,
            },
        };
        let into_chip = anchor != w.target.node_id
            || module
                .nodes
                .get(&anchor)
                .is_some_and(|n| n.kind == NodeKind::Chip);
        // The mirror of the anchor above: a value read back out of a chip is
        // produced by a pin in the CHILD module, so the chip it leaves is the
        // brick this module sees it come from.
        let source_anchor = match module.nodes.get(&w.source.node_id) {
            Some(_) => w.source.node_id,
            None => match owners.owner.get(&w.source.node_id) {
                Some(&chip_id) => chip_id,
                None => continue,
            },
        };
        let out_of_chip = source_anchor != w.source.node_id
            || module
                .nodes
                .get(&source_anchor)
                .is_some_and(|n| n.kind == NodeKind::Chip);
        // An exec edge stays direct only when it is genuine SPINE SEQUENCING:
        // one body brick handing the chain to the next, which has to stay a
        // short wire on its own line. That is decided by the SOURCE, not by
        // the target's port type.
        //
        // A wire out of an input PORT is not sequencing — it is a DELIVERY
        // into the body, the same shape as a delivery into a chip. The port
        // stacks on the page's left edge, so its direct wire is exactly the
        // long diagonal across the whole body that the gutter exists to
        // remove.
        //
        // `is_bus_worthy_source` is the one spelling of "a source worth a lane
        // on its own merit", shared with the single-row filter in `plan_bus`.
        // Its storage clauses are inert on an exec wire — no variable or
        // buffer gate has an exec output — so what it selects here is the
        // declared input ports and the synthesized boundary pins.
        //
        // A chip EXIT is deliberately NOT in that set: `source_anchor` is the
        // chip brick, an ordinary body brick of this module carrying the spine
        // on to the next statement, so exec handed back out of a chip keeps
        // its direct wire.
        let source_delivers = module
            .nodes
            .get(&source_anchor)
            .is_some_and(is_bus_worthy_source);
        if !into_chip && !source_delivers && targets_exec(module, &w.target) {
            continue;
        }
        // Both the anchors (the bricks this module places) and the REAL
        // endpoints (which may live in a child chip). Checking only the anchor
        // passes every foreign literal, because a chip node is always bussable
        // — and emit gives a literal no brick, so the lane's wire would fail to
        // resolve and abort the build.
        if !owners.is_bussable(module, source_anchor)
            || !owners.is_bussable(module, anchor)
            || !owners.is_bussable(module, w.source.node_id)
            || !owners.is_bussable(module, w.target.node_id)
        {
            continue;
        }
        // The source has to be a brick of its own for the lane head to read
        // from; edge pins qualify even though they are placed later.
        if !module.nodes.get(&source_anchor).is_some_and(is_spawnable) {
            continue;
        }
        // Only body nodes hold a row; an edge-pin consumer keeps its own
        // direct wire.
        let Some(&(page, down)) = node_row.get(&anchor) else {
            continue;
        };
        let (Some(p), Some(&row)) = (placements.get(&anchor), row_index.get(&(page, down))) else {
            continue;
        };
        let gi = match index.get(&(page, w.source)) {
            Some(&gi) => gi,
            None => {
                // Same page only: a row index names a row of ONE page's own
                // key set, and the lane is laid in this page's coordinates.
                let source_row = node_row
                    .get(&source_anchor)
                    .filter(|&&(sp, _)| sp == page)
                    .and_then(|&(sp, down)| row_index.get(&(sp, down)).copied());
                groups.push(BusGroup {
                    source: w.source,
                    source_anchor,
                    source_row,
                    page,
                    taps: Vec::new(),
                    crosses_chip: out_of_chip,
                    order,
                });
                index.insert((page, w.source), groups.len() - 1);
                groups.len() - 1
            }
        };
        groups[gi].crosses_chip |= into_chip || out_of_chip;
        groups[gi].taps.push(BusTap {
            target: w.target,
            row,
            x: p.x,
            y: p.y,
        });
    }
    groups
}

/// A lane brick is identified by the colour it mirrors, not by a name.
fn push_bus_node(
    bus: &mut BusLayout,
    x: i32,
    y: i32,
    z: i32,
    rotation: NodeRotation,
    role: BusRole,
    color_of: Option<NodeId>,
) -> BusNodeId {
    bus.nodes.push(BusNode {
        x,
        y,
        z,
        rotation,
        role,
        color_of,
    });
    bus.nodes.len() - 1
}

/// An axis-aligned cell in the plane, as `(x0, x1, y0, y1)`.
type Cell = (i32, i32, i32, i32);

fn cells_overlap(a: Cell, b: Cell) -> bool {
    a.1 > b.0 && b.1 > a.0 && a.3 > b.2 && b.3 > a.2
}

/// The `x` spans already standing in each `(page, lane)` column. A lane is
/// one brick wide, so a column collision is a plain interval overlap.
type LaneCells = HashMap<(usize, usize), Vec<(i32, i32)>>;

/// Claim `[lo, hi)` of a lane's column, or report it taken. Bricks that
/// merely touch are legal, so the test is a strict overlap.
fn claim_column(cells: &mut LaneCells, key: (usize, usize), lo: i32, hi: i32) -> bool {
    let taken = cells.entry(key).or_default();
    if taken.iter().any(|&(a, b)| hi > a && b > lo) {
        return false;
    }
    taken.push((lo, hi));
    true
}

/// Where a lane stops on its way down: the rows it taps, each with the `x`
/// its tap rerouter — and the gate-side rerouters that tap rerouter feeds —
/// stands at. That `x` is the row's own first consumer, so the run out to
/// the gates stays on the row it serves.
///
/// Consecutive rows landing inside one rerouter's footprint repeat an `x`,
/// which shares a stop instead of stacking two bricks. Within one band this
/// cannot fire: `assemble_bands` spaces consecutive sub-rows by the taller
/// row's own height, so a row's tap `x` is lower than the row above it by at
/// least the lower row's brick height. It is here because a page may carry
/// several bands, and `assemble_pages` leaves each band's sub-row offsets
/// starting from zero, so offsets from different bands interleave on one
/// page.
fn lane_stops(group: &BusGroup, demand: &BusDemand) -> Vec<(usize, i32)> {
    let mut stops: Vec<(usize, i32)> = Vec::new();
    for &row in &demand.rows {
        let Some(first) = group
            .taps
            .iter()
            .filter(|t| t.row == row)
            .min_by_key(|t| (t.y, t.x, t.target.node_id, t.target.port.as_str()))
        else {
            continue;
        };
        match stops.last() {
            Some(&(_, x)) if (first.x - x).abs() < 2 * REROUTER_HALF => stops.push((row, x)),
            _ => stops.push((row, first.x)),
        }
    }
    stops
}

/// Lay one value's lane: a left-pointing rerouter beside the producer, a head
/// rerouter out in the lane's column level with it, then a pair of rerouters
/// at every stop down that column — a right-pointing tap standing on the row's
/// own level, running out to the gate-side rerouters that feed that row's
/// consumers, and a down-pointing link stacked on top of it carrying the chain
/// on.
///
/// The whole run reads as right angles: the value turns out of its producer
/// beside the gate that makes it, crosses to the band on that gate's own
/// level, drops straight down one column, turns right once inside the band,
/// and crosses to the gates on the consuming row's level.
///
/// The lane itself runs through the down-pointing links and nothing else. A
/// tap is a BRANCH: its link fans out to it, it carries the value right to
/// that one row, and it stops there — it never hands the lane on to the next
/// stop. So every `Deg90` node is driven by another `Deg90` node or by the
/// real port that heads the lane, and no `Deg0` node ever drives a `Deg90`
/// one.
///
/// Every rerouter takes exactly one inbound wire — the previous link in its
/// chain, its link for a tap, or the source-side rerouter (failing that, the
/// value's source itself) for the head — and each consumer port takes exactly
/// one, from the tap beside it. Fanning one link's output out to both the next
/// link and its own branch is legal and is what keeps that true.
///
/// The source-side rerouter is the one brick standing OUTSIDE the chain on the
/// producer's side. It feeds the head and nothing else, so the chain still
/// begins at that head: a lane's head may be driven from outside it, and every
/// other node of a lane is driven by a node of the same lane.
///
/// Nothing here ever moves a tap off its stop: the game drops overlapping
/// bricks silently, so a brick that does not fit is left out instead.
/// `line_groups` opens a `TAP_RESERVE` in front of every column of a line, so
/// the cell a gate-side rerouter wants is normally there for the taking. What
/// it cannot reserve is a SECOND one: a gate reading two bussed values at once
/// has two lanes wanting that one cell, and the lane that arrives later leaves
/// its rerouter out and hangs its consumers off whichever rerouter precedes it
/// along the row — a longer run, and the same value. `row_cells` is what holds
/// the cells, gates and rerouters alike.
///
/// The head and the stop links are the bricks the band adds beyond the tap
/// column, and `lane_cells` drops either of those if its column cell is held;
/// a tap's own cell is reserved before any of them is offered one.
///
/// Lane 0 is the LEFTMOST column, so the widest-span and hottest values —
/// the ones `allocate_lanes` hands the low indices — hold the outside of the
/// band and the short-lived ones churn nearest the body.
#[allow(clippy::too_many_arguments)]
fn emit_lane(
    bus: &mut BusLayout,
    placements: &HashMap<NodeId, Placement>,
    group: &BusGroup,
    demand: &BusDemand,
    stops: &[(usize, i32)],
    lane: usize,
    lanes_used: usize,
    page: usize,
    body_min_y: i32,
    z: i32,
    row_cells: &mut HashMap<(usize, usize), Vec<Cell>>,
    lane_cells: &mut LaneCells,
) {
    let lane_y = body_min_y - BUS_BAND_GUTTER - (lanes_used - lane) as i32 * LANE_PITCH;
    // Colour and head position both read the anchor, not the port: a value
    // read back out of a chip is produced in the child module, which this
    // module cannot place or colour from.
    let color_of = Some(group.source_anchor);

    // Which way this lane travels. Every tap above the source means the value
    // really does run upward out of its producer, and the chain should say so
    // rather than pointing down a run that goes up.
    //
    // Only the CHAIN turns. A tap and the gate-side rerouters it drives still
    // hand the value rightward into the gate beside them whichever way the
    // lane arrived, so flipping the whole lane would point every leaf away
    // from the thing it feeds — the one rule a rerouter's facing has to obey.
    let chain_rotation = match placements.get(&group.source_anchor) {
        Some(p) if !group.taps.is_empty() && group.taps.iter().all(|t| t.x > p.x) => {
            NodeRotation::Deg270
        }
        _ => NodeRotation::Deg90,
    };
    let mut upstream = BusEnd::Node(demand.source);

    // The head: a down-pointing rerouter in the lane's column at the source's
    // own level, so the value turns into the gutter beside its producer and
    // begins the chain there. A source standing level with a stop is already
    // served by that stop's tap, which holds the cell, so there the source
    // drives the first stop directly.
    if let Some(p) = placements.get(&group.source_anchor) {
        if claim_column(lane_cells, (page, lane), p.x, p.x + 2 * REROUTER_HALF) {
            // The source-side rerouter, the mirror of the gate-side ones
            // below: one tap reserve to the producer's LEFT, on the producer's
            // own level. The gate wires into it and it hands the value on, so
            // a value leaving a gate reads as a right angle at the gate rather
            // than as a wire off its port heading for the gutter.
            //
            // It faces `−Y`, by the same rule that turns the leaves the other
            // way: a rerouter faces the direction its value travels, and from
            // here the value travels left into the band.
            //
            // Offered only where the head itself stands, so the two share the
            // producer's level and the run between them is horizontal — with
            // the head dropped, the lane's first brick is a stop's link out on
            // another level, and inserting this one would draw a diagonal
            // instead of removing one.
            //
            // The cell comes out of the same `row_cells` reservation the
            // gate-side rerouters claim, so nothing is placed unreserved.
            // `line_groups` opens a `TAP_RESERVE` in front of every column, so
            // it is normally free; where it is not — the producer also CONSUMES
            // a bussed value there, or a second lane leaves the same brick by
            // another port — the brick is left out and the producer drives the
            // head directly, exactly as before.
            if let Some(row) = group.source_row {
                let src_y = p.y - TAP_RESERVE;
                let cell: Cell = (
                    p.x,
                    p.x + 2 * REROUTER_HALF,
                    src_y,
                    src_y + 2 * REROUTER_HALF,
                );
                let claimed = row_cells.entry((page, row)).or_default();
                if !claimed.iter().any(|&c| cells_overlap(c, cell)) {
                    claimed.push(cell);
                    let src_id = push_bus_node(
                        bus,
                        p.x,
                        src_y,
                        z,
                        NodeRotation::Deg180,
                        BusRole::Source,
                        color_of,
                    );
                    bus.wires.push(BusWire {
                        source: upstream,
                        target: BusEnd::Bus(src_id),
                    });
                    upstream = BusEnd::Bus(src_id);
                }
            }

            let id = push_bus_node(bus, p.x, lane_y, z, chain_rotation, BusRole::Gutter, color_of);
            bus.wires.push(BusWire {
                source: upstream,
                target: BusEnd::Bus(id),
            });
            upstream = BusEnd::Bus(id);
        }
    }

    // The tap of the stop being served, and the level it stands at, so rows
    // sharing a stop share its bricks.
    let mut stop_tap: Option<(BusNodeId, i32)> = None;
    for &(row, stop_x) in stops {
        let mut taps: Vec<&BusTap> = group.taps.iter().filter(|t| t.row == row).collect();
        if taps.is_empty() {
            continue;
        }
        taps.sort_by_key(|t| (t.y, t.x, t.target.node_id, t.target.port.as_str()));

        let tap_id = match stop_tap {
            Some((id, x)) if x == stop_x => id,
            _ => {
                // The link rides one rerouter above the tap. Its cell is the
                // one thing a stop can lose; the chain then carries on
                // through the tap itself.
                if claim_column(
                    lane_cells,
                    (page, lane),
                    stop_x + 2 * REROUTER_HALF,
                    stop_x + LANE_PAIR_HEIGHT,
                ) {
                    let link_id = push_bus_node(
                        bus,
                        stop_x + 2 * REROUTER_HALF,
                        lane_y,
                        z,
                        chain_rotation,
                        BusRole::Gutter,
                        color_of,
                    );
                    bus.wires.push(BusWire {
                        source: upstream,
                        target: BusEnd::Bus(link_id),
                    });
                    upstream = BusEnd::Bus(link_id);
                }

                // The tap branches off the lane node and stops there: it
                // carries the value out to its row, and `upstream` stays on
                // the down-pointing link so the next stop chains from the
                // lane, not through this branch.
                let id = push_bus_node(bus, stop_x, lane_y, z, NodeRotation::Deg0, BusRole::Gutter, color_of);
                bus.wires.push(BusWire {
                    source: upstream,
                    target: BusEnd::Bus(id),
                });
                stop_tap = Some((id, stop_x));
                id
            }
        };

        // One gate-side rerouter per column: two ports of one gate share a
        // tap rather than stacking two bricks in the same cell.
        let mut prev = tap_id;
        let mut i = 0usize;
        while i < taps.len() {
            let column = taps[i].y;
            let gate_y = column - TAP_GAP - 2 * REROUTER_HALF;
            let cell: Cell = (
                stop_x,
                stop_x + 2 * REROUTER_HALF,
                gate_y,
                gate_y + 2 * REROUTER_HALF,
            );
            let claimed = row_cells.entry((page, row)).or_default();
            if !claimed.iter().any(|&c| cells_overlap(c, cell)) {
                claimed.push(cell);
                let gate_id = push_bus_node(bus, stop_x, gate_y, z, NodeRotation::Deg0, BusRole::Gutter, color_of);
                bus.wires.push(BusWire {
                    source: BusEnd::Bus(prev),
                    target: BusEnd::Bus(gate_id),
                });
                prev = gate_id;
            }
            while i < taps.len() && taps[i].y == column {
                bus.wires.push(BusWire {
                    source: BusEnd::Bus(prev),
                    target: BusEnd::Node(taps[i].target),
                });
                bus.suppressed
                    .insert((taps[i].target.node_id, taps[i].target.port));
                i += 1;
            }
        }
    }
}

/// The lanes a code body owes, resolved before its edge pins are placed.
///
/// The two halves are split because they straddle the pin stack: the stack
/// has to clear the band the lanes claim, and a lane head has to stand at
/// its source's own row — which an edge-pin source only has once the stack
/// itself is placed.
#[derive(Default)]
struct BusPlan {
    /// Gutter band width per page, for the input-pin stack to clear.
    band_widths: Vec<i32>,
    groups: Vec<BusGroup>,
    /// Per page, in emission order: the group a lane carries, its demand,
    /// and its lane index.
    lanes: Vec<Vec<(usize, BusDemand, usize)>>,
    /// Lane count per page; lane 0 is the leftmost of them.
    lanes_used: Vec<usize>,
    /// Cells the body already occupies on each row, so a gate-side tap that
    /// cannot fit beside its gate is not placed.
    row_cells: HashMap<(usize, usize), Vec<Cell>>,
    /// `(page, sub-row offset)` -> row index, so a later pass can name the
    /// same rows `row_cells` is keyed by.
    row_index: HashMap<(usize, i32), usize>,
    /// `(page, row)` -> that row's top edge. Every node in a row shares it
    /// whatever its own height, so it is the ceiling a staggered run may not
    /// climb past without leaving the row it serves.
    row_top: HashMap<(usize, usize), i32>,
}

/// Pick the values worth a lane and hand each one a column: one lane per
/// bussed value, packed into the band between the code body's left edge and
/// the input-pin stack.
///
/// Lanes are allocated per page, so a value read on two pages heads a lane
/// on each: a page is a separate plane in z, and a lane's column is measured
/// from its own page's left edge.
///
/// KNOWN DEVIATION: a page's `page_w`/`page_h` and its [`PageInfo`] are
/// measured before this runs — the lanes read the body's final placements, so
/// they cannot be measured with it — and are never widened afterwards. Nothing
/// collides: `band_widths` is what the input-pin stack clears, and the layout
/// bounds sweep the bus nodes directly. What it costs is centring: on a
/// paginated layout each page is centred on its body alone, so pages whose
/// bands claim different numbers of lanes sit slightly off-centre relative to
/// one another. Single-page layouts — every code layout inside the default
/// budgets — are unaffected.
fn plan_bus(
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    rotations: &HashMap<NodeId, NodeRotation>,
    node_row: &HashMap<NodeId, (usize, i32)>,
    page_infos: &[PageInfo],
) -> BusPlan {
    let mut plan = BusPlan {
        band_widths: vec![0i32; page_infos.len()],
        lanes: vec![Vec::new(); page_infos.len()],
        lanes_used: vec![0usize; page_infos.len()],
        ..BusPlan::default()
    };
    if page_infos.is_empty() || node_row.is_empty() {
        return plan;
    }

    // Row identity per page: the sub-row offsets in top-down order, so a row
    // index reads the way `BusDemand.rows` expects.
    let mut downs: Vec<Vec<i32>> = vec![Vec::new(); page_infos.len()];
    for &(page, down) in node_row.values() {
        if let Some(d) = downs.get_mut(page) {
            d.push(down);
        }
    }
    let mut row_index: HashMap<(usize, i32), usize> = HashMap::default();
    for (pi, d) in downs.iter_mut().enumerate() {
        d.sort_unstable();
        d.dedup();
        for (i, &down) in d.iter().enumerate() {
            row_index.insert((pi, down), i);
        }
    }

    // Cells already claimed on each row. A tap's own height sits inside its
    // row's band, so only that row's gates can collide with it.
    let mut row_cells: HashMap<(usize, usize), Vec<Cell>> = HashMap::default();
    let mut row_top: HashMap<(usize, usize), i32> = HashMap::default();
    for (id, &(page, down)) in node_row.iter() {
        let (Some(p), Some(n), Some(&row)) = (
            placements.get(id),
            module.nodes.get(id),
            row_index.get(&(page, down)),
        ) else {
            continue;
        };
        let (hsx, hsy) = cell_half_size(n, rotation_of(rotations, id));
        row_cells
            .entry((page, row))
            .or_default()
            .push((p.x, p.x + hsx * 2, p.y, p.y + hsy * 2));
        let top = p.x + hsx * 2;
        row_top
            .entry((page, row))
            .and_modify(|t| *t = (*t).max(top))
            .or_insert(top);
    }

    let owners = chip_owners(module);
    let groups = collect_bus_groups(module, placements, node_row, &row_index, &owners);

    for (pi, info) in page_infos.iter().enumerate() {
        if info.min_y == i32::MAX {
            continue;
        }
        let mut picked: Vec<usize> = Vec::new();
        let mut demands: Vec<BusDemand> = Vec::new();
        for (gi, g) in groups.iter().enumerate() {
            if g.page != pi {
                continue;
            }
            let Some(src) = module.nodes.get(&g.source_anchor) else {
                continue;
            };
            let mut rows: Vec<usize> = g.taps.iter().map(|t| t.row).collect();
            rows.sort_unstable();
            rows.dedup();
            // A value confined to one row only earns a lane if it is a
            // stored value, a port, or crosses a chip wall; anything else
            // already reads as a short in-line wire. A crossing never does —
            // it runs between this body and a chip brick, which is the long
            // diagonal the band exists to replace.
            if rows.is_empty() || (rows.len() < 2 && !g.crosses_chip && !is_bus_worthy_source(src))
            {
                continue;
            }
            demands.push(BusDemand {
                source: g.source,
                rows,
                consumers: g.taps.len(),
                source_order: g.order,
            });
            picked.push(gi);
        }
        if demands.is_empty() {
            continue;
        }
        let lanes = allocate_lanes(&demands);
        let lanes_used = lanes.iter().max().copied().unwrap_or(0) + 1;
        // Lane 0 sits at `min_y - BUS_BAND_GUTTER - lanes_used * LANE_PITCH`,
        // which is exactly the band's own left edge, so the band contains
        // every lane and the input-pin stack clears all of them.
        plan.band_widths[pi] = lanes_used as i32 * LANE_PITCH + BUS_BAND_GUTTER;
        plan.lanes_used[pi] = lanes_used;
        plan.lanes[pi] = picked
            .iter()
            .zip(demands)
            .zip(lanes)
            .map(|((&gi, demand), lane)| (gi, demand, lane))
            .collect();
    }

    plan.groups = groups;
    plan.row_cells = row_cells;
    plan.row_index = row_index;
    plan.row_top = row_top;
    plan
}

/// Lay the planned lanes out as rerouters and wires, and record the module
/// wires they replace.
fn lay_bus(
    placements: &HashMap<NodeId, Placement>,
    page_infos: &[PageInfo],
    plan: &mut BusPlan,
) -> (BusLayout, usize) {
    let mut bus = BusLayout::default();
    let mut laid = 0usize;
    let mut lane_cells: LaneCells = HashMap::default();
    let pages = std::mem::take(&mut plan.lanes);
    for (pi, lanes) in pages.iter().enumerate() {
        let Some(info) = page_infos.get(pi) else {
            continue;
        };
        // Every stop's tap holds its column cell before any head or link is
        // offered one: a tap stands on the row it serves and may not be
        // moved off it, while the other two are droppable.
        //
        // Consecutive stops may repeat an `x` — `lane_stops` merges rows that
        // land inside one rerouter's footprint and `emit_lane` reuses the one
        // tap brick for them — so a repeat is claimed once and kept. Any OTHER
        // contention is two taps wanting one cell, and `emit_lane` pushes a tap
        // unconditionally, so it would stack two rerouters in the same cell and
        // the game would silently drop one of them.
        //
        // It cannot arise while a lane's stop `x` descends monotonically down
        // its rows, which holds within a band: a row's own height is at least
        // the tallest brick standing in it, so the next row's `down` has
        // already cleared this row's bricks and `x = page_h - down - height`
        // strictly decreases. Row keys are `(page, down)` and `assemble_pages`
        // restarts each band's offsets at zero, so two bands sharing a page
        // interleave their rows under one key set and that ordering is the one
        // thing that can break it. A stop whose cell is taken is DROPPED rather
        // than stacked: `emit_lane` never reaches it, so it suppresses nothing
        // and its consumers keep the direct wires they came with.
        // A lane has to go somewhere. A group whose source and taps sit at
        // the same height was already an inline hop, and routing it out to the
        // gutter and back spends two rerouters to lengthen a wire that was
        // short to begin with.
        //
        // Measured HERE rather than in `plan_bus` because only here are the
        // placements final: an edge pin has none until `place_edge_pins` runs,
        // and a port delivery is exactly the long run the band exists for, so
        // measuring it early would read zero and throw the lane away. Skipping
        // the lane is all-or-nothing by construction — `emit_lane` is what
        // lays bricks AND records suppression, so a lane never entered lays
        // nothing and suppresses nothing, and its wires stay as they were.
        let lanes: Vec<&(usize, BusDemand, usize)> = lanes
            .iter()
            .filter(|(gi, _, _)| {
                let g = &plan.groups[*gi];
                let Some(head) = placements.get(&g.source_anchor) else {
                    return true;
                };
                let lo = g.taps.iter().map(|t| t.x).fold(head.x, i32::min);
                let hi = g.taps.iter().map(|t| t.x).fold(head.x, i32::max);
                hi - lo >= MIN_LANE_TRAVEL
            })
            .collect();
        laid += lanes.len();

        let mut stops: Vec<Vec<(usize, i32)>> = lanes
            .iter()
            .map(|(gi, demand, _)| lane_stops(&plan.groups[*gi], demand))
            .collect();

        // Several lanes tapping ONE row is the normal case, and every one of
        // their runs leaves the gutter horizontally toward that row. At a
        // single level those runs are drawn on top of each other. Give each
        // its own level, a rerouter's height apart.
        //
        // The whole stop moves: `stop_x` is what the tap, the link stacked
        // above it and every gate-side rerouter along the row are all derived
        // from, so shifting it keeps them level with EACH OTHER — which is
        // what keeps the run out of the gutter horizontal. Shifting one end
        // alone would put back the diagonals this feature exists to remove.
        //
        // Choosing the level and claiming the lane column are ONE step, so the
        // two constraints cannot disagree. Levels are tried upward from the
        // stop's own row, bounded by that row's top edge so a run never climbs
        // out of the row it serves, and each candidate must be both unused on
        // the row and free in the lane's column. The search subsumes the old
        // fixed pre-claim: a lane whose stops do not descend monotonically —
        // possible only where two bands share a page and interleave their row
        // keys — simply finds its next free level instead of stacking two
        // rerouters in one cell.
        //
        // Falling back, in order: the base level un-staggered (the spec's
        // preference — a shared level is a drawing problem, leaving the row is
        // a correctness one), then dropping the stop, which suppresses nothing
        // and leaves its consumers the direct wires they came with.
        let mut used: HashMap<usize, Vec<i32>> = HashMap::default();
        for ((_, _, lane), stops_for_lane) in lanes.iter().zip(stops.iter_mut()) {
            // (original x, the x it was given) of the stop above, so a SHARED
            // stop — `lane_stops` repeats an x when consecutive rows fall
            // inside one rerouter — keeps sharing the one brick after the
            // shift, and does not claim the cell twice.
            let mut prev: Option<(i32, i32)> = None;
            stops_for_lane.retain_mut(|(row, x)| {
                let base = *x;
                if let Some((prev_base, prev_final)) = prev {
                    if prev_base == base {
                        *x = prev_final;
                        used.entry(*row).or_default().push(prev_final);
                        return true;
                    }
                }
                let ceiling = plan
                    .row_top
                    .get(&(pi, *row))
                    .copied()
                    .unwrap_or(i32::MAX)
                    .saturating_sub(2 * REROUTER_HALF);
                let taken = used.entry(*row).or_default();
                let mut chosen = None;
                let mut level = base;
                while level <= ceiling {
                    if !taken.contains(&level)
                        && claim_column(
                            &mut lane_cells,
                            (pi, *lane),
                            level,
                            level + 2 * REROUTER_HALF,
                        )
                    {
                        chosen = Some(level);
                        break;
                    }
                    level += STAGGER_STEP;
                }
                // Overflow: every level in the band already carries a run. A
                // row can hold far more taps than it has levels — its band is
                // only as tall as its tallest brick — so this is reached on
                // any hot row, and what happens here decides how bad the
                // residual looks.
                //
                // Spread, do not pile. Taking the LEAST loaded level divides a
                // hot row's leftover overlap across the whole band instead of
                // stacking every one of them back onto the base level. The
                // bound is unchanged: the candidates ARE the band, so nothing
                // drifts into the row above, and the column claim still has
                // the last word.
                if chosen.is_none() {
                    let mut candidates: Vec<i32> = Vec::new();
                    let mut level = base;
                    while level <= ceiling {
                        candidates.push(level);
                        level += STAGGER_STEP;
                    }
                    candidates.sort_by_key(|l| (taken.iter().filter(|t| *t == l).count(), *l));
                    chosen = candidates.into_iter().find(|&l| {
                        claim_column(&mut lane_cells, (pi, *lane), l, l + 2 * REROUTER_HALF)
                    });
                }
                // Last resort, for a row too shallow to offer a level at all.
                let chosen = chosen.or_else(|| {
                    claim_column(&mut lane_cells, (pi, *lane), base, base + 2 * REROUTER_HALF)
                        .then_some(base)
                });
                match chosen {
                    Some(c) => {
                        taken.push(c);
                        *x = c;
                        prev = Some((base, c));
                        true
                    }
                    None => false,
                }
            });
        }
        for ((gi, demand, lane), stops) in lanes.iter().zip(&stops) {
            emit_lane(
                &mut bus,
                placements,
                &plan.groups[*gi],
                demand,
                stops,
                *lane,
                plan.lanes_used[pi],
                pi,
                info.min_y,
                info.z,
                &mut plan.row_cells,
                &mut lane_cells,
            );
        }
    }
    (bus, laid)
}

/// Route a line's expression values DOWN into the statement gate consuming
/// them — the gutter bus in miniature, inside one line's own block.
///
/// `line_groups` drops a statement's exec gate to the bottom row of its group,
/// below the expression columns feeding it. That is what makes the value flow
/// read as a descent, but it turns each operand wire into a down-AND-left
/// diagonal across the block. One rerouter straightens it: standing where the
/// operand's COLUMN meets the sink's ROW, it takes the value straight down and
/// hands it straight left. The sink's row carries nothing but the sink itself,
/// so that leftward run crosses no gate.
///
/// Same rules as the lanes, which is why it shares their `BusLayout`: the
/// rerouter is a `Deg0` leaf feeding exactly one port, it takes exactly one
/// inbound wire, and the module wire it replaces is suppressed so emit draws
/// one path and not two.
///
/// What it deliberately leaves alone:
/// - anything the gutter already suppressed — that consumer has its one
///   replacement path and a second would be fan-in;
/// - statement-to-statement exec, where source and sink share column 0. Those
///   run straight DOWN the spine already, and detouring them is the spec's
///   stated non-goal. The `src.y > sink.y` test is what separates the two: an
///   operand is strictly right of the spine, a preceding statement is level
///   with it.
#[allow(clippy::too_many_arguments)]
fn lay_mini_bus(
    bus: &mut BusLayout,
    module: &Module,
    placements: &HashMap<NodeId, Placement>,
    rotations: &HashMap<NodeId, NodeRotation>,
    node_row: &HashMap<NodeId, (usize, i32)>,
    node_line: &HashMap<NodeId, usize>,
    page_infos: &[PageInfo],
    plan: &mut BusPlan,
) {
    // Levels already handed out on each sink's row, so two corners dropping
    // into one statement do not leave at the same height.
    let subtree = chip_owners(module);
    let mut sink_levels: HashMap<NodeId, Vec<i32>> = HashMap::default();
    for w in &module.wires {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        if w.source.node_id == w.target.node_id {
            continue;
        }
        // The gutter owns this consumer already.
        if bus
            .suppressed
            .contains(&(w.target.node_id, w.target.port))
        {
            continue;
        }
        if !subtree.is_bussable(module, w.source.node_id)
            || !subtree.is_bussable(module, w.target.node_id)
        {
            continue;
        }
        // Only the sink that MOVED, and only from its own line's block.
        if rotation_of(rotations, &w.target.node_id) != NodeRotation::Deg90 {
            continue;
        }
        let (Some(&src_line), Some(&sink_line)) = (
            node_line.get(&w.source.node_id),
            node_line.get(&w.target.node_id),
        ) else {
            continue;
        };
        if src_line != sink_line {
            continue;
        }
        let (Some(&(src_page, src_down)), Some(&(sink_page, sink_down))) = (
            node_row.get(&w.source.node_id),
            node_row.get(&w.target.node_id),
        ) else {
            continue;
        };
        // Strictly above, on the same page: the drop only exists downward.
        if src_page != sink_page || src_down >= sink_down {
            continue;
        }
        let (Some(&src_at), Some(&sink_at)) = (
            placements.get(&w.source.node_id),
            placements.get(&w.target.node_id),
        ) else {
            continue;
        };
        // ...and strictly right of the spine, which is what makes it an
        // operand rather than the statement before this one.
        if src_at.y <= sink_at.y {
            continue;
        }
        let Some(&row) = plan.row_index.get(&(sink_page, sink_down)) else {
            continue;
        };
        let Some(info) = page_infos.get(sink_page) else {
            continue;
        };

        // Two operands dropping into ONE statement leave their corners on that
        // statement's row, and at a single level their runs into it are drawn
        // on top of each other — the gutter's collision, in the line's own
        // block. So each corner takes its own level, a rerouter apart, bounded
        // by the sink's own top edge: a corner outside the sink's extent is a
        // run that no longer arrives at the gate it feeds.
        //
        // Rows are checked as one set, so a corner can never land inside a
        // gate — or inside a gutter tap already standing on this row. A
        // corner that fits at no level keeps its direct diagonal: a longer
        // read, never a lost one.
        let Some(sink) = module.nodes.get(&w.target.node_id) else {
            continue;
        };
        let sink_top =
            sink_at.x + cell_half_size(sink, rotation_of(rotations, &w.target.node_id)).0 * 2;
        let taken = sink_levels.entry(w.target.node_id).or_default();
        let cell_at = |level: i32| -> Cell {
            (
                level,
                level + 2 * REROUTER_HALF,
                src_at.y,
                src_at.y + 2 * REROUTER_HALF,
            )
        };
        let blocked = |cell: Cell| -> bool {
            plan.row_cells
                .get(&(sink_page, row))
                .is_some_and(|cs| cs.iter().any(|&c| cells_overlap(c, cell)))
        };
        // The lowest free level inside the sink's own extent.
        let mut level = sink_at.x;
        let mut corner_x = loop {
            if level + 2 * REROUTER_HALF > sink_top {
                break None;
            }
            if !taken.contains(&level) && !blocked(cell_at(level)) {
                break Some((level, cell_at(level)));
            }
            level += STAGGER_STEP;
        };
        // Overflow, same rule as the gutter: rather than give up when every
        // level is spoken for, take the LEAST loaded one whose cell is still
        // clear. A shared level draws two runs over each other; no corner at
        // all leaves a diagonal across the block, which is worse.
        if corner_x.is_none() {
            let mut candidates: Vec<i32> = Vec::new();
            let mut level = sink_at.x;
            while level + 2 * REROUTER_HALF <= sink_top {
                candidates.push(level);
                level += STAGGER_STEP;
            }
            candidates.sort_by_key(|l| (taken.iter().filter(|t| *t == l).count(), *l));
            corner_x = candidates
                .into_iter()
                .find(|&l| !blocked(cell_at(l)))
                .map(|l| (l, cell_at(l)));
        }
        let Some((corner_x, cell)) = corner_x else {
            continue;
        };
        taken.push(corner_x);
        plan.row_cells
            .entry((sink_page, row))
            .or_default()
            .push(cell);

        // Faces LEFT: the corner stands in its operand's column, out to the
        // right of the statement, and the run from here goes leftward into it.
        // A rerouter faces what it feeds, so this is the half turn — and it is
        // the one place the two buses differ, since a gutter tap runs the
        // other way.
        let id = push_bus_node(
            bus,
            corner_x,
            src_at.y,
            info.z,
            NodeRotation::Deg180,
            BusRole::Line,
            Some(w.source.node_id),
        );
        bus.wires.push(BusWire {
            source: BusEnd::Node(w.source),
            target: BusEnd::Bus(id),
        });
        bus.wires.push(BusWire {
            source: BusEnd::Bus(id),
            target: BusEnd::Node(w.target),
        });
        bus.suppressed.insert((w.target.node_id, w.target.port));
    }
}

#[cfg(test)]
mod tests;
