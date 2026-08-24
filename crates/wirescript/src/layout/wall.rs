//! Wall layout: every chip grid (root + nested) becomes an upright plane. The
//! root sits above the deployment chip brick, and each nesting level occupies
//! a COLUMN to the right of the one holding its chip brick — depth grows
//! sideways, not upward.
//!
//! Under the in-game-pinned `emit::WALL_ROT` (MEASURED mapping): grid-local
//! +X → world up (dataflow runs bottom→top), grid-local ±Y → world horizontal
//! (along world Y), and the board front faces the chip's @bottom-rerouter
//! side. A pane's vertical half-span is therefore `extent.x` and its
//! horizontal half-span `extent.y`. That mapping is measured, not derived —
//! do not re-derive it.
//!
//! Growing sideways is what frees the vertical axis: a plane anchors its
//! HEIGHT on its own chip brick, mapping the brick's position in the parent
//! plane through the same mapping (local +X → world +Z). A chip's interior
//! therefore opens level with the brick that opens it, however deep it is,
//! instead of a whole board-height above. The column step is what keeps a
//! child clear of its parent, and planes inside one column that would collide
//! pack apart vertically.

use crate::collections::HashMap;

use brdb::{IntVector, Vector3f};

use super::{LayoutResult, Z_PLANE};
use crate::ir::{Module, NodeId};

/// Horizontal gap between one depth column's right edge and the next
/// column's left edge (grid units / cm).
pub const WALL_GUTTER_X: i32 = 10;
/// Vertical gap between adjacent planes packed within one depth column (cm).
pub const WALL_GUTTER_Z: f32 = 20.0;
/// Gap between the chip brick's top face and the root plane's bottom edge
/// (cm). Pinned during in-game verification.
pub const WALL_ROOT_CLEARANCE: f32 = 10.0;
/// `B_1x1_Microchip` half-height (halfSize z = 2).
const CHIP_BRICK_HALF_HEIGHT: f32 = 2.0;

/// One plane's assigned transform. `location` is the grid entity's location:
/// centre-anchored on X/Y, bottom-edge anchored on Z
/// (`location.z - extent.x` = the row baseline; vertical half-span is
/// `extent.x` under the measured `WALL_ROT` mapping).
#[derive(Clone, Copy, Debug)]
pub struct WallSlot {
    pub location: Vector3f,
    /// Plane half-extent in grid units (`PlaneExtent`).
    pub extent: IntVector,
}

pub struct WallLayout {
    pub root: WallSlot,
    pub chips: HashMap<NodeId, WallSlot>,
}

/// A module's plane half-extent from its layout bounds: half-span per axis
/// plus a 5-unit margin, minimum 5 (matches the historical emit formula).
///
/// Unlike x/y, the z axis is not centered around `PlaneCenter` (always
/// `(0, 0, 0)`, see `emit::build_world`) — every gate sits at `Z_PLANE` or
/// higher (paginated code layouts stack pages at `Z_PLANE + p *
/// PAGE_Z_STEP`; the grid-mode fallback stacks layers too), never below it.
/// Covering that range from a fixed center needs the half-extent to reach
/// the farthest z used, not half the bounds' span — so z uses the larger
/// bound directly rather than `(max - min) / 2`, floored at `Z_PLANE` so a
/// single-page layout (`bounds_min.z == bounds_max.z == Z_PLANE`) and an
/// empty one (all-zero bounds) both keep the historical value.
pub fn plane_extent(lr: &LayoutResult) -> IntVector {
    let half_x = (lr.bounds_max.x - lr.bounds_min.x) / 2;
    let half_y = (lr.bounds_max.y - lr.bounds_min.y) / 2;
    let half_z = lr.bounds_max.z.max(lr.bounds_min.z).max(Z_PLANE);
    IntVector {
        x: (half_x + 5).max(5),
        y: (half_y + 5).max(5),
        z: half_z,
    }
}

