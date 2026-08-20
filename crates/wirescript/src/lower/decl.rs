use super::*;

// ---------- declaration body pass ----------

/// True if `p`'s declared output type is `string`, read straight off the IR
/// node's port spec (no typecheck-map dependency). A label value that is already
/// a string wires into the label's `Text` port directly; only a non-string is
/// routed through a `FormatText` gate to coerce it.
fn label_source_is_string(ctx: &LowerCtx, p: PortRef) -> bool {
    ctx.builder
        .module
        .nodes
        .get(&p.node_id)
        .and_then(|n| {
            n.ports
                .outputs
                .iter()
                .find(|s| s.name == crate::intern::intern(p.port.as_str()))
        })
        .map(|s| s.ty == Type::String)
        .unwrap_or(false)
}

/// Lower a label expression to a string-typed value port: the value directly if
/// it is already a string (e.g. a string var, or an interpolation that already
/// built its own `FormatText`), otherwise coerced through a single `FormatText`.
fn lower_label_value(ctx: &mut LowerCtx, le: &Expr) -> PortRef {
    let value = lower_expr(ctx, le);
    if label_source_is_string(ctx, value) {
        value
    } else {
        build_format_text(ctx, "{0}".to_string(), vec![value], le.range())
    }
}

/// Resolve runtime `@label(expr)` on top-level `var`s into dynamic labels.
///
/// For each top-level var whose label doesn't fold to a constant (a constant
/// baked its text statically in pass 1), lower the expression to a value port,
/// string-coerce it if needed, and record `host_var → source_port` on the
/// module. Emit turns each entry into a wire from that port into the var's
/// label `Component_TextDisplay.Text` (see `emit.rs` Pass 3.5).
///
/// Runs after the body pass so any var/gate the label references already exists,
/// and in a *pure* context (`current_exec` cleared) so a var read resolves to
/// the var's live `Value` output rather than an exec-chained `Var_Get`.
pub(super) fn resolve_dynamic_var_labels(ctx: &mut LowerCtx, decls: &[TopDecl]) {
    let saved_exec = ctx.current_exec.take();
    for d in decls {
        let TopDecl::Var(v) = d else { continue };
        let Some(le) = &v.label_expr else { continue };
        if expr_to_literal_in(le, &ctx.const_env).is_some() {
            continue; // constant label — already baked as static text.
        }
        // The host var node carries the label component at emit time.
        let host = match ctx.scope.get(&v.name) {
            Some(Binding::Var(rec)) => rec.node_id,
            _ => continue,
        };
        let src = lower_label_value(ctx, le);
        ctx.builder.module.dynamic_labels.insert(host, src);
    }
    ctx.current_exec = saved_exec;
}

/// Resolve a module-level `@label(<expr>)` into the root microchip's label.
///
/// A constant expression folds to static title text (`root_label_override`);
/// a runtime expression is lowered (string-coerced) and recorded as
/// `root_dynamic_label`, which emit wires into the root shell's `Text` port.
/// Runs in the same post-declaration pass as [`resolve_dynamic_var_labels`], so
/// the expression may forward-reference declarations below it (hoisting).
pub(super) fn resolve_module_label(ctx: &mut LowerCtx, module_label: Option<&Expr>) {
    let Some(le) = module_label else { return };
    if let Some(text) = resolve_label_text(None, Some(le), &ctx.const_env) {
        // Constant — baked static title text.
        ctx.builder.module.root_label_override = Some(text);
        return;
    }
    // Runtime value — lower in a pure context and coerce to string only if it
    // isn't already one. Emit wires this into the root shell's `Text`.
    let saved_exec = ctx.current_exec.take();
    let src = lower_label_value(ctx, le);
    ctx.builder.module.root_dynamic_label = Some(src);
    ctx.current_exec = saved_exec;
}

