use std::error::Error;

use lalrpop_util::lalrpop_mod;

use crate::wires::CompiledModule;

mod ast;
mod compiler;
mod wires;

#[cfg(test)]
mod bearilog_tests;
#[cfg(test)]
mod compiler_tests;
mod helpers;

lalrpop_mod!(pub bearilog);

fn main() {
    println!("Hello, world!");
}

pub fn compile_module(source: &'static str, name: &str) -> Result<CompiledModule, Box<dyn Error>> {
    let p = bearilog::ModuleParser::new();
    let parsed_module = p.parse(source)?;
    let mut compiler = compiler::Compiler::new([parsed_module]);
    Ok(compiler.compile(name)?)
}
