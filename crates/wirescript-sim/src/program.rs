//! A Wirescript program, simulated as the save it compiles to: assembled in
//! memory, written, and read back through the same loader as any `.brz`.
//! Each gate keeps the IR node it came from, so errors name source lines.

use brdb::World;
use wirescript_ir::{EmitOptions, Module, NodeId, Program, SourceRange, exterior};

use crate::collections::HashMap;
use crate::display::DisplayMeta;
use crate::graph::Graph;
use crate::load::{BrickAddr, LoadError, read_brz};

#[derive(Debug, thiserror::Error)]
pub enum ProgramError {
    #[error("emit: {0}")]
    Emit(#[from] wirescript_ir::EmitError),
    #[error("writing the save: {0}")]
    Write(#[from] brdb::BrError),
    #[error("{0}")]
    Load(#[from] LoadError),
}

/// The graph `program` runs as, with every gate tagged by its IR node.
pub fn graph_from_program(program: &Program, opts: &EmitOptions) -> Result<Graph, ProgramError> {
    let assembly = wirescript_ir::assemble(program, opts)?;
    graph_from_world(&assembly.world, &assembly.node_bricks, program)
}

/// The graph of `world`, already assembled from `program` with
/// `node_bricks` the brick each node became.
pub fn graph_from_world(
    world: &World,
    node_bricks: &HashMap<NodeId, usize>,
    program: &Program,
) -> Result<Graph, ProgramError> {
    let bytes = world.to_brz_vec()?;
    let mut graph = Graph::build(read_brz(&bytes)?)?;

    let addrs = brick_addresses(world);
    let mut spans = HashMap::default();
    node_spans(&program.module, &mut spans);
    let mut on_brick: HashMap<BrickAddr, Vec<u32>> = HashMap::default();
    for (&(addr, _), &g) in &graph.by_addr {
        on_brick.entry(addr).or_default().push(g);
    }
    let mut node_gates: HashMap<NodeId, Vec<u32>> = HashMap::default();
    for (node, brick) in node_bricks {
        let Some(addr) = addrs.get(brick) else {
            continue;
        };
        for &g in on_brick.get(addr).into_iter().flatten() {
            let gate = &mut graph.gates[g as usize];
            gate.node = Some(*node);
            gate.span = spans.get(node).cloned();
            node_gates.entry(*node).or_default().push(g);
        }
    }
    tag_exterior(&mut graph, &program.module, &node_gates);
    Ok(graph)
}

/// Name each `@seat` gate and list each `@display`'s row gates, from the
/// root module's exterior nodes.
fn tag_exterior(graph: &mut Graph, module: &Module, node_gates: &HashMap<NodeId, Vec<u32>>) {
    let class_gate = |node: &NodeId, class: &str| {
        node_gates
            .get(node)
            .into_iter()
            .flatten()
            .copied()
            .find(|&g| graph.gates[g as usize].class.ends_with(class))
    };
    let mut seats = Vec::new();
    let mut rows: HashMap<String, Vec<(u32, u32)>> = HashMap::default();
    for (id, node) in module.nodes_in_order() {
        if let Some((name, _)) = exterior::seat_of(node) {
            if let Some(g) = class_gate(id, "Internal_Seat") {
                seats.push((g, name));
            }
        } else if let Some((name, row)) = exterior::row_of(node)
            && let Some(g) = class_gate(id, "TextDisplay")
        {
            rows.entry(name).or_default().push((row, g));
        }
    }
    for (g, name) in seats {
        graph.gates[g as usize].seat = Some(name);
    }
    for (_, node) in module.nodes_in_order() {
        let Some(d) = exterior::DisplayInfo::of(node) else {
            continue;
        };
        let mut gates = vec![None; d.height as usize];
        for (row, g) in rows.remove(&d.name).unwrap_or_default() {
            if let Some(slot) = gates.get_mut(row as usize) {
                *slot = Some(g);
            }
        }
        graph.displays.push(DisplayMeta {
            name: d.name,
            width: d.width,
            height: d.height,
            rows: gates,
        });
    }
}

/// Where the writer puts each brick of `world`: the main grid is grid 1, the
/// `n`th entity grid is grid `n + 2`, and a brick's index counts the bricks
/// before it in the same chunk.
fn brick_addresses(world: &World) -> HashMap<usize, BrickAddr> {
    let mut out = HashMap::default();
    let grids = std::iter::once((1u32, &world.bricks)).chain(
        world
            .grids
            .iter()
            .enumerate()
            .map(|(i, (_, bricks))| (i as u32 + 2, bricks)),
    );
    for (grid, bricks) in grids {
        let mut counts: HashMap<(i16, i16, i16), u32> = HashMap::default();
        for b in bricks {
            let chunk = b.position.to_relative().0;
            let key = (chunk.x, chunk.y, chunk.z);
            let index = counts.entry(key).or_insert(0);
            if let Some(id) = b.id {
                out.insert(id, BrickAddr::new(grid, chunk, *index));
            }
            *index += 1;
        }
    }
    out
}

fn node_spans(module: &Module, out: &mut HashMap<NodeId, SourceRange>) {
    for (id, node) in module.nodes_in_order() {
        out.insert(*id, node.source_range.clone());
    }
    for chip in module.chips.values() {
        node_spans(chip, out);
    }
}
