//! `fn`: one definition plane per function, a Function Call per call site,
//! and outer storage passed in as hidden parameters, with no wire across the
//! plane's wall.

use brdb::schema::BrdbValue;
use wirescript::compile::{FoldMode, compile, compile_to_world};
use wirescript::ir::gate_class as gc;
use wirescript::ir::{Module, Node};

use crate::common;

fn lowered(src: &str) -> Module {
    let (_, tc, lowered) = common::run_stages(src, "fn.ws", FoldMode::Auto);
    assert!(tc.diagnostics.is_empty(), "{:?}", tc.diagnostics);
    let errors: Vec<_> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.severity == wirescript::Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    lowered.module
}

fn all_nodes(m: &Module) -> Vec<&Node> {
    let mut out: Vec<&Node> = Vec::new();
    m.walk(&mut |_, sub| out.extend(sub.nodes.values()));
    out
}

fn count(m: &Module, class: &str) -> usize {
    all_nodes(m)
        .iter()
        .filter(|n| n.gate_class == class)
        .count()
}

fn brz(src: &str) -> Vec<u8> {
    compile(common::input(src, "fn.ws"))
        .unwrap_or_else(|e| panic!("compile: {e:?}"))
        .brz
}

const FIVE_CALLS: &str = "
fn twice(x: int) -> int { return x * 3 + 1 }
in go: exec
var res: int = 0
on go {
  res = twice(1) + twice(2) + twice(3) + twice(4) + twice(5)
}
";

/// A `fn` called five times is one plane and five calls, and its body is
/// emitted once.
#[test]
fn one_plane_and_a_call_per_site() {
    let m = lowered(FIVE_CALLS);
    assert_eq!(count(&m, gc::MICROCHIP_FUNCTION_DEFINITION), 1);
    assert_eq!(count(&m, gc::FUNCTION_CALL), 5);
    assert_eq!(count(&m, gc::CALLABLE_ENTRY_FUNCTION), 1);
    assert_eq!(count(&m, gc::FUNCTION_INPUT), 1);
    assert_eq!(count(&m, gc::MATH_MULTIPLY), 1, "the body is emitted once");
    // The plane is its own module, holding the body.
    let shell = m
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION)
        .unwrap();
    let plane = &m.chips[&shell.id];
    assert!(
        plane
            .nodes
            .values()
            .any(|n| n.gate_class == gc::MATH_MULTIPLY)
    );
    // Every call binds the plane by a wire from its shell.
    let bound = m
        .wires
        .iter()
        .filter(|w| w.source.node_id == shell.id && w.source.port.as_str() == "Definition")
        .count();
    assert_eq!(bound, 5);
}

/// The save carries each input's `PortOrder` and default, each result's
/// `PortOrder`, and per call which inputs it supplies.
#[test]
fn the_save_carries_slots_defaults_and_input_sources() {
    let src = "
fn add(x: int, y: int = 7) -> int { return x + y }
in go: exec
var res: int = 0
on go {
  res = add(1)
  res = add(2, 3)
}
";
    let comps = common::world_components(&brz(src));
    let text = |name: &str| -> Vec<String> {
        comps
            .iter()
            .filter(|c| c.get_name() == name)
            .map(|c| format!("{c}"))
            .collect()
    };
    let inputs = text("BrickComponentData_FunctionInput");
    assert!(
        inputs
            .iter()
            .any(|t| t.contains("PortLabel: \"x\"") && t.contains("PortOrder: 0i32")),
        "{inputs:?}"
    );
    assert!(
        inputs.iter().any(|t| t.contains("PortLabel: \"y\"")
            && t.contains("PortOrder: 1i32")
            && t.contains("DefaultValue: 7i64")),
        "{inputs:?}"
    );
    let outputs = text("BrickComponentData_FunctionOutput");
    assert!(
        outputs.iter().any(|t| t.contains("PortOrder: 0i32")),
        "{outputs:?}"
    );
    let mut sources: Vec<Vec<u64>> = comps
        .iter()
        .filter(|c| c.get_name() == "BCSComponentVertex_FunctionCall")
        .map(|c| match c.get("InputSources") {
            Some(BrdbValue::Array(a)) => a
                .iter()
                .map(|e| match e {
                    BrdbValue::Enum(e) => e.value,
                    _ => u64::MAX,
                })
                .collect(),
            other => panic!("no InputSources: {other:?}"),
        })
        .collect();
    sources.sort();
    assert_eq!(
        sources,
        vec![vec![1, 0, 0, 0, 0, 0, 0, 0], vec![1, 1, 0, 0, 0, 0, 0, 0]]
    );
}

