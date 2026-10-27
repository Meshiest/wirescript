//! Program -> `.wsir` text, in the canonical form `FORMAT.md` describes.
//!
//! The writer first plans the whole file (the text order of every node, its
//! name, its operands, and the number each wire gets), then streams it.
//! Whatever the parser's defaults reproduce is left out; everything else is
//! written, so parsing the output gives back the same program.

use std::io::{self, Write};

use crate::collections::{HashMap, HashSet};
use crate::intern::{Sym, resolve, sym};
use crate::ir::module_tree::ModuleTree;
use crate::ir::port_registry::WirePort;
use crate::ir::{
    GateIO, LayoutMode, Literal, Module, Node, NodeId, NodeKind, PortSpec, Program, ROOT_SCOPE_ID,
    ScopeInfo, ScopeKind, SourceRange, StyleValue, Type, Wire,
};

use super::defaults::{self, AnnValue, is_ident, word};

/// Write `program` as `.wsir` text.
pub fn write_program<W: Write>(program: &Program, out: &mut W) -> io::Result<()> {
    let plan = Plan::build(program)?;
    let mut w = Writer {
        out,
        plan: &plan,
        main_file: program.settings.source_file.as_deref(),
    };
    w.file(program)
}

/// Whether writing `program` gives `text` back, line endings aside: a
/// canonical file is a fixed point of parse then write. `Err` names the first
/// line that differs.
pub fn check_fixed_point(program: &Program, text: &str) -> Result<(), String> {
    let again = program_to_string(program).map_err(|e| format!("cannot write the program: {e}"))?;
    let text = text.replace("\r\n", "\n");
    if again == text {
        return Ok(());
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (again.split('\n').collect(), text.split('\n').collect());
    let line = a
        .iter()
        .zip(&b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()))
        + 1;
    Err(format!(
        "writing the parsed program does not give the same text (first difference at line {line})"
    ))
}

/// [`write_program`] into a string.
pub fn program_to_string(program: &Program) -> io::Result<String> {
    let mut buf = Vec::new();
    write_program(program, &mut buf)?;
    Ok(String::from_utf8(buf).expect("the writer only writes UTF-8"))
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// A module of the program, numbered in text (pre-)order.
struct ModInfo<'p> {
    module: &'p Module,
    /// The chip node this module is the body of.
    chip: Option<NodeId>,
}

/// How a node is written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Gate,
    In,
    Out,
    Chip,
}

enum Operand<'p> {
    Wire {
        port: WirePort,
        wire: &'p Wire,
    },
    Lit {
        port: &'static str,
        lit: &'p Literal,
    },
}

struct NodePlan<'p> {
    node: &'p Node,
    module: usize,
    form: Form,
    /// The pin type of an `in`/`out`.
    pin_type: Option<Type>,
    operands: Vec<Operand<'p>>,
    layout_after: Vec<&'p Wire>,
}

struct Plan<'p> {
    mods: Vec<ModInfo<'p>>,
    tree: ModuleTree,
    /// Node ids in text order.
    order: Vec<NodeId>,
    nodes: HashMap<NodeId, NodePlan<'p>>,
    names: HashMap<NodeId, String>,
    /// The module whose body a chip node's child module is.
    chip_module: HashMap<NodeId, usize>,
    /// Per module: the `!wire_order` list it needs, if any.
    wire_order: Vec<Option<Vec<usize>>>,
}

