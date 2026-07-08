//! Thread-safe cache for [`CompiledTemplate`]s with dependency-aware ordering.
//!
//! [`TemplateCache`] stores compiled module templates and tracks which modules
//! depend on which others.  Two ordering helpers let the caller schedule
//! compilation work efficiently:
//!
//! * [`topo_order`][TemplateCache::topo_order] — a flat topological ordering
//!   (leaves first) produced by Kahn's algorithm.
//! * [`parallel_tiers`][TemplateCache::parallel_tiers] — the same ordering
//!   partitioned into tiers of work that can be done in parallel, where every
//!   module in tier *N* depends only on modules in tiers 0…N-1.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use crate::ast::{
    Block, CallArg, ChipDecl, Expr, If, InterpPart, RecordLitField, Script, Stmt, TopDecl,
};
use crate::template::{CompiledTemplate, InlineModEntry};

/// Thread-safe store of compiled templates with a dependency graph.
pub struct TemplateCache {
    /// Compiled templates keyed by module name (standalone chips).
    templates: RwLock<HashMap<String, Arc<CompiledTemplate>>>,
    /// Cached inline mod expansions (first-call delta + metadata).
    inline_mods: RwLock<HashMap<String, Arc<InlineModEntry>>>,
    /// Adjacency map: `name → set of names that *name* depends on`.
    /// Every node that has ever been mentioned (as a dependent or a dependency)
    /// appears as a key so that `topo_order` and `parallel_tiers` see the full
    /// graph.
    pub(crate) deps: RwLock<HashMap<String, HashSet<String>>>,
}

