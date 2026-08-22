use super::TypeMap;
use super::types::{infer_expr_type, type_expr_str, type_str};
use crate::ast::*;
use crate::catalog::events::find_event;
use crate::diagnostic::SourceRange;

/// The display type string of a handler param's annotation (`a: int` -> "int"),
/// if it has one.
fn handler_param_type_str(p: &HandlerParam) -> Option<String> {
    p.ty.as_ref().map(|t| type_expr_str(t))
}

pub struct SymbolDef {
    pub name: String,
    pub kind: &'static str,
    pub range: SourceRange,
    pub ty: Option<String>,
    pub exec: bool,
    /// `const x = …` / a `const mod`'s declaration / a `const`-marked
    /// parameter — as opposed to a plain `let`/`mod`/param. Drives hover's
    /// `const` keyword and, for a `let`, whether hover attempts to show the
    /// binding's compile-time VALUE (see `hover::render_decl_hover`).
    pub is_const: bool,
}

/// Resolve which declaration of `name` is visible at a cursor position. The same
/// name can be declared in several scopes with different types (a file-scope
/// `var players: string`, a handler-local `var players: character[]`); a flat
/// name lookup would pick an arbitrary one and mis-type completion / hover.
/// Approximate lexical scope by the nearest declaration at or before the cursor
/// (shadowing — the innermost / most-recent declaration wins, and hovering a
/// declaration resolves to itself), falling back to the first declaration for a
/// use-before-declaration reference. `line`/`col` are 0-based (LSP cursor);
/// symbol ranges are 1-based (parser `Pos`).
pub fn resolve_symbol<'a>(
    symbols: &'a [SymbolDef],
    name: &str,
    line: usize,
    col: usize,
) -> Option<&'a SymbolDef> {
    let (cl, cc) = ((line + 1) as u32, (col + 1) as u32);
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
        // Array index read compiles to Exec_ArrayVar_Get — requires exec.
        Expr::IndexAccess { obj, index, .. } => {
            // Conservatively flags all IndexAccess as exec-requiring, not just
            // ones where the object looks like an array.
            let _ = (obj, index);
            true
        }
        Expr::Call { callee, args, .. } => {
            // Array method calls all lower to ArrayVar exec gates, so they
            // require exec context — except `length`, which is a pure read.
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
        // Expr statements — check for exec-requiring expressions (e.g. array methods).
        Stmt::ExprStmt(es) => expr_has_exec(&es.expr),
        // Let/var/buffer/array bindings — check the initialiser expression.
        Stmt::Let(l) => expr_has_exec(&l.value),
        Stmt::Var(v) => v.init.as_ref().is_some_and(expr_has_exec),
        // `return <expr>` in a single-output mod is that mod's output value, so
        // it only needs exec if its expression does (e.g. an array read). A bare
        // `return` is an early exit from an exec chain — exec control flow.
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
                crate::ast::ParamPattern::Record { fields, .. } => {
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
                        syms.push(SymbolDef {
                            name: field_name, kind: "param", range: p.range.clone(), ty, exec: false,
                            is_const: p.is_const,
                        });
                    }
                }
                crate::ast::ParamPattern::Tuple { names, .. } => {
                    for (i, name) in names.iter().enumerate() {
                        let ty = resolve_record_param_field_type(script, &p.typ, &i.to_string());
                        syms.push(SymbolDef {
                            name: name.clone(), kind: "param", range: p.range.clone(), ty, exec: false,
                            is_const: p.is_const,
                        });
                    }
                }
            }
        } else {
            syms.push(SymbolDef {
                name: p.name.clone(),
                kind: "param",
                range: p.range.clone(),
                ty: Some(type_expr_str(&p.typ)),
                exec: false,
                is_const: p.is_const,
            });
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
    syms
}

