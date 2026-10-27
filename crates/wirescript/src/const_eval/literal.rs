//! Folding an expression to a constant [`Literal`] without the full const-mod
//! interpreter: the operator laws, literal and constructor folds, and the
//! shape predicates the type checker and lowering share.

use crate::ast::{ArrayElem, CallArg, Expr, RecordLitField, TypeExpr};
use crate::const_eval::ConstEnv;
use crate::const_scope::Constants;
use crate::ir::{Literal, Type};
use crate::lower::fold;

/// Default initial literal for `Pseudo_Var` data structs, which tells the game
/// the variable's `wire_graph_variant` type. `None` for ref-like types, which
/// no literal spells; emit writes the port type's null ref instead.
///
/// Shared with typecheck, whose `const`-parameter body check seeds a
/// type-shaped placeholder literal (see `typecheck::decl`).
pub(crate) fn default_literal_for_var_type(t: &Type) -> Option<Literal> {
    match t {
        Type::Bool => Some(Literal::Bool(false)),
        Type::Int => Some(Literal::Int(0)),
        Type::String => Some(Literal::String(String::new())),
        Type::Vector => Some(Literal::Vector {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        Type::Rotator => Some(Literal::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        }),
        Type::Quat => Some(Literal::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }),
        Type::Color => Some(Literal::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }),
        Type::Controller | Type::Character | Type::Entity => Some(Literal::Object),
        // No literal spells a ref. With no initial value, emit writes the
        // port type's null ref.
        Type::Ref(_) | Type::Brick | Type::Component(_) => None,
        _ => Some(Literal::Float(0.0)),
    }
}

/// Evaluate a constant unary operator. `None` = not foldable, which preserves
/// whatever error the caller would already have reported.
///
/// Shared with `const_eval::expr::eval_expr`, so the operator's law lives in
/// one place for both evaluators.
pub(crate) fn eval_const_unop(operator: &str, v: Literal) -> Option<Literal> {
    use crate::catalog::operators::op;
    match (operator, v) {
        (op::NEG, Literal::Int(n)) => Some(Literal::Int(n.wrapping_neg())),
        (op::NEG, Literal::Float(f)) => Some(Literal::Float(-f)),
        (op::NOT, Literal::Bool(b)) => Some(Literal::Bool(!b)),
        (op::BIT_NOT, Literal::Int(n)) => Some(Literal::Int(!n)),
        _ => None,
    }
}

/// Ask the certified fold table what the gate returns for these operands.
///
/// Every `OpRule` for one operator names the same gate class, so the operand
/// types are not needed to find it. A non-finite float is refused rather than
/// baked, mirroring the driver guard at `fold/mod.rs`: that keeps float
/// `x / 0.0` on the `0.0` law below instead of baking `inf`.
pub(crate) fn certified_binop(operator: &str, l: &Literal, r: &Literal) -> Option<Literal> {
    let class = crate::catalog::operators::operators()
        .iter()
        .find(|s| s.op == operator)?
        .rules
        .first()?
        .gate_class;
    let (a, b) = (
        fold::eval::Value::from_literal(l)?,
        fold::eval::Value::from_literal(r)?,
    );
    let v = fold::eval::eval(class, &[Some(a), Some(b)])?;
    if matches!(&v, fold::eval::Value::Float(f) if !f.is_finite()) {
        return None;
    }
    Some(v.to_literal())
}

