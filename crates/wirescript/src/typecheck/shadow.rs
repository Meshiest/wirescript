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
/// their own declaration of the same name.
fn add_use(decls: &mut Vec<SourceRange>, at: SourceRange) {
    if !decls.contains(&at) {
        decls.push(at);
    }
}

/// Add to each body's uses the storage the mods it calls use, through any
/// number of calls, until nothing changes. A callee's use that the calling
/// body itself declares stays the callee's.
fn propagate_uses(ctx: &mut TypeCheckCtx) {
    loop {
        let mut added: Vec<(DeclKey, String, SourceRange)> = Vec::new();
        for (caller, callees) in &ctx.body_calls {
            let Some(range) = ctx.body_ranges.get(caller) else { continue };
            let own = ctx.storage_uses.get(caller);
            for callee in callees {
                let Some(uses) = ctx.storage_uses.get(callee) else { continue };
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
            add_use(ctx.storage_uses.entry(caller).or_default().entry(name).or_default(), at);
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
    crate::resolve::collect_idents_in_block(&decl.body, &mut names);
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

/// Note a call of the `mod`/`chip` `decl` by its bare name `name`.
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
    let idents = match ctx.body_idents.get(&key) {
        Some(names) => names.clone(),
        None => {
            let mut names = crate::collections::HashSet::default();
            let mut visited = crate::collections::HashSet::default();
            reachable_idents(ctx, decl, &mut visited, &mut names);
            let mut names: Vec<String> = names.into_iter().collect();
            names.sort_unstable();
            let names = Arc::new(names);
            ctx.body_idents.insert(key.clone(), names.clone());
            names
        }
    };
    // Only a local shadows: a top-level declaration is no local, and what a
    // body names at the top level is decided where the body is written.
    let names: Vec<(String, SourceRange)> = idents
        .iter()
        .filter_map(|n| {
            let (_, s) = ctx.scope.binding_frame(n).filter(|(i, _)| *i > 0)?;
            Some((n.clone(), s.decl_range.clone()))
        })
        .collect();
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
