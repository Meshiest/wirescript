use super::*;
use crate::ast::{CallArg, Expr};
use crate::diagnostic::SourceRange;
use crate::ir::Type;
use crate::typecheck::{SymbolInfo, SymbolKind, TypeCheckCtx};

// Build a bare Wire-param signature for testing check_args directly.
fn wire_sig(name: &str, params: &[(&str, Type, bool)]) -> CallSignature {
    CallSignature {
        name: name.into(),
        params: params
            .iter()
            .map(|(n, t, opt)| Param {
                name: (*n).into(),
                ty: t.clone(),
                optional: *opt,
                kind: ParamKind::Wire,
            })
            .collect(),
        config_gate: None,
    }
}

/// Declare a runtime `var` of type `ty` named `name` in `ctx`'s scope, so an
/// `Expr::Ident` reading it is a non-constant value (fails `expr_to_literal`).
fn declare_var(ctx: &mut TypeCheckCtx, name: &str, ty: Type) {
    ctx.scope.declare(
        name,
        SymbolInfo {
            kind: SymbolKind::Var,
            name: name.into(),
            ty,
            decl_range: SourceRange::default(),
            signature: None,
            event_data: None,
        },
    );
}

/// An `Expr::Ident` reading `name` at the zero range.
fn ident(name: &str) -> Expr {
    Expr::Ident {
        name: name.into(),
        range: SourceRange::default(),
    }
}

/// Register `a: int` in `ctx`'s scope and return two positional `CallArg`s
/// (both reading `a`, so both infer to `int`) plus a call range to attach
/// diagnostics to.
fn two_int_args(ctx: &mut TypeCheckCtx) -> (Vec<CallArg>, SourceRange) {
    declare_var(ctx, "a", Type::Int);
    let range = SourceRange::default();
    let arg = || CallArg::Positional(ident("a"));
    (vec![arg(), arg()], range)
}

#[test]
fn check_args_flags_arity_and_mismatch() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    let (args, range) = two_int_args(&mut ctx);

    // too few args -> WS011
    let sig3 = wire_sig(
        "f",
        &[
            ("x", Type::Int, false),
            ("y", Type::Int, false),
            ("z", Type::Int, false),
        ],
    );
    check_args(&mut ctx, &sig3, &args, 0, true, true, &range);
    assert!(
        ctx.diagnostics.iter().any(|d| d.code == "WS011"),
        "expected WS011 for too-few args, got {:?}",
        ctx.diagnostics
    );

    // right arity, param 1 expects vector, got int -> WS003
    ctx.diagnostics.clear();
    let sig2 = wire_sig(
        "f",
        &[("x", Type::Int, false), ("y", Type::Vector, false)],
    );
    check_args(&mut ctx, &sig2, &args, 0, true, true, &range);
    assert!(
        ctx.diagnostics
            .iter()
            .any(|d| d.code == "WS003" && d.message.contains("vector")),
        "expected WS003 mentioning 'vector', got {:?}",
        ctx.diagnostics
    );
}

#[test]
fn check_args_too_many_positional_is_ws011() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    let (args, range) = two_int_args(&mut ctx);

    let sig1 = wire_sig("f", &[("x", Type::Int, false)]);
    check_args(&mut ctx, &sig1, &args, 0, true, true, &range);
    assert!(
        ctx.diagnostics.iter().any(|d| d.code == "WS011"),
        "expected WS011 for too-many args, got {:?}",
        ctx.diagnostics
    );
}

#[test]
fn check_args_matching_types_is_clean() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    let (args, range) = two_int_args(&mut ctx);

    let sig2 = wire_sig("f", &[("x", Type::Int, false), ("y", Type::Int, false)]);
    check_args(&mut ctx, &sig2, &args, 0, true, true, &range);
    assert!(
        ctx.diagnostics.is_empty(),
        "expected no diagnostics, got {:?}",
        ctx.diagnostics
    );
}

#[test]
fn check_args_optional_param_omitted_is_ok() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "a", Type::Int);
    let range = SourceRange::default();
    let args = vec![CallArg::Positional(ident("a"))];

    let sig = wire_sig(
        "f",
        &[("x", Type::Int, false), ("y", Type::Int, true)],
    );
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    assert!(
        ctx.diagnostics.is_empty(),
        "omitting a trailing optional param should not error, got {:?}",
        ctx.diagnostics
    );
}

// A real gate carrying settable settings-menu config fields, used to exercise
// the config-arm dispatch against the actual inventory schema:
//   - `bOnlyHitPlayerBodyParts` (bool) and `Direction` (enum) are scalar config.
const SWEEP_GATE: &str = crate::ir::gate_class::SWEEP_SIMPLE;

