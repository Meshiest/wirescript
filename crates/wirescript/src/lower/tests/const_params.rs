//! A const parameter must reach the positions that require a literal — the
//! whole point of the feature. `SendCustomEvent`'s channel name is the
//! sharpest case: it is gate CONFIG, not a wire, so a non-constant there is an
//! error and a constant bakes into the component data.

/// Resolve, typecheck and lower `src`, asserting no errors, and return the IR.
/// Folding is FORCED OFF so any gate that is absent is absent because const
/// evaluation removed it, never because the optimizer did.
pub(crate) fn lower_ok(src: &str) -> crate::ir::Module {
    let resolved = crate::resolve(src, "test", &crate::FsLoader);
    assert!(
        resolved
            .diagnostics
            .iter()
            .all(|d| d.severity != crate::Severity::Error),
        "resolve errors: {:?}",
        resolved.diagnostics
    );
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    let out = crate::lower::lower(crate::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: crate::lower::FoldMode::ForceOff,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    });
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.severity != crate::Severity::Error),
        "lower errors: {:?}",
        out.diagnostics
    );
    out.module
}

/// Every node of `class` in `m` and its nested chip modules.
pub(crate) fn nodes_of<'a>(m: &'a crate::ir::Module, class: &str) -> Vec<&'a crate::ir::Node> {
    let mut out: Vec<&crate::ir::Node> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .collect();
    for chip in m.chips.values() {
        out.extend(nodes_of(chip, class));
    }
    out
}

