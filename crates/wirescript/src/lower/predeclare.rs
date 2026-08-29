use super::*;

// ---------- pre-declaration pass ----------

/// Record a `chip`/`mod` declaration into the pass-1 lookup that
/// [`array_elem_literal`]/[`map_entry_literal`] resolve a `const mod` CALL
/// through (see [`LowerCtx::pass1_chips`], and why it is deliberately NOT
/// `ctx.scope`).
///
/// Records into the INNERMOST open frame, so the declaration lives exactly as
/// long as the scope that declared it — a body-local `const mod` shadows a
/// same-named outer one inside that body and is gone once `pop_scope` drops
/// the frame. No manual save/restore is involved or needed.
///
/// MUST be called from the SAME source-ordered loop that pre-declares that
/// statement/declaration list's vars, never from a separate pre-pass:
/// a var declared BEFORE a `const mod` must not resolve it, matching
/// `ctx.scope`'s own sequential registration in pass 2 (which is what WS021's
/// use-before-declaration guard rests on).
pub(super) fn pre_declare_chip_name(ctx: &mut LowerCtx, c: &ChipDecl) {
    // The stack is never empty (the base frame holds the module's top-level
    // declarations), so this always lands somewhere.
    if let Some(frame) = ctx.pass1_chips.last_mut() {
        frame.insert(c.name.clone(), std::sync::Arc::new(c.clone()));
    }
}

/// Pre-declare a top-level storage/input decl, deduped by source location.
/// When the same SOURCE declaration was already lowered — reached through
/// another import of the same file (a plain + `import * as` pair, `import {g}`
/// + `import {g as h}`, or a diamond) — bind `name` to the existing gate
/// instead of running `predeclare` to create a duplicate. Otherwise lower it
/// and record the resulting binding under its source location. A DIFFERENT
/// file's same-named decl has a distinct location, so real collisions still
/// get their own gate. See `LowerCtx::import_state_dedup` (N2/N3).
fn dedup_import_state(
    ctx: &mut LowerCtx,
    range: &SourceRange,
    name: &str,
    predeclare: impl FnOnce(&mut LowerCtx),
) {
    if let Some(binding) = ctx.reuse_import_state(range) {
        ctx.scope.insert(name, binding);
        return;
    }
    predeclare(ctx);
    if let Some(binding) = ctx.scope.get(name).cloned() {
        ctx.record_import_state(range, binding);
    }
}

pub(super) fn pre_declare_decl(ctx: &mut LowerCtx, d: &TopDecl) {
    match d {
        // Record the chip/mod name — WITHOUT touching `ctx.scope` (see
        // `LowerCtx::pass1_chips`'s doc comment for why not) — so a `const
        // mod` CALL embedded in a LATER top-level var/array/map initializer,
        // baked later in this SAME pass-1 loop, can resolve it via
        // `LowerCtx::resolve_mod_pass1`.
        TopDecl::Chip(c) => pre_declare_chip_name(ctx, c),
        // Var/buffer gates are created HERE (pass 1), not in lower_decl's
        // with_nofold wrap — honor the decl's @nofold during registration.
        // Each storage/input decl is deduped by source location: the same
        // source declaration reached through several imports of one file
        // (plain + `import * as`, `import {g}` + `import {g as h}`, a diamond)
        // shares ONE gate instead of one per import (N2/N3).
        TopDecl::Var(v) => dedup_import_state(ctx, &v.range, &v.name, |ctx| {
            ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v))
        }),
        TopDecl::Array(a) => {
            dedup_import_state(ctx, &a.range, &a.name, |ctx| pre_declare_array(ctx, a))
        }
        TopDecl::Map(m) => dedup_import_state(ctx, &m.range, &m.name, |ctx| pre_declare_map(ctx, m)),
        TopDecl::Buffer(b) => {
            dedup_import_state(ctx, &b.range, &b.name, |ctx| pre_declare_buffer(ctx, b))
        }
        TopDecl::In(i) => dedup_import_state(ctx, &i.range, &i.name, |ctx| pre_declare_input(ctx, i)),
        TopDecl::Out(o) => ctx.with_nofold(o.no_fold, |ctx| {
            pre_declare_output(
                ctx,
                &o.name,
                o.value.as_ref(),
                o.typ.as_ref(),
                o.side,
                o.label.as_deref(),
                o.label_expr.as_ref(),
                o.invisible,
                true,
                &o.range,
            )
        }),
        TopDecl::Let(l) => pre_declare_exec_signal(ctx, l),
        TopDecl::AnonChip(ac) => {
            let chip_node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::MICROCHIP_ALT,
                source_range: ac.range.clone(),
                ports: GateIO::default(),
                ..Default::default()
            });
            if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
                node.kind = NodeKind::Chip;
                let props = std::sync::Arc::make_mut(&mut node.properties);
                if ac.closed {
                    props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
                }
                if let Some(label) =
                    resolve_label_text(ac.label.as_deref(), ac.label_expr.as_ref(), &ctx.const_env)
                {
                    props.insert(*sym::NAME_LABEL, Literal::String(label));
                }
                if let Some(doc) = ctx.doc_comments.get(&ac.range.start.offset) {
                    props.insert(*sym::DOC_TEXT, Literal::String(doc.clone()));
                }
            }
            // Tag pre-declared nodes with chip_id.
            let saved = ctx.current_anon_chip.take();
            ctx.current_anon_chip = Some(chip_node_id);
            for s in &ac.body.stmts {
                match s {
                    // Nested declaration shadows a same-named outer one for
                    // any LATER initializer in this body — see
                    // `pre_declare_chip_name`.
                    Stmt::ChipDecl(c) => pre_declare_chip_name(ctx, c),
                    Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
                    Stmt::Buffer(b) => pre_declare_buffer(ctx, b),
                    Stmt::Array(a) => pre_declare_array(ctx, a),
                    Stmt::Map(m) => pre_declare_map(ctx, m),
                    Stmt::In(i) => pre_declare_input(ctx, i),
                    Stmt::OutBinding(o) if o.side.is_some() => {
                        report_non_root_side(ctx, &o.range);
                    }
                    _ => {}
                }
            }
            ctx.current_anon_chip = saved;
        }
        _ => {}
    }
}

/// Pre-declare a top-level `let x: exec` local signal: create a stable Union
/// "hub" gate, bind `x` to its `ExecOut` (so `on x` can trigger off it), and
/// register the emit target. `flush_pending_emits` later wires the union of all
/// `emit x` paths into the hub's `ExecA`. Non-`exec` lets are ignored here (they
/// lower normally in pass 2).
pub(super) fn pre_declare_exec_signal(ctx: &mut LowerCtx, l: &LetDecl) {
    let Some(TypeExpr::Name {
        name: type_name, ..
    }) = &l.typ
    else {
        return;
    };
    if type_name != "exec" {
        return;
    }
    let LetBinding::Ident { name, .. } = &l.binding else {
        return;
    };
    build_exec_signal_hub(ctx, name, &l.range);
}

/// Create the stable `Union` "hub" for a local `let x: exec` signal: bind `x`
/// to its `ExecOut` (so `await x` / `on x` / reads resolve to it) and register
/// the emit target. `flush_pending_emits` later wires the union of all `emit x`
/// paths into the hub's `ExecA`. Used for both top-level signals (this
/// pre-declare pass) and body-level signals (from `lower_let_decl`).
pub(super) fn build_exec_signal_hub(ctx: &mut LowerCtx, name: &str, range: &SourceRange) {
    let hub = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC_A,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::EXEC_B,
                    ty: Type::Exec,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
        },
        ..Default::default()
    });
    ctx.scope.insert(
        &name,
        Binding::Local(LocalRecord {
            port: hub.port(WirePort::ExecOut),
        }),
    );
    // Key the signal per-declaration (`name#hubId`), not by bare name: two
    // bodies declaring the same signal name are distinct signals. Emit/await
    // sites resolve the key through the scope binding (`LowerCtx::signal_key`).
    let key = format!("{name}#{hub}");
    ctx.exec_signal_hubs.insert(key.clone(), hub);
    ctx.exec_signal_keys.insert(hub, key.clone());
    ctx.pending_emits.entry(key).or_default();
}

/// Resolve a lowering-side type annotation to its `Type`. Delegates to the
/// crate's single canonical resolver (`types::resolve::resolve_type`); no
/// generic params or type aliases are in scope on this path (typecheck has
/// already resolved + flagged anything exotic with WS002), so an empty
/// params/aliases context is correct and the returned diagnostics are
/// discarded.
pub(super) fn type_of_type_expr(t: &TypeExpr) -> Type {
    let cx = crate::types::resolve::ResolveCtx {
        params: &[],
        type_aliases: &crate::collections::HashMap::default(),
        generic_aliases: &crate::collections::HashMap::default(),
    };
    crate::types::resolve::resolve_type(t, &cx, &mut Vec::new())
}

#[allow(dead_code)]
pub(super) fn is_entity_family(t: &Type) -> bool {
    matches!(
        t,
        Type::Controller | Type::Character | Type::Entity
    )
}

pub(super) use crate::types::mono::unwrap_ref;

/// Default initial literal for Pseudo_Var data structs, so the game knows
/// the variable's wire_graph_variant type. Only covers primitive types that
/// have a clean wire_graph_variant mapping; object/entity types are omitted
/// — the game defaults them correctly. Every Var must have one.
///
/// `pub(crate)` because typecheck needs the same correctly-TYPED zero value:
/// its `const`-parameter body-check seeds a type-shaped placeholder literal
/// (see `typecheck::decl`). Sharing one table keeps the two from drifting
/// apart as types are added.
pub(crate) fn default_literal_for_var_type(t: &Type) -> Option<Literal> {
    match t {
        Type::Bool => Some(Literal::Bool(false)),
        Type::Int => Some(Literal::Int(0)),
        Type::String => Some(Literal::String(String::new())),
        Type::Vector => Some(Literal::Vector {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        Type::Rotator => Some(Literal::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        }),
        Type::Quat => Some(Literal::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }),
        Type::Color => Some(Literal::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }),
        Type::Controller | Type::Character | Type::Entity => {
            Some(Literal::Object)
        }
        _ => Some(Literal::Float(0.0)),
    }
}

/// Compile-time constant environment: every top-level `let` whose initializer is
/// itself constant, by name. Lets an initializer name a constant (`1 << C_FLAG`)
/// instead of restating its value.
pub type ConstEnv = crate::collections::HashMap<String, Literal>;

/// Every `let`/`const` declaration that belongs to the module's TOP-LEVEL
/// constant scope, in source order.
///
/// That is not the same as "every `TopDecl::Let`": an anonymous `chip { }`
/// SHARES its parent's scope rather than opening one (see `AnonChipDecl`,
/// `typecheck::register`'s and `typecheck::decl`'s `AnonChip` arms, both of
/// which register the body's declarations into the parent, and
/// `lower::decl::lower_anon_chip`, which lowers the body without a
/// `push_scope`). A binding declared directly in such a body is therefore a
/// top-level binding that happens to be drawn inside a box, and its constant
/// must live in the same environment as the rest — nothing else ever opens a
/// `scoped_consts` frame for it, so without this descent an anonymous chip's
/// `const` would be recorded NOWHERE and would read back as a runtime value.
///
/// Descends through nested anonymous chips (each shares the same parent scope
/// in turn) and STOPS at anything that does open a scope of its own — a
/// handler, a named `chip`/`mod` body, an `if` block. Those already record
/// their body constants into their own `scoped_consts` frame.
/// A `let x: exec` local exec signal (parser-desugared to `= 0`), which must
/// never be collected as a constant (see the call site in `build_const_env`).
fn is_exec_signal_let(l: &LetDecl) -> bool {
    matches!(&l.typ, Some(TypeExpr::Name { name, .. }) if name == "exec")
}

