use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::{Script, TopDecl, ChipDecl, AnonChipDecl, Handler, Stmt, Block, Expr, CallArg};
use crate::ir::{Module, NodeKind, gate_class as gc};
use crate::lower::compile_chip_template;
use crate::template_cache::TemplateCache;
use crate::typecheck::TypeCheckResult;

#[derive(Clone, Debug, Default)]
pub struct ResourceEstimate {
    pub gates: usize,
    pub wires: usize,
    pub nested_chips: usize,
    pub total_microchips: usize,
    /// True if this is an inline mod (no physical microchip per call).
    pub is_inline: bool,
}

fn is_spawnable(kind: NodeKind, gate_class: &str) -> bool {
    if gate_class == gc::LITERAL || gate_class == gc::UNSUPPORTED || gate_class.is_empty() {
        return false;
    }
    matches!(
        kind,
        NodeKind::Gate | NodeKind::Event | NodeKind::Input | NodeKind::Output | NodeKind::Coerce
    )
}

pub fn estimate_module(module: &Module) -> ResourceEstimate {
    let non_spawnable: std::collections::HashSet<_> = module
        .nodes
        .iter()
        .filter(|(_, n)| !is_spawnable(n.kind, n.gate_class))
        .map(|(id, _)| *id)
        .collect();

    let gates = module.nodes.len() - non_spawnable.len();
    let wires = module.wires.len();

    let mut est = ResourceEstimate {
        gates,
        wires,
        nested_chips: module.chips.len(),
        total_microchips: 0,
        is_inline: false,
    };

    let mut total_mc = module.chips.len();
    for child in module.chips.values() {
        let child_est = estimate_module(child);
        est.gates += child_est.gates;
        est.wires += child_est.wires;
        total_mc += child_est.total_microchips;
    }
    est.total_microchips = total_mc;
    est
}

fn estimate_key_offset(offset: usize) -> String {
    format!("@{offset}")
}

pub fn lookup_estimate<'a>(
    estimates: &'a HashMap<String, ResourceEstimate>,
    name: &str,
    offset: usize,
) -> Option<&'a ResourceEstimate> {
    if !name.is_empty() && !name.starts_with('_') {
        if let Some(est) = estimates.get(name) {
            return Some(est);
        }
    }
    estimates.get(&estimate_key_offset(offset))
}

pub fn collect_estimates(
    ast: &Script,
    tc: &TypeCheckResult,
    file: &str,
) -> HashMap<String, ResourceEstimate> {
    let cache = Arc::new(TemplateCache::new());
    let mut base_estimates: HashMap<String, ResourceEstimate> = HashMap::new();
    let mut call_graph: HashMap<String, Vec<String>> = HashMap::new();

    // Phase 1: compile templates for base estimates + build call graph
    for decl in &ast.decls {
        collect_from_decl(decl, tc, file, &cache, &mut base_estimates, &mut call_graph);
    }

    // Phase 2: expand estimates by following the call graph
    let mut expanded: HashMap<String, ResourceEstimate> = HashMap::new();
    let keys: Vec<String> = base_estimates.keys().cloned().collect();
    for key in &keys {
        let est = expand_estimate(key, &base_estimates, &call_graph, &mut expanded, &mut Vec::new());
        expanded.insert(key.clone(), est);
    }

    expanded
}

fn expand_estimate(
    key: &str,
    base: &HashMap<String, ResourceEstimate>,
    call_graph: &HashMap<String, Vec<String>>,
    cache: &mut HashMap<String, ResourceEstimate>,
    stack: &mut Vec<String>,
) -> ResourceEstimate {
    if let Some(cached) = cache.get(key) {
        return cached.clone();
    }
    if stack.contains(&key.to_string()) {
        return base.get(key).cloned().unwrap_or_default();
    }

    let mut est = base.get(key).cloned().unwrap_or_default();

    if let Some(callees) = call_graph.get(key) {
        stack.push(key.to_string());
        for callee in callees {
            let callee_est = expand_estimate(callee, base, call_graph, cache, stack);
            est.gates += callee_est.gates;
            est.wires += callee_est.wires;
            est.total_microchips += callee_est.total_microchips;
            if !callee_est.is_inline {
                // Non-inline chip: each call creates a microchip
                est.total_microchips += 1;
            }
        }
        stack.pop();
    }

    cache.insert(key.to_string(), est.clone());
    est
}

