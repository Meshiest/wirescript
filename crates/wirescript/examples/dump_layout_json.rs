//! SCRATCH TOOL (layout-playground prototyping — not for commit).
//! Dumps a compiled module's full wire graph + the CURRENT layout's
//! placements as JSON, for the HTML layout playground.
//!
//! Usage:
//!   cargo run --release -p wirescript --example dump_layout_json -- <file.ws> [out.json]

use std::collections::HashMap;

use serde_json::{Value, json};
use wirescript::catalog::default_catalog;
use wirescript::ir::{Literal, Module, Node, Type};
use wirescript::layout::{LayoutResult, layout};
use wirescript::lower::{LowerInput, lower};
use wirescript::resolve::{FsLoader, resolve};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::typecheck;

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".into(),
        Type::Int => "int".into(),
        Type::Float => "float".into(),
        Type::String => "string".into(),
        Type::Vector => "vector".into(),
        Type::Rotator => "rotator".into(),
        Type::Quat => "quat".into(),
        Type::Color => "color".into(),
        Type::Entity => "entity".into(),
        Type::Character => "character".into(),
        Type::Controller => "controller".into(),
        Type::Brick => "brick".into(),
        Type::Prefab => "prefab".into(),
        Type::Exec => "exec".into(),
        Type::Any => "any".into(),
        Type::Never => "never".into(),
        Type::Ref(t) => format!("ref {}", type_name(t)),
        Type::Array(t) => format!("{}[]", type_name(t)),
        Type::Union(_) => "union".into(),
        Type::Tuple(_) => "tuple".into(),
        Type::Record(_) => "record".into(),
    }
}

fn prop_string(node: &Node, key: wirescript::intern::Sym) -> Option<String> {
    match node.properties.get(&key) {
        Some(Literal::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn node_json(node: &Node, lr: &LayoutResult) -> Value {
    let cat = default_catalog();
    let spec = cat.find_by_class(node.gate_class);
    let (hx, hy) = spec.map(|g| (g.half_size.x, g.half_size.y)).unwrap_or((5, 5));
    let display = spec
        .map(|g| g.component.display_name.clone())
        .unwrap_or_else(|| node.gate_class.rsplit('_').next().unwrap_or("?").to_string());

    let ports = |list: &[wirescript::ir::PortSpec]| -> Vec<Value> {
        list.iter()
            .map(|p| {
                json!({
                    "name": wirescript::intern::resolve(p.name),
                    "ty": type_name(&p.ty),
                })
            })
            .collect()
    };

    let placement = lr.placements.get(&node.id);
    json!({
        "id": node.id.0,
        "kind": format!("{:?}", node.kind),
        "class": node.gate_class,
        "display": display,
        "label": prop_string(node, *wirescript::intern::sym::NAME_LABEL),
        "portLabel": prop_string(node, *wirescript::intern::sym::PORT_LABEL),
        "note": node.note,
        "chain": node.chain_id,
        "scope": node.scope_id,
        "src": node.source_range.start.offset,
        "half": [hx, hy],
        "inputs": ports(&node.ports.inputs),
        "outputs": ports(&node.ports.outputs),
        // Current engine's placement: x = row axis, y = column/depth axis.
        "cur": placement.map(|p| json!([p.x, p.y])),
    })
}

/// One pane = one module (root or a chip body).
fn pane_json(name: &str, module: &Module, lr: &LayoutResult, out: &mut Vec<Value>) {
    let mut nodes: Vec<Value> = module.nodes.values().map(|n| node_json(n, lr)).collect();
    nodes.sort_by_key(|n| n["id"].as_u64());

    // Source-port type per node/port so wires can be colored by type.
    let out_ty: HashMap<(u64, &str), String> = module
        .nodes
        .values()
        .flat_map(|n| {
            n.ports.outputs.iter().map(move |p| {
                (
                    (n.id.0 as u64, wirescript::intern::resolve(p.name)),
                    type_name(&p.ty),
                )
            })
        })
        .collect();

    let wires: Vec<Value> = module
        .wires
        .iter()
        .map(|w| {
            let sp = w.source.port.as_str();
            json!({
                "from": [w.source.node_id.0, sp],
                "to": [w.target.node_id.0, w.target.port.as_str()],
                "ty": out_ty.get(&(w.source.node_id.0 as u64, sp)),
            })
        })
        .collect();

    out.push(json!({
        "name": name,
        "nodes": nodes,
        "wires": wires,
        "bounds": [
            [lr.bounds_min.x, lr.bounds_min.y],
            [lr.bounds_max.x, lr.bounds_max.y],
        ],
    }));

    for (chip_id, child) in &module.chips {
        let chip_name = module
            .nodes
            .get(chip_id)
            .and_then(|n| prop_string(n, *wirescript::intern::sym::NAME_LABEL))
            .unwrap_or_else(|| {
                let mname = wirescript::intern::resolve(child.name);
                if mname.is_empty() {
                    format!("chip#{}", chip_id.0)
                } else {
                    format!("{mname}#{}", chip_id.0)
                }
            });
        let child_lr = lr
            .chip_layouts
            .get(chip_id)
            .cloned()
            .unwrap_or_default();
        pane_json(&format!("{name}/{chip_name}"), child, &child_lr, out);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("usage: dump_layout_json <file.ws> [out.json]");
    let out_path = args.next();

    let source = std::fs::read_to_string(&file).expect("read source");
    let resolved = resolve(&source, &file, &FsLoader);
    let tc = typecheck(&resolved.ast, &file);
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: &file,
        module_name: None,
        template_cache: std::sync::Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
    });
    let errs: Vec<_> = lowered
        .diagnostics
        .iter()
        .filter(|d| matches!(d.severity, wirescript::Severity::Error))
        .collect();
    if !errs.is_empty() {
        eprintln!("lower errors: {errs:?}");
        std::process::exit(1);
    }

    let lr = layout(&lowered.module);

    let stem = std::path::Path::new(&file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "sample".into());
    let mut panes = Vec::new();
    pane_json(&stem, &lowered.module, &lr, &mut panes);

    let doc = json!({ "sample": stem, "panes": panes });
    let text = serde_json::to_string(&doc).expect("serialize");
    match out_path {
        Some(p) => {
            std::fs::write(&p, &text).expect("write out");
            eprintln!("wrote {p} ({} bytes, {} panes)", text.len(), doc["panes"].as_array().unwrap().len());
        }
        None => println!("{text}"),
    }
}
