use super::TypeMap;
use super::types::{infer_expr_type, type_expr_str, type_str};
use crate::ast::*;
use crate::catalog::events::find_event;
use crate::diagnostic::SourceRange;
use crate::ir::Type;

/// The display type string of a handler param's annotation (`a: int` -> "int"),
/// if it has one.
fn handler_param_type_str(p: &HandlerParam) -> Option<String> {
    p.ty.as_ref().map(type_expr_str)
}

#[derive(Clone)]
pub struct SymbolDef {
    pub name: String,
    pub kind: &'static str,
    pub range: SourceRange,
    pub ty: Option<String>,
    pub exec: bool,
    /// A `const x = ...`, a `const mod` or a `const` parameter, as opposed to a
    /// plain `let`, `mod` or parameter. Drives hover's `const` keyword and, for a
    /// `let`, whether hover shows the computed value (`hover::render_decl_hover`).
    pub is_const: bool,
}

impl SymbolDef {
    /// An untyped, pure, non-const symbol.
    pub fn new(name: impl Into<String>, kind: &'static str, range: SourceRange) -> Self {
        Self {
            name: name.into(),
            kind,
            range,
            ty: None,
            exec: false,
            is_const: false,
        }
    }

    #[must_use]
    pub fn ty(mut self, ty: impl Into<Option<String>>) -> Self {
        self.ty = ty.into();
        self
    }

    #[must_use]
    pub fn exec(mut self, exec: bool) -> Self {
        self.exec = exec;
        self
    }

    #[must_use]
    pub fn konst(mut self, is_const: bool) -> Self {
        self.is_const = is_const;
        self
    }
}

/// Which declaration of `name` is visible at a cursor. A name can be declared in
/// several scopes with different types (a file-scope `var players: string`, a
/// handler-local `var players: character[]`), so this approximates lexical
/// scope: the nearest declaration at or before the cursor wins (a declaration
/// resolves to itself), else the first declaration. `line`/`col` are 0-based
/// with a char column; symbol ranges are 1-based with a byte column, so `source`
/// is needed to compare them.
pub fn resolve_symbol<'a>(
    symbols: &'a [SymbolDef],
    source: &str,
    name: &str,
    line: usize,
    col: usize,
) -> Option<&'a SymbolDef> {
    let byte_col = super::text::char_col_to_byte(super::text::line_text(source, line), col);
    let (cl, cc) = ((line + 1) as u32, (byte_col + 1) as u32);
    let mut first: Option<&SymbolDef> = None;
    let mut best: Option<(&SymbolDef, (u32, u32))> = None;
    for s in symbols.iter().filter(|s| s.name == name) {
        first.get_or_insert(s);
        let p = (s.range.start.line, s.range.start.col);
        let precedes = p.0 < cl || (p.0 == cl && p.1 <= cc);
        if precedes && best.is_none_or(|(_, bp)| p > bp) {
            best = Some((s, p));
        }
    }
    best.map(|(s, _)| s).or(first)
}

pub fn block_has_exec(block: &Block) -> bool {
    block.stmts.iter().any(stmt_has_exec)
}

fn expr_has_exec(e: &Expr) -> bool {
    match e {
        // An array index read compiles to `Exec_ArrayVar_Get`, which needs exec.
        // Every `IndexAccess` counts, whatever its object looks like.
        Expr::IndexAccess { .. } => true,
        Expr::Call { callee, args, .. } => {
            // Array method calls lower to `ArrayVar` exec gates and need exec, except
            // `length`, a pure read.
            if let Expr::FieldAccess { field, .. } = callee.as_ref()
                && crate::catalog::arrays::is_array_method(field)
                && field != "length"
            {
                return true;
            }
            args.iter().any(|a| match a {
                CallArg::Positional(v) => expr_has_exec(v),
                CallArg::Named { value, .. } => expr_has_exec(value),
                CallArg::Spread(v) => expr_has_exec(v),
            })
        }
        Expr::BinOp { left, right, .. } => expr_has_exec(left) || expr_has_exec(right),
        Expr::UnOp { operand, .. } => expr_has_exec(operand),
        Expr::FieldAccess { obj, .. } => expr_has_exec(obj),
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => expr_has_exec(cond) || expr_has_exec(then_branch) || expr_has_exec(else_branch),
        Expr::BlockExpr { stmts, value, .. } => {
            stmts.iter().any(stmt_has_exec) || expr_has_exec(value)
        }
        _ => false,
    }
}

