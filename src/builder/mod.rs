use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{
    bearilog::compiler::{self, CompiledModule, CompiledOutput, Gate, GateKind},
    brdb::{
        self, Brick, Color, Position, WireConnection, WirePort, World,
        assets::{
            bricks::B_GATE_CONSTANT,
            components::{LogicGate, LogicGateComponent},
        },
        schema::as_brdb::AsBrdbValue,
    },
    builder::errors::BuilderError,
};

mod errors;

struct BuilderContext<'a> {
    wires: Vec<WireConnection>,
    bricks: Vec<Brick>,
    /// A map of gate index to brick ID
    seen: HashSet<usize>,
    wire_map: &'a WireMap,
    options: BuilderOptions,
}

type WireMap = HashMap<usize, HashSet<usize>>;

enum QueueItem {
    Module(CompiledModule),
    Gate(Arc<Gate>),
    NextRow(usize),
}

#[derive(Debug, Clone, Default, Copy, clap::Args)]
pub struct BuilderOptions {
    /// Space between gates vertically
    #[arg(long, default_value = "0")]
    pub gap_v: u8,
    /// Space between gates horizontally
    #[arg(long, default_value = "0")]
    pub gap_h: u8,
    /// Space around the edge of the baseplate
    #[arg(long, default_value = "0")]
    pub margin: u8,
    /// Space between the edge of the baseplate and the gates
    #[arg(long, default_value = "0")]
    pub padding: u8,
    /// Space between the left edge of the baseplate and the first gates in each row
    #[arg(long, default_value = "0")]
    pub indent: u8,
}

impl<'a> BuilderContext<'a> {
    fn new(map: &'a WireMap, options: BuilderOptions) -> Self {
        Self {
            wires: Vec::new(),
            bricks: Vec::new(),
            seen: HashSet::new(),
            wire_map: map,
            options,
        }
    }

    fn conn_seen(&self, conn: &compiler::WireConnection) -> bool {
        // if this is a reroute, check if any of the gates that link to it
        // have been seen
        if matches!(conn.gate.kind, GateKind::Reroute) {
            if let Some(srcs) = self.wire_map.get(&conn.gate.index) {
                return srcs.iter().any(|src| self.seen.contains(src));
            }
            false
        } else {
            // Otherwise, just check if the gate itself has been seen
            self.seen.contains(&conn.gate.index)
        }
    }

