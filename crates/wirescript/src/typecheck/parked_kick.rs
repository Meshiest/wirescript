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
/// follows (see the module comment). Each block, and each block expression,
/// is one exec chain: an `emit` inside an `if` and an `await` outside it are
/// on different chains and deliberately not flagged. An `import * as` module's
/// own declarations are its file's to report.
pub(super) fn check_parked_kicks(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    use crate::ast::walk::{self, Node};
    for d in decls.iter().filter(|d| !matches!(d, TopDecl::Namespace(_))) {
        walk::walk(Node::Decl(d), walk::Ctx::default(), &mut |node, _| {
            match node {
                Node::Block(b) => scan_chain(ctx, &b.stmts),
                Node::Expr(Expr::BlockExpr { stmts, .. }) => scan_chain(ctx, stmts),
                _ => {}
            }
            true
        });
    }
}

fn scan_chain(ctx: &mut TypeCheckCtx<'_>, stmts: &[Stmt]) {
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
