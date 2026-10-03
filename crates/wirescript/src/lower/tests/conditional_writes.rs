//! Writes that run on only some paths: a call in a loop bound or through a
//! namespace, a write after a `return` that may have run, an `if`/`match`
//! expression arm that acts, and the checks that read those positions.

use super::*;
use crate::ir::gate_class as gc;

/// Whether output `name` of `m` is driven once, by its backing var.
fn backed(m: &crate::ir::Module, name: &str) -> bool {
    let out = m.nodes.iter().find(|(_, n)| {
        n.gate_class == gc::MICROCHIP_OUTPUT
            && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == name)
    });
    let Some((id, _)) = out else { return false };
    let drivers: Vec<_> = m.wires.iter().filter(|w| w.target.node_id == *id && w.target.port == WirePort::RerInput).collect();
    drivers.len() == 1 && m.nodes[&drivers[0].source.node_id].note == Some("out_backing")
}

fn exec_reaches(m: &crate::ir::Module, from: crate::ir::NodeId, to: crate::ir::NodeId) -> bool {
    let mut seen = crate::collections::HashSet::default();
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if seen.insert(n) {
            for w in &m.wires {
                if w.source.node_id == n && w.target.port.as_str().starts_with("Exec") {
                    stack.push(w.target.node_id);
                }
            }
        }
    }
    false
}

/// Whether store `id` writes the integer `v`, held inline or wired from a
/// literal.
fn stores_value(m: &crate::ir::Module, id: crate::ir::NodeId, v: i64) -> bool {
    let is_v = |n: &crate::ir::Node| matches!(n.properties.get(&*crate::intern::sym::VALUE), Some(crate::ir::Literal::Int(x)) if *x == v);
    is_v(&m.nodes[&id])
        || m.wires
            .iter()
            .filter(|w| w.target.node_id == id && w.target.port == WirePort::Value)
            .any(|w| m.nodes.get(&w.source.node_id).is_some_and(|n| n.gate_class == gc::LITERAL && is_v(n)))
}

/// The gate an exec output of `from` starts.
fn chain_start(m: &crate::ir::Module, from: crate::ir::NodeId, port: WirePort) -> crate::ir::NodeId {
    m.wires
        .iter()
        .find(|w| w.source == from.port(port))
        .map(|w| w.target.node_id)
        .expect("a chain on that pin")
}

/// A mod writing an output, called in a `for` bound under an `if`, or through
/// `import * as`, is a conditional or repeated site: the output is stored.
/// Both drove the output constantly, and two namespaced calls fanned in.
#[test]
fn a_call_in_a_loop_bound_or_through_a_namespace_is_a_counted_site() {
    let r = compile(
        "in go: exec\nin c: bool\nvar n: int = 0\nout on_: int = n\nout o: int\n\
         mod cnt(v: int) -> int {\n  emit o = v\n  return 3\n}\n\
         on go {\n  if c {\n    for i in 0..cnt(7) { n = n + 1 }\n  }\n}",
    );
    assert_no_errors(&r);
    assert!(backed(&r.module, "o"), "a call in a for bound");
    let lib = "out o: int\nmod setO(v: int) { emit o = v }\n";
    for entry in [
        "import * as u from \"lib\"\nin go: exec\nin c: bool\non go {\n  if c { u.setO(5) }\n}",
        "import * as u from \"lib\"\nin go: exec\nin go2: exec\non go { u.setO(5) }\non go2 { u.setO(6) }",
    ] {
        let r = compile_multi(entry, &[("lib", lib)]);
        assert_no_errors(&r);
        assert!(backed(&r.module, "o"), "{entry}");
    }
}

