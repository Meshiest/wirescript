//! A `mod`/`chip` body resolves its free names where it is DECLARED, never at
//! the call site: the caller's locals, params and nested declarations are
//! invisible to it, whatever they are called.

use super::*;
use crate::collections::HashMap;
use crate::ir::gate_class as gc;
use crate::ir::{Node, NodeId, Wire};

/// Every node and wire in the result, chip bodies included. Node ids are
/// unique across modules, and a wire may sit in a parent while its target is
/// a chip's pin.
fn flat(r: &LowerResult) -> (HashMap<NodeId, &Node>, Vec<Wire>) {
    fn walk<'a>(m: &'a crate::ir::Module, nodes: &mut HashMap<NodeId, &'a Node>, wires: &mut Vec<Wire>) {
        nodes.extend(m.nodes.iter().map(|(id, n)| (*id, n)));
        wires.extend(m.wires.iter().copied());
        for c in m.chips.values() {
            walk(c, nodes, wires);
        }
    }
    let mut nodes = HashMap::default();
    let mut wires = Vec::new();
    walk(&r.module, &mut nodes, &mut wires);
    (nodes, wires)
}

/// The `Var` gate declared on source line `line` (1-based).
fn var_on_line(r: &LowerResult, line: u32) -> NodeId {
    let (nodes, _) = flat(r);
    let found: Vec<NodeId> = nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::VAR && n.source_range.start.line == line)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(found.len(), 1, "expected one var declared on line {line}, found {found:?}");
    found[0]
}

/// The gates that reach `var` through their `VarRef` port, following chip
/// pins and rerouters.
fn var_user_nodes(nodes: &HashMap<NodeId, &Node>, wires: &[Wire], var: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![var];
    let mut seen = crate::collections::HashSet::default();
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        for w in wires.iter().filter(|w| w.source.node_id == at) {
            let Some(n) = nodes.get(&w.target.node_id) else { continue };
            if matches!(n.gate_class, gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT | gc::REROUTER) {
                stack.push(w.target.node_id);
            } else if w.target.port == WirePort::VarRef {
                out.push(w.target.node_id);
            }
        }
    }
    out
}

/// The classes of the gates that reach `var` through their `VarRef` port.
fn var_users(r: &LowerResult, var: NodeId) -> Vec<&'static str> {
    let (nodes, wires) = flat(r);
    var_user_nodes(&nodes, &wires, var).into_iter().map(|id| nodes[&id].gate_class).collect()
}

/// Lowering binds every name in `src` to the declaration typecheck resolved
/// it to: an assignment to a `var` writes that var's own gate and no other,
/// no read or write reaches a same-named var it did not resolve to, and no
/// read falls back to a placeholder or to another module's constant.
fn assert_agrees(src: &str) {
    assert_agrees_multi(src, &[]);
}

