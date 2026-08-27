use std::sync::Arc;

use super::*;
use crate::ir::Literal;

fn empty_ctx() -> ConstCtx<'static> {
    ConstCtx {
        consts: crate::collections::HashMap::default(),
        module_consts: crate::collections::HashMap::default(),
        enum_defs: Arc::new(crate::collections::HashMap::default()),
        lookup_mod: None,
    }
}

fn eval_str(src_expr: &str) -> Result<Literal, ConstError> {
    let p = crate::parse(&format!("let probe = {src_expr}"), "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else { panic!("expected a let") };
    eval_expr(&l.value, &empty_ctx(), &mut Budget::default())
}

/// Parses `src` (a whole script, typically an `enum` declaration plus a
/// top-level `const`/`let`), builds its whole-module `ConstEnv` the same way
/// a real compile does (`lower::build_const_env`, including the enum
/// registry), and returns the bound value for `name`.
fn eval_ok(src: &str, name: &str) -> Literal {
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let env = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    env.get(name)
        .unwrap_or_else(|| panic!("'{name}' did not evaluate to a compile-time constant; env = {env:?}"))
        .clone()
}

/// Parses `src`, takes its first `mod`/`chip` declaration, and calls it with
/// `args` through the interpreter using a fresh [`Budget`]. Any top-level
/// `const`/`let` in `src` becomes the MODULE constant environment, exactly as
/// `TypeCheckCtx`/`LowerCtx` build it — so a body referencing a module
/// constant resolves it here the same way it would in a real compile.
fn eval_mod_call(src: &str, args: &[Literal]) -> Result<Literal, ConstError> {
    eval_mod_call_with(src, args, Budget::default(), crate::collections::HashMap::default())
}

/// Same as [`eval_mod_call`], but with the call-chain DEPTH allowance
/// overridden (the step budget stays at its default).
fn eval_mod_call_with_budget(src: &str, args: &[Literal], depth: u32) -> Result<Literal, ConstError> {
    let mut budget = Budget::default();
    budget.depth = depth;
    eval_mod_call_with(src, args, budget, crate::collections::HashMap::default())
}

/// Same as [`eval_mod_call`], but with extra CALLER-scope-local constants
/// (the `scoped_consts` frames a real call site would have open) merged into
/// `consts` and deliberately absent from `module_consts` — the shape that
/// proves a callee cannot read the caller's locals.
fn eval_mod_call_with_caller_locals(
    src: &str,
    args: &[Literal],
    caller_locals: &[(&str, Literal)],
) -> Result<Literal, ConstError> {
    let mut locals = crate::collections::HashMap::default();
    for (name, lit) in caller_locals {
        locals.insert((*name).to_string(), lit.clone());
    }
    eval_mod_call_with(src, args, Budget::default(), locals)
}

fn eval_mod_call_with(
    src: &str,
    args: &[Literal],
    mut budget: Budget,
    caller_locals: crate::collections::HashMap<String, Literal>,
) -> Result<Literal, ConstError> {
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let decl = p
        .ast
        .decls
        .iter()
        .find_map(|d| match d {
            crate::ast::TopDecl::Chip(c) => Some(c),
            _ => None,
        })
        .expect("expected a mod/chip declaration");
    // Mirrors `TypeCheckCtx::const_ctx`: `module_consts` is the top-level env
    // alone, `consts` is that env with the caller's open scope frames merged
    // on top.
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let module_consts = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    let mut consts = module_consts.clone();
    for (name, lit) in caller_locals {
        consts.insert(name, lit);
    }
    let cx = ConstCtx { consts, module_consts, enum_defs: enum_defs.clone(), lookup_mod: None };
    eval_call(decl, args, &cx, &mut budget)
}

#[test]
fn evaluates_a_const_mod_body() {
    assert_eq!(
        eval_mod_call("const mod f(n: int) -> int { return n * 3 }", &[Literal::Int(4)]).unwrap(),
        Literal::Int(12)
    );
}

#[test]
fn evaluates_a_const_binding_inside_a_const_mod() {
    let src = "const mod f(n: int) -> int { const doubled = n * 2\n return doubled + 1 }";
    assert_eq!(eval_mod_call(src, &[Literal::Int(5)]).unwrap(), Literal::Int(11));
}

#[test]
fn a_non_const_statement_names_itself_in_the_error() {
    let src = "const mod f(n: int) -> int { BroadcastChatMessage(\"x\")\n return n }";
    let err = eval_mod_call(src, &[Literal::Int(1)]).unwrap_err();
    assert_eq!(err.code(), "WS046");
}

#[test]
fn a_runaway_call_chain_reports_ws048() {
    // Depth is bounded even though WS020 forbids true recursion: a long chain
    // of distinct const mods must fail with a diagnostic, never a stack
    // overflow.
    let err = eval_mod_call_with_budget("const mod f(n: int) -> int { return n }", &[Literal::Int(1)], 0);
    assert_eq!(err.unwrap_err().code(), "WS048");
}

/// A `const mod` body must see MODULE-level constants, exactly like an
/// ordinary `mod` body does (`lower::call::inline` pushes the parameter frame
/// on top of the module scope rather than replacing it). Seeding the callee
/// environment from the arguments alone would report the flatly untrue
/// "'N' is a runtime value" for a compile-time constant.
#[test]
fn a_const_mod_body_sees_module_level_constants() {
    let src = "const N = 4\nconst mod scaled(x: int) -> int { return x * N }";
    assert_eq!(eval_mod_call(src, &[Literal::Int(3)]).unwrap(), Literal::Int(12));
}

/// The motivating use case: building a custom-event channel name from a
/// module constant inside a `const mod`.
#[test]
fn a_const_mod_body_can_build_a_string_from_a_module_constant() {
    let src = "const PFX = \"evt_\"\nconst mod chan(s: string) -> string { return PFX .. s }";
    assert_eq!(
        eval_mod_call(src, &[Literal::String("hit".into())]).unwrap(),
        Literal::String("evt_hit".into())
    );
}

/// The hygiene property, and the guard against over-correcting the fix above
/// into a blanket `cx.consts` merge: a constant bound in the CALLER's local
/// scope is not a module constant, so the callee's body must not resolve it.
/// A mod body is its own scope — an ordinary `mod` body could not read a
/// caller's local `const` either.
#[test]
fn a_const_mod_body_cannot_see_the_callers_local_constants() {
    let src = "const mod peek(x: int) -> int { return x + callerLocal }";
    let err = eval_mod_call_with_caller_locals(src, &[Literal::Int(1)], &[("callerLocal", Literal::Int(9))])
        .unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::NotConstant("callerLocal".into()));
}

/// The parameter frame sits ON TOP of the module constants, so a parameter
/// shadows a module constant of the same name — the same shadowing an
/// ordinary mod's inner MODULE scope frame produces.
#[test]
fn a_parameter_shadows_a_module_constant_of_the_same_name() {
    let src = "const n = 100\nconst mod f(n: int) -> int { return n }";
    assert_eq!(eval_mod_call(src, &[Literal::Int(4)]).unwrap(), Literal::Int(4));
}

/// A missing argument must blame the parameter that has none, not surface
/// later as "'x' is a runtime value" from the first expression that reads it
/// — `x` is a parameter, not a runtime value, and the real fault is at the
/// call. Matches what `bind_call_args` already reports for a nested call.
#[test]
fn a_missing_argument_blames_the_parameter_not_the_body() {
    let err = eval_mod_call("const mod f(a: int, b: int) -> int { return a + b }", &[Literal::Int(1)])
        .unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(err.reason, ConstReason::Unsupported(w) if w.contains("missing an argument")),
        "expected a missing-argument reason, got {:?}",
        err.reason
    );
}

#[test]
fn evaluates_arithmetic_and_string_concatenation() {
    assert_eq!(eval_str("1 << 4").unwrap(), Literal::Int(16));
    assert_eq!(eval_str("\"a\" .. \"b\"").unwrap(), Literal::String("ab".into()));
}

#[test]
fn a_runtime_name_reports_which_name_is_not_constant() {
    let err = eval_str("someRuntimeThing + 1").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        err.message().contains("someRuntimeThing"),
        "the message must name the offending value, got {:?}",
        err.message()
    );
}

/// `1 << 100` has two constant operands; `eval_const_binop` refuses the shift
/// because the distance is outside `0..64`. Blaming an operand would tell the
/// user a literal is a runtime value.
#[test]
fn an_operator_refusing_constant_operands_is_not_blamed_on_the_operands() {
    let err = eval_str("1 << 100").unwrap_err();
    assert_eq!(err.code(), "WS047");
    assert!(
        matches!(err.reason, ConstReason::Refused(_)),
        "got {:?}",
        err.reason
    );
    assert!(
        !err.message().contains("not a compile-time constant"),
        "both operands ARE constant — the operator refused them, got {:?}",
        err.message()
    );
    assert!(
        err.message().contains("<<"),
        "the message must name the operator, got {:?}",
        err.message()
    );
}

/// `Vec` is a builtin constructor that `expr_to_literal_in` folds itself, so a
/// failure inside one is about the argument, not a missing `const mod`.
#[test]
fn a_constructor_call_blames_its_runtime_argument_not_the_constructor() {
    let err = eval_str("Vec(someRuntimeThing, 1.0, 2.0)").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(
        err.reason,
        ConstReason::NotConstant("someRuntimeThing".into())
    );
    assert!(
        !err.message().contains("Vec"),
        "the argument is the problem, not the constructor, got {:?}",
        err.message()
    );
}

