//! Regressions for three lowering bugs reported together (array aliasing,
//! imported-namespace tuple access, and imported `on` handlers).

use super::*;

fn has_unsupported(r: &LowerResult) -> bool {
    fn walk(m: &crate::ir::Module) -> bool {
        m.nodes.values().any(|n| n.gate_class == "_Unsupported") || m.chips.values().any(walk)
    }
    walk(&r.module)
}

fn count_class(m: &crate::ir::Module, class: &str) -> usize {
    let mut n = m.nodes.values().filter(|x| x.gate_class == class).count();
    for c in m.chips.values() {
        n += count_class(c, class);
    }
    n
}

/// A `let` aliasing an array var must resolve for `x[i]` (and array methods),
/// exactly like the original.
#[test]
fn let_alias_of_array_var_resolves_index() {
    let r = compile("var a = [0]\nin go: exec\non go { let ar = a\n let v = ar[0] }");
    assert!(
        !has_unsupported(&r),
        "aliased array index lowered to _Unsupported: {:?}",
        r.diagnostics
    );
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
        1,
        "ar[0] must lower to an ArrayVar_Get on the aliased array"
    );
}

/// `let t = Other.member; t.0` where the namespace member's bare name is ALSO
/// owned by the importer (`in start`): the member must still be reachable as
/// `Other.member` (captured into the namespace map without clobbering the local
/// `in start`).
#[test]
fn imported_namespace_tuple_pick_resolves() {
    let r = compile_multi(
        "in start: exec\n\
         import * as Other from \"test2\"\n\
         on start { let test = Other.start\n let x = test.0 }",
        &[("test2", "let start = (1,2,3,4)")],
    );
    assert!(
        !has_unsupported(&r),
        "imported-namespace tuple pick lowered to _Unsupported: {:?}",
        r.diagnostics
    );
}

/// A top-level `on` handler in an imported file runs as part of the importing
/// program: its body lowers and wires to its own trigger.
#[test]
fn imported_on_handler_generates() {
    let r = compile_multi(
        "import \"lib\"\non ReadBrickGrid() { BroadcastChatMessage(\"main\") }",
        &[("lib", "on ReadBrickGrid() { BroadcastChatMessage(\"lib\") }")],
    );
    assert_eq!(
        count_class(
            &r.module,
            "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage"
        ),
        2,
        "both the local and the imported handler bodies must generate"
    );
    assert!(
        !has_unsupported(&r),
        "imported handler left an _Unsupported node: {:?}",
        r.diagnostics
    );
}

/// A `let` aliasing an INPUT-port array/map (`in a: int[]` then `let x = a`)
/// must resolve for `x[i]` and methods, like a `var` alias.
#[test]
fn let_alias_of_input_array_resolves_index() {
    let r = compile("in a: int[]\nin go: exec\non go { let x = a\n let v = x[0] }");
    assert!(
        !has_unsupported(&r),
        "aliased input-array index lowered to _Unsupported: {:?}",
        r.diagnostics
    );
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
        1,
        "x[0] must lower to an ArrayVar_Get on the aliased input"
    );
}

/// Every node id used as a wire SOURCE anywhere in the module tree.
fn wire_sources(m: &crate::ir::Module) -> crate::collections::HashSet<crate::ir::NodeId> {
    let mut out: crate::collections::HashSet<crate::ir::NodeId> =
        m.wires.iter().map(|w| w.source.node_id).collect();
    for c in m.chips.values() {
        out.extend(wire_sources(c));
    }
    out
}

