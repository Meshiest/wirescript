//! `@side` port annotations place pre-wired rerouter bricks on the outer
//! grid, flush against the compiled microchip brick (spec:
//! wirescript SDK docs/superpowers/specs/2026-07-12-port-side-rerouters-design.md).

use wirescript::compile_to_world;
use wirescript::emit::EmitOptions;

use crate::common;

const REROUTER: &str = "Component_Internal_Rerouter";
const CHIP_IN: &str = "BrickComponentType_Internal_MicrochipInput";
const CHIP_OUT: &str = "BrickComponentType_Internal_MicrochipOutput";

// Declaration order: go(left), players(top), done(left), score(right).
// Left side gets in/out interleaved: go first (top end), done second.
const SRC: &str = "@left in go: exec\n\
                   @top in players: int\n\
                   @left out done: exec\n\
                   @right out score = players + 1\n\
                   on go { emit done }\n";

fn world() -> brdb::World {
    compile_to_world(common::input(SRC, "pins.ws"), EmitOptions::default())
        .expect("should compile")
        .world
}

#[test]
fn rerouter_bricks_sit_flush_per_side_in_source_order() {
    let w = world();
    // Main grid: chip brick + 4 rerouters.
    let rerouters: Vec<&brdb::Brick> = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .collect();
    assert_eq!(rerouters.len(), 4, "one rerouter per annotated port");

    let mut positions: Vec<(i32, i32, i32)> = rerouters
        .iter()
        .map(|b| (b.position.x, b.position.y, b.position.z))
        .collect();
    positions.sort_unstable();
    // chip at (0,0,0), half-extent 5; rerouter half-extent 1 -> edge offset 6,
    // z −1 (bottom-aligned). World<->screen (pinned in-game): +X = up, +Y = right,
    // so left = −Y edge, right = +Y edge, top = +X edge, bottom = −X edge.
    // Pins anchor at the top/left corner (run_start 4) and step in by pitch 2.
    // Left n=2 -> first-declared `go` at the top (x=4), `done` below (x=2);
    // top/right single pins also anchor to the corner (top's leftmost y=−4,
    // right's topmost x=4).
    let mut expected = vec![
        (4, -6, -1), // go (left edge dy=−6, top corner x=4, first in source)
        (2, -6, -1), // done (left edge, second, x=2)
        (6, -4, -1), // players (top edge dx=6, left corner y=−4)
        (4, 6, -1),  // score (right edge dy=6, top corner x=4)
    ];
    expected.sort_unstable();
    assert_eq!(positions, expected);
}

#[test]
fn rerouters_wire_to_inner_io_gates_with_correct_direction() {
    let w = world();
    let ty = |s: &brdb::WirePort| s.component_type.to_string();

    // in ports: rerouter.RER_Output -> MicrochipInput.RER_Input (go, players)
    let in_wires = w
        .wires
        .iter()
        .filter(|c| {
            ty(&c.source) == REROUTER
                && ty(&c.target) == CHIP_IN
                && c.source.port_name.to_string() == "RER_Output"
                && c.target.port_name.to_string() == "RER_Input"
        })
        .count();
    assert_eq!(in_wires, 2, "wires: {:#?}", w.wires);

    // out ports: MicrochipOutput.RER_Output -> rerouter.RER_Input (done, score)
    let out_wires = w
        .wires
        .iter()
        .filter(|c| {
            ty(&c.source) == CHIP_OUT
                && ty(&c.target) == REROUTER
                && c.source.port_name.to_string() == "RER_Output"
                && c.target.port_name.to_string() == "RER_Input"
        })
        .count();
    assert_eq!(out_wires, 2, "wires: {:#?}", w.wires);
}

#[test]
fn interleaved_left_side_keeps_source_order() {
    let w = world();
    // Left edge sits at y = −6; pins run top->bottom along X (screen up = +X).
    // The pin at the top end (x = +2) is `go`, an INPUT -> it must be a wire
    // SOURCE. The one below it (x = −2) is `done`, an output -> a wire TARGET.
    // This breaks if source-order interleaving regresses.
    let id_at = |x: i32| {
        w.bricks
            .iter()
            .find(|b| {
                b.asset == brdb::BrickType::from("B_1x1_Reroute_Node")
                    && b.position.y == -6
                    && b.position.x == x
            })
            .and_then(|b| b.id)
            .expect("left rerouter present")
    };
    let go_id = id_at(4);
    let done_id = id_at(2);
    assert!(
        w.wires.iter().any(|c| c.source.brick_id == go_id),
        "rerouter at x=4 (top corner) must be the input pin (wire source)"
    );
    assert!(
        w.wires.iter().any(|c| c.target.brick_id == done_id),
        "rerouter at x=2 must be the output pin (wire target)"
    );
}

