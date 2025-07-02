use std::collections::HashSet;

use indexmap::IndexSet;
use serde::{Deserialize, Serialize};

use crate::{
    schema::as_brdb::{AsBrdbIter, AsBrdbValue, BrdbArrayIter},
    wrapper::{BrdbComponent, Brick, BrickType},
};

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct BrdbSchemaGlobalData {
    pub entity_type_names: IndexSet<String>,
    pub basic_brick_asset_names: IndexSet<String>,
    pub procedural_brick_asset_names: IndexSet<String>,
    pub material_asset_names: IndexSet<String>,
    pub component_type_names: IndexSet<String>,
    pub component_data_struct_names: Vec<String>,
    pub component_wire_port_names: IndexSet<String>,
    /// Internal set for type checking, not used in the BRDB.
    pub external_asset_types: HashSet<String>,
    pub external_asset_references: IndexSet<(String, String)>,
}

impl BrdbSchemaGlobalData {
    pub fn add_brick_meta(&mut self, brick: &Brick) {
        // Add material
        if !self.material_asset_names.contains(brick.material.as_ref()) {
            self.material_asset_names.insert(brick.material.to_string());
        }

        // Add brick assets
        match &brick.asset {
            BrickType::Basic(asset) if !self.basic_brick_asset_names.contains(asset.as_ref()) => {
                self.basic_brick_asset_names.insert(asset.to_string());
            }
            BrickType::Procedural { asset, .. }
                if !self.procedural_brick_asset_names.contains(asset.as_ref()) =>
            {
                self.procedural_brick_asset_names.insert(asset.to_string());
            }
            // Material and asset are already handled above
            _ => {}
        }
    }

    pub fn add_component_meta(&mut self, component: &dyn BrdbComponent) {
        for (asset_ty, asset_name) in component.get_external_asset_references() {
            self.external_asset_references
                .insert((asset_ty.to_string(), asset_name.to_string()));
            self.external_asset_types.insert(asset_ty.to_string());
        }

        // Add the struct names for components
        if let Some((type_name, struct_name)) = component.get_schema_struct() {
            if self.component_type_names.contains(type_name.as_ref()) {
                return;
            }

            self.component_type_names.insert(type_name.to_string());
            self.component_data_struct_names.push(
                struct_name
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "None".to_owned()),
            );
        }

        // Add the wire port names
        self.component_wire_port_names.extend(
            component
                .get_wire_ports()
                .into_iter()
                .map(|p| p.to_string()),
        );
    }

    pub fn get_port_index(&self, port_name: &str) -> Option<u16> {
        self.component_wire_port_names
            .get_index_of(port_name)
            .map(|i| i as u16)
    }

    pub fn get_component_type_index(&self, type_name: &str) -> Option<u16> {
        self.component_type_names
            .get_index_of(type_name)
            .map(|i| i as u16)
    }
    pub fn has_component_type(&self, type_name: &str) -> bool {
        self.component_type_names.contains(type_name)
    }
    pub fn add_entity_type(&mut self, type_name: &str) {
        self.entity_type_names.insert(type_name.to_string());
    }
}

impl AsBrdbValue for BrdbSchemaGlobalData {
    fn as_brdb_struct_prop_array(
        &self,
        schema: &super::BrdbSchema,
        _struct_name: super::BrdbInterned,
        prop_name: super::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::errors::BrdbSchemaError> {
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
