use super::*;

#[test]
fn record_payload_ferries_and_destructures() {
    // `emit loop = { sum: 0, index: 0 }` on a local signal must write one
    // payload store per field, and `let { sum, index } = await loop` must read
    // them back into locals on the resumed chain.
    let src = "in run: exec\n\
               on run {\n\
                 let loop: exec\n\
                 emit loop = { sum: 1, index: 2 }\n\
                 let { sum, index } = await loop\n\
                 PrintToConsole(\"${sum} ${index}\")\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "destructured payload fields must resolve; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // 3 PseudoVars: sum store, index store, armed flag.
    let vars = gate_count(&r, "BrickComponentType_WireGraphPseudo_Var");
    assert!(
        vars >= 3,
        "expected 2 payload stores + armed flag, got {vars}"
    );
    // 2 payload writes on the emit chain (+ arm/reset sets).
    let sets = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        sets >= 4,
        "expected per-field Var_Set + arm/reset, got {sets}"
    );
    let run = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let cont = find_gate(&r, "BrickComponentType_WireGraph_Exec_PrintToConsole");
    assert!(
        wired_reachable(&r, run, cont),
        "continuation must be exec-wired"
    );
}

#[test]
fn record_literal_return_destructures_to_fields() {
    // A mod with an anonymous record return (`-> { head, rest }` via
    // `return { head: ..., rest: ... }`) must wire each field to its own source,
    // not collapse to a single `_Unsupported` gate the caller can't destructure.
    // Here head/rest come from a Split's Left/Right, so f.head and f.rest must
    // resolve to two DISTINCT ports.
    let src = "mod field(t: string) -> {head: string, rest: string} {\n\
               let p = t.Split(\" \")\n\
               return { head: p.Left, rest: p.Right }\n\
               }\n\
               in s: string\n\
               let f = field(s)\n\
               out h = f.head\n\
               out r = f.rest";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "record-literal return must not lower to _Unsupported; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // The two outputs (h = f.head, r = f.rest) must be fed by DISTINCT source
    // ports (Split.Left vs Split.Right) - the bug wired both to one gate.
    let out_sources: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| r.module.outputs.contains(&w.target.node_id))
        .map(|w| (w.source.node_id, w.source.port))
        .collect();
    assert_eq!(out_sources.len(), 2, "two outputs, got {out_sources:?}");
    assert_ne!(
        out_sources[0], out_sources[1],
        "f.head and f.rest must read distinct ports, got {out_sources:?}"
    );
}

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
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("expected a PseudoVar node for `var n`");

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
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
    let has_incr = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment");
    let has_set = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        has_incr || has_set,
        "record ref field increment inside mod should produce IncVar or Var_Set"
    );

    // The operation should target n's PseudoVar.
    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
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
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .expect("expected an ArrayVar pseudo-node for `array arr`");

    let push_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
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
    assert_eq!(r.module.outputs.len(), 1, "should have one output port");
}

/// Regression: a record LITERAL passed directly to a destructured VALUE param
/// must bind its fields — not lower to an `_Unsupported` value port. This is
/// the roles.ws bug: `addRole(next, { team: T_GREY, … })` pushed all-default
/// rows because the literal arg never became a `Binding::Record`, so the
/// destructuring was skipped. A record *variable* arg already worked; only a
/// record *literal* arg was broken.
#[test]
fn record_literal_arg_to_destructured_param() {
    let r = compile(
        "\
type P = { a: int, b: int }
in x: int
mod f({ a, b }: P) -> int { return a + b }
out result = f({ a: x, b: 1 })",
    );
    assert_no_errors(&r);
    let unsupported = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "_Unsupported")
        .count();
    assert_eq!(
        unsupported, 0,
        "record literal arg to a destructured param must not produce _Unsupported"
    );
    // `a` binds to `x` (non-constant) so `a + b` can't be folded away; the add
    // surviving proves the destructured field value actually flowed in.
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "destructured field `a` (= x) + b should produce a MathAdd"
    );
}