fn collect_from_decl(
    decl: &TopDecl,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
    estimates: &mut HashMap<String, ResourceEstimate>,
    call_graph: &mut HashMap<String, Vec<String>>,
) {
    match decl {
        TopDecl::Chip(chip) => {
            let key = chip_key(chip);
            estimate_chip(chip, tc, file, cache, estimates);
            let calls = collect_calls_in_block(&chip.body);
            if !calls.is_empty() {
                call_graph.insert(key, calls);
            }
            collect_from_block(&chip.body, tc, file, cache, estimates, call_graph);
        }
        TopDecl::AnonChip(ac) => {
            let key = estimate_key_offset(ac.range.start.offset);
            estimate_anon_chip(ac, tc, file, cache, estimates);
            let calls = collect_calls_in_block(&ac.body);
            if !calls.is_empty() {
                call_graph.insert(key, calls);
            }
            collect_from_block(&ac.body, tc, file, cache, estimates, call_graph);
        }
        TopDecl::Handler(h) => {
            estimate_handler(h, tc, file, cache, estimates, call_graph);
        }
        TopDecl::Namespace(ns) => {
            for d in &ns.decls {
                collect_from_decl(d, tc, file, cache, estimates, call_graph);
            }
        }
        _ => {}
    }
}

fn collect_from_block(
    block: &Block,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
    estimates: &mut HashMap<String, ResourceEstimate>,
    call_graph: &mut HashMap<String, Vec<String>>,
) {
    for s in &block.stmts {
        match s {
            Stmt::ChipDecl(chip) => {
                let key = chip_key(chip);
                estimate_chip(chip, tc, file, cache, estimates);
                let calls = collect_calls_in_block(&chip.body);
                if !calls.is_empty() {
                    call_graph.insert(key, calls);
                }
                collect_from_block(&chip.body, tc, file, cache, estimates, call_graph);
            }
            Stmt::AnonChip(ac) => {
                let key = estimate_key_offset(ac.range.start.offset);
                estimate_anon_chip(ac, tc, file, cache, estimates);
                let calls = collect_calls_in_block(&ac.body);
                if !calls.is_empty() {
                    call_graph.insert(key, calls);
                }
                collect_from_block(&ac.body, tc, file, cache, estimates, call_graph);
            }
            Stmt::Handler(h) => {
                estimate_handler(h, tc, file, cache, estimates, call_graph);
            }
            Stmt::If(i) => {
                let key = estimate_key_offset(i.range.start.offset);
                let then_gates = count_gates_in_block(&i.then_block);
                let else_gates = i.else_block.as_ref().map_or(0, count_gates_in_block);
                let then_calls = collect_calls_in_block(&i.then_block);
                let else_calls = i.else_block.as_ref().map(collect_calls_in_block).unwrap_or_default();
                let mut all_calls = then_calls;
                all_calls.extend(else_calls);
                estimates.insert(key.clone(), ResourceEstimate {
                    gates: 1 + then_gates + else_gates,
                    is_inline: true,
                    ..Default::default()
                });
                if !all_calls.is_empty() {
                    call_graph.insert(key, all_calls);
                }
                collect_from_block(&i.then_block, tc, file, cache, estimates, call_graph);
                if let Some(ref else_block) = i.else_block {
                    collect_from_block(else_block, tc, file, cache, estimates, call_graph);
                }
            }
            _ => {}
        }
    }
}

fn chip_key(chip: &ChipDecl) -> String {
    if !chip.name.is_empty() {
        chip.name.clone()
    } else {
        estimate_key_offset(chip.range.start.offset)
    }
}

fn estimate_handler(
    h: &Handler,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
    estimates: &mut HashMap<String, ResourceEstimate>,
    call_graph: &mut HashMap<String, Vec<String>>,
) {
    let key = estimate_key_offset(h.range.start.offset);
    if !estimates.contains_key(&key) {
        let synthetic = ChipDecl {
            name: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            body: h.body.clone(),
            range: h.range.clone(),
            inline: false,
        };
        let template_est = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compile_chip_template(&synthetic, tc, file, cache)
        }))
        .ok()
        .map(|m| estimate_module(&m))
        .unwrap_or_default();

        let est = best_estimate(template_est, &h.body);
        if est.gates > 0 {
            estimates.insert(key.clone(), est);
        }
    }
    let calls = collect_calls_in_block(&h.body);
    if !calls.is_empty() {
        call_graph.insert(key, calls);
    }
    collect_from_block(&h.body, tc, file, cache, estimates, call_graph);
}

// Walk a block's AST to find all call sites to user-defined chips/mods.
// Returns one entry per call (duplicates = multiple calls).
fn collect_calls_in_block(block: &Block) -> Vec<String> {
    let mut calls = Vec::new();
    for s in &block.stmts {
        collect_calls_in_stmt(s, &mut calls);
    }
    calls
}

