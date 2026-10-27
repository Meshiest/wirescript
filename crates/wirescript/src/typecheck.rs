//! The typechecker: a two-pass walk over the AST (declaration registration,
//! then per-declaration checking) with the scope stack and exec/pure context
//! tracked inline.
//!
//! It produces side tables rather than a typed AST: `type_of_expr` (keyed by
//! each expression's file and byte range) and `op_resolutions` (the catalog
//! `OpRule` chosen for every operator), which lowering consumes.
//!
//! Reading a `var`:
//! - in exec context, `n` auto-derefs (`Exec_Var_Get`) and types as the inner T;
//! - in a pure sink expecting `ref T`, `n` is the `VarRef` port;
//! - in a pure sink expecting `T`, it is an error (WS006): write `n.Value`;
//! - `*n` is an explicit deref and needs exec context;
//! - `n.Value` is the delayed read and always yields T.

use crate::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::scope::Scope as ScopeStack;

use crate::ast::*;
use crate::catalog::calls::find_call;
use crate::catalog::events::{events, find_event};
use crate::catalog::operators::{OpRule, resolve_op};
use crate::diagnostic::{Diagnostic, RangeKey, Severity, SourceRange, StartKey};
use crate::ir::Type;
use crate::types::classes::mask_contains;
use crate::types::coerce::{CoerceRule, coerce, widening_join_all};

pub(crate) mod infer;
mod sig;
pub(crate) use sig::{CallSignature, Param, ParamKind, check_args};

mod ctx;
pub use ctx::*;
pub(crate) mod narrow;
pub(crate) use narrow::NarrowedRead;
pub(crate) mod resolution;
pub(crate) use resolution::{
    ContainerKind, MethodTarget, ResolutionKey, Resolutions, classify_method,
};
pub(crate) mod enums;
pub(crate) mod patterns;
mod resolve;
pub(crate) use resolve::*;
mod labels;
use labels::*;
mod parked_kick;
mod styles;
use parked_kick::*;
mod config;
use config::*;
mod custom_events;
pub use custom_events::*;
mod let_binding;
use let_binding::*;
mod call;
pub(crate) mod convert;
pub(crate) mod props;
use call::*;
mod handler;
use handler::*;
mod stmt;
pub(crate) use stmt::*;
mod decl;
mod fn_decl;
use decl::*;
mod instances;
mod register;
mod shadow;
mod untyped_arrays;
use register::*;

/// The most rounds a program with unannotated ports written values of several
/// types is checked for (see `ctx::finish_untyped_ports`): each round seeds
/// the types the previous one joined.
const UNTYPED_PORT_ROUNDS: usize = 4;

pub fn typecheck(script: &Script, ce_slots: &CeSlotMap) -> TypeCheckResult {
    let mut port_seeds = Arc::new(PortSeeds::default());
    for round in 1.. {
        let last = round == UNTYPED_PORT_ROUNDS;
        match typecheck_round(script, ce_slots, &port_seeds, last) {
            Ok(result) => return result,
            Err(next_ports) => port_seeds = Arc::new(next_ports),
        }
    }
    unreachable!()
}

