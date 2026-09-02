//! The `@side` outer rerouters: geometry and emission.

use super::*;

/// Grid-unit offset from the chip brick's centre to a flush outer rerouter's
/// centre: chip half-extent (5) + rerouter half-extent (1).
const REROUTER_EDGE_OFFSET: i32 = 6;

/// Spacing between adjacent rerouter centres along a side.
const REROUTER_PITCH: i32 = 2;

/// Along-edge coordinate of the first pin: flush at the top/left corner of the
/// chip's 10-wide edge (chip half-extent 5 − rerouter half-extent 1). Pins run
/// inward from here (top→bottom, left→right) rather than centred on the edge.
const REROUTER_RUN_START: i32 = 4;

/// Rerouter centre sits 1 unit below the chip centre so both bases rest on
/// the same plane (chip half-height 2, rerouter half-height 1).
const REROUTER_Z_OFFSET: i32 = -1;

/// Rerouter name labels use the smaller tag size, not the full gate-label size.
const REROUTER_LABEL_LINE_HEIGHT: f32 = 1.2;

/// Yaw applied to an outer rerouter brick so its wire nub faces away from the
/// chip. Output pins face outward; input pins are flipped 180° so an input and
/// an output on the same side read opposite ways (as requested). The reroute
/// node's default yaw (`Deg0`) points +Y — "screen right" — so the sides step
/// around from there. The floating text label rides the brick's yaw (its own
/// TextDisplay rotation stays 0), giving the in/out 180° text flip for free.
///
/// World<->screen mapping, confirmed in-game: +X = up, +Y = right, and `Deg90`
/// yaws toward −X — so the per-side values below point each output pin outward.
fn rerouter_orientation(side: &str, is_input: bool) -> brdb::Rotation {
    use brdb::Rotation::*;
    // Outward-facing yaw per side (output pins).
    // Deg0=+Y, Deg90=−X, Deg180=−Y, Deg270=+X.
    let outward = match side {
        "right" => Deg0,   // faces +Y (screen right)
        "bottom" => Deg90, // faces −X (screen down)
        "left" => Deg180,  // faces −Y (screen left)
        _ => Deg270,       // top, faces +X (screen up)
    };
    if is_input {
        match outward {
            Deg0 => Deg180,
            Deg90 => Deg270,
            Deg180 => Deg0,
            Deg270 => Deg90,
        }
    } else {
        outward
    }
}

/// TextDisplay rotation (degrees) for a rerouter pin's name label. Calibrated
/// in-game: on each side the input and output labels read opposite ways.
/// left/top edges → input 0°, output 180°; right/bottom edges → input 180°,
/// output 0°.
fn rerouter_label_rotation(side: &str, is_input: bool) -> f32 {
    let left_or_top = matches!(side, "left" | "top");
    if left_or_top == is_input { 0.0 } else { 180.0 }
}

/// TextDisplay horizontal anchor for a rerouter pin's name label. Calibrated
/// in-game: right/bottom edges anchor at 0, left/top at 1, so the name hangs
/// off the pin's outer end on each edge.
fn rerouter_label_anchor_x(side: &str) -> f32 {
    if matches!(side, "right" | "bottom") {
        0.0
    } else {
        1.0
    }
}

