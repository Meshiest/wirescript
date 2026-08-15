//! The gutter-bus planner: replace long value wires with lanes of rerouters
//! standing beside the code body.

use super::*;

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

/// Every node standing inside a chip nested under `module`, mapped to the
/// chip node that represents it HERE.
///
/// A wire out of this module's body into a chip targets a boundary pin in
/// the CHILD module, which holds no row of its own here. The chip brick the
/// value enters through does, so that is what the wire's tap anchors on.
/// Node ids are unique to one module, so the walk order cannot change the
/// result.
pub(super) fn chip_owners(module: &Module) -> ChipSubtree {
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
pub(super) struct ChipSubtree {
    pub(super) owner: HashMap<NodeId, NodeId>,
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
pub(super) struct BusPlan {
    /// Gutter band width per page, for the input-pin stack to clear.
    pub(super) band_widths: Vec<i32>,
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
    pub(super) row_index: HashMap<(usize, i32), usize>,
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
pub(super) fn plan_bus(
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
pub(super) fn lay_bus(
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
pub(super) fn lay_mini_bus(
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
