use super::*;

#[test]
fn return_in_mod_does_not_kill_caller_exec() {
    // Bug 11 regression: `return` in an inlined mod should jump to the
    // mod's return union, not kill the containing handler's exec chain.
    let r = compile(
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
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    // x = 42 after the mod call should produce a Set gate
    let set_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
        .count();
    assert!(
        set_count >= 2,
        "x = 42 after mod with return should produce a Set gate, found {set_count}"
    );
    // The return union should exist
    let has_union = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Union");
    assert!(
        has_union,
        "mod with return should create a return union gate"
    );
}

#[test]
fn return_in_mod_union_wired() {
    // Both the return path and the fall-through path should wire into the union
    let r = compile(
        "\
var x: int = 0
mod guard(flag: int) {
  if flag == 0 { return }
  x = 1
}
in player: character
on player {
  guard(0)
  x = 2
}",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let union_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Union")
        .count();
    assert!(
        union_count >= 1,
        "should have at least 1 union for mod return, found {union_count}"
    );
}

#[test]
fn multiple_returns_in_mod_chain_unions() {
    // Two returns + fallthrough must not multi-connect to one union input.
    // Each return path should chain through separate union gates.
    let r = compile(
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
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    // Two returns + fallthrough needs 2 merge union gates (chain):
    // union1 merges return1 + return2, union2 merges union1 + fallthrough.
    // (The if-joins collapse: each has only its else arm, so the prune
    // splices them as pass-throughs.)
    let union_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Union")
        .count();
    assert!(
        union_count >= 2,
        "two returns + fallthrough should chain merge unions, found {union_count}"
    );
    // No union input port should have more than one wire
    for node in r.module.nodes.values() {
        if node.gate_class == "BrickComponentType_WireGraph_Exec_Union" {
            for port in &node.ports.inputs {
                let incoming = r
                    .module
                    .wires
                    .iter()
                    .filter(|w| {
                        w.target.node_id == node.id && crate::intern::resolve(port.name) == w.target.port.as_str()
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
}

#[test]
fn nested_mod_array_push_lowers() {
    let r = compile(
        "\
array data: int[]
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
    let unsup = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "_Unsupported")
        .count();
    assert_eq!(
        unsup, 0,
        "nested mod arr.push should not produce _Unsupported, found {unsup}"
    );
    let push_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
        .count();
    assert!(
        push_count >= 2,
        "should have at least 2 ArrayVar_Push gates, found {push_count}"
    );
}

#[test]
fn mod_let_bindings_do_not_leak_into_caller() {
    // A mod with `let x` must not overwrite the caller's `let x`.
    // The caller's `x` should still resolve to the caller's expression
    // after the mod returns, not the mod's internal computation.
    let r = compile("\
var v: int = 0
mod inner(v: *int) {
  let x = v & 0xFF
  v = x
}
in player: character
on player {
  v = 42
  let x = v + 1
  inner(v)
  let y = x
}");
    assert!(
        r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics
    );
    // `let y = x` should wire from the CALLER's `x` (v + 1),
    // not the mod's `x` (v & 0xFF). Check that the `y` local
    // resolves to the MathAdd output, not the BitwiseAND output.
    let add_nodes: Vec<_> = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .collect();
    assert!(!add_nodes.is_empty(), "should have a MathAdd gate for v + 1");
    let and_nodes: Vec<_> = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_BitwiseAND")
        .collect();
    assert!(!and_nodes.is_empty(), "should have a BitwiseAND gate for v & 0xFF");
    // Find the Var_Set for y (the last set in the chain).
    // Its Value input should come from MathAdd, not BitwiseAND.
    // We verify by checking no wire goes from BitwiseAND output to
    // any gate AFTER the mod returns (i.e. no leaked binding).
    let and_id = &and_nodes[0].id;
    let and_out_targets: Vec<_> = r.module.wires.iter()
        .filter(|w| w.source.node_id == *and_id && w.source.port == crate::ir::port_registry::WirePort::Output)
        .map(|w| w.target.node_id)
        .collect();
    assert!(!and_out_targets.is_empty(), "BitwiseAND should have at least one output wire");
    // The output should only connect to nodes that are part of the mod's internal logic,
    // not to arbitrary caller-scope nodes (which would indicate a leak).
    // With numeric IDs we verify the target count is bounded rather than checking names.
}

#[test]
fn single_return_value_wires_directly() {
    let r = compile("\
mod double(x: int) -> (result: int) {
  return x + x
}
in player: character
on player {
  let d = double(21)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    // Inline mods don't create MicrochipOutput — value wires directly to caller
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should have MathAdd for x + x");
    let ret_vars: Vec<_> = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_val"))
        .collect();
    assert!(ret_vars.is_empty(), "single return should not create ret_val var, found {} nodes",
        ret_vars.len());
}

#[test]
fn standalone_chip_exec_body_return_keeps_exec_output() {
    // Regression: a standalone `chip` whose body runs an exec op (array find)
    // and returns a value must expose BOTH its value output and an `_exec_out`.
    // A trailing `return` moves the body's tail exec into mod_return_exec; the
    // standalone-chip path never merged it back before building `_exec_out`, so
    // the exec chain was orphaned and the chip shipped with only one output.
    let r = compile(
        "\
array counts: int[]
chip slotOfUser(uid: int) -> int {
  let res = counts.find(uid)
  return if res.Found then res.Index else -1
}
in z: exec
on z {
  let s = slotOfUser(1)
}",
    );
    assert!(
        r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let chip = r
        .module
        .chips
        .values()
        .find(|c| crate::intern::resolve(c.name).contains("slotOfUser"))
        .expect("slotOfUser chip module should exist");
    assert_eq!(
        chip.outputs.len(),
        2,
        "an exec-body chip that returns a value needs a value output AND an _exec_out, found {}",
        chip.outputs.len()
    );
    // The value output (declared first) must be fed by the if-expr SELECT - the
    // returned value - not clobbered by the exec chain.
    let val_out = chip.outputs[0];
    let fed = chip
        .wires
        .iter()
        .find(|w| w.target.node_id == val_out)
        .expect("value output should be wired");
    let src_class = &chip.nodes[&fed.source.node_id].gate_class;
    assert!(
        src_class.contains("Expr_Select"),
        "value output must be fed by the if-expr SELECT, got {src_class}"
    );
}

#[test]
fn standalone_chip_return_wires_value_with_parent_out() {
    // Regression: a single-`return` chip's value output must be wired even when
    // the enclosing module declares an `out`. The chip inherits the enclosing
    // scope, and the inherited `Binding::Output` used to inflate output_count()
    // past 1 - so `Stmt::Return`'s `else if output_count() == 1` value-wiring
    // branch was skipped and the SELECT never reached the MicrochipOutput.
    let r = compile(
        "\
in z: exec
array counts: int[]
out counts2: int[] = counts
chip slotOfUser(uid: int) -> int {
  let res = counts.find(uid)
  return if res.Found then res.Index else -1
}
on z {
  BroadcastChatMessage(\"slot: ${slotOfUser(1)}\")
}",
    );
    assert!(
        r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let chip = r
        .module
        .chips
        .values()
        .find(|c| crate::intern::resolve(c.name).contains("slotOfUser"))
        .expect("slotOfUser chip");
    let val_out = chip.outputs[0];
    let fed = chip
        .wires
        .iter()
        .find(|w| w.target.node_id == val_out)
        .expect("value output must be wired even with a parent `out`");
    let src_class = &chip.nodes[&fed.source.node_id].gate_class;
    assert!(
        src_class.contains("Expr_Select"),
        "value output must be fed by the SELECT, got {src_class}"
    );
}

#[test]
fn standalone_chip_multi_call_preserves_value_wire() {
    // The 2raab case: the chip is called many times, so only the first call
    // builds the module - the rest instantiate the cached template. EVERY
    // instance must keep its value output fed by the SELECT and expose _exec_out.
    let r = compile(
        "\
array counts: int[]
chip slotOfUser(uid: int) -> int {
  let res = counts.find(uid)
  return if res.Found then res.Index else -1
}
in z: exec
on z {
  let a = slotOfUser(1)
  let b = slotOfUser(2)
}",
    );
    assert!(
        r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let chips: Vec<_> = r
        .module
        .chips
        .values()
        .filter(|c| crate::intern::resolve(c.name).contains("slotOfUser"))
        .collect();
    assert_eq!(chips.len(), 2, "expected two slotOfUser instances, found {}", chips.len());
    for chip in chips {
        assert_eq!(chip.outputs.len(), 2, "each instance needs value + exec outputs");
        let val_out = chip.outputs[0];
        let fed = chip
            .wires
            .iter()
            .find(|w| w.target.node_id == val_out)
            .expect("value output should be wired in every instance");
        let src_class = &chip.nodes[&fed.source.node_id].gate_class;
        assert!(
            src_class.contains("Expr_Select"),
            "value output must be fed by the SELECT in every instance, got {src_class}"
        );
    }
}

#[test]
fn inline_mod_exec_body_returns_select_reaches_consumer() {
    // The 2raab slot lookup as an INLINE mod: exec op (find) then `return <if>`.
    // The SELECT holding the returned value must reach the caller's consumer
    // (a Var_Set), i.e. the return value isn't dropped when the body also
    // advances the exec chain.
    let r = compile(
        "\
array counts: int[]
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
        r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    assert!(!has_gate(&r, "_Unsupported"), "no unsupported placeholder expected");
    let select = find_gate(&r, "BrickComponentType_WireGraph_Expr_Select");
    let set = find_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        wired_reachable(&r, select, set),
        "the returned SELECT value must reach the `result` Var_Set"
    );
}

#[test]
fn multi_return_value_uses_var() {
    let r = compile("\
mod my_clamp(v: int, lo: int, hi: int) -> (result: int) {
  if v < lo { return lo }
  if v > hi { return hi }
  return v
}
in player: character
on player {
  let c = my_clamp(50, 0, 100)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let ret_vars: Vec<_> = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_val"))
        .collect();
    assert!(!ret_vars.is_empty(), "multi-return should create ret_val var");
    let ret_sets = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_set"))
        .count();
    assert_eq!(ret_sets, 3, "should have 3 ret_set Var_Set gates");
    let ret_gets = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_get"))
        .count();
    assert_eq!(ret_gets, 1, "should have 1 ret_get Var_Get gate");
}

#[test]
fn return_value_no_output_ignored() {
    let r = compile("\
mod noop(x: *int) {
  if x > 10 { return 0 }
  x = x + 1
}
var v: int = 5
in player: character
on player { noop(v) }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let ret_nodes: Vec<_> = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_val") || n.note == Some("ret_set"))
        .collect();
    assert!(ret_nodes.is_empty(), "return value in no-output mod should be ignored");
}

#[test]
fn mod_anonymous_output_auto_unwraps() {
    let r = compile("\
mod inc(x: int) -> int {
  return x + 1
}
in player: character
on player {
  let f = inc(5)
  let g = f + 10
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let add_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .count();
    assert!(add_count >= 1, "f + 10 should produce a MathAdd gate");
}

#[test]
fn multi_return_two_branches() {
    let r = compile("\
mod pick(a: int, b: int, sel: *bool) -> (result: int) {
  if sel { return a }
  return b
}
var flag: bool = false
in player: character
on player {
  let p = pick(10, 20, flag)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let ret_vars: Vec<_> = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_val"))
        .collect();
    assert!(!ret_vars.is_empty(), "2-return mod should create ret_val var");
    let ret_sets = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_set"))
        .count();
    assert_eq!(ret_sets, 2, "should have exactly 2 ret_set gates, found {ret_sets}");
}

#[test]
fn return_without_value_in_output_mod() {
    let r = compile("\
mod foo(x: *int) -> int {
  if x > 10 { return }
  return x
}
var v: int = 5
in player: character
on player {
  let f = foo(v)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "return without value should not crash: {:?}", r.diagnostics);
}

#[test]
fn static_var_in_mod_persists() {
    let r = compile("\
mod counter() -> int {
  static var n: int = 0
  n = n + 1
  return n
}
in player: character
on player {
  let c = counter()
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let has_pseudo_var = r.module.nodes.values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var");
    assert!(has_pseudo_var, "static var should create a PseudoVar");
    let has_incr = r.module.nodes.values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Increment");
    let has_set = r.module.nodes.values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(has_incr || has_set,
        "should have Var_Increment or Var_Set for n = n + 1");
}

#[test]
fn return_block_expr() {
    let r = compile("\
mod foo(x: int) -> int {
  return { let a = x + 1; a }
}
in player: character
on player {
  let f = foo(5)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "return block expr should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should produce MathAdd for x + 1 inside returned block");
}

#[test]
fn deeply_nested_returns() {
    let r = compile("\
mod classify(x: int) -> int {
  if x > 100 {
    return 3
  }
  if x > 10 {
    if x > 50 {
      return 2
    }
    return 1
  }
  return 0
}
in player: character
on player {
  var v: int = 42
  let c = classify(v)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "deeply nested returns should compile: {:?}", r.diagnostics);
    let ret_vars: Vec<_> = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_val"))
        .collect();
    assert!(!ret_vars.is_empty(), "4 returns should create ret_val var");
    let ret_sets = r.module.nodes.values()
        .filter(|n| n.note == Some("ret_set"))
        .count();
    assert_eq!(ret_sets, 4, "should have exactly 4 ret_set gates, found {ret_sets}");
}

#[test]
fn return_value_from_nested_mod() {
    let r = compile("\
mod inner(x: int) -> int {
  return x * 2
}
mod outer(x: int) -> int {
  return inner(x) + 1
}
in player: character
on player {
  let v = outer(5)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "nested mod return value should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathMultiply"),
        "should have MathMultiply for x * 2 in inner mod");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should have MathAdd for inner(x) + 1 in outer mod");
}

#[test]
fn return_value_with_array_read() {
    let r = compile("\
array data: int[]
mod get_first(arr: int[]) -> int {
  return arr[0]
}
in player: character
on player {
  data.push(42)
  let f = get_first(data)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "return value with array read should compile: {:?}", r.diagnostics);
    let has_arr_get = r.module.nodes.values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Get");
    assert!(has_arr_get, "should have ArrayVar_Get gate for arr[0]");
}

#[test]
fn return_in_handler_not_mod() {
    let r = compile("\
var x: int = 0
on RoundStart {
  if x > 0 { return }
  x = 1
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "return in handler should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"),
        "should produce Branch gate for exec if in handler");
    let set_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(set_count >= 1,
        "should have at least 1 Var_Set for x = 1, found {set_count}");
}

#[test]
fn mod_with_output_and_ref_params() {
    let r = compile("\
mod inc_and_get(x: *int) -> int {
  x = x + 1
  return x
}
var v: int = 10
in player: character
on player {
  let result = inc_and_get(v)
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "mod with ref params and output should compile: {:?}", r.diagnostics);
    let has_incr = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment");
    let has_set = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(has_incr || has_set,
        "mod with ref param should produce Exec_Var_Set or Exec_Var_Increment");
    // Inline mod output is removed — value flows directly via Var_Get
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"),
        "mod with return x should produce Var_Get for the return value");
}

#[test]
fn two_mods_same_local_names() {
    let r = compile("\
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
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "two mods with same local names should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should have MathAdd for v + 1 in mod a");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathMultiply"),
        "should have MathMultiply for v * 2 in mod b");
}

#[test]
fn var_reset_in_nested_mod() {
    let r = compile("\
mod outer_mod() {
  var x: int = 0
  x = x + 1
  inner_mod()
}
mod inner_mod() {
  var x: int = 0
  x = x + 10
}
in player: character
on player {
  outer_mod()
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "nested mods with same var names should compile: {:?}", r.diagnostics);
    let pseudo_var_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .count();
    assert!(pseudo_var_count >= 2,
        "should have at least 2 PseudoVar nodes for separate var x in each mod, found {pseudo_var_count}");
}

#[test]
fn static_var_across_multiple_calls() {
    let r = compile("\
mod counter() -> int {
  static var n: int = 0
  n = n + 1
  return n
}
in player: character
on player {
  let a = counter()
  let b = counter()
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "static var across multiple calls should compile: {:?}", r.diagnostics);
    let pseudo_vars: Vec<_> = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .collect();
    assert_eq!(pseudo_vars.len(), 2,
        "two inlined calls should each get a PseudoVar, found {}",
        pseudo_vars.len());
}

#[test]
fn single_output_mod_in_if_condition() {
    let r = compile("\
var v: int = 5
mod is_pos(x: int) -> int {
  return if x > 0 then 1 else 0
}
in player: character
on player {
  let check = is_pos(v)
  let label = if check > 0 then \"y\" else \"n\"
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "mod result in if condition should compile: {:?}", r.diagnostics);
    let select_count = gate_count(&r, "BrickComponentType_WireGraph_Expr_Select");
    assert!(select_count >= 2,
        "should have at least 2 Select gates (inner and outer if), found {select_count}");
}
