use super::nodes_of;
use super::*;
use crate::ir::gate_class as gc;
use crate::test_support::{assert_code, assert_code_count};

#[test]
fn return_in_mod_does_not_kill_caller_exec() {
    // Regression: `return` in an inlined mod should jump to the mod's
    // return union, not kill the containing handler's exec chain.
    let r = compile_ok(
        "\
var x: int = 0
mod maybe_return(flag: int) {
  if flag == 1 { return }
  x = 99
}
in player: character
on player {
  maybe_return(1)
  x = 42
}",
    );
    assert_eq!(
        gate_count(&r, gc::VAR_SET),
        2,
        "`x = 99` and the `x = 42` after the mod's return"
    );
    assert_eq!(gate_count(&r, gc::UNION), 1, "the mod's return union");
}

#[test]
fn multiple_returns_in_mod_chain_unions() {
    // Two returns + fallthrough must not multi-connect to one union input.
    // Each return path should chain through separate union gates.
    let r = compile_ok(
        "\
var x: int = 0
mod multi_guard(a: int) {
  if a == 0 { return }
  if a == 1 { return }
  x = 99
}
in player: character
on player {
  multi_guard(5)
  x = 42
}",
    );
    // Two returns + fallthrough needs 2 merge union gates (chain):
    // union1 merges return1 + return2, union2 merges union1 + fallthrough.
    // (The if-joins collapse: each has only its else arm, so the prune
    // splices them as pass-throughs.)
    assert_eq!(
        gate_count(&r, gc::UNION),
        2,
        "two returns + fallthrough chain two merge unions"
    );
    // No union input port should have more than one wire
    for node in r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::UNION)
    {
        for port in &node.ports.inputs {
            let incoming = r
                .module
                .wires
                .iter()
                .filter(|w| {
                    w.target.node_id == node.id
                        && crate::intern::resolve(port.name) == w.target.port.as_str()
                })
                .count();
            assert!(
                incoming <= 1,
                "union port {}:{} has {} incoming exec wires, expected at most 1",
                node.id,
                crate::intern::resolve(port.name),
                incoming
            );
        }
    }
}

#[test]
fn nested_mod_array_push_lowers() {
    let r = compile_ok(
        "\
var data: int[]
in player: character
on player {
  data.clear()
  mod fill(arr: int[]) {
    arr.push(0)
    arr.push(0)
  }
  fill(data)
}",
    );
    assert!(
        !has_unsupported(&r.module),
        "nested mod arr.push should not produce _Unsupported"
    );
    assert_eq!(
        gate_count(&r, gc::ARRAY_PUSH),
        2,
        "one ArrayVar_Push per `arr.push(0)`"
    );
}

#[test]
fn mod_let_bindings_do_not_leak_into_caller() {
    // A mod with `let x` must not overwrite the caller's `let x`: after the mod
    // returns, `w = x` stores the caller's `v + 1`, not the mod's `v & 0xFF`.
    let r = compile_ok(
        "\
var v: int = 0
var w: int = 0
mod inner(v: *int) {
  let x = v & 0xFF
  v = x
}
in player: character
on player {
  v = 42
  let x = v + 1
  inner(v)
  w = x
}",
    );
    let stored = super::generics::stored_values(&r, super::generics::var_labelled(&r, "w"));
    let classes: Vec<&str> = stored.iter().map(|(class, _)| *class).collect();
    assert_eq!(
        classes,
        ["BrickComponentType_Expr_MathAdd"],
        "`w = x` reads the caller's `x`"
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_BitwiseAND"),
        1,
        "the mod's own `v & 0xFF`"
    );
}

#[test]
fn single_return_value_wires_directly() {
    let r = compile_ok(
        "\
mod double(x: int) -> (result: int) {
  return x + x
}
in player: character
on player {
  let d = double(21)
}",
    );
    // Inline mods don't create MicrochipOutput: the value wires directly to the caller.
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_MathAdd"),
        1,
        "x + x"
    );
    let ret_vars = r
        .module
        .nodes
        .values()
        .filter(|n| n.note == Some("ret_val"))
        .count();
    assert_eq!(ret_vars, 0, "single return should not create a ret_val var");
}