/// One plane waiting for a slot in the depth column currently being packed.
struct RowPlane<'a> {
    id: NodeId,
    extent: IntVector,
    /// World Z the plane's centre wants: the chip brick's own world Z, i.e.
    /// the parent plane's centre plus the brick's local X (local +X → world
    /// up under the measured `WALL_ROT` mapping).
    anchor: f32,
    /// Half-span along the axis being packed — the VERTICAL one, so
    /// `extent.x`. Carried rather than read off `extent` inside the packer so
    /// the packer itself is axis-agnostic and there is only one of it.
    half: f32,
    /// Reading order of the chip brick inside the parent plane — top row
    /// first (local +X is up), then left to right, then source order. Breaks
    /// ties between planes that want the same world Z.
    reading: (std::cmp::Reverse<i32>, i32, usize, u32),
    module: &'a Module,
    lr: &'a LayoutResult,
}

/// Assign every chip grid a wall slot. COLUMNS by nesting depth (root = column
/// 0 on the left), each column stepping right past the widest plane in the one
/// before it, so a plane always sits clear of — and to the right of — the
/// plane holding its chip brick.
///
/// Within a column each plane wants to be centred on its own chip brick's
/// world Z, which is what puts a chip's interior level with the brick that
/// opens it. Planes that would overlap merge into a block packed with
/// `WALL_GUTTER_Z` gaps and re-centred on its members' anchors, which spreads
/// them apart without reordering. Every plane shares the chip brick's X.
/// Closed chips get slots too — opening one in-game reveals it in place.
pub fn assign_wall_slots(
    module: &Module,
    lr: &LayoutResult,
    chip_pos: (i32, i32, i32),
) -> WallLayout {
    let (cx, cy, cz) = chip_pos;
    // `plane_extent` derives z from the layout bounds and floors it at
    // `Z_PLANE`, so it already yields the root plane's historical z for
    // single-page layouts while covering a paginated root's full z span.
    let root_extent = plane_extent(lr);

    // The root sits directly above the chip brick, unchanged.
    let mut root = WallSlot {
        location: Vector3f {
            x: cx as f32,
            y: cy as f32,
            z: cz as f32 + CHIP_BRICK_HALF_HEIGHT + WALL_ROOT_CLEARANCE + root_extent.x as f32,
        },
        extent: root_extent,
    };
    // Left edge of the depth column being filled. Column 0 is the root, so
    // depth 1 starts past the root's right edge.
    let mut column_left = cy as f32 + root_extent.y as f32 + WALL_GUTTER_X as f32;

    let mut chips = HashMap::default();
    // Breadth-first over nesting depth. Each frontier entry carries the
    // world Z its plane was given, which is what its own children anchor on.
    let mut frontier: Vec<(f32, &Module, &LayoutResult)> =
        vec![(root.location.z, module, lr)];
    loop {
        let mut row: Vec<RowPlane> = Vec::new();
        for (parent_z, m, mlr) in &frontier {
            for (chip_id, child_module) in &m.chips {
                let Some(clr) = mlr.chip_layouts.get(chip_id) else {
                    continue;
                };
                // Where the chip brick sits in the parent plane. A chip
                // without a placement falls back to the parent's centre.
                let brick = mlr
                    .placements
                    .get(chip_id)
                    .map(|p| (p.x, p.y))
                    .unwrap_or((0, 0));
                let src_off = m
                    .nodes
                    .get(chip_id)
                    .map(|n| n.source_range.start.offset)
                    .unwrap_or(usize::MAX);
                let extent = plane_extent(clr);
                row.push(RowPlane {
                    id: *chip_id,
                    extent,
                    // local +X → world up: the brick's own row in the parent.
                    anchor: parent_z + brick.0 as f32,
                    half: extent.x as f32,
                    reading: (std::cmp::Reverse(brick.0), brick.1, src_off, chip_id.0),
                    module: child_module,
                    lr: clr,
                });
            }
        }
        if row.is_empty() {
            break;
        }

        row.sort_by(|a, b| {
            a.anchor
                .total_cmp(&b.anchor)
                .then(a.reading.cmp(&b.reading))
        });
        let centres = pack_row(&row);
        // The column is as wide as its widest plane; every plane is flush
        // against the column's left edge, so no plane can reach past it into
        // the next column.
        let column_half_w = row.iter().map(|p| p.extent.y).max().unwrap_or(0);
        for (plane, z) in row.iter().zip(&centres) {
            chips.insert(
                plane.id,
                WallSlot {
                    location: Vector3f {
                        x: cx as f32,
                        y: column_left + plane.extent.y as f32,
                        z: *z,
                    },
                    extent: plane.extent,
                },
            );
        }
        column_left += 2.0 * column_half_w as f32 + WALL_GUTTER_X as f32;
        frontier = row
            .iter()
            .zip(centres)
            .map(|(plane, z)| (z, plane.module, plane.lr))
            .collect();
    }

    // Lift the whole assembly so no plane's bottom edge sits below the
    // deployment brick's top (the paste anchor / ground). A nested plane
    // anchors on its chip brick's world Z, so a brick low in its parent can
    // push a tall child plane underground (measured: a child plane bottom at
    // z = -8 while the brick sits at z = 0). Shifting every plane up by the
    // deficit keeps their relative anchoring intact (the bricks a child
    // anchors on live in these same planes and shift with them) while raising
    // the boards clear of the ground. The deployment brick stays at `chip_pos`.
    let floor = cz as f32 + CHIP_BRICK_HALF_HEIGHT;
    let min_bottom = std::iter::once(&root)
        .chain(chips.values())
        .map(|s| s.location.z - s.extent.x as f32)
        .fold(f32::INFINITY, f32::min);
    let lift = (floor - min_bottom).max(0.0);
    if lift > 0.0 {
        root.location.z += lift;
        for slot in chips.values_mut() {
            slot.location.z += lift;
        }
    }

    WallLayout { root, chips }
}

