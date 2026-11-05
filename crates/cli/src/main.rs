use clap::{Parser, Subcommand};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use wirescript::FoldMode;

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
        /// The extension picks the format: `.brdb` is `SQLite`, loadable via
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

        /// Compile a test build: each `testing.*` statement becomes the check
        /// it describes, for an in-game self-checking program. Without it they
        /// compile to nothing.
        #[arg(long)]
        tests: bool,
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
    /// Run a program in the gate simulator and print what it says in chat.
    /// Takes a `.ws` source, a `.wsir` program or a `.brz` save.
    Sim {
        /// The program to run.
        source: PathBuf,

        #[command(flatten)]
        sim: SimArgs,
    },
    /// Run every `@test` (a `fn`, `mod` or named `chip`) in the gate
    /// simulator, and compare each program
    /// that has a `.expected` file beside it with that file. A test fails when
    /// it prints a chat line starting with `FAIL` or the simulation stops with
    /// an error. A `.script` file beside a program drives its world.
    Test {
        /// Files, or directories to search for `.ws` files.
        #[arg(required = true)]
        paths: Vec<PathBuf>,

        /// Run each test through this tick (tick 0 is the Load Event).
        #[arg(long, default_value_t = 600)]
        ticks: u64,

        /// Seconds each tick advances timers by.
        #[arg(long, default_value_t = 1.0 / 60.0)]
        dt: f64,

        /// Run handlers fired by the same change in a seeded order.
        #[arg(long)]
        seed: Option<u64>,

        /// Race mode: also run every program under N seeded handler orders
        /// and fail it when its chat changes with the order.
        #[arg(long, value_name = "N", default_value_t = 0)]
        race: u64,
    },
}

mod test_runner;

/// How a simulation runs.
#[derive(clap::Args, Debug)]
struct SimArgs {
    /// Run through this tick (tick 0 is the Load Event).
    #[arg(long, default_value_t = 600)]
    ticks: u64,

    /// Seconds each tick advances timers by.
    #[arg(long, default_value_t = 1.0 / 60.0)]
    dt: f64,

    /// Run handlers fired by the same change, and custom events, in a seeded
    /// order instead of save order (the game fixes neither).
    #[arg(long)]
    seed: Option<u64>,

    /// Prefix each chat line with the tick it was printed on.
    #[arg(long)]
    show_ticks: bool,

    /// Run save order and then seeds 0 to N-1, printing one line per run:
    /// the seed (`-` for save order), a tab, and the chat lines joined by
    /// ` || `.
    #[arg(long, value_name = "N", conflicts_with = "seed")]
    seeds: Option<u64>,

    /// A world script: players joining, chat commands, sweep results
    /// (`<tick> join alice`, `<tick> chat alice /roll 3`).
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,

    /// Frame times to replay, in microseconds, one per tick from tick 1 (probe
    /// 17's `P17 dt <tick>:` lines can be pasted as they are); the last one
    /// repeats. Tick 0, the Load Event's frame, takes `--dt`.
    #[arg(long, value_name = "FILE")]
    dt_file: Option<PathBuf>,

    /// Print to stderr how long loading and running took and how much ran.
    #[arg(long)]
    stats: bool,

    /// Run the Load Event, then read commands from stdin: script lines
    /// without a tick (`join alice`, `input alice Forward 1`), queued for
    /// the next tick, and `step [N]`, `screen <player>`, `quit`.
    #[arg(long, conflicts_with = "seeds")]
    interactive: bool,

    /// Run this `@test` once on load, as `wirescript test` does.
    #[arg(long, value_name = "NAME")]
    test: Option<String>,
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
            tests,
        } => {
            let fold_mode = match (fold, no_fold) {
                (true, true) => unreachable!("clap conflicts_with rules out --fold --no-fold"),
                (true, false) => FoldMode::ForceOn,
                (false, true) => FoldMode::ForceOff,
                (false, false) => FoldMode::Auto,
            };
            run_wirescript(
                &source,
                output,
                name.as_deref(),
                verbose,
                &place,
                dump_ir,
                emit_ir,
                fold_mode,
                tests,
            )
        }
        Command::Assemble {
            source,
            output,
            place,
            check_roundtrip,
            no_validate,
        } => run_assemble(&source, output, &place, check_roundtrip, !no_validate),
        Command::Fmt { paths, check } => run_fmt(&paths, check),
        Command::Sim { source, sim } => run_sim(&source, &sim),
        Command::Test {
            paths,
            ticks,
            dt,
            seed,
            race,
        } => {
            check_dt(dt)?;
            test_runner::run_tests(
                &paths,
                &test_runner::TestArgs {
                    ticks,
                    dt,
                    seed,
                    race,
                },
            )
        }
    }
}

// ---------- sim ----------

