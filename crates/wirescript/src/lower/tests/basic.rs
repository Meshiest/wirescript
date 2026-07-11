use super::*;

#[test]
fn on_local_exec_signal_fires_from_emit_in_another_handler() {
    // `on sig` must trigger when `emit sig` runs in a *different* handler, even
    // though `on sig` appears after the emitting handler in source. Previously
    // the signal's binding was only created after all handlers were lowered, so
    // `on sig` silently produced nothing. The pre-declared hub carries the emit
    // to the listener; with a single emitter the hub union is spliced out, so
    // assert the connectivity itself: trig's chain must reach the `on sig` body.
    let src = "in trig: exec\n\
               let sig: exec\n\
               static var n: int = 0\n\
               on trig { emit sig }\n\
               on sig { n = n + 1 }";
    let r = compile(src);
    assert_no_errors(&r);
    let trig = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let body = find_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment");
    assert!(
        wired_reachable(&r, trig, body),
        "emit sig in `on trig` must drive the `on sig` body; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn body_local_exec_signal_emit_await_connects() {
    // `let jump: exec` declared inside a handler must wire emit->hub->await,
    // not leave the await trigger as an _Unsupported placeholder.
    let src = "in run: exec\n\
               on run {\n\
                 let jump: exec\n\
                 emit jump\n\
                 await jump\n\
                 PrintToConsole(\"jump\")\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "await trigger must not lower to _Unsupported; gate classes: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // Single emitter: no degenerate Union hub should survive, and the handler's
    // input must drive the post-await continuation. The same-chain emit routes
    // through a Var_Set(armed = true) *before* the (spliced) hub so the
    // awaiting Var_Get can't race the arm.
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Exec_Union"),
        "a single-emitter signal must not keep a pass-through Union; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    let run = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let cont = find_gate(&r, "BrickComponentType_WireGraph_Exec_PrintToConsole");
    assert!(
        wired_reachable(&r, run, cont),
        "emit->await continuation must be exec-wired from the handler input; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn buffered_emit_parses() {
    // `buffer(1) emit sig` must parse as a buffered emit (the `buffer(` form
    // is the emit modifier; `buffer name = ...` stays the value declaration).
    let src = "in run: exec\n\
               on run {\n\
                 let sig: exec\n\
                 buffer(1) emit sig\n\
                 await sig\n\
                 PrintToConsole(\"x\")\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
}

#[test]
fn buffered_emit_inserts_buffer_and_breaks_cycle() {
    // A back-edge `buffer(1) emit loop` after `await loop` must (a) route the
    // emit's exec through a BufferTicks, and (b) leave the loop SCC with a
    // barrier so analyze_cycles reports no WS005.
    let src = "in run: exec\n\
               on run {\n\
                 let loop: exec\n\
                 emit loop\n\
                 await loop\n\
                 buffer(1) emit loop\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraphPseudo_BufferTicks"),
        "buffered emit should insert a BufferTicks; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    let cyc = crate::analyze::analyze_cycles(&r.module);
    assert!(
        !cyc.diagnostics.iter().any(|d| d.code == "WS005"),
        "the buffer must sit inside the loop SCC so no WS005 fires: {:?}",
        cyc.diagnostics
    );
    assert!(
        !cyc.strongly_connected.is_empty(),
        "the back-edge emit must actually close a cycle"
    );
}

#[test]
fn scalar_payload_ferries_through_local_signal() {
    // `emit sig = 7` on a local signal must write a hidden payload store
    // (Var_Set), and `let x = await sig` must read it back (Var_Get) — the
    // value rides the signal instead of being dropped.
    let src = "in run: exec\n\
               on run {\n\
                 let sig: exec\n\
                 emit sig = 7\n\
                 let x = await sig\n\
                 PrintToConsole(\"${x}\")\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    // Stores: payload var + armed flag. The payload must be written on the
    // emit path and read on the resumed path.
    let vars = gate_count(&r, "BrickComponentType_WireGraphPseudo_Var");
    assert!(
        vars >= 2,
        "expected payload store + armed flag, got {vars} PseudoVars"
    );
    let gets = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Get");
    assert!(
        gets >= 2,
        "expected armed read + payload read, got {gets} Var_Gets"
    );
    // And the continuation must still be reachable from the handler input.
    let run = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let cont = find_gate(&r, "BrickComponentType_WireGraph_Exec_PrintToConsole");
    assert!(
        wired_reachable(&r, run, cont),
        "continuation must be exec-wired"
    );
}

#[test]
fn bare_buffer_emit_defaults_to_one_tick() {
    // `buffer emit sig` (no parens) = `buffer(1) emit sig`.
    let src = "in run: exec\n\
               on run {\n\
                 let sig: exec\n\
                 emit sig\n\
                 await sig\n\
                 buffer emit sig\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    let buf = find_gate(&r, "BrickComponentType_WireGraphPseudo_BufferTicks");
    let node = &r.module.nodes[&buf];
    assert_eq!(
        node.properties.get(&*crate::intern::sym::TICKS_TO_WAIT),
        Some(&crate::ir::Literal::Int(1)),
        "bare buffer should default to 1 tick; props: {:?}",
        node.properties
    );
}

#[test]
fn buffered_emit_seconds_uses_buffer_seconds() {
    // An `s` unit on the duration selects the BufferSeconds gate.
    let src = "in run: exec\n\
               on run {\n\
                 let sig: exec\n\
                 emit sig\n\
                 await sig\n\
                 buffer(0.5s) emit sig\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraphPseudo_BufferSeconds"),
        "`0.5s` should select BufferSeconds; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
}

