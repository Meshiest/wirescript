//! `for`-loop lowering: the shell node, its plane, and the entry gate.
//!
//! The range-arithmetic twins of the `shell_pin_literal` tests below live in
//! `lower::for_loop`'s own `mod tests`, which evaluates `normalize_range`'s
//! `PinExpr` tree with no lowering. The names match: these read the same
//! behaviours back off a compiled module, so a mis-lowered formula fails here.

use super::*;
use crate::ir::gate_class as gc;
use crate::test_support::{assert_code, assert_code_count, codes};

/// The single plane-shell node in the root module.
fn shell(r: &LowerResult) -> &crate::ir::Node {
    let mut found: Vec<&crate::ir::Node> = r
        .module
        .nodes
        .values()
        .filter(|n| gc::is_plane_shell(n.gate_class))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one loop shell, got {}",
        found.len()
    );
    found.pop().unwrap()
}

/// The constant value driving the shell's `port` input, or `None` when a
/// computed gate drives it. A constant reaches a pin either as a wired
/// `_Literal` gate or as component data baked onto the shell once
/// `inline_bare_scalar_literals` drops the wire; both count. Returns `i64`:
/// `ceil`/`max` are `float` builtins, so an `int` pin's constant can arrive as a
/// `Literal::Float`, and `ceil` never yields a fraction.
fn shell_pin_literal(r: &LowerResult, port: &str) -> Option<i64> {
    let want = WirePort::from_name(port);
    let sh = shell(r);
    let as_i64 = |lit: &crate::ir::Literal| match lit {
        crate::ir::Literal::Int(n) => Some(*n),
        crate::ir::Literal::Float(f) => Some(*f as i64),
        _ => None,
    };
    if let Some(lit) = sh.properties.get(&want.sym()) {
        return as_i64(lit);
    }
    let wire = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == sh.id && w.target.port == want)?;
    let src = r.module.nodes.get(&wire.source.node_id)?;
    if src.gate_class != gc::LITERAL {
        return None;
    }
    as_i64(src.properties.get(&crate::intern::sym::VALUE)?)
}

#[test]
fn the_body_lands_on_the_child_grid_with_its_entry_gate() {
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i } }");
    let child = only_chip(&r, "the loop body should produce one child module");
    assert!(
        child
            .nodes
            .values()
            .any(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR),
        "the entry gate must live INSIDE the plane"
    );
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR),
        "the entry gate must NOT be on the outer grid"
    );
}

#[test]
fn the_shell_sits_on_the_outer_exec_chain() {
    // Something after the loop must be driven by the shell's `Completed`, not
    // left dangling: `Completed` is the loop's continuation.
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { } t = 1 }");
    let sh = shell(&r);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == sh.id && w.source.port == WirePort::Completed),
        "nothing consumes the shell's Completed"
    );
}

#[test]
fn the_shell_takes_the_incoming_exec() {
    let r = compile_ok("in go: exec\non go { for i in 0..10 { } }");
    let sh = shell(&r);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == sh.id && w.target.port == WirePort::Exec),
        "the shell's Exec is unwired, so the loop never fires"
    );
    let _ = only_chip(&r, "even an empty loop body needs its plane");
}

#[test]
fn the_body_runs_off_the_entry_gates_body_pin() {
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { t = 1 } }");
    let child = only_chip(&r, "the loop body should produce one child module");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR)
        .expect("entry gate");
    assert!(
        child
            .wires
            .iter()
            .any(|w| w.source.node_id == entry.id && w.source.port == WirePort::Body),
        "nothing consumes the entry gate's Body, so the body never runs"
    );
}

#[test]
fn the_collection_source_drives_the_shells_ref_pin() {
    let r = compile("var arr: int[]\nin go: exec\non go { for v in arr { } }");
    let sh = shell(&r);
    let wire = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == sh.id && w.target.port == WirePort::ArrayVarRef)
        .expect("the shell's ArrayVarRef is unwired");
    assert_eq!(
        r.module.nodes[&wire.source.node_id].gate_class,
        gc::ARRAY_VAR,
        "the shell must take the array VARIABLE's ref, not a computed value"
    );
}

