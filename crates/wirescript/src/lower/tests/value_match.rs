//! `match` on a single value: literal, range, string and guarded arms.

use super::*;

fn warnings(r: &LowerResult, code: &str) -> usize {
    r.diagnostics.iter().filter(|d| d.code == code).count()
}

#[test]
fn literal_arms_cost_one_compare_and_one_branch_each() {
    let r = compile(
        "in go: exec\nin s: string\nvar n: int = 0\n\
         on go {\n  match s {\n    \"+\" => { n = 1 }\n    \"-\" => { n = 2 }\n    \"\" => { n = 3 }\n    _ => {}\n  }\n}\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "{:?}", r.diagnostics);
    assert_eq!(gate_count(&r, gc::COMPARE_EQUAL), 3);
    assert_eq!(gate_count(&r, gc::BRANCH), 3);
}

#[test]
fn an_arm_of_alternatives_is_built_once() {
    let r = compile(
        "in go: exec\nin s: string\nvar n: int = 0\n\
         on go {\n  match s {\n    \"a\" | \"b\" | \"c\" => { n = 1 }\n    _ => { n = 2 }\n  }\n}\n",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, gc::BRANCH), 1);
    assert_eq!(gate_count(&r, "BrickComponentType_Exec_Var_Set"), 2);
    assert_eq!(gate_count(&r, "BrickComponentType_Expr_LogicalOR"), 2);
}

#[test]
fn variants_of_one_alternative_build_their_arm_once() {
    let r = compile(
        "enum Dir { N, E, S, W }\nin go: exec\nvar d: Dir = Dir.N\nvar n: int = 0\n\
         on go {\n  match d {\n    N | S => { n = 1 }\n    E | W => { n = 2 }\n  }\n}\n",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, "BrickComponentType_Exec_Var_Set"), 2);
}

