//! Lowering: AST + typecheck annotations -> IR Module.
//!
//! Strategy: walk each top-level declaration. Vars and I/O become IR
//! nodes with deterministic names. Handlers become one event node feeding
//! an exec chain; statements thread a `current_exec` `PortRef` through each
//! step. Expressions produce gate nodes whose value output is threaded
//! into their consumer.

use crate::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{ChipDecl, *};
use crate::catalog::calls::find_call;
use crate::catalog::events::find_event;
use crate::catalog::operators::OpRule;
use crate::diagnostic::{Diagnostic, RangeKey, SourceRange, StartKey};
use crate::intern::{Sym, intern, intern_static, resolve, sym};
use crate::ir::build::{AddNodeOpts, ModuleBuilder, port_ref};
use crate::ir::gate_class as gc;
use crate::ir::{
    GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, ROOT_SCOPE_ID, ScopeId,
    ScopeInfo, ScopeKind, Type, Wire, port_registry::WirePort,
};
use crate::template_cache::TemplateCache;
use crate::typecheck::enums::{DISC, DISC_FIELD, DISCRIMINANT_FIELD, slot_name};
use crate::typecheck::{CeSlotMap, TypeCheckResult};

mod context;
use context::*;

mod bake;
use bake::*;
mod predeclare;
use crate::const_eval::{
    ConstEnv, default_literal_for_var_type, eval_const_binop, expr_to_literal, expr_to_literal_in,
    fold_ammo_override, fold_mesh_colors, holds_refs, ref_shaped,
};
use predeclare::*;

mod decl;
use decl::*;

mod handler;
use handler::*;

mod stmt;
use stmt::*;

mod expr;
pub(crate) use expr::wire_type_of_literal;
use expr::*;

mod pattern_test;
use pattern_test::*;

mod ops;
use ops::*;

mod call;
use call::*;

mod access;
mod ordered;
mod record_assign;
mod ref_fields;
use access::*;

pub(crate) mod boundary_pins;
pub(crate) mod flatten;
pub(crate) use crate::ir::gate_variants;
mod convert;
mod props;
mod style;
pub(crate) use style::{MATERIAL_ENUM, material_arg, style_constant};

// `for`-loop lowering, and the range normalisation it rests on: every range
// spelling collapses onto the callable-loop shell's three driver pins.
pub(crate) mod for_loop;
pub(crate) mod function;

// Maranget decision-tree builder for `match` lowering; the match lowering
// walks the `Decision` it emits.
pub(crate) mod matchtree;

// Certified constant-fold pass. `fold::eval` is re-exported from the crate
// root as `fold_eval` (`#[doc(hidden)]`) for the `--fold-diff` fuzz harness in
// `examples/fuzz_programs.rs`; everything else in `fold` stays crate-private.
#[doc(hidden)]
pub(crate) mod fold;

// Result

pub struct LowerResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition dropped
    /// (see `LowerCtx::dropped_ranges`). Tests check it agrees with
    /// `typecheck::TypeCheckResult::dropped_ranges`.
    pub dropped_ranges: Vec<SourceRange>,
}

pub struct LowerInput<'a> {
    pub ast: &'a Script,
    pub type_of_expr: &'a HashMap<RangeKey, Type>,
    pub op_resolutions: &'a HashMap<RangeKey, OpRule>,
    pub resolutions: &'a crate::typecheck::Resolutions,
    pub file: &'a str,
    pub module_name: Option<&'a str>,
    pub template_cache: Arc<TemplateCache>,
    pub doc_comments: &'a crate::parser::DocComments,
    /// Whether the certified constant-fold pass runs (see [`FoldMode`]). A
    /// module-level `@nofold` (`input.ast.no_fold`) also disables it.
    pub fold_mode: FoldMode,
    /// Inferred custom-event data-slot types, keyed by the receiver handler's
    /// `ce_slot_key`. An unannotated `on CustomEvent(...)` param's `DataOut`
    /// port takes the inferred type rather than `Type::Float`.
    pub ce_slots: &'a CeSlotMap,
}

/// Whether the certified constant-fold pass runs. `Auto` (the default) and
/// `ForceOn` (CLI `--fold`) fold unless the entry file has a module-level
/// `@nofold`; `ForceOff` (CLI `--no-fold`) disables folding for the whole compile.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FoldMode {
    #[default]
    Auto,
    ForceOn,
    ForceOff,
}

/// How many times a handler's body is lowered. `on a | b { ... }` lowers it
/// once per trigger part (see `lower_handler`'s `Trigger::Union` arm), and a
/// part may itself be a union.
fn trigger_multiplicity(t: &Trigger) -> usize {
    match t {
        Trigger::Union { parts, .. } => {
            parts.iter().map(trigger_multiplicity).sum::<usize>().max(1)
        }
        _ => 1,
    }
}

/// Emit counts for one handler, scaled by how many times its body is lowered.
/// Counting the body once leaves a union-trigger handler's single `emit` short
/// of the 2-driver threshold, so the output gets no backing var and the lowered
/// copies fan in on its rerouter, which fails to load in-game.
pub(super) fn count_emits_in_handler(
    h: &Handler,
    in_branch: bool,
    counts: &mut HashMap<String, (usize, bool)>,
) {
    let mut body: HashMap<String, (usize, bool)> = HashMap::default();
    count_emits_in_block(&h.body, in_branch, &mut body);
    let reps = trigger_multiplicity(&h.trigger);
    for (name, (count, branched)) in body {
        let entry = counts.entry(name).or_insert((0, false));
        entry.0 += count * reps;
        entry.1 |= branched;
    }
}

/// Accumulate the `emit <output> = ...` and handler `out <output> = ...` sites
/// in `block` into `counts`: per output, how many sites write a value and
/// whether any runs on only some paths. Anonymous chips share their parent's
/// outputs, nested handlers count once per trigger, a named chip owns its own.
/// An output with several sites, or one conditional site, needs a backing store,
/// since the single-site path wires the value to the rerouter unconditionally.
pub(super) fn count_emits_in_block(
    block: &Block,
    in_branch: bool,
    counts: &mut HashMap<String, (usize, bool)>,
) {
    count_emits_in(walk::Node::Block(block), in_branch, counts);
}

fn count_emits_in(
    node: walk::Node<'_>,
    in_branch: bool,
    counts: &mut HashMap<String, (usize, bool)>,
) {
    let start = walk::Ctx {
        branch: in_branch,
        ..walk::Ctx::default()
    };
    walk::walk(node, start, &mut |n, cx| {
        let name = match n {
            walk::Node::Stmt(Stmt::Emit(e)) if e.value.is_some() => &e.name,
            walk::Node::Stmt(Stmt::OutBinding(o)) if o.value.is_some() => &o.name,
            walk::Node::Stmt(Stmt::Handler(h)) => {
                count_emits_in_handler(h, cx.branch, &mut *counts);
                return false;
            }
            walk::Node::Stmt(Stmt::ChipDecl(_)) => return false,
            _ => return true,
        };
        let entry = counts.entry(name.clone()).or_insert((0, false));
        entry.0 += 1;
        entry.1 |= cx.branch;
        true
    });
}

/// Every name `block` sets with `emit name = v`, at any depth (branches,
/// loops, handlers and anonymous chips included; not a named chip's body).
pub(in crate::lower) fn collect_value_emit_names(block: &Block, names: &mut HashSet<String>) {
    walk::walk(
        walk::Node::Block(block),
        walk::Ctx::default(),
        &mut |n, _| match n {
            walk::Node::Stmt(Stmt::Emit(e)) if e.value.is_some() => {
                names.insert(e.name.clone());
                true
            }
            walk::Node::Stmt(Stmt::ChipDecl(_)) => false,
            _ => true,
        },
    );
}

/// A declaration scope of the program: its own top level (`None`), or an
/// `import * as` namespace, keyed by the namespace declaration's location.
pub(in crate::lower) type DeclScope = Option<StartKey>;

#[expect(
    clippy::unnecessary_wraps,
    reason = "yields a scope key, where `None` is the top level"
)]
pub(in crate::lower) fn namespace_scope(ns: &NamespaceDecl) -> DeclScope {
    Some(ns.range.start_key())
}

/// Per declaration scope, per output name: how many sites write a value and
/// whether any is conditional. The sites are the scope's handlers, anonymous
/// chips and captured events, plus each `mod`/`fn` body's writes once per
/// instantiation of that body (see [`Program::body_instances`]).
pub(in crate::lower) fn output_write_counts(
    ast: &Script,
) -> HashMap<DeclScope, HashMap<String, (usize, bool)>> {
    let program = Program::new(ast);
    let instances = program.body_instances();
    let mut out: HashMap<DeclScope, HashMap<String, (usize, bool)>> = HashMap::default();
    for (si, scope) in program.scopes.iter().enumerate() {
        let counts = out.entry(scope.key.clone()).or_default();
        for d in scope.decls {
            match d {
                TopDecl::Handler(h) => count_emits_in_handler(h, false, counts),
                TopDecl::AnonChip(ac) => count_emits_in_block(&ac.body, false, counts),
                TopDecl::Event(EventDecl {
                    captured: Some(Handler { body: b, .. }),
                    ..
                }) => count_emits_in_block(b, false, counts),
                _ => {}
            }
        }
        for (bi, body) in program.bodies.iter().enumerate() {
            if body.scope != si || !body.inline {
                continue;
            }
            let mut own = HashMap::default();
            count_emits_in(body.node, false, &mut own);
            let (sites, site_branched) = instances[bi];
            for (name, (count, branched)) in own {
                if body.outputs.contains(&name.as_str()) {
                    continue;
                }
                let entry = counts.entry(name).or_insert((0, false));
                entry.0 += count * sites;
                entry.1 |= branched || site_branched;
            }
        }
    }
    out
}

/// A `mod`/`chip`/`fn` declared at the top level of a scope.
struct Body<'a> {
    name: &'a str,
    range: &'a SourceRange,
    node: walk::Node<'a>,
    scope: usize,
    inline: bool,
    self_receiver: bool,
    outputs: Vec<&'a str>,
}

struct Scope<'a> {
    key: DeclScope,
    decls: &'a [TopDecl],
}

