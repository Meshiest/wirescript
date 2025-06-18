use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::Arc,
};

use indexmap::IndexMap;
use rmp::{Marker, decode::RmpRead};

use crate::brdb::{
    Brdb,
    errors::{BrdbSchemaError, BrdbValueError},
    schema::{
        BrdbInterned, BrdbSchema, BrdbSchemaEnum, BrdbSchemaStruct, BrdbSchemaStructProperty,
    },
};

#[derive(Clone)]
pub struct BrdbEnum {
    schema: Arc<BrdbSchema>,
    name: BrdbInterned,
    value: u64,
}

#[derive(Clone)]
pub struct BrdbStruct {
    schema: Arc<BrdbSchema>,
    name: BrdbInterned,
    properties: HashMap<BrdbInterned, BrdbValue>,
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
    Object(String),
    Class(String),
    Enum(BrdbEnum),
    Struct(Box<BrdbStruct>),
    Array(Vec<BrdbValue>),
    FlatArray(Vec<BrdbValue>),
    Map(IndexMap<String, BrdbValue>),
}

impl Brdb {
    pub fn read_type(
        schema: Arc<BrdbSchema>,
        ty: &str,
        mut buf: impl Read,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        // TODO: nil handling for struct fields
        Ok(match ty.as_ref() {
            "bool" => BrdbValue::Bool(Self::read_bool(&mut buf)?),
            "u8" => BrdbValue::U8(Self::read_uint(&mut buf)? as u8),
            "u16" => BrdbValue::U16(Self::read_uint(&mut buf)? as u16),
            "u32" => BrdbValue::U32(Self::read_uint(&mut buf)? as u32),
            "u64" => BrdbValue::U64(Self::read_uint(&mut buf)?),
            "i8" => BrdbValue::I8(Self::read_int(&mut buf)? as i8),
            "i16" => BrdbValue::I16(Self::read_int(&mut buf)? as i16),
            "i32" => BrdbValue::I32(Self::read_int(&mut buf)? as i32),
            "i64" => BrdbValue::I64(Self::read_int(&mut buf)?),
            "f32" => BrdbValue::F32(Self::read_float32(&mut buf)?),
            "f64" => BrdbValue::F64(Self::read_float64(&mut buf)?),
            "str" => BrdbValue::String(Self::read_str(&mut buf)?),
            other => {
                if let Some(s) = schema.get_struct(&other) {
                    Self::read_struct(&schema, buf, other, s)?
                } else if let Some(e) = schema.get_enum(&other) {
                    Self::read_enum(&schema, buf, other, e)?
                } else {
                    return Err(BrdbSchemaError::UnknownType(other.to_string()));
                }
            }
        })
    }

    fn read_struct(
        schema: &Arc<BrdbSchema>,
        mut buf: impl Read,
        other: &str,
        s: &BrdbSchemaStruct,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        Ok(BrdbValue::Struct(Box::new(BrdbStruct {
            schema: Arc::clone(schema),
            name: schema.intern.get_or_insert(other),
            properties: s
                .iter()
                .map(|(k, v)| Ok((*k, Self::read_struct_property(schema, &mut buf, v)?)))
                .collect::<Result<_, BrdbSchemaError>>()?,
        })))
    }

    fn read_struct_property(
        schema: &Arc<BrdbSchema>,
        mut buf: impl Read,
        prop: &BrdbSchemaStructProperty,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        let lookup = |ty: BrdbInterned| {
            schema
                .intern
                .lookup_ref(ty)
                .ok_or(BrdbSchemaError::UnknownType(ty.0.to_string()))
        };

        match prop {
            BrdbSchemaStructProperty::Type(ty) => {
                Self::read_type(Arc::clone(schema), &lookup(*ty)?, buf)
            }
            BrdbSchemaStructProperty::Array(ty) => {
                let mut values = Vec::new();
                let len = Self::read_uint(&mut buf)? as usize;
                for _ in 0..len {
                    values.push(Self::read_type(
                        Arc::clone(schema),
                        &lookup(*ty)?,
                        &mut buf,
                    )?);
                }
                Ok(BrdbValue::Array(values))
            }
            BrdbSchemaStructProperty::FlatArray(ty) => {
                let mut values = Vec::new();
                let len = Self::read_uint(&mut buf)? as usize;
                for _ in 0..len {
                    values.push(Self::read_type(
                        Arc::clone(schema),
                        &lookup(*ty)?,
                        &mut buf,
                    )?);
                }
                Ok(BrdbValue::FlatArray(values))
            }
            BrdbSchemaStructProperty::Map(k, v) => {
                let mut map = IndexMap::new();
                let len = Self::read_uint(&mut buf)? as usize;
                for _ in 0..len {
                    let key = Self::read_str(&mut buf)?;
                    let value = todo!("support some kind of hash function for map values");
                    map.insert(key, value);
                }
                Ok(BrdbValue::Map(map))
            }
        }
    }

    fn read_enum(
        schema: &Arc<BrdbSchema>,
        mut buf: impl Read,
        other: &str,
        e: &BrdbSchemaEnum,
    ) -> Result<BrdbValue, BrdbSchemaError> {
        let value = Self::read_uint(&mut buf)?;
        if e.len() <= value as usize {
            return Err(BrdbSchemaError::EnumIndexOutOfBounds {
                enum_name: other.to_string(),
                index: value,
            });
        }
        Ok(BrdbValue::Enum(BrdbEnum {
            schema: Arc::clone(schema),
            name: schema.intern.get_or_insert(other),
            value,
        }))
    }