/// Guards `FOLDABLE_CONSTRUCTORS` against drifting from the constructor match
/// in `lower::predeclare::expr_to_literal_lit` — a name listed there that no
/// longer folds would be reported as `Refused` forever instead of as a real
/// unknown-mod error.
///
/// Also guards the two CATALOG properties `bind_constructor_args` depends on,
/// neither of which is visible from the fold itself:
///
/// 1. Every listed name IS a `catalog::calls` entry — that is where the
///    parameter names for named-argument binding come from. A miss there
///    silently degrades every `Vec(x = …)` to a refusal.
/// 2. Every OPTIONAL parameter is in LAST position. This is what makes
///    `bind_constructor_args`' stop-at-the-first-unbound-parameter `break`
///    sound: a fold requires a list length of 3 or 4, and with optionals
///    trailing, any such length forces every earlier parameter to have been
///    bound — so a hole can never fold with values shifted onto the wrong
///    slots. A future constructor with an optional param in the MIDDLE would
///    silently break that argument, and nothing else in the suite would
///    notice.
#[test]
fn foldable_constructors_are_all_really_foldable() {
    for name in super::expr::FOLDABLE_CONSTRUCTORS {
        assert!(
            eval_str(&format!("{name}(1.0, 2.0, 3.0)")).is_ok(),
            "'{name}' is listed as a foldable constructor but no longer folds"
        );

        let spec = crate::catalog::calls::find_call(name).unwrap_or_else(|| {
            panic!("'{name}' is a foldable constructor but has no catalog entry to bind named arguments through")
        });
        let first_optional = spec.params.iter().position(|p| p.optional);
        if let Some(i) = first_optional {
            assert!(
                spec.params[i..].iter().all(|p| p.optional),
                "'{name}' has a REQUIRED parameter after an optional one \
                 ({:?}) — `bind_constructor_args` stops collecting at the first \
                 unbound parameter, which is only sound while every optional \
                 parameter trails",
                spec.params.iter().map(|p| (p.name, p.optional)).collect::<Vec<_>>()
            );
        }
    }
}

/// A constructor whose arguments are all constant but which still does not fold
/// (wrong arity here) is the constructor declining them — WS047, not a claim
/// that the builtin is an undeclared mod.
#[test]
fn a_constructor_declining_constant_arguments_is_refused_not_a_mod_call() {
    let err = eval_str("Vec(1.0, 2.0)").unwrap_err();
    assert_eq!(err.code(), "WS047");
    assert!(
        matches!(err.reason, ConstReason::Refused(_)),
        "got {:?}",
        err.reason
    );
}

/// The reported range must cover the offending sub-expression the message
/// names, not the whole enclosing call — otherwise `Vec(someRuntimeThing, 1.0,
/// 2.0)` correctly names `someRuntimeThing` in the message but underlines the
/// entire `Vec(...)` call in the editor.
#[test]
fn the_reported_range_blames_the_offending_sub_expression_not_the_whole_call() {
    let src_expr = "Vec(someRuntimeThing, 1.0, 2.0)";
    let full_src = format!("let probe = {src_expr}");
    let err = eval_str(src_expr).unwrap_err();
    let want_start = full_src.find("someRuntimeThing").unwrap();
    let want_end = want_start + "someRuntimeThing".len();
    assert_eq!(
        err.range.start.offset, want_start,
        "range should start at the offending argument, not the whole call: {:?}",
        err.range
    );
    assert_eq!(
        err.range.end.offset, want_end,
        "range should end at the offending argument, not the whole call: {:?}",
        err.range
    );
}

#[test]
fn evaluates_interpolation_and_string_methods() {
    assert_eq!(eval_str("\"a${1 + 1}b\"").unwrap(), Literal::String("a2b".into()));
    assert_eq!(eval_str("\"hi\".ToUpper()").unwrap(), Literal::String("HI".into()));
    assert_eq!(eval_str("\"  x  \".Trim().Length()").unwrap(), Literal::Int(1));
}

#[test]
fn a_refused_value_reports_ws047_not_ws046() {
    // Non-ASCII string operands are never certified for folding, so const
    // must refuse rather than guess — with a DIFFERENT code, because the fix
    // is not "stop using a variable".
    let err = eval_str("\"café\".ToUpper()").unwrap_err();
    assert_eq!(err.code(), "WS047");
}

/// The layering seam in `expr.rs`'s top-of-file NOTE is only safe while the
/// two evaluators agree on their overlap. Each case is evaluated by
/// const_eval and by the fold pass's certified evaluator directly; a
/// divergence here is a correctness bug in one of them.
#[test]
fn both_evaluators_agree_where_both_are_defined() {
    use crate::lower::fold::eval::Value;

    // Parse a two-operand `left op right` expression and hand its operands to
    // the certified evaluator directly, bypassing const_eval entirely.
    fn eval_via_fold(src_expr: &str, gate_class: &str) -> Option<Literal> {
        let p = crate::parse(&format!("let probe = {src_expr}"), "test");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else { panic!("expected a let") };
        let crate::ast::Expr::BinOp { left, right, .. } = &l.value else {
            panic!("expected a binary expression")
        };
        let env = crate::lower::ConstEnv::default();
        let a = Value::from_literal(&crate::lower::expr_to_literal_in(left, &env).unwrap())?;
        let b = Value::from_literal(&crate::lower::expr_to_literal_in(right, &env).unwrap())?;
        // `fold::eval::eval` is a pure table lookup over already-evaluated
        // `Value`s (one certified wire-gate law per gate class) — not a
        // code/string evaluator, despite the name. See the identical note in
        // `expr.rs` at its own call site.
        crate::lower::fold::eval::eval(gate_class, &[Some(a), Some(b)]).map(|v| v.to_literal())
    }

    for (src, gate_class) in [
        ("2 + 3", "BrickComponentType_WireGraph_Expr_MathAdd"),
        ("7 - 2", "BrickComponentType_WireGraph_Expr_MathSubtract"),
        ("\"a\" .. \"b\"", crate::ir::gate_class::STRING_CONCATENATE),
    ] {
        let ours = eval_str(src).unwrap();
        let theirs = eval_via_fold(src, gate_class).expect("fold must evaluate this too");
        assert_eq!(ours, theirs, "evaluators disagree on {src}");
    }
}

#[test]
fn a_const_if_expression_only_evaluates_the_taken_arm() {
    assert_eq!(eval_str("if true then 1 else 2").unwrap(), Literal::Int(1));
    // The untaken arm need not even be const — that is the point.
    assert_eq!(eval_str("if false then someRuntimeThing else 9").unwrap(), Literal::Int(9));
}

/// A `const` binding whose value is itself a `match` expression
/// folds to the taken arm's value -- `Expr::MatchExpr`'s own const-eval arm.
/// Uses `eval_ok` (not `eval_str`/`empty_ctx`) because folding a real enum
/// construction needs the enum registry `build_const_env` builds.
#[test]
fn a_const_match_folds_to_the_taken_arms_value() {
    let lit = eval_ok(
        "enum Shape { Empty, Circle(float) }\n\
         const AREA = match Shape.Circle(3.0) { Circle(r) => r, Empty => 0.0 }\n",
        "AREA",
    );
    assert_eq!(lit, Literal::Float(3.0));
}

/// The other arm: a scrutinee whose `__disc` names the UNIT variant takes
/// that arm's value, not the payload one -- proves the fold reads the actual
/// disc rather than always taking the first (or last) arm.
#[test]
fn a_const_match_takes_the_matching_unit_variant_arm() {
    let lit = eval_ok(
        "enum Shape { Empty, Circle(float) }\n\
         const AREA = match Shape.Empty { Circle(r) => r, Empty => 0.0 }\n",
        "AREA",
    );
    assert_eq!(lit, Literal::Float(0.0));
}

/// The scrutinee need not be an inline construction -- a `const` NAME bound to
/// one folds the same way, proving the match reads the scrutinee's VALUE
/// (via `eval_expr`, which resolves a named constant through `cx.consts`),
/// not its syntactic shape.
#[test]
fn a_const_match_on_a_named_constant_scrutinee_still_folds() {
    let lit = eval_ok(
        "enum Shape { Empty, Circle(float) }\n\
         const s = Shape.Circle(5.0)\n\
         const AREA = match s { Circle(r) => r, Empty => 0.0 }\n",
        "AREA",
    );
    assert_eq!(lit, Literal::Float(5.0));
}

/// An all-unit match folds when the SCRUTINEE names the enum directly
/// (`Dir.E`), even though every arm name parses as a `Pattern::Binding` rather
/// than a `Pattern::Variant` -- the scrutinee reference disambiguates the
/// governing enum where arm citations alone cannot. Folds to the ACTUAL taken
/// arm (E -> 20), not the first, proving the disc drives the choice.
#[test]
fn a_const_all_unit_match_folds_via_scrutinee_enum_reference() {
    let lit = eval_ok(
        "enum Dir { N, E, S, W }\n\
         const D = match Dir.E { N => 10, E => 20, S => 30, W => 40 }\n",
        "D",
    );
    assert_eq!(lit, Literal::Int(20));
}

/// The genuine residual: a match whose scrutinee is a named CONSTANT (which
/// does not name the enum) AND whose arms are all bare unit-variant names (no
/// `Pattern::Variant` citation) has no way for this evaluator to identify the
/// governing enum without type inference -- it refuses rather than guess, which
/// is always safe (it only costs the fold, never a wrong value).
#[test]
fn a_const_match_with_no_enum_identification_does_not_fold() {
    let p = crate::parse(
        "enum Dir { N, E }\n\
         const s = Dir.N\n\
         const D = match s { N => 1, E => 2 }\n",
        "test",
    );
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let env = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    assert!(
        env.get("D").is_none(),
        "a match that identifies no governing enum must not fold, got {:?}",
        env.get("D")
    );
}

#[test]
fn evaluates_const_collections() {
    assert_eq!(
        eval_str("[1, 2, 3]").unwrap(),
        Literal::Array(vec![Literal::Int(1), Literal::Int(2), Literal::Int(3)])
    );
    assert_eq!(eval_str("[10, 20][1]").unwrap(), Literal::Int(20));
    assert_eq!(eval_str("[10, 20, 30].length()").unwrap(), Literal::Int(3));
}

#[test]
fn an_out_of_range_const_index_is_an_error_not_a_stale_read() {
    // At runtime an out-of-range array read keeps the gate's PREVIOUS value.
    // At compile time there is no previous value and no excuse — say so.
    let err = eval_str("[1, 2][9]").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(
        err.reason,
        ConstReason::ArrayIndexOutOfRange { index: 9, len: 2 }
    );
    assert!(
        !err.message().contains("not a compile-time constant"),
        "both the array and the index ARE constant — the position just doesn't \
         exist, got {:?}",
        err.message()
    );
}

