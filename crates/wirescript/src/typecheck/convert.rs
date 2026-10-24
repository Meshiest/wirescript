//! `value as T`: which conversions the `Convert` gate can make, and the type a
//! conversion gives. Lowering reads the same rule (`conversion`), so a field
//! typecheck accepts is the field lowering converts.

use super::*;
use crate::types::mono::unwrap_ref;

/// `bool`, `int`, `float` and `string`, which the Convert gate turns into one
/// another.
fn basic(t: &Type) -> bool {
    matches!(t, Type::Bool | Type::Int | Type::Float | Type::String)
}

/// Whether the Convert gate turns a `src` into a `dst`, from what
/// `projects/as-probes/` read in game. A pair the gate does not convert comes
/// out as the target type's default (a vector as an int is 0, a string as a
/// vector is all zeros, a vector as a color is white, a live character as a
/// bool is false), so only a measured pair is allowed.
fn scalar_converts(src: &Type, dst: &Type) -> bool {
    match (src, dst) {
        (s, d) if basic(s) && basic(d) => true,
        // A number fills every channel: `2.5` is `X=2.5 Y=2.5 Z=2.5`, and as a
        // color `0.5` is `BCBCBC80` (each channel clamped to 0..1).
        (Type::Int | Type::Float, Type::Vector | Type::Rotator | Type::Color) => true,
        (Type::Rotator, Type::Quat) | (Type::Quat, Type::Rotator) => true,
        // `X=1.500 Y=-2.000 Z=3.000`, `P=10.000000 ...`, `FFBC00FF`, `X=0.000000000 ... W=...`.
        (Type::Vector | Type::Rotator | Type::Quat | Type::Color, Type::String) => true,
        _ => false,
    }
}

/// `lit as dst` worked out at compile time, for the pairs whose in-game
/// result is measured: a float truncates toward zero, a number is `true` when
/// it is not zero, and a plain decimal string reads as its number. `None` for
/// any other pair, which keeps its Convert gate: a float as a string is
/// formatted by the game (`1234.5678` reads `1234.57`), and a string the game
/// reads by its leading number (`"1e3"` as an int is 1) folds only when the
/// whole string is that number.
pub(crate) fn fold_as(lit: &crate::ir::Literal, dst: &Type) -> Option<crate::ir::Literal> {
    use crate::ir::Literal as L;
    let truncate = |f: f64| f.is_finite().then(|| f.trunc() as i64);
    let decimal = |s: &str| {
        let digits = s.strip_prefix('-').unwrap_or(s);
        let (whole, frac) = digits.split_once('.').unwrap_or((digits, "1"));
        let all = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        (all(whole) && all(frac))
            .then(|| s.parse::<f64>().ok())
            .flatten()
    };
    Some(match (lit, dst) {
        (L::Int(_), Type::Int)
        | (L::Float(_), Type::Float)
        | (L::Bool(_), Type::Bool)
        | (L::String(_), Type::String) => lit.clone(),
        (L::Int(n), Type::Float) => L::Float(*n as f64),
        (L::Int(n), Type::Bool) => L::Bool(*n != 0),
        (L::Int(n), Type::String) => L::String(n.to_string()),
        (L::Float(f), Type::Int) => L::Int(truncate(*f)?),
        (L::Float(f), Type::Bool) => L::Bool(*f != 0.0),
        (L::Bool(b), Type::Int) => L::Int(*b as i64),
        (L::Bool(b), Type::Float) => L::Float(if *b { 1.0 } else { 0.0 }),
        (L::Bool(b), Type::String) => L::String(b.to_string()),
        (L::String(s), Type::Int) => L::Int(truncate(decimal(s)?)?),
        (L::String(s), Type::Float) => L::Float(decimal(s)?),
        _ => return None,
    })
}

fn object(t: &Type) -> bool {
    matches!(t, Type::Entity | Type::Character | Type::Controller)
}

