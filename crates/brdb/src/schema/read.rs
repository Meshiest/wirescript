use std::{collections::HashMap, io::Read, sync::Arc};

use indexmap::IndexMap;
use rmp::{Marker, decode::RmpRead};

use crate::{
    errors::BrdbSchemaError,
    schema::{
        BrdbEnum, BrdbInterned, BrdbSchema, BrdbSchemaEnum, BrdbSchemaStruct,
        BrdbSchemaStructProperty, BrdbStruct, BrdbValue, WireVariant,
    },
};

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

pub fn read_type(
    schema: &Arc<BrdbSchema>,
    ty: &str,
    buf: &mut impl Read,
) -> Result<BrdbValue, BrdbSchemaError> {
    // TODO: nil handling for struct fields
    Ok(match ty {
        "bool" => BrdbValue::Bool(read_bool(buf)?),
        "u8" => BrdbValue::U8(read_uint(buf)? as u8),
        "u16" => BrdbValue::U16(read_uint(buf)? as u16),
        "u32" => BrdbValue::U32(read_uint(buf)? as u32),
        "u64" => BrdbValue::U64(read_uint(buf)?),
        "i8" => BrdbValue::I8(read_int(buf)? as i8),
        "i16" => BrdbValue::I16(read_int(buf)? as i16),
        "i32" => BrdbValue::I32(read_int(buf)? as i32),
        "i64" => BrdbValue::I64(read_int(buf)?),
        "f32" => BrdbValue::F32(read_float32(buf)?),
        "f64" => BrdbValue::F64(read_float64(buf)?),
        "str" => BrdbValue::String(read_str(buf)?),
        "wire_graph_variant" => BrdbValue::WireVar(match read_uint(buf)? {
            0 => WireVariant::Number(read_float64(buf)?),
            1 => WireVariant::Int(read_int(buf)?),
            2 => WireVariant::Bool(read_bool(buf)?),
            3 => WireVariant::Object("unknown".to_string()),
            4 => WireVariant::Exec,
            other => return Err(BrdbSchemaError::UnknownWireVariant(other as usize)),
        }),
        "wire_graph_prim_math_variant" => BrdbValue::WireVar(match read_uint(buf)? {
            0 => WireVariant::Number(read_float64(buf)?),
            1 => WireVariant::Int(read_int(buf)?),
            other => return Err(BrdbSchemaError::UnknownWireVariant(other as usize)),
        }),
        "class" | "object" => {
            // Assets are stored as u64 indices
            let id = read_int(buf)?;
            if id < 0 {
                BrdbValue::Asset(None)
            } else {
                let id = id as usize;
                if schema.global_data.external_asset_references.len() <= id {
                    return Err(BrdbSchemaError::UnknownAsset(ty.to_owned(), id));
                }
                BrdbValue::Asset(Some(id))
            }
        }
        other => {
            if let Some(ty) = schema.intern.get(&other) {
                read_named_type(&schema, buf, ty)?
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
        read_struct(&schema, buf, ty, s).map_err(|e| e.wrap(ty.get_or(schema, "unknown struct")))
    } else if let Some(e) = schema.get_enum_interned(ty) {
        read_enum(&schema, buf, ty, e).map_err(|e| e.wrap(ty.get_or(schema, "unknown enum")))
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
        properties.insert(
            *k,
            read_struct_property(schema, buf, v)
                .map_err(|e| e.wrap(k.get_or(schema, "unknown prop")))?,
        );
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
        BrdbSchemaStructProperty::Type(ty) => read_type(schema, &lookup(*ty)?, buf),
        BrdbSchemaStructProperty::Array(ty) => {
            let mut values = Vec::new();
            let len = rmp::decode::read_array_len(buf)? as usize;
            for i in 0..len {
                values.push(read_type(schema, &lookup(*ty)?, buf).map_err(|e| e.wrap(i))?);
            }
            Ok(BrdbValue::Array(values))
        }
        BrdbSchemaStructProperty::FlatArray(ty) => {
            // Read the allocated data for the flat array.
            let flat_buf_len = rmp::decode::read_bin_len(buf)? as usize;

            // Create a buffer with this length
            let mut flat_buf = vec![0; flat_buf_len];
            buf.read_exact(&mut flat_buf)
                .map_err(BrdbSchemaError::ReadError)?;
            let flat_buf = &mut &flat_buf[..];

            // Determine the size of the flat type and validate the buffer length.
            let flat_ty = &lookup(*ty)?;
            let ty_size = flat_type_size(schema, flat_ty);
            if ty_size == 0 {
                return Err(BrdbSchemaError::InvalidFlatType(flat_ty.to_string()));
            }
            if flat_buf_len % ty_size != 0 {
                return Err(BrdbSchemaError::InvalidFlatDataSize(
                    flat_ty.to_string(),
                    flat_buf_len,
                    ty_size,
                ));
            }

            // Read the flat array items from the buffer.
            let mut items = Vec::with_capacity(flat_buf_len / ty_size);
            for i in 0..(flat_buf_len / ty_size) {
                items.push(read_flat_type(schema, flat_ty, flat_buf).map_err(|e| e.wrap(i))?);
            }

            Ok(BrdbValue::FlatArray(items))
        }
        BrdbSchemaStructProperty::Map(k_ty, v_ty) => {
            let mut map = IndexMap::new();
            let len = read_uint(buf)? as usize;
            for _ in 0..len {
                let key = read_named_type(schema, buf, *k_ty)
                    .map_err(|e| e.wrap(k_ty.get_or(schema, "unknown map key")))?;
                let value = read_named_type(schema, buf, *v_ty)
                    .map_err(|e| e.wrap(k_ty.get_or(schema, "unknown map value")))?;
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
    let value = read_uint(buf)?;
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

fn read_str(buf: &mut impl Read) -> Result<String, BrdbSchemaError> {
    let len = rmp::decode::read_str_len(buf)?;
    read_str_from_len(buf, len as usize)
}

/// Read an ambiguously encoded signed integer from the buffer.
pub(crate) fn read_int(buf: &mut impl Read) -> Result<i64, BrdbSchemaError> {
    let marker =
        rmp::decode::read_marker(buf).map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
    Ok(match marker {
        Marker::FixPos(value) => value as i64,
        Marker::U8 => buf
            .read_u8()
            .map_err(rmp::decode::ValueReadError::InvalidDataRead)? as i64,
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

/// Read an ambiguously encoded unsigned integer from the buffer.
fn read_uint(buf: &mut impl Read) -> Result<u64, BrdbSchemaError> {
    let marker =
        rmp::decode::read_marker(buf).map_err(|e| BrdbSchemaError::RmpMarkerReadError(e.0))?;
    Ok(match marker {
        Marker::FixPos(value) => value as u64,
        Marker::U8 => buf
            .read_u8()
            .map_err(rmp::decode::ValueReadError::InvalidDataRead)? as u64,
        Marker::U16 => buf.read_data_u16()? as u64,
        Marker::U32 => buf.read_data_u32()? as u64,
        Marker::U64 => buf.read_data_u64()? as u64,
        // It's very sneaky making values 128 thru 255 use FixNeg markers...
        Marker::FixNeg(value) => (256 + (value as i16)) as u64,
        m => {
            return Err(BrdbSchemaError::ExpectedType(
                "uint".to_string(),
                format!("marker {m:?}"),
            ));
        }
    })
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
                format!("Unexpected marker {marker:?} for float32"),
            )));
        }
    })
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
        Marker::F32 => buf.read_data_f32().map_err(BrdbSchemaError::from)? as f64,
        Marker::F64 => buf.read_data_f64().map_err(BrdbSchemaError::from)?,
        _ => {
            return Err(BrdbSchemaError::RmpMarkerReadError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unexpected marker {marker:?} for float64"),
            )));
        }
    })
}