/// Evaluate a constant binary operator, matching the gates' certified
/// semantics: 64-bit integer maths, and division / modulo by zero yielding 0
/// rather than trapping. Anything outside this set, or any operand pair whose
/// result would be ambiguous, returns `None` and stays an error.
///
/// Shared with `const_eval::expr::eval_expr`, like [`eval_const_unop`].
#[expect(
    clippy::needless_pass_by_value,
    reason = "const_eval shares this signature and passes owned operands"
)]
pub(crate) fn eval_const_binop(operator: &str, l: Literal, r: Literal) -> Option<Literal> {
    use crate::catalog::operators::op;
    use Literal::{Bool, Float, Int, String as Str};

    // The certified table first: it is the probed record of what the game's
    // gate returns, so where it has an answer it wins over the law below.
    if let Some(v) = certified_binop(operator, &l, &r) {
        return Some(v);
    }

    // String concatenation is the one non-numeric binary fold.
    if let (Str(a), Str(b)) = (&l, &r) {
        return match operator {
            op::CONCAT => Some(Str(format!("{a}{b}"))),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            _ => None,
        };
    }
    if let (Bool(a), Bool(b)) = (&l, &r) {
        let (a, b) = (*a, *b);
        return match operator {
            op::AND => Some(Bool(a && b)),
            op::OR => Some(Bool(a || b)),
            op::XOR => Some(Bool(a != b)),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            _ => None,
        };
    }
    // Two ints stay integral (bitwise and shifts are int-only); any float
    // operand promotes the pair to float, mirroring the operator overloads.
    if let (Int(a), Int(b)) = (&l, &r) {
        let (a, b) = (*a, *b);
        return match operator {
            op::ADD => Some(Int(a.wrapping_add(b))),
            op::SUB => Some(Int(a.wrapping_sub(b))),
            op::MUL => Some(Int(a.wrapping_mul(b))),
            op::DIV => Some(Int(if b == 0 { 0 } else { a.wrapping_div(b) })),
            op::REM => Some(Int(if b == 0 { 0 } else { a.wrapping_rem(b) })),
            op::BIT_AND => Some(Int(a & b)),
            op::BIT_OR => Some(Int(a | b)),
            op::BIT_XOR => Some(Int(a ^ b)),
            // A shift distance outside 0..64 is left unfolded rather than guessed.
            op::SHL => (0..64).contains(&b).then(|| Int(a << b)),
            op::SHR => (0..64).contains(&b).then(|| Int(a >> b)),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            op::LT => Some(Bool(a < b)),
            op::LE => Some(Bool(a <= b)),
            op::GT => Some(Bool(a > b)),
            op::GE => Some(Bool(a >= b)),
            _ => None,
        };
    }
    let num = |v: &Literal| match v {
        Int(n) => Some(*n as f64),
        Float(f) => Some(*f),
        _ => None,
    };
    let (a, b) = (num(&l)?, num(&r)?);
    // Match the gates: a non-finite result reads as 0.
    let fin = |f: f64| Float(if f.is_finite() { f } else { 0.0 });
    match operator {
        op::ADD => Some(fin(a + b)),
        op::SUB => Some(fin(a - b)),
        op::MUL => Some(fin(a * b)),
        op::DIV => Some(if b == 0.0 { Float(0.0) } else { fin(a / b) }),
        op::REM => Some(if b == 0.0 { Float(0.0) } else { fin(a % b) }),
        op::EQ => Some(Bool(a == b)),
        op::NE => Some(Bool(a != b)),
        op::LT => Some(Bool(a < b)),
        op::LE => Some(Bool(a <= b)),
        op::GT => Some(Bool(a > b)),
        op::GE => Some(Bool(a >= b)),
        _ => None,
    }
}

/// Fold a constant-literal expression to a [`Literal`] (used for var/array
/// initial values). Returns `None` for anything that isn't a compile-time
/// constant. Shared with the type checker so both agree on what's a literal.
///
/// The environment-free form: it folds only literals, a negated literal and
/// literal-argument constructors, and never resolves names or operators.
/// It decides whether a value bakes into gate data or gets a wired gate, well
/// beyond initializers, so folding `a + b` here would delete gates such as the
/// `MakeRotation` of `Rotation(0.0 + 0.0, ...)`. Initializers use
/// [`expr_to_literal_in`].
pub(crate) fn expr_to_literal(e: &Expr) -> Option<Literal> {
    expr_to_literal_impl::<ConstEnv>(e, None)
}