/// The baked `EventName` of every `SendCustomEvent` node. `EventName` is a
/// data-struct field, not a pre-interned `sym::` constant (see
/// `intern.rs`/`port_registry.rs`'s `WirePort::EventName`), so it's interned
/// on the fly exactly like `lower/call/builtin.rs` does when baking it.
fn event_name_properties(m: &crate::ir::Module) -> Vec<String> {
    nodes_of(m, crate::ir::gate_class::PSEUDO_SEND_CUSTOM_EVENT)
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
/// CustomEvent(...)`). Mirrors `event_name_properties` but reads the RECEIVER
/// gate class, not the sender's — the two must agree on `EventName` for a
/// computed channel to actually connect a sender to its receiver.
fn receiver_event_names(m: &crate::ir::Module) -> Vec<String> {
    nodes_of(m, crate::ir::gate_class::PSEUDO_CUSTOM_EVENT)
        .iter()
        .filter_map(
            |n| match n.properties.get(&crate::intern::intern("EventName")) {
                Some(crate::ir::Literal::String(s)) => Some(s.clone()),
                _ => None,
            },
        )
        .collect()
}

#[test]
fn a_const_param_bakes_a_custom_event_channel_name() {
    let m = lower_ok(
        "mod ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"died\", hp) }",
    );
    let names = event_name_properties(&m);
    assert_eq!(
        names,
        vec!["died".to_string()],
        "the const param must bake as the channel name, got {names:?}"
    );
}

/// The third manifestation of the single-named-output wrapping bug (see
/// `a_single_named_output_const_mod_result_bakes_as_a_scalar` in
/// `const_init.rs` for the first, and
/// `a_single_named_output_const_mod_result_is_scalar_config_not_a_record` in
/// `typecheck/tests.rs` for the second): routed through a `const string`
/// PARAMETER, the record slipped past typecheck's own constant-config check
/// and reached the emitter, which aborted the whole compile with
/// `UnrepresentableLiteral { field: "EventName", literal: Record([("r",
/// String("hit_evt"))]) }`. `lower_ok` asserts no errors from any stage, so
/// this fails on the abort; the assertion then pins the baked name itself.
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
/// that body, so the key must carry it — otherwise call site B silently reuses
/// the body built with call site A's constant.
#[test]
fn a_const_param_bakes_a_channel_name_in_a_chip_body() {
    let m = lower_ok(
        "chip ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"died\", hp) }",
    );
    let names = event_name_properties(&m);
    assert_eq!(
        names,
        vec!["died".to_string()],
        "a chip's const param must bake as the channel name, got {names:?}"
    );
}

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
/// `build_chip_module` and `wire_chip_args_and_outputs` must skip it in
/// lockstep or the runtime args land on the wrong pins — checked with the
/// const param FIRST (worst case: every following param shifts) by confirming
/// the runtime arg still reaches the body's own gate.
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

// Task 10: the receiver side of a `CustomEvent` handler must bake a named
// constant's resolved value as its `EventName`, exactly as the sender side
// already does — otherwise a computed channel name can be sent but never
// received (the two `EventName` properties would never match at runtime).
#[test]
fn a_named_constant_bakes_as_a_receiver_channel_name() {
    let m = lower_ok(
        "const CH = \"evt_\" .. \"died\"\nvar n: int = 0\non CustomEvent(CH) -> (v: int) { n = v }",
    );
    assert_eq!(receiver_event_names(&m), vec!["evt_died".to_string()]);
}

// Task 10's actual point: a computed channel name sent via a named constant
// must reach a receiver bound to the SAME constant — i.e. the sender's and
// receiver's baked `EventName` values must be byte-identical, not just each
// individually non-empty. Before Task 10 this compiled fine (the sender side
// already folded named constants), but the receiver side rejected `CH`
// outright (WS028), so the round trip never lowered at all.
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

/// The same constant at two call sites should still share ONE template — the
/// key must discriminate on the const VALUE, not merely on a const param being
/// present (which would defeat template reuse entirely).
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


// ---------------------------------------------------------------------
// Task 13 fix round 1.

/// A `const` parameter's value has no wire — `lower_chip_call_inline` records
/// it in `scoped_consts` instead of binding a port — so reading it in a plain
/// WIRE position (`n + m`) has to resolve through the constant environment.
/// It did not: `lower_ident` fell through to `_Unsupported`, which is only a
/// WSP001 warning, so the program compiled and the operand silently did
/// nothing at runtime. Regression guard for that (`lower/expr.rs`'s `None`
/// arm), asserting BOTH halves: no `_Unsupported`, and the constant actually
/// reached the consuming gate as an inlined operand.
#[test]
fn a_const_param_read_in_a_wire_position_inlines_as_a_literal() {
    let m = lower_ok(
        "mod addk(n: const int, m: int) -> (r: int) { out r = n + m }\n\
         in go: exec\nvar hp: int = 0\non go { let z = addk(5, hp) }",
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "a const param in a wire position must not lower to _Unsupported: {:?}",
        m.nodes.values().map(|n| n.gate_class).collect::<Vec<_>>()
    );
    let adds = nodes_of(&m, "BrickComponentType_WireGraph_Expr_MathAdd");
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

/// The same const param used where it is only ever READ as a constant still
/// costs nothing — the literal materialized by the fix above must be inlined
/// and pruned, not left behind as an orphan gate.
#[test]
fn a_const_param_used_only_as_config_still_emits_no_literal_gate() {
    let m = lower_ok(
        "mod ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\nvar hp: int = 0\non go { ping(\"died\", hp) }",
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::LITERAL).is_empty()
            && nodes_of(&m, crate::ir::gate_class::STRING_CONCATENATE).is_empty(),
        "a const-only param must leave no literal source gate behind: {:?}",
        m.nodes.values().map(|n| n.gate_class).collect::<Vec<_>>()
    );
}

/// THE acceptance test for the feature's headline claim, and the one the
/// brief's own Step-1 test could not make: a `const mod` call in a
/// constant-only gate-config position must actually BAKE its value into the
/// gate. Typecheck accepting it (`assert_no_diags`) proves nothing on its own
/// — lowering's `literal_for_property_port` folded only env-less literals and
/// bare identifiers, so this program compiled clean with `EventName` unset and
/// the channel silently missing.
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

/// The same accept-but-drop hole, reached through a certified receiver-method
/// call instead of a `const mod` call. Previously WS028 at typecheck; once
/// typecheck was widened to the full evaluator, lowering had to follow or the
/// gate shipped with no channel.
#[test]
fn a_certified_method_call_bakes_as_a_custom_event_channel_name() {
    let m = lower_ok(
        "in go: exec\nvar hp: int = 0\non go { SendCustomEvent(\"died\".ToUpper(), hp) }",
    );
    assert_eq!(
        event_name_properties(&m),
        vec!["DIED".to_string()],
        "a certified method call must bake as the channel name"
    );
}

/// A block-scope **`const`** destructure feeding a constant-only config slot
/// must BAKE its value into the gate.
///
/// This asserts against the baked `EventName` PROPERTY, not the IR dump,
/// because the dump cannot see this class of bug at all: the broken and the
/// working programs both print exactly one `SendCustomEvent` gate and differ
/// only in the component data the dump never renders. The same distinction was
/// confirmed end-to-end against a real `.brdb` with brdb's `read_components`
/// example (0 occurrences broken vs 1 fixed) — see the task report.
#[test]
fn a_block_scope_const_destructure_bakes_a_custom_event_channel_name() {
    let m = lower_ok(
        "mod f(v: int) {\n\
           const { chan } = { chan: \"evt_died\" }\n\
           SendCustomEvent(chan, v)\n\
         }\n\
         in go: exec\nvar hp: int = 0\non go { f(hp) }",
    );
    assert_eq!(
        event_name_properties(&m),
        vec!["evt_died".to_string()],
        "a const destructure must bake as the channel name, not ship an empty one"
    );
}

/// The ALIAS form of the same thing — `{ chan: c2 }` binds `c2`, and it is
/// `c2` that must carry the value into the config slot.
#[test]
fn a_block_scope_const_destructure_alias_bakes_a_custom_event_channel_name() {
    let m = lower_ok(
        "mod f(v: int) {\n\
           const { chan: c2 } = { chan: \"evt_aliased\" }\n\
           SendCustomEvent(c2, v)\n\
         }\n\
         in go: exec\nvar hp: int = 0\non go { f(hp) }",
    );
    assert_eq!(
        event_name_properties(&m),
        vec!["evt_aliased".to_string()],
        "an aliased const destructure must bake the aliased name's value"
    );
}

/// Inside a HANDLER body rather than a mod body — a separate scope path.
#[test]
fn a_const_destructure_in_a_handler_body_bakes_a_custom_event_channel_name() {
    let m = lower_ok(
        "in go: exec\nvar hp: int = 0\n\
         on go {\n\
           const { chan } = { chan: \"evt_handler\" }\n\
           SendCustomEvent(chan, hp)\n\
         }",
    );
    assert_eq!(
        event_name_properties(&m),
        vec!["evt_handler".to_string()],
        "a const destructure in a handler body must bake too"
    );
}

/// A destructured `const` passed to a `const` PARAMETER, which then reaches
/// the config slot — the guarantee `const string` is supposed to make.
#[test]
fn a_destructured_const_satisfies_a_const_parameter_and_bakes() {
    let m = lower_ok(
        "mod send(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         mod f(v: int) {\n\
           const { chan } = { chan: \"evt_param\" }\n\
           send(chan, v)\n\
         }\n\
         in go: exec\nvar hp: int = 0\non go { f(hp) }",
    );
    assert_eq!(
        event_name_properties(&m),
        vec!["evt_param".to_string()],
        "a destructured const must satisfy a const param and bake through it"
    );
}

/// Resolve, typecheck and lower `src`, returning LOWERING's error codes.
/// Unlike [`lower_ok`] this tolerates lowering errors — it exists for the
/// cases where the point IS that lowering reports something typecheck cannot.
/// Typecheck must still be clean, or the test would be asserting about a
/// program that never had a chance to lower.
fn lowering_codes(src: &str) -> Vec<String> {
    let resolved = crate::resolve(src, "test", &crate::FsLoader);
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    crate::lower::lower(crate::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: crate::lower::FoldMode::ForceOff,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    })
    .diagnostics
    .iter()
    .filter(|d| d.severity == crate::Severity::Error)
    .map(|d| d.code.clone())
    .collect()
}