/// A write after a `return` that may have run is conditional, in a handler, a
/// chip's handler and a mod, whether the `return` sits under an `if` or in a
/// `match` arm. Each drove the output with the constant from load time.
#[test]
fn a_write_after_a_possible_return_is_conditional() {
    let head = "enum K { A, B }\nin go: exec\nin c: bool\nin k: int\nvar e: K = K.A\nvar n: int = 0\nout on_: int = n\n";
    for body in [
        "out o: int\non go {\n  if c { return }\n  emit o = 9\n}",
        "out o: int\non go {\n  match e { A => { return } B => { } }\n  emit o = 9\n}",
        "out o: int\non go {\n  n = match e { A => { return }, B => 2 }\n  emit o = 9\n}",
        "out o: int\nmod setO(v: int) { emit o = v }\nmod early(v: int) {\n  if c { return }\n  setO(v)\n}\non go { early(k) }",
        "out o: int\nmod early(v: int) {\n  if c { return }\n  emit o = v\n}\non go { early(k) }",
    ] {
        let src = format!("{head}{body}");
        let r = compile(&src);
        assert_no_errors(&r);
        assert!(backed(&r.module, "o"), "{src}");
    }
    let r = compile(&format!(
        "{head}chip C(t: exec) -> (o: int) {{\n  var d: bool = true\n  on t {{\n    if d {{ return }}\n    emit o = 9\n  }}\n}}\nout ro = C(go)"
    ));
    assert_no_errors(&r);
    assert!(backed(only_chip(&r, "the chip"), "o"), "a chip's handler");
    // With nothing before it that can leave, the write keeps its direct wire.
    let r = compile(&format!("{head}out o: int\non go {{\n  if c {{ n = 1 }}\n  emit o = 9\n}}"));
    assert_no_errors(&r);
    assert!(!backed(&r.module, "o"));
}

/// An `if` expression's branch that writes, through a mod, runs only when it
/// is taken; a branch with a `return` in a `match` leaves only on its branch;
/// and a value `match` arm that acts runs only when it matches. Each ran on
/// every evaluation.
#[test]
fn an_if_or_match_expression_arm_that_acts_runs_only_when_taken() {
    let head = "enum K { A, B }\nin go: exec\nin c: bool\nvar k: K = K.A\nvar n: int = 0\nout on_: int = n\nout o: int\n\
                mod setO(v: int) -> int {\n  emit o = v\n  return v\n}\n";
    for (body, start) in [
        ("on go {\n  n = if c then setO(4) else 0\n}", WirePort::ExecOutA),
        ("on go {\n  n = if c then 0 else setO(4)\n}", WirePort::ExecOutB),
        ("on go {\n  n = match k { A => setO(4), B => 0 }\n}", WirePort::ExecOutA),
    ] {
        let src = format!("{head}{body}");
        let r = compile(&src);
        assert_no_errors(&r);
        assert!(backed(&r.module, "o"), "{src}");
        let store = r.module.nodes.values().find(|n| n.note == Some("out_set")).expect("o's store").id;
        let branch = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::BRANCH)
            .map(|n| n.id)
            .min()
            .expect("a branch");
        assert!(exec_reaches(&r.module, chain_start(&r.module, branch, start), store), "{src}");
        let other = if start == WirePort::ExecOutA { WirePort::ExecOutB } else { WirePort::ExecOutA };
        assert!(!exec_reaches(&r.module, chain_start(&r.module, branch, other), store), "{src}");
    }
    // A `return` in a `match` inside a branch leaves the mod on that branch only.
    let r = compile(
        "enum K { A, B }\nin go: exec\nin c: bool\nvar k: K = K.A\nvar n: int = 0\nout on_: int = n\n\
         mod f() -> int {\n  let q = if c then (match k { A => { return 7 }, B => 2 }) else 3\n  return q\n}\n\
         on go { n = f() }",
    );
    assert_no_errors(&r);
    let branch = r.module.nodes.values().filter(|n| n.gate_class == gc::BRANCH).map(|n| n.id).min().expect("the if");
    let ret7 = r
        .module
        .nodes
        .values()
        .find(|n| n.note == Some("ret_set") && stores_value(&r.module, n.id, 7))
        .expect("the return of 7");
    assert!(exec_reaches(&r.module, chain_start(&r.module, branch, WirePort::ExecOutA), ret7.id));
    assert!(!exec_reaches(&r.module, chain_start(&r.module, branch, WirePort::ExecOutB), ret7.id));
    // A branch that only reads keeps the pure Select.
    let r = compile(&format!("{head}mod id(x: int) -> int {{ return x }}\non go {{\n  n = if c then id(4) else 0\n}}"));
    assert_no_errors(&r);
    assert!(has_gate(&r, gc::SELECT) && !has_gate(&r, gc::BRANCH));
    // A record has no store to choose through: WS097.
    let head = "type P = { x: int, y: int }\nin go: exec\nin c: bool\nvar q: int = 0\nvar p: P = { x: 0, y: 0 }\n\
                mod bump() -> int {\n  q = q + 1\n  return q\n}\n";
    for body in [
        "on go { p = if c then { x: bump(), y: 0 } else { x: 0, y: 0 } }",
        "enum K { A, B }\nvar k: K = K.A\non go { p = match k { A => { x: bump(), y: 0 }, B => { x: 0, y: 0 } } }",
    ] {
        let r = compile(&format!("{head}{body}"));
        assert!(r.diagnostics.iter().any(|d| d.code == "WS097"), "{body}: {:?}", r.diagnostics);
    }
}

