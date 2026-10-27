use std::{env, fs, path::Path, process};
use wirescript::{CompileInput, Severity, diagnostics_only};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: wirescript-check <file.ws> [file2.ws ...]");
        process::exit(1);
    }

    let mut total_errors = 0;
    let mut total_warnings = 0;

    for file_arg in &args[1..] {
        let path = Path::new(file_arg);
        let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let file_str = abs.to_string_lossy().to_string();

        let source = match fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("\x1b[31mERROR\x1b[0m cannot read '{file_arg}': {e}");
                total_errors += 1;
                continue;
            }
        };

        // Run the front end through lowering, not just typecheck: lowering reports
        // the `WSP001` placeholder warnings for a construct that type-checks but has
        // no lowering, which a typecheck-only run would pass as clean.
        // `diagnostics_only` runs `compile` up to emit, on the big compile stack.
        let all = diagnostics_only(CompileInput::new(&source, &file_str), &wirescript::FsLoader);
        // Diagnostics from an imported module are reported too, against the file
        // they came from. Dropping them printed "no errors" for programs the
        // compiler rejects outright, and every real project here is multi-file.
        if all.is_empty() {
            eprintln!("\x1b[32m✓\x1b[0m {file_arg}: no errors");
            continue;
        }

        for d in &all {
            let (label, color) = match d.severity {
                Severity::Error => {
                    total_errors += 1;
                    ("ERROR", "\x1b[31m")
                }
                Severity::Warning => {
                    total_warnings += 1;
                    ("WARN", "\x1b[33m")
                }
                Severity::Info => ("INFO", "\x1b[36m"),
            };
            eprintln!(
                "{color}{label}\x1b[0m [{}] {} ({})",
                d.code,
                d.message,
                d.location_or(file_arg)
            );
        }
    }

    if total_errors > 0 || total_warnings > 0 {
        eprintln!("\n{total_errors} error(s), {total_warnings} warning(s)");
    }
    process::exit(i32::from(total_errors > 0));
}
