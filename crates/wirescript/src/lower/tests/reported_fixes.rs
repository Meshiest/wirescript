//! Regressions for a batch of reported miscompiles: type-preserving math
//! builtins, constant compound assignment, `null` comparisons on object
//! types, a `_` arm after a full `match`, event aliases, and exec-chain
//! statements that never run.

/// The program's `.wsir` text, as `compile --dump-ir` prints it, and every
/// diagnostic as `CODE message`. Panics when `src` has errors.
fn ir(src: &str) -> (String, Vec<String>) {
    let input = crate::CompileInput { source: src, file: "t.ws", module_name: None, fold_mode: crate::FoldMode::Auto };
    let result = match crate::compile_to_world(input, Default::default()) {
        Ok(r) => r,
        Err(crate::CompileError::HasErrors(ds)) => {
            panic!("{:?}\n{src}", ds.iter().map(|d| format!("{} {}", d.code, d.message)).collect::<Vec<_>>())
        }
        Err(e) => panic!("{e:?}\n{src}"),
    };
    let diags = result.diagnostics.iter().map(|d| format!("{} {}", d.code, d.message)).collect();
    let mut program = result.program;
    crate::prepare_program_for_text(&mut program, "t.ws", std::path::Path::new("t.wsir"), crate::FoldMode::Auto)
        .expect("prepare");
    (crate::wsir::program_to_string(&program).expect("write"), diags)
}

/// The `.wsir` line of the node built for `note` (a builtin's name), or of
/// the output `%name`.
fn line<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
}

#[test]
fn integer_min_max_fold_to_an_integer() {
    let (text, _) = ir("let A = max(3, 7)\n@right out o = A\nlet B = min(3, 7)\n@right out p = B\n\
                        let C = abs(-4)\n@right out q = C\nlet D = clamp(12, 0, 10)\n@right out r = D\n");
    for out in ["%o", "%p", "%q", "%r"] {
        let decl = line(&text, &format!("out {out} "));
        assert!(decl.contains(&format!("out {out} int")), "{decl}");
    }
    assert!(!text.contains(".0,") && !text.contains(".0 }"), "an integer fold baked a float:\n{text}");
    for value in ["InputA = 7,", "InputA = 3,", "InputA = 4,", "InputA = 10,"] {
        assert!(text.contains(value), "missing {value}:\n{text}");
    }
}

#[test]
fn runtime_math_gates_carry_the_argument_type() {
    let (text, _) = ir("@left in a: int\n@left in b: int\n@left in t: bool\n@left in f: float\n\
                        @left in v: vector\n@left in w: vector\n@left in c: color\n\
                        @right out m = min(a, b)\n@right out x = max(a, b)\n@right out ab = abs(a)\n\
                        @right out cl = clamp(a, 0, 10)\n@right out bt = abs(t)\n@right out mf = max(a, f)\n\
                        @right out mv = min(v, w)\n@right out ac = abs(c)\n");
    for (out, ty) in [
        ("%m", "int"),
        ("%x", "int"),
        ("%ab", "int"),
        ("%cl", "int"),
        ("%bt", "int"),
        ("%mf", "float"),
        ("%mv", "vector"),
        ("%ac", "color"),
    ] {
        let decl = line(&text, &format!("out {out} "));
        assert!(decl.contains(&format!("out {out} {ty} ")), "{decl}");
    }
    for (note, ty) in [("\"min\"", "int"), ("\"clamp\"", "int")] {
        let gate = text.lines().find(|l| l.contains(note) && l.contains("%a")).expect(note);
        assert!(gate.contains(&format!("-> (Output: {ty})")), "{gate}");
    }
    let mixed = text.lines().find(|l| l.contains("Expr_MathMax") && l.contains("%f")).expect("max(a, f)");
    assert!(!mixed.contains("Output: int"), "{mixed}");
    let vector = text.lines().find(|l| l.contains("Expr_MathMin") && l.contains("%v")).expect("min(v, w)");
    assert!(vector.contains("-> (Output: vector)"), "{vector}");
}

#[test]
fn a_float_argument_makes_the_integer_constants_floats() {
    let (text, _) = ir("@left in f: float\n@right out c = clamp(f, 0, 10)\n");
    let gate = line(&text, "Expr_MathClamp");
    assert!(gate.contains("Min: 0.0") && gate.contains("Max: 10.0"), "{gate}");
    assert!(gate.contains("-> (Output: float)"), "{gate}");
}