#[test]
fn buffered_emit_variable_delay_wires_port() {
    // A non-constant delay must wire into the buffer's TicksToWait port
    // instead of baking a property.
    let src = "in run: exec\n\
               var d: int = 3\n\
               on run {\n\
                 let sig: exec\n\
                 emit sig\n\
                 await sig\n\
                 buffer(d) emit sig\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    use crate::ir::port_registry::WirePort;
    assert!(
        r.module.nodes.iter().any(|(id, n)| {
            n.gate_class == "BrickComponentType_WireGraphPseudo_BufferTicks"
                && r.module
                    .wires
                    .iter()
                    .any(|w| w.target.node_id == *id && w.target.port == WirePort::TicksToWait)
        }),
        "a variable delay must wire into TicksToWait; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn sum_loop_compiles_with_buffered_payload() {
    // The canonical payload-ferry loop: state rides the signal, the back-edge
    // buffers one tick, and the whole thing forms a legal (barriered) cycle.
    let src = "in run: exec\n\
      mod sumItems(arr: int[]) -> int {\n\
        let loop: exec\n\
        emit loop = { sum: 0, index: 0 }\n\
        let { sum, index } = await loop\n\
        if index < arr.length() {\n\
          buffer(1) emit loop = { sum: sum + arr[index], index: index + 1 }\n\
        } else { return sum }\n\
      }\n\
      var numbers: int[] = [1, 2, 3]\n\
      on run { BroadcastChatMessage(\"${sumItems(numbers)}\") }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "loop must lower fully; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraphPseudo_BufferTicks"),
        "back-edge must buffer"
    );
    let cyc = crate::analyze::analyze_cycles(&r.module);
    assert!(
        !cyc.diagnostics.iter().any(|d| d.code == "WS005"),
        "loop cycle must contain its buffer barrier: {:?}",
        cyc.diagnostics
    );
    assert!(
        !cyc.strongly_connected.is_empty(),
        "the back-edge must actually close a cycle"
    );
}

#[test]
fn same_signal_name_in_two_mods_stays_separate() {
    // Two mods each declaring `let loop: exec` must get *separate* signals —
    // previously the second mod reused the first's hub (keyed by bare name),
    // its await lowered to _Unsupported, and its emits cross-wired into the
    // first mod's loop.
    let src = "in run: exec\n\
      mod first(arr: int[]) {\n\
        var i = 0\n\
        let loop: exec\n\
        emit loop\n\
        await loop\n\
        if i < arr.length() {\n\
          i += 1\n\
          buffer emit loop\n\
        }\n\
      }\n\
      mod second(arr: string[]) {\n\
        var i = 0\n\
        let loop: exec\n\
        emit loop\n\
        await loop\n\
        if i < arr.length() {\n\
          BroadcastChatMessage(arr[i])\n\
          i += 1\n\
          buffer emit loop\n\
        }\n\
      }\n\
      array nums: int[] = [1, 2]\n\
      array names: string[] = [\"a\", \"b\"]\n\
      on run {\n\
        first(nums)\n\
        second(names)\n\
      }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "both mods' awaits must resolve; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // Each mod keeps its own buffered back-edge.
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_BufferTicks"),
        2,
        "each mod's loop needs its own buffer"
    );
    // Both loop bodies are driven from the handler input.
    let run = find_gate(&r, "BrickComponentType_Internal_MicrochipInput");
    let chat = find_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage",
    );
    assert!(
        wired_reachable(&r, run, chat),
        "second mod's loop body must be exec-wired"
    );
    let cyc = crate::analyze::analyze_cycles(&r.module);
    assert!(
        !cyc.diagnostics.iter().any(|d| d.code == "WS005"),
        "both cycles carry their buffer: {:?}",
        cyc.diagnostics
    );
}

#[test]
fn handler_local_array_var_rebuilds_without_bogus_var_set() {
    // `var nums = [1,2,3]` inside a handler: the re-init on scope entry must
    // use the array rebuild (clear + push) — the generic Var_Set reset wired a
    // `VarRef` source port that Pseudo_ArrayVar doesn't have (in-game:
    // "Wire source port VarRef does not exist in source component").
    let src = "in run: exec\n\
               on run {\n\
                 var nums = [1, 2, 3]\n\
                 let x = nums[0]\n\
                 PrintToConsole(\"${x}\")\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "_Unsupported"),
        "array literal init must lower; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // No wire may leave a Pseudo_ArrayVar through a port it doesn't have.
    use crate::ir::port_registry::WirePort;
    let array_vars: std::collections::HashSet<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .map(|(id, _)| *id)
        .collect();
    for w in &r.module.wires {
        if array_vars.contains(&w.source.node_id) {
            assert_eq!(
                w.source.port,
                WirePort::ArrayVarRef,
                "ArrayVar only exposes ArrayVarRef; found a wire leaving via {:?}",
                w.source.port.as_str()
            );
        }
    }
    // The runtime re-init rebuilds via clear + pushes.
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Clear"),
        "array re-init should clear then push"
    );
}

#[test]
fn loop_mod_continuation_waits_for_return() {
    // The caller's continuation must fire only on `return`, not once per
    // iteration: previously the back-edge branch (`buffer emit loop` last in
    // the then-block) fell through the if-join into the mod's end exec, so
    // the caller printed per iteration ("Sum: 0" three times, then "Sum: 6").
    let src = "in run: exec\n\
      mod sumItems(arr: int[]) -> int {\n\
        var sum = 0\n\
        var index = 0\n\
        let loop: exec\n\
        emit loop\n\
        await loop\n\
        if index < arr.length() {\n\
          sum += arr[index]\n\
          index += 1\n\
          buffer emit loop\n\
        } else {\n\
          return sum\n\
        }\n\
      }\n\
      array nums: int[] = [1, 2, 3]\n\
      on run { BroadcastChatMessage(\"Sum: ${sumItems(nums)}\") }";
    let r = compile(src);
    assert_no_errors(&r);
    // After emit-terminates-block + dead-join pruning, the only Union left is
    // the armed-emit union (kick + back-edge): the if-join lost both arms
    // (then ends in emit, else in return) and the mod-end union collapsed to
    // the single return path.
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Exec_Union"),
        1,
        "only the armed-emit union should remain; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| (n.gate_class, n.note))
            .collect::<Vec<_>>()
    );
    // The caller's chat message has exactly one exec source — the return path.
    use crate::ir::port_registry::WirePort;
    let chat = find_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage",
    );
    let exec_ins: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.node_id == chat && w.target.port == WirePort::Exec)
        .collect();
    assert_eq!(
        exec_ins.len(),
        1,
        "caller continuation must have a single exec source: {exec_ins:?}"
    );
    let src_node = &r.module.nodes[&exec_ins[0].source.node_id];
    assert_ne!(
        src_node.gate_class, "BrickComponentType_WireGraph_Exec_Union",
        "continuation should come straight from the return path, not a join union"
    );
}

