//! Emit-side materialization of a `BusLayout`: rerouter bricks, the wires
//! that chain them, and the suppression of the module wires they replace.
//!
//! Most of the bus is built by hand here — no allocator, no geometry — so the
//! emit contract is pinned on its own. The last test is the other half: the
//! bus the allocator really builds, carried through emit into a world.

use std::sync::Arc;

use wirescript::ir::Module;
use wirescript::layout::LayoutResult;
use wirescript::template_cache::TemplateCache;

const FILE: &str = "code_layout_bus.ws";
/// Component instance name on a rerouter brick.
const REROUTER: &str = "Component_Internal_Rerouter";

/// resolve → typecheck → lower → layout, mirroring `examples/check_overlaps.rs`.
fn lowered_and_laid_out(src: &str) -> (Module, LayoutResult) {
    let resolved = wirescript::resolve::resolve(src, FILE, &wirescript::resolve::FsLoader);
    let tc = wirescript::typecheck::typecheck(&resolved.ast, FILE, &wirescript::typecheck::CeSlotMap::default());
    let lowered = wirescript::lower::lower(wirescript::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: FILE,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: wirescript::lower::FoldMode::Auto,
        ce_slots: &wirescript::typecheck::CeSlotMap::default(),
    });
    let layout = wirescript::layout::layout_with_opts(
        &lowered.module,
        &wirescript::layout_options_for(&resolved.ast, Some(resolved.source_map.clone())),
    );
    (lowered.module, layout)
}

fn build(module: &Module, layout: &LayoutResult) -> brdb::World {
    wirescript::build_world(
        module,
        layout,
        &wirescript::EmitOptions::default(),
        &Arc::new(TemplateCache::new()),
    )
    .expect("emit")
}

/// Every brick in the world — the main grid plus every chip grid.
fn brick_count_in(world: &brdb::World) -> usize {
    world.bricks.len() + world.grids.iter().map(|(_, b)| b.len()).sum::<usize>()
}

fn brick_count(module: &Module, layout: &LayoutResult) -> usize {
    brick_count_in(&build(module, layout))
}

// Hand-built bus: two rerouters chained, the second tapping a real port.
// Proves emit materializes nodes, chains them, honours suppression, and
// never creates fan-in — with no allocator and no geometry involved.
#[test]
fn emit_materializes_a_hand_built_bus() {
    // A program with one var read from one row.
    let src = "@layout(\"code\")\n\nvar a: int = 1\nin go: exec\non go {\n  PrintToConsole(\"${a}\")\n}\n";
    let (module, mut layout) = lowered_and_laid_out(src);
    // The emit contract is what this pins, so the layout's own lanes are
    // dropped and the bus below is entirely hand-built.
    layout.bus = wirescript::layout::BusLayout::default();

    // Find the var node and the wire out of it that we will replace.
    let var_id = module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("a var node")
        .id;
    let w = module
        .wires
        .iter()
        .find(|w| w.source.node_id == var_id)
        .expect("a wire out of the var")
        .clone();

    let before = brick_count(&module, &layout);

    layout.bus.nodes.push(wirescript::layout::BusNode {
        x: 0,
        y: -40,
        z: 2,
        rotation: wirescript::layout::NodeRotation::Deg90,
        role: wirescript::layout::BusRole::Gutter,
        color_of: Some(var_id),
    });
    layout.bus.nodes.push(wirescript::layout::BusNode {
        x: -20,
        y: -40,
        z: 2,
        rotation: wirescript::layout::NodeRotation::Deg90,
        role: wirescript::layout::BusRole::Gutter,
        color_of: Some(var_id),
    });
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Node(w.source),
        target: wirescript::layout::BusEnd::Bus(0),
    });
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Bus(0),
        target: wirescript::layout::BusEnd::Bus(1),
    });
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Bus(1),
        target: wirescript::layout::BusEnd::Node(w.target),
    });
    layout.bus.suppressed.insert((w.target.node_id, w.target.port));

    let world = build(&module, &layout);

    // Two new rerouter bricks.
    assert_eq!(brick_count_in(&world) - before, 2, "one brick per bus node");
    // No fan-in anywhere.
    let mut seen = std::collections::HashSet::new();
    for wire in &world.wires {
        let key = (
            wire.target.brick_id,
            wire.target.component_type.to_string(),
            wire.target.port_name.to_string(),
        );
        assert!(seen.insert(key), "fan-in on {:?}", wire.target.port_name);
    }
    // The suppressed original wire is gone; the bus path replaces it.
    // (Exactly one wire lands on the consumer's port — the bus tap.)
    let is_rerouter = |p: &brdb::WirePort| p.component_type.to_string() == REROUTER;
    let lane: Vec<_> = world
        .wires
        .iter()
        .filter(|wire| is_rerouter(&wire.source) || is_rerouter(&wire.target))
        .collect();
    assert_eq!(
        lane.len(),
        3,
        "source tap, lane hop, consumer tap — got {lane:#?}"
    );
    let taps: Vec<_> = lane
        .iter()
        .filter(|wire| is_rerouter(&wire.source) && !is_rerouter(&wire.target))
        .collect();
    assert_eq!(taps.len(), 1, "exactly one wire leaves the lane");
    assert_eq!(
        taps[0].target.port_name.to_string(),
        w.target.port.as_str(),
        "the lane drives the consumer's original port"
    );
}

