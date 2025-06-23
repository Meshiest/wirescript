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
    pub fn from_unsaved(fs: &UnsavedFs) -> Result<Self, BrdbError> {
        use BrdbPendingFs::*;
        let mut worlds = vec![];

        let global_data_schema = schemas::global_data_schema();
        let owners_schema = schemas::owners_schema();
        let brick_chunk_index_schema = schemas::bricks_chunk_index_schema();
        let brick_chunk_schema = schemas::bricks_chunks_schema();
        let wires_schema = schemas::bricks_wires_schema();
        let entity_chunk_index_schema = schemas::entities_chunks_schema();

        for (world_id, world) in &fs.worlds {
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
            for (grid_id, grid) in &world.grids {
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
                    .iter()
                    .map(|(chunk, bricks)| {
                        Ok(File(
                            format!("{chunk}.mps"),
                            Some(
                                brick_chunk_schema
                                    .write_brdb("BRSavedBrickChunkSoA", bricks)
                                    .about_f(|| format!("Grids/{grid_id}/Chunks/{chunk}.mps"))?,
                            ),
                        ))
                    })
                    .collect::<Result<Vec<_>, BrdbError>>()?;
                let component_chunks_dir = grid
                    .components
                    .iter()
                    .map(|(chunk, components)| {
                        // Write the initial component SoA data to the buffer
                        let mut chunk_buf = world
                            .component_schema
                            .write_brdb("BRSavedComponentChunkSoA", components)
                            .about_f(|| format!("Grids/{grid_id}/Components/{chunk}.mps"))?;

                        // Write each component's struct data to the chunk buffer
                        for (i, component) in components.unwritten_struct_data.iter().enumerate() {
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
                            .write_brdb("BRChunkIndexSoA", &world.entity_chunk_indices)
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
                .iter()
                .map(|(chunk, entities)| {
                    Ok(File(
                        format!("{chunk}.mps"),
                        Some(
                            world
                                .entity_schema
                                .write_brdb("BRSavedEntityChunkIndexSoA", entities)
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
}