#[test]
fn empty_program() {
    let r = compile("");
    assert!(r.diagnostics.is_empty());
    assert!(r.module.nodes.is_empty());
}

#[test]
fn var_creates_node() {
    let r = compile("var n: int = 0");
    assert!(!r.module.nodes.is_empty());
    let has_var = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var");
    assert!(has_var, "expected a Pseudo_Var node");
}

#[test]
fn vector_arithmetic_lowers_to_math_gates() {
    // vec ⊕ vec lowers to the shared PrimMathVariant math gates.
    let r = compile(
        "let a = Vec(1.0, 2.0, 3.0)\nlet b = Vec(4.0, 5.0, 6.0)\nout s = a + b\nout d = a - b",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "vec + vec should lower to MathAdd"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathSubtract"),
        "vec - vec should lower to MathSubtract"
    );
}

#[test]
fn array_methods_lower_to_their_gates() {
    let src = "array a: int[]\narray b: int[]\nin t: exec\non t {\n  \
        a.push(1)\n  a.insert(0, 9)\n  a.sort()\n  a.reverse()\n  a.swap(0, 1)\n  \
        a.fill(3)\n  a.resize(4, 0)\n  let s = a.sum()\n  let lo = a.min()\n  \
        let hi = a.max()\n  let av = a.average()\n  let i = a.find(3)\n  \
        b.append(a)\n  b.copyFrom(a)\n  b.slice(a, 0, 2)\n}";
    let r = compile(src);
    assert_no_errors(&r);
    for class in [
        "BrickComponentType_WireGraph_Exec_ArrayVar_Insert",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Sort",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Reverse",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Swap",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Fill",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Resize",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Sum",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Min",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Max",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Average",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Find",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Append",
        "BrickComponentType_WireGraph_Exec_ArrayVar_CopyFrom",
        "BrickComponentType_WireGraph_Exec_ArrayVar_Slice",
    ] {
        assert!(has_gate(&r, class), "missing gate {class}");
    }
}

#[test]
fn array_constant_initializer_populates_node() {
    // `array a: int[] = [1, 2, -3]` carries its literals as an InitialValue
    // property the emitter writes into the ArrayVar.
    let r = compile("array a: int[] = [1, 2, -3]\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .expect("expected an ArrayVar node");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::Array(lits)) => {
            assert_eq!(lits.len(), 3);
            assert!(matches!(lits[0], crate::ir::Literal::Int(1)));
            assert!(matches!(lits[2], crate::ir::Literal::Int(-3)));
        }
        other => panic!("expected InitialValue array literal, got {other:?}"),
    }
}

#[test]
fn var_array_desugars_to_array_var() {
    // `var foo: T[]` is an array: it lowers to an ArrayVar gate (not a
    // Pseudo_Var), supports a constant initializer, and the methods work.
    let r = compile(
        "var va: int[] = [7, 8]\nin t: exec\non t {\n  va.push(9)\n  let n = va.length()\n}",
    );
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .expect("a var array should be an ArrayVar gate");
    assert!(
        node.properties
            .contains_key(&crate::intern::intern("InitialValue")),
        "var array initializer should populate the gate"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Push"),
        "var array push should lower to the ArrayVar push gate"
    );
}

#[test]
fn find_returns_record_unwrapping_to_index() {
    // `find` is a record { Index, Found, Value }: fields are accessible, and a
    // bare result auto-unwraps to the int Index (its default).
    let src = "array a: int[]\nvar found: bool\nvar at: int\nin t: exec\n\
        on t {\n  let r = a.find(3)\n  at = r.Index\n  found = r.Found\n  \
        at = a.find(3)\n}";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Find"),
        "expected a Find gate"
    );
}

#[test]
fn find_result_field_access_inline_without_binding() {
    // Regression: `.Found` / `.Index` directly on an inline `find()` (no `let`
    // binding) must lower the Find gate and read its output — not fall through
    // to an `_Unsupported` placeholder that drops the whole call. Previously
    // `arr.find(x).Found` emitted nothing because `obj` (the call) was never
    // lowered when the field didn't name a swizzle/index port.
    let src = "array a: int[]\nvar hit: bool\nin t: exec\n\
        on t {\n  hit = !a.find(3).Found\n}";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Find"),
        "inline find().Found must still emit the Find gate"
    );
    assert!(
        !has_gate(&r, "_Unsupported"),
        "inline find().Found must not degrade to an _Unsupported placeholder"
    );
}

#[test]
fn array_var_type_inferred_without_annotation() {
    // `var foo = [10, 20, 30]` (no `: int[]`) infers an array type and bakes
    // its literals into an ArrayVar gate.
    let r = compile("var foo = [10, 20, 30]\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .expect("an inferred-type array var should be an ArrayVar gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::Array(lits)) => assert_eq!(lits.len(), 3),
        other => panic!("expected InitialValue array literal, got {other:?}"),
    }
}

#[test]
fn scalar_var_type_inferred_without_annotation() {
    // `var s = ""` (no `: string`) infers a string var: the Var gate carries
    // a string InitialValue and its uses type as string (`==` resolves).
    let r = compile("var s = \"\"\nout ready = s == \"go\"\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("inferred string var should be a Var gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::String(s)) => assert_eq!(s, ""),
        other => panic!("expected string InitialValue, got {other:?}"),
    }
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"),
        "string == on an inferred var should lower to CompareEqual"
    );
}

#[test]
fn vector_var_init_folds_to_constant() {
    // `var v = Vec(1.0, 2.0, 3.0)` folds the constructor into the Var gate's
    // InitialValue — no MakeVector gate, no dropped initializer.
    let r = compile("var v = Vec(1.0, 2.0, 3.0)\nout o = v\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("vector var should be a Var gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::Vector { x, y, z }) => {
            assert_eq!((*x, *y, *z), (1.0, 2.0, 3.0));
        }
        other => panic!("expected vector InitialValue, got {other:?}"),
    }
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_MakeVector"),
        "constant Vec initializer should not spawn a MakeVector gate"
    );
}

