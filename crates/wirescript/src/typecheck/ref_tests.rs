//! Ref values and the `brick` type: the typing rules and their diagnostics.

use super::*;
use crate::parser::parse;

fn tc(src: &str) -> TypeCheckResult {
    let p = parse(src, "test");
    assert!(
        p.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        p.diagnostics
    );
    typecheck(&p.ast, &crate::typecheck::CeSlotMap::default())
}

fn codes(src: &str) -> Vec<String> {
    tc(src).diagnostics.into_iter().map(|d| d.code).collect()
}

fn errors(src: &str) -> Vec<String> {
    tc(src)
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect()
}

fn assert_clean(src: &str) {
    let e = errors(src);
    assert!(e.is_empty(), "expected no errors for:\n{src}\ngot {e:?}");
}

fn assert_code(src: &str, code: &str) {
    let c = codes(src);
    assert!(
        c.iter().any(|c| c == code),
        "expected {code} for:\n{src}\ngot {c:?}"
    );
}

#[test]
fn brick_is_a_type_that_copies_but_does_not_convert() {
    assert_clean("var a: brick\nvar b: brick\nin go: exec\non go { b = a }");
    let e = errors("var a: brick\nvar n: int = 0\nin go: exec\non go { n = a }");
    assert!(
        e.iter()
            .any(|m| m.starts_with("[WS003]") && m.contains("brick")),
        "a brick is not an int: {e:?}"
    );
}

#[test]
fn parenthesized_type_groups_rather_than_building_a_tuple() {
    use crate::ast::{TopDecl, TypeExpr};
    let p = parse(
        "var p: (*int)[]\nvar t: (int, float)\nvar g: (int)\n",
        "test",
    );
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let typ = |i: usize| match &p.ast.decls[i] {
        TopDecl::Var(v) => v.typ.clone().expect("annotated"),
        other => panic!("not a var: {other:?}"),
    };
    assert!(
        matches!(typ(0), TypeExpr::Array { inner, .. } if matches!(*inner, TypeExpr::Ref { .. })),
        "`(*int)[]` is an array of refs"
    );
    assert!(matches!(typ(1), TypeExpr::Tuple { fields, .. } if fields.len() == 2));
    assert!(matches!(typ(2), TypeExpr::Name { name, .. } if name == "int"));
}

#[test]
fn ref_helpers_classify_types() {
    use crate::ir::RefKind;
    let r = |t: Type| Type::Ref(Box::new(t));
    let arr = |t: Type| Type::Array(Box::new(t));
    assert_eq!(r(Type::Int).ref_kind(), Some(RefKind::Var));
    assert_eq!(arr(Type::Int).ref_kind(), Some(RefKind::Array));
    assert_eq!(r(arr(Type::Int)).ref_kind(), Some(RefKind::Array));
    assert_eq!(
        Type::Map(Box::new(Type::String), Box::new(Type::Int)).ref_kind(),
        Some(RefKind::Map)
    );
    assert_eq!(Type::Brick.ref_kind(), Some(RefKind::Brick));
    assert_eq!(Type::Int.ref_kind(), None);
    assert!(Type::Color.is_ref_target() && Type::Brick.is_ref_target());
    assert!(!Type::Zone.is_ref_target() && !r(Type::Int).is_ref_target());
    assert_eq!(r(arr(Type::Int)).container_ref_as_value(), arr(Type::Int));
    assert_eq!(r(Type::Int).container_ref_as_value(), r(Type::Int));
    // The two spellings print differently.
    assert_eq!(arr(r(Type::Int)).to_string(), "(*int)[]");
    assert_eq!(r(arr(Type::Int)).to_string(), "*int[]");
    assert_eq!(Type::Brick.to_string(), "brick");
}

#[test]
fn refs_bricks_and_nested_containers_are_storable() {
    for ty in [
        "*int",
        "*color",
        "*int[]",
        "*Map<string, int>",
        "brick",
        "(*int)[]",
        "int[][]",
        "brick[]",
        "Map<string, *color>",
        "Map<string, int[]>",
        "Map<int, brick>",
        // The game keys a map by the brick it names.
        "Map<brick, int>",
        "int[][][]",
    ] {
        let src = format!("var c: {ty}\nin go: exec\non go {{ }}\n");
        let c = codes(&src);
        assert!(
            !c.iter().any(|c| c == "WS025"),
            "`var c: {ty}` is storable: {c:?}"
        );
    }
}

