//! Type utilities (coerce + unify) kept in their own module so typecheck,
//! lower, and layout can depend on the bits they need without pulling
//! all of each other in.

pub mod coerce;
pub mod unify;
