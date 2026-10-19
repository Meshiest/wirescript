//! The typechecker: a two-pass walk over the AST (decl registration, then
//! per-decl checking), with the scope stack and exec/pure context tracking
//! fused inline.
//!
//! Walks the AST producing a side `typeOfExpr` map (keyed by each
//! expression's source-range start offset) so we don't need to rebuild
//! the AST as a typed parallel. `opResolutions` records the catalog
//! `OpRule` chosen for every BinOp/UnOp; the lower phase consumes it.
//!
//! Identifier semantics for `var`:
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
use crate::types::classes::mask_contains;
use crate::types::coerce::{CoerceRule, coerce, widening_join_all};

pub(crate) mod infer;
mod sig;
pub use sig::{CallSignature, Param, ParamKind, check_args};

mod ctx;
pub use ctx::*;
pub(crate) mod narrow;
pub use narrow::NarrowedRead;
pub mod resolution;
pub use resolution::{ContainerKind, MethodTarget, Resolutions, ResolutionKey, classify_method};
pub mod enums;
pub mod patterns;
mod resolve;
pub(crate) use resolve::*;
mod labels;
use labels::*;
mod styles;
mod parked_kick;
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
mod register;
mod shadow;
mod instances;
mod untyped_arrays;
use register::*;

/// The most rounds a program with unannotated ports written values of several
/// types is checked for (see `ctx::finish_untyped_ports`): each round seeds
/// the types the previous one joined.
const UNTYPED_PORT_ROUNDS: usize = 4;

pub fn typecheck(script: &Script, file: &str, ce_slots: &CeSlotMap) -> TypeCheckResult {
    let mut port_seeds = Arc::new(PortSeeds::default());
    for round in 1.. {
        let last = round == UNTYPED_PORT_ROUNDS;
        match typecheck_round(script, file, ce_slots, &port_seeds, last) {
            Ok(result) => return result,
            Err(next_ports) => port_seeds = Arc::new(next_ports),
        }
    }
    unreachable!()
}

