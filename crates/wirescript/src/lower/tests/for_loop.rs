//! `for`-loop lowering: the shell node, its plane, and the entry gate.
//!
//! The range-arithmetic twins of the `shell_pin_literal` tests below live in
//! `lower::for_loop`'s own `mod tests`, which evaluates `normalize_range`'s
//! `PinExpr` tree with no lowering involved. Same names on purpose: these are
//! the same behaviours read back off a compiled module, so a formula that is
//! right in the abstract but mis-lowered fails here and only here.

use super::*;
use crate::ir::gate_class as gc;

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
/// computed gate drives it.
///
/// Two shapes count as constant, because a literal reaches a pin either way:
/// a `_Literal` gate wired in, or the same value baked onto the shell node as
/// component data once `inline_bare_scalar_literals` has dropped the wire.
/// Reading only the wire would call an inlined pin "computed".
///
/// Returns `i64`: `Count` and a reversed range's `FirstIndex` fold through
/// `ceil`/`max`, which are `float` builtins, so the constant can arrive as a
/// `Literal::Float` even though the pin is `int`. `ceil` never yields a
/// fraction, so the narrowing is lossless.
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
fn a_range_loop_emits_a_count_for_shell() {
    let r = compile("in go: exec\non go { for i in 0..10 { } }");
    assert_eq!(shell(&r).gate_class, gc::MICROCHIP_COUNT_FOR);
}

#[test]
fn an_array_loop_emits_a_for_each_array_shell() {
    let r = compile("var arr: int[]\nin go: exec\non go { for v in arr { } }");
    assert_eq!(shell(&r).gate_class, gc::MICROCHIP_FOR_EACH_ARRAY);
}

#[test]
fn a_map_loop_emits_a_for_each_map_shell() {
    let r = compile("var m: Map<string, int>\nin go: exec\non go { for k, v in m { } }");
    assert_eq!(shell(&r).gate_class, gc::MICROCHIP_FOR_EACH_MAP);
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
    let r = compile("in go: exec\non go { for i in 0..10 { } }");
    let sh = shell(&r);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == sh.id
                && w.target.port == WirePort::Exec),
        "the shell's Exec is unwired, so the loop never fires"
    );
}

#[test]
fn an_empty_body_still_produces_a_plane() {
    let r = compile("in go: exec\non go { for i in 0..10 { } }");
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
        child.wires.iter().any(|w| w.source.node_id == entry.id
            && w.source.port == WirePort::Body),
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
        .find(|w| {
            w.target.node_id == sh.id
                && w.target.port == WirePort::ArrayVarRef
        })
        .expect("the shell's ArrayVarRef is unwired");
    assert_eq!(
        r.module.nodes[&wire.source.node_id].gate_class,
        gc::ARRAY_VAR,
        "the shell must take the array VARIABLE's ref, not a computed value"
    );
}

#[test]
fn as_run_binds_the_shell_status_outputs_on_the_outer_grid() {
    let r = compile(
        "var bad: bool = false\nin go: exec\non go { for i in 0..10 as run { } if run.aborted { bad = true } }",
    );
    assert_no_errors(&r);
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
        "the `as run` read belongs on the outer grid, not inside the plane"
    );
}

#[test]
fn a_var_written_in_the_body_is_re_read_after_the_loop() {
    // A read taken before the loop cannot be reused after it: the body writes
    // `t` once per iteration, so the pre-loop value is stale.
    let r = compile(
        "var t: int = 0\nvar o: int = 0\nin go: exec\non go { o = t\n for i in 0..3 { t += 1 }\n o = t }",
    );
    let reads = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_GET)
        .count();
    assert!(
        reads >= 2,
        "expected a fresh read of `t` after the loop, got {reads}"
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
        child
            .nodes
            .values()
            .any(|n| n.gate_class == gc::VAR_GET),
        "the body's read of `t` must be its own gate on the child grid"
    );
}

