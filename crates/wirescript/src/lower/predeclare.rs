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
        std::sync::Arc::make_mut(frame).insert(c.name.clone(), std::sync::Arc::new(c.clone()));
    }
}

/// Whether a body declaring its storage before its statements run declares
/// `name`, the storage `stmts[at]` declares, there. A name the body's frame
/// already binds (a parameter, or a member of the body's `import * as`
/// module) is not: the statement declares it where it is written, so the
/// code before it still reads that binding, as it type-checked. Nor is a
/// name that means something outside the body (an outer frame's binding, a
/// constant, an enum or variant, a namespace) when a statement before it, or
/// its own initializer, reads the name: those read what it meant there.
pub(super) fn hoists_storage(ctx: &LowerCtx, stmts: &[Stmt], at: usize, name: &str) -> bool {
    if ctx.scope.get_current(name).is_some() {
        return false;
    }
    let means_something = ctx.scope.get(name).is_some() || ctx.consts.resolved_by_name(name);
    !means_something
        || !stmts[..=at].iter().any(|s| crate::ast::walk::mentions(crate::ast::walk::Node::Stmt(s), name))
}

/// [`hoists_storage`], for the body pre-declare loops: a declaration it
/// leaves to its statement is recorded, so that statement declares fresh
/// storage even though the frame already binds the name (see
/// `LowerCtx::unhoisted`).
pub(super) fn hoist_or_defer(ctx: &mut LowerCtx, stmts: &[Stmt], at: usize, name: &str, range: &SourceRange) -> bool {
    let hoists = hoists_storage(ctx, stmts, at, name);
    if !hoists {
        ctx.unhoisted.insert((range.file.clone(), range.start.offset));
    }
    hoists
}

/// For each `chip`/`mod` declared anywhere in a statement of `body`, the
/// storage names `body` declares in its later statements (see
/// `LowerCtx::decl_later_storage`). Called for a body whose storage is
/// declared before its statements run, with the body's frame current and
/// before that storage is declared.
pub(super) fn record_later_storage(ctx: &mut LowerCtx, body: &Block) {
    // The declarations anywhere inside a statement, not inside another
    // declaration's body.
    fn decls_in<'a>(s: &'a Stmt, out: &mut Vec<&'a ChipDecl>) {
        use crate::ast::walk::{Ctx, Node, walk};
        if let Stmt::ChipDecl(c) = s {
            out.push(c);
            return;
        }
        walk(Node::Stmt(s), Ctx::default(), &mut |n, _| match n {
            Node::Stmt(Stmt::ChipDecl(c)) => {
                out.push(c);
                false
            }
            _ => true,
        });
    }
    let frame = ctx.scope.current_id();
    for (i, s) in body.stmts.iter().enumerate() {
        let mut decls = Vec::new();
        decls_in(s, &mut decls);
        if decls.is_empty() {
            continue;
        }
        let stmts = &body.stmts;
        let names: Vec<crate::intern::Sym> = (i + 1..stmts.len())
            .filter_map(|j| match &stmts[j] {
                Stmt::Var(v) => Some(&v.name).filter(|n| hoists_storage(ctx, stmts, j, n)),
                Stmt::Array(a) => Some(&a.name).filter(|n| hoists_storage(ctx, stmts, j, n)),
                Stmt::Map(m) => Some(&m.name).filter(|n| hoists_storage(ctx, stmts, j, n)),
                Stmt::Buffer(b) => Some(&b.name).filter(|n| hoists_storage(ctx, stmts, j, n)),
                // A chip body's ports are always declared up front.
                Stmt::In(p) => Some(&p.name),
                _ => None,
            })
            .map(|n| crate::intern::intern(n))
            .collect();
        for c in decls {
            let at = (c.range.file.clone(), c.range.start.offset);
            let entry = ctx.decl_later_storage.entry(at).or_default();
            entry.retain(|(f, _)| *f != frame && ctx.scope.has_frame(*f));
            if !names.is_empty() {
                entry.push((frame, names.clone()));
            }
        }
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
        TopDecl::AnonChip(ac) => pre_declare_anon_chip(ctx, ac, true),
        _ => {}
    }
}

/// Create an anonymous chip's Chip node and register its body's storage
/// declarations. Callable directly with the `AnonChipDecl` so a statement-level
/// `chip { }` need not be rewrapped in a cloned `TopDecl` first.
/// Create the anonymous chip `ac`'s node and declare its storage. `hoisted`
/// for a chip at a module's top level, whose storage every statement there
/// names; one in a body is named from the chip on, so what its storage's
/// names bound before it is bound again until `lower_anon_chip`.
pub(super) fn pre_declare_anon_chip(ctx: &mut LowerCtx, ac: &AnonChipDecl, hoisted: bool) {
    let storage_name = |s: &Stmt| match s {
        Stmt::Var(VarDecl { name, .. })
        | Stmt::Buffer(BufferDecl { name, .. })
        | Stmt::Array(ArrayDecl { name, .. })
        | Stmt::Map(MapDecl { name, .. }) => Some(name.clone()),
        _ => None,
    };
    let before: Vec<(String, Option<Binding>)> = if hoisted {
        Vec::new()
    } else {
        ac.body.stmts.iter().filter_map(storage_name).map(|n| {
            let b = ctx.scope.get_current(&n).cloned();
            (n, b)
        }).collect()
    };
    let chip_node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::MICROCHIP_ALT,
        source_range: ac.range.clone(),
        ports: GateIO::default(),
        ..Default::default()
    });
    let consts = ctx.const_lookup();
    if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
        node.kind = NodeKind::Chip;
        let props = std::sync::Arc::make_mut(&mut node.properties);
        if ac.closed {
            props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        }
        if let Some(label) = resolve_label_text(ac.label.as_deref(), ac.label_expr.as_ref(), &consts) {
            props.insert(*sym::NAME_LABEL, Literal::String(label));
        }
        if let Some(doc) = ctx.doc_comments.get(&crate::parser::doc_key(&ac.range)) {
            props.insert(*sym::DOC_TEXT, Literal::String(doc.clone()));
        }
    }
    // Pair the node with its decl so `lower_anon_chip` finds it without a
    // whole-module scan. Keyed by (range, enclosing chip), the same pair the
    // scan matched on.
    ctx.anon_chip_nodes.insert(
        (ac.range.clone(), ctx.current_anon_chip),
        chip_node_id,
    );
    // Tag pre-declared nodes with chip_id.
    let saved = ctx.current_anon_chip.take();
    ctx.current_anon_chip = Some(chip_node_id);
    for s in &ac.body.stmts {
        match s {
            // Nested declaration shadows a same-named outer one for
            // any LATER initializer in this body. See
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
            // An anon chip shares its parent's scope rather than opening one,
            // so a plain (non-`@side`) `out` written directly in its body is
            // a module-boundary output exactly like a top-level `out`, just
            // drawn inside a box; pre-declare it the same way.
            Stmt::OutBinding(o) => {
                if ctx.lookup_output(&o.name).is_some() {
                    continue;
                }
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
                        true,
                        &o.range,
                    )
                });
            }
            _ => {}
        }
    }
    // The storage the body declares, for `lower_anon_chip` to bind again.
    let storage: Vec<(String, Binding)> = ac
        .body
        .stmts
        .iter()
        .filter_map(storage_name)
        .filter_map(|n| {
            let b = ctx.scope.get_current(&n)?.clone();
            Some((n, b))
        })
        .collect();
    ctx.anon_chip_storage.insert(chip_node_id, storage);
    for (name, b) in before {
        match b {
            Some(b) => ctx.scope.insert(&name, b),
            None => ctx.scope.remove(&name),
        };
    }
    ctx.current_anon_chip = saved;
}

/// Pre-declare a top-level `let x: exec` local signal: create a stable Union
/// "hub" gate, bind `x` to its `ExecOut` (so `on x` can trigger off it), and
/// register the emit target. `flush_pending_emits` later wires the union of all
/// `emit x` paths into the hub's `ExecA`. Non-`exec` lets are ignored here (they
/// lower normally in pass 2).
pub(super) fn pre_declare_exec_signal(ctx: &mut LowerCtx, l: &LetDecl) {
    let Some(te @ TypeExpr::Name {
        name: type_name, ..
    }) = &l.typ
    else {
        return;
    };
    if type_name != "exec" && (l.has_initializer() || !crate::lower::call::is_exec_type(ctx, te)) {
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
        ports: GateIO::exec_join(),
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
        // No literal spells a ref. With no initial value, emit writes the
        // port type's null ref.
        Type::Ref(_) | Type::Brick => None,
        _ => Some(Literal::Float(0.0)),
    }
}

/// Compile-time constant environment: every top-level `let` whose initializer is
/// itself constant, by name. Lets an initializer name a constant (`1 << C_FLAG`)
/// instead of restating its value.
pub type ConstEnv = crate::collections::HashMap<String, Literal>;