impl<'p> Plan<'p> {
    fn build(program: &'p Program) -> io::Result<Self> {
        let mut plan = Plan {
            mods: Vec::new(),
            tree: ModuleTree::new(),
            order: Vec::new(),
            nodes: HashMap::default(),
            names: HashMap::default(),
            chip_module: HashMap::default(),
            wire_order: Vec::new(),
        };
        plan.walk(&program.module, None, None);

        // Names, in id order so the earliest-lowered node wanting a name (the
        // declaration rather than a boundary pin copied from it) gets it.
        let mut taken: HashSet<String> = HashSet::default();
        let mut by_id = plan.order.clone();
        by_id.sort();
        for id in &by_id {
            let np = &plan.nodes[id];
            let given = plan.mods[np.module]
                .module
                .value_names
                .get(id)
                .map(|s| resolve(*s).to_string());
            let name = given
                .filter(|n| {
                    !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
                .filter(|n| !n.bytes().all(|b| b.is_ascii_digit()))
                .or_else(|| source_name(np.node))
                .and_then(|base| {
                    if !taken.contains(&base) {
                        return Some(base);
                    }
                    let alt = format!("{base}_{}", id.0);
                    (!taken.contains(&alt)).then_some(alt)
                })
                .unwrap_or_else(|| id.0.to_string());
            taken.insert(name.clone());
            plan.names.insert(*id, name);
        }

        // Every wire, grouped under its target node.
        let mut incoming: HashMap<NodeId, Vec<&'p Wire>> = HashMap::default();
        for m in &plan.mods {
            for w in &m.module.wires {
                incoming.entry(w.target.node_id).or_default().push(w);
            }
        }
        for id in plan.order.clone() {
            let wires = incoming.remove(&id).unwrap_or_default();
            plan.plan_operands(id, wires)?;
        }
        if let Some((id, _)) = incoming.into_iter().next() {
            return Err(invalid(format!(
                "a wire targets {id}, which is not a node of this program"
            )));
        }
        for w in plan.mods.iter().flat_map(|m| &m.module.wires) {
            if !plan.nodes.contains_key(&w.source.node_id) {
                return Err(invalid(format!(
                    "a wire comes from {}, which is not a node of this program",
                    w.source.node_id
                )));
            }
        }

        // Number the wires in text order, then see which modules store
        // theirs somewhere or in an order the default would not give.
        let mut number: HashMap<*const Wire, usize> = HashMap::default();
        for id in &plan.order {
            let np = &plan.nodes[id];
            for op in &np.operands {
                if let Operand::Wire { wire, .. } = op {
                    let n = number.len();
                    number.insert(std::ptr::from_ref::<Wire>(*wire), n);
                }
            }
            for wire in &np.layout_after {
                let n = number.len();
                number.insert(std::ptr::from_ref::<Wire>(*wire), n);
            }
        }
        let mut default_lists: Vec<Vec<usize>> = vec![Vec::new(); plan.mods.len()];
        let mut by_number: Vec<Option<&Wire>> = vec![None; number.len()];
        for m in &plan.mods {
            for w in &m.module.wires {
                by_number[number[&std::ptr::from_ref::<Wire>(w)]] = Some(w);
            }
        }
        for (n, w) in by_number.iter().enumerate() {
            let w = w.expect("every wire is numbered once");
            let home = plan.tree.common_module(
                plan.nodes[&w.source.node_id].module,
                plan.nodes[&w.target.node_id].module,
            );
            default_lists[home].push(n);
        }
        for (mi, m) in plan.mods.iter().enumerate() {
            for host in m.module.dynamic_labels.keys() {
                if plan.nodes.get(host).is_none_or(|np| np.module != mi) {
                    return Err(invalid(format!(
                        "dynamic label on {host}, which is not a node of its module"
                    )));
                }
            }
            for host in m
                .module
                .dynamic_colors
                .keys()
                .chain(m.module.dynamic_materials.keys())
            {
                if plan.nodes.get(host).is_none_or(|np| np.module != mi) {
                    return Err(invalid(format!(
                        "dynamic style on {host}, which is not a node of its module"
                    )));
                }
            }
        }
        for (mi, m) in plan.mods.iter().enumerate() {
            let actual: Vec<usize> = m
                .module
                .wires
                .iter()
                .map(|w| number[&std::ptr::from_ref::<Wire>(w)])
                .collect();
            plan.wire_order
                .push((actual != default_lists[mi]).then_some(actual));
        }
        Ok(plan)
    }

    fn walk(&mut self, m: &'p Module, parent: Option<usize>, chip: Option<NodeId>) {
        let mi = match parent {
            Some(p) => self.tree.push(p),
            None => 0,
        };
        self.mods.push(ModInfo { module: m, chip });
        let mut ids: Vec<NodeId> = m.nodes.keys().copied().collect();
        ids.sort();
        for id in ids {
            let node = &m.nodes[&id];
            self.order.push(id);
            self.nodes.insert(
                id,
                NodePlan {
                    node,
                    module: mi,
                    form: Form::Gate,
                    pin_type: None,
                    operands: Vec::new(),
                    layout_after: Vec::new(),
                },
            );
            if let Some(child) = m.chips.get(&id) {
                self.chip_module.insert(id, self.mods.len());
                self.walk(child, Some(mi), Some(id));
            }
        }
    }

    fn plan_operands(&mut self, id: NodeId, wires: Vec<&'p Wire>) -> io::Result<()> {
        let node = self.nodes[&id].node;
        let mut layout_after: Vec<&Wire> = Vec::new();
        let mut by_port: HashMap<WirePort, &Wire> = HashMap::default();
        for w in wires {
            if w.target.port == WirePort::Layout {
                if w.source.port != WirePort::Layout {
                    return Err(invalid(format!(
                        "layout edge {} -> {id} leaves from port {}, not _Layout",
                        w.source.node_id, w.source.port
                    )));
                }
                layout_after.push(w);
                continue;
            }
            if let Some(prev) = by_port.insert(w.target.port, w) {
                return Err(invalid(format!(
                    "fan-in: {id}.{} is fed by both {}.{} and {}.{}",
                    w.target.port,
                    prev.source.node_id,
                    prev.source.port,
                    w.source.node_id,
                    w.source.port
                )));
            }
        }
        layout_after.sort_by_key(|w| w.source.node_id);

        let mut operands = Vec::new();
        let mut used_props: HashSet<Sym> = HashSet::default();
        for p in &node.ports.inputs {
            let name = resolve(p.name);
            match WirePort::try_from_name(name).and_then(|wp| by_port.remove(&wp).map(|w| (wp, w)))
            {
                Some((port, wire)) => operands.push(Operand::Wire { port, wire }),
                None => {
                    if let Some(lit) = node.properties.get(&p.name) {
                        used_props.insert(p.name);
                        operands.push(Operand::Lit { port: name, lit });
                    }
                }
            }
        }
        let mut rest: Vec<(WirePort, &Wire)> = by_port.into_iter().collect();
        rest.sort_by_key(|(p, _)| p.as_str());
        operands.extend(
            rest.into_iter()
                .map(|(port, wire)| Operand::Wire { port, wire }),
        );

        let is_chip = self.chip_module.contains_key(&id);
        let pin_type = pin_pattern(&node.ports);
        let wires_only_rer = operands.iter().all(|op| match op {
            Operand::Wire { port, .. } => *port == WirePort::RerInput,
            Operand::Lit { .. } => false,
        });
        let form = if is_chip {
            Form::Chip
        } else if pin_type.is_some() && wires_only_rer && node.kind == NodeKind::Input {
            Form::In
        } else if pin_type.is_some() && wires_only_rer && node.kind == NodeKind::Output {
            Form::Out
        } else {
            Form::Gate
        };
        let np = self.nodes.get_mut(&id).unwrap();
        np.form = form;
        np.pin_type = if matches!(form, Form::In | Form::Out) {
            pin_type
        } else {
            None
        };
        np.operands = operands;
        np.layout_after = layout_after;
        Ok(())
    }

    fn name(&self, id: NodeId) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }
}

/// The type `T` when `ports` are exactly a pin's `(RER_Input: T) -> (RER_Output: T)`.
fn pin_pattern(ports: &GateIO) -> Option<Type> {
    match (ports.inputs.as_slice(), ports.outputs.as_slice()) {
        ([i], [o]) if i.name == *sym::RER_INPUT && o.name == *sym::RER_OUTPUT && i.ty == o.ty => {
            Some(i.ty.clone())
        }
        _ => None,
    }
}

/// The name a node's source gave it, made into a value name.
fn source_name(node: &Node) -> Option<String> {
    let props = &node.properties;
    let pick = |s: Sym| match props.get(&s) {
        Some(Literal::String(t)) if !t.is_empty() => Some(t.clone()),
        _ => None,
    };
    let raw = if matches!(node.kind, NodeKind::Input | NodeKind::Output) {
        pick(*sym::PORT_LABEL).or_else(|| pick(*sym::NAME_LABEL))
    } else {
        pick(*sym::NAME_LABEL).or_else(|| pick(*sym::BINDING_NAME))
    }?;
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    (!name.is_empty() && !name.bytes().all(|b| b.is_ascii_digit())).then_some(name)
}

/// `, ` before every item of a list but the first (item `i`).
fn comma<W: Write>(out: &mut W, i: usize) -> io::Result<()> {
    if i > 0 { out.write_all(b", ") } else { Ok(()) }
}

struct Writer<'w, 'p, W: Write> {
    out: &'w mut W,
    plan: &'w Plan<'p>,
    /// `source_filename`: locations in it are written without the file.
    main_file: Option<&'w str>,
}

impl<W: Write> Writer<'_, '_, W> {
    fn file(&mut self, program: &Program) -> io::Result<()> {
        let s = &program.settings;
        let root = &program.module;
        writeln!(self.out, "; wsir 1")?;
        if let Some(file) = &s.source_file {
            write!(self.out, "source_filename = ")?;
            string(self.out, file)?;
            writeln!(self.out)?;
        }
        if s.layout != LayoutMode::default() {
            let (w, _) = defaults::LAYOUT_MODES
                .iter()
                .find(|(_, m)| *m == s.layout)
                .expect("every layout has a word");
            writeln!(self.out, "layout = {w}")?;
        }
        if s.invisible {
            writeln!(self.out, "invisible = true")?;
        }
        if s.no_gate_labels {
            writeln!(self.out, "no_gate_labels = true")?;
        }
        if s.no_fold {
            writeln!(self.out, "no_fold = true")?;
        }
        if let Some(doc) = &s.module_doc {
            write!(self.out, "doc = ")?;
            string(self.out, doc)?;
            writeln!(self.out)?;
        }
        if let Some(label) = &root.root_label_override {
            write!(self.out, "label = ")?;
            string(self.out, label)?;
            writeln!(self.out)?;
        }
        if let Some(p) = &root.root_dynamic_label {
            write!(self.out, "label_from = ")?;
            self.port_ref(p.node_id, p.port)?;
            writeln!(self.out)?;
        }
        for (key, value) in [
            ("color", &root.root_style.color),
            ("material", &root.root_style.material),
        ] {
            match value {
                Some(StyleValue::Baked(lit)) => {
                    write!(self.out, "{key} = ")?;
                    literal(self.out, lit)?;
                    writeln!(self.out)?;
                }
                Some(StyleValue::Wired(p)) => {
                    write!(self.out, "{key}_from = ")?;
                    self.port_ref(p.node_id, p.port)?;
                    writeln!(self.out)?;
                }
                None => {}
            }
        }
        if let Some(sm) = &s.source_map {
            write!(self.out, "!source(")?;
            string(self.out, &sm.file)?;
            write!(self.out, ", indent: [")?;
            for (i, n) in sm.line_indent.iter().enumerate() {
                if i > 0 {
                    self.out
                        .write_all(if i % 32 == 0 { b",\n  " } else { b", " })?;
                }
                write!(self.out, "{n}")?;
            }
            writeln!(self.out, "])")?;
            for c in &sm.comments {
                write!(self.out, "!comment(")?;
                string(self.out, &sm.file)?;
                write!(self.out, ":{}:{}, ", c.line, c.col)?;
                string(self.out, &c.text)?;
                if c.own_line {
                    write!(self.out, ", own_line")?;
                }
                if c.in_array {
                    write!(self.out, ", in_array")?;
                }
                writeln!(self.out, ")")?;
            }
        }
        writeln!(self.out)?;
        write!(self.out, "module ")?;
        mod_name(self.out, resolve(root.name))?;
        writeln!(self.out, " {{")?;
        self.body(0)?;
        writeln!(self.out, "}}")
    }

