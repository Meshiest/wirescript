use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{
    bearilog::compiler::{CompiledModule, Gate, GateKind},
    brdb::{Brick, WireConnection, World},
    builder::errors::BuilderError,
};

mod errors;

#[derive(Default)]
struct BuilderContext {
    wires: Vec<WireConnection>,
    bricks: Vec<Brick>,
    /// A map of gate index to brick ID
    seen: HashMap<usize, usize>,
    /// A map of wire source gate index to destination gate index
    dst_to_src: HashMap<usize, usize>,
    /// A map of wire source gate index to the gate itself
    gates: HashMap<usize, Arc<Gate>>,
}

impl BuilderContext {
    /// Add another context into this one
    pub fn extend(&mut self, other: BuilderContext) {
        self.wires.extend(other.wires);
        self.bricks.extend(other.bricks);
        self.seen.extend(other.seen);
        self.dst_to_src.extend(other.dst_to_src);
    }
}

struct Bounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

// TODO: recurse submodules and generate bounds for each submodule
// TODO: gates that do not depend on eachother can be placed as siblings
// TODO: gates that depend on another are placed after the gate they depend on
// TODO: groups of gates share bounds, so that they can be placed together on a baseplate

/// Obtain a map of src gate index to dst gate index, ignoring buffers as destinations
fn build_dst_to_src(module: &CompiledModule) -> HashMap<usize, usize> {
    module
        .wires
        .iter()
        .filter_map(|w| {
            (!matches!(w.dst.gate.kind, GateKind::Buffer))
                .then_some((w.dst.gate.index, w.src.gate.index))
        })
        .chain(
            module
                .sub_modules
                .iter()
                .flat_map(|(_, sub_module)| build_dst_to_src(sub_module).into_iter()),
        )
        .collect()
}

/// Iterate every single gate in the module and return its index and the gate itself.
fn build_gate_map(module: &CompiledModule) -> HashMap<usize, Arc<Gate>> {
    module
        .gates
        .iter()
        .map(|g| (g.index, Arc::clone(g)))
        .chain(
            module
                .sub_modules
                .iter()
                .flat_map(|(_, sub_module)| build_gate_map(sub_module).into_iter()),
        )
        .collect::<HashMap<_, _>>()
}

pub fn module_to_world(module: CompiledModule) -> Result<World, BuilderError> {
    let mut world = World::new();

    let mut ctx = BuilderContext {
        dst_to_src: build_dst_to_src(&module),
        gates: build_gate_map(&module),
        ..Default::default()
    };

    Ok(world)
}
