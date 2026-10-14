//! Helpers shared by the emit-level integration tests.
//!
//! Each file in `tests/` is its own crate, so anything they share has to live
//! here and be pulled in with `mod common;`. Not every test uses every helper.
#![allow(dead_code)]

use brdb::schema::BrdbStruct;

/// Every component in an emitted world, in grid then chunk order: what the
/// game would load, read back from the save's bytes.
pub fn world_components(brz: &[u8]) -> Vec<BrdbStruct> {
    wirescript_ir::readback::Save::read(brz).components()
}

/// Run the front end - resolve, typecheck with custom-event slot inference,
/// lower at `fold_mode` - and hand back all three stage results, as a compile
/// runs them.
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
    let fe = wirescript::front_end(source, file, None, fold_mode, &wirescript::resolve::FsLoader);
    (fe.resolved, fe.typecheck, fe.lowered)
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

/// Every brick in a world, root shell and every child grid alike. A chip's
/// shell brick sits on the bricks of the module that instantiates it, so a
/// walk of only `world.bricks` misses every nested one.
pub fn all_bricks(world: &brdb::World) -> Vec<&brdb::Brick> {
    std::iter::once(&world.bricks)
        .chain(world.grids.iter().map(|(_, b)| b))
        .flat_map(|bricks| bricks.iter())
        .collect()
}

/// The brick-asset name of every brick in a world.
pub fn brick_assets(world: &brdb::World) -> Vec<String> {
    all_bricks(world)
        .iter()
        .map(|b| b.asset.asset().to_string())
        .collect()
}

/// Asset name paired with component class, per typed component. The two are
/// set independently at emit, so checking one alone passes while the pair the
/// game validates is still wrong.
pub fn brick_asset_components(world: &brdb::World) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for b in all_bricks(world) {
        let asset = b.asset.asset().to_string();
        for c in &b.components {
            if let Some(t) = c.component_type_ref() {
                out.push((asset.clone(), t.to_string()));
            } else if let Some(t) = c.component_type() {
                out.push((asset.clone(), t.to_string()));
            }
        }
    }
    out
}

/// The `(Count, FirstIndex, Step)` of each counted-loop shell, in world order.
/// Read from the encoded `.brz`, since a constant pin exists only as component
/// data and the in-memory `World` would not prove what the game reads.
pub fn count_for_shell_data(brz: &[u8]) -> Vec<(i64, i64, i64)> {
    use brdb::schema::BrdbValue;
    let mut out = Vec::new();
    for c in world_components(brz) {
        let (Some(BrdbValue::I64(count)), Some(BrdbValue::I64(first)), Some(BrdbValue::I64(step))) =
            (c.get("Count"), c.get("FirstIndex"), c.get("Step"))
        else {
            continue;
        };
        out.push((*count, *first, *step));
    }
    out
}

/// The compile input for `src` named `file`, folded as a compile folds by
/// default.
pub fn input<'a>(src: &'a str, file: &'a str) -> wirescript::CompileInput<'a> {
    wirescript::CompileInput { source: src, file, module_name: None, fold_mode: wirescript::FoldMode::Auto }
}

/// `src` compiled to a world, or a panic.
pub fn world(src: &str, file: &str) -> brdb::World {
    wirescript::compile_to_world(input(src, file), wirescript::emit::EmitOptions::default())
        .unwrap_or_else(|e| panic!("should compile to a world: {e:?}"))
        .world
}

/// `src` compiled to `.brz` bytes, or a panic.
pub fn brz(src: &str, file: &str) -> Vec<u8> {
    wirescript::compile(input(src, file)).unwrap_or_else(|e| panic!("should compile to brz: {e:?}")).brz
}

/// Every diagnostic code compiling `src` produces, whether it compiles or
/// fails with errors.
pub fn compile_diag_codes(src: &str, file: &str) -> Vec<String> {
    match wirescript::compile(input(src, file)) {
        Ok(result) => result.diagnostics.iter().map(|d| d.code.clone()).collect(),
        Err(wirescript::CompileError::HasErrors(errors)) => errors.iter().map(|d| d.code.clone()).collect(),
        Err(wirescript::CompileError::Emit(e)) => panic!("unexpected emit error for src={src:?}: {e:?}"),
    }
}

/// `src` lowered, with no diagnostic from typecheck or lowering.
pub fn lowered(src: &str, file: &str) -> wirescript::ir::Module {
    let (_, tc, lowered) = run_stages(src, file, wirescript::lower::FoldMode::Auto);
    assert!(tc.diagnostics.is_empty(), "{:?}", tc.diagnostics);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    lowered.module
}

/// The property `name` of node `n`.
pub fn prop(n: &wirescript::ir::Node, name: &str) -> Option<wirescript::ir::Literal> {
    n.properties.get(&wirescript::intern::intern(name)).cloned()
}

/// Whether `c` is a text display.
pub fn is_text_display(c: &Box<dyn brdb::BrdbComponent>) -> bool {
    c.component_type().map(|t| t.to_string() == "Component_TextDisplay").unwrap_or(false)
}

/// The `.ws` fixtures in `tests/fixtures/<subdir>`, sorted. Panics when there
/// are none, so a suite cannot pass by finding nothing to check.
pub fn ws_fixtures(subdir: &str) -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(subdir);
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("fixtures/{subdir} must exist"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("ws"))
        .collect();
    out.sort();
    assert!(!out.is_empty(), "no fixtures found; the suite must not silently pass empty");
    out
}
