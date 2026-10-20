//! The children of every declaration, statement and expression, each tagged
//! with how it runs relative to its parent.
//!
//! Every pass that asks what runs inside a body, and on which paths, walks the
//! tree through here, so each position is listed exactly once. The matches
//! below name every variant and bind every field, so a new AST position does
//! not compile until it is listed here.

use super::*;

/// How a child runs relative to its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Each time the parent runs, on the parent's own path.
    Seq,
    /// On some of the parent's paths only: an `if`/`if let` branch, a `let
    /// else`'s `else`, a `match` arm, an `if`-expression branch, or a
    /// statement after one that can end its block early (`return`, `break`).
    Branch,
    /// A `for` body: zero or more times per run of the parent.
    Loop,
    /// An anonymous `chip { }` body: on the parent's path, in its own grid.
    AnonChip,
    /// A nested `on` handler's body (or a captured event's): on its own
    /// trigger.
    Handler,
    /// A nested `chip`/`mod`/`fn` declaration's body: wherever it is called.
    Decl,
}

impl Flow {
    /// Whether the child runs as part of the parent's own exec path: not a
    /// loop body, a nested handler or a declaration.
    pub fn on_path(self) -> bool {
        matches!(self, Flow::Seq | Flow::Branch | Flow::AnonChip)
    }
}

/// One node of the tree.
#[derive(Clone, Copy, Debug)]
pub enum Node<'a> {
    Decl(&'a TopDecl),
    Stmt(&'a Stmt),
    Expr(&'a Expr),
    Block(&'a Block),
}

/// Where a node sits relative to the node a walk started from: the flows
/// crossed on the way down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ctx {
    /// Runs on only some paths of the start node: under a branch, in a loop
    /// body, or after a statement that can end its block early.
    pub branch: bool,
    /// Inside a `for` body.
    pub in_loop: bool,
    /// Inside an anonymous chip body.
    pub anon_chip: bool,
    /// Inside a nested `on` handler.
    pub handler: bool,
    /// Inside a nested named declaration's body.
    pub decl: bool,
}

impl Ctx {
    /// The context of a child reached through `flow`.
    pub fn enter(self, flow: Flow) -> Ctx {
        match flow {
            Flow::Seq => self,
            Flow::Branch => Ctx { branch: true, ..self },
            Flow::Loop => Ctx { branch: true, in_loop: true, ..self },
            Flow::AnonChip => Ctx { anon_chip: true, ..self },
            Flow::Handler => Ctx { handler: true, ..self },
            Flow::Decl => Ctx { decl: true, ..self },
        }
    }
}

/// Call `f` with each direct child of `node` and the flow it is reached by.
pub fn children<'a>(node: Node<'a>, f: &mut dyn FnMut(Node<'a>, Flow)) {
    children_in(node, Exits::Tracked, f)
}

/// Whether a block's statements after one that can end it early are reached
/// as [`Flow::Branch`]. [`any_below`] follows on-path flows, which that
/// does not change, and skips the cost of finding the exits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Exits {
    Tracked,
    Ignored,
}

fn children_in<'a>(node: Node<'a>, exits: Exits, f: &mut dyn FnMut(Node<'a>, Flow)) {
    match node {
        Node::Decl(d) => decl_children(d, f),
        Node::Stmt(s) => stmt_children(s, f),
        Node::Expr(e) => expr_children_in(e, exits, f),
        Node::Block(b) => {
            stmts_children(&b.stmts, exits, f);
        }
    }
}

/// Walk every node below `node` depth first. `f` gets each node with its
/// context and returns whether to descend into it.
pub fn walk<'a>(node: Node<'a>, cx: Ctx, f: &mut dyn FnMut(Node<'a>, Ctx) -> bool) {
    children(node, &mut |child, flow| {
        let cx = cx.enter(flow);
        if f(child, cx) {
            walk(child, cx, &mut *f);
        }
    });
}

/// Whether `node`, or any node below it, reads a name spelled `name`: an
/// identifier, or a record field written as `{ name }`.
pub fn mentions(node: Node, name: &str) -> bool {
    let reads = |n: Node| match n {
        Node::Expr(Expr::Ident { name: x, range: _ }) => x == name,
        Node::Expr(Expr::RecordLit { fields, .. } | Expr::VariantCtor { fields, .. }) => fields
            .iter()
            .any(|f| matches!(f, RecordLitField::Shorthand { name: x, range: _ } if x == name)),
        _ => false,
    };
    if reads(node) {
        return true;
    }
    let mut found = false;
    walk(node, Ctx::default(), &mut |n, _| {
        found = found || reads(n);
        !found
    });
    found
}

/// Every name [`mentions`] finds in `node`, each passed to `out`.
pub fn mentioned_names<'a>(node: Node<'a>, out: &mut dyn FnMut(&'a str)) {
    let mut reads = |n: Node<'a>| match n {
        Node::Expr(Expr::Ident { name, range: _ }) => out(name),
        Node::Expr(Expr::RecordLit { fields, .. } | Expr::VariantCtor { fields, .. }) => {
            for f in fields {
                if let RecordLitField::Shorthand { name, range: _ } = f {
                    out(name);
                }
            }
        }
        _ => {}
    };
    reads(node);
    walk(node, Ctx::default(), &mut |n, _| {
        reads(n);
        true
    });
}

