//! IR + placement hints -> `brdb::World` -> .brz bytes OR .brdb file.
//!
//! Two output modes, same underlying pipeline:
//! - [`emit_brz`] returns the bytes of a `.brz` bundle (zstd-packed, portable, good for
//!   bundle transfer and in-memory preview).
//! - [`emit_brdb`] writes a `.brdb` `SQLite` database to a given path, the format
//!   `BR.World.LoadAdditive <world_name>` accepts.
//!
//! The root module becomes one outer Microchip brick with its gates on the
//! inner grid; each nested chip (`Module.chips`) is a chip brick in its
//! parent's grid with its own grid on the wall (`layout::wall`). Layout
//! supplies a `Placement` for every node. A literal property is baked into
//! the gate's data where the component schema models the field.
//!
//! The emit pipeline:
//!   Module + Placements
//!     -> `brdb::World` (main grid = outer chip + inner grid = gates)
//!     -> `World::to_brz_vec()` (zstd-packed .brz bytes)
//!     OR
//!     -> `World::write_brdb(path)` (`SQLite` database file)

#[cfg(feature = "brdb-full")]
use std::path::Path;

use brdb::{Collision, World};

use crate::intern::resolve;
use crate::ir::{Module, Program, ProgramSettings, StyleValue, brick_color};
use crate::layout::wall::assign_wall_slots;
use crate::layout::{LayoutOptions, LayoutResult, layout_with_opts};

/// The collision mask of an `@invisible` brick: it stops nothing. Spelled out field by
/// field because `Collision::default()` collides with everything, and a channel added to
/// brdb's `Collision` must be switched off here.
pub(super) fn no_collision() -> Collision {
    Collision {
        player: false,
        player1: Some(false),
        player2: Some(false),
        player3: Some(false),
        weapon: false,
        interact: false,
        tool: false,
        physics: false,
    }
}

mod options;
pub use options::*;
mod colors;
mod schema_meta;
mod variants;
pub use schema_meta::*;
mod components;
#[cfg(any(test, feature = "test-internals"))]
#[doc(hidden)]
pub use components::roundtrip_adv_inventory_component;
pub use components::{SettingParamOf, check_native_settings};
#[cfg(feature = "test-internals")]
#[doc(hidden)]
pub use variants::wire_value_as;

