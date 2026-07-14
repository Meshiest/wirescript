//! Every chip grid entity is rotated upright (`WALL_ROT`) and positioned by
//! the wall layout instead of hovering above its shell brick.

use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, compile_to_world};

const SRC: &str = "\
in tick: exec\n\
chip Counter(t: exec) -> (n: int) {\n\
  var c: int = 0\n\
  on t { c = c + 1 }\n\
  out n = c\n\
}\n\
let a = Counter(tick)\n\
@closed chip { var hidden: int = 0 }\n\
out total = a.n\n";

#[test]
fn grids_are_upright_and_stacked() {
    let r = compile_to_world(
        CompileInput {
            source: SRC,
            file: "wall.ws",
            module_name: None,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    // WALL_ROT: pure −90° about Y, in-game pinned via a quat sampler — see
    // emit.rs WALL_ROT for the axis mapping.
    let f = std::f32::consts::FRAC_1_SQRT_2;
    assert!(!r.world.grids.is_empty());
    for (entity, _bricks) in &r.world.grids {
        let q = &entity.rotation;
        assert!(
            q.x == 0.0 && (q.y - -f).abs() < 1e-6 && q.z == 0.0 && (q.w - f).abs() < 1e-6,
            "grid entity should carry WALL_ROT, got {:?}",
            entity.rotation
        );
    }

    // Root grid is pushed first; children sit strictly above it.
    let root_z = r.world.grids[0].0.location.z;
    for (entity, _) in &r.world.grids[1..] {
        assert!(
            entity.location.z > root_z,
            "child plane ({}) should sit above the root plane ({root_z})",
            entity.location.z
        );
    }

    // All planes sit in the wall's plane: they share the chip brick's X
    // (0 by default) — the wall faces −X and rows run along world Y.
    for (entity, _) in &r.world.grids {
        assert_eq!(entity.location.x, 0.0);
    }
}

#[test]
fn planes_get_invisible_header_bricks() {
    let src = "\
in tick: exec\n\
/// Counts ticks forever.\n\
chip Counter(t: exec) -> (n: int) {\n\
  var c: int = 0\n\
  on t { c = c + 1 }\n\
  out n = c\n\
}\n\
let a = Counter(tick)\n\
out total = a.n\n";
    let r = compile_to_world(
        CompileInput {
            source: src,
            file: "wall.ws",
            module_name: None,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    let is_text_display = |c: &Box<dyn brdb::BrdbComponent>| {
        c.component_type()
            .map(|t| t.to_string() == "Component_TextDisplay")
            .unwrap_or(false)
    };

    // Exactly two header bricks: root plane + Counter plane. Headers are the
    // only INVISIBLE bricks in the output.
    let headers: Vec<_> = r
        .world
        .grids
        .iter()
        .flat_map(|(_e, bricks)| bricks)
        .filter(|b| !b.visible)
        .collect();
    assert_eq!(headers.len(), 2, "root + Counter headers");
    for h in &headers {
        assert!(
            h.components.iter().any(is_text_display),
            "header brick must carry a TextDisplay"
        );
        assert_eq!(h.components.len(), 1, "header carries ONLY the text");
    }

    // The header must sit ABOVE the plane's gate area: the game DROPS
    // overlapping bricks at load (orphaning their components and dangling
    // their wires), and a gate can occupy the layout's top-centre. Cheap
    // guard: no two bricks on any grid share an exact position.
    for (entity, bricks) in &r.world.grids {
        let mut seen = std::collections::HashSet::new();
        for b in bricks {
            assert!(
                seen.insert((b.position.x, b.position.y, b.position.z)),
                "duplicate brick position {:?} on grid {:?}",
                b.position,
                entity.id
            );
        }
    }
}
