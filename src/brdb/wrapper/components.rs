use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use indexmap::IndexSet;

use crate::brdb::{
    errors::BrdbSchemaError,
    schema::{
        BrdbSchema, BrdbSchemaGlobalData, BrdbSchemaMeta,
        as_brdb::{AsBrdbIter, AsBrdbValue},
    },
    wrapper::{BString, Quat4f, Vector3f},
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

#[derive(Default)]
pub struct ComponentChunkSoA {
    pub component_type_counters: Vec<ComponentTypeCounter>,
    pub component_brick_indices: Vec<u32>,
    pub joint_brick_indices: Vec<u32>,
    pub joint_entity_references: Vec<u32>,
    pub joint_initial_relative_offsets: Vec<Vector3f>,
    pub joint_initial_relative_rotations: Vec<Quat4f>,

    // A copy of all components that need to be written.
    // The `BrdbComponent` trait is writable
    pub unwritten_struct_data: Vec<Box<dyn BrdbComponent>>,
}

impl ComponentChunkSoA {
    pub fn add_component(
        &mut self,
        global_data: &BrdbSchemaGlobalData,
        brick_index: u32,
        component: &dyn BrdbComponent,
    ) {
        let Some((component_ty_name, struct_ty)) = component.get_schema_struct() else {
            // Cannot add component without a type
            return;
        };
        // Unwrap safety: The component type was already added to the global data before
        // this function was called.
        let type_index = global_data
            .component_type_names
            .get_index_of(component_ty_name.as_ref())
            .unwrap() as u32;

        // Check if the last counter matches the type index
        if let Some(counter) = self.component_type_counters.last_mut() {
            if counter.type_index == type_index {
                counter.num_instances += 1;
            } else {
                // Add a new counter for this component type
                self.component_type_counters.push(ComponentTypeCounter {
                    type_index,
                    num_instances: 1,
                });
            }
        } else {
            // No counters yet, add the first one
            self.component_type_counters.push(ComponentTypeCounter {
                type_index,
                num_instances: 1,
            });
        }
        // Track the brick index for this component
        self.component_brick_indices.push(brick_index);

        // Clone the component data into unwritten_struct_data to be written later
        // Only if the component has a struct type
        if struct_ty.is_some() {
            self.unwritten_struct_data.push(component.boxed_component());
        }
    }
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
    fn get_external_asset_references(&self) -> IndexSet<(BString, BString)> {
        Default::default()
    }

    /// Emit the "ComponentTypeName" and "ComponentDataStructName" pair for this
    /// component
    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        None
    }

    /// Emit a list of wire ports this component uses
    fn get_wire_ports(&self) -> HashSet<BString> {
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

#[derive(Clone)]
pub struct LiteralComponent {
    pub component_name: BString,
    pub struct_name: Option<BString>,
    pub schema: Option<BrdbSchemaMeta>,
    pub data: Arc<HashMap<String, Box<dyn AsBrdbValue>>>,
    pub wire_ports: HashSet<BString>,
}

impl LiteralComponent {
    pub fn new_dataless(
        component_name: impl Into<BString>,
        struct_name: Option<impl Into<BString>>,
    ) -> Self {
        Self {
            component_name: component_name.into(),
            struct_name: struct_name.map(Into::into),
            schema: None,
            data: Default::default(),
            wire_ports: Default::default(),
        }
    }

    pub fn new(
        component_name: impl Into<BString>,
        struct_name: impl Into<BString>,
        schema: &str,
        data: impl IntoIterator<Item = (String, Box<dyn AsBrdbValue>)>,
        ports: impl IntoIterator<Item = BString>,
    ) -> Result<Self, BrdbSchemaError> {
        let schema =
            BrdbSchema::parse_to_meta(schema).map_err(|e| BrdbSchemaError::ParseError(e))?;

        Ok(Self {
            component_name: component_name.into(),
            struct_name: Some(struct_name.into()),
            schema: Some(schema),
            data: Arc::new(data.into_iter().collect()),
            wire_ports: ports.into_iter().collect(),
        })
    }
}

impl AsBrdbValue for LiteralComponent {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let prop_name_str = prop_name.get(schema).unwrap();
        match self.data.get(prop_name_str) {
            Some(value) => Ok(value.as_ref()),
            None => Err(BrdbSchemaError::MissingStructField(
                self.component_name.to_string(),
                prop_name_str.to_string(),
            )),
        }
    }
}

impl BrdbComponent for LiteralComponent {
    fn get_schema(&self) -> Option<BrdbSchemaMeta> {
        self.schema.clone()
    }

    fn get_external_asset_references(&self) -> IndexSet<(BString, BString)> {
        IndexSet::new()
    }

    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        Some((self.component_name.clone(), self.struct_name.clone()))
    }

    fn get_wire_ports(&self) -> HashSet<BString> {
        self.wire_ports.clone()
    }
}
