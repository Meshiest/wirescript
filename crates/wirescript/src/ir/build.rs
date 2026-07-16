use crate::collections::HashMap;
use std::sync::Arc;

use crate::diagnostic::SourceRange;
use crate::intern::{Sym, sym};
use crate::ir::port_registry::WirePort;
use crate::ir::{
    GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, ROOT_SCOPE_ID, ScopeId,
    Type, Wire,
};
use super::gate_class as gc;

#[derive(Default)]
pub struct IdAllocator;

impl IdAllocator {
    pub fn fresh(&mut self, _base_path: &str) -> NodeId {
        NodeId::fresh()
    }
}

/// Build a `/`-joined path id from segments. Each segment is sanitised
/// to `[A-Za-z0-9_.-]`; any other character becomes `_`.
pub fn path_id(segments: &[&str]) -> String {
    segments
        .iter()
        .map(|s| {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub struct ModuleBuilder {
    pub module: Module,
    pub current_chain_id: Option<u32>,
    /// Scope currently being lowered under. Copied onto every node the
    /// builder emits unless `AddNodeOpts.scope_id` overrides it.
    pub current_scope_id: ScopeId,
}

impl ModuleBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            module: Module::new(name),
            current_chain_id: None,
            current_scope_id: ROOT_SCOPE_ID,
        }
    }

    pub fn add_gate(&mut self, ids: &mut IdAllocator, opts: AddNodeOpts) -> NodeId {
        self.add_node(ids, NodeKind::Gate, opts)
    }
    pub fn add_event(&mut self, ids: &mut IdAllocator, opts: AddNodeOpts) -> NodeId {
        self.add_node(ids, NodeKind::Event, opts)
    }
    pub fn add_input(
        &mut self,
        ids: &mut IdAllocator,
        port_name: &str,
        ty: Type,
        source_range: SourceRange,
    ) -> NodeId {
        let mut props = HashMap::default();
        props.insert(*sym::PORT_LABEL, Literal::String(port_name.into()));
        let id = self.add_node(
            ids,
            NodeKind::Input,
            AddNodeOpts {
                gate_class: gc::MICROCHIP_INPUT,
                source_range,
                ports: GateIO {
                    inputs: vec![PortSpec { name: *sym::RER_INPUT, ty: ty.clone() }],
                    outputs: vec![PortSpec { name: *sym::RER_OUTPUT, ty }],
                },
                properties: props,
                chip_id: None,
                chain_id: None,
                scope_id: None,
                note: None,
            },
        );
        self.module.inputs.push(id);
        id
    }
    pub fn add_output(
        &mut self,
        ids: &mut IdAllocator,
        port_name: &str,
        ty: Type,
        source_range: SourceRange,
    ) -> NodeId {
        let mut props = HashMap::default();
        props.insert(*sym::PORT_LABEL, Literal::String(port_name.into()));
        let id = self.add_node(
            ids,
            NodeKind::Output,
            AddNodeOpts {
                gate_class: gc::MICROCHIP_OUTPUT,
                source_range,
                ports: GateIO {
                    inputs: vec![PortSpec { name: *sym::RER_INPUT, ty: ty.clone() }],
                    outputs: vec![PortSpec { name: *sym::RER_OUTPUT, ty }],
                },
                properties: props,
                chip_id: None,
                chain_id: None,
                scope_id: None,
                note: None,
            },
        );
        self.module.outputs.push(id);
        id
    }

    pub fn connect(&mut self, source: PortRef, target: PortRef) {
        self.module.wires.push(Wire { source, target });
    }

    fn add_node(&mut self, _ids: &mut IdAllocator, kind: NodeKind, opts: AddNodeOpts) -> NodeId {
        let id = NodeId::fresh();
        let chain_id = opts.chain_id.or(self.current_chain_id);
        let scope_id = opts.scope_id.unwrap_or(self.current_scope_id);
        let note = opts.note;
        let node = Node {
            id,
            kind,
            gate_class: opts.gate_class,
            properties: shared_properties(opts.properties),
            ports: shared_gate_io(opts.ports),
            source_range: opts.source_range,
            chip_id: opts.chip_id,
            chain_id,
            scope_id,
            note,
        };
        self.module.nodes.insert(id, node);
        id
    }
}

/// Most nodes carry no properties — share one immutable empty map instead of
/// allocating an `Arc<HashMap>` per node. (`Arc::make_mut` copies on write.)
fn shared_properties(props: HashMap<Sym, Literal>) -> Arc<HashMap<Sym, Literal>> {
    static EMPTY: std::sync::LazyLock<Arc<HashMap<Sym, Literal>>> =
        std::sync::LazyLock::new(|| Arc::new(HashMap::default()));
    if props.is_empty() {
        EMPTY.clone()
    } else {
        Arc::new(props)
    }
}

/// Hash-cons `GateIO`s: gates of the same class/type signature repeat the
/// same port table thousands of times, so share one `Arc` per distinct value.
/// Node ports are never mutated after construction, so sharing is safe.
fn shared_gate_io(io: GateIO) -> Arc<GateIO> {
    use std::sync::Mutex;
    static CACHE: std::sync::LazyLock<Mutex<crate::collections::HashSet<Arc<GateIO>>>> =
        std::sync::LazyLock::new(|| Mutex::new(crate::collections::HashSet::default()));
    let mut cache = CACHE.lock().unwrap();
    if let Some(existing) = cache.get(&io) {
        return existing.clone();
    }
    let arc = Arc::new(io);
    cache.insert(arc.clone());
    arc
}

#[derive(Clone, Debug)]
pub struct AddNodeOpts {
    pub gate_class: &'static str,
    pub source_range: SourceRange,
    pub ports: GateIO,
    pub properties: HashMap<Sym, Literal>,
    pub chip_id: Option<NodeId>,
    pub chain_id: Option<u32>,
    /// Override the builder's `current_scope_id`. `None` (the default)
    /// means "use whatever scope the lowering pass is currently in".
    pub scope_id: Option<ScopeId>,
    pub note: Option<&'static str>,
}

impl Default for AddNodeOpts {
    fn default() -> Self {
        Self {
            gate_class: "",
            source_range: SourceRange::default(),
            ports: GateIO::default(),
            properties: HashMap::default(),
            chip_id: None,
            chain_id: None,
            scope_id: None,
            note: None,
        }
    }
}

/// Build a `PortRef` from a node ID and a port name string.
/// Looks up the port name in the port registry to get the `WirePort`.
pub fn port_ref(node_id: NodeId, port_name: &str) -> PortRef {
    PortRef {
        node_id,
        port: WirePort::from_name(port_name),
    }
}
