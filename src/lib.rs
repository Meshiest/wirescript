use std::error::Error;

use lalrpop_util::lalrpop_mod;

use crate::compiler::CompiledModule;

pub mod ast;
pub mod brdb;
pub mod compiler;
pub mod graphviz;

lalrpop_mod!(pub bearilog);

#[cfg(test)]
mod bearilog_tests;
#[cfg(test)]
mod compiler_tests;
pub mod helpers;

pub fn compile_module(source: &str, name: &str) -> Result<CompiledModule, Box<dyn Error>> {
    let p = bearilog::ModuleParser::new();
    let parsed_module = p.parse(source).map_err(|e| e.to_string())?;
    let mut compiler = compiler::Compiler::new([parsed_module]);
    Ok(compiler.compile(name)?)
}