#[test]
fn what_has_no_single_ref_stays_unstorable() {
    for (ty, want) in [
        ("*zone", "WS025"),
        ("(*zone)[]", "WS025"),
        ("exec[][]", "WS025"),
        ("(*int[])[]", "WS025"),
        ("Map<*int, int>", "WS025"),
        ("Map<int[], int>", "WS025"),
    ] {
        let src = format!("var c: {ty}\nin go: exec\non go {{ }}\n");
        assert_code(&src, want);
    }
    // A container of records is several parallel containers, so it has no
    // single ref to put in an outer array.
    assert_code(
        "type P = { x: int, y: int }\nvar c: P[][]\nin go: exec\non go { }\n",
        "WS025",
    );
}

#[test]
fn a_ref_to_a_ref_is_ws086() {
    // `**` lexes as the power operator; the type parser reads it as two refs.
    assert_code("var c: **int\n", "WS086");
    assert_code("var c: *(*int)\n", "WS086");
    assert_code("type R = *int\nvar c: *R\n", "WS086");
    assert_code(
        "var a: int = 0\nvar h: *int\nvar g: *int\nin go: exec\non go { h = &a\n g = &h }",
        "WS086",
    );
    assert_code(
        "var a: int = 0\nvar px: (*int)[]\nvar g: *int\nin go: exec\non go { px.push(&a)\n g = &px[0] }",
        "WS086",
    );
    // A mod parameter's type is resolved for the signature and for the body,
    // and reported once.
    assert_eq!(codes("mod f(p: **int) { }\n"), ["WS086"]);
}

#[test]
fn an_element_of_a_value_array_has_no_ref() {
    assert_code(
        "var xs: int[]\nvar h: *int\nin go: exec\non go { xs.push(1)\n h = &xs[0] }",
        "WS008",
    );
    // Each is reported once: an element of a temporary, and a field of an
    // element of a record array.
    for body in ["h = &[1, 2][0]", "h = &pts[0].x"] {
        let src = format!(
            "type P = {{ x: int }}\nvar pts: P[]\nvar h: *int\nin go: exec\non go {{ {body} }}"
        );
        assert_eq!(codes(&src), ["WS008"], "{body}");
    }
}

#[test]
fn a_local_ref_var_needs_an_initializer() {
    // Review Focus 5.
    assert_code(
        "var a: int = 0\nin go: exec\non go { var h: *int\n h = &a }",
        "WS025",
    );
    assert_code("in go: exec\non go { var b: brick }", "WS025");
    assert_clean("var a: int = 0\nin go: exec\non go { var h: *int = &a\n h = &a }");
    assert_clean("var a: int = 0\nin go: exec\non go { static var h: *int\n h = &a }");
    // Top-level storage starts null and needs no initializer.
    assert_clean("var h: *int\nvar b: brick\n");
}

#[test]
fn an_unannotated_var_keeps_a_ref_values_type() {
    // `var h = px[0]` holds the ref, so a later `h = 5` is a mismatch.
    assert_code(
        "var px: (*int)[]\nin go: exec\non go { var h = px[0]\n h = 5 }",
        "WS003",
    );
}

const REFS: &str = "var a: int = 0\nvar b: int = 0\nvar n: int = 0\nvar f: *float\n\
var h: *int\nvar k: *int\nvar row0: int[]\nvar rows: int[][]\nvar x: brick\nvar y: brick\n\
var c: bool = true\nin go: exec\n";

#[test]
fn a_ref_var_reads_as_a_ref_and_a_ref_param_as_its_target() {
    assert_clean(&format!("{REFS}on go {{ h = &a\n k = h }}"));
    assert_clean(&format!(
        "{REFS}mod bump(p: *int) {{ p = p + 1 }}\non go {{ bump(a) }}"
    ));
    // A field of a `*Record` parameter is a place, not a ref value.
    assert_clean(
        "type P = { a: int }\nvar r: P = { a: 0 }\nmod bump(q: *P, d: int) { q.a = q.a + d }\n\
         in go: exec\non go { bump(r, 1) }",
    );
    // A var holding an array ref reads as the array.
    assert_clean(&format!(
        "{REFS}var cur: *int[]\non go {{ cur = rows[0]\n cur.push(1)\n n = cur.length() }}"
    ));
}

#[test]
fn a_ref_value_is_not_a_number() {
    // Review Focus 2.
    assert_code(&format!("{REFS}on go {{ n = h + 1 }}"), "WS004");
    assert_code(&format!("{REFS}on go {{ n = -h }}"), "WS004");
    assert_code(&format!("{REFS}on go {{ let m: int = h }}"), "WS003");
    assert_code(&format!("{REFS}on go {{ n = h }}"), "WS003");
    assert_code(&format!("{REFS}on go {{ h = 5 }}"), "WS003");
    assert_code(&format!("{REFS}on go {{ h = f }}"), "WS003");
    assert_code(&format!("{REFS}on go {{ x = h }}"), "WS003");
}

