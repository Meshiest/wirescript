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
fn any_annotation_blocks_folding() {
    // `in t: any` types the port as `Type::Opaque` (the same wildcard type
    // `Opaque(...)` produces), so it must be just as opaque to the fold
    // pass as a real `Opaque(...)` probe: `t + 1` must stay a real gate
    // instead of folding away.
    let r = compile_folded("in t: any\nlet v = t + 1\nout y = v");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
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
    // ever exercising the cross-chip VALUE propagation this task adds.
    // `2 + 2` forces the real MicrochipInput+wire path; its value only
    // becomes known via this pass's own fold-then-propagate fixpoint.
    let src = "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nout y = Inc(2 + 2)";
    let r = compile_folded(src);
    no_errors(&r);
    // Dead-feed pruning (boundary-delivery cleanup, Rule A + Rule B): `v`
    // folds away inside the chip (nothing left reads its wire — the SAME
    // proof the pre-cleanup version of this test checked), and `r`'s only
    // exterior consumer (root `y`) gets rewired by Rule B straight to a
    // fresh literal, so `r` ALSO ends up wire-free. With BOTH of Inc's
    // boundary nodes at zero incoming AND zero outgoing wires, the whole
    // instance is wire-free, carries no `@label`/`@closed`/doc annotation,
    // and qualifies for `elide_empty_chips`'s whole-chip removal — NO
    // trace of the call survives: not the chip instance, not a
    // `v`-argument carrier, not an `r`-result carrier.
    assert!(
        r.module.chips.is_empty(),
        "the fully-folded, now wire-free Inc instance must be elided entirely, not just \
         emptied — chips: {:?}",
        r.module.chips.keys().collect::<Vec<_>>()
    );
    // `y` is itself a dataless ROOT-level output boundary (this pass never
    // touches the root script's own `in`/`out` — see
    // `collect_call_boundary_ids`'s doc), so it still needs SOME real gate
    // to deliver its value: exactly one MathAdd carrier, and nothing else
    // survives anywhere in the module (in particular, no SECOND MathAdd for
    // the pruned `v`-argument feed).
    let adds: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .collect();
    assert_eq!(
        adds.len(),
        1,
        "expected exactly one surviving carrier (for `y` itself), found {}: {adds:?}",
        adds.len()
    );
    let carrier = adds[0];
    assert!(
        !r.module.wires.iter().any(|w| w.target.node_id == carrier.id),
        "the carrier must be fully baked (no wired operand) — proof `v + 1` folded at compile \
         time, not delivered by a live wire"
    );
    assert_eq!(
        carrier.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(5)),
        "(2 + 2) + 1 must fold to the baked constant 5"
    );
}

