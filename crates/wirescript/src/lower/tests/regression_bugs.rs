//! Lowering of aliases, imported names and handlers, record-valued
//! expressions, pseudo-field spellings, and handlers whose trigger cannot
//! resolve.

use super::*;
use crate::ir::gate_class as gc;

/// An alias or a record-valued expression lowers to real gates in every
/// position: no `_Unsupported` placeholder, and exact counts of the named
/// classes over the whole module tree.
#[test]
fn aliases_and_record_valued_expressions_lower_in_every_position() {
    const P: &str = "type P = { x: int, y: int }\n";
    let rows: Vec<(&str, String, &[(&str, usize)])> = vec![
        // A `let` aliasing an array var resolves for `x[i]` (and array methods)
        // exactly like the original.
        ("a let alias of an array var", "var a = [0]\nin go: exec\non go { let ar = a\n let v = ar[0] }".into(), &[(gc::ARRAY_GET, 1)]),
        // A `let` aliasing an INPUT-port array resolves like a `var` alias.
        ("a let alias of an input array", "in a: int[]\nin go: exec\non go { let x = a\n let v = x[0] }".into(), &[(gc::ARRAY_GET, 1)]),
        // A `match` EXPRESSION with record VALUE arms lowers to a per-field Select tree
        // and writes the target's fields, as the statement form does.
        (
            "a match expression with record arms writes the target's fields",
            format!(
                "{P}enum S {{ A, B }}\nvar r: P = {{ x: 0, y: 0 }}\nvar p: P = {{ x: 1, y: 2 }}\nvar q: P = {{ x: 3, y: 4 }}\n\
                 var s: S = S.A\nin go: exec\non go {{ r = match s {{ A => p, B => q }} }}"
            ),
            &[(gc::SELECT, 2), (gc::VAR_SET, 2)],
        ),
        // An enum value is a `__disc` + payload-slot record, so it takes the
        // identical per-leaf Select path: one Select and one write, for `__disc`.
        (
            "a match expression with enum arms writes the target's slots",
            "enum Dir { N, E }\nenum S { A, B }\nvar out1: Dir = Dir.N\nvar d1: Dir = Dir.N\nvar d2: Dir = Dir.E\n\
             var s: S = S.A\nin go: exec\non go { out1 = match s { A => d1, B => d2 } }"
                .into(),
            &[(gc::SELECT, 1), (gc::VAR_SET, 1)],
        ),
        (
            "a record-valued match bound to a let is read by field",
            format!(
                "{P}enum S {{ A, B }}\nvar o: int = 0\nvar p: P = {{ x: 1, y: 2 }}\nvar q: P = {{ x: 3, y: 4 }}\n\
                 var s: S = S.A\nin go: exec\non go {{ let m = match s {{ A => p, B => q }}\n o = m.x }}"
            ),
            &[(gc::SELECT, 2)],
        ),
        // `let m = if c then p else q` binds a per-field record as the assignment form
        // writes one, with no WS071 and no placeholder read.
        (
            "a record-valued if expression binds to a let",
            format!(
                "{P}var p: P = {{ x: 1, y: 2 }}\nvar q: P = {{ x: 3, y: 4 }}\nvar c: bool = false\nvar o: int = 0\n\
                 in go: exec\non go {{ let m = if c then p else q\n o = m.y }}"
            ),
            &[(gc::SELECT, 2)],
        ),
        // A record array push, a record map set, an `out` port, and a `mod`/`chip`
        // argument route through the same shared resolver, so a gap in one is a
        // gap in all.
        (
            "a record-valued match in every other value position",
            format!(
                "{P}enum S {{ A, B }}\nvar p: P = {{ x: 1, y: 2 }}\nvar q: P = {{ x: 3, y: 4 }}\nvar s: S = S.A\n\
                 var arr: P[]\nvar mp: Map<int, P>\nvar g1: int = 0\nvar g2: int = 0\n\
                 mod takeRec(v: P) -> (o: int) {{ return v.x + v.y }}\nchip ChRec(v: P) -> (o: int) {{ out o = v.x - v.y }}\n\
                 in go: exec\non go {{\n  arr.push(match s {{ A => p, B => q }})\n  mp.set(1, match s {{ A => p, B => q }})\n\
                 g1 = takeRec(match s {{ A => p, B => q }})\n  g2 = ChRec(match s {{ A => p, B => q }})\n}}\n\
                 out sum = (match s {{ A => p, B => q }}).x"
            ),
            &[],
        ),
        // A `mod` takes a record argument in every form its `chip` twin does, including
        // container-element and record-valued-conditional spellings: each field read
        // is a real port, not a placeholder off one opaque value.
        (
            "a mod record argument in every value form",
            format!(
                "{P}var p: P = {{ x: 1, y: 2 }}\nvar q: P = {{ x: 3, y: 4 }}\nvar arr: P[]\nvar mp: Map<int, P>\n\
                 var c: bool = false\nvar g1: int = 0\nvar g2: int = 0\nvar g3: int = 0\n\
                 mod takeRec(v: P) -> (o: int) {{ return v.x + v.y }}\nin go: exec\n\
                 on go {{\n  arr.push(p)\n  mp.set(1, p)\n  g1 = takeRec(if c then p else q)\n  g2 = takeRec(arr[0])\n  g3 = takeRec(mp[1])\n}}"
            ),
            &[],
        ),
        // A `mod` with ONE record-typed output hands back the record itself, not a
        // wrapper keyed by the output name (`{o: {x, y}}`), so `r = mk(1)` writes each
        // field and `d.x` reads one. Writes: r.x, r.y, a.
        (
            "a single record output mod unwraps to its fields",
            format!(
                "{P}mod mkP(n: int) -> (o: P) {{ out o = {{ x: n, y: n * 2 }} }}\nvar r: P = {{ x: 0, y: 0 }}\nvar a: int = 0\n\
                 in go: exec\non go {{\n  r = mkP(1)\n  let d = mkP(2)\n  a = d.x\n}}"
            ),
            &[(gc::VAR_SET, 3)],
        ),
        // The arms unwrap, so the choice is still made per leaf field. Writes:
        // r.x, r.y, and the `c = Opaque(true)` probe.
        (
            "a record-returning mod in both conditional arms selects per field",
            format!(
                "{P}mod mkP(n: int) -> (o: P) {{ out o = {{ x: n, y: n * 2 }} }}\nvar r: P = {{ x: 0, y: 0 }}\nvar c: bool = false\n\
                 in go: exec\non go {{\n  c = Opaque(true)\n  r = if c then mkP(1) else mkP(3)\n}}"
            ),
            &[(gc::SELECT, 2), (gc::VAR_SET, 3)],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() || has_unsupported(&r.module) {
            failures.push(format!(
                "{label}: errors {errors:?}, placeholder: {}\n{src}",
                has_unsupported(&r.module)
            ));
            continue;
        }
        for &(class, n) in *want {
            let got = count_class(&r.module, class);
            if got != n {
                failures.push(format!("{label}: {got} x {class}, want {n}\n{src}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
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
        !has_unsupported(&r.module),
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
        &[(
            "lib",
            "on ReadBrickGrid() { BroadcastChatMessage(\"lib\") }",
        )],
    );
    // Both files desugar their non-event `on Foo()` to `let _on_expr_0` with a
    // per-file counter, so the generated names must not raise a cross-file WS013:
    // two libraries that each use an `on <call>()` handler compile together.
    assert_no_errors(&r);
    assert_eq!(
        count_class(&r.module, gc::GAMEMODE_BROADCAST_CHAT),
        2,
        "both the local and the imported handler bodies must generate"
    );
    assert!(
        !has_unsupported(&r.module),
        "imported handler left an _Unsupported node: {:?}",
        r.diagnostics
    );
}

/// Every node id used as a wire SOURCE anywhere in the module tree.
fn wire_sources(m: &crate::ir::Module) -> crate::collections::HashSet<crate::ir::NodeId> {
    m.wires_deep().map(|w| w.source.node_id).collect()
}

/// Root-module var gates carrying `name`, by `NAME_LABEL`.
fn var_ids_named(m: &crate::ir::Module, name: &str) -> Vec<crate::ir::NodeId> {
    let mut ids: Vec<crate::ir::NodeId> = m
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::VAR)
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

/// A MULTI-OUTPUT gate result chosen by an `if` picks each port separately:
/// `m.get(k)` is `{Value, Found}`, so `c.Found` reads a Select over the
/// `bFound` ports, not the value Select. One Select over port 0 would be
/// typecheck-clean, placeholder-free and wrong at runtime.
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
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module), "diags: {:?}", r.diagnostics);
    assert_eq!(
        count_class(&r.module, gc::SELECT),
        2,
        "one Select per output port (Value, Found)"
    );
    // BOTH Selects reach a store: with one shared source, the other Select would
    // drive nothing.
    let selects: Vec<crate::ir::NodeId> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::SELECT)
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
    let feeds: crate::collections::HashSet<_> = r
        .module
        .wires
        .iter()
        .filter(|w| {
            selects.contains(&w.target.node_id)
                && matches!(
                    w.target.port,
                    crate::lower::WirePort::InputA | crate::lower::WirePort::InputB
                )
        })
        .map(|w| w.source)
        .collect();
    assert_eq!(
        feeds.len(),
        4,
        "expected 2 bFound + 2 Value feeds, got {feeds:?}"
    );
}

/// A branching-return `mod` called from a pure position reports at the call
/// site. With no output wire and no chain to continue from, the call has no
/// port to hand back; a wire from the never-allocated `NodeId(0)` would die in
/// emit as `DroppedWire("n0 -> ...")`, naming nothing the author wrote.
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
    // No wire references the `NodeId(0)` sentinel.
    assert!(
        !r.module.wires.iter().any(|w| {
            w.source.node_id == crate::ir::NodeId(0) || w.target.node_id == crate::ir::NodeId(0)
        }),
        "a wire still references the never-allocated node"
    );

    // From an exec context the same mod is fine.
    compile_ok(
        "mod branchy(c: bool) -> (r: int) {\n\
         \x20 if c { return 1 } else { return 2 }\n}\n\
         in go: exec\nvar v: int\non go { v = branchy(true) }\n",
    );
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

/// `arr[i].Value` (the capitalisation the docs teach) reads the array get. A
/// `_Unsupported` placeholder would make emit drop the gate and every wire
/// touching it with no diagnostic, so both the real gate AND the wire into the
/// consumer are asserted.
#[test]
fn array_index_capital_value_reads_the_array_get() {
    for field in ["value", "Value"] {
        let src = format!(
            "var regs: int[]\n\
             var g: int = 0\n\
             in go: exec\n\
             on go {{ let pc = regs[15].{field}\n g = pc }}"
        );
        let r = compile_ok(&src);
        assert!(
            !has_unsupported(&r.module),
            "regs[15].{field} lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert_eq!(
            count_class(&r.module, gc::ARRAY_GET),
            1,
            "regs[15].{field} must lower to one ArrayVar_Get"
        );
        assert!(
            wired_between(
                &r,
                gc::ARRAY_GET,
                WirePort::Value,
                gc::VAR_SET,
                WirePort::Value,
            ),
            "regs[15].{field} must drive `g`'s Value pin, not be left dangling"
        );
    }
}

/// Every capitalisation of the out-of-bounds flag resolves, as `Value` does.
#[test]
fn array_index_out_of_bounds_flag_accepts_every_capitalisation() {
    for field in ["bOutOfBounds", "OutOfBounds", "BOutOfBounds", "outOfBounds"] {
        let src = format!(
            "var regs: int[]\n\
             var oob: bool = false\n\
             in go: exec\n\
             on go {{ let f = regs[15].{field}\n oob = f }}"
        );
        let r = compile_ok(&src);
        assert!(
            !has_unsupported(&r.module),
            "regs[15].{field} lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert!(
            wired_between(
                &r,
                gc::ARRAY_GET,
                WirePort::BOutOfBounds,
                gc::VAR_SET,
                WirePort::Value,
            ),
            "regs[15].{field} must read the gate's bOutOfBounds port"
        );
    }
}

/// Every capitalisation of a var field resolves as a trigger. An unmatched
/// trigger arm would return and delete the whole handler with no diagnostic,
/// while the same field in expression position reports WS010.
#[test]
fn a_var_field_trigger_accepts_every_capitalisation() {
    for field in ["Value", "value", "prev", "Prev", "PREV"] {
        let src = format!(
            "var v: int = 0\n\
             var hit: int = 0\n\
             on v.{field} {{ hit = 1 }}"
        );
        let r = compile_ok(&src);
        assert_eq!(
            count_class(&r.module, gc::VAR_SET),
            1,
            "on v.{field} produced no handler body at all: {:?}",
            r.diagnostics
        );
    }
}

/// Every capitalisation of the map-index found flag resolves, `.Found` (the
/// spelling the docs teach) included; a miss is an `_Unsupported` placeholder
/// that emit deletes silently.
#[test]
fn map_index_found_flag_accepts_every_capitalisation() {
    for field in ["bFound", "Found", "BFound", "found"] {
        let src = format!(
            "var m: Map<int, int>\n\
             var hit: bool = false\n\
             in go: exec\n\
             on go {{ let f = m[1].{field}\n hit = f }}"
        );
        let r = compile_ok(&src);
        assert!(
            !has_unsupported(&r.module),
            "m[1].{field} lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert!(
            wired_between(
                &r,
                gc::MAP_GET,
                WirePort::BFound,
                gc::VAR_SET,
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
    assert!(
        has_unsupported(&r.module),
        "arr[i].Found must not resolve to a port the gate lacks"
    );
}

/// A field naming no port on the object's gate still degrades to a placeholder
/// rather than wiring a port the node never declared: the case-insensitive
/// match widens which SPELLINGS resolve, not which objects carry the port.
#[test]
fn value_on_a_gate_without_that_port_stays_unsupported() {
    let r = compile("in v: vector\nout o: float = v.MagnitudeSq().Value");
    assert!(
        has_unsupported(&r.module),
        "a `.Value` read of a gate with no Value port must not fabricate a wire"
    );
}

/// `let r = ref someVar` is the documented way to name a reference
/// (`docs/src/expressions.md`). It binds the variable; an `_Unsupported` gate
/// would make emit delete the binding and every read through it, silently.
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
        let r = compile_ok(&src);
        assert!(
            !has_unsupported(&r.module),
            "`let r = {sigil}someVar` lowered to _Unsupported: {:?}",
            r.diagnostics
        );
        assert!(
            wired_between(&r, gc::VAR, WirePort::VarRef, gc::VAR_GET, WirePort::VarRef,)
                && wired_between(
                    &r,
                    gc::VAR_GET,
                    WirePort::Value,
                    gc::VAR_SET,
                    WirePort::Value,
                ),
            "`v = *r` must read through someVar's ref, not a placeholder"
        );
        // One chain: the Var_Set's value comes from a Var_Get whose ref is a
        // var other than the one the Var_Set writes.
        let m = &r.module;
        let source = |node: crate::ir::NodeId, port: WirePort| {
            m.wires
                .iter()
                .find(|w| w.target.node_id == node && w.target.port == port)
                .map(|w| w.source.node_id)
        };
        let set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gc::VAR_SET)
            .expect("the write of v")
            .id;
        let get = source(set, WirePort::Value).expect("v's value is wired");
        assert_eq!(m.nodes[&get].gate_class, gc::VAR_GET);
        let read = source(get, WirePort::VarRef).expect("the read names a var");
        let written = source(set, WirePort::VarRef).expect("the write names a var");
        assert_eq!(m.nodes[&read].gate_class, gc::VAR);
        assert_ne!(read, written, "`*r` reads someVar, not v");
    }
}

/// A handler whose trigger lowering cannot resolve takes nothing with it but
/// itself: `lower_handler` puts `handler_end_execs` back on every exit, so the
/// PRECEDING handlers' exit execs survive and the statement after the dead
/// handler still has a chain to attach to.
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
        count_class(&r.module, gc::VAR_SET),
        2,
        "the surviving handler and the trailing assignment must both lower"
    );
}