#[test]
fn refs_compare_only_within_their_kind() {
    assert_clean(&format!(
        "{REFS}on go {{ c = h == k\n c = h != &a\n c = rows[0] == row0\n c = x == y }}"
    ));
    assert_code(&format!("{REFS}on go {{ c = h == 1 }}"), "WS084");
    assert_code(&format!("{REFS}on go {{ c = 1 == h }}"), "WS084");
    assert_code(&format!("{REFS}on go {{ c = x == 0 }}"), "WS084");
    assert_code(&format!("{REFS}on go {{ c = h == f }}"), "WS003");
    assert_code(&format!("{REFS}on go {{ c = h == x }}"), "WS003");
    assert_code(&format!("{REFS}on go {{ c = h < k }}"), "WS004");
    assert_code(&format!("{REFS}on go {{ c = x < y }}"), "WS004");
    assert_code(&format!("{REFS}on go {{ c = !h }}"), "WS004");
    // Each accepted comparison resolves to a compare gate for lowering.
    for (cmp, gate) in [
        ("h == k", "CompareEqual"),
        ("h != &a", "CompareNotEqual"),
        ("rows[0] == row0", "CompareEqual"),
        ("x == y", "CompareEqual"),
    ] {
        let src = format!("{REFS}on go {{ c = {cmp} }}");
        let r = tc(&src);
        let at = src.find(cmp).unwrap();
        let rule = r
            .op_resolutions
            .iter()
            .find(|((_, start, end), _)| *start == at && *end == at + cmp.len())
            .map(|(_, rule)| rule.gate_class);
        assert!(rule.is_some_and(|g| g.ends_with(gate)), "{cmp}: {rule:?}");
    }
}

#[test]
fn a_ref_compared_against_any_still_ws004() {
    // `Any` is the error-fallback type, not an operator wildcard: an out-of-range
    // record `.9` pick returns it silently, so a ref (or any operand) compared
    // against it still reports WS004 through overload resolution.
    let decls = "type P = { a: int }\nvar r: P = { a: 0 }\n";
    assert_code(&format!("{REFS}{decls}on go {{ c = h == r.9 }}"), "WS004");
    assert_code(
        &format!("{REFS}{decls}on go {{ c = row0 == r.9 }}"),
        "WS004",
    );
    assert_code(&format!("{REFS}{decls}on go {{ c = x != r.9 }}"), "WS004");
}

#[test]
fn an_array_ref_let_mismatch_keeps_the_lenient_warning() {
    // The no-conversion rule covers a `*T` var ref and `brick` only: an array or
    // map ref mismatch in a `let` annotation keeps its WS016, like any other
    // non-coercing annotation.
    let r = tc("var row0: int[]\nlet a1: float[] = row0\n");
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS003"),
        "an array-ref let mismatch must not escalate to WS003: {:?}",
        r.diagnostics
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS016"),
        "an array-ref let mismatch should still warn WS016: {:?}",
        r.diagnostics
    );
}

#[test]
fn an_if_expression_selects_between_refs() {
    assert_clean(&format!("{REFS}on go {{ h = if c then &a else &b }}"));
    assert_clean(&format!(
        "{REFS}var r1: int[]\non go {{ let r = if c then row0 else r1\n r.push(1) }}"
    ));
    assert_clean(&format!("{REFS}on go {{ y = if c then x else y }}"));
    // zone/teleport stay reference-only.
    assert_code(
        "in z1: zone\nin z2: zone\nvar c: bool = true\nlet z = if c then z1 else z2\n",
        "WS031",
    );
}

#[test]
fn deref_reads_and_writes_through_a_scalar_ref() {
    assert_clean(&format!(
        "{REFS}on go {{ h = &a\n *h = 5\n n = *h + 1\n *h += 2\n var px: (*int)[] = []\n px.push(&b)\n *px[0] = 3 }}"
    ));
    assert_code(&format!("{REFS}on go {{ *h = 1.5 }}"), "WS003");
    assert_code(&format!("{REFS}let v = *h\n"), "WS006");
}

#[test]
fn deref_does_not_apply_to_array_refs_or_bricks() {
    assert_code(&format!("{REFS}on go {{ let r = *rows[0] }}"), "WS085");
    assert_code(&format!("{REFS}on go {{ *rows[0] = row0 }}"), "WS085");
    assert_code(&format!("{REFS}on go {{ let v = *x }}"), "WS085");
}