#[test]
fn chip_output_read_only_via_interpolation_leaves_no_boundary_carrier() {
    // Boundary-consumer rewire — mirrors the fold showcase's
    // `chip Score(...)` + interpolated-print shape (projects/tests/src/
    // test_fold.ws): `score`'s ONLY reader is a `${...}` interpolation
    // slot. `STRING_FORMAT_TEXT` is special-cased straight through `known`
    // (`try_resolve_format_text` reads the chip boundary's Known value
    // directly, no wire needed), so it already folds through the chip
    // boundary on its own merits — what THIS test guards is that once
    // FormatText collapses to a literal and drops its own feed wire (via
    // the pre-existing `rewrite_wires`), Rule A notices BOTH of Score's now
    // wire-free boundary nodes and drops their feeds too, instead of
    // leaving a `materialize_unfoldable_constants` carrier (a
    // `MathAdd`/`MathMultiply`/`MathModulo`-shaped gate) feeding a
    // boundary port nothing reads anymore.
    let src = "chip Score(base: int) -> (r: int) { out r = (base * base + 100) % 977 }\n\
               let score = Score(6 + 6)\n\
               out y = \"score=${score}\"";
    let r = compile_folded(src);
    no_errors(&r);
    assert_eq!(
        count_class(&r.module, gc::STRING_FORMAT_TEXT),
        0,
        "FormatText must fold entirely"
    );
    assert!(
        r.module.chips.is_empty(),
        "Score is fully folded and read only via the now-collapsed FormatText — both boundary \
         feeds must be gone and the wire-free, unannotated chip instance elided — chips: {:?}",
        r.module.chips.keys().collect::<Vec<_>>()
    );
    for class in [
        "BrickComponentType_WireGraph_Expr_MathAdd",
        "BrickComponentType_WireGraph_Expr_MathMultiply",
        "BrickComponentType_WireGraph_Expr_MathModulo",
    ] {
        assert_eq!(
            count_class(&r.module, class),
            0,
            "no leftover carrier/computation for {class} anywhere in the module"
        );
    }
    // The ONLY carrier left is `y`'s own: root `out` is a dataless boundary
    // too (this pass never touches the root script's own I/O — see
    // `collect_call_boundary_ids`'s doc), so its string still needs a real
    // gate — baked with the fully-resolved text, proof the folded chip
    // result actually reached the print (inlined as DATA, zero extra
    // gates for the interpolation itself).
    let expected = ((6 + 6) * (6 + 6) + 100) % 977;
    let carrier = r.module.nodes.values().find(|n| {
        n.gate_class == gc::STRING_CONCATENATE
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::String(format!("score={expected}")))
    });
    assert!(
        carrier.is_some(),
        "expected the interpolated print to bake \"score={expected}\""
    );
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
    // Boundary-delivery cleanup's actual shape now (Rule B, then Rule A):
    // `v`'s Known value (4) is delivered DIRECTLY to the real `v +
    // Opaque(0)` gate, bypassing the MicrochipInput rerouter — and the
    // `materialize_unfoldable_constants` carrier this test used to check
    // for entirely. Rule B rewires the wire straight to a fresh literal,
    // which (single-consumer, same-module as the real MathAdd it feeds)
    // then inlines as baked data via the ordinary `inline_orphan_literals`
    // pass, same as any hand-written constant operand. `v`'s own boundary
    // node ends up with ZERO wires at all — the cleanest possible outcome —
    // but the VALUE must still be there, correctly, on the real gate that
    // needed it (the bug this test guards against was the value going
    // missing across the module boundary, not the wire going missing).
    assert!(
        !child
            .wires
            .iter()
            .any(|w| w.source.node_id == mc_input.id || w.target.node_id == mc_input.id),
        "v's boundary node must end up completely wire-free once its value is delivered \
         directly to its consumer"
    );
    // Two `MathAdd` nodes exist in this chip: the real `v + Opaque(0)` gate,
    // and `Opaque(0)`'s OWN bare-literal argument carrier (an unrelated,
    // pre-existing materialization — Opaque's rerouter has no data struct
    // either, so its `0` argument gets the exact same carrier treatment,
    // see `opaque_blocks_folding`). Distinguish by wiring: the carrier is
    // fully baked (zero incoming wires), the real gate still has InputB
    // wired to the live Opaque rerouter.
    let live_add = child
        .nodes
        .values()
        .find(|n| {
            n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd"
                && child.wires.iter().any(|w| w.target.node_id == n.id)
        })
        .expect("the real v + Opaque(0) gate must survive (Opaque blocks it from folding)");
    assert_eq!(
        live_add.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(4)),
        "v must receive the correctly-folded (2 + 2) = 4, baked directly onto the surviving \
         gate — not silently read 0, and not left stranded on a dangling materialized carrier"
    );
    let wires_into_add: Vec<_> = child
        .wires
        .iter()
        .filter(|w| w.target.node_id == live_add.id)
        .collect();
    assert_eq!(
        wires_into_add.len(),
        1,
        "exactly one wire (InputB, from the live Opaque rerouter) should still feed the \
         surviving gate — InputA is now baked, not wired, and the Opaque barrier must survive \
         untouched: {wires_into_add:?}"
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

// --- Task 4: driver — FormatText folding, composite seeding/delivery ---

fn float_prop(n: &crate::ir::Node, name: &str) -> Option<f64> {
    match n.properties.get(&crate::intern::intern(name)) {
        Some(crate::ir::Literal::Float(f)) => Some(*f),
        _ => None,
    }
}

#[test]
fn format_text_constant_interpolation_folds() {
    // `let n = 42` — no comma grouping needed below 1,000.
    let r = compile_folded("let n = 42\nout y = \"n=${n}\"");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::STRING_FORMAT_TEXT), 0);
    let carrier = r.module.nodes.values().find(|n| {
        n.gate_class == gc::STRING_CONCATENATE
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::String("n=42".to_string()))
    });
    assert!(carrier.is_some(), "expected a materialized carrier baked with \"n=42\"");

    // `let n = 1000` — the certified render law comma-groups from 1,000 up
    // (`render_for_format`); this end-to-end test locks that law all the
    // way through FormatText template substitution, not just in isolation.
    let r2 = compile_folded("let n = 1000\nout y = \"n=${n}\"");
    no_errors(&r2);
    assert_eq!(count_class(&r2.module, gc::STRING_FORMAT_TEXT), 0);
    let carrier2 = r2.module.nodes.values().find(|n| {
        n.gate_class == gc::STRING_CONCATENATE
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::String("n=1,000".to_string()))
    });
    assert!(carrier2.is_some(), "expected a materialized carrier baked with \"n=1,000\" (comma law)");
}