fn typecheck_round(
    script: &Script,
    file: &str,
    ce_slots: &CeSlotMap,
    port_seeds: &Arc<PortSeeds>,
    last: bool,
) -> Result<TypeCheckResult, PortSeeds> {
    let mut ctx = TypeCheckCtx::new(file, ce_slots);
    ctx.untyped_port_seeds = port_seeds.clone();
    ctx.imports = script.imports.clone();
    // A module-level `@nofold` marks the WHOLE program, exactly as
    // `lower::lower` seeds `LowerCtx::nofold_depth` from this same flag.
    // Under `@nofold` lowering still builds a real `Branch` for a constant
    // `if`, so this stage must keep checking BOTH blocks — see the `Stmt::If`
    // arm in `stmt.rs`.
    ctx.nofold_depth = script.no_fold as u32;
    register_builtin_events(&mut ctx);
    // Seed the `Option<T>`/`Result<T, E>` prelude BEFORE the pre-passes below,
    // same placement as `register_builtin_events` - so a user enum that
    // redeclares either name still resolves (see `register_builtin_enums`'s
    // own doc comment for the overwrite-then-flag ordering).
    register_builtin_enums(&mut ctx);

    // Pre-pass: collect every generic type alias (`type Pair<T> = …`) before
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
    ctx.consts =
        crate::const_scope::ConstScope::for_program(&script.decls, &ctx.enum_defs, &script.imports, &ctx.top_names);
    untyped_arrays::type_constant_elements(&mut ctx, &script.decls);
    // `@label(expr)` on a port/chip/nested-var must fold to a compile-time
    // constant (the folded text is baked as the label) — a runtime value there
    // has nowhere to host a wire, so it stays a WS040 error. A TOP-LEVEL `var`
    // is the exception: it carries a wireable text component, so a runtime label
    // there is a valid dynamic label (checked separately in
    // `check_dynamic_var_labels`, after all top-level symbols are declared). A
    // single dedicated walk, since these decls are checked from several
    // different call paths below (top-level, nested in chip/anon-chip bodies,
    // statement-level).
    check_label_exprs(&mut ctx, &script.decls);
    // A plain `emit X` that a same-chain `await X` follows parks the chain forever
    // (the emit fires before the await arms): WS053, suggesting `buffer emit X`.
    check_parked_kicks(&mut ctx, &script.decls);
    // Module-level output frame: the declared types of every ANNOTATED
    // top-level `out o: T`, so a body statement targeting it (`emit o = v`)
    // can be checked against `T` via `current_output_ty`. An unannotated
    // `out y = x` has no declared type to check against — it's excluded here
    // (handled by `TopDecl::Out` directly).
    // Pushed once for both check loops below (including the deferred named
    // chips, which see it as their enclosing scope's frame — a chip pushes
    // its own frame on top per combo, so the nearest-wins lookup still finds
    // the chip's own outputs first).
    //
    // Include EVERY top-level `out` (an unannotated `out y = x` as `Any`), not
    // just the annotated ones, so this frame's length equals lowering's
    // `output_count()`. `return <value>` only wires into a lone output when
    // `output_count() == 1` (`lower/stmt.rs`), so the `Stmt::Return` arm's
    // single-output check must key on the SAME total count — otherwise a lone
    // annotated out sitting beside an unannotated one would be checked here
    // while lowering drops the value. An `Any` frame entry coerces to Same, so
    // `emit`/`out` to an unannotated output stays unchecked.
    //
    // `resolve_type_expr` is NOT pure — it emits (e.g. WS002 "unknown type").
    // Each annotation is resolved (and any error reported) again by the
    // canonical `TopDecl::Out` check below, so DISCARD any diagnostics this
    // frame build produces: snapshot the length first, then truncate back to it.
    let diag_mark = ctx.diagnostics.len();
    let module_outs: Vec<EventDataField> = script
        .decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Out(b) => Some(EventDataField {
                name: b.name.clone(),
                ty: match &b.typ {
                    Some(te) => resolve_type_expr(&mut ctx, te),
                    None => Type::Any,
                },
                is_const: false,
            }),
            _ => None,
        })
        .collect();
    ctx.diagnostics.truncate(diag_mark);
    ctx.out_ctx.push(module_outs);
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

    // Runtime `@label(expr)` on a top-level `var`: type-check the expression
    // now that every top-level symbol is declared (an undefined ref or a bad
    // type surfaces here). A constant label is skipped — it bakes statically.
    check_dynamic_var_labels(&mut ctx, script);
    // `@color` / `@material` anywhere: the value's type, and a constant
    // wherever a runtime one has no brick to wire into.
    styles::check_style_exprs(&mut ctx, script);

    // Whole-program pass: `SendCustomEvent("name", …)` data whose wire types
    // disagree with the `on CustomEvent("name") -> (…)` receiver's declared
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
    for t in r.untyped_port_types.values_mut().chain(r.call_type_args.values_mut().flatten()) {
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
fn warn_top_redeclarations(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    let top = ctx.top_names.clone();
    let namespaced: crate::collections::HashSet<Arc<str>> = decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Namespace(ns) => Some(ns.decls.iter().map(|nd| nd.range().file.clone())),
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
    found.sort_by_key(|(_, _, after)| (after.range.file.clone(), after.range.start.offset));
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
fn explain_hidden_enum_types(ctx: &mut TypeCheckCtx) {
    let mut found = Vec::new();
    for (i, d) in ctx.diagnostics.iter().enumerate() {
        let Some(name) = d.message.strip_prefix("unknown type '").and_then(|r| r.strip_suffix('\'')) else { continue };
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
fn warn_namespace_redeclarations(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    let mut found: Vec<(String, crate::diagnostic::SourceRange, u32)> = Vec::new();
    let mut seen = crate::collections::HashSet::default();
    for d in decls {
        let TopDecl::Namespace(ns) = d else { continue };
        let mut first: crate::collections::HashMap<(Arc<str>, String), u32> = Default::default();
        for nd in &ns.decls {
            let r = nd.range();
            for (name, _) in crate::scope::top::declared_names_of(nd) {
                match first.get(&(r.file.clone(), name.clone())) {
                    Some(line) if seen.insert((r.file.clone(), r.start.offset, name.clone())) => {
                        found.push((name, r.clone(), *line));
                    }
                    Some(_) => {}
                    None => {
                        first.insert((r.file.clone(), name), r.start.line);
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

/// Two-pass typecheck: pass 1 typechecks with no custom-event slot inference
/// (same as calling `typecheck` directly), then infers unannotated
/// custom-event receiver slot types from in-unit senders
/// (`infer_custom_event_slots`) using pass 1's `type_of_expr`. If any
/// slot was inferred, a pass 2 re-typechecks the whole script with the
/// inferred map wired in, so bodies see the inferred types (not `any`) and
/// WS030 compares against them too. Pass 2's diagnostics include the
/// inference pass's own (WS042 for uninferable slots).
///
/// Returns the map alongside the result so `lower` can pick the right
/// wire-port variant for each custom-event slot.
pub fn typecheck_with_inference(script: &Script, file: &str) -> (TypeCheckResult, CeSlotMap) {
    let empty = CeSlotMap::default();
    let pass1 = typecheck(script, file, &empty);
    let (map, infer_diags) = infer_custom_event_slots(script, &pass1.type_of_expr);
    if map.is_empty() {
        return (pass1, map); // no unannotated custom-event slots -> single pass
    }
    let mut pass2 = typecheck(script, file, &map);
    pass2.diagnostics.extend(infer_diags);
    (pass2, map)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ref_tests;

#[cfg(test)]
mod resolution_tests;