const CAPTURES: &str = "
var total: int = 0
var seen: int[]
fn add(x: int) -> int {
  total += x
  seen.push(x)
  return total
}
in go: exec
var res: int = 0
on go { res = add(2) }
";

fn all_wires(m: &Module) -> Vec<wirescript::ir::Wire> {
    let mut out = Vec::new();
    m.walk(&mut |_, sub| out.extend(sub.wires.iter().copied()));
    out
}

/// Inside the plane, the body's write and push take their refs from hidden
/// inputs after the declared one, and the call feeds each from the outer
/// storage.
#[test]
fn reads_and_writes_go_through_hidden_inputs() {
    let m = lowered(CAPTURES);
    let shell = m
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION)
        .unwrap();
    let plane = &m.chips[&shell.id];
    let handles: Vec<&Node> = plane
        .nodes
        .values()
        .filter(|n| n.note == Some("fn capture"))
        .collect();
    assert_eq!(handles.len(), 2, "one hidden input per outer storage");
    assert!(
        all_nodes(&m).iter().all(|n| n.note != Some("fn handle")),
        "no handle var"
    );
    let mut orders: Vec<i64> = handles
        .iter()
        .map(|h| match h.properties.get(&wirescript::intern::intern("PortOrder")) {
            Some(wirescript::ir::Literal::Int(k)) => *k,
            other => panic!("{other:?}"),
        })
        .collect();
    orders.sort();
    assert_eq!(orders, vec![1, 2], "after the declared input");
    let call = node(&m, gc::FUNCTION_CALL);
    let wires = all_wires(&m);
    for (name, port) in [("total", "VarRef"), ("seen", "ArrayVarRef")] {
        let storage = all_nodes(&m)
            .into_iter()
            .find(|n| {
                n.properties.get(&wirescript::intern::intern("_label"))
                    == Some(&wirescript::ir::Literal::String(name.into()))
                    && matches!(n.gate_class, gc::VAR | gc::ARRAY_VAR)
            })
            .unwrap_or_else(|| panic!("no storage {name}"));
        assert!(
            wires.iter().any(|w| w.source.node_id == storage.id
                && w.source.port.as_str() == port
                && w.target.node_id == call.id
                && w.target.port.as_str().starts_with("Input")),
            "the call is fed {name}'s {port}"
        );
    }
    assert_eq!(
        call.properties.get(&wirescript::intern::intern("InputSources")),
        Some(&wirescript::ir::Literal::Array(
            [1, 1, 1, 0, 0, 0, 0, 0]
                .into_iter()
                .map(wirescript::ir::Literal::Int)
                .collect()
        ))
    );
    for gate in plane
        .nodes
        .values()
        .filter(|n| matches!(n.gate_class, gc::VAR_INCREMENT | gc::ARRAY_PUSH))
    {
        let src = plane
            .wires
            .iter()
            .find(|w| {
                w.target.node_id == gate.id
                    && matches!(w.target.port.as_str(), "VarRef" | "ArrayVarRef")
            })
            .unwrap_or_else(|| panic!("{} has no ref wired in the plane", gate.gate_class));
        assert!(
            handles.iter().any(|h| h.id == src.source.node_id),
            "{} reads a hidden input",
            gate.gate_class
        );
    }
}