#[test]
fn format_text_opaque_slot_does_not_fold() {
    // The `Fmt(...)` builtin (not `${...}` interpolation) so the substitution
    // slot is an explicit, typed `Opaque(...)` operand — mirrors the probe's
    // own armoring convention (see `probes/gate_semantics.ws`'s comment on
    // why FormatText's substitution slots are Opaque-wrapped there).
    let r = compile_folded("var x: int = 5\nout y = Fmt(\"n={0}\", Opaque(x))");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::STRING_FORMAT_TEXT), 1);
    assert_eq!(count_class(&r.module, gc::REROUTER), 1, "opaque source survives");
}

#[test]
fn format_text_nonascii_input_keeps_the_gate() {
    // Certified: the string family (including FormatText substitution)
    // refuses any non-ASCII string operand — the multibyte behavior was
    // never certified (see `eval::ascii_str`'s doc comment).
    let r = compile_folded("let s = \"\u{03c0}\"\nout y = \"v=${s}\"");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::STRING_FORMAT_TEXT), 1);
}

#[test]
fn vector_math_folds() {
    // MakeVector/MathAdd all disappear; the composite result is delivered
    // to the dataless `out v` boundary via a materialized MakeVector
    // carrier (baked X/Y/Z data, no wires) — same delivery mechanism
    // `materialize_unfoldable_constants` already used for a hand-written
    // `Vec(...)` literal.
    let r = compile_folded("out v = Vec(1.0, 2.0, 3.0) + Vec(0.5, 0.5, 0.5)");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 0);
    assert_eq!(count_class(&r.module, gc::MAKE_VECTOR), 1);
    let carrier = r.module.nodes.values().find(|n| n.gate_class == gc::MAKE_VECTOR)
        .expect("materialized MakeVector carrier for the folded vector sum");
    assert_eq!(float_prop(carrier, "X"), Some(1.5));
    assert_eq!(float_prop(carrier, "Y"), Some(2.5));
    assert_eq!(float_prop(carrier, "Z"), Some(3.5));
}

/// Regression for a `--fold-diff`-fuzzer-discovered bug (fold2 Task 5):
/// `Dot`/`Cross`/`ScaleVec` (and any other certified gate reached via a
/// builtin CALL, not a binary operator) take their Vector arguments as a
/// baked, UNWIRED node property when both operands are compile-time
/// literals (`lower/call.rs::literal_for_property_port` — the port's data
/// field accepts an inline `Vector` variant, so the arg is written directly
/// onto the CONSUMING gate's own properties, never wired through a separate
/// `_Literal`/`MakeVector` source). `fold/mod.rs`'s driver only ever
/// consulted WIRES (`resolve_input`) when deciding what a data input
/// resolves to, so a gate whose every operand arrived this way had an
/// all-`Unwired` signature — never certified, since the probe never tests
/// "every operand missing" — and could NEVER fold, no matter how
/// well-determined its value actually was. Fixed by `resolve_data_input`
/// (added in fold/mod.rs), which falls back to the node's own baked
/// property whenever a data port comes back `Unwired`. `Dot(Vec, Vec)` was
/// the exact minimal repro (`cargo run -p bearilog-cli -- compile --fold
/// --dump-ir` on a single `out y = Dot(Vec(1,0,0), Vec(0,1,0))` line kept
/// the `VecDotProduct` gate alive before the fix, folded it away after).
#[test]
fn call_argument_baked_vector_literals_fold_through_dot() {
    let r = compile_folded("out y = Dot(Vec(1.0, 2.0, 3.0), Vec(4.0, 5.0, 6.0))");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::VEC_DOT), 0);
    // A scalar Float result materializes via the `MathAdd(n, 0)` carrier
    // recipe (`materialize_unfoldable_constants`) for a dataless `out`
    // boundary, same delivery mechanism as any other folded float constant.
    let carrier = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .expect("materialized MathAdd carrier for the folded dot product");
    assert_eq!(float_prop(carrier, "InputA"), Some(32.0)); // 1*4 + 2*5 + 3*6
    assert_eq!(float_prop(carrier, "InputB"), Some(0.0));
}

