use std::fmt::{Display, Write};

use crate::brdb::schema::WireVariant;

use super::helpers::fmt_iter;

#[derive(Debug, Copy, Clone)]
pub enum Literal {
    Float(f64),
    Int(i64),
    Bool(bool),
}

impl Literal {
    pub fn variant(self) -> WireVariant {
        match self {
            Literal::Float(v) => WireVariant::Number(v),
            Literal::Int(v) => WireVariant::Int(v),
            Literal::Bool(v) => WireVariant::Bool(v),
        }
    }
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

#[derive(Debug, Clone, Copy)]
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
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Eq,
    Neq,
    Lt,
    Leq,
    Gt,
    Geq,
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
            BinaryOpCode::Mul => "mul",
            BinaryOpCode::Div => "div",
            BinaryOpCode::Mod => "mod",
            BinaryOpCode::Add => "add",
            BinaryOpCode::Sub => "sub",
            BinaryOpCode::Eq => "eq",
            BinaryOpCode::Neq => "neq",
            BinaryOpCode::Lt => "lt",
            BinaryOpCode::Leq => "leq",
            BinaryOpCode::Gt => "gt",
            BinaryOpCode::Geq => "geq",
        })
    }
}

#[derive(Debug, Clone, Copy)]
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
    Literal(Literal),
    Var(String),
    BinaryOp(BinaryOpCode, Box<AstExpr>, Box<AstExpr>),
    UnaryOp(UnaryOpCode, Box<AstExpr>),
    Call(String, Vec<AstExpr>),
}

impl Display for AstExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AstExpr::Literal(v) => v.fmt(f),
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
            AstExpr::Literal(Literal::Bool(b)) => AstExpr::Literal(Literal::Bool(!b)),
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
    // a = 1
    Assign(Vec<String>, Vec<AstExpr>),
    // const a = 1
    Const(Vec<String>, Vec<AstExpr>),
    // let a = 1
    Let(Vec<String>, Vec<AstExpr>),
    // buffer a = 1
    // buffer foo // assigned later
    Buffer(Vec<String>, Option<Vec<AstExpr>>),
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
            AstStmt::Let(names, exprs) => {
                f.write_str("let ")?;
                fmt_iter(f, names.iter(), ", ")?;
                f.write_str(" = ")?;
                fmt_iter(f, exprs.iter(), ", ")?;
                Ok(())
            }
            AstStmt::Buffer(names, exprs) => {
                f.write_str("buffer ")?;
                fmt_iter(f, names.iter(), ", ")?;
                if let Some(exprs) = exprs {
                    f.write_str(" = ")?;
                    fmt_iter(f, exprs.iter(), ", ")?;
                }
                Ok(())
            }
        }
    }
}

pub struct AstModule {
    pub name: String,
    pub inline: bool,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub statements: Vec<AstStmt>,
}

impl AstModule {
    pub fn new(
        name: &str,
        inline: bool,
        inputs: Vec<String>,
        outputs: Vec<String>,
        statements: Vec<AstStmt>,
    ) -> Self {
        Self {
            name: name.to_string(),
            inline,
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