    fn indent(&mut self, depth: usize) -> io::Result<()> {
        for _ in 0..=depth {
            self.out.write_all(b"  ")?;
        }
        Ok(())
    }

    fn body(&mut self, mi: usize) -> io::Result<()> {
        let plan = self.plan;
        let info = &plan.mods[mi];
        let m = info.module;
        let depth = plan.tree.depth(mi);
        let mut ids: Vec<NodeId> = m.nodes.keys().copied().collect();
        ids.sort();
        // The node written just before this body is its chip; the root has none.
        let mut prev_id = info.chip.map_or(0, |c| c.0);
        for id in &ids {
            self.indent(depth)?;
            self.node(*id, prev_id)?;
            prev_id = self.last_id_in(*id);
        }

        // Module metadata.
        let mut scope_ids: Vec<&u32> = m.scopes.keys().collect();
        scope_ids.sort();
        let default_root = ScopeInfo::root(ScopeKind::ModuleRoot);
        if !m.scopes.contains_key(&ROOT_SCOPE_ID) {
            return Err(invalid(format!(
                "module {} has no root scope",
                resolve(m.name)
            )));
        }
        for sid in scope_ids {
            let s = &m.scopes[sid];
            if *sid == ROOT_SCOPE_ID && *s == default_root {
                continue;
            }
            self.indent(depth)?;
            write!(self.out, "!{sid} = !scope(")?;
            scope_kind(self.out, &s.kind)?;
            if let Some(p) = s.parent {
                write!(self.out, ", parent: !{p}")?;
            }
            if !is_empty_range(&s.source_range) {
                write!(self.out, ", ")?;
                loc(self.out, &s.source_range, self.main_file)?;
            }
            writeln!(self.out, ")")?;
        }
        for (kw, list, kind) in [
            ("inputs", &m.inputs, NodeKind::Input),
            ("outputs", &m.outputs, NodeKind::Output),
        ] {
            let mut default: Vec<NodeId> = m
                .nodes
                .iter()
                .filter(|(_, n)| n.kind == kind)
                .map(|(id, _)| *id)
                .collect();
            default.sort();
            if *list != default {
                self.indent(depth)?;
                write!(self.out, "!{kw}(")?;
                self.id_list(list)?;
                writeln!(self.out, ")")?;
            }
        }
        if let Some(list) = &plan.wire_order[mi] {
            self.indent(depth)?;
            write!(self.out, "!wire_order(")?;
            for (i, n) in list.iter().enumerate() {
                if i > 0 {
                    self.out
                        .write_all(if i % 32 == 0 { b",\n" } else { b", " })?;
                    if i % 32 == 0 {
                        self.indent(depth + 1)?;
                    }
                }
                write!(self.out, "{n}")?;
            }
            writeln!(self.out, ")")?;
        }
        Ok(())
    }