/// Regression: a record LITERAL passed to a WHOLE-record param must bind as a
/// record so `p.field` resolves (same root cause / fix as the destructured
/// case). `main.ws` dodged this only by passing record *variables*.
#[test]
fn record_literal_arg_to_record_param() {
    let r = compile(
        "\
type P = { a: int, b: int }
in x: int
mod g(p: P) -> int { return p.a + p.b }
out result = g({ a: x, b: 1 })",
    );
    assert_no_errors(&r);
    let unsupported = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "_Unsupported")
        .count();
    assert_eq!(
        unsupported, 0,
        "record literal arg to a whole-record param must not produce _Unsupported"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "p.a (= x) + p.b should produce a MathAdd"
    );
}

/// Regression: a top-level `let` constant referenced inside a chip body must
/// resolve — `Binding::Local` (constant lets) is inherited into the chip scope,
/// not dropped. This was the second half of the roles.ws bug: even after
/// record-literal args destructured, `{ team: T_GREY }` inside the `InitRoles`
/// chip pushed 0 because the const `T_GREY` was invisible in the chip body and
/// lowered to an `_Unsupported` default. Mirrors roles' non-foldable path:
/// a const field value `.push()`ed into a top-level array from inside a chip.
#[test]
fn top_level_const_visible_inside_chip() {
    let r = compile(
        "\
let TG = 2
type RD = { tm: int }
array teams: int[]
mod addR(next: *int, { tm }: RD) -> int {
  teams.push(tm)
  let code = next
  next = next + 1
  return code
}
chip Init() -> (A: int) {
  var nxt: int = 0
  emit A = addR(nxt, { tm: TG })
}
in go: exec
let I = Init(exec = go)",
    );
    assert_no_errors(&r);
    // No `_Unsupported` anywhere — including inside the chip body, where the
    // pushed const value used to lower to an unsupported placeholder (→ 0).
    let mut unsupported = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "_Unsupported")
        .count();
    for (_, child) in &r.module.chips {
        unsupported += child
            .nodes
            .values()
            .filter(|n| n.gate_class == "_Unsupported")
            .count();
    }
    assert_eq!(
        unsupported, 0,
        "top-level const inside a chip must resolve, not lower to _Unsupported"
    );
}

/// Regression (roles bug): a top-level constant used inside a chip must reach
/// its consumer as INLINE gate data, with no value wire crossing the chip
/// boundary. The const's `_Literal` is cloned into the chip, then folded into
/// the consumer by `inline_orphan_literals`.
#[test]
fn const_inlines_into_chip_gate_no_boundary_wire() {
    let r = compile(
        "let TG = 2\ntype RD = { tm: int }\narray teams: int[]\nmod addR(next: *int, { tm }: RD) -> int {\n  teams.push(tm)\n  let code = next\n  next = next + 1\n  return code\n}\nchip Init() -> (A: int) {\n  var nxt: int = 0\n  emit A = addR(nxt, { tm: TG })\n}\nin go: exec\nlet I = Init(exec = go)",
    );
    assert_no_errors(&r);
    for chip in r.module.chips.values() {
        let push = chip
            .nodes
            .values()
            .find(|n| n.gate_class.contains("ArrayVar_Push"))
            .expect("expected an ArrayVar_Push in the chip");
        // The constant reached the push as INLINE data (Value = 2), not a wire.
        assert_eq!(
            push.properties.get(&crate::intern::intern("Value")),
            Some(&crate::ir::Literal::Int(2)),
            "const should inline into the push's Value data as 2"
        );
        // No VALUE wire crosses the boundary: the push's `Value` input must not
        // be fed by a wire from outside the chip (ref ports like ArrayVarRef may
        // legitimately cross; value ports may not).
        let value_sym = crate::intern::intern("Value");
        for w in &chip.wires {
            if w.target.node_id == push.id
                && crate::intern::intern(w.target.port.as_str()) == value_sym
            {
                assert!(
                    chip.nodes.contains_key(&w.source.node_id),
                    "the push Value is fed by a cross-boundary wire"
                );
            }
        }
    }
}

