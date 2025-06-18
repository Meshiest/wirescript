use std::{
    collections::HashMap,
    hash::Hash,
    io::{Read, Write},
    sync::Arc,
};

use indexmap::IndexMap;
use rmp::{Marker, decode::RmpRead};

use crate::brdb::{
    errors::BrdbSchemaError,
    schema::{
        BrdbInterned, BrdbSchema, BrdbSchemaEnum, BrdbSchemaStruct, BrdbSchemaStructProperty,
    },
};

#[derive(Clone)]
pub struct BrdbEnum {
    schema: Arc<BrdbSchema>,
    pub name: BrdbInterned,
    pub value: u64,
}

#[derive(Clone)]
pub struct BrdbStruct {
    schema: Arc<BrdbSchema>,
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

#[derive(Clone)]
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
    Asset(usize),
    Enum(BrdbEnum),
    Struct(Box<BrdbStruct>),
    Array(Vec<BrdbValue>),
    FlatArray(Vec<BrdbValue>),
    Map(IndexMap<BrdbValue, BrdbValue>),
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
impl Eq for BrdbValue {}

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
            Self::Array(v) | Self::FlatArray(v) => Ok(v),
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

    pub fn read_type_vec(
        schema: &Arc<BrdbSchema>,
        ty: &str,
        buf: &mut impl Read,
    ) -> Result<Vec<BrdbValue>, BrdbSchemaError> {
        let len = rmp::decode::read_array_len(buf)? as usize;
        let mut res = Vec::with_capacity(len);
        for _ in 0..len {
            res.push(Self::read_type(schema, ty, buf)?)
        }
        Ok(res)
    }

    pub fn read_type(
        schema: &Arc<BrdbSchema>,
        ty: &str,
        buf: &mut impl Read,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        // TODO: nil handling for struct fields
        Ok(match ty.as_ref() {
            "bool" => BrdbValue::Bool(Self::read_bool(buf)?),
            "u8" => BrdbValue::U8(Self::read_uint(buf)? as u8),
            "u16" => BrdbValue::U16(Self::read_uint(buf)? as u16),
            "u32" => BrdbValue::U32(Self::read_uint(buf)? as u32),
            "u64" => BrdbValue::U64(Self::read_uint(buf)?),
            "i8" => BrdbValue::I8(Self::read_int(buf)? as i8),
            "i16" => BrdbValue::I16(Self::read_int(buf)? as i16),
            "i32" => BrdbValue::I32(Self::read_int(buf)? as i32),
            "i64" => BrdbValue::I64(Self::read_int(buf)?),
            "f32" => BrdbValue::F32(Self::read_float32(buf)?),
            "f64" => BrdbValue::F64(Self::read_float64(buf)?),
            "str" => BrdbValue::String(Self::read_str(buf)?),
            other => {
                if schema.global_data.external_asset_types.contains(other) {
                    let id = Self::read_uint(buf)? as usize;
                    if !schema
                        .global_data
                        .external_asset_references
                        .get_index(id)
                        .is_some_and(|(asset_ty, _)| asset_ty == ty)
                    {
                        return Err(BrdbSchemaError::UnknownAsset(ty.to_owned(), id));
                    }
                    BrdbValue::Asset(id)
                } else if let Some(ty) = schema.intern.get(&other) {
                    Self::read_named_type(&schema, buf, ty)?
                } else {
                    return Err(BrdbSchemaError::UnknownType(other.to_string()));
                }
            }
        })
    }

    fn read_named_type(
        schema: &Arc<BrdbSchema>,
        buf: &mut impl Read,
        ty: BrdbInterned,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        if let Some(s) = schema.get_struct_interned(ty) {
            Self::read_struct(&schema, buf, ty, s)
        } else if let Some(e) = schema.get_enum_interned(ty) {
            Self::read_enum(&schema, buf, ty, e)
        } else {
            return Err(BrdbSchemaError::UnknownSchemaType(
                schema
                    .intern
                    .lookup(ty)
                    .unwrap_or_else(|| format!("unknown ({})", ty.0)),
            ));
        }
    }

    fn read_struct(
        schema: &Arc<BrdbSchema>,
        buf: &mut impl Read,
        name: BrdbInterned,
        s: &BrdbSchemaStruct,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        let mut properties = HashMap::with_capacity(s.len());
        for (k, v) in s.iter() {
            properties.insert(*k, Self::read_struct_property(schema, buf, v)?);
        }
        Ok(BrdbValue::Struct(Box::new(BrdbStruct {
            schema: Arc::clone(schema),
            name,
            properties,
        })))
    }