/// Where a labelled brick is in a written save: (grid, chunk, index) plus the
/// type index of each component on it.
fn labelled(
    brz: &[u8],
) -> Vec<(
    String,
    (u32, brdb::ChunkIndex, u32),
    Vec<(u16, String)>,
    Vec<BrdbValue>,
)> {
    let save = wirescript_ir::readback::Save::read(brz);
    let names = save.component_type_names();
    let mut out = Vec::new();
    for (gid, chunk, soa, comps) in save.component_chunks() {
        let mut types: Vec<u32> = Vec::new();
        for c in &soa.component_type_counters {
            types.extend(std::iter::repeat_n(c.type_index, c.num_instances as usize));
        }
        let mut per: std::collections::BTreeMap<
            u32,
            (Option<String>, Vec<(u16, String)>, Vec<BrdbValue>),
        > = std::collections::BTreeMap::default();
        for (i, c) in comps.iter().enumerate() {
            let e = per.entry(soa.component_brick_indices[i]).or_default();
            e.1.push((types[i] as u16, names[types[i] as usize].clone()));
            if let Some(BrdbValue::String(s)) = c.get("Text") {
                e.0 = Some(s.clone());
            }
            if let Some(v) = c.get("Value") {
                e.2.push(v.clone());
            }
        }
        for (b, (label, tys, values)) in per {
            if let Some(l) = label {
                out.push((l, (gid as u32, chunk, b), tys, values));
            }
        }
    }
    out
}

/// A `fn` that calls another `fn` reaching outer storage takes that storage
/// as a hidden input of its own and passes it on, so no wire crosses either
/// wall.
#[test]
fn a_capture_passes_through_a_calling_fn() {
    let src = "var total: int = 0\nfn inner() { total += 1 }\nfn outer() { inner() }\nin go: exec\non go { outer() }\n";
    let m = lowered(src);
    let captures: Vec<&Node> = all_nodes(&m)
        .into_iter()
        .filter(|n| n.note == Some("fn capture"))
        .collect();
    assert_eq!(captures.len(), 2, "one hidden input in each plane");
    let wires = all_wires(&m);
    let calls: Vec<&Node> = all_nodes(&m)
        .into_iter()
        .filter(|n| n.gate_class == gc::FUNCTION_CALL)
        .collect();
    assert_eq!(calls.len(), 2);
    for call in calls {
        assert!(
            wires
                .iter()
                .any(|w| w.target.node_id == call.id && w.target.port.as_str() == "Input0"),
            "each call is fed its hidden input"
        );
    }
    assert!(
        wires.iter().any(|w| captures.iter().any(|c| c.id == w.source.node_id)
            && calls_target(&m, w)),
        "the inner call is fed from the outer plane's hidden input"
    );
}

fn calls_target(m: &Module, w: &wirescript::ir::Wire) -> bool {
    all_nodes(m)
        .iter()
        .any(|n| n.id == w.target.node_id && n.gate_class == gc::FUNCTION_CALL)
}

/// Outer storage takes an input slot, so a `fn` with all eight declared has
/// none for it.
#[test]
fn a_capture_past_the_last_slot_is_reported() {
    let src = "var total: int = 0\nfn f(a: int, b: int, c: int, d: int, e: int, g: int, h: int, i: int) { total += a }\nin go: exec\non go { f(1, 2, 3, 4, 5, 6, 7, 8) }\n";
    assert!(codes(src).iter().any(|c| c == "WS115"), "{:?}", codes(src));
}

/// Every fixture with a `fn`: no wire in the emitted world joins a brick in a
/// function plane's grid to a brick outside it, but a call's definition wire
/// from its shell.
#[test]
fn no_wire_crosses_a_function_plane() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fn");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("ws") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let file = path.to_string_lossy().to_string();
        let world = compile_to_world(
            common::input(&src, &file),
            wirescript::emit::EmitOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{file}: {e:?}"))
        .world;
        // brick id -> grid (0 = main), and which grids are function planes.
        let mut grid_of = std::collections::HashMap::new();
        for b in &world.bricks {
            grid_of.insert(b.id.unwrap(), 0usize);
        }
        let mut entity_grid = std::collections::HashMap::new();
        for (i, (entity, bricks)) in world.grids.iter().enumerate() {
            for b in bricks {
                if let Some(id) = b.id {
                    grid_of.insert(id, i + 1);
                }
            }
            if let Some(id) = entity.id {
                entity_grid.insert(id, i + 1);
            }
        }
        let shells: std::collections::HashSet<usize> = world
            .bricks
            .iter()
            .chain(world.grids.iter().flat_map(|(_, b)| b.iter()))
            .filter(|b| {
                b.components
                    .iter()
                    .any(|c| c.component_type_ref() == Some(gc::MICROCHIP_FUNCTION_DEFINITION))
            })
            .filter_map(|b| b.id)
            .collect();
        let planes: std::collections::HashSet<usize> = world
            .microchip_links
            .iter()
            .filter(|(brick, _)| shells.contains(brick))
            .filter_map(|(_, entity)| entity_grid.get(entity).copied())
            .collect();
        assert!(!planes.is_empty(), "{file}: no function plane");
        // Each grid's parent: a loop nested in a function is inside its plane.
        let parent: std::collections::HashMap<usize, usize> = world
            .microchip_links
            .iter()
            .filter_map(|(brick, entity)| Some((*entity_grid.get(entity)?, *grid_of.get(brick)?)))
            .collect();
        let plane_of = |mut g: usize| loop {
            if planes.contains(&g) {
                break Some(g);
            }
            match parent.get(&g) {
                Some(p) => g = *p,
                None => break None,
            }
        };
        for w in &world.wires {
            let (s, t) = (grid_of[&w.source.brick_id], grid_of[&w.target.brick_id]);
            // A call's definition wire runs from the shell, the one exception.
            let definition =
                shells.contains(&w.source.brick_id) && w.source.port_name.as_ref() == "Definition";
            assert!(
                definition || plane_of(s) == plane_of(t),
                "{file}: a wire crosses a function plane's wall: {w}"
            );
        }
        checked += 1;
    }
    assert!(checked > 0, "no fixtures in {}", dir.display());
}