impl TemplateCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            templates: RwLock::new(HashMap::new()),
            inline_mods: RwLock::new(HashMap::new()),
            deps: RwLock::new(HashMap::new()),
        }
    }

    /// Record that `name` depends on every module listed in `depends_on`.
    ///
    /// Both `name` and each dependency are inserted into the graph as keys
    /// (with an empty dep-set if they were not already present) so that the
    /// full node set is always visible to the ordering algorithms.
    pub fn register_dependency(&mut self, name: &str, depends_on: &[&str]) {
        let mut deps = self.deps.write().unwrap();
        // Ensure the dependent itself exists in the graph.
        let entry = deps.entry(name.to_string()).or_default();
        for &dep in depends_on {
            entry.insert(dep.to_string());
        }
        // Ensure each dependency also exists as a key (leaf nodes).
        for &dep in depends_on {
            deps.entry(dep.to_string()).or_default();
        }
    }

    /// Store a compiled template under `name`.
    pub fn insert(&self, name: &str, template: CompiledTemplate) {
        self.templates
            .write()
            .unwrap()
            .insert(name.to_string(), Arc::new(template));
    }

    /// Number of compiled standalone chip templates.
    pub fn template_count(&self) -> usize {
        self.templates.read().unwrap().len()
    }

    /// Retrieve a compiled template by name, or `None` if not yet compiled.
    pub fn get(&self, name: &str) -> Option<Arc<CompiledTemplate>> {
        self.templates.read().unwrap().get(name).cloned()
    }

    /// Store a cached inline mod expansion.
    pub fn insert_inline(&self, name: &str, entry: InlineModEntry) {
        self.inline_mods.write().unwrap().insert(name.to_string(), Arc::new(entry));
    }

    /// Retrieve a cached inline mod expansion.
    pub fn get_inline(&self, name: &str) -> Option<Arc<InlineModEntry>> {
        self.inline_mods.read().unwrap().get(name).cloned()
    }

    /// Return all module names in topological order (leaves — modules with no
    /// unresolved dependencies — come first).
    ///
    /// Uses Kahn's algorithm.  Within each "batch" of nodes that become
    /// available at the same time the output is sorted alphabetically so that
    /// the result is deterministic regardless of [`HashMap`] iteration order.
    pub fn topo_order(&self) -> Vec<String> {
        let deps = self.deps.read().unwrap();

        // Build in-degree map and reverse adjacency (who depends on me?).
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut rev_adj: HashMap<&str, Vec<&str>> = HashMap::new();

        for name in deps.keys() {
            in_degree.entry(name.as_str()).or_insert(0);
            rev_adj.entry(name.as_str()).or_default();
        }

        for (name, dep_set) in deps.iter() {
            for dep in dep_set {
                // name depends on dep  →  dep is a prerequisite of name
                // in_degree counts "how many unresolved deps does this node have"
                *in_degree.entry(name.as_str()).or_insert(0) += 1;
                // If dep is satisfied, name becomes one step closer to ready.
                rev_adj.entry(dep.as_str()).or_default().push(name.as_str());
            }
        }

        // We double-counted: the `entry(...).or_insert(0)` above already
        // initialised everything to 0, then we added the dep counts on top.
        // That is correct — reset and recount cleanly.
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for name in deps.keys() {
            in_degree.insert(name.as_str(), deps[name].len());
        }

        // Initial queue: all nodes with in-degree 0 (leaves), sorted.
        let mut queue: VecDeque<&str> = {
            let mut ready: Vec<&str> = in_degree
                .iter()
                .filter(|&(_, &d)| d == 0)
                .map(|(&n, _)| n)
                .collect();
            ready.sort_unstable();
            VecDeque::from(ready)
        };

        let mut order: Vec<String> = Vec::with_capacity(deps.len());

        while let Some(node) = queue.pop_front() {
            order.push(node.to_string());

            // Collect neighbours that become ready, sort them, then enqueue.
            if let Some(dependents) = rev_adj.get(node) {
                let mut newly_ready: Vec<&str> = dependents
                    .iter()
                    .filter_map(|&dep| {
                        let d = in_degree.get_mut(dep)?;
                        *d -= 1;
                        if *d == 0 { Some(dep) } else { None }
                    })
                    .collect();
                newly_ready.sort_unstable();
                for n in newly_ready {
                    queue.push_back(n);
                }
            }
        }

        order
    }

    // ── AST scanning ─────────────────────────────────────────────────────────

    /// Walk `ast.decls` and register every `chip`/`mod` declaration together
    /// with the set of other known chips/mods that it directly calls in its
    /// body.
    ///
    /// Must be called before [`scan_top_level_calls`] or [`reachable_from`].
    pub fn scan_declarations(&mut self, ast: &Script) {
        // First pass: collect all chip/mod names so we know what counts as a
        // call to a known declaration.
        let known: HashSet<String> = ast
            .decls
            .iter()
            .filter_map(|d| {
                if let TopDecl::Chip(c) = d {
                    Some(c.name.clone())
                } else {
                    None
                }
            })
            .collect();

        // Second pass: for each chip/mod, find which known names it calls.
        for decl in &ast.decls {
            if let TopDecl::Chip(c) = decl {
                let mut called = HashSet::new();
                collect_calls_in_block(&c.body, &known, &mut called);
                let dep_refs: Vec<&str> = called.iter().map(|s| s.as_str()).collect();
                self.register_dependency(&c.name, &dep_refs);
            }
        }
    }

    /// Find all known chips/mods that are called from **top-level** code —
    /// i.e. from declarations that are not themselves a `chip`/`mod` body.
    ///
    /// These are the BFS roots for reachability analysis.
    ///
    /// [`scan_declarations`] must be called first so that the known-name set
    /// is populated.
    pub fn scan_top_level_calls(&self, ast: &Script) -> Vec<String> {
        let known: HashSet<String> = {
            let deps = self.deps.read().unwrap();
            deps.keys().cloned().collect()
        };

        let mut found: HashSet<String> = HashSet::new();
        for decl in &ast.decls {
            match decl {
                // Skip chip/mod bodies — those are NOT top-level call sites.
                TopDecl::Chip(_) => {}
                TopDecl::Let(ld) => collect_calls_in_expr(&ld.value, &known, &mut found),
                TopDecl::Out(ob) => {
                    if let Some(v) = &ob.value {
                        collect_calls_in_expr(v, &known, &mut found);
                    }
                }
                TopDecl::Handler(h) => collect_calls_in_block(&h.body, &known, &mut found),
                TopDecl::Assign(a) => {
                    collect_calls_in_expr(&a.target, &known, &mut found);
                    collect_calls_in_expr(&a.value, &known, &mut found);
                }
                TopDecl::If(i) => {
                    collect_calls_in_if(i, &known, &mut found);
                }
                TopDecl::ExprStmt(es) => collect_calls_in_expr(&es.expr, &known, &mut found),
                TopDecl::Var(v) => {
                    if let Some(init) = &v.init {
                        collect_calls_in_expr(init, &known, &mut found);
                    }
                }
                TopDecl::Buffer(b) => collect_calls_in_expr(&b.init, &known, &mut found),
                TopDecl::AnonChip(ac) => collect_calls_in_block(&ac.body, &known, &mut found),
                // Import / Namespace / In / Fn / Event / TypeAlias / Array have
                // no call-containing sub-expressions at the top level.
                _ => {}
            }
        }

        let mut result: Vec<String> = found.into_iter().collect();
        result.sort_unstable();
        result
    }

    /// BFS from `roots` through the dependency graph.  Returns the set of all
    /// reachable node names (roots included).
    pub fn reachable_from(&self, roots: &[&str]) -> HashSet<String> {
        let deps = self.deps.read().unwrap();

        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = roots.iter().map(|s| s.to_string()).collect();

        while let Some(node) = queue.pop_front() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node.clone());

            if let Some(dep_set) = deps.get(&node) {
                for dep in dep_set {
                    if !visited.contains(dep) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        visited
    }

    /// Partition modules into parallel tiers.
    ///
    /// * **Tier 0** — leaf modules (no dependencies).
    /// * **Tier N** — modules whose dependencies are all in tiers 0…N-1.
    ///
    /// Each tier is sorted alphabetically for determinism.
    pub fn parallel_tiers(&self) -> Vec<Vec<String>> {
        let deps = self.deps.read().unwrap();

        // `remaining[name]` = deps of name that have not yet been placed in a tier.
        let mut remaining: HashMap<&str, HashSet<&str>> = deps
            .iter()
            .map(|(name, dep_set)| (name.as_str(), dep_set.iter().map(|s| s.as_str()).collect()))
            .collect();

        let mut tiers: Vec<Vec<String>> = Vec::new();
        let mut done: HashSet<&str> = HashSet::new();

        loop {
            // Find all nodes whose remaining deps are empty (i.e. all satisfied).
            let mut tier: Vec<&str> = remaining
                .iter()
                .filter(|(_, deps_left)| deps_left.is_empty())
                .map(|(&name, _)| name)
                .collect();

            if tier.is_empty() {
                break;
            }

            tier.sort_unstable();

            // Mark these as done and remove from `remaining`.
            for &name in &tier {
                done.insert(name);
                remaining.remove(name);
            }

            // Strip newly-done nodes from the remaining dep sets.
            for dep_set in remaining.values_mut() {
                for &name in &tier {
                    dep_set.remove(name);
                }
            }

            tiers.push(tier.iter().map(|s| s.to_string()).collect());
        }

        tiers
    }
}

