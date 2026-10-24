//! The float-slot rule on every route a value takes into a typed slot: an
//! integer constant (a literal, or a name bound to one) flowing into a `float`
//! slot is that float, folded or not; a runtime value keeps its wire; and an
//! integer constant in an `int` slot stays an integer. One rule
//! (`const_eval::coerce_const_to_type`, recorded by
//! `typecheck::infer::int_constants_in_slot`) serves every route.

use super::*;
use crate::ir::Literal;

/// Every literal in the module and its chips, nested container contents
/// included.
fn literals(m: &crate::ir::Module, out: &mut Vec<Literal>) {
    fn push(l: &Literal, out: &mut Vec<Literal>) {
        match l {
            Literal::Array(items) => items.iter().for_each(|i| push(i, out)),
            Literal::Map(pairs) => pairs.iter().for_each(|(k, v)| {
                push(k, out);
                push(v, out);
            }),
            other => out.push(other.clone()),
        }
    }
    m.nodes_deep()
        .flat_map(|n| n.properties.values())
        .for_each(|l| push(l, out));
}

/// What a value of `kind` is written as in a route's source, and the
/// declarations it needs.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    IntLiteral,
    IntConst,
    IntVar,
    FloatLiteral,
    FloatVar,
    /// `const C13 = TWELVE + 1`, read by name.
    ConstChain,
    /// `let L13 = TWELVE + 1`, read by name.
    LetChain,
    /// `(TWELVE + 1)`, the arithmetic itself.
    Arithmetic,
    /// `-MINUS13`, a negated constant name.
    Negated,
    /// `plus1(TWELVE)`, a `const mod` of a constant.
    ConstModCall,
    /// `TBL[1]`, an element of a constant array.
    ConstIndex,
    /// `REC.a`, a field of a constant record.
    ConstField,
    /// `(if TWELVE > 0 then C13 else 12)`, an `if` of constants.
    ConstIf,
}

impl Kind {
    const ALL: [Kind; 13] = [
        Kind::IntLiteral,
        Kind::IntConst,
        Kind::IntVar,
        Kind::FloatLiteral,
        Kind::FloatVar,
        Kind::ConstChain,
        Kind::LetChain,
        Kind::Arithmetic,
        Kind::Negated,
        Kind::ConstModCall,
        Kind::ConstIndex,
        Kind::ConstField,
        Kind::ConstIf,
    ];

    fn text(self) -> &'static str {
        match self {
            Kind::IntLiteral => "13",
            Kind::IntConst => "THIRTEEN",
            Kind::IntVar => "iv",
            Kind::FloatLiteral => "13.0",
            Kind::FloatVar => "fv",
            Kind::ConstChain => "C13",
            Kind::LetChain => "L13",
            Kind::Arithmetic => "(TWELVE + 1)",
            Kind::Negated => "-MINUS13",
            Kind::ConstModCall => "plus1(TWELVE)",
            Kind::ConstIndex => "TBL[1]",
            Kind::ConstField => "REC.a",
            Kind::ConstIf => "(if TWELVE > 0 then C13 else 12)",
        }
    }

    fn constant(self) -> bool {
        !matches!(self, Kind::IntVar | Kind::FloatVar)
    }
}

/// The declarations every kind reads. None of them holds a 13 or a 6
/// literally, so one in the save came from a route.
const HEAD: &str = "in go: exec\nin c: bool\nin iv: int\nin fv: float\nconst THIRTEEN = 13\n\
                    const TWELVE = 12\nconst C13 = TWELVE + 1\nlet L13 = TWELVE + 1\nconst MINUS13 = -TWELVE - 1\n\
                    const TBL = [12, TWELVE + 1]\nconst REC = { a: TWELVE + 1, b: 0 }\n\
                    const mod plus1(v: int) -> int { return v + 1 }\n\
                    var x: float = 0.0\nout ox: float = x\n\
                    mod half(v: float) -> float { return v / 2 }\n\
                    mod pick<T: int | float>(c: bool, a: T, b: T) -> T { return if c then a else b }\n";

/// Every integer and float a component of the compiled save holds, read back
/// from the `.brz` the way the game loads it (a wire value bakes as the
/// variant it holds, so an `i64 13` here is an integer the game reads).
pub(super) fn save_numbers(src: &str, fold: bool) -> Vec<Literal> {
    read_save(src, &[], fold).0
}

/// [`save_numbers`] for a program importing `deps` (`(name, source)`), and
/// how many grids the save holds. Every grid is read: their ids are sparse
/// (a chip's grid need not follow the one before it), so they come from the
/// save's own `Grids` folder rather than counting up from 1.
fn read_save(src: &str, deps: &[(&str, &str)], fold: bool) -> (Vec<Literal>, usize) {
    try_read_save(src, deps, fold).unwrap_or_else(|codes| panic!("{codes:?}\n{src}"))
}

/// [`read_save`], or the codes of the errors when `src` does not compile.
fn try_read_save(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
) -> Result<(Vec<Literal>, usize), Vec<String>> {
    read_save_values(src, deps, fold, false)
}

/// [`try_read_save`], with every narrower integer, `f32` and bool field read
/// too when `narrow` is set.
fn read_save_values(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
    narrow: bool,
) -> Result<(Vec<Literal>, usize), Vec<String>> {
    use brdb::schema::BrdbValue;
    fn go(v: &BrdbValue, out: &mut Vec<Literal>, narrow: bool) {
        match v {
            BrdbValue::I64(n) => out.push(Literal::Int(*n)),
            BrdbValue::F64(f) => out.push(Literal::Float(*f)),
            BrdbValue::I32(n) if narrow => out.push(Literal::Int(i64::from(*n))),
            BrdbValue::I16(n) if narrow => out.push(Literal::Int(i64::from(*n))),
            BrdbValue::I8(n) if narrow => out.push(Literal::Int(i64::from(*n))),
            BrdbValue::U8(n) if narrow => out.push(Literal::Int(i64::from(*n))),
            BrdbValue::F32(f) if narrow => out.push(Literal::Float(f64::from(*f))),
            BrdbValue::Bool(b) if narrow => out.push(Literal::Bool(*b)),
            BrdbValue::Struct(s) => s.properties.values().for_each(|p| go(p, out, narrow)),
            BrdbValue::Array(items) | BrdbValue::FlatArray(items) => {
                items.iter().for_each(|i| go(i, out, narrow))
            }
            BrdbValue::DynamicArray(a) => a.values.iter().for_each(|i| go(i, out, narrow)),
            BrdbValue::DynamicMap(m) => m.entries.iter().for_each(|(k, v)| {
                go(k, out, narrow);
                go(v, out, narrow);
            }),
            BrdbValue::Map(m) => m.iter().for_each(|(k, v)| {
                go(k, out, narrow);
                go(v, out, narrow);
            }),
            _ => {}
        }
    }
    let walk = |v: &BrdbValue, out: &mut Vec<Literal>| go(v, out, narrow);
    let out = super::declaring_scope::compiled(src, deps, fold)?;
    assert!(out.grid_count() > 0, "no grid in the save\n{src}");
    let mut nums = Vec::new();
    for c in out.components() {
        c.properties.values().for_each(|p| walk(p, &mut nums));
    }
    Ok((nums, out.grid_count()))
}

/// Whether the module's `index`th input feeds anything.
fn input_is_wired(m: &crate::ir::Module, index: usize) -> bool {
    m.inputs
        .get(index)
        .is_some_and(|id| m.wires.iter().any(|w| w.source.node_id == *id))
}

