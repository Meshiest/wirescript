//! What typecheck decided for each method call, index access and loop source,
//! recorded so lowering replays the decision instead of re-deriving it.
//!
//! Keys are the expression's source range plus the concatenated type
//! arguments of every enclosing generic mod/chip body, outermost first
//! (empty outside generics), so each generic instantiation has its own entry.

use crate::collections::HashMap;
use crate::ir::Type;
use std::sync::Arc;

/// `(file, start, end, substitution)`.
pub type ResolutionKey = (Arc<str>, usize, usize, Vec<Type>);

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
            Type::Array(e) => Some(ContainerKind::Array { elem: (**e).clone() }),
            Type::Map(k, v) => Some(ContainerKind::Map { key: (**k).clone(), value: (**v).clone() }),
            Type::Ref(inner) => ContainerKind::of(inner),
            _ => None,
        }
    }

    pub fn as_type(&self) -> Type {
        match self {
            ContainerKind::Array { elem } => Type::Array(Box::new(elem.clone())),
            ContainerKind::Map { key, value } => Type::Map(Box::new(key.clone()), Box::new(value.clone())),
        }
    }
}

/// What a `recv.name(args)` call resolved to.
#[derive(Clone, Debug, PartialEq)]
pub enum MethodTarget {
    /// An array or map method on a container receiver.
    Container { kind: ContainerKind, method: String },
    /// A receiver builtin, by catalog name: `entity.SetLocation(p)`.
    Builtin(String),
    /// A user mod/chip whose first parameter is `self`.
    SelfMod(String),
}

/// Container methods first, then receiver builtins, in typecheck's order.
/// A `self`-mod needs a scope lookup, so its arm records it directly.
pub fn classify_method(name: &str, receiver: &Type) -> Option<MethodTarget> {
    if let Some(kind) = ContainerKind::of(receiver) {
        let is_method = match kind {
            ContainerKind::Array { .. } => crate::catalog::arrays::is_array_method(name),
            ContainerKind::Map { .. } => crate::catalog::maps::is_map_method(name),
        };
        if is_method {
            return Some(MethodTarget::Container { kind, method: name.to_string() });
        }
    }
    crate::catalog::calls::find_call(name)
        .filter(|c| c.receiver.is_some())
        .map(|_| MethodTarget::Builtin(name.to_string()))
}

#[derive(Clone, Debug, Default)]
pub struct Resolutions {
    pub method_targets: HashMap<ResolutionKey, MethodTarget>,
    pub container_kinds: HashMap<ResolutionKey, ContainerKind>,
}
