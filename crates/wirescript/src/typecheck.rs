//! `context.ts` and `scope.ts` fused inline).
//!
//! Walks the AST producing a side `typeOfExpr` map (keyed by each
//! expression's source-range start offset) so we don't need to rebuild
//! the AST as a typed parallel. `opResolutions` records the catalog
//! `OpRule` chosen for every BinOp/UnOp; the lower phase consumes it.
//!
//! Identifier semantics for `var` (the design plan's core rule):
//! - In exec context: `n` auto-derefs (lowered to `Exec_Var_Get`); type = inner T.
//! - In a pure sink expecting `ref T`: `n` is the VarRef port; type = ref T.
//! - In a pure sink expecting `T`: error (WS006); author writes `n.Value`.
//! - `*n`: explicit deref; requires exec context.
//! - `n.Value`: delayed-read form; always yields T.

use crate::collections::HashMap;
use std::sync::Arc;

use crate::scope::Scope as ScopeStack;

use crate::ast::*;
use crate::catalog::calls::find_call;
use crate::catalog::events::{events, find_event};
use crate::catalog::operators::{OpRule, resolve_op};
use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::Type;
use crate::types::coerce::{CoerceRule, coerce};

// ---------- scope + symbol info ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Var,
    Buffer,
    Array,
    Param,
    EventParam,
    LetBinding,
    Fn,
    Chip,
    Event,
    ChipInstance,
    In,
    Out,
    Namespace,
    Type,
}

#[derive(Clone, Debug)]
pub struct EventDataField {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct FnOrChipSig {
    pub params: Vec<EventDataField>,
    pub outputs: Vec<EventDataField>,
}

#[derive(Clone, Debug)]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub name: String,
    pub ty: Type,
    pub decl_range: SourceRange,
    pub signature: Option<FnOrChipSig>,
    pub event_data: Option<Vec<EventDataField>>,
}

/// Thin wrapper around the shared `Scope<V>` stack, preserving the
/// typecheck-specific API (`declare`, `lookup`, `set_type`).
pub struct Scope {
    inner: ScopeStack<SymbolInfo>,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    pub fn new() -> Self {
        Self {
            inner: ScopeStack::new(),
        }
    }
    pub fn push(&mut self) {
        self.inner.push(crate::scope::ScopeTag::BLOCK);
    }
    pub fn pop(&mut self) {
        self.inner.pop();
    }
    /// Declare in the top-most frame. Returns the prior info if any.
    pub fn declare(&mut self, name: &str, info: SymbolInfo) -> Option<SymbolInfo> {
        self.inner.insert(name, info)
    }
    /// Mutate an already-declared symbol's type (used to refine buffer
    /// types after their RHS infers).
    pub fn set_type(&mut self, name: &str, ty: Type) {
        if let Some(info) = self.inner.get_mut(name) {
            info.ty = ty;
        }
    }
    pub fn lookup(&self, name: &str) -> Option<&SymbolInfo> {
        self.inner.get(name)
    }
}

// ---------- exec/pure context ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecMode {
    Exec,
    Pure,
}

// ---------- typecheck context ----------

/// Pre-indexed info about a namespace member for O(1) lookup.
#[derive(Clone, Debug)]
pub struct NsDeclInfo {
    pub kind: SymbolKind,
    pub return_type: Option<TypeExpr>,
}

pub struct TypeCheckCtx {
    pub diagnostics: Vec<Diagnostic>,
    pub scope: Scope,
    exec_stack: Vec<ExecMode>,
    pub file: String,
    pub namespaces: HashMap<String, HashMap<String, NsDeclInfo>>,
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
    /// Ferried payload type per local exec signal, recorded from
    /// `emit sig = <value>` so `let { a, b } = await sig` can type its fields.
    pub signal_payload_types: HashMap<String, Type>,
}

impl TypeCheckCtx {
    pub fn new(file: &str) -> Self {
        Self {
            diagnostics: Vec::new(),
            scope: Scope::new(),
            exec_stack: vec![ExecMode::Pure],
            file: file.to_string(),
            namespaces: HashMap::default(),
            if_contexts: HashMap::default(),
            var_read_contexts: HashMap::default(),
            signal_payload_types: HashMap::default(),
        }
    }
    pub fn exec_mode(&self) -> ExecMode {
        *self.exec_stack.last().unwrap_or(&ExecMode::Pure)
    }
    pub fn in_exec<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.exec_stack.push(ExecMode::Exec);
        let r = f(self);
        self.exec_stack.pop();
        r
    }
    pub fn in_pure<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.exec_stack.push(ExecMode::Pure);
        let r = f(self);
        self.exec_stack.pop();
        r
    }
    pub fn emit(&mut self, code: &str, message: impl Into<String>, range: SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
            range,
        });
    }
}

// ---------- result ----------

pub struct TypeCheckResult {
    /// Typed every visited expression; key is (file, start_offset, end_offset).
    pub type_of_expr: HashMap<(Arc<str>, usize, usize), Type>,
    /// Operator rule chosen for every BinOp/UnOp; same key scheme.
    pub op_resolutions: HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Exec/pure context for each `if` node; key is (file, start_offset). true = exec.
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    /// Exec/pure context for each var identifier read; key is (file, start_offset). true = exec.
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn typecheck(script: &Script, file: &str) -> TypeCheckResult {
    let mut ctx = TypeCheckCtx::new(file);
    register_builtin_events(&mut ctx);

    let mut tmap: HashMap<(Arc<str>, usize, usize), Type> = HashMap::default();
    let mut omap: HashMap<(Arc<str>, usize, usize), OpRule> = HashMap::default();

    // Two-pass: register all top-level decls first so forward refs resolve.
    for d in &script.decls {
        register_decl(&mut ctx, d);
    }
    let mut saw_handler = false;
    // Named chip/mod bodies are checked AFTER everything else: top-level
    // `let` types are only inferred (and thus declared) during this pass, so
    // an eagerly-checked body could not see lets declared later — which is
    // exactly where imported mods land relative to the constants their
    // bodies reference. Signatures were already registered in pass 1, so
    // nothing else depends on a body being checked early.
    let mut deferred_chips: Vec<(bool, &TopDecl)> = Vec::new();
    for d in &script.decls {
        // Statements after `on` handlers run in the combined exec context
        // of all preceding handler exits (exec union).
        let is_handler = matches!(d, TopDecl::Handler(_))
            || matches!(d, TopDecl::AnonChip(ac) if ac.body.stmts.iter().any(|s| matches!(s, Stmt::Handler(_))));
        let exec_wrap = saw_handler && !is_handler;
        if is_handler {
            saw_handler = true;
        }
        if matches!(d, TopDecl::Chip(_)) {
            deferred_chips.push((exec_wrap, d));
            continue;
        }
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
        }
    }
    for (exec_wrap, d) in deferred_chips {
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
        }
    }

    TypeCheckResult {
        type_of_expr: tmap,
        op_resolutions: omap,
        if_contexts: ctx.if_contexts,
        var_read_contexts: ctx.var_read_contexts,
        diagnostics: ctx.diagnostics,
    }
}

fn register_builtin_events(ctx: &mut TypeCheckCtx) {
    let evts = events();
    let mut keys: Vec<&&str> = evts.keys().collect();
    keys.sort();
    for k in keys {
        let spec = &evts[*k];
        ctx.scope.declare(
            spec.surface_name,
            SymbolInfo {
                kind: SymbolKind::Event,
                name: spec.surface_name.to_string(),
                ty: Type::Exec,
                decl_range: SourceRange::default(),
                signature: None,
                event_data: Some(
                    spec.data
                        .iter()
                        .map(|d| EventDataField {
                            name: d.name.to_string(),
                            ty: d.ty.clone(),
                        })
                        .collect(),
                ),
            },
        );
    }
}

// ---------- type expression resolution ----------

fn primitive_name(name: &str) -> Option<Type> {
    Some(match name {
        "bool" => Type::Bool,
        "int" => Type::Int,
        "float" => Type::Float,
        "string" => Type::String,
        "vector" => Type::Vector,
        "rotator" => Type::Rotator,
        "quat" => Type::Quat,
        "color" => Type::Color,
        "entity" => Type::Entity,
        "character" => Type::Character,
        "controller" => Type::Controller,
        "brick" => Type::Brick,
        "prefab" => Type::Prefab,
        "exec" => Type::Exec,
        "any" => Type::Any,
        "never" => Type::Never,
        _ => return None,
    })
}

fn resolve_type_expr(ctx: &mut TypeCheckCtx, t: &TypeExpr) -> Type {
    match t {
        TypeExpr::Name { name, range } => {
            if let Some(prim) = primitive_name(name) {
                return prim;
            }
            if let Some(sym) = ctx.scope.lookup(name)
                && sym.kind == SymbolKind::Type
            {
                return sym.ty.clone();
            }
            ctx.emit("WS002", format!("unknown type '{name}'"), range.clone());
            Type::Any
        }
        TypeExpr::Ref { inner, .. } => Type::Ref(Box::new(resolve_type_expr(ctx, inner))),
        TypeExpr::Array { inner, .. } => Type::Array(Box::new(resolve_type_expr(ctx, inner))),
        TypeExpr::Tuple { fields, .. } => {
            Type::Tuple(fields.iter().map(|f| resolve_type_expr(ctx, f)).collect())
        }
        TypeExpr::Union { options, .. } => {
            Type::Union(options.iter().map(|f| resolve_type_expr(ctx, f)).collect())
        }
        TypeExpr::Record { fields, .. } => Type::Record(
            fields
                .iter()
                .map(|f| (f.name.clone(), resolve_type_expr(ctx, &f.typ)))
                .collect(),
        ),
    }
}

// ---------- decl registration (1st pass) ----------