fn stmt_has_exec(s: &Stmt) -> bool {
    match s {
        // Direct exec-requiring statements (emit with value works in pure too).
        Stmt::Assign(_) | Stmt::Handler(_) => true,
        Stmt::Emit(e) => e.value.is_none(),
        Stmt::If(_) => true,
        // An expression statement needs exec if its expression does (array methods).
        Stmt::ExprStmt(es) => expr_has_exec(&es.expr),
        // A let, var, buffer or array binding needs exec if its initializer does.
        Stmt::Let(l) => expr_has_exec(&l.value),
        Stmt::Var(v) => v.init.as_ref().is_some_and(expr_has_exec),
        // `return <expr>` in a single-output mod is the mod's output, so it needs exec
        // only if its expression does; a bare `return` exits an exec chain.
        Stmt::Return { value, .. } => match value {
            Some(v) => expr_has_exec(v),
            None => true,
        },
        _ => false,
    }
}

fn collect_param_symbols(syms: &mut Vec<SymbolDef>, params: &[Param], script: &Script) {
    use super::hover::resolve_record_param_field_type;
    for p in params {
        if let Some(ref pattern) = p.pattern {
            match pattern {
                ParamPattern::Record { fields, .. } => {
                    for field in fields {
                        let field_name = match field {
                            RecordDestructField::Named { name, alias, .. } => {
                                alias.as_deref().unwrap_or(name).to_string()
                            }
                            RecordDestructField::Rest { name, .. } => name.clone(),
                        };
                        let orig_name = match field {
                            RecordDestructField::Named { name, .. } => name.as_str(),
                            RecordDestructField::Rest { name, .. } => name.as_str(),
                        };
                        let ty = resolve_record_param_field_type(script, &p.typ, orig_name);
                        syms.push(
                            SymbolDef::new(field_name, "param", p.range.clone())
                                .ty(ty)
                                .konst(p.is_const),
                        );
                    }
                }
                ParamPattern::Tuple { names, .. } => {
                    for (i, name) in names.iter().enumerate() {
                        let ty = resolve_record_param_field_type(script, &p.typ, &i.to_string());
                        syms.push(
                            SymbolDef::new(name.clone(), "param", p.range.clone())
                                .ty(ty)
                                .konst(p.is_const),
                        );
                    }
                }
            }
        } else {
            syms.push(
                SymbolDef::new(p.name.clone(), "param", p.range.clone())
                    .ty(type_expr_str(&p.typ))
                    .konst(p.is_const),
            );
        }
    }
}

pub fn collect_symbols(script: &Script, tmap: &TypeMap) -> Vec<SymbolDef> {
    collect_symbols_for_file(script, tmap, None)
}

pub fn collect_symbols_for_file(
    script: &Script,
    tmap: &TypeMap,
    file: Option<&str>,
) -> Vec<SymbolDef> {
    let mut syms = Vec::new();
    for d in &script.decls {
        collect_decl(&mut syms, d, tmap, file, script);
    }
    collect_pattern_symbols(&mut syms, script, tmap);
    syms
}

/// The names `match` arms, `if let` and `let ... else` bind, wherever they
/// sit, typed as typecheck bound them. A bare name typecheck read as a unit
/// variant binds nothing and has no type there, so it is no symbol.
fn collect_pattern_symbols(syms: &mut Vec<SymbolDef>, script: &Script, tmap: &TypeMap) {
    use walk::Node;
    fn visit(node: Node<'_>, syms: &mut Vec<SymbolDef>, tmap: &TypeMap) {
        match node {
            Node::Expr(Expr::MatchExpr { arms, .. }) => {
                for arm in arms {
                    pattern_symbols(syms, &arm.pattern, tmap);
                }
            }
            Node::Stmt(Stmt::IfLet(i)) => pattern_symbols(syms, &i.pattern, tmap),
            Node::Stmt(Stmt::LetElse(l)) => pattern_symbols(syms, &l.pattern, tmap),
            _ => {}
        }
        walk::children(node, &mut |child, _| visit(child, syms, tmap));
    }
    walk::script_children(script, &mut |node, _| visit(node, syms, tmap));
}