pub(super) fn lower_decl(ctx: &mut LowerCtx, d: &TopDecl) {
    match d {
        TopDecl::Out(b) => ctx.with_nofold(b.no_fold, |ctx| {
            lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range)
        }),
        TopDecl::Handler(h) => ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h)),
        TopDecl::Event(e) => ctx.with_nofold(e.no_fold, |ctx| lower_event_decl(ctx, e)),
        TopDecl::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
        TopDecl::Buffer(b) => lower_buffer_body(ctx, b),
        // Gate created in pre-pass; top level is pure, so a non-constant init
        // has no exec reset to apply it — surface the drop.
        TopDecl::Var(v) => ctx.with_nofold(v.no_fold, |ctx| warn_unbaked_var_init(ctx, v, true)),
        TopDecl::Array(_) | TopDecl::Map(_) | TopDecl::In(_) => {} // handled in pre-pass
        TopDecl::Chip(c) => lower_chip_decl(ctx, c),
        TopDecl::AnonChip(ac) => lower_anon_chip(ctx, ac),
        TopDecl::Assign(a) => lower_assign(ctx, a),
        TopDecl::If(i) => lower_if(ctx, i),
        TopDecl::ExprStmt(es) => {
            lower_expr(ctx, &es.expr);
        }
        TopDecl::Fn(f) => {
            // `fn` has been removed (rejected at parse with a hard error). A
            // recovered `fn` decl is still lowered as an inline mod-with-return so
            // a stray one doesn't crash lowering — there is no deprecation warning.
            // Synthesize a ChipDecl from the FnDecl
            let outputs = if let Some(ref ret_type) = f.return_type {
                vec![NamedOutput {
                    name: "_".into(),
                    typ: ret_type.clone(),
                    range: f.range.clone(),
                }]
            } else {
                Vec::new()
            };
            let chip = ChipDecl {
                name: f.name.clone(),
                type_params: Vec::new(),
                inputs: f.params.clone(),
                outputs,
                body: Block {
                    stmts: vec![Stmt::Return {
                        value: Some(f.body.clone()),
                        range: f.range.clone(),
                    }],
                    range: f.range.clone(),
                },
                range: f.range.clone(),
                inline: true,
                label: None,
                label_expr: None,
                closed: false,
                no_fold: false,
                is_const: false,
            };
            lower_chip_decl(ctx, &chip);
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) => {}
        TopDecl::Namespace(ns) => {
            let mut ns_decls: HashMap<String, Binding> = HashMap::default();
            let mut ns_buffers = Vec::new();
            let mut ns_outputs = Vec::new();
            // Value members whose lowered binding we capture into this
            // namespace's map AFTER the loop (so `A.foo` reads A's own `foo`
            // even when a later `import * as B` overwrites the shared bare
            // `foo`). Bare-name value members are only ever `let name`/`array
            // name`/… idents, so a plain name is all we need here.
            let mut ns_value_names: Vec<String> = Vec::new();
            for d in &ns.decls {
                match d {
                    TopDecl::Chip(c) => {
                        ns_decls
                            .insert(c.name.clone(), Binding::Chip(std::sync::Arc::new(c.clone())));
                        // A namespaced mod's body also calls its SIBLING mods by
                        // bare name (`drawCardBg(...)`, not `card.drawCardBg`);
                        // register them so those calls resolve when the body is
                        // inlined at a call site in the importing module. (The
                        // namespaced form stays available via the Namespace
                        // binding below.)
                        if ctx.scope.get(&c.name).is_none() {
                            ctx.scope
                                .insert(&c.name, Binding::Chip(std::sync::Arc::new(c.clone())));
                        }
                    }
                    // A namespaced (`import * as ns`) mod's body references its
                    // OWN module's `let` constants / `array` / `var` by bare
                    // name. Those mods are inlined at call sites in the importing
                    // module, where the members aren't otherwise in scope — so
                    // lower them here, into the enclosing scope, or every such
                    // reference drops to an `_Unsupported` placeholder that reads
                    // 0 at runtime. (Constant `array` initializers bake straight
                    // into the ArrayVar node during pre-declaration.)
                    //
                    // The bare-name insertion is shared across every namespace,
                    // so a name two modules both export collides there. Each
                    // member's binding is ALSO captured per-namespace below, and
                    // `A.member` resolves through that map, so explicit
                    // namespaced access stays correct regardless of the clash.
                    // An imported `let name = …` must not clobber a bare name
                    // the IMPORTER itself owns (its own `in`/`out`/`var`/…): an
                    // imported `let start` used to overwrite a local
                    // `in start: exec`, so `on start` then found a
                    // `Binding::Record`/`Local` instead of the Input and
                    // silently dropped the whole handler body. Unlike the
                    // `is_none()` guard on the sibling kinds below, this checks
                    // `importer_names` (not "any prior binding"), so a member
                    // shadowed only by an EARLIER `import * as` still lowers —
                    // two namespaces exporting the same `let` name each keep
                    // their own value (`A.foo` / `B.foo`).
                    TopDecl::Let(l) => {
                        let importer_owned = matches!(&l.binding,
                            crate::ast::LetBinding::Ident { name, .. }
                                if ctx.importer_names.contains(name));
                        if !importer_owned {
                            ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l));
                            if let crate::ast::LetBinding::Ident { name, .. } = &l.binding {
                                ns_value_names.push(name.clone());
                            }
                        }
                    }
                    TopDecl::Array(a) if ctx.scope.get(&a.name).is_none() => {
                        pre_declare_array(ctx, a);
                        ns_value_names.push(a.name.clone());
                    }
                    TopDecl::Map(m) if ctx.scope.get(&m.name).is_none() => {
                        pre_declare_map(ctx, m);
                        ns_value_names.push(m.name.clone());
                    }
                    TopDecl::Var(v) if ctx.scope.get(&v.name).is_none() => {
                        ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
                        // Module-level = pure: a non-constant init is dropped.
                        ctx.with_nofold(v.no_fold, |ctx| warn_unbaked_var_init(ctx, v, true));
                        ns_value_names.push(v.name.clone());
                    }
                    TopDecl::Buffer(b) if ctx.scope.get(&b.name).is_none() => {
                        pre_declare_buffer(ctx, b);
                        ns_buffers.push(b);
                        ns_value_names.push(b.name.clone());
                    }
                    // An imported module's `in`/`out` PORTS. Without these the
                    // declarations were dropped entirely: `on ns.trigger { … }`
                    // type-checked clean and lowered to nothing at all, taking
                    // the whole handler with it. They become ports of the
                    // importing module's chip, exactly as a local `in`/`out`
                    // does, and are reachable both bare and as `ns.name`.
                    TopDecl::In(i) if ctx.scope.get(&i.name).is_none() => {
                        pre_declare_input(ctx, i);
                        ns_value_names.push(i.name.clone());
                    }
                    TopDecl::Out(o) if ctx.scope.get(&o.name).is_none() => {
                        ctx.with_nofold(o.no_fold, |ctx| {
                            pre_declare_output(
                                ctx,
                                &o.name,
                                o.value.as_ref(),
                                o.typ.as_ref(),
                                o.side,
                                o.label.as_deref(),
                                o.label_expr.as_ref(),
                                o.invisible,
                                &o.range,
                            )
                        });
                        ns_outputs.push(o);
                        ns_value_names.push(o.name.clone());
                    }
                    _ => {}
                }
            }
            // Wire buffer initializers only after every ns member is in scope
            // (an init may reference a member declared after it). Only buffers
            // pre-declared above — a name the importer already owns stays its.
            for b in ns_buffers {
                lower_buffer_body(ctx, b);
            }
            // Same ordering rule as buffers: an `out x = <expr>` initializer may
            // name a member declared after it, so wire the values only once the
            // whole module is in scope.
            for o in ns_outputs {
                ctx.with_nofold(o.no_fold, |ctx| {
                    lower_out_binding(ctx, &o.name, o.value.as_ref(), &o.range)
                });
            }
            // Capture each value member's binding into this namespace's map,
            // NOW — before the next `import * as` lowers its own members and
            // overwrites the shared bare names. A member the importer already
            // owned (the `is_none()`-guarded kinds that were skipped) is not in
            // `ns_value_names`, so it is not captured here.
            for name in ns_value_names {
                if let Some(binding) = ctx.scope.get(&name).cloned() {
                    ns_decls.insert(name, binding);
                }
            }
            ctx.scope
                .insert(&ns.name, Binding::Namespace(ns_decls));
        }
    }
}

