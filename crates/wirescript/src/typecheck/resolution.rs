//! What typecheck decided for each method call, index access and loop source,
//! recorded so lowering replays the decision instead of re-deriving it.
//!
//! Keys are the expression's source range plus the concatenated type
//! arguments of every enclosing generic mod/chip body, outermost first
//! (empty outside generics), so each generic instantiation has its own entry.

use crate::collections::HashMap;
use crate::ir::Type;
use std::sync::Arc;

pub use crate::types::container::ContainerKind;

/// `(file, start, end, substitution)`. The substitution is shared, so a key
/// costs no copy of it.
pub type ResolutionKey = (Arc<str>, usize, usize, Arc<[Type]>);

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
            return Some(MethodTarget::Container {
                kind,
                method: name.to_string(),
            });
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
    /// The type of each expression inside a generic body, per instantiation.
    /// The range-keyed `type_of_expr` holds only the last one checked.
    pub generic_expr_types: HashMap<ResolutionKey, Type>,
    /// Every expression range inside a generic body.
    pub generic_ranges: crate::collections::HashSet<(Arc<str>, usize, usize)>,
    /// The type arguments of each call of a generic mod or chip, in its
    /// declared order, keyed by the call and the caller's substitution.
    /// Lowering instantiates the callee with exactly these.
    pub call_type_args: HashMap<ResolutionKey, Vec<Type>>,
    /// Every mod or chip body typecheck checked: the declaration's range and
    /// the body's resolution key. Lowering builds no other.
    pub checked_bodies: crate::collections::HashSet<ResolutionKey>,
    /// The type a module port no site annotates is stored as, the join of
    /// its writes, keyed by each write's value. Only a port some write of
    /// which has another type has entries.
    pub untyped_port_types: HashMap<(Arc<str>, usize, usize), Type>,
    /// Every argument that is a record given to a parameter of one value
    /// (WS003), by range: the parameter is bound to nothing of it.
    pub rejected_record_args: crate::collections::HashSet<(Arc<str>, usize, usize)>,
}
