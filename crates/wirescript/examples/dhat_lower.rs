//! Allocation-site attribution for the `lower` stage (temporary tooling).
//!   cargo run --release -p wirescript --features dhat-heap --example dhat_lower -- <file.ws>
//! Writes dhat-heap.json (view at https://nnethercote.github.io/dh_view/dh_view.html).
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    #[cfg(not(feature = "dhat-heap"))]
    panic!("build with --features dhat-heap");

    #[cfg(feature = "dhat-heap")]
    {
        let file = std::env::args().nth(1).expect("usage: dhat_lower <file.ws>");
        let source = std::fs::read_to_string(&file).expect("read source");

        let resolved = wirescript::resolve::resolve(&source, &file, &wirescript::resolve::FsLoader);
        let tc = wirescript::typecheck::typecheck(&resolved.ast, &file);
        let cache = std::sync::Arc::new(wirescript::template_cache::TemplateCache::new());

        let _profiler = dhat::Profiler::new_heap();
        let lowered = wirescript::lower::lower(wirescript::lower::LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: &file,
            module_name: None,
            template_cache: cache.clone(),
            doc_comments: &resolved.doc_comments,
        });
        drop(_profiler);
        eprintln!("lowered: {} nodes", lowered.module.nodes.len());
    }
}