/// Whether `pred` holds for some node below `node` reached only through flows
/// `follow` accepts.
pub fn any_below<'a>(
    node: Node<'a>,
    follow: fn(Flow) -> bool,
    pred: &mut dyn FnMut(Node<'a>) -> bool,
) -> bool {
    let mut found = false;
    children_in(node, Exits::Ignored, &mut |child, flow| {
        if found || !follow(flow) {
            return;
        }
        found = pred(child) || any_below(child, follow, &mut *pred);
    });
    found
}

/// Whether running `s` can end its block before the next statement runs: a
/// `return` or `break` on `s`'s own path.
pub fn leaves_early(s: &Stmt) -> bool {
    let exits = |n: Node| matches!(n, Node::Stmt(Stmt::Return { .. } | Stmt::Break { .. }));
    exits(Node::Stmt(s)) || any_below(Node::Stmt(s), Flow::on_path, &mut |n| exits(n))
}

/// Whether a `return` is reached on `block`'s own path (not inside a loop
/// body, a nested handler or a declaration).
pub fn contains_return(block: &Block) -> bool {
    any_below(Node::Block(block), Flow::on_path, &mut |n| matches!(n, Node::Stmt(Stmt::Return { .. })))
}

/// The block arms of the value `match`es in `node`'s own expressions, stopping
/// at every nested block: each runs on a branch of `node`'s path. For a
/// statement `match` these are its arms.
pub fn expr_arm_blocks<'a>(node: Node<'a>) -> Vec<&'a Block> {
    fn go<'a>(node: Node<'a>, in_expr: bool, out: &mut Vec<&'a Block>) {
        children_in(node, Exits::Ignored, &mut |child, _| match child {
            Node::Block(b) if in_expr => out.push(b),
            Node::Block(_) | Node::Decl(_) => {}
            Node::Expr(_) => go(child, true, out),
            Node::Stmt(_) => go(child, false, out),
        });
    }
    let mut out = Vec::new();
    go(node, matches!(node, Node::Expr(_)), &mut out);
    out
}

/// The statements of a block, a block expression or a body: each runs on the
/// block's path until one can end the block early, and the rest only on the
/// paths where it did not. Returns the flow of whatever follows them.
fn stmts_children<'a>(stmts: &'a [Stmt], exits: Exits, f: &mut dyn FnMut(Node<'a>, Flow)) -> Flow {
    let mut flow = Flow::Seq;
    for s in stmts {
        f(Node::Stmt(s), flow);
        if exits == Exits::Tracked && flow == Flow::Seq && leaves_early(s) {
            flow = Flow::Branch;
        }
    }
    flow
}