/// A standalone `chip` whose body runs an exec op (array find) and returns a
/// value exposes BOTH its value output and an `_exec_out` on every instance,
/// and the value output is fed by the returned `if` SELECT.
#[test]
fn a_standalone_chip_returning_from_an_exec_body_keeps_both_outputs() {
    let chip = "chip slotOfUser(uid: int) -> int {\n  let res = counts.find(uid)\n  \
                return if res.Found then res.Index else -1\n}\n";
    let rows: &[(&str, String, usize)] = &[
        (
            "a trailing `return` moves the body's tail exec into mod_return_exec; the standalone \
             chip path never merged it back before building `_exec_out`, so the chip shipped with \
             one output",
            format!("var counts: int[]\n{chip}in z: exec\non z {{\n  let s = slotOfUser(1)\n}}"),
            1,
        ),
        (
            "an enclosing `out` is an inherited `Binding::Output` that inflated output_count() \
             past 1, so `Stmt::Return` skipped wiring the SELECT into the MicrochipOutput",
            format!(
                "in z: exec\nvar counts: int[]\nout counts2: int[] = counts\n{chip}\
                 on z {{\n  BroadcastChatMessage(\"slot: ${{slotOfUser(1)}}\")\n}}"
            ),
            1,
        ),
        (
            "the second call instantiates the cached template, and keeps the value wire too",
            format!(
                "var counts: int[]\n{chip}in z: exec\non z {{\n  let a = slotOfUser(1)\n  let b = slotOfUser(2)\n}}"
            ),
            2,
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, instances) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let chips: Vec<_> = r
            .module
            .chips
            .values()
            .filter(|c| crate::intern::resolve(c.name).contains("slotOfUser"))
            .collect();
        if chips.len() != *instances {
            failures.push(format!(
                "{label}: {} slotOfUser instances, want {instances}",
                chips.len()
            ));
        }
        for chip in chips {
            if chip.outputs.len() != 2 {
                failures.push(format!(
                    "{label}: {} outputs, want a value output and an _exec_out",
                    chip.outputs.len()
                ));
                continue;
            }
            let fed = chip
                .wires
                .iter()
                .find(|w| w.target.node_id == chip.outputs[0])
                .map(|w| chip.nodes[&w.source.node_id].gate_class);
            if fed != Some(gc::SELECT) {
                failures.push(format!(
                    "{label}: the value output is fed by {fed:?}, not the if-expr SELECT"
                ));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn inline_mod_exec_body_returns_select_reaches_consumer() {
    // An inline mod: exec op (find) then `return <if>`. The SELECT holding
    // the returned value must reach the caller's consumer (a Var_Set), i.e.
    // the return value isn't dropped when the body also advances the exec
    // chain.
    let r = compile_ok(
        "\
var counts: int[]
mod slotOfUser(uid: int) -> int {
  let res = counts.find(uid)
  return if res.Found then res.Index else -1
}
var result: int = 0
in z: exec
on z {
  result = slotOfUser(1)
}",
    );
    assert!(
        !has_unsupported(&r.module),
        "no unsupported placeholder expected"
    );
    let select = find_gate(&r, gc::SELECT);
    let set = find_gate(&r, gc::VAR_SET);
    assert!(
        wired_reachable(&r, select, set),
        "the returned SELECT value must reach the `result` Var_Set"
    );
}

/// Several returns from a single-output mod store into return storage: one
/// `ret_val` var, one `ret_set` per return and one `ret_get` the caller reads.
/// A mod with no output ignores a returned value entirely.
#[test]
fn returns_store_into_one_ret_val_per_call() {
    let rows: &[(&str, &str, [usize; 3])] = &[
        (
            "three returns",
            "mod my_clamp(v: int, lo: int, hi: int) -> (result: int) {\n  if v < lo { return lo }\n  \
             if v > hi { return hi }\n  return v\n}\nin player: character\non player {\n  let c = my_clamp(50, 0, 100)\n}",
            [1, 3, 1],
        ),
        (
            "four returns, two of them nested",
            "mod classify(x: int) -> int {\n  if x > 100 {\n    return 3\n  }\n  if x > 10 {\n    if x > 50 {\n      \
             return 2\n    }\n    return 1\n  }\n  return 0\n}\nin player: character\non player {\n  var v: int = 42\n  \
             let c = classify(v)\n}",
            [1, 4, 1],
        ),
        (
            "a returned value in a mod with no output is ignored",
            "mod noop(x: *int) {\n  if x > 10 { return 0 }\n  x = x + 1\n}\nvar v: int = 5\nin player: character\n\
             on player { noop(v) }",
            [0, 0, 0],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let notes = |note| {
            r.module
                .nodes
                .values()
                .filter(|n| n.note == Some(note))
                .count()
        };
        let got = [notes("ret_val"), notes("ret_set"), notes("ret_get")];
        if got != *want {
            failures.push(format!(
                "{label}: [ret_val, ret_set, ret_get] = {got:?}, want {want:?}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn return_block_expr() {
    let r = compile_ok(
        "\
mod foo(x: int) -> int {
  return { let a = x + 1; a }
}
in player: character
on player {
  let f = foo(5)
}",
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_MathAdd"),
        1,
        "x + 1 inside the returned block"
    );
}

#[test]
fn mod_with_output_and_ref_params() {
    let r = compile_ok(
        "\
mod inc_and_get(x: *int) -> int {
  x = x + 1
  return x
}
var v: int = 10
in player: character
on player {
  let result = inc_and_get(v)
}",
    );
    assert_eq!(
        gate_count(&r, gc::VAR_INCREMENT),
        1,
        "`x = x + 1` through the ref param"
    );
    assert_eq!(gate_count(&r, gc::VAR_SET), 0);
    // An inline mod has no output node: `return x` reads the var through a Var_Get.
    assert_eq!(
        gate_count(&r, gc::VAR_GET),
        1,
        "`return x` reads the referenced var"
    );
}

#[test]
fn two_mods_same_local_names() {
    let r = compile_ok(
        "\
var x: int = 10
mod a(v: *int) {
  let tmp = v + 1
  v = tmp
}
mod b(v: *int) {
  let tmp = v * 2
  v = tmp
}
in player: character
on player {
  a(x)
  b(x)
}",
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_MathAdd"),
        1,
        "v + 1 in mod a"
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_MathMultiply"),
        1,
        "v * 2 in mod b"
    );
    assert_eq!(
        gate_count(&r, gc::VAR_SET),
        2,
        "each mod stores its own `tmp`"
    );
}

#[test]
fn var_reset_in_nested_mod() {
    // `inner_mod` is declared first: calling a mod declared later is a WS021,
    // which is not what this test is about.
    let r = compile_ok(
        "\
mod inner_mod() {
  var x: int = 0
  x = x + 10
}
mod outer_mod() {
  var x: int = 0
  x = x + 1
  inner_mod()
}
in player: character
on player {
  outer_mod()
}",
    );
    assert_eq!(gate_count(&r, gc::VAR), 2, "a separate `var x` per mod");
}

#[test]
fn static_var_across_multiple_calls() {
    let r = compile_ok(
        "\
mod counter() -> int {
  static var n: int = 0
  n = n + 1
  return n
}
in player: character
on player {
  let a = counter()
  let b = counter()
}",
    );
    assert_eq!(
        gate_count(&r, gc::VAR),
        2,
        "two inlined calls each get a PseudoVar"
    );
    assert_eq!(
        gate_count(&r, gc::VAR_INCREMENT),
        2,
        "each call increments its own `n`"
    );
}

#[test]
fn single_output_mod_in_if_condition() {
    let r = compile_ok(
        "\
var v: int = 5
mod is_pos(x: int) -> int {
  return if x > 0 then 1 else 0
}
in player: character
on player {
  let check = is_pos(v)
  let label = if check > 0 then \"y\" else \"n\"
}",
    );
    assert_eq!(gate_count(&r, gc::SELECT), 2, "the inner and the outer if");
}

// ── WS090: a value-returning mod/chip whose body can end without one ──

/// Every body here has a path that reaches the end with no `return <value>`
/// and assigns no output by name, so the output would carry a value from
/// another path (the lone return's, or the previous call's).
#[test]
fn a_path_that_falls_off_the_end_of_a_value_mod_is_ws090() {
    for src in [
        "mod f(c: bool, q: int) -> (ret: int) {\n  if c { return q }\n}",
        "mod f(c: bool, q: int) -> int {\n  if c { return q }\n}",
        "mod f(x: int) -> int {\n  if x > 0 { return 1 }\n  if x < 0 { return 2 }\n}",
        "mod f(x: int) -> int {\n  if x > 0 { return 1 } else { let y = 2 }\n}",
        "mod f(x: int) -> int {\n  if x > 0 {\n    if x > 5 { return 1 }\n  } else { return 2 }\n}",
        "chip C(x: int) -> (o: int) {\n  if x > 0 { return 1 }\n}",
        "enum E { A, B }\nmod f(e: E) -> int {\n  match e {\n    A => { return 1 }\n    B => { let y = 2 }\n  }\n}",
        "mod outer() {\n  mod f(c: bool) -> int {\n    if c { return 1 }\n  }\n}",
        // An exec output does not stand in for the value one.
        "mod f(c: bool, q: int) -> (r: int, done: exec) {\n  if c { return q }\n  emit done\n}",
        // A bare `return` before the value output is set on that path.
        "mod f(c: bool) -> int {\n  if c { return 5 }\n  return\n}",
        "mod f(c: bool) -> (r: int) {\n  if c { return }\n  emit r = 1\n  return\n}",
        // Set on one branch only, then the end of the body.
        "mod f(c: bool) -> (r: int) {\n  if c { return 1 }\n  if !c { emit r = 2 }\n}",
        "mod f(c: bool) -> (a: int, b: int) {\n  if c { return (1, 2) }\n  emit a = 3\n}",
    ] {
        let e = errs(src);
        assert!(
            e.iter().any(|c| c == "WS090"),
            "expected WS090:\n{src}\n{e:?}"
        );
    }
}

/// Bodies that return on every path, or set their output by name, or have no
/// value output at all.
#[test]
fn a_body_that_returns_on_every_path_is_not_ws090() {
    for src in [
        "mod f(c: bool, q: int) -> int {\n  if c { return q }\n  return 0\n}",
        "mod f(c: bool, q: int) -> int {\n  if c { return q } else { return 0 }\n}",
        "mod f(c: bool, q: int) -> int {\n  if c { return q } else if q > 0 { return 1 } else { return 2 }\n}",
        "enum E { A, B }\nmod f(e: E) -> int {\n  match e {\n    A => { return 1 }\n    B => { return 2 }\n  }\n}",
        "mod f(c: bool, q: int) -> (r: int) {\n  if c { return q }\n  emit r = 5\n}",
        "mod f(c: bool, q: int) -> (r: int) {\n  out r = q\n}",
        "mod f(c: bool, q: int) -> (r: int, done: exec) {\n  if c { return q }\n  emit r = 0\n  emit done\n}",
        "mod f(c: bool) -> (r: int) {\n  if c {\n    emit r = 1\n    return\n  }\n  return 2\n}",
        "mod f(c: bool) -> (r: int) {\n  if c { emit r = 1 } else { emit r = 2 }\n  return\n}",
        "mod f(c: bool) {\n  if c { return }\n}",
        "var x: int = 0\nmod f(c: bool) -> (done: exec) {\n  if c { return }\n  x = 1\n}",
        "mod f(x: int) -> int {\n  return x\n  let y = 1\n}",
        "mod f(n: int) -> int {\n  var i = 0\n  let loop: exec\n  emit loop\n  await loop\n  \
         if i < n {\n    i += 1\n    buffer emit loop\n  } else {\n    return i\n  }\n}",
    ] {
        let e = errs(src);
        assert!(
            !e.iter().any(|c| c == "WS090"),
            "unexpected WS090:\n{src}\n{e:?}"
        );
    }
}

/// One value returned to a mod with several outputs has no output to go to:
/// `f(c).r` read the call's exec wire through a color split. It is WS003, and
/// the field read never falls back to a component split.
#[test]
fn a_single_value_return_to_a_multi_output_mod_is_rejected() {
    let src = "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
               mod f(c: bool) -> (r: int, done: exec) {\n  return q\n}\n\
               on go { m = f(c).r }";
    assert!(errs(src).iter().any(|c| c == "WS003"), "{:?}", errs(src));
    let r = compile(src);
    assert!(
        !has_gate(&r, crate::ir::gate_class::SPLIT_COLOR),
        "a color split read the call's exec wire"
    );
    assert_code(&r.diagnostics, "WS072");
    // Setting the outputs by name is the working spelling.
    let ok = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
         mod f(c: bool) -> (r: int, done: exec) {\n  emit r = q\n  emit done\n}\n\
         on go { m = f(c).r }",
    );
    assert!(!has_gate(&ok, crate::ir::gate_class::SPLIT_COLOR));
}

/// The source port wired into `class`'s `port`, for its one instance.
fn wired_into(r: &LowerResult, class: &str, port: WirePort) -> Option<crate::ir::PortRef> {
    let id = find_gate(r, class);
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == id && w.target.port == port)
        .map(|w| w.source)
}

/// `f(c).r` names the only output of a mod or chip, so it reads the call's
/// value, in every callee form. A component split must never stand in for it.
#[test]
fn reading_the_only_named_output_of_a_call_is_its_value() {
    let r = compile_ok(
        "var m: int = 0\nin go: exec\nin n: int\n\
         mod s(k: int) -> (r: int) {\n  return k + 1\n}\n\
         on go { m = s(n).r }",
    );
    let add = find_gate(&r, "BrickComponentType_Expr_MathAdd");
    assert_eq!(
        wired_into(&r, crate::ir::gate_class::VAR_SET, WirePort::Value).map(|p| p.node_id),
        Some(add)
    );

    let _ = compile_ok(
        "in go: exec\nchip K(go: exec) -> (r: int) {\n  var v: int = 0\n  on go { v = 1 }\n  out r = v\n}\n\
         out o2: int = K(go).r",
    );

    let r = compile_multi(
        "import * as u from \"lib2\"\nvar m: int = 0\nin go: exec\nin n: int\nin c: bool\n\
         on go {\n  m = u.s(n).r\n  m = u.f(c).r\n}",
        &[(
            "lib2",
            "mod s(k: int) -> (r: int) {\n  return k + 1\n}\n\
             mod f(c: bool) -> (r: int, done: exec) {\n  emit r = 1\n  emit done\n}",
        )],
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, crate::ir::gate_class::SPLIT_COLOR),
        "a named output fell to a color split"
    );
}

/// A record value for a mod with several outputs must line up with them.
#[test]
fn a_record_return_that_does_not_match_the_outputs_is_ws003() {
    for src in [
        "type Point = { x: int, y: int }\nmod f(k: int) -> (p: Point, q: int) {\n  return { x: k, y: 2 }\n}",
        "type Point = { x: int, y: int }\nmod f(c: bool) -> (p: Point, done: exec) {\n  return { x: 1, y: 2 }\n}",
        "type Point = { x: int, y: int }\nvar pt: Point\nmod f(c: bool) -> (p: Point, done: exec) {\n  return pt\n}",
        "mod f(c: bool) -> (a: int, b: int) {\n  return { a: 1 }\n}",
        "mod f(c: bool) -> (a: int, b: int) {\n  return (1, 2, 3)\n}",
    ] {
        let e = errs(src);
        assert!(e.iter().any(|c| c == "WS003"), "{src}\n{e:?}");
    }
}

/// A record output among several is filled on every form.
#[test]
fn a_record_output_among_several_is_filled() {
    for (outs, body) in [
        ("(p: Point, q: int)", "  return ({ x: k, y: 2 }, 3)"),
        ("(p: Point, q: int)", "  return { p: { x: k, y: 2 }, q: 3 }"),
        (
            "(p: Point, q: int)",
            "  emit p = { x: k, y: 2 }\n  emit q = 3",
        ),
        (
            "(p: Point, done: exec)",
            "  emit p = { x: k, y: 2 }\n  emit done",
        ),
    ] {
        let src = format!(
            "type Point = {{ x: int, y: int }}\nvar m: int = 0\nin go: exec\nin n: int\n\
             mod f(k: int) -> {outs} {{\n{body}\n}}\n\
             on go {{\n  let t = f(n)\n  m = t.p.x\n}}"
        );
        let r = compile_ok(&src);
        assert!(!has_gate(&r, crate::ir::gate_class::UNSUPPORTED), "{src}");
        assert!(!has_gate(&r, crate::ir::gate_class::SPLIT_VECTOR), "{src}");
        let n = r
            .module
            .inputs
            .iter()
            .copied()
            .find(|id| r.module.nodes[id].source_range.start.line == 4);
        assert_eq!(
            wired_into(&r, crate::ir::gate_class::VAR_SET, WirePort::Value).map(|p| p.node_id),
            n,
            "`t.p.x` must read `k`:\n{src}"
        );
    }
}

/// Several writes to one output (`emit` in two branches, or `emit` mixed with
/// `return`) go through storage, so the path that ran decides the value.
#[test]
fn several_writes_to_one_output_go_through_storage() {
    for body in [
        "  if c { emit r = q } else { emit r = p }",
        "  if c {\n    emit r = q\n    return\n  }\n  return p",
        "  emit r = p\n  if c { emit r = q }",
    ] {
        let src = format!(
            "in go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
             mod f(c: bool) -> (r: int) {{\n{body}\n}}\non go {{ m = f(c) }}"
        );
        let r = compile_ok(&src);
        let sets = r
            .module
            .nodes
            .values()
            .filter(|n| n.note == Some("ret_set"))
            .count();
        assert_eq!(sets, 2, "each write stores:\n{src}");
        let get = r
            .module
            .nodes
            .values()
            .find(|n| n.note == Some("ret_get"))
            .map(|n| n.id);
        assert!(get.is_some(), "the caller reads the storage:\n{src}");
        let m_set = r
            .module
            .nodes
            .values()
            .find(|n| n.gate_class == crate::ir::gate_class::VAR_SET && n.note.is_none())
            .map(|n| n.id)
            .expect("m's write");
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| Some(w.source.node_id) == get && w.target.node_id == m_set),
            "m reads the stored value:\n{src}"
        );
    }
    // With several outputs, every output goes through the per-output storage.
    let r = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
         mod f(c: bool) -> (a: int, b: int) {\n  if c { emit a = q } else { emit a = p }\n  emit b = q\n}\n\
         on go { m = f(c).a + f(c).b }",
    );
    assert!(!has_gate(&r, crate::ir::gate_class::UNSUPPORTED));
    let add = find_gate(&r, "BrickComponentType_Expr_MathAdd");
    // `a` is written twice, so it is stored; `b` is written once and keeps its
    // direct wire.
    for (port, class) in [
        (WirePort::InputA, crate::ir::gate_class::VAR_GET),
        (WirePort::InputB, crate::ir::gate_class::MICROCHIP_INPUT),
    ] {
        let src = r
            .module
            .wires
            .iter()
            .find(|w| w.target.node_id == add && w.target.port == port)
            .map(|w| w.source.node_id);
        assert_eq!(
            src.map(|s| r.module.nodes[&s].gate_class),
            Some(class),
            "{port:?}"
        );
    }
    // One write per output and no early return keeps the direct wire.
    let r = compile_ok(
        "in go: exec\nin q: int\nvar m: int = 0\nmod f() -> (r: int, s: int) {\n  emit r = q\n  emit s = q\n}\n\
         on go { m = f().r }",
    );
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| matches!(n.note, Some("ret_set" | "ret_get" | "ret_val")))
    );
}

