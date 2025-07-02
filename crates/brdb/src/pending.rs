use std::fmt::Display;

use crate::{
    errors::BrdbError,
    schema::write,
    wrapper::{
        UnsavedFs,
        schemas::{
            self, BRICK_CHUNK_INDEX_SOA, BRICK_CHUNK_SOA, BRICK_COMPONENT_SOA, BRICK_WIRE_SOA,
            ENTITY_CHUNK_INDEX_SOA, ENTITY_CHUNK_SOA, GLOBAL_DATA_SOA, OWNER_TABLE_SOA,
        },
    },
};

/// Describes an entire filesystem tree that needs to be written
/// Any `None` values indicate unchanged files or folders
/// Any absent entries will be deleted
/// All files will be hashed and checked for existing blobs
/// Any overwritten files will be marked as deleted
///
/// A revision will be created along with all of the pending
#[derive(Debug)]
pub enum BrdbPendingFs {
    Root(Vec<(String, BrdbPendingFs)>),
    Folder(Option<Vec<(String, BrdbPendingFs)>>),
    File(Option<Vec<u8>>),
}

// Helper trait for adding context to errors
trait Wrap<T> {
    fn about(self, name: impl Display) -> Result<T, BrdbError>;
    fn about_f(self, name: impl FnMut() -> String) -> Result<T, BrdbError>;
}
impl<T, E> Wrap<T> for Result<T, E>
where
    BrdbError: From<E>,
{
    fn about(self, name: impl Display) -> Result<T, BrdbError> {
        self.map_err(|e| BrdbError::from(e).wrap(name))
    }
    fn about_f(self, mut name: impl FnMut() -> String) -> Result<T, BrdbError> {
        self.map_err(|e| BrdbError::from(e).wrap(name()))
    }
}