/// Call `f` with each top-level declaration of `script` and its module
/// label, color and material.
pub fn script_children<'a>(script: &'a Script, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let Script {
        decls,
        range: _,
        module_doc: _,
        no_fold: _,
        fold: _,
        layout: _,
        flat: _,
        invisible: _,
        module_label,
        module_color,
        module_material,
        imports: _,
    } = script;
    for d in decls {
        f(Node::Decl(d), Flow::Seq);
    }
    opt(module_label, Flow::Seq, f);
    opt(module_color, Flow::Seq, f);
    opt(module_material, Flow::Seq, f);
}

fn opt<'a>(e: &'a Option<Expr>, flow: Flow, f: &mut dyn FnMut(Node<'a>, Flow)) {
    if let Some(e) = e {
        f(Node::Expr(e), flow);
    }
}

fn decl_children<'a>(d: &'a TopDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    match d {
        TopDecl::Import(ImportDecl { path: _, kind: _, range: _ }) => {}
        TopDecl::Namespace(NamespaceDecl { name: _, decls, source_path: _, module_doc: _, range: _ }) => {
            for d in decls {
                f(Node::Decl(d), Flow::Seq);
            }
        }
        TopDecl::Var(v) => var_children(v, f),
        TopDecl::Array(a) => array_children(a, f),
        TopDecl::Map(m) => map_children(m, f),
        TopDecl::Buffer(b) => buffer_children(b, f),
        TopDecl::Fn(FnDecl { name: _, params: _, return_type: _, body, range: _ }) => {
            f(Node::Expr(body), Flow::Decl)
        }
        TopDecl::Chip(c) => chip_children(c, f),
        TopDecl::AnonChip(ac) => anon_chip_children(ac, f),
        TopDecl::Event(EventDecl { name: _, source, captured, no_fold: _, range: _ }) => {
            f(Node::Expr(source), Flow::Seq);
            if let Some(h) = captured {
                handler_children(h, f);
            }
        }
        TopDecl::In(i) => in_children(i, f),
        TopDecl::Out(o) => out_children(o, f),
        TopDecl::Handler(h) => handler_children(h, f),
        TopDecl::Let(l) => let_children(l, f),
        TopDecl::LetElse(l) => let_else_children(l, f),
        TopDecl::Await(a) => await_children(a, f),
        TopDecl::Assign(a) => assign_children(a, f),
        TopDecl::If(i) => if_children(i, f),
        TopDecl::IfLet(i) => if_let_children(i, f),
        TopDecl::ExprStmt(es) => expr_stmt_children(es, f),
        TopDecl::TypeAlias(TypeAliasDecl { name: _, type_params: _, typ: _, range: _ }) => {}
        TopDecl::Enum(EnumDecl { name: _, type_params: _, variants: _, range: _ }) => {}
    }
}

/// Call `f` with each direct child of statement `s`.
pub fn stmt_children<'a>(s: &'a Stmt, f: &mut dyn FnMut(Node<'a>, Flow)) {
    match s {
        Stmt::Assign(a) => assign_children(a, f),
        Stmt::Emit(Emit { name: _, value, buffer, range: _ }) => {
            opt(value, Flow::Seq, f);
            if let Some(BufferSpec { delay, hold, seconds: _, range: _ }) = buffer {
                opt(delay, Flow::Seq, f);
                opt(hold, Flow::Seq, f);
            }
        }
        Stmt::Await(a) => await_children(a, f),
        Stmt::If(i) => if_children(i, f),
        Stmt::IfLet(i) => if_let_children(i, f),
        Stmt::In(i) => in_children(i, f),
        Stmt::Let(l) => let_children(l, f),
        Stmt::LetElse(l) => let_else_children(l, f),
        Stmt::OutBinding(o) => out_children(o, f),
        Stmt::ExprStmt(es) => expr_stmt_children(es, f),
        Stmt::Var(v) => var_children(v, f),
        Stmt::Buffer(b) => buffer_children(b, f),
        Stmt::Array(a) => array_children(a, f),
        Stmt::Map(m) => map_children(m, f),
        Stmt::Handler(h) => handler_children(h, f),
        Stmt::AnonChip(ac) => anon_chip_children(ac, f),
        Stmt::ChipDecl(c) => chip_children(c, f),
        Stmt::Return { value, range: _ } => opt(value, Flow::Seq, f),
        Stmt::For(ForStmt { bindings: _, source, result: _, sliced: _, label: _, label_expr, body, range: _ }) => {
            opt(label_expr, Flow::Seq, f);
            match source {
                ForSource::Range(RangeSource { extent, step, reversed: _, range: _ }) => {
                    match extent {
                        RangeExtent::Bounds { start, end } => {
                            f(Node::Expr(start), Flow::Seq);
                            f(Node::Expr(end), Flow::Seq);
                        }
                        RangeExtent::Count { count, from } => {
                            f(Node::Expr(count), Flow::Seq);
                            opt(from, Flow::Seq, f);
                        }
                    }
                    opt(step, Flow::Seq, f);
                }
                ForSource::Collection { expr, range: _ } => f(Node::Expr(expr), Flow::Seq),
            }
            f(Node::Block(body), Flow::Loop);
        }
        Stmt::Break { range: _ } => {}
    }
}

