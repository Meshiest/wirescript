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
    // Both files desugar their non-event `on Foo()` to `let _on_expr_0`, and
    // the counter restarts per file, so the cross-file WS013 fired on a name
    // the compiler generated. Nothing here asserted on diagnostics, so two
    // libraries that each used an `on <call>()` handler could not be compiled
    // together and this test stayed green.
    assert_no_errors(&r);
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

/// A container method given `exec = <trigger>` must not rewire the statements
/// that follow it.
///
/// `exec =` makes the op a leaf, so the caller's exec context is saved and
/// restored around it. A restore that sits past an early return leaves
/// `current_exec` pointing at the trigger, and every following statement in
/// the handler runs off `trig` instead of `RoundStart`. `insert` is the case
/// here because it rejects named args and bails to `_Unsupported`.
#[test]
fn an_exec_arg_on_a_container_method_does_not_capture_the_following_statements() {
    let r = compile(
        "var xs: int[]\nvar a: int\nin trig: exec\n\
         on RoundStart() {\n  xs.insert(index = 0, value = 7, exec = trig)\n  a = 1\n}\n",
    );
    let event = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.kind == crate::ir::NodeKind::Event)
        .map(|(id, _)| *id)
        .expect("the RoundStart event node");
    let set = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == crate::ir::gate_class::VAR_SET)
        .map(|(id, _)| *id)
        .expect("the `a = 1` set gate");
    let driver = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == set && w.target.port == crate::ir::port_registry::WirePort::Exec)
        .expect("`a = 1` must be driven by something");
    assert_eq!(
        driver.source.node_id, event,
        "`a = 1` must run off RoundStart, not the `exec =` trigger"
    );
}

/// A branching-return `mod` called from a pure position reports at the call
/// site, rather than compiling and failing at emit.
///
/// With no output wire and no chain to continue from, the inline call handed
/// back `NodeId(0)`, the never-allocated node, as if it were a real port. The
/// program type-checked and lowered clean and then died in emit with
/// `DroppedWire("n0 -> ...")`, which names nothing the author wrote.
#[test]
fn a_branching_return_called_purely_reports_at_the_call_site() {
    let src = "mod branchy(c: bool) -> (r: int) {\n\
               \x20 if c { return 1 } else { return 2 }\n}\nout v = branchy(true)\n";
    let r = compile(src);
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS007" && d.message.contains("returns from inside a branch")),
        "expected a call-site diagnostic: {:?}",
        r.diagnostics
    );
    // And no wire references the sentinel, which is what emit choked on.
    assert!(
        !r.module.wires.iter().any(|w| {
            w.source.node_id == crate::ir::NodeId(0) || w.target.node_id == crate::ir::NodeId(0)
        }),
        "a wire still references the never-allocated node"
    );

    // From an exec context the same mod is fine.
    let ok = compile(
        "mod branchy(c: bool) -> (r: int) {\n\
         \x20 if c { return 1 } else { return 2 }\n}\n\
         in go: exec\nvar v: int\non go { v = branchy(true) }\n",
    );
    assert_no_errors(&ok);
}
/// Is some `source_port` of a `source_class` node wired to some `target_port`
/// of a `target_class` node? Proves the read reaches its consumer, which a gate
/// count cannot: an `_Unsupported` object leaves the consumer in place with its
/// input pin unwired, and emit drops the wires rather than the node.
fn wired_between(
    r: &LowerResult,
    source_class: &str,
    source_port: WirePort,
    target_class: &str,
    target_port: WirePort,
) -> bool {
    r.module.wires.iter().any(|w| {
        w.source.port == source_port
            && w.target.port == target_port
            && r.module
                .nodes
                .get(&w.source.node_id)
                .is_some_and(|n| n.gate_class == source_class)
            && r.module
                .nodes
                .get(&w.target.node_id)
                .is_some_and(|n| n.gate_class == target_class)
    })
}

