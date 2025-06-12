use crate::{bearilog, compiler::Compiler};

#[test]
fn compile_and() {
    let p = bearilog::ModuleParser::new();
    let source = "
module example(a, b) -> c {
    c = a && b;
}"
    .trim();
    let module = p.parse(source).expect("Failed to parse module");
    let mut c = Compiler::new([module]);
    let _out = c.compile("example").expect("Failed to compile module");
    // println!("Source:\n{source}\n\n{out}");
}

#[test]
fn compile_multi_and() {
    let p = bearilog::ModuleParser::new();
    let source = "
module example(a, b, c, d) -> e {
    e = a && b && c && d && false;
}"
    .trim();
    let module = p.parse(source).expect("Failed to parse module");
    let mut c = Compiler::new([module]);
    let out = c.compile("example").expect("Failed to compile module");
    println!("Source:\n{source}\n\n{out}");
}
