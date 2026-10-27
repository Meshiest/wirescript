//! A type written through an alias is the type the alias names at every
//! place lowering and typecheck read one: custom-event receiver parameters and
//! `await CustomEvent` captures, chip pins, records of refs, a ref of a record.
//! A program's own declaration named like a builtin generic type is what that
//! name means in every stage. A library names the enums it declares or imports,
//! as it names aliases. Rows compare the save against the twin written without
//! the alias.

use super::declaring_scope::{compiled, save_dump};

/// Compare `a` against `b`, both folds: equal saves, or the same error codes
/// when neither compiles.
fn same_outcome(row: &str, a: &str, b: &str, deps: &[(&str, &str)], failures: &mut Vec<String>) {
    same_outcome_at(row, a, b, deps, &[true, false], failures);
}

/// [`same_outcome`] at the given folds only.
fn same_outcome_at(
    row: &str,
    a: &str,
    b: &str,
    deps: &[(&str, &str)],
    folds: &[bool],
    failures: &mut Vec<String>,
) {
    for &fold in folds {
        match (save_dump(a, deps, fold), save_dump(b, deps, fold)) {
            (Ok(x), Ok(y)) if x == y => {}
            (Err(mut x), Err(mut y)) => {
                x.sort();
                x.dedup();
                y.sort();
                y.dedup();
                if x != y {
                    failures.push(format!("{row} fold {fold}: {x:?} / twin {y:?}\n{a}"));
                }
            }
            (Ok(x), Ok(y)) => {
                let only_a: Vec<&String> = x.iter().filter(|l| !y.contains(l)).take(3).collect();
                let only_b: Vec<&String> = y.iter().filter(|l| !x.contains(l)).take(3).collect();
                failures.push(format!("{row} fold {fold}: saves differ\n{a}\n--- twin\n{b}\nonly a: {only_a:#?}\nonly b: {only_b:#?}"));
            }
            (x, y) => failures.push(format!(
                "{row} fold {fold}: {:?} / twin {:?}\n{a}\n--- twin\n{b}",
                x.err(),
                y.err()
            )),
        }
    }
}

#[track_caller]
fn report(failures: &[String], rows: usize) {
    crate::test_support::report_failures(failures, rows);
}

fn codes(src: &str, deps: &[(&str, &str)]) -> Vec<String> {
    match compiled(src, deps, true) {
        Ok(c) => c.out.diagnostics.iter().map(|d| d.code.clone()).collect(),
        Err(codes) => codes,
    }
}

/// The codes resolving and type-checking `src` report, with no lowering.
fn typecheck_codes(src: &str) -> Vec<String> {
    let loader = crate::resolve::MemLoader {
        files: crate::collections::HashMap::default(),
    };
    let resolved = crate::resolve::resolve(src, "main", &loader);
    let (tc, _) = crate::typecheck::typecheck_with_inference(&resolved.ast);
    resolved
        .diagnostics
        .iter()
        .chain(&tc.diagnostics)
        .map(|d| d.code.clone())
        .collect()
}

const EVENT_HEAD: &str = "in go: exec\nin go2: exec\nin iv: int\nin fv: float\nin bv: bool\nin sv: string\n\
                          in vv: vector\nin ev: entity\nvar oi: int = 0\nvar of: float = 0.0\nvar ob: bool = false\n\
                          var os: string = \"\"\nvar ovv: vector = Vec(0.0, 0.0, 0.0)\nvar oe: entity\n\
                          enum Dir { N, E }\ntype P = { a: int, b: int }\n";

/// `(written type, a value of it the sender sends, the var a received value
/// is stored in)`.
const EVENT_TYPES: &[(&str, &str, &str)] = &[
    ("int", "iv", "oi"),
    ("float", "fv", "of"),
    ("bool", "bv", "ob"),
    ("string", "sv", "os"),
    ("vector", "vv", "ovv"),
    ("entity", "ev", "oe"),
    ("Dir", "Dir.E", "oi"),
    ("P", "iv", "oi"),
    ("int[]", "iv", "oi"),
];

/// How a row spells the receiver's type through an alias: its declarations
/// and the spelling (`@` is the written type). The namespaced row's alias is
/// declared in the library `u`.
const ALIAS_FORMS: &[(&str, &str, &str)] = &[
    ("alias", "type A = @\n", "A"),
    ("alias of an alias", "type A0 = @\ntype A = A0\n", "A"),
    ("generic alias", "type Id<X> = X\n", "Id<@>"),
    ("namespaced alias", "import * as u from \"u\"\n", "u.A"),
];

/// A receiver's data parameter or an `await CustomEvent` capture: `@T` is the
/// type as the row spells it, `@V` the value sent, `@O` the var it lands in.
const EVENT_PLACES: &[(&str, &str)] = &[
    (
        "receiver parameter",
        "on go { SendCustomEvent(\"e\", @V) }\non CustomEvent(\"e\") -> (q: @T) { @O = q }\n",
    ),
    (
        "await capture",
        "on go2 { SendCustomEvent(\"e\", @V) }\non go { let q: @T = await CustomEvent(\"e\")\n @O = q }\n",
    ),
];