#[test]
fn evaluates_const_map_literals_and_indexing() {
    assert_eq!(
        eval_str("{ 1 => \"a\", 2 => \"b\" }").unwrap(),
        Literal::Map(vec![
            (Literal::Int(1), Literal::String("a".into())),
            (Literal::Int(2), Literal::String("b".into())),
        ])
    );
    assert_eq!(
        eval_str("{ 1 => \"a\", 2 => \"b\" }[2]").unwrap(),
        Literal::String("b".into())
    );
    assert_eq!(eval_str("{ 1 => \"a\", 2 => \"b\" }.length()").unwrap(), Literal::Int(2));
}

#[test]
fn a_missing_const_map_key_is_an_error_not_a_stale_read() {
    let err = eval_str("{ 1 => \"a\" }[2]").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::MapKeyNotFound);
}

#[test]
fn evaluates_a_const_record_and_its_fields() {
    assert_eq!(eval_str("{ rooms: 2, timer: 60 }.timer").unwrap(), Literal::Int(60));
}

/// A record field can itself be a record — field access chains through both
/// levels (`.a` yields the inner record, `.b` reads its field).
#[test]
fn evaluates_a_nested_const_record() {
    assert_eq!(eval_str("{ a: { b: 1 } }.a.b").unwrap(), Literal::Int(1));
}

/// Accessing a name that isn't one of the record's fields must say exactly
/// that — the record itself IS constant, so a generic "not a compile-time
/// constant" would blame the wrong thing (compare
/// `an_out_of_range_const_index_is_an_error_not_a_stale_read`, the same
/// distinction for arrays).
#[test]
fn field_access_on_a_missing_record_field_names_the_field() {
    let err = eval_str("{ rooms: 2 }.timer").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::RecordFieldNotFound("timer".to_string()));
    assert!(
        !err.message().contains("not a compile-time constant"),
        "the record IS constant — the field just doesn't exist, got {:?}",
        err.message()
    );
}

/// Same shape as [`eval_mod_call`], but resolves NESTED const-mod calls too:
/// `lookup_mod` scans every top-level `chip`/`mod` decl in `src` by name,
/// mirroring how `LowerCtx::resolve_mod`/`TypeCheckCtx::const_ctx` build
/// their own closures over the real decl table. `eval_mod_call`'s
/// `lookup_mod: None` is deliberate everywhere else — those tests probe a
/// single mod body in isolation — but a test proving one const mod's call
/// reaches ANOTHER (not just a named constant) needs a callee that can
/// actually resolve.
fn eval_mod_call_resolving(
    src: &str,
    callee_name: &str,
    args: &[Literal],
) -> Result<Literal, ConstError> {
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let chips: Vec<Arc<crate::ast::ChipDecl>> = p
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            crate::ast::TopDecl::Chip(c) => Some(Arc::new(c.clone())),
            _ => None,
        })
        .collect();
    let decl = chips
        .iter()
        .find(|c| c.name == callee_name)
        .unwrap_or_else(|| panic!("no decl named '{callee_name}'"));
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let module_consts = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    let lookup = |name: &str| chips.iter().find(|c| c.name == name).cloned();
    let cx = ConstCtx {
        consts: module_consts.clone(),
        module_consts,
        enum_defs: enum_defs.clone(),
        lookup_mod: Some(&lookup),
    };
    eval_call(decl, args, &cx, &mut Budget::default())
}

/// A const-mod call PER ELEMENT — not just named constants — inside an array
/// literal, forcing every element through the real call machinery
/// (`interp::eval_call`), not merely operand folding.
#[test]
fn a_const_mod_call_per_element_evaluates_inside_an_array_literal() {
    let src = "const mod double(n: int) -> int { return n * 2 }\n\
               const mod f() -> int[] { return [double(1), double(2), double(3)] }";
    assert_eq!(
        eval_mod_call_resolving(src, "f", &[]).unwrap(),
        Literal::Array(vec![Literal::Int(2), Literal::Int(4), Literal::Int(6)])
    );
}

/// A record built and returned by one const mod, field-read by another —
/// `f().b` recurses `eval_expr` into the call BEFORE reading the field, so
/// the call really does happen and the field really is read off its result,
/// not off some default.
#[test]
fn a_record_built_in_a_const_mod_is_field_readable_from_the_result() {
    let src = "const mod f() -> { a: int, b: int } { return { a: 1, b: 2 } }\n\
               const mod g() -> int { return f().b }";
    assert_eq!(eval_mod_call_resolving(src, "g", &[]).unwrap(), Literal::Int(2));
}

/// Evaluates `probe_expr` (as a top-level `let probe = <probe_expr>`)
/// appended after `mods_src`, with `lookup_mod` wired to resolve any
/// `chip`/`mod` declared in `mods_src` — mirrors [`eval_mod_call_resolving`]'s
/// closure-over-parsed-decls approach, but evaluates a bare EXPRESSION
/// through [`eval_expr`] rather than calling a specific decl through
/// [`eval_call`]. [`eval_str`]'s permanently-`None` `lookup_mod` can't be used
/// for the nested-call tests below: it would report every `const mod` call as
/// `NotAConstMod`, hiding the exact bug (`NestedConstModCall`) they exist to
/// catch.
fn eval_str_resolving(mods_src: &str, probe_expr: &str) -> Result<Literal, ConstError> {
    let src = format!("{mods_src}\nlet probe = {probe_expr}");
    let p = crate::parse(&src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let chips: Vec<Arc<crate::ast::ChipDecl>> = p
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            crate::ast::TopDecl::Chip(c) => Some(Arc::new(c.clone())),
            _ => None,
        })
        .collect();
    let lookup = |name: &str| chips.iter().find(|c| c.name == name).cloned();
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let module_consts = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    let crate::ast::TopDecl::Let(probe) = p.ast.decls.last().expect("expected at least one decl")
    else {
        panic!("expected the last decl to be `let probe = ...`")
    };
    let cx = ConstCtx {
        consts: module_consts.clone(),
        module_consts,
        enum_defs: enum_defs.clone(),
        lookup_mod: Some(&lookup),
    };
    eval_expr(&probe.value, &cx, &mut Budget::default())
}

/// `double(3) + 1`: `expr_to_literal_in` cannot fold either operand of `+` —
/// it has no notion of a `const mod` call at all — so `eval_expr`'s `BinOp`
/// arm must resolve a nested call like this one itself, rather than leaving
/// it to `reason_for`'s `BinOp` walk to (wrongly) name `NestedConstModCall`.
#[test]
fn a_const_mod_call_nested_in_a_binary_operator_evaluates() {
    let src = "const mod double(n: int) -> int { return n * 2 }";
    assert_eq!(eval_str_resolving(src, "double(3) + 1").unwrap(), Literal::Int(7));
}

/// Unary form of the same case: `-double(3)`.
#[test]
fn a_const_mod_call_nested_in_a_unary_operator_evaluates() {
    let src = "const mod double(n: int) -> int { return n * 2 }";
    assert_eq!(eval_str_resolving(src, "-double(3)").unwrap(), Literal::Int(-6));
}

/// Same case, inside a builtin constructor argument: `Vec`'s constructor
/// match only ever sees already-literal-folded arguments, so a `const mod`
/// call standing as one of `Vec`'s arguments must be evaluated before the
/// match runs, not left invisible to it.
#[test]
fn a_const_mod_call_nested_in_a_constructor_argument_evaluates() {
    let src = "const mod f(n: float) -> float { return n + 1.0 }";
    assert_eq!(
        eval_str_resolving(src, "Vec(f(1.0), 2.0, 3.0)").unwrap(),
        Literal::Vector { x: 2.0, y: 2.0, z: 3.0 }
    );
}

/// Both operands of `*` are themselves `const mod` calls, so this fails if
/// only the LEFT or only the RIGHT operand resolves.
#[test]
fn const_mod_calls_on_both_sides_of_an_operator_evaluate() {
    let src = "const mod double(n: int) -> int { return n * 2 }";
    assert_eq!(
        eval_str_resolving(src, "double(2) * double(3)").unwrap(),
        Literal::Int(24)
    );
}

/// A constructor's NAMED arguments bind by PARAMETER NAME, not by the order
/// they were written — `Vec(z = …, x = …, y = …)` must land each value on the
/// axis it names. Binding positionally instead silently produced
/// `Vector { x: 3.0, y: 1.0, z: 2.0 }`: not an error, just the wrong constant
/// baked into the gate. The runtime path (`lower::call::builtin`) has always
/// bound these by name, so a const and a non-const spelling of the identical
/// source must agree.
#[test]
fn constructor_named_arguments_bind_by_name_not_by_position() {
    assert_eq!(
        eval_str("Vec(z = 3.0, x = 1.0, y = 2.0)").unwrap(),
        Literal::Vector { x: 1.0, y: 2.0, z: 3.0 }
    );
    // Mixed positional + named: the positional args fill from the left, the
    // named one targets its own parameter regardless of where it appears.
    assert_eq!(
        eval_str("Rotation(1.0, roll = 3.0, yaw = 2.0)").unwrap(),
        Literal::Rotator { pitch: 1.0, yaw: 2.0, roll: 3.0 }
    );
    // `Color`'s 4th parameter is OPTIONAL: omitting it must still take the
    // alpha-defaults-to-opaque form rather than being treated as a hole.
    assert_eq!(
        eval_str("Color(b = 0.25, r = 0.5, g = 0.75)").unwrap(),
        Literal::LinearColor { r: 0.5, g: 0.75, b: 0.25, a: 1.0 }
    );
    assert_eq!(
        eval_str("Color(0.5, 0.75, 0.25, a = 0.5)").unwrap(),
        Literal::LinearColor { r: 0.5, g: 0.75, b: 0.25, a: 0.5 }
    );
}

/// A named argument matching no parameter, or a positional one past the last
/// parameter, has no constant form — it must refuse with WS047, never fold a
/// value from the arguments that happened to bind.
#[test]
fn a_constructor_argument_that_binds_no_parameter_is_refused() {
    for src in ["Vec(x = 1.0, y = 2.0, bogus = 3.0)", "Vec(1.0, 2.0, 3.0, 4.0)"] {
        let err = eval_str(src).unwrap_err();
        assert_eq!(err.code(), "WS047", "{src}");
        assert!(
            matches!(err.reason, ConstReason::Refused(_)),
            "{src}: got {:?}",
            err.reason
        );
    }
}