impl BrdbPendingFs {
    pub fn from_unsaved(fs: UnsavedFs) -> Result<Self, BrdbError> {
        use BrdbPendingFs::*;
        let mut worlds = vec![];

        let global_data_schema = schemas::global_data_schema();
        let owners_schema = schemas::owners_schema();
        let brick_chunk_index_schema = schemas::bricks_chunk_index_schema();
        let brick_chunk_schema = schemas::bricks_chunks_schema();
        let wires_schema = schemas::bricks_wires_schema();
        let entity_chunk_index_schema = schemas::entities_chunk_index_schema();

        for (world_id, world) in fs.worlds {
            // This index needs to exist because the type ids of brick assets are
            // stored in the GlobalData, and the type ids of procedural
            // bricks are assigned starting from the end of the basic brick
            // asset names.
            //
            // When new brick assets are added, the length of the basic
            // brick asset names will increase, and the type ids of procedural
            // bricks in older chunks will not match the new
            // basic brick asset names.
            //
            // This offset allows older chunks to properly load, assuming the global
            // data does not change the order of brick asset names.
            let proc_brick_starting_index = world.global_data.basic_brick_asset_names.len() as u32;

            let mut world_dir = vec![
                // Write GlobalData
                (
                    "GlobalData.schema".to_owned(),
                    File(Some(
                        global_data_schema.to_vec().about("GlobalData.schema")?,
                    )),
                ),
                (
                    "GlobalData.mps".to_owned(),
                    File(Some(
                        global_data_schema
                            .write_brdb(GLOBAL_DATA_SOA, &world.global_data)
                            .about("GlobalData.mps")?,
                    )),
                ),
                // Write Owners
                (
                    "Owners.schema".to_owned(),
                    File(Some(owners_schema.to_vec().about("Owners.schema")?)),
                ),
                (
                    "Owners.mps".to_owned(),
                    File(Some(
                        owners_schema
                            .write_brdb(OWNER_TABLE_SOA, &world.owners)
                            .about("Owners.mps")?,
                    )),
                ),
            ];

            if let Some(_env) = world.environment.as_ref() {
                // TODO: Write Environment.bp
            }
            if let Some(_minigame) = world.minigame.as_ref() {
                // TODO: Write Minigame.bp
            }

            let mut bricks_dir = vec![
                // Shared schemas
                (
                    "ChunkIndexShared.schema".to_owned(),
                    File(Some(
                        brick_chunk_index_schema
                            .to_vec()
                            .about("ChunkIndexShared.schema")?,
                    )),
                ),
                (
                    "ChunksShared.schema".to_owned(),
                    File(Some(
                        brick_chunk_schema.to_vec().about("ChunksShared.schema")?,
                    )),
                ),
                (
                    "WiresShared.schema".to_owned(),
                    File(Some(wires_schema.to_vec().about("WiresShared.schema")?)),
                ),
                // Component schema
                (
                    "ComponentsShared.schema".to_owned(),
                    File(Some(
                        world
                            .component_schema
                            .to_vec()
                            .about("ComponentsShared.schema")?,
                    )),
                ),
            ];
            let mut grids_dir = vec![];

            // Bricks/Grids/N/Chunks
            // Bricks/Grids/N/Components
            // Bricks/Grids/N/Wires
            // Bricks/Grids/N/ChunkIndex.mps
            for (grid_id, grid) in world.grids {
                let mut grid_dir = vec![(
                    "ChunkIndex.mps".to_owned(),
                    File(Some(
                        brick_chunk_index_schema
                            .write_brdb(BRICK_CHUNK_INDEX_SOA, &grid.chunk_index)
                            .about_f(|| format!("Grids/{grid_id}/ChunkIndex.mps"))?,
                    )),
                )];

                let brick_chunks_dir = grid
                    .bricks
                    .into_iter()
                    .map(|(chunk, mut bricks)| {
                        bricks.procedural_brick_starting_index = proc_brick_starting_index;
                        Ok((
                            format!("{chunk}.mps"),
                            File(Some(
                                brick_chunk_schema
                                    .write_brdb(BRICK_CHUNK_SOA, &bricks)
                                    .about_f(|| format!("Grids/{grid_id}/Chunks/{chunk}.mps"))?,
                            )),
                        ))
                    })
                    .collect::<Result<Vec<_>, BrdbError>>()?;
                let component_chunks_dir = grid
                    .components
                    .into_iter()
                    .map(|(chunk, components)| {
                        // Write the initial component SoA data to the buffer
                        let mut chunk_buf = world
                            .component_schema
                            .write_brdb(BRICK_COMPONENT_SOA, &components)
                            .about_f(|| format!("Grids/{grid_id}/Components/{chunk}.mps"))?;

                        // Write each component's struct data to the chunk buffer
                        for (i, component) in
                            components.unwritten_struct_data.into_iter().enumerate()
                        {
                            // Unwrap safety: The component can only be added to unwritten_struct_data if
                            // get_schema_struct() returns Some(_, Some(_))
                            let ty = component.get_schema_struct().unwrap().1.unwrap();

                            // Append to the buffer and serialize the component's data
                            write::write_brdb(
                                &world.component_schema,
                                &mut chunk_buf,
                                &ty,
                                component.as_ref(),
                            )
                            .about_f(|| {
                                format!(
                                    "Grids/{grid_id}/Components/{chunk}.mps component {i} ({ty})"
                                )
                            })?;
                        }
                        Ok((format!("{chunk}.mps"), File(Some(chunk_buf))))
                    })
                    .collect::<Result<Vec<_>, BrdbError>>()?;
                let wire_chunks_dir = grid
                    .wires
                    .iter()
                    .map(|(chunk, wires)| {
                        Ok((
                            format!("{chunk}.mps"),
                            File(Some(
                                wires_schema
                                    .write_brdb(BRICK_WIRE_SOA, wires)
                                    .about_f(|| format!("Grids/{grid_id}/Wires/{chunk}.mps"))?,
                            )),
                        ))
                    })
                    .collect::<Result<Vec<_>, BrdbError>>()?;

                // Append non-empty chunk directories to the grid directory
                if !brick_chunks_dir.is_empty() {
                    grid_dir.push(("Chunks".to_owned(), Folder(Some(brick_chunks_dir))));
                }
                if !component_chunks_dir.is_empty() {
                    grid_dir.push(("Components".to_owned(), Folder(Some(component_chunks_dir))));
                }
                if !wire_chunks_dir.is_empty() {
                    grid_dir.push(("Wires".to_owned(), Folder(Some(wire_chunks_dir))));
                }
                grids_dir.push((grid_id.to_string(), Folder(Some(grid_dir))));
            }

            let mut entities_dir = vec![
                (
                    "ChunkIndex.schema".to_owned(),
                    File(Some(
                        entity_chunk_index_schema
                            .to_vec()
                            .about("ChunkIndex.schema")?,
                    )),
                ),
                (
                    "ChunkIndex.mps".to_owned(),
                    File(Some(
                        entity_chunk_index_schema
                            .write_brdb(ENTITY_CHUNK_INDEX_SOA, &world.entity_chunk_index)
                            .about("ChunkIndex.mps")?,
                    )),
                ),
                (
                    "ChunksShared.schema".to_owned(),
                    File(Some(
                        world.entity_schema.to_vec().about("ChunksShared.schema")?,
                    )),
                ),
            ];

            // Entities/Chunks/*
            let entities_chunks_dir = world
                .entity_chunks
                .into_iter()
                .map(|(chunk, entities)| {
                    let mut buf = world
                        .entity_schema
                        .write_brdb(ENTITY_CHUNK_SOA, &entities)
                        .about_f(|| format!("Entities/Chunks/{chunk}.mps"))?;

                    for (i, entity_data) in entities.unwritten_struct_data.into_iter().enumerate() {
                        // Unwrap safety: The component can only be added to unwritten_struct_data if
                        // get_schema_struct() returns Some(_, Some(_))
                        let Some((_, Some(struct_ty))) = entity_data.get_schema_struct() else {
                            // Cannot write entity data without a type
                            continue;
                        };

                        // Append to the buffer and serialize the component's data
                        write::write_brdb(
                            &world.entity_schema,
                            &mut buf,
                            struct_ty.as_ref(),
                            &**entity_data,
                        )
                        .about_f(|| {
                            format!("Entities/Chunks/{chunk}.mps entity {i} ({struct_ty})")
                        })?;
                    }

                    Ok((format!("{chunk}.mps"), File(Some(buf))))
                })
                .collect::<Result<Vec<_>, BrdbError>>()?;

            // Only add the Chunks directory if there are any chunks
            if !entities_chunks_dir.is_empty() {
                entities_dir.push(("Chunks".to_owned(), Folder(Some(entities_chunks_dir))));
            }
            bricks_dir.push(("Grids".to_owned(), Folder(Some(grids_dir))));
            world_dir.push(("Bricks".to_owned(), Folder(Some(bricks_dir))));
            world_dir.push(("Entities".to_owned(), Folder(Some(entities_dir))));
            worlds.push((world_id.to_string(), Folder(Some(world_dir))));
        }

        let meta_dir = (
            "Meta".to_owned(),
            Folder(Some(vec![
                (
                    "Bundle.json".to_owned(),
                    File(Some(
                        serde_json::to_vec(&fs.meta.bundle).about("Bundle.json")?,
                    )),
                ),
                (
                    "Screenshot.jpg".to_owned(),
                    File(fs.meta.screenshot.clone()),
                ),
                (
                    "World.json".to_owned(),
                    File(Some(
                        serde_json::to_vec(&fs.meta.world).about("World.json")?,
                    )),
                ),
            ])),
        );

        let world_dir = ("World".to_owned(), Folder(Some(worlds)));

        Ok(Root(vec![meta_dir, world_dir]))
    }