/// The default set compares each row at one fold, alternating so every place,
/// type and form meets both: the alias is resolved before folding and the
/// row reads only equality with its twin. Both folds run under `--ignored`.
#[test]
fn an_event_parameter_declared_through_an_alias_is_the_type_it_names() {
    event_parameter_through_an_alias_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn an_event_parameter_declared_through_an_alias_is_the_type_it_names_exhaustive() {
    event_parameter_through_an_alias_matrix(true);
}

fn event_parameter_through_an_alias_matrix(full: bool) {
    let mut failures = Vec::new();
    let mut rows = 0;
    for (pi, (place, body)) in EVENT_PLACES.iter().enumerate() {
        for (ti, (written, value, out)) in EVENT_TYPES.iter().enumerate() {
            for (fi, (form, decls, spelled)) in ALIAS_FORMS.iter().enumerate() {
                let lib = format!("type A = {written}\n");
                let namespaced = form.starts_with("namespaced");
                // A library names only its own types, so it aliases the builtins.
                if namespaced && matches!(*written, "Dir" | "P") {
                    continue;
                }
                let deps: &[(&str, &str)] = if namespaced { &[("u", &lib)] } else { &[] };
                let fill = |t: &str| {
                    body.replace("@T", t)
                        .replace("@V", value)
                        .replace("@O", out)
                };
                let decls = decls.replace('@', written);
                // An import comes first; an alias after the types it names.
                let (top, after) = if namespaced {
                    (decls.as_str(), "")
                } else {
                    ("", decls.as_str())
                };
                let aliased = format!(
                    "{top}{EVENT_HEAD}{after}{}",
                    fill(&spelled.replace('@', written))
                );
                let twin = format!("{top}{EVENT_HEAD}{after}{}", fill(written));
                let folds: &[bool] = match (full, (pi + ti + fi) % 2) {
                    (true, _) => &[true, false],
                    (false, 0) => &[true],
                    (false, _) => &[false],
                };
                same_outcome_at(
                    &format!("{place} {form} {written}"),
                    &aliased,
                    &twin,
                    deps,
                    folds,
                    &mut failures,
                );
                rows += 1;
            }
        }
    }
    report(&failures, rows);
}

/// An alias of a ref received from an event is the ref the sender sends.
#[test]
fn an_event_parameter_declared_as_an_alias_of_a_ref_is_that_ref() {
    let program = |t: &str| {
        format!(
            "type RI = *int\nin go: exec\nvar n: int = 0\nvar a: int = 3\nvar h: {t}\non go {{ h = &a\n SendCustomEvent(\"e\", h) }}\n\
             on CustomEvent(\"e\") -> (q: {t}) {{ *q = 7\n n = a }}\n"
        )
    };
    let mut failures = Vec::new();
    same_outcome(
        "alias of a ref",
        &program("RI"),
        &program("*int"),
        &[],
        &mut failures,
    );
    report(&failures, 1);
}

/// A sender and a receiver that disagree are `WS030` whether the receiver
/// writes its type or an alias of it; a constant the receiver's type holds is
/// held in it, through an alias as when written.
#[test]
fn an_event_type_mismatch_is_reported_through_an_alias() {
    for (decl, t) in [
        ("", "int"),
        ("type I = int\n", "I"),
        ("type I0 = int\ntype I = I0\n", "I"),
        ("type Id<X> = X\n", "Id<int>"),
    ] {
        let program = |sent: &str| {
            format!(
                "{decl}in go: exec\nin fv: float\nvar n: int = 0\non go {{ SendCustomEvent(\"e\", {sent}) }}\n\
                 on CustomEvent(\"e\") -> (q: {t}) {{ n = q }}\n"
            )
        };
        let mismatch = codes(&program("fv"), &[]);
        assert!(mismatch.contains(&"WS030".to_string()), "{t}: {mismatch:?}");
        let constant = codes(&program("2.5"), &[]);
        assert!(
            constant.contains(&"WS030".to_string()) && !constant.contains(&"WS106".to_string()),
            "{t}: {constant:?}"
        );
        let matched = codes(&program("2"), &[]);
        assert!(!matched.contains(&"WS030".to_string()), "{t}: {matched:?}");
    }
}

/// A constant is sent as the type it has, so a number constant sent to a
/// receiver that declares the other number type is `WS030` like any other
/// value, in both directions and in every constant form (a literal, a name, a
/// constant `if`, `match` or `Select`, a block value, arithmetic of
/// constants), and is sent unconverted. Every receiver of the channel is
/// checked, typed `await` captures included.
///
/// `WS030` is a typecheck code, so the default set reads it from typecheck
/// alone; the full compile of every row runs under `--ignored`.
#[test]
fn a_number_constant_sent_to_the_other_number_type_is_ws030() {
    number_constant_sent_to_the_other_number_type_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_number_constant_sent_to_the_other_number_type_is_ws030_exhaustive() {
    number_constant_sent_to_the_other_number_type_matrix(true);
}

fn number_constant_sent_to_the_other_number_type_matrix(full: bool) {
    let head = "enum M { A, B }\nconst m = M.A\nconst KF = 2.5\nconst KI = 2\nin go: exec\nin go2: exec\n\
                in fv: float\nin iv: int\nvar n: int = 0\nvar x: float = 0.0\n";
    let floats = [
        "2.5",
        "KF",
        "if true then 2.5 else 1.5",
        "match m { M.A => 2.5, M.B => 1.5 }",
        "Select(true, 2.5, 1.5)",
        "KF * 1.0",
        "-KF",
        "{ 2.5 }",
        "fv",
    ];
    let ints = [
        "13",
        "KI",
        "if true then 2 else 3",
        "match m { M.A => 2, M.B => 3 }",
        "KI + 0",
        "-KI",
        "{ 2 }",
        "iv",
    ];
    let mut rows = 0;
    let mut failures = Vec::new();
    let mut check = |src: String, want: usize| {
        let codes = if full {
            codes(&src, &[])
        } else {
            typecheck_codes(&src)
        };
        let got = codes.iter().filter(|c| *c == "WS030").count();
        if got != want {
            failures.push(format!("{got} WS030, want {want}\n{src}"));
        }
        rows += 1;
    };
    let handler = |sent: &str, recv: &str| {
        format!(
            "{head}on go {{ SendCustomEvent(\"e\", {sent}) }}\non CustomEvent(\"e\") -> {recv}\n"
        )
    };
    for sent in floats {
        check(handler(sent, "(q: int) { n = q }"), 1);
        check(handler(sent, "(q: float) { x = q }"), 0);
        check(
            handler(
                sent,
                "(q: int) { n = q }\non CustomEvent(\"e\") -> (q: float) { x = q }",
            ),
            1,
        );
        check(
            handler(
                sent,
                "(q: float) { x = q }\non CustomEvent(\"e\") -> (q: int) { n = q }",
            ),
            1,
        );
        let capture = |t: &str| {
            format!(
                "{head}on go {{ SendCustomEvent(\"e\", {sent}) }}\non go2 {{ let v: {t} = await CustomEvent(\"e\")\n x = v }}\n"
            )
        };
        check(capture("int"), 1);
        check(capture("float"), 0);
        let global = format!(
            "{head}on go {{ SendGlobalCustomEvent(\"g\", {sent}) }}\non GlobalCustomEvent(\"g\") -> (q: int) {{ n = q }}\n"
        );
        check(global, 1);
    }
    for sent in ints {
        check(handler(sent, "(q: float) { x = q }"), 1);
        check(handler(sent, "(q: int) { n = q }"), 0);
        check(
            handler(
                sent,
                "(q: float) { x = q }\non CustomEvent(\"e\") -> (q: int) { n = q }",
            ),
            1,
        );
        let capture = format!(
            "{head}on go {{ SendCustomEvent(\"e\", {sent}) }}\non go2 {{ let v: float = await CustomEvent(\"e\")\n x = v }}\n"
        );
        check(capture, 1);
    }
    // The constant is sent as written: the save holds the float.
    let sent = super::declaring_scope::save_values(
        &handler("2.5", "(q: int) { n = q }"),
        &[],
        true,
        false,
    )
    .unwrap();
    assert!(
        sent.iter()
            .any(|l| matches!(l, crate::ir::Literal::Float(f) if *f == 2.5)),
        "{sent:?}"
    );
    assert!(rows > 0);
    assert!(
        failures.is_empty(),
        "{} of {rows} rows failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// A chip parameter that is a record of refs written through an alias, or an
/// alias of a ref of a record, binds as the written form does.
#[test]
fn a_chip_parameter_through_an_alias_of_a_ref_binds_as_written() {
    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "record of alias refs",
            "type RI = *int\ntype PP = { a: RI, b: RI }\n",
            "type PP = { a: *int, b: *int }\n",
            "chip S(p: PP) { on go { p.a = p.a + p.b } }\non go { S({ a: x1, b: x2 })\n n = x1 }",
        ),
        (
            "record of alias refs read",
            "type RI = *int\ntype PP = { a: RI, b: RI }\n",
            "type PP = { a: *int, b: *int }\n",
            "chip S(p: PP) { on go { x = p.a / p.b\n s = \"v={p.a}\"\n if p.a == 5 { n = 1 } } }\non go { S({ a: x1, b: x2 }) }",
        ),
        (
            "record with one alias ref",
            "type RI = *int\ntype PP = { a: RI, b: int }\n",
            "type PP = { a: *int, b: int }\n",
            "chip S(p: PP) { on go { p.a = p.a + p.b } }\nmod M(p: PP) { p.a = p.a * p.b }\non go { S({ a: x1, b: 2 })\n M({ a: x1, b: 3 })\n n = x1 }",
        ),
        (
            "alias of a ref of a record literal type",
            "type R = { a: int, b: int }\ntype PR = *{ a: int, b: int }\n",
            "type R = { a: int, b: int }\n",
            "chip f(p: PR) { on go { p.a = p.a + 10\n p.b += 1 } }\non go { f(r)\n n = r.a + r.b }",
        ),
        (
            "alias of a ref of a record alias",
            "type R = { a: int, b: int }\ntype PR = *R\n",
            "type R = { a: int, b: int }\n",
            "chip f(p: PR) { on go { p.a = p.a + 10\n p.b += 1 } }\nmod g(p: PR) { p.a = p.a * 2 }\non go { f(r)\n g(r)\n n = r.a + r.b }",
        ),
    ];
    let head = "in go: exec\nvar n: int = 0\nvar x: float = 0.0\nvar s: string = \"\"\nvar x1: int = 5\nvar x2: int = 2\n\
                var r: R = { a: 1, b: 2 }\n";
    let mut failures = Vec::new();
    for (row, aliased, written, body) in rows {
        let twin_body = body.replace(
            "p: PR",
            if row.contains("literal") {
                "p: *{ a: int, b: int }"
            } else {
                "p: *R"
            },
        );
        let aliased = format!("{aliased}{}{body}\n", head_for(head, aliased));
        let twin = format!("{written}{}{twin_body}\n", head_for(head, written));
        same_outcome(row, &aliased, &twin, &[], &mut failures);
    }
    report(&failures, rows.len());
}

/// `head`, without the record var when `decls` declares no `R` to give it.
fn head_for(head: &str, decls: &str) -> String {
    if decls.contains("type R ") {
        head.to_string()
    } else {
        head.replace("var r: R = { a: 1, b: 2 }\n", "")
    }
}

/// A program's own type named like a builtin generic (`Map`, or `Array`/`Ref`
/// with an arity their builtin spelling does not take) is that type in
/// typecheck, constant evaluation and lowering alike.
#[test]
fn a_program_type_named_like_a_builtin_generic_is_the_programs() {
    let rows: &[(&str, &str, &str)] = &[
        (
            "generic alias Map",
            "type @<K, V> = { k: K, v: V }\n",
            "var m: @<int, int> = { k: 1, v: 2 }\non go { n = m.v }",
        ),
        (
            "generic alias Map, const",
            "type @<K, V> = { k: K, v: V }\n",
            "const M: @<int, float> = { k: 1, v: 2.5 }\non go { x = M.v\n n = M.k }",
        ),
        (
            "generic alias Map, mod parameter",
            "type @<K, V> = { k: K, v: V }\n",
            "mod f(p: @<int, int>) -> int { return p.k + p.v }\non go { n = f({ k: 1, v: 2 }) }",
        ),
        (
            "generic alias Map, chip parameter",
            "type @<K, V> = { k: K, v: V }\n",
            "chip C(p: @<int, int>) -> (r: int) { return p.k * p.v }\nout o = C({ k: 3, v: 2 })",
        ),
        (
            "one-parameter Map",
            "type @<V> = { v: V }\n",
            "var m: @<int> = { v: 4 }\non go { n = m.v }",
        ),
        (
            "generic enum Map",
            "enum @<T> { Some(T), No }\n",
            "var m: @<int> = @.Some(3)\non go { match m { Some(v) => { n = v } No => { } } }",
        ),
        (
            "plain alias Map",
            "type @ = { k: int, v: int }\n",
            "var m: @ = { k: 1, v: 2 }\non go { n = m.v }",
        ),
        (
            "two-parameter Array",
            "type @<K, V> = { k: K, v: V }\n",
            "var m: @<int, int> = { k: 1, v: 2 }\non go { n = m.v }",
        ),
    ];
    let head = "in go: exec\nvar n: int = 0\nvar x: float = 0.0\n";
    let mut failures = Vec::new();
    for (row, decl, body) in rows {
        let name = if row.contains("Array") {
            "Array"
        } else {
            "Map"
        };
        let named = format!(
            "{}{head}{}\n",
            decl.replace('@', name),
            body.replace('@', name)
        );
        let twin = format!(
            "{}{head}{}\n",
            decl.replace('@', "Pair"),
            body.replace('@', "Pair")
        );
        same_outcome(row, &named, &twin, &[], &mut failures);
        if row == &"generic alias Map" {
            let ref_named = named.replace("Map", "Ref");
            same_outcome("two-parameter Ref", &ref_named, &twin, &[], &mut failures);
        }
    }
    // A library's own `Map<K, V>`, in its body and in its importer's call.
    let lib = "type Map<K, V> = { k: K, v: V }\nmod sum(p: Map<int, int>) -> int { let q: Map<int, int> = p\n return q.k + q.v }\n";
    let twin_lib = lib.replace("Map", "Pair");
    for import in ["import { sum } from \"lib\"\n", "import \"lib\"\n"] {
        let src = format!("{import}{head}on go {{ n = sum({{ k: 1, v: 2 }}) }}\n");
        for fold in [true, false] {
            let (a, b) = (
                save_dump(&src, &[("lib", lib)], fold),
                save_dump(&src, &[("lib", &twin_lib)], fold),
            );
            if a.is_err() || a != b {
                failures.push(format!(
                    "library Map {import} fold {fold}: {:?} / {:?}",
                    a.err(),
                    b.err()
                ));
            }
        }
    }
    report(&failures, rows.len() + 3);
}

/// A type or enum declared under a builtin type's name is WS107: `Array<T>`
/// and `Ref<T>` spell `T[]` and `*T`, so a one-parameter `Array` or `Ref` would
/// never be read, and one named like `float` or `any` would shadow the
/// builtin. Another arity is the program's own type (see above).
#[test]
fn a_type_named_like_a_builtin_type_is_ws107() {
    let refs = |decl: &str| {
        format!(
            "{decl}\nin go: exec\nvar n: int = 0\nvar a: int = 1\nmod f(p: Ref<int>) {{ p = 5 }}\non go {{ f(a)\n n = a }}\n"
        )
    };
    let builtin = |decl: &str| format!("{decl}\nvar x: float = 1.5\nout ox: float = x\n");
    let rows: Vec<(String, bool)> = vec![
        (refs("type Ref<V> = V[]"), true),
        (refs("type Array<T> = { v: T }"), true),
        (refs("enum Ref<T> { A(T), B }"), true),
        (refs("enum Array<T> { A(T), B }"), true),
        (builtin("type float = int"), true),
        (builtin("type int = float"), true),
        (builtin("type string = int"), true),
        (builtin("enum vector { A, B }"), true),
        (builtin("type any = int"), true),
        ("type Ref<K, V> = { k: K, v: V }\nin go: exec\nvar n: int = 0\nvar m: Ref<int, int> = { k: 1, v: 2 }\non go { n = m.v }\n".into(), false),
    ];
    let mut failures = Vec::new();
    for (src, want) in &rows {
        let got = match compiled(src, &[], true) {
            Err(codes) => codes.contains(&"WS107".to_string()),
            Ok(c) => {
                if c.out.diagnostics.iter().any(|d| d.code == "WS107") {
                    failures.push(format!("WS107 is not an error:\n{src}"));
                }
                false
            }
        };
        if got != *want {
            failures.push(format!(
                "WS107 error {got}, want {want}: {:?}\n{src}",
                codes(src, &[])
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A library reads its own alias whatever its importer declares under the
/// name, enums included; and a library naming an enum only its importer
/// declares is `WS002`, as one naming such an alias is.
#[test]
fn a_library_names_the_enums_it_declares_or_imports() {
    let own = "type F = float\nmod mo() -> float { static var t: F = 5\n return t / 2 }\n";
    let written = "mod mo() -> float { static var t: float = 5\n return t / 2 }\n";
    let via = "import \"c\"\ntype G = F\nmod mb() -> float { static var t: G = 5\n var u: F = 5\n return t / 2 + u / 2 }\n";
    let via_written =
        "mod mb() -> float { static var t: float = 5\n var u: float = 5\n return t / 2 + u / 2 }\n";
    let c = "type F = float\n";
    let mut failures = Vec::new();
    let mut rows = 0;
    // A whole import brings the library's `F` beside the importer's: `WS013`.
    let whole =
        "import \"own\"\nenum F { A, B }\nin go: exec\nvar x: float = 0.0\non go { x = mo() }\n";
    assert!(codes(whole, &[("own", own)]).contains(&"WS013".to_string()));
    for (import, call) in [
        ("import { mo } from \"own\"\n", "mo()"),
        ("import * as o from \"own\"\n", "o.mo()"),
    ] {
        let src = format!(
            "{import}enum F {{ A, B }}\nin go: exec\nvar x: float = 0.0\non go {{ x = {call} }}\n"
        );
        let renamed = src.replace("enum F", "enum Fx");
        for fold in [true, false] {
            let (a, b) = (
                save_dump(&src, &[("own", own)], fold),
                save_dump(&renamed, &[("own", written)], fold),
            );
            if a.is_err() || a != b {
                failures.push(format!(
                    "own alias over importer enum, {import} fold {fold}: {:?} / {:?}",
                    a.err(),
                    b.err()
                ));
            }
        }
        rows += 1;
    }
    let src = "import { mb } from \"b\"\nenum F { A, B }\nin go: exec\nvar x: float = 0.0\non go { x = mb() }\n";
    for fold in [true, false] {
        let a = save_dump(src, &[("b", via), ("c", c)], fold);
        let b = save_dump(
            &src.replace("enum F", "enum Fx"),
            &[("b", via_written)],
            fold,
        );
        if a.is_err() || a != b {
            failures.push(format!(
                "imported alias over importer enum fold {fold}: {:?} / {:?}",
                a.err(),
                b.err()
            ));
        }
    }
    rows += 1;
    // An enum only the importer declares, named in a type or a construction.
    for lib in [
        "mod me() -> int { var e: Dir = Dir.B\n return EnumToInt(e) }\n",
        "mod me() -> int { return EnumToInt(Dir.B) }\n",
    ] {
        for import in [
            "import { me } from \"le\"\n",
            "import * as l from \"le\"\n",
            "import \"le\"\n",
        ] {
            let call = if import.contains("* as") {
                "l.me()"
            } else {
                "me()"
            };
            let src = format!(
                "{import}enum Dir {{ A, B }}\nin go: exec\nvar x: int = 0\non go {{ x = {call} }}\n"
            );
            let got = codes(&src, &[("le", lib)]);
            if !got.contains(&"WS002".to_string()) {
                failures.push(format!("importer-only enum, {import}{lib}: {got:?}"));
            }
            rows += 1;
        }
    }
    // The library's own enum, and one its import brings, still read.
    let lib = "enum Dir { A, B }\nmod me() -> int { var e: Dir = Dir.B\n return EnumToInt(e) }\n";
    let src = "import { me } from \"le\"\nin go: exec\nvar x: int = 0\non go { x = me() }\n";
    assert!(save_dump(src, &[("le", lib)], true).is_ok());
    let lib = "import \"d\"\nmod me() -> int { var e: Dir = Dir.B\n return EnumToInt(e) }\n";
    assert!(save_dump(src, &[("le", lib), ("d", "enum Dir { A, B }\n")], true).is_ok());
    report(&failures, rows);
}

/// Every type a lowering or typecheck path reads from a written annotation is
/// read with the aliases of the file that writes it: no converter that reads
/// a `TypeExpr` with no aliases in scope exists to be called. The canonical
/// resolver's context is built only by the resolver itself and typecheck's
/// alias-aware entry points.
#[test]
fn no_type_is_read_without_its_aliases() {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read the source tree") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((
                    rel,
                    std::fs::read_to_string(&path).expect("read a source file"),
                ));
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 100, "{}", files.len());
    let builds_context = [
        "src/types/resolve.rs",
        "src/types/resolve/tests.rs",
        "src/typecheck/resolve.rs",
    ];
    let this_file = "src/lower/tests/alias_conversion.rs";
    for (rel, text) in &files {
        if rel == this_file {
            continue;
        }
        let constructs = text.contains(&["ResolveCtx", " {"].concat());
        assert!(
            !constructs || builds_context.contains(&rel.as_str()),
            "{rel} builds a resolver context of its own"
        );
        for gone in [
            "fn type_of_type_expr",
            "fn type_expr_to_type",
            "fn ce_param_type",
        ] {
            assert!(!text.contains(gone), "{rel} declares `{gone}`");
        }
        if rel.starts_with("src/lower/") {
            assert!(
                !text.contains(&["types::resolve::", "resolve_type("].concat()),
                "{rel} calls the canonical resolver directly"
            );
        }
        // An empty declaration table resolves no alias: only a test builds one.
        let test_file = rel.contains("/tests/") || rel.ends_with("tests.rs");
        assert!(
            test_file || !text.contains(&["DeclaredTypes::", "default()"].concat()),
            "{rel} reads types with no declarations"
        );
    }
}

/// `typecheck::enums::EnumRegistry` hands out enums only through its methods,
/// each called only where it may be however the call is spelled: `of` (a resolved
/// type, never one built in place), `expression_path`, `citing_arms`,
/// `editor_lookup` (editor build only), and `written_name`/`type_named` (the
/// alias-aware resolvers alone); `declaring_variants` is the registry's own. No
/// other file keeps a by-name table of enums, which would bypass the registry.
#[test]
fn the_enum_registry_is_read_by_a_written_name_only_in_the_resolvers() {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read the source tree") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = std::fs::read_to_string(&path).expect("read a source file");
                out.push((rel, text.split_whitespace().collect::<String>()));
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 100, "{}", files.len());
    let this_file = "src/lower/tests/alias_conversion.rs";
    let mut found = Vec::new();
    let (mut seen_written, mut seen_paths) = (0, 0);
    for (rel, text) in &files {
        if rel == this_file {
            continue;
        }
        seen_written += registry_calls(text, "written_name") + registry_calls(text, "type_named");
        seen_paths += registry_calls(text, "expression_path");
        found.extend(registry_bypasses(rel, text));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
    let (_, enums) = files
        .iter()
        .find(|(rel, _)| rel == REGISTRY)
        .expect("the registry");
    for handed_out in [
        "fnvalues(&self",
        "Iterator<Item=&EnumDef>",
        "Iterator<Item=&'_EnumDef>",
    ] {
        assert!(
            !enums.contains(handed_out),
            "the registry hands out its enums: {handed_out}"
        );
    }
    assert!(
        seen_written > 0 && seen_paths > 0,
        "the resolvers read written names, the paths read paths"
    );
}

/// The guard catches each way around the registry, and passes what the
/// registry allows: every pattern below is written the way a file would
/// write it, whitespace and all.
#[test]
fn the_enum_registry_guard_catches_each_bypass() {
    let caught = |src: &str| {
        !registry_bypasses(
            "src/lower/stmt.rs",
            &src.split_whitespace().collect::<String>(),
        )
        .is_empty()
    };
    let bypasses = [
        "let d = EnumRegistry::editor_lookup(&reg, written);",
        "let d = crate::typecheck::enums::EnumRegistry::expression_path(&ctx.enum_defs, n);",
        "let d = ctx.enum_defs.expression_path(n);",
        "let f = EnumRegistry::written_name; f(&reg, n)",
        "let d = reg.declaring_variants(&[name]);",
        "let d = reg.citing_arms(arms);",
        "let d = reg.of(&Type::Enum { name: written.clone(), args: vec![] });",
        "let t = Type::Enum { name: written, args: Vec::new() };\nlet d = reg.of(&t);",
        "let mut t: Type = crate::ir::Type::Enum { name: n.to_string(), args };\nif let Some(d) = EnumRegistry::of(&reg, &t) {}",
        "let table: std::collections::BTreeMap<String, EnumDef> = Default::default();",
        "struct S { by_name: HashMap<Arc<str>, crate::typecheck::enums::EnumDef> }",
        "let m: FxHashMap<&str, &EnumDef> = FxHashMap::default();",
        "let v: Vec<(String, EnumDef)> = Vec::new();",
    ];
    for b in bypasses {
        assert!(caught(b), "not caught: {b}");
    }
    let allowed = [
        "let d = ctx.enum_defs.of(&ctx.type_of(e));",
        "let t = Type::Enum { name: def.name.clone(), args };\nlet decision = matchtree::build(enums, &t, &pats);",
        "let d = ctx.enum_defs.of(&field_type).cloned();",
        "let m: HashMap<String, Type> = HashMap::default();",
        "let names = TopNames::of(&decls);",
    ];
    for a in allowed {
        assert!(!caught(a), "caught: {a}");
    }
}

/// The registry, which may read its table any way it likes.
const REGISTRY: &str = "src/typecheck/enums.rs";

/// How many times `text` (whitespace stripped) calls or names the registry
/// method `name`: `.name(`, `::name(`, or `::name` as a value.
fn registry_calls(text: &str, name: &str) -> usize {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(name)
        .filter(|(i, _)| text[..*i].ends_with('.') || text[..*i].ends_with("::"))
        .filter(|(i, _)| !text[i + name.len()..].starts_with(ident))
        .count()
}

/// Each way `text`, the whitespace-stripped source of the file `rel`, reads
/// an enum around the registry.
fn registry_bypasses(rel: &str, text: &str) -> Vec<String> {
    let resolvers = ["src/typecheck/resolve.rs", "src/const_eval.rs", REGISTRY];
    let paths = [
        "src/typecheck/infer.rs",
        "src/lower/expr.rs",
        "src/lower/access.rs",
        "src/const_eval/expr.rs",
        "src/analysis/hover.rs",
        REGISTRY,
    ];
    let mut found = Vec::new();
    let mut only_in = |names: &[&str], allowed: &[&str], what: &str| {
        let calls: usize = names.iter().map(|n| registry_calls(text, n)).sum();
        if calls > 0 && !allowed.contains(&rel) {
            found.push(format!("{rel} {what}"));
        }
    };
    only_in(
        &["written_name", "type_named"],
        &resolvers,
        "reads an enum by a written type name",
    );
    only_in(
        &["expression_path"],
        &paths,
        "reads an enum by name outside an expression path",
    );
    only_in(
        &["editor_lookup"],
        &[],
        "reads an enum by the editor's lookup",
    );
    only_in(
        &["declaring_variants"],
        &[REGISTRY],
        "reads an enum by written variant text",
    );
    only_in(
        &["citing_arms"],
        &["src/const_eval/expr.rs", REGISTRY],
        "reads an enum by the variants arms cite",
    );
    if rel == REGISTRY {
        return found;
    }
    // A type built in place from a name, handed to `of` directly or through
    // a binding.
    let built: Vec<&str> = text
        .split(';')
        .filter_map(|stmt| {
            let at = stmt.rfind("let")?;
            let rest = stmt[at + 3..]
                .strip_prefix("mut")
                .unwrap_or(&stmt[at + 3..]);
            let end = rest.find(['=', ':'])?;
            let value = &rest[rest.find('=')? + 1..];
            let head = value.find(['(', '{']).map_or(value, |i| &value[..=i]);
            head.ends_with("Type::Enum{").then_some(&rest[..end])
        })
        .filter(|name| !name.is_empty())
        .collect();
    for (i, _) in text.match_indices("of(") {
        if !(text[..i].ends_with('.') || text[..i].ends_with("::")) {
            continue;
        }
        let args = &text[i + 3..];
        let mut depth = 0usize;
        let close = args
            .char_indices()
            .find(|&(_, c)| {
                match c {
                    '(' | '{' | '[' => depth += 1,
                    ')' | '}' | ']' if depth == 0 => return true,
                    ')' | '}' | ']' => depth -= 1,
                    _ => {}
                }
                false
            })
            .map_or(args.len(), |(j, _)| j);
        let fed = args[..close].split(',').any(|a| {
            let a = a.trim_start_matches('&');
            a.contains("Type::Enum{") || built.contains(&a)
        });
        if fed {
            found.push(format!(
                "{rel} reads an enum through a type built from its name"
            ));
        }
    }
    // A table of enums by a name.
    let named_key = |k: &str| ["String", "str", "Sym"].iter().any(|s| k.contains(s));
    for open in ["Map<", "Vec<("] {
        for (i, _) in text.match_indices(open) {
            let body = &text[i + open.len()..];
            let mut depth = 0usize;
            let end = body
                .char_indices()
                .find(|&(_, c)| {
                    match c {
                        '<' | '(' => depth += 1,
                        '>' | ')' if depth == 0 => return true,
                        '>' | ')' => depth -= 1,
                        _ => {}
                    }
                    false
                })
                .map_or(body.len(), |(j, _)| j);
            let args = &body[..end];
            if let Some((key, value)) = args.split_once(',')
                && named_key(key)
                && value.contains("EnumDef")
            {
                found.push(format!("{rel} keeps enums by name ({open}{args})"));
            }
        }
    }
    found
}

/// A payload field, a record field, an array element or a local record field
/// declared with an alias of an enum is that enum: a nested pattern on it
/// tests the payload (no arm is called unreachable or dropped, a missing arm
/// is `WS054`), and its storage holds the enum's tag. Each row is compared at
/// the save, both folds, with the twin written with the enum's name (`@` is
/// `D` or `Dir`).
#[test]
fn an_alias_of_an_enum_in_a_payload_or_a_field_is_the_enum() {
    let head = "in go: exec\nvar n: int = 0\nenum Dir { N, S }\ntype D = Dir\n";
    let rows = [
        "enum E { A(@), B }\nvar e: E = E.A(Dir.S)\non go { match e {\n E.A(Dir.N) => { n = 1 }\n E.A(Dir.S) => { n = 2 }\n E.B => { n = 3 }\n} }",
        "enum E { A(@), B }\nvar e: E = E.A(Dir.S)\non go { match e {\n E.A(Dir.N) => { n = 1 }\n E.B => { n = 3 }\n} }",
        "enum E { A(@), B }\nvar e: E = E.A(Dir.S)\non go { match e {\n E.A(Dir.N) => { n = 1 }\n _ => { n = 9 }\n} }",
        "enum E { A { d: @ }, B }\nvar e: E = E.A { d: Dir.S }\non go { match e {\n E.A { d: Dir.N } => { n = 1 }\n _ => { n = 9 }\n} }",
        "type P = { d: @ }\nvar p: P = { d: Dir.S }\non go { match p.d {\n Dir.N => { n = 1 }\n _ => { n = 9 }\n} }",
        "type P = { d: @, k: int }\nvar p: P = { d: Dir.N, k: 1 }\non go { p.d = Dir.S\n n = if p.d is Dir.S then 9 else 1 }",
        "var a: @[] = []\non go { a.push(Dir.S)\n n = if a[0] is Dir.S then 9 else 1 }",
        "type P = { d: @, k: int }\non go { var p: P = { d: Dir.S, k: 1 }\n n = if p.d is Dir.S then 9 else 1 }",
        "type P = { d: @, k: int }\nvar p: P = { d: Dir.S, k: 1 }\non go { n = if p.d is Dir.S then 9 else 1 }",
        "var m: Map<string, @> = {}\non go { m.set(\"a\", Dir.S)\n n = if m[\"a\"] is Dir.S then 9 else 1 }",
    ];
    let mut failures = Vec::new();
    for row in rows {
        let aliased = format!("{head}{}", row.replace('@', "D"));
        let written = format!("{head}{}", row.replace('@', "Dir"));
        same_outcome(row, &aliased, &written, &[], &mut failures);
    }
    // An alias of a generic enum instance as a payload.
    let oi = "in go: exec\nvar n: int = 0\ntype OI = Option<int>\nenum E { A(@), B }\nvar e: E = E.A(Some(4))\n\
              on go { match e {\n E.A(Some(q)) => { n = q }\n E.A(None) => { n = 2 }\n E.B => { n = 3 }\n} }";
    same_outcome(
        "generic alias payload",
        &oi.replace('@', "OI"),
        &oi.replace('@', "Option<int>"),
        &[],
        &mut failures,
    );
    let missing = format!("{head}{}", rows[1].replace('@', "D"));
    assert!(
        codes(&missing, &[]).contains(&"WS054".to_string()),
        "{:?}",
        codes(&missing, &[])
    );
    report(&failures, rows.len() + 1);
}

/// An enum of an `import * as` module is the type `ns.Name` and its variants
/// `ns.Name.Variant`; inside the module, and in an alias the module declares,
/// it is `Name`. Each row equals at the save, both folds, the twin importing
/// the enums by name; an enum named like another of the program is `WS013`.
#[test]
fn an_enum_of_a_namespace_import_reads_as_its_named_import() {
    let lib = "enum Dir { N, E, S }\nenum Shape { Circle(float), Rect(float, float), Empty }\ntype DD = Dir\n\
               mod twice(d: Dir) -> int { return EnumToInt(d) * 2 }\n";
    let deps = [("c", lib)];
    let rows = [
        "var d: c.Dir = c.Dir.S\non go { x = EnumToInt(d) }",
        "var d: c.DD = c.Dir.E\non go { x = EnumToInt(d) }",
        "on go { var e: c.Dir = c.Dir.E\n x = EnumToInt(e) }",
        "var e: c.Dir = c.Dir.S\non go { match e { N => { x = 1 } E => { x = 2 } _ => { x = 3 } } }",
        "var s: c.Shape = c.Shape.Rect(2.0, 3.0)\non go { match s { Circle(r) => { f = r } Rect(a, b) => { f = a * b } Empty => { f = 0.0 } } }",
        "on go { var e: c.Dir = c.Dir.FromInt(2)\n x = e.Discriminant }",
        "var arr: c.Dir[] = [c.Dir.S, c.Dir.N]\non go { x = EnumToInt(arr[0]) }",
        "mod m(p: c.Dir) -> int { return EnumToInt(p) }\non go { x = m(c.Dir.S) }",
        "const k = c.Dir.E\non go { x = EnumToInt(k)\n x = c.Dir.S.Discriminant }",
        "on go { x = c.twice(c.Dir.S) }",
    ];
    let head =
        |import: &str| format!("{import}\nin go: exec\nvar x: int = 0\nvar f: float = 0.0\n");
    let mut failures = Vec::new();
    for row in rows {
        let namespaced = format!("{}{row}", head("import * as c from \"c\""));
        let named = format!(
            "{}{}",
            head("import { Dir, Shape, DD } from \"c\"\nimport * as c from \"c\""),
            row.replace("c.Dir", "Dir")
                .replace("c.Shape", "Shape")
                .replace("c.DD", "DD")
        );
        same_outcome(row, &namespaced, &named, &deps, &mut failures);
        assert!(
            save_dump(&namespaced, &deps, true).is_ok(),
            "{row}: {:?}",
            codes(&namespaced, &deps)
        );
    }
    let clash = format!(
        "{}enum Dir {{ Q }}\non go {{ x = 1 }}",
        head("import * as c from \"c\"")
    );
    assert!(
        codes(&clash, &deps).contains(&"WS013".to_string()),
        "{:?}",
        codes(&clash, &deps)
    );
    report(&failures, rows.len());
}

/// A library naming the enum it imports, in a program that declares another
/// enum of that name (`WS013`), is told so, not that it imports no such enum.
#[test]
fn a_library_enum_hidden_by_a_duplicate_is_named_as_imported() {
    let deps = [
        ("c", "enum Dir { N, E, S }\n"),
        (
            "a",
            "import { Dir } from \"c\"\nmod ma() -> int { var e: Dir = Dir.S\n return EnumToInt(e) }\n",
        ),
    ];
    let src = "import { ma } from \"a\"\nenum Dir { Q, R }\nin go: exec\nvar x: int = 0\non go { x = ma() }\n";
    let loader = crate::resolve::MemLoader {
        files: deps
            .iter()
            .map(|(k, v)| (format!("{k}.ws"), v.to_string()))
            .collect(),
    };
    let input = crate::CompileInput {
        source: src,
        file: "main",
        module_name: None,
        fold_mode: crate::FoldMode::ForceOn,
    };
    let Err(crate::CompileError::HasErrors(errors)) =
        crate::compile_with_loader(input, crate::emit::EmitOptions::default(), &loader)
    else {
        panic!("compiles");
    };
    assert!(errors.iter().any(|d| d.code == "WS013"), "{errors:?}");
    assert!(
        !errors
            .iter()
            .any(|d| d.message.contains("neither declares it nor imports it")),
        "{errors:?}"
    );
    // Every `WS002` of the cascade, the type written in `var e: Dir` too.
    let unknown: Vec<_> = errors.iter().filter(|d| d.code == "WS002").collect();
    assert!(
        unknown.len() >= 2
            && unknown
                .iter()
                .all(|d| d.message.contains("declared in c.ws")),
        "{errors:?}"
    );
}