/// Typecheck admits ANY `let` binding as a trigger name, so a record-valued
/// one reaches lowering and resolves to no port; that is reported, not a silent
/// deletion of the handler.
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

/// The post-lowering wire check reports a stranded wire as WS094 in every
/// build, naming the pass, instead of leaving it to fail at emit or to ship a
/// gate that reads 0.
#[test]
fn a_stranded_wire_is_an_internal_error() {
    let r = compile_ok("in n: int\nout a = n * 2\n");
    let range = crate::diagnostic::SourceRange::default();

    let mut clean = Vec::new();
    check_wires_resolve(&r.module, "a pass", true, &range, &mut clean);
    assert!(
        clean.is_empty(),
        "a lowered program passes the check: {clean:?}"
    );

    let mut broken = r.module.clone();
    let gate = *broken
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == "BrickComponentType_Expr_MathMultiply")
        .expect("the multiply gate")
        .0;
    broken.nodes.remove(&gate);
    let mut diags = Vec::new();
    check_wires_resolve(&broken, "a pass", true, &range, &mut diags);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code, "WS094");
    assert!(
        diags[0].message.contains("after `a pass`"),
        "{}",
        diags[0].message
    );

    // A wire to the never-allocated `NodeId(0)` is exempt only once an error
    // has been reported.
    let mut zero = r.module.clone();
    zero.wires.push(crate::ir::Wire {
        source: crate::ir::NodeId(0).port(WirePort::Output),
        target: zero.outputs[0].port(WirePort::RerInput),
    });
    let mut diags = Vec::new();
    check_wires_resolve(&zero, "a pass", true, &range, &mut diags);
    assert_eq!(
        diags.iter().filter(|d| d.code == "WS094").count(),
        1,
        "{diags:?}"
    );
    let mut diags = vec![crate::diagnostic::Diagnostic::error(
        "WS002",
        "earlier error",
        range.clone(),
    )];
    check_wires_resolve(&zero, "a pass", true, &range, &mut diags);
    assert!(!diags.iter().any(|d| d.code == "WS094"), "{diags:?}");
}

