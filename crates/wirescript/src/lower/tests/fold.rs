use super::*;
use crate::ir::gate_class as gc;

fn no_errors(r: &LowerResult) {
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
}
fn count_class(m: &crate::ir::Module, class: &str) -> usize {
    let mut n = m.nodes.values().filter(|x| x.gate_class == class).count();
    for c in m.chips.values() { n += count_class(c, class); }
    n
}
// Kept for future fold tasks (Task 4+) that check a surviving standalone
// literal directly; unused by the current test set (see
// `annihilator_folds_with_unknown_side` for why a bare literal often doesn't
// survive the full pipeline when it feeds a dataless boundary port).
#[allow(dead_code)]
fn literal_values(m: &crate::ir::Module) -> Vec<crate::ir::Literal> {
    let mut out: Vec<_> = m.nodes.values()
        .filter(|n| n.gate_class == gc::LITERAL)
        .filter_map(|n| n.properties.get(&*crate::intern::sym::VALUE).cloned())
        .collect();
    for c in m.chips.values() { out.extend(literal_values(c)); }
    out
}

#[test]
fn folds_arithmetic_chain_to_fixpoint() {
    // (2 + 3) * 4 == 20 -> single literal true; no Math/Compare gates left.
    let r = compile_folded("out y = (2 + 3) * 4 == 20");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 0);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathMultiply"), 0);
    assert_eq!(count_class(&r.module, gc::COMPARE_EQUAL), 0);
}

#[test]
fn unfolded_helper_keeps_real_gates() {
    // The structural helper must NOT fold (guards the whole existing suite).
    let r = compile("out y = 2 + 3");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
}

#[test]
fn opaque_blocks_folding() {
    // A bare-literal `Opaque(2)` argument hits an unrelated, pre-existing
    // materialization path (Rerouter has no data struct, so
    // `materialize_unfoldable_constants` wraps the literal `2` in its OWN
    // synthetic MathAdd carrier — see `materialize_unfoldable_constants` in
    // lower/mod.rs) that adds a second MathAdd independent of this pass.
    // Route the opaque value through a `var` instead so the Rerouter's input
    // is never itself a `_Literal`, isolating what THIS test checks: the
    // `+ 3` must not fold.
    let r = compile_folded("var x: int = 2\nout y = Opaque(x) + 3");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
    assert_eq!(count_class(&r.module, gc::REROUTER), 1);
}

#[test]
fn nofold_subtree_is_not_folded_or_seen_through() {
    // The @nofold add must stay a real gate AND its downstream consumer must
    // not fold either (its input is Unknown, not the constant 5).
    let r = compile_folded("@nofold let a = 2 + 3\nout y = a * 2");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathMultiply"), 1);
}

#[test]
fn annihilator_folds_with_unknown_side() {
    // As in `opaque_blocks_folding`: route through a `var` so Opaque's own
    // argument isn't a bare literal (which would materialize its own,
    // unrelated carrier gate — see that test's comment).
    // Opaque cond is Unknown, but AND false is a certified annihilator.
    let r = compile_folded("var b: bool = true\nout y = Opaque(b) && false");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_LogicalAND"), 0);
    // The folded `Bool(false)` feeds `out y` directly — a dataless boundary
    // port (MicrochipOutput has no data struct) — so
    // `materialize_unfoldable_constants` re-wraps it in a fresh, fully-baked
    // (no incoming wire) LogicalOR carrier rather than leaving a standalone
    // `_Literal` node (same interaction documented in `opaque_blocks_folding`
    // and the cross-chip tests). Check the carrier's baked operand instead of
    // scanning for a bare literal that the pipeline no longer leaves behind.
    let folded_to_false = r.module.nodes.values().any(|n| {
        n.gate_class == "BrickComponentType_WireGraph_Expr_LogicalOR"
            && n.properties.get(&*crate::intern::sym::B_INPUT_A)
                == Some(&crate::ir::Literal::Bool(false))
    });
    assert!(folded_to_false, "annihilator must compute false, not true (or stay unresolved)");
    // OR-true has no false-annihilator: `Opaque(x) || false` must NOT fold.
    // Baseline is 2 real LogicalOR gates, not 1, REGARDLESS of this pass (the
    // `compile()`-unfolded baseline has the same shape): the game's
    // LogicalOR gate can't hold its BInputB as an inline data default, so
    // even the literal `false` RHS operand materializes its own carrier
    // LogicalOR (same mechanism as the Bool case in
    // `materialize_unfoldable_constants`). What matters here is that this
    // count doesn't DROP to 0/1-collapsed-to-a-literal — it must equal the
    // unfolded baseline exactly, proving the outer `||` stayed real.
    let r2 = compile_folded("var b: bool = true\nout y = Opaque(b) || false");
    no_errors(&r2);
    assert_eq!(count_class(&r2.module, "BrickComponentType_WireGraph_Expr_LogicalOR"), 2);
}