#[test]
fn as_run_binds_the_shell_status_outputs_on_the_outer_grid() {
    let r = compile_ok(
        "var bad: bool = false\nin go: exec\non go { let run = for i in 0..10 { } if run.aborted { bad = true } }",
    );
    let sh = shell(&r);
    // The abort read must consume the SHELL's pin, and must live on the outer
    // grid rather than inside the plane. Compared by `WirePort` rather than by
    // the Debug rendering of one: the variant is spelled `BAborted` while the
    // game-side port is `bAborted`, so a substring test on `{:?}` silently
    // never matches.
    let consumer = r
        .module
        .wires
        .iter()
        .find(|w| w.source.node_id == sh.id && w.source.port == WirePort::BAborted)
        .expect("`run.aborted` must read the shell's bAborted pin");
    assert!(
        r.module.nodes.contains_key(&consumer.target.node_id),
        "the `let run = for` read belongs on the outer grid, not inside the plane"
    );
}

#[test]
fn run_running_reads_the_shells_brunning_pin() {
    let r = compile_ok(
        "var busy: bool = false\nin go: exec\non go { let run = for i in 0..10 { } busy = run.running }",
    );
    let sh = shell(&r);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == sh.id && w.source.port == WirePort::BRunning),
        "`run.running` must read the shell's bRunning pin"
    );
}

#[test]
fn sliced_sets_the_shell_setting_only_when_asked() {
    let flag = |src: &str| {
        let r = compile_ok(src);
        shell(&r)
            .properties
            .get(&crate::intern::intern("bRunAcrossTicks"))
            .cloned()
    };
    assert_eq!(
        flag("var t: int = 0\nin go: exec\non go { for i in 0..3 { t += i } }"),
        None
    );
    assert_eq!(
        flag("var t: int = 0\nin go: exec\non go {\n  @sliced for i in 0..3 { t += i }\n}"),
        Some(crate::ir::Literal::Bool(true))
    );
    assert_eq!(
        flag(
            "var t: int = 0\nin go: exec\non go {\n  @sliced\n  let run = for i in 0..3 { t += i }\n  \
             if run.running { t = 0 }\n}"
        ),
        Some(crate::ir::Literal::Bool(true))
    );
}

#[test]
fn sliced_belongs_on_a_for_loop() {
    for src in [
        "var t: int = 0\nin go: exec\non go {\n  @sliced t = 1\n}",
        "var t: int = 0\nin go: exec\non go {\n  @sliced @nofold for i in 0..3 { t += i }\n}",
        "@sliced\nvar t: int = 0\n",
    ] {
        let parsed = parse(src, "test");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "expected an error for {src:?}"
        );
    }
}

/// `@label` names a loop's microchip, as it names a `chip`'s: a string, or a
/// constant expression, beside `@sliced` in either order and on `let run = for`.
#[test]
fn label_names_the_loop_chip() {
    let named = |src: &str| {
        let r = compile_ok(src);
        let sh = shell(&r);
        let label = match sh.properties.get(&*crate::intern::sym::NAME_LABEL) {
            Some(crate::ir::Literal::String(s)) => Some(s.clone()),
            _ => None,
        };
        (
            label,
            sh.properties
                .contains_key(&crate::intern::intern("bRunAcrossTicks")),
        )
    };
    let body = "var t: int = 0\nconst ROWS = \"Rows\"\nin go: exec\non go {\n";
    assert_eq!(
        named(&format!("{body}  for i in 0..3 {{ t += i }}\n}}")),
        (None, false)
    );
    assert_eq!(
        named(&format!(
            "{body}  @label(\"Spawner\") for i in 0..3 {{ t += i }}\n}}"
        )),
        (Some("Spawner".into()), false)
    );
    assert_eq!(
        named(&format!(
            "{body}  @label(ROWS) for i in 0..3 {{ t += i }}\n}}"
        )),
        (Some("Rows".into()), false)
    );
    assert_eq!(
        named(&format!(
            "{body}  @sliced @label(\"Tick\") for i in 0..3 {{ t += i }}\n}}"
        )),
        (Some("Tick".into()), true)
    );
    assert_eq!(
        named(&format!(
            "{body}  @label(\"Tick\")\n  @sliced\n  for i in 0..3 {{ t += i }}\n}}"
        )),
        (Some("Tick".into()), true)
    );
    assert_eq!(
        named(&format!(
            "{body}  @label(\"Run\")\n  let run = for i in 0..3 {{ t += i }}\n  if run.aborted {{ t = 0 }}\n}}"
        )),
        (Some("Run".into()), false)
    );
}

