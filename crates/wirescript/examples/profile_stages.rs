//! Per-stage timing for a full compile. Usage:
//!   cargo run --release -p wirescript --example profile_stages -- <file.ws>
use std::time::Instant;
use wirescript::emit::{EmitOptions, emit_brz};
use wirescript::layout::layout;
use wirescript::lower::{LowerInput, lower};
use wirescript::resolve::{FsLoader, resolve};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::typecheck;

fn main() {
    let file = std::env::args().nth(1).expect("usage: profile_stages <file.ws>");
    let source = std::fs::read_to_string(&file).expect("read source");

    let t = Instant::now();
    let resolved = resolve(&source, &file, &FsLoader);
    eprintln!("resolve (lex+parse+imports): {:>8.2?}", t.elapsed());

    let t = Instant::now();
    let tc = typecheck(&resolved.ast, &file);
    eprintln!("typecheck:                   {:>8.2?}", t.elapsed());

    let template_cache = std::sync::Arc::new(TemplateCache::new());
    let t = Instant::now();
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: &file,
        module_name: None,
        template_cache: template_cache.clone(),
        doc_comments: &resolved.doc_comments,
    });
    eprintln!("lower:                       {:>8.2?}", t.elapsed());
    eprintln!(
        "  nodes: {}, wires: {}",
        lowered.module.nodes.len(),
        lowered.module.wires.len()
    );

    fn count(m: &wirescript::Module, depth: usize, tally: &mut (usize, usize, usize)) {
        tally.0 += m.nodes.len();
        tally.1 += m.wires.len();
        tally.2 = tally.2.max(depth);
        for child in m.chips.values() {
            count(child, depth + 1, tally);
        }
    }
    let mut tally = (0, 0, 0);
    count(&lowered.module, 0, &mut tally);
    eprintln!(
        "  recursive: {} nodes, {} wires, max chip depth {}",
        tally.0, tally.1, tally.2
    );

    let t = Instant::now();
    let cycles = wirescript::analyze::analyze_cycles(&lowered.module);
    eprintln!("analyze_cycles:              {:>8.2?}", t.elapsed());
    let _ = cycles;

    let t = Instant::now();
    let lr = layout(&lowered.module);
    eprintln!("layout:                      {:>8.2?}", t.elapsed());

    let mut opts = EmitOptions::default();
    opts.prefab_resolver = Some(wirescript::disk_prefab_resolver(&file));
    let t = Instant::now();
    let world = wirescript::build_world(&lowered.module, &lr, &opts, &template_cache)
        .expect("emit");
    eprintln!("build_world:                 {:>8.2?}", t.elapsed());

    let t = Instant::now();
    let unsaved = world.to_unsaved().expect("to_unsaved");
    eprintln!("  to_unsaved (SoA pack):     {:>8.2?}", t.elapsed());
    let t = Instant::now();
    let pending = unsaved.to_pending().expect("to_pending");
    eprintln!("  to_pending (msgpack):      {:>8.2?}", t.elapsed());
    let t = Instant::now();
    let brz_data = pending.to_brz_data(Some(3)).expect("to_brz_data");
    eprintln!("  to_brz_data (zstd 3):      {:>8.2?}", t.elapsed());
    let t = Instant::now();
    let mut brz = Vec::new();
    brz_data.write(&mut brz, Some(3)).expect("write");
    eprintln!("  write (index):             {:>8.2?}", t.elapsed());
    eprintln!("brz size: {} bytes", brz.len());
    let _ = emit_brz;
}
