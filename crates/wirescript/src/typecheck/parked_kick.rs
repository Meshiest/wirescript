//! Parked back-edge kick check (WS053).
//!
//! An unbuffered `emit X` immediately followed, in the SAME statement sequence,
//! by an `await X` on the same signal parks the exec chain forever: the emit
//! fires in the current tick, but the `await` is what ARMS the resume, and it
//! only arms after the emit has already gone. So the await never catches that
//! kick and nothing downstream of it ever runs. `buffer emit X` is correct - it
//! lands the next tick, after the await is armed (which is what a loop's own
//! back edge already does, making the plain kick look deliberate when it isn't).

use super::*;
use crate::collections::HashMap;

/// Walk every block and flag a plain `emit X` that a same-chain `await X`
/// follows (see the module comment).
pub(super) fn check_parked_kicks(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    for d in decls {
        check_decl(ctx, d);
    }
}

fn check_decl(ctx: &mut TypeCheckCtx, d: &TopDecl) {
    match d {
        TopDecl::Chip(c) => check_block(ctx, &c.body),
        TopDecl::AnonChip(ac) => check_block(ctx, &ac.body),
        TopDecl::Handler(h) => check_block(ctx, &h.body),
        TopDecl::If(i) => {
            check_block(ctx, &i.then_block);
            if let Some(e) = &i.else_block {
                check_block(ctx, e);
            }
        }
        _ => {}
    }
}

fn check_block(ctx: &mut TypeCheckCtx, block: &Block) {
    // The pattern is a single-chain hazard: scan the block's DIRECT statements
    // (one exec chain). A nested block (`if`/chip/handler body) is its own chain,
    // scanned separately by the recursion below - an `emit` inside an `if` and an
    // `await` outside it are on different chains and deliberately not flagged.
    scan_chain(ctx, &block.stmts);
    for s in &block.stmts {
        match s {
            Stmt::ChipDecl(c) => check_block(ctx, &c.body),
            Stmt::AnonChip(ac) => check_block(ctx, &ac.body),
            Stmt::Handler(h) => check_block(ctx, &h.body),
            Stmt::If(i) => {
                check_block(ctx, &i.then_block);
                if let Some(e) = &i.else_block {
                    check_block(ctx, e);
                }
            }
            _ => {}
        }
    }
}

fn scan_chain(ctx: &mut TypeCheckCtx, stmts: &[Stmt]) {
    // Signal name -> range of the earliest still-live unbuffered `emit`.
    let mut pending: HashMap<String, SourceRange> = HashMap::default();
    for s in stmts {
        match s {
            Stmt::Emit(e) if e.buffer.is_none() => {
                pending
                    .entry(e.name.clone())
                    .or_insert_with(|| e.range.clone());
            }
            Stmt::Await(a) => {
                if let Expr::Ident { name, .. } = &a.exec_expr
                    && let Some(emit_range) = pending.remove(name)
                {
                    ctx.warn(
                        "WS053",
                        format!(
                            "`emit {name}` fires in this tick, before the following `await {name}` \
                             arms - so the await never resumes from it and the chain parks forever. \
                             Use `buffer emit {name}`, which lands the next tick, after the await is armed"
                        ),
                        emit_range,
                    );
                }
            }
            _ => {}
        }
    }
}
