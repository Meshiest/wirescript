use std::sync::{Arc, atomic};

#[derive(Clone)]
pub struct WireConnection {
    pub gate: Arc<Gate>,
    pub property: String,
}

#[derive(Clone)]
pub struct Wire {
    pub src: WireConnection,
    pub dst: WireConnection,
}

#[derive(Clone)]
pub struct Gate {
    pub kind: String,
    pub index: usize,
}

impl Gate {
    fn next_index() -> usize {
        static NEXT_INDEX: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
        NEXT_INDEX.fetch_add(1, atomic::Ordering::SeqCst)
    }

    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            index: Gate::next_index(),
        }
    }
}
