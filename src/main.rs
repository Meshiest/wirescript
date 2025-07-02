use clap::Parser;
use clap_stdin::FileOrStdin;
use std::{error::Error, path::PathBuf};

use crate::brdb::Brdb;

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
    #[arg(short, long, group = "output")]
    graph: bool,

    /// Output file for the result
    #[arg(short, long, value_name = "FILE", group = "output")]
    output: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let source = args.file.contents()?;

    let res = match bearilog::parse_and_compile(&source, &args.module, args.inline) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("{e}");
            Err(e)?
        }
    };

    if args.graph {
        println!("{}", bearilog::graphviz::render(&res)?);
    } else if let Some(path) = args.output {
        let world = builder::module_to_world(res)?;
        Brdb::new(path)?.save(format!("create module {}", &args.module), &world)?;
    } else {
        println!("{res}");
    }

    Ok(())
}
