//! Baking references into vars: a `fn` handle (a var inside a function plane
//! whose value is a reference to storage outside it), a ref initializer
//! (`var h: *int = &a`, `var rows: int[][] = [row0, row1]`), and
//! `GetMicrochipBrick()`.
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
        let grids = std::iter::once((None, &world.bricks)).chain(
            world
                .grids
                .iter()
                .enumerate()
                .map(|(i, (_, b))| (Some(i), b)),
        );
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

/// The refs a program bakes, found in its module tree.
#[derive(Default)]
struct BakedRefs<'m> {
    /// Each handle var as (handle, target).
    handles: Vec<(NodeId, NodeId)>,
    /// Each `GetMicrochipBrick()` var.
    chips: Vec<NodeId>,
    /// Each `ArrayVar` / `MapVar` whose initializer is refs.
    containers: Vec<&'m Node>,
}

fn baked_refs<'m>(m: &'m Module, out: &mut BakedRefs<'m>) {
    let chip_key = crate::intern::intern(crate::ir::MICROCHIP_BRICK);
    for (id, n) in &m.nodes {
        if let Some(Literal::Int(t)) = n.properties.get(&*sym::HANDLE_TARGET) {
            out.handles.push((*id, NodeId(*t as u32)));
        }
        if n.properties.contains_key(&chip_key) {
            out.chips.push(*id);
        }
        if n.properties.contains_key(&*sym::HANDLE_TARGETS) {
            out.containers.push(n);
        }
    }
    for c in m.chips.values() {
        baked_refs(c, out);
    }
}

/// Replace the `class` component on brick `holder` with one holding `value` in
/// its `Value` field.
fn set_value(
    world: &mut World,
    index: &BrickIndex,
    holder: usize,
    class: &'static str,
    value: Box<dyn AsBrdbValue>,
) -> Option<()> {
    let brick = index.brick_mut(world, holder)?;
    let slot = brick.components.iter_mut().find(|c| {
        c.component_type_ref() == Some(class)
            || c.component_type().is_some_and(|t| t.as_ref() == class)
    })?;
    *slot = Box::new(one_field_component(class, "Value", value));
    Some(())
}

fn set_var_value(
    world: &mut World,
    index: &BrickIndex,
    holder: usize,
    value: WireValue,
) -> Option<()> {
    set_value(world, index, holder, gc::VAR, Box::new(value))
}

/// A reference to `target`'s component, held by a container on brick
/// `holder`. A target on the holder's grid is addressed the way the game
/// writes a container's refs: grid 0, and the chunk as an offset from the
/// holder's chunk. One on another grid keeps its saved grid id and chunk.
fn container_ref(
    world: &World,
    ctx: &EmitContext,
    bricks: &BrickIndex,
    holder: usize,
    target: NodeId,
) -> Result<WireValue, &'static str> {
    let target_brick = *ctx
        .node_brick_ids
        .get(&target)
        .ok_or("a target has no brick")?;
    let class = *ctx
        .class_index
        .get(&target)
        .ok_or("a target has no class")?;
    let component_type_index = world
        .global_data
        .get_component_type_index(class)
        .ok_or("a target's component type is not registered")?;
    let (grid, chunk, index) = bricks
        .address(target_brick)
        .ok_or("a target brick is not in the world")?;
    let (holder_grid, holder_chunk, _) = bricks
        .address(holder)
        .ok_or("its brick is not in the world")?;
    let (grid, chunk) = if grid == holder_grid {
        let offset = brdb::ChunkIndex {
            x: chunk.x - holder_chunk.x,
            y: chunk.y - holder_chunk.y,
            z: chunk.z - holder_chunk.z,
        };
        (0, offset)
    } else {
        (grid, chunk)
    };
    Ok(WireValue::ComponentRef(SavedComponentRef {
        brick: SavedBrickRef {
            grid_persistent_index: grid,
            chunk_index: chunk,
            brick_index_in_chunk: index,
        },
        component_type_index,
    }))
}

