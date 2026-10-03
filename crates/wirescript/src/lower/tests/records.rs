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
    let vars = gate_count(&r, "BrickComponentType_Var");
    assert!(
        vars >= 3,
        "expected 2 payload stores + armed flag, got {vars}"
    );
    // 2 payload writes on the emit chain (+ arm/reset sets).
    let sets = gate_count(&r, "BrickComponentType_Exec_Var_Set");
    assert!(
        sets >= 4,
        "expected per-field Var_Set + arm/reset, got {sets}"
    );
    let run = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let cont = find_gate(&r, "BrickComponentType_Exec_PrintToConsole");
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

/// Record literal and field access.
/// `type State = { val: *int }` with `var n` makes `s.val` alias `n`.
/// Writing `s.val = 42` in exec should produce a Var_Set targeting `n`'s PseudoVar.
#[test]
fn record_field_access_var_set() {
    let r = compile(
        "\
type State = { val: *int }
var n: int = 0
let s: State = { val: n }
on RoundStart() { s.val = 42 }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "writing through a record ref field should produce a Var_Set gate"
    );

    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Var")
        .expect("expected a PseudoVar node for `var n`");

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
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

/// Record pass-through to mod.
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
on RoundStart() { bump(s) }",
    );
    assert_no_errors(&r);

    // The mod inlines, so we should see either an IncVar or a Var_Set for
    // the `s.counter = s.counter + 1` increment.
    let has_incr = has_gate(&r, "BrickComponentType_Exec_Var_Increment");
    let has_set = has_gate(&r, "BrickComponentType_Exec_Var_Set");
    assert!(
        has_incr || has_set,
        "record ref field increment inside mod should produce IncVar or Var_Set"
    );

    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Var")
        .expect("expected a PseudoVar for `var n`");

    let has_ref_wire = r.module.wires.iter().any(|w| {
        w.source.node_id == pseudo_var.id
            && w.source.port == crate::ir::port_registry::WirePort::VarRef
    });
    assert!(
        has_ref_wire,
        "the increment chain should reference n's PseudoVar via VarRef"
    );
}

/// Record with array field.
/// `type Mem = { data: int[] }` should let `m.data.push(42)` resolve to
/// an ArrayVar_Push gate targeting `arr`'s ArrayVar node.
#[test]
fn record_array_field_push() {
    let r = compile(
        "\
type Mem = { data: int[] }
var arr: int[]
let m: Mem = { data: arr }
on RoundStart() { m.data.push(42) }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_Exec_ArrayVar_Push"),
        "pushing through a record array field should produce an ArrayVar_Push gate"
    );

    let array_var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_ArrayVar")
        .expect("expected an ArrayVar pseudo-node for `array arr`");

    let push_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Exec_ArrayVar_Push")
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

/// Record spread.
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

    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "b.x + b.y should produce a MathAdd gate"
    );

    // Spread must not generate extra addition gates.
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_MathAdd"),
        1,
        "should have exactly one addition gate"
    );
}

/// Record destructuring.
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
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "destructured x + y should produce a MathAdd gate"
    );

    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_MathAdd"),
        1,
        "should have exactly one addition gate"
    );

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
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
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
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
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
var teams: int[]
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
        "let TG = 2\ntype RD = { tm: int }\nvar teams: int[]\nmod addR(next: *int, { tm }: RD) -> int {\n  teams.push(tm)\n  let code = next\n  next = next + 1\n  return code\n}\nchip Init() -> (A: int) {\n  var nxt: int = 0\n  emit A = addR(nxt, { tm: TG })\n}\nin go: exec\nlet I = Init(exec = go)",
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
        // Nothing the push reads crosses the boundary as a raw wire. `teams`
        // lives outside the chip, and its ref arrives through a boundary pin
        // whose own port is `RER_Output`: a rerouter has only RER_Input and
        // RER_Output, so a ref port never appears on the pin side of the wire
        // (see `boundary_pins::normalize_pin_ports`).
        for w in &chip.wires {
            if w.target.node_id != push.id {
                continue;
            }
            let src = chip.nodes.get(&w.source.node_id).unwrap_or_else(|| {
                panic!(
                    "the push's {} input is fed by a cross-boundary wire",
                    w.target.port.as_str()
                )
            });
            if matches!(src.kind, crate::ir::NodeKind::Input | crate::ir::NodeKind::Output) {
                assert_eq!(
                    w.source.port,
                    WirePort::RerOutput,
                    "a boundary pin feeds the push from RER_Output, not {}",
                    w.source.port.as_str()
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
var out_arr: int[]
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

/// Mod parameter destructuring.
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
on RoundStart() { set_val(s) }",
    );
    assert_no_errors(&r);
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

/// Nested record field access.
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
on RoundStart() { o.inner.x = 42 }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "nested record field write should produce a Var_Set gate"
    );

    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Var")
        .expect("expected a PseudoVar for `var x`");

    let var_set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
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
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
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

/// Regression: a tuple-destructured mod parameter must bind its names to the
/// argument tuple's elements. A tuple arg arrives as a `Binding::Record` with
/// numeric keys, which the pattern install previously ignored, leaving every
/// name unbound and each use an `_Unsupported` placeholder.
#[test]
fn tuple_param_pattern_binds_from_tuple_arg() {
    let r = compile(
        "\
let pair = (100, 200)
mod add2((a, b): (int, int)) -> int {
  return a + b
}
out result = add2(pair)",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "tuple param names must bind; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    let add = find_gate(&r, "BrickComponentType_Expr_MathAdd");
    let out = find_gate(&r, "BrickComponentType_Internal_MicrochipOutput");
    assert!(wired_reachable(&r, add, out), "sum must reach the output");
    // The elements must reach the gate as its actual operands, not as the
    // zero-valued unwired inputs an `_Unsupported` placeholder left behind.
    let props: Vec<String> = r.module.nodes[&add]
        .properties
        .values()
        .map(|l| format!("{l:?}"))
        .collect();
    assert!(
        props.iter().any(|p| p.contains("100")) && props.iter().any(|p| p.contains("200")),
        "tuple elements must be inlined as the add operands, got {props:?}"
    );
}

/// The same binding path for an inline tuple literal argument.
#[test]
fn tuple_param_pattern_binds_from_tuple_literal_arg() {
    let r = compile(
        "\
mod add2((a, b): (int, int)) -> int {
  return a + b
}
out result = add2((100, 200))",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "tuple literal arg must destructure into the pattern names"
    );
}

/// Regression: `let (a, b) = pair` on a tuple binding must bind both names.
/// The tuple `LetBinding` only handled multi-output nodes, so a tuple built
/// from a literal (a `Binding::Record`) bound nothing.
#[test]
fn let_tuple_destructure_from_tuple_binding() {
    let r = compile(
        "\
let pair = (100, 200)
let (a, b) = pair
out result = a + b",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "let tuple destructure must bind both names"
    );
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "a + b should produce a MathAdd gate"
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
    let get_count = gate_count(&r, "BrickComponentType_Exec_Var_Get");
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
    let set_count = gate_count(&r, "BrickComponentType_Exec_Var_Set");
    assert!(
        set_count >= 1,
        "record field |= inside if inside mod should produce Var_Set, got {}",
        set_count
    );
    let set_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
        .expect("must have a Var_Set node");
    let has_exec_in = r.module.wires.iter().any(|w| {
        w.target.node_id == set_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec
    });
    assert!(
        has_exec_in,
        "Var_Set must have exec input wired (not orphaned)"
    );

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
    let r_direct = compile(
        "\
var flag: int = 0
mod cond_set(f_val: *int, cond: bool) {
  if cond { f_val |= 1 }
}
in tick: exec
on tick { cond_set(flag, true) }",
    );
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

    let direct_sets = gate_count(&r_direct, "BrickComponentType_Exec_Var_Set");
    let record_sets = gate_count(&r_record, "BrickComponentType_Exec_Var_Set");
    eprintln!(
        "Direct Var_Sets: {}, Record Var_Sets: {}",
        direct_sets, record_sets
    );

    assert_eq!(
        record_sets, direct_sets,
        "record version should have same number of Var_Sets as direct"
    );

    let branch_count = gate_count(&r_direct, "BrickComponentType_Exec_Branch");
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
var io: int[]
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
        .any(|n| n.gate_class == "BrickComponentType_Exec_ArrayVar_SetAtIndex");
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
    let get_count = gate_count(&r, "BrickComponentType_Exec_Var_Get");
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
    let get_count = gate_count(&r, "BrickComponentType_Exec_Var_Get");
    assert!(
        get_count >= 3,
        "snapshot + write + re-read should produce at least 3 Var_Gets (got {})",
        get_count
    );
}

/// Destructuring a builtin multi-output gate must bind each name to that
/// gate's own output port. No chip owns those outputs, so the chip lookup
/// finds nothing; the destructure used to bind nothing at all and every use
/// became an `_Unsupported` placeholder wired to no source.
#[test]
fn builtin_multi_output_destructure_binds_gate_ports() {
    let r = compile(
        "on CharacterSpawned() -> (player) {\n\
           let { Forward, Right, Up } = player.InputReader()\n\
           PrintToConsole(\"${Forward} ${Right} ${Up}\")\n\
         }",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "destructured fields must bind to the gate's outputs; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // Each field reads its own port on the splitter, not a shared default.
    let splitter = find_gate(&r, "Component_Internal_InputSplitter");
    let ports: Vec<String> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == splitter)
        .map(|w| w.source.port.as_str().to_string())
        .collect();
    for p in ["InputForward", "InputRight", "InputUp"] {
        assert!(ports.contains(&p.to_string()), "expected a wire from {p}, got {ports:?}");
    }
}

/// Field names are case-sensitive. A name that matches nothing bound silently,
/// leaving dangling placeholders — it now errors and points at the real field.
#[test]
fn destructuring_an_unknown_field_errors_with_a_suggestion() {
    let r = compile(
        "on CharacterSpawned() -> (player) {\n\
           let { forward } = player.InputReader()\n\
           PrintToConsole(\"${forward}\")\n\
         }",
    );
    let msgs: Vec<&str> = r.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("no field `forward`") && m.contains("`Forward`")),
        "expected a suggestion naming the real field, got {msgs:?}"
    );
}

/// A mod that returns another mod's record-typed call result must forward the
/// record fields to its caller. The bug: `return make(x)` fell into the scalar
/// return path, whose value port for a record-returning call is the
/// `NodeId(0)` placeholder — the caller ended up wired to a phantom node that
/// emit later dropped (`UnknownWireNode("n0")`).
#[test]
fn mod_returning_record_call_forwards_fields() {
    let src = "type Pair = { a: float, b: float }\n\
               mod make(x: float) -> Pair {\n\
               let a = x\n\
               let b = x\n\
               return { a, b }\n\
               }\n\
               mod wrap(x: float) -> Pair {\n\
               return make(x)\n\
               }\n\
               in v: float\n\
               let r = wrap(v)\n\
               out ra = r.a\n\
               out rb = r.b";
    let r = compile(src);
    assert_no_errors(&r);
    for w in &r.module.wires {
        assert!(
            r.module.nodes.contains_key(&w.source.node_id),
            "wire source references a nonexistent node: {:?} -> {:?}",
            w.source,
            w.target
        );
        assert!(
            r.module.nodes.contains_key(&w.target.node_id),
            "wire target references a nonexistent node: {:?} -> {:?}",
            w.source,
            w.target
        );
    }
    // Both outputs must trace back to the module input, same as calling
    // `make` directly would.
    let input = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    for out_id in &r.module.outputs {
        let driven_by_input = r
            .module
            .wires
            .iter()
            .any(|w| w.target.node_id == *out_id && w.source.node_id == input);
        assert!(
            driven_by_input,
            "output {out_id} must be driven by the input port"
        );
    }
}

/// Same pass-through, but via a local: `let r = make(x); return r`. The
/// record binding must survive the return, not degrade to a phantom port.
#[test]
fn mod_returning_record_local_forwards_fields() {
    let src = "type Pair = { a: float, b: float }\n\
               mod make(x: float) -> Pair {\n\
               let a = x\n\
               let b = x\n\
               return { a, b }\n\
               }\n\
               mod wrap(x: float) -> Pair {\n\
               let r = make(x)\n\
               return r\n\
               }\n\
               in v: float\n\
               let r = wrap(v)\n\
               out ra = r.a\n\
               out rb = r.b";
    let r = compile(src);
    assert_no_errors(&r);
    for w in &r.module.wires {
        assert!(
            r.module.nodes.contains_key(&w.source.node_id),
            "wire source references a nonexistent node: {:?} -> {:?}",
            w.source,
            w.target
        );
    }
    let input = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    for out_id in &r.module.outputs {
        let driven_by_input = r
            .module
            .wires
            .iter()
            .any(|w| w.target.node_id == *out_id && w.source.node_id == input);
        assert!(
            driven_by_input,
            "output {out_id} must be driven by the input port"
        );
    }
}

// ---- record-typed STORAGE (the aggregate feature) ----

const PT: &str = "type Point = { x: int, y: int }\n";

/// A record VARIABLE decomposes into one `Pseudo_Var` per field; `p.x` reads /
/// `p.x = v` writes the right backing gate — no single-gate collapse and no
/// bogus `SplitVector.X` swizzle (the prior silent miscompile).
#[test]
fn record_var_decomposes_into_per_field_vars() {
    let r = compile(&format!(
        "{PT}var p: Point = {{ x: 1, y: 2 }}\nout ox = p.x\nin s: exec\non s {{ p.x = 5 }}"
    ));
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, gc::VAR), 2, "one Pseudo_Var per field");
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0, "no swizzle on a record");
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::VAR_SET), 1, "p.x = 5 is one Var_Set");
}

/// A nested record variable decomposes recursively (a `Pseudo_Var` per leaf).
#[test]
fn nested_record_var_decomposes_per_leaf() {
    let r = compile(
        "type In = { a: int, b: int }\ntype Out = { p: In, q: int }\n\
         var o: Out = { p: { a: 1, b: 2 }, q: 3 }\nout oa = o.p.a\n\
         in s: exec\non s { o.p.a = 9 }",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, gc::VAR), 3, "a, b, q each a Pseudo_Var");
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0);
    assert!(!has_gate(&r, "_Unsupported"));
}

