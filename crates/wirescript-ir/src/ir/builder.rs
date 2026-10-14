//! Build a [`Program`] gate by gate, for a producer other than the Wirescript
//! front end.
//!
//! A thin layer over the [`ModuleBuilder`] lowering uses. It checks each gate
//! class against the inventory and each wire against the ports of its ends,
//! and fills everything else the way the `.wsir` parser does, so a program
//! built here and the same program parsed from text are the same IR.
//!
//! ```
//! use wirescript_ir::ir::{ProgramBuilder, Type};
//!
//! let mut b = ProgramBuilder::new("and");
//! let root = b.root();
//! let a = b.input(root, "a", Type::Bool);
//! let c = b.input(root, "b", Type::Bool);
//! let g = b.gate(root, "Expr_LogicalAND").unwrap();
//! let y = b.output(root, "y", Type::Bool);
//! b.wire(a, "RER_Output", g, "bInputA").unwrap();
//! b.wire(c, "RER_Output", g, "bInputB").unwrap();
//! b.wire(g, "bOutput", y, "RER_Input").unwrap();
//! let program = b.build();
//! assert!(program.validate().is_empty());
//! ```

use std::sync::Arc;

use super::build::{AddNodeOpts, ModuleBuilder};
use super::module_tree::{ModuleTree, nest_bodies};
use super::port_registry::WirePort;
use super::{GateIO, Literal, NodeId, NodeKind, PortRef, PortSide, Program, ProgramSettings, Type, Wire};
use crate::collections::HashMap;
use crate::intern::{intern, resolve};
use crate::wsir::defaults;

/// A module of the program being built: the root, or a chip's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModuleRef(usize);

/// Why a builder call was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BuildError {
    #[error("`{0}` is not a gate class in the inventory")]
    UnknownClass(String),
    #[error("{0} is not a node of this program")]
    UnknownNode(NodeId),
    #[error("{node} ({class}) has no {side} port `{port}`; its {side} ports are: {}", super::validate::port_list(.valid))]
    UnknownPort { node: NodeId, class: String, side: PortSide, port: String, valid: Vec<String> },
    #[error("{node}.{port} already has a wire into it, from {from}")]
    FanIn { node: NodeId, port: String, from: String },
}

/// Builds a [`Program`]. Node ids count up from 1 in creation order.
pub struct ProgramBuilder {
    modules: Vec<ModuleBuilder>,
    tree: ModuleTree,
    /// Chip node -> the module that is its body.
    chips: HashMap<NodeId, usize>,
    /// Node -> the module it is in.
    home: HashMap<NodeId, usize>,
    /// Each wired input -> the output feeding it.
    fed: HashMap<PortRef, PortRef>,
    next_id: u32,
    settings: ProgramSettings,
}

impl ProgramBuilder {
    /// A program whose root module is called `name`.
    pub fn new(name: &str) -> Self {
        Self {
            modules: vec![ModuleBuilder::new(name)],
            tree: ModuleTree::new(),
            chips: HashMap::default(),
            home: HashMap::default(),
            fed: HashMap::default(),
            next_id: 1,
            settings: ProgramSettings::default(),
        }
    }

    /// The root module.
    pub fn root(&self) -> ModuleRef {
        ModuleRef(0)
    }

    /// The program's settings (layout engine, doc, ...).
    pub fn settings_mut(&mut self) -> &mut ProgramSettings {
        &mut self.settings
    }

    fn take_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn add(&mut self, m: ModuleRef, kind: NodeKind, opts: AddNodeOpts) -> NodeId {
        let id = self.take_id();
        self.modules[m.0].add_node_with_id(id, kind, opts);
        self.home.insert(id, m.0);
        id
    }

    fn pin(&mut self, m: ModuleRef, name: &str, ty: Type, kind: NodeKind) -> NodeId {
        let id = self.take_id();
        self.modules[m.0].add_pin_with_id(id, kind, name, ty, Default::default());
        self.home.insert(id, m.0);
        self.name(id, name);
        id
    }