impl Default for TemplateCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── AST-walking helpers (module-private) ─────────────────────────────────────

fn collect_calls_in_block(block: &Block, known: &HashSet<String>, out: &mut HashSet<String>) {
    for stmt in &block.stmts {
        collect_calls_in_stmt(stmt, known, out);
    }
}

fn collect_calls_in_stmt(stmt: &Stmt, known: &HashSet<String>, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Let(ld) => collect_calls_in_expr(&ld.value, known, out),
        Stmt::Assign(a) => {
            collect_calls_in_expr(&a.target, known, out);
            collect_calls_in_expr(&a.value, known, out);
        }
        Stmt::If(i) => collect_calls_in_if(i, known, out),
        Stmt::ExprStmt(es) => collect_calls_in_expr(&es.expr, known, out),
        Stmt::Var(v) => {
            if let Some(init) = &v.init {
                collect_calls_in_expr(init, known, out);
            }
        }
        Stmt::Buffer(b) => collect_calls_in_expr(&b.init, known, out),
        Stmt::OutBinding(ob) => {
            if let Some(v) = &ob.value {
                collect_calls_in_expr(v, known, out);
            }
        }
        Stmt::Handler(h) => collect_calls_in_block(&h.body, known, out),
        Stmt::AnonChip(ac) => collect_calls_in_block(&ac.body, known, out),
        Stmt::ChipDecl(c) => collect_calls_in_chip(c, known, out),
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                collect_calls_in_expr(e, known, out);
            }
        }
        // Emit / In / Array — no sub-expressions to walk.
        _ => {}
    }
}