/// `xs.pop()`, `get`, `min`, `max` and `average` read bare are the element
/// (`Value`) and `find` is its `Index`, in typecheck as in lowering: an operator
/// on `fs.pop() + a` is typed by the element, not the `IsEmpty` flag, so `var v`
/// stores the float sum and a `float` `a` is no WS004.
#[test]
fn a_bare_multi_output_array_read_is_typed_by_the_output_it_reads() {
    for (decl, expr, want) in [
        ("var fs: float[]", "fs.pop() + a", Type::Float),
        ("var fs: float[]", "fs.pop() + b", Type::Float),
        ("var fs: float[]", "fs.min() + a", Type::Float),
        ("var fs: float[]", "fs.get(0) + a", Type::Float),
        ("var is: int[]", "is.find(3) + a", Type::Int),
    ] {
        let src = format!(
            "in go: exec\nin a: int\nin b: float\n{decl}\nvar o: string = \"\"\nout oo: string = o\n\
             mod f() {{\n  var v = {expr}\n  o = \"${{v}}\"\n}}\non go {{ f() }}"
        );
        let r = compile_ok(&src);
        let v = r
            .module
            .nodes
            .values()
            .find(|n| n.gate_class == crate::ir::gate_class::VAR && n.source_range.start.line == 8)
            .unwrap_or_else(|| panic!("no `v`\n{src}"));
        assert!(
            v.ports.outputs.iter().any(|p| p.ty == want),
            "{expr}: {:?}",
            v.ports.outputs
        );
    }
}

