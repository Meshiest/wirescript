use std::{collections::HashMap, sync::Arc};

use crate::brdb::assets::components::{BufferTicks, Rerouter};

use super::ast::{AstExpr, AstModule, BinaryOpCode, Literal, UnaryOpCode};
mod errors;
mod gates;
mod module;
mod wires;
pub use errors::*;
pub use gates::*;
pub use module::*;
pub use wires::*;

pub struct Compiler {
    ast_modules: HashMap<String, Arc<AstModule>>,
    compiled_modules: HashMap<String, CompiledModule>,
}

struct BuildState<'a> {
    inline_children: bool,
    compiler: &'a mut Compiler,
    ast: Arc<AstModule>,
    scope: HashMap<String, CompiledOutput>,
    outputs: HashMap<String, Option<CompiledOutput>>,
    variables: HashMap<String, CompiledOutput>,
    gates: Vec<Arc<Gate>>,
    wires: Vec<Wire>,
    /// A map of buffer variable names to the port on the gate that represents them
    buffers: HashMap<String, (Arc<Gate>, Option<CompiledOutput>)>,
    /// A map of constant variable names to the port on the gate that represents them
    consts: HashMap<String, CompiledOutput>,
    gate_literals: Vec<(WireConnection, Literal)>,
    pending_wires: Vec<(usize, WireConnection)>,
    /// A list of (module name, module) that have been compiled and added to this module
    sub_modules: Vec<(String, CompiledModule)>,
}

impl Compiler {
    pub fn new(modules: impl IntoIterator<Item = AstModule>) -> Self {
        let mut ast_modules = HashMap::default();
        for module in modules {
            ast_modules.insert(module.name.clone(), Arc::new(module));
        }

        Self {
            ast_modules,
            compiled_modules: Default::default(),
        }
    }

    pub fn get_or_compile(
        &mut self,
        target: &str,
        force_inline: bool,
    ) -> Result<CompiledModule, CompileError> {
        if let Some(module) = self.compiled_modules.get(target) {
            return Ok(module.cloned().0);
        }

        let compiled = self.compile_opts(target, force_inline, force_inline)?;
        let clone = compiled.cloned().0;
        self.compiled_modules.insert(target.to_owned(), clone);
        Ok(compiled)
    }

    pub fn compile(&mut self, target: &str) -> Result<CompiledModule, CompileError> {
        self.compile_opts(target, false, false)
    }

    pub fn compile_opts(
        &mut self,
        target: &str,
        inline_self: bool,
        inline_children: bool,
    ) -> Result<CompiledModule, CompileError> {
        let Some(ast) = self.ast_modules.get(target).map(Arc::clone) else {
            return Err(CompileError::UnknownModule(target.to_owned()));
        };

        let mut state = BuildState::new(self, ast, inline_children)?;
        state.compile_statements()?;
        state.build(inline_self)
    }
}

impl<'a> BuildState<'a> {
    fn new(
        compiler: &'a mut Compiler,
        ast: Arc<AstModule>,
        inline_children: bool,
    ) -> Result<Self, CompileError> {
        // The scope will be used to lookup existing wire output connections
        // Adding new consts or buffers will introduce new variables into the scope
        let mut scope: HashMap<String, CompiledOutput> = Default::default();

        for (i, name) in ast.inputs.iter().enumerate() {
            if scope.contains_key(name) {
                return Err(CompileError::VariableInUse(
                    name.to_owned(),
                    InUseReason::DuplicateInput,
                ));
            }
            scope.insert(name.to_owned(), CompiledOutput::Input(i));
        }

        // A map of output slots that can only be assigned ONCE
        let mut outputs: HashMap<String, Option<CompiledOutput>> = Default::default();
        for o in &ast.outputs {
            if scope.contains_key(o) {
                return Err(CompileError::VariableInUse(
                    o.to_owned(),
                    InUseReason::InputWithSimilarName,
                ));
            }
            if outputs.contains_key(o) {
                return Err(CompileError::VariableInUse(
                    o.to_owned(),
                    InUseReason::DuplicateOutput,
                ));
            }

            outputs.insert(o.to_owned(), None);
        }

        Ok(Self {
            inline_children,
            ast,
            compiler,
            scope,
            outputs,
            variables: Default::default(),
            gates: Default::default(),
            wires: Default::default(),
            gate_literals: Default::default(),
            pending_wires: Default::default(),
            buffers: Default::default(),
            consts: Default::default(),
            sub_modules: Default::default(),
        })
    }