/// Determine the byte size of a flat type in the schema.
pub fn flat_type_size(schema: &BrdbSchema, ty: &str) -> usize {
    match ty {
        "u8" => 1,
        "u16" => 2,
        "u32" => 4,
        "u64" => 8,
        "i8" => 1,
        "i16" => 2,
        "i32" => 4,
        "i64" => 8,
        "f32" => 4,
        "f64" => 8,
        // The only other supported flat type is structs with properties that are also flat
        other => {
            let Some(ty) = schema.get_struct(other) else {
                return 0;
            };

            ty.values()
                .map(|prop| match prop {
                    BrdbSchemaStructProperty::Type(ty) => schema
                        .intern
                        .lookup_ref(*ty)
                        .map(|ty_str| flat_type_size(schema, ty_str))
                        .unwrap_or_default(),
                    _ => 0,
                })
                .sum()
        }
    }
}

/// Read a flat type from a buffer. Flat types are memory dumps of structs or numeric types and
/// are not encoded in a message pack format.
fn read_flat_type(
    schema: &Arc<BrdbSchema>,
    ty: &str,
    buf: &mut impl Read,
) -> Result<BrdbValue, BrdbSchemaError> {
    Ok(match ty {
        "u8" => BrdbValue::U8(read_flat_u8(buf)?),
        "u16" => BrdbValue::U16(read_flat_u16(buf)?),
        "u32" => BrdbValue::U32(read_flat_u32(buf)?),
        "u64" => BrdbValue::U64(read_flat_u64(buf)?),
        "i8" => BrdbValue::I8(read_flat_i8(buf)?),
        "i16" => BrdbValue::I16(read_flat_i16(buf)?),
        "i32" => BrdbValue::I32(read_flat_i32(buf)?),
        "i64" => BrdbValue::I64(read_flat_i64(buf)?),
        "f32" => BrdbValue::F32(read_flat_f32(buf)?),
        "f64" => BrdbValue::F64(read_flat_f64(buf)?),
        // The only other supported flat type is structs with properties that are also flat
        other => {
            if let Some((intern, ty)) = schema
                .intern
                .get(other)
                .and_then(|i| schema.structs.get(&i).map(|s| (i, s)))
            {
                read_flat_struct(schema, buf, intern, ty).map_err(|e| e.wrap(other))?
            } else {
                return Err(BrdbSchemaError::InvalidFlatType(other.to_owned()));
            }
        }
    })
}