/// A range's start and stride feed two of the loop's pins each, and are
/// evaluated once: `for i in bump()..9` called `bump()` twice.
#[test]
fn a_loop_bound_is_evaluated_once() {
    for range in ["bump()..9", "(0..9).step(bump())", "(bump()..9).rev()"] {
        let src = format!(
            "in go: exec\nvar q: int = 0\nvar n: int = 0\nout oq: int = q\nout on_: int = n\n\
             mod bump() -> int {{\n  q = q + 1\n  return q\n}}\non go {{\n  for i in {range} {{ n = n + 1 }}\n}}"
        );
        let r = compile(&src);
        assert_no_errors(&r);
        let bumps = r
            .module
            .nodes
            .values()
            .filter(|n| matches!(n.gate_class, gc::VAR_INCREMENT | gc::VAR_SET) && n.source_range.start.line == 7)
            .count();
        assert_eq!(bumps, 1, "{range}");
    }
}

/// The path checks see a `return` and an `emit` in a value `match` arm:
/// WS090 for a path that leaves before setting the output (it compiled and
/// handed the caller a stale value), and no false WS013 for an output written
/// only there.
#[test]
fn path_checks_see_value_match_arms() {
    let head = "enum K { A, B }\nin go: exec\nvar k: K = K.A\nvar n: int = 0\nout on_: int = n\n";
    let e = errs(&format!(
        "{head}mod f(k: K) -> (r: int) {{\n  let v = match k {{ A => {{ return }}, B => 3 }}\n  return v + 1\n}}\non go {{ n = f(k) }}"
    ));
    assert!(e.contains(&"WS090".to_string()), "{e:?}");
    let e = errs(&format!(
        "{head}chip C(t: exec) -> (o: int) {{\n  on t {{ n = match k {{ A => {{ emit o = 1\n    return }}, B => 2 }} }}\n}}\nout res = C(go)"
    ));
    assert!(!e.contains(&"WS013".to_string()), "{e:?}");
    let e = errs(&format!(
        "{head}mod f(k: K) -> (a: int) {{\n  let v = match k {{ A => {{ emit a = 1\n    return }}, B => 2 }}\n  n = v\n}}\non go {{ n = f(k) + 1 }}"
    ));
    assert!(!e.contains(&"WS013".to_string()) && e.contains(&"WS090".to_string()), "{e:?}");
}

/// A `SendCustomEvent` in a loop is checked against its receiver and types it,
/// as one outside a loop is.
#[test]
fn a_custom_event_sent_in_a_loop_is_checked() {
    let src = "in go: exec\nvar got: int = 0\nout o: int = got\non CustomEvent(\"e\") -> (a: int) {\n  got = a\n}\n\
               on go {\n  for i in 0..2 {\n    SendCustomEvent(\"e\", \"hello\")\n  }\n}";
    let r = compile(src);
    assert!(r.diagnostics.iter().any(|d| d.code == "WS030"), "{:?}", r.diagnostics);
    let src = "in go: exec\nvar got: int = 0\nout o: int = got\non CustomEvent(\"e\") -> (a) {\n  got = a\n}\n\
               on go {\n  for i in 0..2 {\n    SendCustomEvent(\"e\", 7)\n  }\n}";
    let r = compile(src);
    assert!(!r.diagnostics.iter().any(|d| d.code == "WS042"), "{:?}", r.diagnostics);
}

/// An `out` in a `match` arm of a handler declares its port as one in an `if`
/// branch does; it was WS073.
#[test]
fn an_out_in_a_handler_match_arm_is_a_port() {
    let r = compile(
        "enum K { A, B }\nin go: exec\nvar k: K = K.A\non go {\n  match k { A => { out z = 5 } B => { out z = 6 } }\n}",
    );
    assert_no_errors(&r);
    assert!(backed(&r.module, "z"));
}

