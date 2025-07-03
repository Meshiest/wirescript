use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use crate::{helpers::WireMap, options::LayoutOptions};
use bearilog::compiler::{self, CompiledModule, CompiledOutput, Gate, GateKind};
use brdb::{
    self, Brick, BrickSize, Color, Position, WireConnection, WirePort,
    assets::{
        bricks::B_GATE_CONSTANT,
        components::{LogicGate, LogicGateComponent},
    },
    schema::as_brdb::AsBrdbValue,
};

pub struct LayoutBuilderContext<'a> {
    pub wires: Vec<WireConnection>,
    pub bricks: Vec<Brick>,
    /// A map of gate index to brick ID
    seen: HashSet<usize>,
    wire_map: &'a WireMap,
    options: LayoutOptions,
}

pub enum QueueItem {
    Module(CompiledModule),
    Gate(Arc<Gate>),
    NextRow(usize),
}

impl<'a> LayoutBuilderContext<'a> {
    pub fn new(map: &'a WireMap, options: LayoutOptions) -> Self {
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

    pub fn build(&mut self, module: CompiledModule, mut pos: Position) -> Position {
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
            pos.y += 2;
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

                    let mut ctx = LayoutBuilderContext::new(self.wire_map, self.options);

                    if !first_in_row {
                        pos.x += self.options.gap_v as i32; // Add gap between gates
                    } else if !first_row {
                        pos.y += self.options.gap_h as i32; // Add gap between rows
                    }
                    first_in_row = false;

                    pos.x += self.options.margin as i32;
                    pos.y += self.options.margin as i32;

                    // Recurse into the submodule and build its bounds
                    let mut sub_pos = ctx.build(
                        sub_module,
                        pos + Position::UP * 2 * i32::from(!self.options.flat), // When flat, don't increase the z position
                    );
                    // Add the submodule's seen gates/wires to the current row
                    self.wires.extend(ctx.wires);
                    self.bricks.extend(ctx.bricks);
                    current_row.extend(ctx.seen);

                    if !self.options.flat {
                        let mut width = sub_pos.x - pos.x;
                        width += width % 2; // Ensure width is even
                        let mut height = sub_pos.y - pos.y;
                        height += height % 2; // Ensure height is even

                        // Create a baseplate brick for the submodule
                        let color =
                            Color::monochrome(255u8.saturating_sub(pos.z as u8 * 20)).to_linear();
                        for (size, position) in
                            partition_brick((width as u16 / 2, height as u16 / 2, 1).into(), pos)
                        {
                            self.bricks.push(Brick {
                                asset: brdb::BrickType::Procedural {
                                    asset: brdb::assets::bricks::PB_DEFAULT_MICRO_BRICK,
                                    size,
                                },
                                position,
                                // Darken the color with depth
                                color,
                                ..Default::default()
                            });
                        }
                    }

                    sub_pos.x += self.options.margin as i32;

                    max_x = max_x.max(sub_pos.x);
                    // Overwrite the x position with the submodule's end position
                    pos.x = sub_pos.x;
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

// If a brick's size is larger than 100 in any axis, subtract 100 and repeat
// Recursively partition the brick until all dimensions are less than or equal to 100
// Returns vector of (brick size, brick center)
pub fn partition_brick(brick_size: BrickSize, top_left: Position) -> Vec<(BrickSize, Position)> {
    const SIZE_LIMIT: u16 = 128;
    if brick_size.x > SIZE_LIMIT {
        [
            partition_brick(
                BrickSize {
                    x: SIZE_LIMIT,
                    y: brick_size.y,
                    z: brick_size.z,
                },
                top_left,
            ),
            partition_brick(
                BrickSize {
                    x: brick_size.x - SIZE_LIMIT,
                    y: brick_size.y,
                    z: brick_size.z,
                },
                top_left + Position::new(SIZE_LIMIT as i32, 0, 0),
            ),
        ]
        .concat()
    } else if brick_size.y > SIZE_LIMIT {
        [
            partition_brick(
                BrickSize {
                    x: brick_size.x,
                    y: SIZE_LIMIT,
                    z: brick_size.z,
                },
                top_left,
            ),
            partition_brick(
                BrickSize {
                    x: brick_size.x,
                    y: brick_size.y - SIZE_LIMIT,
                    z: brick_size.z,
                },
                top_left + Position::new(0, SIZE_LIMIT as i32, 0),
            ),
        ]
        .concat()
    } else if brick_size.z > SIZE_LIMIT {
        [
            partition_brick(
                BrickSize {
                    x: brick_size.x,
                    y: brick_size.y,
                    z: SIZE_LIMIT,
                },
                top_left,
            ),
            partition_brick(
                BrickSize {
                    x: brick_size.x,
                    y: brick_size.y,
                    z: brick_size.z - SIZE_LIMIT,
                },
                top_left + Position::new(0, 0, SIZE_LIMIT as i32),
            ),
        ]
        .concat()
    } else {
        vec![(brick_size, top_left + Position::from(brick_size))]
    }
}
