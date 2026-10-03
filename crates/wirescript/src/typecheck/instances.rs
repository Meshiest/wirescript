//! The instantiations of generic mods and chips that calls demand.
//!
//! A generic body is checked once per combination of its type parameters'
//! bound members, up to a cap (see the `TopDecl::Chip` arm of
//! `check_decl_inner`); past the cap only a sample of combinations is
//! checked, which can leave out the one a call uses. So every call of a
//! generic records the type arguments it instantiates its callee with
//! (`Resolutions::call_type_args`), and an instantiation no check has covered
//! yet is queued here. Once every declaration has been checked the queue is
//! drained: each instantiation's body is checked in the surroundings its
//! declaration was checked in, and the calls inside it queue theirs, until
//! nothing new is demanded. Lowering then finds typecheck's own entry for
//! every instantiation it builds (`Resolutions::checked_bodies`).

use super::*;
use std::collections::VecDeque;

/// A mod or chip declaration, by its range.
pub(crate) type DeclId = (Arc<str>, usize, usize);

/// The most instantiations calls may demand beyond the combinations each
/// generic's bounds already check.
pub(crate) const MAX_DEMANDED_INSTANTIATIONS: usize = 1024;

fn decl_id(r: &SourceRange) -> DeclId {
    (r.file.clone(), r.start.offset, r.end.offset)
}

/// Everything a generic declaration's body check reads from around it, as it
/// stood where the declaration was checked.
struct DeclEnv {
    view: ScopeView,
    consts: crate::const_scope::ConstFrames,
    /// `mod_decls` above its base frame.
    mod_decls: Vec<HashMap<String, Arc<ChipDecl>>>,
    out_ctx: Vec<Vec<EventDataField>>,
    exec_mode: ExecMode,
    loop_depth: u32,
    inline_body_depth: u32,
    nofold_depth: u32,
    active_combos: usize,
    open_bodies: Vec<super::shadow::DeclKey>,
}

#[derive(Default)]
pub(crate) struct Instances {
    decls: HashMap<DeclId, Arc<ChipDecl>>,
    /// How many leading type arguments of a body's key belong to the bodies
    /// around the declaration.
    prefix_len: HashMap<DeclId, usize>,
    /// Each generic declaration's surroundings, per key of the bodies around
    /// it.
    envs: HashMap<(DeclId, Arc<[Type]>), Arc<DeclEnv>>,
    queue: VecDeque<(DeclId, Arc<[Type]>, SourceRange)>,
    queued: crate::collections::HashSet<(DeclId, Arc<[Type]>)>,
    /// Instantiations checked because a call demanded them.
    demanded: Vec<ResolutionKey>,
}

impl Instances {
    #[cfg(test)]
    pub(super) fn demanded(&self) -> Vec<ResolutionKey> {
        self.demanded.clone()
    }
}

/// Record where generic `c` is being checked, so an instantiation a call
/// demands is checked in the same surroundings. Called once per key of the
/// bodies around `c`, before its own combinations are checked.
pub(super) fn capture_env(ctx: &mut TypeCheckCtx, c: &ChipDecl) {
    let id = decl_id(&c.range);
    let prefix = ctx.resolution_subst.clone();
    ctx.instances.decls.entry(id.clone()).or_insert_with(|| Arc::new(c.clone()));
    ctx.instances.prefix_len.insert(id.clone(), prefix.len());
    let env = DeclEnv {
        view: ctx.scope.capture_view(),
        consts: ctx.consts.snapshot(),
        mod_decls: ctx.mod_decls[1..].to_vec(),
        out_ctx: ctx.out_ctx.clone(),
        exec_mode: ctx.exec_mode(),
        loop_depth: ctx.loop_depth,
        inline_body_depth: ctx.inline_body_depth,
        nofold_depth: ctx.nofold_depth,
        active_combos: ctx.active_combos,
        open_bodies: ctx.open_bodies.clone(),
    };
    ctx.instances.envs.insert((id, prefix), Arc::new(env));
}

/// A call at `call` instantiates the generic declared at `decl` with
/// `type_args`: record them for lowering, and queue that body unless it has
/// been checked already.
pub(super) fn demand(ctx: &mut TypeCheckCtx, decl: &SourceRange, type_args: Vec<Type>, call: &SourceRange) {
    let call_key = ctx.resolution_key(call);
    ctx.resolutions.call_type_args.insert(call_key, type_args.clone());
    let id = decl_id(decl);
    // A declaration not checked yet is a top-level one, whose key has no
    // prefix: a declaration inside a body is checked before any call to it.
    let len = ctx.instances.prefix_len.get(&id).copied().unwrap_or(0).min(ctx.resolution_subst.len());
    let mut key = ctx.resolution_subst[..len].to_vec();
    key.extend(type_args);
    let key: Arc<[Type]> = key.into();
    let body = (id.0.clone(), id.1, id.2, key.clone());
    if !ctx.resolutions.checked_bodies.contains(&body) && ctx.instances.queued.insert((id.clone(), key.clone())) {
        ctx.instances.queue.push_back((id, key, call.clone()));
    }
}

