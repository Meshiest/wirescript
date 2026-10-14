//! An `emit` and a `return` in a block arm of a value `match` behave the same
//! in every expression position as the same `match` written as a statement
//! just ahead of the statement: the arm's write is conditional (the output is
//! stored, never driven straight), the arm leaves the handler or mod (nothing
//! after it runs on that path), and a mod's `return` from the arm goes on to
//! the caller. Every position the canonical traversal (`ast::walk`) lists is
//! here, in a top-level handler, a chip's handler and a mod.

use super::*;
use crate::ir::gate_class as gc;

/// The `match` the value form puts in the position: arm `A` writes `o` and
/// leaves, arm `B` gives the position's value.
const VALUE_MATCH: &str = "match k { A => {\nemit o = 77\nemit o2 = 78\nreturn }, B => 3 }";
/// The same `match` as a statement, for the plain form.
const STMT_MATCH: &str = "match k { A => {\nemit o = 77\nemit o2 = 78\nreturn } B => { } }";

struct Position {
    what: &'static str,
    /// The statement with `@` for the position.
    value: &'static str,
    /// The plain form: `#` is where the statement `match` goes, `@` the
    /// position, holding the arm's value. `None` puts the `match` ahead of
    /// `value`.
    plain: Option<&'static str>,
    contexts: &'static [Ctx],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    Handler,
    ChipHandler,
    Mod,
}

const ALL: &[Ctx] = &[Ctx::Handler, Ctx::ChipHandler, Ctx::Mod];

const POSITIONS: &[Position] = &[
    Position { what: "let value", value: "let v = @\nn = v", plain: None, contexts: ALL },
    Position { what: "var initializer", value: "var w: int = @\nn = w", plain: None, contexts: ALL },
    Position { what: "assignment value", value: "n = @", plain: None, contexts: ALL },
    Position { what: "assignment index", value: "xs[@] = 1", plain: None, contexts: ALL },
    Position { what: "emit value", value: "emit o2 = @", plain: None, contexts: ALL },
    Position { what: "out value", value: "out o3 = @", plain: None, contexts: &[Ctx::Handler] },
    Position { what: "return value", value: "return { o: 0, o2: @ }", plain: None, contexts: &[Ctx::Mod] },
    Position { what: "if condition", value: "if @ > 0 {\nn = 6\n}", plain: None, contexts: ALL },
    Position { what: "for range end", value: "for i in 0..@ {\nn = n + 1\n}", plain: None, contexts: ALL },
    Position { what: "for range start", value: "for i in @..9 {\nn = n + 1\n}", plain: None, contexts: ALL },
    Position { what: "for step", value: "for i in (0..9).step(@) {\nn = n + 1\n}", plain: None, contexts: ALL },
    Position { what: "for count", value: "for i in count(@) {\nn = n + 1\n}", plain: None, contexts: ALL },
    Position { what: "if let scrutinee", value: "if let A = toK(@) {\nn = 6\n}", plain: None, contexts: ALL },
    Position {
        what: "let else scrutinee",
        value: "let Hit(x) = toP(@) else {\nemit o = 1\nemit o2 = 2\nreturn\n}\nn = x",
        plain: None,
        contexts: ALL,
    },
    Position { what: "await value", value: "let v = await (@) on sig\nn = v", plain: None, contexts: &[Ctx::Handler] },
    Position { what: "array initializer", value: "var ys: int[] = [@, 2]\nn = ys.length()", plain: None, contexts: ALL },
    Position { what: "call argument", value: "n = id(@)", plain: None, contexts: ALL },
    Position { what: "method argument", value: "xs.push(@)", plain: None, contexts: ALL },
    Position { what: "record field", value: "let r = { a: @, b: 2 }\nn = r.a", plain: None, contexts: ALL },
    Position { what: "tuple element", value: "let t = (@, 2)\nn = t.0", plain: None, contexts: ALL },
    Position { what: "interpolation", value: "s = \"v=${@}\"", plain: None, contexts: ALL },
    Position { what: "operator left", value: "n = @ + 1", plain: None, contexts: ALL },
    Position { what: "operator right", value: "n = 1 + @", plain: None, contexts: ALL },
    Position { what: "unary operand", value: "n = -(@)", plain: None, contexts: ALL },
    Position { what: "index read", value: "n = xs[@]", plain: None, contexts: ALL },
    Position { what: "if-expression condition", value: "n = if @ > 0 then 1 else 2", plain: None, contexts: ALL },
    Position {
        what: "if-expression branch",
        value: "n = if c then @ else 2",
        plain: Some("if c {\n#\n}\nn = if c then @ else 2"),
        contexts: ALL,
    },
    Position {
        what: "match arm value",
        value: "n = match k2 { A => @, B => 2 }",
        plain: Some("match k2 { A => {\n#\n} B => { } }\nn = match k2 { A => @, B => 2 }"),
        contexts: ALL,
    },
    Position { what: "block value", value: "let bv = {\nlet t = 1\nt + @\n}\nn = bv", plain: None, contexts: ALL },
    Position { what: "nested match scrutinee", value: "n = match toK(@) { A => 1, B => 2 }", plain: None, contexts: ALL },
];

