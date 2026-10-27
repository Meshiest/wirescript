//! The `tests/fixtures/silent/` set: self-checking programs for silent
//! miscompiles, run in game by pasting each one. Running a fixture proves the
//! values; this guard holds the compile side: in both fold modes every fixture
//! keeps the structure its miscompile would break. `paste_fixtures.rs` checks
//! that each one compiles clean, with no placeholder gate or dangling wire.
//!
//! Every fixture needs a row in [`CHECKS`]; one without a structural check
//! fails [`every_fixture_has_a_structural_check`].

use crate::common;

use std::collections::{BTreeSet, HashSet, VecDeque};

use wirescript::FoldMode;
use wirescript::intern::resolve;
use wirescript::ir::gate_class as gc;
use wirescript::ir::port_registry::WirePort;
use wirescript::ir::{Literal, Module, Node, NodeId};

/// Each fixture and the structural check for the miscompile it guards.
const CHECKS: &[(&str, fn(&Fixture<'_>))] = &[
    ("game_enum_ports.ws", enum_pins_carry_variant_tags),
    (
        "generic_nesting.ws",
        nested_chips_are_typed_per_instantiation,
    ),
    ("race_sort.ws", sort_carries_every_record_column),
    ("record_assign.ws", record_assign_reads_before_it_writes),
    ("record_columns.ws", record_columns_stay_distinct),
];

const MODES: [(FoldMode, &str); 2] = [
    (FoldMode::ForceOff, "fold off"),
    (FoldMode::Auto, "fold auto"),
];

fn file_name(path: &std::path::Path) -> &str {
    path.file_name().unwrap().to_str().unwrap()
}

struct Fixture<'a> {
    name: &'a str,
    mode: &'a str,
    src: &'a str,
    module: &'a Module,
}

impl Fixture<'_> {
    /// The 1-based line holding `needle`, which must appear on exactly one line.
    fn line_of(&self, needle: &str) -> u32 {
        let lines: Vec<u32> = self
            .src
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(needle))
            .map(|(i, _)| i as u32 + 1)
            .collect();
        assert_eq!(
            lines.len(),
            1,
            "{}: `{needle}` must appear on exactly one line",
            self.name
        );
        lines[0]
    }

    fn ctx(&self) -> String {
        format!("{} ({})", self.name, self.mode)
    }
}

/// Resolve, typecheck and lower, asserting no diagnostic of any severity.
fn lower_clean(path: &std::path::Path, src: &str, mode: FoldMode, label: &str) -> Module {
    let file = path.to_str().unwrap();
    let (resolved, tc, lowered) = common::run_stages(src, file, mode);
    let diagnostics: Vec<String> = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .chain(lowered.diagnostics.iter())
        .map(|d| {
            format!(
                "{:?} [{}] {} at {}:{}",
                d.severity, d.code, d.message, d.range.start.line, d.range.start.col
            )
        })
        .collect();
    assert!(
        diagnostics.is_empty(),
        "{} ({label}) must compile with no diagnostics, got {diagnostics:#?}",
        file_name(path)
    );
    lowered.module
}

// IR helpers

fn modules(m: &Module) -> Vec<&Module> {
    let mut out = Vec::new();
    m.walk(&mut |_, sub| out.push(sub));
    out
}

fn prop<'a>(n: &'a Node, key: &str) -> Option<&'a Literal> {
    n.properties
        .iter()
        .find(|(k, _)| resolve(**k) == key)
        .map(|(_, v)| v)
}

fn prop_str<'a>(n: &'a Node, key: &str) -> Option<&'a str> {
    match prop(n, key) {
        Some(Literal::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// The storage name the lowering gives a var or column (`rows.pts`).
fn label(n: &Node) -> &str {
    prop_str(n, "_label").unwrap_or("")
}

/// The node wired into `(id, port)` in `m`.
fn driver(m: &Module, id: NodeId, port: WirePort) -> Option<&Node> {
    m.wires
        .iter()
        .find(|w| w.target.node_id == id && w.target.port == port)
        .and_then(|w| m.nodes.get(&w.source.node_id))
}

fn driver_label(m: &Module, id: NodeId, port: WirePort) -> String {
    driver(m, id, port)
        .map(label)
        .unwrap_or("<unwired>")
        .to_string()
}

fn of_class<'a>(m: &'a Module, class: &str) -> Vec<&'a Node> {
    let mut v: Vec<&Node> = m.nodes.values().filter(|n| n.gate_class == class).collect();
    v.sort_by_key(|n| n.id.0);
    v
}