    pub fn to_root(self) -> Option<Vec<(String, BrdbPendingFs)>> {
        match self {
            BrdbPendingFs::Root(items) => Some(items),
            _ => None,
        }
    }

    pub fn to_folder(self) -> Option<Vec<(String, BrdbPendingFs)>> {
        match self {
            BrdbPendingFs::Folder(items) => items,
            _ => None,
        }
    }

    pub fn to_file(self) -> Option<Vec<u8>> {
        match self {
            BrdbPendingFs::File(items) => items,
            _ => None,
        }
    }
}

impl Display for BrdbPendingFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrdbPendingFs::Root(items) => write!(
                f,
                "[{}]",
                items
                    .iter()
                    .map(|(n, i)| format!("{n} {i}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            BrdbPendingFs::Folder(items) => write!(
                f,
                "[{}]",
                items
                    .as_ref()
                    .map(|v| v
                        .iter()
                        .map(|(n, i)| format!("{n} {i}"))
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_else(|| "empty".to_string())
            ),
            BrdbPendingFs::File(content) => write!(
                f,
                "({})",
                content
                    .as_ref()
                    .map(|v| v.len().to_string())
                    .unwrap_or_default()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::{
        pending::BrdbPendingFs,
        schema::{ReadBrdbSchema, as_brdb::AsBrdbValue},
        wrapper::{
            Brick, Entity, World,
            schemas::{
                BRICK_CHUNK_INDEX_SOA, BRICK_CHUNK_SOA, BRICK_COMPONENT_SOA, BRICK_WIRE_SOA,
                ENTITY_CHUNK_SOA, GLOBAL_DATA_SOA, OWNER_TABLE_SOA,
            },
        },
    };

    #[test]
    fn test_brick_write() -> Result<(), Box<dyn Error>> {
        let mut world = World::new();
        world.add_brick(Brick {
            position: (0, 0, 3).into(),
            color: (255, 0, 0).into(),
            ..Default::default()
        });
        world.add_brick_grid(
            Entity {
                frozen: true,
                ..Default::default()
            },
            [Brick {
                position: (0, 0, 3).into(),
                color: (255, 0, 0).into(),
                ..Default::default()
            }],
        );

        let pending = world.to_unsaved()?.to_pending()?;
        let root = pending.to_root().unwrap();

        // Get the world from the root of the tree,
        // validate the Meta dir exists
        let world_dir = 'world: {
            for (name, root_dir) in root {
                let children = root_dir.to_folder().unwrap();
                match name.as_str() {
                    // Ensure all expected meta files exist
                    "Meta" => {
                        children.into_iter().for_each(|(n, _)| match n.as_str() {
                            "World.json" | "Bundle.json" | "Screenshot.jpg" => {}
                            other => panic!("unknown Meta/{other}"),
                        });
                        continue;
                    }
                    "World" => {
                        assert_eq!(children.len(), 1);
                        // Get the /0 directory in the world
                        break 'world children.into_iter().next().unwrap().1.to_folder().unwrap();
                    }
                    other => panic!("unknown {other}"),
                };
            }
            unreachable!()
        };

        let mut owners_schema = None;
        let mut owners_vec = None;
        let mut global_data_schema = None;
        let mut global_data_vec = None;
        let mut bricks_dir = None;
        let mut entities_dir = None;

        for (n, d) in world_dir {
            match (n.as_str(), d) {
                ("Owners.schema", BrdbPendingFs::File(Some(data))) => {
                    owners_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                ("Owners.mps", BrdbPendingFs::File(data)) => {
                    owners_vec = data;
                }
                ("GlobalData.schema", BrdbPendingFs::File(Some(data))) => {
                    global_data_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                ("GlobalData.mps", BrdbPendingFs::File(data)) => {
                    global_data_vec = data;
                }
                ("Bricks", BrdbPendingFs::Folder(items)) => {
                    bricks_dir = items;
                }
                ("Entities", BrdbPendingFs::Folder(items)) => {
                    entities_dir = items;
                }
                (name, BrdbPendingFs::File(_)) => unreachable!("{name}: no more files"),
                (name, BrdbPendingFs::Folder(_)) => unreachable!("{name}: no more folders"),
                (name, BrdbPendingFs::Root(_)) => unreachable!("{name}: no root"),
            }
        }

        // Ensure global data can read completely
        let global_data = global_data_vec
            .unwrap()
            .as_slice()
            .read_brdb(global_data_schema.as_ref().unwrap(), GLOBAL_DATA_SOA)?;

        // Ensure owners can read completely
        let _owners = owners_vec
            .unwrap()
            .as_slice()
            .read_brdb(&owners_schema.unwrap(), OWNER_TABLE_SOA)?;

        let mut brick_index_schema = None;
        let mut brick_schema = None;
        let mut component_schema = None;
        let mut wire_schema = None;
        let mut brick_grids = None;

        for (n, fs) in bricks_dir.unwrap() {
            match (n.as_str(), fs) {
                ("Grids", BrdbPendingFs::Folder(items)) => {
                    brick_grids = items;
                }
                ("ChunkIndexShared.schema", BrdbPendingFs::File(Some(data))) => {
                    brick_index_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                ("ChunksShared.schema", BrdbPendingFs::File(Some(data))) => {
                    brick_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                ("ComponentsShared.schema", BrdbPendingFs::File(Some(data))) => {
                    component_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                ("WiresShared.schema", BrdbPendingFs::File(Some(data))) => {
                    wire_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                (other, f) => unreachable!("unknown Bricks/{other}: {f}"),
            }
        }

        let component_schema = component_schema.as_ref().unwrap();

        for (grid_id, grid) in brick_grids.unwrap() {
            let children = grid.to_folder().unwrap();
            for (n, child) in children {
                match (n.as_str(), child) {
                    ("Chunks", BrdbPendingFs::Folder(Some(chunks))) => {
                        for (_, c) in chunks {
                            let _chunk = c
                                .to_file()
                                .unwrap()
                                .as_slice()
                                .read_brdb(brick_schema.as_ref().unwrap(), BRICK_CHUNK_SOA)?;
                        }
                    }
                    ("Components", BrdbPendingFs::Folder(Some(chunks))) => {
                        for (_, c) in chunks {
                            let content = c.to_file().unwrap();
                            let buf = &mut content.as_slice();
                            let chunk = buf.read_brdb(&component_schema, BRICK_COMPONENT_SOA)?;

                            let type_counters = chunk.prop("ComponentTypeCounters")?.as_array()?;
                            for counter in type_counters {
                                let type_idx =
                                    counter.as_struct()?.prop("TypeIndex")?.as_brdb_u32()?;
                                let num_instances =
                                    counter.as_struct()?.prop("NumInstances")?.as_brdb_u32()?;
                                let struct_name = global_data
                                    .prop("ComponentDataStructNames")?
                                    .index(type_idx as usize)?
                                    .map(|s| s.as_str())
                                    .transpose()?
                                    .unwrap_or("illegal")
                                    .to_owned();

                                if struct_name == "None" {
                                    continue;
                                }

                                for _ in 0..num_instances {
                                    let _component =
                                        buf.read_brdb(&component_schema, &struct_name)?;
                                }
                            }
                        }
                    }
                    ("Wires", BrdbPendingFs::Folder(Some(chunks))) => {
                        for (_, c) in chunks {
                            let _chunk = c
                                .to_file()
                                .unwrap()
                                .as_slice()
                                .read_brdb(wire_schema.as_ref().unwrap(), BRICK_WIRE_SOA)?;
                        }
                    }
                    ("ChunkIndex.mps", BrdbPendingFs::File(data)) => {
                        // read the chunk index
                        let _chunk_index = data.unwrap().as_slice().read_brdb(
                            brick_index_schema.as_ref().unwrap(),
                            BRICK_CHUNK_INDEX_SOA,
                        )?;
                    }
                    (n, other) => unreachable!("unknown Grids/{grid_id}/{n}: {other}"),
                }
            }
        }

        let mut _entity_index_schema = None;
        let mut _entity_index_vec = None;
        let mut entity_schema = None;
        let mut entity_chunks = None;

        for (n, fs) in entities_dir.unwrap() {
            match (n.as_str(), fs) {
                ("Chunks", BrdbPendingFs::Folder(items)) => {
                    entity_chunks = items;
                }
                ("ChunksShared.schema", BrdbPendingFs::File(data)) => {
                    entity_schema = Some(data.unwrap().as_slice().read_brdb_schema()?);
                }
                ("ChunkIndex.schema", BrdbPendingFs::File(data)) => {
                    _entity_index_schema = Some(data.unwrap().as_slice().read_brdb_schema()?);
                }
                ("ChunkIndex.mps", BrdbPendingFs::File(data)) => {
                    _entity_index_vec = data;
                }
                (n, other) => unreachable!("unknown Entities/{n}: {other}"),
            }
        }

        // Ensure all the chunks can be read
        for (_chunk_id, chunk) in entity_chunks.unwrap() {
            let content = chunk.to_file().unwrap();
            let buf = &mut content.as_slice();
            let _chunk_data = buf.read_brdb(entity_schema.as_ref().unwrap(), ENTITY_CHUNK_SOA)?;
        }

        Ok(())
    }
}
