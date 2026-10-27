//! A name bound in a scope hides every outer constant of the same name, for
//! every reader. Each program here is written once with `@` for a binding,
//! then compiled with `@` spelled `K`, the name of a module constant, and
//! with `@` spelled `Zq`, which names nothing else: the two must lower to the
//! same IR and save the same components, with and without folding. A reader
//! that took the outer `K` for the local one shows up as a difference.
//!
//! The binding is also spelled like the other names an evaluator resolves
//! on its own: an `import * as` alias (`u`, read as `u.K`) and a bare enum
//! variant (`Blue`). `%` in a template is the shadowed name in both twins,
//! for an initializer that reads the outer name the binding hides.

use super::*;
use crate::ir::Module;

/// Everything a compile is compared by: the diagnostic codes, the IR shape
/// (every gate with its ports and properties, every wire; labels left out,
/// as they carry names), and every component the save holds.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Outcome {
    pub(super) compiled: bool,
    pub(super) codes: Vec<String>,
    pub(super) shape: Vec<String>,
    pub(super) comps: Vec<String>,
}

/// The gates and wires of `m` and its chips, as text. A literal gate is read
/// as the value it gives each consumer, which is what the save holds whether
/// one gate feeds several consumers or each holds the value itself.
/// [`module_shape`] for `root` and every chip module below it.
fn ir_shape(root: &Module, out: &mut Vec<String>) {
    root.walk(&mut |_, m| module_shape(m, out));
}

fn module_shape(m: &Module, out: &mut Vec<String>) {
    let class = |id: &crate::ir::NodeId| m.nodes.get(id).map_or("pin", |n| n.gate_class);
    let literal = |id: &crate::ir::NodeId| {
        m.nodes
            .get(id)
            .filter(|n| n.gate_class == crate::ir::gate_class::LITERAL)
            .and_then(|n| n.properties.get(&*crate::intern::sym::VALUE))
    };
    let mut fed: crate::collections::HashMap<crate::ir::NodeId, Vec<String>> = HashMap::default();
    for w in &m.wires {
        if let Some(v) = literal(&w.source.node_id) {
            fed.entry(w.target.node_id)
                .or_default()
                .push(format!("{:?}={v:?}", w.target.port));
        }
    }
    for n in m.nodes.values() {
        if n.gate_class == crate::ir::gate_class::LITERAL {
            continue;
        }
        let mut ports: Vec<String> = n
            .ports
            .inputs
            .iter()
            .chain(n.ports.outputs.iter())
            .map(|p| format!("{}:{:?}", crate::intern::resolve(p.name), p.ty))
            .collect();
        ports.sort();
        let mut props: Vec<String> = n
            .properties
            .iter()
            .map(|(k, v)| (crate::intern::resolve(*k), v))
            .filter(|(k, _)| !k.contains("Label") && !k.contains("Doc"))
            .map(|(k, v)| format!("{k}={v:?}"))
            .chain(fed.get(&n.id).into_iter().flatten().cloned())
            .collect();
        props.sort();
        out.push(format!(
            "{} {} {}",
            n.gate_class,
            ports.join(" "),
            props.join(" ")
        ));
    }
    for w in m
        .wires
        .iter()
        .filter(|w| literal(&w.source.node_id).is_none())
    {
        out.push(format!(
            "{}.{:?} -> {}.{:?}",
            class(&w.source.node_id),
            w.source.port,
            class(&w.target.node_id),
            w.target.port
        ));
    }
}

type OutcomeKey = (String, Vec<(String, String)>, bool);

