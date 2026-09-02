//! IR + placement hints → brdb::World → .brz bytes OR .brdb file.
//!
//! Two output modes, same underlying pipeline:
//! - [`emit_brz`] returns the bytes of a `.brz` bundle — zstd-packed,
//!   portable, good for bundle transfer and in-memory preview.
//! - [`emit_brdb`] writes a `.brdb` SQLite database to a given path —
//!   this is what `BR.World.LoadAdditive <world_name>` accepts.
//!
//! Phase 1 scope:
//! - Flat root module: one outer Microchip brick, everything else inside.
//! - Caller supplies a `Placement` for every node (grid-space position).
//!   In Phase 2 the layout module fills these in automatically.
//! - Nested chips (`Module.chips`) are NOT yet handled.
//! - Literal properties are recorded on each node but only those the
//!   component schema actually models get baked in (others are skipped;
//!   Phase 2 adds the synthetic-upstream-Var emit path).
//!
//! The emit pipeline:
//!   Module + Placements
//!     → brdb::World (main grid = outer chip + inner grid = gates)
//!     → World::to_brz_vec() (zstd-packed .brz bytes)
//!     OR
//!     → World::write_brdb(path) (SQLite database file)

use crate::collections::HashMap;
#[cfg(feature = "brdb-full")]
use std::path::Path;

use std::collections::HashMap as StdMap;

use brdb::{
    AsBrdbValue, BString, BrickType, Collision, Color, IntVector, Position, Vector3f,
    WireConnection, WirePort as BrdbWirePort, World,
    assets::LiteralComponent,
    schema::{
        WireArrayVariant, WireMapKey, WireMapKeyData, WireMapValue, WireMapValueData,
        WireMapVariant, WireVariant,
    },
};

use crate::intern::{Sym, intern_static, resolve, sym};
use crate::ir::port_registry::WirePort;
use crate::layout::wall::WallLayout;
use crate::layout::{BusEnd, BusLayout, LayoutResult, NodeRotation};
use crate::ir::{Literal, Module, Node, NodeId, NodeKind, PortRef, Type, Wire, gate_class as gc};

mod options;
pub use options::*;
mod colors;
use colors::*;
mod variants;
use variants::*;
mod schema_meta;
pub use schema_meta::*;
mod components;
use components::*;
#[cfg(test)]
pub(crate) use components::roundtrip_adv_inventory_component;
mod labels;
use labels::*;
mod rerouters;
use rerouters::*;
mod partition;
pub use partition::*;
mod wires;
use wires::*;
mod module;
use module::*;

