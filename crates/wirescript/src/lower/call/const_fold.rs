//! Move a constant argument inside an instance and drop its input pin.

use super::*;

/// A constant argument to be folded into the chip instance's own module,
/// replacing the input rerouter it would otherwise have been wired to.
pub(in crate::lower) struct ConstFold {
    pub(super) pin: NodeId,
    /// Parameter position, so two calls that fold different params are keyed apart.
    pub(super) index: usize,
    pub(super) value: Literal,
    pub(super) ty: Type,
}

/// A literal argument that can live inside the chip instead of crossing its
/// boundary, with its type. Only self-contained scalars qualify: anything
/// that has to be computed still needs a real wire in. A number literal is
/// the number its parameter's type names: an integer literal for a `float`
/// parameter is the float it names, a float literal for an `int` parameter
/// the integer it truncates to (`expr::constant_literal`), a bool literal for
/// an `int` parameter the integer it names.
pub(super) fn const_arg_literal(ctx: &LowerCtx, e: &Expr) -> Option<(Type, Literal)> {
    match e {
        Expr::IntLit { value, .. } => Some(crate::lower::expr::constant_literal(
            ctx,
            e,
            Literal::Int(*value),
        )),
        Expr::AtomLit { value, .. } => Some((Type::Int, Literal::Int(*value))),
        Expr::FloatLit { value, .. } => Some(crate::lower::expr::constant_literal(
            ctx,
            e,
            Literal::Float(*value),
        )),
        Expr::BoolLit { value, .. } if ctx.type_of(e) == Type::Int => {
            Some((Type::Int, Literal::Int(i64::from(*value))))
        }
        Expr::BoolLit { value, .. } => Some((Type::Bool, Literal::Bool(*value))),
        _ => None,
    }
}

/// Move a constant argument inside the chip instance and drop its input pin.
///
/// A constant that crosses the boundary costs a gate per instance, because the
/// rerouter it feeds can't carry inline gate data. Cloning it into the chip's
/// own module lets the literal-inlining pass fold it onto its consumer, so a
/// `chip` call emits exactly the gates its `mod` equivalent does. This is the
/// same trick already applied to captured constants when the body is built.
///
/// Safe to do per instance: every call site builds its own instance, and the
/// shared template was cloned before any argument wiring ran.
pub(super) fn fold_const_chip_input(ctx: &mut LowerCtx, chip_node_id: NodeId, fold: &ConstFold) {
    let Some(child) = ctx.builder.module.chips.get_mut(&chip_node_id) else {
        return;
    };
    let Some(pin_node) = child.nodes.get(&fold.pin) else {
        return;
    };
    let mut props = HashMap::default();
    props.insert(*sym::VALUE, fold.value.clone());
    let lit_id = NodeId::fresh();
    let ports = GateIO::source(fold.ty.clone());
    let lit = crate::ir::Node::new(
        lit_id,
        NodeKind::Gate,
        gc::LITERAL,
        ports,
        props,
        Some(pin_node),
        None,
    );
    child.nodes.insert(lit_id, lit);
    // Everything the pin fed now reads the literal directly.
    for w in &mut child.wires {
        if w.source.node_id == fold.pin {
            w.source = lit_id.port(WirePort::Output);
        }
    }
    child.wires.retain(|w| w.target.node_id != fold.pin);
    child.nodes.remove(&fold.pin);
    child.inputs.retain(|p| *p != fold.pin);
}
