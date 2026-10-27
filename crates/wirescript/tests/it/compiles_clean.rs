//! Programs that must compile to a real circuit in both fold modes: no error,
//! no `_Unsupported` placeholder gate anywhere in the tree, and a `.brz` with
//! the format's magic. A bare `Ok` also passes a program that lowers half its
//! statements to placeholders.

use crate::common;

use wirescript::ir::Module;
use wirescript::{CompileInput, FoldMode, Severity};

struct Case {
    /// What the program exercises.
    label: &'static str,
    src: &'static str,
    /// Typecheck codes it must report.
    has: &'static [&'static str],
    /// Typecheck codes it must not report.
    lacks: &'static [&'static str],
}

const fn ok(label: &'static str, src: &'static str) -> Case {
    Case {
        label,
        src,
        has: &[],
        lacks: &[],
    }
}

const CASES: &[Case] = &[
    // An empty program still emits the outer chip.
    ok("an empty program", ""),
    // --- await
    ok(
        "a bare await",
        "in start: exec\nin done: exec\non start {\n  await done\n}",
    ),
    ok(
        "let x = await v on signal",
        "in start: exec\nin signal: exec\nvar value: int = 0\non start {\n  let x = await value on signal\n}",
    ),
    ok(
        "an await race",
        "in start: exec\nin a: exec\nin b: exec\non start {\n  await a || b\n}",
    ),
    ok(
        "sequential awaits",
        "in start: exec\nin step1: exec\nin step2: exec\nvar count: int = 0\non start {\n  count = 1\n  await step1\n  count = 2\n  await step2\n  count = 3\n}",
    ),
    ok(
        "an await inside an if",
        "in start: exec\nin done: exec\nvar flag: bool = true\non start {\n  if flag {\n    await done\n    flag = false\n  }\n}",
    ),
    // `let foo: int = await CustomEvent("c")` captures the event's DataOut1,
    // typed by the annotation, so no WS055.
    Case {
        label: "a typed CustomEvent capture",
        src: "in go: exec\nvar x: int = 0\non go { let foo: int = await CustomEvent(\"c\")\n  x = foo }",
        has: &[],
        lacks: &["WS055"],
    },
    // Without an annotation the capture still works but warns WS055 (the wire
    // defaults to float).
    Case {
        label: "an untyped CustomEvent capture",
        src: "in go: exec\nvar x: int = 0\non go { let foo = await CustomEvent(\"c\")\n  x = foo }",
        has: &["WS055"],
        lacks: &[],
    },
    // `let (p, t) = await CustomEvent("c")` captures data outputs positionally
    // (p = DataOut1, t = DataOut2); untyped, so WS055.
    Case {
        label: "a positional tuple CustomEvent capture",
        src: "in go: exec\nvar a: int = 0\nvar b: int = 0\non go { let (p, t) = await CustomEvent(\"c\")\n  a = p\n  b = t }",
        has: &["WS055"],
        lacks: &[],
    },
    // --- local exec signals
    ok(
        "a single emit to a local exec",
        "let done: exec\nin start: exec\non start { emit done }",
    ),
    ok(
        "multiple emit sites union into a local exec",
        "let done: exec\nin a: exec\nin b: exec\non a { emit done }\non b { emit done }",
    ),
    ok(
        "awaiting a local exec",
        "let done: exec\nin start: exec\nin trigger: exec\non trigger { emit done }\non start {\n  await done\n}",
    ),
    ok(
        "an await race over local execs",
        "let a: exec\nlet b: exec\nin start: exec\nin t1: exec\nin t2: exec\non t1 { emit a }\non t2 { emit b }\non start { await a || b }",
    ),
    // --- emit target = expr
    Case {
        label: "emit with a value in exec context",
        src: "out result: int\nin start: exec\non start { emit result = 42 }",
        has: &[],
        lacks: &["WS007"],
    },
    Case {
        label: "emit with a value in a chip body (pure context)",
        src: "chip Foo(x: int) -> (status: int) { emit status = x }",
        has: &[],
        lacks: &["WS007"],
    },
    ok(
        "emit with an expression value",
        "out r: int\nin s: exec\nvar x: int = 0\non s { emit r = x + 1 }",
    ),
    // --- constant folding through emit: vector/rotator/quat/color vars carry
    // their folded InitialValue through brz serialization (`WireValue`
    // alternatives Vector/Rotator/Quat/LinearColor).
    ok(
        "constant-folded constructor vars",
        "var v = Vec(1.0, 2.0, 3.0)\nvar rot = Rotation(0.0, 90.0, 0.0)\nvar tint = Color(1.0, 0.5, 0.0)\nvar q: quat\nout o = v\nin start: exec\non start {\n  v = Vec(4.0, 5.0, 6.0)\n  tint = Color(0.0, 1.0, 0.0, 0.5)\n}",
    ),
    ok(
        "a vector array initializer",
        "var pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]\nin start: exec\non start { pts.push(Vec(7.0, 8.0, 9.0)) }",
    ),
    // rotator[]/quat[]/color[] arrays back onto the game's typed array elements
    // (`Rotator` / `Quat` / `LinearColor` dynamic arrays).
    ok(
        "rotator, color and quat arrays",
        "var rots: rotator[] = [Rotation(0.0, 90.0, 0.0)]\nvar tints: color[] = [Color(1.0, 0.0, 0.0), Color(0.0, 0.0, 1.0, 0.5)]\nvar quats: quat[]\nvar n: int = 0\nin start: exec\non start { n = rots.length() }",
    ),
    // --- arrays by reference
    ok(
        "an array bound to a let",
        "var data: int[]\nlet alias = data",
    ),
    ok(
        "an array passed to a chip",
        "var buf: int[]\nchip Render(fb: int[]) -> () {\n  in run: exec\n  on run { fb.push(1) }\n}\nin start: exec\non start { Render(buf) }",
    ),
    ok(
        "an array in a record",
        "var items: int[]\ntype State = { data: int[] }\nlet state: State = { data: items }",
    ),
    ok(
        "an array bound to an out port",
        "var framebuf: int[]\nout display: int[] = framebuf\nin player: character\non player { framebuf.push(42) }",
    ),
    // --- chips and mods
    ok(
        "a chip called once with an output port",
        "chip Add(a: int, b: int) -> (r: int) { out r = a + b }\nlet res = Add(1, 2)\nout result = res.r\n",
    ),
    ok(
        "nested chip calls: Inner called inside Wrapper, Wrapper called twice",
        "chip Inner(x: int) -> (r: int) { out r = x + 1 }\nmod Wrapper(v: int) -> (result: int) {\n    let a = Inner(v)\n    let b = Inner(a.r)\n    return b.r\n}\nlet w1 = Wrapper(10)\nlet w2 = Wrapper(20)\nout total = w1 + w2\n",
    ),
    ok(
        "a mod capturing an array declared after it",
        "mod save(val: int) {\n    log.push(val)\n}\nvar log: int[]\nin tick: exec\non tick {\n    save(42)\n}\n",
    ),
    ok(
        "a buffer capture in pure context",
        "in current: int\nbuffer prev_val = current\nout changed = if current != prev_val then true else false\n",
    ),
    ok(
        "an exec chain in mods: two mods each with exec statements, called in a handler",
        "var a: int = 0\nvar b: int = 0\nmod reset_all() {\n    a = 0\n    b = 0\n}\nmod set_both(x: int) {\n    a = x\n    b = x * 2\n}\nin tick: exec\non tick {\n    reset_all()\n    set_both(5)\n}\nout sum = a + b\n",
    ),
    ok(
        "record params dissolve into individual chip ports",
        "type Vec2 = { x: int, y: int }\nchip add_vec(a: Vec2, b: Vec2) -> (r: Vec2) {\n    out r = { x: a.x + b.x, y: a.y + b.y }\n}\nlet p: Vec2 = { x: 1, y: 2 }\nlet q: Vec2 = { x: 3, y: 4 }\nlet result = add_vec(p, q)\n",
    ),
    ok(
        "a mod capturing a root var, called in a handler",
        "var score: int = 0\nmod add_score(pts: int) {\n    score = score + pts\n}\nin tick: exec\non tick {\n    add_score(10)\n}\nout total = score\n",
    ),
    ok(
        "a mod chain with capture: double_step calls step twice, called twice",
        "var counter: int = 0\nmod step() {\n    counter = counter + 1\n}\nmod double_step() {\n    step()\n    step()\n}\nin tick: exec\non tick {\n    double_step()\n    double_step()\n}\nout count = counter\n",
    ),
];

