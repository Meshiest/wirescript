//! `@display` and `@seat` place bricks around the program's chip: a display's
//! row strips, border and back chip, and seats on the chip's sides.

use brdb::{Brick, BrickType};

use crate::common;

const DETECTOR: &str = "BrickComponentType_Expr_ContainerChangeDetector";
const TEXT: &str = "Component_TextDisplay";
const SEAT: &str = "Component_Internal_Seat";

fn components(b: &Brick) -> Vec<String> {
    b.components
        .iter()
        .filter_map(|c| {
            c.component_type_ref()
                .map(|t| t.to_string())
                .or_else(|| c.component_type().map(|t| t.to_string()))
        })
        .collect()
}

fn has(b: &Brick, class: &str) -> bool {
    components(b).iter().any(|c| c == class)
}

/// A display row: a procedural strip carrying a Text Decal. A chip's name
/// label is a Text Decal too, on a basic brick.
fn is_strip(b: &Brick) -> bool {
    has(b, TEXT) && size(b).is_some()
}

fn size(b: &Brick) -> Option<(u16, u16, u16)> {
    match &b.asset {
        BrickType::Procedural { size, .. } => Some((size.x, size.y, size.z)),
        BrickType::Basic(_) => None,
    }
}

fn degrees(r: brdb::Rotation) -> i32 {
    match r {
        brdb::Rotation::Deg0 => 0,
        brdb::Rotation::Deg90 => 90,
        brdb::Rotation::Deg180 => 180,
        brdb::Rotation::Deg270 => 270,
    }
}

fn pos(b: &Brick) -> (i32, i32, i32) {
    (b.position.x, b.position.y, b.position.z)
}

/// The outer grid relative to the program's chip, which `add_microchip`
/// pushes first.
fn outer(world: &brdb::World) -> (&Brick, &[Brick]) {
    (&world.bricks[0], &world.bricks[1..])
}

const DISPLAY: &str = "@display(width = 4, height = 3, size = 0.4)
@color(Color(1.0, 0.0, 0.0))
var screen

in go: exec
on go {
  screen[5] = Color(0.0, 1.0, 0.0)
}
";

#[test]
fn display_rows_are_strips_carrying_text_decals() {
    let w = common::world(DISPLAY, "display.ws");
    let (chip, bricks) = outer(&w);
    let strips: Vec<&Brick> = bricks.iter().filter(|b| is_strip(b)).collect();
    assert_eq!(strips.len(), 3, "one strip per row");
    // Pitch 4 (size 0.4): each strip is 4 wide per pixel and one pixel tall,
    // stacked downward from the top row, all in the chip's plane.
    let mut zs: Vec<i32> = strips.iter().map(|b| b.position.z).collect();
    zs.sort_unstable();
    assert_eq!(
        zs.windows(2).map(|p| p[1] - p[0]).collect::<Vec<_>>(),
        [4, 4]
    );
    for s in &strips {
        assert_eq!(size(s), Some((2, 8, 2)));
        assert_eq!(
            s.position.x,
            chip.position.x + 30,
            "3 studs out on the top side"
        );
        assert_eq!(
            s.position.y, chip.position.y,
            "one display centers on the chip"
        );
        assert_eq!(
            (s.color.r, s.color.g, s.color.b),
            (255, 0, 0),
            "@color colors the panel"
        );
        assert!(s.position.z > chip.position.z);
    }
}

#[test]
fn display_has_a_border_and_a_back_chip_behind_the_panel() {
    let w = common::world(DISPLAY, "display.ws");
    let (chip, bricks) = outer(&w);
    let border: Vec<&Brick> = bricks
        .iter()
        .filter(|b| b.components.is_empty() && size(b).is_some())
        .collect();
    assert_eq!(border.len(), 4, "four border bars");
    for b in &border {
        assert_eq!((b.color.r, b.color.g, b.color.b), (255, 0, 0));
    }
    let backs: Vec<&Brick> = bricks
        .iter()
        .filter(|b| b.asset == brdb::assets::bricks::B_MICROCHIP)
        .collect();
    assert_eq!(backs.len(), 1, "one back chip");
    let back = backs[0];
    assert!(
        back.position.x > chip.position.x,
        "the back chip sits behind the panel"
    );
    assert_eq!(back.position.y, chip.position.y);
}

