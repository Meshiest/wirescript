//! Tests for game-exposed gate config properties: enum config validation
//! (WS028), enum config landing as gate data, and a constant gate setting
//! stored exactly or reported (WS106).

use super::*;
use crate::ir::{Literal, Type};

/// Each `(label, source, code)` row typechecks to no error when `code` is
/// `None`, or reports the error `code`; `absent` codes never appear.
#[track_caller]
fn check_typecheck_rows(rows: &[(&str, &str, Option<&str>, &[&str])]) {
    let mut failures = Vec::new();
    for (label, src, code, absent) in rows {
        let got = errs(src);
        match code {
            None if !got.is_empty() => failures.push(format!("{label}: errors {got:?}\n{src}")),
            Some(c) if !got.iter().any(|g| g == c) => {
                failures.push(format!("{label}: want {c}, got {got:?}\n{src}"))
            }
            _ => {}
        }
        for a in *absent {
            if got.iter().any(|g| g == a) {
                failures.push(format!("{label}: unexpected {a} in {got:?}\n{src}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn a_config_argument_typechecks_or_says_why() {
    let adv = "in go: exec\nin c: character\nin weapon: entity\n";
    let ammo = |a: &str| {
        format!("{adv}on go {{\n  c.AddInventoryItemAdv(weapon, ammoOverride = {a})\n}}\n")
    };
    let missing_reserve = ammo("{ overrideStartingAmmo: true, resources: [{ loaded: 30 }] }");
    let missing_resources = ammo("{ overrideStartingAmmo: true }");
    let empty_resources = ammo("{ overrideStartingAmmo: false, resources: [] }");
    let runtime_ammo = format!(
        "{adv}in n: int\non go {{\n  c.AddInventoryItemAdv(weapon, ammoOverride = {{ overrideStartingAmmo: true, resources: [{{ loaded: n, reserve: 90 }}] }})\n}}\n"
    );
    let runtime_colors = format!(
        "{adv}in live: color\non go {{\n  c.AddInventoryItemAdv(weapon, meshColors = [live])\n}}\n"
    );
    let clock = |cfg: &str| {
        format!("static var n: int = 0\non Clock(interval = 2.0, {cfg}) {{\n  n = n + 1\n}}\n")
    };
    let (pulse_on, on_time, off_time) = (
        clock("pulseOn = false"),
        clock("onTime = 0.5"),
        clock("offTime = 0.5"),
    );
    check_typecheck_rows(&[
        (
            "`function`/`direction` on Easing are data-only enum config fields: bare member names are valid",
            "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bounce, direction = InOut)\n",
            None,
            &[],
        ),
        (
            "a raw int in range",
            "in t: float\nlet e = Easing(0.0, 1.0, t, function = 3)\n",
            None,
            &[],
        ),
        (
            "an out-of-range raw int",
            "in t: float\nlet e = Easing(0.0, 1.0, t, function = 99)\n",
            Some("WS028"),
            &[],
        ),
        (
            "the quoted-name form is accepted and validated",
            "in t: float\nlet e = Easing(0.0, 1.0, t, function = \"Bounce\")\n",
            None,
            &[],
        ),
        (
            "`bPulseOn` is in the Clock's data struct but the gate never reads it: WS041, not a silently baked property",
            &pulse_on,
            Some("WS041"),
            &[],
        ),
        ("the inert `OnTimeSeconds`", &on_time, Some("WS041"), &[]),
        ("the inert `OffTimeSeconds`", &off_time, Some("WS041"), &[]),
        (
            "a non-constant meshColors element can't fold into gate data: a clean WS028, never a broken wire",
            &runtime_colors,
            Some("WS028"),
            &[],
        ),
        (
            "a non-constant ammoOverride",
            &runtime_ammo,
            Some("WS028"),
            &[],
        ),
        (
            "a non-constant asset-ref config param",
            "in go: exec\nin p: controller\nin f: entity\non go {\n  p.DisplayText(\"hi\", font = f)\n}\n",
            Some("WS028"),
            &[],
        ),
        (
            "`isObject` is constant-only CustomEvent config; a variable would be dropped at lowering",
            "in flag: bool\nstatic var n: int = 0\non CustomEvent(\"dmg\", isObject = flag) -> (amount: int) {\n  n = amount\n}\n",
            Some("WS028"),
            &[],
        ),
        (
            "`interval` wires into the Clock's IntervalSeconds input port, so a variable there is legitimate",
            "in secs: float\nstatic var n: int = 0\non Clock(interval = secs, enabled = true) {\n  n = n + 1\n}\n",
            None,
            &[],
        ),
        (
            "`enabled` is a wire input too",
            "in flag: bool\nstatic var n: int = 0\non Clock(interval = 2.0, enabled = flag) {\n  n = n + 1\n}\n",
            None,
            &[],
        ),
        (
            "a resource record missing `reserve` is not silently baked as 0",
            &missing_reserve,
            Some("WS028"),
            &[],
        ),
        (
            "the ammoOverride record missing `resources`",
            &missing_resources,
            Some("WS028"),
            &[],
        ),
        (
            "an explicit `resources: []` is a deliberate no-op, not a silent default",
            &empty_resources,
            None,
            &[],
        ),
        (
            "the send's object-event Target is entity-typed: a non-entity is a clear WS003",
            "in go: exec\non go {\n  SendCustomEvent(\"dmg\", 7, target = 5)\n}\n",
            Some("WS003"),
            &[],
        ),
        (
            "any object-family reference (entity/character/controller) connects as Target",
            "on CharacterSpawned() -> { character: ch } {\n  SendCustomEvent(\"dmg\", 7, target = ch)\n}\n",
            None,
            &[],
        ),
        (
            "a general call trigger's `-> (a, b)` capture takes its types from the call's output record, \
             not `Any` (`la = a + b` was WS004)",
            "chip pair(x: int) -> (a: int, b: int) {\n  out a = x\n  out b = x + 1\n}\nin go: exec\nvar la: int = 0\n\
             on pair(5, exec = go) -> (a, b) {\n  la = a + b\n}\n",
            None,
            &[],
        ),
        (
            "a baked map entry has no gate to coerce through: a non-string key is not silently \"\"",
            "var m: Map<string, int> = { 1 => 10 }\n",
            Some("WS003"),
            &[],
        ),
        (
            "an atom key in a string map",
            "var m: Map<string, int> = { :alice => 1, :bob => 2 }\n",
            Some("WS003"),
            &[],
        ),
        (
            "a non-string baked value",
            "var v: Map<int, string> = { 1 => 5 }\n",
            Some("WS003"),
            &[],
        ),
        (
            "a correctly-typed string-keyed map",
            "var m: Map<string, int> = { \"a\" => 1, \"b\" => 2 }\n",
            None,
            &[],
        ),
    ]);
}

/// `zone` and `teleport` are reference types: they wire into their own inputs
/// only, and like a var ref cannot flow through a Select or back a variable.
#[test]
fn a_zone_or_teleport_reference_typechecks_or_says_why() {
    check_typecheck_rows(&[
        (
            "a zone value wires into ZoneEntered's `zone` input",
            "in z: zone\non ZoneEntered(zone = z) -> (character) {}\n",
            None,
            &[],
        ),
        (
            "an entity into a zone input",
            "in z: entity\non ZoneEntered(zone = z) -> (character) {}\n",
            Some("WS003"),
            &[],
        ),
        (
            "a teleport point teleports",
            "in e: entity\nin p: teleport\nin go: exec\non go {\n  e.Teleport(p)\n}\n",
            None,
            &[],
        ),
        (
            "a raw position does not teleport (that's SetLocation's job)",
            "in e: entity\nin go: exec\non go {\n  e.Teleport(Vec(0.0, 0.0, 100.0))\n}\n",
            Some("WS003"),
            &[],
        ),
        (
            "neither does an entity",
            "in e: entity\nin d: entity\nin go: exec\non go {\n  e.Teleport(d)\n}\n",
            Some("WS003"),
            &[],
        ),
        (
            "teleport points can't be selected by an if-expr",
            "in c: bool\nin a: teleport\nin b: teleport\nlet p = if c then a else b\n",
            Some("WS031"),
            &[],
        ),
        (
            "a zone can't back a variable gate",
            "var z: zone\n",
            Some("WS025"),
            &[],
        ),
        (
            "a teleport point can't back a variable gate",
            "var p: teleport\n",
            Some("WS025"),
            &[],
        ),
    ]);
}

fn prop<'a>(r: &'a LowerResult, class: &str, field: &str) -> Option<&'a Literal> {
    let node = find_gate(r, class);
    r.module.nodes[&node]
        .properties
        .get(&crate::intern::intern(field))
}

/// A constant config argument lands on its gate's data field, not a wire: by
/// the hand-coded friendly name or by its raw struct field name (any field in
/// the inventory `config` array is settable that way); enum members are bare
/// names, other scalars constants.
#[test]
fn a_constant_config_argument_lands_as_gate_data() {
    let s = |v: &str| Literal::String(v.to_string());
    let rows: Vec<(&str, &str, &str, Vec<(&str, Literal)>)> = vec![
        (
            "a bare enum member (EBREasingFunction::Bounce == 10)",
            "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bounce)\n",
            crate::ir::gate_class::MATH_EASING,
            vec![("Function", Literal::Int(10))],
        ),
        (
            "SweepSimple's bool and enum config (EBrickDirection::Y_Positive == 2)",
            "in go: exec\non go {\n  SweepSimple(100.0, direction = Y_Positive, detectBricks = false, bodyPartsOnly = true)\n}\n",
            crate::ir::gate_class::SWEEP_SIMPLE,
            vec![
                ("Direction", Literal::Int(2)),
                ("bDetectBricks", Literal::Bool(false)),
                ("bOnlyHitPlayerBodyParts", Literal::Bool(true)),
            ],
        ),
        (
            "ColorBlend's enum and bool config (EBRColorSpace::Oklab == 2)",
            "in a: color\nin b: color\nin t: float\nlet c = ColorBlend(a, b, t, blendSpace = Oklab, clampAlpha = false)\n",
            crate::ir::gate_class::COLOR_BLEND,
            vec![
                ("BlendSpace", Literal::Int(2)),
                ("bClampAlpha", Literal::Bool(false)),
            ],
        ),
        (
            "DisplayText's typeface and justify (Bold == 1, Center == 1)",
            "in go: exec\nin p: controller\non go {\n  p.DisplayText(\"hi\", typeface = Bold, justify = Center)\n}\n",
            crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT,
            vec![
                ("Typeface", Literal::Int(1)),
                ("Justification", Literal::Int(1)),
            ],
        ),
        (
            "raw bool field names",
            "in go: exec\non go {\n  SweepSimple(500.0, bOnlyHitPlayerBodyParts = true, bDetectBricks = true)\n}\n",
            crate::ir::gate_class::SWEEP_SIMPLE,
            vec![
                ("bOnlyHitPlayerBodyParts", Literal::Bool(true)),
                ("bDetectBricks", Literal::Bool(true)),
            ],
        ),
        (
            "the raw `Direction` resolves a bare member as the friendly `direction` does",
            "in go: exec\non go {\n  SweepSimple(500.0, Direction = Y_Positive)\n}\n",
            crate::ir::gate_class::SWEEP_SIMPLE,
            vec![("Direction", Literal::Int(2))],
        ),
        (
            "`Value` is a settings-only int on Cycle that no declared param covers",
            "in go: exec\non go {\n  let n = Cycle(4, Value = 2)\n  PrintToConsole(n)\n}\n",
            crate::ir::gate_class::EXEC_CYCLE,
            vec![("Value", Literal::Int(2))],
        ),
        (
            "the hand-coded `bodyPartsOnly` alias keeps setting the same field",
            "in go: exec\non go {\n  SweepSimple(500.0, bodyPartsOnly = true)\n}\n",
            crate::ir::gate_class::SWEEP_SIMPLE,
            vec![("bOnlyHitPlayerBodyParts", Literal::Bool(true))],
        ),
        (
            "constant event data inlines as typed data and the name bakes into EventName",
            "in go: exec\non go {\n  SendCustomEvent(\"spawn\", 42, 99)\n}\n",
            crate::ir::gate_class::SEND_CUSTOM_EVENT,
            vec![
                ("DataIn1", Literal::Int(42)),
                ("DataIn2", Literal::Int(99)),
                ("EventName", s("spawn")),
            ],
        ),
        (
            "the catalog's `isObject` matches case-insensitively: `isobject = true` bakes bIsObjectEvent",
            "static var last: int = 0\non CustomEvent(\"dmg\", isobject = true) -> (amount: int) {\n  last = amount\n}\n",
            crate::ir::gate_class::CUSTOM_EVENT,
            vec![("bIsObjectEvent", Literal::Bool(true))],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, class, fields) in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let nodes = nodes_of(&r.module, class);
        if nodes.len() != 1 {
            failures.push(format!("{label}: {} x {class}, want one", nodes.len()));
            continue;
        }
        for (field, want) in fields {
            let got = nodes[0].properties.get(&crate::intern::intern(field));
            if got != Some(want) {
                failures.push(format!("{label}: {field} = {got:?}, want {want:?}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// An event called as an expression emits its gate and composes like any exec
/// or value source: in a `Union`, in pure context (an event fires itself, so
/// no WS007), and with its input arguments wired in.
#[test]
fn an_event_called_as_an_expression_emits_its_gate() {
    let rows: &[(&str, &str, &str, Option<WirePort>, WirePort)] = &[
        (
            "`RoundStart()` composes through Union",
            "in x: exec\nlet t = Union(RoundStart(), x)\non t {\n  BroadcastChatMessage(\"hi\")\n}",
            "BrickComponentType_Event_Gamemode_RoundStart",
            None,
            WirePort::ExecOut,
        ),
        (
            "a top-level `let` of an event, with no enclosing exec chain",
            "let e = RoundStart()\non e {\n  BroadcastChatMessage(\"hi\")\n}",
            "BrickComponentType_Event_Gamemode_RoundStart",
            None,
            WirePort::ExecOut,
        ),
        (
            "`Clock(interval = iv)` wires its `interval` into the gate",
            "in iv: float\nlet c = Clock(interval = iv)\non c {\n  BroadcastChatMessage(\"tick\")\n}",
            "BrickComponentType_Clock",
            Some(WirePort::from_name("IntervalSeconds")),
            WirePort::from_name("Pulse"),
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, class, input, output) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let gates = nodes_of(&r.module, class);
        if gates.len() != 1 {
            failures.push(format!("{label}: {} x {class}, want one", gates.len()));
            continue;
        }
        let id = gates[0].id;
        if !r
            .module
            .wires
            .iter()
            .any(|w| w.source.node_id == id && w.source.port == *output)
        {
            failures.push(format!("{label}: nothing reads the event's {output:?}"));
        }
        if let Some(port) = input
            && !r
                .module
                .wires
                .iter()
                .any(|w| w.target.node_id == id && w.target.port == *port)
        {
            failures.push(format!("{label}: {port:?} is unwired"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// An event's data outputs take the types its handler declares, infers from
/// an in-unit sender, or (for a built-in event) its catalog gives; unused
/// custom-event slots default to float, never `any`, and the channel name
/// bakes into EventName.
#[test]
fn an_event_data_output_takes_its_declared_or_inferred_type() {
    let custom = crate::ir::gate_class::CUSTOM_EVENT;
    let rows: &[(&str, &str, &str, &[(&str, Type)], Option<&str>)] = &[
        (
            "the leading positional bakes EventName, config stays in the parens, and the typed `->` \
             params type DataOut",
            "static var last: int = 0\non CustomEvent(\"dmg\", isObject = true) -> (amount: int, source: character) {\n  last = amount\n}\n",
            custom,
            &[
                ("DataOut1", Type::Int),
                ("DataOut2", Type::Character),
                ("DataOut3", Type::Float),
                ("DataOut8", Type::Float),
            ],
            Some("dmg"),
        ),
        (
            "the same `-> (amount: int, source: character)` capture with no config",
            "static var last: int = 0\non CustomEvent(\"dmg\") -> (amount: int, source: character) {\n  last = amount\n}\n",
            custom,
            &[("DataOut1", Type::Int), ("DataOut2", Type::Character)],
            Some("dmg"),
        ),
        (
            "no annotations: the sender carries int + character",
            "static var last: int = 0\nin go: exec\non CustomEvent(\"dmg\") -> (amount, source) {\n  last = last + 1\n}\n\
             on CharacterSpawned() -> { character: ch } {\n  SendCustomEvent(\"dmg\", 42, ch)\n}\n",
            custom,
            &[
                ("DataOut1", Type::Int),
                ("DataOut2", Type::Character),
                ("DataOut3", Type::Float),
            ],
            None,
        ),
        (
            "an omitted tuple slot type is inferred through the `->` surface",
            "static var last: int = 0\nin go: exec\non CustomEvent(\"dmg\") -> (amount) {\n  last = last + 1\n}\non go {\n  SendCustomEvent(\"dmg\", 7)\n}\n",
            custom,
            &[("DataOut1", Type::Int)],
            None,
        ),
        (
            "an unannotated param on a NON-custom event keeps its catalog type; the custom-event float \
             fallback never leaks to a built-in event",
            "static var n: int = 0\non CharacterSpawned() -> { character: ch } {\n  n = n + 1\n}\n",
            "BrickComponentType_Event_Gamemode_CharacterSpawned",
            &[("Character", Type::Character)],
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, class, ports, name) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let node = &r.module.nodes[&find_gate(&r, class)];
        for (port, want) in *ports {
            let got = node
                .ports
                .outputs
                .iter()
                .find(|p| p.name == crate::intern::intern(port))
                .map(|p| &p.ty);
            if got != Some(want) {
                failures.push(format!("{label}: {port} is {got:?}, want {want:?}"));
            }
        }
        if let Some(name) = name
            && node.properties.get(&crate::intern::intern("EventName"))
                != Some(&Literal::String(name.to_string()))
        {
            failures.push(format!("{label}: EventName is not {name:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// An inline `mod` exposes no completion exec, so `on <mod call>(exec = ...)`
/// has nothing to trigger on: a clean WS043 error, with the handler body NOT
/// lowered. Each body's arithmetic differs from its mod's, so its gate class
/// is absent only if the body was dropped.
#[test]
fn an_on_trigger_on_an_inline_mod_call_is_ws043() {
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "a single-output mod with `-> (r)`: the desugared `_on_expr_N` is a `Binding::Local`, which \
             fell through to the value-changed-local branch, miswiring the value as an exec trigger \
             and orphaning `exec =`",
            "in s: exec\nmod doWork(x: int) -> (r: int) {\n  return x * 2\n}\nvar last: int = 0\nvar hits: int = 0\n\
             on doWork(5, exec = s) -> (r) {\n  hits = hits + 1\n  last = r\n}\n",
            &["BrickComponentType_Expr_MathAdd"],
        ),
        (
            "a multi-output mod: `_on_expr_N` is a `Binding::Record` with no `exec` field, and the whole \
             handler was dropped from the IR",
            "in s: exec\nvar log: string[]\nmod doWork(x: int) -> (a: int, b: int) {\n  log.push(\"ran\")\n  emit a = x\n  \
             emit b = x + 1\n}\nvar last: int = 0\nvar hits: int = 0\non doWork(5, exec = s) -> (a, b) {\n  hits = hits * 3\n  last = a\n}\n",
            &["BrickComponentType_Expr_MathMultiply"],
        ),
        (
            "`exec =` with no `-> pattern`: no handler exec chain at all, so no Var_Set from the dropped body",
            "in s: exec\nmod doWork(x: int) -> (r: int) {\n  return x * 2\n}\nvar hits: int = 0\non doWork(5, exec = s) {\n  hits = hits + 1\n}\n",
            &[
                "BrickComponentType_Expr_MathAdd",
                crate::ir::gate_class::VAR_SET,
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, absent) in rows {
        let r = compile(src);
        if !has_error(&r, "WS043") {
            failures.push(format!("{label}: no WS043 error in {:?}", r.diagnostics));
        }
        for class in *absent {
            if count_class(&r.module, class) != 0 {
                failures.push(format!("{label}: the handler body was lowered ({class})"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn display_text_font_ref_lands_as_data() {
    // `font` is a font asset ref (`$BrickFontDescriptor/…`) inlined into the
    // gate's `Font` (object) data field, reusing the asset-ref path.
    let src = "in go: exec\nin p: controller\non go {\n  p.DisplayText(\"hi\", font = $BrickFontDescriptor/Roboto)\n}\n";
    let r = compile_ok(src);
    let c = crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT;
    assert!(
        matches!(prop(&r, c, "Font"), Some(Literal::Asset { .. })),
        "Font asset ref should inline as data"
    );
}

#[test]
fn clock_event_config_lands_as_data() {
    // Clock is an event: the body chains from its Pulse; `interval`/`enabled`
    // are wire inputs (so they MAY be dynamic) but a CONSTANT value bakes into
    // the gate's scalar data default, with no carrier gate. A variable value would
    // wire.
    let src = "static var n: int = 0\non Clock(interval = 2.0, enabled = true) {\n  n = n + 1\n}\n";
    let r = compile_ok(src);
    let c = "BrickComponentType_Clock";
    assert!(has_gate(&r, c), "Clock gate placed");
    // Constant interval/enabled bake into their scalar wire-input data (no carrier).
    assert!(
        matches!(prop(&r, c, "IntervalSeconds"), Some(Literal::Float(f)) if (*f - 2.0).abs() < 1e-9)
    );
    assert!(matches!(prop(&r, c, "bEnabled"), Some(Literal::Bool(true))));
    assert!(
        !has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "a constant interval must bake, not materialize a carrier"
    );
}

#[test]
fn dynamic_clock_interval_stays_wired() {
    // Only a CONSTANT setting bakes; a variable `interval` must still wire into
    // IntervalSeconds so the clock rate can change at runtime.
    let src = "static var rate: float = 0.5\non Clock(interval = rate) {\n  rate = rate\n}\n";
    let r = compile_ok(src);
    let c = "BrickComponentType_Clock";
    // No baked interval, and a real wire feeds IntervalSeconds.
    assert!(prop(&r, c, "IntervalSeconds").is_none());
    let clock_id = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == c)
        .map(|(id, _)| *id)
        .unwrap();
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == clock_id && w.target.port.as_str() == "IntervalSeconds"),
        "a variable interval must stay wired"
    );
}

// ---- Advanced inventory: composite (array/struct) gate config -----------

/// `weapon` is wired into `ItemType`, so the round-trip test never bakes an
/// asset index the bare max schema (empty asset table) would reject on read.
const ADV_INVENTORY_SRC: &str = concat!(
    "in go: exec\nin c: character\nin weapon: entity\n",
    "on go {\n",
    "  c.AddInventoryItemAdv(weapon, overrideColors = true,\n",
    "    meshColors = [ColorSRGB(255, 0, 0, 255), ColorSRGB(0, 255, 0, 128)],\n",
    "    ammoOverride = { overrideStartingAmmo: true, resources: [{ loaded: 30, reserve: 90 }] })\n",
    "}\n",
);

#[test]
fn adv_inventory_composite_config_lands_as_data() {
    let r = compile_ok(ADV_INVENTORY_SRC);
    let c = crate::ir::gate_class::CHARACTER_ADD_INVENTORY_ITEM_ADV;
    // The plain bool config field still lands.
    assert!(matches!(
        prop(&r, c, "bOverrideColors"),
        Some(Literal::Bool(true))
    ));
    // meshColors folds to an array of sRGB Color literals (semantic RGBA u8).
    match prop(&r, c, "MeshColors") {
        Some(Literal::Array(cols)) => {
            assert_eq!(cols.len(), 2);
            assert!(matches!(
                cols[0],
                Literal::Color {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255
                }
            ));
            assert!(matches!(
                cols[1],
                Literal::Color {
                    r: 0,
                    g: 255,
                    b: 0,
                    a: 128
                }
            ));
        }
        other => panic!("MeshColors should be an Array of Color, got {other:?}"),
    }
    // ammoOverride folds to Array[Bool, Array[Array[Int, Int], ...]].
    match prop(&r, c, "WeaponAmmoOverride") {
        Some(Literal::Array(parts)) => {
            assert!(matches!(parts[0], Literal::Bool(true)));
            match &parts[1] {
                Literal::Array(res) => {
                    assert_eq!(res.len(), 1);
                    assert!(matches!(&res[0], Literal::Array(p)
                        if matches!(p.as_slice(), [Literal::Int(30), Literal::Int(90)])));
                }
                other => panic!("resources should be an Array, got {other:?}"),
            }
        }
        other => panic!("WeaponAmmoOverride should be an Array, got {other:?}"),
    }
}

#[test]
fn adv_inventory_composite_config_roundtrips_to_bytes() {
    // Distinct, asymmetric values so a mislabeled/reordered byte can't pass.
    let src = concat!(
        "in go: exec\nin c: character\nin weapon: entity\n",
        "on go {\n",
        "  c.AddInventoryItemAdv(weapon,\n",
        "    meshColors = [ColorSRGB(10, 20, 30, 40), ColorSRGB(200, 150, 100, 255)],\n",
        "    ammoOverride = { overrideStartingAmmo: true,\n",
        "      resources: [{ loaded: 7, reserve: 13 }, { loaded: 1, reserve: 2 }] })\n",
        "}\n",
    );
    let r = compile_ok(src);
    let c = crate::ir::gate_class::CHARACTER_ADD_INVENTORY_ITEM_ADV;
    let node = find_gate(&r, c);
    let val = crate::emit::roundtrip_adv_inventory_component(&r.module.nodes[&node]);
    use brdb::schema::BrdbValue;
    let BrdbValue::Struct(s) = &val else {
        panic!("expected a struct, got {val:?}")
    };

    // MeshColors serializes as Color[] in BGRA byte order, no gamma.
    let Some(BrdbValue::Array(colors)) = s.get("MeshColors") else {
        panic!("MeshColors not an array: {:?}", s.get("MeshColors"))
    };
    // MeshColors is a fixed-size array: the two supplied colours, then white
    // padding to exactly 8.
    assert_eq!(
        colors.len(),
        8,
        "MeshColors must be padded to the fixed slot count"
    );
    let bgra = |v: &BrdbValue| -> (u8, u8, u8, u8) {
        let BrdbValue::Struct(cs) = v else {
            panic!("color element not a struct")
        };
        let u8f = |name: &str| match cs.get(name) {
            Some(BrdbValue::U8(n)) => *n,
            other => panic!("{name} = {other:?}"),
        };
        (u8f("B"), u8f("G"), u8f("R"), u8f("A"))
    };
    // ColorSRGB(r, g, b, a) -> struct Color { B, G, R, A }.
    assert_eq!(bgra(&colors[0]), (30, 20, 10, 40));
    assert_eq!(bgra(&colors[1]), (100, 150, 200, 255));
    for c in &colors[2..] {
        assert_eq!(
            bgra(c),
            (255, 255, 255, 255),
            "unspecified slots are white-padded"
        );
    }

    // WeaponAmmoOverride: bOverrideStartingAmmo + Resources[] of Loaded/Reserve.
    let Some(BrdbValue::Struct(ammo)) = s.get("WeaponAmmoOverride") else {
        panic!("WeaponAmmoOverride not a struct")
    };
    assert!(matches!(
        ammo.get("bOverrideStartingAmmo"),
        Some(BrdbValue::Bool(true))
    ));
    // Resources is a fixed-size array the game rejects at any other length: the
    // two supplied entries, then zero-padding to exactly 8.
    let Some(BrdbValue::Array(res)) = ammo.get("Resources") else {
        panic!("Resources not an array")
    };
    assert_eq!(
        res.len(),
        8,
        "Resources must be padded to the fixed slot count"
    );
    let loaded_reserve = |v: &BrdbValue| -> (i32, i32) {
        let BrdbValue::Struct(rs) = v else {
            panic!("resource not a struct")
        };
        let i32f = |name: &str| match rs.get(name) {
            Some(BrdbValue::I32(n)) => *n,
            other => panic!("{name} = {other:?}"),
        };
        (i32f("Loaded"), i32f("Reserve"))
    };
    assert_eq!(loaded_reserve(&res[0]), (7, 13));
    assert_eq!(loaded_reserve(&res[1]), (1, 2));
    for r in &res[2..] {
        assert_eq!(
            loaded_reserve(r),
            (0, 0),
            "unspecified slots are zero-padded"
        );
    }
}

#[test]
fn adv_inventory_omitted_ammo_override_still_fills_fixed_resources() {
    // Regression: with NO `ammoOverride`, the WeaponAmmoOverride field must
    // still serialize a full fixed-length Resources array (the schema writer's
    // default would otherwise emit an empty one and the game load fails with
    // `FixedArraySizeInvalid`). Overriding stays off; every slot is zero.
    let src = concat!(
        "in go: exec\nin c: character\nin weapon: entity\n",
        "on go {\n",
        "  c.AddInventoryItemAdv(weapon, meshColors = [ColorSRGB(1, 2, 3, 4)])\n",
        "}\n",
    );
    let r = compile_ok(src);
    let c = crate::ir::gate_class::CHARACTER_ADD_INVENTORY_ITEM_ADV;
    let node = find_gate(&r, c);
    let val = crate::emit::roundtrip_adv_inventory_component(&r.module.nodes[&node]);
    use brdb::schema::BrdbValue;
    let BrdbValue::Struct(s) = &val else {
        panic!("expected a struct, got {val:?}")
    };
    let Some(BrdbValue::Struct(ammo)) = s.get("WeaponAmmoOverride") else {
        panic!("WeaponAmmoOverride missing when ammoOverride omitted")
    };
    assert!(matches!(
        ammo.get("bOverrideStartingAmmo"),
        Some(BrdbValue::Bool(false))
    ));
    let Some(BrdbValue::Array(res)) = ammo.get("Resources") else {
        panic!("Resources not an array")
    };
    assert_eq!(
        res.len(),
        8,
        "omitted ammoOverride must still fill all slots"
    );
}

#[test]
fn set_inventory_item_adv_mesh_colors_roundtrips() {
    // SetInventoryItemAdv shares the composite fields (plus a leading Slot).
    let src = concat!(
        "in go: exec\nin c: character\nin weapon: entity\n",
        "on go {\n",
        "  c.SetInventoryItemAdv(weapon, 2, meshColors = [ColorSRGB(1, 2, 3, 4)])\n",
        "}\n",
    );
    let r = compile_ok(src);
    let c = crate::ir::gate_class::CHARACTER_SET_INVENTORY_ITEM_ADV;
    let node = find_gate(&r, c);
    let val = crate::emit::roundtrip_adv_inventory_component(&r.module.nodes[&node]);
    use brdb::schema::BrdbValue;
    let BrdbValue::Struct(s) = &val else {
        panic!("expected a struct, got {val:?}")
    };
    let Some(BrdbValue::Array(colors)) = s.get("MeshColors") else {
        panic!("MeshColors not an array")
    };
    assert_eq!(
        colors.len(),
        8,
        "MeshColors must be padded to the fixed slot count"
    );
    let BrdbValue::Struct(cs) = &colors[0] else {
        panic!("color not a struct")
    };
    let u8f = |name: &str| match cs.get(name) {
        Some(BrdbValue::U8(n)) => *n,
        other => panic!("{name} = {other:?}"),
    };
    assert_eq!((u8f("B"), u8f("G"), u8f("R"), u8f("A")), (3, 2, 1, 4));
}

#[test]
fn non_constant_bool_config_is_rejected() {
    // A variable passed to a non-wire (settings-menu) bool config port is a
    // WS028 typecheck error, not a silent wire into a nonexistent pin.
    let src = "in a: quat\nin b: quat\nin t: float\nin useShortest: bool\nlet q = Slerp(a, b, t, shortestPath = useShortest)\n";
    assert!(
        errs(src).contains(&"WS028".to_string()),
        "non-constant bool config should be WS028: {:?}",
        errs(src)
    );
    // The lowering safety net drops it — no bogus property, and (since it never
    // reaches the wire path) no wire into the nonexistent config pin. The Slerp
    // gate keeps only its three real inputs (a, b, alpha).
    let r = compile(src);
    let node = find_gate(&r, crate::ir::gate_class::QUAT_SLERP);
    assert!(prop(&r, crate::ir::gate_class::QUAT_SLERP, "bShortestPath").is_none());
    assert_eq!(
        r.module
            .wires
            .iter()
            .filter(|w| w.target.node_id == node)
            .count(),
        3,
        "Slerp should have exactly its 3 real input wires (a, b, alpha)"
    );
}

#[test]
fn display_text_wireable_enum_bakes_constants_and_wires_runtime_values() {
    // `justify`/`easing` are wire inputs whose data field is a schema enum: a
    // member name still bakes into the field, and a runtime int wires in.
    let src = "in go: exec\nin p: controller\nin j: int\non go {\n  \
               p.DisplayText(\"hi\", justify = Right, easing = j)\n}\n";
    let r = compile_ok(src);
    let c = crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT;
    // EBRDisplayTextJustification::Right == 2.
    assert!(matches!(
        prop(&r, c, "Justification"),
        Some(Literal::Int(2))
    ));
    assert!(
        prop(&r, c, "Easing").is_none(),
        "a runtime easing must wire, not bake"
    );
    let gate = find_gate(&r, c);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == gate && w.target.port.as_str() == "Easing"),
        "the runtime easing value must wire into the Easing port"
    );
}

#[test]
fn event_expression_exposes_data_output() {
    // `CharacterSpawned().character` resolves the event's data output.
    let src = "in go: exec\nvar last: character\non go {\n  last = CharacterSpawned().character\n}";
    let r = compile_ok(src);
    let _ = find_gate(&r, "BrickComponentType_Event_Gamemode_CharacterSpawned");
}

#[test]
fn display_text_constant_axes_bake_vector2d() {
    // Constant per-axis layout bakes the parent Vector2D data field; an axis the
    // caller omits fills from STRUCT_DEFAULTS (Anchor.X default 0.5). No wire.
    let src = "in go: exec\nin p: controller\non go {\n  p.DisplayText(\"hi\", positionX = 0.0, positionY = -30.0, anchorY = 0.25)\n}\n";
    let r = compile_ok(src);
    let c = crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT;
    match prop(&r, c, "Position") {
        Some(Literal::Vector { x, y, .. }) => {
            assert_eq!(*x, 0.0);
            assert_eq!(*y, -30.0);
        }
        other => panic!("Position should bake as a Vector2D literal, got {other:?}"),
    }
    match prop(&r, c, "Anchor") {
        Some(Literal::Vector { x, y, .. }) => {
            assert_eq!(*x, 0.5, "unset Anchor.X fills from STRUCT_DEFAULTS");
            assert_eq!(*y, 0.25);
        }
        other => panic!("Anchor should bake with default X=0.5, got {other:?}"),
    }
}

#[test]
fn display_text_runtime_axis_wires_subport() {
    // A runtime float wires the Position.X sub-port (a valid wire input), and is
    // NOT baked into the Position data field.
    let src = "in go: exec\nin p: controller\nin px: float\non go {\n  p.DisplayText(\"hi\", positionX = px)\n}\n";
    let r = compile_ok(src);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.port == crate::ir::port_registry::WirePort::PositionX),
        "runtime positionX should wire Position.X; wires: {:?}",
        r.module.wires
    );
    assert!(
        prop(
            &r,
            crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT,
            "Position"
        )
        .is_none(),
        "a runtime axis must not bake a constant Position field"
    );
}

// ---- Custom events + Clock enabled-as-wire -------------------------------

#[test]
fn send_ports_carrying_refs_keep_the_ref_type() {
    // A ref value sent as event data must type its DataIn as the ref, not the
    // target's type: a numeric port variant turns a ref into the number 1.
    // A `*T` mod parameter sent as data is a place and still sends its value.
    let src = "var a: int = 0\nvar h: *int\nvar xs: int[]\nin go: exec\n\
               mod val(p: *int) { SendCustomEvent(\"v\", p) }\n\
               on go {\n  h = &a\n  SendCustomEvent(\"r\", &a, h, xs)\n  val(a)\n}\n";
    let r = compile_ok(src);
    let sends = super::refs::gates(&r, crate::ir::gate_class::SEND_CUSTOM_EVENT);
    let port_ty = |name: &str, event: &str| {
        let node = sends
            .iter()
            .copied()
            .find(|n| {
                matches!(r.module.nodes[n].properties.get(&crate::intern::intern("EventName")),
                    Some(Literal::String(s)) if s == event)
            })
            .expect("send gate");
        r.module.nodes[&node]
            .ports
            .inputs
            .iter()
            .find(|p| p.name == crate::intern::intern(name))
            .map(|p| p.ty.clone())
    };
    assert_eq!(
        port_ty("DataIn1", "r"),
        Some(Type::Ref(Box::new(Type::Int))),
        "&a"
    );
    assert_eq!(
        port_ty("DataIn2", "r"),
        Some(Type::Ref(Box::new(Type::Int))),
        "a var holding a ref"
    );
    assert_eq!(
        port_ty("DataIn3", "r"),
        Some(Type::Array(Box::new(Type::Int))),
        "an array"
    );
    assert_eq!(
        port_ty("DataIn1", "v"),
        Some(Type::Int),
        "a *T place sends its value"
    );
}

#[test]
fn set_custom_property_value_port_takes_the_wired_type() {
    let src = "var a: int = 0\nvar s: string = \"\"\nvar m: Map<string, int>\n\
               on ChatCommand(\"t\", \"t\") -> (controller, arguments) {\n\
                 controller.SetCustomProperty(\"i\", a)\n\
                 controller.SetCustomProperty(\"s\", s)\n\
                 controller.SetCustomProperty(\"m\", m)\n\
                 controller.SetCustomProperty(\"r\", &a)\n}\n";
    let r = compile_ok(src);
    let sets = super::refs::gates(&r, crate::ir::gate_class::ENTITY_SET_CUSTOM_PROPERTY);
    let value_ty = |key: &str| {
        let node = sets
            .iter()
            .copied()
            .find(|n| {
                matches!(r.module.nodes[n].properties.get(&crate::intern::intern("Key")),
                    Some(Literal::String(s)) if s == key)
            })
            .expect("set gate");
        r.module.nodes[&node]
            .ports
            .inputs
            .iter()
            .find(|p| p.name == crate::intern::intern("Value"))
            .map(|p| p.ty.clone())
    };
    assert_eq!(value_ty("i"), Some(Type::Int));
    assert_eq!(value_ty("s"), Some(Type::String));
    assert_eq!(
        value_ty("m"),
        Some(Type::Map(Box::new(Type::String), Box::new(Type::Int)))
    );
    assert_eq!(value_ty("r"), Some(Type::Ref(Box::new(Type::Int))));
}

#[test]
fn custom_event_object_config_and_send_target() {
    // The receive gate's `isObject` bakes bIsObjectEvent (scopes the event to
    // a grid/object), and the send gate's `target` (the entity whose grid
    // receives object events) wires into the Target port.
    let src = "in go: exec\n\
               in obj: entity\n\
               static var last: int = 0\n\
               on CustomEvent(\"dmg\", isObject = true) -> (amount: int) {\n  last = amount\n}\n\
               on go {\n  SendCustomEvent(\"dmg\", 7, target = obj)\n}\n";
    let r = compile_ok(src);
    let recv = crate::ir::gate_class::CUSTOM_EVENT;
    let send = crate::ir::gate_class::SEND_CUSTOM_EVENT;
    assert!(
        matches!(prop(&r, recv, "bIsObjectEvent"), Some(Literal::Bool(true))),
        "isObject = true should bake bIsObjectEvent, got {:?}",
        prop(&r, recv, "bIsObjectEvent")
    );
    // `target = obj` wires an entity into the send gate's Target input.
    let send_node = find_gate(&r, send);
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == send_node
            && w.target.port == crate::ir::port_registry::WirePort::Target),
        "target = obj should produce a wire into the send gate's Target port"
    );
}

#[test]
fn map_containers_lower_without_unsupported() {
    // Regression for the `Map<K,V>`-as-param/port/record-field silent
    // miscompile: a map used as a mod param, a record field, an `in`/`out`
    // port, and a `copyFrom` source must all lower to real MapVar gates wiring
    // `MapVarRef`, never `_Unsupported`. (Preferred fix: maps work wherever
    // arrays do.)
    let src = "var m: Map<int, int>\n\
               var m2: Map<int, int>\n\
               type Bag = { counts: Map<int, int> }\n\
               let bag: Bag = { counts: m2 }\n\
               out mapOut: Map<int, int> = m\n\
               in tick: exec\n\
               in inMap: Map<int, int>\n\
               mod useMap(x: Map<int, int>) { x.set(1, 2) }\n\
               mod useBag(b: Bag) { b.counts.set(3, 4) }\n\
               on tick {\n  useMap(m)\n  useBag(bag)\n  m2.copyFrom(inMap)\n}\n";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "no _Unsupported gate for map param / record field / in-out port / copyFrom source"
    );
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.port == crate::ir::port_registry::WirePort::MapVarRef),
        "map operations must wire MapVarRef (the way arrays wire ArrayVarRef)"
    );
}

#[test]
fn map_param_double_call_shares_one_map_gate() {
    // A mod taking a map param, called twice on the same file-scope map, wires
    // BOTH call sites to the SAME MapVar gate (mods inline per call site but the
    // container binding is captured by reference, not copied).
    let src = "var m: Map<int, int>\n\
               in go: exec\n\
               mod bump(mm: Map<int, int>, k: int) { mm.set(k, 5) }\n\
               on go {\n  bump(m, 1)\n  bump(m, 2)\n}\n";
    let r = compile_ok(src);
    let map_gates: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == crate::ir::gate_class::MAP_VAR)
        .collect();
    assert_eq!(map_gates.len(), 1, "one MapVar gate for the single `var m`");
    let map_id = *map_gates[0].0;
    let from_map = r
        .module
        .wires
        .iter()
        .filter(|w| {
            w.source.node_id == map_id
                && w.source.port == crate::ir::port_registry::WirePort::MapVarRef
        })
        .count();
    assert!(
        from_map >= 2,
        "both `bump` call sites must wire to the same MapVar's MapVarRef, got {from_map}"
    );
}

#[test]
fn exec_after_mod_ending_in_emit_signal_lowers() {
    // A mod whose body ends with an unbuffered `emit sig` is a FORK, not a
    // chain terminator — it fires the signal and the exec continues. When such
    // a mod is inlined mid-chain, exec statements AFTER the call must still
    // lower onto the chain, not fall to an `_Unsupported` placeholder. (Only a
    // BUFFERED `buffer emit loop` back-edge terminates the chain.)
    let src = "let sig: exec\n\
               in cube: entity\n\
               in go: exec\n\
               mod ping() { BroadcastChatMessage(\"x\") emit sig }\n\
               on go {\n  let t = cube.GetTag()\n  ping()\n  cube.SetTag(\"y\")\n}\n";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "no `_Unsupported` placeholder should be emitted for an exec statement \
         after a mod that ends in an emit fork"
    );
}

// ---- `on <Event> -> <pattern>` output capture ----------------------------

#[test]
fn on_arrow_record_named_event_binds_outputs() {
    // Subset capture: only `killer` is bound (the `character` slot is filled
    // with a synthesized unused name so the killer stays at its event slot),
    // and `on !killer` reads the event's Killer output.
    let src = "static var n: int = 0\non CharacterDied() -> { killer } {\n  on !killer {\n    n = n + 1\n  }\n}\n";
    let r = compile_ok(src);
    let died = find_gate(&r, "BrickComponentType_Event_Gamemode_CharacterDied");
    let not = find_gate(&r, crate::ir::gate_class::LOGICAL_NOT);
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == died
            && w.source.port == WirePort::from_name("Killer")
            && w.target.node_id == not),
        "`on !killer` must negate the event's Killer output: {:?}",
        r.module.wires
    );
}

#[test]
fn on_arrow_general_chip_call_exec_auto_extracted() {
    // `on <chipCall>(..., exec = trigger) -> (pattern)` is a GENERAL
    // (non-event) expr trigger. `on` auto-extracts the call's own completion
    // exec (a structural `Type::Exec` field on the call's result record) to
    // drive the body, and `-> (code)` binds the call's other (data) output.
    // Same underlying mechanism as the already-working
    // `let r = Init(...); on r.exec { }` (chip.rs's
    // `exec_arg_call_exposes_exec_field`) — reached here through the new
    // `on <call>(...) -> (...)` sugar instead of a manual `let` + `.exec`.
    let src = "var names: string[]\ntype Tables = { names: string[] }\nlet TB: Tables = { names }\nchip Init(tables: Tables) -> (code: int) {\n  tables.names.push(\"a\")\n  emit code = 5\n}\nin s: exec\nvar hit: int = 0\nvar last: int = 0\non Init(TB, exec = s) -> (code) {\n  hit = hit + 1\n  last = code\n}\n";
    let r = compile_ok(src);
    let child = only_chip(&r, "chip instance should produce a child module");
    // The handler body must be driven from the chip's `_exec_out` boundary
    // (the call's auto-extracted exec field), exactly like the manual
    // `on r.exec { }` form.
    let exec_out = *child
        .outputs
        .last()
        .expect("child should have the _exec_out boundary output");
    let exec_wired = r.module.wires.iter().any(|w| w.source.node_id == exec_out);
    assert!(
        exec_wired,
        "on Init(...) -> (code) should wire the body from the chip's _exec_out"
    );
    // `-> (code)` must bind the chip's `code` data output (declared first, so
    // it's `child.outputs[0]`) into the body (`last = code`), not the exec field.
    let code_out = child.outputs[0];
    let code_wired = r.module.wires.iter().any(|w| w.source.node_id == code_out);
    assert!(
        code_wired,
        "-> (code) should wire the chip's `code` output into the body"
    );
}

#[test]
fn send_custom_event_receiver_form_binds_target() {
    // `entity.SendCustomEvent("x", …)` is the receiver form: the object binds to
    // the send gate's `target` (the object-event recipient's grid), NOT to the
    // channel-name param — so the channel name + data still fill their normal
    // slots and the receiver becomes the Target wire.
    let src = "in go: exec\n\
               in obj: entity\n\
               on go {\n  obj.SendCustomEvent(\"dmg\", 7)\n}\n";
    let r = compile_ok(src);
    let send = crate::ir::gate_class::SEND_CUSTOM_EVENT;
    let send_node = find_gate(&r, send);
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == send_node
            && w.target.port == crate::ir::port_registry::WirePort::Target),
        "obj.SendCustomEvent should wire the receiver into the send gate's Target port"
    );
    // Positional 0 is still the channel name (baked into EventName), proving the
    // receiver did not consume the first positional slot.
    assert!(
        matches!(prop(&r, send, "EventName"), Some(Literal::String(s)) if s == "dmg"),
        "channel name should bake into EventName, got {:?}",
        prop(&r, send, "EventName")
    );
    // The global variant behaves identically against its own send gate.
    let gsrc = "in go: exec\n\
                in obj: entity\n\
                on go {\n  obj.SendGlobalCustomEvent(\"dmg\", 7)\n}\n";
    let gr = compile_ok(gsrc);
    let gsend = crate::ir::gate_class::SEND_CUSTOM_EVENT_GLOBAL;
    let gsend_node = find_gate(&gr, gsend);
    assert!(
        gr.module
            .wires
            .iter()
            .any(|w| w.target.node_id == gsend_node
                && w.target.port == crate::ir::port_registry::WirePort::Target),
        "obj.SendGlobalCustomEvent should wire the receiver into the Target port"
    );
}

#[test]
fn send_custom_event_exec_wires_to_execin_not_exec() {
    use crate::ir::port_registry::WirePort;
    // The Send[Global]CustomEvent gates name their exec INPUT `ExecIn`, not the
    // `Exec` that 157 other exec gates use. The exec chain must wire into ExecIn,
    // or the game rejects the connection at load ("port Exec does not exist in
    // target component"). Verify for both the personal and global variants.
    for (src, class) in [
        (
            "in go: exec\nin obj: entity\non go {\n  obj.SendCustomEvent(\"x\", 1)\n}\n",
            crate::ir::gate_class::SEND_CUSTOM_EVENT,
        ),
        (
            "in go: exec\non go {\n  SendGlobalCustomEvent(\"x\", 1)\n}\n",
            crate::ir::gate_class::SEND_CUSTOM_EVENT_GLOBAL,
        ),
    ] {
        let r = compile(src);
        let node = find_gate(&r, class);
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.target.node_id == node && w.target.port == WirePort::ExecIn),
            "exec chain must wire into {class}'s ExecIn port"
        );
        assert!(
            !r.module
                .wires
                .iter()
                .any(|w| w.target.node_id == node && w.target.port == WirePort::Exec),
            "must not wire into a phantom `Exec` port the real {class} component lacks"
        );
    }
}

#[test]
fn nested_on_negated_event_param_lowers_not_omitted() {
    // `on !p` where p is a CustomEvent data param must LOWER (it was silently
    // omitted before — EventParam wasn't in any trigger lookup): the negated
    // trigger emits a LogicalNOT gate driven by p's port.
    let src = "on CustomEvent(\"init\") -> (p: character) {\n  on !p {\n    BroadcastChatMessage(\"gone\")\n  }\n}\n";
    let r = compile(src);
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class == crate::ir::gate_class::LOGICAL_NOT),
        "on !p (negated event-param trigger) must emit a LogicalNOT gate, not be omitted"
    );
}

#[test]
fn var_as_trigger_typechecks_and_lowers() {
    // `on x` where x is a var fires on the var's value change; `on !x` negates.
    let ok = "static var x: bool = false\non x {\n  BroadcastChatMessage(\"changed\")\n}\n";
    let _ = compile_ok(ok);
    let neg = "static var x: bool = false\non !x {\n  BroadcastChatMessage(\"off\")\n}\n";
    let rn = compile_ok(neg);
    assert!(
        rn.module
            .nodes
            .values()
            .any(|n| n.gate_class == crate::ir::gate_class::LOGICAL_NOT),
        "on !x (negated var trigger) should emit a LogicalNOT gate"
    );
}

#[test]
fn spawn_explosion_at_wires_world_position() {
    // SpawnExplosionAt spawns at an absolute world position: `worldPosition`
    // (vector) wires into the WorldPosition input on the SpawnExplosionAt gate.
    let src = "in go: exec\nin pos: vector\nin who: entity\n\
               on go {\n  SpawnExplosionAt(pos, who, instigator = who)\n}\n";
    let r = compile_ok(src);
    let gate = crate::ir::gate_class::EXEC_SPAWN_EXPLOSION_AT;
    let node = find_gate(&r, gate);
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == node
            && w.target.port == crate::ir::port_registry::WirePort::WorldPosition),
        "worldPosition should wire into the SpawnExplosionAt gate's WorldPosition port"
    );
}

#[test]
fn global_custom_event_warns_and_keeps_separate_namespace() {
    let ws030 = |src: &str| {
        crate::typecheck::typecheck(
            &parse(src, "test").ast,
            "test",
            &crate::typecheck::CeSlotMap::default(),
        )
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "WS030")
        .count()
    };
    // A Global send whose data type disagrees with the `on GlobalCustomEvent`
    // receiver warns (WS030), exactly like the personal pair.
    let mismatch = "in go: exec\nstatic var n: int = 0\n\
                    on GlobalCustomEvent(\"g\") -> (amount: int) {\n  n = amount\n}\n\
                    on go {\n  SendGlobalCustomEvent(\"g\", 1.5)\n}\n";
    assert_eq!(
        ws030(mismatch),
        1,
        "global send/recv mismatch must warn WS030"
    );
    // Personal and Global are SEPARATE namespaces: the same channel name with
    // different types across the two kinds must NOT cross-warn.
    let separate = "in go: exec\nstatic var n: int = 0\nstatic var s: string = \"\"\n\
                    on CustomEvent(\"x\") -> (a: int) {\n  n = a\n}\n\
                    on GlobalCustomEvent(\"x\") -> (b: string) {\n  s = b\n}\n\
                    on go {\n  SendCustomEvent(\"x\", 5)\n  SendGlobalCustomEvent(\"x\", \"hi\")\n}\n";
    assert_eq!(
        ws030(separate),
        0,
        "personal/global channels of the same name must not cross-warn"
    );
}

