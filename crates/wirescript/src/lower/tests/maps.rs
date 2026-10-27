//! Map storage, literals, subscripts and methods.

use super::*;

/// The entries baked into the first `InitialValue` map literal of `r`, if any.
fn baked_map(r: &LowerResult) -> Option<Vec<(crate::ir::Literal, crate::ir::Literal)>> {
    r.module.nodes_deep().find_map(|n| {
        match n.properties.get(&crate::intern::intern("InitialValue")) {
            Some(crate::ir::Literal::Map(es)) => Some(es.clone()),
            _ => None,
        }
    })
}

#[test]
fn every_canonical_map_method_lowers() {
    // Exercises every method in MAP_METHODS; a table entry the dispatch can't
    // handle would lower to `_Unsupported` or to no gate of its own and fail
    // here. `var m: Map<K, V>` desugars to a MapVar, as `var x: T[]` does to an
    // ArrayVar.
    let src = "var m: Map<string, int>\nvar m2: Map<string, int>\nvar keys: string[]\n\
        var vals: int[]\nin t: exec\non t {\n  \
        m.set(\"a\", 1)\n  let g = m.get(\"a\")\n  let h = m.has(\"a\")\n  let rm = m.remove(\"a\")\n  \
        let n = m.length()\n  m.copyFrom(m2)\n  m.keys(keys)\n  m.values(vals)\n  m.clear()\n}";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "a map method lowered to _Unsupported"
    );
    assert!(
        has_gate(&r, gc::MAP_VAR),
        "a var-declared map must create a Pseudo_MapVar"
    );
    for m in crate::catalog::maps::MAP_METHODS {
        assert!(
            src.contains(&format!(".{}(", m.name)),
            "map method `{}` is in MAP_METHODS but not covered by this test",
            m.name
        );
        assert!(
            has_gate(&r, m.gate),
            "map method `{}` emitted no {} gate",
            m.name,
            m.gate
        );
    }
}