/// Root-module var gates carrying `name`, by `NAME_LABEL`.
fn var_ids_named(m: &crate::ir::Module, name: &str) -> Vec<crate::ir::NodeId> {
    let mut ids: Vec<crate::ir::NodeId> = m
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .filter(|(_, n)| {
            matches!(
                n.properties.get(&*sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == name || s.starts_with(&format!("{name}."))
            )
        })
        .map(|(id, _)| *id)
        .collect();
    ids.sort();
    ids
}

/// A `chip` with a `*T` param, called twice with different vars: the second
/// instance is stamped from the template cache and must remap its captured
/// externals to its own argument, or every call site writes through to the
/// first argument and the later vars go unwired.
#[test]
fn chip_ref_param_rebinds_per_call_site() {
    let r = compile(
        "chip M(self: *float, x: float) -> float {\n  self = x + 2\n  return self * 3.0\n}\n\
         var ta: float = 1.0\nvar tb: float = 2.0\nin foo: float\n\
         var output: float = 0.0\nout result: float = output\n\
         in go: exec\non go {\n  output += M(ta, foo)\n  output += M(tb, foo)\n}",
    );
    let sources = wire_sources(&r.module);
    for name in ["ta", "tb"] {
        let ids = var_ids_named(&r.module, name);
        assert!(!ids.is_empty(), "no var gate named {name}");
        assert!(
            ids.iter().any(|id| sources.contains(id)),
            "`{name}` was passed by ref to a chip but its var gate is wired to \
             nothing, so the instance rebound to the other call site's var"
        );
    }
}

/// Same for a record `*T` param, where the capture is one entry per field.
#[test]
fn chip_record_ref_param_rebinds_per_call_site() {
    let r = compile(
        "type P = { a: float, b: float }\n\
         chip M(self: *P, x: float) -> float {\n  self.a = x + 2\n  return self.a * self.b\n}\n\
         var ta: P = { a: 1.0, b: 1.5 }\nvar tb: P = { a: 2.0, b: 2.5 }\nin foo: float\n\
         var output: float = 0.0\nout result: float = output\n\
         in go: exec\non go {\n  output += M(ta, foo)\n  output += M(tb, foo)\n}",
    );
    let sources = wire_sources(&r.module);
    for name in ["ta", "tb"] {
        let ids = var_ids_named(&r.module, name);
        assert!(!ids.is_empty(), "no var gates for record {name}");
        assert!(
            ids.iter().all(|id| sources.contains(id)),
            "record `{name}` was passed by ref to a chip but some of its field vars \
             are wired to nothing, so the instance rebound to the other call site's record"
        );
    }
}


/// A `match` EXPRESSION whose arms are record VALUES must lower to a per-field
/// `Select` tree and write the target's fields. It used to drop the whole
/// statement — no Select, no `Var_Set`, no diagnostic — while the statement
/// form (`match s { A => { r = p } ... }`) lowered correctly.
#[test]
fn match_expr_with_record_arms_writes_target_fields() {
    let src = "type P = { x: int, y: int }\n\
               enum S { A, B }\n\
               var r: P = { x: 0, y: 0 }\n\
               var p: P = { x: 1, y: 2 }\n\
               var q: P = { x: 3, y: 4 }\n\
               var s: S = S.A\n\
               in go: exec\n\
               on go { r = match s { A => p, B => q } }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_Select"),
        2,
        "one Select per record leaf field (x, y)"
    );
    // Two leaves written into `r`, plus the source reads.
    assert!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_Var_Set") >= 2,
        "each of `r`'s fields must be written"
    );
}

/// The same for a `match` expression whose arms are ENUM values: an enum value
/// is a `__disc` + payload-slot record, so it takes the identical per-leaf
/// Select path.
#[test]
fn match_expr_with_enum_arms_writes_target_slots() {
    let src = "enum Dir { N, E }\n\
               enum S { A, B }\n\
               var out1: Dir = Dir.N\n\
               var d1: Dir = Dir.N\n\
               var d2: Dir = Dir.E\n\
               var s: S = S.A\n\
               in go: exec\n\
               on go { out1 = match s { A => d1, B => d2 } }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_Select"),
        1,
        "one Select for the `__disc` leaf"
    );
    assert!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_Var_Set") >= 1,
        "the target's `__disc` must be written"
    );
}

/// A record-valued `match` expression bound to a `let`, then read by field.
#[test]
fn match_expr_record_bound_to_let_reads_field() {
    let src = "type P = { x: int, y: int }\n\
               enum S { A, B }\n\
               var o: int = 0\n\
               var p: P = { x: 1, y: 2 }\n\
               var q: P = { x: 3, y: 4 }\n\
               var s: S = S.A\n\
               in go: exec\n\
               on go { let m = match s { A => p, B => q }\n o = m.x }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    assert!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_Select") >= 1,
        "the read field must come through a Select"
    );
}

/// A record-valued `match` in every other value position: a record array push,
/// a record map set, an `out` port, and a `mod`/`chip` argument. Each routes
/// through the same shared resolver, so a gap in one is a gap in all.
#[test]
fn match_expr_record_resolves_in_every_value_position() {
    let src = "type P = { x: int, y: int }\n\
               enum S { A, B }\n\
               var p: P = { x: 1, y: 2 }\n\
               var q: P = { x: 3, y: 4 }\n\
               var s: S = S.A\n\
               var arr: P[]\n\
               var mp: Map<int, P>\n\
               var g1: int = 0\n\
               var g2: int = 0\n\
               mod takeRec(v: P) -> (o: int) { return v.x + v.y }\n\
               chip ChRec(v: P) -> (o: int) { out o = v.x - v.y }\n\
               in go: exec\n\
               on go {\n\
                 arr.push(match s { A => p, B => q })\n\
                 mp.set(1, match s { A => p, B => q })\n\
                 g1 = takeRec(match s { A => p, B => q })\n\
                 g2 = ChRec(match s { A => p, B => q })\n\
               }\n\
               out sum = (match s { A => p, B => q }).x";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
}

