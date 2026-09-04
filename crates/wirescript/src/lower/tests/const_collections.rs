//! A constant array/map initializer must bake into the gate's `InitialValue`
//! with ZERO runtime gates — the same guarantee `predeclare.rs` already gives
//! a plain literal initializer (`var t: int[] = [1, 2, 3]`), now routed
//! through `const_eval::eval_expr` so a `const mod` call per element also
//! qualifies, not just literals and named constants.
use super::*;
use super::const_params::{lower_ok, nodes_of};

/// A test using plain named constants (`[N, N * 2]`) would not discriminate
/// this path — `array_elem_literal` already routes those through
/// `expr_to_literal_in`, which resolves named constants. A const-mod CALL
/// per element is the one form only `const_eval` can evaluate.
#[test]
fn a_const_mod_call_in_an_array_initializer_bakes_with_no_push_gates() {
    let m = lower_ok(
        "const mod size(n: int) -> int { return n * 10 }\n\
         var t: int[] = [size(1), size(2), size(3)]",
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::ARRAY_PUSH).is_empty(),
        "a fully const initializer must bake, not push"
    );
    let arrays = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    let baked = arrays[0].properties.get(&crate::intern::sym::INITIAL_VALUE);
    assert_eq!(
        baked,
        Some(&crate::ir::Literal::Array(vec![
            crate::ir::Literal::Int(10),
            crate::ir::Literal::Int(20),
            crate::ir::Literal::Int(30),
        ]))
    );
}

/// Same discriminating shape, for a `Map<K, V>` initializer via
/// `bake_map_init`/`map_entry_literal`.
#[test]
fn a_const_mod_call_in_a_map_initializer_bakes_with_no_set_gates() {
    let m = lower_ok(
        "const mod size(n: int) -> int { return n * 10 }\n\
         var t: Map<int, int> = { 1 => size(1), 2 => size(2) }",
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::MAP_SET).is_empty(),
        "a fully const map initializer must bake, not set"
    );
    let maps = nodes_of(&m, crate::ir::gate_class::PSEUDO_MAP_VAR);
    let baked = maps[0].properties.get(&crate::intern::sym::INITIAL_VALUE);
    assert_eq!(
        baked,
        Some(&crate::ir::Literal::Map(vec![
            (crate::ir::Literal::Int(1), crate::ir::Literal::Int(10)),
            (crate::ir::Literal::Int(2), crate::ir::Literal::Int(20)),
        ]))
    );
}

/// An array with ONE non-constant element must still fall back to the
/// existing "starts empty, warn" behaviour — routing through `const_eval`
/// must not turn a genuine runtime value into a hard error at this site, nor
/// silently bake a partial array. Nested inside a mod body (not top-level):
/// a top-level array's "starts empty" is reported by TYPECHECK's WS003
/// (`check_top_level_array_init`) instead, and lowering's OWN warning is
/// deliberately suppressed there (`skip_array_inits`) to avoid double
/// reporting — a `static var` local is where lowering's own warning path
/// (`warn_unbaked_var_init`, `skip_array_inits: false`) actually fires, so
/// this is what proves the `const_eval` routing didn't disturb it.
#[test]
fn a_local_array_with_a_non_constant_element_still_starts_empty_and_warns() {
    let src = "mod f(v: int) {\n  static var t: int[] = [1, v, 3]\n}\n\
               in go: exec\non go { f(1) }";
    let r = compile(src);
    let arrays = nodes_of(&r.module, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    assert!(
        arrays[0]
            .properties
            .get(&crate::intern::sym::INITIAL_VALUE)
            .is_none(),
        "a partially-constant array must NOT bake a partial InitialValue"
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.severity == crate::diagnostic::Severity::Warning),
        "a dropped non-constant array initializer must still warn: {:?}",
        r.diagnostics
    );
}