/// Every `return` inside a `match` or `if let` still assigns the output, so
/// neither WS013 nor WS090 fires.
#[test]
fn returns_inside_match_and_if_let_assign_the_output() {
    for src in [
        "enum Sh { A, B(int), C }\nvar s: Sh = Sh.A\nmod f() -> int {\n  match s {\n    \
         A => { return 1 }\n    B(k) => { return k }\n    C => { return 3 }\n  }\n}",
        "enum Opt { Some(int), None }\nvar s: Opt = Opt.None\nmod g() -> int {\n  \
         if let Some(v) = s { return v } else { return 0 }\n}",
        "enum Opt { Some(int), None }\nvar s: Opt = Opt.None\nmod f() -> int {\n  \
         let Some(v) = s else { return 0 }\n  return v\n}",
    ] {
        let e = errs(src);
        assert!(
            !e.iter().any(|c| c == "WS013" || c == "WS090"),
            "{src}\n{e:?}"
        );
    }
}

/// The node wired into the `Value` of the write to `var m` (the Var_Set whose
/// `VarRef` comes from the var labelled `m`).
fn m_value_source(r: &LowerResult) -> Option<crate::ir::NodeId> {
    let m = r.module.nodes.values().find(|n| {
        n.gate_class == crate::ir::gate_class::VAR
            && n.properties.get(&crate::intern::sym::NAME_LABEL)
                == Some(&Literal::String("m".into()))
    })?;
    let set = r
        .module
        .wires
        .iter()
        .find(|w| w.source.node_id == m.id && w.target.port == WirePort::VarRef)?
        .target
        .node_id;
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == set && w.target.port == WirePort::Value)
        .map(|w| w.source.node_id)
}

