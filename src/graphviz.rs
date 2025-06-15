use std::{error::Error, io::Write, sync::atomic};

use crate::compiler::{CompiledModule, CompiledOutput, Wire};

pub fn render(module: &CompiledModule) -> Result<String, Box<dyn Error>> {
    let mut f = vec![];

    subgraph(module, "module", vec![], &mut f, 0)?;

    Ok(String::from_utf8(f)?)
}

pub fn subgraph(
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
        // graph [splines=ortho]; // (add hard lines)

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
        writeln!(f, "{pad}label=\"{name}\"; color=black;\n")?;
    }

    // Ids of input/output gates to vertically align together
    let mut local_input_ids = vec![];
    let mut local_output_ids = vec![];

    // Write the gates as nodes
    for gate in &module.gates {
        let (inputs, outputs) = gate.kind.properties();
        let one_input = inputs.len() == 1;
        let one_output = outputs.len() == 1;

        let mut inputs = inputs.into_iter().peekable();
        let mut outputs = outputs.into_iter().peekable();

        // <input> <output> or <input,output> if there is only one input/output
        let first_labels = match (one_input, one_output) {
            (true, true) => format!("<{},{}>", inputs.next().unwrap(), outputs.next().unwrap()),
            (true, false) => format!("<{}>", inputs.next().unwrap()),
            (false, true) => format!("<{}>", outputs.next().unwrap()),
            (false, false) => String::new(),
        };

        // If there are no inputs/outputs, we don't need a divider
        let ports_divider = if inputs.peek().is_some() || outputs.peek().is_some() {
            "|"
        } else {
            ""
        };

        let mut ports = inputs
            .map(|i| format!("<{i}> {i}"))
            .chain(outputs.map(|o| format!("<{o}> {o}")))
            .collect::<Vec<_>>()
            .join("|");
        if !ports.is_empty() {
            // Wrap the ports in curly braces to stack them vertically
            ports = format!("{{{}}}", ports);
        }

        // This will format the gates as blocks
        writeln!(
            f,
            "{pad}{gate} [label=\"{ports}{ports_divider}{first_labels}{display}\",shape=record{io}]",
            // Use the gate label if it exists, otherwise use the gate's kind
            display = gate
                .meta
                .label
                .clone()
                .unwrap_or_else(|| gate.kind.to_string()),
            io = if gate.meta.is_input {
                ",style=filled,color=lightblue"
            } else if gate.meta.is_output {
                ",style=filled,color=lightgreen"
            } else {
                ""
            },
        )?;

        if gate.meta.is_input {
            local_input_ids.push(gate.to_string());
        }
        if gate.meta.is_output {
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
        // Load all the gates from the submodule
        subgraph(module, name, mod_prefix, f, depth + 1)?;
    }

    // Outputs don't need to be rendered because they are metadata for other
    // nodes to connect to.

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
                CompiledOutput::Immediate(literal) => {
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
