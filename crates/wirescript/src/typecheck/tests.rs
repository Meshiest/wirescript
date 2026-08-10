    use super::*;
    use crate::parser::parse;

    fn tc(src: &str) -> TypeCheckResult {
        let p = parse(src, "test");
        assert!(
            p.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            p.diagnostics
        );
        typecheck(&p.ast, "test")
    }

    fn assert_no_diags(r: &TypeCheckResult) {
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    }

    #[test]
    fn generic_array_and_ref_syntax_desugar() {
        // `Array<V>` is an alternate spelling of `V[]`, and `Ref<V>` of `*V`.
        assert_no_diags(&tc("var a: Array<int> = [1, 2]"));
        assert_no_diags(&tc("var a: int[] = [1, 2]"));
        assert_no_diags(&tc("mod inc(v: Ref<int>) { v = v + 1 }"));
        assert_no_diags(&tc("mod inc(v: *int) { v = v + 1 }"));
    }

    #[test]
    fn generic_map_type_resolves() {
        // `Map<K, V>` resolves to a map type usable in an annotation.
        assert_no_diags(&tc("mod f(m: Map<string, int>) { }"));
    }

    #[test]
    fn map_method_args_are_type_checked() {
        // `m.set`/`m.get` must validate key/value args against the map's types —
        // storing the map itself where an `entity` value is expected (the
        // `prefabs.set(id, prefabs)` bug) is a WS003, not a dangling load wire.
        let r = tc("var m: Map<string, entity>\nin go: exec\non go { m.set(\"x\", m) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "map.set value mismatch must be WS003: {:?}",
            r.diagnostics
        );
        // A wrong KEY type is caught too.
        let rk = tc("var m: Map<string, int>\nin go: exec\non go { let v = m.get(m) }");
        assert!(
            rk.diagnostics.iter().any(|d| d.code == "WS003"),
            "map.get key mismatch must be WS003: {:?}",
            rk.diagnostics
        );
        // Matching types stay clean.
        assert_no_diags(&tc(
            "var m: Map<string, int>\nin go: exec\non go { m.set(\"x\", 5)\n  let v = m.get(\"x\") }",
        ));
    }

    #[test]
    fn map_method_missing_args_are_ws011() {
        // Map methods have fixed params (no variadics), so a MISSING arg must
        // be caught by arity (WS011) — the old ad-hoc validator only coerced
        // the args that were present, so `m.set("k")` (no value) type-checked
        // clean and emitted a dangling wire at load.
        let rs = tc("var m: Map<string, int>\nin s: exec\non s { m.set(\"k\") }");
        assert!(
            rs.diagnostics.iter().any(|d| d.code == "WS011"),
            "map.set missing value must be WS011: {:?}",
            rs.diagnostics
        );
        let rg = tc("var m: Map<string, int>\nin s: exec\non s { let g = m.get() }");
        assert!(
            rg.diagnostics.iter().any(|d| d.code == "WS011"),
            "map.get missing key must be WS011: {:?}",
            rg.diagnostics
        );
        let rk = tc("var m: Map<string, int>\nin s: exec\non s { let g = m.keys() }");
        assert!(
            rk.diagnostics.iter().any(|d| d.code == "WS011"),
            "map.keys missing dest array must be WS011: {:?}",
            rk.diagnostics
        );
        // Type-mismatch checks (WS003) must still fire — regression coverage
        // alongside `map_method_args_are_type_checked` above.
        let rv = tc("var m: Map<string, int>\nin s: exec\non s { m.set(\"k\", \"v\") }");
        assert!(
            rv.diagnostics.iter().any(|d| d.code == "WS003"),
            "map.set value mismatch must be WS003: {:?}",
            rv.diagnostics
        );
        let rke = tc("var m: Map<string, int>\nin s: exec\non s { let g = m.get(m) }");
        assert!(
            rke.diagnostics.iter().any(|d| d.code == "WS003"),
            "map.get key mismatch must be WS003: {:?}",
            rke.diagnostics
        );
        // Fully-supplied, correctly-typed calls stay clean.
        assert_no_diags(&tc(
            "var m: Map<string, int>\nin s: exec\non s { m.set(\"k\", 1)\n  let g = m.get(\"k\") }",
        ));
        assert_no_diags(&tc(
            "var m: Map<string, int>\nvar destArr: string[]\nin s: exec\non s { m.keys(destArr) }",
        ));
    }

    #[test]
    fn array_method_args_are_type_checked() {
        // Array-method call arguments were never routed through `check_args`,
        // so a mismatched arg (e.g. a string pushed onto an `int[]`) type
        // checked clean and emitted a mistyped/dangling pin at lower time.
        let r = tc("var xs: int[]\nin s: exec\non s { xs.push(\"a\") }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "arr.push(wrongType) must be WS003: {:?}",
            r.diagnostics
        );
        // A wrong INDEX type is caught too.
        let ri = tc("var xs: int[]\nin s: exec\non s { xs.insert(\"i\", 1) }");
        assert!(
            ri.diagnostics.iter().any(|d| d.code == "WS003"),
            "arr.insert(wrongIndexType, _) must be WS003: {:?}",
            ri.diagnostics
        );
        // Correctly-typed calls stay clean.
        assert_no_diags(&tc(
            "var xs: int[]\nin s: exec\non s { xs.push(1)\n  xs.insert(0, 2)\n  let v = xs.get(0) }",
        ));
        // `pop` takes no args — an extra positional arg is a WS011 arity error.
        let rp = tc("var xs: int[]\nin s: exec\non s { xs.pop(5) }");
        assert!(
            rp.diagnostics.iter().any(|d| d.code == "WS011"),
            "arr.pop(extraArg) must be WS011: {:?}",
            rp.diagnostics
        );
        // `sortMultiple` is a true variadic (empty declared params, opting it
        // out of arity checking) — passing parallel arrays must stay clean,
        // not wrongly fire "expects at most 0, got N".
        assert_no_diags(&tc(
            "var xs: int[]\nin s: exec\non s { xs.sortMultiple(xs, xs) }",
        ));
        // The `exec = <trigger>` named arg on an array read in a pure binding
        // must not trip arity or type checking.
        assert_no_diags(&tc(
            "var lut: color[]\nin i: int\nout c: color = lut.get(i, exec = i + 1).Value",
        ));
    }

    #[test]
    fn fill_from_zone_requires_zone_not_entity() {
        // `fillFromZone*`'s zone param is `zone` ONLY — the `Zone` slot never
        // accepts a plain entity, only a zone rerouter reference (`in z: zone`
        // → `Type::Zone`). The `z: zone` form, with and without the optional
        // tagFilter, type-checks clean:
        assert_no_diags(&tc(
            "in z: zone\nvar players: character[]\nin s: exec\non s { players.fillFromZonePlayers(z) }",
        ));
        assert_no_diags(&tc(
            "in z: zone\nvar players: character[]\nin s: exec\non s { players.fillFromZonePlayers(z, \"red\") }",
        ));
        assert_no_diags(&tc(
            "in z: zone\nvar ents: entity[]\nin s: exec\non s { ents.fillFromZoneEntities(z, \"tag\") }",
        ));
        // A plain `entity` in the zone slot is REJECTED (entity does not coerce
        // to zone) — the whole point of the `zone`-only param.
        let r = tc("in e: entity\nvar players: character[]\nin s: exec\non s { players.fillFromZonePlayers(e) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "an entity in a zone slot must be WS003: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn pure_chip_stmt_records_var_read_as_pure() {
        // A `let` inside a pure `chip { }` — even one that follows a handler, so
        // the chip is exec-wrapped — reads its vars continuously. The read must
        // record as PURE in `var_read_contexts` (false), or the hover shows the
        // wrong exec context. (Mirrors the lowering's per-statement chip purity.)
        let r = tc("var v: int = 0\nin go: exec\non go { v = 5 }\nchip { let x = v + 1 }\nout r: int = x");
        assert!(!r.var_read_contexts.is_empty(), "no var reads recorded");
        assert!(
            r.var_read_contexts.values().all(|&is_exec| !is_exec),
            "a pure chip's var read was recorded as exec: {:?}",
            r.var_read_contexts
        );
    }

    #[test]
    fn unknown_generic_errors() {
        let r = tc("mod f(x: Bogus<int>) { }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"),
            "unknown generic must be WS002: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ws033_conflict_incompatible_arg_types() {
        // Two arguments pin the same type param to types with no common widening
        // (int vs vector) — InferError::Conflict — surfaced as WS033.
        let r = tc(
            "in flag: bool\nin v: vector\n\
             mod pick<T>(c: bool, a: T, b: T) -> T { return a }\n\
             let x = pick(flag, 1, v)\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("must be the same type")),
            "int/vector conflict must fire the WS033 Conflict path: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ws033_unpinnable_return_only_type_param() {
        // `T` appears only in the return and the call passes no explicit type
        // argument, so nothing constrains it — InferError::Unpinnable — WS033.
        let r = tc(
            "mod zero<T: Numeric>() -> T { static var z: T = z\n return z }\n\
             out r: int = zero()\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("cannot infer type parameter")),
            "return-only type param must fire the WS033 Unpinnable path: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ws033_out_of_mask_arg_violates_bound() {
        // A `string` argument resolves `T` to a type outside its `Numeric` bound
        // mask — InferError::OutOfMask — WS033.
        let r = tc(
            "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\n\
             let x = onlyNumeric(\"hi\")\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("isn't allowed by its bound")),
            "out-of-mask arg must fire the WS033 OutOfMask path: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn use_before_declaration_is_ws021() {
        // A chip/mod call whose declaration lexically follows the call site
        // cannot resolve during lowering (decls register in source order), so
        // typecheck flags it so the editor surfaces it before compiling.
        let r = tc("mod caller() { let x = target(1) }\nmod target(n: int) -> int { return n }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS021"),
            "use-before-declaration must emit WS021; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn declaration_before_use_no_ws021() {
        let r = tc("mod target(n: int) -> int { return n }\nmod caller() { let x = target(1) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS021"),
            "declaration-before-use must NOT emit WS021; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn nested_prefab_types_as_prefab_and_lowers_to_literal() {
        let src = "in go: exec\non go { SpawnPrefab($```in a: exec```) }\n";
        let r = tc(src);
        assert!(
            !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "nested prefab as SpawnPrefab arg should typecheck: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn body_local_prefab_let_reaches_spawn_prefab() {
        // A prefab bound to a body-local `let` is a constant and must be
        // accepted by SpawnPrefab's constant-only `prefab=` config param.
        let r = tc("in go: exec\non go { let pf = $./foo.brz\n  SpawnPrefab(prefab = pf) }");
        assert_no_diags(&r);
    }

    #[test]
    fn top_level_prefab_let_reaches_spawn_prefab() {
        let r = tc("let pf = $./foo.brz\nin go: exec\non go { SpawnPrefab(prefab = pf) }");
        assert_no_diags(&r);
    }

    #[test]
    fn var_prefab_still_rejected_as_ws028() {
        // A genuine runtime `var` (never a compile-time constant, regardless
        // of its initializer) must still be rejected — the scoped-const fix
        // must not weaken constant-only enforcement for `prefab=`.
        let r = tc(
            "var pf: string = \"x\"\nin go: exec\non go { SpawnPrefab(prefab = pf) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS028"),
            "a var in constant-only 'prefab=' must still be WS028: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn nested_prefab_inner_error_remaps_into_outer_file() {
        // A syntax error INSIDE the block must surface (not be dropped), remapped
        // into the block's outer-file span (not left at the inner offset 0).
        let src = "in go: exec\non go { SpawnPrefab($```on q { let y = }```) }\n";
        let r = typecheck(&parse(src, "test").ast, "test");
        let err = r.diagnostics.iter().find(|d| d.severity == Severity::Error);
        assert!(err.is_some(), "inner error should surface: {:?}", r.diagnostics);
        let block = src.find("$```").unwrap();
        let d = err.unwrap();
        assert!(
            d.range.start.offset >= block,
            "diagnostic remapped into the block span (offset {} vs block start {})",
            d.range.start.offset,
            block
        );
    }

    #[test]
    fn nested_handler_on_event_param_is_valid() {
        // A nested handler triggered by the enclosing handler's EVENT data param
        // — `on CustomEvent(…, p: character) { on p { } }` and the negated
        // `on !p { }` — must type-check (the param is bound as EventParam).
        for src in [
            "on CustomEvent(\"x\", p: character) {\n  on p { }\n}\n",
            "on CustomEvent(\"x\", p: character) {\n  on !p { }\n}\n",
        ] {
            let r = typecheck(&parse(src, "test").ast, "test");
            assert!(
                !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
                "`on (!)p` over an event data param should type-check: {:?}",
                r.diagnostics
            );
        }
    }

    #[test]
    fn nested_prefab_lowers_to_nested_literal() {
        // The predeclare const-literal-of mapping turns Expr::NestedPrefab
        // into Literal::NestedPrefab, carrying the raw inner source verbatim
        // — mirroring how Expr::PrefabRef lowers to Literal::PrefabRef.
        let range = crate::diagnostic::SourceRange::default();
        let expr = crate::ast::Expr::NestedPrefab {
            source: "in a: exec".to_string(),
            range,
        };
        assert_eq!(
            crate::lower::expr_to_literal(&expr),
            Some(crate::ir::Literal::NestedPrefab {
                source: "in a: exec".to_string()
            })
        );
    }

    #[test]
    fn random_is_polymorphic_on_prim_math_variant() {
        // Random rides the PrimMath variant like the math operators: min/max may
        // be a vector/rotator/quat/color and the result matches, so assigning it
        // to a same-typed var is clean (no WS003 int-mismatch).
        let r = tc(
            "in a: vector\nin b: vector\nin c1: color\nin c2: color\nvar rv: vector = Vec(0.0, 0.0, 0.0)\nvar rc: color = ColorHex(\"#000000\")\nin go: exec\non go {\n  rv = Random(a, b)\n  rc = Random(c1, c2)\n}",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn random_int_stays_int() {
        // The scalar path is unchanged: Random(int, int) is an int, so it does
        // NOT assign into a vector var.
        let ok = tc("var n: int = 0\nin go: exec\non go { n = Random(1, 10) }");
        assert_no_diags(&ok);
        let bad =
            tc("var v: vector = Vec(0.0, 0.0, 0.0)\nin go: exec\non go { v = Random(1, 10) }");
        assert!(
            bad.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error),
            "Random(int, int) is int and must not assign into a vector var; got {:?}",
            bad.diagnostics
        );
    }

    #[test]
    fn asset_in_array_initializer_warns_ws024() {
        // Asset/prefab references are object references wired in from their own
        // brick; they can't bake into a constant array initializer (they'd be
        // silently dropped), so warn.
        let r =
            tc("var songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS024" && d.severity == Severity::Warning),
            "asset in array initializer should warn WS024; got {:?}",
            r.diagnostics
        );
        // A constant array initializer must NOT warn.
        let ok = tc("var nums: int[] = [1, 2, 3]");
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS024"),
            "constant array initializer must not warn; got {:?}",
            ok.diagnostics
        );
    }

    #[test]
    fn wrong_arg_count_is_ws022() {
        // User chips/mods have no default params, so too few (or too many)
        // positional args leaves a param unbound / an arg dropped.
        let too_few =
            tc("mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }");
        assert!(
            too_few.diagnostics.iter().any(|d| d.code == "WS022"),
            "too-few args must emit WS022; got {:?}",
            too_few.diagnostics
        );
        let too_many =
            tc("mod g(a: int) -> int { return a }\nin z: exec\non z { let x = g(1, 2) }");
        assert!(
            too_many.diagnostics.iter().any(|d| d.code == "WS022"),
            "too-many args must emit WS022; got {:?}",
            too_many.diagnostics
        );
    }

    #[test]
    fn correct_arg_count_no_ws022() {
        // Matching arity, and an extra `exec =` trigger (not a parameter), are
        // both fine.
        let ok = tc(
            "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1, 2) }",
        );
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS022"),
            "matching arity must NOT emit WS022; got {:?}",
            ok.diagnostics
        );
    }

    // ---- Task 5: user mod/chip + self-receiver calls routed through
    // `check_args` (regression tests written BEFORE the swap; they pass
    // against the pre-swap inline coerce loop too — see typecheck.rs) ----

    #[test]
    fn ucc_non_generic_mod_arg_type_mismatch_is_ws003() {
        // `f(v: vector)` called with an int literal must still be caught —
        // proves the (post-swap) `check_args` path still validates a plain
        // user-mod call's positional args, and that arity stays WS022-only
        // (no spurious WS011 from `check_args`'s own arity check).
        let r = tc("mod f(v: vector) { }\nin z: exec\non z { f(5) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "user-mod arg type mismatch must be WS003; got {:?}",
            r.diagnostics
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS011"),
            "user-mod calls must never emit check_args's own WS011 arity code; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ucc_non_generic_mod_valid_call_is_clean() {
        let r = tc("mod f(v: vector) { }\nin p: vector\nin z: exec\non z { f(p) }");
        assert_no_diags(&r);
    }

    #[test]
    fn ucc_generic_conflict_still_ws033_not_ws003() {
        // Same shape as `ws033_conflict_incompatible_arg_types`: inference
        // fails, so the param's type stays an unresolved `Type::Param`,
        // which the coerce path must keep skipping rather than coercing
        // against the raw `T` and emitting a spurious WS003 alongside it.
        let r = tc(
            "in flag: bool\nin v: vector\n\
             mod pick<T>(c: bool, a: T, b: T) -> T { return a }\n\
             let x = pick(flag, 1, v)\n",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS033"),
            "expected WS033: {:?}",
            r.diagnostics
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a failed generic inference must not also emit a spurious WS003; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ucc_self_receiver_arg_type_checked() {
        let clean = tc(
            "mod dist(self: vector, o: vector) -> float { return 0.0 }\n\
             in a: vector\nin b: vector\nin z: exec\non z { let d = a.dist(b) }",
        );
        assert_no_diags(&clean);

        let bad = tc(
            "mod dist(self: vector, o: vector) -> float { return 0.0 }\n\
             in a: vector\nin z: exec\non z { let d = a.dist(5) }",
        );
        assert!(
            bad.diagnostics.iter().any(|d| d.code == "WS003"),
            "a self-receiver call with a mismatched arg must be WS003; got {:?}",
            bad.diagnostics
        );
    }

    #[test]
    fn ucc_self_receiver_mismatched_receiver_is_ws003() {
        // The receiver binds to `self` as positional arg 0 — a receiver of
        // the wrong type must be caught too, not just the trailing args.
        let r = tc(
            "mod dist(self: vector, o: vector) -> float { return 0.0 }\n\
             in n: int\nin b: vector\nin z: exec\non z { let d = n.dist(b) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a mismatched self-receiver must be WS003; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn empty_script() {
        let r = tc("");
        assert_no_diags(&r);
    }

    #[test]
    fn var_int_init() {
        assert_no_diags(&tc("var x: int = 0"));
    }

    #[test]
    fn var_float_int_mismatch_coerces() {
        assert_no_diags(&tc("var x: float = 1"));
    }

    #[test]
    fn var_string_annotation_ok() {
        // Strings can now be stored in vars (WireGraphVariant supports `str`).
        assert_no_diags(&tc("var x: string = \"hi\""));
    }

    #[test]
    fn var_string_inferred_ok() {
        assert_no_diags(&tc("var x = \"hello\""));
    }

    #[test]
    fn var_string_inferred_usable_as_string() {
        // The inferred type must actually be `string`, not `any` — an `any`
        // operand has no `==` overload and would emit WS004.
        assert_no_diags(&tc("var s = \"\"\nout r = s == \"ready\""));
    }

    #[test]
    fn var_int_inferred_usable_in_math() {
        assert_no_diags(&tc("var n = 0\nout d = n + 1"));
    }

    #[test]
    fn var_float_inferred_usable_in_math() {
        assert_no_diags(&tc("var f = 1.5\nout d = f * 2.0"));
    }

    #[test]
    fn var_bool_inferred_usable_in_logic() {
        assert_no_diags(&tc("var b = true\nout d = b && false"));
    }

    #[test]
    fn var_negative_literal_inferred() {
        assert_no_diags(&tc("var n = -5\nout d = n + 1"));
    }

    #[test]
    fn var_nonliteral_init_refines_type() {
        // `var v = Vec(…)` has no literal init; the type refines from the
        // RHS in pass 2 (buffer-style), so vector math resolves.
        assert_no_diags(&tc(
            "var v = Vec(1.0, 2.0, 3.0)\nout d = v + Vec(0.0, 0.0, 1.0)",
        ));
    }

    #[test]
    fn handler_local_var_inferred() {
        assert_no_diags(&tc(
            "on RoundStart { var v = Vec(1.0, 2.0, 3.0)\n let w = v + v }",
        ));
    }

    #[test]
    fn var_inferred_type_catches_mismatch() {
        // Inference makes the var `int`, so assigning a vector is a real
        // WS003 — under the old `any` placeholder this passed silently.
        let r = tc("var n = 0\non RoundStart { n = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into inferred int var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn let_string_annotation_accepts_numeric() {
        // Everything primitive casts to string, so a string annotation on a
        // numeric expression is a format, not a WS016 type lie.
        assert_no_diags(&tc("let s: string = 5"));
    }

    #[test]
    fn let_string_annotation_accepts_entity_family() {
        assert_no_diags(&tc("in c: controller\nlet msg: string = c"));
    }

    #[test]
    fn concat_casts_character_to_string() {
        assert_no_diags(&tc("in p: character\nout s = \"hi \" .. p"));
    }

    #[test]
    fn vector_array_init_elements_are_constants() {
        // Constant Vec(…) folds to a literal, so it's a legal top-level
        // array initializer element (previously WS003).
        assert_no_diags(&tc(
            "var pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]",
        ));
    }

    #[test]
    fn var_array_of_vectors_infers_element_type() {
        // literal_expr_type knows constructor calls, so an unannotated
        // `var foo = [Vec(…)]` infers vector[] instead of any[].
        assert_no_diags(&tc("var pts = [Vec(1.0, 1.0, 1.0)]"));
    }

    #[test]
    fn color_var_inferred_and_reassignable() {
        // Color() now returns `color` (was `any`), so the var refines and a
        // later color assignment typechecks.
        assert_no_diags(&tc(
            "var tint = Color(1.0, 0.0, 0.0)\non RoundStart { tint = Color(0.0, 1.0, 0.0) }",
        ));
    }

    #[test]
    fn color_var_rejects_vector_assignment() {
        let r = tc("var tint = Color(1.0, 0.0, 0.0)\non RoundStart { tint = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into color var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_string_in_handler_ok() {
        assert_no_diags(&tc("on RoundStart { var x: string = \"hi\" }"));
    }

    #[test]
    fn let_string_is_fine() {
        let r = tc("let x = \"hello\"");
        assert_no_diags(&r);
    }

    #[test]
    fn unknown_event_diag() {
        let r = tc("on Bogus { }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS001"));
    }

    #[test]
    fn known_event_no_diag() {
        let r = tc("on RoundStart { }");
        assert_no_diags(&r);
    }

    #[test]
    fn expr_trigger_bool_and_compiles() {
        // `on a && b { x = 1 }` is desugared by the parser to
        //   let _on_expr_0 = a && b
        //   on _on_expr_0 { x = 1 }
        // Both steps should typecheck without errors.
        let src = "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }";
        assert_no_diags(&tc(src));
    }

    #[test]
    fn handler_event_param_typed() {
        let r = tc("on CharacterDied(c) { }");
        assert_no_diags(&r);
    }

    #[test]
    fn assignment_in_handler_ok() {
        let r = tc("var n: int = 0\non RoundStart { n = n + 1 }");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    }

    #[test]
    fn assignment_outside_exec_diag() {
        // Top-level assigns trip WS007 because there's no enclosing exec chain.
        let r = tc("var n: int = 0\nn = 1");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS007"));
    }

    #[test]
    fn binop_resolution_recorded() {
        let r = tc("var x: int = 1\nvar y = x + 2");
        // We don't care about the *contents* of opResolutions deeply here;
        // just that something was recorded.
        assert!(!r.op_resolutions.is_empty());
    }

    #[test]
    fn unknown_var_emits_diag() {
        let r = tc("on RoundStart { x = 1 }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS002"));
    }

    #[test]
    fn namespace_call_with_undefined_base_is_ws002() {
        // A namespace-qualified call whose base identifier isn't in scope — e.g.
        // an `import * as card` was removed but `card.drawLobby(...)` calls
        // remain. None of the namespace/array/receiver branches match, so
        // without an explicit check the call silently lowers to an
        // `_Unsupported` gate that does nothing at runtime.
        let r = tc("mod drawLobby(n: int) { }\non RoundStart { card.drawLobby(1) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS002" && d.message.contains("card")),
            "undefined namespace base must emit WS002; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn return_in_handler_no_error() {
        let r = tc("var x: int = 0\non RoundStart { x = 1\nreturn\nx = 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn return_in_exec_no_error() {
        let r = tc("var x: int = 0\non RoundStart { if x > 5 { return } }");
        assert_no_diags(&r);
    }

    #[test]
    fn not_on_int_no_error() {
        let r = tc("var x: int = 0\nlet y = !x");
        assert_no_diags(&r);
    }

    #[test]
    fn interp_ref_var_no_error() {
        let r = tc("var x: int = 0\nlet s = \"value: ${x}\"");
        assert_no_diags(&r);
    }

    // ---- chip single-output auto-unwrap ----
    #[test]
    fn chip_single_output_pure() {
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f == 42",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn chip_single_output_exec() {
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nvar err: int = 0\non RoundStart {\n  if f != 42 { err = 1 }\n}",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn chip_single_output_field_access_compat() {
        // f.result should still work for backwards compatibility
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f.result",
        );
        assert_no_diags(&r);
    }

    // ---- buffer ----
    #[test]
    fn buffer_decl() {
        let r = tc("var x: int = 0\nbuffer prev: int = x");
        assert_no_diags(&r);
    }

    #[test]
    fn buffer_inferred_type() {
        let r = tc("var x: int = 0\nbuffer prev = x + 1");
        assert_no_diags(&r);
    }

    // ---- mod / inline chip ----
    #[test]
    fn mod_decl_no_error() {
        let r = tc("mod inc(v: *int) { v = v + 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn mod_call_in_exec() {
        let r = tc("var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart { inc(x) }");
        assert_no_diags(&r);
    }

    // ---- anonymous chip ----
    #[test]
    fn anon_chip_shares_scope() {
        let r = tc("var x: int = 0\nchip { var y: int = 0 }\non RoundStart { x = 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn chip_on_handler() {
        let r = tc("var x: int = 0\nchip on RoundStart { x = 1 }");
        assert_no_diags(&r);
    }

    // ---- emit ----
    #[test]
    fn emit_in_exec() {
        let r = tc("var x: int = 0\nout result = x\non RoundStart { emit result }");
        assert_no_diags(&r);
    }

    // ---- bool literal ----
    #[test]
    fn bool_literal() {
        let r = tc("var x: bool = true\nvar y: bool = false");
        assert_no_diags(&r);
    }

    // ---- chip exec param as trigger ----
    #[test]
    fn chip_exec_param_trigger() {
        let r = tc(
            "chip Counter(bump: exec, reset: exec) -> (value: int) {\n  var n: int = 0\n  on bump { n = n + 1 }\n  on reset { n = 0 }\n  out value = n.Value\n}",
        );
        assert_no_diags(&r);
    }

    // ---- character to entity coercion ----
    #[test]
    fn character_coerces_to_entity() {
        let r = tc("in ch: character\non RoundStart { ch.SetLocation(Vec(0.0, 0.0, 0.0)) }");
        assert_no_diags(&r);
    }

    // ---- call arg validation ----
    #[test]
    fn call_too_many_args() {
        let r = tc("on RoundStart { Random(1, 2, 3, 4, 5) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
    }

    #[test]
    fn call_wrong_arg_type() {
        let r = tc("on RoundStart { SetLocation(42, Vec(0.0, 0.0, 0.0)) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("argument"))
        );
    }

    // ---- Task 4: builtin/receiver calls routed through `check_args` ----
    #[test]
    fn call_too_few_args() {
        // Fewer positional args than a builtin requires — the "requires N
        // args" WS011 branch (call_too_many_args above exercises the
        // "expects at most" branch).
        let r = tc("in e: entity\non RoundStart { SetLocation(e) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
    }

    #[test]
    fn receiver_call_wrong_arg_type() {
        // A receiver-method arg mismatch (`e.SetLocation(5)` — `pos` expects
        // a vector) must fire the same WS003 the plain-call form does.
        let r = tc("in e: entity\non RoundStart { e.SetLocation(5) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("argument"))
        );
    }

    #[test]
    fn receiver_call_valid_stays_clean() {
        let r = tc("in e: entity\non RoundStart { e.SetLocation(Vec(0.0, 0.0, 0.0)) }");
        assert_no_diags(&r);
    }

    #[test]
    fn enum_config_arg_still_validates() {
        // A bad enum-member name on a config (non-wire) param — `Easing`'s
        // `function` — must still be rejected as WS028 once the builtin arm
        // routes through the unified call checker.
        let r = tc("in t: float\nlet e = Easing(0.0, 1.0, t, function = Bogus)\n");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS028"),
            "bad enum config member must be WS028: {:?}",
            r.diagnostics
        );
    }

    // ---- namespace import ----
    #[test]
    fn namespace_symbol_registered() {
        use crate::resolve::{MemLoader, resolve};
        let loader = MemLoader {
            files: [("lib.ws".into(), "mod foo(v: *int) { v = v + 1 }".into())]
                .into_iter()
                .collect(),
        };
        let resolved = resolve("import * as lib from \"lib\"", "main.ws", &loader);
        let r = typecheck(&resolved.ast, "main.ws");
        assert_no_diags(&r);
    }

    // ---- namespace call argument checking (acceptance #2) ----
    // A namespaced call `ns.f(args)` previously routed straight to the
    // member's return type with NO argument checking at all — wrong types
    // and wrong arity were silent miscompiles. These pin `check_args` being
    // wired into the namespace call arm.

    fn ns_util_loader() -> crate::resolve::MemLoader {
        crate::resolve::MemLoader {
            files: [("util.ws".into(), "mod f(v: vector) { }".into())]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn namespace_call_wrong_arg_type_is_ws003() {
        use crate::resolve::resolve;
        let loader = ns_util_loader();
        let resolved = resolve(
            "import * as u from \"util\"\non RoundStart { u.f(5) }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "expected WS003 for namespaced call arg type mismatch: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn namespace_call_wrong_arity_is_ws011() {
        use crate::resolve::resolve;
        let loader = ns_util_loader();
        let resolved = resolve(
            "import * as u from \"util\"\non RoundStart { u.f() }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS011"),
            "expected WS011 for namespaced call arity mismatch: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn namespace_call_valid_stays_clean() {
        use crate::resolve::resolve;
        let loader = ns_util_loader();
        let resolved = resolve(
            "import * as u from \"util\"\non RoundStart { u.f(Vec(0.0, 0.0, 0.0)) }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws");
        assert_no_diags(&r);
    }

    #[test]
    fn namespace_import_of_uncalled_generic_member_is_clean() {
        // Regression: populating `NsDeclInfo.params` eagerly resolves a
        // member's param types at import time. A GENERIC member's params
        // reference its own type params (`a: T`), so the population loop must
        // push those type params into scope first — otherwise merely
        // importing the namespace (member never called) wrongly emits WS002
        // "unknown type 'T'". Assert a bare import compiles clean.
        use crate::resolve::resolve;
        let loader = crate::resolve::MemLoader {
            files: [(
                "gutil.ws".into(),
                "mod maxT<T: Numeric>(a: T, b: T) -> T { return a }".into(),
            )]
            .into_iter()
            .collect(),
        };
        let resolved = resolve(
            "import * as g from \"gutil\"\non RoundStart { }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS002"),
            "importing a namespace with an uncalled generic member must not \
             emit WS002: {:?}",
            r.diagnostics
        );
        assert_no_diags(&r);
    }

    // ---- chip let ----
    #[test]
    fn chip_let_pure_context() {
        let r = tc("var x: int = 0\nchip let doubled = x * 2");
        assert_no_diags(&r);
    }

    // ---- receiver call ----
    #[test]
    fn receiver_call_method() {
        let r = tc("var ctrl: controller\non RoundStart { ctrl.DisplayText(\"hi\") }");
        assert_no_diags(&r);
    }

    #[test]
    fn entity_receiver_accepts_character_controller_methods() {
        // An entity wire (e.g. Sweep's HitEntity) can be a player, so
        // character/controller receiver methods and params accept it.
        let r = tc("in e: entity\nin t: exec\non t { e.ShowStatusMessage(\"hi\") }");
        assert_no_diags(&r);
        let r2 = tc("in e: entity\nin t: exec\non t { ShowStatusMessage(e, \"hi\") }");
        assert_no_diags(&r2);
    }

    // ---- array index ----
    #[test]
    fn array_index_returns_element_type() {
        // Array reads require exec context (compile to Exec_ArrayVar_Get).
        let r = tc(
            "var items: int[]\nin trigger: exec\non trigger { let x = items[0]\nlet ok = x + 1 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn array_index_outside_exec_is_ws007() {
        // Array index read in pure context should emit WS007.
        let r = tc("var items: int[]\nlet x = items[0]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "expected WS007 for array index read outside exec context"
        );
    }

    #[test]
    fn array_param_index() {
        // Array params put the mod in exec context, so arr[idx] is fine.
        let r = tc("mod process(arr: int[], idx: int) {\n  let old = arr[idx]\n  out r = old\n}");
        assert_no_diags(&r);
    }

    #[test]
    fn array_param_index_dot_value() {
        // arr[i].value works fine — array params put the mod in exec context.
        let r =
            tc("mod process(arr: int[], idx: int) {\n  let old = arr[idx].value\n  out r = old\n}");
        assert_no_diags(&r);
    }

    // ---- array methods ----
    #[test]
    fn array_push_pop() {
        let r =
            tc("var items: int[]\nin trigger: exec\non trigger { items.push(1)\nitems.pop() }");
        assert_no_diags(&r);
    }

    #[test]
    fn array_length_returns_int() {
        let r = tc(
            "var items: int[]\nin trigger: exec\non trigger { let len = items.length()\nlet ok = len + 1 }",
        );
        assert_no_diags(&r);
        // len should be Int, so len + 1 should resolve without error.
        // If length() returned Any, the + would still work (Any coerces),
        // so also check the inferred type directly.
        let len_type = r.type_of_expr.values().find(|t| **t == Type::Int);
        assert!(len_type.is_some(), "length() should infer as Int");
    }

    // ---- if expression (ternary) ----
    #[test]
    fn if_expr_ternary() {
        let r = tc("var x: int = 0\nlet y = if x > 0 then 1 else 0");
        assert_no_diags(&r);
    }

    // ---- string interpolation ----
    #[test]
    fn string_interp_multiple() {
        let r = tc("var a: int = 1\nvar b: float = 2.0\nlet s = \"a=${a} b=${b}\"");
        assert_no_diags(&r);
    }

    // ---- octal/hex/binary literals ----
    #[test]
    fn numeric_literal_bases() {
        let r = tc("var a: int = 0xFF\nvar b: int = 0b1010\nvar c: int = 0o77");
        assert_no_diags(&r);
    }

    // ---- records & type aliases ----
    #[test]
    fn type_alias_record() {
        let r = tc("type Point = { x: int, y: int }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_literal_typed() {
        let r = tc("type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_field_access() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet sum = p.x + p.y",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_shorthand() {
        let r =
            tc("type Point = { x: int, y: int }\nlet x = 1\nlet y = 2\nlet p: Point = { x, y }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_spread() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet a: Point = { x: 1, y: 2 }\nlet b: Point = { ...a, y: 99 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_destructure() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet { x, y } = p\nlet sum = x + y",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_as_mod_param() {
        let r = tc(
            "type Point = { x: int, y: int }\nmod sum(p: Point) -> (r: int) { return p.x + p.y }\nlet p: Point = { x: 3, y: 4 }\nlet s = sum(p)",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn mod_param_record_destruct() {
        let r = tc(
            "type Point = { x: int, y: int }\nmod add({ x, y }: Point) -> int { return x + y }\nlet p: Point = { x: 3, y: 4 }\nlet sum = add(p)",
        );
        assert_no_diags(&r);
    }

    // ---- `any` type annotation ----

    #[test]
    fn any_in_port_wildcard_operators_typecheck() {
        // `any` on a port must resolve real operator overloads (the wildcard
        // behavior `Type::Opaque` gives it), not fall back to the generic
        // `Type::Any` error type that WS004 rejects for operators.
        let r = tc("in t: any\nlet a = t & 1\nlet b = t + 1\nlet c = t == \"x\"");
        assert_no_diags(&r);
    }

    #[test]
    fn any_let_annotation_no_error() {
        let r = tc("let x: any = 5\nlet y = x + 1");
        assert_no_diags(&r);
    }

    #[test]
    fn any_mod_param_no_error() {
        let r = tc("mod f(v: any) { let inner = v + 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn any_chip_param_and_output_no_error() {
        let r = tc("chip C(v: any) -> (z: any) { out z = v }\nlet c = C(1)\nlet y = c + 1");
        assert_no_diags(&r);
    }

    #[test]
    fn any_out_annotation_no_error() {
        let r = tc("in t: any\nout y: any = t");
        assert_no_diags(&r);
    }

    #[test]
    fn var_any_is_ws025() {
        let r = tc("var foo: any = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`var foo: any` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn static_var_any_is_ws025() {
        let r = tc("static var foo: any = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`static var foo: any` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn array_any_element_is_ws025() {
        let r = tc("var arr: any[]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`var arr: any[]` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn buffer_any_is_ws025() {
        let r = tc("buffer buf: any = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`buffer buf: any = 0` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_any_inside_handler_is_ws025() {
        // Same rejection for a statement-level `var` declared inside a
        // handler body (a separate code path from the top-level decl).
        let r = tc("in t: exec\non t { var foo: any = 0 }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "statement-level `var foo: any` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_unannotated_is_not_ws025() {
        // An unannotated var's placeholder type is `Type::Any` (the
        // generic fallback), never `Type::Opaque` — it must not trip the
        // `any`-storage rejection.
        let r = tc("var foo = 0");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS025"),
            "unannotated var must not emit WS025, got {:?}",
            r.diagnostics
        );
    }

    // ---- string → bool coercion (lowers to an inserted `!= ""` compare) ----

    #[test]
    fn if_string_condition_compiles() {
        // No dedicated bool-condition check exists for `if` — a string
        // condition typechecks cleanly, and lowering inserts the
        // `CompareNotEqual(s, "")` coercion gate in front of the Branch.
        let r = tc("in s: string\nin t: exec\nvar a: int = 0\non t { if s { a = 1 } }");
        assert_no_diags(&r);
    }

    #[test]
    fn let_bool_annotation_from_string_no_warning() {
        // Before the `String -> Bool` coercion rule this hit the generic
        // "let annotated as X, but expression has type Y" WS016 warning
        // (a mismatch is a legitimate re-annotation warning elsewhere, but
        // here it's a certified native coercion, not a type lie).
        let r = tc("in s: string\nlet b: bool = s");
        assert!(
            r.diagnostics.is_empty(),
            "`let b: bool = s` must not warn or error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_bool_assign_from_string_no_error() {
        // Assigning a string-typed value into a declared-bool var previously
        // hit a hard WS003 "expected Bool, got String" mismatch.
        let r = tc("in s: string\nin t: exec\nvar v: bool = false\non t { v = s }");
        assert_no_diags(&r);
    }

    // ---- string -> bool must NOT chain transitively into numerics ----
    //
    // Every consumer of `coerce()` (infer::coerce_or_emit, sig::check_args,
    // check_let_type_annotation, unify_glb) applies exactly ONE rule between
    // a source and a destination type — nothing composes String -> Bool with
    // Bool -> Int — and operator resolution (`resolve_op`) never consults
    // coercions at all. These pins keep it that way.

    #[test]
    fn string_does_not_coerce_to_int_destination() {
        // `let n: int = s` stays flagged (WS016 — the annotated-let mismatch
        // warning, the same diagnostic any non-coercing annotation gets)...
        let r = tc("in s: string\nlet n: int = s");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS016"),
            "`let n: int = s` must still warn WS016, got {:?}",
            r.diagnostics
        );
        // ...and assigning a string into a declared-int var stays a hard
        // WS003 error (the exec-assign path).
        let r = tc("in s: string\nin t: exec\nvar n: int = 0\non t { n = s }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.severity == Severity::Error),
            "`n = s` into an int var must stay a WS003 error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn string_math_operand_still_ws004() {
        // Operator resolution matches explicit rule lists only (no coercion
        // consult), and the math gates have no string operand rules — a
        // string on either side of `+` must keep erroring, not sneak in as
        // String -> Bool -> Int. (`..` is the string concat operator.)
        let r = tc("in s: string\nlet a = s + 1");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS004"),
            "`s + 1` must stay WS004, got {:?}",
            r.diagnostics
        );
        let r = tc("let a = \"a\" + 1");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS004"),
            "`\"a\" + 1` must stay WS004, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn string_into_int_builtin_param_still_errors() {
        // A string argument into an int-typed builtin port stays WS003 —
        // arg checking does one coerce(String, Int) = Mismatch, with no
        // bool hop available.
        let r = tc("in s: string\nlet c = ColorSRGB(s, 0, 0, 255)");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("expected int")),
            "string into ColorSRGB's int param must stay WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn bool_to_int_coercion_still_works() {
        // Regression pin: the neighboring bool -> int coercion (and the
        // annotated-let form) must be unaffected by the string-truthiness
        // rule sitting next to it in coerce().
        let r = tc("in f: bool\nlet n: int = f\nin t: exec\nvar m: int = 0\non t { m = f }");
        assert_no_diags(&r);
    }

    // ---- annotated `out` value/annotation agreement ----

    #[test]
    fn annotated_out_bool_from_string_coerces() {
        // `out y: bool = s` is the string → bool coercion (the `!= ""`
        // compare inserts at lowering) — no diagnostic.
        let r = tc("in s: string\nout y: bool = s");
        assert_no_diags(&r);
    }

    #[test]
    fn annotated_out_int_from_string_is_ws003() {
        // Pre-existing hole: annotated outs never checked their value
        // against the annotation, so `out y: int = s` passed silently and
        // emitted a mistyped pin. Now WS003.
        let r = tc("in s: string\nout y: int = s");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.severity == Severity::Error),
            "`out y: int = s` must be a WS003 error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn annotated_out_string_from_int_formats() {
        // Per the coercion table, int → string is `ViaString` (format
        // gate), not a mismatch — an annotated string out accepts a
        // numeric value without diagnostics.
        let r = tc("in n: int\nout label: string = n");
        assert_no_diags(&r);
    }

    #[test]
    fn annotated_ref_out_still_accepts_var() {
        // `out y: *int = x` — the ref annotation unwraps for the check
        // (int against int), so the ref-exposure pattern stays clean.
        let r = tc("var x: int = 0\nout y: *int = x");
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "ref out must not error: {:?}", errors);
    }

    // ---------- `self`-receiver (UFCS) ----------

    #[test]
    fn self_mod_shadowing_builtin_receiver_is_ws035() {
        // A user `self`-mod named exactly like a builtin receiver-method on the
        // same receiver type (`Dot` on vector) would be silently shadowed by the
        // builtin at every call site — a footgun, so it is rejected at the
        // declaration.
        let r = tc("mod Dot(self: vector, o: vector) -> float { return 0.0 }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS035"),
            "a self-mod shadowing a builtin receiver-method must emit WS035; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn self_mod_distinct_name_no_shadow() {
        // A distinctly-named self-mod does not collide with any builtin.
        let r = tc("mod dist(self: vector, o: vector) -> float { return self.Dot(o) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS035"),
            "a distinct-named self-mod must not be flagged as shadowing; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn self_mod_same_name_different_receiver_no_shadow() {
        // `Dot` is a builtin receiver-method on `vector`; a self-mod named `Dot`
        // whose receiver is a DIFFERENT type (int) does not overlap it, so the
        // builtin never shadows it and there is no error.
        let r = tc("mod Dot(self: int) -> int { return self }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS035"),
            "the same name on a different receiver type must not shadow; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn non_self_mod_receiver_type_still_not_shadow() {
        // A normal (non-`self`) mod is never a receiver method, so it can never
        // shadow a builtin receiver-method even if it shares the name.
        let r = tc("mod Dot(a: vector, o: vector) -> float { return a.Dot(o) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS035"),
            "a non-self mod must never be flagged as shadowing; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn self_mod_method_call_wrong_arg_count_is_ws022() {
        // `a.dist()` is missing the `o` argument: with the receiver bound as
        // arg 0 the call still has too few args for `dist(self, o)`. Proves the
        // method call is resolved to the user mod (before the feature it typed
        // as `any` and no arg-count check fired).
        let r = tc(
            "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
             in a: vector\nin go: exec\non go { let d = a.dist() }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS022"),
            "a receiver method call with too few args must emit WS022; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn non_self_mod_method_call_is_ws036() {
        // `f`'s first param is not named `self`, so `v.f(w)` is NOT a valid
        // method call — only `self` opts in. Rather than silently typing as
        // `any` and lowering to an `_Unsupported` no-op, it is a hard error.
        let r = tc(
            "mod f(a: vector, o: vector) -> float { return a.Dot(o) }\n\
             in v: vector\nin w: vector\nin go: exec\non go { let d = v.f(w) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS036"),
            "a non-self mod called with method syntax must emit WS036; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn unknown_base_method_call_stays_ws002_not_ws036() {
        // An unknown receiver base is the primary problem — it stays WS002 and
        // must NOT be masked by the non-self-mod WS036 check, even when the
        // method name happens to be a known non-self mod.
        let r = tc(
            "mod scale(v: vector, k: float) -> vector { return v * k }\n\
             in go: exec\non go { let s = missing.scale(2.0) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"),
            "an unknown base must emit WS002; got {:?}",
            r.diagnostics
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS036"),
            "WS036 must not double-report over an unknown base; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn non_constant_label_expr_is_rejected() {
        // A runtime `@label` on a PORT (`out`) can't be a dynamic label — a
        // rerouter pin has no text component to wire — so it stays a hard
        // WS040. (Top-level `var`s DO support dynamic labels; see below.)
        let r = tc("in x: int\n@label(x) out v: int = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS040"),
            "a non-constant @label expression must emit WS040; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn runtime_label_on_top_level_var_is_accepted() {
        // A non-constant `@label` on a top-level `var` is a DYNAMIC label — the
        // value is wired into the label's Text port at emit — so it is valid and
        // must NOT emit WS040. Both a self-reference and another var work.
        assert_no_diags(&tc("@label(hp) var hp: int = 0"));
        assert_no_diags(&tc(
            "var hp: int = 0\n@label(hp * 2) var shown: int = 0",
        ));
    }

    #[test]
    fn undefined_ref_in_var_label_errors() {
        // The runtime label is still type-checked: an undefined symbol surfaces
        // (it isn't silently swallowed just because WS040 no longer fires).
        let r = tc("@label(nope) var v: int = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "an undefined ref inside a var @label must error; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn constant_label_expr_is_accepted() {
        // A folded constant expression (a literal, or a constant `let`) is
        // valid and must not emit WS040.
        assert_no_diags(&tc("@label(1 + 2) chip { }"));
        assert_no_diags(&tc("let title = \"Score\"\n@label(title) out v: int = 0"));
    }

    #[test]
    fn non_constant_label_expr_inside_handler_is_rejected() {
        // A decl inside a top-level `on Event { ... }` handler (the standard
        // Wirescript pattern) must still be visited — a non-constant `@label`
        // there is an error, not a silent fall-back to the name.
        let r = tc(
            "in x: int\n\
             on ControllerJoined(c, id, name) {\n\
             @label(x) var y: int = 0\n\
             }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS040"),
            "a non-constant @label inside a handler must emit WS040; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn non_constant_label_expr_inside_top_level_if_is_rejected() {
        // A decl inside a top-level `if` block is likewise visited.
        let r = tc(
            "in go: exec\nin x: int\n\
             on go {\n\
             if x > 0 {\n\
             @label(x) var y: int = 0\n\
             }\n\
             }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS040"),
            "a non-constant @label inside an if must emit WS040; got {:?}",
            r.diagnostics
        );
    }

    // ---- Task 10: comprehensive generic test coverage (bounded masks +
    // max type params). Mask membership (`types::classes::class_mask`):
    // Scalar = {int, float}; Numeric = {int, float, vector, rotator, quat,
    // color}; Variant (unbounded `<T>`) = all 11 wire value variants.
    // `ws033_out_of_mask_arg_violates_bound` above already covers the plain
    // `<T: Numeric>` + `string` case; these add the Scalar/Numeric BOUNDARY
    // and the other machinery (check_args routing, body-checking, explicit
    // type args, multi-param/nested resolution) named in the task brief. ----

    #[test]
    fn numeric_bound_call_site_accepts_every_mask_member() {
        // <T: Numeric> = {int, float, vector, rotator, quat, color} — `+` has
        // an operator rule for every one of them (`catalog::operators::
        // math_binary`'s vec=true rule set), so a call at each in-mask type
        // must stay fully clean.
        assert_no_diags(&tc(
            "mod addT<T: Numeric>(a: T, b: T) -> T { return a + b }\n\
             in i: int\nin f: float\nin v: vector\nin rot: rotator\nin q: quat\nin c: color\n\
             out ri: int = addT(i, i)\n\
             out rf: float = addT(f, f)\n\
             out rv: vector = addT(v, v)\n\
             out rr: rotator = addT(rot, rot)\n\
             out rq: quat = addT(q, q)\n\
             out rc: color = addT(c, c)\n",
        ));
    }

    #[test]
    fn scalar_bound_call_site_accepts_int_and_float() {
        assert_no_diags(&tc(
            "mod addS<T: Scalar>(a: T, b: T) -> T { return a + b }\n\
             in i: int\nin f: float\n\
             out ri: int = addS(i, i)\n\
             out rf: float = addS(f, f)\n",
        ));
    }

    #[test]
    fn scalar_bound_rejects_vector_precise_boundary() {
        // vector IS a member of Numeric but NOT of Scalar — the precise
        // boundary between the two masks the task brief calls out.
        let r = tc(
            "mod onlyScalar<T: Scalar>(v: T) -> T { return v }\n\
             in vec: vector\nlet x = onlyScalar(vec)\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("isn't allowed by its bound")),
            "vector is in Numeric but not Scalar — a <T: Scalar> call with a \
             vector arg must fire WS033 OutOfMask; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn scalar_bound_rejects_bool() {
        let r = tc(
            "mod onlyScalar<T: Scalar>(v: T) -> T { return v }\n\
             in b: bool\nlet x = onlyScalar(b)\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("isn't allowed by its bound")),
            "bool is not in Scalar; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn scalar_bound_rejects_entity() {
        let r = tc(
            "mod onlyScalar<T: Scalar>(v: T) -> T { return v }\n\
             in e: entity\nlet x = onlyScalar(e)\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("isn't allowed by its bound")),
            "entity is not in Scalar; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ucc_bounded_out_of_mask_is_ws033_not_ws003() {
        // Vector is Numeric but not Scalar, so a <T: Scalar> call with a
        // vector arg fails OutOfMask inference (`subst` stays `None`). The
        // recent `check_args` routing of user mod/chip calls must keep
        // skipping the still-generic (`Type::Param`-carrying) `v: T` param
        // via its `type_has_param` guard, rather than coercing the concrete
        // vector arg against the raw unresolved `T` and emitting a spurious
        // WS003 alongside the WS033.
        let r = tc(
            "mod onlyScalar<T: Scalar>(v: T) -> T { return v }\n\
             in vec: vector\nlet x = onlyScalar(vec)\n",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS033"),
            "expected WS033: {:?}",
            r.diagnostics
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a bounded-generic OutOfMask failure must not also emit a spurious \
             WS003; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ucc_bounded_in_mask_call_is_fully_clean() {
        let r = tc(
            "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\n\
             in vec: vector\nlet x = onlyNumeric(vec)\n",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn numeric_bound_body_op_invalid_for_a_member_is_rejected() {
        // `&` (bitwise AND) only has operator rules for int/float/bool
        // operands (`catalog::operators::bitwise_binary`) — it has NO rule
        // for vector/rotator/quat/color, four of the six `Numeric` mask
        // members. The bounded body-check runs the body once per mask
        // member (not once per call site), so this must be rejected even
        // though the mod below is never called. A binary `&` with no
        // resolved overload emits WS011 (not WS004 — see the `matches!`
        // dispatch in `Expr::BinOp` inference).
        let r = tc("mod bitAnd<T: Numeric>(a: T, b: T) { let x = a & b }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS011"),
            "'&' has no overload for vector/rotator/quat/color, so the \
             Numeric body check must reject it as WS011; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn numeric_bound_body_op_valid_for_every_member_is_clean() {
        // Sanity converse: `+` DOES have a rule for every Numeric member, so
        // the same per-mask-member body check must stay clean.
        assert_no_diags(&tc(
            "mod addT<T: Numeric>(a: T, b: T) -> T { return a + b }",
        ));
    }

    #[test]
    fn explicit_type_arg_scalar_out_of_mask_is_ws033() {
        // `f<string>(...)` on a `<T: Scalar>` mod: string isn't in the
        // Scalar mask (int/float) — an explicit type argument is validated
        // against the bound exactly like inference is.
        let r = tc(
            "mod onlyScalarZero<T: Scalar>() -> T { static var z: T = z\n return z }\n\
             out r: int = onlyScalarZero<string>()\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("isn't allowed by its bound")),
            "explicit type arg out of the Scalar bound must be WS033; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn explicit_type_arg_wrong_arity_is_ws033() {
        let r = tc(
            "mod pair<A: Scalar, B: Scalar>(a: A, b: B) -> A { return a }\n\
             in x: int\nin y: float\nout r: int = pair<int>(x, y)\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("type argument")),
            "one type arg given but 2 type params declared must be WS033; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn explicit_type_arg_on_non_generic_is_ws033() {
        let r = tc("mod addOne(a: int) -> int { return a + 1 }\nin x: int\nout r: int = addOne<int>(x)\n");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("not generic")),
            "type args on a non-generic mod must be WS033; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn numeric_bound_conflict_int_vector_no_common_widening() {
        // Two `<T: Numeric>` args pin the shared T to int and vector — both
        // individually valid Numeric members, but with no common widening
        // between them, so this is a genuine Conflict (not OutOfMask).
        let r = tc(
            "mod pick2<T: Numeric>(a: T, b: T) -> T { return a }\n\
             in n: int\nin v: vector\nlet x = pick2(n, v)\n",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS033" && d.message.contains("must be the same type")),
            "int/vector conflict on a Numeric-bounded T must fire WS033 \
             Conflict; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn array_and_map_generic_params_resolve_and_check() {
        // `T[]` (Array<T> sugar) and `Map<K, V>` params both resolve their
        // type params structurally from the argument's shape — T from the
        // array's element type, V from the map's value type. The map's key
        // param is left concrete (`string`), not generic: an unbounded `K`
        // used as a map key is itself invalid (its Variant mask includes
        // `bool`/`vector`/etc., none of which are legal map-key types —
        // `WS039 "map key type must be int, string, or an object"` — and the
        // bounded body-check correctly rejects that per-combo; that's a
        // separate, orthogonal constraint from what this test targets).
        let r = tc(
            "mod combo<T, V>(xs: T[], m: Map<string, V>, fallback: T) -> T { return fallback }\n\
             var ints: int[] = [1, 2, 3]\nvar strs: Map<string, bool>\n\
             in go: exec\non go { let r = combo(ints, strs, 5) }\n",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn generic_return_type_substitutes_correctly() {
        // `identity<T>`'s return must resolve to the CONCRETE call-site type
        // (int here), not stay `Type::Param` — `+ 1` only type-checks if the
        // substitution actually happened.
        assert_no_diags(&tc(
            "mod identity<T>(v: T) -> T { return v }\n\
             in n: int\nout r: int = identity(n) + 1\n",
        ));
    }