/// The frame times in a `--dt-file`: every number after an optional
/// `P17 dt <tick>:` prefix, in microseconds, as seconds.
fn read_dt_file(path: &Path) -> Result<Vec<f64>, Box<dyn Error>> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let values = line.split_once(':').map_or(line, |(_, rest)| rest);
        for word in values.split_whitespace() {
            let us: f64 = word
                .parse()
                .map_err(|_| format!("{}: {word:?} is not a number", path.display()))?;
            let dt = us / 1_000_000.0;
            check_dt(dt)?;
            out.push(dt);
        }
    }
    if out.is_empty() {
        return Err(format!("{} holds no frame times", path.display()).into());
    }
    Ok(out)
}

/// A frame time the timers can count: one microsecond to one hour.
fn check_dt(dt: f64) -> Result<(), Box<dyn Error>> {
    if (1e-6..=3600.0).contains(&dt) {
        Ok(())
    } else {
        Err(format!("--dt must be between 0.000001 and 3600 seconds, not {dt}").into())
    }
}

/// Step `sim` by commands read from stdin, printing chat as it is said.
fn run_interactive(sim: &mut wirescript_sim::Sim) -> Result<(), Box<dyn Error>> {
    use std::io::BufRead;
    const HELP: &str = "commands: step [N], screen <player>, quit; anything else is a script line without its tick (join alice, chat alice /go, input alice Forward 1)";
    sim.run_until(0)?;
    eprintln!("tick 0 done. {HELP}");
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [] => {}
            ["quit" | "exit"] => break,
            ["help"] => eprintln!("{HELP}"),
            ["step"] | ["step", _] => {
                let n: u64 = match words.get(1) {
                    Some(n) => match n.parse() {
                        Ok(n) => n,
                        Err(_) => {
                            eprintln!("step takes a number of ticks");
                            continue;
                        }
                    },
                    None => 1,
                };
                let target = sim.machine.tick() + n;
                sim.run_until(target)?;
                eprintln!("tick {target} done");
            }
            ["screen", name] => match sim.machine.world.screen(name) {
                Some(texts) if texts.is_empty() => eprintln!("{name}'s screen is empty"),
                Some(texts) => {
                    for (id, text) in texts {
                        println!("[{name} text {id}] {}", wirescript_sim::script_world::escape_controls(&text));
                    }
                }
                None => eprintln!("no player named {name}"),
            },
            _ => match sim.push(&line) {
                Ok(()) => eprintln!("queued for tick {}", sim.machine.tick() + 1),
                Err(e) => eprintln!("{e}"),
            },
        }
    }
    Ok(())
}

