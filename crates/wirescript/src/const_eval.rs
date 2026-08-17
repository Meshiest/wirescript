//! Compile-time evaluation, shared by typecheck and lowering.
//!
//! Both must agree exactly: `const if` drops a branch during lowering, and the
//! typechecker must skip type-checking the SAME branch. Two evaluators would
//! make that a silent miscompile, so there is one, here.

mod destructure;
mod error;
mod expr;
mod interp;

pub(crate) use destructure::{bind_destructured, bound_names};
pub use error::{ConstError, ConstReason};
pub use expr::{eval_expr, ConstCtx};
pub use interp::{eval_call, Budget};

#[cfg(test)]
mod tests;