/// SHADOWING. A chip-local `const mod` that shadows a same-named top-level
/// one must win in the BAKE path exactly as it wins everywhere else. The
/// bake's `pass1_chips` lookup is seeded from the ROOT module's declarations
/// and inherited wholesale by every child chip context, so without an
/// explicit per-child overlay the bake silently resolves the OUTER mod while
/// the ordinary call path (`ping(size(1))`, via `ctx.scope`) resolves the
/// inner one — two different answers for the same call text, with no
/// diagnostic. That is strictly worse than the pre-feature behaviour, which
/// baked nothing at all here.
///
/// The `ping(size(1))` call is the control: it is a const-required position
/// resolved through the ordinary scope path, so it pins what the CORRECT
/// answer is (100, the inner mod) independently of the bake under test.
#[test]
fn a_chip_local_const_mod_shadows_the_top_level_one_in_the_bake_path() {
    let m = lower_ok(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         const mod size(n: int) -> int { return n * 10 }\n\
         chip c() {\n\
        \x20 const mod size(n: int) -> int { return n * 100 }\n\
        \x20 in g: exec\n\
        \x20 on g { ping(size(1)) }\n\
        \x20 var t: int[] = [size(1)]\n\
         }\n\
         in go: exec\n\
         on go { c() }",
    );
    let arrays = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    let baked = arrays[0].properties.get(&crate::intern::sym::INITIAL_VALUE);
    assert_eq!(
        baked,
        Some(&crate::ir::Literal::Array(vec![crate::ir::Literal::Int(100)])),
        "the chip-local `size` (n * 100) must win over the top-level one \
         (n * 10) in the bake path, as it does in every other path"
    );
}

/// The same shadowing rule for a `Map<K, V>` initializer (`bake_map_init` /
/// `map_entry_literal`), which resolves through the identical lookup.
#[test]
fn a_chip_local_const_mod_shadows_the_top_level_one_in_the_map_bake_path() {
    let m = lower_ok(
        "const mod size(n: int) -> int { return n * 10 }\n\
         chip c() {\n\
        \x20 const mod size(n: int) -> int { return n * 100 }\n\
        \x20 var t: Map<int, int> = { 1 => size(1) }\n\
         }\n\
         in go: exec\n\
         on go { c() }",
    );
    let maps = nodes_of(&m, crate::ir::gate_class::PSEUDO_MAP_VAR);
    let baked = maps[0].properties.get(&crate::intern::sym::INITIAL_VALUE);
    assert_eq!(
        baked,
        Some(&crate::ir::Literal::Map(vec![(
            crate::ir::Literal::Int(1),
            crate::ir::Literal::Int(100),
        )])),
        "the chip-local `size` must win in the map bake path too"
    );
}

/// The same shadowing rule for an INLINED `mod` body. This path is the
/// hazardous one: unlike a chip (fresh child context), an inlined mod body
/// pre-declares onto the CALLER's `ctx`, so the nested declaration must both
/// win inside the body AND be undone afterwards.
///
/// Both leak detectors are lowered in PASS 2, *after* `f()` is expanded — a
/// TOP-LEVEL `var` would not discriminate, because every top-level
/// initializer bakes during pass 1, before any inlining happens. The two
/// that do discriminate:
///   - `afterh`, a handler-local `static var` following the `f()` call;
///   - `chipv`, inside a chip instantiated after it, which inherits
///     `pass1_chips` by clone at instantiation time.
/// With the restore removed both bake `Int(100)` instead of `Int(10)`.
#[test]
fn a_mod_local_const_mod_shadows_inside_the_body_and_does_not_leak_out() {
    let m = lower_ok(
        "const mod size(n: int) -> int { return n * 10 }\n\
         mod f() {\n\
        \x20 const mod size(n: int) -> int { return n * 100 }\n\
        \x20 static var inner: int[] = [size(1)]\n\
         }\n\
         chip c() {\n\
        \x20 var chipv: int[] = [size(1)]\n\
         }\n\
         in go: exec\n\
         on go { f()\n static var afterh: int[] = [size(1)]\n c() }",
    );
    let baked: Vec<_> = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR)
        .iter()
        .map(|n| {
            (
                match n.properties.get(&crate::intern::sym::NAME_LABEL) {
                    Some(crate::ir::Literal::String(s)) => s.clone(),
                    _ => String::new(),
                },
                n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned(),
            )
        })
        .collect();
    let of = |name: &str| {
        baked
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no array var '{name}' in {baked:?}"))
            .1
            .clone()
    };
    assert_eq!(
        of("inner"),
        Some(crate::ir::Literal::Array(vec![crate::ir::Literal::Int(100)])),
        "the mod-local `size` must win INSIDE the inlined body"
    );
    assert_eq!(
        of("afterh"),
        Some(crate::ir::Literal::Array(vec![crate::ir::Literal::Int(10)])),
        "the mod-local `size` must NOT leak out of the inlined body into a \
         later handler-local initializer"
    );
    assert_eq!(
        of("chipv"),
        Some(crate::ir::Literal::Array(vec![crate::ir::Literal::Int(10)])),
        "the mod-local `size` must NOT leak into a chip instantiated afterwards"
    );
}