#[test]
fn a_ref_value_fits_only_a_matching_ref_parameter() {
    // Review Focus 2, call side.
    let m = "mod set7(p: *int) { p = 7 }\nmod setf(p: *float) { p = 7.0 }\nmod take(v: int) { }\n";
    assert_clean(&format!(
        "{REFS}{m}on go {{ set7(h)\n set7(a)\n set7(&a) }}"
    ));
    assert_clean(&format!(
        "{REFS}{m}var px: (*int)[]\non go {{ set7(px[0]) }}"
    ));
    assert_code(&format!("{REFS}{m}on go {{ setf(h) }}"), "WS003");
    assert_code(&format!("{REFS}{m}on go {{ take(h) }}"), "WS003");
    assert_code(&format!("{REFS}{m}on go {{ take(&a) }}"), "WS003");
    // A ref value fits a genuine `any`/`Opaque` pass-through (custom-event
    // data, `Opaque`/`Change` rerouters, a message port) but not a concrete
    // scalar-typed builtin parameter.
    assert_code(&format!("{REFS}on go {{ n = sqrt(h) }}"), "WS003");
    assert_clean(&format!(
        "{REFS}var p0: color\non go {{ SendCustomEvent(\"reg\", &p0)\n let o = Opaque(h) }}"
    ));
    // Containers: a ref into a value array, a value into a ref array.
    assert_code(
        &format!("{REFS}var xs: int[]\non go {{ xs.push(&a) }}"),
        "WS003",
    );
    assert_eq!(
        codes(&format!("{REFS}var px: (*int)[]\non go {{ px.push(5) }}")),
        vec!["WS003"]
    );
    assert_eq!(
        codes(&format!("{REFS}var px: (*int)[]\non go {{ px.push(a) }}")),
        vec!["WS003"]
    );
    assert_code(
        &format!("{REFS}var px: (*int)[]\non go {{ px.push(x) }}"),
        "WS003",
    );
    // An array value binds an array parameter.
    assert_clean(&format!(
        "{REFS}mod len(xs: int[]) -> int {{ return xs.length() }}\non go {{ n = len(rows[0]) }}"
    ));
}

#[test]
fn refs_and_bricks_are_custom_event_data() {
    let src = "var p0: color\nvar px: (*color)[]\nvar b: brick\nvar row: int[]\nin go: exec\n\
               on CustomEvent(\"reg\") -> (r: *color) { px.push(r)\n *r = Color(1.0, 0.0, 0.0) }\n\
               on CustomEvent(\"brk\") -> (x: brick) { }\n\
               on CustomEvent(\"arr\") -> (a: int[]) { a.push(1) }\n\
               on go { SendCustomEvent(\"reg\", &p0)\n SendCustomEvent(\"brk\", b)\n SendCustomEvent(\"arr\", row) }";
    let c = codes(src);
    assert!(!c.iter().any(|c| c == "WS045" || c == "WS030"), "{c:?}");
    assert_clean(src);
}

#[test]
fn event_data_kinds_must_match() {
    assert_code(
        "var p0: color\nvar n: float = 0.0\nin go: exec\n\
         on CustomEvent(\"reg\") -> (r: *color) { }\non go { SendCustomEvent(\"reg\", n) }",
        "WS030",
    );
    assert_code(
        "var b: brick\nin go: exec\n\
         on CustomEvent(\"reg\") -> (r: *color) { }\non go { SendCustomEvent(\"reg\", b) }",
        "WS030",
    );
}

#[test]
fn unannotated_custom_event_receiver_infers_ref_kind() {
    // These receiver parameters have no annotation (`-> (r)`), so their type comes
    // from the WS042 slot inference (`infer_custom_event_slots`, run by
    // `typecheck_with_inference`; `tc()` never fills `ce_slots`). It adopts the
    // sender's ref kind, not the pointee or element type, so a deref or push in
    // the body sees the ref and WS030 compares like with like.
    let src = "var p0: color\nvar px: (*color)[]\nvar b: brick\nvar row: int[]\nin go: exec\n\
               on CustomEvent(\"reg\") -> (r) { *r = Color(1.0, 0.0, 0.0)\n px.push(r) }\n\
               on CustomEvent(\"brk\") -> (x) { }\n\
               on CustomEvent(\"arr\") -> (a) { a.push(1) }\n\
               on go { SendCustomEvent(\"reg\", &p0)\n SendCustomEvent(\"brk\", b)\n SendCustomEvent(\"arr\", row) }";
    let p = parse(src, "test");
    assert!(
        p.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        p.diagnostics
    );
    let (r, _map) = typecheck_with_inference(&p.ast);
    let errs: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errs.is_empty(), "expected no errors:\n{src}\ngot {errs:?}");
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.code == "WS045" || d.code == "WS030"),
        "{:?}",
        r.diagnostics
    );
}