/// A loop's label must be a constant, and the annotations a loop chip has no
/// use for stay errors.
#[test]
fn a_loop_label_is_constant_and_other_annotations_are_refused() {
    let r = compile(
        "var t: int = 0\nvar name: string = \"x\"\nin go: exec\non go {\n  @label(name) for i in 0..3 { t += i }\n}",
    );
    assert!(
        has_error(&r, "WS040"),
        "a runtime `@label` on a loop: {:?}",
        r.diagnostics
    );
    for src in [
        "var t: int = 0\nin go: exec\non go {\n  @closed for i in 0..3 { t += i }\n}",
        "var t: int = 0\nin go: exec\non go {\n  @label(\"a\") @nofold for i in 0..3 { t += i }\n}",
        "var t: int = 0\nin go: exec\non go {\n  @left for i in 0..3 { t += i }\n}",
    ] {
        let parsed = parse(src, "test");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "expected an error for {src:?}"
        );
    }
}

#[test]
fn a_var_written_in_the_body_is_re_read_after_the_loop() {
    // A read taken before the loop cannot be reused after it: the body writes
    // `t` once per iteration, so the pre-loop value is stale.
    let r = compile(
        "var t: int = 0\nvar o: int = 0\nin go: exec\non go { o = t\n for i in 0..3 { t += 1 }\n o = t }",
    );
    assert_eq!(
        gate_count(&r, gc::VAR_GET),
        2,
        "the pre-loop read of `t` and a fresh one after the loop"
    );
}

#[test]
fn a_var_the_body_never_touches_keeps_its_pre_loop_read() {
    // The other side of the same rule: invalidating every var at a loop would
    // be safe but would re-read what provably cannot have changed.
    let r = compile(
        "var t: int = 0\nvar o: int = 0\nin go: exec\non go { o = t\n for i in 0..3 { }\n o = t }",
    );
    let reads = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_GET)
        .count();
    assert_eq!(
        reads, 1,
        "`t` is untouched by the body, so its pre-loop read still dominates"
    );
}

#[test]
fn a_body_read_is_a_gate_inside_the_plane() {
    // A `Var_Get` created before the loop latches once, on the outer chain;
    // reusing it inside would hand every iteration the pre-loop value.
    let r = compile(
        "var t: int = 0\nvar o: int = 0\nin go: exec\non go { o = t\n for i in 0..3 { o = t } }",
    );
    let child = only_chip(&r, "loop plane");
    assert!(
        child.nodes.values().any(|n| n.gate_class == gc::VAR_GET),
        "the body's read of `t` must be its own gate on the child grid"
    );
}

#[test]
fn nested_loops_nest_their_planes() {
    let r = compile(
        "var t: int = 0\nin go: exec\non go { for i in 0..3 { for j in 0..3 { t += 1 } } }",
    );
    let outer = only_chip(&r, "outer loop plane");
    assert!(
        outer.chips.len() == 1,
        "the inner loop's plane must be a child of the outer one, got {}",
        outer.chips.len()
    );
}

#[test]
fn a_var_declared_in_the_body_lives_inside_the_plane() {
    // A body opens its own scope, exactly like an `if` block, so a `var`
    // declared there is created while `current_anon_chip` is the shell and
    // lands on the child grid rather than the outer one.
    let r = compile_ok("in go: exec\non go { for i in 0..3 { var acc: int = 0\n acc = i } }");
    let child = only_chip(&r, "loop plane");
    assert!(
        child.nodes.values().any(|n| n.gate_class == gc::VAR),
        "the body-local `var` must be a gate inside the plane"
    );
}

#[test]
fn a_loop_inside_a_chip_stays_on_that_chips_grid() {
    let r = compile("var t: int = 0\nin go: exec\nchip { on go { for i in 0..3 { t += 1 } } }");
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| gc::is_plane_shell(n.gate_class)),
        "the shell belongs to the enclosing chip's grid, not the root"
    );
    let anon = only_chip(&r, "the anon chip");
    assert!(
        anon.nodes
            .values()
            .any(|n| gc::is_plane_shell(n.gate_class)),
        "the shell must sit on the enclosing chip's grid"
    );
}

#[test]
fn an_ignored_slot_binds_nothing() {
    let r = compile_ok(
        "var arr: int[]\nvar t: int = 0\nin go: exec\non go { for _, v in arr { t = v } }",
    );
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_FOR_EACH_ARRAY)
        .expect("entry gate");
    assert!(
        !child
            .wires
            .iter()
            .any(|w| w.source.node_id == entry.id && w.source.port == WirePort::Index),
        "`_` must leave its entry port unwired"
    );
    assert!(
        child
            .wires
            .iter()
            .any(|w| w.source.node_id == entry.id && w.source.port == WirePort::Value),
        "`v` must read the entry gate's Value slot"
    );
}