/// A HOLE — an unbound parameter with a bound one AFTER it — must refuse, not
/// silently close up. `bind_constructor_args` stops collecting at the first
/// unbound parameter (`break`); were it to SKIP that parameter instead
/// (`continue`), every later argument would slide one slot left onto the
/// wrong axis and still fold: `Color(0.5, 0.75, a = 0.25)` would come back as
/// `LinearColor { r: 0.5, g: 0.75, b: 0.25, a: 1.0 }`, landing the alpha value
/// on BLUE with no diagnostic at all.
///
/// This is the single line the whole named-argument fix rests on, so it gets
/// its own test: every case here folds successfully — to a wrong value —
/// under that one-word mutation, and the rest of the suite does not notice.
#[test]
fn a_hole_in_a_constructors_arguments_is_refused_not_closed() {
    for src in [
        // Optional `a` bound, required `b` not: the shape that mis-folds most
        // convincingly, because the resulting list is still a legal length.
        "Color(0.5, 0.75, a = 0.25)",
        "Color(r = 1.0, b = 1.0)",
        "Color(r = 1.0, g = 2.0, a = 0.5)",
        "Vec(x = 1.0, z = 3.0)",
        "Vec(1.0, 2.0)",
    ] {
        let err = eval_str(src).unwrap_err();
        assert_eq!(err.code(), "WS047", "{src} must refuse, not fold");
        assert!(
            matches!(err.reason, ConstReason::Refused(_)),
            "{src}: got {:?}",
            err.reason
        );
    }
}

/// Two arguments claiming ONE parameter is refused — the property
/// `bind_constructor_args`' doc comment claims and the runtime path does NOT
/// share (`lower::call::builtin` is last-write-wins, lowering this to
/// `x=2, y=3` with `z` unwired). Folding it would have to pick a winner
/// silently, so it stays an error instead.
#[test]
fn a_constructor_parameter_bound_twice_is_refused() {
    let err = eval_str("Vec(1.0, x = 2.0, y = 3.0)").unwrap_err();
    assert_eq!(err.code(), "WS047");
    assert!(
        matches!(err.reason, ConstReason::Refused(_)),
        "got {:?}",
        err.reason
    );
}

/// A spread argument to a constructor has no constant form — the same refusal
/// `Expr::Array` makes for a spread ELEMENT. Unwrapping it as though it were
/// an ordinary positional argument would bind an array literal to a scalar
/// axis and lean on `fold_constructor` to reject it incidentally.
#[test]
fn a_spread_argument_to_a_constructor_is_unsupported() {
    let err = eval_str("Vec(...someRuntimeThing, 2.0, 3.0)").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(err.reason, ConstReason::Unsupported(w) if w.contains("spread")),
        "expected a spread-specific reason, got {:?}",
        err.reason
    );
}

/// A spread has no constant form — arrays only support it as an
/// exec-context assignment RHS — so it must refuse rather than silently drop
/// or inline the spread source.
#[test]
fn a_spread_in_an_array_literal_is_unsupported() {
    let err = eval_str("[1, ...someRuntimeThing]").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(err.reason, ConstReason::Unsupported(w) if w.contains("spread")),
        "expected a spread-specific reason, got {:?}",
        err.reason
    );
}

/// The point of the whole phase: the emitted circuit's shape follows a
/// configuration constant. `t.push(10)` sits inside an `if` — proving a
/// mutation applied INSIDE a branch is still visible on the `return` after
/// it, not just a mutation at the mod's top level.
#[test]
fn a_const_mod_assembles_an_array_conditionally() {
    let src = "const mod rooms(n: int) -> int[] {\n\
                 const t = []\n\
                 if n >= 1 { t.push(10) }\n\
                 if n >= 2 { t.push(20) }\n\
                 return t\n\
               }";
    assert_eq!(eval_mod_call(src, &[Literal::Int(1)]).unwrap(), Literal::Array(vec![Literal::Int(10)]));
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(2)]).unwrap(),
        Literal::Array(vec![Literal::Int(10), Literal::Int(20)])
    );
    assert_eq!(eval_mod_call(src, &[Literal::Int(0)]).unwrap(), Literal::Array(vec![]));
}

#[test]
fn a_const_mod_supports_set_clear_and_append_on_arrays() {
    let set_src = "const mod f() -> int[] { const t = [1, 2, 3]\n t.set(1, 99)\n return t }";
    assert_eq!(
        eval_mod_call(set_src, &[]).unwrap(),
        Literal::Array(vec![Literal::Int(1), Literal::Int(99), Literal::Int(3)])
    );

    let clear_src = "const mod f() -> int[] { const t = [1, 2, 3]\n t.clear()\n return t }";
    assert_eq!(eval_mod_call(clear_src, &[]).unwrap(), Literal::Array(vec![]));

    let append_src = "const mod f() -> int[] {\n\
                         const t = [1, 2]\n\
                         const more = [3, 4]\n\
                         t.append(more)\n\
                         return t\n\
                       }";
    assert_eq!(
        eval_mod_call(append_src, &[]).unwrap(),
        Literal::Array(vec![Literal::Int(1), Literal::Int(2), Literal::Int(3), Literal::Int(4)])
    );
}

#[test]
fn an_out_of_range_const_set_is_an_error_not_a_stale_write() {
    let src = "const mod f() -> int[] { const t = [1, 2]\n t.set(9, 0)\n return t }";
    let err = eval_mod_call(src, &[]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::ArrayIndexOutOfRange { index: 9, len: 2 });
}

#[test]
fn a_const_mod_supports_set_remove_and_clear_on_maps() {
    let set_new_key_src = "const mod f() -> int { const m = { 1 => 10 }\n m.set(2, 20)\n return m[2] }";
    assert_eq!(eval_mod_call(set_new_key_src, &[]).unwrap(), Literal::Int(20));

    let overwrite_src = "const mod f() -> int { const m = { 1 => 10 }\n m.set(1, 20)\n return m[1] }";
    assert_eq!(eval_mod_call(overwrite_src, &[]).unwrap(), Literal::Int(20));

    let remove_src = "const mod f() -> int {\n\
                         const m = { 1 => 10, 2 => 20 }\n\
                         m.remove(1)\n\
                         return m.length()\n\
                       }";
    assert_eq!(eval_mod_call(remove_src, &[]).unwrap(), Literal::Int(1));

    let clear_src = "const mod f() -> int { const m = { 1 => 10 }\n m.clear()\n return m.length() }";
    assert_eq!(eval_mod_call(clear_src, &[]).unwrap(), Literal::Int(0));
}

/// Any method other than the ones this evaluator implements as a mutation
/// must fail naming the ACTUAL method the user wrote — `sort` is a real,
/// catalogued array method (`catalog::arrays::ARRAY_METHODS`), so this also
/// proves the failure isn't "unrecognized method" but specifically "not
/// implemented as a compile-time mutation".
#[test]
fn an_unsupported_array_method_names_itself_in_the_error() {
    let src = "const mod f() -> int[] { const t = [2, 1]\n t.sort()\n return t }";
    let err = eval_mod_call(src, &[]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(&err.reason, ConstReason::UnsupportedMethod(msg) if msg.contains("sort")),
        "expected the message to name the offending method, got {:?}",
        err.reason
    );
}

#[test]
fn an_unsupported_map_method_names_itself_in_the_error() {
    let src = "const mod f() -> int { const m = { 1 => 10 }\n m.has(1)\n return m.length() }";
    let err = eval_mod_call(src, &[]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(&err.reason, ConstReason::UnsupportedMethod(msg) if msg.contains("has")),
        "expected the message to name the offending method, got {:?}",
        err.reason
    );
}

/// `t` is declared OUTSIDE the `if`; inside it, a FRESH `const t = [99]`
/// SHADOWS the outer `t` — the push targets that inner, shadowed binding, so
/// the outer `t` must come back untouched once the `if` ends. This is the
/// exact trap the branch merge-back logic has to avoid: both "mutate the
/// binding inherited from the outer scope" and "redeclare a same-named
/// binding fresh inside the branch, then mutate THAT" write into
/// `branch_cx.consts` under the identical key "t", so merge-back cannot
/// just ask "does the outer scope already have this name" — it also has to
/// know the branch re-bound it locally.
#[test]
fn a_branch_local_redeclaration_does_not_leak_its_mutations() {
    let src = "const mod f() -> int[] {\n\
                 const t = [1]\n\
                 if true {\n\
                   const t = [99]\n\
                   t.push(100)\n\
                 }\n\
                 return t\n\
               }";
    assert_eq!(eval_mod_call(src, &[]).unwrap(), Literal::Array(vec![Literal::Int(1)]));
}

/// The companion case to the shadowing test above: a mutation of the SAME
/// outer binding (no redeclaration in the branch) DOES survive past the
/// `if` — otherwise the motivating `rooms`-style example
/// (`a_const_mod_assembles_an_array_conditionally`) could never work.
#[test]
fn a_branch_mutation_of_an_outer_binding_survives_the_branch() {
    let src = "const mod f() -> int[] {\n\
                 const t = [1]\n\
                 if true { t.push(2) }\n\
                 return t\n\
               }";
    assert_eq!(eval_mod_call(src, &[]).unwrap(), Literal::Array(vec![Literal::Int(1), Literal::Int(2)]));
}

/// A parameter can shadow a module constant of the same name (already true
/// for reads, see `a_parameter_shadows_a_module_constant_of_the_same_name`);
/// mutating the PARAMETER must not reach through to the module constant —
/// each call seeds its own fresh `env` from `cx.module_consts.clone()`, so
/// there is nothing shared to corrupt across calls in the first place, but
/// this nails it down for the mutation path specifically.
#[test]
fn mutating_a_shadowing_parameter_does_not_touch_the_module_constant() {
    let src = "const t = [1, 2, 3]\nconst mod f(t: int[]) -> int[] { t.push(9)\n return t }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Array(vec![Literal::Int(0)])]).unwrap(),
        Literal::Array(vec![Literal::Int(0), Literal::Int(9)])
    );
    // A second call with a fresh argument must not see the first call's
    // mutation leak in through a shared module constant.
    assert_eq!(
        eval_mod_call(src, &[Literal::Array(vec![Literal::Int(5)])]).unwrap(),
        Literal::Array(vec![Literal::Int(5), Literal::Int(9)])
    );
}

