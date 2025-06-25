use std::collections::HashMap;

use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};

use crate::brdb::{
    errors::BrdbError,
    pending::BrdbPendingFs,
    schema::{BrdbSchema, BrdbSchemaGlobalData},
    wrapper::{
        Brick, BrickChunkIndexSoA, BrickChunkSoA, ChunkIndex, ComponentChunkSoA,
        EntityChunkIndexSoA, EntityChunkSoA, Guid, Owner, OwnerTableSoA, WireChunkSoA,
    },
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BundleJson {
    #[serde(rename = "type")]
    pub level_type: String,
    #[serde(rename = "iD")]
    pub id: String,
    pub name: String,
    pub version: String,
    pub tags: Vec<String>,
    pub authors: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub description: String,
    // Unknown content
    pub dependencies: Vec<serde_json::Value>,
}

impl Default for BundleJson {
    fn default() -> Self {
        Self {
            level_type: "World".to_string(),
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            name: "".to_string(),
            version: "".to_string(),
            tags: vec![],
            authors: vec![],
            created_at: "0001.01.01-00.00.00".to_string(),
            updated_at: "0001.01.01-00.00.00".to_string(),
            description: "".to_string(),
            dependencies: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorldJson {
    pub environment: String,
}

impl Default for WorldJson {
    fn default() -> Self {
        Self {
            environment: "Plate".to_string(),
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct WorldMeta {
    /// Meta/Bundle.json
    pub bundle: BundleJson,
    /// Meta/Screenshot.jpg
    pub screenshot: Option<Vec<u8>>,
    /// Meta/World.json
    pub world: WorldJson,
}

#[derive(Default)]
pub struct World {
    pub meta: WorldMeta,
    pub owners: IndexMap<Guid, Owner>,
    /// Bricks on the main grid
    pub bricks: Vec<Brick>,
    pub grids: HashMap<usize, Vec<Brick>>,
    pub wires: Vec<WireConnection>,
    // TODO: minigame, environment, entities
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_unsaved(&self) -> Result<UnsavedFs, BrdbError> {
        let mut unsaved_fs = UnsavedFs {
            meta: self.meta.clone(),
            worlds: Default::default(),
        };

        // Only one world exists right now...
        {
            let mut world = UnsavedWorld::default();
            for o in self.owners.values() {
                world.owners.add(o);
            }

            // Main grid bricks are on grid 1
            world.add_bricks_to_grid(1, &self.bricks);

            // Add all grids (by grid id)
            for (grid_id, bricks) in &self.grids {
                // TODO: error for brick grid id 1/0
                world.add_bricks_to_grid(*grid_id, bricks);
            }

            // Add the world
            unsaved_fs.worlds.insert(0, world);
        }

        Ok(unsaved_fs)
    }
}

#[derive(Debug, Clone)]
pub struct WireConnection {
    pub source: WirePort,
    pub target: WirePort,
}

#[derive(Debug, Clone)]
pub struct WirePort {
    /// The remote brick where the port is located
    pub brick_id: usize,
    /// Name of the component in the brick to connect
    pub component: String,
    /// Name of the port in the component to connect
    pub port_name: String,
}

/// All of the dynamic data needed to serialize a world
pub struct UnsavedFs {
    /// Meta/
    pub meta: WorldMeta,
    /// World/
    pub worlds: HashMap<usize, UnsavedWorld>,
}

impl UnsavedFs {
    pub fn to_pending(self) -> Result<BrdbPendingFs, BrdbError> {
        BrdbPendingFs::from_unsaved(self)
    }
}

#[derive(Default)]
pub struct UnsavedWorld {
    /// World/N/GlobalData.mps
    pub global_data: BrdbSchemaGlobalData,
    /// World/N/Owners.mps
    pub owners: OwnerTableSoA,
    /// World/N/Bricks/Grids/ComponentsShared.mps
    pub component_schema: BrdbSchema,
    /// World/N/Bricks/Grids/[key.0]/
    pub grids: HashMap<usize, UnsavedGrid>,
    /// World/N/Bricks/Entities/Chunks/[key].mps
    pub entity_chunks: HashMap<ChunkIndex, EntityChunkSoA>,
    /// World/N/Bricks/Entities/ChunksShared.schema
    pub entity_schema: BrdbSchema,
    /// World/N/Bricks/Entities/ChunkIndex.mps
    pub entity_chunk_indices: EntityChunkIndexSoA,

    /// World/N/Minigame.bp
    pub minigame: Option<()>, // TODO: minigames serialization
    /// World/N/Environment.bp
    pub environment: Option<()>, // TODO: environment serialization

    /// Internal map of brick id to (grid_id, chunk_index, brick_index_in_chunk)
    /// This is used to connect wires
    /// and is not saved to the world file.
    brick_id_map: HashMap<usize, (usize, ChunkIndex, usize)>,
}

#[derive(Default)]
pub struct UnsavedGrid {
    /// World/N/Bricks/Grids/I/ChunkIndex.mps
    pub chunk_index: BrickChunkIndexSoA,
    /// World/N/Bricks/Grids/I/Chunks/[key].mps
    pub bricks: HashMap<ChunkIndex, BrickChunkSoA>,
    /// World/N/Bricks/Grids/I/Components/[key].mps
    pub components: HashMap<ChunkIndex, ComponentChunkSoA>,
    /// World/N/Bricks/Grids/I/Wires/[key].mps
    pub wires: HashMap<ChunkIndex, WireChunkSoA>,

    /// Map of 3d chunk index to serial index in the `chunk_index` array
    /// Used to quickly find the index of a chunk in the `chunk_index` array
    chunk_index_map: HashMap<ChunkIndex, usize>,
}

impl UnsavedWorld {
    fn add_brick_meta(&mut self, brick: &Brick) {
        self.global_data.add_brick_meta(brick);

        // Iterate the components of the brick and register
        // their respective struct metadata with the component schema
        for component in &brick.components {
            let Some((enums, structs)) = component.get_schema() else {
                continue;
            };
            self.component_schema.add_meta(enums, structs);
        }
    }

    fn add_bricks_to_grid(&mut self, grid_id: usize, bricks: &[Brick]) {
        let mut grid = UnsavedGrid::default();

        // Bricks are sorted by brick type, size, and position
        for b in bricks.iter().sorted_by(|a, b| a.cmp(b)) {
            self.add_brick_meta(b);

            // Update the owner table
            let owner_id = b.owner_index.unwrap_or(0);
            self.owners.inc_bricks(owner_id);
            self.owners
                .inc_components(owner_id, b.components.len() as u32);

            // Add the brick to the grid
            let (chunk_index, brick_index) = grid.add_brick(&self.global_data, b);
            // Track the brick for wire connections
            if let Some(id) = b.id {
                self.brick_id_map
                    .insert(id, (grid_id, chunk_index, brick_index));
            }
        }

        // Add the grid to the world
        self.grids.insert(grid_id, grid);
    }
}

impl UnsavedGrid {
    /// Appends a new chunk to the chunk_index SoA, returning the index of the chunk
    fn get_chunk_index(&mut self, chunk_index: ChunkIndex) -> usize {
        // Add the chunk to the index if it doesn't exist
        if let Some(index) = self.chunk_index_map.get(&chunk_index) {
            *index
        } else {
            self.chunk_index.chunk_3d_indices.push(chunk_index);
            self.chunk_index.num_bricks.push(0);
            self.chunk_index.num_components.push(0);
            self.chunk_index.num_wires.push(0);
            let index = self.chunk_index_map.len();
            self.chunk_index_map.insert(chunk_index, index);
            index
        }
    }

    /// Add a brick to the grid, returning the chunk index and the brick index
    fn add_brick(
        &mut self,
        global_data: &BrdbSchemaGlobalData,
        brick: &Brick,
    ) -> (ChunkIndex, usize) {
        let chunk_index = brick.position.to_relative().0;
        // Lookup chunk by chunk index (or create a default one if it doesn't exist)
        self.bricks
            .entry(chunk_index)
            .or_insert_with(BrickChunkSoA::default)
            .add_brick(global_data, brick); // Add the brick to the chunk
        // Get the chunk_index SoA index for that chunk
        let i = self.get_chunk_index(chunk_index);
        // Get the brick index
        let brick_index = self.chunk_index.num_bricks[i];
        // Increment the counts for the chunk index
        self.chunk_index.num_bricks[i] += 1;
        self.chunk_index.num_components[i] += brick.components.len() as u32;

        // Write the components to the respective component chunk
        if !brick.components.is_empty() {
            let chunk = self
                .components
                .entry(chunk_index)
                .or_insert_with(ComponentChunkSoA::default);
            for c in &brick.components {
                chunk.add_component(global_data, brick_index, c.as_ref());
            }
        }

        (chunk_index, brick_index as usize)
    }
}