/// Every diagnostic code, typecheck's and lowering's.
fn codes(src: &str) -> Vec<String> {
    let (_, tc, lowered) = common::run_stages(src, "fn.ws", FoldMode::Auto);
    tc.diagnostics
        .iter()
        .chain(&lowered.diagnostics)
        .map(|d| d.code.clone())
        .collect()
}

/// A named string constant is one carrier gate; a plane that reads it gets its own copy
/// instead of a wire across its wall, and a copy nothing outside reads is dropped.
#[test]
fn a_named_string_constant_is_copied_into_the_plane() {
    // A runtime value beside the constant keeps it a shared carrier instead of folding.
    let both = "let HEX = \"fbfbfb\"\nfn f(c: controller, s: string) {\n  c.ShowChatMessage('<color=\"${HEX}\">${s}</>')\n  c.ShowChatMessage('<color=\"${HEX}\">${s}!</>')\n}\non ChatCommand(\"t\", \"t\") -> (controller, arguments) {\n  controller.ShowChatMessage('<color=\"${HEX}\">${arguments}</>')\n  f(controller, arguments)\n}\n";
    assert_eq!(codes(both), Vec::<String>::new());
    let carriers = |m: &Module| {
        all_nodes(m)
            .iter()
            .filter(|n| n.gate_class == gc::STRING_CONCATENATE)
            .count()
    };
    assert_eq!(carriers(&lowered(both)), 2, "one in the plane, one outside");
    let inside_only = "let HEX = \"fbfbfb\"\nfn f(c: controller, s: string) {\n  c.ShowChatMessage('<color=\"${HEX}\">${s}</>')\n  c.ShowChatMessage('<color=\"${HEX}\">${s}!</>')\n}\non ChatCommand(\"t\", \"t\") -> (controller, arguments) { f(controller, arguments) }\n";
    assert_eq!(
        carriers(&lowered(inside_only)),
        1,
        "the outer carrier is dead"
    );
    assert!(!brz(inside_only).is_empty());
}

#[test]
fn fn_mistakes_are_reported() {
    // A computed outer value has no way into the plane.
    assert_eq!(
        codes(
            "in k: int\nfn f() -> int { return k + 1 }\nin go: exec\non go { BroadcastChatMessage(\"${f()}\") }\n"
        ),
        vec!["WS113"]
    );
    // The game does not run a recursive call, direct or through another fn.
    let fact = "fn fact(n: int) -> int {\n  if n <= 1 { return 1 }\n  return n * fact(n - 1)\n}\nin go: exec\nvar res: int = 0\non go { res = fact(4) }\n";
    assert_eq!(codes(fact), vec!["WS020"]);
    // A call needs a chain to fire on.
    assert_eq!(
        codes("fn f() -> int { return 1 }\nout o = f()\n"),
        vec!["WS111"]
    );
    // What one plane cannot be.
    assert_eq!(codes("fn g<T>(x: T) -> T { return x }\n"), vec!["WS114"]);
    assert_eq!(
        codes("fn r() -> { x: int } { return { x: 1 } }\n"),
        vec!["WS114"]
    );
    assert_eq!(codes("fn c(n: const int) {}\n"), vec!["WS114"]);
    assert_eq!(
        codes("fn m(a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int, i: int) {}\n"),
        vec!["WS115"]
    );
    // Defaults: trailing, constant, and only on a mod or fn.
    assert_eq!(
        codes("fn h(a: int = 1, b: int) -> int { return a + b }\n"),
        vec!["WS114"]
    );
    assert_eq!(
        codes("var t: int = 0\nmod d(x: int = t) -> int { return x }\n"),
        vec!["WS046"]
    );
    assert_eq!(codes("chip C(a: int = 3) {}\n"), vec!["WS114"]);
}