/// Spread one depth column's planes along the packing axis, returning each
/// plane's centre in the order given. `planes` must already be in the intended
/// ascending order along that axis; that order is preserved exactly.
///
/// Axis-agnostic: it reads each plane's `anchor` and its `half` span and
/// never touches `extent`, so the same packer serves whichever axis the caller
/// is spreading along. Today that is the vertical one — planes stack up a
/// column — and `half` is `extent.x`.
///
/// Every plane wants its `anchor`. Planes whose spans would collide form a
/// block: inside a block they sit shoulder to shoulder with `WALL_GUTTER_Z`
/// between them, and the block as a whole is centred so that its members are
/// off their anchors by as little as possible (the block's low edge is the
/// mean of the low edges its members would each need). Absorbing a plane can
/// push a block into the one before it, so blocks merge until the column is
/// collision-free.
fn pack_row(planes: &[RowPlane]) -> Vec<f32> {
    let gutter = WALL_GUTTER_Z;
    /// A run of planes packed against each other.
    struct Block {
        /// Index of the block's first plane in `planes`.
        start: usize,
        len: usize,
        /// Sum over members of (wanted low edge − offset in the block); the
        /// block's own low edge is this divided by `len`.
        want_sum: f32,
        /// Total span from the first member's low edge to the last's high.
        width: f32,
    }
    // Each plane's low edge relative to its block's low edge.
    let mut offsets = vec![0.0f32; planes.len()];
    let mut blocks: Vec<Block> = Vec::new();

    for (i, plane) in planes.iter().enumerate() {
        let half = plane.half;
        let mut block = Block {
            start: i,
            len: 1,
            want_sum: plane.anchor - half,
            width: 2.0 * half,
        };
        while let Some(prev) = blocks.last() {
            if prev.want_sum / prev.len as f32 + prev.width + gutter
                <= block.want_sum / block.len as f32
            {
                break;
            }
            // Overlap: append this block to the previous one. Its members
            // shift up past the previous block's span plus the gutter.
            let shift = prev.width + gutter;
            for off in &mut offsets[block.start..block.start + block.len] {
                *off += shift;
            }
            let prev = blocks.pop().expect("checked non-empty above");
            block = Block {
                start: prev.start,
                len: prev.len + block.len,
                want_sum: prev.want_sum + block.want_sum - shift * block.len as f32,
                width: shift + block.width,
            };
        }
        blocks.push(block);
    }

    let mut centres = vec![0.0f32; planes.len()];
    for block in &blocks {
        let low = block.want_sum / block.len as f32;
        for i in block.start..block.start + block.len {
            centres[i] = low + offsets[i] + planes[i].half;
        }
    }
    centres
}

#[cfg(test)]
mod tests;