fn on_line<'a>(m: &'a Module, class: &str, line: u32) -> Vec<&'a Node> {
    of_class(m, class)
        .into_iter()
        .filter(|n| n.source_range.start.line == line)
        .collect()
}

/// Whether `to` runs after `from` on an exec chain of `m`. An exec edge is a
/// wire into an `Exec*` input.
fn exec_reaches(m: &Module, from: NodeId, to: NodeId) -> bool {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([from]);
    while let Some(id) = queue.pop_front() {
        for w in m.wires.iter().filter(|w| w.source.node_id == id) {
            if !format!("{:?}", w.target.port).starts_with("Exec") {
                continue;
            }
            let next = w.target.node_id;
            if next == to {
                return true;
            }
            if seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    false
}

/// An integer constant driving a pin: a `_Literal`, or a constant materialized
/// as `A + B` for a target that takes no baked value.
fn const_int(n: &Node) -> Option<i64> {
    let int = |key: &str| match prop(n, key) {
        Some(Literal::Int(v)) => Some(*v),
        _ => None,
    };
    if n.gate_class == gc::LITERAL {
        return n.properties.values().find_map(|v| match v {
            Literal::Int(v) => Some(*v),
            _ => None,
        });
    }
    if n.gate_class.ends_with("Expr_MathAdd") {
        return Some(int("InputA")? + int("InputB").unwrap_or(0));
    }
    None
}

const PARALLEL: [WirePort; 7] = [
    WirePort::ArrayVarRef2,
    WirePort::ArrayVarRef3,
    WirePort::ArrayVarRef4,
    WirePort::ArrayVarRef5,
    WirePort::ArrayVarRef6,
    WirePort::ArrayVarRef7,
    WirePort::ArrayVarRef8,
];

/// A sort gate as (key column, parallel columns in port order).
fn sort_columns(m: &Module, n: &Node) -> (String, Vec<String>) {
    let key = driver_label(m, n.id, WirePort::ArrayVarRef);
    let parallel = PARALLEL
        .iter()
        .filter_map(|p| driver(m, n.id, *p).map(|d| label(d).to_string()))
        .collect();
    (key, parallel)
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(std::string::ToString::to_string).collect()
}

// Structural checks, one per fixture

/// Both `sortMultiple` calls sort `keys` and carry `order`, all three `rows`
/// columns and `uids`, one array per pin. The record array must not be wired
/// into `bDescending`, which would leave no record column on a parallel pin.
fn sort_carries_every_record_column(f: &Fixture<'_>) {
    let m = f.module;
    let sorts = of_class(m, gc::ARRAY_SORT_MULTIPLE);
    assert_eq!(sorts.len(), 2, "{}: two sortMultiple gates", f.ctx());
    let want = set(&["order", "rows.bonus", "rows.name", "rows.pts", "uids"]);
    for s in &sorts {
        let (key, parallel) = sort_columns(m, s);
        assert_eq!(key, "keys", "{}: the sort key", f.ctx());
        let got: BTreeSet<String> = parallel.iter().cloned().collect();
        assert_eq!(
            got.len(),
            parallel.len(),
            "{}: a column wired twice: {parallel:?}",
            f.ctx()
        );
        assert_eq!(
            got,
            want,
            "{}: every parallel array and record column is carried",
            f.ctx()
        );
        let into_desc: Vec<&str> = m
            .wires
            .iter()
            .filter(|w| w.target.node_id == s.id && w.target.port == WirePort::BDescending)
            .filter_map(|w| m.nodes.get(&w.source.node_id))
            .map(|n| n.gate_class)
            .collect();
        assert!(
            !into_desc.contains(&gc::ARRAY_VAR),
            "{}: an array reached bDescending: {into_desc:?}",
            f.ctx()
        );
    }
    let descending = |line: u32| {
        let s = on_line(m, gc::ARRAY_SORT_MULTIPLE, line);
        assert_eq!(s.len(), 1, "{}: one sort on line {line}", f.ctx());
        prop(s[0], "bDescending").cloned()
    };
    assert_eq!(
        descending(f.line_of("keys.sortMultiple(order, rows, uids, true)")),
        Some(Literal::Bool(true)),
        "{}: `true` sets bDescending",
        f.ctx()
    );
    assert_eq!(
        descending(f.line_of("keys.sortMultiple(order, rows, uids, false)")),
        Some(Literal::Bool(false)),
        "{}: `false` clears bDescending",
        f.ctx()
    );
}

/// Every field of `h` and `h2` is its own column, sorting by a column drags
/// exactly its siblings, sorting a plain array field drags nothing, and the
/// spread copies each unnamed column of `h` into the same column of `h2`.
fn record_columns_stay_distinct(f: &Fixture<'_>) {
    let m = f.module;
    let mut columns: Vec<&str> = of_class(m, gc::ARRAY_VAR)
        .into_iter()
        .chain(of_class(m, gc::MAP_VAR))
        .map(label)
        .collect();
    columns.sort_unstable();
    let unique: BTreeSet<&str> = columns.iter().copied().collect();
    assert_eq!(
        unique.len(),
        columns.len(),
        "{}: a column is declared twice: {columns:?}",
        f.ctx()
    );
    for c in [
        "h.pts.x", "h.pts.y", "h.xs", "h.ys", "h.m.x", "h.m.y", "h2.pts.x", "h2.pts.y", "h2.xs",
        "h2.ys", "h2.m.x", "h2.m.y", "qs.p.x", "qs.p.y", "qs.k",
    ] {
        assert!(
            unique.contains(c),
            "{}: missing column {c}: {columns:?}",
            f.ctx()
        );
    }

    let multi: BTreeSet<(String, BTreeSet<String>)> = of_class(m, gc::ARRAY_SORT_MULTIPLE)
        .into_iter()
        .map(|n| {
            let (key, parallel) = sort_columns(m, n);
            (key, parallel.into_iter().collect())
        })
        .collect();
    let want: BTreeSet<(String, BTreeSet<String>)> = [
        ("h.pts.y".to_string(), set(&["h.pts.x"])),
        ("qs.p.x".to_string(), set(&["qs.k", "qs.p.y"])),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        multi,
        want,
        "{}: sorting by a column carries exactly its siblings",
        f.ctx()
    );
    let single: Vec<String> = of_class(m, gc::ARRAY_SORT)
        .into_iter()
        .map(|n| sort_columns(m, n).0)
        .collect();
    assert_eq!(
        single,
        vec!["h.xs".to_string()],
        "{}: `h.xs.sort()` sorts one array",
        f.ctx()
    );

    let copies: BTreeSet<(String, String)> = of_class(m, gc::ARRAY_COPY_FROM)
        .into_iter()
        .map(|n| {
            (
                driver_label(m, n.id, WirePort::SourceRef),
                driver_label(m, n.id, WirePort::ArrayVarRef),
            )
        })
        .chain(of_class(m, gc::MAP_COPY_FROM).into_iter().map(|n| {
            (
                driver_label(m, n.id, WirePort::SourceRef),
                driver_label(m, n.id, WirePort::MapVarRef),
            )
        }))
        .collect();
    let want: BTreeSet<(String, String)> =
        [("h.ys", "h2.ys"), ("h.m.x", "h2.m.x"), ("h.m.y", "h2.m.y")]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
    assert_eq!(
        copies,
        want,
        "{}: the spread copies each column into the same column",
        f.ctx()
    );
}

/// The swap reads both fields before it writes either, the popped value is
/// popped once, the rebuild reads the old length before it clears, and each
/// record local's initializer writes every field at each place it runs.
fn record_assign_reads_before_it_writes(f: &Fixture<'_>) {
    let m = f.module;
    let var_of = |n: &Node| driver_label(m, n.id, WirePort::VarRef);

    let swap = f.line_of("p = { x: p.y, y: p.x }");
    let gets = on_line(m, gc::VAR_GET, swap);
    let sets = on_line(m, gc::VAR_SET, swap);
    assert_eq!(
        (gets.len(), sets.len()),
        (2, 2),
        "{}: the swap is two reads and two writes",
        f.ctx()
    );
    for g in &gets {
        for s in &sets {
            assert!(
                exec_reaches(m, g.id, s.id),
                "{}: a swap write runs before a read",
                f.ctx()
            );
        }
    }
    for s in &sets {
        let value = driver(m, s.id, WirePort::Value)
            .unwrap_or_else(|| panic!("{}: swap write unwired", f.ctx()));
        let pair = (var_of(s), var_of(value));
        assert!(
            pair == ("p.x".into(), "p.y".into()) || pair == ("p.y".into(), "p.x".into()),
            "{}: each swapped field is written from the other's read, got {pair:?}",
            f.ctx()
        );
    }

    let pop_line = f.line_of("p = { x: a.pop(), y: 0 }");
    let pops = on_line(m, gc::ARRAY_POP, pop_line);
    assert_eq!(
        pops.len(),
        1,
        "{}: the call in a field is lowered once",
        f.ctx()
    );
    let set_x: Vec<&Node> = on_line(m, gc::VAR_SET, pop_line)
        .into_iter()
        .filter(|s| var_of(s) == "p.x")
        .collect();
    assert_eq!(set_x.len(), 1, "{}: one write of p.x from the pop", f.ctx());
    assert!(
        exec_reaches(m, pops[0].id, set_x[0].id),
        "{}: the pop runs before its write",
        f.ctx()
    );

    let rebuild = f.line_of("h = { xs: [9], n: h.xs.length() }");
    let lengths = on_line(m, gc::ARRAY_GET_LENGTH, rebuild);
    let clears = on_line(m, gc::ARRAY_CLEAR, rebuild);
    assert_eq!(
        lengths.len(),
        1,
        "{}: one length read in the rebuild",
        f.ctx()
    );
    assert!(
        !clears.is_empty(),
        "{}: the rebuild clears the old contents",
        f.ctx()
    );
    for c in &clears {
        assert!(
            exec_reaches(m, lengths[0].id, c.id),
            "{}: the rebuild clears before it reads",
            f.ctx()
        );
    }

    for (decl, fields, places) in [
        ("var local: P = { x: 1, y: 2 }", ["local.x", "local.y"], 2),
        ("var fresh: P", ["fresh.x", "fresh.y"], 2),
        ("var q: P = { x: 5, y: 0 }", ["q.x", "q.y"], 1),
    ] {
        let line = f.line_of(decl);
        let mut seen = 0;
        for sub in modules(m) {
            let written: BTreeSet<String> = on_line(sub, gc::VAR_SET, line)
                .into_iter()
                .map(|s| driver_label(sub, s.id, WirePort::VarRef))
                .collect();
            if written.is_empty() {
                continue;
            }
            seen += 1;
            assert_eq!(
                written,
                set(&fields),
                "{}: `{decl}` writes every field where it runs",
                f.ctx()
            );
        }
        assert_eq!(
            seen,
            places,
            "{}: `{decl}` is initialized at each of its {places} call sites",
            f.ctx()
        );
    }
}

/// `inner` and `deep` each exist once per instantiation of their enclosing
/// generics, one typed int and one typed string, and each passes its `x`
/// straight to `y`. The shadowing generic's call lands once per `shadow` copy.
fn nested_chips_are_typed_per_instantiation(f: &Fixture<'_>) {
    let port_type = |n: &Node| {
        n.ports
            .outputs
            .first()
            .map(|p| p.ty.to_string())
            .unwrap_or_default()
    };
    for chip in ["inner", "deep"] {
        let bodies: Vec<&Module> = modules(f.module)
            .into_iter()
            .filter(|c| {
                resolve(c.name)
                    .strip_prefix(chip)
                    .is_some_and(|r| r.starts_with('_'))
            })
            .collect();
        let mut types = BTreeSet::new();
        for body in &bodies {
            let find = |ids: &[NodeId], name: &str| {
                ids.iter()
                    .map(|id| &body.nodes[id])
                    .find(|n| prop_str(n, "PortLabel") == Some(name))
                    .unwrap_or_else(|| panic!("{}: `{chip}` has no pin {name}", f.ctx()))
            };
            let x = find(&body.inputs, "x");
            let y = find(&body.outputs, "y");
            assert_eq!(
                port_type(x),
                port_type(y),
                "{}: `{chip}` x and y agree",
                f.ctx()
            );
            assert!(
                body.wires
                    .iter()
                    .any(|w| w.source.node_id == x.id && w.target.node_id == y.id),
                "{}: `{chip}` passes x to y",
                f.ctx()
            );
            types.insert(port_type(x));
        }
        assert_eq!(
            bodies.len(),
            2,
            "{}: `{chip}` once per instantiation",
            f.ctx()
        );
        assert_eq!(
            types,
            set(&["int", "string"]),
            "{}: `{chip}` typed per instantiation",
            f.ctx()
        );
    }
    let shadow = of_class(f.module, gc::VAR)
        .into_iter()
        .find(|n| label(n) == "shadowB")
        .unwrap_or_else(|| panic!("{}: no shadowB var", f.ctx()));
    let writes = of_class(f.module, gc::VAR_SET)
        .into_iter()
        .filter(|s| driver(f.module, s.id, WirePort::VarRef).map(|n| n.id) == Some(shadow.id))
        .count();
    assert_eq!(
        writes,
        2,
        "{}: `shadowB = B(9)` once per `shadow` instantiation",
        f.ctx()
    );
}

/// Each enum `out` pin is an int pin driven by its variant's tag: the default
/// ones by the constant tag, the emitted ones by a backing var that starts at the
/// default tag and is written by the emit. Each enum `in` pin reaches a reader.
fn enum_pins_carry_variant_tags(f: &Fixture<'_>) {
    let m = f.module;
    let pin = |ids: &[NodeId], name: &str| -> NodeId {
        *ids.iter()
            .find(|id| prop_str(&m.nodes[*id], "PortLabel") == Some(name))
            .unwrap_or_else(|| panic!("{}: no pin {name}", f.ctx()))
    };
    let tag =
        |member: &str| wirescript::catalog::enum_member_value("EBREasingFunction", member).unwrap();

    for (name, want) in [("easeOut", tag("Bounce")), ("hueOut", 3)] {
        let id = pin(&m.outputs, name);
        let d = driver(m, id, WirePort::RerInput)
            .unwrap_or_else(|| panic!("{}: {name} unwired", f.ctx()));
        assert_eq!(
            const_int(d),
            Some(want),
            "{}: {name} carries the default's tag",
            f.ctx()
        );
    }
    for (name, default) in [("easeLive", tag("Sine")), ("hueLive", 1)] {
        let id = pin(&m.outputs, name);
        let d = driver(m, id, WirePort::RerInput)
            .unwrap_or_else(|| panic!("{}: {name} unwired", f.ctx()));
        assert_eq!(
            d.gate_class,
            gc::VAR,
            "{}: {name} is backed by a var",
            f.ctx()
        );
        assert_eq!(
            prop(d, "InitialValue"),
            Some(&Literal::Int(default)),
            "{}: {name} starts at its default",
            f.ctx()
        );
        let writes = of_class(m, gc::VAR_SET)
            .into_iter()
            .filter(|s| driver(m, s.id, WirePort::VarRef).map(|n| n.id) == Some(d.id))
            .count();
        assert_eq!(writes, 1, "{}: the emit writes {name} once", f.ctx());
    }
    for id in m.inputs.iter().chain(m.outputs.iter()) {
        let ty: Vec<String> = m.nodes[id]
            .ports
            .outputs
            .iter()
            .map(|p| p.ty.to_string())
            .collect();
        assert_eq!(
            ty,
            vec!["int".to_string()],
            "{}: an enum pin carries the int tag",
            f.ctx()
        );
    }
    for name in ["easeIn", "hueIn", "easeLiveIn", "hueLiveIn"] {
        let id = pin(&m.inputs, name);
        assert!(
            m.wires.iter().any(|w| w.source.node_id == id),
            "{}: {name} reaches a reader",
            f.ctx()
        );
    }
}

// Tests

#[test]
fn every_fixture_has_a_structural_check() {
    let found: BTreeSet<String> = common::ws_fixtures("silent")
        .iter()
        .map(|p| file_name(p).to_string())
        .collect();
    let checked: BTreeSet<String> = CHECKS.iter().map(|(n, _)| n.to_string()).collect();
    assert_eq!(
        found, checked,
        "every fixture needs a row in CHECKS, and every row a fixture"
    );
}

#[test]
fn each_silent_fixture_keeps_the_structure_its_miscompile_breaks() {
    for path in common::ws_fixtures("silent") {
        let name = file_name(&path);
        let Some((_, check)) = CHECKS.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        let src = std::fs::read_to_string(&path).unwrap();
        for (mode, label) in MODES {
            let module = lower_clean(&path, &src, mode, label);
            check(&Fixture {
                name,
                mode: label,
                src: &src,
                module: &module,
            });
        }
    }
}