/// A bus wire whose end will not resolve must FAIL the build, not warn.
///
/// Pass 3 logs and carries on when a module wire cannot be drawn, and that is
/// right: the wire was going to be drawn, so the loss shows up as a wire
/// missing from the world. A BUS wire is not symmetric. Its original was
/// suppressed and will never be redrawn, so dropping it leaves the consumer
/// reading nothing, forever, in a save that compiles, loads and pastes
/// cleanly. There is no later gate for it — so the build stops here.
#[test]
fn an_unresolvable_bus_wire_fails_the_build() {
    let src = "@layout(\"code\")\n\nvar a: int = 1\nin go: exec\non go {\n  PrintToConsole(\"${a}\")\n}\n";
    let (module, mut layout) = lowered_and_laid_out(src);
    layout.bus = wirescript::layout::BusLayout::default();

    let var_id = module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("a var node")
        .id;
    let w = module
        .wires
        .iter()
        .find(|w| w.source.node_id == var_id)
        .expect("a wire out of the var")
        .clone();

    layout.bus.nodes.push(wirescript::layout::BusNode {
        x: 0,
        y: -40,
        z: 2,
        rotation: wirescript::layout::NodeRotation::Deg90,
        role: wirescript::layout::BusRole::Gutter,
        color_of: Some(var_id),
    });
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Node(w.source),
        target: wirescript::layout::BusEnd::Bus(0),
    });
    // The tap that delivers the value — pointed at a node that does not exist,
    // so its end cannot be resolved to a brick port.
    let ghost = wirescript::ir::PortRef {
        node_id: wirescript::ir::NodeId::fresh(),
        port: w.target.port,
    };
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Bus(0),
        target: wirescript::layout::BusEnd::Node(ghost),
    });
    layout.bus.suppressed.insert((w.target.node_id, w.target.port));

    let built = wirescript::build_world(
        &module,
        &layout,
        &wirescript::EmitOptions::default(),
        &Arc::new(TemplateCache::new()),
    );
    let Err(err) = built else {
        panic!("an unresolvable bus wire must fail the build, not return Ok");
    };

    // The message has to name both ends and both ports, or a future failure is
    // a shrug rather than a lead.
    let text = err.to_string();
    for needle in [
        "bus node 0",
        "RER_Output",
        &ghost.node_id.to_string(),
        ghost.port.as_str(),
    ] {
        assert!(
            text.contains(needle),
            "the error must name {needle:?}; got {text:?}"
        );
    }
}

/// Suppressing a wire must not retroactively break a gate's variable tag.
///
/// A `Var_Get`-style gate names itself by tracing its `VarRef` wire back to
/// the var node through `EmitContext::wire_sources`, which emit builds from
/// `module.wires` before anything is drawn. Suppression filters what pass 3
/// draws and never mutates the IR, so the trace must still land on the var.
/// This pins that separation.
///
/// This does NOT exercise `resolve_var_label`'s rerouter hop: a `BusNode` has
/// no `NodeId`, so it can never appear as a step in that walk, and the lookup
/// here resolves on its first iteration. The hop is gated by the
/// `emit::tests::var_label_walk_follows_rerouter_hops` unit test.
#[test]
fn suppression_leaves_var_tag_lookup_intact() {
    let src = "@layout(\"code\")\n\nvar a: int = 1\nin go: exec\non go {\n  PrintToConsole(\"${a}\")\n}\n";
    let (module, mut layout) = lowered_and_laid_out(src);
    // Hand-built bus only: the layout's own lanes would drive the same port
    // this test taps.
    layout.bus = wirescript::layout::BusLayout::default();

    let var_id = module
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .expect("a var node")
        .id;
    let w = module
        .wires
        .iter()
        .find(|w| w.source.node_id == var_id)
        .expect("a wire out of the var")
        .clone();

    // A lane node carries no text of its own, so every `a` tag below comes
    // from a gate.
    layout.bus.nodes.push(wirescript::layout::BusNode {
        x: 0,
        y: -40,
        z: 2,
        rotation: wirescript::layout::NodeRotation::Deg0,
        role: wirescript::layout::BusRole::Gutter,
        color_of: Some(var_id),
    });
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Node(w.source),
        target: wirescript::layout::BusEnd::Bus(0),
    });
    layout.bus.wires.push(wirescript::layout::BusWire {
        source: wirescript::layout::BusEnd::Bus(0),
        target: wirescript::layout::BusEnd::Node(w.target),
    });
    layout.bus.suppressed.insert((w.target.node_id, w.target.port));

    let tags = label_texts(&build(&module, &layout));
    assert!(
        tags.contains(&("a".to_string(), 1.2)),
        "the gate whose `VarRef` wire was suppressed keeps its `a` tag; got {tags:?}"
    );
}