    fn read_struct_property(
        schema: &Arc<BrdbSchema>,
        buf: &mut impl Read,
        prop: &BrdbSchemaStructProperty,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        let lookup = |ty: BrdbInterned| {
            schema
                .intern
                .lookup_ref(ty)
                .ok_or(BrdbSchemaError::UnknownStructPropertyType(ty.0.to_string()))
        };

        match prop {
            BrdbSchemaStructProperty::Type(ty) => Self::read_type(schema, &lookup(*ty)?, buf),
            BrdbSchemaStructProperty::Array(ty) => {
                let mut values = Vec::new();
                let len = rmp::decode::read_array_len(buf)? as usize;
                for _ in 0..len {
                    values.push(Self::read_type(schema, &lookup(*ty)?, buf)?);
                }
                Ok(BrdbValue::Array(values))
            }
            BrdbSchemaStructProperty::FlatArray(ty) => {
                let mut values = Vec::new();
                let len = rmp::decode::read_array_len(buf)? as usize;
                for _ in 0..len {
                    values.push(Self::read_type(schema, &lookup(*ty)?, buf)?);
                }
                Ok(BrdbValue::FlatArray(values))
            }
            BrdbSchemaStructProperty::Map(k_ty, v_ty) => {
                let mut map = IndexMap::new();
                let len = Self::read_uint(buf)? as usize;
                for _ in 0..len {
                    let key = Self::read_named_type(schema, buf, *k_ty)?;
                    let value = Self::read_named_type(schema, buf, *v_ty)?;
                    map.insert(key, value);
                }
                Ok(BrdbValue::Map(map))
            }
        }
    }

    fn read_enum(
        schema: &Arc<BrdbSchema>,
        buf: &mut impl Read,
        name: BrdbInterned,
        e: &BrdbSchemaEnum,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        let value = Self::read_uint(buf)?;
        if e.len() <= value as usize {
            return Err(BrdbSchemaError::EnumIndexOutOfBounds {
                enum_name: schema
                    .intern
                    .lookup(name)
                    .unwrap_or_else(|| format!("unknown ({})", name.0)),
                index: value,
            });
        }
        Ok(BrdbValue::Enum(BrdbEnum {
            schema: Arc::clone(schema),
            name,
            value,
        }))
    }

    fn read_bool(buf: &mut impl Read) -> Result<bool, BrdbSchemaError> {
        rmp::decode::read_bool(buf).map_err(BrdbSchemaError::from)
    }

    fn write_bool(buf: &mut impl Write, value: bool) -> Result<(), BrdbSchemaError> {
        rmp::encode::write_bool(buf, value)?;
        Ok(())
    }

    fn read_str(buf: &mut impl Read) -> Result<String, BrdbSchemaError> {
        let len = rmp::decode::read_str_len(buf)?;
        read_str_from_len(buf, len as usize)
    }

    fn write_str(mut buf: &mut impl Write, value: &str) -> Result<(), BrdbSchemaError> {
        rmp::encode::write_str(buf, value)?;
        Ok(())
    }

