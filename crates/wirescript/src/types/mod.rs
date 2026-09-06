//! Type utilities (coercion, inference, monomorphisation) kept in their own
//! module so typecheck, lower, and layout can depend on the bits they need
//! without pulling all of each other in.

pub mod classes;
pub mod coerce;
pub mod infer;
pub mod mono;
pub mod resolve;
