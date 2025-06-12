use std::{collections::HashMap, sync::Arc};

use thiserror::Error;

use crate::{
    ast::{AstModule, Literal},
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
    #[error("variable {0} is already in use: {1:?}")]
    VariableInUse(String, InUseReason),
}

#[derive(Debug)]
pub enum InUseReason {
    DuplicateInput,
    DuplicateOutput,
    DuplicateBuffer,
    DuplicateConst,
    InputWithSimilarName,
    ConstWithSimilarName,
    OutputWithSimilarName,
}

enum PendingConnection {
    /// A connection from input (as a source)
    Input(String),
    /// An immediate value to be inserted into the gate
    Const(Literal),
    /// A gate and its property
    Gate(Arc<Gate>, String),
}

/// A type of assignable variable
enum Slot {
    Output,
    Buffer,
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
        let Some(ast) = self.ast_modules.get(target) else {
            return Err(CompileError::UnknownModule(target.to_owned()));
        };

        // The scope will be used to lookup existing wire output connections
        // Adding new consts or buffers will introduce new variables into the scope
        let mut scope: HashMap<String, PendingConnection> = Default::default();
        for i in &ast.inputs {
            if scope.contains_key(i) {
                return Err(CompileError::VariableInUse(
                    i.to_owned(),
                    InUseReason::DuplicateInput,
                ));
            }
            scope.insert(i.to_owned(), PendingConnection::Input(i.to_owned()));
        }

        // A map of available slots (outputs or buffers) that can receive
        // a value ONCE.
        let mut slots: HashMap<String, (Slot, Option<PendingConnection>)> = Default::default();
        for o in &ast.outputs {
            if scope.contains_key(o) {
                return Err(CompileError::VariableInUse(
                    o.to_owned(),
                    InUseReason::InputWithSimilarName,
                ));
            }
            if slots.contains_key(o) {
                return Err(CompileError::VariableInUse(
                    o.to_owned(),
                    InUseReason::DuplicateOutput,
                ));
            }

            // This slot has not been assigned yet
            slots.insert(o.to_owned(), (Slot::Output, None));
        }

        let mut gates: Vec<Arc<Gate>> = Vec::new();
        let mut wires: Vec<WireConnection> = Vec::new();

        // TODO: statements:
        // TODO: walk consts and add new slots
        // TODO: when a module is referenced, compile/clone it and append gates/wires, then hook in inputs/outputs
        // TODO: walk buffers and add slots and scope (as they are inputs and outputs)

        // TODO: assemble rerouters for module inputs

        // TODO: warnings if not all slots have assignments

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
