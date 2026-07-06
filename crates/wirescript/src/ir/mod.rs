//! Intermediate representation.
//!
//! A [`Module`] is a graph: nodes (gate instances) + wires (port-to-port
//! connections). Nested chips are child Modules addressed by the parent's
//! chip-node id. Everything is flat + explicit so the IR is trivially
//! serialisable and round-trips through test goldens.
//!
//! reference implementation. Types module is fused in here (see `Type`
//! enum) since Phase 1 doesn't need unification or tvars — those arrive
//! in the typecheck phase.

pub mod build;
pub mod gate_class;
pub mod port_registry;

use std::collections::HashMap;

use crate::intern::Sym;
use crate::ir::port_registry::WirePort;

/// Cheap numeric node identity — every node gets a globally unique u32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn fresh() -> Self {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        Self(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    pub fn port(self, port: WirePort) -> PortRef {
        PortRef {
            node_id: self,
            port,
        }
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "n{}", self.0)
    }
}

/// Lexical scope id. A module seeds `ScopeId(0)` as its root; every
/// subsequent scope (handler body, chip body, if branches, etc.) gets a
/// fresh id. Nodes record which scope they were lowered under via
/// `Node.scope_id`, and layout uses the `Module.scopes` table to group
/// them into nested regions.
pub type ScopeId = u32;

/// Root scope of every `Module`. Reserved; `Module::new` always seeds it.
pub const ROOT_SCOPE_ID: ScopeId = 0;

/// Kind of a lexical scope, with per-kind payload inlined so layout can
/// switch on the whole scope without consulting sibling `Option` fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    /// Top-level of a non-chip `Module`.
    ModuleRoot,
    /// Top-level of a chip's interior `Module`.
    ChipBody { name: String },
    /// Body of a `handler Trigger { ... }` declaration.
    HandlerBody { trigger_label: String },
    /// Body of a `fn name(...) { ... }` declaration.
    FnBody { name: String },
    /// Synthetic wrapper around the three sub-scopes of an `if` so compose
    /// sees branches as a unit. Exactly one of `IfCond`/`IfThen`/`IfElse`
    /// children may lack a paired sibling, but they are always wrapped.
    IfGroup,
    /// `if (cond)` — the expression's lowered nodes live here.
    IfCond,
    /// `then { ... }` block.
    IfThen,
    /// `else { ... }` block.
    IfElse,
    /// Body of a loop (for/while/etc. — placeholder for future loop forms).
    LoopBody,
    /// A generic `{ ... }` block with no special semantics.
    Block,
}

#[derive(Clone, Debug)]
pub struct ScopeInfo {
    pub kind: ScopeKind,
    pub source_range: SourceRange,
    pub parent: Option<ScopeId>,
}

/// Primitive types tracked on ports + wires for compatibility checks.
/// A subset of the TS `Type` ADT — enough for Phase 1 emit + layout.
/// Extended with `Ref` / `Array` / etc. in Phase 3 (typecheck).
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Bool,
    Int,
    Float,
    String,
    Vector,
    Rotator,
    Quat,
    Color,
    Entity,
    Character,
    Controller,
    Brick,
    Prefab,
    Exec,
    Any,
    Never,
    Ref(Box<Type>),
    Array(Box<Type>),
    Union(Vec<Type>),
    Tuple(Vec<Type>),
    /// Record / struct; Phase 1 keeps this untyped inner (map of name→Type).
    Record(Vec<(String, Type)>),
}