/// [`schema_meta::data_struct_for_gate`], for the emit tests in `wirescript`
/// that need the builtin call table. Not a stable API.
#[cfg(feature = "test-internals")]
#[doc(hidden)]
pub fn data_struct_for_gate(
    gate_class: &str,
) -> Option<(&'static str, &'static [&'static str], bool)> {
    schema_meta::data_struct_for_gate(gate_class)
}
mod labels;
use labels::{chip_name_label, emit_plane_header};
mod styles;
use styles::apply_style;
mod rerouters;
use rerouters::emit_port_rerouters;
mod partition;
pub use partition::*;
mod wires;
pub use wires::is_spawnable;
mod module;
use module::{EmitContext, emit_module};
mod overlap;
use overlap::check_no_overlap;
mod handles;

/// A program laid out and built into a world.
pub struct Assembly {
    pub world: World,
    pub layout: LayoutResult,
}

/// The backend: lay `program` out and build its world. Reads nothing but the
/// program and `opts`.
pub fn assemble(program: &Program, opts: &EmitOptions) -> Result<Assembly, EmitError> {
    assemble_with_phases(program, opts, &mut |_| {})
}

/// [`assemble`], calling `phase` with `"layout"` and then `"emit"` as each
/// stage starts, for a caller reporting progress.
pub fn assemble_with_phases(
    program: &Program,
    opts: &EmitOptions,
    phase: &mut dyn FnMut(&'static str),
) -> Result<Assembly, EmitError> {
    phase("layout");
    let layout = layout_with_opts(
        &program.module,
        &LayoutOptions::for_program(&program.settings),
    );
    phase("emit");
    let world = build_world(&program.module, &program.settings, &layout, opts)?;
    Ok(Assembly { world, layout })
}

/// IR + placements -> in-memory `brdb::World`. The core build step under [`assemble`], for
/// a caller that already holds a layout; the `emit_*` functions wrap it and serialise.
pub fn build_world(
    module: &Module,
    settings: &ProgramSettings,
    layout: &LayoutResult,
    opts: &EmitOptions,
) -> Result<World, EmitError> {
    let mut world = World::new();
    world.meta.bundle.description.clone_from(&opts.description);

    let wall = assign_wall_slots(
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
    if settings.invisible
        && let Some(chip_brick) = world.bricks.last_mut()
    {
        chip_brick.visible = false;
        chip_brick.collision = no_collision();
    }

    // Top-level chip label: the root module's name (entry file stem, or an explicit
    // module_name override) unless a module-level `@label` overrides it. A constant `@label`
    // bakes its text; a runtime one bakes an empty placeholder that Pass 3.5 drives by wire. The
    // chip brick is the one `add_microchip` just pushed onto the main grid.
    if !settings.invisible {
        let dynamic_root = module.root_dynamic_label.is_some();
        let root_label: &str = if dynamic_root {
            // Wire-driven: bake an empty placeholder (the wire supplies the text).
            ""
        } else {
            module
                .root_label_override
                .as_deref()
                .unwrap_or_else(|| resolve(module.name))
        };
        if dynamic_root || !root_label.is_empty() {
            let label = chip_name_label(&mut world, root_label);
            if let Some(chip_brick) = world.bricks.last_mut() {
                chip_brick.add_component_box(Box::new(label));
            }
        }
    }

    // Module-level `@color` / `@material` style the same shell brick: a
    // constant is set on the brick, and a runtime one adds the changer that
    // Pass 3.5 wires.
    if !module.root_style.is_empty()
        && let Some(chip_brick) = world.bricks.last_mut()
    {
        let baked = |v: &Option<StyleValue>| match v {
            Some(StyleValue::Baked(lit)) => Some(lit.clone()),
            _ => None,
        };
        let color = baked(&module.root_style.color).and_then(|lit| brick_color(&lit));
        let material = baked(&module.root_style.material);
        let wired = [&module.root_style.color, &module.root_style.material]
            .into_iter()
            .any(|v| v.as_ref().and_then(StyleValue::wired).is_some());
        apply_style(chip_brick, color.as_ref(), material.as_ref(), wired);
    }

    // Push the root inner grid first so it gets the lowest grid ID (persistent index 2);
    // child grids pushed during `emit_module` get 3, 4, and so on.
    let root_grid_idx = world.grids.len();
    world.grids.push((inner_pair.0.clone(), Vec::new()));

    let mut ctx = EmitContext::new(
        opts.prefab_resolver.clone(),
        opts.nested_compiler.clone(),
        settings,
        chip_brick_id,
    );
    emit_module(
        &mut world,
        &mut ctx,
        module,
        layout,
        &mut inner_pair.1,
        &wall,
    )?;

    if !settings.invisible {
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
            settings.module_doc.as_deref(),
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
    // chip wall (remote wires, see `add_wire` in brdb).
    emit_port_rerouters(&mut world, &mut ctx, module, opts)?;

    // Embed the full component catalog. `register_used_components()` below is an alternative
    // that embeds only used components, for game builds whose schema reader rejects the full
    // catalog.
    // world.register_used_components();
    // Registers every component type -> struct name mapping and wire port name
    // on the World, so the save path can serialize component data.
    world.register_all_components();

    // Emit as a prefab (type "Prefab" + Meta/Prefab.json) so it pastes like a
    // native copied selection, with bounds computed from the microchip shell.
    world.make_prefab();

    check_no_overlap(&world)?;
    // Last: a handle names its target by the address the writer will give it,
    // which only the finished world fixes.
    handles::bake_handles(&mut world, &ctx, module)?;
    Ok(world)
}

/// Shared upright rotation for every chip grid entity: a pure -90 degrees about Y, pinned in
/// game with a quat sampler. Grid-local axes, facing the pane from the chip's bottom-port
/// side: local +X is world up (dataflow runs bottom to top), +Y is viewer-right, +Z (board
/// front) points toward the viewer. So the pane's top edge is the local +X edge (headers go
/// there), its horizontal half-span is `extent.y` and its vertical half-span is `extent.x`
/// (see `layout::wall` packing).
const WALL_ROT: brdb::Quat4f = brdb::Quat4f {
    x: 0.0,
    y: -std::f32::consts::FRAC_1_SQRT_2,
    z: 0.0,
    w: std::f32::consts::FRAC_1_SQRT_2,
};

/// Emit `.brz` bundle bytes (zstd-packed, portable, good for bundle transfer and in-memory
/// preview). `BR.World.LoadAdditive` does not accept these directly; use [`emit_brdb`].
pub fn emit_brz(
    module: &Module,
    settings: &ProgramSettings,
    layout: &LayoutResult,
    opts: &EmitOptions,
) -> Result<Vec<u8>, EmitError> {
    let world = build_world(module, settings, layout, opts)?;
    Ok(world.to_brz_vec()?)
}

/// Emit a `.brdb` `SQLite` database to `path`. This is the format
/// `BR.World.LoadAdditive <name>` reads from `Saved/Worlds/<name>.brdb`.
#[cfg(feature = "brdb-full")]
pub fn emit_brdb(
    module: &Module,
    settings: &ProgramSettings,
    layout: &LayoutResult,
    opts: &EmitOptions,
    path: impl AsRef<Path>,
) -> Result<(), EmitError> {
    let world = build_world(module, settings, layout, opts)?;
    world.write_brdb(path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
