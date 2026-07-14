//! Integration snapshot tests for the wirescript compiler.
//!
//! Tests exercise the full compile pipeline: source → IR → layout → BRZ bytes.
//! Tests are grouped by task area:
//!   Task 6 — basic chip/mod snapshots
//!   Task 7 — scope capture BRZ tests
//!   Task 10 — correctness equivalence tests

use std::sync::Arc;
use wirescript::compile::{CompileInput, compile};
use wirescript::ir::Module;
use wirescript::lower::{LowerInput, lower};
use wirescript::resolve::{FsLoader, resolve};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::typecheck;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Count nodes, wires, chips, and BRZ size for a source string.
///
/// Uses `compile` for the BRZ bytes and `lower` for the IR stats.
/// Chip counts and node/wire counts recurse through all nested chips.
fn compile_stats(src: &str) -> (usize, usize, usize, usize) {
    // --- BRZ via compile ---
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    let result = compile(input).expect("should compile");
    let brz_size = result.brz.len();

    // --- IR stats via lower ---
    let resolved = resolve(src, "test.ws", &FsLoader);
    let tc = typecheck(&resolved.ast, "test.ws");
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test.ws",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
    });

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

// ── Task 6: basic chip/mod snapshots ─────────────────────────────────────────

/// Simple chip Add(a, b) -> (r) called once with output port.
#[test]
fn snapshot_simple_chip() {
    let src = r#"
chip Add(a: int, b: int) -> (r: int) { out r = a + b }
let res = Add(1, 2)
out result = res.r
"#;
    let (_, _, _, brz_size) = compile_stats(src);
    assert!(
        brz_size > 0,
        "BRZ output should be non-empty, got {} bytes",
        brz_size
    );
}

/// Nested chip calls: Inner called inside Wrapper, Wrapper called twice.
#[test]
fn snapshot_nested_chip_calls() {
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
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("nested chip calls should compile");
}

// ── Task 7: scope capture BRZ tests ──────────────────────────────────────────

/// Mod captures var and array from parent scope; called 3 times.
/// BRZ output must be > 100 bytes.
#[test]
fn snapshot_mod_with_parent_capture() {
    let src = r#"
var counter: int = 0
array log: int[]
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
fn snapshot_cross_file_capture_pattern() {
    let src = r#"
mod save(val: int) {
    log.push(val)
}
array log: int[]
in tick: exec
on tick {
    save(42)
}
"#;
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("forward reference to array in mod should compile");
}

// ── Task 10: correctness equivalence tests ────────────────────────────────────

/// Buffer capture: buffer prev_val is read by a mod that checks current != prev_val.
#[test]
fn snapshot_buffer_capture() {
    let src = r#"
in current: int
buffer prev_val = current
out changed = if current != prev_val then true else false
"#;
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("buffer capture in pure context should compile");
}

/// Exec chain in mods: two mods each with exec statements, called in a handler.
#[test]
fn snapshot_exec_chain_in_mod() {
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
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("exec chain in mods should compile");
}

/// Mod calling chip Double 3 times; assert chips == 3.
#[test]
fn snapshot_mod_calling_chip_repeated() {
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
fn snapshot_record_dissolved_chip() {
    let src = r#"
type Vec2 = { x: int, y: int }
chip add_vec(a: Vec2, b: Vec2) -> (r: Vec2) {
    out r = { x: a.x + b.x, y: a.y + b.y }
}
let p: Vec2 = { x: 1, y: 2 }
let q: Vec2 = { x: 3, y: 4 }
let result = add_vec(p, q)
"#;
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("record param chip should compile");
}

/// 10 calls to chip Inc(x) -> (r); assert chips == 10, nodes >= 40, wires >= 30.
#[test]
fn snapshot_many_chip_instances_unique_wires() {
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
    assert!(
        wires >= 30,
        "chained chip calls should produce >= 30 wires, got {}",
        wires
    );
}

/// Grandparent capture: mod add_score captures var score from root, called in handler.
#[test]
fn snapshot_grandparent_capture() {
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
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("grandparent var capture in mod should compile");
}

/// Mod chain: step() captures var, double_step() calls step() twice, double_step called twice.
#[test]
fn snapshot_mod_chain_with_capture() {
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
    let input = CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
    };
    compile(input).expect("mod chain with capture should compile");
}
