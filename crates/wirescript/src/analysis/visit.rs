//! Shared full-AST visitor used by both the WS030 custom-event type check
//! (`typecheck::check_custom_event_types`) and custom-event go-to-definition
//! (`analysis::definition::custom_event_send_definition`). Both passes need
//! the same two things out of a walk over the whole program: every `Handler`
//! (to find `on CustomEvent(...)` receivers) and every `Expr` that is a call
//! (to find `SendCustomEvent(...)` sends). Collapsing the two structurally
//! identical walkers here means a fix to the walk (e.g. a missed AST arm)
//! only has to happen once.

use crate::ast::*;

/// Walk every `Handler` and every `Expr` (including call expressions) in the
/// program. `on_handler` fires for each handler (top-level, in chips/anon-chips,
/// in namespaces, in nested blocks, AND in a captured-event body); `on_call`
/// fires for each `Expr` node so callers can match the ones they care about
/// (e.g. `SendCustomEvent(...)` calls). Lifetimes tie the visited refs to the
/// script so callers may collect `&'a Handler` / `&'a Expr`.
pub fn visit_program<'a>(
    script: &'a Script,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    for d in &script.decls {
        visit_decl(d, on_handler, on_call);
    }
}

fn visit_decl<'a>(
    d: &'a TopDecl,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    match d {
        TopDecl::Handler(h) => {
            on_handler(h);
            visit_block(&h.body, on_handler, on_call);
        }
        TopDecl::Chip(c) => visit_block(&c.body, on_handler, on_call),
        TopDecl::AnonChip(ac) => visit_block(&ac.body, on_handler, on_call),
        TopDecl::Namespace(ns) => {
            for d in &ns.decls {
                visit_decl(d, on_handler, on_call);
            }
        }
        TopDecl::Event(e) => {
            // A captured event's body (`let h = on go { … }`) can host a
            // `SendCustomEvent`; a receiver `on CustomEvent(...)` can't
            // appear there, but walk it uniformly.
            if let Some(b) = &e.captured_body {
                visit_block(b, on_handler, on_call);
            }
        }
        TopDecl::Fn(f) => visit_expr(&f.body, on_call),
        TopDecl::Let(l) => visit_expr(&l.value, on_call),
        TopDecl::Var(v) => {
            if let Some(e) = &v.init {
                visit_expr(e, on_call);
            }
        }
        TopDecl::Buffer(b) => visit_expr(&b.init, on_call),
        TopDecl::Out(o) => {
            if let Some(e) = &o.value {
                visit_expr(e, on_call);
            }
        }
        TopDecl::Assign(a) => {
            visit_expr(&a.target, on_call);
            visit_expr(&a.value, on_call);
        }
        TopDecl::If(i) => visit_if(i, on_handler, on_call),
        TopDecl::ExprStmt(es) => visit_expr(&es.expr, on_call),
        _ => {}
    }
}

fn visit_block<'a>(
    b: &'a Block,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    for s in &b.stmts {
        visit_stmt(s, on_handler, on_call);
    }
}

fn visit_stmt<'a>(
    s: &'a Stmt,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    match s {
        Stmt::Handler(h) => {
            on_handler(h);
            visit_block(&h.body, on_handler, on_call);
        }
        Stmt::AnonChip(ac) => visit_block(&ac.body, on_handler, on_call),
        Stmt::ChipDecl(c) => visit_block(&c.body, on_handler, on_call),
        Stmt::If(i) => visit_if(i, on_handler, on_call),
        Stmt::Let(l) => visit_expr(&l.value, on_call),
        Stmt::Assign(a) => {
            visit_expr(&a.target, on_call);
            visit_expr(&a.value, on_call);
        }
        Stmt::ExprStmt(es) => visit_expr(&es.expr, on_call),
        Stmt::Var(v) => {
            if let Some(e) = &v.init {
                visit_expr(e, on_call);
            }
        }
        Stmt::Buffer(b) => visit_expr(&b.init, on_call),
        Stmt::OutBinding(ob) => {
            if let Some(e) = &ob.value {
                visit_expr(e, on_call);
            }
        }
        Stmt::Emit(e) => {
            if let Some(v) = &e.value {
                visit_expr(v, on_call);
            }
        }
        Stmt::Await(a) => {
            if let Some(e) = &a.value_expr {
                visit_expr(e, on_call);
            }
            visit_expr(&a.exec_expr, on_call);
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                visit_expr(e, on_call);
            }
        }
        _ => {}
    }
}

fn visit_if<'a>(
    i: &'a If,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    visit_expr(&i.cond, on_call);
    visit_block(&i.then_block, on_handler, on_call);
    if let Some(eb) = &i.else_block {
        visit_block(eb, on_handler, on_call);
    }
}

fn visit_expr<'a>(e: &'a Expr, on_call: &mut dyn FnMut(&'a Expr)) {
    match e {
        Expr::Call { callee, args, .. } => {
            on_call(e);
            visit_expr(callee, on_call);
            for a in args {
                match a {
                    CallArg::Positional(x) | CallArg::Spread(x) => visit_expr(x, on_call),
                    CallArg::Named { value, .. } => visit_expr(value, on_call),
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            visit_expr(left, on_call);
            visit_expr(right, on_call);
        }
        Expr::UnOp { operand, .. } | Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
            visit_expr(operand, on_call)
        }
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => visit_expr(obj, on_call),
        Expr::IndexAccess { obj, index, .. } => {
            visit_expr(obj, on_call);
            visit_expr(index, on_call);
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            visit_expr(cond, on_call);
            visit_expr(then_branch, on_call);
            visit_expr(else_branch, on_call);
        }
        Expr::BlockExpr { stmts, value, .. } => {
            // Statements here can host a SendCustomEvent; handlers cannot, so a
            // no-op handler sink suffices.
            let mut no_handler = |_: &Handler| {};
            for s in stmts {
                visit_stmt(s, &mut no_handler, on_call);
            }
            visit_expr(value, on_call);
        }
        Expr::InterpLit { parts, .. } => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    visit_expr(x, on_call);
                }
            }
        }
        Expr::RecordLit { fields, .. } => {
            for f in fields {
                match f {
                    RecordLitField::Named { value, .. } | RecordLitField::Spread { value, .. } => {
                        visit_expr(value, on_call)
                    }
                    RecordLitField::Shorthand { .. } => {}
                }
            }
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            visit_expr(scrutinee, on_call);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(x) => visit_expr(x, on_call),
                    MatchBody::Block(b) => {
                        let mut no_handler = |_: &Handler| {};
                        visit_block(b, &mut no_handler, on_call);
                    }
                }
            }
        }
        Expr::Array { elements, .. } => {
            for el in elements {
                visit_expr(el.expr(), on_call);
            }
        }
        Expr::MapLit { entries, .. } => {
            for en in entries {
                visit_expr(&en.key, on_call);
                visit_expr(&en.value, on_call);
            }
        }
        Expr::IntLit { .. }
        | Expr::AtomLit { .. }
        | Expr::FloatLit { .. }
        | Expr::StringLit { .. }
        | Expr::BoolLit { .. }
        | Expr::AssetRef { .. }
        | Expr::PrefabRef { .. }
        | Expr::NestedPrefab { .. }
        | Expr::Ident { .. } => {}
    }
}