#[test]
fn rotator_var_init_folds_to_constant() {
    let r = compile("var rot = Rotation(0.0, 90.0, 0.0)\nout o = rot\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("rotator var should be a Var gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::Rotator { pitch, yaw, roll }) => {
            assert_eq!((*pitch, *yaw, *roll), (0.0, 90.0, 0.0));
        }
        other => panic!("expected rotator InitialValue, got {other:?}"),
    }
}

#[test]
fn color_var_init_folds_to_constant() {
    // Color(r, g, b) is linear 0–1 with alpha defaulting to opaque.
    let r = compile("var tint = Color(1.0, 0.5, 0.0)\nout o = tint\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("color var should be a Var gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::LinearColor { r, g, b, a }) => {
            assert_eq!((*r, *g, *b, *a), (1.0, 0.5, 0.0, 1.0));
        }
        other => panic!("expected linear color InitialValue, got {other:?}"),
    }
}

#[test]
fn quat_var_defaults_to_identity() {
    let r = compile("var q: quat\nout o = q\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("quat var should be a Var gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::Quat { x, y, z, w }) => {
            assert_eq!((*x, *y, *z, *w), (0.0, 0.0, 0.0, 1.0));
        }
        other => panic!("expected identity quat InitialValue, got {other:?}"),
    }
}

#[test]
fn vector_array_init_folds_elements() {
    // Constant Vec(…) elements are valid array initializers and bake into
    // the ArrayVar's InitialValue list.
    let r = compile("array pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]\n");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .expect("vector array should be an ArrayVar gate");
    match node.properties.get(&crate::intern::intern("InitialValue")) {
        Some(crate::ir::Literal::Array(lits)) => {
            assert_eq!(lits.len(), 2);
            assert!(matches!(lits[1], crate::ir::Literal::Vector { .. }));
        }
        other => panic!("expected InitialValue array literal, got {other:?}"),
    }
}

#[test]
fn constant_vec_inlines_into_var_set() {
    // `v = Vec(…)` in a handler folds to component data on the Var_Set gate —
    // no MakeVector gate needed.
    let r = compile("var v: vector\nin t: exec\non t { v = Vec(1.0, 2.0, 3.0) }");
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_MakeVector"),
        "constant Vec assigned to a var should inline, not spawn MakeVector"
    );
}

#[test]
fn constant_vec_inlines_into_math() {
    // Math gates take prim-math variant data, which carries Vector members.
    let r = compile("in v: vector\nout o = v + Vec(0.0, 0.0, 1.0)");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"));
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_MakeVector"),
        "constant Vec math operand should inline, not spawn MakeVector"
    );
}

#[test]
fn constant_vec_embeds_for_entity_gates() {
    // Entity gates store an unwired Vector input in their data struct, so a
    // folded constant embeds as gate data instead of spawning a MakeVector.
    let r = compile("in e: entity\nin t: exec\non t { e.SetLocation(Vec(0.0, 0.0, 100.0)) }");
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_MakeVector"),
        "SetLocation literal should embed as gate data, not spawn a MakeVector"
    );
    let set_loc = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Entity_SetLocation")
        .expect("SetLocation gate should exist");
    assert_eq!(
        set_loc.properties.get(&crate::intern::intern("Vector")),
        Some(&crate::ir::Literal::Vector { x: 0.0, y: 0.0, z: 100.0 }),
        "the folded vector should ride the gate's data properties"
    );
}

#[test]
fn constant_vec_materializes_for_split_vector() {
    // `.x` lowers to SplitVector, whose Input is a plain struct field — the
    // constant must stay a wired MakeVector, not silently zero out.
    let r = compile("let p = Vec(1.0, 2.0, 3.0)\nout x = p.x");
    assert_no_errors(&r);
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_SplitVector"
    ));
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MakeVector"),
        "component access on a folded constant needs a materialized MakeVector"
    );
}

#[test]
fn array_literal_assignment_desugars_to_clear_push_append() {
    // `foo = [item, 1, ...base, 5]` in an exec handler rebuilds the array:
    // clear, then a push per item and an append per spread.
    let src = "array base: int[] = [3, 4]\nvar foo: int[]\nin t: exec\n\
        on t {\n  let item = 7\n  foo = [item, 1, ...base, 5]\n}";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Clear"),
        "assignment should clear the array first"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Append"),
        "a `...spread` element should lower to an Append gate"
    );
    let pushes = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
        .count();
    assert_eq!(
        pushes, 3,
        "three plain items should lower to three Push gates"
    );
}

#[test]
fn multi_line_array_literals_parse() {
    // Newlines are allowed after '[', around commas, and before ']' — with an
    // optional trailing comma — mirroring the multi-line call-arg rules. Both
    // the top-level baked initializer and the exec-context rebuild use the
    // same literal parse.
    let src = "array names: string[] = [\n  \"a\",\n  \"b\",\n  \"c\",\n]\n\
        array base: int[] = [1, 2]\nvar foo: int[]\nin t: exec\n\
        on t {\n  foo = [\n    3,\n    ...base,\n    4\n  ]\n}";
    let r = compile(src);
    assert_no_errors(&r);
    let pushes = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
        .count();
    assert_eq!(pushes, 2, "multi-line rebuild should still lower each item");
}

#[test]
fn top_level_array_with_non_literal_or_spread_errors() {
    // Outside an exec handler an array initializer is baked, so non-literal
    // elements and spreads must be rejected (not silently dropped). The lower
    // test `compile()` helper drops typecheck errors, so check typecheck here.
    for src in [
        "var x: int = 1\nvar foo = [x, 2]\n",
        "array base: int[] = [1, 2]\narray foo: int[] = [...base, 3]\n",
    ] {
        let parsed = parse(src, "test");
        let tc = typecheck(&parsed.ast, "test");
        assert!(
            tc.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "expected an error for top-level non-literal/spread array init: {src:?}"
        );
    }
}

