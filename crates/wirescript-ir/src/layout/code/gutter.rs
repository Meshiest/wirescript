//! The gutter-bus planner: replace long value wires with lanes of rerouters
//! standing beside the code body.
#![cfg_attr(
    not(feature = "test-internals"),
    expect(unreachable_pub, reason = "public only for the test-internals export")
)]

use crate::collections::HashMap;
use crate::emit::Placement;
use crate::ir::port_registry::WirePort;
use crate::ir::{Module, Node, NodeId, NodeKind, PortRef, gate_class};
use crate::layout::bus::{BusDemand, allocate_lanes};
use crate::layout::{BusEnd, BusLayout, BusNode, BusNodeId, BusRole, BusWire, NodeRotation};

use super::adjacency::targets_exec;
use super::nodes::{cell_half_size, is_spawnable, rotation_of};
use super::pages::PageInfo;
use super::{
    BUS_BAND_GUTTER, LANE_PAIR_HEIGHT, LANE_PITCH, MIN_LANE_TRAVEL, REROUTER_HALF, STAGGER_STEP,
    TAP_GAP, TAP_RESERVE,
};

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
    /// without one (an edge pin, placed later on the far side of the band)
    /// takes no such brick.
    source_row: Option<usize>,
    page: usize,
    taps: Vec<BusTap>,
    /// The value crosses a chip wall (it leaves one, or some tap delivers
    /// into one) rather than running gate to gate across this body.
    crosses_chip: bool,
    /// Position of the group's first wire in `module.wires`: a stable
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
            gate_class::VAR | gate_class::ARRAY_VAR | gate_class::BUFFER | gate_class::BUFFER_TICKS
        )
}