/// A `mod` or `chip` called from an `if`/`match` expression arm whose own `on`
/// handler acts runs that handler on its trigger whatever the condition is:
/// `WS102`, once per call. A handler that only sets the callee's own output,
/// and a call outside an arm, are not warned.
#[test]
fn a_handler_an_arm_cannot_gate_is_warned() {
    let head = "enum K { A, B }\nin go: exec\nin c: bool\nvar k: K = K.A\nvar n: int = 0\nout onn: int = n\n\
                var cnt: int = 0\nout ocnt: int = cnt\n\
                chip Cx(t: exec, v: int) -> (r: int) {\non t {\ncnt = cnt + 1\nemit r = v\n}\n}\n\
                chip Pure(t: exec, v: int) -> (r: int) {\non t {\nemit r = v\n}\n}\n";
    let count = |body: &str| {
        let r = compile(&format!("{head}{body}"));
        assert_no_errors(&r);
        r.diagnostics.iter().filter(|d| d.code == "WS102").count()
    };
    assert_eq!(count("on go { n = if c then Cx(go, 3) else 0 }"), 1);
    assert_eq!(count("on go { n = if c then 0 else Cx(go, 3) + 1 }"), 1);
    assert_eq!(count("on go { n = match k { A => Cx(go, 3), B => 0 } }"), 1);
    assert_eq!(count("mod f() -> int { return if c then Cx(go, 3) else 0 }\non go { n = f() + f() }"), 1);
    assert_eq!(count("on go { n = if c then Pure(go, 3) else 0 }"), 0);
    assert_eq!(count("on go { n = Cx(go, 3) }"), 0);
    assert_eq!(count("on go { n = if Cx(go, 3) > 0 then 1 else 0 }"), 0);
    // State the chip declares outside the handler keeps the handler's write.
    let own = |decl: &str, write: &str| {
        format!("chip Own(t: exec, v: int) -> (r: int) {{\n{decl}\non t {{\n{write}\n}}\n}}\non go {{ n = if c then Own(go, 3) else 0 }}")
    };
    assert_eq!(count(&own("var own: int = 0", "own = own + v\nemit r = own")), 1);
    assert_eq!(count(&own("var arr: int[] = []", "arr.push(v)\nemit r = arr.length()")), 1);
    assert_eq!(count(&own("var own: int = 0", "emit r = own + v")), 0);
    assert_eq!(count(&own("", "var tmp: int = 0\ntmp = v\nemit r = tmp")), 0);
    assert_eq!(count(&own("", "let q = v + 1\nemit r = q")), 0);
}

