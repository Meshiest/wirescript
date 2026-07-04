use std::{env, fs, path::Path, process};
use wirescript::{resolve, typecheck::typecheck, FsLoader, Severity};

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
                eprintln!("\x1b[31mERROR\x1b[0m cannot read '{}': {}", file_arg, e);
                total_errors += 1;
                continue;
            }
        };

        let resolved = resolve(&source, &file_str, &FsLoader);
        let tc = typecheck(&resolved.ast, &file_str);

        let diags: Vec<_> = resolved
            .diagnostics
            .iter()
            .chain(tc.diagnostics.iter())
            .filter(|d| d.range.file.as_ref() == file_str || d.range.file.is_empty())
            .collect();

        if diags.is_empty() {
            eprintln!("\x1b[32m✓\x1b[0m {}: no errors", file_arg);
            continue;
        }

        for d in &diags {
            let (label, color) = match d.severity {
                Severity::Error => { total_errors += 1; ("ERROR", "\x1b[31m") }
                Severity::Warning => { total_warnings += 1; ("WARN", "\x1b[33m") }
                _ => { ("INFO", "\x1b[36m") }
            };
            eprintln!(
                "{}{}\x1b[0m [{}] {} ({}:{}:{})",
                color, label, d.code, d.message, file_arg, d.range.start.line, d.range.start.col
            );
        }
    }

    if total_errors > 0 || total_warnings > 0 {
        eprintln!(
            "\n{} error(s), {} warning(s)",
            total_errors, total_warnings
        );
    }
    process::exit(if total_errors > 0 { 1 } else { 0 });
}