/// `const_eval::eval_expr` already tries `expr_to_literal_in` FIRST (the seam
/// note in `const_eval::expr`), so routing `array_elem_literal` through it
/// must not change what a plain-literal/named-constant array bakes to.
/// Regression guard for the "keep what bakes today baking" requirement.
#[test]
fn a_plain_literal_array_still_bakes_exactly_as_before() {
    let m = lower_ok("const N = 4\nvar t: int[] = [1, N, N * 2]");
    let arrays = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    let baked = arrays[0].properties.get(&crate::intern::sym::INITIAL_VALUE);
    assert_eq!(
        baked,
        Some(&crate::ir::Literal::Array(vec![
            crate::ir::Literal::Int(1),
            crate::ir::Literal::Int(4),
            crate::ir::Literal::Int(8),
        ]))
    );
}



/// WIDTH PARITY between typecheck's constant check and lowering's bake.
///
/// `check_top_level_array_init` (typecheck) resolves a `const mod` callee via
/// `ctx.resolve_mod`, which is FORWARD-TOLERANT — typecheck registers every
/// declaration in a first pass, so a call textually above its callee still
/// resolves there. Lowering's bake resolves via `resolve_mod_pass1`, which is
/// strictly SOURCE-ORDERED. Those two widths differ, and where they differ
/// the failure mode is the one this feature has shipped twice already:
/// typecheck accepts a program whose initializer lowering then silently
/// drops (array starts empty, no diagnostic).
///
/// Today the gap is unreachable only because `infer::infer` runs FIRST in the
/// same loop and emits WS021 for the use-before-declaration, rejecting the
/// program before the divergence can matter. Nothing else pins that ordering.
///
/// This test asserts the INVARIANT rather than the mechanism: for a
/// forward-declared const-mod call in an array initializer, either typecheck
/// REJECTS the program, or lowering must actually BAKE it. Any future change
/// that drops/reorders the WS021 emission without also widening
/// `resolve_mod_pass1` lands in the forbidden middle and fails here.
#[test]
fn typecheck_never_accepts_an_initializer_that_lowering_silently_drops() {
    let src = "var t: int[] = [size(1)]\n\
               const mod size(n: int) -> int { return n * 10 }";
    let resolved = crate::resolve(src, "test", &crate::FsLoader);
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let rejected = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .any(|d| d.severity == crate::diagnostic::Severity::Error);

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
    let baked = nodes_of(&out.module, crate::ir::gate_class::PSEUDO_ARRAY_VAR)
        .first()
        .and_then(|n| n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned());

    assert!(
        rejected || baked.is_some(),
        "a forward-declared const-mod call in an array initializer was ACCEPTED \
         by typecheck but baked nothing at lowering — typecheck's `resolve_mod` \
         is forward-tolerant while lowering's `resolve_mod_pass1` is \
         source-ordered, and the WS021 that used to hide that gap no longer \
         fires. Widen `resolve_mod_pass1` or restore the rejection."
    );
}