    fn build(&mut self, module: CompiledModule, mut pos: Position) -> Position {
        let top_left = pos;

        // When true, allocate some space for the rerouter inputs
        let input_rerouters = module.inputs.len() == module.num_inputs
            && module.inputs.iter().all(|(_, w)| {
                w.gate.meta.input_index.is_some() && matches!(w.gate.kind, GateKind::Reroute)
            });
        // When true, allocate some space for the rerouter outputs
        let output_rerouters = module.outputs.iter().all(|output| {
            matches!(output, CompiledOutput::Wire(wire) if wire.gate.meta.output_index.is_some() && matches!(wire.gate.kind, GateKind::Reroute))
        });

        // If there are inputs, allocate 2 units for their rerouters
        if input_rerouters {
            pos.y += 2; // TODO: rerouter height
        }

        pos.x += self.options.padding as i32 + self.options.indent as i32; // Add padding and indent to the left
        pos.y += self.options.padding as i32; // Add padding to the top

        let mut pending_output_rerouters = vec![];

        let mut max_x = 2;
        let mut next_row_y = pos.y;
        let mut first_in_row = true;
        let mut first_row = true;

        // Gates the have been output but their row is not complete
        let mut current_row = HashSet::new();
        let mut queue = VecDeque::new();

        let mut constants = HashMap::new();

        for (conn, lit) in module.gate_literals {
            let bytes = lit.as_bytes();
            let id = if let Some(id) = constants.get(&bytes) {
                *id
            } else {
                // Create a constant gate for the literal value
                let id = Gate::next_index();
                self.bricks.push(
                    Brick {
                        id: Some(id),
                        position: pos + Position::new(5, 5, 2),
                        asset: B_GATE_CONSTANT,
                        ..Default::default()
                    }
                    .with_component(LogicGateComponent::new(
                        LogicGate::Const,
                        [],
                        Some(Box::new(lit.variant()) as Box<dyn AsBrdbValue>),
                    )),
                );
                constants.insert(bytes, id);

                // Add spacing for this new gate
                if !first_in_row {
                    pos.x += self.options.gap_v as i32; // Add gap between gates
                }
                first_in_row = false;

                pos.x += 10; // gate width

                max_x = max_x.max(pos.x);
                next_row_y = next_row_y.max(pos.y + 10); // gate height

                id
            };

            // Add the wire connection for the constant value
            self.wires.push(WireConnection::new(
                WirePort {
                    brick_id: id,
                    component_type: LogicGate::Const.component_name(),
                    port_name: LogicGate::VALUE,
                },
                WirePort {
                    brick_id: conn.gate.index,
                    component_type: conn.gate.kind.component_name(),
                    port_name: conn.property,
                },
            ));
        }

        // Add all gates to the queue
        for gate in module.gates {
            if input_rerouters && let Some(i) = gate.meta.input_index {
                // Add the rerouter for the input
                self.bricks.push(
                    Brick {
                        id: Some(gate.index),
                        position: top_left + (i as i32 * 2, 0, 0).into() + Position::ONE, // Offset by 1 to center the rerouter
                        asset: gate.kind.brick(),
                        // TODO: support coloring from the module
                        ..Default::default()
                    }
                    .with_component_box(gate.kind.component()),
                );
                // Mark this gate as seen
                self.seen.insert(gate.index);
            } else if output_rerouters && let Some(i) = gate.meta.output_index {
                // Mark this gate as seen so maybe another gate can use it, which it probably won't
                self.seen.insert(gate.index);
                // If this is an output rerouter, add it to the pending outputs
                pending_output_rerouters.push((i, gate));
            } else {
                queue.push_back(QueueItem::Gate(gate));
            }
        }
        for module in module.sub_modules {
            queue.push_back(QueueItem::Module(module.1));
        }

        // When the next row is reached, add a new row to the queue
        queue.push_back(QueueItem::NextRow(0));

        while let Some(gate) = queue.pop_front() {
            match gate {
                QueueItem::Module(sub_module) => {
                    let all_inputs_seen = sub_module.inputs.iter().all(|(_, w)| self.conn_seen(w));
                    if !all_inputs_seen {
                        // If not all inputs are seen, requeue the submodule
                        queue.push_back(QueueItem::Module(sub_module));
                        continue;
                    }

                    let mut ctx = BuilderContext::new(self.wire_map, self.options);

                    if !first_in_row {
                        pos.x += self.options.gap_v as i32; // Add gap between gates
                    } else if !first_row {
                        pos.y += self.options.gap_h as i32; // Add gap between rows
                    }
                    first_in_row = false;

                    pos.x += self.options.margin as i32;
                    pos.y += self.options.margin as i32;

                    // Recurse into the submodule and build its bounds
                    let mut sub_pos = ctx.build(sub_module, pos + Position::UP * 2);
                    // Add the submodule's seen gates/wires to the current row
                    self.wires.extend(ctx.wires);
                    self.bricks.extend(ctx.bricks);
                    current_row.extend(ctx.seen);

                    let mut width = sub_pos.x - pos.x;
                    width += width % 2; // Ensure width is even
                    let mut height = sub_pos.y - pos.y;
                    height += height % 2; // Ensure height is even

                    // Create a baseplate brick for the submodule

                    self.bricks.push(Brick {
                        asset: brdb::BrickType::Procedural {
                            asset: brdb::assets::bricks::PB_DEFAULT_MICRO_BRICK,
                            size: (width as u16 / 2, height as u16 / 2, 1u16).into(),
                        },
                        position: pos + (width / 2, height / 2, 1).into(),
                        // Darken the color with depth
                        color: Color::monochrome(255u8.saturating_sub(pos.z as u8 * 20))
                            .to_linear(),
                        ..Default::default()
                    });

                    // TODO: theoretical gate stacking per row with a max_z

                    sub_pos.x += self.options.margin as i32;

                    max_x = max_x.max(sub_pos.x);
                    pos.x += sub_pos.x;
                    next_row_y = next_row_y.max(sub_pos.y);
                }
                QueueItem::Gate(gate) => {
                    let all_inputs_seen = self
                        .wire_map
                        .get(&gate.index) // Lookup sources connected to this gate as a destination
                        .map(|srcs| srcs.iter().all(|s| self.seen.contains(s))) // Ensure all sources have been seen
                        .unwrap_or(true);
                    if !all_inputs_seen {
                        // If not all inputs are seen, requeue the gate
                        queue.push_back(QueueItem::Gate(gate));
                        continue;
                    }

                    if !first_in_row {
                        pos.x += self.options.gap_v as i32; // Add gap between gates
                    } else if !first_row {
                        pos.y += self.options.gap_h as i32; // Add gap between rows
                    }
                    first_in_row = false;

                    self.bricks.push(
                        Brick {
                            id: Some(gate.index),
                            position: pos + Position::new(5, 5, 2), // Offset by half a 1x1F's size
                            asset: gate.kind.brick(),
                            ..Default::default()
                        }
                        .with_component_box(gate.kind.component()),
                    );

                    pos.x += 10; // gate width

                    max_x = max_x.max(pos.x);
                    next_row_y = next_row_y.max(pos.y + 10); // gate height

                    // Gates that enable cycles can be
                    if gate.kind.cyclic() {
                        self.seen.insert(gate.index);
                    } else {
                        current_row.insert(gate.index);
                    }
                }
                QueueItem::NextRow(n) => {
                    // Finalize current row gates
                    self.seen.extend(current_row.drain());
                    // Reset the position to the left side
                    pos.x = top_left.x + self.options.padding as i32 + self.options.indent as i32; // Add padding and indent to the left

                    // Move down for the next row
                    pos.y = next_row_y;
                    next_row_y = pos.y;
                    first_in_row = true;
                    first_row = false;

                    if n > 10000 {
                        panic!("Too many rows, something is wrong with the module or the code...");
                    }

                    // If there are no more gates to place, break out of the loop
                    if queue.is_empty() {
                        break;
                    }
                    // If there are still gates to place, add a new row
                    queue.push_back(QueueItem::NextRow(n + 1));
                }
            }
        }

        max_x += self.options.padding as i32; // Add padding to the max width
        pos.y += self.options.padding as i32; // Add padding to the top

        if output_rerouters {
            for (index, gate) in pending_output_rerouters {
                // Add the rerouter for the output
                self.bricks.push(
                    Brick {
                        id: Some(gate.index),
                        position: Position::new(top_left.x + index as i32 * 2, pos.y, top_left.z)
                            + Position::ONE, // Offset by 1 to center the rerouter
                        asset: gate.kind.brick(),
                        ..Default::default()
                    }
                    .with_component_box(gate.kind.component()),
                );
            }
            pos.y += 2; // rerouter height
        }

        for w in module.wires {
            let src = WirePort {
                brick_id: w.src.gate.index,
                component_type: w.src.gate.kind.component_name(),
                port_name: w.src.property.into(),
            };
            let dst = WirePort {
                brick_id: w.dst.gate.index,
                component_type: w.dst.gate.kind.component_name(),
                port_name: w.dst.property.into(),
            };
            self.wires.push(WireConnection::new(src, dst));
        }

        Position::new(max_x, pos.y, pos.z)
    }
}