fn pattern_symbols(syms: &mut Vec<SymbolDef>, pattern: &Pattern, tmap: &TypeMap) {
    match pattern {
        Pattern::Binding { name, range } => {
            if let Some(t) = tmap.get(&range.key()) {
                syms.push(SymbolDef::new(name.clone(), "let", range.clone()).ty(type_str(t)));
            }
        }
        Pattern::Variant { sub, .. } => match sub {
            VariantPattern::Unit => {}
            VariantPattern::Positional(subs) => {
                for p in subs {
                    pattern_symbols(syms, p, tmap);
                }
            }
            VariantPattern::Named { fields, .. } => {
                for (_, p) in fields {
                    pattern_symbols(syms, p, tmap);
                }
            }
        },
        Pattern::Concat { parts, .. } => {
            for p in parts {
                pattern_symbols(syms, p, tmap);
            }
        }
        _ => {}
    }
}

pub fn collect_decl(
    syms: &mut Vec<SymbolDef>,
    d: &TopDecl,
    tmap: &TypeMap,
    file: Option<&str>,
    script: &Script,
) {
    let is_local = |range: &SourceRange| -> bool {
        file.is_none_or(|f| {
            range.file.as_ref() == f || range.file.ends_with(f) || f.ends_with(range.file.as_ref())
        })
    };
    match d {
        TopDecl::Var(v) => {
            let ty = v
                .typ
                .as_ref()
                .map(type_expr_str)
                .or_else(|| v.init.as_ref().and_then(|e| infer_expr_type(e, tmap)));
            let kind = if v.is_static { "static var" } else { "var" };
            syms.push(SymbolDef::new(v.name.clone(), kind, v.range.clone()).ty(ty));
        }
        TopDecl::Array(a) => syms.push(
            SymbolDef::new(a.name.clone(), "array", a.range.clone())
                .ty(format!("{}[]", type_expr_str(&a.element_type))),
        ),
        TopDecl::Buffer(b) => {
            let ty = b
                .typ
                .as_ref()
                .map(type_expr_str)
                .or_else(|| infer_expr_type(&b.init, tmap));
            syms.push(SymbolDef::new(b.name.clone(), "buffer", b.range.clone()).ty(ty));
        }
        TopDecl::Chip(c) => {
            // A `const mod`'s parameters are ALL const (the parser sets
            // `is_const` on every one), so this also carries the marker
            // through to hovering the mod's own NAME, not just hovering an
            // individual parameter directly.
            let params: Vec<String> = c
                .inputs
                .iter()
                .map(|p| {
                    let ty = type_expr_str(&p.typ);
                    if p.is_const {
                        format!("{}: const {}", p.name, ty)
                    } else {
                        format!("{}: {}", p.name, ty)
                    }
                })
                .collect();
            let ret_suffix = match c.outputs.as_slice() {
                [] => String::new(),
                [single] => format!(" -> {}", type_expr_str(&single.typ)),
                multiple => {
                    let fields: Vec<String> = multiple
                        .iter()
                        .map(|o| format!("{}: {}", o.name, type_expr_str(&o.typ)))
                        .collect();
                    format!(" -> ({})", fields.join(", "))
                }
            };
            let label = c.kind_label();
            // Generic type parameters render before the arg list, e.g.
            // `mod square<T>(v: T) -> T` / `mod clamp<T: Scalar>(...)`.
            let generics = if c.type_params.is_empty() {
                String::new()
            } else {
                let ps: Vec<String> = c
                    .type_params
                    .iter()
                    .map(|tp| match &tp.bound {
                        Some(b) => format!("{}: {}", tp.name, type_expr_str(b)),
                        None => tp.name.clone(),
                    })
                    .collect();
                format!("<{}>", ps.join(", "))
            };
            syms.push(
                SymbolDef::new(c.name.clone(), label, c.range.clone())
                    .ty(format!("{}({}){}", generics, params.join(", "), ret_suffix))
                    .exec(block_has_exec(&c.body))
                    .konst(c.is_const),
            );
            if is_local(&c.range) {
                for tp in &c.type_params {
                    syms.push(
                        SymbolDef::new(tp.name.clone(), "typeparam", tp.range.clone())
                            .ty(tp.bound.as_ref().map(type_expr_str)),
                    );
                }
                collect_param_symbols(syms, &c.inputs, script);
                for s in &c.body.stmts {
                    collect_stmt(syms, s, tmap, file, script);
                }
            }
        }
        TopDecl::In(i) => syms
            .push(SymbolDef::new(i.name.clone(), "in", i.range.clone()).ty(type_expr_str(&i.typ))),
        TopDecl::Let(l) => {
            collect_let_symbols(syms, l, tmap);
        }
        TopDecl::Event(e) => syms.push(SymbolDef::new(e.name.clone(), "event", e.range.clone())),
        TopDecl::Out(o) => {
            let ty = o
                .value
                .as_ref()
                .and_then(|v| infer_expr_type(v, tmap))
                .or_else(|| o.typ.as_ref().map(type_expr_str));
            syms.push(SymbolDef::new(o.name.clone(), "out", o.range.clone()).ty(ty));
        }
        TopDecl::Handler(h) => {
            collect_stmt(syms, &Stmt::Handler(h.clone()), tmap, file, script);
        }
        TopDecl::AnonChip(ac) => {
            syms.push(SymbolDef::new("", "chip", ac.range.clone()).exec(block_has_exec(&ac.body)));
            for s in &ac.body.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
        }
        TopDecl::If(i) => {
            for s in &i.then_block.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
            if let Some(eb) = &i.else_block {
                for s in &eb.stmts {
                    collect_stmt(syms, s, tmap, file, script);
                }
            }
        }
        TopDecl::TypeAlias(t) => {
            syms.push(
                SymbolDef::new(t.name.clone(), "type", t.range.clone()).ty(type_expr_str(&t.typ)),
            );
        }
        TopDecl::Namespace(ns) => {
            // `import * as u from "..."`: the alias, plus its importable members as
            // qualified `u.member` symbols. The `.` keeps them out of the global identifier
            // list; member completion after `u.` reads them back by prefix.
            syms.push(SymbolDef::new(
                ns.name.clone(),
                "namespace",
                ns.range.clone(),
            ));
            for d in &ns.decls {
                if let Some((mname, mkind)) = namespace_member_decl(d) {
                    // The member's signature, exec-ness and const-ness come from the normal
                    // declaration collection run into a scratch buffer, re-keyed under
                    // `ns.member`. Its declaration is in the imported file, so that yields one
                    // signature symbol; anything else is dropped.
                    let mut scratch = Vec::new();
                    collect_decl(&mut scratch, d, tmap, file, script);
                    let (ty, exec, is_const) = scratch
                        .iter()
                        .find(|s| s.name == mname && s.kind == mkind)
                        .map(|s| (s.ty.clone(), s.exec, s.is_const))
                        .unwrap_or((None, false, false));
                    syms.push(
                        SymbolDef::new(format!("{}.{}", ns.name, mname), mkind, ns.range.clone())
                            .ty(ty)
                            .exec(exec)
                            .konst(is_const),
                    );
                }
            }
        }
        _ => {}
    }
}

