use bearilog::compiler::CompiledModule;
use std::error::Error;

pub mod bearilog;
pub mod brdb;

pub fn compile_module(source: &str, name: &str) -> Result<CompiledModule, Box<dyn Error>> {
    let p = bearilog::grammar::ModuleParser::new();
    let parsed_module = p.parse(source).map_err(|e| e.to_string())?;
    let mut compiler = bearilog::compiler::Compiler::new([parsed_module]);
    Ok(compiler.compile(name)?)
}