/// Call `f` with each direct child of expression `e`.
pub fn expr_children<'a>(e: &'a Expr, f: &mut dyn FnMut(Node<'a>, Flow)) {
    expr_children_in(e, Exits::Tracked, f)
}

fn expr_children_in<'a>(e: &'a Expr, exits: Exits, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let mut each = |xs: &mut dyn Iterator<Item = &'a Expr>, flow: Flow| {
        for x in xs {
            f(Node::Expr(x), flow);
        }
    };
    match e {
        Expr::IntLit { value: _, text: _, range: _ }
        | Expr::AtomLit { name: _, value: _, range: _ }
        | Expr::FloatLit { value: _, text: _, range: _ }
        | Expr::StringLit { value: _, range: _ }
        | Expr::BoolLit { value: _, range: _ }
        | Expr::NullLit { range: _ }
        | Expr::CurrentExec { range: _ }
        | Expr::Ident { name: _, range: _ }
        | Expr::AssetRef { asset_type: _, asset_name: _, range: _ }
        | Expr::PrefabRef { path: _, range: _ }
        | Expr::NestedPrefab { source: _, range: _ } => {}
        Expr::InterpLit { parts, range: _ } => each(
            &mut parts.iter().filter_map(|p| match p {
                InterpPart::Lit(_) => None,
                InterpPart::Expr(x) => Some(x.as_ref()),
            }),
            Flow::Seq,
        ),
        Expr::FieldAccess { obj, field: _, range: _ } | Expr::TuplePick { obj, index: _, range: _ } => {
            each(&mut std::iter::once(obj.as_ref()), Flow::Seq)
        }
        Expr::IndexAccess { obj, index, range: _ } => each(&mut [obj.as_ref(), index.as_ref()].into_iter(), Flow::Seq),
        Expr::UnOp { op: _, operand, range: _ }
        | Expr::Deref { operand, range: _ }
        | Expr::RefOf { operand, range: _ } => each(&mut std::iter::once(operand.as_ref()), Flow::Seq),
        Expr::BinOp { op: _, left, right, range: _ } => {
            each(&mut [left.as_ref(), right.as_ref()].into_iter(), Flow::Seq)
        }
        Expr::Call { callee, args, type_args: _, range: _ } => {
            each(&mut std::iter::once(callee.as_ref()), Flow::Seq);
            each(
                &mut args.iter().map(|a| match a {
                    CallArg::Positional(x) | CallArg::Spread(x) => x,
                    CallArg::Named { name: _, value, name_range: _ } => value,
                }),
                Flow::Seq,
            );
        }
        Expr::IfExpr { cond, then_branch, else_branch, range: _ } => {
            each(&mut std::iter::once(cond.as_ref()), Flow::Seq);
            each(&mut [then_branch.as_ref(), else_branch.as_ref()].into_iter(), Flow::Branch);
        }
        Expr::BlockExpr { stmts, value, range: _ } => {
            let flow = stmts_children(stmts, exits, f);
            f(Node::Expr(value), flow);
        }
        Expr::MatchExpr { scrutinee, arms, range: _ } => {
            f(Node::Expr(scrutinee), Flow::Seq);
            for MatchArm { pattern: _, guard, body, range: _ } in arms {
                if let Some(g) = guard {
                    f(Node::Expr(g), Flow::Branch);
                }
                match body {
                    MatchBody::Expr(x) => f(Node::Expr(x), Flow::Branch),
                    MatchBody::Block(b) => f(Node::Block(b), Flow::Branch),
                }
            }
        }
        Expr::RecordLit { fields, range: _ } => record_fields(fields, f),
        Expr::VariantCtor { path, fields, range: _ } => {
            f(Node::Expr(path), Flow::Seq);
            record_fields(fields, f);
        }
        Expr::Array { elements, range: _ } => array_elems(elements, f),
        Expr::MapLit { entries, range: _ } => {
            for MapLitEntry { key, value, range: _ } in entries {
                f(Node::Expr(key), Flow::Seq);
                f(Node::Expr(value), Flow::Seq);
            }
        }
        Expr::Unsafe { inner, range: _ } => f(Node::Expr(inner), Flow::Seq),
        Expr::Is { value, path, range: _ } => {
            f(Node::Expr(value), Flow::Seq);
            f(Node::Expr(path), Flow::Seq);
        }
        Expr::As { value, .. } => f(Node::Expr(value), Flow::Seq),
    }
}