// `uncovered_signature_stays_unfolded` (brief: `out y = "a" + 1`) is deleted
// per the brief's own inline fallback: `"a" + 1` never reaches lowering as a
// real MathAdd/concat gate at all — it hits an unrelated, pre-existing "IR
// lowering not yet supported for this expression" placeholder (WSP001,
// `_Unsupported` node), so there is no gate left for this pass to (correctly)
// decline to fold. The refusal this test wanted to guard is already covered
// at the eval level by fold/eval.rs's `replay_every_certified_case` (asserts
// exactly 3 math-with-string refusals) and `uncovered_signatures_refuse`.

#[test]
fn overflow_and_nonfinite_results_stay_unfolded() {
    let r = compile_folded("out y = 9223372036854775807 + 1");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
    let r2 = compile_folded("out y = 1.0 / 0.0");
    no_errors(&r2);
    assert_eq!(count_class(&r2.module, "BrickComponentType_WireGraph_Expr_MathDivide"), 1);
}

#[test]
fn string_literal_equality_folds() {
    // Regression for a `--fold-diff` differential-fuzzer finding: a bare
    // string literal lowers to a PORTLESS `String_Concatenate` carrier (see
    // `lower/expr.rs::literal_node` — "String literals can't be inlined as
    // wire_graph_variant immediate values on consumer gates"), never
    // `_Literal`. `collect_infos`/`try_resolve` only ever recognized
    // `_Literal` as a known value, so this carrier was invisible to the
    // pass: a wholly-constant string comparison like `"hello" == "world"`
    // never folded at all, even though string equality IS certified
    // (`fold/eval.rs`'s `eq()` handles `Str`/`Str` directly). Fixed by
    // teaching `collect_infos` to also recognize the portless-carrier shape.
    let r = compile_folded("out y = (\"hello\" == \"world\")");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::COMPARE_EQUAL), 0);
}

#[test]
fn string_literal_folds_across_chip_boundary() {
    // Same bug, minimized from the fuzzer's actual repro: the string
    // literal crosses a chip boundary (wired into a MicrochipInput, itself
    // fed by a portless `String_Concatenate` carrier) before being
    // compared. Must still fold — `count_class` recurses into chips, so
    // this also proves the in-chip `CompareEqual` collapsed.
    let src = "chip F(p: string) -> (r: bool) {\n  out r = (p == \"nope\")\n}\nout y = F(\"hello world\")";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::COMPARE_EQUAL), 0);
}