/// A `mod` takes a record argument in every form its `chip` twin does. The
/// container-element and record-valued-conditional spellings used to reach the
/// callee as one opaque value port, so each field read in the body lowered to
/// an `_Unsupported` placeholder.
#[test]
fn mod_record_argument_accepts_every_value_form() {
    let src = "type P = { x: int, y: int }\n\
               var p: P = { x: 1, y: 2 }\n\
               var q: P = { x: 3, y: 4 }\n\
               var arr: P[]\n\
               var mp: Map<int, P>\n\
               var c: bool = false\n\
               var g1: int = 0\n\
               var g2: int = 0\n\
               var g3: int = 0\n\
               mod takeRec(v: P) -> (o: int) { return v.x + v.y }\n\
               in go: exec\n\
               on go {\n\
                 arr.push(p)\n\
                 mp.set(1, p)\n\
                 g1 = takeRec(if c then p else q)\n\
                 g2 = takeRec(arr[0])\n\
                 g3 = takeRec(mp[1])\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
}

/// `let m = if c then p else q` on records binds a per-field record, the same
/// way the assignment form writes one. Only the assignment position resolved,
/// so the `let` reported WS071 on each branch and read a placeholder.
#[test]
fn record_valued_if_expr_binds_to_a_let() {
    let src = "type P = { x: int, y: int }\n\
               var p: P = { x: 1, y: 2 }\n\
               var q: P = { x: 3, y: 4 }\n\
               var c: bool = false\n\
               var o: int = 0\n\
               in go: exec\n\
               on go { let m = if c then p else q\n o = m.y }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    assert!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_Select") >= 1,
        "the read field must come through a Select"
    );
}

/// A MULTI-OUTPUT gate result chosen by an `if` picks each port separately.
/// `m.get(k)` is `{Value, Found}`, but the conditional built ONE Select over
/// port 0, so `c.Found` read the *value* Select and the gate's `bFound` port
/// was wired nowhere - typecheck-clean, no placeholder, wrong at runtime.
#[test]
fn multi_output_result_through_a_conditional_selects_per_port() {
    let src = "var m: Map<int, int>\n\
               var ok: bool = false\n\
               var v: int = 0\n\
               var c: bool = false\n\
               in go: exec\n\
               on go {\n\
                 c = Opaque(true)\n\
                 let r = if c then m.get(7) else m.get(8)\n\
                 ok = r.Found\n\
                 v = r.Value\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_Select"),
        2,
        "one Select per output port (Value, Found)"
    );
    // BOTH Selects must reach a store: one shared source (what the bug did)
    // leaves the other Select driving nothing.
    let selects: Vec<crate::ir::NodeId> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_Select")
        .map(|n| n.id)
        .collect();
    for sel in &selects {
        assert!(
            r.module.wires.iter().any(|w| w.source.node_id == *sel
                && w.target.port == crate::lower::WirePort::Value),
            "Select {sel} drives no store, so one port's choice was dropped"
        );
    }
    // Each Select reads a DIFFERENT pair of source ports (the two `bFound`
    // ports vs the two `Value` ports).
    let feeds: std::collections::HashSet<_> = r
        .module
        .wires
        .iter()
        .filter(|w| selects.contains(&w.target.node_id)
            && matches!(w.target.port, crate::lower::WirePort::InputA | crate::lower::WirePort::InputB))
        .map(|w| w.source)
        .collect();
    assert_eq!(feeds.len(), 4, "expected 2 bFound + 2 Value feeds, got {feeds:?}");
}

/// A `mod` with ONE record-typed output hands back the record itself, not a
/// wrapper keyed by the output name. Typecheck already reports the call as the
/// record (`mk().x` is the field, `mk().o` a WS010), but lowering bound
/// `{o: {x, y}}`: `r = mk(1)` wrote no field at all, and `let d = mk(1)` then
/// `d.x` lowered to a placeholder.
#[test]
fn single_record_output_mod_unwraps_to_its_fields() {
    let src = "type P = { x: int, y: int }\n\
               mod mkP(n: int) -> (o: P) { out o = { x: n, y: n * 2 } }\n\
               var r: P = { x: 0, y: 0 }\n\
               var a: int = 0\n\
               in go: exec\n\
               on go {\n\
                 r = mkP(1)\n\
                 let d = mkP(2)\n\
                 a = d.x\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    // Both of `r`'s fields, plus `a`.
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_Var_Set"),
        3,
        "`r.x`, `r.y` and `a` must each be written"
    );
}

/// A record-returning `mod` in each arm of a conditional: the arms unwrap, so
/// the choice is still made per leaf field.
#[test]
fn record_returning_mod_in_both_conditional_arms_selects_per_field() {
    let src = "type P = { x: int, y: int }\n\
               mod mkP(n: int) -> (o: P) { out o = { x: n, y: n * 2 } }\n\
               var r: P = { x: 0, y: 0 }\n\
               var c: bool = false\n\
               in go: exec\n\
               on go {\n\
                 c = Opaque(true)\n\
                 r = if c then mkP(1) else mkP(3)\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r), "diags: {:?}", r.diagnostics);
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_Select"),
        2,
        "one Select per record field"
    );
    // `r.x`, `r.y`, plus the `c = Opaque(true)` probe write.
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_Var_Set"),
        3,
        "both of `r`'s fields must be written"
    );
}
