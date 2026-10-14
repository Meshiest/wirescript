//! WS091: a local that shadows the storage a `mod`/`chip` called beside it
//! uses. The body resolves its names where it is declared, so the call reads
//! and writes the outer declaration, never the local, which reads like the
//! call working on the local.

use super::*;

/// A declaration key: the source position of a `mod`/`chip`.
pub(crate) type DeclKey = (Arc<str>, usize);

/// A call of a user `mod`/`chip` by bare name, noted for [`report`]: each name
/// its body mentions that the call site binds, with that binding.
pub(crate) struct CallSite {
    callee: DeclKey,
    callee_name: String,
    names: Vec<(String, SourceRange)>,
}

/// A declaring file as a message names it: its file name.
fn display_file(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or(file)
}

fn within(outer: &SourceRange, inner: &SourceRange) -> bool {
    outer.file == inner.file && outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
}

/// Start checking `decl`'s body: log the storage and calls it resolves.
pub(super) fn open_body(ctx: &mut TypeCheckCtx, decl: &ChipDecl) {
    let key: DeclKey = (decl.range.file.clone(), decl.range.start.offset);
    ctx.body_ranges.insert(key.clone(), decl.range.clone());
    ctx.open_bodies.push(key);
    ctx.scope.open_storage_log();
}

/// Close the storage log `decl`'s body check opened: the storage it resolved
/// that is declared outside the body is the storage the body uses.
pub(super) fn close_body(ctx: &mut TypeCheckCtx, decl: &ChipDecl) {
    ctx.open_bodies.pop();
    let seen = ctx.scope.close_storage_log();
    let uses = ctx.storage_uses.entry((decl.range.file.clone(), decl.range.start.offset)).or_default();
    for (name, at) in seen {
        if !within(&decl.range, &at) {
            add_use(uses.entry(name).or_default(), at);
        }
    }
}

/// Record a declaration a body uses under one name; two callees may each use
/// their own declaration of the same name. Whether it was new.
fn add_use(decls: &mut Vec<SourceRange>, at: SourceRange) -> bool {
    let new = !decls.contains(&at);
    if new {
        decls.push(at);
    }
    new
}

/// Add to each body's uses the storage the mods it calls use, through any
/// number of calls, until nothing changes. A callee's use that the calling
/// body itself declares stays the callee's.
///
/// Each pass takes the calls in one fixed order and applies what it found
/// after, so the order a body's uses are recorded in (which names the
/// declaration in the WS091 message) is that of sweeping every call per pass.
/// A pass visits only the calls whose callee gained a use in the pass before:
/// any other found everything it could then.
fn propagate_uses(ctx: &mut TypeCheckCtx) {
    #[cfg(debug_assertions)]
    let swept = {
        let mut uses = ctx.storage_uses.clone();
        propagate_by_sweeping(ctx, &mut uses);
        uses
    };
    let edges: Vec<(DeclKey, DeclKey)> = ctx
        .body_calls
        .iter()
        .flat_map(|(caller, callees)| callees.iter().map(move |callee| (caller.clone(), callee.clone())))
        .collect();
    let mut by_callee: HashMap<&DeclKey, Vec<usize>> = HashMap::default();
    for (i, (_, callee)) in edges.iter().enumerate() {
        by_callee.entry(callee).or_default().push(i);
    }
    let mut visit: Vec<usize> = (0..edges.len()).collect();
    while !visit.is_empty() {
        let mut added: Vec<(DeclKey, String, SourceRange)> = Vec::new();
        for &i in &visit {
            let (caller, callee) = &edges[i];
            let Some(range) = ctx.body_ranges.get(caller) else { continue };
            let own = ctx.storage_uses.get(caller);
            let Some(uses) = ctx.storage_uses.get(callee) else { continue };
            for (name, decls) in uses {
                for at in decls {
                    if !within(range, at) && !own.and_then(|o| o.get(name)).is_some_and(|d| d.contains(at)) {
                        added.push((caller.clone(), name.clone(), at.clone()));
                    }
                }
            }
        }
        let mut grew: crate::collections::HashSet<DeclKey> = crate::collections::HashSet::default();
        for (caller, name, at) in added {
            if add_use(ctx.storage_uses.entry(caller.clone()).or_default().entry(name).or_default(), at) {
                grew.insert(caller);
            }
        }
        visit = grew.iter().filter_map(|k| by_callee.get(k)).flatten().copied().collect();
        visit.sort_unstable();
        visit.dedup();
    }
    #[cfg(debug_assertions)]
    assert_eq!(ctx.storage_uses, swept, "propagated uses differ from sweeping every call per pass");
}