/// An out-of-range const index INSIDE an array initializer must report the
/// specific out-of-range message, not the generic "must be constant literals"
/// WS003 — that advice ("build the array in an exec handler") is actively
/// wrong for `[[1, 2][9]]`, whose problem is the index, not constant-ness.
/// The two must never both fire, which is why the fold helper emits and the
/// caller reports nothing further.
///
/// A pre-existing, unrelated WS007 (pure-context array read) also fires on
/// this program; the assertion deliberately ignores it and pins only the
/// const diagnostic.
#[test]
fn an_out_of_range_index_in_an_initializer_reports_the_specific_reason() {
    let resolved = crate::resolve("var t: int[] = [[1,2][9]]", "test", &crate::FsLoader);
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let ws046: Vec<_> = tc.diagnostics.iter().filter(|d| d.code == "WS046").collect();
    assert_eq!(
        ws046.len(),
        1,
        "expected exactly one WS046 for the out-of-range index, got {:?}",
        tc.diagnostics
    );
    assert!(
        ws046[0].message.contains("index 9 is out of range"),
        "the specific out-of-range wording must survive to the diagnostic, got {:?}",
        ws046[0].message
    );
    assert!(
        !tc.diagnostics
            .iter()
            .any(|d| d.message.contains("must be constant literals")),
        "the generic WS003 must NOT also fire alongside the specific reason: {:?}",
        tc.diagnostics
    );
}

/// The four-position internal-consistency check for a `mod`-local `const mod`.
///
/// Every position below resolves the SAME call text, `size(1)`, and every one
/// must answer with the declaration that is lexically in scope there. The
/// hazard is that the bake path and the ordinary call path can disagree
/// silently, so this pins all four at once:
///
///   1. `direct`   — initializer directly in the inlined mod body    -> 100
///   2. `chipv`    — chip DECLARED and INSTANTIATED inside that body -> 100
///   3. `deeper`   — chip nested inside that chip                    -> 100
///   4. `sibling`  — handler-local var AFTER the mod call returns    -> 10
///
/// Position 2 is the one that was wrong before `pass1_chips` became
/// scope-managed: the manual restore ran right after pre-declaration, so the
/// chip cloned an already-restored map and resolved the OUTER `size` while an
/// ordinary call inside the same chip resolved the inner one. Position 4 is
/// the leak detector in the other direction — the mod-local declaration must
/// not outlive the body.
#[test]
fn a_mod_local_const_mod_is_visible_everywhere_inside_the_body_and_nowhere_outside() {
    let m = lower_ok(
        "const mod size(n: int) -> int { return n * 10 }\n\
         mod f() {\n\
        \x20 const mod size(n: int) -> int { return n * 100 }\n\
        \x20 static var direct: int[] = [size(1)]\n\
        \x20 chip c1() {\n\
        \x20   var chipv: int[] = [size(1)]\n\
        \x20   chip c2() {\n\
        \x20     var deeper: int[] = [size(1)]\n\
        \x20   }\n\
        \x20   c2()\n\
        \x20 }\n\
        \x20 c1()\n\
         }\n\
         in go: exec\n\
         on go { f()\n static var sibling: int[] = [size(1)] }",
    );
    let baked: Vec<(String, Option<crate::ir::Literal>)> =
        nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR)
            .iter()
            .map(|n| {
                (
                    match n.properties.get(&crate::intern::sym::NAME_LABEL) {
                        Some(crate::ir::Literal::String(s)) => s.clone(),
                        _ => String::new(),
                    },
                    n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned(),
                )
            })
            .collect();
    let of = |name: &str| {
        baked
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no array var '{name}' in {baked:?}"))
            .1
            .clone()
    };
    let arr = |v: i64| Some(crate::ir::Literal::Array(vec![crate::ir::Literal::Int(v)]));

    assert_eq!(of("direct"), arr(100), "mod-local `size` must win directly in the body");
    assert_eq!(
        of("chipv"),
        arr(100),
        "a chip declared and instantiated INSIDE the inlined mod body must see \
         the mod-local `size` — this is the case the manual restore got wrong \
         by running before the body was lowered"
    );
    assert_eq!(
        of("deeper"),
        arr(100),
        "the mod-local `size` must survive a second level of chip nesting"
    );
    assert_eq!(
        of("sibling"),
        arr(10),
        "the mod-local `size` must NOT outlive the body it was declared in"
    );
}