/// The order-sensitive half of branch shadowing: a name mutated FIRST and
/// shadowed LATER in the same branch must keep the earlier mutation. A
/// shadow confines only ITSELF to the branch — it is not a retroactive
/// barrier against everything that happened to the outer binding before it
/// appeared.
///
/// The first form of the merge-back skipped every branch-declared name
/// outright, which silently returned `[1]` here — a wrong constant with no
/// diagnostic, since branch shadowing is legal source that typecheck says
/// nothing about. Restoring the value the name held AT DECLARATION TIME
/// (already `[1, 2]` by then) is what makes both properties hold at once.
#[test]
fn a_shadow_declared_after_a_mutation_keeps_the_mutation() {
    let src = "const mod f() -> int[] {\n\
                 const t = [1]\n\
                 if true {\n\
                   t.push(2)\n\
                   const t = [9]\n\
                   t.push(10)\n\
                 }\n\
                 return t\n\
               }";
    assert_eq!(
        eval_mod_call(src, &[]).unwrap(),
        Literal::Array(vec![Literal::Int(1), Literal::Int(2)]),
        "the push before the shadow mutated the OUTER t and must survive; the \
         shadow's own [9, 10] must not"
    );
}

/// The mirror of the test above, for a name that was UNBOUND before the
/// branch declared it: the declaration must vanish completely when the
/// branch ends, not linger as an empty or default value. Restoring a
/// snapshot of `None` means REMOVING the key, so reading the name after the
/// `if` is the same "not a compile-time constant" it would have been if the
/// branch had never run.
#[test]
fn a_branch_declaration_of_a_previously_unbound_name_vanishes_entirely() {
    let src = "const mod f() -> int[] {\n\
                 if true {\n\
                   const fresh = [9]\n\
                   fresh.push(10)\n\
                 }\n\
                 return fresh\n\
               }";
    let err = eval_mod_call(src, &[]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(
        err.reason,
        ConstReason::NotConstant("fresh".into()),
        "a branch-local declaration must not survive its branch as ANY value — \
         got {:?}",
        err.reason
    );
}

/// The same name declared TWICE in one branch, after an outer mutation. The
/// snapshots have to be replayed in REVERSE declaration order so the
/// EARLIEST one (the true pre-shadow value, `[1, 2]`) is applied last and
/// wins; replaying them forward would leave the SECOND shadow's snapshot
/// (`[9]`) as the surviving value.
#[test]
fn repeated_shadows_in_one_branch_restore_the_earliest_snapshot() {
    let src = "const mod f() -> int[] {\n\
                 const t = [1]\n\
                 if true {\n\
                   t.push(2)\n\
                   const t = [9]\n\
                   const t = [99]\n\
                 }\n\
                 return t\n\
               }";
    assert_eq!(
        eval_mod_call(src, &[]).unwrap(),
        Literal::Array(vec![Literal::Int(1), Literal::Int(2)])
    );
}

/// Mutate-then-shadow one level deeper: the inner `if`'s restore has to
/// leave the middle branch's own view correct so the OUTER `if`'s merge
/// still sees `[1, 2, 3]`. A restore that reached too far (e.g. back to the
/// pre-branch `[1]`) would be invisible in the flat case above and only
/// surface here.
#[test]
fn a_shadow_after_a_mutation_nested_two_levels_deep_keeps_both_mutations() {
    let src = "const mod f() -> int[] {\n\
                 const t = [1]\n\
                 if true {\n\
                   t.push(2)\n\
                   if true {\n\
                     t.push(3)\n\
                     const t = [9]\n\
                     t.push(10)\n\
                   }\n\
                 }\n\
                 return t\n\
               }";
    assert_eq!(
        eval_mod_call(src, &[]).unwrap(),
        Literal::Array(vec![Literal::Int(1), Literal::Int(2), Literal::Int(3)])
    );
}

// ---------- compile-time record destructuring ----------

/// Parses `let <pattern_src> = <value_src>`, evaluates the value half
/// through `eval_expr` (against an empty environment — every case below is a
/// self-contained literal), and splits the result via `bind_destructured`.
/// Mirrors [`eval_str`]'s parse-one-declaration approach, but returns the
/// destructured `(name, value)` pairs rather than a single value.
fn bind_str(pattern_src: &str, value_src: &str) -> Result<Vec<(String, Literal)>, ConstError> {
    let src = format!("let {pattern_src} = {value_src}");
    let p = crate::parse(&src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else { panic!("expected a let") };
    let value = eval_expr(&l.value, &empty_ctx(), &mut Budget::default()).expect("value must evaluate");
    bind_destructured(&l.binding, value)
}

/// The core of `bind_destructured`: each field must land on ITS OWN name.
/// Asserting the exact returned pairs — name AND value together, in order —
/// is what makes this fail if the binding logic bound the wrong field to the
/// wrong name; a test using `{x: 1, y: 1}`-style equal values would pass
/// under a swapped binding just as easily as a correct one.
#[test]
fn bind_destructured_binds_each_field_by_its_own_name() {
    assert_eq!(
        bind_str("{ x, y }", "{ x: 11, y: 22 }").unwrap(),
        vec![("x".to_string(), Literal::Int(11)), ("y".to_string(), Literal::Int(22))]
    );
}

/// An alias binds to the ALIAS, not the source field name. Dropping the
/// alias (binding `name` instead of `alias.unwrap_or(name)`) would return
/// `[("x", 11), ("y", 22)]` instead — a different Vec the exact-pairs
/// assertion below catches directly.
#[test]
fn bind_destructured_binds_an_alias_to_the_alias_name() {
    assert_eq!(
        bind_str("{ x: a, y: b }", "{ x: 11, y: 22 }").unwrap(),
        vec![("a".to_string(), Literal::Int(11)), ("b".to_string(), Literal::Int(22))]
    );
}

/// `Rest` collects every field NO `Named` in the same pattern consumed, in
/// the SOURCE record's own field order (not pattern order, not reversed).
/// Dropping rest-collection entirely would return an EMPTY record here
/// instead of `{ x: 1, z: 3 }`; consuming `y` too (instead of excluding just
/// the `Named` field) would drop it from the rest as well.
#[test]
fn bind_destructured_rest_collects_the_unconsumed_fields_in_source_order() {
    assert_eq!(
        bind_str("{ y, ...rest }", "{ x: 1, y: 2, z: 3 }").unwrap(),
        vec![
            ("y".to_string(), Literal::Int(2)),
            (
                "rest".to_string(),
                Literal::Record(vec![
                    ("x".to_string(), Literal::Int(1)),
                    ("z".to_string(), Literal::Int(3)),
                ])
            ),
        ]
    );
}

/// A name with no matching field is `RecordFieldNotFound`, blamed on THAT
/// NAME's own range — not the whole pattern/statement.
#[test]
fn bind_destructured_a_missing_field_blames_the_names_own_range() {
    // `missing` is written FIRST (not last) so its field range ends at the
    // following comma, not at `}` — a trailing field's range extends to
    // swallow the whitespace before the closing brace, which is an
    // unrelated parser quirk this test must not couple itself to.
    let src = "let { missing, x } = { x: 1 }";
    let err = bind_str("{ missing, x }", "{ x: 1 }").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::RecordFieldNotFound("missing".into()));
    let want_start = src.find("missing").unwrap();
    let want_end = want_start + "missing".len();
    assert_eq!(
        err.range.start.offset, want_start,
        "must blame 'missing' itself, not the whole pattern: {:?}", err.range
    );
    assert_eq!(err.range.end.offset, want_end);
}

/// Destructuring a value that isn't a record at all is `Unsupported` (there
/// is no record to have a missing field in the first place).
#[test]
fn bind_destructured_of_a_non_record_value_is_unsupported() {
    let err = bind_str("{ x }", "1").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(&err.reason, ConstReason::Unsupported(w) if w.contains("not a record")),
        "got {:?}", err.reason
    );
}

/// A tuple pattern binds POSITIONALLY, in the source record's own field
/// order, ignoring field names — so the NAME-keyed record a multi-output
/// `const mod` produces splits exactly like the index-keyed one a tuple
/// literal produces.
#[test]
fn bind_destructured_of_a_tuple_pattern_binds_positionally() {
    let pairs = bind_str("(p, q)", "{ a: 11, b: 22 }").unwrap();
    assert_eq!(
        pairs,
        vec![
            ("p".to_string(), Literal::Int(11)),
            ("q".to_string(), Literal::Int(22)),
        ]
    );
}

/// A tuple pattern's `rest` takes every remaining position, RE-KEYED from
/// zero, matching how a tuple literal keys its own fields and how
/// `lower::decl::install_tuple_destruct` rebuilds the tail.
#[test]
fn bind_destructured_of_a_tuple_pattern_rekeys_its_rest() {
    let pairs = bind_str("(p, ...tail)", "{ a: 11, b: 22, c: 33 }").unwrap();
    assert_eq!(pairs[0], ("p".to_string(), Literal::Int(11)));
    assert_eq!(
        pairs[1],
        (
            "tail".to_string(),
            Literal::Record(vec![
                ("0".to_string(), Literal::Int(22)),
                ("1".to_string(), Literal::Int(33)),
            ])
        )
    );
}

/// A width mismatch names BOTH counts rather than reporting a generic
/// "cannot be evaluated": the value is perfectly constant and perfectly
/// positional — the pattern is simply the wrong width.
#[test]
fn bind_destructured_of_a_tuple_pattern_reports_both_arities() {
    let err = bind_str("(p, q, r)", "{ a: 11, b: 22 }").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(
            &err.reason,
            ConstReason::TupleArityMismatch { expected: 3, got: 2 }
        ),
        "got {:?}", err.reason
    );
}

/// A tuple pattern against a value that is not a record at all has no
/// positions to read, so it gives the same `Unsupported` a record pattern does.
#[test]
fn bind_destructured_of_a_tuple_pattern_on_a_non_record_is_unsupported() {
    let err = bind_str("(a, b)", "1").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(&err.reason, ConstReason::Unsupported(w) if w.contains("not a record")),
        "got {:?}", err.reason
    );
}