pub(super) fn lower_buffer_body(ctx: &mut LowerCtx, d: &BufferDecl) {
    let rec = match ctx.lookup_buffer(&d.name) {
        Some(r) => r.clone(),
        None => return,
    };
    let saved_chain = ctx.builder.current_chain_id;
    let chain = ctx.alloc_chain();
    ctx.builder.current_chain_id = Some(chain);
    if let Some(node) = ctx.builder.module.nodes.get_mut(&rec.node_id) {
        node.chain_id = Some(chain);
    }
    let rhs_port = lower_expr(ctx, &d.init);
    // Through `ctx.connect` (not `builder.connect`) so the string → bool
    // coercion choke point sees this wire — `buffer buf: bool = s` must get
    // its `!= ""` compare like every other bool-typed destination.
    ctx.connect(rhs_port, rec.node_id.port(WirePort::Input));
    ctx.builder.current_chain_id = saved_chain;
}

/// Anonymous chip: reuses the Chip node created during pre-declare.
/// Processes the body in the PARENT scope with chip_id set so nodes
/// get tagged for the emitter to route into a child grid.
pub(super) fn lower_anon_chip(ctx: &mut LowerCtx, d: &AnonChipDecl) {
    // Find the chip node that was created during pre_declare_decl.
    let chip_node_id = ctx
        .builder
        .module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.kind == NodeKind::Chip
                && n.source_range == d.range
                && n.chip_id == ctx.current_anon_chip
        })
        .map(|(id, _)| *id);
    let Some(chip_node_id) = chip_node_id else {
        return;
    };

    let saved_chip = ctx.current_anon_chip.take();
    ctx.current_anon_chip = Some(chip_node_id);

    lower_chip_body(ctx, &d.body);

    ctx.current_anon_chip = saved_chip;
}

/// True for a chip-body statement that is a pure, reactive DECLARATION — the
/// same set `is_pure_top_decl` uses at the top level.
fn is_pure_chip_stmt(s: &Stmt) -> bool {
    matches!(
        s,
        Stmt::Let(_) | Stmt::OutBinding(_) | Stmt::Var(_) | Stmt::Buffer(_)
    )
}

/// Lower an anon-chip body. A `chip { }` is a visual grouping whose top-level
/// statements behave like TOP-LEVEL declarations, not steps on a handler's exec
/// spine: a PURE statement (`let`/`var`/`out`/`buffer`) is reactive signal flow
/// and must NOT inherit the ambient (e.g. post-handler) `current_exec`, or its
/// var reads latch onto that chain as a one-shot `Exec_Var_Get` frozen at the
/// var's init instead of the var's live `.Value`. An EXEC statement (`if`,
/// assignment, expr) KEEPS the ambient exec, so a chip that runs imperative
/// logic (e.g. draining an input queue with `if q.length() > 0 { … }`) still
/// fires. Mirrors the top-level decl loop's `is_pure_top_decl` handling; the
/// rest matches `lower_block` (pre-declare nested chips, flush handler execs,
/// trailing-emit terminates the chain).
fn lower_chip_body(ctx: &mut LowerCtx, block: &Block) {
    for s in &block.stmts {
        if let Stmt::AnonChip(ac) = s {
            pre_declare_decl(ctx, &TopDecl::AnonChip(ac.clone()));
        }
    }
    for s in &block.stmts {
        let is_handler_stmt = matches!(s, Stmt::Handler(_) | Stmt::AnonChip(_));
        if !ctx.handler_end_execs.is_empty() && !is_handler_stmt {
            flush_handler_end_execs(ctx);
        }
        if is_pure_chip_stmt(s) {
            let saved_exec = ctx.current_exec.take();
            lower_stmt(ctx, s);
            ctx.current_exec = saved_exec;
        } else {
            lower_stmt(ctx, s);
        }
    }
    if let Some(Stmt::Emit(e)) = block.stmts.last()
        && (ctx.signal_key(&e.name).is_some() || ctx.lookup_output(&e.name).is_some())
    {
        ctx.current_exec = None;
    }
}

pub(super) fn lower_chip_decl(ctx: &mut LowerCtx, d: &ChipDecl) {
    // Inline declarations (mod keyword or ref params) are stored for
    // expansion at call sites, not compiled as standalone microchips.
    let has_ref_params = d
        .inputs
        .iter()
        .any(|p| matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }));
    if d.inline || has_ref_params {
        ctx.scope
            .insert(&d.name, Binding::Chip(std::sync::Arc::new(d.clone())));
        return;
    }

    // Standalone chips: register for instantiation at call sites.
    ctx.scope
        .insert(&d.name, Binding::Chip(std::sync::Arc::new(d.clone())));
}

