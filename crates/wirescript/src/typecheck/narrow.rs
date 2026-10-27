//! Narrowing an `any` by what its kind test found.
//!
//! In the `Int` arm of `match x.getType() { ... }`, and in the block of
//! `if x.getType() is Type.Int { ... }`, `x` is an `int`. An `any` already
//! wires into a typed pin as it is, so this only changes the types typecheck
//! gives reads of `x` there: operators, calls and hover see the kind the test
//! found, and lowering emits the same gates.
//!
//! `x` is a place: a name, or fields read off one (`p.Value`). A narrowing
//! holds the binding the name had at the test, so a later `let` of the same
//! name is not narrowed.

use super::TypeCheckCtx;
use crate::ast::{CallArg, Expr};
use crate::diagnostic::SourceRange;
use crate::ir::Type;

pub(crate) struct Narrowing {
    root: String,
    decl: SourceRange,
    fields: Vec<String>,
    ty: Type,
    /// The test that narrowed it, as hover names it.
    by: String,
}

/// A read of an `any` that a kind test narrowed: the type it has there, and
/// the test, as hover names it (``the `Int` arm of `match x.getType()` ``).
#[derive(Debug, Clone, PartialEq)]
pub struct NarrowedRead {
    pub ty: Type,
    pub by: String,
}

/// The name `e` reads and the fields it reads off it, outermost first.
fn place(e: &Expr) -> Option<(&str, Vec<&str>)> {
    match e {
        Expr::Ident { name, .. } => Some((name, Vec::new())),
        Expr::FieldAccess { obj, field, .. } => {
            let (root, mut fields) = place(obj)?;
            fields.push(field);
            Some((root, fields))
        }
        _ => None,
    }
}

/// The value whose kind `test` asks for: `x` in `x.getType()` or
/// `GetValueType(x)`.
pub(crate) fn kind_test_subject(test: &Expr) -> Option<&Expr> {
    let Expr::Call { callee, args, .. } = test else {
        return None;
    };
    match (callee.as_ref(), args.as_slice()) {
        (Expr::FieldAccess { obj, field, .. }, []) if field == "getType" => Some(obj),
        (Expr::Ident { name, .. }, [CallArg::Positional(x)]) if name == "GetValueType" => Some(x),
        _ => None,
    }
}

/// The type a `Type` member names. `None` for the kinds with no one
/// Wirescript type (`Enum`, `Component`, `Exec`).
pub(crate) fn member_type(member: &str) -> Option<Type> {
    Some(match member {
        "Bool" => Type::Bool,
        "Int" => Type::Int,
        "Float" => Type::Float,
        "Color" => Type::Color,
        "Vector" => Type::Vector,
        "Rotator" => Type::Rotator,
        "Quat" => Type::Quat,
        "String" => Type::String,
        "Object" => Type::Entity,
        "Brick" => Type::Brick,
        _ => return None,
    })
}

/// The place `test` asks about, as written: `p.Value` in `p.Value.getType()`.
pub(crate) fn test_subject_text(test: &Expr) -> Option<String> {
    let (root, fields) = place(kind_test_subject(test)?)?;
    Some(
        std::iter::once(root)
            .chain(fields)
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// `test` as written, from its subject: `p.Value.getType()` or
/// `GetValueType(p.Value)`.
pub(crate) fn describe_test(test: &Expr) -> Option<String> {
    let subject = test_subject_text(test)?;
    Some(match test {
        Expr::Call { callee, .. } if matches!(callee.as_ref(), Expr::FieldAccess { .. }) => {
            format!("{subject}.getType()")
        }
        _ => format!("GetValueType({subject})"),
    })
}

/// Narrow for the arm `member` of `match test { ... }`, where `test` is a
/// kind test. Gives whether it did, for [`pop`].
pub(crate) fn push_arm(ctx: &mut TypeCheckCtx<'_>, test: &Expr, member: &str) -> bool {
    let (Some(subject), Some(written)) = (kind_test_subject(test), describe_test(test)) else {
        return false;
    };
    push(
        ctx,
        subject,
        member,
        format!("the `{member}` arm of `match {written}`"),
    )
}

/// Narrow `subject` to the type `member` names, for the arm or block about
/// to be checked. Gives whether it did, so the caller pops only its own.
fn push(ctx: &mut TypeCheckCtx<'_>, subject: &Expr, member: &str, by: String) -> bool {
    let (Some((root, fields)), Some(ty)) = (place(subject), member_type(member)) else {
        return false;
    };
    let Some(decl) = ctx.scope.lookup(root).map(|s| s.decl_range.clone()) else {
        return false;
    };
    ctx.narrowings.push(Narrowing {
        root: root.to_string(),
        decl,
        fields: fields.into_iter().map(str::to_string).collect(),
        ty,
        by,
    });
    true
}

pub(crate) fn pop(ctx: &mut TypeCheckCtx<'_>, pushed: bool) {
    if pushed {
        ctx.narrowings.pop();
    }
}

/// What a narrowing gives `e`, a read of a narrowed place, if any.
pub(crate) fn narrowed(ctx: &TypeCheckCtx<'_>, e: &Expr) -> Option<NarrowedRead> {
    let (root, fields) = place(e)?;
    let n = ctx.narrowings.iter().rev().find(|n| {
        n.root == root
            && n.fields
                .iter()
                .map(String::as_str)
                .eq(fields.iter().copied())
    })?;
    let decl = &ctx.scope.lookup(root)?.decl_range;
    (*decl == n.decl).then(|| NarrowedRead {
        ty: n.ty.clone(),
        by: n.by.clone(),
    })
}

/// Narrow for the block an `if` runs when `cond` holds, when `cond` is
/// `x.getType() is Type.X`. Gives whether it did, for [`pop`].
pub(crate) fn push_if(ctx: &mut TypeCheckCtx<'_>, cond: &Expr) -> bool {
    let Expr::Is { value, path, .. } = cond else {
        return false;
    };
    match (
        kind_test_subject(value),
        type_member(path),
        describe_test(value),
    ) {
        (Some(x), Some(member), Some(written)) => {
            push(ctx, x, member, format!("`if {written} is Type.{member}`"))
        }
        _ => false,
    }
}

/// The `Type` member `path` names, written `Type.Int` as `is` requires.
pub(crate) fn type_member(path: &Expr) -> Option<&str> {
    match path {
        Expr::FieldAccess { obj, field, .. } if matches!(obj.as_ref(), Expr::Ident { name, .. } if name == "Type") => {
            Some(field)
        }
        _ => None,
    }
}
