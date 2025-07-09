use clap::Parser;
use clap_stdin::FileOrStdin;
use std::{error::Error, path::PathBuf};

use builder::options::LayoutMode;

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
    #[arg(short, long, group = "display")]
    graph: bool,

    /// Output file for the result
    #[arg(short, long, value_name = "FILE", group = "display")]
    output: Option<PathBuf>,

    /// The layout mode to use
    #[arg(short, long, default_value = "layout")]
    layout: LayoutMode,

    /// Options for the layout builder
    #[clap(flatten)]
    layout_options: builder::options::LayoutOptions,
    #[clap(flatten)]
    grid_options: builder::options::GridOptions,
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
        let world = match args.layout {
            LayoutMode::Layout => builder::layout_module_to_world(res, args.layout_options)?,
            LayoutMode::Grid => builder::build_grid(res, args.grid_options),
        };
        if path.exists() {
            eprintln!("File {path:?} already exists, overwriting...");
            std::fs::remove_file(&path)?;
        }
        world.write_brz(path)?;
    } else {
        println!("{res}");
    }

    Ok(())
}