use crate::const_scope::Constants;

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
/// frame of constants for it, so without this descent an anonymous chip's
/// `const` would be recorded NOWHERE and would read back as a runtime value.
///
/// Descends through nested anonymous chips (each shares the same parent scope
/// in turn) and STOPS at anything that does open a scope of its own — a
/// handler, a named `chip`/`mod` body, an `if` block. Those already record
/// their body constants into their own frame.
/// A `let x: exec` local exec signal (parser-desugared to `= 0`), which must
/// never be collected as a constant (see the call site in `build_const_env`).
/// A `let` written with no value is one through an alias of `exec` (any
/// other type is an error), whatever the alias.
fn is_exec_signal_let(l: &LetDecl) -> bool {
    matches!(&l.typ, Some(TypeExpr::Name { name, .. }) if name == "exec") || !l.has_initializer()
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
    // The top level's own declarations first: where a chip declares a name
    // the top level does too, the module's constant is the top level's, which
    // is what the name means outside the chip (see `scope::top`).
    let mut out: Vec<&LetDecl> = decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Let(l) => Some(l),
            _ => None,
        })
        .collect();
    for d in decls {
        if let TopDecl::AnonChip(ac) = d {
            walk_block(&ac.body, &mut out);
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
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
) -> ConstEnv {
    let types = Arc::new(crate::const_eval::DeclaredTypes::for_program(decls, enum_defs));
    build_const_env_in(decls, enum_defs, &Arc::default(), &types)
}

/// [`build_const_env`] for a program whose files import each other: a
/// `const mod` named like a builtin is called only where `imports` says the
/// calling file can name it.
pub fn build_const_env_in(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
    imports: &Arc<crate::ast::ImportScope>,
    types: &Arc<crate::const_eval::DeclaredTypes>,
) -> ConstEnv {
    build_const_env_at(decls, enum_defs, imports, types, &Arc::new(crate::scope::top::TopNames::of(decls))).0
}

/// The constant of each declaration of a name a file declares more than once
/// at its top level, by the declaration's file, start offset and the name.
pub(crate) type DupConsts = HashMap<(Arc<str>, usize, String), Literal>;

/// [`build_const_env_in`], with `top` the names of `decls`' top level. A name
/// a file declares more than once at its top level is read by position (see
/// [`crate::scope::top::TopNames::visible_decl`]): an initializer reads the
/// declaration visible where it is written, the module's table holds the last
/// declaration's constant, and each declaration's constant comes back in the
/// [`DupConsts`].
pub(crate) fn build_const_env_at(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
    imports: &Arc<crate::ast::ImportScope>,
    types: &Arc<crate::const_eval::DeclaredTypes>,
    top: &Arc<crate::scope::top::TopNames>,
) -> (ConstEnv, DupConsts) {
    let mods = scope_mods(decls);
    let lookup_mod = move |name: &str| mods.get(name).cloned();

    // Flattened via `scope_lets`, so an anonymous chip's body constants join
    // the same fixpoint as the top-level ones — see that function's comment.
    let lets = scope_lets(decls);
    // `Arc`-wrapped because the fixpoint hands the env to `ConstCtx` twice per
    // `let` per pass, and `ConstCtx` owns its tables. Every `ConstCtx` built
    // here is dropped before the next `make_mut`, so the refcount is back to 1
    // and no copy is made: the whole loop clones the table zero times, where
    // deep-copying it per attempt dominated `typecheck` on a const-heavy
    // program (2raab: 495k allocations down to 344k).
    let mut env = std::sync::Arc::new(ConstEnv::default());
    // Namespaced constants: an `import * as Ns` module's top-level `let`/`const`
    // values are reachable as `Ns.name`. Evaluate each namespace's env in
    // ISOLATION (its lets see only its own module's consts, not the importer's),
    // then lift them into `"Ns.name"` keys so an importer's const initializer can
    // fold a namespaced member (`[Other.value]`). Seeded before the fixpoint so a
    // root const that reads a namespaced member settles.
    for d in decls {
        if let TopDecl::Namespace(ns) = d {
            for (name, value) in build_const_env_in(&ns.decls, enum_defs, imports, types) {
                std::sync::Arc::make_mut(&mut env)
                    .entry(format!("{}.{}", ns.name, name))
                    .or_insert(value);
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
    // `scope_lets` lists the top level's own `let`s first.
    let top_lets = decls.iter().filter(|d| matches!(d, TopDecl::Let(_))).count();
    let dup_names: HashSet<&str> = top.dups().map(|(n, _)| n).collect();
    let mut dup_consts = DupConsts::default();
    // What a `const mod` body reads (`ModDups`), rebuilt when `env` or
    // `dup_consts` changes; counted by `env_rev`.
    let mut env_rev = 0usize;
    let mut mod_view: Option<(usize, Arc<crate::const_eval::ModDups>)> = None;
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
            // Both tables are `Arc`-backed, so this per-attempt hand-off is
            // three refcount bumps rather than deep clones of the env and of
            // the whole (game-derived, growing) enum registry. Scoped so the
            // `ConstCtx` is dropped before `make_mut` runs below.
            let evaluated = {
                let value = crate::ast::walk::Node::Expr(&l.value);
                let reads_dup = dup_names.iter().any(|n| crate::ast::walk::mentions(value, n));
                let seen = if reads_dup { Arc::new(dup_env_at(&env, top, &dup_consts, &l.range)) } else { env.clone() };
                let mod_dups = (!dup_names.is_empty()).then(|| match &mod_view {
                    Some((at, view)) if *at == env_rev => view.clone(),
                    _ => {
                        let view = Arc::new(crate::const_eval::ModDups::new(
                            env.clone(),
                            top.clone(),
                            Arc::new(dup_consts.clone()),
                        ));
                        mod_view = Some((env_rev, view.clone()));
                        view
                    }
                });
                let cx = crate::const_eval::ConstCtx::new(
                    crate::const_scope::ConstNames::of(seen),
                    env.clone(),
                    Arc::default(),
                    enum_defs.clone(),
                    imports.clone(),
                    types.clone(),
                    Some(&lookup_mod),
                )
                .with_mod_dups(mod_dups);
                let mut budget = crate::const_eval::Budget::default();
                crate::const_eval::eval_let_value(l, &cx, &mut budget)
            };
            let Ok(lit) = evaluated else {
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
                if i < top_lets && top.is_dup(&l.range.file, &name) {
                    dup_consts.insert((l.range.file.clone(), l.range.start.offset, name), value);
                    env_rev += 1;
                    changed = true;
                    continue;
                }
                // FIRST declaration of a name wins: a duplicate must not
                // overwrite it, or the fixpoint could alternate and never
                // converge.
                if env.contains_key(&name) {
                    continue;
                }
                std::sync::Arc::make_mut(&mut env).insert(name, value);
                env_rev += 1;
                changed = true;
            }
        }
        if !changed {
            for (name, decls) in top.dups() {
                let last = decls.last().and_then(|d| dup_consts.get(&dup_key(d, name)));
                match last {
                    Some(lit) => std::sync::Arc::make_mut(&mut env).insert(name.to_string(), lit.clone()),
                    None => std::sync::Arc::make_mut(&mut env).remove(name),
                };
            }
            // Sole owner here, so this hands back the table itself.
            return (std::sync::Arc::try_unwrap(env).unwrap_or_else(|env| (*env).clone()), dup_consts);
        }
    }
}

/// `env` as a read at `at` sees it: each name a file declares more than once
/// at its top level holds the constant of the declaration visible at `at`
/// (the last one, read from another file), and is absent when that is no
/// constant or not settled yet.
fn dup_env_at(env: &ConstEnv, top: &crate::scope::top::TopNames, dup_consts: &DupConsts, at: &SourceRange) -> ConstEnv {
    let mut out = env.clone();
    for (name, decls) in top.dups() {
        let decl = if decls[0].range.file == at.file {
            top.visible_decl(name, &crate::scope::top::ReadAt::code(at)).flatten()
        } else if top.is_dup(&at.file, name) {
            continue;
        } else {
            decls.last()
        };
        match decl.and_then(|d| dup_consts.get(&dup_key(d, name))) {
            Some(lit) => out.insert(name.to_string(), lit.clone()),
            None => out.remove(name),
        };
    }
    out
}

/// The [`DupConsts`] key of `name` declared by `d`.
pub(crate) fn dup_key(d: &crate::scope::top::TopDup, name: &str) -> (Arc<str>, usize, String) {
    (d.range.file.clone(), d.range.start.offset, name.to_string())
}

/// Every TOP-LEVEL name declared with the `const` keyword (`const x = …`),
/// as opposed to a plain `let` that merely happens to fold — the set
/// [`crate::const_scope::ConstScope`] gates the widened `if`-condition elision on
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

/// The names in `env` ([`build_const_env_in`]'s result for `decls`) whose
/// value is computed through an integer operation the certified fold table
/// has no result for, directly, in a `const mod` it calls, or through another
/// such name (see [`crate::const_eval::integer_ops_certified`]). Such a value
/// is the evaluator's, not the gate's, so a `float` slot is not given it as a
/// literal. A namespace's members are keyed `Ns.name`, as in `env`.
pub fn build_const_uncertified(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
    imports: &Arc<crate::ast::ImportScope>,
    env: &ConstEnv,
    types: &Arc<crate::const_eval::DeclaredTypes>,
) -> crate::collections::HashSet<String> {
    let mut tainted: crate::collections::HashSet<String> = crate::collections::HashSet::default();
    for d in decls {
        if let TopDecl::Namespace(ns) = d {
            let prefix = format!("{}.", ns.name);
            let own: ConstEnv = env
                .iter()
                .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|n| (n.to_string(), v.clone())))
                .collect();
            for name in build_const_uncertified(&ns.decls, enum_defs, imports, &own, types) {
                tainted.insert(format!("{prefix}{name}"));
            }
        }
    }
    let mods = scope_mods(decls);
    let lookup_mod = move |name: &str| mods.get(name).cloned();
    let lets = scope_lets(decls);
    loop {
        let certified: Arc<ConstEnv> =
            Arc::new(env.iter().filter(|(n, _)| !tainted.contains(*n)).map(|(n, v)| (n.clone(), v.clone())).collect());
        let cx = crate::const_eval::ConstCtx::new(
            crate::const_scope::ConstNames::of(certified.clone()),
            certified,
            Arc::default(),
            enum_defs.clone(),
            imports.clone(),
            types.clone(),
            Some(&lookup_mod),
        );
        let mut changed = false;
        for l in &lets {
            let names: Vec<String> = crate::const_eval::bound_names(&l.binding)
                .into_iter()
                .filter(|n| env.contains_key(n) && !tainted.contains(n))
                .collect();
            if names.is_empty() || crate::const_eval::integer_ops_certified(&l.value, &cx) {
                continue;
            }
            tainted.extend(names);
            changed = true;
        }
        if !changed {
            return tainted;
        }
    }
}

