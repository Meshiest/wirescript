//! Array storage, literals and methods.

use super::*;

#[test]
fn array_pop_value_and_isempty_use_distinct_ports() {
    // `pop()` exposes a record { Value, IsEmpty }, and the lowering declares BOTH
    // outputs. With only `Value` declared, `.IsEmpty` falls back to the `Value`
    // port and in game the pop's `Value` resolves to `bIsEmpty`, so a pop yields 0.
    let src = "var a: int[]\n\
               in go: exec\n\
               var v: int = 0\n\
               var e: bool = false\n\
               on go {\n\
                 a.push(42)\n\
                 let p = a.pop()\n\
                 v = p.Value\n\
                 e = p.IsEmpty\n\
               }";
    let r = compile_ok(src);
    let pop_id = find_gate(&r, gc::ARRAY_POP);
    let out_ports: Vec<WirePort> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == pop_id)
        .map(|w| w.source.port)
        .collect();
    assert!(
        out_ports.contains(&WirePort::Value),
        "`v = p.Value` must read the pop's Value port; source ports: {out_ports:?}"
    );
    assert!(
        out_ports.contains(&WirePort::BIsEmpty),
        "`e = p.IsEmpty` must read the pop's bIsEmpty port, not fall back to Value; source ports: {out_ports:?}"
    );
}

/// `xs.join(sep)` is one Join gate reading the array through its
/// `ArrayVarRef`, with a literal separator baked onto `Separator`.
#[test]
fn array_join_lowers_to_one_join_gate() {
    let r = compile_ok(
        "var xs: int[]\nin t: exec\non t {\n  let s = xs.join(\", \")\n  BroadcastChatMessage(s)\n}",
    );
    let join = find_gate(&r, gc::ARRAY_JOIN);
    let arr = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == join && w.target.port == WirePort::ArrayVarRef)
        .expect("Join must have its ArrayVarRef wired");
    assert_eq!(
        r.module.nodes[&arr.source.node_id].gate_class,
        gc::ARRAY_VAR
    );
    assert_eq!(
        r.module.nodes[&join]
            .properties
            .get(&crate::intern::intern("Separator")),
        Some(&crate::ir::Literal::String(", ".into()))
    );
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == join && w.source.port == WirePort::Output),
        "the joined string must feed the chat message"
    );
}

/// The separator is optional: `join()` leaves `Separator` unset.
#[test]
fn array_join_separator_is_optional() {
    let r =
        compile_ok("var xs: string[]\nin t: exec\non t {\n  BroadcastChatMessage(xs.join())\n}");
    let join = find_gate(&r, gc::ARRAY_JOIN);
    assert!(
        r.module.nodes[&join]
            .properties
            .get(&crate::intern::intern("Separator"))
            .is_none()
    );
    assert!(
        !r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == join && w.target.port == WirePort::Separator)
    );
}

/// `join` gives a `string` whatever the element type, and works on int,
/// string and entity arrays.
#[test]
fn array_join_is_a_string_on_any_element_type() {
    let r = compile_ok(
        "var a: int[]\nvar b: string[]\nvar c: entity[]\nvar msg: string\n\
         mod say(s: string) { BroadcastChatMessage(s) }\nin t: exec\non t {\n  \
         say(a.join(\",\"))\n  msg = \"b: \" .. b.join(\" \")\n  let s: string = c.join()\n  say(s)\n}",
    );
    assert_eq!(gate_count(&r, gc::ARRAY_JOIN), 3);
    let bad = compile("var a: int[]\nin t: exec\non t {\n  let n: int = a.join()\n}");
    assert!(
        bad.diagnostics
            .iter()
            .any(|d| d.code == "WS016" && d.message.contains("has type string")),
        "a joined string is not an int: {:?}",
        bad.diagnostics
    );
}

#[test]
fn var_array_desugars_to_array_var() {
    // `var foo: T[]` is an array: it lowers to an ArrayVar gate (not a
    // Pseudo_Var), supports a constant initializer, and the methods work.
    let r = compile_ok(
        "var va: int[] = [7, 8]\nin t: exec\non t {\n  va.push(9)\n  let n = va.length()\n}",
    );
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::ARRAY_VAR)
        .expect("a var array should be an ArrayVar gate");
    assert!(
        node.properties
            .contains_key(&crate::intern::intern("InitialValue")),
        "var array initializer should populate the gate"
    );
    assert!(
        !has_gate_deep(&r, gc::VAR),
        "a var array is not a Pseudo_Var"
    );
    assert!(
        has_gate(&r, gc::ARRAY_PUSH),
        "var array push should lower to the ArrayVar push gate"
    );
}

