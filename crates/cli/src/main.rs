use clap::{Parser, Subcommand};
use std::{error::Error, path::PathBuf};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
        /// Wirescript source file.
        source: PathBuf,

        /// Output path; defaults to the source path with a `.brz` extension.
        /// The extension picks the format: `.brdb` is SQLite, loadable via
        /// `BR.World.LoadAdditive`. Any other extension, or none, writes a
        /// `.brz` (zstd bundle).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Override the module name embedded in the save.
        #[arg(long)]
        name: Option<String>,

        /// Print diagnostics even when successful.
        #[arg(long)]
        verbose: bool,

        #[command(flatten)]
        place: PlacementArgs,

        /// Also print the compiled program as `.wsir` text to stdout, which
        /// `assemble` reads back (diagnostics stay on stderr). Prefab
        /// references are written relative to the current directory.
        #[arg(long)]
        dump_ir: bool,

        /// Also write the program as `.wsir` text to this path; `assemble`
        /// builds the same save from it.
        #[arg(long, value_name = "PATH")]
        emit_ir: Option<PathBuf>,

        /// Disable the certified constant-fold pass (it runs by default);
        /// equivalent to a module-level `@nofold`.
        #[arg(long, conflicts_with = "fold")]
        no_fold: bool,

        /// Explicitly enable the certified constant-fold pass. Folding is on by
        /// default, so this is redundant; a module-level `@nofold` still wins.
        #[arg(long, conflicts_with = "no_fold")]
        fold: bool,
    },
    /// Format wirescript files in place. Each file follows the `.editorconfig`
    /// files that govern it (indentation, `max_line_length`, `end_of_line`,
    /// `insert_final_newline`).
    Fmt {
        /// Files, or directories to search for `.ws` files.
        #[arg(required = true)]
        paths: Vec<PathBuf>,

        /// Write nothing; list the files that are not formatted and fail if any.
        #[arg(long)]
        check: bool,
    },
    /// Build a .brdb or .brz save from a `.wsir` program.
    Assemble {
        /// The `.wsir` file.
        source: PathBuf,

        /// Output path; the extension picks the format as for `compile`.
        #[arg(short, long)]
        output: Option<PathBuf>,

        #[command(flatten)]
        place: PlacementArgs,

        /// Re-write the parsed program and fail unless the text comes back
        /// unchanged (a canonical file is a fixed point).
        #[arg(long)]
        check_roundtrip: bool,

        /// Skip `Program::validate` (unknown classes and ports, exec/value
        /// mismatches, fan-in, missing nodes, chip I/O).
        #[arg(long)]
        no_validate: bool,
    },
}

/// Where the outer microchip goes, and whether chips open.
#[derive(clap::Args, Debug)]
struct PlacementArgs {
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
}

impl PlacementArgs {
    /// `base` with this placement.
    fn apply(&self, base: wirescript::EmitOptions) -> wirescript::EmitOptions {
        wirescript::EmitOptions {
            chip_pos: wirescript::Placement {
                x: self.x,
                y: self.y,
                z: self.z,
            },
            open: self.open,
            ..base
        }
    }
}

fn main() {
    // Wirescript lowering recurses in proportion to program size, so a large
    // module can overflow the default main-thread stack (RUST_MIN_STACK only
    // affects spawned threads, not main). Run the work on a worker thread with a
    // big stack so large programs compile.
    let result = std::thread::Builder::new()
        .stack_size(1024 * 1024 * 1024) // 1 GiB (reserved, not committed)
        .spawn(|| run().map_err(|e| e.to_string()))
        .expect("failed to spawn compile thread")
        .join()
        .expect("compile thread panicked");
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile {
            source,
            output,
            name,
            verbose,
            place,
            dump_ir,
            emit_ir,
            no_fold,
            fold,
        } => {
            let fold_mode = match (fold, no_fold) {
                (true, true) => unreachable!("clap conflicts_with rules out --fold --no-fold"),
                (true, false) => wirescript::FoldMode::ForceOn,
                (false, true) => wirescript::FoldMode::ForceOff,
                (false, false) => wirescript::FoldMode::Auto,
            };
            run_wirescript(
                source, output, name, verbose, &place, dump_ir, emit_ir, fold_mode,
            )
        }
        Command::Assemble {
            source,
            output,
            place,
            check_roundtrip,
            no_validate,
        } => run_assemble(source, output, &place, check_roundtrip, !no_validate),
        Command::Fmt { paths, check } => run_fmt(&paths, check),
    }
}

// ---------- fmt ----------

/// The `.ws` files in `path`: itself if a file, else every one beneath it,
/// skipping hidden directories, `target` and `node_modules`.
fn ws_files(path: &std::path::Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !path.is_dir() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let p = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                ws_files(&p, out)?;
            }
        } else if name.ends_with(".ws") {
            out.push(p);
        }
    }
    Ok(())
}

fn run_fmt(paths: &[PathBuf], check: bool) -> Result<(), Box<dyn Error>> {
    let mut files = Vec::new();
    for path in paths {
        ws_files(path, &mut files).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    }
    let mut unformatted = 0usize;
    for file in &files {
        let src = std::fs::read_to_string(file)
            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let opts = wirescript::analysis::FormatOptions::default().with_editorconfig(file);
        let formatted = wirescript::analysis::format_with(&src, &opts);
        if formatted == src {
            continue;
        }
        unformatted += 1;
        if check {
            println!("{}", file.display());
        } else {
            std::fs::write(file, formatted)
                .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
            println!("formatted {}", file.display());
        }
    }
    if check && unformatted > 0 {
        return Err(format!("{unformatted} of {} file(s) not formatted", files.len()).into());
    }
    Ok(())
}