pub fn collect_decl(syms: &mut Vec<SymbolDef>, d: &TopDecl, tmap: &TypeMap, file: Option<&str>, script: &Script) {
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
            syms.push(SymbolDef {
                name: v.name.clone(),
                kind,
                range: v.range.clone(),
                ty,
                exec: false,
                is_const: false,
            });
        }
        TopDecl::Array(a) => syms.push(SymbolDef {
            name: a.name.clone(),
            kind: "array",
            range: a.range.clone(),
            ty: Some(format!("{}[]", type_expr_str(&a.element_type))),
            exec: false,
            is_const: false,
        }),
        TopDecl::Buffer(b) => {
            let ty = b
                .typ
                .as_ref()
                .map(type_expr_str)
                .or_else(|| infer_expr_type(&b.init, tmap));
            syms.push(SymbolDef {
                name: b.name.clone(),
                kind: "buffer",
                range: b.range.clone(),
                ty,
                exec: false,
                is_const: false,
            });
        }
        TopDecl::Fn(f) => {
            let ret = f
                .return_type
                .as_ref()
                .map(type_expr_str)
                .unwrap_or_else(|| "auto".into());
            let params: Vec<String> = f
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, type_expr_str(&p.typ)))
                .collect();
            syms.push(SymbolDef {
                name: f.name.clone(),
                kind: "fn",
                range: f.range.clone(),
                ty: Some(format!("({}) -> {}", params.join(", "), ret)),
                exec: false,
                is_const: false,
            });
            if is_local(&f.range) {
                collect_param_symbols(syms, &f.params, script);
            }
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
            let label = if c.inline { "mod" } else { "chip" };
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
            syms.push(SymbolDef {
                name: c.name.clone(),
                kind: label,
                range: c.range.clone(),
                ty: Some(format!("{}({}){}", generics, params.join(", "), ret_suffix)),
                exec: block_has_exec(&c.body),
                is_const: c.is_const,
            });
            if is_local(&c.range) {
                for tp in &c.type_params {
                    syms.push(SymbolDef {
                        name: tp.name.clone(),
                        kind: "typeparam",
                        range: tp.range.clone(),
                        ty: tp.bound.as_ref().map(|b| type_expr_str(b)),
                        exec: false,
                        is_const: false,
                    });
                }
                collect_param_symbols(syms, &c.inputs, script);
                for s in &c.body.stmts {
                    collect_stmt(syms, s, tmap, file, script);
                }
            }
        }
        TopDecl::In(i) => syms.push(SymbolDef {
            name: i.name.clone(),
            kind: "in",
            range: i.range.clone(),
            ty: Some(type_expr_str(&i.typ)),
            exec: false,
            is_const: false,
        }),
        TopDecl::Let(l) => {
            collect_let_symbols(syms, l, tmap);
        }
        TopDecl::Event(e) => syms.push(SymbolDef {
            name: e.name.clone(),
            kind: "event",
            range: e.range.clone(),
            ty: None,
            exec: false,
            is_const: false,
        }),
        TopDecl::Out(o) => {
            let ty = o
                .value
                .as_ref()
                .and_then(|v| infer_expr_type(v, tmap))
                .or_else(|| o.typ.as_ref().map(type_expr_str));
            syms.push(SymbolDef {
                name: o.name.clone(),
                kind: "out",
                range: o.range.clone(),
                ty,
                exec: false,
                is_const: false,
            });
        }
        TopDecl::Handler(h) => {
            collect_stmt(syms, &Stmt::Handler(h.clone()), tmap, file, script);
        }
        TopDecl::AnonChip(ac) => {
            syms.push(SymbolDef {
                name: String::new(),
                kind: "chip",
                range: ac.range.clone(),
                ty: None,
                exec: block_has_exec(&ac.body),
                is_const: false,
            });
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
            syms.push(SymbolDef {
                name: t.name.clone(),
                kind: "type",
                range: t.range.clone(),
                ty: Some(type_expr_str(&t.typ)),
                exec: false,
                is_const: false,
            });
        }
        TopDecl::Namespace(ns) => {
            // `import * as u from "…"` — the alias itself, plus its importable
            // members as qualified `u.member` symbols. The `.` in the name keeps
            // them out of the global identifier list (filtered there); member
            // completion after `u.` reads them back by prefix.
            syms.push(SymbolDef {
                name: ns.name.clone(),
                kind: "namespace",
                range: ns.range.clone(),
                ty: None,
                exec: false,
                is_const: false,
            });
            for d in &ns.decls {
                if let Some((mname, mkind)) = namespace_member_decl(d) {
                    // Compute the member's real signature + exec-ness (and
                    // constness) by running the normal decl collection into a
                    // scratch buffer, then re-key it under the qualified
                    // `ns.member` name. The member decl lives in the imported
                    // file (not local), so this yields just its one signature
                    // symbol; any extras are discarded here. Without a `ty`,
                    // hover on `ns.member` shows nothing.
                    let mut scratch = Vec::new();
                    collect_decl(&mut scratch, d, tmap, file, script);
                    let (ty, exec, is_const) = scratch
                        .iter()
                        .find(|s| s.name == mname && s.kind == mkind)
                        .map(|s| (s.ty.clone(), s.exec, s.is_const))
                        .unwrap_or((None, false, false));
                    syms.push(SymbolDef {
                        name: format!("{}.{}", ns.name, mname),
                        kind: mkind,
                        range: ns.range.clone(),
                        ty,
                        exec,
                        is_const,
                    });
                }
            }
        }
        _ => {}
    }
}

