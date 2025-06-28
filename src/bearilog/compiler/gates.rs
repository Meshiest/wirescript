use std::{
    fmt::Display,
    sync::{Arc, atomic},
};

use crate::bearilog::{
    ast::{BinaryOpCode, UnaryOpCode},
    compiler::{CompiledModule, WireConnection},
};

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