#[test]
fn cross_chip_constant_folds_inside_named_chip() {
    // The call argument must NOT be a bare literal AST node (`Inc(4)`) — a
    // pre-existing, pre-Task-3 optimization (`const_arg_literal` in
    // lower/call.rs) already special-cases a syntactic literal argument by
    // inlining it directly into the chip instance, bypassing the
    // MicrochipInput wire entirely, which would make this test pass without
    // ever exercising the NEW cross-chip VALUE propagation this task adds.
    // `2 + 2` forces the real MicrochipInput+wire path; its value only
    // becomes known via this pass's own fold-then-propagate fixpoint.
    let src = "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nout y = Inc(2 + 2)";
    let r = compile_folded(src);
    no_errors(&r);
    let child = r.module.chips.values().next().expect("one Inc instance");
    // The chip's own `v` boundary node must go unused (no outgoing wire) —
    // proof `v + 1` was resolved at compile time from the propagated
    // constant, not by reading `v`'s wire.
    let mc_input = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .expect("chip has a v boundary node");
    assert!(
        !child.wires.iter().any(|w| w.source.node_id == mc_input.id),
        "v must be unused after folding v + 1 at compile time"
    );
    // `r`'s dataless output-boundary port (MicrochipOutput has no data
    // struct) can't hold an inlined literal, so `materialize_unfoldable_constants`
    // re-wraps the folded `_Literal(5)` in a fresh, fully-unwired MathAdd
    // carrier of the SAME class as the original `v + 1` — same-class survival
    // here is expected, not a sign folding failed. The proof folding
    // happened: the surviving add has NO incoming wire (both operands baked)
    // and its baked InputA is the correctly pre-computed sum, 5.
    let surviving_add = child
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .expect("materialized carrier for the folded r");
    assert!(
        !child.wires.iter().any(|w| w.target.node_id == surviving_add.id),
        "the folded add must be fully baked (no wired operand)"
    );
    assert_eq!(
        surviving_add.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(5)),
        "(2 + 2) + 1 must fold to the baked constant 5"
    );
    assert!(r.module.chips.values().all(|c| c.template_key.is_none()),
        "mutated chip instance must drop its template_key");
}

#[test]
fn folded_result_flows_back_out_of_chip() {
    // Inc(4) = 5 propagates back through the chip output boundary and folds
    // the OUTER compare too. `Inc(4)` uses a bare-literal argument here
    // (unlike the boundary-propagation test above) — that's fine: this test
    // targets the OUTPUT-side pass-through specifically, which is exercised
    // regardless of how `v` got its value inside the chip.
    let src = "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nout y = Inc(4) == 5";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::COMPARE_EQUAL), 0);
}

