//! Integration tests for the whole compile pipeline: source -> IR -> layout ->
//! BRZ bytes.
//!
//! Despite the file name, most of these are not snapshots: they assert a
//! program reaches real gates. `assert_compiles_clean` is the floor (no error,
//! no `_Unsupported` placeholder, non-empty output); the few tests with a
//! structural claim state it directly.
//! Tests are grouped by area:
//!   basic chip/mod snapshots
//!   scope capture BRZ tests
//!   correctness equivalence tests

mod common;

use wirescript::compile::{CompileInput, compile};
use wirescript::ir::Module;
use wirescript::lower::FoldMode;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Count nodes, wires, chips, and BRZ size for a source string.
///
/// Uses `compile` for the BRZ bytes and `lower` for the IR stats.
/// Chip counts and node/wire counts recurse through all nested chips.
fn compile_stats(src: &str) -> (usize, usize, usize, usize) {
    // Node/wire/chip counts below are approximate baselines that assume
    // folding stays off in both halves.
    // --- BRZ via compile ---
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
        fold_mode: FoldMode::ForceOff,
    };
    let result = compile(input).expect("should compile");
    let brz_size = result.brz.len();

    // --- IR stats via lower ---
    let (_resolved, _tc, lowered) = common::run_stages(src, "test.ws", FoldMode::ForceOff);

    fn count_recursive(module: &Module) -> (usize, usize, usize) {
        let mut nodes = module.nodes.len();
        let mut wires = module.wires.len();
        let mut chips = module.chips.len();
        for child_module in module.chips.values() {
            let (cn, cw, cc) = count_recursive(child_module);
            nodes += cn;
            wires += cw;
            chips += cc;
        }
        (nodes, wires, chips)
    }

    let (nodes, wires, chips) = count_recursive(&lowered.module);
    (nodes, wires, chips, brz_size)
}

/// Compile `src` and assert it produced a real circuit: no error diagnostic,
/// no `_Unsupported` placeholder gate anywhere in the tree, and non-empty
/// output.
///
/// `compile(...).expect(...)` alone passes on a program that type-checks and
/// then lowers half its statements to placeholders, which is the failure this
/// file exists to notice.
fn assert_compiles_clean(src: &str, why: &str) {
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
        fold_mode: FoldMode::ForceOff,
    };
    let r = compile(input).unwrap_or_else(|e| panic!("{why}: {e}"));
    let errors: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == wirescript::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{why}: {errors:?}");
    fn placeholders(m: &Module) -> usize {
        m.nodes.values().filter(|n| n.gate_class == "_Unsupported").count()
            + m.chips.values().map(placeholders).sum::<usize>()
    }
    let (_resolved, _tc, lowered) = common::run_stages(src, "test.ws", FoldMode::ForceOff);
    assert_eq!(placeholders(&lowered.module), 0, "{why}: lowered to a placeholder gate");
    assert!(!r.brz.is_empty(), "{why}: empty output");
}

// ── basic chip/mod snapshots ─────────────────────────────────────────────────

/// Simple chip Add(a, b) -> (r) called once with output port.
#[test]
fn a_simple_chip_compiles_clean() {
    let src = r#"
chip Add(a: int, b: int) -> (r: int) { out r = a + b }
let res = Add(1, 2)
out result = res.r
"#;
    assert_compiles_clean(src, "a chip called once with an output port");
}

/// Nested chip calls: Inner called inside Wrapper, Wrapper called twice.
#[test]
fn nested_chip_calls_compile_clean() {
    let src = r#"
chip Inner(x: int) -> (r: int) { out r = x + 1 }
mod Wrapper(v: int) -> (result: int) {
    let a = Inner(v)
    let b = Inner(a.r)
    return b.r
}
let w1 = Wrapper(10)
let w2 = Wrapper(20)
out total = w1 + w2
"#;
    assert_compiles_clean(src, "nested chip calls should compile");
}

// ── scope capture BRZ tests ──────────────────────────────────────────────────

/// Mod captures var and array from parent scope; called 3 times.
/// BRZ output must be > 100 bytes.
#[test]
fn a_mod_capturing_parent_state_emits_a_real_circuit() {
    let src = r#"
var counter: int = 0
var log: int[]
mod record_step() {
    counter = counter + 1
    log.push(counter)
}
in tick: exec
on tick {
    record_step()
    record_step()
    record_step()
}
out count = counter
"#;
    let (_, _, _, brz_size) = compile_stats(src);
    assert!(
        brz_size > 100,
        "BRZ for mod-with-capture should be > 100 bytes, got {}",
        brz_size
    );
}

