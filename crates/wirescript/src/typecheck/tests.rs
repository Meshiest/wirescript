    use super::*;
    use crate::parser::parse;

    fn tc(src: &str) -> TypeCheckResult {
        let p = parse(src, "test");
        assert!(
            p.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            p.diagnostics
        );
        typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
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

    /// `GetInputs` outputs must carry their real types, not `any`. A coercible
    /// annotation cannot tell the two apart (`let x: int = <float>` and
    /// `let x: int = <any>` are both accepted), so each field is checked against
    /// `vector`, which nothing here coerces to. The float axis, the bool flag,
    /// and a known-float control must all report the same way.
    #[test]
    fn get_inputs_outputs_are_typed_not_any() {
        let r = tc("in go: exec\n\
                    in pl: character\n\
                    on go {\n\
                      let inp = pl.GetInputs()\n\
                      let axis: vector = inp.Forward\n\
                      let flag: vector = inp.PressedC\n\
                    }");
        let msgs: Vec<&str> = r
            .diagnostics
            .iter()
            .filter(|d| d.code == "WS016")
            .map(|d| d.message.as_str())
            .collect();
        assert!(
            msgs.iter().any(|m| m.contains("has type float")),
            "an axis output must type as float: {:?}",
            r.diagnostics
        );
        assert!(
            msgs.iter().any(|m| m.contains("has type bool")),
            "a pressed flag must type as bool: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn get_inputs_unknown_field_is_ws010() {
        let r = tc("in go: exec\n\
                    in pl: character\n\
                    on go {\n\
                      let inp = pl.GetInputs()\n\
                      let x = inp.Jump\n\
                    }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS010"),
            "an unknown control must be WS010: {:?}",
            r.diagnostics
        );
    }

    /// It is an exec gate, so it samples where the chain reaches it. Used in
    /// pure position it would otherwise lower to a placeholder that reads a
    /// default for every control.
    #[test]
    fn get_inputs_outside_an_exec_context_is_ws007() {
        let r = tc("in pl: character\nlet inp = pl.GetInputs()");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "a pure-position GetInputs must be WS007: {:?}",
            r.diagnostics
        );
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
    fn let_tuple_and_record_annotation_checks_element_types() {
        // A `let` with a tuple/record annotation must validate its value like
        // `var`/params/returns do. Previously the `let` path checked record
        // field NAMES only and skipped tuple annotations entirely (a tuple
        // literal parses as an index-keyed record, whose expected type is a
        // `Type::Tuple` the name-check branch bailed on), so malformed elements
        // slipped through with no diagnostic.
        let ws003 = |src: &str| tc(src).diagnostics.iter().any(|d| d.code == "WS003");
        // Tuple element type mismatch + arity.
        assert!(ws003("let t: (int, int) = (\"s\", 5)"), "tuple element type");
        assert!(ws003("let t: (int, int) = (1, 2, 3)"), "tuple arity");
        // Malformed record inside a tuple (missing field / wrong field type).
        assert!(
            ws003("type Card = { baz: int }\nlet t: (Card, int) = ({}, 5)"),
            "record-in-tuple missing field"
        );
        assert!(
            ws003("type Card = { baz: int }\nlet t: (Card, int) = ({ baz: \"s\" }, 5)"),
            "record-in-tuple wrong field type"
        );
        // A record field's VALUE type is now checked (was names-only), incl. nested.
        assert!(
            ws003("type Card = { baz: int }\nlet c: Card = { baz: \"s\" }"),
            "record field value type"
        );
        assert!(
            ws003("type C = { bar: { baz: int } }\nlet c: C = { bar: { baz: \"s\" } }"),
            "nested record field type"
        );
        // Valid tuples/records stay clean; the nice missing-field message and the
        // string coercion (`let s: string = 5`) are preserved.
        assert_no_diags(&tc("let t: (int, int) = (1, 2)"));
        assert_no_diags(&tc("type Card = { baz: int }\nlet c: Card = { baz: 1 }"));
        assert_no_diags(&tc("let s: string = 5"));
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
    fn container_method_on_a_record_field_types_its_result() {
        // The array/map method arms only accepted a BARE IDENTIFIER receiver,
        // while lowering resolves a whole field chain (`resolve_field_chain`)
        // and emits a real ArrayVar gate. So `g.ready.sum()` lowered correctly
        // but typed as `any`, and arithmetic on the result reported the
        // baffling "no overload for '*' on Any, Int".
        assert_no_diags(&tc(
            "type G = { ready: int[] }\nmod f(g: G) { let n = g.ready.sum() * 4 }",
        ));
        // Maps reach through the same way.
        assert_no_diags(&tc(
            "type G = { counts: Map<int, int> }\nmod f(g: G) { let n = g.counts.get(1) + 1 }",
        ));
        // And so does a nested record.
        assert_no_diags(&tc(
            "type G = { ready: int[] }\ntype O = { g: G }\nmod f(o: O) { let n = o.g.ready.sum() * 2 }",
        ));
        // The element type survives the chain, so args are still checked.
        let r = tc("type G = { ready: int[] }\nmod f(g: G) { g.ready.push(\"nope\") }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "element type must be checked through a record field: {:?}",
            r.diagnostics
        );
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
        let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
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
    fn nested_prefab_custom_event_slot_defaults_to_float_and_warns() {
        // A self-contained prefab block runs the same two-phase inference the
        // compile path uses: an unannotated custom-event slot with no in-block
        // sender defaults to float (WS042) and its typed use surfaces WS003 —
        // in the editor / `just check`, not just at emit time. Both remap into
        // the block's outer span.
        let src =
            "in go: exec\non go { SpawnPrefab($```on CustomEvent(\"init\") -> p { let n = p.GetUserId() }```) }\n";
        let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
        let block = src.find("$```").unwrap();
        let ws042 = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS042")
            .expect("WS042 for the uninferable slot should surface");
        assert!(
            ws042.range.start.offset >= block,
            "WS042 should remap into the block span (offset {} vs block {})",
            ws042.range.start.offset,
            block
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.range.start.offset >= block),
            "the float slot's typed use should surface WS003 inside the block: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn record_fields_match_ref_insensitively() {
        // A record value's fields match an expected record type treating ref-ness
        // as an exposure mode (like params/out/assign): a `let r: T = { shorthand }`
        // built from vars exposes scalar fields as refs and array fields plainly,
        // yet still passes to a mod whose record param mixes plain arrays and ref
        // scalars. Reproduces the gba/chip8 (`*int[]` vs `int[]`) and
        // secret-hitler/2raab data-model plumbing.
        let cases = [
            // Array field via shorthand: value exposes `*int[]`, param wants `int[]`.
            "var regs: int[]\n\
             type Cpu = { regs: int[] }\n\
             mod cpu_init({ regs }: Cpu) { regs.push(0) }\n\
             on RoundStart() {\n  let cpu: Cpu = { regs }\n  cpu_init(cpu)\n}\n",
            // Scalar ref field: value exposes plain `int`, param wants `*int`.
            "var counter_a: int = 0\n\
             type State = { counter: *int, label: int }\n\
             mod bump({ counter }: State) { counter = counter + 1 }\n\
             on RoundStart() {\n  let sa: State = { counter: counter_a, label: 1 }\n  bump(sa)\n}\n",
            // Mixed: arrays plain + scalars ref, all via shorthand (the gba `Cpu`).
            "var regs: int[]\nvar cpsr: int = 0\nvar halted: bool = false\n\
             type Cpu = { regs: int[], cpsr: *int, halted: *bool }\n\
             mod cpu_init({ regs, cpsr, halted }: Cpu) { cpsr = 1 }\n\
             on RoundStart() {\n  let cpu: Cpu = { regs, cpsr, halted }\n  cpu_init(cpu)\n}\n",
        ];
        for src in cases {
            let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
            assert!(
                !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
                "ref-insensitive record field match should type-check cleanly: {:?}\nsrc:\n{src}",
                r.diagnostics
            );
        }
    }

    #[test]
    fn generic_builtins_carry_arg_type() {
        // Select/Swap/Sleep/SleepTicks resolve their `Type::Param` output to the
        // args' concrete type, not `any`. Discriminator: if the output stayed
        // `any`, none of these annotated-let mismatches (WS016) would fire, since
        // `any` matches every annotation.
        let cases = [
            ("in go: exec\non go { let v: vector = Select(true, 1, 2) }\n", "int"),
            ("in go: exec\non go { let v: vector = Select(false, \"x\", \"y\") }\n", "string"),
            (
                "in go: exec\non go { let s = Swap(true, 1, 2)\n  let v: vector = s.Output }\n",
                "int",
            ),
            ("in go: exec\non go { let v: vector = SleepTicks(5, delay = 1) }\n", "int"),
        ];
        for (src, ty) in cases {
            let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
            assert!(
                r.diagnostics.iter().any(|d| d.code == "WS016" && d.message.contains(ty)),
                "generic output should resolve to `{ty}`, not `any`: {:?}",
                r.diagnostics
            );
        }
    }

    #[test]
    fn generic_builtin_arg_conflict_is_ws033() {
        let r = tc("in c: bool\nout y = Select(c, 5, \"hello\")");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS033"),
            "conflicting Select args must error WS033: {:?}", r.diagnostics);
        // Legitimate joins MUST STILL compile clean (do not over-reject):
        assert_no_diags(&tc("in c: bool\nout y = Select(c, 1, 2)"));
        assert_no_diags(&tc("in c: bool\nvar i: int = 0\nvar f: float = 0.0\nout y = Select(c, i, f)"));
        assert_no_diags(&tc("in c: bool\nin z: zone\nlet w = Select(c, z, z)"));
    }

    #[test]
    fn find_player_returns_controller_not_character() {
        // FindPlayer emits the found player's persistent PlayerState (modeled as
        // `controller`), NOT a character — its output was mistyped `Type::Character`
        // before the PlayerState upgrade. A minimal program's only object-typed
        // expression is the call, so its inferred type must be Controller.
        let r = tc("in go: exec\non go { let p = FindPlayer(\"id\") }");
        assert_no_diags(&r);
        assert!(
            r.type_of_expr.values().any(|t| *t == Type::Controller),
            "FindPlayer must type as controller/PlayerState: {:?}",
            r.type_of_expr.values().collect::<Vec<_>>()
        );
        assert!(
            !r.type_of_expr.values().any(|t| *t == Type::Character),
            "FindPlayer must not type as character: {:?}",
            r.type_of_expr.values().collect::<Vec<_>>()
        );
    }

    #[test]
    fn swap_conflict_reports_ws033_once() {
        // Swap's output is `Record[("Output", T), ("OutputB", T)]` — both fields
        // share `T`. A conflict must be reported exactly ONCE, not once per
        // field; this guards the `resolved` memoization that makes that true.
        let r = tc("in c: bool\nout y = Swap(c, 5, \"hello\")");
        let ws033 = r.diagnostics.iter().filter(|d| d.code == "WS033").count();
        assert_eq!(ws033, 1, "Swap conflict must emit exactly one WS033: {:?}", r.diagnostics);
    }

    #[test]
    fn tween_value_output_rides_input_variant() {
        // Tween's `{ Value: <variant>, Arrived: exec }` output resolves the
        // `Value` field to the target's concrete type, not the full
        // `float|int|vector|…` union — so a float target yields a float `Value`
        // that reads as a float and formats into a string (`${tw}`).
        let src = "in target: float\n\
                   let tw = Tween(target, 1.0)\n\
                   out v: float = tw.Value\n\
                   out s: string = \"${tw}\"\n";
        let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "a float Tween's Value should be float and format to string: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn tuple_value_matches_tuple_param() {
        // A tuple literal is represented as an index-keyed record
        // (`{"0":T0,"1":T1}`), but a tuple TYPE annotation resolves to
        // `Type::Tuple([T0,T1])`. They describe the same shape and must be
        // interchangeable — passing a tuple value/literal to a mod whose param
        // is a tuple type must type-check.
        let cases = [
            "let w = (1, 2)\nmod f((a, b): (int, int)) -> int { return a + b }\nout o = f(w)\n",
            "mod f((a, b): (int, int)) -> int { return a + b }\nout o = f((1, 2))\n",
        ];
        for src in cases {
            let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
            assert!(
                !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
                "tuple value should match tuple param: {:?}\nsrc:\n{src}",
                r.diagnostics
            );
        }
    }

    #[test]
    fn tuple_value_length_mismatch_still_errors() {
        // Ref/representation-insensitivity must not accept a genuine arity or
        // element-type mismatch.
        let src = "mod f((a, b): (int, int)) -> int { return a + b }\nout o = f((1, 2, 3))\n";
        let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003" || d.code == "WS022"),
            "a 3-tuple into a 2-tuple param should error: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn array_record_field_is_never_ref_wrapped() {
        // An array is already a reference, so a shorthand `{ arr }` field (and
        // `&arr`) types as `int[]`, never the redundant, write-dropping `*int[]`.
        // A mixed record built from vars matches a `{ int[], *int }` type
        // EXACTLY (plain array field, ref scalar field) — no coercion.
        let ok = "var regs: int[]\nvar cpsr: int = 0\n\
                  type Cpu = { regs: int[], cpsr: *int }\n\
                  mod f(c: Cpu) { }\n\
                  on RoundStart() {\n  let cpu = { regs, cpsr }\n  f(cpu)\n}\n";
        let r = typecheck(&parse(ok, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "shorthand `int[]` + scalar `*int` must match `Cpu` exactly: {:?}",
            r.diagnostics
        );

        // A genuine field mismatch renders the array field as `int[]`, not `*int[]`.
        let bad = "var regs: int[]\n\
                   type Bad = { regs: string }\n\
                   mod g(b: Bad) { }\n\
                   on RoundStart() {\n  let rr = { regs }\n  g(rr)\n}\n";
        let rb = typecheck(&parse(bad, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
        let ws003 = rb
            .diagnostics
            .iter()
            .find(|d| d.code == "WS003")
            .expect("field mismatch should be WS003");
        assert!(
            ws003.message.contains("regs: int[]") && !ws003.message.contains("*int[]"),
            "array field should render as `int[]`, never `*int[]`: {}",
            ws003.message
        );
    }

    #[test]
    fn record_field_value_mismatch_still_errors() {
        // Ref-insensitivity must NOT loosen genuine value-type mismatches: an
        // `int` field where a `string` field is expected is still WS003.
        let src = "type A = { x: int }\ntype B = { x: string }\n\
                   mod take(b: B) { }\n\
                   on RoundStart() {\n  let a: A = { x: 5 }\n  take(a)\n}\n";
        let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "int-field vs string-field record should still be WS003: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn nested_handler_on_event_param_is_valid() {
        // A nested handler triggered by the enclosing handler's EVENT data param
        // — `on CustomEvent("x") -> (p: character) { on p { } }` and the negated
        // `on !p { }` — must type-check (the param is bound as EventParam).
        for src in [
            "on CustomEvent(\"x\") -> (p: character) {\n  on p { }\n}\n",
            "on CustomEvent(\"x\") -> (p: character) {\n  on !p { }\n}\n",
        ] {
            let r = typecheck(&parse(src, "test").ast, "test", &crate::typecheck::CeSlotMap::default());
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
            "on RoundStart() { var v = Vec(1.0, 2.0, 3.0)\n let w = v + v }",
        ));
    }

    #[test]
    fn var_inferred_type_catches_mismatch() {
        // Inference makes the var `int`, so assigning a vector is a real
        // WS003 — under the old `any` placeholder this passed silently.
        let r = tc("var n = 0\non RoundStart() { n = Vec(1.0, 1.0, 1.0) }");
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
            "var tint = Color(1.0, 0.0, 0.0)\non RoundStart() { tint = Color(0.0, 1.0, 0.0) }",
        ));
    }

    #[test]
    fn color_var_rejects_vector_assignment() {
        let r = tc("var tint = Color(1.0, 0.0, 0.0)\non RoundStart() { tint = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into color var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_string_in_handler_ok() {
        assert_no_diags(&tc("on RoundStart() { var x: string = \"hi\" }"));
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
        let r = tc("on RoundStart() { }");
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
        let r = tc("on CharacterDied() -> { character: c } { }");
        assert_no_diags(&r);
    }

    #[test]
    fn assignment_in_handler_ok() {
        let r = tc("var n: int = 0\non RoundStart() { n = n + 1 }");
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
        let r = tc("on RoundStart() { x = 1 }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS002"));
    }

    #[test]
    fn namespace_call_with_undefined_base_is_ws002() {
        // A namespace-qualified call whose base identifier isn't in scope — e.g.
        // an `import * as card` was removed but `card.drawLobby(...)` calls
        // remain. None of the namespace/array/receiver branches match, so
        // without an explicit check the call silently lowers to an
        // `_Unsupported` gate that does nothing at runtime.
        let r = tc("mod drawLobby(n: int) { }\non RoundStart() { card.drawLobby(1) }");
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
        let r = tc("var x: int = 0\non RoundStart() { x = 1\nreturn\nx = 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn return_in_exec_no_error() {
        let r = tc("var x: int = 0\non RoundStart() { if x > 5 { return } }");
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
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nvar err: int = 0\non RoundStart() {\n  if f != 42 { err = 1 }\n}",
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
        let r = tc("var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart() { inc(x) }");
        assert_no_diags(&r);
    }

    // ---- anonymous chip ----
    #[test]
    fn anon_chip_shares_scope() {
        let r = tc("var x: int = 0\nchip { var y: int = 0 }\non RoundStart() { x = 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn chip_on_handler() {
        let r = tc("var x: int = 0\nchip on RoundStart() { x = 1 }");
        assert_no_diags(&r);
    }

    // ---- emit ----
    #[test]
    fn emit_in_exec() {
        let r = tc("var x: int = 0\nout result = x\non RoundStart() { emit result }");
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
        let r = tc("in ch: character\non RoundStart() { ch.SetLocation(Vec(0.0, 0.0, 0.0)) }");
        assert_no_diags(&r);
    }

    // ---- call arg validation ----
    #[test]
    fn call_too_many_args() {
        let r = tc("on RoundStart() { Random(1, 2, 3, 4, 5) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
    }

    #[test]
    fn call_wrong_arg_type() {
        let r = tc("on RoundStart() { SetLocation(42, Vec(0.0, 0.0, 0.0)) }");
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
        let r = tc("in e: entity\non RoundStart() { SetLocation(e) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
    }

    #[test]
    fn receiver_call_wrong_arg_type() {
        // A receiver-method arg mismatch (`e.SetLocation(5)` — `pos` expects
        // a vector) must fire the same WS003 the plain-call form does.
        let r = tc("in e: entity\non RoundStart() { e.SetLocation(5) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("argument"))
        );
    }

    #[test]
    fn receiver_call_valid_stays_clean() {
        let r = tc("in e: entity\non RoundStart() { e.SetLocation(Vec(0.0, 0.0, 0.0)) }");
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
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
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
            "import * as u from \"util\"\non RoundStart() { u.f(5) }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "expected WS003 for namespaced call arg type mismatch: {:?}",
            r.diagnostics
        );
    }

    /// A parameter sharing a name with an `import * as` alias makes every
    /// `ns.f(...)` in that body resolve against the local value, which has no
    /// such member. That used to type as `any` and lower to an `_Unsupported`
    /// no-op with NO diagnostic, so `wirescript-check` called the file clean
    /// and the failure only showed up wherever the `any` was finally consumed,
    /// pointing at a line that was not the mistake.
    #[test]
    fn namespace_call_shadowed_by_a_local_is_ws002() {
        use crate::resolve::resolve;
        let loader = ns_util_loader();
        let resolved = resolve(
            "import * as u from \"util\"\nmod g(u: int) { u.f(Vec(0.0, 0.0, 0.0)) }\non RoundStart() { g(1) }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"),
            "a local shadowing a namespace must emit WS002: {:?}",
            r.diagnostics
        );
    }

    /// The guard above must not fire on the ordinary case: same call, no
    /// shadowing parameter.
    #[test]
    fn namespace_call_without_shadowing_is_clean() {
        use crate::resolve::resolve;
        let loader = ns_util_loader();
        let resolved = resolve(
            "import * as u from \"util\"\nmod g(v: int) { u.f(Vec(0.0, 0.0, 0.0)) }\non RoundStart() { g(1) }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS002"),
            "an unshadowed namespace call must not emit WS002: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn namespace_call_wrong_arity_is_ws011() {
        use crate::resolve::resolve;
        let loader = ns_util_loader();
        let resolved = resolve(
            "import * as u from \"util\"\non RoundStart() { u.f() }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
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
            "import * as u from \"util\"\non RoundStart() { u.f(Vec(0.0, 0.0, 0.0)) }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
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
            "import * as g from \"gutil\"\non RoundStart() { }",
            "main.ws",
            &loader,
        );
        let r = typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
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
        let r = tc("var ctrl: controller\non RoundStart() { ctrl.DisplayText(\"hi\") }");
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

    // ---- map index ----
    #[test]
    fn map_index_returns_value_type() {
        // Map subscript reads require exec context (compile to
        // Exec_MapVar_Get) and type to the map's VALUE type.
        let r = tc(
            "var m: Map<int, int>\nin trigger: exec\non trigger { let x: int = m[1]\nlet ok = x + 1 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn map_index_outside_exec_is_ws007() {
        // Map index read in pure context should emit WS007, mirroring arrays.
        let r = tc("var m: Map<int, int>\nlet x = m[1]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "expected WS007 for map index read outside exec context"
        );
    }

    // ---- module-level const index (narrows WS007) ----
    //
    // A runtime array/map read needs an exec-context gate — an out-of-range
    // runtime read keeps the gate's stale PREVIOUS value, which is why WS007
    // is strict about it. A compile-time constant index into a compile-time
    // constant array/map emits no gate at all, so that reasoning does not
    // apply and WS007 must not fire. This narrows the rule; it must not
    // remove it (see the guard-rail tests below).

    #[test]
    fn a_const_index_into_a_const_array_is_allowed_at_module_level() {
        let r = tc("const t = [10, 20]\nconst z = t[1]");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS007"),
            "a compile-time index into a compile-time array must not raise WS007: {:?}",
            r.diagnostics
        );
    }

    // NOTE: asserts only the ABSENCE OF WS007, not that this program compiles.
    // A bare `const`/`let`-bound map literal always fails the separate,
    // pre-existing `WS026 "a map literal must initialize or assign a Map
    // variable"` (only a `var`/`map` initializer or an assignment RHS reaches
    // `check_map_literal` and skips that arm), and WS026 is an ERROR — so this
    // shape never reaches emit at all. This is NOT end-to-end permission for
    // const map indexing; it pins the WS007 half of the narrowing only.
    #[test]
    fn a_const_index_into_a_const_map_is_allowed_at_module_level() {
        let r = tc("const m = { :k => 5 }\nconst z = m[:k]");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS007"),
            "a compile-time index into a compile-time map must not raise WS007: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn a_runtime_index_into_a_const_array_is_still_ws007() {
        // The RECEIVER being `const` doesn't make the read compile-time — the
        // whole index expression must fold. A runtime index still needs an
        // exec-context Array gate, so this must stay WS007.
        let r = tc("const t = [10, 20]\nvar someVar: int\nlet x = t[someVar]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "a runtime index into a const array must still be WS007: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn a_const_index_into_a_runtime_array_is_still_ws007() {
        let r = tc("var runtimeArr: int[]\nlet x = runtimeArr[1]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "a const index into a runtime array must still be WS007: {:?}",
            r.diagnostics
        );
    }

    // A `const` PARAMETER's value inside its own mod body is a type-shaped
    // PLACEHOLDER (a zero seeded once, before any call site exists — see
    // `scoped_const_placeholders`), not the real call-site value. Deciding
    // "is this index compile-time" against a context that still contains the
    // placeholder would fold `t[m]` using the fictional zero and suppress
    // WS007 for a read whose real (call-site) index is unknown at this point
    // in the pass — a silent miscompile in the making. The nested anon
    // `chip {}` puts these statements in PURE context (mirrors
    // `a_const_destructure_derived_from_a_placeholder_never_decides_a_branch`)
    // even though the enclosing mod body itself is exec-checked, so WS007 is
    // reachable here at all.
    #[test]
    fn a_const_index_using_a_placeholder_value_is_still_ws007() {
        let r = tc(
            "mod f(m: const int) {\n\
               chip {\n\
                 const t = [10, 20]\n\
                 const z = t[m]\n\
               }\n\
             }\n\
             in go: exec\non go { f(1) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "a placeholder-derived index must not silently suppress WS007: {:?}",
            r.diagnostics
        );
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

    // `...rest` on a plain (NON-const) `let`. `bind_let` is shared by the
    // const and runtime paths, and typing `rest` as the record of the
    // unconsumed fields (rather than the old `Type::Any`) widened BOTH — but
    // only the const path had coverage, so these pin the runtime surface the
    // change also serves. Both shapes below were previously silent: `Any`
    // satisfies every parameter and every field access, so a genuine mistake
    // type-checked clean and only failed (or silently misbehaved) later.
    #[test]
    fn a_runtime_let_rest_carries_the_unconsumed_field_types() {
        // The positive direction: the remaining fields keep their real types,
        // so arithmetic on them resolves (under `Type::Any` this produced
        // "no overload for '+' on Any, Any").
        assert_no_diags(&tc(
            "type P = { x: int, y: int, z: int }\n\
             let p: P = { x: 1, y: 2, z: 3 }\n\
             let { x, ...rest } = p\n\
             let sum = rest.y + rest.z",
        ));
    }

    #[test]
    fn a_runtime_let_rest_is_not_the_whole_record() {
        // Passing a `{y, z}` rest where the FULL `{x, y, z}` record is
        // expected must be rejected — the rest is missing `x`.
        let r = tc(
            "type P = { x: int, y: int, z: int }\n\
             mod takesP(p: P) -> int { return p.x }\n\
             let p: P = { x: 1, y: 2, z: 3 }\n\
             let { x, ...rest } = p\n\
             let bad = takesP(rest)",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a rest missing a field must not satisfy the full record type: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn a_runtime_let_rest_rejects_an_unknown_field() {
        // Reading a field that is not in the rest (here the consumed `x`) is
        // now a real error instead of silently typing as `any`.
        let r = tc(
            "type P = { x: int, y: int }\n\
             let p: P = { x: 1, y: 2 }\n\
             let { x, ...rest } = p\n\
             let bad = rest.x",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS010" || d.code == "WS003"),
            "reading a consumed/absent field off a rest must error: {:?}",
            r.diagnostics
        );
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
    fn no_receiver_builtin_called_with_receiver_is_ws036() {
        // `Sweep`/`SweepSimple` take no receiver — they act on their own brick.
        // `body.SweepSimple(...)` has nowhere to bind `body`, so before this it
        // silently typed as `any` and lowered to an `_Unsupported` placeholder.
        // It must be a hard WS036 error instead.
        let r = tc(
            "in body: character\nin go: exec\n\
             on go { let hit = body.SweepSimple(500.0, detectPlayers1 = true) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS036"),
            "a no-receiver builtin called with method syntax must emit WS036; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn no_receiver_builtin_plain_call_stays_clean() {
        // The correct form — no receiver, positional distance — type-checks and
        // yields the hit record (so WS036 above isn't over-firing on the gate).
        let r = tc(
            "in go: exec\n\
             on go { let hit = SweepSimple(500.0, detectPlayers1 = true)\n let d = hit.HitDistance }",
        );
        assert!(
            r.diagnostics.is_empty(),
            "plain SweepSimple(...) must type-check clean; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn prefab_ref_wrong_extension_is_ws019() {
        // A `$./…` prefab reference must be a `.brz` archive or a `.ws` source;
        // anything else is WS019.
        let r = tc("in go: exec\non go { SpawnPrefab(prefab = $./x.png) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS019"),
            "a non-.brz/.ws prefab reference must be WS019; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn ws_source_prefab_ref_surfaces_inner_errors_on_span() {
        // `$./child.ws` is a SOURCE prefab: the type-checker reads + checks the
        // referenced file and surfaces its diagnostics on the reference span, so
        // a broken child underlines `$./child.ws` in the parent (not a silent
        // failure discovered only at emit).
        let dir = std::env::temp_dir().join(format!("ws_prefab_ref_span_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("broken_child.ws"),
            "in go: exec\non go { let x = undefinedThing + 1 }",
        )
        .unwrap();
        let parent_path = dir.join("parent.ws").to_string_lossy().into_owned();
        let src = "in go: exec\non go { SpawnPrefab(prefab = $./broken_child.ws) }";
        let p = parse(src, &parent_path);
        let result = crate::typecheck::typecheck_with_inference(&p.ast, &parent_path).0;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == "WS002" && d.message.contains("in prefab")),
            "a broken $./child.ws must surface its WS002 on the reference span; got {:?}",
            result.diagnostics
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
             on ControllerJoined() -> { controller: c, userId: id, userName: name } {\n\
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

    #[test]
    fn infer_ce_slots_adopts_sender_type_and_warns_when_absent() {
        // A receiver with unannotated `amount`; an in-unit send of an int → amount:int.
        let src = "static var last: int = 0\n\
                   in go: exec\n\
                   on CustomEvent(\"dmg\") -> (amount) {\n  last = last + 1\n}\n\
                   on go {\n  SendCustomEvent(\"dmg\", 42)\n}\n";
        let p = parse(src, "test");
        assert!(p.diagnostics.is_empty(), "parse: {:?}", p.diagnostics);
        let pass1 = typecheck(&p.ast, "test", &CeSlotMap::default());
        let (map, diags) = infer_custom_event_slots(&p.ast, &pass1.type_of_expr);
        // The single receiver's slot 0 resolved to int.
        let slots = map.values().next().expect("one custom-event receiver");
        assert_eq!(slots.get(0).cloned().flatten(), Some(Type::Int));
        assert!(diags.is_empty(), "no WS042 when a sender exists: {:?}", diags);

        // No in-unit sender → float + WS042.
        let src2 = "static var last: int = 0\n\
                    on CustomEvent(\"ext\") -> (amount) {\n  last = last + 1\n}\n";
        let p2 = parse(src2, "test");
        let pass1b = typecheck(&p2.ast, "test", &CeSlotMap::default());
        let (map2, diags2) = infer_custom_event_slots(&p2.ast, &pass1b.type_of_expr);
        let slots2 = map2.values().next().expect("one receiver");
        assert_eq!(slots2.get(0).cloned().flatten(), Some(Type::Float));
        assert!(diags2.iter().any(|d| d.code == "WS042"), "expected WS042: {:?}", diags2);
    }

    #[test]
    fn inferred_ce_slot_type_is_enforced_in_body() {
        // `amount` inferred as int from the sender; using it where a vector is
        // required is a WS003 (type error) — proving the body sees `int`, not
        // `any` (which coerces to everything, including vector — `int` doesn't:
        // numeric-to-vector has no coercion rule, see `types::coerce::coerce`).
        let src = "static var v: vector = Vec(0.0, 0.0, 0.0)\n\
                   in go: exec\n\
                   on CustomEvent(\"dmg\") -> (amount) {\n  v = amount\n}\n\
                   on go {\n  SendCustomEvent(\"dmg\", 42)\n}\n";
        let p = parse(src, "test");
        let (r, _map) = typecheck_with_inference(&p.ast, "test");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS003"),
            "assigning int `amount` to vector var must be WS003: {:?}", r.diagnostics);
        // And WS029 is gone entirely.
        assert!(!r.diagnostics.iter().any(|d| d.code == "WS029"),
            "WS029 is removed: {:?}", r.diagnostics);
    }

    #[test]
    fn uninferable_ce_slot_warns_ws042_not_ws029() {
        let src = "static var n: int = 0\non CustomEvent(\"ext\") -> (amount) {\n  n = n + 1\n}\n";
        let p = parse(src, "test");
        let (r, _map) = typecheck_with_inference(&p.ast, "test");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS042"), "{:?}", r.diagnostics);
        assert!(!r.diagnostics.iter().any(|d| d.code == "WS029"), "{:?}", r.diagnostics);
    }

    #[test]
    fn statement_out_annotation_is_type_checked() {
        let r = tc("chip { out x: int = \"s\" }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "annotated out value must be checked: {:?}",
            r.diagnostics
        );
        assert_no_diags(&tc("chip { out x: int = 5 }")); // ok
        assert_no_diags(&tc("chip { out s: string = 5 }")); // ViaString, still clean
    }

    // ---- Task 4: return / emit / unannotated out checked against declared outputs ----

    #[test]
    fn return_value_checked_against_declared_output() {
        let r = tc("mod f() -> (r: int) { return \"hello\" }\nout y = f()");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "return must be checked vs declared output: {:?}",
            r.diagnostics
        );
        assert_no_diags(&tc("mod f() -> (r: int) { return 7 }\nout y = f()"));
    }

    #[test]
    fn emit_payload_checked_against_output() {
        let r = tc("out o: int\nin go: exec\non go { emit o = \"nope\" }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "emit payload must be checked vs output type: {:?}",
            r.diagnostics
        );
        assert_no_diags(&tc("out o: int\nin go: exec\non go { emit o = 7 }"));
        // A local exec signal (not an output) must NOT be checked/rejected:
        assert_no_diags(&tc("in go: exec\non go { let s: exec\n emit s = 7 }"));
    }

    #[test]
    fn unannotated_out_checked_against_declared_output() {
        // named chip whose signature declares `r: int`; body does `out r = "s"`.
        let r = tc("chip Foo() -> (r: int) { out r = \"s\" }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "unannotated out must be checked vs declared output: {:?}",
            r.diagnostics
        );
        assert_no_diags(&tc("chip Foo() -> (r: int) { out r = 5 }"));
    }

    #[test]
    fn return_in_handler_with_single_output_is_checked() {
        // Lowering wires `return <value>` into the single enclosing output
        // (`output_count() == 1`) even from a top-level handler — so a
        // wrong-typed value must still error (it would otherwise wire a string
        // straight into an int output's port).
        let r = tc("out o: int\nin go: exec\nvar flag: bool = false\non go { if flag { return \"bail\" } }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "return of wrong type in a single-output handler must error: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn return_not_checked_when_more_than_one_output_exists() {
        // With >1 total output, lowering DROPS a bare `return <value>`
        // (`output_count() != 1`), so it must not be checked — even if exactly
        // one of the outputs is annotated. The module frame counts ALL outs
        // (unannotated as `Any`) so its length matches `output_count()`.
        assert_no_diags(&tc(
            "out o: int\nout y = 5\nin go: exec\nvar flag: bool = false\n\
             on go { if flag { return \"bail\" } }",
        ));
    }

    #[test]
    fn out_to_output_shadowed_by_same_named_local_is_checked() {
        // A local `var r` sharing the output's name does NOT shadow the output:
        // `out r = v` lowering resolves the output via `lookup_output` (a
        // disjoint namespace), so a wrong-typed value still wires into the
        // int output and must error. (Guards against a bogus "local shadows
        // output → skip check" rule.)
        let r = tc("chip Foo() -> (r: int) { var r: int = 0\n out r = \"bad\" }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "out to an output shadowed by a same-named local must still be checked: {:?}",
            r.diagnostics
        );
        // ...and the idiomatic same-type case (`out r = r`) stays clean.
        assert_no_diags(&tc("chip Foo() -> (r: int) { var r: int = 0\n out r = r }"));
    }

    #[test]
    fn emit_to_output_shadowed_by_same_named_local_is_checked() {
        // Same disjoint-namespace rule for `emit`: `emit o = go` (go is exec)
        // wires into the int output `o` even with a same-named `let o: exec`,
        // so the type mismatch must error.
        let r = tc("out o: int\nin go: exec\non go { let o: exec\n emit o = go }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "emit to an output shadowed by a same-named local must still be checked: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn module_output_frame_build_does_not_double_report_unknown_type() {
        // Building the module-level `out_ctx` frame calls `resolve_type_expr`,
        // which emits WS002 for an unknown type — but that annotation is ALSO
        // resolved+reported by the canonical `TopDecl::Out` check, so the frame
        // build must discard its own diagnostics. Exactly ONE WS002.
        let r = tc("out o: BadType = 0");
        assert_eq!(
            r.diagnostics.iter().filter(|d| d.code == "WS002").count(),
            1,
            "bad module output type must be reported exactly once: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn chip_output_frame_build_does_not_double_report_unknown_type() {
        // Same for the per-combo chip output frame: `register_decl` already
        // resolves+reports the chip's output annotations, so the frame build in
        // the body-check loop must discard its own. Exactly ONE WS002.
        let r = tc("chip Foo() -> (r: BadType) {\n out r = 0\n}\nlet z = Foo()");
        assert_eq!(
            r.diagnostics.iter().filter(|d| d.code == "WS002").count(),
            1,
            "bad chip output type must be reported exactly once: {:?}",
            r.diagnostics
        );
    }

    // P0-16c: a scalar has no fields, so a field access on one is a typo — it
    // used to type `any` and lowering read the whole base value. The single
    // exception is projecting a single-output call result by its output name.
    #[test]
    fn unknown_field_on_a_scalar_is_ws010() {
        let r = tc("in go: exec\nvar a: int = 5\nvar b: int = 0\non go {\n  let c = a\n  b = c.whatever\n}");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS010"),
            "a field on an int must be WS010, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn single_output_projection_and_its_typo() {
        // Projecting by the declared output name stays legal...
        assert_no_diags(&tc(
            "chip Foo(x: int) -> (result: int) {\n out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f.result\nout o = ok",
        ));
        // ...and so does an aliased re-binding of that result.
        assert_no_diags(&tc(
            "chip Foo(x: int) -> (result: int) {\n out result = x * 2\n}\nlet f = Foo(21)\nlet g = f\nlet ok = g.result\nout o = ok",
        ));
        // But a mis-typed output name is caught.
        let bad = tc(
            "chip Foo(x: int) -> (result: int) {\n out result = x * 2\n}\nlet f = Foo(21)\nlet bad = f.reslt\nout o = bad",
        );
        assert!(
            bad.diagnostics.iter().any(|d| d.code == "WS010"),
            "a mis-typed output name must be WS010, got {:?}",
            bad.diagnostics
        );
    }

    // P0-9: a typo'd event config/input arg matches no slot and silently no-ops
    // at lowering — flag it WS041 (`on Clock(intreval = 2.0)`).
    #[test]
    fn unknown_event_config_arg_is_ws041() {
        let r = tc("on Clock(intreval = 2.0) { BroadcastChatMessage(\"t\") }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS041"),
            "an unknown event config arg must be WS041, got {:?}",
            r.diagnostics
        );
        // The real config name still validates without WS041.
        let ok = tc("on Clock(enabled = true) { BroadcastChatMessage(\"t\") }");
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS041"),
            "a real config name must not be flagged unknown, got {:?}",
            ok.diagnostics
        );
    }

    // P0-13: `let r: rotator = c.GetVelocity()` unwraps a `{Vector, Rotation}`
    // record — lowering wires outputs[0] (Vector) into a rotator sink, so it must
    // be rejected, not silently miscompiled.
    #[test]
    fn record_unwrap_to_wrong_scalar_is_rejected() {
        let r = tc(
            "on CharacterSpawned() -> (c) {\n  let rot: rotator = c.GetVelocity()\n  c.SetRotation(rot)\n}",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error),
            "a record unwrapping to a non-first field must error, got {:?}",
            r.diagnostics
        );
    }

    // P0-14: assigning to a scalar `let` binding type-checked clean then emitted
    // no gate (the write vanished) — flag it WS007.
    #[test]
    fn assign_to_scalar_let_is_ws007() {
        let r = tc("in start: exec\nvar x: int = 3\nlet y = x + 1\non start { y = 5 }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "assigning a scalar `let` must be WS007, got {:?}",
            r.diagnostics
        );
        // An array `let` (reference-backed) stays writable through its methods.
        let ok = tc("in start: exec\nvar xs: int[] = []\nlet ys = xs\non start { ys.push(1) }");
        assert!(
            !ok.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "an array `let` must stay usable, got {:?}",
            ok.diagnostics
        );
    }

    // P0-16a: a tuple literal must never scalar-unwrap (`let x: int = (1, "abc")`).
    #[test]
    fn tuple_literal_as_scalar_is_ws003() {
        let r = tc("let x: int = (1, \"abc\")\nout o = x");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a tuple used as a scalar must be WS003, got {:?}",
            r.diagnostics
        );
    }

    // P0-16b: a heterogeneous array literal in an exec-context assignment was
    // typed from element 0 and pushed the odd element with no check.
    #[test]
    fn heterogeneous_array_literal_is_ws003() {
        let r = tc("in start: exec\nvar xs: int[] = []\non start { xs = [1, \"hello\", 2] }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a heterogeneous array literal must be WS003, got {:?}",
            r.diagnostics
        );
        // A homogeneous (numeric-coercible) literal still passes.
        let ok = tc("in start: exec\nvar xs: int[] = []\non start { xs = [1, 2, 3] }");
        assert!(
            !ok.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "a homogeneous array literal must pass, got {:?}",
            ok.diagnostics
        );
    }

    // P0-17a: `==`/`!=` on the certified composite value variants
    // (vector/rotator/quat/color) must be accepted; ordering stays scalar-only.
    #[test]
    fn composite_equality_is_accepted_ordering_is_not() {
        assert_no_diags(&tc(
            "let a = Vec(1.0, 2.0, 3.0)\nlet b = Vec(1.0, 2.0, 3.0)\nout eq = a == b",
        ));
        assert_no_diags(&tc(
            "let a = Color(1.0, 0.0, 0.0, 1.0)\nlet b = Color(0.0, 1.0, 0.0, 1.0)\nout ne = a != b",
        ));
        // Ordering on vectors has no certified gate — still WS004.
        let lt = tc("let a = Vec(1.0, 2.0, 3.0)\nlet b = Vec(1.0, 2.0, 3.0)\nout o = a < b");
        assert!(
            lt.diagnostics.iter().any(|d| d.code == "WS004"),
            "vector ordering must stay rejected, got {:?}",
            lt.diagnostics
        );
    }

    // P0-17b: an `any[]` parameter must accept a concrete array argument.
    #[test]
    fn any_array_param_accepts_concrete_array() {
        let r = tc(
            "in start: exec\nmod firstlen(xs: any[]) { BroadcastChatMessage(\"x\") }\nvar ns: int[] = []\non start { firstlen(ns) }",
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "an any[] param must accept a concrete int[] arg, got {:?}",
            r.diagnostics
        );
    }

    // Task 5: `const` is a GUARANTEE — a `const` binding whose initializer
    // cannot be evaluated at compile time is WS046. A `let` stays opportunistic.
    #[test]
    fn a_const_binding_must_evaluate_at_compile_time() {
        assert_no_diags(&tc("mod f() { const n = 1 << 4 }"));
        assert_no_diags(&tc("const TOP = 2 * 21"));

        let r = tc("in live: int\nmod f() { const n = live + 1 }");
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS046")
            .unwrap_or_else(|| panic!("expected WS046, got {:?}", r.diagnostics));
        assert!(d.message.contains("live"), "must name the runtime value: {}", d.message);
    }

    #[test]
    fn a_let_binding_that_cannot_fold_is_still_fine() {
        // `let` stays opportunistic — this is the whole difference from `const`.
        assert_no_diags(&tc("in live: int\nmod f() { let n = live + 1 }"));
    }

    #[test]
    fn a_destructuring_const_binding_is_rejected() {
        // `someRecord` is undefined — this stays WS046, but now via
        // `ConstReason::NotConstant` (a runtime value) rather than the old
        // blanket "destructuring can't be a compile-time constant" rejection
        // Task 2 removed. See the tests below for the case this used to
        // reject wrongly: a destructure of a value that genuinely IS
        // constant.
        let r = tc("mod f() { const { a, b } = someRecord }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS046"),
            "expected WS046, got {:?}",
            r.diagnostics
        );
    }

    // Task 2: compile-time record destructuring. `typecheck::decl`'s
    // top-level `const` and `typecheck::stmt`'s block-scope `const` used to
    // share one message rejecting EVERY destructuring `const` outright; both
    // are separate call sites into `const_eval::bind_destructured` now, so
    // each gets its own coverage below.
    //
    // A top-level destructured name must be a compile-time constant
    // EVERYWHERE a plain `const x = 1` is — which means it has to reach
    // `ctx.const_env`/`ctx.const_declared`, built before pass 2 by
    // `lower::predeclare::build_const_env`/`build_const_declared_names`.
    // Accepting the source without that (compiling clean while silently
    // producing non-const bindings) is the exact "looks supported but isn't"
    // failure this wave exists to remove, so each form below is proven
    // through a CONSUMER of const-ness, not just through absence of errors.

    #[test]
    fn a_const_record_binding_destructures_by_name() {
        // Distinguishable per-field values, each checked against its OWN
        // expected number: if x and y were bound to each other's fields both
        // conditions flip, the ill-typed `else` branches survive instead of
        // being elided, and WS003 fires.
        let r = tc(
            "const p = { x: 111, y: 222 }\n\
             const { x, y } = p\n\
             var flag: int = 0\n\
             in go: exec\n\
             on go {\n\
               if x == 111 { flag = 1 } else { flag = \"wrong x\" }\n\
               if y == 222 { flag = 2 } else { flag = \"wrong y\" }\n\
             }",
        );
        assert_no_diags(&r);
        assert_eq!(
            r.dropped_ranges.len(), 2,
            "a top-level destructured const must tree-shake a const `if` like \
             any other const: {:?}", r.dropped_ranges
        );
    }

    #[test]
    fn a_const_record_binding_destructures_with_an_alias() {
        let r = tc(
            "const p = { x: 111, y: 222 }\n\
             const { x: a, y: b } = p\n\
             var flag: int = 0\n\
             in go: exec\n\
             on go {\n\
               if a == 111 { flag = 1 } else { flag = \"wrong a\" }\n\
               if b == 222 { flag = 2 } else { flag = \"wrong b\" }\n\
             }",
        );
        assert_no_diags(&r);
        assert_eq!(r.dropped_ranges.len(), 2, "{:?}", r.dropped_ranges);
    }

    #[test]
    fn a_const_record_binding_destructures_with_a_rest() {
        let r = tc(
            "const p = { x: 111, y: 222, z: 333 }\n\
             const { x, ...rest } = p\n\
             var flag: int = 0\n\
             in go: exec\n\
             on go {\n\
               if x == 111 { flag = 1 } else { flag = \"wrong x\" }\n\
               if rest.y == 222 { flag = 2 } else { flag = \"wrong rest.y\" }\n\
               if rest.z == 333 { flag = 3 } else { flag = \"wrong rest.z\" }\n\
             }",
        );
        assert_no_diags(&r);
        assert_eq!(r.dropped_ranges.len(), 3, "{:?}", r.dropped_ranges);
    }

    // The two cases the fix round called out by name. Both were BROKEN before
    // it: `build_const_env`/`build_const_declared_names` skipped every
    // non-`Ident` binding, so `x` reached neither, and a top-level
    // `const { x, y } = p` compiled clean while producing bindings that were
    // not compile-time constants at all.
    #[test]
    fn a_later_top_level_const_can_read_a_destructured_const_name() {
        // Per-field distinguishable values AND a distinguishable combination
        // (111*1000 + 222): a swapped binding yields 222111, a dropped one
        // fails to evaluate at all.
        let r = tc(
            "const p = { x: 111, y: 222 }\n\
             const { x, y } = p\n\
             const combined = x * 1000 + y\n\
             var flag: int = 0\n\
             in go: exec\n\
             on go { if combined == 111222 { flag = 1 } else { flag = \"wrong\" } }",
        );
        assert_no_diags(&r);
        assert_eq!(
            r.dropped_ranges.len(), 1,
            "`combined` must itself be const (it is built from two \
             destructured const names): {:?}", r.dropped_ranges
        );
    }

    // Declaration-ORDER independence of the destructuring fixpoint is proven
    // where the crate's existing fixpoint tests live —
    // `lower::tests::const_init::a_destructured_constant_chain_resolves_regardless_of_order`
    // — via the baked-value path, not here: a top-level FORWARD reference is
    // WS002 ("unknown identifier") in typecheck regardless of destructuring
    // (a plain `const p = …` read before its own declaration is rejected the
    // same way), because scope registration walks decls in source order while
    // `build_const_env` is order-independent. That split predates this task
    // and is unchanged by it.

    // A destructured const name in a constant-only GATE CONFIG slot (the
    // WS028 surface) and as a custom-event CHANNEL NAME — the other two
    // places the fix round named. A non-const binding is rejected in both.
    #[test]
    fn a_destructured_const_name_satisfies_constant_only_positions() {
        assert_no_diags(&tc(
            "const cfg = { rate: 1.0, chan: \"evt_died\" }\n\
             const { rate, chan } = cfg\n\
             var n: int = 0\n\
             on Clock(interval = rate, enabled = true) { n = n + 1 }",
        ));
        assert_no_diags(&tc(
            "const cfg = { chan: \"evt_died\" }\n\
             const { chan } = cfg\n\
             mod ping(v: int) { SendCustomEvent(chan, v) }",
        ));
    }

    // The block-scope equivalents, INSIDE an ordinary mod body.
    //
    // These deliberately do NOT prove the binding via an ill-typed `else` on
    // a const `if`. That idiom is circular here: a correct const binding
    // ELIDES the `else`, so "no WS003" is equally consistent with the branch
    // being correctly skipped and with it being wrongly skipped while
    // lowering still emits it — which is exactly the silent miscompile fix
    // round 2 found (an earlier version of these three tests passed only
    // because typecheck under-checked a block lowering then emitted).
    //
    // Instead each destructured name is fed to a CONSTANT-ONLY position
    // (`SendCustomEvent`'s channel name, WS028/WS046 if it is not a genuine
    // compile-time constant) and compared against a distinguishable expected
    // value with `==` on a `const`, so a wrong-field binding changes the
    // computed value rather than merely changing which branch is skipped.
    // Branch-elision parity itself is proven separately and structurally by
    // `typecheck_and_lowering_drop_exactly_the_same_ranges`.

    #[test]
    fn a_block_scope_const_destructure_with_an_alias_binds_to_the_alias() {
        // `a * 1000 + b` is 111222 only if the alias `a` took field `x` and
        // `b` took `y`; a swap yields 222111, and either way the value must
        // be a real constant or the `const` binding itself is WS046.
        assert_no_diags(&tc(
            "mod f() {\n\
               const p = { x: 111, y: 222 }\n\
               const { x: a, y: b } = p\n\
               const combined = a * 1000 + b\n\
               SendCustomEvent(\"evt\", combined)\n\
             }\n\
             in go: exec\non go { f() }",
        ));
        // The negative half: if the alias did NOT rebind, `x` would still be
        // in scope and this would compile.
        let stale = tc(
            "mod f() {\n\
               const p = { x: 111, y: 222 }\n\
               const { x: a, y: b } = p\n\
               const bad = x + a + b\n\
             }\n\
             in go: exec\non go { f() }",
        );
        assert!(
            stale.diagnostics.iter().any(|d| d.code == "WS002" || d.code == "WS046"),
            "an alias must bind ONLY the alias name, leaving `x` unbound: {:?}",
            stale.diagnostics
        );
    }

    #[test]
    fn a_block_scope_const_destructure_with_a_rest_collects_remaining_fields() {
        // `rest` must carry y and z with their own values, and must be a
        // genuine constant (the `const` bindings below are WS046 otherwise).
        assert_no_diags(&tc(
            "mod f() {\n\
               const p = { x: 111, y: 222, z: 333 }\n\
               const { x, ...rest } = p\n\
               const combined = x * 1000000 + rest.y * 1000 + rest.z\n\
               SendCustomEvent(\"evt\", combined)\n\
             }\n\
             in go: exec\non go { f() }",
        ));

        // `rest` must NOT include the field `x` already consumed by its own
        // `Named` binding.
        let excluded = tc(
            "mod f() {\n\
               const p = { x: 111, y: 222 }\n\
               const { x, ...rest } = p\n\
               const bad = rest.x\n\
             }\n\
             in go: exec\non go { f() }",
        );
        assert!(
            excluded
                .diagnostics
                .iter()
                .any(|d| d.code == "WS046" || d.code == "WS010"),
            "rest must not include the already-consumed field x: {:?}",
            excluded.diagnostics
        );
    }

    #[test]
    fn a_const_destructure_inside_a_const_mod_body_binds() {
        // `typecheck::stmt`'s block-scope site (a DIFFERENT code path from
        // `typecheck::decl`'s top-level site above) type-checks a
        // destructuring `const` inside a `const mod`'s BODY clean, and
        // calling the mod from a top-level `const` also forces
        // `const_eval::interp`'s own destructuring site to run.
        assert_no_diags(&tc(
            "const mod f() -> int {\n\
               const p = { x: 3, y: 4 }\n\
               const { x, y } = p\n\
               return x * 100 + y\n\
             }\n\
             const total = f()",
        ));
    }

    #[test]
    fn a_const_destructure_naming_a_missing_field_is_ws046() {
        let r = tc("const p = { x: 1 }\nconst { x, missing } = p");
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS046")
            .unwrap_or_else(|| panic!("expected WS046, got {:?}", r.diagnostics));
        assert!(
            d.message.contains("missing"),
            "must name the missing field: {}", d.message
        );
    }

    #[test]
    fn a_const_destructure_of_a_non_record_is_ws046() {
        let r = tc("const { x } = 1");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS046"),
            "expected WS046, got {:?}",
            r.diagnostics
        );
    }

    // stmt.rs's block-scope site, exercised WITHOUT the const-mod machinery
    // interp.rs also touches — proves stmt.rs's own wiring independently of
    // the top-level and const-mod-body sites above. Same non-circular idiom
    // as the alias/rest tests just above (see their header comment): the
    // bound values are compared through a constant-only position, not through
    // an ill-typed `else` that a correct binding would elide anyway.
    #[test]
    fn a_block_scope_const_destructure_binds_inside_an_ordinary_mod_body() {
        assert_no_diags(&tc(
            "mod f() {\n\
               const p = { x: 111, y: 222 }\n\
               const { x, y } = p\n\
               const combined = x * 1000 + y\n\
               SendCustomEvent(\"evt\", combined)\n\
             }\n\
             in go: exec\non go { f() }",
        ));
    }

    /// A **non-`const`** destructuring `let` must NOT be treated as a
    /// compile-time constant, in any of the shapes that reach a
    /// constant-only config slot.
    ///
    /// This is the WS028 that correctly rejected these programs before
    /// destructuring `const` existed. Recording a plain `let`'s destructured
    /// names as constants made typecheck accept them while lowering — whose
    /// non-`const` path is the narrow evaluator, which cannot fold the record
    /// literal being destructured — silently dropped the value, shipping a
    /// `SendCustomEvent` gate with an EMPTY channel name and no diagnostic.
    ///
    /// The positive counterparts (the same programs spelled `const`, asserted
    /// to actually BAKE) live in `lower::tests::const_params`, because the bug
    /// is invisible in typecheck output and in the IR dump alike — only the
    /// baked component data distinguishes them.
    #[test]
    fn a_non_const_destructuring_let_is_not_constant_config() {
        for (label, src) in [
            (
                "plain destructure in a mod body",
                "mod f(v: int) {\n\
                   let { chan } = { chan: \"evt\" }\n\
                   SendCustomEvent(chan, v)\n\
                 }\n\
                 in go: exec\non go { f(1) }",
            ),
            (
                "alias form",
                "mod f(v: int) {\n\
                   let { chan: c2 } = { chan: \"evt\" }\n\
                   SendCustomEvent(c2, v)\n\
                 }\n\
                 in go: exec\non go { f(1) }",
            ),
            (
                "handler body",
                "in go: exec\nvar hp: int = 0\n\
                 on go {\n\
                   let { chan } = { chan: \"evt\" }\n\
                   SendCustomEvent(chan, hp)\n\
                 }",
            ),
            (
                "rest form",
                "mod f(v: int) {\n\
                   let { other, ...rest } = { other: 1, chan: \"evt\" }\n\
                   SendCustomEvent(rest.chan, v)\n\
                 }\n\
                 in go: exec\non go { f(1) }",
            ),
        ] {
            let r = tc(src);
            assert!(
                r.diagnostics.iter().any(|d| d.code == "WS028"),
                "a non-const destructured name must not satisfy constant-only \
                 gate config — {label}: {:?}",
                r.diagnostics
            );
        }
    }

    /// The mirror of the tests above, and the second manifestation of the
    /// single-named-output wrapping bug: a `const mod` with ONE named output
    /// (`-> (r: string)`) types as a bare `string` per `typecheck::call`, so
    /// its result IS a legal scalar for a constant-only config slot. Wrapping
    /// it in a 1-field record made const evaluation disagree with the type
    /// system and produced a WS028 that reads as a flat contradiction:
    /// `'eventName' … takes a single scalar value, not a record` about a value
    /// typecheck itself calls `string`.
    ///
    /// A two-output `const mod` genuinely IS a record and must still be
    /// rejected here, so both directions are asserted — a fix that simply
    /// stopped wrapping everything would pass the first case and fail the
    /// second.
    #[test]
    fn a_single_named_output_const_mod_result_is_scalar_config_not_a_record() {
        let one = tc(
            "const mod chan(p: const string) -> (r: string) { out r = p .. \"_evt\" }\n\
             const NAME = chan(\"hit\")\n\
             var hp: int = 0\n\
             in go: exec\non go { SendCustomEvent(NAME, hp) }",
        );
        assert!(
            !one.diagnostics.iter().any(|d| d.code == "WS028"),
            "a single named output is a scalar, not a record — it must satisfy \
             a constant-only scalar config slot: {:?}",
            one.diagnostics
        );
        assert_no_diags(&one);

        let two = tc(
            "const mod chan(p: const string) -> (r: string, extra: int) { out r = p\n out extra = 1 }\n\
             const NAME = chan(\"hit\")\n\
             var hp: int = 0\n\
             in go: exec\non go { SendCustomEvent(NAME, hp) }",
        );
        assert!(
            two.diagnostics.iter().any(|d| d.code == "WS028"),
            "a MULTI-output const mod's result really is a record and must \
             still be rejected in a scalar config slot: {:?}",
            two.diagnostics
        );
    }

    /// The same hole reached through a `const` PARAMETER: `const string`
    /// promises the callee a compile-time value, so passing a non-`const`
    /// destructured name must be rejected rather than silently dropped inside
    /// the callee.
    #[test]
    fn a_non_const_destructured_name_cannot_satisfy_a_const_parameter() {
        let r = tc(
            "mod send(name: const string, v: int) { SendCustomEvent(name, v) }\n\
             mod f(v: int) {\n\
               let { chan } = { chan: \"evt\" }\n\
               send(chan, v)\n\
             }\n\
             in go: exec\non go { f(1) }",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS028" || d.code == "WS046"),
            "a non-const destructured name must not satisfy a `const` param: {:?}",
            r.diagnostics
        );
    }

    /// The branch a const `if` KEEPS is still fully type-checked — only the
    /// untaken one is skipped (`if constexpr` semantics). Without this, "the
    /// untaken block isn't checked" could silently widen into "neither block
    /// is checked", and a destructured const would stop catching real errors
    /// in the code it actually ships.
    #[test]
    fn a_const_destructure_still_type_checks_the_branch_it_keeps() {
        let r = tc(
            "var flag: int = 0\n\
             mod f() {\n\
               const p = { x: 111, y: 222 }\n\
               const { x, y } = p\n\
               if x == 111 { flag = \"ill typed, and this branch SHIPS\" } else { flag = 1 }\n\
             }\n\
             in go: exec\non go { f() }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "the TAKEN branch must be type-checked: {:?}",
            r.diagnostics
        );
    }

    // A destructured field derived from a `const` PARAMETER's placeholder is
    // just as fictional as a scalar one (see
    // `a_const_param_placeholder_never_decides_a_branch`) — it must not be
    // allowed to decide a branch either, now that a destructuring `const`
    // inside a mod body is actually evaluated instead of being rejected
    // outright.
    #[test]
    fn a_const_destructure_derived_from_a_placeholder_never_decides_a_branch() {
        let r = tc(
            "var x: int = 0\n\
             mod f(m: const int) {\n\
               const rec = { a: m }\n\
               const { a } = rec\n\
               if a == 1 { x = \"wrong\" } else { x = 2 }\n\
             }\n\
             in go: exec\non go { f(1) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "a field destructured from a placeholder-derived record must not \
             decide a branch: {:?}", r.diagnostics
        );
        assert!(
            r.dropped_ranges.is_empty(),
            "a placeholder must not drive an elision via a destructure: {:?}",
            r.dropped_ranges
        );
    }

    // Task 6: `const` PARAMETERS are enforced the same way `const` bindings
    // are — the argument must evaluate at compile time (WS046 if it can't).
    #[test]
    fn a_const_parameter_rejects_a_runtime_argument() {
        assert_no_diags(&tc(
            "mod g(name: const string, v: int) { }\nmod caller(x: int) { g(\"lit\", x) }",
        ));

        let r = tc("in live: string\nmod g(name: const string) { }\nmod caller() { g(live) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS046"),
            "expected WS046, got {:?}", r.diagnostics);
    }

    #[test]
    fn a_const_parameter_still_type_checks_its_argument() {
        let r = tc("mod g(n: const int) { }\nmod caller() { g(\"not an int\") }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS003"),
            "expected WS003, got {:?}", r.diagnostics);
    }

    #[test]
    fn a_named_constant_satisfies_a_const_parameter() {
        assert_no_diags(&tc("const N = 4\nmod g(n: const int) { }\nmod caller() { g(N * 2) }"));
    }

    // Task 10: the sender side (`SendCustomEvent(CH, v)`) has always accepted a
    // named constant as its channel name; the receiver side (`on
    // CustomEvent(CH)`) used the environment-free `expr_to_literal` and rejected
    // it, so a computed channel name could be sent but never received.
    #[test]
    fn an_event_handler_channel_accepts_a_named_constant() {
        assert_no_diags(&tc(
            "const CH = \"evt_\" .. \"died\"\n\
             var n: int = 0\n\
             on CustomEvent(CH) -> (v: int) { n = v }",
        ));
        // The same through a computed expression, not just a bare name.
        assert_no_diags(&tc(
            "const PREFIX = \"evt_\"\n\
             var n: int = 0\n\
             on CustomEvent(PREFIX .. \"died\") -> (v: int) { n = v }",
        ));
    }

    // Task 11: a bare identifier in enum config is an enum MEMBER name first —
    // that must keep working exactly as before. Only when it names no member
    // does it fall back to resolving through the constant environment.
    #[test]
    fn an_enum_config_argument_may_name_a_constant() {
        assert_no_diags(&tc(
            "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = Bounce) }",
        ));
        assert_no_diags(&tc(
            "const EASE = \"Bounce\"\n\
             mod f(a: float, b: float, t: float) { Easing(a, b, t, function = EASE) }",
        ));
    }

    // Task 13: a call to a `const mod` is itself compile-time-evaluable — the
    // callee's body runs through `const_eval::interp::eval_call`, resolved
    // via `ConstCtx::lookup_mod`.
    //
    // This is only the TYPECHECK half. Accepting the program proves nothing
    // about the value reaching the gate — the first version of this feature
    // passed this exact assertion while lowering baked no channel name at all.
    // The real acceptance test inspects the emitted gate:
    // `lower::tests::const_params::a_const_mod_call_bakes_as_a_custom_event_channel_name`.
    #[test]
    fn a_const_mod_call_satisfies_a_literal_position() {
        assert_no_diags(&tc(
            "const mod evtName(kind: string) -> string { return \"evt_\" .. kind }\n\
             mod ping(v: int) { SendCustomEvent(evtName(\"died\"), v) }",
        ));
    }

    // Wiring `lookup_mod` made `NotAConstMod` capable of lying: a `const mod`
    // call that const evaluation does not descend to isn't evaluated, and the
    // failure used to be reported as "'double' is not declared `const mod`"
    // about a mod that plainly IS one. Leaving a position unevaluated is
    // fine; stating something false about the user's code is not.
    #[test]
    fn a_nested_const_mod_call_is_not_blamed_for_being_non_const() {
        // `double(3) + 1` (this test's ORIGINAL trigger) is no longer nested
        // in the sense this test is about: `eval_expr` now resolves a `const
        // mod` call standing as a `BinOp`/`UnOp`/constructor-argument operand
        // directly (see `const_eval::expr`'s
        // `a_const_mod_call_nested_in_a_binary_operator_evaluates`), so that
        // expression just evaluates to `7` and never reaches this
        // diagnostic. Nest one level deeper instead — as an argument to a
        // call whose OWN callee (`scaleUp`) is an ordinary, non-const `mod` —
        // which `eval_expr`'s `Expr::Call` arm still declines to evaluate,
        // falling back to `reason_for`'s walk and landing on the exact
        // `NestedConstModCall` case this test exists to prove.
        let r = tc(
            "const mod double(n: int) -> int { return n * 2 }\n\
             mod scaleUp(n: int) -> int { return n }\n\
             mod f() { const total = scaleUp(double(3)) }",
        );
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS046")
            .unwrap_or_else(|| panic!("expected WS046, got {:?}", r.diagnostics));
        assert!(
            !d.message.contains("is not declared `const mod`"),
            "must not claim a `const mod` isn't one: {}",
            d.message
        );
        assert!(
            d.message.contains("is a `const mod`")
                && d.message.contains("surrounding expression")
                && d.message.contains("never reaches it"),
            "must name the real limitation (the enclosing call, not the callee): {}",
            d.message
        );
    }

    // The other side of that branch: a callee that genuinely is NOT a `const
    // mod` must keep the original, accurate message.
    #[test]
    fn a_nested_non_const_mod_call_still_says_it_is_not_a_const_mod() {
        let r = tc(
            "mod double(n: int) -> int { return n * 2 }\n\
             mod f() { const total = double(3) + 1 }",
        );
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS046")
            .unwrap_or_else(|| panic!("expected WS046, got {:?}", r.diagnostics));
        assert!(
            d.message.contains("'double' is not declared `const mod`"),
            "a genuinely non-const callee keeps the original message: {}",
            d.message
        );
    }

    #[test]
    fn calling_a_non_const_mod_in_a_const_position_is_an_error() {
        let r = tc(
            "mod evtName(kind: string) -> string { return \"evt_\" .. kind }\n\
             mod ping(v: int) { SendCustomEvent(evtName(\"died\"), v) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS028" || d.code == "WS046"),
            "expected a constant-required error, got {:?}",
            r.diagnostics
        );
    }

    // CRITICAL, PROVEN MISCOMPILE: `TypeCheckCtx::mod_decls` used to be a
    // FLAT map — registered by `register_decl`, never scoped, never popped —
    // while lowering resolves a `const mod` CALL through `ctx.scope`, a
    // properly scope-managed stack. A NESTED `const mod` permanently
    // overwrote the flat entry for its name, so a call made from a totally
    // separate, LATER sibling decl resolved the wrong body.
    //
    // Here `A`'s nested `size` (-> 100) is checked first (chips are checked
    // in source order) and, under the flat-map bug, clobbers the top-level
    // `size` (-> 10) for good. `B`'s `const s = size()` then resolved to
    // 100, took the THEN branch (`s == 100`), and dropped the ELSE — never
    // type-checking the `string` assigned into the `int` var `b`. Lowering,
    // resolving `size` correctly through `ctx.scope` (scoped, not flat),
    // sees the TOP-LEVEL `size` (10), takes the ELSE, and emits a live
    // `Exec_Var_Set b = "definitely not an int"` into an `int` var — code
    // that was NEVER type-checked. Before the `mod_decls` scoping fix this
    // program type-checked with NO diagnostics at all.
    #[test]
    fn a_nested_const_mod_does_not_leak_into_a_sibling_chips_resolution() {
        let r = tc(
            "var b: int = 0\n\
             const mod size() -> int { return 10 }\n\
             mod A() { const mod size() -> int { return 100 } }\n\
             mod B() {\n  const s = size()\n  if s == 100 { b = 2 } else { b = \"definitely not an int\" }\n}",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "B must resolve the TOP-LEVEL `size` (10) — same as lowering — take \
             the else branch, and report the type error in it: got {:?}",
            r.diagnostics
        );
    }

    // The scoping is symmetric: once `A`'s body has finished checking (its
    // `mod_decls` frame popped), a call AFTER it in a totally unrelated decl
    // must not see `A`'s nested override either. Pins the exact mechanism —
    // WHICH range gets dropped — rather than just "no error": under the
    // flat-map bug `B` would resolve `size` to `A`'s leaked 999, so
    // `size() == 10` reads false and the THEN block (not the ELSE) would be
    // the one dropped.
    #[test]
    fn a_nested_const_mod_frame_is_dropped_once_its_scope_closes() {
        let src = "const mod size() -> int { return 10 }\n\
             var ok: int = 0\n\
             mod A() { const mod size() -> int { return 999 } }\n\
             mod B() { const s = size()\n if s == 10 { ok = 1 } else { ok = 2 } }";
        let r = tc(src);
        assert!(
            r.diagnostics.iter().all(|d| d.severity != Severity::Error),
            "resolving the top-level `size` after A's scope has closed must not error: {:?}",
            r.diagnostics
        );
        assert_eq!(r.dropped_ranges.len(), 1, "exactly one branch must be dropped: {:?}", r.dropped_ranges);
        let dropped_src = &src[r.dropped_ranges[0].0.start.offset..r.dropped_ranges[0].0.end.offset];
        assert!(
            dropped_src.contains("ok = 2"),
            "B must resolve the TOP-LEVEL `size` (10, not A's leaked 999), so the \
             ELSE block is the one dropped — got dropped range {dropped_src:?}"
        );
    }

    // A self-referential `const mod` reaches `interp::eval_call` through
    // `ConstCtx::lookup_mod`, NOT through `lower_chip_call`'s WS020 guard
    // (that guard only fires during lowering's wire-expansion of an ORDINARY
    // call). This path's own backstop is `interp::Budget`'s depth counter —
    // this test proves it actually fires (a WS048, not a stack overflow) when
    // a mod calls itself.
    #[test]
    fn a_self_referential_const_mod_call_fails_via_budget_not_a_stack_overflow() {
        let r = tc(
            "const mod f(n: int) -> string { return f(n) }\n\
             mod ping(v: int) { SendCustomEvent(f(1), v) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS048"),
            "expected WS048 (budget exceeded), got {:?}",
            r.diagnostics
        );
    }

    // `if constexpr` semantics: a const-evaluable condition drops the
    // untaken block from type-checking entirely, so a branch that wouldn't
    // even compile for THIS const value never gets checked — that's what
    // lets one mod serve cases that share no API. Task 15 does the matching
    // work on the lowering side (`lower_if`'s generalised elision).

    #[test]
    fn a_dropped_branch_is_not_type_checked() {
        assert_no_diags(&tc(
            "const MODE = 1\nvar x: int = 0\nin go: exec\n\
             on go { if MODE == 1 { x = 1 } else { x = totallyUndefinedThing() } }",
        ));
    }

    // MINOR: a body-local `const` declared inside a TAKEN, const-elided
    // branch must not leak past the `if` — `lower_if`'s elision lowers the
    // taken block STRAIGHT INTO THE PARENT SCOPE (no Branch gate), so it
    // still pushes/pops its own scope frame around that block (see
    // `lower_if`'s doc comment) rather than skipping scoping altogether.
    // This property was verified by hand twice during development but never
    // pinned by a test — do so here on the typecheck side (mirrors
    // `ctx.scope`/`scoped_consts`/`scoped_const_declared`/`mod_decls`, all of
    // which are frame stacks pushed/popped 1:1 with `ctx.scope` via
    // `push_scope`/`pop_scope`, so this single check exercises all four).
    #[test]
    fn a_branch_local_const_does_not_leak_past_the_if() {
        let r = tc(
            "const A = 1\nvar x: int = 0\nin go: exec\n\
             on go { if A == 1 { const b = 5\n x = b } else { x = 0 }\n x = b }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"),
            "a `const` declared inside a taken (const-elided) branch must not be \
             visible after the `if` — got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn typecheck_records_which_ranges_it_dropped() {
        let r = tc(
            "const MODE = 1\nvar x: int = 0\nin go: exec\non go { if MODE == 1 { x = 1 } else { x = 2 } }",
        );
        assert_eq!(r.dropped_ranges.len(), 1, "the else block must be recorded as dropped");
    }

    // IMPORTANT: the widened elision (any const-eval-decidable condition, not
    // just a bare `true`/`false`) must fire ONLY for a condition built from
    // names actually DECLARED `const` — never a plain `let` that merely
    // happens to fold. The feature's own first design principle is that a
    // program using no `const` keyword compiles identically to before it
    // existed; before this restriction, `let A = 1` + `if A == 1 {...} else
    // {...}` newly emitted 3 nodes instead of a Branch plus both blocks, and
    // a type error in the untaken block silently stopped being reported —
    // a behavior change for a program containing no `const` at all.
    #[test]
    fn a_plain_let_condition_keeps_its_branch_and_checks_both_blocks() {
        let r = tc(
            "let A = 1\nvar x: int = 0\nin go: exec\n\
             on go { if A == 1 { x = 1 } else { x = totallyUndefinedThing() } }",
        );
        assert!(
            r.dropped_ranges.is_empty(),
            "a plain `let` (no `const` keyword anywhere in the program) must NOT \
             gain the widened elision — pre-feature behavior for a program using no \
             `const` at all: {:?}",
            r.dropped_ranges
        );
        assert!(
            !r.diagnostics.is_empty(),
            "the else block, calling an undefined function, must still be \
             type-checked (a real Branch was built, both blocks checked): {:?}",
            r.diagnostics
        );
    }

    // The same shape, but with `const` instead of `let`, must still
    // tree-shake exactly as the feature intends — proving the restriction
    // above is keyed on the `const` keyword, not on some accidental
    // difference between the two programs.
    #[test]
    fn a_const_condition_with_the_same_shape_still_tree_shakes() {
        let r = tc(
            "const A = 1\nvar x: int = 0\nin go: exec\n\
             on go { if A == 1 { x = 1 } else { x = totallyUndefinedThing() } }",
        );
        assert_no_diags(&r);
        assert_eq!(
            r.dropped_ranges.len(),
            1,
            "a real `const` must still elide the untaken (else) block: {:?}",
            r.dropped_ranges
        );
    }

    /// Sorted ranges of every block a `tc`/`lower` run dropped, for comparing
    /// the two sides directly.
    fn typecheck_dropped(src: &str) -> Vec<crate::diagnostic::SourceRange> {
        let mut ranges: Vec<crate::diagnostic::SourceRange> =
            tc(src).dropped_ranges.into_iter().map(|(range, _reason)| range).collect();
        ranges.sort_by_key(|r| (r.start.offset, r.end.offset));
        ranges
    }

    fn lowering_dropped(src: &str) -> Vec<crate::diagnostic::SourceRange> {
        let p = parse(src, "test");
        assert!(p.diagnostics.is_empty(), "parse diagnostics: {:?}", p.diagnostics);
        let tc_result = typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            tc_result.diagnostics.iter().all(|d| d.severity != Severity::Error),
            "typecheck errors: {:?}",
            tc_result.diagnostics
        );
        let out = crate::lower::lower(crate::lower::LowerInput {
            ast: &p.ast,
            type_of_expr: &tc_result.type_of_expr,
            op_resolutions: &tc_result.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &p.doc_comments,
            fold_mode: crate::lower::FoldMode::ForceOff,
            ce_slots: &crate::typecheck::CeSlotMap::default(),
        });
        assert!(
            out.diagnostics.iter().all(|d| d.severity != Severity::Error),
            "lower errors: {:?}",
            out.diagnostics
        );
        let mut ranges = out.dropped_ranges;
        ranges.sort_by_key(|r| (r.start.offset, r.end.offset));
        ranges
    }

    /// The EXACT-PARITY invariant that `lower::context`, `lower::stmt`,
    /// `lower::decl` and `typecheck::ctx` all cite by name. It was cited by
    /// four doc comments for a long time without ever being written, and that
    /// is precisely how a real divergence shipped: block-scope `const`
    /// recording used the FULL evaluator in `typecheck::stmt` but the NARROW
    /// `expr_to_literal_in` in `lower::decl`, so a `const` bound to a record —
    /// `const p = { x: 1 }` with `if p.x == 1`, and every destructuring
    /// `const` — was compile-time-decidable to typecheck and not to lowering.
    /// Typecheck skipped the untaken block; lowering emitted it. Ill-typed
    /// code reached the graph with no diagnostic at all.
    ///
    /// The weaker containment property
    /// (`typecheck_never_drops_a_block_that_lowering_still_emits`, below) does
    /// NOT catch that: dropping LESS on the typecheck side is the safe
    /// direction for containment, yet it is exactly what under-checking looks
    /// like when it is lowering that drops less. Equality catches both
    /// directions at once.
    ///
    /// Corpus covers a plain `const`, a record-valued `const` read by field, a
    /// block-scope destructure (plain / alias / rest) and a top-level
    /// destructure — every shape whose recording differs between the two
    /// sides. Cases where the two stages genuinely hold DIFFERENT information
    /// (a `const` param's placeholder, `ident_literal_bool`) are excluded by
    /// construction and live in the containment test instead.
    #[test]
    fn typecheck_and_lowering_drop_exactly_the_same_ranges() {
        let cases: &[(&str, &str)] = &[
            (
                "a plain block-scope const",
                "var x: int = 0\n\
                 mod f() { const A = 1\n if A == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                // The shape that shipped broken with no destructuring at all:
                // `expr_to_literal_in` cannot fold a record literal.
                "a record-valued block-scope const read by field",
                "var x: int = 0\n\
                 mod f() { const p = { a: 1 }\n if p.a == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                "a block-scope destructure",
                "var x: int = 0\n\
                 mod f() { const p = { a: 1, b: 2 }\n const { a, b } = p\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                "a block-scope destructure with an alias",
                "var x: int = 0\n\
                 mod f() { const p = { a: 1, b: 2 }\n const { a: q, b: r } = p\n\
                   if q == 1 { x = 1 } else { x = 2 }\n\
                   if r == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                "a block-scope destructure with a rest",
                "var x: int = 0\n\
                 mod f() { const p = { a: 1, b: 2, c: 3 }\n const { a, ...rest } = p\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if rest.c == 3 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                "a top-level destructure",
                "const p = { a: 1, b: 2 }\nconst { a, b } = p\n\
                 var x: int = 0\nin go: exec\n\
                 on go { if a == 1 { x = 1 } else { x = 2 }\n\
                         if b == 9 { x = 3 } else { x = 4 } }",
            ),
            (
                "a destructure inside a nested block",
                "var x: int = 0\n\
                 mod f(n: int) { if n > 0 { const p = { a: 1 }\n const { a } = p\n\
                   if a == 1 { x = 1 } else { x = 2 } } }\n\
                 in go: exec\non go { f(1) }",
            ),
            // SHADOWING — the other half of keeping the two environments in
            // step, and the half that had no coverage at all. Recording the
            // same constants is only sound if RE-BINDING one clears it on both
            // sides too, and lowering's clearing handled a single spelling
            // (`Ident`, narrow evaluator) where typecheck's handled every one.
            // Each case shadows ONE const and leaves a sibling alone, so the
            // still-const half keeps the corpus's non-vacuity assert honest
            // while the shadowed half is what actually has to agree.
            (
                // `let { a } = …` cleared `a`'s const mark in typecheck and
                // not in lowering: typecheck emitted a real Branch for
                // `if a == 1` while lowering tree-shook it on the STALE value,
                // shipping only the `x = 1` arm and no Branch gate at all.
                "a destructuring let shadowing a destructuring const",
                "var x: int = 0\n\
                 mod f() { const { a, b } = { a: 1, b: 2 }\n let { a } = { a: 9 }\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                // The same divergence reachable with NO destructuring: an
                // `Ident` `let` whose value only the FULL evaluator folds
                // (an `if` expression) never reached lowering's narrow
                // clearing path either.
                "a let shadowing a const with a value only the full evaluator folds",
                "var x: int = 0\n\
                 mod f() { const a = 1\n const b = 2\n let a = if true then 9 else 0\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
            ),
            (
                // The same shadow with the `const` at the TOP level, which is
                // a different mechanism rather than a rephrasing: eviction in
                // `const_lookup_declared_only` fires on a name being PRESENT
                // in a `scoped_consts` frame and ABSENT from that frame's
                // marks, so a top-level constant is only evicted once the
                // inner `let` RECORDS its own value there. Clearing the mark
                // alone left the top-level value resolving straight through.
                "a block-scope let shadowing a top-level const",
                "const a = 1\nconst b = 2\nvar x: int = 0\n\
                 in go: exec\n\
                 on go { let a = if true then 9 else 0\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }",
            ),
            // NAMED-CHIP BOUNDARY. A chip body is a `Block` of statements, so
            // its own `const`s never reach `build_const_env` (which takes
            // `TopDecl`s) — they are recorded by `lower_let_decl` into
            // `scoped_consts.last_mut()`, and `instance_body` used to hand the
            // child ctx NO frame at all, so every one was silently discarded.
            // Typecheck has always had a frame here, so the two stages read
            // different constants inside the same body.
            (
                // The sharpest form: lowering decided on the OUTER constant
                // and typecheck on the INNER one, so they emitted and checked
                // OPPOSITE arms — a wrong-branch miscompile AND an unchecked
                // block reaching the graph, from one program.
                "a named chip's own const shadowing a top-level one",
                "const a = 1\nvar x: int = 0\n\
                 chip Named(t: exec) { const a = 9\n\
                   on t { if a == 1 { x = 1 } else { x = 2 }\n\
                          if a == 9 { x = 3 } else { x = 4 } } }\n\
                 in go: exec\nlet r = Named(go)",
            ),
            (
                // The same gap with nothing shadowed: typecheck decided the
                // condition on the chip-body `const` while lowering, holding
                // no value for it, emitted a runtime Branch over both arms —
                // typecheck dropping a block lowering still emits.
                "a named chip's own const with no outer binding",
                "var x: int = 0\n\
                 chip Named(t: exec) { const a = 7\n\
                   on t { if a == 9 { x = 1 } else { x = 2 } } }\n\
                 in go: exec\nlet r = Named(go)",
            ),
            (
                // INLINED-CALLEE BOUNDARY. A `mod` body is lowered into the
                // CALLER's ctx with the caller's block scopes still open, and
                // the const lookups walk every open frame — so a caller-local
                // shadow evicted the top-level constant INSIDE the callee.
                // Typecheck checks a `mod` body once, at its declaration,
                // where no call site's scope exists, so it kept resolving the
                // top-level one and elided an arm lowering then emitted.
                "a caller-local shadow must not reach into an inlined callee",
                "const a = 1\nvar x: int = 0\n\
                 mod inner() { if a == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\n\
                 on go { let a = if true then 9 else 0\n  inner() }",
            ),
        ];
        for (label, src) in cases {
            let tc_dropped = typecheck_dropped(src);
            let lo_dropped = lowering_dropped(src);
            assert_eq!(
                tc_dropped, lo_dropped,
                "typecheck and lowering must elide EXACTLY the same blocks — {label}\n  \
                 typecheck dropped: {tc_dropped:?}\n  lowering dropped:  {lo_dropped:?}"
            );
            // A corpus entry that elides nothing on both sides would satisfy
            // the equality vacuously and silently stop testing anything.
            assert!(
                !tc_dropped.is_empty(),
                "case must actually exercise an elision — {label}"
            );
        }
    }

    /// THE soundness property of this feature, stated exactly:
    /// **everything LOWERED must have been CHECKED.** A block is lowered iff
    /// it is not in lowering's dropped set; checked iff it is not in
    /// typecheck's. So the requirement is the containment
    ///
    /// ```text
    /// typecheck_dropped  ⊆  lowering_dropped
    /// ```
    ///
    /// — typecheck may skip a block only if lowering also skips it. The
    /// reverse slack is deliberately allowed and is the SAFE direction:
    /// lowering knows things typecheck cannot (a call site's real argument, a
    /// literal bound to a gate), so it may drop MORE. That merely means a
    /// block was checked and then not emitted — at worst a diagnostic about
    /// absent code, never shipped-unchecked code.
    ///
    /// Every case below is a shape that genuinely diverged at some point, so
    /// this is a regression net rather than a smoke test. Cases 1-3 also
    /// satisfy exact EQUALITY (both stages hold the same information), which
    /// is asserted separately and more tightly; case 4-5 are the intentional
    /// strict-subset ones.
    #[test]
    fn typecheck_never_drops_a_block_that_lowering_still_emits() {
        // (label, source, both stages have equal information)
        let cases: &[(&str, &str, bool)] = &[
            (
                "a plain top-level const elides on both sides",
                "const A = 1\nvar x: int = 0\nin go: exec\n\
                 on go { if A == 1 { x = 1 } else { x = 2 } if A == 9 { x = 3 } }",
                true,
            ),
            (
                "@nofold on the handler keeps BOTH blocks on both sides",
                "const A = 1\nvar x: int = 0\nin go: exec\n\n\
                 @nofold\non go { if A == 1 { x = 1 } else { x = 2 } }",
                true,
            ),
            (
                "a module-level @nofold keeps both blocks on both sides",
                "@nofold\n\nconst A = 1\nvar x: int = 0\nin go: exec\n\
                 on go { if A == 1 { x = 1 } else { x = 2 } }",
                true,
            ),
            (
                // CRITICAL regression: `mod_decls` used to be a flat map, so
                // `A`'s NESTED `const mod size` (checked first, in source
                // order) permanently overwrote the top-level `size` for
                // every later reader — typecheck resolved `B`'s call to
                // `size` to A's leaked 100 (dropping the ELSE), while
                // lowering (already scope-correct through `ctx.scope`)
                // resolved the top-level 10 (dropping the THEN). Disjoint
                // dropped sets — exactly the containment violation this test
                // exists to catch. Scoping `mod_decls` like `scoped_consts`
                // (mirroring `lower::LowerCtx::pass1_chips`) makes both
                // stages resolve `size` identically once `A`'s body (and its
                // frame) has closed.
                "a nested const mod does not leak into a later sibling's resolution",
                "const mod size() -> int { return 10 }\nvar x: int = 0\n\
                 mod A() { const mod size() -> int { return 100 } }\n\
                 mod B() { const s = size()\n if s == 100 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { B() }",
                true,
            ),
            (
                // Typecheck sees a placeholder ZERO and must decline to elide
                // at all; lowering inlines the REAL `1` and elides the ELSE.
                // Opposite selections — which is precisely why typecheck must
                // not elide. Strict subset: {} ⊂ {else}.
                "a const PARAM's placeholder must not decide a branch",
                "var x: int = 0\nmod f(m: const int) { if m == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f(1) }",
                false,
            ),
            (
                "a const derived FROM a const param is just as fictional",
                "var x: int = 0\n\
                 mod f(m: const int) { const t = m + 1\n if t == 2 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f(1) }",
                false,
            ),
        ];
        for (label, src, equal_information) in cases {
            let tc_dropped = typecheck_dropped(src);
            let lo_dropped = lowering_dropped(src);
            for range in &tc_dropped {
                assert!(
                    lo_dropped.contains(range),
                    "typecheck skipped a block lowering still EMITS (unchecked code ships) \
                     — {label}\n  typecheck dropped: {tc_dropped:?}\n  lowering dropped:  {lo_dropped:?}"
                );
            }
            if *equal_information {
                assert_eq!(
                    tc_dropped, lo_dropped,
                    "both stages have the same information here, so their dropped sets must \
                     match exactly — {label}"
                );
            } else {
                assert!(
                    tc_dropped.len() < lo_dropped.len(),
                    "expected typecheck to be strictly more conservative here — {label}\n  \
                     typecheck dropped: {tc_dropped:?}\n  lowering dropped:  {lo_dropped:?}"
                );
            }
        }
    }

    /// The `@nofold` half of the agreement above, stated as the miscompile it
    /// prevents: lowering keeps the `Branch` and emits BOTH blocks, so a type
    /// error in the "untaken" one is real code that ships. Before typecheck
    /// mirrored `nofold_depth`, this compiled with no diagnostic at all and
    /// emitted a live `Var_Set x = "definitely not an int"` into an int var.
    #[test]
    fn a_nofold_if_still_type_checks_both_blocks() {
        let r = tc(
            "const MODE = 1\nvar x: int = 0\nin go: exec\n\n\
             @nofold\non go { if MODE == 1 { x = 1 } else { x = \"definitely not an int\" } }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "@nofold must keep checking the block it still lowers: {:?}",
            r.diagnostics
        );
        assert!(
            r.dropped_ranges.is_empty(),
            "@nofold must drop nothing: {:?}",
            r.dropped_ranges
        );
    }

    /// A `const` parameter's placeholder is a type-shaped ZERO seeded once,
    /// before any call site exists — so `m == 1` evaluates to FALSE here while
    /// lowering, inlining the real `1`, takes the THEN. Selecting a branch
    /// from it means the branch that actually ships is never checked: this
    /// program used to compile clean and emit a lone `Var_Set x = "wrong"`
    /// into an int var. Both blocks must be checked instead.
    #[test]
    fn a_const_param_placeholder_never_decides_a_branch() {
        let r = tc(
            "var x: int = 0\nmod f(m: const int) { if m == 1 { x = \"wrong\" } else { x = 2 } }\n\
             in go: exec\non go { f(1) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "the branch the real argument selects must be type-checked: {:?}",
            r.diagnostics
        );
        assert!(
            r.dropped_ranges.is_empty(),
            "a placeholder must not drive an elision: {:?}",
            r.dropped_ranges
        );
        // Transitively, too: a `const` derived from the placeholder carries
        // exactly the same fiction.
        let derived = tc(
            "var x: int = 0\n\
             mod f(m: const int) { const t = m + 1\n if t == 2 { x = \"wrong\" } else { x = 2 } }\n\
             in go: exec\non go { f(1) }",
        );
        assert!(
            derived.diagnostics.iter().any(|d| d.code == "WS003"),
            "placeholder-ness must propagate through a derived const: {:?}",
            derived.diagnostics
        );
    }

    /// The guard above must not over-fire: a REAL top-level constant read
    /// inside the same mod body still elides normally, so tightening the
    /// placeholder rule didn't just switch the feature off inside mods.
    #[test]
    fn a_real_constant_still_elides_inside_a_mod_body() {
        assert_no_diags(&tc(
            "const MODE = 1\nvar x: int = 0\n\
             mod f(n: int) { if MODE == 1 { x = n } else { x = totallyUndefinedThing() } }\n\
             in go: exec\non go { f(1) }",
        ));
    }

    /// A constant-only SCALAR config slot must reject a constant COLLECTION.
    ///
    /// Being constant is necessary but not sufficient: a record/array/map has
    /// no single scalar to bake into the gate's data field. Unchecked, these
    /// reached emit and were resolved there by literal kind, two wrong ways —
    /// an array silently defaulted to an empty/zero value (a clean-typechecking
    /// program baking config the author never wrote), and a record hit a
    /// converter with no way to decline, aborting the whole compile with an
    /// `unreachable!` instead of producing a diagnostic.
    #[test]
    fn a_non_scalar_constant_in_a_scalar_config_slot_is_ws028() {
        for (src, what) in [
            // named config field, record
            (
                "const cfg = { rooms: 2, timer: 60 }\n\
                 var x: int = 0\n\
                 on Clock(interval = 1.0, enabled = true, onTime = cfg) { x = x + 1 }",
                "a record",
            ),
            // POSITIONAL config field, record — a separate slot from the above
            (
                "const cfg = { rooms: 2, timer: 60 }\n\
                 var x: int = 0\n\
                 on ChatCommand(cfg) { x = x + 1 }",
                "a record",
            ),
            // the pre-existing array case this subsumes: it used to typecheck
            // clean and silently bake a wrong default
            (
                "const arr = [1, 2, 3]\n\
                 var x: int = 0\n\
                 on Clock(interval = 1.0, enabled = true, onTime = arr) { x = x + 1 }",
                "an array",
            ),
        ] {
            let r = tc(src);
            let hit = r
                .diagnostics
                .iter()
                .find(|d| d.code == "WS028")
                .unwrap_or_else(|| panic!("expected WS028 for {what} in a scalar config slot: {:?}", r.diagnostics));
            assert!(
                hit.message.contains(what),
                "the diagnostic must name the offending kind ({what}), got {:?}",
                hit.message
            );
        }
    }

    /// The guard above must not over-fire: an ordinary SCALAR constant in the
    /// same slot stays accepted, so this is a shape check, not a blanket ban.
    #[test]
    fn a_scalar_constant_in_a_scalar_config_slot_is_still_accepted() {
        assert_no_diags(&tc(
            "const T = 0.5\n\
             var x: int = 0\n\
             on Clock(interval = 1.0, enabled = true, onTime = T) { x = x + 1 }",
        ));
    }