/// The program's declaration scopes and bodies, with the lookups a call
/// resolves its callee through.
struct Program<'a> {
    scopes: Vec<Scope<'a>>,
    bodies: Vec<Body<'a>>,
    body_by_name: HashMap<(usize, &'a str), usize>,
    namespace_by_name: HashMap<(usize, &'a str), usize>,
    imports: &'a ImportScope,
}

impl<'a> Program<'a> {
    fn new(ast: &'a Script) -> Self {
        let mut p = Program {
            scopes: Vec::new(),
            bodies: Vec::new(),
            body_by_name: HashMap::default(),
            namespace_by_name: HashMap::default(),
            imports: &ast.imports,
        };
        p.add_scope(None, &ast.decls);
        p
    }

    fn add_scope(&mut self, key: DeclScope, decls: &'a [TopDecl]) -> usize {
        let si = self.scopes.len();
        self.scopes.push(Scope { key, decls });
        for d in decls {
            let body = match d {
                TopDecl::Chip(c) => Body {
                    name: c.name.as_str(),
                    range: &c.range,
                    node: walk::Node::Block(&c.body),
                    scope: si,
                    inline: c.inline,
                    self_receiver: c.is_self_receiver(),
                    outputs: c.outputs.iter().map(|o| o.name.as_str()).collect(),
                },
                TopDecl::Namespace(ns) => {
                    let inner = self.add_scope(namespace_scope(ns), &ns.decls);
                    self.namespace_by_name.insert((si, ns.name.as_str()), inner);
                    continue;
                }
                _ => continue,
            };
            self.body_by_name.insert((si, body.name), self.bodies.len());
            self.bodies.push(body);
        }
        si
    }

    /// A body named `name` as seen from scope `si`: its own, else the
    /// program's top level.
    fn body(&self, si: usize, name: &str) -> Option<usize> {
        self.body_by_name
            .get(&(si, name))
            .or_else(|| self.body_by_name.get(&(0, name)))
            .copied()
    }

    /// The bodies a call in scope `si` may instantiate: a plain call names its
    /// callee, `ns.f()` a member of a namespace, and a receiver call `x.f()` a
    /// body whose first parameter is `self`.
    fn callees(&self, si: usize, call: &Expr) -> Vec<usize> {
        let Expr::Call { callee, .. } = call else {
            return Vec::new();
        };
        match callee.as_ref() {
            Expr::Ident { name, range } => self
                .body(si, name)
                .filter(|&b| {
                    self.imports
                        .user_mod_answers(self.bodies[b].range, &range.file)
                })
                .into_iter()
                .collect(),
            Expr::FieldAccess { obj, field, range } => {
                let mut found = Vec::new();
                if let Expr::Ident { name, .. } = obj.as_ref()
                    && let Some(&ns) = self
                        .namespace_by_name
                        .get(&(si, name.as_str()))
                        .or_else(|| self.namespace_by_name.get(&(0, name.as_str())))
                    && let Some(&b) = self.body_by_name.get(&(ns, field.as_str()))
                {
                    found.push(b);
                }
                if let Some(b) = self.body(si, field)
                    && self.bodies[b].self_receiver
                    && self
                        .imports
                        .user_mod_answers(self.bodies[b].range, &range.file)
                    && !found.contains(&b)
                {
                    found.push(b);
                }
                found
            }
            _ => Vec::new(),
        }
    }

    /// Per body, how many times it is instantiated and whether any instance
    /// runs on only some paths: its call sites outside every body, plus each
    /// call inside another body once per instantiation of that body. A call is
    /// conditional under a branch, arm, `if` expression or loop, after a
    /// possible early `return`, or inside a conditionally instantiated body.
    fn body_instances(&self) -> Vec<(usize, bool)> {
        let n = self.bodies.len();
        // Calls outside every body, by callee: (count, conditional).
        let mut top: Vec<(usize, bool)> = vec![(0, false); n];
        // Calls made inside each body, by caller then callee.
        let mut inside: Vec<HashMap<usize, (usize, bool)>> = vec![HashMap::default(); n];
        for (si, scope) in self.scopes.iter().enumerate() {
            for d in scope.decls {
                let (node, caller) = match d {
                    TopDecl::Namespace(_) => continue,
                    TopDecl::Chip(ChipDecl { name, .. }) => {
                        let b = self.body_by_name[&(si, name.as_str())];
                        (self.bodies[b].node, Some(b))
                    }
                    _ => (walk::Node::Decl(d), None),
                };
                call_sites(
                    node,
                    walk::Ctx::default(),
                    1,
                    &mut |call, branched, reps| {
                        for callee in self.callees(si, call) {
                            let entry = match caller {
                                Some(c) => inside[c].entry(callee).or_insert((0, false)),
                                None => &mut top[callee],
                            };
                            entry.0 += reps;
                            entry.1 |= branched;
                        }
                    },
                );
            }
        }
        // Each body's callers, in caller order: (caller, calls, any branched).
        let mut callers: Vec<Vec<(usize, usize, bool)>> = vec![Vec::new(); n];
        for (caller, calls) in inside.iter().enumerate() {
            for (&callee, &(k, site_branched)) in calls {
                callers[callee].push((caller, k, site_branched));
            }
        }
        for c in &mut callers {
            c.sort_unstable_by_key(|(caller, _, _)| *caller);
        }
        fn instances(
            b: usize,
            top: &[(usize, bool)],
            callers: &[Vec<(usize, usize, bool)>],
            memo: &mut [Option<(usize, bool)>],
            open: &mut HashSet<usize>,
        ) -> (usize, bool) {
            if let Some(n) = memo[b] {
                return n;
            }
            if !open.insert(b) {
                return (0, false);
            }
            let (mut n, mut branched) = top[b];
            for &(caller, k, site_branched) in &callers[b] {
                let (m, caller_branched) = instances(caller, top, callers, memo, open);
                n += k * m;
                branched |= m > 0 && (site_branched || caller_branched);
            }
            open.remove(&b);
            memo[b] = Some((n, branched));
            (n, branched)
        }
        let mut memo = vec![None; n];
        (0..n)
            .map(|b| instances(b, &top, &callers, &mut memo, &mut HashSet::default()))
            .collect()
    }
}

/// Every call below `node`, with whether it runs on only some paths and how
/// many times its enclosing code is lowered (a handler on `a | b` lowers its
/// body once per trigger).
fn call_sites<'a>(
    node: walk::Node<'a>,
    cx: walk::Ctx,
    reps: usize,
    on: &mut dyn FnMut(&'a Expr, bool, usize),
) {
    if let walk::Node::Expr(e @ Expr::Call { .. }) = node {
        on(e, cx.branch, reps);
    }
    let reps = match node {
        walk::Node::Stmt(Stmt::Handler(h))
        | walk::Node::Decl(
            TopDecl::Handler(h)
            | TopDecl::Event(EventDecl {
                captured: Some(h), ..
            }),
        ) => reps * trigger_multiplicity(&h.trigger),
        _ => reps,
    };
    walk::children(node, &mut |child, flow| {
        call_sites(child, cx.enter(flow), reps, &mut *on);
    });
}

/// Give output `name` a backing `PseudoVar` (idempotent). Two wires into an
/// output rerouter are a fan-in that fails at load, so each `emit` site sets
/// this var and the var feeds the output once. A no-op unless `lookup_output`
/// (or `output_tag`) resolves `name`.
pub(in crate::lower) fn create_output_backing_var(ctx: &mut LowerCtx<'_>, name: &str) {
    if ctx.output_backing_vars.contains_key(name) {
        return;
    }
    let Some(out) = ctx
        .lookup_output(name)
        .or_else(|| ctx.output_tag(name))
        .cloned()
    else {
        return;
    };
    let inner = out.ty;
    let var_id = make_scalar_var_gate(
        ctx,
        &inner,
        HashMap::default(),
        &SourceRange::default(),
        Some("out_backing"),
    );
    ctx.output_backing_vars.insert(
        name.to_string(),
        VarRecord::new(var_id, inner, VarStorage::Var),
    );
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "callers build the input for this one call"
)]
pub fn lower(input: LowerInput<'_>) -> LowerResult {
    // Root module name is on the top-level chip's emitted text label.
    // Explicit `module_name` wins; otherwise use the entry file's stem.
    let file_stem = std::path::Path::new(input.file)
        .file_stem()
        .and_then(|s| s.to_str());
    let builder = ModuleBuilder::new(input.module_name.or(file_stem).unwrap_or("main"));
    // One registry shared by `ctx.enum_defs` and the const scope, so lowering
    // and const evaluation agree on every tag. In an `Arc` so clones are cheap.
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&input.ast.decls));

    let generic_type_aliases = collect_generic_type_aliases(&input.ast.decls);
    let top_names = Arc::new(crate::scope::top::TopNames::of(&input.ast.decls));
    let mut ctx = LowerCtx {
        file: input.file.to_string(),
        type_aliases: expand_alias_bodies(
            &type_aliases_of(&input.ast.decls)
                .map(|(key, ta)| (key, ta.typ.clone()))
                .collect(),
            &generic_type_aliases,
        ),
        // Built by the shared `enums::build_registry`, so lowering reads the
        // discriminant typecheck assigned.
        enum_defs: Arc::clone(&enum_defs),
        template_cache: Arc::clone(&input.template_cache),
        known_fn_names: Arc::new(collect_fn_names(input.ast)),
        imports: Arc::clone(&input.ast.imports),
        known_let_names: Arc::new(collect_let_names(input.ast)),
        consts: crate::const_scope::ConstScope::for_program(
            &input.ast.decls,
            &enum_defs,
            &input.ast.imports,
            &top_names,
        ),
        top_names,
        is_root_module: true,
        // Module-level `@nofold` (top of file + blank line) marks everything.
        nofold_depth: u32::from(input.ast.no_fold),
        ..LowerCtx::empty(
            builder,
            input.type_of_expr,
            input.op_resolutions,
            input.resolutions,
            input.ce_slots,
            input.doc_comments,
        )
    };

    // Pass 1: register I/O + vars + buffers. A name the top level declares
    // more than once is read by position (see `LowerCtx::top_view`).
    for d in &input.ast.decls {
        // A `var` initializer reads the declaration before the `var`.
        let above = match d {
            TopDecl::Var(v) => ctx.top_names.initializer_read(&v.name, &v.range),
            _ => None,
        };
        ctx.consts.set_cursor(Some(
            above.unwrap_or_else(|| crate::scope::top::ReadAt::code(d.range())),
        ));
        pre_declare_decl(&mut ctx, d);
        ctx.keep_top_dup_bindings(d);
    }
    // Pass 1a: a top-level `on` handler may declare a boundary output
    // (`on Clock { @top out flash: bool = Toggle() }`). Runs after pass 1 so the
    // name binds to a same-named top-level `out` anywhere in the file.
    for d in &input.ast.decls {
        match d {
            TopDecl::Handler(h) => pre_declare_handler_outputs(&mut ctx, &h.body, true),
            // A captured event (`let foo = on Trigger { ... }`) is lowered
            // handler-style, so its `out` bindings are handler bindings too.
            TopDecl::Event(e) => {
                if let Some(body) = e.captured_body() {
                    pre_declare_handler_outputs(&mut ctx, body, true);
                }
            }
            _ => {}
        }
    }
    // Snapshot the names the importer owns (everything pass 1 registered) so a
    // pass-2 namespace member cannot clobber one (see `LowerCtx::importer_names`).
    ctx.importer_names = ctx.scope.iter().map(|(k, _)| k.to_string()).collect();
    // Pass 1b: give a backing PseudoVar to every output written from several
    // sites, one conditional site, or a default (`out r = 0`) plus any emit, since
    // two drivers on its rerouter fan in. Only one unconditional emit wires directly.
    let defaulted_outputs: HashSet<String> = input
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Out(o) if o.value.is_some() => Some(o.name.clone()),
            _ => None,
        })
        .collect();
    for d in &input.ast.decls {
        match d {
            TopDecl::Handler(h) => collect_value_emit_names(&h.body, &mut ctx.emitted_outputs),
            TopDecl::AnonChip(ac) => collect_value_emit_names(&ac.body, &mut ctx.emitted_outputs),
            _ => {}
        }
    }
    let mut write_counts = output_write_counts(input.ast);
    let root_writes = write_counts.remove(&None).unwrap_or_default();
    ctx.namespace_output_writes = Arc::new(write_counts);
    ctx.output_write_sites = root_writes
        .iter()
        .map(|(name, (count, _))| {
            (
                name.clone(),
                count + usize::from(defaulted_outputs.contains(name)),
            )
        })
        .collect();
    for (name, (count, in_branch)) in root_writes {
        let has_default = defaulted_outputs.contains(&name);
        if count < 2 && !in_branch && !has_default {
            continue;
        }
        // A record output's backing is a var per field, driving its pins once.
        let record = input
            .ast
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Out(o) if o.name == name => {
                    o.typ.as_ref().map(|t| (t.clone(), o.range.clone()))
                }
                _ => None,
            })
            .and_then(|(t, range)| ctx.record_or_tuple_fields(&t).map(|f| (f, range)));
        match record {
            Some((fields, range)) => {
                let backing = build_record_fields(
                    &mut ctx,
                    VarStorage::Var,
                    &fields,
                    &name,
                    None,
                    None,
                    &range,
                );
                ctx.output_record_backing.insert(name, backing);
            }
            None => create_output_backing_var(&mut ctx, &name),
        }
    }
    // Pass 2: lower bodies.
    for d in &input.ast.decls {
        // Flush handler end execs before a non-handler declaration so code after
        // `on` blocks chains from the combined exits. An anon chip whose body is
        // a single handler counts as a handler.
        if !ctx.handler_end_execs.is_empty() && !is_handler_like(d) {
            flush_handler_end_execs(&mut ctx);
        }
        // A pure declaration (let / out / var / buffer) must not inherit the
        // flushed `current_exec`, so it lowers with the exec cleared.
        ctx.top_view(crate::scope::top::ReadAt::code(d.range()));
        if is_pure_top_decl(d) {
            ctx.pure(|ctx| lower_decl(ctx, d));
        } else {
            lower_decl(&mut ctx, d);
        }
        ctx.keep_top_dup_bindings(d);
    }

    // Pass 2b: resolve runtime `@label(expr)` on top-level vars into dynamic
    // label wires. Runs after every top-level var/gate exists, and before the
    // dead-gate prune below so the coercion gate's input wire keeps it alive.
    resolve_dynamic_var_labels(&mut ctx, &input.ast.decls);
    // A module-level `@label(<expr>)` labels the ROOT chip (same pass, so it may
    // forward-reference declarations below it).
    resolve_module_label(&mut ctx, input.ast.module_label.as_ref());
    // Runtime `@color` / `@material` on top-level vars and the module, the
    // same way and for the same reason.
    style::resolve_dynamic_var_styles(&mut ctx, &input.ast.decls);
    style::resolve_module_style(
        &mut ctx,
        input.ast.module_color.as_ref(),
        input.ast.module_material.as_ref(),
    );

    // Feed each backing PseudoVar into its output rerouter with one wire.
    let backings: Vec<(String, VarRecord)> = ctx
        .output_backing_vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, backing) in backings {
        if let Some(out) = ctx
            .lookup_output(&name)
            .or_else(|| ctx.output_tag(&name))
            .cloned()
        {
            ctx.connect(
                backing.node_id.port(WirePort::Value),
                out.node_id.port(WirePort::RerInput),
            );
        }
    }
    let mut records: Vec<(String, Fields)> = ctx
        .output_record_backing
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, backing) in records {
        if let Some(Binding::Record(pins)) =
            ctx.scope.get(&context::output_scope_key(&name)).cloned()
        {
            call::wire_record_backing(&mut ctx, &backing, &pins);
        }
    }

    flush_pending_emits(&mut ctx);

    let mut module = ctx.builder.module;
    let whole_file = SourceRange::new(
        input.file,
        crate::diagnostic::Pos::default(),
        crate::diagnostic::Pos::default(),
    );
    prune_dead_exec_unions(&mut module);
    convert::prune_unread_conversions(&mut module);
    // Before literal inlining a computation still has its operand wires, so
    // only wireless orphans are pruned here, not an unused `let y = x * 2`.
    prune_dead_pure_gates(&mut module, false);
    prune_unread_gate_read_choices(&mut module);
    // Certified, table-gated constant folding (see `FoldMode`). The module-level
    // `@fold` (`input.ast.fold`) gates nothing.
    let run_fold = match input.fold_mode {
        FoldMode::ForceOff => false,
        FoldMode::ForceOn | FoldMode::Auto => !input.ast.no_fold,
    };
    // Inline a bare source literal into a matching scalar data field
    // (`on Clock(interval = 0.25)`, the `0` in `x | 0`) before folding, when only
    // source literals exist, so the result is the same with folding on or off.
    let diags = &mut ctx.diagnostics;
    run_checked(
        &mut module,
        diags,
        &whole_file,
        "inline_bare_scalar_literals",
        |m, _| {
            inline_bare_scalar_literals(m);
        },
    );
    if run_fold {
        run_checked(
            &mut module,
            diags,
            &whole_file,
            "fold_certified_constants",
            |m, _| {
                fold::fold_certified_constants(m);
                localize_foreign_literals(m);
            },
        );
    }
    run_checked(
        &mut module,
        diags,
        &whole_file,
        "materialize_unfoldable_constants",
        |m, _| {
            materialize_unfoldable_constants(m);
        },
    );
    run_checked(
        &mut module,
        diags,
        &whole_file,
        "inline_orphan_literals",
        |m, _| {
            inline_orphan_literals(m);
        },
    );
    // Folding can leave a bare `_Literal` wired to nothing; sweep those too
    // (a literal has no inputs, so it is never a user computation).
    run_checked(
        &mut module,
        diags,
        &whole_file,
        "prune_dead_pure_gates",
        |m, _| {
            prune_dead_pure_gates(m, true);
        },
    );
    // Merge the constant gates that survived inlining (a `"PREFIX: " ..` concat
    // wrapper repeated per use, a multi-consumer literal) so an identical
    // constant is emitted once and fans out, per chip.
    run_checked(
        &mut module,
        diags,
        &whole_file,
        "dedup_constant_gates",
        |m, _| {
            dedup_constant_gates(m);
        },
    );
    // Common-subexpression elimination: identical pure `Expr_*` gates with wired
    // inputs collapse to one. Runs after the literal and fold passes (properties
    // are final) and before partition (keepers stay in their consumers' chip).
    run_checked(&mut module, diags, &whole_file, "cse_pure_gates", |m, _| {
        cse_pure_gates(m);
    });
    // `@flat` inlines every chip body onto one grid, replacing the anon-chip
    // partition (which moves nodes into child modules). The single module left
    // gives `synthesize_boundary_pins` no cross-module wire.
    if input.ast.flat {
        run_checked(
            &mut module,
            diags,
            &whole_file,
            "flatten_chips",
            |m, diags| {
                flatten::flatten_chips(m, diags);
            },
        );
    } else {
        run_checked(
            &mut module,
            diags,
            &whole_file,
            "partition_anon_chips",
            |m, _| {
                crate::emit::partition_anon_chips(m);
            },
        );
    }
    function::capture_through_handles(&mut module, diags);
    boundary_pins::synthesize_boundary_pins(&mut module);
    check_wires_resolve(
        &module,
        "synthesize_boundary_pins",
        true,
        &whole_file,
        diags,
    );
    // Last: the wire set is final, and the brick size follows the ports in use.
    gate_variants::select_gate_variants(&mut module);
    check_no_exec_into_value(&module, &whole_file, diags);
    crate::emit::check_native_settings(&module, diags, &|note, field| {
        let spec = crate::catalog::calls::find_call(note)?;
        spec.params
            .iter()
            .find(|p| p.port.as_str() == field)
            .map(|p| (spec.name, p.name))
    });
    LowerResult {
        module,
        diagnostics: ctx.diagnostics,
        dropped_ranges: ctx.dropped_ranges,
    }
}