/// Check every queued instantiation no check has covered, and the ones they
/// demand in turn, until the queue is empty. Past
/// [`MAX_DEMANDED_INSTANTIATIONS`] the rest are reported (WS096), not checked.
pub(super) fn check_demanded(ctx: &mut TypeCheckCtx) {
    let mut reported: Option<crate::collections::HashSet<(String, Arc<str>, usize)>> = None;
    while let Some((id, key, call)) = ctx.instances.queue.pop_front() {
        if ctx.resolutions.checked_bodies.contains(&(id.0.clone(), id.1, id.2, key.clone())) {
            continue;
        }
        let len = ctx.instances.prefix_len.get(&id).copied().unwrap_or(0).min(key.len());
        let (Some(decl), Some(env)) = (
            ctx.instances.decls.get(&id).cloned(),
            ctx.instances.envs.get(&(id.clone(), Arc::from(&key[..len]))).cloned(),
        ) else {
            continue;
        };
        if ctx.instances.demanded.len() == MAX_DEMANDED_INSTANTIATIONS {
            let message = over_budget_message(ctx, (id, key));
            ctx.emit("WS096", message, call);
            ctx.instances.queue.clear();
            return;
        }
        ctx.instances.demanded.push((id.0.clone(), id.1, id.2, key.clone()));
        let reported = reported.get_or_insert_with(|| {
            ctx.diagnostics.iter().map(|d| (d.code.clone(), d.range.file.clone(), d.range.start.offset)).collect()
        });
        check_instantiation(ctx, &decl, &env, Arc::from(&key[..len]), &key[len..], reported);
    }
}

/// The WS096 message once the budget is spent and `next` is the first
/// instantiation left out: how many instantiations the calls demand so far
/// (the checked ones, `next`, and the rest still queued), and the generics
/// that demand the most of them.
fn over_budget_message(ctx: &TypeCheckCtx, next: (DeclId, Arc<[Type]>)) -> String {
    let mut per_decl: HashMap<DeclId, usize> = HashMap::default();
    let mut seen: crate::collections::HashSet<(DeclId, Arc<[Type]>)> = crate::collections::HashSet::default();
    let checked = ctx.instances.demanded.iter().map(|(f, s, e, k)| ((f.clone(), *s, *e), k.clone()));
    let pending = std::iter::once(next).chain(ctx.instances.queue.iter().map(|(id, k, _)| (id.clone(), k.clone())));
    for (id, key) in checked.chain(pending) {
        if seen.insert((id.clone(), key)) {
            *per_decl.entry(id).or_default() += 1;
        }
    }
    let total = seen.len();
    let mut ranked: Vec<(DeclId, usize)> = per_decl.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top: Vec<String> = ranked
        .iter()
        .take(3)
        .map(|(id, n)| {
            let name = ctx.instances.decls.get(id).map(|d| d.name.as_str()).unwrap_or("?");
            format!("`{name}` {n}")
        })
        .collect();
    let more = if ranked.len() > top.len() { ", ..." } else { "" };
    format!(
        "calls of generic mods and chips demand at least {total} distinct type-argument \
         combinations beyond the ones their bounds already cover ({}{more}), over the limit of \
         {MAX_DEMANDED_INSTANTIATIONS}, so this one and the rest are not checked. Give the calls \
         fewer distinct types, or narrow the bounds (`T: int | float`) so they cover the calls.",
        top.join(", "),
    )
}

/// Check `decl`'s body with its type parameters bound to `combo`, under the
/// key `prefix` of the bodies around it, in the surroundings `env`. A
/// diagnostic already reported at the same place is not repeated.
fn check_instantiation(
    ctx: &mut TypeCheckCtx,
    decl: &ChipDecl,
    env: &DeclEnv,
    prefix: Arc<[Type]>,
    combo: &[Type],
    reported: &mut crate::collections::HashSet<(String, Arc<str>, usize)>,
) {
    let scope = ctx.scope.enter_view(&env.view);
    let mod_decls = ctx.mod_decls.split_off(1);
    ctx.mod_decls.extend(env.mod_decls.iter().cloned());
    let consts = ctx.consts.replace(env.consts.clone());
    let out_ctx = std::mem::replace(&mut ctx.out_ctx, env.out_ctx.clone());
    let open_bodies = std::mem::replace(&mut ctx.open_bodies, env.open_bodies.clone());
    let resolution_subst = std::mem::replace(&mut ctx.resolution_subst, prefix);
    let loop_depth = std::mem::replace(&mut ctx.loop_depth, env.loop_depth);
    let inline_body_depth = std::mem::replace(&mut ctx.inline_body_depth, env.inline_body_depth);
    let nofold_depth = std::mem::replace(&mut ctx.nofold_depth, env.nofold_depth);
    let active_combos = std::mem::replace(&mut ctx.active_combos, env.active_combos);
    let statement_match = std::mem::take(&mut ctx.statement_match);
    let record_lit_map_fields = std::mem::take(&mut ctx.record_lit_map_fields);
    let expected_ty = ctx.expected_ty.take();
    ctx.exec_stack.push(env.exec_mode);

    let before = ctx.diagnostics.len();
    super::shadow::open_body(ctx, decl);
    super::decl::check_chip_body(ctx, decl, combo);
    super::shadow::close_body(ctx, decl);
    let new = ctx.diagnostics.split_off(before);
    ctx.diagnostics
        .extend(new.into_iter().filter(|d| reported.insert((d.code.clone(), d.range.file.clone(), d.range.start.offset))));

    ctx.exec_stack.pop();
    ctx.expected_ty = expected_ty;
    ctx.record_lit_map_fields = record_lit_map_fields;
    ctx.statement_match = statement_match;
    ctx.active_combos = active_combos;
    ctx.nofold_depth = nofold_depth;
    ctx.inline_body_depth = inline_body_depth;
    ctx.loop_depth = loop_depth;
    ctx.resolution_subst = resolution_subst;
    ctx.open_bodies = open_bodies;
    ctx.out_ctx = out_ctx;
    ctx.consts.replace(consts);
    ctx.mod_decls.truncate(1);
    ctx.mod_decls.extend(mod_decls);
    ctx.scope.leave_view(scope);
}
