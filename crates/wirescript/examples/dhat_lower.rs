//! Allocation-site attribution for the `lower` stage.
//!   `cargo run --release -p wirescript --features dhat-heap --example dhat_lower -- <file.ws>`
//! Writes dhat-heap.json (view at <https://nnethercote.github.io/dh_view/dh_view.html>).
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    #[cfg(not(feature = "dhat-heap"))]
    panic!("build with --features dhat-heap");

    #[cfg(feature = "dhat-heap")]
    {
        let file = std::env::args()
            .nth(1)
            .expect("usage: dhat_lower <file.ws>");
        let source = std::fs::read_to_string(&file).expect("read source");

        let resolved = wirescript::resolve::resolve(&source, &file, &wirescript::resolve::FsLoader);
        let (tc, ce_slots) = wirescript::typecheck_with_inference(&resolved.ast);
        let cache = std::sync::Arc::new(wirescript::TemplateCache::new());

        let profiler = dhat::Profiler::new_heap();
        let lowered = wirescript::lower(wirescript::LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            resolutions: &tc.resolutions,
            file: &file,
            module_name: None,
            template_cache: cache,
            doc_comments: &resolved.doc_comments,
            fold_mode: wirescript::FoldMode::Auto,
            ce_slots: &ce_slots,
        });
        drop(profiler);
        eprintln!("lowered: {} nodes", lowered.module.nodes.len());
    }
}
