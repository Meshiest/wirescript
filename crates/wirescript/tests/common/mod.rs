//! Helpers shared by the emit-level integration tests.
//!
//! Each file in `tests/` is its own crate, so anything they share has to live
//! here and be pulled in with `mod common;`. Not every test uses every helper.
#![allow(dead_code)]

use brdb::IntoReader;
use brdb::schema::BrdbStruct;

/// Every component in an emitted world, in grid then chunk order.
///
/// Going through a real `.brz` on disk is the point: what these tests check is
/// what the game would load, not what the emitter thinks it wrote.
///
/// The scratch file is named per CALL, not per process. The tests in one file
/// run on separate threads of one process, so a pid-keyed name has two of them
/// writing and reading the same bytes, and whichever reads mid-write sees a
/// truncated archive.
///
/// The `1..32` grid sweep stops at the first id the reader rejects: grids are
/// numbered from 1 and allocated densely, so the first miss is the end, and no
/// fixture in this suite comes near 32.
pub fn world_components(brz: &[u8]) -> Vec<BrdbStruct> {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "ws_test_components_{}_{}.brz",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&path, brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut out = Vec::new();
    for gid in 1..32 {
        let Ok(chunks) = reader.brick_chunk_index(gid) else {
            break;
        };
        for chunk in chunks {
            if chunk.num_components == 0 {
                continue;
            }
            let (_soa, comps) = reader
                .component_chunk_soa(gid, chunk.index)
                .expect("read components");
            out.extend(comps);
        }
    }
    std::fs::remove_file(&path).ok();
    out
}

/// Run the front end - resolve, typecheck, lower at `fold_mode` - and hand back
/// all three stage results.
///
/// Asserts nothing on purpose. Several callers are checking that a stage DID
/// produce a diagnostic, so a helper that panicked on one would make the test
/// it is asserting for unwritable.
pub fn run_stages(
    source: &str,
    file: &str,
    fold_mode: wirescript::lower::FoldMode,
) -> (
    wirescript::resolve::ResolveResult,
    wirescript::typecheck::TypeCheckResult,
    wirescript::lower::LowerResult,
) {
    use wirescript::typecheck::CeSlotMap;
    let resolved = wirescript::resolve::resolve(source, file, &wirescript::resolve::FsLoader);
    let slots = CeSlotMap::default();
    let tc = wirescript::typecheck::typecheck(&resolved.ast, file, &slots);
    let lowered = wirescript::lower::lower(wirescript::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: None,
        template_cache: std::sync::Arc::new(wirescript::template_cache::TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode,
        ce_slots: &slots,
    });
    (resolved, tc, lowered)
}

/// Every typecheck diagnostic code `src` produces, in order.
///
/// Stops at typecheck on purpose: these are checker tests, and running lowering
/// would fold in a second stage's diagnostics.
pub fn diag_codes(src: &str) -> Vec<String> {
    let resolved = wirescript::resolve::resolve(src, "test", &wirescript::resolve::FsLoader);
    let tc = wirescript::typecheck::typecheck(
        &resolved.ast,
        "test",
        &wirescript::typecheck::CeSlotMap::default(),
    );
    tc.diagnostics.iter().map(|d| d.code.clone()).collect()
}

/// Whether `src` survives the whole pipeline to a `.brz`.
pub fn compiles(src: &str) -> bool {
    wirescript::compile(wirescript::CompileInput {
        source: src,
        file: "test",
        module_name: None,
        fold_mode: wirescript::FoldMode::Auto,
    })
    .is_ok()
}