#[test]
fn display_rebuild_lives_in_the_back_chip_with_one_detector() {
    let w = common::world(DISPLAY, "display.ws");
    let detectors = common::all_bricks(&w)
        .into_iter()
        .filter(|b| has(b, DETECTOR))
        .count();
    assert_eq!(detectors, 1, "one container change detector");
    // The root plane holds no part of the rebuild: its detector, function
    // plane and row vars are all in the back chip's grid.
    let root_grid = &w.grids[0].1;
    assert!(!root_grid.iter().any(|b| has(b, DETECTOR)));
    assert!(
        !root_grid
            .iter()
            .any(|b| b.asset.asset().to_string().contains("FunctionDefinition"))
    );
}

#[test]
fn display_starts_with_every_pixel_transparent() {
    let m = common::lowered(DISPLAY, "display.ws");
    let screen = m
        .nodes
        .values()
        .find(|n| {
            common::prop(n, "_label") == Some(wirescript::ir::Literal::String("screen".into()))
                && n.gate_class.contains("ArrayVar")
        })
        .expect("the screen array");
    let Some(wirescript::ir::Literal::Array(pixels)) = common::prop(screen, "InitialValue") else {
        panic!("no initial pixels");
    };
    assert_eq!(pixels.len(), 12);
    assert!(pixels.iter().all(|p| matches!(
        p,
        wirescript::ir::Literal::LinearColor { a, .. } if *a == 0.0
    )));
}

#[test]
fn displays_stack_left_to_right_without_overlap() {
    let src = "@display(width = 2, height = 2)\nvar a\n@display(width = 3, height = 1, border = false)\nvar b\n";
    let w = common::world(src, "two.ws");
    let (_, bricks) = outer(&w);
    let strips: Vec<&Brick> = bricks.iter().filter(|b| is_strip(b)).collect();
    assert_eq!(strips.len(), 3);
    let a: Vec<&&Brick> = strips
        .iter()
        .filter(|b| size(b) == Some((2, 10, 5)))
        .collect();
    let b: Vec<&&Brick> = strips
        .iter()
        .filter(|b| size(b) == Some((2, 15, 5)))
        .collect();
    assert_eq!((a.len(), b.len()), (2, 1));
    assert!(
        a[0].position.y < b[0].position.y,
        "declaration order runs left to right"
    );
    assert_eq!(
        bricks
            .iter()
            .filter(|b| b.asset == brdb::assets::bricks::B_MICROCHIP)
            .count(),
        2
    );
}

#[test]
fn a_display_lifts_the_root_plane_above_it() {
    let short = common::world("@display(width = 2, height = 1)\nvar s\n", "short.ws");
    let tall = common::world("@display(width = 2, height = 3)\nvar s\n", "tall.ws");
    let z = |w: &brdb::World| w.grids[0].0.location.z;
    // Two more rows at pitch 10 lift the root plane at least as much.
    assert!(z(&tall) - z(&short) >= 20.0);
    let panel_top = tall
        .bricks
        .iter()
        .filter_map(|b| size(b).map(|s| b.position.z + i32::from(s.2)))
        .max()
        .unwrap();
    assert!(
        z(&tall) > panel_top as f32 + 10.0,
        "the root plane clears the panel"
    );
}

#[test]
fn display_options_are_checked() {
    let codes = |src: &str| common::compile_diag_codes(src, "bad.ws");
    for src in [
        "@display(width = 4)\nvar s\n",
        "@display(width = 4, height = 2)\nvar s: int[]\n",
        "@display(width = 4, height = 2)\nvar s: color[] = []\n",
        "@display(width = 4, height = 2, size = 0.3)\nvar s\n",
        "@display(width = 4, height = 2, font = \"Comic\")\nvar s\n",
        "@display(width = 4, height = 2, depth = 1)\nvar s\n",
        "@display(width = 2000, height = 2)\nvar s\n",
        "@display(width = 1000, height = 2, size = 2.0)\nvar s\n",
        "@display(width = 4, height = 2)\nin s: int\n",
        "in go: exec\non go {\n  @display(width = 4, height = 2)\n  var s\n}\n",
    ] {
        assert!(
            codes(src).contains(&"WS127".to_string()),
            "{src:?}: {:?}",
            codes(src)
        );
    }
}

