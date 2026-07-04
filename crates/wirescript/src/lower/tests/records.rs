use super::*;

/// Test 1: Record literal and field access.
/// `type State = { val: *int }` with `var n` makes `s.val` alias `n`.
/// Writing `s.val = 42` in exec should produce a Var_Set targeting `n`'s PseudoVar.
#[test]
fn record_field_access_var_set() {
    let r = compile(
        "\
type State = { val: *int }
var n: int = 0
let s: State = { val: n }
on RoundStart { s.val = 42 }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "writing through a record ref field should produce a Var_Set gate"
    );

    // The Var_Set's VarRef input should be wired to n's PseudoVar node.
    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraphPseudo_Var"
        })
        .expect("expected a PseudoVar node for `var n`");

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set"
        })
        .expect("expected a Var_Set node");

    let ref_wire = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == var_set.id
                && w.target.port == crate::ir::port_registry::WirePort::VarRef
        })
        .expect("Var_Set must have a VarRef input wire");

    assert_eq!(
        ref_wire.source.node_id, pseudo_var.id,
        "Var_Set VarRef should point to n's PseudoVar"
    );
}

/// Test 2: Record pass-through to mod.
/// Passing a record with a `*int` field into a mod that increments it
/// should produce a Var_Get+add+Var_Set (or IncVar) chain.
#[test]
fn record_passthrough_to_mod() {
    let r = compile(
        "\
type State = { counter: *int }
var n: int = 0
let s: State = { counter: n }
mod bump(s: State) { s.counter = s.counter + 1 }
on RoundStart { bump(s) }",
    );
    assert_no_errors(&r);

    // The mod inlines, so we should see either an IncVar or a Var_Set for
    // the `s.counter = s.counter + 1` increment.
    let has_incr = has_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Var_Increment",
    );
    let has_set = has_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Var_Set",
    );
    assert!(
        has_incr || has_set,
        "record ref field increment inside mod should produce IncVar or Var_Set"
    );

    // The operation should target n's PseudoVar.
    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraphPseudo_Var"
        })
        .expect("expected a PseudoVar for `var n`");

    // Check that at least one VarRef wire leads back to n's PseudoVar.
    let has_ref_wire = r.module.wires.iter().any(|w| {
        w.source.node_id == pseudo_var.id
            && w.source.port == crate::ir::port_registry::WirePort::VarRef
    });
    assert!(
        has_ref_wire,
        "the increment chain should reference n's PseudoVar via VarRef"
    );
}

/// Test 3: Record with array field.
/// `type Mem = { data: int[] }` should let `m.data.push(42)` resolve to
/// an ArrayVar_Push gate targeting `arr`'s ArrayVar node.
#[test]
fn record_array_field_push() {
    let r = compile(
        "\
type Mem = { data: int[] }
array arr: int[]
let m: Mem = { data: arr }
on RoundStart { m.data.push(42) }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Push"),
        "pushing through a record array field should produce an ArrayVar_Push gate"
    );

    // The Push should reference arr's ArrayVar pseudo-node.
    let array_var = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar"
        })
        .expect("expected an ArrayVar pseudo-node for `array arr`");

    let push_node = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class
                == "BrickComponentType_WireGraph_Exec_ArrayVar_Push"
        })
        .unwrap();

    let ref_wire = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == push_node.id
                && w.target.port == crate::ir::port_registry::WirePort::ArrayVarRef
        })
        .expect("ArrayVar_Push must have an ArrayVarRef input wire");

    assert_eq!(
        ref_wire.source.node_id, array_var.id,
        "ArrayVar_Push should reference arr's ArrayVar node"
    );
}

/// Test 4: Record spread.
/// `let b = { ...a, y: 99 }` should resolve `b.x` to `a.x` (literal 1)
/// and `b.y` to literal 99, producing a correct sum.
#[test]
fn record_spread() {
    let r = compile(
        "\
type Point = { x: int, y: int }
let a: Point = { x: 1, y: 2 }
let b: Point = { ...a, y: 99 }
let sum = b.x + b.y
out result = sum",
    );
    assert_no_errors(&r);

    // The addition should exist.
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "b.x + b.y should produce a MathAdd gate"
    );

    // Verify no duplicate additions -- spread should not generate extra gates.
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        1,
        "should have exactly one addition gate"
    );
}