/// [`propagate_uses`] by sweeping every call in each pass, which it must
/// match exactly, order included.
#[cfg(debug_assertions)]
fn propagate_by_sweeping(ctx: &TypeCheckCtx, all: &mut HashMap<DeclKey, HashMap<String, Vec<SourceRange>>>) {
    loop {
        let mut added: Vec<(DeclKey, String, SourceRange)> = Vec::new();
        for (caller, callees) in &ctx.body_calls {
            let Some(range) = ctx.body_ranges.get(caller) else { continue };
            let own = all.get(caller);
            for callee in callees {
                let Some(uses) = all.get(callee) else { continue };
                for (name, decls) in uses {
                    for at in decls {
                        if !within(range, at) && !own.and_then(|o| o.get(name)).is_some_and(|d| d.contains(at)) {
                            added.push((caller.clone(), name.clone(), at.clone()));
                        }
                    }
                }
            }
        }
        if added.is_empty() {
            return;
        }
        for (caller, name, at) in added {
            add_use(all.entry(caller).or_default().entry(name).or_default(), at);
        }
    }
}

/// Every name `decl`'s body mentions, less its params, and those of each
/// `mod`/`chip` it names that is visible here, through any number of calls.
fn reachable_idents(
    ctx: &TypeCheckCtx,
    decl: &ChipDecl,
    visited: &mut crate::collections::HashSet<DeclKey>,
    out: &mut crate::collections::HashSet<String>,
) {
    if !visited.insert((decl.range.file.clone(), decl.range.start.offset)) {
        return;
    }
    let mut names = crate::collections::HashSet::default();
    crate::resolve::collect_idents(crate::ast::walk::Node::Block(&decl.body), crate::resolve::Uses::All, &mut names);
    for p in &decl.inputs {
        names.remove(&p.name);
    }
    for n in &names {
        if let Some(callee) = ctx.resolve_mod(n) {
            reachable_idents(ctx, &callee, visited, out);
        }
    }
    out.extend(names);
}

/// The top-level `mod`/`chip` declarations as [`reachable_idents`] walks them
/// where every name resolves as at the top level: each one's names less its
/// params, the declarations those names call, and, per name, the
/// declarations whose walk reaches a body mentioning it.
#[derive(Default)]
pub(crate) struct TopCalls {
    /// How many top-level declarations this was built from.
    decls: usize,
    own: HashMap<DeclKey, Vec<String>>,
    callers: HashMap<DeclKey, Vec<DeclKey>>,
    mentioned_by: HashMap<String, Vec<DeclKey>>,
    reaching: HashMap<String, Arc<crate::collections::HashSet<DeclKey>>>,
    callees: HashMap<DeclKey, Vec<DeclKey>>,
}

/// What a bare `name` calls as resolved at the top level.
fn resolve_mod_at_top(ctx: &TypeCheckCtx, name: &str) -> Option<Arc<ChipDecl>> {
    match ctx.scope.stack().get_in(0, crate::intern::intern(name)) {
        Some(info) if info.kind == SymbolKind::Chip => ctx.mod_decls.first()?.get(name).cloned(),
        _ => None,
    }
}

/// Whether every name resolves to the same `mod`/`chip` here as at the top
/// level: no `mod` is declared above the top level, and no local binding is a
/// `mod` or shares a top-level one's name.
fn resolves_at_top(ctx: &TypeCheckCtx) -> bool {
    let stack = ctx.scope.stack();
    let Some(top) = ctx.mod_decls.first() else { return false };
    ctx.mod_decls.iter().skip(1).all(HashMap::is_empty)
        && stack.floor() == 0
        && (1..stack.depth()).all(|i| {
            stack
                .frame_entries(i)
                .all(|(sym, info)| info.kind != SymbolKind::Chip && !top.contains_key(crate::intern::resolve(sym)))
        })
}