/// A `mod` takes a default value too: an omitted argument is the default.
#[test]
fn a_mod_default_fills_an_omitted_argument() {
    let m = lowered(
        "mod scale(v: int, by: int = 3) -> int { return v * by }\nin go: exec\nin x: int\nvar res: int = 0\non go { res = scale(x) }\n",
    );
    let mul = all_nodes(&m)
        .into_iter()
        .find(|n| n.gate_class == gc::MATH_MULTIPLY)
        .expect("the multiply");
    let by = mul.properties.get(&wirescript::intern::intern("InputB"));
    assert_eq!(
        by,
        Some(&wirescript::ir::Literal::Int(3)),
        "{:?}",
        mul.properties
    );
}

/// A Function Call maps its slots onto the plane's input and result bricks
/// top to bottom (highest local X first), so slot k's brick is k-th from the
/// top whatever order layout placed them in.
#[test]
fn ports_are_placed_top_to_bottom_in_slot_order() {
    let src = "fn swap(a: int, b: int, c: int) -> (x: int, y: int) { return (c + b, a) }\nin go: exec\nvar res: int = 0\non go { let r = swap(1, 2, 3)\n res = r.x + r.y }\n";
    let world = compile_to_world(
        common::input(src, "fn.ws"),
        wirescript::emit::EmitOptions::default(),
    )
    .unwrap()
    .world;
    let order = |class: &str| -> Vec<i32> {
        let mut found: Vec<(i32, i32)> = Vec::new();
        for b in world.grids.iter().flat_map(|(_, b)| b.iter()) {
            let Some(c) = b
                .components
                .iter()
                .find(|c| c.component_type_ref() == Some(class))
            else {
                continue;
            };
            let schema = brdb::schemas::bricks_components_schema_max();
            let id = schema.intern.get("PortOrder").unwrap();
            let k = c
                .as_brdb_struct_prop_value(schema, id, id)
                .ok()
                .and_then(|v| v.as_brdb_i32().ok());
            found.push((b.position.x, k.unwrap_or(-1)));
        }
        found.sort_by_key(|(x, _)| std::cmp::Reverse(*x));
        found.into_iter().map(|(_, k)| k).collect()
    };
    assert_eq!(order(gc::FUNCTION_INPUT), vec![0, 1, 2]);
    assert_eq!(order(gc::FUNCTION_OUTPUT), vec![0, 1]);
}

/// Each Function Call carries a label naming the `fn` it runs.
#[test]
fn a_call_is_labelled_with_its_function() {
    let bricks = labelled(&brz(FIVE_CALLS));
    let calls = bricks
        .iter()
        .filter(|(_, _, tys, _)| tys.iter().any(|(_, n)| n == gc::FUNCTION_CALL))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 5);
    assert!(calls.iter().all(|(l, ..)| l == "twice"), "{calls:?}");
}

