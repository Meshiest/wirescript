//! Event handlers and triggers: event gates, the outputs they bind and the
//! exec they start.

use super::*;

/// An or-triggered handler (`on a | b { ... }`) runs its body when EITHER part
/// fires: `lower_handler` lowers the `Trigger::Union` body once per part.
#[test]
fn union_trigger_lowers_body_per_part() {
    let r = compile_ok("var v: int = 0\nin t: exec\nin u: exec\non t | u { v = v + 1 }");
    assert_eq!(
        count_class(&r.module, gc::VAR_INCREMENT),
        2,
        "the body must lower once per union trigger part (t and u)"
    );
    // Each input drives one of the two increments (no dropped/dead trigger).
    let inputs: Vec<crate::ir::NodeId> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::MICROCHIP_INPUT)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(inputs.len(), 2);
    assert_eq!(r.module.inputs.len(), 2);
    for inp in &inputs {
        assert_eq!(r.module.nodes[inp].kind, NodeKind::Input);
        assert!(
            r.module.wires.iter().any(|w| w.source.node_id == *inp),
            "each union-trigger input must drive a handler body copy"
        );
    }
}

/// `.exec` names the exec output of a data-carrying event. `GlobalCustomEvent`
/// carries data slots, so `GlobalCustomEvent(c)` types as a record with its exec
/// output FIRST; `.exec` selects that output explicitly so the event composes
/// into a `Union(...)` trigger.
#[test]
fn data_carrying_event_exec_composes_into_union() {
    let r = compile_ok(
        "var v: int = 0\nin reset: exec\n\
         on Union(reset, GlobalCustomEvent(\"ping\").exec) { v = v + 1 }",
    );
    assert!(
        !has_unsupported(&r.module),
        "GlobalCustomEvent(...).exec must resolve to the event's exec output, not _Unsupported"
    );
    let evt = find_gate(&r, gc::CUSTOM_EVENT_GLOBAL);
    let union = find_gate(&r, gc::UNION);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == evt && w.target.node_id == union),
        "the event's exec output must wire into the Union"
    );
}

/// `a.Union(b)` is the receiver form of `Union(a, b)`: the object binds the
/// first exec param, so a wide fan-in reads as a left-associative chain.
#[test]
fn union_exec_receiver_desugars_to_union_call() {
    let r = compile_ok("var v: int = 0\nin a: exec\nin b: exec\non a.Union(b) { v = v + 1 }");
    assert!(!has_unsupported(&r.module));
    let union = find_gate(&r, gc::UNION);
    let inputs: Vec<crate::ir::NodeId> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::MICROCHIP_INPUT)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(inputs.len(), 2);
    for inp in &inputs {
        assert!(
            wired_reachable(&r, *inp, union),
            "each exec input must reach the Union receiver gate"
        );
    }
}

/// `a.Union(b).Union(c)` chains left-associatively: two Union gates, one per
/// `.Union` in the chain, the same shape as nested `Union(Union(a, b), c)`.
#[test]
fn union_exec_receiver_chains_left_associatively() {
    let r = compile_ok(
        "var v: int = 0\nin a: exec\nin b: exec\nin c: exec\non a.Union(b).Union(c) { v = v + 1 }",
    );
    assert!(!has_unsupported(&r.module));
    assert_eq!(
        gate_count(&r, gc::UNION),
        2,
        "a two-link .Union chain must emit two Union gates"
    );
}

/// A captured handler with a NON-event trigger (`let e = on go { ... }` where
/// `go` is an exec input) lowers its body and captures its exit as `e`.
#[test]
fn captured_handler_on_exec_input_lowers_body() {
    let r = compile_ok(
        "in go: exec\nvar x: int = 0\nout r: exec\nlet e = on go { x = x + 1 }\non e { emit r }",
    );
    let inc = find_gate(&r, gc::VAR_INCREMENT);
    // The increment fires off the input, and its exit chains onward (to `on e`).
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == inc),
        "the captured body must be triggered from the input"
    );
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == inc),
        "the captured body's exit must chain into `on e`"
    );
}

