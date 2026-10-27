//! Pass 1: register every declaration's symbol before anything is checked.

use super::*;
use crate::analysis::types::type_str;

/// A namespace value member built from a destructuring `let`, with `ty` as its
/// value type, bound as `name` by the pattern at `range`.
fn ns_let_member(ty: Option<Type>, range: &SourceRange, name: &str) -> NsDeclInfo {
    NsDeclInfo {
        let_decl: Some(super::let_binding::let_key(range, name)),
        ..NsDeclInfo::value(SymbolKind::LetBinding, ty)
    }
}

/// Field `name` of `t`, when `t` is a known record that has it.
fn ns_record_field(t: Option<&Type>, name: &str) -> Option<Type> {
    match t? {
        Type::Record(fields) => fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, ft)| ft.clone()),
        _ => None,
    }
}

/// The type of a namespace value member's initializer, for members without an
/// annotation. `literal_expr_type` stops at primitives, so without this a
/// record-valued export (`let origin = { x: 0.0, y: 0.0 }`), one naming another
/// member, or one projecting a field off another reads as `any` from the
/// importer. All or nothing: a record types only when every field does. Members
/// resolve against `ns_map` as built so far, in source order (a forward
/// reference stays `any`); spread and duplicate keys follow `infer`'s
/// last-wins rule.
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
            let mut set = |name: &str, ty: Type| match rec.iter_mut().find(|(n, _)| n == name) {
                Some(existing) => existing.1 = ty,
                None => rec.push((name.to_string(), ty)),
            };
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        set(name, ns_value_type(ns_map, outer, value)?);
                    }
                    RecordLitField::Shorthand { name, .. } => set(name, member(name)?),
                    RecordLitField::Spread { value, .. } => {
                        match ns_value_type(ns_map, outer, value)? {
                            Type::Record(inner) => {
                                for (n, ty) in inner {
                                    set(&n, ty);
                                }
                            }
                            _ => return None,
                        }
                    }
                }
            }
            Some(Type::Record(rec))
        }
        _ => None,
    }
}

