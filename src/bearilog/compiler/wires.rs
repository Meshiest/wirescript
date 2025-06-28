use std::{collections::HashMap, fmt::Display, sync::Arc};

use super::Gate;

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
