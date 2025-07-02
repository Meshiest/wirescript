use std::{
    fmt::Display,
    io::{Read, Write},
    sync::Arc,
};

use indexmap::IndexMap;
use lalrpop_util::lalrpop_mod;
use rmp::{Marker, decode::RmpRead};

mod global_data;
mod intern;
pub mod read;
mod value;
pub mod write;
pub use global_data::*;
pub use intern::*;
pub use value::*;
pub mod as_brdb;

pub(crate) type PlaintextEnumBody = Vec<(String, i32)>;
pub(crate) type PlaintextStructBody = Vec<(String, BrdbStructPropRaw)>;

lalrpop_mod!(plaintext);

use crate::{
    errors::BrdbSchemaError,
    schema::{
        as_brdb::AsBrdbValue,
        read::{read_owned_str, read_str_from_len},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrdbSchemaStructProperty {
    Type(BrdbInterned),
    Array(BrdbInterned),
    FlatArray(BrdbInterned),
    Map(BrdbInterned, BrdbInterned),
}
pub type BrdbSchemaStruct = IndexMap<BrdbInterned, BrdbSchemaStructProperty>;
pub type BrdbSchemaEnum = IndexMap<BrdbInterned, i32>;

pub type BrdbSchemaMetaEnum = (String, Vec<(String, i32)>);
pub type BrdbSchemaMetaStruct = (String, Vec<(String, BrdbStructPropRaw)>);
pub type BrdbSchemaMeta = (Vec<BrdbSchemaMetaEnum>, Vec<BrdbSchemaMetaStruct>);

impl BrdbSchemaStructProperty {
    pub fn as_string(&self, schema: &BrdbSchema) -> String {
        match self {
            BrdbSchemaStructProperty::Type(t) => {
                schema.intern.lookup(*t).unwrap_or("UnknownType".to_owned())
            }
            BrdbSchemaStructProperty::Array(t) => {
                format!(
                    "{}[]",
                    schema
                        .intern
                        .lookup(*t)
                        .unwrap_or("UnknownArrayType".to_owned())
                )
            }
            BrdbSchemaStructProperty::FlatArray(t) => format!(
                "{}[flat]",
                schema
                    .intern
                    .lookup(*t)
                    .unwrap_or("UnknownFlatArrayType".to_owned())
            ),
            BrdbSchemaStructProperty::Map(k, v) => {
                let key = schema
                    .intern
                    .lookup(*k)
                    .unwrap_or("UnknownMapKeyType".to_owned());
                let value = schema
                    .intern
                    .lookup(*v)
                    .unwrap_or("UnknownMapValueType".to_owned());
                format!("{{{key}: {value}}}")
            }
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct BrdbSchema {
    pub intern: BrdbIntern,
    pub(crate) global_data: Arc<BrdbSchemaGlobalData>,
    pub enums: IndexMap<BrdbInterned, BrdbSchemaEnum>,
    pub structs: IndexMap<BrdbInterned, BrdbSchemaStruct>,
}

impl Display for BrdbSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, values) in &self.enums {
            let name = self
                .intern
                .lookup(*name)
                .unwrap_or("UnknownEnum".to_owned());
            writeln!(f, "enum {name} {{")?;
            for (key, value) in values {
                let key = self.intern.lookup(*key).unwrap_or("UnknownKey".to_owned());
                writeln!(f, "    {key} = {value},")?;
            }
            writeln!(f, "}}")?;
        }
        for (name, properties) in &self.structs {
            let name = self
                .intern
                .lookup(*name)
                .unwrap_or("UnknownStruct".to_owned());
            writeln!(f, "struct {name} {{")?;
            for (prop_name, prop_type) in properties {
                let prop_name = self
                    .intern
                    .lookup(*prop_name)
                    .unwrap_or("UnknownProperty".to_owned());
                writeln!(f, "    {prop_name}: {},", prop_type.as_string(self))?;
            }
            writeln!(f, "}}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrdbStructPropRaw {
    Type(String),
    Array(String),
    FlatArray(String),
    Map(String, String),
}

impl BrdbSchema {
    pub fn get_struct(&self, name: &str) -> Option<&BrdbSchemaStruct> {
        self.structs.get(&self.intern.get(name)?)
    }

    pub fn get_struct_interned(&self, id: BrdbInterned) -> Option<&BrdbSchemaStruct> {
        self.structs.get(&id)
    }

    pub fn get_enum(&self, name: &str) -> Option<&BrdbSchemaEnum> {
        self.enums.get(&self.intern.get(name)?)
    }

    pub fn get_enum_interned(&self, id: BrdbInterned) -> Option<&BrdbSchemaEnum> {
        self.enums.get(&id)
    }

    pub fn parse_to_meta(input: &str) -> Result<BrdbSchemaMeta, String> {
        plaintext::MetaParser::new()
            .parse(input)
            .map_err(|e| e.to_string())
    }

    /// Parse a schema from a plaintext input string into a `BrdbSchema`
    pub fn new_parsed(input: &str) -> Result<BrdbSchema, BrdbSchemaError> {
        let (enums, structs) = Self::parse_to_meta(input).map_err(BrdbSchemaError::ParseError)?;
        let mut schema = BrdbSchema::default();
        schema.add_meta(enums, structs);
        Ok(schema)
    }

    /// Read a schema from a msgpack .schema file into a human readable format
    pub fn read_to_meta(mut buf: impl Read) -> Result<BrdbSchemaMeta, BrdbSchemaError> {
        let header = rmp::decode::read_array_len(&mut buf)?;
        if header != 2 {
            return Err(BrdbSchemaError::InvalidHeader(header));
        }

        // Read enums
        let mut enums = vec![];
        let num_enums = rmp::decode::read_map_len(&mut buf)? as usize;
        for _ in 0..num_enums {
            let enum_name = read_owned_str(&mut buf)?;
            let value_count = rmp::decode::read_map_len(&mut buf)? as usize;
            let mut values = Vec::with_capacity(value_count as usize);
            for _ in 0..value_count {
                let key = read_owned_str(&mut buf)?;
                let value = read::read_int(&mut buf)? as i32;
                values.push((key, value));
            }
            enums.push((enum_name, values));
        }

        // Read structs
        let mut structs = vec![];
        let num_structs = rmp::decode::read_map_len(&mut buf)? as usize;
        for _ in 0..num_structs {
            let struct_name = read_owned_str(&mut buf)?;

            let num_props = rmp::decode::read_map_len(&mut buf)? as usize;
            let mut properties = Vec::with_capacity(num_props);
            for _ in 0..num_props {
                let prop_name = read_owned_str(&mut buf)?;
                let prop_type_marker = rmp::decode::read_marker(&mut buf)
                    .map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
                let property = match prop_type_marker {
                    // Basic types
                    Marker::FixStr(size) => {
                        BrdbStructPropRaw::Type(read_str_from_len(&mut buf, size as usize)?)
                    }
                    Marker::Str8 => {
                        let len = buf.read_data_u8()? as usize;
                        BrdbStructPropRaw::Type(read_str_from_len(&mut buf, len)?)
                    }
                    Marker::Str16 => {
                        let len = buf.read_data_u16()? as usize;
                        BrdbStructPropRaw::Type(read_str_from_len(&mut buf, len)?)
                    }

                    Marker::FixArray(len) if len == 0 => {
                        return Err(BrdbSchemaError::InvalidSchema(
                            "0 length FixArray marker not supported".to_string(),
                        ));
                    }
                    // Array type
                    Marker::FixArray(len) if len == 1 => {
                        let array_type = read_owned_str(&mut buf)?;
                        BrdbStructPropRaw::Array(array_type)
                    }
                    // Flat array has a specific format: [type, nil]
                    Marker::FixArray(len) if len == 2 => {
                        let array_type = read_owned_str(&mut buf)?;
                        // Ensure the second element is nil
                        rmp::decode::read_nil(&mut buf)
                            .map_err(|e| BrdbSchemaError::RmpValueReadError(e))?;
                        BrdbStructPropRaw::FlatArray(array_type)
                    }

                    Marker::FixMap(len) if len != 1 => {
                        return Err(BrdbSchemaError::InvalidSchema(
                            "FixMap with length != 1 is not supported".to_string(),
                        ));
                    }
                    Marker::FixMap(len) if len == 1 => {
                        let key_type = read_owned_str(&mut buf)?;
                        let value_type = read_owned_str(&mut buf)?;
                        BrdbStructPropRaw::Map(key_type, value_type)
                    }
                    marker => {
                        return Err(BrdbSchemaError::InvalidSchema(format!(
                            "Unsupported property type marker: {marker:?}",
                        )));
                    }
                };

                properties.push((prop_name, property));
            }
            structs.push((struct_name, properties));
        }

        Ok((enums, structs))
    }

    pub fn from_meta(
        enums: impl IntoIterator<Item = (String, Vec<(String, i32)>)>,
        structs: impl IntoIterator<Item = (String, Vec<(String, BrdbStructPropRaw)>)>,
    ) -> Self {
        let mut schema = BrdbSchema::default();
        schema.add_meta(enums, structs);
        schema
    }

    /// Bulk insert enums and structs from metadata into this schema
    pub fn add_meta(
        &mut self,
        enums: impl IntoIterator<Item = (String, Vec<(String, i32)>)>,
        structs: impl IntoIterator<Item = (String, Vec<(String, BrdbStructPropRaw)>)>,
    ) {
        for (name, values) in enums {
            self.add_enum(name, values);
        }
        for (name, props) in structs {
            self.add_struct(name, props);
        }
    }

    /// Read from a msgpack .schema buffer into a populated `BrdbSchema` struct
    pub fn read(buf: impl Read) -> Result<BrdbSchema, BrdbSchemaError> {
        let (enums, structs) = Self::read_to_meta(buf)?;
        let mut schema = BrdbSchema::default();
        schema.add_meta(enums, structs);
        Ok(schema)
    }

    /// Add a single enum to the schema
    pub fn add_enum(&mut self, name: String, values: Vec<(String, i32)>) {
        let name = self.intern.get_or_insert(name);
        let values = values
            .into_iter()
            .map(|(k, v)| (self.intern.get_or_insert(k), v))
            .collect::<BrdbSchemaEnum>();
        self.enums.insert(name, values);
    }

    /// Add a single struct to the schema
    pub fn add_struct(&mut self, name: String, props: Vec<(String, BrdbStructPropRaw)>) {
        let name = self.intern.get_or_insert(name);
        let props = props
            .into_iter()
            .map(|(prop, prop_ty)| {
                (
                    self.intern.get_or_insert(prop),
                    match prop_ty {
                        BrdbStructPropRaw::Type(ty) => {
                            BrdbSchemaStructProperty::Type(self.intern.get_or_insert(ty))
                        }
                        BrdbStructPropRaw::Array(ty) => {
                            BrdbSchemaStructProperty::Array(self.intern.get_or_insert(ty))
                        }
                        BrdbStructPropRaw::FlatArray(ty) => {
                            BrdbSchemaStructProperty::FlatArray(self.intern.get_or_insert(ty))
                        }
                        BrdbStructPropRaw::Map(k_ty, v_ty) => BrdbSchemaStructProperty::Map(
                            self.intern.get_or_insert(k_ty),
                            self.intern.get_or_insert(v_ty),
                        ),
                    },
                )
            })
            .collect::<BrdbSchemaStruct>();
        self.structs.insert(name, props);
    }

    /// Attach global data to the schema
    pub fn with_global_data(mut self, global_data: Arc<BrdbSchemaGlobalData>) -> Self {
        self.global_data = global_data;
        self
    }

    /// Attach global data to the schema
    pub fn set_global_data(&mut self, global_data: Arc<BrdbSchemaGlobalData>) {
        self.global_data = global_data;
    }

    /// Serialize the schema as msgpack
    pub fn write(&self, mut buf: impl Write) -> Result<(), BrdbSchemaError> {
        rmp::encode::write_array_len(&mut buf, 2)?;

        let lookup = |interned: BrdbInterned| {
            interned
                .get(self)
                .ok_or(BrdbSchemaError::StringNotInterned(interned.0))
        };

        rmp::encode::write_map_len(&mut buf, self.enums.len() as u32)?;
        for (enum_name, values) in &self.enums {
            rmp::encode::write_str(&mut buf, lookup(*enum_name)?)?;
            rmp::encode::write_map_len(&mut buf, values.len() as u32)?;
            for (key, value) in values {
                rmp::encode::write_str(&mut buf, lookup(*key)?)?;
                write::write_int(&mut buf, *value as i64)?;
            }
        }

        rmp::encode::write_map_len(&mut buf, self.structs.len() as u32)?;
        for (struct_name, properties) in &self.structs {
            rmp::encode::write_str(&mut buf, lookup(*struct_name)?)?;
            rmp::encode::write_map_len(&mut buf, properties.len() as u32)?;
            for (prop_name, prop_type) in properties {
                rmp::encode::write_str(&mut buf, lookup(*prop_name)?)?;
                match prop_type {
                    BrdbSchemaStructProperty::Type(t) => {
                        rmp::encode::write_str(&mut buf, lookup(*t)?)?
                    }
                    BrdbSchemaStructProperty::Array(t) => {
                        rmp::encode::write_array_len(&mut buf, 1)?;
                        rmp::encode::write_str(&mut buf, lookup(*t)?)?;
                    }
                    BrdbSchemaStructProperty::FlatArray(t) => {
                        rmp::encode::write_array_len(&mut buf, 2)?;
                        rmp::encode::write_str(&mut buf, lookup(*t)?)?;
                        rmp::encode::write_nil(&mut buf)?;
                    }
                    BrdbSchemaStructProperty::Map(key_type, value_type) => {
                        rmp::encode::write_map_len(&mut buf, 1)?;
                        rmp::encode::write_str(&mut buf, lookup(*key_type)?)?;
                        rmp::encode::write_str(&mut buf, lookup(*value_type)?)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Serialize the schema to a vector of bytes
    pub fn to_vec(&self) -> Result<Vec<u8>, BrdbSchemaError> {
        let mut buf = Vec::new();
        self.write(&mut buf)?;
        Ok(buf)
    }

    pub fn write_brdb(
        &self,
        ty: &str,
        value: &impl AsBrdbValue,
    ) -> Result<Vec<u8>, BrdbSchemaError> {
        let mut buf = Vec::new();
        write::write_brdb(self, &mut buf, ty, value)?;
        Ok(buf)
    }
}

pub trait ReadBrdbSchema {
    fn read_brdb_schema(&mut self) -> Result<Arc<BrdbSchema>, BrdbSchemaError>;
    fn read_brdb_schema_with_data(
        &mut self,
        data: Arc<BrdbSchemaGlobalData>,
    ) -> Result<Arc<BrdbSchema>, BrdbSchemaError>;
    fn read_brdb(
        &mut self,
        schema: &Arc<BrdbSchema>,
        ty: &str,
    ) -> Result<BrdbValue, BrdbSchemaError>;
}

impl<R: Read> ReadBrdbSchema for R {
    fn read_brdb_schema(&mut self) -> Result<Arc<BrdbSchema>, BrdbSchemaError> {
        BrdbSchema::read(self).map(Arc::new)
    }
    fn read_brdb_schema_with_data(
        &mut self,
        data: Arc<BrdbSchemaGlobalData>,
    ) -> Result<Arc<BrdbSchema>, BrdbSchemaError> {
        BrdbSchema::read(self).map(|schema| Arc::new(schema.with_global_data(data)))
    }
    fn read_brdb(
        &mut self,
        schema: &Arc<BrdbSchema>,
        ty: &str,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        read::read_type(schema, ty, self)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_plaintext() {
        let input = "enum Foo {
    A = 0,
    B = 1,
}
struct Bar {
    value: i32,
    arr: str[],
    flat_arr: str[flat],
    map: {str: i32},
}
";
        let (enums, structs) = super::BrdbSchema::parse_to_meta(input).unwrap();

        // When inserting all the enums and structs into a schema it should
        // produce the same displayed output as the input
        let mut schema = super::BrdbSchema::default();
        schema.add_meta(enums.clone(), structs.clone());
        assert_eq!(schema.to_string(), input,);
    }
}
