//! `.wsir`, the text form of a [`Program`](crate::ir::Program). The grammar
//! is `FORMAT.md` at the crate root; the parser and writer follow it exactly.

mod base64;
pub(crate) mod defaults;
mod lex;
mod parse;
mod write;

pub use defaults::short_class_names;
pub use parse::{ParseError, parse_program};
pub use write::{check_fixed_point, program_to_string, write_program};

#[cfg(test)]
mod tests;
