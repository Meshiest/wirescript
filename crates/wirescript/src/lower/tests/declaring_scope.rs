//! Each name a type is written with is read where it is written: an alias body
//! at the top level of the file declaring it (none of a generic body's type
//! parameters, none of an importer's aliases), a type parameter only inside
//! the declaration that declares it. The rows compare a program whose spelling
//! collides against a twin that renames the colliding name, at the save.
//! Also the declared types the binding rules read through an alias (a ref, an
//! array or a map), a generic enum's record payload in storage, the expected
//! instantiation of an `if`/`match`/argument join, and the constants a gate
//! setting's field stores.

use crate::ir::Literal;

/// Every component of the compiled save, one canonical line each (its struct
/// and its fields, sorted), or the codes of the errors when `src` does not
/// compile. `deps` are the files `src` imports, as `(name, source)`.
pub(super) fn save_dump(src: &str, deps: &[(&str, &str)], fold: bool) -> Result<Vec<String>, Vec<String>> {
    let out = compile_main(src, deps, fold)?;
    let mut lines: Vec<String> =
        wirescript_ir::readback::Save::read(&out.brz).components().iter().map(|c| c.to_string()).collect();
    lines.sort();
    Ok(lines)
}

/// Every 64-bit number a component of the save holds (what a wire value
/// stores), or with `narrow` every number and bool of every width (what a
/// gate's own setting fields store too).
pub(super) fn save_values(src: &str, deps: &[(&str, &str)], fold: bool, narrow: bool) -> Result<Vec<Literal>, Vec<String>> {
    values_in_save(src, deps, fold, narrow, None)
}

/// Every number and bool of every width the property `field` of a component
/// of the save holds (a gate setting's own field, by its name in the save).
pub(super) fn save_field(src: &str, deps: &[(&str, &str)], fold: bool, field: &str) -> Result<Vec<Literal>, Vec<String>> {
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
            BrdbValue::Array(items) | BrdbValue::FlatArray(items) => items.iter().for_each(|i| go(i, out, narrow)),
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
    let out = compile_main(src, deps, fold)?;
    let mut values = Vec::new();
    for c in wirescript_ir::readback::Save::read(&out.brz).components() {
        match field {
            Some(name) => c.get(name).into_iter().for_each(|p| go(p, &mut values, narrow)),
            None => c.properties.values().for_each(|p| go(p, &mut values, narrow)),
        }
    }
    Ok(values)
}

pub(super) fn compile_main(src: &str, deps: &[(&str, &str)], fold: bool) -> Result<crate::CompileResult, Vec<String>> {
    let fold_mode = if fold { crate::FoldMode::ForceOn } else { crate::FoldMode::ForceOff };
    let loader = crate::resolve::MemLoader {
        files: deps.iter().map(|(k, v)| (format!("{k}.ws"), v.to_string())).collect(),
    };
    let input = crate::CompileInput { source: src, file: "main", module_name: None, fold_mode };
    match crate::compile_with_loader(input, Default::default(), &loader) {
        Ok(out) => Ok(out),
        Err(crate::CompileError::HasErrors(errors)) => Err(errors.iter().map(|d| d.code.to_string()).collect()),
        Err(e) => panic!("{e:?}\n{src}"),
    }
}

/// The warnings `src` compiles with, by code.
fn warnings(src: &str, deps: &[(&str, &str)]) -> Vec<(String, String)> {
    compile_main(src, deps, true)
        .unwrap_or_else(|codes| panic!("{codes:?}\n{src}"))
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Warning)
        .map(|d| (d.code.to_string(), d.message.clone()))
        .collect()
}

/// Compare `a` against `b` at the save, both folds; each failure is pushed.
fn same_save(row: &str, a: &str, b: &str, deps_a: &[(&str, &str)], deps_b: &[(&str, &str)], failures: &mut Vec<String>) {
    for fold in [true, false] {
        match (save_dump(a, deps_a, fold), save_dump(b, deps_b, fold)) {
            (Ok(x), Ok(y)) if x == y => {}
            (Ok(x), Ok(y)) => {
                let only_a: Vec<&String> = x.iter().filter(|l| !y.contains(l)).take(3).collect();
                let only_b: Vec<&String> = y.iter().filter(|l| !x.contains(l)).take(3).collect();
                failures.push(format!("{row} fold {fold}: saves differ\n{a}\n--- twin\n{b}\nonly a: {only_a:#?}\nonly b: {only_b:#?}"));
            }
            (x, y) => failures.push(format!("{row} fold {fold}: {:?} / {:?}\n{a}\n--- twin\n{b}", x.err(), y.err())),
        }
    }
}