#[test]
fn nested_loops_nest_their_planes() {
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..3 { for j in 0..3 { t += 1 } } }");
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
    let r = compile("in go: exec\non go { for i in 0..3 { var acc: int = 0\n acc = i } }");
    assert_no_errors(&r);
    let child = only_chip(&r, "loop plane");
    assert!(
        child
            .nodes
            .values()
            .any(|n| n.gate_class == gc::VAR),
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
    let r = compile("var arr: int[]\nvar t: int = 0\nin go: exec\non go { for _, v in arr { t = v } }");
    assert_no_errors(&r);
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_FOR_EACH_ARRAY)
        .expect("entry gate");
    assert!(
        !child.wires.iter().any(|w| w.source.node_id == entry.id
            && w.source.port == WirePort::Index),
        "`_` must leave its entry port unwired"
    );
    assert!(
        child.wires.iter().any(|w| w.source.node_id == entry.id
            && w.source.port == WirePort::Value),
        "`v` must read the entry gate's Value slot"
    );
}

#[test]
fn a_single_array_binding_reads_value_not_index() {
    // The one exception to reading slots in order: `for v in arr` binds the
    // ELEMENT, the way `enumerate` reads.
    let r = compile("var arr: int[]\nvar t: int = 0\nin go: exec\non go { for v in arr { t = v } }");
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_FOR_EACH_ARRAY)
        .expect("entry gate");
    assert!(
        child.wires.iter().any(|w| w.source.node_id == entry.id
            && w.source.port == WirePort::Value),
        "a single array binding must wire from Value"
    );
}

#[test]
fn a_runtime_bound_is_read_before_the_shell_fires() {
    // The bound's `Var_Get` is an exec-taking gate, so it advances the chain;
    // the shell must pick up from THERE, not from the chain head the
    // statement started on, or the read hangs off to one side and the shell
    // latches whatever the port defaults to.
    let r = compile("var n: int = 0
in go: exec
on go { for i in 0..n { } }");
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
    let r = compile("var t: int = 0

chip {
 for i in 0..3 { t = i }
}");
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| gc::is_plane_shell(n.gate_class)),
        "a `for` with no exec chain must emit no shell"
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS074"),
        "expected WS074: {:?}",
        r.diagnostics
    );
}

#[test]
fn a_record_element_binding_is_rejected_rather_than_half_wired() {
    // A record-typed array is stored as one array gate per field, so there is
    // no single `ArrayVarRef` for the shell to take. Binding the fields would
    // produce a plane wired to nothing; say so instead, and say it exactly
    // once - typecheck catches this before lowering ever builds a shell, so
    // no WS071/WS076/WSP001 cascade follows it.
    let r = compile(
        "type Point = { x: int, y: int }
var pts: Point[]
var t: int = 0
in go: exec
on go { for { x, y } in pts { t = x } }",
    );
    assert_eq!(
        r.diagnostics.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(),
        vec!["WS082"],
        "expected exactly one WS082 for a record-element binding: {:?}",
        r.diagnostics
    );
    assert!(
        !r.module.nodes.values().any(|n| gc::is_plane_shell(n.gate_class)),
        "a record-element source must build no shell at all"
    );
}

/// The whole-element sibling of the destructured form above: `for p in pts`
/// over the same record array must report the same single WS082.
#[test]
fn a_record_element_whole_binding_is_also_one_ws082() {
    let r = compile(
        "type Point = { x: int, y: int }
var pts: Point[]
var t: int = 0
in go: exec
on go { for p in pts { t = p.x } }",
    );
    assert_eq!(
        r.diagnostics.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(),
        vec!["WS082"],
        "expected exactly one WS082 for a record-element binding: {:?}",
        r.diagnostics
    );
}

/// A `{ }` over an array or map destructures the slot's value, and no slot of
/// either kind is ever a record, so the shape is refused. Typecheck owns that
/// message; lowering must not repeat it at the same span.
#[test]
fn a_brace_binding_on_a_non_record_element_is_reported_once() {
    let r = compile(
        "var xs: int[]\nvar t: int = 0\nin go: exec\non go { for { x } in xs { t = 1 } }",
    );
    assert_eq!(
        r.diagnostics.iter().map(|d| d.code.as_str()).collect::<Vec<_>>(),
        vec!["WS076"],
        "expected exactly one WS076: {:?}",
        r.diagnostics
    );
}

