use std::{collections::HashMap, sync::Arc};

use thiserror::Error;

use crate::{
    ast::AstModule,
    wires::{Gate, Wire, WireConnection},
};

pub struct Compiler {
    ast_modules: HashMap<String, AstModule>,
    compiled_modules: HashMap<String, CompiledModule>,
}

/// A module that has been compiled into a set of wires and gates.
/// This module can be reused in other modules
pub struct CompiledModule {
    pub name: String,
    /// Wire connections that can be used to connect to this module
    pub inputs: Vec<WireConnection>,
    /// Wire connections that can be used to connect to the outputs of this module
    pub outputs: Vec<WireConnection>,
    /// Wires that connect to gates with the module
    pub wires: Vec<Wire>,
    /// Gates that are part of this module
    pub gates: Vec<Arc<Gate>>,
}

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("unknown module {0}")]
    UnknownModule(String),
}

impl Compiler {
    pub fn new(modules: Vec<AstModule>) -> Self {
        let mut ast_modules = HashMap::default();
        for module in modules {
            ast_modules.insert(module.name.clone(), module);
        }

        Self {
            ast_modules,
            compiled_modules: Default::default(),
        }
    }

    pub fn compile(&self, target: &str) -> Result<CompiledModule, CompileError> {
        if !self.ast_modules.contains_key(target) {
            return Err(CompileError::UnknownModule(target.to_owned()));
        }

        // TODO: assemble rerouters for module inputs

        todo!()
    }
}

impl CompiledModule {
    /// Create a clone of this module with new gate indices and wires pointing at the respective gates.
    pub fn cloned(&self) -> Self {
        let mut gates = Vec::with_capacity(self.gates.len());

        // Create a lookup table for the gates
        // Give all of the gates fresh indices
        let mut gate_lut = HashMap::with_capacity(gates.len());
        for g in &self.gates {
            let new_gate = Arc::new(g.cloned());
            gate_lut.insert(g.index, new_gate.clone());
            gates.push(new_gate);
        }

        Self {
            name: self.name.clone(),
            // Ensure the input/output connections reference the new gate ids
            inputs: self
                .inputs
                .iter()
                .map(|w| w.replace_gate(&gate_lut))
                .collect(),
            outputs: self
                .outputs
                .iter()
                .map(|w| w.replace_gate(&gate_lut))
                .collect(),
            // Ensure all of the wires reference the new gate ids
            wires: self
                .wires
                .iter()
                .map(|w| Wire {
                    src: w.src.replace_gate(&gate_lut),
                    dst: w.dst.replace_gate(&gate_lut),
                })
                .collect(),
            gates,
        }
    }
}