#[test]
fn a_ref_is_never_a_constant() {
    assert_code("var a: int = 0\nconst r = &a\n", "WS046");
    assert_code(
        "var a: int = 0\nmod f(p: const *int) { }\nin go: exec\non go { f(&a) }",
        "WS046",
    );
}

#[test]
fn an_element_receivers_arguments_check_against_the_element_type() {
    assert_clean("var rows: int[][]\nin go: exec\non go { rows[0].push(7) }");
    assert_code(
        "var rows: int[][]\nin go: exec\non go { rows[0].push(\"x\") }",
        "WS003",
    );
    assert_code(
        "var cur: *int[]\nin go: exec\non go { cur.push(\"x\") }",
        "WS003",
    );
}

#[test]
fn a_generic_element_receiver_is_checked_per_type_combination() {
    assert_clean(
        "mod pushAll<T>(rows: T[][], v: T) { rows[0].push(v) }\n\
         var a: int[][]\nvar b: string[][]\nin go: exec\non go { pushAll(a, 1)\n pushAll(b, \"x\") }",
    );
}

const REF_RECORD: &str = "var a: int = 0\nvar n: int = 0\ntype R = { p: *int, n: int }\n\
mod M7(q: *int) { q = 7 }\nin go: exec\n";

#[test]
fn a_ref_field_of_a_record_value_reads_as_a_ref() {
    // A record VALUE (a let-bound literal, a map read) carries the ref itself;
    // only a `*Record` parameter's field is a place that reads its target.
    assert_code(
        "var a: int = 0\nvar m: Map<string, *int>\nvar n: int = 0\nin go: exec\n\
         on go { m.set(\"k\", &a)\n let e = m.get(\"k\")\n n = e.Value + 1 }",
        "WS004",
    );
    assert_code(
        &format!("{REF_RECORD}on go {{ let rr: R = {{ p: &a, n: 1 }}\n let s: string = rr.p }}"),
        "WS003",
    );
    assert_code(
        &format!("{REF_RECORD}var h: *int\non go {{ let rr: R = {{ p: &a, n: 1 }}\n h = &rr.p }}"),
        "WS086",
    );
    assert_clean(&format!(
        "{REF_RECORD}var h: *int\non go {{ let rr: R = {{ p: &a, n: 1 }}\n h = rr.p\n M7(rr.p)\n n = *rr.p + rr.n }}"
    ));
    // A `*Record` parameter's fields still read as their targets.
    assert_clean(
        "type P = { a: int }\nvar r: P = { a: 0 }\nvar n: int = 0\n\
         mod get(q: *P) -> int { return q.a + 1 }\nin go: exec\non go { n = get(r) }",
    );
}

#[test]
fn a_ref_parameter_needs_storage_or_a_ref_value_behind_a_field() {
    assert_code(
        &format!("{REF_RECORD}on go {{ let rr: R = {{ p: &a, n: 1 }}\n M7(rr.n) }}"),
        "WS008",
    );
    assert_code(
        &format!(
            "{REF_RECORD}var mi: Map<string, int>\non go {{ mi.set(\"k\", 1)\n M7(mi.get(\"k\").Value) }}"
        ),
        "WS008",
    );
    assert_code(
        &format!(
            "{REF_RECORD}var mi: Map<string, int>\non go {{ let e = mi.get(\"k\")\n M7(e.Value) }}"
        ),
        "WS008",
    );
    // A record var's field and a `*Record` parameter's field are storage.
    assert_clean(
        "type P = { x: int }\nvar r: P = { x: 0 }\nmod M7(q: *int) { q = 7 }\n\
         mod fwd(s: *P) { M7(s.x) }\nin go: exec\non go { M7(r.x)\n fwd(r) }",
    );
    assert_clean(&format!(
        "{REF_RECORD}var m: Map<string, *int>\non go {{ m.set(\"k\", &a)\n let e = m.get(\"k\")\n M7(e.Value) }}"
    ));
}

#[test]
fn a_ref_output_carries_a_ref_and_a_value_output_a_value() {
    let head = "var c: int = 0\n";
    assert_clean(&format!("{head}out r: *int = &c\nout v: int = c"));
    assert_code(&format!("{head}out r: *int = c"), "WS003");
    assert_code(&format!("{head}out v: int = &c"), "WS003");
    // A signature output: the chip body's `out` answers to the declared type.
    assert_clean("chip Src(x: *int) -> (r: *int) { out r = &x }");
    assert_code("chip Src(x: *int) -> (r: *int) { out r = x }", "WS003");
    assert_code("chip Src(x: *int) -> (r: int) { out r = &x }", "WS003");
    assert_code(
        "chip Src(x: *int) -> (r: *int) { out r: *int = x }",
        "WS003",
    );
    // Array and map refs are used directly, so `*int[]` and `int[]` agree.
    assert_clean("var xs: int[]\nout a: *int[] = xs\nout b: int[] = xs\nout d: *int[] = &xs");
}