#[test]
fn no_fold_flag_matches_unfolded_compile() {
    // lower() with fold_mode: ForceOff must equal the pre-pass structure.
    let folded_off = compile("out y = 2 + 3"); // helper passes fold_mode: ForceOff
    assert_eq!(count_class(&folded_off.module,
        "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
}

#[test]
fn module_level_nofold_disables_pass() {
    // `compile_folded` is ForceOn — @nofold must still win over it.
    let r = compile_folded("@nofold\n\nout y = 2 + 3");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
}

#[test]
fn module_level_fold_enables_pass_under_auto() {
    // Same source as `folds_arithmetic_chain_to_fixpoint`: a chain that fully
    // collapses to a literal `true`, leaving zero Math/Compare gates behind
    // when folded. A bare `2 + 3` directly feeding a dataless `out` boundary
    // is the WRONG probe here — `materialize_unfoldable_constants` re-wraps
    // even a successfully-folded int result in a same-shaped baked carrier
    // gate (see `select_with_unwired_chosen_input_stays`), so a MathAdd
    // count would stay 1 either way and never distinguish folded from
    // unfolded. Auto only folds when the entry file opts in with a
    // module-level `@fold` — this is the default flip itself.
    let annotated = compile_auto("@fold\n\nout y = (2 + 3) * 4 == 20");
    no_errors(&annotated);
    assert_eq!(
        count_class(&annotated.module, gc::COMPARE_EQUAL),
        0,
        "@fold under Auto must run the pass"
    );

    let unannotated = compile_auto("out y = (2 + 3) * 4 == 20");
    no_errors(&unannotated);
    assert_eq!(
        count_class(&unannotated.module, gc::COMPARE_EQUAL),
        1,
        "no @fold under Auto must NOT run the pass (the default flip)"
    );
}

#[test]
fn module_nofold_beats_fold() {
    // Both module-level annotations stacked, `@nofold` first — the parser's
    // collect_module_annotations run supports either order (each annotation
    // just needs to be alone on its own line, with the whole run separated
    // from the first decl by a blank line, same as `@nofold` alone).
    // Same source/reasoning as `module_level_fold_enables_pass_under_auto`:
    // a chain feeding a dataless boundary that collapses to a literal `true`
    // when folded (a bare `2 + 3` int result would get re-wrapped in a
    // same-shaped baked carrier either way and never distinguish folded from
    // unfolded — see that test's comment).
    let src = "@nofold\n@fold\n\nout y = (2 + 3) * 4 == 20";
    let parsed = crate::parser::parse(src, "test");
    assert!(
        parsed.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "parse errors: {:?}",
        parsed.diagnostics
    );
    assert!(
        parsed.diagnostics.iter().any(|d| d.severity == crate::diagnostic::Severity::Warning
            && d.message.contains("conflict")),
        "expected a module-level @fold/@nofold conflict warning, got: {:?}",
        parsed.diagnostics
    );
    assert!(parsed.ast.fold, "the @fold annotation should still be recorded");
    assert!(parsed.ast.no_fold, "@nofold should win but is still recorded on the AST");

    let tc = crate::typecheck::typecheck(&parsed.ast, "test");
    let lower_with = |fold_mode: FoldMode| {
        lower(LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode,
        })
    };

    // Auto: ast.fold && !ast.no_fold == true && false == false -> no fold.
    let r_auto = lower_with(FoldMode::Auto);
    no_errors(&r_auto);
    assert_eq!(
        count_class(&r_auto.module, gc::COMPARE_EQUAL),
        1,
        "@nofold must beat @fold under Auto"
    );

    // ForceOn still respects @nofold ("don't require @fold", not "ignore @nofold").
    let r_force_on = lower_with(FoldMode::ForceOn);
    no_errors(&r_force_on);
    assert_eq!(
        count_class(&r_force_on.module, gc::COMPARE_EQUAL),
        1,
        "@nofold must beat @fold under ForceOn too"
    );
}

#[test]
fn fold_adjacent_to_doc_comment_emits_module_level_only_error() {
    // @fold directly adjacent to a module doc comment (no blank line separating them)
    // is not recognized as module-level. The parser treats it as a directive-level
    // @fold and emits a module-level-only error message.
    let src = "/// doc\n@fold\n\nout y = 1";
    let parsed = crate::parser::parse(src, "test");

    // Diagnostic contains the module-level-only message.
    assert!(
        parsed.diagnostics.iter().any(|d| d.message.contains("module-level only")),
        "expected module-level-only diagnostic, got: {:?}",
        parsed.diagnostics
    );

    // @fold should not be recorded on the AST (it was not recognized as module-level).
    assert!(!parsed.ast.fold, "@fold adjacent to doc comment should not be recognized as module-level");
}

#[test]
fn imported_module_fold_is_inert() {
    // A module-level `@fold` only takes effect for the file `resolve()` was
    // entered on — resolve.rs never reads an imported file's own
    // `Script.fold` when merging its declarations in (mirrors the existing
    // entry-only `@nofold` semantics). An entry file with no `@fold` of its
    // own must NOT fold under Auto, even when everything it imports opts in.
    let lib = "@fold\n\nmod inc(v: int) -> int {\n  return v + 1\n}";
    let main = "import { inc } from \"lib\"\nout y = inc(2) + 3";
    let r = compile_multi(main, &[("lib", lib)]);
    no_errors(&r);
    // Unfolded: `inc(2)` inlines to a real `2 + 1` MathAdd, then `+ 3` is a
    // second MathAdd. A folded result would collapse to a single baked
    // carrier gate instead.
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"),
        2,
        "the imported file's @fold must be inert for an unannotated entry file"
    );
}

#[test]
fn select_shorts_to_nonconstant_source() {
    // Constant-true selector passes the THEN side (InputB) through even
    // though that side is opaque; the Select gate itself disappears.
    let r = compile_folded("let v = Opaque(9)\nout y = if true then v else 3");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::SELECT), 0);
    assert_eq!(count_class(&r.module, gc::REROUTER), 1, "opaque source survives");
}