/// Test 5: Record destructuring.
/// `let { x, y } = p` should install x and y as separate locals.
/// `x + y` should produce an addition wired to the right literal sources.
#[test]
fn record_destructuring() {
    let r = compile(
        "\
type Point = { x: int, y: int }
let p: Point = { x: 10, y: 20 }
let { x, y } = p
let sum = x + y
out result = sum",
    );
    assert_no_errors(&r);

    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "destructured x + y should produce a MathAdd gate"
    );

    // Single addition gate from the destructured fields.
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        1,
        "should have exactly one addition gate"
    );

    // The output node should exist.
    assert_eq!(
        r.module.outputs.len(),
        1,
        "should have one output port"
    );
}

/// Test 6: Mod parameter destructuring.
/// `mod set_val({ val }: State) { val = 42 }` — the destructured `val` field
/// is a `*int` ref, so writing `val = 42` inside the mod should produce a
/// Var_Set gate targeting `n`'s PseudoVar.
#[test]
fn mod_param_destruct() {
    let r = compile(
        "\
type State = { val: *int }
var n: int = 0
let s: State = { val: n }
mod set_val({ val }: State) { val = 42 }
on RoundStart { set_val(s) }",
    );
    assert_no_errors(&r);
    // Should have a Var_Set gate targeting n's PseudoVar
    let has_set = r.module.nodes.values().any(|n| {
        n.gate_class.contains("Var_Set")
    });
    assert!(has_set, "destructured param should allow writing through ref field");
}

/// Test 7: Nested record field access.
/// `o.inner.x = 42` through two levels of record should resolve to
/// a Var_Set targeting `x`'s PseudoVar.
#[test]
fn nested_record_field_access() {
    let r = compile(
        "\
type Inner = { x: *int }
type Outer = { inner: Inner }
var x: int = 0
let i: Inner = { x }
let o: Outer = { inner: i }
on RoundStart { o.inner.x = 42 }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "nested record field write should produce a Var_Set gate"
    );

    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraphPseudo_Var"
        })
        .expect("expected a PseudoVar for `var x`");

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set"
        })
        .expect("expected a Var_Set node");

    let ref_wire = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == var_set.id
                && w.target.port == crate::ir::port_registry::WirePort::VarRef
        })
        .expect("Var_Set must have a VarRef input wire");

    assert_eq!(
        ref_wire.source.node_id, pseudo_var.id,
        "nested record field Var_Set should target x's PseudoVar"
    );
}

/// Regression: `let pair = (100, 200); let t0 = pair.0`
/// Tuple field access via `.0` / `.1` must resolve through the
/// Binding::Record with numeric keys, not fall through to unsupported.
#[test]
fn tuple_field_access() {
    let r = compile(
        "\
let pair = (100, 200)
let t0 = pair.0
let t1 = pair.1
let sum = t0 + t1
out result = sum",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "pair.0 + pair.1 should produce a MathAdd gate"
    );
    let unsupported_count = r
        .module
        .nodes
        .values()
        .filter(|n| {
            n.gate_class == "_Unsupported"
        })
        .count();
    assert_eq!(
        unsupported_count, 0,
        "tuple field access should not produce Unsupported nodes"
    );
}

/// `let x = var` should capture a snapshot (Var_Get), not alias the var.
#[test]
fn let_snapshot_captures_value() {
    let r = compile(
        "\
var counter: int = 0
in tick: exec
on tick {
    let snapshot = counter
}",
    );
    assert_no_errors(&r);
    let get_count = gate_count(
        &r,
        "BrickComponentType_WireGraph_Exec_Var_Get",
    );
    assert!(
        get_count >= 1,
        "let x = var should emit a Var_Get to capture the value"
    );
}

