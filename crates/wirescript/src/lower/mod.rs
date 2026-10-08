//! Lowering: AST + typecheck annotations → IR Module.
//!
//! Strategy: walk each top-level declaration. Vars and I/O become IR
//! nodes with deterministic names. Handlers become one event node feeding
//! an exec chain; statements thread a `current_exec` PortRef through each
//! step. Expressions produce gate nodes whose value output is threaded
//! into their consumer.

use crate::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{ChipDecl, *};
use crate::catalog::calls::find_call;
use crate::catalog::events::find_event;
use crate::catalog::operators::OpRule;
use crate::diagnostic::{Diagnostic, SourceRange};
use crate::intern::{intern, intern_static, sym};
use crate::ir::build::{AddNodeOpts, ModuleBuilder, port_ref};
use crate::ir::gate_class as gc;
use crate::ir::{
    GateIO, Literal, Module, NodeId, NodeKind, PortRef, PortSpec, ROOT_SCOPE_ID, ScopeId,
    ScopeInfo, ScopeKind, Type, port_registry::WirePort,
};
use crate::template_cache::TemplateCache;
use crate::typecheck::{CeSlotMap, TypeCheckResult};

mod context;
use context::*;

mod predeclare;
use predeclare::*;
pub use predeclare::{
    ConstEnv, build_const_declared_names, build_const_env, build_const_env_in, build_const_uncertified, expr_to_literal,
};
pub(crate) use predeclare::{
    DupConsts, build_const_env_at, certified_binop, default_literal_for_var_type, dup_key, eval_const_binop,
    eval_const_unop, expr_to_literal_in, fold_ammo_override, fold_constructor, fold_mesh_colors,
};

mod decl;
use decl::*;

mod handler;
use handler::*;

mod stmt;
use stmt::*;

mod expr;
use expr::*;
// `const_eval::expr`'s own `MatchExpr` arm reuses the same capture-path
// collection as `lower_match_arm`/`lower_match_arm_stmt` (see that
// function's doc comment).
pub(crate) use expr::{collect_pattern_captures, wire_type_of_literal};

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
pub(crate) mod gate_variants;
mod convert;
mod props;

// `for`-loop lowering, and the range normalisation it rests on: every range
// spelling collapses onto the callable-loop shell's three driver pins.
pub(crate) mod for_loop;
pub(crate) mod function;

// Maranget decision-tree builder for `match` lowering. Self-contained; the
// match-expr / match-stmt lowering (later tasks) walks the `Decision` it emits.
pub(crate) mod matchtree;

// Certified constant-fold pass. Exposed as `#[doc(hidden)] pub` rather than
// `pub(crate)` so the `--fold-diff` differential fuzz harness
// (examples/fuzz_programs.rs — an `examples/` binary, which compiles as an
// external crate and cannot see `pub(crate)` items) can reach
// `fold::eval::{eval, Value, format_text}` to predict certified-fold
// outcomes (incl. `${...}`-interpolation) without reimplementing the
// evaluator itself. This is the ONLY surface widened: `fold::table` and the
// propagation/apply internals stay `pub(crate)`.
#[doc(hidden)]
pub mod fold;

// ---------- result ----------

pub struct LowerResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped during lowering — see `LowerCtx::dropped_ranges`. Consumed
    /// only by tests that assert this side agrees with
    /// `typecheck::TypeCheckResult::dropped_ranges` on exactly which ranges
    /// are skipped.
    pub dropped_ranges: Vec<SourceRange>,
}

pub struct LowerInput<'a> {
    pub ast: &'a Script,
    pub type_of_expr: &'a HashMap<(Arc<str>, usize, usize), Type>,
    pub op_resolutions: &'a HashMap<(Arc<str>, usize, usize), OpRule>,
    pub resolutions: &'a crate::typecheck::Resolutions,
    pub file: &'a str,
    pub module_name: Option<&'a str>,
    pub template_cache: Arc<TemplateCache>,
    pub doc_comments: &'a crate::parser::DocComments,
    /// Whether the certified constant-fold pass runs — see [`FoldMode`]. A
    /// module-level `@nofold` (`input.ast.no_fold`) always disables it on
    /// top of this, regardless of mode.
    pub fold_mode: FoldMode,
    /// Inferred custom-event data-slot types (`CeSlotMap`), keyed
    /// by the receiver handler's `ce_slot_key`. Consulted by `lower_handler`
    /// so an unannotated `on CustomEvent(...)` param's `DataOut` port takes
    /// the inferred type instead of defaulting to `Type::Float`.
    pub ce_slots: &'a CeSlotMap,
}

/// Whether the certified constant-fold pass runs. `Auto` (production
/// default): fold unless the entry file opts out with a module-level
/// `@nofold`. Folding is on by default, so `Auto` and `ForceOn` are
/// behaviourally identical; the legacy module-level `@fold` / CLI `--fold`
/// opt-in is still accepted but redundant. `ForceOff` (CLI `--no-fold`) is
/// the only way to disable folding across a whole compile.
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
fn trigger_multiplicity(t: &crate::ast::Trigger) -> usize {
    match t {
        crate::ast::Trigger::Union { parts, .. } => {
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

/// Accumulate the `emit <output> = …` and handler `out <output> = …` sites in
/// `block` into `counts`: per output, how many sites write a value and whether
/// any of them runs on only some paths (a branch, a `match` arm, a loop body,
/// or after a statement that can end the block early). Anonymous chips share
/// their parent's outputs, nested handlers count once per trigger, and a named
/// chip owns its own. An output written from more than one site, or from one
/// conditional site, needs a backing store: the single-site fast path wires the
/// value straight to the rerouter, unconditionally, leaving the taken exec dead.
pub(super) fn count_emits_in_block(
    block: &Block,
    in_branch: bool,
    counts: &mut HashMap<String, (usize, bool)>,
) {
    count_emits_in(walk::Node::Block(block), in_branch, counts);
}

fn count_emits_in(node: walk::Node<'_>, in_branch: bool, counts: &mut HashMap<String, (usize, bool)>) {
    let start = walk::Ctx { branch: in_branch, ..walk::Ctx::default() };
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
    walk::walk(walk::Node::Block(block), walk::Ctx::default(), &mut |n, _| match n {
        walk::Node::Stmt(Stmt::Emit(e)) if e.value.is_some() => {
            names.insert(e.name.clone());
            true
        }
        walk::Node::Stmt(Stmt::ChipDecl(_)) => false,
        _ => true,
    });
}

/// A declaration scope of the program: its own top level (`None`), or an
/// `import * as` namespace, keyed by the namespace declaration's location.
pub(in crate::lower) type DeclScope = Option<(Arc<str>, usize)>;

pub(in crate::lower) fn namespace_scope(ns: &NamespaceDecl) -> DeclScope {
    Some((ns.range.file.clone(), ns.range.start.offset))
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
                TopDecl::Event(EventDecl { captured_body: Some(b), .. }) => {
                    count_emits_in_block(b, false, counts)
                }
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
    imports: &'a crate::ast::ImportScope,
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
                TopDecl::Fn(f) => Body {
                    name: f.name.as_str(),
                    range: &f.range,
                    node: walk::Node::Expr(&f.body),
                    scope: si,
                    inline: true,
                    self_receiver: f.params.first().is_some_and(|p| p.name == "self" && p.pattern.is_none()),
                    outputs: Vec::new(),
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
        self.body_by_name.get(&(si, name)).or_else(|| self.body_by_name.get(&(0, name))).copied()
    }

    /// The bodies a call in scope `si` may instantiate: a plain call names its
    /// callee, `ns.f()` a member of a namespace, and a receiver call `x.f()` a
    /// body whose first parameter is `self`.
    fn callees(&self, si: usize, call: &Expr) -> Vec<usize> {
        let Expr::Call { callee, .. } = call else { return Vec::new() };
        match callee.as_ref() {
            Expr::Ident { name, range } => self
                .body(si, name)
                .filter(|&b| self.imports.user_mod_answers(self.bodies[b].range, &range.file))
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
                    && self.imports.user_mod_answers(self.bodies[b].range, &range.file)
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
    /// call inside another body once per instantiation of that body. A call
    /// under a branch, `match` arm, `if` expression or loop, after a possible
    /// early `return`, or inside a body that is itself instantiated so, is
    /// conditional.
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
                    TopDecl::Chip(ChipDecl { name, .. }) | TopDecl::Fn(FnDecl { name, .. }) => {
                        let b = self.body_by_name[&(si, name.as_str())];
                        (self.bodies[b].node, Some(b))
                    }
                    _ => (walk::Node::Decl(d), None),
                };
                call_sites(node, walk::Ctx::default(), 1, &mut |call, branched, reps| {
                    for callee in self.callees(si, call) {
                        let entry = match caller {
                            Some(c) => inside[c].entry(callee).or_insert((0, false)),
                            None => &mut top[callee],
                        };
                        entry.0 += reps;
                        entry.1 |= branched;
                    }
                });
            }
        }
        fn instances(
            b: usize,
            top: &[(usize, bool)],
            inside: &[HashMap<usize, (usize, bool)>],
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
            for (caller, calls) in inside.iter().enumerate() {
                if let Some(&(k, site_branched)) = calls.get(&b) {
                    let (m, caller_branched) = instances(caller, top, inside, memo, open);
                    n += k * m;
                    branched |= m > 0 && (site_branched || caller_branched);
                }
            }
            open.remove(&b);
            memo[b] = Some((n, branched));
            (n, branched)
        }
        let mut memo = vec![None; n];
        (0..n).map(|b| instances(b, &top, &inside, &mut memo, &mut HashSet::default())).collect()
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
        walk::Node::Stmt(Stmt::Handler(h)) | walk::Node::Decl(TopDecl::Handler(h)) => {
            reps * trigger_multiplicity(&h.trigger)
        }
        _ => reps,
    };
    walk::children(node, &mut |child, flow| call_sites(child, cx.enter(flow), reps, &mut *on));
}

/// Give output `name` a backing PseudoVar (idempotent). An output value-driven
/// from more than one `emit` site — or one conditional site, or a default plus
/// any emit — can't take a wire per site (two wires into the output rerouter is
/// a load-time fan-in): each site does a `Var_Set` into this var, and the var
/// feeds the output once (`lower/mod.rs`'s finalize step). `lookup_output` must
/// already resolve `name` (its rerouter pre-declared) or this is a no-op.
pub(in crate::lower) fn create_output_backing_var(ctx: &mut LowerCtx, name: &str) {
    if ctx.output_backing_vars.contains_key(name) {
        return;
    }
    let Some(out) = ctx.lookup_output(name).or_else(|| ctx.output_tag(name)).cloned() else {
        return;
    };
    let inner = out.ty.clone();
    let var_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR,
        source_range: SourceRange::default(),
        ports: GateIO::var_storage(inner.clone()),
        note: Some("out_backing"),
        ..Default::default()
    });
    ctx.output_backing_vars.insert(
        name.to_string(),
        VarRecord {
            node_id: var_id,
            inner_type: inner,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        },
    );
}