#[test]
fn a_single_array_binding_reads_value_not_index() {
    // The one exception to reading slots in order: `for v in arr` binds the
    // ELEMENT, the way `enumerate` reads.
    let r =
        compile("var arr: int[]\nvar t: int = 0\nin go: exec\non go { for v in arr { t = v } }");
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_FOR_EACH_ARRAY)
        .expect("entry gate");
    assert!(
        child
            .wires
            .iter()
            .any(|w| w.source.node_id == entry.id && w.source.port == WirePort::Value),
        "a single array binding must wire from Value"
    );
}

#[test]
fn a_runtime_bound_is_read_before_the_shell_fires() {
    // The bound's `Var_Get` is an exec-taking gate, so it advances the chain;
    // the shell must pick up from THERE, not from the chain head the
    // statement started on, or the read hangs off to one side and the shell
    // latches whatever the port defaults to.
    let r = compile(
        "var n: int = 0
in go: exec
on go { for i in 0..n { } }",
    );
    let sh = shell(&r);
    let exec_wire = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == sh.id && w.target.port == WirePort::Exec)
        .expect("the shell's Exec is unwired");
    assert_eq!(
        r.module.nodes[&exec_wire.source.node_id].gate_class,
        gc::VAR_GET,
        "the bound's read must chain into the shell"
    );
}

#[test]
fn a_for_with_no_exec_chain_emits_nothing() {
    // WS074 is typecheck's; lowering must then emit no shell at all rather
    // than a plane nothing can ever fire.
    let r = compile(
        "var t: int = 0

chip {
 for i in 0..3 { t = i }
}",
    );
    assert!(
        !r.module
            .nodes_deep()
            .any(|n| gc::is_plane_shell(n.gate_class)),
        "a `for` with no exec chain must emit no shell"
    );
    assert_code(&r.diagnostics, "WS074");
}

