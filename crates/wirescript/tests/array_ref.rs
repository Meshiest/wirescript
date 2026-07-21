use wirescript::{compile, CompileInput, FoldMode};

fn compiles(src: &str) -> bool {
    compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto }).is_ok()
}

#[test]
fn out_array_binding() {
    assert!(compiles("\
array framebuf: int[]
out display: int[] = framebuf"));
}

#[test]
fn let_array_binding() {
    assert!(compiles("\
array data: int[]
let alias = data"));
}

#[test]
fn array_passed_to_chip() {
    assert!(compiles("\
array buf: int[]
chip Render(fb: int[]) -> () {
  in run: exec
  on run { fb.push(1) }
}
in start: exec
on start { Render(buf) }"));
}

#[test]
fn array_in_record() {
    assert!(compiles("\
array items: int[]
type State = { data: int[] }
let state: State = { data: items }"));
}

#[test]
fn out_array_compiles_to_brz() {
    let src = "\
array framebuf: int[]
out display: int[] = framebuf
in player: character
on player { framebuf.push(42) }";
    let r = compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto });
    assert!(r.is_ok(), "out array should compile: {:?}", r.err());
}