#[test]
fn a_line_starting_with_a_deref_is_a_new_statement_and_is_flagged() {
    let r = tc(
        "var a: int = 0\nvar b: int = 0\nvar n: int = 0\nin go: exec\non go {\n  n = a\n  * b\n}",
    );
    let w: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code == "WSP001" && d.severity == Severity::Warning)
        .collect();
    assert_eq!(
        w.len(),
        1,
        "the dangling `* b` line is flagged: {:?}",
        r.diagnostics
    );
    assert!(w[0].message.contains("`*b`"), "{}", w[0].message);
    // The trailing-operator form continues, and a deref that is used is quiet.
    assert!(
        tc(
            "var a: int = 0\nvar b: int = 0\nvar h: *int\nvar n: int = 0\nin go: exec\n\
            on go {\n  n = a *\n    b\n  *h = 5\n  n = *h\n}"
        )
        .diagnostics
        .is_empty()
    );
}

#[test]
fn a_record_let_that_bundles_vars_reads_its_fields_as_places() {
    // `{ cpsr, skip }` names storage, so its `*T` fields read their targets
    // and alias them at a `*T` parameter, whatever the let is annotated with.
    let head = "type Cpu = { regs: int[], cpsr: *int, skip: *bool }\n\
                var regs: int[]\nvar cpsr: int = 0\nvar skip: bool = false\nvar n: int = 0\n\
                mod bump(p: *int) { p = p + 1 }\nmod take(b: bool) { }\nin go: exec\n";
    for bind in [
        "let cpu: Cpu = { regs, cpsr, skip }",
        "let cpu = { regs, cpsr, skip }",
    ] {
        assert_clean(&format!(
            "{head}on go {{ {bind}\n take(cpu.skip)\n n = cpu.cpsr + 1\n bump(cpu.cpsr)\n cpu.regs.push(1) }}"
        ));
    }
    // A copy of the bundle, and a field named from a var, are places too.
    assert_clean(&format!(
        "{head}on go {{ let cpu = {{ regs, cpsr: cpsr, skip }}\n let c2 = cpu\n bump(c2.cpsr)\n n = c2.cpsr }}"
    ));
    // A map read and a literal's `&x` are values, even beside a place.
    assert_code(
        &format!("{head}var a: int = 0\non go {{ let mix = {{ cpsr, p: &a }}\n n = mix.p + 1 }}"),
        "WS004",
    );
    assert_clean(&format!(
        "{head}var a: int = 0\non go {{ let mix = {{ cpsr, p: &a }}\n n = mix.cpsr + *mix.p }}"
    ));
}

#[test]
fn a_ref_output_of_a_multi_output_call_is_a_ref_value() {
    let head = "var a: int = 0\nvar n: int = 0\n\
                chip C(x: *int) -> (r: *int, k: int) { out r = &x\n out k = 1 }\nin go: exec\n";
    assert_code(
        &format!("{head}on go {{ let t = C(a)\n n = t.r + 1 }}"),
        "WS004",
    );
    assert_clean(&format!(
        "{head}on go {{ let t = C(a)\n *t.r = 5\n n = t.k }}"
    ));
}

#[test]
fn a_projection_of_a_place_bundle_reads_its_places() {
    let head = "type Cpu = { regs: int[], cpsr: *int, skip: *bool }\n\
                var regs: int[]\nvar cpsr: int = 0\nvar skip: bool = false\nvar n: int = 0\n\
                mod bump(p: *int) { p = p + 1 }\nin go: exec\n";
    assert_clean(&format!(
        "{head}on go {{ let o = {{ c: {{ regs, cpsr, skip }} }}\n let s = o.c\n n = s.cpsr + 1\n bump(s.cpsr) }}"
    ));
    assert_clean(&format!(
        "{head}on go {{ let t = ({{ regs, cpsr, skip }}, 1)\n let s = t.0\n n = s.cpsr + 1 }}"
    ));
    // A value field keeps its kind through the projection.
    assert_code(
        &format!(
            "{head}var a: int = 0\non go {{ let o = {{ c: {{ p: &a }} }}\n let s = o.c\n n = s.p + 1 }}"
        ),
        "WS004",
    );
    assert_code(
        &format!("{head}var a: int = 0\non go {{ let o = {{ c: {{ p: &a }} }}\n n = o.c.p + 1 }}"),
        "WS004",
    );
}