/// A loop binding with no lowering is reported exactly once, by typecheck,
/// with no WS071/WS076/WSP001 cascade; a record element builds no shell.
#[test]
fn an_unbindable_loop_element_is_one_diagnostic_and_no_shell() {
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "a record-typed array is one array gate per field, so there is no single \
             `ArrayVarRef` for the shell to take: binding its fields would wire the plane to nothing",
            "type Point = { x: int, y: int }\nvar pts: Point[]\nvar t: int = 0\nin go: exec\non go { for { x, y } in pts { t = x } }",
            "WS082",
            true,
        ),
        (
            "the whole-element form over the same record array",
            "type Point = { x: int, y: int }\nvar pts: Point[]\nvar t: int = 0\nin go: exec\non go { for p in pts { t = p.x } }",
            "WS082",
            true,
        ),
        (
            "a `{ }` over an array destructures its value, which is never a record; typecheck owns \
             the message and lowering must not repeat it at the same span",
            "var xs: int[]\nvar t: int = 0\nin go: exec\non go { for { x } in xs { t = 1 } }",
            "WS076",
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, code, no_shell) in rows {
        let r = compile(src);
        let got = codes(&r.diagnostics);
        if got != [*code] {
            failures.push(format!("{label}: codes {got:?}, want exactly [{code}]"));
        }
        if *no_shell
            && r.module
                .nodes_deep()
                .any(|n| gc::is_plane_shell(n.gate_class))
        {
            failures.push(format!("{label}: a shell was built"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Typecheck reports an `await` written inside a `for` body; the lowering
/// backstop covers only the ones that arrive by inlining, so the plainest
/// case must not print twice.
#[test]
fn an_await_written_in_a_loop_body_is_reported_once() {
    let r = compile("let done: exec\nin go: exec\non go { for i in 0..3 { await done } }");
    assert_code_count(&r.diagnostics, "WS079", 1);
}

/// The span of an inlined `await` is the mod's, so two loops calling one mod
/// give two identical diagnostics. Both must survive: the second call site is
/// broken too, and collapsing them would hide it.
#[test]
fn an_inlined_await_is_reported_at_every_call_site() {
    let r = compile(
        "let done: exec\nvar t: int = 0\nmod waiter() { await done\n t = t + 1 }\n\
         in go: exec\non go { for i in 0..3 { waiter() }\n for j in 0..2 { waiter() } }",
    );
    assert_code_count(&r.diagnostics, "WS079", 2);
}

/// A counted loop has no element to destructure, so `{ }` names the loop's
/// own slots: `index` (or an alias of it) reads the entry gate's Index port,
/// over a range and the `count` spelling alike.
#[test]
fn a_range_brace_binding_reads_the_entry_index() {
    let rows: &[(&str, &str)] = &[
        ("a range", "for { index } in 0..10 { t = index }"),
        (
            "the count spelling",
            "for { index } in count(5, from = 2) { t = index }",
        ),
        ("an aliased slot", "for { index: i } in 0..4 { t = i }"),
    ];
    let mut failures = Vec::new();
    for (label, body) in rows {
        let r = compile(&format!("var t: int = 0\nin go: exec\non go {{ {body} }}"));
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let child = only_chip(&r, "loop plane");
        let entry = child
            .nodes
            .values()
            .find(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR)
            .expect("entry gate");
        let reads: Vec<WirePort> = child
            .wires
            .iter()
            .filter(|w| w.source.node_id == entry.id)
            .map(|w| w.source.port)
            .collect();
        if !reads.contains(&WirePort::Index) {
            failures.push(format!(
                "{label}: the binding must wire from the entry's Index port, got {reads:?}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn a_range_brace_binding_takes_its_slots_type() {
    let r =
        compile_ok("var t: int = 0\nin go: exec\non go { for { index } in 0..4 { t = index } }");
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR)
        .expect("entry gate");
    assert_eq!(
        entry
            .ports
            .find_output(WirePort::Index.sym())
            .map(|p| &p.ty),
        Some(&crate::ir::Type::Int)
    );
}

#[test]
fn removed_range_slots_are_ws076() {
    // The CountFor entry has no Ordinal/bIsFirst/bIsLast outputs.
    for name in ["ordinal", "isFirst", "isLast"] {
        let r = compile(&format!(
            "var t: int = 0\nin go: exec\non go {{ for {{ index, {name} }} in 0..4 {{ t = index }} }}"
        ));
        assert_code(&r.diagnostics, "WS076");
    }
}

#[test]
fn an_unknown_range_slot_name_is_ws076() {
    let r = compile("var t: int = 0\nin go: exec\non go { for { nope } in 0..4 { t = 1 } }");
    assert_code_count(&r.diagnostics, "WS076", 1);
    let msg = &r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS076")
        .unwrap()
        .message;
    assert!(
        msg.contains("index"),
        "WS076 must list the valid slots, missing `index`: {msg}"
    );
}

#[test]
fn a_rest_pattern_on_a_range_is_ws076() {
    // `...` collects a record's leftover fields. A range has no record, and
    // binding it to "every slot not named" would invent a value with no wire.
    let r = compile("var t: int = 0\nin go: exec\non go { for { ...rest } in 0..4 { t = 1 } }");
    let ws076 = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS076")
        .unwrap_or_else(|| panic!("expected WS076: {:?}", r.diagnostics));
    assert!(
        ws076.message.contains("index"),
        "WS076 must name the slots a range binding may spell: {}",
        ws076.message
    );
}

/// Each constant range reaches the shell's pins as the values its own row
/// names, read back off the compiled module.
#[test]
fn a_constant_range_drives_the_shell_pins() {
    let rows: &[(&str, &str, &[(&str, i64)])] = &[
        (
            "bounds exclude the end",
            "3..7",
            &[("Count", 4), ("FirstIndex", 3)],
        ),
        ("an empty range is zero iterations", "5..5", &[("Count", 0)]),
        (
            "a backwards range is zero, not negative",
            "10..0",
            &[("Count", 0)],
        ),
        (
            "the count spelling drives the pins directly",
            "count(4, from = 10, step = 5)",
            &[("Count", 4), ("FirstIndex", 10), ("Step", 5)],
        ),
    ];
    let mut failures = Vec::new();
    for (label, range, pins) in rows {
        let r = compile(&format!("in go: exec\non go {{ for i in {range} {{ }} }}"));
        for (pin, want) in *pins {
            let got = shell_pin_literal(&r, pin);
            if got != Some(*want) {
                failures.push(format!("{label}: `{range}` {pin} = {got:?}, want {want}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn a_runtime_rev_still_counts_down_from_the_top() {
    // FirstIndex reaches Count through `CountPin` rather than rebuilding the
    // formula, so a runtime bound must still reach the pins as real gates.
    let r = compile_ok("var n: int = 7\nin go: exec\non go { for i in (0..n).rev() { } }");
    assert_eq!(
        shell_pin_literal(&r, "Count"),
        None,
        "a runtime Count is a gate"
    );
    assert_eq!(
        r.module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::CEIL)
            .count(),
        1,
        "the Count chain belongs to one pin, reused rather than rebuilt"
    );
}

#[test]
fn a_runtime_bound_becomes_real_gates_not_a_placeholder() {
    // `normalize_range` returns `PinExpr` rather than synthetic `Expr::BinOp`s: a
    // `BinOp` built after typecheck has no `op_resolutions` entry, so `lower_binop`
    // would substitute an `_Unsupported` placeholder and the save would do nothing.
    let r = compile_ok("var n: int = 0\nin go: exec\non go { for i in 0..n { } }");
    assert!(
        !has_gate(&r, gc::UNSUPPORTED),
        "a runtime bound must lower to real math gates"
    );
    assert_eq!(
        shell_pin_literal(&r, "Count"),
        None,
        "a runtime bound is computed, not a baked constant"
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Warning),
        "unexpected warnings: {:?}",
        r.diagnostics
    );
}

#[test]
fn a_runtime_step_divides_in_float_before_the_ceiling() {
    // The `* 1.0` widening survives LOWERING, not just `normalize_range`. Gate class,
    // a `Ceil` and no `_Unsupported` look the same either way (`MathDivide` is one
    // class, and only `(Int, Int)` truncates), so the port types `build_binop`
    // declared are what is asserted.
    let r = compile_ok("var n: int = 0\nin go: exec\non go { for i in (0..n).step(3) { } }");

    let ceil = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CEIL)
        .expect("the ceil-divide must reach a real Ceil gate");
    let feed = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == ceil.id && w.target.port == WirePort::Input)
        .expect("nothing drives the Ceil's Input");
    let divide = &r.module.nodes[&feed.source.node_id];
    assert_eq!(
        divide.gate_class, "BrickComponentType_Expr_MathDivide",
        "the Ceil must be fed by the ceil-divide itself"
    );
    assert_eq!(
        divide
            .ports
            .find_input(WirePort::InputA.sym())
            .map(|p| &p.ty),
        Some(&crate::ir::Type::Float),
        "the numerator must resolve as `float`; an `int` one selects the TRUNCATING \
         (Int, Int) divide, and `ceil` then has nothing to round up"
    );
    assert_eq!(
        divide
            .ports
            .find_output(WirePort::Output.sym())
            .map(|p| &p.ty),
        Some(&crate::ir::Type::Float),
        "a truncating divide would declare an `int` result"
    );

    // The widen itself: `span * 1.0` is what makes the numerator float-typed.
    let widen = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == divide.id && w.target.port == WirePort::InputA)
        .map(|w| &r.module.nodes[&w.source.node_id])
        .expect("nothing drives the divide's numerator");
    assert_eq!(
        widen
            .ports
            .find_output(WirePort::Output.sym())
            .map(|p| &p.ty),
        Some(&crate::ir::Type::Float),
        "the `* 1.0` widening multiply must produce a float"
    );
    assert!(
        !has_gate(&r, gc::UNSUPPORTED),
        "the widening multiply must resolve to a real Math gate"
    );
}

#[test]
fn break_emits_a_callable_break_inside_the_plane() {
    let r = compile(
        "var t: int = 0\nin go: exec\non go { for i in 0..10 { if i > 3 { break } t += i } }",
    );
    let child = only_chip(&r, "loop body");
    assert!(
        child
            .nodes
            .values()
            .any(|n| n.gate_class == gc::CALLABLE_BREAK),
        "break must lower to a CallableBreak inside the plane"
    );
    assert!(
        crate::catalog::default_catalog()
            .find_by_class(gc::CALLABLE_BREAK)
            .is_some(),
        "CallableBreak has no brick asset in the catalog, so emit would silently \
         fall back to a reroute node"
    );
}

#[test]
fn a_statement_after_break_in_the_same_body_emits_no_gate() {
    // `Exec_CallableBreak` only stops SUBSEQUENT iterations, so `t += i` is
    // reachable only through its ExecOut; leaving that unwired means the
    // statement never lowers to a gate at all, proving break cut it off.
    let with = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { break\n t += i } }");
    let without = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i } }");
    let increments = |r: &LowerResult| {
        only_chip(r, "loop body")
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::VAR_INCREMENT)
            .count()
    };
    assert_eq!(
        increments(&with),
        0,
        "a statement after break in the same body must emit no gate"
    );
    assert_eq!(
        increments(&without),
        1,
        "the control case must still emit the increment"
    );
}

/// A statement after `break` or `continue` in the same block is dead: WS081
/// names that cause once, and WS058 does not also fire. An assignment reached
/// from a pure position with no `break` keeps reporting WS058.
#[test]
fn a_statement_after_break_or_continue_is_ws081_not_ws058() {
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "after `break` in the loop body",
            "var t: int = 0\nin go: exec\non go { for i in 0..3 { break\n t = t + 1 } }",
            "WS081",
            "WS058",
            "`break`",
        ),
        (
            "after `break` in the same `if` block, not the loop's own body",
            "var t: int = 0\nin go: exec\non go { for i in 0..5 { if i == 2 { break\n t = t + 1 } } }",
            "WS081",
            "WS058",
            "`break`",
        ),
        (
            "after `continue`",
            "var t: int = 0\nin go: exec\non go { for i in 0..3 { continue\n t = t + 1 } }",
            "WS081",
            "WS058",
            "`continue`",
        ),
        (
            "no `break`: an ordinary pure-position assignment (the diagnostics.md WS058 trigger)",
            "var x: int = 0\nmod f() { x = x + 1 }\nlet d = f()",
            "WS058",
            "WS081",
            "",
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want, absent, says) in rows {
        let r = compile(src);
        let found: Vec<&crate::diagnostic::Diagnostic> =
            r.diagnostics.iter().filter(|d| d.code == *want).collect();
        if found.len() != 1 || !found[0].message.contains(says) {
            failures.push(format!(
                "{label}: want one {want} naming {says:?}, got {:?}",
                r.diagnostics
            ));
        }
        if r.diagnostics.iter().any(|d| d.code == *absent) {
            failures.push(format!("{label}: unexpected {absent}: {:?}", r.diagnostics));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn continue_emits_no_gate() {
    // `continue` only ends the sweep's exec chain: the plane gains no gate,
    // and in particular no CallableBreak, which would stop later iterations.
    let with =
        compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i\n continue } }");
    let without = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i } }");
    assert_no_errors(&with);
    let body = only_chip(&with, "loop body");
    assert!(
        !body
            .nodes
            .values()
            .any(|n| n.gate_class == gc::CALLABLE_BREAK),
        "continue must not lower to a CallableBreak"
    );
    assert_eq!(
        body.nodes.len(),
        only_chip(&without, "loop body").nodes.len(),
        "a trailing continue must add no gate to the plane"
    );
}

#[test]
fn a_statement_after_continue_in_the_same_body_emits_no_gate() {
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { continue\n t += i } }");
    let increments = only_chip(&r, "loop body")
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_INCREMENT)
        .count();
    assert_eq!(
        increments, 0,
        "a statement after continue in the same body must emit no gate"
    );
}