    /// The id of the last node written for `id`: itself, or the last node of
    /// its chip body.
    fn last_id_in(&self, id: NodeId) -> u32 {
        match self.plan.chip_module.get(&id) {
            None => id.0,
            Some(&mi) => {
                let m = self.plan.mods[mi].module;
                match m.nodes.keys().max() {
                    Some(last) => self.last_id_in(*last),
                    None => id.0,
                }
            }
        }
    }

    fn id_list(&mut self, ids: &[NodeId]) -> io::Result<()> {
        for (i, id) in ids.iter().enumerate() {
            if i > 0 {
                self.out
                    .write_all(if i % 16 == 0 { b",\n      " } else { b", " })?;
            }
            self.node_ref(*id)?;
        }
        Ok(())
    }

    /// `%name`, or the bare id for a node not in the program.
    fn node_ref(&mut self, id: NodeId) -> io::Result<()> {
        match self.plan.name(id) {
            Some(n) => write!(self.out, "%{n}"),
            None => write!(self.out, "{}", id.0),
        }
    }

    fn port_ref(&mut self, id: NodeId, port: WirePort) -> io::Result<()> {
        let name = self
            .plan
            .name(id)
            .ok_or_else(|| invalid(format!("reference to {id}, not a node of this program")))?;
        write!(self.out, "%{name}")?;
        let src = &self.plan.nodes[&id].node.ports.outputs;
        if !(src.len() == 1 && src[0].name == port.sym()) {
            write!(self.out, ".")?;
            port_name(self.out, port.as_str())?;
        }
        Ok(())
    }