#[test]
fn a_named_integer_constant_steps_an_int_var_by_an_integer() {
    let (text, _) = ir("const STEP = 50\nvar f: int = 0\n@left in x: exec\non x { f -= STEP }\n\
                        @left in y: exec\non y { f -= -STEP }\n");
    let steps: Vec<&str> = text.lines().filter(|l| l.contains("Exec_Var_Increment")).collect();
    assert_eq!(steps.len(), 2, "{text}");
    assert!(steps.iter().any(|l| l.contains("Value: -50)")), "{text}");
    assert!(steps.iter().any(|l| l.contains("Value: 50)")), "{text}");
    assert!(!text.contains("50.0"), "{text}");
}

#[test]
fn an_object_compares_with_null() {
    let (text, diags) = ir("var it: character = null\n@left in go: exec\n@left in e: entity\n@left in p: controller\n\
                            on go {\n  if it != null { BroadcastChatMessage(\"set\") }\n\
                              if null == it { BroadcastChatMessage(\"unset\") }\n}\n\
                            @right out noE = e == null\n@right out someP = p != null\n");
    assert!(diags.iter().all(|d| !d.starts_with("WS")), "{diags:?}");
    for (gate, operand) in [
        ("Expr_CompareNotEqual", "InputB: character"),
        ("Expr_CompareEqual", "InputA: character"),
        ("Expr_CompareEqual", "%e"),
        ("Expr_CompareNotEqual", "%p"),
    ] {
        assert!(
            text.lines().any(|l| l.contains(gate) && l.contains(operand) && l.contains("null")),
            "{gate} on {operand}:\n{text}"
        );
    }
    assert!(!text.contains("_Unsupported"), "{text}");
}

#[test]
fn a_brick_or_ref_compares_with_null() {
    let (text, diags) = ir("@left in b: brick\n@right out none = b == null\nvar a: int = 0\nvar h: *int\n\
                            @left in go: exec\non go { if h == null { h = &a } }\n");
    assert!(diags.iter().all(|d| !d.starts_with("WS")), "{diags:?}");
    assert!(text.lines().any(|l| l.contains("Expr_CompareEqual") && l.contains("%b")), "{text}");
    assert!(text.lines().any(|l| l.contains("Expr_CompareEqual") && l.contains("brick")), "{text}");
    assert!(!text.contains("_Unsupported"), "{text}");
}

#[test]
fn an_annotated_null_let_or_const_is_its_type() {
    let r = crate::typecheck::typecheck(
        &crate::parser::parse("let n: character = null\nconst N: entity = null\n@right out o = n\n@right out q = N\n", "t").ast,
        "t",
        &crate::typecheck::CeSlotMap::default(),
    );
    let errors: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == crate::diagnostic::Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let (text, _) = ir("let n: character = null\nconst N: entity = null\n@right out o = n\n@right out q = N\n");
    assert!(line(&text, "%o = ").contains("(RER_Input: character)"), "{text}");
    assert!(line(&text, "%q = ").contains("(RER_Input: entity)"), "{text}");
}

#[test]
fn a_wildcard_after_every_variant_catches_a_stray_tag() {
    let src = "enum Mode { Off, Dim, Bright }\n@left in presses: int\n\
               @right out label = match Mode.FromInt(presses) { Off => \"Off\", Dim => \"Dim\", Bright => \"Bright\", _ => \"Unknown\" }\n";
    let (text, diags) = ir(src);
    assert!(diags.iter().all(|d| !d.starts_with("WS061")), "{diags:?}");
    assert!(text.lines().any(|l| l.contains("Expr_Select") && l.contains("InputA: \"Unknown\"")), "{text}");
    let (_, diags) = ir("enum Mode { Off, Dim }\n@left in presses: int\n\
                         @right out label = match Mode.FromInt(presses) { Off => 1, Dim => 2, _ => 3, _ => 4 }\n");
    assert_eq!(diags.iter().filter(|d| d.starts_with("WS061")).count(), 1, "{diags:?}");
}

#[test]
fn a_wildcard_after_every_variant_runs_for_a_stray_tag_in_a_handler() {
    let src = "enum Mode { Off, Dim }\n@left in presses: int\n@left in go: exec\nvar n: int = 0\n\
               on go {\n  match Mode.FromInt(presses) {\n    Off => { n = 1 }\n    Dim => { n = 2 }\n    _ => { n = 3 }\n  }\n}\n";
    let (text, diags) = ir(src);
    assert!(diags.iter().all(|d| !d.starts_with("WS061")), "{diags:?}");
    assert!(text.lines().any(|l| l.contains("Exec_Var_Set") && l.contains("Value: 3)")), "{text}");
}

