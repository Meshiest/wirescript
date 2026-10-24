use crate::ir::port_registry::WirePort;
use crate::test_support::{assert_no_code, codes, report_failures};

use super::shape::{assert_no_fan_in, without_names};
use super::*;

// ── Recursion guard (WS020) ──

#[test]
fn const_literal_reaches_into_anon_chip() {
    // Regression: a top-level const `let` referenced inside a `chip on` handler
    // must have its Literal cloned INTO the chip's child module. Otherwise the
    // parent->chip data wire dangles (its target moved into the child during
    // partition_anon_chips) and the chip-side input silently reads the port
    // default (0) — exactly what broke every scalar const inside a `chip on`.
    // K has two consumers (the top-level `out` and the in-chip comparison), so
    // `inline_orphan_literals` (single-consumer only) leaves it a real Literal
    // gate — the case that dangles across the chip boundary.
    let src = "let K: int = 7\nout dummy = K\nin trigger: exec\nchip on trigger { var x: int = 0\n  if x == K { x = 1 } }";
    let r = compile_ok(src);
    let child = only_chip(
        &r,
        "the `chip on` block should produce one anon chip module",
    );
    // The 7 must reach the chip-side comparison — either as a cloned Literal
    // gate wired in, or (since scalar-to-dataless materialization made the EQ
    // the literal's single consumer) as an inlined InputB data default on the
    // CompareEqual node itself. Both deliver the value in-game; what broke
    // originally was the value arriving as NEITHER (dangling cross-boundary
    // wire reading 0).
    let lit7_gate = child.nodes.values().any(|n| {
        n.gate_class == gc::LITERAL
            && n.properties.get(&*crate::intern::sym::VALUE) == Some(&crate::ir::Literal::Int(7))
    });
    let lit7_inlined = child.nodes.values().any(|n| {
        n.gate_class == gc::COMPARE_EQUAL
            && n.properties.get(&crate::intern::intern("InputB"))
                == Some(&crate::ir::Literal::Int(7))
    });
    assert!(
        lit7_gate || lit7_inlined,
        "const `K = 7` must reach the chip body (cloned literal or inlined EQ default)"
    );
}

/// Recursion is a cycle of calls through bodies: a call in an argument of a
/// call to the same mod (`inc(inc(k))`), or in the body of a mod called in
/// such an argument (`f(h(k))` with `h` calling `f`), is lowered at the call
/// site, not in the body, so it is not recursion; a body that reaches its own
/// mod, directly or through others, still is.
#[test]
fn a_nested_call_of_the_same_mod_is_not_recursion() {
    let head = "in go: exec\nvar k: int = 3\nvar n: int = 0\n";
    for (decls, body, recursive) in [
        (
            "mod inc(x: int) -> int { return x + 1 }",
            "n = inc(inc(k))",
            false,
        ),
        (
            "chip C(x: int) -> (r: int) { return x + 1 }",
            "n = C(C(k))",
            false,
        ),
        (
            "mod inc(x: int) -> int { return inc(inc(x)) + 1 }",
            "n = inc(k)",
            true,
        ),
        (
            "mod f(x: int) -> int { return x + 1 }\nmod h(x: int) -> int { return f(x) }",
            "n = f(h(k))",
            false,
        ),
        (
            "mod f(x: int) -> int { return x + 1 }\nchip H(x: int) -> (r: int) { return f(x) }",
            "n = f(H(k))",
            false,
        ),
        (
            "chip F(x: int) -> (r: int) { return x + 1 }\nmod h(x: int) -> int { return F(x) }",
            "n = F(h(k))",
            false,
        ),
        (
            "mod f(x: int) -> int { return x + 1 }\nmod g(x: int) -> int { return f(x) }\nmod h(x: int) -> int { return g(x) }",
            "n = f(h(k))",
            false,
        ),
        (
            "mod a1(x: int) -> int { return b1(x) + 1 }\nmod b1(x: int) -> int { return a1(x) }",
            "n = a1(k)",
            true,
        ),
        (
            "mod g(x: int) -> int { return x * 2 }\nmod f(x: int) -> int { return g(f(x)) + 1 }",
            "n = f(k)",
            true,
        ),
        (
            "chip C(x: int) -> (r: int) { return C(x) + 1 }",
            "n = C(k)",
            true,
        ),
        (
            "mod f(x: int) -> int { if x > 0 { return f(x - 1) + 1 }\nreturn 1 }",
            "n = f(f(k))",
            true,
        ),
    ] {
        let src = format!("{head}{decls}\non go {{ {body} }}");
        let src = src.as_str();
        let r = compile(src);
        let ws020 = r.diagnostics.iter().filter(|d| d.code == "WS020").count();
        assert_eq!(ws020 > 0, recursive, "{src}\n{:?}", r.diagnostics);
        if !recursive {
            assert_no_errors(&r);
            assert_eq!(
                count_class(&r.module, "BrickComponentType_Expr_MathAdd"),
                2,
                "{src}"
            );
        }
    }
}

/// Regression: a local mod calling a SAME-NAMED mod imported from another
/// module (`mod drawCard` calling `card.drawCard`) is not recursion — they're
/// distinct decls. The name-based guard falsely flagged this; keying on the
/// decl's source range (which includes the file) fixes it.
#[test]
fn same_name_mod_across_modules_is_not_recursion() {
    let display = "mod drawCard() -> int { return 1 }";
    let main = "import * as card from \"display\"\n\
                mod drawCard() -> int { return card.drawCard() }\n\
                out result = drawCard()";
    let r = compile_multi(main, &[("display", display)]);
    assert_no_code(&r.diagnostics, "WS020");
    assert_no_errors(&r);
}

#[test]
fn chip_decl_sets_child_root_scope_to_chip_body() {
    // Standalone chips are instantiated per call, so we need to call it.
    let src = "chip Foo(x: int) -> (r: int) { out r = x }\nlet f = Foo(1)";
    let r = compile(src);
    let chip_mod = only_chip(&r, "expected one chip sub-module from call");
    let root = chip_mod
        .scopes
        .get(&crate::ir::ROOT_SCOPE_ID)
        .expect("chip body root scope");
    match &root.kind {
        ScopeKind::ChipBody { name } => assert_eq!(name, "Foo"),
        other => panic!("expected ChipBody, got {:?}", other),
    }
}

/// Each call of a chip is its own instance module, holding one output node per
/// declared output (never duplicated); a `mod` inlines and makes none.
#[test]
fn each_chip_call_is_one_instance_with_its_declared_outputs() {
    // (program, instances, output nodes in each instance, a gate the tree holds)
    let rows: [(&str, usize, usize, Option<&str>); 6] = [
        (
            "chip Add(a: int, b: int) -> (result: int) { out result = a + b }\nlet r1 = Add(1, 2)\nlet r2 = Add(3, 4)",
            2,
            1,
            None,
        ),
        (
            "chip Add(a: int, b: int) -> (result: int) { out result = a + b }\nlet r1 = Add(1, 2)\nlet r2 = Add(3, 4)\n\
             let r3 = Add(5, 6)\nout sum = r1.result + r2.result + r3.result",
            3,
            1,
            None,
        ),
        (
            "chip Double(x: int) -> (result: int) { out result = x * 2 }\nlet r = Double(21)",
            1,
            1,
            None,
        ),
        (
            "chip MinMax(a: int, b: int) -> (lo: int, hi: int) {\n  out lo = if a < b then a else b\n  \
             out hi = if a > b then a else b\n}\nlet mm = MinMax(3, 7)",
            1,
            2,
            None,
        ),
        // Auto-unwrapped single-output results add in the caller.
        (
            "chip Sq(x: int) -> int { out _ = x * x }\nlet a = Sq(3)\nlet b = Sq(4)\nlet y = a + b\nout z = y",
            2,
            1,
            Some("BrickComponentType_Expr_MathAdd"),
        ),
        (
            "var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart() { inc(x) }",
            0,
            0,
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (src, instances, outputs, gate) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let output_nodes: Vec<usize> = r
            .module
            .chips
            .values()
            .map(|c| {
                c.nodes
                    .values()
                    .filter(|n| n.kind == NodeKind::Output)
                    .count()
            })
            .collect();
        if !errors.is_empty()
            || r.module.chips.len() != instances
            || output_nodes.iter().any(|n| *n != outputs)
            || gate.is_some_and(|g| !has_gate(&r, g))
        {
            failures.push(format!(
                "{src}\n  errors {errors:?}, {} instances (want {instances}), output nodes {output_nodes:?} (want {outputs})",
                r.module.chips.len()
            ));
        }
    }
    report_failures(&failures, rows.len());
}

#[test]
fn inline_mod_multi_output_fields_wire_to_sources() {
    // Regression: `let s = M(...); s.p / s.q` on a multi-output inline mod used
    // to bind `s` to a phantom node 0 (ExecOut), leaving the outputs
    // disconnected. Each field must resolve to its real value source.
    let src = "in a: float\n\
        mod M(v: float) -> (p: float, q: float) {\n  out p = v + 1.0\n  out q = v + 2.0\n}\n\
        let s = M(a)\nout x = s.p\nout y = s.q";
    let r = compile_ok(src);
    // No wire may originate from the phantom node 0.
    let phantom = r.module.wires.iter().any(|w| w.source.node_id == NodeId(0));
    assert!(!phantom, "multi-output mod read produced a phantom n0 wire");
    // Every output node must be fed by an incoming wire.
    for oid in &r.module.outputs {
        assert!(
            r.module.wires.iter().any(|w| w.target.node_id == *oid),
            "output node {oid:?} has no incoming wire"
        );
    }
}

#[test]
fn chip_call_wires_args_to_inputs() {
    let src = "in x: int\nin y: int\nchip Add(a: int, b: int) -> (result: int) { out result = a + b }\nlet r = Add(x, y)";
    let r = compile_ok(src);
    // Parent module should have wires to child MicrochipInput nodes
    let child = r.module.chips.values().next().unwrap();
    let child_input_ids: crate::collections::HashSet<NodeId> =
        child.inputs.iter().cloned().collect();
    let input_wires: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| child_input_ids.contains(&w.target.node_id))
        .collect();
    assert_eq!(
        input_wires.len(),
        2,
        "should wire each arg directly to its child MicrochipInput node: {input_wires:?}"
    );
}

#[test]
fn chip_call_atom_arg_inlines_like_int_literal() {
    // A `:atom` chip-call argument is an int at heart (typechecks as
    // `Type::Int`) and must inline into the instance the same way a bare int
    // literal does (`const_arg_literal`'s `AtomLit` arm) instead of wiring a
    // real gate across the chip boundary.
    let atom_src = "chip Double(x: int) -> (result: int) { out result = x * 2 }\nlet r = Double(:red)\nout val = r.result";
    let int_src = "chip Double(x: int) -> (result: int) { out result = x * 2 }\nlet r = Double(21)\nout val = r.result";
    for src in [atom_src, int_src] {
        let r = compile_ok(src);
        let child = r.module.chips.values().next().unwrap();
        let child_input_ids: crate::collections::HashSet<NodeId> =
            child.inputs.iter().cloned().collect();
        let input_wires: Vec<_> = r
            .module
            .wires
            .iter()
            .filter(|w| child_input_ids.contains(&w.target.node_id))
            .collect();
        assert!(
            input_wires.is_empty(),
            "a constant chip-call arg must fold inside the instance, not wire across the \
             boundary (src: {src:?}); wires: {input_wires:?}"
        );
    }
}

#[test]
fn chip_call_output_wire_in_parent() {
    let src = "chip Double(x: int) -> (result: int) { out result = x * 2 }\nlet r = Double(21)\nout val = r.result";
    let r = compile_ok(src);
    // Parent should have a wire from child MicrochipOutput to the out binding
    let child = r.module.chips.values().next().unwrap();
    let child_output_ids: crate::collections::HashSet<NodeId> =
        child.outputs.iter().cloned().collect();
    let out_wires: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| child_output_ids.contains(&w.source.node_id))
        .collect();
    assert_eq!(
        out_wires.len(),
        1,
        "one wire from the child MicrochipOutput: {out_wires:?}"
    );
}

#[test]
fn inline_literals_folded() {
    let r = compile_ok("var x: int = 0\non RoundStart() { x = x + 1 }");
    assert_eq!(
        count_class(&r.module, gc::LITERAL),
        0,
        "_Literal nodes should be inlined into consumer properties"
    );
}

#[test]
fn chip_output_named_like_a_swizzle_is_not_split() {
    // A chip output called `x`/`y`/`z` (or `r`/`g`/`b`/`a`) collides with the
    // vector/color component names. Reading it must wire the chip's OUTPUT
    // through, not lower to a SplitVector that splits the (int) value as if it
    // were a vector and silently reads a garbage component.
    let r = compile_ok(
        "in a: int\nchip F(n: int) -> (y: int) { out y = n * 2 + 1 }\nlet c = F(a)\nout r = c.y",
    );
    assert_eq!(
        count_class(&r.module, gc::SPLIT_VECTOR),
        0,
        "a chip output named `y` must read the output, not lower to a SplitVector"
    );

    // ...but a genuine vector swizzle must still split.
    let v = compile_ok("in v: vector\nlet p = v\nout ox = p.x\nout oy = p.y");
    assert!(
        count_class(&v.module, gc::SPLIT_VECTOR) == 1,
        "a real vector component access must still lower to a SplitVector"
    );
}

#[test]
fn nested_chip_instance_exec_wire_stays_local() {
    // A `chip` exec-called from INSIDE a nested anon chip must have its exec
    // wire routed into that anon chip's module (one grid boundary from the
    // instance), not left at the root spanning several grids. The game can
    // route a wire one grid in, but an exec pulse can't cross into an instance
    // grid nested inside another anon chip, so the called chip would silently
    // never fire. Regression for the Secret Hitler chancellor-enact bug
    // (applyEnact never ran; discard did).
    let src = "\
var x: int = 0
var y: int = 0
in trig: exec
chip setX() { x = 1 }
mod doPick() { y = 2 setX() }
chip on trig {
  chip {
    doPick()
  }
}";
    let r = compile_ok(src);

    // Pins live in child modules, so classes come from the whole tree.
    let classes: crate::collections::HashMap<crate::ir::NodeId, &'static str> = r
        .module
        .nodes_deep()
        .map(|n| (n.id, n.gate_class))
        .collect();
    let is_pin = |id: &crate::ir::NodeId| {
        matches!(
            classes.get(id).copied(),
            Some(gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT)
        )
    };

    // In each module, a wire touching a chip-boundary pin that isn't one of the
    // module's OWN pins must have that pin on a DIRECT child chip.
    fn check(m: &crate::ir::Module, is_pin: &dyn Fn(&crate::ir::NodeId) -> bool) {
        let mut child_pins: crate::collections::HashSet<crate::ir::NodeId> = Default::default();
        for c in m.chips.values() {
            child_pins.extend(c.inputs.iter().copied());
            child_pins.extend(c.outputs.iter().copied());
        }
        for w in &m.wires {
            if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
                continue;
            }
            for ep in [w.source.node_id, w.target.node_id] {
                if is_pin(&ep) && !m.nodes.contains_key(&ep) {
                    assert!(
                        child_pins.contains(&ep),
                        "module {:?}: wire touches boundary pin {ep} not on a direct \
                         child chip — a multi-grid exec wire the instance can't receive",
                        crate::intern::resolve(m.name)
                    );
                }
            }
        }
        for c in m.chips.values() {
            check(c, is_pin);
        }
    }
    check(&r.module, &is_pin);
}

#[test]
fn dedup_keeps_constants_within_their_chip() {
    // A constant used as a WIRED (multi-consumer) literal in two separate anon
    // chips must NOT be merged across the chip boundary. `dedup_constant_gates`
    // runs before `partition_anon_chips`, so merging to a keeper in a different
    // chip leaves the other chip's consumer with a cross-chip literal data wire;
    // partition keeps that wire in the parent without cloning the literal, and
    // emit (which inlines _Literal sources per-module) can't reach it, so the
    // operand silently reads its port default (0). This manifested as seat
    // indices 7/9 folding to 0 in the Secret Hitler input queue. `i` is used
    // twice so the arg literal stays wired (single-use args inline earlier and
    // never reach dedup).
    let src = "\
var q: int[]
in trig: exec
mod enc(i: int) { q.push(i * 4) q.push(i) }
on trig {
  chip { enc(6) enc(7) }
  chip { enc(7) enc(9) }
}";
    let r = compile_ok(src);
    assert_literal_operands_in_their_module(&r.module);
}

#[test]
fn let_constant_referenced_across_chips_is_cloned_per_module() {
    // A `let`-bound constant declared in one chip and referenced in ANOTHER
    // chip (or at the top level) shares a single `_Literal` value gate by
    // reference — unlike the mod-inlined case above, dedup never runs on it.
    // `partition_anon_chips` must clone that literal into EVERY consuming
    // module (chip->chip AND chip->parent), because a `_Literal` is a compiler
    // placeholder emit only inlines into SAME-module consumers; a cross-module
    // literal->operand wire leaves the far operand reading its default 0.
    // (In a real capture-point circuit `let capTotal = 20` in one chip read 0
    // in the `capAmount < capTotal` of another chip.)
    let src = "\
chip {
  let k = 20
  let ka = k * 3
}
chip {
  let kd = k * 9
}
out a: int = ka
out d: int = kd
out b: int = k * 5";
    let r = compile_ok(src);
    assert_literal_operands_in_their_module(&r.module);
}

/// Every `_Literal` feeding an operand port (InputA/InputB) lives in its
/// consumer's module: emit inlines a literal only into same-module consumers, so
/// a literal wired across a chip boundary leaves the operand reading 0.
fn assert_literal_operands_in_their_module(root: &crate::ir::Module) {
    let mut module_of: crate::collections::HashMap<crate::ir::NodeId, usize> = Default::default();
    for (i, m) in root.modules_deep().into_iter().enumerate() {
        module_of.extend(m.nodes.keys().map(|id| (*id, i)));
    }
    let is_literal = |id: &crate::ir::NodeId| {
        root.find_deep(*id)
            .is_some_and(|n| n.gate_class == gc::LITERAL)
    };
    for w in root.wires_deep() {
        if matches!(w.target.port, WirePort::InputA | WirePort::InputB)
            && is_literal(&w.source.node_id)
        {
            assert_eq!(
                module_of.get(&w.source.node_id),
                module_of.get(&w.target.node_id),
                "constant operand wire crosses a chip boundary ({:?} -> {:?}.{:?}); emit can't inline it and the operand reads 0",
                w.source.node_id,
                w.target.node_id,
                w.target.port
            );
        }
    }
}

#[test]
fn pure_chip_after_handler_reads_vars_continuously() {
    // A top-level `chip { }` is a pure visual grouping. When it FOLLOWS an `on`
    // handler, it must NOT inherit the post-handler exec context — otherwise its
    // pure `let` var reads latch onto that exec chain as a one-shot
    // `Exec_Var_Get` frozen at the var's init value, instead of reading the
    // var's live `.Value`. (Symptom: a display chip's computed value stuck at 0
    // even as the var updates — capTotal/timePercent-style.)
    for src in [
        "var v: int = 0
in go: exec
on go { v = 5 }
chip {
  let x = v + 1
}
out r: int = x",
        // The chip's own `out` reads the `let` too.
        "var src: int = 0
in go: exec
on go { src = 1 }
chip { let doubled = src * 2
out d = doubled }
",
    ] {
        let r = compile_ok(src);
        // The only var READ is inside the pure chip; it must lower to the
        // var's continuous `.Value`, never a latched `Exec_Var_Get`.
        assert_eq!(
            count_class(&r.module, gc::VAR_GET),
            0,
            "a pure chip's var read latched into an Exec_Var_Get:
{src}"
        );
    }
}

