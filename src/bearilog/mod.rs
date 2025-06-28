#[cfg(test)]
mod bearilog_tests;
#[cfg(test)]
mod compiler_tests;

pub mod ast;
pub mod compiler;
pub mod graphviz;
pub(crate) mod helpers;

pub mod grammar {
    use lalrpop_util::lalrpop_mod;
    lalrpop_mod!(bearilog_modules);
    pub use bearilog_modules::*;
}