fn read_flat_struct(
    schema: &Arc<BrdbSchema>,
    buf: &mut impl Read,
    name: BrdbInterned,
    s: &BrdbSchemaStruct,
) -> Result<BrdbValue, BrdbSchemaError> {
    let mut properties = HashMap::with_capacity(s.len());
    for (k, v) in s.iter() {
        properties.insert(
            *k,
            read_flat_struct_property(schema, buf, v)
                .map_err(|e| e.wrap(k.get_or(schema, "unknown prop")))?,
        );
    }
    Ok(BrdbValue::Struct(Box::new(BrdbStruct {
        schema: Arc::clone(schema),
        name,
        properties,
    })))
}

fn read_flat_struct_property(
    schema: &Arc<BrdbSchema>,
    buf: &mut impl Read,
    prop: &BrdbSchemaStructProperty,
) -> Result<BrdbValue, BrdbSchemaError> {
    let lookup = |ty: BrdbInterned| {
        ty.get_ok(schema, || {
            BrdbSchemaError::UnknownStructPropertyType(ty.0.to_string())
        })
    };

    match prop {
        BrdbSchemaStructProperty::Type(ty) => read_flat_type(schema, &lookup(*ty)?, buf),
        prop => Err(BrdbSchemaError::UnknownType(format!(
            "flat {}",
            prop.as_string(schema)
        ))),
    }
}

fn read_flat_u8(buf: &mut impl Read) -> Result<u8, BrdbSchemaError> {
    Ok(buf.read_u8()?)
}

fn read_flat_u16(buf: &mut impl Read) -> Result<u16, BrdbSchemaError> {
    let mut bytes = [0; 2];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_flat_u32(buf: &mut impl Read) -> Result<u32, BrdbSchemaError> {
    let mut bytes = [0; 4];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(u32::from_le_bytes(bytes))
}
fn read_flat_u64(buf: &mut impl Read) -> Result<u64, BrdbSchemaError> {
    let mut bytes = [0; 8];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(u64::from_le_bytes(bytes))
}
fn read_flat_i8(buf: &mut impl Read) -> Result<i8, BrdbSchemaError> {
    let byte = buf.read_u8().map_err(BrdbSchemaError::ReadError)?;
    Ok(byte as i8)
}
fn read_flat_i16(buf: &mut impl Read) -> Result<i16, BrdbSchemaError> {
    let mut bytes = [0; 2];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(i16::from_le_bytes(bytes))
}
fn read_flat_i32(buf: &mut impl Read) -> Result<i32, BrdbSchemaError> {
    let mut bytes = [0; 4];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(i32::from_le_bytes(bytes))
}
fn read_flat_i64(buf: &mut impl Read) -> Result<i64, BrdbSchemaError> {
    let mut bytes = [0; 8];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(i64::from_le_bytes(bytes))
}
fn read_flat_f32(buf: &mut impl Read) -> Result<f32, BrdbSchemaError> {
    let mut bytes = [0; 4];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(f32::from_le_bytes(bytes))
}
fn read_flat_f64(buf: &mut impl Read) -> Result<f64, BrdbSchemaError> {
    let mut bytes = [0; 8];
    buf.read_exact(&mut bytes)
        .map_err(BrdbSchemaError::ReadError)?;
    Ok(f64::from_le_bytes(bytes))
}
