use std::fmt::Display;

use crate::{
    schema::as_brdb::{AsBrdbIter, AsBrdbValue, BrdbArrayIter},
    wrapper::{BString, BitFlags, ChunkIndex},
};

pub struct LocalWirePortSource {
    pub brick_index_in_chunk: u32,
    pub component_type_index: u16,
    pub port_index: u16,
}

impl AsBrdbValue for LocalWirePortSource {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
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
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
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
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "BrickIndexInChunk" => Ok(&self.brick_index_in_chunk),
            "ComponentTypeIndex" => Ok(&self.component_type_index),
            "PortIndex" => Ok(&self.port_index),
            _ => unreachable!(),
        }
    }
}

#[derive(Default)]
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
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "PendingPropagationFlags" => Ok(&self.pending_propagation_flags),
            _ => unreachable!(),
        }
    }
    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "RemoteWireSources" => Ok(self.remote_wire_sources.as_brdb_iter()),
            "LocalWireSources" => Ok(self.local_wire_sources.as_brdb_iter()),
            "RemoteWireTargets" => Ok(self.remote_wire_targets.as_brdb_iter()),
            "LocalWireTargets" => Ok(self.local_wire_targets.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}

impl WireChunkSoA {
    pub fn add_local_wire(&mut self, source: LocalWirePortSource, target: WirePortTarget) {
        self.local_wire_sources.push(source);
        self.local_wire_targets.push(target);
    }

    pub fn add_remote_wire(&mut self, source: RemoteWirePortSource, target: WirePortTarget) {
        self.remote_wire_sources.push(source);
        self.remote_wire_targets.push(target);
    }
}

#[derive(Debug, Clone)]
pub struct WireConnection {
    pub source: WirePort,
    pub target: WirePort,
}

impl Display for WireConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} to {}", self.source, self.target)
    }
}

impl WireConnection {
    pub fn new(source: WirePort, target: WirePort) -> Self {
        Self { source, target }
    }
}

#[derive(Debug, Clone)]
pub struct WirePort {
    /// The remote brick where the port is located
    pub brick_id: usize,
    /// Name of the component in the brick to connect
    pub component_type: BString,
    /// Name of the port in the component to connect
    pub port_name: BString,
}

impl WirePort {
    pub fn new(
        brick_id: usize,
        component_type: impl Into<BString>,
        port_name: impl Into<BString>,
    ) -> Self {
        Self {
            brick_id,
            component_type: component_type.into(),
            port_name: port_name.into(),
        }
    }
}

impl Display for WirePort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "brick {} {}.{}",
            self.brick_id, self.component_type, self.port_name
        )
    }
}