fn collect_calls_in_stmt(stmt: &Stmt, calls: &mut Vec<String>) {
    match stmt {
        Stmt::ExprStmt(es) => collect_calls_in_expr(&es.expr, calls),
        Stmt::Let(l) => collect_calls_in_expr(&l.value, calls),
        Stmt::Var(v) => {
            if let Some(ref init) = v.init {
                collect_calls_in_expr(init, calls);
            }
        }
        Stmt::Assign(a) => collect_calls_in_expr(&a.value, calls),
        Stmt::If(i) => {
            collect_calls_in_expr(&i.cond, calls);
            for s in &i.then_block.stmts {
                collect_calls_in_stmt(s, calls);
            }
            if let Some(ref else_block) = i.else_block {
                for s in &else_block.stmts {
                    collect_calls_in_stmt(s, calls);
                }
            }
        }
        Stmt::Handler(h) => {
            for s in &h.body.stmts {
                collect_calls_in_stmt(s, calls);
            }
        }
        Stmt::Return { value, .. } => {
            if let Some(v) = value {
                collect_calls_in_expr(v, calls);
            }
        }
        Stmt::AnonChip(ac) => {
            for s in &ac.body.stmts {
                collect_calls_in_stmt(s, calls);
            }
        }
        Stmt::ChipDecl(c) => {
            for s in &c.body.stmts {
                collect_calls_in_stmt(s, calls);
            }
        }
        _ => {}
    }
}

fn collect_calls_in_expr(expr: &Expr, calls: &mut Vec<String>) {
    match expr {
        Expr::Call { callee, args, .. } => {
            if let Expr::Ident { name, .. } = callee.as_ref() {
                calls.push(name.clone());
            }
            for arg in args {
                match arg {
                    CallArg::Positional(v) | CallArg::Spread(v) => collect_calls_in_expr(v, calls),
                    CallArg::Named { value, .. } => collect_calls_in_expr(value, calls),
                }
            }
        }
        Expr::BinOp { left, right, .. } => {
            collect_calls_in_expr(left, calls);
            collect_calls_in_expr(right, calls);
        }
        Expr::UnOp { operand, .. } => collect_calls_in_expr(operand, calls),
        Expr::FieldAccess { obj, .. } => collect_calls_in_expr(obj, calls),
        Expr::IndexAccess { obj, index, .. } => {
            collect_calls_in_expr(obj, calls);
            collect_calls_in_expr(index, calls);
        }
        Expr::IfExpr { cond, then_branch, else_branch, .. } => {
            collect_calls_in_expr(cond, calls);
            collect_calls_in_expr(then_branch, calls);
            collect_calls_in_expr(else_branch, calls);
        }
        Expr::BlockExpr { stmts, value, .. } => {
            for s in stmts {
                collect_calls_in_stmt(s, calls);
            }
            collect_calls_in_expr(value, calls);
        }
        _ => {}
    }
}

fn count_gates_in_block(block: &Block) -> usize {
    block.stmts.iter().map(count_gates_in_stmt).sum()
}

fn count_gates_in_stmt(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::ExprStmt(es) => count_gates_in_expr(&es.expr),
        Stmt::Let(l) => count_gates_in_expr(&l.value),
        Stmt::Var(v) => 1 + v.init.as_ref().map_or(0, count_gates_in_expr),
        Stmt::Assign(a) => 1 + count_gates_in_expr(&a.value),
        Stmt::Emit(e) => 1 + e.value.as_ref().map_or(0, count_gates_in_expr),
        Stmt::If(i) => {
            1 + count_gates_in_expr(&i.cond)
                + count_gates_in_block(&i.then_block)
                + i.else_block.as_ref().map_or(0, count_gates_in_block)
        }
        Stmt::Handler(h) => count_gates_in_block(&h.body),
        Stmt::AnonChip(ac) => count_gates_in_block(&ac.body),
        Stmt::ChipDecl(c) => count_gates_in_block(&c.body),
        Stmt::Return { value, .. } => 1 + value.as_ref().map_or(0, count_gates_in_expr),
        // await: PseudoVar + Var_Set(arm) + Var_Get + Branch + Var_Set(reset) + 2 literals
        Stmt::Await(a) => {
            7 + count_gates_in_expr(&a.exec_expr)
                + a.value_expr.as_ref().map_or(0, count_gates_in_expr)
        }
        _ => 0,
    }
}

