//! Baking references into vars: a `fn` handle (a var inside a function plane
//! whose value is a reference to storage outside it), and `GetMicrochipBrick()`.
//!
//! A reference names its target by save address: grid, chunk and index in the
//! chunk, plus the index of the component's type in the save's type table. The
//! writer assigns these deterministically from the finished world: the main
//! grid is 1 and each `world.grids` entry is 2, 3, ... in order; a brick's chunk
//! is its position's chunk; its index counts the bricks before it in the same
//! chunk of the same grid. So the bake runs last, on the finished world.

use super::*;

use brdb::schema::{SavedBrickRef, SavedComponentRef, WireValue};

/// The save address of brick `id`: (grid, chunk, index in chunk).
fn brick_address(world: &World, id: usize) -> Option<(u32, brdb::ChunkIndex, u32)> {
    let grids = std::iter::once((1u32, &world.bricks))
        .chain(world.grids.iter().enumerate().map(|(i, (_, bricks))| (2 + i as u32, bricks)));
    for (grid, bricks) in grids {
        let Some(at) = bricks.iter().position(|b| b.id == Some(id)) else { continue };
        let chunk = bricks[at].position.to_relative().0;
        let index = bricks[..at].iter().filter(|b| b.position.to_relative().0 == chunk).count();
        return Some((grid, chunk, index as u32));
    }
    None
}

fn brick_mut(world: &mut World, id: usize) -> Option<&mut brdb::Brick> {
    let grids = world.grids.iter_mut().flat_map(|(_, bricks)| bricks.iter_mut());
    world.bricks.iter_mut().chain(grids).find(|b| b.id == Some(id))
}

/// Every handle node in the tree: (handle, target).
fn handles(m: &Module, out: &mut Vec<(NodeId, NodeId)>) {
    let key = crate::intern::intern(crate::lower::function::HANDLE_TARGET);
    for (id, n) in &m.nodes {
        if let Some(Literal::Int(t)) = n.properties.get(&key) {
            out.push((*id, NodeId(*t as u32)));
        }
    }
    for c in m.chips.values() {
        handles(c, out);
    }
}

/// Every `GetMicrochipBrick()` var in the tree.
fn microchip_bricks(m: &Module, out: &mut Vec<NodeId>) {
    let key = crate::intern::intern(crate::lower::function::MICROCHIP_BRICK);
    out.extend(m.nodes.iter().filter(|(_, n)| n.properties.contains_key(&key)).map(|(id, _)| *id));
    for c in m.chips.values() {
        microchip_bricks(c, out);
    }
}

fn set_var_value(world: &mut World, holder: usize, value: WireValue) -> Option<()> {
    let brick = brick_mut(world, holder)?;
    let slot = brick
        .components
        .iter_mut()
        .find(|c| c.component_type_ref() == Some(gc::VAR) || c.component_type().is_some_and(|t| t.as_ref() == gc::VAR))?;
    let mut data: StdMap<BString, Box<dyn AsBrdbValue>> = StdMap::new();
    data.insert("Value".into(), Box::new(value));
    *slot = Box::new(LiteralComponent::new_from_data(gc::VAR, std::sync::Arc::new(data)));
    Some(())
}

/// Write each handle var's value: a reference to its target's component.
pub(super) fn bake_handles(world: &mut World, ctx: &EmitContext, module: &Module) -> Result<(), EmitError> {
    let mut found = Vec::new();
    handles(module, &mut found);
    found.sort();
    for (handle, target) in found {
        let missing = |what: &str| EmitError::MissingPlacement(format!("fn handle {handle}: {what}"));
        let holder = *ctx.node_brick_ids.get(&handle).ok_or_else(|| missing("no brick"))?;
        let target_brick = *ctx.node_brick_ids.get(&target).ok_or_else(|| missing("its target has no brick"))?;
        let class = *ctx.class_index.get(&target).ok_or_else(|| missing("its target has no class"))?;
        let type_index = world
            .global_data
            .get_component_type_index(class)
            .ok_or_else(|| missing("its target's component type is not registered"))?;
        let (grid, chunk, index) = brick_address(world, target_brick).ok_or_else(|| missing("its target brick is not in the world"))?;
        let value = WireValue::ComponentRef(SavedComponentRef {
            brick: SavedBrickRef { grid_persistent_index: grid, chunk_index: chunk, brick_index_in_chunk: index },
            component_type_index: type_index,
        });
        set_var_value(world, holder, value).ok_or_else(|| missing("its brick has no var"))?;
    }
    // `GetMicrochipBrick()`: the program's own microchip brick, on the main grid.
    let mut chips = Vec::new();
    microchip_bricks(module, &mut chips);
    for var in chips {
        let missing = |what: &str| EmitError::MissingPlacement(format!("GetMicrochipBrick() {var}: {what}"));
        let holder = *ctx.node_brick_ids.get(&var).ok_or_else(|| missing("no brick"))?;
        let (grid, chunk, index) =
            brick_address(world, ctx.root_shell_brick_id).ok_or_else(|| missing("the chip brick is not in the world"))?;
        let value = WireValue::BrickRef(SavedBrickRef {
            grid_persistent_index: grid,
            chunk_index: chunk,
            brick_index_in_chunk: index,
        });
        set_var_value(world, holder, value).ok_or_else(|| missing("its brick has no var"))?;
    }
    Ok(())
}
