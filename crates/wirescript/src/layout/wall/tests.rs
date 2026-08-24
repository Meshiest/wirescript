    use super::*;

    // Build a lowered module the same way layout/compose.rs's test helper does.
    fn lowered(src: &str) -> crate::ir::Module {
        let parsed = crate::parser::parse(src, "test");
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let tc = crate::typecheck::typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
        let r = crate::lower::lower(crate::lower::LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
            ce_slots: &crate::typecheck::CeSlotMap::default(),
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
    /// The anchored axis is the VERTICAL one: depth is carried by the
    /// column step, which frees world Z to track the brick's own row (local +X
    /// → world up).
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