/// [`expr_to_literal`], plus named constants in `env` and operators over them,
/// so an initializer can read `1 << C_FLAG` or `WIDTH * HEIGHT`.
pub(crate) fn expr_to_literal_in<C: Constants + ?Sized>(e: &Expr, env: &C) -> Option<Literal> {
    expr_to_literal_impl(e, Some(env))
}

/// `env == None` folds syntactically only; `Some` also resolves named
/// constants and evaluates operators.
fn expr_to_literal_impl<C: Constants + ?Sized>(e: &Expr, env: Option<&C>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // Constant-environment forms. With no environment these fall through
        // to `_ => None`.
        Expr::Ident { name, .. } => env?.constant(name).cloned(),
        Expr::BinOp {
            op, left, right, ..
        } if env.is_some() => eval_const_binop(op, expr_to_literal(left)?, expr_to_literal(right)?),
        Expr::UnOp { op, operand, .. }
            if env.is_some() && op != crate::catalog::operators::op::NEG =>
        {
            eval_const_unop(op, expr_to_literal(operand)?)
        }
        // Always-constant forms.
        Expr::IntLit { value, .. } => Some(Literal::Int(*value)),
        Expr::AtomLit { value, .. } => Some(Literal::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Literal::Float(*value)),
        Expr::BoolLit { value, .. } => Some(Literal::Bool(*value)),
        Expr::StringLit { value, .. } => Some(Literal::String(value.clone())),
        // `3 as float`, for the conversions measured in game.
        Expr::As {
            value,
            ty: TypeExpr::Name { name, .. },
            ..
        } => crate::typecheck::convert::fold_as(
            &expr_to_literal(value)?,
            &crate::types::resolve::primitive(name)?,
        ),
        // Negative numeric literals: `-5`, `-1.0`.
        Expr::UnOp { op, operand, .. } if op == crate::catalog::operators::op::NEG => {
            match expr_to_literal(operand)? {
                Literal::Int(n) => Some(Literal::Int(n.wrapping_neg())),
                Literal::Float(f) => Some(Literal::Float(-f)),
                _ => None,
            }
        }
        _ => expr_to_literal_lit(e, env),
    }
}

/// Fold a `Vec`/`Rotation`/`Color` builtin constructor call given its
/// already-evaluated argument literals. Shared with `const_eval::expr::eval_expr`;
/// `const_eval::expr::FOLDABLE_CONSTRUCTORS` lists the names accepted here.
/// `ColorSRGB` folds via [`fold_srgb_color`] instead.
///
/// `args` must be in parameter order: a caller with named arguments binds them
/// first (see `const_eval::expr::bind_constructor_args`), or values land on
/// the wrong axis.
pub(crate) fn fold_constructor(name: &str, args: &[Literal]) -> Option<Literal> {
    let mut nums = Vec::with_capacity(args.len());
    for a in args {
        match a {
            Literal::Int(n) => nums.push(*n as f64),
            Literal::Float(f) => nums.push(*f),
            _ => return None,
        }
    }
    match (name, nums.as_slice()) {
        ("Vec", &[x, y, z]) => Some(Literal::Vector { x, y, z }),
        ("Rotation", &[pitch, yaw, roll]) => Some(Literal::Rotator { pitch, yaw, roll }),
        // Color is linear RGBA in 0..1; alpha defaults to opaque.
        ("Color", &[r, g, b]) => Some(Literal::LinearColor { r, g, b, a: 1.0 }),
        ("Color", &[r, g, b, a]) => Some(Literal::LinearColor { r, g, b, a }),
        _ => None,
    }
}