/// Resolve an explicit `@label` override to its baked display text: the
/// string form (`@label("text")`) is used as-is; the expression form
/// (`@label(expr)`) is const-folded against the script's constant
/// environment via [`expr_to_literal_in`] — typecheck.rs already rejects a
/// non-constant expression, so a fold failure here just yields no override
/// (rather than double-reporting the error). `None` means "no override" —
/// the caller's own default (e.g. the decl's name) applies.
pub(super) fn resolve_label_text<C: Constants + ?Sized>(
    label: Option<&str>,
    label_expr: Option<&Expr>,
    env: &C,
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

/// Ask the certified fold table what the gate returns for these operands.
///
/// Every `OpRule` for one operator names the same gate class, so the operand
/// types are not needed to find it. A non-finite float is refused rather than
/// baked, mirroring the driver guard at `fold/mod.rs`: that keeps float
/// `x / 0.0` on the `0.0` law below instead of baking `inf`.
pub(crate) fn certified_binop(operator: &str, l: &Literal, r: &Literal) -> Option<Literal> {
    let class = crate::catalog::operators::operators()
        .iter()
        .find(|s| s.op == operator)?
        .rules
        .first()?
        .gate_class;
    let (a, b) = (
        crate::lower::fold::eval::Value::from_literal(l)?,
        crate::lower::fold::eval::Value::from_literal(r)?,
    );
    let v = crate::lower::fold::eval::eval(class, &[Some(a), Some(b)])?;
    if matches!(&v, crate::lower::fold::eval::Value::Float(f) if !f.is_finite()) {
        return None;
    }
    Some(v.to_literal())
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

    // The certified table first: it is the probed record of what the GAME's
    // gate returns, so where it has an answer this law has no business
    // guessing a different one. It is purely additive (it can only turn a
    // refusal into an answer, never change a pair that folds today), which is
    // what makes it safe to ship ahead of the `-7 / 2` question the two
    // evaluators are still blocked on. Without it, `static var x: int = 1 + true` warned
    // and dropped its initializer while `out x: int = 1 + true` baked `2`.
    if let Some(v) = certified_binop(operator, &l, &r) {
        return Some(v);
    }

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
    expr_to_literal_impl::<ConstEnv>(e, None)
}

/// [`expr_to_literal`], plus named top-level constants and operators over them —
/// so an initializer can read `1 << C_FLAG` or `WIDTH * HEIGHT` instead of a
/// magic number. Only the `var` initializer paths pass an environment;
/// everywhere else keeps the narrower behaviour above.
pub(crate) fn expr_to_literal_in<C: Constants + ?Sized>(e: &Expr, env: &C) -> Option<Literal> {
    expr_to_literal_impl(e, Some(env))
}

/// `env == None` reproduces the original syntactic folding exactly; `Some`
/// additionally resolves named constants and evaluates operators.
fn expr_to_literal_impl<C: Constants + ?Sized>(e: &Expr, env: Option<&C>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // -- Constant-environment forms: initializers only. --
        // With no environment these fall through to `_ => None`.
        Expr::Ident { name, .. } => env?.constant(name).cloned(),
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
fn expr_to_literal_lit<C: Constants + ?Sized>(e: &Expr, env: Option<&C>) -> Option<Literal> {
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
        // at emit into the gate's `Prefab` path string.
        Expr::PrefabRef { path, .. } => Some(Literal::PrefabRef { path: path.clone() }),
        // Inline nested-prefab block `$``` ... ``` ` — inlined; compiled +
        // embedded at emit into the gate's `Prefab` path string.
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
pub(crate) fn fold_mesh_colors<C: Constants + ?Sized>(e: &Expr, env: &C) -> Option<Literal> {
    if let Expr::Ident { name, .. } = e {
        return match env.constant(name) {
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
pub(crate) fn fold_ammo_override<C: Constants + ?Sized>(e: &Expr, env: &C) -> Option<Literal> {
    if let Expr::Ident { name, .. } = e {
        return match env.constant(name) {
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
pub(super) fn coerce_literal_to_type(lit: Literal, ty: &Type) -> Literal {
    match ty {
        Type::Int => match lit {
            Literal::Bool(b) => Literal::Int(b as i64),
            number => crate::const_eval::number_in_type(number, ty),
        },
        Type::Float => match lit {
            Literal::Float(f) => Literal::Float(f),
            Literal::Int(n) => crate::const_eval::number_in_type(Literal::Int(n), ty),
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

/// Why a map literal has no constant form.
enum MapBakeMiss {
    /// Object/asset-family keys have no literal representation: `key_of`
    /// bakes every one as `Object(None)`, so two or more entries would collapse
    /// onto the same null key (a corrupt map with duplicate keys).
    ObjectKeys,
    /// Some entry doesn't fold to a constant.
    NonConstant,
}

/// The constant `(key, value)` pairs of a map literal, coerced to the declared
/// `Map<K, V>` (see [`map_entry_literal`]), or why it has none. The one bake
/// rule for a map var's initializer and a record's map field initializer.
fn map_literal_pairs(
    ctx: &LowerCtx,
    entries: &[crate::ast::MapLitEntry],
    key_ty: &Type,
    val_ty: &Type,
) -> Result<Vec<(Literal, Literal)>, MapBakeMiss> {
    if matches!(key_ty, Type::Entity | Type::Character | Type::Controller) {
        return Err(MapBakeMiss::ObjectKeys);
    }
    let pairs: Vec<(Literal, Literal)> = entries
        .iter()
        .filter_map(|en| map_entry_literal(en, ctx, key_ty, val_ty))
        .collect();
    if pairs.len() == entries.len() {
        Ok(pairs)
    } else {
        Err(MapBakeMiss::NonConstant)
    }
}

/// Bake a constant map-literal initializer (`var m: Map<K, V> = {...}`) into
/// `properties` as an `InitialValue` (`Literal::Map`) — zero runtime gates,
/// exactly like the array path bakes `Literal::Array`.
/// Shared by [`pre_declare_map`] and the `Map<K, V>` branch of
/// [`pre_declare_var`] since both bake the same way. A literal that can't bake
/// (see [`map_literal_pairs`]) leaves the map empty with a warning (lowering
/// handles the exec-context desugar for `m = {…}`).
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
    match map_literal_pairs(ctx, entries, key_ty, val_ty) {
        Ok(pairs) => {
            properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
        }
        Err(MapBakeMiss::ObjectKeys) => ctx.warn(
            format!(
                "'{name}' initializer has object/asset-typed keys, which can't bake as literals, so it starts empty; assign entries inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        ),
        Err(MapBakeMiss::NonConstant) => ctx.warn(
            format!(
                "'{name}' initializer has non-constant entries, which are dropped here; assign them inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        ),
    }
}

/// A `var` initializer that can't bake into the gate as a constant: returns it
/// for diagnosis. `None` = no initializer, or it bakes fine. Takes the whole
/// `ctx` (not just its constants) so the array branch can route through
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
        // A record-literal initializer bakes PER FIELD into each backing gate's
        // InitialValue (see `record_field_storage`), so it is unbaked only when
        // some field's value can't bake there.
        Expr::RecordLit { .. } => record_init_unbaked(ctx, init),
        // `null` bakes to the var's type default (the `or_else` fallback below),
        // which IS its value — never unbaked.
        Expr::NullLit { .. } => false,
        // An enum-typed initializer bakes into `__disc` + any foldable payload
        // slots (see `declare_enum_container`), the same "no single gate to
        // bake it into" reasoning as a record literal just above - the generic
        // scalar warning below would be a false positive even when a payload
        // arg doesn't itself fold (the discriminant still bakes).
        e if matches!(ctx.type_of(e), Type::Enum { .. }) => false,
        e => {
            expr_to_literal_in(e, &ctx.const_lookup()).is_none()
                && const_container_init(ctx, e).is_none()
                && !constant_record(ctx, e)
        }
    };
    unbaked.then_some(init)
}

/// Whether `e` names a constant record (`CP`, `ns.LP`), which bakes field by
/// field (see `bake_const_record`).
fn constant_record(ctx: &LowerCtx, e: &Expr) -> bool {
    matches!(e, Expr::Ident { .. } | Expr::FieldAccess { .. }) && matches!(fold_const(ctx, e), Some(Literal::Record(_)))
}

/// Whether some field of a record-literal initializer can't bake into its
/// backing gate: a scalar that doesn't fold, a container whose value isn't a
/// fully constant literal, a shorthand or spread (neither bakes). Nested record
/// literals recurse, as `record_field_storage` does.
fn record_init_unbaked(ctx: &LowerCtx, e: &Expr) -> bool {
    let Expr::RecordLit { fields, .. } = e else {
        return false;
    };
    fields.iter().any(|f| match f {
        RecordLitField::Named { value, .. } => field_init_unbaked(ctx, value),
        RecordLitField::Shorthand { .. } | RecordLitField::Spread { .. } => true,
    })
}

fn field_init_unbaked(ctx: &LowerCtx, value: &Expr) -> bool {
    match value {
        Expr::RecordLit { .. } => record_init_unbaked(ctx, value),
        Expr::Array { elements, .. } => elements.iter().any(|el| match el {
            ArrayElem::Item(row @ Expr::RecordLit { .. }) => record_init_unbaked(ctx, row),
            ArrayElem::Item(_) => array_elem_literal(el, ctx).is_none(),
            ArrayElem::Spread(_) => true,
        }),
        Expr::MapLit { entries, .. } => {
            let Type::Map(k, v) = ctx.type_of(value) else {
                return true;
            };
            // A record-valued map is columns, baked per column from each
            // entry's record literal; any other map bakes by the one map rule.
            if entries.iter().any(|en| matches!(en.value, Expr::RecordLit { .. })) {
                entries.iter().any(|en| match &en.value {
                    row @ Expr::RecordLit { .. } => {
                        fold_const(ctx, &en.key).is_none() || record_init_unbaked(ctx, row)
                    }
                    _ => true,
                })
            } else {
                map_literal_pairs(ctx, entries, &k, &v).is_err()
            }
        }
        Expr::NullLit { .. } => false,
        e if matches!(ctx.type_of(e), Type::Enum { .. }) => false,
        e => {
            expr_to_literal_in(e, &ctx.const_lookup()).is_none()
                && const_container_init(ctx, e).is_none()
                && !constant_record(ctx, e)
        }
    }
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
    let holds_ref = v.typ.as_ref().is_some_and(|te| matches!(ctx.declared_shape(None, te), TypeExpr::Ref { .. }))
        || matches!(ctx.type_of(init), Type::Ref(_));
    let msg = if holds_ref {
        format!(
            "'var {}' holds a ref, and a ref initializer is not baked yet. It is dropped, so \
             the var starts as a null ref, which halts the exec chain when used; assign the \
             ref inside an exec handler instead",
            v.shown_name()
        )
    } else if v.is_static {
        format!(
            "'static var {}' initializer must be a compile-time constant: this value is dropped and the var starts at its type default",
            v.shown_name()
        )
    } else {
        format!(
            "'var {}' initializer is not a compile-time constant. Outside an exec context it is dropped and the var starts at its type default; assign the value inside an exec handler instead",
            v.shown_name()
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
        gate_class: gc::ARRAY_VAR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec::new(
                              *sym::ARRAY_VAR_REF,
                              Type::Ref(Box::new(Type::Array(Box::new(elem_type.clone())))),
                          )],
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
        gate_class: gc::MAP_VAR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec::new(*sym::MAP_VAR_REF, Type::Ref(Box::new(map_type.clone())))],
        },
        properties,
        note: None,
        ..Default::default()
    })
}

/// Build just the `Pseudo_Var` scalar gate (no scope binding) — the gate half of
/// [`pre_declare_var`]'s scalar tail, shared with [`record_field_storage`]'s
/// per-field scalar backing.
pub(super) fn make_scalar_var_gate(
    ctx: &mut LowerCtx,
    inner_type: &Type,
    properties: HashMap<crate::intern::Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR,
        source_range: range.clone(),
        ports: GateIO::var_storage(inner_type.clone()),
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

/// How deep a record's field tree may be decomposed before lowering gives up.
/// A record alias that names itself (`type L = { tail: L }`) has no finite
/// storage or pin layout, and every walk below follows aliases at every level,
/// so each needs a bound. Typecheck rejects such an alias, but lowering still
/// runs on a rejected program. Matches the type resolver's `MAX_ALIAS_DEPTH`;
/// real record nesting is far shallower.
const MAX_RECORD_FIELD_DEPTH: u32 = 32;

fn record_field_storage(
    ctx: &mut LowerCtx,
    kind: VarStorage,
    label_base: &str,
    field_name: &str,
    field_typ: &TypeExpr,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
    depth: u32,
) -> Binding {
    let label = format!("{label_base}.{field_name}");
    let expanded = container_alias(ctx, field_typ);
    let field_typ = expanded.as_ref().unwrap_or(field_typ);
    // A record alias whose body names itself (`type L = { tail: L }`) expands
    // without end here, because the field walk below follows aliases at every
    // level. Typecheck reports the alias itself, but lowering still runs on a
    // rejected program, so the recursion needs its own bound: past it, stop
    // decomposing and let the field take a single scalar gate. The cap matches
    // the type resolver's `MAX_ALIAS_DEPTH`; real record nesting is far shallower.
    let too_deep = depth >= MAX_RECORD_FIELD_DEPTH;
    if depth == MAX_RECORD_FIELD_DEPTH {
        ctx.diagnostics.push(Diagnostic::error(
            "WS002",
            format!(
                "record type nests more than {MAX_RECORD_FIELD_DEPTH} levels deep at \
                 `{label}`: a record alias that names itself has no finite storage layout"
            ),
            range.clone(),
        ));
    }
    // A nested record (inline `{ … }` or via an alias) recurses regardless of
    // the record's own storage kind — a record ARRAY of a record field is
    // parallel arrays one level deeper. `record_fields_of` follows aliases at
    // every level, which `resolve_local_type` (empty alias table) would not.
    if let Some(sub_fields) = ctx.record_or_tuple_fields(field_typ).filter(|_| !too_deep) {
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
                    depth + 1,
                ),
            );
        }
        // A nested record named rather than spelled (`{ i: CI }`) bakes as
        // `var p: P = CP` does.
        if kind == VarStorage::Var
            && let Some(lit @ Literal::Record(_)) =
                init.filter(|e| !matches!(e, Expr::RecordLit { .. })).and_then(|e| fold_const(ctx, e))
        {
            bake_record_var_fields(ctx, &fmap, &lit);
        }
        return Binding::Record(fmap);
    }
    // An ENUM field is multi-gate exactly as a nested record is: its own
    // `__disc` plus the superset of every variant's payload slots. Falling
    // through to the scalar arm below collapsed all of that into ONE gate, so
    // the tag and payload shared a slot, a write to the field found a scalar
    // `Binding::Var` against a `Binding::Record` source and vanished, and a read
    // had no slots to project.
    //
    // Inside a record ARRAY or MAP the same columns become parallel
    // arrays/maps, one per tag and payload slot, which is what the nested-record
    // recursion above already does a level deeper. Routing through
    // `enum_container_columns` reuses that recursion rather than duplicating it,
    // so the two layouts stay described in one place.
    if let Some(cols) = enum_container_columns(ctx, field_typ, range).filter(|_| !too_deep) {
        if kind == VarStorage::Var {
            let field_type = ctx.resolve_local_type(field_typ);
            if let Type::Enum { args, .. } = &field_type
                && let Some(def) = ctx.enum_defs.of(&field_type).cloned()
            {
                // The field's initializer bakes as a top-level enum var's does.
                let known = init.and_then(|e| static_enum_ctor(ctx, &def, e));
                let fields = build_enum_fields(ctx, &def, args, &label, known.as_ref(), None, range);
                return Binding::Record(fields);
            }
        }
        let mut fmap = HashMap::default();
        for c in &cols {
            fmap.insert(
                crate::intern::intern(&c.name),
                record_field_storage(ctx, kind, &label, &c.name, &c.typ, None, map_key, range, depth + 1),
            );
        }
        return Binding::Record(fmap);
    }
    // A record VARIABLE's array or map field whose element is itself columnar
    // (`type H = { pts: P[] }`) is stored exactly like a top-level
    // `var pts: P[]`: one parallel column per element field, bound as a nested
    // `Binding::Record`, so `h.pts` resolves to columns wherever `pts` does.
    if kind == VarStorage::Var
        && !too_deep
        && let Some(binding) = columnar_container_field(ctx, &label, field_typ, init, range, depth)
    {
        return binding;
    }
    let field_type = ctx.resolve_local_type(field_typ);
    let field_type = &field_type;
    // Taken before the gate-building borrows `ctx` mutably below.
    let consts = ctx.const_lookup();
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(label.clone()));
    match kind {
        // A record VARIABLE: each field is a per-field container matching the
        // field's own type (scalar/array/map), exactly like a record input port.
        VarStorage::Var => match field_type {
            Type::Array(elem) => {
                if let Some(Expr::Array { elements, .. }) = init {
                    let lits: Vec<Literal> = elements
                        .iter()
                        .filter_map(|el| array_elem_literal(el, ctx))
                        .map(|lit| bake_string_bool(lit, elem))
                        .collect();
                    if lits.len() == elements.len() {
                        properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
                    }
                } else if let Some(Literal::Array(items)) = init.and_then(|e| const_container_init(ctx, e)) {
                    let lits = items.into_iter().map(|lit| bake_string_bool(lit, elem)).collect();
                    properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
                }
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
                // Only a fully constant literal bakes; `var_init_unbaked` reports
                // the rest where no exec-context assignment rebuilds it.
                if let Some(Expr::MapLit { entries, .. }) = init
                    && let Ok(pairs) = map_literal_pairs(ctx, entries, k, v)
                {
                    properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
                } else if let Some(Literal::Map(pairs)) = init.and_then(|e| const_container_init(ctx, e)) {
                    let coerce =
                        |(a, b): (Literal, Literal)| (coerce_literal_to_type(a, k), coerce_literal_to_type(b, v));
                    properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs.into_iter().map(coerce).collect()));
                }
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

/// The array or map type a (chain of) alias names: `Pts` for `type Pts = P[]`
/// reads as `P[]`, and `Arr<P>` for `type Arr<T> = T[]` reads as `P[]`. `None`
/// when `te` is not an alias of a container, which leaves record and enum
/// aliases to their own resolvers.
fn container_alias(ctx: &LowerCtx, te: &TypeExpr) -> Option<TypeExpr> {
    let is_container = |t: &TypeExpr| match t {
        TypeExpr::Array { .. } => true,
        TypeExpr::Generic { name, args, .. } => name == "Map" && args.len() == 2,
        _ => false,
    };
    let mut cur = te.clone();
    for _ in 0..MAX_RECORD_FIELD_DEPTH {
        let next = match &cur {
            TypeExpr::Name { name, range } => ctx.alias_at(name, range)?.clone(),
            TypeExpr::Generic { name, args, range } if !is_container(&cur) => {
                ctx.generic_alias_at(name, args, range)?
            }
            _ => return None,
        };
        if is_container(&next) {
            return Some(next);
        }
        cur = next;
    }
    None
}

/// The columns of a record-element array field (`pts: P[]`) or record-value map
/// field (`m: Map<K, P>`) of a record variable: the same per-field
/// `Pseudo_ArrayVar` / `Pseudo_MapVar` layout `pre_declare_var` gives a top-level
/// `var pts: P[]` / `var m: Map<K, P>`, labelled under `label`, with a constant
/// initializer baked per column. `None` when the field is not an array or map of
/// a record, tuple or enum element.
fn columnar_container_field(
    ctx: &mut LowerCtx,
    label: &str,
    field_typ: &TypeExpr,
    init: Option<&Expr>,
    range: &SourceRange,
    depth: u32,
) -> Option<Binding> {
    let (kind, elem_te, map_key) = match field_typ {
        TypeExpr::Array { inner, .. } => (VarStorage::Array, inner.as_ref(), None),
        TypeExpr::Generic { name, args, .. } if name == "Map" && args.len() == 2 => {
            (VarStorage::Map, &args[1], Some(ctx.resolve_local_type(&args[0])))
        }
        _ => return None,
    };
    let cols = ctx
        .record_or_tuple_fields(elem_te)
        .or_else(|| enum_container_columns(ctx, elem_te, range))?;
    let mut fmap = HashMap::default();
    for c in &cols {
        fmap.insert(
            crate::intern::intern(&c.name),
            record_field_storage(
                ctx,
                kind,
                label,
                &c.name,
                &c.typ,
                None,
                map_key.as_ref(),
                range,
                depth + 1,
            ),
        );
    }
    match init {
        Some(Expr::Array { elements, .. }) if kind == VarStorage::Array => {
            let rows: Vec<&Expr> = elements
                .iter()
                .filter_map(|el| match el {
                    ArrayElem::Item(e) => Some(e),
                    ArrayElem::Spread(_) => None,
                })
                .collect();
            if rows.len() == elements.len() {
                bake_array_rows(ctx, &fmap, &rows);
            }
        }
        Some(Expr::MapLit { entries, .. }) if kind == VarStorage::Map => {
            let pairs: Vec<(&Expr, &Expr)> = entries.iter().map(|en| (&en.key, &en.value)).collect();
            bake_record_map_pairs(ctx, &fmap, &pairs);
        }
        Some(e) => {
            if let Some(lit) = const_container_init(ctx, e) {
                bake_constant_columns(ctx, &fmap, lit);
            }
        }
        None => {}
    }
    Some(Binding::Record(fmap))
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
            Binding::Record(sub) => bake_array_rows(ctx, sub, &cols),
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

/// Bake constant rows already folded to their columns (an enum value folds to
/// `__disc` plus its variant's payload columns) into each backing array's
/// `InitialValue`; a column a row has no value for (another variant's
/// payload) takes its type's zero.
fn bake_folded_columns(
    ctx: &mut LowerCtx,
    rec: &HashMap<crate::intern::Sym, Binding>,
    rows: &[Vec<(String, Literal)>],
) {
    for (fkey, binding) in rec {
        let fname = crate::intern::resolve(*fkey);
        let field = |row: &Vec<(String, Literal)>| row.iter().find(|(n, _)| *n == fname).map(|(_, l)| l.clone());
        match binding {
            Binding::Record(sub) => {
                let sub_rows: Vec<Vec<(String, Literal)>> = rows
                    .iter()
                    .map(|r| match field(r) {
                        Some(Literal::Record(fields)) => fields,
                        _ => Vec::new(),
                    })
                    .collect();
                bake_folded_columns(ctx, sub, &sub_rows);
            }
            Binding::Var(v) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let node_id = v.node_id;
                let lits: Option<Vec<Literal>> = rows
                    .iter()
                    .map(|r| field(r).or_else(|| default_literal_for_var_type(&elem)).map(|l| bake_string_bool(l, &elem)))
                    .collect();
                if let Some(lits) = lits {
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
            Binding::Record(sub) => bake_map_rows(ctx, sub, &cols),
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

/// A constant map entry: its key, and its value folded to its columns.
type FoldedEntry = (Literal, Vec<(String, Literal)>);

/// The map twin of [`bake_folded_columns`]: constant entries whose values are
/// already folded to their columns, baked into each backing map's
/// `InitialValue` under the entry's key.
fn bake_folded_map_columns(ctx: &mut LowerCtx, rec: &HashMap<crate::intern::Sym, Binding>, rows: &[FoldedEntry]) {
    for (fkey, binding) in rec {
        let fname = crate::intern::resolve(*fkey);
        let field = |row: &Vec<(String, Literal)>| row.iter().find(|(n, _)| *n == fname).map(|(_, l)| l.clone());
        match binding {
            Binding::Record(sub) => {
                let sub_rows: Vec<FoldedEntry> = rows
                    .iter()
                    .map(|(k, r)| match field(r) {
                        Some(Literal::Record(fields)) => (k.clone(), fields),
                        _ => (k.clone(), Vec::new()),
                    })
                    .collect();
                bake_folded_map_columns(ctx, sub, &sub_rows);
            }
            Binding::Var(v) if v.storage == VarStorage::Map => {
                let Type::Map(key_ty, val_ty) = &v.inner_type else { continue };
                let node_id = v.node_id;
                let pairs: Option<Vec<(Literal, Literal)>> = rows
                    .iter()
                    .map(|(k, r)| {
                        let value = field(r).or_else(|| default_literal_for_var_type(val_ty))?;
                        Some((coerce_literal_to_type(k.clone(), key_ty), coerce_literal_to_type(value, val_ty)))
                    })
                    .collect();
                if let Some(pairs) = pairs {
                    set_initial_value(ctx, node_id, Literal::Map(pairs));
                }
            }
            _ => {}
        }
    }
}

/// Bake constant entries into per-column maps: record literals field by
/// field, and enum values (`{ "a": Hue.Blue }`) by folding each to its tag and
/// payload columns.
fn bake_map_rows(ctx: &mut LowerCtx, rec: &HashMap<crate::intern::Sym, Binding>, entries: &[(&Expr, &Expr)]) {
    if entries.iter().all(|(_, v)| matches!(v, Expr::RecordLit { .. })) {
        bake_record_map_pairs(ctx, rec, entries);
        return;
    }
    let folded: Option<Vec<FoldedEntry>> = entries
        .iter()
        .map(|(k, v)| match (fold_const(ctx, k), fold_const(ctx, v)) {
            (Some(k), Some(Literal::Record(fields))) => Some((k, fields)),
            _ => None,
        })
        .collect();
    if let Some(folded) = folded {
        bake_folded_map_columns(ctx, rec, &folded);
    }
}

/// The value of a container initializer that names a constant rather than
/// spelling a literal (`var xs: int[] = IS`, `var m: Map<string, int> = M`):
/// the folded array or map. `None` for a literal, which its own path bakes.
pub(super) fn const_container_init(ctx: &LowerCtx, init: &Expr) -> Option<Literal> {
    if matches!(init, Expr::Array { .. } | Expr::MapLit { .. }) {
        return None;
    }
    fold_const(ctx, init).filter(|l| matches!(l, Literal::Array(_) | Literal::Map(_)))
}

/// Bake a constant record initializer named rather than spelled (`var p: P =
/// CP`, `CP` a constant record) into the per-field variables bound as `name`,
/// as a record literal's fields bake: a nested record and an enum field
/// (its tag and payload slots) field by field, an array or map field whole.
fn bake_const_record(ctx: &mut LowerCtx, name: &str, init: Option<&Expr>) {
    let Some(init) = init.filter(|e| !matches!(e, Expr::RecordLit { .. })) else { return };
    let Some(lit @ Literal::Record(_)) = fold_const(ctx, init) else { return };
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else { return };
    bake_record_var_fields(ctx, &rec, &lit);
}

fn bake_record_var_fields(ctx: &mut LowerCtx, rec: &HashMap<crate::intern::Sym, Binding>, lit: &Literal) {
    let Literal::Record(values) = lit else { return };
    for (field, value) in values {
        match rec.get(&crate::intern::intern(field)) {
            Some(Binding::Record(sub)) => bake_record_var_fields(ctx, sub, value),
            Some(Binding::Var(v)) => {
                let baked = match (v.storage, value) {
                    (VarStorage::Array, Literal::Array(items)) => Literal::Array(
                        items.iter().map(|l| bake_literal_for_type(l.clone(), &v.inner_type)).collect(),
                    ),
                    (VarStorage::Map, Literal::Map(pairs)) => match &v.inner_type {
                        Type::Map(k, t) => {
                            let coerce = |(a, b): &(Literal, Literal)| {
                                (coerce_literal_to_type(a.clone(), k), coerce_literal_to_type(b.clone(), t))
                            };
                            Literal::Map(pairs.iter().map(coerce).collect())
                        }
                        _ => continue,
                    },
                    (VarStorage::Var, value) => bake_literal_for_type(value.clone(), &v.inner_type),
                    _ => continue,
                };
                set_initial_value(ctx, v.node_id, baked);
            }
            _ => {}
        }
    }
}

/// Bake a constant initializer named rather than spelled (see
/// [`const_container_init`]) into the per-column arrays or maps of the
/// record or enum container bound as `name`.
fn bake_const_columns(ctx: &mut LowerCtx, name: &str, init: Option<&Expr>) {
    let Some(lit) = init.and_then(|e| const_container_init(ctx, e)) else { return };
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else { return };
    bake_constant_columns(ctx, &rec, lit);
}

/// Bake a constant array or map of records or enums, `lit`, into the
/// per-column arrays or maps `rec`.
fn bake_constant_columns(ctx: &mut LowerCtx, rec: &HashMap<crate::intern::Sym, Binding>, lit: Literal) {
    let row = |l: &Literal| match l {
        Literal::Record(fields) => Some(fields.clone()),
        _ => None,
    };
    match lit {
        Literal::Array(rows) => {
            if let Some(rows) = rows.iter().map(row).collect::<Option<Vec<_>>>() {
                bake_folded_columns(ctx, rec, &rows);
            }
        }
        Literal::Map(pairs) => {
            if let Some(rows) = pairs.iter().map(|(k, v)| row(v).map(|r| (k.clone(), r))).collect::<Option<Vec<_>>>() {
                bake_folded_map_columns(ctx, rec, &rows);
            }
        }
        _ => {}
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
    bake_array_rows(ctx, &rec, &rows);
}

/// Bake constant rows into per-column arrays: record literals field by field,
/// and enum values (`[Hue.Red, S.B(3)]`) by folding each to its tag and
/// payload columns.
fn bake_array_rows(ctx: &mut LowerCtx, rec: &HashMap<crate::intern::Sym, Binding>, rows: &[&Expr]) {
    if rows.iter().all(|r| matches!(r, Expr::RecordLit { .. })) {
        bake_record_array_columns(ctx, rec, rows);
        return;
    }
    let folded: Option<Vec<Vec<(String, Literal)>>> = rows
        .iter()
        .map(|r| match fold_const(ctx, r) {
            Some(Literal::Record(fields)) => Some(fields),
            _ => None,
        })
        .collect();
    if let Some(folded) = folded {
        bake_folded_columns(ctx, rec, &folded);
    }
}

/// Bake a record MAP initializer (`= { k => {..} }`) into the per-field maps.
fn bake_record_map_init(ctx: &mut LowerCtx, name: &str, entries: &[crate::ast::MapLitEntry]) {
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    let pairs: Vec<(&Expr, &Expr)> = entries.iter().map(|en| (&en.key, &en.value)).collect();
    bake_map_rows(ctx, &rec, &pairs);
}

/// Decompose a record-typed storage declaration (`var p: Rec`, `var a: Rec[]`,
/// `var m: Map<K, Rec>`) into one `Binding::Record` of per-field backing gates,
/// and bind it in scope under `name`. `kind` selects the per-field backing kind;
/// `map_key` carries the map's key type for the `Map` case.
/// The parallel-column list for an enum stored in a CONTAINER (an enum-element
/// array or an enum-valued map), expressed as synthetic record fields so the
/// whole record-container machinery (per-column storage, the push/get/set
/// fan-out in `lower_record_array_method` / `lower_record_map_method`, and the
/// per-column element read) applies unchanged.
///
/// The columns are exactly the enum value layout: `__disc` plus every variant's
/// payload slots, keyed `__{Variant}_{index}` for a positional payload and
/// `__{Variant}_{field}` for a named one. That is the same key scheme
/// `build_enum_fields` and `lower_enum_ctor` use, so a constructed value's
/// record lines up with the columns key for key.
///
/// `None` for an unknown enum, which leaves the caller on its existing
/// single-column path.
pub(super) fn enum_container_columns(
    ctx: &LowerCtx,
    elem_te: &TypeExpr,
    range: &SourceRange,
) -> Option<Vec<crate::ast::RecordTypeField>> {
    use crate::typecheck::enums::Payload;
    // The element's own annotation, read where it is written (an alias as the
    // type it names).
    let def = ctx.enum_defs.of(&ctx.resolve_local_type(elem_te))?;
    let mut cols = vec![crate::ast::RecordTypeField {
        name: "__disc".to_string(),
        typ: TypeExpr::Name {
            name: "int".to_string(),
            range: range.clone(),
        },
        range: range.clone(),
    }];
    for v in &def.variants {
        match &v.payload {
            Payload::Unit => {}
            Payload::Positional(types) => {
                for (i, te) in types.iter().enumerate() {
                    cols.push(crate::ast::RecordTypeField {
                        name: format!("__{}_{}", v.name, i),
                        typ: te.clone(),
                        range: range.clone(),
                    });
                }
            }
            Payload::Named(nfields) => {
                for (fname, te) in nfields {
                    cols.push(crate::ast::RecordTypeField {
                        name: format!("__{}_{}", v.name, fname),
                        typ: te.clone(),
                        range: range.clone(),
                    });
                }
            }
        }
    }
    Some(cols)
}

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
                0,
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
fn static_enum_ctor(ctx: &LowerCtx, def: &crate::typecheck::enums::EnumDef, e: &Expr) -> Option<StaticEnumCtor> {
    let enum_name = def.name.as_str();
    // `Enum.Variant` matches `enum_name` - every construction form starts
    // with this same `obj.field` shape (bare for unit, as a call callee for
    // positional, as a VariantCtor path for named).
    let is_enum_obj = |obj: &Expr| ctx.enum_head(obj) == Some(enum_name);
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
    enum_ty: &Type,
    args: &[Type],
    label_base: &str,
    init: Option<&Expr>,
    range: &SourceRange,
) {
    let Some(def) = ctx.enum_defs.of(enum_ty).cloned() else {
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
    let known = init.and_then(|e| static_enum_ctor(ctx, &def, e));
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
    if let Type::Enum { args: nested_args, .. } = &field_type
        && let Some(nested_def) = ctx.enum_defs.of(&field_type).cloned()
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
    // A payload typed by one of the enum's own type parameters (`Opt<P>`'s
    // `Some(T)`) is laid out as the record or tuple this instantiation binds
    // it to, field by field, exactly as a payload written `Some(P)` is.
    let instantiated;
    let field_typ = match &field_type {
        Type::Record(_) | Type::Tuple(_) if ctx.record_or_tuple_fields(field_typ).is_none() => {
            instantiated = super::context::type_expr_of(&field_type, range);
            &instantiated
        }
        _ => field_typ,
    };
    if let Some(sub_fields) = ctx.record_or_tuple_fields(field_typ) {
        // `init_lit` for a record slot arrives as a `Literal::Record` (the
        // folded `{ a: 7, b: 9 }`), so its per-field values are threaded down
        // by KEY. Passing `None` here instead left every sub-field at its type
        // default, silently zeroing the payload of a declaration initializer
        // like `var s: St = St.B { p: { a: 7, b: 9 } }` - and nothing runs at
        // load to repair it, unlike an assignment.
        let sub_lits: HashMap<&str, &Literal> = match &init_lit {
            Some(Literal::Record(rf)) => rf.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            _ => HashMap::default(),
        };
        let mut fmap = HashMap::default();
        for f in &sub_fields {
            let sub_label = format!("{label}.{}", f.name);
            fmap.insert(
                crate::intern::intern(&f.name),
                enum_payload_slot(
                    ctx,
                    &f.typ,
                    type_params,
                    args,
                    &sub_label,
                    sub_lits.get(f.name.as_str()).map(|l| (*l).clone()),
                    range,
                ),
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
/// Returns `None` — changing nothing — unless `obj` is a bare name that is a
/// constant (a `const`, or a `let` bound to a constant array/map, see
/// `lower_let_decl`), holds an array/map literal, and has NO binding of its own
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
        // A gate built further out for another constant of this name (an
        // importer's, around a namespaced body recording its own module's)
        // is not this constant's: this one gets a gate of its own.
        let other = ctx.immutable_containers.contains(&rec.node_id)
            && ctx.const_container_literal(name).is_some_and(|lit| {
                ctx.builder.module.nodes.get(&rec.node_id).and_then(|n| n.properties.get(&*sym::INITIAL_VALUE))
                    != Some(&lit)
            });
        if !other {
            return matches!(rec.storage, VarStorage::Array | VarStorage::Map).then(|| rec.clone());
        }
    } else if ctx.scope.get(name).is_some() {
        return None;
    }
    let lit = ctx.const_container_literal(name)?;
    // Prefer the type CHECKED for this very read over one re-derived from the
    // literal: typecheck's symbol type is what picked the method signature and
    // the element/value types the call was validated against, so taking any
    // other answer here would let the gate's port types disagree with the
    // signature the program was accepted under. The literal is the fallback for
    // a read typecheck recorded nothing for.
    let checked = unwrap_ref(&ctx.type_of(obj));
    let rec = const_container_gate(ctx, name, lit, checked, range)?;
    ctx.scope.insert(name, Binding::Var(rec.clone()));
    Some(rec)
}

/// A constant container `name` a body records (a namespaced body's module
/// constant) while a gate further out holds another constant of the name (the
/// importer's, materialized where the body can see it): every read of `name`
/// in the body would find that gate first, so the body's own is built and
/// bound here.
pub(super) fn own_constant_container(ctx: &mut LowerCtx, name: &str, lit: &Literal, range: &SourceRange) {
    if matches!(lit, Literal::Array(_) | Literal::Map(_)) && ctx.lookup_var(name).is_some() {
        materialize_const_container(ctx, &Expr::Ident { name: name.to_string(), range: range.clone() });
    }
}

/// A constant `let`/`const` container that SHADOWS a binding of the same name
/// (a `var`, an `in` port, a parameter, an earlier materialization): it cannot
/// stay lazy, since every runtime read would find the outer binding first, so
/// its immutable gate is built and bound here. A no-op for an unbound name.
pub(super) fn shadow_with_const_container(ctx: &mut LowerCtx, name: &str, lit: Literal, value: &Expr) {
    if ctx.scope.get(name).is_none() {
        return;
    }
    let checked = unwrap_ref(&ctx.type_of(value));
    if let Some(rec) = const_container_gate(ctx, name, lit, checked, value.range()) {
        ctx.scope.insert(name, Binding::Var(rec));
    }
}

/// Bind `const` parameter `name`, whose value is `lit` of type `ty`, in the
/// body's own frame: a record or enum value field by field, always; a scalar
/// as its literal and an array or map as its immutable container when an
/// outer binding of that name would otherwise answer a read of it. With no
/// outer binding a scalar or container read already resolves through the
/// constant environment, and nothing is bound for it. An array or map field
/// `body` never reads gets no container.
pub(super) fn bind_shadowing_const_param(
    ctx: &mut LowerCtx,
    name: &str,
    lit: Literal,
    ty: Type,
    range: &SourceRange,
    body: &crate::ast::Block,
) {
    if ctx.scope.get(name).is_none() && !matches!(lit, Literal::Record(_)) {
        return;
    }
    let read = field_reads(body, name);
    /// A constant value of type `ty` as literal gates: a record (or enum
    /// value) field by field, an array or map field as its immutable
    /// container labelled `label` when `read` holds it.
    fn literal_binding(
        ctx: &mut LowerCtx,
        label: &str,
        lit: Literal,
        ty: &Type,
        range: &SourceRange,
        read: &Option<HashSet<String>>,
    ) -> Option<Binding> {
        match lit {
            Literal::Record(fields) => {
                let mut out = HashMap::default();
                for (f, v) in fields {
                    let fty = match unwrap_ref(ty) {
                        Type::Record(tf) => tf.iter().find(|(n, _)| *n == f).map(|(_, t)| t.clone()),
                        _ => None,
                    };
                    let fty = fty.unwrap_or(Type::Any);
                    let path = format!("{label}.{f}");
                    let unread = read.as_ref().is_some_and(|r| {
                        !r.iter().any(|w| {
                            w == &path
                                || w.strip_prefix(path.as_str()).is_some_and(|rest| rest.starts_with('.'))
                                || path.strip_prefix(w.as_str()).is_some_and(|rest| rest.starts_with('.'))
                        })
                    });
                    if unread && matches!(v, Literal::Array(_) | Literal::Map(_)) {
                        continue;
                    }
                    let field = literal_binding(ctx, &path, v, &fty, range, read)?;
                    out.insert(crate::intern::intern(&f), field);
                }
                Some(Binding::Record(out))
            }
            lit @ (Literal::Array(_) | Literal::Map(_)) => {
                const_container_gate(ctx, label, lit, unwrap_ref(ty), range).map(Binding::Var)
            }
            lit => crate::lower::expr::wire_type_of_literal(&lit).map(|t| {
                Binding::Local(LocalRecord { port: crate::lower::expr::literal_node_range(ctx, range, t, lit) })
            }),
        }
    }
    let binding = literal_binding(ctx, name, lit, &ty, range, &read);
    if let Some(b) = binding {
        ctx.scope.insert(name, b);
    }
}

/// Every field path `body` reads whole off the name `root` (`q.i.s` for
/// `q.i.s[k]`, `q.i` for `let inner = q.i`, which reads every field below
/// it), or `None` when it uses `root` other than to read a field of it,
/// which may read every field.
fn field_reads(body: &crate::ast::Block, root: &str) -> Option<HashSet<String>> {
    use crate::ast::walk::{Ctx, Node, walk};
    fn path(e: &Expr, root: &str) -> Option<String> {
        match e {
            Expr::Ident { name, .. } if name == root => Some(name.clone()),
            Expr::FieldAccess { obj, field, .. } => Some(format!("{}.{field}", path(obj, root)?)),
            _ => None,
        }
    }
    let mut reads: HashMap<String, (usize, usize)> = HashMap::default();
    let (mut uses, mut field_uses) = (0usize, 0usize);
    walk(Node::Block(body), Ctx::default(), &mut |n, _| {
        if let Node::Expr(e) = n {
            match e {
                Expr::Ident { name, .. } if name == root => uses += 1,
                // A method's receiver (`q.i.f()`) is read whole.
                Expr::Call { callee, .. } => {
                    if let Expr::FieldAccess { obj, .. } = callee.as_ref()
                        && let Some(q) = path(obj, root)
                    {
                        reads.entry(q).or_default().0 += 1;
                    }
                }
                Expr::FieldAccess { obj, .. } => {
                    if let Some(p) = path(e, root) {
                        reads.entry(p).or_default().0 += 1;
                        match obj.as_ref() {
                            Expr::Ident { name, .. } if name == root => field_uses += 1,
                            inner => {
                                if let Some(q) = path(inner, root) {
                                    reads.entry(q).or_default().1 += 1;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        true
    });
    (uses == field_uses).then(|| reads.into_iter().filter(|(_, (all, deeper))| all > deeper).map(|(p, _)| p).collect())
}

/// An array/map literal ARGUMENT (`C(go, [10, 20, 30])`) as an immutable
/// container gate, the same form a `const` table gets, bound to no name. `None`
/// unless `e` is an array/map literal whose value is a compile-time constant.
pub(super) fn materialize_literal_container(
    ctx: &mut LowerCtx,
    e: &Expr,
    label: &str,
) -> Option<VarRecord> {
    let e = &*ctx.with_map_literals(e);
    if !matches!(e, Expr::Array { .. } | Expr::MapLit { .. }) {
        return None;
    }
    let lit = {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()?
    };
    let checked = unwrap_ref(&ctx.type_of(e));
    // A record/tuple/enum element is several columns, not one gate.
    let columnar = |t: &Type| matches!(t, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. });
    if matches!(&checked, Type::Array(t) | Type::Map(_, t) if columnar(t)) {
        return None;
    }
    const_container_gate(ctx, label, lit, checked, e.range())
}

/// The immutable container gate holding `lit`, labelled `label`: an ArrayVar
/// or MapVar with the contents baked into `InitialValue`, added to
/// `ctx.immutable_containers`. `None` for a literal that is not an array/map.
fn const_container_gate(
    ctx: &mut LowerCtx,
    label: &str,
    lit: Literal,
    checked: Type,
    range: &SourceRange,
) -> Option<VarRecord> {
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(label.to_string()));
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
            // `var` paths bake through (see `bake_string_bool`), and an integer
            // in a `float` table is the float it names.
            let items: Vec<Literal> = items
                .into_iter()
                .map(|lit| {
                    let lit = crate::const_eval::coerce_const_to_type(lit, &elem_type, ctx.consts.types());
                    bake_string_bool(lit, &elem_type)
                })
                .collect();
            properties.insert(*sym::INITIAL_VALUE, Literal::Array(items));
            let node_id = make_array_var_gate(ctx, &elem_type, properties, range);
            ctx.immutable_containers.insert(node_id);
            Some(VarRecord {
                node_id,
                inner_type: elem_type,
                get_node_for_handler: None,
                storage: VarStorage::Array,
            })
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
            let node_id = make_map_var_gate(ctx, &map_type, properties, range);
            ctx.immutable_containers.insert(node_id);
            Some(VarRecord {
                node_id,
                inner_type: map_type,
                get_node_for_handler: None,
                storage: VarStorage::Map,
            })
        }
        _ => None,
    }
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

/// The text a `var`'s storage gate is labelled with in-game: its `@label` /
/// `@label(expr)` when it has one, its source name otherwise.
fn var_label(ctx: &LowerCtx, d: &VarDecl) -> String {
    resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_lookup())
        .unwrap_or_else(|| d.name.clone())
}

pub(super) fn pre_declare_var(ctx: &mut LowerCtx, d: &VarDecl) {
    // An alias naming a container (`type Pts = P[]`) declares that container.
    let expanded = d.typ.as_ref().and_then(|te| container_alias(ctx, te));
    let typ = expanded.as_ref().or(d.typ.as_ref());
    // `resolve_local_type` monomorphizes a `T` annotation inside a generic mod
    // body and reads every alias where it is declared.
    // A gate read holds its first output, in a record's field too.
    let inner_type = typ
        .map(|te| ctx.resolve_local_type(te))
        .or_else(|| d.init.as_ref().and_then(|e| ctx.held_by_var_at(e.range())))
        .unwrap_or(Type::Any);
    // An unannotated array literal of enums (`var x = [c, d]`) is declared as
    // `var x: Hue[]` would be.
    let inferred_enum_array = match (&d.typ, &inner_type) {
        (None, Type::Array(elem)) => match elem.as_ref() {
            Type::Enum { name, args } if args.is_empty() => Some(TypeExpr::Array {
                inner: Box::new(TypeExpr::Name { name: name.clone(), range: d.range.clone() }),
                range: d.range.clone(),
            }),
            _ => None,
        },
        _ => None,
    };
    let typ = typ.or(inferred_enum_array.as_ref());
    // An unannotated `var` of a record or tuple (`var r = m.get(k)` on a map
    // of records) is declared as its type written out would be.
    let written_out = match (typ, &inner_type) {
        (None, Type::Record(_) | Type::Tuple(_)) => Some(crate::lower::context::type_expr_of(&inner_type, &d.range)),
        _ => None,
    };
    let typ = typ.or(written_out.as_ref());

    // `var foo: T[]` is an array — desugar to an ArrayVar gate so the array
    // methods actually work. A `= [..]` initializer carries its constant
    // literals inline (mirrors the map path below).
    if let Type::Array(elem) = &inner_type {
        // `var pts: Rec[]` — a record ARRAY decomposes into one parallel array
        // per field (`pts.x`, `pts.y`), bound as a `Binding::Record` of
        // `VarStorage::Array` fields. Every element op fans out across them (see
        // `lower_record_array_method`). `record_fields_of` follows a `type P = {…}`
        // alias on the element the resolved `Type` would not preserve.
        if let Some(TypeExpr::Array { inner: elem_te, .. }) = typ
            && let Some(fields) = ctx.record_or_tuple_fields(elem_te)
        {
            let label = var_label(ctx, d);
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
            bake_const_columns(ctx, &d.name, d.init.as_ref());
            return;
        }
        // `var arr: S[]` for an enum `S`: an enum value spans several wires
        // just as a record does, so the array needs the same parallel columns,
        // one per `__disc`/payload slot. A single ArrayVar collapsed the tag
        // and the payload together, and a push of a constructed value found no
        // column to carry it and emitted no `Value` wire at all.
        if let Some(TypeExpr::Array { inner: elem_te, .. }) = typ
            && let Some(cols) = enum_container_columns(ctx, elem_te, &d.range)
        {
            let label = var_label(ctx, d);
            declare_record_container(
                ctx,
                &d.name,
                VarStorage::Array,
                &cols,
                &label,
                None,
                None,
                &d.range,
            );
            if let Some(Expr::Array { elements, .. }) = &d.init {
                bake_record_array_init(ctx, &d.name, elements);
            }
            bake_const_columns(ctx, &d.name, d.init.as_ref());
            return;
        }
        let elem_type = elem.as_ref().clone();
        let mut properties = HashMap::default();
        let label = var_label(ctx, d);
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
        } else if let Some(Literal::Array(lits)) = d.init.as_ref().and_then(|e| const_container_init(ctx, e)) {
            let lits = lits.into_iter().map(|lit| bake_string_bool(lit, &elem_type)).collect();
            properties.insert(intern_static("InitialValue"), Literal::Array(lits));
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
        if let Some(val_te) = map_value_typeexpr(typ)
            && let Some(fields) = ctx.record_or_tuple_fields(val_te)
        {
            let label = var_label(ctx, d);
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
            bake_const_columns(ctx, &d.name, d.init.as_ref());
            return;
        }
        // The map twin of the enum-element array above: one parallel map per
        // column, keyed identically.
        if let Some(val_te) = map_value_typeexpr(typ)
            && let Some(cols) = enum_container_columns(ctx, val_te, &d.range)
        {
            let label = var_label(ctx, d);
            declare_record_container(
                ctx,
                &d.name,
                VarStorage::Map,
                &cols,
                &label,
                None,
                Some(key_ty),
                &d.range,
            );
            if let Some(Expr::MapLit { entries, .. }) = &d.init {
                bake_record_map_init(ctx, &d.name, entries);
            }
            bake_const_columns(ctx, &d.name, d.init.as_ref());
            return;
        }
        let (key_ty, value_ty) = (key_ty.as_ref().clone(), value_ty.as_ref().clone());
        let mut properties = HashMap::default();
        let label = var_label(ctx, d);
        properties.insert(*sym::NAME_LABEL, Literal::String(label));
        bake_map_init(ctx, &mut properties, &d.name, &d.init, &key_ty, &value_ty);
        if let Some(Literal::Map(pairs)) = d.init.as_ref().and_then(|e| const_container_init(ctx, e)) {
            let pairs = pairs
                .into_iter()
                .map(|(k, v)| (coerce_literal_to_type(k, &key_ty), coerce_literal_to_type(v, &value_ty)))
                .collect();
            properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
        }
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
    if let Type::Enum { args, .. } = &inner_type {
        let label = var_label(ctx, d);
        let args = args.clone();
        declare_enum_container(ctx, &d.name, &inner_type, &args, &label, d.init.as_ref(), &d.range);
        return;
    }

    // `var p: Rec` — a record variable decomposes into one per-field `Pseudo_Var`
    // (recursing for nested records), bound as a `Binding::Record` so `p.field`
    // reads/writes the right backing gate. Without this a record var collapsed
    // to a single `Pseudo_Var` and `p.x` lowered to a bogus `SplitVector.X`
    // swizzle — a silent miscompile. Mirrors the record INPUT-PORT expansion in
    // `pre_declare_input`; `record_fields_of` follows a `type P = { … }` alias
    // that `resolve_local_type`'s empty alias table cannot.
    if let Some(fields) = typ.and_then(|te| ctx.record_or_tuple_fields(te)) {
        let label = var_label(ctx, d);
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
        bake_const_record(ctx, &d.name, d.init.as_ref());
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
    let label = var_label(ctx, d);
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    if let Some(lit) = init_lit {
        properties.insert(*sym::INITIAL_VALUE, lit);
    }

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR,
        source_range: d.range.clone(),
        ports: GateIO::var_storage(inner_type.clone()),
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
                PortSpec::new(*sym::INPUT, inner_type.clone()),
                PortSpec::new(*sym::TICKS_TO_WAIT, Type::Int),
            ],
            outputs: vec![PortSpec::new(*sym::OUTPUT, inner_type.clone())],
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
    if let Some(fields) = ctx.record_or_tuple_fields(&d.element_type) {
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
    if let Some(fields) = ctx.record_or_tuple_fields(&d.value_type) {
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

/// One boundary pin per LEAF field of a record-typed `in` port, recursing into
/// nested record fields. Flat records were expanded field-by-field here, but a
/// NESTED record field got a single pin for its whole sub-record, so a copy out
/// of the port wrote only the flat fields and the nested ones silently vanished.
/// Pin names join with `_` at every level (`p_i_m`), matching the flat scheme.
fn record_input_pins(
    ctx: &mut LowerCtx,
    te: &TypeExpr,
    prefix: &str,
    range: &SourceRange,
    depth: u32,
) -> HashMap<crate::intern::Sym, Binding> {
    if depth >= MAX_RECORD_FIELD_DEPTH {
        return HashMap::default();
    }
    let fields = ctx.record_or_tuple_fields(te).unwrap_or_default();
    let mut out = HashMap::default();
    for field in &fields {
        let port_name = format!("{prefix}_{}", field.name);
        if ctx.record_or_tuple_fields(&field.typ).is_some() {
            let inner = record_input_pins(ctx, &field.typ, &port_name, range, depth + 1);
            out.insert(crate::intern::intern(&field.name), Binding::Record(inner));
            continue;
        }
        let ft = ctx.resolve_local_type(&field.typ);
        let node_id = ctx.add_input(&port_name, ft.clone(), range.clone());
        // Array / Map / ref fields of a record-typed input port bind a
        // container ref-port (see `container_binding`); a scalar field is a
        // plain by-value input.
        let binding = match super::context::container_binding(ctx.ref_storage(None, &field.typ), &ft) {
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
        out.insert(crate::intern::intern(&field.name), binding);
    }
    out
}

/// The `out` twin of [`record_input_pins`] - one output pin per leaf field,
/// recursing into nested records so a nested field's pins are wired rather than
/// left dangling.
pub(super) fn record_output_pins(
    ctx: &mut LowerCtx,
    te: &TypeExpr,
    prefix: &str,
    range: &SourceRange,
    depth: u32,
) -> HashMap<crate::intern::Sym, Binding> {
    if depth >= MAX_RECORD_FIELD_DEPTH {
        return HashMap::default();
    }
    let fields = ctx.record_or_tuple_fields(te).unwrap_or_default();
    let mut out = HashMap::default();
    for field in &fields {
        let port_name = format!("{prefix}_{}", field.name);
        if ctx.record_or_tuple_fields(&field.typ).is_some() {
            let inner = record_output_pins(ctx, &field.typ, &port_name, range, depth + 1);
            out.insert(crate::intern::intern(&field.name), Binding::Record(inner));
            continue;
        }
        let ft = ctx.resolve_local_type(&field.typ);
        let node_id = ctx.add_output(&port_name, ft.clone(), range.clone());
        out.insert(
            crate::intern::intern(&field.name),
            Binding::Output(NodeRecord { node_id, ty: ft }),
        );
    }
    out
}

pub(super) fn pre_declare_input(ctx: &mut LowerCtx, d: &InDecl) {
    // A record-typed input port (inline `{ … }`, a non-generic `type P = { … }`,
    // or a generic `type Pair<T> = { … }` instantiated as `Pair<int>`) dissolves
    // into one sub-port per field, bound as a `Record` so `p.field` reads the
    // right sub-port. Without this a record port collapsed to a single `any`
    // port and its field accesses lowered to `_Unsupported`/swizzle gates —
    // mirrors the standalone-chip input expansion in `lower::mod`.
    if ctx.record_or_tuple_fields(&d.typ).is_some() {
        let record_fields = record_input_pins(ctx, &d.typ, &d.name, &d.range, 0);
        ctx.scope.insert(&d.name, Binding::Record(record_fields));
        return;
    }
    // A payload-free enum crosses the boundary as one wire, its tag, so the
    // name binds the enum value whose `__disc` is the port.
    let unit_enum = unit_enum_port(ctx, &ctx.resolve_local_type(&d.typ));
    let t = if unit_enum { Type::Int } else { ctx.resolve_local_type(&d.typ) };
    let node_id = ctx.add_input(&d.name, t.clone(), d.range.clone());
    apply_port_side(ctx, node_id, d.side, d.invisible, &d.range);
    if let Some(label) =
        resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_lookup())
    {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label));
        }
    }
    let input = Binding::Input(NodeRecord { node_id, ty: t });
    if unit_enum {
        let mut value = HashMap::default();
        value.insert(crate::intern::intern("__disc"), input);
        ctx.scope.insert(&d.name, Binding::Record(value));
        return;
    }
    ctx.scope.insert(&d.name, input);
}

/// Whether a port of type `t` carries a payload-free enum (a built-in game enum
/// included), which crosses a top-level port as one wire: its tag.
pub(super) fn unit_enum_port(ctx: &LowerCtx, t: &Type) -> bool {
    matches!(t, Type::Enum { .. } if ctx.enum_defs.of(t).is_some_and(|d| {
        d.variants.iter().all(|v| matches!(v.payload, crate::typecheck::enums::Payload::Unit))
    }))
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
        && let Some(fields) = ctx.record_or_tuple_fields(te)
    {
        let _ = &fields;
        let record_fields = record_output_pins(ctx, te, name, range, 0);
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
    // string port. A `*T` port carries a ref, so it keeps its ref type; an
    // array or map ref is the container itself.
    let t = if let (Some(te), Some(_)) = (typ, value) {
        ctx.resolve_local_type(te).container_ref_as_value()
    } else if let Some(v) = value {
        // A port written values of several types is stored as their join.
        let r = v.range();
        match ctx.resolutions.untyped_port_types.get(&(r.file.clone(), r.start.offset, r.end.offset)) {
            Some(joined) => joined.clone(),
            None => ctx.type_of(v).container_ref_as_value(),
        }
    } else if let Some(te) = typ {
        if crate::lower::call::is_exec_type(ctx, te) { Type::Exec } else { ctx.resolve_local_type(te) }
    } else {
        Type::Any
    };
    // A payload-free enum crosses the boundary as one wire, its tag: bound as
    // an enum value whose `__disc` is the pin, like a record output's fields.
    if boundary && unit_enum_port(ctx, &t) {
        let node_id = ctx.add_output(name, Type::Int, range.clone());
        apply_port_side(ctx, node_id, side, invisible, range);
        let mut fields = HashMap::default();
        fields.insert(
            crate::intern::intern("__disc"),
            Binding::Output(NodeRecord { node_id, ty: Type::Int }),
        );
        ctx.scope.insert(&crate::lower::context::output_scope_key(name), Binding::Record(fields));
        return;
    }
    let node_id = ctx.add_output(name, t.clone(), range.clone());
    apply_port_side(ctx, node_id, side, invisible, range);
    if let Some(label) = resolve_label_text(label, label_expr, &ctx.const_lookup())
        && let Some(node) = ctx.builder.module.nodes.get_mut(&node_id)
    {
        std::sync::Arc::make_mut(&mut node.properties).insert(*sym::NAME_LABEL, Literal::String(label));
    }
    ctx.scope.insert(
        &crate::lower::context::output_scope_key(name),
        Binding::Output(NodeRecord { node_id, ty: t }),
    );
}

/// Register every `out` declared inside a top-level handler body as a module
/// boundary port, so `lookup_output` resolves it when the body lowers.
///
/// The site set comes from `ast::handler_port_sites`, shared with typecheck's
/// `register_handler_outputs` and `scoped_refs`'s widening pass so the three
/// cannot claim different sites. The backing-var rule is what keeps two sites
/// from fanning into one rerouter.
///
/// A name that already resolves as an output binds to that port instead of
/// minting a second one, which is why the caller runs this pass AFTER the whole
/// pass-1 loop.
///
/// `boundary` is `pre_declare_output`'s, and matches whatever the enclosing
/// module's own top-level `out` passes: true for the entry file, false for a
/// namespaced module's members.
pub(super) fn pre_declare_handler_outputs(ctx: &mut LowerCtx, block: &Block, boundary: bool) {
    for o in crate::ast::handler_port_sites(block) {
        if ctx.scope.get(&crate::lower::context::output_scope_key(&o.name)).is_some() {
            continue;
        }
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
                boundary,
                &o.range,
            )
        });
    }
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
