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

// --- parse ---

#[test]
fn parse_bare_await() {
    let r = wirescript::parse("in e: exec\non e { await e }", "test");
    assert!(r.diagnostics.is_empty(), "parse errors: {:?}", r.diagnostics);
}

#[test]
fn parse_let_await() {
    let r = wirescript::parse("in e: exec\nvar v: int = 0\non e { let x = await v on e }", "test");
    assert!(r.diagnostics.is_empty(), "parse errors: {:?}", r.diagnostics);
}

#[test]
fn parse_await_race() {
    let r = wirescript::parse("in a: exec\nin b: exec\non a { await a || b }", "test");
    assert!(r.diagnostics.is_empty(), "parse errors: {:?}", r.diagnostics);
}

// --- typecheck ---

#[test]
fn await_in_exec_no_error() {
    let src = "in e: exec\non e { await e }";
    let codes = diag_codes(src);
    assert!(!codes.iter().any(|c| c == "WS007"), "await in exec should not error: {codes:?}");
}


// --- compile ---

#[test]
fn bare_await_compiles() {
    assert!(compiles("\
in start: exec
in done: exec
on start {
  await done
}"));
}

#[test]
fn let_await_on_compiles() {
    assert!(compiles("\
in start: exec
in signal: exec
var value: int = 0
on start {
  let x = await value on signal
}"));
}

#[test]
fn await_race_compiles() {
    assert!(compiles("\
in start: exec
in a: exec
in b: exec
on start {
  await a || b
}"));
}

#[test]
fn sequential_awaits_compile() {
    assert!(compiles("\
in start: exec
in step1: exec
in step2: exec
var count: int = 0
on start {
  count = 1
  await step1
  count = 2
  await step2
  count = 3
}"));
}

#[test]
fn await_inside_if_compiles() {
    assert!(compiles("\
in start: exec
in done: exec
var flag: bool = true
on start {
  if flag {
    await done
    flag = false
  }
}"));
}