/// The constructor / reference cases, split out to keep the dispatch above
/// readable.
fn expr_to_literal_lit<C: Constants + ?Sized>(e: &Expr, env: Option<&C>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // Constructor calls on constant numeric args fold to literals, so
        // `var v = Vec(1.0, 2.0, 3.0)` (and Rotation/Color) bakes into the
        // gate's initial value instead of being dropped.
        Expr::Call { callee, args, .. } => {
            let Expr::Ident { name, .. } = callee.as_ref() else {
                return None;
            };
            let mut lits = Vec::with_capacity(args.len());
            for a in args {
                let CallArg::Positional(arg) = a else {
                    return None;
                };
                lits.push(expr_to_literal(arg)?);
            }
            fold_constructor(name, &lits)
        }
        // Asset reference `$Type/Name`, inlined into the gate's component data.
        // A component type is stored as the asset `BrickComponentTypeBase/<class>`.
        Expr::AssetRef { asset_type, .. } if asset_type == crate::analysis::assets::COMPONENT => {
            crate::typecheck::props::component_type_arg(e).map(|class| Literal::Asset {
                asset_type: "BrickComponentTypeBase".to_string(),
                asset_name: class.to_string(),
            })
        }
        Expr::AssetRef {
            asset_type,
            asset_name,
            ..
        } => Some(Literal::Asset {
            asset_type: asset_type.clone(),
            asset_name: asset_name.clone(),
        }),
        // Prefab file reference `$./file.brz`, resolved and embedded at emit
        // into the gate's `Prefab` path string.
        Expr::PrefabRef { path, .. } => Some(Literal::PrefabRef { path: path.clone() }),
        // Inline nested-prefab block `$``` ... ``` `, compiled and embedded at
        // emit into the gate's `Prefab` path string.
        Expr::NestedPrefab { source, .. } => Some(Literal::NestedPrefab {
            source: source.clone(),
        }),
        _ => None,
    }
}

/// Fold a constant `ColorSRGB(r, g, b, a)` call to a `Literal::Color`: sRGB u8
/// with no gamma re-encoding (Brickadia stores brick colors sRGB-direct).
/// Anything else, including the linear `Color(..)` constructor (whose gamma
/// conversion would be ambiguous), folds to `None` so the caller reports it.
pub(crate) fn fold_srgb_color(e: &Expr) -> Option<Literal> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident { name, .. } = callee.as_ref() else {
        return None;
    };
    if name != "ColorSRGB" {
        return None;
    }
    let mut nums: Vec<i64> = Vec::with_capacity(args.len());
    for a in args {
        let CallArg::Positional(arg) = a else {
            return None;
        };
        match expr_to_literal(arg)? {
            Literal::Int(n) => nums.push(n),
            Literal::Float(f) => nums.push(f as i64),
            _ => return None,
        }
    }
    let [r, g, b, a] = nums.as_slice() else {
        return None;
    };
    let clamp_u8 = |n: i64| n.clamp(0, 255) as u8;
    Some(Literal::Color {
        r: clamp_u8(*r),
        g: clamp_u8(*g),
        b: clamp_u8(*b),
        a: clamp_u8(*a),
    })
}

/// Fold a `meshColors` argument (an array literal of constant `ColorSRGB`
/// colors, or a constant bound to an array of `Literal::Color`) to a
/// `Literal::Array` of colors for a gate's `MeshColors: Color[]` data field.
/// `None` (a call-site error) for any other element or shape.
pub(crate) fn fold_mesh_colors<C: Constants + ?Sized>(e: &Expr, env: &C) -> Option<Literal> {
    if let Expr::Ident { name, .. } = e {
        return match env.constant(name) {
            Some(lit @ Literal::Array(items))
                if items.iter().all(|c| matches!(c, Literal::Color { .. })) =>
            {
                Some(lit.clone())
            }
            _ => None,
        };
    }
    let Expr::Array { elements, .. } = e else {
        return None;
    };
    let mut colors = Vec::with_capacity(elements.len());
    for el in elements {
        let ArrayElem::Item(item) = el else {
            return None;
        };
        colors.push(fold_srgb_color(item)?);
    }
    Some(Literal::Array(colors))
}

/// Whether `top` is exactly the [`fold_ammo_override`] encoding, checked on a
/// named constant before accepting it in place of the record literal.
fn is_ammo_override_shape(top: &[Literal]) -> bool {
    matches!(
        top,
        [Literal::Bool(_), Literal::Array(resources)]
            if resources.iter().all(|r| matches!(
                r,
                Literal::Array(pair) if matches!(pair.as_slice(), [Literal::Int(_), Literal::Int(_)])
            ))
    )
}