/// Sibling of `call_argument_baked_vector_literals_fold_through_dot`: proves
/// the same fix also covers a Vector-RESULT gate (`Cross`), not just a
/// scalar-result one — delivered via the `MakeVector` carrier recipe
/// instead of `MathAdd`.
#[test]
fn call_argument_baked_vector_literals_fold_through_cross() {
    let r = compile_folded("out v = Cross(Vec(1.0, 0.0, 0.0), Vec(0.0, 1.0, 0.0))");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::VEC_CROSS), 0);
    assert_eq!(count_class(&r.module, gc::MAKE_VECTOR), 1);
    let carrier = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MAKE_VECTOR)
        .expect("materialized MakeVector carrier for the folded cross product");
    assert_eq!(float_prop(carrier, "X"), Some(0.0));
    assert_eq!(float_prop(carrier, "Y"), Some(0.0));
    assert_eq!(float_prop(carrier, "Z"), Some(1.0));
}

/// Documents the SAME `resolve_data_input` mechanism one level further in:
/// `Vec(...)`'s OWN X/Y/Z params go through the identical
/// `literal_for_property_port` baked-vs-wired split as any other certified
/// CALL's arguments (a bare-literal component bakes onto MakeVector's own
/// properties with no wire; a computed-but-foldable one wires normally and
/// resolves once its own source folds). Before the fix, this MakeVector
/// call never fully folded (X/Y invisible to the driver as `Unwired`, an
/// uncertified all-unknown-ish signature); after, all three resolve and it
/// folds like any other fully-known composite constructor. NOTE: this exact
/// shape (2 literal + 1 arithmetic component) also appears in
/// `probes/gate_semantics.ws`'s `renderVec` render-showcase call, feeding
/// an `Opaque(...)`-wrapped argument — since `Vec(...)`'s construction now
/// also folds away there, `tests/fold_invariants.rs::probe_is_fold_invariant`
/// picks up a 1-wire structural delta (2072n/3204w unfolded vs.
/// 2072n/3203w folded) even though the probe's OBSERVABLE behavior is
/// unchanged (`Opaque`'s own wire stays real; `MakeVector` reads whatever's
/// at its ports, wired-computed or baked-default, per this whole feature's
/// foundational "unwired input reads its own data default" law — the same
/// one the composite/scalar carriers throughout this file already rely on).
/// Reconciling the probe's structural invariant requires probe-side armor
/// (e.g. wrapping `Vec(...)`'s individual components in `Opaque(...)`,
/// mirroring `compositeMakeCases`'s pattern) — out of this task's file
/// scope (`probes/` is read-only here); flagged in the task report instead
/// of silently worked around.
#[test]
fn mixed_literal_and_computed_vector_components_fold() {
    let r = compile_folded("out v = Vec(0.5, -1.25, 1.0 / 3.0)");
    no_errors(&r);
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathDivide"), 0);
    assert_eq!(count_class(&r.module, gc::MAKE_VECTOR), 1);
    let carrier = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MAKE_VECTOR)
        .expect("materialized MakeVector carrier for the fully-folded vector");
    assert_eq!(float_prop(carrier, "X"), Some(0.5));
    assert_eq!(float_prop(carrier, "Y"), Some(-1.25));
    assert!(float_prop(carrier, "Z").is_some_and(|z| (z - 1.0 / 3.0).abs() < 1e-9));
}

#[test]
fn composite_value_folds_through_a_non_math_gate() {
    // Wirescript's `==` operator isn't overloaded for Vector/Rotator/Quat/
    // Color (only the scalar/variant-able types — see
    // `catalog/operators.rs::compare_binary`), so a source-level "composite
    // EQ" expression doesn't exist to test. `ColorToHex` exercises the same
    // driver capability instead — a certified, non-Math gate whose ONE
    // input is a composite (`Color`) value and whose OUTPUT is a plain
    // scalar (`string`) — proving composite `known`-propagation drives
    // folding through gates beyond MathAdd/VecScale.
    let r = compile_folded("out y = Color(1.0, 0.5, 0.0).ToHex()");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::COLOR_TO_HEX), 0);
    let carrier = r.module.nodes.values().find(|n| {
        n.gate_class == gc::STRING_CONCATENATE
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::String("FFBC00".to_string()))
    });
    assert!(carrier.is_some(), "expected a materialized carrier baked with \"FFBC00\"");
}

