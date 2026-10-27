//! `@label(expr)` checking (WS040).

use super::*;

// ---------- `@label(expr)` constant-folding check (WS040) ----------

/// Walk every top-level declaration, into chip and anonymous chip bodies and
/// the `if` and `on` blocks inside them, and flag any `@label(expr)` whose
/// expression does not fold to a constant.
pub(super) fn check_label_exprs(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    for d in decls {
        check_label_expr_decl(ctx, d);
    }
}

fn check_label_expr_decl(ctx: &mut TypeCheckCtx<'_>, d: &TopDecl) {
    match d {
        // A top-level `var` accepts a runtime `@label(expr)`: it becomes a dynamic
        // label wired into the text component's `Text` port
        // (`lower::resolve_dynamic_var_labels`), so it is not WS040 and is type-checked
        // in `check_dynamic_var_labels` once every top-level symbol is in scope. A
        // constant label still bakes statically.
        TopDecl::Var(_) => {}
        TopDecl::In(i) => check_one_label_expr(ctx, i.label_expr.as_ref()),
        TopDecl::Out(o) => check_one_label_expr(ctx, o.label_expr.as_ref()),
        TopDecl::Chip(c) => {
            check_one_label_expr(ctx, c.label_expr.as_ref());
            check_label_exprs_in_block(ctx, &c.body);
        }
        TopDecl::AnonChip(ac) => {
            check_one_label_expr(ctx, ac.label_expr.as_ref());
            check_label_exprs_in_block(ctx, &ac.body);
        }
        // A declaration can also sit in a top-level `on Event { ... }` handler or a
        // top-level `if`, so those blocks are walked too; a runtime `@label` there
        // would otherwise fall back to the name silently.
        TopDecl::Handler(h) => check_label_exprs_in_block(ctx, &h.body),
        TopDecl::If(i) => {
            check_label_exprs_in_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_label_exprs_in_block(ctx, else_b);
            }
        }
        _ => {}
    }
}

/// Visit every statement in `block`, recursing into nested chip/handler/if
/// bodies. Shared by the decl-level and statement-level walks so the two
/// stay in step.
fn check_label_exprs_in_block(ctx: &mut TypeCheckCtx<'_>, block: &Block) {
    for s in &block.stmts {
        check_label_expr_stmt(ctx, s);
    }
}

fn check_label_expr_stmt(ctx: &mut TypeCheckCtx<'_>, s: &Stmt) {
    match s {
        Stmt::Var(v) => check_one_label_expr(ctx, v.label_expr.as_ref()),
        Stmt::In(i) => check_one_label_expr(ctx, i.label_expr.as_ref()),
        Stmt::OutBinding(o) => check_one_label_expr(ctx, o.label_expr.as_ref()),
        Stmt::ChipDecl(c) => {
            check_one_label_expr(ctx, c.label_expr.as_ref());
            check_label_exprs_in_block(ctx, &c.body);
        }
        Stmt::AnonChip(ac) => {
            check_one_label_expr(ctx, ac.label_expr.as_ref());
            check_label_exprs_in_block(ctx, &ac.body);
        }
        Stmt::If(i) => {
            check_label_exprs_in_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_label_exprs_in_block(ctx, else_b);
            }
        }
        Stmt::Handler(h) => check_label_exprs_in_block(ctx, &h.body),
        Stmt::For(f) => {
            check_one_label_expr(ctx, f.label_expr.as_ref());
            check_label_exprs_in_block(ctx, &f.body);
        }
        _ => {}
    }
}

fn check_one_label_expr(ctx: &mut TypeCheckCtx<'_>, label_expr: Option<&Expr>) {
    let Some(expr) = label_expr else { return };
    if crate::const_eval::expr_to_literal_in(expr, &ctx.const_lookup()).is_some() {
        return;
    }
    ctx.emit(
        "WS040",
        "`@label` expression must be a compile-time constant (a literal or a constant `let`); \
         a runtime value cannot be baked as a label",
        expr.range().clone(),
    );
}

/// Type-check the runtime `@label(expr)` of each top-level `var`, so an undefined
/// symbol or a bad type is reported (lowering then wires the value into the
/// label's `Text` port). Constant labels bake statically and do not come here.
/// Runs after the declaration loop, so every top-level symbol is declared.
pub(super) fn check_dynamic_var_labels(ctx: &mut TypeCheckCtx<'_>, script: &Script) {
    for d in &script.decls {
        if let TopDecl::Var(v) = d
            && let Some(le) = &v.label_expr
        {
            let view = ctx.top_view(crate::scope::top::ReadAt::code(&v.range));
            if crate::const_eval::expr_to_literal_in(le, &ctx.const_lookup()).is_none() {
                infer::infer(ctx, le);
            }
            ctx.restore_top_view(view);
        }
    }
}
