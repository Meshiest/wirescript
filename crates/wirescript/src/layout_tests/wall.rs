use crate::ir::NodeId;
use crate::layout::internals::*;
use crate::layout::wall::*;

// Lower `src` to its module.
fn lowered(src: &str) -> crate::ir::Module {
    let crate::compile::FrontEnd {
        resolved: parsed,
        lowered: r,
        ..
    } = crate::compile::front_end(
        src,
        "test",
        None,
        crate::lower::FoldMode::Auto,
        &crate::resolve::FsLoader,
    );
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
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
#[test]
fn columns_grow_rightward_without_overlap() {
    let module = lowered(SRC);
    let lr = crate::layout::layout(&module);
    let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

    // Root: bottom edge at least just above the chip brick's top face. The
    // whole assembly is lifted when a nested plane would otherwise sit
    // underground (this fixture nests deeply enough to trigger it), so the
    // root can be higher than the nominal clearance but never lower.
    let chip_top = 6.0 + 2.0; // pos.z + half height
    let root_bottom = wall.root.location.z - wall.root.extent.x as f32;
    assert!(root_bottom >= chip_top + WALL_ROOT_CLEARANCE);

    // No plane's bottom edge sits below the deployment brick's top face:
    // nothing is buried in the ground when pasted at ground level.
    for s in std::iter::once(&wall.root).chain(wall.chips.values()) {
        assert!(
            s.location.z - s.extent.x as f32 >= chip_top,
            "a chip plane is underground: bottom {} < floor {chip_top}",
            s.location.z - s.extent.x as f32
        );
    }

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

/// Three sibling chips declared top to bottom: their planes appear in the
/// same order down the wall, not in id order, under the DAG layout and
/// under `@layout("code")`, where a chip brick's row IS its source line.
#[test]
fn chip_planes_follow_their_code_position() {
    let src = "in go: exec\n\
chip A(t: exec) { on t { } }\n\
chip B(t: exec) { on t { } }\n\
chip C(t: exec) { on t { } }\n\
let a = A(go)\n\
let b = B(go)\n\
let c = C(go)\n";
    let module = lowered(src);
    let layouts: [(&str, fn(&crate::ir::Module) -> LayoutResult); 2] =
        [("dag", crate::layout::layout), ("code", code_layout)];
    for (mode, lay) in layouts {
        let lr = lay(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));

        let mut chips: Vec<(NodeId, i32)> = module
            .chips
            .keys()
            .map(|id| (*id, lr.placements[id].x))
            .collect();
        chips.sort_by_key(|(_, x)| std::cmp::Reverse(*x)); // top of the plane first
        // Siblings share a depth column, so their order is VERTICAL: a
        // brick higher in the parent opens a plane higher on the wall.
        // World Y proves nothing here: planes in a column share a left
        // edge, so that sequence sorts by plane width, not code order.
        let slot_axis: Vec<f32> = chips
            .iter()
            .map(|(id, _)| wall.chips[id].location.z)
            .collect();
        assert!(
            slot_axis.windows(2).all(|w| w[0] >= w[1]),
            "{mode}: wall slots must follow code order top-down, got {slot_axis:?}"
        );
    }
}

/// No two planes overlap, under the DAG layout and under the code layout's
/// anchored placement, including across nesting depths. Each row: the
/// layout, its source, and, when the root plane is checked too, how many
/// planes there are.
#[test]
fn chip_planes_do_not_overlap_each_other() {
    let rows: [(
        &str,
        fn(&crate::ir::Module) -> LayoutResult,
        &str,
        Option<usize>,
    ); 2] = [
        (
            "dag",
            crate::layout::layout,
            "in go: exec\n\
chip A(t: exec) { on t { PrintToConsole(\"a\") } }\n\
chip B(t: exec) { on t { PrintToConsole(\"b\") } }\n\
let a = A(go)\n\
let b = B(go)\n\
chip { var inner: int = 0 }\n",
            None,
        ),
        (
            "code",
            code_layout,
            "in go: exec\n\
chip A(t: exec) { on t { PrintToConsole(\"a\") } }\n\
chip B(t: exec) {\n\
  chip Inner(u: exec) { on u { PrintToConsole(\"i\") } }\n\
  let i = Inner(t)\n\
}\n\
let a = A(go)\n\
let b = B(go)\n\
chip { var inner: int = 0 }\n",
            // The root + 4 chip planes.
            Some(5),
        ),
    ];
    for (mode, lay, src, with_root) in rows {
        let module = lowered(src);
        let lr = lay(&module);
        let wall = assign_wall_slots(&module, &lr, (0, 0, 6));
        let mut slots: Vec<WallSlot> = wall.chips.values().copied().collect();
        if let Some(n) = with_root {
            slots.push(WallSlot { ..wall.root });
            assert_eq!(slots.len(), n, "{mode}: root + chip planes");
        }
        for i in 0..slots.len() {
            for j in (i + 1)..slots.len() {
                let (a, b) = (&slots[i], &slots[j]);
                let sep_y = (a.location.y - b.location.y).abs() >= (a.extent.y + b.extent.y) as f32;
                let sep_z = (a.location.z - b.location.z).abs() >= (a.extent.x + b.extent.x) as f32;
                assert!(sep_y || sep_z, "{mode}: planes overlap: {a:?} vs {b:?}");
            }
        }
    }
}

/// A nested plane anchors on its own parent's plane (the parent's slot plus
/// the chip brick's offset inside it), not on the wall's centre. The lone
/// depth-2 plane here has nothing to collide with, so it lands exactly on
/// its brick. The anchored axis is world Z, since local +X is world up.
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

/// Depth grows rightward, and a plane's height follows its own brick, so a
/// chip's interior opens beside the brick that holds it. Rightward alone
/// would bottom-align every plane; brick-tracking alone would put a child on
/// top of its parent.
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
        inner.location.y - inner.extent.y as f32 >= b_slot.location.y + b_slot.extent.y as f32,
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

/// Single-page layouts (every Dag layout, and an empty module whose bounds
/// are all zero) keep the `Z_PLANE` half-extent on z; only paginated layouts
/// widen z.
#[test]
fn single_page_layouts_keep_the_historical_z_extent() {
    let module = lowered(SRC);
    let lr = crate::layout::layout(&module);
    assert_eq!(lr.bounds_min.z, crate::layout::Z_PLANE);
    assert_eq!(lr.bounds_max.z, crate::layout::Z_PLANE);
    assert_eq!(
        plane_extent(&lr).z,
        crate::layout::Z_PLANE,
        "Dag root plane"
    );
    for clr in lr.chip_layouts.values() {
        assert_eq!(
            plane_extent(clr).z,
            crate::layout::Z_PLANE,
            "Dag chip plane"
        );
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
    assert!(
        wall.chips
            .values()
            .all(|s| s.extent.z == crate::layout::Z_PLANE)
    );
}
