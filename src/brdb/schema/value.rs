use std::{collections::HashMap, hash::Hash, sync::Arc};

use indexmap::IndexMap;

use crate::brdb::{
    errors::BrdbSchemaError,
    schema::{BrdbInterned, BrdbSchema},
};

#[derive(Clone, Debug)]
pub struct BrdbEnum {
    pub(crate) schema: Arc<BrdbSchema>,
    pub name: BrdbInterned,
    pub value: u64,
}

#[derive(Clone, Debug)]
pub struct BrdbStruct {
    pub(crate) schema: Arc<BrdbSchema>,
    pub name: BrdbInterned,
    pub properties: HashMap<BrdbInterned, BrdbValue>,
}

impl BrdbStruct {
    pub fn get(&self, prop: impl AsRef<str>) -> Option<&BrdbValue> {
        let key = self.schema.intern.get(prop.as_ref())?;
        self.properties.get(&key)
    }

    pub fn prop(&self, prop: impl AsRef<str>) -> Result<&BrdbValue, BrdbSchemaError> {
        let prop = prop.as_ref();
        self.get(prop).ok_or_else(|| {
            BrdbSchemaError::MissingStructField(
                self.schema
                    .intern
                    .lookup(self.name)
                    .unwrap_or_else(|| "unknown struct".to_string()),
                prop.to_owned(),
            )
        })
    }
}

impl BrdbEnum {
    pub fn get_value_raw(&self) -> u64 {
        self.value
    }

    pub fn get_name(&self) -> &str {
        self.schema
            .intern
            .lookup_ref(self.name)
            .unwrap_or("unknown")
    }

    pub fn get_value(&self) -> String {
        self.schema
            .intern
            .lookup(self.name)
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[derive(Clone, Debug)]
pub enum BrdbValue {
    Nil,
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Asset(Option<usize>),
    Enum(BrdbEnum),
    Struct(Box<BrdbStruct>),
    Array(Vec<BrdbValue>),
    FlatArray(Vec<BrdbValue>),
    Map(IndexMap<BrdbValue, BrdbValue>),
}

impl BrdbValue {
    pub fn get_type(&self) -> &'static str {
        match self {
            BrdbValue::Nil => "nil",
            BrdbValue::Bool(_) => "bool",
            BrdbValue::U8(_) => "u8",
            BrdbValue::U16(_) => "u16",
            BrdbValue::U32(_) => "u32",
            BrdbValue::U64(_) => "u64",
            BrdbValue::I8(_) => "i8",
            BrdbValue::I16(_) => "i16",
            BrdbValue::I32(_) => "i32",
            BrdbValue::I64(_) => "i64",
            BrdbValue::F32(_) => "f32",
            BrdbValue::F64(_) => "f64",
            BrdbValue::String(_) => "string",
            BrdbValue::Asset(_) => "asset",
            BrdbValue::Enum(_) => "enum",
            BrdbValue::Struct(_) => "struct",
            BrdbValue::Array(_) => "array",
            BrdbValue::FlatArray(_) => "flatarray",
            BrdbValue::Map(_) => "map",
        }
    }
    pub fn as_struct(&self) -> Result<&BrdbStruct, BrdbSchemaError> {
        if let Self::Struct(v) = self {
            Ok(v)
        } else {
            Err(BrdbSchemaError::ExpectedType(
                "struct".to_owned(),
                self.get_type().to_string(),
            ))
        }
    }

    pub fn as_array(&self) -> Result<&Vec<BrdbValue>, BrdbSchemaError> {
        match self {
            Self::Array(v) => Ok(v),
            _ => Err(BrdbSchemaError::ExpectedType(
                "array".to_owned(),
                self.get_type().to_string(),
            )),
        }
    }

    pub fn as_str(&self) -> Result<&str, BrdbSchemaError> {
        if let Self::String(v) = self {
            Ok(v)
        } else {
            Err(BrdbSchemaError::ExpectedType(
                "string".to_owned(),
                self.get_type().to_string(),
            ))
        }
    }

    pub fn display(&self, schema: &BrdbSchema) -> String {
        self.display_inner(schema, 0)
    }

    fn display_inner(&self, schema: &BrdbSchema, depth: usize) -> String {
        match self {
            BrdbValue::Nil => "nil".to_string(),
            BrdbValue::Bool(v) => format!("{v}"),
            BrdbValue::U8(v) => format!("{v}u8"),
            BrdbValue::U16(v) => format!("{v}u16"),
            BrdbValue::U32(v) => format!("{v}u32"),
            BrdbValue::U64(v) => format!("{v}u64"),
            BrdbValue::I8(v) => format!("{v}i8"),
            BrdbValue::I16(v) => format!("{v}i16"),
            BrdbValue::I32(v) => format!("{v}i32"),
            BrdbValue::I64(v) => format!("{v}i64"),
            BrdbValue::F32(v) => format!("{v}f32"),
            BrdbValue::F64(v) => format!("{v}f64"),
            BrdbValue::String(v) => format!("\"{v}\""),
            BrdbValue::Asset(None) => "none".to_string(),
            BrdbValue::Asset(Some(v)) => {
                if let Some((asset_ty, asset_name)) =
                    schema.global_data.external_asset_references.get_index(*v)
                {
                    format!("{asset_ty}/{asset_name}")
                } else {
                    format!("unknown asset {v}")
                }
            }
            BrdbValue::Enum(e) => format!("{}::{}", e.get_name(), e.get_value()),
            BrdbValue::Struct(s) => {
                let pad = "  ".repeat(depth);
                let mut props = s
                    .properties
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{pad}  {}: {},\n",
                            schema.intern.lookup_ref(*k).unwrap_or("unknown prop"),
                            v.display_inner(schema, depth + 1)
                        )
                    })
                    .collect::<Vec<_>>();
                props.sort();
                format!(
                    "{} {{\n{}{pad}}}",
                    schema.intern.lookup_ref(s.name).unwrap_or("unknown struct"),
                    props.join("")
                )
            }
            BrdbValue::Array(v) => {
                let pad = "  ".repeat(depth);
                let elems = v
                    .iter()
                    .map(|e| format!("{pad}  {},\n", e.display_inner(schema, depth + 1)))
                    .collect::<Vec<_>>();
                format!("[\n{}{}]", elems.join(""), "  ".repeat(depth))
            }
            BrdbValue::FlatArray(v) => {
                let pad = "  ".repeat(depth);
                let elems = v
                    .iter()
                    .map(|e| format!("{pad}  {},\n", e.display_inner(schema, depth + 1)))
                    .collect::<Vec<_>>();
                format!("flat[\n{}{}]", elems.join(""), "  ".repeat(depth))
            }
            BrdbValue::Map(map) => {
                let pad = "  ".repeat(depth);
                let mut entries = map
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{pad}  {}: {},\n",
                            k.display_inner(schema, depth + 1),
                            v.display_inner(schema, depth + 1)
                        )
                    })
                    .collect::<Vec<_>>();
                entries.sort();
                format!("{{\n{}\n{pad}}}", entries.join(""))
            }
        }
    }
}