/// Place one outer-grid rerouter per `@side`-annotated root port, flush
/// against the chip brick's edge, pre-wired to the port's inner
/// MicrochipInput/Output gate. The brdb writer serialises the cross-grid
/// wires as remote wire sources automatically.
pub(super) fn emit_port_rerouters(world: &mut World, ctx: &EmitContext, module: &Module, opts: &EmitOptions) {
    let side_sym = *sym::REROUTE_SIDE;

    // side → [(source offset, node id, is_input)], later sorted per side so
    // ins and outs interleave in declaration order (the spec's ordering rule).
    let mut by_side: HashMap<&'static str, Vec<(usize, NodeId, bool)>> = HashMap::default();
    for (ids, is_input) in [(&module.inputs, true), (&module.outputs, false)] {
        for id in ids.iter() {
            let Some(node) = module.nodes.get(id) else {
                continue;
            };
            let Some(Literal::String(side)) = node.properties.get(&side_sym) else {
                continue;
            };
            let side: &'static str = match side.as_str() {
                "left" => "left",
                "right" => "right",
                "top" => "top",
                _ => "bottom",
            };
            by_side
                .entry(side)
                .or_default()
                .push((node.source_range.start.offset, *id, is_input));
        }
    }

    // Fixed side order for deterministic brick/wire output.
    for side in ["left", "right", "top", "bottom"] {
        let Some(mut ports) = by_side.remove(side) else {
            continue;
        };
        ports.sort_by_key(|(off, id, _)| (*off, id.0));
        for (i, (_, node_id, is_input)) in ports.into_iter().enumerate() {
            let Some(&io_brick_id) = ctx.node_brick_ids.get(&node_id) else {
                continue;
            };
            let node = &module.nodes[&node_id];

            // Pins run from the top/left corner inward at REROUTER_PITCH spacing,
            // first-declared first: `run` steps from REROUTER_RUN_START toward
            // the far end.
            let run = REROUTER_RUN_START - REROUTER_PITCH * i as i32;
            // World<->screen pinned in-game: +X = up, +Y = right. Edges: left =
            // −Y, right = +Y, top = +X, bottom = −X. Left/right pins run down
            // the X axis from the top (+X); top/bottom pins run along Y from the
            // left (−Y), so their `run` is negated.
            let (dx, dy) = match side {
                "left" => (run, -REROUTER_EDGE_OFFSET),
                "right" => (run, REROUTER_EDGE_OFFSET),
                "top" => (REROUTER_EDGE_OFFSET, -run),
                _ => (-REROUTER_EDGE_OFFSET, -run), // bottom
            };
            let position = Position {
                x: opts.chip_pos.x + dx,
                y: opts.chip_pos.y + dy,
                z: opts.chip_pos.z + REROUTER_Z_OFFSET,
            };

            let (mut brick, rer_brick_id) = brdb::Brick {
                asset: BrickType::from("B_1x1_Reroute_Node"),
                position,
                color: io_node_color(node), // matches the inner chip-I/O gate colour
                rotation: rerouter_orientation(side, is_input),
                ..Default::default()
            }
            .with_id_split();
            // Saved worlds use the component's instance name; the rerouter's
            // data struct is empty.
            brick.add_component_box(Box::new(LiteralComponent::new(gc::REROUTER)));

            // `@invisible` ports: hide the rerouter, drop all collision so it
            // doesn't block players/weapons/tools, and skip its name label.
            let invisible = matches!(
                node.properties.get(&*sym::REROUTE_INVISIBLE),
                Some(Literal::Bool(true))
            );
            if invisible {
                brick.visible = false;
                brick.collision = Collision {
                    player: false,
                    player1: Some(false),
                    player2: Some(false),
                    player3: Some(false),
                    weapon: false,
                    interact: false,
                    tool: false,
                    physics: false,
                };
            }

            if !invisible {
                if let Some(name) = microchip_io_label(node) {
                    // Rotation + anchor are calibrated per side/direction (see
                    // rerouter_label_rotation / rerouter_label_anchor_x).
                    let label = text_label(
                        world,
                        &name,
                        rerouter_label_rotation(side, is_input),
                        0.5,
                        REROUTER_LABEL_LINE_HEIGHT,
                        rerouter_label_anchor_x(side),
                        0.5,
                    );
                    brick.add_component_box(Box::new(label));
                }
            }
            world.bricks.push(brick);

            // in:  rerouter.RER_Output → MicrochipInput.RER_Input
            // out: MicrochipOutput.RER_Output → rerouter.RER_Input
            // (mirrors the chip-port remap in build_port_index)
            let rer_port = |port: &'static str| BrdbWirePort {
                brick_id: rer_brick_id,
                component_type: BString::Static(gc::REROUTER),
                port_name: BString::Static(port),
            };
            let io_class = if is_input {
                gc::MICROCHIP_INPUT
            } else {
                gc::MICROCHIP_OUTPUT
            };
            let io_port = |port: &'static str| BrdbWirePort {
                brick_id: io_brick_id,
                component_type: BString::Static(io_class),
                port_name: BString::Static(port),
            };
            let conn = if is_input {
                WireConnection {
                    source: rer_port("RER_Output"),
                    target: io_port("RER_Input"),
                }
            } else {
                WireConnection {
                    source: io_port("RER_Output"),
                    target: rer_port("RER_Input"),
                }
            };
            world.add_wire(conn);
        }
    }
}
