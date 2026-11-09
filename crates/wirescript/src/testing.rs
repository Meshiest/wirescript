//! `testing.*`: assertions that are code only in a test build.
//!
//! [`rewrite`] turns each `testing.*` statement of one parsed file into
//! ordinary code: an `if true { ... }` that binds the operands to `let`s and
//! prints a `FAIL <file>:<line>: ...` (or `LOG ...`) chat line. The `if`
//! keeps the statement's range, and its key is recorded so lowering can drop
//! it from a normal build. `toEqual` and `toContain` depend on the operand
//! types, so the first rewrite binds their operands only and leaves the
//! statement pending; [`complete`] finishes it from a type check.

use std::cell::Cell;
use std::sync::Arc;

use crate::ast::{
    Block, CallArg, ChipDecl, Expr, ExprStmt, Handler, If, ImportKind, InterpPart, LetBinding,
    LetDecl, MatchBody, Script, Stmt, TopDecl,
};
use crate::collections::{HashMap, HashSet};
use crate::diagnostic::{Diagnostic, Pos, RangeKey, SourceRange, display_path};
use crate::ir::Type;

/// The builtin namespace the statements are written in.
pub const TESTING: &str = "testing";

/// The prefix of the `let` names a rewrite binds operands to.
const LET_PREFIX: &str = "__testing_";

/// Synthetic offsets the code of one statement takes, and the more a pending
/// statement keeps for its completion, which makes a check per record field.
const STMT_BAND: usize = 1 << 8;
const PENDING_BAND: usize = 1 << 16;

/// One `testing` member or `expect` matcher, for completion and hover.
pub struct Member {
    pub name: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
}

/// The members of `testing`.
pub const MEMBERS: &[Member] = &[
    Member {
        name: "fail",
        signature: "testing.fail(args...)",
        doc: "Fails: prints `FAIL <file>:<line>: <args>`, the arguments joined by spaces.",
    },
    Member {
        name: "log",
        signature: "testing.log(args...)",
        doc: "Prints `LOG <file>:<line>: <args>`, the arguments joined by spaces.",
    },
    Member {
        name: "assert",
        signature: "testing.assert(cond, msg...)",
        doc: "Fails when `cond` is false, printing `msg`, or the condition's source when there is none.",
    },
    Member {
        name: "assertEq",
        signature: "testing.assertEq(a, b, msg...)",
        doc: "Fails when `a != b`, printing `a`'s source, the value expected and the value it had.",
    },
    Member {
        name: "expect",
        signature: "testing.expect(a).<matcher>(b)",
        doc: "Checks `a` with a matcher: `toBe`, `toEqual`, `toBeGreaterThan`, `toBeLessThan`, \
              `toBeCloseTo` or `toContain`, each negated by `.not`.",
    },
];

/// The matchers of `testing.expect(a)`.
pub const MATCHERS: &[Member] = &[
    Member {
        name: "toBe",
        signature: "testing.expect(a).toBe(b)",
        doc: "Fails when `a != b`.",
    },
    Member {
        name: "toEqual",
        signature: "testing.expect(a).toEqual(b)",
        doc: "Fails when a field of record `a` differs from `b`'s, nested records included; \
              for any other value, as `toBe`.",
    },
    Member {
        name: "toBeGreaterThan",
        signature: "testing.expect(a).toBeGreaterThan(b)",
        doc: "Fails unless `a > b`.",
    },
    Member {
        name: "toBeLessThan",
        signature: "testing.expect(a).toBeLessThan(b)",
        doc: "Fails unless `a < b`.",
    },
    Member {
        name: "toBeCloseTo",
        signature: "testing.expect(a).toBeCloseTo(b, digits = 2)",
        doc: "Fails when `abs(a - b) >= 10^-digits / 2`.",
    },
    Member {
        name: "toContain",
        signature: "testing.expect(a).toContain(b)",
        doc: "Fails when string `a` does not contain `b`, or array `a` has no element equal to `b`.",
    },
    Member {
        name: "not",
        signature: "testing.expect(a).not.<matcher>(b)",
        doc: "Negates the matcher that follows: `.not.toBe(b)` fails when `a == b`.",
    },
];

/// Whether `name` is one a rewrite binds an operand to.
pub fn is_testing_name(name: &str) -> bool {
    name.strip_prefix(LET_PREFIX).is_some_and(|rest| {
        rest.split_once('_').is_some_and(|(n, side)| {
            !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && matches!(side, "a" | "b")
        })
    })
}

/// What a `.` at the end of `head` (a line's text up to the dot) reads a
/// member of: `testing` itself, or `testing.expect(...)`, negated or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    Testing,
    Expect { negated: bool },
}

/// The receiver `head` ends with, read from the text alone.
pub fn receiver_before_dot(head: &str) -> Option<Receiver> {
    if ends_with_word(head, TESTING) {
        return Some(Receiver::Testing);
    }
    let (head, negated) = match head.strip_suffix(".not") {
        Some(h) => (h, true),
        None => (head, false),
    };
    let inner = head.strip_suffix(')')?;
    let mut depth = 0usize;
    let open = inner.char_indices().rev().find_map(|(i, c)| match c {
        ')' => {
            depth += 1;
            None
        }
        '(' if depth == 0 => Some(i),
        '(' => {
            depth -= 1;
            None
        }
        _ => None,
    })?;
    let callee = inner[..open].trim_end();
    let before = callee.strip_suffix("expect")?.strip_suffix('.')?;
    ends_with_word(before, TESTING).then_some(Receiver::Expect { negated })
}

/// Whether `text` ends with the whole word `word`, not a longer name or a
/// member of something else.
fn ends_with_word(text: &str, word: &str) -> bool {
    text.strip_suffix(word).is_some_and(|rest| {
        !rest
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    })
}

