use std::error::Error;

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

pub fn parse_and_compile(
    source: &str,
    module: &str,
    inline: bool,
) -> Result<compiler::CompiledModule, Box<dyn Error>> {
    let modules = grammar::ModulesParser::new()
        .parse(&source)
        .map_err(|e| e.to_string())?;
    let mut compiler = compiler::Compiler::new(modules);

    if inline {
        compiler.set_inline();
    }

    Ok(compiler.compile(module)?)
}