/// A literal used to initialise non-wired component properties.
///
/// Mirrors the TS `Literal` discriminated union 1:1. Phase 1 lowers these
/// straight to brdb values in the emitter.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Vector {
        x: f64,
        y: f64,
        z: f64,
    },
    Rotator {
        pitch: f64,
        yaw: f64,
        roll: f64,
    },
    Quat {
        x: f64,
        y: f64,
        z: f64,
        w: f64,
    },
    Color {
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Linear RGBA color (0–1 components) — the wire `LinearColor` variant
    /// member, produced by folding a constant `Color(r, g, b, a?)` call.
    LinearColor {
        r: f64,
        g: f64,
        b: f64,
        a: f64,
    },
    /// Null object reference — used for entity/controller/character var defaults.
    Object,
    /// A list of literals — used to carry an array's constant initial contents
    /// (`array foo: int[] = [1, 2, 3]`) to the emitter.
    Array(Vec<Literal>),
    /// External asset reference `$AssetType/AssetName`. The emitter registers it
    /// in the world's external-asset table and writes the resulting index.
    Asset {
        asset_type: String,
        asset_name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortSpec {
    pub name: Sym,
    pub ty: Type,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GateIO {
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
}

impl GateIO {
    pub fn all_port_names(&self) -> impl Iterator<Item = Sym> + '_ {
        self.inputs
            .iter()
            .chain(self.outputs.iter())
            .map(|p| p.name)
    }

    pub fn find_input(&self, name: Sym) -> Option<&PortSpec> {
        self.inputs.iter().find(|p| p.name == name)
    }

    pub fn find_output(&self, name: Sym) -> Option<&PortSpec> {
        self.outputs.iter().find(|p| p.name == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Gate,
    Event,
    Chip,
    Input,
    Output,
    /// Type-coercion adapter. Emitted as a real brick (e.g. `Exec_Controller_GetFromEntity`
    /// when a character-typed wire is routed into a controller-typed port).
    Coerce,
}

pub use crate::diagnostic::SourceRange;

use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Component class of the gate brick this IR node spawns.
    /// For `Chip` nodes this is `BrickComponentType_Internal_Microchip`; for
    /// `Input` / `Output` inside a chip body it's `...MicrochipInput` /
    /// `...MicrochipOutput`.
    pub gate_class: &'static str,
    /// Non-wired component values applied after the brick is spawned.
    /// Wrapped in Arc so template instantiation can clone without
    /// copying the entire map.
    pub properties: Arc<HashMap<Sym, Literal>>,
    /// Port specifications. Arc-shared across template instances.
    pub ports: Arc<GateIO>,
    pub source_range: SourceRange,
    /// For nodes living inside a chip body: the chip node's id.
    ///
    /// TODO(follow-up PR): this is redundant with `ScopeKind::ChipBody`
    /// walking `scope_id → scopes[id] → parent` in `Module.scopes`.
    /// Remove once no downstream code reads it.
    pub chip_id: Option<NodeId>,
    /// Layout-row hint. Nodes sharing a `chain_id` render in the same row
    /// (one exec chain = one row). `None` means "global" (row 0).
    pub chain_id: Option<u32>,
    /// Lexical scope this node was lowered under. Resolved against
    /// `Module.scopes`. Defaults to `ROOT_SCOPE_ID` for nodes built before
    /// the lowering pass assigns scopes.
    pub scope_id: ScopeId,
    /// Debug-only human-readable note.
    pub note: Option<&'static str>,
}

impl Node {
    /// True if this node is a stateful buffer/Var that legitimately
    /// participates in a cycle. Layout uses this to pick a deterministic
    /// feedback edge when breaking SCCs.
    ///
    /// New buffer-capable gate classes go here — this is the single
    /// source of truth consulted by layout.
    pub fn is_buffer(&self) -> bool {
        self.gate_class == gate_class::VARIABLE || self.gate_class == gate_class::BUFFER
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub node_id: NodeId,
    pub port: WirePort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wire {
    pub source: PortRef,
    pub target: PortRef,
}

/// A compiled wirescript module — the input to the emitter.
#[derive(Debug, Clone)]
pub struct Module {
    pub name: Sym,
    pub nodes: HashMap<NodeId, Node>,
    pub wires: Vec<Wire>,
    /// Chip sub-modules keyed by the chip node's id in this Module.
    pub chips: HashMap<NodeId, Module>,
    /// MicrochipInput node ids in declaration order.
    pub inputs: Vec<NodeId>,
    /// MicrochipOutput node ids in declaration order.
    pub outputs: Vec<NodeId>,
    /// Scope table for layout. `ROOT_SCOPE_ID` is always present and is
    /// this module's root (`ModuleRoot` by default; chip-body modules
    /// overwrite it with `ChipBody { name }` at construction time).
    pub scopes: HashMap<ScopeId, ScopeInfo>,
    /// Base template name for grid deduplication. When set, identical chip
    /// instances (same template_key) can reuse the first instance's
    /// serialized grid data instead of emitting duplicate grids.
    pub template_key: Option<Sym>,
    /// External node_ids referenced by this module's wires but NOT in its
    /// `nodes` map. Discovered by bottom-up diff after lowering. Used by
    /// template instantiation to remap external refs per instance.
    pub scope_captures: Vec<NodeId>,
}

impl Default for Module {
    fn default() -> Self {
        let mut scopes = HashMap::new();
        scopes.insert(
            ROOT_SCOPE_ID,
            ScopeInfo {
                kind: ScopeKind::ModuleRoot,
                source_range: SourceRange::default(),
                parent: None,
            },
        );
        Self {
            name: crate::intern::intern_static(""),
            nodes: HashMap::new(),
            wires: Vec::new(),
            chips: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            scopes,
            template_key: None,
            scope_captures: Vec::new(),
        }
    }
}

impl Module {
    pub fn new(name: &str) -> Self {
        Self {
            name: crate::intern::intern(name),
            ..Default::default()
        }
    }

    /// Construct a module whose root scope is a chip body. The root is
    /// still `ROOT_SCOPE_ID`; its `ScopeInfo` is overwritten to
    /// `ChipBody { name }`.
    pub fn new_chip_body(module_name: &str, chip_name: impl Into<String>) -> Self {
        let mut m = Self::new(module_name);
        m.scopes.insert(
            ROOT_SCOPE_ID,
            ScopeInfo {
                kind: ScopeKind::ChipBody {
                    name: chip_name.into(),
                },
                source_range: SourceRange::default(),
                parent: None,
            },
        );
        m
    }

    /// Insert a node; returns its id so the caller can wire against it.
    pub fn add_node(&mut self, n: Node) -> NodeId {
        let id = n.id;
        self.nodes.insert(id, n);
        id
    }

    pub fn add_wire(&mut self, w: Wire) {
        self.wires.push(w);
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }
}

pub fn dump_module(module: &Module, indent: usize) {
    dump_module_with_source(module, indent, None);
}

pub fn dump_module_with_source(module: &Module, indent: usize, source: Option<&str>) {
    use crate::intern::resolve;
    let pad = "  ".repeat(indent);
    eprintln!(
        "{pad}module '{}' ({} nodes, {} wires, {} chips)",
        resolve(module.name),
        module.nodes.len(),
        module.wires.len(),
        module.chips.len()
    );
    let mut ids: Vec<&NodeId> = module.nodes.keys().collect();
    ids.sort_by_key(|id| id.0);
    for id in ids {
        let n = &module.nodes[id];
        let chip_tag = n.chip_id.map(|c| format!(" chip={c}")).unwrap_or_default();
        let note_tag = n
            .note
            .as_deref()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        let sr = &n.source_range;
        let loc = if sr.start.line > 0 || sr.start.col > 0 {
            format!(" @ {}:{}", sr.start.line, sr.start.col)
        } else {
            String::new()
        };
        let snippet = source
            .and_then(|s| s.get(sr.start.offset..sr.end.offset.min(sr.start.offset + 40)))
            .map(|s| {
                let s = s.split('\n').next().unwrap_or(s);
                format!(" `{s}`")
            })
            .unwrap_or_default();
        eprintln!(
            "{pad}  [{:?}] {id} ({}){chip_tag}{note_tag}{loc}{snippet}",
            n.kind, n.gate_class
        );
    }
    for w in &module.wires {
        eprintln!(
            "{pad}  wire: {}.{} -> {}.{}",
            w.source.node_id, w.source.port, w.target.node_id, w.target.port
        );
    }
    for (chip_id, child) in &module.chips {
        eprintln!("{pad}  chip {chip_id}:");
        dump_module_with_source(child, indent + 2, source);
    }
}