#[test]
fn find_result_fields_read_the_find_gate() {
    // `find` is a record { Index, Found, Value }: its fields read from a bound
    // result or inline (`.Found` on an inline `find()` still lowers the call),
    // and a bare result auto-unwraps to the int Index.
    let src = "var a: int[]\nvar found: bool\nvar at: int\nvar hit: bool\nin t: exec\n\
        on t {\n  let r = a.find(3)\n  at = r.Index\n  found = r.Found\n  \
        at = a.find(3)\n  hit = !a.find(3).Found\n}";
    let r = compile_ok(src);
    assert_eq!(gate_count(&r, gc::ARRAY_FIND), 3, "one Find gate per call");
    assert!(
        !has_unsupported(&r.module),
        "inline find().Found must not degrade to an _Unsupported placeholder"
    );
}

#[test]
fn every_canonical_array_method_lowers() {
    // Exercises every method in the canonical ARRAY_METHODS table. This ties
    // the table (which drives editor completion/hover) to the dispatch in
    // lower_array_method: a table entry the dispatch can't handle would lower
    // to an `_Unsupported` gate, or to no gate of its own, and fail here.
    let src = "var a: int[]\nvar b: int[]\nvar bricks: brick[]\nvar ent: entity\nin z: zone\nin t: exec\non t {\n  \
        a.push(1)\n  let p = a.pop()\n  let n = a.length()\n  a.remove(0)\n  a.insert(0, 9)\n  \
        a.clear()\n  let i = a.find(3)\n  let g = a.get(0)\n  a.sort()\n  a.reverse()\n  a.shuffle()\n  a.swap(0, 1)\n  \
        a.fill(3)\n  a.resize(4, 0)\n  let s = a.sum()\n  let lo = a.min()\n  let hi = a.max()\n  \
        let av = a.average()\n  let j = a.join(\", \")\n  b.append(a)\n  b.copyFrom(a)\n  b.slice(a, 0, 2)\n  \
        a.fillFromPlayers()\n  a.fillFromTeam(ent)\n  a.fillFromZoneEntities(z)\n  \
        a.fillFromZonePlayers(z)\n  a.sortMultiple(b)\n}";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "an array method lowered to _Unsupported"
    );
    // Guard: this test must exercise every entry in the canonical table, so
    // the table cannot list a method without proving it lowers.
    for m in crate::catalog::arrays::ARRAY_METHODS {
        assert!(
            src.contains(&format!(".{}(", m.name)),
            "array method `{}` is in ARRAY_METHODS but not covered by this test",
            m.name
        );
        assert!(
            has_gate(&r, m.gate),
            "array method `{}` emitted no {} gate",
            m.name,
            m.gate
        );
    }
    for class in [
        gc::ARRAY_INSERT,
        gc::ARRAY_SORT,
        gc::ARRAY_REVERSE,
        gc::ARRAY_SWAP,
        gc::ARRAY_FILL,
        gc::ARRAY_RESIZE,
        gc::ARRAY_SUM,
        gc::ARRAY_MIN,
        gc::ARRAY_MAX,
        gc::ARRAY_AVERAGE,
        gc::ARRAY_FIND,
        gc::ARRAY_APPEND,
        gc::ARRAY_COPY_FROM,
        gc::ARRAY_SLICE,
    ] {
        assert!(has_gate(&r, class), "missing gate {class}");
    }
}

