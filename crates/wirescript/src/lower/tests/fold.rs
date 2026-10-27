use super::*;
use crate::ir::gate_class as gc;

const MATH_ADD: &str = "BrickComponentType_Expr_MathAdd";
const MATH_MULTIPLY: &str = "BrickComponentType_Expr_MathMultiply";
const LOGICAL_AND: &str = "BrickComponentType_Expr_LogicalAND";
const LOGICAL_OR: &str = "BrickComponentType_Expr_LogicalOR";

/// A `(label, fold mode, source, [(gate class, count)])` row: the source lowers
/// with no error at that mode, holding exactly `count` gates of each class in
/// the whole module tree.
type CountRow<'a> = (&'a str, FoldMode, &'a str, &'a [(&'a str, usize)]);

#[track_caller]
fn check_gate_counts(rows: &[CountRow<'_>]) {
    let mut failures = Vec::new();
    for (label, mode, src, counts) in rows {
        let r = lower_with(src, "test", *mode, &crate::resolve::FsLoader);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}\n{src}"));
            continue;
        }
        for (class, want) in *counts {
            let got = count_class(&r.module, class);
            if got != *want {
                failures.push(format!("{label}: {got} x {class}, want {want}\n{src}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn the_fold_pass_folds_constants_and_keeps_every_barrier() {
    use FoldMode::{ForceOff, ForceOn};
    check_gate_counts(&[
        (
            "(2 + 3) * 4 == 20 folds to one literal: no Math or Compare gate is left",
            ForceOn,
            "out y = (2 + 3) * 4 == 20",
            &[(MATH_ADD, 0), (MATH_MULTIPLY, 0), (gc::COMPARE_EQUAL, 0)],
        ),
        (
            "the unfolded helper keeps every real gate (it guards the whole structural suite)",
            ForceOff,
            "out y = (2 + 3) * 4 == 20",
            &[(MATH_ADD, 1), (MATH_MULTIPLY, 1), (gc::COMPARE_EQUAL, 1)],
        ),
        (
            "Opaque blocks folding; the opaque value goes through a `var` so the Rerouter's input \
             is never a bare `_Literal` that would get its own MathAdd carrier",
            ForceOn,
            "var x: int = 2\nout y = Opaque(x) + 3",
            &[(MATH_ADD, 1), (gc::REROUTER, 1)],
        ),
        (
            "`in t: any` is as opaque as `Opaque(...)`: `t + 1` stays a real gate",
            ForceOn,
            "in t: any\nlet v = t + 1\nout y = v",
            &[(MATH_ADD, 1)],
        ),
        (
            "an @nofold add stays real and its consumer does not see through it",
            ForceOn,
            "@nofold let a = 2 + 3\nout y = a * 2",
            &[(MATH_ADD, 1), (MATH_MULTIPLY, 1)],
        ),
        (
            "an i64 overflow stays unfolded",
            ForceOn,
            "out y = 9223372036854775807 + 1",
            &[(MATH_ADD, 1)],
        ),
        (
            "a non-finite result stays unfolded",
            ForceOn,
            "out y = 1.0 / 0.0",
            &[("BrickComponentType_Expr_MathDivide", 1)],
        ),
        (
            "a bare string literal is a portless String_Concatenate carrier, recognized as a \
             known value, so constant string equality folds (a `--fold-diff` fuzzer finding)",
            ForceOn,
            "out y = (\"hello\" == \"world\")",
            &[(gc::COMPARE_EQUAL, 0)],
        ),
        (
            "the same string fold across a chip boundary (the fuzzer's minimized repro)",
            ForceOn,
            "chip F(p: string) -> (r: bool) {\n  out r = (p == \"nope\")\n}\nout y = F(\"hello world\")",
            &[(gc::COMPARE_EQUAL, 0)],
        ),
        (
            "Inc(4) = 5 flows back out of the chip and folds the outer compare",
            ForceOn,
            "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nout y = Inc(4) == 5",
            &[(gc::COMPARE_EQUAL, 0)],
        ),
        (
            "a constant-true selector shorts to its opaque THEN side; the Select goes",
            ForceOn,
            "let v = Opaque(9)\nout y = if true then v else 3",
            &[(gc::SELECT, 0), (gc::REROUTER, 1)],
        ),
        (
            "AND false annihilates; the orphaned NOT feeding it is swept; Opaque is never elided",
            ForceOn,
            "out y = !Opaque(true) && false",
            &[(LOGICAL_AND, 0), (gc::LOGICAL_NOT, 0), (gc::REROUTER, 1)],
        ),
        (
            "a FormatText substitution slot fed by Opaque does not fold",
            ForceOn,
            "var x: int = 5\nout y = Fmt(\"n={0}\", Opaque(x))",
            &[(gc::STRING_FORMAT_TEXT, 1), (gc::REROUTER, 1)],
        ),
        (
            "a non-ASCII string operand keeps FormatText (multibyte behavior is not certified)",
            ForceOn,
            "let s = \"\u{03c0}\"\nout y = \"v=${s}\"",
            &[(gc::STRING_FORMAT_TEXT, 1)],
        ),
        (
            "VecNormalize is certified but deferred, so a known operand still keeps the gate",
            ForceOn,
            "out v = Vec(1.0, 2.0, 3.0).Normalize()",
            &[(gc::VEC_NORMALIZE, 1)],
        ),
    ]);
}

#[test]
fn fold_annotations_and_modes_decide_whether_the_pass_runs() {
    use FoldMode::{Auto, ForceOff, ForceOn};
    // `(2 + 3) * 4 == 20` collapses to one literal when folded, so its
    // CompareEqual tells folded from unfolded; a bare `2 + 3` into an `out` is
    // re-wrapped in a same-shaped MathAdd carrier either way.
    let chain = "out y = (2 + 3) * 4 == 20";
    let nofold_chain = "@nofold\n\nout y = (2 + 3) * 4 == 20";
    let both = "@nofold\n@fold\n\nout y = (2 + 3) * 4 == 20";
    let nofold_branch = "in t: exec\nvar a: int = 0\nvar b: int = 0\n@nofold on t { if true { a = 1 } else { b = 2 } }";
    let nofold_ident = "in t: exec\nvar a: int = 0\nvar b: int = 0\n@nofold on t { let c = true\nif c { a = 1 } else { b = 2 } }";
    check_gate_counts(&[
        (
            "a module @nofold beats ForceOn",
            ForceOn,
            nofold_chain,
            &[(gc::COMPARE_EQUAL, 1)],
        ),
        (
            "Auto folds with no annotation",
            Auto,
            chain,
            &[(gc::COMPARE_EQUAL, 0)],
        ),
        (
            "a redundant @fold under Auto still folds",
            Auto,
            "@fold\n\nout y = (2 + 3) * 4 == 20",
            &[(gc::COMPARE_EQUAL, 0)],
        ),
        (
            "@nofold disables folding under Auto (the only opt-out)",
            Auto,
            nofold_chain,
            &[(gc::COMPARE_EQUAL, 1)],
        ),
        (
            "@nofold beats a stacked @fold under Auto",
            Auto,
            both,
            &[(gc::COMPARE_EQUAL, 1)],
        ),
        (
            "@nofold beats a stacked @fold under ForceOn too",
            ForceOn,
            both,
            &[(gc::COMPARE_EQUAL, 1)],
        ),
        (
            "a module @nofold keeps FormatText constant interpolation",
            ForceOn,
            "@nofold\n\nlet n = 42\nout y = \"n=${n}\"",
            &[(gc::STRING_FORMAT_TEXT, 1)],
        ),
        (
            "a module @nofold keeps composite vector math",
            ForceOn,
            "@nofold\n\nout v = Vec(1.0, 2.0, 3.0) + Vec(0.5, 0.5, 0.5)",
            &[(MATH_ADD, 1)],
        ),
        (
            "an @nofold handler keeps its constant-condition Branch",
            ForceOn,
            "in t: exec\nvar a: int = 0\n@nofold on t { if 1 == 1 { a = 1 } else { a = 2 } }",
            &[(gc::BRANCH, 1)],
        ),
        (
            "lower_if's literal-bool shortcut must not fire under @nofold (unfolded helper)",
            ForceOff,
            nofold_branch,
            &[(gc::BRANCH, 1), (gc::VAR_SET, 2)],
        ),
        (
            "the fold pass's own @nofold barrier keeps that Branch and both arms",
            ForceOn,
            nofold_branch,
            &[(gc::BRANCH, 1), (gc::VAR_SET, 2)],
        ),
        (
            "the ident-bound literal shortcut (`let c = true; if c`) respects @nofold too",
            ForceOff,
            nofold_ident,
            &[(gc::BRANCH, 1)],
        ),
        (
            "the ident-bound literal shortcut respects @nofold folded",
            ForceOn,
            nofold_ident,
            &[(gc::BRANCH, 1)],
        ),
    ]);
}

#[test]
fn module_fold_annotations_parse_and_conflict() {
    // Both annotations stacked, `@nofold` first: each is alone on its line and
    // the run is separated from the first decl by a blank line.
    let parsed = crate::parser::parse("@nofold\n@fold\n\nout y = (2 + 3) * 4 == 20", "test");
    assert!(
        crate::test_support::error_codes(&parsed.diagnostics).is_empty(),
        "parse errors: {:?}",
        parsed.diagnostics
    );
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.severity == crate::diagnostic::Severity::Warning
                && d.message.contains("conflict")),
        "expected a module-level @fold/@nofold conflict warning, got: {:?}",
        parsed.diagnostics
    );
    assert!(
        parsed.ast.fold,
        "the @fold annotation should still be recorded"
    );
    assert!(
        parsed.ast.no_fold,
        "@nofold should win but is still recorded on the AST"
    );

    // `@fold` directly after a module doc comment is not module-level: the
    // parser reads it as a directive-level `@fold` and says it is module-level
    // only.
    let parsed = crate::parser::parse("/// doc\n@fold\n\nout y = 1", "test");
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.message.contains("module-level only")),
        "expected module-level-only diagnostic, got: {:?}",
        parsed.diagnostics
    );
    assert!(
        !parsed.ast.fold,
        "@fold adjacent to doc comment should not be recognized as module-level"
    );
}

#[test]
fn a_constant_branch_condition_keeps_only_the_taken_arm() {
    use FoldMode::ForceOn;
    // A bare `true` condition takes lower_if's literal-bool shortcut and never
    // reaches the fold pass; `1 == 1` forces a real CompareEqual into a real
    // Branch whose condition becomes known only through the pass's fixpoint.
    check_gate_counts(&[
        (
            "a folded condition truncates the untaken side: only `a = 1` survives",
            ForceOn,
            "in t: exec\nvar a: int = 0\nvar b: int = 0\non t { if 1 == 1 { a = 1 } else { b = 2 } }",
            &[(gc::BRANCH, 0), (gc::VAR_SET, 1)],
        ),
        (
            "an else-side tail also reached from another handler is not deleted",
            ForceOn,
            "in t: exec\nin u: exec\nvar a: int = 0\nvar b: int = 0\nlet tail: exec\n\
             on t { if 1 == 1 { a = 1 } else { emit tail } }\non u { emit tail }\non tail { b = 2 }",
            &[(gc::BRANCH, 0), (gc::VAR_SET, 2)],
        ),
        (
            "without @nofold the literal-bool shortcut emits no Branch, only the taken arm",
            ForceOn,
            "in t: exec\nvar a: int = 0\nvar b: int = 0\non t { if true { a = 1 } else { b = 2 } }",
            &[(gc::BRANCH, 0), (gc::VAR_SET, 1)],
        ),
        (
            "nested else-if truncation chains all the way through: only `a = 2` survives",
            ForceOn,
            "in t: exec\nvar a: int = 0\non t { if 1 == 2 { a = 1 } else { if 2 == 2 { a = 2 } else { a = 3 } } }",
            &[(gc::BRANCH, 0), (gc::COMPARE_EQUAL, 0), (gc::VAR_SET, 1)],
        ),
        // A string condition is coerced through `CompareNotEqual(s, "")`, which
        // has no certified (str, str) signature, so the pass refuses it and the
        // Branch survives. Once (str, str) is certified: "" keeps the else arm,
        // "0" and "a" the then arm ("0" != "" is true, unlike native truthiness).
        (
            "if \"\": the uncertified coercion gate and the Branch survive",
            ForceOn,
            "in t: exec\nvar a: int = 0\nvar b: int = 0\non t { if \"\" { a = 1 } else { b = 2 } }",
            &[
                (gc::COMPARE_NOT_EQUAL, 1),
                (gc::BRANCH, 1),
                (gc::VAR_SET, 2),
            ],
        ),
        (
            "if \"0\": the uncertified coercion gate and the Branch survive",
            ForceOn,
            "in t: exec\nvar a: int = 0\nvar b: int = 0\non t { if \"0\" { a = 1 } else { b = 2 } }",
            &[
                (gc::COMPARE_NOT_EQUAL, 1),
                (gc::BRANCH, 1),
                (gc::VAR_SET, 2),
            ],
        ),
        (
            "if \"a\": the uncertified coercion gate and the Branch survive",
            ForceOn,
            "in t: exec\nvar a: int = 0\nvar b: int = 0\non t { if \"a\" { a = 1 } else { b = 2 } }",
            &[
                (gc::COMPARE_NOT_EQUAL, 1),
                (gc::BRANCH, 1),
                (gc::VAR_SET, 2),
            ],
        ),
    ]);
}

#[test]
fn annihilator_folds_with_unknown_side() {
    // As in the Opaque row above: route through a `var` so Opaque's own
    // argument isn't a bare literal with its own carrier gate.
    // Opaque cond is Unknown, but AND false is a certified annihilator.
    let r = compile_ok_folded("var b: bool = true\nout y = Opaque(b) && false");
    assert_eq!(count_class(&r.module, LOGICAL_AND), 0);
    // The folded `Bool(false)` feeds `out y`, a dataless boundary port, so
    // `materialize_unfoldable_constants` re-wraps it in a fully-baked LogicalOR
    // carrier rather than leaving a standalone `_Literal`: check the carrier's
    // baked operand.
    let folded_to_false = r.module.nodes.values().any(|n| {
        n.gate_class == LOGICAL_OR
            && n.properties.get(&*crate::intern::sym::B_INPUT_A)
                == Some(&crate::ir::Literal::Bool(false))
    });
    assert!(
        folded_to_false,
        "annihilator must compute false, not true (or stay unresolved)"
    );
    // OR-true has no false-annihilator: `Opaque(x) || false` must NOT collapse.
    // The bare `false` RHS inlines into the outer LogicalOR's `bInputB` data (a
    // plain `bool` field holds a matching constant), so ONE real LogicalOR
    // remains, carrying the baked false operand.
    let r2 = compile_ok_folded("var b: bool = true\nout y = Opaque(b) || false");
    assert_eq!(count_class(&r2.module, LOGICAL_OR), 1);
    assert!(
        r2.module.nodes.values().any(|n| n.gate_class == LOGICAL_OR
            && n.properties.get(&crate::intern::intern("bInputB"))
                == Some(&crate::ir::Literal::Bool(false))),
        "the outer OR bakes its false operand into bInputB (no carrier)"
    );
}

#[test]
fn constant_bitwise_operand_inlines_not_carrier() {
    // A bitwise gate stores its operands as plain `i64`, so a native-int
    // constant operand (the `0` in `x | 0`) inlines into the gate's `InputB`
    // data rather than materializing a `MathAdd(0, 0)` carrier.
    let r = compile_ok_folded("var x: int = 3\nout y = x | 0");
    assert_eq!(
        count_class(&r.module, "BrickComponentType_Expr_BitwiseOR"),
        1
    );
    assert_eq!(
        count_class(&r.module, MATH_ADD),
        0,
        "the constant 0 must inline, not spawn a MathAdd(0, 0) carrier"
    );
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class == "BrickComponentType_Expr_BitwiseOR"
                && n.properties.get(&crate::intern::intern("InputB"))
                    == Some(&crate::ir::Literal::Int(0))),
        "the OR bakes its 0 operand into InputB"
    );
}