/// A record `let` whose FIELD needs a `const mod` call must still lower
/// through the ordinary `Binding::Record` path, NOT bake as a
/// `Literal::Record`.
///
/// A record is the one bakeable literal kind with a pre-existing non-literal
/// lowering, so `lower_let_decl` baking it early-returns straight past that
/// path. The visible damage was a miscompile, not just a lost optimization:
/// the later `p.x`/`p.y` reads lost their record binding and fell through to
/// the vector-swizzle path, emitting a bogus `Expr_SplitVector` `.X`/`.Y` on
/// a record — and the baked `Literal::Record` then reached emit, where it has
/// no wire form at all. Arrays and maps have no such existing lowering, which
/// is why only records need excluding from the bake.
#[test]
fn a_record_let_with_a_const_mod_field_keeps_its_record_binding() {
    for src in [
        // body-local `let`
        "type Point = { x: int, y: int }\n\
         const mod mk(n: int) -> int { return n * 2 }\n\
         var ax: int = 0\n\
         var ay: int = 0\n\
         on Clock(interval = 1.0, enabled = true) {\n\
           let p: Point = { x: mk(3), y: 10 }\n\
           ax = p.x + 1\n\
           ay = p.y + 1\n\
         }",
        // top-level `let` — same bug, different frame (no `scoped_consts`
        // frame exists there, but the bake path returns early regardless)
        "type Point = { x: int, y: int }\n\
         const mod mk(n: int) -> int { return n * 2 }\n\
         var ax: int = 0\n\
         var ay: int = 0\n\
         let p: Point = { x: mk(3), y: 10 }\n\
         on Clock(interval = 1.0, enabled = true) {\n\
           ax = p.x + 1\n\
           ay = p.y + 1\n\
         }",
    ] {
        let m = lower_ok(src);
        assert!(
            nodes_of(&m, crate::ir::gate_class::SPLIT_VECTOR).is_empty(),
            "a record field read must not lower as a vector swizzle: {src}"
        );
        // The const mod's body really was inlined (its `n * 2` gate exists),
        // proving the field kept a real lowering rather than vanishing.
        assert_eq!(
            nodes_of(&m, "BrickComponentType_WireGraph_Expr_MathMultiply").len(),
            1,
            "the const mod body must still lower: {src}"
        );
        assert_eq!(
            nodes_of(&m, "BrickComponentType_WireGraph_Expr_MathAdd").len(),
            2,
            "both field reads must still lower: {src}"
        );
    }
}

/// A compile-time index into a compile-time array must reach emit as a real
/// LITERAL in BOTH channels a constant can be consumed through — the bake slot
/// (an `InitialValue` property) and a runtime WIRE OPERAND.
///
/// The wire-operand half is the one that regressed: typecheck was narrowed to
/// accept `const z = t[1]` at module level (the read emits no gate, so WS007's
/// exec-context rule does not apply), but lowering had no form for it and
/// synthesised an `_Unsupported` placeholder. Emit writes no component for a
/// placeholder, so the wire feeding the consumer died and the gate silently
/// read its type default — `if z == rv` compiled clean and compared against 0
/// instead of 555. The bake channel alone could not catch it: an initializer
/// resolves the NAME through `LowerCtx.const_env`, which always worked, while a
/// runtime read resolves through the port binding the ordinary lowering
/// installs. Both are asserted here for exactly that reason.
#[test]
fn a_const_index_reaches_emit_as_a_literal_in_both_the_bake_and_wire_channels() {
    // Bake channel: the folded `t[1]` lands in the array var's InitialValue,
    // alongside a distinctive control literal so a wrong value cannot pass.
    let m = lower_ok(
        "const t = [777, 555]\n\
         const z = t[1]\n\
         var counts: int[] = [z, 12345]",
    );
    let arrays = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    assert_eq!(
        arrays[0].properties.get(&crate::intern::sym::INITIAL_VALUE),
        Some(&crate::ir::Literal::Array(vec![
            crate::ir::Literal::Int(555),
            crate::ir::Literal::Int(12345),
        ])),
        "a const index must bake its real value into the array initializer"
    );

    // Wire-operand channel: `z` feeds a Compare, so the folded literal must be
    // inlined as the operand. Pre-fix this property was absent and the operand
    // wire dangled, so the gate read 0.
    let m = lower_ok(
        "const t = [777, 555]\n\
         const z = t[1]\n\
         var rv: int = 0\n\
         in go: exec\n\
         on go { if z == rv { BroadcastChatMessage(\"EQ\") } }",
    );
    // No placeholder anywhere: neither for `const z = t[1]` on line 2, nor for
    // the `const t = [777, 555]` declaration on line 1, which used to leave an
    // orphan `_Unsupported` plus a WSP001 warning behind on every compile.
    assert!(
        nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "a const collection, and a const index into it, must lower to no _Unsupported placeholder"
    );
    let compares = nodes_of(&m, crate::ir::gate_class::COMPARE_EQUAL);
    assert_eq!(
        compares[0].properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(555)),
        "a const index used as a wire operand must inline its real value"
    );
}