/// An output no site gives a type is stored as the join of every value written
/// to it: `int` and `float` give `float`, so an integer constant written to it
/// is that float, and a bare multi-output read is the output it reads. Values
/// no one type holds (a string, bool or enum beside a number, a record beside
/// a scalar, two different records) are `WS103` (a string was stored into an
/// `int` var).
#[test]
fn an_untyped_output_is_stored_as_the_join_of_its_writes() {
    let head = "enum K { A, B }\ntype P = { a: int, b: float }\ntype Q = { a: int, b: int }\nin go: exec\n\
                in c: bool\nin iv: int\nin fa: float\nvar k: K = K.A\nvar pv: P = { a: 1, b: 2.0 }\n\
                var qv: Q = { a: 1, b: 2 }\nvar fs: float[] = [1.5]\nvar is: int[] = [1, 2]\n\
                var m: Map<string, float> = {}\n";
    for body in [
        "on go {\n  if c { out z = 7 } else { out z = \"six\" }\n}",
        "on go {\n  match k { A => { out z = 7 } B => { out z = \"six\" } }\n}",
        "on go {\n  out z = 7\n  if c { emit z = true }\n}",
        "on go {\n  if c { out z = k } else { out z = 7 }\n}",
        "on go {\n  if c { out z = pv } else { out z = 7 }\n}",
        "on go {\n  if c { out z = pv } else { out z = qv }\n}",
    ] {
        let e = errs(&format!("{head}{body}"));
        assert_eq!(e.iter().filter(|c| *c == "WS103").count(), 1, "{body}: {e:?}");
    }
    let lib = "in go: exec\non go { out z = 5 }";
    let r = compile_multi("import \"lib\"\nin go2: exec\non go2 { out z = \"s\" }", &[("lib", lib)]);
    assert!(r.diagnostics.iter().any(|d| d.code == "WS103"), "across files: {:?}", r.diagnostics);
    // Joined to `float`: the integer constant is stored as the float.
    for body in [
        "on go {\n  if c { out z = 7 } else { out z = 2.5 }\n}",
        "on go {\n  if c { out z = 2.5 } else { out z = 7 }\n}",
        "on go {\n  out z = 7\n}\non c {\n  out z = 2.5\n}",
        "on go {\n  if c { out z = fa } else { out z = 7 }\n}",
        "on go {\n  if c { out z = -7 } else { out z = 1.0 }\n}",
        "const SEVEN = 7\non go {\n  if c { out z = fa } else { out z = SEVEN }\n}",
        "on go {\n  if c { out z = fs.max() } else { out z = 7 }\n}",
    ] {
        let src = format!("{head}{body}");
        let e = errs(&src);
        assert!(e.is_empty(), "{body}: {e:?}");
        let r = compile(&src);
        let port = r.module.nodes.values().find(|n| {
            n.gate_class == gc::MICROCHIP_OUTPUT
                && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(Literal::String(s)) if s == "z")
        });
        let port_ty = port.and_then(|n| n.ports.inputs.first()).map(|p| p.ty.clone());
        assert_eq!(port_ty, Some(crate::ir::Type::Float), "{body}: the port is not a float");
        for fold in [false, true] {
            let saved = super::float_slots::save_numbers(&src, fold);
            let seven = |l: &Literal| matches!(l, Literal::Float(f) if f.abs() == 7.0);
            assert!(saved.iter().any(seven), "{body} fold {fold}: no float 7\n{saved:?}");
            assert!(!saved.iter().any(|l| matches!(l, Literal::Int(7 | -7))), "{body} fold {fold}: {saved:?}");
        }
    }
    for body in [
        "on go {\n  if c { out z = 5 } else { out z = iv }\n}",
        "on go {\n  if c { out z: float = 5 } else { out z = 2.5 }\n}",
        "on go {\n  match k { A => { out z = \"a\" } B => { out z = \"b\" } }\n}",
        "on go {\n  if c { out z = fs.max() } else { out z = 2.5 }\n}",
        "on go {\n  if c { out z = m.get(\"x\") } else { out z = 0.0 }\n}",
        "on go {\n  if c { out z = is.find(iv) } else { out z = -1 }\n}",
        "on go {\n  if c { out z = is.pop() } else { out z = 0 }\n}",
        "on go {\n  if c { out z = is.find(iv) } else { out z = iv }\n}",
        "on go {\n  if c { out z = fs.pop() } else { out z = fs.max() }\n}",
        "on go {\n  if c { out z = pv } else { out z = { a: 1, b: 2 } }\n}",
        "on go {\n  if c { out z = { a: 1, b: 2 } } else { out z = pv }\n}",
    ] {
        let e = errs(&format!("{head}{body}"));
        assert!(e.is_empty(), "{body}: {e:?}");
    }
}