/// Typecheck reports an `await` written inside a `for` body; the lowering
/// backstop covers only the ones that arrive by inlining, so the plainest
/// case must not print twice.
#[test]
fn an_await_written_in_a_loop_body_is_reported_once() {
    let r = compile("let done: exec\nin go: exec\non go { for i in 0..3 { await done } }");
    assert_eq!(
        r.diagnostics
            .iter()
            .filter(|d| d.code == "WS079")
            .count(),
        1,
        "expected one WS079: {:?}",
        r.diagnostics
    );
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
    assert_eq!(
        r.diagnostics
            .iter()
            .filter(|d| d.code == "WS079")
            .count(),
        2,
        "expected one WS079 per call site: {:?}",
        r.diagnostics
    );
}

#[test]
fn a_range_brace_binding_names_the_entry_ports() {
    // A counted loop has no element to destructure, so `{ }` names the
    // loop's own slots. It is the only way to reach `isLast` without writing
    // out the positional bindings before it.
    let r = compile(
        "var t: int = 0\nvar done: bool = false\nin go: exec\non go { for { index, isLast } in 0..10 { t = index\n done = isLast } }",
    );
    assert_no_errors(&r);
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
    assert!(
        reads.contains(&WirePort::Index),
        "`index` must wire from the entry gate's Index port, got {reads:?}"
    );
    assert!(
        reads.contains(&WirePort::BIsLast),
        "`isLast` must wire from the entry gate's bIsLast port, got {reads:?}"
    );
    assert!(
        !reads.contains(&WirePort::Ordinal) && !reads.contains(&WirePort::BIsFirst),
        "only the named slots should be read, got {reads:?}"
    );
}

#[test]
fn a_count_spelling_takes_a_brace_binding_too() {
    let r = compile(
        "var t: int = 0\nvar o: int = 0\nin go: exec\non go { for { index, ordinal } in count(5, from = 2) { t = index\n o = ordinal } }",
    );
    assert_no_errors(&r);
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
    assert!(
        reads.contains(&WirePort::Index) && reads.contains(&WirePort::Ordinal),
        "both named slots must wire from the entry gate, got {reads:?}"
    );
}

#[test]
fn a_range_brace_binding_takes_its_slots_type() {
    // `isFirst` is a `bool` slot; binding it as anything else would wire a
    // bool port into an int consumer.
    let r = compile(
        "var b: bool = false\nin go: exec\non go { for { isFirst } in 0..4 { b = isFirst } }",
    );
    assert_no_errors(&r);
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR)
        .expect("entry gate");
    assert_eq!(
        entry
            .ports
            .find_output(WirePort::BIsFirst.sym())
            .map(|p| &p.ty),
        Some(&crate::ir::Type::Bool)
    );
    assert!(
        child
            .wires
            .iter()
            .any(|w| w.source.node_id == entry.id && w.source.port == WirePort::BIsFirst),
        "`isFirst` must read the bIsFirst slot"
    );
}

#[test]
fn a_range_brace_binding_can_alias() {
    let r = compile("var t: int = 0\nin go: exec\non go { for { index: i } in 0..4 { t = i } }");
    assert_no_errors(&r);
    let child = only_chip(&r, "loop plane");
    let entry = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::CALLABLE_ENTRY_COUNT_FOR)
        .expect("entry gate");
    assert!(
        child
            .wires
            .iter()
            .any(|w| w.source.node_id == entry.id && w.source.port == WirePort::Index),
        "the aliased name must still read the Index slot"
    );
}

#[test]
fn an_unknown_range_slot_name_is_ws076() {
    let r = compile("var t: int = 0\nin go: exec\non go { for { nope } in 0..4 { t = 1 } }");
    let ws076: Vec<&crate::diagnostic::Diagnostic> =
        r.diagnostics.iter().filter(|d| d.code == "WS076").collect();
    assert_eq!(ws076.len(), 1, "expected one WS076, got {:?}", r.diagnostics);
    let msg = &ws076[0].message;
    for label in ["index", "ordinal", "isFirst", "isLast"] {
        assert!(
            msg.contains(label),
            "WS076 must list the valid slots, missing `{label}`: {msg}"
        );
    }
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
        ws076.message.contains("isLast"),
        "WS076 must name the slots a range binding may spell: {}",
        ws076.message
    );
}

