use crate::collections::HashMap;
use std::sync::OnceLock;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct GatePortDoc {
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub tooltip: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GateDoc {
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub description: String,
    pub inputs: HashMap<String, GatePortDoc>,
    #[allow(dead_code)]
    pub outputs: HashMap<String, GatePortDoc>,
}

pub fn gate_docs() -> &'static HashMap<String, GateDoc> {
    static GATE_DOCS: OnceLock<HashMap<String, GateDoc>> = OnceLock::new();
    GATE_DOCS.get_or_init(|| {
        let json_str = include_str!("../../gate_docs.json");
        serde_json::from_str(json_str).unwrap_or_default()
    })
}