#[test]
fn vec_normalize_deferred_does_not_fold() {
    // `VecNormalize` is certified but deliberately never folded (the
    // `deferredOps` chapter — see `eval::eval`'s `DEFERRED` list) even
    // though its Vector operand is fully known.
    let r = compile_folded("out v = Vec(1.0, 2.0, 3.0).Normalize()");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::VEC_NORMALIZE), 1);
}

#[test]
fn make_rotation_does_not_fold() {
    // Arithmetic (not bare literals) for every arg forces a REAL wired
    // MakeRotation gate (a bare-literal `Rotation(...)` call would instead
    // fold to a `_Literal(Rotator)` at AST-lowering time — see
    // `lower/predeclare.rs::expr_to_literal` — bypassing the gate this test
    // targets entirely). Each MathAdd operand still folds and inlines, but
    // MakeRotation itself must stay real: its only table evidence renders
    // blank (`BLANK_RENDER_REFUSED` in `eval::eval`), so it never folds in
    // production regardless of how determined its inputs are.
    let r = compile_folded(
        "out r: rotator = Rotation(0.0 + 0.0, 90.0 + 0.0, 45.5 + 0.0)",
    );
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::MAKE_ROTATION), 1);
    // Each `X.0 + 0.0` operand DOES still fold (to a `_Literal(Float)`), but
    // MakeRotation's Pitch/Yaw/Roll fields don't accept inlined data (unlike
    // e.g. a native `Vector`/`Rotator`/`Quat` struct field — see
    // `emit::port_accepts_inline_variant`), so each folded operand
    // re-materializes as a fully-baked (no incoming wire), same-class
    // MathAdd(n, 0) carrier — the SAME mechanism proven by
    // `cross_chip_constant_folds_inside_named_chip`. What matters here is
    // MakeRotation itself: it must never become a `_Literal`.
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 3);
    let all_baked = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .all(|n| !r.module.wires.iter().any(|w| w.target.node_id == n.id));
    assert!(all_baked, "each operand's carrier must be fully baked (no wired operand)");
}

#[test]
fn string_concat_operator_form_folds() {
    // The `..` operator's two bare string-literal operands are each the
    // portless `String_Concatenate` carrier shape (see `collect_infos`'s
    // `Info.lit` comment) — both fold via the certified `Concatenate` law
    // into a single materialized carrier.
    let r = compile_folded("out y = (\"a\" .. \"b\")");
    no_errors(&r);
    let carrier = r.module.nodes.values().find(|n| {
        n.gate_class == gc::STRING_CONCATENATE
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::String("ab".to_string()))
    });
    assert!(carrier.is_some(), "expected a materialized carrier baked with \"ab\"");
}

#[test]
fn nofold_blocks_format_text_and_vector_math() {
    // Two representative shapes from this task, both gated by @nofold:
    // FormatText constant interpolation, and composite vector math.
    let r = compile_folded("@nofold\n\nlet n = 42\nout y = \"n=${n}\"");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::STRING_FORMAT_TEXT), 1);

    let r2 = compile_folded("@nofold\n\nout v = Vec(1.0, 2.0, 3.0) + Vec(0.5, 0.5, 0.5)");
    no_errors(&r2);
    assert_eq!(count_class(&r2.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 1);
}

