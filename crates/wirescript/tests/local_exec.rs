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

#[test]
fn let_exec_declaration() {
    assert!(compiles("let done: exec"));
}

#[test]
fn single_emit_to_local_exec() {
    assert!(compiles("\
let done: exec
in start: exec
on start { emit done }"));
}

#[test]
fn multiple_emit_sites_union() {
    assert!(compiles("\
let done: exec
in a: exec
in b: exec
on a { emit done }
on b { emit done }"));
}

#[test]
fn await_local_exec() {
    assert!(compiles("\
let done: exec
in start: exec
in trigger: exec
on trigger { emit done }
on start {
  await done
}"));
}

#[test]
fn emit_value_to_local_exec() {
    assert!(compiles("\
out result: int
let done: exec
in start: exec
var x: int = 42
on start { emit done }"));
}

#[test]
fn await_race_local_execs() {
    assert!(compiles("\
let a: exec
let b: exec
in start: exec
in t1: exec
in t2: exec
on t1 { emit a }
on t2 { emit b }
on start { await a || b }"));
}