/// `arr[i].Value` is the capitalisation the docs teach, and the hand-written
/// field list matched only the lowercase `value`, so it type-checked and then
/// lowered to an `_Unsupported` placeholder: emit refuses to spawn that gate
/// and drops every wire touching it, deleting the read with no diagnostic.
/// Asserts the real gate AND the wire into the consumer, because the whole
/// suite stayed green while this behaviour changed.
#[test]
fn array_index_capital_value_reads_the_array_get() {
    for field in ["value", "Value"] {
        let src = format!(
            "var regs: int[]\n\
             var g: int = 0\n\
             in go: exec\n\
             on go {{ let pc = regs[15].{field}\n g = pc }}"
        );
        let r = compile(&src);
        assert_no_errors(&r);
        assert!(
            !has_unsupported(&r),
            "regs[15].{field} lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert_eq!(
            count_class(&r.module, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
            1,
            "regs[15].{field} must lower to one ArrayVar_Get"
        );
        assert!(
            wired_between(
                &r,
                "BrickComponentType_WireGraph_Exec_ArrayVar_Get",
                WirePort::Value,
                "BrickComponentType_WireGraph_Exec_Var_Set",
                WirePort::Value,
            ),
            "regs[15].{field} must drive `g`'s Value pin, not be left dangling"
        );
    }
}

/// The out-of-bounds flag reached the same hand-written list, so every
/// capitalisation of it is pinned alongside `Value`.
#[test]
fn array_index_out_of_bounds_flag_accepts_every_capitalisation() {
    for field in ["bOutOfBounds", "OutOfBounds", "BOutOfBounds", "outOfBounds"] {
        let src = format!(
            "var regs: int[]\n\
             var oob: bool = false\n\
             in go: exec\n\
             on go {{ let f = regs[15].{field}\n oob = f }}"
        );
        let r = compile(&src);
        assert_no_errors(&r);
        assert!(
            !has_unsupported(&r),
            "regs[15].{field} lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert!(
            wired_between(
                &r,
                "BrickComponentType_WireGraph_Exec_ArrayVar_Get",
                WirePort::BOutOfBounds,
                "BrickComponentType_WireGraph_Exec_Var_Set",
                WirePort::Value,
            ),
            "regs[15].{field} must read the gate's bOutOfBounds port"
        );
    }
}

/// A capitalisation the pseudo-field list missed did not degrade: the trigger
/// arm matched nothing and returned, deleting the whole handler with no
/// diagnostic, while the same field in expression position reported WS010.
#[test]
fn a_var_field_trigger_accepts_every_capitalisation() {
    for field in ["Value", "value", "prev", "Prev", "PREV"] {
        let src = format!(
            "var v: int = 0\n\
             var hit: int = 0\n\
             on v.{field} {{ hit = 1 }}"
        );
        let r = compile(&src);
        assert_no_errors(&r);
        assert_eq!(
            count_class(&r.module, "BrickComponentType_WireGraph_Exec_Var_Set"),
            1,
            "on v.{field} produced no handler body at all: {:?}",
            r.diagnostics
        );
    }
}

/// The map-index found flag reached the same hand-written list, and `.Found`
/// (the spelling the docs teach) was the one missing name, so it lowered
/// to an `_Unsupported` placeholder that emit deletes, silently.
#[test]
fn map_index_found_flag_accepts_every_capitalisation() {
    for field in ["bFound", "Found", "BFound", "found"] {
        let src = format!(
            "var m: Map<int, int>\n\
             var hit: bool = false\n\
             in go: exec\n\
             on go {{ let f = m[1].{field}\n hit = f }}"
        );
        let r = compile(&src);
        assert_no_errors(&r);
        assert!(
            !has_unsupported(&r),
            "m[1].{field} lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert!(
            wired_between(
                &r,
                "BrickComponentType_WireGraph_Exec_MapVar_Get",
                WirePort::BFound,
                "BrickComponentType_WireGraph_Exec_Var_Set",
                WirePort::Value,
            ),
            "m[1].{field} must read the gate's bFound port"
        );
    }
}

/// An ARRAY has no `bFound` port, so widening which spellings resolve must not
/// widen which objects carry the port: `arr[i].Found` still degrades.
#[test]
fn found_on_an_array_index_stays_unsupported() {
    let r = compile(
        "var regs: int[]\n\
         var hit: bool = false\n\
         in go: exec\n\
         on go { let f = regs[0].Found\n hit = f }",
    );
    assert!(has_unsupported(&r), "arr[i].Found must not resolve to a port the gate lacks");
}

/// A field naming no port on the object's gate still degrades to a placeholder
/// rather than wiring a port the node never declared: the case-insensitive
/// match widens which SPELLINGS resolve, not which objects carry the port.
#[test]
fn value_on_a_gate_without_that_port_stays_unsupported() {
    let r = compile("in v: vector\nout o: float = v.MagnitudeSq().Value");
    assert!(
        has_unsupported(&r),
        "a `.Value` read of a gate with no Value port must not fabricate a wire"
    );
}

/// `xs.sort(descending = true)` type-checks (`descending` is the parameter's
/// real name) but lowering read only `args.first()` as a POSITIONAL, so the
/// flag was dropped and the array sorted ascending with no diagnostic at all.
/// Provable only through the gate's properties: `--dump-ir` renders the named
/// and positional forms identically.
#[test]
fn sort_named_descending_reaches_the_gate() {
    for call in ["xs.sort(descending = true)", "xs.sort(true)"] {
        let r = compile(&format!(
            "var xs: int[] = [1, 2, 3]\nin go: exec\non go {{ {call} }}"
        ));
        assert_no_errors(&r);
        let node = find_gate(&r, crate::ir::gate_class::ARRAY_SORT);
        let n = &r.module.nodes[&node];
        let baked = n.properties.get(&WirePort::BDescending.sym());
        let wired = r
            .module
            .wires
            .iter()
            .any(|w| w.target == node.port(WirePort::BDescending));
        assert!(
            matches!(baked, Some(Literal::Bool(true))) || wired,
            "`{call}` left bDescending unset (props {:?})",
            n.properties
        );
    }
}

/// `xs.sort()` takes no flag, so the arm above cannot have started setting one
/// unconditionally.
#[test]
fn sort_without_an_argument_leaves_descending_unset() {
    let r = compile("var xs: int[] = [1, 2, 3]\nin go: exec\non go { xs.sort() }");
    assert_no_errors(&r);
    let node = find_gate(&r, crate::ir::gate_class::ARRAY_SORT);
    assert!(
        r.module.nodes[&node]
            .properties
            .get(&WirePort::BDescending.sym())
            .is_none(),
        "a bare sort() must not bake a descending flag"
    );
    assert!(
        !r.module
            .wires
            .iter()
            .any(|w| w.target == node.port(WirePort::BDescending)),
        "a bare sort() must not wire the descending pin"
    );
}

/// `let r = ref someVar` is the documented way to name a reference
/// (`docs/src/expressions.md`). It type-checked and lowered to an
/// `_Unsupported` gate, which emit refuses to spawn: the binding and every read
/// through it were deleted with no error.
#[test]
fn let_of_a_ref_binds_the_variable() {
    for sigil in ["ref ", "&"] {
        let src = format!(
            "var someVar: int = 0\n\
             let r = {sigil}someVar\n\
             in go: exec\n\
             var v: int = 0\n\
             on go {{ v = *r }}\n\
             out o: int = v"
        );
        let r = compile(&src);
        assert_no_errors(&r);
        assert!(
            !has_unsupported(&r),
            "`let r = {sigil}someVar` lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert!(
            wired_between(
                &r,
                "BrickComponentType_WireGraphPseudo_Var",
                WirePort::Value,
                "BrickComponentType_WireGraph_Exec_Var_Set",
                WirePort::Value,
            ),
            "`v = *r` must read someVar's value, not a placeholder"
        );
    }
}

/// A handler whose trigger lowering cannot resolve takes nothing with it but
/// itself. `lower_handler` takes `handler_end_execs` on entry and every exit
/// puts it back, except the unresolved-trigger one, which dropped the exit
/// exec of every PRECEDING handler. The statement after the dead handler then
/// had no chain to attach to, so it was deleted as well, and the only
/// diagnostic (WS058) pointed at the user's line instead of the dead trigger.
#[test]
fn an_unresolvable_trigger_does_not_orphan_the_next_statement() {
    let r = compile(
        "var g: int = 0\n\
         on RoundStart() { g = 1 }\n\
         on notAnEvent { g = 2 }\n\
         g = 3\n",
    );
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS058"),
        "`g = 3` must still chain onto RoundStart's exit: {:?}",
        r.diagnostics
    );
    // `g = 1` and `g = 3`; `g = 2` belongs to the dead handler.
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_Var_Set"),
        2,
        "the surviving handler and the trailing assignment must both lower"
    );
}

/// Typecheck admits ANY `let` binding as a trigger name, so a record-valued
/// one reaches lowering, resolves to no port, and used to delete the whole
/// handler with no diagnostic from either stage.
#[test]
fn a_record_valued_let_used_as_a_trigger_is_reported() {
    let r = compile(
        "var g: int = 0\n\
         let t = { a: 1 }\n\
         on RoundStart() { g = 1 }\n\
         on t { g = 2 }\n\
         g = 3\n",
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS001" && d.severity == crate::diagnostic::Severity::Error),
        "dropping a handler must be reported, not silent: {:?}",
        r.diagnostics
    );
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS058"),
        "`g = 3` must still chain onto RoundStart's exit: {:?}",
        r.diagnostics
    );
}
