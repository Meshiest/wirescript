//! Type utilities (coercion, inference, monomorphisation) kept in their own
//! module so typecheck, lower, and layout can depend on the bits they need
//! without pulling all of each other in.

pub(crate) mod classes;
pub(crate) mod coerce;
pub(crate) mod container;
pub(crate) mod infer;
pub(crate) mod mono;
pub(crate) mod resolve;
pub(crate) mod signature;
