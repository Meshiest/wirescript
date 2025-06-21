use crate::brdb::{
    schema::as_brdb::{AsBrdbValue, BrdbArrayIter, AsBrdbIter},
    wrapper::{BitFlags, ChunkIndex},
};

pub struct LocalWirePortSource {
    pub brick_index_in_chunk: u32,
    pub component_type_index: u16,
    pub port_index: u16,
}

impl AsBrdbValue for LocalWirePortSource {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "BrickIndexInChunk" => Ok(&self.brick_index_in_chunk),
            "ComponentTypeIndex" => Ok(&self.component_type_index),
            "PortIndex" => Ok(&self.port_index),
            _ => unreachable!(),
        }
    }
}

pub struct RemoteWirePortSource {
    pub grid_persistent_index: u32,
    pub chunk_index: ChunkIndex,
    pub brick_index_in_chunk: u32,
    pub component_type_index: u16,
    pub port_index: u16,
}
impl AsBrdbValue for RemoteWirePortSource {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "GridPersistentIndex" => Ok(&self.grid_persistent_index),
            "ChunkIndex" => Ok(&self.chunk_index),
            "BrickIndexInChunk" => Ok(&self.brick_index_in_chunk),
            "ComponentTypeIndex" => Ok(&self.component_type_index),
            "PortIndex" => Ok(&self.port_index),
            _ => unreachable!(),
        }
    }
}
pub struct WirePortTarget {
    pub brick_index_in_chunk: u32,
    pub component_type_index: u16,
    pub port_index: u16,
}
impl AsBrdbValue for WirePortTarget {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "BrickIndexInChunk" => Ok(&self.brick_index_in_chunk),
            "ComponentTypeIndex" => Ok(&self.component_type_index),
            "PortIndex" => Ok(&self.port_index),
            _ => unreachable!(),
        }
    }
}
pub struct WireChunkSoA {
    pub remote_wire_sources: Vec<RemoteWirePortSource>,
    pub local_wire_sources: Vec<LocalWirePortSource>,
    pub remote_wire_targets: Vec<WirePortTarget>,
    pub local_wire_targets: Vec<WirePortTarget>,
    pub pending_propagation_flags: BitFlags,
}
impl AsBrdbValue for WireChunkSoA {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "PendingPropagationFlags" => Ok(&self.pending_propagation_flags),
            _ => unreachable!(),
        }
    }
    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "RemoteWireSources" => Ok(self.remote_wire_sources.as_brdb_iter()),
            "LocalWireSources" => Ok(self.local_wire_sources.as_brdb_iter()),
            "RemoteWireTargets" => Ok(self.remote_wire_targets.as_brdb_iter()),
            "LocalWireTargets" => Ok(self.local_wire_targets.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}