fn scope_lets(decls: &[TopDecl]) -> Vec<&LetDecl> {
    fn walk_block<'a>(block: &'a Block, out: &mut Vec<&'a LetDecl>) {
        for s in &block.stmts {
            match s {
                Stmt::Let(l) => out.push(l),
                Stmt::AnonChip(ac) => walk_block(&ac.body, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for d in decls {
        match d {
            TopDecl::Let(l) => out.push(l),
            TopDecl::AnonChip(ac) => walk_block(&ac.body, &mut out),
            _ => {}
        }
    }
    out
}

/// The `const mod` declarations visible in the module's top-level scope, by
/// name — the same flattening [`scope_lets`] applies, for the same reason: a
/// `mod` declared in an anonymous chip's body is registered into the parent
/// scope (`predeclare`'s own `AnonChip` arm calls `pre_declare_chip_name` for
/// it), so a `const` initializer anywhere in the module can call it.
/// Every `mod`/`chip` declaration reachable at module scope (top-level plus
/// anon-chip bodies), keyed by name - const AND non-const. `build_const_env`
/// feeds this to `const_eval` as `lookup_mod`, which serves two purposes, both
/// safe with non-const mods present:
/// - const-mod CALL evaluation: every eval site (`eval_expr`'s `Expr::Call`
///   arm, `interp::exec_call_stmt`) gates on `decl.is_const`, so a non-const
///   mod resolves here but is never evaluated - it just declines to fold, same
///   as if it were absent.
/// - bare-variant SHADOWING: `const_eval::resolve_bare_variant_enum` consults
///   `lookup_mod` so a user `mod Some` shadows the bare prelude `Some`, matching
///   typecheck (whose `resolve_mod` also returns any chip/mod, const or not).
///   Collecting only `is_const` mods here would leave a plain `mod Some`
///   invisible to that shadow check - the exact typecheck-vs-const-eval
///   divergence this returns all mods to close.
fn scope_mods(decls: &[TopDecl]) -> HashMap<String, Arc<ChipDecl>> {
    fn walk_block(block: &Block, out: &mut HashMap<String, Arc<ChipDecl>>) {
        for s in &block.stmts {
            match s {
                Stmt::ChipDecl(c) => {
                    out.insert(c.name.clone(), Arc::new(c.clone()));
                }
                Stmt::AnonChip(ac) => walk_block(&ac.body, out),
                _ => {}
            }
        }
    }
    let mut out = HashMap::default();
    for d in decls {
        match d {
            TopDecl::Chip(c) => {
                out.insert(c.name.clone(), Arc::new(c.clone()));
            }
            TopDecl::AnonChip(ac) => walk_block(&ac.body, &mut out),
            _ => {}
        }
    }
    out
}

/// Collect the constant top-level `let` bindings of a script.
///
/// Iterates to a fixpoint so a constant may be defined in terms of an earlier
/// one (`let A = 1` then `let B = A + 1`) regardless of declaration order — once
/// imports are merged, order is not dependency order. A binding that never
/// resolves (it needs a runtime value, or it is part of a reference cycle) is
/// simply absent, so callers fall back to their existing "not a constant" path.
///
/// A `let`/`const` initializer that calls a top-level `const mod` resolves the
/// same way: `mods` is collected ONCE, up front, from the whole `decls` slice —
/// not incrementally alongside the fixpoint — so a call to a const mod
/// declared LATER in the file resolves exactly like one declared earlier
/// (only whether the CALLEE's own constant inputs have converged yet gates a
/// given attempt, never whether the callee has been "seen" yet). Each
/// attempt is evaluated through `const_eval::eval_expr` — the same evaluator
/// `typecheck::decl`'s per-decl `const` re-check uses against the fully
/// converged result of this very function — with a fresh, bounded `Budget`
/// per attempt (mirroring that same call site), so environment construction
/// can never recurse or loop unboundedly even across many fixpoint passes.
///
/// `enum_defs` is the whole-program enum registry (`typecheck::enums::build_registry`
/// / `TypeCheckCtx::enum_defs`). Both callers (`typecheck.rs`, `lower/mod.rs`)
/// already have one built before they reach this function, so it is threaded
/// in rather than rebuilt here, keeping this the ONE place lowering and
/// typecheck both read a `const` enum value's tag from.
pub fn build_const_env(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::collections::HashMap<String, crate::typecheck::enums::EnumDef>>,
) -> ConstEnv {
    let mods = scope_mods(decls);
    let lookup_mod = move |name: &str| mods.get(name).cloned();

    // Flattened via `scope_lets`, so an anonymous chip's body constants join
    // the same fixpoint as the top-level ones — see that function's comment.
    let lets = scope_lets(decls);
    let mut env = ConstEnv::default();
    // Namespaced constants: an `import * as Ns` module's top-level `let`/`const`
    // values are reachable as `Ns.name`. Evaluate each namespace's env in
    // ISOLATION (its lets see only its own module's consts, not the importer's),
    // then lift them into `"Ns.name"` keys so an importer's const initializer can
    // fold a namespaced member (`[Other.value]`). Seeded before the fixpoint so a
    // root const that reads a namespaced member settles.
    for d in decls {
        if let TopDecl::Namespace(ns) = d {
            for (name, value) in build_const_env(&ns.decls, enum_defs) {
                env.entry(format!("{}.{}", ns.name, name)).or_insert(value);
            }
        }
    }
    // Which decls have already produced their final answer, by position — a
    // bare `env.contains_key(name)` check has no meaning for a binding that
    // introduces several names. A decl is settled once its initializer EVALUATES:
    // the resulting value is final, and `bind_destructured` is a pure
    // function of it, so neither the value nor the split can change on a
    // later pass. A decl whose initializer does NOT yet evaluate is left
    // unsettled — that is exactly what the fixpoint exists to retry.
    let mut settled = vec![false; lets.len()];
    loop {
        let mut changed = false;
        for (i, l) in lets.iter().enumerate() {
            if settled[i] {
                continue;
            }
            // A `let x: exec` local signal is parser-desugared to `= 0`; that `0`
            // is a placeholder for a signal, not a constant value. Registering it
            // makes a later use of the signal resolve the NAME to `0`, which then
            // inlines `0` into a builtin's exec wire port (e.g. SpawnPrefab's
            // `destroyAll`) instead of wiring the signal. Never bind an exec let.
            if is_exec_signal_let(l) {
                settled[i] = true;
                continue;
            }
            // `enum_defs` is the `Arc`-backed registry both callers already hold,
            // so this per-fixpoint-iteration hand-off is a refcount bump, not a
            // deep clone of the whole (game-derived, growing) registry.
            let cx = crate::const_eval::ConstCtx {
                consts: env.clone(),
                module_consts: env.clone(),
                enum_defs: enum_defs.clone(),
                lookup_mod: Some(&lookup_mod),
            };
            let mut budget = crate::const_eval::Budget::default();
            let Ok(lit) = crate::const_eval::eval_expr(&l.value, &cx, &mut budget) else {
                continue; // not constant YET — retry on a later pass
            };
            settled[i] = true;
            // Split the evaluated value across whatever names the binding
            // introduces — one for `const x = …`, several for
            // `const { x, y } = p`. Routed through the SAME
            // `bind_destructured` the two typecheck sites and the const-mod
            // interpreter use, so the pre-pass cannot disagree with them
            // about which field lands on which name. A binding form it
            // cannot split (a tuple destructure, today) simply contributes
            // nothing, exactly as every non-`Ident` binding did before.
            let Ok(pairs) = crate::const_eval::bind_destructured(&l.binding, lit) else {
                continue;
            };
            for (name, value) in pairs {
                // FIRST declaration of a name wins: a duplicate must not
                // overwrite it, or the fixpoint could alternate and never
                // converge.
                if env.contains_key(&name) {
                    continue;
                }
                env.insert(name, value);
                changed = true;
            }
        }
        if !changed {
            return env;
        }
    }
}

/// Every TOP-LEVEL name declared with the `const` keyword (`const x = …`),
/// as opposed to a plain `let` that merely happens to fold — the set
/// [`TypeCheckCtx::const_declared`](crate::typecheck::TypeCheckCtx)/
/// `LowerCtx::const_declared` gate the widened `if`-condition elision on
/// (see `Stmt::If`/`lower_if`): the feature's own first design principle is
/// that a program using no `const` compiles identically to before, so the
/// generalised const-eval elision may only fire for a condition built
/// entirely from const-DECLARED names, never one that merely happens to be
/// foldable. A name whose initializer never evaluates (already reported as
/// WS046/047/048) is harmless to include here — it simply has no matching
/// entry in [`build_const_env`]'s result, so nothing ever resolves through
/// it.
///
/// A DESTRUCTURING `const` contributes every name it binds (`const { x, y } =
/// p` declares both `x` and `y` as const), via `const_eval::bound_names` —
/// the syntactic counterpart of the `bind_destructured` that
/// [`build_const_env`] splits the actual VALUES with, kept in that same
/// module so the two cannot disagree about which names a binding form
/// introduces.
pub fn build_const_declared_names(decls: &[TopDecl]) -> crate::collections::HashSet<String> {
    // Same [`scope_lets`] flattening as [`build_const_env`] — the two must
    // see the identical set of bindings, or a name would be marked
    // const-DECLARED with no value behind it (or the reverse).
    scope_lets(decls)
        .into_iter()
        .filter(|l| l.is_const)
        .flat_map(|l| crate::const_eval::bound_names(&l.binding))
        .collect()
}

/// Resolve an explicit `@label` override to its baked display text: the
/// string form (`@label("text")`) is used as-is; the expression form
/// (`@label(expr)`) is const-folded against the script's constant
/// environment via [`expr_to_literal_in`] — typecheck.rs already rejects a
/// non-constant expression, so a fold failure here just yields no override
/// (rather than double-reporting the error). `None` means "no override" —
/// the caller's own default (e.g. the decl's name) applies.
pub(super) fn resolve_label_text(
    label: Option<&str>,
    label_expr: Option<&Expr>,
    env: &ConstEnv,
) -> Option<String> {
    if let Some(s) = label {
        return Some(s.to_string());
    }
    let lit = expr_to_literal_in(label_expr?, env)?;
    Some(literal_to_label_text(&lit))
}

/// Render a folded `@label(expr)` literal as its baked display text.
fn literal_to_label_text(lit: &Literal) -> String {
    match lit {
        Literal::String(s) => s.clone(),
        Literal::Int(n) => n.to_string(),
        // A float reads the same 3-decimal / trailing-zero-trimmed way
        // FormatText renders one everywhere else (the certified render law),
        // not full `f64` precision.
        Literal::Float(f) => {
            crate::lower::fold::eval::render_for_format(&crate::lower::fold::eval::Value::Float(*f))
        }
        Literal::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Evaluate a constant unary operator. `None` = not foldable, which preserves
/// whatever error the caller would already have reported.
///
/// `pub(crate)` (re-exported at `lower::mod`) so `const_eval::expr::eval_expr`
/// can apply this SAME law to an operand it resolved itself (e.g. a `const
/// mod` call nested inside the operator) — the whole point being that the
/// operator's law lives in exactly one place regardless of which evaluator
/// reached it.
pub(crate) fn eval_const_unop(operator: &str, v: Literal) -> Option<Literal> {
    use crate::catalog::operators::op;
    match (operator, v) {
        (op::NEG, Literal::Int(n)) => Some(Literal::Int(n.wrapping_neg())),
        (op::NEG, Literal::Float(f)) => Some(Literal::Float(-f)),
        (op::NOT, Literal::Bool(b)) => Some(Literal::Bool(!b)),
        (op::BIT_NOT, Literal::Int(n)) => Some(Literal::Int(!n)),
        _ => None,
    }
}

/// Evaluate a constant binary operator, matching the gates' certified
/// semantics: 64-bit integer maths, and division / modulo by zero yielding 0
/// rather than trapping. Anything outside this set — or any operand pair whose
/// result would be ambiguous — returns `None` and stays an error, so this can
/// only ever turn a rejected program into a working one, never change the
/// meaning of one that already compiles.
///
/// `pub(crate)` (re-exported at `lower::mod`) for the same reason as
/// [`eval_const_unop`] — `const_eval::expr::eval_expr` applies this SAME law
/// to operands it resolved itself.
pub(crate) fn eval_const_binop(operator: &str, l: Literal, r: Literal) -> Option<Literal> {
    use crate::catalog::operators::op;
    use Literal::{Bool, Float, Int, String as Str};

    // String concatenation is the one non-numeric binary fold.
    if let (Str(a), Str(b)) = (&l, &r) {
        return match operator {
            op::CONCAT => Some(Str(format!("{a}{b}"))),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            _ => None,
        };
    }
    if let (Bool(a), Bool(b)) = (&l, &r) {
        let (a, b) = (*a, *b);
        return match operator {
            op::AND => Some(Bool(a && b)),
            op::OR => Some(Bool(a || b)),
            op::XOR => Some(Bool(a != b)),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            _ => None,
        };
    }
    // Two ints stay integral (bitwise and shifts are int-only); any float
    // operand promotes the pair to float, mirroring the operator overloads.
    if let (Int(a), Int(b)) = (&l, &r) {
        let (a, b) = (*a, *b);
        return match operator {
            op::ADD => Some(Int(a.wrapping_add(b))),
            op::SUB => Some(Int(a.wrapping_sub(b))),
            op::MUL => Some(Int(a.wrapping_mul(b))),
            op::DIV => Some(Int(if b == 0 { 0 } else { a.wrapping_div(b) })),
            op::REM => Some(Int(if b == 0 { 0 } else { a.wrapping_rem(b) })),
            op::BIT_AND => Some(Int(a & b)),
            op::BIT_OR => Some(Int(a | b)),
            op::BIT_XOR => Some(Int(a ^ b)),
            // A shift distance outside 0..64 is left unfolded rather than guessed.
            op::SHL => (0..64).contains(&b).then(|| Int(a << b)),
            op::SHR => (0..64).contains(&b).then(|| Int(a >> b)),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            op::LT => Some(Bool(a < b)),
            op::LE => Some(Bool(a <= b)),
            op::GT => Some(Bool(a > b)),
            op::GE => Some(Bool(a >= b)),
            _ => None,
        };
    }
    let num = |v: &Literal| match v {
        Int(n) => Some(*n as f64),
        Float(f) => Some(*f),
        _ => None,
    };
    let (a, b) = (num(&l)?, num(&r)?);
    // Match the gates: a non-finite result reads as 0.
    let fin = |f: f64| Float(if f.is_finite() { f } else { 0.0 });
    match operator {
        op::ADD => Some(fin(a + b)),
        op::SUB => Some(fin(a - b)),
        op::MUL => Some(fin(a * b)),
        op::DIV => Some(if b == 0.0 { Float(0.0) } else { fin(a / b) }),
        op::REM => Some(if b == 0.0 { Float(0.0) } else { fin(a % b) }),
        op::EQ => Some(Bool(a == b)),
        op::NE => Some(Bool(a != b)),
        op::LT => Some(Bool(a < b)),
        op::LE => Some(Bool(a <= b)),
        op::GT => Some(Bool(a > b)),
        op::GE => Some(Bool(a >= b)),
        _ => None,
    }
}

/// Fold a constant-literal expression to a [`Literal`] (used for var/array
/// initial values). Returns `None` for anything that isn't a compile-time
/// constant. Shared with the type checker so both agree on what's a literal.
///
/// This is the environment-free form. It folds only what is constant on its
/// face — literals, a negated literal, and literal-argument constructors — and
/// deliberately does NOT resolve names or evaluate operators.
///
/// That restraint is load-bearing. This function decides whether a value bakes
/// into a gate's data or gets a real wired gate, and it is used well beyond
/// initializers (port values, buffer delays, handler fields). Folding `a + b`
/// here would silently delete gates from programs that already compile — e.g.
/// `Rotation(0.0 + 0.0, ...)` would collapse to a `_Literal(Rotator)` instead of
/// emitting the `MakeRotation` gate it must. Use [`expr_to_literal_in`] for the
/// initializer paths, where richer constants are wanted.
pub fn expr_to_literal(e: &Expr) -> Option<Literal> {
    expr_to_literal_impl(e, None)
}

/// [`expr_to_literal`], plus named top-level constants and operators over them —
/// so an initializer can read `1 << C_FLAG` or `WIDTH * HEIGHT` instead of a
/// magic number. Only the `var` initializer paths pass an environment;
/// everywhere else keeps the narrower behaviour above.
pub fn expr_to_literal_in(e: &Expr, env: &ConstEnv) -> Option<Literal> {
    expr_to_literal_impl(e, Some(env))
}

/// `env == None` reproduces the original syntactic folding exactly; `Some`
/// additionally resolves named constants and evaluates operators.
fn expr_to_literal_impl(e: &Expr, env: Option<&ConstEnv>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // -- Constant-environment forms: initializers only. --
        // With no environment these fall through to `_ => None`.
        Expr::Ident { name, .. } => env?.get(name).cloned(),
        Expr::BinOp {
            op, left, right, ..
        } if env.is_some() => eval_const_binop(op, expr_to_literal(left)?, expr_to_literal(right)?),
        Expr::UnOp { op, operand, .. }
            if env.is_some() && op != crate::catalog::operators::op::NEG =>
        {
            eval_const_unop(op, expr_to_literal(operand)?)
        }
        // -- Always-constant forms. --
        Expr::IntLit { value, .. } => Some(Literal::Int(*value)),
        Expr::AtomLit { value, .. } => Some(Literal::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Literal::Float(*value)),
        Expr::BoolLit { value, .. } => Some(Literal::Bool(*value)),
        Expr::StringLit { value, .. } => Some(Literal::String(value.clone())),
        // Negative numeric literals: `-5`, `-1.0`.
        Expr::UnOp { op, operand, .. } if op == crate::catalog::operators::op::NEG => {
            match expr_to_literal(operand)? {
                Literal::Int(n) => Some(Literal::Int(n.wrapping_neg())),
                Literal::Float(f) => Some(Literal::Float(-f)),
                _ => None,
            }
        }
        _ => expr_to_literal_lit(e, env),
    }
}

/// Fold a `Vec`/`Rotation`/`Color` builtin constructor call given its
/// ALREADY-EVALUATED argument literals — the pure arity/type-matching law,
/// with no opinion on how those literals were obtained. Split out of
/// `expr_to_literal_lit`'s `Expr::Call` arm (pure extraction, same behavior)
/// so `const_eval::expr::eval_expr`'s own constructor arm can reuse the exact
/// same law after resolving an argument `expr_to_literal_lit` cannot reach
/// (a `const mod` call) — see `const_eval::expr::FOLDABLE_CONSTRUCTORS`'s doc
/// comment, which is the list of constructor names this function accepts.
/// `ColorSRGB` is deliberately absent — it folds via the separate
/// `fold_srgb_color` path, which const evaluation never reaches.
///
/// `args` are in PARAMETER order, not source order — a caller handling the
/// named-argument form (`Vec(z = …, x = …)`) must bind them to the catalog's
/// parameter list FIRST (see `const_eval::expr::bind_constructor_args`).
/// Slot matching here cannot recover a name this function never saw, so
/// handing it source-ordered named arguments silently lands each value on the
/// wrong axis.
pub(crate) fn fold_constructor(name: &str, args: &[Literal]) -> Option<Literal> {
    let mut nums = Vec::with_capacity(args.len());
    for a in args {
        match a {
            Literal::Int(n) => nums.push(*n as f64),
            Literal::Float(f) => nums.push(*f),
            _ => return None,
        }
    }
    match (name, nums.as_slice()) {
        ("Vec", &[x, y, z]) => Some(Literal::Vector { x, y, z }),
        ("Rotation", &[pitch, yaw, roll]) => Some(Literal::Rotator { pitch, yaw, roll }),
        // Color is linear RGBA 0–1; alpha defaults to opaque.
        ("Color", &[r, g, b]) => Some(Literal::LinearColor { r, g, b, a: 1.0 }),
        ("Color", &[r, g, b, a]) => Some(Literal::LinearColor { r, g, b, a }),
        _ => None,
    }
}

/// The constructor / reference cases, split out to keep the dispatch above
/// readable.
fn expr_to_literal_lit(e: &Expr, env: Option<&ConstEnv>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // Constructor calls on constant numeric args fold to literals, so
        // `var v = Vec(1.0, 2.0, 3.0)` (and Rotation/Color) bakes into the
        // gate's initial value instead of being dropped.
        Expr::Call { callee, args, .. } => {
            let Expr::Ident { name, .. } = callee.as_ref() else {
                return None;
            };
            let mut lits = Vec::with_capacity(args.len());
            for a in args {
                let CallArg::Positional(arg) = a else {
                    return None;
                };
                lits.push(expr_to_literal(arg)?);
            }
            fold_constructor(name, &lits)
        }
        // Asset reference `$Type/Name` — inlined into the gate's component data.
        Expr::AssetRef {
            asset_type,
            asset_name,
            ..
        } => Some(Literal::Asset {
            asset_type: asset_type.clone(),
            asset_name: asset_name.clone(),
        }),
        // Prefab file reference `$./file.brz` — inlined; resolved + embedded
        // at emit into the gate's `bundle_path_ref` property.
        Expr::PrefabRef { path, .. } => Some(Literal::PrefabRef { path: path.clone() }),
        // Inline nested-prefab block `$``` ... ``` ` — inlined; compiled +
        // embedded at emit into the gate's `bundle_path_ref` property.
        Expr::NestedPrefab { source, .. } => Some(Literal::NestedPrefab {
            source: source.clone(),
        }),
        _ => None,
    }
}

/// Fold a single array-literal element to a constant [`Literal`]. Spreads have
/// no constant form (they're only valid in exec-context assignments), so they
/// fold to `None` — which makes the all-literal length check fail and the
/// initializer is left empty (the type checker has already reported the error).
///
/// Routes through `const_eval::eval_expr`, which tries the SAME
/// `expr_to_literal_in` fold FIRST (see the seam note atop `const_eval::expr`)
/// — so a plain-literal element still bakes byte-for-byte — and
/// only then reaches for the certified evaluator's extra surface (a const-mod
/// CALL per element, string methods/interpolation, ...) that plain literal
/// folding cannot reach. `ctx` (rather than a bare `&ConstEnv`) is needed to
/// resolve a callee to its `ChipDecl` — via
/// [`resolve_mod_pass1`](LowerCtx::resolve_mod_pass1), NOT `resolve_mod`
/// (every OTHER `const_eval` call site in lowering uses `resolve_mod`, but
/// baking runs during pass 1, before `resolve_mod`'s `ctx.scope` has any
/// chips registered — see `resolve_mod_pass1`'s doc comment).
fn array_elem_literal(el: &ArrayElem, ctx: &LowerCtx) -> Option<Literal> {
    match el {
        ArrayElem::Item(e) => {
            let lookup = |n: &str| ctx.resolve_mod_pass1(n);
            let mut budget = crate::const_eval::Budget::default();
            crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()
        }
        ArrayElem::Spread(_) => None,
    }
}

/// Fold a constant `ColorSRGB(r, g, b, a)` call to a `Literal::Color` — sRGB u8
/// with NO gamma re-encoding (Brickadia brick colours are stored sRGB-direct).
/// Only the four-arg `ColorSRGB` form (0–255 ints, the natural sRGB source) is
/// accepted; anything else — including the linear `Color(..)` constructor,
/// whose 0–1 components would need an ambiguous gamma conversion — folds to
/// `None`, so the caller reports a clean error instead of guessing bytes.
pub(crate) fn fold_srgb_color(e: &Expr) -> Option<Literal> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident { name, .. } = callee.as_ref() else {
        return None;
    };
    if name != "ColorSRGB" {
        return None;
    }
    let mut nums: Vec<i64> = Vec::with_capacity(args.len());
    for a in args {
        let CallArg::Positional(arg) = a else {
            return None;
        };
        match expr_to_literal(arg)? {
            Literal::Int(n) => nums.push(n),
            Literal::Float(f) => nums.push(f as i64),
            _ => return None,
        }
    }
    let [r, g, b, a] = nums.as_slice() else {
        return None;
    };
    let clamp_u8 = |n: i64| n.clamp(0, 255) as u8;
    Some(Literal::Color {
        r: clamp_u8(*r),
        g: clamp_u8(*g),
        b: clamp_u8(*b),
        a: clamp_u8(*a),
    })
}

/// Fold a `meshColors` argument — an array literal of constant `ColorSRGB`
/// colours, or a bare identifier naming a constant already bound to that same
/// `Literal::Array(Literal::Color…)` shape — to `Literal::Array(Literal::Color…)`
/// for a gate's `MeshColors: Color[]` data field. Returns `None` (a clean
/// call-site error) if any element is non-constant or not a `ColorSRGB(..)`,
/// or the named constant isn't bound to a matching array; spreads never fold.
pub(crate) fn fold_mesh_colors(e: &Expr, env: &ConstEnv) -> Option<Literal> {
    if let Expr::Ident { name, .. } = e {
        return match env.get(name) {
            Some(lit @ Literal::Array(items))
                if items.iter().all(|c| matches!(c, Literal::Color { .. })) =>
            {
                Some(lit.clone())
            }
            _ => None,
        };
    }
    let Expr::Array { elements, .. } = e else {
        return None;
    };
    let mut colors = Vec::with_capacity(elements.len());
    for el in elements {
        let ArrayElem::Item(item) = el else {
            return None;
        };
        colors.push(fold_srgb_color(item)?);
    }
    Some(Literal::Array(colors))
}

/// Whether `top` is exactly the `fold_ammo_override` encoding: `[
/// Bool(overrideStartingAmmo), Array[ Array[Int(loaded), Int(reserve)], … ] ]`.
/// Used to validate a bare identifier's resolved constant before accepting it
/// in place of the syntactic `RecordLit` form.
fn is_ammo_override_shape(top: &[Literal]) -> bool {
    matches!(
        top,
        [Literal::Bool(_), Literal::Array(resources)]
            if resources.iter().all(|r| matches!(
                r,
                Literal::Array(pair) if matches!(pair.as_slice(), [Literal::Int(_), Literal::Int(_)])
            ))
    )
}

/// Fold an `ammoOverride` argument — a record literal `{ overrideStartingAmmo:
/// bool, resources: [{ loaded: int, reserve: int }] }`, or a bare identifier
/// naming a constant already bound to that same encoding — for a gate's
/// `WeaponAmmoOverride` nested-struct data field. Encoded in existing `Literal`
/// variants (no new variant is introduced):
/// `Array[ Bool(overrideStartingAmmo), Array[ Array[Int(loaded), Int(reserve)],
/// … ] ]`; the emitter decodes this exact shape. Returns `None` on any
/// non-constant value, unexpected field, or a named constant that isn't bound
/// to a matching array.
pub(crate) fn fold_ammo_override(e: &Expr, env: &ConstEnv) -> Option<Literal> {
    if let Expr::Ident { name, .. } = e {
        return match env.get(name) {
            Some(lit @ Literal::Array(top)) if is_ammo_override_shape(top) => Some(lit.clone()),
            _ => None,
        };
    }
    let Expr::RecordLit { fields, .. } = e else {
        return None;
    };
    let mut override_starting = None;
    let mut resources: Option<Vec<Literal>> = None;
    for f in fields {
        let crate::ast::RecordLitField::Named { name, value, .. } = f else {
            return None;
        };
        match name.as_str() {
            "overrideStartingAmmo" => {
                let Literal::Bool(b) = expr_to_literal(value)? else {
                    return None;
                };
                override_starting = Some(b);
            }
            "resources" => {
                let Expr::Array { elements, .. } = value else {
                    return None;
                };
                let mut rs = Vec::with_capacity(elements.len());
                for el in elements {
                    let ArrayElem::Item(item) = el else {
                        return None;
                    };
                    rs.push(fold_resource_amount(item)?);
                }
                resources = Some(rs);
            }
            _ => return None,
        }
    }
    // Both fields are required. A missing one is a user mistake — reject it (the
    // caller reports WS028 with the expected shape) rather than silently
    // defaulting `overrideStartingAmmo` to false / `resources` to empty. An
    // explicit `resources: []` is still accepted (the key is present).
    Some(Literal::Array(vec![
        Literal::Bool(override_starting?),
        Literal::Array(resources?),
    ]))
}

/// One `{ loaded, reserve }` resource of `ammoOverride.resources`, folded to
/// `Array[Int(loaded), Int(reserve)]` (see [`fold_ammo_override`]).
fn fold_resource_amount(e: &Expr) -> Option<Literal> {
    let Expr::RecordLit { fields, .. } = e else {
        return None;
    };
    let mut loaded = None;
    let mut reserve = None;
    for f in fields {
        let crate::ast::RecordLitField::Named { name, value, .. } = f else {
            return None;
        };
        let Literal::Int(n) = expr_to_literal(value)? else {
            return None;
        };
        match name.as_str() {
            "loaded" => loaded = Some(n),
            "reserve" => reserve = Some(n),
            _ => return None,
        }
    }
    // Both fields are required — a missing `loaded`/`reserve` is rejected (WS028)
    // rather than silently baked as 0.
    Some(Literal::Array(vec![
        Literal::Int(loaded?),
        Literal::Int(reserve?),
    ]))
}

/// Coerce a constant literal to a declared scalar type, matching the gate
/// coercion laws the array bake path uses (string→bool via `!= ""`, and
/// numeric int/float/bool normalization). Identity for anything already the
/// right kind or with no defined coercion.
///
/// Without this, a coercion-mixed map entry (e.g. `Map<int, bool> = { 1 =>
/// "on" }`) would bake its RAW folded literal (`String("on")`), which emit's
/// `wire_map_variant_from_literals` then can't match against the declared
/// value kind and silently zero-falls-back to `false` — a typechecked
/// program baking the wrong data.
fn coerce_literal_to_type(lit: Literal, ty: &Type) -> Literal {
    match ty {
        Type::Int => match lit {
            Literal::Int(n) => Literal::Int(n),
            Literal::Float(f) => Literal::Int(f as i64),
            Literal::Bool(b) => Literal::Int(b as i64),
            other => other,
        },
        Type::Float => match lit {
            Literal::Float(f) => Literal::Float(f),
            Literal::Int(n) => Literal::Float(n as f64),
            Literal::Bool(b) => Literal::Float(b as i64 as f64),
            other => other,
        },
        Type::Bool => match lit {
            Literal::Bool(b) => Literal::Bool(b),
            Literal::Int(n) => Literal::Bool(n != 0),
            Literal::String(s) => Literal::Bool(!s.is_empty()), // the `!= ""` law
            other => other,
        },
        // string / vector / rotator / quat / color / object: no cross-coercion here.
        _ => lit,
    }
}

/// Fold a map-literal entry to a `(key, value)` literal pair coerced to the
/// declared `key_ty`/`val_ty`, or `None` if either side isn't a compile-time
/// constant. Routes through `const_eval::eval_expr` the same way and for the
/// same reason as [`array_elem_literal`] — see its doc comment.
fn map_entry_literal(
    e: &crate::ast::MapLitEntry,
    ctx: &LowerCtx,
    key_ty: &Type,
    val_ty: &Type,
) -> Option<(Literal, Literal)> {
    // See `array_elem_literal`'s doc comment: `resolve_mod_pass1`, not
    // `resolve_mod` — this bakes during pass 1, before `ctx.scope` has any
    // chips registered.
    let lookup = |n: &str| ctx.resolve_mod_pass1(n);
    let cx = ctx.const_ctx(Some(&lookup));
    let key_lit = crate::const_eval::eval_expr(&e.key, &cx, &mut crate::const_eval::Budget::default()).ok()?;
    let val_lit = crate::const_eval::eval_expr(&e.value, &cx, &mut crate::const_eval::Budget::default()).ok()?;
    Some((
        coerce_literal_to_type(key_lit, key_ty),
        coerce_literal_to_type(val_lit, val_ty),
    ))
}

/// Bake a constant map-literal initializer (`var m: Map<K, V> = {...}`) into
/// `properties` as an `InitialValue` (`Literal::Map`) — zero runtime gates,
/// exactly like the array path bakes `Literal::Array`.
/// Shared by [`pre_declare_map`] and the `Map<K, V>` branch of
/// [`pre_declare_var`] since both bake the same way. Non-constant entries
/// can't bake at a (pure) decl: the map starts empty and a warning is
/// raised (lowering handles the exec-context desugar for `m = {…}`).
///
/// `key_ty`/`val_ty` are the declared `Map<K, V>` types — every entry is
/// coerced to them at fold time (see [`coerce_literal_to_type`]) so the baked
/// `Literal::Map` is already correct, not a raw literal emit has to guess at.
fn bake_map_init(
    ctx: &mut LowerCtx,
    properties: &mut HashMap<crate::intern::Sym, Literal>,
    name: &str,
    init: &Option<Expr>,
    key_ty: &Type,
    val_ty: &Type,
) {
    let Some(Expr::MapLit { entries, .. }) = init else {
        return;
    };
    // Object/asset-family keys have no literal representation — `key_of`
    // bakes every one as `Object(None)`, so two or more entries would
    // collapse onto the same null key (a corrupt map with duplicate keys).
    // Fall to the non-constant path instead: warn + start empty.
    if matches!(
        key_ty,
        Type::Entity | Type::Character | Type::Controller
    ) {
        ctx.warn(
            format!(
                "'{name}' initializer has object/asset-typed keys, which can't bake as literals — it starts empty; assign entries inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        );
        return;
    }
    let pairs: Vec<(Literal, Literal)> = entries
        .iter()
        .filter_map(|en| map_entry_literal(en, ctx, key_ty, val_ty))
        .collect();
    if pairs.len() == entries.len() {
        properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
    } else {
        ctx.warn(
            format!(
                "'{name}' initializer has non-constant entries — they are dropped here; assign them inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        );
    }
}

/// A `var` initializer that can't bake into the gate as a constant: returns it
/// for diagnosis. `None` = no initializer, or it bakes fine. Takes the whole
/// `ctx` (not just `&ctx.const_env`) so the array branch can route through
/// `array_elem_literal`'s `const_eval` check — otherwise this would disagree
/// with what `pre_declare_var` actually baked, and warn about an initializer
/// that baked just fine (e.g. a const-mod call per element).
fn var_init_unbaked<'a>(v: &'a VarDecl, ctx: &LowerCtx) -> Option<&'a Expr> {
    let init = v.init.as_ref()?;
    let unbaked = match init {
        Expr::Array { elements, .. } => elements
            .iter()
            .any(|el| array_elem_literal(el, ctx).is_none()),
        // A map literal is baked — and any non-bakeable case (object keys,
        // non-constant entries) is warned — by `bake_map_init`, the single
        // authority on map-init diagnostics. A constant `{ "k": v }` bakes as a
        // `Literal::Map` InitialValue, so it is NOT unbaked; never double-report
        // it here with the generic "not a compile-time constant" message.
        Expr::MapLit { .. } => false,
        // A record-literal initializer bakes PER FIELD into each backing
        // `Pseudo_Var`'s InitialValue (see `record_field_storage`), so the whole
        // record is never "unbaked" — the generic scalar warning would be a
        // false positive (there is no single gate to bake it into).
        Expr::RecordLit { .. } => false,
        // `null` bakes to the var's type default (the `or_else` fallback below),
        // which IS its value — never unbaked.
        Expr::NullLit { .. } => false,
        // An enum-typed initializer bakes into `__disc` + any foldable payload
        // slots (see `declare_enum_container`), the same "no single gate to
        // bake it into" reasoning as a record literal just above - the generic
        // scalar warning below would be a false positive even when a payload
        // arg doesn't itself fold (the discriminant still bakes).
        e if matches!(ctx.type_of(e), Type::Enum { .. }) => false,
        e => expr_to_literal_in(e, &ctx.const_env).is_none(),
    };
    unbaked.then_some(init)
}

/// Warn when a `var` initializer is silently dropped: it can't bake into the
/// Variable gate as a constant, and no exec-context reset will apply it (the
/// var is in pure position, or is `static`, which skips the per-entry reset) —
/// so the var starts at its type default. `skip_array_inits` avoids
/// double-reporting top-level array literals the type checker already errors
/// on.
pub(super) fn warn_unbaked_var_init(ctx: &mut LowerCtx, v: &VarDecl, skip_array_inits: bool) {
    let Some(init) = var_init_unbaked(v, ctx) else {
        return;
    };
    if skip_array_inits && matches!(init, Expr::Array { .. }) {
        return;
    }
    let msg = if v.is_static {
        format!(
            "'static var {}' initializer must be a compile-time constant — this value is dropped and the var starts at its type default",
            v.name
        )
    } else {
        format!(
            "'var {}' initializer is not a compile-time constant — outside an exec context it is dropped and the var starts at its type default; assign the value inside an exec handler instead",
            v.name
        )
    };
    ctx.warn(msg, init.range());
}

/// Build the backing `Pseudo_ArrayVar` gate for `name` and bind the name as a
/// `VarStorage::Array`.
///
/// THE array-var construction: [`pre_declare_array`], [`pre_declare_var`]'s
/// `T[]` branch and [`materialize_const_container`] all route through it, so
/// the gate class, the `ArrayVarRef` port's type and the `Binding::Var` record
/// cannot drift apart between the sites that create one. `properties` is the
/// caller's (label, and `InitialValue` when it has constant contents to bake).
fn declare_array_var(
    ctx: &mut LowerCtx,
    name: &str,
    elem_type: Type,
    properties: HashMap<crate::intern::Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    let node_id = make_array_var_gate(ctx, &elem_type, properties, range);
    ctx.scope.insert(
        name,
        Binding::Var(VarRecord {
            node_id,
            inner_type: elem_type,
            get_node_for_handler: None,
            storage: VarStorage::Array,
        }),
    );
    node_id
}

/// Build just the `Pseudo_ArrayVar` gate (no scope binding) — the gate half of
/// [`declare_array_var`], shared with [`record_field_storage`]'s per-field array
/// backing so a record ARRAY field and a top-level array var cannot drift apart.
fn make_array_var_gate(
    ctx: &mut LowerCtx,
    elem_type: &Type,
    properties: HashMap<crate::intern::Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_ARRAY_VAR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::ARRAY_VAR_REF,
                ty: Type::Ref(Box::new(Type::Array(Box::new(elem_type.clone())))),
            }],
        },
        properties,
        note: None,
        ..Default::default()
    })
}

