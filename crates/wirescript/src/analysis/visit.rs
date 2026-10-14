//! Shared full-AST visitor used by both the WS030 custom-event type check
//! (`typecheck::check_custom_event_types`) and custom-event go-to-definition
//! (`analysis::definition::custom_event_send_definition`). Both passes need
//! the same two things out of a walk over the whole program: every `Handler`
//! (to find `on CustomEvent(...)` receivers) and every `Expr` that is a call
//! (to find `SendCustomEvent(...)` sends). The walk itself is the canonical
//! traversal in `ast::walk`, so it reaches every position the AST has: loop
//! bodies, `match` arms, block expressions, handler config and labels.

use crate::ast::walk::{self, Node};
use crate::ast::*;

/// Walk every `Handler` and every `Expr` (including call expressions) in the
/// program. `on_handler` fires for each handler (top-level, in chips/anon-chips,
/// in namespaces, in nested blocks, AND in a captured-event body); `on_call`
/// fires for each call, record literal, `match`, variant construction and
/// embedded prefab so callers can match the ones they care about (e.g.
/// `SendCustomEvent(...)` calls). Lifetimes tie the visited refs to the script
/// so callers may collect `&'a Handler` / `&'a Expr`.
pub fn visit_program<'a>(
    script: &'a Script,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    walk::script_children(script, &mut |n, _| visit(n, on_handler, on_call));
}

fn visit<'a>(node: Node<'a>, on_handler: &mut dyn FnMut(&'a Handler), on_call: &mut dyn FnMut(&'a Expr)) {
    match node {
        Node::Decl(TopDecl::Handler(h)) | Node::Stmt(Stmt::Handler(h)) => on_handler(h),
        // A captured event's body is the handler it runs.
        Node::Decl(TopDecl::Event(EventDecl { captured: Some(h), .. })) => on_handler(h),
        // The `match` and variant-construction nodes fire too: the "fill
        // missing match arms" code action locates the enclosing `match` at a
        // cursor through this, and hover / go-to-definition on a named payload
        // field's key inspect a construction's `fields`. Embedded prefab
        // leaves fire so a caller can count every `$./file` reference and
        // `$```…``` ` block. Callers filter for the kinds they want.
        Node::Expr(
            e @ (Expr::Call { .. }
            | Expr::RecordLit { .. }
            | Expr::MatchExpr { .. }
            | Expr::VariantCtor { .. }
            | Expr::PrefabRef { .. }
            | Expr::NestedPrefab { .. }),
        ) => on_call(e),
        _ => {}
    }
    walk::children(node, &mut |c, _| visit(c, &mut *on_handler, &mut *on_call));
}

/// Whether `e` contains a call anywhere (or one of the other nodes
/// [`visit_program`] reports: a record literal, `match`, variant construction
/// or embedded prefab).
pub(crate) fn contains_call(e: &Expr) -> bool {
    let mut found = false;
    visit(Node::Expr(e), &mut |_| {}, &mut |_| found = true);
    found
}