/// Every `Component_TextDisplay` text in the emitted world, paired with its
/// line height (2.4 = element name, 1.2 = variable tag). Read back through a
/// serialized `.brz` because component data is opaque in memory.
fn label_texts(world: &brdb::World) -> Vec<(String, f32)> {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    // One scratch file per CALL, not per process: the tests in this file run
    // on separate threads of one process, so a pid-keyed name has two of them
    // writing and reading the same bytes and whichever reads mid-write sees a
    // truncated archive.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "ws_code_layout_bus_tags_{}_{}.brz",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&path, world.to_brz_vec().expect("to brz")).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut out = Vec::new();
    for gid in 1..32 {
        let Ok(chunks) = reader.brick_chunk_index(gid) else {
            break;
        };
        for chunk in chunks {
            if chunk.num_components == 0 {
                continue;
            }
            let (_soa, comps) = reader
                .component_chunk_soa(gid, chunk.index)
                .expect("read components");
            for c in comps {
                if let (Some(BrdbValue::String(text)), Some(BrdbValue::F32(line_height))) =
                    (c.get("Text"), c.get("LineHeight"))
                {
                    out.push((text.clone(), *line_height));
                }
            }
        }
    }
    out
}

/// Strip the bus out of a layout, chips included, so the same program can be
/// emitted with and without one. Suppression goes with it, so the "bare"
/// world is the direct-wire world.
fn strip_bus(layout: &mut LayoutResult) {
    layout.bus = wirescript::layout::BusLayout::default();
    for child in layout.chip_layouts.values_mut() {
        strip_bus(child);
    }
}

fn bus_node_count(layout: &LayoutResult) -> usize {
    layout.bus.nodes.len()
        + layout
            .chip_layouts
            .values()
            .map(bus_node_count)
            .sum::<usize>()
}

fn suppressed_count(layout: &LayoutResult) -> usize {
    layout.bus.suppressed.len()
        + layout
            .chip_layouts
            .values()
            .map(suppressed_count)
            .sum::<usize>()
}

fn bus_wire_count(layout: &LayoutResult) -> usize {
    layout.bus.wires.len()
        + layout
            .chip_layouts
            .values()
            .map(bus_wire_count)
            .sum::<usize>()
}

/// Every wire in the world, main grid and chip grids alike.
fn wires_in(world: &brdb::World) -> Vec<&brdb::WireConnection> {
    world.wires.iter().collect()
}

