    use super::*;

    const DISPLAY_TEXT: &str = "BrickComponentData_WireGraph_Exec_PlayerState_DisplayText";

    fn empty_ctx() -> EmitContext {
        EmitContext {
            node_brick_ids: HashMap::default(),
            class_index: HashMap::default(),
            prefab_resolver: None,
            nested_compiler: None,
            wire_sources: HashMap::default(),
            var_labels: HashMap::default(),
            invisible: false,
            root_shell_brick_id: 0,
        }
    }

    /// A gate reading a var through a rerouter must still resolve the var's
    /// name for its tag, and a rerouter with nothing upstream (a bus lane
    /// node, which has no IR node and so no `wire_sources` entry) must end
    /// the walk rather than spin.
    #[test]
    fn var_label_walk_follows_rerouter_hops() {
        let var = NodeId::fresh();
        let hop = NodeId::fresh();
        let second_hop = NodeId::fresh();
        let mut ctx = empty_ctx();
        ctx.var_labels.insert(var, "count".to_string());
        ctx.class_index.insert(hop, gc::REROUTER);
        ctx.class_index.insert(second_hop, gc::REROUTER);
        ctx.wire_sources.insert((hop, WirePort::RerInput), var);
        ctx.wire_sources
            .insert((second_hop, WirePort::RerInput), hop);

        assert_eq!(
            resolve_var_label(&ctx, second_hop).map(String::as_str),
            Some("count")
        );

        let dangling = NodeId::fresh();
        ctx.class_index.insert(dangling, gc::REROUTER);
        assert_eq!(resolve_var_label(&ctx, dangling), None);
    }

    #[test]
    fn var_values_cover_all_variant_members() {
        use crate::ir::Type;
        // A var can hold any WireGraphVariant member, defaulted by its type.
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Bool)),
            WireVariant::Bool(false)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Int)),
            WireVariant::Int(0)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Float)),
            WireVariant::Number(_)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::String)),
            WireVariant::Str(_)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Vector)),
            WireVariant::Vector(_)
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Rotator)),
            WireVariant::Rotator { .. }
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Quat)),
            WireVariant::Quat { w, .. } if w == 1.0
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Color)),
            WireVariant::LinearColor { .. }
        ));
        assert!(matches!(
            var_type_to_wire_variant(Some(&Type::Entity)),
            WireVariant::Object(None)
        ));
        // Literal initializers convert to the matching variant member.
        assert!(matches!(
            literal_to_wire_variant(&Literal::String("x".into())),
            Some(WireVariant::Str(_))
        ));
        assert!(matches!(
            literal_to_wire_variant(&Literal::Vector {
                x: 1.0,
                y: 2.0,
                z: 3.0
            }),
            Some(WireVariant::Vector(_))
        ));
    }

    #[test]
    fn array_element_type_maps_to_array_variant() {
        use crate::ir::Type;
        let r = |t: Type| Type::Ref(Box::new(Type::Array(Box::new(t))));
        // element type is unwrapped through `ref array<T>`
        assert!(matches!(array_element_type(&r(Type::Int)), Some(Type::Int)));
        // each scalar element type selects the matching array variant member
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Int)),
            WireArrayVariant::Int64Array(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Float)),
            WireArrayVariant::DoubleArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Bool)),
            WireArrayVariant::BoolArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::String)),
            WireArrayVariant::StringArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Vector)),
            WireArrayVariant::VectorArray(_)
        ));
        assert!(matches!(
            empty_wire_array_variant(Some(&Type::Entity)),
            WireArrayVariant::ObjectArray(_)
        ));
        // unknown / missing element type falls back to a double array
        assert!(matches!(
            empty_wire_array_variant(None),
            WireArrayVariant::DoubleArray(_)
        ));
    }

    #[test]
    fn make_vector_has_data_struct_so_literals_persist() {
        // Regression: without this entry the inlined X/Y/Z literals of
        // `Vec(1.0, 2.0, 3.0)` are dropped at emit and the vector reads (0,0,0).
        let entry = data_struct_for_gate(crate::ir::gate_class::MAKE_VECTOR);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Expr_MakeVector",
                ["X", "Y", "Z"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn nearly_equal_has_data_struct_so_literals_persist() {
        // Regression: without this entry a literal `b`/tolerance arg of
        // `NearlyEqual(x, 1.0, 0.001)` drops to 0, so comparisons against any
        // non-zero constant always fail.
        let entry = data_struct_for_gate(crate::ir::gate_class::NEARLY_EQUAL);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Expr_NearlyEqual",
                ["InputA", "InputB", "Tolerance"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn every_gate_data_field_serializes_a_literal() {
        // Exhaustive write audit: one node per derived gate class, with a
        // schema-typed literal in EVERY representable field, emitted through
        // the real brz writer. Catches any field whose inlined literal can't
        // be boxed/serialized (the `min/max` and Vector→0i64 bug class) for
        // every component in the game, present and future.
        use crate::ir::Literal;
        use crate::ir::build::{AddNodeOpts, IdAllocator, ModuleBuilder};

        let schema = brdb::schemas::bricks_components_schema_max();
        let mut builder = ModuleBuilder::new("audit");
        builder.module.scopes.insert(
            crate::ir::ROOT_SCOPE_ID,
            crate::ir::ScopeInfo {
                kind: crate::ir::ScopeKind::ModuleRoot,
                source_range: crate::diagnostic::SourceRange::default(),
                parent: None,
            },
        );
        let mut ids = IdAllocator::default();
        let mut filled = 0usize;
        let mut gates = 0usize;

        for (class, (struct_name, fields)) in derived_gate_data() {
            // Special-cased emit branches with their own property contracts.
            if matches!(
                *class,
                "BrickComponentType_WireGraphPseudo_Var"
                    | "BrickComponentType_WireGraphPseudo_ArrayVar"
                    | "BrickComponentType_Internal_MicrochipInput"
                    | "BrickComponentType_Internal_MicrochipOutput"
                    | "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry"
            ) {
                continue;
            }
            let mut props: crate::collections::HashMap<crate::intern::Sym, Literal> =
                std::collections::HashMap::default();
            for field in fields {
                let Some(ty) = schema_field_type_str(struct_name, field) else {
                    continue;
                };
                let lit = if schema.get_enum(&ty).is_some() {
                    Some(Literal::Int(0))
                } else {
                    match ty.as_str() {
                        "str" => Some(Literal::String("x".into())),
                        "bool" => Some(Literal::Bool(true)),
                        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => {
                            Some(Literal::Int(1))
                        }
                        "f32" | "f64" => Some(Literal::Float(1.5)),
                        "WireGraphVariant" | "WireGraphPrimMathVariant" => {
                            Some(Literal::Float(2.5))
                        }
                        "Vector" => Some(Literal::Vector {
                            x: 1.0,
                            y: 2.0,
                            z: 3.0,
                        }),
                        "Rotator" => Some(Literal::Rotator {
                            pitch: 1.0,
                            yaw: 2.0,
                            roll: 3.0,
                        }),
                        "Quat" => Some(Literal::Quat {
                            x: 0.0,
                            y: 0.0,
                            z: 0.0,
                            w: 1.0,
                        }),
                        "class" | "object" => Some(Literal::Asset {
                            asset_type: "BRItemBase".into(),
                            asset_name: "Weapon_Pickaxe".into(),
                        }),
                        // arrays, composite structs, bundle_path_ref: not
                        // literal-representable — writer fills defaults.
                        _ => None,
                    }
                };
                if let Some(l) = lit {
                    props.insert(crate::intern::intern(field), l);
                    filled += 1;
                }
            }
            builder.add_gate(
                &mut ids,
                AddNodeOpts {
                    gate_class: class,
                    properties: props,
                    ..Default::default()
                },
            );
            gates += 1;
        }
        assert!(
            gates > 150,
            "sweep should cover the whole pair table, got {gates}"
        );
        assert!(filled > 200, "sweep should fill real fields, got {filled}");

        let module = builder.module;
        let lr = crate::layout::layout(&module);
        let brz = emit_brz(
            &module,
            &lr,
            &EmitOptions::default(),
            &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
        );
        assert!(
            brz.is_ok(),
            "every gate's data fields should serialize inlined literals: {:?}",
            brz.err()
        );
    }

    #[test]
    fn unlisted_gates_derive_data_structs_from_pair_table() {
        // Gates without a hand-written entry derive their (struct, full field
        // list) from brdb's game-extracted pair table + the schema, so a new
        // game gate embeds literals without a table edit.
        let entry = data_struct_for_gate("BrickComponentType_Internal_CharacterZoneEvent_Entered");
        let (s, fields, uwv) = entry.expect("zone event should derive from the pair table");
        assert_eq!(s, "BrickComponentData_Internal_CharacterZoneEvent");
        assert!(
            fields.contains(&"bCollisionEnabled_Player"),
            "derived fields should be the full struct: {fields:?}"
        );
        assert!(!uwv, "derived entries rely on per-field variant detection");
    }

    #[test]
    fn literal_params_with_schema_fields_are_mapped() {
        // Guard for the missing-data-mapping bug class (MakeVector,
        // EdgeDetector, ShowStatusMessage, Sleep, ...): a literal arg to a
        // builtin call is inlined into the gate's data properties at lowering,
        // but build_gate_component only writes fields listed in
        // data_struct_for_gate — an unlisted field silently drops the value.
        // For every call param that can carry a literal, if the gate's schema
        // data struct has a matching field, the mapping must list it.
        //
        // Not covered: gates whose data struct name isn't derivable from the
        // class name (checked via their mapping entry when present), and
        // gates with no schema struct at all (wire-only inputs — literals
        // there are a separate lowering concern).
        use crate::ir::Type;
        let schema = brdb::schemas::bricks_components_schema_max();
        let mut findings: Vec<String> = Vec::new();
        for (_, spec) in crate::catalog::calls::calls().iter() {
            for p in spec.params.iter() {
                if !matches!(
                    p.ty,
                    Type::String | Type::Int | Type::Float | Type::Bool | Type::Any
                ) {
                    continue;
                }
                let field = p.port.as_str();
                let entry = data_struct_for_gate(spec.gate_class);
                let (struct_name, listed) = match entry {
                    Some((s, f, _)) => (s.to_string(), Some(f)),
                    None => {
                        // Resolve the gate's data struct via the game-extracted
                        // pair table — many gates share structs (PrimMath,
                        // Float_Float, …) whose names aren't derivable from the
                        // class name. Not in the table → no data component.
                        match brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
                            .iter()
                            .find(|(t, _)| *t == spec.gate_class)
                        {
                            Some((_, s)) => (s.to_string(), None),
                            None => continue,
                        }
                    }
                };
                let covered = listed.is_some_and(|f| f.contains(&field));
                if covered {
                    continue;
                }
                // Allowlist. SetInventoryEntry builds its data in a dedicated
                // emit branch; the Teleport gates' Destination/Source are
                // composite TeleportDestination structs, deliberately
                // unmapped (wire-only — a scalar literal can't fill them).
                if matches!(
                    spec.gate_class,
                    "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry"
                        | "BrickComponentType_WireGraph_Exec_Entity_Teleport"
                        | "BrickComponentType_WireGraph_Exec_Entity_RelativeTeleport"
                ) {
                    continue;
                }
                let has_field = schema
                    .get_struct(&struct_name)
                    .zip(schema.intern.get(field))
                    .is_some_and(|(s, id)| s.get(&id).is_some());
                if has_field {
                    findings.push(format!(
                        "{}({}) class={} field {}",
                        spec.name, p.name, spec.gate_class, field,
                    ));
                }
            }
        }
        findings.sort();
        assert!(
            findings.is_empty(),
            "literal args to these params are silently dropped at emit — \
             add the field to the gate's data_struct_for_gate entry:\n{}",
            findings.join("\n")
        );
    }

    #[test]
    fn show_status_message_data_struct_includes_message() {
        // Regression: the entry existed but with an empty field list, so the
        // inlined message of `ShowStatusMessage(ctrl, "hi")` was dropped at
        // emit — the gate pasted with an empty internal Message and no wire.
        let entry = data_struct_for_gate(crate::ir::gate_class::PLAYERSTATE_SHOW_STATUS);
        assert_eq!(
            entry,
            Some((
                "BrickComponentData_WireGraph_Exec_PlayerState_ShowStatusMessage",
                ["Message"].as_slice(),
                false,
            )),
        );
    }

    #[test]
    fn field_enum_values_lists_justification() {
        let vals = field_enum_values(
            "BrickComponentType_WireGraph_Exec_PlayerState_DisplayText",
            "Justification",
        )
        .expect("justify maps to an enum field");
        for expected in ["Left", "Center", "Right"] {
            assert!(
                vals.iter().any(|v| v == expected),
                "missing {expected}: {vals:?}"
            );
        }
        // Names must be bare (no `EnumType::` prefix).
        assert!(vals.iter().all(|v| !v.contains("::")), "prefixed: {vals:?}");
    }

    #[test]
    fn enum_resolve_bare_name() {
        let v = try_resolve_enum(
            DISPLAY_TEXT,
            "Justification",
            &Literal::String("Left".into()),
        );
        assert_eq!(v, Some(0));
    }

    #[test]
    fn enum_resolve_prefixed_name() {
        let v = try_resolve_enum(
            DISPLAY_TEXT,
            "Justification",
            &Literal::String("EBRDisplayTextJustification::Center".into()),
        );
        assert_eq!(v, Some(1));
    }

    #[test]
    fn enum_resolve_int_passthrough() {
        let v = try_resolve_enum(DISPLAY_TEXT, "Justification", &Literal::Int(2));
        assert_eq!(v, Some(2));
    }

    #[test]
    fn enum_resolve_unknown_name_returns_none() {
        let v = try_resolve_enum(
            DISPLAY_TEXT,
            "Justification",
            &Literal::String("Nonsense".into()),
        );
        assert_eq!(v, None);
    }

    #[test]
    fn enum_resolve_easing_function_and_direction() {
        const EASING: &str = "BrickComponentData_WireGraph_Expr_MathEasing";
        // Named easing functions/directions resolve to their engine enum ints.
        assert_eq!(
            try_resolve_enum(EASING, "Function", &Literal::String("Quad".into())),
            Some(2)
        );
        assert_eq!(
            try_resolve_enum(EASING, "Function", &Literal::String("Cubic".into())),
            Some(3)
        );
        assert_eq!(
            try_resolve_enum(EASING, "Direction", &Literal::String("InOut".into())),
            Some(2)
        );
        assert_eq!(
            try_resolve_enum(EASING, "Direction", &Literal::String("Out".into())),
            Some(1)
        );
        // ints pass through
        assert_eq!(
            try_resolve_enum(EASING, "Function", &Literal::Int(5)),
            Some(5)
        );
    }

    #[test]
    fn enum_resolve_non_enum_field_returns_none() {
        let v = try_resolve_enum(DISPLAY_TEXT, "FontSize", &Literal::String("Left".into()));
        assert_eq!(v, None);
    }

    #[test]
    fn enum_resolve_easing_field_is_f64_not_enum() {
        assert_eq!(
            try_resolve_enum(
                DISPLAY_TEXT,
                "Transition",
                &Literal::String("Linear".into())
            ),
            None,
        );
    }

    #[test]
    fn enum_resolve_brick_direction() {
        let item_spawn = "BrickComponentData_ItemSpawn";
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupOffsetDirection",
                &Literal::String("X_Positive".into())
            ),
            Some(0),
        );
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupOffsetDirection",
                &Literal::String("Z_Negative".into())
            ),
            Some(5),
        );
    }

    #[test]
    fn enum_resolve_brick_axis() {
        let item_spawn = "BrickComponentData_ItemSpawn";
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupAnimationAxis",
                &Literal::String("X".into())
            ),
            Some(0),
        );
        assert_eq!(
            try_resolve_enum(
                item_spawn,
                "PickupAnimationAxis",
                &Literal::String("Z".into())
            ),
            Some(2),
        );
    }

    #[test]
    fn enum_resolve_collision_channel() {
        let bot_spawn = "BrickComponentData_BotSpawn";
        assert_eq!(
            try_resolve_enum(
                bot_spawn,
                "TeamCollisionChannel",
                &Literal::String("Channel1".into())
            ),
            Some(0),
        );
        assert_eq!(
            try_resolve_enum(
                bot_spawn,
                "TeamCollisionChannel",
                &Literal::String("Channel4".into())
            ),
            Some(3),
        );
    }
    #[test]
    fn microchip_io_labels_map_synthesized_names() {
        fn node_with_label(label: &str) -> Node {
            let mut props = HashMap::default();
            props.insert(*sym::PORT_LABEL, Literal::String(label.into()));
            Node {
                id: NodeId::fresh(),
                kind: crate::ir::NodeKind::Input,
                gate_class: "BrickComponentType_Internal_MicrochipInput",
                properties: std::sync::Arc::new(props),
                ports: std::sync::Arc::new(crate::ir::GateIO::default()),
                source_range: crate::diagnostic::SourceRange::default(),
                chip_id: None,
                chain_id: None,
                scope_id: crate::ir::ROOT_SCOPE_ID,
                note: None,
            }
        }
        // Synthesized exec plumbing reads "exec"; the anonymous return "_"
        // reads "return"; user names pass through; other underscore-prefixed
        // plumbing stays unlabeled.
        let get = |l: &str| microchip_io_label(&node_with_label(l));
        assert_eq!(get("_exec_in").as_deref(), Some("exec"));
        assert_eq!(get("_exec_out").as_deref(), Some("exec"));
        assert_eq!(get("_").as_deref(), Some("return"));
        assert_eq!(get("speed").as_deref(), Some("speed"));
        assert_eq!(get("_hidden"), None);
        assert_eq!(get(""), None);
    }

    #[test]
    fn chip_is_closed_reads_the_closed_prop() {
        let mut node = Node {
            id: NodeId::fresh(),
            kind: crate::ir::NodeKind::Chip,
            gate_class: gc::MICROCHIP,
            properties: std::sync::Arc::new(HashMap::default()),
            ports: std::sync::Arc::new(crate::ir::GateIO::default()),
            source_range: crate::diagnostic::SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        };
        assert!(!chip_is_closed(&node), "default is open");
        std::sync::Arc::make_mut(&mut node.properties)
            .insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        assert!(chip_is_closed(&node));
    }

    #[test]
    fn non_empty_map_variant_bakes_key_and_value_kinds() {
        // The non-empty MapVar byte layout is otherwise only exercisable in-game
        // (a .ws test can't run one), so pin the key/value member kinds and the
        // baked entry data for the three key flavors that reach emit.
        use crate::ir::{Literal, Type};

        // Int-keyed `Map<int, int>`: an `int64` key wrapper + `int64` value. The
        // type-derived kinds start empty (a fresh MapVar's at-rest state).
        let int_kinds =
            map_variant_from_type(&Type::Map(Box::new(Type::Int), Box::new(Type::Int)));
        assert_eq!(int_kinds.key, WireMapKey::Int64);
        assert_eq!(int_kinds.value, WireMapValue::Int64);
        assert!(int_kinds.entries.is_empty(), "type-derived kinds carry no entries");
        let int_map = wire_map_variant_from_literals(
            int_kinds,
            &[
                (Literal::Int(1), Literal::Int(10)),
                (Literal::Int(2), Literal::Int(20)),
            ],
        );
        assert_eq!(int_map.key, WireMapKey::Int64);
        assert_eq!(int_map.value, WireMapValue::Int64);
        assert_eq!(int_map.entries.len(), 2);
        assert_eq!(
            int_map.entries[0],
            (WireMapKeyData::Int64(1), WireMapValueData::Int64(10))
        );
        assert_eq!(
            int_map.entries[1],
            (WireMapKeyData::Int64(2), WireMapValueData::Int64(20))
        );

        // String-keyed `Map<string, float>`: the string key wrapper + a `double`
        // value. The literal key/value bake through unchanged.
        let str_kinds =
            map_variant_from_type(&Type::Map(Box::new(Type::String), Box::new(Type::Float)));
        assert_eq!(str_kinds.key, WireMapKey::Str);
        assert_eq!(str_kinds.value, WireMapValue::Number);
        let str_map = wire_map_variant_from_literals(
            str_kinds,
            &[(Literal::String("hp".into()), Literal::Float(2.5))],
        );
        assert_eq!(str_map.key, WireMapKey::Str);
        assert_eq!(str_map.value, WireMapValue::Number);
        assert_eq!(str_map.entries.len(), 1);
        assert_eq!(
            str_map.entries[0],
            (
                WireMapKeyData::Str("hp".to_string()),
                WireMapValueData::Number(2.5)
            )
        );

        // Atom keys hash (xxHash64) to an int64, so at emit they are just an int
        // literal over a `Map<int, V>` — they share the int-keyed `Int64` wrapper
        // rather than getting a distinct key kind.
        let atom_kinds =
            map_variant_from_type(&Type::Map(Box::new(Type::Int), Box::new(Type::Bool)));
        assert_eq!(atom_kinds.key, WireMapKey::Int64);
        assert_eq!(atom_kinds.value, WireMapValue::Bool);
        let atom_hash = 0x1234_5678_9abc_def0_i64;
        let atom_map = wire_map_variant_from_literals(
            atom_kinds,
            &[(Literal::Int(atom_hash), Literal::Bool(true))],
        );
        assert_eq!(atom_map.key, WireMapKey::Int64);
        assert_eq!(atom_map.value, WireMapValue::Bool);
        assert_eq!(atom_map.entries.len(), 1);
        assert_eq!(
            atom_map.entries[0],
            (WireMapKeyData::Int64(atom_hash), WireMapValueData::Bool(true))
        );
    }

    #[test]
    fn build_world_rejects_an_unresolvable_wire() {
        // A module wire whose endpoint never got a brick used to be logged to
        // stderr and dropped, laundering a lowering miscompile into a
        // format-valid `.brz` with a silently-missing wire. Emit must now fail.
        // Synthetic on purpose: it guards the emit backstop itself, so it stays
        // valid even after the lowering bugs that produce such wires are fixed.
        use crate::ir::Wire;
        use crate::ir::build::{AddNodeOpts, IdAllocator, ModuleBuilder, port_ref};

        let mut builder = ModuleBuilder::new("drop_guard");
        let mut ids = IdAllocator::default();
        // One real gate so the build proceeds normally up to the wire pass.
        builder.add_gate(
            &mut ids,
            AddNodeOpts {
                gate_class: "BrickComponentType_WireGraph_Expr_MathAdd",
                ..Default::default()
            },
        );

        // Lay out the clean module, THEN dangle a wire onto a node that was
        // never placed — so layout/wall assignment only ever see the real node,
        // and the phantom surfaces solely in emit's pass-3 resolution.
        let lr = crate::layout::layout(&builder.module);
        let phantom = crate::ir::NodeId::fresh();
        builder.module.wires.push(Wire {
            source: port_ref(phantom, "Value"),
            target: port_ref(phantom, "Value"),
        });

        // `World` isn't `Debug`, so match the Result rather than `expect_err`.
        match build_world(
            &builder.module,
            &lr,
            &EmitOptions::default(),
            &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
        ) {
            Ok(_) => panic!("a wire to an unplaced node must fail emit, not ship a bad save"),
            Err(EmitError::DroppedWire(_)) => {}
            Err(other) => panic!("expected DroppedWire, got {other:?}"),
        }
    }
