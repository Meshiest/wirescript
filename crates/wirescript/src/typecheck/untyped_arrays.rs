//! `var` declarations whose initializer is an array or map literal and that
//! carry no type annotation.
//!
//! A literal with elements is typed by the join of its elements: `[1, 2.5]`
//! is a `float[]`, `[c, d]` of an enum is an array of that enum. An empty
//! `[]` or `{}` says nothing about what the container holds, and a container
//! gate has one element variant, so it is WS098, asking for an annotation.

use super::*;
use crate::types::mono::unwrap_ref;

/// An array element type an unannotated array literal can take from its
/// elements: a single wire, or an enum with no type arguments (stored as
/// columns, as `var x: Hue[]` is).
pub(super) fn inferable_element(t: &Type) -> bool {
    var_storable(t) || matches!(t, Type::Enum { args, .. } if args.is_empty())
}

/// A top-level `var x = [..]` with a constant element that is not spelled as
/// a literal (`[1, K]`): registration typed it from its literal elements
/// alone, so give it the join of every element's value before any body reads
/// it.
pub(super) fn type_constant_elements(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    for d in decls {
        let TopDecl::Var(v) = d else { continue };
        let (None, Some(Expr::Array { elements, .. })) = (&v.typ, &v.init) else {
            continue;
        };
        if elements
            .iter()
            .all(|el| matches!(el, ArrayElem::Item(e) if literal_expr_type(e).is_some()))
        {
            continue;
        }
        let types: Option<Vec<Type>> = elements
            .iter()
            .map(|el| match el {
                ArrayElem::Item(e) => {
                    let lit = ctx.const_eval(e, crate::const_scope::Admit::All).ok()?;
                    match crate::lower::wire_type_of_literal(&lit) {
                        Some(t) => Some(t),
                        // An enum value (`Hue.Blue`, `Shape.Circle(2.5)`) is the
                        // enum it constructs, also through a constant naming one;
                        // the body check reports anything amiss with it.
                        None => {
                            let t = ctx.discarding_diagnostics(|ctx| {
                                unwrap_ref(&infer::infer(ctx, named_value(decls, e)))
                            });
                            matches!(t, Type::Enum { .. }).then_some(t)
                        }
                    }
                }
                ArrayElem::Spread(_) => None,
            })
            .collect();
        if let Some(elem) = types.and_then(widening_join_all)
            && inferable_element(&elem)
        {
            ctx.scope
                .set_type(&v.name, Type::Ref(Box::new(Type::Array(Box::new(elem)))));
        }
    }
}

/// The expression a top-level constant `e` names was declared with, through
/// any chain of such names; `e` itself for anything else.
fn named_value<'a>(decls: &'a [TopDecl], mut e: &'a Expr) -> &'a Expr {
    for _ in 0..decls.len() {
        let Expr::Ident { name, .. } = e else { break };
        let declared = decls.iter().find_map(|d| match d {
            TopDecl::Let(l) if matches!(&l.binding, LetBinding::Ident { name: n, .. } if n == name) => Some(&l.value),
            _ => None,
        });
        match declared {
            Some(v) => e = v,
            None => break,
        }
    }
    e
}

/// An array literal with no element of its own: empty, or only spreads of
/// such literals (`[...[]]`).
fn spreads_only_empty(e: &Expr) -> bool {
    match e {
        Expr::Array { elements, .. } => elements
            .iter()
            .all(|el| matches!(el, ArrayElem::Spread(s) if spreads_only_empty(s))),
        _ => false,
    }
}

/// After a `var` declaration is checked: an unannotated empty `[]` or `{}`
/// initializer, or an array literal with only spreads of empty ones, is WS098.
pub(super) fn declare(ctx: &mut TypeCheckCtx<'_>, v: &VarDecl) {
    if v.typ.is_some() {
        return;
    }
    let message = match &v.init {
        Some(Expr::Array { elements, .. }) if elements.is_empty() => format!(
            "`{}` is declared `= []` with no element type, so its storage can't be chosen. \
             Annotate the declaration with the type it holds, such as `var {}: int[] = []`",
            v.name, v.name
        ),
        Some(init) if spreads_only_empty(init) => format!(
            "`{}` is declared with an array literal whose only elements are spreads of empty \
             literals, so it has no element type and its storage can't be chosen. Annotate the \
             declaration with the type it holds, such as `var {}: int[] = []`",
            v.name, v.name
        ),
        Some(init) if map_init_entries(init).is_some_and(<[crate::ast::MapLitEntry]>::is_empty) => {
            format!(
                "`{}` is declared `= {{}}` with no key and value types, so its storage can't be \
             chosen. Annotate the declaration with the types it holds, such as \
             `var {}: Map<string, int> = {{}}`",
                v.name, v.name
            )
        }
        _ => return,
    };
    let at = &v.range;
    if ctx.untyped_reported.insert(at.start_key()) {
        ctx.emit("WS098", message, at.clone());
    }
}