    /// Read an ambiguously encoded signed integer from the buffer.
    fn read_int(buf: &mut impl Read) -> Result<i64, BrdbSchemaError> {
        let marker =
            rmp::decode::read_marker(buf).map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
        Ok(match marker {
            Marker::FixPos(value) => value as i64,
            Marker::U8 => {
                buf.read_u8()
                    .map_err(rmp::decode::ValueReadError::InvalidDataRead)? as i64
            }
            Marker::U16 => buf.read_data_u16()? as i64,
            Marker::U32 => buf.read_data_u32()? as i64,
            Marker::U64 => buf.read_data_u64()? as i64,
            Marker::FixNeg(value) => value as i64,
            Marker::I8 => buf.read_data_i8()? as i64,
            Marker::I16 => buf.read_data_i16()? as i64,
            Marker::I32 => buf.read_data_i32()? as i64,
            Marker::I64 => buf.read_data_i64()? as i64,
            _ => {
                return Err(BrdbSchemaError::RmpMarkerReadError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected marker for integer",
                )));
            }
        })
    }

    /// Write the smallest possible integer representation of `value` to the buffer.
    fn write_int(buf: &mut impl Write, value: i64) -> Result<(), BrdbSchemaError> {
        if value >= 0 {
            if value < 128 {
                rmp::encode::write_pfix(buf, value as u8)?;
            } else if value <= i8::MAX as i64 {
                rmp::encode::write_i8(buf, value as i8)?;
            } else if value <= i16::MAX as i64 {
                rmp::encode::write_i16(buf, value as i16)?;
            } else if value <= i32::MAX as i64 {
                rmp::encode::write_i32(buf, value as i32)?;
            } else {
                rmp::encode::write_i64(buf, value)?;
            }
        } else {
            if value > -32 {
                rmp::encode::write_nfix(buf, value as i8)?;
            } else if value >= i8::MIN as i64 {
                rmp::encode::write_i8(buf, value as i8)?;
            } else if value >= i16::MIN as i64 {
                rmp::encode::write_i16(buf, value as i16)?;
            } else if value >= i32::MIN as i64 {
                rmp::encode::write_i32(buf, value as i32)?;
            } else {
                rmp::encode::write_i64(buf, value)?;
            }
        }
        Ok(())
    }

    /// Read an ambiguously encoded unsigned integer from the buffer.
    fn read_uint(buf: &mut impl Read) -> Result<u64, BrdbSchemaError> {
        let marker =
            rmp::decode::read_marker(buf).map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
        Ok(match marker {
            Marker::FixPos(value) => value as u64,
            Marker::U8 => {
                buf.read_u8()
                    .map_err(rmp::decode::ValueReadError::InvalidDataRead)? as u64
            }
            Marker::U16 => buf.read_data_u16()? as u64,
            Marker::U32 => buf.read_data_u32()? as u64,
            Marker::U64 => buf.read_data_u64()? as u64,
            m => {
                return Err(BrdbSchemaError::ExpectedType(
                    "uint".to_string(),
                    format!("marker {m:?}"),
                ));
            }
        })
    }

    /// Write the smallest possible unsigned integer representation of `value` to the buffer.
    fn write_uint(buf: &mut impl Write, value: u64) -> Result<(), BrdbSchemaError> {
        if value < 128 {
            rmp::encode::write_pfix(buf, value as u8)?;
        } else if value <= u8::MAX as u64 {
            rmp::encode::write_u8(buf, value as u8)?;
        } else if value <= u16::MAX as u64 {
            rmp::encode::write_u16(buf, value as u16)?;
        } else if value <= u32::MAX as u64 {
            rmp::encode::write_u32(buf, value as u32)?;
        } else {
            rmp::encode::write_u64(buf, value)?;
        }
        Ok(())
    }

    fn read_float32(buf: &mut impl Read) -> Result<f32, BrdbSchemaError> {
        let marker =
            rmp::decode::read_marker(buf).map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
        Ok(match marker {
            Marker::FixPos(value) => value as f32,
            Marker::FixNeg(value) => value as f32,
            Marker::I8 => buf.read_data_i8().map_err(BrdbSchemaError::from)? as f32,
            Marker::I16 => buf.read_data_i16().map_err(BrdbSchemaError::from)? as f32,
            Marker::U8 => buf.read_data_u8().map_err(BrdbSchemaError::from)? as f32,
            Marker::U16 => buf.read_data_u16().map_err(BrdbSchemaError::from)? as f32,
            Marker::F32 => buf.read_data_f32().map_err(BrdbSchemaError::from)?,
            _ => {
                return Err(BrdbSchemaError::RmpMarkerReadError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected marker for float32",
                )));
            }
        })
    }

    fn write_float32(mut buf: &mut impl Write, value: f32) -> Result<(), BrdbSchemaError> {
        // Attempt to write as ints on whole numbers less than 8 or 16 bits
        if value.eq(&value.round()) && (value as u16) < u16::MAX && (value as i16) > i16::MIN {
            Self::write_int(buf, value as i64)?;
        } else {
            rmp::encode::write_f32(buf, value)?;
        }
        Ok(())
    }

    fn read_float64(buf: &mut impl Read) -> Result<f64, BrdbSchemaError> {
        let marker =
            rmp::decode::read_marker(buf).map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
        Ok(match marker {
            Marker::FixPos(value) => value as f64,
            Marker::FixNeg(value) => value as f64,
            Marker::I8 => buf.read_data_i8().map_err(BrdbSchemaError::from)? as f64,
            Marker::I16 => buf.read_data_i16().map_err(BrdbSchemaError::from)? as f64,
            Marker::I32 => buf.read_data_i32().map_err(BrdbSchemaError::from)? as f64,
            Marker::U8 => buf.read_data_u8().map_err(BrdbSchemaError::from)? as f64,
            Marker::U16 => buf.read_data_u16().map_err(BrdbSchemaError::from)? as f64,
            Marker::U32 => buf.read_data_u32().map_err(BrdbSchemaError::from)? as f64,
            Marker::F64 => buf.read_data_f64().map_err(BrdbSchemaError::from)?,
            _ => {
                return Err(BrdbSchemaError::RmpMarkerReadError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected marker for float64",
                )));
            }
        })
    }

    fn write_float64(buf: &mut impl Write, value: f64) -> Result<(), BrdbSchemaError> {
        // Attempt to write as ints on whole numbers less than 8, 16, or 32 bits
        if value.eq(&value.round()) && (value as u32) < u32::MAX && (value as i32) > i32::MIN {
            Self::write_int(buf, value as i64)?;
        } else {
            rmp::encode::write_f64(buf, value)?;
        }
        Ok(())
    }
}

/// Read a message pack string from the stream and return it as an owned String.
pub fn read_owned_str(bytes: &mut impl Read) -> Result<String, BrdbSchemaError> {
    let len = rmp::decode::read_str_len(bytes)? as usize;
    read_str_from_len(bytes, len)
}

/// Read a message pack string of a specific length from the stream and return it as an owned String.
pub fn read_str_from_len(bytes: &mut impl Read, len: usize) -> Result<String, BrdbSchemaError> {
    let mut buf = vec![0; len as usize];
    bytes
        .read_exact(&mut buf)
        .map_err(BrdbSchemaError::ReadError)?;
    String::from_utf8(buf).map_err(BrdbSchemaError::InvalidUtf8)
}