#[test]
fn zero_to_n_is_count_n_from_zero_step_one() {
    let r = compile("in go: exec\non go { for i in 0..10 { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(10));
    assert_eq!(shell_pin_literal(&r, "FirstIndex"), Some(0));
    assert_eq!(shell_pin_literal(&r, "Step"), Some(1));
}

#[test]
fn bounds_exclude_the_end() {
    let r = compile("in go: exec\non go { for i in 3..7 { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(4));
    assert_eq!(shell_pin_literal(&r, "FirstIndex"), Some(3));
}

#[test]
fn step_is_a_ceil_divide() {
    // 0..10 by 3 yields 0, 3, 6, 9 - four iterations, not three.
    let r = compile("in go: exec\non go { for i in (0..10).step(3) { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(4));
    assert_eq!(shell_pin_literal(&r, "Step"), Some(3));
}

#[test]
fn an_empty_range_is_zero_iterations() {
    let r = compile("in go: exec\non go { for i in 5..5 { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(0));
}

#[test]
fn a_backwards_range_is_zero_not_negative() {
    let r = compile("in go: exec\non go { for i in 10..0 { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(0));
}

#[test]
fn rev_counts_down_from_the_top() {
    // (0..5).rev() yields 4, 3, 2, 1, 0.
    let r = compile("in go: exec\non go { for i in (0..5).rev() { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(5));
    assert_eq!(shell_pin_literal(&r, "FirstIndex"), Some(4));
    assert_eq!(shell_pin_literal(&r, "Step"), Some(-1));
}

#[test]
fn a_runtime_rev_still_counts_down_from_the_top() {
    // FirstIndex reaches Count through `CountPin` rather than rebuilding the
    // formula, so a runtime bound must still reach the pins as real gates.
    let r = compile("var n: int = 7\nin go: exec\non go { for i in (0..n).rev() { } }");
    assert_no_errors(&r);
    assert_eq!(shell_pin_literal(&r, "Count"), None, "a runtime Count is a gate");
    assert_eq!(
        r.module.nodes.values().filter(|n| n.gate_class == gc::CEIL).count(),
        1,
        "the Count chain belongs to one pin, reused rather than rebuilt"
    );
}

#[test]
fn count_spelling_drives_the_pins_directly() {
    let r = compile("in go: exec\non go { for i in count(4, from = 10, step = 5) { } }");
    assert_eq!(shell_pin_literal(&r, "Count"), Some(4));
    assert_eq!(shell_pin_literal(&r, "FirstIndex"), Some(10));
    assert_eq!(shell_pin_literal(&r, "Step"), Some(5));
}

#[test]
fn a_runtime_bound_becomes_real_gates_not_a_placeholder() {
    // The whole reason `normalize_range` returns `PinExpr` rather than
    // synthetic `Expr::BinOp`s: a `BinOp` built after typecheck has no
    // `op_resolutions` entry, so `lower_binop` would substitute an
    // `_Unsupported` placeholder and the save would load and do nothing.
    let r = compile("var n: int = 0\nin go: exec\non go { for i in 0..n { } }");
    assert_no_errors(&r);
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
    // The `* 1.0` widening has to survive LOWERING, not just
    // `normalize_range`. `MathDivide` is one gate class for every operand
    // pair, and only `(Int, Int)` truncates, so the gate class, the presence
    // of a `Ceil` and the absence of `_Unsupported` are identical either
    // way. The rule `build_binop` resolved is visible only as the port types
    // it declared. Assert those, or this test cannot fail for the thing it
    // is named after.
    let r = compile("var n: int = 0\nin go: exec\non go { for i in (0..n).step(3) { } }");
    assert_no_errors(&r);

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
        divide.ports.find_input(WirePort::InputA.sym()).map(|p| &p.ty),
        Some(&crate::ir::Type::Float),
        "the numerator must resolve as `float`; an `int` one selects the TRUNCATING \
         (Int, Int) divide, and `ceil` then has nothing to round up"
    );
    assert_eq!(
        divide.ports.find_output(WirePort::Output.sym()).map(|p| &p.ty),
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
        widen.ports.find_output(WirePort::Output.sym()).map(|p| &p.ty),
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
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..10 { if i > 3 { break } t += i } }");
    let child = only_chip(&r, "loop body");
    assert!(
        child.nodes.values().any(|n| n.gate_class == gc::CALLABLE_BREAK),
        "break must lower to a CallableBreak inside the plane"
    );
    assert!(
        crate::catalog::default_catalog().find_by_class(gc::CALLABLE_BREAK).is_some(),
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

#[test]
fn a_statement_after_break_is_ws081_not_ws058() {
    // A statement after `break` in the same block is dead: WS081 must name
    // that cause, and WS058 must not also fire for it.
    let r = compile("var t: int = 0\nin go: exec\non go { for i in 0..3 { break\n t = t + 1 } }");
    let codes: Vec<&str> = r.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"WS081"), "expected WS081, got {codes:?}");
    assert!(!codes.contains(&"WS058"), "WS058 must be suppressed here, got {codes:?}");
}

#[test]
fn a_statement_after_break_nested_in_an_if_is_also_ws081() {
    // The dead statement sits in the SAME block as `break` (the `if`'s
    // then-block), not the loop's own top-level body.
    let r = compile(
        "var t: int = 0\nin go: exec\non go { for i in 0..5 { if i == 2 { break\n t = t + 1 } } }",
    );
    let codes: Vec<&str> = r.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"WS081"), "expected WS081, got {codes:?}");
    assert!(!codes.contains(&"WS058"), "WS058 must be suppressed here, got {codes:?}");
}

#[test]
fn a_statement_with_no_preceding_break_is_still_ws058() {
    // WS081 must not swallow the ordinary case: an assignment genuinely
    // reached from a pure position, with no `break` involved at all, keeps
    // reporting WS058 (the diagnostics.md trigger for this code).
    let r = compile("var x: int = 0\nmod f() { x = x + 1 }\nlet d = f()");
    let codes: Vec<&str> = r.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"WS058"), "expected WS058, got {codes:?}");
    assert!(!codes.contains(&"WS081"), "unexpected WS081, got {codes:?}");
}

/// Every node of `class` anywhere in the chip tree (a loop body lands on the
/// shell's own grid).
fn tree_nodes<'a>(m: &'a crate::ir::Module, class: &str) -> Vec<&'a crate::ir::Node> {
    let mut out: Vec<&crate::ir::Node> = m.nodes.values().filter(|n| n.gate_class == class).collect();
    for c in m.chips.values() {
        out.extend(tree_nodes(c, class));
    }
    out
}

