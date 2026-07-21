//! AABB-overlap scan over the emitted world's bricks. The game silently DROPS
//! overlapping bricks at load — a dropped brick in an exec chain stalls the
//! chain at that point with no error, so overlaps must be zero.
//!   cargo run --release -p wirescript --example check_overlaps -- <file.ws>
use std::time::Instant;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn scan(label: &str, bricks: &[brdb::Brick]) -> usize {
    // (min, max) world-space AABB per brick from position + local bounds.
    let boxes: Vec<(&brdb::Brick, brdb::Position, brdb::Position)> = bricks
        .iter()
        .map(|b| {
            let (lo, hi) = b.local_bounds();
            (b, b.position + lo, b.position + hi)
        })
        .collect();
    let mut overlaps = 0;
    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            let (a, alo, ahi) = &boxes[i];
            let (b, blo, bhi) = &boxes[j];
            let sep = ahi.x <= blo.x
                || bhi.x <= alo.x
                || ahi.y <= blo.y
                || bhi.y <= alo.y
                || ahi.z <= blo.z
                || bhi.z <= alo.z;
            if !sep {
                overlaps += 1;
                if overlaps <= 20 {
                    println!(
                        "{label}: OVERLAP #{overlaps}: [{i}] {:?}@{:?} <-> [{j}] {:?}@{:?}",
                        a.asset, a.position, b.asset, b.position
                    );
                }
            }
        }
    }
    println!("{label}: {} bricks, {} overlapping pairs", bricks.len(), overlaps);
    overlaps
}

fn main() {
    let file = std::env::args().nth(1).expect("usage: check_overlaps <file.ws>");
    let source = std::fs::read_to_string(&file).expect("read source");

    let t = Instant::now();
    let resolved = wirescript::resolve::resolve(&source, &file, &wirescript::resolve::FsLoader);
    let tc = wirescript::typecheck::typecheck(&resolved.ast, &file);
    let cache = std::sync::Arc::new(wirescript::template_cache::TemplateCache::new());
    let lowered = wirescript::lower::lower(wirescript::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: &file,
        module_name: None,
        template_cache: cache.clone(),
        doc_comments: &resolved.doc_comments,
        fold_mode: wirescript::lower::FoldMode::Auto,
    });
    let lr = wirescript::layout::layout(&lowered.module);
    let mut opts = wirescript::EmitOptions::default();
    opts.prefab_resolver = Some(wirescript::disk_prefab_resolver(&file));
    let world = wirescript::build_world(&lowered.module, &lr, &opts, &cache).expect("emit");
    eprintln!("compiled in {:?}", t.elapsed());

    let mut total = scan("root", &world.bricks);
    for (i, (entity, bricks)) in world.grids.iter().enumerate() {
        total += scan(&format!("grid {} ({})", i, entity.asset), bricks);
    }
    // Fan-in check: two wires driving the same (brick, component, port) make
    // the game reject one at load ("Failed to connect wire") — which can also
    // break an exec chain mid-run.
    let mut seen: std::collections::HashMap<(usize, String, String), usize> =
        std::collections::HashMap::new();
    let mut dups = 0;
    for w in &world.wires {
        let key = (
            w.target.brick_id,
            w.target.component_type.to_string(),
            w.target.port_name.to_string(),
        );
        let n = seen.entry(key).or_insert(0);
        *n += 1;
        if *n == 2 {
            dups += 1;
            if dups <= 20 {
                println!("FAN-IN: multiple wires into brick {} {}.{}", w.target.brick_id, w.target.component_type, w.target.port_name);
            }
        }
    }
    println!("{} wires, {} fan-in targets", world.wires.len(), dups);
    if total == 0 && dups == 0 {
        println!("OK: no overlaps, no fan-in");
    } else {
        println!("FAIL: {total} overlapping pairs, {dups} fan-in targets");
        std::process::exit(1);
    }
}
