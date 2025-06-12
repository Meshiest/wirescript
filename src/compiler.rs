use std::collections::HashMap;

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
#[derive(Clone)]
pub struct CompiledModule {
    pub name: String,
    /// Wire connections that can be used to connect to this module
    pub inputs: Vec<WireConnection>,
    /// Wire connections that can be used to connect to the outputs of this module
    pub outputs: Vec<WireConnection>,
    /// Wires that connect to gates with the module
    pub wires: Vec<Wire>,
    /// Gates that are part of this module
    pub gates: Vec<Gate>,
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

    pub fn compile(&self, target: String) -> CompiledModule {
        // TODO: assemble rerouters for module inputs

        todo!()
    }
}