/// The routes into a `float` slot: `@` is the value, and each ends in a
/// division by 2 that reads 6 for an integer and 6.5 for a float.
const FLOAT_ROUTES: &[(&str, &str, &str)] = &[
    ("assignment", "", "on go { x = @\nx = x / 2 }"),
    ("mod parameter", "", "on go { x = half(@) }"),
    ("generic parameter", "", "on go { x = pick(c, @, 2.5) / 2 }"),
    (
        "chip parameter",
        "chip D(v: float) -> (r: float) { return v / 2 }\n",
        "out od = D(@)",
    ),
    (
        "annotated let",
        "",
        "on go {\nlet l: float = @\nx = l / 2\n}",
    ),
    (
        "return value",
        "mod give() -> float { return @ }\n",
        "on go { x = give() / 2 }",
    ),
    (
        "chip return value",
        "chip Sev() -> float { return @ }\n",
        "on go { x = Sev() / 2 }",
    ),
    ("float output", "out of: float\n", "on go { emit of = @ }"),
    (
        "array push",
        "var fs: float[]\n",
        "on go { fs.push(@)\nx = fs[0] / 2 }",
    ),
    (
        "map set",
        "var fm: Map<int, float>\n",
        "on go { fm.set(1, @)\nx = fm[1] / 2 }",
    ),
    (
        "enum payload",
        "enum S { Circle(float), Sq }\nvar s: S = S.Sq\n",
        "on go { s = S.Circle(@)\nmatch s { Circle(r) => { x = r / 2 } Sq => { } } }",
    ),
    (
        "enum named payload",
        "enum S { Box { w: float }, Sq }\nvar s: S = S.Sq\n",
        "on go { s = S.Box { w: @ }\nmatch s { Box { w } => { x = w / 2 } Sq => { } } }",
    ),
    (
        "record literal push",
        "type P = { a: float, b: int }\nvar ps: P[]\n",
        "on go { ps.push({ a: @, b: 1 })\nx = ps[0].a / 2 }",
    ),
    (
        "record literal assign",
        "type P = { a: float, b: int }\nvar p: P = { a: 1.0, b: 1 }\n",
        "on go { p = { a: @, b: 1 }\nx = p.a / 2 }",
    ),
    (
        "record literal let",
        "type P = { a: float, b: int }\n",
        "on go {\nlet r: P = { a: @, b: 1 }\nx = r.a / 2\n}",
    ),
    (
        "tuple assign",
        "var t: (float, int) = (0.0, 0)\n",
        "on go { t = (@, 1)\nx = t.0 / 2 }",
    ),
    (
        "tuple let",
        "",
        "on go {\nlet u: (float, int) = (@, 1)\nx = u.0 / 2\n}",
    ),
    ("if join", "", "on go { x = (if c then @ else 2.5) / 2 }"),
    (
        "match join",
        "enum E { A, B }\nvar e: E = E.A\n",
        "on go { x = (match e { A => @, B => 2.5 }) / 2 }",
    ),
];

/// Constant-only routes: a `const` table and map, and a constant read through
/// an annotated `const`.
const CONSTANT_ROUTES: &[(&str, &str, &str)] = &[
    (
        "const array",
        "const A: float[] = [@, 4.0]\nin k: int\n",
        "on go { x = A[k] / 2 }",
    ),
    (
        "const map",
        "const M: Map<string, float> = { \"a\": @ }\n",
        "on go { x = M[\"a\"] / 2 }",
    ),
    (
        "annotated const",
        "const K: float = @\nconst KK = K / 2\n",
        "on go { x = KK }",
    ),
];

/// The IR literals and the save of `src` in both fold modes, each pair from
/// one compile.
fn check_float_slot(what: &str, src: &str, kind: Kind) {
    for (mode, fold) in [("nofold", false), ("fold", true)] {
        let compiled = super::declaring_scope::compiled(src, &[], fold)
            .unwrap_or_else(|codes| panic!("{what} {kind:?} {mode}: {codes:?}\n{src}"));
        let module = compiled.module();
        let mut lits = Vec::new();
        literals(module, &mut lits);
        let saved = save_numbers(src, fold);
        for (stage, nums) in [("IR", &lits), ("save", &saved)] {
            let ints = nums
                .iter()
                .filter(|l| matches!(l, Literal::Int(13) | Literal::Int(6)))
                .count();
            assert_eq!(
                ints, 0,
                "{what} {kind:?} {mode} {stage}: an integer 13 or 6 in a float slot\n{src}\n{nums:?}"
            );
            let floats = nums
                .iter()
                .any(|l| matches!(l, Literal::Float(f) if *f == 13.0 || *f == 6.5));
            if kind.constant() {
                assert!(
                    floats,
                    "{what} {kind:?} {mode} {stage}: the float is missing\n{src}\n{nums:?}"
                );
            } else {
                assert!(
                    !floats,
                    "{what} {kind:?} {mode} {stage}: a runtime value became a constant\n{src}\n{nums:?}"
                );
            }
        }
        // A runtime value keeps its wire: an `int` variable reaches the slot
        // as the integer it holds (no conversion gate, documented).
        if !kind.constant() {
            let input = if kind == Kind::IntVar { 2 } else { 3 };
            assert!(
                input_is_wired(module, input),
                "{what} {kind:?} {mode}: the input's wire is gone\n{src}"
            );
        }
    }
}

/// Routes that take every kind in the sampled set: a store, a mod and a chip
/// parameter, a container, an enum payload and a join.
const FULL_FLOAT_ROUTES: &[&str] = &[
    "assignment",
    "mod parameter",
    "chip parameter",
    "map set",
    "enum named payload",
    "if join",
];

/// The kinds route `i` takes. `full` takes all of them; otherwise a route
/// outside [`FULL_FLOAT_ROUTES`] takes one runtime kind and two constant
/// kinds, each rotating with `i` so every kind meets several routes.
fn float_route_kinds(i: usize, what: &str, full: bool) -> Vec<Kind> {
    if full || FULL_FLOAT_ROUTES.contains(&what) {
        return Kind::ALL.to_vec();
    }
    let runtime: Vec<Kind> = Kind::ALL.into_iter().filter(|k| !k.constant()).collect();
    let constant: Vec<Kind> = Kind::ALL.into_iter().filter(|k| k.constant()).collect();
    vec![
        runtime[i % runtime.len()],
        constant[(2 * i) % constant.len()],
        constant[(2 * i + 1) % constant.len()],
    ]
}