fn report(failures: Vec<String>, rows: usize) {
    assert!(rows > 0, "no rows ran");
    assert!(failures.is_empty(), "{} of {rows} rows failed:\n{}", failures.len(), failures.join("\n\n"));
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
    report(failures, GENERIC_BODY_ROWS.len());
}

/// The alias body of the first row stores the float its declaration names.
#[test]
fn an_alias_of_a_float_alias_stores_a_float_in_a_generic_body() {
    let src = format!("{HEAD}{}\n", GENERIC_BODY_ROWS[0].1.replace("{A}", "T"));
    for fold in [true, false] {
        let values = save_values(&src, &[], fold, false).unwrap();
        assert!(values.contains(&Literal::Float(5.0)), "fold {fold}: {values:?}");
        assert!(!values.contains(&Literal::Int(5)), "fold {fold}: {values:?}");
    }
}

/// A library's body (spelled `{B}` with its alias `F`), and how `main` calls
/// into it.
const LIBRARY_BODIES: &[(&str, &str, &str)] = &[
    ("static var", "type F = int\nmod lh() -> int { static var t: F = 5\n return t / 2 }\n", "lh()"),
    ("handler var", "type F = int\nmod lh() -> int { var t: F = 5\n return t / 2 }\n", "lh()"),
    ("let", "type F = int\nmod lh() -> int { let t: F = 5\n return t / 2 }\n", "lh()"),
    ("record alias", "type F = int\ntype R = { v: F }\nmod lh() -> int { static var t: R = { v: 5 }\n return t.v / 2 }\n", "lh()"),
    ("generic mod", "type F = int\nmod lh<T>(a: T) -> int { static var t: F = 5\n return t / 2 }\n", "lh(1)"),
    ("generic alias", "type F = int\ntype P<E> = { v: E, w: F }\nmod lh() -> int { static var t: P<float> = { v: 1, w: 5 }\n return t.w / 2 }\n", "lh()"),
    ("chip", "type F = int\nchip lh() -> (r: int) { static var t: F = 5\n out r = t / 2 }\n", "lh()"),
];

/// How `main` imports the library, the call prefix that takes, and the
/// declaration of `{M}`, `main`'s own alias.
const IMPORTS: &[(&str, &str, &str)] = &[
    ("named", "import { lh } from \"lib\"\n", ""),
    ("namespace", "import * as u from \"lib\"\n", "u."),
];

#[test]
fn a_library_reads_its_own_aliases_whatever_the_importer_declares() {
    let mut failures = Vec::new();
    let mut rows = 0;
    for (body_row, lib, call) in LIBRARY_BODIES {
        for (import_row, import, prefix) in IMPORTS {
            rows += 1;
            let row = format!("{body_row} / {import_row}");
            let main = |alias: &str| {
                format!("{import}type {alias} = float\nin go: exec\nvar n: float = 0.0\nvar v: {alias} = 1\non go {{ n = {prefix}{call} + v }}\n")
            };
            let deps = [("lib", *lib)];
            same_save(&row, &main("F"), &main("G"), &deps, &deps, &mut failures);
            for fold in [true, false] {
                match save_values(&main("F"), &deps, fold, false) {
                    Ok(values)
                        if (values.contains(&Literal::Int(5)) || values.contains(&Literal::Int(2)))
                            && !values.contains(&Literal::Float(5.0))
                            && !values.contains(&Literal::Float(2.5)) => {}
                    other => failures.push(format!("{row} fold {fold}: the library's `F` is not an int: {other:?}")),
                }
            }
        }
    }
    report(failures, rows);
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
    same_save("library signature", main, main, &[("lib", &collide)], &[("lib", &twin)], &mut failures);
    report(failures, 1);
}

