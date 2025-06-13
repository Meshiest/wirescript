use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
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
    BinaryOp(BinaryOpCode),
    UnaryOp(UnaryOpCode),
}

impl Display for GateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateKind::Buffer => f.write_str("buffer"),
            GateKind::BinaryOp(op) => op.fmt(f),
            GateKind::UnaryOp(op) => op.fmt(f),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Gate {
    pub kind: GateKind,
    pub index: usize,
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
        }
    }

    pub fn cloned(&self) -> Self {
        Self::new(&self.kind)
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
            GateKind::UnaryOp(_) | GateKind::Buffer => {
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
    pub outputs: Vec<CompiledOutput>,
    /// Wires that connect to gates inside the module
    pub wires: Vec<Wire>,
    /// Gates that are part of this module
    pub gates: Vec<Arc<Gate>>,
    /// Literal values that will be shoved into the gates of this module
    pub gate_literals: Vec<(WireConnection, Literal)>,
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
                .map(|w| w.replace_gate(&gate_lut))
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
        }
    }

    pub fn digraph(&self) -> Result<String, Box<dyn Error>> {
        use std::io::Write;
        let mut f = vec![];

        writeln!(f, "digraph module {{")?;
        for i in 0..self.num_inputs {
            writeln!(f, "in{i} [style=filled,color=lightblue];")?;
        }
        for (i, w) in &self.inputs {
            writeln!(f, "in{i} -> {} [headlabel=\"{}\"];", w.gate, w.property)?;
        }
        for (i, o) in self.outputs.iter().enumerate() {
            writeln!(f, "out{i} [style=filled,color=lightgreen];")?;
            match o {
                CompiledOutput::Input(j) => {
                    writeln!(f, "in{j} -> out{i};")?;
                }
                CompiledOutput::Immediate(literal) => {
                    writeln!(f, "{literal} -> out{i};")?;
                }
                CompiledOutput::Wire(w) => {
                    writeln!(f, "{} -> out{i} [taillabel=\"{}\"];", w.gate, w.property)?;
                }
            }
        }
        for Wire { src, dst } in &self.wires {
            writeln!(
                f,
                "{} -> {} [taillabel=\"{}\",headlabel=\"{}\"];",
                src.gate, dst.gate, src.property, dst.property
            )?;
        }
        for (wc, lit) in &self.gate_literals {
            writeln!(f, "{lit} -> {} [headlabel=\"{}\"];", wc.gate, wc.property)?;
        }

        writeln!(f, "}}")?;

        Ok(String::from_utf8(f)?)
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
