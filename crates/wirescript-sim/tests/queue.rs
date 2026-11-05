//! A Queue (Ticks) has no Wirescript syntax yet, so its program is written as
//! `.wsir`: `rec N` is recorded on ticks 1 to 3 into a queue that waits 5
//! ticks, and each release prints `out N`.

use wirescript_ir::EmitOptions;
use wirescript_sim::{NullWorld, Sim, SimOptions};

#[test]
fn a_queue_releases_one_record_a_tick_in_order() {
    let text = include_str!("fixtures/queue_ticks.wsir");
    let program = wirescript_ir::wsir::parse_program(text, "queue_ticks.wsir").expect("parses");
    let mut sim = Sim::from_program(
        &program,
        &EmitOptions::default(),
        Box::new(NullWorld::default()),
        SimOptions::default(),
    )
    .expect("loads");
    sim.run_until(15).expect("runs");
    let lines: Vec<String> = sim.chat().iter().map(|l| format!("{}: {}", l.tick, l.text)).collect();
    assert_eq!(lines, ["1: rec 1", "2: rec 2", "3: rec 3", "6: out 1", "7: out 2", "8: out 3"]);
}
