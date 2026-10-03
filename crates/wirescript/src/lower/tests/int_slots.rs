//! The int-slot rule, the reverse of `float_slots`: a float constant (a
//! literal, a name bound to one, or an expression of constants) that is the
//! whole value of a place declared `int` is the integer it truncates to, held
//! as an integer in the save wherever the value then goes, exactly as the
//! same program written with that integer. A fraction dropped doing so is
//! `WS106`; a value no `int` holds (beyond the 64-bit range, or not finite) is
//! `WS028`. One rule (`const_eval::float_as_int`, through
//! `const_eval::coerce_const_to_type`) serves every route.

use super::declaring_scope::{compile_main, save_dump, save_values};
use crate::ir::Literal;

/// What the constant is written as. Every kind truncates to 13, so the twin
/// of each row writes `13`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    FloatWhole,
    FloatFraction,
    FloatConst,
    FloatConstWhole,
    FloatArithmetic,
    Negated,
    IntLiteral,
    IntConst,
}

impl Kind {
    const ALL: [Kind; 8] = [
        Kind::FloatWhole,
        Kind::FloatFraction,
        Kind::FloatConst,
        Kind::FloatConstWhole,
        Kind::FloatArithmetic,
        Kind::Negated,
        Kind::IntLiteral,
        Kind::IntConst,
    ];

    fn text(self) -> &'static str {
        match self {
            Kind::FloatWhole => "13.0",
            Kind::FloatFraction => "13.5",
            Kind::FloatConst => "F135",
            Kind::FloatConstWhole => "F13",
            Kind::FloatArithmetic => "(F12 + 1.5)",
            Kind::Negated => "-FM135",
            Kind::IntLiteral => "13",
            Kind::IntConst => "THIRTEEN",
        }
    }

    fn drops_a_fraction(self) -> bool {
        matches!(self, Kind::FloatFraction | Kind::FloatConst | Kind::FloatArithmetic | Kind::Negated)
    }

    /// What the twin writes: `13` for a literal, and for a name or an
    /// expression of constants the integer constant `I13`, which lowers the
    /// way a name does.
    fn twin(self) -> &'static str {
        match self {
            Kind::FloatWhole | Kind::FloatFraction | Kind::IntLiteral => "13",
            _ => "I13",
        }
    }
}

/// Constants no `int` holds, each a `WS028` wherever it goes into one.
const OUT_OF_RANGE: [&str; 6] = ["1e30", "-1e30", "9.3e18", "-9.3e18", "9223372036854775808.0", "BIG"];

/// The declarations every row reads; the twin reads them too, so an unused
/// one adds nothing to either save.
const HEAD: &str = "in go: exec\nin c: bool\nvar x: float = 0.0\nvar n: int = 0\nvar m: int = 0\nvar s: string = \"\"\n\
                    const F135 = 13.5\nconst F13 = 13.0\nconst F12 = 12.0\nconst FM135 = -13.5\n\
                    const THIRTEEN = 13\nconst I13 = 13\nconst BIG = 1e30\n";

