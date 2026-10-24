//! [`Program::validate`]: the invariants lowering guarantees and emit relies
//! on, checked for a program that came from somewhere else.

use super::port_registry::WirePort;
use super::{Module, Node, NodeId, NodeKind, PortRef, Program, Type, gate_class as gc};
use crate::collections::HashMap;
use crate::intern::{Sym, resolve};

/// One broken invariant.
#[derive(Clone, Debug, PartialEq)]
pub enum ValidationError {
    /// A node's class is in neither the inventory nor the pseudo-classes the
    /// backend knows.
    UnknownClass { node: NodeId, class: String },
    /// A wire end names a port its node does not have on that side, or a
    /// node declares a port its inventory class does not have.
    UnknownPort {
        node: NodeId,
        class: String,
        port: String,
        side: PortSide,
        valid: Vec<String>,
    },
    /// A wire carries an exec pulse into a value input, which it cannot
    /// fill. (A value into an exec input is legal: its change fires it.)
    TypeMismatch {
        from: PortRef,
        to: PortRef,
        from_type: Type,
        to_type: Type,
    },
    /// Two wires feed one input. Fan-in fails the whole save at load.
    FanIn {
        target: PortRef,
        sources: Vec<PortRef>,
    },
    /// A wire, label or list names a node that is not in the program.
    MissingNode { node: NodeId, referenced_by: String },
    /// A module's inputs, outputs or chips disagree with its nodes.
    ChipIo {
        module: String,
        node: NodeId,
        problem: String,
    },
    /// A `coerce` node, which lowering never makes and the backend does not
    /// emit.
    Coerce { node: NodeId },
    /// One node id is used in more than one module. Ids name nodes across
    /// the whole program, so wires and bricks would mix the two up.
    DuplicateId { node: NodeId, modules: Vec<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortSide {
    Input,
    Output,
}

impl std::fmt::Display for PortSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PortSide::Input => "input",
            PortSide::Output => "output",
        })
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::UnknownClass { node, class } => {
                write!(f, "{node}: `{class}` is not a gate class in the inventory")
            }
            ValidationError::UnknownPort {
                node,
                class,
                port,
                side,
                valid,
            } => write!(
                f,
                "{node} ({class}) has no {side} port `{port}`; its {side} ports are: {}",
                port_list(valid)
            ),
            ValidationError::TypeMismatch {
                from,
                to,
                from_type,
                to_type,
            } => write!(
                f,
                "wire {}.{} -> {}.{} carries `{from_type}` into a `{to_type}` input; an exec pulse carries no value",
                from.node_id, from.port, to.node_id, to.port
            ),
            ValidationError::FanIn { target, sources } => write!(
                f,
                "{}.{} is fed by {} wires ({}); an input takes one",
                target.node_id,
                target.port,
                sources.len(),
                sources
                    .iter()
                    .map(|s| format!("{}.{}", s.node_id, s.port))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ValidationError::MissingNode {
                node,
                referenced_by,
            } => {
                write!(
                    f,
                    "{referenced_by} names {node}, which is not a node of this program"
                )
            }
            ValidationError::ChipIo {
                module,
                node,
                problem,
            } => write!(f, "module `{module}`, {node}: {problem}"),
            ValidationError::Coerce { node } => {
                write!(
                    f,
                    "{node} is a `coerce` node, which the backend does not emit; use a gate of the conversion's class"
                )
            }
            ValidationError::DuplicateId { node, modules } => {
                write!(
                    f,
                    "{node} is defined in more than one module: {}",
                    modules.join(", ")
                )
            }
        }
    }
}

/// A node's ports as an error lists them: comma-separated, or `none`.
pub(crate) fn port_list(ports: &[String]) -> String {
    if ports.is_empty() {
        "none".to_string()
    } else {
        ports.join(", ")
    }
}

/// Classes the backend handles that the inventory does not list.
fn is_pseudo_class(class: &str) -> bool {
    class == gc::LITERAL || class == gc::MICROCHIP
}