    fn read_bool(mut buf: impl Read) -> Result<bool, BrdbSchemaError> {
        rmp::decode::read_bool(&mut buf).map_err(BrdbSchemaError::from)
    }

    fn write_bool(mut buf: impl Write, value: bool) -> Result<(), BrdbSchemaError> {
        rmp::encode::write_bool(&mut buf, value)?;
        Ok(())
    }

    fn read_str(mut buf: impl Read) -> Result<String, BrdbSchemaError> {
        let len = rmp::decode::read_str_len(&mut buf)?;
        read_str_from_len(&mut buf, len as usize)
    }

    fn write_str(mut buf: impl Write, value: &str) -> Result<(), BrdbSchemaError> {
        rmp::encode::write_str(&mut buf, value)?;
        Ok(())
    }

    /// Read an ambiguously encoded signed integer from the buffer.
    fn read_int(mut buf: impl Read) -> Result<i64, BrdbSchemaError> {
        let marker = rmp::decode::read_marker(&mut buf)
            .map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
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
    fn write_int(mut buf: impl Write, value: i64) -> Result<(), BrdbSchemaError> {
        if value >= 0 {
            if value < 128 {
                rmp::encode::write_pfix(&mut buf, value as u8)?;
            } else if value <= i8::MAX as i64 {
                rmp::encode::write_i8(&mut buf, value as i8)?;
            } else if value <= i16::MAX as i64 {
                rmp::encode::write_i16(&mut buf, value as i16)?;
            } else if value <= i32::MAX as i64 {
                rmp::encode::write_i32(&mut buf, value as i32)?;
            } else {
                rmp::encode::write_i64(&mut buf, value)?;
            }
        } else {
            if value > -32 {
                rmp::encode::write_nfix(&mut buf, value as i8)?;
            } else if value >= i8::MIN as i64 {
                rmp::encode::write_i8(&mut buf, value as i8)?;
            } else if value >= i16::MIN as i64 {
                rmp::encode::write_i16(&mut buf, value as i16)?;
            } else if value >= i32::MIN as i64 {
                rmp::encode::write_i32(&mut buf, value as i32)?;
            } else {
                rmp::encode::write_i64(&mut buf, value)?;
            }
        }
        Ok(())
    }

    /// Read an ambiguously encoded unsigned integer from the buffer.
    fn read_uint(mut buf: impl Read) -> Result<u64, BrdbSchemaError> {
        let marker = rmp::decode::read_marker(&mut buf)
            .map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
        Ok(match marker {
            Marker::FixPos(value) => value as u64,
            Marker::U8 => {
                buf.read_u8()
                    .map_err(rmp::decode::ValueReadError::InvalidDataRead)? as u64
            }
            Marker::U16 => buf.read_data_u16()? as u64,
            Marker::U32 => buf.read_data_u32()? as u64,
            Marker::U64 => buf.read_data_u64()? as u64,
            _ => {
                return Err(BrdbSchemaError::RmpMarkerReadError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unexpected marker for unsigned integer",
                )));
            }
        })
    }

    /// Write the smallest possible unsigned integer representation of `value` to the buffer.
    fn write_uint(mut buf: impl Write, value: u64) -> Result<(), BrdbSchemaError> {
        if value < 128 {
            rmp::encode::write_pfix(&mut buf, value as u8)?;
        } else if value <= u8::MAX as u64 {
            rmp::encode::write_u8(&mut buf, value as u8)?;
        } else if value <= u16::MAX as u64 {
            rmp::encode::write_u16(&mut buf, value as u16)?;
        } else if value <= u32::MAX as u64 {
            rmp::encode::write_u32(&mut buf, value as u32)?;
        } else {
            rmp::encode::write_u64(&mut buf, value)?;
        }
        Ok(())
    }

    fn read_float32(mut buf: impl Read) -> Result<f32, BrdbSchemaError> {
        let marker = rmp::decode::read_marker(&mut buf)
            .map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
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

    fn write_float32(mut buf: impl Write, value: f32) -> Result<(), BrdbSchemaError> {
        // Attempt to write as ints on whole numbers less than 8 or 16 bits
        if value.eq(&value.round()) && (value as u16) < u16::MAX && (value as i16) > i16::MIN {
            Self::write_int(&mut buf, value as i64)?;
        } else {
            rmp::encode::write_f32(&mut buf, value)?;
        }
        Ok(())
    }

    fn read_float64(mut buf: impl Read) -> Result<f64, BrdbSchemaError> {
        let marker = rmp::decode::read_marker(&mut buf)
            .map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
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

    fn write_float64(mut buf: impl Write, value: f64) -> Result<(), BrdbSchemaError> {
        // Attempt to write as ints on whole numbers less than 8, 16, or 32 bits
        if value.eq(&value.round()) && (value as u32) < u32::MAX && (value as i32) > i32::MIN {
            Self::write_int(&mut buf, value as i64)?;
        } else {
            rmp::encode::write_f64(&mut buf, value)?;
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
