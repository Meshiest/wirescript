use super::ast::*;
use crate::bearilog::grammar;
use Literal::*;

#[test]
fn test_exprs() {
    let pe = grammar::ExprParser::new();
    assert!(pe.parse("true").is_ok());
    macro_rules! eq {
        ($p:pat => $s:expr) => {
            assert!(matches!(pe.parse($s), Ok($p)))
        };
        ($p:pat if $cond:expr => $s:expr) => {
            assert!(match pe.parse($s) {
                Ok($p) if $cond => true,
                Ok(v) => {
                    dbg!(v);
                    false
                }
                Err(e) => {
                    dbg!(e);
                    false
                }
            })
        };
    }
    macro_rules! seq {
        ($a:expr => $b:expr) => {
            assert_eq!($a, pe.parse($b).unwrap().to_string());
        };
    }

    eq!(AstExpr::Literal(Bool(true)) => "true");
    eq!(AstExpr::Literal(Bool(false)) => "false");
    eq!(AstExpr::Literal(Float(1.0)) => "1.0");
    eq!(AstExpr::Literal(Int(1)) => "1");
    eq!(AstExpr::Literal(Int(2)) => "(2)");
    eq!(AstExpr::Literal(Int(1)) => "0b1");
    eq!(AstExpr::Literal(Int(1)) => "0x1");
    eq!(AstExpr::Literal(Int(100)) => "1_00");
    eq!(AstExpr::Literal(Int(2)) => "0b1_0");
    eq!(AstExpr::Literal(Int(16)) => "0x1_0");
    eq!(AstExpr::Var(a) if a == "a" => "a");
    seq!("(and (band 1 2) 3)" => "1 & 2 && 3");
    seq!("(nand 1 2)" => "not (1 and 2)");
}

#[test]
fn test_stmts() {
    let ps = grammar::StmtParser::new();

    macro_rules! seq {
        ($a:expr => $b:expr) => {
            assert_eq!($a, ps.parse($b).unwrap().to_string());
        };
    }

    seq!("b = 1" => "b = 1");
    seq!("const b = 1" => "const b = 1");
    seq!("let b = 1" => "let b = 1");
    seq!("c, d = 1, 2" => "c, d = 1, 2");
    seq!("buffer c, d = 1, 2" => "buffer c, d = 1, 2");
    seq!("buffer a" => "buffer a");
    seq!("let c, d = 1, 2" => "let c, d = 1, 2");
    seq!("const c, d = 1, 2" => "const c, d = 1, 2");
}

#[test]
fn test_mod() {
    let pm = grammar::ModuleParser::new();

    macro_rules! seq {
        ($a:expr => $b:expr) => {
            assert_eq!($a, pm.parse($b).unwrap().to_string());
        };
    }

    seq!("module foo(a) -> b {\n  b = a\n}" => "module foo(a) -> b { b = a; }");
    seq!("module foo() -> b {\n  b = (and 1 2)\n}" => "module foo()-> b { b = 1 and 2; }");
    seq!("module foo() -> b {\n  b = (and 1 2)\n}" => "module foo() -> b {
            // This is a comment
            b = 1 and 2;
            /* This is also a comment */
        }");
}
