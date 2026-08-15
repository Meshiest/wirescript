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
/// boundary. Only self-contained scalars qualify — anything that has to be
/// computed still needs a real wire in.
pub(super) fn const_arg_literal(e: &Expr) -> Option<Literal> {
    match e {
        Expr::IntLit { value, .. } => Some(Literal::Int(*value)),
        Expr::AtomLit { value, .. } => Some(Literal::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Literal::Float(*value)),
        Expr::BoolLit { value, .. } => Some(Literal::Bool(*value)),
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
    let lit = crate::ir::Node {
        id: lit_id,
        kind: NodeKind::Gate,
        gate_class: gc::LITERAL,
        properties: std::sync::Arc::new(props),
        ports: std::sync::Arc::new(GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: fold.ty.clone(),
            }],
        }),
        source_range: pin_node.source_range.clone(),
        chip_id: pin_node.chip_id,
        chain_id: None,
        scope_id: pin_node.scope_id,
        note: None,
    };
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