/// A `mod` body is inlined where it is called, so its write to an output with
/// no type is one of that output's writes: joined with the others, and
/// `WS103` when no type holds them all. A generic body counts for the
/// instantiations calls make, not for every type its bounds admit.
#[test]
fn a_mod_body_write_to_an_untyped_output_joins_it() {
    let head = "in go: exec\nin go2: exec\nin go3: exec\nin a: int\nin fa: float\nin s: string\n";
    for body in [
        "mod wr(v: string) { out z = v }\non go { out z = 5 }\non go2 { wr(s) }",
        "mod wr<T>(v: T) { out z = v }\non go { out z = 5 }\non go2 { wr(a) }\non go3 { wr(s) }",
        "mod b(v: string) { out z = v }\nmod c(v: string) { b(v) }\non go { out z = 5 }\non go2 { c(s) }",
    ] {
        let e = errs(&format!("{head}{body}"));
        assert_eq!(e.iter().filter(|c| *c == "WS103").count(), 1, "{body}: {e:?}");
    }
    let port_ty = |src: &str| {
        let r = compile(src);
        assert_no_errors(&r);
        let port = r.module.nodes.values().find(|n| {
            n.gate_class == gc::MICROCHIP_OUTPUT
                && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(Literal::String(s)) if s == "z")
        });
        port.and_then(|n| n.ports.inputs.first()).map(|p| p.ty.clone())
    };
    // Joined to `float`: the handler's integer constant is stored as the float.
    for body in [
        "mod wr(v: float) { out z = v }\non go2 { wr(fa) }\non go { out z = 5 }",
        "mod wr(v: float) { out z = v }\non go { out z = 5 }\non go2 { wr(fa) }",
        "mod wr<T>(v: T) { out z = v }\non go { out z = 5 }\non go2 { wr(a) }\non go3 { wr(fa) }",
        "mod b(v: float) { out z = v }\nmod c(v: float) { b(v) }\non go { out z = 5 }\non go2 { c(fa) }",
    ] {
        let src = format!("{head}{body}");
        assert_eq!(port_ty(&src), Some(crate::ir::Type::Float), "{body}: the port is not a float");
        for fold in [false, true] {
            let saved = super::float_slots::save_numbers(&src, fold);
            assert!(saved.contains(&Literal::Float(5.0)), "{body} fold {fold}: no float 5\n{saved:?}");
            assert!(!saved.contains(&Literal::Int(5)), "{body} fold {fold}: {saved:?}");
        }
    }
    // An instantiation no call makes writes nothing.
    let src = format!("{head}mod wr<T>(v: T) {{ out z = v }}\non go {{ out z = 5 }}\non go2 {{ wr(a) }}");
    assert_eq!(port_ty(&src), Some(crate::ir::Type::Int), "an int-only generic write");
    // Nor does a mod nothing running calls, directly or through another one
    // nothing calls; one a handler reaches through a chain still writes.
    for (body, want) in [
        ("mod wr(v: float) { out z = v }\nmod never() { wr(1.5) }\non go { out z = 5 }", crate::ir::Type::Int),
        ("mod wr(v: string) { out z = v }\non go { out z = 5 }", crate::ir::Type::Int),
        ("mod wr(v: float) { out z = v }\nmod mid() { wr(1.5) }\non go { out z = 5 }\non go2 { mid() }", crate::ir::Type::Float),
    ] {
        assert_eq!(port_ty(&format!("{head}{body}")), Some(want), "{body}");
    }
}

/// A range's bounds run in source order: `for i in a()..b()` called `b`
/// first, so a side effect of `a` that `b` reads was not seen.
#[test]
fn a_range_evaluates_its_bounds_in_source_order() {
    for range in ["one()..two()", "(one()..two()).rev()", "(one()..9).step(two())", "count(one(), from = two())"] {
        let src = format!(
            "in go: exec\nvar q: int = 0\nvar n: int = 0\nout oq: int = q\nout on_: int = n\n\
             mod one() -> int {{\n  q = q + 1\n  return q\n}}\nmod two() -> int {{\n  q = q + 10\n  return q\n}}\n\
             on go {{\n  for i in {range} {{ n = n + i }}\n}}"
        );
        for r in [compile(&src), compile_folded(&src)] {
            assert_no_errors(&r);
            let at_line = |line: u32| {
                r.module
                    .nodes
                    .values()
                    .find(|n| matches!(n.gate_class, gc::VAR_INCREMENT | gc::VAR_SET) && n.source_range.start.line == line)
                    .map(|n| n.id)
                    .unwrap_or_else(|| panic!("{range}: no write on line {line}"))
            };
            let (first, second) = (at_line(7), at_line(11));
            assert!(exec_reaches(&r.module, first, second), "{range}: `one()` runs after `two()`");
            assert!(!exec_reaches(&r.module, second, first), "{range}");
        }
    }
}