fn typecheck_round(
    script: &Script,
    ce_slots: &CeSlotMap,
    port_seeds: &Arc<PortSeeds>,
    last: bool,
) -> Result<TypeCheckResult, PortSeeds> {
    let mut ctx = TypeCheckCtx::new(ce_slots);
    ctx.untyped_port_seeds = Arc::clone(port_seeds);
    ctx.imports = Arc::clone(&script.imports);
    // A module-level `@nofold` marks the whole program, as `lower::lower` seeds
    // `LowerCtx::nofold_depth` from it. Lowering then builds a real `Branch` for a
    // constant `if`, so both blocks are checked (the `Stmt::If` arm in `stmt.rs`).
    ctx.nofold_depth = u32::from(script.no_fold);
    register_builtin_events(&mut ctx);
    // Seed the `Option<T>`/`Result<T, E>` prelude BEFORE the pre-passes below,
    // same placement as `register_builtin_events` - so a user enum that
    // redeclares either name still resolves (see `register_builtin_enums`'s
    // own doc comment for the overwrite-then-flag ordering).
    register_builtin_enums(&mut ctx);

    // Pre-pass: collect every generic type alias (`type Pair<T> = ...`) before
    // any decl registration/resolution runs, so a use resolves regardless of
    // whether its alias is declared earlier or later in the file.
    collect_generic_aliases(&mut ctx, &script.decls);
    // Pre-pass: assign every enum's discriminants (auto-numbering + WS064 for
    // collisions) and register its `EnumDef` before any decl is checked, so
    // a use resolves regardless of declaration order (mirrors the generic
    // alias pre-pass just above).
    enums::collect_enum_defs(&mut ctx, &script.decls);

    ctx.top_names = Arc::new(crate::scope::top::TopNames::of(&script.decls));
    warn_top_redeclarations(&mut ctx, &script.decls);
    warn_namespace_redeclarations(&mut ctx, &script.decls);
    // Two-pass: register all top-level decls first so forward refs resolve.
    for d in &script.decls {
        register_decl(&mut ctx, d);
        ctx.keep_top_dup_symbols(d);
    }
    // After registration, so a payload field naming a record alias resolves:
    // reject any payload field that would need container storage (WS069).
    enums::check_payload_storage(&mut ctx, &script.decls);
    // Constant `let`s, resolved before any decl is checked so a `var` / `array`
    // initializer may name one. Built from the same function lowering uses, so
    // the two can't disagree about what counts as a compile-time constant.
    ctx.consts = crate::const_scope::ConstScope::for_program(
        &script.decls,
        &ctx.enum_defs,
        &script.imports,
        &ctx.top_names,
    );
    untyped_arrays::type_constant_elements(&mut ctx, &script.decls);
    // `@label(expr)` on a port, chip or nested var must fold to a constant (the
    // text is baked): a runtime value has nowhere to host a wire (WS040). A
    // top-level `var` carries a wireable text component, so a runtime label there
    // is valid and is checked in `check_dynamic_var_labels`. One dedicated walk,
    // since these declarations are checked from several paths.
    check_label_exprs(&mut ctx, &script.decls);
    // A plain `emit X` that a same-chain `await X` follows parks the chain forever
    // (the emit fires before the await arms): WS053, suggesting `buffer emit X`.
    check_parked_kicks(&mut ctx, &script.decls);
    // The module-level output frame, so a statement targeting a top-level `out`
    // (`emit o = v`) is checked against its type through `current_output_ty`.
    // Pushed once for both check loops below; a chip pushes its own frame per
    // combination on top, and the nearest frame wins.
    //
    // It holds every top-level `out` (an unannotated one as `Any`, which coerces
    // as Same, so writes to it stay unchecked), so its length equals lowering's
    // `output_count()`: `return <value>` wires into a lone output only when that
    // count is 1 (`lower/stmt.rs`), and the `Stmt::Return` check keys on the same
    // count.
    //
    // `resolve_type_expr` reports errors, and the `TopDecl::Out` check resolves
    // each annotation again, so this frame build's diagnostics are dropped.
    let module_outs: Vec<EventDataField> = ctx.discarding_diagnostics(|ctx| {
        script
            .decls
            .iter()
            .filter_map(|d| match d {
                TopDecl::Out(b) => Some(EventDataField {
                    name: b.name.clone(),
                    ty: match &b.typ {
                        Some(te) => resolve_type_expr(ctx, te),
                        None => Type::Any,
                    },
                    is_const: false,
                }),
                _ => None,
            })
            .collect()
    });
    ctx.out_ctx.push(module_outs);
    let mut saw_handler = false;
    // Named chip and mod bodies are checked after everything else: top-level
    // `let` types are inferred during this pass, so a body checked early could not
    // see lets declared later (where imported mods land relative to the constants
    // they read). Signatures were registered in pass 1.
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
        // The top level is read by position: a name declared there more than
        // once means the declaration visible here.
        ctx.top_view(crate::scope::top::ReadAt::code(d.range()));
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d);
        }
        ctx.keep_top_dup_symbols(d);
    }
    for (exec_wrap, d) in deferred_chips {
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d);
        }
    }
    // Every body the bound combinations cover is checked by now; check the
    // instantiations calls use beyond those.
    instances::check_demanded(&mut ctx);
    ctx.settle_string_bound_hints();
    explain_hidden_enum_types(&mut ctx);
    finish_untyped_ports(&mut ctx, &script.decls, last);
    if let Some(ports) = ctx.untyped_port_next.take() {
        return Err(ports);
    }
    ctx.out_ctx.pop();

    // A runtime `@label(expr)` on a top-level `var` is type-checked now that every
    // top-level symbol is declared; a constant label bakes statically and is skipped.
    check_dynamic_var_labels(&mut ctx, script);
    // `@color` / `@material` anywhere: the value's type, and a constant
    // wherever a runtime one has no brick to wire into.
    styles::check_style_exprs(&mut ctx, script);

    // Whole-program pass: `SendCustomEvent("name", ...)` data whose wire types
    // disagree with the `on CustomEvent("name") -> (...)` receiver's declared
    // params.
    // Runs last so every arg has an inferred type in `type_of_expr`.
    // The custom-event pass reads already-inferred arg types while emitting
    // diagnostics; move the map out to satisfy the borrow checker, then restore.
    let type_of_expr = std::mem::take(&mut ctx.type_of_expr);
    // `ce_slots` is `Copy` (a shared reference), so this doesn't hold `ctx`
    // borrowed across the call below. On pass 1, `typecheck`'s `ce_slots` arg
    // is empty, so unannotated receiver slots stay unresolved; pass 2 (driven
    // by `typecheck_with_inference`) passes the real inferred map, so this
    // WS030 check sees inferred receiver types too.
    let ce_slots = ctx.ce_slots;
    check_custom_event_types(&mut ctx, script, &type_of_expr, ce_slots);
    ctx.type_of_expr = type_of_expr;

    // Every body is checked by now, so every call's callee has recorded the
    // storage it uses.
    shadow::report(&mut ctx);
    ctx::reserved_names(&mut ctx);

    // A port's stored type and a type argument hold a gate read as the
    // record of its outputs.
    let r = &mut ctx.resolutions;
    for t in r
        .untyped_port_types
        .values_mut()
        .chain(r.call_type_args.values_mut().flatten())
    {
        if t.holds_outputs() {
            *t = t.as_record();
        }
    }
    Ok(TypeCheckResult {
        type_of_expr: ctx.type_of_expr,
        op_resolutions: ctx.op_resolutions,
        if_contexts: ctx.if_contexts,
        var_read_contexts: ctx.var_read_contexts,
        narrowed_reads: ctx.narrowed_reads,
        diagnostics: ctx.diagnostics,
        dropped_ranges: ctx.dropped_ranges,
        resolutions: ctx.resolutions,
        imports: ctx.imports,
        #[cfg(test)]
        port_sites: ctx.port_sites,
        #[cfg(test)]
        ident_decls: ctx.ident_decls,
        #[cfg(test)]
        demanded_instantiations: ctx.instances.demanded(),
    })
}

