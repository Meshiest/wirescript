//! Local exec signals: `emit`, `await`, buffered emits and payloads.

use super::*;

#[test]
fn on_local_exec_signal_fires_from_emit_in_another_handler() {
    // `on sig` must trigger when `emit sig` runs in a *different* handler, even
    // though `on sig` appears after the emitting handler in source. The
    // pre-declared hub carries the emit to the listener; with a single emitter
    // the hub union is spliced out, so assert the connectivity itself: trig's
    // chain must reach the `on sig` body.
    let src = "in trig: exec\n\
               let sig: exec\n\
               static var n: int = 0\n\
               on trig { emit sig }\n\
               on sig { n = n + 1 }";
    let r = compile_ok(src);
    let trig = find_gate(&r, gc::MICROCHIP_INPUT);
    let body = find_gate(&r, gc::VAR_INCREMENT);
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
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "await trigger must not lower to _Unsupported: {:?}",
        wiring_shape(&r.module)
    );
    // Single emitter: no degenerate Union hub should survive, and the handler's
    // input must drive the post-await continuation. The same-chain emit routes
    // through a Var_Set(armed = true) *before* the (spliced) hub so the
    // awaiting Var_Get can't race the arm.
    assert!(
        !has_gate_deep(&r, gc::UNION),
        "a single-emitter signal must not keep a pass-through Union: {:?}",
        wiring_shape(&r.module)
    );
    let run = find_gate(&r, gc::MICROCHIP_INPUT);
    let cont = find_gate(&r, gc::PRINT_TO_CONSOLE);
    assert!(
        wired_reachable(&r, run, cont),
        "emit->await continuation must be exec-wired from the handler input; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn await_continuation_reads_a_fresh_var_get() {
    // A var can change while an exec is suspended on `await`, so the resumed
    // chain must re-read from a fresh Var_Get rather than reuse the
    // pre-await gate — the same invalidation `lower_if` already does at its
    // branch boundaries.
    let src = "static var n: int = 0\n\
               in go: exec\n\
               let done: exec\n\
               on go {\n\
                 let a = n\n\
                 await done\n\
                 let b = n\n\
                 emit done = go\n\
               }";
    let r = compile_ok(src);
    // `await` also allocates its own "armed" pseudo-var whose guard read is an
    // unrelated Var_Get, so count only the readers wired to `n`'s own VarRef.
    let n_var = r
        .module
        .nodes
        .iter()
        .find(|(_, node)| {
            node.gate_class == gc::VAR
                && node.properties.get(&*crate::intern::sym::NAME_LABEL)
                    == Some(&crate::ir::Literal::String("n".to_string()))
        })
        .map(|(id, _)| *id)
        .expect("`n`'s pseudo-var node must exist");
    let n_readers: crate::collections::HashSet<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == n_var && w.source.port == WirePort::VarRef)
        .map(|w| w.target.node_id)
        .collect();
    assert_eq!(
        n_readers.len(),
        2,
        "one Var_Get of `n` before the await and a fresh one after it: {n_readers:?}"
    );
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
    let r = compile_ok(src);
    assert!(
        has_gate(&r, gc::BUFFER_TICKS),
        "buffered emit should insert a BufferTicks: {:?}",
        wiring_shape(&r.module)
    );
    let cyc = crate::analyze::analyze_cycles(&r.module);
    crate::test_support::assert_no_code(&cyc.diagnostics, "WS005");
    assert!(
        !cyc.strongly_connected.is_empty(),
        "the back-edge emit must actually close a cycle"
    );
}