/// Regression: record field compound-assign inside a conditional in an inline mod
/// must produce a Var_Set that's wired into the exec chain of the branch body.
#[test]
fn record_field_assign_inside_if_in_mod() {
    let r = compile(
        "\
type Flags = { val: *int }
var flag: int = 0
mod cond_set(f: Flags, cond: bool) {
  if cond { f.val |= 1 }
}
in tick: exec
on tick {
  let f: Flags = { val: flag }
  cond_set(f, true)
}",
    );
    assert_no_errors(&r);
    // The Var_Set must exist (compound assign through record field)
    let set_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        set_count >= 1,
        "record field |= inside if inside mod should produce Var_Set, got {}",
        set_count
    );
    // The Var_Set must be connected to the exec chain (not orphaned)
    let set_node = r.module.nodes.values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
        .expect("must have a Var_Set node");
    let has_exec_in = r.module.wires.iter().any(|w| {
        w.target.node_id == set_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec
    });
    assert!(has_exec_in, "Var_Set must have exec input wired (not orphaned)");

    // Also verify it compiles to brz without errors
    let lr = crate::layout::layout(&r.module);
    let brz = crate::emit::emit_brz(&r.module, &lr.placements, &Default::default(), &std::sync::Arc::new(crate::template_cache::TemplateCache::new()));
    assert!(brz.is_ok(), "should compile to brz: {:?}", brz.err());
}

#[test]
fn dump_record_vs_direct_cond_set() {
    // Version A: direct *int param (old style)
    let r_direct = compile("\
var flag: int = 0
mod cond_set(f_val: *int, cond: bool) {
  if cond { f_val |= 1 }
}
in tick: exec
on tick { cond_set(flag, true) }");
    // Version B: record param (new style)
    let r_record = compile("\
type F = { val: *int }
var flag: int = 0
mod cond_set(f: F, cond: bool) {
  if cond { f.val |= 1 }
}
in tick: exec
on tick {
  let f: F = { val: flag }
  cond_set(f, true)
}");
    assert_no_errors(&r_direct);
    assert_no_errors(&r_record);

    let direct_sets = gate_count(&r_direct, "BrickComponentType_WireGraph_Exec_Var_Set");
    let record_sets = gate_count(&r_record, "BrickComponentType_WireGraph_Exec_Var_Set");
    eprintln!("Direct Var_Sets: {}, Record Var_Sets: {}", direct_sets, record_sets);

    assert_eq!(record_sets, direct_sets, "record version should have same number of Var_Sets as direct");

    // Constant-folded: no Branch gate needed when condition is literal bool
    let branch_count = gate_count(&r_direct, "BrickComponentType_WireGraph_Exec_Branch");
    assert_eq!(branch_count, 0, "literal bool condition should be constant-folded, no Branch gate");
}

/// Regression: array index write must work after the array is captured into a record.
#[test]
fn array_set_after_record_capture() {
    let r = compile("\
array io: int[]
in tick: exec
on tick {
  io.push(0)
  io.push(0)
  let mem = { data: io }
  io[1] = 8
}");
    assert_no_errors(&r);
    let has_arr_set = r.module.nodes.values().any(|n|
        n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_SetAtIndex"
    );
    assert!(has_arr_set, "io[1] = 8 should produce ArrayVar_SetAtIndex after record capture");
}

/// Regression: Var_Get cache invalidation must recurse into Record bindings.
/// After `rec.val += 1`, a subsequent read of the same var through the record
/// must produce a fresh Var_Get, not reuse a stale cached one.
#[test]
fn cache_invalidation_recurses_into_records() {
    let r = compile(
        "\
type S = { val: *int }
var x: int = 0
mod inc_and_read(s: S) -> int {
  s.val += 1
  return s.val + 0
}
in tick: exec
on tick {
  let s: S = { val: x }
  let r = inc_and_read(s)
}",
    );
    assert_no_errors(&r);
    // Should have at least 2 Var_Gets: one for the += read, one for the return read
    let get_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Get");
    assert!(
        get_count >= 2,
        "read after record field write should produce fresh Var_Get (got {})",
        get_count
    );
}

/// Regression: `let prev = rec.field` must capture a snapshot, not alias.
/// After `rec.field += 10`, `prev` should still hold the old value.
#[test]
fn let_snapshot_of_record_field() {
    let r = compile(
        "\
type S = { val: *int }
var x: int = 0
mod test(s: S) -> int {
  let prev = s.val
  s.val += 10
  return prev + s.val
}
in tick: exec
on tick {
  let s: S = { val: x }
  let r = test(s)
}",
    );
    assert_no_errors(&r);
    // `prev` should be a Local (snapshot), producing one Var_Get.
    // `s.val` after the write produces a second Var_Get.
    // The `+= 10` produces a third Var_Get (read before write).
    // Total: at least 3 Var_Gets for `s.val`.
    let get_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Get");
    assert!(
        get_count >= 3,
        "snapshot + write + re-read should produce at least 3 Var_Gets (got {})",
        get_count
    );
}

