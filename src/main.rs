use lalrpop_util::lalrpop_mod;

pub mod ast;
pub mod helpers;

#[cfg(test)]
mod bearilog_test;

lalrpop_mod!(pub bearilog);

fn main() {
    println!("Hello, world!");
}