#[test]
fn namespace_chip_call_resolves() {
    let lib_src = "chip Double(x: int) -> (result: int) { out result = x + x }";
    let main_src = "import * as math from \"lib\"
let r = math.Double(5)
out result = r.result";
    let loader = crate::resolve::MemLoader {
        files: [("lib.ws".to_string(), lib_src.to_string())]
            .into_iter()
            .collect(),
    };
    // Unfolded, so the constant chip body is not elided before the instance count.
    let crate::compile::FrontEnd {
        resolved,
        typecheck: tc,
        lowered: lr,
        ..
    } = crate::compile::front_end(main_src, "test", None, FoldMode::ForceOff, &loader);
    crate::test_support::assert_no_diags(&resolved.diagnostics);
    let errors = [
        crate::test_support::error_codes(&tc.diagnostics),
        crate::test_support::error_codes(&lr.diagnostics),
    ];
    assert!(errors.iter().all(|e| e.is_empty()), "{errors:?}");
    assert_eq!(
        lr.module.chips.len(),
        1,
        "namespace chip should create one chip instance"
    );
}

#[test]
fn unused_local_mod_no_warning() {
    let r = compile(
        "mod foo(x: *int) { x = 1 }\nvar y: int = 0\nin player: character\non player { y = 2 }",
    );
    assert_no_code(&r.diagnostics, "WS014");
}

/// Inline mod outputs must not contaminate the parent's: with an output named
/// `key_state` in both, the mod's inline cleanup keeps the parent's one output,
/// read whole or as `.key_state` on the auto-unwrapped result, and it emits.
#[test]
fn inline_mod_output_named_like_the_parents_keeps_one_parent_output() {
    for read in ["pad", "pad.key_state"] {
        let r = compile_ok(&format!(
            "mod Gamepad(a: bool) -> (key_state: int) {{\n  return if a then 0x3FE else 0x3FF\n}}\n\
             let pad = Gamepad(true)\nout key_state = {read}"
        ));
        assert_eq!(r.module.outputs.len(), 1, "{read}: the parent's outputs");
        let output_nodes = r
            .module
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Output)
            .count();
        assert_eq!(output_nodes, 1, "{read}: output nodes after inline cleanup");
        assert_emits(&r.module, &crate::layout::layout(&r.module));
    }
}

/// An `if` on a literal condition, or on a local bound to one (an inline mod
/// parameter given `true`), folds: no Branch gate, and only the chosen body
/// writes.
#[test]
fn an_if_on_a_literal_condition_folds_to_the_chosen_body() {
    let rows = [
        (
            "var x: int = 0\nin tick: exec\non tick { if true { x = 1 } }",
            1,
        ),
        (
            "var x: int = 0\nin tick: exec\non tick { if false { x = 1 } else { x = 2 } }",
            2,
        ),
        (
            "mod foo(x: *int, cond: bool) { if cond { x = 1 } }\nvar v: int = 0\nin tick: exec\non tick { foo(v, true) }",
            1,
        ),
    ];
    let mut failures = Vec::new();
    for (src, written) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let sets: Vec<_> = nodes_of(&r.module, gc::VAR_SET)
            .into_iter()
            .map(|n| n.id)
            .collect();
        let branches = count_class(&r.module, gc::BRANCH);
        if !errors.is_empty()
            || branches != 0
            || sets.len() != 1
            || !stores_value(&r.module, sets[0], written)
        {
            failures.push(format!("{src}\n  errors {errors:?}, {branches} branches, {} sets, want one writing {written}", sets.len()));
        }
    }
    report_failures(&failures, rows.len());
}

/// Record params: ref/array fields are NOT declared MicrochipInput ports
/// under their record-field name (no `s_val`/`s_arr` labels); the crossing
/// each dissolved field leaves behind is resolved by the boundary-pins pass
/// into its own synthesized pin, leaving no raw external capture.
#[test]
fn chip_record_param_dissolves_to_ports() {
    let r = compile_ok(
        "\
type State = { val: *int, arr: int[] }
var v: int = 0
var a: int[]
chip Foo(s: State) -> (result: int) {
  in run: exec
  on run { s.arr.push(s.val) }
  out result = 0
}
let s: State = { val: v, arr: a }
let r = Foo(s)",
    );
    assert_eq!(r.module.chips.len(), 1, "should create one chip instance");
    let chip = r.module.chips.values().next().unwrap();
    let input_labels: Vec<_> = chip
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Input)
        .filter_map(|n| {
            n.properties
                .get(&crate::intern::intern("PortLabel"))
                .and_then(|l| {
                    if let Literal::String(s) = l {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
        })
        .collect();
    assert!(
        !input_labels.contains(&"s_val".to_string()),
        "s_val (*int) should be scope-captured, not a MicrochipInput, got: {:?}",
        input_labels
    );
    assert!(
        !input_labels.contains(&"s_arr".to_string()),
        "s_arr (int[]) should be scope-captured, not a MicrochipInput, got: {:?}",
        input_labels
    );
    assert!(
        chip.scope_captures.is_empty(),
        "ref/array field crossings are resolved through boundary pins, leaving no raw capture"
    );
    let boundary_pin_count = chip
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT && n.note == Some("boundary_pin"))
        .count();
    assert_eq!(
        boundary_pin_count, 2,
        "the val and arr fields each get their own synthesized boundary pin"
    );
}

/// `&x` (address-of) passed to a `*T` param must bind the SAME var as bare `x`,
/// so a write through the param reaches the caller's var. The `&` form (the
/// documented `inc(&x)` idiom) needs the inline ref-binding to unwrap
/// `Expr::RefOf`, or the param binds to nothing and every write is dropped.
#[test]
fn addr_of_arg_binds_ref_param_so_write_lands() {
    let r = compile_ok(
        "\
var x: int = 0
mod inc(v: *int) { v = v + 1 }
in go: exec
on go { inc(&x) }",
    );
    assert!(
        !has_unsupported(&r.module),
        "the ref param must bind, not fall to an _Unsupported read"
    );
    // The `v = v + 1` write must be wired to x's storage gate (its VarRef).
    let x_var = find_gate(&r, gc::VAR);
    let writes = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == x_var && w.source.port == WirePort::VarRef)
        .count();
    assert_eq!(
        writes, 1,
        "the write through `&x` must target x's storage gate, not vanish"
    );
}

/// The chip twin of `addr_of_arg_binds_ref_param_so_write_lands`. A chip
/// resolves a ref/container argument in `call/binding.rs` and `call/wiring.rs`,
/// neither of which saw through `Expr::RefOf`, so `&x` fed the ref pin nothing
/// and every write through the param was dropped with no diagnostic.
#[test]
fn addr_of_arg_binds_chip_ref_param() {
    for arg in ["x", "&x", "ref x"] {
        let src = format!(
            "var x: int = 0
chip inc(v: *int) {{ v = v + 1 }}
in go: exec
on go {{ inc({arg}) }}"
        );
        let r = compile_ok(&src);
        assert_eq!(r.module.chips.len(), 1, "`inc({arg})`: one instance");
        let x_var = find_gate(&r, gc::VAR);
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.source.node_id == x_var && w.source.port == WirePort::VarRef),
            "`inc({arg})` left the chip ref pin unfed, so the write vanished"
        );
    }
}

/// Same for the container and record forms: `&arr` and `&rec` must bind the
/// caller's storage exactly as the bare spellings do.
#[test]
fn addr_of_arg_binds_chip_container_and_record_params() {
    let arr = compile_ok(
        "var a: int[]
chip push1(xs: int[]) { xs.push(1) }
in go: exec
on go { push1(&a) }",
    );
    let a_var = find_gate(&arr, gc::ARRAY_VAR);
    assert!(
        arr.module.wires.iter().any(|w| w.source.node_id == a_var),
        "`push1(&a)` left the array ref pin unfed"
    );

    let rec = compile_ok(
        "type P = { x: int, y: int }
chip setx(s: *P, v: int) { s.x = v }
         var p: P = { x: 0, y: 0 }
in go: exec
on go { setx(&p, 1) }",
    );
    let wired = rec
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR)
        .filter(|n| rec.module.wires.iter().any(|w| w.source.node_id == n.id))
        .count();
    // Only the written field `p.x` is wired.
    assert_eq!(
        wired, 1,
        "`setx(&p, 1)` left `p.x` unwired, so the write vanished"
    );
}

#[test]
fn unassigned_output_warning() {
    // A bare `x + 1` statement assigns nothing, so `result` has no driver and
    // `let b` reads a port that is never set.
    let r = compile(
        "\
mod bad(x: int) -> (result: int) {
  x + 1
}
in player: character
on player {
  let b = bad(5)
}",
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS013" && d.message.contains("'result' is never assigned")),
        "an unwritten output must be reported: {:?}",
        r.diagnostics
    );
    // Writing it clears the diagnostic.
    compile_ok(
        "\
mod good(x: int) -> (result: int) {
  return x + 1
}
in player: character
on player {
  let b = good(5)
}",
    );
}

/// Regression: chip value params that require exec (Var_Get) must be read
/// BEFORE the chip's auto-exec entry, not after. Otherwise the exec chain
/// creates a cycle: chip._exec_out -> Var_Get -> ... -> chip.param (data back-edge).
#[test]
fn chip_value_args_exec_before_chip_entry() {
    let src = "\
var a: int = 0
var b: int = 0
chip Add(x: int, y: int) -> (result: int) { out result = x + y }
chip Inc(v: *int) { v = v + 1 }
in start: exec
on start {
  Inc(a)
  Inc(b)
  let r = Add(a, b)
}";
    let r = compile_ok(src);

    // Find the Add chip's child module (uniquely has a MathAdd body gate).
    let (add_chip_id, add_child) = r
        .module
        .chips
        .iter()
        .find(|(_, child)| {
            child
                .nodes
                .values()
                .any(|n| n.gate_class == "BrickComponentType_Expr_MathAdd")
        })
        .expect("should find Add chip");
    assert_eq!(
        add_child.outputs.len(),
        2,
        "`result` and the auto `_exec_out`"
    );

    // Exec goes to child's _exec_in MicrochipInput (last input)
    let exec_in_node = *add_child.inputs.last().unwrap();
    let exec_into_chip = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == exec_in_node && w.target.port == WirePort::RerInput)
        .expect("child _exec_in MicrochipInput should have a wire");

    // The exec comes from the arguments' reads, not the handler entry or a
    // chip node (which would mean exec fires before reads).
    let source_class = r
        .module
        .nodes
        .get(&exec_into_chip.source.node_id)
        .expect("exec source node should exist")
        .gate_class;
    assert_eq!(
        source_class,
        gc::VAR_GET,
        "exec into chip should come from a Var_Get (args first)"
    );

    // Verify no layout cycle
    let layout = crate::layout::layout(&r.module);
    assert!(
        layout.placements.contains_key(add_chip_id),
        "chip node should have a valid layout placement (no cycles)"
    );
}

#[test]
fn map_var_emits_brz() {
    // The MapVar brick serializes a `dynamic_map`; this
    // exercises the full map pipeline through emit for several key/value kinds.
    let src = r#"
in t: exec
var e: entity
var scores: Map<string, int>
var byId: Map<int, entity>
var pos: Map<string, vector>
on t {
  scores.set("a", 1)
  let g = scores.get("a")
  if g.Found { PrintToConsole("${g.Value}") }
  byId.set(1, e)
  pos.set("spawn", Vec(0.0, 0.0, 0.0))
}"#;
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "map methods lowered to _Unsupported"
    );
    assert_emits(&r.module, &crate::layout::layout(&r.module));
}

#[test]
fn map_get_with_exec_arg_lowers_in_pure_context() {
    // `m.get(k, exec = <trigger>)` in a PURE binding drives the read off an
    // explicit trigger (mirrors the array `lut.get(i, exec = …)` path), so it must
    // lower to a real Map_Get gate — not the `_Unsupported` placeholder.
    // `Change(k).exec` is a bare-exec builtin result used as the trigger — the
    // same shape the user hit — so this also guards the `.exec`-on-exec lowering.
    let src = "\
var teams: Map<string, string> = { \"red\" => \"f00\" }
in k: string
let color = teams.get(k, exec = Change(k).exec)
out status: string = color";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "pure-context map get with exec= must not lower to _Unsupported"
    );
    assert_eq!(
        count_class(&r.module, gc::MAP_GET),
        1,
        "expected a Map_Get gate for the pure-context map get"
    );
}

#[test]
fn emit_value_inside_chip_handler_assigns_outputs() {
    // Init-style chips mint values in an `on` handler and emit them into
    // their outputs. The WS013 unassigned-output check must count `emit`
    // (including nested in handlers), and lowering must accept the shape.
    compile_ok(
        "chip Init(t: exec) -> (code: int, done: exec) {\n  on t {\n    emit code = 7\n    emit done\n  }\n}\nin s: exec\nlet r = Init(s)\nout v = r.code",
    );
}

/// A chip body's push runs on the exec that drives the body: its exec param's
/// `on init { }` handler (the param does not invoke the body by itself), or an
/// `exec =` named arg outside an exec context. The push is exec-wired in the
/// child module and the caller wires the body's exec boundary input.
#[test]
fn a_chip_body_push_is_driven_by_the_exec_given_to_the_body() {
    let tables =
        "var names: string[]\ntype Tables = { names: string[] }\nlet TB: Tables = { names }\n";
    let rows = [
        format!(
            "{tables}chip Init(init: exec, tables: Tables) -> (code: int) {{\n  on init {{\n    tables.names.push(\"a\")\n    \
             emit code = 5\n  }}\n}}\nin s: exec\nlet r = Init(s, TB)\nout v = r.code"
        ),
        format!(
            "{tables}chip Init(tables: Tables) -> (code: int) {{\n  tables.names.push(\"a\")\n  emit code = 5\n}}\n\
             in s: exec\nlet r = Init(TB, exec = s)\nout v = r.code"
        ),
        "var names: string[]\nvar count: int = 0\nchip Init() -> (code: int) {\n  names.push(\"a\")\n  count = count + 1\n  \
         emit code = 7\n}\nin s: exec\nlet r = Init(exec = s)\nout v = r.code"
            .to_string(),
    ];
    let mut failures = Vec::new();
    for src in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let Some(child) = r
            .module
            .chips
            .values()
            .next()
            .filter(|_| r.module.chips.len() == 1)
        else {
            failures.push(format!(
                "{src}\n  {} instances, errors {errors:?}",
                r.module.chips.len()
            ));
            continue;
        };
        let pushes = nodes_of(child, gc::ARRAY_PUSH);
        let push_wired =
            pushes.len() == 1 && child.wires.iter().any(|w| w.target.node_id == pushes[0].id);
        let exec_in_wired = r
            .module
            .wires
            .iter()
            .any(|w| child.inputs.contains(&w.target.node_id));
        if !errors.is_empty() || !push_wired || !exec_in_wired {
            failures.push(format!(
                "{src}\n  errors {errors:?}, {} pushes, push wired {push_wired}, boundary input wired {exec_in_wired}",
                pushes.len()
            ));
        }
    }
    report_failures(&failures, rows.len());
}

#[test]
fn entity_setters_compile_to_brz() {
    // Entity Set*/Teleport gates derive their data structs (Vector/Rotator
    // and composite TeleportDestination fields) — the writer must fill
    // defaults for all of them without failing the emit.
    let src = "in trigger: exec\nin e: entity\nin d: teleport\non trigger {\n  \
        e.SetLocation(Vec(1.0, 2.0, 3.0))\n  \
        e.SetRotation(Rotation(0.0, 90.0, 0.0))\n  \
        e.SetVelocity(Vec(0.0, 0.0, 10.0))\n  \
        e.Teleport(d)\n}";
    let r = compile_ok(src);
    assert_emits(&r.module, &crate::layout::layout(&r.module));
}

#[test]
fn entity_setter_embeds_vector_literal() {
    // Set* gates store unwired input values in their data structs, so folded
    // Vec/Rotation literals embed directly instead of spawning a
    // MakeVector/MakeRotation wired into the port.
    let src = "in trigger: exec\nin e: entity\non trigger {\n  \
        e.SetLocation(Vec(1.0, 2.0, 3.0))\n  \
        e.SetRotation(Rotation(0.0, 90.0, 0.0))\n}";
    let r = compile_ok(src);
    for class in [gc::MAKE_VECTOR, gc::MAKE_ROTATION] {
        assert!(
            !has_gate_deep(&r, class),
            "literal args should embed as gate data, not spawn {class}"
        );
    }
    let set_loc = find_gate(&r, gc::ENTITY_SET_LOCATION);
    assert_eq!(
        r.module.nodes[&set_loc]
            .properties
            .get(&crate::intern::intern("Vector")),
        Some(&Literal::Vector {
            x: 1.0,
            y: 2.0,
            z: 3.0
        }),
        "the folded vector should ride the gate's data properties"
    );
    // and the emit path serializes the embedded f64 struct values
    assert_emits(&r.module, &crate::layout::layout(&r.module));
}

#[test]
fn spawn_prefab_compiles_to_brz() {
    // The PrefabSpawner data struct has an array field (SpawnedEntityIds)
    // wirescript never populates - LiteralComponent must report it as
    // missing (serialized as an empty array), not fail the whole emit.
    // Covers both the bare call and the full argument set.
    let src = "in trigger: exec\non trigger {\n  let car = SpawnPrefab()\n  \
        let boat = SpawnPrefab(offset = Vec(0.0, 0.0, 50.0), rotation = Rotation(0.0, 90.0, 0.0), velocity = Vec(0.0, 0.0, 100.0), lifetime = 10.0, limit = 5)\n}";
    let r = compile_ok(src);
    assert_emits(&r.module, &crate::layout::layout(&r.module));
}

/// Assert a compiled `SpawnPrefab` call has no `_Unsupported` placeholder
/// anywhere in the module, and its `Prefab` gate property was baked to the
/// given path via `Literal::PrefabRef` — i.e. the `prefab = <name>` config
/// arg resolved the `let`-bound name to its constant instead of falling
/// through to an unresolvable wire.
fn assert_prefab_property_baked(r: &LowerResult, expected_path: &str) {
    assert_no_errors(r);
    assert!(
        !has_unsupported(&r.module),
        "no _Unsupported placeholder gate should be emitted: {:?}",
        r.module.nodes
    );
    let spawner = find_gate(r, gc::PREFAB_SPAWNER);
    let node = &r.module.nodes[&spawner];
    let prop = node
        .properties
        .get(&crate::intern::intern(WirePort::Prefab.as_str()));
    assert_eq!(
        prop,
        Some(&Literal::PrefabRef {
            path: expected_path.to_string()
        }),
        "SpawnPrefab's Prefab property should be baked from the `let`-bound name: {:?}",
        node.properties
    );
}