/// What rewriting the files of a program found.
#[derive(Debug, Default)]
pub struct Rewritten {
    /// The key of every `if` a statement became.
    pub keys: HashSet<RangeKey>,
    /// The statements [`complete`] finishes.
    pub(crate) pending: Vec<Pending>,
    /// One past the last offset a rewrite of the entry file made up: code
    /// added after it ([`crate::display::rewrite`]) starts here or later.
    pub(crate) end: usize,
}

/// A `toEqual` or `toContain` whose operands are bound and whose check waits
/// on their types.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    key: RangeKey,
    matcher: Typed,
    negated: bool,
    a: Operand,
    b: Operand,
    head: String,
    synth: Synth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Typed {
    ToEqual,
    ToContain,
}

/// A bound operand: its `let` name, its source text and its range.
#[derive(Clone, Debug)]
struct Operand {
    name: String,
    text: String,
    range: SourceRange,
}

/// Rewrite every `testing.*` statement of `ast`, a file parsed from `source`,
/// unless the file binds the name `testing` itself.
pub(crate) fn rewrite(
    ast: &mut Script,
    source: &str,
    out: &mut Rewritten,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if binds_testing(ast) {
        return;
    }
    ast.decls.retain(|d| match d {
        TopDecl::ExprStmt(es) if rooted_at_testing(&es.expr) => {
            diagnostics.push(Diagnostic::error("WS126", EXEC_ONLY, es.range.clone()));
            false
        }
        _ => true,
    });
    let mut r = Rewriter {
        source,
        next: source.len() + 1,
        out,
        diagnostics,
    };
    for d in &mut ast.decls {
        walk_decl(&mut r, d);
    }
    for e in [
        &mut ast.module_label,
        &mut ast.module_color,
        &mut ast.module_material,
    ]
    .into_iter()
    .flatten()
    {
        walk_expr(&mut r, e);
    }
    let end = r.next;
    out.end = out.end.max(end);
}

/// Whether the file binds `testing`: an import names it, or a top-level
/// declaration declares it.
fn binds_testing(ast: &Script) -> bool {
    ast.decls.iter().any(|d| match d {
        TopDecl::Import(imp) => match &imp.kind {
            ImportKind::Namespace(alias) => alias == TESTING,
            ImportKind::Named(bindings) => bindings
                .iter()
                .any(|b| b.alias.as_deref().unwrap_or(&b.name) == TESTING),
            ImportKind::All => false,
        },
        TopDecl::Map(m) => m.name == TESTING,
        _ => crate::resolve::decl_binds(d, TESTING),
    })
}

/// Finish each pending statement in `ast` from the operand types in `types`.
pub(crate) fn complete(
    ast: &mut Script,
    pending: Vec<Pending>,
    types: &HashMap<RangeKey, Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut c = Completer {
        pending: pending.into_iter().map(|p| (p.key.clone(), p)).collect(),
        types,
        diagnostics,
    };
    for d in &mut ast.decls {
        walk_decl(&mut c, d);
    }
}

const EXEC_ONLY: &str =
    "a `testing.*` statement runs in an exec context: an `on` handler, or a `mod` or `fn` body";

/// Report each `testing.*` statement `diagnostics` (a type check's) found
/// outside an exec context as one WS126, in place of the WS007s the code it
/// became drew.
pub(crate) fn exec_context_errors(keys: &HashSet<RangeKey>, diagnostics: &mut Vec<Diagnostic>) {
    let outside: Vec<SourceRange> = diagnostics
        .iter()
        .filter(|d| d.code == "WS007" && keys.contains(&d.range.key()))
        .map(|d| d.range.clone())
        .collect();
    if outside.is_empty() {
        return;
    }
    diagnostics.retain(|d| {
        d.code != "WS007"
            || !outside.iter().any(|r| {
                r.contains(&d.range)
                    || (r.file == d.range.file
                        && r.start.line == d.range.start.line
                        && r.start.col == d.range.start.col)
            })
    });
    diagnostics.extend(
        outside
            .into_iter()
            .map(|r| Diagnostic::error("WS126", EXEC_ONLY, r)),
    );
}

// ---------- the mutable walk ----------

/// A pass over the tree that may replace what it visits. Each hook returns
/// whether it handled the node, in which case the walk does not descend.
trait VisitMut {
    fn decl(&mut self, _d: &mut TopDecl) -> bool {
        false
    }
    fn stmt(&mut self, _s: &mut Stmt) -> bool {
        false
    }
    fn expr(&mut self, _e: &mut Expr) -> bool {
        false
    }
}

fn walk_decl(v: &mut dyn VisitMut, d: &mut TopDecl) {
    if v.decl(d) {
        return;
    }
    match d {
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Enum(_) => {}
        TopDecl::Namespace(n) => {
            for d in &mut n.decls {
                walk_decl(v, d);
            }
        }
        TopDecl::Var(x) => {
            for e in [
                &mut x.init,
                &mut x.label_expr,
                &mut x.color_expr,
                &mut x.material_expr,
            ]
            .into_iter()
            .flatten()
            {
                walk_expr(v, e);
            }
        }
        TopDecl::Array(a) => {
            for el in &mut a.init {
                walk_array_elem(v, el);
            }
        }
        TopDecl::Map(m) => walk_opt(v, &mut m.init),
        TopDecl::Buffer(b) => walk_expr(v, &mut b.init),
        TopDecl::Chip(c) => walk_chip(v, c),
        TopDecl::AnonChip(ac) => {
            walk_opt(v, &mut ac.label_expr);
            walk_opt(v, &mut ac.color_expr);
            walk_opt(v, &mut ac.material_expr);
            walk_block(v, &mut ac.body);
        }
        TopDecl::Event(e) => {
            walk_expr(v, &mut e.source);
            if let Some(h) = &mut e.captured {
                walk_handler(v, h);
            }
        }
        TopDecl::In(i) => walk_opt(v, &mut i.label_expr),
        TopDecl::Out(o) => {
            walk_opt(v, &mut o.value);
            walk_opt(v, &mut o.label_expr);
        }
        TopDecl::Handler(h) => walk_handler(v, h),
        TopDecl::Let(l) => walk_expr(v, &mut l.value),
        TopDecl::LetElse(l) => {
            walk_expr(v, &mut l.scrutinee);
            walk_block(v, &mut l.else_block);
        }
        TopDecl::Await(a) => {
            walk_opt(v, &mut a.value_expr);
            walk_expr(v, &mut a.exec_expr);
        }
        TopDecl::Assign(a) => {
            walk_expr(v, &mut a.target);
            walk_expr(v, &mut a.value);
        }
        TopDecl::If(i) => walk_if(v, i),
        TopDecl::IfLet(i) => {
            walk_expr(v, &mut i.scrutinee);
            walk_block(v, &mut i.then_block);
            if let Some(b) = &mut i.else_block {
                walk_block(v, b);
            }
        }
        TopDecl::ExprStmt(es) => walk_expr(v, &mut es.expr),
    }
}