/// The input port node declared on `line`.
fn input_on_line(r: &LowerResult, line: u32) -> Option<crate::ir::NodeId> {
    r.module
        .inputs
        .iter()
        .copied()
        .find(|id| r.module.nodes[id].source_range.start.line == line)
}

/// An output bound by `out` beside one that goes through the emit storage
/// keeps its own value: `f(c).b` read a storage field nothing wrote.
#[test]
fn an_out_bound_output_beside_a_stored_one_keeps_its_value() {
    let r = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
         mod f(c: bool) -> (a: int, b: int) {\n  if c { emit a = q } else { emit a = p }\n  out b = q\n}\n\
         on go { m = f(c).b }",
    );
    assert_eq!(
        m_value_source(&r),
        input_on_line(&r, 3),
        "`f(c).b` must read `q`"
    );

    let r = compile_ok(
        "type Point = { x: int, y: int }\nin go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
         var pt: Point\nmod f(c: bool) -> (a: int, pp: Point) {\n  if c { emit a = q } else { emit a = p }\n  \
         out pp = { x: q, y: 1 }\n}\non go {\n  let t = f(c)\n  m = t.pp.x\n}",
    );
    assert_eq!(
        m_value_source(&r),
        input_on_line(&r, 4),
        "`t.pp.x` must read `q`"
    );
}

/// A call result routes into a mod's outputs by position when its value
/// outputs line up, whatever they are named and whatever exec outputs it has.
#[test]
fn a_call_result_returned_to_several_outputs_routes_by_position() {
    for src in [
        "in c: bool\nmod g(k: int) -> (a: int, b: int, done: exec) {\n  emit a = k\n  emit b = 2\n  emit done\n}\n\
         mod f(k: int) -> (a: int, b: int) {\n  if c { return g(k) }\n  return (1, 1)\n}",
        "in c: bool\nmod g(k: int) -> (a: int, b: int) {\n  return (k, 2)\n}\n\
         mod f(k: int) -> (x: int, y: int) {\n  if c { return g(k) }\n  return (1, 1)\n}",
    ] {
        let e = errs(src);
        assert!(!e.iter().any(|c| c == "WS003"), "{src}\n{e:?}");
    }
    // A user-built record still routes by name.
    let e = errs("mod f(k: int) -> (x: int, y: int) {\n  return { a: 1, b: 2 }\n}");
    assert!(e.iter().any(|c| c == "WS003"), "{e:?}");
}

/// A write to a mod output inside a nested handler, anonymous chip or loop
/// goes through the storage too, so it is not dropped for the first write.
#[test]
fn nested_writes_to_a_mod_output_go_through_storage() {
    for body in [
        "  emit r = p\n  for i in 0..3 {\n    if c { emit r = q }\n  }",
        "  emit r = p\n  chip {\n    if c { emit r = q }\n  }",
    ] {
        let src = format!(
            "in go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
             mod f(c: bool) -> (r: int) {{\n{body}\n}}\non go {{ m = f(c) }}"
        );
        let r = compile_ok(&src);
        let sets = nodes_of(&r.module, crate::ir::gate_class::VAR_SET)
            .into_iter()
            .filter(|n| n.note == Some("ret_set"))
            .count();
        assert_eq!(sets, 2, "both writes store:\n{src}");
    }
}

/// A chip instance that emits one output from both arms of a branch routes the
/// writes through a backing var instead of fanning in on the output.
#[test]
fn a_chip_emitting_one_output_from_two_arms_compiles() {
    let r = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nin p: int\n\
         chip K(go: exec, c: bool) -> (r: int) {\n  on go {\n    if c { emit r = q } else { emit r = p }\n  }\n}\n\
         out o: int = K(go, c)",
    );
    let chip = only_chip(&r, "K's instance");
    let backing = chip
        .nodes
        .values()
        .find(|n| n.note == Some("out_backing"))
        .map(|n| n.id);
    assert!(backing.is_some(), "the output is backed by a var");
    assert_eq!(
        chip.nodes
            .values()
            .filter(|n| n.gate_class == crate::ir::gate_class::VAR_SET)
            .count(),
        2
    );
    let layout = crate::layout::layout(&r.module);
    assert_emits(&r.module, &layout);
}

/// An `emit` only inside `if let` / `match` arms assigns the output (no WS013).
#[test]
fn an_emit_in_if_let_or_match_arms_assigns_the_output() {
    for src in [
        "enum Sh { A, B(int) }\nvar sh: Sh = Sh.A\nin p: int\nmod f() -> (r: int) {\n  match sh {\n    \
         A => { emit r = p }\n    B(k) => { emit r = k }\n  }\n}",
        "enum Opt { Some(int), None }\nvar s: Opt = Opt.None\nmod f() -> (r: int) {\n  \
         if let Some(v) = s { emit r = v } else { emit r = 7 }\n}",
    ] {
        let e = errs(src);
        assert!(!e.iter().any(|c| c == "WS013"), "{src}\n{e:?}");
    }
}

/// Indexing an array literal: a constant index folds, a runtime index reads
/// the literal as an immutable container, and a runtime element is WS044.
#[test]
fn indexing_an_array_literal_folds_or_reads_a_container() {
    let r = compile_ok("in go: exec\nvar m: int = 0\non go { m = [10, 20, 30][1] }");
    crate::test_support::assert_no_code(&r.diagnostics, "WSP001");
    assert!(
        nodes_of(&r.module, crate::ir::gate_class::ARRAY_GET).is_empty(),
        "a constant read folds"
    );

    let r = compile_ok("in go: exec\nin i: int\nvar m: int = 0\non go { m = [10, 20, 30][i] }");
    crate::test_support::assert_no_code(&r.diagnostics, "WSP001");
    assert_eq!(
        nodes_of(&r.module, crate::ir::gate_class::ARRAY_GET).len(),
        1
    );

    // A runtime element at a runtime index has no storage to read (a constant
    // index selects the element, see below).
    let r = compile(
        "in go: exec\nin a: int\nin b: int\nin i: int\nvar m: int = 0\non go { m = [a, b][i] }",
    );
    assert_code(&r.diagnostics, "WS044");
    crate::test_support::assert_no_code(&r.diagnostics, "WSP001");
}