impl Program {
    /// Every broken invariant, in a stable order. A lowered Wirescript
    /// program has none.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut nodes: HashMap<NodeId, (&Node, Sym)> = HashMap::default();
        let mut dups: HashMap<NodeId, Vec<Sym>> = HashMap::default();
        collect(&self.module, &mut nodes, &mut dups);
        let mut errors = Vec::new();
        let mut dups: Vec<(NodeId, Vec<Sym>)> = dups.into_iter().collect();
        dups.sort_by_key(|(id, _)| *id);
        errors.extend(dups.into_iter().map(|(node, modules)| {
            ValidationError::DuplicateId {
                node,
                modules: modules
                    .into_iter()
                    .map(|m| resolve(m).to_string())
                    .collect(),
            }
        }));
        let nodes: HashMap<NodeId, &Node> = nodes.into_iter().map(|(id, (n, _))| (id, n)).collect();
        check_module(&self.module, &nodes, &mut errors, true);

        // Fan-in is across modules: a wire may be stored above its target.
        let mut feeds: HashMap<PortRef, Vec<PortRef>> = HashMap::default();
        let mut all_wires = Vec::new();
        wires(&self.module, &mut all_wires);
        for w in &all_wires {
            if w.target.port != WirePort::Layout {
                feeds.entry(w.target).or_default().push(w.source);
            }
        }
        let mut fan: Vec<(PortRef, Vec<PortRef>)> =
            feeds.into_iter().filter(|(_, s)| s.len() > 1).collect();
        fan.sort_by_key(|(t, _)| (t.node_id, t.port.as_str()));
        for (target, mut sources) in fan {
            sources.sort_by_key(|s| (s.node_id, s.port.as_str()));
            errors.push(ValidationError::FanIn { target, sources });
        }
        errors
    }
}

/// Every node with the module it is in, and each id found in more than one
/// module with all of their names.
fn collect<'a>(
    m: &'a Module,
    out: &mut HashMap<NodeId, (&'a Node, Sym)>,
    dups: &mut HashMap<NodeId, Vec<Sym>>,
) {
    for (id, n) in &m.nodes {
        if let Some((_, first)) = out.get(id) {
            dups.entry(*id).or_insert_with(|| vec![*first]).push(m.name);
        } else {
            out.insert(*id, (n, m.name));
        }
    }
    let mut ids: Vec<&NodeId> = m.chips.keys().collect();
    ids.sort();
    for id in ids {
        collect(&m.chips[id], out, dups);
    }
}

fn wires(m: &Module, out: &mut Vec<super::Wire>) {
    out.extend(m.wires.iter().copied());
    let mut ids: Vec<&NodeId> = m.chips.keys().collect();
    ids.sort();
    for id in ids {
        wires(&m.chips[id], out);
    }
}

fn port_names(ps: &[super::PortSpec]) -> Vec<String> {
    ps.iter().map(|p| resolve(p.name).to_string()).collect()
}

