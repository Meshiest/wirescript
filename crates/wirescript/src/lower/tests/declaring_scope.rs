//! Each name a type is written with is read where it is written: an alias body
//! at the top level of the file declaring it (none of a generic body's type
//! parameters, none of an importer's aliases), a type parameter only inside
//! the declaration that declares it. The rows compare a program whose spelling
//! collides against a twin that renames the colliding name, at the save.
//! Also the declared types the binding rules read through an alias (a ref, an
//! array or a map), a generic enum's record payload in storage, and the
//! expected instantiation of an `if`/`match`/argument join.

use crate::ir::Literal;
use crate::ir::gate_class as gc;
use brdb::schema::BrdbStruct;
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

/// One compile of a `(src, deps, fold)`, and what the save reads back as,
/// read on first use.
pub(super) struct Compiled {
    pub(super) out: crate::CompileResult,
    module: crate::ir::Module,
    save: OnceCell<(Vec<BrdbStruct>, usize)>,
    dump: OnceCell<Vec<String>>,
}

impl Compiled {
    /// The module lowering made, in the run that wrote the save.
    pub(super) fn module(&self) -> &crate::ir::Module {
        &self.module
    }

    /// Every component of the save, grid by grid.
    pub(super) fn components(&self) -> &[BrdbStruct] {
        &self.read().0
    }

    /// How many grids the save holds.
    pub(super) fn grid_count(&self) -> usize {
        self.read().1
    }

    fn read(&self) -> &(Vec<BrdbStruct>, usize) {
        self.save.get_or_init(|| {
            let save = wirescript_ir::readback::Save::read(&self.out.brz);
            (save.components(), save.grid_ids().len())
        })
    }

    fn dump(&self) -> &[String] {
        self.dump.get_or_init(|| {
            let mut lines: Vec<String> = self
                .components()
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            lines.sort();
            lines
        })
    }
}

type CompileKey = (String, Vec<(String, String)>, bool);

/// Entries kept per thread before the cache starts over; each test runs on a
/// thread of its own, so this bounds one test's memory.
const COMPILED_CAP: usize = 512;

thread_local! {
    static COMPILED: RefCell<crate::collections::HashMap<CompileKey, Result<Rc<Compiled>, Vec<String>>>> =
        RefCell::new(crate::collections::HashMap::default());
}

/// `src` compiled with `deps` at `fold`, or the codes of its errors. Compiled
/// once per thread for each `(src, deps, fold)`: the save helpers below and
/// the differential tests that read one program several ways share it.
pub(super) fn compiled(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
) -> Result<Rc<Compiled>, Vec<String>> {
    let key: CompileKey = (
        src.to_string(),
        deps.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        fold,
    );
    if let Some(hit) = COMPILED.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let fold_mode = if fold {
        crate::FoldMode::ForceOn
    } else {
        crate::FoldMode::ForceOff
    };
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
        fold_mode,
    };
    let result = match crate::compile::compile_keeping_lowered(
        input,
        crate::emit::EmitOptions::default(),
        &loader,
    ) {
        (Some(module), Ok(out)) => Ok(Rc::new(Compiled {
            out,
            module,
            save: OnceCell::new(),
            dump: OnceCell::new(),
        })),
        (_, Err(crate::CompileError::HasErrors(errors))) => {
            Err(errors.iter().map(|d| d.code.clone()).collect())
        }
        (_, Err(e)) => panic!("{e:?}\n{src}"),
        (None, Ok(_)) => panic!("a compile kept no module\n{src}"),
    };
    COMPILED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= COMPILED_CAP {
            c.clear();
        }
        c.insert(key, result.clone());
    });
    result
}

/// Every component of the compiled save, one canonical line each (its struct
/// and its fields, sorted), or the codes of the errors when `src` does not
/// compile. `deps` are the files `src` imports, as `(name, source)`.
pub(super) fn save_dump(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
) -> Result<Vec<String>, Vec<String>> {
    Ok(compiled(src, deps, fold)?.dump().to_vec())
}

