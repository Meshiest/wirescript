//! `$./file.brz` prefab references embed the referenced prefab and point the
//! SpawnPrefab gate's `Prefab` property at the embedded copy.

use brdb::IntoReader;
use std::sync::Arc;
use wirescript::{
    CompileInput, EmitOptions, FoldMode, NestedCompiler, PrefabResolver, compile_to_world,
};

/// Build a trivial single-brick prefab's `.brz` bytes to stand in for a
/// dragged-in / on-disk prefab file.
fn inner_prefab_bytes() -> Vec<u8> {
    let mut inner = brdb::World::new();
    inner.bricks.push(brdb::Brick::default());
    inner.make_prefab();
    inner.to_brz_vec().unwrap()
}

#[test]
fn spawn_prefab_embeds_and_references_prefab() {
    let inner = inner_prefab_bytes();

    // In-memory resolver: `$./turret.brz` -> the inner prefab bytes.
    let inner_for_resolver = inner.clone();
    let resolver = PrefabResolver(Arc::new(move |path: &str| {
        if path == "./turret.brz" {
            Ok(inner_for_resolver.clone())
        } else {
            Err(format!("unknown prefab {path}"))
        }
    }));

    let src = "\
in start: exec
on start {
  SpawnPrefab(prefab = $./turret.brz, lifetime = 5.0)
}
";
    let opts = EmitOptions {
        prefab_resolver: Some(resolver),
        ..Default::default()
    };
    let result = compile_to_world(
        CompileInput { source: src, file: "test.ws", module_name: None, fold_mode: FoldMode::Auto },
        opts,
    )
    .expect("compile should succeed");

    // Exactly one prefab, embedded content-addressed at Prefabs/Uploads/, with
    // the exact referenced bytes.
    let world = result.world;
    let embedded: Vec<(&String, &Vec<u8>)> = world.prefabs.iter().collect();
    assert_eq!(embedded.len(), 1, "expected one embedded prefab");
    let (path, bytes) = embedded[0];
    assert!(
        path.starts_with("Prefabs/Uploads/") && path.ends_with(".brz"),
        "embedded at unexpected path: {path}"
    );
    assert_eq!(bytes, &inner, "embedded bytes must equal the referenced .brz");

    // Round-trip through .brz: the reader enumerates the same embedded path
    // (the spawner component's `Prefab` bundle_path_ref points at it).
    let expected_path = path.clone();
    let out = world.to_brz_vec().unwrap();
    let reader = brdb::Brz::read_slice(&out).unwrap().into_reader();
    assert_eq!(reader.prefab_paths().unwrap(), vec![expected_path]);
}

#[test]
fn prefab_ref_requires_brz_extension() {
    // `$./turret` (no .brz) is a typecheck error (WS019).
    let src = "\
in start: exec
on start {
  SpawnPrefab(prefab = $./turret, lifetime = 5.0)
}
";
    let resolver = PrefabResolver(Arc::new(|_: &str| Ok(Vec::new())));
    let opts = EmitOptions {
        prefab_resolver: Some(resolver),
        ..Default::default()
    };
    let msg = match compile_to_world(
        CompileInput { source: src, file: "test.ws", module_name: None, fold_mode: FoldMode::Auto },
        opts,
    ) {
        Ok(_) => panic!("missing .brz extension should fail"),
        Err(e) => format!("{e}"),
    };
    assert!(msg.contains("WS019") || msg.contains(".brz"), "got: {msg}");
}

#[test]
fn prefab_ref_without_resolver_errors() {
    let src = "\
in start: exec
on start {
  SpawnPrefab(prefab = $./turret.brz)
}
";
    // Default (disk) resolver against a nonexistent file: emit fails with a
    // clear error rather than silently dropping the reference.
    let msg = match compile_to_world(
        CompileInput { source: src, file: "nonexistent.ws", module_name: None, fold_mode: FoldMode::Auto },
        EmitOptions::default(),
    ) {
        Ok(_) => panic!("missing prefab file should fail"),
        Err(e) => format!("{e}"),
    };
    assert!(msg.contains("turret.brz"), "got: {msg}");
}

#[test]
fn nested_prefab_embeds_compiled_inner() {
    let inner = inner_prefab_bytes();
    let inner_for = inner.clone();
    let opts = EmitOptions {
        nested_compiler: Some(NestedCompiler::new(move |src: &str, _depth: usize| {
            assert!(src.contains("in q: exec"), "inner source passed through: {src}");
            Ok(inner_for.clone())
        })),
        ..Default::default()
    };
    let src = "in start: exec\non start { SpawnPrefab($```in q: exec```) }\n";
    let world = compile_to_world(
        CompileInput {
            source: src,
            file: "test.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        opts,
    )
    .expect("compile should succeed")
    .world;
    assert_eq!(world.prefabs.iter().count(), 1, "exactly one embedded prefab");
}

#[test]
fn nested_prefab_compiles_end_to_end_without_explicit_compiler() {
    // No caller-provided nested_compiler: the driver supplies a recursive one.
    let src = "in start: exec\non start { SpawnPrefab($```in q: exec\non q { }```) }\n";
    let world = compile_to_world(
        CompileInput { source: src, file: "test.ws", module_name: None, fold_mode: FoldMode::Auto },
        EmitOptions::default(),
    )
    .expect("compile should succeed")
    .world;
    assert_eq!(world.prefabs.iter().count(), 1, "one embedded prefab");
}

#[test]
fn nested_prefab_depth_guard_trips() {
    // A block nested past the cap fails with a clear error, not a hang/overflow.
    let mut inner = "in q: exec".to_string();
    for _ in 0..9 {
        inner = format!("in go: exec\non go {{ SpawnPrefab($```{inner}```) }}");
    }
    let src = format!("in go: exec\non go {{ SpawnPrefab($```{inner}```) }}\n");
    // `CompileWorldResult` embeds `brdb::World` (no `Debug` impl), so
    // `expect_err` can't be used here — match instead, as the resolver tests
    // above do.
    let msg = match compile_to_world(
        CompileInput { source: &src, file: "t.ws", module_name: None, fold_mode: FoldMode::Auto },
        EmitOptions::default(),
    ) {
        Ok(_) => panic!("deeply nested prefab should fail"),
        Err(e) => format!("{e}").to_lowercase(),
    };
    assert!(msg.contains("nest") || msg.contains("deep"), "depth-guard message: {msg}");
}

#[test]
fn nested_prefab_brz_path_compiles_without_explicit_compiler() {
    // The public `.brz` entry (`compile`, used by the CLI / VS Code build) must
    // also supply a default nested compiler, not just the `.brz`-less World path.
    let src = "in start: exec\non start { SpawnPrefab($```in q: exec```) }\n";
    let result = wirescript::compile(CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("brz compile should succeed");
    // The emitted `.brz` embeds exactly one prefab (the compiled inner block).
    let reader = brdb::Brz::read_slice(&result.brz).unwrap().into_reader();
    assert_eq!(reader.prefab_paths().unwrap().len(), 1, "one embedded prefab");
}