#[test]
fn quat_literal_constant_folds_to_carrier() {
    // Each component is an ARITHMETIC expression (`n + 0.0`), NOT a bare
    // literal. A bare-literal `Quat(0.0, 0.0, ...)` call never produces a
    // `Literal::Quat` at all: `expr_to_literal` (lower/predeclare.rs) has no
    // `Quat` arm (unlike `Vec`/`Rotation`/`Color`), so `Expr::Call` lowering
    // (lower/expr.rs) falls through to the ordinary `lower_call` path, and
    // each bare-literal float argument inlines directly into the
    // MakeQuaternion gate's OWN properties at lowering time — a real gate
    // that happens to carry the right data, indistinguishable from the
    // materialized carrier this test exists to protect (that was the
    // vacuousness bug: the original version of this test used bare literals
    // and passed regardless of whether the `Literal::Quat` carrier arm in
    // `materialize_unfoldable_constants` existed at all). Arithmetic args
    // force a real `MathAdd` per component, wired into MakeQuaternion, so a
    // `Literal::Quat` can only appear if the certified fold pass evaluates
    // `MakeQuaternion` itself once all four inputs are known
    // (`fold/eval.rs::make_quaternion`, transitively certified).
    let r = compile_folded(
        "let q = Quat(0.0 + 0.0, 0.0 + 0.0, 0.38 + 0.0, 0.92 + 0.0).SplitQuat()\n\
         out x = q.X"
    );
    no_errors(&r);
    // All four MathAdd gates folded away — proves the fold pass actually
    // ran (not just that some MakeQuaternion happens to carry the right
    // data via the pre-existing bare-literal inline path).
    assert_eq!(count_class(&r.module, "BrickComponentType_WireGraph_Expr_MathAdd"), 0);
    // SplitQuaternion's input can't take inline data (`_Expr_Split` gates
    // are excluded in `emit::port_accepts_inline_variant`), so the folded
    // `Literal::Quat` must be materialized to a real MakeQuaternion carrier:
    // baked X/Y/Z/W, no incoming wires (a pure data source), and wired OUT
    // to the SplitQuat consumer.
    let carrier = r.module.nodes.values()
        .find(|n| {
            n.gate_class == gc::MAKE_QUATERNION
                && n.properties.get(&crate::intern::intern("X"))
                    == Some(&crate::ir::Literal::Float(0.0))
                && n.properties.get(&crate::intern::intern("Y"))
                    == Some(&crate::ir::Literal::Float(0.0))
                && n.properties.get(&crate::intern::intern("Z"))
                    == Some(&crate::ir::Literal::Float(0.38))
                && n.properties.get(&crate::intern::intern("W"))
                    == Some(&crate::ir::Literal::Float(0.92))
                && !r.module.wires.iter().any(|w| w.target.node_id == n.id)
        })
        .expect(
            "expected a materialized MakeQuaternion carrier baked with the folded \
             Quat(0, 0, 0.38, 0.92) constant"
        );
    let delivers_to_consumer = r.module.wires.iter().any(|w| {
        w.source.node_id == carrier.id
            && r.module
                .nodes
                .get(&w.target.node_id)
                .is_some_and(|t| t.gate_class == gc::SPLIT_QUATERNION)
    });
    assert!(
        delivers_to_consumer,
        "carrier must be wired to the SplitQuat consumer; if this fails after deleting the \
         Literal::Quat arm in materialize_unfoldable_constants, the wire is silently dropped at emit"
    );
}

// --- fold2 Task 5 (review follow-up): the SAME call-argument-baking gap,
// still open in the two resolvers `resolve_data_input` was never wired
// into: `try_resolve_format_text`'s substitution slots, and
// `resolve_condition` (shared by Select/Branch). Both used plain
// `resolve_input`, which only ever consults wires, so a baked slot/condition
// reads back as `Unwired` no matter how determined its actual value is.

#[test]
fn format_text_call_argument_baked_slot_folds() {
    // `Fmt`'s optional `a..g` params go through the exact same
    // `lower/call.rs::literal_for_property_port` baked-vs-wired split as any
    // other builtin CALL argument (see
    // `call_argument_baked_vector_literals_fold_through_dot`): a bare-literal
    // argument bakes directly onto the FormatText node's own InputA..InputG
    // property, with NO wire at all. `try_resolve_format_text`'s slot loop
    // used plain `resolve_input`, which only consults wires, so the baked `5`
    // in slot `{1}` read back as `Unwired` and rendered as the
    // unbound-slot default `"0"` instead of `"5"` — `Fmt("{0}-{1}", 1.0 + 2.0,
    // 5)` folded to `"3-0"`. `1.0 + 2.0` (not a bare literal) keeps slot `{0}`
    // genuinely WIRED (to a real MathAdd carrier that itself folds to `3.0`),
    // so this also locks in that the pre-existing wired-slot path is
    // untouched by the fix.
    let r = compile_folded("out y = Fmt(\"{0}-{1}\", 1.0 + 2.0, 5)");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::STRING_FORMAT_TEXT), 0);
    let carrier = r.module.nodes.values().find(|n| {
        n.gate_class == gc::STRING_CONCATENATE
            && n.properties.get(&*crate::intern::sym::INPUT_A)
                == Some(&crate::ir::Literal::String("3-5".to_string()))
    });
    assert!(
        carrier.is_some(),
        "expected a materialized carrier baked with \"3-5\" — the baked slot {{1}}=5 must \
         resolve, not silently render as the unbound-slot default \"0\""
    );
}