    fn node(&mut self, id: NodeId, prev_id: u32) -> io::Result<()> {
        let plan = self.plan;
        let np = &plan.nodes[&id];
        let node = np.node;
        let name = &plan.names[&id];
        let numeric = name.bytes().all(|b| b.is_ascii_digit());
        let class = defaults::class_spelling(node.gate_class);

        let mut skip_label = false;
        match np.form {
            Form::In => {
                write!(self.out, "%{name} = in")?;
                if node.gate_class != defaults::INPUT_CLASS {
                    write!(self.out, " {class}")?;
                }
                write!(self.out, " ")?;
                ty(self.out, np.pin_type.as_ref().unwrap())?;
                if let Some(Operand::Wire { wire, .. }) = np.operands.first() {
                    write!(self.out, "(")?;
                    self.port_ref(wire.source.node_id, wire.source.port)?;
                    write!(self.out, ")")?;
                }
                skip_label = true;
            }
            Form::Out => {
                write!(self.out, "out %{name}")?;
                if node.gate_class != defaults::OUTPUT_CLASS {
                    write!(self.out, " {class}")?;
                }
                write!(self.out, " ")?;
                ty(self.out, np.pin_type.as_ref().unwrap())?;
                if let Some(Operand::Wire { wire, .. }) = np.operands.first() {
                    write!(self.out, " = ")?;
                    self.port_ref(wire.source.node_id, wire.source.port)?;
                }
                skip_label = true;
            }
            Form::Chip => {
                write!(self.out, "%{name} = chip")?;
                if node.gate_class != defaults::CHIP_CLASS {
                    write!(self.out, " {class}")?;
                }
                let child = plan.mods[plan.chip_module[&id]].module;
                if resolve(child.name) != defaults::default_chip_module_name(id.0) {
                    write!(self.out, " ")?;
                    mod_name(self.out, resolve(child.name))?;
                }
                if !np.operands.is_empty() {
                    write!(self.out, "(")?;
                    self.operands(&np.operands)?;
                    write!(self.out, ")")?;
                }
            }
            Form::Gate => {
                write!(self.out, "%{name} = {class}")?;
                if !np.operands.is_empty() {
                    write!(self.out, "(")?;
                    self.operands(&np.operands)?;
                    write!(self.out, ")")?;
                }
            }
        }

        // Attributes: every property not already written as an operand.
        let operand_props: HashSet<&str> = np
            .operands
            .iter()
            .filter_map(|op| match op {
                Operand::Lit { port, .. } => Some(*port),
                Operand::Wire { .. } => None,
            })
            .collect();
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut label_written = false;
        for (k, v) in node.properties.iter() {
            let key = resolve(*k);
            if operand_props.contains(key) {
                continue;
            }
            if skip_label && *k == *sym::PORT_LABEL {
                if matches!(v, Literal::String(s) if s == name) {
                    label_written = true;
                    continue;
                }
                label_written = true;
            }
            attrs.push(self.attr(key, v)?);
        }
        if skip_label && !label_written {
            attrs.push((defaults::NO_LABEL.to_string(), String::new()));
        }
        attrs.sort();
        if !attrs.is_empty() {
            write!(self.out, " {{ ")?;
            for (i, (k, v)) in attrs.iter().enumerate() {
                comma(self.out, i)?;
                self.out.write_all(k.as_bytes())?;
                if !v.is_empty() {
                    write!(self.out, " = {v}")?;
                }
            }
            write!(self.out, " }}")?;
        }

        // Signature.
        let default_ports_hold = match np.form {
            Form::In | Form::Out => true,
            _ => {
                // What the parser gives a gate with no signature: its default
                // ports, each `any` input typed by the wire feeding it.
                let mut d =
                    defaults::default_ports(node.gate_class, node.properties.get(&*sym::VALUE));
                for op in &np.operands {
                    let Operand::Wire { port, wire } = op else {
                        continue;
                    };
                    let src = &plan.nodes[&wire.source.node_id].node.ports;
                    if let Some(t) = src.find_output(wire.source.port.sym()) {
                        defaults::infer_any_input(&mut d, port.sym(), t.ty.clone());
                    }
                }
                d == *node.ports
            }
        };
        if !default_ports_hold {
            write!(self.out, " : (")?;
            port_list(self.out, &node.ports.inputs)?;
            write!(self.out, ") -> (")?;
            port_list(self.out, &node.ports.outputs)?;
            write!(self.out, ")")?;
        }

        // Metadata.
        if !numeric && id.0 != prev_id.wrapping_add(1) {
            write!(self.out, " !id {}", id.0)?;
        }
        let default_kind = match np.form {
            Form::In => NodeKind::Input,
            Form::Out => NodeKind::Output,
            Form::Chip => NodeKind::Chip,
            Form::Gate => defaults::default_kind(node.gate_class),
        };
        if node.kind != default_kind {
            write!(self.out, " !kind {}", kind_name(node.kind))?;
        }
        if node.scope_id != ROOT_SCOPE_ID {
            write!(self.out, " !scope !{}", node.scope_id)?;
        }
        if let Some(c) = node.chain_id {
            write!(self.out, " !chain {c}")?;
        }
        if let Some(c) = node.chip_id {
            write!(self.out, " !chip ")?;
            self.node_ref(c)?;
        }
        if let Some(p) = plan.mods[np.module].module.dynamic_labels.get(&id) {
            write!(self.out, " !label(")?;
            self.port_ref(p.node_id, p.port)?;
            write!(self.out, ")")?;
        }
        if let Some(p) = plan.mods[np.module].module.dynamic_colors.get(&id) {
            write!(self.out, " !color(")?;
            self.port_ref(p.node_id, p.port)?;
            write!(self.out, ")")?;
        }
        if let Some(p) = plan.mods[np.module].module.dynamic_materials.get(&id) {
            write!(self.out, " !material(")?;
            self.port_ref(p.node_id, p.port)?;
            write!(self.out, ")")?;
        }
        if !np.layout_after.is_empty() {
            write!(self.out, " !layout_after(")?;
            for (i, w) in np.layout_after.iter().enumerate() {
                comma(self.out, i)?;
                self.node_ref(w.source.node_id)?;
            }
            write!(self.out, ")")?;
        }
        if let Some(note) = node.note {
            write!(self.out, " !note ")?;
            string(self.out, note)?;
        }

        if np.form == Form::Chip {
            writeln!(self.out, " {{")?;
            let child = plan.chip_module[&id];
            self.body(child)?;
            self.indent(plan.tree.depth(np.module))?;
            write!(self.out, "}}")?;
        }
        if !is_empty_range(&node.source_range) {
            write!(self.out, " ")?;
            loc(self.out, &node.source_range, self.main_file)?;
        }
        writeln!(self.out)
    }

