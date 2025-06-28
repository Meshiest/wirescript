use clap::Parser;
use clap_stdin::FileOrStdin;
use std::error::Error;

pub mod bearilog;
pub mod brdb;
pub mod builder;

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
    let modules = bearilog::grammar::ModulesParser::new()
        .parse(&source)
        .map_err(|e| e.to_string())?;
    let mut compiler = bearilog::compiler::Compiler::new(modules);

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
        println!("{}", bearilog::graphviz::render(&res)?);
    } else {
        println!("{res}");
    }

    Ok(())
}