#[test]
fn a_string_pattern_binds_its_holes_from_case_sensitive_gates() {
    let r = compile(
        "in go: exec\nin s: string\nvar res: string = \"\"\n\
         on go {\n  match s {\n    \"say \" .. rest => { res = rest }\n    key .. \"=\" .. value => { res = key .. value }\n    _ => {}\n  }\n}\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "{:?}", r.diagnostics);
    assert_eq!(gate_count(&r, gc::STRING_STARTS_WITH), 1);
    assert_eq!(gate_count(&r, gc::STRING_FIND), 1);
    assert_eq!(gate_count(&r, gc::STRING_SUBSTRING), 3);
    for class in [gc::STRING_STARTS_WITH, gc::STRING_FIND] {
        let id = find_gate(&r, class);
        let wired = r.module.wires.iter().find(|w| w.target.node_id == id && w.target.port == WirePort::BCaseSensitive);
        let case = match wired {
            Some(w) => r.module.nodes[&w.source.node_id].properties.get(&crate::intern::sym::VALUE),
            None => r.module.nodes[&id].properties.get(&WirePort::BCaseSensitive.sym()),
        };
        assert_eq!(case, Some(&crate::ir::Literal::Bool(true)), "{class}");
    }
}

#[test]
fn a_template_pattern_builds_what_its_dotdot_spelling_does() {
    let shape = |pattern: &str| {
        let r = compile(&format!(
            "in s: string\nout o = match s {{\n  {pattern} => who .. item,\n  _ => \"\",\n}}\n"
        ));
        assert_no_errors(&r);
        wiring_shape(&r.module)
    };
    assert_eq!(
        shape(r#""give ${item} to ${who}""#),
        shape(r#""give " .. item .. " to " .. who"#)
    );
}

#[test]
fn a_template_hole_named_like_a_value_in_scope_warns() {
    let r = compile(
        "in s: string\nin player: string\nout o = match s {\n  \"hi ${player}\" => 1,\n  \"bye ${p}\" if p == player => 2,\n  _ => 3,\n}\n",
    );
    assert_no_errors(&r);
    assert_eq!(warnings(&r, "WS123"), 1, "{:?}", r.diagnostics);
    let dotdot = compile("in s: string\nin player: string\nout o = match s {\n  \"hi \" .. player => 1,\n  _ => 3,\n}\n");
    assert_eq!(warnings(&dotdot, "WS123"), 0, "{:?}", dotdot.diagnostics);
    assert!(errs("in s: string\nout o = match s { \"n${1 + 1}\" => 1, _ => 2 }\n").iter().any(|c| c == "WS122"));
}

#[test]
fn a_value_match_as_a_value_is_a_select_chain() {
    let r = compile(
        "in x: int\nout kind = match x {\n  0 => 10,\n  1..10 => 20,\n  n if n > 100 => 30,\n  _ => 40,\n}\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "{:?}", r.diagnostics);
    assert_eq!(gate_count(&r, gc::SELECT), 3);
    assert_eq!(gate_count(&r, gc::BRANCH), 0);
}

#[test]
fn a_constant_scrutinee_builds_only_the_taken_arm() {
    let r = compile(
        "const OP = \"-\"\nin go: exec\nvar n: int = 0\n\
         on go {\n  match OP {\n    \"+\" => { n = 1 }\n    \"-\" => { n = 2 }\n    _ => { n = 3 }\n  }\n}\n",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, gc::COMPARE_EQUAL), 0);
    assert_eq!(gate_count(&r, gc::BRANCH), 0);
    assert_eq!(written_value(&r, "n"), Some(crate::ir::Literal::Int(2)));
}

#[test]
fn let_else_takes_a_string_pattern() {
    let r = compile(
        "in go: exec\nin s: string\nvar res: string = \"\"\n\
         on go {\n  let \"go \" .. dest = s else { return }\n  res = dest\n}\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "{:?}", r.diagnostics);
    assert_eq!(gate_count(&r, gc::STRING_STARTS_WITH), 1);
    assert_eq!(gate_count(&r, gc::BRANCH), 1);
}

#[test]
fn if_let_takes_a_literal_in_a_payload() {
    let r = compile(
        "enum Opt { Some(int), None }\nin go: exec\nvar o: Opt = Opt.None\nvar n: int = 0\n\
         on go {\n  if let Some(0) = o { n = 1 } else { n = 2 }\n}\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "{:?}", r.diagnostics);
    assert_eq!(gate_count(&r, gc::BRANCH), 2);
}

#[test]
fn sliced_inside_another_loop_warns_once() {
    let written = compile(
        "var t: int = 0\nin go: exec\non go {\n  for i in 0..3 {\n    @sliced for j in 0..3 { t += j }\n  }\n}\n",
    );
    assert_eq!(warnings(&written, "WS119"), 1, "{:?}", written.diagnostics);
    let inlined = compile(
        "var t: int = 0\nin go: exec\nmod inner() {\n  @sliced for j in 0..3 { t += j }\n}\n\
         on go {\n  for i in 0..3 { inner() }\n}\n",
    );
    assert_eq!(warnings(&inlined, "WS119"), 1, "{:?}", inlined.diagnostics);
    let outer = compile("var t: int = 0\nin go: exec\non go {\n  @sliced for i in 0..3 {\n    for j in 0..3 { t += j }\n  }\n}\n");
    assert_eq!(warnings(&outer, "WS119"), 0, "{:?}", outer.diagnostics);
}

#[test]
fn value_match_mistakes_are_errors() {
    for (src, code) in [
        ("in s: string\nout o = match s { \"a\" => 1 }\n", "WS054"),
        ("in s: string\nout o = match s { x | \"b\" => 1, _ => 2 }\n", "WS121"),
        ("in s: string\nout o = match s { a .. b => 1, _ => 2 }\n", "WS122"),
        ("in n: int\nout o = match n { \"a\" => 1, _ => 2 }\n", "WS120"),
        ("in n: int\nout o = match n { Red => 1, _ => 2 }\n", "WS120"),
        ("in n: int\nout o = match n { 1.5 => 1, _ => 2 }\n", "WS120"),
        ("in v: vector\nout o = match v { _ => 1 }\n", "WS066"),
    ] {
        assert!(errs(src).iter().any(|c| c == code), "{src}: {:?}", errs(src));
    }
    assert!(errs("in f: float\nout o = match f { 1 => 1, 2.5 => 2, _ => 3 }\n").is_empty());
    assert!(errs("in b: bool\nout o = match b { true => 1, false => 2 }\n").is_empty());
}