#[test]
fn every_canonical_array_method_lowers() {
    // Exercises every method in the canonical ARRAY_METHODS table. This ties
    // the table (which drives editor completion/hover) to the dispatch in
    // lower_array_method: a table entry the dispatch can't handle would lower
    // to an `_Unsupported` gate and fail here.
    let src = "array a: int[]\narray b: int[]\nvar ent: entity\nin t: exec\non t {\n  \
        a.push(1)\n  let p = a.pop()\n  let n = a.length()\n  a.remove(0)\n  a.insert(0, 9)\n  \
        a.clear()\n  let i = a.find(3)\n  a.sort()\n  a.reverse()\n  a.shuffle()\n  a.swap(0, 1)\n  \
        a.fill(3)\n  a.resize(4, 0)\n  let s = a.sum()\n  let lo = a.min()\n  let hi = a.max()\n  \
        let av = a.average()\n  b.append(a)\n  b.copyFrom(a)\n  b.slice(a, 0, 2)\n  \
        a.fillFromPlayers()\n  a.fillFromTeam(ent)\n}";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "an array method lowered to _Unsupported"
    );
    // Guard: this test must exercise every entry in the canonical table, so
    // the table cannot list a method without proving it lowers.
    for m in crate::catalog::arrays::ARRAY_METHODS {
        assert!(
            src.contains(&format!(".{}(", m.name)),
            "array method `{}` is in ARRAY_METHODS but not covered by this test",
            m.name
        );
    }
}

#[test]
fn timer_call_lowers_with_exec_controls_and_outputs() {
    // Timer is a function-call instance: optional exec controls wire in, and
    // its Time/Expired outputs are usable as a value / an event.
    let r = compile(
        "in trigger: exec\nlet done: exec\nlet t = Timer(10.0, restart = trigger)\n\
         out elapsed = t.Time\non t.Expired { emit done }",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraphPseudo_Timer"),
        "Timer() should lower to a Pseudo_Timer gate"
    );
}

#[test]
fn in_creates_input_node() {
    let r = compile("in tick: exec");
    let has_input = r.module.nodes.values().any(|n| n.kind == NodeKind::Input);
    assert!(has_input);
    assert_eq!(r.module.inputs.len(), 1);
}

#[test]
fn out_binding_creates_output_node() {
    let r = compile("var n: int = 0\nout count = n");
    let has_output = r.module.nodes.values().any(|n| n.kind == NodeKind::Output);
    assert!(has_output);
    assert_eq!(r.module.outputs.len(), 1);
}

#[test]
fn handler_creates_event_and_exec_chain() {
    let r = compile("on RoundStart { }");
    let has_event = r.module.nodes.values().any(|n| n.kind == NodeKind::Event);
    assert!(has_event, "expected event node for RoundStart");
}

#[test]
fn get_aim_is_one_gate_with_both_ports() {
    // `c.GetAim().Origin` / `.Direction` resolve to a single GetAim gate,
    // with each field accessor wired from its own output port.
    let r = compile(
        "var c: character\nin fire: exec\non fire {\n  let aim = c.GetAim()\n  c.SetLocation(aim.Origin)\n  c.SetVelocity(linear = aim.Direction)\n}",
    );
    assert_no_errors(&r);
    let aim_nodes: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_WireGraph_Exec_Character_GetAim")
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(aim_nodes.len(), 1, "expected exactly one GetAim gate");
    let aim_id = aim_nodes[0];
    let source_ports: std::collections::HashSet<&str> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == aim_id)
        .map(|w| w.source.port.as_str())
        .collect();
    assert!(
        source_ports.contains("Origin"),
        "Origin port not wired: {source_ports:?}"
    );
    assert!(
        source_ports.contains("Direction"),
        "Direction port not wired: {source_ports:?}"
    );
}

#[test]
fn chat_command_config_args_set_gate_data() {
    // Positional config fills CommandName/HelpText; identifier params bind
    // the controller/arguments outputs.
    let r = compile(
        "on ChatCommand(\"greet\", \"Greets the player\", player, args) {\n  player.DisplayText(\"hi ${args}\")\n}",
    );
    assert_no_errors(&r);
    let evt = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ChatCommand")
        .expect("expected a ChatCommand event node");
    let prop = |field: &str| match evt.properties.get(&intern(field)) {
        Some(Literal::String(s)) => Some(s.clone()),
        _ => None,
    };
    assert_eq!(prop("CommandName").as_deref(), Some("greet"));
    assert_eq!(prop("HelpText").as_deref(), Some("Greets the player"));
}

#[test]
fn chat_command_named_description_sets_help_text() {
    // `Description = "..."` is an alias for the HelpText field.
    let r = compile("on ChatCommand(\"wave\", Description = \"Wave at everyone\") { }");
    assert_no_errors(&r);
    let evt = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ChatCommand")
        .expect("expected a ChatCommand event node");
    let prop = |field: &str| match evt.properties.get(&intern(field)) {
        Some(Literal::String(s)) => Some(s.clone()),
        _ => None,
    };
    assert_eq!(prop("CommandName").as_deref(), Some("wave"));
    assert_eq!(prop("HelpText").as_deref(), Some("Wave at everyone"));
}

#[test]
fn counter_program_end_to_end() {
    let src = "in tick: exec\nvar n: int = 0\non tick {\n  n = n + 1\n}\nout count = n";
    let r = compile(src);
    assert!(
        r.diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .count()
            == 0,
        "unexpected errors: {:?}",
        r.diagnostics
    );
    // Should have: MicrochipInput, Pseudo_Var, MicrochipOutput, and at least a
    // Var_Increment or Var_Set gate.
    let has_incr = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Increment");
    assert!(has_incr, "counter should lower to Var_Increment");
    // All wires should reference valid node ids.
    for w in &r.module.wires {
        assert!(
            r.module.nodes.contains_key(&w.source.node_id),
            "dangling wire source: {}",
            w.source.node_id
        );
        assert!(
            r.module.nodes.contains_key(&w.target.node_id),
            "dangling wire target: {}",
            w.target.node_id
        );
    }
}

#[test]
fn if_expr_creates_select_gate() {
    let r = compile("out x = if true then 1 else 0");
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let has_select = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_Select");
    assert!(has_select, "expected a Select gate for if-expr");
}