// ---------- assemble ----------

fn run_assemble(
    source: PathBuf,
    output: Option<PathBuf>,
    place: &PlacementArgs,
    check_roundtrip: bool,
    validate: bool,
) -> Result<(), Box<dyn Error>> {
    let text = std::fs::read_to_string(&source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let file = source.to_string_lossy().to_string();
    let program = wirescript::wsir::parse_program(&text, &file)?;
    if validate {
        let errors = program.validate();
        if !errors.is_empty() {
            for e in errors.iter().take(20) {
                eprintln!("{file}: {e}");
            }
            return Err(format!("{file}: {} validation error(s)", errors.len()).into());
        }
    }
    if check_roundtrip {
        wirescript::wsir::check_fixed_point(&program, &text).map_err(|e| format!("{file}: {e}"))?;
    }
    // `.ws` source prefabs compile with the fold setting the program had.
    let opts = place.apply(wirescript::assemble_options(
        &file,
        if program.settings.no_fold {
            wirescript::FoldMode::ForceOff
        } else {
            wirescript::FoldMode::Auto
        },
    ));
    let world = wirescript::assemble(&program, &opts)?.world;
    write_world(
        &world,
        &output.unwrap_or_else(|| source.with_extension("brz")),
    )
}

/// Write `world` to `out_path`, in the format its extension picks: `.brdb` is
/// SQLite, anything else a `.brz`.
fn write_world(world: &brdb::World, out_path: &std::path::Path) -> Result<(), Box<dyn Error>> {
    if out_path.extension().is_some_and(|e| e == "brdb") {
        world.write_brdb(out_path)?;
    } else {
        std::fs::write(out_path, world.to_brz_vec()?)?;
    }
    eprintln!("wrote {}", out_path.display());
    Ok(())
}

// ---------- wirescript compile ----------

fn run_wirescript(
    source: PathBuf,
    output: Option<PathBuf>,
    module_name: Option<String>,
    verbose: bool,
    place: &PlacementArgs,
    dump_ir: bool,
    emit_ir: Option<PathBuf>,
    fold_mode: wirescript::FoldMode,
) -> Result<(), Box<dyn Error>> {
    let src = std::fs::read_to_string(&source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let file = source.to_string_lossy().to_string();

    let input = wirescript::CompileInput {
        source: &src,
        file: &file,
        module_name: module_name.as_deref(),
        fold_mode,
    };
    let result = wirescript::compile_to_world(input, place.apply(Default::default()))?;
    report_diagnostics(&result.diagnostics, verbose);
    write_world(
        &result.world,
        &output.unwrap_or_else(|| source.with_extension("brz")),
    )?;
    if dump_ir {
        // Prefab references are re-expressed from the directory the dump is
        // read back in, taken to be the current one.
        let mut program = result.program.clone();
        wirescript::prepare_program_for_text(
            &mut program,
            &file,
            std::path::Path::new("dump.wsir"),
            fold_mode,
        )?;
        wirescript::wsir::write_program(&program, &mut std::io::stdout().lock())?;
    }
    if let Some(ir_path) = emit_ir {
        let mut program = result.program;
        wirescript::prepare_program_for_text(&mut program, &file, &ir_path, fold_mode)?;
        let f = std::fs::File::create(&ir_path)
            .map_err(|e| format!("cannot write {}: {e}", ir_path.display()))?;
        let mut w = std::io::BufWriter::new(f);
        wirescript::wsir::write_program(&program, &mut w)?;
        std::io::Write::flush(&mut w)?;
        eprintln!("wrote {}", ir_path.display());
    }
    Ok(())
}

/// Print compile diagnostics. Warnings and errors ALWAYS print: a successful
/// compile that silently swallows a warning is how a miscompile ships. `verbose`
/// additionally prints lower-severity notes.
fn report_diagnostics(diags: &[wirescript::Diagnostic], verbose: bool) {
    use wirescript::Severity;
    for d in diags {
        let show = verbose || matches!(d.severity, Severity::Error | Severity::Warning);
        if !show {
            continue;
        }
        let (label, color) = match d.severity {
            Severity::Error => ("ERROR", "[31m"),
            Severity::Warning => ("WARN", "[33m"),
            _ => ("NOTE", "[36m"),
        };
        eprintln!("{color}{label}[0m {d}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// The `compile` help describes what the code does: the source is a file
    /// path, and an output path without a `.brdb` extension (or no output
    /// path) gets a `.brz`.
    #[test]
    fn compile_help_matches_the_code() {
        let mut cmd = Cli::command();
        let help = cmd
            .find_subcommand_mut("compile")
            .expect("compile")
            .render_long_help()
            .to_string();
        assert!(!help.contains("stdin"), "{help}");
        assert!(!help.contains("no extension given"), "{help}");
        assert!(
            help.contains("Any other extension, or none, writes a `.brz`"),
            "{help}"
        );
    }
}