/// The default set runs every route with three kinds and the routes of
/// [`FULL_FLOAT_ROUTES`] with all of them; the full product runs under
/// `--ignored`.
#[test]
fn an_integer_constant_in_a_float_slot_is_a_float_on_every_route() {
    integer_constant_in_a_float_slot_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn an_integer_constant_in_a_float_slot_is_a_float_on_every_route_exhaustive() {
    integer_constant_in_a_float_slot_matrix(true);
}

fn integer_constant_in_a_float_slot_matrix(full: bool) {
    let mut checked = 0;
    let mut kinds_seen = Vec::new();
    let names: Vec<&str> = FLOAT_ROUTES.iter().map(|r| r.0).collect();
    assert!(
        FULL_FLOAT_ROUTES.iter().all(|r| names.contains(r)),
        "a full route is not a route"
    );
    for (i, (what, decls, body)) in FLOAT_ROUTES.iter().enumerate() {
        for kind in float_route_kinds(i, what, full) {
            let src = format!(
                "{HEAD}{}{}",
                decls.replace('@', kind.text()),
                body.replace('@', kind.text())
            );
            check_float_slot(what, &src, kind);
            kinds_seen.push(kind);
            checked += 1;
        }
        // Positive controls for the runtime kinds' absence checks: the same
        // route given the constant stores the float, and given an integer
        // computed at run time stores the integer 13 it adds, so the save
        // reader sees what this route stores.
        for (text, float) in [("13", true), ("(13 + iv)", false)] {
            let src = format!(
                "{HEAD}{}{}",
                decls.replace('@', text),
                body.replace('@', text)
            );
            for fold in [false, true] {
                let saved = save_numbers(&src, fold);
                let found = saved.iter().any(|l| match l {
                    Literal::Float(f) => float && (*f == 13.0 || *f == 6.5),
                    Literal::Int(n) => !float && *n == 13,
                    _ => false,
                });
                assert!(
                    found,
                    "{what} {text} fold {fold}: the control's value is missing\n{src}\n{saved:?}"
                );
            }
        }
    }
    for (what, decls, body) in CONSTANT_ROUTES {
        for kind in [
            Kind::IntLiteral,
            Kind::FloatLiteral,
            Kind::ConstChain,
            Kind::Arithmetic,
            Kind::ConstIndex,
        ] {
            let src = format!("{HEAD}{}{body}", decls.replace('@', kind.text()));
            check_float_slot(what, &src, kind);
            checked += 1;
        }
    }
    assert!(
        Kind::ALL.iter().all(|k| kinds_seen.contains(k)),
        "a kind never ran"
    );
    let routes = FLOAT_ROUTES.len() - FULL_FLOAT_ROUTES.len();
    let want = if full {
        FLOAT_ROUTES.len() * Kind::ALL.len() + CONSTANT_ROUTES.len() * 5
    } else {
        FULL_FLOAT_ROUTES.len() * Kind::ALL.len() + routes * 3 + CONSTANT_ROUTES.len() * 5
    };
    assert!(checked >= want, "{checked}");
}

/// A constant computed from a `float`-annotated constant is computed as a
/// float at compile time too: `K / 2` is 6.5, so a constant `if` on it keeps
/// its branch (it read 6, dropped the branch, and stored 6 where the runtime
/// value was 6.5).
#[test]
fn a_float_annotated_constant_is_a_float_in_constant_evaluation() {
    let src = "in go: exec\nvar x: float = 0.0\nvar y: float = 0.0\nout ox: float = x\nout oy: float = y\n\
               const K: float = 13\nconst KK = K / 2\non go {\nif KK > 6.2 { x = 1.0 }\ny = KK\n}";
    for r in [compile(src), compile_folded(src)] {
        assert_no_errors(&r);
        let mut lits = Vec::new();
        literals(&r.module, &mut lits);
        assert!(
            lits.iter()
                .any(|l| matches!(l, Literal::Float(f) if *f == 1.0)),
            "the branch is kept: {lits:?}"
        );
        assert!(
            !lits.iter().any(|l| matches!(l, Literal::Int(6))),
            "KK is not 6: {lits:?}"
        );
    }
}

/// An annotated `let`/`const` whose value the annotation makes a float (an
/// integer constant, or a literal built to the annotation's shape) binds the
/// annotation, as a `var` does: `const A: float[] = [13, 4]` is a `float[]`
/// table, not an `int[]` one holding floats.
#[test]
fn an_annotated_let_or_const_binds_its_annotation() {
    use crate::ir::Type;
    let float = || Box::new(Type::Float);
    for (decl, name, want) in [
        ("const A: float[] = [13, 4]", "A", Type::Array(float())),
        (
            "const M: Map<string, float> = { \"a\": 13 }",
            "M",
            Type::Map(Box::new(Type::String), float()),
        ),
        ("let L: float = 13", "L", Type::Float),
        ("const K: float = 13", "K", Type::Float),
        (
            "let T: (float, int) = (13, 1)",
            "T",
            Type::Tuple(vec![Type::Float, Type::Int]),
        ),
    ] {
        let src = format!(
            "in go: exec\n{decl}\nvar seen: float = 0.0\nout o: float = seen\non go {{\nlet r = {name}\n}}"
        );
        let tc = crate::typecheck::typecheck(
            &crate::parser::parse(&src, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        );
        let at = src.rfind(name).unwrap();
        let read = tc
            .type_of_expr
            .iter()
            .find(|((_, s, e), _)| *s == at && *e == at + name.len())
            .map(|(_, t)| t.clone());
        assert_eq!(read, Some(want), "{decl}");
    }
}

/// A namespaced call is a route of its own.
#[test]
fn an_integer_constant_into_a_namespaced_float_parameter_is_a_float() {
    let lib = "mod half(v: float) -> float { return v / 2 }\n";
    for kind in [Kind::IntLiteral, Kind::IntConst, Kind::FloatLiteral] {
        for prefix in ["", "@nofold\n\n"] {
            let src = format!(
                "{prefix}import * as u from \"lib\"\nin go: exec\nconst THIRTEEN = 13\nvar x: float = 0.0\nout ox: float = x\non go {{ x = u.half({}) }}",
                kind.text()
            );
            let r = compile_multi(&src, &[("lib", lib)]);
            assert_no_errors(&r);
            let mut lits = Vec::new();
            literals(&r.module, &mut lits);
            assert!(
                !lits
                    .iter()
                    .any(|l| matches!(l, Literal::Int(13) | Literal::Int(6))),
                "{kind:?} {prefix:?}: {lits:?}"
            );
            assert!(
                lits.iter()
                    .any(|l| matches!(l, Literal::Float(f) if *f == 13.0 || *f == 6.5)),
                "{kind:?} {prefix:?}: {lits:?}"
            );
        }
    }
    // The argument itself is typed `float` at the call, as a flat call's is.
    let loader = crate::resolve::MemLoader {
        files: [("lib.ws".to_string(), lib.to_string())]
            .into_iter()
            .collect(),
    };
    let src = "import * as u from \"lib\"\nin go: exec\nvar x: float = 0.0\nout ox: float = x\non go { x = u.half(13) }";
    let resolved = crate::resolve::resolve(src, "main", &loader);
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "main",
        &crate::typecheck::CeSlotMap::default(),
    );
    let at = src.find("13").unwrap();
    let arg = tc
        .type_of_expr
        .iter()
        .find(|((f, s, e), _)| &**f == "main" && *s == at && *e == at + 2)
        .map(|(_, t)| t.clone());
    assert_eq!(arg, Some(crate::ir::Type::Float));
}

/// The same routes into an `int` slot keep an integer constant an integer,
/// including an `if` whose other branch is a float (the `int` slot decides)
/// and a generic whose other argument is `min` of integers (an integer).
#[test]
fn an_integer_constant_in_an_int_slot_stays_an_integer() {
    let head = "in go: exec\nin c: bool\nin k: int\nin fk: float\nconst THIRTEEN = 13\nvar n: int = 0\nout on_: int = n\n\
                const TWELVE = 12\nconst C13 = TWELVE + 1\nlet L13 = TWELVE + 1\nconst MINUS13 = -TWELVE - 1\n\
                const TBL = [12, TWELVE + 1]\nconst REC = { a: TWELVE + 1, b: 0 }\n\
                const mod plus1(v: int) -> int { return v + 1 }\n\
                mod ihalf(v: int) -> int { return v / 2 }\n\
                mod pick<T: int | float>(c: bool, a: T, b: T) -> T { return if c then a else b }\n";
    let routes: &[(&str, &str)] = &[
        ("", "on go { n = @\nn = n / 2 }"),
        ("", "on go { n = ihalf(@) }"),
        ("", "on go { n = pick(c, @, min(k, 3)) / 2 }"),
        ("", "on go { n = pick(c, @, max(k, 3)) / 2 }"),
        ("", "on go { n = pick(c, @, abs(k)) / 2 }"),
        ("", "on go { n = pick(c, @, clamp(k, 0, 9)) / 2 }"),
        ("", "on go { n = if c then @ else fk }"),
        (
            "chip D(v: int) -> (r: int) { return v / 2 }\n",
            "out od = D(@)",
        ),
        ("var is: int[]\n", "on go { is.push(@)\nn = is[0] / 2 }"),
        (
            "enum S { Sq(int), No }\nvar s: S = S.No\n",
            "on go { s = S.Sq(@) }",
        ),
        (
            "type P = { a: int }\nvar ps: P[]\n",
            "on go { ps.push({ a: @ }) }",
        ),
        ("const A: int[] = [@, 4]\n", "on go { n = A[k] / 2 }"),
        (
            "mod give() -> int { return @ }\n",
            "on go { n = give() / 2 }",
        ),
    ];
    for (decls, body) in routes {
        for kind in Kind::ALL
            .into_iter()
            .filter(|k| k.constant() && *k != Kind::FloatLiteral)
        {
            let text = if decls.starts_with("const") {
                "13"
            } else {
                kind.text()
            };
            let src = format!(
                "{head}{}{}",
                decls.replace('@', text),
                body.replace('@', text)
            );
            for (mode, r) in [("nofold", compile(&src)), ("fold", compile_folded(&src))] {
                assert_no_errors(&r);
                let mut lits = Vec::new();
                literals(&r.module, &mut lits);
                assert!(
                    !lits
                        .iter()
                        .any(|l| matches!(l, Literal::Float(f) if *f == 13.0 || *f == 6.5)),
                    "{body} {kind:?} {mode}: a float in an int slot\n{lits:?}"
                );
            }
        }
    }
}

/// Each read of a constant takes its own slot's type: `N2` read into a
/// `float` parameter is the float 5, and read into an integer division beside
/// it stays the integer 5, so the two routes do not share one division.
#[test]
fn each_read_of_a_constant_expression_takes_its_own_slot() {
    let src = "in go: exec\nvar n: int = 0\nout onn: int = n\nvar f: float = 0.0\nout of: float = f\n\
               var f2: float = 0.0\nout of2: float = f2\nmod half(v: float) -> float { return v / 2 }\n\
               const N = 3\nconst N2 = N + 2\nlet L5 = N + 2\non go {\nf = half(N2)\nf2 = half(L5)\nn = N2 / 2\n}";
    for fold in [false, true] {
        let saved = save_numbers(src, fold);
        assert!(
            saved
                .iter()
                .any(|l| matches!(l, Literal::Float(f) if *f == 5.0)),
            "fold {fold}: no float 5\n{saved:?}"
        );
        let r = if fold {
            compile_ok_folded(src)
        } else {
            compile_ok(src)
        };
        let divides = gate_count(&r, "BrickComponentType_Expr_MathDivide");
        if fold {
            // `N2 / 2` folds to the integer 2; the float route keeps a live
            // division of the float 5.
            assert!(
                saved.iter().any(|l| matches!(l, Literal::Int(2))),
                "fold: n is not the integer 2\n{saved:?}"
            );
            assert_eq!(divides, 1, "fold: one float division");
        } else {
            assert_eq!(
                divides, 2,
                "nofold: the float and the integer routes have their own division"
            );
        }
    }
}

/// `min`, `max`, `clamp`, `abs`, `sign` and the roundings of integers type as
/// integers, so a generic given one infers `int`; once an operand is a float
/// the call is a float.
#[test]
fn integer_math_builtins_type_as_the_join_of_their_operands() {
    let head = "in k: int\nin f: float\n";
    for call in [
        "min(k, 3)",
        "max(k, 3)",
        "clamp(k, 0, 9)",
        "abs(k)",
        "sign(k)",
        "floor(k)",
        "ceil(k)",
        "round(k)",
    ] {
        let src = format!("{head}let r = {call}\nlet t: int = r");
        let tc = crate::typecheck::typecheck(
            &crate::parser::parse(&src, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        );
        let at = src.find(call).unwrap();
        let r_ty = tc
            .type_of_expr
            .iter()
            .find(|((_, s, e), _)| *s == at && *e == at + call.len())
            .map(|(_, t)| t.clone());
        assert_eq!(r_ty, Some(crate::ir::Type::Int), "{call}");
    }
    for call in ["min(k, f)", "max(f, 3)", "clamp(f, 0, 9)", "abs(f)"] {
        let src = format!("{head}let r = {call}");
        let tc = crate::typecheck::typecheck(
            &crate::parser::parse(&src, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        );
        let at = src.find(call).unwrap();
        let r_ty = tc
            .type_of_expr
            .iter()
            .find(|((_, s, e), _)| *s == at && *e == at + call.len())
            .map(|(_, t)| t.clone());
        assert_eq!(r_ty, Some(crate::ir::Type::Float), "{call}");
    }
}

/// An integer constant a float cannot hold exactly goes into a `float` slot
/// as the nearest float, so it warns (`WS104`) once per expression; one a
/// float holds exactly, and one in an `int` slot, do not.
#[test]
fn an_integer_constant_a_float_cannot_hold_warns() {
    let head = "in go: exec\nvar x: float = 0.0\nout ox: float = x\nvar n: int = 0\nout onn: int = n\n\
                mod half(v: float) -> float { return v / 2 }\nconst BIG = 9007199254740993\n";
    let count = |body: &str| {
        let r = compile_ok(&format!("{head}{body}"));
        r.diagnostics.iter().filter(|d| d.code == "WS104").count()
    };
    assert_eq!(count("on go { x = 9007199254740993 }"), 1);
    assert_eq!(count("on go { x = half(BIG) }"), 1);
    assert_eq!(count("on go { x = half(BIG + 2) }"), 1);
    assert_eq!(
        count("var y: float = -9007199254740993\nout oy: float = y"),
        1
    );
    assert_eq!(count("on go { x = 9007199254740992 }"), 0);
    assert_eq!(count("on go { n = 9007199254740993 }"), 0);
    assert_eq!(count("on go { x = 3 }"), 0);
    // A top-level `float[]` initializer's elements.
    assert_eq!(
        count("var arr: float[] = [BIG, 1]\non go { x = arr[0] }"),
        1
    );
    assert_eq!(
        count("var arr: float[] = [9007199254740993, BIG]\non go { x = arr[0] }"),
        2
    );
    assert_eq!(count("var arr: int[] = [BIG, 1]\non go { n = arr[0] }"), 0);
    assert_eq!(
        count("var arr: float[] = [9007199254740992, 1]\non go { x = arr[0] }"),
        0
    );
    // A parameter named like the constant is not the constant, read directly
    // or through a `let` copying it.
    assert_eq!(
        count("mod g(BIG: int) -> float { return BIG }\non go { x = g(n) }"),
        0
    );
    assert_eq!(
        count("mod g(BIG: int) -> float { let q = BIG\nreturn q }\non go { x = g(n) }"),
        0
    );
    assert_eq!(count("on go { for BIG in 0..n { x = half(BIG) } }"), 0);
}

/// A number derived from the shadowed constant 77 (`77`, `77 / 2`, `77 * 2`).
fn from_77(l: &Literal) -> bool {
    match l {
        Literal::Int(n) => [77, 38, 154].contains(n),
        Literal::Float(f) => [77.0, 38.0, 38.5, 154.0].contains(f),
        _ => false,
    }
}

/// A parameter or local named like an outer integer constant is its own value
/// in a `float` slot, in every binding form: the save holds nothing derived
/// from the constant, and the input the local was given still feeds the
/// program. Each row's control renames the binding, so the same read names the
/// constant, and the save holds the constant's value.
#[test]
fn a_local_named_like_a_constant_keeps_its_own_value() {
    let head = "in go: exec\nin a: int\nin fa: float\nin k: int\nconst N = 77\nlet W = 77\n\
                var f: float = 0.0\nout of: float = f\nmod half(v: float) -> float { return v / 2 }\n";
    // (binding form, the name it shadows, declarations, body, the input the
    // local is given); `@` is the binding's name.
    let rows: &[(&str, &str, &str, &str, usize)] = &[
        (
            "float mod parameter",
            "N",
            "mod h(@: float) -> float { return N / 2 }\n",
            "on go { f = h(fa) }",
            2,
        ),
        (
            "int mod parameter",
            "N",
            "mod h(@: int) -> float { return half(N) }\n",
            "on go { f = h(a) }",
            1,
        ),
        (
            "annotated let of a parameter",
            "N",
            "mod h(@: int) -> float { let q: float = N\nreturn q }\n",
            "on go { f = h(a) }",
            1,
        ),
        (
            "parameter shadowing a let",
            "W",
            "mod scale(@: float) -> float { return W * 2.0 }\n",
            "on go { f = scale(fa) }",
            2,
        ),
        (
            "chip parameter",
            "N",
            "chip C(t: exec, @: int) -> (r: float) { on t { emit r = half(N) } }\n",
            "on go { f = C(go, a) }",
            1,
        ),
        (
            "handler let",
            "N",
            "",
            "on go { let @ = a\nf = half(N) }",
            1,
        ),
        (
            "handler var",
            "N",
            "",
            "on go { var @: int = a\nf = half(N) }",
            1,
        ),
        (
            "for variable",
            "N",
            "",
            "on go { for @ in 0..k { f = f + half(N) } }",
            3,
        ),
        (
            "match binding",
            "N",
            "enum S { A(int), B }\nvar sv: S = S.B\n",
            "on go { sv = S.A(a)\nmatch sv { A(@) => { f = half(N) }, B => { f = 0.0 } } }",
            1,
        ),
        (
            "record destructuring",
            "N",
            "",
            "on go { let { N: @ } = { N: a }\nf = half(N) }",
            1,
        ),
        (
            "tuple destructuring",
            "N",
            "",
            "on go { let (@, q) = (a, 1)\nf = half(N) }",
            1,
        ),
    ];
    for (form, name, decls, body, input) in rows {
        for (binding, shadows) in [(*name, true), ("Q", false)] {
            let src = format!(
                "{head}{}{}",
                decls.replace('@', binding),
                body.replace('@', binding)
            );
            for fold in [false, true] {
                let compiled = super::declaring_scope::compiled(&src, &[], fold)
                    .unwrap_or_else(|codes| panic!("{form} fold {fold}: {codes:?}\n{src}"));
                let (saved, _) = read_save(&src, &[], fold);
                if shadows {
                    assert!(
                        !saved.iter().any(from_77),
                        "{form} fold {fold}: the local read as the constant\n{src}\n{saved:?}"
                    );
                    assert!(
                        input_is_wired(compiled.module(), *input),
                        "{form} fold {fold}: the local's input is unwired\n{src}"
                    );
                } else {
                    assert!(
                        saved.iter().any(from_77),
                        "{form} fold {fold}: the control lost the constant\n{src}\n{saved:?}"
                    );
                }
            }
        }
    }

    // A mod imported by name whose parameter shadows its own module's `let`.
    for (binding, shadows) in [("W", true), ("Q", false)] {
        let lib = format!("let W = 77\nmod scl({binding}: float) -> float {{ return W * 2.0 }}\n");
        let src = "import { scl } from \"clib\"\nin go: exec\nin fa: float\nvar f: float = 0.0\nout of: float = f\n\
                   on go { f = scl(fa) }";
        for fold in [false, true] {
            let r = compile_multi(src, &[("clib", &lib)]);
            assert_no_errors(&r);
            let (saved, _) = read_save(src, &[("clib", &lib)], fold);
            if shadows {
                assert!(
                    !saved.iter().any(from_77),
                    "imported parameter fold {fold}: read as the let\n{saved:?}"
                );
                assert!(
                    input_is_wired(&r.module, 1),
                    "imported parameter: the input is unwired"
                );
            } else {
                assert!(
                    saved.iter().any(from_77),
                    "imported control fold {fold}: the let is missing\n{saved:?}"
                );
            }
        }
    }

    // A constant local shadowing the constant is that local's constant.
    let src = format!("{head}on go {{ const N = 5\nf = half(N) }}");
    for fold in [false, true] {
        let (saved, _) = read_save(&src, &[], fold);
        assert!(
            !saved.iter().any(from_77),
            "inner const fold {fold}: {saved:?}"
        );
        assert!(
            saved
                .iter()
                .any(|l| matches!(l, Literal::Float(f) if *f == 5.0 || *f == 2.5)),
            "inner const fold {fold}: {saved:?}"
        );
    }
}

/// An integer constant expression whose operation the certified fold table
/// has no result for (an overflow, a division of mixed signs with a
/// remainder) is not baked into a `float` slot: its gate stays, holding the
/// operands. A division by zero is 0, as the gate gives.
#[test]
fn an_integer_expression_without_a_certified_result_keeps_its_gate() {
    let head = "in go: exec\nvar x: float = 0.0\nout ox: float = x\nmod half(v: float) -> float { return v / 2 }\n\
                const BIG = 9223372036854775807\nconst B2 = BIG + 1\nconst M7 = -7\nconst Z = 0\n\
                const mod plus1(v: int) -> int { return v + 1 }\n";
    let wrapped = |l: &Literal| matches!(l, Literal::Float(f) if *f == -9223372036854775808.0 || *f == -4611686018427387904.0);
    for (body, baked, kept) in [
        (
            "on go { x = half(BIG + 1) }",
            wrapped as fn(&Literal) -> bool,
            Literal::Int(i64::MAX),
        ),
        ("on go { x = BIG + 1 }", wrapped, Literal::Int(i64::MAX)),
        (
            "on go { let q: float = BIG * 2\nx = q }",
            wrapped,
            Literal::Int(i64::MAX),
        ),
        ("on go { x = half(B2) }", wrapped, Literal::Int(i64::MAX)),
        ("on go { x = plus1(BIG) }", wrapped, Literal::Int(i64::MAX)),
        (
            "on go { x = plus1(1) + BIG }",
            wrapped,
            Literal::Int(i64::MAX),
        ),
        (
            "on go { let q = B2\nx = q }",
            wrapped,
            Literal::Int(i64::MAX),
        ),
        (
            "on go { x = half(-(BIG + 1)) }",
            wrapped,
            Literal::Int(i64::MAX),
        ),
        (
            "on go { x = half(M7 / 2) }",
            |l| matches!(l, Literal::Float(f) if *f == -3.0 || *f == -1.5),
            Literal::Int(-7),
        ),
    ] {
        let src = format!("{head}{body}");
        for fold in [false, true] {
            let compiled = super::declaring_scope::compiled(&src, &[], fold)
                .unwrap_or_else(|codes| panic!("{body} fold {fold}: {codes:?}"));
            let diagnostics = &compiled.out.diagnostics;
            crate::test_support::assert_no_code(diagnostics, "WS104");
            let saved = save_numbers(&src, fold);
            assert!(
                !saved.iter().any(baked),
                "{body} fold {fold}: baked\n{saved:?}"
            );
            assert!(
                saved.contains(&kept),
                "{body} fold {fold}: the gate's operand is gone\n{saved:?}"
            );
        }
    }
    let src = format!("{head}on go {{ x = half(M7 / Z) }}");
    for fold in [false, true] {
        let saved = save_numbers(&src, fold);
        assert!(
            !saved.contains(&Literal::Int(-7)),
            "division by zero fold {fold}: not baked\n{saved:?}"
        );
    }
}

/// Where a constant with a declared type is introduced. `{X}` is declared (or
/// is a parameter) with type `{T}` and value `{V}`; `{R}` is set to `{U}`, a
/// use of it, of type `{RT}`. A `const mod` body reads `{C}`, the use with
/// every index constant.
const INTRODUCTIONS: &[(&str, &str)] = &[
    ("const", "const X: {T} = {V}\non go { {R} = {U} }"),
    ("let", "let X: {T} = {V}\non go { {R} = {U} }"),
    ("handler let", "on go {\nlet X: {T} = {V}\n{R} = {U}\n}"),
    ("handler const", "on go {\nconst X: {T} = {V}\n{R} = {U}\n}"),
    (
        "mod const parameter",
        "mod m(X: const {T}) -> {RT} { return {U} }\non go { {R} = m({V}) }",
    ),
    (
        "chip const parameter",
        "chip c(X: const {T}) -> (r: {RT}) { on go { emit r = {U} } }\non go { {R} = c({V}) }",
    ),
    (
        "const mod parameter",
        "const mod cm(X: {T}) -> {RT} { return {C} }\nconst Y = cm({V})\non go { {R} = Y }",
    ),
    (
        "const mod result",
        "const mod cm() -> {T} { return {V} }\nconst X = cm()\non go { {R} = {U} }",
    ),
    ("var", "var X: {T} = {V}\non go { {R} = {U} }"),
    (
        "static var",
        "on go {\nstatic var X: {T} = {V}\n{R} = {U}\n}",
    ),
    ("handler var", "on go {\nvar X: {T} = {V}\n{R} = {U}\n}"),
    ("buffer", "on go {\nbuffer X: {T} = {V}\n{R} = {U}\n}"),
];

/// The shapes a declared type gives a constant: its type and value (`{N}` is
/// `float` or `int`, `@` the number), how a run-time read and a constant read
/// take the number back out, and the declarations it needs. An `int` field
/// beside the number holds 7, and a generic's type parameter is named like a
/// `string` alias, which it hides.
const SHAPES: &[(&str, &str, &str, &str, &str, &str)] = &[
    ("scalar", "{N}", "@", "X", "X", ""),
    ("scalar alias", "FA", "@", "X", "X", "type FA = {N}\n"),
    ("array", "{N}[]", "[@, @]", "X[k]", "X[1]", ""),
    (
        "map",
        "Map<string, {N}>",
        "{ \"a\": @ }",
        "X[s]",
        "X[\"a\"]",
        "",
    ),
    (
        "record",
        "Q",
        "{ f: @, g: 7 }",
        "X.f",
        "X.f",
        "type Q = { f: {N}, g: int }\n",
    ),
    (
        "nested record",
        "O",
        "{ q: { f: @, g: 7 } }",
        "X.q.f",
        "X.q.f",
        "type Q = { f: {N}, g: int }\ntype O = { q: Q }\n",
    ),
    (
        "record array field",
        "A",
        "{ fs: [@, @] }",
        "X.fs[k]",
        "X.fs[1]",
        "type A = { fs: {N}[] }\n",
    ),
    ("tuple", "({N}, int)", "(@, 7)", "X.0", "X.0", ""),
    (
        "enum payload",
        "S",
        "S.C(@)",
        "(match X { C(v) => v, E => {Z} })",
        "(match X { C(v) => v, E => {Z} })",
        "enum S { C({N}), E }\n",
    ),
    (
        "generic enum payload",
        "GE<{N}>",
        "GE.C(@)",
        "(match X { C(v) => v, E => {Z} })",
        "(match X { C(v) => v, E => {Z} })",
        "type W = string\nenum GE<W> { C(W), E }\n",
    ),
    (
        "generic record alias",
        "GQ",
        "{ f: @, g: 7 }",
        "X.f",
        "X.f",
        "type E = string\ntype GP<E> = { f: E, g: int }\ntype GQ = GP<{N}>\n",
    ),
];

/// The uses of the number: `(name, use with `@` for the read, result type)`.
const USES: &[(&str, &str, &str)] = &[
    ("divide", "(@) / 2", "{N}"),
    ("compare", "(@) > 12", "bool"),
    ("store", "@", "{N}"),
];

/// The routes the route table leaves out, each loud: (introduction, shape,
/// error codes every row of it reports, why).
const UNREACHABLE: &[(&str, &str, &[&str], &str)] = &[
    (
        "let",
        "map",
        &["WS026"],
        "a map literal initializes only a map `var` or `const`",
    ),
    (
        "handler let",
        "map",
        &["WS026"],
        "a map literal initializes only a map `var` or `const`",
    ),
    (
        "mod const parameter",
        "map",
        &["WS026"],
        "a map literal is not an argument",
    ),
    (
        "chip const parameter",
        "map",
        &["WS026"],
        "a map literal is not an argument",
    ),
    (
        "chip const parameter",
        "enum payload",
        &["WS049"],
        "an enum does not cross a chip port",
    ),
    (
        "chip const parameter",
        "generic enum payload",
        &["WS049"],
        "an enum does not cross a chip port",
    ),
    (
        "const mod parameter",
        "array",
        &["WS007"],
        "a `const mod` reading a container is an exec call",
    ),
    (
        "const mod parameter",
        "map",
        &["WS026", "WS007"],
        "both of the above",
    ),
    (
        "const mod parameter",
        "tuple",
        &["WS046"],
        "a tuple element of a parameter is not evaluated",
    ),
    (
        "const mod result",
        "map",
        &["WS026"],
        "a map literal is not a return value",
    ),
    (
        "const mod result",
        "tuple",
        &["WSP001", "WS002"],
        "`->` takes named outputs, so a tuple type does not parse there",
    ),
    (
        "buffer",
        "map",
        &["WS026"],
        "a map literal initializes only a map `var` or `const`",
    ),
    (
        "buffer",
        "record",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
    (
        "buffer",
        "nested record",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
    (
        "buffer",
        "record array field",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
    (
        "buffer",
        "tuple",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
    (
        "buffer",
        "enum payload",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
    (
        "buffer",
        "generic enum payload",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
    (
        "buffer",
        "generic record alias",
        &["WSP001"],
        "a buffer holds one wire, not a record, tuple or enum",
    ),
];

/// A constant is held in its declared type wherever it is introduced: an
/// integer declared `float` (directly, or as a field, element, entry or
/// payload) is the float it names in the save, whatever reads it and in both
/// fold modes, and an integer declared `int` stays an integer.
///
/// The default set gives each (introduction, shape, type) one row per use,
/// the numbers rotating across the uses, and a route the table pins runs
/// fold off only (its codes are type errors); the full product runs under
/// `--ignored`.
#[test]
fn a_constant_is_held_in_its_declared_type_wherever_it_is_introduced() {
    constant_held_in_its_declared_type_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_constant_is_held_in_its_declared_type_wherever_it_is_introduced_exhaustive() {
    constant_held_in_its_declared_type_matrix(true);
}

fn constant_held_in_its_declared_type_matrix(full: bool) {
    let head = "in go: exec\nin k: int\nin s: string\nconst THIRTEEN = 13\nvar x: float = 0.0\nvar n: int = 0\n\
                var b: bool = false\nout ox: float = x\nout on_: int = n\nout ob: bool = b\n";
    let mut failures = Vec::new();
    let mut rejected: Vec<(String, String)> = Vec::new();
    let mut checked = 0;
    let mut seen: Vec<(bool, &str, &str)> = Vec::new();
    for (i, (intro, template)) in INTRODUCTIONS.iter().enumerate() {
        for (j, (shape, ty, value, read, const_read, decls)) in SHAPES.iter().enumerate() {
            let pinned = UNREACHABLE
                .iter()
                .find(|(p, s, _, _)| p == intro && s == shape);
            for float in [true, false] {
                let num = if float { "float" } else { "int" };
                let zero = if float { "0.0" } else { "0" };
                let numbers: &[&str] = if float {
                    &["13", "THIRTEEN", "13.0"]
                } else {
                    &["13", "THIRTEEN"]
                };
                for (n, number) in numbers.iter().enumerate() {
                    for (u, (use_name, use_of, result_ty)) in USES.iter().enumerate() {
                        if !full && (n + i + j) % numbers.len() != u % numbers.len() {
                            continue;
                        }
                        seen.push((float, *number, *use_name));
                        let fill = |t: &str| t.replace("{N}", num).replace("{Z}", zero);
                        let result_ty = fill(result_ty);
                        let result = match result_ty.as_str() {
                            "bool" => "b",
                            "float" => "x",
                            _ => "n",
                        };
                        let src = format!(
                            "{head}{}{}",
                            fill(decls),
                            template
                                .replace("{T}", &fill(ty))
                                .replace("{V}", &value.replace('@', number))
                                .replace("{RT}", &result_ty)
                                .replace("{R}", result)
                                .replace("{U}", &use_of.replace('@', &fill(read)))
                                .replace("{C}", &use_of.replace('@', &fill(const_read)))
                        );
                        let row = format!("{intro} / {shape} / {num} {number} / {use_name}");
                        let folds: &[bool] = if !full && pinned.is_some() {
                            &[false]
                        } else {
                            &[false, true]
                        };
                        for &fold in folds {
                            checked += 1;
                            // Every row of a pinned route fails with the pinned
                            // codes, and every row of any other route compiles.
                            let saved = match (try_read_save(&src, &[], fold), pinned) {
                                (Ok(_), Some(_)) => {
                                    failures.push(format!(
                                        "{row} fold {fold}: a pinned route compiles\n{src}"
                                    ));
                                    continue;
                                }
                                (Ok((saved, _)), None) => saved,
                                (Err(codes), Some((_, _, want, _))) => {
                                    if !want.iter().all(|w| codes.iter().any(|c| c == w)) {
                                        failures.push(format!(
                                            "{row} fold {fold}: {codes:?}, pinned {want:?}\n{src}"
                                        ));
                                    }
                                    if !rejected.iter().any(|(i, s)| i == intro && s == shape) {
                                        rejected.push((intro.to_string(), shape.to_string()));
                                    }
                                    continue;
                                }
                                (Err(codes), None) => {
                                    failures.push(format!(
                                        "{row} fold {fold}: does not compile: {codes:?}\n{src}"
                                    ));
                                    continue;
                                }
                            };
                            let has = |want: &dyn Fn(&Literal) -> bool| saved.iter().any(want);
                            if has(&|l| matches!(l, Literal::Float(f) if *f == 7.0)) {
                                failures.push(format!(
                                    "{row} fold {fold}: the int field is a float\n{src}\n{saved:?}"
                                ));
                            }
                            let stored = *use_name != "compare";
                            if float {
                                if has(&|l| matches!(l, Literal::Int(13) | Literal::Int(6))) {
                                    failures.push(format!(
                                        "{row} fold {fold}: an integer\n{src}\n{saved:?}"
                                    ));
                                } else if stored
                                    && !has(
                                        &|l| matches!(l, Literal::Float(f) if *f == 13.0 || *f == 6.5),
                                    )
                                {
                                    failures.push(format!(
                                        "{row} fold {fold}: the float is missing\n{src}\n{saved:?}"
                                    ));
                                }
                            } else if has(
                                &|l| matches!(l, Literal::Float(f) if *f == 13.0 || *f == 6.5),
                            ) {
                                failures
                                    .push(format!("{row} fold {fold}: a float\n{src}\n{saved:?}"));
                            } else if stored
                                && !has(&|l| matches!(l, Literal::Int(13) | Literal::Int(6)))
                            {
                                failures.push(format!(
                                    "{row} fold {fold}: the integer is missing\n{src}\n{saved:?}"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    let got: Vec<String> = rejected.iter().map(|(i, s)| format!("{i} / {s}")).collect();
    let want: Vec<String> = UNREACHABLE
        .iter()
        .map(|(i, s, _, _)| format!("{i} / {s}"))
        .collect();
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert_eq!(got, want, "the routes that do not compile");
    let routes = INTRODUCTIONS.len() * SHAPES.len();
    let want_checked = if full {
        2 * routes * 5 * USES.len()
    } else {
        (routes - UNREACHABLE.len()) * 2 * 2 * USES.len() + UNREACHABLE.len() * 2 * USES.len()
    };
    assert_eq!(checked, want_checked, "{checked}");
    for (float, numbers) in [
        (true, &["13", "THIRTEEN", "13.0"][..]),
        (false, &["13", "THIRTEEN"][..]),
    ] {
        for number in numbers {
            for (use_name, _, _) in USES {
                assert!(
                    seen.contains(&(float, *number, *use_name)),
                    "{float} {number} {use_name} never ran"
                );
            }
        }
    }
}

/// How a number type `{N}` is written in a declaration: its spelling, and the
/// declarations it needs before and after the program (`u.F` is `lib`'s).
const NUMBER_SPELLINGS: &[(&str, &str, &str, &str)] = &[
    ("plain", "{N}", "", ""),
    ("scalar alias", "F", "type F = {N}\n", ""),
    ("alias of alias", "G", "type F = {N}\ntype G = F\n", ""),
    ("alias declared after its use", "F", "", "\ntype F = {N}\n"),
    ("namespaced alias", "u.F", "", ""),
];

/// Where storage of the spelled type `{S}` is declared with the value `{V}`,
/// and the read that takes the number back out. `{R}` is set to `{U}`, a use
/// of the read, of type `{RT}`. An `int` beside the number holds 7.
const STORAGE_INTRODUCTIONS: &[(&str, &str, &str)] = &[
    ("var", "var X: {S} = {V}\non go { {R} = {U} }", "X"),
    (
        "handler var",
        "on go {\nvar X: {S} = {V}\n{R} = {U}\n}",
        "X",
    ),
    (
        "static var",
        "on go {\nstatic var X: {S} = {V}\n{R} = {U}\n}",
        "X",
    ),
    (
        "record field",
        "type RS = { f: {S}, g: int }\nvar X: RS = { f: {V}, g: 7 }\non go { {R} = {U} }",
        "X.f",
    ),
    (
        "record field of an aliased record",
        "type RS = { f: {S}, g: int }\ntype RA = RS\nvar X: RA = { f: {V}, g: 7 }\non go { {R} = {U} }",
        "X.f",
    ),
    (
        "handler record var",
        "type RS = { f: {S}, g: int }\non go {\nvar X: RS = { f: {V}, g: 7 }\n{R} = {U}\n}",
        "X.f",
    ),
    (
        "static record var",
        "type RS = { f: {S}, g: int }\non go {\nstatic var X: RS = { f: {V}, g: 7 }\n{R} = {U}\n}",
        "X.f",
    ),
    (
        "tuple element",
        "var X: ({S}, int) = ({V}, 7)\non go { {R} = {U} }",
        "X.0",
    ),
    (
        "tuple alias element",
        "type TS = ({S}, int)\nvar X: TS = ({V}, 7)\non go { {R} = {U} }",
        "X.0",
    ),
    ("buffer", "buffer X: {S} = {V}\non go { {R} = {U} }", "X"),
    (
        "handler buffer",
        "on go {\nbuffer X: {S} = {V}\n{R} = {U}\n}",
        "X",
    ),
    (
        "ref parameter",
        "var X: {S} = {V}\nmod rd(p: *{S}) -> {RT} { return {U} }\non go { {R} = rd(X) }",
        "p",
    ),
];

/// An integer declared `float` is stored as that float wherever storage is
/// declared and however the `float` is spelled: directly, through an alias, an
/// alias of an alias, an alias declared after its use or a namespaced alias,
/// as a record field or tuple element too. The save held the integer for every
/// alias spelling and for any `buffer`, so dividing it was an integer division.
/// An `int` declared through an alias stays an integer, and so does an `int`
/// beside the number.
///
/// The default set gives each (spelling, introduction, type) one row per use,
/// the numbers rotating across the uses; the full product runs under
/// `--ignored`.
#[test]
fn a_float_declared_through_an_alias_stores_an_integer_as_that_float() {
    float_declared_through_an_alias_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_float_declared_through_an_alias_stores_an_integer_as_that_float_exhaustive() {
    float_declared_through_an_alias_matrix(true);
}

fn float_declared_through_an_alias_matrix(full: bool) {
    let head = "in go: exec\nconst THIRTEEN = 13\nvar x: float = 0.0\nvar n: int = 0\nout ox: float = x\nout on_: int = n\n";
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut seen: Vec<(bool, &str, &str)> = Vec::new();
    for (i, (spelling, spelled, before, after)) in NUMBER_SPELLINGS.iter().enumerate() {
        for (j, (intro, template, read)) in STORAGE_INTRODUCTIONS.iter().enumerate() {
            for float in [true, false] {
                let num = if float { "float" } else { "int" };
                let numbers: &[&str] = if float {
                    &["13", "THIRTEEN", "13.0"]
                } else {
                    &["13", "THIRTEEN"]
                };
                let fill = |t: &str| t.replace("{N}", num);
                let (import, deps) = if spelled.starts_with("u.") {
                    (
                        "import * as u from \"lib\"\n",
                        vec![("lib", fill("type F = {N}\n"))],
                    )
                } else {
                    ("", Vec::new())
                };
                let deps: Vec<(&str, &str)> = deps.iter().map(|(k, v)| (*k, v.as_str())).collect();
                for (n, number) in numbers.iter().enumerate() {
                    for (u, (use_name, use_of)) in [("divide", "(@) / 2"), ("store", "@")]
                        .into_iter()
                        .enumerate()
                    {
                        if !full && (n + i + j) % numbers.len() != u % numbers.len() {
                            continue;
                        }
                        seen.push((float, *number, use_name));
                        let result = if float { "x" } else { "n" };
                        let src = format!(
                            "{import}{head}{}{}{}",
                            fill(before),
                            template
                                .replace("{S}", &fill(spelled))
                                .replace("{V}", number)
                                .replace("{RT}", num)
                                .replace("{R}", result)
                                .replace("{U}", &use_of.replace('@', read)),
                            fill(after)
                        );
                        let row = format!("{spelling} / {intro} / {num} {number} / {use_name}");
                        // A top-level declaration reads only the aliases
                        // declared above it.
                        let unknown =
                            !after.is_empty() && template.find("{S}") < template.find("on go");
                        for fold in [false, true] {
                            checked += 1;
                            let saved = match (try_read_save(&src, &deps, fold), unknown) {
                                (Ok((saved, _)), false) => saved,
                                (Err(codes), true) if codes.iter().any(|c| c == "WS002") => {
                                    continue;
                                }
                                (result, _) => {
                                    let codes = result.err();
                                    failures.push(format!("{row} fold {fold}: {codes:?}\n{src}"));
                                    continue;
                                }
                            };
                            let has = |want: &dyn Fn(&Literal) -> bool| saved.iter().any(want);
                            let int_of =
                                |l: &Literal| matches!(l, Literal::Int(13) | Literal::Int(6));
                            let float_of = |l: &Literal| matches!(l, Literal::Float(f) if *f == 13.0 || *f == 6.5);
                            if float && (has(&int_of) || !has(&float_of)) {
                                failures.push(format!(
                                    "{row} fold {fold}: not stored as a float\n{src}\n{saved:?}"
                                ));
                            }
                            if !float && (has(&float_of) || !has(&int_of)) {
                                failures.push(format!(
                                    "{row} fold {fold}: not stored as an integer\n{src}\n{saved:?}"
                                ));
                            }
                            if has(&|l| matches!(l, Literal::Float(f) if *f == 7.0)) {
                                failures.push(format!("{row} fold {fold}: the int beside it is a float\n{src}\n{saved:?}"));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    let rows_per_route = if full { 5 * 2 } else { 2 * 2 };
    assert_eq!(
        checked,
        NUMBER_SPELLINGS.len() * STORAGE_INTRODUCTIONS.len() * rows_per_route * 2,
        "{checked}"
    );
    for (float, numbers) in [
        (true, &["13", "THIRTEEN", "13.0"][..]),
        (false, &["13", "THIRTEEN"][..]),
    ] {
        for number in numbers {
            for use_name in ["divide", "store"] {
                assert!(
                    seen.contains(&(float, *number, use_name)),
                    "{float} {number} {use_name} never ran"
                );
            }
        }
    }
}

/// A `const mod`'s type parameter is the type the call binds it to (from the
/// arguments, an integer and a float joining to `float`, or written as
/// `f<float>(5)`), in its parameters, its result and an annotated `let` in its
/// body, whatever an alias, enum or record named like it declares. A type
/// parameter named like a `float` alias held an integer argument as a float,
/// and a call binding `float` kept an integer argument an integer.
///
/// The default set runs every call under no collision and under the `float`
/// alias, and each other collision under half the calls (each half binds
/// both types through all three mods); the full product runs under
/// `--ignored`.
#[test]
fn a_const_mod_type_parameter_is_the_type_the_call_binds() {
    const_mod_type_parameter_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_const_mod_type_parameter_is_the_type_the_call_binds_exhaustive() {
    const_mod_type_parameter_matrix(true);
}

fn const_mod_type_parameter_matrix(full: bool) {
    let head =
        "in go: exec\nvar x: float = 0.0\nvar n: int = 0\nout ox: float = x\nout on_: int = n\n";
    let collisions = [
        ("nothing", ""),
        ("a float alias", "type T = float\n"),
        ("an int alias", "type T = int\n"),
        ("an enum", "enum T { A, B }\n"),
        ("a record", "type T = { a: float }\n"),
    ];
    let id = "const mod f<T>(a: T) -> T { return a }\n";
    let pick =
        "const mod pick<T: int | float>(c: bool, a: T, b: T) -> T { return if c then a else b }\n";
    let body_let = "const mod g<T: int | float>(a: T) -> T { let y: T = 5\nreturn y + a }\n";
    // (call, declarations, whether the call binds `float`)
    let calls = [
        ("f(5)", id, false),
        ("f(5.0)", id, true),
        ("f<float>(5)", id, true),
        ("f<int>(5)", id, false),
        ("pick(true, 5, 2.5)", pick, true),
        ("pick(true, 5, 2)", pick, false),
        ("g(0)", body_let, false),
        ("g(0.0)", body_let, true),
    ];
    let mut failures = Vec::new();
    for (ci, (collision, decl)) in collisions.into_iter().enumerate() {
        for (ki, (call, mods, float)) in calls.into_iter().enumerate() {
            if !full && ci >= 2 && (ki + ci) % 2 != 0 {
                continue;
            }
            let program = |p: &str| {
                let mods = mods
                    .replace("<T", &format!("<{p}"))
                    .replace(": T", &format!(": {p}"));
                let mods = mods.replace("-> T", &format!("-> {p}"));
                format!("{head}{decl}{mods}const K = {call}\non go {{ x = K / 2\nn = K / 2 }}\n")
            };
            let (src, twin) = (program("T"), program("U"));
            let row = format!("{collision} / {call}");
            for fold in [false, true] {
                let (Ok((saved, _)), Ok((twin_saved, _))) = (
                    try_read_save(&src, &[], fold),
                    try_read_save(&twin, &[], fold),
                ) else {
                    failures.push(format!("{row} fold {fold}: does not compile\n{src}"));
                    continue;
                };
                let has = |want: &dyn Fn(&Literal) -> bool| saved.iter().any(want);
                let five_float =
                    |l: &Literal| matches!(l, Literal::Float(f) if *f == 5.0 || *f == 2.5);
                if float && (has(&|l| *l == Literal::Int(5)) || !has(&five_float)) {
                    failures.push(format!(
                        "{row} fold {fold}: not the float 5\n{src}\n{saved:?}"
                    ));
                }
                if !float && has(&five_float) {
                    failures.push(format!(
                        "{row} fold {fold}: not the integer 5\n{src}\n{saved:?}"
                    ));
                }
                let spelled = |ls: &[Literal]| {
                    let mut v: Vec<String> = ls.iter().map(|l| format!("{l:?}")).collect();
                    v.sort();
                    v
                };
                if spelled(&saved) != spelled(&twin_saved) {
                    failures.push(format!(
                        "{row} fold {fold}: differs from the renamed twin\n{src}"
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// A generic enum value built where its instantiation is declared (an
/// annotated `const`, `let` or `var`, an assignment, a `mod` parameter, a
/// return value, through an alias of the instantiation too) holds its payload
/// in the instantiated type: `Opt.Some(7)` declared `Opt<float>` holds the
/// float 7, with no warning, and declared `Opt<int>` the integer. It held the
/// integer with a `WS016` warning, and the `var`, assignment and parameter
/// forms were `WS003`.
///
/// The default set gives each (form, constructor, spelling, type) one of
/// the numbers, rotating; the full product runs under `--ignored`.
#[test]
fn a_generic_enum_payload_is_held_in_its_instantiated_type() {
    generic_enum_payload_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_generic_enum_payload_is_held_in_its_instantiated_type_exhaustive() {
    generic_enum_payload_matrix(true);
}

fn generic_enum_payload_matrix(full: bool) {
    let head = "in go: exec\nconst SEVEN = 7\nvar x: float = 0.0\nvar n: int = 0\nout ox: float = x\nout on_: int = n\n\
                enum Opt<T> { Some(T), None }\nenum Box<T> { B { w: T }, N }\ntype OF = Opt<{N}>\n";
    let read = "(match K { Some(v) => v, None => {Z} }) / 2";
    let forms = [
        ("const", "const K: {E} = {C}\non go { {R} = {U} }"),
        ("handler let", "on go {\nlet K: {E} = {C}\n{R} = {U}\n}"),
        ("var", "var K: {E} = {C}\non go { {R} = {U} }"),
        (
            "assignment",
            "var K: {E} = {NONE}\non go {\nK = {C}\n{R} = {U}\n}",
        ),
        (
            "mod const parameter",
            "mod m(K: const {E}) -> {N} { return {U} }\non go { {R} = m({C}) }",
        ),
        (
            "mod parameter",
            "mod m(K: {E}) -> {N} { return {U} }\non go { {R} = m({C}) }",
        ),
        (
            "return value",
            "mod mk() -> {E} { return {C} }\non go {\nlet K = mk()\n{R} = {U}\n}",
        ),
    ];
    let mut failures = Vec::new();
    let mut seen: Vec<(bool, &str)> = Vec::new();
    for (fi, (form, template)) in forms.into_iter().enumerate() {
        for (named, ctor) in [(false, "Opt.Some(@)"), (true, "Box.B { w: @ }")] {
            for (si, spelling) in ["Opt<{N}>", "OF"].into_iter().enumerate() {
                if named && spelling == "OF" {
                    continue;
                }
                for float in [true, false] {
                    let num = if float { "float" } else { "int" };
                    let zero = if float { "0.0" } else { "0" };
                    let numbers: &[&str] = if float {
                        &["7", "SEVEN", "7.0"]
                    } else {
                        &["7", "SEVEN"]
                    };
                    let combo = usize::from(named) * 2 + si;
                    for (n, number) in numbers.iter().enumerate() {
                        if !full && n != (fi + combo) % numbers.len() {
                            continue;
                        }
                        seen.push((float, *number));
                        let ty = if named { "Box<{N}>" } else { spelling };
                        let use_of = if named {
                            read.replace("Some(v) => v, None", "B { w } => w, N")
                        } else {
                            read.to_string()
                        };
                        let fill = |t: &str| t.replace("{N}", num).replace("{Z}", zero);
                        let src = fill(&format!(
                            "{head}{}",
                            template
                                .replace("{E}", ty)
                                .replace("{C}", &ctor.replace('@', number))
                                .replace("{NONE}", if named { "Box.N" } else { "Opt.None" })
                                .replace("{R}", if float { "x" } else { "n" })
                                .replace("{U}", &use_of)
                        ));
                        let row = format!("{form} / {ctor} / {spelling} / {num} {number}");
                        for fold in [false, true] {
                            let codes = match super::declaring_scope::compiled(&src, &[], fold) {
                                Ok(c) => c
                                    .out
                                    .diagnostics
                                    .iter()
                                    .map(|d| d.code.to_string())
                                    .collect(),
                                Err(codes) => codes,
                            };
                            if codes.iter().any(|c| c == "WS016" || c == "WS003") {
                                failures.push(format!("{row} fold {fold}: {codes:?}\n{src}"));
                                continue;
                            }
                            let Ok((saved, _)) = try_read_save(&src, &[], fold) else {
                                failures
                                    .push(format!("{row} fold {fold}: does not compile\n{src}"));
                                continue;
                            };
                            let has = |want: &dyn Fn(&Literal) -> bool| saved.iter().any(want);
                            let int_of =
                                |l: &Literal| matches!(l, Literal::Int(7) | Literal::Int(3));
                            let float_of = |l: &Literal| matches!(l, Literal::Float(f) if *f == 7.0 || *f == 3.5);
                            if float && (has(&int_of) || !has(&float_of)) {
                                failures.push(format!(
                                    "{row} fold {fold}: not the float 7\n{src}\n{saved:?}"
                                ));
                            }
                            if !float && (has(&float_of) || !has(&int_of)) {
                                failures.push(format!(
                                    "{row} fold {fold}: not the integer 7\n{src}\n{saved:?}"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    for (float, number) in [
        (true, "7"),
        (true, "SEVEN"),
        (true, "7.0"),
        (false, "7"),
        (false, "SEVEN"),
    ] {
        assert!(
            seen.contains(&(float, number)),
            "{float} {number} never ran"
        );
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// A constant for a gate setting the gate stores as an integer, a float or a
/// bool (a tick count, a duration, a font size, a flag, an enum setting) is
/// stored as that kind, converted as a wire converts it (a float truncates).
/// The float tick count failed at emit with no code (`unsupported conversion
/// from i32 to f64`), as did every other pair.
#[test]
fn a_constant_gate_setting_is_stored_as_the_kind_the_gate_holds() {
    let head = "in go: exec\nin c: controller\nin s: string\nvar n: int = 0\nvar b: bool = false\nout on_: int = n\nout ob: bool = b\n";
    let rows: &[(&str, &str, Option<Literal>)] = &[
        (
            "const D: float = 2\non go { await SleepTicks(_, delay = D)\nn = 1 }",
            "float constant tick count",
            Some(Literal::Int(2)),
        ),
        (
            "type F = float\nconst D: F = 2\non go { await SleepTicks(_, delay = D)\nn = 1 }",
            "alias",
            Some(Literal::Int(2)),
        ),
        (
            "on go { await SleepTicks(_, delay = 2.5)\nn = 1 }",
            "float literal tick count",
            Some(Literal::Int(2)),
        ),
        (
            "const D = 2.0\non go { await SleepTicks(_, delay = D)\nn = 1 }",
            "float constant",
            Some(Literal::Int(2)),
        ),
        (
            "const D = 2\non go { await Sleep(_, delay = D)\nn = 1 }",
            "int seconds",
            Some(Literal::Float(2.0)),
        ),
        (
            "on go { c.DisplayText(\"hi\", fontSize = 40.5) }",
            "float font size",
            Some(Literal::Int(40)),
        ),
        (
            "on go { c.DisplayText(\"hi\", fontSize = true) }",
            "bool font size",
            Some(Literal::Int(1)),
        ),
        (
            "on go { c.DisplayText(\"hi\", outlineSize = 3.5) }",
            "float outline",
            Some(Literal::Int(3)),
        ),
        (
            "on go { c.DisplayText(\"hi\", angle = true) }",
            "bool angle",
            Some(Literal::Float(1.0)),
        ),
        (
            "on go { c.DisplayText(\"hi\", justify = 1.0) }",
            "float enum setting",
            None,
        ),
        (
            "on go { b = s.Contains(\"b\", caseSensitive = 1.5) }",
            "float flag",
            None,
        ),
        (
            "out o: bool = Edge(true)",
            "bool into a float field",
            Some(Literal::Float(1.0)),
        ),
    ];
    for (body, what, want) in rows {
        let src = format!("{head}{body}\n");
        for fold in [false, true] {
            let saved = match read_save_values(&src, &[], fold, true) {
                Ok((saved, _)) => saved,
                Err(codes) => panic!("{what} fold {fold}: {codes:?}\n{src}"),
            };
            if let Some(want) = want {
                assert!(
                    saved.contains(want),
                    "{what} fold {fold}: {want:?} is not in the save\n{saved:?}"
                );
            }
        }
    }
}