/// The fields of a record or tuple type, a tuple's by index.
fn aggregate_fields(t: &Type) -> Option<Vec<(String, Type)>> {
    match t {
        Type::Record(fs) => Some(fs.clone()),
        Type::Tuple(ts) => Some(
            ts.iter()
                .enumerate()
                .map(|(i, t)| (i.to_string(), t.clone()))
                .collect(),
        ),
        _ => None,
    }
}

/// The type `value as dst` gives when `value` is a `src`, or why `as` cannot
/// convert it. A record or tuple converts field by field and needs the same
/// fields on both sides; a field already of its target type stays as it is.
pub(crate) fn conversion(src: &Type, dst: &Type) -> Result<Type, String> {
    let src = unwrap_ref(src);
    if src == *dst {
        return Ok(dst.clone());
    }
    let shown = |t: &Type| crate::analysis::types::type_str(t);
    match (&src, dst) {
        // An earlier error, nothing to add.
        (Type::Any, _) => Ok(dst.clone()),
        // An untyped wire may already carry a `dst`, and the gate converts it
        // by what arrives.
        (Type::Opaque, d)
            if basic(d)
                || object(d)
                || matches!(d, Type::Vector | Type::Rotator | Type::Quat | Type::Color) =>
        {
            Ok(dst.clone())
        }
        (s, d) if scalar_converts(s, d) => Ok(dst.clone()),
        (s, d) if object(s) || object(d) => Err(format!(
            "the game does not convert {} to {} (a live character as a bool came out false)",
            shown(s),
            shown(d)
        )),
        (s, d) if aggregate_fields(s).is_none() && aggregate_fields(d).is_none() => Err(format!(
            "the game gives a default {} for a {} rather than converting it",
            shown(d),
            shown(s)
        )),
        (s, d) if aggregate_fields(s).is_some() && aggregate_fields(d).is_some() => {
            // A tuple's elements are its fields "0", "1", ...: a tuple literal
            // and a tuple type meet whichever of the two forms each is in.
            let (ss, ds) = (aggregate_fields(s).unwrap(), aggregate_fields(d).unwrap());
            let missing: Vec<&str> = ds
                .iter()
                .filter(|(n, _)| !ss.iter().any(|(m, _)| m == n))
                .chain(ss.iter().filter(|(n, _)| !ds.iter().any(|(m, _)| m == n)))
                .map(|(n, _)| n.as_str())
                .collect();
            if !missing.is_empty() {
                return Err(format!("the fields differ (`{}`)", missing.join("`, `")));
            }
            let mut converted = Vec::with_capacity(ds.len());
            for (name, d) in &ds {
                let s = &ss.iter().find(|(n, _)| n == name).expect("same fields").1;
                converted.push((
                    name.clone(),
                    conversion(s, d).map_err(|why| format!("field `{name}`: {why}"))?,
                ));
            }
            Ok(match dst {
                Type::Tuple(_) => Type::Tuple(converted.into_iter().map(|(_, t)| t).collect()),
                _ => Type::Record(converted),
            })
        }
        (s, d) => Err(format!("no conversion from {} to {}", shown(s), shown(d))),
    }
}

