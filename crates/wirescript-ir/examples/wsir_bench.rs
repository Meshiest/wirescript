//! Parse and write throughput of the `.wsir` text form.
//!
//! `cargo run --release -p wirescript-ir --example wsir_bench -- <file.wsir> [runs]`
//! (`just ir-bench` compiles 2raab to `.wsir` first and runs this on it).

use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: wsir_bench <file.wsir> [runs]");
    let runs: u32 = args.next().map_or(5, |n| n.parse().expect("runs is a number"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let lines = text.lines().count();
    let mb = text.len() as f64 / 1e6;

    let best = |f: &mut dyn FnMut()| -> Duration {
        (0..runs)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .min()
            .unwrap()
    };

    let mut program = None;
    let parse = best(&mut || {
        program = Some(wirescript_ir::wsir::parse_program(&text, &path).unwrap_or_else(|e| panic!("{e}")));
    });
    let program = program.unwrap();
    let mut out = Vec::with_capacity(text.len());
    let write = best(&mut || {
        out.clear();
        wirescript_ir::wsir::write_program(&program, &mut out).expect("write");
    });
    assert_eq!(out, text.as_bytes(), "the file is not in canonical form, so write != read");

    let nodes = count_nodes(&program.module);
    println!("{path}: {lines} lines, {mb:.2} MB, {nodes} nodes; best of {runs}");
    for (what, d) in [("parse", parse), ("write", write)] {
        let s = d.as_secs_f64();
        println!(
            "  {what}: {:>8.1} ms  {:>7.1} MB/s  {:>6.2} M lines/s  {:>6.2} M nodes/s",
            s * 1e3,
            mb / s,
            lines as f64 / s / 1e6,
            nodes as f64 / s / 1e6
        );
    }
}

fn count_nodes(m: &wirescript_ir::Module) -> usize {
    let mut n = 0;
    m.walk(&mut |_, sub| n += sub.nodes.len());
    n
}