/// Build the backing `Pseudo_MapVar` gate for `name` and bind the name as a
/// `VarStorage::Map` whose `inner_type` carries the whole `Type::Map(K, V)`.
/// The map counterpart of [`declare_array_var`], with the same
/// one-construction-shared-by-every-site rule.
fn declare_map_var(
    ctx: &mut LowerCtx,
    name: &str,
    map_type: Type,
    properties: HashMap<crate::intern::Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    let node_id = make_map_var_gate(ctx, &map_type, properties, range);
    ctx.scope.insert(
        name,
        Binding::Var(VarRecord {
            node_id,
            inner_type: map_type,
            get_node_for_handler: None,
            storage: VarStorage::Map,
        }),
    );
    node_id
}

/// Build just the `Pseudo_MapVar` gate (no scope binding) — the gate half of
/// [`declare_map_var`], shared with [`record_field_storage`]'s per-field map
/// backing (a record MAP field).
fn make_map_var_gate(
    ctx: &mut LowerCtx,
    map_type: &Type,
    properties: HashMap<crate::intern::Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_MAP_VAR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::MAP_VAR_REF,
                ty: Type::Ref(Box::new(map_type.clone())),
            }],
        },
        properties,
        note: None,
        ..Default::default()
    })
}

/// Build just the `Pseudo_Var` scalar gate (no scope binding) — the gate half of
/// [`pre_declare_var`]'s scalar tail, shared with [`record_field_storage`]'s
/// per-field scalar backing.
fn make_scalar_var_gate(
    ctx: &mut LowerCtx,
    inner_type: &Type,
    properties: HashMap<crate::intern::Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(inner_type.clone())),
                },
            ],
        },
        properties,
        note: None,
        ..Default::default()
    })
}