fn walk_opt(v: &mut dyn VisitMut, e: &mut Option<Expr>) {
    if let Some(e) = e {
        walk_expr(v, e);
    }
}

fn walk_array_elem(v: &mut dyn VisitMut, el: &mut crate::ast::ArrayElem) {
    match el {
        crate::ast::ArrayElem::Item(x) | crate::ast::ArrayElem::Spread(x) => walk_expr(v, x),
    }
}

fn walk_chip(v: &mut dyn VisitMut, c: &mut ChipDecl) {
    walk_opt(v, &mut c.label_expr);
    walk_opt(v, &mut c.color_expr);
    walk_opt(v, &mut c.material_expr);
    walk_block(v, &mut c.body);
}

fn walk_handler(v: &mut dyn VisitMut, h: &mut Handler) {
    for c in &mut h.config {
        match c {
            crate::ast::HandlerConfigArg::Positional(x)
            | crate::ast::HandlerConfigArg::Named { value: x, .. } => walk_expr(v, x),
        }
    }
    walk_block(v, &mut h.body);
}

fn walk_if(v: &mut dyn VisitMut, i: &mut If) {
    walk_expr(v, &mut i.cond);
    walk_block(v, &mut i.then_block);
    if let Some(b) = &mut i.else_block {
        walk_block(v, b);
    }
}

fn walk_block(v: &mut dyn VisitMut, b: &mut Block) {
    for s in &mut b.stmts {
        walk_stmt(v, s);
    }
}

