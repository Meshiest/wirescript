//! Pass 1: register every declaration's symbol before anything is checked.

use super::*;

/// A namespace value member built from a `let`, with `ty` as its value type.
fn ns_let_member(ty: Option<Type>) -> NsDeclInfo {
    NsDeclInfo {
        kind: SymbolKind::LetBinding,
        return_type: None,
        params: Vec::new(),
        value_type: ty,
    }
}

/// Field `name` of `t`, when `t` is a known record that has it.
fn ns_record_field(t: Option<&Type>, name: &str) -> Option<Type> {
    match t? {
        Type::Record(fields) => fields.iter().find(|(k, _)| k == name).map(|(_, ft)| ft.clone()),
        _ => None,
    }
}

/// The type of a namespace value member's initializer, for the members that
/// carry no annotation. `literal_expr_type` stops at primitives, so a
/// record-valued export (`let origin = { x: 0.0, y: 0.0 }`), one that names
/// another member, or one that projects a field off another all read as `any`
/// from the importing module — and every use of them against a concrete type
/// then finds no overload.
///
/// Deliberately all-or-nothing: a record types only when every field does, so
/// a member this can't derive keeps the `any` it has today rather than
/// acquiring a half-known type that could reject a use that works now.
///
/// Members resolve against `ns_map` as built so far — source order. A forward
/// reference yields `None` and stays `any`. Spread and duplicate
/// keys follow the same last-wins rule as `infer`'s record-literal arm.
fn ns_value_type(
    ns_map: &HashMap<String, NsDeclInfo>,
    outer: &HashMap<String, HashMap<String, NsDeclInfo>>,
    e: &Expr,
) -> Option<Type> {
    if let Some(t) = literal_expr_type(e) {
        return Some(t);
    }
    let member = |name: &str| ns_map.get(name).and_then(|i| i.value_type.clone());
    match e {
        Expr::Ident { name, .. } => member(name),
        Expr::FieldAccess { obj, field, .. } => {
            if let Some(t) = ns_record_field(ns_value_type(ns_map, outer, obj).as_ref(), field) {
                return Some(t);
            }
            // `L.member` where `L` is ANOTHER namespace - the one this module
            // imported for itself, which travels in beside us. It is registered
            // before us (resolve prepends it), so its members are already in
            // `outer`. Without this a member initialized from one typed as
            // nothing, and reading it from the importer was a WS002 "not a
            // readable value".
            let Expr::Ident { name: base, .. } = obj.as_ref() else {
                return None;
            };
            outer
                .get(base.as_str())
                .and_then(|m| m.get(field.as_str()))
                .and_then(|i| i.value_type.clone())
        }
        Expr::RecordLit { fields, .. } => {
            let mut rec: Vec<(String, Type)> = Vec::new();
            let mut set = |name: &str, ty: Type| {
                match rec.iter_mut().find(|(n, _)| n == name) {
                    Some(existing) => existing.1 = ty,
                    None => rec.push((name.to_string(), ty)),
                }
            };
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        set(name, ns_value_type(ns_map, outer, value)?)
                    }
                    RecordLitField::Shorthand { name, .. } => set(name, member(name)?),
                    RecordLitField::Spread { value, .. } => match ns_value_type(ns_map, outer, value)? {
                        Type::Record(inner) => {
                            for (n, ty) in inner {
                                set(&n, ty);
                            }
                        }
                        _ => return None,
                    },
                }
            }
            Some(Type::Record(rec))
        }
        _ => None,
    }
}

