//! The top-level constant environment: every `let`/`const` of a module whose
//! initializer evaluates, by name.

use std::sync::Arc;

use crate::ast::{Block, ChipDecl, ImportScope, LetDecl, Stmt, TopDecl, TypeExpr};
use crate::collections::{HashMap, HashSet};
use crate::diagnostic::SourceRange;
use crate::ir::Literal;

/// Compile-time constant environment: every top-level `let` whose initializer is
/// itself constant, by name. Lets an initializer name a constant (`1 << C_FLAG`)
/// instead of restating its value.
pub(crate) type ConstEnv = HashMap<String, Literal>;

/// A `let x: exec` local exec signal, which must never be collected as a
/// constant. A `let` written with no value is one through an alias of `exec`
/// (any other type is an error), whatever the alias.
fn is_exec_signal_let(l: &LetDecl) -> bool {
    matches!(&l.typ, Some(TypeExpr::Name { name, .. }) if name == "exec") || !l.has_initializer()
}

/// Every `let`/`const` declaration in the module's top-level constant scope,
/// in source order.
///
/// An anonymous `chip { }` shares its parent's scope (see `AnonChipDecl` and
/// `lower::decl::lower_anon_chip`), so its body's lets are top-level constants
/// too; without this descent its `const` would read back as a runtime value.
/// Descends through nested anonymous chips and stops at anything that opens
/// its own scope (a handler, a named `chip`/`mod` body, an `if` block).
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

/// Every `mod`/`chip` declaration in the module's top-level scope (top level
/// plus anonymous-chip bodies, flattened as in [`scope_lets`]), keyed by name,
/// const or not. Fed to `const_eval` as `lookup_mod`:
/// - const-mod call evaluation gates on `decl.is_const`, so a non-const mod
///   resolves but declines to fold;
/// - `const_eval::resolve_bare_variant_enum` needs every mod so a user
///   `mod Some` shadows the prelude `Some`, matching typecheck's `resolve_mod`.
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

/// Collect the constant top-level `let` bindings of a script (test-only entry
/// point; see [`build_const_env_at`]). `enum_defs` is the whole-program enum
/// registry.
///
/// Iterates to a fixpoint, so a constant may name another regardless of
/// declaration order, and a `const mod` declared later resolves like one
/// declared earlier. A binding that never resolves (it needs a runtime value,
/// or is in a reference cycle) is absent. Each attempt has a bounded `Budget`.
#[cfg(test)]
pub(crate) fn build_const_env(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
) -> ConstEnv {
    let types = Arc::new(crate::const_eval::DeclaredTypes::for_program(
        decls, enum_defs,
    ));
    build_const_env_in(decls, enum_defs, &Arc::default(), &types)
}