fn walk_stmt(v: &mut dyn VisitMut, s: &mut Stmt) {
    if v.stmt(s) {
        return;
    }
    match s {
        Stmt::Assign(a) => {
            walk_expr(v, &mut a.target);
            walk_expr(v, &mut a.value);
        }
        Stmt::Emit(e) => {
            walk_opt(v, &mut e.value);
            if let Some(b) = &mut e.buffer {
                walk_opt(v, &mut b.delay);
                walk_opt(v, &mut b.hold);
            }
        }
        Stmt::Await(a) => {
            walk_opt(v, &mut a.value_expr);
            walk_expr(v, &mut a.exec_expr);
        }
        Stmt::If(i) => walk_if(v, i),
        Stmt::IfLet(i) => {
            walk_expr(v, &mut i.scrutinee);
            walk_block(v, &mut i.then_block);
            if let Some(b) = &mut i.else_block {
                walk_block(v, b);
            }
        }
        Stmt::In(i) => walk_opt(v, &mut i.label_expr),
        Stmt::Let(l) => walk_expr(v, &mut l.value),
        Stmt::LetElse(l) => {
            walk_expr(v, &mut l.scrutinee);
            walk_block(v, &mut l.else_block);
        }
        Stmt::OutBinding(o) => {
            walk_opt(v, &mut o.value);
            walk_opt(v, &mut o.label_expr);
        }
        Stmt::ExprStmt(es) => walk_expr(v, &mut es.expr),
        Stmt::Var(x) => {
            for e in [
                &mut x.init,
                &mut x.label_expr,
                &mut x.color_expr,
                &mut x.material_expr,
            ]
            .into_iter()
            .flatten()
            {
                walk_expr(v, e);
            }
        }
        Stmt::Buffer(b) => walk_expr(v, &mut b.init),
        Stmt::Array(a) => {
            for el in &mut a.init {
                walk_array_elem(v, el);
            }
        }
        Stmt::Map(m) => walk_opt(v, &mut m.init),
        Stmt::Handler(h) => walk_handler(v, h),
        Stmt::AnonChip(ac) => {
            walk_opt(v, &mut ac.label_expr);
            walk_opt(v, &mut ac.color_expr);
            walk_opt(v, &mut ac.material_expr);
            walk_block(v, &mut ac.body);
        }
        Stmt::ChipDecl(c) => walk_chip(v, c),
        Stmt::Return { value, .. } => walk_opt(v, value),
        Stmt::For(f) => {
            walk_opt(v, &mut f.label_expr);
            match &mut f.source {
                crate::ast::ForSource::Range(r) => {
                    match &mut r.extent {
                        crate::ast::RangeExtent::Bounds { start, end } => {
                            walk_expr(v, start);
                            walk_expr(v, end);
                        }
                        crate::ast::RangeExtent::Count { count, from } => {
                            walk_expr(v, count);
                            walk_opt(v, from);
                        }
                    }
                    walk_opt(v, &mut r.step);
                }
                crate::ast::ForSource::Collection { expr, .. } => walk_expr(v, expr),
            }
            walk_block(v, &mut f.body);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn walk_expr(v: &mut dyn VisitMut, e: &mut Expr) {
    if v.expr(e) {
        return;
    }
    match e {
        Expr::IntLit { .. }
        | Expr::AtomLit { .. }
        | Expr::FloatLit { .. }
        | Expr::StringLit { .. }
        | Expr::BoolLit { .. }
        | Expr::NullLit { .. }
        | Expr::CurrentExec { .. }
        | Expr::Ident { .. }
        | Expr::AssetRef { .. }
        | Expr::PrefabRef { .. }
        | Expr::NestedPrefab { .. } => {}
        Expr::InterpLit { parts, .. } => {
            for p in parts {
                if let InterpPart::Expr(x) = p {
                    walk_expr(v, x);
                }
            }
        }
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => walk_expr(v, obj),
        Expr::IndexAccess { obj, index, .. } => {
            walk_expr(v, obj);
            walk_expr(v, index);
        }
        Expr::UnOp { operand, .. } | Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
            walk_expr(v, operand);
        }
        Expr::BinOp { left, right, .. } => {
            walk_expr(v, left);
            walk_expr(v, right);
        }
        Expr::Call { callee, args, .. } => {
            walk_expr(v, callee);
            for a in args {
                match a {
                    CallArg::Positional(x)
                    | CallArg::Spread(x)
                    | CallArg::Named { value: x, .. } => {
                        walk_expr(v, x);
                    }
                }
            }
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            walk_expr(v, cond);
            walk_expr(v, then_branch);
            walk_expr(v, else_branch);
        }
        Expr::BlockExpr { stmts, value, .. } => {
            for s in stmts {
                walk_stmt(v, s);
            }
            walk_expr(v, value);
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            walk_expr(v, scrutinee);
            for arm in arms {
                walk_opt(v, &mut arm.guard);
                match &mut arm.body {
                    MatchBody::Expr(x) => walk_expr(v, x),
                    MatchBody::Block(b) => walk_block(v, b),
                }
            }
        }
        Expr::RecordLit { fields, .. } => walk_record_fields(v, fields),
        Expr::VariantCtor { path, fields, .. } => {
            walk_expr(v, path);
            walk_record_fields(v, fields);
        }
        Expr::Array { elements, .. } => {
            for el in elements {
                walk_array_elem(v, el);
            }
        }
        Expr::MapLit { entries, .. } => {
            for en in entries {
                walk_expr(v, &mut en.key);
                walk_expr(v, &mut en.value);
            }
        }
        Expr::Unsafe { inner, .. } => walk_expr(v, inner),
        Expr::Is { value, path, .. } => {
            walk_expr(v, value);
            walk_expr(v, path);
        }
        Expr::As { value, .. } => walk_expr(v, value),
    }
}

fn walk_record_fields(v: &mut dyn VisitMut, fields: &mut [crate::ast::RecordLitField]) {
    for f in fields {
        match f {
            crate::ast::RecordLitField::Named { value, .. }
            | crate::ast::RecordLitField::Spread { value, .. } => walk_expr(v, value),
            crate::ast::RecordLitField::Shorthand { .. } => {}
        }
    }
}

// ---------- the first rewrite ----------

struct Rewriter<'a> {
    source: &'a str,
    /// The next synthetic offset: past the end of the file, so no node made
    /// here shares a range key with one parsed from it.
    next: usize,
    out: &'a mut Rewritten,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl VisitMut for Rewriter<'_> {
    fn stmt(&mut self, s: &mut Stmt) -> bool {
        if let Stmt::ExprStmt(es) = s
            && let Some(i) = self.statement(es)
        {
            *s = Stmt::If(i);
            return true;
        }
        false
    }

    fn expr(&mut self, e: &mut Expr) -> bool {
        if !rooted_at_testing(e) {
            return false;
        }
        let range = e.range().clone();
        self.error(
            "`testing.*` is a statement and has no value; write it on a line of its own",
            range.clone(),
        );
        *e = Expr::NullLit { range };
        true
    }
}

/// Whether `e` reads `testing` at the root of a chain of member accesses,
/// calls and indexes.
fn rooted_at_testing(e: &Expr) -> bool {
    match e {
        Expr::Ident { name, .. } => name == TESTING,
        Expr::FieldAccess { obj, .. }
        | Expr::IndexAccess { obj, .. }
        | Expr::TuplePick { obj, .. } => rooted_at_testing(obj),
        Expr::Call { callee, .. } => rooted_at_testing(callee),
        _ => false,
    }
}

/// `testing.expect(<args>)`: its arguments.
fn expect_call(e: &Expr) -> Option<&[CallArg]> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::FieldAccess { obj, field, .. } = callee.as_ref() else {
        return None;
    };
    (field == "expect" && matches!(obj.as_ref(), Expr::Ident { name, .. } if name == TESTING))
        .then_some(args.as_slice())
}

/// What a `testing.*` statement asks for.
enum Form {
    Member {
        name: String,
        args: Vec<CallArg>,
    },
    Matcher {
        name: String,
        negated: bool,
        subject: Vec<CallArg>,
        args: Vec<CallArg>,
    },
}

/// The [`Form`] of the statement expression `e`, or the WS126 message for one
/// that is no form.
fn form_of(e: Expr) -> Result<Form, String> {
    let not_call = match &e {
        Expr::Call { .. } => None,
        Expr::FieldAccess { obj, field, .. } => Some(match obj.as_ref() {
            o if expect_call(o).is_some() => needs_matcher(),
            Expr::FieldAccess { obj: inner, .. } if expect_call(inner).is_some() => needs_matcher(),
            Expr::Ident { .. } => {
                format!("`testing.{field}` is a call: write `testing.{field}(...)`")
            }
            _ => NOT_A_FORM.to_string(),
        }),
        _ => Some(NAMESPACE.to_string()),
    };
    if let Some(message) = not_call {
        return Err(message);
    }
    let Expr::Call { callee, args, .. } = e else {
        unreachable!("checked above");
    };
    let Expr::FieldAccess { obj, field, .. } = *callee else {
        return Err(NAMESPACE.to_string());
    };
    if matches!(obj.as_ref(), Expr::Ident { name, .. } if name == TESTING) {
        if MEMBERS.iter().any(|m| m.name == field) {
            return Ok(Form::Member { name: field, args });
        }
        if matchers().iter().any(|m| m.name == field) {
            return Err(format!(
                "`{field}` is a matcher of `testing.expect(a)`: write `testing.expect(a).{field}(b)`"
            ));
        }
        return Err(format!(
            "`testing` has no member `{field}`; its members are {}",
            names(MEMBERS)
        ));
    }
    let (negated, subject) = match *obj {
        Expr::Call { .. } if expect_call(&obj).is_some() => (false, obj),
        Expr::FieldAccess {
            obj: inner,
            field: not,
            ..
        } if not == "not" && expect_call(&inner).is_some() => (true, inner),
        _ => return Err(NOT_A_FORM.to_string()),
    };
    let Expr::Call { args: subject, .. } = *subject else {
        unreachable!("expect_call matched a call");
    };
    if field == "not" || !MATCHERS.iter().any(|m| m.name == field) {
        return Err(if field == "not" {
            "`.not` is written once, before the matcher".to_string()
        } else {
            format!(
                "`testing.expect(a)` has no matcher `{field}`; its matchers are {}",
                names(matchers())
            )
        });
    }
    Ok(Form::Matcher {
        name: field,
        negated,
        subject,
        args,
    })
}

/// The names of `list`, quoted and joined.
fn names(list: &[Member]) -> String {
    list.iter()
        .map(|m| format!("`{}`", m.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The matchers, `not` left out.
fn matchers() -> &'static [Member] {
    &MATCHERS[..MATCHERS.len() - 1]
}

fn needs_matcher() -> String {
    format!("`testing.expect(a)` needs a matcher: {}", names(matchers()))
}

const NAMESPACE: &str =
    "`testing` is a namespace: call one of its members, such as `testing.assert(cond)`";

const NOT_A_FORM: &str =
    "a `testing.*` statement is `testing.<member>(...)` or `testing.expect(a).<matcher>(b)`";

impl Rewriter<'_> {
    fn error(&mut self, message: impl Into<String>, range: SourceRange) {
        self.diagnostics
            .push(Diagnostic::error("WS126", message, range));
    }

    fn synth(&mut self, at: &SourceRange, reserve: usize) -> Synth {
        let s = Synth {
            file: Arc::clone(&at.file),
            line: at.start.line,
            col: at.start.col,
            next: Cell::new(self.next),
        };
        self.next += reserve;
        s
    }

    /// The source text of `range`, its whitespace runs collapsed.
    fn text(&self, range: &SourceRange) -> String {
        self.source
            .get(range.start.offset..range.end.offset)
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The `if` the statement `es` becomes, or `None` when it is not a
    /// `testing.*` statement.
    fn statement(&mut self, es: &mut ExprStmt) -> Option<If> {
        if !rooted_at_testing(&es.expr) {
            return None;
        }
        let range = es.range.clone();
        let mut expr = std::mem::replace(
            &mut es.expr,
            Expr::NullLit {
                range: range.clone(),
            },
        );
        walk_chain_args(self, &mut expr);
        let body = match form_of(expr) {
            Ok(form) => self.lower_form(form, &range),
            Err(message) => Err(message),
        };
        let synth = self.synth(&range, 2);
        let stmts = body.unwrap_or_else(|message| {
            self.error(message, range.clone());
            Vec::new()
        });
        self.out.keys.insert(range.key());
        Some(If {
            cond: Expr::BoolLit {
                value: true,
                range: synth.range(),
            },
            then_block: Block {
                stmts,
                range: synth.range(),
            },
            else_block: None,
            range,
        })
    }

    fn lower_form(&mut self, form: Form, range: &SourceRange) -> Result<Vec<Stmt>, String> {
        let file = file_name(&range.file);
        let line = range.start.line;
        let fail = format!("FAIL {file}:{line}: ");
        let a_name = format!("{LET_PREFIX}{}_a", range.start.offset);
        let b_name = format!("{LET_PREFIX}{}_b", range.start.offset);
        match form {
            Form::Member { name, args } => {
                let args = positional(&name, args)?;
                let s = self.synth(range, STMT_BAND);
                match name.as_str() {
                    "fail" | "log" => {
                        let tag = if name == "fail" { "FAIL" } else { "LOG" };
                        let mut parts = vec![Part::Text(format!("{tag} {file}:{line}"))];
                        if !args.is_empty() {
                            parts.push(Part::Text(": ".to_string()));
                            parts.extend(joined(args));
                        }
                        Ok(vec![s.print(parts)])
                    }
                    "assert" => {
                        let mut args = args.into_iter();
                        let Some(cond) = args.next() else {
                            return Err("`testing.assert` takes a condition: `testing.assert(cond, msg...)`".to_string());
                        };
                        let msg: Vec<Expr> = args.collect();
                        let mut parts = vec![Part::Text(fail)];
                        if msg.is_empty() {
                            parts.push(Part::Text(format!(
                                "assert failed: {}",
                                self.text(cond.range())
                            )));
                        } else {
                            parts.extend(joined(msg));
                        }
                        let bind = s.let_(&a_name, cond);
                        let test = s.not(s.ident(&a_name));
                        let report = s.print(parts);
                        Ok(vec![bind, s.if_(test, vec![report])])
                    }
                    "assertEq" => {
                        let mut args = args.into_iter();
                        let (Some(a), Some(b)) = (args.next(), args.next()) else {
                            return Err("`testing.assertEq` takes two values: `testing.assertEq(a, b, msg...)`".to_string());
                        };
                        let msg: Vec<Expr> = args.collect();
                        let a_text = self.text(a.range());
                        let mut stmts = vec![s.let_(&a_name, a), s.let_(&b_name, b)];
                        let mut parts = vec![
                            Part::Text(format!("{fail}{a_text} expected ")),
                            Part::Value(s.ident(&b_name)),
                            Part::Text(", got ".to_string()),
                            Part::Value(s.ident(&a_name)),
                        ];
                        if !msg.is_empty() {
                            parts.push(Part::Text(" (".to_string()));
                            parts.extend(joined(msg));
                            parts.push(Part::Text(")".to_string()));
                        }
                        let test = s.bin("!=", s.ident(&a_name), s.ident(&b_name));
                        let report = s.print(parts);
                        stmts.push(s.if_(test, vec![report]));
                        Ok(stmts)
                    }
                    "expect" => Err(needs_matcher()),
                    _ => unreachable!("form_of admits only members"),
                }
            }
            Form::Matcher {
                name,
                negated,
                subject,
                args,
            } => {
                let subject = positional("expect", subject)?;
                let [a] = <[Expr; 1]>::try_from(subject).map_err(|_| {
                    "`testing.expect` takes one value: `testing.expect(a)`".to_string()
                })?;
                let a_text = self.text(a.range());
                let (b, digits) = matcher_args(&name, args)?;
                let a_op = Operand {
                    name: a_name.clone(),
                    text: a_text.clone(),
                    range: a.range().clone(),
                };
                let b_op = Operand {
                    name: b_name.clone(),
                    text: self.text(b.range()),
                    range: b.range().clone(),
                };
                let typed = match name.as_str() {
                    "toEqual" => Some(Typed::ToEqual),
                    "toContain" => Some(Typed::ToContain),
                    _ => None,
                };
                let s = self.synth(range, STMT_BAND);
                let mut stmts = vec![s.let_(&a_name, a), s.let_(&b_name, b)];
                if let Some(matcher) = typed {
                    let synth = self.synth(range, PENDING_BAND);
                    self.out.pending.push(Pending {
                        key: range.key(),
                        matcher,
                        negated,
                        a: a_op,
                        b: b_op,
                        head: fail,
                        synth,
                    });
                    return Ok(stmts);
                }
                let not = if negated { "not " } else { "" };
                let (failed, said) = match name.as_str() {
                    "toBe" => (
                        s.bin(
                            if negated { "==" } else { "!=" },
                            s.ident(&a_name),
                            s.ident(&b_name),
                        ),
                        format!("expected {not}"),
                    ),
                    "toBeGreaterThan" | "toBeLessThan" => {
                        let op = if name == "toBeGreaterThan" { ">" } else { "<" };
                        let held = s.bin(op, s.ident(&a_name), s.ident(&b_name));
                        let word = if op == ">" { "greater" } else { "less" };
                        (
                            if negated { held } else { s.not(held) },
                            format!("expected {not}{word} than "),
                        )
                    }
                    "toBeCloseTo" => {
                        let diff = s.bin("-", s.ident(&a_name), s.ident(&b_name));
                        let gap = s.call("abs", vec![diff]);
                        let tolerance = match digits {
                            Some(Expr::IntLit { value, .. }) => {
                                s.float(10f64.powi(-i32::try_from(value).unwrap_or(i32::MAX)) / 2.0)
                            }
                            None => s.float(0.005),
                            Some(d) => {
                                let neg = s.un("-", d);
                                let ten = s.float(10.0);
                                let p = s.call("pow", vec![ten, neg]);
                                let two = s.float(2.0);
                                s.bin("/", p, two)
                            }
                        };
                        let close = s.bin("<", gap, tolerance);
                        (
                            if negated { close } else { s.not(close) },
                            format!("expected {not}close to "),
                        )
                    }
                    _ => unreachable!("form_of admits only matchers"),
                };
                let parts = vec![
                    Part::Text(format!("{fail}{a_text} {said}")),
                    Part::Value(s.ident(&b_name)),
                    Part::Text(", got ".to_string()),
                    Part::Value(s.ident(&a_name)),
                ];
                let report = s.print(parts);
                stmts.push(s.if_(failed, vec![report]));
                Ok(stmts)
            }
        }
    }
}

/// Walk the arguments of every call along the chain `e` roots, the chain
/// itself left alone.
fn walk_chain_args(v: &mut dyn VisitMut, e: &mut Expr) {
    match e {
        Expr::Call { callee, args, .. } => {
            for a in args {
                let (CallArg::Positional(x) | CallArg::Spread(x) | CallArg::Named { value: x, .. }) =
                    a;
                walk_expr(v, x);
            }
            walk_chain_args(v, callee);
        }
        Expr::IndexAccess { obj, index, .. } => {
            walk_expr(v, index);
            walk_chain_args(v, obj);
        }
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => walk_chain_args(v, obj),
        _ => {}
    }
}

/// `args` as positional values, or the message for a named or spread one.
fn positional(member: &str, args: Vec<CallArg>) -> Result<Vec<Expr>, String> {
    args.into_iter()
        .map(|a| match a {
            CallArg::Positional(x) => Ok(x),
            CallArg::Named { name, .. } => {
                Err(format!("`{member}` takes no argument named `{name}`"))
            }
            CallArg::Spread(_) => Err(format!("`{member}` takes no `...` argument")),
        })
        .collect()
}

/// A matcher's value and, for `toBeCloseTo`, its digits.
fn matcher_args(name: &str, args: Vec<CallArg>) -> Result<(Expr, Option<Expr>), String> {
    let mut values = Vec::new();
    let mut digits = None;
    for a in args {
        match a {
            CallArg::Named { name: n, value, .. } if n == "digits" && name == "toBeCloseTo" => {
                digits = Some(value);
            }
            other => values.extend(positional(name, vec![other])?),
        }
    }
    let wants = if name == "toBeCloseTo" && digits.is_none() {
        2
    } else {
        1
    };
    if values.is_empty() || values.len() > wants {
        return Err(if name == "toBeCloseTo" {
            "`toBeCloseTo` takes a value and an optional number of digits: `.toBeCloseTo(b, digits = 2)`"
                .to_string()
        } else {
            format!("`{name}` takes one value: `.{name}(b)`")
        });
    }
    let mut values = values.into_iter();
    let b = values.next().expect("checked non-empty");
    Ok((b, digits.or_else(|| values.next())))
}

/// The file name of `file`, without its directories.
fn file_name(file: &str) -> String {
    let shown = display_path(file);
    shown
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&shown)
        .to_string()
}

/// Message arguments joined by spaces.
fn joined(args: Vec<Expr>) -> Vec<Part> {
    let mut parts = Vec::new();
    for (i, a) in args.into_iter().enumerate() {
        if i > 0 {
            parts.push(Part::Text(" ".to_string()));
        }
        parts.push(match a {
            Expr::StringLit { value, .. } => Part::Text(value),
            a => Part::Value(a),
        });
    }
    parts
}

/// A piece of a message: text, or a value printed into it.
enum Part {
    Text(String),
    Value(Expr),
}

/// Makes the nodes of one rewritten statement, each with a range of its own
/// at the statement's line and column.
#[derive(Clone, Debug)]
struct Synth {
    file: Arc<str>,
    line: u32,
    col: u32,
    next: Cell<usize>,
}

impl Synth {
    fn range(&self) -> SourceRange {
        let at = Pos {
            offset: self.next.get(),
            line: self.line,
            col: self.col,
        };
        self.next.set(at.offset + 1);
        SourceRange {
            file: Arc::clone(&self.file),
            start: at,
            end: at,
        }
    }

    fn ident(&self, name: &str) -> Expr {
        Expr::Ident {
            name: name.to_string(),
            range: self.range(),
        }
    }

    fn float(&self, value: f64) -> Expr {
        Expr::FloatLit {
            value,
            text: format!("{value:?}"),
            range: self.range(),
        }
    }

    fn field(&self, obj: Expr, field: &str) -> Expr {
        Expr::FieldAccess {
            obj: Box::new(obj),
            field: field.to_string(),
            range: self.range(),
        }
    }

    fn bin(&self, op: &str, left: Expr, right: Expr) -> Expr {
        Expr::BinOp {
            op: op.to_string(),
            left: Box::new(left),
            right: Box::new(right),
            range: self.range(),
        }
    }

    fn un(&self, op: &str, operand: Expr) -> Expr {
        Expr::UnOp {
            op: op.to_string(),
            operand: Box::new(operand),
            range: self.range(),
        }
    }

    fn not(&self, operand: Expr) -> Expr {
        self.un("!", operand)
    }

    fn call(&self, name: &str, args: Vec<Expr>) -> Expr {
        let callee = self.ident(name);
        self.call_on(callee, args)
    }

    fn method(&self, receiver: Expr, name: &str, args: Vec<Expr>) -> Expr {
        let callee = self.field(receiver, name);
        self.call_on(callee, args)
    }

    fn call_on(&self, callee: Expr, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Box::new(callee),
            args: args.into_iter().map(CallArg::Positional).collect(),
            type_args: Vec::new(),
            range: self.range(),
        }
    }

    fn let_(&self, name: &str, value: Expr) -> Stmt {
        let range = self.range();
        Stmt::Let(LetDecl {
            binding: LetBinding::Ident {
                name: name.to_string(),
                range: range.clone(),
            },
            typ: None,
            value,
            no_fold: false,
            is_const: false,
            range,
        })
    }

    fn if_(&self, cond: Expr, stmts: Vec<Stmt>) -> Stmt {
        Stmt::If(If {
            cond,
            then_block: Block {
                stmts,
                range: self.range(),
            },
            else_block: None,
            range: self.range(),
        })
    }

    /// `BroadcastChatMessage` of `parts`, adjacent text merged.
    fn print(&self, parts: Vec<Part>) -> Stmt {
        let mut merged: Vec<InterpPart> = Vec::new();
        for p in parts {
            match (p, merged.last_mut()) {
                (Part::Text(t), Some(InterpPart::Lit(prev))) => prev.push_str(&t),
                (Part::Text(t), _) => merged.push(InterpPart::Lit(t)),
                (Part::Value(e), _) => merged.push(InterpPart::Expr(Box::new(e))),
            }
        }
        let message = match merged.as_slice() {
            [InterpPart::Lit(text)] => Expr::StringLit {
                value: text.clone(),
                range: self.range(),
            },
            _ => Expr::InterpLit {
                parts: merged,
                range: self.range(),
            },
        };
        let call = self.call_on(
            Expr::Ident {
                name: "BroadcastChatMessage".to_string(),
                range: self.range(),
            },
            vec![message],
        );
        Stmt::ExprStmt(ExprStmt {
            expr: call,
            range: self.range(),
        })
    }
}

// ---------- the typed completion ----------

struct Completer<'a> {
    pending: HashMap<RangeKey, Pending>,
    types: &'a HashMap<RangeKey, Type>,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl VisitMut for Completer<'_> {
    fn stmt(&mut self, s: &mut Stmt) -> bool {
        match s {
            Stmt::If(i) => self.finish(i),
            _ => false,
        }
    }
}

/// A value's type with a leading reference read through.
fn value_type(t: &Type) -> &Type {
    match t {
        Type::Ref(inner) => inner,
        t => t,
    }
}

impl Completer<'_> {
    fn finish(&mut self, i: &mut If) -> bool {
        let Some(p) = self.pending.remove(&i.range.key()) else {
            return false;
        };
        let type_of = |o: &Operand| self.types.get(&o.range.key()).map(value_type).cloned();
        let (Some(ta), tb) = (type_of(&p.a), type_of(&p.b)) else {
            return true;
        };
        if matches!(ta, Type::Any) {
            return true;
        }
        let checks = match p.matcher {
            Typed::ToEqual => self.to_equal(&p, &ta, tb.as_ref(), &i.range),
            Typed::ToContain => self.to_contain(&p, &ta, tb.as_ref(), &i.range),
        };
        i.then_block.stmts.extend(checks);
        true
    }

    fn error(&mut self, message: String, range: &SourceRange) {
        self.diagnostics
            .push(Diagnostic::error("WS126", message, range.clone()));
    }

    fn to_equal(
        &mut self,
        p: &Pending,
        ta: &Type,
        tb: Option<&Type>,
        range: &SourceRange,
    ) -> Vec<Stmt> {
        let s = p.synth.clone();
        let not = if p.negated { "not " } else { "" };
        let Type::Record(fields) = ta else {
            let failed = s.bin(
                if p.negated { "==" } else { "!=" },
                s.ident(&p.a.name),
                s.ident(&p.b.name),
            );
            let mut parts = vec![Part::Text(format!("{}{} expected {not}", p.head, p.a.text))];
            if tb.is_none_or(printable) && printable(ta) {
                parts.extend([
                    Part::Value(s.ident(&p.b.name)),
                    Part::Text(", got ".to_string()),
                    Part::Value(s.ident(&p.a.name)),
                ]);
            } else {
                parts.push(Part::Text(p.b.text.clone()));
            }
            let report = s.print(parts);
            return vec![s.if_(failed, vec![report])];
        };
        let mut leaves = Vec::new();
        if let Err(path) = record_leaves(fields, &mut Vec::new(), &mut leaves) {
            self.error(
                format!(
                    "`toEqual` compares a record field by field, and `{}.{path}` holds an array or map",
                    p.a.text
                ),
                range,
            );
            return Vec::new();
        }
        let read = |s: &Synth, base: &str, path: &[String]| {
            let mut e = s.ident(base);
            for f in path {
                e = s.field(e, f);
            }
            e
        };
        if p.negated {
            let mut all = None;
            for (path, _) in &leaves {
                let a = read(&s, &p.a.name, path);
                let b = read(&s, &p.b.name, path);
                let same = s.bin("==", a, b);
                all = Some(match all {
                    None => same,
                    Some(prev) => s.bin("&&", prev, same),
                });
            }
            let Some(all) = all else {
                return Vec::new();
            };
            let report = s.print(vec![Part::Text(format!(
                "{}{} expected not to equal {}",
                p.head, p.a.text, p.b.text
            ))]);
            return vec![s.if_(all, vec![report])];
        }
        let mut stmts = Vec::new();
        for (path, ty) in &leaves {
            let shown = format!("{}.{}", p.a.text, path.join("."));
            let a = read(&s, &p.a.name, path);
            let b = read(&s, &p.b.name, path);
            let differs = s.bin("!=", a, b);
            let parts = if printable(ty) {
                vec![
                    Part::Text(format!("{}{shown} expected ", p.head)),
                    Part::Value(read(&s, &p.b.name, path)),
                    Part::Text(", got ".to_string()),
                    Part::Value(read(&s, &p.a.name, path)),
                ]
            } else {
                vec![Part::Text(format!(
                    "{}{shown} differs from {}.{}",
                    p.head,
                    p.b.text,
                    path.join(".")
                ))]
            };
            let report = s.print(parts);
            stmts.push(s.if_(differs, vec![report]));
        }
        stmts
    }

    fn to_contain(
        &mut self,
        p: &Pending,
        ta: &Type,
        tb: Option<&Type>,
        range: &SourceRange,
    ) -> Vec<Stmt> {
        let s = p.synth.clone();
        let not = if p.negated { "not " } else { "" };
        let a = s.ident(&p.a.name);
        let b = s.ident(&p.b.name);
        let held = match ta {
            Type::String => {
                let case_sensitive = Expr::BoolLit {
                    value: true,
                    range: s.range(),
                };
                s.method(a, "Contains", vec![b, case_sensitive])
            }
            Type::Array(_) => {
                let found = s.method(a, "find", vec![b]);
                s.field(found, "Found")
            }
            other => {
                self.error(
                    format!(
                        "`toContain` needs a string or an array, and `{}` is {}",
                        p.a.text,
                        crate::analysis::types::type_str(other)
                    ),
                    range,
                );
                return Vec::new();
            }
        };
        let failed = if p.negated { held } else { s.not(held) };
        let mut parts = vec![Part::Text(format!(
            "{}{} expected {not}to contain ",
            p.head, p.a.text
        ))];
        if tb.is_none_or(printable) {
            parts.push(Part::Value(s.ident(&p.b.name)));
        } else {
            parts.push(Part::Text(p.b.text.clone()));
        }
        if matches!(ta, Type::String) {
            parts.push(Part::Text(", got ".to_string()));
            parts.push(Part::Value(s.ident(&p.a.name)));
        }
        let report = s.print(parts);
        vec![s.if_(failed, vec![report])]
    }
}

/// Every scalar field of a record, by its path, nested records walked; the
/// path of a field holding an array or map is an error.
fn record_leaves(
    fields: &[(String, Type)],
    path: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Type)>,
) -> Result<(), String> {
    for (name, ty) in fields {
        path.push(name.clone());
        match value_type(ty) {
            Type::Record(sub) => record_leaves(sub, path, out)?,
            Type::Array(_) | Type::Map(..) => return Err(path.join(".")),
            t => out.push((path.clone(), t.clone())),
        }
        path.pop();
    }
    Ok(())
}

/// Whether a value of type `t` prints into a message.
fn printable(t: &Type) -> bool {
    crate::types::coerce::formats_to_string(value_type(t))
}

#[cfg(test)]
mod tests;