/// Each place declared `int`: its declarations and a body, `@` the value.
/// Every body reads the value back the ways a value is used (an integer
/// division, an untyped `var`, an interpolation) where it can.
const ROUTES: &[(&str, &str, &str)] = &[
    ("assignment", "", "on go { n = @\nx = n / 4\nvar u = n\nx = u / 4 }"),
    ("var initializer", "var v: int = @\n", "on go { x = v / 4\nvar u = v\nx = u / 4 }"),
    ("handler var", "", "on go { var v: int = @\nx = v / 4\nvar u = v\nx = u / 4 }"),
    ("static var", "mod sm() -> float { static var sv: int = @\nsv = sv + 1\nreturn sv / 4 }\n", "on go { x = sm() }"),
    ("mod parameter", "mod ih(v: int) -> float { var u = v\nm = v\nreturn u / 4 }\n", "on go { x = ih(@) }"),
    ("generic argument", "mod gi<T>(v: T) -> T { return v }\n", "on go { n = gi<int>(@)\nx = n / 4 }"),
    ("chip parameter", "chip C(p: int) -> (r: int) { on go { m = p\nemit r = p * 10 } }\n", "on go { n = C(@) }"),
    ("pure chip parameter", "chip D(v: int) -> (r: float) { return v / 4 }\n", "out od = D(@)"),
    ("chip parameter read untyped", "chip U(p: int) { on go { var u = p\nx = u / 4\ns = \"${p}\" } }\n", "on go { U(@) }"),
    ("annotated let", "", "on go { let l: int = @\nx = l / 4\nvar u = l\nx = u / 4\ns = \"${l}\" }"),
    ("annotated const", "const K: int = @\n", "on go { x = K / 4\nvar u = K\nx = u / 4\ns = \"${K}\" }"),
    ("top-level let", "let L: int = @\n", "on go { x = L / 4\nvar u = L\nx = u / 4 }"),
    ("const chain", "const K: int = @\nconst KK = K / 4\n", "on go { x = KK\nn = K + 1 }"),
    ("const operand", "const K: int = @\nmod f(v: int) -> int { return v * 10 }\n", "on go { m = f(6) + K }"),
    ("return value", "mod give() -> int { return @ }\n", "on go { x = give() / 4\nvar u = give()\nx = u / 4 }"),
    ("emitted output", "out o: int\n", "on go { emit o = @ }"),
    ("out initializer", "out o: int = @\n", "on go { x = o / 4 }"),
    ("chip emitted output", "chip E() -> (r: int) { on go { emit r = @ } }\n", "on go { n = E()\nx = n / 4 }"),
    ("array literal", "var is: int[] = [@, 4]\n", "on go { x = is[0] / 4 }"),
    ("array push", "var is: int[]\n", "on go { is.push(@)\nx = is[0] / 4 }"),
    ("array insert and fill", "var is: int[]\n", "on go { is.insert(0, @)\nis.fill(@)\nx = is[0] / 4 }"),
    ("array element write", "var is: int[]\n", "on go { is.push(1)\nis[0] = @\nx = is[0] / 4 }"),
    ("map literal", "var im: Map<string, int> = { \"a\": @ }\n", "on go { x = im[\"a\"] / 4 }"),
    ("map set", "var im: Map<string, int>\n", "on go { im.set(\"a\", @)\nx = im[\"a\"] / 4 }"),
    ("const array", "const A: int[] = [@, 4]\nin k: int\n", "on go { x = A[k] / 4 }"),
    ("const map", "const M: Map<string, int> = { \"a\": @ }\n", "on go { n = M[\"a\"] + 1\nx = M[\"a\"] / 4 }"),
    ("enum payload", "enum S { Sq(int), No }\nvar sv: S = S.No\n", "on go { sv = S.Sq(@)\nmatch sv { Sq(v) => { x = v / 4 } No => { } } }"),
    ("enum named payload", "enum S { Box { w: int }, No }\nvar sv: S = S.No\n", "on go { sv = S.Box { w: @ }\nmatch sv { Box { w } => { x = w / 4 } No => { } } }"),
    ("generic enum initializer", "enum Opt<T> { Some(T), None }\nvar o: Opt<int> = Opt.Some(@)\n", "on go { match o { Some(v) => { x = v / 4 } None => { } } }"),
    ("generic enum assignment", "enum Opt<T> { Some(T), None }\nvar o: Opt<int> = Opt.None\n", "on go { o = Opt.Some(@)\nmatch o { Some(v) => { x = v / 4 } None => { } } }"),
    ("generic enum const", "enum Opt<T> { Some(T), None }\nconst O: Opt<int> = Opt.Some(@)\n", "on go { match O { Some(v) => { x = v / 4 } None => { } } }"),
    ("record var", "type P = { a: int, b: float }\nvar p: P = { a: @, b: 1.0 }\n", "on go { x = p.a / 4 }"),
    ("record assignment", "type P = { a: int, b: float }\nvar p: P = { a: 1, b: 1.0 }\n", "on go { p = { a: @, b: 1.0 }\nx = p.a / 4 }"),
    ("record field write", "type P = { a: int, b: float }\nvar p: P = { a: 1, b: 1.0 }\n", "on go { p.a = @\nx = p.a / 4 }"),
    ("record let", "type P = { a: int, b: float }\n", "on go { let r: P = { a: @, b: 1.0 }\nx = r.a / 4 }"),
    ("record push", "type P = { a: int, b: float }\nvar ps: P[]\n", "on go { ps.push({ a: @, b: 1.0 })\nx = ps[0].a / 4 }"),
    ("record mod argument", "type P = { a: int, b: float }\nmod pa(p: P) -> float { return p.a / 4 }\n", "on go { x = pa({ a: @, b: 1.0 }) }"),
    ("record chip argument", "type P = { a: int, b: float }\nchip RC(p: P) -> (r: float) { return p.a / 4 }\n", "out od = RC({ a: @, b: 1.0 })"),
    ("record enum payload", "type P = { a: int, b: float }\nenum E { W(P), No }\nvar e: E = E.No\n", "on go { e = E.W({ a: @, b: 1.0 })\nmatch e { W(p) => { x = p.a / 4 } No => { } } }"),
    ("record generic enum payload", "type P = { a: int, b: float }\nenum Opt<T> { Some(T), None }\nvar o: Opt<P> = Opt.None\n", "on go { o = Opt.Some({ a: @, b: 1.0 })\nmatch o { Some(p) => { x = p.a / 4 } None => { } } }"),
    ("tuple var", "var t: (int, float) = (@, 1.0)\n", "on go { x = t.0 / 4 }"),
    ("tuple assignment", "var t: (int, float) = (0, 1.0)\n", "on go { t = (@, 1.0)\nx = t.0 / 4 }"),
    ("tuple let", "", "on go { let t: (int, float) = (@, 1.0)\nx = t.0 / 4 }"),
    ("tuple mod argument", "mod ta(t: (int, float)) -> float { return t.0 / 4 }\n", "on go { x = ta((@, 1.0)) }"),
    ("buffer", "buffer b: int = @\n", "on go { x = b / 4 }"),
    ("array index", "var ar: int[] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]\n", "on go { n = ar[@]\nx = n / 4 }"),
    ("string index", "var st: string = \"abcdefghijklmnop\"\n", "on go { s = st[@] }"),
    ("range bound", "", "on go { for i in 0..@ { m = m + i }\nx = m / 4 }"),
    ("range count", "", "on go { for i in count(@, from = 1) { m = m + i } }"),
];