/// An output written only by `out`, twice or inside a handler, goes through the
/// storage like an emitted one: the write on the path that ran is read.
#[test]
fn out_only_writes_to_a_mod_output_go_through_storage() {
    for (src, stores) in [
        (
            "in go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
             mod f(c: bool) -> (r: int) {\n  if c { out r = q } else { out r = p }\n}\non go { m = f(c) }",
            2,
        ),
        (
            "in go: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
             mod f(c: bool) -> (a: int, b: int) {\n  emit a = 1\n  if c { out b = q } else { out b = p }\n}\n\
             on go { m = f(c).b }",
            2,
        ),
        (
            "in go: exec\nin tick: exec\nin q: int\nin p: int\nvar m: int = 0\n\
             mod f() -> (r: int) {\n  out r = p\n  on tick { out r = q }\n}\non go { m = f() }",
            2,
        ),
    ] {
        let r = compile_ok(src);
        // Every write stores, besides the write to `m` itself.
        let sets = nodes_of(&r.module, crate::ir::gate_class::VAR_SET).len() - 1;
        assert_eq!(sets, stores, "each write stores:\n{src}");
        let src_node = m_value_source(&r).expect("m's write");
        assert!(
            nodes_of(&r.module, crate::ir::gate_class::VAR_GET)
                .iter()
                .any(|n| n.id == src_node),
            "m reads the storage:\n{src}"
        );
    }
}

/// `m = f()` on a mod with one value output and an exec output reads the value
/// output, never the exec wire.
#[test]
fn auto_unwrapping_a_value_and_exec_output_mod_reads_the_value() {
    let r = compile_ok(
        "in go: exec\nin q: int\nvar m: int = 0\n\
         mod f() -> (r: int, done: exec) {\n  emit r = q\n  emit done\n}\non go { m = f() }",
    );
    assert_eq!(
        m_value_source(&r),
        input_on_line(&r, 2),
        "`m = f()` must read `q`"
    );
    let set = r
        .module
        .wires
        .iter()
        .find(|w| Some(w.source.node_id) == input_on_line(&r, 2))
        .map(|w| w.source.port);
    assert_eq!(set, Some(WirePort::RerOutput));
}

/// A single conditional write stores, so the path that skips it reads the
/// value the output last held rather than the write's value.
#[test]
fn a_single_conditional_write_to_a_mod_output_is_stored() {
    for body in [
        "  if c { emit r = q }",
        "  for i in 0..3 {\n    if c { emit r = q + i }\n  }",
    ] {
        let src = format!(
            "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
             mod f(c: bool) -> (r: int) {{\n{body}\n}}\non go {{ m = f(c) }}"
        );
        let r = compile_ok(&src);
        let src_node = m_value_source(&r).expect("m's write");
        assert!(
            nodes_of(&r.module, crate::ir::gate_class::VAR_GET)
                .iter()
                .any(|n| n.id == src_node),
            "m reads the storage, not the write's value:\n{src}"
        );
        // A call that skips the write reads the value the output last held
        // (the storage persists, as a chip's output does): no reset.
        let resets = nodes_of(&r.module, crate::ir::gate_class::VAR_SET)
            .into_iter()
            .filter(|n| n.note == Some("ret_reset"))
            .count();
        assert_eq!(resets, 0, "{src}");
    }
}

/// `f(c).pp.x` reads a stored record output directly, as `let t = f(c)` then
/// `t.pp.x` does.
#[test]
fn a_field_of_a_stored_record_output_reads_directly() {
    let r = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nin p: int\ntype Point = { x: int, y: int }\nvar m: int = 0\n\
         mod f(c: bool) -> (a: int, pp: Point) {\n  emit a = 1\n  \
         if c { emit pp = { x: q, y: 1 } } else { emit pp = { x: p, y: 2 } }\n}\n\
         on go { m = f(c).pp.x }",
    );
    assert!(!has_gate(&r, crate::ir::gate_class::SPLIT_VECTOR));
    let src_node = m_value_source(&r).expect("m's write");
    assert!(
        nodes_of(&r.module, crate::ir::gate_class::VAR_GET)
            .iter()
            .any(|n| n.id == src_node)
    );
}

/// Chip outputs written in several ways compile and emit: a non-constant
/// default beside an emit is a user diagnostic, a record output emitted from
/// two arms is backed per field, and a `return` beside an `emit` stores both.
#[test]
fn chip_outputs_written_several_ways_compile_or_say_why() {
    let r = compile(
        "in go: exec\nin c: bool\nin q: int\nin p: int\n\
         chip K(go: exec, c: bool) -> (r: int) {\n  out r = q\n  on go {\n    if c { emit r = p }\n  }\n}\n\
         out o: int = K(go, c)",
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS013" && d.message.contains("constant default")),
        "{:?}",
        r.diagnostics
    );
    for src in [
        "in go: exec\nin c: bool\nin q: int\nin p: int\ntype Point = { x: int, y: int }\nvar m: int = 0\n\
         chip K(go: exec, c: bool) -> (pp: Point) {\n  on go {\n    \
         if c { emit pp = { x: q, y: 1 } } else { emit pp = { x: p, y: 2 } }\n  }\n}\n\
         on go { m = K(go, c).x }",
        "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
         chip K(c: bool) -> (r: int) {\n  if c { return 5 }\n  emit r = q\n}\non go { m = K(c) }",
    ] {
        let r = compile_ok(src);
        let layout = crate::layout::layout(&r.module);
        assert_emits(&r.module, &layout);
    }
}

/// `[a, b][1]` with a constant index selects that element.
#[test]
fn a_constant_index_into_an_array_literal_selects_the_element() {
    let r =
        compile_ok("in go: exec\nin a: int\nin b: int\nvar m: int = 0\non go { m = [a, b][1] }");
    assert_eq!(
        m_value_source(&r),
        input_on_line(&r, 3),
        "`[a, b][1]` is `b`"
    );
}

/// A stored output written with no exec chain (a pure call) is one WS007 that
/// points at the call site, with no line break in its text.
#[test]
fn a_stored_output_in_a_pure_call_is_one_clear_ws007() {
    let r = compile(
        "in tick: exec\nin q: int\nin p: int\n\
         mod f() -> (r: int) {\n  out r = p\n  on tick { out r = q }\n}\nout o: int = f()",
    );
    let ws007: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code == "WS007")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(ws007.len(), 1, "{ws007:?}");
    assert!(
        ws007[0].contains("exec = ...") && !ws007[0].contains('\n') && !ws007[0].contains("  "),
        "{}",
        ws007[0]
    );
    // A branch that never lowers in a pure call still reports.
    let r = compile(
        "in c: bool\nin q: int\nin p: int\n\
         mod f(c: bool) -> (r: int) {\n  if c { emit r = q } else { emit r = p }\n}\nout o: int = f(c)",
    );
    assert_code_count(&r.diagnostics, "WS007", 1);
}

/// A stored write lowers its value once: a handler's `out a = xs.length()`
/// read the array's length twice.
#[test]
fn a_stored_write_lowers_its_value_once() {
    let r = compile_ok(
        "in tick: exec\nvar xs: int[]\n\
         mod f() -> (a: int, b: int) {\n  on tick {\n    out a = xs.length()\n    out b = xs.length()\n  }\n}\n\
         let { a, b } = f()\nout o: int = a + b",
    );
    assert_eq!(
        nodes_of(&r.module, crate::ir::gate_class::ARRAY_GET_LENGTH).len(),
        2
    );
}