/// Every node standing inside a chip nested under `module`, mapped to the
/// chip node that represents it HERE.
///
/// A wire into a chip targets a boundary pin in the CHILD module, which has no
/// row here, so its tap anchors on the chip brick instead.
pub fn chip_owners(module: &Module) -> ChipSubtree {
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
/// `owner` is the chip brick THIS module sees a foreign node through, which is
/// what a wire's tap anchors on. `class` is that node's real gate class, which the
/// anchor cannot stand in for: a chip node is always bussable, so checking the
/// anchor's class says nothing about whether the endpoint is a literal.
#[derive(Default)]
pub struct ChipSubtree {
    pub owner: HashMap<NodeId, NodeId>,
    class: HashMap<NodeId, &'static str>,
}

impl ChipSubtree {
    /// True when a node may carry a bus wire: it is neither inlined into its
    /// consumer's data (a literal) nor unrepresented (unsupported). Resolves
    /// through the subtree, so a FOREIGN endpoint is judged on its own class
    /// rather than on the chip brick it is reached through.
    ///
    /// A node this module has never heard of is not bussable: emit gives it no
    /// brick, so emit's bus-wire pass would have nothing to wire.
    fn is_bussable(&self, module: &Module, id: NodeId) -> bool {
        let class = match module.nodes.get(&id) {
            Some(n) => Some(n.gate_class),
            None => self.class.get(&id).copied(),
        };
        class.is_some_and(|c| !gate_class::is_inline_only(c))
    }
}

/// Group the module's value wires by `(page, source port)`.
///
/// Exec edges are left out only when they are SPINE SEQUENCING: one body
/// brick handing the chain to the next. Re-routing those through the gutter
/// would take the spine off its own line.
///
/// Every other exec wire is a DELIVERY and buses like any value: a wire into
/// a chip, or a wire out of an input port or boundary pin (stacked on the
/// page's edge). The test is on the wire's source, not its target's port type.
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
        // Exec out of an input port or boundary pin is a delivery from the
        // page's edge and buses. `is_bus_worthy_source` selects exactly those
        // here, since no storage gate has an exec output. A chip exit's
        // `source_anchor` is the chip brick, a body brick carrying the spine,
        // so exec handed back out of a chip keeps its direct wire.
        let source_delivers = module
            .nodes
            .get(&source_anchor)
            .is_some_and(is_bus_worthy_source);
        if !into_chip && !source_delivers && targets_exec(module, w.target) {
            continue;
        }
        // Check the REAL endpoints as well as the anchors: a chip anchor is
        // always bussable, but a foreign literal gets no brick from emit, so
        // its lane wire would fail to resolve and abort the build.
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

/// Push one lane brick and return its id. A lane brick is identified by the
/// node whose color it mirrors, not by a name.
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

/// Where a lane stops on its way down: the rows it taps, each with the `x` its
/// tap rerouter (and the gate-side rerouters it feeds) stands at. That `x` is
/// the row's first consumer, so the run out to the gates stays on its row.
///
/// Consecutive rows landing inside one rerouter's footprint repeat an `x` and
/// share a stop instead of stacking two bricks. Within one band rows never do
/// that; it happens where `assemble_pages` puts several bands on one page,
/// since each band's sub-row offsets start from zero and interleave.
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
/// rerouter in the lane's column level with it, then a pair at every stop down
/// that column: a right-pointing tap on the row's level, feeding the gate-side
/// rerouters of that row's consumers, and a down-pointing link above it
/// carrying the chain on.
///
/// A tap is a BRANCH off its link and never hands the lane on, so no `Deg0`
/// node drives a `Deg90` one. Every rerouter and consumer port takes exactly
/// one inbound wire.
///
/// The game silently drops overlapping bricks, so a brick whose cell is taken
/// (`row_cells` for row cells, `lane_cells` for the column) is left out, never
/// moved; a consumer whose gate-side rerouter is skipped hangs off the
/// previous rerouter on the row. Lane 0 is the LEFTMOST column.
#[expect(
    clippy::too_many_arguments,
    reason = "a lane reads its group, stops and page and writes both cell maps"
)]
fn emit_lane(
    bus: &mut BusLayout,
    body: &Body<'_>,
    group: &BusGroup,
    demand: &BusDemand,
    stops: &[(usize, i32)],
    lane: usize,
    lanes_used: usize,
    page: usize,
    row_cells: &mut HashMap<(usize, usize), Vec<Cell>>,
    lane_cells: &mut LaneCells,
) {
    let placements = body.placements;
    let (body_min_y, z) = (body.page_infos[page].min_y, body.page_infos[page].z);
    let lane_y = body_min_y - BUS_BAND_GUTTER - (lanes_used - lane) as i32 * LANE_PITCH;
    // Color and head position both read the anchor, not the port: a value
    // read back out of a chip is produced in the child module, which this
    // module cannot place or color from.
    let color_of = Some(group.source_anchor);

    // The chain points up when every tap is above the source. Only the CHAIN
    // turns: taps and gate-side rerouters still face right into the gate they
    // feed, because a rerouter must face the thing it feeds.
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
    if let Some(p) = placements.get(&group.source_anchor)
        && claim_column(lane_cells, (page, lane), p.x, p.x + 2 * REROUTER_HALF)
    {
        // The source-side rerouter: one tap reserve to the producer's LEFT on
        // its own level, facing `-Y` (the way the value travels, into the
        // band). Offered only when the head is placed, so the run between them
        // is horizontal. Its cell is claimed from `row_cells`; when taken (the
        // producer also consumes a bussed value there, or a second lane leaves
        // the same brick) it is left out and the producer drives the head.
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

        let id = push_bus_node(
            bus,
            p.x,
            lane_y,
            z,
            chain_rotation,
            BusRole::Gutter,
            color_of,
        );
        bus.wires.push(BusWire {
            source: upstream,
            target: BusEnd::Bus(id),
        });
        upstream = BusEnd::Bus(id);
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
                let id = push_bus_node(
                    bus,
                    stop_x,
                    lane_y,
                    z,
                    NodeRotation::Deg0,
                    BusRole::Gutter,
                    color_of,
                );
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
                let gate_id = push_bus_node(
                    bus,
                    stop_x,
                    gate_y,
                    z,
                    NodeRotation::Deg0,
                    BusRole::Gutter,
                    color_of,
                );
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
/// Planning and laying are split around the pin stack: the stack has to clear
/// the band the lanes claim, and a lane head stands at its source's row, which
/// an edge-pin source only has once the stack is placed.
#[derive(Default)]
pub(super) struct BusPlan {
    /// The lanes planned on each page.
    pub(super) pages: Vec<PageBus>,
    groups: Vec<BusGroup>,
    /// Cells the body already occupies on each row, so a gate-side tap that
    /// cannot fit beside its gate is not placed.
    row_cells: HashMap<(usize, usize), Vec<Cell>>,
    /// `(page, sub-row offset)` -> row index, so a later pass can name the
    /// same rows `row_cells` is keyed by.
    pub(super) row_index: HashMap<(usize, i32), usize>,
    /// `(page, row)` -> that row's top edge. Every node in a row shares it
    /// whatever its own height, so it is the ceiling a staggered run may not
    /// climb past without leaving the row it serves.
    row_top: HashMap<(usize, usize), i32>,
}

/// The gutter lanes planned on one page.
#[derive(Clone, Default)]
pub(super) struct PageBus {
    /// Width of the gutter band, for the input-pin stack to clear.
    pub(super) band_width: i32,
    /// In emission order: the group a lane carries, its demand, and its lane
    /// index.
    lanes: Vec<(usize, BusDemand, usize)>,
    /// Lane count; lane 0 is the leftmost of them.
    lanes_used: usize,
}

/// A laid-out code body, as the gutter bus reads it: the module and where its
/// nodes landed.
#[derive(Clone, Copy)]
pub(super) struct Body<'a> {
    pub(super) module: &'a Module,
    pub(super) placements: &'a HashMap<NodeId, Placement>,
    pub(super) rotations: &'a HashMap<NodeId, NodeRotation>,
    /// The `(page, sub-row offset)` each body node sits in.
    pub(super) node_row: &'a HashMap<NodeId, (usize, i32)>,
    pub(super) page_infos: &'a [PageInfo],
    pub(super) owners: &'a ChipSubtree,
}

/// Pick the values worth a lane and hand each one a column: one lane per
/// bussed value, packed into the band between the code body's left edge and
/// the input-pin stack.
///
/// Lanes are allocated per page (a separate plane in z), so a value read on
/// two pages heads a lane on each.
///
/// KNOWN DEVIATION: a page's size and [`PageInfo`] are measured before the
/// lanes exist and are never widened for them. Nothing collides (the pin stack
/// clears `band_width`, and layout bounds include bus nodes), but each page of
/// a paginated layout is centered on its body alone, so pages with different
/// lane counts sit slightly off-center from one another.
pub(super) fn plan_bus(body: &Body<'_>) -> BusPlan {
    let Body {
        module,
        placements,
        rotations,
        node_row,
        page_infos,
        owners,
    } = *body;
    let mut plan = BusPlan {
        pages: vec![PageBus::default(); page_infos.len()],
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
    for (id, &(page, down)) in node_row {
        let (Some(p), Some(n), Some(&row)) = (
            placements.get(id),
            module.nodes.get(id),
            row_index.get(&(page, down)),
        ) else {
            continue;
        };
        let (hsx, hsy) = cell_half_size(n, rotation_of(rotations, *id));
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

    let groups = collect_bus_groups(module, placements, node_row, &row_index, owners);

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
            // is already a short in-line wire. A chip crossing runs to the
            // chip brick, a long diagonal, so it always gets a lane.
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
        plan.pages[pi] = PageBus {
            band_width: lanes_used as i32 * LANE_PITCH + BUS_BAND_GUTTER,
            lanes: picked
                .iter()
                .zip(demands)
                .zip(lanes)
                .map(|((&gi, demand), lane)| (gi, demand, lane))
                .collect(),
            lanes_used,
        };
    }

    plan.groups = groups;
    plan.row_cells = row_cells;
    plan.row_index = row_index;
    plan.row_top = row_top;
    plan
}

/// Lay the planned lanes out as rerouters and wires, and record the module
/// wires they replace.
pub(super) fn lay_bus(body: &Body<'_>, plan: &mut BusPlan) -> (BusLayout, usize) {
    let Body {
        placements,
        page_infos,
        ..
    } = *body;
    let mut bus = BusLayout::default();
    let mut laid = 0usize;
    let mut lane_cells: LaneCells = HashMap::default();
    let pages: Vec<Vec<(usize, BusDemand, usize)>> = plan
        .pages
        .iter_mut()
        .map(|p| std::mem::take(&mut p.lanes))
        .collect();
    for (pi, lanes) in pages.iter().enumerate() {
        if page_infos.get(pi).is_none() {
            continue;
        }
        // A lane whose source and taps span less than `MIN_LANE_TRAVEL` is
        // already a short hop and keeps its direct wires. Measured here, not
        // in `plan_bus`, because edge pins have no placement until
        // `place_edge_pins` runs. A skipped lane lays and suppresses nothing.
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

        // Lanes tapping one row each get their own level, a rerouter apart,
        // so their runs are not drawn over each other. The whole stop moves
        // (tap, link and gate-side rerouters all derive from `stop_x`), which
        // keeps the run horizontal.
        //
        // Choosing a level and claiming the lane's column cell are one step.
        // Levels go upward from the stop's row and never past the row's top
        // edge. `emit_lane` places a tap unconditionally, so a stop with no
        // free cell is DROPPED here; its consumers keep their direct wires.
        let mut used: HashMap<usize, Vec<i32>> = HashMap::default();
        for ((_, _, lane), stops_for_lane) in lanes.iter().zip(stops.iter_mut()) {
            // (original x, the x it was given) of the stop above, so a stop
            // `lane_stops` SHARED between rows keeps sharing the one brick
            // after the shift and does not claim the cell twice.
            let mut prev: Option<(i32, i32)> = None;
            stops_for_lane.retain_mut(|(row, x)| {
                let base = *x;
                if let Some((prev_base, prev_final)) = prev
                    && prev_base == base
                {
                    *x = prev_final;
                    used.entry(*row).or_default().push(prev_final);
                    return true;
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
                // Overflow: every level on the row already carries a run (a
                // hot row has more taps than levels). Take the LEAST loaded
                // level so the overlap spreads across the row's band; the
                // candidates stay within the row and the column claim decides.
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
                body,
                &plan.groups[*gi],
                demand,
                stops,
                *lane,
                plan.pages[pi].lanes_used,
                pi,
                &mut plan.row_cells,
                &mut lane_cells,
            );
        }
    }
    (bus, laid)
}

/// Route a line's expression values DOWN into the statement gate consuming
/// them: the gutter bus in miniature, inside one line's own block.
///
/// `line_groups` drops a statement's exec gate below the expression columns
/// feeding it, which makes each operand wire a down-and-left diagonal. One
/// rerouter where the operand's COLUMN meets the sink's ROW turns it into a
/// straight drop and a straight run left; the sink's row holds only the sink,
/// so the run crosses no gate.
///
/// The rerouter takes one inbound wire and feeds one port, and the replaced
/// module wire is suppressed so emit draws one path. Consumers the gutter
/// already suppressed are skipped (a second path would be fan-in).
pub(super) fn lay_mini_bus(
    bus: &mut BusLayout,
    body: &Body<'_>,
    node_line: &HashMap<NodeId, usize>,
    plan: &mut BusPlan,
) {
    let Body {
        module,
        placements,
        rotations,
        node_row,
        page_infos,
        owners: subtree,
    } = *body;
    // Levels already handed out on each sink's row, so two corners dropping
    // into one statement do not leave at the same height.
    let mut sink_levels: HashMap<NodeId, Vec<i32>> = HashMap::default();
    for w in &module.wires {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        if w.source.node_id == w.target.node_id {
            continue;
        }
        // The gutter owns this consumer already.
        if bus.suppressed.contains(&(w.target.node_id, w.target.port)) {
            continue;
        }
        if !subtree.is_bussable(module, w.source.node_id)
            || !subtree.is_bussable(module, w.target.node_id)
        {
            continue;
        }
        // Only the sink that MOVED, and only from its own line's block.
        if rotation_of(rotations, w.target.node_id) != NodeRotation::Deg90 {
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
        // Strictly right of the spine: an operand, not the preceding
        // statement.
        if src_at.y <= sink_at.y {
            continue;
        }
        let Some(&row) = plan.row_index.get(&(sink_page, sink_down)) else {
            continue;
        };
        let Some(info) = page_infos.get(sink_page) else {
            continue;
        };

        // Corners into one sink each take their own level, a rerouter apart,
        // within the sink's extent so the run still arrives at its gate. Cells
        // are checked against the row's gates and gutter taps; a corner that
        // fits nowhere keeps its direct wire.
        let Some(sink) = module.nodes.get(&w.target.node_id) else {
            continue;
        };
        let sink_top =
            sink_at.x + cell_half_size(sink, rotation_of(rotations, w.target.node_id)).0 * 2;
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
        // Overflow: when every level is taken, share the LEAST loaded one
        // whose cell is clear.
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

        // Faces LEFT: the corner stands right of the statement and a rerouter
        // faces what it feeds (a gutter tap faces the other way).
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
