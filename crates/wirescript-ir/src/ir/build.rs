use crate::collections::HashMap;
use std::sync::Arc;

use super::gate_class as gc;
use crate::diagnostic::SourceRange;
use crate::intern::{Sym, sym};
use crate::ir::port_registry::WirePort;
use crate::ir::{
    GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, ROOT_SCOPE_ID, ScopeId, Type, Wire,
};

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

    pub fn add_gate(&mut self, opts: AddNodeOpts) -> NodeId {
        self.add_node(NodeKind::Gate, opts)
    }
    pub fn add_event(&mut self, opts: AddNodeOpts) -> NodeId {
        self.add_node(NodeKind::Event, opts)
    }
    pub fn add_input(&mut self, port_name: &str, ty: Type, source_range: SourceRange) -> NodeId {
        self.add_pin(NodeKind::Input, port_name, ty, source_range)
    }
    pub fn add_output(&mut self, port_name: &str, ty: Type, source_range: SourceRange) -> NodeId {
        self.add_pin(NodeKind::Output, port_name, ty, source_range)
    }

    /// A chip pin of `kind` (`Input` or `Output`) carrying `ty`, labelled
    /// `port_name` and appended to the module's input or output list.
    pub fn add_pin(
        &mut self,
        kind: NodeKind,
        port_name: &str,
        ty: Type,
        source_range: SourceRange,
    ) -> NodeId {
        self.add_pin_with_id(NodeId::fresh(), kind, port_name, ty, source_range)
    }

    /// [`ModuleBuilder::add_pin`] under an id the caller chose.
    pub fn add_pin_with_id(
        &mut self,
        id: NodeId,
        kind: NodeKind,
        port_name: &str,
        ty: Type,
        source_range: SourceRange,
    ) -> NodeId {
        let mut properties = HashMap::default();
        properties.insert(*sym::PORT_LABEL, Literal::String(port_name.into()));
        let gate_class = if kind == NodeKind::Input {
            gc::MICROCHIP_INPUT
        } else {
            gc::MICROCHIP_OUTPUT
        };
        let opts = AddNodeOpts {
            gate_class,
            source_range,
            ports: GateIO::pin(ty),
            properties,
            ..Default::default()
        };
        self.add_node_with_id(id, kind, opts);
        if kind == NodeKind::Input {
            self.module.inputs.push(id);
        } else {
            self.module.outputs.push(id);
        }
        id
    }

    pub fn connect(&mut self, source: PortRef, target: PortRef) {
        self.module.wires.push(Wire { source, target });
    }

    fn add_node(&mut self, kind: NodeKind, opts: AddNodeOpts) -> NodeId {
        self.add_node_with_id(NodeId::fresh(), kind, opts)
    }

    /// Insert a node under an id the caller chose.
    pub fn add_node_with_id(&mut self, id: NodeId, kind: NodeKind, opts: AddNodeOpts) -> NodeId {
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

#[derive(Clone, Debug, Default)]
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

pub fn port_ref(node_id: NodeId, port_name: &str) -> PortRef {
    PortRef {
        node_id,
        port: WirePort::from_name(port_name),
    }
}