/// The node wired into the input of the top-level `out` port on `line`.
fn out_port_source(r: &LowerResult, line: u32) -> Option<crate::ir::NodeId> {
    let out = r
        .module
        .outputs
        .iter()
        .copied()
        .find(|id| r.module.nodes[id].source_range.start.line == line)?;
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == out && w.target.port == WirePort::RerInput)
        .map(|w| w.source.node_id)
}

/// A mod whose output only a nested handler writes, called from pure context,
/// reads the output's storage directly: the handler's writes are what it shows.
#[test]
fn a_handler_written_mod_output_reads_its_storage_in_a_pure_call() {
    for (src, line) in [
        (
            "in tick: exec\nmod counter() -> (n: int) {\n  var k: int = 0\n  on tick { k = k + 1; out n = k }\n}\nout o = counter()",
            6,
        ),
        (
            "in tick: exec\nin q: int\nmod f() -> (r: int) {\n  on tick { out r = q }\n}\nout o = f()",
            6,
        ),
        (
            "in tick: exec\nin c: bool\nin q: int\nmod f() -> (r: int) {\n  on tick { if c { out r = q } }\n}\nout o = f()",
            7,
        ),
        (
            "in tick: exec\nin c: bool\nin q: int\nmod f() -> (r: int) {\n  on tick { if c { emit r = q } }\n}\nlet v = f()\nout o = v",
            8,
        ),
    ] {
        let r = compile_ok(src);
        let s = out_port_source(&r, line).expect("the output is driven");
        assert_eq!(
            r.module.nodes[&s].gate_class,
            crate::ir::gate_class::VAR,
            "{src}"
        );
        assert_eq!(r.module.nodes[&s].note, Some("ret_val"), "{src}");
    }
}

/// An output a path leaves unwritten keeps its last value, in a mod exactly as
/// in a chip: no reset at the start of a call, and a nested handler's write
/// stays visible until the caller's chain writes the output.
#[test]
fn an_unwritten_mod_output_holds_its_last_value() {
    for src in [
        "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
         mod K(c: bool) -> (r: int) {\n  if c { emit r = q }\n}\non go { m = K(c) }",
        "in go: exec\nin tick: exec\nin c: bool\nin q: int\nin p: int\nvar m: int = 0\n\
         mod f(c: bool) -> (r: int) {\n  if c { out r = q }\n  on tick { out r = p }\n}\non go { m = f(c) }",
    ] {
        let r = compile_ok(src);
        assert!(
            !r.module.nodes.values().any(|n| n.note == Some("ret_reset")),
            "no reset:\n{src}"
        );
        let get = r
            .module
            .nodes
            .values()
            .find(|n| n.note == Some("ret_get"))
            .map(|n| n.id);
        assert_eq!(m_value_source(&r), get, "m reads the held storage:\n{src}");
    }
}

/// A continuous `out` beside another `out` in a handler names the real second
/// write and does not suggest `emit`.
#[test]
fn a_continuous_out_beside_a_handler_out_says_what_fights() {
    let r = compile("in go: exec\nin q: int\nin x: int\nout r = x\non go { out r = q }");
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS013")
        .expect("WS013");
    assert!(!d.message.contains("emit"), "{}", d.message);
    assert!(d.message.contains("constant default"), "{}", d.message);
}

/// A pure read of a mod whose writes need an exec chain reports WS007 once,
/// for the output the read uses.
#[test]
fn ws007_for_a_pure_read_is_once_and_names_the_read_output() {
    let r = compile(
        "in go: exec\nin c: bool\nin live: int\nvar m: int = 0\n\
         mod f(c: bool) -> (a: int, b: int) {\n  out b = live\n  if c { return (1, 5) }\n  emit a = 2\n}\n\
         out o: int = f(c).b\non go { m = f(c).a }",
    );
    let ws007: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code == "WS007")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(ws007.len(), 1, "{ws007:?}");
    assert!(ws007[0].contains("`b`"), "{}", ws007[0]);
}

/// A multi-output call bound whole (`let res = f(c)`) does not also read its
/// first output as a single value: only `res.r` reads the storage.
#[test]
fn a_call_bound_as_a_record_reads_no_output_it_does_not_use() {
    let r = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
         mod f(c: bool) -> (r: int, done: exec) {\n  if c { out r = q }\n  emit done\n}\n\
         on go {\n  let res = f(c)\n  m = res.r\n}",
    );
    assert_eq!(nodes_of(&r.module, crate::ir::gate_class::VAR_GET).len(), 1);
}

/// `m = f(c).a` on a multi-output mod reads output `a` from the call's record
/// and nothing else: the call does not also read its first output as a single
/// value, which left an `Exec_Var_Get` on the chain whose value went nowhere.
#[test]
fn a_named_output_read_off_a_multi_output_call_reads_only_that_output() {
    let r = compile_ok(
        "in go: exec\nin c: bool\nin q: int\nvar m: int = 0\n\
         mod f(c: bool) -> (a: int, b: int) {\n  if c { emit a = q }\n  emit b = 9\n}\n\
         on go { m = f(c).a }",
    );
    let gets = nodes_of(&r.module, crate::ir::gate_class::VAR_GET);
    assert_eq!(gets.len(), 1, "only the read of `a`");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == gets[0].id && w.source.port == WirePort::Value),
        "the read feeds the store"
    );
}

/// No wire carries an exec pin into a data port.
fn assert_no_exec_as_value(r: &LowerResult) {
    for w in &r.module.wires {
        let (s, t) = (w.source.port.as_str(), w.target.port.as_str());
        assert!(
            !(s.starts_with("Exec") && !t.starts_with("Exec")),
            "exec pin {s} wired into data port {t}"
        );
    }
}

/// A `return` inside a block arm of a `match` used as a value leaves the mod
/// with that value, and the other arms' values reach the rest of the body: the
/// match stored nothing and handed its exec pin on as the value.
#[test]
fn a_return_in_a_value_match_arm_returns_from_the_mod() {
    let src = "enum E { A, B }\nin go: exec\nin e: bool\nvar r: int = 0\n\
               mod f(k: E) -> int {\n  let v = match k {\n    A => { return 7 },\n    B => 2,\n  }\n  return v + 1\n}\n\
               on go { r = f(if e then E.A else E.B) }";
    let r = compile_ok(src);
    assert!(!has_gate(&r, crate::ir::gate_class::UNSUPPORTED));
    assert_no_exec_as_value(&r);
    // `r` is stored from the return storage, written by both returns.
    let sets = nodes_of(&r.module, crate::ir::gate_class::VAR_SET);
    let store_r = sets
        .iter()
        .find(|n| n.source_range.start.line == 12)
        .expect("the store into r");
    let src_node = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == store_r.id && w.target.port == WirePort::Value)
        .map(|w| w.source.node_id)
        .expect("r's value is wired");
    assert_eq!(
        r.module.nodes[&src_node].gate_class,
        crate::ir::gate_class::VAR_GET
    );
    assert_eq!(r.module.nodes[&src_node].note, Some("ret_get"));
    assert_eq!(
        sets.iter().filter(|n| n.note == Some("ret_set")).count(),
        2,
        "both returns store"
    );
}

/// The same in a handler, where the arm's `return` ends the handler's run and
/// the other arm's value is stored.
#[test]
fn a_return_in_a_value_match_arm_ends_the_handler() {
    let r = compile_ok(
        "enum E { A, B }\nin go: exec\nvar e: E = E.A\nvar n: int = 0\n\
         on go { n = match e { A => { return }, B => 2 } }",
    );
    assert_no_exec_as_value(&r);
    let sets = nodes_of(&r.module, crate::ir::gate_class::VAR_SET);
    let store_n = sets
        .iter()
        .find(|n| n.source_range.start.col == 9)
        .expect("the store into n");
    let src_node = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == store_n.id && w.target.port == WirePort::Value)
        .map(|w| w.source.node_id)
        .expect("n's value is wired");
    assert_eq!(
        r.module.nodes[&src_node].gate_class,
        crate::ir::gate_class::VAR_GET
    );
}