    fn operands(&mut self, ops: &[Operand<'_>]) -> io::Result<()> {
        for (i, op) in ops.iter().enumerate() {
            comma(self.out, i)?;
            match op {
                Operand::Wire { port, wire } => {
                    port_name(self.out, port.as_str())?;
                    write!(self.out, ": ")?;
                    self.port_ref(wire.source.node_id, wire.source.port)?;
                }
                Operand::Lit { port, lit } => {
                    port_name(self.out, port)?;
                    write!(self.out, ": ")?;
                    literal(self.out, lit)?;
                }
            }
        }
        Ok(())
    }

    /// `%name` of the node whose id is `id`, if it names one.
    fn node_name(&self, id: &Literal) -> Option<String> {
        let Literal::Int(n) = id else { return None };
        let n = NodeId(u32::try_from(*n).ok()?);
        self.plan.name(n)?;
        Some(format!("%{}", self.plan.names[&n]))
    }

    /// One attribute as `(key, value)`; an empty value is a bare flag.
    fn attr(&mut self, prop: &str, v: &Literal) -> io::Result<(String, String)> {
        if let Some((key, kind)) = defaults::annotation_by_prop(prop) {
            match (kind, v) {
                (AnnValue::Flag, Literal::Bool(true)) => {
                    return Ok((key.to_string(), String::new()));
                }
                (AnnValue::Str, Literal::String(_)) => return Ok((key.to_string(), lit_string(v))),
                (AnnValue::Word, Literal::String(s))
                    if is_ident(s) && !defaults::is_literal_word(s) =>
                {
                    return Ok((key.to_string(), s.clone()));
                }
                (AnnValue::Node, Literal::Int(n))
                    if u32::try_from(*n).is_ok_and(|n| self.plan.name(NodeId(n)).is_some()) =>
                {
                    let n = NodeId(*n as u32);
                    return Ok((key.to_string(), format!("%{}", self.plan.names[&n])));
                }
                (AnnValue::Nodes, Literal::Array(ids)) => {
                    if let Some(names) = ids
                        .iter()
                        .map(|id| self.node_name(id))
                        .collect::<Option<Vec<_>>>()
                    {
                        return Ok((key.to_string(), format!("[{}]", names.join(", "))));
                    }
                }
                (AnnValue::Nodes, Literal::Map(entries)) => {
                    let entries = entries
                        .iter()
                        .map(|(k, id)| {
                            Some(format!("{} => {}", lit_string(k), self.node_name(id)?))
                        })
                        .collect::<Option<Vec<_>>>();
                    if let Some(entries) = entries {
                        return Ok((key.to_string(), format!("map[{}]", entries.join(", "))));
                    }
                }
                _ => {}
            }
        }
        let shadowed = defaults::annotation_by_key(prop).is_some() || prop == defaults::NO_LABEL;
        let key = if is_ident(prop) && !shadowed {
            prop.to_string()
        } else {
            let mut buf = Vec::new();
            string(&mut buf, prop)?;
            String::from_utf8(buf).unwrap()
        };
        Ok((key, lit_string(v)))
    }
}

fn lit_string(v: &Literal) -> String {
    let mut buf = Vec::new();
    literal(&mut buf, v).expect("writing to a Vec cannot fail");
    String::from_utf8(buf).unwrap()
}

fn is_empty_range(r: &SourceRange) -> bool {
    *r == SourceRange::default()
}

fn kind_name(k: NodeKind) -> &'static str {
    defaults::NODE_KINDS
        .iter()
        .find(|(_, n)| *n == k)
        .map(|(w, _)| *w)
        .expect("every node kind has a word")
}

fn scope_kind<W: Write>(out: &mut W, k: &ScopeKind) -> io::Result<()> {
    let (w, name) = defaults::scope_kind_word(k);
    out.write_all(w.as_bytes())?;
    match name {
        Some(name) => {
            out.write_all(b" ")?;
            string(out, name)
        }
        None => Ok(()),
    }
}

fn mod_name<W: Write>(out: &mut W, name: &str) -> io::Result<()> {
    if is_ident(name) {
        write!(out, "@{name}")
    } else {
        write!(out, "@")?;
        string(out, name)
    }
}

pub(super) fn string<W: Write>(out: &mut W, s: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let esc: Option<&[u8]> = match c {
            '"' => Some(b"\\\""),
            '\\' => Some(b"\\\\"),
            '\n' => Some(b"\\n"),
            '\r' => Some(b"\\r"),
            '\t' => Some(b"\\t"),
            _ => None,
        };
        let control = esc.is_none() && (c < ' ' || c == '\u{7f}');
        if esc.is_some() || control {
            out.write_all(&s.as_bytes()[start..i])?;
            match esc {
                Some(e) => out.write_all(e)?,
                None => write!(out, "\\u{{{:x}}}", c as u32)?,
            }
            start = i + c.len_utf8();
        }
    }
    out.write_all(&s.as_bytes()[start..])?;
    out.write_all(b"\"")
}