#[test]
fn give_weapon_lowers_to_set_inventory_entry_with_asset() {
    // `char.GiveWeapon($BRItemBase/Weapon_Pistol, 0)` lowers to the
    // SetInventoryEntry gate, carrying the weapon as an asset property.
    let r = compile("in p: character\non p {\n  p.GiveWeapon($BRItemBase/Weapon_Pistol, 0)\n}");
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry")
        .expect("expected a SetInventoryEntry gate");
    match node
        .properties
        .get(&crate::intern::intern("ItemTypeIfItem"))
    {
        Some(crate::ir::Literal::Asset {
            asset_type,
            asset_name,
        }) => {
            assert_eq!(asset_type, "BRItemBase");
            assert_eq!(asset_name, "Weapon_Pistol");
        }
        other => panic!("expected an asset property, got {other:?}"),
    }
}

#[test]
fn messaging_builtins_lower_to_their_gates() {
    // Per-controller chat/message-box plus global chat/status broadcasts.
    let r = compile(
        "in p: character\non p {\n  let c = p.ControllerOf()\n  c.ShowChatMessage(\"psst\")\n  c.ShowMessageBox(\"body\", title = \"note\")\n  BroadcastChatMessage(\"hello everyone\")\n  BroadcastStatusMessage(\"round over\", flash = true)\n}",
    );
    assert_no_errors(&r);
    for class in [
        "BrickComponentType_WireGraph_Exec_Controller_ShowChatMessage",
        "BrickComponentType_WireGraph_Exec_Controller_ShowMessageBox",
        "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage",
        "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastStatusMessage",
    ] {
        assert!(has_gate(&r, class), "expected a {class} gate");
    }
}

#[test]
fn play_audio_builtins_carry_audio_asset() {
    // The `$BrickOneShotAudioDescriptor/…` ref inlines into the gate's
    // AudioDescriptor data field (registered as an external asset at emit).
    let r = compile(
        "in p: character\non p {\n  p.PlayAudioAt($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press, volume = 0.5)\n  PlayGlobalAudio($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press)\n}",
    );
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "Component_WireGraph_PlayAudioAt")
        .expect("expected a PlayAudioAt gate");
    match node
        .properties
        .get(&crate::intern::intern("AudioDescriptor"))
    {
        Some(crate::ir::Literal::Asset {
            asset_type,
            asset_name,
        }) => {
            assert_eq!(asset_type, "BrickOneShotAudioDescriptor");
            assert_eq!(asset_name, "BOSA_Buttons_Button_1_Press");
        }
        other => panic!("expected an asset property, got {other:?}"),
    }
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_PlayGlobalAudio"),
        "expected a PlayGlobalAudio gate"
    );
}

#[test]
fn entity_tags_lower_to_tag_gates() {
    let r = compile(
        "in p: character\non p {\n  p.SetTag(\"slot3\")\n  let t = p.GetTag()\n  p.DisplayText(\"tag ${t}\")\n}",
    );
    assert_no_errors(&r);
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Entity_SetTag"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Entity_GetTag"
    ));
}

#[test]
fn find_player_is_exec_and_change_detector() {
    // `FindPlayer` is an exec gate (Exec/ExecOut) that emits a character, so it
    // runs inside an exec handler. `Change` targets the Exec change detector
    // (which carries the OnChanged output); the plain-bool gate is `Changed`.
    let r = compile(
        "in name: string\n\
         in go: exec\n\
         static var p: character\n\
         on go { p = FindPlayer(name) }\n\
         out changed = Change(name)",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_FindPlayer"));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_ChangeDetectorExec"
    ));
}

#[test]
fn quat_make_split_dot_lower_to_their_gates() {
    let r = compile(
        "let q = Quat(0.0, 0.0, 0.0, 1.0)\nlet s = q.SplitQuat()\nout w = s.W\nout d = q.QuatDot(q)",
    );
    assert_no_errors(&r);
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_MakeQuaternion"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_SplitQuaternion"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_QuatDotProduct"
    ));
}

#[test]
fn inventory_family_carries_asset_properties() {
    let r = compile(
        "in p: character\non p {\n  p.AddInventoryItem($BRItemBase/Weapon_Pistol)\n  p.SetInventoryItem($BRItemBase/Weapon_Bow, slot = 2)\n  p.AddInventoryItemAdv($BRItemBase/Weapon_Pistol, damage = 2.0, itemName = \"Big Iron\")\n}",
    );
    assert_no_errors(&r);
    let item = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Character_AddInventoryItem")
        .expect("expected an AddInventoryItem gate");
    match item.properties.get(&crate::intern::intern("Item")) {
        Some(crate::ir::Literal::Asset { asset_name, .. }) => {
            assert_eq!(asset_name, "Weapon_Pistol");
        }
        other => panic!("expected an asset property, got {other:?}"),
    }
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Character_SetInventoryItem"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Exec_Character_AddInventoryItemAdv"
    ));
}

#[test]
fn damage_and_zone_events_lower_to_event_gates() {
    let r = compile(
        "on CharacterDamaged(char, dmg) {\n  char.DisplayText(\"ouch ${dmg}\")\n}\non EntityZoneEntered(e) {\n  e.SetTag(\"inside\")\n}\non ProjectileZoneEntered(shooter) {\n  shooter.DisplayText(\"hit!\")\n}",
    );
    assert_no_errors(&r);
    for class in [
        "BrickComponentType_WireGraph_Fake_Gamemode_CharacterDamagedEvent",
        "BrickComponentType_Internal_EntityZoneEvent_Entered",
        "BrickComponentType_Internal_ProjectileZoneEvent_Entered",
    ] {
        assert!(has_gate(&r, class), "expected a {class} event gate");
    }
}

#[test]
fn has_role_lowers_with_config_role_name() {
    // `ctrl.HasRole("Admin")` lowers to the HasRole gate (RoleName is a config
    // string) and returns a bool.
    let r = compile(
        "in p: character\non p {\n  let c = p.ControllerOf()\n  let a = c.HasRole(\"Admin\")\n  if a { p.DisplayText(\"hi\") }\n}",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Controller_HasRole"),
        "expected a HasRole gate"
    );
}

