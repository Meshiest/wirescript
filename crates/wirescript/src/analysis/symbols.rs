use super::TypeMap;
use super::types::{infer_expr_type, type_expr_str, type_str};
use crate::ast::*;
use crate::catalog::events::find_event;
use crate::diagnostic::SourceRange;

pub struct SymbolDef {
    pub name: String,
    pub kind: &'static str,
    pub range: SourceRange,
    pub ty: Option<String>,
    pub exec: bool,
}

pub fn block_has_exec(block: &Block) -> bool {
    block.stmts.iter().any(stmt_has_exec)
}

fn expr_has_exec(e: &Expr) -> bool {
    match e {
        // Array index read compiles to Exec_ArrayVar_Get — requires exec.
        Expr::IndexAccess { obj, index, .. } => {
            // Only flag if the object looks like an array (not arbitrary indexing).
            // We conservatively flag all IndexAccess as exec-requiring.
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
            // Recurse into args.
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
        // If statement always requires exec context.
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
                        });
                    }
                }
                crate::ast::ParamPattern::Tuple { names, .. } => {
                    for (i, name) in names.iter().enumerate() {
                        let ty = resolve_record_param_field_type(script, &p.typ, &i.to_string());
                        syms.push(SymbolDef {
                            name: name.clone(), kind: "param", range: p.range.clone(), ty, exec: false,
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
            });
        }
        TopDecl::Array(a) => syms.push(SymbolDef {
            name: a.name.clone(),
            kind: "array",
            range: a.range.clone(),
            ty: Some(format!("{}[]", type_expr_str(&a.element_type))),
            exec: false,
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
            });
            if is_local(&f.range) {
                collect_param_symbols(syms, &f.params, script);
            }
        }
        TopDecl::Chip(c) => {
            let params: Vec<String> = c
                .inputs
                .iter()
                .map(|p| format!("{}: {}", p.name, type_expr_str(&p.typ)))
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
            syms.push(SymbolDef {
                name: c.name.clone(),
                kind: label,
                range: c.range.clone(),
                ty: Some(format!("({}){}", params.join(", "), ret_suffix)),
                exec: block_has_exec(&c.body),
            });
            if is_local(&c.range) {
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
            });
            for d in &ns.decls {
                if let Some((mname, mkind)) = namespace_member_decl(d) {
                    syms.push(SymbolDef {
                        name: format!("{}.{}", ns.name, mname),
                        kind: mkind,
                        range: ns.range.clone(),
                        ty: None,
                        exec: false,
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
                        let ty = evt.data.get(i).map(|d| type_str(&d.ty));
                        syms.push(SymbolDef {
                            name: pname.clone(),
                            kind: "param",
                            range: h.range.clone(),
                            ty,
                            exec: false,
                        });
                    }
                } else {
                    for pname in &h.params {
                        syms.push(SymbolDef {
                            name: pname.clone(),
                            kind: "param",
                            range: h.range.clone(),
                            ty: Some("any".into()),
                            exec: false,
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
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{FsLoader, MemLoader, resolve};
    use crate::typecheck::typecheck;

    #[test]
    fn namespace_import_collects_qualified_members() {
        // `import * as u` must yield a `u` namespace symbol plus qualified
        // `u.<member>` symbols for the module's importable decls, so member
        // completion after `u.` can find them.
        let loader = MemLoader {
            files: [(
                "lib.ws".to_string(),
                "mod swap(a: int) {}\nlet PI = 3\ntype Pt = { x: int }".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        let resolved = resolve("import * as u from \"lib\"", "main", &loader);
        let tc = typecheck(&resolved.ast, "main");
        let syms = collect_symbols(&resolved.ast, &tc.type_of_expr);
        assert!(
            syms.iter().any(|s| s.name == "u" && s.kind == "namespace"),
            "namespace alias symbol missing: {:?}",
            syms.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        for (m, k) in [("u.swap", "mod"), ("u.PI", "let"), ("u.Pt", "type")] {
            assert!(
                syms.iter().any(|s| s.name == m && s.kind == k),
                "namespace member {m} ({k}) missing"
            );
        }
    }

    /// The `exec` flag the LSP hover shows for the mod/chip named `name`.
    fn mod_exec(source: &str, name: &str) -> bool {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test");
        collect_symbols(&resolved.ast, &tc.type_of_expr)
            .into_iter()
            .find(|s| s.name == name && (s.kind == "mod" || s.kind == "chip"))
            .unwrap_or_else(|| panic!("no mod/chip symbol named {name}"))
            .exec
    }

    #[test]
    fn pure_return_expr_mod_is_pure() {
        // Regression: `return <expr>` was flagged exec unconditionally, so a
        // pure single-output mod (only comparisons + literals) read as exec.
        let src = "mod band(n: int) -> int {\n  return if n >= 22 then 4 else if n >= 11 then 1 else 0\n}";
        assert!(!mod_exec(src, "band"), "pure return-expr mod should be pure");
    }

    #[test]
    fn array_index_return_mod_is_exec() {
        // `return arr[i]` reads an array -> Exec_ArrayVar_Get -> genuinely exec.
        let src = "array xs: int[]\nmod at(i: int) -> int {\n  return xs[i]\n}";
        assert!(mod_exec(src, "at"), "array-index return mod should be exec");
    }

    #[test]
    fn bare_return_mod_is_exec() {
        // A bare early `return` is exec-chain control flow.
        let src = "mod f(x: int) {\n  return\n}";
        assert!(mod_exec(src, "f"), "bare return should be exec");
    }

    #[test]
    fn if_statement_mod_is_exec() {
        let src = "array xs: int[]\nmod g(x: int) {\n  if x > 0 { xs.push(x) }\n}";
        assert!(mod_exec(src, "g"), "if-statement mod should be exec");
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