#[test]
fn select_baked_true_condition_shorts_to_truthy_side() {
    // `Select`'s `cond` param bakes a bare-literal argument directly onto
    // `BSelectB` (no wire), the same call-argument-baking shape as `Fmt`'s
    // slots above. `resolve_condition` (shared by Select/Branch) used plain
    // `resolve_input`, so the baked `true` read back as `Unwired` ->
    // `eval::truthy(None)` is falsy -> the pass shorted to the WRONG side
    // (`InputA`) instead of the truthy `InputB`. `Opaque(1000)`/`Opaque(2000)`
    // each materialize their own distinct, traceable `MathAdd` carrier (same
    // shape documented in `opaque_blocks_folding`).
    //
    // Rerouter (`Opaque`) nodes are NEVER elided by the demand sweep (see
    // `demand_sweep_removes_orphaned_feeders`'s "Opaque never elided" note),
    // so BOTH sides' Rerouter nodes stay in the module — confirmed via
    // `cargo run -p bearilog-cli -- compile --fold --dump-ir` on this exact
    // source before the fix, which shows `n3`=`Opaque(1000)` AND
    // `n5`=`Opaque(2000)` both still present, with only `n3` (the wrong side)
    // actually wired to `out y`. A survivor-count assertion can't tell "chose
    // the right side" from "chose the wrong side" here — trace the LIVE wire
    // path from `out y` back through its chosen Rerouter to that Rerouter's
    // OWN carrier instead, which does prove which side was actually chosen.
    let r = compile_folded("out y = Select(true, Opaque(1000), Opaque(2000))");
    no_errors(&r);
    assert_eq!(count_class(&r.module, gc::SELECT), 0, "constant-conditioned select removed");
    let out_node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_OUTPUT)
        .expect("out y's boundary node");
    let chosen_opaque_id = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == out_node.id)
        .map(|w| w.source.node_id)
        .expect("out y must still receive a live wire after the short-circuit");
    let carrier_id = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == chosen_opaque_id)
        .map(|w| w.source.node_id)
        .expect("the chosen Opaque must still have its materialized carrier wired in");
    let carrier = r.module.nodes.get(&carrier_id).expect("carrier node must exist");
    assert_eq!(
        carrier.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(2000)),
        "truthy condition must short to InputB's source (2000), not InputA's (1000)"
    );
}

#[test]
fn string_condition_coercion_gate_blocks_folding_for_now() {
    // A string condition no longer reaches the Branch's `BCond` directly:
    // the language-level string → bool coercion inserts a
    // `CompareNotEqual(s, "")` gate at lowering time, so `if "x"` means
    // exactly `"x" != ""` — empty is false, everything else (INCLUDING "0"
    // and "false", which the game's native port truthiness would call
    // falsy) is true.
    //
    // Fold status: `CompareNotEqual` has NO certified (str, str) signature
    // in `data/gate_semantics.json` (only int,int / int,str / vector,vector
    // / color,color / rotator,rotator / quat,quat were probed), so the fold
    // pass REFUSES the inserted gate and the Branch survives even under
    // @fold — correct-but-unoptimized until that signature is probed and
    // certified. When (str, str) coverage lands, this test should flip to
    // asserting full truncation under the NEW law: "" → else arm survives,
    // "0" → THEN arm survives ("0" != "" is true — deliberately different
    // from native truthiness), "a" → then arm survives.
    for cond in ["\"\"", "\"0\"", "\"a\""] {
        let src = format!(
            "in t: exec\nvar a: int = 0\nvar b: int = 0\non t {{ if {cond} {{ a = 1 }} else {{ b = 2 }} }}"
        );
        let r = compile_folded(&src);
        no_errors(&r);
        assert_eq!(
            count_class(&r.module, gc::COMPARE_NOT_EQUAL),
            1,
            "if {cond}: the inserted != \"\" coercion gate must survive (uncertified str,str)"
        );
        assert_eq!(
            count_class(&r.module, gc::BRANCH),
            1,
            "if {cond}: Branch must survive — its condition can't fold through the \
             uncertified compare"
        );
        assert_eq!(
            count_class(&r.module, gc::VAR_SET),
            2,
            "if {cond}: both arms must survive an unresolved Branch"
        );
    }
}
