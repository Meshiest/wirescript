//! `@color` / `@material` checking (WS003, WS040).

use super::*;
use crate::types::mono::unwrap_ref;

/// Which annotation a value belongs to.
#[derive(Clone, Copy)]
enum Style {
    Color,
    Material,
}

impl Style {
    fn word(self) -> &'static str {
        match self {
            Style::Color => "@color",
            Style::Material => "@material",
        }
    }

    fn want(self) -> Type {
        match self {
            Style::Color => Type::Color,
            Style::Material => Type::Enum { name: crate::lower::MATERIAL_ENUM.to_string(), args: vec![] },
        }
    }
}

/// Check every `@color` / `@material` in the program: the value must be a
/// `color` / `BrickMaterial`, and outside the module and a top-level `var` (the
/// only places a runtime value has a brick to wire into, as for `@label`) a
/// compile-time constant. Runs after the main pass so a runtime value may name
/// any top-level declaration, including one below it.
pub(super) fn check_style_exprs(ctx: &mut TypeCheckCtx, script: &Script) {
    check_pair(ctx, script.module_color.as_ref(), script.module_material.as_ref(), true);
    for d in &script.decls {
        match d {
            TopDecl::Var(v) => {
                let view = ctx.top_view(crate::scope::top::ReadAt::code(&v.range));
                check_pair(ctx, v.color_expr.as_ref(), v.material_expr.as_ref(), true);
                ctx.restore_top_view(view);
            }
            TopDecl::Chip(c) => {
                check_pair(ctx, c.color_expr.as_ref(), c.material_expr.as_ref(), false);
                check_block(ctx, &c.body);
            }
            TopDecl::AnonChip(ac) => {
                check_pair(ctx, ac.color_expr.as_ref(), ac.material_expr.as_ref(), false);
                check_block(ctx, &ac.body);
            }
            TopDecl::Handler(h) => check_block(ctx, &h.body),
            TopDecl::If(i) => {
                check_block(ctx, &i.then_block);
                if let Some(else_b) = &i.else_block {
                    check_block(ctx, else_b);
                }
            }
            _ => {}
        }
    }
}

/// The nested declarations in `block`, where every value must be constant.
fn check_block(ctx: &mut TypeCheckCtx, block: &Block) {
    for s in &block.stmts {
        match s {
            Stmt::Var(v) => check_pair(ctx, v.color_expr.as_ref(), v.material_expr.as_ref(), false),
            Stmt::ChipDecl(c) => {
                check_pair(ctx, c.color_expr.as_ref(), c.material_expr.as_ref(), false);
                check_block(ctx, &c.body);
            }
            Stmt::AnonChip(ac) => {
                check_pair(ctx, ac.color_expr.as_ref(), ac.material_expr.as_ref(), false);
                check_block(ctx, &ac.body);
            }
            Stmt::If(i) => {
                check_block(ctx, &i.then_block);
                if let Some(else_b) = &i.else_block {
                    check_block(ctx, else_b);
                }
            }
            Stmt::Handler(h) => check_block(ctx, &h.body),
            Stmt::For(f) => check_block(ctx, &f.body),
            _ => {}
        }
    }
}

fn check_pair(ctx: &mut TypeCheckCtx, color: Option<&Expr>, material: Option<&Expr>, runtime_ok: bool) {
    if let Some(e) = color {
        check_one(ctx, Style::Color, e, runtime_ok);
    }
    if let Some(e) = material {
        check_one(ctx, Style::Material, e, runtime_ok);
    }
}

fn check_one(ctx: &mut TypeCheckCtx, style: Style, e: &Expr, runtime_ok: bool) {
    let arg = match style {
        Style::Color => std::borrow::Cow::Borrowed(e),
        Style::Material => crate::lower::material_arg(e),
    };
    let e = &*arg;
    let constant = crate::lower::style_constant(e, &ctx.const_ctx(None)).is_some();
    if !constant && !runtime_ok {
        ctx.emit(
            "WS040",
            format!(
                "`{}` value must be a compile-time constant here (a literal or a constant `let`); \
                 only the module and a top-level `var` take a runtime value",
                style.word()
            ),
            e.range().clone(),
        );
        return;
    }
    let got = infer::infer(ctx, e);
    let want = style.want();
    if coerce(&unwrap_ref(&got), &want) == CoerceRule::Mismatch {
        ctx.emit(
            "WS003",
            format!(
                "`{}` takes a {}, got {}",
                style.word(),
                crate::analysis::types::type_str(&want),
                crate::analysis::types::type_str(&got),
            ),
            e.range().clone(),
        );
    }
}
