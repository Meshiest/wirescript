//! Repro: mods imported from another file fail to resolve module-level `let`
//! constants referenced inside their bodies (WS002 unknown identifier +
//! WS004 overload cascades), even when the constants are ALSO explicitly
//! imported. Reported 2026-07-01 compiling the wirescript repo's
//! projects/input files.

use wirescript::resolve::{resolve, FsLoader, MemLoader};
use wirescript::typecheck::typecheck;

fn diag_report(label: &str, source: &str, file: &str) {
    let resolved = resolve(source, file, &FsLoader);
    let tc = typecheck(&resolved.ast, file);
    eprintln!("=== {label}: {} resolve diags, {} tc diags", resolved.diagnostics.len(), tc.diagnostics.len());
    for d in resolved.diagnostics.iter().chain(tc.diagnostics.iter()).take(25) {
        eprintln!(
            "  [{}] {} ({}:{}:{})",
            d.code, d.message, d.range.file, d.range.start.line, d.range.start.col
        );
    }
}

#[test]
fn real_project_files_typecheck() {
    let base = r"C:\Users\cake\dev\brickadia\wirescript\projects\input";
    for name in ["lib.ws", "cursor.ws", "calibrate.ws", "test_cursor.ws"] {
        let path = format!("{base}\\{name}");
        let src = std::fs::read_to_string(&path).expect("read source");
        diag_report(name, &src, &path);
    }
}

/// Minimal shape: file B imports a mod from file A; the mod's body uses a
/// module-level `let` from A. Also imports the let explicitly.
#[test]
fn imported_mod_sees_defining_modules_lets() {
    let lib = r#"
let K = 3
mod Triple(v: int) -> (r: int) {
  out r = v * K
}
"#;
    let main = r#"
import { Triple, K } from "lib"
in x: int
let t = Triple(x)
out y: int = t.r
"#;
    let mut files = std::collections::HashMap::new();
    files.insert("lib.ws".to_string(), lib.to_string());
    let loader = MemLoader { files };

    let resolved = resolve(main, "main.ws", &loader);
    let tc = typecheck(&resolved.ast, "main.ws");
    let all: Vec<String> = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    eprintln!("resolved decl order:");
    for d in &resolved.ast.decls {
        eprintln!("  {:?}", std::mem::discriminant(d));
    }
    assert!(all.is_empty(), "expected clean typecheck, got: {all:#?}");
}

/// Ordering probe A: same as above but with the let imported FIRST. If this
/// passes while the mod-first order fails, typecheck is order-sensitive.
#[test]
fn imported_let_before_mod_order_probe() {
    let lib = r#"
let K = 3
mod Triple(v: int) -> (r: int) {
  out r = v * K
}
"#;
    let main = r#"
import { K, Triple } from "lib"
in x: int
let t = Triple(x)
out y: int = t.r
"#;
    let mut files = std::collections::HashMap::new();
    files.insert("lib.ws".to_string(), lib.to_string());
    let loader = MemLoader { files };
    let resolved = resolve(main, "main.ws", &loader);
    let tc = typecheck(&resolved.ast, "main.ws");
    let all: Vec<String> = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    assert!(all.is_empty(), "let-first order: {all:#?}");
}

/// Ordering probe B: single file, mod textually BEFORE the let it uses. If
/// this fails, top-level typecheck is order-sensitive in general (not an
/// import bug per se).
#[test]
fn same_file_mod_before_let_order_probe() {
    let src = r#"
mod Triple(v: int) -> (r: int) {
  out r = v * K
}
let K = 3
in x: int
let t = Triple(x)
out y: int = t.r
"#;
    let parsed = wirescript::parse(src, "t.ws");
    let tc = typecheck(&parsed.ast, "t.ws");
    let all: Vec<String> = parsed
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    assert!(all.is_empty(), "same-file mod-before-let: {all:#?}");
}

/// Same but WITHOUT importing K explicitly — the resolver's fixed-point
/// puller should bring K along because Triple's body references it.
#[test]
fn puller_brings_lets_used_by_imported_mod() {
    let lib = r#"
let K = 3
mod Triple(v: int) -> (r: int) {
  out r = v * K
}
"#;
    let main = r#"
import { Triple } from "lib"
in x: int
let t = Triple(x)
out y: int = t.r
"#;
    let mut files = std::collections::HashMap::new();
    files.insert("lib.ws".to_string(), lib.to_string());
    let loader = MemLoader { files };

    let resolved = resolve(main, "main.ws", &loader);
    let tc = typecheck(&resolved.ast, "main.ws");
    let all: Vec<String> = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect();
    assert!(all.is_empty(), "expected clean typecheck, got: {all:#?}");
}
