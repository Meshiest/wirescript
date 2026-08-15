//! `@label(expr)` checking (WS040).

use super::*;

// ---------- `@label(expr)` constant-folding check (WS040) ----------

/// Walk every top-level decl, recursing into chip/anon-chip bodies (and the
/// blocks nested inside them — `if`/`on`), and flag any `@label(expr)` whose
/// expression doesn't fold to a compile-time constant.
pub(super) fn check_label_exprs(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    for d in decls {
        check_label_expr_decl(ctx, d);
    }
}

fn check_label_expr_decl(ctx: &mut TypeCheckCtx, d: &TopDecl) {
    match d {
        // A TOP-LEVEL `var` accepts a runtime `@label(expr)` — it becomes a
        // dynamic label wired into the text component's `Text` port at emit
        // (`lower::resolve_dynamic_var_labels`). So no WS040 here; the
        // expression is instead type-checked in `check_dynamic_var_labels`
        // after every top-level symbol is in scope. (A CONSTANT label still
        // bakes statically — that path folds it and never reaches the wire.)
        TopDecl::Var(_) => {}
        TopDecl::In(i) => check_one_label_expr(ctx, &i.label_expr),
        TopDecl::Out(o) => check_one_label_expr(ctx, &o.label_expr),
        TopDecl::Chip(c) => {
            check_one_label_expr(ctx, &c.label_expr);
            check_label_exprs_in_block(ctx, &c.body);
        }
        TopDecl::AnonChip(ac) => {
            check_one_label_expr(ctx, &ac.label_expr);
            check_label_exprs_in_block(ctx, &ac.body);
        }
        // A decl can also live inside a top-level `on Event { ... }` handler
        // (the standard Wirescript pattern) or a top-level `if` — recurse into
        // those blocks too, or a non-constant `@label` there would silently
        // fall back to the name instead of erroring.
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
fn check_label_exprs_in_block(ctx: &mut TypeCheckCtx, block: &Block) {
    for s in &block.stmts {
        check_label_expr_stmt(ctx, s);
    }
}

fn check_label_expr_stmt(ctx: &mut TypeCheckCtx, s: &Stmt) {
    match s {
        Stmt::Var(v) => check_one_label_expr(ctx, &v.label_expr),
        Stmt::In(i) => check_one_label_expr(ctx, &i.label_expr),
        Stmt::OutBinding(o) => check_one_label_expr(ctx, &o.label_expr),
        Stmt::ChipDecl(c) => {
            check_one_label_expr(ctx, &c.label_expr);
            check_label_exprs_in_block(ctx, &c.body);
        }
        Stmt::AnonChip(ac) => {
            check_one_label_expr(ctx, &ac.label_expr);
            check_label_exprs_in_block(ctx, &ac.body);
        }
        Stmt::If(i) => {
            check_label_exprs_in_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_label_exprs_in_block(ctx, else_b);
            }
        }
        Stmt::Handler(h) => check_label_exprs_in_block(ctx, &h.body),
        _ => {}
    }
}

fn check_one_label_expr(ctx: &mut TypeCheckCtx, label_expr: &Option<Expr>) {
    let Some(expr) = label_expr else { return };
    if crate::lower::expr_to_literal_in(expr, &ctx.const_env).is_some() {
        return;
    }
    ctx.emit(
        "WS040",
        "`@label` expression must be a compile-time constant (a literal or a constant `let`); \
         a runtime value cannot be baked as a label",
        expr.range().clone(),
    );
}

/// Type-check the runtime `@label(expr)` on each top-level `var` so an undefined
/// symbol or a bad type surfaces (the lowering pass then wires the value into
/// the label component's `Text` port). Only the non-constant labels reach here —
/// a constant one bakes its text statically and needs no wire. Runs after the
/// main decl loop so every top-level symbol is already declared.
pub(super) fn check_dynamic_var_labels(
    ctx: &mut TypeCheckCtx,
    script: &Script,
) {
    for d in &script.decls {
        if let TopDecl::Var(v) = d {
            if let Some(le) = &v.label_expr {
                if crate::lower::expr_to_literal_in(le, &ctx.const_env).is_none() {
                    infer::infer(ctx, le);
                }
            }
        }
    }
}
