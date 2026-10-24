use super::*;
use crate::ast::{CallArg, Expr};
use crate::diagnostic::SourceRange;
use crate::ir::Type;
use crate::test_support::report_failures;
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

/// A signature with one optional config param, or none, on `gate`.
fn config_sig(
    name: &str,
    param: Option<(&str, Type, ParamKind)>,
    gate: Option<&'static str>,
) -> CallSignature {
    CallSignature {
        name: name.into(),
        params: param
            .into_iter()
            .map(|(n, ty, kind)| Param {
                name: n.into(),
                ty,
                optional: true,
                kind,
            })
            .collect(),
        config_gate: gate,
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

fn pos(name: &str) -> CallArg {
    CallArg::Positional(ident(name))
}

fn named(name: &str, value: &str) -> CallArg {
    CallArg::Named {
        name_range: SourceRange::default(),
        name: name.into(),
        value: ident(value),
    }
}

/// What `check_args` must report for a row.
#[derive(Debug)]
enum Want {
    /// No diagnostic at all.
    Clean,
    /// A diagnostic with this code whose message contains every fragment.
    Has(&'static str, &'static [&'static str]),
    /// No diagnostic with this code.
    Lacks(&'static str),
}

struct Row {
    label: &'static str,
    vars: Vec<(&'static str, Type)>,
    sig: CallSignature,
    args: Vec<CallArg>,
    check_arity: bool,
    check_named: bool,
    wants: Vec<Want>,
}

fn row(
    label: &'static str,
    vars: Vec<(&'static str, Type)>,
    sig: CallSignature,
    args: Vec<CallArg>,
    wants: Vec<Want>,
) -> Row {
    Row {
        label,
        vars,
        sig,
        args,
        check_arity: true,
        check_named: true,
        wants,
    }
}

// A real gate carrying settable settings-menu config fields, used to exercise
// the config-arm dispatch against the actual inventory schema:
//   - `bOnlyHitPlayerBodyParts` (bool) and `Direction` (enum) are scalar config.
const SWEEP_GATE: &str = crate::ir::gate_class::SWEEP_SIMPLE;

#[test]
fn check_args_reports_arity_types_config_and_named_args() {
    use Want::*;
    let int = || Type::Int;
    let a = || vec![("a", Type::Int)];
    let direction = crate::catalog::config_field_enum_type(SWEEP_GATE, "Direction")
        .expect("SweepSimple.Direction should back a schema enum");
    let rows = vec![
        row(
            "too few args",
            a(),
            wire_sig(
                "f",
                &[
                    ("x", int(), false),
                    ("y", int(), false),
                    ("z", int(), false),
                ],
            ),
            vec![pos("a"), pos("a")],
            vec![Has("WS011", &[])],
        ),
        row(
            "right arity, param 1 expects vector, got int",
            a(),
            wire_sig("f", &[("x", int(), false), ("y", Type::Vector, false)]),
            vec![pos("a"), pos("a")],
            vec![Has("WS003", &["vector"])],
        ),
        row(
            "too many positional args",
            a(),
            wire_sig("f", &[("x", int(), false)]),
            vec![pos("a"), pos("a")],
            vec![Has("WS011", &[])],
        ),
        row(
            "matching types",
            a(),
            wire_sig("f", &[("x", int(), false), ("y", int(), false)]),
            vec![pos("a"), pos("a")],
            vec![Clean],
        ),
        row(
            "a trailing optional param omitted",
            a(),
            wire_sig("f", &[("x", int(), false), ("y", int(), true)]),
            vec![pos("a")],
            vec![Clean],
        ),
        // A `ConfigScalar` param given a non-constant value emits the constant-only
        // WS028, reached via a NAMED arg matched by SURFACE name (`bodyPartsOnly`,
        // port `bOnlyHitPlayerBodyParts`); the message names the surface param.
        row(
            "non-constant scalar config by surface name",
            vec![("live", Type::Bool)],
            config_sig(
                "SweepSimple",
                Some((
                    "bodyPartsOnly",
                    Type::Bool,
                    ParamKind::ConfigScalar("bOnlyHitPlayerBodyParts"),
                )),
                Some(SWEEP_GATE),
            ),
            vec![named("bodyPartsOnly", "live")],
            vec![Has("WS028", &["'bodyPartsOnly'"])],
        ),
        // A `ConfigEnum(et)` param validates a bare member name against the schema.
        row(
            "a valid bare enum member",
            vec![],
            config_sig(
                "SweepSimple",
                Some(("direction", int(), ParamKind::ConfigEnum(direction))),
                Some(SWEEP_GATE),
            ),
            vec![pos("Y_Positive")],
            vec![Clean],
        ),
        // An unknown member is WS028, never WS002: it must not read as a variable.
        row(
            "an unknown bare enum member",
            vec![],
            config_sig(
                "SweepSimple",
                Some(("direction", int(), ParamKind::ConfigEnum(direction))),
                Some(SWEEP_GATE),
            ),
            vec![pos("Nope")],
            vec![Has("WS028", &[]), Lacks("WS002")],
        ),
        // A named arg that matches NO declared param falls back to the gate's
        // data-driven settings config via `config_gate` (`bOnlyHitPlayerBodyParts`
        // is a raw schema field): validated (WS028 for a non-constant value) but
        // a known name, so never WS041.
        row(
            "non-constant data-driven config fallback",
            vec![("live", Type::Bool)],
            config_sig("SweepSimple", None, Some(SWEEP_GATE)),
            vec![named("bOnlyHitPlayerBodyParts", "live")],
            vec![Has("WS028", &[]), Lacks("WS041")],
        ),
        // A `ConfigComposite(port)` param rejects a non-constant value (can't fold
        // to a Color[]); the message uses the surface name, not the port.
        row(
            "non-constant composite config",
            vec![("live", Type::Color)],
            config_sig(
                "AddInventoryItemAdv",
                Some((
                    "meshColors",
                    Type::Any,
                    ParamKind::ConfigComposite("MeshColors"),
                )),
                None,
            ),
            vec![named("meshColors", "live")],
            vec![Has("WS028", &["'meshColors'"])],
        ),
        // A named arg matching no declared param and no data-driven config field
        // does nothing at lower time, so it is flagged WS041 naming the call and
        // the parameter: the `p.DisplayText(t, positionX = 0.0)` typo.
        row(
            "unknown named arg",
            a(),
            wire_sig("DisplayText", &[("position", Type::Vector, true)]),
            vec![named("positionX", "a")],
            vec![Has("WS041", &["positionX", "DisplayText"])],
        ),
        // `exec = ...` is the universal exec-override for a pure-context call.
        row(
            "the exec= override",
            vec![("go", Type::Any)],
            wire_sig("Get", &[("k", int(), true)]),
            vec![named("exec", "go")],
            vec![Lacks("WS041")],
        ),
        // A REQUIRED param supplied by name satisfies arity
        // (`Sweep(origin, dir, distance = 100.0)` is complete).
        row(
            "a required param given by name",
            vec![("d", Type::Float)],
            wire_sig(
                "Sweep",
                &[
                    ("origin", Type::Float, false),
                    ("distance", Type::Float, false),
                ],
            ),
            vec![pos("d"), named("distance", "d")],
            vec![Lacks("WS011")],
        ),
        // Matching is case-sensitive, so a mis-cased name points at the real one.
        row(
            "a mis-cased named arg",
            vec![("r", Type::Float)],
            wire_sig("Sweep", &[("radius", Type::Float, true)]),
            vec![named("Radius", "r")],
            vec![Has("WS041", &["did you mean 'radius'"])],
        ),
        // A variadic call that opts out of arity checking
        // (`arr.sortMultiple(other, descending = true)`) can't enumerate its legal
        // named args, so no WS041 fires there.
        Row {
            check_arity: false,
            check_named: false,
            ..row(
                "a variadic call's named arg",
                vec![("d", Type::Bool)],
                wire_sig("sortMultiple", &[]),
                vec![named("descending", "d")],
                vec![Lacks("WS041")],
            )
        },
        // A user mod/chip call checks its count as WS022 upstream, but its full
        // param list IS known, so `g(1, bogus = 5)` is still WS041.
        Row {
            check_arity: false,
            ..row(
                "a user call's unknown named arg",
                vec![("v", int())],
                wire_sig("g", &[("a", int(), false)]),
                vec![named("bogus", "v")],
                vec![Has("WS041", &[])],
            )
        },
    ];

    let mut failures = Vec::new();
    for r in &rows {
        let ce_slots = crate::typecheck::CeSlotMap::default();
        let mut ctx = TypeCheckCtx::new("t", &ce_slots);
        for (name, ty) in &r.vars {
            declare_var(&mut ctx, name, ty.clone());
        }
        check_args(
            &mut ctx,
            &r.sig,
            &r.args,
            0,
            r.check_arity,
            r.check_named,
            &SourceRange::default(),
        );
        let d = &ctx.diagnostics;
        let unmet: Vec<&Want> = r
            .wants
            .iter()
            .filter(|w| match w {
                Clean => !d.is_empty(),
                Has(code, frags) => !d
                    .iter()
                    .any(|x| x.code == *code && frags.iter().all(|f| x.message.contains(f))),
                Lacks(code) => d.iter().any(|x| x.code == *code),
            })
            .collect();
        if !unmet.is_empty() {
            let got: Vec<String> = d
                .iter()
                .map(|x| format!("[{}] {}", x.code, x.message))
                .collect();
            failures.push(format!("{}: unmet {unmet:?}; got {got:#?}", r.label));
        }
    }
    report_failures(&failures, rows.len());
}
