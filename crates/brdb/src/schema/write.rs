use std::io::Write;

use crate::{
    errors::BrdbSchemaError,
    schema::{
        BrdbEnum, BrdbInterned, BrdbSchema, BrdbSchemaEnum, BrdbSchemaStruct,
        BrdbSchemaStructProperty, BrdbStruct, BrdbValue, WireVariant, as_brdb::AsBrdbValue,
        read::flat_type_size,
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
        ("wire_graph_variant", BrdbValue::WireVar(v)) => write_wire_var(buf, v)?,
        ("wire_graph_prim_math_variant", BrdbValue::WireVar(v)) => match v {
            WireVariant::Number(v) => {
                write_uint(buf, 0)?;
                write_float64(buf, *v)?;
            }
            WireVariant::Int(v) => {
                write_uint(buf, 1)?;
                write_int(buf, *v)?;
            }
            other => {
                return Err(BrdbSchemaError::ExpectedType(
                    "wire_graph_prim_math_variant".to_owned(),
                    other.to_string(),
                ));
            }
        },

        ("class" | "object" | _, BrdbValue::Asset(None)) => {
            // None is -1
            write_int(buf, -1)?;
        }
        ("class" | "object" | _, BrdbValue::Asset(Some(s))) => {
            if let Some((asset_ty, _)) = schema.global_data.external_asset_references.get_index(*s)
            {
                if asset_ty != ty {
                    return Err(BrdbSchemaError::UnknownAsset(ty.to_owned(), *s));
                }
                // Assets are stored as u64 indices
                write_uint(buf, *s as u64)?;
            } else {
                return Err(BrdbSchemaError::UnknownAsset(ty.to_owned(), *s));
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

fn write_wire_var(buf: &mut impl Write, v: &WireVariant) -> Result<(), BrdbSchemaError> {
    match v {
        WireVariant::Number(v) => {
            write_uint(buf, 0)?;
            write_float64(buf, *v)?;
        }
        WireVariant::Int(v) => {
            write_uint(buf, 1)?;
            write_int(buf, *v)?;
        }
        WireVariant::Bool(v) => {
            write_uint(buf, 2)?;
            write_bool(buf, *v)?;
        }
        WireVariant::Object(_) => {
            write_uint(buf, 3)?;
            // nothing to write atm?
        }
        WireVariant::Exec => {
            write_uint(buf, 4)?;
        }
    }
    Ok(())
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
                value
                    .name
                    .get_or_else(schema, || "unknown struct".to_owned()),
                k.get_or_else(schema, || "unknown property".to_owned()),
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
        ty.get_ok(schema, || {
            BrdbSchemaError::UnknownStructPropertyType(ty.0.to_string())
        })
    };

    match (prop_schema, value) {
        (BrdbSchemaStructProperty::Type(ty), value) => {
            write_named_type(schema, buf, &lookup(*ty)?, value)?
        }
        (BrdbSchemaStructProperty::Array(ty), BrdbValue::Array(arr)) => {
            rmp::encode::write_array_len(buf, arr.len() as u32)?;
            // Write each item in the array
            let item_ty = &lookup(*ty)?;
            for item in arr {
                write_named_type(schema, buf, item_ty, item)?;
            }
        }
        (BrdbSchemaStructProperty::FlatArray(ty), BrdbValue::FlatArray(arr_data)) => {
            // Write the length of the buffer that will be allocated
            let type_size = flat_type_size(schema, &lookup(*ty)?);
            rmp::encode::write_bin_len(buf, (arr_data.len() * type_size) as u32)?;

            let item_ty = &lookup(*ty)?;
            for item in arr_data {
                write_flat_type(schema, buf, item_ty, item)?;
            }
        }
        (BrdbSchemaStructProperty::Map(k_ty, v_ty), BrdbValue::Map(map)) => {
            // Write the number of items in the map
            rmp::encode::write_map_len(buf, map.len() as u32)?;
            // Write each key-value pair
            for (key, val) in map {
                write_named_type(schema, buf, &lookup(*k_ty)?, key)?;
                write_named_type(schema, buf, &lookup(*v_ty)?, val)?;
            }
        }
        (ty, val) => {
            return Err(BrdbSchemaError::ExpectedType(
                ty.as_string(schema),
                val.get_type().to_string(),
            ));
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

                for (k, prop_schema) in s_ty {
                    let prop_val = s.properties.get(k).ok_or_else(|| {
                        BrdbSchemaError::MissingStructField(
                            schema
                                .intern
                                .lookup(s.name)
                                .unwrap_or_else(|| "unknown struct".to_owned()),
                            schema
                                .intern
                                .lookup(*k)
                                .unwrap_or_else(|| "unknown property".to_owned()),
                        )
                    })?;

                    // Flat types can only write properties of type `Type`
                    match prop_schema {
                        BrdbSchemaStructProperty::Type(ty) => write_flat_type(
                            schema,
                            buf,
                            schema.intern.lookup_ref(*ty).ok_or(
                                BrdbSchemaError::UnknownStructPropertyType(ty.0.to_string()),
                            )?,
                            prop_val,
                        )?,
                        other => {
                            return Err(BrdbSchemaError::InvalidFlatType(other.as_string(schema)));
                        }
                    }
                }
            } else {
                return Err(BrdbSchemaError::UnknownType(other.to_owned()));
            }
        }
        (other, _) => return Err(BrdbSchemaError::InvalidFlatType(other.to_owned())),
    }
    Ok(())
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

pub fn write_brdb(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &str,
    value: &dyn AsBrdbValue,
) -> Result<(), BrdbSchemaError> {
    let lookup = |ty: BrdbInterned| {
        schema
            .intern
            .lookup_ref(ty)
            .ok_or(BrdbSchemaError::UnknownType(ty.0.to_string()))
    };

    match ty {
        "bool" => write_bool(buf, value.as_brdb_bool()?)?,
        "u8" => write_uint(buf, value.as_brdb_u8()? as u64)?,
        "u16" => write_uint(buf, value.as_brdb_u16()? as u64)?,
        "u32" => write_uint(buf, value.as_brdb_u32()? as u64)?,
        "u64" => write_uint(buf, value.as_brdb_u64()?)?,
        "i8" => write_int(buf, value.as_brdb_i8()? as i64)?,
        "i16" => write_int(buf, value.as_brdb_i16()? as i64)?,
        "i32" => write_int(buf, value.as_brdb_i32()? as i64)?,
        "i64" => write_int(buf, value.as_brdb_i64()?)?,
        "f32" => write_float32(buf, value.as_brdb_f32()?)?,
        "f64" => write_float64(buf, value.as_brdb_f64()?)?,
        "str" => write_str(buf, value.as_brdb_str()?)?,
        "wire_graph_variant" => write_wire_var(buf, &value.as_brdb_wire_variant()?)?,
        "wire_graph_prim_math_variant" => match value.as_brdb_wire_variant()? {
            WireVariant::Number(v) => {
                write_uint(buf, 0)?;
                write_float64(buf, v)?;
            }
            WireVariant::Int(v) => {
                write_uint(buf, 1)?;
                write_int(buf, v)?;
            }
            other => {
                return Err(BrdbSchemaError::ExpectedType(
                    "wire_graph_prim_math_variant".to_owned(),
                    other.to_string(),
                ));
            }
        },
        "class" | "object" => {
            let asset_index = value.as_brdb_asset(schema, ty)?;
            if let Some(asset_index) = asset_index {
                write_uint(buf, asset_index as u64)?;
            } else {
                write_int(buf, -1)?;
            }
        }
        other => {
            if let (Some(s_id), Some(s_ty)) = (schema.intern.get(other), schema.get_struct(other)) {
                for (prop_id, prop_schema) in s_ty {
                    match prop_schema {
                        BrdbSchemaStructProperty::Type(ty_id) => {
                            let prop_value =
                                value.as_brdb_struct_prop_value(schema, s_id, *prop_id)?;
                            write_brdb(schema, buf, &lookup(*ty_id)?, &*prop_value)?;
                        }
                        BrdbSchemaStructProperty::Array(ty_id) => {
                            let ty = &lookup(*ty_id)?;
                            let prop_values =
                                value.as_brdb_struct_prop_array(schema, s_id, *prop_id)?;
                            rmp::encode::write_array_len(buf, prop_values.len() as u32)?;
                            for prop_value in prop_values {
                                write_brdb(schema, buf, ty, &*prop_value)?;
                            }
                        }
                        BrdbSchemaStructProperty::FlatArray(ty_id) => {
                            let ty = &lookup(*ty_id)?;
                            let prop_values =
                                value.as_brdb_struct_prop_array(schema, s_id, *prop_id)?;
                            // Write the length of the buffer that will be allocated
                            let type_size = flat_type_size(schema, ty);
                            rmp::encode::write_bin_len(
                                buf,
                                (prop_values.len() * type_size) as u32,
                            )?;
                            for prop_value in prop_values {
                                write_brdb_flat(schema, buf, ty, &*prop_value)?;
                            }
                        }
                        BrdbSchemaStructProperty::Map(k_ty_id, v_ty_id) => {
                            let k_ty = &lookup(*k_ty_id)?;
                            let v_ty = &lookup(*v_ty_id)?;
                            let prop_values =
                                value.as_brdb_struct_prop_map(schema, s_id, *prop_id)?;
                            rmp::encode::write_map_len(buf, prop_values.len() as u32)?;
                            for (key, val) in prop_values {
                                write_brdb(schema, buf, k_ty, &*key)?;
                                write_brdb(schema, buf, v_ty, &*val)?;
                            }
                        }
                    }
                }
            } else if let Some(enum_ty) = schema.get_enum(other) {
                let enum_value = value.as_brdb_enum(schema, enum_ty)?;
                if enum_value >= enum_ty.len() as i32 {
                    return Err(BrdbSchemaError::EnumIndexOutOfBounds {
                        enum_name: other.to_owned(),
                        index: enum_value as u64,
                    });
                }
                write_uint(buf, enum_value as u64)?;
            } else {
                return Err(BrdbSchemaError::UnknownType(other.to_owned()));
            }
        }
    }
    Ok(())
}

fn write_brdb_flat(
    schema: &BrdbSchema,
    buf: &mut impl Write,
    ty: &str,
    value: &dyn AsBrdbValue,
) -> Result<(), BrdbSchemaError> {
    match ty {
        "u8" => write_flat_u8(buf, value.as_brdb_u8()?)?,
        "u16" => write_flat_u16(buf, value.as_brdb_u16()?)?,
        "u32" => write_flat_u32(buf, value.as_brdb_u32()?)?,
        "u64" => write_flat_u64(buf, value.as_brdb_u64()?)?,
        "i8" => write_flat_i8(buf, value.as_brdb_i8()?)?,
        "i16" => write_flat_i16(buf, value.as_brdb_i16()?)?,
        "i32" => write_flat_i32(buf, value.as_brdb_i32()?)?,
        "i64" => write_flat_i64(buf, value.as_brdb_i64()?)?,
        "f32" => write_flat_f32(buf, value.as_brdb_f32()?)?,
        "f64" => write_flat_f64(buf, value.as_brdb_f64()?)?,
        other => {
            let (Some(s_id), Some(s_ty)) = (schema.intern.get(other), schema.get_struct(other))
            else {
                return Err(BrdbSchemaError::InvalidFlatType(other.to_owned()));
            };
            for (prop_id, prop_schema) in s_ty {
                let BrdbSchemaStructProperty::Type(prop_ty) = prop_schema else {
                    return Err(BrdbSchemaError::InvalidFlatType(format!(
                        "flat {other} struct property"
                    )));
                };
                let prop_ty = prop_ty.get_ok(schema, || {
                    BrdbSchemaError::UnknownStructPropertyType(prop_ty.0.to_string())
                })?;
                let prop_val = value.as_brdb_struct_prop_value(schema, s_id, *prop_id)?;
                write_brdb_flat(schema, buf, prop_ty, &*prop_val)?;
            }
        }
    }
    Ok(())
}
