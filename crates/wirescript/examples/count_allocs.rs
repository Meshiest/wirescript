//! Allocation counts + bytes per compile stage (companion to
//! `profile_stages`, which reports wall time).
//!   cargo run --release -p wirescript --example count_allocs -- <file.ws>
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new as u64, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn snap(label: &str, last: &mut (u64, u64)) {
    let a = ALLOCS.load(Ordering::Relaxed);
    let b = BYTES.load(Ordering::Relaxed);
    eprintln!(
        "{label:<18} {:>10} allocs {:>9.1} MB",
        a - last.0,
        (b - last.1) as f64 / 1e6
    );
    *last = (a, b);
}

fn main() {
    let file = std::env::args().nth(1).expect("usage: count_allocs <file.ws>");
    let source = std::fs::read_to_string(&file).expect("read source");
    let mut last = (0u64, 0u64);

    let resolved = wirescript::resolve::resolve(&source, &file, &wirescript::resolve::FsLoader);
    snap("resolve", &mut last);

    let tc = wirescript::typecheck::typecheck(&resolved.ast, &file);
    snap("typecheck", &mut last);

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
    snap("lower", &mut last);

    let _cycles = wirescript::analyze::analyze_cycles(&lowered.module);
    snap("analyze_cycles", &mut last);

    let lr = wirescript::layout::layout(&lowered.module);
    snap("layout", &mut last);

    let mut opts = wirescript::EmitOptions::default();
    opts.prefab_resolver = Some(wirescript::disk_prefab_resolver(&file));
    let world = wirescript::build_world(&lowered.module, &lr, &opts, &cache).expect("emit");
    snap("build_world", &mut last);

    let unsaved = world.to_unsaved().expect("to_unsaved");
    snap("to_unsaved", &mut last);
    let pending = unsaved.to_pending().expect("to_pending");
    snap("to_pending", &mut last);
    let brz = pending.to_brz_data(Some(3)).expect("brz");
    snap("to_brz_data", &mut last);
    let _ = brz;
}