#[test]
fn spawn_prefab_top_level_let_reaches_prefab_property() {
    // A top-level `let pf = $./foo.brz` referenced later as `prefab = pf`
    // must bake the SAME way a literal `prefab = $./foo.brz` argument does:
    // `literal_for_property_port` must fold the name through the const env.
    let src = "let pf = $./foo.brz\nin go: exec\non go { SpawnPrefab(prefab = pf) }";
    let r = compile(src);
    assert_prefab_property_baked(&r, "./foo.brz");
}

#[test]
fn spawn_prefab_body_local_let_reaches_prefab_property() {
    // A BODY-LOCAL `let pf = $./foo.brz` (declared inside the handler that
    // uses it) is the scope-aware case: `pf` only exists in the handler's
    // block scope, so resolving it requires the lowering-side
    // `scoped_consts` frame stack (mirroring typecheck's), not just the
    // top-level `const_env`. Before that stack existed, `pf` didn't resolve
    // as a constant — and `Expr::PrefabRef` has no general expression
    // lowering of its own (`lower_expr` falls through to
    // `synthesise_unsupported` for it) — so the `let` itself emitted an
    // `_Unsupported` placeholder plus a warning, and the `prefab=` arg had
    // no way to reach the gate's `Prefab` property.
    let src = "in go: exec\non go { let pf = $./foo.brz\n  SpawnPrefab(prefab = pf) }";
    let r = compile(src);
    assert_prefab_property_baked(&r, "./foo.brz");
}

#[test]
fn spawn_prefab_destroy_all_wires_a_local_exec_signal() {
    // A local `let sig: exec` fed to `destroyAll =` must WIRE into the spawner's
    // DestroyAll pin. The signal is parser-desugared to `= 0`; if that `0` leaks
    // into the const env, `destroyAll = sig` folds the name to `0` and inlines it
    // as a config property, emitting NO wire (the spawned prefabs are then never
    // destroyed).
    let src = "in fire: exec\nin go: exec\nlet sig: exec\nvar made: int = 0\n\
               on fire { emit sig }\n\
               on go { let e = SpawnPrefab(prefab = $./foo.brz, destroyAll = sig)\n  made = made + 1 }";
    let r = compile_ok(src);
    let spawner = find_gate(&r, gc::PREFAB_SPAWNER);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == spawner && w.target.port == WirePort::DestroyAll),
        "destroyAll = <local signal> must wire into the DestroyAll pin: {:?}",
        r.module.wires
    );
    assert!(
        !r.module.nodes[&spawner]
            .properties
            .contains_key(&crate::intern::intern(WirePort::DestroyAll.as_str())),
        "DestroyAll must be wired, not baked as a constant property"
    );
}

#[test]
fn exec_arg_call_exposes_exec_field() {
    // A chip call with `exec =` also returns the chip's completion exec as an
    // `exec` record field: `let r = Init(TB, exec = s)` ... `on r.exec { }`.
    let src = "var names: string[]\ntype Tables = { names: string[] }\nlet TB: Tables = { names }\nchip Init(tables: Tables) -> (code: int) {\n  tables.names.push(\"a\")\n  emit code = 5\n}\nin s: exec\nlet r = Init(TB, exec = s)\nvar hit: int = 0\non r.exec { hit = hit + 1 }\nout v = r.code";
    let r = compile_ok(src);
    let child = only_chip(&r, "chip instance should produce a child module");
    let exec_out = *child
        .outputs
        .last()
        .expect("child should have the _exec_out boundary output");
    // The handler must be driven from the boundary output in the parent.
    let exec_wired = r.module.wires.iter().any(|w| w.source.node_id == exec_out);
    assert!(
        exec_wired,
        "on r.exec should wire from the chip's _exec_out"
    );
}

// ── Use-before-declaration guard (WS021) ──

/// A body reading a top-level `let` declared below its call, or a call before
/// its mod's declaration, is WS021 once per read or call (it would lower to an
/// `_Unsupported` reading 0). The same programs declared first, and a call of
/// a name that is no chip or mod, are not.
#[test]
fn a_use_before_its_declaration_is_ws021_once_per_use() {
    // (program, WS021 count, their lines where pinned, compiles clean)
    let rows: [(&str, usize, &[u32], bool); 6] = [
        // A chip body reads the top-level `let`s where it is called; both were
        // `_Unsupported` gates reading 0 with only a WSP001 warning.
        (
            "in n: int\nin m: int\nchip show(s: int) -> int {\n  return s + A + B\n}\nout b = show(n)\n\
             let A = n * 2\nlet B = m * 2\nout a = A\n",
            2,
            &[],
            false,
        ),
        (
            "in n: int\nin m: int\nchip show(s: int) -> int {\n  return s + A + B\n}\nlet A = n * 2\nlet B = m * 2\n\
             out b = show(n)\nout a = A\n",
            0,
            &[],
            true,
        ),
        // Type checking and lowering each reported a call before its declaration.
        (
            "in go: exec\nvar n: int = 0\non go {\n  n = late(1)\n  n = late(2)\n}\nmod late(a: int) -> int { return a }",
            2,
            &[4, 5],
            false,
        ),
        (
            "in n: int\nin go: exec\nvar v: int = 0\nmod f() -> int { return A }\non go { v = f() }\nlet A = n * 2",
            1,
            &[],
            false,
        ),
        (
            "in z: exec\nmod target(n: int) -> int { return n + 1 }\n\
             mod caller() { let x = target(1) BroadcastChatMessage(\"${x}\") }\non z { caller() }\n",
            0,
            &[],
            true,
        ),
        // `vec` is no declared chip or mod: the placeholder path, not WS021.
        ("let v = vec(1.0, 2.0, 3.0)\n", 0, &[], false),
    ];
    let mut failures = Vec::new();
    for (src, count, lines, clean) in rows {
        let r = compile(src);
        let got: Vec<u32> = r
            .diagnostics
            .iter()
            .filter(|d| d.code == "WS021")
            .map(|d| d.range.start.line)
            .collect();
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if got.len() != count
            || (!lines.is_empty() && got != lines)
            || (clean && !errors.is_empty())
        {
            failures.push(format!(
                "{src}\n  WS021 at {got:?} (want {count} {lines:?}), errors {errors:?}"
            ));
        }
    }
    report_failures(&failures, rows.len());
}

// ── `in` array inputs are first-class ──

/// An `in X: T[]` input works like a var array: indexed, its methods called,
/// and passed to an inline mod's `int[]` param, whose reads resolve to the
/// input's ref.
#[test]
fn an_in_array_is_indexed_called_and_passed_like_a_var_array() {
    for src in [
        "in counts: int[]\nin z: exec\n\
         on z { let a = counts[0]  let b = counts.length()  BroadcastChatMessage(\"${a} ${b}\") }",
        "in counts: int[]\nvar dst: int[]\nin z: exec\n\
         mod f(a: int[], d: int[]) { d.clear()  d.push(a[0])  BroadcastChatMessage(\"${a.length()}\") }\n\
         on z { f(counts, dst) }",
    ] {
        let r = compile_ok(src);
        assert_no_code(&r.diagnostics, "WSP001");
        assert!(!has_unsupported(&r.module), "{src}");
    }
}

// ── Namespaced (`import * as`) mods resolve their siblings ──

#[test]
fn namespaced_mod_resolves_siblings() {
    // A namespaced mod body references sibling constants, arrays, and mods by
    // bare name; those must resolve when the mod is inlined at a call site in
    // the importing module (no `_Unsupported` placeholders).
    let lib = "let K = 7.0\n\
        var TBL: int[] = [10, 20, 30]\n\
        mod helper(n: int) -> int { return n + 1 }\n\
        mod draw(ctrl: controller, i: int) {\n\
          let v = K + TBL[i]\n\
          let h = helper(i)\n\
          ctrl.DisplayText(\"hi\", positionX = v, fontSize = h)\n\
        }";
    let r = compile_multi(
        "import * as lib from \"lib\"\non ControllerJoined() -> { controller: c, userId: uid } { lib.draw(c, 0) }",
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    assert_no_code(&r.diagnostics, "WSP001");
    assert!(!has_unsupported(&r.module));
}

#[test]
fn chip_constant_arg_folds_into_the_instance() {
    // A constant argument folds into the instance instead of crossing the boundary as a real gate: the caller
    // built a `_Var` holding the value and wired it into the chip's input
    // rerouter, because a rerouter has no data struct to carry inline gate data.
    // That made `Bump(1)` strictly more expensive than the same call written as a
    // `mod`, which folds the constant straight onto its consumer. The constant is
    // now cloned into the instance and the pin it fed is dropped. Instances of one
    // chip share a template, so the hazard is every instance collapsing onto a
    // single call's value — each must keep its own.
    let src = "\
var acc: int = 0
in t: exec
chip Bump(n: int) { acc = acc + n }
on t { Bump(1) Bump(2) }";
    let r = compile_ok(src);

    // Only the real `acc` variable is left in the caller; no constant-carrying
    // gate remains feeding a chip boundary.
    let parent_vars = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR)
        .count();
    assert_eq!(
        parent_vars, 1,
        "a constant arg should not leave a `_Var` gate in the caller"
    );

    let mut folded: Vec<i64> = Vec::new();
    for child in r.module.chips.values() {
        assert_eq!(
            child.inputs.len(),
            2,
            "the value pin should be folded away, leaving the exec pin plus the acc VarRef boundary pin"
        );
        for n in child.nodes.values() {
            if n.gate_class == gc::VAR_INCREMENT
                && let Some(Literal::Int(v)) = n.properties.get(&*sym::VALUE)
            {
                folded.push(*v);
            }
        }
    }
    folded.sort();
    assert_eq!(
        folded,
        vec![1, 2],
        "each instance must keep its own folded constant"
    );
}

#[test]
fn gate_builtins_desugar_to_their_gates() {
    // The callable gate builtins (GetMapElement, PushToArray, SetVariable, …)
    // desugar to the SAME gates as the method / assignment forms — never an
    // `_Unsupported` placeholder.
    let src = "\
var v: int = 0
var m: Map<string, int> = { \"a\" => 1 }
var a: int[]
in t: exec
on t {
  SetVariable(v, 5)
  PushToArray(a, 7)
  SetArrayElement(a, 0, 9)
  SetMapElement(m, \"b\", 2)
  IncrementVariable(v, 3)
  let g = GetVariable(v)
  let mv = GetMapElement(m, \"a\")
  let el = GetArrayElement(a, 0)
  if HasMapElement(m, \"a\") { v = g + mv.Value + el.Value }
}
out o = v";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "gate builtins must not lower to _Unsupported"
    );
    for cls in [
        gc::VAR_SET,
        gc::VAR_INCREMENT,
        gc::ARRAY_PUSH,
        gc::ARRAY_SET_AT_INDEX,
        gc::MAP_SET,
        gc::MAP_GET,
        gc::MAP_HAS,
    ] {
        assert!(has_gate(&r, cls), "expected gate {cls} from a builtin");
    }
}

#[test]
fn union_and_branch_builtins_lower_to_their_gates() {
    // `Union(a, b)` merges two execs; `Branch(cond, e).A`/`.B` route one. Both
    // are callable builtins that lower to their game gates — no `_Unsupported`,
    // and the `.A`/`.B` friendly outputs resolve to the ExecOutA/ExecOutB ports.
    let src = "\
in a: exec
in b: exec
in c: bool
var x: int = 0
let merged = Union(a, b)
on merged { x = 1 }
on Branch(c, a).A { x = 2 }
on Branch(c, a).B { x = 3 }
out o = x";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "Union/Branch must not lower to _Unsupported"
    );
    assert!(has_gate(&r, gc::UNION), "expected an Exec_Union gate");
    assert!(has_gate(&r, gc::BRANCH), "expected an Exec_Branch gate");
}

#[test]
fn fill_array_builtins_lower_to_their_gates() {
    // The game-named array-fill builtins desugar to the array `fillFrom*`
    // methods and lower to the Gamemode/Zone fill gates — no `_Unsupported`.
    let src = "\
var players: character[]
var ents: character[]
var team: entity
in zoneA: zone
in t: exec
on t {
  FillArrayFromPlayers(players)
  FillArrayFromTeamMembers(players, team)
  GetPlayersInZone(players, zoneA)
  GetEntitiesInZone(ents, zoneA)
}";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "fill builtins must not lower to _Unsupported"
    );
    for cls in [
        gc::GAMEMODE_FILL_FROM_PLAYERS,
        gc::GAMEMODE_FILL_FROM_TEAM,
        gc::ZONE_GET_PLAYERS,
        gc::ZONE_GET_ENTITIES,
    ] {
        assert!(has_gate(&r, cls), "expected gate {cls} from a fill builtin");
    }
}

// ── Template cache must key on call-site context, not the bare name ──

#[test]
fn pure_then_exec_chip_call_gets_its_own_exec_body() {
    // A chip called first in a PURE context (built with no exec boundary
    // pins) then in an EXEC context must NOT have the exec call reuse the pure
    // body. The template cache keyed on the bare chip name, so the exec call got
    // the pin-less body and its exec was wired into a data pin (dropped at
    // emit). Keying on (range, auto_exec, captures) rebuilds for the exec call.
    let src = "\
chip Double(v: int) -> (y: int) { out y = v * 2 }
let pure = Double(3)
in go: exec
var r: int = 0
on go { r = Double(4) }";
    let r = compile_ok(src);
    // Two Double instances: the pure one exposes only `y`; the exec one also
    // carries an `_exec_out` boundary pin. (Assert on OUTPUTS, not inputs — both
    // calls pass a constant, so const-folding drops the `v` input pin either
    // way.) Before the fix both share the pin-less body, so both have one output.
    let mut out_counts: Vec<usize> = r
        .module
        .chips
        .values()
        .filter(|c| crate::intern::resolve(c.name).contains("Double"))
        .map(|c| c.outputs.len())
        .collect();
    out_counts.sort_unstable();
    assert_eq!(
        out_counts,
        vec![1, 2],
        "the exec-context call must get an `_exec_out` pin (2 outputs) while the pure one stays pin-less (1); got {out_counts:?}"
    );
    assert_no_fan_in(&r.module, "root");
}

#[test]
fn captured_var_then_input_arg_chip_call_no_fanin() {
    // `SumInto(xs)` captures a var (no pin); `SumInto(ys)` pins an input.
    // Sharing one name-keyed body handed the 2nd call the 1st's (pin-less) body,
    // fanning ys's ref and the exec chain into a single pin. Keying on the
    // capture set gives each call the right body.
    let src = "\
var xs: int[]
in ys: int[]
static var total: int = 0
chip SumInto(src: int[]) { total = total + src.length() }
in go: exec
on go {
  SumInto(xs)
  SumInto(ys)
}";
    let r = compile_ok(src);
    assert_no_fan_in(&r.module, "root");
}

#[test]
fn same_named_chip_in_two_namespaces_get_distinct_bodies() {
    // Two `chip Helper` in different modules share a name-keyed cache
    // slot, so the 2nd call instantiates the 1st module's body. Keying on the
    // decl's source range (file + span) keeps them distinct.
    let one = "chip Helper() -> (v: int) { out v = 100 }";
    let two = "chip Helper() -> (v: int) { out v = GetUnixTime() }";
    let main = "\
import * as one from \"one\"
import * as two from \"two\"
in go: exec
var r1: int = 0
var r2: int = 0
on go {
  r1 = one.Helper()
  r2 = two.Helper()
}";
    let r = compile_multi(main, &[("one", one), ("two", two)]);
    assert_no_errors(&r);
    // ns_two's body calls GetUnixTime (gate class `..._GetUnixEpoch`); if the
    // 2nd call wrongly got ns_one's const-100 body, no such gate exists anywhere.
    assert!(
        has_gate_deep(&r, gc::GET_UNIX_EPOCH),
        "ns_two.Helper's GetUnixTime (GetUnixEpoch) must be present; the 2nd call got ns_one's body"
    );
}

// ── Two `return expr` in a non-inline chip must not fan in on the output ──

#[test]
fn standalone_chip_two_returns_route_through_pseudovar() {
    // A standalone (non-inline) chip with two `return expr` must route both
    // through a return PseudoVar (Var_Set per return, one Var_Get after the union
    // -> a single output wire), exactly like the inline path. `build_chip_module`
    // never set `mod_return_var`, so both returns wired straight into the output
    // pin — a load-time fan-in failure with no diagnostic.
    let src = "\
in c: bool
in go: exec
var r: int = 0
chip pick() -> (v: int) {
  if c { return 1 }
  return 2
}
on go { r = pick() }";
    let r = compile_ok(src);
    assert_no_fan_in(&r.module, "root");
    let chip = r
        .module
        .chips
        .values()
        .find(|c| crate::intern::resolve(c.name).contains("pick"))
        .expect("pick chip instance");
    // The value output (declared first) takes exactly one incoming wire, fed by
    // the return PseudoVar's Var_Get — not two (one per return).
    let val_out = chip.outputs[0];
    let into_val: Vec<_> = chip
        .wires
        .iter()
        .filter(|w| w.target.node_id == val_out)
        .collect();
    assert_eq!(
        into_val.len(),
        1,
        "value output must have exactly one incoming wire (via the return PseudoVar), got {}: {into_val:?}",
        into_val.len()
    );
    let feeder = &chip.nodes[&into_val[0].source.node_id];
    assert!(
        feeder.gate_class == gc::VAR_GET,
        "value output must be fed by the return Var_Get, got {}",
        feeder.gate_class
    );
    // A Var_Set per return (2), tagged `ret_set`.
    let set_count = chip
        .nodes
        .values()
        .filter(|n| n.note == Some("ret_set"))
        .count();
    assert_eq!(set_count, 2, "a Var_Set per return");
}

// ── Multiple `emit out = v` sites must not fan in on the output ──

#[test]
fn multiple_emit_sites_to_one_output_route_through_backing_var() {
    // Two handlers each `emit result = <v>` would otherwise wire each value
    // straight into the output rerouter — a load-time fan-in with no diagnostic.
    // They must instead share a backing PseudoVar (a Var_Set per site), the var
    // feeding the output once (last-writer-wins).
    let src = "\
out result: int
in a: exec
in b: exec
on a { emit result = 1 }
on b { emit result = 2 }";
    let r = compile_ok(src);
    assert_no_fan_in(&r.module, "root");
    // A backing PseudoVar, written by a Var_Set on each handler (2).
    let set_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.note == Some("out_set"))
        .count();
    assert_eq!(set_count, 2, "a Var_Set per emit site");
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.note == Some("out_backing")),
        "expected an out_backing PseudoVar for the multi-emit output"
    );
}

/// `on a | b { ... }` lowers its body once per trigger part, so ONE syntactic
/// `emit` becomes two drivers on the output rerouter. The emit prescan walks a
/// handler body once regardless of trigger count, so it saw a single emit,
/// skipped the backing var, and the two copies fanned in. Fan-in on a rerouter
/// makes the save fail to load, and `check` reported nothing.
#[test]
fn union_trigger_emit_routes_through_backing_var() {
    let src = "out result: int
var v: int = 7
in a: exec
in b: exec
on a | b { emit result = v }";
    let r = compile_ok(src);
    assert_no_fan_in(&r.module, "root");
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.note == Some("out_backing")),
        "a union-trigger emit needs a backing var: its body lowers once per part"
    );
}