/// A call inside a `fn` to another `fn` binds it by a wire straight from that
/// shell, the one wire allowed across the wall (the game refuses a definition
/// held in a var).
#[test]
fn a_call_inside_a_plane_binds_by_a_wire_from_the_shell() {
    let m = lowered(
        "fn inner(n: int) -> int { return n + 1 }\nfn outer(n: int) -> int { return inner(n) * 2 }\nin go: exec\nvar res: int = 0\non go { res = outer(4) }\n",
    );
    let shells: Vec<&Node> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION)
        .collect();
    let inner = shells
        .iter()
        .find(|n| {
            n.properties.get(&wirescript::intern::intern("_label"))
                == Some(&wirescript::ir::Literal::String("inner".into()))
        })
        .unwrap();
    let outer = shells.iter().find(|n| n.id != inner.id).unwrap();
    let call = m.chips[&outer.id]
        .nodes
        .values()
        .find(|n| n.gate_class == gc::FUNCTION_CALL)
        .expect("the inner call");
    let def = m
        .wires
        .iter()
        .find(|w| w.target.node_id == call.id && w.target.port.as_str() == "Definition")
        .expect("its definition is wired from outside the plane");
    assert_eq!(def.source.node_id, inner.id);
    assert!(
        all_nodes(&m).iter().all(|n| n.note != Some("fn handle")),
        "no handle for a definition"
    );
}

/// `GetMicrochipBrick()` is a var holding a brick reference to the program's own
/// microchip brick, the one brick on the main grid, from a `fn` plane too.
#[test]
fn get_microchip_brick_names_the_programs_own_brick() {
    let src = "in go: exec\nfn paint(c: color) { GetMicrochipBrick().SetBrickProperty(Color, c) }\non go {\n  GetMicrochipBrick().SetBrickProperty(Color, Color(1.0, 0.0, 0.0))\n  paint(Color(0.0, 1.0, 0.0))\n}\n";
    let comps = common::world_components(&brz(src));
    let refs: Vec<String> = comps
        .iter()
        .filter(|c| c.get_name() == "BrickComponentData_Var")
        .map(|c| {
            format!("{c}")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(refs.len(), 2, "{refs:?}");
    for r in refs {
        assert!(
            r.contains("BRSavedBrickRef { BrickIndexInChunk: 0u32, ChunkIndex: BRSavedChunk3DIndex { X: 0i16, Y: 0i16, Z: 0i16, }, GridPersistentIndex: 1u32"),
            "{r}"
        );
    }
}

fn wired(m: &Module, from: (&Node, &str), to: (&Node, &str)) -> bool {
    m.wires.iter().any(|w| {
        w.source.node_id == from.0.id
            && w.source.port.as_str() == from.1
            && w.target.node_id == to.0.id
            && w.target.port.as_str() == to.1
    })
}

fn node<'a>(m: &'a Module, class: &str) -> &'a Node {
    let found: Vec<&Node> = m.nodes.values().filter(|n| n.gate_class == class).collect();
    assert_eq!(found.len(), 1, "one {class}");
    found[0]
}

/// `exec = trigger` runs a call from pure context: the trigger fires the
/// call, and an `on` over the call fires its body on `Completed`, with the
/// results readable there.
#[test]
fn an_exec_argument_triggers_the_call_and_on_fires_on_completed() {
    let src = "
in go: exec
var last: int = 0
var hits: int = 0
fn add(x: int, y: int = 1) -> int { return x + y }
on add(2, exec = go) -> (v) {
  hits += 1
  last = v
}
";
    let m = lowered(src);
    let go = node(&m, gc::MICROCHIP_INPUT);
    let call = node(&m, gc::FUNCTION_CALL);
    let inc = node(&m, gc::VAR_INCREMENT);
    let set = node(&m, gc::VAR_SET);
    assert!(wired(&m, (go, "RER_Output"), (call, "Exec")));
    assert!(wired(&m, (call, "Completed"), (inc, "Exec")));
    assert!(wired(&m, (call, "Output0"), (set, "Value")));

    // A `fn` with no results still exposes its completion.
    let src = "
in go: exec
var hits: int = 0
var total: int = 0
fn bump(x: int) { total += x }
on bump(2, exec = go) { hits += 1 }
let r = bump(3, exec = go)
on r.exec { hits += 2 }
";
    let m = lowered(src);
    let calls: Vec<&Node> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::FUNCTION_CALL)
        .collect();
    assert_eq!(calls.len(), 2);
    let incs: Vec<&Node> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_INCREMENT)
        .collect();
    for call in calls {
        assert!(
            incs.iter()
                .any(|i| wired(&m, (call, "Completed"), (i, "Exec"))),
            "each call fires its own handler"
        );
    }
}