#[test]
fn handler_local_array_var_rebuilds_without_bogus_var_set() {
    // `var nums = [1,2,3]` inside a handler: the re-init on scope entry uses the
    // array rebuild (clear + push). The generic Var_Set reset would wire a `VarRef`
    // source port Pseudo_ArrayVar lacks (in game: "Wire source port VarRef does not
    // exist in source component").
    let src = "in run: exec\n\
               on run {\n\
                 var nums = [1, 2, 3]\n\
                 let x = nums[0]\n\
                 PrintToConsole(\"${x}\")\n\
               }";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "array literal init must lower: {:?}",
        wiring_shape(&r.module)
    );
    let array_vars: crate::collections::HashSet<_> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == gc::ARRAY_VAR)
        .map(|(id, _)| *id)
        .collect();
    for w in &r.module.wires {
        if array_vars.contains(&w.source.node_id) {
            assert_eq!(
                w.source.port,
                WirePort::ArrayVarRef,
                "ArrayVar only exposes ArrayVarRef; found a wire leaving via {:?}",
                w.source.port.as_str()
            );
        }
    }
    assert!(
        has_gate(&r, gc::ARRAY_CLEAR),
        "array re-init should clear then push"
    );
}

#[test]
fn array_literal_assignment_desugars_to_clear_push_append() {
    // `foo = [item, 1, ...base, 5]` in an exec handler rebuilds the array:
    // clear, then a push per item and an append per spread.
    let src = "var base: int[] = [3, 4]\nvar foo: int[]\nin t: exec\n\
        on t {\n  let item = 7\n  foo = [item, 1, ...base, 5]\n}";
    let r = compile_ok(src);
    assert_eq!(
        gate_count(&r, gc::ARRAY_CLEAR),
        1,
        "assignment should clear the array first"
    );
    assert_eq!(
        gate_count(&r, gc::ARRAY_APPEND),
        1,
        "a `...spread` element should lower to an Append gate"
    );
    assert_eq!(
        gate_count(&r, gc::ARRAY_PUSH),
        3,
        "three plain items should lower to three Push gates"
    );
}

#[test]
fn multi_line_array_literals_parse() {
    // Newlines are allowed after '[', around commas, and before ']', with an
    // optional trailing comma, mirroring the multi-line call-arg rules. Both the
    // top-level baked initializer and the exec-context rebuild use the same
    // literal parse.
    let src = "var names: string[] = [\n  \"a\",\n  \"b\",\n  \"c\",\n]\n\
        var base: int[] = [1, 2]\nvar foo: int[]\nin t: exec\n\
        on t {\n  foo = [\n    3,\n    ...base,\n    4\n  ]\n}";
    let r = compile_ok(src);
    assert_eq!(
        gate_count(&r, gc::ARRAY_PUSH),
        2,
        "multi-line rebuild should still lower each item"
    );
}