#[test]
fn rotation_quat_color_receivers_lower_to_their_gates() {
    // The cl14428 rotation/quaternion + sRGB/hex color receivers and
    // constructors lower to their gates. `quat` is a distinct type from the
    // euler `rotator`.
    let src = "in p: character\non p {\n  \
        let dir = Vec(1.0, 0.0, 0.0)\n  \
        let q = dir.ToRotation()\n  let v = q.ToDirection()\n  \
        let spun = dir.Rotate(q)\n  let inv = q.Invert()\n  \
        let q2 = dir.RotationTo(Vec(0.0, 1.0, 0.0))\n  \
        let ang = q.AngleTo(q2)\n  let mid = q.Slerp(q2, 0.5)\n  \
        let aa = q.ToAxisAngle()\n  let fa = dir.RotationByAngle(1.57)\n  \
        let rot = Rotation(0.0, 90.0, 0.0)\n  let e = rot.ToEuler()\n  \
        let c = ColorSRGB(255, 128, 0, 255)\n  let h = c.ToHex()\n  \
        let c2 = ColorHex(\"#ff8800\")\n  let s = c.ToSRGB()\n  \
        let bl = c.Blend(c2, 0.5)\n  \
        p.DisplayText(\"${ang} ${h}\")\n}";
    let r = compile(src);
    assert_no_errors(&r);
    for class in [
        "BrickComponentType_WireGraph_Expr_DirectionToRotation",
        "BrickComponentType_WireGraph_Expr_RotationToDirection",
        "BrickComponentType_WireGraph_Expr_RotateVector",
        "BrickComponentType_WireGraph_Expr_InvertRotation",
        "BrickComponentType_WireGraph_Expr_QuatBetween",
        "BrickComponentType_WireGraph_Expr_QuatAngleBetween",
        "BrickComponentType_WireGraph_Expr_QuatSlerp",
        "BrickComponentType_WireGraph_Expr_QuatToAxisAngle",
        "BrickComponentType_WireGraph_Expr_QuatFromAxisAngle",
        "BrickComponentType_WireGraph_Expr_MakeRotation",
        "BrickComponentType_WireGraph_Expr_SplitRotation",
        "BrickComponentType_WireGraph_Expr_MakeColorSRGB",
        "BrickComponentType_WireGraph_Expr_MakeColorHex",
        "BrickComponentType_WireGraph_Expr_SplitColorSRGB",
        "BrickComponentType_WireGraph_Expr_ColorToHex",
        "BrickComponentType_WireGraph_Expr_ColorBlend",
    ] {
        assert!(has_gate(&r, class), "missing gate {class}");
    }
}

#[test]
fn field_access_vector_creates_split() {
    let r = compile("out x = vec(1.0, 2.0, 3.0).x");
    let has_split = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_SplitVector");
    assert!(has_split, "expected a SplitVector gate for .x access");
}

#[test]
fn field_access_vector_component_on_local_creates_split() {
    // `let a = ...; a.x` goes through the local-binding path, which previously
    // returned the whole vector port instead of splitting out the component.
    let r = compile(
        "in p: character\non p {\n  let a = vec(1.0, 2.0, 3.0) + vec(4.0, 5.0, 6.0)\n  let cx = a.x\n  p.DisplayText(\"${cx}\")\n}",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let has_split = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_SplitVector");
    assert!(has_split, "expected a SplitVector gate for `.x` on a local");
}

#[test]
fn splitvec_record_fields_reuse_single_split() {
    // Regression: `let p = v.SplitVec(); p.x / p.y / p.z` must read the X/Y/Z
    // ports of the one split. Previously the swizzle fallthrough re-split p's
    // first field (a scalar), so y/z read garbage. Exactly one SplitVector.
    let r = compile(
        "in pl: character\non pl {\n  let v = vec(11.0, 22.0, 33.0)\n  let p = v.SplitVec()\n  let s = p.x + p.y + p.z\n  pl.DisplayText(\"${s}\")\n}",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let splits = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_SplitVector")
        .count();
    assert_eq!(
        splits, 1,
        "p.x/p.y/p.z should reuse the single SplitVector, got {splits}"
    );
}

#[test]
fn array_decl_creates_pseudo_node() {
    let r = compile("array items: int[]");
    let has_arr = r
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar");
    assert!(has_arr, "expected an ArrayVar pseudo-node");
}

#[test]
fn var_value_trigger_lowers_body() {
    let r = compile(
        "\
var x: int = 0
on x.value {
  let y = x + 10
}
out result = x",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "let inside on var.value handler should produce MathAdd gate"
    );
}

#[test]
fn var_value_trigger_nested_if_lowers() {
    let r = compile(
        "\
var dir: int = 0
var moving: bool = false
in stop: bool
in pos: float
in target: float
var floor: int = 0
on floor.value {
  if moving {
    let will_stop = dir == 1 && stop && pos < target
    if will_stop {
      moving = false
    }
  }
}
out result = dir",
    );
    assert_no_errors(&r);
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"),
        "nested let inside on var.value + if should produce CompareEqual"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareLess"),
        "nested let inside on var.value + if should produce CompareLess"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalAND"),
        "nested let inside on var.value + if should produce LogicalAND"
    );
}

#[test]
fn detector_builtins_map_to_split_gate_classes() {
    // cl14860 split the detectors: `Change` (value pulse-through, OnChanged)
    // lives on the Exec gate now, `Changed` (bool pulse) on the plain gate;
    // `EdgeExec` fires exec pulses for `on`/`await`, and `Edge`'s
    // rising/falling record fields resolve via the port aliases.
    let src = "in v: float\nin b: bool\nin x: int\nin t: exec\n\
        let c = Change(x)\nlet cd = Changed(x)\nlet e = Edge(b)\nlet ee = EdgeExec(v)\n\
        out cv = c\nout cdv = cd\nout rising = e.Rising\nout falling = e.Falling\n\
        var n: int = 0\non ee.Rising { n = n + 1 }\non ee.Falling { n = n - 1 }";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_ChangeDetectorExec"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_ChangeDetector"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_EdgeDetector"
    ));
    assert!(has_gate(
        &r,
        "BrickComponentType_WireGraph_Expr_EdgeDetectorExec"
    ));
}

/// `on ControllerLeft(controller, userId)` exposes the gate's `UserId` output
/// as a second positional param. The gate is a pure source, so a wire SOURCED
/// from its `UserId` port into the handler body proves the id is bound.
#[test]
fn controller_left_exposes_user_id() {
    let r = compile(
        "\
var lastLeft: string
on ControllerLeft(controller, userId) {
  lastLeft = userId
}",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "userId must bind to the gate's UserId port, not an _Unsupported placeholder"
    );
    let event_node = r
        .module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraph_Fake_Gamemode_ControllerLeftEvent"
        })
        .expect("expected a ControllerLeft event gate");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == event_node.id && w.source.port.as_str() == "UserId"),
        "the handler body must consume the event gate's UserId output"
    );
}