pub(super) fn lower_let_decl(ctx: &mut LowerCtx, d: &LetDecl) {
    // Clear any leftover inline-mod record so only THIS statement's call can set
    // it (an inline mod call within `d.value` sets it definitively at its end).
    ctx.pending_inline_record = None;

    // `let name: exec` — local exec signal, register as emit target. Checked
    // first and returns unconditionally: an exec-typed `let` (parsed with a
    // placeholder `0` value when it has no `= ...` initializer — see the
    // parser) never names a constant, so it must not reach the const
    // recording below.
    if let Some(TypeExpr::Name {
        name: ref type_name,
        ..
    }) = d.typ
    {
        if type_name == "exec" {
            if let LetBinding::Ident { name, .. } = &d.binding {
                // Top-level signals were already hubbed by the pre-declare
                // pass; skip the pass-2 revisit (no exec context at top level).
                // Body-level declarations always build a fresh hub — each
                // mod/handler instance is its own signal, even under the same
                // name (shadowing any outer signal).
                if ctx.current_exec.is_none() && ctx.signal_key(name).is_some() {
                    return;
                }
                build_exec_signal_hub(ctx, name, &d.range);
            }
            return;
        }
    }

    // A plain (non-`const`) `let` re-binding a name CLEARS the `const` mark
    // and value it shadows — for EVERY binding form, and using the FULL
    // evaluator, because that is exactly what `typecheck::stmt`'s `Stmt::Let`
    // arm does on both of its sides (`Ident` and destructuring alike).
    //
    // This is the CLEARING half of the const environment, and it has to be
    // symmetric with typecheck's for the same reason the recording half is:
    // `scoped_const_declared` is what BOTH sides' `if` arms consult
    // (`if_cond_const_ctx`/`const_lookup_declared_only`) to decide a condition
    // is compile-time decidable. Clearing LESS here than typecheck clears
    // there is a silent wrong-branch miscompile — typecheck sees a shadowed,
    // no-longer-constant name and checks both arms, while lowering still holds
    // the stale value, decides the condition, and emits only one arm with no
    // Branch gate at all. Three spellings reached that, all measured:
    //   `const { x } = { x: 111 }` + `let { x } = { x: 222 }` — only the
    //     `Ident` binding form was handled here at all;
    //   `const x = 111` + `let x = if true then 222 else 0` — an `Ident`
    //     whose value only the FULL evaluator folds, so the narrow recording
    //     below never ran;
    //   the same `Ident` case with the `const` at the TOP level — the eviction
    //     `const_lookup_declared_only` performs is driven by the name being
    //     PRESENT in a `scoped_consts` frame and ABSENT from that frame's mark
    //     set, so merely removing the mark evicts nothing when the value lives
    //     in `const_env`.
    //
    // Each arm mirrors its typecheck counterpart exactly, and the difference
    // between them is load-bearing rather than an inconsistency to tidy away:
    // typecheck's `Ident` arm RECORDS the newly evaluated value (which is what
    // makes the eviction above fire for an outer/top-level constant, and is
    // also the value a constant-only config slot must now see), while its
    // destructuring arm REMOVES. Inserting on the destructuring side too would
    // evict where typecheck does not — lowering emitting an arm typecheck
    // elided and never checked, the one direction that is never safe.
    //
    // Gated on the evaluation SUCCEEDING, again mirroring typecheck exactly:
    // its `Err(_)` arm leaves the mark alone, so clearing unconditionally
    // (from `bound_names`, say) would diverge in that same bad direction.
    // Routed through the shared `bind_destructured` so "which names does this
    // binding re-bind" cannot drift from the answer the recording sites use.
    //
    // Confined to names that currently read as a DECLARED `const`
    // (`is_declared_const`): a shadow of anything else is not part of this
    // feature, and skipping it keeps the promise that a program using no
    // `const` keyword lowers exactly as it did before — every
    // `scoped_const_declared` frame is empty there, so this whole block is
    // inert.
    if !d.is_const {
        let rebound: Vec<(String, Literal)> = {
            let lookup = |n: &str| ctx.resolve_mod(n);
            let mut budget = crate::const_eval::Budget::default();
            crate::const_eval::eval_expr(&d.value, &ctx.const_ctx(Some(&lookup)), &mut budget)
                .and_then(|lit| crate::const_eval::bind_destructured(&d.binding, lit))
                .unwrap_or_default()
        };
        let is_ident = matches!(&d.binding, LetBinding::Ident { .. });
        for (name, lit) in rebound {
            if !ctx.is_declared_const(&name) {
                continue;
            }
            if let Some(frame) = ctx.scoped_consts.last_mut() {
                if is_ident {
                    frame.insert(name.clone(), lit);
                } else {
                    frame.remove(&name);
                }
            }
            if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                frame.remove(&name);
            }
        }
    }

    // A body-local `let name = <constant>` is recorded in the innermost
    // `scoped_consts` frame (mirroring how a top-level `let` lands in
    // `const_env` via `build_const_env`), so a constant-only config arg
    // elsewhere in this scope (or a nested one) can resolve `name` via
    // `ctx.const_lookup()` — see `literal_for_property_port`. This is a
    // no-op at the top level: `lower_let_decl` also handles `TopDecl::Let`
    // (via `lower_decl`), but no `push_scope` has run there yet, so
    // `scoped_consts` is empty and `last_mut()` finds no frame — top-level
    // constants are already covered by `const_env`.
    //
    // A plain (non-`const`) `let`: NARROW evaluator, exactly as before this
    // feature existed. Widening this one would change how a program using no
    // `const` at all compiles — `literal_for_property_port` would start baking
    // config args whose producing gate must survive (the `@nofold let s =
    // "a${1 + 1}b"` case gate 2 below describes) — so it is deliberately left
    // on `expr_to_literal_in`. It re-inserts the value the clearing above just
    // dropped, which is why it must run AFTER it. The clearing's full
    // evaluator does not subsume this one: `expr_to_literal_in` also folds a
    // prefab reference, which `eval_expr` refuses.
    if let LetBinding::Ident { name, .. } = &d.binding
        && !d.is_const
        && let Some(lit) = expr_to_literal_in(&d.value, &ctx.const_lookup())
    {
        if let Some(frame) = ctx.scoped_consts.last_mut() {
            frame.insert(name.clone(), lit);
        }
        // A plain `let` re-binding a name CLEARS any `const` mark it had —
        // see `LowerCtx::const_declared`'s doc comment.
        if let Some(frame) = ctx.scoped_const_declared.last_mut() {
            frame.remove(name);
        }
    }

    // A `const` binding — EVERY binding form, `Ident` and destructuring
    // alike. This is a CORRECTNESS requirement, not a convenience:
    // `typecheck::stmt`'s `Stmt::Let` arm records these into its own
    // `scoped_consts`/`scoped_const_declared` using the FULL evaluator, and
    // `scoped_const_declared` is what BOTH sides' `if` arms consult
    // (`if_cond_const_ctx`) to decide a condition is compile-time decidable.
    // Recording less here than typecheck records there makes typecheck skip
    // an untaken block that lowering still EMITS — ill-typed code reaching
    // the graph with no diagnostic at all, which is exactly the divergence
    // `const_eval`'s module doc and `lower_if`'s "the same evaluator ... so
    // the two sides agree" comment exist to prevent. Guarded by
    // `typecheck_and_lowering_drop_exactly_the_same_ranges`.
    //
    // The narrow `expr_to_literal_in` above is NOT sufficient here: it cannot
    // fold a record/array/map literal, string interpolation, an `if`
    // expression, indexing, or a certified method call, all of which the full
    // evaluator (and therefore typecheck) folds. A block-scope
    // `const p = { x: 1 }` was recorded by typecheck and NOT by lowering, so
    // `if p.x == 1 { … } else { … }` elided only on the typecheck side — the
    // same one-sided elision, reachable with no destructuring at all.
    //
    // Restricted to `is_const` so a program using no `const` keyword compiles
    // byte-identically to before (the feature's own first design rule); a
    // `const` is a semantic guarantee that the value IS compile-time, so
    // widening it cannot change the meaning of a program that had none.
    //
    // Unlike the const-mod path below this only RECORDS and never
    // early-returns, so it cannot delete a gate: `lower_ident` consults
    // `const_lookup()` only for a name with NO scope binding at all, so every
    // runtime read still resolves through the port binding the ordinary
    // lowering installs.
    if d.is_const {
        let pairs = {
            let lookup = |n: &str| ctx.resolve_mod(n);
            let mut budget = crate::const_eval::Budget::default();
            crate::const_eval::eval_expr(&d.value, &ctx.const_ctx(Some(&lookup)), &mut budget)
                .and_then(|lit| crate::const_eval::bind_destructured(&d.binding, lit))
                .unwrap_or_default()
        };
        // A `const` whose value IS an array or map literal. Recorded above like
        // any other constant, and then NOT lowered — the same early-return
        // shape as the prefab-reference and const-mod skips below, and for the
        // same reason: `lower_expr` has no `Expr::Array` arm at all and an
        // explicit `Expr::MapLit` -> unsupported arm, so falling through emits
        // a placeholder `_Unsupported` gate plus a WSP001 warning for a value
        // the compiler has already computed correctly.
        //
        // Gated on the SYNTACTIC form, not merely on the evaluated literal's
        // kind, so this can only ever delete a placeholder: those are the two
        // expression forms with no runtime lowering to lose. Gated on
        // `d.is_const` as well, so a plain `let xs = [1, 2, 3]` keeps its
        // existing behavior and a program using no `const` compiles unchanged.
        //
        // A runtime read of the name still works: it materializes the container
        // on demand — see `predeclare::materialize_const_container`.
        let container = matches!(&d.value, Expr::Array { .. } | Expr::MapLit { .. })
            && pairs
                .iter()
                .any(|(_, l)| matches!(l, Literal::Array(_) | Literal::Map(_)));
        for (name, lit) in pairs {
            if let Some(frame) = ctx.scoped_consts.last_mut() {
                frame.insert(name.clone(), lit);
            }
            if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                frame.insert(name);
            }
        }
        if container {
            return;
        }
    }

    // An initializer whose value can ONLY be obtained by CALLING a `const
    // mod`: record the value and STOP, skipping the ordinary lowering of the
    // expression.
    //
    // This is a correctness requirement, not an optimization. A `const mod`
    // that ASSEMBLES a collection (`const t = []` … `t.push(x)` … `return t`)
    // has NO valid ordinary lowering at all: its mutations target a `const`
    // binding, which has no array/map var for `lower_array_method` to point
    // at, so inlining the body emitted WS044 "the operation would be silently
    // dropped" — once per mutation — for a `const r = rooms(2)` whose value
    // the compiler had ALREADY computed correctly. Binding such a mod's result
    // to a name is the natural way to reuse one, so that failure hit exactly
    // the spelling users reach for first.
    //
    // THREE gates, and each is load-bearing — any one alone is wrong (gate 3
    // is stated at its own `.filter`, below):
    //
    // 1. `expr_to_literal_in` must have FAILED. Its successes are the
    //    pre-existing folding surface, and other parts of lowering depend on
    //    those still producing a real WIRE: a named Vector/Rotator/Quat/Color
    //    constant must keep its wired `Make*` producer (see
    //    `literal_for_property_port`, which excludes exactly those four from
    //    its own bare-`Ident` constant path for this reason — resolving them
    //    to a literal lets a receiver like `dir.RotationByAngle(…)` skip the
    //    wire and silently prunes the producing gate as a wireless orphan).
    //
    // 2. The value must be UNREACHABLE without `lookup_mod` — i.e. a const-mod
    //    call is genuinely required, not merely present-in-principle. Gate 1
    //    alone also catches string interpolation and `if`-expressions, which
    //    the full evaluator can fold but the narrow one cannot; eliding those
    //    silently deleted real gates from programs containing no `const` at
    //    all (`@nofold let s = "a${1 + 1}b"` lost its FormatText + MathAdd,
    //    4 gates -> 2), violating both `@nofold`'s "nothing folded or elided"
    //    promise and this feature's rule that a program using no `const` must
    //    compile identically. Re-running the evaluator with `lookup_mod: None`
    //    answers "did this NEED a const mod?" exactly, and — unlike scanning
    //    the AST for a call node — cannot drift from what the evaluator
    //    actually does, since it IS the evaluator.
    //
    // Deliberately NOT gated on `nofold_depth`/`FoldMode`. `@nofold` is a
    // barrier against the fold pass, and a barrier can only suppress an
    // optimization that has a valid unoptimized form to fall back to; a
    // mutating `const mod` has none, so honoring `@nofold` here would just
    // restore the WS044 breakage under an attribute. (`@nofold const` is a
    // parse error anyway — the attribute only precedes `var`/`static
    // var`/`let`/`on` — and a `const` binding is a semantic guarantee that
    // the value is compile-time, which no fold setting can revoke.) The
    // `@nofold`-observable surface is unaffected in practice: gate 2 confines
    // this to expressions that had no faithful gate form to preserve.
    //
    // Every name skipped here still resolves in a WIRE position through
    // `lower_ident`'s `const_lookup()` fallback.
    if expr_to_literal_in(&d.value, &ctx.const_lookup()).is_none() {
        // Scope the immutable borrows of `ctx` (the `resolve_mod` closure and
        // `const_ctx`) so they end before the `scoped_consts` insert below
        // needs `&mut ctx`.
        let evaluated = {
            let mut budget = crate::const_eval::Budget::default();
            // Gate 2, first: with no `lookup_mod`, `eval_expr` cannot resolve
            // any const-mod call. Success here means the value never needed
            // one, so this initializer keeps lowering exactly as it did
            // before this feature existed.
            if crate::const_eval::eval_expr(&d.value, &ctx.const_ctx(None), &mut budget).is_ok() {
                None
            } else {
                let lookup = |n: &str| ctx.resolve_mod(n);
                let mut budget = crate::const_eval::Budget::default();
                crate::const_eval::eval_expr(&d.value, &ctx.const_ctx(Some(&lookup)), &mut budget)
                    .ok()
            }
        };
        // Split the one evaluated value across whatever names the binding
        // introduces, through the SAME `bind_destructured` the const
        // recording above, `build_const_env` and both typecheck sites use.
        // A destructuring binding is NOT a second, lesser spelling here: a
        // multi-output `const mod` (`-> (a: …, b: …)`) evaluates to a
        // `Literal::Record`, and `const { a, b } = pair(2)` is the only way
        // to bind its outputs — so restricting this to `Ident` left every
        // multi-output const mod lowering its body as ordinary gates, which
        // is precisely the failure the comment above says this path exists
        // to prevent (a mutating one hit WS044; one whose `out` sits inside
        // a const `if` hit WS002 "no field ... available fields: ..."; a
        // plain one just emitted dead gates for every `out` value).
        // `Tuple` still splits nowhere, so it simply never takes this path.
        if let Some(lit) = evaluated
            && let Ok(pairs) = crate::const_eval::bind_destructured(&d.binding, lit)
            // GATE 3, and it exists only for records — now stated over the
            // BOUND VALUES rather than the whole evaluated one, which is the
            // same rule with the destructuring case included. Unlike every
            // other bakeable kind, a record ALREADY has a non-literal
            // lowering below (`Expr::RecordLit` -> `Binding::Record`, and the
            // multi-output-call paths further down, all of which bind each
            // field to its own port), and baking here early-returns straight
            // past it. That is not a missed optimization, it is a miscompile:
            // `let p: Point = { x: mk(3), y: 10 }` (a field needing a
            // const-mod call, so gates 1 and 2 both pass it through) lost its
            // `Binding::Record`, and the later `p.x`/`p.y` reads fell through
            // to the vector-swizzle path — a bogus `Expr_SplitVector`
            // `.X`/`.Y` on a record, plus the `Literal::Record` reaching emit
            // and tripping its `unreachable!`. Arrays and maps have no such
            // existing lowering to hijack, which is why only this one kind
            // needs excluding. A record's FIELDS still bake through
            // `lower_record_lit`'s own per-field constant folding.
            //
            // Checking the SPLIT values is what lets a multi-output const
            // mod through while keeping that guarantee exactly: `const { a, b
            // } = pair(2)` binds two scalars and the record itself is never
            // stored under any name, whereas `const dummy = pair(2)` (whole
            // record under one name) and `const { x, ...rest } = p` (a
            // `...rest` re-collects one) still keep their existing lowering.
            && !pairs.iter().any(|(_, l)| matches!(l, Literal::Record(_)))
        {
            for (name, lit) in pairs {
                // Body-local only, exactly like the narrow recording above: a
                // top-level `let`/`const` has no open frame here and is already
                // covered by `build_const_env`.
                if let Some(frame) = ctx.scoped_consts.last_mut() {
                    frame.insert(name.clone(), lit);
                }
                // Same is_const tracking as the narrow recording above — a
                // plain `let` that reaches this path (its value NEEDED a
                // `const mod` call) is still just a plain `let`.
                if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                    if d.is_const {
                        frame.insert(name);
                    } else {
                        frame.remove(&name);
                    }
                }
            }
            return;
        }
    }

    // A prefab-only reference (`let pf = $./file.brz` / an inline nested-
    // prefab block): constant-only — it lives in the const env (recorded
    // above, resolved by `literal_for_property_port` wherever `pf` is later
    // passed as a `prefab = pf` config arg) and has no wire value of its
    // own. `Expr::PrefabRef`/`Expr::NestedPrefab` aren't handled by the
    // general expression lowerer (`lower_expr` falls through to
    // `synthesise_unsupported` for them), so without this early return —
    // mirroring the record-lit/record-alias early-returns below — the `let`
    // would emit a placeholder `_Unsupported` gate plus a warning even
    // though the reference resolves fine as a constant.
    if matches!(&d.value, Expr::PrefabRef { .. } | Expr::NestedPrefab { .. }) {
        return;
    }

    // Handle record literals specially — they produce a Binding::Record,
    // not a single PortRef.
    if let Expr::RecordLit { fields, .. } = &d.value {
        let record = lower_record_lit(ctx, fields);
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(record));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &record, destruct_fields);
                return;
            }
            LetBinding::Tuple { names, rest, .. } => {
                let order = tuple_positions(&ctx.type_of(&d.value), names.len());
                install_tuple_destruct(ctx, &record, names, rest.as_ref(), &order);
                return;
            }
            _ => {
                // Record name destructuring on a record lit — fall through
                // to normal handling (unlikely but safe).
            }
        }
    }

    // Handle RHS that is an ident referencing a record binding.
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let Some(Binding::Record(src)) = ctx.scope.get(rhs_name).cloned()
    {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(src));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &src, destruct_fields);
                return;
            }
            LetBinding::Tuple { names, rest, .. } => {
                let order = tuple_positions(&ctx.type_of(&d.value), names.len());
                install_tuple_destruct(ctx, &src, names, rest.as_ref(), &order);
                return;
            }
            _ => {}
        }
    }

    // Handle RHS that is a field-chain resolving to a record binding.
    if let Some(Binding::Record(src)) = resolve_field_chain(ctx, &d.value).cloned() {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(src));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &src, destruct_fields);
                return;
            }
            LetBinding::Tuple { names, rest, .. } => {
                let order = tuple_positions(&ctx.type_of(&d.value), names.len());
                install_tuple_destruct(ctx, &src, names, rest.as_ref(), &order);
                return;
            }
            _ => {}
        }
    }

    let rhs_port = lower_expr(ctx, &d.value);
    let rhs_type = ctx.type_of(&d.value);

    // Multi-output inline mod: the call stashed a field→source-port record (its
    // output nodes are internal and were removed). Bind the record directly.
    if matches!(&d.value, Expr::Call { .. })
        && let Some(record) = ctx.pending_inline_record.take()
    {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(record));
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &record, destruct_fields);
            }
            LetBinding::Tuple { names, rest, .. } => {
                let order = tuple_positions(&rhs_type, names.len());
                install_tuple_destruct(ctx, &record, names, rest.as_ref(), &order);
            }
            _ => {}
        }
        return;
    }

    // Multi-output chip/call: the rhs_port points to the first output's
    // MicrochipOutput node. If the type is a Record, look up the chip node
    // that owns these outputs and build field→port bindings.
    if let Type::Record(ref fields) = rhs_type {
        if let Expr::Call { .. } = &d.value {
            // Find the chip node whose outputs include rhs_port.node_id
            let chip_entry = ctx
                .builder
                .module
                .chips
                .iter()
                .find(|(_, child)| child.outputs.contains(&rhs_port.node_id));
            if let Some((_, child)) = chip_entry {
                let outputs = child.outputs.clone();
                let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
                for (i, (field_name, _ty)) in fields.iter().enumerate() {
                    if let Some(&out_id) = outputs.get(i) {
                        record.insert(
                            crate::intern::intern(field_name),
                            Binding::Local(LocalRecord {
                                port: out_id.port(WirePort::RerOutput),
                            }),
                        );
                    }
                }
                match &d.binding {
                    LetBinding::Ident { name, .. } => {
                        ctx.scope.insert(&name, Binding::Record(record));
                    }
                    LetBinding::RecordDestruct {
                        fields: destruct_fields,
                        ..
                    } => {
                        install_record_destruct(ctx, &record, destruct_fields);
                    }
                    LetBinding::Tuple { names, rest, .. } => {
                        let order = tuple_positions(&rhs_type, names.len());
                        install_tuple_destruct(ctx, &record, names, rest.as_ref(), &order);
                    }
                    _ => {}
                }
                return;
            }
            // A builtin multi-output gate (e.g. `character.InputReader()`) owns
            // its outputs directly — no chip wraps them, so the lookup above
            // finds nothing. Bind each declared field to the gate's matching
            // output port, the same mapping `r.Forward` resolves through.
            // Without this a destructure bound nothing and every use became an
            // `_Unsupported` placeholder wired to no source.
            let record: HashMap<crate::intern::Sym, Binding> = fields
                .iter()
                .filter_map(|(field_name, _ty)| {
                    let port =
                        resolve_output_field_port(ctx, rhs_port.node_id, field_name)?;
                    Some((
                        crate::intern::intern(field_name),
                        Binding::Local(LocalRecord { port }),
                    ))
                })
                .collect();
            // Only destructuring needs this. Binding the whole call to a name
            // must stay a `Local` on the gate's default output: field access
            // already resolves siblings through the node's ports, and making it
            // a record would break bare use of the result (`let p = a.pop()`
            // reads the popped element, not a record).
            if !record.is_empty()
                && let LetBinding::RecordDestruct {
                    fields: destruct_fields,
                    ..
                } = &d.binding
            {
                install_record_destruct(ctx, &record, destruct_fields);
                return;
            }
        }
    }

    match &d.binding {
        LetBinding::Ident { name, .. } => {
            // Tag this `let`'s value node with the binding name, so a boundary
            // pin that later captures it across a chip edge derives its label
            // from `progress`/`init` rather than a synthetic `ext1`/`ext2`.
            // Derivation-only (emit ignores it), and it never overrides a node
            // that already carries a name (a var/param keeps its own).
            if let Some(node) = ctx.builder.module.nodes.get_mut(&rhs_port.node_id) {
                let props = std::sync::Arc::make_mut(&mut node.properties);
                if !props.contains_key(&*crate::intern::sym::NAME_LABEL)
                    && !props.contains_key(&*crate::intern::sym::BINDING_NAME)
                {
                    props.insert(
                        *crate::intern::sym::BINDING_NAME,
                        crate::ir::Literal::String(name.clone()),
                    );
                }
            }
            ctx.scope
                .insert(&name, Binding::Local(LocalRecord { port: rhs_port }));
        }
        LetBinding::Tuple { names, .. } | LetBinding::Record { names, .. } => {
            let source_node = ctx.builder.module.nodes.get(&rhs_port.node_id).cloned();
            if let Some(node) = source_node {
                for (i, name) in names.iter().enumerate() {
                    if let Some(port) = node.ports.outputs.get(i) {
                        ctx.scope.insert(
                            &name,
                            Binding::Local(LocalRecord {
                                port: port_ref(node.id, crate::intern::resolve(port.name)),
                            }),
                        );
                    }
                }
            }
        }
        LetBinding::RecordDestruct { .. } => {
            // Record destructuring of non-record RHS — nothing to do.
            // TODO: this is an error..?
        }
    }
}

