use indexmap::{IndexMap, IndexSet};

use crate::{
    errors::BrdbSchemaError,
    schema::{BrdbInterned, BrdbSchema, BrdbSchemaEnum, BrdbValue, WireVariant},
};

pub type BrdbArrayIter<'a> = Box<dyn ExactSizeIterator<Item = &'a dyn AsBrdbValue> + 'a>;
pub type BrdbMapIter<'a> =
    Box<dyn ExactSizeIterator<Item = (&'a dyn AsBrdbValue, &'a dyn AsBrdbValue)> + 'a>;

/// A helper trait to allow serializing implementing types to msgpack schema format
pub trait AsBrdbValue {
    fn as_brdb_bool(&self) -> Result<bool, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "bool".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_u8(&self) -> Result<u8, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "u8".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_u16(&self) -> Result<u16, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "u16".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_u32(&self) -> Result<u32, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "u32".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_u64(&self) -> Result<u64, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "u64".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_i8(&self) -> Result<i8, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "i8".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_i16(&self) -> Result<i16, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "i16".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_i32(&self) -> Result<i32, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "i32".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_i64(&self) -> Result<i64, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "i64".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_f32(&self) -> Result<f32, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "f32".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_f64(&self) -> Result<f64, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "f64".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_str(&self) -> Result<&str, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "str".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_asset(
        &self,
        _schema: &BrdbSchema,
        _ty: &str,
    ) -> Result<Option<usize>, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "asset".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
    fn as_brdb_enum(
        &self,
        _schema: &BrdbSchema,
        _def: &BrdbSchemaEnum,
    ) -> Result<i32, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "enum".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }

    fn as_brdb_wire_variant(&self) -> Result<crate::schema::value::WireVariant, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "wire variant".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }

    /// Read a specific struct property value from the schema.
    fn as_brdb_struct_prop_value(
        &self,
        _schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        _prop_name: BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "struct property".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }

    /// Get the the number of entries in a struct property.
    fn as_brdb_struct_prop_array(
        &self,
        _schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        _prop_name: BrdbInterned,
    ) -> Result<BrdbArrayIter, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "struct property array".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }

    /// Get the the number of entries in a struct property.
    fn as_brdb_struct_prop_map(
        &self,
        _schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        _prop_name: BrdbInterned,
    ) -> Result<BrdbMapIter, BrdbSchemaError> {
        Err(BrdbSchemaError::UnimplementedCast(
            "struct property map".to_owned(),
            std::any::type_name::<Self>(),
        ))
    }
}

impl AsBrdbValue for () {}

macro_rules! as_brdb_fn {
    ($fn_name:ident, $ty:ty, $method:ident) => {
        fn $fn_name(&self) -> Result<$ty, BrdbSchemaError> {
            if let BrdbValue::$method(v) = self {
                Ok(*v as $ty)
            } else {
                Err(BrdbSchemaError::ExpectedType(
                    stringify!($ty).to_owned(),
                    self.get_type().to_string(),
                ))
            }
        }
    };
}