// (a) A `ConfigScalar` param given a non-constant value emits the constant-only
// WS028 — reached via a NAMED arg matched by SURFACE name (`bodyPartsOnly`),
// which is the end-to-end path that would have caught the Fix 1 name conflict.
#[test]
fn config_scalar_named_arg_nonconstant_is_ws028() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "live", Type::Bool);
    let range = SourceRange::default();

    let sig = CallSignature {
        name: "SweepSimple".into(),
        // Surface name `bodyPartsOnly` differs from its port `bOnlyHitPlayerBodyParts`.
        params: vec![Param {
            name: "bodyPartsOnly".into(),
            ty: Type::Bool,
            optional: true,
            kind: ParamKind::ConfigScalar("bOnlyHitPlayerBodyParts"),
        }],
        config_gate: Some(SWEEP_GATE),
    };
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "bodyPartsOnly".into(),
        value: ident("live"),
    }];
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    let diag = ctx.diagnostics.iter().find(|d| d.code == "WS028");
    assert!(
        diag.is_some(),
        "non-constant scalar config via named arg should be WS028, got {:?}",
        ctx.diagnostics
    );
    // The message shows the SURFACE name, not the port name.
    assert!(
        diag.unwrap().message.contains("'bodyPartsOnly'"),
        "WS028 should name the surface param, got {:?}",
        diag.unwrap().message
    );
}

// (b) A `ConfigEnum(et)` param validates a bare member name against the schema:
// a known member is clean; an unknown one is WS028 (and never WS002).
#[test]
fn config_enum_arm_validates_bare_member() {
    let enum_type = crate::catalog::config_field_enum_type(SWEEP_GATE, "Direction")
        .expect("SweepSimple.Direction should back a schema enum");
    let range = SourceRange::default();
    let sig = CallSignature {
        name: "SweepSimple".into(),
        params: vec![Param {
            name: "direction".into(),
            ty: Type::Int,
            optional: true,
            kind: ParamKind::ConfigEnum(enum_type),
        }],
        config_gate: Some(SWEEP_GATE),
    };

    // A valid bare member -> no diagnostic.
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    let good = vec![CallArg::Positional(ident("Y_Positive"))];
    check_args(&mut ctx, &sig, &good, 0, true, true, &range);
    assert!(
        ctx.diagnostics.is_empty(),
        "a valid enum member should not error, got {:?}",
        ctx.diagnostics
    );

    // An unknown member -> WS028, not WS002 (must not read as a variable).
    ctx.diagnostics.clear();
    let bad = vec![CallArg::Positional(ident("Nope"))];
    check_args(&mut ctx, &sig, &bad, 0, true, true, &range);
    assert!(
        ctx.diagnostics.iter().any(|d| d.code == "WS028"),
        "unknown enum member should be WS028, got {:?}",
        ctx.diagnostics
    );
    assert!(
        !ctx.diagnostics.iter().any(|d| d.code == "WS002"),
        "a bare enum member must not read as an unknown identifier, got {:?}",
        ctx.diagnostics
    );
}

// (c) A named arg that matches NO declared param falls back to the gate's
// data-driven settings config via `config_gate` (`bOnlyHitPlayerBodyParts` is a
// raw schema field); a non-constant value there is WS028.
#[test]
fn named_arg_data_driven_config_fallback_is_ws028() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "live", Type::Bool);
    let range = SourceRange::default();

    let sig = CallSignature {
        name: "SweepSimple".into(),
        params: vec![], // no declared param by this name -> data-driven fallback
        config_gate: Some(SWEEP_GATE),
    };
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "bOnlyHitPlayerBodyParts".into(),
        value: ident("live"),
    }];
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    assert!(
        ctx.diagnostics.iter().any(|d| d.code == "WS028"),
        "non-constant data-driven config should be WS028, got {:?}",
        ctx.diagnostics
    );
}

// (d) A `ConfigComposite(port)` param rejects a non-constant value (can't fold
// to the expected shape) with WS028, and the message names the SURFACE param.
#[test]
fn config_composite_arm_rejects_nonconstant() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "live", Type::Color);
    let range = SourceRange::default();

    let sig = CallSignature {
        name: "AddInventoryItemAdv".into(),
        params: vec![Param {
            name: "meshColors".into(),
            ty: Type::Any,
            optional: true,
            kind: ParamKind::ConfigComposite("MeshColors"),
        }],
        config_gate: None,
    };
    // A bare identifier can't fold to a constant Color[].
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "meshColors".into(),
        value: ident("live"),
    }];
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    let diag = ctx.diagnostics.iter().find(|d| d.code == "WS028");
    assert!(
        diag.is_some(),
        "non-constant composite config should be WS028, got {:?}",
        ctx.diagnostics
    );
    // Message uses the surface name (`meshColors`), not the port (`MeshColors`).
    assert!(
        diag.unwrap().message.contains("'meshColors'"),
        "WS028 should name the surface param, got {:?}",
        diag.unwrap().message
    );
}

