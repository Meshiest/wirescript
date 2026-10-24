//! Every example and test fixture program holds the wiring invariant: in a
//! debug build a violation in a program reporting no other error panics.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, files);
        } else if p.extension().is_some_and(|x| x == "ws") {
            files.push(p);
        }
    }
}

#[test]
fn every_example_and_fixture_holds_the_wiring_invariant() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["tests/fixtures", "../../examples", "../wasm/playground"] {
        walk(&root.join(dir), &mut files);
    }
    assert!(files.len() >= 50, "{}", files.len());
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let file = path.to_string_lossy();
        let diags = wirescript::diagnostics_only(
            wirescript::CompileInput {
                source: &source,
                file: &file,
                module_name: None,
                fold_mode: wirescript::FoldMode::Auto,
            },
            &wirescript::FsLoader,
        );
        assert!(
            !diags
                .iter()
                .any(|d| d.message.contains(wirescript::diagnostic::EXEC_INTO_VALUE)),
            "{file}: {diags:?}"
        );
    }
}
