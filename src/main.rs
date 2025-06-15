use std::error::Error;

use clap::Parser;
use clap_stdin::FileOrStdin;
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

/// Program to parse bearilog to gates
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the module (or stdin)
    file: FileOrStdin,

    /// The module to parse
    module: String,

    /// Force all modules to be inlined
    #[arg(short, long)]
    inline: bool,

    /// Generate a graphviz visual
    #[arg(short, long)]
    graph: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let source = args.file.contents()?;
    let modules = bearilog::ModulesParser::new()
        .parse(&source)
        .map_err(|e| e.to_string())?;
    let mut compiler = compiler::Compiler::new(modules);

    if args.inline {
        compiler.set_inline();
    }

    let res = match compiler.compile(&args.module) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("{e}");
            Err(e)?
        }
    };

    if args.graph {
        println!("{}", res.graphviz()?);
    } else {
        println!("{res}");
    }

    Ok(())
}

pub fn compile_module(source: &str, name: &str) -> Result<CompiledModule, Box<dyn Error>> {
    let p = bearilog::ModuleParser::new();
    let parsed_module = p.parse(source).map_err(|e| e.to_string())?;
    let mut compiler = compiler::Compiler::new([parsed_module]);
    Ok(compiler.compile(name)?)
}