/// IR + placements → in-memory `brdb::World`. The core build step; the two
/// public `emit_*` functions wrap this and serialise to their respective
/// on-disk format.
pub fn build_world(
    module: &Module,
    layout: &LayoutResult,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<World, EmitError> {
    let mut world = World::new();
    world.meta.bundle.description = opts.description.clone();

    let wall = crate::layout::wall::assign_wall_slots(
        module,
        layout,
        (opts.chip_pos.x, opts.chip_pos.y, opts.chip_pos.z),
    );

    let (chip_brick_id, _root_entity_id, mut inner_pair) = world.add_microchip(
        opts.chip_pos.into(),
        wall.root.location,
        wall.root.extent,
        !opts.open,
    );
    inner_pair.0.rotation = WALL_ROT;

    // Module-level `@invisible`: hide the shell brick `add_microchip` just
    // pushed and drop all its collision, mirroring the `@invisible` port
    // rerouter treatment in `emit_port_rerouters`.
    if opts.invisible {
        if let Some(chip_brick) = world.bricks.last_mut() {
            chip_brick.visible = false;
            chip_brick.collision = Collision {
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
    }

    // Top-level chip label. The default is the root module's name (entry file
    // stem, or an explicit module_name override); a module-level `@label`
    // overrides it — a constant with baked text, a runtime value with an empty
    // placeholder that Pass 3.5 drives by wire. The chip brick is the one
    // `add_microchip` just pushed onto the main grid.
    if !opts.invisible {
        let dynamic_root = module.root_dynamic_label.is_some();
        let root_label: &str = if dynamic_root {
            // Wire-driven — bake an empty placeholder (the wire supplies the
            // text). `named`-style suppression doesn't apply to the root label.
            ""
        } else {
            module
                .root_label_override
                .as_deref()
                .unwrap_or_else(|| resolve(module.name))
        };
        if dynamic_root || !root_label.is_empty() {
            let label = text_label(
                &mut world,
                root_label,
                LABEL_ROTATION_DEG,
                -0.5,
                LABEL_LINE_HEIGHT,
                0.5,
                0.5,
            );
            if let Some(chip_brick) = world.bricks.last_mut() {
                chip_brick.add_component_box(Box::new(label));
            }
        }
    }

    // Push root inner grid FIRST so it gets the lowest grid ID (persistent
    // index 2). Child grids pushed during emit_module_bricks get 3, 4, etc.
    let root_grid_idx = world.grids.len();
    world.grids.push((inner_pair.0.clone(), Vec::new()));

    let mut ctx = EmitContext {
        node_brick_ids: HashMap::default(),
        class_index: HashMap::default(),
        prefab_resolver: opts.prefab_resolver.clone(),
        nested_compiler: opts.nested_compiler.clone(),
        wire_sources: HashMap::default(),
        var_labels: HashMap::default(),
        invisible: opts.invisible,
        no_gate_labels: opts.no_gate_labels,
        root_shell_brick_id: chip_brick_id,
    };
    emit_module(
        &mut world,
        &mut ctx,
        module,
        layout,
        &mut inner_pair.1,
        &wall,
        template_cache,
    )?;

    if !opts.invisible {
        // The plane header title stays static text: a constant module `@label`
        // override, else the module name. (A runtime module `@label` drives the
        // outer shell label by wire; the inner header keeps the module name.)
        let header = module
            .root_label_override
            .clone()
            .unwrap_or_else(|| resolve(module.name).to_string());
        let root_title = (!header.is_empty()).then_some(header);
        emit_plane_header(
            &mut world,
            &mut inner_pair.1,
            wall.root.extent,
            root_title.as_deref(),
            opts.module_doc.as_deref(),
        );
    }

    // Replace placeholder with actual bricks (shifted by -CHUNK_HALF).
    let shifted: Vec<brdb::Brick> = inner_pair
        .1
        .into_iter()
        .map(|mut b| {
            b.position -= brdb::Position::CHUNK_HALF;
            b
        })
        .collect();
    world.grids[root_grid_idx] = (inner_pair.0, shifted);

    // Outer rerouters for `@side`-annotated root ports, wired through the
    // chip wall (remote wires — see save.rs add_wire).
    emit_port_rerouters(&mut world, &ctx, module, opts);

    // Embed the full component catalog. `register_used_components()` below is a
    // commented-out fallback that embeds only used components, for game builds
    // whose schema reader rejects the full catalog.
    // world.register_used_components();
    // Registers every component type → struct name mapping and wire port name
    // on the World, so the save path can serialize component data.
    world.register_all_components();

    // Emit as a prefab (type "Prefab" + Meta/Prefab.json) so it pastes like a
    // native copied selection, with bounds computed from the microchip shell.
    world.make_prefab();

    Ok(world)
}

/// Shared upright rotation for every chip grid entity, PINNED IN-GAME via a
/// quat sampler (a pure −90° about Y). MEASURED mapping of grid-local axes
/// (edge-marker sampler, facing the pane from the chip's bottom-port side):
/// local +X → world up (dataflow runs bottom→top), local +Y → viewer-right,
/// local +Z (board front) → toward the viewer (the chip's bottom-port side).
/// Everything geometric hangs off this: the pane's TOP edge is the local +X
/// edge (headers go there), its horizontal half-span is `extent.y`, and its
/// vertical half-span is `extent.x` (see layout/wall.rs packing).
const WALL_ROT: brdb::Quat4f = brdb::Quat4f {
    x: 0.0,
    y: -std::f32::consts::FRAC_1_SQRT_2,
    z: 0.0,
    w: std::f32::consts::FRAC_1_SQRT_2,
};

/// Emit `.brz` bundle bytes — zstd-packed, portable, good for bundle
/// transfer and in-memory preview. `BR.World.LoadAdditive` doesn't accept
/// these directly; use [`emit_brdb`] for that.
pub fn emit_brz(
    module: &Module,
    layout: &LayoutResult,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
) -> Result<Vec<u8>, EmitError> {
    let world = build_world(module, layout, opts, template_cache)?;
    Ok(world.to_brz_vec()?)
}

/// Emit a `.brdb` SQLite database to `path`. This is the format
/// `BR.World.LoadAdditive <name>` reads from `Saved/Worlds/<name>.brdb`.
#[cfg(feature = "brdb-full")]
pub fn emit_brdb(
    module: &Module,
    layout: &LayoutResult,
    opts: &EmitOptions,
    template_cache: &std::sync::Arc<crate::template_cache::TemplateCache>,
    path: impl AsRef<Path>,
) -> Result<(), EmitError> {
    let world = build_world(module, layout, opts, template_cache)?;
    world.write_brdb(path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
