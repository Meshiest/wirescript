//! A const parameter reaches the positions that require a literal.
//! `SendCustomEvent`'s channel name is the sharpest case: it is gate CONFIG,
//! not a wire, so a non-constant there is an error and a constant bakes into
//! the component data.

pub(crate) use super::lower_ok;

/// The baked `EventName` of every `SendCustomEvent` node. `EventName` is a
/// data-struct field, not a pre-interned `sym::` constant (see
/// `intern.rs`/`port_registry.rs`'s `WirePort::EventName`), so it's interned
/// on the fly exactly like `lower/call/builtin.rs` does when baking it.
fn event_name_properties(m: &crate::ir::Module) -> Vec<String> {
    super::nodes_of(m, crate::ir::gate_class::SEND_CUSTOM_EVENT)
        .iter()
        .filter_map(
            |n| match n.properties.get(&crate::intern::intern("EventName")) {
                Some(crate::ir::Literal::String(s)) => Some(s.clone()),
                _ => None,
            },
        )
        .collect()
}

/// The baked `EventName` of every `CustomEvent` receiver node (`on
/// CustomEvent(...)`). Mirrors `event_name_properties` for the RECEIVER gate
/// class; sender and receiver must agree on `EventName` for a computed channel
/// to connect.
fn receiver_event_names(m: &crate::ir::Module) -> Vec<String> {
    super::nodes_of(m, crate::ir::gate_class::CUSTOM_EVENT)
        .iter()
        .filter_map(
            |n| match n.properties.get(&crate::intern::intern("EventName")) {
                Some(crate::ir::Literal::String(s)) => Some(s.clone()),
                _ => None,
            },
        )
        .collect()
}

/// A single-named-output const mod result routed through a `const string`
/// PARAMETER bakes as the scalar, not a record: a `Record([("r", ...)])` reaching
/// the emitter aborts the compile with `UnrepresentableLiteral`. `lower_ok`
/// asserts no errors from any stage; the assertion pins the baked name.
#[test]
fn a_single_named_output_const_mod_bakes_a_custom_event_channel_name() {
    let m = lower_ok(
        "mod ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         const mod chan(p: const string) -> (r: string) { out r = p .. \"_evt\" }\n\
         const NAME = chan(\"hit\")\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(NAME, hp) }",
    );
    let names = event_name_properties(&m);
    assert_eq!(
        names,
        vec!["hit_evt".to_string()],
        "a single named output's value must bake as the channel name, got {names:?}"
    );
}

#[test]
fn two_call_sites_bake_two_different_channels() {
    let m = lower_ok(
        "mod ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"a\", hp) ping(\"b\", hp) }",
    );
    let mut names = event_name_properties(&m);
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
}

/// A `chip` shares ONE compiled body per cache key across call sites, unlike a
/// `mod` (re-inlined fresh every time). A const param's value is baked into
/// that body, so the key must carry it, or call site B silently reuses the
/// body built with call site A's constant.
#[test]
fn two_chip_call_sites_get_two_bodies_not_one_reused() {
    let m = lower_ok(
        "chip ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"a\", hp) ping(\"b\", hp) }",
    );
    let mut names = event_name_properties(&m);
    names.sort();
    assert_eq!(
        names,
        vec!["a".to_string(), "b".to_string()],
        "each call site needs its OWN body; a shared template would bake one \
         name twice, got {names:?}"
    );
    let keys: crate::collections::HashSet<_> = m.chips.values().map(|c| c.template_key).collect();
    assert_eq!(
        keys.len(),
        2,
        "differing const args must produce differing template keys, got {keys:?}"
    );
}