/// `f(...).exec` names the call's `Completed`. On a `mod` that runs no exec
/// step there is nothing to name, which is an error rather than a gate that
/// computes nothing.
#[test]
fn dot_exec_names_completed_and_is_an_error_without_one() {
    let src = "
in go: exec
var hits: int = 0
fn add(x: int) -> int { return x + 1 }
let e = add(1, exec = go).exec
on e { hits += 1 }
";
    let m = lowered(src);
    let call = node(&m, gc::FUNCTION_CALL);
    let inc = node(&m, gc::VAR_INCREMENT);
    assert!(wired(&m, (call, "Completed"), (inc, "Exec")));

    let mod_src = "in go: exec\nvar hits: int = 0\nmod inc(x: int) -> int { return x + 1 }\non go {\n  let e = inc(1).exec\n  on e { hits += 1 }\n}\n";
    let (_, _, lowered) = common::run_stages(mod_src, "fn.ws", FoldMode::Auto);
    let errors: Vec<_> = lowered
        .diagnostics
        .iter()
        .filter(|d| d.severity == wirescript::Severity::Error)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(errors, vec!["WS043"], "{:?}", lowered.diagnostics);
    // Beside another error it stays an error.
    let both = format!("{mod_src}fn f() -> int {{ return f() }}\non go {{ hits = f() }}\n");
    let (_, _, lowered) = common::run_stages(&both, "fn.ws", FoldMode::Auto);
    assert!(
        lowered
            .diagnostics
            .iter()
            .any(|d| d.code == "WS043" && d.severity == wirescript::Severity::Error)
    );
}

/// An argument given by its parameter's name reaches that parameter's slot.
#[test]
fn a_named_argument_fills_its_slot() {
    let src = "
in go: exec
in k: int
var r: int = 0
fn add(x: int, y: int = 1) -> int { return x + y }
on go { r = add(k, y = k + 2) }
";
    let m = lowered(src);
    let call = node(&m, gc::FUNCTION_CALL);
    let plus = node(&m, gc::MATH_ADD);
    assert!(wired(&m, (plus, "Output"), (call, "Input1")));
    assert_eq!(
        call.properties
            .get(&*wirescript::intern::sym::INPUT_SOURCES),
        Some(&wirescript::ir::Literal::Array(
            [1, 1, 0, 0, 0, 0, 0, 0]
                .into_iter()
                .map(wirescript::ir::Literal::Int)
                .collect()
        ))
    );
}

/// An `emit` of an outer signal in a `fn` uses no handle: the body sets a flag
/// reset at its start, a result slot carries it out, and each call branches
/// on that slot off its `Completed`.
#[test]
fn an_emit_returns_a_flag_each_call_fires_on() {
    let src = "let ping: exec\nvar n: int = 0\non ping { n += 1 }\nfn f(k: int) -> int {\n  emit ping\n  return k\n}\nin go: exec\non go {\n  let a = f(1)\n  let b = f(2)\n}\n";
    let m = lowered(src);
    let nodes = all_nodes(&m);
    let note = |n: &&Node, s: &str| n.note.as_deref() == Some(s);
    assert!(
        !nodes.iter().any(|n| note(n, "fn handle")),
        "an emit needs no handle"
    );
    let outputs: Vec<&&Node> = nodes
        .iter()
        .filter(|n| n.gate_class == gc::FUNCTION_OUTPUT)
        .collect();
    assert_eq!(outputs.len(), 2, "the result and the emit flag");
    assert_eq!(
        nodes
            .iter()
            .filter(|n| note(n, "fn emit flag reset") || note(n, "fn emit"))
            .count(),
        2,
        "one reset at the start, one set at the emit"
    );
    let calls: Vec<_> = nodes
        .iter()
        .filter(|n| n.gate_class == gc::FUNCTION_CALL)
        .map(|n| n.id)
        .collect();
    let mut branched = 0;
    m.walk(&mut |_, sub| {
        branched += sub
            .wires
            .iter()
            .filter(|w| {
                calls.contains(&w.source.node_id)
                    && w.source.port
                        == wirescript::ir::port_registry::WirePort::from_name("Output1")
            })
            .count();
    });
    assert_eq!(branched, 2, "each call branches on the flag slot");
    assert!(!brz(src).is_empty());
}
