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
fn compile_7seg1() {
    let p = bearilog::ModuleParser::new();
    // copywrite: smallguy
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
    println!("Source:\n{source}\n\n{}", out.digraph().unwrap());
}

#[test]
fn compile_7seg2() {
    let p = bearilog::ModuleParser::new();
    // copywrite: smallguy
    let source = "
module decoder7seg(a0, a1, a2, a3) -> a, b, c, d, e, f, g {
    const o1 = !a2 & !a0;
    const o2 = !a1 & a0;
    const o3 = a1 & !a0;
    const o4 = a2 & !a1;
    const o5 = !a2 & a0;
    const o6 = !a1 & !a0;
    const o7 = a3 & a2;
    const o8 = a3 & a1;
    const o9 = a2 & a0;
    const o10 = !a2 & a1;
    a = a3 | (a2 & a0) | o3 | o1;
    b = o1 | o2 | o3 | o4;
    c = o5 | o2 | o4 | a3 | o3;
    d = o6 | (a2 & !a1 & a0) | (o10 & !a0) | (o10 & a0) | (a3 & !a0);
    e = !a0 | o4 | o7;
    f = o3 | o1 | o7 | o8 | o4;
    g = o10 | o3 | o7 | o9 | o8;
}
    "
    .trim();
    let module = p.parse(source).expect("Failed to parse module");
    let mut c = Compiler::new([module]);
    let out = c.compile("decoder7seg").expect("Failed to compile module");
    println!("Source:\n{source}\n\n{}\n\n{out}", out.digraph().unwrap());
}

#[test]
fn compile_encoder() {
    let p = bearilog::ModuleParser::new();
    let source1 = "
module decoder8(a) -> a0, a1, a2, a3, a4, a5, a6, a7 {
  a0 = (a & 1) ^^ 0;
  a1 = (a & 2) ^^ 0;
  a2 = (a & 4) ^^ 0;
  a3 = (a & 8) ^^ 0;
  a4 = (a & 16) ^^ 0;
  a5 = (a & 32) ^^ 0;
  a6 = (a & 64) ^^ 0;
  a7 = (a & 128) ^^ 0;
}
    "
    .trim();
    let source2 = "
module encoder8(a0, a1, a2, a3, a4, a5, a6, a7) -> a {
  a =
    a0 |
    (a1 << 1) |
    (a2 << 2) |
    (a3 << 3) |
    (a4 << 4) |
    (a5 << 5) |
    (a6 << 6) |
    (a7 << 7);
}
    "
    .trim();
    let mut c = Compiler::new([
        p.parse(source1).expect("Failed to parse module"),
        p.parse(source2).expect("Failed to parse module"),
    ]);
    let out1 = c.compile("decoder8").expect("Failed to compile module");
    let out2 = c.compile("encoder8").expect("Failed to compile module");
    println!(
        "Source:\n{source1}\n\n{}\n\nSource:\n{source2}\n\n{}",
        out1.digraph().unwrap(),
        out2.digraph().unwrap()
    );
}
