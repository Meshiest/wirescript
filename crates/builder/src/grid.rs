use std::collections::{HashMap, VecDeque};

use crate::{layout::QueueItem, options::GridOptions};
use bearilog::{
    ast::Literal,
    compiler::{CompiledModule, CompiledOutput, Gate, GateKind, WireConnection},
};
use brdb::{
    Brick, Position, WirePort, World,
    assets::{
        bricks::B_GATE_CONSTANT,
        components::{LogicGate, LogicGateComponent},
    },
    schema::as_brdb::AsBrdbValue,
};

/// Adds a constant gate to the world if it doesn't already exist.
/// Then adds a wire connection from the constant gate to the specified connection.
fn get_or_add_constant(
    constants: &mut HashMap<Vec<u8>, usize>,
    world: &mut World,
    next_gate_pos: &mut dyn FnMut() -> Position,
    conn: WireConnection,
    lit: Literal,
) {
    let bytes = lit.as_bytes();
    let id = if let Some(id) = constants.get(&bytes) {
        *id
    } else {
        // Create a constant gate for the literal value
        let id = Gate::next_index();
        world.add_brick(
            Brick {
                id: Some(id),
                position: next_gate_pos(),
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
        id
    };

    // Add the wire connection for the constant value
    world.add_wire_connection(
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
    );
}

pub fn build_grid(module: CompiledModule, opts: GridOptions) -> World {
    let mut world = World::new();

    // When true, allocate some space for the rerouter inputs
    let input_rerouters = module.inputs.len() == module.num_inputs
        && module.inputs.iter().all(|(_, w)| {
            w.gate.meta.input_index.is_some() && matches!(w.gate.kind, GateKind::Reroute)
        });
    // When true, allocate some space for the rerouter outputs
    let output_rerouters = module.outputs.iter().all(|output| {
        matches!(output, CompiledOutput::Wire(wire) if wire.gate.meta.output_index.is_some() && matches!(wire.gate.kind, GateKind::Reroute))
    });

    let mut pending_output_rerouters = vec![];

    let mut num_gates = 0;

    // Gates the have been output but their row is not complete
    let mut constants = HashMap::new();

    let grid_width = opts.width.get() as usize;
    let grid_height = opts.height.get() as usize;
    let base_z = i32::from(opts.iobelow) * 2;

    let mut next_gate_pos = || {
        // In layers mode, place bricks on an XY grid, then stack them vertically
        // In stacks mode, stack bricks in XZ columns, then move to the next row
        let pos = if opts.layers {
            Position::new(
                // Each gate moves over 1 in the X direction, wrapping every W
                (num_gates % grid_width) as i32 * 10 + 5,
                // Each W gates moves down 1 in the Y direction, wrapping every H
                (num_gates / grid_width % grid_height) as i32 * 10 + 5,
                // Each WxH gates moves up 1 in the Z direction
                (num_gates / grid_width / grid_height) as i32 * 4 + 2 + base_z,
            )
        } else {
            Position::new(
                // Each H gates moves over 1 in the X direction, wrapping every W
                (num_gates / grid_height % grid_width) as i32 * 10 + 5,
                // Each WxH gates moves down 1 in the Y direction
                (num_gates / grid_height / grid_width) as i32 * 10 + 5,
                // Each gate moves up 1 in the Z direction, wrapping every H
                (num_gates % grid_height) as i32 * 4 + 2 + base_z,
            )
        };
        num_gates += 1;
        pos
    };

    for (conn, lit) in module.gate_literals {
        get_or_add_constant(&mut constants, &mut world, &mut next_gate_pos, conn, lit);
    }
    let mut queue = VecDeque::new();

    // Add all gates to the queue
    for gate in module.gates {
        if input_rerouters && let Some(i) = gate.meta.input_index {
            // Add the rerouter for the input
            world.add_brick(
                Brick {
                    id: Some(gate.index),
                    position: (i as i32 * 2 + 1, if opts.iobelow { 1 } else { -1 }, 1).into(), // Offset by 1 to center the rerouter
                    asset: gate.kind.brick(),
                    // TODO: support coloring from the module
                    ..Default::default()
                }
                .with_component_box(gate.kind.component()),
            );
        } else if output_rerouters && let Some(i) = gate.meta.output_index {
            // If this is an output rerouter, add it to the pending outputs
            pending_output_rerouters.push((i, gate));
        } else {
            queue.push_back(QueueItem::Gate(gate));
        }
    }

    for module in module.sub_modules {
        queue.push_back(QueueItem::Module(module.1));
    }

    while let Some(gate) = queue.pop_front() {
        match gate {
            QueueItem::Module(sub_module) => {
                // Add all gates to the queue
                for gate in sub_module.gates {
                    queue.push_back(QueueItem::Gate(gate));
                }
                // Add all submodules to the queue
                for sub in sub_module.sub_modules {
                    queue.push_back(QueueItem::Module(sub.1));
                }
                // Add all wires to the world
                for w in sub_module.wires {
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
                    world.add_wire_connection(src, dst);
                }
                // Add all constants
                for (conn, lit) in sub_module.gate_literals {
                    get_or_add_constant(&mut constants, &mut world, &mut next_gate_pos, conn, lit);
                }
            }
            QueueItem::Gate(gate) => {
                world.add_brick(
                    Brick {
                        id: Some(gate.index),
                        position: next_gate_pos(),
                        asset: gate.kind.brick(),
                        ..Default::default()
                    }
                    .with_component_box(gate.kind.component()),
                );
            }
            // No-op for this algorithm
            QueueItem::NextRow(_) => {}
        }
    }

    if output_rerouters {
        let y_offset = if opts.iobelow {
            8 // Place the rerouters under the gates
        } else {
            10 // Place the rerouters after the gates
        };
        // Determine the next Y position using num_gates
        let output_row = if opts.layers {
            // In layers mode, the output rerouters are placed at the bottom of the grid
            // But if there are not enough gates to fill the grid, they are placed after the last row
            Position::new(
                0,
                (num_gates / grid_width).min(grid_height - 1) as i32 * 10 + y_offset,
                0,
            )
        } else {
            let last_y = (num_gates.saturating_sub(1) / grid_height / grid_width) as i32 * 10;
            // In stacks mode, the output rerouters are placed on the beginning of the next row, which could
            // be any number of stacks/rows
            Position::new(0, last_y + y_offset, 0)
        };
        for (index, gate) in pending_output_rerouters {
            // Add the rerouter for the output
            world.add_brick(
                Brick {
                    id: Some(gate.index),
                    position: output_row + Position::new(index as i32 * 2 + 1, 1, 1), // Offset by 1 to center the rerouter
                    asset: gate.kind.brick(),
                    ..Default::default()
                }
                .with_component_box(gate.kind.component()),
            );
        }
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
        world.add_wire_connection(src, dst);
    }

    world
}
