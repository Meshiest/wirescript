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
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "writing through a record ref field should produce a Var_Set gate"
    );

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
    let has_incr = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment");
    let has_set = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        has_incr || has_set,
        "record ref field increment inside mod should produce IncVar or Var_Set"
    );

    let pseudo_var = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
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
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Push"),
        "pushing through a record array field should produce an ArrayVar_Push gate"
    );

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
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "b.x + b.y should produce a MathAdd gate"
    );

    // Spread must not generate extra addition gates.
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
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
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "destructured x + y should produce a MathAdd gate"
    );

    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
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
    let add = find_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd");
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
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
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
    let set_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        set_count >= 1,
        "record field |= inside if inside mod should produce Var_Set, got {}",
        set_count
    );
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
    assert_eq!(gate_count(&r, gc::PSEUDO_VAR), 2, "one Pseudo_Var per field");
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
    assert_eq!(gate_count(&r, gc::PSEUDO_VAR), 3, "a, b, q each a Pseudo_Var");
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
    assert_eq!(gate_count(&r, gc::PSEUDO_ARRAY_VAR), 2, "one array per field");
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
    assert_eq!(gate_count(&r, gc::PSEUDO_MAP_VAR), 2, "one map per field");
    // set(1) + m[1]=(1) = 2 sets, each fanned to 2 fields = 4
    assert_eq!(gate_count(&r, gc::MAP_SET), 4);
    // get(0).x (1) + m[1].x (1) + p=m.get(0) (2) = 4
    assert_eq!(gate_count(&r, gc::MAP_GET), 4);
    assert_eq!(gate_count(&r, gc::MAP_HAS), 1, "has reads first field");
    assert_eq!(gate_count(&r, gc::MAP_GET_LENGTH), 1, "length reads first field");
    assert_eq!(gate_count(&r, gc::MAP_REMOVE), 2);
    assert_eq!(gate_count(&r, gc::MAP_CLEAR), 2);
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
        .filter(|n| n.gate_class == gc::PSEUDO_ARRAY_VAR)
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
        .filter(|n| n.gate_class == gc::PSEUDO_MAP_VAR && n.properties.contains_key(&iv))
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
    assert_eq!(gate_count(&r, gc::PSEUDO_VAR), 3, "p.x, p.y + local a");
    assert_eq!(gate_count(&r, gc::PSEUDO_ARRAY_VAR), 2, "pts.x, pts.y");
    assert_eq!(gate_count(&r, gc::PSEUDO_MAP_VAR), 2, "m.x, m.y");
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
    assert_eq!(gate_count(&r, gc::PSEUDO_ARRAY_VAR), 2);
    assert_eq!(gate_count(&r, gc::PSEUDO_MAP_VAR), 2);
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
