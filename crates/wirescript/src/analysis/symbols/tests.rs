use super::*;
use crate::resolve::{FsLoader, MemLoader, resolve};
use crate::typecheck::typecheck;

#[test]
fn namespace_import_collects_qualified_members() {
    // `import * as u` must yield a `u` namespace symbol plus qualified
    // `u.<member>` symbols for the module's importable decls, so member
    // completion after `u.` can find them.
    let loader = MemLoader {
        files: [(
            "lib.ws".to_string(),
            "mod swap(a: int) {}\nlet PI = 3\ntype Pt = { x: int }".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let resolved = resolve("import * as u from \"lib\"", "main", &loader);
    let tc = typecheck(
        &resolved.ast,
        "main",
        &crate::typecheck::CeSlotMap::default(),
    );
    let syms = collect_symbols(&resolved.ast, &tc.type_of_expr);
    assert!(
        syms.iter().any(|s| s.name == "u" && s.kind == "namespace"),
        "namespace alias symbol missing: {:?}",
        syms.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    for (m, k) in [("u.swap", "mod"), ("u.PI", "let"), ("u.Pt", "type")] {
        assert!(
            syms.iter().any(|s| s.name == m && s.kind == k),
            "namespace member {m} ({k}) missing"
        );
    }
}

/// The `exec` flag the LSP hover shows for the mod/chip named `name`.
fn mod_exec(source: &str, name: &str) -> bool {
    let resolved = resolve(source, "test", &FsLoader);
    let tc = typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    collect_symbols(&resolved.ast, &tc.type_of_expr)
        .into_iter()
        .find(|s| s.name == name && (s.kind == "mod" || s.kind == "chip"))
        .unwrap_or_else(|| panic!("no mod/chip symbol named {name}"))
        .exec
}

/// Whether a mod reads as exec: each row's source, mod name, and the flag.
#[test]
fn a_mod_is_exec_only_when_its_body_is() {
    let rows: &[(&str, &str, &str, bool)] = &[
        // `return <expr>` was flagged exec unconditionally, so a pure
        // single-output mod (only comparisons + literals) read as exec.
        (
            "a pure return-expr mod",
            "mod band(n: int) -> int {\n  return if n >= 22 then 4 else if n >= 11 then 1 else 0\n}",
            "band",
            false,
        ),
        // `return arr[i]` reads an array -> Exec_ArrayVar_Get.
        (
            "an array-index return",
            "var xs: int[]\nmod at(i: int) -> int {\n  return xs[i]\n}",
            "at",
            true,
        ),
        // A bare early `return` is exec-chain control flow.
        ("a bare return", "mod f(x: int) {\n  return\n}", "f", true),
        (
            "an if statement",
            "var xs: int[]\nmod g(x: int) {\n  if x > 0 { xs.push(x) }\n}",
            "g",
            true,
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, name, exec) in rows {
        if mod_exec(src, name) != *exec {
            failures.push(format!("{label}: `{name}` exec should be {exec}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Completion offers whatever `collect_symbols_for_file` returns. A port
/// declared inside a handler, or by a file-scope anon chip, is a real
/// boundary port after hoisting, so it must appear like any top-level
/// `out`.
#[test]
fn a_hoisted_port_is_collected_as_a_symbol() {
    let rows = [
        (
            "a handler-declared port",
            "on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}",
            "flash",
        ),
        (
            "an anon-chip-declared port",
            "chip { out shared: int = 1 }",
            "shared",
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, name) in rows {
        let resolved = resolve(src, "test", &FsLoader);
        let tc = typecheck(
            &resolved.ast,
            "test",
            &crate::typecheck::CeSlotMap::default(),
        );
        let syms = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
        if !syms.iter().any(|s| s.name == name && s.kind == "out") {
            failures.push(format!(
                "{label}: no `out {name}` in {:?}",
                syms.iter().map(|s| (&s.name, s.kind)).collect::<Vec<_>>()
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn resolve_symbol_reads_a_char_column_against_byte_ranges() {
    // Symbol ranges carry the lexer's BYTE columns and the cursor arrives
    // as a char column, so enough non-ASCII text before a declaration on
    // its own line made that declaration look like it came AFTER the
    // cursor: hover on the local `v` fell back to the file-scope one and
    // reported `string` for an `int`.
    let src = "\
let v: string = \"s\"
in go: exec
on go {
  /* \u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9} */ let v: int = 1
}";
    let resolved = crate::resolve::resolve(src, "test", &crate::resolve::FsLoader);
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let syms = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));
    let l = src.lines().nth(3).unwrap();
    let col = l[..l.rfind('v').unwrap()].chars().count();
    let sym = resolve_symbol(&syms, src, "v", 3, col).expect("a `v` is in scope");
    assert_eq!(
        sym.ty.as_deref(),
        Some("int"),
        "the declaration under the cursor, not the file-scope one"
    );
}