const SEATS: &str = "@seat(label = \"One\") @color(Color(0.0, 0.0, 1.0))
in a
@seat in b
@top @seat in c
@left @seat in d
@right @seat(mode = \"Ground\", camera = Rotation(-10.0, 0.0, 0.0), exit = Vec(0, 0, 30))
in e
out busy = a.occupied || b.occupied || c.occupied || d.occupied || e.occupied
";

fn seats(w: &brdb::World) -> Vec<&Brick> {
    w.bricks.iter().filter(|b| has(b, SEAT)).collect()
}

#[test]
fn seats_sit_on_their_side_centered_in_order_facing_the_chip() {
    let w = common::world(SEATS, "seats.ws");
    let (chip, _) = outer(&w);
    let (cx, cy, cz) = pos(chip);
    let mut placed: Vec<((i32, i32, i32), i32)> = seats(&w)
        .iter()
        .map(|b| (pos(b), degrees(b.rotation)))
        .collect();
    placed.sort_by_key(|(p, _)| *p);
    let z = cz - 2 + 11;
    // Deg0 faces +Y, Deg90 -X, Deg180 -Y, Deg270 +X.
    let mut expected = vec![
        // bottom row (-X), left to right as seen from the seats = +Y; a then b
        ((cx - 25, cy - 15, z), 270),
        ((cx - 25, cy + 15, z), 270),
        // top (+X), alone and centered, facing -X
        ((cx + 25, cy, z), 90),
        // left (-Y), facing +Y
        ((cx, cy - 25, z), 0),
        // right (+Y), facing -Y
        ((cx, cy + 25, z), 180),
    ];
    expected.sort_by_key(|(p, _)| *p);
    assert_eq!(placed, expected);
    // Source order: `a` is the bottom row's leftmost (lowest Y), so it holds
    // the label.
    let a = seats(&w)
        .into_iter()
        .find(|b| pos(b) == (cx - 25, cy - 15, z))
        .unwrap();
    assert_eq!((a.color.r, a.color.g, a.color.b), (0, 0, 255));
    assert!(a.asset == brdb::assets::bricks::B_SEAT);
}

#[test]
fn seat_options_reach_the_seat_component() {
    let brz = common::brz(SEATS, "seats.ws");
    use brdb::schema::BrdbValue;
    let comps = common::world_components(&brz);
    let labels: Vec<String> = comps
        .iter()
        .filter_map(|c| match c.get("PromptCustomLabel") {
            Some(BrdbValue::String(s)) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, ["One"]);
    let exits: Vec<i32> = comps
        .iter()
        .filter_map(|c| match c.get("ExitOffset") {
            Some(BrdbValue::Struct(s)) => match s.get("Z") {
                Some(BrdbValue::I32(z)) => Some(*z),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(exits.contains(&30), "{exits:?}");
    // Only `e` asked for Ground (1); the others keep Auto (0).
    let grounds = comps
        .iter()
        .filter(|c| matches!(c.get("SeatMode"), Some(BrdbValue::Enum(m)) if m.get_value_raw() == 1))
        .count();
    assert_eq!(grounds, 1);
}

#[test]
fn seat_reads_wire_from_the_seat_outputs() {
    let m = common::lowered(SEATS, "seats.ws");
    let seat_ids: Vec<_> = m
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == SEAT)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(seat_ids.len(), 5);
    assert!(
        m.wires.iter().any(|w| seat_ids.contains(&w.source.node_id)),
        "something reads a seat"
    );
}

#[test]
fn seat_options_are_checked() {
    let codes = |src: &str| common::compile_diag_codes(src, "bad.ws");
    for src in [
        "@seat in s: int\n",
        "@seat(mode = \"Fly\") in s\n",
        "@seat(label = 3) in s\n",
        "@seat(color = 1) in s\n",
        "@seat out s = 1\n",
        "@seat var s: int = 0\n",
    ] {
        assert!(
            codes(src).contains(&"WS128".to_string()),
            "{src:?}: {:?}",
            codes(src)
        );
    }
}

#[test]
fn programs_without_display_or_seat_place_nothing_outside() {
    let w = common::world(
        "in go: exec\nvar n: int = 0\non go { n += 1 }\n",
        "plain.ws",
    );
    assert_eq!(w.bricks.len(), 1, "only the chip");
}

#[test]
fn a_negated_record_field_trigger_fires_on_the_inverse() {
    // `on !q.occupied` once dropped its `!` for any record binding and fired
    // on the field going true.
    let m = common::lowered(
        "in q: { occupied: bool, character: character }\non !q.occupied {\n  BroadcastChatMessage(\"empty\")\n}\n",
        "neg.ws",
    );
    let not = m
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class.contains("LogicalNOT"))
        .map(|(id, _)| *id)
        .expect("a NOT gate");
    let say = m
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class.contains("BroadcastChatMessage"))
        .map(|(id, _)| *id)
        .unwrap();
    assert!(
        m.wires
            .iter()
            .any(|w| w.source.node_id == not && w.target.node_id == say),
        "the handler fires off the NOT"
    );
}

#[test]
fn testing_statements_stay_out_of_the_back_chip() {
    // The `testing` rewrite and the display's code both make up ranges past
    // the end of the file; the display's start after the testing ones.
    let src = "@display(width = 2, height = 1)\nvar s\n\n@test mod t() {\n  s[0] = Color(1.0, 0.0, 0.0)\n  testing.assert(s.length() == 2, \"two pixels\")\n}\n";
    let input = wirescript::CompileInput::new(src, "t.ws")
        .test_build(true)
        .test_entry("t");
    let result =
        wirescript::compile_to_world(input, wirescript::emit::EmitOptions::default()).unwrap();
    let module = &result.program.module;
    let back = module
        .nodes
        .iter()
        .find(|(_, n)| {
            common::prop(n, "_exterior") == Some(wirescript::ir::Literal::String("display".into()))
        })
        .map(|(id, _)| *id)
        .expect("a back chip");
    let chip = &module.chips[&back];
    assert!(
        !chip
            .nodes
            .values()
            .any(|n| n.gate_class.contains("BroadcastChatMessage")),
        "the assertion's message stays in the program's plane"
    );
}

#[test]
fn display_with_flat_and_other_misuse_is_reported() {
    let codes = |src: &str| common::compile_diag_codes(src, "bad.ws");
    assert!(
        codes("@flat\n\n@display(width = 2, height = 2)\nvar s\n").contains(&"WS127".to_string())
    );
    assert!(codes("@display(width = 300, height = 300)\nvar s\n").contains(&"WS127".to_string()));
    assert!(codes("var __display_s_rows: int = 0\n").contains(&"WS093".to_string()));
}

#[test]
fn a_seat_of_the_programs_own_is_left_alone() {
    let codes = common::compile_diag_codes(
        "enum Seat { Front, Back }\nvar s: Seat = Seat.Front\n",
        "own.ws",
    );
    assert!(!codes.contains(&"WS013".to_string()), "{codes:?}");
    let codes = common::compile_diag_codes(
        "type Seat = { occupied: bool, character: character }\n@seat in p\nout o = p.occupied\n",
        "alias.ws",
    );
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_displays_material_styles_its_panel() {
    let w = common::world(
        "@display(width = 2, height = 2) @material(Glow)\nvar s\n",
        "glow.ws",
    );
    let plain = common::world("@display(width = 2, height = 2)\nvar s\n", "plain.ws");
    let material = |w: &brdb::World| {
        w.bricks
            .iter()
            .filter(|b| size(b).is_some())
            .map(|b| b.material.to_string())
            .collect::<Vec<_>>()
    };
    let (glow, flat) = (material(&w), material(&plain));
    assert_eq!(glow.len(), 6, "two strips and four border bars");
    assert!(glow.iter().all(|m| *m == glow[0]));
    assert_ne!(glow[0], flat[0], "@material changes the panel's material");
}

#[test]
fn a_panel_is_micro_bricks_of_any_size() {
    // The game drops a `PB_DefaultBrick` that is not whole studs and plates,
    // as a 2-unit-deep row strip is; a micro brick takes any size.
    let w = common::world(
        "@display(width = 3, height = 2, size = 0.2)\nvar s\n",
        "micro.ws",
    );
    let panel: Vec<String> = w
        .bricks
        .iter()
        .filter(|b| size(b).is_some())
        .map(|b| b.asset.asset().to_string())
        .collect();
    assert_eq!(panel.len(), 6);
    assert!(
        panel.iter().all(|a| a == "PB_DefaultMicroBrick"),
        "{panel:?}"
    );
}

#[test]
fn a_display_stands_on_its_side_facing_the_chip() {
    // Seen from above: +X up the screen, +Y right. Each side's panel stands 3
    // studs out from the chip and is turned to face it.
    let cases = [
        ("", (30, 0), 0),
        ("@bottom ", (-30, 0), 180),
        ("@left ", (0, -30), 270),
        ("@right ", (0, 30), 90),
    ];
    for (side, (dx, dy), turn) in cases {
        let src = format!("{side}@display(width = 2, height = 1, border = false)\nvar s\n");
        let w = common::world(&src, "side.ws");
        let (chip, bricks) = outer(&w);
        let strip = bricks.iter().find(|b| is_strip(b)).expect("a strip");
        assert_eq!(
            (
                strip.position.x - chip.position.x,
                strip.position.y - chip.position.y,
                degrees(strip.rotation)
            ),
            (dx, dy, turn),
            "{side:?}"
        );
    }
}

#[test]
fn an_offset_moves_the_display_in_its_own_frame() {
    // On the left side the display faces +Y, so its viewer looks toward -Y:
    // their right is +X, and out toward them is +Y.
    let src =
        "@left @display(width = 2, height = 1, border = false, offset = Vec(1, 2, 3))\nvar s\n";
    let w = common::world(src, "offset.ws");
    let plain = common::world(
        "@left @display(width = 2, height = 1, border = false)\nvar s\n",
        "plain.ws",
    );
    let strip = |w: &brdb::World| {
        let b = w.bricks.iter().find(|b| is_strip(b)).unwrap().position;
        (b.x, b.y, b.z)
    };
    let (a, b) = (strip(&w), strip(&plain));
    assert_eq!((a.0 - b.0, a.1 - b.1, a.2 - b.2), (10, 20, 30));
}

#[test]
fn the_back_chip_is_closed_and_flat_on_the_panel() {
    let w = common::world("@display(width = 4, height = 4)\nvar s\n", "back.ws");
    let (chip, bricks) = outer(&w);
    let back = bricks
        .iter()
        .find(|b| b.asset == brdb::assets::bricks::B_MICROCHIP)
        .unwrap();
    assert!(
        matches!(back.direction, brdb::Direction::XPositive),
        "facing away from the panel"
    );
    // Centered on the panel: its border bars' middle.
    let bars: Vec<&Brick> = bricks
        .iter()
        .filter(|b| b.components.is_empty() && size(b).is_some())
        .collect();
    let mid_z = bars.iter().map(|b| b.position.z).sum::<i32>() / bars.len() as i32;
    assert_eq!(back.position.y, chip.position.y);
    assert_eq!(back.position.z, mid_z);
    let m = common::lowered("@display(width = 4, height = 4)\nvar s\n", "back.ws");
    let node = m
        .nodes
        .values()
        .find(|n| {
            common::prop(n, "_exterior") == Some(wirescript::ir::Literal::String("display".into()))
        })
        .unwrap();
    assert_eq!(
        common::prop(node, "_closed"),
        Some(wirescript::ir::Literal::Bool(true))
    );
}

#[test]
fn wires_to_the_back_chip_run_through_invisible_rerouters() {
    let src = "@display(width = 2, height = 1)\nvar s\nin go: exec\non go {\n  s[0] = Color(1.0, 0.0, 0.0)\n  BroadcastChatMessage(__display_s_row0)\n}\n";
    let w = common::world(src, "wires.ws");
    let (_, bricks) = outer(&w);
    let hidden: Vec<&Brick> = bricks
        .iter()
        .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .collect();
    // In: the pixels. Out: the row the program reads back.
    assert_eq!(hidden.len(), 2);
    assert!(hidden.iter().all(|b| !b.visible && !b.collision.player));
}

#[test]
fn a_display_rises_over_a_seat_only_when_it_stands_over_one() {
    let z = |src: &str| {
        let w = common::world(src, "z.ws");
        let (chip, bricks) = outer(&w);
        bricks.iter().find(|b| is_strip(b)).unwrap().position.z - chip.position.z
    };
    let alone = z("@display(width = 2, height = 1, border = false)\nvar s\n");
    let facing = z("@display(width = 2, height = 1, border = false)\nvar s\n@seat in p\n");
    let over = z("@display(width = 2, height = 1, border = false)\nvar s\n@top @seat in p\n");
    assert_eq!(
        facing, alone,
        "a seat across the chip leaves the display down"
    );
    assert!(over > alone, "a seat under the display raises it");
}

#[test]
fn a_display_wire_rerouter_points_up_at_the_back_chip() {
    let src = "@display(width = 2, height = 1)\nvar s\nin go: exec\non go {\n  s[0] = Color(1.0, 0.0, 0.0)\n}\n";
    let w = common::world(src, "arrow.ws");
    let (_, bricks) = outer(&w);
    let back = bricks
        .iter()
        .find(|b| b.asset == brdb::assets::bricks::B_MICROCHIP)
        .unwrap();
    let rer = bricks
        .iter()
        .find(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
        .unwrap();
    assert_eq!(
        format!("{:?}", rer.direction),
        format!("{:?}", back.direction)
    );
    assert_eq!(degrees(rer.rotation), 270);
    assert!(rer.position.z < back.position.z, "under the back chip");
}