/// Every 64-bit number a component of the save holds (what a wire value
/// stores), or with `narrow` every number and bool of every width (what a
/// gate's own setting fields store too).
pub(super) fn save_values(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
    narrow: bool,
) -> Result<Vec<Literal>, Vec<String>> {
    values_in_save(src, deps, fold, narrow, None)
}

/// Every number and bool of every width the property `field` of a component
/// of the save holds (a gate setting's own field, by its name in the save).
pub(super) fn save_field(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
    field: &str,
) -> Result<Vec<Literal>, Vec<String>> {
    values_in_save(src, deps, fold, true, Some(field))
}

fn values_in_save(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
    narrow: bool,
    field: Option<&str>,
) -> Result<Vec<Literal>, Vec<String>> {
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
            BrdbValue::Enum(e) if narrow => out.push(Literal::Int(e.value as i64)),
            BrdbValue::Struct(s) => s.properties.values().for_each(|p| go(p, out, narrow)),
            BrdbValue::Array(items) | BrdbValue::FlatArray(items) => {
                for i in items {
                    go(i, out, narrow);
                }
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
    let out = compiled(src, deps, fold)?;
    let mut values = Vec::new();
    for c in out.components() {
        match field {
            Some(name) => c
                .get(name)
                .into_iter()
                .for_each(|p| go(p, &mut values, narrow)),
            None => c
                .properties
                .values()
                .for_each(|p| go(p, &mut values, narrow)),
        }
    }
    Ok(values)
}

/// [`compiled`], as an owned result.
pub(super) fn compile_main(
    src: &str,
    deps: &[(&str, &str)],
    fold: bool,
) -> Result<crate::CompileResult, Vec<String>> {
    let c = compiled(src, deps, fold)?;
    Ok(crate::CompileResult {
        brz: c.out.brz.clone(),
        diagnostics: c.out.diagnostics.clone(),
        placements: c.out.placements.clone(),
    })
}

/// The warnings `src` compiles with, by code.
pub(super) fn warnings(src: &str, deps: &[(&str, &str)]) -> Vec<(String, String)> {
    compiled(src, deps, true)
        .unwrap_or_else(|codes| panic!("{codes:?}\n{src}"))
        .out
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Warning)
        .map(|d| (d.code.clone(), d.message.clone()))
        .collect()
}

/// Compare `a` against `b` at the save, both folds; each failure is pushed.
fn same_save(
    row: &str,
    a: &str,
    b: &str,
    deps_a: &[(&str, &str)],
    deps_b: &[(&str, &str)],
    failures: &mut Vec<String>,
) {
    same_save_at(row, a, b, deps_a, deps_b, &[true, false], failures);
}