/// The bus the ALLOCATOR built, carried end to end into a world.
///
/// The two tests above zero the layout's own lanes and hand-build a bus, so
/// they pin the emit contract in isolation and nothing else here reaches a
/// world through the real geometry. This does: it compiles an
/// `@layout("code")` program and compares the emitted world against the same
/// program built with the lanes stripped out.
///
/// The program carries a chip in both directions — a value delivered in and a
/// value read back out — because a DELIVERY tap is the endpoint the whole
/// pipeline is least sure of: the wire's own end lives in another module, so
/// it holds no placement here and every filter along the way has to be told to
/// keep it. A chip-free two-row body proves the wire accounting and nothing
/// about that.
#[test]
fn the_allocated_bus_reaches_the_emitted_world() {
    let src = "@layout(\"code\")

var a: int = 1
var log: string[]
in go: exec
chip Doubler(run: exec, amount: int) -> (twice: int) {
  var seen: int = 0
  var runs: int = 0
  on run {
    seen = amount + amount
    log.push(\"d${seen}\")
    runs = runs + 1
    log.push(\"e${runs}\")
    PrintToConsole(\"f${seen}${runs}\")
    log.push(\"g${seen}\")
  }
  out twice = seen
}
let doubled = Doubler(go, a)
on go {
  PrintToConsole(\"${a}\")
  PrintToConsole(\"x${a}\")
  PrintToConsole(\"${doubled.twice}\")
  PrintToConsole(\"y${doubled.twice}${a}\")
}
";
    let (module, layout) = lowered_and_laid_out(src);
    assert!(
        !layout.chip_layouts.is_empty(),
        "the fixture must build a chip interior of its own"
    );
    assert!(
        layout
            .chip_layouts
            .values()
            .any(|c| !c.bus.nodes.is_empty()),
        "the chip's own interior must earn a lane too"
    );
    let nodes = bus_node_count(&layout);
    assert!(nodes > 0, "the fixture must earn a lane");
    assert!(
        suppressed_count(&layout) > 0,
        "a lane must replace some wire"
    );

    let mut bare = layout.clone();
    strip_bus(&mut bare);
    let world = build(&module, &layout);
    let bare_world = build(&module, &bare);

    // (a) One rerouter brick per bus node, over and above the rerouters the
    // program already emitted for its own ports.
    fn rerouters(w: &brdb::World) -> usize {
        let one = |bs: &[brdb::Brick]| {
            bs.iter()
                .filter(|b| b.asset == brdb::BrickType::from("B_1x1_Reroute_Node"))
                .count()
        };
        one(&w.bricks) + w.grids.iter().map(|(_, b)| one(b)).sum::<usize>()
    }
    assert_eq!(
        brick_count_in(&world) - brick_count_in(&bare_world),
        nodes,
        "every bus node must become a brick"
    );
    assert_eq!(
        rerouters(&world) - rerouters(&bare_world),
        nodes,
        "and every one of them a rerouter"
    );

    // (b) No fan-in anywhere in the emitted world.
    let mut seen = std::collections::HashSet::new();
    for wire in wires_in(&world) {
        let key = (
            wire.target.brick_id,
            wire.target.component_type.to_string(),
            wire.target.port_name.to_string(),
        );
        assert!(seen.insert(key), "fan-in on {:?}", wire.target);
    }

    // (c) Every consumer port keeps exactly one inbound wire. Brick ids are
    // minted per build so they cannot be compared across the two worlds;
    // the port identity can, and with (b) proving each tuple unique, an
    // unchanged multiset of non-rerouter targets says every port that was
    // driven directly is now driven by the bus instead — no port lost its
    // wire, and none gained a second.
    let consumer_ports = |w: &brdb::World| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = wires_in(w)
            .iter()
            .map(|wire| {
                (
                    wire.target.component_type.to_string(),
                    wire.target.port_name.to_string(),
                )
            })
            .filter(|(ty, _)| ty != REROUTER)
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        consumer_ports(&world),
        consumer_ports(&bare_world),
        "the bus must land on the same consumer ports the direct wires did"
    );

    // ...and the wire count balances exactly: each suppressed wire dropped,
    // each bus wire drawn.
    assert_eq!(
        wires_in(&world).len(),
        wires_in(&bare_world).len() - suppressed_count(&layout) + bus_wire_count(&layout),
        "wire accounting: dropped {} suppressed, drew {} bus wires",
        suppressed_count(&layout),
        bus_wire_count(&layout)
    );

    // (d) The bussed variable's gates still name themselves. A `Var_Get`
    // reads its name by tracing its `VarRef` wire back to the var node, and
    // here that wire is one the allocator suppressed. The walk runs over
    // `wire_sources`, which emit builds from `module.wires` before anything
    // is drawn, so it must be untouched by the suppression — this pins that.
    // It does NOT reach `resolve_var_label`'s rerouter hop: a `BusNode` has
    // no `NodeId`, so it can never be a step in that walk whatever the
    // allocator does. The hop's gate is `emit::tests::
    // var_label_walk_follows_rerouter_hops`.
    let tags = label_texts(&world);
    assert!(
        tags.contains(&("a".to_string(), 1.2)),
        "a bussed variable's gates keep their `a` tag; got {tags:?}"
    );
}

/// Dag mode builds no lanes, so it is the layout that still carries an empty
/// bus through emit.
#[test]
fn an_empty_bus_changes_nothing() {
    let src = "var a: int = 1\nin go: exec\non go {\n  PrintToConsole(\"${a}\")\n}\n";
    let (module, layout) = lowered_and_laid_out(src);
    assert!(layout.bus.is_empty(), "dag mode builds no bus");
    let world = build(&module, &layout);
    assert!(!world.bricks.is_empty() || !world.grids.is_empty());
}