/// `WS108` for each top-level declaration of a name the top level of its
/// file declared before: the later one hides the earlier from its position
/// on (see `scope::top`). Two storage declarations of one name are `WS013`
/// at registration instead. A module imported `import * as` too is warned
/// about once, by [`warn_namespace_redeclarations`].
fn warn_top_redeclarations(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    let top = Arc::clone(&ctx.top_names);
    let namespaced: crate::collections::HashSet<Arc<str>> = decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Namespace(ns) => Some(ns.decls.iter().map(|nd| Arc::clone(&nd.range().file))),
            _ => None,
        })
        .flatten()
        .collect();
    let mut found: Vec<(&str, &crate::scope::top::TopDup, &crate::scope::top::TopDup)> = top
        .dups()
        .flat_map(|(name, decls)| decls.windows(2).map(move |w| (name, &w[0], &w[1])))
        .filter(|(_, before, after)| !(before.storage && after.storage))
        .filter(|(_, _, after)| !namespaced.contains(&after.range.file))
        .collect();
    found.sort_by_key(|(_, _, after)| after.range.start_key());
    for (name, before, after) in found {
        let line = before.range.start.line;
        let message = if after.storage {
            format!(
                "'{name}' is declared again here, and the top level already declares it on line {line}: from \
                 this declaration on, '{name}' names this one, and so does a read above line {line}, since \
                 storage is declared ahead of everything at the top level; a read in between names the one on \
                 line {line}. Rename one of them to read both"
            )
        } else {
            format!(
                "'{name}' is declared again here, and the top level already declares it on line {line}: from \
                 this declaration on, '{name}' names this one and the one on line {line} is hidden, while a read \
                 above it still names that one. Rename one of them to read both"
            )
        };
        ctx.warn("WS108", message, after.range.clone());
    }
}