/// Whole-record assignment `p = { .. }` writes each field (one Var_Set per
/// field); it used to silently emit nothing.
#[test]
fn whole_record_literal_assignment_writes_each_field() {
    let r = compile(&format!(
        "{PT}var p: Point = {{ x: 1, y: 2 }}\nout ox = p.x\nin s: exec\non s {{ p = {{ x: 5, y: 6 }} }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::VAR_SET), 2, "one Var_Set per field");
}

/// Record-to-record assignment `p = q` reads each field of `q` and writes it to
/// `p` (one Var_Get + one Var_Set per field).
#[test]
fn record_to_record_assignment_copies_each_field() {
    let r = compile(&format!(
        "{PT}var p: Point = {{ x: 1, y: 2 }}\nvar q: Point = {{ x: 3, y: 4 }}\n\
         out ox = p.x\nin s: exec\non s {{ p = q }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::VAR_GET), 2, "read each field of q");
    assert_eq!(gate_count(&r, gc::VAR_SET), 2, "write each field of p");
}

/// A record ARRAY is stored as one parallel `Pseudo_ArrayVar` per field, and
/// `push` fans the record value across them.
#[test]
fn record_array_push_fans_out_per_field() {
    let r = compile(&format!(
        "{PT}var pts: Point[]\nin s: exec\non s {{ pts.push({{ x: 1, y: 2 }}) }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::ARRAY_VAR), 2, "one array per field");
    assert_eq!(gate_count(&r, gc::ARRAY_PUSH), 2, "push fans out per field");
}

/// `pts.length()` reads only the FIRST field's array (they share one count);
/// the deterministic lockstep mutations fan out across every field.
#[test]
fn record_array_length_first_field_and_lockstep_mutations() {
    let r = compile(&format!(
        "{PT}var pts: Point[]\nvar n: int = 0\nin s: exec\n\
         on s {{ pts.insert(0, {{ x: 5, y: 6 }})\n pts.fill({{ x: 0, y: 0 }})\n \
         pts.remove(0)\n pts.clear()\n n = pts.length() }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::ARRAY_GET_LENGTH), 1, "length reads first field only");
    assert_eq!(gate_count(&r, gc::ARRAY_INSERT), 2);
    assert_eq!(gate_count(&r, gc::ARRAY_FILL), 2);
    assert_eq!(gate_count(&r, gc::ARRAY_REMOVE_AT_INDEX), 2);
    assert_eq!(gate_count(&r, gc::ARRAY_CLEAR), 2);
}

/// Record-array element indexing: `pts[i].x` reads that field's parallel array
/// (an `ArrayVar_Get`, NOT a `SplitVector`); `pts[i].x = v` and `pts[i] = rec`
/// write via `SetAtIndex`; `p = pts[i]` reads every field.
#[test]
fn record_array_index_read_write() {
    let r = compile(&format!(
        "{PT}var pts: Point[]\nvar gx: int = 0\nvar p: Point = {{ x: 0, y: 0 }}\n\
         in s: exec\non s {{ pts.push({{ x: 1, y: 2 }})\n gx = pts[0].x\n \
         pts[0].y = 7\n pts[0] = {{ x: 9, y: 8 }}\n p = pts[0] }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0, "no swizzle on a record element");
    // gx = pts[0].x (1) + p = pts[0] (2 fields) = 3 ArrayVar_Get
    assert_eq!(gate_count(&r, gc::ARRAY_GET), 3);
    // pts[0].y = 7 (1) + pts[0] = {..} (2 fields) = 3 SetAtIndex
    assert_eq!(gate_count(&r, gc::ARRAY_SET_AT_INDEX), 3);
}

/// A NESTED record inside a record array decomposes to leaf-field parallel
/// arrays just like flat fields, so every depth of access lowers to real gates:
/// `arr[i].inner.a` (nested scalar read/write), `arr[i].inner` (whole nested
/// record read/write), and `let row = arr[i]` (whole element read). The nested
/// path also wins over the swizzle fallback, so genuine `.a`/`.b` fields read
/// their column instead of a bogus `SplitColor` component.
#[test]
fn nested_record_array_field_path() {
    let r = compile(
        "type Inner = { a: int, b: int }\n\
         type Outer = { n: int, inner: Inner }\n\
         var arr: Outer[]\n\
         var gna: int = 0\n\
         var ib: Inner = { a: 0, b: 0 }\n\
         var row: Outer = { n: 0, inner: { a: 0, b: 0 } }\n\
         in s: exec\n\
         on s {\n\
           arr.push({ n: 1, inner: { a: 2, b: 3 } })\n\
           gna = arr[0].inner.a\n\
           arr[0].inner.a = 9\n\
           ib = arr[0].inner\n\
           arr[0].inner = { a: 7, b: 8 }\n\
           row = arr[0]\n\
           arr[0] = { n: 4, inner: { a: 5, b: 6 } }\n\
         }",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "nested record-array access must not placeholder");
    assert_eq!(gate_count(&r, gc::SPLIT_COLOR), 0, "a genuine `.a`/`.b` field must not swizzle");
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0);
    // n, inner.a, inner.b -> 3 parallel arrays.
    assert_eq!(gate_count(&r, gc::ARRAY_VAR), 3, "one array per leaf field");
    assert_eq!(gate_count(&r, gc::ARRAY_PUSH), 3, "push fans to every leaf");
    // arr[0].inner.a (1) + arr[0].inner (2) + arr[0] (3) = 6 reads.
    assert_eq!(gate_count(&r, gc::ARRAY_GET), 6);
    // inner.a=9 (1) + inner={..} (2) + arr[0]={..} (3) = 6 writes.
    assert_eq!(gate_count(&r, gc::ARRAY_SET_AT_INDEX), 6);
}

/// R13: indexing a record-array COLUMN (`pts.x[i]`) reads the field's column as
/// a real `ArrayVar_Get` and types as the field, not `Any`. Because the type is
/// right it also works inside an expression (a comparison), which used to fall
/// to `_Unsupported` after a WS004 (Any) killed operator resolution.
#[test]
fn record_array_column_index() {
    let r = compile(
        "type P = { x: int, y: int }\n\
         var pts: P[]\n\
         var a: int = 0\n\
         var c: bool = false\n\
         in s: exec\n\
         on s {\n\
           pts.push({ x: 10, y: 20 })\n\
           a = pts.x[0]\n\
           c = pts.x[0] != 5\n\
         }",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "indexing a column must not placeholder");
    // Both `a = pts.x[0]` and the comparison operand read the `x` column.
    assert_eq!(gate_count(&r, gc::ARRAY_GET), 2, "each pts.x[i] reads its column");
    assert!(
        r.module.nodes.values().any(|n| n.gate_class.contains("CompareNotEqual")),
        "the != on a column value must resolve to a real compare, not a placeholder"
    );
}

/// A record MAP is stored as one parallel `Pseudo_MapVar` per field; `set`/`get`
/// fan across the fields, `has`/`length` read the first field, and `remove`/
/// `clear` fan out. `m.get(k).x`, `m[k].x`, `m[k] = rec`, and `p = m.get(k)` all
/// lower to real gates (no swizzle, no placeholder).
#[test]
fn record_map_fans_out_per_field() {
    let r = compile(&format!(
        "{PT}var m: Map<int, Point>\nvar gx: int = 0\nvar p: Point = {{ x: 0, y: 0 }}\n\
         var n: int = 0\nvar f: bool = false\nin s: exec\non s {{ \
         m.set(0, {{ x: 1, y: 2 }})\n m[1] = {{ x: 3, y: 4 }}\n gx = m.get(0).x\n \
         gx = m[1].x\n p = m.get(0)\n f = m.has(0)\n n = m.length()\n \
         m.remove(0)\n m.clear() }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0);
    assert_eq!(gate_count(&r, gc::MAP_VAR), 2, "one map per field");
    // set(1) + m[1]=(1) = 2 sets, each fanned to 2 fields = 4
    assert_eq!(gate_count(&r, gc::MAP_SET), 4);
    // get(0).x (1) + m[1].x (1) + p=m.get(0) (2) = 4
    assert_eq!(gate_count(&r, gc::MAP_GET), 4);
    assert_eq!(gate_count(&r, gc::MAP_HAS), 1, "has reads first field");
    assert_eq!(gate_count(&r, gc::MAP_GET_LENGTH), 1, "length reads first field");
    assert_eq!(gate_count(&r, gc::MAP_REMOVE), 2);
    assert_eq!(gate_count(&r, gc::MAP_CLEAR), 2);
}

/// A NESTED record inside a record MAP decomposes to leaf-field parallel maps
/// the same way arrays do, so every depth of value access lowers to real gates
/// (`m[k].inner.a` read/write, `m[k].inner` whole read/write, `row = m[k]`).
/// The nested path wins over the swizzle fallback, so a genuine `.a`/`.b` field
/// reads its column rather than a bogus `SplitColor` off a placeholder.
#[test]
fn nested_record_map_field_path() {
    let r = compile(
        "type Inner = { a: int, b: int }\n\
         type Outer = { n: int, inner: Inner }\n\
         var m: Map<int, Outer>\n\
         var gna: int = 0\n\
         var ib: Inner = { a: 0, b: 0 }\n\
         var row: Outer = { n: 0, inner: { a: 0, b: 0 } }\n\
         in s: exec\n\
         on s {\n\
           m.set(0, { n: 1, inner: { a: 2, b: 3 } })\n\
           gna = m[0].inner.a\n\
           m[0].inner.a = 9\n\
           ib = m[0].inner\n\
           m[0].inner = { a: 7, b: 8 }\n\
           row = m[0]\n\
         }",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "nested record-map access must not placeholder");
    assert_eq!(gate_count(&r, gc::SPLIT_COLOR), 0, "a genuine `.a`/`.b` field must not swizzle");
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0);
    // n, inner.a, inner.b -> 3 parallel maps.
    assert_eq!(gate_count(&r, gc::MAP_VAR), 3, "one map per leaf field");
    // set (3, fans to every leaf) + inner.a=9 (1) + inner={..} (2) = 6.
    assert_eq!(gate_count(&r, gc::MAP_SET), 6);
    // m[0].inner.a (1) + m[0].inner (2) + m[0] (3) = 6 reads.
    assert_eq!(gate_count(&r, gc::MAP_GET), 6);
}

/// The safe record-array ops fan out; `sort`/`shuffle`/aggregates are rejected
/// with WS050 (they'd desync or fold over whole records) rather than silently
/// no-op'ing to a placeholder.
#[test]
fn record_array_pop_swap_resize_and_ws050() {
    let r = compile(&format!(
        "{PT}var pts: Point[]\nvar p: Point = {{ x: 0, y: 0 }}\nin s: exec\n\
         on s {{ pts.push({{ x: 1, y: 2 }})\n pts.push({{ x: 3, y: 4 }})\n \
         pts.swap(0, 1)\n pts.resize(3, {{ x: 9, y: 9 }})\n p = pts.pop() }}"
    ));
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::ARRAY_SWAP), 2, "swap fans out per field");
    assert_eq!(gate_count(&r, gc::ARRAY_RESIZE), 2, "resize fans out per field");
    assert_eq!(gate_count(&r, gc::ARRAY_POP), 2, "pop fans out per field");

    // sort has no per-field meaning on a record array -> WS050.
    let bad = compile(&format!("{PT}var pts: Point[]\nin s: exec\non s {{ pts.sort() }}"));
    assert!(
        bad.diagnostics.iter().any(|d| d.code == "WS050"),
        "pts.sort() must be WS050: {:?}",
        bad.diagnostics
    );
}

/// A record ARRAY / MAP constructor literal bakes per-field columns into each
/// backing container's InitialValue (`var pts: Point[] = [{x:1,y:2},{x:3,y:4}]`
/// bakes x -> [1,3], y -> [2,4]); it used to compile clean but silently start
/// empty.
#[test]
fn record_container_constructor_bakes_per_field() {
    let iv = crate::intern::intern("InitialValue");

    let ra = compile(&format!(
        "{PT}var pts: Point[] = [{{ x: 1, y: 2 }}, {{ x: 3, y: 4 }}]"
    ));
    assert_no_errors(&ra);
    let cols: Vec<Vec<i64>> = ra
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_VAR)
        .filter_map(|n| match n.properties.get(&iv) {
            Some(crate::ir::Literal::Array(items)) => Some(
                items
                    .iter()
                    .filter_map(|l| match l {
                        crate::ir::Literal::Int(i) => Some(*i),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    assert!(cols.contains(&vec![1, 3]), "x column [1,3] must bake; got {cols:?}");
    assert!(cols.contains(&vec![2, 4]), "y column [2,4] must bake; got {cols:?}");

    let rm = compile(&format!(
        "{PT}var m: Map<int, Point> = {{ 0 => {{ x: 5, y: 6 }}, 1 => {{ x: 7, y: 8 }} }}"
    ));
    assert_no_errors(&rm);
    let maps: usize = rm
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MAP_VAR && n.properties.contains_key(&iv))
        .count();
    assert_eq!(maps, 2, "both field maps must bake an InitialValue");
}

/// Record storage imported from another module (plain `import "lib"`) decomposes
/// into ONE set of per-field gates and is fully usable across the boundary.
#[test]
fn imported_record_storage_lowers() {
    let lib = "type Point = { x: int, y: int }\n\
               var p: Point = { x: 1, y: 2 }\nvar pts: Point[]\nvar m: Map<int, Point>";
    let r = compile_multi(
        "import \"lib\"\nvar a: int = 0\nin go: exec\n\
         on go { p.x = 5\n pts.push({ x: 3, y: 4 })\n m.set(0, { x: 7, y: 8 })\n \
         a = pts[0].y\n a = m.get(0).x }",
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::VAR), 3, "p.x, p.y + local a");
    assert_eq!(gate_count(&r, gc::ARRAY_VAR), 2, "pts.x, pts.y");
    assert_eq!(gate_count(&r, gc::MAP_VAR), 2, "m.x, m.y");
}

/// Record storage imported via a namespace (`import * as L`) lowers its field/
/// index/method access (`L.p.x`, `L.pts.push`, `L.pts[i].y`, `L.m.get(k).x`).
#[test]
fn namespaced_record_storage_lowers() {
    let lib = "type Point = { x: int, y: int }\n\
               var p: Point = { x: 1, y: 2 }\nvar pts: Point[]\nvar m: Map<int, Point>";
    let r = compile_multi(
        "import * as L from \"lib\"\nvar a: int = 0\nin go: exec\n\
         on go { L.pts.push({ x: 3, y: 4 })\n L.p.x = 5\n L.m.set(0, { x: 7, y: 8 })\n \
         a = L.p.x\n a = L.pts[0].y\n a = L.m.get(0).x }",
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 0, "no swizzle across the import");
    assert_eq!(gate_count(&r, gc::ARRAY_VAR), 2);
    assert_eq!(gate_count(&r, gc::MAP_VAR), 2);
}

/// SoA field access resolves array methods on `pts.field` (the parallel array) -
/// including `min`/`max`, which used to fall through to the no-receiver builtin
/// check (WS036). And `pts.field.sort()` sorts the WHOLE record by that field via
/// sortMultiple (reordering sibling columns), not the one column in isolation.
#[test]
fn soa_field_methods_and_sort_by_field() {
    let r = compile(&format!(
        "{PT}var a: int = 0\nvar b: int = 0\nin s: exec\n\
         on s {{ var pts: Point[]\n pts.push({{x:5,y:1}})\n pts.push({{x:2,y:9}})\n \
         a = pts.x.min()\n b = pts.x.max() }}"
    ));
    assert_no_errors(&r); // no WS036
    assert_eq!(gate_count(&r, gc::ARRAY_MIN), 1);
    assert_eq!(gate_count(&r, gc::ARRAY_MAX), 1);

    let s = compile(&format!(
        "{PT}in s: exec\non s {{ var pts: Point[]\n pts.push({{x:5,y:1}})\n \
         pts.push({{x:2,y:9}})\n pts.x.sort() }}"
    ));
    assert_no_errors(&s);
    assert_eq!(
        gate_count(&s, gc::ARRAY_SORT_MULTIPLE), 1,
        "sort by a field must use sortMultiple over the sibling columns"
    );
    assert_eq!(gate_count(&s, gc::ARRAY_SORT), 0, "not a single-column sort");

    // A record wider than the gate's 8 columns sorts in groups of 7, each later
    // group against a copy of the original key (so no 8-field limit).
    let wide = compile(
        "type W = { a:int,b:int,c:int,d:int,e:int,f:int,g:int,h:int,i:int,j:int,k:int,l:int }\n\
         in s: exec\non s { var w: W[]\n w.push({a:1,b:1,c:1,d:1,e:1,f:1,g:1,h:1,i:1,j:1,k:1,l:1})\n w.a.sort() }",
    );
    assert_no_errors(&wide);
    assert!(!has_gate(&wide, "_Unsupported"));
    // 11 sibling columns -> groups of 7 + 4 -> 2 sortMultiple, 1 key copy.
    assert_eq!(gate_count(&wide, gc::ARRAY_SORT_MULTIPLE), 2);
    assert_eq!(gate_count(&wide, gc::ARRAY_COPY_FROM), 1);
}

/// An inline record-returning mod call read immediately by field
/// (`adder(n, 1).sum`, no intermediate `let`) projects the field out of the
/// call's stashed multi-output record instead of degrading to `_Unsupported`.
#[test]
fn inline_record_call_field_access_projects_field() {
    let r = compile(
        "mod adder(x: int, y: int) -> { sum: int, diff: int } {\n\
           return { sum: x + y, diff: x - y }\n\
         }\nin n: int\nout total = adder(n, 1).sum",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "inline f().sum must project the field, not degrade to a placeholder"
    );
    let input = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let out = *r.module.outputs.first().unwrap();
    assert!(
        wired_reachable(&r, input, out),
        "the input `n` must reach `total` through the sum adder"
    );
}

/// A record-typed top-level output (`out p: Rec = { .. }`) explodes into
/// per-field boundary pins, each wired from its field's source. It used to
/// dangle: one pin driven by a placeholder / lossy single wire.
#[test]
fn record_out_binding_wires_per_field_pins() {
    let lit = compile("type Rec = { foo: int, bar: int }\nout p: Rec = { foo: 1, bar: 2 }");
    assert_no_errors(&lit);
    assert!(
        !has_gate(&lit, "_Unsupported"),
        "a record out literal must materialise, not placeholder"
    );
    assert!(
        !lit.module.outputs.is_empty(),
        "the record output must produce boundary pins"
    );
    for out_id in &lit.module.outputs {
        assert!(
            lit.module.wires.iter().any(|w| w.target.node_id == *out_id),
            "every record-output field pin must have an incoming driver"
        );
    }

    // A record-returning call and a record var both wire per-field too.
    let call = compile(
        "type Rec = { foo: int, bar: int }\nin n: int\n\
         mod mk(v: int) -> Rec { return { foo: v, bar: v } }\nout p: Rec = mk(n)",
    );
    assert_no_errors(&call);
    assert!(!has_gate(&call, "_Unsupported"));
    for out_id in &call.module.outputs {
        assert!(
            call.module.wires.iter().any(|w| w.target.node_id == *out_id),
            "record-returning call must drive every output field pin"
        );
    }
}

/// Whole-array assignment from a record-array literal (`arr = [rec, rec]`) fans
/// clear + push across the parallel per-field arrays. A record array is also a
/// `Binding::Record`, so the whole-record-assign path used to claim it and drop
/// the array literal silently.
#[test]
fn record_array_whole_assign_from_literal_fans_clear_push() {
    let r = compile(
        "type DoubleInt = { a: int, b: int }\n\
         let pair = ({ a: 10, b: 20 }, { a: -10, b: -20 })\n\
         var arr: DoubleInt[]\n\
         on ReadBrickGrid() { arr = [pair.0, pair.1] }",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // 2 field arrays cleared; each of 2 elements pushed to each field (2 clears, 4 pushes).
    assert_eq!(gate_count(&r, "BrickComponentType_Exec_ArrayVar_Clear"), 2);
    assert_eq!(gate_count(&r, "BrickComponentType_Exec_ArrayVar_Push"), 4);
}

/// A field access on an aggregate-typed if-expression distributes over the
/// branches (`(if c then a else b).u` -> `if c then a.u else b.u`), lowering to
/// a Select over the two field values instead of an `_Unsupported`/Split*.
#[test]
fn if_expr_field_access_distributes_over_branches() {
    let r = compile(
        "type P = { u: int, v: int }\nin c: bool\n\
         let a: P = { u: 1, v: 2 }\nlet b: P = { u: 3, v: 4 }\nout r = (if c then a else b).u",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "record if-expr field must not degrade");
    assert!(has_gate(&r, "BrickComponentType_Expr_Select"));
}

/// An index on an aggregate-typed if-expression distributes over the branches
/// (`(if c then a else b)[i]` -> `if c then a[i] else b[i]`) for scalar arrays.
#[test]
fn if_expr_index_access_distributes_over_branches() {
    let r = compile(
        "in c: bool\nin go: exec\nvar a: int[]\nvar b: int[]\nvar o: int\n\
         on go {\n  a.push(1)\n  b.push(4)\n  o = (if c then a else b)[0]\n}",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "array if-expr index must not degrade");
    assert!(has_gate(&r, "BrickComponentType_Expr_Select"));
}

/// A NESTED record passed as a chip param explodes recursively into per-leaf
/// pins on both the body and caller sides, so `o.inr.a` reads its own pin
/// instead of re-splitting the first param pin via SplitColor, and the outer
/// arg wire is not dropped.
#[test]
fn nested_record_chip_param_explodes_recursively() {
    let r = compile(
        "type Inner = { a: int, b: int }\ntype Outer = { inr: Inner, c: int }\n\
         chip Sink(o: Outer) -> (r: int) { out r = o.inr.a + o.c }\n\
         in n: int\nout total = Sink({ inr: { a: n, b: 2 }, c: 3 })",
    );
    assert_no_errors(&r);
    fn any_gate(m: &crate::ir::Module, class: &str) -> bool {
        m.nodes.values().any(|n| n.gate_class == class) || m.chips.values().any(|c| any_gate(c, class))
    }
    assert!(!any_gate(&r.module, "_Unsupported"), "nested record chip param must bind");
    assert!(
        !any_gate(&r.module, "BrickComponentType_Expr_SplitColor")
            && !any_gate(&r.module, "BrickComponentType_Expr_SplitVector"),
        "o.inr.a must read its own leaf pin, not SplitColor the first input"
    );
    // The caller's `n` arg must wire into the chip (before the fix the nested
    // arg wire was dropped, because binding_to_port on a Record returned None).
    let n_input = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == n_input),
        "the nested record arg `n` must wire into the chip, not be dropped"
    );
    // Inside the chip body, `o.inr.a + o.c` feeds a MathAdd from two leaf pins.
    let add = r
        .module
        .chips
        .values()
        .flat_map(|c| c.nodes.values())
        .find(|n| n.gate_class == "BrickComponentType_Expr_MathAdd")
        .expect("chip body must contain the o.inr.a + o.c add");
    assert_eq!(add.ports.inputs.len(), 2, "both record leaves must feed the add");
}

/// A TUPLE parameter on a chip signature explodes into per-element pins on both
/// sides (like a record), so the `(a, b)` destructure binds the names instead of
/// leaving the body reading `_Unsupported`. The mod form already worked.
#[test]
fn chip_tuple_param_pattern_binds() {
    let r = compile(
        "chip adder((a, b): (int, int)) -> (s: int) { out s = a + b }\n\
         in x: int\nout r = adder((x, 1))",
    );
    assert_no_errors(&r);
    fn any_gate(m: &crate::ir::Module, class: &str) -> bool {
        m.nodes.values().any(|n| n.gate_class == class) || m.chips.values().any(|c| any_gate(c, class))
    }
    assert!(!any_gate(&r.module, "_Unsupported"), "chip tuple param names must bind");
    let add = r
        .module
        .chips
        .values()
        .flat_map(|c| c.nodes.values())
        .find(|n| n.gate_class == "BrickComponentType_Expr_MathAdd")
        .expect("chip body must contain a + b");
    assert_eq!(add.ports.inputs.len(), 2, "both tuple elements must feed the add");
}

#[test]
fn dot_value_opens_a_record_variable() {
    // `.Value` opens a VARIABLE, whatever it holds. A var needing more than one
    // storage gate (a record here, an enum elsewhere) binds as a
    // `Binding::Record`, so it never reaches the single-port `Binding::Var` arm
    // that answers `.Value` for a scalar.
    //
    // Asserts WHICH storage gate the read lands on, not just how many gates
    // exist: a record var has one `Pseudo_Var` per field (labelled `p.x`,
    // `p.y`), so resolving `.Value` to the wrong field still emits exactly one
    // `Var_Get` and would sail past a node count while reading `y` for `x`.
    let read_field_label = |src: &str| -> String {
        let r = compile(src);
        assert_no_errors(&r);
        assert!(!has_gate(&r, "_Unsupported"), "{:?}", r.diagnostics);
        let get = r
            .module
            .nodes
            .values()
            .find(|n| n.gate_class == crate::ir::gate_class::VAR_GET)
            .expect("a Var_Get reading the record field");
        // The `VarRef` input names the storage gate this read is bound to.
        let src_node = r
            .module
            .wires
            .iter()
            .find(|w| w.target.node_id == get.id && w.target.port == WirePort::VarRef)
            .map(|w| w.source.node_id)
            .expect("the Var_Get must have its VarRef wired");
        match r.module.nodes[&src_node]
            .properties
            .get(&crate::intern::sym::NAME_LABEL)
        {
            Some(crate::ir::Literal::String(s)) => s.clone(),
            other => panic!("storage gate carries no name label: {other:?}"),
        }
    };
    let decl = "type P = { x: int, y: int }
                var p: P = { x: 1, y: 2 }
                var got: int
";
    let bare = read_field_label(&format!("{decl}on RoundStart() {{ got = p.x }}
"));
    let dotted = read_field_label(&format!("{decl}on RoundStart() {{ got = p.Value.x }}
"));
    assert_eq!(bare, "p.x", "`p.x` must read the x field's storage");
    assert_eq!(dotted, bare, "`p.Value.x` must read the same storage as `p.x`");
}

#[test]
fn dot_value_on_a_record_value_is_not_silently_an_identity() {
    // The opening is gated on the record being STORAGE. A record VALUE (a `let`,
    // a chip's multi-output result) has no variable to open, and typecheck
    // already calls `.Value` an unknown field on it (WS010) -- lowering must not
    // quietly answer it as an identity and contradict that.
    let r = compile(
        "var got: int
         on RoundStart() {
           let rec = { x: 9, y: 8 }
           got = rec.Value.x
         }
",
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS010" || d.code == "WSP001"),
        "`.Value` on a record value must stay loud, got {:?}",
        r.diagnostics
    );
}

// `&recordVar` passed to a `*P` ref param

#[test]
fn ref_param_given_an_address_of_a_record_var_runs_the_body() {
    // `bump(&p, u)` must bind `q` to the caller's record exactly as the bare
    // `bump(p, u)` form does. The record fast path resolved the UN-unwrapped
    // `Expr::RefOf`, which never names a binding, so control fell to the
    // container arm, which resolves through `lookup_var` and holds nothing for
    // a record, so the param bound to nothing. Every statement in the body then
    // referenced a free name and the whole body vanished without a word.
    let r = compile(
        "type P = { a: int, b: int }
         in go: exec
         in u: int
         var p: P = { a: 0, b: 0 }
         mod bump(q: *P, d: int) { q.a = q.a + d }
         on go { bump(&p, u) }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["p.a".to_string()],
        "the body must write the caller's `p.a` through the ref param"
    );
}

#[test]
fn ref_param_address_of_form_matches_the_bare_form() {
    // The two spellings are the same binding, so they must lower to the same
    // gates. This structural equality catches a fix that wires `&p` somewhere
    // plausible but different.
    let src = "type P = { a: int, b: int }
         in go: exec
         in u: int
         var p: P = { a: 0, b: 0 }
         mod bump(q: *P, d: int) { q.a = q.a + d }
         on go { bump(ARG, u) }
";
    let amp = compile(&src.replace("ARG", "&p"));
    let bare = compile(&src.replace("ARG", "p"));
    assert_no_errors(&amp);
    assert_no_errors(&bare);
    assert_eq!(amp.module.nodes.len(), bare.module.nodes.len());
    assert_eq!(written_var_labels(&amp), written_var_labels(&bare));
}

// a record-returning call as a NESTED record-literal field

#[test]
fn record_returning_call_in_a_nested_literal_field_writes_every_leaf() {
    // `o = { i: mkInner(u), k: v }` has to write all three leaves. The nested
    // field lowered through `lower_expr`, which keeps one port and discards the
    // call's `pending_inline_record`, so the `i` sub-record arrived as a
    // `Binding::Local` and `assign_record_fields` skipped it in silence.
    let r = compile(
        "type Inner = { m: int, n: int }
         type Outer = { i: Inner, k: int }
         in go: exec
         in u: int
         in v: int
         var o: Outer = { i: { m: 0, n: 0 }, k: 0 }
         mod mkInner(x: int) -> (res: Inner) { return { m: x, n: x } }
         on go { o = { i: mkInner(u), k: v } }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["o.i.m".to_string(), "o.i.n".to_string(), "o.k".to_string()],
    );
}

#[test]
fn record_returning_call_in_a_nested_literal_field_pushes_every_column() {
    // The array form is worse than a wrong value: pushing only the `k` column
    // leaves the parallel arrays at different LENGTHS, so every later row is
    // misaligned. One push per leaf column is the invariant.
    let r = compile(
        "type Inner = { m: int, n: int }
         type Outer = { i: Inner, k: int }
         in go: exec
         in u: int
         in v: int
         var arr: Outer[]
         mod mkInner(x: int) -> (res: Inner) { return { m: x, n: x } }
         on go { arr.push({ i: mkInner(u), k: v }) }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_Exec_ArrayVar_Push"),
        3,
        "one push per parallel column, or the columns desync"
    );
}

// a record-valued `if/then/else`

#[test]
fn record_valued_ternary_selects_each_leaf() {
    // `z = if c then x else y` on records used to emit NOTHING: no Select, no
    // Var_Set, and no diagnostic. A record has no single-wire form, so the
    // choice has to be made per leaf, one Select per field, all sharing the one
    // lowered condition.
    let r = compile(
        "type P = { a: int, b: int }
         in go: exec
         in c: bool
         var x: P = { a: 1, b: 2 }
         var y: P = { a: 3, b: 4 }
         var z: P = { a: 0, b: 0 }
         on go { z = if c then x else y }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["z.a".to_string(), "z.b".to_string()],
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_Expr_Select"),
        2,
        "one Select per leaf field"
    );
}

#[test]
fn record_valued_ternary_shares_one_condition_gate() {
    // The per-leaf fan-out must reuse the condition, not re-lower it per field:
    // a `Var_Get` per leaf would multiply with the record's width.
    let r = compile(
        "type P = { a: int, b: int, c: int }
         in go: exec
         var cond: bool = false
         var x: P = { a: 1, b: 2, c: 3 }
         var y: P = { a: 4, b: 5, c: 6 }
         var z: P = { a: 0, b: 0, c: 0 }
         on go { z = if cond then x else y }
",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, "BrickComponentType_Expr_Select"), 3);
    let selects: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_Expr_Select")
        .map(|(id, _)| *id)
        .collect();
    let cond_sources: crate::collections::HashSet<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.port == WirePort::BSelectB && selects.contains(&w.target.node_id))
        .map(|w| w.source.node_id)
        .collect();
    assert_eq!(cond_sources.len(), 1, "every Select reads the same condition");
}

#[test]
fn nested_record_valued_ternary_selects_every_leaf() {
    // The per-leaf walk has to recurse: a nested record field is two leaves,
    // not one.
    let r = compile(
        "type Inner = { m: int, n: int }
         type Outer = { i: Inner, k: int }
         in go: exec
         in c: bool
         var x: Outer = { i: { m: 1, n: 2 }, k: 3 }
         var y: Outer = { i: { m: 4, n: 5 }, k: 6 }
         var z: Outer = { i: { m: 0, n: 0 }, k: 0 }
         on go { z = if c then x else y }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["z.i.m".to_string(), "z.i.n".to_string(), "z.k".to_string()],
    );
    assert_eq!(gate_count(&r, "BrickComponentType_Expr_Select"), 3);
}

#[test]
fn record_valued_ternary_into_a_subfield_selects_each_leaf() {
    let r = compile(
        "type Inner = { m: int, n: int }
         type Outer = { i: Inner, k: int }
         in go: exec
         in c: bool
         var a: Inner = { m: 1, n: 2 }
         var b: Inner = { m: 3, n: 4 }
         var o: Outer = { i: { m: 0, n: 0 }, k: 0 }
         on go { o.i = if c then a else b }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["o.i.m".to_string(), "o.i.n".to_string()],
    );
}

#[test]
fn record_valued_ternary_wires_every_output_pin() {
    // The `out` position bailed the same way, leaving BOTH boundary pins
    // dangling, which reads in game as an unconnected chip output.
    let r = compile(
        "type P = { a: int, b: int }
         in c: bool
         var x: P = { a: 1, b: 2 }
         var y: P = { a: 3, b: 4 }
         out res: P = if c then x else y
",
    );
    assert_no_errors(&r);
    let out_pins: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_Internal_MicrochipOutput")
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(out_pins.len(), 2, "one pin per leaf field");
    for pin in out_pins {
        assert!(
            r.module.wires.iter().any(|w| w.target.node_id == pin),
            "output pin {pin:?} is dangling"
        );
    }
}

// a NESTED record `in` / `out` boundary port

#[test]
fn nested_record_input_port_explodes_every_leaf() {
    // Port expansion iterated the top-level fields once with no recursion, so a
    // nested record field got ONE pin for its two leaves and `o = p` copied
    // only the flat field.
    let r = compile(
        "type Inner = { m: int, n: int }
         type Outer = { i: Inner, k: int }
         in go: exec
         in p: Outer
         var o: Outer = { i: { m: 0, n: 0 }, k: 0 }
         on go { o = p }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_Internal_MicrochipInput"),
        4,
        "one pin per leaf (m, n, k) plus the exec input"
    );
    assert_eq!(
        written_var_labels(&r),
        vec!["o.i.m".to_string(), "o.i.n".to_string(), "o.k".to_string()],
    );
}

#[test]
fn nested_record_output_port_wires_every_leaf() {
    let r = compile(
        "type Inner = { m: int, n: int }
         type Outer = { i: Inner, k: int }
         var o: Outer = { i: { m: 1, n: 2 }, k: 3 }
         out res: Outer = o
",
    );
    assert_no_errors(&r);
    let out_pins: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_Internal_MicrochipOutput")
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(out_pins.len(), 3, "one pin per leaf field");
    for pin in out_pins {
        assert!(
            r.module.wires.iter().any(|w| w.target.node_id == pin),
            "output pin {pin:?} is dangling"
        );
    }
}

// an enum-typed FIELD of a record

#[test]
fn enum_typed_record_field_gets_its_own_disc_and_slots() {
    // `record_field_storage` recursed for a nested record and special-cased
    // array/map, but had no enum arm, so an enum field fell through to the
    // scalar arm and collapsed the whole `__disc` + slots layout into one gate.
    let r = compile(
        "enum S { A(int), B(int) }
         type H = { s: S, k: int }
         var h: H = { s: S.A(0), k: 0 }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_Var"),
        4,
        "h.s.__disc + h.s.__A_0 + h.s.__B_0 + h.k"
    );
}

#[test]
fn assigning_an_enum_typed_record_field_writes_its_slots() {
    // The write side: the target field was a scalar `Binding::Var` against a
    // `Binding::Record` source, so `binding_to_port` yielded nothing and the
    // enum field's write vanished while the sibling scalar's landed.
    let r = compile(
        "enum S { A(int), B(int) }
         type H = { s: S, k: int }
         in go: exec
         in u: int
         var h: H = { s: S.A(0), k: 0 }
         var sv: S = S.B(5)
         on go { h = { s: sv, k: u } }
",
    );
    assert_no_errors(&r);
    let written = written_var_labels(&r);
    assert!(
        written.contains(&"h.s.__disc".to_string()),
        "the enum field's tag must be written, got {written:?}"
    );
    assert!(
        written.contains(&"h.k".to_string()),
        "the sibling scalar must still be written, got {written:?}"
    );
}

// a whole-record copy must carry array/map fields

#[test]
fn whole_record_copy_carries_an_array_field() {
    // `q = p` copied the scalar fields and left `q.xs` at its old contents,
    // making it a clean-compiling wrong copy. A container field needs its own
    // rebuild, not a skip.
    let r = compile(
        "type P = { a: int, xs: int[] }
         in go: exec
         var p: P = { a: 0, xs: [1, 2] }
         var q: P = { a: 0, xs: [9] }
         on go { q = p }
",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_Exec_ArrayVar_CopyFrom")
            || has_gate(&r, "BrickComponentType_Exec_ArrayVar_Clear"),
        "the array field must be rebuilt, not skipped; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
}

// a record-typed chip SIGNATURE output

#[test]
fn record_typed_chip_signature_output_emits_its_body() {
    // `chip Mk(x: int) -> (p: P)` gave the record output ONE pin, so the body's
    // `out p = { … }` had nowhere to land and emitted nothing at all, leaving
    // the instance an empty shell whose caller-side pins were never wired.
    let r = compile(
        "type P = { a: int, b: int }
         in u: int
         chip Mk(x: int) -> (p: P) { out p = { a: x, b: x } }
         out res: P = Mk(u)
",
    );
    assert_no_errors(&r);
    let mk = r
        .module
        .chips
        .values()
        .find(|c| crate::intern::resolve(c.name).starts_with("Mk"))
        .expect("the Mk chip instance");
    assert!(
        !mk.wires.is_empty(),
        "the chip body must emit wiring, got {} nodes / 0 wires",
        mk.nodes.len()
    );
    for (id, n) in &r.module.nodes {
        if n.gate_class == "BrickComponentType_Internal_MicrochipOutput" {
            assert!(
                r.module.wires.iter().any(|w| w.target.node_id == *id),
                "caller output pin {id:?} is dangling"
            );
        }
    }
}

#[test]
fn record_typed_chip_signature_output_writes_a_var() {
    // The exec form: `q = Mk(u)` used to emit ZERO Var_Set, so `q` was never
    // written.
    let r = compile(
        "type P = { a: int, b: int }
         in go: exec
         in u: int
         var q: P = { a: 0, b: 0 }
         chip Mk(x: int) -> (p: P) { out p = { a: x, b: x } }
         on go { q = Mk(u) }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["q.a".to_string(), "q.b".to_string()],
    );
}

/// A record is a compile-time bundle of per-field ports, not one wire, so
/// using one where a single value is expected (a string concatenation, or any
/// operator) cannot lower: WS003. Only a gate read gives an operand its first
/// output.
#[test]
fn a_record_value_in_a_scalar_position_is_an_error() {
    let concat = compile(
        "type Pt = { x: int, y: int }\n\
         let p: Pt = { x: 1, y: 2 }\n\
         out s = \"p=\" .. p\n",
    );
    assert!(
        concat
            .diagnostics
            .iter()
            .any(|d| d.code == "WS003" && d.severity == crate::diagnostic::Severity::Error),
        "a record concatenated into a string must be an error: {:?}",
        concat.diagnostics
    );

    // A multi-output chip result bound to a name is its outputs: as an
    // arithmetic operand it is its first output, as naming it is.
    let src = "in n: int\nchip Pair(v: int) -> (lo: int, hi: int) { out lo = v\nout hi = v + 1 }\n\
               let p = Pair(n)\nout total = p + 1\n";
    for fold in [false, true] {
        let got = super::shadow_differential::outcome(src, &[], fold);
        let want = super::shadow_differential::outcome(&src.replace("Pair(n)\n", "Pair(n).lo\n"), &[], fold);
        assert!(got.compiled, "{src}: {:?}", got.codes);
        assert_eq!(got, want, "{src}");
    }
}

/// The error must not fire on the shapes that DO auto-unwrap at lowering. A
/// multi-output BUILTIN gate result binds a `Binding::Local` on the gate's
/// primary port, and a call written inline lowers to its first output pin. Both
/// produce a real wire, so both stay legal.
#[test]
fn auto_unwrapping_multi_output_results_stay_legal() {
    let builtin_let = compile(
        "in go: exec\nvar out1: int = 0\n\
         on go { let pi = ParseInt(\"42\")\nout1 = pi + 1 }\n",
    );
    assert_no_errors(&builtin_let);
    assert!(
        !builtin_let.diagnostics.iter().any(|d| d.code == "WS071"),
        "a builtin multi-output result reads its primary port: {:?}",
        builtin_let.diagnostics
    );

    let inline_chip = compile(
        "in n: int\n\
         chip Pair(v: int) -> (lo: int, hi: int) { out lo = v\nout hi = v }\n\
         out total = Pair(n) + 1\n",
    );
    assert_no_errors(&inline_chip);
    assert!(
        !inline_chip.diagnostics.iter().any(|d| d.code == "WS071"),
        "an inline chip call unwraps to its first output pin: {:?}",
        inline_chip.diagnostics
    );
}

/// `*T` on a record parameter distributes over the record's fields, so a chip
/// can write through to the caller's storage. A record is one gate PER FIELD, so
/// a single scalar ref pin has nothing to point at.
#[test]
fn a_record_ref_chip_param_writes_through_to_the_callers_fields() {
    let r = compile(
        "type T = { a: float, b: float }\n\
         chip M(s: *T, x: float) { s.a = x\ns.b = x }\n\
         var g: T = { a: 1.0, b: 2.0 }\n\
         var src: float = 0.0\n\
         in go: exec\n\
         on go { M(g, src) }\n",
    );
    assert_no_errors(&r);

    // One ref pin per field, each sourced from a DIFFERENT backing var: a single
    // shared pin would write both fields to the same storage.
    let ref_sources: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.port == WirePort::VarRef)
        .filter(|w| {
            r.module
                .nodes
                .get(&w.source.node_id)
                .is_some_and(|n| n.gate_class == "BrickComponentType_Var")
        })
        // Only the refs that cross INTO the chip; a plain var also drives its
        // own `Var_Get` through a VarRef wire inside the caller.
        .filter(|w| w.target.port == WirePort::RerInput)
        .map(|w| w.source.node_id)
        .collect();
    let distinct: std::collections::HashSet<_> = ref_sources.iter().collect();
    assert_eq!(
        distinct.len(),
        2,
        "each field must pass its own var ref, got {ref_sources:?}"
    );

    fn set_count(m: &crate::ir::Module) -> usize {
        m.nodes
            .values()
            .filter(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
            .count()
            + m.chips.values().map(set_count).sum::<usize>()
    }
    assert_eq!(set_count(&r.module), 2, "both field writes must emit a Var_Set");

    fn undriven_pins(m: &crate::ir::Module, all: &crate::ir::Module) -> usize {
        let mine = m
            .inputs
            .iter()
            .filter(|id| !all.wires.iter().any(|w| w.target.node_id == **id))
            .count();
        mine + m.chips.values().map(|c| undriven_pins(c, all)).sum::<usize>()
    }
    let module = r.module.clone();
    assert_eq!(
        r.module.chips.values().map(|c| undriven_pins(c, &module)).sum::<usize>(),
        0,
        "no chip input pin may be left unwired"
    );
}

/// The distribution is a type-level rewrite, so a `mod` (inline expansion) and a
/// `chip` (a real microchip boundary) accept the same signature.
#[test]
fn a_record_ref_param_works_for_mods_and_chips_alike() {
    for kw in ["mod", "chip"] {
        let r = compile(&format!(
            "type T = {{ a: float, b: float }}\n\
             {kw} M(s: *T, x: float) {{ s.a = x }}\n\
             var g: T = {{ a: 1.0, b: 2.0 }}\n\
             var src: float = 0.0\n\
             in go: exec\n\
             on go {{ M(g, src) }}\n"
        ));
        assert_no_errors(&r);
    }
}

/// A tuple `var` decomposes into one backing gate per element, and `g.0 = v`
/// writes the matching gate.
#[test]
fn a_tuple_var_gets_per_element_storage_and_writable_elements() {
    let r = compile(
        "var g: (float, float) = (1.0, 2.0)\n\
         var src: float = 0.0\n\
         in go: exec\n\
         on go { g.0 = src\ng.1 = src }\n",
    );
    assert_no_errors(&r);
    assert_eq!(
        written_var_labels(&r),
        vec!["g.0".to_string(), "g.1".to_string()],
        "each element must be written through its own backing gate"
    );
}

/// `*T` distributes over a TUPLE the way it does over a record, through a type
/// alias as well as the inline spelling.
#[test]
fn a_tuple_ref_chip_param_writes_through_to_the_caller() {
    for ty in ["P", "(float, float)"] {
        let r = compile(&format!(
            "type P = (float, float)\n\
             chip M(s: *{ty}, x: float) {{ s.0 = x }}\n\
             var g: P = (1.0, 2.0)\n\
             var src: float = 0.0\n\
             in go: exec\n\
             on go {{ M(g, src) }}\n"
        ));
        assert_no_errors(&r);
        let crossing = r.module.wires.iter().any(|w| {
            w.source.port == WirePort::VarRef
                && w.target.port == WirePort::RerInput
                && r.module
                    .nodes
                    .get(&w.source.node_id)
                    .is_some_and(|n| n.gate_class == "BrickComponentType_Var")
        });
        assert!(crossing, "`*{ty}` must pass an element ref into the chip");
    }
}

/// `*Enum` distributes over the enum's slot columns (`__disc` plus the payload
/// slots), so a chip can reassign the caller's enum variable.
#[test]
fn an_enum_ref_chip_param_writes_every_slot_of_the_caller() {
    let r = compile(
        "enum Shape { Empty, Circle { r: float } }\n\
         chip M(s: *Shape) { s = Shape.Empty }\n\
         var g: Shape = Shape.Circle { r: 1.0 }\n\
         in go: exec\n\
         on go { M(g) }\n",
    );
    assert_no_errors(&r);
    // Both of the enum's backing gates (`__disc` and the `r` payload slot) must
    // reach the chip as refs; one alone means a variant switch that leaves stale
    // payload behind.
    let refs: std::collections::HashSet<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.port == WirePort::VarRef && w.target.port == WirePort::RerInput)
        .filter(|w| {
            r.module
                .nodes
                .get(&w.source.node_id)
                .is_some_and(|n| n.gate_class == "BrickComponentType_Var")
        })
        .map(|w| w.source.node_id)
        .collect();
    assert_eq!(refs.len(), 2, "both enum slots must cross as refs, got {refs:?}");
}

/// `rec.field.Value` is the explicit read of a ref-typed record field. Only
/// `x.Value` on a plain identifier was handled, so the field spelling degraded
/// to an `_Unsupported` placeholder and its consumer got no wire.
#[test]
fn dot_value_on_a_ref_record_field_reads_the_backing_var() {
    let r = compile(
        "type S = { a: float, b: bool }\n\
         chip M(s: *S, f: float) -> (r: float) { out r = if s.b.Value then s.a.Value else f }\n\
         var g: S = { a: 1.0, b: false }\n\
         var res: float = 0.0\n\
         var src: float = 0.0\n\
         in go: exec\n\
         on go { res = M(g, src) }\n",
    );
    assert_no_errors(&r);
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WSP001"),
        "`.Value` on a record field must resolve: {:?}",
        r.diagnostics
    );
    fn select_inputs(m: &crate::ir::Module) -> usize {
        let sel: Vec<_> = m
            .nodes
            .values()
            .filter(|n| n.gate_class == "BrickComponentType_Expr_Select")
            .map(|n| n.id)
            .collect();
        m.wires
            .iter()
            .filter(|w| sel.contains(&w.target.node_id))
            .count()
            + m.chips.values().map(select_inputs).sum::<usize>()
    }
    assert!(
        select_inputs(&r.module) >= 2,
        "the Select must be driven, not left dangling"
    );
}

/// The array behind a record field can be any spelling a bare name accepts —
/// not just a `var`. An `in rom0: int[]` bound into a record (`let m: Mem = {
/// rom: rom0 }`) and then indexed through the field (`m.rom[i]`) must lower
/// exactly like the direct `rom0[i]`: the same `ArrayVar_Get`, fed from the
/// same ref source. Only `Binding::Var` used to resolve through a field chain,
/// so this — the cartridge/socket idiom, where a record describes a memory map
/// whose banks arrive on ports — emitted an `_Unsupported` placeholder that
/// read 0 forever behind nothing louder than a WSP001 warning.
#[test]
fn input_array_through_record_field_resolves() {
    let r = compile(
        "type Mem = { rom: int[] }\n\
         in rom0: int[]\n\
         in go: exec\n\
         var a: int = 0\n\
         var b: int = 0\n\
         var n: int = 0\n\
         on go {\n\
           let m: Mem = { rom: rom0 }\n\
           a = m.rom[3]\n\
           b = rom0[3]\n\
           n = m.rom.length()\n\
           m.rom[1] = 7\n\
         }",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "indexing an input array through a record field must not placeholder"
    );
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WSP001"),
        "no lowering placeholder may be reported: {:?}",
        r.diagnostics
    );
    // `m.rom[3]` and `rom0[3]` — one real read each.
    assert_eq!(gate_count(&r, gc::ARRAY_GET), 2);
    assert_eq!(gate_count(&r, gc::ARRAY_SET_AT_INDEX), 1, "`m.rom[1] = 7` writes");
    assert_eq!(gate_count(&r, gc::ARRAY_GET_LENGTH), 1, "`m.rom.length()` reads");
    // Every one of those four ops must take its ref from the SAME place the
    // direct spelling does: the input's rerouter output.
    let sources: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.port == crate::ir::port_registry::WirePort::ArrayVarRef)
        .map(|w| (w.source.node_id, w.source.port))
        .collect();
    assert_eq!(sources.len(), 4, "one ArrayVarRef wire per array op: {sources:?}");
    assert!(
        sources.iter().all(|s| *s == sources[0]),
        "field-reached and direct spellings must share one ref source: {sources:?}"
    );
}

/// The map twin of `input_array_through_record_field_resolves`: an `in` map
/// bound into a record and read back through the field (`m.tbl[k]` /
/// `m.tbl.get(k)`) resolves to real `MapVar_Get` gates off the input's
/// rerouter, instead of a silent placeholder.
#[test]
fn input_map_through_record_field_resolves() {
    let r = compile(
        "type Bus = { tbl: Map<int, int> }\n\
         in tbl0: Map<int, int>\n\
         in go: exec\n\
         var a: int = 0\n\
         var b: int = 0\n\
         on go {\n\
           let m: Bus = { tbl: tbl0 }\n\
           a = m.tbl[3]\n\
           b = m.tbl.get(4)\n\
         }",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WSP001"),
        "no lowering placeholder may be reported: {:?}",
        r.diagnostics
    );
    assert_eq!(gate_count(&r, gc::MAP_GET), 2);
    let sources: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.port == crate::ir::port_registry::WirePort::MapVarRef)
        .map(|w| (w.source.node_id, w.source.port))
        .collect();
    assert_eq!(sources.len(), 2, "one MapVarRef wire per map op: {sources:?}");
    assert!(sources.iter().all(|s| *s == sources[0]));
}

/// A record field holding an array that arrived on a WIRE rather than from a
/// named container — here a `let`-bound select over two input banks — is the
/// third spelling the field chain has to accept. `let r = m.rom; r[i]` already
/// worked (the ref rides the local's port); `m.rom[i]` must too.
#[test]
fn wire_borne_array_through_record_field_resolves() {
    let r = compile(
        "type Mem = { rom: int[] }\n\
         in rom0: int[]\n\
         in rom1: int[]\n\
         in hi: bool\n\
         in go: exec\n\
         var a: int = 0\n\
         on go {\n\
           let m: Mem = { rom: if hi then rom1 else rom0 }\n\
           a = m.rom[3]\n\
         }",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WSP001"),
        "no lowering placeholder may be reported: {:?}",
        r.diagnostics
    );
    assert_eq!(gate_count(&r, gc::ARRAY_GET), 1);
    // The ref comes off the Select, so the bank choice reaches the read.
    let sel = find_gate(&r, "BrickComponentType_Expr_Select");
    let get = find_gate(&r, gc::ARRAY_GET);
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == sel
            && w.target.node_id == get
            && w.target.port == crate::ir::port_registry::WirePort::ArrayVarRef),
        "the selected bank must drive the read's ArrayVarRef"
    );
}

/// The ids of every gate of `class`.
fn gate_ids(r: &LowerResult, class: &str) -> Vec<crate::ir::NodeId> {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .map(|n| n.id)
        .collect()
}

/// The node feeding `target`'s `port`, if any.
fn port_source(r: &LowerResult, target: crate::ir::NodeId, port: WirePort) -> Option<crate::ir::NodeId> {
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == target && w.target.port == port)
        .map(|w| w.source.node_id)
}

/// `rec.xs = [1, 2]` on a record's array field rebuilds that field's ArrayVar
/// like the bare `xs = [1, 2]` (Clear + one Push per item), never the scalar
/// field-write path (an `_Unsupported` literal feeding a Var_Set on a `VarRef`
/// port the ArrayVar does not have).
#[test]
fn record_array_field_literal_assign_clears_and_pushes() {
    let r = compile(
        "type R = { xs: int[], n: int }\nvar rec: R\nin go: exec\non go { rec.xs = [1, 2] }",
    );
    assert!(r.diagnostics.is_empty(), "no diagnostics: {:?}", r.diagnostics);
    assert!(!has_gate(&r, gc::UNSUPPORTED));
    assert!(!has_gate(&r, gc::VAR_SET));
    let arr = gate_ids(&r, gc::ARRAY_VAR);
    assert_eq!(arr.len(), 1);
    let clear = gate_ids(&r, gc::ARRAY_CLEAR);
    let pushes = gate_ids(&r, gc::ARRAY_PUSH);
    assert_eq!((clear.len(), pushes.len()), (1, 2));
    for g in clear.iter().chain(&pushes) {
        assert_eq!(port_source(&r, *g, WirePort::ArrayVarRef), Some(arr[0]));
    }
    for p in &pushes {
        assert!(wired_reachable(&r, clear[0], *p), "pushes run after the clear");
    }
}

/// The map spelling: `rec.m = { .. }` is a Clear + one Set per entry on the
/// field's own MapVar.
#[test]
fn record_map_field_literal_assign_clears_and_sets() {
    let r = compile(
        "type R = { m: Map<string, int>, n: int }\nvar rec: R\nin go: exec\n\
         on go { rec.m = { \"a\": 1 } }",
    );
    assert!(r.diagnostics.is_empty(), "no diagnostics: {:?}", r.diagnostics);
    assert!(!has_gate(&r, gc::UNSUPPORTED));
    assert!(!has_gate(&r, gc::VAR_SET));
    let map = gate_ids(&r, gc::MAP_VAR);
    assert_eq!(map.len(), 1);
    let clear = gate_ids(&r, gc::MAP_CLEAR);
    let sets = gate_ids(&r, gc::MAP_SET);
    assert_eq!((clear.len(), sets.len()), (1, 1));
    assert_eq!(port_source(&r, clear[0], WirePort::MapVarRef), Some(map[0]));
    assert_eq!(port_source(&r, sets[0], WirePort::MapVarRef), Some(map[0]));
    assert!(wired_reachable(&r, clear[0], sets[0]), "the set runs after the clear");
}

/// A non-literal whole-container assign to a record field is WS027, as it is
/// for a bare container var.
#[test]
fn record_container_field_non_literal_assign_is_ws027() {
    let r = compile(
        "type R = { xs: int[], n: int }\nvar rec: R\nvar ys: int[]\nin go: exec\non go { rec.xs = ys }",
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS027"
            && d.severity == crate::diagnostic::Severity::Error
            && d.message.contains("rec.xs.copyFrom")),
        "expected WS027 naming rec.xs: {:?}",
        r.diagnostics
    );
    assert!(!has_gate(&r, gc::VAR_SET));
}

/// `{}` where a map is expected is the empty map literal, in every slot a map
/// literal fills: a map field of a stored record's literal (initializer and
/// spread assign), a handler-local map `var`, a whole-map assign, and a map
/// argument.
#[test]
fn empty_braces_are_an_empty_map_where_a_map_is_expected() {
    let r = compile(
        "type H = { n: int, m: Map<int, int> }\n\
         var h: H = { n: 1, m: {} }\n\
         var mm: Map<int, int> = {}\n\
         var r: int = 0\n\
         mod f(m: Map<int, int>) -> int { return m.length() }\n\
         in go: exec\n\
         on go {\n\
           h = { ...h, m: {} }\n\
           var lm: Map<int, int> = {}\n\
           lm.set(1, 5)\n\
           mm = {}\n\
           r = lm.get(1) + f({})\n\
         }",
    );
    assert!(r.diagnostics.is_empty(), "no diagnostics: {:?}", r.diagnostics);
    assert!(!has_gate(&r, gc::UNSUPPORTED));
    // h.m, mm, lm, and the argument's own empty map.
    assert_eq!(gate_count(&r, gc::MAP_VAR), 4);
    // `h = { ...h, m: {} }`, `var lm = {}` and `mm = {}` each empty their map.
    assert_eq!(gate_count(&r, gc::MAP_CLEAR), 3);
    assert_eq!(gate_count(&r, gc::MAP_SET), 1);
}

/// `{}` for a map in a position no map literal fills is the map literal's
/// position error, not a record-versus-map type mismatch.
#[test]
fn empty_braces_as_a_map_outside_a_map_literal_slot_is_ws026() {
    let codes = errs("var r: int = 0\nin go: exec\non go { let m: Map<int, int> = {}\n r = m.length() }");
    assert!(codes.contains(&"WS026".to_string()), "{codes:?}");
    assert!(!codes.contains(&"WS003".to_string()), "{codes:?}");
}

/// A record local in a mod is declared once per call site: the body pre-pass
/// declared its field vars, and the statement declared a second, unwired set.
#[test]
fn a_record_local_in_a_mod_is_declared_once_per_call() {
    let src = "type P = { x: int, y: int }\nvar n: int = 0\nout onn: int = n\n\
               mod f() -> int {\n  var local: P = { x: 1, y: 2 }\n  local.x += 10\n  var fresh: P\n  fresh.y += 1\n  return local.x + fresh.y\n}\n\
               in go: exec\non go {\n  n = f()\n  n = n + f()\n}";
    for r in [compile(src), compile_folded(src)] {
        assert_no_errors(&r);
        let vars: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::VAR && matches!(n.source_range.start.line, 5 | 7))
            .map(|n| n.id)
            .collect();
        assert_eq!(vars.len(), 8, "two fields of two locals, at each of two calls");
        for v in vars {
            assert!(r.module.wires.iter().any(|w| w.source.node_id == v), "an unwired field var {v:?}");
        }
    }
}

/// The annotation a WS105 suggests compiles as written, with no diagnostic,
/// and drives every pin: for a record, a tuple, a nested record, a mod's
/// record result (named by its alias), a gate's named outputs (`Edge`, whose
/// suggested type nested the record in its own first field and compiled with
/// that field's pins undriven) and an output declared apart from its write
/// (the annotation goes on the declaration).
#[test]
fn a_ws105_hint_compiles_as_written() {
    let head = "type P = { a: int, b: float }\nin go: exec\nin bb: bool\nvar k: int = 3\nvar fl: float = 1.5\n\
                var n: int = 0\nmod mk(x: int) -> P { return { a: x, b: 0.5 } }\non go { n = mk(k).a }\n";
    for untyped in [
        "on go { out z = { a: k, b: fl } }",
        "on go { out z = (k, fl) }",
        "on go { out z = { a: k, b: { c: fl } } }",
        "on go { out z = mk(k) }",
        "out z = Edge(bb)",
        "on go { out z = Edge(bb) }",
        "out z\non go { emit z = (k, fl) }",
        "out z\non go { emit z = mk(k) }",
    ] {
        let src = format!("{head}{untyped}");
        let r = compile(&src);
        let d = r.diagnostics.iter().find(|d| d.code == "WS105").unwrap_or_else(|| panic!("`{untyped}`: {:?}", r.diagnostics));
        let hint = d.message.rsplit_once("pin: `").unwrap().1.trim_end_matches('`');
        let fixed = match hint.strip_suffix(" = ...") {
            Some(decl) => src.replacen("out z = ", &format!("{decl} = "), 1),
            None => src.replacen("out z\n", &format!("{hint}\n"), 1),
        };
        assert_ne!(fixed, src, "`{untyped}`: the hint `{hint}` names no place");
        if untyped.contains("mk(") {
            assert!(hint.contains("out z: P"), "`{untyped}`: {hint}");
        }
        for fold in [false, true] {
            let r = if fold { compile_folded(&fixed) } else { compile(&fixed) };
            assert!(r.diagnostics.is_empty(), "`{hint}` fold {fold}: {:?}", r.diagnostics);
            let outputs: Vec<_> = r.module.nodes.values().filter(|n| n.kind == crate::ir::NodeKind::Output).collect();
            assert!(outputs.len() >= 2, "`{hint}`: {} pins", outputs.len());
            for o in outputs {
                assert!(r.module.wires.iter().any(|w| w.target.node_id == o.id), "`{hint}` fold {fold}: an undriven pin");
            }
        }
    }
}

/// A record or tuple written to a module output with no type was one pin that
/// nothing drove: WS105, in a handler, at the top level, through `emit` and in
/// an anonymous chip. The annotated twin drives a pin per field, and a value
/// the port carries as one wire (a gate's output read, an enum tag) or a
/// record output of a mod or chip still compiles.
#[test]
fn a_record_written_to_an_output_with_no_type_is_ws105() {
    let head = "type P = { a: int, b: float }\nenum Hue { Red, Green }\nin go: exec\nin bb: bool\n\
                var pv: P = { a: 1, b: 2.0 }\nvar k: int = 3\nvar fl: float = 1.5\nvar c: Hue = Hue.Green\n\
                var fs: float[] = [1.5]\nvar n: int = 0\n";
    let codes = |r: &LowerResult| -> Vec<String> {
        r.diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.to_string())
            .collect()
    };
    let driven = |r: &LowerResult| {
        r.module
            .nodes
            .values()
            .filter(|n| n.kind == crate::ir::NodeKind::Output)
            .filter(|n| r.module.wires.iter().any(|w| w.target.node_id == n.id))
            .count()
    };
    for (untyped, typed) in [
        ("on go { out z = pv }", "on go { out z: P = pv }"),
        ("out z = pv", "out z: P = pv"),
        ("on go { out z = { a: k, b: fl } }", "on go { out z: P = { a: k, b: fl } }"),
        ("on go { out z = (k, fl) }", "on go { out z: (int, float) = (k, fl) }"),
        ("out z = { a: k, b: fl }", "out z: P = { a: k, b: fl }"),
        ("out z\non go { emit z = pv }", "out z: P\non go { emit z = pv }"),
        ("chip { out z = { a: k, b: fl } }", "chip { out z: P = { a: k, b: fl } }"),
    ] {
        let r = compile(&format!("{head}{untyped}"));
        assert_eq!(codes(&r), ["WS105"], "`{untyped}`");
        let d = r.diagnostics.iter().find(|d| d.code == "WS105").unwrap();
        assert!(d.message.contains("`out z: "), "{}", d.message);
        let r = compile(&format!("{head}{typed}"));
        assert_no_errors(&r);
        assert!(driven(&r) >= 2, "`{typed}`: each field drives its own pin");
    }
    for ok in [
        "let e = Edge(bb)\nout rising = e.Rising",
        "on go { out z = fs.max() }",
        "on go { out z = c }",
        "out z: bool = Edge(bb)",
        "on go { out z: bool = Edge(bb) }",
        "mod mk(x: int) -> (p: P) { out p = { a: x, b: 2.5 } }\non go { let r = mk(k)\n n = r.a }",
        "chip C(x: int) -> (p: P) { out p = { a: x, b: 2.5 } }\non go { let r = C(k)\n n = r.a }",
    ] {
        let r = compile(&format!("{head}{ok}"));
        assert_no_errors(&r);
    }
}

/// A record of the program's own where one value goes is `WS003` on every
/// route, naming the record; the caller of `return q` into an `int` output
/// stored the exec wire. A multi-output read still gives its first output.
#[test]
fn a_record_where_one_value_goes_is_ws003() {
    let head = "var n: int = 0\nout on1: int = n\nin go: exec\nin a: int\nin bb: bool\n\
                type Q = { a: int, b: int }\nvar pv: Q = { a: 7, b: 2 }\nvar xs: int[]\n\
                var rr: { k: int, j: int } = { k: 0, j: 0 }\nvar ps: Q[]\n";
    for (what, body) in [
        ("return", "mod C(q: Q) -> int { return q }\non go { n = C(pv) }"),
        ("emit", "mod C(q: Q) -> (r: int) { emit r = q }\non go { n = C(pv) }"),
        ("out", "mod C(q: Q) -> (r: int) { out r = q }\non go { n = C(pv) }"),
        ("assignment", "on go { n = pv }"),
        ("argument", "mod D(v: int) -> int { return v + 1 }\non go { n = D(pv) }"),
        ("record field", "on go { rr = { k: pv, j: 1 } }"),
        ("push", "on go { xs.push(pv) }"),
        ("record literal", "on go { n = { a: 1, b: 2 } }"),
        ("one-output mod", "mod F() -> Q { return { a: 3, b: 4 } }\non go { n = F() }"),
        ("element", "on go { n = ps[0] }"),
        ("top-level out", "out z: int = pv"),
    ] {
        let r = compile(&format!("{head}{body}"));
        let m = r.diagnostics.iter().map(|d| format!("[{}] {}", d.code, d.message)).collect::<Vec<_>>().join("
");
        // A literal names no alias, only its fields.
        let got = if what == "record literal" { "got record `{a: int, b: int}`" } else { "got record `Q`" };
        assert!(r.diagnostics.iter().any(|d| d.code == "WS003" && d.message.contains(got)), "{what}: {m}");
        assert!(!m.contains("WS071"), "{what}: one error for the record: {m}");
        assert!(!m.contains("WS105"), "{what}: the output has a type: {m}");
    }
    // A `let` of named outputs written to an output declared `bool`, and
    // assigned, gives each its first output, as naming it does.
    let src = "in go: exec\nin bb: bool\nvar bo: bool = false\nlet e = Edge(bb)\nout o: bool = e\non go { bo = e }";
    for fold in [false, true] {
        let got = super::shadow_differential::outcome(src, &[], fold);
        let want = super::shadow_differential::outcome(&src.replace("Edge(bb)", "Edge(bb).Rising"), &[], fold);
        assert!(got.compiled, "{src}: {:?}", got.codes);
        assert_eq!((got.comps, got.codes), (want.comps, want.codes), "{src}");
    }
    let head = "var n: int = 0\nout on1: int = n\nin go: exec\nin a: int\nin bb: bool\n\
                var e: bool = false\nvar fs: float[]\nvar f: float = 0.0\n";
    for ok in [
        "on go { e = Edge(bb) }",
        "on go { f = fs.max() }",
        "on go { let r = fs.pop()\n f = r }",
        "mod g(x: int) -> (p: int, q: int) { out p = x + 1\n out q = x + 2 }\non go { n = g(a) }",
        "mod g(x: int) -> (r: int, e: exec) { emit r = x\n emit e }\non go { n = g(a) }",
        "var vv: vector = Vec(1.0, 2.0, 3.0)\non go { f = SplitVec(vv) }",
        "mod g(x: int) -> (p: int, q: int) { out p = x + 1\n out q = x + 2 }\non go { f = g(a) }",
        "out z: bool = Edge(bb)",
    ] {
        let r = compile(&format!("{head}{ok}"));
        assert_no_errors(&r);
    }
    // Where one value of another type goes, a multi-output read gives its
    // first output converted as a value of that type is: it saves what the
    // program naming the output saves.
    let head = format!(
        "{head}var bo: bool = false\nvar so: string = \"\"\nvar vv: vector = Vec(1.0, 2.0, 3.0)\nvar ns: int[]\n\
         var is: int[]\nvar m: Map<string, float>\nmod g(x: int) -> (p: int, q: int) {{ out p = x\n out q = x }}\n\
         mod D(v: bool) -> int {{ return 1 }}\nmod one(v: int) -> int {{ return v }}\n"
    );
    for (body, named) in [
        ("on go { bo = SplitVec(vv) }", "on go { bo = SplitVec(vv).x }"),
        ("on go { so = SplitVec(vv) }", "on go { so = SplitVec(vv).x }"),
        ("out z: int = Edge(bb)", "out z: int = Edge(bb).Rising"),
        ("on go { n = D(SplitVec(vv)) }", "on go { n = D(SplitVec(vv).x) }"),
        ("on go { bo = g(a) }", "on go { bo = g(a).p }"),
        ("on go { n = fs.max() }", "on go { n = fs.max().Value }"),
        ("on go { n = fs.pop() }", "on go { n = fs.pop().Value }"),
        ("on go { n = m.get(\"k\") }", "on go { n = m.get(\"k\").Value }"),
        ("on go { bo = is.pop() }", "on go { bo = is.pop().Value }"),
        ("on go { so = is.max() }", "on go { so = is.max().Value }"),
        ("on go { n = Edge(bb) }", "on go { n = Edge(bb).Rising }"),
        ("on go { f = Edge(bb) }", "on go { f = Edge(bb).Rising }"),
        ("on go { n = one(fs.max()) }", "on go { n = one(fs.max().Value) }"),
        ("on go { is.push(fs.max()) }", "on go { is.push(fs.max().Value) }"),
        ("on go { let t = is.pop()\n bo = t }", "on go { let t = is.pop().Value\n bo = t }"),
    ] {
        for fold in [false, true] {
            let got = super::shadow_differential::outcome(&format!("{head}{body}"), &[], fold);
            let want = super::shadow_differential::outcome(&format!("{head}{named}"), &[], fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert_eq!(got, want, "{body}, fold {fold}");
        }
    }
}

/// A gate read gives a place for one value its first output wherever it
/// reaches one, held in a `let`, a record field, a tuple, an argument or a
/// generic parameter on the way: it saves the program its twin with the
/// output named (`fs.max().Value`) saves, in both fold modes. A record of the
/// program's own in the same places is an error.
#[test]
fn a_gate_read_gives_its_first_output_in_every_place() {
    use super::shadow_differential::outcome;
    let head = "in go: exec\nin a: int\nin bb: bool\nvar n: int = 0\nout on1: int = n\nvar f: float = 0.0\n\
                out of1: float = f\nvar bo: bool = false\nout obo: bool = bo\nvar s: string = \"\"\nout os: string = s\n\
                var vv: vector = Vec(1.0, 2.0, 3.0)\nvar fs: float[] = [1.0, 2.0]\nvar ns: int[] = [1, 2]\n\
                var bs: bool[]\nvar ss: string[]\nvar mp: Map<string, int> = { \"k\": 1 }\n\
                var ms: Map<string, string> = { \"k\": \"v\" }\ntype Q = { a: int, b: int }\nvar pv: Q = { a: 7, b: 2 }\n\
                mod g(x: int) -> (p: int, q: int) { out p = x + 1\nout q = x + 2 }\n";
    // Each read, the output a place for one value reads, its type, and a
    // var and an array of that type.
    let reads = [
        ("fs.max()", "Value", "float", "f", "fs"),
        ("fs.min()", "Value", "float", "f", "fs"),
        ("fs.average()", "Value", "float", "f", "fs"),
        ("ns.find(2)", "Index", "int", "n", "ns"),
        ("ns.pop()", "Value", "int", "n", "ns"),
        ("ns.get(0)", "Value", "int", "n", "ns"),
        ("mp.get(\"k\")", "Value", "int", "n", "ns"),
        ("ms.get(\"k\")", "Value", "string", "s", "ss"),
        ("Edge(bb)", "Rising", "bool", "bo", "bs"),
        ("SplitVec(vv)", "x", "float", "f", "fs"),
        ("g(a)", "p", "int", "n", "ns"),
    ];
    // `R` the read, `TY` its output's type, `X` and `XS` a var and an array.
    let places = [
        ("let", "on go { let t = R\nX = t }"),
        ("let record field", "on go { let t = { m: R, k: 1 }\nX = t.m }"),
        ("annotated let", "on go { let t: TY = R\nX = t }"),
        ("annotated let record field", "type T3 = { m: TY, k: int }\non go { let t: T3 = { m: R, k: 1 }\nX = t.m }"),
        ("untyped var", "on go { var t = R\nX = t }"),
        ("assignment", "on go { X = R }"),
        ("push", "on go { XS.push(R)\nX = XS[0] }"),
        ("push from a let field", "on go { let t = { m: R }\nXS.push(t.m)\nX = XS[0] }"),
        ("argument", "mod D(v: TY) -> TY { return v }\non go { X = D(R) }"),
        ("argument from a let field", "mod D(v: TY) -> TY { return v }\non go { let t = { m: R }\nX = D(t.m) }"),
        ("argument from a let", "mod D(v: TY) -> TY { return v }\non go { let t = R\nX = D(t) }"),
        ("generic argument from a let", "mod id<T>(x: T) -> T { return x }\non go { let t = R\nX = id(t) }"),
        ("interpolation", "on go { s = \"v${R}\" }"),
        ("return", "mod D() -> TY { return R }\non go { X = D() }"),
        ("emit", "mod D() -> (r: TY) { emit r = R }\non go { X = D() }"),
        ("out", "mod D() -> (r: TY) { out r = R }\non go { X = D() }"),
        ("comparison operand", "on go { bo = R == R }"),
        ("operand from a let", "on go { let x = R\nbo = x == x }"),
        ("condition", "on go { if R { n = 1 } }"),
        ("array literal element", "on go { var xs: TY[] = [R]\nX = xs[0] }"),
        ("tuple element", "on go { let t = (R, 1)\nX = t.0 }"),
        ("generic argument", "mod id<T>(x: T) -> T { return x }\non go { X = id(R) }"),
        ("stored record field", "var rr: { m: TY, k: int }\non go { rr = { m: R, k: 1 }\nX = rr.m }"),
    ];
    let spell = |place: &str, read: &str, ty: &str, x: &str, xs: &str| {
        format!("{head}{}", place.replace("TY", ty).replace("XS", xs).replace('X', x).replace('R', read))
    };
    // A `let` naming the output it holds labels its gate with its name.
    let outcome = |src: &str, fold: bool| {
        let mut o = outcome(src, &[], fold);
        for s in &mut o.shape {
            *s = s.split(' ').filter(|t| !t.is_empty() && !t.starts_with("_bindname=")).collect::<Vec<_>>().join(" ");
        }
        o.shape.sort();
        o
    };
    let mut pairs = 0;
    for (read, first, ty, x, xs) in reads {
        for (what, place) in places {
            pairs += 1;
            let src = spell(place, read, ty, x, xs);
            let twin = spell(place, &format!("{read}.{first}"), ty, x, xs);
            for fold in [false, true] {
                let (got, want) = (outcome(&src, fold), outcome(&twin, fold));
                assert!(got.compiled && want.compiled, "{read} {what}, fold {fold}: {:?} {:?}\n{src}", got.codes, want.codes);
                assert_eq!(got, want, "{read} {what}, fold {fold}: not its first output\n{src}");
            }
        }
    }
    assert_eq!(pairs, 11 * 23);
    // A chip's parameter takes the first output of a read held in a `let`, of
    // one in a record field, and of one written inline.
    let src = "in bb: bool\nin vv: vector\nchip C(v: bool) -> (r: bool) { out r = v }\nlet e = Edge(bb)\n\
               let q = C(e)\nout o: bool = q\nchip F(v: float) -> (r: float) { out r = v }\n\
               let s = { m: SplitVec(vv) }\nout of: float = F(s.m)\nout og: float = F(SplitVec(vv))\n";
    let twin = src.replace("Edge(bb)\n", "Edge(bb).Rising\n").replace("SplitVec(vv)", "SplitVec(vv).x");
    for fold in [false, true] {
        let (got, want) = (outcome(src, fold), outcome(&twin, fold));
        assert!(got.compiled, "chip arguments: {:?}", got.codes);
        assert_eq!(got, want, "chip arguments, fold {fold}");
    }
    // A record of the program's own reads no output: each place rejects it,
    // with WS003 where one value goes.
    let mut got = Vec::new();
    for (what, place) in places {
        let src = spell(place, "pv", "int", "n", "ns");
        let mut codes: Vec<String> = compile(&src)
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.to_string())
            .collect();
        codes.dedup();
        got.push(format!("{what}: {}", codes.join(" ")));
    }
    let want = [
        "let: WS003",
        "let record field: WS003",
        "annotated let: WS071 WS003",
        "annotated let record field: WS003",
        "untyped var: WS003",
        "assignment: WS003",
        "push: WS003",
        "push from a let field: WS003",
        "argument: WS003",
        "argument from a let field: WS003",
        "argument from a let: WS003",
        "generic argument from a let: WS033 WS003",
        "interpolation: WS003",
        "return: WS003",
        "emit: WS003",
        "out: WS003",
        "comparison operand: WS004",
        "operand from a let: WS004",
        "condition: WS003",
        "array literal element: WS003",
        "tuple element: WS003",
        "generic argument: WS033 WS003",
        "stored record field: WS003",
    ];
    assert_eq!(got, want);
}

/// The module `held_place_outcome` programs import as `u`.
const HELD_LIB: &str = "mod pair(x: int) -> (lo: int, hi: int) { return (x + 1, x * 2) }\n\
                        chip CC(x: int) -> (lo: int, hi: int) { out lo = x + 1\nout hi = x * 2 }\n";

/// What `src` saves, importing [`HELD_LIB`] as `u`, its `let` labels aside.
fn held_place_outcome(src: &str, fold: bool) -> super::shadow_differential::Outcome {
    let mut o = super::shadow_differential::outcome(src, &[("lib", HELD_LIB)], fold);
    for s in &mut o.shape {
        *s = s.split(' ').filter(|t| !t.is_empty() && !t.starts_with("_bindname=")).collect::<Vec<_>>().join(" ");
    }
    o.shape.sort();
    o
}

/// What `got` has that `want` lacks, and the reverse, of the shape and the
/// save.
fn outcome_diff(got: &super::shadow_differential::Outcome, want: &super::shadow_differential::Outcome) -> String {
    let only = |a: &[String], b: &[String]| a.iter().filter(|s| !b.contains(s)).cloned().collect::<Vec<_>>();
    format!(
        "{:?} / {:?}; {:?} / {:?}",
        only(&got.shape, &want.shape),
        only(&want.shape, &got.shape),
        only(&got.comps, &want.comps),
        only(&want.comps, &got.comps)
    )
}

/// A gate read gives a place for one value its first output however it gets
/// there: held in a `let`, a record field or a tuple, returned by every
/// `return` of a `mod` (an early one included), destructured by the names its
/// outputs are declared with, sent as event data or a signal's payload, or
/// chosen by an `if` or `Select`, from every producer (a builtin, a container
/// method, a `mod`, `chip`, generic or `self` mod call, flat or through
/// `import * as`). Each program saves what its twin naming the first output
/// saves, in both fold modes.
#[test]
fn a_gate_read_held_returned_or_destructured_gives_its_first_output() {
    let head = "import * as u from \"lib\"\nin go: exec\nin a: int\nin bb: bool\nvar n: int = 0\nout on1: int = n\n\
                var f: float = 0.0\nout of1: float = f\nvar bo: bool = false\nout obo: bool = bo\nvar s: string = \"\"\n\
                out os: string = s\nvar vv: vector = Vec(1.0, 2.0, 3.0)\nvar fs: float[] = [1.0, 2.0]\n\
                var ns: int[] = [1, 2]\nvar bs: bool[]\nvar ss: string[]\nvar mp: Map<string, int> = { \"k\": 1 }\n\
                var ms: Map<string, string> = { \"k\": \"v\" }\n\
                mod g(x: int) -> (p: int, q: int) { out p = x + 1\nout q = x + 2 }\n\
                chip C2(x: int) -> (lo: int, hi: int) { out lo = x + 1\nout hi = x * 2 }\n\
                mod two(self: int) -> (p: int, q: int) { return (self + 1, self + 2) }\n\
                mod gg<T: Scalar>(x: T) -> (lo: T, hi: T) { return (x, x + 1) }\n\
                mod rmx() -> (Value: float, IsEmpty: bool) { return fs.max() }\n\
                mod rmn() -> (Value: float, IsEmpty: bool) { return fs.min() }\n\
                mod rav() -> (Value: float, IsEmpty: bool) { return fs.average() }\n\
                mod rpop() -> (Value: float, IsEmpty: bool) { return fs.pop() }\n\
                mod rgt() -> (Value: int, Found: bool) { return mp.get(\"k\") }\n\
                mod rfd() -> (Index: int, Found: bool) { return ns.find(2) }\n\
                mod red() -> (Rising: bool, Falling: bool) { return Edge(bb) }\n\
                mod rsv() -> (x: float, y: float, z: float) { return SplitVec(vv) }\n\
                mod rrn() -> (v: float, e: bool) { return fs.max() }\n\
                mod rgn<T: Scalar>(xs: T[]) -> (Value: T, IsEmpty: bool) { return xs.min() }\n\
                mod rhl() -> (Value: float, IsEmpty: bool) { let k = fs.max()\nreturn k }\n\
                mod rer() -> (Value: float, IsEmpty: bool) { if bb { return fs.max() }\nreturn fs.min() }\n";
    // Each read, the output a place for one value reads, its type, and a
    // var and an array of that type. The `r*` reads are a `mod` returning a
    // builtin read whole: renamed (`rrn`), generic, held and on two paths.
    let reads = [
        ("fs.max()", "Value", "float", "f", "fs"),
        ("fs.pop()", "Value", "float", "f", "fs"),
        ("ns.find(2)", "Index", "int", "n", "ns"),
        ("mp.get(\"k\")", "Value", "int", "n", "ns"),
        ("ms.get(\"k\")", "Value", "string", "s", "ss"),
        ("Edge(bb)", "Rising", "bool", "bo", "bs"),
        ("SplitVec(vv)", "x", "float", "f", "fs"),
        ("g(a)", "p", "int", "n", "ns"),
        ("C2(a)", "lo", "int", "n", "ns"),
        ("a.two()", "p", "int", "n", "ns"),
        ("gg(a)", "lo", "int", "n", "ns"),
        ("u.pair(a)", "lo", "int", "n", "ns"),
        ("u.CC(a)", "lo", "int", "n", "ns"),
        ("rmx()", "Value", "float", "f", "fs"),
        ("rmn()", "Value", "float", "f", "fs"),
        ("rav()", "Value", "float", "f", "fs"),
        ("rpop()", "Value", "float", "f", "fs"),
        ("rgt()", "Value", "int", "n", "ns"),
        ("rfd()", "Index", "int", "n", "ns"),
        ("red()", "Rising", "bool", "bo", "bs"),
        ("rsv()", "x", "float", "f", "fs"),
        ("rrn()", "v", "float", "f", "fs"),
        ("rgn(fs)", "Value", "float", "f", "fs"),
        ("rhl()", "Value", "float", "f", "fs"),
        ("rer()", "Value", "float", "f", "fs"),
    ];
    // `{R}` the read, `{F}` its first output, `{TY}` that output's type,
    // `{X}` and `{XS}` a var and an array of it. With no twin given, the
    // twin is the place with `{R}` spelled `{R}.{F}`.
    let places: [(&str, &str, Option<&str>); 47] = [
        ("return of a let", "mod D() -> {TY} { let k = {R}\nreturn k }\non go { {X} = D() }", None),
        (
            "early return of a let",
            "mod D(z: {TY}) -> {TY} { let k = {R}\nif bb { return k }\nreturn z }\non go { {X} = D({X}) }",
            None,
        ),
        ("return of a let field", "mod D() -> {TY} { let t = { m: {R}, z: 1 }\nreturn t.m }\non go { {X} = D() }", None),
        ("return of a tuple element", "mod D() -> {TY} { let t = ({R}, 1)\nreturn t.0 }\non go { {X} = D() }", None),
        (
            "tuple returned to outputs",
            "mod D() -> (r: {TY}, q: int) { let k = {R}\nreturn (k, 2) }\non go { {X} = D().r }",
            None,
        ),
        (
            "tuples returned on two paths",
            "mod D() -> (r: {TY}, q: int) { let k = {R}\nif bb { return (k, 2) }\nreturn (k, 3) }\non go { {X} = D().r }",
            None,
        ),
        ("return of a let chain", "mod D() -> {TY} { let k = {R}\nlet j = k\nreturn j }\non go { {X} = D() }", None),
        (
            "return of a destructured field",
            "mod D() -> {TY} { let { m, z } = { m: {R}, z: 1 }\nreturn m }\non go { {X} = D() }",
            None,
        ),
        ("emit of a let", "mod D() -> (r: {TY}) { let k = {R}\nemit r = k }\non go { {X} = D() }", None),
        ("out of a let", "mod D() -> (r: {TY}) { let k = {R}\nout r = k }\non go { {X} = D() }", None),
        ("chip out of a let", "chip H() -> (r: {TY}) { let k = {R}\nout r = k }\non go { {X} = H() }", None),
        ("let chain", "on go { let k = {R}\nlet j = k\n{X} = j }", None),
        ("tuple destructure", "on go { let (v, ...w) = {R}\n{X} = v }", Some("on go { let v = {R}.{F}\n{X} = v }")),
        (
            "tuple destructure of a let",
            "on go { let k = {R}\nlet (v, ...w) = k\n{X} = v }",
            Some("on go { let k = {R}\nlet v = k.{F}\n{X} = v }"),
        ),
        ("record destructure", "on go { let { {F}: v } = {R}\n{X} = v }", Some("on go { let v = {R}.{F}\n{X} = v }")),
        (
            "record destructure of a let",
            "on go { let k = {R}\nlet { {F}: v } = k\n{X} = v }",
            Some("on go { let k = {R}\nlet v = k.{F}\n{X} = v }"),
        ),
        ("shorthand record field", "var rr: { m: {TY}, z: int }\non go { let m = {R}\nrr = { m, z: 1 }\n{X} = rr.m }", None),
        ("stored record field of a let", "var rr: { m: {TY}, z: int }\non go { let k = {R}\nrr = { m: k, z: 1 }\n{X} = rr.m }", None),
        ("signal payload", "on go { let sig: exec\nbuffer emit sig = {R}\nlet v = await sig\n{X} = v }", None),
        (
            "signal payload of a let",
            "on go { let k = {R}\nlet sig: exec\nbuffer emit sig = k\nlet v = await sig\n{X} = v }",
            None,
        ),
        ("event data", "on go { SendCustomEvent(\"ch\", {R}) }\non CustomEvent(\"ch\") -> (p: {TY}) { {X} = p }", None),
        (
            "event data of a let",
            "on go { let k = {R}\nSendCustomEvent(\"ch\", k) }\non CustomEvent(\"ch\") -> (p: {TY}) { {X} = p }",
            None,
        ),
        ("if join", "on go { {X} = if bb then {R} else {X} }", None),
        ("Select", "on go { {X} = Select(bb, {R}, {X}) }", None),
        ("if join of a let", "on go { let k = {R}\nlet j = if bb then k else {X}\n{X} = j }", None),
        ("if join of two reads", "on go { let t = if bb then {R} else {R}\n{X} = t }", None),
        ("if of two reads", "on go { {X} = if bb then {R} else {R} }", None),
        ("Select of two reads", "on go { {X} = Select(bb, {R}, {R}) }", None),
        ("annotated tuple", "on go { let t: ({TY}, int) = ({R}, 1)\n{X} = t.0 }", None),
        ("interpolation of a let", "on go { let k = {R}\ns = \"v${k}\" }", None),
        ("condition of a let", "on go { let k = {R}\nif k { n = 1 } }", None),
        ("array literal of a let", "on go { let k = {R}\nvar xs: {TY}[] = [k]\n{X} = xs[0] }", None),
        ("assignment", "on go { {X} = {R} }", None),
        ("push", "on go { {XS}.push({R})\n{X} = {XS}[0] }", None),
        ("argument", "mod D(v: {TY}) -> {TY} { return v }\non go { {X} = D({R}) }", None),
        ("return", "mod D() -> {TY} { return {R} }\non go { {X} = D() }", None),
        ("emit", "mod D() -> (r: {TY}) { emit r = {R} }\non go { {X} = D() }", None),
        ("out", "mod D() -> (r: {TY}) { out r = {R} }\non go { {X} = D() }", None),
        ("interpolation", "on go { s = \"v${{R}}\" }", None),
        ("comparison operand", "on go { bo = {R} == {R} }", None),
        ("condition", "on go { if {R} { n = 1 } }", None),
        ("untyped var", "on go { var t = {R}\n{X} = t }", None),
        (
            "record literal argument",
            "type P2 = { m: {TY}, z: int }\nmod D(p: P2) -> {TY} { return p.m }\non go { {X} = D({ m: {R}, z: 1 }) }",
            None,
        ),
        (
            "record literal pushed",
            "var ps: { m: {TY}, z: int }[]\non go { ps.push({ m: {R}, z: 1 })\n{X} = ps[0].m }",
            None,
        ),
        (
            "record literal returned",
            "type P2 = { m: {TY}, z: int }\nmod D() -> P2 { return { m: {R}, z: 1 } }\non go { {X} = D().m }",
            None,
        ),
        (
            "record literal emitted",
            "type P2 = { m: {TY}, z: int }\nmod D() -> (r: P2) { emit r = { m: {R}, z: 1 } }\non go { {X} = D().m }",
            None,
        ),
        (
            "record literal chip argument",
            "type P2 = { m: {TY}, z: int }\nchip K(p: P2) -> (r: {TY}) { out r = p.m }\non go { {X} = K({ m: {R}, z: 1 }) }",
            None,
        ),
    ];
    let spell = |place: &str, read: &str, first: &str, ty: &str, x: &str, xs: &str| {
        let body = place
            .replace("{R}", read)
            .replace("{F}", first)
            .replace("{TY}", ty)
            .replace("{XS}", xs)
            .replace("{X}", x);
        format!("{head}{body}")
    };
    let mut pairs = 0;
    let mut wrong = Vec::new();
    for (read, first, ty, x, xs) in reads {
        let named = format!("{read}.{first}");
        for (what, place, twin) in places {
            pairs += 1;
            let src = spell(place, read, first, ty, x, xs);
            let twin = match twin {
                Some(t) => spell(t, read, first, ty, x, xs),
                None => spell(place, &named, first, ty, x, xs),
            };
            for fold in [false, true] {
                let (got, want) = (held_place_outcome(&src, fold), held_place_outcome(&twin, fold));
                // A choice held whole between two reads that change their
                // container can't run only the arm taken (`WS097`).
                if matches!(what, "if join of two reads" | "if of two reads") && read.contains("pop") {
                    if got.compiled || !got.codes.iter().all(|c| c == "WS097") || !want.compiled {
                        wrong.push(format!("{read} {what}, fold {fold}: {:?} {:?}", got.codes, want.codes));
                    }
                    continue;
                }
                // A read held from a mod's stored outputs (`rer()`, two
                // returns) is read where it is used, its twin where it is
                // bound, and a choice of two reads the storage of each
                // output: beside reads of storage, the same gates, and no
                // exec wired into a value the twin does not wire.
                let stored = read == "rer()";
                let set = |v: &[String]| {
                    v.iter().filter(|c| !c.contains("Exec_Var_Get")).cloned().collect::<std::collections::BTreeSet<_>>()
                };
                let exec_into_values = |o: &super::shadow_differential::Outcome| {
                    o.shape
                        .iter()
                        .filter(|w| w.contains(".ExecOut -> ") && (w.ends_with(".Value") || w.contains(".Input")))
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>()
                };
                if !(got.compiled && want.compiled) {
                    wrong.push(format!("{read} {what}, fold {fold}: {:?} {:?}", got.codes, want.codes));
                } else if stored
                    && (got.codes != want.codes
                        || set(&got.comps) != set(&want.comps)
                        || !exec_into_values(&got).is_subset(&exec_into_values(&want)))
                {
                    wrong.push(format!("{read} {what}, fold {fold}: {}", outcome_diff(&got, &want)));
                } else if !stored && got != want {
                    let only = |a: &[String], b: &[String]| a.iter().filter(|s| !b.contains(s)).cloned().collect::<Vec<_>>();
                    wrong.push(format!(
                        "{read} {what}, fold {fold}: not its first output: {:?} / {:?}",
                        only(&got.shape, &want.shape),
                        only(&want.shape, &got.shape)
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(pairs, 25 * 47);
    // A record place of the outputs' names takes every output by its name,
    // inline or held.
    for (read, a, b, ty) in [("fs.max()", "Value", "IsEmpty", "float"), ("Edge(bb)", "Rising", "Falling", "bool")] {
        let decl = format!("var rv: {{ {a}: {ty}, {b}: bool }}\n");
        let twin = format!("{head}{decl}on go {{ let t = {read}\nrv = {{ {a}: t.{a}, {b}: t.{b} }}\nbo = rv.{b} }}");
        for body in [
            format!("{head}{decl}on go {{ rv = {read}\nbo = rv.{b} }}"),
            format!("{head}{decl}on go {{ let k = {read}\nrv = k\nbo = rv.{b} }}"),
        ] {
            for fold in [false, true] {
                let (got, want) = (held_place_outcome(&body, fold), held_place_outcome(&twin, fold));
                assert!(got.compiled && got == want, "{body}, fold {fold}: {:?} {}", got.codes, outcome_diff(&got, &want));
            }
        }
    }
    // A generic body reads each instantiation's own outputs: an untyped `var`
    // holds the first output of the type that instantiation reads, as a body
    // written for that type does.
    let generic = "mod mx<T: Scalar>(xs: T[]) -> T { var t = xs.max()\nreturn t }\non go { n = mx(ns)\nf = mx(fs) }";
    let written = "mod mi(xs: int[]) -> int { var t = xs.max()\nreturn t }\n\
                   mod mf(xs: float[]) -> float { var t = xs.max()\nreturn t }\non go { n = mi(ns)\nf = mf(fs) }";
    for fold in [false, true] {
        let got = held_place_outcome(&format!("{head}{generic}"), fold);
        let want = held_place_outcome(&format!("{head}{written}"), fold);
        assert!(got.compiled && got == want, "generic var, fold {fold}: {}", outcome_diff(&got, &want));
    }
    // A read held whole and returned to outputs named otherwise fills them by
    // position, as the read written inline does.
    for (read, a, b, ty, x) in [
        ("fs.min()", "Value", "IsEmpty", "float", "f"),
        ("rmx()", "Value", "IsEmpty", "float", "f"),
        ("rgn(fs)", "Value", "IsEmpty", "float", "f"),
        ("mp.get(\"k\")", "Value", "Found", "int", "n"),
    ] {
        let body = |ret: &str| {
            format!(
                "{head}mod h() -> (r: {ty}, q: bool) {{ let k = {read}\nreturn {ret} }}\n\
                 on go {{ let (v, e) = h()\n{x} = v\nbo = e }}"
            )
        };
        for fold in [false, true] {
            let got = held_place_outcome(&body("k"), fold);
            let want = held_place_outcome(&body(&format!("(k.{a}, k.{b})")), fold);
            assert!(got.compiled && got == want, "{read} held return, fold {fold}: {:?} {}", got.codes, outcome_diff(&got, &want));
        }
    }
}

/// A `mod` returning a gate read whole fills its outputs as the same `mod`
/// returning a tuple of the read's outputs, written out, does: by name when
/// its outputs are named like the read's (in any order), by position when
/// none is. The twin goes through no re-keying, so a swapped position or name
/// map reads the wrong output in some place below and fails. Every producer
/// (a builtin, a container method, renamed, reordered, generic, held, two
/// returns) at every place reading each output, both folds.
#[test]
fn a_mod_returning_a_read_whole_fills_its_outputs_as_its_tuple_return_twin() {
    let head = "in go: exec\nin bb: bool\nvar n: int = 0\nout on1: int = n\nvar n2: int = 0\nout on2: int = n2\n\
                var f: float = 0.0\nout of1: float = f\nvar f2: float = 0.0\nout of2: float = f2\n\
                var bo: bool = false\nout obo: bool = bo\nvar bo2: bool = false\nout obo2: bool = bo2\n\
                var s: string = \"\"\nout os: string = s\nvar vv: vector = Vec(1.0, 2.0, 3.0)\n\
                var fs: float[] = [1.0, 2.0]\nvar ns: int[] = [1, 2]\nvar bs: bool[]\nvar mp: Map<string, int> = { \"k\": 1 }\n";
    // The producer's call, its outputs (name, type, a var of that type) as
    // the mod declares them, the read it returns whole, and the read's own
    // outputs in the order the declared ones take them.
    type Output = (&'static str, &'static str, &'static str);
    let producers: [(&str, &str, [Output; 2], &str, [&str; 2]); 13] = [
        ("rmx()", "", [("Value", "float", "f"), ("IsEmpty", "bool", "bo")], "fs.max()", ["Value", "IsEmpty"]),
        ("rmn()", "", [("Value", "float", "f"), ("IsEmpty", "bool", "bo")], "fs.min()", ["Value", "IsEmpty"]),
        ("rav()", "", [("Value", "float", "f"), ("IsEmpty", "bool", "bo")], "fs.average()", ["Value", "IsEmpty"]),
        ("rpop()", "", [("Value", "float", "f"), ("IsEmpty", "bool", "bo")], "fs.pop()", ["Value", "IsEmpty"]),
        ("rgt()", "", [("Value", "int", "n"), ("Found", "bool", "bo")], "mp.get(\"k\")", ["Value", "Found"]),
        ("rfd()", "", [("Index", "int", "n"), ("Found", "bool", "bo")], "ns.find(2)", ["Index", "Found"]),
        ("red()", "", [("Rising", "bool", "bo"), ("Falling", "bool", "bo2")], "Edge(bb)", ["Rising", "Falling"]),
        ("rrn()", "", [("v", "float", "f"), ("e", "bool", "bo")], "fs.max()", ["Value", "IsEmpty"]),
        ("rro()", "", [("IsEmpty", "bool", "bo"), ("Value", "float", "f")], "fs.min()", ["IsEmpty", "Value"]),
        ("rfo()", "", [("Found", "bool", "bo"), ("Index", "int", "n")], "ns.find(1)", ["Found", "Index"]),
        ("rgn(fs)", "<T: Scalar>", [("Value", "T", "f"), ("IsEmpty", "bool", "bo")], "xs.min()", ["Value", "IsEmpty"]),
        ("rhl()", "held", [("Value", "float", "f"), ("IsEmpty", "bool", "bo")], "fs.max()", ["Value", "IsEmpty"]),
        ("rer()", "two", [("Value", "float", "f"), ("IsEmpty", "bool", "bo")], "fs.max()", ["Value", "IsEmpty"]),
    ];
    // `{M}` the call, `{A}`/`{B}` its outputs, `{TA}`/`{TB}` their types and
    // `{XA}`/`{XB}` vars of them.
    let places = [
        "on go { {XA} = {M} }",
        "on go { {XA} = {M}.{A} }",
        "on go { {XB} = {M}.{B} }",
        "on go { let k = {M}\n{XA} = k.{A}\n{XB} = k.{B} }",
        "on go { let (v, w) = {M}\n{XA} = v\n{XB} = w }",
        "on go { let { {A}: v, {B}: w } = {M}\n{XA} = v\n{XB} = w }",
        "on go { let { {B}: w } = {M}\n{XB} = w }",
        "on go { {XA} = if bb then {M} else {M} }",
        "on go { let k = if bb then {M} else {M}\n{XB} = k.{B} }",
        "mod D(x: {TA}) -> {TA} { return x }\non go { {XA} = D({M}) }",
        "mod W() -> ({A}: {TA}, {B}: {TB}) { return {M} }\non go { {XB} = W().{B} }",
        "mod W() -> (p: {TA}, q: {TB}) { let k = {M}\nreturn k }\non go { let (v, w) = W()\n{XA} = v\n{XB} = w }",
        "on go { s = \"${{M}.{B}}\" }",
        "on go { SendCustomEvent(\"ch\", {M}) }\non CustomEvent(\"ch\") -> (p: {TA}) { {XA} = p }",
        "var rr: { m: {TB}, z: int }\non go { let k = {M}\nrr = { m: k.{B}, z: 1 }\n{XB} = rr.m }",
    ];
    let mut wrong = Vec::new();
    let mut rows = 0;
    for (call, kind, [(a, ta, xa), (b, tb, xb)], read, [ra, rb]) in producers {
        let name = call.split(['(', '<']).next().unwrap_or(call);
        let (tparams, params) = if kind.starts_with('<') { (kind, "xs: T[]") } else { ("", "") };
        let sig = format!("mod {name}{tparams}({params}) -> ({a}: {ta}, {b}: {tb})");
        let whole = match kind {
            "held" => format!("{sig} {{ let k = {read}\nreturn k }}\n"),
            "two" => format!("{sig} {{ if bb {{ return {read} }}\nreturn fs.min() }}\n"),
            _ => format!("{sig} {{ return {read} }}\n"),
        };
        let tuple = match kind {
            "two" => format!(
                "{sig} {{ if bb {{ let k9 = {read}\nreturn (k9.{ra}, k9.{rb}) }}\nlet k8 = fs.min()\nreturn (k8.{ra}, k8.{rb}) }}\n"
            ),
            _ => format!("{sig} {{ let k9 = {read}\nreturn (k9.{ra}, k9.{rb}) }}\n"),
        };
        // A generic producer's outputs are typed by its instantiation here.
        let (ta, tb) = (ta.replace('T', "float"), tb.replace('T', "float"));
        for place in places {
            rows += 1;
            let body = place
                .replace("{M}", call)
                .replace("{A}", a)
                .replace("{B}", b)
                .replace("{TA}", &ta)
                .replace("{TB}", &tb)
                .replace("{XA}", xa)
                .replace("{XB}", xb);
            for fold in [false, true] {
                let got = held_place_outcome(&format!("{head}{whole}{body}"), fold);
                let want = held_place_outcome(&format!("{head}{tuple}{body}"), fold);
                // A choice between two reads that change their container
                // can't run only the arm taken: `WS097` in both.
                if !want.compiled {
                    if got.codes != want.codes || !want.codes.iter().all(|c| c == "WS097") {
                        wrong.push(format!("{call} twin `{body}`, fold {fold}: {:?} {:?}", got.codes, want.codes));
                    }
                } else if got != want {
                    wrong.push(format!("{call} `{body}`, fold {fold}: {:?} {}", got.codes, outcome_diff(&got, &want)));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(rows, 13 * 15);
}

/// A choice (`if`, `match`, `Select`) between two multi-output reads given as
/// an argument is one value, made by the join every other place for one value
/// uses: output by output when the reads declare the same outputs in the same
/// order, otherwise their first outputs. Every argument kind (a `mod`, a
/// generic, a `chip`, a builtin, a `self` mod, a namespaced `mod`) x every
/// choice kind x reads with the same, reordered and different outputs saves
/// what its twin naming each branch's output saves, both folds. The exec of
/// a branch's call was the value given before.
#[test]
fn a_choice_between_two_reads_given_as_an_argument_is_their_joined_value() {
    const LIB: &str = "mod pk(x: float) -> float { return x }\nmod ik(x: int) -> int { return x }\n";
    let head = "import * as u from \"lib\"\nin go: exec\nin bb: bool\nin a: int\nvar n: int = 0\nout on1: int = n\n\
                var f: float = 0.0\nout of1: float = f\nvar fs: float[] = [1.5, 2.5]\nvar ns: int[] = [1, 2]\n\
                mod A(x: int) -> (lo: int, hi: int) { return (x + 1, x * 2) }\n\
                mod B(x: int) -> (hi: int, lo: int) { return (x + 3, x * 4) }\n\
                mod Cf(x: int) -> (lo: float, hi: float) { return (x + 0.5, x * 1.5) }\n\
                mod pk(x: float) -> float { return x }\nmod ik(x: int) -> int { return x }\n\
                mod idt<T>(x: T) -> T { return x }\nchip PK(x: float) -> (o: float) { out o = x }\n\
                mod sq(self: float) -> float { return self * 2.0 }\nenum Hb { T, F }\nvar hb: Hb = Hb.T\n";
    // The two reads, the output each gives where one value goes, and the
    // var of the joined type.
    let reads = [
        ("A(a)", "lo", "Cf(a)", "lo", "f"),
        ("fs.max()", "Value", "ns.max()", "Value", "f"),
        ("fs.max()", "Value", "fs.min()", "Value", "f"),
        ("A(a)", "lo", "A(5)", "lo", "n"),
        ("A(a)", "lo", "B(a)", "hi", "n"),
        ("A(a)", "lo", "fs.max()", "Value", "f"),
        ("ns.find(2)", "Index", "A(a)", "lo", "n"),
    ];
    let choices = ["if bb then @L else @R", "Select(bb, @L, @R)", "match hb { Hb.T => @L, Hb.F => @R }"];
    // `{C}` the choice, `{X}` the var.
    let args = [
        ("mod", "on go { {X} = pk({C}) }"),
        ("int mod", "on go { n = ik({C}) }"),
        ("generic", "on go { {X} = idt({C}) }"),
        ("chip", "on go { f = PK({C}) }"),
        ("builtin", "on go { {X} = abs({C}) }"),
        ("self mod", "on go { f = sq({C}) }"),
        ("namespaced", "on go { f = u.pk({C}) }"),
        ("namespaced int", "on go { n = u.ik({C}) }"),
        ("held, mod", "on go { let j = {C}\n{X} = pk(j) }"),
        ("held, generic", "on go { let j = {C}\n{X} = idt(j) }"),
    ];
    let outcome = |src: &str, fold: bool| {
        let mut o = super::shadow_differential::outcome(src, &[("lib", LIB)], fold);
        for s in &mut o.shape {
            *s = s.split(' ').filter(|t| !t.is_empty() && !t.starts_with("_bindname=")).collect::<Vec<_>>().join(" ");
        }
        o.shape.sort();
        o
    };
    let mut wrong = Vec::new();
    let mut rows = 0;
    for (l, lf, r, rf, x) in reads {
        for choice in choices {
            for (kind, place) in args {
                // A held `match` of reads is a record `match`, which tests
                // its last arm by elimination where the scalar twin builds
                // the default arm too: the same values, other gates.
                if kind.starts_with("held") && choice.starts_with("match") {
                    continue;
                }
                rows += 1;
                let spell = |l: &str, r: &str| {
                    let c = choice.replace("@L", l).replace("@R", r);
                    format!("{head}{}", place.replace("{C}", &c).replace("{X}", x))
                };
                let src = spell(l, r);
                let twin = spell(&format!("{l}.{lf}"), &format!("{r}.{rf}"));
                for fold in [false, true] {
                    let (got, want) = (outcome(&src, fold), outcome(&twin, fold));
                    if !want.compiled || got != want {
                        wrong.push(format!(
                            "{kind} `{choice}` of {l} and {r}, fold {fold}: {:?} {:?} {}",
                            got.codes,
                            want.codes,
                            outcome_diff(&got, &want)
                        ));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(rows, 7 * 3 * 8 + 7 * 2 * 2);
}

/// The wiring invariant (`check_no_exec_into_value`): an exec source wired
/// into an input that takes a value is a coded internal error naming both
/// ends and the line, and a rerouter pin of no type may carry an exec.
#[test]
fn an_exec_wired_into_a_value_is_an_internal_error() {
    use crate::ir::build::{AddNodeOpts, ModuleBuilder};
    let at = |line: u32| {
        let pos = crate::diagnostic::Pos { offset: 0, line, col: 1 };
        crate::diagnostic::SourceRange::new("main.ws", pos.clone(), pos)
    };
    let build = |into: Type, port: WirePort| {
        let mut b = ModuleBuilder::new("m");
        let go = b.add_input("go", Type::Exec, at(1));
        let set = b.add_gate(AddNodeOpts {
            gate_class: gc::VAR_SET,
            source_range: at(4),
            ports: GateIO {
                inputs: vec![PortSpec::exec(*sym::EXEC), PortSpec::new(*sym::VALUE, into.clone())],
                outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
            },
            ..Default::default()
        });
        let out = b.add_output("o", into, at(5));
        b.connect(go.port(WirePort::RerOutput), set.port(WirePort::Exec));
        match port {
            WirePort::Value => b.connect(go.port(WirePort::RerOutput), set.port(WirePort::Value)),
            _ => b.connect(go.port(WirePort::RerOutput), out.port(WirePort::RerInput)),
        }
        b.module
    };
    let check = |m: &crate::ir::Module| {
        let mut diags = Vec::new();
        crate::lower::check_no_exec_into_value(m, &at(0), &mut diags);
        diags
    };
    let diags = check(&build(Type::Float, WirePort::Value));
    assert_eq!(diags.len(), 1, "{diags:?}");
    let d = &diags[0];
    assert_eq!(d.code, "WS094");
    assert!(d.message.contains(crate::diagnostic::EXEC_INTO_VALUE), "{}", d.message);
    assert!(d.message.contains(".RER_Output") && d.message.contains(".Value") && d.message.contains("line 4"), "{}", d.message);
    assert_eq!(d.range.start.line, 4);
    // A typed output pin takes a value; one of no type passes the exec on.
    assert_eq!(check(&build(Type::Int, WirePort::RerInput)).len(), 1);
    assert!(check(&build(Type::Any, WirePort::RerInput)).is_empty());
    assert!(check(&build(Type::Exec, WirePort::RerInput)).is_empty());
    // A `bool` input reads an exec as true for the tick it fires.
    assert!(check(&build(Type::Bool, WirePort::Value)).is_empty());
    assert!(check(&build(Type::Bool, WirePort::RerInput)).is_empty());
    // Every violation is reported, and an exec passed on by a pin of no type
    // is followed to the input it ends at.
    let mut b = ModuleBuilder::new("m");
    let go = b.add_input("go", Type::Exec, at(1));
    let gate = |b: &mut ModuleBuilder, ty: Type, line: u32| {
        b.add_gate(AddNodeOpts {
            gate_class: gc::VAR_SET,
            source_range: at(line),
            ports: GateIO { inputs: vec![PortSpec::new(*sym::VALUE, ty)], outputs: vec![] },
            ..Default::default()
        })
    };
    let first = gate(&mut b, Type::Float, 4);
    b.connect(go.port(WirePort::RerOutput), first.port(WirePort::Value));
    let second = gate(&mut b, Type::String, 6);
    b.connect(go.port(WirePort::RerOutput), second.port(WirePort::Value));
    let pin = b.add_output("p", Type::Any, at(7));
    b.connect(go.port(WirePort::RerOutput), pin.port(WirePort::RerInput));
    let read = gate(&mut b, Type::Int, 8);
    b.connect(pin.port(WirePort::RerOutput), read.port(WirePort::Value));
    let held = gate(&mut b, Type::Bool, 9);
    b.connect(pin.port(WirePort::RerOutput), held.port(WirePort::Value));
    let diags = check(&b.module);
    let lines: Vec<u32> = diags.iter().map(|d| d.range.start.line).collect();
    assert_eq!(lines, [4, 6, 8], "{diags:?}");
    assert!(diags.iter().all(|d| d.code == "WS094"), "{diags:?}");
    assert!(diags[2].message.contains("pin of no type") && !diags[2].message.contains(crate::diagnostic::EXEC_INTO_VALUE), "{}", diags[2].message);
    // A logical gate reads an exec as true for the tick it fires.
    let r = compile("in reset: bool\nin start: exec\nvar n: int = 0\nout o: int = n\non reset || start { n = n + 1 }");
    assert_no_errors(&r);
    assert!(r.module.nodes.values().any(|n| n.gate_class == "BrickComponentType_Expr_LogicalOR"));
}

/// An exec read where a `bool` goes is true for the tick it fires: wherever it
/// reaches a `bool` input (a branch or select condition, a logical operator),
/// directly, through a `let` in a handler or at the top level, or through an
/// `if` join, the program compiles in both fold modes with no `WS094` (which a
/// debug build panics on), and an untyped custom-event data pin keeps its
/// `WS045`. An exec passed on by a pin of no type is followed to the input it
/// ends at: a `bool` takes it, a number, a string or a color is `WS094`.
#[test]
fn an_exec_where_a_bool_goes_compiles_and_where_a_value_goes_is_ws094() {
    use super::declaring_scope::compile_main;
    let sources = [("", "a"), ("let q = a\n ", "q"), ("", "Q"), ("", "(if c then a else b)"), ("", "!a"), ("", "(a || c)")];
    let places = ["if @ { n = 1 }", "n = if @ then 1 else 2", "c = !(@)", "c = @ && c", "SendCustomEvent(\"e\", @)"];
    let mut wrong = Vec::new();
    let mut rows = 0;
    for (pre, e) in sources {
        for place in places {
            let src = format!(
                "in go: exec\nin a: exec\nin b: exec\nvar c: bool = false\nvar n: int = 0\nlet Q = a\non go {{ {pre}{} }}",
                place.replace('@', e)
            );
            rows += 1;
            for fold in [true, false] {
                match compile_main(&src, &[], fold) {
                    Ok(out) => {
                        let codes: Vec<&str> = out.diagnostics.iter().map(|d| &*d.code).collect();
                        let untyped_data = place.starts_with("Send") && !e.contains(['!', '|']);
                        if codes.contains(&"WS045") != untyped_data {
                            wrong.push(format!("fold {fold}: {codes:?}\n{src}"));
                        }
                    }
                    Err(codes) => wrong.push(format!("fold {fold}: {codes:?}\n{src}")),
                }
            }
        }
    }
    let chip = "in go: exec\nvar n: int = 0\nvar f: float = 0.0\nvar s: string = \"\"\nvar b: bool = false\n\
                var col: color = Color(0.0, 0.0, 0.0)\nchip C() { out o: any\n on go { emit o } }\nlet k = C()\n";
    for (place, ws094) in [
        ("if k.o { n = 1 }", false),
        ("b = k.o", false),
        ("f = k.o", true),
        ("n = k.o", true),
        ("s = \"${k.o}\"", true),
        ("col = k.o", true),
    ] {
        let src = format!("{chip}on go {{ {place} }}");
        rows += 1;
        for fold in [true, false] {
            let codes = match compile_main(&src, &[], fold) {
                Ok(out) => out.diagnostics.iter().map(|d| d.code.to_string()).collect(),
                Err(codes) => codes,
            };
            if codes.iter().any(|c| c == "WS094") != ws094 || (!ws094 && codes.iter().any(|c| c.starts_with("WS0") && c != "WS032")) {
                wrong.push(format!("fold {fold}: {codes:?}\n{src}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {rows} rows:\n{}", wrong.len(), wrong.join("\n\n"));
}

/// Every wire of every grid of a save, as text, sorted: the save's wiring.
fn save_wires(brz: &[u8]) -> Vec<String> {
    use brdb::IntoReader;
    let brz = brdb::Brz::read_slice(brz).expect("read brz");
    let folders = &brz.index_data.folder_names;
    let grids: Vec<usize> = folders
        .iter()
        .zip(&brz.index_data.folder_parent_ids)
        .filter(|(_, parent)| usize::try_from(**parent).is_ok_and(|p| folders.get(p).is_some_and(|n| n == "Grids")))
        .filter_map(|(name, _)| name.parse().ok())
        .collect();
    let reader = brz.into_reader();
    let mut out = Vec::new();
    for gid in grids {
        for chunk in reader.brick_chunk_index(gid).expect("chunk index") {
            if chunk.num_wires > 0 {
                out.push(reader.wire_chunk_soa(gid, chunk.index).expect("wires").to_string());
            }
        }
    }
    out.sort();
    out
}

/// An exec where a `bool` goes drives the `bool` pin itself, in the graph
/// and in the save, both folds: the exec input `a` (line 1) is wired straight
/// into the condition, the select or the event data, and the save holds the
/// gates and wires the same program with `in a: bool` holds (the event data,
/// untyped, is `WS045`). An exec carried through an untyped pin into the
/// value of a `bool` array write compiles as into a `bool` map's.
#[test]
fn an_exec_in_a_bool_place_drives_the_bool_pin_in_the_save() {
    use super::declaring_scope::compile_main;
    let head = "in a: exec\nin b: exec\nin go: exec\nvar c: bool = false\nvar n: int = 0\nvar s: string = \"\"\n";
    let rows = [
        ("on go { if a { n = 1 } }", "BrickComponentType_Exec_Branch", true),
        ("on go { SendCustomEvent(\"e\", a) }", "BrickComponentType_Exec_SendCustomEvent", false),
        ("on go { n = if a then 1 else 2 }", "BrickComponentType_Expr_Select", true),
        ("on go { let q = a\n if q { n = 1 } }", "BrickComponentType_Exec_Branch", true),
        ("let q = a\non go { if q { n = 1 } }", "BrickComponentType_Exec_Branch", true),
    ];
    let mut wrong = Vec::new();
    for (body, reader, twin_bool) in rows {
        let src = format!("{head}{body}");
        for fold in [true, false] {
            let r = if fold { compile_folded(&src) } else { compile(&src) };
            let input = r.module.nodes.values().find(|n| {
                n.gate_class == crate::ir::gate_class::MICROCHIP_INPUT && n.source_range.start.line == 1
            });
            let driven = input.is_some_and(|i| {
                r.module.wires.iter().any(|w| {
                    w.source.node_id == i.id
                        && r.module.nodes.get(&w.target.node_id).is_some_and(|t| {
                            t.gate_class == reader
                                && t.ports.find_input(w.target.port.sym()).is_some_and(|p| p.ty != Type::Exec)
                        })
                })
            });
            if !driven {
                wrong.push(format!("fold {fold}: `a` drives no {reader} value pin\n{src}"));
            }
            match compile_main(&src, &[], fold) {
                Ok(out) => {
                    let untyped = out.diagnostics.iter().any(|d| d.code == "WS045");
                    if untyped == twin_bool {
                        wrong.push(format!("fold {fold}: WS045 {untyped}\n{src}"));
                    }
                    if twin_bool {
                        let twin = src.replacen("in a: exec", "in a: bool", 1);
                        match compile_main(&twin, &[], fold) {
                            Ok(t) => {
                                let same = super::shadow_differential::save_components(&out.brz)
                                    == super::shadow_differential::save_components(&t.brz)
                                    && save_wires(&out.brz) == save_wires(&t.brz);
                                if !same {
                                    wrong.push(format!("fold {fold}: the save differs from `in a: bool`\n{src}"));
                                }
                            }
                            Err(codes) => wrong.push(format!("fold {fold}: twin {codes:?}\n{twin}")),
                        }
                    }
                }
                Err(codes) => wrong.push(format!("fold {fold}: {codes:?}\n{src}")),
            }
        }
    }
    let chip = "in go: exec\nvar ba: bool[] = []\nvar ia: int[] = []\nvar mp: Map<int, bool> = {}\nvar fa: float[] = []\n\
                chip C() { out o: any\n on go { emit o } }\nlet k = C()\n";
    for (place, ws094) in [
        ("ba.push(k.o)", false),
        ("mp.set(1, k.o)", false),
        ("ba.insert(0, k.o)", false),
        ("ia.push(k.o)", true),
        ("fa.push(k.o)", true),
    ] {
        let src = format!("{chip}on go {{ {place} }}");
        for fold in [true, false] {
            let codes = match compile_main(&src, &[], fold) {
                Ok(out) => out.diagnostics.iter().map(|d| d.code.to_string()).collect(),
                Err(codes) => codes,
            };
            if codes.iter().any(|c| c == "WS094") != ws094 {
                wrong.push(format!("fold {fold}: {codes:?}\n{src}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

/// Every example and test fixture program holds the wiring invariant: in a
/// debug build a violation in a program reporting no other error panics.
#[test]
fn every_example_and_fixture_holds_the_wiring_invariant() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, files);
            } else if p.extension().is_some_and(|x| x == "ws") {
                files.push(p);
            }
        }
    }
    for dir in ["tests/fixtures", "../../examples", "../wasm/playground"] {
        walk(&root.join(dir), &mut files);
    }
    assert!(files.len() >= 50, "{}", files.len());
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let file = path.to_string_lossy();
        let diags = crate::compile::diagnostics_only(crate::CompileInput {
            source: &source,
            file: &file,
            module_name: None,
            fold_mode: crate::FoldMode::Auto,
        });
        assert!(
            !diags.iter().any(|d| d.message.contains(crate::diagnostic::EXEC_INTO_VALUE)),
            "{file}: {diags:?}"
        );
    }
}

/// A `var` or `let` written with no type, holding a record or tuple literal
/// with a gate read in a field or element, holds it as the read's first
/// output, as the one annotated with the type does: copied whole, given as an
/// argument, compared, its field read and written, returned, pushed, nested,
/// in an array. The field held the whole read before: a whole copy dropped
/// it, an argument read the exec, and `t = (2.5, 3)` was `WS003`.
#[test]
fn an_unannotated_var_holds_a_read_in_a_field_as_its_first_output() {
    let head = "in go: exec\nin bb: bool\nvar n: int = 0\nout on1: int = n\nvar f: float = 0.0\nout of1: float = f\n\
                var bo: bool = false\nout obo: bool = bo\nvar fs: float[] = [1.5, 2.5]\ntype M = { m: float, k: int }\n\
                type N = { r: M, z: int }\nvar mm: M = { m: 0.0, k: 0 }\nvar ms: M[]\n\
                mod gm(x: M) -> float { return x.m }\nmod mk() -> (lo: float, hi: float) { return (1.5, 2.5) }\n";
    // `@T`, `@U` and `@N` are where the twin writes `: M`, `: (float, int)`
    // and `: N`; `@V` is `var` or `let`.
    let rows = [
        "on go { @V t@T = { m: fs.max(), k: 7 }\nmm = t\nf = mm.m }",
        "on go { @V t@T = { m: fs.max(), k: 7 }\nf = gm(t) }",
        "on go { @V t@T = { m: fs.max(), k: 7 }\nf = t.m\nn = t.k }",
        "on go { @V t@T = { m: fs.max(), k: 7 }\nbo = t.m == 2.5 }",
        "on go { @V t@T = { m: fs.max(), k: 7 }\ns2 = \"${t.m}\" }\nvar s2: string = \"\"",
        "mod h() -> M { @V t@T = { m: fs.max(), k: 7 }\nreturn t }\non go { f = h().m }",
        "on go { @V t@T = { m: fs.max(), k: 7 }\nms.push(t)\nf = ms[0].m }",
        "on go { @V t@T = { m: mk(), k: 7 }\nf = t.m }",
        "on go { let r = fs.min()\n@V t@T = { m: r, k: 7 }\nf = t.m }",
        "on go { @V t@T = { k: 7, m: fs.max() }\nmm = t\nf = mm.m }",
        "on go { @V t@U = (fs.max(), 7)\nf = t.0\nn = t.1 }",
        "on go { @V t@U = (fs.max(), 7)\nlet (a, b) = t\nf = a\nn = b }",
        "on go { @V t@N = { r: { m: fs.max(), k: 1 }, z: 2 }\nf = t.r.m }",
        "on go { @V t@N = { r: { m: fs.max(), k: 1 }, z: 2 }\nmm = t.r\nf = mm.m }",
    ];
    // Writes, a `var`'s alone.
    let var_rows = [
        "on go { var t@T = { m: fs.max(), k: 7 }\nt.m = 4.5\nf = t.m }",
        "on go { var t@T = { m: fs.max(), k: 7 }\nt = { m: 2.5, k: 3 }\nf = t.m }",
        "on go { var t@U = (fs.max(), 7)\nt = (2.5, 3)\nf = t.0 }",
        "on go { var t@U = (fs.max(), 7)\nvar u@U = t\nf = u.0 }",
        "on go { var t@T = { m: fs.max(), k: 7 }\nt = { m: fs.min(), k: 1 }\nf = t.m }",
    ];
    let spell = |row: &str, kw: &str, typed: bool| {
        let (t, u, nn) = if typed { (": M", ": (float, int)", ": N") } else { ("", "", "") };
        format!("{head}{}", row.replace("@V", kw).replace("@T", t).replace("@U", u).replace("@N", nn))
    };
    let mut wrong = Vec::new();
    let mut count = 0;
    let all = rows.iter().flat_map(|r| [(*r, "var"), (*r, "let")]).chain(var_rows.iter().map(|r| (*r, "var")));
    for (row, kw) in all {
        count += 1;
        for fold in [false, true] {
            let got = held_place_outcome(&spell(row, kw, false), fold);
            let want = held_place_outcome(&spell(row, kw, true), fold);
            if !want.compiled || got != want {
                wrong.push(format!("{kw} `{row}`, fold {fold}: {:?} {:?} {}", got.codes, want.codes, outcome_diff(&got, &want)));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(count, 14 * 2 + 5);
}

/// A gate read returned whole to several outputs fills them by name when any
/// output is named like one of the read's outputs, and then every output must
/// be; by position when none is; the counts must agree; and two outputs of one
/// name are `WS013`. A partial match routed by position before, so an output
/// named `Value` could carry `IsEmpty`.
#[test]
fn a_read_returned_whole_routes_its_outputs_by_name_or_by_position() {
    let head = "in go: exec\nvar f: float = 0.0\nout of1: float = f\nvar bo: bool = false\nout obo: bool = bo\n\
                var n: int = 0\nout on1: int = n\nvar fs: float[] = [1.5, 2.5]\n";
    let compile_codes = |src: &str| {
        let r = compile(&format!("{head}{src}"));
        r.diagnostics.iter().filter(|d| d.severity == crate::diagnostic::Severity::Error).map(|d| (d.code.clone(), d.message.clone())).collect::<Vec<_>>()
    };
    // Rejected, with the output the message must name.
    for (decl, names) in [
        ("(Found: bool, Value: float)", "`Found`"),
        ("(Value: float, Found: bool)", "`Found`"),
        ("(Value: float, IsEmpty: bool, z: int)", "`z`"),
        ("(lo: float, IsEmpty: bool)", "`lo`"),
    ] {
        let src = format!("mod pm() -> {decl} {{ return fs.min() }}\non go {{ let o = pm()\nf = o.Value }}");
        let errs = compile_codes(&src);
        assert!(
            errs.iter().any(|(c, m)| c == "WS003" && m.contains("named like one") && m.contains(names)),
            "{decl}: {errs:?}"
        );
        // Held first, the same.
        let held = format!("mod pm() -> {decl} {{ let k = fs.min()\nreturn k }}\non go {{ let o = pm()\nf = o.Value }}");
        assert!(compile_codes(&held).iter().any(|(c, _)| c == "WS003"), "{decl} held: {:?}", compile_codes(&held));
    }
    // By name in any order, by position with no name shared: each saves what
    // the mod returning the outputs it names saves.
    for (decl, twin) in [
        ("(IsEmpty: bool, Value: float)", "(k.IsEmpty, k.Value)"),
        ("(Value: float, IsEmpty: bool)", "(k.Value, k.IsEmpty)"),
        ("(lo: float, em: bool)", "(k.Value, k.IsEmpty)"),
        ("(em: float, lo: bool)", "(k.Value, k.IsEmpty)"),
    ] {
        let (a, b) = {
            let inner = decl.trim_matches(['(', ')']);
            let mut it = inner.split(", ").map(|p| p.split(':').next().unwrap().trim().to_string());
            (it.next().unwrap(), it.next().unwrap())
        };
        let read = format!("on go {{ let o = pm()\nf = o.{a}\nbo = o.{b} }}");
        let whole = format!("{head}mod pm() -> {decl} {{ return fs.min() }}\n{read}");
        let tuple = format!("{head}mod pm() -> {decl} {{ let k = fs.min()\nreturn {twin} }}\n{read}");
        for fold in [false, true] {
            let (got, want) = (held_place_outcome(&whole, fold), held_place_outcome(&tuple, fold));
            assert!(got.compiled && got == want, "{decl}, fold {fold}: {:?} {}", got.codes, outcome_diff(&got, &want));
        }
    }
    // Two outputs of one name.
    for decl in ["mod pm() -> (lo: float, lo: bool) { return fs.min() }", "chip pc() -> (lo: float, lo: bool) { out lo = 1.0 }"] {
        let errs = compile_codes(&format!("{decl}\non go {{ n = 1 }}"));
        assert!(errs.iter().any(|(c, m)| c == "WS013" && m.contains("declared twice")), "{decl}: {errs:?}");
    }
}

/// A named output of a call with several outputs read where its value goes
/// (`go2(a).o`, an enum) saves what the call read whole saves, chosen in an
/// `if` too; it was a false `WS072`.
#[test]
fn a_named_enum_output_of_a_call_reads_as_the_call_does() {
    let head = "in go: exec\nin c: bool\nin a: int\nvar n: int = 0\nout on1: int = n\nenum Opt { Some(int), None }\n\
                var oo: Opt = Opt.None\nmod go2(x: int) -> (o: Opt, q: int) { if x > 1 { return (Opt.Some(x), x) }\n\
                return (Opt.None, x) }\n";
    let tail = "\nmatch oo { Opt.Some(v) => { n = v } Opt.None => { n = 0 } } }";
    for (src, twin) in [
        ("on go { oo = go2(a).o", "on go { oo = go2(a)"),
        ("on go { oo = if c then go2(a).o else Opt.None", "on go { oo = if c then go2(a) else Opt.None"),
        ("on go { let k = go2(a).o\noo = k", "on go { let k = go2(a)\noo = k"),
    ] {
        for fold in [false, true] {
            let got = held_place_outcome(&format!("{head}{src}{tail}"), fold);
            let want = held_place_outcome(&format!("{head}{twin}{tail}"), fold);
            assert!(got.compiled && got == want, "{src}, fold {fold}: {:?} {}", got.codes, outcome_diff(&got, &want));
        }
    }
}

/// A gate read whose first output is an enum is that enum where one value
/// goes: assigned or returned held in a `let`, a `match`, `if let` or `is`
/// scrutinee, `EnumToInt`'s argument, an element pushed, a branch of an `if`,
/// `match` or `Select` beside an enum, an enum parameter. Each saves what its
/// twin naming the output saves; `==` is `WS004` in both, enums having no
/// `==`. A held one assigned was an internal compiler error (`WS094`), and the
/// rest were `WS066`, `WS003` or placeholders.
#[test]
fn a_gate_read_with_an_enum_first_output_is_that_enum() {
    let head = "in go: exec\nin a: int\nin bb: bool\nenum Hue { Red, Green, Blue }\nvar hh: Hue = Hue.Red\n\
                var hs: Hue[] = [Hue.Green]\nvar n: int = 0\nout on1: int = n\nvar bo: bool = false\nout obo: bool = bo\n\
                mod gh(x: int) -> (h: Hue, q: int) { if x > 0 { return (Hue.Blue, 1) }\nreturn (Hue.Green, 2) }\n";
    let places = [
        "on go { hh = @ }",
        "on go { let k = @\nhh = k }",
        "on go { match @ { Hue.Blue => { n = 1 } _ => { n = 2 } } }",
        "on go { let k = @\nmatch k { Hue.Blue => { n = 1 } _ => { n = 2 } } }",
        "on go { n = match @ { Hue.Blue => 1, _ => 2 } }",
        "on go { if let Hue.Blue = @ { n = 1 } }",
        "on go { n = EnumToInt(@) }",
        "on go { let k = @\nn = EnumToInt(k) }",
        "on go { bo = @ is Hue.Blue }",
        "on go { hs.push(@) }",
        "mod D() -> Hue { let k = @\nreturn k }\non go { hh = D() }",
        "mod D() -> Hue { let k = @\nif bb { return k }\nreturn Hue.Red }\non go { hh = D() }",
        "var rr: { h: Hue, k: int } = { h: Hue.Red, k: 0 }\non go { rr = { h: @, k: 1 }\nhh = rr.h }",
        "var rr: { h: Hue, k: int } = { h: Hue.Red, k: 0 }\non go { let m = @\nrr = { h: m, k: 1 }\nhh = rr.h }",
        "on go { let t = { m: @, z: 1 }\nhh = t.m }",
        "on go { hh = if bb then @ else Hue.Red }",
        "on go { let k = @\nhh = if bb then Hue.Red else k }",
        "on go { hh = Select(bb, @, Hue.Red) }",
        "on go { let k = @\nhh = Select(bb, Hue.Red, k) }",
        "on go { hh = match hh { Hue.Red => @, _ => Hue.Blue } }",
        "on go { n = EnumToInt(if bb then @ else Hue.Red) }",
        "mod idm(x: Hue) -> Hue { return x }\non go { hh = idm(@) }",
        "mod idm(x: Hue) -> Hue { return x }\non go { let k = @\nhh = idm(k) }",
        "mod idm(x: Hue) -> Hue { return x }\non go { let k = @\nhh = idm(if bb then k else Hue.Blue) }",
    ];
    for (read, first) in [("gh(a)", "h"), ("hs.pop()", "Value")] {
        for place in places {
            let src = format!("{head}{}", place.replace('@', read));
            let twin = format!("{head}{}", place.replace('@', &format!("{read}.{first}")));
            for fold in [false, true] {
                let (got, want) = (held_place_outcome(&src, fold), held_place_outcome(&twin, fold));
                // A choice can't run only the arm taken of a read that
                // changes its container (`WS097`), named or not.
                if read.contains("pop") && !want.compiled && want.codes.iter().all(|c| c == "WS097") {
                    assert_eq!(got.codes, want.codes, "{read} {place}, fold {fold}");
                    continue;
                }
                assert!(got.compiled && want.compiled, "{read} {place}, fold {fold}: {:?} {:?}", got.codes, want.codes);
                // A read held from a mod's stored outputs is read where it is
                // used, its twin where it is bound; each call has storage of
                // its own, so the two read the same value and save the same.
                if place.contains("if bb") {
                    assert_eq!((&got.codes, &got.comps), (&want.codes, &want.comps), "{read} {place}, fold {fold}");
                    continue;
                }
                assert!(got == want, "{read} {place}, fold {fold}: {}", outcome_diff(&got, &want));
            }
        }
        let src = format!("{head}on go {{ bo = {read} == Hue.Blue }}");
        let twin = format!("{head}on go {{ bo = {read}.{first} == Hue.Blue }}");
        assert_eq!(held_place_outcome(&src, false).codes, held_place_outcome(&twin, false).codes, "{read} ==");
        // The message spells the types as the program does.
        let r = compile(&src);
        let msg = r.diagnostics.iter().find(|d| d.code == "WS004").map(|d| d.message.clone()).unwrap_or_default();
        assert!(msg.contains("on ") && !msg.contains("Outputs(") && !msg.contains("Enum {"), "{msg}");
    }
}

/// `Select` or `if` choosing between two reads of several outputs gives a
/// place for one value the output each declares first, whatever the outputs
/// are called. Each row is spelled with many name pairs, in both orders, so a
/// choice made by the order a map holds the names in (which follows their
/// hashes) cannot pass: every form is compared with its twin naming the
/// first output, for a `mod`, a `chip`, held reads of both, three outputs, a
/// generic `mod` and a namespaced call.
#[test]
fn a_choice_between_two_reads_takes_the_first_declared_output_whatever_its_name() {
    let names = [
        ("lo", "hi"),
        ("hi", "lo"),
        ("a1", "b1"),
        ("b1", "a1"),
        ("zq", "aq"),
        ("aq", "zq"),
        ("p1", "p2"),
        ("p2", "p1"),
        ("first", "second"),
        ("second", "first"),
        ("ox", "oy"),
        ("oy", "ox"),
        ("mm", "nn"),
        ("nn", "mm"),
        ("out_b", "out_a"),
        ("out_a", "out_b"),
        ("u7", "k3"),
        ("k3", "u7"),
        ("Value", "Found"),
        ("Found", "Value"),
        ("ee", "dd"),
        ("dd", "ee"),
        ("wq", "vq"),
        ("vq", "wq"),
    ];
    let forms = [
        ("on go { n = Select(bb, g2(a), g2(5)) }", "on go { n = Select(bb, g2(a).@, g2(5).@) }"),
        ("on go { n = Select(bb, C2(a), C2(5)) }", "on go { n = Select(bb, C2(a).@, C2(5).@) }"),
        (
            "let k = C2(a)\nlet j = C2(5)\non go { n = Select(bb, k, j) }",
            "let k = C2(a)\nlet j = C2(5)\non go { n = Select(bb, k.@, j.@) }",
        ),
        (
            "on go { let k = g2(a)\nlet j = g2(5)\nn = Select(bb, k, j) }",
            "on go { let k = g2(a)\nlet j = g2(5)\nn = Select(bb, k.@, j.@) }",
        ),
        ("on go { n = Select(bb, g3(a), g3(5)) }", "on go { n = Select(bb, g3(a).@, g3(5).@) }"),
        ("on go { n = Select(bb, gm(a), gm(5)) }", "on go { n = Select(bb, gm(a).@, gm(5).@) }"),
        ("on go { n = Select(bb, u.pair(a), u.pair(5)) }", "on go { n = Select(bb, u.pair(a).@, u.pair(5).@) }"),
        ("on go { n = Select(bb, g2(a), 7) }", "on go { n = Select(bb, g2(a).@, 7) }"),
        ("on go { n = Select(bb, g2(a), g2(5)) + 1 }", "on go { n = Select(bb, g2(a).@, g2(5).@) + 1 }"),
        ("on go { let t = Select(bb, g2(a), g2(5))\nn = t }", "on go { n = Select(bb, g2(a).@, g2(5).@) }"),
        ("on go { n = if bb then g2(a) else g2(5) }", "on go { n = if bb then g2(a).@ else g2(5).@ }"),
        (
            "let k = C2(a)\nlet j = C2(5)\non go { n = if bb then k else j }",
            "let k = C2(a)\nlet j = C2(5)\non go { n = if bb then k.@ else j.@ }",
        ),
    ];
    let mut wrong = Vec::new();
    for (o1, o2) in names {
        let head = format!(
            "import * as u from \"lib\"\nin go: exec\nin a: int\nin bb: bool\nvar n: int = 0\nout on1: int = n\n\
             mod g2(x: int) -> ({o1}: int, {o2}: int) {{ return (x + 1, x * 2) }}\n\
             mod g3(x: int) -> ({o1}: int, {o2}: int, w3: int) {{ return (x + 1, x * 2, x * 3) }}\n\
             chip C2(x: int) -> ({o1}: int, {o2}: int) {{ out {o1} = x + 1\nout {o2} = x * 2 }}\n\
             mod gm<T: Scalar>(x: T) -> ({o1}: T, {o2}: T) {{ return (x + 1, x * 2) }}\n"
        );
        let lib = format!("mod pair(x: int) -> ({o1}: int, {o2}: int) {{ return (x + 1, x * 2) }}\n");
        let outcome = |src: &str, fold: bool| {
            let mut o = super::shadow_differential::outcome(src, &[("lib", &lib)], fold);
            for s in &mut o.shape {
                *s = s.split(' ').filter(|t| !t.is_empty() && !t.starts_with("_bindname=")).collect::<Vec<_>>().join(" ");
            }
            o.shape.sort();
            o
        };
        for (form, twin) in forms {
            let src = format!("{head}{form}");
            let twin = format!("{head}{}", twin.replace('@', o1));
            for fold in [false, true] {
                let (got, want) = (outcome(&src, fold), outcome(&twin, fold));
                if !got.compiled || got != want {
                    wrong.push(format!("({o1}, {o2}) {form}, fold {fold}: {:?} {}", got.codes, outcome_diff(&got, &want)));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// An `if` or `Select` joining two gate reads joins their outputs under the
/// plain coercion law: output by output when the reads declare the same
/// outputs (an `int` one widens to `float`), by their first outputs when
/// they do not. Each saves what its twin naming the outputs saves; a join
/// with no common type is `WS003` naming both.
#[test]
fn a_join_of_two_reads_joins_their_outputs_by_the_coercion_law() {
    let head = "in go: exec\nin bb: bool\nvar f: float = 0.0\nout of1: float = f\nvar n: int = 0\nout on1: int = n\n\
                var bo: bool = false\nout obo: bool = bo\nvar fs: float[] = [1.5, 2.5]\nvar ns: int[] = [1, 2]\n\
                var ss: string[] = [\"a\"]\nvar s: string = \"\"\nout os: string = s\n\
                mod gi() -> (lo: int, hi: int) { return (1, 2) }\n";
    for (body, twin) in [
        ("f = if bb then fs.max() else ns.max()", "f = if bb then fs.max().Value else ns.max().Value"),
        ("f = Select(bb, fs.max(), ns.max())", "f = Select(bb, fs.max().Value, ns.max().Value)"),
        ("f = if bb then ns.max() else fs.min()", "f = if bb then ns.max().Value else fs.min().Value"),
        ("f = if bb then fs.max() else gi()", "f = if bb then fs.max().Value else gi().lo"),
        ("n = if bb then ns.max() else gi()", "n = if bb then ns.max().Value else gi().lo"),
        (
            "f = if bb then fs.max() else (if bb then ns.max() else fs.min())",
            "f = if bb then fs.max().Value else (if bb then ns.max().Value else fs.min().Value)",
        ),
        ("let t = if bb then fs.max() else ns.max()\nf = t", "let t = if bb then fs.max() else ns.max()\nf = t.Value"),
    ] {
        for fold in [false, true] {
            let got = held_place_outcome(&format!("{head}on go {{ {body} }}"), fold);
            let want = held_place_outcome(&format!("{head}on go {{ {twin} }}"), fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert!(got == want, "{body}, fold {fold}: {}", outcome_diff(&got, &want));
        }
    }
    let r = compile(&format!("{head}on go {{ s = if bb then fs.max() else ss.max() }}"));
    let msg = r.diagnostics.iter().find(|d| d.code == "WS003").map(|d| d.message.clone()).unwrap_or_default();
    assert!(msg.contains("float") && msg.contains("string"), "{msg}");
}

/// A record a bounded type parameter stands for, read where one value goes
/// in a generic body, is `WS003` naming the bound to narrow (a field of the
/// record is no field of the bound's other members), in whatever order the
/// bound names its members, and the bound it suggests compiles. A bound of
/// records alone keeps the field hint.
#[test]
fn a_generic_body_record_read_as_one_value_names_the_bound_to_narrow() {
    let head = "in go: exec\ntype R = { x: int, z: int }\ntype Q = { x: int, w: int }\n\
                var mi: Map<string, int> = { \"k\": 4 }\nvar n: int = 0\nout on1: int = n\n";
    let body = |bound: &str| {
        format!(
            "{head}mod gf<T: {bound}>(m: Map<string, T>) -> int {{ let r = m.get(\"k\")\nreturn r + 1 }}\n\
             on go {{ n = gf(mi) }}"
        )
    };
    // The members named are the ones whose check of the body reported no
    // error, a class in the bound (`Scalar`, `Numeric`) read as its members,
    // so the bound suggested compiles.
    for (bound, narrowed) in [
        ("R | int", "int"),
        ("int | R", "int"),
        ("R | int | float", "int | float"),
        ("R | Scalar", "int | float"),
        ("Scalar | R", "int | float"),
        ("R | Numeric", "int | float"),
        ("Numeric | R", "int | float"),
        ("R | int | vector", "int"),
        ("vector | int | R", "int"),
    ] {
        let r = compile(&body(bound));
        let msg = r.diagnostics.iter().find(|d| d.code == "WS003").map(|d| d.message.clone()).unwrap_or_default();
        assert!(msg.contains(&format!("narrow the bound to the members this body works with: `<T: {narrowed}>`")), "{bound}: {msg}");
        assert!(!msg.contains("`r.x`") && !msg.contains("read one of its fields"), "{bound}: {msg}");
        assert!(errs(&body(narrowed)).is_empty(), "{narrowed}: {:?}", errs(&body(narrowed)));
    }
    // A bound naming a class admits each of its members.
    let scalar = format!("{head}mod id<T: R | Scalar>(x: T) -> T {{ return x }}\non go {{ n = id(n) }}");
    assert!(errs(&scalar).is_empty(), "{:?}", errs(&scalar));
    let src = format!("{head}mod gf<T: R | Q>(m: Map<string, T>) -> int {{ let r = m.get(\"k\")\nreturn r + 1 }}");
    let r = compile(&src);
    let msg = r.diagnostics.iter().find(|d| d.code == "WS003").map(|d| d.message.clone()).unwrap_or_default();
    assert!(msg.contains("`r.x`") && !msg.contains("narrow"), "{msg}");
}

/// A field read of a call reads that call's outputs, never the outputs of
/// a call an earlier statement gave as an argument of one value: after
/// `mtf.set("k", gr())`, `mtf.get("j").Value` read `gr()`'s `Value`.
#[test]
fn a_field_read_of_a_call_reads_its_own_outputs() {
    let head = "in go: exec\nvar f: float = 0.0\nout of1: float = f\nvar bo: bool = false\nout obo: bool = bo\n\
                var fs: float[] = [1.5, 2.5]\nvar mtf: Map<string, float> = { \"j\": 9.0 }\n\
                mod gr() -> (Value: float, IsEmpty: bool) { return fs.min() }\n\
                mod g2() -> (lo: float, hi: float) { return (1.0, 2.0) }\n";
    for (body, twin) in [
        ("mtf.set(\"k\", gr())\nf = mtf.get(\"j\").Value", "mtf.set(\"k\", gr().Value)\nf = mtf.get(\"j\").Value"),
        ("fs.push(gr())\nf = mtf.get(\"j\").Value", "fs.push(gr().Value)\nf = mtf.get(\"j\").Value"),
        ("mtf.set(\"k\", gr())\nbo = mtf.get(\"j\").Found", "mtf.set(\"k\", gr().Value)\nbo = mtf.get(\"j\").Found"),
        ("mtf.set(\"k\", g2())\nbo = fs.find(2.5).Found", "mtf.set(\"k\", g2().lo)\nbo = fs.find(2.5).Found"),
    ] {
        for fold in [false, true] {
            let got = held_place_outcome(&format!("{head}on go {{ {body} }}"), fold);
            let want = held_place_outcome(&format!("{head}on go {{ {twin} }}"), fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert!(got == want, "{body}, fold {fold}: {}", outcome_diff(&got, &want));
        }
    }
}

/// A `var` written with no type and initialized from a record is declared
/// as the record's type written out, field by field: it saves what the
/// annotated `var` saves, read by field, assigned whole or returned. It was
/// one `float` var whose field read was a vector's `X`, and returning or
/// assigning it whole was `WS094` or `WS071`.
#[test]
fn an_unannotated_var_of_a_record_is_declared_as_the_record() {
    let head = "in go: exec\nvar n: int = 0\nout on1: int = n\ntype R = { x: int, y: int }\nvar rr: R = { x: 0, y: 0 }\n\
                var mr: Map<string, R> = { \"k\": { x: 5, y: 6 } }\nvar rs: R[] = [{ x: 1, y: 2 }]\n";
    for (body, ty) in [
        ("on go { var v@ = (n, 1)\nn = v.0 + v.1 }", ": (int, int)"),
        ("on go { var v@ = { x: n, y: 2 }\nv.x = v.y + 3\nn = v.x }", ": R"),
    ] {
        for fold in [false, true] {
            let got = held_place_outcome(&format!("{head}{}", body.replace('@', "")), fold);
            let want = held_place_outcome(&format!("{head}{}", body.replace('@', ty)), fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert!(got == want, "{body}, fold {fold}: {}", outcome_diff(&got, &want));
        }
    }
    for body in [
        "on go { var r@ = mr.get(\"k\")\nn = r.y }",
        "on go { var r@ = mr.get(\"k\")\nrr = r }",
        "on go { var r@ = rs.pop()\nn = r.x + r.y }",
        "on go { var r@ = rr\nr.x = 4\nrr = r }",
        "mod fr(m: Map<string, R>) -> R { var r@ = m.get(\"k\")\nreturn r }\non go { rr = fr(mr) }",
        "mod fq(q: R) -> R { var r@ = q\nreturn r }\non go { rr = fq(rr) }",
    ] {
        for fold in [false, true] {
            let got = held_place_outcome(&format!("{head}{}", body.replace('@', "")), fold);
            let want = held_place_outcome(&format!("{head}{}", body.replace('@', ": R")), fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert!(got == want, "{body}, fold {fold}: {}", outcome_diff(&got, &want));
        }
    }
}

/// A record of the program's own has no first output: read as one value in
/// an operand, a comparison, a condition or an interpolation it is `WS003`,
/// a call's result included, and so is a gate read whose first output is
/// one (the first output is read once, never a field of it). A generic body
/// whose union bound holds such a record is checked per member, whichever
/// order the bound lists them in. A record field named like a gate output
/// (`Value`) is that field.
#[test]
fn a_record_of_the_programs_own_read_as_one_value_is_ws003() {
    let head = "in go: exec\nin a: int\nvar n: int = 0\nout on1: int = n\nvar bo: bool = false\nout obo: bool = bo\n\
                var s: string = \"\"\nout os: string = s\ntype R = { Value: int, z: int }\n\
                var pv: R = { Value: 5, z: 3 }\nvar mr: Map<string, R> = { \"k\": { Value: 5, z: 3 } }\nvar rs: R[]\n\
                mod mk(x: int) -> R { return { Value: x, z: 9 } }\n\
                mod g3(x: int) -> (p: R, q: int) { return ({ Value: x, z: 7 }, 9) }\nmod one(k: int) -> int { return k }\n";
    for body in [
        "on go { n = mk(a) + 1 }",
        "on go { n = -mk(a) }",
        "on go { s = \"${mk(a)}\" }",
        "on go { n = mr.get(\"k\") + 1 }",
        "on go { bo = mr.get(\"k\") == 5 }",
        "on go { if mr.get(\"k\") { n = 1 } }",
        "on go { s = \"${mr.get(\"k\")}\" }",
        "on go { n = rs.pop() + 1 }",
        "on go { bo = if mk(a) then true else false }",
        "on go { n = pv + 1 }",
        "on go { s = \"${pv}\" }",
        "on go { if pv { n = 1 } }",
        "on go { n = g3(a) }",
        "on go { n = g3(a) + 1 }",
        "on go { s = \"${g3(a)}\" }",
        "on go { n = one(g3(a)) }",
        "on go { let t = g3(a)\nn = t }",
        "mod f<T: R | string>(m: Map<string, T>) -> string { let r = m.get(\"k\")\nreturn \"${r}\" }\non go { s = f(mr) }",
        "mod f<T: string | R>(m: Map<string, T>) -> string { let r = m.get(\"k\")\nreturn \"${r}\" }\non go { s = f(mr) }",
        "mod f<T: R | int>(m: Map<string, T>) -> int { let r = m.get(\"k\")\nreturn r + 1 }\non go { n = f(mr) }",
        "mod f<T: int | R>(m: Map<string, T>) -> int { return m.get(\"k\") + 1 }\non go { n = f(mr) }",
    ] {
        let mut codes = errs(&format!("{head}{body}"));
        codes.dedup();
        assert_eq!(codes, ["WS003"], "{body}");
    }
    // A bound of members each giving a gate read reads its first output in
    // every instantiation, whichever order the bound lists them in.
    let head2 = format!("{head}var ms: Map<string, string> = {{ \"k\": \"v\" }}\nvar mi: Map<string, int> = {{ \"k\": 4 }}\n");
    let generic = |bound: &str, read: &str| {
        format!(
            "mod f<T: {bound}>(m: Map<string, T>) -> string {{ let r = m.get(\"k\"){read}\nreturn \"${{r}}\" }}\n\
             on go {{ s = f(ms)\ns = f(mi) }}"
        )
    };
    for (body, named) in [
        (generic("int | string", ""), generic("int | string", ".Value")),
        (generic("string | int", ""), generic("string | int", ".Value")),
    ] {
        for fold in [false, true] {
            let (got, want) =
                (held_place_outcome(&format!("{head2}{body}"), fold), held_place_outcome(&format!("{head2}{named}"), fold));
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert_eq!(got, want, "{body}, fold {fold}");
        }
    }
    // A field named like a gate output is the field, as destructuring it is.
    let head = "in go: exec\nvar f: float = 0.0\nout of1: float = f\nvar bo: bool = false\nout obo: bool = bo\n\
                type V = { Value: float, IsEmpty: bool }\nvar rv: V = { Value: 2.0, IsEmpty: true }\n";
    for fold in [false, true] {
        let got = held_place_outcome(&format!("{head}on go {{ f = rv.Value\nbo = rv.IsEmpty }}"), fold);
        let want =
            held_place_outcome(&format!("{head}on go {{ let {{ Value: v, IsEmpty: e }} = rv\nf = v\nbo = e }}"), fold);
        assert!(got.compiled, "{:?}", got.codes);
        assert!(got == want, "fold {fold}: {}", outcome_diff(&got, &want));
    }
    let codes = errs(&format!("{head}on go {{ let t = rv.Value\nbo = t.IsEmpty }}"));
    assert_eq!(codes, ["WS010"]);
}

/// A `var` initialized from a namespaced constant record stores its fields
/// with no warning that the value is dropped.
#[test]
fn a_var_from_a_namespaced_constant_record_is_stored_without_a_warning() {
    let lib = "pub type P = { x: int, y: float }\npub const LP: P = { x: 11, y: 1.25 }\n";
    let main = "import * as u from \"lib\"\nin go: exec\nvar n: int = 0\nout on1: int = n\nvar p: u.P = u.LP\non go { n = p.x }\n";
    let r = compile_multi(main, &[("lib", lib)]);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let stored: Vec<Literal> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::VAR)
        .filter_map(|n| n.properties.get(&*crate::intern::sym::INITIAL_VALUE).cloned())
        .collect();
    assert!(stored.contains(&Literal::Int(11)) && stored.contains(&Literal::Float(1.25)), "{stored:?}");
}
