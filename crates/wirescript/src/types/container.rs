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
    pub fn of(t: &Type) -> Option<ContainerKind> {
        match t {
            Type::Array(e) => Some(ContainerKind::Array {
                elem: (**e).clone(),
            }),
            Type::Map(k, v) => Some(ContainerKind::Map {
                key: (**k).clone(),
                value: (**v).clone(),
            }),
            Type::Ref(inner) => ContainerKind::of(inner),
            _ => None,
        }
    }

    pub fn as_type(&self) -> Type {
        match self {
            ContainerKind::Array { elem } => Type::Array(Box::new(elem.clone())),
            ContainerKind::Map { key, value } => {
                Type::Map(Box::new(key.clone()), Box::new(value.clone()))
            }
        }
    }
}