/// The named fields of a record-literal initializer (`= { x: e, y }`), by field
/// name, so [`record_field_storage`] can bake each backing gate's `InitialValue`
/// from the matching sub-expression. Shorthand `{ x }` reads as `x: x`; spreads
/// have no place in a pure decl initializer and are skipped.
fn init_record_fields(init: Option<&Expr>) -> HashMap<String, &Expr> {
    let mut out = HashMap::default();
    let Some(Expr::RecordLit { fields, .. }) = init else {
        return out;
    };
    for f in fields {
        match f {
            RecordLitField::Named { name, value, .. } => {
                out.insert(name.clone(), value);
            }
            // `{ x }` shorthand carries no expression node to fold; the field
            // falls back to its type default here (an explicit `x: x` bakes).
            RecordLitField::Shorthand { .. } | RecordLitField::Spread { .. } => {}
        }
    }
    out
}

/// Build the per-field STORAGE backing for one field of a record-typed
/// variable, recursing for nested records. Each leaf field gets its own gate of
/// the SAME KIND as the record's storage would be (a scalar record var's fields
/// are scalar `Pseudo_Var`s; a record array/map's fields are `Pseudo_ArrayVar`/
/// `Pseudo_MapVar`s — see [`declare_record_container`]). Returns the field's
/// `Binding` (no scope insertion — it lives in the enclosing `Binding::Record`).
///
/// `kind` is the record's own storage kind: `Var` decomposes each field into a
/// per-field variable of the field's type; `Array`/`Map` decompose each SCALAR
/// field into a per-field array/map of that field's type (the parallel-arrays
/// representation). Non-variant leaf fields are rejected by typecheck; this
/// falls back to a scalar gate defensively.
fn record_field_storage(
    ctx: &mut LowerCtx,
    kind: VarStorage,
    label_base: &str,
    field_name: &str,
    field_typ: &TypeExpr,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
) -> Binding {
    let label = format!("{label_base}.{field_name}");
    // A nested record (inline `{ … }` or via an alias) recurses regardless of
    // the record's own storage kind — a record ARRAY of a record field is
    // parallel arrays one level deeper. `record_fields_of` follows aliases at
    // every level, which `resolve_local_type` (empty alias table) would not.
    if let Some(sub_fields) = ctx.record_fields_of(field_typ) {
        let sub_inits = init_record_fields(init);
        let mut fmap = HashMap::default();
        for f in &sub_fields {
            fmap.insert(
                crate::intern::intern(&f.name),
                record_field_storage(
                    ctx,
                    kind,
                    &label,
                    &f.name,
                    &f.typ,
                    sub_inits.get(&f.name).copied(),
                    map_key,
                    range,
                ),
            );
        }
        return Binding::Record(fmap);
    }
    let field_type = ctx.resolve_local_type(field_typ);
    let field_type = &field_type;
    // Taken before the gate-building borrows `ctx` mutably below.
    let consts = ctx.const_lookup();
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    match kind {
        // A record VARIABLE: each field is a per-field container matching the
        // field's own type (scalar/array/map), exactly like a record input port.
        VarStorage::Var => match field_type {
            Type::Array(elem) => {
                let node_id = make_array_var_gate(ctx, elem, properties, range);
                Binding::Var(VarRecord {
                    node_id,
                    inner_type: (**elem).clone(),
                    get_node_for_handler: None,
                    storage: VarStorage::Array,
                })
            }
            Type::Map(k, v) => {
                let map_type = Type::Map(k.clone(), v.clone());
                let node_id = make_map_var_gate(ctx, &map_type, properties, range);
                Binding::Var(VarRecord {
                    node_id,
                    inner_type: map_type,
                    get_node_for_handler: None,
                    storage: VarStorage::Map,
                })
            }
            scalar => {
                let init_lit = init
                    // `_in`, not the bare `expr_to_literal`: an initializer
                    // naming a constant (`var h: float = K`) is an `Ident`, and
                    // without the const env it resolved to nothing and the var
                    // silently started at its type default.
                    .and_then(|e| expr_to_literal_in(e, &consts))
                    .map(|lit| bake_literal_for_type(lit, scalar))
                    .or_else(|| default_literal_for_var_type(scalar));
                if let Some(lit) = init_lit {
                    properties.insert(*sym::INITIAL_VALUE, lit);
                }
                let node_id = make_scalar_var_gate(ctx, scalar, properties, range);
                Binding::Var(VarRecord {
                    node_id,
                    inner_type: scalar.clone(),
                    get_node_for_handler: None,
                    storage: VarStorage::Var,
                })
            }
        },
        // A record ARRAY: each scalar field backs onto its own parallel array of
        // that field's element type. (Container-typed fields inside an array
        // element are rejected by typecheck — no array-of-arrays.)
        VarStorage::Array => {
            let node_id = make_array_var_gate(ctx, field_type, properties, range);
            Binding::Var(VarRecord {
                node_id,
                inner_type: field_type.clone(),
                get_node_for_handler: None,
                storage: VarStorage::Array,
            })
        }
        // A record MAP: each scalar field backs onto its own parallel map from
        // the SAME key type to that field's value type.
        VarStorage::Map => {
            let key = map_key.cloned().unwrap_or(Type::Any);
            let map_type = Type::Map(Box::new(key), Box::new(field_type.clone()));
            let node_id = make_map_var_gate(ctx, &map_type, properties, range);
            Binding::Var(VarRecord {
                node_id,
                inner_type: map_type,
                get_node_for_handler: None,
                storage: VarStorage::Map,
            })
        }
        VarStorage::Buffer => {
            let node_id = make_scalar_var_gate(ctx, field_type, properties, range);
            Binding::Var(VarRecord {
                node_id,
                inner_type: field_type.clone(),
                get_node_for_handler: None,
                storage: VarStorage::Var,
            })
        }
    }
}

