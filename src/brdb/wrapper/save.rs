use serde::{Deserialize, Serialize};

use crate::brdb::wrapper::Brick;

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
    pub bundle: BundleJson,
    pub world: WorldJson,
}

#[derive(Debug, Default, Clone)]
pub struct World {
    pub meta: WorldMeta,
    pub main_grid: Vec<Brick>,
    pub grids: Vec<BrickGrid>,
    pub wires: Vec<WireConnection>,
    // TODO: minigame, environment
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

#[derive(Debug, Default, Clone)]
pub struct BrickGrid {
    pub id: u32,
    pub bricks: Vec<Brick>,
}