/// Populate `ctx.generic_type_aliases` from every `type Name<T, …> = …`
/// declaration in `decls` (top-level and namespaced). Deliberately does NOT
/// resolve the alias body here — it's still parametric (references its own
/// free `type_params`, unbound until a use site supplies concrete args), so
/// resolving it now would spuriously flag those params as unknown types.
/// Instantiation happens lazily, per use, in `types::resolve::resolve_type`.
pub(super) fn collect_generic_aliases(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    for d in decls {
        match d {
            TopDecl::TypeAlias(t) if !t.type_params.is_empty() => {
                ctx.generic_type_aliases.insert(
                    t.name.clone(),
                    crate::types::resolve::GenericAlias {
                        params: t.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        body: t.typ.clone(),
                    },
                );
            }
            TopDecl::Namespace(ns) => {
                for nd in &ns.decls {
                    if let TopDecl::TypeAlias(t) = nd
                        && !t.type_params.is_empty()
                    {
                        let qualified = format!("{}.{}", ns.name, t.name);
                        ctx.generic_type_aliases.insert(
                            qualified,
                            crate::types::resolve::GenericAlias {
                                params: t.type_params.iter().map(|tp| tp.name.clone()).collect(),
                                body: t.typ.clone(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn register_builtin_events(ctx: &mut TypeCheckCtx) {
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
                            is_const: false,
                        })
                        .collect(),
                ),
            },
        );
    }
}

/// Seed the built-in `Option<T>`/`Result<T, E>` prelude (`enums::prelude_enum_defs`)
/// and the built-in game enums (`enums::game_enum_defs`) into `ctx.enum_defs`
/// and declare their type-name scope symbols - the same `SymbolKind::Type`
/// entry `TopDecl::Enum`'s own registration below declares for a user enum,
/// so `Option<int>`/`Result<int, string>` and a game enum like
/// `EasingFunction` all resolve as types with no `enum` declaration anywhere
/// in the program. Run alongside `register_builtin_events`, i.e. BEFORE
/// `collect_enum_defs`'s pre-pass over `decls` - a user enum that redeclares
/// one of these names then overwrites this seed in `ctx.enum_defs` (matching
/// `enums::build_registry`'s identical merge order) while `declare_or_dup`
/// still reports the redeclaration (`WS013`), same as any other duplicate
/// top-level name.
///
/// Bare variant names (`Some`/`None`/`Ok`/`Err` used unqualified) are NOT
/// declared here - they are not scope symbols at all, only enum members.
/// Resolving `Some(42)` to `Option.Some(42)` happens in `infer.rs`, keyed off
/// this same `ctx.enum_defs` entry (see `resolve_bare_variant_enum`).
pub(super) fn register_builtin_enums(ctx: &mut TypeCheckCtx) {
    for def in crate::typecheck::enums::prelude_enum_defs()
        .into_iter()
        .chain(crate::typecheck::enums::game_enum_defs())
    {
        ctx.scope.declare(
            &def.name,
            SymbolInfo {
                kind: SymbolKind::Type,
                name: def.name.clone(),
                ty: Type::Enum {
                    name: def.name.clone(),
                    args: vec![],
                },
                decl_range: SourceRange::default(),
                signature: None,
                event_data: None,
            },
        );
        Arc::make_mut(&mut ctx.enum_defs).insert(def.name.clone(), def);
    }
}

// ---------- decl registration (1st pass) ----------

pub(super) fn register_decl(ctx: &mut TypeCheckCtx, d: &TopDecl) {
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
            if let Some(t) = &v.typ {
                // A `var` can be scalar, array (`var x: T[]`), or map
                // (`var m: Map<K,V>`) storage — each needs a concrete element/
                // value wire type. Reject `any` in the STORED position: the
                // element for an array, the value for a map, else the type
                // itself. (Matches the `array`/`map` decl checks below, so the
                // `var` form has full storage-type parity with them.)
                let (stored, what) = match &inner {
                    Type::Array(elem) => (elem.as_ref(), "an array's element type"),
                    Type::Map(_, val) => (val.as_ref(), "a map's value type"),
                    other => (other, "a variable gate"),
                };
                reject_any_storage(ctx, stored, type_expr_range(t), what);
            }
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
            reject_any_storage(
                ctx,
                &inner,
                type_expr_range(&a.element_type),
                "an array's element type",
            );
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
        TopDecl::Map(m) => {
            let key = resolve_type_expr(ctx, &m.key_type);
            let value = resolve_type_expr(ctx, &m.value_type);
            reject_any_storage(
                ctx,
                &value,
                type_expr_range(&m.value_type),
                "a map's value type",
            );
            declare_or_dup(
                ctx,
                &m.name,
                SymbolInfo {
                    kind: SymbolKind::Map,
                    name: m.name.clone(),
                    ty: Type::Map(Box::new(key), Box::new(value)),
                    decl_range: m.range.clone(),
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
            if let Some(t) = &b.typ {
                reject_any_storage(ctx, &placeholder, type_expr_range(t), "a buffer");
            }
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
            warn_any_annotation(ctx, &t, type_expr_range(&d.typ));
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
                .map(|p| {
                    let ty = resolve_type_expr(ctx, &p.typ);
                    warn_any_annotation(ctx, &ty, type_expr_range(&p.typ));
                    EventDataField {
                        name: p.name.clone(),
                        ty,
                        is_const: p.is_const,
                    }
                })
                .collect();
            let ret = f
                .return_type
                .as_ref()
                .map(|t| {
                    let ty = resolve_type_expr(ctx, t);
                    warn_any_annotation(ctx, &ty, type_expr_range(t));
                    ty
                })
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
                        variadic: false,
                        outputs: vec![EventDataField {
                            name: "_".into(),
                            ty: ret,
                            is_const: false,
                        }],
                        type_params: Vec::new(),
                    }),
                    event_data: None,
                },
            );
        }
        TopDecl::Chip(c) => {
            // Generic *chips* (physical microchips, `inline == false`) are
            // monomorphized per distinct type instantiation at lowering time:
            // `lower_chip_call_instance` builds one microchip template per
            // `(name, concrete subst)` and keys the emitted grid on it, so
            // `Box<int>` and `Box<vector>` never share a body. (Generic `mod`s
            // still inline per call site.) Nothing to reject here.
            // Generic decl: register each type param as a scope `Type`
            // symbol resolving to `Type::Param(name)` so the sig's own
            // `TypeExpr`s (below) see `T` as a known type instead of
            // WS002. Scoped to this signature's resolution only — popped
            // before `declare_or_dup` so it never leaks to sibling decls.
            let has_type_params = !c.type_params.is_empty();
            // Name + mask for each type param, computed once at registration
            // (mirrors the scope-declare loop just below) so the call-typing
            // path can later solve `Constraint`s against it without having to
            // re-walk the decl. Stored on the signature (see `FnOrChipSig`).
            let mut type_param_masks: Vec<(String, Vec<Type>)> = Vec::new();
            if has_type_params {
                ctx.push_scope();
                for tp in &c.type_params {
                    ctx.scope.declare(
                        &tp.name,
                        SymbolInfo {
                            kind: SymbolKind::Type,
                            name: tp.name.clone(),
                            ty: Type::Param(tp.name.clone()),
                            decl_range: tp.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                type_param_masks = c
                    .type_params
                    .iter()
                    .map(|tp| (tp.name.clone(), type_param_mask(ctx, tp)))
                    .collect();
            }
            let params: Vec<EventDataField> = c
                .inputs
                .iter()
                .map(|p| {
                    let ty = resolve_type_expr(ctx, &p.typ);
                    warn_any_annotation(ctx, &ty, type_expr_range(&p.typ));
                    EventDataField {
                        name: p.name.clone(),
                        ty,
                        is_const: p.is_const,
                    }
                })
                .collect();
            let outputs: Vec<EventDataField> = c
                .outputs
                .iter()
                .map(|o| {
                    let ty = resolve_type_expr(ctx, &o.typ);
                    warn_any_annotation(ctx, &ty, type_expr_range(&o.typ));
                    EventDataField {
                        name: o.name.clone(),
                        ty,
                        is_const: false,
                    }
                })
                .collect();
            if has_type_params {
                ctx.pop_scope();
            }
            // A `self`-mod whose name AND receiver type collide with a builtin
            // receiver-method (e.g. `mod Dot(self: vector, …)` vs the builtin
            // `Dot` on vector) would be silently shadowed at every call site —
            // the builtin always wins the `.method()` resolution — so reject it
            // at the declaration. Only flags an *overlapping* receiver type
            // (same coercion rule the completion uses): a same-named mod on an
            // unrelated receiver type is reachable and left alone.
            if c.is_self_receiver()
                && let Some(recv) = params.first().map(|p| p.ty.clone())
                && let Some(builtin_recv) = find_call(&c.name).and_then(|b| b.receiver.clone())
                && matches!(coerce(&recv, &builtin_recv), CoerceRule::Same | CoerceRule::Coerce)
            {
                ctx.emit(
                    "WS035",
                    format!(
                        "`{name}` shadows the builtin receiver-method `{name}` on `{recv}` — \
                         a call like `x.{name}(…)` always resolves to the builtin, so this \
                         mod could never be reached as a method (rename it)",
                        name = c.name,
                        recv = crate::analysis::types::type_str(&recv),
                    ),
                    c.inputs
                        .first()
                        .map(|p| p.range.clone())
                        .unwrap_or_else(|| c.range.clone()),
                );
            }
            // A `...rest` variadic capture is resolved at each call site by
            // inlining the body with the trailing args (see `lower/call/inline`).
            // A physical microchip (`chip`, not `mod`) instantiates once and can't
            // vary its pin count per call, so a variadic there would silently drop
            // the captured args — reject it at the declaration.
            if !c.inline && c.rest.is_some() {
                ctx.emit(
                    "WS052",
                    format!(
                        "`{}` is a chip, but only a `mod` may take a `...rest` variadic \
                         parameter (a chip instantiates once and cannot capture a \
                         per-call argument list) — make it a `mod`",
                        c.name
                    ),
                    c.range.clone(),
                );
            }
            declare_or_dup(
                ctx,
                &c.name,
                SymbolInfo {
                    kind: SymbolKind::Chip,
                    name: c.name.clone(),
                    ty: Type::Any,
                    decl_range: c.range.clone(),
                    signature: Some(FnOrChipSig {
                        params,
                        variadic: c.rest.is_some(),
                        outputs,
                        type_params: type_param_masks,
                    }),
                    event_data: None,
                },
            );
            // Backs `TypeCheckCtx::resolve_mod` — record into the INNERMOST
            // open frame (mirrors `lower::predeclare::pre_declare_chip_name`),
            // so a body-local `const mod` shadows a same-named outer one only
            // for the duration of its own scope. The stack is never empty
            // (the base frame holds the module's top-level declarations), so
            // this always lands somewhere — see `mod_decls`'s own doc comment.
            if let Some(frame) = ctx.mod_decls.last_mut() {
                frame.insert(c.name.clone(), Arc::new(c.clone()));
            }
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
            if t.type_params.is_empty() {
                let resolved = resolve_type_expr(ctx, &t.typ);
                warn_any_annotation(ctx, &resolved, type_expr_range(&t.typ));
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
            } else {
                // Generic alias: already collected into `ctx.generic_type_aliases`
                // by the `collect_generic_aliases` pre-pass (its body is
                // resolved lazily, per-use, by `resolve_type` — NOT here,
                // since it still references its own free `type_params`).
                // Still declare a placeholder `SymbolKind::Type` symbol so
                // `declare_or_dup` catches a duplicate name; a bare use of
                // this name (no `<Args>`) is caught by `resolve_type`'s
                // `generic_aliases` check before it would ever reach this
                // placeholder via the plain alias map.
                declare_or_dup(
                    ctx,
                    &t.name,
                    SymbolInfo {
                        kind: SymbolKind::Type,
                        name: t.name.clone(),
                        ty: Type::Any,
                        decl_range: t.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        TopDecl::Enum(e) => {
            // Discriminants were already assigned by the `collect_enum_defs`
            // pre-pass (before registration runs, alongside
            // `collect_generic_aliases`); this just declares the name as a
            // resolvable type, the same way `TopDecl::TypeAlias` does, so a
            // use (`E`, a variant match, ...) finds it and `declare_or_dup`
            // catches a duplicate name (WS013). A generic enum (`enum
            // Option<T> { ... }`) seeds the bare name here too -
            // instantiation against concrete type args is a later phase.
            declare_or_dup(
                ctx,
                &e.name,
                SymbolInfo {
                    kind: SymbolKind::Type,
                    name: e.name.clone(),
                    ty: Type::Enum {
                        name: e.name.clone(),
                        args: vec![],
                    },
                    decl_range: e.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Out(_)
        | TopDecl::Let(_)
        | TopDecl::LetElse(_)
        | TopDecl::Handler(_)
        | TopDecl::Assign(_)
        | TopDecl::If(_)
        | TopDecl::IfLet(_)
        | TopDecl::ExprStmt(_)
        | TopDecl::Import(_)
        | TopDecl::Await(_) => {
            // Resolved before typecheck. `LetElse`/`IfLet`'s pattern captures
            // are NOT registered here (same as a plain `Stmt::Let`/match arm
            // capture) - Task 19 gives them real pass-1 registration if
            // needed.
        }
        TopDecl::Namespace(ns) => {
            // An `import * as` alias is file-local, so a second namespace of the
            // same name (one module's own import traveling in beside the
            // importer's) is not a duplicate declaration. `namespaces_by_file`
            // is what keeps them apart; the flat scope entry is only a fallback.
            let shadows_namespace = matches!(
                ctx.scope.lookup(&ns.name),
                Some(s) if s.kind == SymbolKind::Namespace
            );
            let info = SymbolInfo {
                kind: SymbolKind::Namespace,
                name: ns.name.clone(),
                ty: Type::Any,
                decl_range: ns.range.clone(),
                signature: None,
                event_data: None,
            };
            if shadows_namespace {
                ctx.scope.declare(&ns.name, info);
            } else {
                declare_or_dup(ctx, &ns.name, info);
            }
            // The namespaces registered so far, including one that traveled in
            // with THIS module (resolve prepends it, so it is already
            // registered). Moved OUT rather than borrowed, so the member loop
            // below can read it while still using `ctx` mutably; restored, plus
            // this namespace's own entry, at the end of the arm.
            let member_file = ns.decls.first().map(|d| d.range().file.clone());
            let mut outer_ns = std::mem::take(&mut ctx.namespaces);
            // Overlay the aliases THIS module wrote for itself, so a member
            // initialized through one (`let v = Other.value`) resolves to that
            // module's `Other` and not the importer's same-named one.
            if let Some(file) = &member_file
                && let Some(own) = ctx.namespaces_by_file.get(file)
            {
                for (n, m) in own {
                    outer_ns.insert(n.clone(), m.clone());
                }
            }
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
                        // Mirror the chip param build (register_decl's
                        // `TopDecl::Chip` arm above) so a namespaced call can
                        // be arg-checked the same way a local call is. The
                        // `warn_any_annotation` call is skipped here — it
                        // already fires once when the member's own decl is
                        // checked (this loop only indexes it for lookup).
                        //
                        // A GENERIC member's param types reference its own
                        // type params (`mod maxT<T>(a: T, …)`), so — exactly
                        // like the normal registration above — push those type
                        // params into scope as `Type::Param` symbols BEFORE
                        // resolving, then pop. Without this a generic member's
                        // `T` hits `resolve_type_expr`'s unknown-type arm and
                        // wrongly emits WS002 at mere import time (the member
                        // need not even be called).
                        let has_type_params = !c.type_params.is_empty();
                        if has_type_params {
                            ctx.push_scope();
                            for tp in &c.type_params {
                                ctx.scope.declare(
                                    &tp.name,
                                    SymbolInfo {
                                        kind: SymbolKind::Type,
                                        name: tp.name.clone(),
                                        ty: Type::Param(tp.name.clone()),
                                        decl_range: tp.range.clone(),
                                        signature: None,
                                        event_data: None,
                                    },
                                );
                            }
                        }
                        let params: Vec<EventDataField> = c
                            .inputs
                            .iter()
                            .map(|p| EventDataField {
                                name: p.name.clone(),
                                ty: resolve_type_expr(ctx, &p.typ),
                                is_const: p.is_const,
                            })
                            .collect();
                        if has_type_params {
                            ctx.pop_scope();
                        }
                        ns_map.insert(
                            c.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Chip,
                                return_type,
                                params,
                                value_type: None,
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
                                is_const: p.is_const,
                            })
                            .collect();
                        ns_map.insert(
                            f.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Fn,
                                return_type: f.return_type.clone(),
                                params,
                                value_type: None,
                            },
                        );
                    }
                    // VALUE members. Lowering already puts these in the
                    // importing scope under their bare name (so an inlined
                    // namespaced mod body can reach them); index their type here
                    // so a qualified read (`ns.myValue`) types as the value
                    // instead of `any`. An annotation is authoritative;
                    // otherwise fall back to a literal initializer's type.
                    // Only a simple `let name = …` has a single qualified name;
                    // a destructuring `let` binds several and isn't reachable
                    // as one `ns.member`.
                    TopDecl::Let(l) => match &l.binding {
                        LetBinding::Ident { name, .. } => {
                            let ty = match &l.typ {
                                Some(te) => Some(resolve_type_expr(ctx, te)),
                                None => ns_value_type(&ns_map, &outer_ns, &l.value),
                            };
                            ns_map.insert(
                                name.clone(),
                                NsDeclInfo {
                                    kind: SymbolKind::LetBinding,
                                    return_type: None,
                                    params: Vec::new(),
                                    value_type: ty,
                                },
                            );
                        }
                        // A destructuring `let { a, b } = rect` binds several
                        // names, each reachable as its own `ns.a`. Project them
                        // off the initializer's record type so they carry the
                        // field's type rather than `any`.
                        LetBinding::Record { names, .. } => {
                            let src = ns_value_type(&ns_map, &outer_ns, &l.value);
                            for name in names {
                                let ty = ns_record_field(src.as_ref(), name);
                                ns_map.insert(name.clone(), ns_let_member(ty));
                            }
                        }
                        LetBinding::RecordDestruct { fields, .. } => {
                            let src = ns_value_type(&ns_map, &outer_ns, &l.value);
                            for f in fields {
                                // `{ a: x }` binds `x` to field `a`; a `...rest`
                                // binds a record of whatever is left, which this
                                // doesn't reconstruct — it stays `any`.
                                if let RecordDestructField::Named { name, alias, .. } = f {
                                    let ty = ns_record_field(src.as_ref(), name);
                                    let bound = alias.as_ref().unwrap_or(name);
                                    ns_map.insert(bound.clone(), ns_let_member(ty));
                                }
                            }
                        }
                        // Tuple destructuring binds a multi-output call's
                        // results, which have no single initializer type here.
                        LetBinding::Tuple { .. } => {}
                    },
                    TopDecl::Var(v) => {
                        let ty = match &v.typ {
                            Some(te) => Some(resolve_type_expr(ctx, te)),
                            None => v.init.as_ref().and_then(literal_expr_type),
                        };
                        ns_map.insert(
                            v.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Var,
                                return_type: None,
                                params: Vec::new(),
                                value_type: ty,
                            },
                        );
                    }
                    // Declare the module's type aliases under their qualified
                    // name so `let p: Ns.Point` resolves through the ordinary
                    // type lookup. The bare name stays private to the module.
                    TopDecl::TypeAlias(t) => {
                        let qualified = format!("{}.{}", ns.name, t.name);
                        if t.type_params.is_empty() {
                            let resolved = resolve_type_expr(ctx, &t.typ);
                            warn_any_annotation(ctx, &resolved, type_expr_range(&t.typ));
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
                        } else {
                            // Generic alias, already collected under its
                            // qualified name by `collect_generic_aliases`;
                            // see the top-level `TopDecl::TypeAlias` arm.
                            ctx.scope.declare(
                                &qualified,
                                SymbolInfo {
                                    kind: SymbolKind::Type,
                                    name: qualified.clone(),
                                    ty: Type::Any,
                                    decl_range: t.range.clone(),
                                    signature: None,
                                    event_data: None,
                                },
                            );
                        }
                    }
                    // Container members carry a value type so a namespaced
                    // read (`S.scores.get(k)`, `S.arr.pop()`) can type its
                    // element/value instead of falling through to `any`.
                    TopDecl::Array(a) => {
                        let inner = resolve_type_expr(ctx, &a.element_type);
                        ns_map.insert(
                            a.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Array,
                                return_type: None,
                                params: Vec::new(),
                                value_type: Some(Type::Array(Box::new(inner))),
                            },
                        );
                    }
                    TopDecl::Map(m) => {
                        let key = resolve_type_expr(ctx, &m.key_type);
                        let value = resolve_type_expr(ctx, &m.value_type);
                        ns_map.insert(
                            m.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Map,
                                return_type: None,
                                params: Vec::new(),
                                value_type: Some(Type::Map(Box::new(key), Box::new(value))),
                            },
                        );
                    }
                    // Ports read as VALUES through the namespace: `L.count`
                    // reads the output's value, `L.tick` the input's. An
                    // annotated port carries its declared type; an unannotated
                    // `out y = x` has none to resolve at registration and reads
                    // as `any`. Without these, a namespaced port read was WS002
                    // "not found in namespace" and lowered to `_Unsupported`.
                    TopDecl::Out(o) => {
                        let ty = o.typ.as_ref().map(|te| resolve_type_expr(ctx, te));
                        ns_map.insert(
                            o.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Out,
                                return_type: None,
                                params: Vec::new(),
                                value_type: ty,
                            },
                        );
                    }
                    TopDecl::In(i) => {
                        let ty = resolve_type_expr(ctx, &i.typ);
                        ns_map.insert(
                            i.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::In,
                                return_type: None,
                                params: Vec::new(),
                                value_type: Some(ty),
                            },
                        );
                    }
                    TopDecl::Buffer(b) => {
                        let ty = b.typ.as_ref().map(|te| resolve_type_expr(ctx, te));
                        ns_map.insert(
                            b.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Buffer,
                                return_type: None,
                                params: Vec::new(),
                                value_type: ty,
                            },
                        );
                    }
                    _ => {}
                }
            }
            ctx.namespaces = outer_ns;
            ctx.namespaces_by_file
                .entry(ns.range.file.clone())
                .or_default()
                .insert(ns.name.clone(), ns_map.clone());
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
