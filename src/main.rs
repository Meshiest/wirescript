use lalrpop_util::lalrpop_mod;

pub mod ast;
pub mod compiler;
pub mod wires;

#[cfg(test)]
mod bearilog_test;
pub mod helpers;

lalrpop_mod!(pub bearilog);

fn main() {
    println!("Hello, world!");
}
