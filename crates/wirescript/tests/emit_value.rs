use wirescript::resolve::{resolve, FsLoader};
use wirescript::typecheck::typecheck;
use wirescript::{compile, CompileInput, FoldMode};

fn diag_codes(src: &str) -> Vec<String> {
    let resolved = resolve(src, "test", &FsLoader);
    let tc = typecheck(&resolved.ast, "test");
    tc.diagnostics.iter().map(|d| d.code.clone()).collect()
}

fn compiles(src: &str) -> bool {
    compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto }).is_ok()
}

// --- emit target = expr ---

#[test]
fn emit_value_in_exec_no_error() {
    let src = "out result: int\nin start: exec\non start { emit result = 42 }";
    let codes = diag_codes(src);
    assert!(!codes.contains(&"WS007".into()), "emit value in exec should not error: {codes:?}");
}

#[test]
fn emit_value_in_pure_no_error() {
    // emit-value inside a pure chip body
    let src = "chip Foo(x: int) -> (status: int) { emit status = x }";
    let codes = diag_codes(src);
    assert!(!codes.contains(&"WS007".into()), "emit value in pure should not error: {codes:?}");
}

#[test]
fn emit_no_value_in_exec_works() {
    let src = "out result: int\nin start: exec\non start { emit result }";
    let codes = diag_codes(src);
    assert!(!codes.contains(&"WS007".into()), "bare emit in exec should not error: {codes:?}");
}

#[test]
fn emit_value_compiles_in_exec() {
    assert!(compiles("out result: int\nin start: exec\non start { emit result = 42 }"));
}

#[test]
fn emit_value_compiles_in_exec_context() {
    assert!(compiles("out r: int\nin s: exec\non s { emit r = 42 }"));
}

#[test]
fn emit_value_with_expression() {
    assert!(compiles("out r: int\nin s: exec\nvar x: int = 0\non s { emit r = x + 1 }"));
}