/// A namespaced call is a route of its own.
const LIB: &str = "mod ih(v: int) -> float { return v / 4 }\nchip C(p: int) -> (r: int) { return p * 10 }\n";
const NAMESPACED: &[(&str, &str)] =
    &[("namespaced mod argument", "on go { x = u.ih(@) }"), ("namespaced chip argument", "out oc = u.C(@)")];

fn wsc(src: &str, deps: &[(&str, &str)], code: &str) -> usize {
    match compile_main(src, deps, true) {
        Ok(out) => out.diagnostics.iter().filter(|d| d.code == code).count(),
        Err(_) => 0,
    }
}

fn check_row(what: &str, src: &str, twin: &str, deps: &[(&str, &str)], kind: Kind, failures: &mut Vec<String>) {
    for fold in [true, false] {
        match (save_dump(src, deps, fold), save_dump(twin, deps, fold)) {
            (Ok(a), Ok(b)) if a == b => {}
            (Ok(a), Ok(b)) => {
                let only_a: Vec<&String> = a.iter().filter(|l| !b.contains(l)).take(4).collect();
                let only_b: Vec<&String> = b.iter().filter(|l| !a.contains(l)).take(4).collect();
                failures.push(format!("{what} {kind:?} fold {fold}: differs from the twin\n{src}\nonly here: {only_a:#?}\nonly twin: {only_b:#?}"));
            }
            (a, b) => failures.push(format!("{what} {kind:?} fold {fold}: {:?} / twin {:?}\n{src}", a.err(), b.err())),
        }
    }
    // The save holds the value as an integer: no float 13.5 or 13 in it.
    if let Ok(values) = save_values(src, deps, false, false)
        && let Some(f) = values.iter().find(|l| matches!(l, Literal::Float(f) if *f == 13.5 || *f == 13.0))
    {
        failures.push(format!("{what} {kind:?}: the save holds {f:?}\n{src}"));
    }
    let warned = wsc(src, deps, "WS106");
    if kind.drops_a_fraction() != (warned > 0) {
        failures.push(format!("{what} {kind:?}: {warned} WS106\n{src}"));
    }
    let mismatched = wsc(src, deps, "WS030");
    if mismatched > 0 {
        failures.push(format!("{what} {kind:?}: {mismatched} WS030\n{src}"));
    }
}