/// The value expression of field `name` in a record literal `row`; `None` if
/// `row` isn't a record literal or lacks the field (shorthand `{ x }` has no
/// expression to fold, so the field falls back to its default).
fn record_lit_field_expr<'a>(row: &'a Expr, name: &str) -> Option<&'a Expr> {
    let Expr::RecordLit { fields, .. } = row else {
        return None;
    };
    fields.iter().find_map(|f| match f {
        RecordLitField::Named { name: n, value, .. } if n == name => Some(value),
        _ => None,
    })
}

/// Fold an expression to a constant [`Literal`] through the certified evaluator,
/// resolving `const mod` calls via the pass-1 lookup (same seam as
/// [`array_elem_literal`]).
fn fold_const(ctx: &LowerCtx, e: &Expr) -> Option<Literal> {
    let lookup = |n: &str| ctx.resolve_mod_pass1(n);
    let mut budget = crate::const_eval::Budget::default();
    crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()
}

/// Set a container gate's `InitialValue` property.
fn set_initial_value(ctx: &mut LowerCtx, node_id: NodeId, value: Literal) {
    if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(*sym::INITIAL_VALUE, value);
    }
}

/// Bake the per-field columns of a record ARRAY literal into each backing array's
/// `InitialValue`: `var foos: Foo[] = [{foo:1}, {foo:3}]` bakes `[1, 3]` into the
/// `foo` array (recursing for nested record fields). A field whose column isn't
/// fully constant is left empty (the array simply starts short that field's data;
/// the whole-row literal path is a pure decl and can't emit runtime writes).
fn bake_record_array_columns(
    ctx: &mut LowerCtx,
    rec: &HashMap<crate::intern::Sym, Binding>,
    rows: &[&Expr],
) {
    for (fkey, binding) in rec {
        let fname = crate::intern::resolve(*fkey);
        let cols: Option<Vec<&Expr>> = rows.iter().map(|r| record_lit_field_expr(r, &fname)).collect();
        let Some(cols) = cols else {
            continue;
        };
        match binding {
            Binding::Record(sub) => bake_record_array_columns(ctx, sub, &cols),
            Binding::Var(v) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let node_id = v.node_id;
                let lits: Vec<Literal> = cols
                    .iter()
                    .filter_map(|e| fold_const(ctx, e))
                    .map(|l| bake_string_bool(l, &elem))
                    .collect();
                if lits.len() == cols.len() {
                    set_initial_value(ctx, node_id, Literal::Array(lits));
                }
            }
            _ => {}
        }
    }
}

/// Bake the per-field entries of a record MAP literal into each backing map's
/// `InitialValue`: `var m: Map<int, Point> = { 0 => {x:1, y:2} }` bakes `{0: 1}`
/// into the `x` map and `{0: 2}` into `y` (recursing for nested record fields).
/// `entries` is `(keyExpr, valueRecordExpr)` per entry.
fn bake_record_map_pairs(
    ctx: &mut LowerCtx,
    rec: &HashMap<crate::intern::Sym, Binding>,
    entries: &[(&Expr, &Expr)],
) {
    for (fkey, binding) in rec {
        let fname = crate::intern::resolve(*fkey);
        let cols: Option<Vec<(&Expr, &Expr)>> = entries
            .iter()
            .map(|(k, v)| record_lit_field_expr(v, &fname).map(|fe| (*k, fe)))
            .collect();
        let Some(cols) = cols else {
            continue;
        };
        match binding {
            Binding::Record(sub) => bake_record_map_pairs(ctx, sub, &cols),
            Binding::Var(v) if v.storage == VarStorage::Map => {
                let (key_ty, val_ty) = match &v.inner_type {
                    Type::Map(k, vv) => ((**k).clone(), (**vv).clone()),
                    _ => (Type::Any, Type::Any),
                };
                let node_id = v.node_id;
                let pairs: Vec<(Literal, Literal)> = cols
                    .iter()
                    .filter_map(|(k, ve)| {
                        Some((
                            coerce_literal_to_type(fold_const(ctx, k)?, &key_ty),
                            coerce_literal_to_type(fold_const(ctx, ve)?, &val_ty),
                        ))
                    })
                    .collect();
                if pairs.len() == cols.len() {
                    set_initial_value(ctx, node_id, Literal::Map(pairs));
                }
            }
            _ => {}
        }
    }
}

/// Bake a record ARRAY initializer (`= [{..}, ..]`) into the per-field arrays,
/// once the `Binding::Record` for `name` already exists. Spreads have no constant
/// form, so an initializer containing one bakes nothing (starts empty).
fn bake_record_array_init(ctx: &mut LowerCtx, name: &str, elements: &[ArrayElem]) {
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    let rows: Vec<&Expr> = elements
        .iter()
        .filter_map(|el| match el {
            ArrayElem::Item(e) => Some(e),
            ArrayElem::Spread(_) => None,
        })
        .collect();
    if rows.len() != elements.len() {
        return;
    }
    bake_record_array_columns(ctx, &rec, &rows);
}

/// Bake a record MAP initializer (`= { k => {..} }`) into the per-field maps.
fn bake_record_map_init(ctx: &mut LowerCtx, name: &str, entries: &[crate::ast::MapLitEntry]) {
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    let pairs: Vec<(&Expr, &Expr)> = entries.iter().map(|en| (&en.key, &en.value)).collect();
    bake_record_map_pairs(ctx, &rec, &pairs);
}

/// Decompose a record-typed storage declaration (`var p: Rec`, `var a: Rec[]`,
/// `var m: Map<K, Rec>`) into one `Binding::Record` of per-field backing gates,
/// and bind it in scope under `name`. `kind` selects the per-field backing kind;
/// `map_key` carries the map's key type for the `Map` case.
fn declare_record_container(
    ctx: &mut LowerCtx,
    name: &str,
    kind: VarStorage,
    fields: &[crate::ast::RecordTypeField],
    label_base: &str,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
) {
    let record_fields = build_record_fields(ctx, kind, fields, label_base, init, map_key, range);
    ctx.scope.insert(name, Binding::Record(record_fields));
}

/// The per-field storage `Binding::Record` of a record var/param - the shared
/// body of [`declare_record_container`] (which then inserts it into scope) and
/// the multi-return-mod record storage (`lower::call::inline`, which holds it as
/// the return value rather than a scope binding).
pub(super) fn build_record_fields(
    ctx: &mut LowerCtx,
    kind: VarStorage,
    fields: &[crate::ast::RecordTypeField],
    label_base: &str,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let sub_inits = init_record_fields(init);
    let mut record_fields = HashMap::default();
    for f in fields {
        record_fields.insert(
            crate::intern::intern(&f.name),
            record_field_storage(
                ctx,
                kind,
                label_base,
                &f.name,
                &f.typ,
                sub_inits.get(&f.name).copied(),
                map_key,
                range,
            ),
        );
    }
    record_fields
}

