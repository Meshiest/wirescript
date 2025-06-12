use std::{
    collections::HashMap,
    sync::{Arc, atomic},
};

use crate::ast::{BinaryOpCode, UnaryOpCode};

#[derive(Clone)]
pub struct WireConnection {
    pub gate: Arc<Gate>,
    pub property: String,
}

impl WireConnection {
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

#[derive(Clone)]
pub struct Wire {
    pub src: WireConnection,
    pub dst: WireConnection,
}

#[derive(Clone)]
pub enum GateKind {
    BinaryOp(BinaryOpCode),
    UnaryOp(UnaryOpCode),
}

#[derive(Clone)]
pub struct Gate {
    pub kind: GateKind,
    pub index: usize,
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
