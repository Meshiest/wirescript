use std::io::Write;

use crate::brdb::{
    errors::BrdbSchemaError,
    schema::{
        BrdbEnum, BrdbInterned, BrdbSchema, BrdbSchemaEnum, BrdbSchemaStruct,
        BrdbSchemaStructProperty, BrdbStruct, BrdbValue, read::flat_type_size,
    },
};

pub fn write_bool(buf: &mut impl Write, value: bool) -> Result<(), BrdbSchemaError> {
    rmp::encode::write_bool(buf, value)?;
    Ok(())
}

pub fn write_str(buf: &mut impl Write, value: &str) -> Result<(), BrdbSchemaError> {
    rmp::encode::write_str(buf, value)?;
    Ok(())
}

pub fn write_type(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &str,
    value: &BrdbValue,
) -> Result<(), BrdbSchemaError> {
    Ok(match (ty, value) {
        ("bool", BrdbValue::Bool(v)) => write_bool(buf, *v)?,
        ("u8", BrdbValue::U8(v)) => write_uint(buf, *v as u64)?,
        ("u16", BrdbValue::U16(v)) => write_uint(buf, *v as u64)?,
        ("u32", BrdbValue::U32(v)) => write_uint(buf, *v as u64)?,
        ("u64", BrdbValue::U64(v)) => write_uint(buf, *v)?,
        ("i8", BrdbValue::I8(v)) => write_int(buf, *v as i64)?,
        ("i16", BrdbValue::I16(v)) => write_int(buf, *v as i64)?,
        ("i32", BrdbValue::I32(v)) => write_int(buf, *v as i64)?,
        ("i64", BrdbValue::I64(v)) => write_int(buf, *v)?,
        ("f32", BrdbValue::F32(v)) => write_float32(buf, *v)?,
        ("f64", BrdbValue::F64(v)) => write_float64(buf, *v)?,
        ("str", BrdbValue::String(v)) => write_str(buf, &v)?,
        (other, BrdbValue::Asset(s)) => {
            if let Some((asset_ty, _)) = schema.global_data.external_asset_references.get_index(*s)
            {
                if asset_ty != other {
                    return Err(BrdbSchemaError::UnknownAsset(other.to_owned(), *s));
                }
                // Assets are stored as u64 indices
                write_uint(buf, *s as u64)?;
            } else {
                return Err(BrdbSchemaError::UnknownAsset(other.to_owned(), *s));
            }
        }
        (other, BrdbValue::Struct(_) | BrdbValue::Enum(_)) => {
            write_named_type(schema, buf, other, value)?
        }
        (expected, found) => {
            return Err(BrdbSchemaError::ExpectedType(
                expected.to_owned(),
                found.get_type().to_string(),
            ));
        }
    })
}

fn write_named_type(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty_str: &str,
    value: &BrdbValue,
) -> Result<(), BrdbSchemaError> {
    match (
        schema.intern.get(ty_str),
        schema.get_struct(ty_str),
        schema.get_enum(ty_str),
        value,
    ) {
        (Some(intern_ty), Some(struct_ty), _, BrdbValue::Struct(s)) => {
            if intern_ty != s.name {
                return Err(BrdbSchemaError::ExpectedType(
                    ty_str.to_owned(),
                    schema
                        .intern
                        .lookup(s.name)
                        .unwrap_or_else(|| "unknown struct".to_owned()),
                ));
            }
            write_struct(schema, buf, struct_ty, s)
        }
        (Some(intern_ty), _, Some(enum_ty), BrdbValue::Enum(e)) => {
            if intern_ty != e.name {
                return Err(BrdbSchemaError::ExpectedType(
                    ty_str.to_owned(),
                    schema
                        .intern
                        .lookup(e.name)
                        .unwrap_or_else(|| "unknown enum".to_owned()),
                ));
            }
            write_enum(schema, buf, enum_ty, e)
        }
        _ => {
            return Err(BrdbSchemaError::UnknownType(ty_str.to_owned()));
        }
    }
}

fn write_struct(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &BrdbSchemaStruct,
    value: &BrdbStruct,
) -> Result<(), BrdbSchemaError> {
    // Write the struct properties
    for (k, prop_schema) in ty {
        let prop_val = value.properties.get(k).ok_or_else(|| {
            BrdbSchemaError::MissingStructField(
                schema
                    .intern
                    .lookup(value.name)
                    .unwrap_or_else(|| "unknown struct".to_owned()),
                schema
                    .intern
                    .lookup(*k)
                    .unwrap_or_else(|| "unknown property".to_owned()),
            )
        })?;
        write_struct_property(schema, buf, prop_schema, prop_val)?;
    }
    Ok(())
}

