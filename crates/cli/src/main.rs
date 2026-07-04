use clap::{Parser, Subcommand};
use std::{error::Error, path::PathBuf};

/// Logic-brick toolchain: compile wirescript source to Brickadia saves.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Compile a wirescript source file (.ws) to a .brdb or .brz save.
    Compile {
        /// Wirescript source file (or - for stdin).
        source: PathBuf,

        /// Output path. Extension determines format:
        /// `.brdb` — SQLite, loadable via `BR.World.LoadAdditive`.
        /// `.brz` — zstd bundle (default if no extension given → .brdb).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Override the module name embedded in the save.
        #[arg(long)]
        name: Option<String>,

        /// Print diagnostics even when successful.
        #[arg(long)]
        verbose: bool,

        /// World-grid X position of the outer microchip brick.
        #[arg(long, default_value_t = 0)]
        x: i32,

        /// World-grid Y position of the outer microchip brick.
        #[arg(long, default_value_t = 0)]
        y: i32,

        /// World-grid Z position of the outer microchip brick.
        #[arg(long, default_value_t = 0)]
        z: i32,

        /// Emit all microchips uncollapsed (expanded).
        #[arg(long)]
        open: bool,

        /// Dump the lowered IR to stderr before emitting.
        #[arg(long)]
        dump_ir: bool,
    },

    /// Legacy bearilog gate-language (kept for backward-compat).
    Bearilog {
        file: clap_stdin::FileOrStdin,
        module: String,
        #[arg(short, long)]
        inline: bool,
        #[arg(short, long, group = "display")]
        graph: bool,
        #[arg(short, long, value_name = "FILE", group = "display")]
        output: Option<PathBuf>,
        #[arg(short, long, default_value = "layout")]
        layout: builder::options::LayoutMode,
        #[clap(flatten)]
        layout_options: builder::options::LayoutOptions,
        #[clap(flatten)]
        grid_options: builder::options::GridOptions,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile {
            source,
            output,
            name,
            verbose,
            x,
            y,
            z,
            open,
            dump_ir,
        } => run_wirescript(source, output, name, verbose, x, y, z, open, dump_ir),
        Command::Bearilog {
            file,
            module,
            inline,
            graph,
            output,
            layout,
            layout_options,
            grid_options,
        } => run_bearilog(
            file,
            module,
            inline,
            graph,
            output,
            layout,
            layout_options,
            grid_options,
        ),
    }
}

// ---------- wirescript compile ----------

fn run_wirescript(
    source: PathBuf,
    output: Option<PathBuf>,
    module_name: Option<String>,
    verbose: bool,
    x: i32,
    y: i32,
    z: i32,
    open: bool,
    dump_ir: bool,
) -> Result<(), Box<dyn Error>> {
    let src = std::fs::read_to_string(&source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let file = source.to_string_lossy().to_string();

    if dump_ir {
        let resolved = wirescript::resolve(&src, &file, &wirescript::FsLoader);
        let tc = wirescript::typecheck::typecheck(&resolved.ast, &file);
        let lowered = wirescript::lower::lower(wirescript::lower::LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: &file,
            module_name: module_name.as_deref(),
            template_cache: std::sync::Arc::new(wirescript::template_cache::TemplateCache::new()),
        });
        wirescript::ir::dump_module_with_source(&lowered.module, 0, Some(&src));
    }

    let opts = wirescript::EmitOptions {
        chip_pos: wirescript::Placement { x, y, z },
        inner_grid_location: brdb::Vector3f {
            x: x as f32,
            y: y as f32,
            z: z as f32 + 21.0,
        },
        open,
        ..Default::default()
    };

    let out_path = output.unwrap_or_else(|| source.with_extension("brz"));
    let is_brdb = out_path.extension().is_some_and(|e| e == "brdb");

    let input = wirescript::CompileInput {
        source: &src,
        file: &file,
        module_name: module_name.as_deref(),
    };

    if is_brdb {
        let result = wirescript::compile_to_world(input, opts)
            .map_err(|e| -> Box<dyn Error> { e.to_string().into() })?;
        if verbose {
            for d in &result.diagnostics {
                eprintln!("[{}] {} ({}:{}:{})", d.code, d.message, d.range.file, d.range.start.line, d.range.start.col);
            }
        }
        result.world.write_brdb(&out_path)?;
    } else {
        let result = wirescript::compile_with_opts(input, opts)
            .map_err(|e| -> Box<dyn Error> { e.to_string().into() })?;
        if verbose {
            for d in &result.diagnostics {
                eprintln!("[{}] {} ({}:{}:{})", d.code, d.message, d.range.file, d.range.start.line, d.range.start.col);
            }
        }
        std::fs::write(&out_path, &result.brz)?;
    }

    eprintln!("wrote {}", out_path.display());
    Ok(())
}

// ---------- legacy bearilog ----------

fn run_bearilog(
    file: clap_stdin::FileOrStdin,
    module: String,
    inline: bool,
    graph: bool,
    output: Option<PathBuf>,
    layout: builder::options::LayoutMode,
    layout_options: builder::options::LayoutOptions,
    grid_options: builder::options::GridOptions,
) -> Result<(), Box<dyn Error>> {
    let source = file.contents()?;
    let res = match bearilog::parse_and_compile(&source, &module, inline) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("{e}");
            return Err(e.into());
        }
    };
    if graph {
        println!("{}", bearilog::graphviz::render(&res)?);
    } else if let Some(path) = output {
        let world = match layout {
            builder::options::LayoutMode::Layout => {
                builder::layout_module_to_world(res, layout_options)?
            }
            builder::options::LayoutMode::Grid => builder::build_grid(res, grid_options),
        };
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        world.write_brz(path)?;
    } else {
        println!("{res}");
    }
    Ok(())
}