/// `LetBinding::Record { names }` (the plain, no-alias/no-rest destructure
/// shape) is unreachable from the current parser — `let { x, y } = …` always
/// produces `RecordDestruct` with plain `Named` fields — but
/// `bind_destructured` still has to handle it correctly, so it is
/// constructed directly here rather than left as untested dead code.
#[test]
fn bind_destructured_handles_the_plain_record_binding_form_directly() {
    let binding = crate::ast::LetBinding::Record {
        names: vec!["x".to_string(), "y".to_string()],
        range: crate::diagnostic::SourceRange::default(),
    };
    let value = Literal::Record(vec![
        ("x".to_string(), Literal::Int(11)),
        ("y".to_string(), Literal::Int(22)),
    ]);
    assert_eq!(
        bind_destructured(&binding, value).unwrap(),
        vec![("x".to_string(), Literal::Int(11)), ("y".to_string(), Literal::Int(22))]
    );
}

/// `bound_names` (syntactic, used by
/// `lower::predeclare::build_const_declared_names`, which has no value to
/// destructure) must list EXACTLY the names `bind_destructured` (semantic)
/// produces, in the same order. They are two functions answering "which
/// names does this binding introduce?", so nothing but a test keeps them
/// from drifting — and a drift is silent in the worst way: a name
/// `bound_names` omits is registered as a runtime value while
/// `bind_destructured` happily binds a constant to it, and a name
/// `bound_names` invents is marked const-declared with no value behind it.
///
/// Each case uses DISTINGUISHABLE field values so this also fails if the
/// two disagree about which field feeds which name, not merely about the
/// name set.
#[test]
fn bound_names_agrees_with_bind_destructured() {
    for (pattern, value_src) in [
        ("x", "{ a: 1, b: 2 }"),                    // Ident: the whole value
        ("{ a, b }", "{ a: 11, b: 22 }"),           // plain named fields
        ("{ a: p, b: q }", "{ a: 11, b: 22 }"),     // aliases
        ("{ a, ...rest }", "{ a: 11, b: 22, c: 33 }"), // rest
    ] {
        let src = format!("let {pattern} = {value_src}");
        let p = crate::parse(&src, "test");
        assert!(p.diagnostics.is_empty(), "{pattern}: {:?}", p.diagnostics);
        let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else { panic!("expected a let") };
        let value = eval_expr(&l.value, &empty_ctx(), &mut Budget::default()).unwrap();
        let bound: Vec<String> = bind_destructured(&l.binding, value)
            .unwrap_or_else(|e| panic!("{pattern}: {e:?}"))
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            super::destructure::bound_names(&l.binding),
            bound,
            "`bound_names` and `bind_destructured` disagree for `{pattern}`"
        );
    }
}

/// `interp::exec_block`'s `Stmt::Let` arm is a code path entirely separate
/// from `typecheck::decl`'s top-level site — fixing one does not fix the
/// other, so this exercises `bind_destructured` reached through the
/// interpreter specifically.
#[test]
fn a_const_destructure_inside_a_const_mod_body_binds() {
    let src = "const mod f() -> int {\n\
                 const p = { x: 3, y: 4 }\n\
                 const { x, y } = p\n\
                 return x * 100 + y\n\
               }";
    // A swapped binding (x reading y's field, or vice versa) would compute
    // 403 instead of 304.
    assert_eq!(eval_mod_call(src, &[]).unwrap(), Literal::Int(304));
}

#[test]
fn a_const_destructure_with_an_alias_inside_a_const_mod_binds_to_the_alias() {
    let src = "const mod f() -> int {\n\
                 const p = { x: 3, y: 4 }\n\
                 const { x: a, y: b } = p\n\
                 return a * 100 + b\n\
               }";
    assert_eq!(eval_mod_call(src, &[]).unwrap(), Literal::Int(304));
}

#[test]
fn a_const_destructure_with_a_rest_inside_a_const_mod_collects_remaining_fields() {
    let src = "const mod f() -> int {\n\
                 const p = { x: 3, y: 4, z: 5 }\n\
                 const { x, ...rest } = p\n\
                 return x * 10000 + rest.y * 100 + rest.z\n\
               }";
    assert_eq!(eval_mod_call(src, &[]).unwrap(), Literal::Int(30405));
}

#[test]
fn a_const_destructure_naming_a_missing_field_inside_a_const_mod_is_ws046() {
    let src = "const mod f() -> int {\n\
                 const p = { x: 1 }\n\
                 const { x, missing } = p\n\
                 return x\n\
               }";
    let err = eval_mod_call(src, &[]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::RecordFieldNotFound("missing".into()));
}

#[test]
fn a_const_destructure_of_a_non_record_inside_a_const_mod_is_ws046() {
    let src = "const mod f() -> int {\n\
                 const { x } = 1\n\
                 return x\n\
               }";
    let err = eval_mod_call(src, &[]).unwrap_err();
    assert_eq!(err.code(), "WS046");
}

/// The destructuring analogue of
/// `a_shadow_declared_after_a_mutation_keeps_the_mutation`: a destructure
/// binds SEVERAL names from ONE statement, and each needs its OWN shadow
/// snapshot taken immediately before ITS OWN insert. A single snapshot for
/// the whole statement would remember what only ONE of the names held
/// before the branch, so restoring it on branch exit would fix that ONE
/// name and leave every OTHER name the SAME destructure bound clobbering
/// whatever the outer scope held.
#[test]
fn a_branch_local_destructure_restores_every_shadowed_name_not_just_one() {
    let src = "const mod f() -> int {\n\
                 const a = 1\n\
                 const b = 2\n\
                 if true {\n\
                   const { a, b } = { a: 9, b: 99 }\n\
                 }\n\
                 return a * 1000 + b\n\
               }";
    assert_eq!(
        eval_mod_call(src, &[]).unwrap(),
        Literal::Int(1002),
        "both a and b must be restored to their PRE-branch values — not just \
         whichever one a single-snapshot-per-statement implementation \
         happened to catch"
    );
}

/// The mirror case: a destructure inside a branch introduces SEVERAL
/// previously-unbound names — every one of them must vanish entirely once
/// the branch ends, not just the first. Probed independently for each name
/// so a bug that only snapshots (and therefore only un-declares) ONE of
/// them is caught regardless of which one that happens to be.
#[test]
fn a_branch_local_destructure_of_unbound_names_makes_every_name_vanish() {
    for name in ["a", "b"] {
        let src = format!(
            "const mod f() -> int {{\n\
               if true {{\n\
                 const {{ a, b }} = {{ a: 9, b: 99 }}\n\
               }}\n\
               return {name}\n\
             }}"
        );
        let err = eval_mod_call(&src, &[]).unwrap_err();
        assert_eq!(err.code(), "WS046", "name={name}");
        assert_eq!(
            err.reason,
            ConstReason::NotConstant(name.to_string()),
            "name={name}: a branch-local destructured name must not survive \
             its branch as ANY value"
        );
    }
}

// ---------- `out`-form multi-output const mods ----------

/// The motivating bug: a `const mod` with `-> (a: int, b: int)` has no
/// `return` to produce a value from — its outputs are set via `out`
/// statements instead — so `eval_call` must assemble a `Literal::Record` from
/// whatever the body's `out` statements collected, exactly the shape a
/// `return { a: .., b: .. }` body already produces.
#[test]
fn a_multi_output_const_mod_yields_a_record() {
    let src = "const mod pair(n: int) -> (a: int, b: int) { out a = n\n out b = n + 1 }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(2)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(2)), ("b".to_string(), Literal::Int(3))])
    );
}

/// The record an `out`-form multi-output `const mod` yields destructures
/// exactly like the `return`-form one already did — `bind_destructured`
/// neither knows nor cares which statement shape produced the value.
#[test]
fn a_multi_output_const_mod_result_destructures() {
    let src = "const mod pair(n: int) -> (a: int, b: int) { out a = n\n out b = n + 1 }\n\
               const mod g(n: int) -> int {\n\
                 const { a, b } = pair(n)\n\
                 return a * 100 + b\n\
               }";
    assert_eq!(eval_mod_call_resolving(src, "g", &[Literal::Int(2)]).unwrap(), Literal::Int(203));
}

/// The record's fields must come out in the SIGNATURE's declaration order,
/// not the order the body happened to assign them in — `eval_call` walks
/// `decl.outputs`, not the `out` statements, to build the result. The body
/// below assigns `b` before `a`, the opposite of the signature; a naive
/// implementation that built the record in ASSIGNMENT order would return
/// `[("b", 3), ("a", 2)]`, which this exact-`Vec` equality would catch as a
/// different (and wrong) value from `[("a", 2), ("b", 3)]`.
#[test]
fn a_multi_output_const_mod_record_is_in_declaration_order() {
    let src = "const mod pair(n: int) -> (a: int, b: int) { out b = n + 1\n out a = n }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(2)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(2)), ("b".to_string(), Literal::Int(3))])
    );
}

/// An output the body never assigns via `out` (and that no `return` covers
/// either, since there is none here) is an error blamed on THAT output's own
/// declaration in the signature — not the call, not the whole body — so the
/// diagnostic underlines exactly the unassigned output. `b` is written
/// second in the signature so its range doesn't accidentally overlap `a`'s.
#[test]
fn an_unassigned_output_in_a_const_mod_is_an_error() {
    let src = "const mod pair(n: int) -> (a: int, b: int) { out a = n }";
    let err = eval_mod_call(src, &[Literal::Int(2)]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(err.reason, ConstReason::UnsupportedMessage(w) if w.contains("never assigns")),
        "got {:?}",
        err.reason
    );
    let want_start = src.find("b: int").unwrap();
    assert_eq!(
        err.range.start.offset, want_start,
        "must blame the unassigned output 'b' itself, not the call or the whole body: {:?}",
        err.range
    );
}

/// No regression: a single-output `const mod` (by far the most common shape)
/// must keep producing its `return` value — `eval_call` tries `return` FIRST
/// and only falls back to assembling a record from `out` statements when
/// there was none.
#[test]
fn a_single_output_const_mod_still_returns_its_value() {
    let src = "const mod f(n: int) -> int { return n * 3 }";
    assert_eq!(eval_mod_call(src, &[Literal::Int(4)]).unwrap(), Literal::Int(12));
}