/// Write each ref-initialized `ArrayVar` / `MapVar`: its elements (an array) or
/// entry values (a map) as references to their targets' components.
fn bake_containers(
    world: &mut World,
    ctx: &EmitContext,
    bricks: &BrickIndex,
    containers: &[&Node],
) -> Result<(), EmitError> {
    for node in containers {
        let missing = |what: &str| {
            EmitError::MissingPlacement(format!("ref initializer {}: {what}", node.id))
        };
        let holder = *ctx
            .node_brick_ids
            .get(&node.id)
            .ok_or_else(|| missing("no brick"))?;
        let target_ref = |t: &Literal| match t {
            Literal::Int(t) => {
                container_ref(world, ctx, bricks, holder, NodeId(*t as u32)).map_err(missing)
            }
            other => Err(missing(&format!("a target is {other:?}, not a node"))),
        };
        let (class, value): (&'static str, Box<dyn AsBrdbValue>) =
            match node.properties.get(&*sym::HANDLE_TARGETS) {
                Some(Literal::Array(targets)) => {
                    let values = targets.iter().map(target_ref).collect::<Result<_, _>>()?;
                    (
                        gc::ARRAY_VAR,
                        Box::new(DynArray {
                            element_type: brdb::schema::COMPONENT_HANDLE,
                            values,
                        }),
                    )
                }
                Some(Literal::Map(targets)) => {
                    let map_ty = node
                        .ports
                        .outputs
                        .iter()
                        .find(|p| p.name == *sym::MAP_VAR_REF)
                        .map(|p| &p.ty);
                    let empty = map_from_type(map_ty.unwrap_or(&Type::Any));
                    let entries = targets
                        .iter()
                        .map(|(k, t)| Ok((wire_value_as(empty.key_type, k), target_ref(t)?)))
                        .collect::<Result<_, EmitError>>()?;
                    (
                        gc::MAP_VAR,
                        Box::new(DynMap {
                            value_type: brdb::schema::COMPONENT_HANDLE,
                            entries,
                            ..empty
                        }),
                    )
                }
                other => return Err(missing(&format!("its targets are {other:?}"))),
            };
        set_value(world, bricks, holder, class, value)
            .ok_or_else(|| missing("its brick has no container"))?;
    }
    Ok(())
}

/// Write each handle var's value (a reference to its target's component), each
/// ref-initialized container's references, and each `GetMicrochipBrick()` var.
pub(super) fn bake_handles(
    world: &mut World,
    ctx: &EmitContext,
    module: &Module,
) -> Result<(), EmitError> {
    let mut refs = BakedRefs::default();
    baked_refs(module, &mut refs);
    let BakedRefs {
        handles: mut found,
        chips,
        containers,
    } = refs;
    if found.is_empty() && chips.is_empty() && containers.is_empty() {
        return Ok(());
    }
    let bricks = BrickIndex::of(world);
    bake_containers(world, ctx, &bricks, &containers)?;
    found.sort();
    for (handle, target) in found {
        let missing =
            |what: &str| EmitError::MissingPlacement(format!("fn handle {handle}: {what}"));
        let holder = *ctx
            .node_brick_ids
            .get(&handle)
            .ok_or_else(|| missing("no brick"))?;
        let target_brick = *ctx
            .node_brick_ids
            .get(&target)
            .ok_or_else(|| missing("its target has no brick"))?;
        let class = *ctx
            .class_index
            .get(&target)
            .ok_or_else(|| missing("its target has no class"))?;
        let type_index = world
            .global_data
            .get_component_type_index(class)
            .ok_or_else(|| missing("its target's component type is not registered"))?;
        let (grid, chunk, index) = bricks
            .address(target_brick)
            .ok_or_else(|| missing("its target brick is not in the world"))?;
        let value = WireValue::ComponentRef(SavedComponentRef {
            brick: SavedBrickRef {
                grid_persistent_index: grid,
                chunk_index: chunk,
                brick_index_in_chunk: index,
            },
            component_type_index: type_index,
        });
        set_var_value(world, &bricks, holder, value)
            .ok_or_else(|| missing("its brick has no var"))?;
    }
    // `GetMicrochipBrick()`: the program's own microchip brick, on the main grid.
    for var in chips {
        let missing =
            |what: &str| EmitError::MissingPlacement(format!("GetMicrochipBrick() {var}: {what}"));
        let holder = *ctx
            .node_brick_ids
            .get(&var)
            .ok_or_else(|| missing("no brick"))?;
        let (grid, chunk, index) = bricks
            .address(ctx.root_shell_brick_id)
            .ok_or_else(|| missing("the chip brick is not in the world"))?;
        let value = WireValue::BrickRef(SavedBrickRef {
            grid_persistent_index: grid,
            chunk_index: chunk,
            brick_index_in_chunk: index,
        });
        set_var_value(world, &bricks, holder, value)
            .ok_or_else(|| missing("its brick has no var"))?;
    }
    Ok(())
}