/// The same count must hold for a union trigger nested in an anon chip, which
/// the prescan reaches through `Stmt::Handler`.
#[test]
fn union_trigger_emit_in_anon_chip_routes_through_backing_var() {
    let src = "out result: int
var v: int = 7
in a: exec
in b: exec
chip on a | b { emit result = v }";
    let r = compile_ok(src);
    assert_no_fan_in(&r.module, "root");
}

#[test]
fn chip_emit_to_its_exec_output_is_wired() {
    // A chip that emits its own declared exec output (`-> (next: exec)` + `emit
    // next`) must wire the emit to that output. `build_chip_module` never flushed
    // the child's pending emits, so `emit next` was silently dropped and the
    // `next` output stayed dead — an awaiting caller could never resume.
    let src = "\
chip Emitter() -> (next: exec) {
  emit next
}
in go: exec
static var n: int = 0
on go {
  await Emitter()
  n = 1
}";
    let r = compile_ok(src);
    let chip = r
        .module
        .chips
        .values()
        .find(|c| crate::intern::resolve(c.name).contains("Emitter"))
        .expect("Emitter chip instance");
    // `next` (declared) is added before the auto `_exec_out`, so it's outputs[0].
    assert_eq!(
        chip.outputs.len(),
        2,
        "Emitter has a declared `next` plus the auto `_exec_out`, got {}",
        chip.outputs.len()
    );
    let next_out = chip.outputs[0];
    assert!(
        chip.wires.iter().any(|w| w.target.node_id == next_out),
        "`emit next` must feed the chip's `next` output — build_chip_module must flush pending emits"
    );
}

#[test]
fn single_emit_to_output_keeps_direct_wire() {
    // A single-site value-emit must keep the direct value->output wire and NOT
    // grow a backing PseudoVar (no extra gate, unchanged behavior).
    let src = "\
out result: int
in a: exec
on a { emit result = 42 }";
    let r = compile_ok(src);
    assert_no_fan_in(&r.module, "root");
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.note == Some("out_backing")),
        "a single-emit output must not get a backing PseudoVar"
    );
}

#[test]
fn exec_call_initializer_in_a_handler_nested_chip_runs_on_the_chain() {
    // A `chip { }` in a handler body is a step on that handler's chain, so a
    // declaration there initializes as it does one level up. The purity rule is
    // for a top-level chip inheriting a leaked exec; here it dropped the
    // initializer entirely, no chip instance and no Var_Set.
    let r = compile_ok(
        "chip MyExecChip() -> (r: int) {\n\
           let t = ReadBrickGrid().GetLocationRotation()\n\
           return 1\n\
         }\n\
         on ServerUptime() {\n\
           chip { var helo = MyExecChip() }\n\
         }\n",
    );
    assert_eq!(
        count_class(&r.module, gc::VAR_SET),
        1,
        "the initializer must write the var"
    );
    assert!(
        count_class(&r.module, gc::READ_BRICK_GRID) == 1,
        "the called chip must be instantiated"
    );
}

/// A field read on a multi-output chip call projects the named output, inline
/// (`Pair(n).hi`) or through a `let`. The inline call has no record and its
/// outputs are rerouters, so only the positional pin mapping resolves the
/// field; both spellings are checked so they cannot drift apart.
#[test]
fn a_multi_output_chip_field_read_projects_the_named_output() {
    let decl = "in n: int\nchip Pair(v: int) -> (lo: int, hi: int) {\n  out lo = v - 1\n  out hi = v + 1\n}\n";
    for read in [
        "out total = Pair(n).hi\n",
        "let p = Pair(n)\nout total = p.hi\n",
    ] {
        let r = compile_ok(&format!("{decl}{read}"));
        assert_no_code(&r.diagnostics, "WSP001");
        // Not merely "no placeholder": it must pick the SECOND output pin.
        // Falling back to the call's primary value port would also drop the
        // placeholder while silently reading `lo`.
        let chip = only_chip(&r, "the Pair chip instance");
        let total = *r.module.outputs.first().expect("the `total` output pin");
        assert!(
            wired_reachable(&r, chip.outputs[1], total),
            "{read}: `total` must be driven by the `hi` pin"
        );
        assert!(
            !wired_reachable(&r, chip.outputs[0], total),
            "{read}: `total` must NOT be driven by the `lo` pin"
        );
    }
}