#[test]
fn a_whole_import_of_a_colliding_alias_is_a_duplicate_and_its_twin_reads_the_library() {
    let lib = LIBRARY_BODIES[0].1;
    let main = |alias: &str| {
        format!("import \"lib\"\ntype {alias} = float\nin go: exec\nvar n: float = 0.0\nvar v: {alias} = 1\non go {{ n = lh() + v }}\n")
    };
    let deps = [("lib", lib)];
    assert_eq!(save_dump(&main("F"), &deps, true).err(), Some(vec!["WS013".to_string()]));
    let values = save_values(&main("G"), &deps, true, false).unwrap();
    assert!(values.contains(&Literal::Int(5)), "{values:?}");
}

#[test]
fn a_library_cannot_name_an_alias_only_its_importer_declares() {
    let lib = "mod lh() -> int { static var t: G = 5\n return t / 2 }\n";
    for (import_row, import, prefix) in IMPORTS.iter().chain([("whole", "import \"lib\"\n", "")].iter()) {
        let main = format!("{import}type G = int\nin go: exec\nvar n: float = 0.0\non go {{ n = {prefix}lh() }}\n");
        let codes = save_dump(&main, &[("lib", lib)], true).err().unwrap_or_default();
        assert!(codes.contains(&"WS002".to_string()), "{import_row}: {codes:?}");
    }
}