/// A construction expression (`Dir.E`, `Shape.Circle(5.0)`,
/// `Box.Dims { w: 1.0, h: 2.0 }`) whose variant - and any LITERAL-foldable
/// payload arg - is known at pre-declare time, i.e. before any exec chain
/// exists to wire a live construction through. Used only to bake a stored
/// enum var's initializer (`declare_enum_container`); the general/live
/// construction path (an assignment inside an `on` handler, a `let`, ...) goes
/// through `lower_enum_ctor` in `lower/expr.rs` instead.
pub(super) struct StaticEnumCtor {
    variant: String,
    disc: i64,
    /// `(slot key, literal value)` for every payload arg that folded - a
    /// positional arg's key is its index ("0", "1", ...), a named field's key is
    /// its own name. An arg that didn't fold is simply absent, not an error:
    /// the discriminant alone is still statically known from the variant
    /// name, so the tag bakes even when a payload value doesn't.
    slots: Vec<(String, Literal)>,
}

/// Recognize `e` as a construction of `enum_name` with a statically-known
/// variant, per [`StaticEnumCtor`]. `None` for anything else (a non-constant
/// enum-typed expression, a different enum, a plain identifier, ...) - general
/// enum const-eval is a later task; this is deliberately narrow.
fn static_enum_ctor(ctx: &LowerCtx, enum_name: &str, e: &Expr) -> Option<StaticEnumCtor> {
    let def = ctx.enum_defs.get(enum_name)?;
    // `Enum.Variant` matches `enum_name` - every construction form starts
    // with this same `obj.field` shape (bare for unit, as a call callee for
    // positional, as a VariantCtor path for named).
    let is_enum_obj =
        |obj: &Expr| matches!(obj, Expr::Ident { name, .. } if name == enum_name);
    match e {
        Expr::FieldAccess { obj, field: variant, .. } if is_enum_obj(obj) => {
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots: Vec::new(),
            })
        }
        // Bare unit-variant reference (`None` for `Option.None`): valid
        // whenever `variant` names one of `enum_name`'s OWN variants and
        // isn't shadowed by a scope symbol of the same name. Typecheck
        // already refused an ambiguous/shadowed bare name outright by the
        // time lowering runs (`typecheck::infer::resolve_bare_variant_enum`),
        // so this only needs to confirm membership in the ALREADY-KNOWN
        // target enum (from the var's own declared type) - not re-derive
        // global uniqueness across every registered enum the way the general
        // live-construction path (`lower::expr::resolve_bare_variant_enum`)
        // must.
        Expr::Ident { name: variant, .. } if ctx.scope.get(variant).is_none() => {
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots: Vec::new(),
            })
        }
        Expr::Call { callee, args, .. } => {
            let variant = match callee.as_ref() {
                Expr::FieldAccess { obj, field: variant, .. } if is_enum_obj(obj) => variant,
                // Bare positional construction (`Some(42)` for
                // `Option.Some(42)`) - same shadow/membership check as the
                // bare unit-reference arm above.
                Expr::Ident { name: variant, .. } if ctx.scope.get(variant).is_none() => variant,
                _ => return None,
            };
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            let mut slots = Vec::new();
            let mut i = 0usize;
            for a in args {
                if let CallArg::Positional(v) = a {
                    if let Some(lit) = fold_const(ctx, v) {
                        slots.push((i.to_string(), lit));
                    }
                    i += 1;
                }
            }
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots,
            })
        }
        Expr::VariantCtor { path, fields, .. } => {
            let Expr::FieldAccess { obj, field: variant, .. } = path.as_ref() else {
                return None;
            };
            if !is_enum_obj(obj) {
                return None;
            }
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            let mut slots = Vec::new();
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        if let Some(lit) = fold_const(ctx, value) {
                            slots.push((name.clone(), lit));
                        }
                    }
                    RecordLitField::Shorthand { name, range } => {
                        let ident = Expr::Ident {
                            name: name.clone(),
                            range: range.clone(),
                        };
                        if let Some(lit) = fold_const(ctx, &ident) {
                            slots.push((name.clone(), lit));
                        }
                    }
                    RecordLitField::Spread { .. } => {}
                }
            }
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots,
            })
        }
        _ => None,
    }
}

/// Reconstruct a [`StaticEnumCtor`] for a NESTED enum value from its already-
/// folded `Literal::Record`. A nested initializer reaches [`enum_payload_slot`]
/// as the folded literal for its slot (`Outer.W(Color.Green)` bakes `Color.Green`
/// to `{__disc: Int(1)}`), not as a surface `Expr`, so this reads the tag back
/// out of `__disc`, recovers the variant name from `def`, and strips the
/// `__<variant>_` prefix off every payload field to recover its slot key - the
/// inverse of what [`build_enum_fields`]/const-eval's construction fold write.
/// `None` when the record has no readable `Int` `__disc` or the tag matches no
/// variant, so the caller keeps the default first-variant behavior.
fn static_enum_ctor_from_literal(
    def: &crate::typecheck::enums::EnumDef,
    fields: &[(String, Literal)],
) -> Option<StaticEnumCtor> {
    let disc = fields.iter().find_map(|(n, v)| match (n.as_str(), v) {
        ("__disc", Literal::Int(d)) => Some(*d),
        _ => None,
    })?;
    let vdef = def.variants.iter().find(|v| v.discriminant == disc)?;
    let prefix = format!("__{}_", vdef.name);
    let slots = fields
        .iter()
        .filter_map(|(n, v)| n.strip_prefix(&prefix).map(|key| (key.to_string(), v.clone())))
        .collect();
    Some(StaticEnumCtor { variant: vdef.name.clone(), disc, slots })
}

/// Decompose an enum-typed variable (`var d: Dir`) into the SUPERSET
/// `Binding::Record`: a scalar int `__disc` `Pseudo_Var` plus one payload slot
/// gate per (variant, field) across EVERY variant of `enum_name` - see the
/// "Enum value layout" doc on `LowerCtx::enum_defs`. `init`, when it's a
/// statically-known constructor ([`static_enum_ctor`]), bakes `__disc` and any
/// literal payload args into `InitialValue`; anything else leaves the
/// defaults (first-variant discriminant, zeroed slots) for a later exec-context
/// assignment to overwrite.
fn declare_enum_container(
    ctx: &mut LowerCtx,
    name: &str,
    enum_name: &str,
    args: &[Type],
    label_base: &str,
    init: Option<&Expr>,
    range: &SourceRange,
) {
    let Some(def) = ctx.enum_defs.get(enum_name).cloned() else {
        // Unknown enum shouldn't happen post-typecheck (or in the isolated
        // resource-estimation `LowerCtx`, which seeds `enum_defs` empty) -
        // fall back to a bare scalar so lowering doesn't panic.
        let node_id = make_scalar_var_gate(ctx, &Type::Int, HashMap::default(), range);
        ctx.scope.insert(
            name,
            Binding::Var(VarRecord {
                node_id,
                inner_type: Type::Int,
                get_node_for_handler: None,
                storage: VarStorage::Var,
            }),
        );
        return;
    };
    let known = init.and_then(|e| static_enum_ctor(ctx, enum_name, e));
    let fields = build_enum_fields(ctx, &def, args, label_base, known.as_ref(), None, range);
    ctx.scope.insert(name, Binding::Record(fields));
}

/// Build the `__disc` + per-(variant, field) payload-slot bindings for one
/// enum value - the shared body of [`declare_enum_container`] (a top-level
/// enum `var`) and [`enum_payload_slot`]'s nested-enum recursion. `known`, when
/// present, bakes its discriminant into `__disc` and its foldable payload args
/// into their matching slots; absent it, `__disc` defaults to the enum's FIRST
/// variant (matching how a plain scalar var's `InitialValue` defaults to the
/// type's zero value) and every slot is zeroed.
///
/// `disc_override`, when present, binds `__disc` DIRECTLY to that value port (a
/// `Binding::Local`) instead of a defaulted `Pseudo_Var` - the runtime-disc
/// path `Enum.FromInt(n)` uses to wire the lowered `n` in while leaving every
/// payload slot at its zero default (see `lower::expr::try_lower_enum_from_int`).
/// It is mutually exclusive with `known` (a `FromInt` value has no static
/// constructor), so a caller passes at most one.
pub(super) fn build_enum_fields(
    ctx: &mut LowerCtx,
    def: &crate::typecheck::enums::EnumDef,
    args: &[Type],
    label_base: &str,
    known: Option<&StaticEnumCtor>,
    disc_override: Option<PortRef>,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    use crate::typecheck::enums::Payload;
    let mut fields = HashMap::default();

    let disc_binding = if let Some(port) = disc_override {
        Binding::Local(LocalRecord { port })
    } else {
        let mut disc_props = HashMap::default();
        disc_props.insert(
            *sym::NAME_LABEL,
            Literal::String(format!("{label_base}.__disc")),
        );
        let disc_init = known
            .map(|k| k.disc)
            .or_else(|| def.variants.first().map(|v| v.discriminant))
            .unwrap_or(0);
        disc_props.insert(*sym::INITIAL_VALUE, Literal::Int(disc_init));
        let disc_node = make_scalar_var_gate(ctx, &Type::Int, disc_props, range);
        Binding::Var(VarRecord {
            node_id: disc_node,
            inner_type: Type::Int,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        })
    };
    fields.insert(crate::intern::intern("__disc"), disc_binding);

    for v in &def.variants {
        let slot_init = |slot_key: &str| -> Option<Literal> {
            known
                .filter(|k| k.variant == v.name)
                .and_then(|k| k.slots.iter().find(|(s, _)| s == slot_key))
                .map(|(_, lit)| lit.clone())
        };
        match &v.payload {
            Payload::Unit => {}
            Payload::Positional(types) => {
                for (i, te) in types.iter().enumerate() {
                    let slot_key = i.to_string();
                    let slot_name = format!("__{}_{}", v.name, slot_key);
                    let init_lit = slot_init(&slot_key);
                    let label = format!("{label_base}.{slot_name}");
                    fields.insert(
                        crate::intern::intern(&slot_name),
                        enum_payload_slot(ctx, te, &def.type_params, args, &label, init_lit, range),
                    );
                }
            }
            Payload::Named(nfields) => {
                for (fname, te) in nfields {
                    let slot_name = format!("__{}_{}", v.name, fname);
                    let init_lit = slot_init(fname);
                    let label = format!("{label_base}.{slot_name}");
                    fields.insert(
                        crate::intern::intern(&slot_name),
                        enum_payload_slot(ctx, te, &def.type_params, args, &label, init_lit, range),
                    );
                }
            }
        }
    }
    fields
}

/// Resolve one enum payload field's declared type THROUGH the enclosing enum's
/// generic instantiation: a bare type parameter (`Wrap<Color>`'s `W(T)`) becomes
/// the corresponding instantiation argument (`Color`), so its slot lays out as
/// that argument's storage - a nested enum record, not a scalar. Any other type
/// resolves normally and then has the enum's parameters substituted through it
/// (a nested generic-enum payload keeps its own arguments). Non-generic enums
/// pass `type_params`/`args` empty and this is exactly `resolve_local_type`.
fn resolve_payload_slot_type(
    ctx: &LowerCtx,
    field_typ: &TypeExpr,
    type_params: &[TypeParam],
    args: &[Type],
) -> Type {
    if let TypeExpr::Name { name, .. } = field_typ
        && let Some(idx) = type_params.iter().position(|p| &p.name == name)
    {
        return args.get(idx).cloned().unwrap_or(Type::Any);
    }
    let resolved = ctx.resolve_local_type(field_typ);
    if type_params.is_empty() || args.is_empty() {
        return resolved;
    }
    let subst: crate::types::infer::Subst = type_params
        .iter()
        .zip(args)
        .map(|(p, a)| (p.name.clone(), a.clone()))
        .collect();
    crate::types::mono::substitute(&resolved, &subst)
}