/// [`assert_agrees`] for `entry` importing the in-memory `deps`, each
/// `(name, source)` loaded as `name.ws`.
fn assert_agrees_multi(entry: &str, deps: &[(&str, &str)]) {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: deps.iter().map(|(k, v)| (format!("{k}.ws"), v.to_string())).collect(),
    };
    let resolved = resolve(entry, "test", &loader);
    assert!(resolved.diagnostics.is_empty(), "resolve diags: {:?}", resolved.diagnostics);
    let (tc, ce_slots) = typecheck_with_inference(&resolved.ast, "test");
    let mut r = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        resolutions: &tc.resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::ForceOff,
        ce_slots: &ce_slots,
    });
    r.diagnostics.extend(tc.diagnostics);
    let idents = tc.ident_decls;
    let mut sources: HashMap<String, &str> = HashMap::default();
    sources.insert("test".to_string(), entry);
    for (k, v) in deps {
        sources.insert(format!("{k}.ws"), v);
    }
    let text = |at: &SourceRange| -> &str {
        sources.get(&*at.file).and_then(|s| s.get(at.start.offset..at.end.offset)).unwrap_or("")
    };
    let src = entry;

    assert_no_errors(&r);
    let (nodes, wires) = flat(&r);
    let vars: Vec<NodeId> = nodes.iter().filter(|(_, n)| n.gate_class == gc::VAR).map(|(id, _)| *id).collect();
    let contains = |outer: &SourceRange, inner: &SourceRange| {
        outer.file == inner.file && outer.start.offset <= inner.start.offset && inner.end.offset <= outer.end.offset
    };
    let mut checked = 0;
    for (target, decl, _) in idents.iter().filter(|e| e.2) {
        let declared: Vec<NodeId> = vars.iter().copied().filter(|v| nodes[v].source_range == *decl).collect();
        if declared.is_empty() {
            continue;
        }
        let written: Vec<NodeId> = vars
            .iter()
            .copied()
            .filter(|v| {
                var_user_nodes(&nodes, &wires, *v).iter().any(|g| {
                    matches!(nodes[g].gate_class, gc::VAR_SET | gc::VAR_INCREMENT)
                        && contains(&nodes[g].source_range, target)
                })
            })
            .collect();
        assert!(!written.is_empty(), "the write to {target:?} lowered to nothing in\n{src}");
        assert!(
            written.iter().all(|v| declared.contains(v)),
            "the write to {target:?} reached a var typecheck did not resolve it to in\n{src}"
        );
        checked += 1;
    }
    assert!(checked > 0, "no assignment to a var was checked in\n{src}");

    // Every identifier, read or written: no gate lowered for it reaches a
    // same-named `var` other than the declaration typecheck resolved it to.
    let var_name = |v: &NodeId| {
        text(&nodes[v].source_range)
            .strip_prefix("var ")
            .map(|t| t.split(|c: char| c == ':' || c.is_whitespace()).next().unwrap_or("").to_string())
    };
    for (at, decl, _) in &idents {
        let name = text(at);
        for v in vars.iter().filter(|v| var_name(v).as_deref() == Some(name) && nodes[*v].source_range != *decl) {
            assert!(
                !var_user_nodes(&nodes, &wires, *v).iter().any(|g| contains(&nodes[g].source_range, at)),
                "`{name}` at {at:?} reached a var typecheck did not resolve it to in\n{src}"
            );
        }
    }
    // No var is written by a declaration of another var: a declaration's
    // initializer writes only its own gate.
    for v in &vars {
        for g in var_user_nodes(&nodes, &wires, *v) {
            let gate = &nodes[&g];
            if gate.gate_class == gc::VAR_SET
                && text(&gate.source_range).starts_with("var ")
                && gate.source_range != nodes[v].source_range
            {
                panic!("the declaration at {:?} writes another var in\n{src}", gate.source_range);
            }
        }
    }
    assert!(!nodes.values().any(|n| n.gate_class == "_Unsupported"), "a placeholder in\n{src}");
    assert_no_cross_module_literal(&r);
}

/// Whether the var declared on `line` is written (set or incremented).
fn written(r: &LowerResult, line: u32) -> bool {
    var_users(r, var_on_line(r, line))
        .iter()
        .any(|c| *c == gc::VAR_SET || *c == gc::VAR_INCREMENT)
}

/// The value baked into the `Var_Set` that writes the var on `line`, when
/// it is a constant rather than a wire.
fn baked_write(r: &LowerResult, line: u32) -> Option<crate::ir::Literal> {
    let (nodes, wires) = flat(r);
    let var = var_on_line(r, line);
    wires
        .iter()
        .filter(|w| w.source.node_id == var && w.target.port == WirePort::VarRef)
        .filter_map(|w| nodes.get(&w.target.node_id))
        .find(|n| n.gate_class == gc::VAR_SET)
        .and_then(|n| n.properties.get(&crate::intern::sym::VALUE).cloned())
}

