//! Chip template instantiation through the whole pipeline: source -> IR ->
//! layout -> BRZ bytes. Each chip call must become its own instance with its
//! own internals.

use crate::common;

use wirescript::FoldMode;
use wirescript::compile::{CompileInput, compile};
use wirescript::ir::Module;

/// Count nodes, wires, chips, and BRZ size for a source string, folding off:
/// `compile` gives the BRZ bytes and `lower` the IR stats, counted through
/// every nested chip.
fn compile_stats(src: &str) -> (usize, usize, usize, usize) {
    let input = CompileInput::new(src, "test.ws").fold_mode(FoldMode::ForceOff);
    let result = compile(input).expect("should compile");
    let brz_size = result.brz.len();

    let (_resolved, _tc, lowered) = common::run_stages(src, "test.ws", FoldMode::ForceOff);

    fn count_recursive(module: &Module) -> (usize, usize, usize) {
        let (mut nodes, mut wires, mut chips) = (0, 0, 0);
        module.walk(&mut |_, m| {
            nodes += m.nodes.len();
            wires += m.wires.len();
            chips += m.chips.len();
        });
        (nodes, wires, chips)
    }

    let (nodes, wires, chips) = count_recursive(&lowered.module);
    (nodes, wires, chips, brz_size)
}

/// 10 chained calls to chip Inc(x) -> (r) make 10 instances, each with its
/// own internals and wiring.
#[test]
fn ten_chip_calls_make_ten_instances() {
    let src = r"
chip Inc(x: int) -> (r: int) { out r = x + 1 }
let v0 = Inc(0)
let v1 = Inc(v0.r)
let v2 = Inc(v1.r)
let v3 = Inc(v2.r)
let v4 = Inc(v3.r)
let v5 = Inc(v4.r)
let v6 = Inc(v5.r)
let v7 = Inc(v6.r)
let v8 = Inc(v7.r)
let v9 = Inc(v8.r)
out result = v9.r
";
    let (nodes, wires, chips, brz) = compile_stats(src);
    assert!(brz > 0, "empty output");
    // 29 wires, not 30: the first call's literal arg (`Inc(0)`) inlines as a
    // data default on its consumer gate (same mechanism as `n + 1`), eliding
    // that instance's input wire entirely.
    assert_eq!(
        (chips, nodes, wires),
        (10, 40, 29),
        "(chip instances, nodes, wires)"
    );
}
