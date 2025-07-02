use super::{compiler::Compiler, grammar, graphviz};

#[test]
fn compile_and() {
    let p = grammar::ModuleParser::new();
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
    let p = grammar::ModuleParser::new();
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
    let p = grammar::ModuleParser::new();
    // copyright: smallguy
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
    println!("Source:\n{source}\n\n{}", graphviz::render(&out).unwrap());
}

#[test]
fn compile_7seg2() {
    let p = grammar::ModuleParser::new();
    // copyright: smallguy
    let source = "
module decoder7seg(a0, a1, a2, a3) -> a, b, c, d, e, f, g {
    const xor1 = a3 ^ a1;
    const xor2 = xor1 ^ a2;
    const and1 = a0 & a2;
    const xor3 = a1 ^ and1;
    const nor1 = !(xor2 | xor3);
    const nor2 = !(nor1 | xor1);
    const or1 = nor2 | a0;
    const xor4 = a0 ^ a3;
    const xor5 = xor4 ^ xor2;
    const nor3 = !(xor5 | a3);
    const xor6 = nor3 ^ a3;
    const and3 = nor2 & a0;
    const and4 = xor6 & a2;
    const or3 = and1 | xor4;
    const or4 = nor2 | or3;
    const nor5 = !(xor3 | and1);
    const nor6 = !(or4 | nor5);
    const nor7 = !(nor5 | a3);
    a = !(xor6 | nor7);
    b = and3 | and4;
    c = and4 ^ nor6;
    d = or1 & xor5;
    e = or4 ^ a3;
    f = !(nor2 | xor6);
    g = nor1;
}
    "
    .trim();
    let module = p.parse(source).expect("Failed to parse module");
    let mut c = Compiler::new([module]);
    let out = c.compile("decoder7seg").expect("Failed to compile module");
    println!(
        "Source:\n{source}\n\n{}\n\n{out}",
        graphviz::render(&out).unwrap()
    );
}

#[test]
fn compile_encoder() {
    let p = grammar::ModuleParser::new();
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
        graphviz::render(&out1).unwrap(),
        graphviz::render(&out2).unwrap()
    );
}
