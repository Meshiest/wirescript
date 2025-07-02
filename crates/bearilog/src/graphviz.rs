use std::{error::Error, io::Write, sync::atomic};

use super::compiler::{CompiledModule, CompiledOutput, Gate, Wire};

pub fn render(module: &CompiledModule) -> Result<String, Box<dyn Error>> {
    let mut f = vec![];

    subgraph(module, "module", vec![], &mut f, 0)?;

    Ok(String::from_utf8(f)?)
}

fn subgraph(
    module: &CompiledModule,
    name: &str,
    prefix: Vec<usize>,
    f: &mut impl Write,
    depth: usize,
) -> Result<(), Box<dyn Error>> {
    static CONST_INDEX: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

    let root_pad = "  ".repeat(depth);
    let pad = "  ".repeat(depth + 1);

    if depth == 0 {
        writeln!(f, "{root_pad}digraph {name} {{")?;
        // writeln!(f, "{pad}graph [rankdir=LR];")?;
        // writeln!(f, "{pad}graph [splines=ortho];")?; // (add hard lines)

        // The top level module inputs are rendered as nodes
        for i in 0..module.num_inputs {
            writeln!(f, "{pad}in{i} [style=filled,color=lightblue];",)?;
        }
        // Align inputs on the same level
        if module.num_inputs > 1 {
            writeln!(
                f,
                "{pad}{{rank=same; {}}}",
                (0..module.num_inputs)
                    .map(|i| format!("in{i}"))
                    .collect::<Vec<_>>()
                    .join(";")
            )?;
        }
        // Align outputs on the same level
        if module.outputs.len() > 1 {
            writeln!(
                f,
                "{pad}{{rank=same; {}}}",
                (0..module.outputs.len())
                    .map(|i| format!("out{i}"))
                    .collect::<Vec<_>>()
                    .join(";")
            )?;
        }
    } else {
        let idx = CONST_INDEX.fetch_add(1, atomic::Ordering::SeqCst);
        writeln!(f, "{root_pad}subgraph cluster_{idx} {{")?;
        writeln!(f, "{pad}label=<<b>{name}</b>>; color=black;\n")?;
    }

    // Ids of input/output gates to vertically align together
    let mut local_input_ids = vec![];
    let mut local_output_ids = vec![];

    // Write the gates as nodes
    for gate in &module.gates {
        write!(f, "{pad}{}", render_gate(gate))?;
        if gate.meta.input_index.is_some() {
            local_input_ids.push(gate.to_string());
        }
        if gate.meta.output_index.is_some() {
            local_output_ids.push(gate.to_string());
        }
    }
    if local_input_ids.len() > 1 {
        writeln!(f, "{pad}{{rank=same; {}}}", local_input_ids.join(";"))?;
    }
    if local_output_ids.len() > 1 {
        writeln!(f, "{pad}{{rank=same; {}}}", local_output_ids.join(";"))?;
    }

    writeln!(f)?;

    // Render the subgraphs for sub-modules
    for (mod_idx, (name, module)) in module.sub_modules.iter().enumerate() {
        let mod_prefix = [prefix.clone(), vec![mod_idx]].concat();
        subgraph(module, name, mod_prefix, f, depth + 1)?;
    }

    for Wire { src, dst } in &module.wires {
        writeln!(
            f,
            "{pad}{}:{} -> {}:{};",
            src.gate, src.property, dst.gate, dst.property
        )?;
    }

    // Render inputs and outputs for the root module
    if depth == 0 {
        // Connect the inputs for the root module
        // This happens here rather than up top because the gates may not exist yet.
        for (i, w) in &module.inputs {
            writeln!(f, "{pad}in{i} -> {}:{};", w.gate, w.property)?;
        }

        // Connect outputs for the root module
        for (i, out) in module.outputs.iter().enumerate() {
            writeln!(f, "{pad}out{i} [style=filled,color=lightgreen];")?;
            let name = match out {
                CompiledOutput::Input(n) => format!("in{n}"),
                CompiledOutput::Literal(literal) => {
                    let lit_idx = CONST_INDEX.fetch_add(1, atomic::Ordering::SeqCst);
                    let lit = format!("lit{lit_idx}");
                    writeln!(
                        f,
                        "{pad}{lit} [label=\"{literal}\",style=filled,color=white];"
                    )?;
                    lit
                }
                CompiledOutput::Wire(w) => format!("{}:{}", w.gate, w.property),
            };
            writeln!(f, "{pad}{name} -> out{i};")?;
        }
    }

    if !module.gate_literals.is_empty() {
        writeln!(f)?;
    }
    for (wc, literal) in &module.gate_literals {
        let lit_idx = CONST_INDEX.fetch_add(1, atomic::Ordering::SeqCst);
        let lit = format!("lit{lit_idx}");
        writeln!(
            f,
            "{pad}{lit} [label=\"{literal}\",style=filled,color=white];"
        )?;
        writeln!(f, "{pad}{lit} -> {}:{};", wc.gate, wc.property)?;
    }

    Ok(writeln!(f, "{root_pad}}}")?)
}

fn render_gate(gate: &Gate) -> String {
    let (inputs, outputs) = gate.kind.properties();
    let one_input = inputs.len() == 1;
    let one_output = outputs.len() == 1;

    let mut inputs = inputs.into_iter();
    let mut outputs = outputs.into_iter();

    let mut label = gate
        .meta
        .label
        .clone()
        .unwrap_or_else(|| gate.kind.to_string());
    label = match (one_input, one_output) {
        (true, true) => format!(
            "<{},{}>{label}",
            inputs.next().unwrap(),
            outputs.next().unwrap()
        ),
        (true, false) => format!("<{}>{label}", inputs.next().unwrap()),
        (false, true) => format!("<{}>{label}", outputs.next().unwrap()),
        (false, false) => label,
    };

    let input_ports = inputs
        .map(|i| format!("<{i}>{i}"))
        .collect::<Vec<_>>()
        .join("|");
    let output_ports = outputs
        .map(|i| format!("<{i}>{i}"))
        .collect::<Vec<_>>()
        .join("|");

    // This will format the gates as blocks
    format!(
        "{gate} [label=\"{{{}{label}{}}}\",shape=record{io}]",
        // If there is only one input or output, the port is on the node name
        if one_input {
            "".to_string()
        } else {
            format!("{{{input_ports}}}|")
        },
        if one_output {
            "".to_string()
        } else {
            format!("|{{{output_ports}}}")
        },
        // Use the gate label if it exists, otherwise use the gate's kind
        io = if gate.meta.input_index.is_some() {
            ",style=filled,color=lightblue"
        } else if gate.meta.output_index.is_some() {
            ",style=filled,color=lightgreen"
        } else {
            ""
        },
    )
}