/// Run one post-lowering `pass` over `module`, then check that every wire it
/// left still resolves ([`check_wires_resolve`]), naming the pass on failure.
fn run_checked(
    module: &mut Module,
    diagnostics: &mut Vec<Diagnostic>,
    fallback: &SourceRange,
    name: &str,
    pass: impl FnOnce(&mut Module, &mut Vec<Diagnostic>),
) {
    pass(module, diagnostics);
    check_wires_resolve(module, name, false, fallback, diagnostics);
}

/// Consistency check after a post-lowering pass: every wire names two nodes
/// that exist somewhere in the module tree. A violation is a compiler bug,
/// reported once as `WS094` naming the pass just run.
///
/// Per-pass checks run in debug builds; the final `pinned` check runs always and
/// also requires each data-wire endpoint to be in the wire's module or a boundary
/// pin of its chips (a foreign `_Literal` would reach the save as nothing).
///
/// A wire to `NodeId(0)` (a void call result, `lower/call/wiring.rs`) is
/// accepted only once an error has been reported.
fn check_wires_resolve(
    root: &Module,
    after: &str,
    pinned: bool,
    fallback: &SourceRange,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if (!pinned && !cfg!(debug_assertions)) || diagnostics.iter().any(|d| d.code == "WS094") {
        return;
    }
    let zero_expected = diagnostics
        .iter()
        .any(|d| d.severity == crate::diagnostic::Severity::Error);
    type Owners<'m> = std::cell::OnceCell<HashMap<NodeId, &'m Module>>;
    // Almost every wire has both endpoints in its own module, so the tree-wide
    // owner index is built only when some endpoint is not, and the message
    // only when a wire fails.
    fn check<'m>(
        m: &'m Module,
        root: &'m Module,
        owners: &Owners<'m>,
        pinned: bool,
        zero_expected: bool,
    ) -> Option<(String, Option<SourceRange>)> {
        let owner_of = |id: &NodeId| -> Option<&'m Module> {
            owners
                .get_or_init(|| {
                    let mut owner = HashMap::default();
                    root.walk(&mut |_, m| owner.extend(m.nodes.keys().map(|id| (*id, m))));
                    owner
                })
                .get(id)
                .copied()
        };
        let range_of = |id: &NodeId| {
            owner_of(id)
                .and_then(|o| o.nodes.get(id))
                .map(|n| n.source_range.clone())
        };
        // Every pin of `m`'s child chips, gathered on the first wire that asks.
        let pins: std::cell::OnceCell<HashSet<NodeId>> = std::cell::OnceCell::new();
        for w in &m.wires {
            let (src, dst) = (w.source.node_id, w.target.node_id);
            let wire = || {
                format!(
                    "wire {src}.{} -> {dst}.{} in module '{}'",
                    w.source.port.as_str(),
                    w.target.port.as_str(),
                    resolve(m.name)
                )
            };
            let carries_value =
                w.source.port != WirePort::Layout && w.target.port != WirePort::Layout;
            for (end, other) in [(src, dst), (dst, src)] {
                if m.nodes.contains_key(&end) || (zero_expected && end == NodeId(0)) {
                    continue;
                }
                let Some(end_owner) = owner_of(&end) else {
                    return Some((
                        format!("{} references {end}, which no module holds", wire()),
                        range_of(&other),
                    ));
                };
                let is_pin = || {
                    pins.get_or_init(|| {
                        m.chips
                            .values()
                            .flat_map(|c| c.inputs.iter().chain(&c.outputs))
                            .copied()
                            .collect()
                    })
                    .contains(&end)
                };
                let definition = || {
                    owner_of(&src)
                        .and_then(|o| o.nodes.get(&src))
                        .is_some_and(|n| function::is_definition_wire(n, w))
                };
                if !pinned || !carries_value || is_pin() || definition() {
                    continue;
                }
                let what = if end_owner
                    .nodes
                    .get(&end)
                    .is_some_and(|n| n.gate_class == gc::LITERAL)
                {
                    "reads another module's literal"
                } else {
                    "reaches a node in another module without a boundary pin"
                };
                return Some((format!("{} {what} ({end})", wire()), range_of(&other)));
            }
        }
        m.chips
            .values()
            .find_map(|c| check(c, root, owners, pinned, zero_expected))
    }
    let owners = Owners::new();
    if let Some((problem, range)) = check(root, root, &owners, pinned, zero_expected) {
        diagnostics.push(Diagnostic::error(
            "WS094",
            format!("internal compiler error after `{after}`: {problem}. This is a compiler bug; please report it."),
            range.unwrap_or_else(|| fallback.clone()),
        ));
    }
}