/// `on ControllerLeft() -> { userId: userId }` exposes the gate's `UserId`
/// output. The gate is a pure source, so a wire SOURCED from its `UserId` port
/// into the handler body proves the id is bound.
#[test]
fn controller_left_exposes_user_id() {
    let r = compile_ok(
        "var lastLeft: string\non ControllerLeft() -> { userId: userId } {\n  lastLeft = userId\n}",
    );
    assert!(
        !has_unsupported(&r.module),
        "userId must bind to the gate's UserId port, not an _Unsupported placeholder"
    );
    let ev = find_gate(&r, "BrickComponentType_Event_Gamemode_ControllerLeft");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == ev && w.source.port.as_str() == "UserId"),
        "the handler body must consume the event gate's UserId output"
    );
}

/// `on ZoneEntered(zone = zoneA) -> { character: character }` wires the
/// `zoneA` value into the event gate's `Zone` input port. Event gates are
/// otherwise pure sources, so an input wire into the gate (sourced from the
/// `in` port) proves the bind.
#[test]
fn zone_event_input_binding() {
    let r = compile_ok(
        "in zoneA: zone\non ZoneEntered(zone = zoneA) -> { character: character } {\n  PrintToConsole(\"entered\")\n}",
    );
    let ev = find_gate(&r, "BrickComponentType_Internal_CharacterZoneEvent_Entered");
    let zone_wire = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == ev)
        .expect("ZoneEntered gate must have its Zone input wired");
    assert_eq!(
        class_of(&r, zone_wire.source.node_id),
        gc::MICROCHIP_INPUT,
        "Zone input should be wired from the `in zoneA` port"
    );
    // The zone gate's exec output is named `Exec`, not `ExecOut`. The handler
    // body must chain from that port or the game silently drops the connection.
    let exec_wire = r
        .module
        .wires
        .iter()
        .find(|w| w.source.node_id == ev)
        .expect("ZoneEntered gate must chain its exec into the handler body");
    assert_eq!(
        exec_wire.source.port.as_str(),
        "Exec",
        "zone event exec output port must be `Exec`, not `ExecOut`"
    );
}

/// `on Loaded()` is one Load Event gate whose `ExecOut` starts the body.
#[test]
fn loaded_event_drives_its_handler() {
    let r = compile_ok("var loads: int = 0\non Loaded() {\n  loads += 1\n}");
    let ev = find_gate(&r, "BrickComponentType_Event_Loaded");
    assert_eq!(r.module.nodes[&ev].kind, NodeKind::Event);
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == ev
            && w.source.port == WirePort::ExecOut
            && w.target.port == WirePort::Exec),
        "the Load Event's ExecOut must start the handler"
    );
}

#[test]
fn damage_and_zone_events_lower_to_event_gates() {
    let r = compile_ok(
        "on CharacterDamaged() -> { character: char, damage: dmg } {\n  char.DisplayText(\"ouch ${dmg}\")\n}\non EntityZoneEntered() -> { entity: e } {\n  e.SetTag(\"inside\")\n}\non ProjectileZoneEntered() -> { character: shooter } {\n  shooter.DisplayText(\"hit!\")\n}",
    );
    for class in [
        "BrickComponentType_Event_Gamemode_CharacterDamaged",
        "BrickComponentType_Internal_EntityZoneEvent_Entered",
        "BrickComponentType_Internal_ProjectileZoneEvent_Entered",
    ] {
        assert!(has_gate(&r, class), "expected a {class} event gate");
    }
}

