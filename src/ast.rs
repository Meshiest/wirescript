use std::fmt::{Display, Write};

use crate::helpers::fmt_iter;

#[derive(Debug)]
pub enum Literal {
    Float(f64),
    Int(i64),
    Bool(bool),
}

impl Display for Literal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Float(v) => v.fmt(f),
            Self::Int(v) => v.fmt(f),
            Self::Bool(v) => v.fmt(f),
        }
    }
}

#[derive(Debug)]
pub enum BinaryOpCode {
    BoolAnd,
    BoolNand,
    BoolNor,
    BoolOr,
    BoolXor,
    BitAnd,
    BitNand,
    BitNor,
    BitOr,
    BitXor,
    BitShiftLeft,
    BitShiftRight,
}

impl Display for BinaryOpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BinaryOpCode::BoolAnd => "and",
            BinaryOpCode::BoolNand => "nand",
            BinaryOpCode::BoolNor => "nor",
            BinaryOpCode::BoolOr => "or",
            BinaryOpCode::BoolXor => "xor",
            BinaryOpCode::BitAnd => "band",
            BinaryOpCode::BitNand => "bnand",
            BinaryOpCode::BitNor => "bnor",
            BinaryOpCode::BitOr => "bor",
            BinaryOpCode::BitXor => "bxor",
            BinaryOpCode::BitShiftLeft => "shl",
            BinaryOpCode::BitShiftRight => "shr",
        })
    }
}

#[derive(Debug)]
pub enum UnaryOpCode {
    BoolNot,
    BitNot,
}

impl Display for UnaryOpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UnaryOpCode::BoolNot => "not",
            UnaryOpCode::BitNot => "bnot",
        })
    }
}

#[derive(Debug)]
pub enum AstExpr {
    Const(Literal),
    Var(String),
    BinaryOp(BinaryOpCode, Box<AstExpr>, Box<AstExpr>),
    UnaryOp(UnaryOpCode, Box<AstExpr>),
    Call(String, Vec<AstExpr>),
}

impl Display for AstExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AstExpr::Const(v) => v.fmt(f),
            AstExpr::Var(v) => f.write_str(v),
            AstExpr::BinaryOp(op, l, r) => write!(f, "({op} {l} {r})"),
            AstExpr::UnaryOp(op, v) => write!(f, "({op} {v})"),
            AstExpr::Call(n, exprs) => {
                f.write_str(n)?;
                f.write_char('(')?;
                fmt_iter(f, exprs.iter(), ", ")?;
                f.write_char(')')
            }
        }
    }
}

impl AstExpr {
    pub fn bool_not(self) -> Self {
        match self {
            AstExpr::BinaryOp(BinaryOpCode::BoolAnd, l, r) => {
                AstExpr::BinaryOp(BinaryOpCode::BoolNand, l, r)
            }
            AstExpr::Const(Literal::Bool(b)) => AstExpr::Const(Literal::Bool(!b)),
            AstExpr::BinaryOp(BinaryOpCode::BoolOr, l, r) => {
                AstExpr::BinaryOp(BinaryOpCode::BoolNor, l, r)
            }
            e => AstExpr::UnaryOp(UnaryOpCode::BoolNot, Box::new(e)),
        }
    }

    pub fn bit_not(self) -> Self {
        match self {
            AstExpr::BinaryOp(BinaryOpCode::BitAnd, l, r) => {
                AstExpr::BinaryOp(BinaryOpCode::BitNand, l, r)
            }
            AstExpr::BinaryOp(BinaryOpCode::BitOr, l, r) => {
                AstExpr::BinaryOp(BinaryOpCode::BitNor, l, r)
            }
            e => AstExpr::UnaryOp(UnaryOpCode::BitNot, Box::new(e)),
        }
    }
}

#[derive(Debug)]
pub enum AstStmt {
    Assign(Vec<String>, Vec<AstExpr>),
    Const(Vec<String>, Vec<AstExpr>),
}

impl Display for AstStmt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AstStmt::Assign(names, exprs) => {
                fmt_iter(f, names.iter(), ", ")?;
                f.write_str(" = ")?;
                fmt_iter(f, exprs.iter(), ", ")?;
                Ok(())
            }
            AstStmt::Const(names, exprs) => {
                f.write_str("const ")?;
                fmt_iter(f, names.iter(), ", ")?;
                f.write_str(" = ")?;
                fmt_iter(f, exprs.iter(), ", ")?;
                Ok(())
            }
        }
    }
}

pub struct AstModule {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub statements: Vec<AstStmt>,
}

impl AstModule {
    pub fn new(
        name: &str,
        inputs: Vec<String>,
        outputs: Vec<String>,
        statements: Vec<AstStmt>,
    ) -> Self {
        Self {
            name: name.to_string(),
            inputs,
            outputs,
            statements,
        }
    }
}

impl Display for AstModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "module {}", self.name)?;
        if self.inputs.is_empty() {
            f.write_str("()")?;
        } else {
            f.write_char('(')?;
            fmt_iter(f, self.inputs.iter(), ", ")?;
            f.write_char(')')?;
        }

        // Outputs should never be empty either way
        if !self.outputs.is_empty() {
            f.write_str(" -> ")?;
            fmt_iter(f, self.outputs.iter(), ", ")?;
            f.write_char(' ')?;
        }

        f.write_str("{\n")?;
        for s in &self.statements {
            writeln!(f, "  {s}")?;
        }
        f.write_char('}')
    }
}