    fn build(mut self, force_inline: bool) -> Result<CompiledModule, CompileError> {
        let ast = self.ast.clone();

        let is_inline = force_inline || ast.inline;

        if !is_inline {
            // If this is not an inline module, add rerouters for the inputs and outputs
            self.reroute_ports()?;
        }

        // Resolve output order by name in the ast
        let outputs = ast
            .outputs
            .iter()
            .map(|n| {
                self.outputs
                    .get(n)
                    .cloned()
                    .ok_or_else(|| CompileError::UnknownVariable(n.to_owned()))?
                    .ok_or_else(|| CompileError::UnassignedOutput(n.to_owned()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(CompiledModule {
            num_inputs: ast.inputs.len(),
            outputs,
            inputs: self.pending_wires,
            wires: self.wires,
            gates: self.gates,
            gate_literals: self.gate_literals,
            sub_modules: self.sub_modules,
            force_inline: is_inline,
        })
    }

    /// Insert rerouters on the inputs and outputs of this module, rerouting
    /// the previous connections through them
    fn reroute_ports(&mut self) -> Result<(), CompileError> {
        let mut new_wires = vec![];

        let old_inputs = std::mem::take(&mut self.pending_wires);
        let mut new_gates = vec![];

        // Create new named rerouters for each input
        for (index, name) in self.ast.clone().inputs.iter().enumerate() {
            let rerouter = Arc::new(
                Gate::new(&GateKind::Reroute)
                    .with_label(name)
                    .with_input(index),
            );
            // Add a pending wire that routes into the new rerouter input
            self.pending_wires
                .push((index, WireConnection::new(&rerouter, Rerouter::INPUT)));
            new_gates.push(rerouter);
        }

        // Connect the old inputs to the new rerouters
        for (index, dst) in old_inputs {
            // Should be unreachable, but get the new rerouter for the input
            let Some(gate) = new_gates.get(index) else {
                return Err(CompileError::ModuleInputOutOfRange(
                    self.ast.inputs.len(),
                    index,
                ));
            };

            // Create a wire between the new rerouter and the old input
            self.add_wire(
                CompiledOutput::Wire(WireConnection::new(gate, Rerouter::OUTPUT)),
                CompiledOutput::Wire(dst),
            )?;
        }

        let output_indices = self
            .ast
            .outputs
            .iter()
            .enumerate()
            .map(|(i, name)| (name, i))
            .collect::<HashMap<_, _>>();

        // Insert the named rerouters for each output
        for (name, slot) in self.outputs.iter_mut() {
            let Some(src) = slot.take() else {
                return Err(CompileError::UnassignedOutput(name.to_owned()));
            };

            let src = match src {
                // If the internal output would have connected to the input of a module,
                // instead use the new rerouter
                CompiledOutput::Input(i) => {
                    let Some(gate) = new_gates.get(i) else {
                        return Err(CompileError::ModuleInputOutOfRange(
                            self.ast.inputs.len(),
                            i,
                        ));
                    };
                    CompiledOutput::Wire(WireConnection::new(gate, Rerouter::OUTPUT))
                }
                other => other,
            };

            // Create a rerouter for the output
            let rerouter = Arc::new(
                Gate::new(&GateKind::Reroute).with_label(name).with_output(
                    *output_indices
                        .get(name)
                        .ok_or_else(|| CompileError::UnknownVariable(name.to_owned()))?,
                ),
            );

            // Create a wire between the old slot and the rerouter input
            new_wires.push((
                src,
                CompiledOutput::Wire(WireConnection::new(&rerouter, Rerouter::INPUT)),
            ));

            // Update the slot to point to the rerouter output
            *slot = Some(CompiledOutput::Wire(WireConnection::new(
                &rerouter,
                Rerouter::OUTPUT,
            )));

            new_gates.push(rerouter);
        }

        self.gates.extend(new_gates);

        // Add the new wires to the wires list
        for (src, dst) in new_wires {
            self.add_wire(src, dst)?;
        }

        Ok(())
    }

    /// Compile a list of expressions and return a list of input and output pending connections.
    fn exprs<'b>(
        &mut self,
        exprs: impl Iterator<Item = &'b AstExpr>,
    ) -> Result<Vec<CompiledOutput>, CompileError> {
        use AstExpr::*;

        // walk the expressions and resolve them into CompiledModules
        // effectively union the modules into a single module

        let mut outputs = Vec::new();

        for expr in exprs {
            match expr {
                Literal(literal) => Self::literal_expr(literal, &mut outputs),
                Var(name) => self.assign_expr(name, &mut outputs)?,
                BinaryOp(op, a, b) => self.binaryop_expr(*op, a, b, &mut outputs)?,
                UnaryOp(op, input) => self.unaryop_expr(*op, input, &mut outputs)?,
                Call(name, params) => {
                    // TODO: discern if "name" is for a module or a builtin
                    self.module_expr(name, params, &mut outputs)?;
                }
            };
        }

        Ok(outputs)
    }

    /// A constant expression is a literal value that can be directly inserted
    fn literal_expr(literal: &Literal, outputs: &mut Vec<CompiledOutput>) {
        outputs.push(CompiledOutput::Literal(literal.clone()))
    }

    /// A variable expression is a reference to another gate and is entirely metadata
    fn assign_expr(
        &self,
        name: &str,
        outputs: &mut Vec<CompiledOutput>,
    ) -> Result<(), CompileError> {
        let Some(conn) = self.scope.get(name) else {
            return Err(CompileError::UnknownVariable(name.to_owned()));
        };

        outputs.push(conn.clone());
        Ok(())
    }

    /// A binary operation expression is a combination of two expressions
    /// that produces a single output.
    fn binaryop_expr(
        &mut self,
        op: BinaryOpCode,
        a: &AstExpr,
        b: &AstExpr,
        outputs: &mut Vec<CompiledOutput>,
    ) -> Result<(), CompileError> {
        // Resolve the operands as expressions (recursively)
        let a_outputs = self.exprs([a].into_iter())?;
        if a_outputs.len() != 1 {
            return Err(
                CompileError::MismatchedOutputs(a.to_string(), 1, a_outputs.len())
                    .wrap(format!("{op} a operand")),
            );
        }
        let b_outputs = self.exprs([b].into_iter())?;
        if b_outputs.len() != 1 {
            return Err(
                CompileError::MismatchedOutputs(b.to_string(), 1, b_outputs.len())
                    .wrap(format!("{op} b operand")),
            );
        }

        let module = GateKind::BinaryOp(op).module();

        let module_outs = self.add_module(
            &op.to_string(),
            module,
            vec![
                // Unwrap safety: a_outputs and b_outputs are guaranteed to have one item each
                a_outputs.into_iter().next().unwrap(),
                b_outputs.into_iter().next().unwrap(),
            ],
        )?;
        outputs.extend(module_outs);
        Ok(())
    }

    /// A unary operation expression is a single expression that produces a single output.
    fn unaryop_expr(
        &mut self,
        op: UnaryOpCode,
        input: &AstExpr,
        outputs: &mut Vec<CompiledOutput>,
    ) -> Result<(), CompileError> {
        // Resolve the operand as an expression (recursively)
        let outputs_expr = self.exprs([input].into_iter())?;
        if outputs_expr.len() != 1 {
            return Err(
                CompileError::MismatchedOutputs(input.to_string(), 1, outputs_expr.len()).wrap(op),
            );
        }

        let module = GateKind::UnaryOp(op).module();
        let module_outs = self.add_module(
            &op.to_string(),
            module,
            vec![
                // Unwrap safety: outputs_expr is guaranteed to have one item
                outputs_expr.into_iter().next().unwrap(),
            ],
        )?;
        outputs.extend(module_outs);
        Ok(())
    }

    /// Invoke a module
    fn module_expr(
        &mut self,
        name: &str,
        params: &[AstExpr],
        outputs: &mut Vec<CompiledOutput>,
    ) -> Result<(), CompileError> {
        // Build or lookup a module with the given name
        let module = self
            .compiler
            .get_or_compile(name, self.inline_children)
            .map_err(|e| e.wrap(format!("call {name}")))?;

        // Resolve the input expressions
        let call_outputs = self.exprs(params.iter())?;

        // Ensure the outputs match the module's inputs
        if call_outputs.len() != module.num_inputs {
            return Err(CompileError::MismatchedInputs(
                name.to_owned(),
                module.num_inputs,
                call_outputs.len(),
            ));
        }

        // Wire the module inputs to the call outputs
        let module_outs = self.add_module(name, module, call_outputs)?;
        outputs.extend(module_outs);
        Ok(())
    }

    /// Add a wire connection between two pending connections.
    /// If the wire can be realized immediately, it will be added to the wires list.
    /// Otherwise, it will be added to the pending wires list, where it may get forwarded to the next module.
    fn add_wire(&mut self, src: CompiledOutput, dst: CompiledOutput) -> Result<(), CompileError> {
        match (src, dst) {
            // Two wire connections can be resolved immediately
            (CompiledOutput::Wire(src), CompiledOutput::Wire(dst)) => {
                self.wires.push(Wire { src, dst });
            }
            // A wire from an input is always pending
            (CompiledOutput::Input(name), CompiledOutput::Wire(dst)) => {
                // If the source is an input, we need to create a pending wire
                self.pending_wires.push((name, dst));
            }
            (CompiledOutput::Literal(literal), CompiledOutput::Wire(dst)) => {
                self.gate_literals.push((dst, literal));
            }
            (_, CompiledOutput::Input(_)) => {
                unreachable!("Cannot wire to an input connection");
            }
            (_, CompiledOutput::Literal(_)) => {
                unreachable!("Cannot wire to an immediate");
            }
        }

        Ok(())
    }

    /// Absorb the gates and wires from the module into the current state and
    /// redirect the module's inputs to the provided inputs.
    fn add_module(
        &mut self,
        name: &str,
        module: CompiledModule,
        inputs: Vec<CompiledOutput>,
    ) -> Result<Vec<CompiledOutput>, CompileError> {
        assert_eq!(
            module.num_inputs,
            inputs.len(),
            "Module inputs do not match",
        );

        let input_for = |i| {
            inputs
                .get(i)
                .ok_or(CompileError::ModuleInputOutOfRange(inputs.len(), i))
                .cloned()
        };

        // Resolve the module outputs and return them as pending connections
        // This is necessary because an output from a module could be an input to the parent module
        let outputs = module
            .outputs
            .iter()
            .map(|out| match out {
                CompiledOutput::Input(p) => input_for(*p),
                w => Ok(w.clone()),
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Connect input wires into the module
        // The module's inputs are connections to its internal gates.
        // If
        for (input, dst) in module.inputs.iter() {
            self.add_wire(input_for(*input)?, CompiledOutput::Wire(dst.clone()))?;
        }

        // The gates and wires of an inlined module are merged into this module
        if module.force_inline {
            self.gates.extend(module.gates);
            self.wires.extend(module.wires);
            self.gate_literals.extend(module.gate_literals);
        } else {
            // Keeping the gates and wires local to a submodule allows for it to be
            // better visually grouped for graph visualizations and build assembling.
            // There is no computational need to store the gates in submodules
            self.sub_modules.push((name.to_owned(), module));
        }

        Ok(outputs)
    }

    /// Compile the statements in the AST module.
    fn compile_statements(&mut self) -> Result<(), CompileError> {
        for s in &Arc::clone(&self.ast).statements {
            use super::ast::AstStmt::*;
            // Resolve all the exprs, map them to the names (in order)
            // The number of outputs from the expression must match the number of names
            match s {
                Assign(names, exprs) => {
                    let outputs = self.exprs(exprs.iter())?;
                    Self::ensure_outputs_len(names, outputs.len())?;

                    for (name, conn) in names.iter().zip(outputs.into_iter()) {
                        // Try assigning a buffer
                        match self.buffers.get_mut(name) {
                            Some((_slot, Some(_))) => {
                                return Err(CompileError::BufferAlreadyAssigned(name.to_owned()));
                            }
                            Some((_, opt @ None)) => {
                                // Assign the connection to the slot
                                *opt = Some(conn.clone());
                                // Now that the variable is assigned, it can be used as a variable
                                self.scope.insert(name.to_owned(), conn);
                                continue;
                            }
                            _ => {}
                        }

                        // Overwrite the previous output if it exists
                        match self.outputs.get_mut(name) {
                            Some(slot) => {
                                // Assign the connection to the output slot
                                *slot = Some(conn.clone());
                                // Now that the variable is assigned, it can be used as a variable
                                self.scope.insert(name.to_owned(), conn);
                                continue;
                            }
                            None => {}
                        }

                        // Overwrite the previous variable if it exists
                        match self.variables.get_mut(name) {
                            Some(prev) => {
                                // Assign the connection to the variable
                                *prev = conn.clone();
                                self.scope.insert(name.to_owned(), conn.clone());
                                continue;
                            }
                            None => {}
                        }

                        // If the variable is not a buffer or output, it must be a constant or an input
                        if self.scope.contains_key(name) {
                            return Err(CompileError::ReadOnlyVariable(name.to_owned()));
                        }

                        // The only assignable values are buffers and outputs.
                        return Err(CompileError::UnknownVariable(name.to_owned()));
                    }
                }
                Const(names, exprs) => {
                    let outputs = self.exprs(exprs.iter())?;
                    Self::ensure_outputs_len(names, outputs.len())?;

                    for (name, conn) in names.iter().zip(outputs.into_iter()) {
                        self.ensure_unique_name(name)?;

                        // Add the constant to the scope and consts map
                        self.scope.insert(name.to_owned(), conn.clone());
                        self.consts.insert(name.to_owned(), conn.clone());
                    }
                }
                Let(names, exprs) => {
                    let outputs = self.exprs(exprs.iter())?;
                    Self::ensure_outputs_len(names, outputs.len())?;

                    for (name, conn) in names.iter().zip(outputs.into_iter()) {
                        self.ensure_unique_name(name)?;

                        // Add the variable to the scope
                        self.scope.insert(name.to_owned(), conn.clone());
                        self.variables.insert(name.to_owned(), conn);
                    }
                }
                Buffer(names, exprs) => {
                    // Register the buffer names in the state
                    for name in names {
                        self.ensure_unique_name(name)?;

                        let gate = Arc::new(Gate::new(&GateKind::Buffer));

                        // Buffers are available in the scope even when they are not assigned
                        self.scope.insert(
                            name.to_owned(),
                            CompiledOutput::Wire(WireConnection::new(&gate, BufferTicks::OUTPUT)),
                        );

                        // Add the buffer to the state
                        self.buffers
                            .insert(name.to_owned(), (Arc::clone(&gate), None));
                        self.gates.push(gate);
                    }

                    // If this is an assignment, we need to resolve the expressions
                    // and add the wires to the buffers
                    if let Some(exprs) = exprs {
                        let outputs = self.exprs(exprs.iter())?;
                        Self::ensure_outputs_len(names, outputs.len())?;

                        // Add the wires to the buffers
                        for (name, output) in names.iter().zip(outputs.into_iter()) {
                            let gate = {
                                let Some((buf, slot)) = self.buffers.get_mut(name) else {
                                    return Err(CompileError::UnknownVariable(name.to_owned()));
                                };

                                // Somehow the buffer is already assigned. Should be unreachable
                                if slot.is_some() {
                                    return Err(CompileError::ReadOnlyVariable(name.to_owned()));
                                }
                                *slot = Some(output.clone());
                                Arc::clone(buf)
                            };

                            // Redirect the wire write to the buffer's input
                            self.add_wire(
                                output,
                                CompiledOutput::Wire(WireConnection::new(
                                    &gate,
                                    BufferTicks::INPUT,
                                )),
                            )?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Ensure that the number of outputs matches the number of names
    /// This is used to ensure that the number of outputs from an expression matches the number of names
    fn ensure_outputs_len(names: &[String], outputs: usize) -> Result<(), CompileError> {
        if names.len() != outputs {
            return Err(CompileError::MismatchedOutputs(
                names.join(", "),
                names.len(),
                outputs,
            ));
        }
        Ok(())
    }

    /// Ensure a new addition to the scope has a unique name and does not conflict with
    /// any slots (buffers, outputs)
    fn ensure_unique_name(&self, name: &str) -> Result<(), CompileError> {
        if self.buffers.contains_key(name) {
            return Err(CompileError::VariableInUse(
                name.to_owned(),
                InUseReason::BufferWithSimilarName,
            ));
        }
        if self.outputs.contains_key(name) {
            return Err(CompileError::VariableInUse(
                name.to_owned(),
                InUseReason::OutputWithSimilarName,
            ));
        }
        if self.consts.contains_key(name) {
            return Err(CompileError::VariableInUse(
                name.to_owned(),
                InUseReason::ConstWithSimilarName,
            ));
        }
        if self.scope.contains_key(name) {
            return Err(CompileError::VariableInUse(
                name.to_owned(),
                InUseReason::InputWithSimilarName,
            ));
        }

        Ok(())
    }
}
