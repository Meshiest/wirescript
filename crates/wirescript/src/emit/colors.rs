//! Brick colour policy for emitted nodes.

use super::*;

// ---------- semantic colouring ----------

// Brickadia renders stored brick-colour bytes as sRGB directly (a raw
// paint value like 60,160,240 shows up as that same bright blue in-game),
// so these are the perceived sRGB colours we want, used verbatim.
const C_YELLOW: Color = Color {
    r: 184,
    g: 145,
    b: 21,
}; // triggers + chip I/O
const C_WHITE: Color = Color {
    r: 184,
    g: 184,
    b: 184,
}; // branch / union / select
const C_GREY: Color = Color {
    r: 72,
    g: 72,
    b: 72,
}; // exec-taking statements
const C_INT: Color = Color {
    r: 39,
    g: 184,
    b: 199,
}; // int — cyan
const C_FLOAT: Color = Color {
    r: 39,
    g: 145,
    b: 72,
}; // float — green
const C_BOOL: Color = Color {
    r: 176,
    g: 39,
    b: 39,
}; // bool — red
const C_STRING: Color = Color {
    r: 184,
    g: 161,
    b: 28,
}; // string — yellow
const C_CHARACTER: Color = Color {
    r: 21,
    g: 28,
    b: 138,
}; // character — deep blue
const C_STRUCT: Color = Color {
    r: 184,
    g: 109,
    b: 28,
}; // vector/struct/entity — orange

/// Choose a brick colour for `node` following the scheme:
/// - Events + chip I/O → yellow
/// - Branch / union / select → white
/// - Pseudo-storage vars → coloured by inner type
/// - Var_Get / Var_Set / Var_Increment → coloured by the var they touch
/// - Other exec-taking statement gates → grey
/// - Pure expressions → coloured by their output type
pub(super) fn color_for_node(
    node: &Node,
    module: &Module,
    wire_target_index: &StdMap<(NodeId, WirePort), NodeId>,
) -> Color {
    if matches!(node.kind, NodeKind::Event) {
        return C_YELLOW;
    }
    // Microchip I/O gates colour by their port's value type so the type reads
    // at a glance; exec (trigger) ports keep the neutral yellow.
    if matches!(node.kind, NodeKind::Input | NodeKind::Output) {
        return io_node_color(node);
    }
    if node.gate_class.contains("Exec_Branch")
        || node.gate_class.contains("Exec_Union")
        || node.gate_class.contains("Expr_Select")
    {
        return C_WHITE;
    }
    let is_pseudo = node
        .gate_class
        .starts_with("BrickComponentType_WireGraphPseudo");
    if is_pseudo {
        if let Some(t) = node
            .ports
            .outputs
            .iter()
            .find(|p| {
                let pn = resolve(p.name);
                pn == "Value" || pn == "Output"
            })
            .map(|p| &p.ty)
        {
            return color_for_type(t);
        }
        return C_STRUCT;
    }
    if node.gate_class.contains("Exec_Var_") || node.gate_class.contains("Exec_ArrayVar_") {
        if let Some(ty) = var_ref_target_type(node, module, wire_target_index) {
            return color_for_type(&ty);
        }
        if let Some(ref_port) = node.ports.inputs.iter().find(|p| {
            let pn = resolve(p.name);
            pn == "VarRef" || pn == "ArrayVarRef"
        }) {
            return color_for_type(&ref_port.ty);
        }
    }
    let takes_exec = node.ports.inputs.iter().any(|p| matches!(p.ty, Type::Exec));
    if takes_exec {
        return C_GREY;
    }
    node.ports
        .outputs
        .iter()
        .find(|p| !matches!(p.ty, Type::Exec))
        .map(|p| color_for_type(&p.ty))
        .unwrap_or(C_GREY)
}

/// For a Var_Get/Var_Set style gate, follow its `VarRef` / `ArrayVarRef`
/// input wire back to the Pseudo_Var source and return that var's inner
/// type. Uses pre-built wire_target_index for O(1) lookup.
fn var_ref_target_type(
    node: &Node,
    module: &Module,
    wire_target_index: &StdMap<(NodeId, WirePort), NodeId>,
) -> Option<Type> {
    let ref_port_sym = node
        .ports
        .inputs
        .iter()
        .find(|p| {
            let pn = resolve(p.name);
            pn == "VarRef" || pn == "ArrayVarRef"
        })
        .map(|p| p.name)?;
    let ref_port_idx = WirePort::from_name(resolve(ref_port_sym));
    let src = wire_target_index.get(&(node.id, ref_port_idx))?;
    let var_node = module.nodes.get(src)?;
    var_node
        .ports
        .outputs
        .iter()
        .find(|p| {
            let pn = resolve(p.name);
            pn == "Value" || pn == "Output"
        })
        .map(|p| p.ty.clone())
}

/// Colour for a microchip I/O gate (and its outer rerouter pin), taken from
/// the port's declared value type. Both the `RER_Input` and `RER_Output` ports
/// carry that type; exec (trigger) ports keep the neutral yellow.
pub(super) fn io_node_color(node: &Node) -> Color {
    let ty = node
        .ports
        .outputs
        .iter()
        .chain(node.ports.inputs.iter())
        .map(|p| &p.ty)
        .next();
    match ty {
        Some(Type::Exec) | None => C_YELLOW,
        Some(t) => color_for_type(t),
    }
}

fn color_for_type(t: &Type) -> Color {
    match t {
        Type::Int => C_INT,
        Type::Float => C_FLOAT,
        Type::Bool => C_BOOL,
        Type::String => C_STRING,
        Type::Character => C_CHARACTER,
        // Ref/Array wrappers: unwrap and recurse so a `Ref<Int>` still
        // colours as int.
        Type::Ref(inner) | Type::Array(inner) => color_for_type(inner),
        // Everything else (Vector, Rotator, Color, Entity, Controller,
        // Brick, Record, Tuple, Union, Any, Never, Exec) falls
        // back to the struct-ish light-orange bucket.
        _ => C_STRUCT,
    }
}