// Three pins per side, sides interleaved in the source, ins and outs mixed.
// Every pin on a side has a different type, so its rerouter's colour says which
// pin it is.
const FOUR_SIDES_SRC: &str = "@left in l1: exec\n\
                              @top out t1: exec\n\
                              @right in r1: bool\n\
                              @bottom in b1: int\n\
                              @left out l2 = r1\n\
                              @top in t2: int\n\
                              @right out r2 = b1 + 1\n\
                              @bottom out b2 = r1\n\
                              @left in l3: int\n\
                              @top out t3 = r1\n\
                              @right in r3: exec\n\
                              @bottom in b3: exec\n\
                              on l1 { emit t1 }\n";

/// The colour a lone `@left` pin of type `ty` gets.
fn pin_color(ty: &str) -> brdb::Color {
    let w = common::world(&format!("@left in p: {ty}\n"), "color.ws");
    w.bricks
        .iter()
        .find(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .expect("a rerouter brick")
        .color
}

#[test]
fn each_side_runs_first_declared_from_the_top_left_corner() {
    let w = common::world(FOUR_SIDES_SRC, "four_sides.ws");
    let rerouters: Vec<&brdb::Brick> = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .collect();
    assert_eq!(rerouters.len(), 12, "one rerouter per annotated port");
    let is_input = |b: &brdb::Brick| w.wires.iter().any(|c| Some(c.source.brick_id) == b.id);

    // The inner MicrochipInput/Output brick a rerouter is wired to.
    let inner_of = |b: &brdb::Brick| -> &brdb::Brick {
        let other = w
            .wires
            .iter()
            .find_map(|c| {
                if Some(c.source.brick_id) == b.id {
                    Some(c.target.brick_id)
                } else if Some(c.target.brick_id) == b.id {
                    Some(c.source.brick_id)
                } else {
                    None
                }
            })
            .expect("a rerouter is wired to its inner pin");
        common::all_bricks(&w)
            .into_iter()
            .find(|i| i.id == Some(other))
            .expect("the inner pin brick")
    };

    // (side, edge test, distance from the corner, declared (type, is_input) in order).
    // +X is screen up and +Y screen right, so left/right pins run top to bottom
    // down X from x=4 and top/bottom pins run left to right along Y from y=−4.
    let sides: [(
        &str,
        fn(&brdb::Brick) -> bool,
        fn(&brdb::Brick) -> i32,
        [(&str, bool); 3],
    ); 4] = [
        (
            "left",
            |b| b.position.y == -6,
            |b| -b.position.x,
            [("exec", true), ("bool", false), ("int", true)],
        ),
        (
            "top",
            |b| b.position.x == 6,
            |b| b.position.y,
            [("exec", false), ("int", true), ("bool", false)],
        ),
        (
            "right",
            |b| b.position.y == 6,
            |b| -b.position.x,
            [("bool", true), ("int", false), ("exec", true)],
        ),
        (
            "bottom",
            |b| b.position.x == -6,
            |b| b.position.y,
            [("int", true), ("bool", false), ("exec", true)],
        ),
    ];
    for (side, on_edge, along, declared) in sides {
        let mut pins: Vec<&brdb::Brick> =
            rerouters.iter().copied().filter(|b| on_edge(b)).collect();
        assert_eq!(pins.len(), 3, "{side}: three pins on the edge");
        pins.sort_by_key(|b| along(b));
        assert_eq!(
            pins.iter().map(|b| along(b)).collect::<Vec<_>>(),
            vec![-4, -2, 0],
            "{side}: pins step in from the top/left corner"
        );
        for (i, (pin, (ty, input))) in pins.iter().zip(declared).enumerate() {
            assert_eq!(
                pin.color,
                pin_color(ty),
                "{side}: pin {} should be the `{ty}` port",
                i + 1
            );
            assert_eq!(is_input(pin), input, "{side}: pin {} in/out", i + 1);
        }
        // Pins 1 and 3 share a direction, so their inner gates share a stack,
        // where the earlier-declared one sits higher (+X is up on the plane).
        assert!(
            inner_of(pins[0]).position.x > inner_of(pins[2]).position.x,
            "{side}: pin 1's inner gate should sit above pin 3's"
        );
    }
}

/// Every chip's `MicrochipInput` and `MicrochipOutput` gates, read top to bottom
/// (highest X first), come in declaration order: the root program's ports and
/// a named chip's parameters and outputs alike.
#[test]
fn inner_io_gates_stack_first_declared_at_the_top() {
    use wirescript::ir::{Module, NodeKind};
    let src = "in a: int\n\
               in b: int\n\
               in c: int\n\
               chip Inner(p: int, q: int, r: int) -> (x: int, y: int, z: int) {\n  \
                 out x = p + 1\n  \
                 out y = q + 2\n  \
                 out z = r + 3\n\
               }\n\
               let i = Inner(a, b, c)\n\
               out d = i.x\n\
               out e = i.y\n\
               out f = i.z\n";
    let module = common::lowered(src, "inner_io.ws");
    let laid = wirescript::layout::layout(&module);

    fn check(module: &Module, x_of: &dyn Fn(wirescript::NodeId) -> i32, what: &str) {
        for kind in [NodeKind::Input, NodeKind::Output] {
            let mut by_decl: Vec<_> = module.nodes.values().filter(|n| n.kind == kind).collect();
            assert_eq!(by_decl.len(), 3, "{what}: three {kind:?} gates");
            by_decl.sort_by_key(|n| (n.source_range.start.offset, n.id.0));
            let mut top_down = by_decl.clone();
            top_down.sort_by_key(|n| std::cmp::Reverse(x_of(n.id)));
            assert_eq!(
                top_down.iter().map(|n| n.id).collect::<Vec<_>>(),
                by_decl.iter().map(|n| n.id).collect::<Vec<_>>(),
                "{what}: {kind:?} gates top to bottom should follow declaration order"
            );
        }
    }
    check(&module, &|id| laid.placements[&id].x, "root");
    assert_eq!(module.chips.len(), 1, "one named chip instance");
    for (id, child) in &module.chips {
        check(
            child,
            &|n| laid.chip_layouts[id].placements[&n].x,
            "named chip",
        );
    }
}

#[test]
fn rerouters_carry_port_name_labels() {
    let w = world();
    let labelled = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .filter(|b| b.components.iter().any(common::is_text_display))
        .count();
    assert_eq!(labelled, 4, "every rerouter gets a port-name label");
}

#[test]
fn unannotated_ports_add_no_outer_bricks() {
    let r = compile_to_world(
        common::input("in go: exec\nout score = 1\n", "plain.ws"),
        EmitOptions::default(),
    )
    .expect("should compile");
    assert_eq!(
        r.world.bricks.len(),
        1,
        "only the chip brick on the main grid: {:?}",
        r.world.bricks.iter().map(|b| &b.asset).collect::<Vec<_>>()
    );
}

// Two @bottom ports: exercises the bottom side arm + multi-port centering
// on a top/bottom row, which the main SRC never does.
const BOTTOM_SRC: &str = "@bottom in reset: exec\n\
                          @bottom out ready: exec\n\
                          on reset { emit ready }\n";

fn bottom_world() -> brdb::World {
    compile_to_world(
        common::input(BOTTOM_SRC, "bottom.ws"),
        EmitOptions::default(),
    )
    .expect("should compile")
    .world
}

#[test]
fn bottom_side_two_ports_center_along_y() {
    let w = bottom_world();
    let mut positions: Vec<(i32, i32, i32)> = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .map(|b| (b.position.x, b.position.y, b.position.z))
        .collect();
    positions.sort_unstable();
    // Bottom = screen-down = −X edge (dx = −6), z = −1; pins run left->right from
    // the left corner (y=−4) at pitch 2. reset (in, source-first) at y=−4;
    // ready (out) at y=−2.
    assert_eq!(positions, vec![(-6, -4, -1), (-6, -2, -1)]);
}

#[test]
fn invisible_port_rerouter_is_hidden_and_noncolliding() {
    let world = compile_to_world(
        common::input("@left @invisible in go: exec\non go { }\n", "invisible.ws"),
        EmitOptions::default(),
    )
    .expect("should compile")
    .world;
    let rer = world
        .bricks
        .iter()
        .find(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .expect("a rerouter brick");
    assert!(
        !rer.visible,
        "invisible port rerouter must have visible=false"
    );
    let c = rer.collision;
    assert!(
        !c.player
            && c.player1 != Some(true)
            && c.player2 != Some(true)
            && c.player3 != Some(true)
            && !c.weapon
            && !c.interact
            && !c.tool
            && !c.physics,
        "invisible rerouter must not collide: {c:?}"
    );
    assert!(
        !rer.components.iter().any(common::is_text_display),
        "invisible rerouter must not carry a port-name label"
    );
}

#[test]
fn bottom_ports_wire_and_label() {
    let w = bottom_world();
    let ty = |s: &brdb::WirePort| s.component_type.to_string();
    // reset is an input pin: rerouter.RER_Output -> MicrochipInput.RER_Input
    assert_eq!(
        w.wires
            .iter()
            .filter(|c| ty(&c.source) == REROUTER && ty(&c.target) == CHIP_IN)
            .count(),
        1,
        "wires: {:#?}",
        w.wires
    );
    // ready is an output pin: MicrochipOutput.RER_Output -> rerouter.RER_Input
    assert_eq!(
        w.wires
            .iter()
            .filter(|c| ty(&c.source) == CHIP_OUT && ty(&c.target) == REROUTER)
            .count(),
        1,
        "wires: {:#?}",
        w.wires
    );
    // both bottom rerouters carry a port-name label
    let labelled = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .filter(|b| b.components.iter().any(common::is_text_display))
        .count();
    assert_eq!(labelled, 2);
}