/// A buffered emit's duration picks its buffer: bare `buffer` is one tick, an
/// `s` unit selects BufferSeconds, and a runtime delay wires `TicksToWait`
/// instead of baking a property.
#[test]
fn buffered_emit_duration_selects_its_buffer() {
    enum Want {
        Ticks(i64),
        Seconds,
        WiredTicks,
    }
    let rows: &[(&str, &str, Want)] = &[
        ("", "", Want::Ticks(1)),
        ("(0.5s)", "", Want::Seconds),
        ("(d)", "var d: int = 3\n", Want::WiredTicks),
    ];
    let mut failures = Vec::new();
    for (dur, decl, want) in rows {
        let src = format!(
            "in run: exec\n{decl}on run {{\n  let sig: exec\n  emit sig\n  await sig\n  buffer{dur} emit sig\n}}"
        );
        let r = compile(&src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let ok = errors.is_empty()
            && match want {
                Want::Ticks(n) => {
                    let b = nodes_of(&r.module, gc::BUFFER_TICKS);
                    b.len() == 1
                        && b[0].properties.get(&*crate::intern::sym::TICKS_TO_WAIT)
                            == Some(&crate::ir::Literal::Int(*n))
                }
                Want::Seconds => has_gate(&r, gc::BUFFER_SECONDS),
                Want::WiredTicks => r.module.nodes.iter().any(|(id, n)| {
                    n.gate_class == gc::BUFFER_TICKS
                        && r.module.wires.iter().any(|w| {
                            w.target.node_id == *id && w.target.port == WirePort::TicksToWait
                        })
                }),
            };
        if !ok {
            failures.push(format!(
                "buffer{dur} emit sig: errors {errors:?}\n{:?}",
                wiring_shape(&r.module)
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
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
    let r = compile_ok(src);
    // Stores: payload var + armed flag; reads: the armed guard + the payload.
    assert_eq!(gate_count(&r, gc::VAR), 2, "payload store + armed flag");
    assert_eq!(gate_count(&r, gc::VAR_GET), 2, "armed read + payload read");
    let run = find_gate(&r, gc::MICROCHIP_INPUT);
    let cont = find_gate(&r, gc::PRINT_TO_CONSOLE);
    assert!(
        wired_reachable(&r, run, cont),
        "continuation must be exec-wired"
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
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "loop must lower fully: {:?}",
        wiring_shape(&r.module)
    );
    assert!(has_gate(&r, gc::BUFFER_TICKS), "back-edge must buffer");
    let cyc = crate::analyze::analyze_cycles(&r.module);
    crate::test_support::assert_no_code(&cyc.diagnostics, "WS005");
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
      var nums: int[] = [1, 2]\n\
      var names: string[] = [\"a\", \"b\"]\n\
      on run {\n\
        first(nums)\n\
        second(names)\n\
      }";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "both mods' awaits must resolve: {:?}",
        wiring_shape(&r.module)
    );
    assert_eq!(
        gate_count(&r, gc::BUFFER_TICKS),
        2,
        "each mod's loop needs its own buffer"
    );
    let run = find_gate(&r, gc::MICROCHIP_INPUT);
    let chat = find_gate(&r, gc::GAMEMODE_BROADCAST_CHAT);
    assert!(
        wired_reachable(&r, run, chat),
        "second mod's loop body must be exec-wired"
    );
    let cyc = crate::analyze::analyze_cycles(&r.module);
    crate::test_support::assert_no_code(&cyc.diagnostics, "WS005");
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
      var nums: int[] = [1, 2, 3]\n\
      on run { BroadcastChatMessage(\"Sum: ${sumItems(nums)}\") }";
    let r = compile_ok(src);
    // After emit-terminates-block + dead-join pruning, the only Union left is
    // the armed-emit union (kick + back-edge): the if-join lost both arms
    // (then ends in emit, else in return) and the mod-end union collapsed to
    // the single return path.
    assert_eq!(
        gate_count(&r, gc::UNION),
        1,
        "only the armed-emit union should remain; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| (n.gate_class, n.note))
            .collect::<Vec<_>>()
    );
    // The caller's chat message has exactly one exec source — the return path.
    let chat = find_gate(&r, gc::GAMEMODE_BROADCAST_CHAT);
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
    assert_ne!(
        r.module.nodes[&exec_ins[0].source.node_id].gate_class,
        gc::UNION,
        "continuation should come straight from the return path, not a join union"
    );
}