/// An anonymous `chip { }` body shares the top level, and its statements name
/// its own storage whatever the top level declares of the name, before the
/// chip or after it: each program saves what its twin with the top-level name
/// renamed saves, in both fold modes. A `const` before the chip built a second,
/// unused gate for the chip's `var`, and a write to it was `WS007`.
#[test]
fn a_top_level_name_beside_anon_chip_storage_hides_nothing_from_the_chip() {
    // A gate is labelled with the name it stores or binds.
    let outcome =
        |src: &str, fold: bool| without_names(super::shadow_differential::outcome(src, &[], fold));
    let head = "in go: exec\nin a: int\nvar n: int = 0\nout on1: int = n\n";
    for body in [
        "const @ = 4\nchip { var x: int = 9\n on go { n = x } }",
        "const @ = 4\nchip { var x: int = 9\n on go { x = x + 1\n n = x } }",
        "let @ = a + 1\nchip { var x: int = 9\n on go { x += 1\n n = x } }",
        "const @ = 4\nchip { buffer x: int = 3\n on go { n = x } }",
        "chip { var x: int = 9\n on go { x += 1\n n = x } }\nconst @ = 4",
        "chip { var x: int[] = []\n on go { x.push(1)\n n = x.length() } }\nconst @ = 4",
        "chip { var x: Map<string, int> = {}\n on go { x.set(\"k\", 1)\n n = x.length() } }\nconst @ = 4",
        "chip { static var x: int = 9\n on go { x += 1\n n = x } }\nconst @ = 4",
        "chip { buffer x = a\n on go { n = x } }\nconst @ = 4",
        "chip { chip { var x: int = 9\n on go { n = x } } }\nconst @ = 4",
        "chip { var x: int = 9\n on go { n = x } }\nlet (@, y) = (1, 2)",
        "chip { var x: int = 9\n on go { n = x } }\nlet { x: @, y } = { x: 1, y: 2 }",
        "chip { var x: int = 9\n on go { x += 1\n n = x } }\nlet @: exec",
        // A chip in a handler names its storage from the chip on.
        "var x: int = 7\non go { let q = x\n chip { var @: int = 5\n @ += 1 }\n n = q }",
    ] {
        for fold in [false, true] {
            let got = outcome(&format!("{head}{}", body.replace('@', "x")), fold);
            let want = outcome(&format!("{head}{}", body.replace('@', "zq")), fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert_eq!(got, want, "{body}, fold {fold}");
        }
    }
    // A top-level `let` beside top-level storage of its name hides it from
    // its position on (`WS108`).
    let twice = format!("{head}buffer x: int = x + 1\nlet x = ServerUptime()\non x {{ n = 1 }}");
    let renamed =
        format!("{head}buffer x0: int = x0 + 1\nlet x = ServerUptime()\non x {{ n = 1 }}");
    for fold in [false, true] {
        let (got, want) = (outcome(&twice, fold), outcome(&renamed, fold));
        assert!(
            got.compiled && got.codes.contains(&"WS108".to_string()),
            "{:?}",
            got.codes
        );
        assert_eq!(
            (got.shape, got.comps),
            (want.shape, want.comps),
            "fold {fold}"
        );
    }
}

/// One name has one binding at each place, for every reader. A top-level
/// declaration an anonymous chip's storage shares a name with is what the
/// name means outside the chip, before it and after it, a constant-only slot
/// and the body of a `mod` or `chip` written outside and called inside
/// included; inside the chip its storage is. Each row is compared with its
/// twin where one of the two declarations is renamed (`@`), both folds: the
/// chip's storage, or for a chip in a handler, which names its storage from
/// the chip on, the top-level one.
#[test]
fn a_name_beside_anon_chip_storage_binds_one_declaration_at_each_place() {
    let outcome =
        |src: &str, fold: bool| without_names(super::shadow_differential::outcome(src, &[], fold));
    let head = "in go: exec\nin go2: exec\nin a: int\nvar n: int = 0\nout on1: int = n\nvar s: string = \"\"\nout os: string = s\n";
    let rows: [(&str, &str); 20] = [
        (
            "ch",
            "const ch = \"evt\"\nchip { var @: string = \"chipvar\"\n on go { @ = \"y\" } }\n\
             on go2 { s = ch\n SendCustomEvent(ch, 1) }\non CustomEvent(\"evt\") -> (p: int) { n = p }",
        ),
        (
            "x",
            "let x = \"hello\"\nchip { var @: int = 9\n on go { @ += 1\n n = @ } }\non go2 { s = x }",
        ),
        (
            "x",
            "chip { var @: int = 9\n on go { @ += 1\n n = @ } }\nconst x = \"zz\"\non go2 { s = x }",
        ),
        (
            "ch",
            "const ch = \"evt\"\non go2 { s = ch\n SendCustomEvent(ch, 1) }\nchip { var @: string = \"x\"\n on go { @ = \"y\" } }\n\
             on CustomEvent(\"evt\") -> (p: int) { n = p }",
        ),
        (
            "x",
            "let x = \"hello\"\nchip { var @: int = 9\n on go { @ += 1 } }\nmod rd() -> string { return x }\non go2 { s = rd() }",
        ),
        (
            "x",
            "mod rd() -> string { return x }\nlet x = \"hello\"\nchip { var @: int = 9\n on go { @ += 1 } }\non go2 { s = rd() }",
        ),
        (
            "x",
            "let x = \"hello\"\nchip { var @: int = 9\n on go { @ += 1 } }\nlet z = x\non go2 { s = z }",
        ),
        (
            "x",
            "let x = \"hello\"\nchip { var @: int = 9\n on go { @ += 1 } }\nout ox: string = x",
        ),
        (
            "K",
            "const K = 3\nchip { var @: int = 9\n on go { @ += 1 } }\non go2 { n = K * 2 }",
        ),
        (
            "K",
            "const K = 3\nchip { var @: int = 9\n on go { n = @ * 2 } }",
        ),
        (
            "xs",
            "const xs = 4\nchip { var @: int[] = [1, 2]\n on go { @.push(a)\n n = @.length() } }\non go2 { n = xs }",
        ),
        (
            "mm",
            "let mm = a * 2\nchip { var @: Map<string, int> = {}\n on go { @.set(\"k\", a)\n n = @.get(\"k\").Value } }\n\
             on go2 { n = mm }",
        ),
        (
            "x",
            "const x = 4\nchip { buffer @: int = 3\n on go { n = @ } }\non go2 { n = x }",
        ),
        (
            "x",
            "const x = 4\nchip { static var @: int = 9\n on go { @ += 1\n n = @ } }\non go2 { n = x * 2 }",
        ),
        (
            "K",
            "const @ = 3\non go { chip { var K: int = 9\n K += 1 }\n n = K * 2 }\non go2 { n = @ }",
        ),
        // A declaration written outside the chip reads the top level, called
        // from inside it; one written inside reads the chip's storage.
        (
            "K",
            "const K = 3\nmod rd() -> int { return K * 2 }\nchip { var @: int = 9\n on go { n = rd() } }",
        ),
        (
            "x",
            "let x = \"top\"\nmod rd() -> string { return x }\nchip { var @: string = \"chip\"\n on go { s = rd() } }",
        ),
        (
            "x",
            "let x = \"top\"\nchip CX() -> (o: string) { out o = x }\nchip { var @: string = \"chip\"\n on go { s = CX() } }",
        ),
        (
            "K",
            "const @ = 3\nchip { var K: int = 9\n chip CK() -> (o: int) { out o = K * 2 }\n on go { n = CK() } }",
        ),
        (
            "K",
            "const @ = 3\nchip { var K: int = 9\n mod mk() -> int { return K * 2 }\n on go { n = mk() } }",
        ),
    ];
    for (name, body) in rows {
        for fold in [false, true] {
            let got = outcome(&format!("{head}{}", body.replace('@', name)), fold);
            let want = outcome(&format!("{head}{}", body.replace('@', "cz")), fold);
            assert!(got.compiled, "{body}: {:?}", got.codes);
            assert_eq!(got, want, "{body}, fold {fold}");
        }
    }
    // Inside the chip its storage is no constant, for a constant-only slot
    // too.
    let inside = format!(
        "{head}const ch = \"evt\"\nchip {{ var ch: string = \"x\"\n on go {{ SendCustomEvent(ch, 1) }} }}"
    );
    assert!(
        errs(&inside).contains(&"WS028".to_string()),
        "{:?}",
        errs(&inside)
    );
    // The program without the chip reads the `const` where the chip-wrapped
    // one does.
    let sends = |o: &super::shadow_differential::Outcome| {
        o.comps
            .iter()
            .any(|c| c.contains("Exec_Var_Set") && c.contains("\"evt\""))
            && o.comps
                .iter()
                .any(|c| c.contains("SendCustomEvent") && c.contains("\"evt\""))
    };
    let reads = "\non go2 { s = ch\n SendCustomEvent(ch, 1) }\non CustomEvent(\"evt\") -> (p: int) { n = p }";
    let o = outcome(
        &format!("{head}const ch = \"evt\"\nchip {{ var ch: string = \"chipvar\" }}{reads}"),
        false,
    );
    assert!(o.compiled && sends(&o), "{:?} {:?}", o.codes, o.comps);
    // Without the chip, the `var` hides the `const` from its position on
    // (`WS108`), so the send names storage, as it does with the `const`
    // renamed.
    let twice = errs(&format!(
        "{head}const ch = \"evt\"\nvar ch: string = \"chipvar\"{reads}"
    ));
    let renamed = errs(&format!(
        "{head}const ch0 = \"evt\"\nvar ch: string = \"chipvar\"{reads}"
    ));
    assert!(
        !twice.contains(&"WS013".to_string()) && twice == renamed,
        "{twice:?} / {renamed:?}"
    );
}

/// An anonymous `chip { }` writes its body wherever it is placed. The one
/// placement that used to drop it on the floor is directly inside a NAMED
/// chip's body: `build_chip_module`'s pre-declare loop never created the
/// anon chip's `NodeKind::Chip` node, so `lower_anon_chip` found nothing to
/// attach to and returned, discarding the whole body with no diagnostic.
/// The other rows are the controls that always worked; they are here so a
/// future change cannot fix one placement by breaking another.
#[test]
fn an_anon_chip_body_lowers_in_every_placement() {
    // One body text in every row, so a row can only differ by where the
    // `chip { }` sits. It carries its own handler because an anon chip in a
    // pure position has no ambient exec to fire a call on.
    let body = "on go { BroadcastChatMessage(\"x\") }";
    let cases: Vec<(&str, String)> = vec![
        ("top level", format!("in go: exec\nchip {{ {body} }}\n")),
        (
            "in a top-level handler",
            format!("in go: exec\nin go2: exec\non go2 {{ chip {{ {body} }} }}\n"),
        ),
        (
            "in a top-level anon chip",
            format!("in go: exec\nchip {{ chip {{ {body} }} }}\n"),
        ),
        (
            "in a handler inside a named chip",
            format!(
                "in go: exec\n\
                 in t0: exec\n\
                 chip Outer(t: exec) {{ on t {{ chip {{ {body} }} }} }}\n\
                 let w = Outer(t0)\n"
            ),
        ),
        (
            "directly in a named chip's body",
            format!(
                "in go: exec\n\
                 in t0: exec\n\
                 chip Outer(t: exec) {{ chip {{ {body} }} }}\n\
                 let w = Outer(t0)\n"
            ),
        ),
    ];
    for (label, src) in &cases {
        let r = compile_ok(src);
        assert!(
            has_gate_deep(&r, gc::GAMEMODE_BROADCAST_CHAT),
            "{label}: the anon chip's body vanished:\n{src}"
        );
    }
}

/// The same drop, seen through what the enclosing chip promised: a `var`, a
/// `partition_anon_chips` walked only the root, so an anon `chip { }` written
/// inside a NAMED chip kept its `chip_id` tags and never became a child
/// module. Emit skips a `Chip` node with no `chips` entry, so the box the
/// author labelled was silently dropped and its gates were emitted flat onto
/// the enclosing chip's grid. Live in `secret-hitler/src/main.ws`, where 259
/// nodes were affected.
#[test]
fn an_anon_chip_in_a_named_chip_gets_its_own_sub_grid() {
    let r = compile_ok(
        "in go: exec\n\
         chip Outer(t: exec) -> (rr: int) {\n\
           chip {\n\
             var counter: int = 0\n\
             on t { counter = counter + 1 }\n\
             out rr = counter\n\
           }\n\
         }\n\
         let w = Outer(go)\n\
         out o: int = w\n",
    );
    let outer = r
        .module
        .chips
        .values()
        .next()
        .expect("the Outer instance module");
    assert!(
        !outer.chips.is_empty(),
        "the nested anon chip must become a child module of Outer, not stay tagged on its grid"
    );
    // The tags are consumed by the partition, not merely duplicated: a node
    // still carrying `chip_id` at Outer's level is one emit would place on
    // the wrong grid.
    assert!(
        outer.nodes.values().all(|n| n.chip_id.is_none()),
        "no node may keep a `chip_id` tag once its anon chip has a module"
    );
}

/// `let` and the named chip's own declared `out` all live inside the nested
/// anon chip, so losing the body left `Outer_0` with three boundary pins and
/// no gates at all, its declared output wired straight from the input
/// rerouter. Documented as supported at `docs/src/chips.md:697,714`.
#[test]
fn an_anon_chip_in_a_named_chip_body_drives_the_declared_output() {
    let r = compile_ok(
        "in go: exec\n\
         chip Outer(t: exec) -> (rr: int) {\n\
           chip {\n\
             var counter: int = 0\n\
             let doubled = 21 * 2\n\
             on t { counter = counter + 1 }\n\
             out rr = doubled\n\
           }\n\
         }\n\
         let w = Outer(go)\n\
         out o: int = w\n",
    );
    let outer = only_chip(&r, "the Outer instance module");
    assert!(
        count_class(outer, gc::VAR) == 1,
        "the nested anon chip's `var counter` must exist in Outer's module tree"
    );
    // `doubled` reaches the declared `rr` pin: unfolded that is a real
    // multiply, so the output is driven by a gate rather than by the chip's
    // own input rerouter.
    let rr = *outer.outputs.last().expect("Outer's declared `rr` pin");
    let driver = outer
        .wires
        .iter()
        .chain(outer.chips.values().flat_map(|c| c.wires.iter()))
        .find(|w| w.target.node_id == rr)
        .map(|w| w.source.node_id)
        .expect("`out rr = doubled` must drive the declared output pin");
    assert_ne!(
        Some(&NodeKind::Input),
        outer.nodes.get(&driver).map(|n| &n.kind),
        "the declared output must not be fed straight from the chip's input rerouter"
    );
}

/// `depth` anonymous chips nested one inside the next, each bumping its own
/// var, wrapped by `wrap` (which receives the nested block).
fn nested_anon_chips(depth: usize, wrap: impl Fn(&str, &str) -> String) -> String {
    let vars: String = (1..=depth)
        .map(|i| format!("var v{i}: int = 0\n"))
        .collect();
    let mut body = String::new();
    for i in 1..=depth {
        body.push_str(&format!("chip {{\nv{i} = v{i} + {i}\n"));
    }
    for _ in 1..=depth {
        body.push_str("}\n");
    }
    wrap(&vars, &body)
}

/// Every `Chip` node in the tree has a child module, and no node still carries
/// the anon-chip tag the partition consumes.
fn assert_chip_tree_complete(m: &crate::ir::Module, what: &str) {
    for (id, n) in &m.nodes {
        if n.kind == NodeKind::Chip {
            assert!(
                m.chips.contains_key(id),
                "{what}: chip node {id} in module {:?} has no child module",
                crate::intern::resolve(m.name)
            );
        }
        assert!(
            n.chip_id.is_none(),
            "{what}: node {id} kept its chip_id tag"
        );
    }
    for c in m.chips.values() {
        assert_chip_tree_complete(c, what);
    }
}

/// The partition built every anon chip's module flat and then re-nested them
/// in hash order, dropping a grandchild's module whenever its parent moved
/// first, so three or more levels failed at emit with a wire to an unknown node.
/// Nested in a handler, a named chip's handler, or a mod body, every level gets
/// its module, keeps its increment, and the program emits.
#[test]
fn deeply_nested_anon_chips_all_get_modules() {
    let wraps: [(&str, fn(&str, &str) -> String); 3] = [
        ("handler", |vars, body| {
            format!("in go: exec\n{vars}on go {{\n{body}v1 = 9\n}}\nout o: int = v1 + v2\n")
        }),
        ("named chip", |vars, body| {
            format!(
                "in go: exec\nchip Outer(t: exec) -> (r: int) {{\n{vars}on t {{\n{body}}}\n\
                 out r = v1 + v2\n}}\nlet w = Outer(go)\nout o: int = w\n"
            )
        }),
        ("mod", |vars, body| {
            format!(
                "in go: exec\n{vars}mod bump() {{\n{body}}}\non go {{ bump() }}\nout o: int = v1 + v2\n"
            )
        }),
    ];
    for (place, wrap) in wraps {
        for depth in 3..=5 {
            let src = nested_anon_chips(depth, wrap);
            let what = format!("{place} depth {depth}");
            let r = compile_ok(&src);
            assert_chip_tree_complete(&r.module, &what);
            assert_eq!(
                count_class(&r.module, gc::VAR_INCREMENT),
                depth,
                "{what}: every nesting level's increment must survive:\n{src}"
            );
            assert_emits(&r.module, &crate::layout::layout(&r.module));
        }
    }
}

/// A plain `emit` ending a handler's anon chip forks the chain; it does not end
/// it, so the statement after the chip still runs (the same body in `if true`
/// always kept it).
#[test]
fn plain_emit_ending_an_anon_chip_keeps_the_chain() {
    let r = compile_ok(
        "in go: exec\nvar a: int = 0\nvar b: int = 0\nlet sig: exec\n\
         on go {\n  chip {\n    a = 1\n    emit sig\n  }\n  b = 2\n}\n\
         on sig { a = a + 10 }\nout o: int = a + b\n",
    );
    assert_no_code(&r.diagnostics, "WS058");
    assert_eq!(
        triggered_var_sets(&r.module),
        2,
        "both `a = 1` and `b = 2` must have an exec trigger"
    );
    let lr = crate::layout::layout(&r.module);
    assert_emits(&r.module, &lr);
}

/// `Exec_Var_Set` gates, tree-wide, whose `Exec` input is wired.
fn triggered_var_sets(m: &crate::ir::Module) -> usize {
    m.modules_deep()
        .into_iter()
        .map(|sub| {
            let triggered = |id: &crate::ir::NodeId| {
                sub.wires
                    .iter()
                    .any(|w| w.target.node_id == *id && w.target.port == WirePort::Exec)
            };
            sub.nodes
                .values()
                .filter(|n| n.gate_class == gc::VAR_SET && triggered(&n.id))
                .count()
        })
        .sum()
}

/// A BUFFERED `emit` ending the chip does close the chain; the statement after
/// it is reported as cut off by that emit, not as sitting in a pure position.
#[test]
fn buffered_emit_ending_an_anon_chip_names_the_emit() {
    let r = compile_ok(
        "in go: exec\nvar a: int = 0\nvar b: int = 0\nlet sig: exec\n\
         on go {\n  chip {\n    a = 1\n    buffer emit sig\n  }\n  b = 2\n}\n\
         on sig { a = a + 10 }\nout o: int = a + b\n",
    );
    let ws058: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS058").collect();
    assert_eq!(ws058.len(), 1, "{:?}", r.diagnostics);
    assert!(
        ws058[0].message.contains("buffer emit") && !ws058[0].message.contains("pure position"),
        "WS058 must blame the buffered emit: {}",
        ws058[0].message
    );
}

/// The node feeding the `Value` of the root-level `Exec_Var_Set` that writes
/// the var labelled `label`.
fn var_set_value_source(r: &LowerResult, label: &str) -> Option<crate::ir::NodeId> {
    let m = &r.module;
    let var = m.nodes.iter().find(|(_, n)| {
        n.gate_class == gc::VAR
            && matches!(
                n.properties.get(&crate::intern::sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == label
            )
    })?;
    let set = m.wires.iter().find(|w| {
        w.source.node_id == *var.0
            && w.target.port == WirePort::VarRef
            && m.nodes
                .get(&w.target.node_id)
                .is_some_and(|n| n.gate_class == gc::VAR_SET)
    })?;
    m.wires
        .iter()
        .find(|w| w.target.node_id == set.target.node_id && w.target.port == WirePort::Value)
        .map(|w| w.source.node_id)
}

/// Every output pin of `m` has exactly one driver inside `m`.
fn assert_outputs_driven_once(m: &crate::ir::Module, what: &str) {
    for out in &m.outputs {
        let drivers = m
            .wires
            .iter()
            .filter(|w| w.target.node_id == *out && w.target.port != WirePort::Layout)
            .count();
        assert_eq!(drivers, 1, "{what}: output pin {out} has {drivers} drivers");
    }
}

/// Early returns in a multi-output chip each drove every output pin directly,
/// a fan-in the emitter rejects; each return now stores the outputs, and the
/// storage drives each pin once.
#[test]
fn multi_output_chip_with_early_returns_drives_each_output_once() {
    let r = compile_ok(
        "in go: exec\nin ia: int\nin ib: int\nin bd: bool\nvar oa: int = 0\nvar ob: int = 0\n\
         chip C(a0: int, b0: int, d0: bool) -> (zeta: int, alpha: int) {\n\
           if d0 { return (a0 + 1, b0 + 2) }\n\
           return (a0 + 3, b0 + 4)\n\
         }\n\
         on go {\n  let (u, v) = C(ia, ib, bd)\n  oa = u\n  ob = v\n}\n",
    );
    let c = only_chip(&r, "the C instance module");
    assert_outputs_driven_once(c, "C");
    assert_eq!(
        c.nodes
            .values()
            .filter(|n| n.gate_class == gc::VAR_SET)
            .count(),
        4,
        "each of the two returns stores both outputs"
    );
    let lr = crate::layout::layout(&r.module);
    assert_emits(&r.module, &lr);
}

/// A record output among several has one pin per field. The return wired only
/// the first output, and the caller read the record's pins as if each output
/// had one, so `v.x` split a vector off the `p.x` pin and `w` read `p.y`.
#[test]
fn multi_output_chip_with_a_record_output_wires_every_pin() {
    let r = compile_ok(
        "type P = { x: int, y: int }\nin ia: int\nin ib: int\n\
         var oa: int = 0\nvar ob: int = 0\nvar ox: int = 0\n\
         chip D(a0: int, b0: int) -> (a: int, p: P, b: int) {\n\
           return (a0, { x: b0, y: a0 }, b0)\n\
         }\n\
         in go2: exec\n\
         on go2 {\n  let (u, v, w) = D(ia, ib)\n  oa = u\n  ox = v.x\n  ob = w\n}\n",
    );
    assert!(
        !has_gate_deep(&r, gc::SPLIT_VECTOR),
        "`v.x` is a record field, not a swizzle"
    );
    let d = only_chip(&r, "the D instance module");
    assert_outputs_driven_once(d, "D");
    // Pins in declaration order: a, p.x, p.y, b.
    assert_eq!(var_set_value_source(&r, "oa"), Some(d.outputs[0]));
    assert_eq!(var_set_value_source(&r, "ox"), Some(d.outputs[1]));
    assert_eq!(var_set_value_source(&r, "ob"), Some(d.outputs[3]));
    let lr = crate::layout::layout(&r.module);
    assert_emits(&r.module, &lr);
}

/// Both at once: a record output among several, written by two returns.
#[test]
fn multi_output_chip_with_a_record_output_and_early_returns() {
    let r = compile_ok(
        "type P = { x: int, y: int }\nin go: exec\nin ia: int\nin ib: int\nin bd: bool\n\
         var oa: int = 0\nvar ob: int = 0\nvar ox: int = 0\nvar oy: int = 0\n\
         chip D(a0: int, b0: int, d0: bool) -> (a: int, p: P, b: int) {\n\
           if d0 { return (a0, { x: b0, y: a0 }, b0) }\n\
           return (b0, { x: a0, y: b0 }, a0)\n\
         }\n\
         on go {\n  let (u, v, w) = D(ia, ib, bd)\n  oa = u\n  ox = v.x\n  oy = v.y\n  ob = w\n}\n",
    );
    let d = only_chip(&r, "the D instance module");
    assert_outputs_driven_once(d, "D");
    assert_eq!(
        d.nodes
            .values()
            .filter(|n| n.gate_class == gc::VAR_SET)
            .count(),
        8,
        "each of the two returns stores all four leaves"
    );
    assert_eq!(var_set_value_source(&r, "ox"), Some(d.outputs[1]));
    assert_eq!(var_set_value_source(&r, "oy"), Some(d.outputs[2]));
    assert_eq!(var_set_value_source(&r, "ob"), Some(d.outputs[3]));
    let lr = crate::layout::layout(&r.module);
    assert_emits(&r.module, &lr);
}

/// A tuple return with one element per VALUE output skips an exec output in
/// the middle of the signature. Filled by raw position, `b`'s value went to
/// the `done` exec pin (chip) or nowhere (mod), and `b` read 0.
#[test]
fn tuple_return_skips_an_exec_output_between_values() {
    for kw in ["chip", "mod"] {
        for body in [
            "return (a0 + 1, a0 + 2)",
            "if d0 { return (a0 + 1, a0 + 2) }\nreturn (a0 + 3, a0 + 4)",
        ] {
            let src = format!(
                "in go: exec\nin ia: int\nin bd: bool\nvar oa: int = 0\nvar ob: int = 0\n\
                 {kw} M(a0: int, d0: bool) -> (a: int, done: exec, b: int) {{\n{body}\n}}\n\
                 on go {{\n  let r = M(ia, bd)\n  oa = r.a\n  ob = r.b\n}}\n"
            );
            let r = compile_ok(&src);
            let what = format!("{kw}: {body}");
            // Every `a0 + k` the body computes reaches a write or an output pin.
            let adds = nodes_of(&r.module, "BrickComponentType_Expr_MathAdd");
            let used = r.module.modules_deep().into_iter().map(|m| {
                adds.iter()
                    .filter(|n| {
                        m.nodes.contains_key(&n.id)
                            && m.wires.iter().any(|w| w.source.node_id == n.id)
                    })
                    .count()
            });
            let (adds, used) = (adds.len(), used.sum::<usize>());
            assert_eq!(
                adds,
                body.matches("a0 +").count(),
                "{what}: every returned value is lowered"
            );
            assert_eq!(adds, used, "{what}: {used} of {adds} values reach anything");
            if kw == "chip" {
                let m = only_chip(&r, "the M instance module");
                let done = m.outputs[1];
                assert!(
                    !m.wires.iter().any(|w| w.target.node_id == done),
                    "{what}: no value may drive the `done` exec pin"
                );
                assert_eq!(var_set_value_source(&r, "ob"), Some(m.outputs[2]), "{what}");
            }
            let lr = crate::layout::layout(&r.module);
            assert_emits(&r.module, &lr);
        }
    }
}

/// With no exec chain in the body a branch holding a `return` is never
/// lowered, so the call always gave the last return's value. A chip of any
/// output count, and a mod with several outputs, now say so (the one-output
/// mod already did).
#[test]
fn branch_returns_without_an_exec_chain_are_reported() {
    for (kw, sig, ret1, ret2, read) in [
        ("chip", "-> int", "a0 + 1", "a0 + 3", "C(ia, bd)"),
        (
            "chip",
            "-> (p: int, q: int)",
            "(a0 + 1, a0)",
            "(a0 + 3, a0)",
            "C(ia, bd).p",
        ),
        (
            "mod",
            "-> (p: int, q: int)",
            "(a0 + 1, a0)",
            "(a0 + 3, a0)",
            "C(ia, bd).p",
        ),
    ] {
        let src = format!(
            "in ia: int\nin bd: bool\n\
             {kw} C(a0: int, d0: bool) {sig} {{\n  if d0 {{ return {ret1} }}\n  return {ret2}\n}}\n\
             out o: int = {read}\n"
        );
        let r = compile(&src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS007" && d.message.contains("returns from inside a branch")),
            "{kw} {sig}: expected WS007, got {:?}",
            r.diagnostics
        );
    }
}

/// A chip whose one output is a record: `return rec` drives the field pins,
/// once, from the return that ran. Every return used to be dropped, leaving
/// the pins unwired, and two returns stored into a scalar var no pin read.
#[test]
fn record_output_chip_returns_drive_the_field_pins() {
    for (sig, body, sets) in [
        ("-> P", "return { x: a0, y: 1 }", 0),
        (
            "-> P",
            "if d0 { return { x: a0, y: 1 } }\nreturn { x: 2, y: a0 }",
            4,
        ),
        (
            "-> (r: P)",
            "if d0 { emit r = { x: a0, y: 1 } }\nreturn { x: 2, y: a0 }",
            4,
        ),
    ] {
        let src = format!(
            "type P = {{ x: int, y: int }}\nin go: exec\nin ia: int\nin bd: bool\n\
             var ox: int = 0\nvar oy: int = 0\n\
             chip D(a0: int, d0: bool) {sig} {{\n{body}\n}}\n\
             on go {{\n  let p = D(ia, bd)\n  ox = p.x\n  oy = p.y\n}}\n"
        );
        let r = compile_ok(&src);
        let d = only_chip(&r, "the D instance module");
        assert_outputs_driven_once(d, body);
        assert_eq!(
            d.nodes
                .values()
                .filter(|n| n.gate_class == gc::VAR_SET)
                .count(),
            sets,
            "{body}: every write stores both fields"
        );
        assert_eq!(var_set_value_source(&r, "ox"), Some(d.outputs[0]), "{body}");
        assert_eq!(var_set_value_source(&r, "oy"), Some(d.outputs[1]), "{body}");
        let lr = crate::layout::layout(&r.module);
        assert_emits(&r.module, &lr);
    }
}

/// A branch `return` in a chip called with no exec chain is reported at the
/// call, once per call, not at the chip's declaration.
#[test]
fn chip_branch_return_ws007_is_reported_at_each_pure_call() {
    let src = "in ia: int\nin bd: bool\n\
               chip C(a0: int, d0: bool) -> (p: int, q: int) {\n\
                 if d0 { return (a0 + 1, a0) }\n  return (a0 + 3, a0)\n}\n\
               in go: exec\nvar x: int = 0\non go { x = C(ia, bd).p }\n\
               out o: int = C(ia, bd).q\nout o2: int = C(ia, bd).p\n";
    let r = compile(src);
    let lines: Vec<u32> = r
        .diagnostics
        .iter()
        .filter(|d| d.code == "WS007" && d.message.contains("returns from inside a branch"))
        .map(|d| d.range.start.line)
        .collect();
    assert_eq!(
        lines,
        vec![10, 11],
        "one WS007 per pure call, at the call: {:?}",
        r.diagnostics
    );
}

/// A chip whose first parameter is `exec` has no exec chain of its own, so
/// "call it from a handler" does not help; the returns belong under `on t`.
#[test]
fn chip_with_an_exec_first_param_names_its_trigger_in_ws007() {
    let r = compile(
        "in go: exec\nin ia: int\nin bd: bool\nvar oa: int = 0\n\
         chip C(t: exec, a0: int, d0: bool) -> int {\n\
           if d0 { return a0 + 1 }\n  return a0 + 3\n}\n\
         on go {\n  oa = C(go, ia, bd)\n}\n",
    );
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS007")
        .unwrap_or_else(|| panic!("expected WS007: {:?}", r.diagnostics));
    assert!(
        d.message.contains("on t {") && !d.message.contains("call it from a handler"),
        "{}",
        d.message
    );
    assert_eq!(d.range.start.line, 10, "reported at the call");
}

/// Returns inside a nested `on` or an anonymous `chip {}` of a chip or mod body
/// store into the outputs' storage: the path that ran wins and the output holds
/// it. They were not counted, so a misleading WS013 fired and the IR carried a
/// fan-in.
#[test]
fn returns_in_a_nested_handler_or_anon_chip_are_stored() {
    let bodies = [
        (
            "t: exec, a0: int",
            "-> (r: int, s: int)",
            "on t {\n  if a0 > 0 { return (a0, 2) }\n  return (3, a0)\n}",
            "let (u, v) = C(go, ia)\n  oa = u\n  ob = v",
            4,
        ),
        (
            "t: exec, a0: int",
            "-> int",
            "on t {\n  if a0 > 0 { return a0 }\n  return 2\n}",
            "oa = C(go, ia)",
            2,
        ),
        (
            "t: exec, a0: int",
            "-> int",
            "on t {\n  return a0 + 1\n}",
            "oa = C(go, ia)",
            1,
        ),
        (
            "a0: int, d0: bool",
            "-> (r: int, s: int)",
            "chip {\n  if d0 { return (a0 + 1, a0 + 2) }\n  return (a0 + 3, a0 + 4)\n}",
            "let (u, v) = C(ia, bd)\n  oa = u\n  ob = v",
            4,
        ),
        (
            "a0: int, d0: bool",
            "-> int",
            "chip {\n  if d0 { return a0 + 1 }\n  return a0 + 3\n}",
            "oa = C(ia, bd)",
            2,
        ),
    ];
    for kw in ["chip", "mod"] {
        for (params, sig, body, call, stores) in bodies {
            let src = format!(
                "in go: exec\nin ia: int\nin bd: bool\nvar oa: int = 0\nvar ob: int = 0\n\
                 {kw} C({params}) {sig} {{\n{body}\n}}\non go {{\n  {call}\n}}\n"
            );
            let r = compile_ok(&src);
            assert_no_fan_in(&r.module, &src);
            let caller_sets = if call.contains("ob") { 2 } else { 1 };
            assert_eq!(
                count_class(&r.module, gc::VAR_SET),
                stores + caller_sets,
                "every return stores each output:\n{src}"
            );
            if kw == "chip" {
                let c = only_chip(&r, "the C instance module");
                for pin in c.outputs.iter().filter(|o| {
                    c.nodes[o]
                        .ports
                        .inputs
                        .iter()
                        .all(|p| p.ty != crate::ir::Type::Exec)
                }) {
                    let driver = c
                        .wires
                        .iter()
                        .find(|w| w.target.node_id == *pin)
                        .and_then(|w| c.nodes.get(&w.source.node_id))
                        .map(|n| n.gate_class);
                    assert!(
                        matches!(driver, Some(gc::VAR | gc::VAR_GET)),
                        "{src}\nvalue pin {pin} must read the storage, got {driver:?}"
                    );
                }
            }
            let lr = crate::layout::layout(&r.module);
            assert_emits(&r.module, &lr);
        }
    }
}

/// A `return` in a nested handler ends that handler's run; it is not a path
/// out of the chip's own call, so it must not reach the chip's exec output.
#[test]
fn a_return_in_a_nested_handler_does_not_fire_the_call_continuation() {
    let r = compile(
        "in go: exec\nin go2: exec\nin ia: int\nvar oa: int = 0\nvar ob: int = 0\n\
         chip C(a0: int, t: exec) -> int {\n  on t {\n    if a0 > 0 { return 1 }\n    return 2\n  }\n}\n\
         on go {\n  oa = C(ia, go2)\n  ob = 1\n}\n",
    );
    assert_no_errors(&r);
    let c = only_chip(&r, "the C instance module");
    let exec_in = *c.inputs.last().expect("_exec_in");
    let exec_out = *c.outputs.last().expect("_exec_out");
    let drivers: Vec<_> = c
        .wires
        .iter()
        .filter(|w| w.target.node_id == exec_out)
        .map(|w| w.source.node_id)
        .collect();
    assert_eq!(
        drivers,
        vec![exec_in],
        "the call's exec passes straight through"
    );
}

/// A tuple-typed output among several gets one pin per element, as a record
/// output gets one per field; it had a single pin and read a placeholder.
#[test]
fn tuple_typed_chip_output_gets_a_pin_per_element() {
    let r = compile_ok(
        "in go: exec\nin ia: int\nvar oa: int = 0\nvar ob: int = 0\nvar oc: int = 0\n\
         chip D(a0: int) -> (a: int, t: (int, int), b: int) {\n\
           return (a0, (a0 + 1, a0 + 2), a0 + 3)\n}\n\
         on go {\n  let (u, v, w) = D(ia)\n  oa = u\n  ob = v.1\n  oc = w\n}\n",
    );
    assert_no_code(&r.diagnostics, "WSP001");
    let d = only_chip(&r, "the D instance module");
    assert_outputs_driven_once(d, "D");
    // Pins in declaration order: a, t.0, t.1, b.
    assert_eq!(var_set_value_source(&r, "oa"), Some(d.outputs[0]));
    assert_eq!(var_set_value_source(&r, "ob"), Some(d.outputs[2]));
    assert_eq!(var_set_value_source(&r, "oc"), Some(d.outputs[3]));
    let lr = crate::layout::layout(&r.module);
    assert_emits(&r.module, &lr);
}

/// The same with a tuple inside a record output, and with two returns.
#[test]
fn tuple_inside_a_record_chip_output_and_early_returns() {
    let r = compile_ok(
        "type P = { x: int, t: (int, int) }\nin go: exec\nin ia: int\nin bd: bool\n\
         var ob: int = 0\nvar oc: int = 0\n\
         chip D(a0: int, d0: bool) -> (a: int, p: P, t: (int, int)) {\n\
           if d0 { return (a0, { x: a0 + 1, t: (a0 + 2, a0 + 4) }, (1, a0)) }\n\
           return (2, { x: 3, t: (4, a0) }, (a0, 5))\n}\n\
         on go {\n  let (u, v, w) = D(ia, bd)\n  ob = v.t.1\n  oc = w.0\n}\n",
    );
    let d = only_chip(&r, "the D instance module");
    assert_outputs_driven_once(d, "D");
    // Pins: a, p.x, p.t.0, p.t.1, t.0, t.1.
    assert_eq!(var_set_value_source(&r, "ob"), Some(d.outputs[3]));
    assert_eq!(var_set_value_source(&r, "oc"), Some(d.outputs[4]));
    assert_eq!(
        count_class(d, gc::VAR_SET),
        12,
        "two returns store all six leaves"
    );
    let lr = crate::layout::layout(&r.module);
    assert_emits(&r.module, &lr);
}

/// A top-level `let` read above its declaration outside every mod and chip
/// body is the type check's one WS002; WS021 (whose message is about a body
/// reading the `let` where it is called) is for a body.
#[test]
fn a_root_read_above_a_let_is_one_ws002() {
    for src in [
        "in n: int\nout a = A\nlet A = n * 2",
        "in n: int\nin go: exec\nvar v: int = 0\non go { v = A }\nlet A = n * 2",
    ] {
        let r = compile(src);
        assert_eq!(
            codes(&r.diagnostics),
            ["WS002"],
            "{src}: {:?}",
            r.diagnostics
        );
    }
}

/// WS007 for a chip whose output writes need an exec chain names the cause:
/// several returns, a return beside a nested handler's, or a return beside an
/// `emit`.
#[test]
fn an_unsequenced_chip_return_names_its_cause() {
    for (body, cause) in [
        (
            "  if d0 { return a0 + 1 }\n  return a0 + 3",
            "returns from inside a branch",
        ),
        (
            "  on t { return 5 }\n  return a0 + 1",
            "returns from its body and from a nested `on` handler",
        ),
        (
            "  emit r = a0 + 2\n  return a0 + 1",
            "returns a value and also writes an output with `emit` or `out`",
        ),
    ] {
        let src = format!(
            "in ia: int\nin bd: bool\nin go: exec\nchip C(a0: int, d0: bool, t: exec) -> (r: int) {{\n{body}\n}}\n\
             out o: int = C(ia, bd, go)\n"
        );
        let r = compile(&src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS007" && d.message.contains(&format!("`C` {cause}, so"))),
            "{body}: {:?}",
            r.diagnostics
        );
    }
}

/// A `const` with no value is one error asking for the value, whatever its
/// type: a `const` is a value known while compiling, never a signal.
#[test]
fn a_const_with_no_value_is_one_error() {
    for ty in ["int", "Sig"] {
        for at in ["top", "handler"] {
            let decl = format!("const s: {ty}");
            let src = match at {
                "top" => format!(
                    "type Sig = exec\nin go: exec\nvar n: int = 0\nout o: int = n\n{decl}\non go {{ n = 1 }}"
                ),
                _ => format!(
                    "type Sig = exec\nin go: exec\nvar n: int = 0\nout o: int = n\non go {{ {decl}\nn = 1 }}"
                ),
            };
            let r = compile(&src);
            let errors: Vec<&crate::diagnostic::Diagnostic> = r
                .diagnostics
                .iter()
                .filter(|d| d.severity == crate::diagnostic::Severity::Error)
                .collect();
            assert!(
                errors.len() == 1
                    && errors[0].code == "WSP001"
                    && errors[0]
                        .message
                        .contains(&format!("`const s: {ty}` needs a value")),
                "{at} `{decl}`: {errors:?}"
            );
        }
    }
}

/// `let s: Sig` with no value, for `type Sig = exec` (directly or through
/// another alias), declares a signal exactly as `let s: exec` does: at the top
/// level, in a handler and in a mod body. Any other type still needs its value.
#[test]
fn a_signal_declared_through_an_alias_of_exec_is_its_exec_twin() {
    let head = "type Sig = exec\ntype Sig2 = Sig\nin go: exec\nvar n: int = 0\nout o: int = n\n";
    for body in [
        "let s: @\non go { emit s }\non s { n = n + 1 }",
        "on go {\n  let s: @\n  buffer emit s\n  await s\n  n = n + 1\n}",
        "mod m() {\n  let s: @\n  buffer emit s\n  await s\n  n = n + 1\n}\non go { m() }",
    ] {
        for alias in ["Sig", "Sig2"] {
            for fold in [false, true] {
                let build = |ty: &str| {
                    let src = format!("{head}{}", body.replace('@', ty));
                    let r = if fold {
                        compile_folded(&src)
                    } else {
                        compile(&src)
                    };
                    assert_no_errors(&r);
                    wiring_shape(&r.module)
                };
                assert_eq!(build(alias), build("exec"), "{body} {alias} fold {fold}");
            }
        }
    }
    let e = errs("type Num = int\nin go: exec\nvar n: int = 0\non go {\n  let s: Num\n  n = s\n}");
    assert!(e.contains(&"WSP001".to_string()), "{e:?}");
    let e = errs("in go: exec\nvar n: int = 0\non go {\n  let s: int\n  n = s\n}");
    assert!(e.contains(&"WSP001".to_string()), "{e:?}");
    // With a value, an alias of `exec` names that signal again, as before.
    assert!(
        errs("type Sig = exec\nvar n: int = 0\nin go: exec\nlet s: Sig = go\non s { n = 2 }")
            .is_empty()
    );
}

/// A `const` parameter named like an outer binding (a module `var`, `let` or
/// `const`) is the argument it was given, in a `mod` and in a `chip`: the
/// body read the outer binding.
#[test]
fn a_const_parameter_named_like_an_outer_binding_is_its_argument() {
    use crate::ir::Literal;
    let head = "in go: exec\nvar x: float = 0.0\nout ox: float = x\nvar k: int = 0\nout ok: int = k\n\
                mod half(v: float) -> float { return v / 2 }\n";
    for (outer, decls, body) in [
        (
            "var m: int = 77\nout om: int = m",
            "mod f(m: const int) -> float { return m }",
            "x = f(3)",
        ),
        (
            "const m = 77",
            "mod f(m: const int) -> float { return half(m) }",
            "x = f(3)",
        ),
        (
            "let m = 77",
            "mod f(m: const int) -> int { return m + 1 }",
            "k = f(3)",
        ),
        (
            "const m = 77",
            "chip C(t: exec, m: const int) -> (r: int) { on t { emit r = m + 1 } }",
            "k = C(go, 3)",
        ),
    ] {
        let src = format!("{head}{outer}\n{decls}\non go {{ {body} }}");
        for fold in [false, true] {
            let saved = super::float_slots::save_numbers(&src, fold);
            let three = |l: &Literal| {
                matches!(l, Literal::Int(3 | 4))
                    || matches!(l, Literal::Float(f) if [3.0, 1.5].contains(f))
            };
            let outer_value = |l: &Literal| {
                matches!(l, Literal::Int(78))
                    || matches!(l, Literal::Float(f) if [77.0, 38.5].contains(f))
            };
            assert!(
                saved.iter().any(three),
                "{decls} fold {fold}: the argument is missing\n{saved:?}"
            );
            assert!(
                !saved.iter().any(outer_value),
                "{decls} fold {fold}: read the outer binding\n{saved:?}"
            );
        }
    }
}

/// A `const` record parameter named like an outer binding is the record it
/// was given, in a `chip` as in a `mod`: the chip's body read the outer one
/// (and failed to load without folding).
#[test]
fn a_const_record_parameter_named_like_an_outer_binding_is_its_argument() {
    use crate::ir::Literal;
    let head = "type P = { a: int, b: int }\nin go: exec\nvar k: int = 0\nout ok: int = k\nlet m = { a: 55, b: 66 }\n";
    for (decls, body) in [
        (
            "chip C(m: const P) -> (r: int) { out r = m.b }",
            "k = C({ a: 3, b: 4 })",
        ),
        (
            "mod f(m: const P) -> int { return m.b + 0 }",
            "k = f({ a: 3, b: 4 })",
        ),
    ] {
        let src = format!("{head}{decls}\non go {{ {body} }}");
        for fold in [false, true] {
            let r = if fold {
                compile_folded(&src)
            } else {
                compile(&src)
            };
            assert_no_errors(&r);
            let saved = super::float_slots::save_numbers(&src, fold);
            assert!(
                saved.contains(&Literal::Int(4)),
                "{decls} fold {fold}: the argument is missing\n{saved:?}"
            );
            assert!(
                !saved.contains(&Literal::Int(66)),
                "{decls} fold {fold}: read the outer binding\n{saved:?}"
            );
        }
    }
}

/// A chip's `const` record parameter with no outer binding of its name is the
/// record it was given, as a `mod`'s is: a field, a nested record's field, a
/// string field and an array field, each reaching the chip's output through its
/// wires. An integer given for a `float` field is that float. Every chip form
/// was a placeholder that read zero or `""` (`WSP001`), or `WS044` for the
/// array, and the integer divided as an integer.
#[test]
fn a_chip_const_record_parameter_is_its_argument() {
    use crate::ir::{Literal, Module};
    /// Whether a node holding `want` reaches one of `m`'s outputs through
    /// its wires (or is the output).
    fn drives_output(m: &Module, want: &Literal) -> bool {
        let holds = |id: &crate::ir::NodeId| {
            m.nodes.get(id).is_some_and(|n| {
                n.properties
                    .values()
                    .any(|l| l == want || matches!(l, Literal::Array(xs) if xs.contains(want)))
            })
        };
        let mut seen: Vec<crate::ir::NodeId> = m.outputs.clone();
        let mut i = 0;
        while i < seen.len() {
            let at = seen[i];
            if holds(&at) {
                return true;
            }
            for w in &m.wires {
                if w.target.node_id == at && !seen.contains(&w.source.node_id) {
                    seen.push(w.source.node_id);
                }
            }
            i += 1;
        }
        false
    }
    let head = "type I = { a: int, s: string }
type Q = { b: int, f: float, g: float, s: string, i: I, xs: int[], fs: float[] }
                in go: exec
var n: int = 0
var k: int = 0
var fo: float = 0.0
var so: string = \"\"
";
    let arg = "{ b: 4, f: 2.5, g: 2, s: \"in\", i: { a: 3, s: \"z\" }, xs: [6, 7], fs: [1, 2] }";
    for (out_ty, read, store, want) in [
        ("int", "q.b", "n", Literal::Int(4)),
        ("int", "q.i.a", "n", Literal::Int(3)),
        ("float", "q.f", "fo", Literal::Float(2.5)),
        ("float", "q.g", "fo", Literal::Float(2.0)),
        ("string", "q.s", "so", Literal::String("in".into())),
        ("string", "q.i.s", "so", Literal::String("z".into())),
    ] {
        let src = format!(
            "{head}chip C(q: const Q) -> (r: {out_ty}) {{ out r = {read} }}
on go {{ {store} = C({arg}) }}"
        );
        for fold in [false, true] {
            let r = if fold {
                compile_folded(&src)
            } else {
                compile(&src)
            };
            assert_no_errors(&r);
            assert!(
                !r.diagnostics.iter().any(|d| d.code == "WSP001"),
                "{read} fold {fold}: {:?}",
                r.diagnostics
            );
            // Folded, the constant output is the value the caller stores.
            let stored =
                r.module.nodes.values().any(|n| {
                    n.gate_class == gc::VAR_SET && n.properties.values().any(|l| *l == want)
                });
            assert!(
                stored || drives_output(only_chip(&r, read), &want),
                "{read} fold {fold}: {want:?} does not drive `r`"
            );
            // The array fields it does not read get no container.
            let tables = only_chip(&r, read)
                .nodes
                .values()
                .filter(|n| n.gate_class == gc::ARRAY_VAR)
                .count();
            assert_eq!(tables, 0, "{read} fold {fold}");
        }
    }
    // Array fields are the argument's arrays, read at run time: `[1, 2]` for a
    // `float[]` field is a float table, so `fs[0] / fs[1]` is 0.5.
    for (read, want) in [
        (
            "q.xs[1] + q.xs.length()",
            Literal::Array(vec![Literal::Int(6), Literal::Int(7)]),
        ),
        (
            "q.fs[k] / q.fs[k + 1]",
            Literal::Array(vec![Literal::Float(1.0), Literal::Float(2.0)]),
        ),
    ] {
        let out_ty = if read.contains("fs") { "float" } else { "int" };
        let store = if read.contains("fs") { "fo" } else { "n" };
        let src = format!(
            "{head}chip C(q: const Q) -> (r: {out_ty}) {{ on go {{ emit r = {read} }} }}
on go {{ {store} = C({arg}) }}"
        );
        for fold in [false, true] {
            let r = if fold {
                compile_folded(&src)
            } else {
                compile(&src)
            };
            assert_no_errors(&r);
            let chip = only_chip(&r, read);
            let Literal::Array(items) = &want else {
                unreachable!()
            };
            assert!(
                drives_output(chip, &items[0]),
                "{read} fold {fold}: {want:?} does not drive `r`"
            );
            assert!(
                chip.nodes
                    .values()
                    .any(|n| n.properties.values().any(|l| *l == want)),
                "{read} fold {fold}: the table is not {want:?}"
            );
        }
    }
}

/// A nested record of a chip's `const` record parameter read whole (bound to a
/// `let`, passed to a `mod`, returned, spread, put in a record literal,
/// destructured, chosen by an `if` or a method's receiver) reads every field
/// below it, so its array fields are the argument's tables: the `float` table
/// reaches `r`. Each such read was a `WSP001` placeholder. An array field the
/// body never reaches gets no table.
#[test]
fn a_nested_record_read_whole_keeps_every_field_below_it() {
    use crate::ir::Literal;
    let head = "var x: float = 0.0\nin go: exec\nin k: int\ntype I = { fs: float[], is: int[] }\ntype Q = { i: I, a: int }\n";
    let call = "on go { x = C({ i: { fs: [1.5, 2.5], is: [33] }, a: 1 }) }";
    let fs = Literal::Array(vec![Literal::Float(1.5), Literal::Float(2.5)]);
    let is = Literal::Array(vec![Literal::Int(33)]);
    for (form, decls, body, whole) in [
        ("let", "", "let inner = q.i\nemit r = inner.fs[k]", true),
        (
            "annotated let",
            "",
            "let j: I = q.i\nemit r = j.fs[k]",
            true,
        ),
        (
            "let of a let",
            "",
            "let a = q.i\nlet b = a\nemit r = b.fs[k]",
            true,
        ),
        (
            "mod argument",
            "mod g(i: I, j: int) -> float { return i.fs[j] }\n",
            "emit r = g(q.i, k)",
            true,
        ),
        (
            "returned by a mod",
            "mod pick(i: I) -> I { return i }\n",
            "let p = pick(q.i)\nemit r = p.fs[k]",
            true,
        ),
        ("spread", "", "let j = { ...q.i }\nemit r = j.fs[k]", true),
        (
            "record literal field",
            "",
            "let w = { v: q.i }\nemit r = w.v.fs[k]",
            true,
        ),
        ("destructured", "", "let { fs } = q.i\nemit r = fs[k]", true),
        (
            "if",
            "",
            "emit r = (if k > 0 then q.i else q.i).fs[k]",
            true,
        ),
        (
            "method receiver",
            "mod first(self: I) -> float { return self.fs[1] }\n",
            "emit r = q.i.first()",
            true,
        ),
        ("field path", "", "emit r = q.i.fs[k]", false),
    ] {
        let src = format!(
            "{head}{decls}chip C(q: const Q) -> (r: float) {{ on go {{ {body} }} }}\n{call}"
        );
        for fold in [false, true] {
            let r = if fold {
                compile_folded(&src)
            } else {
                compile(&src)
            };
            assert_no_errors(&r);
            assert!(
                !r.diagnostics.iter().any(|d| d.code == "WSP001"),
                "{form} fold {fold}: {:?}",
                r.diagnostics
            );
            let tables: Vec<&Literal> = only_chip(&r, form)
                .nodes
                .values()
                .flat_map(|n| n.properties.values())
                .collect();
            assert!(
                tables.contains(&&fs),
                "{form} fold {fold}: no `float` table"
            );
            assert_eq!(
                tables.contains(&&is),
                whole,
                "{form} fold {fold}: the `int` table"
            );
            let saved = super::float_slots::save_numbers(&src, fold);
            assert!(
                saved.contains(&Literal::Float(2.5)),
                "{form} fold {fold}: not in the save\n{saved:?}"
            );
        }
    }
}

/// A constant `if` or `match` is decided only by names that are the
/// constants they name: a parameter, local, loop variable, destructured name
/// or `const` record parameter named like an outer `const` is that binding,
/// so each program compiles as its twin with the binding renamed. The outer
/// constant decided the branch, and the other one was never built.
#[test]
fn a_name_shadowing_a_constant_does_not_decide_an_if_or_match() {
    let head = "type P = { a: int, b: int }\nenum Hue { Red, Green }\nin go: exec\nvar n: int = 0\nvar k: int = 3\n\
                var pv: P = { a: 3, b: 4 }\nvar hv: Hue = Hue.Green\n";
    for (outer, prog, renamed) in [
        (
            "const m = 5",
            "mod f(m: int) -> int { if m == 3 { return 30 } return 50 }\non go { n = f(k) }",
            "q",
        ),
        (
            "const m = { a: 5, b: 6 }",
            "mod f(m: P) -> int { if m.a == 3 { return 30 } return 50 }\non go { n = f(pv) }",
            "q",
        ),
        (
            "const m = 5",
            "on go { var m: int = k\n if m == 3 { n = 30 } else { n = 50 } }",
            "q",
        ),
        (
            "const m = 5",
            "on go { for m in 0..k { if m == 5 { n = 1 } } }",
            "q",
        ),
        (
            "const m = 5",
            "on go { let { a: m } = pv\n if m == 3 { n = 30 } else { n = 50 } }",
            "q",
        ),
        (
            "const m = Hue.Red",
            "mod f(m: Hue) -> int { return match m { Hue.Red => 30, _ => 50 } }\non go { n = f(hv) }",
            "q",
        ),
        (
            "const m = { a: 5, b: 6 }",
            "mod f(m: const P) -> int { if m.a == 3 { return 30 } return 50 }\non go { n = f({ a: 3, b: 4 }) }",
            "",
        ),
    ] {
        let src = format!("{head}{outer}\n{prog}");
        // The twin renames the binding, or, with no name given, the outer
        // constant.
        let twin = if renamed.is_empty() {
            format!("{head}{}\n{prog}", outer.replacen("const m", "const mm", 1))
        } else {
            let p = prog
                .replace("(m: ", &format!("({renamed}: "))
                .replace("var m:", &format!("var {renamed}:"));
            let p = p
                .replace("for m in", &format!("for {renamed} in"))
                .replace("{ a: m }", &format!("{{ a: {renamed} }}"));
            let p = p
                .replace("if m", &format!("if {renamed}"))
                .replace("match m", &format!("match {renamed}"));
            format!("{head}{outer}\n{p}")
        };
        assert_ne!(src, twin);
        // A local var is labelled with its name.
        let shape = |r: &LowerResult| {
            let mut s: Vec<String> = super::generic_differential::shape(r)
                .into_iter()
                .map(|l| l.replace("_label=String(\"q\")", "_label=String(\"m\")"))
                .collect();
            s.sort();
            s
        };
        for ((mode, a), (_, b)) in super::generics::both_folds(&src)
            .into_iter()
            .zip(super::generics::both_folds(&twin))
        {
            assert_no_errors(&a);
            assert_eq!(shape(&a), shape(&b), "`{prog}`, {mode}");
        }
    }
}

/// One name, one declaration, at each place (see `scope::top`). A `mod` or
/// `chip` written inside a top-level anonymous chip reads the chip's
/// declarations wherever it is called from: inside the chip, after it, above
/// it, through a `mod` written outside, from a nested chip. A chip's own
/// `const` or `let` is the chip's inside it and the top level's outside it,
/// for value reads and constant-only slots alike. Each row equals its twin
/// with the chip's declaration renamed (`@`), both folds; before, the rows
/// after the chip read the top-level declaration (a string multiplied,
/// constants folded from it) beside a false `WS091`, and a chip `const` was
/// read by the slots outside it. Two declarations of one name in one chip are
/// `WS013`; two at the top level are read by position (see
/// `a_name_the_top_level_declares_twice_is_read_by_position`).
#[test]
fn a_declaration_written_in_an_anonymous_chip_reads_the_chips_names_wherever_called() {
    let outcome = |src: &str, fold: bool| {
        let mut o = without_names(super::shadow_differential::outcome(src, &[], fold));
        // A boundary pin is labelled with the name it carries.
        for c in &mut o.comps {
            if let Some(at) = c.find("PortLabel: ") {
                let end = c[at..].find(',').map_or(c.len(), |e| at + e);
                c.replace_range(at..end, "PortLabel");
            }
        }
        o.comps.sort();
        o
    };
    let head = "in go: exec\nin go2: exec\nin a: int\nvar n: int = 0\nout on1: int = n\nvar s: string = \"\"\nout os: string = s\n";
    let rows: [(&str, &str); 12] = [
        (
            "x",
            "let x = \"four\"\nchip { var @: int = 1\n mod rd() -> int { return @ * 10 }\n on go { @ += 1 } }\non go2 { n = rd() }",
        ),
        (
            "x",
            "let x = 4\nchip { var @: int = 1\n mod rd() -> int { return @ * 10 }\n on go { @ += 1 } }\non go2 { n = rd() }",
        ),
        (
            "x",
            "let x = 4\nchip { var @: int = 1\n chip RC() -> (o: int) { out o = @ * 10 }\n on go { @ += 1 } }\non go2 { n = RC() }",
        ),
        (
            "K",
            "const K = 4\nchip { var @: int = 1\n mod rd() -> int { return @ * 10 }\n on go { @ += 1 } }\n\
             mod outer() -> int { return rd() + 1 }\non go2 { n = outer() }",
        ),
        (
            "x",
            "let x = \"four\"\nchip { var @: int = 1\n mod rd() -> int { return @ * 10 }\n on go { @ += 1\n n = rd() } }\non go2 { n = rd() }",
        ),
        (
            "x",
            "let x = 4\nchip { var @: int = 1\n chip { mod rd() -> int { return @ * 10 }\n on go { @ += 1 } } }\n\
             on go2 { n = rd() + x }",
        ),
        (
            "ch",
            "const ch = \"evt\"\nchip { const @ = \"inner\"\n on go { s = @ } }\non go2 { s = ch\n SendCustomEvent(ch, 1) }\n\
             on CustomEvent(\"evt\") -> (p: int) { n = p }",
        ),
        (
            "K",
            "const K = 3\nchip { const @ = 5\n on go { n = @ } }\non go2 { n = K\n await SleepTicks(_, delay = K)\n n = K * 2 }",
        ),
        (
            "ch",
            "const ch = \"evt\"\nchip { const @ = \"inner\"\n on go { SendCustomEvent(@, 1) } }\n\
             on CustomEvent(\"inner\") -> (p: int) { n = p }\non go2 { SendCustomEvent(ch, 2) }",
        ),
        (
            "x",
            "let x = 3\nchip { let @ = 5\n on go { n = @ } }\non go2 { n = x * 2 }",
        ),
        (
            "K",
            "const K = 3\nchip { const @ = 5\n mod rd() -> int { return @ * 2 }\n on go { n = rd() } }\non go2 { n = rd() + K }",
        ),
        (
            "K",
            "chip { const @ = 5\n on go { n = @ } }\nconst K = 3\non go2 { await SleepTicks(_, delay = K)\n n = K }",
        ),
    ];
    let mut wrong = Vec::new();
    for (name, body) in rows {
        for fold in [false, true] {
            let got = outcome(&format!("{head}{}", body.replace('@', name)), fold);
            let want = outcome(&format!("{head}{}", body.replace('@', "cz")), fold);
            if !got.compiled || got != want {
                let only = |a: &[String], b: &[String]| {
                    a.iter()
                        .filter(|s| !b.contains(s))
                        .cloned()
                        .collect::<Vec<_>>()
                };
                wrong.push(format!(
                    "{body}, fold {fold}: {:?} {:?} {:?} / {:?}; {:?} / {:?}",
                    got.codes,
                    want.codes,
                    only(&got.shape, &want.shape),
                    only(&want.shape, &got.shape),
                    only(&got.comps, &want.comps),
                    only(&want.comps, &got.comps)
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    let codes = |src: &str| outcome(&format!("{head}{src}"), false).codes;
    let twice = "chip { const k = 1\n const k = 2\n on go { n = k } }";
    assert!(
        codes(twice).contains(&"WS013".to_string()),
        "{twice}: {:?}",
        codes(twice)
    );
}

/// A name the top level of a file declares more than once is read by
/// position (see `scope::top::TopNames::visible_decl`): every read, by every
/// reader (value reads, constant-only slots, constant initializers, a `mod`,
/// `chip`, generic or `const mod` body wherever it is called from, folded
/// `if`s, the editor), names the latest declaration visible where it is
/// written, and `WS108` warns at each later declaration. Storage is declared
/// ahead of everything, so a read above every declaration names it; with no
/// storage, a body above every declaration names the first. A library reads
/// its own duplicates the same way however it is imported. Each row equals
/// its twin where the earlier declaration and the reads that see it are
/// renamed (`@`), both folds.
/// Before, a top-level redeclaration was `WS013`; before that, value reads
/// named the latest declaration while constant-only slots and constant
/// initializers named the first.
#[test]
fn a_name_the_top_level_declares_twice_is_read_by_position() {
    let outcome = |src: &str, fold: bool| {
        let mut o = without_names(super::shadow_differential::outcome(src, &[], fold));
        o.comps.sort();
        o
    };
    let head = "in go: exec\nin go2: exec\nvar n: int = 0\nout on1: int = n\nvar n2: int = 0\nout on2: int = n2\n\
                var s: string = \"\"\nout os: string = s\n";
    let recv = "\non CustomEvent(\"a\") -> (p: int) { n2 = p }\non CustomEvent(\"b\") -> (p: int) { n2 = p + 10 }";
    let rows = [
        format!("const @ = \"a\"\nconst k = \"b\"\non go {{ s = k\n SendCustomEvent(k, 1) }}{recv}"),
        format!("let @ = \"a\"\nlet k = \"b\"\non go {{ s = k\n SendCustomEvent(k, 1) }}{recv}"),
        format!(
            "const @ = \"a\"\non go {{ s = @\n SendCustomEvent(@, 1) }}\nconst k = \"b\"\n\
             on go2 {{ s = k\n SendCustomEvent(k, 2) }}{recv}"
        ),
        "const @ = 1\nmod m() -> int { return @ }\nconst k = 2\non go { n = m()\n n2 = k }".into(),
        "const @ = 1\nmod m() -> int { return @ }\non go { n = m() }\nconst k = 2\non go2 { n2 = m() + k }".into(),
        "const @ = 1\nconst j = @ + 10\nconst k = 2\non go { n = j\n n2 = k }".into(),
        "const @ = 1\nconst k = 2\nconst j = k + 10\non go { n = j }".into(),
        "let @ = 1\nlet k = @ + 1\non go { n = k }".into(),
        "const @ = 3\non go { await SleepTicks(_, delay = @)\n n = @ }\nconst k = 5\n\
         on go2 { await SleepTicks(_, delay = k)\n n2 = k }"
            .into(),
        "const @ = 1\non go { if @ == 1 { n = 1 } else { n = 2 } }\nconst k = 2\n\
         on go2 { if k == 1 { n2 = 1 } else { n2 = 2 } }"
            .into(),
        "const @ = 1\non go { n = @ }\nconst k = \"s\"\non go2 { s = k }".into(),
        "const @ = 1\nchip { on go { n = @ } }\nconst k = 2\non go2 { n2 = k }".into(),
        "buffer @: int = @ + 1\non go { n = @ }\nlet k = ServerUptime()\non go2 { n2 = k }".into(),
        "const @ = 1\nconst k = 2\nmod m() -> int { return k }\non go { n = m() }".into(),
        "enum E { A, B }\nconst @ = E.A\non go { match @ { E.A => { n = 1 } E.B => { n = 2 } } }\nconst k = E.B\n\
         on go2 { match k { E.A => { n2 = 1 } E.B => { n2 = 2 } } }"
            .into(),
        "const @ = [1, 2]\non go { n = @[1] }\nconst k = [5, 6]\non go2 { n2 = k[1] }".into(),
        "const @ = 1\nstatic var w: int = @\nconst k = 2\non go { n = w + k }".into(),
        "const @ = 1\nvar w: int = @\nconst k = 2\non go { n = w + k }".into(),
        "let @ = 1\non go { n = @ }\nvar k: int = 5\non go2 { n2 = k\n k = 3 }".into(),
        "var @: int = 4\non go { n = @ }\nlet k = 9\non go2 { n2 = k }".into(),
        "const @ = 1\nchip C(a: int) -> int { return a + @ }\nconst k = 2\non go { n = C(1)\n n2 = k }".into(),
        "const @ = 1\nmod g<T: int | float>(a: T) -> T { return a + @ }\nconst k = 2\non go { n = g(1)\n n2 = k }".into(),
        "const @ = 1\nconst mod f() -> int { return @ }\nconst k = 2\non go { n = f()\n n2 = k }".into(),
        format!("const @ = \"a\"\nmod snd() {{ SendCustomEvent(@, 1) }}\nconst k = \"b\"\non go {{ snd() }}{recv}"),
        "const @ = 1\non go { s = \"v${@}\" }\nconst k = 2\non go2 { s = \"v${k}\" }".into(),
        "let @ = ServerUptime()\non go { n = @ > 10 }\nlet k = \"x\"\non go2 { s = k }".into(),
        "let @ = ServerUptime() > 5.0\non @ { n = 1 }\nlet k = ServerUptime() > 9.0\non k { n2 = 1 }".into(),
        "const @ = 0\nconst a = [10, 20, 30]\non go { n = a[@] }\nconst k = 2\non go2 { n2 = a[k] }".into(),
        "const @ = 1\nchip {\n mod m() -> int { return @ }\n on go { n = m() }\n}\nconst k = 2\non go2 { n2 = k }".into(),
        "const @ = 3\non go { await SleepTicks(_, delay = @)\n n = 1 }\nlet k = ServerUptime()\non go2 { n2 = k }".into(),
        "const @ = 1\nlet k = 2\non go2 { n = k }".into(),
        "var @: int = 0\nlet k = 1\non go2 { n = k }".into(),
        // Storage is no constant: a constant-only slot after it fails in both.
        format!("const @ = \"a\"\nvar k: string = \"chipvar\"\non go2 {{ s = k\n SendCustomEvent(k, 1) }}{recv}"),
        // A `const mod` reads as a `mod` does where it is declared.
        "const @ = 2\nconst k = 10\nconst mod f(x: int) -> int { return x * k }\nconst R = f(3)\non go { n = R }".into(),
        "const @ = 2\nconst mod f(x: int) -> int { return x * @ }\nconst k = 10\nconst R = f(3)\non go { n = R + k }".into(),
        // A body above every declaration reads the first, called from above,
        // between or below them.
        "const mod f(x: int) -> int { return x * @ }\nconst @ = 2\nconst k = 10\nconst R = f(3)\non go { n = R + k }".into(),
        "mod f() -> int { return @ }\non go { n = f() }\nconst @ = 2\nconst k = 10\non go2 { n2 = k }".into(),
        "mod f() -> int { return @ }\nconst @ = 2\non go { n = f() }\nconst k = 10\non go2 { n2 = k + f() }".into(),
        "mod f() -> int { return @ }\nconst @ = 2\nconst k = 10\non go { n = f() + k }".into(),
        "chip C() { on go { n = @ } }\nlet @ = 2\nlet k = 10\nC()\non go2 { n2 = k }".into(),
        "mod g<T: int | float>(a: T) -> T { return a + @ }\nconst @ = 1\nconst k = 2\non go { n = g(1)\n n2 = k }".into(),
        // Storage is declared ahead of everything: above every declaration
        // it is what a read names, a body's too.
        "on go { n = k }\nlet @ = 2\non go2 { n2 = @ }\nvar k: int = 1\non go2 { s = \"${k}\" }".into(),
        "on go { n = @ }\nvar @: int = 4\non go2 { n2 = @ }\nlet k = 9\non go { s = \"${k}\" }".into(),
        "mod f() -> int { return k }\nlet @ = 2\nvar k: int = 1\non go { n = f() }".into(),
        "var @: bool = false\non @ { n = 1 }\nlet k = ServerUptime() > 9.0\non k { n2 = 1 }".into(),
        "on k { n = 1 }\nlet @ = ServerUptime() > 9.0\non @ { n2 = 1 }\nvar k: bool = false".into(),
        // A constant-only slot in a body.
        "const @ = 3\nmod w() { await SleepTicks(_, delay = @)\n n = 1 }\nconst k = 5\non go { w() }\n\
         on go2 { await SleepTicks(_, delay = k)\n n2 = 1 }"
            .into(),
        format!(
            "mod snd() {{ SendCustomEvent(@, 1) }}\nconst @ = \"a\"\nconst k = \"b\"\non go {{ snd() }}\n\
             on go2 {{ SendCustomEvent(k, 2) }}{recv}"
        ),
        format!(
            "const @ = \"a\"\nchip D() {{ on go {{ SendCustomEvent(@, 1) }} }}\nconst k = \"b\"\nD()\n\
             on go2 {{ SendCustomEvent(k, 2) }}{recv}"
        ),
    ];
    let mut wrong = Vec::new();
    let mut failed = 0;
    for row in &rows {
        for fold in [false, true] {
            let got = outcome(&format!("{head}{}", row.replace('@', "k")), fold);
            let want = outcome(&format!("{head}{}", row.replace('@', "k0")), fold);
            if !fold && !want.compiled {
                failed += 1;
            }
            let without = |codes: &[String]| {
                codes
                    .iter()
                    .filter(|c| *c != "WS108")
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let warned = !got.compiled || got.codes.iter().any(|c| c == "WS108");
            if got.compiled != want.compiled
                || got.shape != want.shape
                || got.comps != want.comps
                || without(&got.codes) != want.codes
                || !warned
            {
                wrong.push(format!(
                    "{row}, fold {fold}: {:?} / {:?}",
                    got.codes, want.codes
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert_eq!(failed, 1, "only the storage row fails");

    // A library declaring a name twice reads it by position however it is
    // imported (flat, by name, by two paths), and `WS108` warns once at its
    // later declaration; an importer reads its last declaration.
    let lib_outcome = |src: &str, deps: &[(&str, &str)], fold: bool| {
        let mut o = super::shadow_differential::outcome(src, deps, fold);
        o.comps
            .retain(|c| !c.starts_with("BrickComponentData_TextDisplay"));
        o.comps.sort();
        o
    };
    let fab =
        "const @ = 2\nmod fa() -> int { return @ }\nconst k = 30\nmod fb() -> int { return k }\n";
    let flat = "import \"lib\"\n";
    let libs: [(&str, &str, &str); 7] = [
        (fab, flat, "on go { n = fa() * 1000 + fb() * 10 + k }"),
        (
            fab,
            "import { fa, fb, k } from \"lib\"\n",
            "on go { n = fa() * 1000 + fb() * 10 + k }",
        ),
        (
            fab,
            "import \"pa\"\nimport \"pb\"\n",
            "on go { n = ma() * 1000 + mb() * 10 + k }",
        ),
        ("let @ = 2\nlet k = \"str\"\n", flat, "on go { s = k }"),
        (
            "const @ = \"a\"\nmod sa() { SendCustomEvent(@, 1) }\nconst k = \"b\"\nmod sb() { SendCustomEvent(k, 2) }\n",
            flat,
            "on go { sa()\n sb() }",
        ),
        (
            "buffer @: int = @ + 1\nmod rb() -> int { return @ }\nlet k = 5\nmod rt() -> int { return k }\n",
            flat,
            "on go { n = rt() + rb() }",
        ),
        (
            "mod f0() -> int { return @ }\nconst @ = 2\nconst k = 30\nmod f2() -> int { return k }\n",
            flat,
            "on go { n = f0() * 100 + f2() + k }",
        ),
    ];
    let paths = [
        ("pa", "import \"lib\"\nmod ma() -> int { return fa() }\n"),
        ("pb", "import \"lib\"\nmod mb() -> int { return fb() }\n"),
    ];
    for (lib, imports, body) in libs {
        let src = format!("{imports}{head}{body}{recv}");
        for fold in [false, true] {
            let deps = |name: &str| {
                let mut d = vec![("lib", lib.replace('@', name))];
                d.extend(paths.iter().map(|(p, t)| (*p, t.to_string())));
                d
            };
            let (dk, dk0) = (deps("k"), deps("k0"));
            fn as_refs<'a>(d: &'a [(&'a str, String)]) -> Vec<(&'a str, &'a str)> {
                d.iter().map(|(n, t)| (*n, t.as_str())).collect()
            }
            let got = lib_outcome(&src, &as_refs(&dk), fold);
            let want = lib_outcome(&src, &as_refs(&dk0), fold);
            let warned = got.codes.iter().filter(|c| *c == "WS108").count();
            let without = |codes: &[String]| {
                codes
                    .iter()
                    .filter(|c| *c != "WS108")
                    .cloned()
                    .collect::<Vec<_>>()
            };
            if !got.compiled
                || got.comps != want.comps
                || without(&got.codes) != want.codes
                || warned != 1
            {
                wrong.push(format!(
                    "{lib}{src}, fold {fold}: {:?} / {:?}",
                    got.codes, want.codes
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Compare each `(program, twin)` pair at the save, both folds: the program
/// declares a name twice at the top level and warns `WS108`; the twin renames
/// the declarations the program's reads must not name. Each mismatch is
/// pushed to `wrong`.
fn same_as_renamed_twin(
    rows: &[(String, String)],
    deps: &[(&str, &str)],
    twin_deps: &[(&str, &str)],
    wrong: &mut Vec<String>,
) {
    let outcome = |src: &str, deps: &[(&str, &str)], fold: bool| {
        let mut o = without_names(super::shadow_differential::outcome(src, deps, fold));
        o.comps.sort();
        o
    };
    // The imports a row starts with stay above the declarations it reads.
    let placed = |row: &str| {
        let at = row
            .lines()
            .take_while(|l| l.starts_with("import"))
            .map(|l| l.len() + 1)
            .sum::<usize>();
        format!("{}{DUP_HEAD}{}", &row[..at], &row[at..])
    };
    for (row, twin) in rows {
        for fold in [false, true] {
            let got = outcome(&placed(row), deps, fold);
            let want = outcome(&placed(twin), twin_deps, fold);
            let without = |codes: &[String]| {
                codes
                    .iter()
                    .filter(|c| *c != "WS108")
                    .cloned()
                    .collect::<Vec<_>>()
            };
            if !got.compiled
                || !want.compiled
                || got.shape != want.shape
                || got.comps != want.comps
                || without(&got.codes) != without(&want.codes)
                || !got.codes.iter().any(|c| c == "WS108")
            {
                wrong.push(format!(
                    "{row}\n  fold {fold}: {:?} / twin {:?}",
                    got.codes, want.codes
                ));
            }
        }
    }
}

/// What every `same_as_renamed_twin` row reads and writes.
const DUP_HEAD: &str = "in go: exec\nin go2: exec\nvar n: int = 0\nout on1: int = n\nvar n2: int = 0\nout on2: int = n2\n\
                        var s: string = \"\"\nout os: string = s\n";

/// Each declaration of a name the top level declares twice holds the value
/// of its own initializer, evaluated where it is written: a `const mod` it
/// calls reads the declarations visible where the `const mod` is declared,
/// and a later declaration never reads as the first. Every row equals its
/// renamed twin at the save, both folds.
#[test]
fn a_duplicate_initialized_by_a_const_mod_holds_its_own_value() {
    let recv = "\non CustomEvent(\"a\") -> (p: int) { n2 = p }\non CustomEvent(\"ab\") -> (p: int) { n2 = p + 10 }";
    let f2 = "const mod f() -> int { return A * 2 }";
    let f2t = "const mod f() -> int { return A0 * 2 }";
    let pairs: Vec<(String, String)> = vec![
        (format!("let A = 1\n{f2}\nlet A = f() + 5\non go {{ n = A }}"), format!("let A0 = 1\n{f2t}\nlet A = f() + 5\non go {{ n = A }}")),
        (format!("const A = 1\n{f2}\nlet A = f() + 5\non go {{ n = A }}"), format!("const A0 = 1\n{f2t}\nlet A = f() + 5\non go {{ n = A }}")),
        (format!("const A = 1\n{f2}\nconst A = f() + 5\non go {{ n = A }}"), format!("const A0 = 1\n{f2t}\nconst A = f() + 5\non go {{ n = A }}")),
        (format!("{f2}\nconst A = 1\nconst A = f() + 5\non go {{ n = A }}"), format!("{f2t}\nconst A0 = 1\nconst A = f() + 5\non go {{ n = A }}")),
        (
            "const A = 1\nconst mod f() -> int { return 2 }\nconst A = f()\non go { n = A }".into(),
            "const A0 = 1\nconst mod f() -> int { return 2 }\nconst A = f()\non go { n = A }".into(),
        ),
        (
            "let A = 3\nconst mod f() -> int { return A + 1 }\nconst A = f()\non go { n = A }".into(),
            "let A0 = 3\nconst mod f() -> int { return A0 + 1 }\nconst A = f()\non go { n = A }".into(),
        ),
        (
            "const mod f() -> int { return B * 2 }\nconst A = 1\nconst A = f()\nconst B = 4\non go { n = A }".into(),
            "const mod f() -> int { return B * 2 }\nconst A0 = 1\nconst A = f()\nconst B = 4\non go { n = A }".into(),
        ),
        (
            "const A = 1\nconst mod f() -> int { return A + 2 }\nconst A = f()\non go { await SleepTicks(_, delay = A)\n n = 1 }".into(),
            "const A0 = 1\nconst mod f() -> int { return A0 + 2 }\nconst A = f()\non go { await SleepTicks(_, delay = A)\n n = 1 }".into(),
        ),
        (
            format!("const A = \"a\"\nconst mod f() -> string {{ return \"${{A}}b\" }}\nconst A = f()\non go {{ SendCustomEvent(A, 1) }}{recv}"),
            format!("const A0 = \"a\"\nconst mod f() -> string {{ return \"${{A0}}b\" }}\nconst A = f()\non go {{ SendCustomEvent(A, 1) }}{recv}"),
        ),
        (
            format!("const A = 1\n{f2}\nconst A = f() + 5\nmod m() -> int {{ return A }}\non go {{ n = m() }}"),
            format!("const A0 = 1\n{f2t}\nconst A = f() + 5\nmod m() -> int {{ return A }}\non go {{ n = m() }}"),
        ),
        (
            format!("const A = 1\n{f2}\nconst A = f() + 5\nchip C() {{ on go {{ n = A }} }}\nC()"),
            format!("const A0 = 1\n{f2t}\nconst A = f() + 5\nchip C() {{ on go {{ n = A }} }}\nC()"),
        ),
        (
            format!("const A = 1\n{f2}\nconst A = f() + 5\nmod g<T: int | float>(a: T) -> T {{ return a + A }}\non go {{ n = g(1) }}"),
            format!("const A0 = 1\n{f2t}\nconst A = f() + 5\nmod g<T: int | float>(a: T) -> T {{ return a + A }}\non go {{ n = g(1) }}"),
        ),
        (
            format!("const A = 1\n{f2}\nmod m() -> int {{ return A }}\nconst A = f() + 5\non go {{ n = m()\n n2 = A }}"),
            format!("const A0 = 1\n{f2t}\nmod m() -> int {{ return A0 }}\nconst A = f() + 5\non go {{ n = m()\n n2 = A }}"),
        ),
        (
            "let A = [1, 2]\non go { n = A[1] }\nlet A = [3, 4]\non go2 { n2 = A[1] }".into(),
            "let A0 = [1, 2]\non go { n = A0[1] }\nlet A = [3, 4]\non go2 { n2 = A[1] }".into(),
        ),
        // Three declarations in a chain, each read by the next one's `const mod`.
        (
            "const A = 1\nconst mod f() -> int { return A * 2 }\nconst A = f() + 5\n\
             const mod g() -> int { return A * 10 }\nconst A = g() + 1\non go { n = A }"
                .into(),
            "const A0 = 1\nconst mod f() -> int { return A0 * 2 }\nconst A1 = f() + 5\n\
             const mod g() -> int { return A1 * 10 }\nconst A = g() + 1\non go { n = A }"
                .into(),
        ),
        // A `const mod` calling a `const mod`, each reading at its own position.
        (
            "const A = 1\nconst mod g() -> int { return A * 3 }\nconst A = 5\n\
             const mod f() -> int { return g() + A }\nconst A = f()\non go { n = A }"
                .into(),
            "const A0 = 1\nconst mod g() -> int { return A0 * 3 }\nconst A1 = 5\n\
             const mod f() -> int { return g() + A1 }\nconst A = f()\non go { n = A }"
                .into(),
        ),
        // The first declaration is the one a `const mod` gives.
        (
            "const mod f() -> int { return 2 }\nconst A = f()\non go { n = A }\nconst A = 3\non go2 { n2 = A }".into(),
            "const mod f() -> int { return 2 }\nconst A0 = f()\non go { n = A0 }\nconst A = 3\non go2 { n2 = A }".into(),
        ),
    ];
    let mut wrong = Vec::new();
    same_as_renamed_twin(&pairs, &[], &[], &mut wrong);
    assert!(
        wrong.is_empty(),
        "{} of {} rows:\n{}",
        wrong.len(),
        pairs.len() * 2,
        wrong.join("\n")
    );
}

/// A `var` or `static var` initializer reads the declaration visible before
/// it, as a `let` initializer does, never the storage it initializes; a
/// `buffer` initializer reads the buffer itself (the previous tick). Every
/// row equals its renamed twin at the save, both folds.
#[test]
fn a_var_initializer_reads_the_declaration_before_it() {
    let pairs: Vec<(String, String)> = [
        ("let v = 5\nvar v: int = v\non go { n = v }", "let v0 = 5\nvar v: int = v0\non go { n = v }"),
        ("let t = 3\nvar t: int = t + 1\non go { n = t }", "let t0 = 3\nvar t: int = t0 + 1\non go { n = t }"),
        ("let t = 3\nstatic var t: int = t + 1\non go { n = t }", "let t0 = 3\nstatic var t: int = t0 + 1\non go { n = t }"),
        (
            "const v = 5\nvar v: int = v * 2\non go { n = v\n v = v + 1 }",
            "const v0 = 5\nvar v: int = v0 * 2\non go { n = v\n v = v + 1 }",
        ),
        (
            "let v = 2\nmod m() -> int { return v }\nvar v: int = v * 3\non go { n = m() + v }",
            "let v0 = 2\nmod m() -> int { return v0 }\nvar v: int = v0 * 3\non go { n = m() + v }",
        ),
        (
            "const v = 2\nconst mod f() -> int { return v * 5 }\nvar v: int = f()\non go { n = v }",
            "const v0 = 2\nconst mod f() -> int { return v0 * 5 }\nvar v: int = f()\non go { n = v }",
        ),
        (
            "let v = \"x\"\nvar v: string = \"${v}y\"\non go { s = v }",
            "let v0 = \"x\"\nvar v: string = \"${v0}y\"\non go { s = v }",
        ),
        (
            "var v: int = 4\non go { n = v }\nlet v = 9\non go2 { n2 = v }",
            "var v: int = 4\non go { n = v }\nlet v0 = 9\non go2 { n2 = v0 }",
        ),
        ("let t = 3\nbuffer t: int = t + 1\non go { n = t }", "let t0 = 3\nbuffer t: int = t + 1\non go { n = t }"),
        (
            "buffer t: int = t + 1\non go { n = t }\nlet t = 3\non go2 { n2 = t }",
            "buffer t: int = t + 1\non go { n = t }\nlet t0 = 3\non go2 { n2 = t0 }",
        ),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    let mut wrong = Vec::new();
    same_as_renamed_twin(&pairs, &[], &[], &mut wrong);
    let storage = "in go: exec\nvar v: int = 1\nvar v: int = v + 1\non go { v = 2 }";
    let codes = super::shadow_differential::outcome(storage, &[], true).codes;
    if !codes.contains(&"WS013".to_string()) {
        wrong.push(format!("{storage}: {codes:?}"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A library declaring a name twice reads it by position however the
/// program reaches it (a named import beside a flat one, several named
/// imports, an alias, three paths), since its declarations keep their order
/// in their own file; the program reads the library's last declaration, and
/// `WS108` warns once, at the later declaration, naming the earlier one's
/// line. A library imported both flat and `import * as` gets one warning.
#[test]
fn a_library_reached_by_several_imports_reads_its_duplicates_in_source_order() {
    let lib =
        "const K = 2\nmod fa() -> int { return K }\nconst K = 30\nmod fb() -> int { return K }\n";
    let lib_t =
        "const K0 = 2\nmod fa() -> int { return K0 }\nconst K = 30\nmod fb() -> int { return K }\n";
    let above = "mod f0() -> int { return K }\nconst K = 2\nmod fa() -> int { return K }\nconst K = 30\n\
                 mod fb() -> int { return K }\n";
    let above_t = "mod f0() -> int { return K0 }\nconst K0 = 2\nmod fa() -> int { return K0 }\nconst K = 30\n\
                   mod fb() -> int { return K }\n";
    let all = "on go { n = fa() * 1000 + fb() * 10 + K }";
    let paths = [
        (
            "NK",
            "import { K } from \"L\"\nmod mk() -> int { return K }\n",
        ),
        ("PA", "import \"L\"\nmod ma() -> int { return fa() }\n"),
        (
            "PB",
            "import { fb } from \"L\"\nmod mb() -> int { return fb() }\n",
        ),
    ];
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            lib,
            lib_t,
            "import { K } from \"L\"\nimport \"L\"\n".into(),
            all,
        ),
        (
            lib,
            lib_t,
            "import \"L\"\nimport { K } from \"L\"\n".into(),
            all,
        ),
        (
            lib,
            lib_t,
            "import { fb } from \"L\"\nimport \"L\"\n".into(),
            all,
        ),
        (
            lib,
            lib_t,
            "import { fa } from \"L\"\nimport \"L\"\n".into(),
            all,
        ),
        (
            lib,
            lib_t,
            "import { fb } from \"L\"\nimport { fa } from \"L\"\nimport { K } from \"L\"\n".into(),
            all,
        ),
        (
            lib,
            lib_t,
            "import { K } from \"L\"\nimport { fa } from \"L\"\n".into(),
            "on go { n = fa() * 1000 + K }",
        ),
        (
            lib,
            lib_t,
            "import { fa } from \"L\"\nimport { fb } from \"L\"\n".into(),
            "on go { n = fa() * 1000 + fb() * 10 }",
        ),
        (
            lib,
            lib_t,
            "import { K as KK } from \"L\"\nimport { fa, fb } from \"L\"\n".into(),
            "on go { n = fa() * 1000 + fb() * 10 + KK }",
        ),
        (
            lib,
            lib_t,
            "import { K as KK } from \"L\"\nimport \"L\"\n".into(),
            "on go { n = fa() * 1000 + fb() * 10 + KK + K }",
        ),
        (
            lib,
            lib_t,
            "import \"NK\"\nimport \"L\"\n".into(),
            "on go { n = fa() * 1000 + fb() * 10 + mk() }",
        ),
        (
            lib,
            lib_t,
            "import \"PB\"\nimport { K } from \"L\"\nimport \"PA\"\n".into(),
            "on go { n = ma() * 1000 + mb() * 10 + K }",
        ),
        (
            above,
            above_t,
            "import { K } from \"L\"\nimport \"L\"\n".into(),
            "on go { n = f0() * 100 + fa() * 10 + fb() + K }",
        ),
        (
            above,
            above_t,
            "import { fb } from \"L\"\nimport { f0 } from \"L\"\n".into(),
            "on go { n = f0() * 100 + fb() }",
        ),
    ];
    let mut wrong = Vec::new();
    for (l, l_t, imports, body) in &rows {
        let deps: Vec<(&str, &str)> = [("L", *l)].into_iter().chain(paths).collect();
        let twin_deps: Vec<(&str, &str)> = [("L", *l_t)].into_iter().chain(paths).collect();
        let main = format!("{imports}{body}");
        same_as_renamed_twin(
            &[(main.clone(), main.clone())],
            &deps,
            &twin_deps,
            &mut wrong,
        );
        let line_of = |nth: usize| {
            l.lines()
                .enumerate()
                .filter(|(_, x)| x.starts_with("const K"))
                .nth(nth)
                .map(|(i, _)| i + 1)
        };
        let (first, last) = (line_of(0).unwrap_or(0), line_of(1).unwrap_or(0));
        match super::declaring_scope::compile_main(
            &format!("{imports}{DUP_HEAD}{body}"),
            &deps,
            true,
        ) {
            Ok(out) => {
                let warned: Vec<_> = out
                    .diagnostics
                    .iter()
                    .filter(|d| d.code == "WS108")
                    .collect();
                let placed = |d: &&crate::diagnostic::Diagnostic| {
                    d.range.file.ends_with("L.ws")
                        && d.range.start.line as usize == last
                        && d.message.contains(&format!("on line {first}:"))
                };
                if warned.len() != 1 || !warned.iter().all(placed) {
                    let shown: Vec<_> = warned
                        .iter()
                        .map(|d| format!("{}:{} {}", d.range.file, d.range.start.line, d.message))
                        .collect();
                    wrong.push(format!("{main}: WS108 {shown:#?}"));
                }
            }
            Err(codes) => wrong.push(format!("{main}: {codes:?}")),
        }
    }
    // Flat and `import * as`: one warning, which says what both read.
    let both = "import \"L\"\nimport * as ns from \"L\"\nin go: exec\nvar n: int = 0\non go { n = fa() * 1000 + ns.fb() * 10 + K }";
    match super::declaring_scope::compile_main(both, &[("L", lib)], true) {
        Ok(out) => {
            let warned: Vec<_> = out
                .diagnostics
                .iter()
                .filter(|d| d.code == "WS108")
                .map(|d| d.message.clone())
                .collect();
            if warned.len() != 1 || !warned[0].contains("import * as") {
                wrong.push(format!("flat and namespace: {warned:#?}"));
            }
        }
        Err(codes) => wrong.push(format!("flat and namespace: {codes:?}")),
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