/// A plain `unknown type` naming an enum a file sees, but which the program
/// hides behind its own enum of that name, says so (see
/// `infer::hidden_enum_message`), as the enum's other reads there do.
fn explain_hidden_enum_types(ctx: &mut TypeCheckCtx<'_>) {
    let mut found = Vec::new();
    for (i, d) in ctx.diagnostics.iter().enumerate() {
        let Some(name) = d
            .message
            .strip_prefix("unknown type '")
            .and_then(|r| r.strip_suffix('\''))
        else {
            continue;
        };
        if d.code == "WS002"
            && let Some(message) = infer::hidden_enum_message(ctx, name, &d.range)
        {
            found.push((i, message));
        }
    }
    for (i, message) in found {
        ctx.diagnostics[i].message = message;
    }
}

/// `WS108` for a name a module imported `import * as` declares more than once
/// at its top level. Its namespace holds one declaration of the name, the
/// last, which every read in the module and through the namespace names;
/// once per declaration, however many paths reach the module.
fn warn_namespace_redeclarations(ctx: &mut TypeCheckCtx<'_>, decls: &[TopDecl]) {
    let mut found: Vec<(String, crate::diagnostic::SourceRange, u32)> = Vec::new();
    let mut seen = crate::collections::HashSet::default();
    for d in decls {
        let TopDecl::Namespace(ns) = d else { continue };
        let mut first: crate::collections::HashMap<(Arc<str>, String), u32> = HashMap::default();
        for nd in &ns.decls {
            let r = nd.range();
            for (name, _) in crate::scope::top::declared_names_of(nd) {
                match first.get(&(Arc::clone(&r.file), name.clone())) {
                    Some(line)
                        if seen.insert((Arc::clone(&r.file), r.start.offset, name.clone())) =>
                    {
                        found.push((name, r.clone(), *line));
                    }
                    Some(_) => {}
                    None => {
                        first.insert((Arc::clone(&r.file), name), r.start.line);
                    }
                }
            }
        }
    }
    for (name, range, line) in found {
        // Imported flat as well, its flat reads name the last one too.
        let elsewhere = if ctx.top_names.is_dup(&range.file, &name) {
            "its flat import here reads it so too; compiled alone"
        } else {
            "imported flat or compiled alone"
        };
        ctx.warn(
            "WS108",
            format!(
                "'{name}' is declared again here, and the top level already declares it on line {line}: this \
                 module is imported with `import * as`, whose namespace holds only this last declaration, so \
                 every read of '{name}' in the module names this one, the reads above it too ({elsewhere}, a \
                 read above names the one on line {line}). Rename one of them to read both"
            ),
            range,
        );
    }
}

/// Two-pass typecheck. Pass 1 checks without custom-event slot inference (as
/// `typecheck` does), then unannotated receiver slots are inferred from in-unit
/// senders (`infer_custom_event_slots`) over pass 1's `type_of_expr`; when any
/// was inferred, pass 2 checks again with them, so bodies and WS030 see the
/// inferred types. Pass 2's diagnostics include the inference's (WS042). The
/// map is returned too, so `lower` picks each slot's wire-port variant.
pub fn typecheck_with_inference(script: &Script) -> (TypeCheckResult, CeSlotMap) {
    let empty = CeSlotMap::default();
    let pass1 = typecheck(script, &empty);
    let (map, infer_diags) = infer_custom_event_slots(script, &pass1.type_of_expr);
    if map.is_empty() {
        return (pass1, map); // no unannotated custom-event slots -> single pass
    }
    let mut pass2 = typecheck(script, &map);
    pass2.diagnostics.extend(infer_diags);
    (pass2, map)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ref_tests;

#[cfg(test)]
mod resolution_tests;