fn check_module(
    m: &Module,
    all: &HashMap<NodeId, &Node>,
    errors: &mut Vec<ValidationError>,
    root: bool,
) {
    let catalog = crate::catalog::default_catalog();
    let module_name = resolve(m.name).to_string();
    let mut ids: Vec<&NodeId> = m.nodes.keys().collect();
    ids.sort();

    for id in &ids {
        let n = &m.nodes[*id];
        if catalog.find_by_class(n.gate_class).is_none() && !is_pseudo_class(n.gate_class) {
            errors.push(ValidationError::UnknownClass {
                node: n.id,
                class: n.gate_class.to_string(),
            });
        }
        if n.kind == NodeKind::Coerce {
            errors.push(ValidationError::Coerce { node: n.id });
        }
    }

    // A wired port of a plain gate must be a wire port of its inventory
    // class. Declared ports may hold more (a gate's settings are ports too,
    // and `DisplayText` splits `Position` into `Position.X`/`.Y`); pins,
    // chips and loop shells take their ports from what they carry.
    let inventory_port = |n: &Node, port: WirePort, side: PortSide| -> Option<ValidationError> {
        if !matches!(n.kind, NodeKind::Gate | NodeKind::Event) {
            return None;
        }
        let spec = catalog.find_by_class(n.gate_class)?;
        if catalog.has_wire_port(n.gate_class, side, port.as_str()) {
            return None;
        }
        let inv = if side == PortSide::Input {
            &spec.component.inputs
        } else {
            &spec.component.outputs
        };
        Some(ValidationError::UnknownPort {
            node: n.id,
            class: n.gate_class.to_string(),
            port: port.as_str().to_string(),
            side,
            valid: inv.iter().map(|q| q.name.clone()).collect(),
        })
    };

    for w in &m.wires {
        let (Some(src), Some(dst)) = (all.get(&w.source.node_id), all.get(&w.target.node_id))
        else {
            for end in [w.source.node_id, w.target.node_id] {
                if !all.contains_key(&end) {
                    errors.push(ValidationError::MissingNode {
                        node: end,
                        referenced_by: format!("a wire in module `{module_name}`"),
                    });
                }
            }
            continue;
        };
        if w.source.port == WirePort::Layout && w.target.port == WirePort::Layout {
            continue;
        }
        let out = src
            .ports
            .outputs
            .iter()
            .find(|p| p.name == w.source.port.sym());
        let inp = dst
            .ports
            .inputs
            .iter()
            .find(|p| p.name == w.target.port.sym());
        if out.is_none() {
            errors.push(ValidationError::UnknownPort {
                node: src.id,
                class: src.gate_class.to_string(),
                port: w.source.port.as_str().to_string(),
                side: PortSide::Output,
                valid: port_names(&src.ports.outputs),
            });
        }
        if inp.is_none() {
            errors.push(ValidationError::UnknownPort {
                node: dst.id,
                class: dst.gate_class.to_string(),
                port: w.target.port.as_str().to_string(),
                side: PortSide::Input,
                valid: port_names(&dst.ports.inputs),
            });
        }
        if out.is_some() {
            errors.extend(inventory_port(src, w.source.port, PortSide::Output));
        }
        if inp.is_some() {
            errors.extend(inventory_port(dst, w.target.port, PortSide::Input));
        }
        // A value into an exec input is legal (the change fires it); an exec
        // pulse into a value input carries nothing.
        if let (Some(o), Some(i)) = (out, inp)
            && o.ty == Type::Exec
            && i.ty != Type::Exec
            && i.ty != Type::Any
        {
            errors.push(ValidationError::TypeMismatch {
                from: w.source,
                to: w.target,
                from_type: o.ty.clone(),
                to_type: i.ty.clone(),
            });
        }
    }

    for (host, src) in &m.dynamic_labels {
        for id in [*host, src.node_id] {
            if !all.contains_key(&id) {
                errors.push(ValidationError::MissingNode {
                    node: id,
                    referenced_by: format!("a label in `{module_name}`"),
                });
            }
        }
    }
    if root
        && let Some(p) = m.root_dynamic_label
        && !all.contains_key(&p.node_id)
    {
        errors.push(ValidationError::MissingNode {
            node: p.node_id,
            referenced_by: "the root label".into(),
        });
    }
    for (what, map) in [
        ("@color", &m.dynamic_colors),
        ("@material", &m.dynamic_materials),
    ] {
        for (host, src) in map {
            for id in [*host, src.node_id] {
                if !all.contains_key(&id) {
                    errors.push(ValidationError::MissingNode {
                        node: id,
                        referenced_by: format!("a {what} in `{module_name}`"),
                    });
                }
            }
        }
    }
    if root {
        for p in m
            .root_style
            .color
            .iter()
            .chain(m.root_style.material.iter())
            .filter_map(crate::ir::StyleValue::wired)
        {
            if !all.contains_key(&p.node_id) {
                errors.push(ValidationError::MissingNode {
                    node: p.node_id,
                    referenced_by: "the root style".into(),
                });
            }
        }
    }

    // Inputs and outputs: each listed once, and each a pin of this module.
    // (A pin may be left off the list: lowering does that for pins it
    // reaches only by wire.)
    for (list, kind, what) in [
        (&m.inputs, NodeKind::Input, "input"),
        (&m.outputs, NodeKind::Output, "output"),
    ] {
        let mut seen = crate::collections::HashSet::default();
        for id in list {
            let problem = match m.nodes.get(id) {
                None => Some(format!("listed as an {what} but not a node of this module")),
                Some(n) if n.kind != kind => {
                    Some(format!("listed as an {what} but is a {:?} node", n.kind))
                }
                Some(_) if !seen.insert(*id) => Some(format!("listed as an {what} twice")),
                Some(_) => None,
            };
            if let Some(problem) = problem {
                errors.push(ValidationError::ChipIo {
                    module: module_name.clone(),
                    node: *id,
                    problem,
                });
            }
        }
    }

    let mut chip_ids: Vec<&NodeId> = m.chips.keys().collect();
    chip_ids.sort();
    for id in chip_ids {
        match m.nodes.get(id) {
            None => errors.push(ValidationError::ChipIo {
                module: module_name.clone(),
                node: *id,
                problem: "has a chip body but is not a node of this module".into(),
            }),
            Some(n) if n.kind != NodeKind::Chip => errors.push(ValidationError::ChipIo {
                module: module_name.clone(),
                node: *id,
                problem: format!("has a chip body but is a {:?} node", n.kind),
            }),
            Some(_) => {}
        }
        check_module(&m.chips[id], all, errors, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{NodeId, PortRef, ProgramBuilder, Wire};
    use crate::wsir::parse_program;

    fn parse(src: &str) -> Program {
        parse_program(src, "t.wsir").unwrap_or_else(|e| panic!("{e}"))
    }

    fn kinds(p: &Program) -> Vec<&'static str> {
        p.validate()
            .iter()
            .map(|e| match e {
                ValidationError::UnknownClass { .. } => "class",
                ValidationError::UnknownPort { .. } => "port",
                ValidationError::TypeMismatch { .. } => "type",
                ValidationError::FanIn { .. } => "fan-in",
                ValidationError::MissingNode { .. } => "missing",
                ValidationError::ChipIo { .. } => "chip-io",
                ValidationError::Coerce { .. } => "coerce",
                ValidationError::DuplicateId { .. } => "duplicate-id",
            })
            .collect()
    }

    const AND: &str = "source_filename = \"and.ws\"\nmodule @and {\n  %a = in bool\n  %b = in bool\n  %g = Expr_LogicalAND(bInputA: %a, bInputB: %b)\n  out %y bool = %g\n}\n";

    #[test]
    fn a_valid_program_has_no_errors() {
        assert_eq!(kinds(&parse(AND)), Vec::<&str>::new());
    }

    #[test]
    fn the_builder_and_the_text_make_the_same_ir() {
        let mut b = ProgramBuilder::new("and");
        b.settings_mut().source_file = Some("and.ws".into());
        let root = b.root();
        let a = b.input(root, "a", Type::Bool);
        let c = b.input(root, "b", Type::Bool);
        let g = b.gate(root, "Expr_LogicalAND").unwrap();
        b.name(g, "g");
        let y = b.output(root, "y", Type::Bool);
        b.wire(a, "RER_Output", g, "bInputA").unwrap();
        b.wire(c, "RER_Output", g, "bInputB").unwrap();
        b.wire(g, "bOutput", y, "RER_Input").unwrap();
        let built = b.build();
        assert_eq!(built.diff(&parse(AND)), None);
        assert_eq!(
            crate::wsir::program_to_string(&built).unwrap(),
            crate::wsir::program_to_string(&parse(AND)).unwrap()
        );
    }

    /// A pin made with type `any` takes the type of what feeds it, on both
    /// sides, as an untyped `in` does in the text, and a gate it feeds sees
    /// that type.
    #[test]
    fn the_builder_types_an_any_pin_like_the_text() {
        let mut b = ProgramBuilder::new("m");
        let root = b.root();
        let x = b.input(root, "x", Type::Bool);
        let (c, body) = b.chip(root);
        b.name(c, "c");
        let p = b.input(body, "p", Type::Any);
        let g = b.gate(body, "Expr_LogicalAND").unwrap();
        b.name(g, "g");
        // Wired gate-first, so the pin's type arrives after the gate's wire.
        b.wire(p, "RER_Output", g, "bInputA").unwrap();
        b.wire(x, "RER_Output", p, "RER_Input").unwrap();
        let text = "module @m {\n  %x = in bool\n  %c = chip {\n    %p = in(%x)\n    %g = Expr_LogicalAND(bInputA: %p)\n  }\n}\n";
        assert_eq!(b.build().diff(&parse(text)), None);
    }

    #[test]
    fn the_builder_refuses_what_validation_would_flag() {
        let mut b = ProgramBuilder::new("m");
        let root = b.root();
        assert!(matches!(
            b.gate(root, "NoSuchGate"),
            Err(crate::ir::BuildError::UnknownClass(_))
        ));
        let a = b.input(root, "a", Type::Bool);
        let g = b.gate(root, "Expr_LogicalAND").unwrap();
        let err = b.wire(a, "RER_Output", g, "bNope").unwrap_err();
        assert!(err.to_string().contains("bInputA, bInputB"), "{err}");
        b.wire(a, "RER_Output", g, "bInputA").unwrap();
        assert!(matches!(
            b.wire(a, "RER_Output", g, "bInputA"),
            Err(crate::ir::BuildError::FanIn { .. })
        ));
    }

    #[test]
    fn unknown_class() {
        let p = parse("module @m {\n  %x = NoSuchGate\n}\n");
        assert_eq!(kinds(&p), ["class"]);
        assert!(p.validate()[0].to_string().contains("NoSuchGate"));
    }

    #[test]
    fn unknown_port() {
        // A wired port the inventory class does not have.
        let p = parse(
            "module @m {\n  %a = in bool\n  %g = Expr_LogicalAND(Exec: %a) : (bInputA: bool, Exec: bool) -> (bOutput: bool)\n}\n",
        );
        assert_eq!(kinds(&p), ["port"]);
        let msg = p.validate()[0].to_string();
        assert!(
            msg.contains("`Exec`") && msg.contains("bInputA, bInputB"),
            "{msg}"
        );
        // A wire end on a port its node does not declare.
        let mut p = parse(AND);
        p.module.wires[0].target.port = WirePort::from_name("Exec");
        assert!(kinds(&p).contains(&"port"), "{:?}", p.validate());
    }

    #[test]
    fn type_mismatch() {
        let p = parse("module @m {\n  %go = in exec\n  %g = Expr_LogicalAND(bInputA: %go)\n}\n");
        let errs = p.validate();
        assert_eq!(kinds(&p), ["type"]);
        let msg = errs[0].to_string();
        assert!(msg.contains("`exec`") && msg.contains("`bool`"), "{msg}");
    }

    #[test]
    fn fan_in() {
        let mut p = parse(AND);
        let extra = Wire {
            source: PortRef {
                node_id: NodeId(2),
                port: WirePort::RerOutput,
            },
            target: PortRef {
                node_id: NodeId(3),
                port: WirePort::from_name("bInputA"),
            },
        };
        p.module.wires.push(extra);
        assert_eq!(kinds(&p), ["fan-in"]);
        assert!(
            p.validate()[0]
                .to_string()
                .contains("n1.RER_Output, n2.RER_Output")
        );
    }

    #[test]
    fn missing_node() {
        let mut p = parse(AND);
        p.module.wires[0].source.node_id = NodeId(99);
        assert_eq!(kinds(&p), ["missing"]);
        assert!(p.validate()[0].to_string().contains("n99"));
    }

    #[test]
    fn chip_io() {
        // An input listed twice.
        let mut p = parse(AND);
        p.module.inputs.push(NodeId(1));
        assert_eq!(kinds(&p), ["chip-io"]);
        // A gate listed as an output.
        let mut p = parse(AND);
        p.module.outputs.push(NodeId(3));
        assert_eq!(kinds(&p), ["chip-io"]);
        // A chip body under a node that is not a chip.
        let mut p = parse("module @m {\n  %c = chip { }\n}\n");
        p.module.nodes.get_mut(&NodeId(1)).unwrap().kind = NodeKind::Gate;
        assert_eq!(kinds(&p), ["chip-io"]);
    }

    #[test]
    fn coerce_node() {
        let p = parse(
            "module @m {\n  %a = in bool\n  %c = Expr_LogicalAND(bInputA: %a) !kind coerce\n}\n",
        );
        assert_eq!(kinds(&p), ["coerce"]);
        assert!(
            p.validate()[0].to_string().contains("coerce"),
            "{}",
            p.validate()[0]
        );
    }

    #[test]
    fn duplicate_id() {
        let mut p = parse(
            "module @m {\n  %g = Expr_LogicalAND\n  %c = chip @body {\n    %h = Expr_LogicalOR\n  }\n}\n",
        );
        let g = p.module.nodes[&NodeId(1)].clone();
        p.module
            .chips
            .get_mut(&NodeId(2))
            .unwrap()
            .nodes
            .insert(NodeId(1), g);
        assert_eq!(kinds(&p), ["duplicate-id"]);
        assert!(
            p.validate()[0].to_string().contains("m, body"),
            "{}",
            p.validate()[0]
        );
    }
}
