//! Wall layout: every chip grid (root + nested) becomes an upright plane,
//! stacked in depth rows above the deployment chip brick — root row at the
//! bottom, deeper nesting higher. Under the in-game-pinned `emit::WALL_ROT`
//! (MEASURED mapping): grid-local +X → world up (dataflow runs bottom→top),
//! grid-local ±Y → world horizontal (along world Y), and the board front
//! faces the chip's @bottom-rerouter side. A pane's vertical half-span is
//! therefore `extent.x` and its horizontal half-span `extent.y`.

use crate::collections::HashMap;

use brdb::{IntVector, Vector3f};

use super::LayoutResult;
use crate::ir::{Module, NodeId};

/// Horizontal gap between adjacent planes in a row (grid units / cm).
pub const WALL_GUTTER_X: i32 = 10;
/// Vertical gap between a row's top edge and the next row's bottom edge (cm).
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
pub fn plane_extent(lr: &LayoutResult) -> IntVector {
    let half_x = (lr.bounds_max.x - lr.bounds_min.x) / 2;
    let half_y = (lr.bounds_max.y - lr.bounds_min.y) / 2;
    IntVector {
        x: (half_x + 5).max(5),
        y: (half_y + 5).max(5),
        z: 0,
    }
}

/// Assign every chip grid a wall slot. Rows by nesting depth (root = row 0 at
/// the bottom). Within a row, chips sort by (parent's index in its own row,
/// source offset) so siblings sit together beneath their parent's area, then
/// shelf-pack along world Y with `WALL_GUTTER_X` gaps, centred on the chip
/// brick's Y; every plane shares the chip brick's X. Closed chips get slots
/// too — opening one in-game reveals it in place.
pub fn assign_wall_slots(
    module: &Module,
    lr: &LayoutResult,
    chip_pos: (i32, i32, i32),
) -> WallLayout {
    let (cx, cy, cz) = chip_pos;
    let root_extent = {
        let mut e = plane_extent(lr);
        e.z = 2; // the root plane keeps its historical z half-extent
        e
    };

    // Breadth-first rows of (chip id, extent).
    let mut rows: Vec<Vec<(NodeId, IntVector)>> = Vec::new();
    let mut frontier: Vec<(usize, &Module, &LayoutResult)> = vec![(0, module, lr)];
    loop {
        let mut children: Vec<((usize, usize, u32), NodeId, IntVector, &Module, &LayoutResult)> =
            Vec::new();
        for (parent_idx, m, mlr) in &frontier {
            for (chip_id, child_module) in &m.chips {
                let Some(clr) = mlr.chip_layouts.get(chip_id) else {
                    continue;
                };
                let src_off = m
                    .nodes
                    .get(chip_id)
                    .map(|n| n.source_range.start.offset)
                    .unwrap_or(usize::MAX);
                children.push((
                    (*parent_idx, src_off, chip_id.0),
                    *chip_id,
                    plane_extent(clr),
                    child_module,
                    clr,
                ));
            }
        }
        if children.is_empty() {
            break;
        }
        children.sort_by_key(|(key, ..)| *key);
        frontier = children
            .iter()
            .enumerate()
            .map(|(i, (_, _, _, m, clr))| (i, *m, *clr))
            .collect();
        rows.push(
            children
                .into_iter()
                .map(|(_, id, extent, _, _)| (id, extent))
                .collect(),
        );
    }

    // MEASURED WALL_ROT mapping (see emit.rs): grid-local +X → world up,
    // local ±Y → world horizontal, board front → the chip's bottom-port
    // side. A pane's vertical half-span is therefore `extent.x` and its
    // horizontal half-span `extent.y`; rows pack along world Y, centred on
    // the chip brick's Y, and every plane shares the chip's X.

    // Stack rows upward from the chip brick, shelf-packing each row.
    let mut baseline = cz as f32 + CHIP_BRICK_HALF_HEIGHT + WALL_ROOT_CLEARANCE;
    let root = WallSlot {
        location: Vector3f {
            x: cx as f32,
            y: cy as f32,
            z: baseline + root_extent.x as f32,
        },
        extent: root_extent,
    };
    baseline += 2.0 * root_extent.x as f32 + WALL_GUTTER_Z;

    let mut chips = HashMap::default();
    for row in rows {
        let total_w: i32 = row.iter().map(|(_, e)| 2 * e.y).sum::<i32>()
            + WALL_GUTTER_X * (row.len() as i32 - 1);
        let mut left = cy as f32 + total_w as f32 / 2.0;
        let row_half_h = row.iter().map(|(_, e)| e.x).max().unwrap_or(0);
        for (id, extent) in row {
            chips.insert(
                id,
                WallSlot {
                    location: Vector3f {
                        x: cx as f32,
                        y: left - extent.y as f32,
                        z: baseline + extent.x as f32,
                    },
                    extent,
                },
            );
            left -= (2 * extent.y + WALL_GUTTER_X) as f32;
        }
        baseline += 2.0 * row_half_h as f32 + WALL_GUTTER_Z;
    }

    WallLayout { root, chips }
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

    const SRC: &str = "\
in tick: exec\n\
chip A(t: exec) { on t { } }\n\
chip B(t: exec) { on t { } }\n\
let a = A(tick)\n\
let b = B(tick)\n\
chip { chip { var inner: int = 0 } var outer: int = 0 }\n";

    #[test]
    fn rows_stack_upward_without_overlap() {
        let module = lowered(SRC);
        let lr = crate::layout::layout(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        // Root: bottom edge just above the chip brick's top face. Vertical
        // half-span is extent.x under the measured WALL_ROT mapping.
        let chip_top = 6.0 + 2.0; // pos.z + half height
        let root_bottom = wall.root.location.z - wall.root.extent.x as f32;
        assert_eq!(root_bottom, chip_top + WALL_ROOT_CLEARANCE);

        // Depth-1 planes (A, B, anon) share a baseline above the root's top.
        let root_top = wall.root.location.z + wall.root.extent.x as f32;
        let depth1: Vec<&WallSlot> = module
            .chips
            .keys()
            .map(|id| wall.chips.get(id).expect("slot per root chip"))
            .collect();
        assert_eq!(depth1.len(), 3);
        for s in &depth1 {
            let bottom = s.location.z - s.extent.x as f32;
            assert_eq!(bottom, root_top + WALL_GUTTER_Z, "same baseline per row");
        }

        // Rows pack along world Y with horizontal half-span extent.y: no
        // overlap within the row, gaps == WALL_GUTTER_X.
        let mut ys: Vec<(f32, f32)> = depth1
            .iter()
            .map(|s| {
                (
                    s.location.y - s.extent.y as f32,
                    s.location.y + s.extent.y as f32,
                )
            })
            .collect();
        ys.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
        for w in ys.windows(2) {
            assert_eq!(w[1].0 - w[0].1, WALL_GUTTER_X as f32);
        }

        // The nested chip sits a full row above depth 1.
        let (_, anon_child_module) = module
            .chips
            .iter()
            .find(|(_, m)| !m.chips.is_empty())
            .expect("anon chip with a nested chip");
        let nested_id = *anon_child_module.chips.keys().next().unwrap();
        let nested = wall.chips.get(&nested_id).expect("depth-2 slot");
        let depth1_top = depth1
            .iter()
            .map(|s| s.location.z + s.extent.x as f32)
            .fold(f32::MIN, f32::max);
        assert_eq!(
            nested.location.z - nested.extent.x as f32,
            depth1_top + WALL_GUTTER_Z
        );

        // Every plane sits in the wall's plane (shares the chip brick's X).
        assert!(wall.chips.values().all(|s| s.location.x == 0.0));
    }
}