fn record_fields<'a>(fields: &'a [RecordLitField], f: &mut dyn FnMut(Node<'a>, Flow)) {
    for field in fields {
        match field {
            RecordLitField::Named { name: _, value, range: _ } | RecordLitField::Spread { value, range: _ } => {
                f(Node::Expr(value), Flow::Seq)
            }
            RecordLitField::Shorthand { name: _, range: _ } => {}
        }
    }
}

fn array_elems<'a>(elements: &'a [ArrayElem], f: &mut dyn FnMut(Node<'a>, Flow)) {
    for el in elements {
        match el {
            ArrayElem::Item(x) | ArrayElem::Spread(x) => f(Node::Expr(x), Flow::Seq),
        }
    }
}

fn assign_children<'a>(a: &'a Assign, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let Assign { target, value, range: _ } = a;
    f(Node::Expr(target), Flow::Seq);
    f(Node::Expr(value), Flow::Seq);
}

fn await_children<'a>(a: &'a AwaitStmt, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let AwaitStmt {
        binding: _,
        binding_type: _,
        destructure: _,
        tuple_destructure: _,
        value_expr,
        exec_expr,
        no_fold: _,
        range: _,
    } = a;
    opt(value_expr, Flow::Seq, f);
    f(Node::Expr(exec_expr), Flow::Seq);
}

fn if_children<'a>(i: &'a If, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let If { cond, then_block, else_block, range: _ } = i;
    f(Node::Expr(cond), Flow::Seq);
    f(Node::Block(then_block), Flow::Branch);
    if let Some(b) = else_block {
        f(Node::Block(b), Flow::Branch);
    }
}

fn if_let_children<'a>(i: &'a IfLet, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let IfLet { pattern: _, scrutinee, then_block, else_block, range: _ } = i;
    f(Node::Expr(scrutinee), Flow::Seq);
    f(Node::Block(then_block), Flow::Branch);
    if let Some(b) = else_block {
        f(Node::Block(b), Flow::Branch);
    }
}

fn let_children<'a>(l: &'a LetDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let LetDecl { binding: _, typ: _, value, no_fold: _, is_const: _, range: _ } = l;
    f(Node::Expr(value), Flow::Seq);
}

fn let_else_children<'a>(l: &'a LetElse, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let LetElse { pattern: _, scrutinee, else_block, is_const: _, range: _ } = l;
    f(Node::Expr(scrutinee), Flow::Seq);
    f(Node::Block(else_block), Flow::Branch);
}

fn expr_stmt_children<'a>(es: &'a ExprStmt, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let ExprStmt { expr, range: _ } = es;
    f(Node::Expr(expr), Flow::Seq);
}