#[test]
fn constant_map_literal_bakes_no_set_gates() {
    // A constant map literal in a `var` initializer bakes into the Pseudo_MapVar's
    // InitialValue at rest with zero runtime gates, as `var a: int[] = [1,2,3]` bakes
    // a `Literal::Array`. `{}` parses as an empty RECORD, which a Map-typed slot
    // accepts as the empty map.
    let rows: &[(&str, usize)] = &[
        (
            "var scores: Map<int, int> = { :red => 10, :blue => 20 }\n",
            2,
        ),
        ("var m: Map<string, int> = {}\n", 0),
        ("static var m: Map<int, int> = {}\nin t: exec\non t {}\n", 0),
    ];
    let mut failures = Vec::new();
    for (src, entries) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let baked = baked_map(&r).map_or(0, |es| es.len());
        if !errors.is_empty()
            || !has_gate(&r, gc::MAP_VAR)
            || has_gate_deep(&r, gc::MAP_SET)
            || baked != *entries
        {
            failures.push(format!(
                "{src:?}: errors {errors:?}, baked {baked} of {entries}\n{:?}",
                wiring_shape(&r.module)
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn map_literal_coerces_mixed_entry_literals() {
    // A coercion-mixed constant map entry (key/value literal kind differs from the
    // declared K/V) bakes through the same coercion laws as the array path:
    // string->bool `!= ""`, bool<->int, and int<->float truncation/widening.
    let cases = [
        (
            "var m: Map<int, bool> = { 1 => \"on\" }\n",
            crate::ir::Literal::Bool(true),
        ),
        (
            "var m: Map<int, int>  = { 1 => true }\n",
            crate::ir::Literal::Int(1),
        ),
        (
            "var m: Map<int, int>  = { 1 => 2.9 }\n",
            crate::ir::Literal::Int(2),
        ),
        (
            "var m: Map<int, float> = { 1 => true }\n",
            crate::ir::Literal::Float(1.0),
        ),
    ];
    for (src, want_val) in cases {
        let r = compile_ok(src);
        let baked = baked_map(&r).and_then(|es| es.first().map(|(_, v)| v.clone()));
        assert_eq!(
            baked.as_ref(),
            Some(&want_val),
            "wrong baked value for {src:?}"
        );
    }
}

#[test]
fn map_literal_in_a_map_slot_compiles_clean() {
    // A map literal in a VALID slot (a `map`/`var` initializer or an assignment
    // RHS to a map var), `{}` included, must NOT trip the WS026 position guard
    // or any other error.
    for src in [
        "var m: Map<int, int> = { :red => 1, :blue => 2 }\n",
        "var m: Map<string, int> = { \"a\" => 1 }\n",
        "var m: Map<int, int>\nin t: exec\non t {\n  m = { 1 => 2 }\n}\n",
        "var m: Map<int, int>\nin t: exec\non t {\n  m = { 1 => 2, 3 => 4 }\n}\n",
        "var m: Map<string, int> = {}\n",
        "static var m: Map<int, int> = {}\nin t: exec\non t {}\n",
        "in t: exec\non t {\n  var m: Map<int, int> = {}\n}\n",
    ] {
        let r = compile(src);
        assert!(
            crate::test_support::error_codes(&r.diagnostics).is_empty(),
            "map literal in a valid slot must compile clean for {src:?}: {:?}",
            r.diagnostics
        );
    }
}

#[test]
fn runtime_map_literal_desugars_to_clear_and_sets() {
    // A map literal with a runtime key (`[k]`), or any `m = {...}` in a handler,
    // can't bake: it is MapVar_Clear then one MapVar_Set per entry on the exec
    // chain. A non-static handler `var m: Map<K, V> = {...}` re-runs that reset
    // each invocation (Pseudo_MapVar has no `VarRef` for a Var_Set reset).
    let rows = [
        "var m: Map<int, int>\nin k: int\nin t: exec\non t {\n  m = { [k] => 1, :fixed => 2 }\n}\n",
        "in k: int\nin t: exec\non t {\n  var m: Map<int, int> = { [k] => 1, :fixed => 2 }\n}\n",
    ];
    let mut failures = Vec::new();
    for src in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let (clears, sets) = (
            count_class(&r.module, gc::MAP_CLEAR),
            count_class(&r.module, gc::MAP_SET),
        );
        if !errors.is_empty() || clears != 1 || sets != 2 {
            failures.push(format!(
                "{src:?}: errors {errors:?}, {clears} clears, {sets} sets (want 1, 2)"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn non_literal_map_assign_is_rejected_not_silently_miscompiled() {
    // Only a map literal RHS lowers by the desugar. `m = m2` would reach the
    // generic Var_Set reset and wire a `VarRef` port Pseudo_MapVar lacks (a
    // load failure in game), so it is the error WS027 pointing at `.copyFrom`;
    // a warning would be missed by error-gated checks.
    let r =
        compile("var m: Map<int, int>\nvar m2: Map<int, int>\nin t: exec\non t {\n  m = m2\n}\n");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.severity == crate::diagnostic::Severity::Error
                && d.message.contains("copyFrom")),
        "a non-literal whole-map assign must be a hard ERROR pointing at .copyFrom, got: {:?}",
        r.diagnostics
    );
    // The guard returns before lowering the RHS or a reset gate.
    assert!(
        !has_gate_deep(&r, gc::MAP_SET),
        "a rejected non-literal map assign must not emit a Set gate"
    );
    let map_node_ids: Vec<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::MAP_VAR)
        .map(|(id, _)| *id)
        .collect();
    assert!(
        !r.module
            .wires
            .iter()
            .any(|w| map_node_ids.contains(&w.source.node_id) && w.source.port == WirePort::VarRef),
        "no wire may source a nonexistent VarRef port off a Pseudo_MapVar node; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn map_subscript_key_value_ports_carry_concrete_types() {
    // `m[k]` and `m[k] = v` desugar to the MapVar_Get / MapVar_Set gates of
    // `m.get(k)` / `m.set(k, v)`, with Key/Value ports of the map's concrete
    // types: an `any` Key bakes a float `0.0` default, which the game rejects
    // for a string-keyed map at load.
    let r = compile_ok(
        "var m: Map<string, bool>\nin t: exec\non t {\n  m[\"a\"] = true\n  let v = m[\"a\"]\n}\n",
    );
    assert!(
        !has_unsupported(&r.module),
        "map[k] must not lower to _Unsupported"
    );
    let key_sym = crate::intern::intern("Key");
    let value_sym = crate::intern::intern("Value");
    let mut checked = 0;
    for n in r.module.nodes.values() {
        let is_get = n.gate_class == gc::MAP_GET;
        let is_set = n.gate_class == gc::MAP_SET;
        if !is_get && !is_set {
            continue;
        }
        checked += 1;
        let key_port = n.ports.inputs.iter().find(|p| p.name == key_sym);
        assert_eq!(
            key_port.map(|p| &p.ty),
            Some(&Type::String),
            "{} Key port must be Type::String, not any: {:?}",
            n.gate_class,
            n.ports.inputs
        );
        let value_port = if is_set {
            n.ports.inputs.iter().find(|p| p.name == value_sym)
        } else {
            n.ports.outputs.iter().find(|p| p.name == value_sym)
        };
        assert_eq!(
            value_port.map(|p| &p.ty),
            Some(&Type::Bool),
            "{} Value port must be Type::Bool, not any",
            n.gate_class
        );
    }
    assert_eq!(
        checked, 2,
        "expected one MapVar_Get and one MapVar_Set node"
    );
}

/// A map method on a `let` bound to a map var reads the var's map: the local
/// carries the `MapVarRef`, so the call is not a placeholder.
#[test]
fn a_map_method_on_a_let_bound_map_reads_the_var() {
    let r = compile_ok(
        "var m: Map<int, int>\nin go: exec\nvar v: int = 0\non go {\n  let mm = m\n  v = mm.get(1)\n}\n",
    );
    assert!(!has_unsupported(&r.module));
    let gets = nodes_of(&r.module, crate::ir::gate_class::MAP_GET);
    assert_eq!(gets.len(), 1, "one MapVar_Get");
    let reads_m = r.module.wires_deep().any(|w| {
        w.target.node_id == gets[0].id
            && w.source.port == crate::ir::port_registry::WirePort::MapVarRef
    });
    assert!(reads_m, "the get reads the map var's MapVarRef");
}
