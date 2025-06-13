use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
    io::Write,
    sync::{Arc, atomic},
};

use crate::ast::{BinaryOpCode, Literal, UnaryOpCode};

#[derive(Debug, Clone)]
pub struct WireConnection {
    pub gate: Arc<Gate>,
    pub property: String,
}

impl WireConnection {
    pub fn new(gate: &Arc<Gate>, property: impl Display) -> Self {
        Self {
            gate: Arc::clone(gate),
            property: property.to_string(),
        }
    }

    pub fn replace_gate(&self, lut: &HashMap<usize, Arc<Gate>>) -> Self {
        if let Some(g) = lut.get(&self.gate.index) {
            Self {
                gate: Arc::clone(g),
                property: self.property.clone(),
            }
        } else {
            self.clone()
        }
    }
}

impl Display for WireConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.gate, self.property)
    }
}

#[derive(Clone, Debug)]
pub struct Wire {
    pub src: WireConnection,
    pub dst: WireConnection,
}

#[derive(Clone, Debug)]
pub enum GateKind {
    Buffer,
    ReRouter,
    BinaryOp(BinaryOpCode),
    UnaryOp(UnaryOpCode),
}

impl Display for GateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateKind::Buffer => f.write_str("buffer"),
            GateKind::ReRouter => f.write_str("rerouter"),
            GateKind::BinaryOp(op) => op.fmt(f),
            GateKind::UnaryOp(op) => op.fmt(f),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GateMeta {
    pub is_input: bool,
    pub is_output: bool,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Gate {
    pub kind: GateKind,
    pub index: usize,
    pub meta: GateMeta,
}

impl Display for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.kind, self.index)
    }
}

impl Gate {
    fn next_index() -> usize {
        static NEXT_INDEX: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
        NEXT_INDEX.fetch_add(1, atomic::Ordering::SeqCst)
    }

    pub fn new(kind: &GateKind) -> Self {
        Self {
            kind: kind.clone(),
            index: Gate::next_index(),
            meta: Default::default(),
        }
    }

    pub fn with_label(mut self, label: impl Display) -> Self {
        self.meta.label = Some(label.to_string());
        self
    }

    pub fn with_input(mut self) -> Self {
        self.meta.is_input = true;
        self
    }

    pub fn with_output(mut self) -> Self {
        self.meta.is_output = true;
        self
    }

    pub fn cloned(&self) -> Self {
        Self {
            kind: self.kind.clone(),
            index: Gate::next_index(),
            meta: self.meta.clone(),
        }
    }
}

impl GateKind {
    /// Get the input and output properties of this gate kind.
    pub fn properties(&self) -> (Vec<String>, Vec<String>) {
        match self {
            GateKind::BinaryOp(_op) => {
                let inputs = vec!["a".to_string(), "b".to_string()];
                let outputs = vec!["output".to_string()];
                (inputs, outputs)
            }
            GateKind::UnaryOp(_) | GateKind::Buffer | GateKind::ReRouter => {
                let inputs = vec!["input".to_string()];
                let outputs = vec!["output".to_string()];
                (inputs, outputs)
            }
        }
    }