#[test]
fn select_with_unwired_chosen_input_stays() {
    // Falsy selector chooses InputA; if nothing drives it, refuse.
    // (Constructed via IR is awkward from source; assert instead that a
    // fully-constant select folds to its value.)
    let r = compile_folded("out y = if false then 1 else 2");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::SELECT), 0);
    // `out y`'s port (MicrochipOutput's RER_Input) has no data struct, so
    // `materialize_unfoldable_constants` re-wraps the shorted `_Literal(2)`
    // in a fresh, fully-baked (no incoming wire) MathAdd carrier rather than
    // leaving a standalone `_Literal` node — same interaction documented in
    // `annihilator_folds_with_unknown_side`. Check the carrier's baked
    // operand instead of scanning for a bare literal the pipeline no longer
    // leaves behind.
    let folded_to_two = r.module.nodes.values().any(|n| {
        n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd"
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::Int(2))
            && !r.module.wires.iter().any(|w| w.target.node_id == n.id)
    });
    assert!(folded_to_two, "falsy select must short to the else value 2, not stay unresolved");
}

#[test]
fn branch_truncates_untaken_side() {
    // A non-literal-AST condition (`1 == 1`) is used instead of a bare
    // `true`: `lower_if` already special-cases a literal-bool `s.cond`
    // (skips the Branch gate entirely, an unrelated pre-existing lowering
    // shortcut — see stmt.rs), so `if true {...}` never reaches THIS pass
    // at all, regardless of Branch truncation being implemented. `1 == 1`
    // forces a real `CompareEqual` gate feeding a real `Branch`, and the
    // condition only becomes Known via this pass's own fixpoint.
    let src = "in t: exec\nvar a: int = 0\nvar b: int = 0\n\
               on t { if 1 == 1 { a = 1 } else { b = 2 } }";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::BRANCH), 0, "constant branch removed");
    // Exactly one Var_Set survives (the taken side's `a = 1`).
    assert_eq!(count_class(&r.module, gc::VAR_SET), 1);
}

#[test]
fn branch_dead_side_with_external_exec_survives() {
    // The else-side chain is also reachable from a second handler -> the
    // shared tail must NOT be deleted. See `branch_truncates_untaken_side`
    // for why the condition is `1 == 1` rather than a bare `true`.
    let src = "in t: exec\nin u: exec\nvar a: int = 0\nvar b: int = 0\n\
               let tail: exec\n\
               on t { if 1 == 1 { a = 1 } else { emit tail } }\n\
               on u { emit tail }\n\
               on tail { b = 2 }";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::BRANCH), 0);
    assert_eq!(count_class(&r.module, gc::VAR_SET), 2, "shared tail survives");
}

#[test]
fn nofold_branch_stays() {
    let src = "in t: exec\nvar a: int = 0\n\
               @nofold on t { if 1 == 1 { a = 1 } else { a = 2 } }";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::BRANCH), 1);
}

#[test]
fn nofold_literal_condition_branch_stays() {
    // `if true {...} else {...}` normally takes `lower_if`'s literal-bool
    // shortcut (stmt.rs) and never emits a Branch gate at all — LOWERING
    // TIME, before this fold pass ever runs. That shortcut must NOT fire
    // under `@nofold`, which promises "nothing folded or elided" (a
    // @nofold handler that silently lost its Branch would violate the
    // guarantee even though this pass itself never touched anything).
    // Checked via BOTH lowering helpers: `compile` (fold_mode: ForceOff — the
    // unfolded structural microscope, which isolates the stmt.rs shortcut
    // itself from this pass) and `compile_folded` (fold_mode: ForceOn,
    // exercising the pass directly, which additionally proves the fold
    // pass's own `_nofold` barrier leaves the surviving Branch alone). A
    // real Branch with both arms intact must survive either way once
    // `@nofold` is in scope.
    let src = "in t: exec\nvar a: int = 0\nvar b: int = 0\n\
               @nofold on t { if true { a = 1 } else { b = 2 } }";
    let unfolded = compile(src);
    no_errors(&unfolded);
    assert_eq!(
        count_class(&unfolded.module, gc::BRANCH),
        1,
        "compile (ForceOff helper) must keep a real Branch under @nofold"
    );
    assert_eq!(count_class(&unfolded.module, gc::VAR_SET), 2, "both arms lower under @nofold");

    let folded = compile_folded(src);
    no_errors(&folded);
    assert_eq!(
        count_class(&folded.module, gc::BRANCH),
        1,
        "compile_folded must also keep a real Branch under @nofold"
    );
    assert_eq!(count_class(&folded.module, gc::VAR_SET), 2, "both arms survive the fold pass too");
}

