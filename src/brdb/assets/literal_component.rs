use std::{collections::HashMap, sync::Arc};

use crate::brdb::{
    errors::BrdbSchemaError,
    schema::{BrdbSchema, BrdbSchemaMeta, as_brdb::AsBrdbValue},
    wrapper::{BString, BrdbComponent},
};

#[derive(Clone)]
pub struct LiteralComponent {
    pub component_name: BString,
    pub struct_name: Option<BString>,
    pub schema: Option<BrdbSchemaMeta>,
    pub data: Arc<HashMap<String, Box<dyn AsBrdbValue>>>,
    pub wire_ports: Vec<BString>,
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

    fn get_external_asset_references(&self) -> Vec<(BString, BString)> {
        Vec::new()
    }

    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        Some((self.component_name.clone(), self.struct_name.clone()))
    }

    fn get_wire_ports(&self) -> Vec<BString> {
        self.wire_ports.clone()
    }
}
