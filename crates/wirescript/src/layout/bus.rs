//! Gutter rerouter bus.
//!
//! A value with many consumers is carried up a lane of chained rerouter
//! bricks in the gutter, and each consumer taps the lane at its own row,
//! instead of one long diagonal wire per consumer. The lane is described
//! here in the same coordinate space as the module's [`Placement`]s
//! (`x`/`y` are the brick's min corner); emit turns it into bricks and
//! wires, and drops the module wires the lane replaced.
//!
//! [`Placement`]: crate::emit::Placement

use super::NodeRotation;
use crate::collections::HashSet;
use crate::ir::port_registry::WirePort;
use crate::ir::{NodeId, PortRef};

/// Index into `BusLayout.nodes`.
pub type BusNodeId = usize;

/// Which rerouter structure a node belongs to.
///
/// All of them live in one [`BusLayout`] because they obey the same rules —
/// one inbound wire per rerouter, `Deg0` leaves feeding ports, `Deg90`
/// reserved for chaining — and emit materialises them identically.
///
/// What separates them is SCOPE, and with it shape. A [`Gutter`] node belongs
/// to a lane running the height of a whole page in the band left of the body:
/// it is one brick of a chain, it shares a column with the rest of that chain,
/// and it can be walked from a head down to a consumer. A [`Line`] node
/// belongs to no chain at all — it is a single brick inside one line's own
/// block, turning that line's operand wire down into the statement below it.
/// A [`Source`] node belongs to no chain either: it stands beside a PRODUCER
/// inside the body and hands that producer's value to the head of the lane
/// carrying it, so it is where a lane begins rather than a link of one.
///
/// So every claim about lanes — a shared column, a chain to walk, a band clear
/// of the body — is false of a mini-bus corner and of a source-side rerouter
/// by construction, and an assertion about one has to be able to say it does
/// not mean the others.
///
/// [`Gutter`]: BusRole::Gutter
/// [`Line`]: BusRole::Line
/// [`Source`]: BusRole::Source
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BusRole {
    /// A gutter lane brick: a `Deg90` link chaining down a lane's column, a
    /// `Deg0` tap branching off one, or a `Deg0` gate-side rerouter marching
    /// along a row toward its consumer.
    Gutter,
    /// A mini-bus corner: the single `Deg0` leaf that turns one line's own
    /// operand wire down into the statement gate sitting below it.
    Line,
    /// The producer-side mirror of a gate-side rerouter: the single `Deg180`
    /// brick standing beside a producing gate, which that gate wires into and
    /// which hands the value leftward to its lane's head.
    Source,
}

/// A synthesized rerouter brick with no IR node of its own.
///
/// A lane carries no text: it is identified in-game by the colour it mirrors
/// off its value's producer, so there is no name field to set.
#[derive(Clone, Debug)]
pub struct BusNode {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub rotation: NodeRotation,
    pub role: BusRole,
    /// Mirror this module node's brick colour, so a lane reads as the
    /// value it carries. `None` uses the default rerouter colour.
    pub color_of: Option<NodeId>,
}

/// One end of a bus wire: either a synthesized rerouter or a real port.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BusEnd {
    Bus(BusNodeId),
    Node(PortRef),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BusWire {
    pub source: BusEnd,
    pub target: BusEnd,
}

/// Empty outside `LayoutMode::Code`.
#[derive(Clone, Debug, Default)]
pub struct BusLayout {
    pub nodes: Vec<BusNode>,
    pub wires: Vec<BusWire>,
    /// `(target node, target port)` of every module wire the bus replaced.
    /// Emit must not draw these. The tuple is unique because fan-in is
    /// illegal, so it identifies exactly one wire.
    pub suppressed: HashSet<(NodeId, WirePort)>,
}

impl BusLayout {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.wires.is_empty() && self.suppressed.is_empty()
    }
}

/// One value's demand on the bus: the rows that consume it, ascending.
/// `rows` is never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusDemand {
    /// The port the value leaves its producer by.
    pub source: PortRef,
    /// Consuming rows, ascending, deduplicated.
    pub rows: Vec<usize>,
    /// Total consumer count across those rows (a row may consume twice).
    pub consumers: usize,
    /// Tiebreak for deterministic ordering.
    pub source_order: usize,
}

/// Lane index assigned to each demand, parallel to the input slice.
/// Lane 0 is leftmost.
///
/// # Panics
///
/// Every [`BusDemand`] must carry at least one row. Both halves below read
/// `rows` the same way, so an empty demand fails here rather than being
/// silently scored as a zero-span lane and then panicking on the same field
/// during placement.
pub fn allocate_lanes(demands: &[BusDemand]) -> Vec<usize> {
    // (first row, last row) per demand, read once so the ranking and the
    // packing below cannot disagree about what a demand spans.
    let ends: Vec<(usize, usize)> = demands
        .iter()
        .map(|d| {
            (
                *d.rows.first().expect("BusDemand.rows is never empty"),
                *d.rows.last().expect("BusDemand.rows is never empty"),
            )
        })
        .collect();

    // Sort demand indices by (row span descending, consumers descending,
    // source_order ascending). This ordering is stable and touches no
    // hash-based collections, so it is deterministic across runs.
    let mut order: Vec<usize> = (0..demands.len()).collect();
    order.sort_by(|&a, &b| {
        let span_a = ends[a].1 - ends[a].0;
        let span_b = ends[b].1 - ends[b].0;
        span_b
            .cmp(&span_a)
            .then(demands[b].consumers.cmp(&demands[a].consumers))
            .then(demands[a].source_order.cmp(&demands[b].source_order))
    });

    // Each lane holds the last row occupied by its current tenant. A demand
    // may reuse a lane only if that lane's range ends strictly before this
    // demand's first row: touching ranges (occupant ends on the same row
    // this demand starts) must NOT share a lane, since both need a
    // rerouter tap on that row.
    let mut lane_ends: Vec<usize> = Vec::new();
    let mut lanes = vec![0usize; demands.len()];

    for &idx in &order {
        let (first, last) = ends[idx];

        let mut chosen = None;
        for (lane, end) in lane_ends.iter().enumerate() {
            if *end < first {
                chosen = Some(lane);
                break;
            }
        }

        match chosen {
            Some(lane) => {
                lane_ends[lane] = last;
                lanes[idx] = lane;
            }
            None => {
                lane_ends.push(last);
                lanes[idx] = lane_ends.len() - 1;
            }
        }
    }

    lanes
}

#[cfg(test)]
mod tests;