/// A `const` container used ONLY at compile time costs nothing: no container
/// gate, and no placeholder either.
///
/// Both halves matter and neither implies the other. The placeholder half is
/// the bug that shipped — `const t = [...]` recorded its value correctly AND
/// lowered the same literal as a runtime expression, which has no lowering, so
/// every compile emitted an orphan `_Unsupported` plus a WSP001 warning. The
/// no-container half is the guarantee that the fix stayed LAZY: materializing
/// eagerly would also kill the placeholder, while charging every compile-time
/// const table a gate it never uses.
#[test]
fn a_const_container_used_only_at_compile_time_emits_no_gate() {
    let m = lower_ok(
        "const t = [777, 555]\n\
         const z = t[1]\n\
         var counts: int[] = [z, 12345]",
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "a const array declaration must not synthesise a placeholder"
    );
    // Exactly one array gate: `counts`. `t` is answered entirely at compile
    // time, so it has no runtime form here.
    let arrays = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    assert_eq!(
        arrays.len(),
        1,
        "only the `var counts` array may have a gate; `t` is compile-time only"
    );
    assert_eq!(
        arrays[0].properties.get(&crate::intern::sym::INITIAL_VALUE),
        Some(&crate::ir::Literal::Array(vec![
            crate::ir::Literal::Int(555),
            crate::ir::Literal::Int(12345),
        ])),
        "the surviving gate must be `counts`, with the folded index baked"
    );

    let m = lower_ok("const t = { \"a\": 1 }\nconst z = t[\"a\"]\nvar sink: int = z");
    assert!(
        nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "a const map declaration must not synthesise a placeholder"
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::PSEUDO_MAP_VAR).is_empty(),
        "a compile-time-only const map must have no runtime container gate"
    );
}

