//! How the editor's per-request analysis grows with file size. Generates a
//! source of N lines (declarations, uses of one common name, un-annotated
//! `let`s, calls and a few errors) at 2k, 4k and 8k lines, times each request
//! through the public `analysis` API, and fails any whose 4k -> 8k ratio is
//! above 2.6. Usage:
//!   `cargo run --release -p wirescript --example editor_scale [-- --runs 7]`
use std::time::{Duration, Instant};
use wirescript::analysis::{
    collect_inlay_hints, find_name_range, prepare_rename_at, references_at, semantic_tokens,
};
use wirescript::resolve::{FsLoader, resolve_parsed};

const SIZES: [usize; 3] = [2000, 4000, 8000];
const LIMIT: f64 = 2.6;
/// A request under this at 8k lines is shown but not judged.
const NOISE_FLOOR: Duration = Duration::from_millis(5);
const FILE: &str = "editor_scale.ws";

/// About `lines` lines: a common var read and written throughout, a block of
/// six lines per step, and an undefined name every fiftieth step.
fn source(lines: usize) -> String {
    let mut out = vec![
        "in start: exec".to_string(),
        "var counter: int = 0".to_string(),
        "mod helper(v: *int, by: int) { v = v + by }".to_string(),
    ];
    let mut i = 0;
    while out.len() < lines {
        out.push(format!("var v{i}: int = {i}"));
        out.push(format!("let l{i} = v{i} + counter"));
        out.push("on start {".to_string());
        out.push(format!("  counter = counter + l{i}"));
        out.push(format!("  helper(counter, {i})"));
        out.push("}".to_string());
        if i % 50 == 49 {
            out.push(format!("let e{i} = missing{i} + 1"));
        }
        i += 1;
    }
    out.join("\n") + "\n"
}

/// Each request's time on `src`, in a fixed order.
fn requests(src: &str) -> Vec<(&'static str, Duration)> {
    let mut times = Vec::new();
    let mut time = |name: &'static str, f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        times.push((name, t.elapsed()));
    };

    let ast = wirescript::parse(src, FILE).ast;
    let mut resolved = None;
    let mut tc = None;
    // Parse, resolve and typecheck, as the language server does per edit.
    time("front end", &mut || {
        let r = resolve_parsed(wirescript::parse(src, FILE), FILE, &FsLoader);
        tc = Some(wirescript::typecheck_with_inference(&r.ast).0);
        resolved = Some(r);
    });
    let (resolved, tc) = (resolved.unwrap(), tc.unwrap());

    // Semantic tokens, narrowing each coarse span to its name as the language
    // server does.
    time("semantic tokens", &mut || {
        let spans = semantic_tokens(&ast);
        let narrowed = spans
            .iter()
            .filter(|s| s.coarse)
            .filter_map(|s| find_name_range(src, &s.range, &s.name))
            .count();
        std::hint::black_box((spans.len(), narrowed));
    });
    // Find-references and prepare-rename on `counter`'s declaration (line 2).
    time("references", &mut || {
        let (_, sites) = references_at(&ast, src, FILE, 1, 5).expect("counter has references");
        let narrowed = sites
            .iter()
            .filter(|s| s.coarse)
            .filter_map(|s| find_name_range(src, &s.range, "counter"))
            .count();
        std::hint::black_box((sites.len(), narrowed));
    });
    time("rename", &mut || {
        std::hint::black_box(prepare_rename_at(&ast, src, FILE, 1, 5));
    });
    time("inlay hints", &mut || {
        std::hint::black_box(collect_inlay_hints(src, &resolved.ast, &tc.type_of_expr, FILE).len());
    });
    time("diagnostics", &mut || {
        let input = wirescript::CompileInput::new(src, FILE);
        std::hint::black_box(wirescript::diagnostics_only(input, &FsLoader).len());
    });
    times
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let runs: usize = args
        .iter()
        .position(|a| a == "--runs")
        .and_then(|i| args.get(i + 1))
        .and_then(|r| r.parse().ok())
        .unwrap_or(5);

    let sources: Vec<String> = SIZES.iter().map(|&n| source(n)).collect();
    // The fastest of `runs` runs of each request at each size.
    let best: Vec<Vec<(&str, Duration)>> = sources
        .iter()
        .map(|src| {
            let mut best = wirescript::on_big_stack(|| requests(src));
            for _ in 1..runs {
                for (b, (_, t)) in best
                    .iter_mut()
                    .zip(wirescript::on_big_stack(|| requests(src)))
                {
                    b.1 = b.1.min(t);
                }
            }
            best
        })
        .collect();

    println!(
        "request times in ms at {} lines, ratio = t(8k) / t(4k); fail above {LIMIT}",
        SIZES.map(|n| n.to_string()).join(" / ")
    );
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut failures = Vec::new();
    for (k, (name, _)) in best[0].iter().enumerate() {
        let [a, b, c] = [0, 1, 2].map(|s| best[s][k].1);
        let ratio = ms(c) / ms(b).max(1e-6);
        let judged = c >= NOISE_FLOOR;
        let bad = judged && ratio > LIMIT;
        if bad {
            failures.push(format!("{name} {ratio:.2}"));
        }
        let mark = if bad {
            "  FAIL"
        } else if judged {
            ""
        } else {
            "  (under noise floor)"
        };
        println!(
            "{name:<16} {:>8.1} / {:>8.1} / {:>8.1}   {ratio:5.2}{mark}",
            ms(a),
            ms(b),
            ms(c)
        );
    }
    if !failures.is_empty() {
        eprintln!(
            "editor_scale: {} request(s) grow faster than {LIMIT}x per doubling: {}",
            failures.len(),
            failures.join(", ")
        );
        std::process::exit(1);
    }
    println!("editor_scale: every request scales within the limit");
}