/// A named chip's body is a `Block`, so its `const`s never go through
/// `build_const_env`; they are recorded into `scoped_consts`, and the chip's
/// child ctx used to open no frame for them at all. The constant was dropped
/// on the floor and `SendCustomEvent` baked an EMPTY channel — a gate that
/// silently never fires — with nothing reported by either stage.
///
/// Asserted on the BAKED value rather than on a diagnostic, because a
/// diagnostic is exactly the oracle that missed this: every spelling here
/// type-checks and lowers without a word.
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

    // The `mod` spelling of the identical body always worked, which is what
    // made the chip one look like a language rule instead of a lost frame.
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

/// The residual of the case above: a chip declared INSIDE another chip is
/// built against its own fresh const stack, so the ENCLOSING chip's body
/// constants do not reach it, while typecheck (whose scopes nest) resolves
/// them fine. Making that work needs declaration-site const scoping for chip
/// bodies, which is a design change; what must NOT happen meanwhile is the
/// old behaviour — baking an empty channel name and saying nothing.
#[test]
fn a_config_value_that_does_not_resolve_at_lowering_reports_instead_of_baking_a_default() {
    let codes = lowering_codes(
        "mod send(name: const string) { SendCustomEvent(name, 1) }\n\
         chip Outer(t: exec) { const ch = \"deep\"\n\
           chip Inner(u: exec) { on u { send(ch) } }\n\
           let q = Inner(t) }\n\
         in go: exec\nlet r = Outer(go)",
    );
    assert!(
        codes.iter().any(|c| c == "WS028"),
        "a constant-only config field whose value did not resolve during lowering \
         must report WS028, not silently ship the type default; got {codes:?}"
    );
}
