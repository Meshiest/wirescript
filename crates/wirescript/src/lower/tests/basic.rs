use super::*;

#[test]
fn empty_program() {
    let r = compile("");
    assert!(r.diagnostics.is_empty());
    assert!(r.module.nodes.is_empty());
}

/// `v as entity.GetLocation()` reads `entity.GetLocation` as the type: one
/// error saying to parenthesise, not a cascade, and the parenthesised form is
/// clean.
#[test]
fn cast_then_method_without_parentheses_is_one_clear_error() {
    let src = |e: &str| format!("in v: any\nin go: exec\nvar p: vector\non go {{\n  p = {e}\n}}");
    let bad = src("v as entity.GetLocation()");
    let parsed = crate::parser::parse(&bad, "test");
    assert_eq!(parsed.diagnostics.len(), 1, "{:?}", parsed.diagnostics);
    let msg = &parsed.diagnostics[0].message;
    assert!(
        msg.contains("`(value as entity).GetLocation(...)`"),
        "{msg}"
    );
    let tc =
        crate::typecheck::typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        tc.diagnostics.is_empty(),
        "the recovered parse is the parenthesised form: {:?}",
        tc.diagnostics
    );
    compile_ok(&src("(v as entity).GetLocation()"));
}

// ── Outputs: one driver per output rerouter ──

/// A single `emit out = <expr>` inside an `if` must be var-backed so the write
/// is gated by the branch's taken-exec — not wired straight to the output
/// rerouter (unconditional). The single-emit-site fast path used to bypass the
/// guard, making the output continuously equal the value regardless of `c`.
#[test]
fn single_conditional_emit_is_branch_gated() {
    let r =
        compile_ok("in c: bool\nin a: int\nin go: exec\nout r: int\non go { if c { emit r = a } }");
    let out_id = find_gate(&r, gc::MICROCHIP_OUTPUT);
    let feed = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == out_id)
        .map(|w| w.source.node_id)
        .and_then(|id| r.module.nodes.get(&id));
    assert!(
        feed.is_some_and(|n| n.gate_class == gc::VAR),
        "the conditional output must be fed by a backing var, not wired unconditionally"
    );
    // The gated write exists: a Var_Set driven from the branch's taken exec.
    let branch = find_gate(&r, gc::BRANCH);
    let set_ids: crate::collections::HashSet<crate::ir::NodeId> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::VAR_SET)
        .map(|(id, _)| *id)
        .collect();
    assert!(
        r.module.wires.iter().any(|w| {
            w.source.node_id == branch
                && w.source.port == WirePort::ExecOutA
                && set_ids.contains(&w.target.node_id)
        }),
        "the branch's taken exec must drive the emit's Var_Set (gated write)"
    );
}