fn var_children<'a>(v: &'a VarDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let VarDecl { name: _, typ: _, init, is_static: _, no_fold: _, label: _, label_expr, color_expr, material_expr, range: _ } = v;
    opt(init, Flow::Seq, f);
    opt(label_expr, Flow::Seq, f);
    opt(color_expr, Flow::Seq, f);
    opt(material_expr, Flow::Seq, f);
}

fn array_children<'a>(a: &'a ArrayDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let ArrayDecl { name: _, element_type: _, init, range: _ } = a;
    array_elems(init, f);
}

fn map_children<'a>(m: &'a MapDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let MapDecl { name: _, key_type: _, value_type: _, init, range: _ } = m;
    opt(init, Flow::Seq, f);
}

fn buffer_children<'a>(b: &'a BufferDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let BufferDecl { name: _, typ: _, init, range: _ } = b;
    f(Node::Expr(init), Flow::Seq);
}

fn in_children<'a>(i: &'a InDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let InDecl { name: _, typ: _, side: _, label: _, label_expr, invisible: _, range: _ } = i;
    opt(label_expr, Flow::Seq, f);
}

fn out_children<'a>(o: &'a OutBinding, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let OutBinding {
        name: _,
        value,
        typ: _,
        side: _,
        label: _,
        label_expr,
        invisible: _,
        no_fold: _,
        range: _,
    } = o;
    opt(value, Flow::Seq, f);
    opt(label_expr, Flow::Seq, f);
}

fn handler_children<'a>(h: &'a Handler, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let Handler {
        trigger: _,
        params: _,
        config,
        body,
        no_fold: _,
        expr_trigger_has_exec_arg: _,
        alias: _,
        range: _,
    } = h;
    for c in config {
        match c {
            HandlerConfigArg::Positional(x) | HandlerConfigArg::Named { name: _, value: x } => {
                f(Node::Expr(x), Flow::Seq)
            }
        }
    }
    f(Node::Block(body), Flow::Handler);
}

fn anon_chip_children<'a>(ac: &'a AnonChipDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let AnonChipDecl { body, range: _, label: _, label_expr, color_expr, material_expr, closed: _ } = ac;
    opt(label_expr, Flow::Seq, f);
    opt(color_expr, Flow::Seq, f);
    opt(material_expr, Flow::Seq, f);
    f(Node::Block(body), Flow::AnonChip);
}

fn chip_children<'a>(c: &'a ChipDecl, f: &mut dyn FnMut(Node<'a>, Flow)) {
    let ChipDecl {
        name: _,
        type_params: _,
        inputs: _,
        rest: _,
        outputs: _,
        body,
        range: _,
        inline: _,
        label: _,
        label_expr,
        color_expr,
        material_expr,
        closed: _,
        no_fold: _,
        is_const: _,
        is_fn: _,
    } = c;
    opt(label_expr, Flow::Seq, f);
    opt(color_expr, Flow::Seq, f);
    opt(material_expr, Flow::Seq, f);
    f(Node::Block(body), Flow::Decl);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mentioned_names` finds exactly the names `mentions` answers yes for,
    /// statement by statement: identifiers, `{ name }` shorthand, nested blocks.
    #[test]
    fn mentioned_names_agrees_with_mentions() {
        let src = "type P = { a: int, b: int }
in go: exec
var a: int = 0
var b: int = 0
                   on go {
  var c: int = a
  let p: P = { a, b: c }
  if b > 0 { c = p.a }
  b = c + 1
}";
        let r = crate::parser::parse(src, "t.ws");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let body = r
            .ast
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(&h.body),
                _ => None,
            })
            .expect("the handler");
        for s in &body.stmts {
            let mut names = Vec::new();
            mentioned_names(Node::Stmt(s), &mut |n| names.push(n));
            for name in ["a", "b", "c", "p", "go", "P"] {
                assert_eq!(names.contains(&name), mentions(Node::Stmt(s), name), "{name} in {s:?}");
            }
        }
    }
}