thread_local! {
    /// The outcomes [`memoized`] keeps while it runs, by source, deps and fold.
    static MEMO: std::cell::RefCell<Option<crate::collections::HashMap<OutcomeKey, Outcome>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f`, compiling each `(src, deps, fold)` it asks [`outcome`] for once.
pub(super) fn memoized<T>(f: impl FnOnce() -> T) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            MEMO.with(|m| *m.borrow_mut() = None);
        }
    }
    MEMO.with(|m| *m.borrow_mut() = Some(HashMap::default()));
    let _clear = Clear;
    f()
}

/// Compile `src` (importing `deps`) in one fold mode. Inside [`memoized`], a
/// repeated request gives the first one's outcome.
pub(super) fn outcome(src: &str, deps: &[(&str, &str)], fold: bool) -> Outcome {
    let key: OutcomeKey = (
        src.to_string(),
        deps.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        fold,
    );
    if let Some(hit) = MEMO.with(|m| m.borrow().as_ref().and_then(|m| m.get(&key).cloned())) {
        return hit;
    }
    let o = compile_outcome(src, deps, fold);
    MEMO.with(|m| {
        if let Some(m) = m.borrow_mut().as_mut() {
            m.insert(key, o.clone());
        }
    });
    o
}

/// [`outcome`], from one run of the compile: the shape is the module that run lowered.
fn compile_outcome(src: &str, deps: &[(&str, &str)], fold: bool) -> Outcome {
    use crate::resolve::MemLoader;
    let fold_mode = if fold {
        crate::FoldMode::ForceOn
    } else {
        crate::FoldMode::ForceOff
    };
    let loader = MemLoader {
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
    let (lowered, result) = crate::compile::compile_keeping_lowered(
        input,
        crate::emit::EmitOptions::default(),
        &loader,
    );
    let (compiled, mut codes, comps) = match result {
        Ok(out) => (
            true,
            out.diagnostics.iter().map(|d| d.code.clone()).collect(),
            save_components(&out.brz),
        ),
        Err(crate::CompileError::HasErrors(errors)) => (
            false,
            errors.iter().map(|d| d.code.clone()).collect::<Vec<_>>(),
            Vec::new(),
        ),
        Err(e) => panic!("{e:?}\n{src}"),
    };
    codes.sort();
    let mut shape = Vec::new();
    ir_shape(
        &lowered.expect("a compile that did not fail in emit keeps its module"),
        &mut shape,
    );
    shape.sort();
    Outcome {
        compiled,
        codes,
        shape,
        comps,
    }
}

/// Every component of every grid of a save, each as text, sorted.
pub(super) fn save_components(brz: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = wirescript_ir::readback::Save::read(brz)
        .components()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    out.sort();
    out
}

/// `template` with `@` spelled `name`, and `%` spelled `outer`.
fn spell_as(template: &str, name: &str, outer: &str) -> String {
    template.replace('@', name).replace('%', outer)
}

/// `template` with `@` (and `%`) spelled `name`.
fn spell(template: &str, name: &str) -> String {
    spell_as(template, name, name)
}

/// The renamed program's outcome, read as the shadowing one's would be: its
/// `Zq` spelled `name` wherever a name reaches the output.
fn as_shadow(o: Outcome, name: &str) -> Outcome {
    let k = |v: Vec<String>| {
        v.into_iter()
            .map(|s| s.replace("Zq", name))
            .collect::<Vec<_>>()
    };
    let mut shape = k(o.shape);
    shape.sort();
    let mut comps = k(o.comps);
    comps.sort();
    Outcome {
        compiled: o.compiled,
        codes: o.codes,
        shape,
        comps,
    }
}

/// Assert `template` compiles alike with `@` spelled `K` and `Zq`, in both
/// fold modes. Returns whether the renamed program compiled.
fn assert_twins(what: &str, template: &str, deps: &[(&str, &str)]) -> bool {
    matches!(twins(what, template, deps, "K"), Twins::Compiled)
}

/// How a pair of twins came out, once they agree.
#[derive(Debug, PartialEq)]
enum Twins {
    Compiled,
    /// Both rejected, with these codes.
    Rejected(Vec<String>),
}

/// Assert `template` compiles alike with `@` spelled `name` (shadowing) and
/// `Zq`, `%` spelled `name` in both, in both fold modes. Twins rejected in
/// one mode are rejected in both.
fn twins(what: &str, template: &str, deps: &[(&str, &str)], name: &str) -> Twins {
    twins_in(what, template, deps, name, &[false, true])
}

/// [`twins`], in the fold modes `folds`.
fn twins_in(
    what: &str,
    template: &str,
    deps: &[(&str, &str)],
    name: &str,
    folds: &[bool],
) -> Twins {
    let spelled = |n: &str| {
        deps.iter()
            .map(|(d, s)| (d.to_string(), spell_as(s, n, name)))
            .collect::<Vec<_>>()
    };
    let (sd, rd) = (spelled(name), spelled("Zq"));
    let sd: Vec<(&str, &str)> = sd.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let rd: Vec<(&str, &str)> = rd.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut result = Twins::Compiled;
    for &fold in folds {
        let shadow = outcome(&spell_as(template, name, name), &sd, fold);
        let renamed = as_shadow(outcome(&spell_as(template, "Zq", name), &rd, fold), name);
        if !renamed.compiled {
            result = Twins::Rejected(renamed.codes.clone());
        }
        // A program that does not compile ships nothing: twins that both fail
        // with the same diagnostics agree, and the caller lists each such pair.
        let both_rejected = !shadow.compiled && !renamed.compiled && shadow.codes == renamed.codes;
        if shadow != renamed && !both_rejected {
            let only = |a: &[String], b: &[String]| {
                a.iter()
                    .filter(|s| !b.contains(s))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            panic!(
                "{what}, fold {fold}: a name shadowing the constant `K` reads differently from its renamed twin\n\
                 codes: {:?} vs {:?}\nIR only shadowing: {:#?}\nIR only renamed: {:#?}\n\
                 save only shadowing: {:#?}\nsave only renamed: {:#?}\n{}",
                shadow.codes,
                renamed.codes,
                only(&shadow.shape, &renamed.shape),
                only(&renamed.shape, &shadow.shape),
                only(&shadow.comps, &renamed.comps),
                only(&renamed.comps, &shadow.comps),
                spell_as(template, name, name),
            );
        }
    }
    result
}

/// Every pair of `bindings` x `uses` (each `@` read through `read`) compared
/// as twins under the shadowing spelling `name`: the pair count, and the pairs
/// both twins reject with their codes. Both folds when `full`, else one,
/// alternating so every binding and use meets both.
fn matrix(
    head: &str,
    bindings: &[(&str, &str)],
    uses: &[(&str, &str)],
    read: &str,
    deps: &[(&str, &str)],
    name: &str,
    full: bool,
) -> (usize, Vec<(String, Vec<String>)>) {
    let mut pairs = 0;
    let mut rejected = Vec::new();
    for (bi, (bwhat, binding)) in bindings.iter().enumerate() {
        for (ui, (uwhat, body)) in uses.iter().enumerate() {
            pairs += 1;
            let what = format!("{name}: {bwhat} read by {uwhat}");
            let template = format!(
                "{head}{}",
                binding.replace("{BODY}", &body.replace('@', read))
            );
            let folds: &[bool] = if full {
                &[false, true]
            } else if (bi + ui) % 2 == 0 {
                &[false]
            } else {
                &[true]
            };
            if let Twins::Rejected(codes) = twins_in(&what, &template, deps, name, folds) {
                rejected.push((format!("{bwhat} read by {uwhat}"), codes));
            }
        }
    }
    (pairs, rejected)
}

/// Assert the pairs both twins reject are exactly `expected`: each pair with
/// its codes and why the program is rejected.
fn assert_rejected(got: &[(String, Vec<String>)], expected: &[(&str, &[&str], &str)]) {
    let want: Vec<(String, Vec<String>)> = expected
        .iter()
        .map(|(pair, codes, _why)| {
            (
                pair.to_string(),
                codes.iter().map(std::string::ToString::to_string).collect(),
            )
        })
        .collect();
    assert_eq!(got, want.as_slice(), "the pairs both twins reject");
}

const HEAD: &str = "in go: exec\nin a: int\nin fa: float\nin c: bool\n\
                    var n: int = 0\nout on1: int = n\nvar f: float = 0.0\nout of1: float = f\n\
                    var s: string = \"\"\nout os: string = s\nvar v: vector = Vec(0.0, 0.0, 0.0)\nout ov: vector = v\n\
                    var xs: int[] = []\nout oe: int\nvar w: int = 3\n\
                    enum E { A(int), B }\nvar e: E = E.A(7)\ntype P = { @: int, y: int }\nconst K = 4\n";

/// Each way a scope binds `@` to a value that is not the module's `K`, with
/// `{BODY}` where the binding is in scope (in an exec context).
const BINDINGS: &[(&str, &str)] = &[
    (
        "mod int parameter",
        "mod g(@: int) {\n{BODY}\n}\non go { g(a) }",
    ),
    (
        "mod float parameter",
        "mod g(@: float) {\n{BODY}\n}\non go { g(fa) }",
    ),
    (
        "generic mod parameter",
        "mod g<T: int | float>(@: T) {\n{BODY}\n}\non go { g(a)\ng(fa) }",
    ),
    (
        "chip parameter",
        "chip C(t: exec, @: int) {\non t {\n{BODY}\n}\n}\nlet q = C(go, a)",
    ),
    (
        "const parameter",
        "mod g(@: const int) {\n{BODY}\n}\non go { g(7) }",
    ),
    (
        "ref parameter",
        "mod g(@: *int) {\n{BODY}\n}\non go { g(w) }",
    ),
    ("handler let", "on go { let @ = a\n{BODY}\n}"),
    ("handler var", "on go { var @: int = a\n{BODY}\n}"),
    (
        "handler static var",
        "on go { static var @: int = 0\n@ = a\n{BODY}\n}",
    ),
    ("handler const", "on go { const @ = 9\n{BODY}\n}"),
    ("for variable", "on go { for @ in 0..a {\n{BODY}\n} }"),
    (
        "match binding",
        "on go { match e { A(@) => {\n{BODY}\n}, B => {} } }",
    ),
    ("if-let binding", "on go { if let A(@) = e {\n{BODY}\n} }"),
    (
        "let-else binding",
        "on go { let A(@) = e else { return }\n{BODY}\n}",
    ),
    (
        "record destructure",
        "on go { let p: P = { @: a, y: 1 }\nlet { @, y } = p\n{BODY}\n}",
    ),
    (
        "tuple destructure",
        "on go { let (@, z) = (a, 1)\n{BODY}\n}",
    ),
    (
        "destructured record parameter",
        "mod g({ @, y }: P) {\n{BODY}\n}\non go { g({ @: a, y: 1 }) }",
    ),
    (
        "self mod parameter",
        "mod g(self: int, @: int) {\n{BODY}\n}\non go { a.g(a) }",
    ),
    (
        "var initialized from the outer name",
        "on go { var @: int = % + a\n{BODY}\n}",
    ),
    ("let of the outer name", "on go { let @ = %\n{BODY}\n}"),
];

/// Each way a body reads `@`.
const USES: &[(&str, &str)] = &[
    ("int slot", "n = @"),
    ("float slot", "f = @"),
    ("let alias chain", "let q = @\nlet r = q\nf = r"),
    ("let of an operation", "let q = @ * 2\nf = q"),
    ("arithmetic", "n = @ + 1\nf = @ * 2.5"),
    ("abs", "n = abs(@)"),
    ("min and max", "n = min(@, 9) + max(@, 0)"),
    ("clamp", "f = clamp(@, 0.0, 9.0)"),
    ("Vec", "v = Vec(@, 0.0, 0.0)"),
    ("Select", "n = Select(c, @, 1)"),
    ("Select through an alias", "let q = @\nn = Select(c, q, 1)"),
    ("array literal index", "n = [@, 1][0]"),
    ("push and sort", "xs.push(@)\nxs.sort()\nn = xs[0]"),
    ("record field", "let r = { u: @ }\nn = r.u"),
    ("interpolation", "s = \"v${@}\""),
    ("comparison", "let b = @ == 4\nif b { n = 1 }"),
    ("condition", "if @ == 4 { n = 1 } else { n = 2 }"),
    (
        "if expression through an alias",
        "let q = @\nf = if c then q else 0.5",
    ),
    ("return", "mod rr() -> int { return @ }\nn = rr()"),
    ("emit", "emit oe = @"),
    ("loop bound", "for i in 0..@ { n = n + 1 }"),
];

/// A container binding shadowing a constant container: an initializer that
/// names it copies the binding's elements, never the constant's.
#[test]
fn a_container_binding_hides_an_outer_constant_container() {
    a_container_binding_hides_an_outer_constant_container_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_container_binding_hides_an_outer_constant_container_exhaustive() {
    a_container_binding_hides_an_outer_constant_container_matrix(true);
}

/// Every kind x binding x initializer when `full`; otherwise every kind x
/// binding with one initializer, rotating so each kind and each binding
/// meets every initializer.
fn a_container_binding_hides_an_outer_constant_container_matrix(full: bool) {
    let head = "in go: exec\nvar n: int = 0\nout on1: int = n\nenum Hue { Red, Green, Blue }\n\
                type R = { h: Hue, k: int }\n";
    // Per kind: the constant, a value of the kind to bind, its type and a read.
    let kinds = [
        (
            "int array",
            "[1, 2]",
            "[7, 8, 9]",
            "int[]",
            "n = cs.length() + cs[0]",
        ),
        (
            "map",
            "{ \"a\": 1 }",
            "{ \"b\": 5, \"c\": 6 }",
            "Map<string, int>",
            "n = cs.length()",
        ),
        (
            "record array",
            "[{ h: Hue.Red, k: 1 }]",
            "[{ h: Hue.Blue, k: 5 }, { h: Hue.Green, k: 6 }]",
            "R[]",
            "n = cs.length() + cs[0].k",
        ),
        (
            "enum array",
            "[Hue.Blue, Hue.Red]",
            "[Hue.Green]",
            "Hue[]",
            "n = cs.length() + cs[0].ToInt()",
        ),
    ];
    for (ki, (kwhat, constant, value, ty, read)) in kinds.into_iter().enumerate() {
        let bindings = [
            (
                "mod parameter",
                format!(
                    "var src: {ty} = {value}\nmod g(@: {ty}) {{\n{{BODY}}\n}}\non go {{ g(src) }}"
                ),
            ),
            (
                "generic parameter",
                format!(
                    "var src: {ty} = {value}\nmod g<T>(@: T) {{\n{{BODY}}\n}}\non go {{ g(src) }}"
                ),
            ),
            (
                "chip parameter",
                format!(
                    "var src: {ty} = {value}\nchip C(t: exec, @: {ty}) {{\non t {{\n{{BODY}}\n}}\n}}\nlet q = C(go, src)"
                ),
            ),
            (
                "const parameter",
                format!("mod g(@: const {ty}) {{\n{{BODY}}\n}}\non go {{ g({value}) }}"),
            ),
            (
                "handler var",
                format!("on go {{ var @: {ty} = {value}\n{{BODY}}\n}}"),
            ),
            (
                "let alias",
                format!("var src: {ty} = {value}\non go {{ let @ = src\n{{BODY}}\n}}"),
            ),
        ];
        for (bi, (bwhat, binding)) in bindings.iter().enumerate() {
            let field_read = read.replace("cs", "w.cs");
            let slots = [
                ("handler-local", format!("var cs: {ty} = @\n{read}")),
                ("static", format!("static var cs: {ty} = @\n{read}")),
                (
                    "handler-local record field",
                    format!("var w: W = {{ cs: @, z: 1 }}\n{field_read}"),
                ),
                (
                    "static record field",
                    format!("static var w: W = {{ cs: @, z: 1 }}\n{field_read}"),
                ),
            ];
            let n = slots.len();
            for (si, (swhat, body)) in slots.into_iter().enumerate() {
                if !full && si != (ki + bi) % n {
                    continue;
                }
                let template = format!(
                    "{head}type W = {{ cs: {ty}, z: int }}\nconst K: {ty} = {constant}\n{}",
                    binding.replace("{BODY}", &body)
                );
                assert_twins(
                    &format!("{kwhat} {bwhat}, {swhat} initializer"),
                    &template,
                    &[],
                );
            }
        }
    }
}

/// The bindings each part of the `K` matrix runs, as ranges of [`BINDINGS`]:
/// together every binding once.
const K_PARTS: [std::ops::Range<usize>; 4] = [0..5, 5..10, 10..15, 15..20];

/// The bindings in `part` with every reader, both folds.
fn k_matrix_part(part: usize) {
    let mut covered: Vec<usize> = K_PARTS.iter().flat_map(std::clone::Clone::clone).collect();
    covered.sort_unstable();
    assert_eq!(
        covered,
        (0..BINDINGS.len()).collect::<Vec<_>>(),
        "each binding in one part"
    );
    let bindings = &BINDINGS[K_PARTS[part].clone()];
    let (pairs, rejected) = matrix(HEAD, bindings, USES, "@", &[], "K", true);
    assert_eq!(
        pairs,
        bindings.len() * USES.len(),
        "every binding with every reader"
    );
    let expected: &[(&str, &[&str], &str)] = &[
        (
            "chip parameter read by emit",
            &["WS057"],
            "a chip body cannot drive the module's output",
        ),
        (
            "for variable read by return",
            &["WS078"],
            "a loop body cannot declare a mod that returns",
        ),
    ];
    let expected: Vec<_> = expected
        .iter()
        .copied()
        .filter(|(pair, _, _)| {
            bindings
                .iter()
                .any(|(b, _)| pair.starts_with(&format!("{b} read by ")))
        })
        .collect();
    assert_rejected(&rejected, &expected);
}

#[test]
fn a_binding_hides_an_outer_constant_from_every_reader() {
    k_matrix_part(0);
}

#[test]
fn a_binding_hides_an_outer_constant_from_every_reader_part2() {
    k_matrix_part(1);
}

#[test]
fn a_binding_hides_an_outer_constant_from_every_reader_part3() {
    k_matrix_part(2);
}

#[test]
fn a_binding_hides_an_outer_constant_from_every_reader_part4() {
    k_matrix_part(3);
}

/// A binding named like a bare enum variant (an `int` here) is that binding
/// for every reader: none takes it for the variant `Blue`.
#[test]
fn a_binding_named_like_a_variant_hides_it_from_every_reader() {
    a_binding_named_like_a_variant_hides_it_from_every_reader_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_binding_named_like_a_variant_hides_it_from_every_reader_exhaustive() {
    a_binding_named_like_a_variant_hides_it_from_every_reader_matrix(true);
}

/// Every pair in both fold modes when `full`, else each pair in one (the
/// binding hides the variant before folding runs).
fn a_binding_named_like_a_variant_hides_it_from_every_reader_matrix(full: bool) {
    let head = format!("{HEAD}enum Hue {{ Red, Green, Blue }}\n");
    let bindings: Vec<(&str, &str)> = BINDINGS
        .iter()
        .copied()
        .filter(|(what, _)| !what.contains("outer name"))
        .collect();
    let (pairs, rejected) = matrix(&head, &bindings, USES, "@", &[], "Blue", full);
    assert_eq!(pairs, 18 * 21, "every binding with every reader");
    assert_rejected(
        &rejected,
        &[
            (
                "chip parameter read by emit",
                &["WS057"],
                "a chip body cannot drive the module's output",
            ),
            (
                "for variable read by return",
                &["WS078"],
                "a loop body cannot declare a mod that returns",
            ),
        ],
    );
}

/// The library an `import * as u` names in the namespace-alias spelling.
const ALIAS_LIB: &str = "pub const K = 4\npub type R = { K: int, y: int }\n";

/// A record binding named like an `import * as u` alias: `u.K` is its field
/// for every reader, never the namespace's constant `K`.
#[test]
fn a_binding_named_like_a_namespace_alias_hides_its_members() {
    a_binding_named_like_a_namespace_alias_hides_its_members_matrix(false);
}

#[test]
#[ignore = "exhaustive"]
fn a_binding_named_like_a_namespace_alias_hides_its_members_exhaustive() {
    a_binding_named_like_a_namespace_alias_hides_its_members_matrix(true);
}

/// Every pair in both fold modes when `full`, else each pair in one (the
/// binding hides the alias before folding runs).
fn a_binding_named_like_a_namespace_alias_hides_its_members_matrix(full: bool) {
    let head = format!("import * as u from \"lib\"\n{HEAD}");
    let bindings: &[(&str, &str)] = &[
        (
            "mod parameter",
            "mod g(@: u.R) {\n{BODY}\n}\non go { g({ K: a, y: 1 }) }",
        ),
        (
            "mod parameter from a var",
            "var src: u.R = { K: 6, y: 1 }\nmod g(@: u.R) {\n{BODY}\n}\non go { g(src) }",
        ),
        (
            "chip parameter",
            "chip C(t: exec, @: u.R) {\non t {\n{BODY}\n}\n}\nlet q = C(go, { K: a, y: 1 })",
        ),
        (
            "self mod parameter",
            "mod g(self: int, @: u.R) {\n{BODY}\n}\non go { a.g({ K: a, y: 1 }) }",
        ),
        ("handler let", "on go { let @ = { K: a, y: 2 }\n{BODY}\n}"),
        (
            "handler var",
            "on go { var @: u.R = { K: a, y: 2 }\n{BODY}\n}",
        ),
        (
            "var initialized from the outer name",
            "on go { var @: u.R = { K: %.K + a, y: 2 }\n{BODY}\n}",
        ),
        (
            "let of the outer name",
            "on go { let @ = { K: %.K, y: 2 }\n{BODY}\n}",
        ),
    ];
    let (pairs, rejected) = matrix(
        &head,
        bindings,
        USES,
        "@.K",
        &[("lib", ALIAS_LIB)],
        "u",
        full,
    );
    assert_eq!(pairs, 8 * 21, "every binding with every reader");
    assert_rejected(
        &rejected,
        &[(
            "chip parameter read by emit",
            &["WS057"],
            "a chip body cannot drive the module's output",
        )],
    );
}

/// An enum binding named like one of its enum's bare variants: every reader
/// reads the binding's value, none the variant `Blue`.
#[test]
fn an_enum_binding_named_like_a_variant_reads_its_value() {
    let head = "in go: exec\nin a: int\nin c: bool\nvar n: int = 0\nout on1: int = n\n\
                var f: float = 0.0\nout of1: float = f\nenum Hue { Red, Green, Blue }\n\
                var h: Hue = Hue.Green\nvar hs: Hue[]\n";
    let bindings: &[(&str, &str)] = &[
        (
            "mod parameter",
            "mod g(@: Hue) {\n{BODY}\n}\non go { g(h) }",
        ),
        (
            "self mod parameter",
            "mod g(self: int, @: Hue) {\n{BODY}\n}\non go { a.g(h) }",
        ),
        (
            "const parameter",
            "mod g(@: const Hue) {\n{BODY}\n}\non go { g(Hue.Green) }",
        ),
        (
            "top-level var",
            "var @: Hue = Hue.Green\non go {\n{BODY}\n}",
        ),
        ("handler let", "on go { let @ = h\n{BODY}\n}"),
        ("handler var", "on go { var @: Hue = h\n{BODY}\n}"),
        ("let of the outer name", "on go { let @ = %\n{BODY}\n}"),
        (
            "var initialized from the outer name",
            "on go { var @: Hue = %\n{BODY}\n}",
        ),
    ];
    let uses: &[(&str, &str)] = &[
        ("tag", "n = @.ToInt()"),
        ("EnumToInt", "n = EnumToInt(@)"),
        ("Discriminant", "n = @.Discriminant"),
        (
            "match",
            "match @ { Red => { n = 1 }, Green => { n = 2 }, Blue => { n = 3 } }",
        ),
        ("variant test", "if @ is Hue.Blue { n = 1 } else { n = 2 }"),
        ("if expression", "n = if @ is Hue.Red then 1 else 2"),
        (
            "array initializer",
            "var ys: Hue[] = [@, Hue.Red]\nn = ys[0].ToInt()",
        ),
        ("push", "hs.push(@)\nn = hs[0].ToInt()"),
        ("let alias", "let q = @\nn = q.ToInt()"),
        ("Select", "n = Select(c, @.ToInt(), 1)"),
        ("float slot", "f = @.ToInt()"),
    ];
    let (pairs, rejected) = matrix(head, bindings, uses, "@", &[], "Blue", true);
    assert_eq!(pairs, 8 * 11, "every binding with every reader");
    assert_rejected(&rejected, &[]);
}

/// A namespaced library body reads its own module's constants, scalars and
/// containers alike, when the importer declares a constant of the same name;
/// the importer's own reads keep the importer's.
#[test]
fn a_namespaced_body_reads_its_own_modules_containers() {
    let lib = "pub const K = 4\npub const T = [4, 5]\n\
               pub mod rt(i: int) -> int { return T[i] }\npub mod rt2() -> int { return T[1] }\n\
               pub mod rlen() -> int { return T.length() }\npub mod rk() -> int { return K }\n\
               pub const mod tab(i: int) -> int { return T[i] }\n\
               pub chip ct(i: int) -> (r: int) { out r = T[i] + K }\n\
               pub mod viatab() -> int { return tab(1) }\n\
               pub mod viaconst() -> int { const z = tab(1)\nreturn z }\n";
    let head = "import * as u from \"lib\"\nin go: exec\nin a: int\nvar n: int = 0\nout on1: int = n\n\
                var m: int = 0\nout om: int = m\nconst @ = [7, 7, 7]\n";
    let uses: &[(&str, &str)] = &[
        (
            "constant index and length",
            "on go { n = u.rt2() + u.rlen() }",
        ),
        ("runtime index", "on go { n = u.rt(a) }"),
        (
            "runtime index after the importer's own",
            "on go { m = @[a]\nn = u.rt(a) }",
        ),
        ("scalar", "on go { n = u.rk() }"),
        ("const mod", "on go { n = u.tab(0) + u.tab(1) }"),
        ("chip", "on go { n = u.ct(a) }"),
        (
            "bare const mod call in the library",
            "on go { n = u.viatab() }",
        ),
        (
            "const of a const mod call in the library",
            "on go { n = u.viaconst() }",
        ),
        (
            "beside the importer's own gate at the top level",
            "chip C(xs: int[]) -> (r: int) { out r = xs.length() }\nlet q = C(@, exec = go)\nout oq: int = q.r\non go { n = u.rt(a) }",
        ),
    ];
    for (what, body) in uses {
        let template = format!("{head}{body}");
        let got = twins(
            &format!("namespaced body, {what}"),
            &template,
            &[("lib", lib)],
            "T",
        );
        assert_eq!(got, Twins::Compiled, "{what}");
    }
    // The library's values, not the importer's: 5 + 2 folded.
    let r = compile_multi(
        &format!("{}{}", spell(head, "T"), uses[0].1),
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    let mut shape = Vec::new();
    ir_shape(&r.module, &mut shape);
    assert!(
        shape.iter().any(|s| s.contains("Int(7)")),
        "u.rt2() + u.rlen() is 5 + 2: {shape:#?}"
    );
}

/// The positive control: unshadowed, each reader takes the constant `K` as
/// the literal it names, exactly as if the literal were written there.
#[test]
fn an_unshadowed_constant_is_its_literal_for_every_reader() {
    let head = spell(HEAD, "pf");
    for (uwhat, body) in USES {
        let named = format!("{head}on go {{\n{}\n}}", body.replace('@', "K"));
        let literal = format!("{head}on go {{\n{}\n}}", body.replace('@', "4"));
        for fold in [false, true] {
            let (a, b) = (outcome(&named, &[], fold), outcome(&literal, &[], fold));
            assert!(a.compiled, "{uwhat}: {:?}\n{named}", a.codes);
            // A constructor given a named constant keeps its gate, which holds
            // the constant, where its literal twin folds to a vector value.
            if *uwhat == "Vec" {
                assert!(
                    a.shape
                        .iter()
                        .any(|s| s.contains("MakeVector") && s.contains("X=Int(4)")),
                    "Vec, fold {fold}: {:#?}",
                    a.shape
                );
                continue;
            }
            assert_eq!(
                a, b,
                "{uwhat}, fold {fold}: the constant `K` did not read as the literal 4\n{named}"
            );
        }
    }
}

/// Once the scope that hid it closes, the constant reads as itself again.
#[test]
fn a_constant_comes_back_after_the_scope_that_hid_it() {
    let after = [
        ("block", "on go { if c { let @ = a\nn = @ }\nf = K }"),
        ("mod body", "mod g(@: int) { n = @ }\non go { g(a)\nf = K }"),
        ("loop", "on go { for @ in 0..a { n = n + @ }\nf = K + 1 }"),
        (
            "match arm",
            "on go { match e { A(@) => { n = @ }, B => {} }\nf = abs(K) }",
        ),
        (
            "chip",
            "chip C(t: exec, @: int) { on t { n = @ } }\nlet q = C(go, a)\non go { f = K }",
        ),
    ];
    for (what, body) in after {
        assert!(
            assert_twins(what, &format!("{HEAD}{body}"), &[]),
            "{what}: did not compile"
        );
    }
}

/// An enum constant shadowed by an enum binding: a `match` on it, a
/// comparison, and its tag.
#[test]
fn a_binding_hides_an_outer_enum_constant() {
    let head = "in go: exec\nin hi: int\nvar n: int = 0\nout on1: int = n\nenum Hue { Red, Green, Blue }\n\
                var h: Hue = Hue.Red\nconst K = Hue.Blue\n";
    let bindings = [
        (
            "mod parameter",
            "mod g(@: Hue) {\n{BODY}\n}\non go { g(h) }",
        ),
        ("handler let", "on go { let @ = h\n{BODY}\n}"),
        ("handler var", "on go { var @: Hue = h\n{BODY}\n}"),
    ];
    let uses = [
        ("match", "match @ { Blue => { n = 1 }, _ => { n = 2 } }"),
        ("variant test", "if @ is Hue.Blue { n = 1 } else { n = 2 }"),
        ("tag", "n = @.ToInt()"),
    ];
    for (bwhat, binding) in bindings {
        for (uwhat, body) in uses {
            let template = format!("{head}{}", binding.replace("{BODY}", body));
            assert!(
                assert_twins(&format!("enum {bwhat} read by {uwhat}"), &template, &[]),
                "{bwhat} {uwhat}"
            );
        }
    }
}

/// A library's parameter named like the library's own constant, called
/// through a named import and through a namespace, and a namespaced constant
/// beside a local of its bare name.
#[test]
fn an_imported_parameter_hides_its_own_modules_constant() {
    let lib = "pub const K = 4\n\
               pub mod li(@: int) -> int { return abs(@) + @ }\n\
               pub mod lf(@: float) -> float { let q = @\nreturn q * 2.0 }\n\
               pub mod ls(@: int) -> int { let q = @\nreturn Select(true, q, 1) }\n";
    let main = "import * as u from \"lib\"\nimport { li, lf, ls } from \"lib\"\n\
                in go: exec\nin a: int\nin fa: float\nvar n: int = 0\nout on1: int = n\n\
                var f: float = 0.0\nout of1: float = f\n\
                on go { n = li(a) + u.li(a) + ls(a)\nf = lf(fa) + u.lf(fa) }";
    assert!(assert_twins("imported parameter", main, &[("lib", lib)]));
    let beside = "import * as u from \"lib\"\nin go: exec\nin a: int\nvar n: int = 0\nout on1: int = n\n\
                  on go { let @ = a\nn = u.K + @ }";
    assert!(assert_twins(
        "namespaced constant beside a local",
        beside,
        &[("lib", "pub const K = 4\n")]
    ));
}

/// `const q = K`, `K` a binding that hides the constant `K`, is `WS046`: the
/// binding is no constant, whatever the outer `K` holds.
#[test]
fn a_const_of_a_binding_hiding_a_constant_is_ws046() {
    let head = "in go: exec\nin a: int\nvar n: int = 0\nout on1: int = n\nconst K = 4\n";
    for body in [
        "mod g(K: int) -> int { const q = K\nreturn q }\non go { n = g(a) }",
        "on go { let K = a\nconst q = K\nn = q }",
        "on go { for K in 0..a { const q = K\nn = q } }",
    ] {
        let codes = errs(&format!("{head}{body}"));
        assert!(codes.iter().any(|c| c == "WS046"), "{body}: {codes:?}");
    }
    assert!(
        errs(&format!("{head}on go {{ const q = K\nn = q }}")).is_empty(),
        "control"
    );
}

/// A mod of an `import * as` module reads its module's constants as
/// constants, as the type check does: a constant-only argument bakes (no
/// `WS028`) and an integer constant in a `float` place is that float.
#[test]
fn a_namespaced_mod_reads_its_modules_constants() {
    let lib = "pub const CH = \"chx\"\npub const K = 4\n\
               pub mod send(v: float) { SendCustomEvent(CH, v) }\npub mod fk() -> float { return K }\n";
    let main = "import * as u from \"lib\"\nin go: exec\nvar f: float = 0.0\nout of1: float = f\n\
                on go { u.send(1.5)\nf = u.fk() }\non CustomEvent(\"chx\") -> (v: float) { f = v }";
    for fold in [false, true] {
        let o = outcome(main, &[("lib", lib)], fold);
        assert!(o.compiled, "fold {fold}: {:?}", o.codes);
        assert!(
            o.comps
                .iter()
                .any(|c| c.contains("SendCustomEvent") && c.contains("\"chx\"")),
            "fold {fold}"
        );
    }
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    let mut lits = Vec::new();
    ir_shape(&r.module, &mut lits);
    assert!(
        lits.iter().any(|s| s.contains("Float(4.0)")),
        "the constant in the float place: {lits:#?}"
    );
}

/// A name bound again in the same frame reads its new binding, not the
/// constant the frame recorded for the old one: in a constant-only slot, a
/// builtin's argument and a `float` place alike.
#[test]
fn a_name_bound_again_in_its_frame_reads_the_new_binding() {
    let head = "in go: exec\nin a: int\nvar n: int = 0\nout on1: int = n\nvar f: float = 0.0\nout of1: float = f\n";
    for (what, body) in [
        ("let", "on go { let q = 9\nlet q = a\nn = abs(q)\nf = q }"),
        (
            "let-else",
            "enum E { A(int), B }\nvar e: E = E.A(7)\non go { let q = 9\nlet A(q) = e else { return }\nn = abs(q)\nf = q }",
        ),
        (
            "destructure",
            "on go { let q = 9\nlet (q, z) = (a, 1)\nn = abs(q)\nf = q }",
        ),
    ] {
        for fold in [false, true] {
            let o = outcome(&format!("{head}{body}"), &[], fold);
            assert!(o.compiled, "{what}: {:?}", o.codes);
            assert!(
                !o.shape
                    .iter()
                    .any(|s| s.contains("Int(9)") || s.contains("Float(9.0)")),
                "{what} fold {fold}: {:#?}",
                o.shape
            );
        }
    }
}

/// A `mod` or `chip` body's `var` named like a variant or an enum is not
/// there before its declaration: a read above it, and its own initializer,
/// read the variant or the enum, in typecheck and lowering alike, as the
/// twin whose `var` has another name does. `@` is the `var`, `%` the variant
/// (`Blue`) or enum (`Hue`) in both twins.
#[test]
fn a_body_var_named_like_a_variant_or_enum_is_not_there_before_it() {
    let head = "in go: exec\nin a: int\nin c: bool\nvar n: int = 0\nout on1: int = n\n\
                var f: float = 0.0\nout of1: float = f\nenum Hue { Red, Green, Blue }\nvar hs: Hue[]\n";
    let variant = [
        (
            "mod: EnumToInt above",
            "mod m(p: int) -> int { let q = EnumToInt(%)\nvar @: int = p\nreturn q * 100 + @ }\non go { n = m(a) }",
        ),
        (
            "mod: annotated let above",
            "mod m(p: int) -> int { let q: Hue = %\nvar @: int = p\nreturn q.ToInt() * 100 + @ }\non go { n = m(a) }",
        ),
        (
            "mod: push above",
            "mod m(p: int) -> int { hs.push(%)\nvar @: int = p\nreturn hs[0].ToInt() * 100 + @ }\non go { n = m(a) }",
        ),
        (
            "mod: initializer",
            "mod m(p: int) -> int { var @: int = EnumToInt(%) + p\nreturn @ }\non go { n = m(a) }",
        ),
        (
            "mod: enum initializer reset each call",
            "mod m(p: int) -> int { var @: Hue = %\nif p > 0 { @ = Hue.Red }\nreturn @.ToInt() }\non go { n = m(a)\nf = m(0 - a) }",
        ),
        (
            "mod: nested block",
            "mod m(p: int) -> int { if c { n = EnumToInt(%) }\nvar @: int = p\nreturn @ }\non go { f = m(a) }",
        ),
        (
            "chip: handler above",
            "chip C(g3: exec, p: int) { on g3 { n = EnumToInt(%) }\nvar @: int = p\non g3 { f = @ } }\nlet q = C(go, a)",
        ),
        (
            "chip: initializer",
            "chip C(g3: exec, p: int) { var @: int = EnumToInt(%) + p\non g3 { n = @ } }\nlet q = C(go, a)",
        ),
        (
            "handler: read above",
            "on go { let q = EnumToInt(%)\nvar @: int = a\nn = q * 100 + @ }",
        ),
        (
            "handler block: read above",
            "on go { if c { let q = EnumToInt(%)\nvar @: int = a\nn = q * 100 + @ } }",
        ),
    ];
    for (what, body) in variant {
        assert_eq!(
            twins(what, &format!("{head}{body}"), &[], "Blue"),
            Twins::Compiled,
            "{what}"
        );
    }
    let enumeration = [
        (
            "mod: read above",
            "mod m(p: int) -> int { let q = EnumToInt(%.Blue)\nvar @: int = p\nreturn q * 100 + @ }\non go { n = m(a) }",
        ),
        (
            "mod: method above",
            "mod m(p: int) -> int { let q = %.Green.ToInt()\nvar @: int = p\nreturn q * 100 + @ }\non go { n = m(a) }",
        ),
        (
            "mod: initializer",
            "mod m(p: int) -> int { var @: int = EnumToInt(%.Blue) + p\nreturn @ }\non go { n = m(a) }",
        ),
        (
            "chip: handler above",
            "chip C(g3: exec, p: int) { on g3 { n = EnumToInt(%.Blue) }\nvar @: int = p\non g3 { f = @ } }\nlet q = C(go, a)",
        ),
        (
            "handler: read above",
            "on go { let q = EnumToInt(%.Blue)\nvar @: int = a\nn = q * 100 + @ }",
        ),
    ];
    for (what, body) in enumeration {
        assert_eq!(
            twins(what, &format!("{head}{body}"), &[], "Hue"),
            Twins::Compiled,
            "{what}"
        );
    }
}

/// A `var` initializer reads the names around the declaration, as a `let`'s
/// does, never the variable it declares; and in a `mod` or `chip` body a read
/// above a `var` reads the binding that `var` later shadows. `%` is the outer
/// name in both twins.
#[test]
fn a_var_initializer_reads_the_name_around_it() {
    let head = "in go: exec\nin a: int\nin c: bool\nvar n: int = 0\nout on1: int = n\n\
                var f: float = 0.0\nout of1: float = f\nconst K = 4\ntype P = { x: int, y: int }\n";
    for (what, body) in [
        (
            "handler var from a constant",
            "on go { var @: int = % + 1\nn = abs(@)\nf = @ }",
        ),
        (
            "handler var from a constant and an input",
            "on go { var @: int = % + a\nn = abs(@)\nf = @ }",
        ),
        (
            "static var from a constant",
            "on go { static var @: int = % + 1\nn = @ }",
        ),
        (
            "mod var from its parameter",
            "mod md(%: int) -> int { var @: int = % * 2\nreturn @ }\non go { n = md(a) }",
        ),
        (
            "block var from an outer var",
            "on go { var %: int = a\nif c { var @: int = % + 1\nn = @ } }",
        ),
        (
            "record var from a parameter",
            "mod md(%: P) -> int { var @: P = { x: %.x + 1, y: %.y }\nreturn @.x }\non go { n = md({ x: a, y: 2 }) }",
        ),
        (
            "read above a mod body's var",
            "mod md(z: int) -> int { let q = % + z\nvar @: int = 9 + z\nreturn q * @ }\non go { n = md(a) }",
        ),
        (
            "read above a chip body's var",
            "chip C(t: exec, z: int) { on t { n = % + z\nvar @: int = 9 + z\nf = @ } }\nlet q = C(go, a)",
        ),
    ] {
        assert_eq!(
            twins(what, &format!("{head}{body}"), &[], "K"),
            Twins::Compiled,
            "{what}"
        );
    }
    // With no outer name of its own, the initializer reads nothing, and the
    // message says why.
    for src in [
        "on go { var y: int = y + 1\nn = y }",
        "mod md() -> int { var y: int = y + 1\nreturn y }\non go { n = md() }",
        "on go { if c { var y = y * 2\nn = y } }",
    ] {
        let r = compile(&format!("{head}{src}"));
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"
                && d.message.contains("`y` is not declared before this `var`")
                && d.message
                    .contains("a `var`'s initializer cannot read the variable it declares")),
            "{src}: {:?}",
            r.diagnostics
        );
    }
    // The declaration it suggests spells the type as written, and names none
    // for a `var` written with none and an initializer of no known type.
    let hint = |src: &str| {
        compile(&format!("{head}{src}"))
            .diagnostics
            .into_iter()
            .find(|d| d.code == "WS002")
            .map(|d| d.message)
    };
    assert!(
        hint("on go { var y: int = y + 1\nn = y }").is_some_and(|m| m.contains("(`var y: int`)"))
    );
    assert!(
        hint("on go { static var y: int = y + 1\nn = y }")
            .is_some_and(|m| m.contains("(`static var y: int`)"))
    );
    let untyped = hint("on go { var y = y + 1\nn = y }").unwrap_or_default();
    assert!(
        untyped.contains("Declare it without one, or") && !untyped.contains(": T`"),
        "{untyped}"
    );
    // Checked against the parameter it reads, not the `var` it declares.
    let codes = errs(&format!(
        "{head}in s: string\nmod md(x: string) -> int {{ var x: int = x + 1\nreturn x }}\non go {{ n = md(s) }}"
    ));
    assert!(codes.iter().any(|c| c == "WS004"), "{codes:?}");
    assert!(
        errs(&format!("{head}in s: string\nmod md(x: string) -> int {{ var x: int = x.Length()\nreturn x }}\non go {{ n = md(s) }}")).is_empty(),
        "x.Length() of the string parameter"
    );
}