#[test]
fn literal_condition_shortcut_fires_without_nofold() {
    // Baseline (no @nofold): `if true {...} else {...}` still takes the
    // pre-existing lowering shortcut and emits no Branch at all — confirms
    // the `nofold_depth == 0` guard added for the test above didn't change
    // behavior outside `@nofold`.
    let src = "in t: exec\nvar a: int = 0\nvar b: int = 0\n\
               on t { if true { a = 1 } else { b = 2 } }";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::BRANCH), 0);
    assert_eq!(count_class(&r.module, gc::VAR_SET), 1, "only the taken arm's Var_Set exists");
}

#[test]
fn nofold_ident_bound_literal_condition_branch_stays() {
    // The ident-bound-to-literal shortcut site in lower_if (let c = true;
    // if c) must also respect @nofold — a consolidation of the two guard
    // sites that drops one nofold_depth check would reopen the guarantee
    // hole for this common shape without failing any test.
    let src = "in t: exec\nvar a: int = 0\nvar b: int = 0\n\
               @nofold on t { let c = true\nif c { a = 1 } else { b = 2 } }";
    let r = compile(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::BRANCH), 1);
    let r2 = compile_folded(src);
    no_errors(&r2);
    assert_eq!(count_class(&r2.module, gc::BRANCH), 1);
}

#[test]
fn multi_hop_branch_truncation_chains() {
    // Nested else-if where BOTH conditions fold: the outer Branch truncates
    // to its else side, which itself contains a second Branch that also
    // truncates (to its then side, `a = 2`). Locks in that chained
    // truncation resolves all the way through — zero Branch/CompareEqual
    // debris left anywhere in the chain, exactly one surviving Var_Set.
    let src = "in t: exec\nvar a: int = 0\n\
               on t { if 1 == 2 { a = 1 } else { if 2 == 2 { a = 2 } else { a = 3 } } }";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::BRANCH), 0, "both branches truncate away");
    assert_eq!(count_class(&r.module, gc::COMPARE_EQUAL), 0, "both compares fold to constants");
    assert_eq!(count_class(&r.module, gc::VAR_SET), 1, "only a = 2 survives");
}

#[test]
fn demand_sweep_removes_orphaned_feeders() {
    // The annihilator kills the AND; the pure NOT feeding its unknown side
    // loses its only consumer and sweeps away; the Opaque rerouter STAYS.
    let r = compile_folded("out y = !Opaque(true) && false");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_LogicalAND"), 0);
    assert_eq!(count_class(&r.module, gc::LOGICAL_NOT), 0, "orphaned NOT swept");
    assert_eq!(count_class(&r.module, gc::REROUTER), 1, "Opaque never elided");
}

#[test]
fn emptied_unannotated_chip_is_elided() {
    // Anon-chip-with-out syntax (`chip { ... }` used as an expression) never
    // reaches `module.chips` at fold time — `partition_anon_chips` (which
    // creates that entry) runs AFTER this whole pass, so a fully-folded
    // unannotated anon chip's tagged nodes just get swept like any other
    // dead nodes, and partition never creates a child for it at all (see
    // the module doc on `elide_empty_chips`). Use a NAMED chip instance
    // instead — those already live in `module.chips` during lowering
    // (`lower/call.rs`), so this actually exercises whole-chip elision.
    let r = compile_folded("chip Empty() { let dead = 2 + 3 }\nEmpty()\nout y = 1");
    no_errors(&r);
    assert!(r.module.chips.is_empty(), "unannotated empty chip must be elided");
    assert_eq!(count_class(&r.module, gc::MICROCHIP), 0);
}