/// ONE named output is NOT a record. `typecheck::call`'s
/// `type_user_symbol_call` unwraps a single output to its bare type
/// (`if sig.outputs.len() == 1 && !has_exec_arg { return out_ty(&sig.outputs[0].ty) }`),
/// so wrapping it here produced a value the type system called `int` while
/// const evaluation called it `Record([("r", Int)])` — a silent miscompile
/// with three separate manifestations (a wrong baked value, a spurious
/// WS028, and a hard `UnrepresentableLiteral` emit abort), each pinned by
/// its own test in `lower/tests` / `typecheck/tests`.
///
/// `docs/wirescript/chips.md` steers users straight into this shape: it
/// rejects `const chip C(v: int) -> (r: int)` and tells them to "use a mod".
#[test]
fn a_single_named_output_const_mod_yields_a_bare_value() {
    let src = "const mod C(v: int) -> (r: int) { out r = v }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(7)]).unwrap(),
        Literal::Int(7),
        "a single named output must unwrap to its bare value, exactly as \
         `typecheck::call` types it — NOT a 1-field record"
    );
}

/// A `const mod` with SEVERAL named outputs still yields a record — the
/// unwrap above is keyed on `len() == 1`, mirroring `typecheck::call`, not a
/// blanket "never wrap".
#[test]
fn a_two_named_output_const_mod_still_yields_a_record() {
    let src = "const mod C(v: int) -> (a: int, b: int) { out a = v\n out b = v + 1 }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(7)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(7)), ("b".to_string(), Literal::Int(8))])
    );
}

/// A SCALAR `return` in a mod that declares several named outputs has no
/// meaning: lowering wires a returned value only through `output_count() ==
/// 1` (or a record — see the test below), so with 2+ outputs the value is
/// silently dropped there. Letting it through const evaluation would yield
/// `Int(42)` where every consumer expects a record — a field read off it
/// (`c.a`) would then lower to a `SplitColor` gate, silently reinterpreting
/// the field as a colour channel.
///
/// Both statement orderings are covered. This is an EVALUATION-time error,
/// not a static one: an untaken guard `return` is never evaluated (the
/// untaken branch of a const `if` is never walked at all), so the `before`
/// case below only errors for the argument that actually takes the guard —
/// an untaken return produces no value to hijack anything with.
#[test]
fn a_valued_scalar_return_in_a_multi_output_const_mod_is_an_error() {
    let cases: &[(&str, &str, i64)] = &[
        (
            "return after the outs",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n out b = n + 1\n return 42 }",
            1,
        ),
        (
            // The guard-return spelling: the `return` runs BEFORE any `out`,
            // so nothing is collected when it wins.
            "guard return before the outs",
            "const mod cfg(n: int) -> (a: int, b: int) { if n < 0 { return 0 }\n out a = n\n out b = n + 1 }",
            -1,
        ),
    ];
    for (label, src, arg) in cases {
        let err = eval_mod_call(src, &[Literal::Int(*arg)]).unwrap_err();
        assert_eq!(err.code(), "WS046", "{label}");
        assert!(
            matches!(err.reason, ConstReason::UnsupportedMessage(_)),
            "{label}: got {:?}",
            err.reason
        );
    }
}

/// The exemption the rule above must NOT swallow: a RECORD `return` is the
/// documented multi-output mechanism (`lower::stmt`'s `Stmt::Return` arm
/// stashes a `RecordLit` as `pending_return_record` and forwards it
/// per-field, rather than wiring one value port), so it stays supported.
#[test]
fn a_record_return_in_a_multi_output_const_mod_is_still_forwarded() {
    let src = "const mod cfg(n: int) -> (a: int, b: int) { return { a: n, b: n + 1 } }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(1)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(1)), ("b".to_string(), Literal::Int(2))])
    );
}

/// The scoping property this test guards: the `outputs` accumulator is
/// threaded through the `Stmt::If` arm's recursive `exec_block` call, so an
/// `out` inside a TAKEN branch is still visible to `eval_call` after the
/// branch's own environment undo has run. Passing a fresh `&mut Vec::new()`
/// there instead would make `b` never reach `eval_call`, so the record
/// couldn't be built and this would fail with WS046 instead of a value.
///
/// Both arms are asserted, so the test also fails if only the `then` side is
/// threaded.
#[test]
fn an_out_inside_a_taken_branch_is_collected() {
    let src = "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n\
                 if n > 0 { out b = 77 } else { out b = 88 }\n\
               }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(1)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(1)), ("b".to_string(), Literal::Int(77))]),
        "an `out` in the taken THEN branch must survive the branch"
    );
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(0)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(0)), ("b".to_string(), Literal::Int(88))]),
        "an `out` in the taken ELSE branch must survive the branch"
    );
}

/// A BARE `return` is the natural early exit for the multi-output form (the
/// value comes from the `out`s, so there is nothing to return): it stops the
/// block and lets the collected outputs build the result, rather than being
/// refused as "a `return` with no value".
///
/// `n = 1` also pins that a bare `return` inside an `if` stops the OUTER
/// block, not merely the branch — the statement after the `if` must not run,
/// so `b` stays `1` rather than being overwritten with `999`.
#[test]
fn a_bare_return_in_a_multi_output_const_mod_stops_and_builds() {
    let src = "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n\
                 out b = 1\n\
                 if n > 0 { return }\n\
                 out b = 999\n\
               }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(1)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(1)), ("b".to_string(), Literal::Int(1))]),
        "the bare `return` must stop the whole body, so `out b = 999` never runs"
    );
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(0)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(0)), ("b".to_string(), Literal::Int(999))]),
        "with the guard untaken the body runs on and the later `out` wins"
    );
}

/// An `out` naming an output the SIGNATURE does not declare must NOT be
/// rejected: declaring an output in the BODY is a supported form (both
/// `lower/mod.rs` and `lower/call/instance_body.rs` route it to
/// `pre_declare_output`), so the identical body compiles clean as a plain
/// `mod`. It is also not part of the call's RESULT — `typecheck::call` types
/// a signature-bearing mod's call from the signature alone, reporting
/// `no field 'zz' on record (has: a, b)` for exactly this program — so
/// evaluating it and building the record from the signature (which ignores
/// it) is precisely the shape typecheck already promises.
///
/// There is no signal separating a typo from an intentional body-declared
/// output, so rejecting one without the other isn't possible.
#[test]
fn an_out_naming_an_output_outside_the_signature_is_ignored_not_rejected() {
    let src = "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n out zz = 999\n out b = n + 1 }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(1)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(1)), ("b".to_string(), Literal::Int(2))]),
        "the result is built from the SIGNATURE — a body-declared output is \
         not one of its fields"
    );
}

/// At arity 1 an `out` and a valued `return` can BOTH assign the one output,
/// and the two halves of the compiler disagree about which wins: lowering
/// keeps the FIRST assignment in SOURCE order and drops the rest, while
/// const evaluation short-circuits at the `return`. Without a check, both
/// report no errors:
///
/// ```text
/// mod pick(n: int) -> (r: int) { out r = 111  if n > 0 { return 222 } }
///   as a `const mod` -> bakes 222      as a plain `mod` -> wires 111
/// ```
///
/// So a valued `return` reached past an earlier `out` is refused, at any
/// arity. The differential half of this — that the two SPELLINGS of the same
/// program now agree — lives in `tests/const_differential.rs`.
#[test]
fn a_valued_return_after_an_earlier_out_is_an_error_at_arity_one() {
    for (label, src, arg) in [
        (
            "out then return",
            "const mod pick(n: int) -> (r: int) { out r = 111\n return 222 }",
            5,
        ),
        (
            // The measured repro: the `return` is guarded, so it is the
            // "assign a default, then override" idiom rather than dead code.
            "out then guarded return",
            "const mod pick(n: int) -> (r: int) { out r = 111\n if n > 0 { return 222 } }",
            5,
        ),
        (
            // The `out` never RUNS (its branch is untaken), but it still
            // lowers and still wins the port — so a "was anything collected"
            // test would wrongly accept this. Positional checking catches it.
            "out in an untaken branch, then return",
            "const mod pick(n: int) -> (r: int) { if n < 0 { out r = 111 }\n return 222 }",
            5,
        ),
    ] {
        let err = eval_mod_call(src, &[Literal::Int(arg)]).unwrap_err();
        assert_eq!(err.code(), "WS046", "{label}");
        assert!(
            matches!(err.reason, ConstReason::UnsupportedMessage(w) if w.contains("earlier `out`")),
            "{label}: expected the earlier-`out` conflict, got {:?}",
            err.reason
        );
    }
}

/// The converse ordering AGREES and must stay legal: with the `return` first
/// in source order, lowering's first-wins keeps the returned value and const
/// evaluation returns the same thing — rejecting this would break a working
/// program.
///
/// This is why the check is positional rather than "does this mod contain any
/// `out`".
#[test]
fn a_valued_return_before_a_later_out_is_still_allowed() {
    let src = "const mod pick(n: int) -> (r: int) { if n > 0 { return 222 }\n out r = 111 }";
    assert_eq!(eval_mod_call(src, &[Literal::Int(5)]).unwrap(), Literal::Int(222));
    // With the guard untaken the `out` supplies the value instead.
    assert_eq!(eval_mod_call(src, &[Literal::Int(0)]).unwrap(), Literal::Int(111));
}

/// A returned RECORD lands its fields in signature-declaration order, so the
/// `return { … }` path and the `out` path build identically-ordered records
/// for the same signature. The literal below is written in the OPPOSITE order
/// from the signature, so a fix that merely forwarded the literal as-is
/// returns `[("b", …), ("a", …)]` and fails here.
#[test]
fn a_returned_record_is_reordered_to_declaration_order() {
    let src = "const mod cfg(n: int) -> (a: int, b: int) { return { b: n + 1, a: n } }";
    assert_eq!(
        eval_mod_call(src, &[Literal::Int(1)]).unwrap(),
        Literal::Record(vec![("a".to_string(), Literal::Int(1)), ("b".to_string(), Literal::Int(2))])
    );
}