impl TopCalls {
    fn of(ctx: &TypeCheckCtx) -> Self {
        let mut out = Self { decls: ctx.mod_decls.first().map_or(0, HashMap::len), ..Default::default() };
        for decl in ctx.mod_decls.first().into_iter().flat_map(HashMap::values) {
            let key: DeclKey = (decl.range.file.clone(), decl.range.start.offset);
            let mut names = crate::collections::HashSet::default();
            crate::resolve::collect_idents(crate::ast::walk::Node::Block(&decl.body), crate::resolve::Uses::All, &mut names);
            for p in &decl.inputs {
                names.remove(&p.name);
            }
            let mut callees = Vec::new();
            for n in &names {
                out.mentioned_by.entry(n.clone()).or_default().push(key.clone());
                if let Some(callee) = resolve_mod_at_top(ctx, n) {
                    let to: DeclKey = (callee.range.file.clone(), callee.range.start.offset);
                    out.callers.entry(to.clone()).or_default().push(key.clone());
                    callees.push(to);
                }
            }
            out.callees.insert(key.clone(), callees);
            out.own.insert(key, names.into_iter().collect());
        }
        out
    }

    /// The declarations whose walk reaches a body mentioning `name`.
    fn reaching(&mut self, name: &str) -> Arc<crate::collections::HashSet<DeclKey>> {
        if let Some(r) = self.reaching.get(name) {
            return r.clone();
        }
        let mut found: crate::collections::HashSet<DeclKey> =
            self.mentioned_by.get(name).into_iter().flatten().cloned().collect();
        let mut queue: Vec<DeclKey> = found.iter().cloned().collect();
        while let Some(k) = queue.pop() {
            for caller in self.callers.get(&k).into_iter().flatten() {
                if found.insert(caller.clone()) {
                    queue.push(caller.clone());
                }
            }
        }
        let found = Arc::new(found);
        self.reaching.insert(name.to_string(), found.clone());
        found
    }

    /// [`reachable_idents`] from `key`, sorted.
    fn idents(&self, key: &DeclKey) -> Vec<String> {
        let mut visited: crate::collections::HashSet<&DeclKey> = crate::collections::HashSet::default();
        let mut names: crate::collections::HashSet<&String> = crate::collections::HashSet::default();
        let mut stack = vec![key];
        while let Some(k) = stack.pop() {
            if !visited.insert(k) {
                continue;
            }
            names.extend(self.own.get(k).into_iter().flatten());
            stack.extend(self.callees.get(k).into_iter().flatten());
        }
        let mut names: Vec<String> = names.into_iter().cloned().collect();
        names.sort_unstable();
        names
    }
}

/// The top-level call graph, built on first use and again when the top level
/// has declared more since.
fn top_calls<'c>(ctx: &'c mut TypeCheckCtx<'_>) -> &'c mut TopCalls {
    let decls = ctx.mod_decls.first().map_or(0, HashMap::len);
    if ctx.top_calls.as_ref().is_none_or(|t| t.decls != decls) {
        ctx.top_calls = Some(TopCalls::of(ctx));
    }
    ctx.top_calls.as_mut().expect("just built")
}