    pub fn module(&self) -> CompiledModule {
        let gate = Arc::new(Gate::new(self));

        let (inputs, outputs) = self.properties();

        CompiledModule {
            num_inputs: inputs.len(),
            inputs: inputs
                .into_iter()
                .enumerate()
                .map(|(i, p)| (i, WireConnection::new(&gate, p)))
                .collect(),
            outputs: outputs
                .into_iter()
                .map(|p| WireConnection::new(&gate, p).into())
                .collect(),
            wires: Default::default(),
            gates: vec![gate],
            gate_literals: Default::default(),
            // Gates are atomic and should always be inlined
            force_inline: true,
            // No submodules in a single gate module
            sub_modules: Default::default(),
        }
    }
}

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
    /// List of (module name, inputs, module) that this module contains
    pub sub_modules: Vec<(String, Vec<CompiledOutput>, CompiledModule)>,
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

        // Ensure the new gates and connections reference the new gate ids
        Self {
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
            sub_modules: self
                .sub_modules
                .iter()
                .map(|(name, inputs, module)| {
                    (
                        name.clone(),
                        inputs.iter().map(|i| i.replace_gate(&gate_lut)).collect(),
                        module.cloned(),
                    )
                })
                .collect(),
        }
    }

    pub fn digraph(&self) -> Result<String, Box<dyn Error>> {
        let mut f = vec![];
        self.subgraph("module", vec![], &mut Default::default(), &mut f, 0)?;
        Ok(String::from_utf8(f)?)
    }

    pub fn subgraph(
        &self,
        name: &str,
        prefix: Vec<usize>,
        // A map of input_index to the name of the node it corresponds to
        input_map: &mut HashMap<Vec<usize>, String>,
        f: &mut impl Write,
        depth: usize,
    ) -> Result<(), Box<dyn Error>> {
        static CONST_INDEX: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

        let root_pad = "  ".repeat(depth);
        let pad = "  ".repeat(depth + 1);

        if depth == 0 {
            writeln!(f, "{root_pad}digraph {name} {{")?;
            // writeln!(f, "{pad}graph [rankdir=LR];")?;
            // graph [splines=ortho]; // (add hard lines)
        } else {
            // Connect the inputs for descendant modules. This has to happen before the subgraph
            // is created so the connected nodes are outside of the subgraph.
            for (i, w) in &self.inputs {
                if let Some(name) = input_map.get(&[prefix.clone(), vec![*i]].concat()) {
                    writeln!(f, "{root_pad}{name} -> {}:{};", w.gate, w.property)?;
                }
            }

            let idx = CONST_INDEX.fetch_add(1, atomic::Ordering::SeqCst);
            writeln!(f, "\n{root_pad}subgraph cluster_{idx} {{")?;
            writeln!(f, "{pad}label=\"{name}\"; color=black;\n")?;
        }

        // Write the gates as nodes
        for gate in &self.gates {
            let (inputs, outputs) = gate.kind.properties();
            let one_input = inputs.len() == 1;
            let one_output = outputs.len() == 1;

            let mut inputs = inputs.into_iter().peekable();
            let mut outputs = outputs.into_iter().peekable();

            // <input> <output> or <input,output> if there is only one input/output
            let first_labels = match (one_input, one_output) {
                (true, true) => format!("<{},{}>", inputs.next().unwrap(), outputs.next().unwrap()),
                (true, false) => format!("<{}>", inputs.next().unwrap()),
                (false, true) => format!("<{}>", outputs.next().unwrap()),
                (false, false) => String::new(),
            };

            // If there are no inputs/outputs, we don't need a divider
            let ports_divider = if inputs.peek().is_some() || outputs.peek().is_some() {
                "|"
            } else {
                ""
            };

            let mut ports = inputs
                .map(|i| format!("<{i}> {i}"))
                .chain(outputs.map(|o| format!("<{o}> {o}")))
                .collect::<Vec<_>>()
                .join("|");
            if !ports.is_empty() {
                // Wrap the ports in curly braces to stack them vertically
                ports = format!("{{{}}}", ports);
            }

            // This will format the gates as blocks
            writeln!(
                f,
                "{pad}{gate} [label=\"{ports}{ports_divider}{first_labels}{display}\",shape=record{io}]",
                // Use the gate label if it exists, otherwise use the gate's kind
                display = gate
                    .meta
                    .label
                    .clone()
                    .unwrap_or_else(|| gate.kind.to_string()),
                io = if gate.meta.is_input {
                    ",style=filled,color=lightblue"
                } else if gate.meta.is_output {
                    ",style=filled,color=lightgreen"
                } else {
                    ""
                },
            )?;
        }

        writeln!(f)?;

        // The top level module inputs are rendered as nodes
        if depth == 0 {
            for i in 0..self.num_inputs {
                input_map.insert([prefix.clone(), vec![i]].concat(), format!("in{i}"));
                writeln!(f, "{pad}in{i} [style=filled,color=lightblue];")?;
            }

            // Connect the inputs for the root module
            // This happens here rather than up top because the gates didn't exist yet.
            for (i, w) in &self.inputs {
                if let Some(name) = input_map.get(&[prefix.clone(), vec![*i]].concat()) {
                    writeln!(f, "{pad}{name} -> {}:{};", w.gate, w.property)?;
                }
            }
        }

        // Render the subgraphs for sub-modules
        for (i, (name, inputs, module)) in self.sub_modules.iter().enumerate() {
            // Prefix for the submodule's inputs
            let mod_prefix = [prefix.clone(), vec![i]].concat();

            // The input connections to the submodule are scoped based on parent module
            for (input_idx, w) in inputs.iter().enumerate() {
                let node = match w {
                    // This module's input is referencing an input of the parent module
                    CompiledOutput::Input(j) => {
                        match input_map.get(&[prefix.clone(), vec![*j]].concat()) {
                            Some(target) => target.clone(),
                            None => continue,
                        }
                    }
                    CompiledOutput::Immediate(literal) => {
                        let lit_idx = CONST_INDEX.fetch_add(1, atomic::Ordering::SeqCst);
                        let node = format!("lit{lit_idx}");
                        writeln!(
                            f,
                            "{pad}{node} [label=\"{literal}\",style=filled,color=white];"
                        )?;
                        node
                    }
                    CompiledOutput::Wire(w) => format!("{}:{}", w.gate, w.property),
                };

                input_map.insert([mod_prefix.clone(), vec![input_idx]].concat(), node);
            }

            module.subgraph(name, mod_prefix, input_map, f, depth + 1)?;
        }

        // Outputs don't need to be rendered because they are metadata for other
        // nodes to connect to.

        writeln!(f)?;
        for Wire { src, dst } in &self.wires {
            writeln!(
                f,
                "{pad}{}:{} -> {}:{};",
                src.gate, src.property, dst.gate, dst.property
            )?;
        }

        writeln!(f)?;
        for (wc, literal) in &self.gate_literals {
            let lit_idx = CONST_INDEX.fetch_add(1, atomic::Ordering::SeqCst);
            let lit = format!("lit{lit_idx}");
            writeln!(
                f,
                "{pad}{lit} [label=\"{literal}\",style=filled,color=white];"
            )?;
            writeln!(f, "{pad}{lit} -> {}:{};", wc.gate, wc.property)?;
        }

        Ok(writeln!(f, "{root_pad}}}")?)
    }
}

impl Display for CompiledModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CompiledModule with {} inputs", self.num_inputs)?;
        if !self.inputs.is_empty() {
            f.write_str("\nInputs:\n")?;
            for (i, conn) in &self.inputs {
                write!(f, " - [{i}] -> {conn}\n")?;
            }
        }
        if !self.outputs.is_empty() {
            f.write_str("\nOutputs:\n")?;
            for conn in &self.outputs {
                write!(f, " - {conn}\n")?;
            }
        }
        if !self.gates.is_empty() {
            f.write_str("\nGates:\n")?;
            for gate in &self.gates {
                write!(f, " - {gate}\n")?;
            }
        }
        if !self.gate_literals.is_empty() {
            f.write_str("\nGate Literals:\n")?;
            for (conn, lit) in &self.gate_literals {
                write!(f, "   {} = {}\n", conn, lit)?;
            }
        }
        if !self.wires.is_empty() {
            f.write_str("\nWires:\n")?;
            for wire in &self.wires {
                write!(f, "  {} -> {}\n", wire.src, wire.dst)?;
            }
        }
        Ok(())
    }
}
