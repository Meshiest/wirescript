use std::{collections::HashMap, fmt::Display, sync::Arc};

use super::super::{
    ast::Literal,
    compiler::{Gate, Wire, WireConnection},
};

#[derive(Clone, Debug)]
pub enum CompiledOutput {
    /// An input port of a module
    Input(usize),
    /// An immediate value to be inserted into a gate
    Immediate(Literal),
    /// A reference to an existing gate
    Wire(WireConnection),
}

impl Display for CompiledOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompiledOutput::Input(i) => write!(f, "in{i}"),
            CompiledOutput::Immediate(lit) => lit.fmt(f),
            CompiledOutput::Wire(wire) => wire.fmt(f),
        }
    }
}

impl From<WireConnection> for CompiledOutput {
    fn from(wire: WireConnection) -> Self {
        CompiledOutput::Wire(wire)
    }
}

impl CompiledOutput {
    pub fn replace_gate(&self, lut: &HashMap<usize, Arc<Gate>>) -> Self {
        match self {
            CompiledOutput::Wire(wire) => CompiledOutput::Wire(wire.replace_gate(lut)),
            other => other.clone(),
        }
    }
}

/// A module that has been compiled into a set of wires and gates.
/// This module can be reused in other modules
#[derive(Debug)]
pub struct CompiledModule {
    pub num_inputs: usize,
    /// Inbound wire connections to values inside this module
    /// A pair of (input index, WireConnection) to indicate which input this wire connects to
    pub inputs: Vec<(usize, WireConnection)>,
    /// Output wire connections that can be used to connect to the outputs of this module
    /// Outputs may contain an optional rerouter gate
    pub outputs: Vec<CompiledOutput>,
    /// Wires that connect to gates inside the module
    pub wires: Vec<Wire>,
    /// Gates that are part of this module
    pub gates: Vec<Arc<Gate>>,
    /// Literal values that will be shoved into the gates of this module
    pub gate_literals: Vec<(WireConnection, Literal)>,
    /// Force this module to be inlined rather than being a submodule
    pub force_inline: bool,
    /// List of (module name, module) that this module contains
    pub sub_modules: Vec<(String, CompiledModule)>,
}

impl CompiledModule {
    /// Create a clone of this module with new gate indices and wires pointing at the respective gates.
    pub fn cloned(&self) -> (Self, HashMap<usize, Arc<Gate>>) {
        let mut gates = Vec::with_capacity(self.gates.len());

        // Create a lookup table for the gates
        // Give all of the gates fresh indices
        let mut gate_lut = HashMap::with_capacity(gates.len());

        for g in &self.gates {
            let new_gate = Arc::new(g.cloned());
            gate_lut.insert(g.index, new_gate.clone());
            gates.push(new_gate);
        }

        let mut sub_modules = vec![];

        // Gate lookups are retained for submodules because wires in the parent
        // module connect to gates in the submodule. If the lookups are missing
        // then the wires in parent modules will connect to gates that don't exist
        for (name, module) in &self.sub_modules {
            let (sub_module, more_gates) = module.cloned();
            gate_lut.extend(more_gates);
            sub_modules.push((name.clone(), sub_module));
        }

        // Ensure the new gates and connections reference the new gate ids
        let new_module = Self {
            num_inputs: self.num_inputs,
            inputs: self
                .inputs
                .iter()
                .map(|(i, w)| (*i, w.replace_gate(&gate_lut)))
                .collect(),
            outputs: self
                .outputs
                .iter()
                .map(|output| output.replace_gate(&gate_lut))
                .collect(),
            wires: self
                .wires
                .iter()
                .map(|w| Wire {
                    src: w.src.replace_gate(&gate_lut),
                    dst: w.dst.replace_gate(&gate_lut),
                })
                .collect(),
            gate_literals: self
                .gate_literals
                .iter()
                .map(|(c, v)| (c.replace_gate(&gate_lut), *v))
                .collect(),
            gates,
            force_inline: self.force_inline,
            sub_modules,
        };

        (new_module, gate_lut)
    }
}

impl Display for CompiledModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CompiledModule with {} inputs and {} submodules",
            self.num_inputs,
            self.sub_modules.len()
        )?;
        if !self.inputs.is_empty() {
            f.write_str("\nInputs:\n")?;
            for (i, conn) in &self.inputs {
                writeln!(f, " - [{i}] -> {conn}")?;
            }
        }
        if !self.outputs.is_empty() {
            f.write_str("\nOutputs:\n")?;
            for conn in &self.outputs {
                writeln!(f, " - {conn}")?;
            }
        }
        if !self.gates.is_empty() {
            f.write_str("\nGates:\n")?;
            for gate in &self.gates {
                writeln!(f, " - {gate}")?;
            }
        }
        if !self.gate_literals.is_empty() {
            f.write_str("\nGate Literals:\n")?;
            for (conn, lit) in &self.gate_literals {
                writeln!(f, "   {} = {}", conn, lit)?;
            }
        }

        let submodules = self
            .sub_modules
            .iter()
            .map(|(name, module)| {
                format!(
                    "\nSubmodule {name}:\n | {}",
                    module.to_string().replace("\n", "\n | ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        submodules.fmt(f)?;

        if !self.wires.is_empty() {
            f.write_str("\nWires:\n")?;
            for wire in &self.wires {
                writeln!(f, "  {} -> {}", wire.src, wire.dst)?;
            }
        }

        Ok(())
    }
}