/// The source for `body` in `ctx`, with a sentinel `n = 5` after it unless
/// `body` ends the mod with its own `return`.
fn program(ctx: Ctx, body: &str, ends_with_return: bool) -> String {
    let tail = if ends_with_return {
        ""
    } else {
        "\nn = 5\nemit o = 3\nemit o2 = 4"
    };
    let lib = "enum K { A, B }\nin go: exec\nin sig: exec\nvar m: int = 0\nvar m2: int = 0\n\
               mod id(x: int) -> int { return x }\n\
               mod toK(i: int) -> K { return if i > 0 then K.A else K.B }\n\
               enum P { Hit(int), Miss }\nmod toP(i: int) -> P { return if i > 0 then P.Hit(i) else P.Miss }\n";
    let locals = "var k: K = K.A\nvar k2: K = K.A\nvar c: bool = true\nvar n: int = 0\nvar s: string = \"\"\nvar xs: int[]\n";
    match ctx {
        Ctx::Handler => format!(
            "{lib}{locals}out o: int\nout o2: int\nout on_: int = n\nout os: string = s\non go {{\n{body}\nn = 5\n}}\n"
        ),
        Ctx::ChipHandler => format!(
            "{lib}chip C(t: exec, sig: exec) -> (o: int, o2: int) {{\n{locals}on t {{\n{body}\nn = 5\nemit o2 = 4\n}}\n}}\n\
             let r = C(go, sig)\nout ro = r.o\nout ro2 = r.o2\n"
        ),
        Ctx::Mod => format!(
            "{lib}var gk: K = K.A\nmod f(k: K) -> (o: int, o2: int) {{\n{}{body}{tail}\n}}\n\
             on go {{\nm = f(gk).o\nm2 = 7\n}}\nout om: int = m\nout om2: int = m2\n",
            locals.replacen("var k: K = K.A\n", "", 1)
        ),
    }
}

fn line_of(src: &str, text: &str) -> u32 {
    let idx = src.lines().position(|l| l.trim() == text).unwrap_or_else(|| panic!("no line `{text}` in\n{src}"));
    idx as u32 + 1
}

/// Whether some exec root reaches `to` when the exec wires out of `cut` are
/// removed. A root is a gate no exec wire enters.
fn reached_without(m: &crate::ir::Module, cut: Option<crate::ir::PortRef>, to: crate::ir::NodeId) -> bool {
    let is_exec = |w: &&crate::ir::Wire| w.target.port.as_str().starts_with("Exec");
    let entered: crate::collections::HashSet<_> = m.wires.iter().filter(is_exec).map(|w| w.target.node_id).collect();
    let mut stack: Vec<_> = m.nodes.keys().filter(|id| !entered.contains(*id)).copied().collect();
    let mut seen = crate::collections::HashSet::default();
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if seen.insert(n) {
            for w in m.wires.iter().filter(is_exec) {
                if w.source.node_id == n && Some(&w.source) != cut.as_ref() {
                    stack.push(w.target.node_id);
                }
            }
        }
    }
    false
}

/// Whether gate `id` is reached from `port` of `from` along exec wires.
fn reached_from(m: &crate::ir::Module, from: crate::ir::PortRef, id: crate::ir::NodeId) -> bool {
    m.wires
        .iter()
        .filter(|w| w.source == from && w.target.port.as_str().starts_with("Exec"))
        .any(|w| exec_reaches(m, w.target.node_id, id))
}

/// The Branch output every exec path to `store` takes, when there is one
/// whose other output does not lead to `store`: the store runs on that branch
/// only, never on the untaken side.
fn deciding_branch(m: &crate::ir::Module, store: crate::ir::NodeId) -> Option<crate::ir::PortRef> {
    let reached = reached_without(m, None, store);
    assert!(reached, "the store is reached from a root");
    m.nodes.values().filter(|n| n.gate_class == gc::BRANCH).find_map(|b| {
        [(WirePort::ExecOutA, WirePort::ExecOutB), (WirePort::ExecOutB, WirePort::ExecOutA)]
            .into_iter()
            .find(|(taken, other)| {
                !reached_without(m, Some(b.id.port(*taken)), store) && !reached_from(m, b.id.port(*other), store)
            })
            .map(|(taken, _)| b.id.port(taken))
    })
}

