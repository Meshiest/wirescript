//! `@side` port annotations place pre-wired rerouter bricks on the outer
//! grid, flush against the compiled microchip brick (spec:
//! wirescript SDK docs/superpowers/specs/2026-07-12-port-side-rerouters-design.md).

use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, FoldMode, compile_to_world};

fn is_text_display(c: &Box<dyn brdb::BrdbComponent>) -> bool {
    c.component_type()
        .map(|t| t.to_string() == "Component_TextDisplay")
        .unwrap_or(false)
}

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
    compile_to_world(
        CompileInput {
            source: SRC,
            file: "pins.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
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
    positions.sort();
    // chip at (0,0,0), half-extent 5; rerouter half-extent 1 → edge offset 6,
    // z −1 (bottom-aligned). World↔screen (pinned in-game): +X = up, +Y = right,
    // so left = −Y edge, right = +Y edge, top = +X edge, bottom = −X edge.
    // Pins anchor at the top/left corner (run_start 4) and step in by pitch 2.
    // Left n=2 → first-declared `go` at the top (x=4), `done` below (x=2);
    // top/right single pins also anchor to the corner (top's leftmost y=−4,
    // right's topmost x=4).
    let mut expected = vec![
        (4, -6, -1), // go (left edge dy=−6, top corner x=4, first in source)
        (2, -6, -1), // done (left edge, second, x=2)
        (6, -4, -1), // players (top edge dx=6, left corner y=−4)
        (4, 6, -1),  // score (right edge dy=6, top corner x=4)
    ];
    expected.sort();
    assert_eq!(positions, expected);
}

#[test]
fn rerouters_wire_to_inner_io_gates_with_correct_direction() {
    let w = world();
    let ty = |s: &brdb::WirePort| s.component_type.to_string();

    // in ports: rerouter.RER_Output → MicrochipInput.RER_Input (go, players)
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

    // out ports: MicrochipOutput.RER_Output → rerouter.RER_Input (done, score)
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
    // Left edge sits at y = −6; pins run top→bottom along X (screen up = +X).
    // The pin at the top end (x = +2) is `go`, an INPUT → it must be a wire
    // SOURCE. The one below it (x = −2) is `done`, an output → a wire TARGET.
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

#[test]
fn rerouters_carry_port_name_labels() {
    let w = world();
    let labelled = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .filter(|b| b.components.iter().any(is_text_display))
        .count();
    assert_eq!(labelled, 4, "every rerouter gets a port-name label");
}

#[test]
fn unannotated_ports_add_no_outer_bricks() {
    let r = compile_to_world(
        CompileInput {
            source: "in go: exec\nout score = 1\n",
            file: "plain.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
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
        CompileInput { source: BOTTOM_SRC, file: "bottom.ws", module_name: None, fold_mode: FoldMode::Auto },
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
    positions.sort();
    // Bottom = screen-down = −X edge (dx = −6), z = −1; pins run left→right from
    // the left corner (y=−4) at pitch 2. reset (in, source-first) at y=−4;
    // ready (out) at y=−2.
    assert_eq!(positions, vec![(-6, -4, -1), (-6, -2, -1)]);
}

#[test]
fn bottom_ports_wire_and_label() {
    let w = bottom_world();
    let ty = |s: &brdb::WirePort| s.component_type.to_string();
    // reset is an input pin: rerouter.RER_Output -> MicrochipInput.RER_Input
    assert_eq!(
        w.wires.iter().filter(|c| ty(&c.source) == REROUTER && ty(&c.target) == CHIP_IN).count(),
        1,
        "wires: {:#?}", w.wires
    );
    // ready is an output pin: MicrochipOutput.RER_Output -> rerouter.RER_Input
    assert_eq!(
        w.wires.iter().filter(|c| ty(&c.source) == CHIP_OUT && ty(&c.target) == REROUTER).count(),
        1,
        "wires: {:#?}", w.wires
    );
    // both bottom rerouters carry a port-name label
    let labelled = w
        .bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .filter(|b| b.components.iter().any(is_text_display))
        .count();
    assert_eq!(labelled, 2);
}