/// The storage backing for ONE enum payload slot: a nested enum recurses into
/// its own `__disc` + slots superset ([`build_enum_fields`]); a nested plain
/// record decomposes per-field (mirrors [`record_field_storage`]'s
/// `VarStorage::Var` arm); an array/map field gets its own container gate;
/// anything else is a scalar `Pseudo_Var`, baked from `init_lit` when given.
/// Always `VarStorage::Var`-shaped - an enum payload slot is never itself a
/// parallel-array/map column the way a record ARRAY/MAP field is. `type_params`
/// / `args` are the enclosing enum's instantiation, so a payload whose type is
/// one of those parameters resolves to the concrete argument (an enum-typed
/// `Wrap<Color>` payload lays out as a nested Color record, not a scalar).
fn enum_payload_slot(
    ctx: &mut LowerCtx,
    field_typ: &TypeExpr,
    type_params: &[TypeParam],
    args: &[Type],
    label: &str,
    init_lit: Option<Literal>,
    range: &SourceRange,
) -> Binding {
    let field_type = resolve_payload_slot_type(ctx, field_typ, type_params, args);
    if let Type::Enum { name: nested_name, args: nested_args } = &field_type
        && let Some(nested_def) = ctx.enum_defs.get(nested_name).cloned()
    {
        // A nested enum initializer folds to a `Literal::Record` for THIS slot
        // (`Outer.W(Color.Green)` -> `{__disc: Int(1)}`); recover its variant so
        // the inner `__disc`/slots bake from the constructed variant rather than
        // defaulting to the nested enum's first variant.
        let nested_known = match &init_lit {
            Some(Literal::Record(rf)) => static_enum_ctor_from_literal(&nested_def, rf),
            _ => None,
        };
        let fields = build_enum_fields(
            ctx,
            &nested_def,
            nested_args,
            label,
            nested_known.as_ref(),
            None,
            range,
        );
        return Binding::Record(fields);
    }
    if let Some(sub_fields) = ctx.record_fields_of(field_typ) {
        let mut fmap = HashMap::default();
        for f in &sub_fields {
            fmap.insert(
                crate::intern::intern(&f.name),
                record_field_storage(ctx, VarStorage::Var, label, &f.name, &f.typ, None, None, range),
            );
        }
        return Binding::Record(fmap);
    }
    let mut props = HashMap::default();
    props.insert(*sym::NAME_LABEL, Literal::String(label.to_string()));
    match &field_type {
        Type::Array(elem) => {
            let node_id = make_array_var_gate(ctx, elem, props, range);
            Binding::Var(VarRecord {
                node_id,
                inner_type: (**elem).clone(),
                get_node_for_handler: None,
                storage: VarStorage::Array,
            })
        }
        Type::Map(k, v) => {
            let map_type = Type::Map(k.clone(), v.clone());
            let node_id = make_map_var_gate(ctx, &map_type, props, range);
            Binding::Var(VarRecord {
                node_id,
                inner_type: map_type,
                get_node_for_handler: None,
                storage: VarStorage::Map,
            })
        }
        scalar => {
            if let Some(lit) = init_lit {
                props.insert(*sym::INITIAL_VALUE, bake_literal_for_type(lit, scalar));
            } else if let Some(lit) = default_literal_for_var_type(scalar) {
                props.insert(*sym::INITIAL_VALUE, lit);
            }
            let node_id = make_scalar_var_gate(ctx, scalar, props, range);
            Binding::Var(VarRecord {
                node_id,
                inner_type: scalar.clone(),
                get_node_for_handler: None,
                storage: VarStorage::Var,
            })
        }
    }
}

/// Give a `const` array/map its RUNTIME form: the same container gate a `var`
/// would get, with the constant contents baked into `InitialValue`, bound in
/// scope so every ordinary container path (index read, method call, a
/// `T[]`/`Map<K,V>` argument) resolves it exactly like a `var`.
///
/// LAZY, and that is the point. It is called only from those runtime paths, so
/// a `const` container used ONLY at compile time (`const t = [10, 20, 30]` with
/// `const z = t[1]`) still emits no gate at all. The SCOPE BINDING is the memo:
/// the second runtime read finds it through `lookup_var` and never reaches
/// here, so N reads of one const table share one gate. The binding lives in
/// whichever frame is open at the first runtime use, so a `const` container
/// inside a `mod`/`chip` body materializes per inline instance exactly as a
/// `var` there does.
///
/// Returns `None` — changing nothing — unless `obj` is a bare name that is
/// `const`-declared, holds an array/map literal, and has NO binding of its own
/// yet. That last condition is what keeps a real container of the same name
/// winning: this can only ever fill a hole, never shadow. An already
/// materialized name simply returns its existing record.
///
/// The new node joins `ctx.immutable_containers`; see that field's doc comment
/// for why mutation has to stay rejected.
pub(super) fn materialize_const_container(
    ctx: &mut LowerCtx,
    obj: &Expr,
) -> Option<VarRecord> {
    let Expr::Ident { name, range } = obj else {
        return None;
    };
    if let Some(rec) = ctx.lookup_var(name) {
        return matches!(rec.storage, VarStorage::Array | VarStorage::Map).then(|| rec.clone());
    }
    if ctx.scope.get(name).is_some() {
        return None;
    }
    let lit = ctx.const_container_literal(name)?;
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(name.clone()));
    // Prefer the type CHECKED for this very read over one re-derived from the
    // literal: typecheck's symbol type is what picked the method signature and
    // the element/value types the call was validated against, so taking any
    // other answer here would let the gate's port types disagree with the
    // signature the program was accepted under. The literal is the fallback for
    // a read typecheck recorded nothing for.
    let checked = unwrap_ref(&ctx.type_of(obj));
    match lit {
        Literal::Array(items) => {
            let elem_type = match checked {
                Type::Array(e) => *e,
                _ => items
                    .first()
                    .and_then(wire_type_of_literal)
                    .unwrap_or(Type::Any),
            };
            // Element-wise compile-time string → bool, the same `!= ""` law the
            // `var` paths bake through (see `bake_string_bool`).
            let items: Vec<Literal> = items
                .into_iter()
                .map(|lit| bake_string_bool(lit, &elem_type))
                .collect();
            properties.insert(*sym::INITIAL_VALUE, Literal::Array(items));
            let node_id = declare_array_var(ctx, name, elem_type, properties, range);
            ctx.immutable_containers.insert(node_id);
        }
        Literal::Map(pairs) => {
            let (key_ty, val_ty) = match checked {
                Type::Map(k, v) => (*k, *v),
                _ => match pairs.first() {
                    Some((k, v)) => (
                        wire_type_of_literal(k).unwrap_or(Type::Any),
                        wire_type_of_literal(v).unwrap_or(Type::Any),
                    ),
                    None => (Type::Any, Type::Any),
                },
            };
            // Coerce every entry to the map's own K/V exactly as `bake_map_init`
            // does, so the baked `Literal::Map` is already correct rather than
            // something emit has to guess at.
            let pairs: Vec<(Literal, Literal)> = pairs
                .into_iter()
                .map(|(k, v)| {
                    (
                        coerce_literal_to_type(k, &key_ty),
                        coerce_literal_to_type(v, &val_ty),
                    )
                })
                .collect();
            properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
            let map_type = Type::Map(Box::new(key_ty), Box::new(val_ty));
            let node_id = declare_map_var(ctx, name, map_type, properties, range);
            ctx.immutable_containers.insert(node_id);
        }
        _ => return None,
    }
    ctx.lookup_var(name).cloned()
}

/// The VALUE type-expression of a `Map<K, V>` annotation (following an outer
/// `*Map<..>` ref), so a record-valued map decomposes through the alias-aware
/// `record_fields_of`. `None` for anything that isn't a `Map<_, _>`.
fn map_value_typeexpr(te: Option<&TypeExpr>) -> Option<&TypeExpr> {
    match te? {
        TypeExpr::Generic { name, args, .. } if name == "Map" && args.len() == 2 => Some(&args[1]),
        TypeExpr::Ref { inner, .. } => map_value_typeexpr(Some(inner)),
        _ => None,
    }
}

pub(super) fn pre_declare_var(ctx: &mut LowerCtx, d: &VarDecl) {
    // `resolve_local_type` monomorphizes a `T` annotation inside a generic mod
    // body (and is identical to `type_of_type_expr` everywhere else).
    let inner_type = d
        .typ
        .as_ref()
        .map(|te| ctx.resolve_local_type(te))
        .or_else(|| d.init.as_ref().map(|e| ctx.type_of(e)))
        .unwrap_or(Type::Any);

    // `var foo: T[]` is an array — desugar to an ArrayVar gate so the array
    // methods actually work. A `= [..]` initializer carries its constant
    // literals inline (mirrors the map path below).
    if let Type::Array(elem) = &inner_type {
        // `var pts: Rec[]` — a record ARRAY decomposes into one parallel array
        // per field (`pts.x`, `pts.y`), bound as a `Binding::Record` of
        // `VarStorage::Array` fields. Every element op fans out across them (see
        // `lower_record_array_method`). `record_fields_of` follows a `type P = {…}`
        // alias on the element the resolved `Type` would not preserve.
        if let Some(TypeExpr::Array { inner: elem_te, .. }) = d.typ.as_ref()
            && let Some(fields) = ctx.record_fields_of(elem_te)
        {
            let label =
                resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
                    .unwrap_or_else(|| d.name.clone());
            declare_record_container(
                ctx,
                &d.name,
                VarStorage::Array,
                &fields,
                &label,
                None,
                None,
                &d.range,
            );
            // Bake a constant `= [{..}, ..]` initializer into the per-field arrays.
            if let Some(Expr::Array { elements, .. }) = &d.init {
                bake_record_array_init(ctx, &d.name, elements);
            }
            return;
        }
        let elem_type = elem.as_ref().clone();
        let mut properties = HashMap::default();
        let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
            .unwrap_or_else(|| d.name.clone());
        properties.insert(*sym::NAME_LABEL, Literal::String(label));
        if let Some(Expr::Array { elements, .. }) = &d.init {
            // Element-wise compile-time string → bool for `var v: bool[] =
            // [..]` — same `!= ""` law as the wire path's CompareNotEqual
            // gate (see `bake_string_bool`).
            let lits: Vec<Literal> = elements
                .iter()
                .filter_map(|el| array_elem_literal(el, ctx))
                .map(|lit| bake_string_bool(lit, &elem_type))
                .collect();
            if lits.len() == elements.len() {
                properties.insert(intern_static("InitialValue"), Literal::Array(lits));
            }
        }
        declare_array_var(ctx, &d.name, elem_type, properties, &d.range);
        return;
    }

    // `var m: Map<K, V>` is a map — desugar to a MapVar gate so the map
    // methods work. A constant `= {...}` initializer bakes via
    // `bake_map_init`.
    if let Type::Map(key_ty, value_ty) = &inner_type {
        // `var m: Map<K, Rec>` — a record VALUE decomposes into parallel per-field
        // maps `Map<K, fieldType>`, keyed the same, bound as a `Binding::Record`.
        // Every map op fans out across them (see `lower_record_map_method`).
        if let Some(val_te) = map_value_typeexpr(d.typ.as_ref())
            && let Some(fields) = ctx.record_fields_of(val_te)
        {
            let label =
                resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
                    .unwrap_or_else(|| d.name.clone());
            declare_record_container(
                ctx,
                &d.name,
                VarStorage::Map,
                &fields,
                &label,
                None,
                Some(key_ty),
                &d.range,
            );
            // Bake a constant `= { k => {..} }` initializer into the per-field maps.
            if let Some(Expr::MapLit { entries, .. }) = &d.init {
                bake_record_map_init(ctx, &d.name, entries);
            }
            return;
        }
        let (key_ty, value_ty) = (key_ty.as_ref().clone(), value_ty.as_ref().clone());
        let mut properties = HashMap::default();
        let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
            .unwrap_or_else(|| d.name.clone());
        properties.insert(*sym::NAME_LABEL, Literal::String(label));
        bake_map_init(ctx, &mut properties, &d.name, &d.init, &key_ty, &value_ty);
        declare_map_var(ctx, &d.name, inner_type, properties, &d.range);
        return;
    }

    // `var d: Dir` - an enum-typed variable decomposes into the SUPERSET
    // `__disc` + every variant's payload slots (see `declare_enum_container`
    // and `LowerCtx::enum_defs`'s "Enum value layout" doc), bound as a
    // `Binding::Record` so `d.Discriminant` reads the right backing gate and a
    // constructed variant assigned into `d` writes its slots field-wise. A
    // statically-known constructor initializer (`= Dir.E`) bakes `__disc` (and
    // any literal payload args) into `InitialValue` directly - general enum
    // const-eval is a later task (see `static_enum_ctor`).
    if let Type::Enum { name: enum_name, args } = &inner_type {
        let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
            .unwrap_or_else(|| d.name.clone());
        let args = args.clone();
        declare_enum_container(ctx, &d.name, enum_name, &args, &label, d.init.as_ref(), &d.range);
        return;
    }

    // `var p: Rec` — a record variable decomposes into one per-field `Pseudo_Var`
    // (recursing for nested records), bound as a `Binding::Record` so `p.field`
    // reads/writes the right backing gate. Without this a record var collapsed
    // to a single `Pseudo_Var` and `p.x` lowered to a bogus `SplitVector.X`
    // swizzle — a silent miscompile. Mirrors the record INPUT-PORT expansion in
    // `pre_declare_input`; `record_fields_of` follows a `type P = { … }` alias
    // that `resolve_local_type`'s empty alias table cannot.
    if let Some(fields) = d.typ.as_ref().and_then(|te| ctx.record_fields_of(te)) {
        let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
            .unwrap_or_else(|| d.name.clone());
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Var,
            &fields,
            &label,
            d.init.as_ref(),
            None,
            &d.range,
        );
        return;
    }

    let init_lit = d
        .init
        .as_ref()
        // `_in`, not the bare `expr_to_literal`: an initializer naming a
        // constant (`var h: float = K`) is an `Ident`, and without the const
        // env it resolved to nothing and the var silently started at its type
        // default.
        .and_then(|e| expr_to_literal_in(e, &ctx.const_lookup()))
        // Bake the literal in the DECLARED type's shape: the Var gate's
        // wire variant is picked from the InitialValue's own kind, so
        // `var v: float = 0` must widen to Float(0.0) or it ships an integer
        // variable, and `var v: bool = "x"` bakes Bool(!s.is_empty()) under
        // the same `!= ""` law as the wire path's `CompareNotEqual` gate
        // (see `bake_literal_for_type`).
        .map(|lit| bake_literal_for_type(lit, &inner_type))
        .or_else(|| default_literal_for_var_type(&inner_type));
    let mut properties = HashMap::default();
    let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
        .unwrap_or_else(|| d.name.clone());
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    if let Some(lit) = init_lit {
        properties.insert(*sym::INITIAL_VALUE, lit);
    }

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(inner_type.clone())),
                },
            ],
        },
        properties,
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord {
            node_id,
            inner_type,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        }),
    );
}