/// A `const` table read at a RUNTIME index gets a real container gate with its
/// contents baked into `InitialValue` — the same gate, and the same baked
/// property, that `var t: int[] = [...]` gets.
///
/// This is the silent miscompile the feature closes: the read used to fall
/// through to an `_Unsupported` placeholder that emit writes no component for,
/// so a const lookup table indexed by a runtime value compiled clean and read 0
/// in game. Asserting the BAKED VALUES, not merely that a gate exists, is what
/// makes that unfakeable — an empty container gate would read 0 exactly like
/// the placeholder did.
#[test]
fn a_const_container_read_at_a_runtime_index_materializes_one_baked_gate() {
    let m = lower_ok(
        "const t = [777, 555]\n\
         var i: int = 1\n\
         in go: exec\n\
         on go { BroadcastChatMessage(\"${t[i]}\") }",
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "a runtime read of a const table must not lower to a placeholder"
    );
    let arrays = nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR);
    assert_eq!(
        arrays.len(),
        1,
        "the const table gets exactly one container gate"
    );
    assert_eq!(
        arrays[0].properties.get(&crate::intern::sym::INITIAL_VALUE),
        Some(&crate::ir::Literal::Array(vec![
            crate::ir::Literal::Int(777),
            crate::ir::Literal::Int(555),
        ])),
        "the materialized container must carry the const value's real contents"
    );
    assert_eq!(
        nodes_of(&m, crate::ir::gate_class::ARRAY_GET).len(),
        1,
        "the read itself must be a real ArrayVar_Get"
    );

    // The map half. A constant key would be answered by the fold instead, so
    // the key here is a runtime var — that is what forces the container.
    let m = lower_ok(
        "const t = { \"a\": 11, \"b\": 22 }\n\
         var k: string = \"b\"\n\
         in go: exec\n\
         on go { BroadcastChatMessage(\"${t[k]}\") }",
    );
    let maps = nodes_of(&m, crate::ir::gate_class::PSEUDO_MAP_VAR);
    assert_eq!(maps.len(), 1, "the const map gets exactly one container gate");
    assert_eq!(
        maps[0].properties.get(&crate::intern::sym::INITIAL_VALUE),
        Some(&crate::ir::Literal::Map(vec![
            (
                crate::ir::Literal::String("a".into()),
                crate::ir::Literal::Int(11)
            ),
            (
                crate::ir::Literal::String("b".into()),
                crate::ir::Literal::Int(22)
            ),
        ])),
        "the materialized map must carry the const value's real entries"
    );
}

/// Materialization is memoized: many runtime uses of one `const` table share
/// ONE container gate.
///
/// Three DIFFERENT shapes of use are mixed deliberately — an index read, a
/// read-only method, and a `T[]` argument — because they enter through three
/// separate call sites, so a per-site memo would still emit three gates.
#[test]
fn many_runtime_uses_of_one_const_table_share_a_single_container_gate() {
    let m = lower_ok(
        "const t = [777, 555]\n\
         mod pick(ys: int[], at: int) -> int { return ys[at] }\n\
         var i: int = 1\n\
         in go: exec\n\
         on go {\n\
           BroadcastChatMessage(\"${t[i]}\")\n\
           BroadcastChatMessage(\"${t.length()}\")\n\
           BroadcastChatMessage(\"${pick(t, i)}\")\n\
         }",
    );
    assert_eq!(
        nodes_of(&m, crate::ir::gate_class::PSEUDO_ARRAY_VAR).len(),
        1,
        "every runtime use of one const table must share a single container gate"
    );
    assert!(
        nodes_of(&m, crate::ir::gate_class::UNSUPPORTED).is_empty(),
        "no use of a const table may fall back to a placeholder"
    );
}

/// A `const` container is IMMUTABLE, and that is what keeps its two forms —
/// the compile-time value and the runtime gate — from ever disagreeing. A
/// mutating method is WS044; a read-only one compiles.
///
/// Both halves run the SAME program shape, so this can only pass by
/// distinguishing the METHODS, not by accepting or rejecting const containers
/// wholesale.
#[test]
fn a_mutating_method_on_a_const_container_is_rejected_and_a_read_only_one_is_not() {
    const PROGRAM: &str = "const t = [777, 555]\n\
         in go: exec\n\
         on go { BroadcastChatMessage(\"${t.METHOD()}\") }";

    let r = compile(&PROGRAM.replace("METHOD", "length"));
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "a read-only method on a const container must compile: {:?}",
        r.diagnostics
    );

    let r = compile(&PROGRAM.replace("METHOD", "reverse"));
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS044"),
        "a mutating method on a const container must be WS044: {:?}",
        r.diagnostics
    );

    // Through a `T[]` parameter the mutation is spelled with a name that is not
    // `const` at all, so a rule keyed on the receiver's NAME would miss it and
    // the callee would quietly rewrite the container while the const
    // environment kept reporting the original contents.
    let r = compile(
        "const t = [777, 555]\n\
         mod wipe(ys: int[]) { ys.clear() }\n\
         in go: exec\n\
         on go { wipe(t) }",
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS044"),
        "mutating a const container through an array parameter must be WS044: {:?}",
        r.diagnostics
    );
}

