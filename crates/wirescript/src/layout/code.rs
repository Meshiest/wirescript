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
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::diagnostic::Pos;
    use crate::ir::{GateIO, ROOT_SCOPE_ID, SourceRange, Wire, gate_class};

    fn lowered(src: &str) -> Module {
        let parsed = crate::parser::parse(src, "test");
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let tc = crate::typecheck::typecheck(&parsed.ast, "test");
        let r = crate::lower::lower(crate::lower::LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
        });
        r.module
    }

    /// Lower an entry file plus the in-memory files it imports, the way
    /// `compile` does, and return the module alongside layout options
    /// carrying the ENTRY file's source map.
    fn lowered_with_imports(entry: &str, files: &[(&str, &str)]) -> (Module, LayoutOptions) {
        let loader = crate::resolve::MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let resolved = crate::resolve::resolve(entry, "main.ws", &loader);
        assert!(
            !resolved
                .diagnostics
                .iter()
                .any(|d| matches!(d.severity, crate::diagnostic::Severity::Error)),
            "{:?}",
            resolved.diagnostics
        );
        let tc = crate::typecheck::typecheck(&resolved.ast, "main.ws");
        let r = crate::lower::lower(crate::lower::LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "main.ws",
            module_name: None,
            template_cache: Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &resolved.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
        });
        let opts = LayoutOptions {
            source_map: Some(resolved.source_map.clone()),
            ..Default::default()
        };
        (r.module, opts)
    }

    fn make_range(file: &str, line: u32, col: u32, start: usize, end: usize) -> SourceRange {
        SourceRange {
            file: file.into(),
            start: Pos {
                offset: start,
                line,
                col,
            },
            end: Pos {
                offset: end,
                line,
                col: col + (end - start) as u32,
            },
        }
    }

    fn make_node(gate_class: &'static str, range: SourceRange) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO::default()),
            source_range: range,
            chip_id: None,
            chain_id: None,
            scope_id: ROOT_SCOPE_ID,
            note: None,
        }
    }

    fn wire(from: NodeId, to: NodeId) -> Wire {
        Wire {
            source: from.port(WirePort::Output),
            target: to.port(WirePort::Input),
        }
    }

    fn module_with(nodes: Vec<Node>, wires: Vec<Wire>) -> Module {
        let mut m = Module::new("t");
        for n in nodes {
            m.add_node(n);
        }
        m.wires = wires;
        m
    }

    fn opts() -> LayoutOptions {
        LayoutOptions::default()
    }

    /// `opts()` with the mode set, for the tests that lay a chip tree out
    /// recursively. `layout_code` places THIS module whatever the mode says,
    /// but the recursion goes back through `layout_with_opts`, which dispatches
    /// on it — so a default-mode recursive call lays every chip interior out in
    /// DAG mode and no chip ever builds a bus.
    fn code_opts() -> LayoutOptions {
        LayoutOptions {
            mode: crate::layout::LayoutMode::Code,
            ..LayoutOptions::default()
        }
    }

    #[test]
    fn earlier_line_sits_above_later_line() {
        let m = lowered("var a: int = 0\nvar b: int = 0\n");
        let l = layout_code(&m, &opts(), false);
        let a = m
            .nodes
            .values()
            .find(|n| n.source_range.start.line == 1)
            .expect("line 1 node");
        let b = m
            .nodes
            .values()
            .find(|n| n.source_range.start.line == 2)
            .expect("line 2 node");
        assert!(
            l.placements[&a.id].x > l.placements[&b.id].x,
            "earlier line must sit higher (greater Placement.x)"
        );
    }

    #[test]
    fn same_line_nodes_flow_left_to_right_in_token_order() {
        let a = make_node("G", make_range("f", 1, 0, 0, 1));
        let b = make_node("G", make_range("f", 1, 4, 4, 5));
        let c = make_node("G", make_range("f", 1, 8, 8, 9));
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let m = module_with(vec![a, b, c], vec![]);
        let l = layout_code(&m, &opts(), false);
        assert!(l.placements[&a_id].y < l.placements[&b_id].y);
        assert!(l.placements[&b_id].y < l.placements[&c_id].y);
    }

    #[test]
    fn statement_value_sink_heads_its_line_on_the_left() {
        // The `Var_Set` is the statement's sink — it takes the exec and
        // nothing on the line reads it back — so it is pinned to the line's
        // left column and reads its value from the right. Every other gate
        // on the line, the whole expression it consumes, sits strictly
        // right of it.
        let m = lowered("var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }");
        let l = layout_code(&m, &opts(), false);
        let var_set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_SET)
            .expect("Var_Set node");
        let line = var_set.source_range.start.line;
        let set_y = l.placements[&var_set.id].y;
        let mut expression_gates = 0usize;
        for n in m.nodes.values().filter(|n| {
            n.kind == NodeKind::Gate && n.source_range.start.line == line && n.id != var_set.id
        }) {
            expression_gates += 1;
            assert!(
                l.placements[&n.id].y > set_y,
                "expression gate {} must sit RIGHT of the sink it feeds (y={}, sink y={set_y})",
                n.gate_class,
                l.placements[&n.id].y
            );
        }
        assert!(
            expression_gates >= 4,
            "fixture must lower an expression tree, got {expression_gates} gates"
        );
    }

    #[test]
    fn expression_operands_sit_left_of_the_operator_they_feed() {
        // `(a + b) * (a + 2)` — the two adds FEED the multiply, so both sit
        // strictly LEFT of it, and they occupy different sub-rows.
        let src = "var a: int = 1\nvar b: int = 2\nin go: exec\non go {\n  let m = (a + b) * (a + 2)\n  PrintToConsole(\"${m}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let by_class = |c: &str| -> Vec<NodeId> {
            let mut v: Vec<NodeId> = m
                .nodes
                .values()
                .filter(|n| n.gate_class.ends_with(c))
                .map(|n| n.id)
                .collect();
            v.sort();
            v
        };
        let mul = by_class("MathMultiply");
        let add = by_class("MathAdd");
        assert!(!mul.is_empty() && add.len() >= 2);
        let mul_y = l.placements[&mul[0]].y;
        for a in &add {
            assert!(
                l.placements[a].y < mul_y,
                "operand {a} must sit LEFT of the operator it feeds (operand y={}, operator y={mul_y})",
                l.placements[a].y
            );
        }
        // The two adds are siblings: same depth column, different rows.
        assert_ne!(
            l.placements[&add[0]].x, l.placements[&add[1]].x,
            "sibling operands must occupy different sub-rows"
        );
    }

    #[test]
    fn sequenced_statements_on_one_line_keep_source_order() {
        // The exec wire runs `a = 1` -> `b = 2`. Counted as an operand edge
        // it would make `b = 2` the line's root and print the line backwards.
        let src = "var a: int = 0\nvar b: int = 0\nin go: exec\non go { a = 1  b = 2 }\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        let mut sets: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| n.gate_class == gate_class::VAR_SET)
            .collect();
        sets.sort_by_key(|n| n.source_range.start.offset);
        assert_eq!(sets.len(), 2, "one Var_Set per statement");
        assert_eq!(
            sets[0].source_range.start.line, sets[1].source_range.start.line,
            "fixture must put both statements on one line"
        );
        assert!(
            l.placements[&sets[0].id].y < l.placements[&sets[1].id].y,
            "the first statement must sit left of the second"
        );
    }

    #[test]
    fn a_trigger_sharing_a_line_with_its_statement_stays_leftmost() {
        // The event node and the statement it fires share line 2, joined by
        // an exec wire — read as an operand edge, that wire pushes the
        // trigger to the right of its own body.
        let src = "var a: int = 0\non RoundStart { a = 1 }\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        let y_of = |kind: NodeKind| -> i32 {
            let n = m
                .nodes
                .values()
                .find(|n| n.source_range.start.line == 2 && n.kind == kind)
                .unwrap_or_else(|| panic!("a {kind:?} node on line 2"));
            l.placements[&n.id].y
        };
        assert!(
            y_of(NodeKind::Event) < y_of(NodeKind::Gate),
            "the handler's trigger must stay left of the statement it fires"
        );
    }

    #[test]
    fn flat_line_layout_is_unchanged_by_tree_ordering() {
        // A line with no in-line nesting must keep the single-row shape.
        let src = "var a: int = 1\nin go: exec\non go {\n  a = 1\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let rows: std::collections::HashSet<i32> = l.placements.values().map(|p| p.x).collect();
        assert!(
            rows.len() <= 3,
            "flat program should stay compact, got {} rows",
            rows.len()
        );
    }

    #[test]
    fn indent_column_shifts_line_right() {
        let a = make_node("G", make_range("f", 1, 0, 0, 1));
        let b = make_node("G", make_range("f", 2, 2, 10, 11));
        let (a_id, b_id) = (a.id, b.id);
        let m = module_with(vec![a, b], vec![]);
        let l = layout_code(&m, &opts(), false);
        assert_eq!(
            l.placements[&b_id].y - l.placements[&a_id].y,
            2 * INDENT_UNIT
        );
    }

    /// `lowered` parses as `"test"`, so the map must claim the same file or
    /// the anchor guard sends the layout to its no-source-map fallbacks.
    fn opts_with_map(src: &str) -> LayoutOptions {
        LayoutOptions {
            source_map: Some(std::sync::Arc::new(crate::ast::SourceMap::from_source(
                src, "test",
            ))),
            ..Default::default()
        }
    }

    /// y of the head node of the 1-based source `line`, i.e. the node the
    /// line's indent is applied to.
    fn head_y(m: &Module, l: &LayoutResult, line: u32) -> i32 {
        let mut heads: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| n.source_range.start.line == line && !is_edge_pin(n))
            .collect();
        heads.sort_by_key(|n| (n.source_range.start.offset, Reverse(n.source_range.end.offset)));
        l.placements[&heads.first().expect("a node on that line").id].y
    }

    #[test]
    fn indent_comes_from_source_line_not_first_node_column() {
        // Both statements start at source column 0, but their first IR
        // nodes are the `+` expressions — at columns 18 and 9. Keying the
        // indent off the node column staggers two unindented lines.
        let src = "let aaaaaaaaaa = 1 + 2\nlet b = 3 + 4\n";
        let m = lowered(src);

        let fallback = layout_code(&m, &opts(), false);
        assert_ne!(
            head_y(&m, &fallback, 1),
            head_y(&m, &fallback, 2),
            "fixture must actually exercise the first-node-column path"
        );

        let mapped = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(
            head_y(&m, &mapped, 1),
            head_y(&m, &mapped, 2),
            "two column-0 statements must share the same left margin"
        );
    }

    #[test]
    fn source_map_indent_shifts_a_line_by_its_own_column() {
        // Line 2's statement is indented two columns. Under the node
        // columns alone (12 and 11) it would sit one unit LEFT of line 1.
        let src = "let aaaa = 1 + 2\n  let b = 3 + 4\n";
        let m = lowered(src);

        let fallback = layout_code(&m, &opts(), false);
        assert!(
            head_y(&m, &fallback, 2) < head_y(&m, &fallback, 1),
            "fixture must actually exercise the first-node-column path"
        );

        let l = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(
            head_y(&m, &l, 2) - head_y(&m, &l, 1),
            2 * INDENT_UNIT,
            "a two-column indent shifts the line by two indent units"
        );
    }

    #[test]
    fn nested_statement_indent_scales_with_source_column() {
        let src = "in go: exec\non go {\n  PrintToConsole(\"x\")\n}\n";
        let sm = crate::ast::SourceMap::from_source(src, "test");
        // line 2 (0-based) is indented two spaces
        assert_eq!(sm.line_indent[2], 2);
        assert_eq!(sm.line_indent[0], 0);
        assert_eq!(sm.line_indent[3], 0);
    }

    #[test]
    fn own_line_comments_get_their_own_row() {
        // The handler needs a body: an empty one lowers to no nodes, leaving
        // a single row for the comment to be trivially "between".
        let src = "var x: int = 0\nin go: exec\n// a standalone note\non go { x = 1 }\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(l.annotations.len(), 1, "one own-line comment renders");
        let a = &l.annotations[0];
        assert_eq!(a.text, "a standalone note");
        // Sits between the `var x` row and the `on go` row.
        let rows: Vec<i32> = l.placements.values().map(|p| p.x).collect();
        let top = *rows.iter().max().unwrap();
        let bottom = *rows.iter().min().unwrap();
        assert!(
            a.x < top && a.x > bottom,
            "comment row {} must fall between the code rows {bottom}..{top}",
            a.x
        );
    }

    #[test]
    fn comment_labels_start_at_their_own_source_indent() {
        let src =
            "var x: int = 0\nin go: exec\non go {\n  // indented note\n  x = 1\n}\n// flush note\nvar y: int = 0\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        let y_of = |text: &str| -> i32 {
            l.annotations
                .iter()
                .find(|a| a.text == text)
                .unwrap_or_else(|| panic!("{text} should render; got {:?}", l.annotations))
                .y
        };
        assert_eq!(
            y_of("indented note") - y_of("flush note"),
            2 * INDENT_UNIT,
            "a two-column indent shifts the label by two indent units"
        );
    }

    #[test]
    fn comment_rows_stay_inside_the_plane_and_clear_of_every_gate() {
        let src = "// header note\nvar x: int = 0\nin go: exec\non go {\n  // step one\n  x = x + 1\n}\n// footer note\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(l.annotations.len(), 3, "{:?}", l.annotations);

        let e = crate::layout::wall::plane_extent(&l);
        for a in &l.annotations {
            assert!(
                a.x >= -e.x && a.x + ANNOTATION_SIZE <= e.x,
                "{:?} x-range [{}, {}] escapes plane extent {}",
                a.text,
                a.x,
                a.x + ANNOTATION_SIZE,
                e.x
            );
            assert!(
                a.y >= -e.y && a.y + ANNOTATION_SIZE <= e.y,
                "{:?} y-range [{}, {}] escapes plane extent {}",
                a.text,
                a.y,
                a.y + ANNOTATION_SIZE,
                e.y
            );
        }

        // The game DROPS overlapping bricks at load, so a comment's carrier
        // brick must clear every gate's footprint.
        for a in &l.annotations {
            for (id, p) in &l.placements {
                let (hsx, hsy) = measured_half_size(&m.nodes[id], &l);
                let disjoint = a.z != p.z
                    || a.x + ANNOTATION_SIZE <= p.x
                    || p.x + hsx * 2 <= a.x
                    || a.y + ANNOTATION_SIZE <= p.y
                    || p.y + hsy * 2 <= a.y;
                assert!(disjoint, "comment {:?} overlaps node {id}", a.text);
            }
        }
    }

    #[test]
    fn each_comment_lands_on_exactly_one_plane() {
        let src = "// file note\nin go: exec\nchip C(t: exec) {\n  on t {\n    PrintToConsole(\"a\")\n    // inner note\n    PrintToConsole(\"b\")\n  }\n}\nlet c = C(go)\n";
        let m = lowered(src);
        let o = opts_with_map(src);
        let texts = |l: &LayoutResult| -> Vec<String> {
            l.annotations.iter().map(|a| a.text.clone()).collect()
        };

        // The chip's own rows bracket the inner note, so the root skips it;
        // the leading file note belongs to no chip, so the root keeps it.
        assert_eq!(texts(&layout_code(&m, &o, false)), ["file note"]);

        let chip = m.chips.values().next().expect("chip module");
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        assert_eq!(texts(&layout_code(chip, &nested, false)), ["inner note"]);
    }

    /// A note inside an array literal is not rendered.
    ///
    /// A data table carries a note per row, each costing a brick and each
    /// saying much the same thing — the highest-volume, lowest-value comments
    /// on a plane. Notes outside the brackets are untouched.
    #[test]
    fn comments_inside_array_literals_are_not_rendered() {
        let src = "// kept: before the table
in go: exec
let table = [
  // dropped: a row note
  1,
  // dropped: another row note
  2,
]
// kept: after the table
on go { PrintToConsole(\"${table[0]}\") }
";
        let m = lowered(src);
        let o = opts_with_map(src);
        let l = layout_code(&m, &o, false);
        let texts: Vec<String> = l.annotations.iter().map(|a| a.text.clone()).collect();
        assert!(
            texts.iter().all(|t| !t.starts_with("dropped")),
            "array-literal notes must not reach the plane, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "kept: before the table"),
            "a note outside the brackets must still render, got {texts:?}"
        );
    }

    /// A comment is rendered by exactly ONE plane in the whole tree, even when
    /// sibling modules' line ranges OVERLAP.
    ///
    /// `line_span` is a min/max ENVELOPE, not the set of lines a module
    /// occupies. A module whose nodes are scattered — a `mod` inlined at two
    /// distant call sites, an anon chip partitioned out of a long handler —
    /// gets a window covering everything in between, and sibling windows then
    /// overlap almost entirely. The claim excludes a module's own CHILDREN,
    /// which never excludes a SIBLING, so every comment in the overlap is
    /// rendered once per sibling.
    ///
    /// Measured on a real program before the fix: 951 own-line comments in the
    /// source became 2958 comment bricks, with three sibling modules each
    /// claiming ~940 of them.
    ///
    /// Built from synthetic modules rather than lowered source because the
    /// shape is a property of the SPANS, and which nodes lowering hands to
    /// which chip is not something a fixture can pin down.
    #[test]
    fn overlapping_sibling_modules_do_not_each_claim_the_same_comment() {
        // Two chips whose own rows are sparse and far apart: both start at
        // line 3 and run to 11 and 16. Their envelopes overlap over 3..11.
        let a = module_with(
            vec![
                make_node("G", make_range("test", 3, 0, 0, 1)),
                make_node("G", make_range("test", 11, 0, 2, 3)),
            ],
            vec![],
        );
        let b = module_with(
            vec![
                make_node("G", make_range("test", 3, 0, 4, 5)),
                make_node("G", make_range("test", 16, 0, 6, 7)),
            ],
            vec![],
        );

        let a_id = NodeId::fresh();
        let b_id = NodeId::fresh();
        let mut root = module_with(
            vec![
                make_node("G", make_range("test", 1, 0, 8, 9)),
                make_node("G", make_range("test", 18, 0, 10, 11)),
            ],
            vec![],
        );
        root.chips.insert(a_id, a);
        root.chips.insert(b_id, b);

        // A file whose line 10 carries an own-line comment, inside both
        // chips' envelopes and inside neither's children.
        let src = "








// shared note








";
        let o = opts_with_map(src);
        let anchor: Arc<str> = "test".into();

        // Ownership is settled for the whole tree, then each plane reads it —
        // the same path `layout_code` takes.
        let o = LayoutOptions {
            comment_owner: Some(Arc::new(assign_comment_owners(&root, &o, &anchor))),
            ..o
        };
        let nested = |chip: NodeId| LayoutOptions {
            nested: true,
            self_chip: Some(chip),
            ..o.clone()
        };
        let (na, nb) = (nested(a_id), nested(b_id));
        let root_claims = claimed_comments(&root, &o, &anchor);
        let a_claims = claimed_comments(&root.chips[&a_id], &na, &anchor);
        let b_claims = claimed_comments(&root.chips[&b_id], &nb, &anchor);

        let claimants: Vec<&str> = [
            ("root", root_claims.contains_key(&10)),
            ("chip_a", a_claims.contains_key(&10)),
            ("chip_b", b_claims.contains_key(&10)),
        ]
        .into_iter()
        .filter(|(_, has)| *has)
        .map(|(n, _)| n)
        .collect();

        assert_eq!(
            claimants.len(),
            1,
            "the note on line 10 must be rendered by exactly one plane, got              {claimants:?}"
        );
    }



    /// Entry file `main.ws` (lines 1..=7) and imported `lib.ws` (lines
    /// 1..=9): the note on main.ws line 6 also falls inside the imported
    /// chip's own lib.ws span, so a plane matching comment lines against
    /// rows without checking whose file they number renders it twice.
    const IMPORTED_CHIP_LIB: &str =
        "chip Helper(t: exec) {\n  var n: int = 0\n  on t {\n    n = n + 1\n    n = n * 2\n    n = n + 3\n    n = n - 4\n  }\n}\n";
    const IMPORTED_CHIP_MAIN: &str =
        "import { Helper } from \"lib\"\n\nin go: exec\n\n\n// a note on line six\nlet h = Helper(go)\n";

    #[test]
    fn a_comment_is_not_reclaimed_by_a_plane_from_another_file() {
        let (m, o) = lowered_with_imports(IMPORTED_CHIP_MAIN, &[("lib.ws", IMPORTED_CHIP_LIB)]);
        let texts = |l: &LayoutResult| -> Vec<String> {
            l.annotations.iter().map(|a| a.text.clone()).collect()
        };

        assert_eq!(texts(&layout_code(&m, &o, false)), ["a note on line six"]);

        let chip = m.chips.values().next().expect("imported chip module");
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        assert!(
            texts(&layout_code(chip, &nested, false)).is_empty(),
            "a plane anchored on lib.ws claims none of main.ws's comments"
        );
    }

    #[test]
    fn indent_comes_from_the_planes_own_file_not_the_entry_files_map() {
        // lib.ws lines 4 and 5 share a source column, so the chip's rows for
        // them share a left margin. main.ws indents its own line 4 by eight
        // columns and its line 5 by none — reading the entry file's map on a
        // lib.ws-anchored plane would stagger the two rows by that much.
        let lib = "chip Helper(t: exec) {\n  var n: int = 0\n  on t {\n    n = n + 1\n    n = n + 2\n  }\n}\n";
        let main = "import { Helper } from \"lib\"\n\nin go: exec\n        let h = Helper(go)\nout done = h\n";
        let (m, o) = lowered_with_imports(main, &[("lib.ws", lib)]);
        let map = o.source_map.as_ref().expect("entry source map");
        assert_eq!(
            (map.line_indent[3], map.line_indent[4]),
            (8, 0),
            "fixture must actually stagger the entry file's lines 4 and 5"
        );

        let chip = m.chips.values().next().expect("imported chip module");
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        let l = layout_code(chip, &nested, false);
        assert_eq!(
            head_y(chip, &l, 4),
            head_y(chip, &l, 5),
            "two lib.ws lines at the same column must share a left margin"
        );
    }

    #[test]
    fn a_comment_in_a_doubly_nested_chip_is_claimed_once() {
        // The outer chip's own rows are just the inner chip's node, so a
        // module's claim has to account for its grandchildren's rows too or
        // both the root and the inner chip render this note.
        let src = "var g: int = 0\nin go: exec\non go {\n  chip {\n    chip {\n      g = g + 1\n      // deep note\n      g = g + 2\n    }\n  }\n}\n";
        let m = lowered(src);
        let o = opts_with_map(src);
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        let texts = |l: &LayoutResult| -> Vec<String> {
            l.annotations.iter().map(|a| a.text.clone()).collect()
        };

        let outer = m.chips.values().next().expect("outer anon chip");
        let inner = outer.chips.values().next().expect("inner anon chip");

        assert!(texts(&layout_code(&m, &o, false)).is_empty(), "root");
        assert!(
            texts(&layout_code(outer, &nested, false)).is_empty(),
            "outer chip"
        );
        assert_eq!(texts(&layout_code(inner, &nested, false)), ["deep note"]);
    }

    #[test]
    fn trailing_comments_are_not_rendered() {
        let src = "in go: exec\non go { } // trailing\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        assert!(l.annotations.is_empty());
    }

    #[test]
    fn dag_mode_emits_no_annotations() {
        let m = lowered("in go: exec\n// note\non go { }\n");
        let l = crate::layout::layout(&m);
        assert!(l.annotations.is_empty());
    }

    #[test]
    fn blank_lines_leave_a_gap_and_clamp_at_two() {
        let gap_for = |later_line: u32| -> i32 {
            let a = make_node("G", make_range("f", 1, 0, 0, 1));
            let b = make_node("G", make_range("f", later_line, 0, 100, 101));
            let (hsx, _) = brick_half_size(&a);
            let a_h = hsx * 2;
            let (a_id, b_id) = (a.id, b.id);
            let m = module_with(vec![a, b], vec![]);
            let l = layout_code(&m, &opts(), false);
            (l.placements[&a_id].x - l.placements[&b_id].x) - a_h
        };
        // one blank line (line 2) between occupied lines 1 and 3.
        assert_eq!(gap_for(3), 1 * EMPTY_LINE_HEIGHT);
        // eight blank lines between occupied lines 1 and 10, clamped to 2.
        assert_eq!(gap_for(10), 2 * EMPTY_LINE_HEIGHT);
    }

    #[test]
    fn foreign_file_node_adopts_consumer_line() {
        let a = make_node("G", make_range("main", 3, 0, 50, 51));
        let b = make_node("G", make_range("other", 1, 0, 0, 1));
        let (a_id, b_id) = (a.id, b.id);
        let m = module_with(vec![a, b], vec![wire(b_id, a_id)]);
        let l = layout_code(&m, &opts(), false);
        assert_eq!(l.placements[&a_id].x, l.placements[&b_id].x);
        // Adopted onto its consumer's row, the producer reads first: left of
        // the node it feeds.
        assert!(l.placements[&b_id].y < l.placements[&a_id].y);
    }

    #[test]
    fn synthetic_default_range_node_adopts_transitively() {
        let a = make_node("G", make_range("main", 5, 0, 200, 201));
        let b = make_node("G", SourceRange::default());
        let c = make_node("G", SourceRange::default());
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let m = module_with(vec![a, b, c], vec![wire(c_id, b_id), wire(b_id, a_id)]);
        let l = layout_code(&m, &opts(), false);
        assert_eq!(l.placements[&b_id].x, l.placements[&a_id].x);
        assert_eq!(l.placements[&c_id].x, l.placements[&a_id].x);
    }

    #[test]
    fn consumerless_homeless_node_lands_on_overflow_row() {
        let a = make_node("G", make_range("main", 1, 0, 0, 1));
        let b = make_node("G", make_range("main", 2, 0, 10, 11));
        let c = make_node("G", SourceRange::default());
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let m = module_with(vec![a, b, c], vec![]);
        let l = layout_code(&m, &opts(), false);
        let last_source_x = l.placements[&a_id].x.min(l.placements[&b_id].x);
        assert!(l.placements[&c_id].x < last_source_x);
    }

    #[test]
    fn long_line_soft_wraps_into_indented_continuation_row() {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for i in 0..5 {
            let off = i * 10;
            let n = make_node(
                "G",
                make_range("f", 1, 0, off, off + 1),
            );
            ids.push(n.id);
            nodes.push(n);
        }
        let d = make_node("G", make_range("f", 2, 0, 1000, 1001));
        let d_id = d.id;
        nodes.push(d);
        let m = module_with(nodes, vec![]);
        // Each node is its own group: 10 wide plus the column's tap
        // reserve, so three fit in 45 and the fourth wraps.
        let budgets = CodeBudgets {
            line_width: 3 * (10 + TAP_RESERVE),
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        let xs: Vec<i32> = ids.iter().map(|id| l.placements[id].x).collect();
        let ys: Vec<i32> = ids.iter().map(|id| l.placements[id].y).collect();

        // first three share a sub-row, last two share a lower sub-row.
        assert_eq!(xs[0], xs[1]);
        assert_eq!(xs[1], xs[2]);
        assert_eq!(xs[3], xs[4]);
        assert!(xs[3] < xs[0], "continuation sub-row must sit lower");

        // continuation sub-row starts at indent (0) + CONTINUATION_INDENT.
        assert_eq!(ys[3] - ys[0], CONTINUATION_INDENT);

        // the next source line shifts down by both sub-rows' heights.
        let (hsx, _) = brick_half_size(&m.nodes[&ids[0]]);
        let row_h = hsx * 2;
        assert_eq!(xs[0] - l.placements[&d_id].x, 2 * row_h);
    }

    #[test]
    fn every_spawnable_node_gets_a_placement() {
        let m = lowered(
            "var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }\nout y = x\nchip { var inner: int = 0 }\n",
        );
        let spawnable_count = m
            .nodes
            .values()
            .filter(|n| is_spawnable(n))
            .count();
        let l = layout_code(&m, &opts(), false);
        assert_eq!(l.placements.len(), spawnable_count);
        for n in m.nodes.values().filter(|n| is_spawnable(n)) {
            assert!(
                l.placements.contains_key(&n.id),
                "node {} missing a placement",
                n.id
            );
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let m = lowered("var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }\n");
        let a = layout_code(&m, &opts(), false);
        let b = layout_code(&m, &opts(), false);
        assert_eq!(a.placements, b.placements);
        // A run that placed the same cells but turned a different set of
        // bricks would still hand emit a different build.
        assert_eq!(a.rotations, b.rotations);
        assert!(!a.rotations.is_empty(), "fixture must rotate something");
    }

    #[test]
    fn empty_module_returns_empty_layout() {
        let l = layout_code(&Module::new("empty"), &opts(), false);
        assert!(l.placements.is_empty());
        assert_eq!(l.bounds_min, IntVec3::default());
        assert_eq!(l.bounds_max, IntVec3::default());
    }

    /// Four 10-high one-node lines (1..=4), band budget 25 → bands split
    /// 2/2; line/plane budgets stay default so nothing else wraps.
    fn band_split_module() -> (Module, Vec<NodeId>) {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for i in 0..4u32 {
            let off = (i as usize) * 10;
            let n = make_node("G", make_range("f", i + 1, 0, off, off + 1));
            ids.push(n.id);
            nodes.push(n);
        }
        (module_with(nodes, vec![]), ids)
    }

    /// Six 10-high one-node lines, band budget 25 → three 2-line bands,
    /// each 10 wide plus its column's tap reserve; `PAGE_BUDGET` fits two
    /// of them with a gutter between, so the third band starts page 1.
    /// Two bands wide, gutter included.
    const PAGE_BUDGET: i32 = 2 * (10 + TAP_RESERVE) + BAND_GUTTER;

    fn paginated_module() -> (Module, Vec<NodeId>) {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for i in 0..6u32 {
            let off = (i as usize) * 10;
            let n = make_node("G", make_range("f", i + 1, 0, off, off + 1));
            ids.push(n.id);
            nodes.push(n);
        }
        (module_with(nodes, vec![]), ids)
    }

    /// A real lowered body that both paginates AND earns lanes.
    ///
    /// `paginated_module` is synthetic and carries NO WIRES, so every
    /// pagination test built on it runs against an empty bus — pagination is
    /// the one place lanes are allocated per page, off each page's own left
    /// edge, and nothing was checking that. This is the fixture that is.
    fn paginated_bus_module() -> (Module, CodeBudgets) {
        (
            lowered(BAND_SRC),
            CodeBudgets {
                band_height: 40,
                plane_width: 120,
                ..CodeBudgets::default()
            },
        )
    }

    #[test]
    fn band_wrap_moves_overflow_lines_to_a_second_column() {
        let (m, ids) = band_split_module();
        let budgets = CodeBudgets {
            band_height: 25,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        let p: Vec<_> = ids.iter().map(|id| l.placements[id]).collect();
        let (_, hsy) = brick_half_size(&m.nodes[&ids[0]]);
        let node_w = hsy * 2;

        let band1_max_y = p[0].y.max(p[1].y) + node_w;
        assert!(
            p[2].y >= band1_max_y + BAND_GUTTER,
            "3rd line ({}) must sit right of band 1's extent ({}) + gutter",
            p[2].y,
            band1_max_y
        );
        assert!(p[3].y >= band1_max_y + BAND_GUTTER);

        assert_eq!(p[2].x, p[0].x, "band 2's first line restarts at the top");
        assert_eq!(p[3].x, p[1].x);
        assert!(p[1].x < p[0].x);
        assert!(p.iter().all(|pl| pl.z == Z_PLANE), "single page only");
    }

    #[test]
    fn page_wrap_stacks_bands_in_z_with_page_step() {
        let (m, ids) = paginated_module();
        let budgets = CodeBudgets {
            band_height: 25,
            plane_width: PAGE_BUDGET,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        for id in &ids[..4] {
            assert_eq!(l.placements[id].z, Z_PLANE, "first two bands stay on page 0");
        }
        for id in &ids[4..] {
            assert_eq!(
                l.placements[id].z,
                Z_PLANE + PAGE_Z_STEP,
                "overflow band lands on page 1"
            );
        }
    }

    #[test]
    fn bounds_cover_all_pages() {
        let (m, _) = paginated_module();
        let budgets = CodeBudgets {
            band_height: 25,
            plane_width: PAGE_BUDGET,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        assert_eq!(l.bounds_min.z, Z_PLANE);
        assert_eq!(l.bounds_max.z, Z_PLANE + PAGE_Z_STEP);
        for p in l.placements.values() {
            assert!(p.x >= l.bounds_min.x && p.x <= l.bounds_max.x);
            assert!(p.y >= l.bounds_min.y && p.y <= l.bounds_max.y);
            assert!(p.z >= l.bounds_min.z && p.z <= l.bounds_max.z);
        }

        // The emitted microchip grid's PlaneExtent (centered on PlaneCenter
        // `(0, 0, 0)`, see `emit::build_world`) must also actually contain
        // every page — bounds alone don't guarantee that since the grid's
        // extent is a separate, derived quantity.
        let extent = crate::layout::wall::plane_extent(&l);
        for p in l.placements.values() {
            assert!(
                p.x >= -extent.x && p.x <= extent.x,
                "x {} outside plane extent {}",
                p.x,
                extent.x
            );
            assert!(
                p.y >= -extent.y && p.y <= extent.y,
                "y {} outside plane extent {}",
                p.y,
                extent.y
            );
            assert!(
                p.z >= -extent.z && p.z <= extent.z,
                "z {} outside plane extent {} (page z-stacking must fit the plane)",
                p.z,
                extent.z
            );
        }
    }

    /// A port's declared label (`PortLabel`), for asserting stack order.
    fn label_of(n: &Node) -> String {
        match n.properties.get(&*crate::intern::sym::PORT_LABEL) {
            Some(crate::ir::Literal::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    /// Two declared value params behind an exec in, two declared outs, and
    /// two synthesized boundary pins (the body reads outer globals). The
    /// params are fed from globals rather than literals so the fold pass
    /// can't collapse them out of the signature.
    const PORTS_SRC: &str = "var g1: int = 1\nvar g2: int = 2\nin go: exec\nchip C(t: exec, a: int, b: int) -> (x: int, y: int) {\n  out x = a + g1\n  out y = b + g2\n}\nlet c = C(go, g1, g2)\n";

    #[test]
    fn declared_ports_sit_on_the_plane_edges_in_signature_order() {
        let m = lowered(PORTS_SRC);
        let chip = m.chips.values().next().expect("chip module");
        let l = layout_code(chip, &LayoutOptions::default(), false);

        let ids_of = |k: NodeKind| -> Vec<NodeId> {
            let mut v: Vec<NodeId> = chip
                .nodes
                .values()
                .filter(|n| n.kind == k)
                .map(|n| n.id)
                .collect();
            v.sort_by_key(|id| Reverse(l.placements[id].x)); // top-down
            v
        };
        let inputs = ids_of(NodeKind::Input);
        let outputs = ids_of(NodeKind::Output);
        assert!(!inputs.is_empty() && !outputs.is_empty());

        // Every input is left of every non-port node; every output is right of them.
        let body_ys: Vec<i32> = chip
            .nodes
            .values()
            .filter(|n| !matches!(n.kind, NodeKind::Input | NodeKind::Output))
            .filter_map(|n| l.placements.get(&n.id).map(|p| p.y))
            .collect();
        let body_min = *body_ys.iter().min().unwrap();
        let body_max = *body_ys.iter().max().unwrap();
        for id in &inputs {
            assert!(
                l.placements[id].y < body_min,
                "input {id} not on the left edge"
            );
        }
        for id in &outputs {
            assert!(
                l.placements[id].y > body_max,
                "output {id} not on the right edge"
            );
        }

        // Stacks descend in signature order: declared ports in declaration
        // order first, then synthesized boundary pins by label. (Every
        // declared port shares the chip signature's source offset, so the
        // labels — not the offsets — are what pin down declaration order.)
        let labels = |ids: &[NodeId]| -> Vec<String> {
            ids.iter().map(|id| label_of(&chip.nodes[id])).collect()
        };
        assert_eq!(labels(&inputs), ["t", "a", "b", "g1", "g2"]);
        assert_eq!(labels(&outputs), ["x", "y"]);

        // Declared entries precede synthesized ones, and are themselves in
        // non-decreasing source order.
        let declared: Vec<usize> = inputs
            .iter()
            .filter(|id| has_range(&chip.nodes[id]))
            .map(|id| chip.nodes[id].source_range.start.offset)
            .collect();
        assert_eq!(declared.len(), 3, "t/a/b are the declared inputs");
        assert!(
            declared.windows(2).all(|w| w[0] <= w[1]),
            "declared inputs not in signature order: {declared:?}"
        );
        let synth_first_x = inputs
            .iter()
            .filter(|id| !has_range(&chip.nodes[id]))
            .map(|id| l.placements[id].x)
            .max()
            .unwrap();
        let declared_last_x = inputs
            .iter()
            .filter(|id| has_range(&chip.nodes[id]))
            .map(|id| l.placements[id].x)
            .min()
            .unwrap();
        assert!(
            synth_first_x < declared_last_x,
            "synthesized pins must stack below every declared port"
        );
    }

    /// Routing declared ports to the edges lets a chip end up with no body
    /// nodes at all, so the page list comes back empty — a state the row
    /// model could not previously reach, since declared ports used to
    /// occupy a source row themselves.
    #[test]
    fn a_ports_only_chip_still_places_and_stays_inside_the_plane() {
        let m = lowered(
            "var g: int = 1\nin go: exec\nchip C(t: exec, a: int) -> (x: int) {\n  out x = a\n}\nlet c = C(go, g)\n",
        );
        let chip = m.chips.values().next().expect("chip module");
        let ports: Vec<&Node> = chip.nodes.values().filter(|n| is_spawnable(n)).collect();
        assert!(
            ports.iter().all(|n| is_edge_pin(n)),
            "fixture sanity: this chip must be nothing but ports"
        );

        let l = layout_code(chip, &LayoutOptions::default(), false);
        assert_eq!(l.placements.len(), ports.len());

        // Inputs still land left of outputs, and the plane still contains
        // everything.
        let in_y = ports
            .iter()
            .filter(|n| n.kind == NodeKind::Input)
            .map(|n| l.placements[&n.id].y)
            .max()
            .unwrap();
        let out_y = ports
            .iter()
            .filter(|n| n.kind == NodeKind::Output)
            .map(|n| l.placements[&n.id].y)
            .min()
            .unwrap();
        assert!(in_y < out_y, "inputs must stay left of outputs");

        let e = crate::layout::wall::plane_extent(&l);
        for n in &ports {
            let p = l.placements[&n.id];
            let (hsx, hsy) = measured_half_size(n, &l);
            assert!(p.x >= -e.x && p.x + hsx * 2 <= e.x);
            assert!(p.y >= -e.y && p.y + hsy * 2 <= e.y);
        }
        assert_no_overlap(chip, &l);
    }

    #[test]
    fn port_stacks_start_at_the_page_top() {
        let m = lowered(
            "var g1: int = 1\nin go: exec\nchip C(t: exec, a: int) -> (x: int) {\n  out x = a + g1\n}\nlet c = C(go, g1)\n",
        );
        let chip = m.chips.values().next().expect("chip module");
        let l = layout_code(chip, &LayoutOptions::default(), false);
        let top = l.placements.values().map(|p| p.x).max().unwrap();
        let first_in = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Input && has_range(n))
            .min_by_key(|n| (n.source_range.start.offset, n.id))
            .unwrap();
        assert_eq!(
            l.placements[&first_in.id].x, top,
            "first declared input must sit at the page's top row"
        );

        // ...and the rest of the stack descends from it, one row per port.
        // Declared ports all share the chip signature's source line, so
        // literal placement would pile them onto a single row instead.
        let mut inputs: Vec<&Node> = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Input)
            .collect();
        assert!(inputs.len() >= 2, "fixture sanity: needs a multi-pin stack");
        inputs.sort_by_key(|n| Reverse(l.placements[&n.id].x));
        for w in inputs.windows(2) {
            let (hsx, _) = brick_half_size(w[1]);
            assert_eq!(
                l.placements[&w[1].id].x,
                l.placements[&w[0].id].x - hsx * 2,
                "input stack must descend one pin height per entry"
            );
        }

        // The output stack starts at the same top row.
        let first_out = chip
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Output)
            .unwrap();
        assert_eq!(l.placements[&first_out.id].x, top);
    }

    #[test]
    fn every_brick_fits_inside_the_plane_extent() {
        // A chip whose interior reads two outer globals: inbound pins only,
        // so the pin stack extends left with nothing balancing it on the right.
        let m = lowered(
            "var a: int = 1\nvar b: int = 2\nin go: exec\nchip C(t: exec) { on t { PrintToConsole(\"${a} ${b}\") } }\nlet c = C(go)\n",
        );
        let chip = m.chips.values().next().expect("chip module");
        let l = layout_code(chip, &LayoutOptions::default(), false);
        let e = crate::layout::wall::plane_extent(&l);
        for (id, p) in &l.placements {
            let (hsx, hsy) = measured_half_size(&chip.nodes[id], &l);
            assert!(
                p.x >= -e.x && p.x + hsx * 2 <= e.x,
                "node {id} x-range [{}, {}] escapes plane extent {}",
                p.x,
                p.x + hsx * 2,
                e.x
            );
            assert!(
                p.y >= -e.y && p.y + hsy * 2 <= e.y,
                "node {id} y-range [{}, {}] escapes plane extent {}",
                p.y,
                p.y + hsy * 2,
                e.y
            );
            assert!(
                p.z <= e.z,
                "node {id} z {} escapes plane extent {}",
                p.z,
                e.z
            );
        }
    }

    const ANON_CHIP_SRC: &str =
        "var g: int = 7\nin t: exec\nchip {\n  var h: int = 0\n  on t { h = h + g }\n}\nout o = h\n";

    fn is_boundary_pin(n: &Node) -> bool {
        n.note == Some("boundary_pin")
    }

    #[test]
    fn synthesized_pins_stack_on_edges() {
        // Anonymous chip whose interior reads global g and whose state (h)
        // is read at root: the chip module gets 2 boundary MicrochipInputs
        // and 2 boundary MicrochipOutputs (verified against dumped IR).
        let root = lowered(ANON_CHIP_SRC);
        let chip = root.chips.values().next().expect("one chip");
        let l = layout_code(chip, &opts(), false);

        let boundary_ins: Vec<NodeId> = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Input && is_boundary_pin(n))
            .map(|n| n.id)
            .collect();
        let boundary_outs: Vec<NodeId> = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Output && is_boundary_pin(n))
            .map(|n| n.id)
            .collect();
        assert_eq!(boundary_ins.len(), 2, "fixture sanity");
        assert_eq!(boundary_outs.len(), 2, "fixture sanity");

        let main_ys: Vec<i32> = chip
            .nodes
            .values()
            .filter(|n| is_spawnable(n) && !is_boundary_pin(n))
            .map(|n| l.placements[&n.id].y)
            .collect();
        let main_min = *main_ys.iter().min().unwrap();
        let main_max = *main_ys.iter().max().unwrap();

        for id in &boundary_ins {
            assert!(
                l.placements[id].y < main_min,
                "input pin must sit left of every main node"
            );
        }
        for id in &boundary_outs {
            assert!(
                l.placements[id].y > main_max,
                "output pin must sit right of every main node"
            );
        }

        // The global y extremes belong to the edge stacks.
        let min_id = *l.placements.iter().min_by_key(|(_, p)| p.y).unwrap().0;
        let max_id = *l.placements.iter().max_by_key(|(_, p)| p.y).unwrap().0;
        assert!(boundary_ins.contains(&min_id));
        assert!(boundary_outs.contains(&max_id));

        // Two pins anchored to the same consumer must stack, not collide.
        let in_xs: Vec<i32> = boundary_ins.iter().map(|id| l.placements[id].x).collect();
        assert_ne!(in_xs[0], in_xs[1], "same-anchor pins must bump apart");

        // Coverage and determinism now include the edge-pin path.
        let spawnable_count = chip.nodes.values().filter(|n| is_spawnable(n)).count();
        assert_eq!(l.placements.len(), spawnable_count);
        let l2 = layout_code(chip, &opts(), false);
        assert_eq!(l.placements, l2.placements);

        // Declared in/out pins (real ranges) join the edge stacks too,
        // ahead of the synthesized pins.
        let named = lowered(
            "var g: int = 7\nin t: exec\nchip C(u: exec) -> (r: int) {\n  var h: int = 0\n  on u { h = h + g }\n  out r = h\n}\nlet c = C(t)\nout o = c\n",
        );
        let cm = named.chips.values().next().expect("one chip");
        let l3 = layout_code(cm, &opts(), false);

        let declared_in = cm
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Input && has_range(n))
            .expect("declared u pin");
        let declared_out = cm
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Output && has_range(n))
            .expect("declared r pin");
        let boundary_in = cm
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Input && is_boundary_pin(n))
            .expect("boundary g pin");

        // Each stack starts at the page's top row, so the sole declared
        // input and the sole declared output both hold it.
        let top_x = cm
            .nodes
            .values()
            .filter(|n| is_spawnable(n) && !is_boundary_pin(n))
            .map(|n| l3.placements[&n.id].x)
            .max()
            .unwrap();
        assert_eq!(l3.placements[&declared_in.id].x, top_x);
        assert_eq!(l3.placements[&declared_out.id].x, top_x);

        // The boundary input shares the left edge with the declared input
        // (same column) and stacks below it, rather than sitting further
        // left on a row of its own.
        assert_eq!(
            l3.placements[&boundary_in.id].y,
            l3.placements[&declared_in.id].y
        );
        assert!(l3.placements[&boundary_in.id].x < l3.placements[&declared_in.id].x);

        // The declared output is on the right edge — right of every body
        // node, including the rightmost one.
        let var_get = cm
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_GET)
            .unwrap();
        assert!(l3.placements[&declared_out.id].y > l3.placements[&var_get.id].y);
    }

    #[test]
    fn a_pin_stack_orders_ext_labels_by_number() {
        // Plain string order files `ext10` between `ext1` and `ext2`.
        let mut labels = ["ext10", "ext2", "ext1", "score", "ext"];
        labels.sort_by_key(|l| label_sort_key(l));
        assert_eq!(labels, ["ext", "ext1", "ext2", "ext10", "score"]);
    }

    /// The footprint a node actually occupies once placed — the oracle every
    /// overlap and containment sweep below measures with.
    ///
    /// The swap is spelled out rather than delegated to `cell_half_size`: an
    /// oracle that reuses the function under test stays green if that
    /// function stops swapping, which is exactly the regression this guards.
    ///
    /// Only the QUARTER turns swap. `Deg180` and `Deg270` are listed
    /// explicitly rather than swept into a catch-all: a `_` arm here would
    /// silently hand a future variant whichever footprint it happened to sit
    /// next to, and which side of this split a facing lands on is exactly the
    /// mistake these sweeps exist to catch.
    fn measured_half_size(node: &Node, lr: &LayoutResult) -> (i32, i32) {
        let (bx, by) = brick_half_size(node);
        match rotation_of(&lr.rotations, &node.id) {
            NodeRotation::Deg0 | NodeRotation::Deg180 => (bx, by),
            NodeRotation::Deg90 | NodeRotation::Deg270 => (by, bx),
        }
    }

    /// The half-extent of a bus node's brick, re-spelled from the brick
    /// itself: `emit_bus` places `B_1x1_Reroute_Node` — a 2×2×2 — centred one
    /// unit off the layout's min corner on each axis, and the footprint is
    /// square so rotation does not swap it.
    ///
    /// Deliberately not `REROUTER_HALF`: an oracle measuring with the
    /// constant under test goes green the moment that constant stops
    /// describing the brick, which is exactly the disagreement these sweeps
    /// exist to catch.
    const MEASURED_BUS_HALF: i32 = 1;

    /// The overlap sweep, sized the way the bricks actually land: a `Deg90`
    /// node's half-sizes are swapped. `check_overlaps` cannot do this —
    /// `brdb::Brick::local_bounds()` ignores rotation — so this is the gate
    /// that catches a layout/emit footprint disagreement.
    ///
    /// The gutter bus carries a brick per node too, and a lane's taps stand
    /// inside the body's rows, so they are swept with everything else.
    ///
    /// So do the comment annotations. Their carrier is an invisible 1×1
    /// `PB_DefaultBrick`, which the game drops on overlap exactly like a
    /// visible one — and being invisible is precisely why nobody would notice
    /// it had landed inside a gate.
    fn assert_no_overlap(module: &Module, lr: &LayoutResult) {
        let mut boxes: Vec<(String, i32, i32, i32, i32, i32)> = lr
            .placements
            .iter()
            .filter_map(|(id, p)| {
                let n = module.nodes.get(id)?;
                let (hx, hy) = measured_half_size(n, lr);
                Some((format!("{id:?}"), p.x, p.x + hx * 2, p.y, p.y + hy * 2, p.z))
            })
            .collect();
        boxes.extend(lr.bus.nodes.iter().enumerate().map(|(i, n)| {
            (
                format!("bus node {i}"),
                n.x,
                n.x + MEASURED_BUS_HALF * 2,
                n.y,
                n.y + MEASURED_BUS_HALF * 2,
                n.z,
            )
        }));
        boxes.extend(lr.annotations.iter().enumerate().map(|(i, a)| {
            (
                format!("annotation {i} ({:?})", a.text),
                a.x,
                a.x + ANNOTATION_SIZE,
                a.y,
                a.y + ANNOTATION_SIZE,
                a.z,
            )
        }));
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let disjoint = a.5 != b.5 || a.2 <= b.1 || b.2 <= a.1 || a.4 <= b.3 || b.4 <= a.3;
                assert!(disjoint, "{} overlaps {} (rotation-aware)", a.0, b.0);
            }
        }
    }

    /// The test-side spelling of the in-line consumer relation: does any
    /// node on `n`'s own line read the value `n` produces? Re-derived from
    /// the module's wires rather than borrowed from `line_groups`, so it
    /// cannot go green alongside a grouping bug.
    fn value_is_read_on_its_line(m: &Module, n: &Node) -> bool {
        let line = n.source_range.start.line;
        m.wires.iter().any(|w| {
            w.source.node_id == n.id
                && w.source.port != WirePort::Layout
                && w.target.port != WirePort::Layout
                && !targets_exec(m, &w.target)
                && m.nodes
                    .get(&w.target.node_id)
                    .is_some_and(|t| t.source_range.start.line == line)
        })
    }

    /// The sub-row a placed node sits in, named by that row's top edge —
    /// the one value every node in a row shares whatever its own height.
    fn row_top(node: &Node, l: &LayoutResult) -> i32 {
        l.placements[&node.id].x + measured_half_size(node, l).0 * 2
    }

    /// The whole spine rule, both halves, over every exec gate the fixture
    /// lowers: a gate turns exactly when it takes an exec input and nothing
    /// on its line reads its value back.
    #[test]
    fn exec_gates_turn_exactly_when_they_are_their_lines_sink() {
        let src =
            "var a: int = 1\nin go: exec\non go {\n  a = a + 1\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let mut sinks = 0usize;
        let mut reads = 0usize;
        for n in m
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Gate && l.placements.contains_key(&n.id))
        {
            let want = if takes_exec_input(n) && !value_is_read_on_its_line(&m, n) {
                sinks += 1;
                NodeRotation::Deg90
            } else {
                if takes_exec_input(n) {
                    reads += 1;
                }
                NodeRotation::Deg0
            };
            assert_eq!(
                rotation_of(&l.rotations, &n.id),
                want,
                "{} (exec in = {}, value read on its line = {})",
                n.gate_class,
                takes_exec_input(n),
                value_is_read_on_its_line(&m, n)
            );
        }
        assert!(sinks > 0, "fixture must lower a statement sink");
        assert!(reads > 0, "fixture must lower a value-producing exec read");
    }

    /// A read-heavy statement must not grow a sub-row per read.
    ///
    /// The line is the reads' own stacked column plus the statement row the
    /// sink dropped to — three. Pinning every exec gate to column 0 would put
    /// both reads in the spine column under the `PrintToConsole` instead of
    /// beside it, and the same drop would then land the sink below all three:
    /// four rows. That is the regression this counts.
    #[test]
    fn reads_do_not_add_a_sub_row_to_their_statement() {
        let src = "var a: int = 1\nvar b: int = 2\nin go: exec\non go {\n  PrintToConsole(\"${a} ${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let line = m
            .nodes
            .values()
            .find(|n| n.gate_class.ends_with("PrintToConsole"))
            .expect("PrintToConsole node")
            .source_range
            .start
            .line;
        let on_line: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| n.source_range.start.line == line && l.placements.contains_key(&n.id))
            .collect();
        let reads = on_line
            .iter()
            .filter(|n| n.gate_class.ends_with("Exec_Var_Get"))
            .count();
        assert_eq!(reads, 2, "fixture must lower two in-line reads");
        let rows: HashSet<i32> = on_line.iter().map(|n| row_top(n, &l)).collect();
        assert_eq!(
            rows.len(),
            3,
            "the two reads share one value column, so the line is that column \
             plus the sink's own row — not a row per read"
        );
    }

    /// The narrow half of the rule, pinned on its own. An `Exec_Var_Get`
    /// inside an interpolation takes an exec input just like the
    /// `PrintToConsole` it feeds, but it belongs to the value flow: pinning
    /// it to column 0 would turn it AND push it onto a second sub-row,
    /// making every read-heavy line taller for nothing.
    #[test]
    fn a_value_producing_exec_read_stays_in_the_value_columns() {
        let src = "var a: int = 1\nin go: exec\non go {\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let by_class = |c: &str| -> &Node {
            m.nodes
                .values()
                .find(|n| n.gate_class.ends_with(c))
                .unwrap_or_else(|| panic!("a {c} node"))
        };
        let read = by_class("Exec_Var_Get");
        let print = by_class("PrintToConsole");
        assert!(
            takes_exec_input(read),
            "the read must take an exec input, or this test proves nothing"
        );
        assert_eq!(
            rotation_of(&l.rotations, &read.id),
            NodeRotation::Deg0,
            "an expression-side exec read must stay horizontal"
        );
        assert!(
            l.placements[&read.id].y > l.placements[&print.id].y,
            "the read must sit RIGHT of the sink consuming it, not in column 0"
        );

        // The whole expression still resolves on ONE row — the read beside
        // the `FormatText` it feeds, not under it — and the statement takes
        // the row below. Pinning the read to column 0 would stack it under
        // the sink and make that two expression rows instead of one.
        let rows: HashSet<i32> = m
            .nodes
            .values()
            .filter(|n| {
                n.source_range.start.line == print.source_range.start.line
                    && l.placements.contains_key(&n.id)
            })
            .map(|n| row_top(n, &l))
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "one row of expression plus the statement row under it"
        );
        assert_eq!(
            row_top(read, &l),
            row_top(by_class("FormatText"), &l),
            "the read shares its row with the pure gate it feeds"
        );
    }

    /// The spine, not its operands, is what turns. `PrintToConsole` is the
    /// statement's exec gate and `FormatText` the pure gate feeding it; a
    /// depth-ordered spine puts them the other way round, leaving the pure
    /// gate on the left facing down and the exec gate on the right flat.
    #[test]
    fn the_exec_gate_turns_and_the_pure_gate_it_reads_does_not() {
        let src =
            "var a: int = 1\nin go: exec\non go {\n  a = a + 1\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let by_class = |c: &str| -> &Node {
            m.nodes
                .values()
                .find(|n| n.gate_class.ends_with(c))
                .unwrap_or_else(|| panic!("a {c} node"))
        };
        let print = by_class("PrintToConsole");
        let fmt = by_class("String_FormatText");
        assert_eq!(
            rotation_of(&l.rotations, &print.id),
            NodeRotation::Deg90,
            "the statement's exec gate must face down the spine"
        );
        assert_eq!(
            rotation_of(&l.rotations, &fmt.id),
            NodeRotation::Deg0,
            "an expression-side gate must stay horizontal"
        );
        assert!(
            l.placements[&print.id].y < l.placements[&fmt.id].y,
            "the exec gate must sit LEFT of the expression it reads"
        );
    }

    #[test]
    fn non_exec_gates_are_never_rotated() {
        let src = "var a: int = 1\nin go: exec\non go {\n  let m = (a + 1) * (a + 2)\n  PrintToConsole(\"${m}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        for (id, rot) in &l.rotations {
            let n = &m.nodes[id];
            assert!(takes_exec_input(n), "non-exec gate {id} rotated ({rot:?})");
        }
    }

    /// Only gate bricks may carry a rotation. The microchip shell is
    /// emitted by a separate path that hardcodes its 1×1 offsets and never
    /// reads `rotations`, so a rotation recorded for a chip node would be a
    /// footprint the emitter does not honour — the exact layout/emit
    /// disagreement this mechanism exists to prevent.
    ///
    /// A chip instance is exec-triggered in the source, but its IR node
    /// exposes no `Type::Exec` input port, so the rule's first clause
    /// already excludes it; restricting to `NodeKind::Gate` is a guard
    /// against that changing, and this test is what would catch it.
    #[test]
    fn only_gate_nodes_are_ever_rotated() {
        let src = "var s: int = 0\nin bump: exec\nchip Scorer(go: exec, amount: int) -> (total: int) {\n  on go { s = s + amount }\n  out total = s\n}\nlet scored = Scorer(bump, 5)\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            m.nodes
                .values()
                .any(|n| n.kind == NodeKind::Chip && l.placements.contains_key(&n.id)),
            "fixture must place a chip instance"
        );
        for id in l.rotations.keys() {
            assert_eq!(
                m.nodes[id].kind,
                NodeKind::Gate,
                "only gate bricks may be rotated; {id} is a {:?}",
                m.nodes[id].kind
            );
        }
    }

    #[test]
    fn dag_mode_sets_no_rotations() {
        let m = lowered("var a: int = 1\nin go: exec\non go { a = a + 1 }\n");
        let l = crate::layout::layout(&m);
        assert!(l.rotations.is_empty());
    }

    #[test]
    fn rotated_gates_do_not_overlap_their_neighbours() {
        // The overlap gate, run over a fixture that actually rotates something.
        let src = "var a: int = 1\nin go: exec\non go {\n  a = a + 1\n  a = a + 2\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            l.rotations.values().any(|r| *r == NodeRotation::Deg90),
            "fixture must rotate at least one gate"
        );
        assert_no_overlap(&m, &l);
    }

    /// The fixture that makes the footprint swap observable: `DisplayText`
    /// is a statement sink — nothing on its line reads it back — so it
    /// lands in column 0 and rotates, and it is 8×5, so the swap moves
    /// real geometry. Its line also carries a `Var_Get` feeding a
    /// `FormatText`, both of which stay in the value columns.
    pub(super) const WIDE_EXEC_SRC: &str = "var a: int = 1\non ControllerJoined(c) {\n  a = a + 1\n  c.DisplayText(\"hi ${a}\")\n  a = a + 2\n}\n";

    /// A square gate's swap is a no-op, so a fixture of 5×5 gates cannot
    /// tell a correct reservation from a missing one. `DisplayText` is 8×5
    /// on the exec spine: rotating it turns a 16×10 footprint into a 10×16
    /// one, and reserving the unswapped cell puts it through its
    /// neighbour.
    #[test]
    fn a_rotated_wide_exec_gate_reserves_its_swapped_cell() {
        let src = WIDE_EXEC_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let wide: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| {
                l.placements.contains_key(&n.id) && {
                    let (hx, hy) = brick_half_size(n);
                    hx != hy
                }
            })
            .collect();
        assert!(
            !wide.is_empty(),
            "fixture must lower a non-square gate, got {:?}",
            m.nodes.values().map(|n| n.gate_class).collect::<Vec<_>>()
        );
        let rotated_wide: Vec<&&Node> = wide
            .iter()
            .filter(|n| rotation_of(&l.rotations, &n.id) == NodeRotation::Deg90)
            .collect();
        assert!(
            !rotated_wide.is_empty(),
            "fixture must rotate a non-square exec gate, got {:?}",
            wide.iter()
                .map(|n| (n.gate_class, brick_half_size(n)))
                .collect::<Vec<_>>()
        );

        // The reserved cell is the swapped one.
        for n in &rotated_wide {
            let (hx, hy) = brick_half_size(n);
            assert_eq!(
                cell_half_size(n, NodeRotation::Deg90),
                (hy, hx),
                "rotated {} must reserve its swapped footprint",
                n.gate_class
            );
        }
        assert_no_overlap(&m, &l);
    }

    #[test]
    fn no_two_bricks_overlap_in_any_mode() {
        // Soft-wrapping line.
        let mut nodes = Vec::new();
        for i in 0..5 {
            let off = i * 10;
            nodes.push(make_node("G", make_range("f", 1, 0, off, off + 1)));
        }
        nodes.push(make_node("G", make_range("f", 2, 0, 1000, 1001)));
        let wrap_m = module_with(nodes, vec![]);
        let budgets = CodeBudgets {
            line_width: 30,
            ..CodeBudgets::default()
        };
        assert_no_overlap(
            &wrap_m,
            &layout_code_with_budgets(&wrap_m, &opts(), false, &budgets),
        );

        // Paginated synthetic module.
        let (page_m, _) = paginated_module();
        let budgets = CodeBudgets {
            band_height: 25,
            plane_width: PAGE_BUDGET,
            ..CodeBudgets::default()
        };
        assert_no_overlap(
            &page_m,
            &layout_code_with_budgets(&page_m, &opts(), false, &budgets),
        );

        // Paginated body with a real bus on every page.
        let (bus_m, bus_budgets) = paginated_bus_module();
        assert_no_overlap(
            &bus_m,
            &layout_code_with_budgets(&bus_m, &opts(), false, &bus_budgets),
        );

        // Full pipeline (boundary pins included): the whole chip tree, each
        // level's own band included.
        let root = lowered(ANON_CHIP_SRC);
        assert_layout_tree_is_sound(&root, &layout_code(&root, &code_opts(), true));
    }

    /// The band's bricks are swept with the body's. A lane's tap stands on
    /// the row it serves, so a tap nudged off that row would land inside a
    /// gate — and the game drops overlapping bricks silently.
    #[test]
    fn a_full_band_never_overlaps_the_body() {
        let src = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${b}\")\n  PrintToConsole(\"${a}\")\n  b = a + b\n  log.push(\"y${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            l.bus.nodes.len() > 8,
            "fixture must build a band worth sweeping, got {} nodes",
            l.bus.nodes.len()
        );
        assert_no_overlap(&m, &l);
    }

    /// A lane's vertical run passes only through down-pointing rerouters. A
    /// right-pointing tap is a BRANCH hanging off one of them — it carries
    /// the value out to a row and stops there, and must never hand the lane
    /// on to the next stop.
    ///
    /// Stated as two halves so neither can be satisfied by accident: no
    /// `Deg0` node may drive a `Deg90` node, and every `Deg90` node is driven
    /// either by another `Deg90` node or from OUTSIDE the chain, which is
    /// where a lane's head takes its value. Outside the chain has two
    /// spellings — the producer's own port, and the source-side rerouter
    /// standing beside that producer, which is itself driven by that port and
    /// so is an entry into the lane rather than a link of it. Fanning a lane
    /// node out to both the next link and its own tap is the intended shape
    /// and stays legal.
    #[test]
    fn taps_branch_off_the_lane_and_never_carry_it() {
        let src = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${b}\")\n  PrintToConsole(\"${a}\")\n  b = a + b\n  log.push(\"y${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            l.bus.nodes.len() > 8,
            "fixture must build a band worth walking, got {} nodes",
            l.bus.nodes.len()
        );
        let rot = |i: usize| l.bus.nodes[i].rotation;

        for w in &l.bus.wires {
            if let (BusEnd::Bus(a), BusEnd::Bus(b)) = (w.source, w.target) {
                assert!(
                    !(rot(a) == NodeRotation::Deg0 && rot(b) == NodeRotation::Deg90),
                    "tap {a} carries the lane on to {b}; a tap is a branch, \
                     only down-pointing rerouters chain"
                );
            }
        }

        let mut lane_links = 0usize;
        let mut heads = 0usize;
        for (i, n) in l.bus.nodes.iter().enumerate() {
            if n.rotation != NodeRotation::Deg90 {
                continue;
            }
            lane_links += 1;
            let inbound = l
                .bus
                .wires
                .iter()
                .find(|w| w.target == BusEnd::Bus(i))
                .unwrap_or_else(|| panic!("lane node {i} is driven by nothing"));
            match inbound.source {
                // The head, taking the value from its producer's own port.
                BusEnd::Node(_) => heads += 1,
                // The head again, through the rerouter standing beside that
                // producer — which is outside the chain and must itself be fed
                // by the port, or the lane would be entered from the bus.
                BusEnd::Bus(p) if l.bus.nodes[p].role == BusRole::Source => {
                    assert!(
                        l.bus.wires.iter().any(|w| w.target == BusEnd::Bus(p)
                            && matches!(w.source, BusEnd::Node(_))),
                        "the source-side rerouter {p} heading lane node {i} is not \
                         driven by a real port"
                    );
                    heads += 1;
                }
                BusEnd::Bus(p) => assert_eq!(
                    rot(p),
                    NodeRotation::Deg90,
                    "lane node {i} is driven by {p}, which is not a lane node"
                ),
            }
        }
        assert!(
            lane_links > 2,
            "fixture must run real lanes, got {lane_links} chain-carrying nodes"
        );
        assert!(
            heads > 0,
            "fixture must head a lane, or the entry-point half proves nothing"
        );
    }

    /// Lanes are allocated per PAGE, off that page's own left edge, and a
    /// value read on two pages heads a lane on each. Every other bus test runs
    /// inside the default budgets, which never paginate, so this is the only
    /// place that shape is measured.
    #[test]
    fn a_paginated_body_builds_a_lane_on_every_page() {
        let (m, budgets) = paginated_bus_module();
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        let body_pages: BTreeSet<i32> = l.placements.values().map(|p| p.z).collect();
        assert!(
            body_pages.len() > 1,
            "fixture must paginate, got {} page(s)",
            body_pages.len()
        );
        // Not every page: one whose values never travel `MIN_LANE_TRAVEL`
        // earns no lane at all. What must hold is that a band only ever sits
        // on a page that has a body, and that the pages which DO band do it
        // off their own edges — checked below.
        let bus_pages: BTreeSet<i32> = l.bus.nodes.iter().map(|n| n.z).collect();
        assert!(!bus_pages.is_empty(), "the fixture must still band somewhere");
        assert!(
            bus_pages.is_subset(&body_pages),
            "a band may only sit on a page that has a body"
        );

        // A page's band hangs off that page's OWN left edge. Edge pins are held
        // out of the body edge — they stack further left still, clearing the
        // band `plan.band_widths` reserved for them — and so are the gate-side
        // taps, which stand one `TAP_RESERVE` inside the body's own columns.
        let mut lane_counts: Vec<usize> = Vec::new();
        for &z in &body_pages {
            let body_min_y = l
                .placements
                .iter()
                .filter(|(id, p)| p.z == z && !is_edge_pin(&m.nodes[*id]))
                .map(|(_, p)| p.y)
                .min()
                .expect("a page with body nodes");
            let gutter: BTreeSet<i32> = l
                .bus
                .nodes
                .iter()
                .filter(|n| n.z == z && n.y <= body_min_y - BUS_BAND_GUTTER)
                .map(|n| n.y)
                .collect();
            if gutter.is_empty() {
                // A page whose rows are too short for any value to travel
                // `MIN_LANE_TRAVEL` earns no band, which is the zero-travel
                // rule working. The per-page geometry claims below apply to
                // the pages that DO band.
                continue;
            }
            let inner = *gutter.iter().next_back().expect("a non-empty gutter");
            assert_eq!(
                inner,
                body_min_y - BUS_BAND_GUTTER - LANE_PITCH,
                "the innermost lane on the page at z {z} must stand one pitch off \
                 THIS page's left edge, not another page's"
            );
            let outer = *gutter.iter().next().expect("a non-empty gutter");
            assert_eq!(
                outer,
                inner - (gutter.len() as i32 - 1) * LANE_PITCH,
                "a page's lanes stand flush against each other: {gutter:?}"
            );
            lane_counts.push(gutter.len());
        }
        assert!(
            lane_counts.iter().collect::<HashSet<_>>().len() > 1,
            "lanes are allocated per page, so the pages must not all claim the \
             same count by coincidence: {lane_counts:?}"
        );

        assert_no_overlap(&m, &l);
        assert_no_fan_in(&l);
        let proven = assert_suppressed_consumers_stay_reachable(&m, &l);
        assert!(proven > 0, "a paginated bus must still replace wires");
    }

    #[test]
    fn a_multi_row_variable_gets_a_lane() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "a two-row read must build a lane");
        // Every consumer's original wire is suppressed.
        let var_id = m
            .nodes
            .values()
            .find(|n| n.gate_class == crate::ir::gate_class::PSEUDO_VAR)
            .unwrap()
            .id;
        for w in m.wires.iter().filter(|w| w.source.node_id == var_id) {
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "wire into {:?} must be replaced by the bus",
                w.target.node_id
            );
        }
    }

    /// The bus source of a chain, and every bus node the chain owns, split
    /// into its lane rerouters and its gate-side taps.
    ///
    /// A node's kind is read off the WIRES and off `BusRole`, never off its
    /// `y` — that is the coordinate the geometry tests below assert on, so
    /// classifying by it would make them self-confirming. The shape is
    /// unambiguous: a node driven from outside the chain is a lane head,
    /// whether that is a `BusEnd::Node` port or the source-side rerouter
    /// standing beside a body producer; a node driven by a LANE node standing
    /// in a DIFFERENT column (`x`, the row axis) is the next link down the
    /// lane, since a new row means a new row column; and a node driven from
    /// within the same column is a tap, because a row's taps are placed at
    /// their lane rerouter's own `x`.
    struct Chain {
        source: PortRef,
        lanes: Vec<usize>,
        taps: Vec<usize>,
    }

    /// Follow the one hop a source-side rerouter adds, so a walk starting at a
    /// real port lands on the LANE brick whichever kind of producer it left.
    ///
    /// An edge pin drives its head directly; a body gate drives it through the
    /// rerouter standing beside that gate. A test matching only the direct
    /// shape is how a claim about heads quietly stops looking at any.
    fn through_source_side(l: &LayoutResult, first: BusNodeId) -> BusNodeId {
        if l.bus.nodes[first].role != BusRole::Source {
            return first;
        }
        l.bus
            .wires
            .iter()
            .find(|w| w.source == BusEnd::Bus(first))
            .and_then(|w| match w.target {
                BusEnd::Bus(i) => Some(i),
                BusEnd::Node(_) => None,
            })
            .expect("a source-side rerouter drives its lane's head")
    }

    fn bus_chains(l: &LayoutResult) -> Vec<Chain> {
        // Fan-in is illegal, so each bus node has exactly one inbound wire.
        let mut inbound: HashMap<usize, BusEnd> = HashMap::default();
        for w in &l.bus.wires {
            if let BusEnd::Bus(i) = w.target {
                assert!(
                    inbound.insert(i, w.source).is_none(),
                    "bus node {i} is driven twice"
                );
            }
        }
        assert_eq!(
            inbound.len(),
            l.bus.nodes.len(),
            "every bus node must be driven"
        );

        // A node driven from OUTSIDE the chain is that chain's head: either
        // straight off a real port, or off the source-side rerouter standing
        // beside a body producer — which shares the head's row, so without the
        // role test it would read as a tap of its own head's column.
        let is_lane = |i: usize| match inbound[&i] {
            BusEnd::Node(_) => true,
            BusEnd::Bus(p) => {
                l.bus.nodes[p].role != BusRole::Gutter || l.bus.nodes[p].x != l.bus.nodes[i].x
            }
        };

        // Walk to the chain's head. A bus node is always pushed before the
        // wire that drives it, so the source index is strictly smaller and
        // the walk terminates.
        let head_of = |mut i: usize| -> (usize, PortRef) {
            for _ in 0..=l.bus.nodes.len() {
                match inbound[&i] {
                    BusEnd::Node(p) => return (i, p),
                    BusEnd::Bus(p) => {
                        assert!(p < i, "bus wire {p} -> {i} runs backwards");
                        i = p;
                    }
                }
            }
            panic!("bus chain from {i} has no head");
        };

        // Chains describe the GUTTER structure — a column of links with taps
        // branching off it. A mini-bus corner is a one-brick turn inside the
        // body with no chain to walk and no column to share, and a source-side
        // rerouter stands beside a producer in the body rather than in the
        // band, so every claim built on `Chain` would be false of either by
        // construction; their geometry is pinned by
        // `the_mini_bus_drops_an_expression_value_into_its_statement` and
        // `a_lane_is_fed_from_a_rerouter_beside_its_producer` instead. The
        // "every node is driven" check above still covers all three.
        //
        // `head_of` walks THROUGH a source-side rerouter to the port behind
        // it, so `Chain.source` is the value's real producer either way, and
        // the head index it keys on is stable per lane.
        let mut by_head: Vec<Chain> = Vec::new();
        let mut index: HashMap<usize, usize> = HashMap::default();
        for i in 0..l.bus.nodes.len() {
            if l.bus.nodes[i].role != BusRole::Gutter {
                continue;
            }
            let (head, source) = head_of(i);
            let ci = *index.entry(head).or_insert_with(|| {
                by_head.push(Chain {
                    source,
                    lanes: Vec::new(),
                    taps: Vec::new(),
                });
                by_head.len() - 1
            });
            if is_lane(i) {
                by_head[ci].lanes.push(i);
            } else {
                by_head[ci].taps.push(i);
            }
        }
        by_head
    }

    /// A lane is a COLUMN: every lane rerouter of one chain shares one `y`,
    /// however many rows the chain reaches. And every gate-side tap sits
    /// exactly one gap left of the gate it drives — not merely beside some
    /// gate somewhere on the plane.
    #[test]
    fn lane_rerouters_share_a_column() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "the fixture must build a lane");

        let chains = bus_chains(&l);
        assert!(
            chains.iter().any(|c| c.lanes.len() >= 2),
            "fixture must run one lane down at least two rows, or the \
             shared-column claim is vacuous; got {:?}",
            chains.iter().map(|c| c.lanes.len()).collect::<Vec<_>>()
        );

        for c in &chains {
            let (&first, rest) = c.lanes.split_first().expect("a chain has a head");
            let y = l.bus.nodes[first].y;
            for &i in rest {
                assert_eq!(
                    l.bus.nodes[i].y, y,
                    "lane rerouter {i} of the chain from {:?} left its column \
                     (y={} vs the head's {y})",
                    c.source, l.bus.nodes[i].y
                );
            }

            // Each tap sits one gap left of its OWN consumer.
            for &i in &c.taps {
                let driven: Vec<PortRef> = l
                    .bus
                    .wires
                    .iter()
                    .filter(|w| w.source == BusEnd::Bus(i))
                    .filter_map(|w| match w.target {
                        BusEnd::Node(p) => Some(p),
                        BusEnd::Bus(_) => None,
                    })
                    .collect();
                assert!(!driven.is_empty(), "tap {i} drives no gate");
                for p in driven {
                    assert_eq!(
                        l.bus.nodes[i].y,
                        l.placements[&p.node_id].y - TAP_GAP - 2 * REROUTER_HALF,
                        "tap {i} must sit one gap left of the gate it feeds"
                    );
                }
            }
        }
    }

    /// Lane 0 is the leftmost column, and `allocate_lanes` gives lane 0 to
    /// the widest-span value — so the value read across two rows holds a
    /// column further out than the one-row variable feeding it.
    #[test]
    fn the_widest_span_value_takes_the_leftmost_lane() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let chains = bus_chains(&l);

        // Rank the lanes by how far down the band each one actually reaches,
        // and by the column it stands in. Keying on gate CLASS instead only
        // works while a body happens to carry exactly one lane of each kind,
        // which is not a property of the rule being tested — it is a property
        // of a fixture small enough that it no longer earns a bus at all.
        let mut by_span: Vec<(usize, i32)> = chains
            .iter()
            .filter(|c| !c.lanes.is_empty())
            .map(|c| (c.lanes.len(), l.bus.nodes[c.lanes[0]].y))
            .collect();
        by_span.sort_by_key(|&(span, _)| Reverse(span));
        assert!(
            by_span.len() >= 2,
            "fixture must run at least two lanes, got {}",
            by_span.len()
        );
        let widest = by_span[0].0;
        assert!(
            widest > by_span.last().expect("a lane").0,
            "fixture must run lanes of DIFFERENT spans, or the ranking claim \
             is vacuous: {by_span:?}"
        );
        let widest_y = by_span[0].1;
        for &(span, y) in &by_span {
            assert!(
                span == widest || widest_y < y,
                "the widest-span lane must hold the outermost column: it sits \
                 at y={widest_y}, and a {span}-stop lane sits further out at y={y}"
            );
        }

        // ...and the whole band still sits left of the code body.
        let body_min_y = m
            .nodes
            .values()
            .filter(|n| is_spawnable(n) && !is_edge_pin(n))
            .map(|n| l.placements[&n.id].y)
            .min()
            .expect("a placed body node");
        for c in &chains {
            for &i in &c.lanes {
                assert!(
                    l.bus.nodes[i].y + 2 * REROUTER_HALF <= body_min_y - BUS_BAND_GUTTER,
                    "lane rerouter {i} at y={} is not clear of the body edge {body_min_y}",
                    l.bus.nodes[i].y
                );
            }
        }
    }

    #[test]
    fn a_tap_and_its_lane_rerouter_share_a_row() {
        // The horizontal run must be straight: same x on both ends.
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        for w in &l.bus.wires {
            if let (crate::layout::BusEnd::Bus(a), crate::layout::BusEnd::Bus(b)) =
                (w.source, w.target)
            {
                let (na, nb) = (&l.bus.nodes[a], &l.bus.nodes[b]);
                assert!(
                    na.x == nb.x || na.y == nb.y,
                    "every bus-to-bus run must be axis-aligned"
                );
            }
        }
    }

    /// No bus wire target may be driven twice, anywhere.
    fn assert_no_fan_in(l: &LayoutResult) {
        let mut seen: HashSet<BusEnd> = HashSet::default();
        for w in &l.bus.wires {
            assert!(seen.insert(w.target), "two wires into {:?}", w.target);
        }
    }

    #[test]
    fn the_bus_creates_no_fan_in() {
        let src = "var a: int = 1\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}\")\n  log.push(\"y\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert_no_fan_in(&l);
    }

    #[test]
    fn dag_mode_builds_no_bus() {
        let m = lowered("var a: int = 1\nin go: exec\non go { PrintToConsole(\"${a}\") }\n");
        let l = crate::layout::layout(&m);
        assert!(l.bus.is_empty());
    }

    #[test]
    fn tap_rerouters_point_right_and_lane_rerouters_point_down() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "fixture must build a bus");
        // A rerouter faces what it FEEDS, so its turn names the direction the
        // value travels next. A LANE run goes rightward into the body, so the
        // band is built from exactly two facings and never the leftward one.
        //
        // Two structures do face left and neither is a lane brick: a mini-bus
        // corner, and a source-side rerouter handing its producer's value out
        // to the gutter. `BusRole` is what separates them from the band, so
        // they are counted here on their own terms rather than being allowed
        // to soften the count below.
        let mut downs = 0;
        let mut ups = 0;
        let mut rights = 0;
        let mut lefts = 0;
        let mut source_lefts = 0;
        let mut source_others = 0;
        for n in &l.bus.nodes {
            match (n.role, n.rotation) {
                (BusRole::Gutter, crate::layout::NodeRotation::Deg90) => downs += 1,
                (BusRole::Gutter, crate::layout::NodeRotation::Deg270) => ups += 1,
                (BusRole::Gutter, crate::layout::NodeRotation::Deg0) => rights += 1,
                (BusRole::Gutter, crate::layout::NodeRotation::Deg180) => lefts += 1,
                (BusRole::Source, crate::layout::NodeRotation::Deg180) => source_lefts += 1,
                (BusRole::Source, _) => source_others += 1,
                (BusRole::Line, _) => {}
            }
        }
        assert!(
            downs + ups > 0,
            "lane rerouters must carry the chain, down or up"
        );
        assert!(rights > 0, "tap and gate-side rerouters must point right");
        assert_eq!(
            lefts, 0,
            "a gutter run goes rightward into the body; no lane brick faces left"
        );
        assert!(
            source_lefts > 0,
            "fixture must stand a rerouter beside a producer, or the split \
             between lane bricks and source-side ones proves nothing"
        );
        assert_eq!(
            source_others, 0,
            "a source-side rerouter hands its value LEFT into the gutter"
        );
    }

    #[test]
    fn each_tap_has_a_leaf_partner_before_any_real_port() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        // Walk the bus wires: a value reaches a real port from a LEAF, never
        // straight off a chain link. `Deg90` is what carries a chain onward,
        // so a leaf is anything else — which is the property this means, and
        // it stays true however many facings leaves come in.
        for w in &l.bus.wires {
            if let (crate::layout::BusEnd::Bus(a), crate::layout::BusEnd::Node(_)) =
                (w.source, w.target)
            {
                assert!(
                    !carries_a_chain(l.bus.nodes[a].rotation),
                    "a wire into a real port must leave a leaf, not a chain link"
                );
            }
        }
    }

    #[test]
    fn a_lane_head_sits_at_its_sources_row() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        // The head is the GUTTER node a real port reaches FIRST. Where the
        // producer is a body gate it reaches it through the source-side
        // rerouter standing beside that gate, so the walk has to follow that
        // hop — reading only the wires whose target is already a lane brick
        // would match nothing here and go green having checked no head at all.
        //
        // A mini-bus corner is also fed by a real port, but it stands on its
        // CONSUMER's row rather than its source's — that is what turns the
        // wire — so it is a different claim and is pinned separately.
        let mut checked = 0usize;
        for w in &l.bus.wires {
            let (crate::layout::BusEnd::Node(src_port), crate::layout::BusEnd::Bus(first)) =
                (w.source, w.target)
            else {
                continue;
            };
            let head = &l.bus.nodes[through_source_side(&l, first)];
            if head.role != BusRole::Gutter {
                continue;
            }
            let src_x = l.placements[&src_port.node_id].x;
            assert!(
                (head.x - src_x).abs() <= 10,
                "lane head x {} must sit at its source's row {src_x}",
                head.x
            );
            assert_eq!(
                head.rotation,
                crate::layout::NodeRotation::Deg90,
                "a lane head points down"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "fixture must head a lane from a real port, or this proves nothing"
        );
    }

    /// A body handing values AND exec across a chip wall, so one fixture
    /// carries a producer of each kind: the gate that hands the anonymous
    /// chip its exec, and the chip brick a value is read back out of.
    const CHIP_PRODUCER_SRC: &str = "var score: int = 0