/// The importable name + symbol kind a namespace member exposes, or `None` for a
/// non-importable decl (`var`/`array`/`in`/`out`/handlers). Mirrors the set the
/// resolver allows through `import { … }` / `import * as`.
fn namespace_member_decl(d: &TopDecl) -> Option<(String, &'static str)> {
    match d {
        TopDecl::Chip(c) => Some((c.name.clone(), if c.inline { "mod" } else { "chip" })),
        TopDecl::Fn(f) => Some((f.name.clone(), "fn")),
        TopDecl::Let(l) => match &l.binding {
            LetBinding::Ident { name, .. } => Some((name.clone(), "let")),
            _ => None,
        },
        TopDecl::TypeAlias(t) => Some((t.name.clone(), "type")),
        TopDecl::Event(e) => Some((e.name.clone(), "event")),
        _ => None,
    }
}

pub fn collect_stmt(syms: &mut Vec<SymbolDef>, s: &Stmt, tmap: &TypeMap, file: Option<&str>, script: &Script) {
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
                        syms.push(SymbolDef {
                            name: pname.name.clone(),
                            kind: "param",
                            range: h.range.clone(),
                            ty,
                            exec: false,
                            is_const: false,
                        });
                    }
                } else {
                    for pname in &h.params {
                        syms.push(SymbolDef {
                            name: pname.name.clone(),
                            kind: "param",
                            range: h.range.clone(),
                            ty: handler_param_type_str(pname).or_else(|| Some("any".into())),
                            exec: false,
                            is_const: false,
                        });
                    }
                }
            }
            for s in &h.body.stmts {
                collect_stmt(syms, s, tmap, file, script);
            }
        }
        Stmt::AnonChip(ac) => {
            syms.push(SymbolDef {
                name: String::new(),
                kind: "chip",
                range: ac.range.clone(),
                ty: None,
                exec: block_has_exec(&ac.body),
                is_const: false,
            });
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
        _ => {}
    }
}

fn collect_let_symbols(syms: &mut Vec<SymbolDef>, l: &LetDecl, tmap: &TypeMap) {
    match &l.binding {
        LetBinding::Ident { name, .. } => {
            let ty = l.typ.as_ref().map(type_expr_str)
                .or_else(|| infer_expr_type(&l.value, tmap));
            syms.push(SymbolDef {
                name: name.clone(),
                kind: "let",
                range: l.range.clone(),
                ty,
                exec: false,
                is_const: l.is_const,
            });
        }
        LetBinding::Tuple { names, .. } | LetBinding::Record { names, .. } => {
            for (i, name) in names.iter().enumerate() {
                // Positional destructure: the i-th field/element of the
                // initializer's record/tuple type, falling back to a
                // same-named record field.
                let ty = value_field_type(&l.value, tmap, Some(i), name);
                syms.push(SymbolDef {
                    name: name.clone(), kind: "let", range: l.range.clone(), ty, exec: false,
                    is_const: l.is_const,
                });
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
                syms.push(SymbolDef {
                    name, kind: "let", range: l.range.clone(), ty, exec: false,
                    is_const: l.is_const,
                });
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
    use crate::ir::Type;
    let r = value.range();
    let ty = tmap.get(&(r.file.clone(), r.start.offset, r.end.offset))?;
    match ty {
        Type::Record(fs) => fs
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