/// `.length()` on a `const` container in PURE context. Typecheck exempts a
/// non-mutating read on a `const` receiver from the exec-context rule
/// (`container_call_exec_exempt`) on the premise that it folds, so lowering has
/// to actually fold it. The map form used to wire the map REFERENCE into the
/// consumer with no diagnostic at all, and the array form a placeholder.
///
/// The indexed analogue (`const_fold_index_access`) already closes exactly this
/// hole for `t[i]` / `m[k]`.
#[test]
fn const_container_length_folds_in_pure_context() {
    for (src, want) in [
        ("const m = { \"a\": 1, \"b\": 2 }\nconst n = m.length()\nout o = n", 2),
        ("const m = { \"a\": 1, \"b\": 2 }\nout o = m.length()", 2),
        ("const m = { \"a\": 1, \"b\": 2 }\nlet n = m.length()\nout o = n", 2),
        ("const a = [1, 2, 3]\nconst n = a.length()\nout o = n", 3),
        ("const a = [1, 2, 3]\nout o = a.length()", 3),
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        assert!(
            !has_gate(&r, crate::ir::gate_class::UNSUPPORTED),
            "`.length()` on a const container fell to a placeholder:\n{src}"
        );
        // The value must reach the consumer as a baked constant, not as the
        // container's own ref port (which is the silent-miscompile shape). A
        // constant feeding a dataless target (an output rerouter's `RerInput`)
        // is carried by a materialized-constant gate rather than a bare literal
        // node - the same shape the `m[k]` index fold already produces.
        let baked = r.module.nodes.values().any(|n| {
            n.note
                .is_some_and(|note| note.starts_with("materialized constant"))
                && n.properties.values().any(|l| *l == Literal::Int(want))
        });
        assert!(
            baked,
            "expected the folded length {want} baked into a constant carrier for:\n{src}\nnodes: {:?}",
            r.module
                .nodes
                .values()
                .map(|n| (n.gate_class, n.note))
                .collect::<Vec<_>>()
        );
        assert!(
            !r.module.wires.iter().any(|w| {
                matches!(w.source.port, WirePort::MapVarRef | WirePort::ArrayVarRef)
                    && r.module.nodes.get(&w.target.node_id).is_some_and(|n| {
                        n.gate_class == crate::ir::gate_class::MICROCHIP_OUTPUT
                    })
            }),
            "a container REF reached an output port, which is the silent miscompile:\n{src}"
        );
    }
}

/// A `const`-receiver container read that CANNOT fold (a runtime key) has no
/// pure-context form: there is no exec chain to sequence the gate on. It must
/// say so. Handing back the receiver's own ref port instead makes the container
/// REFERENCE stand in for the value, with no diagnostic anywhere - typecheck
/// exempted the read (`container_call_exec_exempt`) expecting a fold that could
/// not happen. The array path already reports; the map path did not.
#[test]
fn an_unfoldable_const_container_read_in_pure_context_is_reported() {
    for src in [
        "const m = { \"a\": 1, \"b\": 2 }\nin k: string\nout o = m.get(k)",
        "const m = { \"a\": 1 }\nin k: string\nout o = m.has(k)",
        "const a = [1, 2, 3]\nin x: int\nout o = a.find(x)",
    ] {
        let r = compile(src);
        assert!(
            !r.diagnostics.is_empty(),
            "an unfoldable const container read in pure context must be reported:\n{src}"
        );
        assert!(
            !r.module.wires.iter().any(|w| {
                matches!(w.source.port, WirePort::MapVarRef | WirePort::ArrayVarRef)
                    && r.module
                        .nodes
                        .get(&w.target.node_id)
                        .is_some_and(|n| n.gate_class == crate::ir::gate_class::MICROCHIP_OUTPUT)
            }),
            "the container REFERENCE must not stand in for the value:\n{src}"
        );
    }
}