/// A container receiver that no array or map method arm takes, calling a
/// `self` mod of the method's name, is typed once, so its diagnostics are
/// reported once.
#[test]
fn a_self_mod_on_a_container_receiver_types_the_receiver_once() {
    let e = errs(
        "in go: exec\nvar ms: Map<int, int>[]\nvar n: int = 0\nmod sum(self: Map<int, int>) -> int { return 1 }\n\
         on go { n = ms[nope].sum() }",
    );
    assert_eq!(e.iter().filter(|c| *c == "WS002").count(), 1, "{e:?}");
}

/// A generic builtin's ports take the type its arguments and result pin, so no
/// value port ships as `any` (the float variant) and none is an internal error.
#[test]
fn a_generic_builtin_types_every_port_from_its_arguments_and_result() {
    let head = "in go: exec\nin c: bool\nin a: string\nin b: string\nvar s: string = \"\"\nout os: string = s\n";
    for body in [
        "on go { s = Select(c, a, b) }",
        "on go { s = Swap(c, a, b).OutputB }",
        "let d = Sleep(a, delay = 1.0)\nout od: string = d",
        "let d = SleepTicks(a, delay = 2)\nout od: string = d",
    ] {
        let src = format!("{head}{body}");
        let r = compile_ok(&src);
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS094"),
            "{body}: {:?}",
            r.diagnostics
        );
        let gate = r
            .module
            .nodes
            .values()
            .find(|n| {
                n.gate_class.contains("Select")
                    || n.gate_class.contains("Swap")
                    || n.gate_class.contains("Buffer")
            })
            .unwrap_or_else(|| panic!("{body}: no gate"));
        for p in gate.ports.inputs.iter().chain(&gate.ports.outputs) {
            let name = crate::intern::resolve(p.name);
            if matches!(name, "Input" | "InputA" | "InputB" | "Output") {
                let leaves = match &p.ty {
                    Type::Record(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
                    t => vec![t.clone()],
                };
                assert!(
                    leaves.iter().all(|t| *t == Type::String),
                    "{body}: port {name} is {:?}",
                    p.ty
                );
            }
        }
    }
}