/// Lower a record literal into a `HashMap<Sym, Binding>`.
pub(super) fn lower_record_lit(
    ctx: &mut LowerCtx,
    fields: &[RecordLitField],
) -> HashMap<crate::intern::Sym, Binding> {
    let mut map = HashMap::default();
    for field in fields {
        match field {
            RecordLitField::Named { name, value, .. } => {
                // Check if value is itself a record literal (nested records).
                if let Expr::RecordLit {
                    fields: inner_fields,
                    ..
                } = value
                {
                    let inner = lower_record_lit(ctx, inner_fields);
                    map.insert(crate::intern::intern(name), Binding::Record(inner));
                } else if let Some(binding) = resolve_field_chain(ctx, value).cloned() {
                    // Value references something in scope (possibly through a record chain).
                    map.insert(crate::intern::intern(name), binding);
                } else {
                    // Otherwise evaluate as expression and store as Local.
                    let port = lower_expr(ctx, value);
                    map.insert(
                        crate::intern::intern(name),
                        Binding::Local(LocalRecord { port }),
                    );
                }
            }
            RecordLitField::Shorthand { name, .. } => {
                // { foo } means { foo: foo } — look up foo in scope.
                if let Some(binding) = ctx.scope.get(name).cloned() {
                    map.insert(crate::intern::intern(name), binding);
                }
            }
            RecordLitField::Spread { value, .. } => {
                // ...expr — expr must resolve to a Binding::Record.
                if let Some(Binding::Record(src_fields)) = resolve_field_chain(ctx, value).cloned()
                {
                    for (k, v) in src_fields {
                        map.insert(k, v); // later fields override
                    }
                }
            }
        }
    }
    map
}