#[test]
fn constant_string_operand_inlines_not_carrier() {
    // A `str` field holds a matching String constant, so a constant string
    // operand (`Contains`'s search text, an interpolation template's literal
    // parts) bakes into the gate's data rather than materializing a
    // `String_Concatenate` carrier.
    let r = compile_ok_folded("var x: string = \"\"\nout z = x.Contains(\"foo\")");
    assert_eq!(
        count_class(&r.module, gc::STRING_CONCATENATE),
        0,
        "the constant search string must inline, not spawn a String_Concatenate carrier"
    );
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::STRING_CONTAINS
                && n.properties.get(&crate::intern::intern("Search"))
                    == Some(&crate::ir::Literal::String("foo".to_string()))),
        "Contains bakes its search string into the Search field"
    );
}

#[test]
fn cross_chip_constant_folds_inside_named_chip() {
    // The argument is `2 + 2`, not `Inc(4)`: `const_arg_literal` inlines a bare
    // literal argument into the chip instance, bypassing the MicrochipInput
    // wire. Its value becomes known only through this pass's fold fixpoint.
    let src = "chip Inc(v: int) -> (r: int) { out r = v + 1 }\nout y = Inc(2 + 2)";
    let r = compile_ok_folded(src);
    // Dead-feed pruning (Rule A + Rule B) leaves both of Inc's boundary nodes
    // wire-free, and an unannotated wire-free instance is elided whole: no
    // chip, no `v`-argument carrier, no `r`-result carrier.
    assert!(
        r.module.chips.is_empty(),
        "the fully-folded, now wire-free Inc instance must be elided entirely, not just \
         emptied: chips: {:?}",
        r.module.chips.keys().collect::<Vec<_>>()
    );
    // `y` is a dataless ROOT-level output boundary the pass never touches, so it
    // still needs exactly one MathAdd carrier, and nothing else survives.
    let adds: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == MATH_ADD)
        .collect();
    assert_eq!(
        adds.len(),
        1,
        "expected exactly one surviving carrier (for `y` itself), found {}: {adds:?}",
        adds.len()
    );
    let carrier = adds[0];
    assert!(
        !r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == carrier.id),
        "the carrier must be fully baked (no wired operand): proof `v + 1` folded at compile \
         time, not delivered by a live wire"
    );
    assert_eq!(
        carrier.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(5)),
        "(2 + 2) + 1 must fold to the baked constant 5"
    );
}

