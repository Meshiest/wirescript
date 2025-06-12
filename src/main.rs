use lalrpop_util::lalrpop_mod;

pub mod ast;
pub mod helpers;

lalrpop_mod!(pub bearilog);

fn main() {
    println!("Hello, world!");
}

#[cfg(test)]
mod test_grammar {
    use crate::{ast::*, bearilog};
    use AstExpr::*;
    use Literal::*;

    #[test]
    fn test_exprs() {
        let pe = bearilog::ExprParser::new();
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

        eq!(Const(Bool(true)) => "true");
        eq!(Const(Bool(false)) => "false");
        eq!(Const(Float(1.0)) => "1.0");
        eq!(Const(Int(1)) => "1");
        eq!(Const(Int(2)) => "(2)");
        eq!(Const(Int(1)) => "0b1");
        eq!(Const(Int(1)) => "0x1");
        eq!(Const(Int(100)) => "1_00");
        eq!(Const(Int(2)) => "0b1_0");
        eq!(Const(Int(16)) => "0x1_0");
        eq!(Var(a) if a == "a" => "a");
        seq!("(and (band 1 2) 3)" => "1 & 2 && 3");
        seq!("(nand 1 2)" => "not (1 and 2)");
    }

    #[test]
    fn test_stmts() {
        let ps = bearilog::StmtParser::new();

        macro_rules! seq {
            ($a:expr => $b:expr) => {
                assert_eq!($a, ps.parse($b).unwrap().to_string());
            };
        }

        seq!("a" => "a");
        seq!("b = 1" => "b = 1");
        seq!("const b = 1" => "const b = 1");
        seq!("c, d = 1, 2" => "c, d = 1, 2");
        seq!("const c, d = 1, 2" => "const c, d = 1, 2");
    }

    #[test]
    fn test_mod() {
        let pm = bearilog::ModuleParser::new();

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
}
