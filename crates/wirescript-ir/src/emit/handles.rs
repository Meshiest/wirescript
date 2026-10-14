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

/// Where each brick of a world is, by its id: its save address (grid, chunk,
/// index in chunk) and its place in the world's lists (`None` for the main
/// grid's, else the `world.grids` entry) for writing to it. The first brick
/// with an id wins, as a search in grid order finds it. Changing a brick's
/// components keeps it valid.
struct BrickIndex(HashMap<usize, (Address, Place)>);

/// A brick's save address: grid, chunk, index in chunk.
type Address = (u32, brdb::ChunkIndex, u32);
/// A brick's place in the world's lists: `None` for the main grid's, else the
/// `world.grids` entry, and its index there.
type Place = (Option<usize>, usize);

impl BrickIndex {
    fn of(world: &World) -> Self {
        let mut at = HashMap::default();
        let grids = std::iter::once((None, &world.bricks)).chain(world.grids.iter().enumerate().map(|(i, (_, b))| (Some(i), b)));
        for (list, bricks) in grids {
            let grid = list.map_or(1, |i| 2 + i as u32);
            let mut in_chunk: HashMap<brdb::ChunkIndex, u32> = HashMap::default();
            for (i, b) in bricks.iter().enumerate() {
                let chunk = b.position.to_relative().0;
                let index = in_chunk.entry(chunk).or_insert(0);
                if let Some(id) = b.id {
                    at.entry(id).or_insert(((grid, chunk, *index), (list, i)));
                }
                *index += 1;
            }
        }
        Self(at)
    }

    /// The save address of brick `id`: (grid, chunk, index in chunk).
    fn address(&self, id: usize) -> Option<Address> {
        self.0.get(&id).map(|(address, _)| *address)
    }

    fn brick_mut<'w>(&self, world: &'w mut World, id: usize) -> Option<&'w mut brdb::Brick> {
        let (_, (list, i)) = self.0.get(&id)?;
        match list {
            None => world.bricks.get_mut(*i),
            Some(g) => world.grids.get_mut(*g)?.1.get_mut(*i),
        }
    }
}

/// Every handle node in the tree as (handle, target), and every
/// `GetMicrochipBrick()` var.
fn baked_refs(m: &Module, handles: &mut Vec<(NodeId, NodeId)>, chips: &mut Vec<NodeId>) {
    let handle_key = crate::intern::intern(crate::ir::HANDLE_TARGET);
    let chip_key = crate::intern::intern(crate::ir::MICROCHIP_BRICK);
    for (id, n) in &m.nodes {
        if let Some(Literal::Int(t)) = n.properties.get(&handle_key) {
            handles.push((*id, NodeId(*t as u32)));
        }
        if n.properties.contains_key(&chip_key) {
            chips.push(*id);
        }
    }
    for c in m.chips.values() {
        baked_refs(c, handles, chips);
    }
}

fn set_var_value(world: &mut World, index: &BrickIndex, holder: usize, value: WireValue) -> Option<()> {
    let brick = index.brick_mut(world, holder)?;
    let slot = brick
        .components
        .iter_mut()
        .find(|c| c.component_type_ref() == Some(gc::VAR) || c.component_type().is_some_and(|t| t.as_ref() == gc::VAR))?;
    *slot = Box::new(one_field_component(gc::VAR, "Value", Box::new(value)));
    Some(())
}

/// Write each handle var's value: a reference to its target's component.
pub(super) fn bake_handles(world: &mut World, ctx: &EmitContext, module: &Module) -> Result<(), EmitError> {
    let (mut found, mut chips) = (Vec::new(), Vec::new());
    baked_refs(module, &mut found, &mut chips);
    if found.is_empty() && chips.is_empty() {
        return Ok(());
    }
    let bricks = BrickIndex::of(world);
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
        let (grid, chunk, index) = bricks.address(target_brick).ok_or_else(|| missing("its target brick is not in the world"))?;
        let value = WireValue::ComponentRef(SavedComponentRef {
            brick: SavedBrickRef { grid_persistent_index: grid, chunk_index: chunk, brick_index_in_chunk: index },
            component_type_index: type_index,
        });
        set_var_value(world, &bricks, holder, value).ok_or_else(|| missing("its brick has no var"))?;
    }
    // `GetMicrochipBrick()`: the program's own microchip brick, on the main grid.
    for var in chips {
        let missing = |what: &str| EmitError::MissingPlacement(format!("GetMicrochipBrick() {var}: {what}"));
        let holder = *ctx.node_brick_ids.get(&var).ok_or_else(|| missing("no brick"))?;
        let (grid, chunk, index) =
            bricks.address(ctx.root_shell_brick_id).ok_or_else(|| missing("the chip brick is not in the world"))?;
        let value = WireValue::BrickRef(SavedBrickRef {
            grid_persistent_index: grid,
            chunk_index: chunk,
            brick_index_in_chunk: index,
        });
        set_var_value(world, &bricks, holder, value).ok_or_else(|| missing("its brick has no var"))?;
    }
    Ok(())
}
