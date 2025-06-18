use std::{
    collections::HashMap,
    fmt::Display,
    io::{Read, Write},
};

use indexmap::IndexMap;
use rmp::{Marker, decode::RmpRead};

mod intern;
mod value;

use crate::brdb::{
    errors::BrdbSchemaError,
    schema::{
        intern::{BrdbIntern, BrdbInterned},
        value::{read_owned_str, read_str_from_len},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrdbAssetType {
    Class,
    Object,
}

pub type SchemaGlobalData = HashMap<(BrdbAssetType, String), u32>;

pub enum BrdbSchemaStructProperty {
    Type(BrdbInterned),
    Array(BrdbInterned),
    FlatArray(BrdbInterned),
    Map(BrdbInterned, BrdbInterned),
}
pub type BrdbSchemaStruct = IndexMap<BrdbInterned, BrdbSchemaStructProperty>;
pub type BrdbSchemaEnum = IndexMap<BrdbInterned, i32>;

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

pub struct BrdbSchema {
    intern: BrdbIntern,
    pub enums: IndexMap<BrdbInterned, BrdbSchemaEnum>,
    pub structs: IndexMap<BrdbInterned, BrdbSchemaStruct>,
}

impl Display for BrdbSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BrdbSchema {{\n")?;
        for (name, values) in &self.enums {
            let name = self
                .intern
                .lookup(*name)
                .unwrap_or("UnknownEnum".to_owned());
            write!(f, "  Enum {name} {{\n")?;
            for (key, value) in values {
                let key = self.intern.lookup(*key).unwrap_or("UnknownKey".to_owned());
                write!(f, "    {key}: {value}\n")?;
            }
            write!(f, "  }}\n")?;
        }
        for (name, properties) in &self.structs {
            let name = self
                .intern
                .lookup(*name)
                .unwrap_or("UnknownStruct".to_owned());
            write!(f, "  Struct {name} {{\n")?;
            for (prop_name, prop_type) in properties {
                let prop_name = self
                    .intern
                    .lookup(*prop_name)
                    .unwrap_or("UnknownProperty".to_owned());
                write!(f, "    {prop_name}: {}\n", prop_type.as_string(self))?;
            }
            write!(f, "  }}\n")?;
        }
        write!(f, "}}")
    }
}

impl BrdbSchema {
    pub fn get_struct(&self, name: &str) -> Option<&BrdbSchemaStruct> {
        self.structs.get(&self.intern.get(name)?)
    }

    pub fn get_enum(&self, name: &str) -> Option<&BrdbSchemaEnum> {
        self.enums.get(&self.intern.get(name)?)
    }

    pub fn read(mut buf: impl Read) -> Result<BrdbSchema, BrdbSchemaError> {
        let header = rmp::decode::read_array_len(&mut buf)?;
        if header != 2 {
            return Err(BrdbSchemaError::InvalidHeader(header));
        }

        let intern = BrdbIntern::default();

        // Read enums
        let num_enums = rmp::decode::read_map_len(&mut buf)? as usize;
        let mut enums = IndexMap::with_capacity(num_enums);
        for _ in 0..num_enums {
            let enum_name = read_owned_str(&mut buf)?;
            let value_count = rmp::decode::read_map_len(&mut buf)? as usize;
            let mut values = BrdbSchemaEnum::with_capacity(value_count as usize);
            for _ in 0..value_count {
                let key = read_owned_str(&mut buf)?;
                let value = rmp::decode::read_i32(&mut buf)?;
                values.insert(intern.get_or_insert(key), value);
            }
            enums.insert(intern.get_or_insert(enum_name), values);
        }

        // Read structs
        let num_structs = rmp::decode::read_map_len(&mut buf)? as usize;
        let mut structs = IndexMap::with_capacity(num_structs);
        for _ in 0..num_structs {
            let struct_name = read_owned_str(&mut buf)?;

            let num_props = rmp::decode::read_map_len(&mut buf)? as usize;
            let mut properties = BrdbSchemaStruct::with_capacity(num_props);
            for _ in 0..num_props {
                let prop_name = intern.get_or_insert(read_owned_str(&mut buf)?);
                let prop_type_marker = rmp::decode::read_marker(&mut buf)
                    .map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
                let property = match prop_type_marker {
                    // Basic types
                    Marker::FixStr(size) => BrdbSchemaStructProperty::Type(
                        intern.get_or_insert(read_str_from_len(&mut buf, size as usize)?),
                    ),
                    Marker::Str8 => {
                        let len = buf.read_data_u8()? as usize;
                        BrdbSchemaStructProperty::Type(
                            intern.get_or_insert(read_str_from_len(&mut buf, len)?),
                        )
                    }
                    Marker::Str16 => {
                        let len = buf.read_data_u16()? as usize;
                        BrdbSchemaStructProperty::Type(
                            intern.get_or_insert(read_str_from_len(&mut buf, len)?),
                        )
                    }

                    Marker::FixArray(len) if len == 0 => {
                        return Err(BrdbSchemaError::InvalidSchema(
                            "0 length FixArray marker not supported".to_string(),
                        ));
                    }
                    // Array type
                    Marker::FixArray(len) if len == 1 => {
                        let array_type = read_owned_str(&mut buf)?;
                        BrdbSchemaStructProperty::Array(intern.get_or_insert(array_type))
                    }
                    // Flat array has a specific format: [type, nil]
                    Marker::FixArray(len) if len == 2 => {
                        let array_type = read_owned_str(&mut buf)?;
                        // Ensure the second element is nil
                        rmp::decode::read_nil(&mut buf)
                            .map_err(|e| BrdbSchemaError::RmpValueReadError(e))?;

                        BrdbSchemaStructProperty::FlatArray(intern.get_or_insert(array_type))
                    }
                    Marker::FixMap(len) if len != 1 => {
                        return Err(BrdbSchemaError::InvalidSchema(
                            "FixMap with length != 1 is not supported".to_string(),
                        ));
                    }
                    Marker::FixMap(len) if len == 1 => {
                        let key_type = intern.get_or_insert(read_owned_str(&mut buf)?);
                        let value_type = intern.get_or_insert(read_owned_str(&mut buf)?);
                        BrdbSchemaStructProperty::Map(key_type, value_type)
                    }
                    marker => {
                        return Err(BrdbSchemaError::InvalidSchema(format!(
                            "Unsupported property type marker: {marker:?}",
                        )));
                    }
                };

                properties.insert(prop_name, property);
            }
            structs.insert(intern.get_or_insert(struct_name), properties);
        }

        Ok(BrdbSchema {
            intern,
            enums,
            structs,
        })
    }

    pub fn write(&self, mut buf: impl Write) -> Result<(), BrdbSchemaError> {
        rmp::encode::write_array_len(&mut buf, 2)?;

        let lookup = |interned: BrdbInterned| {
            self.intern
                .lookup_ref(interned)
                .ok_or(BrdbSchemaError::StringNotInterned(interned.0))
        };

        rmp::encode::write_map_len(&mut buf, self.enums.len() as u32)?;
        for (enum_name, values) in &self.enums {
            rmp::encode::write_str(&mut buf, &lookup(*enum_name)?)?;
            rmp::encode::write_map_len(&mut buf, values.len() as u32)?;
            for (key, value) in values {
                rmp::encode::write_str(&mut buf, &lookup(*key)?)?;
                rmp::encode::write_i32(&mut buf, *value)?;
            }
        }

        rmp::encode::write_map_len(&mut buf, self.structs.len() as u32)?;
        for (struct_name, properties) in &self.structs {
            rmp::encode::write_str(&mut buf, &lookup(*struct_name)?)?;
            rmp::encode::write_map_len(&mut buf, properties.len() as u32)?;
            for (prop_name, prop_type) in properties {
                rmp::encode::write_str(&mut buf, &lookup(*prop_name)?)?;
                match prop_type {
                    BrdbSchemaStructProperty::Type(t) => {
                        rmp::encode::write_str(&mut buf, &lookup(*t)?)?
                    }
                    BrdbSchemaStructProperty::Array(t) => {
                        rmp::encode::write_array_len(&mut buf, 1)?;
                        rmp::encode::write_str(&mut buf, &lookup(*t)?)?;
                    }
                    BrdbSchemaStructProperty::FlatArray(t) => {
                        rmp::encode::write_array_len(&mut buf, 2)?;
                        rmp::encode::write_str(&mut buf, &lookup(*t)?)?;
                        rmp::encode::write_nil(&mut buf)?;
                    }
                    BrdbSchemaStructProperty::Map(key_type, value_type) => {
                        rmp::encode::write_map_len(&mut buf, 1)?;
                        rmp::encode::write_str(&mut buf, &lookup(*key_type)?)?;
                        rmp::encode::write_str(&mut buf, &lookup(*value_type)?)?;
                    }
                }
            }
        }

        Ok(())
    }
}
