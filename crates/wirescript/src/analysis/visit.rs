//! The full-AST visitor shared by the WS030 custom-event check
//! (`typecheck::check_custom_event_types`) and custom-event go-to-definition
//! (`analysis::definition::custom_event_send_definition`), which need every
//! `Handler` (for `on CustomEvent(...)` receivers) and every call (for
//! `SendCustomEvent(...)` sends). It runs on `ast::walk`, so it reaches loop
//! bodies, `match` arms, block expressions, handler config and labels.

use crate::ast::walk::{self, Node};
use crate::ast::*;

/// Walk every `Handler` and every `Expr` in the program. `on_handler` fires for
/// each handler (top-level, in chips, anonymous chips, namespaces, nested
/// blocks and captured-event bodies); `on_call` for each call, record literal,
/// `match`, variant construction and embedded prefab, for callers to filter.
/// The visited references live as long as the script, so callers can keep them.
pub fn visit_program<'a>(
    script: &'a Script,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    walk::script_children(script, &mut |n, _| visit(n, on_handler, on_call));
}

fn visit<'a>(
    node: Node<'a>,
    on_handler: &mut dyn FnMut(&'a Handler),
    on_call: &mut dyn FnMut(&'a Expr),
) {
    match node {
        Node::Decl(TopDecl::Handler(h)) | Node::Stmt(Stmt::Handler(h)) => on_handler(h),
        // A captured event's body is the handler it runs.
        Node::Decl(TopDecl::Event(EventDecl {
            captured: Some(h), ..
        })) => on_handler(h),
        // `match` and variant-construction nodes fire too: the "fill missing match
        // arms" action finds the enclosing `match` through this, and hover and
        // go-to-definition on a payload field key read a construction's `fields`.
        // Embedded prefabs fire so a caller can count every `$./file` reference and
        // `$```...``` ` block.
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