fn collect_calls_in_if(i: &If, known: &HashSet<String>, out: &mut HashSet<String>) {
    collect_calls_in_expr(&i.cond, known, out);
    collect_calls_in_block(&i.then_block, known, out);
    if let Some(eb) = &i.else_block {
        collect_calls_in_block(eb, known, out);
    }
}

fn collect_calls_in_chip(c: &ChipDecl, known: &HashSet<String>, out: &mut HashSet<String>) {
    collect_calls_in_block(&c.body, known, out);
}

fn collect_calls_in_expr(expr: &Expr, known: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        Expr::Call { callee, args, .. } => {
            // Check if the callee is a direct reference to a known chip/mod.
            if let Expr::Ident { name, .. } = callee.as_ref() {
                if known.contains(name) {
                    out.insert(name.clone());
                }
            }
            // Recurse into callee (could itself be a call expression) and args.
            collect_calls_in_expr(callee, known, out);
            for arg in args {
                match arg {
                    CallArg::Positional(e) => collect_calls_in_expr(e, known, out),
                    CallArg::Named { value, .. } => collect_calls_in_expr(value, known, out),
                    CallArg::Spread(e) => collect_calls_in_expr(e, known, out),
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            collect_calls_in_expr(left, known, out);
            collect_calls_in_expr(right, known, out);
        }
        Expr::UnOp { operand, .. } | Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
            collect_calls_in_expr(operand, known, out)
        }
        Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } => {
            collect_calls_in_expr(obj, known, out)
        }
        Expr::IndexAccess { obj, index, .. } => {
            collect_calls_in_expr(obj, known, out);
            collect_calls_in_expr(index, known, out);
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_calls_in_expr(cond, known, out);
            collect_calls_in_expr(then_branch, known, out);
            collect_calls_in_expr(else_branch, known, out);
        }
        Expr::BlockExpr { stmts, value, .. } => {
            for s in stmts {
                collect_calls_in_stmt(s, known, out);
            }
            collect_calls_in_expr(value, known, out);
        }
        Expr::InterpLit { parts, .. } => {
            for p in parts {
                if let InterpPart::Expr(e) = p {
                    collect_calls_in_expr(e, known, out);
                }
            }
        }
        Expr::RecordLit { fields, .. } => {
            for f in fields {
                match f {
                    RecordLitField::Named { value, .. } => collect_calls_in_expr(value, known, out),
                    RecordLitField::Spread { value, .. } => {
                        collect_calls_in_expr(value, known, out)
                    }
                    RecordLitField::Shorthand { .. } => {}
                }
            }
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            collect_calls_in_expr(scrutinee, known, out);
            for arm in arms {
                match &arm.body {
                    crate::ast::MatchBody::Expr(e) => collect_calls_in_expr(e, known, out),
                    crate::ast::MatchBody::Block(b) => collect_calls_in_block(b, known, out),
                }
            }
        }
        Expr::Array { elements, .. } => {
            for e in elements {
                collect_calls_in_expr(e.expr(), known, out);
            }
        }
        // Literals and bare identifiers have nothing to recurse into.
        Expr::IntLit { .. }
        | Expr::FloatLit { .. }
        | Expr::StringLit { .. }
        | Expr::BoolLit { .. }
        | Expr::AssetRef { .. }
        | Expr::PrefabRef { .. }
        | Expr::Ident { .. } => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Module;
    use crate::template::CompiledTemplate;

    fn make_template(name: &str) -> CompiledTemplate {
        let m = Module::new(name);
        CompiledTemplate::from_module(m)
    }

    // ── Test 1 ────────────────────────────────────────────────────────────────
    /// A → B → C chain: C should appear before B, B before A.
    #[test]
    fn topo_order_leaves_first() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["C"]);
        // C is a leaf — no explicit registration needed, but register it so
        // it is definitely in the graph.
        cache.register_dependency("C", &[]);

        let order = cache.topo_order();

        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("C") < pos("B"), "C must come before B");
        assert!(pos("B") < pos("A"), "B must come before A");
    }

    // ── Test 2 ────────────────────────────────────────────────────────────────
    /// A → C and B → C: C must appear before both A and B.
    #[test]
    fn topo_order_independent_modules_adjacent() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["C"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &[]);

        let order = cache.topo_order();

        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("C") < pos("A"), "C must come before A");
        assert!(pos("C") < pos("B"), "C must come before B");
    }

    // ── Test 3 ────────────────────────────────────────────────────────────────
    /// A → {C, D}; B → C; C and D are leaves.
    /// Expected: tier 0 = [C, D], A and B in later tiers.
    #[test]
    fn parallel_tiers_groups_independent_work() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["C", "D"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &[]);
        cache.register_dependency("D", &[]);

        let tiers = cache.parallel_tiers();

        assert!(!tiers.is_empty(), "expected at least one tier");

        // Tier 0 must be the two leaves.
        assert_eq!(
            tiers[0],
            vec!["C".to_string(), "D".to_string()],
            "tier 0 should be [C, D] (sorted)"
        );

        // A and B must appear in tiers after 0.
        let a_tier = tiers.iter().position(|t| t.contains(&"A".to_string()));
        let b_tier = tiers.iter().position(|t| t.contains(&"B".to_string()));
        assert!(a_tier.unwrap() > 0, "A should be in a tier after 0");
        assert!(b_tier.unwrap() > 0, "B should be in a tier after 0");
    }

    // ── Test 4 ────────────────────────────────────────────────────────────────
    /// Basic insert + get round-trip.
    #[test]
    fn cache_stores_and_retrieves() {
        let cache = TemplateCache::new();
        let t = make_template("test");
        cache.insert("mymod", t);

        let retrieved = cache.get("mymod");
        assert!(
            retrieved.is_some(),
            "expected to retrieve inserted template"
        );
        assert!(cache.get("missing").is_none());
    }

    // ── Test 5 ────────────────────────────────────────────────────────────────
    /// Diamond: A → B, C; B → D; C → D; D leaf.
    /// D must be in tier 0, B and C in the same tier, A last.
    #[test]
    fn diamond_dependency() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B", "C"]);
        cache.register_dependency("B", &["D"]);
        cache.register_dependency("C", &["D"]);
        cache.register_dependency("D", &[]);

        let tiers = cache.parallel_tiers();
        let order = cache.topo_order();

        // D must be in tier 0.
        assert_eq!(tiers[0], vec!["D".to_string()], "tier 0 must be [D]");

        // B and C must be in the same tier (tier 1).
        let bc_tier_idx = tiers
            .iter()
            .position(|t| t.contains(&"B".to_string()))
            .unwrap();
        let c_tier_idx = tiers
            .iter()
            .position(|t| t.contains(&"C".to_string()))
            .unwrap();
        assert_eq!(bc_tier_idx, c_tier_idx, "B and C must be in the same tier");
        assert!(bc_tier_idx > 0, "B and C must not be in tier 0");

        // A must be after B and C in topo order.
        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("D") < pos("B"), "D before B");
        assert!(pos("D") < pos("C"), "D before C");
        assert!(pos("B") < pos("A"), "B before A");
        assert!(pos("C") < pos("A"), "C before A");
    }

    // ── Test 6 ────────────────────────────────────────────────────────────────
    /// A self-recursive dependency must not hang topo_order.
    #[test]
    fn self_recursive_does_not_hang() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["A"]);

        // Must return without hanging; a cycle means Kahn's algorithm will
        // simply not emit the cyclic node(s).
        let order = cache.topo_order();
        assert!(
            order.len() <= 1,
            "expected at most 1 node in result, got {:?}",
            order
        );
    }

    // ── Test 7 ────────────────────────────────────────────────────────────────
    /// Mutual recursion A → B, B → A must not hang.
    #[test]
    fn mutual_recursion_does_not_hang() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["A"]);

        let order = cache.topo_order();
        assert!(
            order.len() < 2,
            "cycle nodes should not appear in topo order, got {:?}",
            order
        );
    }

    // ── Test 8 ────────────────────────────────────────────────────────────────
    /// Both a "Used" and an "Unused" leaf module must appear in tier 0.
    #[test]
    fn unused_module_still_in_tiers() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("Used", &[]);
        cache.register_dependency("Unused", &[]);

        let tiers = cache.parallel_tiers();
        assert_eq!(tiers.len(), 1, "expected exactly one tier");
        assert!(
            tiers[0].contains(&"Used".to_string()),
            "tier 0 should contain 'Used'"
        );
        assert!(
            tiers[0].contains(&"Unused".to_string()),
            "tier 0 should contain 'Unused'"
        );
    }

    // ── Test 9 ────────────────────────────────────────────────────────────────
    /// A single module with no deps produces exactly one tier containing it.
    #[test]
    fn single_module_no_deps() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("Solo", &[]);

        let tiers = cache.parallel_tiers();
        assert_eq!(tiers.len(), 1, "expected exactly 1 tier");
        assert_eq!(tiers[0], vec!["Solo".to_string()]);
    }

    // ── Test 11 ──────────────────────────────────────────────────────────────
    /// BFS reachability: Used+Dep reachable from Used; Unused is not.
    #[test]
    fn reachable_filters_unused() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("Used", &["Dep"]);
        cache.register_dependency("Dep", &[]);
        cache.register_dependency("Unused", &["Dep"]);
        let reachable = cache.reachable_from(&["Used"]);
        assert!(reachable.contains("Used"));
        assert!(reachable.contains("Dep"));
        assert!(!reachable.contains("Unused"));
    }

    // ── Test 12 ──────────────────────────────────────────────────────────────
    /// BFS reachability: A→B→C; X not reachable from A.
    #[test]
    fn reachable_transitive() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &[]);
        cache.register_dependency("X", &[]);
        let reachable = cache.reachable_from(&["A"]);
        assert_eq!(reachable.len(), 3); // A, B, C — not X
        assert!(reachable.contains("A"));
        assert!(reachable.contains("C"));
        assert!(!reachable.contains("X"));
    }

    // ── Test 13 ──────────────────────────────────────────────────────────────
    /// scan_declarations finds chip and mod deps correctly.
    #[test]
    fn scan_finds_chip_and_mod_deps() {
        let parsed = crate::parser::parse(
            "chip ALU(a: int, b: int) -> (r: int) { out r = a + b }\n\
             mod process(x: int) -> (r: int) { return ALU(x, 1) }\n\
             let p = process(10)\n\
             out result = p",
            "test.ws",
        );
        let mut cache = TemplateCache::new();
        cache.scan_declarations(&parsed.ast);
        let deps = cache.deps.read().unwrap();
        assert!(deps.contains_key("ALU"));
        assert!(deps.contains_key("process"));
        assert!(deps["ALU"].is_empty());
        assert!(deps["process"].contains("ALU"));
    }

    // ── Test 14 ──────────────────────────────────────────────────────────────
    /// scan_top_level_calls finds roots in top-level expressions.
    #[test]
    fn scan_top_level_calls_finds_roots() {
        let parsed = crate::parser::parse(
            "chip ALU(a: int, b: int) -> (r: int) { out r = a + b }\n\
             mod process(x: int) -> (r: int) { return ALU(x, 1) }\n\
             let p = process(10)\n\
             out result = p",
            "test.ws",
        );
        let mut cache = TemplateCache::new();
        cache.scan_declarations(&parsed.ast);
        let roots = cache.scan_top_level_calls(&parsed.ast);
        assert!(
            roots.contains(&"process".to_string()),
            "process is called at top level"
        );
        assert!(
            !roots.contains(&"ALU".to_string()),
            "ALU is only called inside process, not at top level"
        );
    }

    // ── Test 10 ───────────────────────────────────────────────────────────────
    /// A → B → C → D → E (5-node chain): 5 tiers, E first, A last.
    #[test]
    fn long_chain_correct_order() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &["D"]);
        cache.register_dependency("D", &["E"]);
        cache.register_dependency("E", &[]);

        let tiers = cache.parallel_tiers();
        let order = cache.topo_order();

        assert_eq!(
            tiers.len(),
            5,
            "expected 5 tiers for a 5-node chain, got {:?}",
            tiers
        );
        assert_eq!(tiers[0], vec!["E".to_string()], "tier 0 must be [E]");
        assert_eq!(tiers[4], vec!["A".to_string()], "tier 4 must be [A]");

        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("E") < pos("D"), "E before D");
        assert!(pos("D") < pos("C"), "D before C");
        assert!(pos("C") < pos("B"), "C before B");
        assert!(pos("B") < pos("A"), "B before A");
    }
}