/// The type of a constant-literal expression, used to infer an unannotated
/// var's (or array var's element) type at registration, before the full type
/// map exists. Returns `None` for anything that isn't a compile-time literal.
fn literal_expr_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::IntLit { .. } => Some(Type::Int),
        Expr::FloatLit { .. } => Some(Type::Float),
        Expr::BoolLit { .. } => Some(Type::Bool),
        Expr::StringLit { .. } | Expr::InterpLit { .. } => Some(Type::String),
        Expr::UnOp { op, operand, .. } if op == "-" => literal_expr_type(operand),
        // Constructor calls type by name alone — the value folds to a constant
        // later only if the args are constant, but the type holds regardless.
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident { name, .. } => match name.as_str() {
                "Vec" => Some(Type::Vector),
                "Rotation" => Some(Type::Rotator),
                "Color" | "ColorSRGB" | "ColorHex" => Some(Type::Color),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Types a Variable gate can hold as a wire variant — the safe targets when
/// refining an unannotated `var`'s placeholder `any` from its initializer.
fn var_storable(t: &Type) -> bool {
    matches!(
        t,
        Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Vector
            | Type::Rotator
            | Type::Quat
            | Type::Color
            | Type::Entity
            | Type::Character
            | Type::Controller
            | Type::Brick
            | Type::Prefab
    )
}

fn register_decl(ctx: &mut TypeCheckCtx, d: &TopDecl) {
    match d {
        TopDecl::Var(v) => {
            let inner = v
                .typ
                .as_ref()
                .map(|t| resolve_type_expr(ctx, t))
                // No annotation: infer an array type from a `[..]` initializer so
                // `var foo = [1, 2]` indexes/iterates as an array, not `Any`, and
                // a scalar type from a literal initializer so `var foo = ""` is a
                // string var (`var n = 0` an int var, …).
                .or_else(|| match &v.init {
                    Some(Expr::Array { elements, .. }) => {
                        let elem = elements
                            .iter()
                            .find_map(|el| literal_expr_type(el.expr()))
                            .unwrap_or(Type::Any);
                        Some(Type::Array(Box::new(elem)))
                    }
                    Some(init) => literal_expr_type(init),
                    None => None,
                })
                .unwrap_or(Type::Any);
            declare_or_dup(
                ctx,
                &v.name,
                SymbolInfo {
                    kind: SymbolKind::Var,
                    name: v.name.clone(),
                    ty: Type::Ref(Box::new(inner)),
                    decl_range: v.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Array(a) => {
            let inner = resolve_type_expr(ctx, &a.element_type);
            declare_or_dup(
                ctx,
                &a.name,
                SymbolInfo {
                    kind: SymbolKind::Array,
                    name: a.name.clone(),
                    ty: Type::Array(Box::new(inner)),
                    decl_range: a.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Buffer(b) => {
            // Type refined in pass 2 from RHS unless an annotation exists.
            let placeholder = b
                .typ
                .as_ref()
                .map(|t| resolve_type_expr(ctx, t))
                .unwrap_or(Type::Any);
            declare_or_dup(
                ctx,
                &b.name,
                SymbolInfo {
                    kind: SymbolKind::Buffer,
                    name: b.name.clone(),
                    ty: placeholder,
                    decl_range: b.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::In(d) => {
            let t = resolve_type_expr(ctx, &d.typ);
            declare_or_dup(
                ctx,
                &d.name,
                SymbolInfo {
                    kind: SymbolKind::In,
                    name: d.name.clone(),
                    ty: t,
                    decl_range: d.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Fn(f) => {
            let params: Vec<EventDataField> = f
                .params
                .iter()
                .map(|p| EventDataField {
                    name: p.name.clone(),
                    ty: resolve_type_expr(ctx, &p.typ),
                })
                .collect();
            let ret = f
                .return_type
                .as_ref()
                .map(|t| resolve_type_expr(ctx, t))
                .unwrap_or(Type::Any);
            declare_or_dup(
                ctx,
                &f.name,
                SymbolInfo {
                    kind: SymbolKind::Fn,
                    name: f.name.clone(),
                    ty: Type::Any,
                    decl_range: f.range.clone(),
                    signature: Some(FnOrChipSig {
                        params,
                        outputs: vec![EventDataField {
                            name: "_".into(),
                            ty: ret,
                        }],
                    }),
                    event_data: None,
                },
            );
        }
        TopDecl::Chip(c) => {
            let params: Vec<EventDataField> = c
                .inputs
                .iter()
                .map(|p| EventDataField {
                    name: p.name.clone(),
                    ty: resolve_type_expr(ctx, &p.typ),
                })
                .collect();
            let outputs: Vec<EventDataField> = c
                .outputs
                .iter()
                .map(|o| EventDataField {
                    name: o.name.clone(),
                    ty: resolve_type_expr(ctx, &o.typ),
                })
                .collect();
            declare_or_dup(
                ctx,
                &c.name,
                SymbolInfo {
                    kind: SymbolKind::Chip,
                    name: c.name.clone(),
                    ty: Type::Any,
                    decl_range: c.range.clone(),
                    signature: Some(FnOrChipSig { params, outputs }),
                    event_data: None,
                },
            );
        }
        TopDecl::Event(e) => {
            declare_or_dup(
                ctx,
                &e.name,
                SymbolInfo {
                    kind: SymbolKind::Event,
                    name: e.name.clone(),
                    ty: Type::Exec,
                    decl_range: e.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::AnonChip(ac) => {
            // Anon chip shares parent scope — register its inner decls.
            for s in &ac.body.stmts {
                match s {
                    Stmt::Var(v) => register_decl(ctx, &TopDecl::Var(v.clone())),
                    Stmt::Buffer(b) => register_decl(ctx, &TopDecl::Buffer(b.clone())),
                    Stmt::Array(a) => register_decl(ctx, &TopDecl::Array(a.clone())),
                    Stmt::In(i) => register_decl(ctx, &TopDecl::In(i.clone())),
                    _ => {}
                }
            }
        }
        TopDecl::TypeAlias(t) => {
            let resolved = resolve_type_expr(ctx, &t.typ);
            declare_or_dup(
                ctx,
                &t.name,
                SymbolInfo {
                    kind: SymbolKind::Type,
                    name: t.name.clone(),
                    ty: resolved,
                    decl_range: t.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Out(_)
        | TopDecl::Let(_)
        | TopDecl::Handler(_)
        | TopDecl::Assign(_)
        | TopDecl::If(_)
        | TopDecl::ExprStmt(_)
        | TopDecl::Import(_)
        | TopDecl::Await(_) => {
            // Resolved before typecheck.
        }
        TopDecl::Namespace(ns) => {
            declare_or_dup(
                ctx,
                &ns.name,
                SymbolInfo {
                    kind: SymbolKind::Namespace,
                    name: ns.name.clone(),
                    ty: Type::Any,
                    decl_range: ns.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
            let mut ns_map = HashMap::default();
            for d in &ns.decls {
                match d {
                    TopDecl::Chip(c) => {
                        // Carry the outputs across as the member's result type.
                        // Without them a namespaced `mod f() -> int` types as
                        // `Any`, so `Ns.f(x) + 1` finds no operator overload and
                        // the whole expression drops to an unsupported gate.
                        let return_type = match c.outputs.len() {
                            0 => None,
                            1 => Some(c.outputs[0].typ.clone()),
                            _ => Some(TypeExpr::Record {
                                fields: c
                                    .outputs
                                    .iter()
                                    .map(|o| RecordTypeField {
                                        name: o.name.clone(),
                                        typ: o.typ.clone(),
                                        range: o.range.clone(),
                                    })
                                    .collect(),
                                range: c.range.clone(),
                            }),
                        };
                        ns_map.insert(
                            c.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Chip,
                                return_type,
                            },
                        );
                    }
                    TopDecl::Fn(f) => {
                        ns_map.insert(
                            f.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Fn,
                                return_type: f.return_type.clone(),
                            },
                        );
                    }
                    // Declare the module's type aliases under their qualified
                    // name so `let p: Ns.Point` resolves through the ordinary
                    // type lookup. The bare name stays private to the module.
                    TopDecl::TypeAlias(t) => {
                        let resolved = resolve_type_expr(ctx, &t.typ);
                        let qualified = format!("{}.{}", ns.name, t.name);
                        ctx.scope.declare(
                            &qualified,
                            SymbolInfo {
                                kind: SymbolKind::Type,
                                name: qualified.clone(),
                                ty: resolved,
                                decl_range: t.range.clone(),
                                signature: None,
                                event_data: None,
                            },
                        );
                    }
                    _ => {}
                }
            }
            ctx.namespaces.insert(ns.name.clone(), ns_map);
        }
    }
}

fn declare_or_dup(ctx: &mut TypeCheckCtx, name: &str, info: SymbolInfo) {
    let range = info.decl_range.clone();
    if ctx.scope.declare(name, info).is_some() {
        ctx.emit("WS013", format!("duplicate declaration of '{name}'"), range);
    }
}

// ---------- decl checking (2nd pass) ----------

/// Validate a top-level (non-exec) array initializer: every element must be a
/// constant literal whose type coerces to the array's element type. Spreads and
/// non-literal values are only meaningful when the array is built at runtime, so
/// they're rejected here with a pointer to assigning the array in an exec
/// handler.
fn check_top_level_array_init(
    ctx: &mut TypeCheckCtx,
    elements: &[ArrayElem],
    elem_ty: &Type,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    for el in elements {
        let e = el.expr();
        let t = ctx.in_pure(|ctx| infer_expr(ctx, e, tmap, omap));
        if matches!(el, ArrayElem::Spread(_)) {
            ctx.emit(
                "WS003",
                "spread `...` in an array initializer is only allowed when building the array inside an exec handler",
                e.range().clone(),
            );
        } else if let Some(lit) = crate::lower::expr_to_literal(e) {
            // Asset / prefab references are object references — they lower to
            // their own reference gate (e.g. AudioReference) whose output must be
            // WIRED into the array, so they can't be baked into the initializer's
            // constant value list. Inlined here they'd be silently dropped.
            if matches!(
                lit,
                crate::ir::Literal::Asset { .. } | crate::ir::Literal::PrefabRef { .. }
            ) {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS024".into(),
                    message: "asset / prefab references can't be inlined into an array initializer — \
                              they're object references wired in from their own brick. Build the array \
                              with `.push(...)` inside an exec handler instead."
                        .into(),
                    range: e.range().clone(),
                });
            } else if !matches!(elem_ty, Type::Any) && coerce(&t, elem_ty) == CoerceRule::Mismatch {
                ctx.emit(
                    "WS003",
                    format!(
                        "array element: expected {}, got {}",
                        crate::analysis::type_str(elem_ty),
                        crate::analysis::type_str(&t)
                    ),
                    e.range().clone(),
                );
            }
        } else {
            ctx.emit(
                "WS003",
                "array initializer elements must be constant literals — assign the array inside an exec handler to build it from runtime values",
                e.range().clone(),
            );
        }
    }
}

fn check_decl(
    ctx: &mut TypeCheckCtx,
    d: &TopDecl,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    match d {
        TopDecl::Var(v) => {
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                // An array-valued `var` declared at top level (no exec context)
                // is baked into the gate just like an `array` decl — its elements
                // must be constant literals.
                if let Expr::Array { elements, .. } = init {
                    // Type the whole array expr so its element type lands in the
                    // map — lowering reads it to infer an unannotated var's type.
                    ctx.in_pure(|ctx| {
                        infer_expr(ctx, init, tmap, omap);
                    });
                    let elem_ty = match &inner {
                        Type::Array(e) => e.as_ref().clone(),
                        _ => Type::Any,
                    };
                    check_top_level_array_init(ctx, elements, &elem_ty, tmap, omap);
                } else {
                    ctx.in_pure(|ctx| {
                        let t = infer_expr(ctx, init, tmap, omap);
                        expect_coerce(ctx, &t, &inner, init.range());
                        // Unannotated var with a non-literal init (`var v =
                        // Vec(…)`): refine the placeholder `any` from the RHS,
                        // like buffers do.
                        if v.typ.is_none() && matches!(inner, Type::Any) {
                            let u = unwrap_ref(&t);
                            if var_storable(&u) {
                                ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                            }
                        }
                    });
                }
            }
        }
        TopDecl::Buffer(b) => {
            ctx.in_pure(|ctx| {
                let t = infer_expr(ctx, &b.init, tmap, omap);
                if b.typ.is_none() {
                    let unwrapped = unwrap_ref(&t);
                    ctx.scope.set_type(&b.name, unwrapped);
                }
            });
        }
        TopDecl::Array(a) => {
            // A top-level initializer is baked into the gate, so its elements
            // must be constant literals matching the element type.
            if !a.init.is_empty() {
                let inner = resolve_type_expr(ctx, &a.element_type);
                check_top_level_array_init(ctx, &a.init, &inner, tmap, omap);
            }
        }
        TopDecl::In(_) => {
            // Already handled in registration.
        }
        TopDecl::Out(b) => {
            if let Some(value) = &b.value {
                ctx.in_pure(|ctx| {
                    infer_expr(ctx, value, tmap, omap);
                });
                // When out has ref type and value is a var, override to show "ref" in hover
                if let Some(ref te) = b.typ {
                    let resolved = resolve_type_expr(ctx, te);
                    if matches!(resolved, Type::Ref(_))
                        && let Expr::Ident { range, .. } = value
                    {
                        ctx.var_read_contexts
                            .remove(&(range.file.clone(), range.start.offset));
                    }
                }
                if b.typ.is_none()
                    && let Expr::Ident { name, .. } = value
                    && let Some(sym) = ctx.scope.lookup(name)
                    && sym.kind == SymbolKind::Var
                {
                    ctx.diagnostics.push(Diagnostic {
                                    severity: Severity::Warning,
                                    code: "WS017".into(),
                                    message: format!(
                                        "out '{}' infers type from var '{}' — add explicit type: \
                                         `out {}: {} = {}` for value, or `out {}: *{} = {}` for ref",
                                        b.name, name,
                                        b.name, crate::analysis::types::type_str(&unwrap_ref(&sym.ty)), name,
                                        b.name, crate::analysis::types::type_str(&unwrap_ref(&sym.ty)), name,
                                    ),
                                    range: b.range.clone(),
                                });
                }
            }
        }
        TopDecl::Let(l) => {
            let t = ctx.in_pure(|ctx| infer_expr(ctx, &l.value, tmap, omap));
            check_let_type_annotation(ctx, l, &t, tmap, omap);
            bind_let(ctx, &l.binding, &t);
        }
        TopDecl::Fn(f) => {
            ctx.scope.push();
            for p in &f.params {
                let pt = resolve_type_expr(ctx, &p.typ);
                ctx.scope.declare(
                    &p.name,
                    SymbolInfo {
                        kind: SymbolKind::Param,
                        name: p.name.clone(),
                        ty: pt,
                        decl_range: p.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            ctx.in_pure(|ctx| {
                infer_expr(ctx, &f.body, tmap, omap);
            });
            ctx.scope.pop();
        }
        TopDecl::Chip(c) => {
            ctx.scope.push();
            for p in &c.inputs {
                let pt = resolve_type_expr(ctx, &p.typ);
                let kind = if matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
                    SymbolKind::Var
                } else {
                    SymbolKind::Param
                };
                // If the param has a destructuring pattern, register the
                // synthetic name with the full type, then also register each
                // destructured field with its resolved field type.
                if let Some(pattern) = &p.pattern {
                    ctx.scope.declare(
                        &p.name,
                        SymbolInfo {
                            kind: SymbolKind::Param,
                            name: p.name.clone(),
                            ty: pt.clone(),
                            decl_range: p.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                    match pattern {
                        crate::ast::ParamPattern::Record { fields, .. } => {
                            for field in fields {
                                match field {
                                    crate::ast::RecordDestructField::Named {
                                        name, alias, ..
                                    } => {
                                        let bind_name = alias.as_ref().unwrap_or(name);
                                        let field_ty = if let Type::Record(rec_fields) = &pt {
                                            rec_fields
                                                .iter()
                                                .find(|(k, _)| k == name)
                                                .map(|(_, t)| t.clone())
                                                .unwrap_or(Type::Any)
                                        } else {
                                            Type::Any
                                        };
                                        ctx.scope.declare(
                                            bind_name,
                                            SymbolInfo {
                                                kind: SymbolKind::Param,
                                                name: bind_name.clone(),
                                                ty: field_ty,
                                                decl_range: p.range.clone(),
                                                signature: None,
                                                event_data: None,
                                            },
                                        );
                                    }
                                    crate::ast::RecordDestructField::Rest { name, .. } => {
                                        ctx.scope.declare(
                                            name,
                                            SymbolInfo {
                                                kind: SymbolKind::Param,
                                                name: name.clone(),
                                                ty: Type::Any,
                                                decl_range: p.range.clone(),
                                                signature: None,
                                                event_data: None,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        crate::ast::ParamPattern::Tuple { names, .. } => {
                            let field_types = if let Type::Tuple(fs) = &pt {
                                fs.clone()
                            } else {
                                vec![]
                            };
                            for (i, name) in names.iter().enumerate() {
                                let field_ty = field_types.get(i).cloned().unwrap_or(Type::Any);
                                ctx.scope.declare(
                                    name,
                                    SymbolInfo {
                                        kind: SymbolKind::Param,
                                        name: name.clone(),
                                        ty: field_ty,
                                        decl_range: p.range.clone(),
                                        signature: None,
                                        event_data: None,
                                    },
                                );
                            }
                        }
                    }
                } else {
                    ctx.scope.declare(
                        &p.name,
                        SymbolInfo {
                            kind,
                            name: p.name.clone(),
                            ty: pt,
                            decl_range: p.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
            }
            ctx.in_exec(|ctx| check_block(ctx, &c.body, tmap, omap));
            // Warn if outputs are declared but never assigned. Assignments
            // count anywhere in the body, including nested if blocks and
            // `on` handlers: `out x = expr`, `emit x (= expr)`, or a plain
            // `x = expr` assignment.
            if !c.outputs.is_empty() && !block_has_return_value(&c.body) {
                let mut assigned = std::collections::HashSet::default();
                collect_output_assignments(&c.body, &mut assigned);
                for out in &c.outputs {
                    if !assigned.contains(&out.name) {
                        ctx.emit(
                            "WS013",
                            format!("output '{}' is never assigned — use `out {} = expr`, `emit {}`, or `return expr`", out.name, out.name, out.name),
                            out.range.clone(),
                        );
                    }
                }
            }
            ctx.scope.pop();
        }
        TopDecl::AnonChip(ac) => {
            // Anon chip shares parent scope — NO scope push/pop.
            // Vars already pre-registered in pass 1; use check_decl (not
            // check_stmt) for them to avoid duplicate-declaration errors.
            check_anon_chip_stmts(ctx, &ac.body.stmts, true, tmap, omap);
        }
        TopDecl::Event(e) => {
            if let Some(body) = &e.captured_body {
                ctx.in_exec(|ctx| check_block(ctx, body, tmap, omap));
            } else {
                ctx.in_pure(|ctx| {
                    infer_expr(ctx, &e.source, tmap, omap);
                });
            }
        }
        TopDecl::Handler(h) => {
            ctx.scope.push();
            bind_handler_trigger_params(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body, tmap, omap));
            ctx.scope.pop();
        }
        TopDecl::ExprStmt(s) => {
            ctx.in_pure(|ctx| {
                infer_expr(ctx, &s.expr, tmap, omap);
            });
        }
        TopDecl::Assign(a) => {
            check_stmt(ctx, &Stmt::Assign(a.clone()), tmap, omap);
        }
        TopDecl::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "top-level 'if' outside an exec context",
                    i.range.clone(),
                );
            }
            check_stmt(ctx, &Stmt::If(i.clone()), tmap, omap);
        }
        TopDecl::Namespace(ns) => {
            // A namespaced (`import * as ns`) mod body references its sibling
            // constants and mods by BARE name, and those mods are inlined at
            // call sites in the importing module. Typecheck the bodies here in
            // an isolated scope (siblings registered as bare names) so operator
            // resolutions and expression types get recorded — otherwise the
            // inlined body's arithmetic and sibling calls lower to _Unsupported.
            ctx.scope.push();
            for d in &ns.decls {
                register_decl(ctx, d);
            }
            for d in &ns.decls {
                if matches!(
                    d,
                    TopDecl::Let(_) | TopDecl::Var(_) | TopDecl::Array(_) | TopDecl::Buffer(_)
                ) {
                    check_decl(ctx, d, tmap, omap);
                }
            }
            for d in &ns.decls {
                if matches!(d, TopDecl::Chip(_) | TopDecl::Fn(_)) {
                    check_decl(ctx, d, tmap, omap);
                }
            }
            ctx.scope.pop();
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) => {}
    }
}

fn bind_handler_trigger_params(ctx: &mut TypeCheckCtx, h: &Handler) {
    let (name, range) = match &h.trigger {
        Trigger::Ident { name, range } => (name, range),
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, range } => (name, range),
            _ => return,
        },
        _ => return,
    };
    {
        let evt = find_event(name);
        let sym = ctx.scope.lookup(name).cloned();
        let known_event = evt.is_some();
        let known_capture = matches!(&sym, Some(s) if s.kind == SymbolKind::Event);
        let known_input_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::In && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
        );
        let known_buffer_trigger = matches!(
            &sym,
            Some(s)
                if s.kind == SymbolKind::Buffer
                    && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Any)
        );
        let known_let_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::LetBinding
        );
        let known_param_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::Param && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Character | Type::Controller | Type::Entity)
        );
        if !known_event
            && !known_capture
            && !known_input_trigger
            && !known_buffer_trigger
            && !known_let_trigger
            && !known_param_trigger
        {
            ctx.emit(
                "WS001",
                format!("unknown event or trigger '{name}'"),
                range.clone(),
            );
        }
        if h.params.is_empty() {
            return;
        }
        let Some(evt) = evt else {
            // Unknown event: bind params as Any so they don't trip downstream lookups.
            for pname in &h.params {
                ctx.scope.declare(
                    pname,
                    SymbolInfo {
                        kind: SymbolKind::EventParam,
                        name: pname.clone(),
                        ty: Type::Any,
                        decl_range: h.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            return;
        };
        if evt.data.len() < h.params.len() {
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: expected {} param(s), got {}",
                    evt.data.len(),
                    h.params.len()
                ),
                h.range.clone(),
            );
        }
        for (i, pname) in h.params.iter().enumerate() {
            let ty = evt.data.get(i).map(|d| d.ty.clone()).unwrap_or(Type::Any);
            ctx.scope.declare(
                pname,
                SymbolInfo {
                    kind: SymbolKind::EventParam,
                    name: pname.clone(),
                    ty,
                    decl_range: h.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        return;
    }

    // TrigField: TODO, treat params as Any if any.
    #[allow(unreachable_code)]
    for pname in &h.params {
        ctx.scope.declare(
            pname,
            SymbolInfo {
                kind: SymbolKind::EventParam,
                name: pname.clone(),
                ty: Type::Any,
                decl_range: h.range.clone(),
                signature: None,
                event_data: None,
            },
        );
    }
}

/// Collect names assigned as outputs anywhere in a block: `out name = expr`
/// bindings, `emit name (= expr)`, and bare `name = expr` assignments (an
/// over-approximation — variable assigns land here too — but this set only
/// suppresses the WS013 unassigned-output warning). Recurses into if blocks,
/// `on` handlers, and anonymous chip blocks; nested named chips own their
/// outputs and are skipped.
fn collect_output_assignments(block: &Block, assigned: &mut std::collections::HashSet<String>) {
    for s in &block.stmts {
        match s {
            Stmt::OutBinding(o) => {
                assigned.insert(o.name.clone());
            }
            Stmt::Emit(e) => {
                assigned.insert(e.name.clone());
            }
            Stmt::Assign(a) => {
                if let Expr::Ident { name, .. } = &a.target {
                    assigned.insert(name.clone());
                }
            }
            Stmt::If(i) => {
                collect_output_assignments(&i.then_block, assigned);
                if let Some(eb) = &i.else_block {
                    collect_output_assignments(eb, assigned);
                }
            }
            Stmt::Handler(h) => collect_output_assignments(&h.body, assigned),
            Stmt::AnonChip(ac) => collect_output_assignments(&ac.body, assigned),
            _ => {}
        }
    }
}

fn block_has_return_value(block: &Block) -> bool {
    for s in &block.stmts {
        match s {
            Stmt::Return { value: Some(_), .. } => return true,
            Stmt::If(i) => {
                if block_has_return_value(&i.then_block) {
                    return true;
                }
                if let Some(eb) = &i.else_block
                    && block_has_return_value(eb)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn check_block(
    ctx: &mut TypeCheckCtx,
    block: &Block,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    ctx.scope.push();
    for s in &block.stmts {
        check_stmt(ctx, s, tmap, omap);
    }
    ctx.scope.pop();
}

fn check_anon_chip_stmts(
    ctx: &mut TypeCheckCtx,
    stmts: &[Stmt],
    pre_registered: bool,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    if !pre_registered {
        for s in stmts {
            match s {
                Stmt::Var(v) => register_decl(ctx, &TopDecl::Var(v.clone())),
                Stmt::Buffer(b) => register_decl(ctx, &TopDecl::Buffer(b.clone())),
                Stmt::Array(a) => register_decl(ctx, &TopDecl::Array(a.clone())),
                Stmt::In(i) => register_decl(ctx, &TopDecl::In(i.clone())),
                _ => {}
            }
        }
    }
    for s in stmts {
        match s {
            Stmt::Var(v) => check_decl(ctx, &TopDecl::Var(v.clone()), tmap, omap),
            Stmt::Buffer(b) => check_decl(ctx, &TopDecl::Buffer(b.clone()), tmap, omap),
            Stmt::Array(a) => check_decl(ctx, &TopDecl::Array(a.clone()), tmap, omap),
            Stmt::In(i) => check_decl(ctx, &TopDecl::In(i.clone()), tmap, omap),
            other => check_stmt(ctx, other, tmap, omap),
        }
    }
}

fn check_stmt(
    ctx: &mut TypeCheckCtx,
    s: &Stmt,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    match s {
        Stmt::Var(v) => {
            register_decl(ctx, &TopDecl::Var(v.clone()));
            // Statement-level vars inherit the current exec context (not forced pure)
            // so that `var x: int = arr[i]` works inside handlers.
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                let t = infer_expr(ctx, init, tmap, omap);
                expect_coerce(ctx, &t, &inner, init.range());
                // Refine an unannotated var's placeholder `any` from a
                // non-literal init, same as the top-level decl path.
                if v.typ.is_none() && matches!(inner, Type::Any) {
                    let u = unwrap_ref(&t);
                    if var_storable(&u) {
                        ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                    }
                }
            }
        }
        Stmt::Buffer(b) => {
            register_decl(ctx, &TopDecl::Buffer(b.clone()));
            check_decl(ctx, &TopDecl::Buffer(b.clone()), tmap, omap);
        }
        Stmt::Array(a) => {
            register_decl(ctx, &TopDecl::Array(a.clone()));
            check_decl(ctx, &TopDecl::Array(a.clone()), tmap, omap);
        }
        Stmt::Let(l) => {
            let t = infer_expr(ctx, &l.value, tmap, omap);
            check_let_type_annotation(ctx, l, &t, tmap, omap);
            bind_let(ctx, &l.binding, &t);
        }
        Stmt::Assign(a) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    format!(
                        "var write '{}' outside an exec context",
                        target_name(&a.target).unwrap_or("<expr>".into())
                    ),
                    a.range.clone(),
                );
            }
            let target_ty = infer_assign_target(ctx, &a.target, tmap, omap);
            let value_ty = infer_expr(ctx, &a.value, tmap, omap);
            expect_coerce(ctx, &value_ty, &target_ty, a.value.range());
        }
        Stmt::OutBinding(b) => {
            if let Some(value) = &b.value {
                infer_expr(ctx, value, tmap, omap);
            }
        }
        Stmt::Emit(e) => {
            if e.value.is_none() && ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    format!("emit '{}' outside an exec context", e.name),
                    e.range.clone(),
                );
            }
            if let Some(ref val) = e.value {
                let t = infer_expr(ctx, val, tmap, omap);
                // Remember the ferried payload type so a later
                // `let { .. } = await sig` can type its destructured fields.
                ctx.signal_payload_types.insert(e.name.clone(), t);
            }
        }
        Stmt::Await(a) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit("WS007", "await outside an exec context", a.range.clone());
            }
            // Push scope with `_` as Bool (the armed flag) for exec expression
            ctx.scope.push();
            ctx.scope.declare(
                "_",
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: "_".into(),
                    ty: Type::Bool,
                    decl_range: a.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
            let exec_ty = infer_expr(ctx, &a.exec_expr, tmap, omap);
            ctx.scope.pop();
            let val_ty = if let Some(ref val) = a.value_expr {
                infer_expr(ctx, val, tmap, omap)
            } else {
                exec_ty
            };
            if let Some(ref binding) = a.binding {
                ctx.scope.declare(
                    binding,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: binding.clone(),
                        ty: val_ty,
                        decl_range: a.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            // `let { a, b } = await sig`: type each destructured local from the
            // signal's recorded payload record (Any when unknown).
            if let Some(ref fields) = a.destructure {
                let payload_ty = match &a.exec_expr {
                    Expr::Ident { name, .. } => ctx.signal_payload_types.get(name).cloned(),
                    _ => None,
                };
                for (field, local) in fields {
                    let fty = match &payload_ty {
                        Some(Type::Record(fs)) => fs
                            .iter()
                            .find(|(n, _)| n == field)
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Type::Any),
                        _ => Type::Any,
                    };
                    ctx.scope.declare(
                        local,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: local.clone(),
                            ty: fty,
                            decl_range: a.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
            }
        }
        Stmt::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "'if' statement outside an exec context",
                    i.range.clone(),
                );
            }
            ctx.if_contexts.insert(
                (i.range.file.clone(), i.range.start.offset),
                ctx.exec_mode() == ExecMode::Exec,
            );
            infer_expr(ctx, &i.cond, tmap, omap);
            check_block(ctx, &i.then_block, tmap, omap);
            if let Some(else_b) = &i.else_block {
                check_block(ctx, else_b, tmap, omap);
            }
        }
        Stmt::ExprStmt(es) => {
            infer_expr(ctx, &es.expr, tmap, omap);
        }
        Stmt::In(i) => {
            register_decl(ctx, &TopDecl::In(i.clone()));
        }
        Stmt::Handler(h) => {
            ctx.scope.push();
            bind_handler_trigger_params(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body, tmap, omap));
            ctx.scope.pop();
        }
        Stmt::AnonChip(ac) => {
            // Anon chip shares parent scope — register + check inline.
            check_anon_chip_stmts(ctx, &ac.body.stmts, false, tmap, omap);
        }
        Stmt::ChipDecl(c) => {
            register_decl(ctx, &TopDecl::Chip(c.clone()));
            check_decl(ctx, &TopDecl::Chip(c.clone()), tmap, omap);
        }
        Stmt::Return { value, range } => {
            if ctx.exec_mode() != ExecMode::Exec && value.is_none() {
                ctx.emit(
                    "WS007",
                    "'return' (without value) outside an exec context",
                    range.clone(),
                );
            }
            if let Some(expr) = value {
                infer_expr(ctx, expr, tmap, omap);
            }
        }
    }
}

fn target_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// The element types of `t` viewed as a tuple. A tuple literal desugars to a
/// record keyed by element index, so `Record([("0", T0), ("1", T1)])` describes
/// the same shape as `Tuple([T0, T1])` and destructures the same way.
/// The result type of a call whose declared output is a union.
///
/// The math-variant gates (`Blend`/`lerp`/`Tween`) carry whichever variant
/// their inputs do, so a union output resolves to the type of the first
/// positional argument bound to a union-typed param. Left as the union, the
/// result would satisfy no operator overload and every use of it would fail.
/// Any other output type is returned unchanged.
fn union_output_type(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    out_index: usize,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    let declared = c.outputs[out_index].ty.clone();
    if !matches!(declared, Type::Union(_)) {
        return declared;
    }
    for (i, p) in c.params.iter().enumerate() {
        if matches!(p.ty, Type::Union(_))
            && let Some(CallArg::Positional(e)) = args.get(i)
        {
            let t = unwrap_ref(&infer_expr(ctx, e, tmap, omap));
            if !matches!(t, Type::Any) {
                return t;
            }
        }
    }
    declared
}

fn as_tuple_fields(t: &Type) -> Option<Vec<Type>> {
    match t {
        Type::Tuple(fields) => Some(fields.clone()),
        Type::Record(fields) => fields
            .iter()
            .enumerate()
            .map(|(i, (key, ft))| (*key == i.to_string()).then(|| ft.clone()))
            .collect(),
        _ => None,
    }
}

fn bind_let(ctx: &mut TypeCheckCtx, b: &LetBinding, t: &Type) {
    match b {
        LetBinding::Ident { name, range } => {
            ctx.scope.declare(
                name,
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: name.clone(),
                    ty: t.clone(),
                    decl_range: range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        LetBinding::Tuple { names, range, .. } => {
            if let Some(fields) = as_tuple_fields(t)
                && fields.len() == names.len()
            {
                for (n, ft) in names.iter().zip(fields.iter()) {
                    ctx.scope.declare(
                        n,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: n.clone(),
                            ty: ft.clone(),
                            decl_range: range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                return;
            }
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: expected tuple[{}], got {:?}",
                    names.len(),
                    t
                ),
                range.clone(),
            );
            for n in names {
                ctx.scope.declare(
                    n,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: n.clone(),
                        ty: Type::Any,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        LetBinding::Record { names, range } => {
            for n in names {
                let ty = if let Type::Record(fields) = t {
                    fields
                        .iter()
                        .find(|(k, _)| k == n)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                } else {
                    Type::Any
                };
                ctx.scope.declare(
                    n,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: n.clone(),
                        ty,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        LetBinding::RecordDestruct { fields, range } => {
            for field in fields {
                let (name, ty) = match field {
                    crate::ast::RecordDestructField::Named { name, alias, .. } => {
                        let bind_name = alias.as_ref().unwrap_or(name);
                        let field_ty = if let Type::Record(rec_fields) = t {
                            rec_fields
                                .iter()
                                .find(|(k, _)| k == name)
                                .map(|(_, t)| t.clone())
                                .unwrap_or(Type::Any)
                        } else {
                            Type::Any
                        };
                        (bind_name.clone(), field_ty)
                    }
                    crate::ast::RecordDestructField::Rest { name, .. } => {
                        // Rest collects remaining fields into a new record
                        (name.clone(), Type::Any)
                    }
                };
                ctx.scope.declare(
                    &name,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: name.clone(),
                        ty,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
    }
}

fn infer_assign_target(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    if let Expr::Ident { name, range } = e {
        if name == "_" {
            return Type::Any;
        }
        let sym = ctx.scope.lookup(name).cloned();
        match sym {
            None => {
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::Var => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Array => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::LetBinding => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)) => {
                unwrap_ref(&s.ty)
            }
            Some(s) if s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_)) => {
                unwrap_ref(&s.ty)
            }
            _ => {
                ctx.emit(
                    "WS007",
                    format!("'{name}' isn't a writable target"),
                    range.clone(),
                );
                Type::Any
            }
        }
    } else if let Expr::IndexAccess { obj, index, .. } = e {
        let obj_ty = infer_assign_target(ctx, obj, tmap, omap);
        infer_expr(ctx, index, tmap, omap);
        match obj_ty {
            Type::Array(inner) => *inner,
            _ => Type::Any,
        }
    } else {
        Type::Any
    }
}

// ---------- expression inference ----------

fn infer_expr(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    let t = infer_expr_inner(ctx, e, tmap, omap);
    let r = e.range();
    tmap.insert((r.file.clone(), r.start.offset, r.end.offset), t.clone());
    t
}

fn infer_expr_inner(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    match e {
        Expr::IntLit { .. } => Type::Int,
        Expr::FloatLit { .. } => Type::Float,
        Expr::StringLit { .. } => Type::String,
        Expr::BoolLit { .. } => Type::Bool,
        Expr::Array { elements, .. } => {
            // Type each element so it lands in the type map; the array's element
            // type is taken from the first element. A spread contributes its
            // source array's element type, a plain item its value type. Whether
            // the elements must be constant literals is enforced at the
            // declaration site (top level) — not here, since the same literal is
            // valid with runtime elements in an exec-context assignment.
            let mut elem = Type::Any;
            for (i, el) in elements.iter().enumerate() {
                let t = unwrap_ref(&infer_expr_inner(ctx, el.expr(), tmap, omap));
                let et = match el {
                    ArrayElem::Spread(_) => match t {
                        Type::Array(inner) => *inner,
                        other => other,
                    },
                    ArrayElem::Item(_) => t,
                };
                if i == 0 {
                    elem = et;
                }
            }
            Type::Array(Box::new(elem))
        }
        Expr::AssetRef { .. } => {
            // An external asset reference (`$Type/Name`) is an object/class
            // reference — typed `entity` so it can be compared against entity
            // values (e.g. `weapon == $BRItemBase/Weapon_Pickaxe`) and passed
            // into object/class gate ports (which accept `any`/entity anyway).
            // Validation against the asset catalog happens in analysis.
            Type::Entity
        }
        Expr::PrefabRef { path, range } => {
            // A prefab file reference flows into a `bundle_path_ref` gate
            // property; typed `any` so it's accepted there. The `.brz`
            // extension is required (the file resolution + embedding happens at
            // emit); flag it early so the error points at the reference.
            if !path.ends_with(".brz") {
                ctx.emit(
                    "WS019",
                    format!("prefab reference `${path}` must end in `.brz`"),
                    range.clone(),
                );
            }
            Type::Any
        }
        Expr::InterpLit { parts, .. } => {
            for p in parts {
                if let InterpPart::Expr(expr) = p {
                    let t = unwrap_ref(&infer_expr(ctx, expr, tmap, omap));
                    if coerce(&t, &Type::String) == CoerceRule::Mismatch {
                        ctx.emit(
                            "WS003",
                            format!("expected string, got {:?}", t),
                            expr.range().clone(),
                        );
                    }
                }
            }
            Type::String
        }
        Expr::Ident { name, range } => {
            let Some(sym) = ctx.scope.lookup(name).cloned() else {
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                return Type::Any;
            };
            match sym.kind {
                SymbolKind::Event => Type::Exec,
                SymbolKind::Fn | SymbolKind::Chip => Type::Any,
                SymbolKind::Var => {
                    let is_exec = ctx.exec_mode() == ExecMode::Exec;
                    ctx.var_read_contexts
                        .insert((range.file.clone(), range.start.offset), is_exec);
                    unwrap_ref(&sym.ty)
                }
                SymbolKind::Array => sym.ty.clone(),
                _ => sym.ty.clone(),
            }
        }
        Expr::Deref { operand, range } => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS006",
                    format!(
                        "'*{}' deref requires exec context — use .Value for pure reads",
                        target_name(operand).unwrap_or("<expr>".into())
                    ),
                    range.clone(),
                );
            }
            let t = infer_expr(ctx, operand, tmap, omap);
            match t {
                Type::Ref(inner) => *inner,
                Type::Any => Type::Any,
                other => {
                    ctx.emit(
                        "WS003",
                        format!("expected ref T, got {:?}", other),
                        range.clone(),
                    );
                    Type::Any
                }
            }
        }
        Expr::RefOf { operand, .. } => {
            let t = infer_expr(ctx, operand, tmap, omap);
            if matches!(t, Type::Ref(_)) {
                t
            } else {
                Type::Ref(Box::new(t))
            }
        }
        Expr::UnOp { op, operand, range } => {
            let operand_t = infer_expr(ctx, operand, tmap, omap);
            let op_key = if op == "-" { "-u" } else { op.as_str() };
            let unwrapped = unwrap_ref(&operand_t);
            let rule = resolve_op(op_key, &[unwrapped]);
            if let Some(r) = rule {
                let result = r.result.clone();
                omap.insert(
                    (range.file.clone(), range.start.offset, range.end.offset),
                    r.clone(),
                );
                result
            } else {
                let code = if matches!(op.as_str(), "&" | "|" | "^" | "~" | "<<" | ">>") {
                    "WS011"
                } else {
                    "WS004"
                };
                ctx.emit(
                    code,
                    format!("no overload for '{op}' on {:?}", operand_t),
                    range.clone(),
                );
                Type::Any
            }
        }
        Expr::BinOp {
            op,
            left,
            right,
            range,
        } => {
            let lt = infer_expr(ctx, left, tmap, omap);
            let rt = infer_expr(ctx, right, tmap, omap);
            let lt_u = unwrap_ref(&lt);
            let rt_u = unwrap_ref(&rt);
            let rule = resolve_op(op, &[lt_u, rt_u]);
            if let Some(r) = rule {
                let result = r.result.clone();
                omap.insert(
                    (range.file.clone(), range.start.offset, range.end.offset),
                    r.clone(),
                );
                result
            } else {
                let code = if matches!(op.as_str(), "&" | "|" | "^" | "<<" | ">>") {
                    "WS011"
                } else {
                    "WS004"
                };
                ctx.emit(
                    code,
                    format!("no overload for '{op}' on {:?}, {:?}", lt, rt),
                    range.clone(),
                );
                Type::Any
            }
        }
        Expr::FieldAccess { obj, field, range } => {
            let ot = infer_expr(ctx, obj, tmap, omap);
            if let Type::Ref(inner) = &ot {
                if field == "Value" || field == "prev" {
                    return inner.as_ref().clone();
                }
                if field == "VarRef" {
                    return ot.clone();
                }
            }
            // `cN.prev` where `cN` was auto-dereffed in exec context:
            // the obj's type is the inner T, not Ref(T). Look up the
            // declared var type directly.
            if (field == "Value" || field == "prev")
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && let Type::Ref(inner) = &sym.ty
            {
                return inner.as_ref().clone();
            }
            // `x.Value` on a var/ref reads through to the inner type. A record
            // with its own `Value` field (a multi-output gate result such as
            // `a.pop()` → `{ Value, IsEmpty }`) must project that field instead,
            // or `.Value` yields the whole record and every use of it mistypes.
            if (field == "value" || field == "Value") && !matches!(ot, Type::Record(_)) {
                return unwrap_ref(&ot);
            }
            if let Type::Record(fields) = &ot {
                if let Some((_, t)) = fields.iter().find(|(k, _)| k == field) {
                    return t.clone();
                }
                ctx.emit(
                    "WS010",
                    format!("no field '{field}' on {:?}", ot),
                    range.clone(),
                );
                return Type::Any;
            }
            match (&ot, field.as_str()) {
                (Type::Vector, "x" | "X" | "y" | "Y" | "z" | "Z") => Type::Float,
                (Type::Color, "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A") => Type::Float,
                (Type::Rotator, "pitch" | "yaw" | "roll") => Type::Float,
                // An array read yields the element plus a bounds flag, but it is
                // typed as the bare element (see IndexAccess), so by the time the
                // flag is projected - directly or through a `let` - the object is
                // the element type and this would fall through to Any. Lowering
                // already maps these names to the gate's bOutOfBounds port.
                (_, "OutOfBounds" | "bOutOfBounds") => Type::Bool,
                _ => Type::Any,
            }
        }
        Expr::IndexAccess { obj, index, range } => {
            let ot = unwrap_ref(&infer_expr(ctx, obj, tmap, omap));
            infer_expr(ctx, index, tmap, omap);
            match &ot {
                Type::Array(inner) => {
                    if ctx.exec_mode() != ExecMode::Exec {
                        ctx.emit(
                            "WS007",
                            format!(
                                "array index read '{}[...]' outside an exec context",
                                target_name(obj).unwrap_or("<expr>".into())
                            ),
                            range.clone(),
                        );
                    }
                    inner.as_ref().clone()
                }
                _ => Type::Any,
            }
        }
        Expr::TuplePick { obj, index, range } => {
            let ot = infer_expr(ctx, obj, tmap, omap);
            match &ot {
                Type::Tuple(fields) => fields.get(*index).cloned().unwrap_or_else(|| {
                    ctx.emit(
                        "WS010",
                        format!("tuple index .{index} out of range"),
                        range.clone(),
                    );
                    Type::Any
                }),
                Type::Record(fields) => fields
                    .get(*index)
                    .map(|(_, t)| t.clone())
                    .unwrap_or(Type::Any),
                _ => Type::Any,
            }
        }
        Expr::Call { callee, args, .. } => {
            // Side-effect: typecheck every arg.
            for a in args {
                match a {
                    CallArg::Positional(v) => {
                        infer_expr(ctx, v, tmap, omap);
                    }
                    CallArg::Named { value, .. } => {
                        infer_expr(ctx, value, tmap, omap);
                    }
                    CallArg::Spread(v) => {
                        infer_expr(ctx, v, tmap, omap);
                    }
                }
            }
            // Resolve callee if it's a plain identifier.
            if let Expr::Ident { name, range } = callee.as_ref() {
                if let Some(c) = find_call(name) {
                    if c.exec && ctx.exec_mode() != ExecMode::Exec {
                        let has_exec_arg = args
                            .iter()
                            .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                        if !has_exec_arg {
                            ctx.emit("WS007", format!("exec call '{name}' outside an exec context (pass exec = ... to override)"), range.clone());
                        }
                    }
                    // Random rides the PrimMath variant like the math operators:
                    // its min/max may be a vector/rotator/quat/color (a
                    // per-component random on the same gate), and the result then
                    // matches that type rather than the int-typed CallSpec.
                    if name == "Random" {
                        let arg_tys: Vec<Type> = args
                            .iter()
                            .filter_map(|a| match a {
                                CallArg::Positional(e) => {
                                    Some(unwrap_ref(&infer_expr(ctx, e, tmap, omap)))
                                }
                                _ => None,
                            })
                            .collect();
                        if let Some(t) = arg_tys.into_iter().find(|t| {
                            matches!(t, Type::Vector | Type::Color | Type::Rotator | Type::Quat)
                        }) {
                            return t;
                        }
                    }
                    check_call_args(ctx, c, args, range, tmap, omap);
                    if c.outputs.len() == 1 {
                        return union_output_type(ctx, c, args, 0, tmap, omap);
                    }
                    if c.outputs.len() > 1 {
                        return Type::Record(
                            c.outputs
                                .iter()
                                .map(|o| (o.port.as_str().into(), o.ty.clone()))
                                .collect(),
                        );
                    }
                    if c.exec {
                        return Type::Any;
                    }
                    return c.params.first().map(|p| p.ty.clone()).unwrap_or(Type::Any);
                }
                let Some(sym) = ctx.scope.lookup(name).cloned() else {
                    ctx.emit(
                        "WS002",
                        format!("unknown identifier '{name}'"),
                        range.clone(),
                    );
                    return Type::Any;
                };
                // Use-before-declaration. Chips/mods are registered in source
                // order during lowering, so a call whose declaration lexically
                // follows the call site cannot resolve — it would synthesise an
                // `_Unsupported` gate that silently reads 0 at runtime. Only
                // applies to same-file chip/mod decls (imports live elsewhere
                // and are always available).
                if sym.kind == SymbolKind::Chip
                    && sym.signature.is_some()
                    && sym.decl_range.file == range.file
                    && (range.start.line, range.start.col)
                        < (sym.decl_range.start.line, sym.decl_range.start.col)
                {
                    ctx.emit(
                        "WS021",
                        format!(
                            "call to `{name}` before its declaration — chips and \
                             mods must be declared before the point where they \
                             are used (move the declaration above its first caller)"
                        ),
                        range.clone(),
                    );
                }
                // Argument-count check. User chips/mods/fns have no default
                // parameters, so the positional-argument count must equal the
                // parameter count — each param (including a whole-record or
                // destructured one) takes exactly one positional arg. Named args
                // (e.g. `exec =`) aren't parameters; a spread makes the count
                // dynamic, so skip the check then. A mismatch would otherwise
                // leave a param unbound, silently reading 0 / an empty value.
                if let Some(sig) = &sym.signature {
                    let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                    if !has_spread {
                        let positional = args
                            .iter()
                            .filter(|a| matches!(a, CallArg::Positional(_)))
                            .count();
                        let expected = sig.params.len();
                        if positional != expected {
                            ctx.emit(
                                "WS022",
                                format!(
                                    "`{name}` expects {expected} argument{} but {positional} {} given",
                                    if expected == 1 { "" } else { "s" },
                                    if positional == 1 { "was" } else { "were" },
                                ),
                                range.clone(),
                            );
                        }
                    }
                }
                if let Some(sig) = sym.signature {
                    // A call with an `exec =` trigger also returns the chip's
                    // completion exec as an `exec` field (unless the chip
                    // declares its own `exec` output).
                    let has_exec_arg = args
                        .iter()
                        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                    if sig.outputs.len() == 1 && !has_exec_arg {
                        return sig.outputs[0].ty.clone();
                    }
                    if !sig.outputs.is_empty() {
                        let mut fields: Vec<(String, Type)> = sig
                            .outputs
                            .iter()
                            .map(|o| (o.name.clone(), o.ty.clone()))
                            .collect();
                        if has_exec_arg && !fields.iter().any(|(n, _)| n == "exec") {
                            fields.push(("exec".into(), Type::Exec));
                        }
                        return Type::Record(fields);
                    }
                }
            }
            // Namespace call: ns.foo(args)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Expr::Ident { name: ns_name, .. } = obj.as_ref()
                && ctx.scope.lookup(ns_name).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                let ns_lookup = ctx
                    .namespaces
                    .get(ns_name.as_str())
                    .and_then(|ns_map| ns_map.get(field.as_str()))
                    .map(|info| (info.kind, info.return_type.clone()));
                match ns_lookup {
                    Some((_, Some(ret))) => return resolve_type_expr(ctx, &ret),
                    Some((_, None)) => return Type::Any,
                    None => {
                        ctx.emit(
                            "WS002",
                            format!("'{}' not found in namespace '{}'", field, ns_name),
                            fa_range.clone(),
                        );
                        return Type::Any;
                    }
                }
            }
            // Array method call: arr.push(val), arr.length(), arr.pop(), etc.
            // Any array-typed value works (an `array` decl or a `var ids: T[]`),
            // gated on the field actually being an array method.
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && (sym.kind == SymbolKind::Array || matches!(unwrap_ref(&sym.ty), Type::Array(_)))
                && crate::catalog::arrays::is_array_method(field)
            {
                let elem = match unwrap_ref(&sym.ty) {
                    Type::Array(inner) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                // Return type is derived from the method's gate
                // output ports (see catalog::arrays). Multi-output
                // gates (e.g. find) yield a record that auto-unwraps
                // to whichever field matches the use.
                return crate::catalog::arrays::array_return_type(field, &elem)
                    .unwrap_or(Type::Any);
            }
            // Receiver method call: entity.SetLocation(pos)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(c) = find_call(field)
                && c.receiver.is_some()
            {
                let mut recv_args = vec![CallArg::Positional(obj.as_ref().clone())];
                recv_args.extend(args.iter().cloned());
                check_call_args(ctx, c, &recv_args, fa_range, tmap, omap);
                if c.outputs.len() == 1 {
                    return union_output_type(ctx, c, &recv_args, 0, tmap, omap);
                }
                if c.outputs.len() > 1 {
                    return Type::Record(
                        c.outputs
                            .iter()
                            .map(|o| (o.port.as_str().into(), o.ty.clone()))
                            .collect(),
                    );
                }
                return Type::Any;
            }
            // A method/namespace call whose base identifier resolves to nothing:
            // e.g. `card.drawLobby(...)` after an `import * as card` was removed.
            // None of the branches above matched and `card` is not a namespace,
            // variable, or value in scope. Left alone this silently lowers to an
            // `_Unsupported` gate that reads a default (does nothing) at runtime —
            // flag the dangling base, mirroring the bare-identifier WS002 above.
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident {
                    name,
                    range: base_range,
                } = obj.as_ref()
                && ctx.scope.lookup(name).is_none()
                && find_call(name).is_none()
            {
                ctx.emit(
                    "WS002",
                    format!(
                        "unknown identifier '{name}' in call `{name}.{field}(...)` — \
                         no namespace, variable, or value named '{name}' is in scope \
                         (is an import missing?)"
                    ),
                    base_range.clone(),
                );
            }
            Type::Any
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range,
            ..
        } => {
            ctx.if_contexts
                .insert((range.file.clone(), range.start.offset), false);
            infer_expr(ctx, cond, tmap, omap);
            let tt = infer_expr(ctx, then_branch, tmap, omap);
            let et = infer_expr(ctx, else_branch, tmap, omap);
            if coerce(&tt, &et) == CoerceRule::Mismatch {
                ctx.emit(
                    "WS003",
                    format!(
                        "if-then-else branch type mismatch: then is {}, else is {} (Select output follows else type)",
                        crate::analysis::types::type_str(&tt),
                        crate::analysis::types::type_str(&et),
                    ),
                    range.clone(),
                );
            }
            et
        }
        Expr::BlockExpr { stmts, value, .. } => {
            ctx.scope.push();
            for s in stmts {
                check_stmt(ctx, s, tmap, omap);
            }
            let t = infer_expr(ctx, value, tmap, omap);
            ctx.scope.pop();
            t
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            infer_expr(ctx, scrutinee, tmap, omap);
            let mut tys: Vec<Type> = Vec::new();
            for arm in arms {
                if let MatchBody::Expr(expr) = &arm.body {
                    tys.push(infer_expr(ctx, expr, tmap, omap));
                }
            }
            if tys.is_empty() {
                Type::Any
            } else if tys
                .iter()
                .all(|t| std::mem::discriminant(t) == std::mem::discriminant(&tys[0]))
            {
                tys[0].clone()
            } else {
                Type::Union(tys)
            }
        }
        Expr::RecordLit { fields, .. } => {
            let mut rec_fields: Vec<(String, Type)> = Vec::new();
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        let ty = infer_expr(ctx, value, tmap, omap);
                        // Override if field already exists (from spread)
                        if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                            existing.1 = ty;
                        } else {
                            rec_fields.push((name.clone(), ty));
                        }
                    }
                    RecordLitField::Shorthand { name, .. } => {
                        let ty = ctx
                            .scope
                            .lookup(name)
                            .map(|s| s.ty.clone())
                            .unwrap_or(Type::Any);
                        if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                            existing.1 = ty;
                        } else {
                            rec_fields.push((name.clone(), ty));
                        }
                    }
                    RecordLitField::Spread { value, .. } => {
                        let spread_ty = infer_expr(ctx, value, tmap, omap);
                        if let Type::Record(spread_fields) = spread_ty {
                            for (fname, fty) in spread_fields {
                                if let Some(existing) =
                                    rec_fields.iter_mut().find(|(n, _)| *n == fname)
                                {
                                    existing.1 = fty;
                                } else {
                                    rec_fields.push((fname, fty));
                                }
                            }
                        }
                    }
                }
            }
            Type::Record(rec_fields)
        }
    }
}

fn unwrap_ref(t: &Type) -> Type {
    match t {
        Type::Ref(inner) => inner.as_ref().clone(),
        other => other.clone(),
    }
}

fn check_call_args(
    ctx: &mut TypeCheckCtx,
    spec: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    range: &SourceRange,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    let positional: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            _ => None,
        })
        .collect();
    let required_count = spec.params.iter().filter(|p| !p.optional).count();
    if positional.len() > spec.params.len() {
        ctx.emit(
            "WS011",
            format!(
                "'{}' expects at most {} positional arg{}, got {}",
                spec.name,
                spec.params.len(),
                if spec.params.len() == 1 { "" } else { "s" },
                positional.len(),
            ),
            range.clone(),
        );
    } else if positional.len() < required_count {
        ctx.emit(
            "WS011",
            format!(
                "'{}' requires {} arg{}, got {}",
                spec.name,
                required_count,
                if required_count == 1 { "" } else { "s" },
                positional.len(),
            ),
            range.clone(),
        );
    }
    for (i, arg_expr) in positional.iter().enumerate() {
        if i >= spec.params.len() {
            break;
        }
        let arg_ty = unwrap_ref(&infer_expr(ctx, arg_expr, tmap, omap));
        let param = &spec.params[i];
        if coerce(&arg_ty, &param.ty) == CoerceRule::Mismatch {
            ctx.emit(
                "WS003",
                format!(
                    "argument '{}': expected {:?}, got {:?}",
                    param.name, param.ty, arg_ty,
                ),
                arg_expr.range().clone(),
            );
        }
    }
}

fn expect_coerce(ctx: &mut TypeCheckCtx, from: &Type, to: &Type, range: &SourceRange) {
    if coerce(from, to) == CoerceRule::Mismatch {
        ctx.emit(
            "WS003",
            format!("expected {:?}, got {:?}", to, from),
            range.clone(),
        );
    }
}

fn check_let_type_annotation(
    ctx: &mut TypeCheckCtx,
    l: &crate::ast::LetDecl,
    inferred: &Type,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    if let Some(ref te) = l.typ {
        // Record literals: validate field names against the expected record type.
        // Point errors at the specific field/spread that introduced the mismatch.
        if let Expr::RecordLit { fields, .. } = &l.value {
            let expected = resolve_type_expr(ctx, te);
            if let Type::Record(expected_fields) = &expected {
                let type_name = crate::analysis::types::type_expr_str(te);
                // Check each field/spread for extra fields
                for f in fields {
                    match f {
                        RecordLitField::Named { name, range, .. } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{}' not in type {}", name, type_name),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Shorthand { name, range } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{}' not in type {}", name, type_name),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Spread { value, range } => {
                            let spread_ty = infer_expr(ctx, value, tmap, omap);
                            if let Type::Record(spread_fields) = &spread_ty {
                                let extras: Vec<&str> = spread_fields
                                    .iter()
                                    .filter(|(n, _)| !expected_fields.iter().any(|(en, _)| en == n))
                                    .map(|(n, _)| n.as_str())
                                    .collect();
                                if !extras.is_empty() {
                                    ctx.emit(
                                        "WS003",
                                        format!(
                                            "spread introduces fields not in {}: {}",
                                            type_name,
                                            extras.join(", ")
                                        ),
                                        range.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
                // Check for missing fields (use the whole literal range)
                if let Type::Record(inferred_fields) = inferred {
                    for (fname, _) in expected_fields {
                        if !inferred_fields.iter().any(|(n, _)| n == fname) {
                            ctx.emit(
                                "WS003",
                                format!("missing field '{}' for type {}", fname, type_name),
                                l.range.clone(),
                            );
                        }
                    }
                }
            }
            return;
        }
        let expected = resolve_type_expr(ctx, te);
        let rule = coerce(inferred, &expected);
        // `ViaString` is fine: anything primitive casts to string, so
        // `let s: string = 5` is an intentional format, not a type lie.
        if rule == CoerceRule::Mismatch {
            let name = match &l.binding {
                crate::ast::LetBinding::Ident { name, .. } => name.clone(),
                _ => "<binding>".into(),
            };
            ctx.diagnostics.push(crate::Diagnostic {
                severity: crate::diagnostic::Severity::Warning,
                code: "WS016".into(),
                message: format!(
                    "let '{}' annotated as {}, but expression has type {}",
                    name,
                    crate::analysis::types::type_expr_str(te),
                    crate::analysis::types::type_str(inferred),
                ),
                range: l.range.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn tc(src: &str) -> TypeCheckResult {
        let p = parse(src, "test");
        assert!(
            p.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            p.diagnostics
        );
        typecheck(&p.ast, "test")
    }

    fn assert_no_diags(r: &TypeCheckResult) {
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    }

    #[test]
    fn use_before_declaration_is_ws021() {
        // A chip/mod call whose declaration lexically follows the call site
        // cannot resolve during lowering (decls register in source order), so
        // typecheck flags it so the editor surfaces it before compiling.
        let r = tc("mod caller() { let x = target(1) }\nmod target(n: int) -> int { return n }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS021"),
            "use-before-declaration must emit WS021; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn declaration_before_use_no_ws021() {
        let r = tc("mod target(n: int) -> int { return n }\nmod caller() { let x = target(1) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS021"),
            "declaration-before-use must NOT emit WS021; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn random_is_polymorphic_on_prim_math_variant() {
        // Random rides the PrimMath variant like the math operators: min/max may
        // be a vector/rotator/quat/color and the result matches, so assigning it
        // to a same-typed var is clean (no WS003 int-mismatch).
        let r = tc("in a: vector\nin b: vector\nin c1: color\nin c2: color\nvar rv: vector = Vec(0.0, 0.0, 0.0)\nvar rc: color = ColorHex(\"#000000\")\nin go: exec\non go {\n  rv = Random(a, b)\n  rc = Random(c1, c2)\n}");
        assert_no_diags(&r);
    }

    #[test]
    fn random_int_stays_int() {
        // The scalar path is unchanged: Random(int, int) is an int, so it does
        // NOT assign into a vector var.
        let ok = tc("var n: int = 0\nin go: exec\non go { n = Random(1, 10) }");
        assert_no_diags(&ok);
        let bad = tc("var v: vector = Vec(0.0, 0.0, 0.0)\nin go: exec\non go { v = Random(1, 10) }");
        assert!(
            bad.diagnostics.iter().any(|d| d.severity == Severity::Error),
            "Random(int, int) is int and must not assign into a vector var; got {:?}",
            bad.diagnostics
        );
    }

    #[test]
    fn asset_in_array_initializer_warns_ws024() {
        // Asset/prefab references are object references wired in from their own
        // brick; they can't bake into a constant array initializer (they'd be
        // silently dropped), so warn.
        let r = tc(
            "array songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]",
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS024" && d.severity == Severity::Warning),
            "asset in array initializer should warn WS024; got {:?}",
            r.diagnostics
        );
        // A constant array initializer must NOT warn.
        let ok = tc("array nums: int[] = [1, 2, 3]");
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS024"),
            "constant array initializer must not warn; got {:?}",
            ok.diagnostics
        );
    }

    #[test]
    fn wrong_arg_count_is_ws022() {
        // User chips/mods have no default params, so too few (or too many)
        // positional args leaves a param unbound / an arg dropped.
        let too_few = tc("mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }");
        assert!(
            too_few.diagnostics.iter().any(|d| d.code == "WS022"),
            "too-few args must emit WS022; got {:?}",
            too_few.diagnostics
        );
        let too_many =
            tc("mod g(a: int) -> int { return a }\nin z: exec\non z { let x = g(1, 2) }");
        assert!(
            too_many.diagnostics.iter().any(|d| d.code == "WS022"),
            "too-many args must emit WS022; got {:?}",
            too_many.diagnostics
        );
    }

    #[test]
    fn correct_arg_count_no_ws022() {
        // Matching arity, and an extra `exec =` trigger (not a parameter), are
        // both fine.
        let ok = tc("mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1, 2) }");
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS022"),
            "matching arity must NOT emit WS022; got {:?}",
            ok.diagnostics
        );
    }

    #[test]
    fn empty_script() {
        let r = tc("");
        assert_no_diags(&r);
    }

    #[test]
    fn var_int_init() {
        assert_no_diags(&tc("var x: int = 0"));
    }

    #[test]
    fn var_float_int_mismatch_coerces() {
        assert_no_diags(&tc("var x: float = 1"));
    }

    #[test]
    fn var_string_annotation_ok() {
        // Strings can now be stored in vars (WireGraphVariant supports `str`).
        assert_no_diags(&tc("var x: string = \"hi\""));
    }

    #[test]
    fn var_string_inferred_ok() {
        assert_no_diags(&tc("var x = \"hello\""));
    }

    #[test]
    fn var_string_inferred_usable_as_string() {
        // The inferred type must actually be `string`, not `any` — an `any`
        // operand has no `==` overload and would emit WS004.
        assert_no_diags(&tc("var s = \"\"\nout r = s == \"ready\""));
    }

    #[test]
    fn var_int_inferred_usable_in_math() {
        assert_no_diags(&tc("var n = 0\nout d = n + 1"));
    }

    #[test]
    fn var_float_inferred_usable_in_math() {
        assert_no_diags(&tc("var f = 1.5\nout d = f * 2.0"));
    }

    #[test]
    fn var_bool_inferred_usable_in_logic() {
        assert_no_diags(&tc("var b = true\nout d = b && false"));
    }

    #[test]
    fn var_negative_literal_inferred() {
        assert_no_diags(&tc("var n = -5\nout d = n + 1"));
    }

    #[test]
    fn var_nonliteral_init_refines_type() {
        // `var v = Vec(…)` has no literal init; the type refines from the
        // RHS in pass 2 (buffer-style), so vector math resolves.
        assert_no_diags(&tc(
            "var v = Vec(1.0, 2.0, 3.0)\nout d = v + Vec(0.0, 0.0, 1.0)",
        ));
    }

    #[test]
    fn handler_local_var_inferred() {
        assert_no_diags(&tc(
            "on RoundStart { var v = Vec(1.0, 2.0, 3.0)\n let w = v + v }",
        ));
    }

    #[test]
    fn var_inferred_type_catches_mismatch() {
        // Inference makes the var `int`, so assigning a vector is a real
        // WS003 — under the old `any` placeholder this passed silently.
        let r = tc("var n = 0\non RoundStart { n = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into inferred int var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn let_string_annotation_accepts_numeric() {
        // Everything primitive casts to string, so a string annotation on a
        // numeric expression is a format, not a WS016 type lie.
        assert_no_diags(&tc("let s: string = 5"));
    }

    #[test]
    fn let_string_annotation_accepts_entity_family() {
        assert_no_diags(&tc("in c: controller\nlet msg: string = c"));
    }

    #[test]
    fn concat_casts_character_to_string() {
        assert_no_diags(&tc("in p: character\nout s = \"hi \" .. p"));
    }

    #[test]
    fn vector_array_init_elements_are_constants() {
        // Constant Vec(…) folds to a literal, so it's a legal top-level
        // array initializer element (previously WS003).
        assert_no_diags(&tc(
            "array pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]",
        ));
    }

    #[test]
    fn var_array_of_vectors_infers_element_type() {
        // literal_expr_type knows constructor calls, so an unannotated
        // `var foo = [Vec(…)]` infers vector[] instead of any[].
        assert_no_diags(&tc("var pts = [Vec(1.0, 1.0, 1.0)]"));
    }

    #[test]
    fn color_var_inferred_and_reassignable() {
        // Color() now returns `color` (was `any`), so the var refines and a
        // later color assignment typechecks.
        assert_no_diags(&tc(
            "var tint = Color(1.0, 0.0, 0.0)\non RoundStart { tint = Color(0.0, 1.0, 0.0) }",
        ));
    }

    #[test]
    fn color_var_rejects_vector_assignment() {
        let r = tc("var tint = Color(1.0, 0.0, 0.0)\non RoundStart { tint = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into color var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_string_in_handler_ok() {
        assert_no_diags(&tc("on RoundStart { var x: string = \"hi\" }"));
    }

    #[test]
    fn let_string_is_fine() {
        let r = tc("let x = \"hello\"");
        assert_no_diags(&r);
    }

    #[test]
    fn unknown_event_diag() {
        let r = tc("on Bogus { }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS001"));
    }

    #[test]
    fn known_event_no_diag() {
        let r = tc("on RoundStart { }");
        assert_no_diags(&r);
    }

    #[test]
    fn expr_trigger_bool_and_compiles() {
        // `on a && b { x = 1 }` is desugared by the parser to
        //   let _on_expr_0 = a && b
        //   on _on_expr_0 { x = 1 }
        // Both steps should typecheck without errors.
        let src = "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }";
        assert_no_diags(&tc(src));
    }

    #[test]
    fn handler_event_param_typed() {
        let r = tc("on CharacterDied(c) { }");
        assert_no_diags(&r);
    }

    #[test]
    fn assignment_in_handler_ok() {
        let r = tc("var n: int = 0\non RoundStart { n = n + 1 }");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    }

    #[test]
    fn assignment_outside_exec_diag() {
        // Top-level assigns trip WS007 because there's no enclosing exec chain.
        let r = tc("var n: int = 0\nn = 1");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS007"));
    }

    #[test]
    fn binop_resolution_recorded() {
        let r = tc("var x: int = 1\nvar y = x + 2");
        // We don't care about the *contents* of opResolutions deeply here;
        // just that something was recorded.
        assert!(!r.op_resolutions.is_empty());
    }

    #[test]
    fn unknown_var_emits_diag() {
        let r = tc("on RoundStart { x = 1 }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS002"));
    }

    #[test]
    fn namespace_call_with_undefined_base_is_ws002() {
        // A namespace-qualified call whose base identifier isn't in scope — e.g.
        // an `import * as card` was removed but `card.drawLobby(...)` calls
        // remain. None of the namespace/array/receiver branches match, so
        // without an explicit check the call silently lowers to an
        // `_Unsupported` gate that does nothing at runtime.
        let r = tc("mod drawLobby(n: int) { }\non RoundStart { card.drawLobby(1) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS002" && d.message.contains("card")),
            "undefined namespace base must emit WS002; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn return_in_handler_no_error() {
        let r = tc("var x: int = 0\non RoundStart { x = 1\nreturn\nx = 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn return_in_exec_no_error() {
        let r = tc("var x: int = 0\non RoundStart { if x > 5 { return } }");
        assert_no_diags(&r);
    }

    #[test]
    fn not_on_int_no_error() {
        let r = tc("var x: int = 0\nlet y = !x");
        assert_no_diags(&r);
    }

    #[test]
    fn interp_ref_var_no_error() {
        let r = tc("var x: int = 0\nlet s = \"value: ${x}\"");
        assert_no_diags(&r);
    }

    // ---- chip single-output auto-unwrap ----
    #[test]
    fn chip_single_output_pure() {
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f == 42",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn chip_single_output_exec() {
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nvar err: int = 0\non RoundStart {\n  if f != 42 { err = 1 }\n}",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn chip_single_output_field_access_compat() {
        // f.result should still work for backwards compatibility
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f.result",
        );
        assert_no_diags(&r);
    }

    // ---- buffer ----
    #[test]
    fn buffer_decl() {
        let r = tc("var x: int = 0\nbuffer prev: int = x");
        assert_no_diags(&r);
    }

    #[test]
    fn buffer_inferred_type() {
        let r = tc("var x: int = 0\nbuffer prev = x + 1");
        assert_no_diags(&r);
    }

    // ---- mod / inline chip ----
    #[test]
    fn mod_decl_no_error() {
        let r = tc("mod inc(v: *int) { v = v + 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn mod_call_in_exec() {
        let r = tc("var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart { inc(x) }");
        assert_no_diags(&r);
    }

    // ---- anonymous chip ----
    #[test]
    fn anon_chip_shares_scope() {
        let r = tc("var x: int = 0\nchip { var y: int = 0 }\non RoundStart { x = 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn chip_on_handler() {
        let r = tc("var x: int = 0\nchip on RoundStart { x = 1 }");
        assert_no_diags(&r);
    }

    // ---- emit ----
    #[test]
    fn emit_in_exec() {
        let r = tc("var x: int = 0\nout result = x\non RoundStart { emit result }");
        assert_no_diags(&r);
    }

    // ---- bool literal ----
    #[test]
    fn bool_literal() {
        let r = tc("var x: bool = true\nvar y: bool = false");
        assert_no_diags(&r);
    }

    // ---- chip exec param as trigger ----
    #[test]
    fn chip_exec_param_trigger() {
        let r = tc(
            "chip Counter(bump: exec, reset: exec) -> (value: int) {\n  var n: int = 0\n  on bump { n = n + 1 }\n  on reset { n = 0 }\n  out value = n.Value\n}",
        );
        assert_no_diags(&r);
    }

    // ---- character to entity coercion ----
    #[test]
    fn character_coerces_to_entity() {
        let r = tc("in ch: character\non RoundStart { ch.SetLocation(Vec(0.0, 0.0, 0.0)) }");
        assert_no_diags(&r);
    }

    // ---- call arg validation ----
    #[test]
    fn call_too_many_args() {
        let r = tc("on RoundStart { Random(1, 2, 3, 4, 5) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
    }

    #[test]
    fn call_wrong_arg_type() {
        let r = tc("on RoundStart { SetLocation(42, Vec(0.0, 0.0, 0.0)) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("argument"))
        );
    }

    // ---- namespace import ----
    #[test]
    fn namespace_symbol_registered() {
        use crate::resolve::{MemLoader, resolve};
        let loader = MemLoader {
            files: [("lib.ws".into(), "mod foo(v: *int) { v = v + 1 }".into())].into_iter().collect(),
        };
        let resolved = resolve("import * as lib from \"lib\"", "main.ws", &loader);
        let r = typecheck(&resolved.ast, "main.ws");
        assert_no_diags(&r);
    }

    // ---- chip let ----
    #[test]
    fn chip_let_pure_context() {
        let r = tc("var x: int = 0\nchip let doubled = x * 2");
        assert_no_diags(&r);
    }

    // ---- receiver call ----
    #[test]
    fn receiver_call_method() {
        let r = tc("var ctrl: controller\non RoundStart { ctrl.DisplayText(\"hi\") }");
        assert_no_diags(&r);
    }

    #[test]
    fn entity_receiver_accepts_character_controller_methods() {
        // An entity wire (e.g. Sweep's HitEntity) can be a player, so
        // character/controller receiver methods and params accept it.
        let r = tc("in e: entity\nin t: exec\non t { e.ShowStatusMessage(\"hi\") }");
        assert_no_diags(&r);
        let r2 = tc("in e: entity\nin t: exec\non t { ShowStatusMessage(e, \"hi\") }");
        assert_no_diags(&r2);
    }

    // ---- array index ----
    #[test]
    fn array_index_returns_element_type() {
        // Array reads require exec context (compile to Exec_ArrayVar_Get).
        let r = tc(
            "array items: int[]\nin trigger: exec\non trigger { let x = items[0]\nlet ok = x + 1 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn array_index_outside_exec_is_ws007() {
        // Array index read in pure context should emit WS007.
        let r = tc("array items: int[]\nlet x = items[0]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "expected WS007 for array index read outside exec context"
        );
    }

    #[test]
    fn array_param_index() {
        // Array params put the mod in exec context, so arr[idx] is fine.
        let r = tc("mod process(arr: int[], idx: int) {\n  let old = arr[idx]\n  out r = old\n}");
        assert_no_diags(&r);
    }

    #[test]
    fn array_param_index_dot_value() {
        // arr[i].value works fine — array params put the mod in exec context.
        let r =
            tc("mod process(arr: int[], idx: int) {\n  let old = arr[idx].value\n  out r = old\n}");
        assert_no_diags(&r);
    }

    // ---- array methods ----
    #[test]
    fn array_push_pop() {
        let r =
            tc("array items: int[]\nin trigger: exec\non trigger { items.push(1)\nitems.pop() }");
        assert_no_diags(&r);
    }

    #[test]
    fn array_length_returns_int() {
        let r = tc(
            "array items: int[]\nin trigger: exec\non trigger { let len = items.length()\nlet ok = len + 1 }",
        );
        assert_no_diags(&r);
        // len should be Int, so len + 1 should resolve without error.
        // If length() returned Any, the + would still work (Any coerces),
        // so also check the inferred type directly.
        let len_type = r.type_of_expr.values().find(|t| **t == Type::Int);
        assert!(len_type.is_some(), "length() should infer as Int");
    }

    // ---- if expression (ternary) ----
    #[test]
    fn if_expr_ternary() {
        let r = tc("var x: int = 0\nlet y = if x > 0 then 1 else 0");
        assert_no_diags(&r);
    }

    // ---- string interpolation ----
    #[test]
    fn string_interp_multiple() {
        let r = tc("var a: int = 1\nvar b: float = 2.0\nlet s = \"a=${a} b=${b}\"");
        assert_no_diags(&r);
    }

    // ---- octal/hex/binary literals ----
    #[test]
    fn numeric_literal_bases() {
        let r = tc("var a: int = 0xFF\nvar b: int = 0b1010\nvar c: int = 0o77");
        assert_no_diags(&r);
    }

    // ---- records & type aliases ----
    #[test]
    fn type_alias_record() {
        let r = tc("type Point = { x: int, y: int }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_literal_typed() {
        let r = tc("type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_field_access() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet sum = p.x + p.y",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_shorthand() {
        let r =
            tc("type Point = { x: int, y: int }\nlet x = 1\nlet y = 2\nlet p: Point = { x, y }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_spread() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet a: Point = { x: 1, y: 2 }\nlet b: Point = { ...a, y: 99 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_destructure() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet { x, y } = p\nlet sum = x + y",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_as_mod_param() {
        let r = tc(
            "type Point = { x: int, y: int }\nmod sum(p: Point) -> (r: int) { return p.x + p.y }\nlet p: Point = { x: 3, y: 4 }\nlet s = sum(p)",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn mod_param_record_destruct() {
        let r = tc(
            "type Point = { x: int, y: int }\nmod add({ x, y }: Point) -> int { return x + y }\nlet p: Point = { x: 3, y: 4 }\nlet sum = add(p)",
        );
        assert_no_diags(&r);
    }
}