/// `on ZoneEntered(character, zone = zoneA)` wires the `zoneA` value into the
/// event gate's `Zone` input port. Event gates are otherwise pure sources, so
/// an input wire into the gate — sourced from the `in` port — proves the bind.
#[test]
fn zone_event_input_binding() {
    let r = compile(
        "\
in zoneA: entity
on ZoneEntered(character, zone = zoneA) {
  PrintToConsole(\"entered\")
}",
    );
    assert_no_errors(&r);
    let event_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_Internal_CharacterZoneEvent_Entered")
        .expect("expected a ZoneEntered event gate");
    let zone_wire = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == event_node.id)
        .expect("ZoneEntered gate must have its Zone input wired");
    let src = r
        .module
        .nodes
        .get(&zone_wire.source.node_id)
        .expect("Zone wire source node");
    assert_eq!(
        src.gate_class, "BrickComponentType_Internal_MicrochipInput",
        "Zone input should be wired from the `in zoneA` port"
    );
    // The zone gate's exec output is named `Exec`, not `ExecOut`. The handler
    // body must chain from that port or the game silently drops the connection.
    let exec_wire = r
        .module
        .wires
        .iter()
        .find(|w| w.source.node_id == event_node.id)
        .expect("ZoneEntered gate must chain its exec into the handler body");
    assert_eq!(
        exec_wire.source.port.as_str(),
        "Exec",
        "zone event exec output port must be `Exec`, not `ExecOut`"
    );
}

#[test]
fn out_binding_same_name_as_array_wires_array_ref() {
    // `out X = X` where X is also an array: the output binding must not
    // clobber the array's scope entry - the init expr reads the ArrayVar,
    // not an _Unsupported placeholder (which emit would drop the wire for).
    let r = compile("array deckCounts: int[]\nout deckCounts: int[] = deckCounts");
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "init expr should resolve to the array var"
    );
    let out_id = r
        .module
        .nodes
        .values()
        .find(|n| n.kind == crate::ir::NodeKind::Output)
        .expect("output node")
        .id;
    let arr_id = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_ArrayVar")
        .expect("array var node")
        .id;
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == arr_id
            && w.target.node_id == out_id
            && w.target.port == WirePort::RerInput),
        "expected wire ArrayVar -> Output.RER_Input, wires: {:?}",
        r.module.wires
    );
}

#[test]
fn out_binding_same_name_as_var_wires_var_value() {
    // Scalar flavor of the same-name case: `var n` + `out n = n`.
    let r = compile("var n: int\nout n: int = n");
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "init expr should resolve to the var"
    );
    let out_id = r
        .module
        .nodes
        .values()
        .find(|n| n.kind == crate::ir::NodeKind::Output)
        .expect("output node")
        .id;
    let var_id = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("var node")
        .id;
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == var_id
            && w.target.node_id == out_id
            && w.target.port == WirePort::RerInput),
        "expected wire Var -> Output.RER_Input, wires: {:?}",
        r.module.wires
    );
}

#[test]
fn out_binding_declared_before_var_still_wires() {
    // Reverse order: the var predeclare must not clobber the output either.
    let r = compile("out n: int = n\nvar n: int");
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    let out_id = r
        .module
        .nodes
        .values()
        .find(|n| n.kind == crate::ir::NodeKind::Output)
        .expect("output node")
        .id;
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == out_id && w.target.port == WirePort::RerInput),
        "expected a wire into Output.RER_Input, wires: {:?}",
        r.module.wires
    );
}

// ── Silently-dropped var initializers warn (WSP001, "dropped") ──

fn dropped_init_warns(r: &LowerResult) -> Vec<String> {
    r.diagnostics
        .iter()
        .filter(|d| {
            d.severity == crate::diagnostic::Severity::Warning && d.message.contains("dropped")
        })
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn top_level_nonconstant_var_init_warns() {
    let r = compile("in a: int\nvar x: int = a + 1\nout r = x");
    let warns = dropped_init_warns(&r);
    assert_eq!(warns.len(), 1, "diags: {:?}", r.diagnostics);
    assert!(warns[0].contains("'var x'"), "got: {}", warns[0]);
}

#[test]
fn top_level_constant_var_init_no_warn() {
    let r = compile("var x: int = 5\nvar v: vector = Vec(1.0, 2.0, 3.0)\nout r = x");
    assert!(
        dropped_init_warns(&r).is_empty(),
        "diags: {:?}",
        r.diagnostics
    );
}

#[test]
fn chip_pure_var_init_from_param_warns() {
    let r = compile("chip g(e: bool) {\n  var x: bool = e\n}\nvar e: bool = true\ng(e)");
    let warns = dropped_init_warns(&r);
    assert_eq!(warns.len(), 1, "diags: {:?}", r.diagnostics);
}

#[test]
fn handler_var_runtime_init_no_warn() {
    let r = compile("in start: exec\nin a: int\non start { var x: int = a + 1\nx = x }");
    assert!(
        dropped_init_warns(&r).is_empty(),
        "diags: {:?}",
        r.diagnostics
    );
}

#[test]
fn static_var_nonconstant_init_warns_even_in_exec() {
    let r = compile("in start: exec\nin a: int\non start { static var x: int = a + 1\nx = x }");
    let warns = dropped_init_warns(&r);
    assert_eq!(warns.len(), 1, "diags: {:?}", r.diagnostics);
    assert!(warns[0].contains("static var"), "got: {}", warns[0]);
}

#[test]
fn exec_array_var_nonliteral_init_warns() {
    let r = compile(
        "in start: exec\non start { var f: int[] = [1, 2]\nvar g2: int[] = f\ng2.push(3) }",
    );
    let warns = dropped_init_warns(&r);
    assert_eq!(warns.len(), 1, "diags: {:?}", r.diagnostics);
    assert!(warns[0].contains("array literal"), "got: {}", warns[0]);
}
