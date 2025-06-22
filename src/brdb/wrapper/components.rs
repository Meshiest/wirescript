use crate::brdb::{
    schema::as_brdb::AsBrdbValue,
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
