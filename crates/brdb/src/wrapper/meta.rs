use serde::{Deserialize, Serialize};

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
            description: "A Generated World".to_string(),
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
