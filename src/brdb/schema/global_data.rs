use std::collections::HashSet;

use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

use crate::brdb::schema::as_brdb::{AsBrdbIter, AsBrdbValue, BrdbArrayIter};

#[derive(Default, Serialize, Deserialize)]
pub struct BrdbSchemaGlobalData {
    pub entity_type_names: IndexSet<String>,
    pub basic_brick_asset_names: IndexSet<String>,
    pub procedural_brick_asset_names: IndexSet<String>,
    pub material_asset_names: IndexSet<String>,
    pub component_type_names: IndexSet<String>,
    pub component_data_struct_names: IndexSet<String>,
    pub component_wire_port_names: IndexSet<String>,
    /// Internal set for type checking, not used in the BRDB.
    pub external_asset_types: HashSet<String>,
    pub external_asset_references: IndexSet<(String, String)>,
}

impl AsBrdbValue for BrdbSchemaGlobalData {
    fn as_brdb_struct_prop_array(
        &self,
        schema: &super::BrdbSchema,
        _struct_name: super::BrdbInterned,
        prop_name: super::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::brdb::errors::BrdbSchemaError> {
        Ok(match prop_name.get(schema).unwrap() {
            "EntityTypeNames" => self.entity_type_names.as_brdb_iter(),
            "BasicBrickAssetNames" => self.basic_brick_asset_names.as_brdb_iter(),
            "ProceduralBrickAssetNames" => self.procedural_brick_asset_names.as_brdb_iter(),
            "MaterialAssetNames" => self.material_asset_names.as_brdb_iter(),
            "ComponentTypeNames" => self.component_type_names.as_brdb_iter(),
            "ComponentDataStructNames" => self.component_data_struct_names.as_brdb_iter(),
            "ComponentWirePortNames" => self.component_wire_port_names.as_brdb_iter(),
            // BRSavedPrimaryAssetId is automatically inferred from (&str, &str)
            "ExternalAssetReferences" => self.external_asset_references.as_brdb_iter(),
            _ => unreachable!(),
        })
    }
}