/// A const param consumes no pin, so every LATER param's pin index shifts.
/// `build_chip_module` and `wire_chip_args_and_outputs` skip it in lockstep, or
/// the runtime args land on the wrong pins. Checked with the const param FIRST
/// (every following param shifts) by confirming the runtime arg reaches the body.
#[test]
fn a_leading_const_param_does_not_shift_later_pins() {
    let m = lower_ok(
        "chip ping(name: const string, v: int) -> (r: int) { out r = v * 2 }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { let out1 = ping(\"x\", hp) }",
    );
    let chip = m.chips.values().next().expect("one chip instance");
    // Exactly one value pin (`v`); `name` must NOT have produced one. The
    // auto-exec `_exec_in` pin is the only other input.
    let pin_names: Vec<&str> = chip
        .inputs
        .iter()
        .filter_map(|id| chip.nodes.get(id))
        .filter_map(|n| n.properties.get(&crate::intern::sym::PORT_LABEL))
        .filter_map(|l| match l {
            crate::ir::Literal::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !pin_names.contains(&"name"),
        "a const param must not become a MicrochipInput pin, got {pin_names:?}"
    );
    assert!(
        pin_names.contains(&"v"),
        "the runtime param must keep its pin, got {pin_names:?}"
    );
}

// A computed channel name sent via a named constant reaches a receiver bound to
// the SAME constant: the sender's and receiver's baked `EventName` values are
// byte-identical, and the receiver accepts a named constant like `CH` (no WS028).
#[test]
fn a_named_constant_round_trips_from_sender_to_receiver() {
    let m = lower_ok(
        "const CH = \"evt_\" .. \"died\"\n\
         in go: exec\n\
         var n: int = 0\n\
         on go { SendCustomEvent(CH, n) }\n\
         on CustomEvent(CH) -> (v: int) { n = v }",
    );
    let sent = event_name_properties(&m);
    let received = receiver_event_names(&m);
    assert_eq!(sent, vec!["evt_died".to_string()]);
    assert_eq!(
        sent, received,
        "sender and receiver must bake the same EventName for the computed \
         channel to actually connect, got sender={sent:?} receiver={received:?}"
    );
}

// The channel need not be a bare name: a computed expression in the same
// positional slot folds and bakes identically.
#[test]
fn a_computed_channel_expression_bakes_as_a_receiver_channel_name() {
    let m = lower_ok(
        "const PREFIX = \"evt_\"\n\
         var n: int = 0\n\
         on CustomEvent(PREFIX .. \"died\") -> (v: int) { n = v }",
    );
    assert_eq!(receiver_event_names(&m), vec!["evt_died".to_string()]);
}

/// The same constant at two call sites shares ONE template: the key
/// discriminates on the const VALUE, not merely on a const param being present.
#[test]
fn two_chip_call_sites_with_the_same_const_share_one_template() {
    let m = lower_ok(
        "chip ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"same\", hp) ping(\"same\", hp) }",
    );
    let keys: crate::collections::HashSet<_> = m.chips.values().map(|c| c.template_key).collect();
    assert_eq!(
        keys.len(),
        1,
        "identical const args should share a template, got {keys:?}"
    );
}

/// A `const` parameter's value has no wire (`lower_chip_call_inline` records it
/// in `scoped_consts` instead of binding a port), so reading it in a plain WIRE
/// position (`n + m`) resolves through the constant environment. Both halves are
/// asserted: no `_Unsupported` (only a WSP001 warning), and the constant reaches
/// the consuming gate as an inlined operand.
#[test]
fn a_const_param_read_in_a_wire_position_inlines_as_a_literal() {
    let m = lower_ok(
        "mod addk(n: const int, m: int) -> (r: int) { out r = n + m }\n\
         in go: exec\nvar hp: int = 0\non go { let z = addk(5, hp) }",
    );
    assert!(
        super::nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "a const param in a wire position must not lower to _Unsupported: {:?}",
        m.nodes.values().map(|n| n.gate_class).collect::<Vec<_>>()
    );
    let adds = super::nodes_of(&m, "BrickComponentType_Expr_MathAdd");
    assert_eq!(adds.len(), 1, "expected exactly one MathAdd");
    assert!(
        adds[0]
            .properties
            .values()
            .any(|v| *v == crate::ir::Literal::Int(5)),
        "the const param's value must be inlined as an operand, got {:?}",
        adds[0].properties
    );
}

/// The same const param used where it is only ever READ as a constant costs
/// nothing: its materialized literal is inlined and pruned, not left as an
/// orphan gate.
#[test]
fn a_const_param_used_only_as_config_still_emits_no_literal_gate() {
    let m = lower_ok(
        "mod ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\nvar hp: int = 0\non go { ping(\"died\", hp) }",
    );
    assert!(
        super::nodes_of(&m, crate::ir::gate_class::LITERAL).is_empty()
            && super::nodes_of(&m, crate::ir::gate_class::STRING_CONCATENATE).is_empty(),
        "a const-only param must leave no literal source gate behind: {:?}",
        m.nodes.values().map(|n| n.gate_class).collect::<Vec<_>>()
    );
}

/// A `const mod` call in a constant-only gate-config position BAKES its value
/// into the gate. Typecheck accepting it (`assert_no_diags`) is not enough: with
/// `EventName` unset, the program compiles clean and the channel is missing.
#[test]
fn a_const_mod_call_bakes_as_a_custom_event_channel_name() {
    let m = lower_ok(
        "const mod evtName(kind: string) -> string { return \"evt_\" .. kind }\n\
         mod ping(v: int) { SendCustomEvent(evtName(\"died\"), v) }\n\
         in go: exec\nvar hp: int = 0\non go { ping(hp) }",
    );
    assert_eq!(
        event_name_properties(&m),
        vec!["evt_died".to_string()],
        "the const-mod call must bake as the channel name"
    );
}

/// A certified receiver-method call in the same position bakes too: lowering
/// evaluates certified method calls the way typecheck's full evaluator does,
/// or the gate ships with no channel.
#[test]
fn a_certified_method_call_bakes_as_a_custom_event_channel_name() {
    let m =
        lower_ok("in go: exec\nvar hp: int = 0\non go { SendCustomEvent(\"died\".ToUpper(), hp) }");
    assert_eq!(
        event_name_properties(&m),
        vec!["DIED".to_string()],
        "a certified method call must bake as the channel name"
    );
}

/// A block-scope **`const`** destructure feeding a constant-only config slot
/// BAKES its value into the gate, in every scope and spelling. Asserted against
/// the baked `EventName` PROPERTY: the IR dump does not render component data,
/// so a missing channel prints the same as a baked one.
#[test]
fn a_block_scope_const_destructure_bakes_a_custom_event_channel_name() {
    let rows: &[(&str, &str, &str)] = &[
        (
            "a const destructure in a mod body",
            "mod f(v: int) {\n  const { chan } = { chan: \"evt_died\" }\n  SendCustomEvent(chan, v)\n}\n\
             in go: exec\nvar hp: int = 0\non go { f(hp) }",
            "evt_died",
        ),
        // `{ chan: c2 }` binds `c2`, and it is `c2` that carries the value.
        (
            "the alias form bakes the aliased name's value",
            "mod f(v: int) {\n  const { chan: c2 } = { chan: \"evt_aliased\" }\n  SendCustomEvent(c2, v)\n}\n\
             in go: exec\nvar hp: int = 0\non go { f(hp) }",
            "evt_aliased",
        ),
        // A HANDLER body rather than a mod body: a separate scope path.
        (
            "a const destructure in a handler body",
            "in go: exec\nvar hp: int = 0\n\
             on go {\n  const { chan } = { chan: \"evt_handler\" }\n  SendCustomEvent(chan, hp)\n}",
            "evt_handler",
        ),
        // The guarantee `const string` is supposed to make.
        (
            "a destructured const satisfies a const parameter and bakes through it",
            "mod send(name: const string, v: int) { SendCustomEvent(name, v) }\n\
             mod f(v: int) {\n  const { chan } = { chan: \"evt_param\" }\n  send(chan, v)\n}\n\
             in go: exec\nvar hp: int = 0\non go { f(hp) }",
            "evt_param",
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = super::compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let names = event_name_properties(&r.module);
        if !errors.is_empty() || names != [want] {
            failures.push(format!(
                "{label}: errors {errors:?}, channels {names:?}, want [{want:?}]\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Resolve, typecheck and lower `src`, returning LOWERING's error codes.
/// Unlike [`lower_ok`] this tolerates lowering errors, for the cases where
/// lowering reports something typecheck cannot. Typecheck must still be clean,
/// so the program did lower.
fn lowering_codes(src: &str) -> Vec<String> {
    let crate::compile::FrontEnd {
        typecheck: tc,
        lowered,
        ..
    } = crate::compile::front_end(
        src,
        "test",
        None,
        crate::lower::FoldMode::ForceOff,
        &crate::FsLoader,
    );
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    lowered
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::Severity::Error)
        .map(|d| d.code.clone())
        .collect()
}

/// A named chip's body is a `Block`, so its `const`s skip `build_const_env` and
/// are recorded into `scoped_consts`; the chip's child ctx opens a frame for
/// them. A lost constant bakes an EMPTY channel (a gate that never fires) with
/// no diagnostic from either stage, so the BAKED value is asserted.
#[test]
fn a_named_chips_own_const_bakes_in_its_own_handler() {
    const BODY: &str = "const a = \"ev\"\n\
           const b = a .. \"t\"\n\
           on t { send(b) }";
    let via_chip = lower_ok(&format!(
        "mod send(name: const string) {{ SendCustomEvent(name, 1) }}\n\
         chip Named(t: exec) {{ {BODY} }}\n\
         in go: exec\nlet r = Named(go)"
    ));
    assert_eq!(
        event_name_properties(&via_chip),
        vec!["evt".to_string()],
        "a chip-body const must reach the chip's own handler; an empty name here \
         means the constant was dropped rather than baked"
    );

    // The `mod` spelling of the identical body bakes the same channel.
    let via_mod = lower_ok(&format!(
        "mod send(name: const string) {{ SendCustomEvent(name, 1) }}\n\
         mod Named(t: exec) {{ {BODY} }}\n\
         in go: exec\nlet r = Named(go)"
    ));
    assert_eq!(
        event_name_properties(&via_mod),
        event_name_properties(&via_chip),
        "the chip and mod spellings of the same body must bake the same channel"
    );
}

/// The nested form of the case above: a chip declared INSIDE another chip is
/// built against the constants of the scope its declaration sees, so the
/// ENCLOSING chip's body constants reach it, as typecheck (whose scopes nest)
/// resolves them, with no WS028.
#[test]
fn a_chip_inside_a_chip_reads_the_enclosing_chips_constants() {
    let src = "mod send(name: const string) { SendCustomEvent(name, 1) }\n\
         chip Outer(t: exec) { const ch = \"deep\"\n\
           chip Inner(u: exec) { on u { send(ch) } }\n\
           let q = Inner(t) }\n\
         in go: exec\nlet r = Outer(go)";
    let codes = lowering_codes(src);
    assert!(
        !codes.iter().any(|c| c == "WS028"),
        "the enclosing chip's constant did not resolve: {codes:?}"
    );
    assert_eq!(
        event_name_properties(&lower_ok(src)),
        vec!["deep".to_string()],
        "the inner chip must bake the enclosing chip's constant"
    );
}
