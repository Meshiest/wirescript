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

#[test]
fn compile_7seg() {
    let p = bearilog::ModuleParser::new();
    let source = "
module decoder7seg(a0, a1, a2, a3) -> a, b, c, d, e, f, g {
    a = a3 || (a2 && a0) || (a1 && !a0) || (!a2 && !a0);
    b = (!a2 && !a0) || (!a1 && a0) || (a1 && !a0) || (a2 && !a1);
    c = (!a2 && a0) || (!a1 && a0) || (a2 && !a1) || a3 || (a1 && !a0);
    d = (!a1 && !a0) || (a2 && !a1 && a0) || (!a2 && a1 && !a0) || (!a2 && a1 && a0) || (a3 && !a0);
    e = !a0 || (a2 && !a1) || (a3 && a2);
    f = (a1 && !a0) || (!a2 && !a0) || (a3 && a2) || (a3 && a1) || (a2 && !a1);
    g = (!a2 && a1) || (a1 && !a0) || (a3 && a2) || (a2 && a0) || (a3 && a1);
}
    "
    .trim();
    let module = p.parse(source).expect("Failed to parse module");
    let mut c = Compiler::new([module]);
    let out = c.compile("decoder7seg").expect("Failed to compile module");
    println!("Source:\n{source}\n\n{out}");
}
