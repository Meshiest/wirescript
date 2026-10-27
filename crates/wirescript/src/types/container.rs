//! The two kinds of container a value can be: an array or a map.

use crate::ir::Type;

/// What a container-valued expression is.
#[derive(Clone, Debug, PartialEq)]
pub enum ContainerKind {
    Array { elem: Type },
    Map { key: Type, value: Type },
}

impl ContainerKind {
    /// The kind of a container type, looking through a ref wrapper.
    pub fn of(t: &Type) -> Option<Self> {
        match t {
            Type::Array(e) => Some(Self::Array {
                elem: (**e).clone(),
            }),
            Type::Map(k, v) => Some(Self::Map {
                key: (**k).clone(),
                value: (**v).clone(),
            }),
            Type::Ref(inner) => Self::of(inner),
            _ => None,
        }
    }

    pub fn as_type(&self) -> Type {
        match self {
            Self::Array { elem } => Type::Array(Box::new(elem.clone())),
            Self::Map { key, value } => Type::Map(Box::new(key.clone()), Box::new(value.clone())),
        }
    }
}