/// Type `value as ty`. A conversion `as` cannot make yet is WS109 (a warning)
/// and leaves the value its own type, so what follows is checked against what
/// the value really is.
pub(super) fn infer_as(
    ctx: &mut TypeCheckCtx,
    value: &Expr,
    ty: &TypeExpr,
    range: &SourceRange,
) -> Type {
    let src = unwrap_ref(&infer::infer(ctx, value));
    let dst = resolve::resolve_type_expr(ctx, ty);
    match conversion(&src, &dst) {
        Ok(t) => t,
        Err(why) => {
            ctx.warn(
                "WS109",
                format!(
                    "`as` cannot convert {} to {} yet: {why}; the value stays {}",
                    crate::analysis::types::type_str(&src),
                    crate::analysis::types::type_str(&dst),
                    crate::analysis::types::type_str(&src),
                ),
                range.clone(),
            );
            src
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_objects_and_shapes() {
        let ok = |s: Type, d: Type| conversion(&s, &d).is_ok();
        assert!(ok(Type::Int, Type::Bool));
        assert!(ok(Type::String, Type::Float));
        // Measured to come out as the target's default.
        assert!(!ok(Type::String, Type::Vector));
        assert!(ok(Type::Int, Type::Color));
        assert!(ok(Type::Float, Type::Rotator));
        assert!(ok(Type::Rotator, Type::Quat));
        assert!(ok(Type::Quat, Type::String));
        assert!(!ok(Type::Vector, Type::Rotator));
        assert!(!ok(Type::Vector, Type::Color));
        assert!(!ok(Type::Character, Type::Controller));
        assert!(!ok(Type::Entity, Type::Bool));
        assert!(!ok(Type::Bool, Type::Entity));
        assert!(!ok(
            Type::Array(Box::new(Type::Int)),
            Type::Array(Box::new(Type::Float))
        ));
        assert!(!ok(Type::Brick, Type::String));
        assert!(!ok(Type::Vector, Type::Float));
        assert!(ok(Type::Vector, Type::String));
        assert!(ok(Type::Int, Type::Vector));
        let t = |v: Vec<Type>| Type::Tuple(v);
        assert_eq!(
            conversion(
                &t(vec![Type::Int, Type::Int]),
                &t(vec![Type::Bool, Type::Int])
            ),
            Ok(t(vec![Type::Bool, Type::Int]))
        );
        assert!(conversion(&t(vec![Type::Int]), &t(vec![Type::Bool, Type::Int])).is_err());
        let r = |v: Vec<(&str, Type)>| {
            Type::Record(v.into_iter().map(|(n, t)| (n.to_string(), t)).collect())
        };
        assert!(
            conversion(
                &r(vec![("x", Type::Int), ("y", Type::Int)]),
                &r(vec![("y", Type::Float), ("x", Type::Float)])
            )
            .is_ok()
        );
        assert!(conversion(&r(vec![("x", Type::Int)]), &r(vec![("y", Type::Int)])).is_err());
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use crate::ir::Literal as L;

    /// The values `projects/as-probes/as_convert.ws` read in game.
    #[test]
    fn folds_what_the_game_measured() {
        let f = |l: L, t: Type| fold_as(&l, &t);
        assert_eq!(f(L::Float(2.7), Type::Int), Some(L::Int(2)));
        assert_eq!(f(L::Float(-2.7), Type::Int), Some(L::Int(-2)));
        assert_eq!(f(L::Float(-2.5), Type::Int), Some(L::Int(-2)));
        assert_eq!(f(L::String("2.7".into()), Type::Int), Some(L::Int(2)));
        assert_eq!(f(L::String("2.5".into()), Type::Float), Some(L::Float(2.5)));
        assert_eq!(f(L::Int(42), Type::String), Some(L::String("42".into())));
        assert_eq!(f(L::Bool(true), Type::Int), Some(L::Int(1)));
        assert_eq!(f(L::Int(0), Type::Bool), Some(L::Bool(false)));
        assert_eq!(f(L::Int(7), Type::Float), Some(L::Float(7.0)));
        assert_eq!(f(L::Float(0.5), Type::Bool), Some(L::Bool(true)));
        assert_eq!(f(L::Bool(true), Type::Float), Some(L::Float(1.0)));
        // The game formats, or reads only a leading number: left to the gate.
        assert_eq!(f(L::Float(2.5), Type::String), None);
        assert_eq!(f(L::String("hi".into()), Type::Int), None);
        assert_eq!(f(L::String("1e3".into()), Type::Int), None);
        assert_eq!(f(L::String("  42".into()), Type::Int), None);
        assert_eq!(
            f(L::Bool(false), Type::String),
            Some(L::String("false".into()))
        );
    }
}