/// A block arm of a value `match` that does not `return` has no value: WS095,
/// where it stored the arm's exec pin as the value.
#[test]
fn a_block_arm_of_a_value_match_must_return() {
    let head = "enum E { A, B }\nin go: exec\nvar e: E = E.A\nvar n: int = 0\nvar k: int = 0\n";
    assert!(
        errs(&format!(
            "{head}on go {{ n = match e {{ A => {{ k = 5 }}, B => 2 }} }}"
        ))
        .contains(&"WS095".to_string())
    );
    // A statement `match` runs its block arms for their effects.
    assert!(
        errs(&format!(
            "{head}on go {{ match e {{ A => {{ k = 5 }} B => {{ k = 2 }} }} }}"
        ))
        .is_empty()
    );
    assert!(
        errs(&format!(
            "{head}on go {{ n = match e {{ A => {{ return }}, B => 2 }} }}"
        ))
        .is_empty()
    );
}

/// Every output pin is driven by at most one wire: two drivers fail to load.
fn assert_no_output_fan_in(r: &LowerResult) {
    for (id, n) in &r.module.nodes {
        if n.gate_class != crate::ir::gate_class::MICROCHIP_OUTPUT {
            continue;
        }
        let drivers = r
            .module
            .wires
            .iter()
            .filter(|w| w.target.node_id == *id && w.target.port == WirePort::RerInput)
            .count();
        assert!(
            drivers <= 1,
            "output {:?} has {drivers} drivers",
            n.source_range
        );
    }
}

/// A record output emitted from two handlers, and a top-level output a mod
/// writes from two call sites, store into backing storage that drives the
/// output once, as a scalar output emitted from two handlers does. Both failed
/// at emit with a fan-in.
#[test]
fn an_output_written_from_two_places_drives_its_pin_once() {
    let rec = compile_ok(
        "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\nin go: exec\nin go2: exec\nout o: P\n\
         on go { emit o = p }\non go2 { emit o = { x: 9, y: 8 } }",
    );
    assert_no_output_fan_in(&rec);
    assert_eq!(
        gate_count(&rec, crate::ir::gate_class::VAR_SET),
        4,
        "two stores per field"
    );
    let modw = compile_ok(
        "var k: int = 1\nin go: exec\nin go2: exec\nout o: int\n\
         mod m(x: int) { emit o = x }\non go { m(k) }\non go2 { m(5) }",
    );
    assert_no_output_fan_in(&modw);
    assert_eq!(
        gate_count(&modw, crate::ir::gate_class::VAR_SET),
        2,
        "one store per call"
    );
    // Through a mod that calls it, once per outer call.
    let nested = compile_ok(
        "var k: int = 1\nin go: exec\nin go2: exec\nout o: int\n\
         mod m(x: int) { emit o = x }\nmod w(x: int) { m(x) }\non go { w(k) }\non go2 { w(5) }",
    );
    assert_no_output_fan_in(&nested);
}

/// Whether output `name` of `m` is driven by its backing var: one wire, from
/// a var, never a value wired straight in.
fn driven_by_backing(m: &crate::ir::Module, name: &str) -> bool {
    let out = m.nodes.iter().find(|(_, n)| {
        n.gate_class == crate::ir::gate_class::MICROCHIP_OUTPUT
            && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == name)
    });
    let Some((id, _)) = out else { return false };
    let drivers: Vec<_> = m
        .wires
        .iter()
        .filter(|w| w.target.node_id == *id && w.target.port == WirePort::RerInput)
        .collect();
    drivers.len() == 1 && m.nodes[&drivers[0].source.node_id].note == Some("out_backing")
}

/// An `emit` inside a `return`-ending block arm of a value `match` is a
/// conditional write, exactly like the same `emit` in a statement `match`: the
/// mod stores both writes of each output and the caller reads the one that
/// ran, and the arm's `return` goes on to the caller. Value and statement
/// forms lower alike. The value form stored literals and dropped the other
/// path's write; both forms dropped the caller's chain after the arm.
#[test]
fn an_emit_in_a_returning_match_arm_is_a_conditional_write() {
    let head = "enum K { A, B }\nin go: exec\nvar k: K = K.A\nvar x: int = 0\nvar y: int = 0\n";
    let tail = "on go {\n  let r = f(k)\n  x = r.a\n  y = r.b\n}";
    let value = "mod f(k: K) -> (a: int, b: int) {\n  let v = match k { A => { emit a = 1\n    emit b = 2\n    return }, B => 3 }\n  emit a = v\n  emit b = v + 10\n}\n";
    let stmt = "mod f(k: K) -> (a: int, b: int) {\n  var v: int = 0\n  match k { A => { emit a = 1\n    emit b = 2\n    return } B => { v = 3 } }\n  emit a = v\n  emit b = v + 10\n}\n";
    for body in [value, stmt] {
        let src = format!("{head}{body}{tail}");
        let r = compile_ok(&src);
        assert_no_exec_as_value(&r);
        let sets = nodes_of(&r.module, crate::ir::gate_class::VAR_SET);
        let line_of = |n: &crate::ir::Node| n.source_range.start.line;
        let tail_line = 6 + body.lines().count() as u32;
        let store = |line: u32| {
            *sets
                .iter()
                .find(|n| line_of(n) == line)
                .expect("the caller's store")
        };
        let (store_x, store_y) = (store(tail_line + 2), store(tail_line + 3));
        // Each caller store reads the mod's storage, not a literal.
        for s in [store_x, store_y] {
            let src_node = r
                .module
                .wires
                .iter()
                .find(|w| w.target.node_id == s.id && w.target.port == WirePort::Value)
                .map(|w| w.source.node_id)
                .unwrap_or_else(|| panic!("{src}\nthe store's value is wired"));
            assert_eq!(
                r.module.nodes[&src_node].gate_class,
                crate::ir::gate_class::VAR_GET,
                "{src}"
            );
        }
        // All four emits store, and the arm's last store reaches the caller.
        let emits: Vec<_> = sets
            .iter()
            .filter(|n| line_of(n) > 6 && line_of(n) < tail_line)
            .collect();
        assert_eq!(
            emits.len(),
            if body == value { 5 } else { 6 },
            "{src}\n{emits:?}"
        );
        let arm_b = emits
            .iter()
            .find(|n| line_of(n) == 8)
            .expect("`emit b = 2`");
        assert!(
            wired_reachable(&r, arm_b.id, store_x.id),
            "{src}\nthe arm's return goes on to the caller"
        );
        // `v + 10` is stored, not left dangling.
        let add = nodes_of(&r.module, "BrickComponentType_Expr_MathAdd");
        assert!(
            add.iter()
                .all(|a| r.module.wires.iter().any(|w| w.source.node_id == a.id)),
            "{src}"
        );
    }
}

/// In a handler, an `emit` of a top-level output inside a returning arm of a
/// value `match` gives the output a backing var and stores on the arm's chain,
/// as the statement form does. It drove the output with the constant, and
/// beside a second writer was an internal fan-in.
#[test]
fn an_emit_in_a_returning_match_arm_of_a_handler_backs_the_output() {
    let head = "enum K { A, B }\nin go: exec\nin go2: exec\nvar k: K = K.A\nvar n: int = 0\nout o: int\nout on_: int = n\n";
    for (handler, second) in [
        (
            "on go { n = match k { A => { emit o = 1\n  return }, B => 2 } }",
            "",
        ),
        (
            "on go { n = match k { A => { emit o = 1\n  return }, B => 2 } }",
            "\non go2 { emit o = 5 }",
        ),
        (
            "on go { match k { A => { emit o = 1\n  return } B => { n = 2 } } }",
            "",
        ),
    ] {
        let src = format!("{head}{handler}{second}");
        let r = compile_ok(&src);
        assert_no_output_fan_in(&r);
        assert!(driven_by_backing(&r.module, "o"), "{src}");
        let out_sets = r
            .module
            .nodes
            .values()
            .filter(|n| n.note == Some("out_set"))
            .count();
        assert_eq!(out_sets, 1 + usize::from(!second.is_empty()), "{src}");
    }
    // The same inside a named chip's handler.
    let r = compile_ok(
        "enum K { A, B }\nin go: exec\nvar k: K = K.A\n\
         chip C(t: exec) -> (o: int) {\n  var n: int = 0\n  on t {\n    n = match k { A => { emit o = 1\n      return }, B => 2 }\n    emit o = n + 5\n  }\n}\nout res = C(go)",
    );
    let chip = only_chip(&r, "the chip instance");
    assert!(driven_by_backing(chip, "o"));
    assert_eq!(
        chip.nodes
            .values()
            .filter(|n| n.note == Some("out_set"))
            .count(),
        2
    );
}