fn run_sim(source: &Path, args: &SimArgs) -> Result<(), Box<dyn Error>> {
    use wirescript_sim::script_world::escape_controls;
    use wirescript_sim::{Dt, Loaded, ScriptWorld, SimOptions};
    check_dt(args.dt)?;
    let file = source.to_string_lossy().to_string();
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let started = std::time::Instant::now();
    let loaded = match ext.as_str() {
        "brz" => {
            let bytes =
                fs::read(source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
            Loaded::from_brz(&bytes)?
        }
        "wsir" => {
            let text = fs::read_to_string(source)
                .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
            let program = wirescript::wsir::parse_program(&text, &file)?;
            let fold = if program.settings.no_fold {
                FoldMode::ForceOff
            } else {
                FoldMode::Auto
            };
            let emit = wirescript::assemble_options(&file, fold);
            Loaded::from_program(&program, &emit)?
        }
        _ => {
            let src = fs::read_to_string(source)
                .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
            let mut input = wirescript::CompileInput::new(&src, &file).test_build(true);
            if let Some(name) = &args.test {
                input = input.test_entry(name);
            }
            let emit = wirescript::emit::EmitOptions::default();
            let result = wirescript::compile_to_world(input, emit)?;
            report_diagnostics(&result.diagnostics, false);
            Loaded::from_world(&result.world, &result.node_bricks, &result.program)?
        }
    };
    let dt = match &args.dt_file {
        Some(path) => {
            let mut schedule = vec![args.dt];
            schedule.extend(read_dt_file(path)?);
            Dt::Schedule(schedule)
        }
        None => Dt::Constant(args.dt),
    };
    let opts = |seed| SimOptions {
        seed,
        dt: dt.clone(),
    };
    let script = match &args.script {
        Some(path) => Some(
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?,
        ),
        None => None,
    };
    let world = || -> Result<ScriptWorld, Box<dyn Error>> {
        Ok(match &script {
            Some(text) => ScriptWorld::from_script(text)?,
            None => ScriptWorld::default(),
        })
    };
    if let Some(n) = args.seeds {
        let mut failed = 0;
        for seed in std::iter::once(None).chain((0..n).map(Some)) {
            let mut sim = loaded.start(Box::new(world()?), opts(seed));
            let outcome = sim.run_until(args.ticks);
            let mut lines: Vec<String> = sim.chat().iter().map(|l| escape_controls(&l.text)).collect();
            if let Err(e) = outcome {
                failed += 1;
                lines.push(format!("Error: {e}"));
            }
            let label = seed.map_or("-".to_string(), |s| s.to_string());
            println!("{label}\t{}", lines.join(" || "));
        }
        if failed > 0 {
            return Err(format!("{failed} run(s) ended in an error").into());
        }
        return Ok(());
    }
    // Lines print as they are said, so a run that errors still shows them.
    let mut world = world()?;
    world.echo = true;
    world.echo_ticks = args.show_ticks;
    let loaded_at = started.elapsed();
    let gates = loaded.graph.gates.len();
    let mut sim = loaded.into_sim(Box::new(world), opts(args.seed));
    if args.interactive {
        return run_interactive(&mut sim);
    }
    let outcome = sim.run_until(args.ticks);
    if args.stats {
        let run = started.elapsed().saturating_sub(loaded_at);
        let ticks = sim.machine.tick();
        let (steps, evals) = sim.machine.stats();
        let secs = run.as_secs_f64().max(1e-9);
        eprintln!(
            "stats: {} gates; compile and load {:.3} s; {} ticks in {:.3} s ({:.0} ticks/s); {} exec gates run ({:.0}/s), {} pure gates evaluated ({:.0}/s)",
            gates,
            loaded_at.as_secs_f64(),
            ticks + 1,
            secs,
            (ticks + 1) as f64 / secs,
            steps,
            steps as f64 / secs,
            evals,
            evals as f64 / secs,
        );
    }
    outcome?;
    Ok(())
}

// ---------- fmt ----------

/// The `.ws` files in `path`: itself if a file, else every one beneath it,
/// skipping hidden directories, `target` and `node_modules`.
fn ws_files(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !path.is_dir() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(path)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let p = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // A linked directory is not followed: a link back up the tree would
        // loop.
        if entry.file_type()?.is_dir() {
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
        let src =
            fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let opts = wirescript::analysis::FormatOptions::default().with_editorconfig(file);
        let formatted = wirescript::analysis::format_with(&src, &opts);
        if formatted == src {
            continue;
        }
        unformatted += 1;
        if check {
            println!("{}", file.display());
        } else {
            fs::write(file, formatted)
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
    source: &Path,
    output: Option<PathBuf>,
    place: &PlacementArgs,
    check_roundtrip: bool,
    validate: bool,
) -> Result<(), Box<dyn Error>> {
    let text =
        fs::read_to_string(source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
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
            FoldMode::ForceOff
        } else {
            FoldMode::Auto
        },
    ));
    let world = wirescript::assemble(&program, &opts)?.world;
    write_world(
        &world,
        &output.unwrap_or_else(|| source.with_extension("brz")),
    )
}

/// Write `world` to `out_path`, in the format its extension picks: `.brdb` is
/// `SQLite`, anything else a `.brz`.
fn write_world(world: &brdb::World, out_path: &Path) -> Result<(), Box<dyn Error>> {
    if out_path.extension().is_some_and(|e| e == "brdb") {
        world.write_brdb(out_path)?;
    } else {
        fs::write(out_path, world.to_brz_vec()?)?;
    }
    eprintln!("wrote {}", out_path.display());
    Ok(())
}

// ---------- wirescript compile ----------

#[expect(clippy::too_many_arguments, reason = "one argument per `compile` flag")]
fn run_wirescript(
    source: &Path,
    output: Option<PathBuf>,
    module_name: Option<&str>,
    verbose: bool,
    place: &PlacementArgs,
    dump_ir: bool,
    emit_ir: Option<PathBuf>,
    fold_mode: FoldMode,
    test_build: bool,
) -> Result<(), Box<dyn Error>> {
    let src =
        fs::read_to_string(source).map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let file = source.to_string_lossy().to_string();

    let input = wirescript::CompileInput {
        source: &src,
        file: &file,
        module_name,
        fold_mode,
        test_entry: None,
        test_build,
    };
    let result =
        wirescript::compile_to_world(input, place.apply(wirescript::emit::EmitOptions::default()))?;
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
            Path::new("dump.wsir"),
            fold_mode,
        )?;
        wirescript::wsir::write_program(&program, &mut std::io::stdout().lock())?;
    }
    if let Some(ir_path) = emit_ir {
        let mut program = result.program;
        wirescript::prepare_program_for_text(&mut program, &file, &ir_path, fold_mode)?;
        let f = fs::File::create(&ir_path)
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
            Severity::Info => ("NOTE", "[36m"),
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