/// Regression: a top-level `in` port referenced inside a chip body must
/// resolve. Chips close over the whole module-global (ROOT) scope, so inputs
/// are visible just like vars/consts — not dropped by a per-type whitelist.
#[test]
fn top_level_input_visible_inside_chip() {
    let r = compile(
        "\
in y: int
array out_arr: int[]
chip C() { out_arr.push(y) }
in go: exec
let I = C(exec = go)",
    );
    assert_no_errors(&r);
    let mut unsupported = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "_Unsupported")
        .count();
    for (_, child) in &r.module.chips {
        unsupported += child
            .nodes
            .values()
            .filter(|n| n.gate_class == "_Unsupported")
            .count();
    }
    assert_eq!(
        unsupported, 0,
        "a top-level input referenced inside a chip must resolve, not lower to _Unsupported"
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
    let has_set = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class.contains("Var_Set"));
    assert!(
        has_set,
        "destructured param should allow writing through ref field"
    );
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
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("expected a PseudoVar for `var x`");

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
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
        .filter(|n| n.gate_class == "_Unsupported")
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
    let get_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Get");
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
    let set_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
        .expect("must have a Var_Set node");
    let has_exec_in = r.module.wires.iter().any(|w| {
        w.target.node_id == set_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec
    });
    assert!(
        has_exec_in,
        "Var_Set must have exec input wired (not orphaned)"
    );

    // Also verify it compiles to brz without errors
    let lr = crate::layout::layout(&r.module);
    let brz = crate::emit::emit_brz(
        &r.module,
        &lr,
        &Default::default(),
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    );
    assert!(brz.is_ok(), "should compile to brz: {:?}", brz.err());
}

#[test]
fn dump_record_vs_direct_cond_set() {
    // Version A: direct *int param (old style)
    let r_direct = compile(
        "\
var flag: int = 0
mod cond_set(f_val: *int, cond: bool) {
  if cond { f_val |= 1 }
}
in tick: exec
on tick { cond_set(flag, true) }",
    );
    // Version B: record param (new style)
    let r_record = compile(
        "\
type F = { val: *int }
var flag: int = 0
mod cond_set(f: F, cond: bool) {
  if cond { f.val |= 1 }
}
in tick: exec
on tick {
  let f: F = { val: flag }
  cond_set(f, true)
}",
    );
    assert_no_errors(&r_direct);
    assert_no_errors(&r_record);

    let direct_sets = gate_count(&r_direct, "BrickComponentType_WireGraph_Exec_Var_Set");
    let record_sets = gate_count(&r_record, "BrickComponentType_WireGraph_Exec_Var_Set");
    eprintln!(
        "Direct Var_Sets: {}, Record Var_Sets: {}",
        direct_sets, record_sets
    );

    assert_eq!(
        record_sets, direct_sets,
        "record version should have same number of Var_Sets as direct"
    );

    // Constant-folded: no Branch gate needed when condition is literal bool
    let branch_count = gate_count(&r_direct, "BrickComponentType_WireGraph_Exec_Branch");
    assert_eq!(
        branch_count, 0,
        "literal bool condition should be constant-folded, no Branch gate"
    );
}

/// Regression: array index write must work after the array is captured into a record.
#[test]
fn array_set_after_record_capture() {
    let r = compile(
        "\
array io: int[]
in tick: exec
on tick {
  io.push(0)
  io.push(0)
  let mem = { data: io }
  io[1] = 8
}",
    );
    assert_no_errors(&r);
    let has_arr_set = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_SetAtIndex");
    assert!(
        has_arr_set,
        "io[1] = 8 should produce ArrayVar_SetAtIndex after record capture"
    );
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