/// Whether an exec may drive the input `port` of type `ty` of `dst`: an exec
/// input, or a `bool` one (a branch or select condition, a `bool` `Var_Set`,
/// the value written to a `bool` map or array, a logical gate's input), which
/// reads the exec as true for the tick it fires, as `expressions.md`
/// documents. The data pin of a custom-event send takes it too: type checking
/// reports the untyped data (`WS045`).
pub(crate) fn exec_may_drive(dst: &Node, port: WirePort, ty: &Type) -> bool {
    matches!(ty, Type::Exec | Type::Bool)
        || dst.gate_class.starts_with(gc::EXPR_LOGICAL_PREFIX)
        || (matches!(ty, Type::Any | Type::Opaque)
            && [gc::SEND_CUSTOM_EVENT, gc::SEND_CUSTOM_EVENT_GLOBAL].contains(&dst.gate_class)
            && port.as_str().starts_with("DataIn"))
}

/// The wiring invariant: no exec drives an input that takes a value other than
/// a `bool` ([`exec_may_drive`]), since an exec carries no value and the gate
/// reads 0 in game. Followed through untyped rerouter pins (`out r: any`).
///
/// Each violation is `WS094`: a compiler bug for a direct wire, a user error
/// through an untyped pin. A debug build also panics on a direct one when no
/// other error exists ([`crate::diagnostic::drop_internal_errors_beside_errors`]).
pub(crate) fn check_no_exec_into_value(
    root: &Module,
    fallback: &SourceRange,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if diagnostics
        .iter()
        .any(|d| d.code == crate::diagnostic::INTERNAL_ERROR)
    {
        return;
    }
    fn index<'m>(m: &'m Module, nodes: &mut HashMap<NodeId, &'m Node>, wires: &mut Vec<Wire>) {
        nodes.extend(m.nodes.iter().map(|(id, n)| (*id, n)));
        let laid_out =
            |w: &&Wire| w.source.port == WirePort::Layout || w.target.port == WirePort::Layout;
        wires.extend(m.wires.iter().filter(|w| !laid_out(w)).copied());
        for c in m.chips.values() {
            index(c, nodes, wires);
        }
    }
    let mut nodes = HashMap::default();
    let mut wires = Vec::new();
    index(root, &mut nodes, &mut wires);
    let ends = |w: &Wire| {
        let (src, dst) = (nodes.get(&w.source.node_id)?, nodes.get(&w.target.node_id)?);
        let from = src.ports.find_output(w.source.port.sym()).map(|p| &p.ty);
        Some((
            *src,
            from,
            *dst,
            dst.ports.find_input(w.target.port.sym()).map(|p| &p.ty)?,
        ))
    };
    // The rerouters an exec reaches through a pin of no type: their output
    // carries it on.
    let mut carriers: HashSet<NodeId> = HashSet::default();
    let carries = |carriers: &HashSet<NodeId>, w: &Wire, from: Option<&Type>| {
        from == Some(&Type::Exec)
            || (w.source.port == WirePort::RerOutput && carriers.contains(&w.source.node_id))
    };
    let untyped_pin = |w: &Wire, into: &Type| {
        w.target.port == WirePort::RerInput && matches!(into, Type::Any | Type::Opaque)
    };
    loop {
        let mut grew = false;
        for w in &wires {
            if let Some((_, from, _, into)) = ends(w)
                && carries(&carriers, w, from)
                && untyped_pin(w, into)
            {
                grew |= carriers.insert(w.target.node_id);
            }
        }
        if !grew {
            break;
        }
    }
    // An array write takes its value untyped; a `bool` array's is a `bool`
    // place, as a `bool` map's value is.
    let feeds: HashMap<(NodeId, WirePort), PortRef> = wires
        .iter()
        .map(|w| ((w.target.node_id, w.target.port), w.source))
        .collect();
    let array_writes = [
        gc::ARRAY_PUSH,
        gc::ARRAY_SET_AT_INDEX,
        gc::ARRAY_INSERT,
        gc::ARRAY_FILL,
        gc::ARRAY_RESIZE,
    ];
    let stored_bool = |w: &Wire, dst: &Node| {
        if w.target.port != WirePort::Value || !array_writes.contains(&dst.gate_class) {
            return false;
        }
        let mut at = feeds.get(&(w.target.node_id, WirePort::ArrayVarRef));
        for _ in 0..nodes.len() {
            let Some(src) = at else { return false };
            let Some(node) = nodes.get(&src.node_id) else {
                return false;
            };
            match node.ports.find_output(src.port.sym()).map(|p| &p.ty) {
                Some(Type::Ref(t)) if matches!(t.as_ref(), Type::Array(e) if **e == Type::Bool) => {
                    return true;
                }
                Some(Type::Array(e)) if **e == Type::Bool => return true,
                Some(Type::Any | Type::Opaque) if src.port == WirePort::RerOutput => {
                    at = feeds.get(&(src.node_id, WirePort::RerInput));
                }
                _ => return false,
            }
        }
        false
    };
    let mut reported: HashSet<(NodeId, WirePort)> = HashSet::default();
    let mut found = Vec::new();
    for w in &wires {
        let Some((src, from, dst, into)) = ends(w) else {
            continue;
        };
        let into = if stored_bool(w, dst) {
            &Type::Bool
        } else {
            into
        };
        if !carries(&carriers, w, from)
            || untyped_pin(w, into)
            || exec_may_drive(dst, w.target.port, into)
        {
            continue;
        }
        if !reported.insert((w.target.node_id, w.target.port)) {
            continue;
        }
        let range = [&dst.source_range, &src.source_range]
            .into_iter()
            .find(|r| r.start.line != 0)
            .cloned()
            .unwrap_or_else(|| fallback.clone());
        let message = if from == Some(&Type::Exec) {
            format!(
                "internal compiler error: the exec output `{}.{}` ({}) drives the value input `{}.{}` ({}) of the \
                 gate built for line {}: {}, so the gate would read 0. This is a compiler bug; please report it.",
                w.source.node_id,
                w.source.port.as_str(),
                src.gate_class,
                w.target.node_id,
                w.target.port.as_str(),
                dst.gate_class,
                range.start.line,
                crate::diagnostic::EXEC_INTO_VALUE,
            )
        } else {
            format!(
                "an exec reaches the value input `{}.{}` ({}) of the gate built for line {} through the pin of no \
                 type `{}.{}` ({}): an exec carries no value, only a `bool` reads it (as true for the tick it \
                 fires), so the gate would read 0. Give the pin a type, or test the exec with `if`",
                w.target.node_id,
                w.target.port.as_str(),
                dst.gate_class,
                range.start.line,
                w.source.node_id,
                w.source.port.as_str(),
                src.gate_class,
            )
        };
        found.push(Diagnostic::error(
            crate::diagnostic::INTERNAL_ERROR,
            message,
            range,
        ));
    }
    let at = |d: &Diagnostic| {
        (
            Arc::clone(&d.range.file),
            d.range.start.line,
            d.range.start.col,
            d.message.clone(),
        )
    };
    found.sort_by_key(at);
    diagnostics.extend(found);
}