fn write_struct_property(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    prop_schema: &BrdbSchemaStructProperty,
    value: &BrdbValue,
) -> Result<(), BrdbSchemaError> {
    let lookup = |ty: BrdbInterned| {
        schema
            .intern
            .lookup_ref(ty)
            .ok_or(BrdbSchemaError::UnknownStructPropertyType(ty.0.to_string()))
    };

    match prop_schema {
        BrdbSchemaStructProperty::Type(ty) => write_named_type(schema, buf, &lookup(*ty)?, value)?,
        BrdbSchemaStructProperty::Array(ty) => {
            let BrdbValue::Array(arr) = value else {
                return Err(BrdbSchemaError::ExpectedType(
                    "array".to_owned(),
                    value.get_type().to_string(),
                ));
            };
            rmp::encode::write_array_len(buf, arr.len() as u32)?;
            // Write each item in the array
            let item_ty = &lookup(*ty)?;
            for item in arr {
                write_named_type(schema, buf, item_ty, item)?;
            }
        }
        BrdbSchemaStructProperty::FlatArray(ty) => {
            let BrdbValue::FlatArray(arr_data) = value else {
                return Err(BrdbSchemaError::ExpectedType(
                    "flat array".to_owned(),
                    value.get_type().to_string(),
                ));
            };

            // Write the length of the buffer that will be allocated
            let type_size = flat_type_size(schema, &lookup(*ty)?);
            rmp::encode::write_bin_len(buf, (arr_data.len() * type_size) as u32)?;

            let item_ty = &lookup(*ty)?;
            for item in arr_data {
                write_flat_type(schema, buf, item_ty, item)?;
            }
        }
        BrdbSchemaStructProperty::Map(k_ty, v_ty) => {
            let BrdbValue::Map(map) = value else {
                return Err(BrdbSchemaError::ExpectedType(
                    "map".to_owned(),
                    value.get_type().to_string(),
                ));
            };
            // Write the number of items in the map
            rmp::encode::write_map_len(buf, map.len() as u32)?;
            // Write each key-value pair
            for (key, val) in map {
                write_named_type(schema, buf, &lookup(*k_ty)?, key)?;
                write_named_type(schema, buf, &lookup(*v_ty)?, val)?;
            }
        }
    }
    Ok(())
}

fn write_enum(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &BrdbSchemaEnum,
    e: &BrdbEnum,
) -> Result<(), BrdbSchemaError> {
    if e.value >= ty.len() as u64 {
        return Err(BrdbSchemaError::EnumIndexOutOfBounds {
            // Unwrap safety: e.name matches a known enum sourced from the schema
            enum_name: schema.intern.lookup(e.name).unwrap(),
            index: e.value,
        });
    }
    // Write the enum index
    write_uint(buf, e.value)
}