/// Fill `ctx.generic_type_aliases` from every `type Name<T, ...> = ...` in
/// `decls`, top-level and namespaced. The body is not resolved here: it names
/// its own type parameters, unbound until a use supplies arguments, so each use
/// instantiates it in `types::resolve::resolve_type`.
pub(super) fn collect_generic_aliases(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
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

pub(super) fn register_builtin_events(ctx: &mut TypeCheckCtx<'_>) {
    let evts = events();
    let mut keys: Vec<&&str> = evts.keys().collect();
    keys.sort();
    for k in keys {
        let spec = &evts[*k];
        let data = spec
            .data
            .iter()
            .map(|d| EventDataField {
                name: d.name.to_string(),
                ty: d.ty.clone(),
                is_const: false,
            })
            .collect();
        ctx.scope.declare(
            spec.surface_name,
            SymbolInfo::new(
                SymbolKind::Event,
                spec.surface_name,
                Type::Exec,
                SourceRange::default(),
            )
            .with_event_data(data),
        );
    }
}

/// Seed the `Option<T>`/`Result<T, E>` prelude (`enums::prelude_enum_defs`) and
/// the game enums (`enums::game_enum_defs`) into `ctx.enum_defs`, and declare
/// their type names as `SymbolKind::Type` symbols, as a user `TopDecl::Enum`
/// does, so they resolve as types with no declaration. Runs before
/// `collect_enum_defs`, so a user enum of the same name overwrites the seed (as
/// `enums::build_registry` merges) while `declare_or_dup` still reports the
/// redeclaration (WS013). Bare variant names (`Some`, `None`) are not scope
/// symbols; `infer.rs` resolves them through `ctx.enum_defs`
/// (`resolve_bare_variant_enum`).
pub(super) fn register_builtin_enums(ctx: &mut TypeCheckCtx<'_>) {
    for def in crate::typecheck::enums::prelude_enum_defs()
        .into_iter()
        .chain(crate::typecheck::enums::game_enum_defs())
    {
        ctx.scope.declare_new(
            SymbolKind::Type,
            &def.name,
            Type::Enum {
                name: def.name.clone(),
                args: vec![],
            },
            SourceRange::default(),
        );
        Arc::make_mut(&mut ctx.enum_defs).insert(def);
    }
}

// ---------- decl registration (1st pass) ----------

/// Whether this VALUE needs more than one wire. A tuple literal counts: the
/// parser desugars `(a, b)` to a `RecordLit` with numeric field names.
/// Lowering leaves such a port's own pin unwired.
fn value_needs_more_than_one_wire(e: &Expr) -> bool {
    matches!(e, Expr::RecordLit { .. })
}

/// Whether a port annotated with this type dissolves into one pin per field.
/// Mirrors `lower::predeclare::pre_declare_output`'s explode condition
/// (`LowerCtx::record_or_tuple_fields`): a record, a tuple, a ref to either,
/// or a ref to an enum.
fn port_dissolves_into_field_pins(t: &Type) -> bool {
    match t {
        Type::Record(_) | Type::Tuple(_) => true,
        Type::Ref(inner) => {
            matches!(**inner, Type::Enum { .. }) || port_dissolves_into_field_pins(inner)
        }
        _ => false,
    }
}

/// Register the port `name` under `out_scope_key`, or reconcile this site with
/// an already-registered port of that name.
///
/// The port's type is the first concrete one any site states: an unannotated
/// site registers `Type::Any`, which a later annotated site upgrades, so file
/// order does not decide it. Two concrete types conflict, and the first
/// governs, as in lowering; both passes must agree on the site set and on
/// which site governs. Lowering wires every site into one typed var with no
/// conversion, so a later site stating another type is WS003 at its
/// annotation. This compares declarations; a value against the port is
/// `check_port_write`'s job.
///
/// `always_live` says whether the site drives the port continuously (a
/// top-level or anonymous-chip `out`) rather than when its handler runs.
fn register_port_site(ctx: &mut TypeCheckCtx<'_>, o: &OutBinding, always_live: bool) {
    let name = o.name.as_str();
    let annotated = o.typ.as_ref();
    let decl_range = &o.range;
    // `resolve_type_expr` is NOT pure - it emits (e.g. WS002 "unknown type").
    // The canonical check for this site's own annotation runs later (the
    // `TopDecl::Out`/`Stmt::OutBinding` value check), so discard what this
    // registration pass produces.
    let resolved = ctx.discarding_diagnostics(|ctx| annotated.map(|te| resolve_type_expr(ctx, te)));

    // A port is readable only when it carries a SINGLE wire, so a record- or
    // tuple-shaped one is not registered: the annotation states that shape
    // when there is one, the value states it otherwise. Registering it would
    // let a bare read type-check and then lower to nothing. WS002 instead.
    if resolved
        .as_ref()
        .is_some_and(port_dissolves_into_field_pins)
        || o.value.as_ref().is_some_and(value_needs_more_than_one_wire)
    {
        return;
    }
    let key = out_scope_key(name);
    // Several sites may drive one port when they are exec-derived: only one fires
    // per tick, so the port carries one value at a time. Two always-live sites
    // both drive it continuously, so whenever they disagree the value read is
    // undefined (a pure `uptime % 2 > 1` disagrees on its own schedule), and the
    // emitted chip has two output bricks with the same `PortLabel`, one wired to
    // nothing.
    if always_live && o.value.is_some() && !ctx.out_ports_with_initializer.insert(key.clone()) {
        ctx.emit(
            "WS013",
            format!(
                "output port '{name}' already has a value at another always-live \
                 site, and both drive it continuously, so which value the port reads \
                 is undefined. Give it one value, or write it from a handler \
                 (`emit {name} = ...`) so only one site fires per tick"
            ),
            decl_range.clone(),
        );
    }
    if let Some(existing) = ctx.scope.lookup(&key) {
        let existing_ty = existing.ty.clone();
        // `Type::Any` is what an unannotated site registers, and it states
        // nothing. A later site carrying a real annotation supplies the port's
        // type rather than conflicting with the absence of one, so the port
        // ends up with the same type whichever order the two sites appear in.
        let Some(new_ty) = resolved.filter(|t| !matches!(t, Type::Any)) else {
            return;
        };
        if matches!(existing_ty, Type::Any) {
            ctx.scope.set_type(&key, new_ty);
        } else if new_ty != existing_ty {
            ctx.emit(
                "WS003",
                format!(
                    "'{name}' is already declared as an output port with type {}, \
                     so it cannot also be declared here as {}",
                    type_str(&existing_ty),
                    type_str(&new_ty),
                ),
                type_expr_range(annotated.expect("resolved implies annotated")),
            );
        }
        return;
    }
    #[cfg(test)]
    ctx.port_sites.push(name.to_string());
    ctx.scope.declare(
        &key,
        SymbolInfo::new(
            SymbolKind::Out,
            name,
            resolved.unwrap_or(Type::Any),
            decl_range.clone(),
        ),
    );
}

/// Register every `out` in a top-level handler body (or a captured event's
/// body) as a readable port, as `lower::predeclare::pre_declare_handler_outputs`
/// does, over `ast::handler_port_sites` (shared with that pass and
/// `scoped_refs`, so the three claim the same sites). It declares into the
/// caller's open frame, the file root for a top-level handler, so the port is
/// readable anywhere in the file.
fn register_handler_outputs(ctx: &mut TypeCheckCtx<'_>, block: &Block) {
    for o in crate::ast::handler_port_sites(block) {
        register_port_site(ctx, o, false);
    }
}

/// A declaration's parameters as `EventDataField`s, resolving each annotation
/// and warning on a bare `any` as it goes.
fn params_as_event_data(
    ctx: &mut TypeCheckCtx<'_>,
    params: &[crate::ast::Param],
) -> Vec<EventDataField> {
    params
        .iter()
        .map(|p| {
            let ty = resolve_annotation(ctx, &p.typ);
            EventDataField {
                name: p.name.clone(),
                ty,
                is_const: p.is_const,
            }
        })
        .collect()
}

/// The storage an anon-chip body statement declares in the scope it shares:
/// its name and declaration.
pub(super) fn anon_chip_storage_decl(s: &Stmt) -> Option<(&String, &SourceRange)> {
    match s {
        Stmt::Var(VarDecl { name, range, .. })
        | Stmt::Buffer(BufferDecl { name, range, .. })
        | Stmt::Array(ArrayDecl { name, range, .. }) => Some((name, range)),
        _ => None,
    }
}

/// Register what an anon-chip body contributes to the ENCLOSING scope. A chip
/// shares its parent's scope, so its `var`/`buffer`/`array`/`in` declarations
/// and its port sites name things there.
///
/// The other statement kinds are not skipped so much as not DECLARATIONS - the
/// ordinary statement walk still checks them.
pub(super) fn register_anon_chip_decls(ctx: &mut TypeCheckCtx<'_>, stmts: &[Stmt]) {
    for s in stmts {
        match s {
            Stmt::Var(v) => register_decl(ctx, &TopDecl::Var(v.clone())),
            Stmt::Buffer(b) => register_decl(ctx, &TopDecl::Buffer(b.clone())),
            Stmt::Array(a) => register_decl(ctx, &TopDecl::Array(a.clone())),
            Stmt::In(i) => register_decl(ctx, &TopDecl::In(i.clone())),
            _ => {}
        }
        if let Some((name, range)) = anon_chip_storage_decl(s)
            && ctx.scope.depth() == 1
            && let Some(sym) = ctx
                .scope
                .lookup(name)
                .filter(|sym| &sym.decl_range == range)
                .cloned()
        {
            ctx.anon_chip_storage.insert(range.start_key(), sym);
        }
    }
    register_anon_chip_outputs(ctx, stmts);
}

/// Register every plain `out` written directly in an anonymous chip body as a
/// module-boundary port, as `lower::predeclare::pre_declare_anon_chip` does,
/// over the shared `ast::anon_chip_port_sites`. An anonymous chip shares its
/// parent's scope, so such an `out` is a boundary port drawn inside a box, and
/// reads and writes of it resolve through the port registry.
pub(super) fn register_anon_chip_outputs(ctx: &mut TypeCheckCtx<'_>, stmts: &[Stmt]) {
    for o in crate::ast::anon_chip_port_sites(stmts) {
        register_port_site(ctx, o, true);
    }
}

pub(super) fn register_decl(ctx: &mut TypeCheckCtx<'_>, d: &TopDecl) {
    match d {
        TopDecl::Var(v) => {
            let inner = v
                .typ
                .as_ref()
                .map(|t| resolve_type_expr(ctx, t))
                // No annotation: infer an array type from a `[..]` initializer, so
                // `var foo = [1, 2]` indexes and iterates as an array, and a scalar type from a
                // literal (`var foo = ""` is a string var, `var n = 0` an int var). Literal
                // elements give their join (`[1, 2.5]` is `float[]`).
                .or_else(|| match &v.init {
                    Some(Expr::Array { elements, .. }) => {
                        let literal: Option<Vec<Type>> = elements
                            .iter()
                            .map(|el| match el {
                                ArrayElem::Item(e) => literal_expr_type(e),
                                ArrayElem::Spread(_) => None,
                            })
                            .collect();
                        let elem = literal.and_then(widening_join_all).unwrap_or(Type::Any);
                        Some(Type::Array(Box::new(elem)))
                    }
                    // An empty `{}` is a map (WS098 asks for its types).
                    Some(init)
                        if map_init_entries(init)
                            .is_some_and(<[crate::ast::MapLitEntry]>::is_empty) =>
                    {
                        Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any)))
                    }
                    Some(init) => literal_expr_type(init),
                    None => None,
                })
                .unwrap_or(Type::Any);
            if let Some(t) = &v.typ {
                // A `var` is scalar, array (`var x: T[]`) or map (`var m: Map<K,V>`) storage,
                // each needing a concrete wire type, so `any` is rejected in the stored
                // position (the element of an array, the value of a map, else the type), as
                // the `array` and `map` declarations below check.
                let (stored, what) = match &inner {
                    Type::Array(elem) => (elem.as_ref(), "an array's element type"),
                    Type::Map(_, val) => (val.as_ref(), "a map's value type"),
                    other => (other, "a variable gate"),
                };
                reject_any_storage(ctx, stored, type_expr_range(t), what);
                if let Type::Map(key, _) = &inner {
                    reject_ref_key(ctx, key, type_expr_range(t));
                }
            }
            declare_or_dup(
                ctx,
                &v.name,
                SymbolInfo::new(
                    SymbolKind::Var,
                    &v.name,
                    Type::Ref(Box::new(inner)),
                    v.range.clone(),
                ),
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
                SymbolInfo::new(
                    SymbolKind::Array,
                    &a.name,
                    Type::Array(Box::new(inner)),
                    a.range.clone(),
                ),
            );
        }
        TopDecl::Map(m) => {
            let key = resolve_type_expr(ctx, &m.key_type);
            let value = resolve_type_expr(ctx, &m.value_type);
            reject_ref_key(ctx, &key, type_expr_range(&m.key_type));
            reject_any_storage(
                ctx,
                &value,
                type_expr_range(&m.value_type),
                "a map's value type",
            );
            declare_or_dup(
                ctx,
                &m.name,
                SymbolInfo::new(
                    SymbolKind::Map,
                    &m.name,
                    Type::Map(Box::new(key), Box::new(value)),
                    m.range.clone(),
                ),
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
                SymbolInfo::new(SymbolKind::Buffer, &b.name, placeholder, b.range.clone()),
            );
        }
        TopDecl::In(d) => {
            // A port cannot take a type parameter, so `any` here is no WS032.
            let t = resolve_type_expr(ctx, &d.typ);
            declare_or_dup(
                ctx,
                &d.name,
                SymbolInfo::new(SymbolKind::In, &d.name, t, d.range.clone()),
            );
        }
        TopDecl::Chip(c) => {
            // Two outputs of one name: a read of the call could take only one.
            let mut seen: HashSet<&str> = HashSet::default();
            for o in &c.outputs {
                if !seen.insert(o.name.as_str()) {
                    ctx.emit(
                        "WS013",
                        format!(
                            "output '{}' is declared twice in `{}`'s outputs, so a read of the call would take \
                             only one of them. Rename one",
                            o.name, c.name
                        ),
                        o.range.clone(),
                    );
                }
            }
            // Generic chips (physical microchips, `inline == false`) are monomorphized
            // per instantiation at lowering (`lower_chip_call_instance` keys a template on
            // `(name, concrete subst)`), so `Box<int>` and `Box<vector>` never share a
            // body; generic `mod`s inline per call site.
            // Each type parameter becomes a scope `Type` symbol resolving to
            // `Type::Param(name)` while this signature resolves, popped before
            // `declare_or_dup` so it does not leak to sibling declarations.
            let has_type_params = !c.type_params.is_empty();
            // Name + mask for each type param, computed once at registration
            // (mirrors the scope-declare loop just below) so the call-typing
            // path can later solve `Constraint`s against it without having to
            // re-walk the decl. Stored on the signature (see `FnOrChipSig`).
            let mut type_param_masks: Vec<(String, Vec<Type>)> = Vec::new();
            if has_type_params {
                ctx.push_scope();
                for tp in &c.type_params {
                    ctx.scope.declare_new(
                        SymbolKind::Type,
                        &tp.name,
                        Type::Param(tp.name.clone()),
                        tp.range.clone(),
                    );
                }
                type_param_masks = c
                    .type_params
                    .iter()
                    .map(|tp| (tp.name.clone(), type_param_mask(ctx, tp)))
                    .collect();
            }
            let params = params_as_event_data(ctx, &c.inputs);
            let outputs: Vec<EventDataField> = c
                .outputs
                .iter()
                .map(|o| {
                    let ty = resolve_annotation(ctx, &o.typ);
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
            // A self-mod whose name and receiver type collide with a builtin receiver
            // method (`mod Dot(self: vector, ...)` against the builtin `Dot`) would always
            // lose `.method()` resolution, so it is rejected at the declaration. Only an
            // overlapping receiver type counts (the coercion rule completion uses); on an
            // unrelated type the mod is reachable.
            if c.is_self_receiver()
                && let Some(recv) = params.first().map(|p| p.ty.clone())
                && let Some(builtin_recv) = find_call(&c.name).and_then(|b| b.receiver.clone())
                && matches!(
                    coerce(&recv, &builtin_recv),
                    CoerceRule::Same | CoerceRule::Coerce
                )
            {
                ctx.emit(
                    "WS035",
                    format!(
                        "`{name}` shadows the builtin receiver-method `{name}` on `{recv}`: \
                         a call like `x.{name}(…)` always resolves to the builtin, so this \
                         mod could never be reached as a method (rename it)",
                        name = c.name,
                        recv = type_str(&recv),
                    ),
                    c.inputs
                        .first()
                        .map(|p| p.range.clone())
                        .unwrap_or_else(|| c.range.clone()),
                );
            }
            // A `...rest` capture is resolved per call site by inlining the body with the
            // trailing arguments (`lower/call/inline`). A `chip` instantiates once and
            // cannot vary its pin count per call, so a variadic chip is rejected.
            if !c.inline && c.rest.is_some() {
                ctx.emit(
                    "WS052",
                    format!(
                        "`{}` is a chip, but only a `mod` may take a `...rest` variadic \
                         parameter (a chip instantiates once and cannot capture a \
                         per-call argument list), so make it a `mod`",
                        c.name
                    ),
                    c.range.clone(),
                );
            }
            declare_or_dup(
                ctx,
                &c.name,
                SymbolInfo::new(SymbolKind::Chip, &c.name, Type::Any, c.range.clone())
                    .with_signature(FnOrChipSig {
                        required: c.inputs.iter().take_while(|p| p.default.is_none()).count(),
                        params,
                        variadic: c.rest.is_some(),
                        outputs,
                        type_params: type_param_masks,
                        is_fn: c.is_fn,
                    }),
            );
            // For `TypeCheckCtx::resolve_mod`: record into the innermost open frame (as
            // `lower::predeclare::pre_declare_chip_name` does), so a body-local
            // `const mod` shadows an outer one only inside its scope. The base frame is
            // always there (see `mod_decls`).
            if let Some(frame) = ctx.mod_decls.last_mut() {
                frame.insert(c.name.clone(), Arc::new(c.clone()));
            }
        }
        TopDecl::Event(e) => {
            declare_or_dup(
                ctx,
                &e.name,
                SymbolInfo::new(SymbolKind::Event, &e.name, Type::Exec, e.range.clone()),
            );
            // `let e = on Clock(...) { ... }` (a captured event) carries a
            // handler body here and is lowered handler-style by
            // `lower_event_decl`, so its `out` bindings are top-level handler
            // bindings too - see `register_handler_outputs`. Lowering's pass
            // 1a (`lower/mod.rs`) walks this same field for the same reason.
            if let Some(body) = e.captured_body() {
                register_handler_outputs(ctx, body);
            }
        }
        TopDecl::AnonChip(ac) => register_anon_chip_decls(ctx, &ac.body.stmts),
        TopDecl::TypeAlias(t) => {
            reject_builtin_type_name(ctx, "type", &t.name, t.type_params.len(), &t.range);
            if t.type_params.is_empty() {
                let resolved = resolve_annotation(ctx, &t.typ);
                declare_or_dup(
                    ctx,
                    &t.name,
                    SymbolInfo::new(SymbolKind::Type, &t.name, resolved, t.range.clone()),
                );
            } else {
                // A generic alias is already in `ctx.generic_type_aliases`
                // (`collect_generic_aliases`), its body resolved per use by `resolve_type`. A
                // placeholder `SymbolKind::Type` symbol is still declared, so `declare_or_dup`
                // catches a duplicate name; a bare use without `<Args>` is caught by
                // `resolve_type`'s `generic_aliases` check first.
                declare_or_dup(
                    ctx,
                    &t.name,
                    SymbolInfo::new(SymbolKind::Type, &t.name, Type::Any, t.range.clone()),
                );
            }
        }
        TopDecl::Enum(e) => {
            reject_builtin_type_name(ctx, "enum", &e.name, e.type_params.len(), &e.range);
            for v in &e.variants {
                ctx.reserve_check(&v.name, &v.range);
                if let crate::ast::EnumPayloadDecl::Named(fields) = &v.payload {
                    for (name, _, range) in fields {
                        ctx.reserve_check(name, range);
                    }
                }
            }
            // Discriminants are already assigned (`collect_enum_defs`); this declares the
            // name as a resolvable type, as `TopDecl::TypeAlias` does, so a use finds it
            // and `declare_or_dup` catches a duplicate (WS013). A generic enum
            // (`enum Option<T> { ... }`) declares its bare name too.
            declare_or_dup(
                ctx,
                &e.name,
                SymbolInfo::new(
                    SymbolKind::Type,
                    &e.name,
                    Type::Enum {
                        name: e.name.clone(),
                        args: vec![],
                    },
                    e.range.clone(),
                ),
            );
        }
        // A port is a readable value but not in the ordinary name tier: in
        // `var count` with `out count = count`, the right side means the var. The
        // `out:` prefix keys ports as lowering's `output_scope_key` does, so the two
        // passes agree on which sites are ports, and chip and mod bodies scope them
        // through the existing scope push and pop. It goes through
        // `register_port_site`, so a top-level and a handler `out` of one name follow
        // the same first-wins and conflict rule.
        TopDecl::Out(o) => {
            register_port_site(ctx, o, true);
        }
        // A handler-body `out` is a module-boundary port, as
        // `pre_declare_handler_outputs` hoists it for lowering, so it is registered
        // here too (else a read of it is WS002). It is declared from this dispatch
        // with no scope pushed for the handler body, so it lands in the open frame,
        // the file root for a top-level handler, as `TopDecl::Out` does.
        TopDecl::Handler(h) => register_handler_outputs(ctx, &h.body),
        TopDecl::Let(_)
        | TopDecl::LetElse(_)
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
            let info =
                SymbolInfo::new(SymbolKind::Namespace, &ns.name, Type::Any, ns.range.clone());
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
            let member_file = ns
                .decls
                .first()
                .map(|d| std::sync::Arc::clone(&d.range().file));
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
                        // Build the parameters as `register_decl`'s `TopDecl::Chip` arm does, so a
                        // namespaced call is checked like a local one (`warn_any_annotation` already
                        // fired when the member's own declaration was checked). A generic member's
                        // parameter types name its type parameters (`mod maxT<T>(a: T, ...)`), so
                        // those are pushed as `Type::Param` symbols while resolving, else `T` would be
                        // WS002 at import time.
                        let has_type_params = !c.type_params.is_empty();
                        if has_type_params {
                            ctx.push_scope();
                            for tp in &c.type_params {
                                ctx.scope.declare_new(
                                    SymbolKind::Type,
                                    &tp.name,
                                    Type::Param(tp.name.clone()),
                                    tp.range.clone(),
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
                        let signature = Some({
                            let sig = FnOrChipSig {
                                required: c
                                    .inputs
                                    .iter()
                                    .take_while(|p| p.default.is_none())
                                    .count(),
                                params: params.clone(),
                                variadic: c.rest.is_some(),
                                outputs: c
                                    .outputs
                                    .iter()
                                    .map(|o| EventDataField {
                                        name: o.name.clone(),
                                        ty: resolve_type_expr(ctx, &o.typ),
                                        is_const: false,
                                    })
                                    .collect(),
                                type_params: c
                                    .type_params
                                    .iter()
                                    .map(|tp| {
                                        (tp.name.clone(), super::resolve::type_param_mask(ctx, tp))
                                    })
                                    .collect(),
                                is_fn: c.is_fn,
                            };
                            (sig, c.range.clone())
                        });
                        if has_type_params {
                            ctx.pop_scope();
                        }
                        ns_map.insert(
                            c.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Chip,
                                return_type,
                                outputs: c
                                    .outputs
                                    .iter()
                                    .map(|o| (o.name.clone(), o.typ.clone()))
                                    .collect(),
                                params,
                                value_type: None,
                                let_decl: None,
                                signature,
                            },
                        );
                    }
                    // Value members: lowering puts them in the importing scope under their bare
                    // name (so an inlined namespaced body reaches them); their type is indexed
                    // here so `ns.myValue` types as the value. An annotation decides, else a
                    // literal initializer's type. Only a plain `let name = ...` has one qualified
                    // name; a destructuring `let` binds several.
                    TopDecl::Let(l) => match &l.binding {
                        LetBinding::Ident { name, range } => {
                            let ty = match &l.typ {
                                Some(te) => Some(resolve_type_expr(ctx, te)),
                                None => ns_value_type(&ns_map, &outer_ns, &l.value),
                            };
                            ns_map.insert(name.clone(), ns_let_member(ty, range, name));
                        }
                        // A destructuring `let { a, b } = rect` binds several
                        // names, each reachable as its own `ns.a`. Project them
                        // off the initializer's record type so they carry the
                        // field's type rather than `any`.
                        LetBinding::RecordDestruct { fields, range } => {
                            let src = ns_value_type(&ns_map, &outer_ns, &l.value);
                            for f in fields {
                                // `{ a: x }` binds `x` to field `a`; a `...rest` binds a record of what is
                                // left, which is not reconstructed here and stays `any`.
                                if let RecordDestructField::Named { name, alias, .. } = f {
                                    let ty = ns_record_field(src.as_ref(), name);
                                    let bound = alias.as_ref().unwrap_or(name);
                                    ns_map.insert(bound.clone(), ns_let_member(ty, range, bound));
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
                        ns_map.insert(v.name.clone(), NsDeclInfo::value(SymbolKind::Var, ty));
                    }
                    // Declare the module's type aliases under their qualified
                    // name so `let p: Ns.Point` resolves through the ordinary
                    // type lookup. The bare name stays private to the module.
                    TopDecl::TypeAlias(t) => {
                        let qualified = format!("{}.{}", ns.name, t.name);
                        if t.type_params.is_empty() {
                            let resolved = resolve_annotation(ctx, &t.typ);
                            ctx.scope.declare_new(
                                SymbolKind::Type,
                                &qualified,
                                resolved,
                                t.range.clone(),
                            );
                        } else {
                            // Generic alias, already collected under its
                            // qualified name by `collect_generic_aliases`;
                            // see the top-level `TopDecl::TypeAlias` arm.
                            ctx.scope.declare_new(
                                SymbolKind::Type,
                                &qualified,
                                Type::Any,
                                t.range.clone(),
                            );
                        }
                    }
                    // The module's enum, registered under its own name (see
                    // `enums::collect_enum_defs`), is the type `Ns.Name`.
                    TopDecl::Enum(e) => {
                        let qualified = format!("{}.{}", ns.name, e.name);
                        ctx.scope.declare_new(
                            SymbolKind::Type,
                            &qualified,
                            Type::Enum {
                                name: e.name.clone(),
                                args: vec![],
                            },
                            e.range.clone(),
                        );
                    }
                    // Container members carry a value type so a namespaced
                    // read (`S.scores.get(k)`, `S.arr.pop()`) can type its
                    // element/value instead of falling through to `any`.
                    TopDecl::Array(a) => {
                        let inner = resolve_type_expr(ctx, &a.element_type);
                        ns_map.insert(
                            a.name.clone(),
                            NsDeclInfo::value(
                                SymbolKind::Array,
                                Some(Type::Array(Box::new(inner))),
                            ),
                        );
                    }
                    TopDecl::Map(m) => {
                        let key = resolve_type_expr(ctx, &m.key_type);
                        let value = resolve_type_expr(ctx, &m.value_type);
                        ns_map.insert(
                            m.name.clone(),
                            NsDeclInfo::value(
                                SymbolKind::Map,
                                Some(Type::Map(Box::new(key), Box::new(value))),
                            ),
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
                        ns_map.insert(o.name.clone(), NsDeclInfo::value(SymbolKind::Out, ty));
                    }
                    TopDecl::In(i) => {
                        let ty = resolve_type_expr(ctx, &i.typ);
                        ns_map.insert(i.name.clone(), NsDeclInfo::value(SymbolKind::In, Some(ty)));
                    }
                    TopDecl::Buffer(b) => {
                        let ty = b.typ.as_ref().map(|te| resolve_type_expr(ctx, te));
                        ns_map.insert(b.name.clone(), NsDeclInfo::value(SymbolKind::Buffer, ty));
                    }
                    _ => {}
                }
            }
            // A member module's top-level handler may declare a boundary port
            // (`on trig { out hport: int = 7 }`), hoisted by lowering's namespace arm as an
            // `out` member is, so `L.hport` reads it as `L.count` reads an `out` member.
            // After the loop, so an explicit `out` member of the same name governs, as in
            // `pre_declare_handler_outputs`.
            for d in &ns.decls {
                let TopDecl::Handler(h) = d else { continue };
                for o in crate::ast::handler_port_sites(&h.body) {
                    let ty = o.typ.as_ref().map(|te| resolve_type_expr(ctx, te));
                    ns_map
                        .entry(o.name.clone())
                        .or_insert(NsDeclInfo::value(SymbolKind::Out, ty));
                }
            }
            ctx.namespaces = outer_ns;
            ctx.namespaces_by_file
                .entry(std::sync::Arc::clone(&ns.range.file))
                .or_default()
                .insert(ns.name.clone(), ns_map.clone());
            ctx.namespaces.insert(ns.name.clone(), ns_map);
        }
    }
}

/// `WS107` for a `type` or `enum` declared under the name of a builtin type
/// (`float`, `string`, `any`, ...), or as `Array` or `Ref` with one type
/// parameter (`Array<T>` and `Ref<T>` are how `T[]` and `*T` are spelled):
/// the builtin is what that name means wherever a type is written, so the
/// declaration would never be read. Any other builtin generic name (`Map`) is
/// the user declaration's wherever the program declares one.
fn reject_builtin_type_name(
    ctx: &mut TypeCheckCtx<'_>,
    what: &str,
    name: &str,
    type_params: usize,
    range: &SourceRange,
) {
    let spelled = matches!(name, "Array" | "Ref") && type_params == 1;
    if crate::types::resolve::primitive(name).is_none() && !spelled {
        return;
    }
    if ctx
        .diagnostics
        .iter()
        .any(|d| d.code == "WS107" && d.range == *range)
    {
        return;
    }
    let message = if spelled {
        let builtin = if name == "Array" { "T[]" } else { "*T" };
        format!(
            "`{what} {name}<..>` is named like the builtin `{name}<T>`, which is how `{builtin}` is \
             written wherever a type is written, so this declaration would never be used. Give it another name"
        )
    } else {
        format!(
            "`{what} {name}` is named like the builtin type `{name}`, which is what `{name}` means \
             wherever a type is written, so this declaration would never be used. Give it another name"
        )
    };
    ctx.emit("WS107", message, range.clone());
}

fn declare_or_dup(ctx: &mut TypeCheckCtx<'_>, name: &str, info: SymbolInfo) {
    let range = info.decl_range.clone();
    if ctx.scope.declare(name, info).is_some() {
        ctx.duplicate_decls.insert(name.to_string());
        ctx.emit("WS013", format!("duplicate declaration of '{name}'"), range);
    }
}