/// A `(label, source, baked text, classes that must be gone)` row: the source
/// folds to one `String_Concatenate` carrier baked with the text, with no chip
/// and no gate of each listed class anywhere in the tree.
#[test]
fn a_constant_string_folds_into_one_baked_carrier() {
    let score = (6 + 6) * (6 + 6) + 100;
    let score_text = format!("score={score}");
    let rows: &[(&str, &str, &str, &[&str])] = &[
        (
            "`let n = 42` interpolates with no comma grouping below 1,000",
            "let n = 42\nout y = \"n=${n}\"",
            "n=42",
            &[gc::STRING_FORMAT_TEXT],
        ),
        (
            "the certified render law comma-groups from 1,000 up, through FormatText substitution",
            "let n = 1000\nout y = \"n=${n}\"",
            "n=1,000",
            &[gc::STRING_FORMAT_TEXT],
        ),
        (
            "a bare-literal Fmt argument bakes onto FormatText's own InputB property with no \
             wire; reading only wires rendered slot {1} as the unbound default, `3-0`. \
             `1.0 + 2.0` keeps slot {0} wired to a real carrier",
            "out y = Fmt(\"{0}-{1}\", 1.0 + 2.0, 5)",
            "3-5",
            &[gc::STRING_FORMAT_TEXT],
        ),
        (
            "a composite (Color) input drives folding through a non-Math gate, ColorToHex",
            "out y = Color(1.0, 0.5, 0.0).ToHex()",
            "FFBC00",
            &[gc::COLOR_TO_HEX],
        ),
        (
            "the `..` operator's two portless literal carriers fold by the Concatenate law",
            "out y = (\"a\" .. \"b\")",
            "ab",
            &[],
        ),
        (
            "a chip output read only by an interpolation: once FormatText collapses, Rule A drops \
             both of Score's wire-free boundary feeds and the instance is elided, leaving no \
             carrier for a port nothing reads",
            "chip Score(base: int) -> (r: int) { out r = (base * base + 100) % 977 }\n\
             let score = Score(6 + 6)\nout y = \"score=${score}\"",
            &score_text,
            &[
                gc::STRING_FORMAT_TEXT,
                MATH_ADD,
                MATH_MULTIPLY,
                "BrickComponentType_Expr_MathModulo",
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, text, gone) in rows {
        let r = compile_folded(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        if !r.module.chips.is_empty() {
            failures.push(format!("{label}: {} chip(s) left", r.module.chips.len()));
        }
        for class in *gone {
            let n = count_class(&r.module, class);
            if n != 0 {
                failures.push(format!("{label}: {n} x {class} left"));
            }
        }
        let baked = r.module.nodes.values().any(|n| {
            n.gate_class == gc::STRING_CONCATENATE
                && n.properties.get(&*crate::intern::sym::INPUT_A)
                    == Some(&crate::ir::Literal::String(text.to_string()))
        });
        if !baked {
            failures.push(format!("{label}: no carrier baked with {text:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

fn float_prop(n: &crate::ir::Node, name: &str) -> Option<f64> {
    match n.properties.get(&crate::intern::intern(name)) {
        Some(crate::ir::Literal::Float(f)) => Some(*f),
        _ => None,
    }
}

/// A folded vector reaches its dataless `out` boundary through one baked
/// `MakeVector` carrier, the same delivery a hand-written `Vec(...)` gets.
#[test]
fn a_constant_vector_folds_into_one_baked_make_vector() {
    let rows: &[(&str, &str, &str, [f64; 3])] = &[
        (
            "component-wise vector addition",
            "out v = Vec(1.0, 2.0, 3.0) + Vec(0.5, 0.5, 0.5)",
            MATH_ADD,
            [1.5, 2.5, 3.5],
        ),
        (
            "Cross's literal arguments are baked onto the gate's own properties with no wire; the \
             driver read only wires, so the gate had an all-`Unwired` signature and never folded \
             (a `--fold-diff` fuzzer finding; `resolve_data_input` falls back to the property)",
            "out v = Cross(Vec(1.0, 0.0, 0.0), Vec(0.0, 1.0, 0.0))",
            gc::VEC_CROSS,
            [0.0, 0.0, 1.0],
        ),
        (
            "Vec's own X/Y/Z take the same baked-vs-wired split: two baked literal components and \
             one computed one all resolve. (The probe's renderVec call shows this shape too and \
             needs per-component Opaque armor to stay fold-invariant.)",
            "out v = Vec(0.5, -1.25, 1.0 / 3.0)",
            "BrickComponentType_Expr_MathDivide",
            [0.5, -1.25, 1.0 / 3.0],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, gone, xyz) in rows {
        let r = compile_folded(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        if count_class(&r.module, gone) != 0 {
            failures.push(format!("{label}: {gone} survived"));
        }
        let carriers = nodes_of(&r.module, gc::MAKE_VECTOR);
        let got: Vec<_> = carriers
            .iter()
            .map(|c| [float_prop(c, "X"), float_prop(c, "Y"), float_prop(c, "Z")])
            .collect();
        if got != [xyz.map(Some)] {
            failures.push(format!(
                "{label}: MakeVector carriers {got:?}, want one baked {xyz:?}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn select_with_unwired_chosen_input_stays() {
    // Falsy selector chooses InputA; if nothing drives it, refuse. (Built from
    // source, the closest form is a fully-constant select folding to its value.)
    let r = compile_ok_folded("out y = if false then 1 else 2");
    assert_eq!(count_class(&r.module, gc::SELECT), 0);
    // `out y`'s port has no data struct, so the shorted `_Literal(2)` is re-wrapped
    // in a fully-baked MathAdd carrier: check that carrier's baked operand.
    let folded_to_two = r.module.nodes.values().any(|n| {
        n.gate_class == MATH_ADD
            && n.properties.get(&*crate::intern::sym::INPUT_A) == Some(&crate::ir::Literal::Int(2))
            && !r.module.wires.iter().any(|w| w.target.node_id == n.id)
    });
    assert!(
        folded_to_two,
        "falsy select must short to the else value 2, not stay unresolved"
    );
}

#[test]
fn imported_module_nofold_is_inert() {
    // A module-level `@nofold` only takes effect for the file `resolve()` was
    // entered on: resolve.rs never reads an imported file's own `no_fold`. A
    // `@nofold` in an imported library is inert and the unannotated entry folds.
    let lib = "@nofold\n\nmod inc(v: int) -> int {\n  return v + 1\n}";
    let main = "import { inc } from \"lib\"\nout y = inc(2) * 4 == 12";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    // Folded: `inc(2)` inlines to `2 + 1`, `* 4`, `== 12` -> literal `true`. An
    // imported `@nofold` that leaked would keep the CompareEqual alive.
    assert_eq!(
        count_class(&r.module, gc::COMPARE_EQUAL),
        0,
        "the imported file's @nofold must be inert; the entry still folds"
    );
}

#[test]
fn emptied_unannotated_chip_is_elided() {
    // An anon chip used as an expression only becomes a `module.chips` entry in
    // `partition_anon_chips`, which runs after this pass, so a NAMED chip
    // instance (already in `module.chips` during lowering) is what exercises
    // whole-chip elision.
    let r = compile_ok_folded("chip Empty() { let dead = 2 + 3 }\nEmpty()\nout y = 1");
    assert!(
        r.module.chips.is_empty(),
        "unannotated empty chip must be elided"
    );
    assert_eq!(count_class(&r.module, gc::MICROCHIP), 0);
}

#[test]
fn annotated_empty_chip_is_kept() {
    let r =
        compile_ok_folded("@label(\"Keep\") chip Empty() { let dead = 2 + 3 }\nEmpty()\nout y = 1");
    // The labeled chip node survives even though its contents folded away.
    let chip_nodes = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP || n.gate_class == gc::MICROCHIP_ALT)
        .count();
    assert_eq!(chip_nodes, 1, "@label chip must be retained");
    assert_eq!(
        r.module.chips.len(),
        1,
        "labeled empty chip's module entry survives"
    );
}

// `inline_orphan_literals` cross-module guard

#[test]
fn cross_module_literal_inline_skips_orphan_deletion() {
    // `inline_orphan_literals` must not delete a single-consumer `_Literal`'s
    // wire when the consumer is in another module (a chip's `MicrochipInput`
    // fed by a parent literal), or the port reads 0. `2 + 2` becomes such a
    // `_Literal` only through this pass.
    let src = "chip Passthrough(v: int) -> (r: int) { out r = v + Opaque(0) }\n\
               out y = Passthrough(2 + 2)";

    let folded = compile_ok_folded(src);
    let child = only_chip(&folded, "one Passthrough instance");
    let mc_input = child
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .expect("chip has a v boundary node");
    // Boundary-delivery cleanup (Rule B, then Rule A): `v`'s known value 4 is
    // delivered straight to the real `v + Opaque(0)` gate as baked data, and
    // `v`'s own boundary node ends with zero wires. The value must still reach
    // the gate that needs it.
    assert!(
        !child
            .wires
            .iter()
            .any(|w| w.source.node_id == mc_input.id || w.target.node_id == mc_input.id),
        "v's boundary node must end up completely wire-free once its value is delivered \
         directly to its consumer"
    );
    // Two MathAdds exist in the chip: the real `v + Opaque(0)` gate, and the
    // carrier of Opaque's own bare `0` argument. The carrier has no incoming
    // wire; the real gate still has InputB wired to the live Opaque rerouter.
    let live_add = child
        .nodes
        .values()
        .find(|n| n.gate_class == MATH_ADD && child.wires.iter().any(|w| w.target.node_id == n.id))
        .expect("the real v + Opaque(0) gate must survive (Opaque blocks it from folding)");
    assert_eq!(
        live_add.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(4)),
        "v must receive the correctly-folded (2 + 2) = 4, baked directly onto the surviving \
         gate, not silently read 0, and not left stranded on a dangling materialized carrier"
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
         surviving gate: InputA is now baked, not wired, and the Opaque barrier must survive \
         untouched: {wires_into_add:?}"
    );

    // Unfolded: `2 + 2` stays a real same-module MathAdd, and the value reaches
    // `v` over a live wire.
    let unfolded = compile_ok(src);
    let child2 = only_chip(&unfolded, "one Passthrough instance");
    let mc_input2 = child2
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .expect("chip has a v boundary node");
    assert!(
        unfolded
            .module
            .wires
            .iter()
            .any(|w| w.target.node_id == mc_input2.id),
        "unfolded path must also deliver v's value via a live wire"
    );
}

/// `Dot`/`Cross`/`ScaleVec` take literal Vector arguments as baked, unwired
/// properties (`lower/expr.rs::literal_for_property_port`); `resolve_data_input`
/// reads the baked property, so the gate folds. `Dot` is the smallest case; its
/// scalar result materializes via the `MathAdd(n, 0)` carrier.
#[test]
fn call_argument_baked_vector_literals_fold_through_dot() {
    let r = compile_ok_folded("out y = Dot(Vec(1.0, 2.0, 3.0), Vec(4.0, 5.0, 6.0))");
    assert_eq!(count_class(&r.module, gc::VEC_DOT), 0);
    let carrier = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == MATH_ADD)
        .expect("materialized MathAdd carrier for the folded dot product");
    assert_eq!(float_prop(carrier, "InputA"), Some(32.0)); // 1*4 + 2*5 + 3*6
    assert_eq!(float_prop(carrier, "InputB"), Some(0.0));
}

#[test]
fn make_rotation_does_not_fold() {
    // Arithmetic args force a real wired MakeRotation gate (a bare-literal
    // `Rotation(...)` folds to a `_Literal(Rotator)` at lowering time, see
    // `lower/predeclare.rs::expr_to_literal`). MakeRotation's only table evidence
    // renders blank (`BLANK_RENDER_REFUSED`), so it never folds.
    let r = compile_ok_folded("out r: rotator = Rotation(0.0 + 0.0, 90.0 + 0.0, 45.5 + 0.0)");
    assert_eq!(count_class(&r.module, gc::MAKE_ROTATION), 1);
    // Each `X.0 + 0.0` is a folded expression, not a bare source literal, so it
    // folds to a `_Literal(Float)` and re-materializes as a fully-baked
    // MathAdd(n, 0) carrier, keeping folding a structural no-op.
    assert_eq!(count_class(&r.module, MATH_ADD), 3);
    let all_baked = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == MATH_ADD)
        .all(|n| !r.module.wires.iter().any(|w| w.target.node_id == n.id));
    assert!(
        all_baked,
        "each operand's carrier must be fully baked (no wired operand)"
    );
}

#[test]
fn quat_literal_constant_folds_to_carrier() {
    // Each component is arithmetic (`n + 0.0`): bare literals would inline into
    // MakeQuaternion's own properties at lowering, so a `Literal::Quat` appears
    // only if the pass evaluates MakeQuaternion itself.
    let r = compile_ok_folded(
        "let q = Quat(0.0 + 0.0, 0.0 + 0.0, 0.38 + 0.0, 0.92 + 0.0).SplitQuat()\n\
         out x = q.X",
    );
    // All four MathAdd gates folded away: the fold pass actually ran.
    assert_eq!(count_class(&r.module, MATH_ADD), 0);
    // SplitQuaternion's input takes no inline data, so the folded `Literal::Quat`
    // materializes as a MakeQuaternion carrier: baked X/Y/Z/W, no incoming
    // wires, wired OUT to the SplitQuat consumer.
    let carrier = r
        .module
        .nodes
        .values()
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
             Quat(0, 0, 0.38, 0.92) constant",
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

#[test]
fn select_baked_true_condition_shorts_to_truthy_side() {
    // `Select`'s `cond` bakes a bare literal onto `BSelectB` with no wire; read
    // as `Unwired` (falsy) it would short to InputA. Both Opaque rerouters stay,
    // so a count cannot tell the sides apart: trace the live wire from `out y`
    // through the chosen Rerouter to its MathAdd carrier.
    let r = compile_ok_folded("out y = Select(true, Opaque(1000), Opaque(2000))");
    assert_eq!(
        count_class(&r.module, gc::SELECT),
        0,
        "constant-conditioned select removed"
    );
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
    let carrier = r
        .module
        .nodes
        .get(&carrier_id)
        .expect("carrier node must exist");
    assert_eq!(
        carrier.properties.get(&*crate::intern::sym::INPUT_A),
        Some(&crate::ir::Literal::Int(2000)),
        "truthy condition must short to InputB's source (2000), not InputA's (1000)"
    );
}

/// A runtime `@label(<expr>)` is consumed only by a wire EMIT adds later (into
/// the root shell's `Text` port, or a var's label), so the fold demand sweep
/// must keep its source node or emit silently drops the label.
#[test]
fn fold_keeps_every_dynamic_label_source() {
    // The blank line after the root `@label(...)` makes it the module's label
    // rather than one attached to the `var who` below it.
    let root = compile_ok_folded(
        "@label(\"root: ${who}\")\n\nvar who: string = \"\"\nin go: exec\non go { who = \"alice\" }",
    );
    let src = root
        .module
        .root_dynamic_label
        .as_ref()
        .expect("root dynamic label recorded during lowering");
    assert!(
        root.module.nodes.contains_key(&src.node_id),
        "root label source {:?} was pruned",
        src.node_id
    );
    // A per-var label whose source is a FormatText gate, not a bare Variable.
    let var = compile_ok_folded(
        "var who: string = \"\"\n@label(\"m: ${who}\") var mirror: string = \"\"\n\
         in go: exec\non go { who = \"alice\" mirror = \"alice\" }",
    );
    assert_eq!(
        var.module.dynamic_labels.len(),
        1,
        "one per-var dynamic label recorded during lowering"
    );
    for src in var.module.dynamic_labels.values() {
        assert!(
            var.module.nodes.contains_key(&src.node_id),
            "var label source {:?} was pruned",
            src.node_id
        );
    }
}

#[test]
fn constant_string_operand_inlines_into_its_concat_consumer() {
    // A bare string literal lowers to a `String_Concatenate` wrapper carrying
    // the text in `InputA`. When that wrapper's only consumer is another
    // concat, the text belongs in the consumer's own `InputA`/`InputB` data:
    // those are native `str` fields, and a Concatenate reads an unwired input
    // from data, which is exactly how the wrapper itself supplies its text.
    let r = compile_ok_folded("in n: int\nout s: string = \"n=\" .. n");
    assert_eq!(
        count_class(&r.module, gc::STRING_CONCATENATE),
        1,
        "the literal operand must bake into the real concat, leaving one gate: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| (n.gate_class, std::sync::Arc::clone(&n.properties)))
            .collect::<Vec<_>>()
    );
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::STRING_CONCATENATE
                && n.properties.get(&*crate::intern::sym::INPUT_A)
                    == Some(&crate::ir::Literal::String("n=".into()))),
        "the surviving concat carries the baked operand: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| std::sync::Arc::clone(&n.properties))
            .collect::<Vec<_>>()
    );
}

/// The fold pass does not delete a self-firing gate or the handler it drives.
/// `sweep_dead_exec` seeds every gate with an unwired exec input as untriggered,
/// but a `Timer`'s `restart`/`pause`/`resume` are optional controls on a
/// countdown that runs regardless, so a Timer with none wired is live.
#[test]
fn folding_keeps_a_self_firing_gate_and_its_handler() {
    let src = "let t = Timer(10.0)\nvar n: int\non t.Expired {\n  n = 1 + 1\n}\n";
    let folded = compile_folded(src);
    assert!(
        has_gate(&folded, gc::STOPWATCH),
        "the Timer must survive folding: {:?}",
        folded
            .module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    assert!(
        has_gate(&folded, gc::VAR_SET),
        "the handler's write must survive folding: {:?}",
        folded
            .module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // The exec edge from the Timer into the handler is intact.
    let timer = find_gate(&folded, gc::STOPWATCH);
    assert!(
        folded.module.wires.iter().any(|w| w.source.node_id == timer
            && w.target.port == crate::ir::port_registry::WirePort::Exec),
        "the Timer must still drive the handler"
    );

    // An event-driven handler survives, and a chain nothing can trigger (a
    // `let` exec signal no statement emits) is still swept: the behaviour the
    // seed exists for.
    let event = compile_ok_folded("var n: int\nin unused: exec\non RoundStart() { n = 1 }\n");
    assert_eq!(
        count_class(&event.module, gc::VAR_SET),
        1,
        "the RoundStart handler's write survives"
    );
    let orphan = compile_ok_folded("var n: int\nlet tail: exec\non tail { n = 1 }\n");
    assert_eq!(
        count_class(&orphan.module, gc::VAR_SET),
        0,
        "a never-emitted signal's handler is swept"
    );
}

/// An integer literal in a top-level `let` typechecked as `float` is the float
/// it names, so `L / 2` divides a float folded or not: never an integer 3 the
/// fold divides as an integer while the game would see the float.
#[test]
fn an_int_literal_in_a_top_level_float_let_is_a_float_folded_or_not() {
    let src =
        "in go: exec\nvar x: float = 0.0\nout ox: float = x\nlet L: float = 3\non go { x = L / 2 }";
    for r in [compile_ok(src), compile_ok_folded(src)] {
        let lits: Vec<crate::ir::Literal> = r
            .module
            .nodes_deep()
            .flat_map(|n| n.properties.values().cloned())
            .collect();
        assert!(!lits.contains(&crate::ir::Literal::Int(3)), "{lits:?}");
        assert!(
            lits.contains(&crate::ir::Literal::Float(3.0))
                || lits.contains(&crate::ir::Literal::Float(1.5)),
            "{lits:?}"
        );
    }
}

/// A builtin's `float`-declared math port takes an integer literal as
/// written: `clamp(n, 0, 8)`, `min(n, 4)`, `max(n, 1)` on an `int` keep their
/// integer bounds.
#[test]
fn an_int_literal_on_a_builtin_math_port_stays_an_int() {
    let r = compile_ok(
        "in go: exec\nin n: int\nvar r: int = 0\nvar q: int = 0\nvar s: int = 0\n\
         on go {\n  r = clamp(n, 0, 8)\n  q = min(n, 4)\n  s = max(n, 1)\n}",
    );
    let lits: Vec<crate::ir::Literal> = r
        .module
        .nodes_deep()
        .flat_map(|n| n.properties.values().cloned())
        .collect();
    for v in [0, 8, 4, 1] {
        assert!(lits.contains(&crate::ir::Literal::Int(v)), "{v}: {lits:?}");
        assert!(
            !lits.contains(&crate::ir::Literal::Float(v as f64)),
            "{v}: {lits:?}"
        );
    }
}