/// Exactly one wire drives an output's `RER_Input` (2+ = a load-breaking
/// fan-in). An output defaulted (`out r = 0`) and emitted, or emitted from a
/// handler and from an anonymous `chip { … }` sharing the parent's output,
/// is var-backed rather than driven by each site.
#[test]
fn an_output_written_from_two_places_has_one_driver() {
    let rows = [
        "in go: exec\nout r = 0\non go { emit r = 42 }",
        "in go: exec\nin a: int\nin b: int\nout r: int\non go { emit r = a\n chip { emit r = b } }",
    ];
    let mut failures = Vec::new();
    for src in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let out_id = find_gate(&r, gc::MICROCHIP_OUTPUT);
        let drivers = r
            .module
            .wires
            .iter()
            .filter(|w| w.target.node_id == out_id && w.target.port == WirePort::RerInput)
            .count();
        if !errors.is_empty() || drivers != 1 {
            failures.push(format!("{src:?}: errors {errors:?}, {drivers} drivers"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// `out X = X` where `X` also names storage: the output binding must not
/// clobber the storage's scope entry, in either declaration order. The
/// initializer reads the storage gate, not an `_Unsupported` placeholder
/// (which emit would drop the wire for).
#[test]
fn out_binding_named_like_storage_wires_the_storage() {
    let rows = [
        (
            "var deckCounts: int[]\nout deckCounts: int[] = deckCounts",
            gc::ARRAY_VAR,
        ),
        ("var n: int\nout n: int = n", gc::VAR),
        ("out n: int = n\nvar n: int", gc::VAR),
    ];
    let mut failures = Vec::new();
    for (src, storage) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let outs: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Output)
            .map(|n| n.id)
            .collect();
        let fed = outs.first().is_some_and(|out_id| {
            r.module.wires.iter().any(|w| {
                w.target.node_id == *out_id
                    && w.target.port == WirePort::RerInput
                    && class_of(&r, w.source.node_id) == storage
            })
        });
        if !errors.is_empty()
            || has_unsupported(&r.module)
            || outs.len() != 1
            || r.module.outputs.len() != 1
            || !fed
        {
            failures.push(format!(
                "{src:?}: errors {errors:?}, outputs {outs:?}, fed from {storage}: {fed}\n{:?}",
                r.module.wires
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

// ── Spreads into call arguments ──

/// `...tuple` splats a bound tuple across consecutive positional args — into a
/// builtin's data slots and a user mod's params.
#[test]
fn spread_tuple_into_call_args() {
    let r = compile_ok(
        "in a: int\nin b: float\nin go: exec\n\
         on go { let t = (a, b)\n SendGlobalCustomEvent(\"c\", ...t) }",
    );
    assert!(!has_unsupported(&r.module));
    let send = find_gate(&r, gc::SEND_CUSTOM_EVENT_GLOBAL);
    let ports: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.node_id == send)
        .map(|w| w.target.port)
        .collect();
    assert!(
        ports.contains(&WirePort::DataIn1) && ports.contains(&WirePort::DataIn2),
        "spread must wire the tuple into DataIn1 + DataIn2: {ports:?}"
    );

    // A user mod: `add3(...t)` binds each element to a param.
    let m = compile_ok(
        "mod add3(a: int, b: int, c: int) -> int { return a + b + c }\n\
         in n: int\nout r = add3(...(n, 2, 3))",
    );
    assert!(
        !has_unsupported(&m.module),
        "spread into mod params must bind"
    );
}

/// A `...rest` variadic mod captures the trailing call args into a compile-time
/// tuple and a `...rest` in its body splats them back — both into a builtin's
/// data slots and into another (fixed-arity) mod's params.
#[test]
fn variadic_mod_captures_and_splats() {
    let r = compile_ok(
        "in a: int\nin b: float\nin go: exec\n\
         mod broadcast(name: const string, ...rest) { SendGlobalCustomEvent(name, ...rest) }\n\
         on go { broadcast(\"dmg\", a, b) }",
    );
    assert!(!has_unsupported(&r.module));
    let send = find_gate(&r, gc::SEND_CUSTOM_EVENT_GLOBAL);
    let ports: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.node_id == send)
        .map(|w| w.target.port)
        .collect();
    assert!(
        ports.contains(&WirePort::DataIn1) && ports.contains(&WirePort::DataIn2),
        "captured rest must wire into DataIn1 + DataIn2: {ports:?}"
    );

    // Forwarding the captured rest into a fixed-arity user mod.
    let m = compile_ok(
        "mod add3(x: int, y: int, z: int) -> int { return x + y + z }\n\
         mod sum3(base: int, ...rest) -> int { return add3(base, ...rest) }\n\
         var total: int = 0\nin go: exec\non go { total = sum3(1, 2, 3) }",
    );
    assert!(
        !has_unsupported(&m.module),
        "variadic forward into a mod must bind every element"
    );
}

/// Spreading a tuple that ORIGINATED from a multi-output mod call expands like
/// `let (a, b) = f()` — one output per positional slot — instead of a numeric
/// TuplePick that missed the name-keyed call record and left `_Unsupported`.
#[test]
fn spread_multi_output_call_result_binds() {
    for (src, what) in [
        (
            "mod two() -> (a: int, b: int) { return 1, 2 }\nin n: int\nin go: exec\n\
             on go { let p = two()\n SendGlobalCustomEvent(\"c\", n, ...p) }",
            "bound",
        ),
        (
            "mod two() -> (a: int, b: int) { return 1, 2 }\nin n: int\nin go: exec\n\
             on go { SendGlobalCustomEvent(\"c\", n, ...two()) }",
            "inline",
        ),
    ] {
        let r = compile_ok(src);
        assert!(
            !has_unsupported(&r.module),
            "{what} call-result spread must bind"
        );
    }
}