/// A top-level output written by a mod counts as conditional when the CALL
/// is: under an `if`, in a loop, through receiver syntax, or inside another
/// mod that calls it conditionally. Each drove the output straight from the
/// mod's value, constantly, or fanned two receiver calls into its pin.
#[test]
fn a_mod_writing_an_output_from_a_conditional_call_backs_it() {
    for src in [
        "in go: exec\nin c: bool\nout o: int\nmod setO(v: int) { emit o = v }\non go {\n  if c { setO(5) }\n}",
        "in go: exec\nout o: int\nmod setO(v: int) { emit o = v }\non go {\n  for i in 0..3 { setO(i) }\n}",
        "in go: exec\nin c: bool\nout o: float\nvar a: vector\nmod setV(self: vector) { emit o = self.X }\non go { if c { a.setV() } }",
        "in go: exec\nin go2: exec\nout o: float\nvar a: vector\nmod setV(self: vector) { emit o = self.X }\non go { a.setV() }\non go2 { a.setV() }",
        "in go: exec\nin c: bool\nout o: int\nmod setO(v: int) { emit o = v }\nmod viaIf(v: int) { if c { setO(v) } }\non go { viaIf(5) }",
    ] {
        let r = compile_ok(src);
        assert_no_output_fan_in(&r);
        assert!(driven_by_backing(&r.module, "o"), "{src}");
    }
    // A single unconditional call keeps its direct wire.
    let r = compile_ok(
        "in go: exec\nin k: int\nout o: int\nmod setO(v: int) { emit o = v }\non go { setO(k) }",
    );
    assert!(!driven_by_backing(&r.module, "o"));
}

/// A value `match` outside an exec context has no chain for a block arm to
/// run on, so its statements and `return` would vanish: WS095.
#[test]
fn a_block_arm_of_a_pure_value_match_is_ws095() {
    let head = "enum K { A, B }\nvar k: K = K.A\n";
    for decl in [
        "out o2: int = match k { A => { return 7 }, B => 2 }",
        "let v = match k { A => { return 7 }, B => 2 }\nout o: int = v",
    ] {
        assert!(
            errs(&format!("{head}{decl}")).contains(&"WS095".to_string()),
            "{decl}"
        );
    }
    // A value arm is fine there, and a block arm on a chain is too.
    assert!(errs(&format!("{head}out o2: int = match k {{ A => 7, B => 2 }}")).is_empty());
    assert!(errs(&format!("{head}in go: exec\nvar n: int = 0\non go {{ n = match k {{ A => {{ return }}, B => 2 }} }}")).is_empty());
}

/// A stored output's value is read before the store runs: the store's exec
/// follows the reads its value makes, so it never stores the value an
/// earlier run left behind.
#[test]
fn an_emit_into_a_backed_output_stores_after_its_reads() {
    let r = compile_ok(
        "in go: exec\nin c: bool\nvar n: int = 0\nout o: int\n\
         on go {\n  n = n + 1\n  if c { emit o = 1 }\n  emit o = n + 5\n}",
    );
    let source = |id: crate::ir::NodeId, port: WirePort| {
        r.module
            .wires
            .iter()
            .find(|w| w.target.node_id == id && w.target.port == port)
            .map(|w| w.source)
    };
    let add = nodes_of(&r.module, "BrickComponentType_Expr_MathAdd")
        .into_iter()
        .find(|a| a.source_range.start.line == 8)
        .expect("n + 5");
    let read = source(add.id, WirePort::InputA).expect("n's read").node_id;
    assert_eq!(
        r.module.nodes[&read].gate_class,
        crate::ir::gate_class::VAR_GET
    );
    let store = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.note == Some("out_set")
                && source(n.id, WirePort::Value).map(|s| s.node_id) == Some(add.id)
        })
        .expect("the store of n + 5");
    assert_eq!(
        source(store.id, WirePort::Exec).map(|s| s.node_id),
        Some(read),
        "the store runs after the read"
    );
}

#[test]
fn a_statement_after_an_if_whose_branches_all_buffer_emit_is_reported() {
    let src = "@left in go: bool\nvar n: int = 0\nvar done: int = 0\non go {\n  let sig: exec\n  buffer emit sig\n  await sig\n  n += 1\n\
               if n < 5 { buffer emit sig } else { buffer emit sig }\n  done += 1\n}\n";
    let (text, diags) = wsir_and_diags(src);
    assert!(diags.iter().any(|d| d.starts_with("WS058")), "{diags:?}");
    assert!(text.contains("emit arms await"), "{text}");
    assert!(!text.contains("%done.VarRef"), "{text}");
    let (_, diags) = wsir_and_diags(
        "@left in go: exec\nvar n: int = 0\non go {\n  let sig: exec\n  if n < 5 { buffer emit sig } else { buffer emit sig }\n  BroadcastChatMessage(\"x\")\n}\n",
    );
    assert!(diags.iter().any(|d| d.starts_with("WS058")), "{diags:?}");
    assert!(!diags.iter().any(|d| d.starts_with("WSP001")), "{diags:?}");
}

#[test]
fn a_statement_after_a_buffer_emit_in_the_same_block_is_reported() {
    let (text, diags) = wsir_and_diags(
        "@left in go: exec\nvar step: int = 0\non go {\n  let ramp: exec\n  step += 1\n  buffer emit ramp\n  step += 100\n}\n",
    );
    assert!(diags.iter().any(|d| d.starts_with("WS058")), "{diags:?}");
    assert!(!text.contains("Value: 100)"), "{text}");
    // The kick of a back-edge loop still arms the `await` that follows it.
    let (text, diags) = wsir_and_diags(
        "@left in go: exec\nvar step: int = 0\non go {\n  let ramp: exec\n  buffer emit ramp\n  await ramp\n  step += 1\n}\n",
    );
    assert!(diags.iter().all(|d| !d.starts_with("WS058")), "{diags:?}");
    assert!(
        text.lines().any(|l| l.contains("Exec_Var_Set")
            && l.contains("Exec: %go,")
            && l.contains("Value: true")),
        "{text}"
    );
}

#[test]
fn a_back_edge_to_an_await_on_an_outer_signal_in_a_branch_is_ws118() {
    let looped = "@left in go: exec\nvar i: int = 0\non go {\n  let s: exec\n  if i == 0 {\n    buffer emit s\n    await s\n\
                  if i < 5 {\n      i += 1\n      buffer emit s\n    }\n  }\n}\n";
    let (_, diags) = wsir_and_diags(looped);
    assert_eq!(
        diags.iter().filter(|d| d.starts_with("WS118")).count(),
        1,
        "{diags:?}"
    );
    // A one-tick delay in the branch has no back-edge.
    let delay = "@left in go: exec\nvar i: int = 0\non go {\n  let s: exec\n  if i == 0 {\n    buffer emit s\n    await s\n    i += 1\n  }\n}\n";
    let (_, diags) = wsir_and_diags(delay);
    assert!(diags.iter().all(|d| !d.starts_with("WS118")), "{diags:?}");
}