/// Forward reference: mod save(val) uses array log which is declared after the mod.
#[test]
fn a_forward_referenced_array_capture_compiles_clean() {
    let src = r#"
mod save(val: int) {
    log.push(val)
}
var log: int[]
in tick: exec
on tick {
    save(42)
}
"#;
    assert_compiles_clean(src, "forward reference to array in mod should compile");
}

// ── correctness equivalence tests ────────────────────────────────────────────

/// Buffer capture: buffer prev_val is read by a mod that checks current != prev_val.
#[test]
fn a_buffer_capture_compiles_clean() {
    let src = r#"
in current: int
buffer prev_val = current
out changed = if current != prev_val then true else false
"#;
    assert_compiles_clean(src, "buffer capture in pure context should compile");
}

/// Exec chain in mods: two mods each with exec statements, called in a handler.
#[test]
fn an_exec_chain_in_a_mod_compiles_clean() {
    let src = r#"
var a: int = 0
var b: int = 0
mod reset_all() {
    a = 0
    b = 0
}
mod set_both(x: int) {
    a = x
    b = x * 2
}
in tick: exec
on tick {
    reset_all()
    set_both(5)
}
out sum = a + b
"#;
    assert_compiles_clean(src, "exec chain in mods should compile");
}

/// Mod calling chip Double 3 times; assert chips == 3.
#[test]
fn a_mod_calling_a_chip_repeatedly_instantiates_it_per_call() {
    let src = r#"
chip Double(v: int) -> (r: int) { out r = v * 2 }
mod apply_double(v: *int) {
    let d = Double(v)
    v = d.r
}
var x: int = 1
var y: int = 2
var z: int = 3
in tick: exec
on tick {
    apply_double(x)
    apply_double(y)
    apply_double(z)
}
out total = x + y + z
"#;
    let (_, _, chips, _) = compile_stats(src);
    assert_eq!(
        chips, 3,
        "3 calls to apply_double should create 3 chip instances, got {}",
        chips
    );
}

/// Record params dissolve into individual ports on a chip.
#[test]
fn a_record_param_chip_compiles_clean() {
    let src = r#"
type Vec2 = { x: int, y: int }
chip add_vec(a: Vec2, b: Vec2) -> (r: Vec2) {
    out r = { x: a.x + b.x, y: a.y + b.y }
}
let p: Vec2 = { x: 1, y: 2 }
let q: Vec2 = { x: 3, y: 4 }
let result = add_vec(p, q)
"#;
    assert_compiles_clean(src, "record param chip should compile");
}

/// 10 calls to chip Inc(x) -> (r); assert chips == 10, nodes >= 40, wires >= 30.
#[test]
fn ten_chip_calls_make_ten_instances() {
    let src = r#"
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
"#;
    let (nodes, wires, chips, _) = compile_stats(src);
    assert_eq!(
        chips, 10,
        "10 Inc calls should create 10 chip instances, got {}",
        chips
    );
    assert!(
        nodes >= 40,
        "10 chip instances with internals should produce >= 40 nodes, got {}",
        nodes
    );
    // 29, not 30: the first call's literal arg (`Inc(0)`) inlines as a data
    // default on its consumer gate (same mechanism as `n + 1`), eliding that
    // instance's input wire entirely.
    assert!(
        wires >= 29,
        "chained chip calls should produce >= 29 wires, got {}",
        wires
    );
}

/// Grandparent capture: mod add_score captures var score from root, called in handler.
#[test]
fn a_grandparent_var_capture_compiles_clean() {
    let src = r#"
var score: int = 0
mod add_score(pts: int) {
    score = score + pts
}
in tick: exec
on tick {
    add_score(10)
}
out total = score
"#;
    assert_compiles_clean(src, "grandparent var capture in mod should compile");
}

/// Mod chain: step() captures var, double_step() calls step() twice, double_step called twice.
#[test]
fn a_mod_chain_with_capture_compiles_clean() {
    let src = r#"
var counter: int = 0
mod step() {
    counter = counter + 1
}
mod double_step() {
    step()
    step()
}
in tick: exec
on tick {
    double_step()
    double_step()
}
out count = counter
"#;
    assert_compiles_clean(src, "mod chain with capture should compile");
}
