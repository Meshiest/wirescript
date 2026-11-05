//! `@test` support for a test runner: list a file's tests, and compile a
//! program with one of them called when the grid loads.

use std::sync::Arc;

use crate::ast::{
    Block, CallArg, ChipDecl, Expr, ExprStmt, Handler, Script, Stmt, TopDecl, Trigger,
};
use crate::diagnostic::{Diagnostic, SourceRange};

/// The chat line a test entry prints once the test's body has finished. A
/// runner that never sees it within its tick budget fails the test.
pub const TEST_DONE: &str = "[wirescript test done]";

/// The declaration a test is written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKind {
    /// `@test fn`: runs in its own function plane, synchronously.
    Fn,
    /// `@test mod`: inlined into the load handler; may read module state and
    /// `await`.
    Mod,
    /// `@test chip`: a microchip joined to the load handler's chain.
    Chip,
}

impl TestKind {
    fn of(c: &ChipDecl) -> Self {
        if c.is_fn {
            Self::Fn
        } else if c.inline {
            Self::Mod
        } else {
            Self::Chip
        }
    }
}

/// One `@test` declaration of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFn {
    pub name: String,
    pub kind: TestKind,
    /// The whole declaration, from its keyword to its closing `}`.
    pub range: SourceRange,
}

/// The `@test` declarations `source` holds, in source order, read from a
/// parse alone. A file that does not parse cleanly still lists the tests the
/// parser recovered.
pub fn list_tests(source: &str, file: &str) -> Vec<TestFn> {
    crate::parser::parse(source, file)
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Chip(c) if c.is_test => Some(TestFn {
                name: c.name.clone(),
                kind: TestKind::of(c),
                range: c.range.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Add `on Loaded() { name()  BroadcastChatMessage(TEST_DONE) }` to `ast`,
/// where `name` is a `@test` declared in the entry file `file`. The handler
/// sits at the end of the entry file, so the call resolves as code written
/// there, and the sentinel prints once the test's chain has run to its end.
pub(crate) fn add_test_call(ast: &mut Script, file: &str, name: &str) -> Result<(), Diagnostic> {
    let entry = |c: &&ChipDecl| c.name == name && c.range.file.as_ref() == file;
    let found = ast
        .decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Chip(c) => Some(c),
            _ => None,
        })
        .rfind(entry);
    let Some(test) = found.filter(|c| c.is_test) else {
        let message = match found {
            Some(c) => format!(
                "test entry `{name}` is a `{}` without @test",
                c.kind_label()
            ),
            None => format!("test entry `{name}`: this file declares no `@test` named `{name}`"),
        };
        return Err(Diagnostic::error("WS125", message, ast.range.clone()));
    };
    let at = SourceRange {
        file: Arc::clone(&test.range.file),
        start: ast.range.end,
        end: ast.range.end,
    };
    let call = |callee: &str, args: Vec<CallArg>| {
        Stmt::ExprStmt(ExprStmt {
            expr: Expr::Call {
                callee: Box::new(Expr::Ident {
                    name: callee.to_string(),
                    range: at.clone(),
                }),
                args,
                type_args: Vec::new(),
                range: at.clone(),
            },
            range: at.clone(),
        })
    };
    let done = CallArg::Positional(Expr::StringLit {
        value: TEST_DONE.to_string(),
        range: at.clone(),
    });
    let stmts = vec![
        call(name, Vec::new()),
        call("BroadcastChatMessage", vec![done]),
    ];
    ast.decls.push(TopDecl::Handler(Handler {
        trigger: Trigger::Ident {
            name: "Loaded".to_string(),
            range: at.clone(),
        },
        params: Vec::new(),
        config: Vec::new(),
        body: Block {
            stmts,
            range: at.clone(),
        },
        no_fold: false,
        expr_trigger_has_exec_arg: false,
        alias: None,
        range: at,
    }));
    Ok(())
}
