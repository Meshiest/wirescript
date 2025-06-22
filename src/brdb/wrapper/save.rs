use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::brdb::{
    schema::{BrdbSchema, BrdbSchemaGlobalData},
    wrapper::{
        Brick, BrickChunkIndexSoA, BrickChunkSoA, ChunkIndex, ComponentChunkSoA,
        EntityChunkIndexSoA, EntityChunkSoA, OwnerTableSoA, WireChunkSoA,
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
    /// Meta/Bundle.Json
    pub bundle: BundleJson,
    /// Meta/Screenshot.jpg
    pub screenshot: Option<Vec<u8>>,
    /// Meta/World.json
    pub world: WorldJson,
}

#[derive(Default)]
pub struct World {
    pub meta: WorldMeta,
    pub main_grid: Vec<Brick>,
    pub grids: Vec<BrickGrid>,
    pub wires: Vec<WireConnection>,
    // TODO: minigame, environment, entities
}

#[derive(Debug, Clone)]
pub struct WireConnection {
    pub source: WirePort,
    pub target: WirePort,
}

#[derive(Debug, Clone)]
pub struct WirePort {
    /// The remote brick where the port is located
    pub brick: RemoteBrick,
    /// Name of the component in the brick to connect
    pub component: String,
    /// Name of the port in the component to connect
    pub port_name: String,
}

/// A reference to a brick on a remote grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoteBrick {
    pub grid_id: usize,
    pub brick_id: usize,
}

#[derive(Default, Clone)]
pub struct BrickGrid {
    pub id: u32,
    pub bricks: Vec<Brick>,
}

/// All of the dynamic data needed to serialize a world
pub struct UnsavedFs {
    /// Meta/
    pub meta: WorldMeta,
    /// World/
    pub worlds: HashMap<usize, UnsavedWorld>,
}

pub struct UnsavedWorld {
    /// World/N/GlobalData.mps
    pub global_data: BrdbSchemaGlobalData,
    /// World/N/Owners.mps
    pub owners: OwnerTableSoA,
    /// World/N/Bricks/Grids/[key.0]/Chunks/[key.1].mps
    pub brick_chunks: HashMap<(usize, ChunkIndex), BrickChunkSoA>,
    /// World/N/Bricks/Grids/[key.0]/Components/[key.1].mps
    pub component_chunks: HashMap<(usize, ChunkIndex), ComponentChunkSoA>,
    /// World/N/Bricks/Grids/ComponentsShared.mps
    pub component_schema: BrdbSchema,
    /// World/N/Bricks/Grids/[key.0]/Wires/[key.1].mps
    pub wire_chunks: HashMap<(usize, ChunkIndex), WireChunkSoA>,
    /// World/N/Bricks/Grids/[key.0]/ChunkIndex.mps
    pub grid_chunk_indices: HashMap<usize, BrickChunkIndexSoA>,
    /// World/N/Bricks/Entities/Chunks/[key].mps
    pub entity_chunks: HashMap<ChunkIndex, EntityChunkSoA>,
    /// World/N/Bricks/Entities/ChunksShared.mps
    pub entity_schema: BrdbSchema,
    /// World/N/Bricks/Entities/ChunkIndex.mps
    pub entity_chunk_indices: EntityChunkIndexSoA,

    /// World/N/Minigame.bp
    pub minigame: Option<()>, // TODO: minigames serialization
    /// World/N/Environment.bp
    pub environment: Option<()>, // TODO: environment serialization
}