/// [`same_save`] at the given folds only.
fn same_save_at(
    row: &str,
    a: &str,
    b: &str,
    deps_a: &[(&str, &str)],
    deps_b: &[(&str, &str)],
    folds: &[bool],
    failures: &mut Vec<String>,
) {
    for &fold in folds {
        match (save_dump(a, deps_a, fold), save_dump(b, deps_b, fold)) {
            (Ok(x), Ok(y)) if x == y => {}
            (Ok(x), Ok(y)) => {
                let only_a: Vec<&String> = x.iter().filter(|l| !y.contains(l)).take(3).collect();
                let only_b: Vec<&String> = y.iter().filter(|l| !x.contains(l)).take(3).collect();
                failures.push(format!("{row} fold {fold}: saves differ\n{a}\n--- twin\n{b}\nonly a: {only_a:#?}\nonly b: {only_b:#?}"));
            }
            (x, y) => failures.push(format!(
                "{row} fold {fold}: {:?} / {:?}\n{a}\n--- twin\n{b}",
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

const HEAD: &str = "in go: exec\nvar x: float = 0.0\nvar n: int = 0\nvar s: string = \"\"\n";

/// Programs where a name read inside a generic body (or inside a body
/// inlined there) comes from an alias body or another declaration, spelled
/// `{A}`. The collision spells it `T`, the generic's type parameter; the twin
/// spells it `Z`. The alias body is written at the top level, where `T` is
/// the alias, so both must store the same.
const GENERIC_BODY_ROWS: &[(&str, &str)] = &[
    (
        "alias of an alias",
        "type {A} = float\ntype G = {A}\nmod g<T>(a: T) -> float { static var y: G = 5\n return y / 2 }\non go { x = g(1) }",
    ),
    (
        "alias of an alias, handler var",
        "type {A} = float\ntype G = {A}\nmod g<T>(a: T) -> float { var y: G = 5\n return y / 2 }\non go { x = g(1) }",
    ),
    (
        "string alias",
        "type {A} = string\ntype G = {A}\nmod g<T>(a: T) -> string { static var y: G = \"abc\"\n return y }\non go { s = g(1) }",
    ),
    (
        "record alias field",
        "type {A} = float\ntype W = { v: {A}, k: int }\nmod g<T>(a: T) -> float { static var y: W = { v: 5, k: 7 }\n return y.v / 2 }\non go { x = g(1) }",
    ),
    (
        "array alias",
        "type {A} = float\ntype G = {A}[]\nmod g<T>(a: T) -> float { var y: G = [5]\n return y[0] / 2 }\non go { x = g(1) }",
    ),
    (
        "enum alias",
        "enum {A} { P, Q }\ntype G = {A}\nconst QV: G = {A}.Q\nmod g<T>(a: T) -> int { var y: G = QV\n return y.ToInt() }\non go { n = g(1) }",
    ),
    (
        "generic alias body",
        "type {A} = float\ntype P<E> = { v: E, w: {A} }\nmod g<T>(a: T) -> float { static var y: P<int> = { v: 7, w: 5 }\n return y.w / 2 }\non go { x = g(1) }",
    ),
    (
        "mod inlined in a generic body",
        "type {A} = float\nmod h() -> float { static var y: {A} = 5\n return y / 2 }\nmod g<T>(a: T) -> float { return h() }\non go { x = g(1) }",
    ),
    (
        "two type parameters",
        "type {A} = float\ntype G = {A}\nmod g<U, T>(b: U, a: T) -> float { static var y: G = 5\n return y / 2 }\non go { x = g(1, \"s\") }",
    ),
    (
        "nested generic body",
        "type {A} = float\ntype G = {A}\nmod g2<T>(a: T) -> float { static var y: G = 5\n return y / 2 }\nmod g<T>(a: T) -> float { return g2(a) }\non go { x = g(1) }",
    ),
];

#[test]
fn an_alias_body_reads_its_names_where_it_is_declared() {
    let mut failures = Vec::new();
    for (row, body) in GENERIC_BODY_ROWS {
        let collide = format!("{HEAD}{}\n", body.replace("{A}", "T"));
        let twin = format!("{HEAD}{}\n", body.replace("{A}", "Z"));
        same_save(row, &collide, &twin, &[], &[], &mut failures);
    }
    report(&failures, GENERIC_BODY_ROWS.len());
}

/// The alias body of the first row stores the float its declaration names.
#[test]
fn an_alias_of_a_float_alias_stores_a_float_in_a_generic_body() {
    let src = format!("{HEAD}{}\n", GENERIC_BODY_ROWS[0].1.replace("{A}", "T"));
    for fold in [true, false] {
        let values = save_values(&src, &[], fold, false).unwrap();
        assert!(
            values.contains(&Literal::Float(5.0)),
            "fold {fold}: {values:?}"
        );
        assert!(
            !values.contains(&Literal::Int(5)),
            "fold {fold}: {values:?}"
        );
    }
}

/// A library's body (spelled `{B}` with its alias `F`), and how `main` calls
/// into it.
const LIBRARY_BODIES: &[(&str, &str, &str)] = &[
    (
        "static var",
        "type F = int\nmod lh() -> int { static var t: F = 5\n return t / 2 }\n",
        "lh()",
    ),
    (
        "handler var",
        "type F = int\nmod lh() -> int { var t: F = 5\n return t / 2 }\n",
        "lh()",
    ),
    (
        "let",
        "type F = int\nmod lh() -> int { let t: F = 5\n return t / 2 }\n",
        "lh()",
    ),
    (
        "record alias",
        "type F = int\ntype R = { v: F }\nmod lh() -> int { static var t: R = { v: 5 }\n return t.v / 2 }\n",
        "lh()",
    ),
    (
        "generic mod",
        "type F = int\nmod lh<T>(a: T) -> int { static var t: F = 5\n return t / 2 }\n",
        "lh(1)",
    ),
    (
        "generic alias",
        "type F = int\ntype P<E> = { v: E, w: F }\nmod lh() -> int { static var t: P<float> = { v: 1, w: 5 }\n return t.w / 2 }\n",
        "lh()",
    ),
    (
        "chip",
        "type F = int\nchip lh() -> (r: int) { static var t: F = 5\n out r = t / 2 }\n",
        "lh()",
    ),
];

/// How `main` imports the library, the call prefix that takes, and the
/// declaration of `{M}`, `main`'s own alias.
const IMPORTS: &[(&str, &str, &str)] = &[
    ("named", "import { lh } from \"lib\"\n", ""),
    ("namespace", "import * as u from \"lib\"\n", "u."),
];

/// The default set compares each row with its renamed twin at one fold,
/// alternating (the alias is resolved before folding), and reads the stored
/// values at both; both folds of the twin run under `--ignored`.
#[test]
fn a_library_reads_its_own_aliases_whatever_the_importer_declares() {
    library_reads_its_own_aliases_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_library_reads_its_own_aliases_whatever_the_importer_declares_exhaustive() {
    library_reads_its_own_aliases_matrix(true);
}

fn library_reads_its_own_aliases_matrix(full: bool) {
    let mut failures = Vec::new();
    let mut rows = 0;
    for (b, (body_row, lib, call)) in LIBRARY_BODIES.iter().enumerate() {
        for (m, (import_row, import, prefix)) in IMPORTS.iter().enumerate() {
            let folds = row_folds(b + m, full);
            rows += 1;
            let row = format!("{body_row} / {import_row}");
            let main = |alias: &str| {
                format!(
                    "{import}type {alias} = float\nin go: exec\nvar n: float = 0.0\nvar v: {alias} = 1\non go {{ n = {prefix}{call} + v }}\n"
                )
            };
            let deps = [("lib", *lib)];
            same_save_at(
                &row,
                &main("F"),
                &main("G"),
                &deps,
                &deps,
                folds,
                &mut failures,
            );
            for fold in [true, false] {
                match save_values(&main("F"), &deps, fold, false) {
                    Ok(values)
                        if (values.contains(&Literal::Int(5))
                            || values.contains(&Literal::Int(2)))
                            && !values.contains(&Literal::Float(5.0))
                            && !values.contains(&Literal::Float(2.5)) => {}
                    other => failures.push(format!(
                        "{row} fold {fold}: the library's `F` is not an int: {other:?}"
                    )),
                }
            }
        }
    }
    report(&failures, rows);
}

/// A library's signature names an alias whose body names another alias,
/// spelled like the generic's type parameter (`{A}`). Imported by name, the
/// signature is written out where the library declares its aliases.
#[test]
fn a_library_signature_alias_reads_its_names_where_it_is_declared() {
    let lib = "type {A} = float
type G = {A}
mod lg<T>(a: T, b: G) -> float { return b / 2 }
";
    let main = "import { lg } from \"lib\"
in go: exec
var x: float = 0.0
on go { x = lg(1, 5) }
";
    let collide = lib.replace("{A}", "T");
    let twin = lib.replace("{A}", "Z");
    let mut failures = Vec::new();
    same_save(
        "library signature",
        main,
        main,
        &[("lib", &collide)],
        &[("lib", &twin)],
        &mut failures,
    );
    report(&failures, 1);
}

#[test]
fn a_whole_import_of_a_colliding_alias_is_a_duplicate_and_its_twin_reads_the_library() {
    let lib = LIBRARY_BODIES[0].1;
    let main = |alias: &str| {
        format!(
            "import \"lib\"\ntype {alias} = float\nin go: exec\nvar n: float = 0.0\nvar v: {alias} = 1\non go {{ n = lh() + v }}\n"
        )
    };
    let deps = [("lib", lib)];
    assert_eq!(
        save_dump(&main("F"), &deps, true).err(),
        Some(vec!["WS013".to_string()])
    );
    let values = save_values(&main("G"), &deps, true, false).unwrap();
    assert!(values.contains(&Literal::Int(5)), "{values:?}");
}

#[test]
fn a_library_cannot_name_an_alias_only_its_importer_declares() {
    let lib = "mod lh() -> int { static var t: G = 5\n return t / 2 }\n";
    for (import_row, import, prefix) in IMPORTS
        .iter()
        .chain([("whole", "import \"lib\"\n", "")].iter())
    {
        let main = format!(
            "{import}type G = int\nin go: exec\nvar n: float = 0.0\non go {{ n = {prefix}lh() }}\n"
        );
        let codes = save_dump(&main, &[("lib", lib)], true)
            .err()
            .unwrap_or_default();
        assert!(
            codes.contains(&"WS002".to_string()),
            "{import_row}: {codes:?}"
        );
    }
}

/// A library's own alias naming no type is reported where the library uses it.
#[test]
fn a_library_alias_naming_no_type_is_reported_where_it_is_used() {
    for (lib, body) in [
        (
            "type F = Nope
",
            "mod lh() -> int { static var t: F = 5
 return t }
",
        ),
        (
            "type F = Nope
type H = { v: F }
",
            "mod lh() -> int { static var t: H = { v: 5 }
 return t.v }
",
        ),
    ] {
        let lib = format!("{lib}{body}");
        for (import_row, import, prefix) in IMPORTS {
            let main = format!(
                "{import}in go: exec
var n: float = 0.0
on go {{ n = {prefix}lh() }}
"
            );
            let codes = save_dump(&main, &[("lib", &lib)], true)
                .err()
                .unwrap_or_default();
            assert!(
                codes.contains(&"WS002".to_string()),
                "{import_row}: {codes:?}
{lib}"
            );
        }
    }
}

/// A parameter, field or binding declared through an alias of a ref, an
/// array or a map takes its value as the written type does. `{R}` is the
/// alias in the first program and the type it names in the twin; `decl` is
/// the alias declaration the first program adds.
const REF_ROWS: &[(&str, &str, &str, &str)] = &[
    (
        "mod param write",
        "type RI = *int\n",
        "*int",
        "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }",
    ),
    (
        "mod param read",
        "type RF = *float\n",
        "*float",
        "mod rd(p: {R}) -> float { return p / 2 }\non go { x = rd(&f) }",
    ),
    (
        "mod param read and write",
        "type RI = *int\n",
        "*int",
        "mod dbl(p: {R}) { p = p * 2 }\non go { dbl(&v)\n dbl(&v)\n n = v }",
    ),
    (
        "chip param",
        "type RI = *int\n",
        "*int",
        "chip C(p: {R}) -> (r: int) { on go { emit r = p + 1 } }\non go { n = C(&v) }",
    ),
    (
        "chip param write",
        "type RI = *int\n",
        "*int",
        "chip C(p: {R}) { on go { p = 9 } }\non go { C(&v)\n n = v }",
    ),
    (
        "alias of an alias",
        "type RI0 = *int\ntype RI = RI0\n",
        "*int",
        "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }",
    ),
    (
        "generic alias",
        "type R<T> = *T\n",
        "*int",
        "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }",
    ),
    (
        "ref of a map alias",
        "type MI = Map<int, int>\n",
        "Map<int, int>",
        "mod put(p: *{R}) { p.set(1, 5) }\non go { put(&m)\n n = m.get(1).Value }",
    ),
    (
        "map alias",
        "type MI = Map<int, int>\n",
        "Map<int, int>",
        "mod put(p: {R}) { p.set(1, 5) }\non go { put(m)\n n = m.get(1).Value }",
    ),
    (
        "array alias",
        "type AF = float[]\n",
        "float[]",
        "mod add(p: {R}) { p.push(2.0) }\non go { add(arr)\n x = arr.sum() }",
    ),
    (
        "record field",
        "type RI = *int\n",
        "*int",
        "mod setb(b: { p: {R} }) { b.p = 7 }\non go { setb({ p: &v })\n n = v }",
    ),
    (
        "let annotation",
        "type RI = *int\n",
        "*int",
        "on go { let h: {R} = &v\n *h = 3\n n = v }",
    ),
    (
        "var holding a ref",
        "type RI = *int\n",
        "*int",
        "on go { var h: {R} = &v\n *h = 4\n n = v }",
    ),
    (
        "ref of an array",
        "type RA = *float[]
",
        "*float[]",
        "mod add(p: {R}) { p.push(2.0) }
on go { add(&arr)
 x = arr.sum() }",
    ),
    (
        "ref of a map",
        "type RM = *Map<int, int>
",
        "*Map<int, int>",
        "mod put(p: {R}) { p.set(1, 5) }
on go { put(&m)
 n = m.get(1).Value }",
    ),
    (
        "return",
        "type RI = *int
",
        "*int",
        "mod pick() -> {R} { return &v }
on go { let h = pick()
 *h = 6
 n = v }",
    ),
    (
        "record of refs",
        "type RI = *int\n",
        "*int",
        "mod sw(r: { a: {R}, b: {R} }) { let t = r.a\n r.a = r.b\n r.b = t }\nvar w: int = 2\non go { sw({ a: &v, b: &w })\n n = v }",
    ),
];

fn ref_row_alias(decl: &str, body: &str) -> String {
    let alias = match decl {
        d if d.starts_with("type R<T>") => "R<int>",
        d => d
            .rsplit("type ")
            .next()
            .and_then(|l| l.split(' ').next())
            .unwrap_or(""),
    };
    format!("{REF_HEAD}{decl}{}\n", body.replace("{R}", alias))
}

const REF_HEAD: &str = "in go: exec\nvar v: int = 5\nvar f: float = 5.0\nvar n: int = 0\nvar x: float = 0.0\n\
                        var arr: float[] = [1.0]\nvar m: Map<int, int> = {}\n";

/// One fold per row in the default set, alternating: the alias is resolved
/// before folding and the row reads only equality with its twin. Both folds
/// run under `--ignored`.
#[test]
fn an_alias_of_a_ref_binds_as_the_ref_it_names() {
    alias_of_a_ref_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn an_alias_of_a_ref_binds_as_the_ref_it_names_exhaustive() {
    alias_of_a_ref_matrix(true);
}

/// The folds row `i` of a twin comparison runs at: both with `full`, else
/// one, alternating with `i`.
fn row_folds(i: usize, full: bool) -> &'static [bool] {
    match (full, i % 2) {
        (true, _) => &[true, false],
        (false, 0) => &[true],
        (false, _) => &[false],
    }
}

fn alias_of_a_ref_matrix(full: bool) {
    let mut failures = Vec::new();
    for (i, (row, decl, written, body)) in REF_ROWS.iter().enumerate() {
        let aliased = ref_row_alias(decl, body);
        let twin = format!("{REF_HEAD}{}\n", body.replace("{R}", written));
        same_save_at(
            row,
            &aliased,
            &twin,
            &[],
            &[],
            row_folds(i, full),
            &mut failures,
        );
    }
    report(&failures, REF_ROWS.len());
}

#[test]
fn a_namespaced_alias_of_a_ref_binds_as_the_ref_it_names() {
    let lib = "type RI = *int\n";
    let body = "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }";
    let aliased = format!(
        "import * as u from \"lib\"\n{REF_HEAD}{}\n",
        body.replace("{R}", "u.RI")
    );
    let twin = format!(
        "import * as u from \"lib\"\n{REF_HEAD}{}\n",
        body.replace("{R}", "*int")
    );
    let mut failures = Vec::new();
    same_save(
        "namespaced alias",
        &aliased,
        &twin,
        &[("lib", lib)],
        &[("lib", lib)],
        &mut failures,
    );
    report(&failures, 1);
}

/// The write `inc` makes through an alias of `*int` reaches `v`: the body has
/// the increment its written twin has.
#[test]
fn a_write_through_an_alias_of_a_ref_is_emitted() {
    let (_, decl, written, body) = REF_ROWS[0];
    let writes = |src: &str| {
        let module = super::compile_ok(src).module;
        (
            super::count_class(&module, gc::VAR_INCREMENT),
            super::count_class(&module, gc::VAR_SET),
        )
    };
    let src = ref_row_alias(decl, body);
    let twin = format!("{REF_HEAD}{}\n", body.replace("{R}", written));
    assert_eq!(
        writes(&src),
        (1, 1),
        "`inc`'s increment through `p` and `n = v`\n{src}"
    );
    assert_eq!(
        writes(&src),
        writes(&twin),
        "the aliased body writes as its written twin does\n{src}"
    );
}

/// A generic enum holding a record in storage keeps its record payload field
/// by field, as the same enum written without a type parameter does. `{E}`
/// declares the enum, `{T}` is the stored type and `{C}` builds a value.
const RECORD_PAYLOAD_ROWS: &[(&str, &str)] = &[
    (
        "var",
        "var K: {T} = {C}.Some({ a: 7, b: 7 })\non go { match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "handler var",
        "on go { var K: {T} = {C}.Some({ a: 7, b: 7 })\n match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "static var",
        "mod h() { static var K: {T} = {C}.Some({ a: 7, b: 7 })\n match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }\non go { h() }",
    ),
    (
        "assignment",
        "var K: {T} = {C}.None\non go { K = {C}.Some({ a: 7, b: 7 })\n match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "if let",
        "var K: {T} = {C}.Some({ a: 7.5, b: 3 })\non go { if let Some(v) = K { x = v.a\n n = v.b } }",
    ),
    (
        "mod parameter",
        "mod rd(k: {T}) { match k { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }\nvar K: {T} = {C}.Some({ a: 7, b: 7 })\non go { rd(K) }",
    ),
    (
        "mod result",
        "mod mk(q: int) -> {T} { return {C}.Some({ a: 7.5, b: q }) }\non go { let k = mk(3)\n match k { Some(v) => { x = v.a\n n = v.b }, None => { x = 0.0 } } }",
    ),
];

#[test]
fn a_generic_enum_stores_a_record_payload_field_by_field() {
    let mut failures = Vec::new();
    let head = "in go: exec\nvar x: float = 0.0\nvar n: int = 0\ntype P = { a: float, b: int }\n";
    for (row, body) in RECORD_PAYLOAD_ROWS {
        let generic = format!(
            "{head}enum Opt<Q> {{ Some(Q), None }}\n{}\n",
            body.replace("{T}", "Opt<P>").replace("{C}", "Opt")
        );
        let plain = format!(
            "{head}enum Opt {{ Some(P), None }}\n{}\n",
            body.replace("{T}", "Opt").replace("{C}", "Opt")
        );
        same_save(row, &generic, &plain, &[], &[], &mut failures);
    }
    report(&failures, RECORD_PAYLOAD_ROWS.len());
}

#[test]
fn a_generic_enum_record_payload_reads_its_fields_not_color_channels() {
    let src = "in go: exec\nvar x: float = 0.0\nvar n: int = 0\ntype P = { a: float, b: int }\nenum Opt<Q> { Some(Q), None }\n\
               var K: Opt<P> = Opt.Some({ a: 7.0, b: 7 })\non go { match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }\n";
    let module = super::compile(src).module;
    assert!(
        !module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::SPLIT_COLOR),
        "a record field read through color channels"
    );
    for fold in [true, false] {
        let values = save_values(src, &[], fold, false).unwrap();
        assert!(
            values.contains(&Literal::Float(7.0)) && values.contains(&Literal::Int(7)),
            "fold {fold}: {values:?}"
        );
    }
}

/// `Opt<int>` given `2.5` holds the integer `int` holds, in a `const` as in a
/// `var`, and says so.
#[test]
fn an_int_payload_given_a_fraction_truncates_with_a_warning_const_and_var_alike() {
    let mut failures = Vec::new();
    for intro in ["const", "var"] {
        let src = format!(
            "in go: exec\nvar x: float = 0.0\nvar n: int = 0\nenum Opt<T> {{ Some(T), None }}\n{intro} K: Opt<int> = Opt.Some(2.5)\n\
             on go {{ match K {{ Some(v) => {{ n = v }}, None => {{ n = 9 }} }} }}\n"
        );
        if !warnings(&src, &[])
            .iter()
            .any(|(c, m)| c == "WS106" && m.contains("2.5") && m.contains("as 2"))
        {
            failures.push(format!("{intro}: no WS106\n{src}"));
        }
        for fold in [true, false] {
            let values = save_values(&src, &[], fold, false).unwrap();
            if values.contains(&Literal::Float(2.5)) || !values.contains(&Literal::Int(2)) {
                failures.push(format!("{intro} fold {fold}: {values:?}\n{src}"));
            }
        }
    }
    for decl in ["const K: int = 2.5", "let K: int = 2.5", "var K: int = 2.5"] {
        let src = format!("in go: exec\nvar n: int = 0\n{decl}\non go {{ n = K }}\n");
        let values = save_values(&src, &[], true, false).unwrap();
        if values.contains(&Literal::Float(2.5)) {
            failures.push(format!("{decl}: stores 2.5\n{values:?}"));
        }
        if !warnings(&src, &[]).iter().any(|(c, _)| c == "WS106") {
            failures.push(format!("{decl}: no WS106"));
        }
    }
    report(&failures, 5);
}

/// An `if`, `match`, argument or `return` whose value is checked against an
/// enum instantiation builds each branch at it. Each compiles and equals its
/// twin that writes the payload as a float.
const JOIN_ROWS: &[(&str, &str)] = &[
    (
        "if",
        "on go { var L: Opt<float> = if n > 0 then K else Opt.Some(@)\n match L { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "if with None",
        "on go { K = if n > 0 then Opt.Some(@) else Opt.None\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "nested if",
        "on go { K = if n > 0 then Opt.Some(@) else if n < 0 then Opt.None else K\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "match",
        "enum C { A, B }\nvar c: C = C.A\non go { K = match c { A => Opt.Some(@), B => Opt.None }\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }",
    ),
    (
        "argument",
        "mod f(o: Opt<float>) -> float { match o { Some(v) => { return v / 2 }, None => { return 0.0 } } }\non go { x = f(if n > 0 then Opt.Some(@) else Opt.None) }",
    ),
    (
        "return",
        "mod f(c: bool) -> Opt<float> { return if c then Opt.Some(@) else Opt.None }\non go { K = f(n > 0)\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }",
    ),
];

#[test]
fn an_enum_join_builds_each_branch_at_the_expected_instantiation() {
    let mut failures = Vec::new();
    let head = "in go: exec\nvar x: float = 0.0\nvar n: int = 0\nenum Opt<T> { Some(T), None }\nvar K: Opt<float> = Opt.Some(7.0)\n";
    for (row, body) in JOIN_ROWS {
        let int_payload = format!("{head}{}\n", body.replace('@', "7"));
        let float_payload = format!("{head}{}\n", body.replace('@', "7.0"));
        same_save(row, &int_payload, &float_payload, &[], &[], &mut failures);
    }
    report(&failures, JOIN_ROWS.len());
}

#[test]
fn a_buffer_initializer_mismatch_names_the_buffer() {
    let src = "in go: exec\nvar n: int = 0\non go { buffer bb: int = \"5\"\n n = bb }\n";
    let result = crate::compile(crate::CompileInput {
        source: src,
        file: "main",
        module_name: None,
        fold_mode: crate::FoldMode::Auto,
    });
    let Err(crate::CompileError::HasErrors(errors)) = result else {
        panic!("compiles")
    };
    assert!(
        errors
            .iter()
            .any(|d| d.code == "WS003" && d.message.contains("buffer 'bb'")),
        "{errors:?}"
    );
}