#[test]
fn top_level_array_with_non_literal_or_spread_errors() {
    // Outside an exec handler an array initializer is baked, so non-literal
    // elements and spreads must be rejected (not silently dropped).
    let rows = [
        ("var x: int = 1\nvar foo = [x, 2]\n", "WS003"),
        (
            "var base: int[] = [1, 2]\nvar foo: int[] = [...base, 3]\n",
            "WS003",
        ),
    ];
    let mut failures = Vec::new();
    for (src, code) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if errors != [code] {
            failures.push(format!("{src:?}: want [{code}], got {errors:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn non_literal_array_assign_is_rejected_not_silently_miscompiled() {
    // `xs = ys` has no whole-array assignment gate; the generic Var_Set path would
    // wire the ArrayVar's nonexistent `VarRef` port, which fails only at emit.
    let r = compile("var xs: int[]\nvar ys: int[]\nin t: exec\non t {\n  xs = ys\n}\n");
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS027"
            && d.severity == crate::diagnostic::Severity::Error
            && d.message.contains("xs.copyFrom")),
        "a non-literal whole-array assign must be a WS027 error, got: {:?}",
        r.diagnostics
    );
    assert!(!has_gate_deep(&r, gc::VAR_SET));
    assert!(
        !r.module
            .wires_deep()
            .any(|w| w.source.port == WirePort::VarRef),
        "no wire may source the ArrayVar's nonexistent VarRef port; wires: {:?}",
        r.module.wires
    );
}

// sortMultiple's parallel arrays: at most 7, each resolved or loud

const SORT_DECLS: &str = "var k: int[]\nvar a1: int[]\nvar a2: int[]\nvar a3: int[]\n\
    var a4: int[]\nvar a5: int[]\nvar a6: int[]\nvar a7: int[]\nvar a8: int[]\n\
    var rows: int[][]\nin go: exec\nin d: bool\n";

/// The gate has ArrayVarRef2..8, so an 8th parallel array has no port. It
/// must be rejected, not dropped.
#[test]
fn sort_multiple_with_more_than_seven_arrays_is_an_error() {
    for call in [
        "k.sortMultiple(a1, a2, a3, a4, a5, a6, a7, a8)",
        "k.sortMultiple(a1, a2, a3, a4, a5, a6, a7, rows[0])",
        "k.sortMultiple(a1, a2, a3, a4, a5, a6, a7, a8, true)",
    ] {
        let src = format!("{SORT_DECLS}on go {{\n  {call}\n}}");
        let e = errs(&src);
        assert!(e.iter().any(|c| c == "WS011"), "{call}: {e:?}");
    }
    let src = format!("{SORT_DECLS}on go {{\n  k.sortMultiple(a1, a2, a3, a4, a5, a6, a7, d)\n}}");
    let r = compile_ok(&src);
    let sort = find_gate(&r, gc::ARRAY_SORT_MULTIPLE);
    let into = |port: WirePort| {
        r.module
            .wires
            .iter()
            .filter(|w| w.target.node_id == sort && w.target.port == port)
            .count()
    };
    assert_eq!(
        into(WirePort::ArrayVarRef8),
        1,
        "the 7th array fills ArrayVarRef8"
    );
    assert_eq!(
        into(WirePort::BDescending),
        1,
        "a trailing bool is `descending`"
    );
}

/// Only a trailing bool may be `descending`. Any other positional is an
/// array, and one that does not resolve is WS044 rather than a wire into
/// `bDescending`.
#[test]
fn sort_multiple_never_wires_an_array_argument_into_descending() {
    let e = errs(&format!(
        "{SORT_DECLS}on go {{\n  k.sortMultiple(a1, 5)\n}}"
    ));
    assert!(
        e.iter().any(|c| c == "WS003"),
        "a non-array, non-bool positional: {e:?}"
    );
    let e = errs(&format!(
        "{SORT_DECLS}on go {{\n  k.sortMultiple(d, a1)\n}}"
    ));
    assert!(
        e.iter().any(|c| c == "WS003"),
        "a bool before an array: {e:?}"
    );

    let r = compile(&format!(
        "{SORT_DECLS}on go {{\n  k.sortMultiple(a1, [1, 2])\n}}"
    ));
    crate::test_support::assert_code(&r.diagnostics, "WS044");
    assert!(
        !r.module
            .wires_deep()
            .any(|w| w.target.port == WirePort::BDescending),
        "an array argument reached bDescending"
    );

    // A record array is carried along as every one of its columns, none of them
    // wired into bDescending.
    let r = compile_ok(&format!(
        "type Row = {{ u: string, g: float, f: string }}\nvar rs: Row[]\n{SORT_DECLS}\
         on go {{\n  k.sortMultiple(a1, rs, a2, d)\n}}"
    ));
    let sort = find_gate(&r, gc::ARRAY_SORT_MULTIPLE);
    let into = |port: WirePort| {
        r.module
            .wires
            .iter()
            .filter(|w| w.target.node_id == sort && w.target.port == port)
            .count()
    };
    for port in [
        WirePort::ArrayVarRef2,
        WirePort::ArrayVarRef3,
        WirePort::ArrayVarRef4,
        WirePort::ArrayVarRef5,
        WirePort::ArrayVarRef6,
    ] {
        assert_eq!(into(port), 1, "{port:?}");
    }
    assert_eq!(into(WirePort::BDescending), 1);
    let r = compile(&format!(
        "type Row = {{ u: string, g: float, f: string }}\nvar rs: Row[]\n{SORT_DECLS}\
         on go {{\n  k.sortMultiple(a1, rs, a2, a3, a4, a5)\n}}"
    ));
    crate::test_support::assert_code(&r.diagnostics, "WS011");

    let r = compile(&format!(
        "const T = [3, 1, 2]\n{SORT_DECLS}on go {{\n  k.sortMultiple(a1, T)\n}}"
    ));
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS044" && d.message.contains("would change")),
        "sortMultiple reorders its parallel arrays, so a const one is rejected: {:?}",
        r.diagnostics
    );
}