/// `let x = on <trigger>` names the trigger: every `on x` handler compiles as
/// if it wrote the trigger itself, config args and bound outputs included.
#[test]
fn an_event_alias_is_its_trigger_in_every_handler() {
    use super::declaring_scope::save_dump;
    let head = "@left in front: exec\n@left in back: exec\n@left in room: zone\nvar n: int = 0\n";
    let rows = [
        ("pin", "let rang = on front\non rang { n += 1 }\non rang { n += 10 }\n", "on front { n += 1 }\non front { n += 10 }\n"),
        (
            "event with config",
            "let tick = on Clock(interval = 1.0)\non tick { n += 1 }\non tick { n += 2 }\n",
            "on Clock(interval = 1.0) { n += 1 }\non Clock(interval = 1.0) { n += 2 }\n",
        ),
        (
            "bound outputs",
            "let died = on CharacterDied()\non died -> (character) { if character != null { n += 3 } }\n",
            "on CharacterDied() -> (character) { if character != null { n += 3 } }\n",
        ),
        (
            "config and bound outputs",
            "let arrived = on ZoneEntered(zone = room)\non arrived -> (c) { if c != null { n += 3 } }\non arrived { n += 1 }\n",
            "on ZoneEntered(zone = room) -> (c) { if c != null { n += 3 } }\non ZoneEntered(zone = room) { n += 1 }\n",
        ),
        ("union", "let rang = on Union(front, back)\non rang { n += 1 }\n", "on Union(front, back) { n += 1 }\n"),
    ];
    let mut failures = Vec::new();
    for (what, alias, direct) in rows {
        let (a, b) = (format!("{head}{alias}"), format!("{head}{direct}"));
        match (save_dump(&a, &[], true), save_dump(&b, &[], true)) {
            (Ok(x), Ok(y)) if x == y => {}
            other => failures.push(format!("{what}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A captured event on an expression trigger runs its body, and an `on` of
/// it runs after.
#[test]
fn a_captured_event_on_an_expression_runs_its_body() {
    let (text, diags) = ir("@left in front: exec\n@left in back: exec\nvar n: int = 0\n\
                            let x = on Union(front, back) { n += 1 }\non x { n += 2 }\n");
    assert!(diags.iter().all(|d| !d.starts_with("WS")), "{diags:?}");
    let steps: Vec<&str> = text.lines().filter(|l| l.contains("Exec_Var_Increment")).collect();
    assert_eq!(steps.len(), 2, "{text}");
}

#[test]
fn a_statement_after_an_if_whose_branches_all_buffer_emit_is_reported() {
    let src = "@left in go: bool\nvar n: int = 0\nvar done: int = 0\non go {\n  let sig: exec\n  buffer emit sig\n  await sig\n  n += 1\n\
               if n < 5 { buffer emit sig } else { buffer emit sig }\n  done += 1\n}\n";
    let (text, diags) = ir(src);
    assert!(diags.iter().any(|d| d.starts_with("WS058")), "{diags:?}");
    assert!(text.contains("emit arms await"), "{text}");
    assert!(!text.contains("%done.VarRef"), "{text}");
    let (_, diags) = ir("@left in go: exec\nvar n: int = 0\non go {\n  let sig: exec\n  if n < 5 { buffer emit sig } else { buffer emit sig }\n  BroadcastChatMessage(\"x\")\n}\n");
    assert!(diags.iter().any(|d| d.starts_with("WS058")), "{diags:?}");
    assert!(!diags.iter().any(|d| d.starts_with("WSP001")), "{diags:?}");
}

#[test]
fn a_statement_after_a_buffer_emit_in_the_same_block_is_reported() {
    let (text, diags) = ir("@left in go: exec\nvar step: int = 0\non go {\n  let ramp: exec\n  step += 1\n  buffer emit ramp\n  step += 100\n}\n");
    assert!(diags.iter().any(|d| d.starts_with("WS058")), "{diags:?}");
    assert!(!text.contains("Value: 100)"), "{text}");
    // The kick of a back-edge loop still arms the `await` that follows it.
    let (text, diags) = ir("@left in go: exec\nvar step: int = 0\non go {\n  let ramp: exec\n  buffer emit ramp\n  await ramp\n  step += 1\n}\n");
    assert!(diags.iter().all(|d| !d.starts_with("WS058")), "{diags:?}");
    assert!(
        text.lines().any(|l| l.contains("Exec_Var_Set") && l.contains("Exec: %go,") && l.contains("Value: true")),
        "{text}"
    );
}

#[test]
fn a_back_edge_loop_inside_a_branch_rearms_every_iteration() {
    let src = "@left in go: exec\nvar running: bool = false\nvar step: int = 0\non go {\n  if !running {\n    running = true\n    step = 0\n\
               let ramp: exec\n    buffer emit ramp\n    await ramp\n    if step < 5 {\n      step += 1\n      buffer emit ramp\n    } else {\n      running = false\n    }\n  }\n}\n";
    let (text, _) = ir(src);
    assert!(text.contains("emit arms await"), "the back-edge does not re-arm the await:\n{text}");
}

#[test]
fn an_enum_returned_from_a_branch_of_a_pure_call_is_ws007() {
    let src = "mod safeDiv(a: int, b: int) -> Option<int> { if b == 0 { return None } else { return Some(a / b) } }\n\
               @left in a: int\n@left in b: int\n@right out text = match safeDiv(a, b) { Some(q) => \"${q}\", None => \"none\" }\n";
    let input = crate::CompileInput { source: src, file: "t.ws", module_name: None, fold_mode: crate::FoldMode::Auto };
    match crate::compile(input) {
        Err(crate::CompileError::HasErrors(ds)) => assert!(ds.iter().any(|d| d.code == "WS007"), "{ds:?}"),
        other => panic!("compiled: {:?}", other.map(|r| r.diagnostics)),
    }
}

#[test]
fn a_back_edge_to_an_await_on_an_outer_signal_in_a_branch_is_ws118() {
    let looped = "@left in go: exec\nvar i: int = 0\non go {\n  let s: exec\n  if i == 0 {\n    buffer emit s\n    await s\n\
                  if i < 5 {\n      i += 1\n      buffer emit s\n    }\n  }\n}\n";
    let (_, diags) = ir(looped);
    assert_eq!(diags.iter().filter(|d| d.starts_with("WS118")).count(), 1, "{diags:?}");
    // A one-tick delay in the branch has no back-edge.
    let delay = "@left in go: exec\nvar i: int = 0\non go {\n  let s: exec\n  if i == 0 {\n    buffer emit s\n    await s\n    i += 1\n  }\n}\n";
    let (_, diags) = ir(delay);
    assert!(diags.iter().all(|d| !d.starts_with("WS118")), "{diags:?}");
}

/// `let x = on <trigger> { .. }` runs its body as `on <trigger> { .. }` does,
/// config args and bound outputs included, and `on x` continues from its end.
#[test]
fn a_captured_event_honors_its_config_and_captures() {
    use super::declaring_scope::save_dump;
    let head = "@left in front: exec\n@left in room: zone\nvar n: int = 0\n";
    let rows = [
        ("pin", "let t = on front { n += 1 }\n", "on front { n += 1 }\n"),
        ("event config", "let t = on Clock(interval = 2.0) { n += 1 }\n", "on Clock(interval = 2.0) { n += 1 }\n"),
        (
            "config and bound outputs",
            "let t = on ZoneEntered(zone = room) -> (c) { if c != null { n += 3 } }\n",
            "on ZoneEntered(zone = room) -> (c) { if c != null { n += 3 } }\n",
        ),
        (
            "continued",
            "let t = on Clock(interval = 2.0) { n += 1 }\non t { n += 10 }\n",
            "on Clock(interval = 2.0) { n += 1\n n += 10 }\n",
        ),
    ];
    let mut failures = Vec::new();
    for (what, captured, direct) in rows {
        let (a, b) = (format!("{head}{captured}"), format!("{head}{direct}"));
        match (save_dump(&a, &[], true), save_dump(&b, &[], true)) {
            (Ok(x), Ok(y)) if x == y => {}
            other => failures.push(format!("{what}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A captured event on an expression trigger binds the call's outputs with
/// `-> (..)` exactly as the `on <call> -> (..)` handler does.
#[test]
fn a_captured_event_on_an_expression_binds_outputs() {
    use super::declaring_scope::save_dump;
    let head = "@left in go: exec\nvar n: int = 0\nfn add(x: int) -> int { return x + 1 }\n";
    let captured = format!("{head}let t = on add(5, exec = go) -> (r) {{ n += r }}\non t {{ n += 10 }}\n");
    let direct = format!("{head}on add(5, exec = go) -> (r) {{ n += r\n n += 10 }}\n");
    let (a, b) = (save_dump(&captured, &[], true), save_dump(&direct, &[], true));
    assert!(matches!((&a, &b), (Ok(x), Ok(y)) if x == y), "{a:?}\n{b:?}");
}

/// A captured `on CustomEvent(..)` is a receiver for every custom-event check
/// a plain handler gets: sender/receiver type agreement, slot inference and
/// the import scan.
#[test]
fn a_captured_custom_event_is_a_receiver() {
    use super::declaring_scope::save_dump;
    let head = "@left in go: exec\n@left in fv: float\nvar last: int = 0\non go {\n  SendCustomEvent(\"dmg\", fv)\n}\n";
    let mismatch = format!("{head}let r = on CustomEvent(\"dmg\") -> (amount: int) {{\n  last = amount\n}}\n");
    let (_, diags) = ir(&mismatch);
    assert!(diags.iter().any(|d| d.starts_with("WS030")), "{diags:?}");
    // An unannotated slot takes its sender's type, as on a plain handler.
    let head = "@left in go: exec\nvar last: float = 0.0\non go {\n  SendCustomEvent(\"dmg\", 2.5)\n}\n";
    let captured = format!("{head}let r = on CustomEvent(\"dmg\") -> (amount) {{\n  last = amount\n}}\n");
    let direct = format!("{head}on CustomEvent(\"dmg\") -> (amount) {{\n  last = amount\n}}\n");
    let (a, b) = (save_dump(&captured, &[], true), save_dump(&direct, &[], true));
    assert!(matches!((&a, &b), (Ok(x), Ok(y)) if x == y), "{a:?}\n{b:?}");
    // A type imported only for the captured receiver's capture is used.
    let r = super::compile_multi(
        "import { Amount } from \"lib\"\nvar last: int = 0\nlet r = on CustomEvent(\"dmg\") -> (a: Amount) { last = a }\n",
        &[("lib", "type Amount = int\n")],
    );
    assert!(r.diagnostics.iter().all(|d| d.code != "WS014"), "{:?}", r.diagnostics);
}

#[test]
fn dampen_keeps_its_target_type() {
    let (text, diags) = ir("@left in v: vector\n@left in i: int\n@left in c: color\n@left in r: rotator\n@left in f: float\n\
                            @right out dv = Dampen(v, 0.5)\n@right out di = Dampen(i, 1)\n@right out dc = Dampen(c, 0.5)\n\
                            @right out dr = Dampen(r, smoothTime = 2)\n@right out df = Dampen(f, 1)\n\
                            let x: vector = Dampen(v, 0.25)\n@right out dx = x\n");
    assert!(diags.iter().all(|d| !d.starts_with("WS")), "{diags:?}");
    for (out, ty) in [("%dv", "vector"), ("%di", "int"), ("%dc", "color"), ("%dr", "rotator"), ("%df", "float")] {
        let decl = line(&text, &format!("out {out} "));
        assert!(decl.contains(&format!("out {out} {ty} ")), "{decl}");
    }
    for (input, ty) in [("%v", "vector"), ("%i", "int"), ("%c", "color"), ("%r", "rotator"), ("%f", "float")] {
        let gate = text
            .lines()
            .find(|l| l.contains("= Dampen(") && l.contains(&format!("Target: {input},")))
            .unwrap_or_else(|| panic!("no Dampen of {input}:\n{text}"));
        assert!(
            gate.contains(&format!("(Target: {ty}, SmoothTime: float) -> (Value: {ty})")),
            "{gate}"
        );
    }
}

#[test]
fn remap_takes_lowercase_easing_settings() {
    let func = crate::catalog::enum_member_value("EBREasingFunction", "Bounce").expect("Bounce");
    let dir = crate::catalog::enum_member_value("EBREasingDirection", "InOut").expect("InOut");
    let baked = format!("Function: {func}, Direction: {dir})");
    for src in [
        "@left in v: float\n@right out r = Remap(v, 0.0, 1.0, 0.0, 100.0, function = Bounce, direction = InOut)\n",
        "@left in v: float\n@right out r = Remap(v, 0.0, 1.0, 0.0, 100.0, Function = Bounce, Direction = InOut)\n",
        "@left in v: float\n@right out r = Remap(v, 0.0, 1.0, 0.0, 100.0, function = EasingFunction.Bounce, direction = EasingDirection.InOut)\n",
    ] {
        let (text, diags) = ir(src);
        assert!(diags.iter().all(|d| !d.starts_with("WS")), "{diags:?}");
        assert!(line(&text, "Expr_Remap").contains(&baked), "{src}\n{text}");
    }
    let input = crate::CompileInput {
        source: "@left in v: float\n@left in f: int\n@right out r = Remap(v, 0.0, 1.0, 0.0, 100.0, function = f)\n",
        file: "t.ws",
        module_name: None,
        fold_mode: crate::FoldMode::Auto,
    };
    match crate::compile_to_world(input, Default::default()) {
        Err(crate::CompileError::HasErrors(ds)) => assert!(ds.iter().any(|d| d.code == "WS028"), "{ds:?}"),
        other => panic!("a runtime `function` must be WS028, got {:?}", other.map(|r| r.diagnostics)),
    }
}