#[test]
fn annotated_empty_chip_is_kept() {
    let r = compile_folded("@label(\"Keep\") chip Empty() { let dead = 2 + 3 }\nEmpty()\nout y = 1");
    no_errors(&r);
    // The labeled chip node survives even though its contents folded away.
    let chip_nodes = r.module.nodes.values()
        .filter(|n| n.gate_class == gc::MICROCHIP || n.gate_class == gc::MICROCHIP_ALT)
        .count();
    assert!(chip_nodes >= 1, "@label chip must be retained");
    assert_eq!(r.module.chips.len(), 1, "labeled empty chip's module entry survives");
}

// --- Task 3 review follow-up: `inline_orphan_literals` cross-module guard ---

#[test]
fn cross_module_literal_inline_skips_orphan_deletion() {
    // Regression for a pre-existing silent miscompile that this feature
    // AMPLIFIES: `inline_orphan_literals` (lower/mod.rs) finds a
    // single-consumer `_Literal` and writes its value into the consumer's
    // data via `module.nodes.get_mut(&target_id)` — but when the consumer
    // lives in a DIFFERENT module (a chip's `MicrochipInput` fed by a
    // parent-side literal), that lookup silently no-ops while the literal
    // AND its wire still get deleted, so the boundary port reads 0 at
    // runtime with zero diagnostics. `2 + 2` (not a bare literal) forces the
    // call argument through the real MicrochipInput+wire path — bypassing
    // `const_arg_literal`'s syntactic inlining — and only becomes a
    // `_Literal` via THIS pass, reproducing the exact cross-module
    // single-consumer-literal shape the bug hit.
    let src = "chip Passthrough(v: int) -> (r: int) { out r = v + Opaque(0) }\n\
               out y = Passthrough(2 + 2)";

    let folded = compile_folded(src);
    no_errors(&folded);
    let child = folded.module.chips.values().next().expect("one Passthrough instance");
    let mc_input = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .expect("chip has a v boundary node");
    // The fixed pipeline's actual shape: `materialize_unfoldable_constants`
    // converts the parent-side `_Literal(4)` into a real carrier gate
    // (`MathAdd(4, 0)`) wired into the MicrochipInput — a raw literal-source
    // wire into a cross-module pin has NO emit-time delivery mechanism (a
    // rerouter pin has no data field to inline into, and literal-source
    // wires are skipped at emit), so only the carrier shape actually
    // reaches the game. (`inline_orphan_literals`' cross-module guard keeps
    // the literal alive until the carrier pass runs.)
    let feeder_id = folded
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == mc_input.id)
        .map(|w| w.source.node_id)
        .expect("v's MicrochipInput must still have an incoming wire, not be orphaned");
    let feeder = folded
        .module
        .nodes
        .get(&feeder_id)
        .expect("feeder wire must reference a live node");
    assert_eq!(
        feeder.gate_class,
        "BrickComponentType_WireGraph_Expr_MathAdd",
        "surviving feeder must be the materialized constant carrier"
    );
    assert_eq!(
        feeder.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(4)),
        "v must receive the correctly-folded (2 + 2) = 4, not silently read 0"
    );
    assert_eq!(
        feeder.properties.get(&*crate::intern::sym::INPUT_B),
        Some(&crate::ir::Literal::Int(0)),
        "carrier's second operand must be the identity 0"
    );

    // Unfolded: `2 + 2` stays a real (same-module) MathAdd — its own operand
    // literals `2`/`2` are inlined same-module (never cross-module), so this
    // path was never exposed to the bug. Confirmed here as a baseline: the
    // value must reach `v` here too.
    let unfolded = compile(src);
    no_errors(&unfolded);
    let child2 = unfolded.module.chips.values().next().expect("one Passthrough instance");
    let mc_input2 = child2
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .expect("chip has a v boundary node");
    assert!(
        unfolded.module.wires.iter().any(|w| w.target.node_id == mc_input2.id),
        "unfolded path must also deliver v's value via a live wire"
    );
}