/// [`build_const_env`] for a program whose files import each other: a
/// `const mod` named like a builtin is called only where `imports` says the
/// calling file can name it.
pub(crate) fn build_const_env_in(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
    imports: &Arc<ImportScope>,
    types: &Arc<crate::const_eval::DeclaredTypes>,
) -> ConstEnv {
    build_const_env_at(
        decls,
        enum_defs,
        imports,
        types,
        &Arc::new(crate::scope::top::TopNames::of(decls)),
    )
    .0
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
    imports: &Arc<ImportScope>,
    types: &Arc<crate::const_eval::DeclaredTypes>,
    top: &Arc<crate::scope::top::TopNames>,
) -> (ConstEnv, DupConsts) {
    let mods = scope_mods(decls);
    let lookup_mod = move |name: &str| mods.get(name).cloned();

    let lets = scope_lets(decls);
    // `Arc`-wrapped because `ConstCtx` owns its tables. Every `ConstCtx` built
    // here is dropped before the next `make_mut`, so the refcount is back to 1
    // and the table is never deep-copied.
    let mut env = std::sync::Arc::new(ConstEnv::default());
    // An `import * as Ns` module's constants are evaluated in isolation (its
    // lets see only its own module's consts), then lifted into `"Ns.name"`
    // keys. Seeded before the fixpoint so a root const reading one settles.
    for d in decls {
        if let TopDecl::Namespace(ns) = d {
            for (name, value) in build_const_env_in(&ns.decls, enum_defs, imports, types) {
                std::sync::Arc::make_mut(&mut env)
                    .entry(format!("{}.{}", ns.name, name))
                    .or_insert(value);
            }
        }
    }
    // Which decls have produced their final answer, by position (a binding can
    // introduce several names, so `env.contains_key` cannot tell). A decl is
    // settled once its initializer evaluates; one that does not is retried.
    let mut settled = vec![false; lets.len()];
    // `scope_lets` lists the top level's own `let`s first.
    let top_lets = decls
        .iter()
        .filter(|d| matches!(d, TopDecl::Let(_)))
        .count();
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
            // A `let x: exec` signal is parser-desugared to a placeholder `= 0`.
            // Binding it would inline `0` into exec wire ports (e.g. SpawnPrefab's
            // `destroyAll`) instead of wiring the signal.
            if is_exec_signal_let(l) {
                settled[i] = true;
                continue;
            }
            // Scoped so the `ConstCtx` is dropped before `make_mut` runs below.
            let evaluated = {
                let value = crate::ast::walk::Node::Expr(&l.value);
                let reads_dup = dup_names
                    .iter()
                    .any(|n| crate::ast::walk::mentions(value, n));
                let seen = if reads_dup {
                    Arc::new(dup_env_at(&env, top, &dup_consts, &l.range))
                } else {
                    std::sync::Arc::clone(&env)
                };
                let mod_dups = (!dup_names.is_empty()).then(|| match &mod_view {
                    Some((at, view)) if *at == env_rev => std::sync::Arc::clone(view),
                    _ => {
                        let view = Arc::new(crate::const_eval::ModDups::new(
                            std::sync::Arc::clone(&env),
                            std::sync::Arc::clone(top),
                            Arc::new(dup_consts.clone()),
                        ));
                        mod_view = Some((env_rev, std::sync::Arc::clone(&view)));
                        view
                    }
                });
                let cx = crate::const_eval::ConstCtx::new(
                    crate::const_scope::ConstNames::of(seen),
                    std::sync::Arc::clone(&env),
                    Arc::default(),
                    std::sync::Arc::clone(enum_defs),
                    std::sync::Arc::clone(imports),
                    std::sync::Arc::clone(types),
                    Some(&lookup_mod),
                )
                .with_mod_dups(mod_dups);
                let mut budget = crate::const_eval::Budget::default();
                crate::const_eval::eval_let_value(l, &cx, &mut budget)
            };
            let Ok(lit) = evaluated else {
                continue; // not constant yet; retry on a later pass
            };
            settled[i] = true;
            // Split the value across the names the binding introduces, through
            // the same `bind_destructured` typecheck and the const-mod
            // interpreter use. A form it cannot split contributes nothing.
            let Ok(pairs) = crate::const_eval::bind_destructured(&l.binding, lit) else {
                continue;
            };
            for (name, value) in pairs {
                if i < top_lets && top.is_dup(&l.range.file, &name) {
                    dup_consts.insert(
                        (
                            std::sync::Arc::clone(&l.range.file),
                            l.range.start.offset,
                            name,
                        ),
                        value,
                    );
                    env_rev += 1;
                    changed = true;
                    continue;
                }
                // The first declaration of a name wins: a duplicate that
                // overwrote it could make the fixpoint alternate forever.
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
                    Some(lit) => {
                        std::sync::Arc::make_mut(&mut env).insert(name.to_string(), lit.clone())
                    }
                    None => std::sync::Arc::make_mut(&mut env).remove(name),
                };
            }
            // Sole owner here, so this hands back the table itself.
            return (
                std::sync::Arc::try_unwrap(env).unwrap_or_else(|env| (*env).clone()),
                dup_consts,
            );
        }
    }
}

/// `env` as a read at `at` sees it: each name a file declares more than once
/// at its top level holds the constant of the declaration visible at `at`
/// (the last one, read from another file), and is absent when that is no
/// constant or not settled yet.
fn dup_env_at(
    env: &ConstEnv,
    top: &crate::scope::top::TopNames,
    dup_consts: &DupConsts,
    at: &SourceRange,
) -> ConstEnv {
    let mut out = env.clone();
    for (name, decls) in top.dups() {
        let decl = if decls[0].range.file == at.file {
            top.visible_decl(name, &crate::scope::top::ReadAt::code(at))
                .flatten()
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
    (
        std::sync::Arc::clone(&d.range.file),
        d.range.start.offset,
        name.to_string(),
    )
}

/// Every top-level name declared with the `const` keyword, as opposed to a
/// plain `let` that happens to fold. [`crate::const_scope::ConstScope`] gates
/// the `if`-condition elision on this set, so a program with no `const`
/// compiles as if const evaluation did not exist. A name whose initializer
/// never evaluates has no entry in the const env, so nothing resolves through it.
///
/// A destructuring `const` contributes every name it binds, via
/// `const_eval::bound_names` (the syntactic counterpart of `bind_destructured`).
pub(crate) fn build_const_declared_names(decls: &[TopDecl]) -> HashSet<String> {
    // Same [`scope_lets`] flattening as [`build_const_env_at`]: the two must
    // see the same bindings, or a name is const-declared with no value.
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
pub(crate) fn build_const_uncertified(
    decls: &[TopDecl],
    enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
    imports: &Arc<ImportScope>,
    env: &ConstEnv,
    types: &Arc<crate::const_eval::DeclaredTypes>,
) -> HashSet<String> {
    let mut tainted: HashSet<String> = HashSet::default();
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
        let certified: Arc<ConstEnv> = Arc::new(
            env.iter()
                .filter(|(n, _)| !tainted.contains(*n))
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect(),
        );
        let cx = crate::const_eval::ConstCtx::new(
            crate::const_scope::ConstNames::of(std::sync::Arc::clone(&certified)),
            certified,
            Arc::default(),
            std::sync::Arc::clone(enum_defs),
            std::sync::Arc::clone(imports),
            std::sync::Arc::clone(types),
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