/// Every name a top-level `let` of the program (imports included) binds.
fn collect_let_names(ast: &Script) -> HashSet<String> {
    ast.decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::Let(l) => Some(crate::const_eval::bound_names(&l.binding)),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Every chip/mod name declared anywhere in the program (imports included, top
/// level and nested), so a use-before-declaration call (WS021) is told apart
/// from a call to a name that is no function at all (a placeholder).
fn collect_fn_names(ast: &Script) -> HashSet<String> {
    fn walk_block(block: &Block, names: &mut HashSet<String>) {
        for s in &block.stmts {
            match s {
                Stmt::ChipDecl(c) => {
                    names.insert(c.name.clone());
                    walk_block(&c.body, names);
                }
                Stmt::AnonChip(ac) => walk_block(&ac.body, names),
                Stmt::Handler(h) => walk_block(&h.body, names),
                Stmt::If(i) => {
                    walk_block(&i.then_block, names);
                    if let Some(eb) = &i.else_block {
                        walk_block(eb, names);
                    }
                }
                // A `mod`/`chip` declared inside a `for` body is still a
                // module-scope name a call elsewhere needs to resolve.
                Stmt::For(f) => walk_block(&f.body, names),
                _ => {}
            }
        }
    }
    let mut names = HashSet::default();
    for d in &ast.decls {
        match d {
            TopDecl::Chip(c) => {
                names.insert(c.name.clone());
                walk_block(&c.body, &mut names);
            }
            TopDecl::AnonChip(ac) => walk_block(&ac.body, &mut names),
            TopDecl::Handler(h) => walk_block(&h.body, &mut names),
            _ => {}
        }
    }
    names
}

/// How many aliases deep [`expand_alias_bodies`] follows a body.
const MAX_ALIAS_CHAIN: u32 = 32;

/// `aliases` with each non-generic alias whose body names another alias or
/// instantiates a generic one (`type IP = Pair<int>`) holding the type that
/// body stands for, so an alias of a record reads as its fields wherever
/// lowering expands one, as the record written out does. A namespaced alias
/// (`Ns.IP`) reads the names in its body in its own module first.
fn expand_alias_bodies(
    aliases: &HashMap<String, TypeExpr>,
    generic: &HashMap<String, crate::types::resolve::GenericAlias>,
) -> HashMap<String, TypeExpr> {
    let mut out = aliases.clone();
    for (key, body) in &mut out {
        if generic.contains_key(key) {
            continue;
        }
        let prefix = key.rsplit_once('.').map(|(ns, _)| format!("{ns}."));
        let own = |name: &str| prefix.as_ref().map(|p| format!("{p}{name}"));
        for _ in 0..MAX_ALIAS_CHAIN {
            let next = match &*body {
                TypeExpr::Name { name, .. } => own(name)
                    .and_then(|n| aliases.get(&n))
                    .or_else(|| aliases.get(name))
                    .filter(|_| !generic.contains_key(name) && name != key)
                    .cloned(),
                TypeExpr::Generic { name, args, .. } => own(name)
                    .and_then(|n| {
                        crate::types::resolve::instantiate_generic_alias(&n, args, generic)
                    })
                    .or_else(|| {
                        crate::types::resolve::instantiate_generic_alias(name, args, generic)
                    }),
                _ => None,
            };
            match next {
                Some(next) => *body = next,
                None => break,
            }
        }
    }
    out
}

/// Collect every generic type alias (`type Pair<T> = { ... }`, top-level and
/// namespaced) into the `name -> (params, body)` map lowering instantiates from.
/// Mirrors typecheck's `collect_generic_aliases` pre-pass so both layers agree
/// on which aliases are generic (non-empty `type_params`).
fn collect_generic_type_aliases(
    decls: &[TopDecl],
) -> HashMap<String, crate::types::resolve::GenericAlias> {
    type_aliases_of(decls)
        .filter(|(_, ta)| !ta.type_params.is_empty())
        .map(|(key, ta)| {
            let generic = crate::types::resolve::GenericAlias {
                params: ta.type_params.iter().map(|tp| tp.name.clone()).collect(),
                body: ta.typ.clone(),
            };
            (key, generic)
        })
        .collect()
}

/// Every `type` alias of the program with its lookup key: the bare name at the
/// top level, `Ns.Name` (the parser's spelling of a qualified type) inside an
/// `import * as Ns` namespace.
fn type_aliases_of(decls: &[TopDecl]) -> impl Iterator<Item = (String, &TypeAliasDecl)> {
    decls.iter().flat_map(|d| {
        let aliases: Vec<(String, &TypeAliasDecl)> = match d {
            TopDecl::TypeAlias(ta) => vec![(ta.name.clone(), ta)],
            TopDecl::Namespace(ns) => ns
                .decls
                .iter()
                .filter_map(|nd| match nd {
                    TopDecl::TypeAlias(ta) => Some((format!("{}.{}", ns.name, ta.name), ta)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        aliases
    })
}

/// Inline a bare source literal wired into a gate's scalar data field of the
/// same native type (`port_accepts_inline_scalar`): write it into the gate's
/// data and drop the wire. Runs before folding, so it sees only source literals
/// and behaves the same with folding on or off. Recurses into chip sub-modules.
fn inline_bare_scalar_literals(root: &mut Module) {
    let external = ExternalWires::of(root);
    root.walk_mut(&mut |_, m| inline_bare_scalar_literals_in(m, &external));
}

/// Nodes with a wire stored outside their own module (a cross-module wire lives
/// in either endpoint's module). A per-module pass judging a node by its own
/// module's wires alone must leave these nodes in place.
struct ExternalWires {
    /// Sources of a wire stored in another module.
    sources: HashSet<NodeId>,
    /// Targets of a wire stored in another module.
    targets: HashSet<NodeId>,
}

impl ExternalWires {
    fn of(root: &Module) -> Self {
        let mut out = Self {
            sources: HashSet::default(),
            targets: HashSet::default(),
        };
        root.walk(&mut |_, module| {
            for w in &module.wires {
                if !module.nodes.contains_key(&w.source.node_id) {
                    out.sources.insert(w.source.node_id);
                }
                if !module.nodes.contains_key(&w.target.node_id) {
                    out.targets.insert(w.target.node_id);
                }
            }
        });
        out
    }

    fn touches(&self, id: NodeId) -> bool {
        self.sources.contains(&id) || self.targets.contains(&id)
    }
}

/// Give every module (and anonymous chip) its own copy of each `_Literal` it
/// reads from another module, moving the reading wire with it. Emit inlines a
/// literal only within its module, so a foreign one reaches the save as nothing;
/// folding a top-level constant read by a named chip produces such wires.
fn localize_foreign_literals(root: &mut Module) {
    fn extract(
        m: &mut Module,
        owner: &HashMap<NodeId, usize>,
        literals: &HashMap<NodeId, Node>,
        moved: &mut Vec<(usize, Wire)>,
    ) {
        let before = m.wires.len();
        m.wires.retain(|w| {
            let foreign = literals.contains_key(&w.source.node_id)
                && owner
                    .get(&w.target.node_id)
                    .is_some_and(|t| owner.get(&w.source.node_id) != Some(t));
            if foreign {
                moved.push((owner[&w.target.node_id], *w));
            }
            !foreign
        });
        if m.wires.len() != before {
            m.template_key = None;
        }
        for child in m.chips.values_mut() {
            extract(child, owner, literals, moved);
        }
    }
    let mut paths: Vec<Vec<NodeId>> = Vec::new();
    let mut owner: HashMap<NodeId, usize> = HashMap::default();
    let mut literals: HashMap<NodeId, Node> = HashMap::default();
    root.walk(&mut |path, m| {
        let here = paths.len();
        paths.push(path.to_vec());
        for (id, n) in &m.nodes {
            owner.insert(*id, here);
            if n.gate_class == gc::LITERAL {
                literals.insert(*id, n.clone());
            }
        }
    });
    if literals.is_empty() {
        return;
    }
    let mut moved: Vec<(usize, Wire)> = Vec::new();
    extract(root, &owner, &literals, &mut moved);
    moved.sort_by_key(|(_, w)| (w.target.node_id, w.source.node_id));

    let mut copies: HashMap<(usize, Option<NodeId>, NodeId), NodeId> = HashMap::default();
    for (at, mut w) in moved {
        let m = root.at_mut(&paths[at]);
        let chip_id = m.nodes.get(&w.target.node_id).and_then(|n| n.chip_id);
        let lit = w.source.node_id;
        let copy = *copies.entry((at, chip_id, lit)).or_insert_with(|| {
            let id = NodeId::fresh();
            let mut node = literals[&lit].clone();
            node.id = id;
            node.chip_id = chip_id;
            m.nodes.insert(id, node);
            id
        });
        w.source.node_id = copy;
        m.wires.push(w);
        m.template_key = None;
    }
}

fn inline_bare_scalar_literals_in(module: &mut Module, external: &ExternalWires) {
    let value_sym = *sym::VALUE;
    // A string literal lowers to a `String_Concatenate` with the text in
    // `InputA` and the other fields empty; that shape with no wired inputs bakes
    // into a `str` field like an int literal does.
    let wired_targets: HashSet<NodeId> = module.wires.iter().map(|w| w.target.node_id).collect();
    let is_empty_str = |n: &Node, k| match n.properties.get(&k) {
        None => true,
        Some(Literal::String(s)) => s.is_empty(),
        _ => false,
    };
    let separator_sym = *sym::SEPARATOR;
    let input_a_sym = *sym::INPUT_A;
    let input_b_sym = *sym::INPUT_B;
    let mut inlines: Vec<(usize, NodeId, WirePort, Literal)> = Vec::new();
    for (i, w) in module.wires.iter().enumerate() {
        let Some(src) = module.nodes.get(&w.source.node_id) else {
            continue;
        };
        let lit = if src.gate_class == gc::LITERAL {
            match src.properties.get(&value_sym) {
                Some(
                    l @ (Literal::Int(_)
                    | Literal::Float(_)
                    | Literal::Bool(_)
                    | Literal::String(_)),
                ) => l.clone(),
                _ => continue,
            }
        } else if src.gate_class == gc::STRING_CONCATENATE
            && !wired_targets.contains(&w.source.node_id)
            && !external.targets.contains(&w.source.node_id)
            && is_empty_str(src, input_b_sym)
            && is_empty_str(src, separator_sym)
        {
            match src.properties.get(&input_a_sym) {
                Some(l @ Literal::String(_)) => l.clone(),
                _ => continue,
            }
        } else {
            continue;
        };
        let Some(target) = module.nodes.get(&w.target.node_id) else {
            continue;
        };
        if !crate::catalog::is_wire_input(target.gate_class, w.target.port.as_str()) {
            continue;
        }
        if !crate::emit::port_accepts_inline_scalar(target.gate_class, w.target.port, &lit) {
            continue;
        }
        inlines.push((i, w.target.node_id, w.target.port, lit));
    }
    let mut drop_wires: Vec<usize> = Vec::with_capacity(inlines.len());
    let mut inlined_sources: HashSet<NodeId> = HashSet::default();
    for (i, target_id, port, lit) in inlines {
        if let Some(target) = module.nodes.get_mut(&target_id) {
            std::sync::Arc::make_mut(&mut target.properties).insert(port.sym(), lit);
            if let Some(w) = module.wires.get(i) {
                inlined_sources.insert(w.source.node_id);
            }
            drop_wires.push(i);
        }
    }
    // Drop the carried wires in one O(W) `retain` pass.
    let drop: HashSet<usize> = drop_wires.into_iter().collect();
    let mut wire_idx = 0usize;
    module.wires.retain(|_| {
        let keep = !drop.contains(&wire_idx);
        wire_idx += 1;
        keep
    });
    // Drop a carrier every consumer took inline here: the fold pass removes
    // these too, so leaving them would make the module differ by fold mode.
    if !inlined_sources.is_empty() {
        let live_sources: HashSet<NodeId> = module.wires.iter().map(|w| w.source.node_id).collect();
        inlined_sources.retain(|id| !live_sources.contains(id) && !external.sources.contains(id));
    }
    if !inlined_sources.is_empty() {
        module.nodes.retain(|id, _| !inlined_sources.contains(id));
    }
}

/// A gate that carries a constant `_Literal` to a consumer that cannot take it
/// inline: the gate class, its output port, and the value as data on its
/// unwired inputs, in the order they are inserted.
struct Carrier {
    class: &'static str,
    out_sym: Sym,
    out_port: WirePort,
    ty: Type,
    data: Vec<(Sym, Literal)>,
}

impl Carrier {
    /// Whether `lit` is carried by a `Make*` gate (vector, rotator, color),
    /// which needs no check of what its consumer accepts beyond the inline test.
    fn is_composite(lit: &Literal) -> bool {
        matches!(
            lit,
            Literal::Vector { .. } | Literal::Rotator { .. } | Literal::LinearColor { .. }
        )
    }

    /// Whether any gate can carry `lit`.
    fn exists_for(lit: &Literal) -> bool {
        Self::is_composite(lit)
            || matches!(
                lit,
                Literal::Int(_)
                    | Literal::Float(_)
                    | Literal::Bool(_)
                    | Literal::String(_)
                    | Literal::Quat { .. }
            )
    }

    /// The carrier for `lit`; `None` when no gate carries its kind. A scalar
    /// gets a pure gate whose unwired inputs hold the value (a Variable gate
    /// will not do; its Value output stays null until an exec write), and a
    /// `Quat`, which has no arithmetic identity gate, a `MakeQuaternion`.
    fn of(lit: &Literal) -> Option<Self> {
        let make = |class, ty, fields: Vec<(WirePort, f64)>| Self {
            class,
            out_sym: *sym::OUTPUT,
            out_port: WirePort::Output,
            ty,
            data: fields
                .into_iter()
                .map(|(port, v)| (port.sym(), Literal::Float(v)))
                .collect(),
        };
        Some(match lit {
            Literal::Vector { x, y, z } => make(
                gc::MAKE_VECTOR,
                Type::Vector,
                vec![(WirePort::X, *x), (WirePort::Y, *y), (WirePort::Z, *z)],
            ),
            Literal::Rotator { pitch, yaw, roll } => make(
                gc::MAKE_ROTATION,
                Type::Rotator,
                vec![
                    (WirePort::Pitch, *pitch),
                    (WirePort::Yaw, *yaw),
                    (WirePort::Roll, *roll),
                ],
            ),
            Literal::LinearColor { r, g, b, a } => make(
                gc::MAKE_COLOR,
                Type::Color,
                vec![
                    (WirePort::R, *r),
                    (WirePort::G, *g),
                    (WirePort::B, *b),
                    (WirePort::A, *a),
                ],
            ),
            Literal::Quat { x, y, z, w } => make(
                gc::MAKE_QUATERNION,
                Type::Quat,
                vec![
                    (WirePort::X, *x),
                    (WirePort::Y, *y),
                    (WirePort::Z, *z),
                    (WirePort::W, *w),
                ],
            ),
            Literal::Int(n) => Self {
                class: gc::MATH_ADD,
                out_sym: *sym::OUTPUT,
                out_port: WirePort::Output,
                ty: Type::Int,
                data: vec![
                    (*sym::INPUT_A, Literal::Int(*n)),
                    (*sym::INPUT_B, Literal::Int(0)),
                ],
            },
            Literal::Float(f) => Self {
                class: gc::MATH_ADD,
                out_sym: *sym::OUTPUT,
                out_port: WirePort::Output,
                ty: Type::Float,
                data: vec![
                    (*sym::INPUT_A, Literal::Float(*f)),
                    (*sym::INPUT_B, Literal::Float(0.0)),
                ],
            },
            Literal::Bool(b) => Self {
                class: gc::LOGICAL_OR,
                out_sym: *sym::B_OUTPUT,
                out_port: WirePort::BOutput,
                ty: Type::Bool,
                data: vec![
                    (*sym::B_INPUT_A, Literal::Bool(*b)),
                    (*sym::B_INPUT_B, Literal::Bool(false)),
                ],
            },
            Literal::String(text) => Self {
                class: gc::STRING_CONCATENATE,
                out_sym: *sym::OUTPUT,
                out_port: WirePort::Output,
                ty: Type::String,
                data: vec![
                    (*sym::INPUT_A, Literal::String(text.clone())),
                    (*sym::INPUT_B, Literal::String(String::new())),
                    (*sym::SEPARATOR, Literal::String(String::new())),
                ],
            },
            _ => return None,
        })
    }

    /// The gate node `id`, placed like `src`; `_nofold` is copied when `no_fold`.
    fn node(self, id: NodeId, src: &Node, no_fold: bool, note: &'static str) -> Node {
        let mut properties: HashMap<Sym, Literal> = HashMap::default();
        for (k, v) in self.data {
            properties.insert(k, v);
        }
        if no_fold {
            properties.insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        let ports = GateIO {
            inputs: vec![],
            outputs: vec![PortSpec::new(self.out_sym, self.ty)],
        };
        Node::new(
            id,
            NodeKind::Gate,
            self.class,
            ports,
            properties,
            Some(src),
            Some(note),
        )
    }
}

/// What a scalar or `Quat` constant wired into one port needs.
enum ScalarFeed {
    /// The port already holds it, or there is nothing to deliver to.
    Keep,
    /// A data field with no input port: write the value into the target.
    WriteData,
    /// A carrier gate feeds the port.
    Carry,
}

/// Decide how the constant `lit`, wired by `w`, reaches its target. The target
/// may sit in a chip child (a named chip's `MicrochipInput` fed by a parent
/// constant); emit delivers no literal across modules, so such a pin would read
/// 0 and gets a parent-side carrier too.
fn scalar_feed(module: &Module, w: &Wire, lit: &Literal) -> ScalarFeed {
    let (target, cross_module) = match module.find_deep(w.target.node_id) {
        Some(t) => (t, !module.nodes.contains_key(&w.target.node_id)),
        // A dangling wire has nothing to deliver to.
        None => return ScalarFeed::Keep,
    };
    // Inline data works only within a module, so a cross-module target needs
    // the carrier even when its port could hold the value.
    let target_ok = !cross_module
        && crate::emit::port_accepts_inline_variant(target.gate_class, w.target.port, lit);
    let target_placeholder =
        target.gate_class == gc::LITERAL || target.gate_class == gc::UNSUPPORTED;
    if target_ok || target_placeholder {
        return ScalarFeed::Keep;
    }
    let takes_wire = crate::catalog::is_wire_input(target.gate_class, w.target.port.as_str());
    // A cross-module data-only port can neither take a wire nor be written from
    // this module's pass, so the wire is left alone.
    if cross_module && !takes_wire {
        return ScalarFeed::Keep;
    }
    // A data field with no input port (DisplayText.FontSize) takes no wire
    // (emit fails the brdb port lookup), so the value is written into the
    // target's data and the wire dropped.
    if takes_wire {
        ScalarFeed::Carry
    } else {
        ScalarFeed::WriteData
    }
}

/// Give a constant `_Literal` a real carrier gate wherever its consumer cannot
/// take it as inline data: a `Make*` gate for a vector, rotator, color or quat
/// (entity gates, `Split*` inputs, chip IO), and an identity gate for a scalar,
/// so a constant is never silently dropped. Recurses into chip sub-modules.
fn materialize_unfoldable_constants(module: &mut Module) {
    let value_sym = *sym::VALUE;
    let mut make_nodes: Vec<Node> = Vec::new();
    let mut make_for: HashMap<NodeId, NodeId> = HashMap::default();
    let mut rewires: Vec<(usize, NodeId, WirePort)> = Vec::new();
    // (wire index, target node, target port, value) for constants bound to a
    // port that carries data but takes no wire: written as data, wire dropped.
    let mut inlines: Vec<(usize, NodeId, WirePort, Literal)> = Vec::new();

    for (i, w) in module.wires.iter().enumerate() {
        let Some(src) = module.nodes.get(&w.source.node_id) else {
            continue;
        };
        if src.gate_class != gc::LITERAL {
            continue;
        }
        let Some(lit) = src
            .properties
            .get(&value_sym)
            .filter(|l| Carrier::exists_for(l))
        else {
            continue;
        };
        let (note, no_fold) = if Carrier::is_composite(lit) {
            let target_ok = module.nodes.get(&w.target.node_id).is_some_and(|t| {
                crate::emit::port_accepts_inline_variant(t.gate_class, w.target.port, lit)
            });
            if target_ok {
                continue;
            }
            ("materialized constant", false)
        } else {
            match scalar_feed(module, w, lit) {
                ScalarFeed::Keep => continue,
                ScalarFeed::WriteData => {
                    inlines.push((i, w.target.node_id, w.target.port, lit.clone()));
                    continue;
                }
                ScalarFeed::Carry => {}
            }
            (
                "materialized constant (dataless target)",
                src.properties.contains_key(&*sym::NO_FOLD),
            )
        };
        let carrier = Carrier::of(lit).expect("a literal with a carrier");
        let out_port = carrier.out_port;
        let make_id = *make_for.entry(w.source.node_id).or_insert_with(|| {
            let id = NodeId::fresh();
            make_nodes.push(carrier.node(id, src, no_fold, note));
            id
        });
        rewires.push((i, make_id, out_port));
    }

    for n in make_nodes {
        module.nodes.insert(n.id, n);
    }
    for (i, make_id, port) in rewires {
        module.wires[i].source = PortRef {
            node_id: make_id,
            port,
        };
    }
    // Write data-only port values onto their targets, then drop the wires that
    // carried them in one retain pass.
    let mut drop_wires: Vec<usize> = Vec::with_capacity(inlines.len());
    for (i, target_id, port, lit) in inlines {
        if let Some(target) = module.nodes.get_mut(&target_id) {
            std::sync::Arc::make_mut(&mut target.properties).insert(port.sym(), lit);
        }
        drop_wires.push(i);
    }
    let drop: HashSet<usize> = drop_wires.into_iter().collect();
    let mut wire_idx = 0usize;
    module.wires.retain(|_| {
        let keep = !drop.contains(&wire_idx);
        wire_idx += 1;
        keep
    });
    for child_module in module.chips.values_mut() {
        materialize_unfoldable_constants(child_module);
    }
}

/// Fold a standalone `_Literal` whose value is used once (`n + 1`, `n > 10`)
/// into a property on the consumer gate and delete the literal. Recurses into
/// chip sub-modules.
fn inline_orphan_literals(root: &mut Module) {
    let external = ExternalWires::of(root);
    root.walk_mut(&mut |_, m| inline_orphan_literals_in(m, &external));
}

fn inline_orphan_literals_in(module: &mut Module, external: &ExternalWires) {
    let value_sym = *sym::VALUE;
    // Per-source (count, first target): the single-consumer check needs only
    // the one target. Buffers are reused across fixpoint iterations.
    let mut outgoing: HashMap<NodeId, (usize, PortRef)> =
        HashMap::with_capacity_and_hasher(module.nodes.len(), rustc_hash::FxBuildHasher);
    let mut incoming_count: HashMap<NodeId, usize> =
        HashMap::with_capacity_and_hasher(module.nodes.len(), rustc_hash::FxBuildHasher);
    let mut lit_ids: Vec<NodeId> = Vec::new();
    let mut concat_ids: Vec<NodeId> = Vec::new();
    let mut removed: HashSet<NodeId> = HashSet::default();
    // Built once, then maintained incrementally: a fold removes only the folded
    // node and its one outgoing wire, so only the target's incoming count changes.
    for w in &module.wires {
        let e = outgoing.entry(w.source.node_id).or_insert((0, w.target));
        e.0 += 1;
        *incoming_count.entry(w.target.node_id).or_default() += 1;
    }
    // A node with a wire in another module is not single-use however many
    // wires this module holds for it; inlining it would strand those wires.
    loop {
        lit_ids.clear();
        lit_ids.extend(
            module
                .nodes
                .iter()
                .filter(|(id, n)| n.gate_class == gc::LITERAL && !external.touches(**id))
                .map(|(id, _)| *id),
        );
        let mut changed = false;
        for &lit_id in &lit_ids {
            let Some(&(out_len, target)) = outgoing.get(&lit_id) else {
                continue;
            };
            if out_len != 1 {
                continue;
            }
            if incoming_count.get(&lit_id).copied().unwrap_or(0) != 0 {
                continue;
            }
            let (target_id, target_port) = (target.node_id, target.port);
            // A cross-module target (a chip's `MicrochipInput` fed by a parent
            // literal) cannot take the property from here; deleting the literal
            // would leave the pin reading its default, so leave it wired.
            if !module.nodes.contains_key(&target_id) {
                continue;
            }
            // A rerouter has no data struct, so emit would silently drop a
            // property; keep it wired.
            if module
                .nodes
                .get(&target_id)
                .is_some_and(|t| t.gate_class == gc::REROUTER)
            {
                continue;
            }
            let Some(value) = module
                .nodes
                .get(&lit_id)
                .and_then(|n| n.properties.get(&value_sym).cloned())
            else {
                continue;
            };
            let target_port_sym = target_port.sym();
            if let Some(target) = module.nodes.get_mut(&target_id) {
                std::sync::Arc::make_mut(&mut target.properties)
                    .entry(target_port_sym)
                    .or_insert(value);
            }
            module.nodes.remove(&lit_id);
            removed.insert(lit_id);
            outgoing.remove(&lit_id);
            if let Some(c) = incoming_count.get_mut(&target_id) {
                *c = c.saturating_sub(1);
            }
            changed = true;
        }

        // Fold constant-string `String_Concatenate` wrappers into consumers
        // that accept an inline string variant (`port_accepts_inline_variant`);
        // a wire-only port keeps the concat gate.
        concat_ids.clear();
        concat_ids.extend(
            module
                .nodes
                .iter()
                .filter(|(id, n)| {
                    n.gate_class == gc::STRING_CONCATENATE
                        && !removed.contains(id)
                        && !external.touches(**id)
                })
                .map(|(id, _)| *id),
        );
        for &cid in &concat_ids {
            if incoming_count.get(&cid).copied().unwrap_or(0) != 0 {
                continue;
            }
            let Some(&(1, target)) = outgoing.get(&cid) else {
                continue;
            };
            let (target_id, target_port) = (target.node_id, target.port);
            // Only a single constant string: INPUT_A set, INPUT_B and Separator
            // empty.
            let text = {
                let Some(node) = module.nodes.get(&cid) else {
                    continue;
                };
                let Some(Literal::String(text)) = node.properties.get(&*sym::INPUT_A).cloned()
                else {
                    continue;
                };
                let is_empty = |k| match node.properties.get(&k) {
                    None => true,
                    Some(Literal::String(s)) => s.is_empty(),
                    _ => false,
                };
                if !is_empty(*sym::INPUT_B) || !is_empty(*sym::SEPARATOR) {
                    continue;
                }
                text
            };
            let text_lit = Literal::String(text.clone());
            let accepts = module.nodes.get(&target_id).is_some_and(|t| {
                crate::emit::port_accepts_inline_variant(t.gate_class, target_port, &text_lit)
            });
            if !accepts {
                continue;
            }
            let target_port_sym = target_port.sym();
            if let Some(t) = module.nodes.get_mut(&target_id) {
                std::sync::Arc::make_mut(&mut t.properties)
                    .entry(target_port_sym)
                    .or_insert(Literal::String(text));
            }
            module.nodes.remove(&cid);
            removed.insert(cid);
            outgoing.remove(&cid);
            if let Some(c) = incoming_count.get_mut(&target_id) {
                *c = c.saturating_sub(1);
            }
            changed = true;
        }

        if !changed {
            break;
        }
    }
    // Removed nodes had no incoming wires, so one sweep of source-side wires
    // covers every iteration's folds.
    if !removed.is_empty() {
        module
            .wires
            .retain(|w| !removed.contains(&w.source.node_id));
    }
}

/// Merge identical constant gates within each chip so a repeated constant is
/// emitted once and fans out. A constant is a pure gate (`_Literal` or `Expr_*`)
/// with no incoming data wire, so class and properties determine its output.
/// Per module, so no wire is rewired across a microchip boundary.
fn dedup_constant_gates(root: &mut Module) {
    // Canonical key for a constant gate: class + its properties (sorted, so the
    // key is order-independent). `Debug` on `Literal` is deterministic per value.
    fn const_key(n: &Node) -> String {
        let mut kv: Vec<(&str, String)> = n
            .properties
            .iter()
            .map(|(k, v)| (resolve(*k), format!("{v:?}")))
            .collect();
        kv.sort_unstable();
        format!("{}\u{1}{kv:?}", n.gate_class)
    }

    // dup node id -> keeper node id. Same class means same output ports, so
    // each consumer keeps its `source.port` and only the node id changes.
    let mut redirect: HashMap<NodeId, NodeId> = HashMap::default();

    fn collect(
        module: &mut Module,
        external: &ExternalWires,
        redirect: &mut HashMap<NodeId, NodeId>,
    ) {
        let has_incoming: HashSet<NodeId> = module
            .wires
            .iter()
            .filter(|w| w.target.port != WirePort::Layout)
            .map(|w| w.target.node_id)
            .collect();
        // Key by (owning anon-chip, const_key): this runs before
        // `partition_anon_chips`, and a keeper in a different chip than its
        // consumer would leave the operand reading its port default (0).
        let mut groups: HashMap<(Option<NodeId>, String), Vec<NodeId>> = HashMap::default();
        for (id, n) in &module.nodes {
            let is_pure = n.gate_class == gc::LITERAL || gc::is_pure_expr(n.gate_class);
            let fed = has_incoming.contains(id) || external.targets.contains(id);
            if n.kind == NodeKind::Gate && is_pure && !fed {
                groups
                    .entry((n.chip_id, const_key(n)))
                    .or_default()
                    .push(*id);
            }
        }
        for mut group in groups.into_values() {
            if group.len() < 2 {
                continue;
            }
            group.sort_unstable(); // deterministic keeper (lowest id)
            let keeper = group[0];
            for dup in &group[1..] {
                redirect.insert(*dup, keeper);
                module.nodes.remove(dup);
            }
        }
    }

    let external = ExternalWires::of(root);
    root.walk_mut(&mut |_, m| collect(m, &external, &mut redirect));
    if redirect.is_empty() {
        return;
    }
    root.walk_mut(&mut |_, m| apply_gate_redirect(m, &redirect));
}

/// Rewrite one module after merging gates: wires from a dropped dup source
/// from its keeper, wires into a dup are removed (the keeper has identical feeds),
/// duplicate wires collapse, and captures and style refs follow the keeper.
/// Node ids are globally unique, so one `redirect` map serves the whole tree.
fn apply_gate_redirect(module: &mut Module, redirect: &HashMap<NodeId, NodeId>) {
    module.wires.retain_mut(|w| {
        if let Some(&keeper) = redirect.get(&w.source.node_id) {
            w.source.node_id = keeper;
        }
        !redirect.contains_key(&w.target.node_id)
    });
    // Redirection can collapse two wires onto the same source->target pair.
    let mut seen: HashSet<(PortRef, PortRef)> = HashSet::default();
    module.wires.retain(|w| seen.insert((w.source, w.target)));
    // A runtime `@label` / `@color` / `@material` source is a port reference,
    // not a wire, so it must follow its dup to the keeper or emit finds a
    // deleted node.
    let follow = |p: &mut PortRef| {
        if let Some(&keeper) = redirect.get(&p.node_id) {
            p.node_id = keeper;
        }
    };
    module
        .dynamic_labels
        .values_mut()
        .chain(module.dynamic_colors.values_mut())
        .chain(module.dynamic_materials.values_mut())
        .chain(module.root_dynamic_label.iter_mut())
        .for_each(follow);
    for style in [
        &mut module.root_style.color,
        &mut module.root_style.material,
    ] {
        if let Some(crate::ir::StyleValue::Wired(p)) = style {
            follow(p);
        }
    }
    // A dropped dup referenced as an external capture becomes its keeper.
    if !module.scope_captures.is_empty() {
        let mut seen_caps = HashSet::default();
        module.scope_captures = module
            .scope_captures
            .iter()
            .map(|id| *redirect.get(id).unwrap_or(id))
            .filter(|id| seen_caps.insert(*id))
            .collect();
    }
}

/// Common-subexpression elimination: within each chip, merge `Expr_*` gates
/// with the same class, properties, output types and incoming wires, so a
/// repeated pure subexpression is emitted once and fans out.
///
/// One pass in topological order, since a gate's key uses its inputs' canonical
/// sources. Keyed on `chip_id`, like `dedup_constant_gates`. Excludes the
/// stateful change/edge detectors and `_nofold` gates. Merging only redirects
/// wire sources, so it never creates fan-in.
fn cse_pure_gates(root: &mut Module) {
    fn eligible(n: &Node) -> bool {
        n.kind == NodeKind::Gate
            && gc::is_pure_expr(n.gate_class)
            && !n.gate_class.contains("ChangeDetector")
            && !n.gate_class.contains("EdgeDetector")
            && !n.properties.contains_key(&*sym::NO_FOLD)
    }

    // Output types are part of the key because a Convert gate's target type is
    // its `Output` port's type (`x as int` vs `x as string`). Property values
    // (f64s have no Eq/Hash) and output types go through a Debug render.
    type CseKey = (
        &'static str,
        Vec<(&'static str, String)>,
        Vec<(WirePort, NodeId, WirePort)>,
        Vec<String>,
    );
    fn cse_key(n: &Node, incoming: Option<&Vec<(WirePort, NodeId, WirePort)>>) -> CseKey {
        let mut props: Vec<(&'static str, String)> = n
            .properties
            .iter()
            .map(|(k, v)| (resolve(*k), format!("{v:?}")))
            .collect();
        props.sort_unstable();
        let mut ins: Vec<(WirePort, NodeId, WirePort)> = incoming.cloned().unwrap_or_default();
        ins.sort_unstable_by_key(|(tp, sid, sp)| (*tp as u16, sid.0, *sp as u16));
        let outs = n
            .ports
            .outputs
            .iter()
            .map(|p| format!("{}:{:?}", resolve(p.name), p.ty))
            .collect();
        (n.gate_class, props, ins, outs)
    }

    fn cse_module(
        module: &mut Module,
        external: &ExternalWires,
        redirect: &mut HashMap<NodeId, NodeId>,
    ) {
        // Per-node incoming data wires; Layout edges are not inputs.
        let mut incoming: HashMap<NodeId, Vec<(WirePort, NodeId, WirePort)>> = HashMap::default();
        for w in &module.wires {
            if w.target.port == WirePort::Layout {
                continue;
            }
            incoming.entry(w.target.node_id).or_default().push((
                w.target.port,
                w.source.node_id,
                w.source.port,
            ));
        }

        // In-degree over the eligible-gate subgraph (an edge is an eligible
        // source feeding an eligible consumer), for a Kahn topological order.
        let eligible_ids: HashSet<NodeId> = module
            .nodes
            .iter()
            .filter(|(id, n)| eligible(n) && !external.targets.contains(id))
            .map(|(id, _)| *id)
            .collect();
        let mut indeg: HashMap<NodeId, usize> = HashMap::default();
        let mut succ: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
        for id in &eligible_ids {
            let preds: HashSet<NodeId> = incoming
                .get(id)
                .into_iter()
                .flatten()
                .map(|(_, sid, _)| *sid)
                .filter(|sid| eligible_ids.contains(sid))
                .collect();
            indeg.insert(*id, preds.len());
            for p in preds {
                succ.entry(p).or_default().push(*id);
            }
        }
        let mut queue: Vec<NodeId> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(id, _)| *id)
            .collect();
        queue.sort_unstable(); // deterministic keeper selection

        // A node's key uses its sources canonicalized through committed merges,
        // final in topo order. Gates in a combinational cycle never reach
        // in-degree zero and stay unmerged.
        let mut seen: HashMap<(Option<NodeId>, CseKey), NodeId> = HashMap::default();
        let mut qi = 0;
        while qi < queue.len() {
            let id = queue[qi];
            qi += 1;
            if let Some(n) = module.nodes.get(&id) {
                let key_ins: Option<Vec<(WirePort, NodeId, WirePort)>> =
                    incoming.get(&id).map(|ins| {
                        ins.iter()
                            .map(|(tp, sid, sp)| (*tp, *redirect.get(sid).unwrap_or(sid), *sp))
                            .collect()
                    });
                let key = (n.chip_id, cse_key(n, key_ins.as_ref()));
                match seen.get(&key) {
                    Some(&keeper) => {
                        redirect.insert(id, keeper);
                        module.nodes.remove(&id);
                    }
                    None => {
                        seen.insert(key, id);
                    }
                }
            }
            // Release consumers whose last eligible predecessor was just handled.
            let mut newly: Vec<NodeId> = Vec::new();
            if let Some(cs) = succ.get(&id) {
                for &c in cs {
                    if let Some(d) = indeg.get_mut(&c) {
                        *d -= 1;
                        if *d == 0 {
                            newly.push(c);
                        }
                    }
                }
            }
            newly.sort_unstable();
            queue.extend(newly);
        }

        for child in module.chips.values_mut() {
            cse_module(child, external, redirect);
        }
    }

    let mut redirect: HashMap<NodeId, NodeId> = HashMap::default();
    let external = ExternalWires::of(root);
    cse_module(root, &external, &mut redirect);
    if !redirect.is_empty() {
        root.walk_mut(&mut |_, m| apply_gate_redirect(m, &redirect));
    }
}

/// Remove pure gates no data wire touches on either side (a `let` duplicated by
/// a namespace plus a named import of one module). Only fully wireless gates go:
/// an unused `let y = x * 2` still has input wires and is kept.
///
/// Connectivity spans the whole module tree, as wires cross modules.
/// `literals_only` limits pruning to `_Literal` gates, for the run after literal
/// inlining, when an `Expr_*` gate can be wireless with its operands inlined.
fn prune_dead_pure_gates(module: &mut Module, literals_only: bool) {
    let is_pure = |gate_class: &str| -> bool {
        gate_class == gc::LITERAL || (!literals_only && gc::is_pure_expr(gate_class))
    };
    // Node ids touched by a non-Layout wire, as source or target.
    let mut connected = HashSet::default();
    for w in module.wires_deep() {
        if w.source.port != WirePort::Layout {
            connected.insert(w.source.node_id);
        }
        if w.target.port != WirePort::Layout {
            connected.insert(w.target.node_id);
        }
    }
    let dead = module.node_ids_where(|n| {
        n.kind == NodeKind::Gate && is_pure(n.gate_class) && !connected.contains(&n.id)
    });
    // Only Layout wires can touch a dead node (it has no data wires); they go too.
    if !dead.is_empty() {
        module.remove_nodes_tree(&dead);
    }
}

/// Drop each `Select` a choice between two gate reads built for an output no
/// place reads (`f = t` after `let t = if c then fs.max() else fs.min()`
/// reads `Value`, never `IsEmpty`). Connectivity spans the whole module tree,
/// as in [`prune_dead_pure_gates`].
fn prune_unread_gate_read_choices(module: &mut Module) {
    let was_read: HashSet<NodeId> = module
        .wires_deep()
        .filter(|w| w.source.port != WirePort::Layout)
        .map(|w| w.source.node_id)
        .collect();
    let dead = module
        .node_ids_where(|n| n.note == Some(stmt::GATE_READ_CHOICE) && !was_read.contains(&n.id));
    if !dead.is_empty() {
        module.remove_nodes_tree(&dead);
    }
}

/// Clean up degenerate `Exec_Union` nodes, repeating to a fixpoint (each
/// removal can degrade another union). Recurses into chip sub-modules.
///
/// - **No outgoing wires** (sink): remove the union and its incoming wires.
/// - **No incoming wires** (dead source, e.g. an if-join whose branches both
///   `return`): remove it and its outgoing wires.
/// - **Exactly one incoming wire** (pass-through): splice it out, rewiring its
///   consumers to the single source.
///
/// Degrees come from one module's wire list, so a union with a wire in another
/// module (a top-level `let sig: exec` read inside a `chip`) is left alone;
/// deleting it would strand that wire (`EmitError::DroppedWire`).
pub(super) fn prune_dead_exec_unions(module: &mut Module) {
    let external = ExternalWires::of(module);
    module.walk_mut(&mut |_, m| prune_dead_exec_unions_in(m, &external));
}

fn prune_dead_exec_unions_in(module: &mut Module, external: &ExternalWires) {
    /// Chase a source through spliced unions to the node actually carrying
    /// its wires now. Returns `None` for a pure splice cycle (unions feeding
    /// only each other, whose wires all drop).
    fn resolve_src(spliced: &HashMap<NodeId, PortRef>, start: PortRef) -> Option<PortRef> {
        let mut cur = start;
        let mut hops = 0usize;
        while let Some(next) = spliced.get(&cur.node_id) {
            cur = *next;
            hops += 1;
            if hops > spliced.len() {
                return None;
            }
        }
        Some(cur)
    }

    // Degrees and adjacency are computed once and maintained through a worklist.
    let queue: Vec<NodeId> = module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::UNION)
        .map(|(id, _)| *id)
        // Referenced from another module: its real degree is not visible here.
        .filter(|id| !external.touches(*id))
        .collect();
    if queue.is_empty() {
        return;
    }
    let is_union: HashSet<NodeId> = queue.iter().copied().collect();
    let mut queue = queue;

    // Counts and adjacency are only read for unions, so only union entries are
    // tracked; decrements on other nodes fall through `get_mut`.
    let mut in_count: HashMap<NodeId, usize> = HashMap::default();
    let mut out_count: HashMap<NodeId, usize> = HashMap::default();
    let mut in_edges: HashMap<NodeId, Vec<PortRef>> = HashMap::default();
    let mut out_edges: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    for w in &module.wires {
        if is_union.contains(&w.source.node_id) {
            *out_count.entry(w.source.node_id).or_default() += 1;
            out_edges
                .entry(w.source.node_id)
                .or_default()
                .push(w.target.node_id);
        }
        if is_union.contains(&w.target.node_id) {
            *in_count.entry(w.target.node_id).or_default() += 1;
            in_edges.entry(w.target.node_id).or_default().push(w.source);
        }
    }

    let mut removed: HashSet<NodeId> = HashSet::default();
    let mut spliced: HashMap<NodeId, PortRef> = HashMap::default();

    while let Some(id) = queue.pop() {
        if removed.contains(&id) || spliced.contains_key(&id) {
            continue;
        }
        let ins = in_count.get(&id).copied().unwrap_or(0);
        let outs = out_count.get(&id).copied().unwrap_or(0);
        if ins == 0 || outs == 0 {
            // Dead sink/source: remove the union; its live edges die with it,
            // so decrement each live neighbor and requeue affected unions.
            removed.insert(id);
            for s in in_edges.get(&id).into_iter().flatten() {
                let Some(src) = resolve_src(&spliced, *s) else {
                    continue;
                };
                if removed.contains(&src.node_id) {
                    continue; // edge already died with its source
                }
                if let Some(c) = out_count.get_mut(&src.node_id) {
                    *c = c.saturating_sub(1);
                }
                if is_union.contains(&src.node_id) {
                    queue.push(src.node_id);
                }
            }
            for t in out_edges.get(&id).into_iter().flatten() {
                if removed.contains(t) || spliced.contains_key(t) {
                    continue; // edge already accounted for at the other end
                }
                if let Some(c) = in_count.get_mut(t) {
                    *c = c.saturating_sub(1);
                }
                if is_union.contains(t) {
                    queue.push(*t);
                }
            }
        } else if ins == 1 {
            // Pass-through: splice out. Consumers keep their in-degree (the
            // wires just change source); the carrier gains this union's
            // out-edges and loses the one edge that fed it.
            let raw = in_edges
                .get(&id)
                .into_iter()
                .flatten()
                .filter_map(|s| resolve_src(&spliced, *s))
                .find(|s| !removed.contains(&s.node_id))
                .expect("counted one incoming wire");
            spliced.insert(id, raw);
            let transferred = out_edges.remove(&id).unwrap_or_default();
            // Only live transferred edges count toward the carrier's degree;
            // edges to removed or spliced targets were already discounted.
            let live = transferred
                .iter()
                .filter(|t| !removed.contains(t) && !spliced.contains_key(t))
                .count();
            if let Some(c) = out_count.get_mut(&raw.node_id) {
                *c += live;
                *c = c.saturating_sub(1);
            }
            // Adjacency is only read back for unions; drop transfers to
            // non-union carriers.
            if is_union.contains(&raw.node_id) {
                out_edges
                    .entry(raw.node_id)
                    .or_default()
                    .extend(transferred);
            }
        }
    }

    if !removed.is_empty() || !spliced.is_empty() {
        for id in removed.iter().chain(spliced.keys()) {
            module.nodes.remove(id);
        }
        module.wires.retain_mut(|w| {
            // Wires into a removed union die; wires into a spliced union are
            // superseded by the rewired consumer edges.
            if removed.contains(&w.target.node_id) || spliced.contains_key(&w.target.node_id) {
                return false;
            }
            if removed.contains(&w.source.node_id) {
                return false;
            }
            if spliced.contains_key(&w.source.node_id) {
                let Some(src) = resolve_src(&spliced, w.source) else {
                    return false;
                };
                if removed.contains(&src.node_id) {
                    return false;
                }
                w.source = src;
            }
            true
        });
    }
}

/// Compile a standalone chip declaration into an isolated [`Module`] for a
/// [`CompiledTemplate`]: the child context `lower_chip_call_instance` builds,
/// without any parent-side wiring.
pub(crate) fn compile_chip_template(
    chip_decl: &ChipDecl,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
) -> Module {
    let template_name = &chip_decl.name;

    let mut builder = ModuleBuilder::new(template_name);
    builder.module.scopes.insert(
        ROOT_SCOPE_ID,
        ScopeInfo {
            kind: ScopeKind::ChipBody {
                name: chip_decl.name.clone(),
            },
            source_range: chip_decl.range.clone(),
            parent: None,
        },
    );

    // A gate-count estimate needs no doc text and no custom-event slots (an
    // unresolved slot estimates as `float`), so both maps are empty.
    let empty_docs = crate::parser::DocComments::default();
    let empty_ce_slots = CeSlotMap::default();

    let mut ctx = LowerCtx {
        file: file.to_string(),
        template_cache: Arc::clone(cache),
        chip_call_stack: if chip_decl.name.is_empty() {
            Vec::new()
        } else {
            vec![(chip_decl.range.clone(), true)]
        },
        template_build: true,
        imports: Arc::clone(&tc.imports),
        // No whole-program decls here, so aliases and the enum registry stay
        // empty and an enum-typed var falls back to a bare scalar
        // (`declare_enum_container`).
        ..LowerCtx::empty(
            builder,
            &tc.type_of_expr,
            &tc.op_resolutions,
            &tc.resolutions,
            &empty_ce_slots,
            &empty_docs,
        )
    };

    for inp in &chip_decl.inputs {
        let resolved_record = ctx.record_fields_of(&inp.typ);
        if let Some(fields) = &resolved_record {
            let mut record_fields = HashMap::default();
            for field in fields {
                let port_name = format!("{}_{}", inp.name, field.name);
                let ft = ctx.resolve_local_type(&field.typ);
                let shape = ctx.declared_shape(None, &field.typ);
                let is_array = matches!(shape, TypeExpr::Array { .. });
                let is_ref = matches!(shape, TypeExpr::Ref { .. });
                let node_id = ctx.add_input(&port_name, ft.clone(), chip_decl.range.clone());
                let binding = if is_array {
                    let inner = match &ft {
                        Type::Array(inner) => inner.as_ref().clone(),
                        Type::Ref(inner) => match inner.as_ref() {
                            Type::Array(inner) => inner.as_ref().clone(),
                            _ => ft.clone(),
                        },
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord::new(node_id, inner, VarStorage::Array))
                } else if is_ref {
                    let inner = match &ft {
                        Type::Ref(inner) => inner.as_ref().clone(),
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord::new(node_id, inner, VarStorage::Var))
                } else {
                    Binding::Input(NodeRecord {
                        node_id,
                        ty: ft.clone(),
                    })
                };
                record_fields.insert(intern(&field.name), binding);
            }
            ctx.scope.insert(&inp.name, Binding::Record(record_fields));
        } else if ctx.ref_or_array(Some(chip_decl), &inp.typ) {
            let t = ctx.resolve_local_type(&inp.typ);
            let is_array = matches!(
                ctx.declared_shape(Some(chip_decl), &inp.typ),
                TypeExpr::Array { .. }
            );
            let inner = match &t {
                Type::Ref(inner) => inner.as_ref().clone(),
                Type::Array(inner) => inner.as_ref().clone(),
                _ => t.clone(),
            };
            let node_id = ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
            ctx.scope.insert(
                &inp.name,
                Binding::Var(VarRecord::new(
                    node_id,
                    inner,
                    if is_array {
                        VarStorage::Array
                    } else {
                        VarStorage::Var
                    },
                )),
            );
        } else {
            let t = ctx.resolve_local_type(&inp.typ);
            let node_id = ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
            ctx.scope
                .insert(&inp.name, Binding::Input(NodeRecord { node_id, ty: t }));
        }
    }

    for out in &chip_decl.outputs {
        let t = ctx.resolve_local_type(&out.typ);
        let node_id = ctx.add_output(&out.name, t.clone(), chip_decl.range.clone());
        ctx.scope.insert(
            &output_scope_key(&out.name),
            Binding::Output(NodeRecord { node_id, ty: t }),
        );
    }

    let sig_output_names: HashSet<&str> =
        chip_decl.outputs.iter().map(|n| n.name.as_ref()).collect();
    for stmt in &chip_decl.body.stmts {
        match stmt {
            // The source-ordered nested-declaration overlay of the other
            // pre-declare loops (see `pre_declare_chip_name`).
            Stmt::ChipDecl(c) => pre_declare_chip_name(&mut ctx, c),
            Stmt::In(i) => pre_declare_input(&mut ctx, i),
            Stmt::Var(v) => ctx.with_nofold(v.no_fold, |c| pre_declare_var(c, v)),
            Stmt::Buffer(b) => pre_declare_buffer(&mut ctx, b),
            Stmt::Array(a) => pre_declare_array(&mut ctx, a),
            Stmt::Map(m) => pre_declare_map(&mut ctx, m),
            Stmt::OutBinding(o) if !sig_output_names.contains(&o.name.as_ref()) => {
                pre_declare_output(
                    &mut ctx,
                    &o.name,
                    o.value.as_ref(),
                    o.typ.as_ref(),
                    o.side,
                    o.label.as_deref(),
                    o.label_expr.as_ref(),
                    o.invisible,
                    false,
                    &o.range,
                );
            }
            _ => {}
        }
    }
    for stmt in &chip_decl.body.stmts {
        lower_stmt(&mut ctx, stmt);
    }

    ctx.builder.module
}

#[cfg(test)]
mod tests;