/// Install record destructure bindings from a source record into the scope.
pub(super) fn install_record_destruct(
    ctx: &mut LowerCtx,
    src: &HashMap<crate::intern::Sym, Binding>,
    destruct_fields: &[RecordDestructField],
) {
    let mut remaining = src.clone();
    for field in destruct_fields {
        match field {
            RecordDestructField::Named {
                name, alias, range, ..
            } => {
                let key = crate::intern::intern(name);
                if let Some(binding) = remaining.remove(&key) {
                    let bind_name = alias.as_deref().unwrap_or(name);
                    ctx.scope.insert(&bind_name, binding);
                } else {
                    // Binding nothing would leave every use of the name an
                    // `_Unsupported` placeholder wired to no source — a circuit
                    // that silently reads 0. Field names are case-sensitive, so
                    // point at a differently-cased field when there is one.
                    let available: Vec<String> = src
                        .keys()
                        .map(|k| crate::intern::resolve(*k).to_string())
                        .collect();
                    let suggestion = available
                        .iter()
                        .find(|f| f.eq_ignore_ascii_case(name))
                        .map(|f| format!(" — did you mean `{f}`?"))
                        .unwrap_or_else(|| {
                            let mut names = available.clone();
                            names.sort();
                            format!(" — available fields: {}", names.join(", "))
                        });
                    ctx.diagnostics.push(Diagnostic::error(
                        "WS002",
                        format!("no field `{name}` on this value{suggestion}"),
                        range.clone(),
                    ));
                }
            }
            RecordDestructField::Rest { name, .. } => {
                ctx.scope
                    .insert(&name, Binding::Record(remaining.clone()));
            }
        }
    }
}

