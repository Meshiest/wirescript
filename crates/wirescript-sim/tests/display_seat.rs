//! `@display` and `@seat` in the simulator: a display's rows decode back
//! into the pixels the program wrote, and a seat reports who sits in it.
//!
//! The programs are `.wsir`, compiled from:
//!
//! ```text
//! // display_grid
//! @display(width = 3, height = 2)
//! var screen
//! on Loaded() {
//!   screen[1] = Color(1.0, 0.0, 0.0)
//!   screen[5] = Color(0.0, 0.0, 1.0)
//! }
//!
//! // seat_rider
//! @seat in p1
//! on p1.occupied { BroadcastChatMessage("p1 sat") }
//! let keys = p1.character.InputReader()
//! on keys.Forward > 0.5 { BroadcastChatMessage("p1 forward") }
//! ```

use wirescript_ir::EmitOptions;
use wirescript_sim::{ScriptWorld, Sim, SimOptions};

fn sim(text: &str, file: &str, script: &str) -> Sim {
    let program = wirescript_ir::wsir::parse_program(text, file).expect("parses");
    let world = ScriptWorld::from_script(script).expect("script parses");
    Sim::from_program(
        &program,
        &EmitOptions::default(),
        Box::new(world),
        SimOptions::default(),
    )
    .expect("loads")
}

const RED: Option<[u8; 3]> = Some([255, 0, 0]);
const BLUE: Option<[u8; 3]> = Some([0, 0, 255]);

#[test]
fn a_display_shows_its_pixels_a_tick_after_they_are_written() {
    let mut sim = sim(
        include_str!("fixtures/display_grid.wsir"),
        "display_grid.wsir",
        "",
    );
    sim.run_until(0).expect("runs");
    let screen = |sim: &Sim| {
        sim.machine
            .world
            .displays()
            .and_then(|d| d.get("screen"))
            .cloned()
            .expect("the display is recorded")
    };
    assert!(
        screen(&sim).pixels().iter().all(Option::is_none),
        "the load tick wrote the pixels; the rows have not caught up"
    );
    sim.run_until(1).expect("runs");
    let s = screen(&sim);
    assert_eq!((s.width, s.height), (3, 2));
    assert_eq!(s.pixels(), [None, RED, None, None, None, BLUE]);
    assert_eq!(s.pixel(1, 0), RED);
    let frames = sim.machine.world.displays().unwrap().frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].tick, 1);
}

#[test]
fn a_seat_reports_its_sitter_and_their_keys() {
    let script = "\
3 sit alice p1
5 input alice Forward 1
7 stand alice
8 input alice Forward 0
9 sit bob p1
11 input bob Forward 1
";
    let mut sim = sim(
        include_str!("fixtures/seat_rider.wsir"),
        "seat_rider.wsir",
        script,
    );
    sim.run_until(15).expect("runs");
    let lines: Vec<String> = sim
        .chat()
        .iter()
        .map(|l| format!("{}: {}", l.tick, l.text))
        .collect();
    assert_eq!(
        lines,
        ["3: p1 sat", "5: p1 forward", "9: p1 sat", "11: p1 forward"]
    );
}

#[test]
fn a_player_who_leaves_frees_the_seat() {
    let mut sim = sim(
        include_str!("fixtures/seat_rider.wsir"),
        "seat_rider.wsir",
        "1 join alice\n2 sit alice p1\n4 leave alice\n6 sit bob p1\n",
    );
    sim.run_until(8).expect("runs");
    assert_eq!(
        sim.machine.world.unfinished(),
        None,
        "bob takes the empty seat"
    );
    let lines: Vec<String> = sim
        .chat()
        .iter()
        .map(|l| format!("{}: {}", l.tick, l.text))
        .collect();
    assert_eq!(lines, ["2: p1 sat", "6: p1 sat"]);
}

#[test]
fn an_occupied_seat_refuses_a_second_sitter() {
    let mut sim = sim(
        include_str!("fixtures/seat_rider.wsir"),
        "seat_rider.wsir",
        "2 sit alice p1\n4 sit bob p1\n",
    );
    sim.run_until(6).expect("runs");
    let problem = sim
        .machine
        .world
        .unfinished()
        .expect("the second sit fails");
    assert!(problem.contains("alice is sitting there"), "{problem}");
}

#[test]
fn a_seat_no_line_fills_is_empty() {
    let mut sim = sim(
        include_str!("fixtures/seat_rider.wsir"),
        "seat_rider.wsir",
        "",
    );
    sim.run_until(5).expect("an empty seat is not an error");
    assert!(sim.chat().is_empty());
}
