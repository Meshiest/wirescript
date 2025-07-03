use std::{
    collections::HashMap,
    fmt::Display,
    sync::{Arc, atomic},
};

use crate::{
    ast::{BinaryOpCode, UnaryOpCode},
    compiler::{CompiledModule, WireConnection},
};
use brdb::{
    self, BString, BrickType,
    assets::components::{BufferTicks, LogicGate, Rerouter},
    schema::as_brdb::AsBrdbValue,
};

#[derive(Clone, Debug)]
pub enum GateKind {
    Buffer,
    Reroute,
    BinaryOp(BinaryOpCode),
    UnaryOp(UnaryOpCode),
    Blend,
    Ceil,
    Floor,
    EdgeDetector,
}

impl Display for GateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateKind::Buffer => f.write_str("buffer"),
            GateKind::Reroute => f.write_str("rerouter"),
            GateKind::BinaryOp(op) => op.fmt(f),
            GateKind::UnaryOp(op) => op.fmt(f),
            GateKind::Blend => f.write_str("blend"),
            GateKind::Ceil => f.write_str("ceil"),
            GateKind::Floor => f.write_str("floor"),
            GateKind::EdgeDetector => f.write_str("edge_detector"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GateMeta {
    pub input_index: Option<usize>,
    pub output_index: Option<usize>,
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
    pub fn next_index() -> usize {
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

    pub fn with_input(mut self, i: usize) -> Self {
        self.meta.input_index = Some(i);
        self
    }

    pub fn with_output(mut self, i: usize) -> Self {
        self.meta.output_index = Some(i);
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
    pub fn properties(&self) -> (Vec<BString>, Vec<BString>) {
        match self {
            GateKind::BinaryOp(BinaryOpCode::BoolAnd)
            | GateKind::BinaryOp(BinaryOpCode::BoolOr)
            | GateKind::BinaryOp(BinaryOpCode::BoolXor)
            | GateKind::BinaryOp(BinaryOpCode::BoolNand)
            | GateKind::BinaryOp(BinaryOpCode::BoolNor) => (
                vec![LogicGate::BOOL_INPUT_A, LogicGate::BOOL_INPUT_B],
                vec![LogicGate::BOOL_OUTPUT],
            ),
            GateKind::BinaryOp(BinaryOpCode::Eq)
            | GateKind::BinaryOp(BinaryOpCode::Neq)
            | GateKind::BinaryOp(BinaryOpCode::Lt)
            | GateKind::BinaryOp(BinaryOpCode::Leq)
            | GateKind::BinaryOp(BinaryOpCode::Gt)
            | GateKind::BinaryOp(BinaryOpCode::Geq) => (
                vec![LogicGate::INPUT_A, LogicGate::INPUT_B],
                vec![LogicGate::BOOL_OUTPUT],
            ),
            GateKind::BinaryOp(_) => (
                vec![LogicGate::INPUT_A, LogicGate::INPUT_B],
                vec![LogicGate::OUTPUT],
            ),
            GateKind::UnaryOp(UnaryOpCode::BoolNot) => {
                (vec![LogicGate::BOOL_INPUT], vec![LogicGate::BOOL_OUTPUT])
            }
            GateKind::Reroute => (vec![Rerouter::INPUT], vec![Rerouter::OUTPUT]),
            GateKind::Buffer => (vec![BufferTicks::INPUT], vec![BufferTicks::OUTPUT]),
            GateKind::UnaryOp(_) => (vec![LogicGate::INPUT], vec![LogicGate::OUTPUT]),
            GateKind::Blend => (
                vec![LogicGate::INPUT_A, LogicGate::INPUT_B, LogicGate::BLEND],
                vec![LogicGate::OUTPUT],
            ),
            GateKind::Ceil => (vec![LogicGate::INPUT], vec![LogicGate::OUTPUT]),
            GateKind::Floor => (vec![LogicGate::INPUT], vec![LogicGate::OUTPUT]),
            GateKind::EdgeDetector => (
                vec![LogicGate::INPUT],
                vec![LogicGate::RISING_EDGE, LogicGate::FALLING_EDGE],
            ),
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

    pub fn gate(&self) -> Option<LogicGate> {
        match self {
            GateKind::Buffer => None,
            GateKind::Reroute => None,
            GateKind::BinaryOp(op) => match op {
                BinaryOpCode::BoolAnd => Some(LogicGate::BoolAnd),
                BinaryOpCode::BoolOr => Some(LogicGate::BoolOr),
                BinaryOpCode::BoolXor => Some(LogicGate::BoolXor),
                BinaryOpCode::BoolNand => Some(LogicGate::BoolNand),
                BinaryOpCode::BoolNor => Some(LogicGate::BoolNor),
                BinaryOpCode::BitAnd => Some(LogicGate::BitAnd),
                BinaryOpCode::BitOr => Some(LogicGate::BitOr),
                BinaryOpCode::BitXor => Some(LogicGate::BitXor),
                BinaryOpCode::BitNand => Some(LogicGate::BitNand),
                BinaryOpCode::BitNor => Some(LogicGate::BitNor),
                BinaryOpCode::BitShiftLeft => Some(LogicGate::BitShiftLeft),
                BinaryOpCode::BitShiftRight => Some(LogicGate::BitShiftRight),
                BinaryOpCode::Add => Some(LogicGate::Add),
                BinaryOpCode::Sub => Some(LogicGate::Sub),
                BinaryOpCode::Mul => Some(LogicGate::Mul),
                BinaryOpCode::Div => Some(LogicGate::Div),
                BinaryOpCode::Mod => Some(LogicGate::Mod),
                BinaryOpCode::Eq => Some(LogicGate::Eq),
                BinaryOpCode::Neq => Some(LogicGate::Neq),
                BinaryOpCode::Lt => Some(LogicGate::Lt),
                BinaryOpCode::Leq => Some(LogicGate::Leq),
                BinaryOpCode::Gt => Some(LogicGate::Gt),
                BinaryOpCode::Geq => Some(LogicGate::Geq),
            },
            GateKind::UnaryOp(op) => match op {
                UnaryOpCode::BoolNot => Some(LogicGate::BoolNot),
                UnaryOpCode::BitNot => Some(LogicGate::BitNot),
            },
            GateKind::Blend => Some(LogicGate::Blend),
            GateKind::Ceil => Some(LogicGate::Ceil),
            GateKind::Floor => Some(LogicGate::Floor),
            GateKind::EdgeDetector => Some(LogicGate::EdgeDetector),
        }
    }

    pub fn cyclic(&self) -> bool {
        match self {
            GateKind::Buffer => true,
            _ => false,
        }
    }

    pub fn brick(&self) -> BrickType {
        match self {
            GateKind::Buffer => brdb::assets::bricks::B_GATE_BUFFER_TICK,
            GateKind::Reroute => brdb::assets::bricks::B_REROUTE,
            _ => self.gate().unwrap().brick(), // unwrap safety: only reroute and buffer don't have gates
        }
    }

    pub fn component(&self) -> Box<dyn brdb::BrdbComponent> {
        match self {
            GateKind::Buffer => Box::new(brdb::assets::components::BufferTicks::new(0, 0)),
            GateKind::Reroute => Box::new(brdb::assets::components::Rerouter),
            _ => Box::new(self.gate().unwrap().component()), // unwrap safety: only reroute and buffer don't have gates
        }
    }

    pub fn component_with_inputs(
        &self,
        inputs: HashMap<BString, Box<dyn AsBrdbValue>>,
    ) -> Box<dyn brdb::BrdbComponent> {
        match self {
            GateKind::Buffer => Box::new(brdb::assets::components::BufferTicks::new(0, 0)),
            GateKind::Reroute => Box::new(brdb::assets::components::Rerouter),
            _ => Box::new(self.gate().unwrap().component_with_overrides(inputs)), // unwrap safety: only reroute and buffer don't have gates
        }
    }

    pub fn component_name(&self) -> BString {
        match self {
            GateKind::Buffer => brdb::assets::components::BufferTicks::COMPONENT,
            GateKind::Reroute => brdb::assets::components::Rerouter::COMPONENT,
            _ => self.gate().unwrap().component_name(), // unwrap safety: only reroute and buffer don't have gates
        }
    }
}