#[test]
fn a_statement_after_an_if_with_continue_runs_off_the_other_branch() {
    // `if i == 3 { continue }` ends the then-branch's chain, so the increment
    // after the `if` hangs off the Branch gate's false side alone.
    let r = compile_ok(
        "var t: int = 0\nin go: exec\non go { for i in 0..10 { if i == 3 { continue }\n t += i } }",
    );
    let body = only_chip(&r, "loop body");
    let inc = body
        .nodes
        .values()
        .find(|n| n.gate_class == gc::VAR_INCREMENT)
        .expect("the increment after the `if` must still lower");
    let feeds: Vec<_> = body
        .wires
        .iter()
        .filter(|w| w.target.node_id == inc.id && w.target.port == WirePort::Exec)
        .collect();
    assert_eq!(
        feeds.len(),
        1,
        "the increment must run off exactly one exec wire"
    );
    let src = &body.nodes[&feeds[0].source.node_id];
    assert_eq!(
        src.gate_class,
        gc::BRANCH,
        "the increment must run off the `if`'s Branch gate"
    );
    assert_eq!(
        feeds[0].source.port,
        WirePort::ExecOutB,
        "the increment must run off the false side"
    );
}

/// A `for` over an array or map whose elements are enums (with or without a
/// payload) or tuples binds each element whole: the loop runs off one of the
/// element's columns and reads every column at the element's index or key.
#[test]
fn a_for_loop_over_columnar_elements_binds_each_element() {
    let cases = [
        (
            "enum E { A, B }\nvar es: E[] = []",
            "es.push(E.B)\n for x in es { match x { A => { r += 1 } B => { r += 10 } } }\n for i, x in es { if x is E.B { r += i } }",
            gc::MICROCHIP_FOR_EACH_ARRAY,
            gc::ARRAY_GET,
            (2, 2),
        ),
        (
            "enum E { Empty, Circle(int) }\nvar es: E[] = []",
            "es.push(E.Circle(3))\n for x in es { match x { Empty => { r += 1 } Circle(c) => { r += c } } }",
            gc::MICROCHIP_FOR_EACH_ARRAY,
            gc::ARRAY_GET,
            (1, 2),
        ),
        (
            "var ts: (int, int)[] = []",
            "ts.push((1, 2))\n for t in ts { r += t.0 + t.1 }",
            gc::MICROCHIP_FOR_EACH_ARRAY,
            gc::ARRAY_GET,
            (1, 2),
        ),
        (
            "enum E { A, B }\nvar m: Map<int, E>",
            "m.set(1, E.B)\n for k, v in m { match v { A => { r += 1 } B => { r += k } } }",
            gc::MICROCHIP_FOR_EACH_MAP,
            gc::MAP_GET,
            (1, 1),
        ),
    ];
    // (shells, column reads inside the planes)
    for (decls, body, shell, read, (shell_count, read_count)) in cases {
        let src = format!("{decls}\nvar r: int = 0\nin go: exec\non go {{\n {body}\n}}");
        let r = compile_ok(&src);
        assert!(!has_unsupported(&r.module), "{src}");
        let shells = nodes_of(&r.module, shell);
        assert_eq!(shells.len(), shell_count, "{src}");
        // The driver pin takes a column's ref, and the body reads the columns.
        for s in &shells {
            let driven = r.module.wires.iter().any(|w| {
                w.target.node_id == s.id
                    && matches!(w.target.port, WirePort::ArrayVarRef | WirePort::MapVarRef)
            });
            assert!(driven, "{src}: the loop's driver pin is wired");
        }
        let reads = nodes_of(&r.module, read)
            .into_iter()
            .filter(|n| !r.module.nodes.contains_key(&n.id))
            .count();
        assert_eq!(
            reads, read_count,
            "{src}: the body reads the element's columns inside the plane"
        );
    }
}