/// A `for` over an array or map whose elements are enums (with or without a
/// payload) or tuples binds each element whole: the loop runs off one of the
/// element's columns and reads every column at the element's index or key,
/// where the source failed as WS071.
#[test]
fn a_for_loop_over_columnar_elements_binds_each_element() {
    let cases = [
        (
            "enum E { A, B }\nvar es: E[] = []",
            "es.push(E.B)\n for x in es { match x { A => { r += 1 } B => { r += 10 } } }\n for i, x in es { if x is E.B { r += i } }",
            gc::MICROCHIP_FOR_EACH_ARRAY,
            gc::ARRAY_GET,
        ),
        (
            "enum E { Empty, Circle(int) }\nvar es: E[] = []",
            "es.push(E.Circle(3))\n for x in es { match x { Empty => { r += 1 } Circle(c) => { r += c } } }",
            gc::MICROCHIP_FOR_EACH_ARRAY,
            gc::ARRAY_GET,
        ),
        (
            "var ts: (int, int)[] = []",
            "ts.push((1, 2))\n for t in ts { r += t.0 + t.1 }",
            gc::MICROCHIP_FOR_EACH_ARRAY,
            gc::ARRAY_GET,
        ),
        (
            "enum E { A, B }\nvar m: Map<int, E>",
            "m.set(1, E.B)\n for k, v in m { match v { A => { r += 1 } B => { r += k } } }",
            gc::MICROCHIP_FOR_EACH_MAP,
            gc::MAP_GET,
        ),
    ];
    for (decls, body, shell, read) in cases {
        let src = format!("{decls}\nvar r: int = 0\nin go: exec\non go {{\n {body}\n}}");
        let r = compile(&src);
        assert!(
            r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
            "{src}\n{:?}",
            r.diagnostics
        );
        assert!(tree_nodes(&r.module, gc::UNSUPPORTED).is_empty(), "{src}");
        let shells = tree_nodes(&r.module, shell);
        assert!(!shells.is_empty(), "{src}");
        // The driver pin takes a column's ref, and the body reads the columns.
        for s in &shells {
            let driven = r.module.wires.iter().any(|w| {
                w.target.node_id == s.id
                    && matches!(w.target.port, WirePort::ArrayVarRef | WirePort::MapVarRef)
            });
            assert!(driven, "{src}: the loop's driver pin is wired");
        }
        let reads = tree_nodes(&r.module, read)
            .into_iter()
            .filter(|n| n.chip_id.is_some() || r.module.chips.values().any(|c| c.nodes.contains_key(&n.id)))
            .count();
        assert!(reads >= 1, "{src}: the body reads the element's columns");
    }
}