pub(super) fn pre_declare_buffer(ctx: &mut LowerCtx, d: &BufferDecl) {
    let annotated = d.typ.as_ref().map(|te| ctx.resolve_local_type(te));
    let rhs_type = ctx.type_of(&d.init);
    let inner_type = annotated.unwrap_or_else(|| unwrap_ref(&rhs_type));

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BUFFER_TICKS,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::INPUT,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::TICKS_TO_WAIT,
                    ty: Type::Int,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: inner_type.clone(),
            }],
        },
        properties: [(*sym::TICKS_TO_WAIT, Literal::Int(1))]
            .into_iter()
            .collect(),
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Buffer(NodeRecord {
            node_id,
            ty: inner_type,
        }),
    );
}

pub(super) fn pre_declare_array(ctx: &mut LowerCtx, d: &ArrayDecl) {
    // A record element decomposes into parallel per-field arrays — same as the
    // `var pts: Rec[]` form (see `pre_declare_var`'s array branch).
    if let Some(fields) = ctx.record_fields_of(&d.element_type) {
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Array,
            &fields,
            &d.name,
            None,
            None,
            &d.range,
        );
        if !d.init.is_empty() {
            bake_record_array_init(ctx, &d.name, &d.init);
        }
        return;
    }
    let elem_type = ctx.resolve_local_type(&d.element_type);
    // Constant initializer (`var foo: int[] = [1, 2, 3]`): every element must
    // be a literal. Carry the values as an `InitialValue` property the emitter
    // writes straight into the ArrayVar's array variant (no runtime gates).
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    if !d.init.is_empty() {
        // Element-wise compile-time string → bool for `var a: bool[] =
        // ["x", ""]` → [true, false] — same `!= ""` law as the wire path's
        // CompareNotEqual gate (see `bake_string_bool`); a raw String
        // element in a Bool array variant would diverge to the gate's
        // native content-aware truthiness at load.
        let lits: Vec<Literal> = d
            .init
            .iter()
            .filter_map(|el| array_elem_literal(el, ctx))
            .map(|lit| bake_string_bool(lit, &elem_type))
            .collect();
        if lits.len() == d.init.len() {
            properties.insert(intern_static("InitialValue"), Literal::Array(lits));
        }
    }
    declare_array_var(ctx, &d.name, elem_type, properties, &d.range);
}

/// `var name: Map<K, V>` — create the backing `Pseudo_MapVar` gate (exposing a
/// `MapVarRef`) and bind the name as a `VarStorage::Map` whose `inner_type`
/// carries the whole `Type::Map(K, V)`. Mirrors [`pre_declare_array`].
pub(super) fn pre_declare_map(ctx: &mut LowerCtx, d: &crate::ast::MapDecl) {
    // A record VALUE decomposes into parallel per-field maps — same as the
    // `var m: Map<K, Rec>` form (see `pre_declare_var`'s map branch).
    if let Some(fields) = ctx.record_fields_of(&d.value_type) {
        let key_type = ctx.resolve_local_type(&d.key_type);
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Map,
            &fields,
            &d.name,
            None,
            Some(&key_type),
            &d.range,
        );
        if let Some(Expr::MapLit { entries, .. }) = &d.init {
            bake_record_map_init(ctx, &d.name, entries);
        }
        return;
    }
    let key_type = ctx.resolve_local_type(&d.key_type);
    let value_type = ctx.resolve_local_type(&d.value_type);
    let map_type = Type::Map(Box::new(key_type.clone()), Box::new(value_type.clone()));
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    bake_map_init(
        ctx,
        &mut properties,
        &d.name,
        &d.init,
        &key_type,
        &value_type,
    );
    declare_map_var(ctx, &d.name, map_type, properties, &d.range);
}

/// Push the WS023 "annotation on a non-root port" diagnostic. Shared so the
/// message text has a single source (apply_port_side and the anon-chip output
/// path both use it).
fn report_non_root_side(ctx: &mut LowerCtx, range: &SourceRange) {
    ctx.diagnostics.push(Diagnostic::error(
        "WS023",
        "side annotations only apply to top-level ports of the compiled file",
        range.clone(),
    ));
}

/// Attach a `@side` annotation to a freshly created I/O node, or reject it
/// with WS023 when the port doesn't belong to the root module (chip/mod
/// bodies, anonymous chips). Also carries the `@invisible` flag onto the
/// same node when the port declared it.
fn apply_port_side(
    ctx: &mut LowerCtx,
    node_id: NodeId,
    side: Option<crate::ast::PortSide>,
    invisible: bool,
    range: &SourceRange,
) {
    if invisible {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*crate::intern::sym::REROUTE_INVISIBLE, Literal::Bool(true));
        }
    }
    let Some(side) = side else { return };
    if !ctx.is_root_module || ctx.current_anon_chip.is_some() {
        report_non_root_side(ctx, range);
        return;
    }
    if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(
            *crate::intern::sym::REROUTE_SIDE,
            Literal::String(side.as_str().to_string()),
        );
    }
}

pub(super) fn pre_declare_input(ctx: &mut LowerCtx, d: &InDecl) {
    // A record-typed input port (inline `{ … }`, a non-generic `type P = { … }`,
    // or a generic `type Pair<T> = { … }` instantiated as `Pair<int>`) dissolves
    // into one sub-port per field, bound as a `Record` so `p.field` reads the
    // right sub-port. Without this a record port collapsed to a single `any`
    // port and its field accesses lowered to `_Unsupported`/swizzle gates —
    // mirrors the standalone-chip input expansion in `lower::mod`.
    if let Some(fields) = ctx.record_fields_of(&d.typ) {
        let mut record_fields = HashMap::default();
        for field in &fields {
            let port_name = format!("{}_{}", d.name, field.name);
            let ft = type_of_type_expr(&field.typ);
            let node_id = ctx.add_input(&port_name, ft.clone(), d.range.clone());
            // Array / Map / ref fields of a record-typed input port bind a
            // container ref-port (see `container_binding`); a scalar field is a
            // plain by-value input.
            let binding = match super::context::container_binding(&field.typ, &ft) {
                Some((storage, inner)) => Binding::Var(VarRecord {
                    node_id,
                    inner_type: inner,
                    get_node_for_handler: None,
                    storage,
                }),
                None => Binding::Input(NodeRecord {
                    node_id,
                    ty: ft.clone(),
                }),
            };
            record_fields.insert(crate::intern::intern(&field.name), binding);
        }
        ctx.scope.insert(&d.name, Binding::Record(record_fields));
        return;
    }
    let t = type_of_type_expr(&d.typ);
    let node_id = ctx.add_input(&d.name, t.clone(), d.range.clone());
    apply_port_side(ctx, node_id, d.side, d.invisible, &d.range);
    if let Some(label) =
        resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
    {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label));
        }
    }
    ctx.scope
        .insert(&d.name, Binding::Input(NodeRecord { node_id, ty: t }));
}

pub(super) fn pre_declare_output(
    ctx: &mut LowerCtx,
    name: &str,
    value: Option<&Expr>,
    typ: Option<&TypeExpr>,
    side: Option<crate::ast::PortSide>,
    label: Option<&str>,
    label_expr: Option<&Expr>,
    invisible: bool,
    // True only for the top-level module boundary. A record output there dissolves
    // into per-field pins (below); inlined mods and chip instances instead route
    // records through `pending_out_records` at their call site and rely on a
    // single `Binding::Output` per declared output, so they pass false.
    boundary: bool,
    range: &SourceRange,
) {
    // A record-typed output port dissolves into one sub-port per field, bound as
    // a `Record` so `out p: Rec = rec` wires each field to its own boundary pin —
    // the output analogue of `pre_declare_input`. Without this a record output
    // collapsed to a single pin and its value fell through to a lossy single wire
    // (or an `_Unsupported` placeholder for a record literal), leaving the output
    // dangling. Only the top-level boundary needs this (see `boundary`).
    if boundary
        && let Some(te) = typ
        && let Some(fields) = ctx.record_fields_of(te)
    {
        let mut record_fields = HashMap::default();
        for field in &fields {
            let port_name = format!("{name}_{}", field.name);
            let ft = type_of_type_expr(&field.typ);
            let node_id = ctx.add_output(&port_name, ft.clone(), range.clone());
            record_fields.insert(
                crate::intern::intern(&field.name),
                Binding::Output(NodeRecord { node_id, ty: ft }),
            );
        }
        ctx.scope.insert(
            &crate::lower::context::output_scope_key(name),
            Binding::Record(record_fields),
        );
        return;
    }
    // An explicit annotation IS the port's type — the value coerces INTO it
    // (typecheck validates the pair; the `ctx.connect` choke point inserts
    // any adapter, e.g. the string → bool `!= ""` compare). Deriving the
    // port type from the VALUE instead silently made `out y: bool = s` a
    // string port. Ref annotations unwrap so `out y: *int = x` keeps the
    // value-typed port it always had (the ref-ness lives in the AST/emit
    // handling, not the pin type).
    let t = if let (Some(te), Some(_)) = (typ, value) {
        unwrap_ref(&ctx.resolve_local_type(te))
    } else if let Some(v) = value {
        unwrap_ref(&ctx.type_of(v))
    } else if let Some(te) = typ {
        ctx.resolve_local_type(te)
    } else {
        Type::Any
    };
    let node_id = ctx.add_output(name, t.clone(), range.clone());
    apply_port_side(ctx, node_id, side, invisible, range);
    if let Some(label) = resolve_label_text(label, label_expr, &ctx.const_env) {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label));
        }
    }
    ctx.scope.insert(
        &crate::lower::context::output_scope_key(name),
        Binding::Output(NodeRecord { node_id, ty: t }),
    );
}

#[cfg(test)]
mod composite_config_const_env_tests {
    // `fold_mesh_colors`/`fold_ammo_override` resolve a bare identifier
    // through the const environment before their normal syntactic
    // `Expr::Array`/`Expr::RecordLit` check. No current `const` source syntax
    // can bind an array/record value (array folding isn't wired into
    // `expr_to_literal_in` — see the doc comment on `expr_to_literal`), so
    // this exercises the fallback directly against a hand-built `ConstEnv`
    // rather than through a full source-to-lower pipeline.
    use super::*;

    fn ident(name: &str) -> Expr {
        Expr::Ident {
            name: name.to_string(),
            range: SourceRange::default(),
        }
    }

    #[test]
    fn mesh_colors_resolves_a_matching_identifier() {
        let colors = Literal::Array(vec![Literal::Color { r: 255, g: 0, b: 0, a: 255 }]);
        let mut env = ConstEnv::default();
        env.insert("MESH".to_string(), colors.clone());
        assert_eq!(fold_mesh_colors(&ident("MESH"), &env), Some(colors));
    }

    #[test]
    fn mesh_colors_rejects_a_wrong_shaped_identifier() {
        let mut env = ConstEnv::default();
        env.insert("MESH".to_string(), Literal::Array(vec![Literal::Int(1)]));
        assert_eq!(fold_mesh_colors(&ident("MESH"), &env), None);
    }

    #[test]
    fn mesh_colors_rejects_an_unbound_identifier() {
        let env = ConstEnv::default();
        assert_eq!(fold_mesh_colors(&ident("MESH"), &env), None);
    }

    #[test]
    fn ammo_override_resolves_a_matching_identifier() {
        let over = Literal::Array(vec![
            Literal::Bool(true),
            Literal::Array(vec![Literal::Array(vec![Literal::Int(30), Literal::Int(90)])]),
        ]);
        let mut env = ConstEnv::default();
        env.insert("AMMO".to_string(), over.clone());
        assert_eq!(fold_ammo_override(&ident("AMMO"), &env), Some(over));
    }

    #[test]
    fn ammo_override_rejects_a_wrong_shaped_identifier() {
        let mut env = ConstEnv::default();
        env.insert("AMMO".to_string(), Literal::Array(vec![Literal::Bool(true)]));
        assert_eq!(fold_ammo_override(&ident("AMMO"), &env), None);
    }
}