/// A library's own alias naming no type is reported where the library uses
/// it, as it was when the library's body read its aliases through the
/// importer's.
#[test]
fn a_library_alias_naming_no_type_is_reported_where_it_is_used() {
    for (lib, body) in [
        ("type F = Nope
", "mod lh() -> int { static var t: F = 5
 return t }
"),
        ("type F = Nope
type H = { v: F }
", "mod lh() -> int { static var t: H = { v: 5 }
 return t.v }
"),
    ] {
        let lib = format!("{lib}{body}");
        for (import_row, import, prefix) in IMPORTS {
            let main = format!("{import}in go: exec
var n: float = 0.0
on go {{ n = {prefix}lh() }}
");
            let codes = save_dump(&main, &[("lib", &lib)], true).err().unwrap_or_default();
            assert!(codes.contains(&"WS002".to_string()), "{import_row}: {codes:?}
{lib}");
        }
    }
}

/// A parameter, field or binding declared through an alias of a ref, an
/// array or a map takes its value as the written type does. `{R}` is the
/// alias in the first program and the type it names in the twin; `decl` is
/// the alias declaration the first program adds.
const REF_ROWS: &[(&str, &str, &str, &str)] = &[
    ("mod param write", "type RI = *int\n", "*int", "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }"),
    ("mod param read", "type RF = *float\n", "*float", "mod rd(p: {R}) -> float { return p / 2 }\non go { x = rd(&f) }"),
    ("mod param read and write", "type RI = *int\n", "*int", "mod dbl(p: {R}) { p = p * 2 }\non go { dbl(&v)\n dbl(&v)\n n = v }"),
    ("chip param", "type RI = *int\n", "*int", "chip C(p: {R}) -> (r: int) { on go { emit r = p + 1 } }\non go { n = C(&v) }"),
    ("chip param write", "type RI = *int\n", "*int", "chip C(p: {R}) { on go { p = 9 } }\non go { C(&v)\n n = v }"),
    ("alias of an alias", "type RI0 = *int\ntype RI = RI0\n", "*int", "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }"),
    ("generic alias", "type R<T> = *T\n", "*int", "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }"),
    ("ref of a map alias", "type MI = Map<int, int>\n", "Map<int, int>", "mod put(p: *{R}) { p.set(1, 5) }\non go { put(&m)\n n = m.get(1).Value }"),
    ("map alias", "type MI = Map<int, int>\n", "Map<int, int>", "mod put(p: {R}) { p.set(1, 5) }\non go { put(m)\n n = m.get(1).Value }"),
    ("array alias", "type AF = float[]\n", "float[]", "mod add(p: {R}) { p.push(2.0) }\non go { add(arr)\n x = arr.sum() }"),
    ("record field", "type RI = *int\n", "*int", "mod setb(b: { p: {R} }) { b.p = 7 }\non go { setb({ p: &v })\n n = v }"),
    ("let annotation", "type RI = *int\n", "*int", "on go { let h: {R} = &v\n *h = 3\n n = v }"),
    ("var holding a ref", "type RI = *int\n", "*int", "on go { var h: {R} = &v\n *h = 4\n n = v }"),
    ("ref of an array", "type RA = *float[]
", "*float[]", "mod add(p: {R}) { p.push(2.0) }
on go { add(&arr)
 x = arr.sum() }"),
    ("ref of a map", "type RM = *Map<int, int>
", "*Map<int, int>", "mod put(p: {R}) { p.set(1, 5) }
on go { put(&m)
 n = m.get(1).Value }"),
    ("return", "type RI = *int
", "*int", "mod pick() -> {R} { return &v }
on go { let h = pick()
 *h = 6
 n = v }"),
    ("record of refs", "type RI = *int\n", "*int", "mod sw(r: { a: {R}, b: {R} }) { let t = r.a\n r.a = r.b\n r.b = t }\nvar w: int = 2\non go { sw({ a: &v, b: &w })\n n = v }"),
];

fn ref_row_alias(decl: &str, body: &str) -> String {
    let alias = match decl {
        d if d.starts_with("type R<T>") => "R<int>",
        d => d.rsplit("type ").next().and_then(|l| l.split(' ').next()).unwrap_or(""),
    };
    format!("{REF_HEAD}{decl}{}\n", body.replace("{R}", alias))
}

const REF_HEAD: &str = "in go: exec\nvar v: int = 5\nvar f: float = 5.0\nvar n: int = 0\nvar x: float = 0.0\n\
                        var arr: float[] = [1.0]\nvar m: Map<int, int> = {}\n";

#[test]
fn an_alias_of_a_ref_binds_as_the_ref_it_names() {
    let mut failures = Vec::new();
    for (row, decl, written, body) in REF_ROWS {
        let aliased = ref_row_alias(decl, body);
        let twin = format!("{REF_HEAD}{}\n", body.replace("{R}", written));
        same_save(row, &aliased, &twin, &[], &[], &mut failures);
    }
    report(failures, REF_ROWS.len());
}

#[test]
fn a_namespaced_alias_of_a_ref_binds_as_the_ref_it_names() {
    let lib = "type RI = *int\n";
    let body = "mod inc(p: {R}) { p = p + 1 }\non go { inc(&v)\n n = v }";
    let aliased = format!("import * as u from \"lib\"\n{REF_HEAD}{}\n", body.replace("{R}", "u.RI"));
    let twin = format!("import * as u from \"lib\"\n{REF_HEAD}{}\n", body.replace("{R}", "*int"));
    let mut failures = Vec::new();
    same_save("namespaced alias", &aliased, &twin, &[("lib", lib)], &[("lib", lib)], &mut failures);
    report(failures, 1);
}

/// The write `inc` makes through an alias of `*int` reaches `v`: the body has
/// the increment its written twin has.
#[test]
fn a_write_through_an_alias_of_a_ref_is_emitted() {
    let src = ref_row_alias(REF_ROWS[0].1, REF_ROWS[0].3);
    let module = super::compile(&src).module;
    let writes = super::count_class(&module, "BrickComponentType_Exec_Var_Increment") + super::count_class(&module, "BrickComponentType_Exec_Var_Set");
    assert!(writes >= 2, "the write through `p` is missing\n{src}");
}

/// A generic enum holding a record in storage keeps its record payload field
/// by field, as the same enum written without a type parameter does. `{E}`
/// declares the enum, `{T}` is the stored type and `{C}` builds a value.
const RECORD_PAYLOAD_ROWS: &[(&str, &str)] = &[
    ("var", "var K: {T} = {C}.Some({ a: 7, b: 7 })\non go { match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }"),
    ("handler var", "on go { var K: {T} = {C}.Some({ a: 7, b: 7 })\n match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }"),
    ("static var", "mod h() { static var K: {T} = {C}.Some({ a: 7, b: 7 })\n match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }\non go { h() }"),
    ("assignment", "var K: {T} = {C}.None\non go { K = {C}.Some({ a: 7, b: 7 })\n match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }"),
    ("if let", "var K: {T} = {C}.Some({ a: 7.5, b: 3 })\non go { if let Some(v) = K { x = v.a\n n = v.b } }"),
    ("mod parameter", "mod rd(k: {T}) { match k { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }\nvar K: {T} = {C}.Some({ a: 7, b: 7 })\non go { rd(K) }"),
    ("mod result", "mod mk(q: int) -> {T} { return {C}.Some({ a: 7.5, b: q }) }\non go { let k = mk(3)\n match k { Some(v) => { x = v.a\n n = v.b }, None => { x = 0.0 } } }"),
];

#[test]
fn a_generic_enum_stores_a_record_payload_field_by_field() {
    let mut failures = Vec::new();
    let head = "in go: exec\nvar x: float = 0.0\nvar n: int = 0\ntype P = { a: float, b: int }\n";
    for (row, body) in RECORD_PAYLOAD_ROWS {
        let generic = format!("{head}enum Opt<Q> {{ Some(Q), None }}\n{}\n", body.replace("{T}", "Opt<P>").replace("{C}", "Opt"));
        let plain = format!("{head}enum Opt {{ Some(P), None }}\n{}\n", body.replace("{T}", "Opt").replace("{C}", "Opt"));
        same_save(row, &generic, &plain, &[], &[], &mut failures);
    }
    report(failures, RECORD_PAYLOAD_ROWS.len());
}

#[test]
fn a_generic_enum_record_payload_reads_its_fields_not_color_channels() {
    let src = "in go: exec\nvar x: float = 0.0\nvar n: int = 0\ntype P = { a: float, b: int }\nenum Opt<Q> { Some(Q), None }\n\
               var K: Opt<P> = Opt.Some({ a: 7.0, b: 7 })\non go { match K { Some(v) => { x = v.a / 2\n n = v.b / 2 }, None => { x = 0.0 } } }\n";
    let module = super::compile(src).module;
    assert!(
        !module.nodes.values().any(|n| n.gate_class == "BrickComponentType_Expr_SplitColor"),
        "a record field read through color channels"
    );
    for fold in [true, false] {
        let values = save_values(src, &[], fold, false).unwrap();
        assert!(values.contains(&Literal::Float(7.0)) && values.contains(&Literal::Int(7)), "fold {fold}: {values:?}");
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
        if !warnings(&src, &[]).iter().any(|(c, m)| c == "WS106" && m.contains("2.5") && m.contains("as 2")) {
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
    report(failures, 5);
}

/// An `if`, `match`, argument or `return` whose value is checked against an
/// enum instantiation builds each branch at it. Each compiles and equals its
/// twin that writes the payload as a float.
const JOIN_ROWS: &[(&str, &str)] = &[
    ("if", "on go { var L: Opt<float> = if n > 0 then K else Opt.Some(@)\n match L { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }"),
    ("if with None", "on go { K = if n > 0 then Opt.Some(@) else Opt.None\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }"),
    ("nested if", "on go { K = if n > 0 then Opt.Some(@) else if n < 0 then Opt.None else K\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }"),
    ("match", "enum C { A, B }\nvar c: C = C.A\non go { K = match c { A => Opt.Some(@), B => Opt.None }\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }"),
    ("argument", "mod f(o: Opt<float>) -> float { match o { Some(v) => { return v / 2 }, None => { return 0.0 } } }\non go { x = f(if n > 0 then Opt.Some(@) else Opt.None) }"),
    ("return", "mod f(c: bool) -> Opt<float> { return if c then Opt.Some(@) else Opt.None }\non go { K = f(n > 0)\n match K { Some(v) => { x = v / 2 }, None => { x = 0.0 } } }"),
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
    report(failures, JOIN_ROWS.len());
}

/// A constant given to a gate setting the gate keeps in its own data: what
/// the program says about it, and the value the save holds.
enum Setting {
    /// Stored as written, with nothing to say.
    Exact(Literal),
    /// Stored as this other value, with a `WS106` naming the setting.
    Inexact(Literal),
    /// Not storable: `WS028`.
    Invalid,
}

const SETTING_HEAD: &str = "in go: exec\nin c: controller\nin s: string\nvar n: int = 0\nvar b: bool = false\n\
                            out on_: int = n\nout ob: bool = b\n";

fn setting_rows() -> Vec<(&'static str, &'static str, &'static str, Setting)> {
    use Literal::{Bool, Float, Int};
    use Setting::{Exact, Inexact, Invalid};
    vec![
        ("SleepTicks 2.0", "on go { await SleepTicks(_, delay = 2.0)\n n = 1 }", "TicksToWait", Exact(Int(2))),
        ("SleepTicks 2.7", "on go { await SleepTicks(_, delay = 2.7)\n n = 1 }", "TicksToWait", Inexact(Int(2))),
        ("SleepTicks 1.999", "on go { await SleepTicks(_, delay = 1.999)\n n = 1 }", "TicksToWait", Inexact(Int(1))),
        ("SleepTicks -1.5", "on go { await SleepTicks(_, delay = -1.5)\n n = 1 }", "TicksToWait", Inexact(Int(-1))),
        ("SleepTicks true", "on go { await SleepTicks(_, delay = true)\n n = 1 }", "TicksToWait", Inexact(Int(1))),
        ("SleepTicks 1e30", "on go { await SleepTicks(_, delay = 1e30)\n n = 1 }", "TicksToWait", Invalid),
        ("SleepTicks const 2.7", "const D: float = 2.7\non go { await SleepTicks(_, delay = D)\n n = 1 }", "TicksToWait", Inexact(Int(2))),
        ("Sleep 2", "on go { await Sleep(_, delay = 2)\n n = 1 }", "SecondsToWait", Exact(Float(2.0))),
        ("Sleep false", "on go { await Sleep(_, delay = false)\n n = 1 }", "SecondsToWait", Inexact(Float(0.0))),
        ("fontSize 40", "on go { c.DisplayText(\"hi\", fontSize = 40) }", "FontSize", Exact(Int(40))),
        ("fontSize 40.5", "on go { c.DisplayText(\"hi\", fontSize = 40.5) }", "FontSize", Inexact(Int(40))),
        ("fontSize 0.5", "on go { c.DisplayText(\"hi\", fontSize = 0.5) }", "FontSize", Inexact(Int(0))),
        ("fontSize true", "on go { c.DisplayText(\"hi\", fontSize = true) }", "FontSize", Inexact(Int(1))),
        ("fontSize 1e10", "on go { c.DisplayText(\"hi\", fontSize = 1e10) }", "FontSize", Invalid),
        ("fontSize 5000000000", "on go { c.DisplayText(\"hi\", fontSize = 5000000000) }", "FontSize", Invalid),
        ("outlineSize 3.5", "on go { c.DisplayText(\"hi\", outlineSize = 3.5) }", "OutlineSize", Inexact(Int(3))),
        ("angle true", "on go { c.DisplayText(\"hi\", angle = true) }", "Angle", Inexact(Float(1.0))),
        ("lifetime true", "on go { c.DisplayText(\"hi\", lifetime = true) }", "Lifetime", Inexact(Float(1.0))),
        ("justify 2.0", "on go { c.DisplayText(\"hi\", justify = 2.0) }", "Justification", Exact(Int(2))),
        ("justify 2.7", "on go { c.DisplayText(\"hi\", justify = 2.7) }", "Justification", Inexact(Int(2))),
        ("justify -0.5", "on go { c.DisplayText(\"hi\", justify = -0.5) }", "Justification", Inexact(Int(0))),
        ("justify false", "on go { c.DisplayText(\"hi\", justify = false) }", "Justification", Inexact(Int(0))),
        ("justify 3.0", "on go { c.DisplayText(\"hi\", justify = 3.0) }", "Justification", Invalid),
        ("justify 3.5", "on go { c.DisplayText(\"hi\", justify = 3.5) }", "Justification", Invalid),
        ("justify 7.0", "on go { c.DisplayText(\"hi\", justify = 7.0) }", "Justification", Invalid),
        ("justify 1e30", "on go { c.DisplayText(\"hi\", justify = 1e30) }", "Justification", Invalid),
        ("justify const 7.0", "const J = 7.0\non go { c.DisplayText(\"hi\", justify = J) }", "Justification", Invalid),
        ("caseSensitive 0", "on go { b = s.Contains(\"b\", caseSensitive = 0) }", "bCaseSensitive", Exact(Bool(false))),
        ("caseSensitive 0.0", "on go { b = s.Contains(\"b\", caseSensitive = 0.0) }", "bCaseSensitive", Exact(Bool(false))),
        ("miteredOutline 1", "on go { c.DisplayText(\"hi\", miteredOutline = 1) }", "bMiteredOutline", Exact(Bool(true))),
        ("miteredOutline 1.5", "on go { c.DisplayText(\"hi\", miteredOutline = 1.5) }", "bMiteredOutline", Inexact(Bool(true))),
        ("miteredOutline 0.2", "on go { c.DisplayText(\"hi\", miteredOutline = 0.2) }", "bMiteredOutline", Inexact(Bool(true))),
        ("miteredOutline 2", "on go { c.DisplayText(\"hi\", miteredOutline = 2) }", "bMiteredOutline", Inexact(Bool(true))),
        ("Edge true", "out o: bool = Edge(true)", "Input", Exact(Float(1.0))),
        ("Edge 2.5", "out o: bool = Edge(2.5)", "Input", Exact(Float(2.5))),
    ]
}

/// What each setting field the rows read holds when its setting is not
/// given, so a row whose value is that default could not tell a dropped
/// setting from a stored one.
const SETTING_DEFAULTS: &[(&str, Literal)] = &[
    ("TicksToWait", Literal::Int(0)),
    ("SecondsToWait", Literal::Float(1.0)),
    ("FontSize", Literal::Int(16)),
    ("OutlineSize", Literal::Int(2)),
    ("Angle", Literal::Float(0.0)),
    ("Lifetime", Literal::Float(5.0)),
    ("Justification", Literal::Int(1)),
    ("bCaseSensitive", Literal::Bool(true)),
    ("bMiteredOutline", Literal::Bool(false)),
    ("Input", Literal::Float(0.0)),
];

#[test]
fn a_constant_gate_setting_is_stored_exactly_or_reported() {
    let mut failures = Vec::new();
    let rows = setting_rows();
    for (row, body, field, want) in &rows {
        let src = format!("{SETTING_HEAD}{body}\n");
        if let Setting::Exact(stored) | Setting::Inexact(stored) = want {
            let default = SETTING_DEFAULTS.iter().find(|(f, _)| f == field).map(|(_, d)| d);
            assert!(default.is_some_and(|d| d != stored), "{row}: {stored:?} is the field's default");
        }
        for fold in [true, false] {
            let compiled = compile_main(&src, &[], fold);
            match (want, compiled) {
                (Setting::Invalid, Err(codes)) if codes.contains(&"WS028".to_string()) => {}
                (Setting::Invalid, other) => {
                    failures.push(format!("{row} fold {fold}: not WS028: {:?}", other.map(|o| o.diagnostics.len())))
                }
                (Setting::Exact(_) | Setting::Inexact(_), Err(codes)) => {
                    failures.push(format!("{row} fold {fold}: does not compile: {codes:?}"))
                }
                (Setting::Exact(stored) | Setting::Inexact(stored), Ok(out)) => {
                    let ws106: Vec<&String> =
                        out.diagnostics.iter().filter(|d| d.code == "WS106").map(|d| &d.message).collect();
                    match want {
                        Setting::Exact(_) if !ws106.is_empty() => {
                            failures.push(format!("{row} fold {fold}: exact, but {ws106:?}"))
                        }
                        Setting::Inexact(_) if ws106.len() != 1 => {
                            failures.push(format!("{row} fold {fold}: want one WS106, got {ws106:?}"))
                        }
                        _ => {}
                    }
                    let held = save_field(&src, &[], fold, field).unwrap();
                    if held.is_empty() || held.iter().any(|v| v != stored) {
                        failures.push(format!("{row} fold {fold}: {field} holds {held:?}, not {stored:?}"));
                    }
                }
            }
        }
    }
    report(failures, rows.len());
}

/// A setting fed by a wire is the wire's, and nothing is said about it.
#[test]
fn a_wired_gate_setting_is_not_reported() {
    for body in [
        "in kf: float\non go { await SleepTicks(_, delay = kf)\n n = 1 }",
        "in kf: float\non go { c.DisplayText(\"hi\", fontSize = kf) }",
        "in kf: float\non go { b = s.Contains(\"b\", caseSensitive = kf) }",
    ] {
        let src = format!("{SETTING_HEAD}{body}\n");
        let w = warnings(&src, &[]);
        assert!(!w.iter().any(|(c, _)| c == "WS106"), "{w:?}\n{src}");
    }
}

#[test]
fn the_unbounded_parameter_hint_names_the_one_the_receiver_comes_from() {
    let src = "var n: int = 0\nvar s: string = \"ab\"\nin go: exec\nmod id<A>(v: A) -> A { return v }\n\
               mod f<T, U>(v: T, w: U) -> int { return id(v).Length() }\non go { n = f(s, s) }\n";
    let result = crate::compile(crate::CompileInput { source: src, file: "main", module_name: None, fold_mode: crate::FoldMode::Auto });
    let Err(crate::CompileError::HasErrors(errors)) = result else { panic!("compiles") };
    assert!(errors.iter().any(|d| d.code == "WS003" && d.message.contains("<T: string>")), "{errors:?}");
    let fixed = src.replace("mod f<T, U>", "mod f<T: string, U>");
    assert!(save_dump(&fixed, &[], true).is_ok(), "the suggested bound does not compile");
    // A callee whose result is its second argument's type gives the
    // parameter that argument has.
    let src = "var n: int = 0\nvar s: string = \"ab\"\nin go: exec\nmod two<A, B>(a: A, b: B) -> B { return b }\n\
               mod f<T, U>(v: T, w: U) -> int { return two(v, w).Length() }\non go { n = f(s, s) }\n";
    let result = crate::compile(crate::CompileInput { source: src, file: "main", module_name: None, fold_mode: crate::FoldMode::Auto });
    let Err(crate::CompileError::HasErrors(errors)) = result else { panic!("compiles") };
    assert!(errors.iter().any(|d| d.code == "WS003" && d.message.contains("<U: string>")), "{errors:?}");
    let fixed = src.replace("mod f<T, U>", "mod f<T, U: string>");
    assert!(save_dump(&fixed, &[], true).is_ok(), "the suggested bound does not compile");
}

/// A setting's diagnostic quotes the value as it is usually written, names
/// the setting on every path, and calls an operand of a pure expression an
/// argument.
#[test]
fn a_setting_diagnostic_names_the_setting_and_the_value_as_written() {
    let message = |body: &str, code: &str| {
        let src = format!("{SETTING_HEAD}{body}\n");
        let diagnostics = match crate::compile(crate::CompileInput { source: &src, file: "main", module_name: None, fold_mode: crate::FoldMode::Auto }) {
            Ok(out) => out.diagnostics,
            Err(crate::CompileError::HasErrors(errors)) => errors,
            Err(e) => panic!("{e:?}"),
        };
        diagnostics.into_iter().find(|d| d.code == code).map(|d| d.message).unwrap_or_else(|| panic!("no {code}: {body}"))
    };
    let m = message("on go { c.DisplayText(\"hi\", fontSize = 1e10) }", "WS028");
    assert!(m.contains("cannot be 1e10") && !m.contains("10000000000"), "{m}");
    let m = message("on go { c.DisplayText(\"hi\", justify = 3) }", "WS028");
    assert!(m.contains("`DisplayText` setting `justify` cannot be 3"), "{m}");
    let m = message("on go { c.DisplayText(\"hi\", justify = 3.0) }", "WS028");
    assert!(m.contains("`DisplayText` setting `justify`"), "{m}");
    let m = message("var y: int = 0\nout oy: int = y\non go { y = 1e30 }", "WS028");
    assert!(m.starts_with("1e30 goes into an `int` here"), "{m}");
    let m = message("on go { n = BitCount(2.5) }", "WS106");
    assert!(m.contains("argument") && !m.contains("setting"), "{m}");
    let m = message("on go { c.DisplayText(\"hi\", fontSize = 40.5) }", "WS106");
    assert!(m.contains("`DisplayText` setting `fontSize`"), "{m}");
}

#[test]
fn a_type_named_like_a_builtin_type_is_rejected() {
    for decl in ["type float = int", "type int = float", "type string = int", "enum vector { A, B }", "type any = int"] {
        let src = format!("{decl}\nvar x: float = 1.5\nout ox: float = x\n");
        let codes = save_dump(&src, &[], true).err().unwrap_or_default();
        assert!(codes.contains(&"WS107".to_string()), "{decl}: {codes:?}");
    }
}

#[test]
fn a_buffer_initializer_mismatch_names_the_buffer() {
    let src = "in go: exec\nvar n: int = 0\non go { buffer bb: int = \"5\"\n n = bb }\n";
    let result = crate::compile(crate::CompileInput { source: src, file: "main", module_name: None, fold_mode: crate::FoldMode::Auto });
    let Err(crate::CompileError::HasErrors(errors)) = result else { panic!("compiles") };
    assert!(errors.iter().any(|d| d.code == "WS003" && d.message.contains("buffer 'bb'")), "{errors:?}");
}