/// A receiver with no annotation takes its type from the first send on its
/// channel, so a later send of another type is WS030 naming that send, not a
/// type the receiver "declares".
#[test]
fn a_custom_event_mismatch_names_where_the_receiver_type_came_from() {
    let msg = |src: &str, code: &str| {
        let r = compile(src);
        let d: Vec<_> = r.diagnostics.iter().filter(|d| d.code == code).collect();
        assert_eq!(d.len(), 1, "{code}: {:?}", r.diagnostics);
        assert!(!d[0].message.contains('\u{2014}'), "{}", d[0].message);
        d[0].message.clone()
    };
    let m = msg(
        "in go: exec\nvar n: int = 0\nout onn: int = n\non go {\n  for i in 0..3 { SendCustomEvent(\"lp\", 1.5) }\n  SendCustomEvent(\"lp\", 2)\n}\n\
         on CustomEvent(\"lp\") -> (v) {\n  n = n + 1\n}",
        "WS030",
    );
    assert!(m.contains("first send on this channel (line 5)") && !m.contains("declares"), "{m}");
    let m = msg(
        "in go: exec\nvar n: int = 0\nout onn: int = n\non go { SendCustomEvent(\"lp\", 2.5) }\n\
         on CustomEvent(\"lp\") -> (v: int) {\n  n = v\n}",
        "WS030",
    );
    assert!(m.contains("declares int"), "{m}");
    msg("var n: int = 0\nout onn: int = n\non CustomEvent(\"q\") -> (v) {\n  n = n + 1\n}", "WS042");
    // A first send in another file is named with that file.
    let lib = "in go: exec\non go {\n  SendCustomEvent(\"ch\", 1)\n}";
    let loader = crate::resolve::MemLoader { files: [("celib.ws".to_string(), lib.to_string())].into_iter().collect() };
    let src = "import \"celib\"\nin go2: exec\nvar tot: float = 0.0\nout ot: float = tot\n\
               on go2 { SendCustomEvent(\"ch\", 2.5) }\non CustomEvent(\"ch\") -> (x) { tot = tot + x }";
    let input = crate::CompileInput { source: src, file: "main", module_name: None, fold_mode: crate::FoldMode::Auto };
    let r = crate::compile_with_loader(input, Default::default(), &loader).expect("compiles");
    let m = r.diagnostics.iter().find(|d| d.code == "WS030").map(|d| d.message.clone()).unwrap_or_default();
    assert!(m.contains("first send on this channel (celib.ws line 3)"), "{m}\n{:?}", r.diagnostics);
}

/// A field read of a record-valued `if` or `match` whose arm acts is WS097 in
/// the read's own words (the read is one value; the arms are records), and the
/// rewrite it suggests, the field read in each arm, compiles with the arm's
/// write on its branch only.
#[test]
fn a_field_read_of_an_acting_record_choice_says_so() {
    let head = "enum K { A, B }\nin go: exec\nin c: bool\nvar k: K = K.A\nvar n: int = 0\nout onn: int = n\n\
                var cnt: int = 0\nout ocnt: int = cnt\ntype P = { a: int, b: int }\n\
                mod mk() -> P {\n  cnt = cnt + 1\n  return { a: 1, b: 2 }\n}\n";
    for body in [
        "on go { n = (if c then mk() else { a: 3, b: 4 }).a }",
        "on go { n = (match k { A => mk(), B => { a: 3, b: 4 } }).a }",
    ] {
        let r = compile(&format!("{head}{body}"));
        let d: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS097").collect();
        assert_eq!(d.len(), 1, "{body}: {:?}", r.diagnostics);
        assert!(d[0].message.starts_with("this reads `.a`"), "{}", d[0].message);
    }
    let r = compile(&format!("{head}on go {{ n = if c then mk().a else 3 }}"));
    assert_no_errors(&r);
    let branch = r.module.nodes.values().find(|n| n.gate_class == gc::BRANCH).expect("a Branch").id;
    let bump = r.module.nodes.values().find(|n| n.gate_class == gc::VAR_INCREMENT).expect("the increment").id;
    let (a, b) = (chain_start(&r.module, branch, WirePort::ExecOutA), chain_start(&r.module, branch, WirePort::ExecOutB));
    assert!(exec_reaches(&r.module, a, bump), "the write runs on the taken branch");
    assert!(!exec_reaches(&r.module, b, bump), "and not on the other");
}

/// An `out` after a value-`match` arm that `return`s outside any exec chain is
/// already rejected (WS095/WS007); it has no second writer, so no WS013 that
/// names one.
#[test]
fn an_out_after_a_rejected_early_exit_names_no_other_writer() {
    let r = compile(
        "enum K { A, B }\nin go: exec\nvar k: K = K.A\nchip {\n  let v = match k { A => { return }, B => 2 }\n  out ov: int = v\n}",
    );
    let codes: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == crate::diagnostic::Severity::Error).map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"WS095"), "{codes:?}");
    assert!(!codes.contains(&"WS013") && !codes.contains(&"WS094"), "{codes:?}");
}
