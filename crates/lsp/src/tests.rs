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
            resource_estimates: Default::default(),
            pre_resolve_ast: pre_resolve.ast,
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

        let refs = collect_references_across_files(&docs, &client_uri, "foo");
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
        // ...and regular functions are still there.
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
        // String methods are offered.
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
            SymbolDef { name: "u".into(), kind: "namespace", range: Default::default(), ty: None, exec: false },
            SymbolDef { name: "u.swap".into(), kind: "mod", range: Default::default(), ty: None, exec: false },
            SymbolDef { name: "u.clamp".into(), kind: "fn", range: Default::default(), ty: None, exec: false },
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
        // splitter's record fields (Forward/Right/Up), not nothing. (`Jump` was
        // dropped in the InputSplitter rework; `Up` is its current axis.)
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