pub(super) fn float<W: Write>(out: &mut W, f: f64) -> io::Result<()> {
    if f.is_nan() {
        write!(
            out,
            "{}{}",
            if f.is_sign_negative() { "-" } else { "" },
            word::NAN
        )
    } else if f.is_infinite() {
        write!(out, "{}{}", if f < 0.0 { "-" } else { "" }, word::INF)
    } else {
        write!(out, "{f:?}")
    }
}

pub(super) fn literal<W: Write>(out: &mut W, lit: &Literal) -> io::Result<()> {
    let floats = |out: &mut W, name: &str, fs: &[f64]| -> io::Result<()> {
        write!(out, "{name}(")?;
        for (i, f) in fs.iter().enumerate() {
            comma(out, i)?;
            float(out, *f)?;
        }
        write!(out, ")")
    };
    match lit {
        Literal::Bool(b) => out.write_all(if *b { word::TRUE } else { word::FALSE }.as_bytes()),
        Literal::Int(n) => write!(out, "{n}"),
        Literal::Float(f) => float(out, *f),
        Literal::String(s) => string(out, s),
        Literal::Vector { x, y, z } => floats(out, word::VEC, &[*x, *y, *z]),
        Literal::Rotator { pitch, yaw, roll } => floats(out, word::ROT, &[*pitch, *yaw, *roll]),
        Literal::Quat { x, y, z, w } => floats(out, word::QUAT, &[*x, *y, *z, *w]),
        Literal::Color { r, g, b, a } => write!(out, "#{r:02x}{g:02x}{b:02x}{a:02x}"),
        Literal::LinearColor { r, g, b, a } => floats(out, word::LRGBA, &[*r, *g, *b, *a]),
        Literal::Object => out.write_all(word::NULL.as_bytes()),
        Literal::Array(items) => {
            write!(out, "[")?;
            for (i, it) in items.iter().enumerate() {
                comma(out, i)?;
                literal(out, it)?;
            }
            write!(out, "]")
        }
        Literal::Map(entries) => {
            write!(out, "{}[", word::MAP)?;
            for (i, (k, v)) in entries.iter().enumerate() {
                comma(out, i)?;
                literal(out, k)?;
                write!(out, " => ")?;
                literal(out, v)?;
            }
            write!(out, "]")
        }
        Literal::Record(fields) => {
            write!(out, "{}{{", word::REC)?;
            for (i, (k, v)) in fields.iter().enumerate() {
                comma(out, i)?;
                if is_ident(k) {
                    write!(out, "{k}")?;
                } else {
                    string(out, k)?;
                }
                write!(out, ": ")?;
                literal(out, v)?;
            }
            write!(out, "}}")
        }
        Literal::Asset {
            asset_type,
            asset_name,
        } => {
            write!(out, "{}(", word::ASSET)?;
            string(out, asset_type)?;
            write!(out, ", ")?;
            string(out, asset_name)?;
            write!(out, ")")
        }
        Literal::PrefabRef { path } => {
            write!(out, "{}(", word::PREFAB)?;
            string(out, path)?;
            write!(out, ")")
        }
        Literal::NestedPrefab { source } => {
            write!(out, "{}(", word::NESTED_PREFAB)?;
            string(out, source)?;
            write!(out, ")")
        }
        Literal::PrefabBytes(bytes) => {
            write!(out, "{}(\"", word::PREFAB_BYTES)?;
            out.write_all(super::base64::encode(bytes).as_bytes())?;
            write!(out, "\")")
        }
    }
}