/// The other output of the Branch output `taken`.
fn other_side(taken: &crate::ir::PortRef) -> crate::ir::PortRef {
    let other = if taken.port == WirePort::ExecOutA { WirePort::ExecOutB } else { WirePort::ExecOutA };
    taken.node_id.port(other)
}

/// The stores on `line` of the module.
fn stores_on(m: &crate::ir::Module, line: u32) -> Vec<crate::ir::NodeId> {
    m.nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_SET && n.source_range.start.line == line)
        .map(|n| n.id)
        .collect()
}

/// Whether output `name` is driven once, by its backing var.
fn backed(m: &crate::ir::Module, name: &str) -> bool {
    let out = m.nodes.iter().find(|(_, n)| {
        n.gate_class == gc::MICROCHIP_OUTPUT
            && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == name)
    });
    let Some((id, _)) = out else { return false };
    let drivers: Vec<_> = m.wires.iter().filter(|w| w.target.node_id == *id && w.target.port == WirePort::RerInput).collect();
    drivers.len() == 1 && m.nodes[&drivers[0].source.node_id].note == Some("out_backing")
}

/// What the arm's write and exit do, read off the lowered program.
#[derive(Debug, PartialEq, Eq)]
struct Shape {
    /// `o` is driven by its stored value (handlers), or the caller reads it
    /// from the mod's storage (mods).
    stored: bool,
    /// Nothing after the position runs on the arm's path.
    arm_leaves: bool,
    /// The arm's `return` goes on to the caller (mods).
    reaches_caller: bool,
    /// The arm's stores of `o` and `o2` run on one Branch output only, and
    /// the other output does not reach them.
    arm_on_its_branch: bool,
}

fn shape(ctx: Ctx, src: &str, has_sentinel: bool) -> Shape {
    let r = compile(src);
    let errors: Vec<_> = r.diagnostics.iter().filter(|d| d.severity == crate::diagnostic::Severity::Error).collect();
    assert!(errors.is_empty(), "{errors:?}
{src}");
    let m = match ctx {
        Ctx::ChipHandler => only_chip(&r, "the chip"),
        _ => &r.module,
    };
    let arm: Vec<_> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_SET && stores_value(m, n.id, 77))
        .map(|n| n.id)
        .collect();
    assert_eq!(arm.len(), 1, "the arm's store\n{src}");
    let arm = arm[0];
    let o2: Vec<_> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_SET && stores_value(m, n.id, 78))
        .map(|n| n.id)
        .collect();
    assert_eq!(o2.len(), 1, "the arm's `o2` store\n{src}");
    let arm_on_its_branch = deciding_branch(m, arm)
        .is_some_and(|b| !reached_without(m, Some(b.clone()), o2[0]) && !reached_from(m, other_side(&b), o2[0]));
    let arm_leaves = !has_sentinel || stores_on(m, line_of(src, "n = 5")).iter().all(|s| !exec_reaches(m, arm, *s));
    let (stored, reaches_caller) = match ctx {
        Ctx::Mod => {
            let store_m = stores_on(m, line_of(src, "m = f(gk).o"));
            let from_storage = store_m.iter().all(|s| {
                m.wires
                    .iter()
                    .find(|w| w.target.node_id == *s && w.target.port == WirePort::Value)
                    .is_some_and(|w| m.nodes[&w.source.node_id].gate_class == gc::VAR_GET)
            });
            let cont = stores_on(m, line_of(src, "m2 = 7"));
            (from_storage, cont.iter().any(|c| exec_reaches(m, arm, *c)))
        }
        _ => (backed(m, "o"), false),
    };
    Shape { stored, arm_leaves, reaches_caller, arm_on_its_branch }
}

#[test]
fn a_returning_match_arm_behaves_alike_in_every_position() {
    let mut checked = 0;
    for p in POSITIONS {
        for &ctx in p.contexts {
            let value_body = p.value.replace('@', VALUE_MATCH);
            let plain_body = match p.plain {
                Some(t) => t.replace('#', STMT_MATCH).replace('@', "3"),
                None => format!("{STMT_MATCH}\n{}", p.value.replace('@', "3")),
            };
            let has_sentinel = !p.value.starts_with("return");
            let (value_src, plain_src) = (program(ctx, &value_body, !has_sentinel), program(ctx, &plain_body, !has_sentinel));
            let plain = shape(ctx, &plain_src, has_sentinel);
            let value = shape(ctx, &value_src, has_sentinel);
            assert_eq!(value, plain, "{} in {ctx:?}\n--- value form\n{value_src}\n--- plain form\n{plain_src}", p.what);
            let want = Shape { stored: true, arm_leaves: true, reaches_caller: ctx == Ctx::Mod, arm_on_its_branch: true };
            assert_eq!(plain, want, "{} in {ctx:?}\n{plain_src}", p.what);
            checked += 1;
        }
    }
    assert!(checked >= 80, "only {checked} combinations checked");
}