fn count_gates_in_expr(expr: &Expr) -> usize {
    match expr {
        Expr::Call { callee, args, .. } => {
            // User-defined mod/chip calls (plain ident not in builtin catalog)
            // are handled by the call graph — don't count the call itself.
            // Builtins and method calls each produce ~1 gate.
            let self_cost = match callee.as_ref() {
                Expr::Ident { name, .. } => {
                    if crate::catalog::calls::calls().contains_key(name.as_str()) {
                        1
                    } else {
                        0
                    }
                }
                _ => 1,
            };
            self_cost + args.iter().map(|a| match a {
                CallArg::Positional(v) | CallArg::Spread(v) => count_gates_in_expr(v),
                CallArg::Named { value, .. } => count_gates_in_expr(value),
            }).sum::<usize>()
        }
        Expr::BinOp { left, right, .. } => 1 + count_gates_in_expr(left) + count_gates_in_expr(right),
        Expr::UnOp { operand, .. } => 1 + count_gates_in_expr(operand),
        Expr::IfExpr { cond, then_branch, else_branch, .. } => {
            1 + count_gates_in_expr(cond) + count_gates_in_expr(then_branch) + count_gates_in_expr(else_branch)
        }
        Expr::IndexAccess { obj, index, .. } => 1 + count_gates_in_expr(obj) + count_gates_in_expr(index),
        Expr::FieldAccess { obj, .. } => count_gates_in_expr(obj),
        Expr::BlockExpr { stmts, value, .. } => {
            stmts.iter().map(count_gates_in_stmt).sum::<usize>() + count_gates_in_expr(value)
        }
        // Variable reads in exec context produce a Var_Get gate
        Expr::Ident { .. } => 1,
        _ => 0,
    }
}

fn best_estimate(template_est: ResourceEstimate, body: &Block) -> ResourceEstimate {
    let heuristic_gates = count_gates_in_block(body);
    if heuristic_gates > template_est.gates {
        ResourceEstimate { gates: heuristic_gates, ..template_est }
    } else {
        template_est
    }
}

fn estimate_chip(
    chip: &ChipDecl,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
    estimates: &mut HashMap<String, ResourceEstimate>,
) {
    let key = chip_key(chip);
    if estimates.contains_key(&key) {
        return;
    }
    let template_est = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile_chip_template(chip, tc, file, cache)
    }))
    .ok()
    .map(|m| estimate_module(&m))
    .unwrap_or_default();

    let mut est = best_estimate(template_est, &chip.body);
    est.is_inline = chip.inline;
    if est.gates > 0 {
        estimates.insert(key, est);
    }
}

fn estimate_anon_chip(
    ac: &AnonChipDecl,
    tc: &TypeCheckResult,
    file: &str,
    cache: &Arc<TemplateCache>,
    estimates: &mut HashMap<String, ResourceEstimate>,
) {
    let key = estimate_key_offset(ac.range.start.offset);
    if estimates.contains_key(&key) {
        return;
    }
    let synthetic = ChipDecl {
        name: String::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        body: ac.body.clone(),
        range: ac.range.clone(),
        inline: false,
    };
    let template_est = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compile_chip_template(&synthetic, tc, file, cache)
    }))
    .ok()
    .map(|m| estimate_module(&m))
    .unwrap_or_default();

    let est = best_estimate(template_est, &ac.body);
    if est.gates > 0 {
        estimates.insert(key, est);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{resolve, FsLoader};
    use crate::typecheck::typecheck;

    fn estimates_for(source: &str) -> HashMap<String, ResourceEstimate> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test");
        collect_estimates(&resolved.ast, &tc, "test")
    }

    #[test]
    fn basic_chip_has_gates_and_wires() {
        let est = estimates_for("chip Add(a: int, b: int) -> (r: int) { out r = a + b }");
        let add = est.get("Add").expect("should have Add estimate");
        assert!(add.gates > 0, "chip should have gates, got {}", add.gates);
        assert!(add.wires > 0, "chip should have wires, got {}", add.wires);
    }

    #[test]
    fn mod_has_gates() {
        let est = estimates_for("mod inc(v: *int) { v = v + 1 }");
        let inc = est.get("inc").expect("should have inc estimate");
        assert!(inc.gates > 0, "mod should have gates, got {}", inc.gates);
    }

    #[test]
    fn chip_calling_mod_includes_mod_gates() {
        let src = "\
mod double(v: *int) { v = v + v }
chip Wrap(a: *int) -> () {
  in run: exec
  on run { double(a); double(a) }
}";
        let est = estimates_for(src);
        let double = est.get("double").expect("should have double");
        let wrap = est.get("Wrap").expect("should have Wrap");
        // Wrap calls double 2x, so its gates should be > double's base
        assert!(
            wrap.gates > double.gates,
            "Wrap ({}) should include double ({}) gates",
            wrap.gates,
            double.gates
        );
    }
}
