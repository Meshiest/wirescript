    use super::*;

    fn doc_state(source: &str) -> DocState {
        let pre_resolve = wirescript::parse(source, "test");
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck_with_inference(&resolved.ast, "test").0;
        DocState {
            source: source.to_string(),
            symbols: Vec::new(),
            doc_comments: Default::default(),
            type_map: tc.type_of_expr,
            if_contexts: tc.if_contexts,
            var_read_contexts: tc.var_read_contexts,
            dropped_ranges: tc.dropped_ranges,
            resource_estimates: Default::default(),
            pre_resolve_ast: pre_resolve.ast,
            imported_files: resolved.imported_files.clone(),
        }
    }

    #[test]
    fn cross_file_references_skip_open_docs_even_with_respelled_uris() {
        // Clients spell file URIs differently from `Url::from_file_path`
        // (VS Code sends `file:///c%3A/…`); the same-directory disk scan must
        // still recognize an open doc as the same file and not re-collect it,
        // or rename gets two identical edits per site and the client refuses
        // to apply them.
        let dir = std::env::temp_dir().join(format!("ws-lsp-refs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.ws");
        let source = "mod foo() {\n}\nin go: exec\non go { foo() }\n";
        std::fs::write(&file, source).unwrap();

        let canonical_uri = Url::from_file_path(&file).unwrap();
        // Same file, percent-encoded spelling → a different Url value.
        let respelled = canonical_uri.as_str().replacen("main.ws", "mai%6E.ws", 1);
        let client_uri = Url::parse(&respelled).unwrap();
        assert_ne!(client_uri, canonical_uri, "respelling must differ as a Url");
        assert_eq!(
            client_uri.to_file_path().unwrap(),
            canonical_uri.to_file_path().unwrap(),
            "…but point at the same file"
        );

        let mut docs = HashMap::new();
        docs.insert(client_uri.clone(), doc_state(source));

        // Cursor on the `foo` call site (line 3, `on go { foo() }`).
        let ds = docs.get(&client_uri).unwrap();
        let (target, current_sites) =
            references_at(&ds.pre_resolve_ast, &ds.source, "test", 3, 9).expect("target");
        let refs = collect_references_across_files(&docs, &client_uri, &target, &current_sites);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            refs.len(),
            2,
            "each site once (def + call), not doubled by the disk scan: {refs:?}"
        );
        assert!(
            refs.iter().all(|(u, _)| u == &client_uri),
            "sites must be reported under the open doc's URI: {refs:?}"
        );
    }

    fn symbols_for(source: &str) -> Vec<SymbolDef> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck_with_inference(&resolved.ast, "test").0;
        collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"))
    }

    fn labels(source: &str, line: usize, col: usize) -> Vec<String> {
        let syms = symbols_for(source);
        build_completions(source, &syms, line, col, &[])
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    fn labels_with_prefabs(
        source: &str,
        line: usize,
        col: usize,
        prefabs: &[String],
    ) -> Vec<String> {
        let syms = symbols_for(source);
        build_completions(source, &syms, line, col, prefabs)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn completion_inside_nested_block_isolates_from_outer_context() {
        // Cursor inside a `$```…``` ` block: the outer SpawnPrefab param names
        // (`velocity`, `offset`, `lifetime`) must NOT leak in; the block is
        // completed as its own isolated program.
        let src = "on go { let e = SpawnPrefab(lifetime = 0, offset = 1, $```in a: exec\non a { v }```) }\nin go: exec";
        let cursor = src.find("on a { v").unwrap() + "on a { ".len();
        let (line, col) = offset_to_line_col(src, cursor);
        let block = nested_block_at(src, "t", line, col);
        assert!(block.is_some(), "cursor inside the block should be detected");
        let (inner, il, ic) = block.unwrap();
        assert!(inner.contains("in a: exec"), "inner source captured: {inner:?}");
        let syms = symbols_for(&inner);
        let got: Vec<String> = build_completions(&inner, &syms, il, ic, &[])
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert!(
            !got.iter().any(|l| l == "velocity" || l == "offset" || l == "lifetime"),
            "SpawnPrefab params must not leak into the nested block: {got:?}"
        );
    }

    #[test]
    fn completion_outside_nested_block_is_not_isolated() {
        // A cursor that is NOT inside a nested block returns None (the normal
        // outer-file completion path runs).
        let src = "on go { SpawnPrefab(prefab = $./a.brz) }\nin go: exec";
        let col = src.find("SpawnPrefab").unwrap() + 3;
        assert!(nested_block_at(src, "t", 0, col).is_none());
    }

    #[test]
    fn prefab_ref_completes_from_candidate_paths() {
        let prefabs = vec![
            "./turret.brz".to_string(),
            "./enemies/tank.brz".to_string(),
            "./notes.txt".to_string(), // not a candidate; excluded by the scan
        ];
        // `SpawnPrefab(prefab = $./t` → offers the `./t…` prefab paths.
        let src = "on x { SpawnPrefab(prefab = $./t) }";
        let col = src.find("$./t").unwrap() + "$./t".len();
        let got = labels_with_prefabs(src, 0, col, &prefabs);
        assert!(got.contains(&"./turret.brz".to_string()), "got: {got:?}");
        assert!(
            !got.contains(&"./enemies/tank.brz".to_string()),
            "`./t` shouldn't match `./enemies/…`: {got:?}"
        );
    }

    #[test]
    fn completion_includes_builtin_events() {
        let ls = labels("", 0, 0);
        assert!(ls.iter().any(|l| l == "ChatCommand"), "ChatCommand missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "RoundStart"), "RoundStart missing");
        assert!(ls.iter().any(|l| l == "CharacterSpawned"), "CharacterSpawned missing");
        assert!(ls.iter().any(|l| l == "GetAim"), "GetAim function missing");
    }

    #[test]
    fn asset_ref_completes_types_then_names() {
        // After `$` → asset types.
        let src = "let w = $";
        let ls = labels(src, 0, 9);
        assert!(ls.iter().any(|l| l == "BRItemBase"), "asset type missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "BrickAudioDescriptor"), "asset type missing");
        // It's an isolated context — keywords/functions must not leak in.
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked into $ completion");

        // After `$BRItemBase/` → asset names of that type.
        let src2 = "let w = $BRItemBase/";
        let ls2 = labels(src2, 0, 20);
        assert!(ls2.iter().any(|l| l == "Weapon_Pistol"), "asset name missing: {ls2:?}");
        assert!(!ls2.iter().any(|l| l == "BRItemBase"), "type leaked into name completion");
    }

    #[test]
    fn asset_ref_config_param_completes_typed_refs() {
        // `weapon = <here>` offers full `$BRItemBase/Name` refs (the author needn't
        // know the type name), gated on the param being constant-only config.
        let src = "on go { GiveWeapon(c, weapon = ) }";
        let col = src.find("weapon = ").unwrap() + "weapon = ".len();
        let ls = labels(src, 0, col);
        assert!(
            ls.iter().any(|l| l == "$BRItemBase/Weapon_Pistol"),
            "asset-ref value completion: {ls:?}"
        );
        // `font = <here>` resolves to the font descriptor type.
        let src2 = "in p: controller\non go { p.DisplayText(\"hi\", font = ) }";
        let col2 = src2.rfind("font = ").unwrap() + "font = ".len()
            - (src2.find('\n').unwrap() + 1); // col within line 1
        let ls2 = labels(src2, 1, col2);
        assert!(
            ls2.iter().any(|l| l == "$BrickFontDescriptor/Roboto"),
            "font asset completion: {ls2:?}"
        );
    }

    #[test]
    fn event_config_params_complete() {
        // `on Clock(<here>)` offers the event's wired input + config args (the
        // call-param path has no CallSpec for an event).
        let src = "on Clock() { }";
        let col = src.find("Clock(").unwrap() + "Clock(".len();
        let ls = labels(src, 0, col);
        assert!(ls.iter().any(|l| l == "enabled = "), "config arg missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "interval = "), "wired input missing: {ls:?}");
    }

    #[test]
    fn enum_value_slot_completes_all_members() {
        // `direction = <here>` on SweepSimple offers every EBrickDirection member.
        let src = "on go { SweepSimple(500.0, direction = ) }";
        let col = src.find("direction = ").unwrap() + "direction = ".len();
        let ls = labels(src, 0, col);
        assert!(ls.iter().any(|l| l == "X_Positive"), "enum member missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Y_Negative"), "enum member missing: {ls:?}");
        // The sentinel is never offered.
        assert!(!ls.iter().any(|l| l == "MAX"), "sentinel leaked: {ls:?}");
    }

    #[test]
    fn match_arm_completes_variants() {
        // Cursor in arm-head position inside `match s {  }` (s: Shape) offers
        // the enum's variant names as candidate patterns. `col` is computed
        // against LINE 2's own text (not the whole multi-line `src`), same
        // convention as `plain_value_slot_offers_in_scope_idents_not_arg_names`
        // above - a whole-`src` offset would overshoot a middle-of-line target.
        let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\nin s: Shape\nout x = match s {  }\n";
        let line = 2;
        let col = src.lines().nth(line).unwrap().find("match s { ").unwrap() + "match s { ".len();
        let ls = labels(src, line, col);
        assert!(ls.iter().any(|l| l == "Circle"), "variant missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Empty"), "variant missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Rect"), "variant missing: {ls:?}");
    }

    #[test]
    fn discriminant_completes_after_value() {
        // `s.` on an enum-typed value offers `.Discriminant`.
        let src = "enum S { A, B }\nin s: S\nout d = s.\n";
        let line = 2;
        let col = src.lines().nth(line).unwrap().find("s.").unwrap() + "s.".len();
        let ls = labels(src, line, col);
        assert!(ls.iter().any(|l| l == "Discriminant"), "Discriminant missing: {ls:?}");
    }

    #[test]
    fn bare_enum_type_receiver_completes_variants() {
        // `Shape.` (the enum TYPE name, not a value) offers its variant
        // names, matching the compiler's own `Enum.Variant` construction
        // syntax rather than a value's `.Discriminant`.
        let src = "enum Shape { Empty, Circle(float) }\nout d = Shape.\n";
        let line = 1;
        let col = src.lines().nth(line).unwrap().find("Shape.").unwrap() + "Shape.".len();
        let ls = labels(src, line, col);
        assert!(ls.iter().any(|l| l == "Circle"), "variant missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Empty"), "variant missing: {ls:?}");
        assert!(!ls.iter().any(|l| l == "Discriminant"), "Discriminant should not appear on a type name: {ls:?}");
    }

    #[test]
    fn shadowed_enum_name_receiver_offers_no_variants() {
        // A value binding (`Shape: int` param) whose name equals the enum's
        // SHADOWS the type, so `Shape.` here is a (invalid) field access on an
        // int, never variant construction - the completion must not offer the
        // enum's variants. Mirrors the compiler's own shadow guard
        // (`resolve_variant_for_construction`).
        let src = "enum Shape { Empty, Circle(float) }\nmod useShape(Shape: int) -> int { return Shape. }\n";
        let line = 1;
        let col = src.lines().nth(line).unwrap().find("return Shape.").unwrap()
            + "return Shape.".len();
        let ls = labels(src, line, col);
        assert!(!ls.iter().any(|l| l == "Empty"), "shadowed enum leaked a variant: {ls:?}");
        assert!(!ls.iter().any(|l| l == "Circle"), "shadowed enum leaked a variant: {ls:?}");
    }

    #[test]
    fn match_arm_after_block_arm_completes_variants() {
        // A block-bodied arm may omit its trailing comma, so the cursor on the
        // line after `Empty => { 1 }` is still an arm head and must offer the
        // remaining variants.
        let src = "enum Shape { Empty, Circle(float) }\nin s: Shape\nout x = match s {\n  Empty => { 1.0 }\n  \n}\n";
        let line = 4; // the blank arm-head line between the block arm and `}`
        let col = 2;
        let ls = labels(src, line, col);
        assert!(ls.iter().any(|l| l == "Circle"), "variant missing after block arm: {ls:?}");
        assert!(ls.iter().any(|l| l == "Empty"), "variant missing after block arm: {ls:?}");
    }

    #[test]
    fn plain_value_slot_offers_in_scope_idents_not_arg_names() {
        // In a non-enum / non-asset value slot (`textId = <here>`), completion
        // offers in-scope identifiers, NOT the call's argument names.
        let src = "in c: controller\nlet myVal = 7\nin go: exec\non go {\n  c.DisplayText(\"hi\", textId = )\n}";
        let line = 4; // the DisplayText line
        let col =
            src.lines().nth(line).unwrap().find("textId = ").unwrap() + "textId = ".len();
        let ls = labels(src, line, col);
        assert!(ls.iter().any(|l| l == "myVal"), "in-scope ident missing: {ls:?}");
        assert!(
            !ls.iter().any(|l| l == "positionX = "),
            "argument names must not be offered in a value slot: {ls:?}"
        );
    }

    #[test]
    fn data_driven_config_completes_raw_field_names() {
        // The gate's raw settings-menu field names are offered alongside params.
        let src = "on go { SweepSimple(500.0, ) }";
        let col = src.find("500.0, ").unwrap() + "500.0, ".len();
        let ls = labels(src, 0, col);
        assert!(
            ls.iter().any(|l| l == "bOnlyHitPlayerBodyParts = "),
            "raw config field missing: {ls:?}"
        );
        assert!(ls.iter().any(|l| l == "Direction = "), "raw enum config field missing: {ls:?}");
    }

    #[test]
    fn data_driven_raw_enum_value_completes() {
        // `Direction = <here>` (raw field) still offers the enum members.
        let src = "on go { SweepSimple(500.0, Direction = ) }";
        let col = src.find("Direction = ").unwrap() + "Direction = ".len();
        let ls = labels(src, 0, col);
        assert!(ls.iter().any(|l| l == "X_Positive"), "raw enum member missing: {ls:?}");
    }

    #[test]
    fn string_dot_shows_only_string_methods() {
        let src = "let foo = \"\"\nfoo.";
        let ls = labels(src, 1, 4); // cursor right after `foo.`
        assert!(ls.iter().any(|l| l == "Length"), "Length missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Contains"), "Contains missing: {ls:?}");
        // The global list must NOT leak into a member-access context.
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked into member completion");
        assert!(!ls.iter().any(|l| l == "int"), "type leaked into member completion");
        assert!(!ls.iter().any(|l| l == "GetAim"), "non-string fn leaked");
        assert!(!ls.iter().any(|l| l == "ChatCommand"), "event leaked into member completion");
    }

    #[test]
    fn array_dot_shows_full_method_set() {
        let src = "var xs: int[]\nxs.";
        let ls = labels(src, 1, 3);
        // The full method set is offered, not just push/pop/length.
        for m in ["push", "find", "sort", "insert", "append", "slice"] {
            assert!(ls.iter().any(|l| l == m), "array method {m} missing: {ls:?}");
        }
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked");
    }

    #[test]
    fn var_array_dot_shows_array_methods() {
        // `var ids: string[]` is an array-typed var — it must complete array
        // methods (`.push`, `.find`, ...), not the var Value/prev fields.
        let src = "var ids: string[]\nids.";
        let ls = labels(src, 1, 4);
        assert!(ls.iter().any(|l| l == "push"), "push missing on var array: {ls:?}");
        assert!(ls.iter().any(|l| l == "find"), "find missing on var array: {ls:?}");
    }

    #[test]
    fn shadowed_var_completes_by_scope_not_first_decl() {
        // Same name in two scopes with different types: a file-scope `players:
        // string` and a handler-local `players: character[]`. At the cursor
        // inside the handler, `players.` must complete ARRAY methods (the
        // in-scope array), not the string's — a flat name-lookup would pick the
        // first decl and show the wrong members.
        let src = "var players: string = \"\"\non t {\n  var players: character[]\n  players.\n}";
        let ls = labels(src, 3, 10);
        assert!(ls.iter().any(|l| l == "push"), "array method missing on in-scope array: {ls:?}");
        assert!(ls.iter().any(|l| l == "fillFromPlayers"), "fillFromPlayers missing: {ls:?}");
        assert!(!ls.iter().any(|l| l == "Contains"), "string method leaked from shadowed decl: {ls:?}");
    }

    #[test]
    fn identifier_completion_dedups_shadowed_var_by_scope() {
        // A file-scope `players: string` and a handler-local `players:
        // character[]`. Completing a bare identifier at file scope (above and
        // outside the handler) must offer a SINGLE `players`, typed `string` —
        // the handler-local array is neither in scope nor declared yet.
        let src = "var players: string = \"\"\nplayers\non tick {\n  var players: character[]\n}";
        let syms = symbols_for(src);
        let items = build_completions(src, &syms, 1, 7, &[]);
        let players: Vec<&CompletionItem> = items.iter().filter(|i| i.label == "players").collect();
        assert_eq!(players.len(), 1, "expected one `players`, got: {players:?}");
        assert_eq!(
            players[0].detail.as_deref(),
            Some("string"),
            "file-scope `players` must show its string type, not the handler-local array"
        );
    }

    #[test]
    fn var_map_dot_shows_map_methods_not_array() {
        // `var m: Map<K, V>` completes the MAP method table — the map-only names
        // (`get`/`set`/`has`) and NOT array-only names (`push`/`find`).
        let src = "var m: Map<string, int>\nm.";
        let ls = labels(src, 1, 2);
        for m in ["get", "set", "has", "remove", "clear", "keys", "values"] {
            assert!(ls.iter().any(|l| l == m), "map method {m} missing: {ls:?}");
        }
        assert!(!ls.iter().any(|l| l == "push"), "array-only `push` leaked onto map: {ls:?}");
        assert!(!ls.iter().any(|l| l == "find"), "array-only `find` leaked onto map: {ls:?}");
    }

    #[test]
    fn var_map_via_type_alias_dot_shows_map_methods() {
        // A var whose type is a type ALIAS of a map (`type Scores = Map<...>`)
        // must resolve through the alias and complete map methods.
        let src = "type Scores = Map<string, int>\nvar s: Scores\ns.";
        let ls = labels(src, 2, 2);
        assert!(ls.iter().any(|l| l == "get"), "get missing on aliased map: {ls:?}");
        assert!(ls.iter().any(|l| l == "has"), "has missing on aliased map: {ls:?}");
        assert!(!ls.iter().any(|l| l == "push"), "array-only `push` leaked onto aliased map: {ls:?}");
    }

    #[test]
    fn var_map_via_generic_alias_instance_dot_shows_map_methods() {
        // `type Grid<T> = Map<string, T>`; `var g: Grid<int>` — the generic-alias
        // instance resolves by base name to the map table.
        let src = "type Grid<T> = Map<string, T>\nvar g: Grid<int>\ng.";
        let ls = labels(src, 2, 2);
        assert!(ls.iter().any(|l| l == "get"), "get missing on generic-aliased map: {ls:?}");
        assert!(ls.iter().any(|l| l == "set"), "set missing on generic-aliased map: {ls:?}");
        assert!(!ls.iter().any(|l| l == "push"), "array-only `push` leaked: {ls:?}");
    }

    #[test]
    fn namespace_dot_completes_members() {
        // `import * as u` exposes qualified `u.member` symbols; `u.` lists the
        // members by their bare names, and they never leak into the global list.
        let symbols = vec![
            SymbolDef { name: "u".into(), kind: "namespace", range: Default::default(), ty: None, exec: false, is_const: false },
            SymbolDef { name: "u.swap".into(), kind: "mod", range: Default::default(), ty: None, exec: false, is_const: false },
            SymbolDef { name: "u.clamp".into(), kind: "fn", range: Default::default(), ty: None, exec: false, is_const: false },
        ];
        let src = "let a = u.";
        let ls: Vec<String> = build_completions(src, &symbols, 0, src.len(), &[])
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert!(ls.contains(&"swap".to_string()), "swap missing: {ls:?}");
        assert!(ls.contains(&"clamp".to_string()), "clamp missing: {ls:?}");
        assert!(!ls.iter().any(|l| l.contains('.')), "qualified name leaked: {ls:?}");
        // A bare-identifier position must NOT list the qualified members.
        let bare: Vec<String> = build_completions("", &symbols, 0, 0, &[])
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert!(bare.contains(&"u".to_string()), "namespace alias missing: {bare:?}");
        assert!(!bare.iter().any(|l| l.contains('.')), "qualified leaked into global: {bare:?}");
    }

    #[test]
    fn typed_var_dot_shows_value_and_type_methods() {
        // `var pos: vector` completes `.Value`/`.prev` AND vector methods/swizzle.
        let src = "var pos: vector = Vec(1.0, 2.0, 3.0)\npos.";
        let ls = labels(src, 1, 4);
        assert!(ls.iter().any(|l| l == "Value"), "Value missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Normalize"), "vector method missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "x"), "swizzle x missing: {ls:?}");
    }

    #[test]
    fn static_var_dot_shows_value_prev() {
        let src = "static var score: int = 0\nscore.";
        let ls = labels(src, 1, 6);
        assert!(ls.iter().any(|l| l == "Value"), "Value missing on static var: {ls:?}");
        assert!(ls.iter().any(|l| l == "prev"), "prev missing on static var: {ls:?}");
    }

    #[test]
    fn named_record_alias_dot_completes_fields() {
        // `let p: Pos = …` where `Pos` is a `type` alias completes Pos's fields.
        let src = "type Pos = { x: int, y: int }\nlet p: Pos = { x: 1, y: 2 }\np.";
        let ls = labels(src, 2, 2);
        assert!(ls.iter().any(|l| l == "x"), "field x missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "y"), "field y missing: {ls:?}");
    }

    #[test]
    fn user_mod_call_completes_param_names() {
        // Inside `shift(‸)` for a user mod, its param names are offered — not the
        // whole global keyword/function list.
        let src = "mod shift(dist: int, fast: bool) {}\non x { shift() }";
        let line = "on x { shift() }";
        let col = line.find("shift(").unwrap() + "shift(".len();
        let ls = labels(src, 1, col);
        assert!(ls.iter().any(|l| l == "dist"), "param dist missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "fast"), "param fast missing: {ls:?}");
        assert!(!ls.iter().any(|l| l == "if"), "global list leaked: {ls:?}");
    }

    #[test]
    fn builtin_all_required_call_shows_required_params() {
        // `Vec(‸)` — all params required — offers x/y/z, not the global list.
        let src = "let v = Vec()";
        let col = src.find("Vec(").unwrap() + "Vec(".len();
        let ls = labels(src, 0, col);
        for p in ["x", "y", "z"] {
            assert!(ls.iter().any(|l| l == p), "required param {p} missing: {ls:?}");
        }
        assert!(!ls.iter().any(|l| l == "if"), "global list leaked: {ls:?}");
    }

    #[test]
    fn method_call_skips_receiver_param() {
        // `ctrl.DisplayText(‸)` must not offer the already-bound receiver param.
        let src = "in ctrl: controller\non x { ctrl.DisplayText(\"hi\", ) }";
        let line = "on x { ctrl.DisplayText(\"hi\", ) }";
        let col = line.find("\"hi\", ").unwrap() + "\"hi\", ".len();
        let ls = labels(src, 1, col);
        assert!(ls.iter().any(|l| l.starts_with("fontSize")), "optional param missing: {ls:?}");
        assert!(!ls.iter().any(|l| l == "controller" || l == "target"), "receiver leaked: {ls:?}");
    }

    #[test]
    fn annotation_completions_include_label_and_closed() {
        let ls = labels("", 0, 0);
        for a in ["@left", "@label", "@closed"] {
            assert!(ls.iter().any(|l| l == a), "annotation {a} missing: {ls:?}");
        }
    }

    /// The module-level run that opens a file is offered too. These are the
    /// annotations a reader is least likely to already know exist, so leaving
    /// them out of completion is what kept them invisible.
    #[test]
    fn annotation_completions_include_the_module_level_run() {
        let ls = labels("", 0, 0);
        for a in ["@fold", "@nofold", "@layout", "@flat"] {
            assert!(ls.iter().any(|l| l == a), "module annotation {a} missing: {ls:?}");
        }
    }

    #[test]
    fn input_reader_field_trigger_completes_record_fields() {
        // `on split.<here>` where `split = pl.InputReader()` completes the
        // splitter's record fields (Forward/Right/Up), not nothing.
        let src = "in pl: character\nlet split = pl.InputReader()\non split.";
        let ls = labels(src, 2, 9); // cursor right after `on split.`
        for f in ["Forward", "Right", "Up"] {
            assert!(ls.iter().any(|l| l == f), "record field {f} missing: {ls:?}");
        }
        // Member context is isolated — no keyword/function leak.
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked: {ls:?}");
    }

    #[test]
    fn receiver_method_completes_inside_nested_call_arg() {
        // `pl.AddVelocity(linear = pl.<here>` completes pl's methods, not the
        // enclosing AddVelocity's params — the member access wins.
        let src = "in pl: character\non x { pl.AddVelocity(linear = pl.) }";
        let col = src.lines().nth(1).unwrap().find("pl.)").unwrap() + 3;
        let ls = labels(src, 1, col);
        assert!(ls.iter().any(|l| l == "GetVelocity"), "GetVelocity missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "GetLocation"), "GetLocation missing: {ls:?}");
        // The enclosing call's param name must NOT show here.
        assert!(!ls.iter().any(|l| l == "linear = "), "call param leaked: {ls:?}");
    }

    #[test]
    fn plain_call_paren_still_shows_params() {
        // A plain `Call(<here>` (dot belongs to the callee) still offers the
        // call's optional params — member access must not hijack this.
        let src = "in pl: character\non x { pl.DisplayText(\"hi\", ) }";
        let col = src.lines().nth(1).unwrap().find("\"hi\", ") .unwrap() + "\"hi\", ".len();
        let ls = labels(src, 1, col);
        assert!(
            ls.iter().any(|l| l.starts_with("fontSize")),
            "optional param missing: {ls:?}"
        );
    }

    #[test]
    fn type_annotation_completes_builtin_game_enum_name() {
        // `var e: <here>` offers the built-in game enum type names (e.g.
        // `EasingFunction`), the same list a hand-declared `enum` would show
        // once it has a symbol. Game enums never get one, so they're seeded
        // straight from the catalog into the type-name completion list.
        let src = "var e: Eas";
        let ls = labels(src, 0, src.len());
        assert!(ls.iter().any(|l| l == "EasingFunction"), "EasingFunction missing: {ls:?}");
    }

    #[test]
    fn bare_builtin_game_enum_type_receiver_completes_variants() {
        // `EasingFunction.` (the built-in enum TYPE name, no `enum` decl
        // anywhere in the file) offers its variant names, same as a
        // user-declared enum's bare type receiver.
        let src = "out d = EasingFunction.\n";
        let col = src.find("EasingFunction.").unwrap() + "EasingFunction.".len();
        let ls = labels(src, 0, col);
        assert!(ls.iter().any(|l| l == "Bounce"), "Bounce missing: {ls:?}");
    }

    #[test]
    fn qualified_builtin_game_enum_variant_completes_discriminant() {
        // `EasingFunction.Bounce.` (a bare variant VALUE of a built-in game
        // enum) offers `.Discriminant`, same as any other enum-typed value
        // receiver. `member_receiver_at` only reports the identifier directly
        // before the dot (`Bounce`, not the qualified `EasingFunction.Bounce`),
        // so this only works if a bare variant name resolves to its uniquely
        // owning enum.
        let src = "out d = EasingFunction.Bounce.\n";
        let col = src.find("EasingFunction.Bounce.").unwrap() + "EasingFunction.Bounce.".len();
        let ls = labels(src, 0, col);
        assert!(ls.iter().any(|l| l == "Discriminant"), "Discriminant missing: {ls:?}");
    }

    #[test]
    fn config_arg_value_offers_qualified_builtin_enum_variant_alongside_bare_name() {
        // A config-arg value slot backed by a built-in game enum (`Easing`'s
        // `function` param, backed by `EBREasingFunction`) offers the
        // qualified `EasingFunction.Bounce` form alongside the existing bare
        // `Bounce` suggestion, so an author can disambiguate at the call site
        // the same way they would construct the value directly.
        let src = "on go { let e = Easing(0.0, 1.0, 0.5, function = ) }\nin go: exec";
        let col = src.find("function = ").unwrap() + "function = ".len();
        let ls = labels(src, 0, col);
        assert!(ls.iter().any(|l| l == "Bounce"), "bare member missing: {ls:?}");
        assert!(
            ls.iter().any(|l| l == "EasingFunction.Bounce"),
            "qualified member missing: {ls:?}"
        );
    }

    // ---------- end-to-end LSP rename / references (scoped resolver) ----------

    /// A fresh `Backend` wired through the real `LanguageServer` trait (so its
    /// `rename`/`references`/`prepare_rename` handlers run exactly as the
    /// editor would call them), with no documents open yet. The returned
    /// `LspService` owns the `Backend`; `.inner()` gives `&Backend`, which is
    /// enough to call the trait methods directly — no transport / socket is
    /// needed since we drive the handlers in-process.
    fn build_backend() -> LspService<Backend> {
        let (service, _socket) =
            LspService::new(|client| Backend {
                client,
                docs: Mutex::new(HashMap::new()),
                watch_files: std::sync::atomic::AtomicBool::new(false),
            });
        service
    }

    /// Insert a document into a running `Backend` the same shape `did_open`
    /// would produce (built via the module's own `doc_state` helper above).
    fn open_doc(service: &LspService<Backend>, uri: &Url, source: &str) {
        service
            .inner()
            .docs
            .lock()
            .unwrap()
            .insert(uri.clone(), doc_state(source));
    }

    /// Like `doc_state`, but with the symbol table populated — `goto_definition`
    /// reads `doc.symbols` (its `in_type_def` heuristic and `definition_at`
    /// both consult it), which `doc_state` leaves empty.
    fn doc_state_with_symbols(source: &str) -> DocState {
        let mut ds = doc_state(source);
        ds.symbols = symbols_for(source);
        ds
    }

    fn text_document_position(uri: &Url, line: u32, character: u32) -> TextDocumentPositionParams {
        TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position { line, character },
        }
    }

    /// Run the real `code_action` handler for a zero-width range at
    /// `(line, col)` and return only the `CodeAction` variants (none of this
    /// server's actions are bare `Command`s).
    async fn code_action_at(service: &LspService<Backend>, uri: &Url, line: u32, col: u32) -> Vec<CodeAction> {
        let pos = Position { line, character: col };
        let resp = service
            .inner()
            .code_action(CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range { start: pos, end: pos },
                context: CodeActionContext {
                    diagnostics: Vec::new(),
                    only: None,
                    trigger_kind: None,
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap()
            .unwrap_or_default();
        resp.into_iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(ca) => Some(ca),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect()
    }

    /// The `new_text` of a code action's first `TextEdit`.
    fn first_edit_text(action: &CodeAction) -> String {
        action
            .edit
            .as_ref()
            .and_then(|e| e.changes.as_ref())
            .and_then(|c| c.values().next())
            .and_then(|edits| edits.first())
            .map(|e| e.new_text.clone())
            .expect("code action has a text edit")
    }

    /// A tempdir under a name unique to this test + the process, so parallel
    /// test runs never collide.
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ws-lsp-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn rename_capture_does_not_touch_comments_types_or_other_files() {
        // main.ws: a `character` capture (a LOCAL binding, scoped to its own
        // handler body), a `: character` TYPE-position annotation, and a
        // `// character` comment. other.ws: an unrelated `character`-typed
        // `in` in a sibling file. None of the three may be touched by
        // renaming the capture — types are a separate namespace, comments/
        // strings are never AST sites, and captures never cross files.
        let dir = scratch_dir("rename-capture");
        let main_path = dir.join("main.ws");
        let main_source = "in go: exec\n\
in target: character\n\
// character comment\n\
on CharacterSpawned() -> (character) {\n\
  character.DisplayText(\"hi\")\n\
}\n";
        std::fs::write(&main_path, main_source).unwrap();
        let other_path = dir.join("other.ws");
        std::fs::write(&other_path, "in character: character\n").unwrap();

        let main_uri = Url::from_file_path(&main_path).unwrap();
        let service = build_backend();
        open_doc(&service, &main_uri, main_source);

        // Cursor on the capture's BODY use (`  character.DisplayText(...)`).
        let line = 4u32;
        let col = main_source.lines().nth(4).unwrap().find("character").unwrap() as u32;

        let edit = service
            .inner()
            .rename(RenameParams {
                text_document_position: text_document_position(&main_uri, line, col),
                new_name: "spawned".to_string(),
                work_done_progress_params: Default::default(),
            })
            .await
            .unwrap()
            .expect("rename should produce edits");

        std::fs::remove_dir_all(&dir).ok();

        let changes = match edit.document_changes.expect("document_changes present") {
            DocumentChanges::Operations(ops) => ops,
            DocumentChanges::Edits(_) => panic!("expected Operations, got flat Edits"),
        };
        assert_eq!(changes.len(), 1, "only main.ws should be edited: {changes:?}");
        let DocumentChangeOperation::Edit(te) = &changes[0] else {
            panic!("expected a TextDocumentEdit operation: {:?}", changes[0]);
        };
        assert_eq!(te.text_document.uri, main_uri, "edit landed in the wrong file");
        assert_eq!(
            te.edits.len(),
            2,
            "exactly the capture's decl + its one body use, nothing from the \
             type annotation, comment, or other.ws: {:?}",
            te.edits
        );
        for e in &te.edits {
            let OneOf::Left(edit) = e else {
                panic!("expected a plain TextEdit, got an annotated one: {e:?}")
            };
            assert!(
                edit.range.start.line == 3 || edit.range.start.line == 4,
                "edit escaped the capture's own handler body: {edit:?}"
            );
        }
    }

    #[tokio::test]
    async fn rename_exported_mod_updates_import_specifier_and_call_across_files() {
        // lib.ws defines an exported `mod`; main.ws imports and calls it.
        // Renaming from the CALL SITE in main.ws (an `Imported` target) must
        // edit BOTH files: lib.ws's declaration, and main.ws's import
        // specifier plus its call — even though lib.ws is never opened in
        // the editor (it's resolved from disk via the import path).
        let dir = scratch_dir("rename-import");
        let lib_path = dir.join("lib.ws");
        std::fs::write(&lib_path, "mod helper() {\n}\n").unwrap();
        let main_path = dir.join("main.ws");
        let main_source = "import { helper } from \"lib\"\nin go: exec\non go { helper() }\n";
        std::fs::write(&main_path, main_source).unwrap();

        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(&lib_path).unwrap();
        let service = build_backend();
        open_doc(&service, &main_uri, main_source);

        // Cursor on the `helper()` call in `on go { helper() }` (line 2).
        let line = 2u32;
        let col = main_source.lines().nth(2).unwrap().find("helper").unwrap() as u32;

        let edit = service
            .inner()
            .rename(RenameParams {
                text_document_position: text_document_position(&main_uri, line, col),
                new_name: "assist".to_string(),
                work_done_progress_params: Default::default(),
            })
            .await
            .unwrap()
            .expect("rename should produce edits");

        std::fs::remove_dir_all(&dir).ok();

        let changes = match edit.document_changes.expect("document_changes present") {
            DocumentChanges::Operations(ops) => ops,
            DocumentChanges::Edits(_) => panic!("expected Operations, got flat Edits"),
        };
        let mut by_file: HashMap<Url, usize> = HashMap::new();
        for op in &changes {
            let DocumentChangeOperation::Edit(te) = op else {
                panic!("expected a TextDocumentEdit operation: {op:?}")
            };
            by_file.insert(te.text_document.uri.clone(), te.edits.len());
        }
        assert_eq!(
            by_file.get(&lib_uri).copied(),
            Some(1),
            "lib.ws's declaration must be renamed even though it was never opened: {by_file:?}"
        );
        assert_eq!(
            by_file.get(&main_uri).copied(),
            Some(2),
            "main.ws needs both the import specifier AND the call site renamed: {by_file:?}"
        );
    }

    #[tokio::test]
    async fn find_references_is_scoped_not_a_name_string_search() {
        // Same fixture shape as the capture-rename test: `textDocument/
        // references` on the `character` capture must return ONLY its
        // handler-body occurrences (decl + one use) — never the `:
        // character` type annotation, the `// character` comment, or
        // other.ws. `references()` and `rename()` share `references_at`, so
        // this guards both from becoming a naive same-name string search.
        let dir = scratch_dir("find-refs-scoped");
        let main_path = dir.join("main.ws");
        let main_source = "in go: exec\n\
in target: character\n\
// character comment\n\
on CharacterSpawned() -> (character) {\n\
  character.DisplayText(\"hi\")\n\
}\n";
        std::fs::write(&main_path, main_source).unwrap();
        let other_path = dir.join("other.ws");
        std::fs::write(&other_path, "in character: character\n").unwrap();

        let main_uri = Url::from_file_path(&main_path).unwrap();
        let service = build_backend();
        open_doc(&service, &main_uri, main_source);

        let line = 4u32;
        let col = main_source.lines().nth(4).unwrap().find("character").unwrap() as u32;

        let locations = service
            .inner()
            .references(ReferenceParams {
                text_document_position: text_document_position(&main_uri, line, col),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: ReferenceContext { include_declaration: true },
            })
            .await
            .unwrap()
            .expect("references should find sites");

        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            locations.len(),
            2,
            "exactly the capture's decl + its one body use: {locations:?}"
        );
        for loc in &locations {
            assert_eq!(loc.uri, main_uri, "reference escaped into other.ws: {loc:?}");
            assert!(
                loc.range.start.line == 3 || loc.range.start.line == 4,
                "reference escaped the capture's own handler body: {loc:?}"
            );
        }
    }

    #[tokio::test]
    async fn find_references_on_field_name_is_refused() {
        // Regression: only `prepare_rename` used to refuse a field-name
        // cursor; `references`/`rename` called `references_at` directly, so
        // a field cursor sitting inside a COARSE enclosing binding's whole-
        // decl range (here `var x`'s initializer `p.field`) resolved to
        // THAT binding and returned its whole reference set instead of
        // being refused — mirrors the scoped_refs unit test
        // `field_access_inside_coarse_var_init_is_refused`, one layer up
        // through the real LSP `references` handler.
        let source = "type P = { field: int }\nin p: P\nvar x: int = p.field\n";
        let uri = Url::from_file_path(std::env::temp_dir().join("ws-lsp-find-refs-field.ws")).unwrap();
        let service = build_backend();
        open_doc(&service, &uri, source);

        // Cursor inside `field` of `p.field` (line 2).
        let col = source.lines().nth(2).unwrap().find("field").unwrap() as u32 + 1;

        let locations = service
            .inner()
            .references(ReferenceParams {
                text_document_position: text_document_position(&uri, 2, col),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: ReferenceContext { include_declaration: true },
            })
            .await
            .unwrap();

        assert!(
            locations.is_none() || locations.as_ref().unwrap().is_empty(),
            "field click must be refused, not resolve to the enclosing var's references: {locations:?}"
        );
    }

    #[tokio::test]
    async fn rename_export_updates_multiline_aliased_import_specifier_not_whole_decl() {
        // Regression: `find_name_range` used to search only the coarse
        // site's FIRST line via a plain substring match — a multi-line
        // `import { … }` specifier (only ALIASED specifiers stay coarse)
        // would fail to find the name on that first line and fall back to
        // replacing the WHOLE `import { … } from "…"` span. This fixture
        // also plants a decoy (`helperX`) on an earlier line whose name is
        // a PREFIX of the real target, to catch a non-word-boundary match
        // landing inside it.
        let dir = scratch_dir("rename-multiline-import");
        let lib_path = dir.join("lib.ws");
        let lib_source = "mod helper() {\n}\n";
        std::fs::write(&lib_path, lib_source).unwrap();
        let main_path = dir.join("main.ws");
        let main_source = "import {\n  helperX,\n  helper as x\n} from \"lib\"\nin go: exec\non go { x() }\n";
        std::fs::write(&main_path, main_source).unwrap();

        let lib_uri = Url::from_file_path(&lib_path).unwrap();
        let main_uri = Url::from_file_path(&main_path).unwrap();
        let service = build_backend();
        open_doc(&service, &lib_uri, lib_source);
        open_doc(&service, &main_uri, main_source);

        // Cursor on `helper` in `mod helper` (lib.ws, line 0).
        let line = 0u32;
        let col = lib_source.lines().next().unwrap().find("helper").unwrap() as u32;

        let edit = service
            .inner()
            .rename(RenameParams {
                text_document_position: text_document_position(&lib_uri, line, col),
                new_name: "assist".to_string(),
                work_done_progress_params: Default::default(),
            })
            .await
            .unwrap()
            .expect("rename should produce edits");

        std::fs::remove_dir_all(&dir).ok();

        let changes = match edit.document_changes.expect("document_changes present") {
            DocumentChanges::Operations(ops) => ops,
            DocumentChanges::Edits(_) => panic!("expected Operations, got flat Edits"),
        };
        let main_te = changes
            .iter()
            .find_map(|op| {
                let DocumentChangeOperation::Edit(te) = op else {
                    panic!("expected a TextDocumentEdit operation: {op:?}")
                };
                (te.text_document.uri == main_uri).then_some(te)
            })
            .expect("main.ws must be edited");

        assert_eq!(
            main_te.edits.len(),
            1,
            "only the specifier — the `helper` half — should be edited: {:?}",
            main_te.edits
        );
        let OneOf::Left(edit) = &main_te.edits[0] else {
            panic!("expected a plain TextEdit: {:?}", main_te.edits[0])
        };
        assert_eq!(edit.new_text, "assist");
        assert_eq!(
            edit.range.start.line, 2,
            "the edit must land on line 2 (`  helper as x`), not the decoy or the decl line: {edit:?}"
        );
        assert_eq!(edit.range.start.character, 2, "must land on `helper`, not swallow the whole decl: {edit:?}");
        assert_eq!(edit.range.end.character, 8, "must cover exactly the `helper` token: {edit:?}");
    }

    #[tokio::test]
    async fn rename_aliased_import_alias_is_file_local() {
        // `import { helper as assist } from "lib"` + `assist()`: the alias is
        // a FILE-LOCAL name. Renaming `assist` must edit ONLY main.ws — the
        // alias specifier token and the call — and must NOT touch lib.ws
        // (whose decl still spells `helper`) nor the `helper` half of the
        // specifier.
        let dir = scratch_dir("rename-alias-local");
        let lib_path = dir.join("lib.ws");
        std::fs::write(&lib_path, "mod helper() {\n}\n").unwrap();
        let main_path = dir.join("main.ws");
        let main_source =
            "import { helper as assist } from \"lib\"\nin go: exec\non go { assist() }\n";
        std::fs::write(&main_path, main_source).unwrap();

        let main_uri = Url::from_file_path(&main_path).unwrap();
        let lib_uri = Url::from_file_path(&lib_path).unwrap();
        let service = build_backend();
        open_doc(&service, &main_uri, main_source);

        // Cursor on the `assist()` call (line 2).
        let line = 2u32;
        let col = main_source.lines().nth(2).unwrap().find("assist").unwrap() as u32;

        let edit = service
            .inner()
            .rename(RenameParams {
                text_document_position: text_document_position(&main_uri, line, col),
                new_name: "aid".to_string(),
                work_done_progress_params: Default::default(),
            })
            .await
            .unwrap()
            .expect("rename should produce edits");

        std::fs::remove_dir_all(&dir).ok();

        let changes = match edit.document_changes.expect("document_changes present") {
            DocumentChanges::Operations(ops) => ops,
            DocumentChanges::Edits(_) => panic!("expected Operations, got flat Edits"),
        };
        assert_eq!(changes.len(), 1, "only main.ws may be edited: {changes:?}");
        let DocumentChangeOperation::Edit(te) = &changes[0] else {
            panic!("expected a TextDocumentEdit operation: {:?}", changes[0]);
        };
        assert_eq!(te.text_document.uri, main_uri, "an aliased-import rename must stay in-file");
        assert_ne!(te.text_document.uri, lib_uri, "lib.ws must be untouched");
        assert_eq!(
            te.edits.len(),
            2,
            "exactly the alias specifier token + the call site: {:?}",
            te.edits
        );
        for e in &te.edits {
            let OneOf::Left(edit) = e else {
                panic!("expected a plain TextEdit: {e:?}")
            };
            assert_eq!(edit.new_text, "aid", "each site renames to the new alias");
            // The specifier edit must land on the alias token, NOT the `helper`
            // half: on line 0 it starts at/after `import { helper as ` (col > 17).
            if edit.range.start.line == 0 {
                assert!(
                    edit.range.start.character > 17,
                    "specifier edit hit the `helper` half, not the alias: {edit:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn rename_export_updates_aliased_importer_original_name() {
        // lib.ws `mod helper(){}`; other.ws `import { helper as x }` + `x()`.
        // Renaming `helper` from lib.ws's decl must rename lib.ws's decl AND
        // the `helper` half of other.ws's specifier — but leave the alias `x`
        // and its call `x()` untouched.
        let dir = scratch_dir("rename-export-aliased-importer");
        let lib_path = dir.join("lib.ws");
        let lib_source = "mod helper() {\n}\n";
        std::fs::write(&lib_path, lib_source).unwrap();
        let other_path = dir.join("other.ws");
        let other_source = "import { helper as x } from \"lib\"\nin go: exec\non go { x() }\n";
        std::fs::write(&other_path, other_source).unwrap();

        let lib_uri = Url::from_file_path(&lib_path).unwrap();
        let other_uri = Url::from_file_path(&other_path).unwrap();
        let service = build_backend();
        open_doc(&service, &lib_uri, lib_source);
        open_doc(&service, &other_uri, other_source);

        // Cursor on `helper` in `mod helper` (lib.ws, line 0).
        let line = 0u32;
        let col = lib_source.lines().next().unwrap().find("helper").unwrap() as u32;

        let edit = service
            .inner()
            .rename(RenameParams {
                text_document_position: text_document_position(&lib_uri, line, col),
                new_name: "assist".to_string(),
                work_done_progress_params: Default::default(),
            })
            .await
            .unwrap()
            .expect("rename should produce edits");

        std::fs::remove_dir_all(&dir).ok();

        let changes = match edit.document_changes.expect("document_changes present") {
            DocumentChanges::Operations(ops) => ops,
            DocumentChanges::Edits(_) => panic!("expected Operations, got flat Edits"),
        };
        let mut by_file: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        for op in &changes {
            let DocumentChangeOperation::Edit(te) = op else {
                panic!("expected a TextDocumentEdit operation: {op:?}")
            };
            let plain: Vec<TextEdit> = te
                .edits
                .iter()
                .map(|e| match e {
                    OneOf::Left(t) => t.clone(),
                    OneOf::Right(_) => panic!("expected a plain TextEdit"),
                })
                .collect();
            by_file.insert(te.text_document.uri.clone(), plain);
        }

        let lib_edits = by_file.get(&lib_uri).expect("lib.ws must be edited");
        assert_eq!(lib_edits.len(), 1, "lib.ws: just the decl: {lib_edits:?}");
        assert_eq!(lib_edits[0].new_text, "assist");

        let other_edits = by_file.get(&other_uri).expect("other.ws must be edited");
        assert_eq!(
            other_edits.len(),
            1,
            "other.ws: only the `helper` half of the specifier, never the alias or its call: {other_edits:?}"
        );
        let e = &other_edits[0];
        assert_eq!(e.new_text, "assist");
        assert_eq!(e.range.start.line, 0, "the specifier edit is on the import line");
        // The `helper` token sits at cols 9..15 of `import { helper as x }`; the
        // alias `x` is at col 19. The edit must cover `helper`, not `x`.
        assert_eq!(e.range.start.character, 9, "edit must land on the `helper` token: {e:?}");
        assert_eq!(e.range.end.character, 15, "edit must end at the `helper` token: {e:?}");
    }

    #[tokio::test]
    async fn goto_definition_on_record_type_field_routes_through_definition_at() {
        // A cursor on a record-TYPE field name inside a `type` decl must NOT
        // resolve to the enclosing type's reference set; it falls through to
        // `definition_at`. Field rename/resolution is deferred, so
        // `definition_at` yields no target here — the load-bearing assertion
        // is that the response is NOT the type's references (which is what
        // the un-guarded `references_at` branch would return).
        let source = "type P = { field: int }\n";
        let uri = Url::from_file_path(std::env::temp_dir().join("ws-lsp-goto-field.ws")).unwrap();
        let service = build_backend();
        service
            .inner()
            .docs
            .lock()
            .unwrap()
            .insert(uri.clone(), doc_state_with_symbols(source));

        // Cursor on the record-type field name `field` (line 0).
        let col = source.find("field").unwrap() as u32;
        let resp = service
            .inner()
            .goto_definition(GotoDefinitionParams {
                text_document_position_params: text_document_position(&uri, 0, col),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap();

        assert!(
            resp.is_none(),
            "field click must route to definition_at, not the type's reference set: {resp:?}"
        );
    }

    #[tokio::test]
    async fn semantic_tokens_tag_capture_as_variable_and_annotation_as_type() {
        // Corrective semantic tokens for the position-blind grammar: a
        // `character` handler capture (and its use) must NOT show as the
        // `character` TYPE — the resolver reclassifies it as a plain Variable
        // token, while the actual `: character` TYPE annotation keeps Type.
        let source = "\
in go: exec
in foo: character
on CharacterSpawned() -> (character) {
  character.DisplayText(\"hi\")
}";
        let uri = Url::from_file_path(std::env::temp_dir().join("ws-lsp-semtok.ws")).unwrap();
        let service = build_backend();
        open_doc(&service, &uri, source);

        let resp = service
            .inner()
            .semantic_tokens_full(SemanticTokensParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap()
            .expect("semantic tokens should be produced");

        let SemanticTokensResult::Tokens(tokens) = resp else {
            panic!("expected a full SemanticTokens result, got a delta/partial variant");
        };

        // Decode the delta-encoded stream back to absolute (line, start_char,
        // length, token_type) tuples per the LSP spec's own encoding rule.
        let mut abs = Vec::new();
        let mut line = 0u32;
        let mut start = 0u32;
        for t in &tokens.data {
            line += t.delta_line;
            start = if t.delta_line == 0 { start + t.delta_start } else { t.delta_start };
            abs.push((line, start, t.length, t.token_type));
        }

        // Legend order (see the capability registration): Type=0,
        // Function=1, Parameter=2, Variable=3, Namespace=4.
        let decl_col = source.lines().nth(2).unwrap().find("character").unwrap() as u32;
        assert!(
            abs.iter().any(|&(l, c, _, ty)| l == 2 && c == decl_col && ty == 3),
            "capture decl should tokenize as Variable: {abs:?}"
        );

        let type_col = source.lines().nth(1).unwrap().find("character").unwrap() as u32;
        assert!(
            abs.iter().any(|&(l, c, _, ty)| l == 1 && c == type_col && ty == 0),
            ": character annotation should tokenize as Type: {abs:?}"
        );
    }

    /// An import must resolve against an OPEN EDITOR BUFFER, not the last bytes
    /// saved to disk. Otherwise a file importing something you are currently
    /// editing reports diagnostics for a version you can no longer see, until
    /// you hit save.
    #[test]
    fn open_buffer_shadows_disk_for_imports() {
        let dir = std::env::temp_dir().join(format!("ws-lsp-openimp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("lib.ws");
        // On DISK the export is named `OLD`.
        std::fs::write(&lib, "let OLD: int = 1\n").unwrap();

        // The unsaved editor buffer for that same file renames it to `FRESH`.
        let mut open = HashMap::new();
        open.insert(
            FsLoader.canonical_path(&lib.to_string_lossy(), "."),
            "let FRESH: int = 2\n".to_string(),
        );
        let loader = OpenDocLoader { open };

        let main_path = dir.join("main.ws");
        let r = resolve(
            "import { FRESH } from \"lib\"\nout o = FRESH",
            &main_path.to_string_lossy(),
            &loader,
        );
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            r.diagnostics.is_empty(),
            "the unsaved buffer's export must be visible, got {:?}",
            r.diagnostics
        );
    }

    /// The fallback still reaches disk for a file that is not open.
    #[test]
    fn unopened_import_still_loads_from_disk() {
        let dir = std::env::temp_dir().join(format!("ws-lsp-diskimp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.ws"), "let ONDISK: int = 1\n").unwrap();
        let loader = OpenDocLoader { open: HashMap::new() };
        let main_path = dir.join("main.ws");
        let r = resolve(
            "import { ONDISK } from \"lib\"\nout o = ONDISK",
            &main_path.to_string_lossy(),
            &loader,
        );
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            r.diagnostics.is_empty(),
            "a closed import must still load from disk, got {:?}",
            r.diagnostics
        );
    }

    /// A mod added since the last save must get a gate estimate immediately.
    ///
    /// `did_change` used to carry the previous estimate map forward instead of
    /// recomputing, and estimates are looked up by NAME — so a newly typed mod
    /// had no entry and hovered with no gate count while its neighbours showed
    /// one, which reads as the feature being broken rather than stale.
    #[tokio::test]
    async fn a_mod_added_since_the_last_save_still_gets_a_gate_estimate() {
        let dir = scratch_dir("estimate-freshness");
        let file = dir.join("main.ws");
        let opened = "mod alpha(v: *int) { v = v + 1 }\n";
        std::fs::write(&file, opened).unwrap();
        let uri = Url::from_file_path(&file).unwrap();

        let service = build_backend();
        service
            .inner()
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "wirescript".into(),
                    version: 1,
                    text: opened.into(),
                },
            })
            .await;

        // Type a second mod — a change only, never saved to disk.
        let edited = "mod alpha(v: *int) { v = v + 1 }\nmod beta(v: *int) { v = v * 3 + 2 }\n";
        service
            .inner()
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version: 2 },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: edited.into(),
                }],
            })
            .await;

        let docs = service.inner().docs.lock().unwrap();
        let est = &docs.get(&uri).expect("doc state").resource_estimates;
        assert!(
            est.contains_key("beta"),
            "a mod added by an unsaved edit must have an estimate; keys: {:?}",
            est.keys().collect::<Vec<_>>()
        );
        assert!(est.contains_key("alpha"), "the pre-existing mod must keep its estimate");
    }

    /// `if constexpr` semantics mean an untaken branch is never checked, so it
    /// can silently rot. Making it visible in the editor is the compensation.
    #[tokio::test]
    async fn hovering_dropped_code_says_it_was_removed_at_compile_time() {
        let dir = scratch_dir("const-dropped");
        let file = dir.join("main.ws");
        let src = "const MODE = 1\nvar x: int = 0\nin go: exec\non go {\n  if MODE == 1 { x = 1 } else { x = 2 }\n}\n";
        std::fs::write(&file, src).unwrap();
        let uri = Url::from_file_path(&file).unwrap();
        let service = build_backend();
        service.inner().did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem { uri: uri.clone(), language_id: "wirescript".into(), version: 1, text: src.into() },
        }).await;

        // column inside the `x = 2` else-block body
        let col = src.lines().nth(4).unwrap().find("x = 2").unwrap() as u32 + 1;
        let h = service.inner().hover(HoverParams {
            text_document_position_params: text_document_position(&uri, 4, col),
            work_done_progress_params: Default::default(),
        }).await.unwrap().expect("hover");
        let HoverContents::Markup(m) = h.contents else { panic!("expected markup") };
        assert!(m.value.contains("Removed at compile time"), "got {:?}", m.value);
    }

    // ---------- fill missing match arms (Task 22) ----------

    /// A `match` covering only `Circle` is missing `Empty` and `Rect`; the
    /// code action must offer both as witness arms, reusing the same
    /// witness engine (`typecheck::patterns::analyze`) the compiler's own
    /// WS054 exhaustiveness diagnostic runs. The placeholder body is a plain
    /// `todo`, never LSP snippet syntax (`${...}`), which this client would
    /// insert verbatim and fail to parse.
    #[tokio::test]
    async fn fill_missing_match_arms_offers_the_witnesses() {
        let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\nin s: Shape\nout x = match s {\n  Circle(r) => 1.0,\n}\n";
        let uri = Url::parse("file:///t.ws").unwrap();
        let service = build_backend();
        open_doc(&service, &uri, src);

        // cursor on the `match` line
        let actions = code_action_at(&service, &uri, 2, 10).await;
        let fill = actions
            .iter()
            .find(|a| a.title.contains("Fill missing match arms"))
            .expect("action offered");
        let text = first_edit_text(fill);
        assert!(text.contains("Empty"), "missing Empty witness: {text:?}");
        assert!(text.contains("Rect("), "missing Rect witness: {text:?}");
        assert!(text.contains("todo"), "missing plain placeholder body: {text:?}");
        assert!(!text.contains("${"), "must not emit LSP snippet syntax: {text:?}");
    }

    /// A `match` already covering every variant offers no fill action; the
    /// action must not appear when there's nothing to fill.
    #[tokio::test]
    async fn exhaustive_match_offers_no_fill_action() {
        let src = "enum Shape { Empty, Circle(float) }\nin s: Shape\nout x = match s {\n  Circle(r) => 1.0,\n  Empty => 0.0,\n}\n";
        let uri = Url::parse("file:///t2.ws").unwrap();
        let service = build_backend();
        open_doc(&service, &uri, src);

        let actions = code_action_at(&service, &uri, 2, 10).await;
        assert!(
            !actions.iter().any(|a| a.title.contains("Fill missing match arms")),
            "an exhaustive match must not offer a fill action: {actions:?}"
        );
    }
