use std::error::Error;

use lalrpop_util::lalrpop_mod;

use crate::{
    ast::AstModule,
    compiler::{CompiledModule, Compiler},
};

#[cfg(test)]
mod bearilog_tests;
#[cfg(test)]
mod compiler_tests;

pub mod ast;
pub mod compiler;
pub mod graphviz;
pub(crate) mod helpers;

lalrpop_mod!(pub grammar);

pub fn parse_modules(source: &str) -> Result<Vec<AstModule>, String> {
    let parser = grammar::ModulesParser::new();
    parser.parse(&source).map_err(|e| e.to_string())
}

pub fn parse_and_compile(
    source: &str,
    module: &str,
    inline: bool,
) -> Result<CompiledModule, Box<dyn Error>> {
    let modules = parse_modules(source)?;
    let mut compiler = Compiler::new(modules);

    Ok(compiler.compile_opts(module, false, inline)?)
}

pub fn compile_module(source: &str, name: &str) -> Result<CompiledModule, Box<dyn Error>> {
    let p = grammar::ModuleParser::new();
    let parsed_module = p.parse(source).map_err(|e| e.to_string())?;
    let mut compiler = Compiler::new([parsed_module]);
    Ok(compiler.compile(name)?)
}