/// Each binding of a columnar element reads ITS column: a tuple's `.0` and
/// `.1`, an enum's tag and each payload slot. Traced from every store in the
/// loop body back through the plane's pins to the storage it reads.
#[test]
fn a_columnar_element_reads_each_field_from_its_own_column() {
    let r = compile(
        "enum E { Empty, Box(int, int) }\nin go: exec\nvar ts: (int, string)[] = []\nvar es: E[] = []\n\
         var a: int = 0\nvar b: string = \"\"\nvar c: int = 0\nvar d: int = 0\nvar g: int = 0\n\
         on go {\n  for t in ts {\n    a = t.0\n    b = t.1\n  }\n  for x in es {\n    match x { Empty => { g = 1 } Box(p, q) => { c = p\n      d = q } }\n  }\n}",
    );
    assert_no_errors(&r);
    let mut nodes: HashMap<crate::ir::NodeId, &crate::ir::Node> = HashMap::default();
    let mut wires: Vec<crate::ir::Wire> = Vec::new();
    let mut stack = vec![&r.module];
    while let Some(m) = stack.pop() {
        nodes.extend(m.nodes.iter().map(|(id, n)| (*id, n)));
        wires.extend(m.wires.iter().copied());
        stack.extend(m.chips.values());
    }
    // The node driving `port` of `id`, followed back through plane pins.
    let source = |id: crate::ir::NodeId, port: WirePort| -> Option<crate::ir::NodeId> {
        let mut at = wires.iter().find(|w| w.target.node_id == id && w.target.port == port)?.source.node_id;
        while let Some(w) = wires
            .iter()
            .find(|w| w.target.node_id == at && w.target.port == WirePort::RerInput && nodes.get(&at).is_some_and(|n| n.kind != crate::ir::NodeKind::Output))
        {
            at = w.source.node_id;
        }
        Some(at)
    };
    let label = |id: crate::ir::NodeId| match nodes[&id].properties.get(&crate::intern::sym::NAME_LABEL) {
        Some(crate::ir::Literal::String(s)) => s.clone(),
        _ => format!("{}#{id}", nodes[&id].gate_class),
    };
    // Each store's target var and the column its value is read from.
    let mut read: Vec<(String, String)> = Vec::new();
    for (id, n) in &nodes {
        if n.gate_class != gc::VAR_SET {
            continue;
        }
        let (Some(var), Some(value)) = (source(*id, WirePort::VarRef), source(*id, WirePort::Value)) else {
            continue;
        };
        if nodes[&value].gate_class != gc::ARRAY_GET {
            continue;
        }
        let column = source(value, WirePort::ArrayVarRef).expect("the read's column");
        read.push((label(var), label(column)));
    }
    read.sort();
    let tag_read = nodes.values().filter(|n| n.gate_class == gc::ARRAY_GET).any(|n| {
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