#[test]
fn team_predicates_are_pure_bool() {
    // IsBuilderTeam / IsUnaffiliatedTeam are pure predicates over a Team entity,
    // yielding bool. Receiver form works too (`team.IsUnaffiliatedTeam()`).
    let src = "in t: entity\nlet a = IsBuilderTeam(t)\nlet b = t.IsUnaffiliatedTeam()\n\
               out x: bool = a\nout y: bool = b\n";
    let r = compile_ok(src);
    assert!(
        has_gate(&r, crate::ir::gate_class::GAMEMODE_IS_BUILDER_TEAM),
        "IsBuilderTeam gate"
    );
    assert!(
        has_gate(&r, crate::ir::gate_class::GAMEMODE_IS_UNAFFILIATED_TEAM),
        "IsUnaffiliatedTeam gate (receiver form)"
    );
    // A vector annotation rejects the bool result (bool has no vector coercion),
    // proving the return type is a real bool rather than `any`.
    let bad = crate::typecheck::typecheck(
        &parse("in t: entity\nout v: vector = IsBuilderTeam(t)\n", "test").ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    // IsBuilderTeam returns bool, which has no vector coercion.
    crate::test_support::assert_code(&bad.diagnostics, "WS003");
}

#[test]
fn team_join_leave_events_expose_team_and_playerstate() {
    // ControllerJoinedTeam/LeftTeam fire on team join/leave, binding the joining
    // player's PlayerState plus the Team entity, userId, userName.
    let src = "static var n: int = 0\n\
               on ControllerJoinedTeam() -> { controller: controller, team: team, userId: userId, userName: userName } {\n  n = n + 1\n}\n\
               on ControllerLeftTeam() -> { controller: controller, team: team, userId: userId, userName: userName } {\n  n = n - 1\n}\n";
    let r = compile_ok(src);
    for event in [
        "BrickComponentType_Event_Gamemode_ControllerJoinedTeam",
        "BrickComponentType_Event_Gamemode_ControllerLeftTeam",
    ] {
        let gate = find_gate(&r, event);
        let body = r
            .module
            .wires
            .iter()
            .find(|w| w.source.node_id == gate && w.source.port == WirePort::ExecOut)
            .map(|w| r.module.nodes[&w.target.node_id].gate_class);
        assert_eq!(
            body,
            Some(crate::ir::gate_class::VAR_INCREMENT),
            "{event} drives its handler's write"
        );
    }
}

#[test]
fn untyped_custom_event_param_infers_or_warns_ws042() {
    // A Custom Event receiver param without a type annotation is inferred from
    // an in-unit sender (no warning); with no sender to infer from, it warns
    // WS042 and defaults to float. WS029 (the old "always annotate" lint) is
    // gone — replaced by inference.
    let no_sender =
        "static var n: int = 0\non CustomEvent(\"dmg\") -> (amount) {\n  n = n + 1\n}\n";
    let parsed = crate::parser::parse(no_sender, "test");
    let (r, _map) = crate::typecheck::typecheck_with_inference(&parsed.ast, "test");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS042" && d.severity == crate::diagnostic::Severity::Warning),
        "untyped custom event param with no in-unit sender should warn WS042: {:?}",
        r.diagnostics
            .iter()
            .map(|d| d.code.to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS029"),
        "WS029 is removed: {:?}",
        r.diagnostics
    );

    // A matching in-unit sender infers the type instead — no warning at all.
    let with_sender = "in go: exec\nstatic var n: int = 0\non CustomEvent(\"dmg\") -> (amount) {\n  n = n + 1\n}\non go {\n  SendCustomEvent(\"dmg\", 42)\n}\n";
    let p = crate::parser::parse(with_sender, "test");
    let (ri, _map) = crate::typecheck::typecheck_with_inference(&p.ast, "test");
    assert!(
        !ri.diagnostics
            .iter()
            .any(|x| x.code == "WS029" || x.code == "WS042"),
        "inferred custom event param should not warn: {:?}",
        ri.diagnostics
            .iter()
            .map(|d| d.code.to_string())
            .collect::<Vec<_>>()
    );

    // The typed form does NOT warn, and neither do ordinary fixed-type events.
    for ok in [
        "static var n: int = 0\non CustomEvent(\"dmg\") -> (amount: int) {\n  n = n + 1\n}\n",
        "on CharacterSpawned() -> (character) {}\n",
    ] {
        let p = crate::parser::parse(ok, "test");
        let (d, _m) = crate::typecheck::typecheck_with_inference(&p.ast, "test");
        assert!(
            !d.diagnostics
                .iter()
                .any(|x| x.code == "WS029" || x.code == "WS042"),
            "should not warn: {ok}"
        );
    }
}

#[test]
fn send_custom_event_type_mismatch_lints_ws030() {
    // A `SendCustomEvent` data value whose wire type disagrees with the
    // `on CustomEvent` receiver's declared param type warns (WS030).
    let src = "in go: exec\nvar last: int = 0\non CustomEvent(\"dmg\") -> (amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", 1.5)\n}\n";
    let parsed = crate::parser::parse(src, "test");
    let diags =
        crate::typecheck::typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default())
            .diagnostics;
    assert!(
        diags
            .iter()
            .any(|d| d.code == "WS030" && d.severity == crate::diagnostic::Severity::Warning),
        "float sent to int custom-event param should warn WS030: {:?}",
        diags
            .iter()
            .map(|d| (d.code.to_string(), d.message.clone()))
            .collect::<Vec<_>>()
    );

    for ok in [
        // Exact type match — silent.
        "in go: exec\nvar last: int = 0\non CustomEvent(\"dmg\") -> (amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", 7)\n}\n",
        // character vs entity: both the Object wire variant, not a real mismatch.
        "on CharacterSpawned() -> (character) {\n  SendCustomEvent(\"hit\", character)\n}\non CustomEvent(\"hit\") -> (who: entity) {}\n",
        // Dynamic (non-literal) channel name — can reach any receiver, never linted.
        "in go: exec\nvar chan: string = \"dmg\"\non CustomEvent(\"dmg\") -> (amount: int) {}\non go {\n  SendCustomEvent(chan, 1.5)\n}\n",
        // No matching receiver in this unit — nothing to compare against.
        "in go: exec\non go {\n  SendCustomEvent(\"orphan\", 1.5)\n}\n",
    ] {
        let p = crate::parser::parse(ok, "test");
        let d =
            crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
                .diagnostics;
        assert!(
            !d.iter().any(|x| x.code == "WS030"),
            "should not warn WS030: {ok}\n{:?}",
            d.iter()
                .map(|x| (x.code.to_string(), x.message.clone()))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn ws030_sees_send_inside_captured_event() {
    // A SendCustomEvent inside a captured event body (`let h = on go { … }`)
    // must be seen by the WS030 sender/receiver type check.
    let src = "on CustomEvent(\"hp\") -> (amount: float) { let x = amount }\n\
               let h = on go { SendCustomEvent(\"hp\", 5) }\n";
    let d = crate::typecheck::typecheck(
        &crate::parser::parse(src, "t").ast,
        "t",
        &crate::typecheck::CeSlotMap::default(),
    )
    .diagnostics;
    assert!(
        d.iter().any(|x| x.code == "WS030"),
        "WS030 must see a send inside a captured event body: {:?}",
        d.iter().map(|x| x.code.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn any_annotation_warns_but_storage_and_probe_do_not() {
    let diags = |s: &str| {
        crate::typecheck::typecheck(
            &crate::parser::parse(s, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        )
        .diagnostics
    };
    let has = |s: &str, code: &str, sev: crate::diagnostic::Severity| {
        diags(s).iter().any(|x| x.code == code && x.severity == sev)
    };
    use crate::diagnostic::Severity;
    // param + output annotations `any` -> warn
    assert!(
        has(
            "mod f(a: any) -> any { return a }\n",
            "WS032",
            Severity::Warning
        ),
        "any param/output should warn"
    );
    // in-port `any` -> no warn: a port cannot take a type parameter
    assert!(
        !diags("in x: any\n").iter().any(|z| z.code == "WS032"),
        "any in-port must not warn"
    );
    // Opaque(...) probe VALUE is not an annotation -> no warn
    assert!(
        !diags("in x: int\nlet y = Opaque(x)\n")
            .iter()
            .any(|z| z.code == "WS032"),
        "Opaque() value must not warn"
    );
    // storage `any` holds a WireValue: neither an error nor a warning
    assert!(
        !diags("var v: any = 0\n")
            .iter()
            .any(|z| z.code == "WS025" || z.code == "WS032"),
        "storage any is allowed and does not warn"
    );
}

#[test]
fn any_annotation_warns_on_compound_and_stmt_out() {
    let diags = |s: &str| {
        crate::typecheck::typecheck(
            &crate::parser::parse(s, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        )
        .diagnostics
    };
    let has = |s: &str, code: &str, sev: crate::diagnostic::Severity| {
        diags(s).iter().any(|x| x.code == code && x.severity == sev)
    };
    let count = |s: &str, code: &str| diags(s).iter().filter(|x| x.code == code).count();
    use crate::diagnostic::Severity;
    // `*any` (Ref-wrapped) ref param -> warn (shallow matches! would miss it)
    assert!(
        has("mod inc(v: *any) { v = v }\n", "WS032", Severity::Warning),
        "*any ref param should warn"
    );
    // `any[]` (Array-wrapped) param -> warn
    assert!(
        has(
            "mod f(a: any[]) -> int { return 0 }\n",
            "WS032",
            Severity::Warning
        ),
        "any[] param should warn"
    );
    // anon-chip statement-level `out` annotation `any` -> no warn: it is a
    // port, which cannot take a type parameter (Stmt::OutBinding path)
    assert_eq!(
        count("in go: exec\nchip { @bottom out done: any = 5 }\n", "WS032"),
        0,
        "anon-chip statement-level out any must not warn"
    );
    // fires once per annotation (at the top-level range), not once per nested Opaque
    assert_eq!(
        count("mod inc(v: *any) { v = v }\n", "WS032"),
        1,
        "one warn for *any param"
    );
    assert_eq!(
        count("mod f(a: any[]) -> int { return 0 }\n", "WS032"),
        1,
        "one warn for any[] param"
    );
}

/// A constant given to a gate setting the gate keeps in its own data: what
/// the program says about it, and the value the save holds.
enum Setting {
    /// Stored as written, with nothing to say.
    Exact(Literal),
    /// Stored as this other value, with a `WS106` naming the setting.
    Inexact(Literal),
    /// Not storable: `WS028`.
    Invalid,
}

const SETTING_HEAD: &str = "in go: exec\nin c: controller\nin s: string\nvar n: int = 0\nvar b: bool = false\n\
                            out on_: int = n\nout ob: bool = b\n";

fn setting_rows() -> Vec<(&'static str, &'static str, &'static str, Setting)> {
    use Literal::{Bool, Float, Int};
    use Setting::{Exact, Inexact, Invalid};
    vec![
        (
            "SleepTicks 2.0",
            "on go { await SleepTicks(_, delay = 2.0)\n n = 1 }",
            "TicksToWait",
            Exact(Int(2)),
        ),
        (
            "SleepTicks 2.7",
            "on go { await SleepTicks(_, delay = 2.7)\n n = 1 }",
            "TicksToWait",
            Inexact(Int(2)),
        ),
        (
            "SleepTicks 1.999",
            "on go { await SleepTicks(_, delay = 1.999)\n n = 1 }",
            "TicksToWait",
            Inexact(Int(1)),
        ),
        (
            "SleepTicks -1.5",
            "on go { await SleepTicks(_, delay = -1.5)\n n = 1 }",
            "TicksToWait",
            Inexact(Int(-1)),
        ),
        (
            "SleepTicks true",
            "on go { await SleepTicks(_, delay = true)\n n = 1 }",
            "TicksToWait",
            Inexact(Int(1)),
        ),
        (
            "SleepTicks 1e30",
            "on go { await SleepTicks(_, delay = 1e30)\n n = 1 }",
            "TicksToWait",
            Invalid,
        ),
        (
            "SleepTicks const 2.7",
            "const D: float = 2.7\non go { await SleepTicks(_, delay = D)\n n = 1 }",
            "TicksToWait",
            Inexact(Int(2)),
        ),
        (
            "Sleep 2",
            "on go { await Sleep(_, delay = 2)\n n = 1 }",
            "SecondsToWait",
            Exact(Float(2.0)),
        ),
        (
            "Sleep false",
            "on go { await Sleep(_, delay = false)\n n = 1 }",
            "SecondsToWait",
            Inexact(Float(0.0)),
        ),
        (
            "fontSize 40",
            "on go { c.DisplayText(\"hi\", fontSize = 40) }",
            "FontSize",
            Exact(Int(40)),
        ),
        (
            "fontSize 40.5",
            "on go { c.DisplayText(\"hi\", fontSize = 40.5) }",
            "FontSize",
            Inexact(Int(40)),
        ),
        (
            "fontSize 0.5",
            "on go { c.DisplayText(\"hi\", fontSize = 0.5) }",
            "FontSize",
            Inexact(Int(0)),
        ),
        (
            "fontSize true",
            "on go { c.DisplayText(\"hi\", fontSize = true) }",
            "FontSize",
            Inexact(Int(1)),
        ),
        (
            "fontSize 1e10",
            "on go { c.DisplayText(\"hi\", fontSize = 1e10) }",
            "FontSize",
            Invalid,
        ),
        (
            "fontSize 5000000000",
            "on go { c.DisplayText(\"hi\", fontSize = 5000000000) }",
            "FontSize",
            Invalid,
        ),
        (
            "outlineSize 3.5",
            "on go { c.DisplayText(\"hi\", outlineSize = 3.5) }",
            "OutlineSize",
            Inexact(Int(3)),
        ),
        (
            "angle true",
            "on go { c.DisplayText(\"hi\", angle = true) }",
            "Angle",
            Inexact(Float(1.0)),
        ),
        (
            "lifetime true",
            "on go { c.DisplayText(\"hi\", lifetime = true) }",
            "Lifetime",
            Inexact(Float(1.0)),
        ),
        (
            "justify 2.0",
            "on go { c.DisplayText(\"hi\", justify = 2.0) }",
            "Justification",
            Exact(Int(2)),
        ),
        (
            "justify 2.7",
            "on go { c.DisplayText(\"hi\", justify = 2.7) }",
            "Justification",
            Inexact(Int(2)),
        ),
        (
            "justify -0.5",
            "on go { c.DisplayText(\"hi\", justify = -0.5) }",
            "Justification",
            Inexact(Int(0)),
        ),
        (
            "justify false",
            "on go { c.DisplayText(\"hi\", justify = false) }",
            "Justification",
            Inexact(Int(0)),
        ),
        (
            "justify 3.0",
            "on go { c.DisplayText(\"hi\", justify = 3.0) }",
            "Justification",
            Invalid,
        ),
        (
            "justify 3.5",
            "on go { c.DisplayText(\"hi\", justify = 3.5) }",
            "Justification",
            Invalid,
        ),
        (
            "justify 7.0",
            "on go { c.DisplayText(\"hi\", justify = 7.0) }",
            "Justification",
            Invalid,
        ),
        (
            "justify 1e30",
            "on go { c.DisplayText(\"hi\", justify = 1e30) }",
            "Justification",
            Invalid,
        ),
        (
            "justify const 7.0",
            "const J = 7.0\non go { c.DisplayText(\"hi\", justify = J) }",
            "Justification",
            Invalid,
        ),
        (
            "caseSensitive 0",
            "on go { b = s.Contains(\"b\", caseSensitive = 0) }",
            "bCaseSensitive",
            Exact(Bool(false)),
        ),
        (
            "caseSensitive 0.0",
            "on go { b = s.Contains(\"b\", caseSensitive = 0.0) }",
            "bCaseSensitive",
            Exact(Bool(false)),
        ),
        (
            "miteredOutline 1",
            "on go { c.DisplayText(\"hi\", miteredOutline = 1) }",
            "bMiteredOutline",
            Exact(Bool(true)),
        ),
        (
            "miteredOutline 1.5",
            "on go { c.DisplayText(\"hi\", miteredOutline = 1.5) }",
            "bMiteredOutline",
            Inexact(Bool(true)),
        ),
        (
            "miteredOutline 0.2",
            "on go { c.DisplayText(\"hi\", miteredOutline = 0.2) }",
            "bMiteredOutline",
            Inexact(Bool(true)),
        ),
        (
            "miteredOutline 2",
            "on go { c.DisplayText(\"hi\", miteredOutline = 2) }",
            "bMiteredOutline",
            Inexact(Bool(true)),
        ),
        (
            "Edge true",
            "out o: bool = Edge(true)",
            "Input",
            Exact(Float(1.0)),
        ),
        (
            "Edge 2.5",
            "out o: bool = Edge(2.5)",
            "Input",
            Exact(Float(2.5)),
        ),
    ]
}

/// What each setting field the rows read holds when its setting is not
/// given, so a row whose value is that default could not tell a dropped
/// setting from a stored one.
const SETTING_DEFAULTS: &[(&str, Literal)] = &[
    ("TicksToWait", Literal::Int(0)),
    ("SecondsToWait", Literal::Float(1.0)),
    ("FontSize", Literal::Int(16)),
    ("OutlineSize", Literal::Int(2)),
    ("Angle", Literal::Float(0.0)),
    ("Lifetime", Literal::Float(5.0)),
    ("Justification", Literal::Int(1)),
    ("bCaseSensitive", Literal::Bool(true)),
    ("bMiteredOutline", Literal::Bool(false)),
    ("Input", Literal::Float(0.0)),
];

#[test]
fn a_constant_gate_setting_is_stored_exactly_or_reported() {
    let mut failures = Vec::new();
    let rows = setting_rows();
    for (row, body, field, want) in &rows {
        let src = format!("{SETTING_HEAD}{body}\n");
        if let Setting::Exact(stored) | Setting::Inexact(stored) = want {
            let default = SETTING_DEFAULTS
                .iter()
                .find(|(f, _)| f == field)
                .map(|(_, d)| d);
            assert!(
                default.is_some_and(|d| d != stored),
                "{row}: {stored:?} is the field's default"
            );
        }
        for fold in [true, false] {
            match (want, super::declaring_scope::compiled(&src, &[], fold)) {
                (Setting::Invalid, Err(codes)) if codes.contains(&"WS028".to_string()) => {}
                (Setting::Invalid, other) => failures.push(format!(
                    "{row} fold {fold}: not WS028: {:?}",
                    other.map(|c| c.out.diagnostics.len())
                )),
                (Setting::Exact(_) | Setting::Inexact(_), Err(codes)) => {
                    failures.push(format!("{row} fold {fold}: does not compile: {codes:?}"))
                }
                (Setting::Exact(stored) | Setting::Inexact(stored), Ok(c)) => {
                    let ws106: Vec<&String> = c
                        .out
                        .diagnostics
                        .iter()
                        .filter(|d| d.code == "WS106")
                        .map(|d| &d.message)
                        .collect();
                    match want {
                        Setting::Exact(_) if !ws106.is_empty() => {
                            failures.push(format!("{row} fold {fold}: exact, but {ws106:?}"))
                        }
                        Setting::Inexact(_) if ws106.len() != 1 => failures
                            .push(format!("{row} fold {fold}: want one WS106, got {ws106:?}")),
                        _ => {}
                    }
                    let held = super::declaring_scope::save_field(&src, &[], fold, field).unwrap();
                    if held.is_empty() || held.iter().any(|v| v != stored) {
                        failures.push(format!(
                            "{row} fold {fold}: {field} holds {held:?}, not {stored:?}"
                        ));
                    }
                }
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A setting fed by a wire is the wire's, and nothing is said about it.
#[test]
fn a_wired_gate_setting_is_not_reported() {
    for body in [
        "in kf: float\non go { await SleepTicks(_, delay = kf)\n n = 1 }",
        "in kf: float\non go { c.DisplayText(\"hi\", fontSize = kf) }",
        "in kf: float\non go { b = s.Contains(\"b\", caseSensitive = kf) }",
    ] {
        let src = format!("{SETTING_HEAD}{body}\n");
        let w = super::declaring_scope::warnings(&src, &[]);
        assert!(!w.iter().any(|(c, _)| c == "WS106"), "{w:?}\n{src}");
    }
}

/// A setting's diagnostic quotes the value as it is usually written, names
/// the setting on every path, and calls an operand of a pure expression an
/// argument.
#[test]
fn a_setting_diagnostic_names_the_setting_and_the_value_as_written() {
    let message = |body: &str, code: &str| {
        let src = format!("{SETTING_HEAD}{body}\n");
        let diagnostics = match crate::compile(crate::CompileInput {
            source: &src,
            file: "main",
            module_name: None,
            fold_mode: crate::FoldMode::Auto,
        }) {
            Ok(out) => out.diagnostics,
            Err(crate::CompileError::HasErrors(errors)) => errors,
            Err(e) => panic!("{e:?}"),
        };
        diagnostics
            .into_iter()
            .find(|d| d.code == code)
            .map(|d| d.message)
            .unwrap_or_else(|| panic!("no {code}: {body}"))
    };
    let m = message("on go { c.DisplayText(\"hi\", fontSize = 1e10) }", "WS028");
    assert!(
        m.contains("cannot be 1e10") && !m.contains("10000000000"),
        "{m}"
    );
    let m = message("on go { c.DisplayText(\"hi\", justify = 3) }", "WS028");
    assert!(
        m.contains("`DisplayText` setting `justify` cannot be 3"),
        "{m}"
    );
    let m = message("on go { c.DisplayText(\"hi\", justify = 3.0) }", "WS028");
    assert!(m.contains("`DisplayText` setting `justify`"), "{m}");
    let m = message(
        "var y: int = 0\nout oy: int = y\non go { y = 1e30 }",
        "WS028",
    );
    assert!(m.starts_with("1e30 goes into an `int` here"), "{m}");
    let m = message("on go { n = BitCount(2.5) }", "WS106");
    assert!(m.contains("argument") && !m.contains("setting"), "{m}");
    let m = message("on go { c.DisplayText(\"hi\", fontSize = 40.5) }", "WS106");
    assert!(m.contains("`DisplayText` setting `fontSize`"), "{m}");
}
