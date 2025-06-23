use std::collections::HashSet;

use indexmap::IndexSet;

use crate::brdb::{
    schema::{
        BrdbSchemaMeta,
        as_brdb::{AsBrdbIter, AsBrdbValue},
    },
    wrapper::{Quat4f, Vector3f},
};

pub struct ComponentTypeCounter {
    pub type_index: u32,
    pub num_instances: u32,
}

impl AsBrdbValue for ComponentTypeCounter {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "TypeIndex" => Ok(&self.type_index),
            "NumInstances" => Ok(&self.num_instances),
            _ => unreachable!(),
        }
    }
}

pub struct ComponentChunkSoA {
    pub component_type_counters: Vec<ComponentTypeCounter>,
    pub component_brick_indices: Vec<u32>,
    pub joint_brick_indices: Vec<u32>,
    pub joint_entity_references: Vec<u32>,
    pub joint_initial_relative_offsets: Vec<Vector3f>,
    pub joint_initial_relative_rotations: Vec<Quat4f>,
}

impl AsBrdbValue for ComponentChunkSoA {
    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<crate::brdb::schema::as_brdb::BrdbArrayIter, crate::brdb::errors::BrdbSchemaError>
    {
        Ok(match prop_name.get(schema).unwrap() {
            "ComponentTypeCounters" => self.component_type_counters.as_brdb_iter(),
            "ComponentBrickIndices" => self.component_brick_indices.as_brdb_iter(),
            "JointBrickIndices" => self.joint_brick_indices.as_brdb_iter(),
            "JointEntityReferences" => self.joint_entity_references.as_brdb_iter(),
            "JointInitialRelativeOffsets" => self.joint_initial_relative_offsets.as_brdb_iter(),
            "JointInitialRelativeRotations" => self.joint_initial_relative_rotations.as_brdb_iter(),
            _ => unreachable!(),
        })
    }
}

/// This trait allows BrdbComponents to be cloned
/// despite being a dyn trait
pub trait BoxedComponent {
    fn boxed_component(&self) -> Box<dyn BrdbComponent>;
}

pub trait BrdbComponent: AsBrdbValue + BoxedComponent {
    /// Emit the structs needed to use this component in a world
    fn get_schema(&self) -> Option<BrdbSchemaMeta> {
        None
    }

    /// Emit asset references this component uses
    fn get_external_asset_references(&self) -> IndexSet<(String, String)> {
        Default::default()
    }

    /// Emit the "ComponentTypeName" and "ComponentDataStructName" pair for this
    /// component
    fn get_schema_struct(&self) -> Option<(String, Option<String>)> {
        None
    }

    /// Emit a list of wire ports this component uses
    fn get_wire_ports(&self) -> HashSet<String> {
        Default::default()
    }
}

/// Blanket implement boxed for all BrdbComponents with Clone
/// ... enabling them to be cloned
impl<T: Clone + BrdbComponent + 'static> BoxedComponent for T {
    fn boxed_component(&self) -> Box<dyn BrdbComponent> {
        Box::new(self.clone())
    }
}