fn check_out_of_range(what: &str, src: &str, deps: &[(&str, &str)], failures: &mut Vec<String>) {
    for fold in [true, false] {
        match compile_main(src, deps, fold) {
            Err(codes) if codes.iter().any(|c| c == "WS028") => {}
            other => failures.push(format!("{what} fold {fold}: not WS028: {:?}\n{src}", other.err())),
        }
    }
}

#[test]
fn a_float_constant_in_an_int_place_is_the_integer_it_truncates_to_on_every_route() {
    let mut failures = Vec::new();
    let mut rows = 0;
    let mut twin_ints = 0;
    for (what, decls, body) in ROUTES {
        let program = |v: &str| format!("{HEAD}{}{}", decls.replace('@', v), body.replace('@', v));
        let twin = program("13");
        // The route stores something the reader sees: the twin's 13, or what
        // it folds to.
        let values = save_values(&twin, &[], false, false).unwrap_or_else(|c| panic!("{what}: {c:?}\n{twin}"));
        if values.iter().any(|l| matches!(l, Literal::Int(13) | Literal::Int(3) | Literal::Int(130) | Literal::Int(14))) {
            twin_ints += 1;
        } else {
            failures.push(format!("{what}: the twin holds no integer from the route\n{twin}\n{values:?}"));
        }
        for kind in Kind::ALL {
            check_row(what, &program(kind.text()), &program(kind.twin()), &[], kind, &mut failures);
            rows += 1;
        }
        for v in OUT_OF_RANGE {
            check_out_of_range(&format!("{what} {v}"), &program(v), &[], &mut failures);
            rows += 1;
        }
    }
    for (what, body) in NAMESPACED {
        let program = |v: &str| format!("import * as u from \"lib\"\n{HEAD}{}", body.replace('@', v));
        let deps = [("lib", LIB)];
        for kind in Kind::ALL {
            check_row(what, &program(kind.text()), &program(kind.twin()), &deps, kind, &mut failures);
            rows += 1;
        }
        for v in OUT_OF_RANGE {
            check_out_of_range(&format!("{what} {v}"), &program(v), &deps, &mut failures);
            rows += 1;
        }
    }
    assert!(twin_ints > 0);
    assert!(failures.is_empty(), "{} of {rows} rows failed:\n{}", failures.len(), failures.join("\n\n"));
}