/// Obtain a map of src gate index to dst gate index, ignoring buffers as destinations
fn build_dst_to_src(module: &CompiledModule) -> WireMap {
    let mut map = WireMap::new();
    for w in &module.wires {
        // Ignore buffers as destinations
        if matches!(w.dst.gate.kind, GateKind::Buffer) {
            continue;
        }

        map.entry(w.dst.gate.index)
            .or_default()
            .insert(w.src.gate.index);
    }

    for (_, sub_module) in &module.sub_modules {
        let sub_map = build_dst_to_src(sub_module);
        for (dst, srcs) in sub_map {
            map.entry(dst).or_default().extend(srcs);
        }
    }

    map
}

pub fn module_to_world(
    module: CompiledModule,
    options: BuilderOptions,
) -> Result<World, BuilderError> {
    let mut world = World::new();

    let map = build_dst_to_src(&module);
    let mut ctx = BuilderContext::new(&map, options);
    ctx.build(module, Position::ZERO);

    world.add_bricks(ctx.bricks);
    world.add_wires(ctx.wires);

    Ok(world)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::{
        bearilog::parse_and_compile,
        brdb::Brdb,
        builder::{BuilderOptions, module_to_world},
    };

    #[test]
    fn test() -> Result<(), Box<dyn Error>> {
        let source = "
        inline module add(a, b) -> c {
            c = a + b;
        }
        module foo(a, b, c, d) -> o {
            o = add(add(a, b), add(c, d)) + 2;
        }
        ";

        let options = BuilderOptions::default();
        let world = module_to_world(parse_and_compile(source, "foo", false)?, options)?;
        Brdb::new_memory()?.save("create", &world)?;

        Ok(())
    }
}