pub fn lower(input: LowerInput<'_>) -> LowerResult {
    // Root module name is on the top-level chip's emitted text label.
    // Explicit `module_name` wins; otherwise use the entry file's stem.
    let file_stem = std::path::Path::new(input.file)
        .file_stem()
        .and_then(|s| s.to_str());
    let builder = ModuleBuilder::new(input.module_name.or(file_stem).unwrap_or("main"));
    // Built once, up front, and reused for both `ctx.enum_defs` and
    // `build_const_env` below, the same registry a `const` enum-value
    // initializer's own const evaluation reads its discriminant from
    // (`const_eval::ConstCtx::enum_defs`), so lowering can never disagree
    // with itself about a tag. Wrapped in `Arc` here so every later
    // `.clone()` of `ctx.enum_defs` (per const-evaluation, per scope, per
    // mod/chip inlining) is a refcount bump rather than a deep copy.
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&input.ast.decls));

    let generic_type_aliases = collect_generic_type_aliases(&input.ast.decls);
    let top_names = Arc::new(crate::scope::top::TopNames::of(&input.ast.decls));
    let mut ctx = LowerCtx {
        file: input.file.to_string(),
        type_aliases: expand_alias_bodies({
            let mut m = HashMap::default();
            for d in &input.ast.decls {
                match d {
                    crate::ast::TopDecl::TypeAlias(ta) => {
                        m.insert(ta.name.clone(), ta.typ.clone());
                    }
                    // `import * as Ns` arrives as a Namespace decl holding the
                    // imported module's declarations. Its aliases are keyed
                    // `Ns.Name`, the spelling the parser produces for a
                    // qualified type and the keying
                    // `collect_generic_type_aliases` uses for the generic half
                    // of this map.
                    crate::ast::TopDecl::Namespace(ns) => {
                        for nd in &ns.decls {
                            if let crate::ast::TopDecl::TypeAlias(ta) = nd {
                                m.insert(format!("{}.{}", ns.name, ta.name), ta.typ.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
            m
        }, &generic_type_aliases),
        // The enum registry is built by the SHARED `enums::build_registry`
        // (which numbers via `enums::variant_defs`) so lowering reads the exact
        // discriminant typecheck assigned - one source of truth for the tag,
        // never a re-derivation.
        enum_defs: enum_defs.clone(),
        template_cache: input.template_cache.clone(),
        known_fn_names: Arc::new(collect_fn_names(input.ast)),
        imports: input.ast.imports.clone(),
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
        nofold_depth: input.ast.no_fold as u32,
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
        ctx.consts.set_cursor(Some(above.unwrap_or_else(|| crate::scope::top::ReadAt::code(d.range()))));
        pre_declare_decl(&mut ctx, d);
        ctx.keep_top_dup_bindings(d);
    }
    // Pass 1a: a top-level `on` handler may DECLARE a boundary output
    // (`on Clock { @top out flash: bool = Toggle() }`). Run after the loop
    // above, not inside it, so a handler-declared name binds to an explicit
    // top-level `out` of the same name wherever that declaration sits in the
    // file rather than minting a second port for it.
    for d in &input.ast.decls {
        match d {
            TopDecl::Handler(h) => pre_declare_handler_outputs(&mut ctx, &h.body, true),
            // `let foo = on Trigger { ... }` (a captured event) carries its
            // handler body here and is lowered handler-style by
            // `lower_event_decl`, so its `out` bindings are top-level
            // handler bindings too.
            TopDecl::Event(e) => {
                if let Some(body) = &e.captured_body {
                    pre_declare_handler_outputs(&mut ctx, body, true);
                }
            }
            _ => {}
        }
    }
    // Snapshot the names the importer itself owns (everything pass 1 just
    // registered) so a pass-2 `import * as` namespace member can't clobber one
    // of them — see `LowerCtx::importer_names`.
    ctx.importer_names = ctx.scope.iter().map(|(k, _)| k.to_string()).collect();
    // Pass 1b: an output value-driven from more than one `emit` site
    // can't take a wire per site (two wires into the output rerouter is a
    // load-time fan-in) — give it a backing PseudoVar. `lower_emit` does a
    // Var_Set into it at each site; the var feeds the output once at the end
    // (finalized just before `flush_pending_emits`). A SINGLE emit inside a
    // conditional also needs the backing var (so the write is gated by the
    // branch exec); only a single UNCONDITIONAL emit keeps the direct
    // value→output wire.
    // Outputs with a default initializer (`out r = 0`). The default is a
    // persistent driver on the output rerouter, so ANY `emit r = …` adds a
    // second — a fan-in. Such an output also needs the backing var: the default
    // seeds the var (see `lower_out_binding`) and each emit does a Var_Set.
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
        .map(|(name, (count, _))| (name.clone(), count + usize::from(defaulted_outputs.contains(name))))
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
                TopDecl::Out(o) if o.name == name => o.typ.as_ref().map(|t| (t.clone(), o.range.clone())),
                _ => None,
            })
            .and_then(|(t, range)| ctx.record_or_tuple_fields(&t).map(|f| (f, range)));
        match record {
            Some((fields, range)) => {
                let backing = crate::lower::predeclare::build_record_fields(
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
        // Flush handler end execs before non-handler declarations so that
        // code after `on` blocks chains from the combined handler exits.
        // Anon chips whose body is a single handler count as handlers for
        // this purpose — they're just visually grouped handlers.
        if !ctx.handler_end_execs.is_empty() && !is_handler_like(d) {
            flush_handler_end_execs(&mut ctx);
        }
        // The flush above sets `current_exec` for imperative top-level statements
        // (assign / if / expr) that genuinely continue from a handler. A pure,
        // reactive declaration (let / out / var / buffer) must NOT inherit it —
        // its reads are pure signal flow, not steps on the handler's spine — so
        // lower it with a cleared exec, then restore for any following statement.
        ctx.top_view(crate::scope::top::ReadAt::code(d.range()));
        if is_pure_top_decl(d) {
            let saved_exec = ctx.current_exec;
            ctx.current_exec = None;
            lower_decl(&mut ctx, d);
            ctx.current_exec = saved_exec;
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

    // Feed each multi-emit output's backing PseudoVar value into its
    // output rerouter — one wire, placed after every handler's Var_Set exists.
    let backings: Vec<(String, VarRecord)> = ctx
        .output_backing_vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, backing) in backings {
        if let Some(out) = ctx.lookup_output(&name).or_else(|| ctx.output_tag(&name)).cloned() {
            ctx.connect(
                backing.node_id.port(WirePort::Value),
                out.node_id.port(WirePort::RerInput),
            );
        }
    }
    let mut records: Vec<(String, HashMap<crate::intern::Sym, Binding>)> =
        ctx.output_record_backing.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, backing) in records {
        if let Some(Binding::Record(pins)) = ctx.scope.get(&context::output_scope_key(&name)).cloned() {
            call::wire_record_backing(&mut ctx, &backing, &pins);
        }
    }

    flush_pending_emits(&mut ctx);

    let mut module = ctx.builder.module;
    let whole_file = SourceRange::new(input.file, Default::default(), Default::default());
    prune_dead_exec_unions(&mut module);
    convert::prune_unread_conversions(&mut module);
    // Before literal-inlining/materialization: a real computation still has its
    // operand WIRES here, so only genuinely wireless orphans (e.g. a top-level
    // constant duplicated by a namespace + named import of the same module) are
    // pruned — not a user's connected-but-unused `let y = x * 2`.
    prune_dead_pure_gates(&mut module, false);
    prune_unread_gate_read_choices(&mut module);
    // Certified constant folding — table-gated. `FoldMode::Auto` (the
    // production default) folds unless the entry file opts out with a
    // module-level `@nofold`; `ForceOff` (CLI `--no-fold`) disables it
    // outright. `ForceOn` (CLI `--fold`) is equivalent to `Auto`, and the
    // legacy `@fold` opt-in (`input.ast.fold`) does not gate anything. A
    // module-level `@nofold` always wins over any mode short of `ForceOff`.
    let run_fold = match input.fold_mode {
        FoldMode::ForceOff => false,
        FoldMode::ForceOn | FoldMode::Auto => !input.ast.no_fold,
    };
    // Inline a bare source literal into a gate's scalar data field when its type
    // matches (a constant `on Clock(interval = 0.25)`, a `Sweep` distance, the
    // `0` in `x | 0`, …) BEFORE folding: at this point only source literals
    // exist, never fold-produced ones, so the pass does the same thing whether or
    // not folding runs — keeping fold a structural no-op. A folded constant
    // EXPRESSION is untouched here and still materializes a carrier below
    // (matching the unfolded compute gate), so the fold invariant holds.
    inline_bare_scalar_literals(&mut module);
    check_wires_resolve(&module, "inline_bare_scalar_literals", false, &whole_file, &mut ctx.diagnostics);
    if run_fold {
        fold::fold_certified_constants(&mut module);
        localize_foreign_literals(&mut module);
        check_wires_resolve(&module, "fold_certified_constants", false, &whole_file, &mut ctx.diagnostics);
    }
    materialize_unfoldable_constants(&mut module);
    check_wires_resolve(&module, "materialize_unfoldable_constants", false, &whole_file, &mut ctx.diagnostics);
    inline_orphan_literals(&mut module);
    check_wires_resolve(&module, "inline_orphan_literals", false, &whole_file, &mut ctx.diagnostics);
    // Folding an operand into its consumer can leave a bare `_Literal` wired to
    // nothing; sweep those literal orphans too (always safe — a literal has no
    // inputs, so it can never be a user computation whose operands were inlined).
    prune_dead_pure_gates(&mut module, true);
    check_wires_resolve(&module, "prune_dead_pure_gates", false, &whole_file, &mut ctx.diagnostics);
    // Merge the constant gates that survived inlining (a `"PREFIX: " ..` concat
    // wrapper repeated per use, a multi-consumer literal) so an identical
    // constant is emitted once and fans out, per chip.
    dedup_constant_gates(&mut module);
    check_wires_resolve(&module, "dedup_constant_gates", false, &whole_file, &mut ctx.diagnostics);
    // Common-subexpression elimination: the same generalized merge, extended to
    // pure `Expr_*` gates WITH wired inputs — a repeated `x + 1`, a duplicated
    // inline-`mod` body — collapse to one gate that fans out. Runs after all the
    // literal-inlining/fold passes so properties are final, and before
    // partition so keepers stay in their consumers' chip.
    cse_pure_gates(&mut module);
    check_wires_resolve(&module, "cse_pure_gates", false, &whole_file, &mut ctx.diagnostics);
    // `@flat` inlines every chip body onto one grid. It stands in for the
    // anon-chip partition rather than following it — that pass moves nodes
    // INTO child modules, which is what flattening undoes (see
    // `flatten`'s module docs). Afterwards there is a single module, so
    // `synthesize_boundary_pins` finds no cross-module wire and adds no pins.
    if input.ast.flat {
        flatten::flatten_chips(&mut module, &mut ctx.diagnostics);
        check_wires_resolve(&module, "flatten_chips", false, &whole_file, &mut ctx.diagnostics);
    } else {
        crate::emit::partition_anon_chips(&mut module);
        check_wires_resolve(&module, "partition_anon_chips", false, &whole_file, &mut ctx.diagnostics);
    }
    function::capture_through_handles(&mut module, &mut ctx.diagnostics);
    boundary_pins::synthesize_boundary_pins(&mut module);
    check_wires_resolve(&module, "synthesize_boundary_pins", true, &whole_file, &mut ctx.diagnostics);
    // Last: the wire set is final, and the brick size follows the ports in use.
    gate_variants::select_gate_variants(&mut module);
    check_no_exec_into_value(&module, &whole_file, &mut ctx.diagnostics);
    crate::emit::check_native_settings(&module, &mut ctx.diagnostics);
    LowerResult {
        module,
        diagnostics: ctx.diagnostics,
        dropped_ranges: ctx.dropped_ranges,
    }
}

/// Internal consistency check run after every post-lowering pass: every wire
/// names two nodes that still exist somewhere in the module tree. A pass that
/// deletes a node must delete the node's wires in every module, not only its
/// own. A violation is a compiler bug, reported as `WS094` naming the pass
/// just run, so the compile fails where the wire was stranded instead of at
/// emit, or, for a literal, instead of shipping a gate that reads 0.
///
/// The final, `pinned` check runs in every build and catches anything the
/// earlier ones would; the per-pass ones run in debug builds, where they name
/// the pass that left the wire.
///
/// `pinned` checks the shape `synthesize_boundary_pins` leaves: each endpoint
/// of a data wire is in the wire's own module or is a boundary pin of one of
/// that module's chips. That includes a wire from another module's `_Literal`:
/// emit inlines a literal only into its own module's consumers and a boundary
/// pin never carries one, so such a wire reaches the save as nothing. `Layout`
/// edges carry no value and are only checked for existence.
///
/// `NodeId(0)` is the never-allocated id a void call result stands for
/// (`lower/call/wiring.rs`): a wire to it is expected only in a program that
/// already has an error, and emit rejects it. The check stops at the first
/// violation, and does nothing once one has been reported.
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
    fn index<'m>(m: &'m Module, owner: &mut HashMap<NodeId, &'m Module>) {
        for id in m.nodes.keys() {
            owner.insert(*id, m);
        }
        for c in m.chips.values() {
            index(c, owner);
        }
    }
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
                    index(root, &mut owner);
                    owner
                })
                .get(id)
                .copied()
        };
        let range_of = |id: &NodeId| owner_of(id).and_then(|o| o.nodes.get(id)).map(|n| n.source_range.clone());
        for w in &m.wires {
            let (src, dst) = (w.source.node_id, w.target.node_id);
            let wire = || {
                format!(
                    "wire {src}.{} -> {dst}.{} in module '{}'",
                    w.source.port.as_str(),
                    w.target.port.as_str(),
                    crate::intern::resolve(m.name)
                )
            };
            let carries_value = w.source.port != WirePort::Layout && w.target.port != WirePort::Layout;
            for (end, other) in [(src, dst), (dst, src)] {
                if m.nodes.contains_key(&end) || (zero_expected && end == NodeId(0)) {
                    continue;
                }
                let Some(end_owner) = owner_of(&end) else {
                    return Some((format!("{} references {end}, which no module holds", wire()), range_of(&other)));
                };
                let is_pin = || m.chips.values().any(|c| c.inputs.contains(&end) || c.outputs.contains(&end));
                let definition = || {
                    owner_of(&src)
                        .and_then(|o| o.nodes.get(&src))
                        .is_some_and(|n| function::is_definition_wire(n, w))
                };
                if !pinned || !carries_value || is_pin() || definition() {
                    continue;
                }
                let what = if end_owner.nodes.get(&end).is_some_and(|n| n.gate_class == gc::LITERAL) {
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
pub(crate) fn exec_may_drive(dst: &crate::ir::Node, port: WirePort, ty: &Type) -> bool {
    matches!(ty, Type::Exec | Type::Bool)
        || dst.gate_class.starts_with("BrickComponentType_Expr_Logical")
        || (matches!(ty, Type::Any | Type::Opaque)
            && [gc::SEND_CUSTOM_EVENT, gc::SEND_CUSTOM_EVENT_GLOBAL].contains(&dst.gate_class)
            && port.as_str().starts_with("DataIn"))
}

/// The wiring invariant every lowered program must hold: no exec (an
/// `ExecOut`, the rerouter of an `exec` input or pin, any port typed `exec`)
/// drives an input that takes a value other than a `bool` (see
/// [`exec_may_drive`]). An exec pin carries no value, so such a wire reads 0
/// in game while the source says otherwise. A rerouter pin of no type (`out r:
/// any`) passes an exec on, and the invariant follows it to the input it ends
/// at. Each violation is `WS094` naming both ends and the source line of the
/// gate that reads it: a compiler bug where the exec is wired directly, and a
/// program reading an exec as a value where it passes an untyped pin. Runs
/// once, after the last post-lowering pass, over every wire of the module
/// tree. A debug build (every test) also panics on a direct one, in
/// [`crate::diagnostic::drop_internal_errors_beside_errors`], when the program
/// reports no other error.
pub(crate) fn check_no_exec_into_value(root: &Module, fallback: &SourceRange, diagnostics: &mut Vec<Diagnostic>) {
    if diagnostics.iter().any(|d| d.code == crate::diagnostic::INTERNAL_ERROR) {
        return;
    }
    fn index<'m>(m: &'m Module, nodes: &mut HashMap<NodeId, &'m crate::ir::Node>, wires: &mut Vec<crate::ir::Wire>) {
        nodes.extend(m.nodes.iter().map(|(id, n)| (*id, n)));
        let laid_out = |w: &&crate::ir::Wire| w.source.port == WirePort::Layout || w.target.port == WirePort::Layout;
        wires.extend(m.wires.iter().filter(|w| !laid_out(w)).copied());
        for c in m.chips.values() {
            index(c, nodes, wires);
        }
    }
    let mut nodes = HashMap::default();
    let mut wires = Vec::new();
    index(root, &mut nodes, &mut wires);
    let ends = |w: &crate::ir::Wire| {
        let (src, dst) = (nodes.get(&w.source.node_id)?, nodes.get(&w.target.node_id)?);
        let from = src.ports.find_output(w.source.port.sym()).map(|p| &p.ty);
        Some((*src, from, *dst, dst.ports.find_input(w.target.port.sym()).map(|p| &p.ty)?))
    };
    // The rerouters an exec reaches through a pin of no type: their output
    // carries it on.
    let mut carriers: HashSet<NodeId> = HashSet::default();
    let carries = |carriers: &HashSet<NodeId>, w: &crate::ir::Wire, from: Option<&Type>| {
        from == Some(&Type::Exec) || (w.source.port == WirePort::RerOutput && carriers.contains(&w.source.node_id))
    };
    let untyped_pin = |w: &crate::ir::Wire, into: &Type| {
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
    let feeds: HashMap<(NodeId, WirePort), crate::ir::PortRef> =
        wires.iter().map(|w| ((w.target.node_id, w.target.port), w.source)).collect();
    let array_writes = [gc::ARRAY_PUSH, gc::ARRAY_SET_AT_INDEX, gc::ARRAY_INSERT, gc::ARRAY_FILL, gc::ARRAY_RESIZE];
    let stored_bool = |w: &crate::ir::Wire, dst: &crate::ir::Node| {
        if w.target.port != WirePort::Value || !array_writes.contains(&dst.gate_class) {
            return false;
        }
        let mut at = feeds.get(&(w.target.node_id, WirePort::ArrayVarRef));
        for _ in 0..nodes.len() {
            let Some(src) = at else { return false };
            let Some(node) = nodes.get(&src.node_id) else { return false };
            match node.ports.find_output(src.port.sym()).map(|p| &p.ty) {
                Some(Type::Ref(t)) if matches!(t.as_ref(), Type::Array(e) if **e == Type::Bool) => return true,
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
        let Some((src, from, dst, into)) = ends(w) else { continue };
        let into = if stored_bool(w, dst) { &Type::Bool } else { into };
        if !carries(&carriers, w, from) || untyped_pin(w, into) || exec_may_drive(dst, w.target.port, into) {
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
        found.push(Diagnostic::error(crate::diagnostic::INTERNAL_ERROR, message, range));
    }
    let at = |d: &Diagnostic| (d.range.file.clone(), d.range.start.line, d.range.start.col, d.message.clone());
    found.sort_by_key(at);
    diagnostics.extend(found);
}

/// Every name a top-level `let` of the (import-merged) program binds.
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

/// Collect every chip/mod name declared anywhere in the (import-merged) program
/// — top-level and nested. Lets lowering tell a use-before-declaration call
/// (a real chip/mod, just registered later → WS021) from a call to a name that
/// is not a function at all (e.g. an unimplemented builtin → placeholder).
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
    aliases: HashMap<String, crate::ast::TypeExpr>,
    generic: &HashMap<String, crate::types::resolve::GenericAlias>,
) -> HashMap<String, crate::ast::TypeExpr> {
    use crate::ast::TypeExpr;
    let mut out = aliases.clone();
    for (key, body) in out.iter_mut() {
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
                    .and_then(|n| crate::types::resolve::instantiate_generic_alias(&n, args, generic))
                    .or_else(|| crate::types::resolve::instantiate_generic_alias(name, args, generic)),
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

/// Collect every generic type alias (`type Pair<T> = { … }`, top-level and
/// namespaced) into the `name → (params, body)` map lowering instantiates from.
/// Mirrors typecheck's `collect_generic_aliases` pre-pass so both layers agree
/// on which aliases are generic (non-empty `type_params`).
fn collect_generic_type_aliases(
    decls: &[TopDecl],
) -> HashMap<String, crate::types::resolve::GenericAlias> {
    let mut m = HashMap::default();
    for d in decls {
        match d {
            TopDecl::TypeAlias(ta) if !ta.type_params.is_empty() => {
                m.insert(
                    ta.name.clone(),
                    crate::types::resolve::GenericAlias {
                        params: ta.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        body: ta.typ.clone(),
                    },
                );
            }
            TopDecl::Namespace(ns) => {
                for nd in &ns.decls {
                    if let TopDecl::TypeAlias(ta) = nd
                        && !ta.type_params.is_empty()
                    {
                        m.insert(
                            format!("{}.{}", ns.name, ta.name),
                            crate::types::resolve::GenericAlias {
                                params: ta.type_params.iter().map(|tp| tp.name.clone()).collect(),
                                body: ta.typ.clone(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    m
}

/// Constant `Vec/Rotation/Color` calls lower to `_Literal` nodes so consumers
/// inline them as component data. That only works for sinks that store the
/// value as a wire-variant data field; for every other consumer — entity
/// gates whose struct-typed inputs must be wired, `Split*` inputs, chip IO,
/// unmapped gates — this pass materializes the real `Make*` gate (component
/// values baked into its data struct) and re-points those wires at it, so a
/// folded constant is never silently dropped. Recurses into chip sub-modules.
/// Inline a bare source literal wired into a gate's scalar data field, when the
/// literal's type matches the field's native storage type
/// (`port_accepts_inline_scalar`), by writing it into the gate's data and
/// dropping the wire — so a constant settings value or a matching operand (the
/// `0` in `x | 0`) needs no carrier gate. Runs BEFORE the fold pass, so it only
/// ever sees source literals — never fold-produced ones — which makes it behave
/// identically with or without folding (the fold pass stays a structural no-op);
/// a folded constant EXPRESSION still materializes a carrier in
/// `materialize_unfoldable_constants` (matching the unfolded compute gate).
/// Type-mismatched constants and data-only ports are left to that pass too.
/// Recurses into chip sub-modules.
fn inline_bare_scalar_literals(root: &mut Module) {
    let external = ExternalWires::of(root);
    inline_bare_scalar_literals_in(root, &external);
}

/// Nodes wired by a wire stored outside their own module. A wire between two
/// modules is stored in either endpoint's module, so a node's own module can
/// hold only some of its wires: a top-level constant read by a named chip has
/// its root consumers in the root's list and the chip's reads in the chip's.
/// A per-module pass that judges a node by its own module's wires alone must
/// leave these nodes in place.
struct ExternalWires {
    /// Sources of a wire stored in another module.
    sources: HashSet<NodeId>,
    /// Targets of a wire stored in another module.
    targets: HashSet<NodeId>,
}

impl ExternalWires {
    fn of(root: &Module) -> Self {
        fn walk(module: &Module, out: &mut ExternalWires) {
            for w in &module.wires {
                if !module.nodes.contains_key(&w.source.node_id) {
                    out.sources.insert(w.source.node_id);
                }
                if !module.nodes.contains_key(&w.target.node_id) {
                    out.targets.insert(w.target.node_id);
                }
            }
            for child in module.chips.values() {
                walk(child, out);
            }
        }
        let mut out = ExternalWires {
            sources: HashSet::default(),
            targets: HashSet::default(),
        };
        walk(root, &mut out);
        out
    }

    fn touches(&self, id: &NodeId) -> bool {
        self.sources.contains(id) || self.targets.contains(id)
    }
}

/// Give every module its own copy of each `_Literal` it reads from another
/// module, and move the reading wire into that module. Emit inlines a literal
/// only into consumers in its own module and a boundary pin never carries one,
/// so a wire from another module's literal reaches the save as nothing: the
/// consumer reads its port default. Lowering never wires one
/// (`LowerCtx::local_literals`), but folding does: a top-level constant
/// expression (`let K = 3 + 4`) read by a named chip becomes a literal in
/// place, in the root. One copy per consuming module and anonymous chip; the
/// original keeps only its own module's reads, and is pruned once it has none.
fn localize_foreign_literals(root: &mut Module) {
    fn index(
        m: &Module,
        path: &mut Vec<NodeId>,
        paths: &mut Vec<Vec<NodeId>>,
        owner: &mut HashMap<NodeId, usize>,
        literals: &mut HashMap<NodeId, crate::ir::Node>,
    ) {
        let here = paths.len();
        paths.push(path.clone());
        for (id, n) in &m.nodes {
            owner.insert(*id, here);
            if n.gate_class == gc::LITERAL {
                literals.insert(*id, n.clone());
            }
        }
        let mut chips: Vec<NodeId> = m.chips.keys().copied().collect();
        chips.sort_unstable();
        for c in chips {
            path.push(c);
            index(&m.chips[&c], path, paths, owner, literals);
            path.pop();
        }
    }
    fn extract(
        m: &mut Module,
        owner: &HashMap<NodeId, usize>,
        literals: &HashMap<NodeId, crate::ir::Node>,
        moved: &mut Vec<(usize, crate::ir::Wire)>,
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
    fn module_at<'m>(root: &'m mut Module, path: &[NodeId]) -> &'m mut Module {
        path.iter().fold(root, |m, c| m.chips.get_mut(c).expect("indexed chip module"))
    }

    let mut paths = Vec::new();
    let mut owner = HashMap::default();
    let mut literals = HashMap::default();
    index(root, &mut Vec::new(), &mut paths, &mut owner, &mut literals);
    if literals.is_empty() {
        return;
    }
    let mut moved: Vec<(usize, crate::ir::Wire)> = Vec::new();
    extract(root, &owner, &literals, &mut moved);
    moved.sort_by_key(|(_, w)| (w.target.node_id, w.source.node_id));

    let mut copies: HashMap<(usize, Option<NodeId>, NodeId), NodeId> = HashMap::default();
    for (at, mut w) in moved {
        let m = module_at(root, &paths[at]);
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
    // A string literal has no `_Literal` form: `lower_literal` gives it a
    // `String_Concatenate` carrying the text in `InputA` with the other fields
    // empty. Recognizing that shape here lets a literal operand bake into a
    // `str` field the same way an int or bool one does. Only a carrier with no
    // wired inputs qualifies, which is what separates it from a real 2-input
    // concatenation.
    let wired_targets: crate::collections::HashSet<crate::ir::NodeId> =
        module.wires.iter().map(|w| w.target.node_id).collect();
    let is_empty_str = |n: &crate::ir::Node, k| match n.properties.get(&k) {
        None => true,
        Some(Literal::String(s)) => s.is_empty(),
        _ => false,
    };
    // Interned once rather than per wire: `intern` is a global map lookup, and
    // this loop runs over every wire in the module.
    let separator_sym = intern("Separator");
    let input_a_sym = *sym::INPUT_A;
    let input_b_sym = *sym::INPUT_B;
    let mut inlines: Vec<(usize, crate::ir::NodeId, WirePort, Literal)> = Vec::new();
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
    let mut inlined_sources: crate::collections::HashSet<crate::ir::NodeId> =
        crate::collections::HashSet::default();
    for (i, target_id, port, lit) in inlines {
        if let Some(target) = module.nodes.get_mut(&target_id) {
            // `port.sym()` is the per-variant cached symbol; interning
            // `port.as_str()` here was a global-map lookup per inlined literal.
            std::sync::Arc::make_mut(&mut target.properties).insert(port.sym(), lit);
            if let Some(w) = module.wires.get(i) {
                inlined_sources.insert(w.source.node_id);
            }
            drop_wires.push(i);
        }
    }
    // Drop the carried wires in one pass. `retain` visits each wire once (O(W));
    // removing them individually with `Vec::remove` shifted the tail per drop
    // (O(D*W)) and dominated literal-dense programs.
    let drop: crate::collections::HashSet<usize> = drop_wires.into_iter().collect();
    let mut wire_idx = 0usize;
    module.wires.retain(|_| {
        let keep = !drop.contains(&wire_idx);
        wire_idx += 1;
        keep
    });
    // A carrier whose every consumer took the value inline is now unreachable.
    // Drop it here rather than leaving it for a later prune: the fold pass also
    // removes these, so leaving them would make the module differ by fold mode.
    // Index the surviving sources once (O(W)) rather than rescanning the whole
    // wire list per candidate (O(S*W)); on a literal-dense program that scan was
    // the most expensive thing lowering did.
    if !inlined_sources.is_empty() {
        let live_sources: crate::collections::HashSet<crate::ir::NodeId> =
            module.wires.iter().map(|w| w.source.node_id).collect();
        inlined_sources.retain(|id| !live_sources.contains(id) && !external.sources.contains(id));
    }
    if !inlined_sources.is_empty() {
        module.nodes.retain(|id, _| !inlined_sources.contains(id));
    }
    for child in module.chips.values_mut() {
        inline_bare_scalar_literals_in(child, external);
    }
}

fn materialize_unfoldable_constants(module: &mut Module) {
    use crate::ir::{Node, NodeId, NodeKind, PortRef, PortSpec};
    let value_sym = *sym::VALUE;
    let mut make_nodes: Vec<Node> = Vec::new();
    let mut make_for: HashMap<NodeId, NodeId> = HashMap::default();
    let mut rewires: Vec<(usize, NodeId, WirePort)> = Vec::new();
    // (wire index, target node, target port, value) for constants bound to a
    // port that carries data but takes no wire — written as data, wire dropped.
    let mut inlines: Vec<(usize, NodeId, WirePort, Literal)> = Vec::new();

    for (i, w) in module.wires.iter().enumerate() {
        let Some(src) = module.nodes.get(&w.source.node_id) else {
            continue;
        };
        if src.gate_class != gc::LITERAL {
            continue;
        }
        let recipe = match src.properties.get(&value_sym) {
            Some(Literal::Vector { x, y, z }) => Some((
                gc::MAKE_VECTOR,
                Type::Vector,
                vec![
                    (WirePort::X, Literal::Float(*x)),
                    (WirePort::Y, Literal::Float(*y)),
                    (WirePort::Z, Literal::Float(*z)),
                ],
            )),
            Some(Literal::Rotator { pitch, yaw, roll }) => Some((
                gc::MAKE_ROTATION,
                Type::Rotator,
                vec![
                    (WirePort::Pitch, Literal::Float(*pitch)),
                    (WirePort::Yaw, Literal::Float(*yaw)),
                    (WirePort::Roll, Literal::Float(*roll)),
                ],
            )),
            Some(Literal::LinearColor { r, g, b, a }) => Some((
                gc::MAKE_COLOR,
                Type::Color,
                vec![
                    (WirePort::R, Literal::Float(*r)),
                    (WirePort::G, Literal::Float(*g)),
                    (WirePort::B, Literal::Float(*b)),
                    (WirePort::A, Literal::Float(*a)),
                ],
            )),
            _ => None,
        };
        let Some((gate_class, out_ty, fields)) = recipe else {
            // Scalar literal (int/float/bool/string) — or `Quat`, the one
            // composite variant the `recipe` match above has no arm for
            // (Vector/Rotator/LinearColor all short-circuit into `recipe`
            // and never reach this branch at all) — whose target port
            // cannot hold an inlined data default (e.g. Opaque's rerouter
            // RER_Input — no matching data field, so emit's inline is
            // silently dropped and the gate reads 0/empty in-game):
            // materialize a real pure gate whose UNWIRED inputs carry the
            // value as data defaults (the same in-game-proven mechanism as
            // `n + 1` inlining). A Variable gate is NOT usable here: its
            // Value output stays null until an exec write — InitialValue
            // data alone does not drive the port.
            let scalar = matches!(
                src.properties.get(&value_sym),
                Some(
                    Literal::Int(_)
                        | Literal::Float(_)
                        | Literal::Bool(_)
                        | Literal::String(_)
                        | Literal::Quat { .. }
                )
            );
            if !scalar {
                continue;
            }
            // The target lives either in this module or inside an
            // already-built chip child (a named chip instance's
            // `MicrochipInput` pin fed by a parent-side constant — a folded
            // `P(2 + 2)` argument, or the fold-independent `let k = 4` /
            // `P(k)` alias). Nothing delivers a literal across modules at
            // emit: each module's literal-inline scan walks only its OWN
            // wires, and `partition_anon_chips`' literal-clone arm only
            // covers nodes being moved into ANON chips — so a parent-literal
            // → child-pin wire survives to emit, where its literal source
            // makes it skipped, and the pin silently reads 0. Materialize
            // the carrier for cross-module targets too: a real parent-side
            // gate wired cross-grid into the pin, the same shape the
            // unfolded program emits for a computed argument.
            let (target, cross_module) = match module.nodes.get(&w.target.node_id) {
                Some(t) => (t, false),
                None => match find_node_in_chips(module, &w.target.node_id) {
                    Some(t) => (t, true),
                    // Dangling wire — nothing anywhere to deliver to.
                    None => continue,
                },
            };
            // Inlining into the target's data only works same-module (see
            // above), so a cross-module target needs the carrier even when
            // its port could hold an inline value.
            let target_ok = !cross_module
                && src.properties.get(&value_sym).is_some_and(|lit| {
                    crate::emit::port_accepts_inline_variant(target.gate_class, w.target.port, lit)
                });
            let target_placeholder =
                target.gate_class == gc::LITERAL || target.gate_class == gc::UNSUPPORTED;
            if target_ok || target_placeholder {
                continue;
            }
            // A cross-module data-only port can neither take a wire nor be
            // written from this module's pass — leave the wire alone (the
            // child's own recursive pass owns its same-module literals).
            if cross_module
                && !crate::catalog::is_wire_input(target.gate_class, w.target.port.as_str())
            {
                continue;
            }
            let lit = src.properties.get(&value_sym).unwrap().clone();
            // A settable-but-unwireable port (a data field with no input port,
            // e.g. DisplayText.FontSize) cannot take a wire at all: emitting one
            // fails the brdb port lookup. Materializing a gate here would just
            // produce a different unwireable wire, so write the value into the
            // target's data and drop the wire instead.
            if !crate::catalog::is_wire_input(target.gate_class, w.target.port.as_str()) {
                inlines.push((i, w.target.node_id, w.target.port, lit));
                continue;
            }
            let no_fold = src.properties.contains_key(&*sym::NO_FOLD);
            let (const_class, props, out_port_sym, out_port, ty): (
                &'static str,
                Vec<(crate::intern::Sym, Literal)>,
                crate::intern::Sym,
                WirePort,
                Type,
            ) = match &lit {
                Literal::Int(n) => (
                    "BrickComponentType_Expr_MathAdd",
                    vec![
                        (*sym::INPUT_A, Literal::Int(*n)),
                        (*sym::INPUT_B, Literal::Int(0)),
                    ],
                    *sym::OUTPUT,
                    WirePort::Output,
                    Type::Int,
                ),
                Literal::Float(f) => (
                    "BrickComponentType_Expr_MathAdd",
                    vec![
                        (*sym::INPUT_A, Literal::Float(*f)),
                        (*sym::INPUT_B, Literal::Float(0.0)),
                    ],
                    *sym::OUTPUT,
                    WirePort::Output,
                    Type::Float,
                ),
                Literal::Bool(b) => (
                    "BrickComponentType_Expr_LogicalOR",
                    vec![
                        (*sym::B_INPUT_A, Literal::Bool(*b)),
                        (*sym::B_INPUT_B, Literal::Bool(false)),
                    ],
                    *sym::B_OUTPUT,
                    WirePort::BOutput,
                    Type::Bool,
                ),
                Literal::String(str_v) => (
                    gc::STRING_CONCATENATE,
                    vec![
                        (*sym::INPUT_A, Literal::String(str_v.clone())),
                        (*sym::INPUT_B, Literal::String(String::new())),
                        (intern_static("Separator"), Literal::String(String::new())),
                    ],
                    *sym::OUTPUT,
                    WirePort::Output,
                    Type::String,
                ),
                // `Quat` has no compositeMath identity to piggyback on (the
                // certified `compositeMath` chapter's `MathAdd`/`VecScale`
                // identities are Vector-only — `fold/eval.rs::composite_math`
                // never matches a `Value::Quat`), so this reuses the SAME
                // proven mechanism the `recipe` match above already uses for
                // Vector/Rotator/Color: a real `MakeQuaternion` gate whose
                // X/Y/Z/W inputs are baked, UNWIRED data defaults (no wires
                // at all) — the general "unwired input reads its own data
                // default" law every carrier in this function relies on.
                Literal::Quat { x, y, z, w } => (
                    gc::MAKE_QUATERNION,
                    vec![
                        (intern_static("X"), Literal::Float(*x)),
                        (intern_static("Y"), Literal::Float(*y)),
                        (intern_static("Z"), Literal::Float(*z)),
                        (intern_static("W"), Literal::Float(*w)),
                    ],
                    *sym::OUTPUT,
                    WirePort::Output,
                    Type::Quat,
                ),
                _ => unreachable!("scalar match above"),
            };
            let const_id = *make_for.entry(w.source.node_id).or_insert_with(|| {
                let id = NodeId::fresh();
                let mut properties: HashMap<crate::intern::Sym, Literal> = HashMap::default();
                for (k, v) in props {
                    properties.insert(k, v);
                }
                if no_fold {
                    properties.insert(*sym::NO_FOLD, Literal::Bool(true));
                }
                make_nodes.push(Node {
                    id,
                    kind: NodeKind::Gate,
                    gate_class: const_class,
                    properties: std::sync::Arc::new(properties),
                    ports: std::sync::Arc::new(GateIO {
                        inputs: vec![],
                        outputs: vec![PortSpec::new(out_port_sym, ty.clone())],
                    }),
                    source_range: src.source_range.clone(),
                    chip_id: src.chip_id,
                    chain_id: src.chain_id,
                    scope_id: src.scope_id,
                    note: Some("materialized constant (dataless target)"),
                });
                id
            });
            rewires.push((i, const_id, out_port));
            continue;
        };
        let target_ok = match (
            src.properties.get(&value_sym),
            module.nodes.get(&w.target.node_id),
        ) {
            (Some(lit), Some(t)) => {
                crate::emit::port_accepts_inline_variant(t.gate_class, w.target.port, lit)
            }
            _ => false,
        };
        if target_ok {
            continue;
        }
        let make_id = *make_for.entry(w.source.node_id).or_insert_with(|| {
            let id = NodeId::fresh();
            let properties: HashMap<crate::intern::Sym, Literal> = fields
                .iter()
                .map(|(port, lit)| (port.sym(), lit.clone()))
                .collect();
            make_nodes.push(Node {
                id,
                kind: NodeKind::Gate,
                gate_class,
                properties: std::sync::Arc::new(properties),
                ports: std::sync::Arc::new(GateIO::source(out_ty.clone())),
                source_range: src.source_range.clone(),
                chip_id: src.chip_id,
                chain_id: src.chain_id,
                scope_id: src.scope_id,
                note: Some("materialized constant"),
            });
            id
        });
        rewires.push((i, make_id, WirePort::Output));
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
    // Drop the carried wires in one pass. `retain` visits each wire once (O(W));
    // removing them individually with `Vec::remove` shifted the tail per drop
    // (O(D*W)) and dominated literal-dense programs.
    let drop: crate::collections::HashSet<usize> = drop_wires.into_iter().collect();
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

/// Depth-first lookup of a node id across `module.chips` child modules
/// (named chip instances exist as populated child modules throughout
/// lowering, before `partition_anon_chips` runs). Node ids are globally
/// unique, so map iteration order cannot affect the result.
fn find_node_in_chips<'m>(
    module: &'m Module,
    id: &crate::ir::NodeId,
) -> Option<&'m crate::ir::Node> {
    for child in module.chips.values() {
        if let Some(n) = child.nodes.get(id) {
            return Some(n);
        }
        if let Some(n) = find_node_in_chips(child, id) {
            return Some(n);
        }
    }
    None
}

/// Fold standalone `_Literal` bricks whose value is only used once into a
/// property on the consumer gate, then delete the literal. Avoids having
/// rows of constant-value bricks cluttering the chip for things like
/// `n + 1`, `n > 10`, etc. Recurses into chip sub-modules.
fn inline_orphan_literals(root: &mut Module) {
    let external = ExternalWires::of(root);
    inline_orphan_literals_in(root, &external);
}

fn inline_orphan_literals_in(module: &mut Module, external: &ExternalWires) {
    let value_sym = *sym::VALUE;
    // Per-source (count, first target): the single-consumer check only ever
    // needs the one target wire, so no per-node target Vecs. Buffers are
    // reused across fixpoint iterations.
    let mut outgoing: HashMap<NodeId, (usize, PortRef)> =
        HashMap::with_capacity_and_hasher(module.nodes.len(), Default::default());
    let mut incoming_count: HashMap<NodeId, usize> =
        HashMap::with_capacity_and_hasher(module.nodes.len(), Default::default());
    let mut lit_ids: Vec<NodeId> = Vec::new();
    let mut concat_ids: Vec<NodeId> = Vec::new();
    let mut removed: HashSet<NodeId> = HashSet::default();
    // Built once, then maintained incrementally: a fold only ever removes the
    // folded node and its single outgoing wire, so surviving nodes' entries
    // never change except the target's incoming count.
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
                .filter(|(id, n)| n.gate_class == gc::LITERAL && !external.touches(id))
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
            // Cross-module wire (e.g. a chip's `MicrochipInput` fed by a
            // parent-side literal): `module.nodes` only holds THIS module's
            // own nodes, so a same-module presence check is required here —
            // mirrors the guard in `materialize_unfoldable_constants` (~line
            // 294). Without it, `module.nodes.get_mut(&target_id)` below
            // silently no-ops (target lives in a different module) while the
            // literal AND its cross-module wire still get deleted, leaving
            // the boundary port to read its zero/empty default at runtime
            // with no diagnostic. Leave the literal + wire alone instead.
            if !module.nodes.contains_key(&target_id) {
                continue;
            }
            // Rerouter (`Opaque`) has no data struct — folding the literal
            // into a node property would silently drop it at emit time
            // instead of blocking the fold as intended. Keep it wired.
            if module
                .nodes
                .get(&target_id)
                .is_some_and(|t| t.gate_class == gc::REROUTER)
            {
                continue;
            }
            let value = match module
                .nodes
                .get(&lit_id)
                .and_then(|n| n.properties.get(&value_sym).cloned())
            {
                Some(v) => v,
                None => continue,
            };
            // Convert PortIndex → Sym for use as a property key
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

        // Fold pure constant-string `String_Concatenate` wrappers into
        // consumers that accept an inline string variant. Unlike
        // `_Literal`, this is gated on `port_accepts_inline_variant` — a string
        // can't fill a wire-only port, so those keep the real concat gate.
        // A native `str` field is handled pre-fold by
        // `inline_bare_scalar_literals`, which keeps that case fold-invariant.
        concat_ids.clear();
        concat_ids.extend(
            module
                .nodes
                .iter()
                .filter(|(id, n)| {
                    n.gate_class == gc::STRING_CONCATENATE && !removed.contains(id) && !external.touches(id)
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
            // Only a single constant string (INPUT_A set, INPUT_B + Separator
            // empty) — real 2-input concats have wired inputs (incoming != 0).
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
                if !is_empty(*sym::INPUT_B) || !is_empty(intern("Separator")) {
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
    // Removed nodes had no incoming wires, so only source-side wires can
    // reference them — one sweep at the end covers every iteration's folds.
    if !removed.is_empty() {
        module
            .wires
            .retain(|w| !removed.contains(&w.source.node_id));
    }
    for child_module in module.chips.values_mut() {
        inline_orphan_literals_in(child_module, external);
    }
}

/// Merge structurally-identical constant gates within each chip so a repeated
/// constant is emitted once and fans out to its consumers, instead of once per
/// use (e.g. a `"PREFIX: " .. name` chip that repeats `"PREFIX: "` — a constant
/// `String_Concatenate` wrapper — on every line, or a multi-consumer `_Literal`
/// that inlining left behind).
///
/// A "constant" here is a pure gate (`_Literal` or `Expr_*`) with NO incoming
/// data wire: its output is then fully determined by its config properties, so
/// any two with the same gate class + properties are interchangeable. `Random`
/// is exec-classed (never `Expr_*`), and the stateful edge/change detectors
/// always have a wired input, so neither is ever a candidate.
///
/// Dedup is per-module: a constant lives in the chip that uses it, so merging
/// within each module avoids rewiring across microchip boundaries. Node ids are
/// globally unique, so redirecting a dropped dup's consumers is a tree-wide wire
/// rewrite. Recurses into chip sub-modules.
fn dedup_constant_gates(root: &mut Module) {
    // Canonical key for a constant gate: class + its properties (sorted, so the
    // key is order-independent). `Debug` on `Literal` is deterministic per value.
    fn const_key(n: &crate::ir::Node) -> String {
        let mut kv: Vec<(&str, String)> = n
            .properties
            .iter()
            .map(|(k, v)| (crate::intern::resolve(*k), format!("{v:?}")))
            .collect();
        kv.sort_unstable();
        format!("{}\u{1}{kv:?}", n.gate_class)
    }

    // dup node id -> keeper node id (same class ⇒ same output ports, so each
    // consumer keeps its own `source.port` and only the node id changes).
    let mut redirect: HashMap<NodeId, NodeId> = HashMap::default();

    fn collect(module: &mut Module, external: &ExternalWires, redirect: &mut HashMap<NodeId, NodeId>) {
        let has_incoming: HashSet<NodeId> = module
            .wires
            .iter()
            .filter(|w| w.target.port != WirePort::Layout)
            .map(|w| w.target.node_id)
            .collect();
        // Key by (owning anon-chip, const_key): this pass runs BEFORE
        // `partition_anon_chips` splits each `chip_id`-tagged group into its own
        // emit module, so merging across `chip_id` tags would redirect a
        // consumer's wire to a keeper that lands in a DIFFERENT chip. Partition
        // keeps such a cross-chip data wire in the parent without cloning the
        // literal into the child (only parent→chip literals are cloned), so
        // emit can't inline it and the operand silently reads its port default
        // (0). Deduping within a chip_id group keeps every keeper in the same
        // future module as its consumers.
        let mut groups: HashMap<(Option<NodeId>, String), Vec<NodeId>> = HashMap::default();
        for (id, n) in &module.nodes {
            let is_pure = n.gate_class == gc::LITERAL
                || n.gate_class
                    .starts_with("BrickComponentType_Expr_");
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
        for child in module.chips.values_mut() {
            collect(child, external, redirect);
        }
    }

    let external = ExternalWires::of(root);
    collect(root, &external, &mut redirect);
    if redirect.is_empty() {
        return;
    }
    apply_gate_redirect(root, &redirect);
}

/// Rewrite the module tree after a set of gates has been merged: every wire
/// sourced from a dropped dup now sources from its keeper, wires targeting a
/// dropped dup (its own now-dead incoming feeds, or a Layout edge) are removed,
/// collapsed source→target duplicates are deduped, and `scope_captures`
/// referencing a dup become its keeper. Node ids are globally unique, so one
/// `redirect` map applies across the whole tree. Shared by
/// `dedup_constant_gates` (zero-incoming constants) and `cse_pure_gates`
/// (expression gates with wired inputs) — for the latter a dropped dup's
/// incoming feeds are genuinely dead (its keeper carries the identical feeds).
fn apply_gate_redirect(module: &mut Module, redirect: &HashMap<NodeId, NodeId>) {
    module.wires.retain_mut(|w| {
        if let Some(&keeper) = redirect.get(&w.source.node_id) {
            w.source.node_id = keeper;
        }
        !redirect.contains_key(&w.target.node_id)
    });
    // Redirection can collapse two wires onto the same source→target pair.
    let mut seen: HashSet<(PortRef, PortRef)> = HashSet::default();
    module.wires.retain(|w| seen.insert((w.source, w.target)));
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
    for child in module.chips.values_mut() {
        apply_gate_redirect(child, redirect);
    }
}

/// Common-subexpression elimination for pure expression gates: within each chip,
/// merge `Expr_*` gates that compute the same value — same gate class, same
/// config properties, and the same set of incoming `(input port ← source
/// node/port)` wires — so a repeated pure subexpression (`x + 1` used in three
/// outputs) or a duplicated inline-`mod` body is emitted once and fans out to
/// its consumers instead of once per use.
///
/// Runs as a single dependency-ordered pass: a pure gate's key is a function of
/// its inputs' CANONICAL sources, so visiting inputs before consumers (a
/// topological order over the eligible-gate subgraph) lets one pass find every
/// merge the old whole-tree-rescan fixpoint found, in O(nodes + wires) instead of
/// O(nodes x expression-depth). Keyed on `chip_id` (like `dedup_constant_gates`)
/// so a keeper never lands in a different partition module than a consumer.
///
/// Excludes anything whose output isn't a pure function of its current inputs:
/// non-`Expr_*` classes (the rerouter, exec/IO gates), the stateful
/// change/edge detectors (output depends on eval history), and `_nofold` gates
/// (a deliberate optimization barrier). Merging only ever redirects wire
/// SOURCES onto a shared keeper — it never creates fan-in — so it's always
/// legal for a pure gate.
fn cse_pure_gates(root: &mut Module) {
    fn eligible(n: &crate::ir::Node) -> bool {
        n.kind == NodeKind::Gate
            && n.gate_class
                .starts_with("BrickComponentType_Expr_")
            && !n.gate_class.contains("ChangeDetector")
            && !n.gate_class.contains("EdgeDetector")
            && !n.properties.contains_key(&*sym::NO_FOLD)
    }

    // A pure gate's behaviour is fully determined by (class, properties, the set
    // of wires feeding its input ports, and the types of its outputs). The output
    // types matter where they are the setting: a Convert gate's target type is
    // its `Output` port's type, so `x as int` and `x as string` share class,
    // properties and input and must still stay two gates. Build a stable,
    // order-independent key: structured rather than formatted, so only property
    // VALUES (which may hold f64s with no Eq/Hash) and the output types go
    // through a canonicalizing Debug render.
    type CseKey = (
        &'static str,
        Vec<(&'static str, String)>,
        Vec<(WirePort, NodeId, WirePort)>,
        Vec<String>,
    );
    fn cse_key(n: &crate::ir::Node, incoming: Option<&Vec<(WirePort, NodeId, WirePort)>>) -> CseKey {
        let mut props: Vec<(&'static str, String)> = n
            .properties
            .iter()
            .map(|(k, v)| (crate::intern::resolve(*k), format!("{v:?}")))
            .collect();
        props.sort_unstable();
        let mut ins: Vec<(WirePort, NodeId, WirePort)> = incoming.cloned().unwrap_or_default();
        ins.sort_unstable_by_key(|(tp, sid, sp)| (*tp as u16, sid.0, *sp as u16));
        let outs = n.ports.outputs.iter().map(|p| format!("{}:{:?}", crate::intern::resolve(p.name), p.ty)).collect();
        (n.gate_class, props, ins, outs)
    }

    fn cse_module(module: &mut Module, external: &ExternalWires, redirect: &mut HashMap<NodeId, NodeId>) {
        // Per-node incoming data wires (skip Layout edges — they're not inputs).
        let mut incoming: HashMap<NodeId, Vec<(WirePort, NodeId, WirePort)>> = HashMap::default();
        for w in &module.wires {
            if w.target.port == WirePort::Layout {
                continue;
            }
            incoming
                .entry(w.target.node_id)
                .or_default()
                .push((w.target.port, w.source.node_id, w.source.port));
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

        // One pass in dependency order. A node's key is built from its incoming
        // sources canonicalized through the merges already committed; topo order
        // guarantees those are final, so a single visit suffices. Gates trapped
        // in a combinational cycle (later rejected by `analyze`) never reach
        // in-degree zero and are simply left unmerged.
        let mut seen: HashMap<(Option<NodeId>, CseKey), NodeId> = HashMap::default();
        let mut qi = 0;
        while qi < queue.len() {
            let id = queue[qi];
            qi += 1;
            if let Some(n) = module.nodes.get(&id) {
                let key_ins: Option<Vec<(WirePort, NodeId, WirePort)>> = incoming.get(&id).map(|ins| {
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
        apply_gate_redirect(root, &redirect);
    }
}

/// Remove pure, side-effect-free expression gates that are fully disconnected —
/// no data wire touches them on either side.
///
/// Such orphans arise when a module is imported via BOTH a namespace
/// (`import * as x`) and a named import: the named path materializes a top-level
/// `let` wired to its consumers, while the namespace path materializes the SAME
/// `let` again (every importable decl is carried in the namespace), and that copy
/// is referenced by nothing — so a constant ships as a gate wired to nothing.
/// `inline_orphan_literals` only folds literals with exactly one consumer, so a
/// zero-connection orphan slips through. Dropping these changes no behavior.
///
/// The criterion is deliberately "no data wires at all", NOT "output feeds
/// nothing": a user's connected-but-unused computation (`let y = x * 2` with `y`
/// unused) still has input wires, and this project's cleanups prune only
/// compiler-generated cruft, never user computations. A duplicated top-level
/// constant has neither inputs (it's a literal) nor outputs, so it is caught
/// while `let y = x * 2` is left intact.
///
/// Connectivity is computed across the WHOLE module tree at once: a gate in one
/// module can be wired to a node in another via a cross-scope wire, so the
/// per-module recursion the neighbouring cleanups use would wrongly drop a gate
/// wired only to a sibling/child module.
///
/// `literals_only` restricts pruning to `_Literal` gates — used for the pass run
/// AFTER literal-inlining, where an `Expr_*` gate can be wireless simply because
/// its operands were folded into properties (a real, if unused, computation),
/// whereas a wireless literal is always a dead constant.
fn prune_dead_pure_gates(module: &mut Module, literals_only: bool) {
    let is_pure = |gate_class: &str| -> bool {
        gate_class == gc::LITERAL
            || (!literals_only && gate_class.starts_with("BrickComponentType_Expr_"))
    };
    // Node ids touched by a real (non-Layout) data wire, as source OR target.
    fn collect_connected(module: &Module, connected: &mut HashSet<NodeId>) {
        for w in &module.wires {
            if w.source.port != WirePort::Layout {
                connected.insert(w.source.node_id);
            }
            if w.target.port != WirePort::Layout {
                connected.insert(w.target.node_id);
            }
        }
        for child in module.chips.values() {
            collect_connected(child, connected);
        }
    }
    fn collect_orphans(
        module: &Module,
        connected: &HashSet<NodeId>,
        is_pure: &impl Fn(&str) -> bool,
        dead: &mut HashSet<NodeId>,
    ) {
        for (id, n) in &module.nodes {
            if n.kind == NodeKind::Gate && is_pure(n.gate_class) && !connected.contains(id) {
                dead.insert(*id);
            }
        }
        for child in module.chips.values() {
            collect_orphans(child, connected, is_pure, dead);
        }
    }
    fn remove_dead(module: &mut Module, dead: &HashSet<NodeId>) {
        module.nodes.retain(|id, _| !dead.contains(id));
        // Only Layout wires can touch a dead node (it has no data wires); drop them too.
        module
            .wires
            .retain(|w| !dead.contains(&w.source.node_id) && !dead.contains(&w.target.node_id));
        module.scope_captures.retain(|id| !dead.contains(id));
        for child in module.chips.values_mut() {
            remove_dead(child, dead);
        }
    }
    let mut connected = HashSet::default();
    collect_connected(module, &mut connected);
    let mut dead = HashSet::default();
    collect_orphans(module, &connected, &is_pure, &mut dead);
    if !dead.is_empty() {
        remove_dead(module, &dead);
    }
}

/// Drop each `Select` a choice between two gate reads built for an output no
/// place reads (`f = t` after `let t = if c then fs.max() else fs.min()`
/// reads `Value`, never `IsEmpty`). Connectivity spans the whole module tree,
/// as in [`prune_dead_pure_gates`].
fn prune_unread_gate_read_choices(module: &mut Module) {
    fn read(module: &Module, out: &mut HashSet<NodeId>) {
        out.extend(module.wires.iter().filter(|w| w.source.port != WirePort::Layout).map(|w| w.source.node_id));
        for child in module.chips.values() {
            read(child, out);
        }
    }
    fn unread(module: &Module, read: &HashSet<NodeId>, out: &mut HashSet<NodeId>) {
        for (id, n) in &module.nodes {
            if n.note == Some(stmt::GATE_READ_CHOICE) && !read.contains(id) {
                out.insert(*id);
            }
        }
        for child in module.chips.values() {
            unread(child, read, out);
        }
    }
    fn remove(module: &mut Module, dead: &HashSet<NodeId>) {
        module.nodes.retain(|id, _| !dead.contains(id));
        module.wires.retain(|w| !dead.contains(&w.source.node_id) && !dead.contains(&w.target.node_id));
        module.scope_captures.retain(|id| !dead.contains(id));
        for child in module.chips.values_mut() {
            remove(child, dead);
        }
    }
    let mut was_read = HashSet::default();
    read(module, &mut was_read);
    let mut dead = HashSet::default();
    unread(module, &was_read, &mut dead);
    if !dead.is_empty() {
        remove(module, &dead);
    }
}

/// Clean up degenerate `Exec_Union` nodes, repeating to a fixpoint (each
/// removal can degrade another union). Recurses into chip sub-modules.
///
/// - **No outgoing wires** (sink): remove the union and its incoming wires.
/// - **No incoming wires** (dead source, e.g. an if-join whose branches both
///   terminated via `return`/final `emit`): remove it and its outgoing wires —
///   whatever it fed keeps its other sources only.
/// - **Exactly one incoming wire** (pass-through): splice it out, rewiring its
///   consumers straight to the single source.
///
/// Degrees are counted from ONE module's wire list, but a wire may legitimately
/// name a node in another module of the tree before `boundary_pins` routes it
/// through pins — a top-level `let sig: exec` consumed inside a `chip` is the
/// common case. Such a union looks like a dead sink here (its only consumer is
/// invisible), and both removing and splicing it would leave that other
/// module's wire naming a node this deletes, which emit rejects as
/// `EmitError::DroppedWire`. So a union referenced from outside its own module
/// is left alone; at worst a degenerate Union gate survives, which is what the
/// unfolded lowering would have emitted anyway.
pub(super) fn prune_dead_exec_unions(module: &mut Module) {
    let external = ExternalWires::of(module);
    prune_dead_exec_unions_in(module, &external);
}

fn prune_dead_exec_unions_in(module: &mut Module, external: &ExternalWires) {
    /// Chase a source through spliced unions to the node actually carrying
    /// its wires now. Returns `None` for a pure splice cycle (unions feeding
    /// only each other — dead code whose wires all drop).
    fn resolve_src(
        spliced: &HashMap<NodeId, crate::ir::PortRef>,
        start: &crate::ir::PortRef,
    ) -> Option<crate::ir::PortRef> {
        let mut cur = start.clone();
        let mut hops = 0usize;
        while let Some(next) = spliced.get(&cur.node_id) {
            cur = next.clone();
            hops += 1;
            if hops > spliced.len() {
                return None;
            }
        }
        Some(cur)
    }

    // Degrees + adjacency computed once and maintained incrementally via a
    // worklist, avoiding an O(unions × wires) rebuild over every wire on
    // union-heavy modules.
    let queue: Vec<NodeId> = module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::UNION)
        .map(|(id, _)| *id)
        // Referenced from another module — its real degree is not visible in
        // this wire list, so neither removal nor splicing is sound here.
        .filter(|id| !external.touches(id))
        .collect();
    if queue.is_empty() {
        for child_module in module.chips.values_mut() {
            prune_dead_exec_unions_in(child_module, external);
        }
        return;
    }
    let is_union: HashSet<NodeId> = queue.iter().copied().collect();
    let mut queue = queue;

    // Counts and adjacency are only ever READ for union nodes (the queue is
    // union-only and every requeue is guarded by `is_union`), so only track
    // union entries — decrements on other nodes fall through `get_mut`.
    let mut in_count: HashMap<NodeId, usize> = HashMap::default();
    let mut out_count: HashMap<NodeId, usize> = HashMap::default();
    let mut in_edges: HashMap<NodeId, Vec<crate::ir::PortRef>> = HashMap::default();
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
            in_edges
                .entry(w.target.node_id)
                .or_default()
                .push(w.source.clone());
        }
    }

    let mut removed: HashSet<NodeId> = HashSet::default();
    let mut spliced: HashMap<NodeId, crate::ir::PortRef> = HashMap::default();

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
                let Some(src) = resolve_src(&spliced, s) else {
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
                .filter_map(|s| resolve_src(&spliced, s))
                .find(|s| !removed.contains(&s.node_id))
                .expect("counted one incoming wire");
            spliced.insert(id, raw.clone());
            let transferred = out_edges.remove(&id).unwrap_or_default();
            // Only LIVE transferred edges count toward the carrier's degree —
            // edges to removed/spliced targets were already discounted.
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
                let Some(src) = resolve_src(&spliced, &w.source) else {
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

    for child_module in module.chips.values_mut() {
        prune_dead_exec_unions_in(child_module, external);
    }
}

/// Compile a standalone chip declaration into an isolated [`Module`] suitable
/// for wrapping in a [`CompiledTemplate`].  This replicates the child-context
/// creation logic from `lower_chip_call_instance` without any parent-side
/// wiring.
pub fn compile_chip_template(
    chip_decl: &ChipDecl,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
) -> Module {
    use crate::ast::*;
    use crate::ir::build::ModuleBuilder;

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

    // Resource-estimation templates don't need real doc text (DOC_TEXT is
    // display-only) — an empty map keeps the LowerCtx lifetime local. Same
    // for `ce_slots`: this builds an isolated per-chip template purely for
    // gate-count estimation, not the final emitted graph, so an unresolved
    // custom-event slot here just estimates as `float`.
    let empty_docs = crate::parser::DocComments::default();
    let empty_ce_slots = CeSlotMap::default();

    let mut ctx = LowerCtx {
        file: file.to_string(),
        template_cache: cache.clone(),
        chip_call_stack: if chip_decl.name.is_empty() {
            Vec::new()
        } else {
            vec![(chip_decl.range.clone(), true)]
        },
        template_build: true,
        imports: tc.imports.clone(),
        // Standalone template path (resource estimation / cache) resolves
        // record aliases from inline literals only, and has no whole-program
        // `decls` slice to re-run `typecheck::enums::build_registry` against,
        // so an enum-typed var here falls back to a bare scalar (see
        // `declare_enum_container`'s unknown-enum fallback). Both maps stay at
        // `empty`'s defaults, which is that graceful degradation.
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
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: VarStorage::Array,
                    })
                } else if is_ref {
                    let inner = match &ft {
                        Type::Ref(inner) => inner.as_ref().clone(),
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: VarStorage::Var,
                    })
                } else {
                    Binding::Input(NodeRecord {
                        node_id,
                        ty: ft.clone(),
                    })
                };
                record_fields.insert(crate::intern::intern(&field.name), binding);
            }
            ctx.scope.insert(&inp.name, Binding::Record(record_fields));
        } else if ctx.ref_or_array(Some(chip_decl), &inp.typ) {
            let t = ctx.resolve_local_type(&inp.typ);
            let is_array = matches!(ctx.declared_shape(Some(chip_decl), &inp.typ), TypeExpr::Array { .. });
            let inner = match &t {
                Type::Ref(inner) => inner.as_ref().clone(),
                Type::Array(inner) => inner.as_ref().clone(),
                _ => t.clone(),
            };
            let node_id = ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
            ctx.scope.insert(
                &inp.name,
                Binding::Var(VarRecord {
                    node_id,
                    inner_type: inner,
                    get_node_for_handler: None,
                    storage: if is_array {
                        VarStorage::Array
                    } else {
                        VarStorage::Var
                    },
                }),
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
            &crate::lower::context::output_scope_key(&out.name),
            Binding::Output(NodeRecord { node_id, ty: t }),
        );
    }

    let sig_output_names: HashSet<&str> =
        chip_decl.outputs.iter().map(|n| n.name.as_ref()).collect();
    for stmt in &chip_decl.body.stmts {
        match stmt {
            // Same source-ordered nested-declaration overlay as the other
            // pre-declare loops (see `pre_declare_chip_name`). This template
            // path starts with an EMPTY `pass1_chips` (no inherited outer
            // declarations to shadow), so here it only adds resolution rather
            // than correcting any.
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