const MODES: [(FoldMode, &str); 2] = [
    (FoldMode::ForceOff, "fold off"),
    (FoldMode::Auto, "fold auto"),
];

fn placeholders(m: &Module) -> usize {
    let mut n = 0;
    m.walk(&mut |_, sub| {
        n += sub
            .nodes
            .values()
            .filter(|x| x.gate_class == "_Unsupported")
            .count();
    });
    n
}

/// Every way `case` falls short, one line each.
fn case_failures(case: &Case) -> Vec<String> {
    let mut out = Vec::new();
    for (mode, mode_label) in MODES {
        match wirescript::compile(CompileInput::new(case.src, "test.ws").fold_mode(mode)) {
            Ok(r) => {
                let errors: Vec<_> = r
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .collect();
                if !errors.is_empty() {
                    out.push(format!("{mode_label}: errors {errors:?}"));
                }
                if !r.brz.starts_with(b"BRZ\0") {
                    out.push(format!(
                        "{mode_label}: output is not a .brz ({} bytes)",
                        r.brz.len()
                    ));
                }
            }
            Err(e) => out.push(format!("{mode_label}: {e}")),
        }
        let (_, _, lowered) = common::run_stages(case.src, "test.ws", mode);
        let n = placeholders(&lowered.module);
        if n > 0 {
            out.push(format!("{mode_label}: lowered {n} placeholder gate(s)"));
        }
    }
    let codes = common::diag_codes(case.src);
    for code in case.has.iter().filter(|c| !codes.iter().any(|g| g == **c)) {
        out.push(format!("must report {code}, got {codes:?}"));
    }
    for code in case.lacks.iter().filter(|c| codes.iter().any(|g| g == **c)) {
        out.push(format!("must not report {code}, got {codes:?}"));
    }
    out
}

#[test]
fn every_program_compiles_to_a_real_circuit() {
    let failures: Vec<String> = CASES
        .iter()
        .flat_map(|case| {
            case_failures(case)
                .into_iter()
                .map(move |f| format!("{}: {f}", case.label))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