var tick: int = 0
var log: string[]
in go: exec
chip Scorer(run: exec, amount: int) -> (total: int) {
  on run {
    score = score + amount
    log.push(\"s\")
  }
  out total = score
}
let scored = Scorer(go, 5)
on go {
  score = 0
  PrintToConsole(\"a\")
  chip {
    score = score + 1
    log.push(\"i\")
  }
  tick = tick + 1
  log.push(\"r${tick}\")
  PrintToConsole(\"z${scored.total}\")
}
";

    /// The producer side of a lane, mirroring the consumer side it has always
    /// had: a rerouter standing beside the gate that makes the value, which
    /// that gate wires into and which hands the value out to the lane head.
    ///
    /// Without it a lane began at the gutter and the only thing marking where
    /// a value LEFT was the far end of a long wire. Values and execs alike —
    /// the same code path lays both, and an exec chain is the flow the brick
    /// most needs to make visible.
    #[test]
    fn a_lane_is_fed_from_a_rerouter_beside_its_producer() {
        let mut exec_producers = 0usize;
        for (name, m) in [
            ("values", lowered(TWO_ROW_BUS_SRC)),
            ("crossings", lowered(CHIP_PRODUCER_SRC)),
        ] {
            let l = layout_code(&m, &code_opts(), false);
            let owners = chip_owners(&m);
            let mut beside = 0usize;
            for w in &l.bus.wires {
                let (BusEnd::Node(src), BusEnd::Bus(i)) = (w.source, w.target) else {
                    continue;
                };
                if l.bus.nodes[i].role != BusRole::Source {
                    continue;
                }
                // The brick THIS module sees the value leave: the producing
                // gate itself, or the chip a value is read back out of.
                let anchor = if m.nodes.contains_key(&src.node_id) {
                    src.node_id
                } else {
                    owners.owner[&src.node_id]
                };
                let at = l.placements[&anchor];
                let node = &l.bus.nodes[i];
                assert_eq!(node.z, at.z, "{name}: the rerouter shares its producer's plane");
                assert_eq!(
                    node.x, at.x,
                    "{name}: ...stands at the producer's own level"
                );
                assert_eq!(
                    node.y,
                    at.y - TAP_RESERVE,
                    "{name}: ...one tap reserve to its left, the mirror of a \
                     gate-side rerouter"
                );
                assert_eq!(
                    node.rotation,
                    NodeRotation::Deg180,
                    "{name}: ...facing the gutter its value travels into"
                );
                // ...and it hands that value to a lane head on the same level,
                // so the run out to the band is horizontal rather than the
                // diagonal the gate's own port used to draw.
                let head = &l.bus.nodes[through_source_side(&l, i)];
                assert_eq!(
                    head.role,
                    BusRole::Gutter,
                    "{name}: a source-side rerouter feeds a lane head"
                );
                assert_eq!(head.x, node.x, "{name}: ...on the level it stands at");
                assert!(
                    head.y < node.y,
                    "{name}: ...out in the gutter, left of the body"
                );
                if src.port.as_str().contains("Exec") {
                    exec_producers += 1;
                }
                beside += 1;
            }
            assert!(
                beside > 0,
                "{name}: fixture must stand a rerouter beside a producer"
            );
            assert_no_fan_in(&l);
            assert_no_overlap(&m, &l);
            assert_suppressed_consumers_stay_reachable(&m, &l);
        }
        assert!(
            exec_producers > 0,
            "an exec delivery must get a source-side rerouter like any value"
        );
    }

    /// The chain invariant, restated for the shape the source-side rerouter
    /// makes rather than weakened by it.
    ///
    /// A lane HEAD is fed from outside its chain — by a real port, or now by
    /// the rerouter standing beside its producer, which is itself fed by that
    /// port and drives nothing else. Every OTHER lane brick must still be fed
    /// by a lane brick. That is what keeps a lane one value from one producer:
    /// a second entry point partway down a column would carry a different
    /// value into the same run, and no consumer of it would say so.
    #[test]
    fn only_a_lane_head_is_fed_from_outside_its_chain() {
        for (name, m) in [
            ("two_row", lowered(TWO_ROW_BUS_SRC)),
            ("band", lowered(BAND_SRC)),
            ("mini", lowered(MINI_BUS_SRC)),
            ("crossings", lowered(CHIP_PRODUCER_SRC)),
        ] {
            let l = layout_code(&m, &code_opts(), false);
            let mut inbound: HashMap<usize, BusEnd> = HashMap::default();
            for w in &l.bus.wires {
                if let BusEnd::Bus(i) = w.target {
                    assert!(
                        inbound.insert(i, w.source).is_none(),
                        "{name}: bus node {i} is driven twice"
                    );
                }
            }

            let mut entries = 0usize;
            for (i, n) in l.bus.nodes.iter().enumerate() {
                if n.role != BusRole::Gutter {
                    continue;
                }
                match inbound[&i] {
                    BusEnd::Node(_) => entries += 1,
                    BusEnd::Bus(p) if l.bus.nodes[p].role == BusRole::Source => {
                        assert!(
                            matches!(inbound[&p], BusEnd::Node(_)),
                            "{name}: the source-side rerouter feeding lane brick {i} is \
                             itself fed from the bus, not from its producer's port"
                        );
                        entries += 1;
                    }
                    BusEnd::Bus(p) => assert_eq!(
                        l.bus.nodes[p].role,
                        BusRole::Gutter,
                        "{name}: lane brick {i} is fed by a {:?} node, not by its own chain",
                        l.bus.nodes[p].role
                    ),
                }
            }
            let chains = bus_chains(&l);
            assert!(!chains.is_empty(), "{name}: fixture must build lanes");
            assert_eq!(
                entries,
                chains.len(),
                "{name}: a lane takes exactly one entry point"
            );

            // ...and the new brick is an entry point and nothing more: it
            // hands its value to the band, never to a gate, and never twice.
            for (i, n) in l.bus.nodes.iter().enumerate() {
                if n.role != BusRole::Source {
                    continue;
                }
                let out: Vec<BusEnd> = l
                    .bus
                    .wires
                    .iter()
                    .filter(|w| w.source == BusEnd::Bus(i))
                    .map(|w| w.target)
                    .collect();
                assert_eq!(
                    out.len(),
                    1,
                    "{name}: source-side rerouter {i} drives {} bricks",
                    out.len()
                );
                assert!(
                    matches!(out[0], BusEnd::Bus(_)),
                    "{name}: a source-side rerouter feeds the band, not a gate"
                );
            }
        }
    }

    #[test]
    fn lanes_are_packed_at_the_tightened_pitch() {
        assert_eq!(LANE_PITCH, 2, "lane spacing is halved");
    }

    /// A port heads a lane just like a stored value does, even though the
    /// pin stack is placed only after the band it has to clear is measured.
    /// Without the plan/lay split the port would have no placement to stand
    /// beside when its lane is laid.
    #[test]
    fn an_input_ports_lane_is_headed_beside_the_port() {
        let src = "in v: int
in go: exec
var b: int = 2\nvar log: string[]\non go {
  PrintToConsole(\"${v}\")
  PrintToConsole(\"x${v}\")\n  log.push(\"p${b}\")\n  PrintToConsole(\"q${b}\")\n  b = b + 1\n  log.push(\"r${b}\")
}
";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        // BOTH declared ports head a lane: the int one because its value is
        // read on two rows, and the exec one because handing exec from a port
        // into the body is a delivery, not spine sequencing.
        //
        // A port drives its head DIRECTLY. The source-side rerouter a body
        // producer gets stands one tap reserve to the producer's left, and an
        // edge pin's stack is placed on the far side of the band from the
        // body — so a brick there would face away from the lane it feeds, and
        // no row out there reserves a cell for it. That the target below is
        // the head itself is what pins the exclusion.
        let headed: Vec<(NodeId, &BusNode)> = l
            .bus
            .wires
            .iter()
            .filter_map(|w| match (w.source, w.target) {
                (BusEnd::Node(p), BusEnd::Bus(i))
                    if m.nodes
                        .get(&p.node_id)
                        .is_some_and(|n| n.kind == NodeKind::Input) =>
                {
                    Some((p.node_id, &l.bus.nodes[i]))
                }
                _ => None,
            })
            .collect();
        let ports: HashSet<NodeId> = headed.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ports.len(),
            2,
            "both declared ports must head a lane, got {}",
            ports.len()
        );
        for (pin_id, head) in &headed {
            assert_eq!(
                head.role,
                BusRole::Gutter,
                "an edge pin drives its lane brick directly; nothing stands \
                 between the pin stack and the band"
            );
            // The lane's first brick stands in the port's OWN row band. Two
            // shapes land here and both are correct: a head proper, at the
            // port's row exactly, and — when the port is level with the
            // lane's first stop, so that stop's tap already holds the cell —
            // the stop's own link, one rerouter above it. Either way the
            // brick is inside the row, which is what proves the port's
            // placement was known when the lane was laid; that is the whole
            // point of the plan/lay split, since the pin stack is placed only
            // after the band it has to clear is measured.
            let at = l.placements[pin_id].x;
            assert!(
                head.x >= at && head.x < at + LANE_PAIR_HEIGHT,
                "the lane's first brick sits at x={}, outside the port's own \
                 row band [{at}, {})",
                head.x,
                at + LANE_PAIR_HEIGHT
            );
            assert_eq!(head.rotation, NodeRotation::Deg90, "a lane head points down");
        }
    }

    /// A value entering a chip is a delivery, not a diagonal. The pin it
    /// lands on lives in the CHILD module and holds no row here, so its tap
    /// anchors on the chip brick the value enters through. Exec deliveries
    /// count: handing exec to a chip is a crossing, which is a different
    /// thing from an exec chain sequencing statements along one spine.
    #[test]
    fn values_entering_a_chip_tap_the_bus_at_the_chips_row() {
        let src = "var score: int = 0
var log: string[]
var tick: int = 0
in go: exec
chip Scorer(run: exec, amount: int) -> (total: int) {
  on run {
    score = score + amount
    log.push(\"s\")
  }
  out total = score
}
let scored = Scorer(go, 5)
on go {
  score = 0
  chip {
    score = score + 1
    log.push(\"i\")
  }
  PrintToConsole(\"${scored.total}\")
  log.push(\"p${tick}\")
  PrintToConsole(\"q${tick}\")
  tick = tick + 1
  log.push(\"r${tick}\")
}
";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        // A delivery: a wire out of this module's body whose target is not a
        // node of this module, so it lands inside some chip.
        let deliveries: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| !m.nodes.contains_key(&w.target.node_id))
            .filter(|w| m.nodes.get(&w.source.node_id).is_some_and(is_spawnable))
            .collect();
        assert!(
            deliveries.len() >= 3,
            "fixture must deliver several values into chips, got {}",
            deliveries.len()
        );
        assert!(
            deliveries
                .iter()
                .any(|w| w.source.port.as_str().contains("Exec")),
            "fixture must include an exec delivery"
        );

        let owners = chip_owners(&m);
        for w in &deliveries {
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the delivery {} .{} -> {} .{} still runs as a direct wire",
                w.source.node_id,
                w.source.port.as_str(),
                w.target.node_id,
                w.target.port.as_str()
            );
            let chip_id = *owners
                .owner
                .get(&w.target.node_id)
                .expect("a delivery lands inside some chip of this module");
            let chip = &m.nodes[&chip_id];
            let chip_at = l.placements[&chip_id];
            let driver = l
                .bus
                .wires
                .iter()
                .find(|bw| bw.target == BusEnd::Node(w.target))
                .and_then(|bw| match bw.source {
                    BusEnd::Bus(i) => Some(&l.bus.nodes[i]),
                    BusEnd::Node(_) => None,
                })
                .expect("a bus node drives the suppressed port");
            let top = chip_at.x + measured_half_size(chip, &l).0 * 2;
            assert!(
                driver.x >= chip_at.x && driver.x < top,
                "tap for {} .{} sits at x {}, off chip {chip_id}'s row [{}, {top})",
                w.target.node_id,
                w.target.port.as_str(),
                driver.x,
                chip_at.x
            );
            assert!(
                driver.y < chip_at.y,
                "a tap feeds its chip from the left, not from x {} y {}",
                driver.x,
                driver.y
            );
            assert_eq!(
                driver.rotation,
                NodeRotation::Deg0,
                "a chip is fed by a right-pointing tap like any other consumer"
            );
        }

        // Several values enter the same chip, so several taps land on one
        // row. That is fan-OUT across distinct ports; two wires into one port
        // would be fan-in, and the bricks must still not collide.
        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
    }

    /// A statement's exec gate sits BELOW the expression it consumes, not
    /// above it, while keeping the left column it heads its line from.
    ///
    /// Values flow left to right into the sink; stacking the sink on the
    /// expression's own top row put it level with the operands it reads, so
    /// the run into it came back leftward across the line. Dropping it to the
    /// bottom row of its group's block turns that into a descent: the
    /// expression resolves across the upper rows, and the value comes DOWN
    /// into the statement that consumes it.
    ///
    /// The column is deliberately unchanged. Exec gates still line up in one
    /// vertical column down the left margin, so the chain reads as a single
    /// spine and the downward-rotation rule keeps its meaning.
    #[test]
    fn a_statement_exec_gate_sits_below_the_expression_it_consumes() {
        let m = lowered("var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }");
        let l = layout_code(&m, &opts(), false);
        let var_set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_SET)
            .expect("Var_Set node");
        let line = var_set.source_range.start.line;
        let set_at = l.placements[&var_set.id];

        let expression: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| {
                n.kind == NodeKind::Gate
                    && n.source_range.start.line == line
                    && n.id != var_set.id
                    && l.placements.contains_key(&n.id)
            })
            .collect();
        assert!(
            expression.len() >= 4,
            "fixture must lower an expression tree, got {} gates",
            expression.len()
        );

        // Lower on the plane is a SMALLER x: a row's placement is derived as
        // `page_h - down - height`, so a greater sub-row offset reads lower.
        for n in &expression {
            assert!(
                l.placements[&n.id].x > set_at.x,
                "expression gate {} sits at x={}, not ABOVE the sink it feeds \
                 (sink x={})",
                n.gate_class,
                l.placements[&n.id].x,
                set_at.x
            );
        }

        // ...and it still heads its line on the left.
        for n in &expression {
            assert!(
                l.placements[&n.id].y > set_at.y,
                "expression gate {} must stay RIGHT of the sink",
                n.gate_class
            );
        }
        assert_eq!(
            rotation_of(&l.rotations, &var_set.id),
            NodeRotation::Deg90,
            "the sink is still on the spine and still faces down it"
        );
    }

    /// Dropping the sink below its expression turns the operand wire into a
    /// down-AND-left diagonal across the line's own block. The mini-bus turns
    /// it back into a right angle: one rerouter standing where the operand's
    /// column meets the sink's row takes the value straight DOWN, then hands
    /// it straight LEFT along a row that holds nothing but the sink.
    ///
    /// Same rules as the gutter bus, which is why it is the same `BusLayout`:
    /// a `Deg0` leaf feeding one port, exactly one inbound wire, and the
    /// module wire it replaces suppressed so emit never draws both.
    #[test]
    fn the_mini_bus_drops_an_expression_value_into_its_statement() {
        let m = lowered(MINI_BUS_SRC);
        let l = layout_code(&m, &opts(), false);
        let var_set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_SET)
            .expect("Var_Set node");
        let line = var_set.source_range.start.line;

        // The operand wires the drop created: source on the sink's own line,
        // above it and to its right.
        let drops: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| w.target.node_id == var_set.id)
            .filter(|w| {
                m.nodes
                    .get(&w.source.node_id)
                    .is_some_and(|n| n.kind == NodeKind::Gate && n.source_range.start.line == line)
            })
            .collect();
        assert!(
            !drops.is_empty(),
            "fixture must feed its sink from an expression on the same line"
        );

        for w in &drops {
            let src_at = l.placements[&w.source.node_id];
            let sink_at = l.placements[&var_set.id];
            assert!(
                src_at.x > sink_at.x && src_at.y > sink_at.y,
                "fixture operand must sit above and right of its sink"
            );
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the drop into {} .{} still runs as a direct diagonal",
                w.target.node_id,
                w.target.port.as_str()
            );
            let driver = l
                .bus
                .wires
                .iter()
                .find(|bw| bw.target == BusEnd::Node(w.target))
                .and_then(|bw| match bw.source {
                    BusEnd::Bus(i) => Some(&l.bus.nodes[i]),
                    BusEnd::Node(_) => None,
                })
                .expect("a bus node drives the suppressed port");
            assert_eq!(
                driver.rotation,
                NodeRotation::Deg180,
                "a mini-bus rerouter is a leaf like every other tap, but it                  runs LEFTWARD into its statement, so it faces the other way"
            );
            // The right angle, both halves: on the sink's own row, and in the
            // column of the operand it reads.
            //
            // The row is a BAND, not one level. Two operands dropping into a
            // single statement are staggered inside it so their runs are not
            // drawn on top of each other; a corner OUTSIDE the band would be a
            // run that no longer arrives at the gate it feeds.
            let sink_top = sink_at.x + measured_half_size(var_set, &l).0 * 2;
            assert!(
                driver.x >= sink_at.x && driver.x + 2 * REROUTER_HALF <= sink_top,
                "the rerouter must stand within the sink's own row band \
                 [{}, {sink_top}), got x={}",
                sink_at.x,
                driver.x
            );
            assert_eq!(
                driver.y, src_at.y,
                "...and in the operand's own column, so the drop is straight"
            );
        }

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// A lane must travel farther than the bricks it stands up to travel.
    ///
    /// A group whose source and taps sit at the same height buys nothing: the
    /// value was already an inline hop, and the lane replaces it with a
    /// rerouter out in the gutter plus a gate-side rerouter coming back —
    /// strictly more bricks and a longer path for the same wire.
    ///
    /// Measured over every chain that has a locally-placed producer, so a chip
    /// exit (whose producer lives in the child module) is out of scope here
    /// rather than silently passing.
    #[test]
    fn a_group_with_no_vertical_travel_is_not_bussed() {
        // Single-page fixtures only. Pages are centred independently, so on a
        // paginated body a producer and a consumer on different pages have
        // incomparable `x`: the rule measures within one page and so must this
        // check, rather than comparing coordinates from two origins.
        let cases: Vec<(&str, Module, CodeBudgets)> = vec![
            ("band", lowered(BAND_SRC), CodeBudgets::default()),
            ("mini", lowered(MINI_BUS_SRC), CodeBudgets::default()),
            ("two_row", lowered(TWO_ROW_BUS_SRC), CodeBudgets::default()),
        ];
        let mut checked = 0usize;
        let mut tall = 0usize;
        for (name, m, budgets) in &cases {
            let l = layout_code_with_budgets(m, &opts(), false, budgets);
            for c in bus_chains(&l) {
                let Some(src) = l.placements.get(&c.source.node_id) else {
                    continue;
                };
                let members: HashSet<usize> =
                    c.lanes.iter().chain(c.taps.iter()).copied().collect();
                // Heights compare only within one page, and a page IS a z
                // plane, so consumers on another plane are out of scope.
                let mut lo = src.x;
                let mut hi = src.x;
                for w in &l.bus.wires {
                    if let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) {
                        if members.contains(&i) {
                            if let Some(p) = l.placements.get(&t.node_id) {
                                if p.z != src.z {
                                    continue;
                                }
                                lo = lo.min(p.x);
                                hi = hi.max(p.x);
                            }
                        }
                    }
                }
                if hi == lo && members.is_empty() {
                    continue;
                }
                checked += 1;
                if hi - lo >= MIN_LANE_TRAVEL {
                    tall += 1;
                }
                assert!(
                    hi - lo >= MIN_LANE_TRAVEL,
                    "{name}: a lane carrying {:?} travels only {} units — the                      value was already inline and the lane costs more than the                      wire it replaced",
                    c.source.node_id,
                    hi - lo
                );
            }
        }
        assert!(checked > 4, "fixtures must build real lanes, got {checked}");
        assert!(tall > 0, "a genuinely tall lane must still bus");
    }

    /// The rotation that carries a chain onward, either way it runs.
    ///
    /// A lane whose taps all sit ABOVE its source travels upward, so its links
    /// face up. Both facings are chain links; neither is a leaf. Tests that
    /// hardcode `Deg90` would misfile an upward lane's links as leaves.
    fn carries_a_chain(r: NodeRotation) -> bool {
        matches!(r, NodeRotation::Deg90 | NodeRotation::Deg270)
    }

    /// A lane whose taps ALL sit above its source runs upward, and its chain
    /// links say so.
    ///
    /// The links are what travel; the leaves are not. A gate-side rerouter
    /// still hands its value rightward into the gate beside it whichever way
    /// the lane came from, so flipping the whole lane would point every leaf
    /// away from the thing it feeds. Only the chain turns.
    #[test]
    fn a_lane_whose_taps_are_all_above_its_source_runs_upward() {
        // Pagination is what puts a producer BELOW the rows that read it: a
        // page restarts the row order, so a value carried onto a later page
        // heads its lane from underneath its own consumers.
        let (m, budgets) = paginated_bus_module();
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);
        let chains = bus_chains(&l);
        assert!(!chains.is_empty(), "fixture must build lanes");

        let mut up = 0usize;
        let mut down = 0usize;
        {
            let l = &l;
        for c in &chains {
            if c.lanes.is_empty() {
                continue;
            }
            let tap_levels: Vec<i32> = c.taps.iter().map(|&i| l.bus.nodes[i].x).collect();
            if tap_levels.is_empty() {
                continue;
            }
            // `Chain.lanes` is "driven by something in another row", which
            // also catches a tap whose head was dropped for a contested cell.
            // The claim is about the nodes that actually CARRY the chain.
            let links: Vec<usize> = c
                .lanes
                .iter()
                .copied()
                .filter(|&i| carries_a_chain(l.bus.nodes[i].rotation))
                .collect();
            if links.is_empty() {
                continue;
            }
            // One lane, one direction: every link of a chain must agree, or
            // the run reverses partway down its own column.
            let facing = l.bus.nodes[links[0]].rotation;
            for &i in &links {
                assert_eq!(
                    l.bus.nodes[i].rotation,
                    facing,
                    "a lane's links must all face the same way; taps at                      {tap_levels:?}"
                );
            }
            match facing {
                NodeRotation::Deg270 => up += 1,
                _ => down += 1,
            }

            // ...and where the producer is placed in THIS module, the facing
            // has to match where the taps actually sit. A chip exit's producer
            // lives in the child module and holds no placement here, which is
            // exactly the lane that runs upward, so the semantic check is made
            // wherever it CAN be made rather than skipping the interesting
            // ones and proving nothing.
            if let Some(src) = l.placements.get(&c.source.node_id) {
                // The CONSUMERS' own rows, which is what the decision reads —
                // not the stop columns the bricks ended up in.
                let members: HashSet<usize> =
                    c.lanes.iter().chain(c.taps.iter()).copied().collect();
                let consumers: Vec<i32> = l
                    .bus
                    .wires
                    .iter()
                    .filter_map(|w| match (w.source, w.target) {
                        (BusEnd::Bus(i), BusEnd::Node(t)) if members.contains(&i) => {
                            l.placements.get(&t.node_id).map(|p| p.x)
                        }
                        _ => None,
                    })
                    .collect();
                if !consumers.is_empty() {
                    let all_above = consumers.iter().all(|&x| x > src.x);
                    assert_eq!(
                        facing == NodeRotation::Deg270,
                        all_above,
                        "a lane chains upward exactly when its taps are all                          above its source; source at x={}, consumers at                          {consumers:?}",
                        src.x
                    );
                }
            }
            // Its leaves are untouched either way: they face the gates.
            for &i in &c.taps {
                assert!(
                    !carries_a_chain(l.bus.nodes[i].rotation),
                    "a leaf must not take a chain-carrying facing"
                );
            }
        }
        }
        assert!(up > 0, "fixture must build an upward lane");
        assert!(down > 0, "fixture must keep a downward lane");
    }

    /// A rerouter faces what it FEEDS, and the three structures feed in
    /// different directions.
    ///
    /// A gutter tap stands left of the body and runs RIGHTWARD into the gate
    /// it drives, so it faces `+Y` — `Deg0`. A mini-bus corner stands out in
    /// its operand's column and runs LEFTWARD into the statement gate at the
    /// line's indent column, so it faces `−Y` — `Deg180`. Inheriting the
    /// gutter's `Deg0` pointed every expression branch away from the very
    /// thing it was feeding. A source-side rerouter stands beside a producer
    /// and runs LEFTWARD out to the gutter, so it faces `−Y` too — the same
    /// rule reaching the opposite answer from the tap it mirrors.
    ///
    /// Chain links are untouched at `Deg90`: a half turn on those would point
    /// them up, which is not where a drop goes.
    #[test]
    fn bus_rerouters_face_the_direction_their_value_travels() {
        // Carries every structure: a band feeding several rows, a statement
        // consuming an expression above it, and producers beside the band.
        let m = lowered(MINI_BUS_SRC);
        let l = layout_code(&m, &opts(), false);

        let mut gutter_leaves = 0usize;
        let mut mini_leaves = 0usize;
        let mut source_leaves = 0usize;
        for n in &l.bus.nodes {
            match (n.role, n.rotation) {
                // A chain link, either structure: a quarter turn, down for a
                // lane running down and up for one running up.
                (_, NodeRotation::Deg90) | (_, NodeRotation::Deg270) => {}
                (BusRole::Gutter, NodeRotation::Deg0) => gutter_leaves += 1,
                (BusRole::Line, NodeRotation::Deg180) => mini_leaves += 1,
                (BusRole::Source, NodeRotation::Deg180) => source_leaves += 1,
                (role, rot) => panic!(
                    "a {role:?} rerouter at ({}, {}) faces {rot:?}, which is not \
                     the way its value travels",
                    n.x, n.y
                ),
            }
        }
        assert!(gutter_leaves > 0, "fixture must build gutter taps");
        assert!(mini_leaves > 0, "fixture must build mini-bus corners");
        assert!(source_leaves > 0, "fixture must build source-side rerouters");

        // The directions the faces claim, measured. A mini-bus corner stands
        // RIGHT of the port it feeds; a gutter tap stands LEFT of its
        // consumer. Without this the rotations above are just two constants.
        let mut mini_runs = 0usize;
        let mut gutter_runs = 0usize;
        for w in &l.bus.wires {
            let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) else {
                continue;
            };
            let Some(sink) = l.placements.get(&t.node_id) else {
                continue;
            };
            match l.bus.nodes[i].role {
                BusRole::Line => {
                    assert!(
                        l.bus.nodes[i].y > sink.y,
                        "a mini-bus corner must stand right of its sink, or \
                         Deg180 faces away from it"
                    );
                    mini_runs += 1;
                }
                BusRole::Gutter => {
                    assert!(
                        l.bus.nodes[i].y < sink.y,
                        "a gutter tap must stand left of its consumer, or Deg0 \
                         faces away from it"
                    );
                    gutter_runs += 1;
                }
                // A source-side rerouter takes a value OUT of the body into
                // the band; a wire from one into a real port would be a run
                // back into the gates it just left.
                BusRole::Source => panic!(
                    "a source-side rerouter at ({}, {}) drives the real port {} .{}",
                    l.bus.nodes[i].x,
                    l.bus.nodes[i].y,
                    t.node_id,
                    t.port.as_str()
                ),
            }
        }
        assert!(mini_runs > 0, "fixture must drive a port from the mini-bus");
        assert!(gutter_runs > 0, "fixture must drive a port from the gutter");
    }

    /// Every bus node that drives a real port, grouped by the row its
    /// consumer stands in. Two runs leaving at the SAME level draw over each
    /// other, so this is the raw material for both stagger tests.
    fn drivers_by_row(m: &Module, l: &LayoutResult, role: BusRole) -> HashMap<i32, Vec<usize>> {
        let mut out: HashMap<i32, Vec<usize>> = HashMap::default();
        for w in &l.bus.wires {
            let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) else {
                continue;
            };
            if l.bus.nodes[i].role != role {
                continue;
            }
            let Some(node) = m.nodes.get(&t.node_id) else {
                continue;
            };
            if !l.placements.contains_key(&t.node_id) {
                continue;
            }
            out.entry(row_top(node, l)).or_default().push(i);
        }
        out
    }

    /// Taps that share a row must not share a LEVEL.
    ///
    /// Several lanes tapping one row is the normal case — a statement reading
    /// two variables does it — and each of their runs leaves the gutter
    /// horizontally toward that row. At one level those runs are drawn on top
    /// of each other and the picture stops being readable, however clean the
    /// bricks underneath are. Offsetting each by a rerouter's height gives
    /// every lane its own line out.
    #[test]
    fn gutter_taps_sharing_a_row_are_staggered() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let chains = bus_chains(&l);
        let chain_of = |i: usize| -> Option<usize> {
            chains
                .iter()
                .position(|c| c.lanes.contains(&i) || c.taps.contains(&i))
        };

        let mut shared_rows = 0usize;
        for (row, drivers) in &drivers_by_row(&m, &l, BusRole::Gutter) {
            let lanes: HashSet<usize> = drivers.iter().filter_map(|&i| chain_of(i)).collect();
            if lanes.len() < 2 {
                continue;
            }
            shared_rows += 1;
            let mut levels: Vec<i32> = drivers.iter().map(|&i| l.bus.nodes[i].x).collect();
            levels.sort_unstable();
            levels.dedup();
            assert_eq!(
                levels.len(),
                lanes.len(),
                "row at {row}: {} lanes tap it but their runs leave at {} \
                 level(s) {levels:?}",
                lanes.len(),
                levels.len()
            );
        }
        assert!(
            shared_rows > 0,
            "fixture must have a row tapped by two lanes, or this proves nothing"
        );

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// Each staggered stop moves as ONE piece: the tap standing out in the
    /// gutter and every gate-side rerouter it drives share a level, so the run
    /// between them is horizontal. Shifting only one end is what would put the
    /// diagonals back.
    #[test]
    fn a_staggered_stop_stays_level_with_itself() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let mut checked = 0usize;
        for w in &l.bus.wires {
            let (BusEnd::Bus(a), BusEnd::Bus(b)) = (w.source, w.target) else {
                continue;
            };
            let (na, nb) = (&l.bus.nodes[a], &l.bus.nodes[b]);
            // Tap -> gate-side, and gate-side -> gate-side: the horizontal run
            // out of the gutter. Both ends are Deg0 leaves.
            if na.role != BusRole::Gutter
                || nb.role != BusRole::Gutter
                || na.rotation != NodeRotation::Deg0
                || nb.rotation != NodeRotation::Deg0
            {
                continue;
            }
            checked += 1;
            assert_eq!(
                na.x, nb.x,
                "a stop's run must stay level: {a} at x={} drives {b} at x={}",
                na.x, nb.x
            );
        }
        assert!(checked > 4, "fixture must run real taps, got {checked}");
    }

    /// The same rule for the mini-bus. Two operands dropping into one
    /// statement leave their corners on that statement's row, and at one level
    /// their runs into it are drawn on top of each other — the same collision,
    /// in the line's own block rather than out in the gutter.
    #[test]
    fn mini_bus_corners_sharing_a_sink_are_staggered() {
        let m = lowered(MINI_BUS_SRC);
        let l = layout_code(&m, &opts(), false);

        let mut shared = 0usize;
        for (row, drivers) in &drivers_by_row(&m, &l, BusRole::Line) {
            if drivers.len() < 2 {
                continue;
            }
            shared += 1;
            let mut levels: Vec<i32> = drivers.iter().map(|&i| l.bus.nodes[i].x).collect();
            levels.sort_unstable();
            levels.dedup();
            assert_eq!(
                levels.len(),
                drivers.len(),
                "sink row at {row}: {} corners drop into it but leave at {} \
                 level(s) {levels:?}",
                drivers.len(),
                levels.len()
            );
        }
        assert!(
            shared > 0,
            "fixture must drop two operands into one statement"
        );

        // A staggered corner must still stand inside the sink's own vertical
        // extent, or the run into it leaves the gate it is meant to feed.
        for w in &l.bus.wires {
            let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) else {
                continue;
            };
            let node = &l.bus.nodes[i];
            if node.role != BusRole::Line {
                continue;
            }
            let sink = &m.nodes[&t.node_id];
            let at = l.placements[&t.node_id];
            let top = at.x + measured_half_size(sink, &l).0 * 2;
            assert!(
                node.x >= at.x && node.x + 2 * REROUTER_HALF <= top,
                "a staggered corner at x={} left its sink's band [{}, {top})",
                node.x,
                at.x
            );
        }
        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// A handler driven by a declared exec port, with a body long enough to
    /// sequence one statement into the next — so the same fixture carries both
    /// an exec DELIVERY (port → body) and exec SEQUENCING (body gate → body
    /// gate), and the two must be treated differently.
    const EXEC_DELIVERY_SRC: &str = "var score: int = 0
var tick: int = 0
var log: string[]
in start: exec
on start {
  score = 0
  PrintToConsole(\"reset\")
  PrintToConsole(\"done\")
  log.push(\"p${tick}\")
  PrintToConsole(\"q${tick}\")
  tick = tick + 1
  log.push(\"r${tick}\")
}
";

    /// Every exec wire out of a declared `in <name>: exec` port, paired with
    /// the module wire it is, over `EXEC_DELIVERY_SRC`.
    fn exec_deliveries<'a>(m: &'a Module) -> Vec<&'a Wire> {
        m.wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| {
                m.nodes
                    .get(&w.source.node_id)
                    .is_some_and(|n| n.kind == NodeKind::Input)
            })
            .filter(|w| targets_exec(m, &w.target))
            .collect()
    }

    /// An `in <name>: exec` port handing exec to a handler in the SAME module
    /// is a DELIVERY, not spine sequencing, and takes a lane like any other
    /// value.
    ///
    /// The exclusion this pins the correction to was written for the spine:
    /// one statement handing the chain to the next, which must stay a short
    /// direct wire on its own line. A wire out of an input PORT is not that.
    /// The port stacks on the page's left edge, so its direct wire is exactly
    /// the long diagonal across the whole body that the gutter exists to
    /// remove — the same shape as the delivery into a chip, which already
    /// buses.
    #[test]
    fn an_input_ports_exec_delivery_into_a_handler_is_bussed() {
        let m = lowered(EXEC_DELIVERY_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let deliveries = exec_deliveries(&m);
        assert_eq!(
            deliveries.len(),
            1,
            "fixture must hand exec from one declared port into its handler"
        );
        let w = deliveries[0];
        assert!(
            l.bus
                .suppressed
                .contains(&(w.target.node_id, w.target.port)),
            "the exec delivery {} .{} -> {} .{} still runs as a direct wire",
            w.source.node_id,
            w.source.port.as_str(),
            w.target.node_id,
            w.target.port.as_str()
        );

        // ...and a lane really carries it: a bus node drives the port, and it
        // stands on the CONSUMING row, not out at the port's own.
        let driver = l
            .bus
            .wires
            .iter()
            .find(|bw| bw.target == BusEnd::Node(w.target))
            .and_then(|bw| match bw.source {
                BusEnd::Bus(i) => Some(&l.bus.nodes[i]),
                BusEnd::Node(_) => None,
            })
            .expect("a bus node drives the suppressed exec port");
        let target_at = l.placements[&w.target.node_id];
        let target = &m.nodes[&w.target.node_id];
        let top = target_at.x + measured_half_size(target, &l).0 * 2;
        assert!(
            driver.x >= target_at.x && driver.x < top,
            "the tap sits at x {}, off its consumer's row [{}, {top})",
            driver.x,
            target_at.x
        );
        assert_eq!(
            driver.rotation,
            NodeRotation::Deg0,
            "an exec consumer is fed by a right-pointing tap like any other"
        );

        // The lane is headed beside the port itself, pointing down.
        let head = l
            .bus
            .wires
            .iter()
            .find_map(|bw| match (bw.source, bw.target) {
                (BusEnd::Node(p), BusEnd::Bus(i)) if p.node_id == w.source.node_id => {
                    Some(&l.bus.nodes[i])
                }
                _ => None,
            })
            .expect("the port heads a lane");
        assert_eq!(
            head.x, l.placements[&w.source.node_id].x,
            "the head stands at the port's own row"
        );
        assert_eq!(head.rotation, NodeRotation::Deg90, "a lane head points down");

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// The other half, and the reason the exclusion exists at all: an exec
    /// wire from one BODY GATE to the next is spine sequencing and stays a
    /// direct wire. Routing it through the gutter would take the spine off its
    /// own line, which is the spec's stated non-goal.
    #[test]
    fn body_gate_exec_sequencing_stays_direct() {
        let m = lowered(EXEC_DELIVERY_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let sequencing: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| targets_exec(&m, &w.target))
            // A STATEMENT gate, which the layout records by turning it down
            // the spine. An expression-side `Exec_Var_Get` is also a Gate
            // handing exec on, but it lives in the value columns and its
            // exec-out is an expression value — the mini-bus turns those
            // deliberately, and reading `kind == Gate` here would call that a
            // regression.
            .filter(|w| {
                m.nodes.contains_key(&w.source.node_id)
                    && rotation_of(&l.rotations, &w.source.node_id) == NodeRotation::Deg90
            })
            .filter(|w| m.nodes.contains_key(&w.target.node_id))
            .collect();
        assert!(
            sequencing.len() >= 2,
            "fixture must chain at least three statements, got {} exec hops",
            sequencing.len()
        );
        for w in &sequencing {
            assert!(
                !l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the spine hop {} .{} -> {} .{} was detoured through the gutter",
                w.source.node_id,
                w.source.port.as_str(),
                w.target.node_id,
                w.target.port.as_str()
            );
        }
    }

    /// The mirror of a delivery: a value LEAVING a chip. Its producing pin
    /// lives in the CHILD module and holds no row here, so the lane head —
    /// which always stands beside its source, pointing down — anchors on the
    /// chip brick the value comes out of.
    #[test]
    fn values_leaving_a_chip_head_a_lane_at_the_chips_row() {
        let src = "in go: exec
var tick: int = 0
var log: string[]
chip Scorer(run: exec, amount: int) -> (total: int) {
  var score: int = 0
  on run { score = score + amount }
  out total = score
}
let scored = Scorer(go, 5)
on go {
  PrintToConsole(\"${scored.total}\")
  PrintToConsole(\"x${scored.total}\")
  log.push(\"p${tick}\")
  PrintToConsole(\"q${tick}\")
  tick = tick + 1
  log.push(\"r${tick}\")
}
";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        // An exit: a wire landing on this module's body whose source is not a
        // node of this module, so it leaves some chip.
        let exits: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| !m.nodes.contains_key(&w.source.node_id))
            .filter(|w| m.nodes.contains_key(&w.target.node_id))
            .collect();
        assert!(
            !exits.is_empty(),
            "fixture must read a value back out of a chip"
        );

        let owners = chip_owners(&m);
        for w in &exits {
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the exit {} .{} -> {} .{} still runs as a direct wire",
                w.source.node_id,
                w.source.port.as_str(),
                w.target.node_id,
                w.target.port.as_str()
            );
            let chip_id = *owners
                .owner
                .get(&w.source.node_id)
                .expect("an exit leaves some chip of this module");
            let chip_at = l.placements[&chip_id];
            // The chip brick is a body producer like any other, so its value
            // reaches the head through the source-side rerouter standing
            // beside it. Reading the wire's target as the head directly would
            // land on that rerouter and pin the wrong brick's facing.
            let head = l
                .bus
                .wires
                .iter()
                .find(|bw| bw.source == BusEnd::Node(w.source))
                .and_then(|bw| match bw.target {
                    BusEnd::Bus(i) => Some(&l.bus.nodes[through_source_side(&l, i)]),
                    BusEnd::Node(_) => None,
                })
                .expect("a lane head reads the chip's output");
            let top = chip_at.x + measured_half_size(&m.nodes[&chip_id], &l).0 * 2;
            assert!(
                head.x >= chip_at.x && head.x < top,
                "lane head for {} .{} sits at x {}, off chip {chip_id}'s row [{}, {top})",
                w.source.node_id,
                w.source.port.as_str(),
                head.x,
                chip_at.x
            );
            assert_eq!(
                head.rotation,
                NodeRotation::Deg90,
                "a lane head points down"
            );
        }

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
    }

    /// The smallest body that both reads ONE value across two rows and earns
    /// a bus.
    ///
    /// The opening two reads are the shape the lane tests pin; everything
    /// after is bulk. It is there because a bus has to pay for itself — a body
    /// of a few gates keeps its direct wires by design — so a test about what
    /// a lane LOOKS like has to be handed a body that builds one.
    const TWO_ROW_BUS_SRC: &str = "var a: int = 1
var b: int = 2
var log: string[]
in go: exec
on go {
  PrintToConsole(\"${a}\")
  PrintToConsole(\"x${a}\")
  log.push(\"p${b}\")
  PrintToConsole(\"q${a}${b}\")
  b = a + b
  log.push(\"r${b}\")
}
";

    /// One statement consuming an expression built from TWO operand columns —
    /// the shape the mini-bus turns — on a body big enough to earn a bus.
    const MINI_BUS_SRC: &str = "var x: int = 0
var y: int = 1
var log: string[]
in t: exec
on t {
  x = (1 + x) * (2 + x)
  log.push(\"p${y}\")
  PrintToConsole(\"q${x}${y}\")
  y = y + 1
  log.push(\"r${y}\")
}
";

    /// A body dense enough to exercise the band: two variables read across
    /// six rows, some of them twice on one row.
    const BAND_SRC: &str = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${b}\")\n  PrintToConsole(\"${a}\")\n  b = a + b\n  log.push(\"y${b}\")\n}\n";

    /// The same band with no declared input port, so nothing stacks further
    /// left than the gutter and the band itself is the plane's left edge.
    const NO_PIN_BAND_SRC: &str = "var a: int = 1
var b: int = 2
var log: string[]
on ControllerJoined(who) {
  log.push(\"${a}\")
  PrintToConsole(\"${a}${b}\")
  log.push(\"${b}\")
  PrintToConsole(\"${a}\")
  b = a + b
  log.push(\"y${b}\")
}
";

    /// The end-to-end reachability oracle: every consumer whose own wire the
    /// bus took over must still RECEIVE the value, by a path of bus wires
    /// from the port that produces it.
    ///
    /// This is the feature's worst failure mode and the only one with no
    /// symptom: a suppressed wire whose replacement path does not exist
    /// compiles, loads, and pastes cleanly, and the value simply never
    /// arrives. Nothing else here proves it — `emit_lane` leaves out a
    /// rerouter whose cell is contested and re-points the run at whatever
    /// precedes it, so which node a consumer ends up hanging off is not
    /// decidable by reading one statement. Only the walk settles it.
    ///
    /// The walk follows `bus.wires` and nothing else. The suppressed wire
    /// itself is never followed — that is the wire being replaced — so a bus
    /// that suppressed a consumer and wired it to nothing cannot pass.
    ///
    /// Returns the number of consumers proven reachable, so a caller can
    /// refuse a fixture that proves nothing.
    fn assert_suppressed_consumers_stay_reachable(m: &Module, l: &LayoutResult) -> usize {
        let mut out: HashMap<BusEnd, Vec<BusEnd>> = HashMap::default();
        for w in &l.bus.wires {
            out.entry(w.source).or_default().push(w.target);
        }
        for (node_id, port) in l.bus.suppressed.iter() {
            let target = PortRef {
                node_id: *node_id,
                port: *port,
            };
            // The module wire the bus replaced names the value's real
            // producer. Fan-in is illegal, so there is exactly one.
            let source = m
                .wires
                .iter()
                .find(|w| w.target == target)
                .unwrap_or_else(|| panic!("suppressed {target:?} replaced no module wire"))
                .source;

            let mut seen: HashSet<BusEnd> = HashSet::default();
            let mut queue: VecDeque<BusEnd> = VecDeque::new();
            queue.push_back(BusEnd::Node(source));
            let mut reached = false;
            while let Some(cur) = queue.pop_front() {
                if cur == BusEnd::Node(target) {
                    reached = true;
                    break;
                }
                if !seen.insert(cur) {
                    continue;
                }
                for &next in out.get(&cur).into_iter().flatten() {
                    queue.push_back(next);
                }
            }
            assert!(
                reached,
                "{} .{} lost its value: its wire from {} .{} is suppressed and no \
                 path of bus wires reaches it",
                target.node_id,
                target.port.as_str(),
                source.node_id,
                source.port.as_str()
            );
        }
        l.bus.suppressed.len()
    }

    /// Every oracle above, at EVERY level of the chip tree.
    ///
    /// The single-level spellings all take a `recurse = false` layout, whose
    /// `chip_layouts` is empty by construction — so a chip's own interior bus
    /// has never been swept for overlap, fan-in, or a stranded consumer. Only
    /// the ROOT's deliveries into chips were, and those are a different set of
    /// wires built by a different call. That is exactly the surface the last
    /// real defect lived on: three independent filters were silently dropping
    /// foreign endpoints, and the outer anonymous chip's bus went from 0 nodes
    /// to 9 once they were fixed — a whole band that no assertion had ever
    /// looked at.
    ///
    /// Walks `(module, layout)` in lockstep: `lr.chip_layouts` is keyed by chip
    /// node id and `module.chips` holds the child module under the same key, so
    /// a layout with no module is itself a failure. Returns the total consumers
    /// proven reachable across the whole tree.
    fn assert_layout_tree_is_sound(module: &Module, lr: &LayoutResult) -> usize {
        assert_no_overlap(module, lr);
        assert_no_fan_in(lr);
        let mut proven = assert_suppressed_consumers_stay_reachable(module, lr);
        // Deterministic order, so a failure names the same chip every run.
        let mut ids: Vec<NodeId> = lr.chip_layouts.keys().copied().collect();
        ids.sort();
        for id in ids {
            let child = module
                .chips
                .get(&id)
                .unwrap_or_else(|| panic!("chip layout {id} has no child module"));
            proven += assert_layout_tree_is_sound(child, &lr.chip_layouts[&id]);
        }
        proven
    }

    /// A big root and a tiny chip, so one tree carries both decisions.
    const MIXED_BUS_SRC: &str = "var a: int = 1
var b: int = 2
var log: string[]
in go: exec
chip Tiny(t: exec) -> (n: int) {
  var h: int = 0
  on t { h = h + 1 }
  out n = h
}
let tiny = Tiny(go)
on go {
  log.push(\"${a}\")
  PrintToConsole(\"${a}${b}\")
  log.push(\"${b}\")
  PrintToConsole(\"${a}\")
  b = a + b
  log.push(\"y${b}\")
  PrintToConsole(\"${tiny.n}\")
}
";

    /// A bus has to earn its bricks. On a body of a few gates the lanes cost
    /// more rerouters than the gates they serve, and the plane reads worse for
    /// having them — so a module that small keeps its direct wires.
    ///
    /// The decision is per module and ALL-OR-NOTHING: a module that drops its
    /// bus must be indistinguishable from one built before the feature
    /// existed, which means no nodes, no wires and — the part that would
    /// silently break things — no suppressed entries either. A module left
    /// holding suppression it has no lanes to honour would drop every one of
    /// those wires at emit and strand the consumers.
    #[test]
    fn a_small_chip_drops_its_bus_while_a_big_module_keeps_one() {
        let m = lowered(MIXED_BUS_SRC);
        let l = layout_code(&m, &code_opts(), true);

        assert!(
            !l.bus.nodes.is_empty(),
            "the root body is big enough to earn its bus"
        );
        assert!(
            !l.bus.suppressed.is_empty(),
            "...and to suppress the wires it replaced"
        );

        let (chip_id, chip) = l
            .chip_layouts
            .iter()
            .next()
            .expect("the fixture must lay out its chip");
        let chip_module = &m.chips[chip_id];
        let gates = chip_module.nodes.values().filter(|n| is_spawnable(n)).count();
        assert!(
            gates < 12,
            "fixture's chip must stay small, got {gates} spawnable gates"
        );
        assert!(
            chip.bus.is_empty(),
            "a {gates}-gate chip must build no bus at all, got {} nodes / {} \
             wires / {} suppressed",
            chip.bus.nodes.len(),
            chip.bus.wires.len(),
            chip.bus.suppressed.len()
        );

        // The mix is the new case: one tree, one module bussed and one not.
        assert_layout_tree_is_sound(&m, &l);
    }

    /// Chips in both directions and two levels deep: the outer chip takes a
    /// value in and hands one back, and the chip nested inside it does the
    /// same again — so every level owns a bus that carries a foreign endpoint.
    const NESTED_CHIP_SRC: &str = "var score: int = 0
var log: string[]
in go: exec
on go {
  score = 0
  chip {
    var acc: int = 0
    acc = score + 1
    log.push(\"o${acc}\")
    chip {
      var inner: int = 0
      inner = acc + score
      log.push(\"i${inner}\")
      log.push(\"j${inner}${acc}\")
      score = inner + acc
    }
    log.push(\"p${acc}${score}\")
  }
  PrintToConsole(\"${score}\")
  PrintToConsole(\"x${score}\")
}
";

    /// The whole tree, swept. Without the recursion this passes on an empty
    /// `chip_layouts` and proves nothing about any chip's own band.
    #[test]
    fn every_level_of_the_chip_tree_holds_the_bus_invariants() {
        let m = lowered(NESTED_CHIP_SRC);
        let l = layout_code(&m, &code_opts(), true);

        // The fixture has to actually reach two levels, or the recursion is
        // exercised against nothing.
        fn depth(lr: &LayoutResult) -> usize {
            1 + lr.chip_layouts.values().map(depth).max().unwrap_or(0)
        }
        fn bus_nodes(lr: &LayoutResult) -> usize {
            lr.bus.nodes.len() + lr.chip_layouts.values().map(bus_nodes).sum::<usize>()
        }
        assert!(
            depth(&l) >= 3,
            "fixture must nest two levels of chip under the root, got depth {}",
            depth(&l)
        );
        assert!(
            !l.chip_layouts.is_empty(),
            "fixture must lay out its chips recursively"
        );

        // Every chip interior builds a band of its own, so the recursion has
        // something to sweep at each level.
        let mut with_a_bus = 0usize;
        let mut stack: Vec<&LayoutResult> = l.chip_layouts.values().collect();
        while let Some(child) = stack.pop() {
            if !child.bus.nodes.is_empty() {
                with_a_bus += 1;
            }
            stack.extend(child.chip_layouts.values());
        }
        assert!(
            with_a_bus >= 2,
            "both nesting levels must build a bus of their own, got {with_a_bus}"
        );
        assert!(
            bus_nodes(&l) > l.bus.nodes.len(),
            "the chips' own bands must be part of what is swept"
        );

        let proven = assert_layout_tree_is_sound(&m, &l);
        assert!(
            proven > 8,
            "the tree must suppress a band's worth of wires, got {proven}"
        );
    }

    #[test]
    fn every_suppressed_consumer_still_receives_its_value() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let proven = assert_suppressed_consumers_stay_reachable(&m, &l);
        assert!(
            proven > 8,
            "fixture must suppress a band's worth of wires, got {proven}"
        );
    }

    /// The same oracle over the shapes whose replacement path is least
    /// obvious: chip deliveries and exits, whose tap anchors on a chip brick
    /// while the wire's own endpoint lives in another module.
    ///
    /// Laid with `recurse = true` and walked through the tree, so the chips'
    /// OWN bands are held to it too — the root's deliveries into a chip and
    /// the band a chip builds for its interior are separate sets of wires from
    /// separate calls, and only the walk covers the second.
    #[test]
    fn chip_crossings_stay_reachable_through_the_bus() {
        let src = "var score: int = 0\nvar log: string[]\nin go: exec\nchip Scorer(run: exec, amount: int) -> (total: int) {\n  on run {\n    score = score + amount\n    log.push(\"s\")\n  }\n  out total = score\n}\nlet scored = Scorer(go, 5)\non go {\n  score = 0\n  chip {\n    score = score + 1\n    log.push(\"i\")\n  }\n  PrintToConsole(\"${scored.total}\")\n  PrintToConsole(\"x${scored.total}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &code_opts(), true);
        assert!(
            !l.chip_layouts.is_empty(),
            "fixture must lay out its chips' interiors"
        );
        let proven = assert_layout_tree_is_sound(&m, &l);
        assert!(proven > 8, "fixture must cross chip walls, got {proven}");
        // The crossings themselves: wires whose target sits in another
        // module. These are the ones the oracle exists for.
        let crossings = l
            .bus
            .suppressed
            .iter()
            .filter(|(id, _)| !m.nodes.contains_key(id))
            .count();
        assert!(crossings > 0, "fixture must suppress a chip delivery");
    }

    /// Every consumer is fed from a gate-side rerouter standing in its OWN
    /// column — one tap reserve left of the gate it drives — unless another
    /// lane's rerouter already stands in that exact cell.
    ///
    /// A run that starts further back along the row crosses the gates in
    /// between, which is the diagonal the band exists to remove. Before
    /// `TAP_RESERVE` the cell mostly did not exist: a row packed its columns
    /// flush, so the gate one column left held it and the consumer fell back
    /// to the band tap out in the gutter.
    ///
    /// The case the reserve cannot cover is a gate consuming two bussed
    /// values at once: both lanes want the same single cell and one has to
    /// read from further back. That exemption is spelled out as "somebody
    /// else is standing exactly there", so an EMPTY cell — the shape the
    /// reserve fixes — still fails.
    #[test]
    fn every_consumer_column_gets_its_own_gate_side_tap() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let mut checked = 0usize;
        let mut shared = 0usize;
        for w in &l.bus.wires {
            let BusEnd::Node(target) = w.target else {
                continue;
            };
            // Foreign endpoints hold no placement in this module.
            let Some(p) = l.placements.get(&target.node_id) else {
                continue;
            };
            let BusEnd::Bus(i) = w.source else {
                panic!("a consumer is driven by a raw port, not by a tap");
            };
            let driver = &l.bus.nodes[i];
            // A mini-bus corner stands in its OPERAND's column, not one tap
            // reserve left of the gate it feeds — it is turning a wire, not
            // marching along a row. `the_mini_bus_drops_an_expression_value_
            // into_its_statement` is what holds it to its own geometry.
            if driver.role != BusRole::Gutter {
                continue;
            }
            let own_column = p.y - TAP_RESERVE;
            checked += 1;
            if driver.y == own_column {
                continue;
            }
            let holder = l
                .bus
                .nodes
                .iter()
                .any(|n| n.z == driver.z && n.y == own_column && n.x == driver.x);
            assert!(
                holder,
                "{} .{} is fed from y={}, and the cell in its own column at \
                 y={own_column} stands empty",
                target.node_id,
                target.port.as_str(),
                driver.y
            );
            shared += 1;
        }
        assert!(checked > 8, "fixture must tap a band's worth of gates");
        assert!(
            shared * 4 < checked,
            "the reserve must cover the great majority of columns; \
             {shared} of {checked} fell back"
        );
    }

    /// The bus's own bricks against the body's, measured at the rerouter's
    /// real footprint rather than the 5×5 gate default. A tap stands INSIDE
    /// the body's rows, so a mis-sized bus brick lands in a gate and the game
    /// silently drops one of the two.
    #[test]
    fn bus_nodes_never_overlap_gates_or_each_other() {
        let src = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${a}\")\n  PrintToConsole(\"${b}\")\n  log.push(\"${a}${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "fixture must build a bus");
        assert_no_overlap(&m, &l);
    }

    /// A page's grid is sized from the layout bounds and centred on the
    /// origin, so a brick outside the plane extent is a brick outside the
    /// emitted microchip. The band hangs off the body's left edge — the side
    /// nothing else reaches — so it is the part most likely to fall out.
    #[test]
    fn bus_nodes_sit_inside_the_plane_extent() {
        // A whole band, not a single lane: the extent is derived from the
        // bounds with a 5-unit margin on each side, so a one-lane band is
        // narrow enough to hide inside the margin even when the bounds have
        // forgotten it entirely.
        let m = lowered(NO_PIN_BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(l.bus.nodes.len() > 8, "fixture must build a real band");
        let e = crate::layout::wall::plane_extent(&l);
        for (i, n) in l.bus.nodes.iter().enumerate() {
            assert!(
                n.x >= -e.x
                    && n.x + 2 * MEASURED_BUS_HALF <= e.x
                    && n.y >= -e.y
                    && n.y + 2 * MEASURED_BUS_HALF <= e.y,
                "bus node {i} at ({}, {}) escapes the extent ({}, {})",
                n.x,
                n.y,
                e.x,
                e.y
            );
            assert!(n.z <= e.z, "bus node {i} at z {} escapes {}", n.z, e.z);
        }
    }

    /// A row is `(page, sub-row offset)`, and `assemble_pages` restarts each
    /// band's offsets at zero — so two bands standing side by side on one
    /// page contribute their top rows under the SAME key, and one lane stop
    /// serves both. This pins that, deliberately.
    ///
    /// It is not a fault to route around. The gutter band sits left of the
    /// whole page, so a value delivered into the second band has to cross the
    /// first one however the rows are keyed; keying rows per band would only
    /// start that crossing further left, at the gutter tap, instead of at the
    /// first band's own last tapped column. The merged key gives the shorter
    /// run, and the row's cells are still checked as one set, so a tap can
    /// never land inside a gate of either band.
    ///
    /// What it costs is a run that visibly crosses the gutter between bands.
    /// The assertions below are the shape of that run, plus the invariants
    /// that have to survive it.
    #[test]
    fn bands_sharing_a_page_share_their_row_keys() {
        let src = "var a: int = 1
var b: int = 2
var log: string[]
in go: exec
on go {
  PrintToConsole(\"${a}\")
  PrintToConsole(\"x${a}\")
  PrintToConsole(\"y${a}\")
  PrintToConsole(\"z${a}\")
  log.push(\"p${b}\")
  PrintToConsole(\"q${a}${b}\")
  b = b + 1
  log.push(\"r${b}\")
}
";
        let m = lowered(src);
        // Short enough to split the body into bands, and the default plane
        // budget is wide enough to keep them on one page.
        let budgets = CodeBudgets {
            band_height: 80,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &LayoutOptions::default(), false, &budgets);
        let planes: BTreeSet<i32> = l.placements.values().map(|p| p.z).collect();
        assert_eq!(planes.len(), 1, "fixture must keep every band on one page");

        // Two bands' gates landing on one row read as a gap wider than the
        // gutter standing between the bands.
        let mut rows: HashMap<i32, Vec<i32>> = HashMap::default();
        for (id, p) in &l.placements {
            let (hsx, _) = measured_half_size(&m.nodes[id], &l);
            rows.entry(p.x + hsx * 2).or_default().push(p.y);
        }
        let mut split_rows = 0usize;
        for ys in rows.values_mut() {
            ys.sort_unstable();
            if ys.windows(2).any(|w| w[1] - w[0] > BAND_GUTTER) {
                split_rows += 1;
            }
        }
        assert!(
            split_rows > 0,
            "fixture must put two bands' gates under one row key"
        );

        // ...and one lane chains straight across that gap.
        let crossing = l
            .bus
            .wires
            .iter()
            .filter(|w| match (w.source, w.target) {
                (BusEnd::Bus(a), BusEnd::Bus(b)) => {
                    let (na, nb) = (&l.bus.nodes[a], &l.bus.nodes[b]);
                    na.rotation == NodeRotation::Deg0
                        && nb.rotation == NodeRotation::Deg0
                        && (na.y - nb.y).abs() > BAND_GUTTER
                }
                _ => false,
            })
            .count();
        assert!(crossing > 0, "the merged row must chain across the gutter");

        // What the merge must not cost: no brick landing inside another, no
        // port driven twice, and every consumer the bus took over still
        // receiving its value.
        assert_no_overlap(&m, &l);
        assert_no_fan_in(&l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// A LITERAL living in a sibling chip must not earn a lane.
    ///
    /// The endpoint is foreign, so the wire's tap anchors on the chip brick —
    /// and a chip brick is always bussable, so checking the anchor's class
    /// says nothing about the endpoint's. Emit gives a literal no brick, so
    /// the lane's wire fails to resolve and aborts the whole build with
    /// `BusWireUnresolved`. This is the shape that hard-failed a 25-line
    /// program through the real compiler path.
    #[test]
    fn a_literal_inside_a_sibling_chip_is_never_bussed() {
        let src = "var o1: int = 0
var o2: int = 0
var o3: int = 0
in go: exec
chip { let k = 0 }
chip {
  let a = k + 1
  let b = k + 2
  let c = k + 3
  let d = k + 4
  let e = k + 5
  let f = k + 6
}
on go {
  o1 = a + b
  o2 = c + d
  o3 = e + f
}
";
        let m = lowered(src);
        let l = layout_code(&m, &code_opts(), true);

        // Resolve every bus endpoint through the chip tree, which is exactly
        // what the production check has to do. A same-module lookup returns
        // `None` for a foreign node and would pass vacuously.
        fn classes(m: &Module, out: &mut HashMap<NodeId, &'static str>) {
            for (id, n) in &m.nodes {
                out.insert(*id, n.gate_class);
            }
            for c in m.chips.values() {
                classes(c, out);
            }
        }
        let mut all: HashMap<NodeId, &'static str> = HashMap::default();
        classes(&m, &mut all);

        fn walk(m: &Module, l: &LayoutResult, all: &HashMap<NodeId, &'static str>) {
            for w in &l.bus.wires {
                for end in [w.source, w.target] {
                    if let BusEnd::Node(p) = end {
                        let cls = all.get(&p.node_id).copied();
                        assert!(
                            cls.is_some_and(|c| c != gate_class::LITERAL
                                && c != gate_class::UNSUPPORTED),
                            "a lane claims {} ({cls:?}), which emit gives no brick",
                            p.node_id
                        );
                    }
                }
            }
            for (id, c) in &m.chips {
                if let Some(cl) = l.chip_layouts.get(id) {
                    walk(c, cl, all);
                }
            }
        }
        walk(&m, &l, &all);
        assert_layout_tree_is_sound(&m, &l);
    }

    #[test]
    fn literal_sources_are_never_bussed() {
        let m =
            lowered("in go: exec\non go {\n  PrintToConsole(\"a\")\n  PrintToConsole(\"b\")\n}\n");
        let l = layout_code(&m, &LayoutOptions::default(), false);
        for w in &l.bus.wires {
            if let crate::layout::BusEnd::Node(p) = w.source {
                let cls = m.nodes.get(&p.node_id).map(|n| n.gate_class);
                assert_ne!(cls, Some(crate::ir::gate_class::LITERAL));
            }
        }
    }
}