#[test]
fn a_caller_let_does_not_capture_a_mod_write() {
    let r = compile(
        "var q: int = 0\nvar z: int = 0\nmod bump() {\n  q = q + 1\n}\nin go: exec\n\
         on go {\n  let q = 7\n  bump()\n  z = q\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "bump must write the top-level q");
    assert_eq!(baked_write(&r, 2), Some(crate::ir::Literal::Int(7)), "z reads the caller's own q");
}

#[test]
fn a_caller_var_does_not_capture_a_mod_write() {
    let r = compile(
        "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\n\
         on go {\n  var q: int = 5\n  bump()\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "bump must write the top-level q");
    let local = var_on_line(&r, 5);
    assert!(
        !var_users(&r, local).contains(&gc::VAR_INCREMENT),
        "the caller's var q is not the mod's q"
    );
}

#[test]
fn a_caller_param_does_not_capture_a_mod_write() {
    let r = compile(
        "var q: int = 0\nmod bump() { q = q + 1 }\nmod caller(q: int) -> int {\n  bump()\n  return q\n}\n\
         var z: int = 0\nin go: exec\non go {\n  z = caller(3)\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "bump must write the top-level q, not caller's param");
}

#[test]
fn a_generic_mod_body_resolves_where_it_is_declared() {
    let r = compile(
        "var q: int = 0\nmod bump<T>(x: T) { q = q + 1 }\nvar z: int = 0\nin go: exec\n\
         on go {\n  let q = 7\n  bump(1)\n  z = q\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "the generic body must write the top-level q");
}

#[test]
fn a_nested_mod_reads_its_enclosing_mods_local() {
    let r = compile(
        "var z: int = 0\nmod outer() {\n  var a: int = 1\n  mod f() { a = a + 1 }\n\
           if z > 0 {\n    let a = 5\n    f()\n    z = a\n  }\n}\nin go: exec\non go { outer() }",
    );
    assert_no_errors(&r);
    assert!(written(&r, 3), "f must increment outer's var a");
    assert!(
        var_users(&r, var_on_line(&r, 3)).contains(&gc::VAR_INCREMENT),
        "the write is f's increment"
    );
}

#[test]
fn a_nested_mod_in_a_handler_reads_the_handler_local() {
    let r = compile(
        "var z: int = 0\nin go: exec\non go {\n  var a: int = 1\n  mod f() { a = a + 1 }\n\
           if z > 0 {\n    let a = 5\n    f()\n    z = a\n  }\n}",
    );
    assert_no_errors(&r);
    assert!(
        var_users(&r, var_on_line(&r, 4)).contains(&gc::VAR_INCREMENT),
        "f must increment the handler's var a"
    );
}

#[test]
fn a_mod_calls_the_callee_its_own_scope_names() {
    // The caller declares its own `helper`; `bump`'s `helper()` is the
    // top-level one, which writes q.
    let r = compile(
        "var q: int = 0\nvar k: int = 0\nmod helper() { q = q + 1 }\nmod bump() { helper() }\nin go: exec\n\
         on go {\n  mod helper() { k = k + 1 }\n  bump()\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "bump's helper is the top-level one");
    assert!(!written(&r, 2), "the caller's nested helper never runs");
}

#[test]
fn a_named_import_mod_keeps_its_module_var_under_a_caller_local() {
    let lib = "var q: int = 0\nmod bump() { q = q + 1 }";
    let r = compile_multi(
        "import { bump } from \"lib\"\nvar z: int = 0\nin go: exec\n\
         on go {\n  let q = 7\n  bump()\n  z = q\n}",
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    let (nodes, _) = flat(&r);
    let lib_q = nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::VAR && &*n.source_range.file == "lib.ws")
        .map(|(id, _)| *id)
        .expect("lib's q");
    assert!(var_users(&r, lib_q).contains(&gc::VAR_INCREMENT), "bump writes lib's q");
}

#[test]
fn a_namespaced_mod_keeps_its_module_var_under_a_caller_local() {
    let lib = "var q: int = 0\nmod helper() -> int {\n  q = q + 1\n  return 5\n}\nmod bump2() -> int {\n  return helper()\n}";
    let r = compile_multi(
        "import * as L from \"lib\"\nvar o: int = 0\nin go: exec\n\
         on go {\n  let q = 7\n  mod helper() -> int { return 9 }\n  o = L.bump2()\n}",
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    let (nodes, _) = flat(&r);
    let lib_q = nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::VAR && &*n.source_range.file == "lib.ws")
        .map(|(id, _)| *id)
        .expect("lib's q");
    assert!(var_users(&r, lib_q).contains(&gc::VAR_INCREMENT), "lib's helper writes lib's q");
}

#[test]
fn a_named_import_mod_calls_its_own_modules_helper() {
    let lib = "var q: int = 0\nmod helper() -> int {\n  q = q + 1\n  return 5\n}\nmod bump2() -> int {\n  return helper()\n}";
    let r = compile_multi(
        "import { bump2 } from \"lib\"\nvar o: int = 0\nin go: exec\n\
         on go {\n  mod helper() -> int { return 9 }\n  o = bump2()\n}",
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    let (nodes, _) = flat(&r);
    let lib_q = nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::VAR && &*n.source_range.file == "lib.ws")
        .map(|(id, _)| *id)
        .expect("lib's q");
    assert!(var_users(&r, lib_q).contains(&gc::VAR_INCREMENT), "bump2 calls lib's helper");
}

#[test]
fn a_chip_body_resolves_where_it_is_declared() {
    let r = compile(
        "var q: int = 0\nvar z: int = 0\nchip bump() {\n  q = q + 1\n}\nin go: exec\n\
         on go {\n  let q = 7\n  bump()\n  z = q\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "the chip body writes the top-level q");
}

#[test]
fn a_chip_param_does_not_capture_a_mod_called_in_its_body() {
    let r = compile(
        "var q: int = 0\nmod bump() { q = q + 1 }\nchip C(q: int) {\n  on go { bump() }\n}\nin go: exec\nC(4)",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "bump inside C still writes the top-level q");
}

#[test]
fn a_name_declared_after_a_nested_mod_is_not_its_name() {
    // Typecheck checks `f` where it is written, before the local `a` exists,
    // so `f`'s `a` is the top-level var.
    let r = compile(
        "var a: int = 0\nvar z: int = 0\nin go: exec\non go {\n  mod f() { a = a + 1 }\n\
           let a = 5\n  f()\n  z = a\n}",
    );
    assert_no_errors(&r);
    assert!(written(&r, 1), "f writes the top-level a");
}

#[test]
fn a_same_scope_redeclaration_after_a_nested_mod_is_not_its_name() {
    let r = compile(
        "var z: int = 0\nin go: exec\non go {\n  var b: int = 1\n  mod f() { b = b + 1 }\n\
           let b = 5\n  f()\n  z = b\n}",
    );
    assert_no_errors(&r);
    assert!(
        var_users(&r, var_on_line(&r, 4)).contains(&gc::VAR_INCREMENT),
        "f writes the var b declared before it"
    );
}

#[test]
fn a_mod_body_var_declared_after_a_nested_mod_is_not_its_name() {
    let r = compile(
        "var a: int = 0\nvar z: int = 0\nmod outer() {\n  mod f() { a = a + 1 }\n  var a: int = 5\n\
           f()\n  z = a\n}\nin go: exec\non go { outer() }",
    );
    assert_no_errors(&r);
    assert!(
        var_users(&r, var_on_line(&r, 1)).contains(&gc::VAR_INCREMENT),
        "f writes the top-level a"
    );
    assert!(
        !var_users(&r, var_on_line(&r, 5)).contains(&gc::VAR_INCREMENT),
        "outer's own a is declared after f"
    );
}

#[test]
fn typecheck_and_lowering_bind_a_body_name_to_the_same_declaration() {
    for src in [
        "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  let q = 7\n  bump()\n}",
        "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  var q: int = 5\n  bump()\n  q = 2\n}",
        "var q: int = 0\nmod bump() { q = q + 1 }\nmod caller(q: int) -> int {\n  bump()\n  return q\n}\n\
         var z: int = 0\nin go: exec\non go { z = caller(3) }",
        "var q: int = 0\nmod bump<T>(x: T) { q = q + 1 }\nin go: exec\non go {\n  let q = 7\n  bump(1)\n}",
        "var z: int = 0\nmod outer() {\n  var a: int = 1\n  mod f() { a = a + 1 }\n\
           if z > 0 {\n    let a = 5\n    f()\n    z = a\n  }\n}\nin go: exec\non go { outer() }",
        "var q: int = 0\nvar k: int = 0\nmod helper() { q = q + 1 }\nmod bump() { helper() }\nin go: exec\n\
         on go {\n  mod helper() { k = k + 1 }\n  bump()\n  helper()\n}",
        "var q: int = 0\nchip bump() { q = q + 1 }\nin go: exec\non go {\n  let q = 7\n  bump()\n}",
        "var q: int = 0\nmod bump() { q = q + 1 }\nchip C(q: int) {\n  on go { bump() }\n}\nin go: exec\nC(4)",
        "var a: int = 0\nin go: exec\non go {\n  mod f() { a = a + 1 }\n  let a = 5\n  f()\n}",
        "var a: int = 0\nmod outer() {\n  mod f() { a = a + 1 }\n  var a: int = 5\n  f()\n  a = 3\n}\n\
         in go: exec\non go { outer() }",
        "var z: int = 0\nin go: exec\non go {\n  let k = 12345\n  mod inc() { z = z + k }\n  chip C() { inc() }\n  C()\n}",
        "var z: int = 0\nin P: int\nmod outer(a: int) {\n  mod inner() { z = a }\n  var a: int = 5\n  inner()\n}\n\
         in go: exec\non go { outer(P) }",
        "var z: int = 0\nin P: int\nmod outer(a: int) {\n  z = a\n  var a: int = 5\n  z = a\n}\n\
         in go: exec\non go { outer(P) }",
        "var a: string = \"s\"\nvar z: int = 0\nin c: bool\nmod outer() {\n  if c {\n    mod inner() { a = a .. \"x\" }\n\
             inner()\n  }\n  var a: int = 5\n  z = a\n}\nin go: exec\non go { outer() }",
        "var a: int = 0\nvar z: int = 0\nin c: bool\nchip C() {\n  on go {\n    if c {\n      mod inner() { a = a + z }\n\
                 inner()\n    }\n  }\n  var a: int = 5\n}\nin go: exec\nC()",
        "var g: int = 7\nvar z: int = 0\nmod f(a: *int) {\n  var a: int = 12345\n  z = a\n}\nin go: exec\non go { f(g) }",
        "var g: int = 7\nvar z: int = 0\nin go: exec\nchip C(a: *int) {\n  var a: int = 12345\n  on go { z = a }\n}\nC(g)",
    ] {
        assert_agrees(src);
    }
    assert_agrees_multi(
        "import * as L from \"lib\"\nvar z: int = 0\nin go: exec\non go { z = L.f() }",
        &[("lib", "var count: int = 7\nmod f() -> int {\n  var count: int = 12345\n  count = count + 1\n  return count\n}")],
    );
}

#[test]
fn a_chip_declared_in_a_mod_captures_each_expansions_own_var() {
    // Each expansion of `M` has its own `a`; the chip instance each one
    // makes increments that expansion's, not the first one's.
    let r = compile(
        "in go: exec\nmod M() {\n  var a: int = 0\n  chip C() { a = a + 1 }\n  C()\n}\non go {\n  M()\n  M()\n}",
    );
    assert_no_errors(&r);
    let (nodes, wires) = flat(&r);
    let vars: Vec<NodeId> = nodes.iter().filter(|(_, n)| n.gate_class == gc::VAR).map(|(id, _)| *id).collect();
    assert_eq!(vars.len(), 2, "one `a` per expansion");
    for v in vars {
        let increments = var_user_nodes(&nodes, &wires, v)
            .into_iter()
            .filter(|g| nodes[g].gate_class == gc::VAR_INCREMENT)
            .count();
        assert_eq!(increments, 1, "each expansion's `a` is incremented by its own chip");
    }
}

#[test]
fn an_exec_trigger_argument_is_the_callers_name() {
    // The mod's param `go` must not shadow the caller's trigger.
    let r = compile("var q: int = 0\nin go: exec\nmod f(go: int) { q = q + go }\nf(2, exec = go)");
    assert_no_errors(&r);
    let (nodes, wires) = flat(&r);
    let inc = nodes.iter().find(|(_, n)| n.gate_class == gc::VAR_INCREMENT).map(|(id, _)| *id).expect("increment");
    let trigger = wires
        .iter()
        .find(|w| w.target.node_id == inc && w.target.port == WirePort::Exec)
        .map(|w| nodes[&w.source.node_id].gate_class);
    assert_eq!(trigger, Some(gc::MICROCHIP_INPUT), "the increment runs off the `in go` port");
}

/// No wire reads a `_Literal` that lives in another module: emit inlines a
/// literal only into its own module, so such a wire drops the value.
fn assert_no_cross_module_literal(r: &LowerResult) {
    let (nodes, _) = flat(r);
    fn walk(m: &crate::ir::Module, nodes: &HashMap<NodeId, &Node>) {
        for w in &m.wires {
            if !m.nodes.contains_key(&w.source.node_id)
                && nodes.get(&w.source.node_id).is_some_and(|n| n.gate_class == gc::LITERAL)
            {
                panic!("wire from another module's literal {:?} into {:?}", w.source, w.target);
            }
        }
        for c in m.chips.values() {
            walk(c, nodes);
        }
    }
    walk(&r.module, &nodes);
}

/// Whether some gate of `class` takes the constant `value`, baked into its
/// data or wired from a literal in its own module.
fn takes_constant(r: &LowerResult, class: &str, value: i64) -> bool {
    fn walk(m: &crate::ir::Module, class: &str, value: i64) -> bool {
        let want = crate::ir::Literal::Int(value);
        let found = m.nodes.values().filter(|n| n.gate_class == class).any(|g| {
            g.properties.values().any(|p| *p == want)
                || m.wires.iter().any(|w| {
                    w.target.node_id == g.id
                        && m.nodes.get(&w.source.node_id).is_some_and(|s| {
                            s.gate_class == gc::LITERAL && s.properties.get(&crate::intern::sym::VALUE) == Some(&want)
                        })
                })
        });
        found || m.chips.values().any(|c| walk(c, class, value))
    }
    walk(&r.module, class, value)
}

#[test]
fn a_constant_read_by_a_mod_called_from_a_chip_reaches_the_chip() {
    for src in [
        "var z: int = 0\nin go: exec\non go {\n  let k = 12345\n  mod inc() { z = z + k }\n  chip C() { inc() }\n  C()\n}",
        "var z: int = 0\nmod outer(p: int) {\n  mod inc() { z = z + p }\n  chip C() { on go { inc() } }\n  C()\n}\n\
         in go: exec\nouter(12345)",
        "var z: int = 0\nin go: exec\nchip Top() {\n  let k = 12345\n  mod inc() { z = z + k }\n  chip C() { on go { inc() } }\n  C()\n}\nTop()",
        "var z: int = 0\nin go: exec\non go {\n  let k = 12345\n  mod inc() { z = z + k }\n  if z > 0 {\n    let k = 7\n    chip C() { inc() }\n    C()\n  }\n}",
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        assert_no_cross_module_literal(&r);
        assert!(takes_constant(&r, gc::VAR_INCREMENT, 12345), "12345 never reaches the increment in\n{src}");
    }
}

#[test]
fn a_param_redeclared_after_a_nested_mod_is_the_nested_mods_name() {
    for src in [
        "var z: int = 0\nin P: int\nmod outer(a: int) {\n  mod inner() { z = a }\n  var a: int = 5\n  inner()\n}\n\
         in go: exec\non go { outer(P) }",
        "var z: int = 0\nin P: int\nchip C(a: int) {\n  mod inner() { z = a }\n  var a: int = 5\n  on go { inner() }\n}\n\
         in go: exec\nC(P)",
        "var z: int = 0\nin P: int\nmod outer(a: int) {\n  z = a\n  var a: int = 5\n}\nin go: exec\non go { outer(P) }",
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        assert!(
            !has_gate_class_anywhere(&r, "_Unsupported"),
            "the param read lowered to a placeholder in\n{src}"
        );
        let (nodes, wires) = flat(&r);
        let z = var_on_line(&r, 1);
        let set = var_user_nodes(&nodes, &wires, z)
            .into_iter()
            .find(|g| nodes[g].gate_class == gc::VAR_SET)
            .expect("z is set");
        let src_class = wires
            .iter()
            .find(|w| w.target.node_id == set && w.target.port == WirePort::Value)
            .map(|w| nodes[&w.source.node_id].gate_class);
        assert_eq!(src_class, Some(gc::MICROCHIP_INPUT), "z is set from the param, not the var, in\n{src}");
    }
}

fn has_gate_class_anywhere(r: &LowerResult, class: &str) -> bool {
    flat(r).0.values().any(|n| n.gate_class == class)
}

#[test]
fn a_name_an_enclosing_body_declares_later_is_not_a_nested_mods_name() {
    for (src, top_line) in [
        (
            "var a: int = 0\nvar z: int = 0\nin c: bool\nmod outer() {\n  if c {\n    mod inner() { a = a + 1 }\n    inner()\n  }\n\
               var a: int = 5\n  z = a\n}\nin go: exec\non go { outer() }",
            1,
        ),
        (
            "var a: int = 0\nvar z: int = 0\nin c: bool\nchip C() {\n  on go {\n    if c {\n      mod inner() { a = a + 1 }\n\
                   inner()\n    }\n  }\n  var a: int = 5\n}\nin go: exec\nC()",
            1,
        ),
        (
            "var a: int = 0\nchip C() {\n  on go { mod inner() { a = a + 1 }  inner() }\n  var a: int = 5\n}\nin go: exec\nC()",
            1,
        ),
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        assert!(
            var_users(&r, var_on_line(&r, top_line)).contains(&gc::VAR_INCREMENT),
            "inner increments the top-level a in\n{src}"
        );
    }
}

#[test]
fn a_chip_keeps_a_handler_constant_its_nested_mod_reads() {
    let r = compile(
        "var z: int = 0\nin c: bool\nin go: exec\non go {\n  const xs = [11111, 22222, 12345]\n  if c {\n\
             mod f() -> int { return xs[2] }\n    chip C() { z = f() }\n    C()\n  }\n}",
    );
    assert_no_errors(&r);
    assert!(!has_gate_class_anywhere(&r, "_Unsupported"), "xs[2] lowered to a placeholder");
    assert_no_cross_module_literal(&r);
    assert!(takes_constant(&r, gc::VAR_SET, 12345), "z is set to 12345");
}

/// The lines WS091 is reported on.
fn ws091_lines(src: &str) -> Vec<u32> {
    let r = compile(src);
    let mut lines: Vec<u32> = r.diagnostics.iter().filter(|d| d.code == "WS091").map(|d| d.range.start.line).collect();
    lines.sort_unstable();
    lines
}

#[test]
fn a_local_shadowing_storage_a_called_mod_uses_is_ws091() {
    // At the local's declaration: the call uses the outer name.
    assert_eq!(
        ws091_lines("var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  var q: int = 5\n  bump()\n}"),
        vec![5]
    );
    assert_eq!(
        ws091_lines("var xs: int[]\nmod add(v: int) { xs.push(v) }\nin go: exec\non go {\n  var xs: int[]\n  add(3)\n}"),
        vec![5]
    );
    assert_eq!(
        ws091_lines("var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  let q = 7\n  bump()\n}"),
        vec![5]
    );
    // A nested mod reading its enclosing mod's var, under a block local.
    assert_eq!(
        ws091_lines(
            "var z: int = 0\nmod outer() {\n  var a: int = 1\n  mod f() { a = a + 1 }\n\
               if z > 0 {\n    let a = 5\n    f()\n  }\n}\nin go: exec\non go { outer() }"
        ),
        vec![6]
    );
}

#[test]
fn a_local_the_called_mod_does_not_reach_is_not_ws091() {
    for src in [
        // No shadow.
        "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go { bump() }",
        // The mod declares its own `q`.
        "mod bump() {\n  var q: int = 0\n  q = q + 1\n}\nin go: exec\non go {\n  var q: int = 5\n  bump()\n}",
        // `q` is the mod's parameter.
        "mod bump(q: int) -> int { return q + 1 }\nvar z: int = 0\nin go: exec\non go {\n  let q = 7\n  z = bump(q)\n}",
        // The shadowed name is a constant, not storage.
        "let k = 3\nvar z: int = 0\nmod bump() { z = z + k }\nin go: exec\non go {\n  let k = 7\n  bump()\n}",
        // The local is declared after the call.
        "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  bump()\n  var q: int = 5\n}",
        // The called mods only name `a` as their own local.
        "var a: int = 0\nvar xs: int[]\nmod inner(addr: int) -> int {\n  let a = addr & 3\n  xs.push(a)\n  return a\n}\n\
         mod outer(x: int) -> int { return inner(x) }\nvar z: int = 0\nin go: exec\non go {\n  var a: int = 5\n  z = outer(a)\n}",
    ] {
        assert_eq!(ws091_lines(src), Vec::<u32>::new(), "in\n{src}");
    }
}

/// The classes of the gates that take the storage gate `id` through a
/// `VarRef`/`ArrayVarRef`/`MapVarRef` port, following chip pins.
fn storage_user_classes(r: &LowerResult, id: NodeId) -> Vec<&'static str> {
    let (nodes, wires) = flat(r);
    let mut out = Vec::new();
    let mut stack = vec![id];
    let mut seen = crate::collections::HashSet::default();
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        for w in wires.iter().filter(|w| w.source.node_id == at) {
            let Some(n) = nodes.get(&w.target.node_id) else { continue };
            if matches!(n.gate_class, gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT | gc::REROUTER) {
                stack.push(w.target.node_id);
            } else if matches!(w.target.port, WirePort::VarRef | WirePort::ArrayVarRef | WirePort::MapVarRef) {
                out.push(n.gate_class);
            }
        }
    }
    out
}

/// The storage gate (`var`, array or map) declared in `file` on `line`.
fn storage_on_line(r: &LowerResult, file: &str, line: u32) -> NodeId {
    let (nodes, _) = flat(r);
    let found: Vec<NodeId> = nodes
        .iter()
        .filter(|(_, n)| {
            matches!(n.gate_class, gc::VAR | gc::ARRAY_VAR | gc::MAP_VAR)
                && &*n.source_range.file == file
                && n.source_range.start.line == line
        })
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(found.len(), 1, "expected one storage gate on {file}:{line}, found {found:?}");
    found[0]
}

#[test]
fn a_local_declared_over_storage_the_body_frame_binds_is_its_own() {
    // A ref, array or map param, and a chip ref param: the body's own `var`
    // of that name is a fresh gate, and the caller's `g` is left alone.
    for src in [
        "var g: int = 7\nvar z: int = 0\nmod f(a: *int) {\n  var a: int = 12345\n  z = a\n}\nin go: exec\non go { f(g) }",
        "var g: int[] = []\nvar z: int = 0\nmod f(xs: int[]) {\n  var xs: int[] = []\n  xs.push(12345)\n  z = xs.length()\n}\n\
         in go: exec\non go { f(g) }",
        "var g: Map<int, int> = {}\nvar z: int = 0\nmod f(m: Map<int, int>) {\n  var m: Map<int, int> = {}\n  m.set(1, 12345)\n\
           z = m.get(1)\n}\nin go: exec\non go { f(g) }",
        "var g: int = 7\nvar z: int = 0\nin go: exec\nchip C(a: *int) {\n  var a: int = 12345\n  on go { z = a }\n}\nC(g)",
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        let g = storage_on_line(&r, "test", 1);
        let users = storage_user_classes(&r, g);
        assert!(
            !users.iter().any(|c| c.contains("Set") || c.contains("Push") || c.contains("Increment") || c.contains("Clear")),
            "the caller's g was changed ({users:?}) in\n{src}"
        );
        assert!(
            takes_constant(&r, gc::VAR_SET, 12345) || takes_constant(&r, gc::VAR, 12345) || !src.contains("*int"),
            "the local var never takes 12345 in\n{src}"
        );
        let (nodes, _) = flat(&r);
        let own = nodes
            .values()
            .filter(|n| matches!(n.gate_class, gc::VAR | gc::ARRAY_VAR | gc::MAP_VAR) && n.source_range.start.line > 2)
            .count();
        assert!(own >= 1, "the body's own storage has no gate in\n{src}");
    }
}

#[test]
fn a_namespaced_mods_local_is_not_its_module_var() {
    let lib = "var count: int = 7\nmod f() -> int {\n  var count: int = 12345\n  count = count + 1\n  return count\n}";
    let r = compile_multi("import * as L from \"lib\"\nvar z: int = 0\nin go: exec\non go { z = L.f() }", &[("lib", lib)]);
    assert_no_errors(&r);
    let module_count = storage_on_line(&r, "lib.ws", 1);
    assert!(
        storage_user_classes(&r, module_count).is_empty(),
        "the module's count was written by f's local"
    );
    storage_on_line(&r, "lib.ws", 3);
}

#[test]
fn a_chip_port_declared_after_a_nested_mod_is_not_its_name() {
    let r = compile(
        "var x: int = 12345\nvar z: int = 0\nin go: exec\nchip C() {\n  mod f() { z = x }\n  in x: int\n  on go { f() }\n}\nC()",
    );
    assert_no_errors(&r);
    assert!(
        var_users(&r, var_on_line(&r, 1)).contains(&gc::VAR_GET),
        "f reads the top-level x"
    );
}

#[test]
fn a_local_shadowing_storage_a_mod_reaches_through_another_mod_is_ws091() {
    assert_eq!(
        ws091_lines(
            "var q: int = 0\nvar z: int = 0\nmod inner() { q = q + 1 }\nmod outer() { inner() }\nin go: exec\n\
             on go {\n  var q: int = 5\n  outer()\n  z = q\n}"
        ),
        vec![7]
    );
    // Recursion through the call graph terminates.
    assert_eq!(
        ws091_lines(
            "var q: int = 0\nmod a() { b() }\nmod b() { q = q + 1 }\nmod c() { a() }\nin go: exec\n\
             on go {\n  var q: int = 5\n  c()\n}"
        ),
        vec![7]
    );
}