    /// An input of module `m` carrying `ty`, labelled and named `name`. On a
    /// chip's body, wire into its `RER_Input` from the parent to feed it.
    pub fn input(&mut self, m: ModuleRef, name: &str, ty: Type) -> NodeId {
        self.pin(m, name, ty, NodeKind::Input)
    }

    /// An output of module `m` carrying `ty`, labelled and named `name`.
    pub fn output(&mut self, m: ModuleRef, name: &str, ty: Type) -> NodeId {
        self.pin(m, name, ty, NodeKind::Output)
    }

    /// A gate of inventory class `class` (with or without its
    /// `BrickComponentType_` prefix), with the inventory's ports.
    pub fn gate(&mut self, m: ModuleRef, class: &str) -> Result<NodeId, BuildError> {
        let class = defaults::resolve_class(class);
        if crate::catalog::default_catalog().find_by_class(class).is_none() {
            return Err(BuildError::UnknownClass(class.to_string()));
        }
        let ports = defaults::default_ports(class, None);
        Ok(self.add(m, defaults::default_kind(class), AddNodeOpts { gate_class: class, ports, ..Default::default() }))
    }

    /// A gate of any class with the ports given: a pseudo-class such as
    /// `_Literal`, or ports typed more narrowly than the inventory's.
    pub fn gate_with_ports(&mut self, m: ModuleRef, class: &str, ports: GateIO) -> NodeId {
        let class = defaults::resolve_class(class);
        self.add(m, defaults::default_kind(class), AddNodeOpts { gate_class: class, ports, ..Default::default() })
    }

    /// A chip in module `m`: its node, and the module that is its body.
    pub fn chip(&mut self, m: ModuleRef) -> (NodeId, ModuleRef) {
        let id = self.add(m, NodeKind::Chip, AddNodeOpts { gate_class: defaults::CHIP_CLASS, ..Default::default() });
        let body = self.tree.push(m.0);
        self.modules.push(ModuleBuilder::new(&defaults::default_chip_module_name(id.0)));
        self.chips.insert(id, body);
        (id, ModuleRef(body))
    }

    /// Give `node` a name the text form writes it by.
    pub fn name(&mut self, node: NodeId, name: &str) {
        if let Some(&m) = self.home.get(&node) {
            self.modules[m].module.value_names.insert(node, intern(name));
        }
    }

    fn node_mut(&mut self, node: NodeId) -> Result<&mut super::Node, BuildError> {
        let m = *self.home.get(&node).ok_or(BuildError::UnknownNode(node))?;
        Ok(self.modules[m].module.nodes.get_mut(&node).expect("home lists only added nodes"))
    }

    fn node(&self, node: NodeId) -> Result<&super::Node, BuildError> {
        let m = *self.home.get(&node).ok_or(BuildError::UnknownNode(node))?;
        Ok(&self.modules[m].module.nodes[&node])
    }

    /// Set a property: a setting of the gate, or the value baked into an
    /// unwired input of that name.
    pub fn set(&mut self, node: NodeId, property: &str, value: Literal) -> Result<(), BuildError> {
        let n = self.node_mut(node)?;
        Arc::make_mut(&mut n.properties).insert(intern(property), value);
        Ok(())
    }

