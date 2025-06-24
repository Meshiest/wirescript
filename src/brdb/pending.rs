use std::fmt::Display;

use crate::brdb::{
    errors::BrdbError,
    schema::write,
    wrapper::{UnsavedFs, schemas},
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
    Root(Vec<BrdbPendingFs>),
    Folder(String, Option<Vec<BrdbPendingFs>>),
    File(String, Option<Vec<u8>>),
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
            let mut world_dir = vec![
                // Write GlobalData
                File(
                    "GlobalData.schema".to_owned(),
                    Some(global_data_schema.to_vec().about("GlobalData.schema")?),
                ),
                File(
                    "GlobalData.mps".to_owned(),
                    Some(
                        global_data_schema
                            .write_brdb("BRSavedGlobalDataSoA", &world.global_data)
                            .about("GlobalData.mps")?,
                    ),
                ),
                // Write Owners
                File(
                    "Owners.schema".to_owned(),
                    Some(owners_schema.to_vec().about("Owners.schema")?),
                ),
                File(
                    "Owners.mps".to_owned(),
                    Some(
                        owners_schema
                            .write_brdb("BRSavedOwnerTableSoA", &world.owners)
                            .about("Owners.mps")?,
                    ),
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
                File(
                    "ChunkIndexShared.schema".to_owned(),
                    Some(
                        brick_chunk_index_schema
                            .to_vec()
                            .about("ChunkIndexShared.schema")?,
                    ),
                ),
                File(
                    "ChunksShared.schema".to_owned(),
                    Some(brick_chunk_schema.to_vec().about("ChunksShared.schema")?),
                ),
                File(
                    "WiresShared.schema".to_owned(),
                    Some(wires_schema.to_vec().about("WiresShared.schema")?),
                ),
                // Component schema
                File(
                    "ComponentsShared.schema".to_owned(),
                    Some(
                        world
                            .component_schema
                            .to_vec()
                            .about("ComponentsShared.schema")?,
                    ),
                ),
            ];
            let mut grids_dir = vec![];

            // Bricks/Grids/N/Chunks
            // Bricks/Grids/N/Components
            // Bricks/Grids/N/Wires
            // Bricks/Grids/N/ChunkIndex.mps
            for (grid_id, grid) in world.grids {
                let mut grid_dir = vec![File(
                    "ChunkIndex.mps".to_owned(),
                    Some(
                        brick_chunk_index_schema
                            .write_brdb("BRSavedBrickChunkIndexSoA", &grid.chunk_index)
                            .about_f(|| format!("Grids/{grid_id}/ChunkIndex.mps"))?,
                    ),
                )];

                let brick_chunks_dir = grid
                    .bricks
                    .into_iter()
                    .map(|(chunk, bricks)| {
                        Ok(File(
                            format!("{chunk}.mps"),
                            Some(
                                brick_chunk_schema
                                    .write_brdb("BRSavedBrickChunkSoA", &bricks)
                                    .about_f(|| format!("Grids/{grid_id}/Chunks/{chunk}.mps"))?,
                            ),
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
                            .write_brdb("BRSavedComponentChunkSoA", &components)
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
                        Ok(File(format!("{chunk}.mps"), Some(chunk_buf)))
                    })
                    .collect::<Result<Vec<_>, BrdbError>>()?;
                let wire_chunks_dir = grid
                    .wires
                    .iter()
                    .map(|(chunk, wires)| {
                        Ok(File(
                            format!("{chunk}.mps"),
                            Some(
                                wires_schema
                                    .write_brdb("BRSavedWireChunkSoA", wires)
                                    .about_f(|| format!("Grids/{grid_id}/Wires/{chunk}.mps"))?,
                            ),
                        ))
                    })
                    .collect::<Result<Vec<_>, BrdbError>>()?;

                // Append non-empty chunk directories to the grid directory
                if !brick_chunks_dir.is_empty() {
                    grid_dir.push(Folder("Chunks".to_owned(), Some(brick_chunks_dir)));
                }
                if !component_chunks_dir.is_empty() {
                    grid_dir.push(Folder("Components".to_owned(), Some(component_chunks_dir)));
                }
                if !wire_chunks_dir.is_empty() {
                    grid_dir.push(Folder("Wires".to_owned(), Some(wire_chunks_dir)));
                }
                grids_dir.push(Folder(grid_id.to_string(), Some(grid_dir)));
            }

            let mut entities_dir = vec![
                File(
                    "ChunkIndex.schema".to_owned(),
                    Some(
                        entity_chunk_index_schema
                            .to_vec()
                            .about("ChunkIndex.schema")?,
                    ),
                ),
                File(
                    "ChunkIndex.mps".to_owned(),
                    Some(
                        entity_chunk_index_schema
                            .write_brdb("BRSavedEntityChunkIndexSoA", &world.entity_chunk_indices)
                            .about("ChunkIndex.mps")?,
                    ),
                ),
                File(
                    "ChunksShared.schema".to_owned(),
                    Some(world.entity_schema.to_vec().about("ChunksShared.schema")?),
                ),
            ];

            // Entities/Chunks/*
            let entities_chunks_dir = world
                .entity_chunks
                .into_iter()
                .map(|(chunk, entities)| {
                    Ok(File(
                        format!("{chunk}.mps"),
                        Some(
                            world
                                .entity_schema
                                .write_brdb("BRSavedEntityChunkSoA", &entities)
                                .about_f(|| format!("Entities/Chunks/{chunk}.mps"))?,
                        ),
                    ))
                })
                .collect::<Result<Vec<_>, BrdbError>>()?;

            // Only add the Chunks directory if there are any chunks
            if !entities_chunks_dir.is_empty() {
                entities_dir.push(Folder("Chunks".to_owned(), Some(entities_chunks_dir)));
            }
            bricks_dir.push(Folder("Grids".to_owned(), Some(grids_dir)));
            world_dir.push(Folder("Bricks".to_owned(), Some(bricks_dir)));
            world_dir.push(Folder("Entities".to_owned(), Some(entities_dir)));
            worlds.push(Folder(world_id.to_string(), Some(world_dir)));
        }

        let meta_dir = Folder(
            "Meta".to_owned(),
            Some(vec![
                File(
                    "Bundle.json".to_owned(),
                    Some(serde_json::to_vec(&fs.meta.bundle).about("Bundle.json")?),
                ),
                File("Screenshot.jpg".to_owned(), fs.meta.screenshot.clone()),
                File(
                    "World.json".to_owned(),
                    Some(serde_json::to_vec(&fs.meta.bundle).about("World.json")?),
                ),
            ]),
        );

        let world_dir = Folder("World".to_owned(), Some(worlds));

        Ok(Root(vec![meta_dir, world_dir]))
    }

    pub fn to_root(self) -> Option<Vec<BrdbPendingFs>> {
        match self {
            BrdbPendingFs::Root(items) => Some(items),
            _ => None,
        }
    }

    pub fn to_folder(self) -> Option<(String, Vec<BrdbPendingFs>)> {
        match self {
            BrdbPendingFs::Folder(name, items) => Some((name, items?)),
            _ => None,
        }
    }

    pub fn to_file(self) -> Option<(String, Option<Vec<u8>>)> {
        match self {
            BrdbPendingFs::File(name, items) => Some((name, items)),
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
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            BrdbPendingFs::Folder(n, items) => write!(
                f,
                "{n} [{}]",
                items
                    .as_ref()
                    .map(|v| v
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_else(|| "empty".to_string())
            ),
            BrdbPendingFs::File(n, content) => write!(
                f,
                "{n} ({})",
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

    use crate::brdb::{
        pending::BrdbPendingFs,
        schema::{ReadBrdbSchema, as_brdb::AsBrdbValue},
        wrapper::{Brick, World},
    };

    #[test]
    fn test_brick_write() -> Result<(), Box<dyn Error>> {
        let mut world = World::new();
        world.bricks.push(Brick {
            position: (0, 0, 3).into(),
            color: (255, 0, 0).into(),
            ..Default::default()
        });
        let pending = world.to_unsaved()?.to_pending()?;
        let root = pending.to_root().unwrap();

        // Get the world from the root of the tree,
        // validate the Meta dir exists
        let world_dir = 'world: {
            for root_dir in root {
                let (name, children) = root_dir.to_folder().unwrap();
                match name.as_str() {
                    // Ensure all expected meta files exist
                    "Meta" => {
                        children
                            .into_iter()
                            .for_each(|c| match c.to_file().unwrap().0.as_str() {
                                "World.json" | "Bundle.json" | "Screenshot.jpg" => {}
                                other => panic!("unknown Meta/{other}"),
                            });
                        continue;
                    }
                    "World" => {
                        assert_eq!(children.len(), 1);
                        // Get the /0 directory in the world
                        break 'world children.into_iter().next().unwrap().to_folder().unwrap().1;
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

        for d in world_dir {
            match d {
                BrdbPendingFs::File(n, Some(data)) if n == "Owners.schema" => {
                    owners_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, data) if n == "Owners.mps" => {
                    owners_vec = data;
                }
                BrdbPendingFs::File(n, Some(data)) if n == "GlobalData.schema" => {
                    global_data_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, data) if n == "GlobalData.mps" => {
                    global_data_vec = data;
                }
                BrdbPendingFs::Folder(n, items) if n == "Bricks" => {
                    bricks_dir = items;
                }
                BrdbPendingFs::Folder(n, items) if n == "Entities" => {
                    entities_dir = items;
                }
                BrdbPendingFs::File(_, _) => unreachable!("no more files"),
                BrdbPendingFs::Folder(_, _) => unreachable!("no more folders"),
                BrdbPendingFs::Root(_) => unreachable!("no root"),
            }
        }

        // Ensure global data can read completely
        let global_data = global_data_vec
            .unwrap()
            .as_slice()
            .read_brdb(global_data_schema.as_ref().unwrap(), "BRSavedGlobalDataSoA")?;

        // Ensure owners can read completely
        let _owners = owners_vec
            .unwrap()
            .as_slice()
            .read_brdb(&owners_schema.unwrap(), "BRSavedOwnerTableSoA")?;

        let mut brick_index_schema = None;
        let mut brick_schema = None;
        let mut component_schema = None;
        let mut wire_schema = None;
        let mut brick_grids = None;

        for fs in bricks_dir.unwrap() {
            match fs {
                BrdbPendingFs::Folder(n, items) if n == "Grids" => {
                    brick_grids = items;
                }
                BrdbPendingFs::File(n, Some(data)) if n == "ChunkIndexShared.schema" => {
                    brick_index_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, Some(data)) if n == "ChunksShared.schema" => {
                    brick_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, Some(data)) if n == "ComponentsShared.schema" => {
                    component_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, Some(data)) if n == "WiresShared.schema" => {
                    wire_schema = Some(data.as_slice().read_brdb_schema()?);
                }
                other => unreachable!("unknown Bricks/{other}"),
            }
        }

        let component_schema = component_schema.as_ref().unwrap();

        for grid in brick_grids.unwrap() {
            let (grid_id, children) = grid.to_folder().unwrap();
            for child in children {
                match child {
                    BrdbPendingFs::Folder(n, Some(chunks)) if n == "Chunks" => {
                        for c in chunks {
                            let _chunk = c.to_file().unwrap().1.unwrap().as_slice().read_brdb(
                                brick_schema.as_ref().unwrap(),
                                "BRSavedBrickChunkSoA",
                            )?;
                        }
                    }
                    BrdbPendingFs::Folder(n, Some(chunks)) if n == "Components" => {
                        for c in chunks {
                            let Some((_name, Some(content))) = c.to_file() else {
                                panic!("invalid chunk {n}")
                            };
                            let buf = &mut content.as_slice();
                            let chunk =
                                buf.read_brdb(&component_schema, "BRSavedComponentChunkSoA")?;

                            let type_counters = chunk
                                .as_struct()?
                                .prop("ComponentTypeCounters")?
                                .as_array()?;
                            for counter in type_counters {
                                let type_idx =
                                    counter.as_struct()?.prop("TypeIndex")?.as_brdb_u32()?;
                                let num_instances =
                                    counter.as_struct()?.prop("NumInstances")?.as_brdb_u32()?;
                                let type_name = global_data
                                    .as_struct()?
                                    .prop("ComponentTypeNames")?
                                    .as_array()?
                                    .get(type_idx as usize)
                                    .map(|s| s.as_str())
                                    .transpose()?
                                    .unwrap_or("illegal")
                                    .to_owned();
                                let struct_name = global_data
                                    .as_struct()?
                                    .prop("ComponentDataStructNames")?
                                    .as_array()?
                                    .get(type_idx as usize)
                                    .map(|s| s.as_str())
                                    .transpose()?
                                    .unwrap_or("illegal")
                                    .to_owned();

                                println!(
                                    "Component type {type_name}/{struct_name} (index {type_idx}) has {num_instances} instances"
                                );

                                if struct_name == "None" {
                                    continue;
                                }

                                for _ in 0..num_instances {
                                    let component =
                                        buf.read_brdb(&component_schema, &struct_name)?;
                                    println!("Component: {}", component.display(&component_schema));
                                }
                            }
                        }
                    }
                    BrdbPendingFs::Folder(n, Some(chunks)) if n == "Wires" => {
                        for c in chunks {
                            let _chunk =
                                c.to_file().unwrap().1.unwrap().as_slice().read_brdb(
                                    wire_schema.as_ref().unwrap(),
                                    "BRSavedWireChunkSoA",
                                )?;
                        }
                    }
                    BrdbPendingFs::File(n, data) if n == "ChunkIndex" => {
                        // read the chunk index
                        let _chunk_index = data.unwrap().as_slice().read_brdb(
                            brick_index_schema.as_ref().unwrap(),
                            "BRSavedBrickChunkIndexSoA",
                        )?;
                    }
                    other => unreachable!("unknown Grids/{grid_id}/{other}"),
                }
            }
        }

        let mut _entity_index_schema = None;
        let mut _entity_index_vec = None;
        let mut _entity_schema = None;
        let mut _entity_chunks = None;

        for fs in entities_dir.unwrap() {
            match fs {
                BrdbPendingFs::Folder(n, items) if n == "Chunks" => {
                    _entity_chunks = items;
                }
                BrdbPendingFs::File(n, data) if n == "ChunksShared.schema" => {
                    _entity_schema = Some(data.unwrap().as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, data) if n == "ChunkIndex.schema" => {
                    _entity_index_schema = Some(data.unwrap().as_slice().read_brdb_schema()?);
                }
                BrdbPendingFs::File(n, data) if n == "ChunkIndex.mps" => {
                    _entity_index_vec = data;
                }
                BrdbPendingFs::Folder(_, _) => unreachable!(),
                BrdbPendingFs::File(_, _) => todo!(),
                BrdbPendingFs::Root(_) => unreachable!(),
            }
        }

        Ok(())
    }
}