/// The pre-existing "falls off the end" error must survive all of the above:
/// a body that neither returns nor assigns any output still reports it, at
/// the body's own range. Without the collected-is-empty fallback this would
/// instead blame the anonymous `_` output of `-> int` as "never assigned",
/// which is both a worse message and a worse range for by far the most
/// common no-return mistake.
#[test]
fn a_body_that_produces_nothing_still_falls_off_the_end() {
    for src in [
        "const mod f(n: int) -> int { }",
        "const mod f(n: int) -> (a: int, b: int) { }",
    ] {
        let err = eval_mod_call(src, &[Literal::Int(1)]).unwrap_err();
        assert_eq!(err.code(), "WS046", "{src}");
        assert!(
            matches!(err.reason, ConstReason::Unsupported(w) if w.contains("falls off the end")),
            "{src}: got {:?}",
            err.reason
        );
    }
}

/// ...but a body that DID hit a `return` must not be described as falling off
/// the end. Because the `-> T` arrow form synthesises a `_` output, the
/// accurate "a `return` with no value" message must stay reachable for such
/// mods. The error is blamed on the `return` statement itself, which is why
/// `Flow` carries its range.
#[test]
fn a_bare_return_that_produces_nothing_blames_the_return_not_the_body() {
    let src = "const mod f(n: int) -> int { return }";
    let err = eval_mod_call(src, &[Literal::Int(1)]).unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert!(
        matches!(err.reason, ConstReason::Unsupported(w) if w.contains("`return` with no value")),
        "a body that hit a `return` must not claim it fell off the end, got {:?}",
        err.reason
    );
    let want_start = src.find("return").unwrap();
    assert_eq!(
        err.range.start.offset, want_start,
        "must blame the `return` statement itself: {:?}",
        err.range
    );
}

// ---------- enum const evaluation ----------

/// `Shape.Circle.Discriminant` is a compile-time int: a bare variant PATH's
/// `.Discriminant` resolves straight from the enum registry, no `obj`
/// evaluation involved.
#[test]
fn const_enum_discriminant_folds() {
    let v = eval_ok("enum Shape { Empty, Circle(float) }\nconst D = Shape.Circle.Discriminant\n", "D");
    assert_eq!(v, Literal::Int(1));
}

/// A unit variant reference folds to `{ __disc: N }` with no payload slots,
/// and reading `.Discriminant` back off THAT value (not off the bare path)
/// agrees with the registry.
#[test]
fn const_unit_variant_folds_and_its_discriminant_reads_back() {
    let src = "enum Dir { N, E, S, W }\nconst d = Dir.E\nconst D = d.Discriminant\n";
    assert_eq!(eval_ok(src, "D"), Literal::Int(1));
}

/// Positional construction folds every argument into `__{Variant}_{index}`
/// slots alongside `__disc`, matching `lower::predeclare::build_enum_fields`'s
/// slot-key format exactly (so a const-folded value and a runtime-baked one
/// agree on where the payload lives).
#[test]
fn const_positional_variant_construction_folds_its_slots() {
    let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
               const r = Shape.Rect(1.0, 2.0)\n";
    assert_eq!(
        eval_ok(src, "r"),
        Literal::Record(vec![
            ("__disc".to_string(), Literal::Int(2)),
            ("__Rect_0".to_string(), Literal::Float(1.0)),
            ("__Rect_1".to_string(), Literal::Float(2.0)),
        ])
    );
}

/// `.Discriminant` on a positionally-constructed value reads back through
/// its `__disc` slot, exactly like the unit-variant case above.
#[test]
fn const_positional_variant_discriminant_reads_back() {
    let src = "enum Shape { Empty, Circle(float) }\n\
               const D = Shape.Circle(5.0).Discriminant\n";
    assert_eq!(eval_ok(src, "D"), Literal::Int(1));
}

/// `.ToInt()` is an exact alias for `.Discriminant` in const position too: a
/// variant path folds straight from the registry, and a stored value reads its
/// `__disc` slot.
#[test]
fn const_enum_to_int_folds_like_discriminant() {
    let a = eval_ok("enum Shape { Empty, Circle(float) }\nconst A = Shape.Circle.ToInt()\n", "A");
    assert_eq!(a, Literal::Int(1));
    let b = eval_ok("enum Dir { N, E, S, W }\nconst d = Dir.S\nconst B = d.ToInt()\n", "B");
    assert_eq!(b, Literal::Int(2));
}

/// `Enum.FromInt(n)` folds to a tag-only record `{ __disc: n }` with no payload
/// slots (payloads default at runtime), matching the bare-variant-path fold.
#[test]
fn const_enum_from_int_folds_to_tag_only_record() {
    let v = eval_ok(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst e = Shape.FromInt(2)\n",
        "e",
    );
    assert_eq!(v, Literal::Record(vec![("__disc".to_string(), Literal::Int(2))]));
    // `FromInt(1)` also folds when the tagged variant, here `Circle`, is a
    // unit variant with no payload.
    let one = eval_ok("enum Shape { Empty, Circle }\nconst e = Shape.FromInt(1)\n", "e");
    assert_eq!(one, Literal::Record(vec![("__disc".to_string(), Literal::Int(1))]));
}

/// A const match on a `FromInt` value routes by the tag: disc 2 is `Rect`, so
/// the `Rect` arm is the one taken. The arm bodies ignore the (defaulted,
/// unreadable-in-const) payload captures.
#[test]
fn const_from_int_match_takes_arm_by_tag() {
    let v = eval_ok(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         const picked = match Shape.FromInt(2) { Rect(w, h) => 20, Circle(r) => 10, _ => 0 }\n",
        "picked",
    );
    assert_eq!(v, Literal::Int(20));
}

/// `EnumToInt(<const enum>)` folds to its discriminant int, the const
/// mirror of `.ToInt()` - a variant literal and a `const` value both fold.
#[test]
fn const_enum_to_integer_folds_to_disc() {
    let a = eval_ok(
        "enum Shape { Empty, Circle(float) }\nconst A = EnumToInt(Shape.Circle(1.0))\n",
        "A",
    );
    assert_eq!(a, Literal::Int(1));
    let b = eval_ok(
        "enum Dir { N, E, S, W }\nconst d = Dir.S\nconst B = EnumToInt(d)\n",
        "B",
    );
    assert_eq!(b, Literal::Int(2));
}

/// `IntToEnum(n)` folds to a tag-only `{ __disc: n }` record, matching
/// `Enum.FromInt(n)`'s const fold (payload slots default at runtime); the enum's
/// concrete type is irrelevant to the tag-only record.
#[test]
fn const_integer_to_enum_folds_to_tag_only_record() {
    let e = eval_ok(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst e = IntToEnum(2)\n",
        "e",
    );
    assert_eq!(e, Literal::Record(vec![("__disc".to_string(), Literal::Int(2))]));
}

/// A const `match` over `IntToEnum(n)` routes by the tag: disc 2 is `Rect`.
#[test]
fn const_integer_to_enum_match_takes_arm_by_tag() {
    let v = eval_ok(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         const picked = match IntToEnum(2) { Rect(w, h) => 20, Circle(r) => 10, _ => 0 }\n",
        "picked",
    );
    assert_eq!(v, Literal::Int(20));
}

/// A real variant literally named `FromInt` still constructs normally in const
/// position - the tag-only constructor never shadows a genuine variant.
#[test]
fn const_variant_named_from_int_constructs_normally() {
    let v = eval_ok("enum E { A, FromInt(int) }\nconst e = E.FromInt(3)\n", "e");
    assert_eq!(
        v,
        Literal::Record(vec![
            ("__disc".to_string(), Literal::Int(1)),
            ("__FromInt_0".to_string(), Literal::Int(3)),
        ])
    );
}

/// Named-field (`VariantCtor`) construction folds each field into
/// `__{Variant}_{fieldName}`, the braced-syntax sibling of the positional
/// case above.
#[test]
fn const_named_variant_construction_folds_its_slots() {
    let src = "enum Shape { Empty, Box { w: float, h: float } }\n\
               const b = Shape.Box { w: 1.0, h: 2.0 }\n";
    assert_eq!(
        eval_ok(src, "b"),
        Literal::Record(vec![
            ("__disc".to_string(), Literal::Int(1)),
            ("__Box_w".to_string(), Literal::Float(1.0)),
            ("__Box_h".to_string(), Literal::Float(2.0)),
        ])
    );
}

/// A non-const positional argument declines the WHOLE construction (falls
/// back to runtime lowering) rather than folding a partial record. `D`
/// never settles, so it is simply absent from `build_const_env`'s result.
#[test]
fn const_variant_construction_with_a_non_const_argument_does_not_fold() {
    let src = "enum Shape { Empty, Circle(float) }\n\
               var live: float = 0.0\n\
               const D = Shape.Circle(live).Discriminant\n";
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let env = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    assert!(
        env.get("D").is_none(),
        "a construction with a non-const argument must not fold, got {:?}",
        env.get("D")
    );
}

/// A user `mod`/`chip` named after a prelude variant SHADOWS
/// the bare prelude variant in const-eval, matching typecheck. `build_const_env`
/// (a production path feeding both `TypeCheckCtx::const_env` and
/// `LowerCtx::const_env`) folds `let y = Some(5)`; typecheck resolves `Some(5)`
/// as an ordinary call to the user `mod Some`, so const-eval MUST NOT resolve
/// it to the prelude `Option.Some` instead - a `Literal::Record` with `__disc`
/// here would be the exact typecheck-vs-const-eval disagreement the shadow
/// alignment forbids. A plain `mod Some` is not a `const mod`, so const-eval
/// refuses to fold the call at all: `y` is simply absent (falls back to runtime
/// lowering), like the non-const-argument case above.
#[test]
fn a_user_mod_named_after_a_prelude_variant_shadows_it_in_const_eval() {
    let src = "mod Some(x: int) -> (r: int) { return x + 1 }\n\
               let y = Some(5)\n";
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let env = crate::lower::build_const_env(&p.ast.decls, &enum_defs);
    match env.get("y") {
        None => {}
        Some(Literal::Record(fields)) if fields.iter().any(|(k, _)| k == "__disc") => panic!(
            "`Some(5)` wrongly folded to the prelude variant record instead of \
             shadowing to the user `mod Some`: {:?}",
            env.get("y")
        ),
        Some(other) => panic!("unexpected fold for `y`: {other:?}"),
    }
}