fn port_list<W: Write>(out: &mut W, ports: &[PortSpec]) -> io::Result<()> {
    for (i, p) in ports.iter().enumerate() {
        comma(out, i)?;
        port_name(out, resolve(p.name))?;
        write!(out, ": ")?;
        ty(out, &p.ty)?;
    }
    Ok(())
}

pub(super) fn ty<W: Write>(out: &mut W, t: &Type) -> io::Result<()> {
    let list = |out: &mut W, name: &str, ts: &[Type]| -> io::Result<()> {
        write!(out, "{name}<")?;
        for (i, t) in ts.iter().enumerate() {
            comma(out, i)?;
            ty(out, t)?;
        }
        write!(out, ">")
    };
    let fields = |out: &mut W, fs: &[(String, Type)]| -> io::Result<()> {
        write!(out, "{{")?;
        for (i, (k, t)) in fs.iter().enumerate() {
            comma(out, i)?;
            if is_ident(k) {
                write!(out, "{k}")?;
            } else {
                string(out, k)?;
            }
            write!(out, ": ")?;
            ty(out, t)?;
        }
        write!(out, "}}")
    };
    if let Some((w, _)) = defaults::SCALAR_TYPES.iter().find(|(_, s)| s == t) {
        return out.write_all(w.as_bytes());
    }
    match t {
        Type::Ref(inner) => {
            write!(out, "*")?;
            ty(out, inner)
        }
        Type::Array(inner) if matches!(inner.as_ref(), Type::Ref(_)) => {
            list(out, "array", std::slice::from_ref(inner))
        }
        Type::Array(inner) => {
            ty(out, inner)?;
            write!(out, "[]")
        }
        Type::Map(k, v) => list(out, "map", &[(**k).clone(), (**v).clone()]),
        Type::Union(ts) => list(out, "union", ts),
        Type::Tuple(ts) => list(out, "tuple", ts),
        Type::Record(fs) => fields(out, fs),
        Type::Outputs(fs) => {
            write!(out, "outputs")?;
            fields(out, fs)
        }
        Type::Param(n) => {
            write!(out, "param<")?;
            string(out, n)?;
            write!(out, ">")
        }
        Type::Enum { name, args } => {
            write!(out, "enum<")?;
            string(out, name)?;
            for a in args {
                write!(out, ", ")?;
                ty(out, a)?;
            }
            write!(out, ">")
        }
        Type::Component(class) => {
            write!(out, "component<")?;
            string(out, class)?;
            write!(out, ">")
        }
        _ => unreachable!("every type without parameters is in SCALAR_TYPES"),
    }
}

pub(super) fn loc<W: Write>(
    out: &mut W,
    r: &SourceRange,
    main_file: Option<&str>,
) -> io::Result<()> {
    write!(out, "loc(")?;
    if main_file != Some(&*r.file) {
        string(out, &r.file)?;
        write!(out, ":")?;
    }
    write!(out, "{}:{}", r.start.line, r.start.col)?;
    if r.start.offset != 0 {
        write!(out, "+{}", r.start.offset)?;
    }
    if r.end != r.start {
        write!(out, " to {}:{}", r.end.line, r.end.col)?;
        if r.end.offset != 0 {
            write!(out, "+{}", r.end.offset)?;
        }
    }
    write!(out, ")")
}

/// A port name: bare when it is an identifier, quoted otherwise (`DisplayText`'s
/// `Position.X` sub-ports are not).
fn port_name<W: Write>(out: &mut W, name: &str) -> io::Result<()> {
    if is_ident(name) {
        out.write_all(name.as_bytes())
    } else {
        string(out, name)
    }
}