/// Bind a tuple pattern's names against a record source. Tuple literals lower
/// to a `Binding::Record` keyed by the element index (`"0"`, `"1"`, ...), so
/// positional names read straight out of that map. `rest` collects the tail,
/// re-indexed from zero so it stays a well-formed tuple.
/// The field names of `ty`, in declaration order — the POSITIONS a tuple
/// pattern binds against. A `Binding::Record` is a `HashMap` and so carries no
/// order of its own; the static type does, for both spellings that reach a
/// tuple pattern: a tuple literal types as an index-keyed record (`"0"`,
/// `"1"`, …) and a multi-output `mod` call as a NAME-keyed one in its
/// signature's declaration order. Falls back to index keys when the type is
/// not a record, which is exactly the pre-existing behaviour.
fn tuple_positions(ty: &Type, arity: usize) -> Vec<String> {
    match ty {
        Type::Record(fields) => fields.iter().map(|(n, _)| n.clone()).collect(),
        _ => (0..arity).map(|i| i.to_string()).collect(),
    }
}

/// Bind a tuple pattern positionally against `src`, reading the positions in
/// `order` (see [`tuple_positions`]). `rest` collects everything past `names`,
/// re-keyed by its new position so `rest.0` is the first leftover — the same
/// re-keying `const_eval::destructure`'s `Tuple` arm does, so the compile-time
/// and runtime halves of the same pattern agree.
pub(super) fn install_tuple_destruct(
    ctx: &mut LowerCtx,
    src: &HashMap<crate::intern::Sym, Binding>,
    names: &[String],
    rest: Option<&String>,
    order: &[String],
) {
    for (i, name) in names.iter().enumerate() {
        if let Some(key) = order.get(i)
            && let Some(binding) = src.get(&crate::intern::intern(key)).cloned()
        {
            ctx.scope.insert(name, binding);
        }
    }
    if let Some(rest_name) = rest {
        let mut tail: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for (i, key) in order.iter().skip(names.len()).enumerate() {
            if let Some(binding) = src.get(&crate::intern::intern(key)).cloned() {
                tail.insert(crate::intern::intern(&i.to_string()), binding);
            }
        }
        ctx.scope.insert(rest_name, Binding::Record(tail));
    }
}