/// Write the smallest possible integer representation of `value` to the buffer.
pub fn write_int(buf: &mut impl Write, value: i64) -> Result<(), BrdbSchemaError> {
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

/// Write the smallest possible unsigned integer representation of `value` to the buffer.
pub fn write_uint(buf: &mut impl Write, value: u64) -> Result<(), BrdbSchemaError> {
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

pub fn write_float32(buf: &mut impl Write, value: f32) -> Result<(), BrdbSchemaError> {
    // Attempt to write as ints on whole numbers less than 8 or 16 bits
    if value.eq(&value.round()) && (value as u16) < u16::MAX && (value as i16) > i16::MIN {
        write_int(buf, value as i64)?;
    } else {
        rmp::encode::write_f32(buf, value)?;
    }
    Ok(())
}

pub fn write_float64(buf: &mut impl Write, value: f64) -> Result<(), BrdbSchemaError> {
    // Attempt to write as ints on whole numbers less than 8, 16, or 32 bits
    if value.eq(&value.round()) && (value as u32) < u32::MAX && (value as i32) > i32::MIN {
        write_int(buf, value as i64)?;
    } else {
        rmp::encode::write_f64(buf, value)?;
    }
    Ok(())
}

fn write_flat_type(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &str,
    value: &BrdbValue,
) -> Result<(), BrdbSchemaError> {
    match (ty, value) {
        ("u8", BrdbValue::U8(v)) => write_flat_u8(buf, *v)?,
        ("u16", BrdbValue::U16(v)) => write_flat_u16(buf, *v)?,
        ("u32", BrdbValue::U32(v)) => write_flat_u32(buf, *v)?,
        ("u64", BrdbValue::U64(v)) => write_flat_u64(buf, *v)?,
        ("i8", BrdbValue::I8(v)) => write_flat_i8(buf, *v)?,
        ("i16", BrdbValue::I16(v)) => write_flat_i16(buf, *v)?,
        ("i32", BrdbValue::I32(v)) => write_flat_i32(buf, *v)?,
        ("i64", BrdbValue::I64(v)) => write_flat_i64(buf, *v)?,
        ("f32", BrdbValue::F32(v)) => write_flat_f32(buf, *v)?,
        ("f64", BrdbValue::F64(v)) => write_flat_f64(buf, *v)?,
        (other, BrdbValue::Struct(s)) => {
            if let Some((intern, s_ty)) = schema
                .intern
                .get(other)
                .and_then(|i| schema.structs.get(&i).map(|s| (i, s)))
            {
                if s.name != intern {
                    return Err(BrdbSchemaError::ExpectedType(
                        other.to_owned(),
                        schema
                            .intern
                            .lookup(s.name)
                            .unwrap_or_else(|| "unknown struct".to_owned()),
                    ));
                }
                write_flat_struct(schema, buf, s_ty, s)?;
            } else {
                return Err(BrdbSchemaError::UnknownType(other.to_owned()));
            }
        }
        (other, _) => return Err(BrdbSchemaError::InvalidFlatType(other.to_owned())),
    }
    Ok(())
}

fn write_flat_struct(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &BrdbSchemaStruct,
    value: &BrdbStruct,
) -> Result<(), BrdbSchemaError> {
    // Write the struct properties
    for (k, prop_schema) in ty {
        let prop_val = value.properties.get(k).ok_or_else(|| {
            BrdbSchemaError::MissingStructField(
                schema
                    .intern
                    .lookup(value.name)
                    .unwrap_or_else(|| "unknown struct".to_owned()),
                schema
                    .intern
                    .lookup(*k)
                    .unwrap_or_else(|| "unknown property".to_owned()),
            )
        })?;
        write_flat_struct_property(schema, buf, prop_schema, prop_val)?;
    }
    Ok(())
}
fn write_flat_struct_property(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    prop_schema: &BrdbSchemaStructProperty,
    value: &BrdbValue,
) -> Result<(), BrdbSchemaError> {
    let lookup = |ty: BrdbInterned| {
        schema
            .intern
            .lookup_ref(ty)
            .ok_or(BrdbSchemaError::UnknownStructPropertyType(ty.0.to_string()))
    };

    match prop_schema {
        BrdbSchemaStructProperty::Type(ty) => write_flat_type(schema, buf, &lookup(*ty)?, value),
        BrdbSchemaStructProperty::Array(ty) => Err(BrdbSchemaError::UnknownType(format!(
            "flat array {}",
            schema
                .intern
                .lookup(*ty)
                .unwrap_or_else(|| format!("unknown ({})", ty.0))
        ))),
        BrdbSchemaStructProperty::FlatArray(ty) => Err(BrdbSchemaError::UnknownType(format!(
            "flat flat array {}",
            schema
                .intern
                .lookup(*ty)
                .unwrap_or_else(|| format!("unknown ({})", ty.0))
        ))),
        BrdbSchemaStructProperty::Map(k_ty, v_ty) => Err(BrdbSchemaError::UnknownType(format!(
            "flat map {},{}",
            schema
                .intern
                .lookup(*k_ty)
                .unwrap_or_else(|| format!("unknown ({})", k_ty.0)),
            schema
                .intern
                .lookup(*v_ty)
                .unwrap_or_else(|| format!("unknown ({})", v_ty.0))
        ))),
    }
}

fn write_flat_u8(buf: &mut impl Write, value: u8) -> Result<(), BrdbSchemaError> {
    buf.write(&[value])?;
    Ok(())
}
fn write_flat_u16(buf: &mut impl Write, value: u16) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
fn write_flat_u32(buf: &mut impl Write, value: u32) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
fn write_flat_u64(buf: &mut impl Write, value: u64) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
fn write_flat_i8(buf: &mut impl Write, value: i8) -> Result<(), BrdbSchemaError> {
    buf.write(&[value as u8])?;
    Ok(())
}
fn write_flat_i16(buf: &mut impl Write, value: i16) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
fn write_flat_i32(buf: &mut impl Write, value: i32) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
fn write_flat_i64(buf: &mut impl Write, value: i64) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}

fn write_flat_f32(buf: &mut impl Write, value: f32) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
fn write_flat_f64(buf: &mut impl Write, value: f64) -> Result<(), BrdbSchemaError> {
    buf.write(&value.to_le_bytes())?;
    Ok(())
}