/// The importable name + symbol kind a namespace member exposes, or `None` for a
/// non-importable decl (`var`/`array`/`in`/`out`/handlers). Mirrors the set the
/// resolver allows through `import { ... }` / `import * as`.
fn namespace_member_decl(d: &TopDecl) -> Option<(String, &'static str)> {
    match d {
        TopDecl::Chip(c) => Some((c.name.clone(), c.kind_label())),
        TopDecl::Let(l) => match &l.binding {
            LetBinding::Ident { name, .. } => Some((name.clone(), "let")),
            _ => None,
        },
        TopDecl::TypeAlias(t) => Some((t.name.clone(), "type")),
        TopDecl::Event(e) => Some((e.name.clone(), "event")),
        _ => None,
    }
}

pub fn collect_stmt(
    syms: &mut Vec<SymbolDef>,
    s: &Stmt,
    tmap: &TypeMap,
    file: Option<&str>,
    script: &Script,
) {
    match s {
        Stmt::Var(v) => collect_decl(syms, &TopDecl::Var(v.clone()), tmap, file, script),
        Stmt::Buffer(b) => collect_decl(syms, &TopDecl::Buffer(b.clone()), tmap, file, script),
        Stmt::Array(a) => collect_decl(syms, &TopDecl::Array(a.clone()), tmap, file, script),
        Stmt::Let(l) => collect_decl(syms, &TopDecl::Let(l.clone()), tmap, file, script),
        Stmt::In(i) => collect_decl(syms, &TopDecl::In(i.clone()), tmap, file, script),
        Stmt::OutBinding(o) => collect_decl(syms, &TopDecl::Out(o.clone()), tmap, file, script),
        Stmt::Handler(h) => {
            let trigger_name = match &h.trigger {
                Trigger::Ident { name, .. } => Some(name.as_str()),
                Trigger::Not { inner, .. } => match inner.as_ref() {
                    Trigger::Ident { name, .. } => Some(name.as_str()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(tname) = trigger_name {
                if let Some(evt) = find_event(tname) {
                    for (i, pname) in h.params.iter().enumerate() {
                        // A handler annotation (`a: int` on Custom Event) wins
                        // over the event's declared data type.
                        let ty = handler_param_type_str(pname)
                            .or_else(|| evt.data.get(i).map(|d| type_str(&d.ty)));
                        syms.push(
                            SymbolDef::new(pname.name.clone(), "param", h.range.clone()).ty(ty),
                        );
                    }
                } else {
                    for pname in &h.params {
                        syms.push(
                            SymbolDef::new(pname.name.clone(), "param", h.range.clone())
                                .ty(handler_param_type_str(pname).or_else(|| Some("any".into()))),
                        );
                    }
                }
            }
            for s in &h.body.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
        }
        Stmt::AnonChip(ac) => {
            syms.push(SymbolDef::new("", "chip", ac.range.clone()).exec(block_has_exec(&ac.body)));
            for s in &ac.body.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
        }
        Stmt::ChipDecl(c) => collect_decl(syms, &TopDecl::Chip(c.clone()), tmap, file, script),
        Stmt::If(i) => {
            for s in &i.then_block.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
            if let Some(eb) = &i.else_block {
                for s in &eb.stmts {
                    collect_stmt(syms, s, tmap, file, script);
                }
            }
        }
        Stmt::IfLet(i) => {
            for s in &i.then_block.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
            if let Some(eb) = &i.else_block {
                for s in &eb.stmts {
                    collect_stmt(syms, s, tmap, file, script);
                }
            }
        }
        Stmt::LetElse(l) => {
            for s in &l.else_block.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
        }
        Stmt::ExprStmt(es) => {
            if let Expr::MatchExpr { arms, .. } = &es.expr {
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        for s in &b.stmts {
                            collect_stmt(syms, s, tmap, file, script);
                        }
                    }
                }
            }
        }
        Stmt::For(f) => {
            collect_for_bindings(syms, f, tmap);
            for s in &f.body.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
            if let Some((name, name_range)) = &f.result {
                let ty = Type::Record(
                    crate::ir::plane::result_fields()
                        .into_iter()
                        .map(|(field, _port, t)| (field.to_string(), t))
                        .collect(),
                );
                syms.push(
                    SymbolDef::new(name.clone(), "let", name_range.clone()).ty(type_str(&ty)),
                );
            }
        }
        _ => {}
    }
}

/// Hover types for a `for` loop's slot bindings, sourced from `ir::plane`'s
/// slot tables. A collection's element type is read back from `tmap`
/// (typecheck's own inference), since this module cannot infer types itself.
fn collect_for_bindings(syms: &mut Vec<SymbolDef>, f: &ForStmt, tmap: &TypeMap) {
    use crate::ir::plane::{ForSlot, LoopKind, for_slots, plane_spec, slot_types};

    let (kind, elem): (LoopKind, Vec<Type>) = match &f.source {
        ForSource::Range(_) => (LoopKind::CountFor, Vec::new()),
        ForSource::Collection { expr, .. } => {
            let r = expr.range();
            match tmap.get(&r.key()) {
                Some(Type::Array(elem)) => (LoopKind::ForEachArray, vec![(**elem).clone()]),
                Some(Type::Map(k, v)) => (LoopKind::ForEachMap, vec![(**k).clone(), (**v).clone()]),
                // Source type not resolved (e.g. WS075 already fired for this
                // loop) - nothing meaningful to type the slots with.
                _ => return,
            }
        }
    };
    let spec = plane_spec(kind);
    let slot_tys = slot_types(kind, &elem);
    let slots = for_slots(kind, &f.bindings);
    // More names than slots (WS076) - malformed, nothing to type.
    if slots.contains(&ForSlot::OverBound) {
        return;
    }

    for (pat, slot) in f.bindings.iter().zip(&slots) {
        match slot {
            ForSlot::RangeSlotNames => {
                let ForPat::Record { fields, .. } = pat else {
                    continue;
                };
                for field in fields {
                    let RecordDestructField::Named { name, alias, range } = field else {
                        continue;
                    };
                    let Some(idx) = spec.slot_labels.iter().position(|l| l == name) else {
                        continue;
                    };
                    syms.push(
                        SymbolDef::new(
                            alias.clone().unwrap_or_else(|| name.clone()),
                            "let",
                            range.clone(),
                        )
                        .ty(type_str(&slot_tys[idx])),
                    );
                }
            }
            ForSlot::Slot(i) => push_for_pat_symbol(syms, pat, &slot_tys[*i]),
            ForSlot::OverBound => {}
        }
    }
}

/// Bind one `for` slot pattern's name(s) at `ty`: a plain name, `_` (nothing),
/// or a `{ }` destructure of the slot's value.
fn push_for_pat_symbol(syms: &mut Vec<SymbolDef>, pat: &ForPat, ty: &Type) {
    match pat {
        ForPat::Ignore(_) => {}
        ForPat::Name(name, range) => {
            syms.push(SymbolDef::new(name.clone(), "let", range.clone()).ty(type_str(ty)));
        }
        ForPat::Record { fields, .. } => {
            // An already-erroneous slot (the source type didn't resolve to a
            // record): bind every name with no type rather than guessing one.
            let rec = if let Type::Record(rec) | Type::Outputs(rec) = ty {
                Some(rec)
            } else {
                None
            };
            for field in fields {
                let (name, range, fty) = match field {
                    RecordDestructField::Named { name, alias, range } => (
                        alias.clone().unwrap_or_else(|| name.clone()),
                        range.clone(),
                        rec.and_then(|r| r.iter().find(|(n, _)| n == name))
                            .map(|(_, t)| type_str(t)),
                    ),
                    RecordDestructField::Rest { name, range } => {
                        (name.clone(), range.clone(), None)
                    }
                };
                syms.push(SymbolDef::new(name, "let", range).ty(fty));
            }
        }
    }
}

fn collect_let_symbols(syms: &mut Vec<SymbolDef>, l: &LetDecl, tmap: &TypeMap) {
    match &l.binding {
        LetBinding::Ident { name, .. } => {
            let ty = l
                .typ
                .as_ref()
                .map(type_expr_str)
                .or_else(|| infer_expr_type(&l.value, tmap));
            syms.push(
                SymbolDef::new(name.clone(), "let", l.range.clone())
                    .ty(ty)
                    .konst(l.is_const),
            );
        }
        LetBinding::Tuple { names, .. } => {
            for (i, name) in names.iter().enumerate() {
                // Positional destructure: the i-th field/element of the
                // initializer's record/tuple type, falling back to a
                // same-named record field.
                let ty = value_field_type(&l.value, tmap, Some(i), name);
                syms.push(
                    SymbolDef::new(name.clone(), "let", l.range.clone())
                        .ty(ty)
                        .konst(l.is_const),
                );
            }
        }
        LetBinding::RecordDestruct { fields, .. } => {
            for field in fields {
                let (name, ty) = match field {
                    RecordDestructField::Named { name, alias, .. } => (
                        alias.as_deref().unwrap_or(name).to_string(),
                        // The bound name may be aliased; the record field is `name`.
                        value_field_type(&l.value, tmap, None, name),
                    ),
                    RecordDestructField::Rest { name, .. } => (name.clone(), None),
                };
                syms.push(
                    SymbolDef::new(name, "let", l.range.clone())
                        .ty(ty)
                        .konst(l.is_const),
                );
            }
        }
    }
}

/// Type of a destructured binding, read from the initializer expression's
/// type in `tmap`: record fields resolve by `field` name (or by `index` for
/// positional patterns); tuples resolve by index only.
fn value_field_type(
    value: &Expr,
    tmap: &TypeMap,
    index: Option<usize>,
    field: &str,
) -> Option<String> {
    let r = value.range();
    let ty = tmap.get(&r.key())?;
    match ty {
        Type::Record(fs) | Type::Outputs(fs) => fs
            .iter()
            .find(|(k, _)| k == field)
            .map(|(_, t)| t)
            .or_else(|| index.and_then(|i| fs.get(i)).map(|(_, t)| t))
            .map(type_str),
        Type::Tuple(ts) => index.and_then(|i| ts.get(i)).map(type_str),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