/// A default impl for `BrdbValue`.
impl AsBrdbValue for BrdbValue {
    as_brdb_fn!(as_brdb_bool, bool, Bool);
    as_brdb_fn!(as_brdb_u8, u8, U8);
    as_brdb_fn!(as_brdb_u16, u16, U16);
    as_brdb_fn!(as_brdb_u32, u32, U32);
    as_brdb_fn!(as_brdb_u64, u64, U64);
    as_brdb_fn!(as_brdb_i8, i8, I8);
    as_brdb_fn!(as_brdb_i16, i16, I16);
    as_brdb_fn!(as_brdb_i32, i32, I32);
    as_brdb_fn!(as_brdb_i64, i64, I64);
    as_brdb_fn!(as_brdb_f32, f32, F32);
    as_brdb_fn!(as_brdb_f64, f64, F64);
    fn as_brdb_str(&self) -> Result<&str, BrdbSchemaError> {
        if let BrdbValue::String(s) = self {
            Ok(s)
        } else {
            Err(BrdbSchemaError::ExpectedType(
                "str".to_owned(),
                self.get_type().to_string(),
            ))
        }
    }
    fn as_brdb_asset(
        &self,
        _schema: &BrdbSchema,
        _ty: &str,
    ) -> Result<Option<usize>, BrdbSchemaError> {
        if let BrdbValue::Asset(index) = self {
            Ok(*index)
        } else {
            Err(BrdbSchemaError::ExpectedType(
                "asset".to_owned(),
                self.get_type().to_string(),
            ))
        }
    }
    fn as_brdb_enum(
        &self,
        _schema: &BrdbSchema,
        _def: &BrdbSchemaEnum,
    ) -> Result<i32, BrdbSchemaError> {
        if let BrdbValue::Enum(e) = self {
            Ok(e.value as i32)
        } else {
            Err(BrdbSchemaError::ExpectedType(
                "enum".to_owned(),
                self.get_type().to_string(),
            ))
        }
    }
    fn as_brdb_wire_variant(&self) -> Result<WireVariant, BrdbSchemaError> {
        if let BrdbValue::WireVar(wire) = self {
            Ok(wire.to_owned())
        } else {
            Err(BrdbSchemaError::ExpectedType(
                "wire variant".to_owned(),
                self.get_type().to_string(),
            ))
        }
    }
    fn as_brdb_struct_prop_value(
        &self,
        schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, BrdbSchemaError> {
        let BrdbValue::Struct(s) = self else {
            return Err(BrdbSchemaError::ExpectedType(
                "struct".to_owned(),
                self.get_type().to_string(),
            ));
        };

        if let Some(prop) = s.properties.get(&prop_name) {
            Ok(prop)
        } else {
            Err(BrdbSchemaError::MissingStructField(
                schema
                    .intern
                    .lookup(s.name)
                    .unwrap_or_else(|| "unknown struct".to_owned()),
                schema
                    .intern
                    .lookup(prop_name)
                    .unwrap_or_else(|| "unknown property".to_owned()),
            ))
        }
    }
    fn as_brdb_struct_prop_array(
        &self,
        schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> Result<BrdbArrayIter, BrdbSchemaError> {
        let BrdbValue::Struct(s) = self else {
            return Err(BrdbSchemaError::ExpectedType(
                "struct".to_owned(),
                self.get_type().to_string(),
            ));
        };
        match s.properties.get(&prop_name) {
            Some(BrdbValue::Array(vec)) | Some(BrdbValue::FlatArray(vec)) => Ok(vec.as_brdb_iter()),
            _ => Err(BrdbSchemaError::MissingStructField(
                schema
                    .intern
                    .lookup(s.name)
                    .unwrap_or_else(|| "unknown struct".to_owned()),
                schema
                    .intern
                    .lookup(prop_name)
                    .unwrap_or_else(|| "unknown property".to_owned()),
            )),
        }
    }
    fn as_brdb_struct_prop_map(
        &self,
        schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> Result<BrdbMapIter, BrdbSchemaError> {
        let BrdbValue::Struct(s) = self else {
            return Err(BrdbSchemaError::ExpectedType(
                "struct".to_owned(),
                self.get_type().to_string(),
            ));
        };
        if let Some(BrdbValue::Map(map)) = s.properties.get(&prop_name) {
            Ok(Box::new(map.iter().map(|(k, v)| {
                (k as &dyn AsBrdbValue, v as &dyn AsBrdbValue)
            })))
        } else {
            Err(BrdbSchemaError::MissingStructField(
                schema
                    .intern
                    .lookup(s.name)
                    .unwrap_or_else(|| "unknown struct".to_owned()),
                schema
                    .intern
                    .lookup(prop_name)
                    .unwrap_or_else(|| "unknown property".to_owned()),
            ))
        }
    }
}
impl AsBrdbValue for WireVariant {
    fn as_brdb_wire_variant(&self) -> Result<WireVariant, BrdbSchemaError> {
        Ok(self.clone())
    }
}

macro_rules! as_brdb_int(
    ($ty:ty, $($rest:ty),*) => {
        as_brdb_int!($ty);
        as_brdb_int!($($rest),*);
    };
    ($ty:ty) => {
        impl AsBrdbValue for $ty {
            fn as_brdb_bool(&self) -> Result<bool, BrdbSchemaError> {
                Ok(*self != 0)
            }
            fn as_brdb_u8(&self) -> Result<u8, BrdbSchemaError> {
                Ok(*self as u8)
            }
            fn as_brdb_u16(&self) -> Result<u16, BrdbSchemaError> {
                Ok(*self as u16)
            }
            fn as_brdb_u32(&self) -> Result<u32, BrdbSchemaError> {
                Ok(*self as u32)
            }
            fn as_brdb_u64(&self) -> Result<u64, BrdbSchemaError> {
                Ok(*self as u64)
            }
            fn as_brdb_i8(&self) -> Result<i8, BrdbSchemaError> {
                Ok(*self as i8)
            }
            fn as_brdb_i16(&self) -> Result<i16, BrdbSchemaError> {
                Ok(*self as i16)
            }
            fn as_brdb_i32(&self) -> Result<i32, BrdbSchemaError> {
                Ok(*self as i32)
            }
            fn as_brdb_i64(&self) -> Result<i64, BrdbSchemaError> {
                Ok(*self as i64)
            }
            fn as_brdb_f32(&self) -> Result<f32, BrdbSchemaError> {
                Ok(*self as f32)
            }
            fn as_brdb_f64(&self) -> Result<f64, BrdbSchemaError> {
                Ok(*self as f64)
            }
            fn as_brdb_wire_variant(
                &self,
            ) -> Result<WireVariant, BrdbSchemaError> {
                Ok((*self).into())
            }
        }
    }
);
as_brdb_int!(u8, u16, u32, u64, i8, i16, i32, i64);

macro_rules! as_brdb_float {
    ($ty:ty, $($rest:ty),*) => {
        as_brdb_float!($ty);
        as_brdb_float!($($rest),*);
    };
    ($ty:ty) => {
        impl AsBrdbValue for $ty {
            fn as_brdb_f32(&self) -> Result<f32, BrdbSchemaError> {
                Ok(*self as f32)
            }
            fn as_brdb_f64(&self) -> Result<f64, BrdbSchemaError> {
                Ok(*self as f64)
            }
            fn as_brdb_wire_variant(
                &self,
            ) -> Result<WireVariant, BrdbSchemaError> {
                Ok(WireVariant::Number(*self as f64))
            }
        }
    };
}
as_brdb_float!(f32, f64);

impl AsBrdbValue for bool {
    fn as_brdb_bool(&self) -> Result<bool, BrdbSchemaError> {
        Ok(*self)
    }
    fn as_brdb_wire_variant(&self) -> Result<crate::schema::value::WireVariant, BrdbSchemaError> {
        Ok(WireVariant::Bool(*self))
    }
}
impl AsBrdbValue for String {
    fn as_brdb_str(&self) -> Result<&str, BrdbSchemaError> {
        Ok(self)
    }
}
impl AsBrdbValue for &str {
    fn as_brdb_str(&self) -> Result<&str, BrdbSchemaError> {
        Ok(self)
    }
}

pub trait AsBrdbIter {
    fn as_brdb_iter(&self) -> BrdbArrayIter;
}
pub trait AsBrdbMapIter {
    fn as_brdb_map_iter(&self) -> BrdbMapIter;
}

impl<T: AsBrdbValue> AsBrdbIter for Vec<T> {
    fn as_brdb_iter(&self) -> BrdbArrayIter {
        Box::new(self.iter().map(|v| v as &dyn AsBrdbValue))
    }
}
impl<T: AsBrdbValue> AsBrdbIter for IndexSet<T> {
    fn as_brdb_iter(&self) -> BrdbArrayIter {
        Box::new(self.iter().map(|v| v as &dyn AsBrdbValue))
    }
}
impl<K: AsBrdbValue, V: AsBrdbValue> AsBrdbMapIter for IndexMap<K, V> {
    fn as_brdb_map_iter(&self) -> BrdbMapIter {
        Box::new(
            self.iter()
                .map(|(k, v)| (k as &dyn AsBrdbValue, v as &dyn AsBrdbValue)),
        )
    }
}

// Automatically implement `AsBrdbValue` for tuples of any number of elements
// and treat them as struct properties.
macro_rules! as_brdb_tuple {
    ($($name:ident $index:tt),*) => {
        impl<$($name: AsBrdbValue),*> AsBrdbValue for ($($name),*) {
            fn as_brdb_struct_prop_value(
                &self,
                schema: &BrdbSchema,
                struct_name: BrdbInterned,
                prop_name: BrdbInterned,
            ) -> Result<&dyn AsBrdbValue, BrdbSchemaError> {
                let s_ty = schema.get_struct_interned(struct_name).unwrap();
                let prop_index = s_ty.get_index_of(&prop_name);
                match prop_index {
                    $(
                        Some($index) => Ok(&self.$index),
                    )*
                    _ => Err(BrdbSchemaError::MissingStructField(
                        struct_name.get_or_else(schema, || "unknown struct".to_owned()),
                        prop_name.get_or_else(schema, || "unknown property".to_owned()),
                    )),
                }
            }
        }
    };
}

as_brdb_tuple!(A 0, B 1);
as_brdb_tuple!(A 0, B 1, C 2);
as_brdb_tuple!(A 0, B 1, C 2, D 3);
as_brdb_tuple!(A 0, B 1, C 2, D 3, E 4);