impl Hash for BrdbValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        match self {
            BrdbValue::Nil => ().hash(state),
            BrdbValue::Bool(v) => v.hash(state),
            BrdbValue::U8(v) => v.hash(state),
            BrdbValue::U16(v) => v.hash(state),
            BrdbValue::U32(v) => v.hash(state),
            BrdbValue::U64(v) => v.hash(state),
            BrdbValue::I8(v) => v.hash(state),
            BrdbValue::I16(v) => v.hash(state),
            BrdbValue::I32(v) => v.hash(state),
            BrdbValue::I64(v) => v.hash(state),
            BrdbValue::F32(v) => v.to_bits().hash(state),
            BrdbValue::F64(v) => v.to_bits().hash(state),
            BrdbValue::String(v) => v.hash(state),
            BrdbValue::Asset(v) => v.hash(state),
            BrdbValue::Enum(e) => {
                e.name.hash(state);
                e.value.hash(state);
            }
            BrdbValue::Struct(s) => {
                s.name.hash(state);
                for (k, v) in &s.properties {
                    k.hash(state);
                    v.hash(state);
                }
            }
            BrdbValue::Array(v) => v.hash(state),
            BrdbValue::FlatArray(v) => v.hash(state),
            BrdbValue::Map(map) => map.iter().for_each(|(k, v)| {
                k.hash(state);
                v.hash(state);
            }),
        }
    }
}

impl Eq for BrdbValue {}

impl PartialEq for BrdbValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(l0), Self::Bool(r0)) => l0 == r0,
            (Self::U8(l0), Self::U8(r0)) => l0 == r0,
            (Self::U16(l0), Self::U16(r0)) => l0 == r0,
            (Self::U32(l0), Self::U32(r0)) => l0 == r0,
            (Self::U64(l0), Self::U64(r0)) => l0 == r0,
            (Self::I8(l0), Self::I8(r0)) => l0 == r0,
            (Self::I16(l0), Self::I16(r0)) => l0 == r0,
            (Self::I32(l0), Self::I32(r0)) => l0 == r0,
            (Self::I64(l0), Self::I64(r0)) => l0 == r0,
            (Self::F32(l0), Self::F32(r0)) => l0 == r0,
            (Self::F64(l0), Self::F64(r0)) => l0 == r0,
            (Self::String(l0), Self::String(r0)) => l0 == r0,
            (Self::Asset(l0), Self::Asset(r0)) => l0 == r0,
            (Self::Enum(l0), Self::Enum(r0)) => l0.name == r0.name && l0.value == r0.value,
            (Self::Struct(l0), Self::Struct(r0)) => {
                if l0.name != r0.name {
                    return false;
                }
                // Compare all properties
                for (k, lv) in &l0.properties {
                    let Some(kv) = r0.properties.get(k) else {
                        return false;
                    };
                    if lv != kv {
                        return false;
                    }
                }
                return true;
            }
            (Self::Array(l0), Self::Array(r0)) => l0 == r0,
            (Self::FlatArray(l0), Self::FlatArray(r0)) => l0 == r0,
            (Self::Map(l0), Self::Map(r0)) => l0 == r0,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}