    /// Wire output `from_port` of `from` into input `to_port` of `to`. Both
    /// ports must exist, and an input takes one wire. An input typed `any`
    /// takes the type of what feeds it when the program is built, as in the
    /// text form.
    pub fn wire(&mut self, from: NodeId, from_port: &str, to: NodeId, to_port: &str) -> Result<(), BuildError> {
        let unknown = |n: &super::Node, side: PortSide, port: &str| {
            let ports = if side == PortSide::Input { &n.ports.inputs } else { &n.ports.outputs };
            BuildError::UnknownPort {
                node: n.id,
                class: n.gate_class.to_string(),
                side,
                port: port.to_string(),
                valid: ports.iter().map(|p| resolve(p.name).to_string()).collect(),
            }
        };
        let src = self.node(from)?;
        if !src.ports.outputs.iter().any(|p| resolve(p.name) == from_port) {
            return Err(unknown(src, PortSide::Output, from_port));
        }
        let source_port = WirePort::try_from_name(from_port).ok_or_else(|| unknown(src, PortSide::Output, from_port))?;
        let dst = self.node(to)?;
        if !dst.ports.inputs.iter().any(|p| resolve(p.name) == to_port) {
            return Err(unknown(dst, PortSide::Input, to_port));
        }
        let target_port = WirePort::try_from_name(to_port).ok_or_else(|| unknown(dst, PortSide::Input, to_port))?;
        let target = PortRef { node_id: to, port: target_port };
        if let Some(prev) = self.fed.get(&target) {
            return Err(BuildError::FanIn {
                node: to,
                port: to_port.to_string(),
                from: format!("{}.{}", prev.node_id, prev.port),
            });
        }
        let source = PortRef { node_id: from, port: source_port };
        self.fed.insert(target, source);
        let home = self.tree.common_module(self.home[&from], self.home[&to]);
        self.modules[home].connect(source, target);
        Ok(())
    }

    /// A layout-only edge: `after` is placed after `before`. Not a wire.
    pub fn layout_after(&mut self, before: NodeId, after: NodeId) -> Result<(), BuildError> {
        self.node(before)?;
        self.node(after)?;
        let home = self.tree.common_module(self.home[&before], self.home[&after]);
        self.modules[home].module.wires.push(Wire {
            source: PortRef { node_id: before, port: WirePort::Layout },
            target: PortRef { node_id: after, port: WirePort::Layout },
        });
        Ok(())
    }

    /// Type what the text form leaves to inference, in the parser's order:
    /// each pin made with type `any` takes, on both sides, the type of what
    /// feeds its `RER_Input`, until no pin changes; then each `any` input of
    /// any other node takes the type of the output wired into it.
    fn infer_types(&mut self) {
        let mut wires: Vec<(PortRef, PortRef)> = self.fed.iter().map(|(t, s)| (*t, *s)).collect();
        wires.sort_by_key(|(t, _)| (t.node_id, t.port.as_str()));
        let out_ty = |b: &Self, p: PortRef| {
            b.node(p.node_id).ok()?.ports.outputs.iter().find(|o| o.name == p.port.sym()).map(|o| o.ty.clone())
        };
        let any_pin = GateIO::pin(Type::Any);
        let is_pin = |n: &super::Node| matches!(n.kind, NodeKind::Input | NodeKind::Output);
        loop {
            let mut changed = false;
            for (target, source) in &wires {
                let Ok(n) = self.node(target.node_id) else { continue };
                if target.port != WirePort::RerInput || !is_pin(n) || *n.ports != any_pin {
                    continue;
                }
                let Some(ty) = out_ty(self, *source).filter(|t| *t != Type::Any) else { continue };
                self.node_mut(target.node_id).expect("found above").ports = Arc::new(GateIO::pin(ty));
                changed = true;
            }
            if !changed {
                break;
            }
        }
        for (target, source) in &wires {
            let Ok(n) = self.node(target.node_id) else { continue };
            if is_pin(n) {
                continue;
            }
            let Some(ty) = out_ty(self, *source) else { continue };
            let node = self.node_mut(target.node_id).expect("found above");
            defaults::infer_any_input(Arc::make_mut(&mut node.ports), target.port.sym(), ty);
        }
    }

    /// The finished program.
    pub fn build(mut self) -> Program {
        self.infer_types();
        let bodies = self.chips.iter().map(|(&chip, &body)| (chip, body, self.home[&chip])).collect();
        let module = nest_bodies(self.modules.into_iter().map(|b| b.module).collect(), bodies);
        Program { module, settings: self.settings }
    }
}