/// `BrickChanged` / `BrickRemoved` bind the gate's `Brick` and `Character`
/// outputs, and a use of either reads from that port.
#[test]
fn brick_zone_events_bind_brick_and_character() {
    for (event, class) in [
        (
            "BrickChanged",
            "BrickComponentType_Internal_ZoneEvent_BrickChanged",
        ),
        (
            "BrickRemoved",
            "BrickComponentType_Internal_ZoneEvent_BrickRemoved",
        ),
    ] {
        let r = compile_ok(&format!(
            "in z: zone\nvar lastBrick: brick\nvar who: character\n\
             on {event}(zone = z) -> (brick, character) {{\n  lastBrick = brick\n  who = character\n}}"
        ));
        let ev = find_gate(&r, class);
        let mut ports: Vec<&str> = r
            .module
            .wires
            .iter()
            .filter(|w| {
                w.source.node_id == ev
                    && w.target.port == WirePort::Value
                    && class_of(&r, w.target.node_id) == gc::VAR_SET
            })
            .map(|w| w.source.port.as_str())
            .collect();
        ports.sort_unstable();
        assert_eq!(
            ports,
            ["Brick", "Character"],
            "{event}: bindings must wire from the gate's ports"
        );
    }
}

/// `ChatCommand` config fills the gate's `CommandName` and `HelpText`: by
/// position, or with `Description = ...` as an alias for `HelpText`.
/// Identifier params bind the controller/arguments outputs.
#[test]
fn chat_command_config_sets_gate_data() {
    let rows = [
        (
            "on ChatCommand(\"greet\", \"Greets the player\") -> { controller: player, arguments: args } {\n  player.DisplayText(\"hi ${args}\")\n}",
            "greet",
            "Greets the player",
        ),
        (
            "on ChatCommand(\"wave\", Description = \"Wave at everyone\") { }",
            "wave",
            "Wave at everyone",
        ),
    ];
    let mut failures = Vec::new();
    for (src, name, help) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let evt = r
            .module
            .nodes
            .values()
            .find(|n| n.gate_class == "BrickComponentType_Event_ChatCommand");
        let prop = |field: &str| match evt.and_then(|e| e.properties.get(&intern(field))) {
            Some(Literal::String(s)) => Some(s.clone()),
            _ => None,
        };
        let got = (prop("CommandName"), prop("HelpText"));
        if !errors.is_empty() || got != (Some(name.to_string()), Some(help.to_string())) {
            failures.push(format!("{src}: errors {errors:?}, got {got:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn on_input_splitter_field_trigger_selects_named_port() {
    // `on split.PressedE` where `split = pl.InputReader()` triggers from the
    // splitter's `bPressedE` output, not its default `InputForward` port: the
    // local-trigger path must honour the trigger field.
    let src = "in pl: character\n\
               let split = pl.InputReader()\n\
               on split.PressedE { pl.ShowStatusMessage(\"e\") }";
    let r = compile_ok(src);
    let splitter = find_gate(&r, gc::INPUT_SPLITTER);
    let show = find_gate(&r, gc::PLAYERSTATE_SHOW_STATUS);
    let trig = r
        .module
        .wires
        .iter()
        .find(|w| w.source.node_id == splitter && w.target.node_id == show)
        .expect("splitter must drive the handler body");
    assert_eq!(
        trig.source.port.as_str(),
        "bPressedE",
        "on split.PressedE must trigger from bPressedE"
    );
}

#[test]
fn var_value_trigger_lowers_body() {
    let r = compile_ok("var x: int = 0\non x.value {\n  let y = x + 10\n}\nout result = x");
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "let inside on var.value handler should produce MathAdd gate"
    );
}

#[test]
fn var_value_trigger_nested_if_lowers() {
    let r = compile_ok(
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
    for class in [
        gc::COMPARE_EQUAL,
        "BrickComponentType_Expr_CompareLess",
        "BrickComponentType_Expr_LogicalAND",
    ] {
        assert!(
            has_gate(&r, class),
            "nested let inside on var.value + if should produce {class}"
        );
    }
}
