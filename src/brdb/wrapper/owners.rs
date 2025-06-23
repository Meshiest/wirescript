use crate::brdb::schema::as_brdb::{AsBrdbIter, AsBrdbValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}
impl Default for Guid {
    fn default() -> Self {
        Self {
            a: u32::MAX,
            b: u32::MAX,
            c: u32::MAX,
            d: u32::MAX,
        }
    }
}

impl AsBrdbValue for Guid {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "A" => Ok(&self.a),
            "B" => Ok(&self.b),
            "C" => Ok(&self.c),
            "D" => Ok(&self.d),
            _ => unreachable!(),
        }
    }
}

#[derive(Default)]
pub struct Owner {
    pub user_id: Guid,
    pub user_name: String,
    pub display_name: String,
}

#[derive(Default)]
pub struct OwnerTableSoA {
    pub user_ids: Vec<Guid>,
    pub user_names: Vec<String>,
    pub display_names: Vec<String>,
    pub entity_counts: Vec<u32>,
    pub brick_counts: Vec<u32>,
    pub component_counts: Vec<u32>,
    pub wire_counts: Vec<u32>,
}

impl AsBrdbValue for OwnerTableSoA {
    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<crate::brdb::schema::as_brdb::BrdbArrayIter, crate::brdb::errors::BrdbSchemaError>
    {
        match prop_name.get(schema).unwrap() {
            "UserIds" => Ok(self.user_ids.as_brdb_iter()),
            "UserNames" => Ok(self.user_names.as_brdb_iter()),
            "DisplayNames" => Ok(self.display_names.as_brdb_iter()),
            "EntityCounts" => Ok(self.entity_counts.as_brdb_iter()),
            "BrickCounts" => Ok(self.brick_counts.as_brdb_iter()),
            "ComponentCounts" => Ok(self.component_counts.as_brdb_iter()),
            "WireCounts" => Ok(self.wire_counts.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}