/// Each binding of a columnar element reads ITS column: a tuple's `.0` and
/// `.1`, an enum's tag and each payload slot. Traced from every store in the
/// loop body back through the plane's pins to the storage it reads.
#[test]
fn a_columnar_element_reads_each_field_from_its_own_column() {
    let r = compile_ok(
        "enum E { Empty, Box(int, int) }\nin go: exec\nvar ts: (int, string)[] = []\nvar es: E[] = []\n\
         var a: int = 0\nvar b: string = \"\"\nvar c: int = 0\nvar d: int = 0\nvar g: int = 0\n\
         on go {\n  for t in ts {\n    a = t.0\n    b = t.1\n  }\n  for x in es {\n    match x { Empty => { g = 1 } Box(p, q) => { c = p\n      d = q } }\n  }\n}",
    );
    let nodes: HashMap<crate::ir::NodeId, &crate::ir::Node> =
        r.module.nodes_deep().map(|n| (n.id, n)).collect();
    let wires: Vec<crate::ir::Wire> = r.module.wires_deep().copied().collect();
    // The node driving `port` of `id`, followed back through plane pins.
    let source = |id: crate::ir::NodeId, port: WirePort| -> Option<crate::ir::NodeId> {
        let mut at = wires
            .iter()
            .find(|w| w.target.node_id == id && w.target.port == port)?
            .source
            .node_id;
        while let Some(w) = wires.iter().find(|w| {
            w.target.node_id == at
                && w.target.port == WirePort::RerInput
                && nodes
                    .get(&at)
                    .is_some_and(|n| n.kind != crate::ir::NodeKind::Output)
        }) {
            at = w.source.node_id;
        }
        Some(at)
    };
    let label =
        |id: crate::ir::NodeId| match nodes[&id].properties.get(&crate::intern::sym::NAME_LABEL) {
            Some(crate::ir::Literal::String(s)) => s.clone(),
            _ => format!("{}#{id}", nodes[&id].gate_class),
        };
    // Each store's target var and the column its value is read from.
    let mut read: Vec<(String, String)> = Vec::new();
    for (id, n) in &nodes {
        if n.gate_class != gc::VAR_SET {
            continue;
        }
        let (Some(var), Some(value)) =
            (source(*id, WirePort::VarRef), source(*id, WirePort::Value))
        else {
            continue;
        };
        if nodes[&value].gate_class != gc::ARRAY_GET {
            continue;
        }
        let column = source(value, WirePort::ArrayVarRef).expect("the read's column");
        read.push((label(var), label(column)));
    }
    read.sort();
    let tag_read = nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_GET)
        .any(|n| {
            let column = source(n.id, WirePort::ArrayVarRef).map(label);
            column.as_deref() == Some("es.__disc")
        });
    assert!(tag_read, "the match reads the tag column");
    assert_eq!(
        read,
        vec![
            ("a".to_string(), "ts.0".to_string()),
            ("b".to_string(), "ts.1".to_string()),
            ("c".to_string(), "es.__Box_0".to_string()),
            ("d".to_string(), "es.__Box_1".to_string()),
        ]
    );
}