/// Note a call of the `mod`/`chip` `decl` by its bare name `name`.
///
/// The names a callee's walk reaches are worked out once per declaration and
/// kept. A declaration first called where every name resolves as at the top
/// level is answered from the top-level call graph instead, for only the names
/// the call site binds, and its full walk is made on its next call, with the
/// top-level resolution it had: a chain of calls then costs one walk, not one
/// per link.
pub(super) fn note_call(ctx: &mut TypeCheckCtx, name: &str, decl: &ChipDecl) {
    // What the call site binds, and the names the callee mentions, are no use
    // of the body the call sits in.
    ctx.scope.pause_storage_log();
    let key: DeclKey = (decl.range.file.clone(), decl.range.start.offset);
    for body in &ctx.open_bodies {
        if *body != key {
            ctx.body_calls.entry(body.clone()).or_default().insert(key.clone());
        }
    }
    // Only a local shadows: a top-level declaration is no local, and what a
    // body names at the top level is decided where the body is written.
    let local = |ctx: &TypeCheckCtx, n: &str| {
        let (_, s) = ctx.scope.binding_frame(n).filter(|(i, _)| *i > 0)?;
        Some((n.to_string(), s.decl_range.clone()))
    };
    let names: Vec<(String, SourceRange)> = match ctx.body_idents.get(&key).cloned() {
        None if !ctx.first_called_at_top.contains(&key) && resolves_at_top(ctx) => {
            ctx.first_called_at_top.insert(key.clone());
            let stack = ctx.scope.stack();
            let mut bound: Vec<String> = (1..stack.depth())
                .flat_map(|i| stack.frame_entries(i).map(|(sym, _)| crate::intern::resolve(sym).to_string()))
                .collect();
            bound.sort_unstable();
            bound.dedup();
            let mut names = Vec::new();
            for n in bound {
                if top_calls(ctx).reaching(&n).contains(&key) {
                    names.extend(local(ctx, &n));
                }
            }
            debug_assert_eq!(
                names,
                {
                    let mut all = crate::collections::HashSet::default();
                    reachable_idents(ctx, decl, &mut crate::collections::HashSet::default(), &mut all);
                    let mut all: Vec<String> = all.into_iter().collect();
                    all.sort_unstable();
                    all.iter().filter_map(|n| local(ctx, n)).collect::<Vec<_>>()
                },
                "WS091 names for a call of `{name}`"
            );
            names
        }
        cached => {
            let idents = match cached {
                Some(names) => names,
                None => {
                    let names = if ctx.first_called_at_top.contains(&key) {
                        top_calls(ctx).idents(&key)
                    } else {
                        let mut names = crate::collections::HashSet::default();
                        let mut visited = crate::collections::HashSet::default();
                        reachable_idents(ctx, decl, &mut visited, &mut names);
                        let mut names: Vec<String> = names.into_iter().collect();
                        names.sort_unstable();
                        names
                    };
                    let names = Arc::new(names);
                    ctx.body_idents.insert(key.clone(), names.clone());
                    names
                }
            };
            idents.iter().filter_map(|n| local(ctx, n)).collect()
        }
    };
    ctx.scope.resume_storage_log();
    if !names.is_empty() {
        ctx.call_sites.push(CallSite { callee: key, callee_name: name.to_string(), names });
    }
}

/// WS091 for every call whose body uses a storage declaration that the call
/// site binds, under the same name, to something else. Reported once per
/// shadowing declaration and name, at the declaration.
pub(super) fn report(ctx: &mut TypeCheckCtx) {
    propagate_uses(ctx);
    let mut seen: crate::collections::HashSet<(SourceRange, String)> = crate::collections::HashSet::default();
    let mut found: Vec<(SourceRange, String)> = Vec::new();
    for site in &ctx.call_sites {
        let Some(uses) = ctx.storage_uses.get(&site.callee) else { continue };
        for (name, here) in &site.names {
            let Some(used) = uses.get(name).and_then(|d| d.iter().find(|u| *u != here)) else { continue };
            if !seen.insert((here.clone(), name.clone())) {
                continue;
            }
            let at = if used.file == here.file {
                format!("line {}", used.start.line)
            } else {
                format!("{} line {}", display_file(&used.file), used.start.line)
            };
            found.push((
                here.clone(),
                format!(
                    "this `{name}` shadows the `{name}` that `{}` uses, and `{}` is called in \
                     this local's scope: the called mod uses the outer `{name}` ({at}), not this \
                     local. Rename one of them, or pass the local to the call",
                    site.callee_name, site.callee_name
                ),
            ));
        }
    }
    for (range, message) in found {
        ctx.warn("WS091", message, range);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn another_files_declaration_is_named_by_its_file_name() {
        assert_eq!(super::display_file(r"\\?\C:\Users\x\ns\lib4.ws"), "lib4.ws");
        assert_eq!(super::display_file("C:/Users/x/lib.ws"), "lib.ws");
        assert_eq!(super::display_file("lib.ws"), "lib.ws");
    }
}