#[test]
fn a_ref_value_fills_a_ref_field_of_a_record_argument() {
    let head = "type R = { p: *int }\nvar a: int = 0\nvar hs: (*int)[]\nvar n: int = 0\n\
                chip C(go: exec, q: R) { on go { n = q.p + 1 } }\nin go: exec\n";
    for arg in ["{ p: &a }", "{ p: hs[0] }", "{ p: e }"] {
        assert_clean(&format!("{head}on go {{ let e = hs[0]\n C(go, {arg}) }}"));
    }
    assert_code(&format!("{head}on go {{ C(go, {{ p: 5 }}) }}"), "WS008");
}

#[test]
fn a_ref_to_a_record_of_refs_is_that_record() {
    assert_clean("type P = { a: int, b: int }\nchip S2(q: *P) -> (r: *P) { out r = &q }");
}

const CPU_NS: &str = "type Cpu = { regs: int[], cpsr: *int, skip: *bool }\n\
var regs: int[]\nvar cpsr: int = 0\nvar skip: bool = false\n\
let cpu = { regs, cpsr, skip }\nlet { cpsr: cp } = cpu\nlet o = { c: cpu }\nlet { c } = o\n";

fn tc_multi(entry: &str, deps: &[(&str, &str)]) -> TypeCheckResult {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: deps
            .iter()
            .map(|(k, v)| (format!("{k}.ws"), v.to_string()))
            .collect(),
    };
    let resolved = resolve(entry, "main", &loader);
    typecheck(&resolved.ast, &crate::typecheck::CeSlotMap::default())
}

fn multi_errors(entry: &str, deps: &[(&str, &str)]) -> Vec<String> {
    tc_multi(entry, deps)
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect()
}

#[test]
fn a_destructured_namespace_member_keeps_its_place() {
    let head = "import * as u from \"u\"\nvar h: *int\nvar n: int = 0\nmod M7(v: *int) { v = v + 1 }\nin go: exec\n";
    for body in [
        "n = u.cp + 1",
        "M7(u.cp)",
        "h = &u.cp",
        "M7(u.c.cpsr)",
        "n = u.c.cpsr + 1",
    ] {
        let e = multi_errors(&format!("{head}on go {{ {body} }}"), &[("u", CPU_NS)]);
        assert!(e.is_empty(), "{body}: {e:?}");
    }
    let e = multi_errors(&format!("{head}on go {{ h = u.cp }}"), &[("u", CPU_NS)]);
    assert!(
        e.iter().any(|e| e.starts_with("[WS003]")),
        "a place is not a ref value: {e:?}"
    );
}

#[test]
fn a_ref_of_a_place_is_its_vars_ref() {
    let head = "type Cpu = { regs: int[], cpsr: *int, skip: *bool }\n\
                var regs: int[]\nvar cpsr: int = 0\nvar skip: bool = false\nvar h: *int\nin go: exec\n";
    for body in [
        "let s = { regs, cpsr, skip }\n h = &s.cpsr",
        "let { cpsr: cp } = { regs, cpsr, skip }\n h = &cp",
        "let t = (cpsr, 1)\n h = &t.0",
    ] {
        assert_clean(&format!("{head}on go {{ {body} }}"));
    }
    // A ref value has no ref of its own.
    assert_code(
        &format!("{head}var a: int = 0\non go {{ let r = {{ p: &a }}\n h = &r.p }}"),
        "WS086",
    );
}

#[test]
fn a_tuple_of_vars_bundles_places() {
    assert_clean(
        "var x: int = 0\nvar y: int = 0\nmod M7(v: *int) { v = v + 1 }\nin go: exec\n\
         on go { let t = (y, x)\n M7(t.1) }",
    );
}

#[test]
fn an_enum_cannot_cross_a_port() {
    let head = "enum E { A, B(int) }\n";
    for decl in [
        "in e: E",
        "var v: E = E.A\nout o: E = v",
        "var v: E = E.A\nout o = v",
        "chip C(e: E) { }",
        "chip C() -> (o: E) { out o = E.A }",
    ] {
        assert_code(&format!("{head}{decl}"), "WS049");
    }
    // A ref to one distributes over its slots, a mod inlines, and a game enum
    // is one value.
    assert_clean(&format!(
        "{head}chip C(e: *E) {{ e = E.A }}\nmod M(e: E) -> int {{ return e.Discriminant }}"
    ));
    assert_clean("in ease: EasingFunction");
}

fn ws049_count(src: &str) -> usize {
    tc(src)
        .diagnostics
        .iter()
        .filter(|d| d.code == "WS049")
        .count()
}

