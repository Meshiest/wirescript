use super::*;

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
        node.properties.contains_key(&crate::intern::intern("InitialValue")),
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
    let r = compile(
        "array pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]\n",
    );
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
fn constant_vec_materializes_for_entity_gates() {
    // Entity gates take their Vector inputs from wires (no data struct), so
    // the folded constant re-materializes as a real MakeVector gate.
    let r = compile(
        "in e: entity\nin t: exec\non t { e.SetLocation(Vec(0.0, 0.0, 100.0)) }",
    );
    assert_no_errors(&r);
    let make = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MakeVector")
        .expect("SetLocation arg should materialize a MakeVector gate");
    assert_eq!(
        make.properties.get(&crate::intern::intern("Z")),
        Some(&crate::ir::Literal::Float(100.0)),
        "materialized MakeVector should carry the folded components"
    );
}

#[test]
fn constant_vec_materializes_for_split_vector() {
    // `.x` lowers to SplitVector, whose Input is a plain struct field — the
    // constant must stay a wired MakeVector, not silently zero out.
    let r = compile("let p = Vec(1.0, 2.0, 3.0)\nout x = p.x");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_SplitVector"));
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
    assert_eq!(pushes, 3, "three plain items should lower to three Push gates");
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
    assert!(!has_gate(&r, "_Unsupported"), "an array method lowered to _Unsupported");
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
    assert!(source_ports.contains("Origin"), "Origin port not wired: {source_ports:?}");
    assert!(source_ports.contains("Direction"), "Direction port not wired: {source_ports:?}");
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
    let r = compile(
        "in p: character\non p {\n  p.GiveWeapon($BRItemBase/Weapon_Pistol, 0)\n}",
    );
    assert_no_errors(&r);
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry")
        .expect("expected a SetInventoryEntry gate");
    match node.properties.get(&crate::intern::intern("ItemTypeIfItem")) {
        Some(crate::ir::Literal::Asset { asset_type, asset_name }) => {
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
    match node.properties.get(&crate::intern::intern("AudioDescriptor")) {
        Some(crate::ir::Literal::Asset { asset_type, asset_name }) => {
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
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Entity_SetTag"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Entity_GetTag"));
}

#[test]
fn find_player_and_change_detector_are_pure() {
    let r = compile(
        "in name: string\nlet p = FindPlayer(name)\nout changed = Change(name)",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_FindPlayer"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_ChangeDetector"));
}

#[test]
fn quat_make_split_dot_lower_to_their_gates() {
    let r = compile(
        "let q = Quat(0.0, 0.0, 0.0, 1.0)\nlet s = q.SplitQuat()\nout w = s.W\nout d = q.QuatDot(q)",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MakeQuaternion"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_SplitQuaternion"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_QuatDotProduct"));
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
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraph_Exec_Character_AddInventoryItem"
        })
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
    let r = compile("\
var x: int = 0
on x.value {
  let y = x + 10
}
out result = x");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "let inside on var.value handler should produce MathAdd gate");
}

#[test]
fn var_value_trigger_nested_if_lowers() {
    let r = compile("\
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
out result = dir");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"),
        "nested let inside on var.value + if should produce CompareEqual");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareLess"),
        "nested let inside on var.value + if should produce CompareLess");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_LogicalAND"),
        "nested let inside on var.value + if should produce LogicalAND");
}