/// Fold an `ammoOverride` argument (a record literal `{ overrideStartingAmmo:
/// bool, resources: [{ loaded: int, reserve: int }] }`, or a constant bound to
/// its encoding) for a gate's `WeaponAmmoOverride` nested-struct data field.
/// Encoded as `Array[Bool(overrideStartingAmmo), Array[Array[Int(loaded),
/// Int(reserve)], ...]]`, the exact shape the emitter decodes. `None` on any
/// non-constant value, unexpected field or mismatched constant.
pub(crate) fn fold_ammo_override<C: Constants + ?Sized>(e: &Expr, env: &C) -> Option<Literal> {
    if let Expr::Ident { name, .. } = e {
        return match env.constant(name) {
            Some(lit @ Literal::Array(top)) if is_ammo_override_shape(top) => Some(lit.clone()),
            _ => None,
        };
    }
    let Expr::RecordLit { fields, .. } = e else {
        return None;
    };
    let mut override_starting = None;
    let mut resources: Option<Vec<Literal>> = None;
    for f in fields {
        let RecordLitField::Named { name, value, .. } = f else {
            return None;
        };
        match name.as_str() {
            "overrideStartingAmmo" => {
                let Literal::Bool(b) = expr_to_literal(value)? else {
                    return None;
                };
                override_starting = Some(b);
            }
            "resources" => {
                let Expr::Array { elements, .. } = value else {
                    return None;
                };
                let mut rs = Vec::with_capacity(elements.len());
                for el in elements {
                    let ArrayElem::Item(item) = el else {
                        return None;
                    };
                    rs.push(fold_resource_amount(item)?);
                }
                resources = Some(rs);
            }
            _ => return None,
        }
    }
    // Both fields are required: a missing one is rejected (the caller reports
    // WS028) rather than defaulted. An explicit `resources: []` is accepted.
    Some(Literal::Array(vec![
        Literal::Bool(override_starting?),
        Literal::Array(resources?),
    ]))
}

/// One `{ loaded, reserve }` resource of `ammoOverride.resources`, folded to
/// `Array[Int(loaded), Int(reserve)]` (see [`fold_ammo_override`]).
fn fold_resource_amount(e: &Expr) -> Option<Literal> {
    let Expr::RecordLit { fields, .. } = e else {
        return None;
    };
    let mut loaded = None;
    let mut reserve = None;
    for f in fields {
        let RecordLitField::Named { name, value, .. } = f else {
            return None;
        };
        let Literal::Int(n) = expr_to_literal(value)? else {
            return None;
        };
        match name.as_str() {
            "loaded" => loaded = Some(n),
            "reserve" => reserve = Some(n),
            _ => return None,
        }
    }
    // Both fields are required: a missing `loaded`/`reserve` is rejected
    // (WS028) rather than baked as 0.
    Some(Literal::Array(vec![
        Literal::Int(loaded?),
        Literal::Int(reserve?),
    ]))
}

/// Whether a container of `elem` holds refs: `*T`, arrays and maps (an array
/// or map value is its own ref).
pub(crate) fn holds_refs(elem: &Type) -> bool {
    matches!(elem, Type::Ref(_) | Type::Array(_) | Type::Map(..))
}

/// Whether initializer element `e` is written as a ref to storage: `&x`, or a
/// bare name (an array or map names its own ref). Lowering resolves it with
/// `storage_ref_target` in lowering; the type checker lets it through for a container
/// that [`holds_refs`].
pub(crate) fn ref_shaped(e: &Expr) -> bool {
    let operand = match e {
        Expr::RefOf { operand, .. } => operand.as_ref(),
        other => other,
    };
    matches!(
        operand,
        Expr::Ident { .. } | Expr::FieldAccess { .. } | Expr::TuplePick { .. }
    )
}
