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
    let root = WallSlot {
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
mod tests {
    use super::*;

    // Build a lowered module the same way layout/compose.rs's test helper does.
    fn lowered(src: &str) -> crate::ir::Module {
        let parsed = crate::parser::parse(src, "test");
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let tc = crate::typecheck::typecheck(&parsed.ast, "test");
        let r = crate::lower::lower(crate::lower::LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
        });
        r.module
    }

    /// Lay a module out in `@layout("code")` mode, where a brick's row is its
    /// source line.
    fn code_layout(module: &crate::ir::Module) -> LayoutResult {
        crate::layout::layout_with_opts(
            module,
            &crate::layout::LayoutOptions {
                mode: crate::layout::LayoutMode::Code,
                ..Default::default()
            },
        )
    }

    const SRC: &str = "\
in tick: exec\n\
chip A(t: exec) { on t { } }\n\
chip B(t: exec) { on t { } }\n\
let a = A(tick)\n\
let b = B(tick)\n\
chip { chip { var inner: int = 0 } var outer: int = 0 }\n";

    /// Depth grows RIGHTWARD: each nesting level is a column stepping past
    /// the widest plane of the level before it, and planes inside a column are
    /// spread vertically instead.
    ///
    /// Replaces `rows_stack_upward_without_overlap`, which pinned the opposite
    /// arrangement — depth stacked in world Z with siblings spread along Y.
    /// Every claim it made survives here with the two axes swapped; the
    /// no-overlap and child-clear-of-parent guarantees are unchanged.
    #[test]
    fn columns_grow_rightward_without_overlap() {
        let module = lowered(SRC);
        let lr = crate::layout::layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        // Root: bottom edge just above the chip brick's top face. Unchanged —
        // the root's own placement is not part of the transposition.
        let chip_top = 6.0 + 2.0; // pos.z + half height
        let root_bottom = wall.root.location.z - wall.root.extent.x as f32;
        assert_eq!(root_bottom, chip_top + WALL_ROOT_CLEARANCE);

        // Depth-1 planes (A, B, anon) share a column left edge past the
        // root's right edge.
        let root_right = wall.root.location.y + wall.root.extent.y as f32;
        let depth1: Vec<&WallSlot> = module
            .chips
            .keys()
            .map(|id| wall.chips.get(id).expect("slot per root chip"))
            .collect();
        assert_eq!(depth1.len(), 3);
        for s in &depth1 {
            let left = s.location.y - s.extent.y as f32;
            assert_eq!(left, root_right + WALL_GUTTER_X as f32, "one column edge");
        }

        // Inside the column the planes spread along world Z with vertical
        // half-span extent.x: no overlap, and nothing closer than the gutter.
        let mut zs: Vec<(f32, f32)> = depth1
            .iter()
            .map(|s| {
                (
                    s.location.z - s.extent.x as f32,
                    s.location.z + s.extent.x as f32,
                )
            })
            .collect();
        zs.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
        for w in zs.windows(2) {
            assert!(
                w[1].0 - w[0].1 >= WALL_GUTTER_Z,
                "planes in a column must clear each other by the gutter, got {}",
                w[1].0 - w[0].1
            );
        }
        assert!(
            zs.windows(2).any(|w| w[1].0 - w[0].1 == WALL_GUTTER_Z),
            "fixture must actually pack a colliding pair, or the gap claim is              vacuous: {zs:?}"
        );

        // The nested chip sits a full column right of depth 1.
        let (_, anon_child_module) = module
            .chips
            .iter()
            .find(|(_, m)| !m.chips.is_empty())
            .expect("anon chip with a nested chip");
        let nested_id = *anon_child_module.chips.keys().next().unwrap();
        let nested = wall.chips.get(&nested_id).expect("depth-2 slot");
        let depth1_right = depth1
            .iter()
            .map(|s| s.location.y + s.extent.y as f32)
            .fold(f32::MIN, f32::max);
        assert_eq!(
            nested.location.y - nested.extent.y as f32,
            depth1_right + WALL_GUTTER_X as f32
        );

        // Every plane sits in the wall's plane (shares the chip brick's X).
        assert!(wall.chips.values().all(|s| s.location.x == 0.0));
    }

    /// `pack_row` in isolation: planes that all want the same spot end up
    /// exactly `WALL_GUTTER_Z` apart and centred on that spot, and planes that
    /// already clear each other keep their anchors untouched.
    ///
    /// The packer reads `half`, never `extent`, so these numbers describe it
    /// on whichever axis it is asked to spread. It is used on the VERTICAL one
    /// — planes stacking up a depth column — so the gap is `WALL_GUTTER_Z`.
    #[test]
    fn pack_row_packs_tight_and_leaves_clear_planes_alone() {
        let m = crate::ir::Module::new("t");
        let l = LayoutResult::default();
        let plane = |anchor: f32, half: i32| RowPlane {
            id: NodeId(0),
            extent: IntVector {
                x: half,
                y: 10,
                z: Z_PLANE,
            },
            anchor,
            half: half as f32,
            reading: (std::cmp::Reverse(0), 0, 0, 0),
            module: &m,
            lr: &l,
        };

        // Three 20-tall planes on one spot: 20 of span plus a 20 gutter each.
        let stacked = [plane(0.0, 10), plane(0.0, 10), plane(0.0, 10)];
        assert_eq!(pack_row(&stacked), vec![-40.0, 0.0, 40.0]);

        let clear = [plane(-100.0, 10), plane(0.0, 10), plane(100.0, 10)];
        assert_eq!(pack_row(&clear), vec![-100.0, 0.0, 100.0]);

        // A pair that only just collides slides apart around its midpoint.
        let nudged = [plane(-5.0, 10), plane(5.0, 10)];
        assert_eq!(pack_row(&nudged), vec![-20.0, 20.0]);
    }

    #[test]
    fn chip_planes_follow_their_code_position() {
        // Three sibling chips declared top to bottom; their planes must appear
        // in the same order along the wall's horizontal axis, not in id order.
        let src = "in go: exec\n\
chip A(t: exec) { on t { } }\n\
chip B(t: exec) { on t { } }\n\
chip C(t: exec) { on t { } }\n\
let a = A(go)\n\
let b = B(go)\n\
let c = C(go)\n";
        let module = lowered(src);
        let lr = crate::layout::layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        // Order chips by their brick position in the parent plane (code order),
        // then assert the wall slots preserve that order.
        let mut chips: Vec<(NodeId, i32)> = module
            .chips
            .keys()
            .map(|id| (*id, lr.placements[id].x))
            .collect();
        chips.sort_by_key(|(_, x)| std::cmp::Reverse(*x)); // top of the plane first
        // Siblings share a depth column, so the axis that carries their order
        // is the VERTICAL one: a brick higher in the parent opens a plane
        // higher on the wall. Reading world Y here would prove nothing — every
        // plane in a column is flush against the same left edge, so that
        // sequence is sorted by plane WIDTH, not by code order.
        let slot_axis: Vec<f32> = chips
            .iter()
            .map(|(id, _)| wall.chips[id].location.z)
            .collect();
        assert!(
            slot_axis.windows(2).all(|w| w[0] >= w[1]),
            "wall slots must follow code order top-down, got {slot_axis:?}"
        );
    }

    /// Same contract as [`chip_planes_follow_their_code_position`] under
    /// `@layout("code")`, where a chip brick's row IS its source line: the
    /// planes must read left to right in source order.
    #[test]
    fn chip_planes_follow_their_code_position_in_code_layout() {
        let src = "in go: exec\n\
chip A(t: exec) { on t { } }\n\
chip B(t: exec) { on t { } }\n\
chip C(t: exec) { on t { } }\n\
let a = A(go)\n\
let b = B(go)\n\
let c = C(go)\n";
        let module = lowered(src);
        let lr = code_layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        let mut chips: Vec<(NodeId, i32)> = module
            .chips
            .keys()
            .map(|id| (*id, lr.placements[id].x))
            .collect();
        chips.sort_by_key(|(_, x)| std::cmp::Reverse(*x)); // top of the plane first
        // Siblings share a depth column, so the axis that carries their order
        // is the VERTICAL one: a brick higher in the parent opens a plane
        // higher on the wall. Reading world Y here would prove nothing — every
        // plane in a column is flush against the same left edge, so that
        // sequence is sorted by plane WIDTH, not by code order.
        let slot_axis: Vec<f32> = chips
            .iter()
            .map(|(id, _)| wall.chips[id].location.z)
            .collect();
        assert!(
            slot_axis.windows(2).all(|w| w[0] >= w[1]),
            "wall slots must follow code order top-down, got {slot_axis:?}"
        );
    }

    /// A nested plane anchors on ITS OWN parent's plane — the parent's slot
    /// plus the chip brick's offset inside it — not on the wall's centre. The
    /// lone depth-2 plane here has nothing to collide with, so it lands
    /// exactly on its brick.
    ///
    /// The anchored axis is now the VERTICAL one: depth is carried by the
    /// column step, which frees world Z to track the brick's own row (local +X
    /// → world up). Previously this asserted the same relationship on world Y
    /// against the brick's local Y — the claim is unchanged, the axes swapped
    /// with the arrangement.
    #[test]
    fn nested_planes_anchor_on_their_parents_plane() {
        let src = "in go: exec\n\
chip A(t: exec) { on t { PrintToConsole(\"a\") } }\n\
chip B(t: exec) {\n\
  chip Inner(u: exec) { on u { } }\n\
  let i = Inner(t)\n\
}\n\
let a = A(go)\n\
let b = B(go)\n";
        let module = lowered(src);
        let lr = code_layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        let (b_id, b_module) = module
            .chips
            .iter()
            .find(|(_, m)| !m.chips.is_empty())
            .expect("root chip with a nested chip");
        let b_slot = wall.chips[b_id];
        let inner_id = *b_module.chips.keys().next().expect("nested chip");
        let brick_local = lr.chip_layouts[b_id].placements[&inner_id].x;
        assert_ne!(
            brick_local, 0,
            "guard: the brick must be off its plane's centre for this to bite"
        );

        let brick_z = b_slot.location.z + brick_local as f32;
        let slot = wall.chips[&inner_id];
        assert!(
            (slot.location.z - brick_z).abs() < 0.001,
            "nested plane at z={} should open level with its brick at {brick_z}",
            slot.location.z
        );
    }

    /// Depth grows RIGHTWARD, and a plane's height follows its own brick.
    ///
    /// Stacking each nesting level above the last put a chip's interior a
    /// whole board-height away from the brick that opens it — the deeper the
    /// chip, the further the eye had to travel. Growing sideways instead keeps
    /// a plane beside its brick: the horizontal step is what separates the
    /// levels, which frees the vertical axis to track where the brick actually
    /// sits inside its parent.
    ///
    /// Both halves matter. Rightward alone would leave every plane bottom
    /// aligned; brick-tracking alone would put a child on top of its parent.
    #[test]
    fn child_planes_grow_rightward_and_track_their_bricks_height() {
        let src = "in go: exec\n\
chip A(t: exec) { on t { PrintToConsole(\"a\") } }\n\
chip B(t: exec) {\n\
  chip Inner(u: exec) { on u { PrintToConsole(\"i\") } }\n\
  let i = Inner(t)\n\
}\n\
let a = A(go)\n\
let b = B(go)\n";
        let module = lowered(src);
        let lr = code_layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        // (a) Every depth-1 plane is strictly right of the root plane.
        let root_right = wall.root.location.y + wall.root.extent.y as f32;
        for id in module.chips.keys() {
            let s = wall.chips[id];
            assert!(
                s.location.y - s.extent.y as f32 >= root_right,
                "a child plane must sit RIGHT of its parent: child's left edge \
                 is {}, root's right edge is {root_right}",
                s.location.y - s.extent.y as f32
            );
        }

        // ...and the same one level deeper, against its own parent.
        let (b_id, b_module) = module
            .chips
            .iter()
            .find(|(_, m)| !m.chips.is_empty())
            .expect("root chip with a nested chip");
        let b_slot = wall.chips[b_id];
        let inner_id = *b_module.chips.keys().next().expect("nested chip");
        let inner = wall.chips[&inner_id];
        assert!(
            inner.location.y - inner.extent.y as f32
                >= b_slot.location.y + b_slot.extent.y as f32,
            "a depth-2 plane must sit right of its depth-1 parent"
        );

        // (b) The nested plane's HEIGHT tracks its brick inside the parent
        // plane: local +X is world up, so the brick's own row is where its
        // interior opens.
        let brick_x = lr.chip_layouts[b_id].placements[&inner_id].x as f32;
        let want_z = b_slot.location.z + brick_x;
        assert!(
            (inner.location.z - want_z).abs() < 0.001,
            "the nested plane sits at z={}, but its brick opens at z={want_z}",
            inner.location.z
        );
    }

    #[test]
    fn chip_planes_do_not_overlap_each_other() {
        let src = "in go: exec\n\
chip A(t: exec) { on t { PrintToConsole(\"a\") } }\n\
chip B(t: exec) { on t { PrintToConsole(\"b\") } }\n\
let a = A(go)\n\
let b = B(go)\n\
chip { var inner: int = 0 }\n";
        let module = lowered(src);
        let lr = crate::layout::layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));
        let slots: Vec<&WallSlot> = wall.chips.values().collect();
        for i in 0..slots.len() {
            for j in (i + 1)..slots.len() {
                let (a, b) = (slots[i], slots[j]);
                let sep_y = (a.location.y - b.location.y).abs()
                    >= (a.extent.y + b.extent.y) as f32;
                let sep_z = (a.location.z - b.location.z).abs()
                    >= (a.extent.x + b.extent.x) as f32;
                assert!(sep_y || sep_z, "planes overlap: {a:?} vs {b:?}");
            }
        }
    }

    /// The anchored placement must keep the no-overlap guarantee under the
    /// code layout too, including across nesting depths.
    #[test]
    fn chip_planes_do_not_overlap_in_code_layout() {
        let src = "in go: exec\n\
chip A(t: exec) { on t { PrintToConsole(\"a\") } }\n\
chip B(t: exec) {\n\
  chip Inner(u: exec) { on u { PrintToConsole(\"i\") } }\n\
  let i = Inner(t)\n\
}\n\
let a = A(go)\n\
let b = B(go)\n\
chip { var inner: int = 0 }\n";
        let module = lowered(src);
        let lr = code_layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        let mut slots: Vec<WallSlot> = wall.chips.values().copied().collect();
        slots.push(WallSlot { ..wall.root });
        assert_eq!(slots.len(), 5, "root + 4 chip planes");
        for i in 0..slots.len() {
            for j in (i + 1)..slots.len() {
                let (a, b) = (&slots[i], &slots[j]);
                let sep_y = (a.location.y - b.location.y).abs() >= (a.extent.y + b.extent.y) as f32;
                let sep_z = (a.location.z - b.location.z).abs() >= (a.extent.x + b.extent.x) as f32;
                assert!(sep_y || sep_z, "planes overlap: {a:?} vs {b:?}");
            }
        }
    }

    /// Single-page layouts (every Dag layout, and an empty module whose
    /// bounds are all zero) must keep the historical `Z_PLANE` half-extent
    /// on z — the paginated-layout z coverage must not disturb them.
    #[test]
    fn single_page_layouts_keep_the_historical_z_extent() {
        let module = lowered(SRC);
        let lr = crate::layout::layout(&module);
        assert_eq!(lr.bounds_min.z, crate::layout::Z_PLANE);
        assert_eq!(lr.bounds_max.z, crate::layout::Z_PLANE);
        assert_eq!(plane_extent(&lr).z, crate::layout::Z_PLANE, "Dag root plane");
        for clr in lr.chip_layouts.values() {
            assert_eq!(plane_extent(clr).z, crate::layout::Z_PLANE, "Dag chip plane");
        }

        let empty = crate::layout::layout(&crate::ir::Module::new("empty"));
        assert_eq!(
            plane_extent(&empty).z,
            crate::layout::Z_PLANE,
            "an empty module's plane keeps a non-degenerate z half-extent"
        );

        // The wall slots carry those extents through unchanged.
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));
        assert_eq!(wall.root.extent.z, crate::layout::Z_PLANE);
        assert!(wall.chips.values().all(|s| s.extent.z == crate::layout::Z_PLANE));
    }
}