#[test]
fn an_enum_anywhere_inside_a_port_is_ws049() {
    let head = "enum E { A, B(int) }\nenum U { X, Y }\ntype R = { e: E, n: int }\ntype Q = { u: U, n: int }\n";
    for decl in [
        "in r: R",
        "in q: Q",
        "var r: R = { e: E.A, n: 1 }\nout o: R = r",
        "in t: (E, int)",
        "in t: (U, int)",
        "in es: E[]",
        "chip C(g: exec, us: U[]) { }",
        "chip C(g: exec, r: R) { }",
        "chip C(g: exec) -> (o: R) { }",
        "chip C(g: exec, u: U) { }",
        "chip C(g: exec, e: EasingFunction) { }",
        "chip C(g: exec) -> (o: U) { }",
    ] {
        assert_eq!(
            ws049_count(&format!("{head}{decl}\n")),
            1,
            "{decl}: {:?}",
            codes(&format!("{head}{decl}\n"))
        );
    }
    // One wire each: a payload-free enum or a game enum at a top-level port.
    for decl in [
        "in us: U[]",
        "in m: Map<int, U>",
        "in u: U",
        "var v: U = U.X\nout o: U = v",
        "in ease: EasingFunction",
        "var v: EasingFunction = EasingFunction.Sine\nout o = v",
    ] {
        assert_eq!(ws049_count(&format!("{head}{decl}\n")), 0, "{decl}");
    }
    // Reported once for a chip output set in its body.
    let src = format!("{head}var v: E = E.A\nchip C(g: exec) -> (o: E) {{ out o = v }}\n");
    assert_eq!(ws049_count(&src), 1, "{:?}", codes(&src));
}

#[test]
fn a_destructured_tuple_of_vars_keeps_its_places() {
    let head =
        "var x: int = 0\nvar y: int = 0\nvar h: *int\nmod M7(v: *int) { v = v + 1 }\nin go: exec\n";
    assert_clean(&format!(
        "{head}on go {{ let (p, q) = (x, y)\n M7(q)\n h = &p }}"
    ));
    assert_code(
        &format!("{head}on go {{ let (p, q) = (x + 1, y)\n M7(p) }}"),
        "WS008",
    );
    assert_code(
        &format!(
            "{head}mod pair() -> (a: int, b: int) {{ return x, y }}\non go {{ let (p, q) = pair()\n M7(q) }}"
        ),
        "WS008",
    );
}

#[test]
fn a_value_arm_of_a_statement_match_is_an_error() {
    let head = "enum E { A, B }\ntype P = { x: int, y: int }\nvar e: E = E.A\nvar x: int = 0\nvar y: int = 0\n\
                var s: P = { x: 0, y: 0 }\nin go: exec\n";
    for arm in ["{ x, y }", "{ x: 1 }", "{ ...s }"] {
        assert_code(
            &format!("{head}on go {{ match e {{ A => {arm}, B => {{ x = 1 }} }} }}"),
            "WS092",
        );
    }
    assert_code(
        &format!("{head}on go {{ match e {{ A => {{ x, y }}, B => {{ x, y }} }} }}"),
        "WS092",
    );
    assert_clean(&format!(
        "{head}on go {{ match e {{ A => {{ x = 2 }}, B => {{ x = 1 }} }} }}"
    ));
}

#[test]
fn a_name_starting_with_two_underscores_is_reserved() {
    for src in [
        "var __x: int = 0",
        "let __y = 1",
        "mod __m() { }",
        "mod m(__p: int) { }",
        "type __T = { a: int }",
        "type T = { __a: int }",
        "var r = { __a: 1 }",
        "enum E { __A, B }",
        "enum __E { A }",
        "type T = { a: int }\nvar t: T = { a: 1 }\nout o: int = t.__disc",
        "in go: exec\non go { let { a: __b } = { a: 1 } }",
        "in __go: exec",
    ] {
        assert_code(src, "WS093");
    }
    assert_clean("var a__b: int = 0\nvar _c: int = 0");
}

#[test]
fn a_ref_param_given_other_storage_reads_as_one_sentence() {
    let r = tc("var s: string = \"\"\nmod set(p: *int) { p = 1 }\nin go: exec\non go { set(s) }");
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS003")
        .expect("WS003");
    assert!(
        d.message.contains("so it needs a `int` variable;") && !d.message.contains("  "),
        "{}",
        d.message
    );
}

#[test]
fn a_nested_container_of_several_wire_elements_names_their_kind() {
    for (decl, kind) in [
        ("type P = { x: int, y: int }\nvar c: P[][]", "records"),
        ("var c: (int, float)[][]", "tuples"),
        ("enum S { A, B(int) }\nvar c: S[][]", "enums"),
    ] {
        let r = tc(&format!("{decl}\nin go: exec\non go {{ }}\n"));
        let d = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS025")
            .expect("WS025");
        assert!(
            d.message.contains(&format!("its elements are {kind},")),
            "{decl}: {}",
            d.message
        );
    }
}