/// A negated integer constant (`--2`, `-(-2)`, `-(-(-2))`, `-(-K)`, `-K`) is
/// the integer it names, in both fold modes: it equals the twin writing that
/// integer at the save. Negating the least `int` is `WS028`. A negated float
/// constant stays a float.
#[test]
fn a_negated_integer_constant_is_an_integer_in_both_fold_modes() {
    let head = "in go: exec\nconst K2 = 2\nconst F = 2.5\nvar n: int = 0\nvar x: float = 0.0\n";
    let mut failures = Vec::new();
    let rows = [
        ("n = --2", "n = 2"),
        ("n = -(-2)", "n = 2"),
        ("n = -(-(-2))", "n = -2"),
        ("n = -(-K2)", "n = 2"),
        ("n = -K2", "n = -2"),
        ("var u = -(-2)\n x = u / 4", "var u = 2\n x = u / 4"),
    ];
    for (src, twin) in rows {
        let (a, b) = (format!("{head}on go {{ {src} }}"), format!("{head}on go {{ {twin} }}"));
        check_row(src, &a, &b, &[], Kind::IntLiteral, &mut failures);
    }
    // The one integer no negation of which an `int` holds.
    for src in ["n = -(-9223372036854775808)", "n = -KN"] {
        let program = format!("{head}const KN = -9223372036854775808\non go {{ {src} }}");
        for fold in [true, false] {
            match compile_main(&program, &[], fold) {
                Err(codes) if codes.iter().any(|c| c == "WS028") => {}
                other => failures.push(format!("{src} fold {fold}: not WS028: {:?}", other.err())),
            }
        }
    }
    let float = format!("{head}on go {{ x = -(-2.5) }}");
    let twin = format!("{head}on go {{ x = 2.5 }}");
    match (save_dump(&float, &[], true), save_dump(&twin, &[], true)) {
        (Ok(a), Ok(b)) if a == b => {}
        other => failures.push(format!("float negation: {other:?}")),
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// An integer constant a `mod`, `chip` or generic parameter is bound to,
/// negated in the body, that goes into a `float` place is the float the
/// program writing `-2` there stores: with folding on the save equals that
/// twin, and in neither fold mode does it hold the integer `-2`.
#[test]
fn a_negated_integer_parameter_going_into_a_float_place_is_a_float() {
    let head = "in go: exec\nin k: int\nvar n: int = 0\nvar f: float = 0.0\nconst K = 2\n";
    let neg = "mod m(x: int) -> int { return -x }\n";
    let rows: [(&str, String, &str); 14] = [
        ("float var", format!("{neg}on go {{ f = m(2) }}"), "on go { f = -2 }"),
        ("mod declared float", "mod m(x: int) -> float { return -x }\non go { f = m(2) }".into(), "on go { f = -2.0 }"),
        ("chip float output", "chip C(x: int) { out o: float = -x }\nlet c = C(2)\non go { f = c.o }".into(), "on go { f = -2 }"),
        ("chip handler", "chip C(x: int) { on go { f = -x } }\nC(2)".into(), "chip C(x: int) { on go { f = -2 } }\nC(2)"),
        ("float array push", format!("{neg}var a: float[] = []\non go {{ a.push(m(2)) }}"), "var a: float[] = []\non go { a.push(-2) }"),
        ("float array write", format!("{neg}var fa: float[] = [0.0]\non go {{ fa[0] = m(2) }}"), "var fa: float[] = [0.0]\non go { fa[0] = -2 }"),
        ("float map value", format!("{neg}var mp: Map<int, float> = {{}}\non go {{ mp.set(1, m(2)) }}"), "var mp: Map<int, float> = {}\non go { mp.set(1, -2) }"),
        (
            "float record field",
            "type P = { a: int, b: float }\nmod m(x: int) -> P { return { a: 0, b: -x } }\nvar p: P = { a: 0, b: 0.0 }\non go { p = m(2) }".into(),
            "type P = { a: int, b: float }\nvar p: P = { a: 0, b: 0.0 }\non go { p = { a: 0, b: -2 } }",
        ),
        ("float parameter", format!("{neg}mod h(y: float) -> float {{ return y }}\non go {{ f = h(m(2)) }}"), "mod h(y: float) -> float { return y }\non go { f = h(-2) }"),
        ("generic", "mod g<T: int | float>(x: T) -> T { return -x }\non go { f = g(2) }".into(), "on go { f = -2 }"),
        ("float out", format!("{neg}out o: float = m(2)"), "out o: float = -2"),
        ("divided after", format!("{neg}on go {{ f = m(2)\n f = f / k }}"), "on go { f = -2\n f = f / k }"),
        ("vector component", "mod m(x: int) -> vector { return Vec(-x, 0.0, 0.0) }\nvar v: vector = Vec(0.0, 0.0, 0.0)\non go { v = m(2) }".into(), "var v: vector = Vec(0.0, 0.0, 0.0)\non go { v = Vec(-2, 0.0, 0.0) }"),
        ("constant argument", format!("{neg}on go {{ f = m(K) }}"), "on go { f = -2 }"),
    ];
    let mut failures = Vec::new();
    for (what, src, twin) in rows {
        let (a, b) = (format!("{head}{src}"), format!("{head}{twin}"));
        match (save_dump(&a, &[], true), save_dump(&b, &[], true)) {
            (Ok(x), Ok(y)) if x == y => {}
            other => failures.push(format!("{what}: differs from the twin writing -2: {other:?}")),
        }
        for fold in [true, false] {
            match save_values(&a, &[], fold, false) {
                Ok(values) if values.contains(&Literal::Int(-2)) => {
                    failures.push(format!("{what} fold {fold}: the save holds the integer -2"));
                }
                Ok(_) => {}
                Err(c) => failures.push(format!("{what} fold {fold}: {c:?}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Each diagnostic of `src` (with `deps`) as `CODE message`.
fn diagnostic_lines(src: &str, deps: &[(&str, &str)]) -> Vec<String> {
    let loader = crate::resolve::MemLoader {
        files: deps.iter().map(|(k, v)| (format!("{k}.ws"), v.to_string())).collect(),
    };
    let input = crate::CompileInput { source: src, file: "main", module_name: None, fold_mode: crate::FoldMode::ForceOn };
    let diags = match crate::compile_with_loader(input, Default::default(), &loader) {
        Ok(out) => out.diagnostics,
        Err(crate::CompileError::HasErrors(errors)) => errors,
        Err(e) => panic!("{e:?}\n{src}"),
    };
    diags.iter().map(|d| format!("{} {}", d.code, d.message)).collect()
}

/// A float literal no float holds exactly, going into an `int`, warns `WS106`
/// with the value stored; a literal quoted in `WS106` or `WS028` is quoted as
/// written.
#[test]
fn a_float_literal_going_into_an_int_is_quoted_as_written() {
    let head = "in go: exec\nvar n: int = 0\n";
    let lines = |v: &str| diagnostic_lines(&format!("{head}on go {{ n = {v} }}"), &[]);
    let inexact = lines("4503599627370497.5");
    assert!(inexact.iter().any(|l| l.starts_with("WS106 4503599627370497.5") && l.contains("4503599627370498")), "{inexact:?}");
    let whole = lines("9007199254740993.0");
    assert!(whole.iter().any(|l| l.starts_with("WS106 9007199254740993.0")), "{whole:?}");
    let exact = lines("4503599627370496.0");
    assert!(!exact.iter().any(|l| l.starts_with("WS106")), "{exact:?}");
    let max = lines("9223372036854775807.0");
    assert!(max.iter().any(|l| l.starts_with("WS028 9223372036854775807.0 goes")), "{max:?}");
    // Written within the `int` range, it is a float beyond it: the message says which.
    assert!(max.iter().any(|l| l.contains("where it is the float 9223372036854775808")), "{max:?}");
    let beyond = lines("1e30");
    assert!(beyond.iter().any(|l| l.starts_with("WS028 1e30") && !l.contains("where it is the float")), "{beyond:?}");
    let min = lines("-9.223372036854775808e18");
    assert!(min.is_empty(), "{min:?}");
}

/// A loop stride is no `int` place: a constant float stride steps by its
/// value in both fold modes (the save holds it and the count it gives), every
/// spelling of it saving as the literal does. A stride that is not greater
/// than zero, integer or float, written or folded, is `WS077`, a literal
/// quoted as written.
#[test]
fn a_constant_stride_keeps_its_value_and_a_non_positive_one_is_ws077() {
    let head = "in go: exec\nvar n: int = 0\nvar x: float = 0.0\nvar arr: int[] = [1, 2]\n\
                const S05 = 0.5\nconst S1 = 1.0\nconst SN = -0.5\nconst Z = 0\n\
                const mod half() -> float { return 0.5 }\nconst mod neg() -> float { return -0.5 }\n";
    let shapes = [
        ("range", "on go { for i in (0..6).step(@) { x = x + i } }", 0.5),
        ("count", "on go { for i in count(12, from = 1, step = @) { x = x + i } }", 0.5),
        ("rev", "on go { for i in (0..6).step(@).rev() { x = x + i } }", -0.5),
        ("nested", "on go { for j in 0..2 { for i in (0..6).step(@) { for v in arr { x = x + i * v } } } }", 0.5),
    ];
    let mut failures = Vec::new();
    for (shape, body, stride) in shapes {
        let program = |v: &str| format!("{head}{}", body.replace('@', v));
        for fold in [true, false] {
            let literal = save_dump(&program("0.5"), &[], fold);
            match save_values(&program("0.5"), &[], fold, false) {
                Ok(values) => {
                    if !values.contains(&Literal::Float(stride)) || !values.contains(&Literal::Int(12)) {
                        failures.push(format!("{shape} fold {fold}: no stride {stride} or count 12: {values:?}"));
                    }
                }
                Err(c) => failures.push(format!("{shape} fold {fold}: {c:?}")),
            }
            for form in ["S05", "(S1 - 0.5)", "half()", "(1.0 / 2.0)"] {
                if save_dump(&program(form), &[], fold) != literal {
                    failures.push(format!("{shape} {form} fold {fold}: differs from the literal 0.5"));
                }
            }
        }
        for form in ["0.5", "S05", "half()"] {
            let warned = wsc(&program(form), &[], "WS106");
            if warned > 0 {
                failures.push(format!("{shape} {form}: {warned} WS106"));
            }
        }
        for form in ["0", "0.0", "-1", "-1.5", "Z", "SN", "(S05 - 0.5)", "(0.0 / 0.0)", "-1e30", "neg()"] {
            for fold in [true, false] {
                match compile_main(&program(form), &[], fold) {
                    Err(codes) if codes.iter().any(|c| c == "WS077") => {}
                    other => failures.push(format!("{shape} {form} fold {fold}: not WS077: {:?}", other.err())),
                }
            }
        }
    }
    let quoted = |v: &str| diagnostic_lines(&format!("{head}on go {{ for i in (0..6).step({v}) {{ }} }}"), &[]);
    for (v, got) in [("0.0", "got 0.0."), ("-1.5", "got -1.5."), ("-1", "got -1."), ("SN", "got -0.5.")] {
        let lines = quoted(v);
        if !lines.iter().any(|l| l.starts_with("WS077") && l.contains(got)) {
            failures.push(format!("{v}: {lines:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The bound a string method on a value of an unbounded type parameter
/// suggests is one the program's calls compile under: when a call gives the
/// parameter a type that is no string, none is suggested.
#[test]
fn a_string_bound_is_suggested_only_where_every_call_takes_it() {
    let src = |arg: &str| {
        format!(
            "in go: exec\nvar v: vector = Vec(1.0, 2.0, 3.0)\nvar fl: float = 0.0\n\
             mod f<T>(a: T) -> float {{ return a.Length() }}\non go {{ fl = f({arg}) }}"
        )
    };
    let vector = diagnostic_lines(&src("v"), &[]);
    assert!(
        vector.iter().any(|l| l.starts_with("WS003") && l.contains("a call gives `T` a `vector`") && !l.contains("<T: string>")),
        "{vector:?}"
    );
    let string = diagnostic_lines(&src("\"ab\""), &[]);
    assert!(string.iter().any(|l| l.starts_with("WS003") && l.contains("bound it: `<T: string>`")), "{string:?}");
}

/// An integer literal given to a `bool` parameter is the bool it names, in
/// both fold modes, as the twin writing `true` or `false`; a bool literal
/// given to an `int` parameter is the integer, as the twin writing `1` or `0`.
#[test]
fn a_literal_given_to_a_bool_or_int_parameter_is_that_type() {
    let mut failures = Vec::new();
    for (form, call) in [
        ("chip C(p: bool) { on go { s = \"${p}\" } }\n", "C(@)"),
        ("mod M(p: bool) { s = \"${p}\" }\n", "on go { M(@) }"),
        ("mod M(p: bool) -> int { return if p then 1 else 2 }\n", "on go { m = M(@) }"),
    ] {
        for (int, b) in [("1", "true"), ("0", "false")] {
            let p = |v: &str| format!("{HEAD}{form}{}", call.replace('@', v));
            for fold in [true, false] {
                match (save_dump(&p(int), &[], fold), save_dump(&p(b), &[], fold)) {
                    (Ok(a), Ok(t)) if a == t => {}
                    other => failures.push(format!("{form}{call} {int} fold {fold}: {other:?}")),
                }
            }
        }
    }
    // The mirror: a bool literal given to an `int` parameter is the integer.
    for (form, call) in [
        ("chip C(p: int) { on go { s = \"${p}\" } }\n", "C(@)"),
        ("mod M(p: int) { s = \"${p}\" }\n", "on go { M(@) }"),
        ("mod M(p: int) -> int { return p * 3 }\n", "on go { m = M(@) }"),
        ("mod G<T>(p: T) -> T { return p }\n", "on go { m = G<int>(@) }"),
    ] {
        for (b, int) in [("true", "1"), ("false", "0")] {
            let p = |v: &str| format!("{HEAD}{form}{}", call.replace('@', v));
            for fold in [true, false] {
                match (save_dump(&p(b), &[], fold), save_dump(&p(int), &[], fold)) {
                    (Ok(a), Ok(t)) if a == t => {}
                    other => failures.push(format!("{form}{call} {b} fold {fold}: {other:?}")),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