// (e) A named arg matching no declared param and no data-driven config field
// does nothing at lower time (both typecheck and emit silently drop it), so it
// is flagged WS041 — the `p.DisplayText(t, positionX = 0.0)` typo. The message
// names the call and the unknown parameter.
#[test]
fn unknown_named_arg_is_ws041() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "a", Type::Int);
    let range = SourceRange::default();
    // `position` is the real param; `positionX` is the removed per-axis alias.
    let sig = wire_sig("DisplayText", &[("position", Type::Vector, true)]);
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "positionX".into(),
        value: ident("a"),
    }];
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    let diag = ctx.diagnostics.iter().find(|d| d.code == "WS041");
    assert!(
        diag.is_some(),
        "an unknown named arg should be WS041, got {:?}",
        ctx.diagnostics
    );
    assert!(
        diag.unwrap().message.contains("positionX")
            && diag.unwrap().message.contains("DisplayText"),
        "WS041 should name the call and the unknown parameter, got {:?}",
        diag.unwrap().message
    );
}

// (f) `exec = ...` is the universal exec-override for a pure-context call, never
// a declared param — it must NOT be flagged unknown.
#[test]
fn exec_named_arg_is_not_ws041() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "go", Type::Any);
    let range = SourceRange::default();
    let sig = wire_sig("Get", &[("k", Type::Int, true)]);
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "exec".into(),
        value: ident("go"),
    }];
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    assert!(
        !ctx.diagnostics.iter().any(|d| d.code == "WS041"),
        "the `exec =` override must not be flagged unknown, got {:?}",
        ctx.diagnostics
    );
}

// (g) A variadic call that opts out of arity checking (`check_arity == false`,
// e.g. `arr.sortMultiple(other, descending = true)`) can't enumerate its legal
// named args in a fixed `Param` list, so no WS041 fires there.
#[test]
fn unknown_named_arg_skipped_when_arity_unchecked() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "d", Type::Bool);
    let range = SourceRange::default();
    let sig = wire_sig("sortMultiple", &[]);
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "descending".into(),
        value: ident("d"),
    }];
    check_args(&mut ctx, &sig, &args, 0, /*check_arity=*/ false, /*check_named=*/ false, &range);
    assert!(
        !ctx.diagnostics.iter().any(|d| d.code == "WS041"),
        "a variadic (arity-unchecked) call must not flag its named args, got {:?}",
        ctx.diagnostics
    );
}

// (g2) A user mod/chip call passes `check_arity = false` (its count is checked as
// WS022 upstream) but `check_named = true` — its full param list IS known, so an
// unknown named arg (`g(1, bogus = 5)`) must still be flagged WS041 (P0-16d).
#[test]
fn unknown_named_arg_flagged_when_arity_off_but_named_on() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "v", Type::Int);
    let range = SourceRange::default();
    let sig = wire_sig("g", &[("a", Type::Int, false)]);
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "bogus".into(),
        value: ident("v"),
    }];
    check_args(&mut ctx, &sig, &args, 0, /*check_arity=*/ false, /*check_named=*/ true, &range);
    assert!(
        ctx.diagnostics.iter().any(|d| d.code == "WS041"),
        "a user call's unknown named arg must be WS041 even with arity off, got {:?}",
        ctx.diagnostics
    );
}

// (h) A real settings-menu config field routed through `config_gate` is a known
// name — it is validated (WS028 here, for a non-constant value) but never WS041.
#[test]
fn known_config_named_arg_is_not_ws041() {
    let ce_slots = crate::typecheck::CeSlotMap::default();
    let mut ctx = crate::typecheck::TypeCheckCtx::new("t", &ce_slots);
    declare_var(&mut ctx, "live", Type::Bool);
    let range = SourceRange::default();
    let sig = CallSignature {
        name: "SweepSimple".into(),
        params: vec![],
        config_gate: Some(SWEEP_GATE),
    };
    let args = vec![CallArg::Named {
        name_range: SourceRange::default(),
        name: "bOnlyHitPlayerBodyParts".into(),
        value: ident("live"),
    }];
    check_args(&mut ctx, &sig, &args, 0, true, true, &range);
    assert!(
        !ctx.diagnostics.iter().any(|d| d.code == "WS041"),
        "a known config field must not be flagged unknown, got {:?}",
        ctx.diagnostics
    );
}
