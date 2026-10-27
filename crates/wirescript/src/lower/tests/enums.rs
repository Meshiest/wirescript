use super::*;
use crate::ir::Literal;

// The placeholder gate class is `gc::UNSUPPORTED` (`"_Unsupported"`); a
// prefixed spelling like `BrickComponentType_Unsupported` matches no gate, so
// a check against it would pass even when a construction fell back to one.

/// The `InitialValue` baked into the `Pseudo_Var` whose `_label` `NAME_LABEL` is
/// exactly `label` (an enum var's `__disc`/slot gates are labelled
/// `"{var}.__disc"` / `"{var}.__{Variant}_{slot}"` by `declare_enum_container`).
/// `None` if no such gate exists or it carries no `InitialValue`. This locks in
/// the actual baked tag/payload value, not merely the gate count: baking
/// `__disc = 0` for every variant still emits the right number of `Pseudo_Var`s
/// and would pass a count-only check.
fn baked_init(r: &LowerResult, label: &str) -> Option<Literal> {
    r.module
        .nodes_deep()
        .filter(|n| n.gate_class == gc::VAR)
        .find(|n| matches!(n.properties.get(&crate::intern::sym::NAME_LABEL), Some(Literal::String(s)) if s == label))
        .and_then(|n| n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned())
}

/// How many `class` gates carry a `NAME_LABEL` of exactly `label`.
fn gate_count_labelled(r: &LowerResult, class: &str, label: &str) -> usize {
    r.module
        .nodes_deep()
        .filter(|n| n.gate_class == class)
        .filter(|n| matches!(n.properties.get(&crate::intern::sym::NAME_LABEL), Some(Literal::String(s)) if s == label))
        .count()
}

/// Every gate class in `r` (nested chips included), by count.
fn gate_class_counts(r: &LowerResult) -> std::collections::BTreeMap<String, usize> {
    let mut acc = std::collections::BTreeMap::new();
    for n in r.module.nodes_deep() {
        *acc.entry(n.gate_class.to_string()).or_default() += 1;
    }
    acc
}

/// Every `Float` value baked into ANY gate property in `r` (a bare `_Literal`
/// gate, or a value the literal-inlining pass folded into a consumer gate's
/// data field - e.g. a constant output materialized as `MathAdd(value, 0.0)`).
fn float_operands(r: &LowerResult) -> Vec<f64> {
    r.module
        .nodes_deep()
        .flat_map(|n| n.properties.values())
        .filter_map(|l| match l {
            Literal::Float(f) => Some(*f),
            _ => None,
        })
        .collect()
}

/// Every `Int` value baked into ANY gate property in `r`, wherever the
/// literal-inlining pass put it.
fn int_operands(r: &LowerResult) -> Vec<i64> {
    r.module
        .nodes_deep()
        .flat_map(|n| n.properties.values())
        .filter_map(|l| match l {
            Literal::Int(i) => Some(*i),
            _ => None,
        })
        .collect()
}

/// What one enum row must show beyond lowering clean (no error, no WSP001, no
/// `_Unsupported` anywhere). Counts are over the whole module tree.
#[derive(Default)]
struct Expect<'a> {
    /// `(class, n)`: exactly `n` gates of the class.
    counts: &'a [(&'a str, usize)],
    /// Classes with at least one gate.
    present: &'a [&'a str],
    /// `(label, value)`: what the var labelled so bakes (`None`: nothing).
    baked: Vec<(&'a str, Option<Literal>)>,
    /// `(label, n)`: exactly `n` vars carry the label.
    labelled: &'a [(&'a str, usize)],
    /// `(label, n)`: exactly `n` stores write the var labelled so.
    stores: &'a [(&'a str, usize)],
    /// `(note, n)`: exactly `n` gates carry the note.
    notes: &'a [(&'a str, usize)],
    /// The sorted labels of every var some `Var_Set` writes, exactly.
    writes: Option<&'a [&'a str]>,
    /// Integers and floats some gate property bakes.
    ints: &'a [i64],
    floats: &'a [f64],
}

/// A `(label, fold mode, source, expectation)` row.
type EnumRow<'a> = (&'a str, FoldMode, &'a str, Expect<'a>);

/// Every row's failures, a row that panics included, then one report.
#[track_caller]
fn check_enum_rows(rows: &[EnumRow<'_>]) {
    let mut failures = Vec::new();
    for (label, mode, src, want) in rows {
        let row = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut bad = Vec::new();
            let r = lower_with(src, "test", *mode, &crate::resolve::FsLoader);
            let codes = crate::test_support::error_codes(&r.diagnostics);
            if !codes.is_empty()
                || r.diagnostics.iter().any(|d| d.code == "WSP001")
                || has_unsupported(&r.module)
            {
                bad.push(format!("not clean: {:?}", r.diagnostics));
                return bad;
            }
            for (class, n) in want.counts {
                let got = count_class(&r.module, class);
                if got != *n {
                    bad.push(format!("{got} x {class}, want {n}"));
                }
            }
            for class in want.present {
                if count_class(&r.module, class) == 0 {
                    bad.push(format!("no {class}"));
                }
            }
            for (var, value) in &want.baked {
                let got = baked_init(&r, var);
                if got != *value {
                    bad.push(format!("{var} bakes {got:?}, want {value:?}"));
                }
            }
            for (var, n) in want.labelled {
                let got = gate_count_labelled(&r, gc::VAR, var);
                if got != *n {
                    bad.push(format!("{got} vars labelled {var}, want {n}"));
                }
            }
            for (var, n) in want.stores {
                let got =
                    super::generics::stored_values(&r, super::generics::var_labelled(&r, var))
                        .len();
                if got != *n {
                    bad.push(format!("{got} stores into {var}, want {n}"));
                }
            }
            for (note, n) in want.notes {
                let got = r
                    .module
                    .nodes_deep()
                    .filter(|g| g.note == Some(*note))
                    .count();
                if got != *n {
                    bad.push(format!("{got} gates noted {note}, want {n}"));
                }
            }
            if let Some(writes) = want.writes {
                let got = written_var_labels(&r);
                if got != writes {
                    bad.push(format!("writes {got:?}, want {writes:?}"));
                }
            }
            for v in want.ints {
                if !int_operands(&r).contains(v) {
                    bad.push(format!("no baked {v}: {:?}", int_operands(&r)));
                }
            }
            for v in want.floats {
                if !float_operands(&r).contains(v) {
                    bad.push(format!("no baked {v}: {:?}", float_operands(&r)));
                }
            }
            bad
        }));
        match row {
            Ok(bad) if bad.is_empty() => {}
            Ok(bad) => failures.push(format!("{label}:\n  {}", bad.join("\n  "))),
            Err(_) => failures.push(format!("{label}: panicked")),
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// How each enum construction, match and conversion lowers. Scrutinees are
/// `static var`s because an enum input port lowers to a scalar binding; a
/// trailing `emit done` keeps a match's join observable (an unconsumed
/// `Union` is pruned).
#[test]
fn an_enum_lowers_to_its_shape() {
    use FoldMode::{ForceOff as Off, ForceOn as On};
    use gc::{BRANCH, COMPARE_EQUAL, SELECT, UNION, VAR, VAR_GET};
    check_enum_rows(&[
        (
            "a match EXPRESSION is a pure Select tree: no exec Branch (that is the statement form)",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nstatic var s: Shape = Shape.Circle(3.0)\n\
             out area = match s { Circle(r) => r, Rect(w, h) => w, Empty => 0.0 }\n",
            Expect {
                counts: &[(SELECT, 3), (COMPARE_EQUAL, 3), (BRANCH, 0)],
                ..Default::default()
            },
        ),
        (
            "a match STATEMENT is a Branch/Union tree",
            Off,
            "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(3.0)\nout msg: float\nout done: exec\n\
             on ReadBrickGrid() {\n  match s {\n    Circle(r) => { emit msg = r }\n    Empty => { emit msg = 0.0 }\n  }\n  emit done\n}\n",
            Expect {
                counts: &[(BRANCH, 2), (UNION, 2)],
                ..Default::default()
            },
        ),
        (
            "C2: a `let`-bound construction never reaches a stored `var`, so it carries the full \
             payload superset itself; a unit/named construction bound only `__disc` and every \
             other arm fell to `_Unsupported`",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nout v: float\non ReadBrickGrid() {\n  let s = Shape.Empty\n  \
             emit v = match s { Empty => 42.0, Circle(w) => w, Rect(w, h) => w + h }\n}\n",
            Expect {
                counts: &[(SELECT, 3)],
                ..Default::default()
            },
        ),
        (
            "C3: an enum construction passed as a mod ARGUMENT hands the callee its payload record, \
             not just `__disc` (the enum-typed param did not `want_record`)",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
             mod area(s: Shape) -> (r: float) {\n  return match s { Empty => 1.0, Circle(w) => w, Rect(w, h) => w + h }\n}\n\
             out a: float\non ReadBrickGrid() { emit a = area(Shape.Rect(3.0, 4.0)) }\n",
            Expect {
                counts: &[(SELECT, 3)],
                ..Default::default()
            },
        ),
        (
            "C4: a multi-return mod whose output is an enum forwards the payload RECORD through return \
             storage, so the match selects on the RUNTIME tag; a Call-form return leaked its own \
             construction and the match folded to one branch",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
             mod choose(k: int) -> (s: Shape) {\n  if k > 0 { return Shape.Circle(7.0) }\n  return Shape.Empty\n}\n\
             out a: float\non ReadBrickGrid() {\n  let sc = choose(1)\n  emit a = match sc { Empty => 1.0, Circle(w) => w, Rect(w, h) => w + h }\n}\n",
            Expect {
                counts: &[(SELECT, 3)],
                ..Default::default()
            },
        ),
        (
            "the record analog: per-field return storage (`__ret.*`), not the last return's literal",
            Off,
            "type Point = { x: int, y: int }\nmod choose(k: int) -> (p: Point) {\n  if k > 0 { return { x: 1, y: 2 } }\n  \
             return { x: 3, y: 4 }\n}\nout a: int\non ReadBrickGrid() {\n  let p = choose(1)\n  emit a = p.x\n}\n",
            Expect {
                labelled: &[("__ret.x", 1)],
                ..Default::default()
            },
        ),
        (
            "`if let` lowers through the match-statement path: a `disc == Some` Branch, `x` bound to \
             `o`'s `__Some_0` slot",
            Off,
            "enum Opt { Some(int), None }\nstatic var o: Opt = Opt.Some(5)\nout r: int\n\
             on ReadBrickGrid() {\n  if let Some(x) = o { emit r = x } else { emit r = 0 }\n}\n",
            Expect {
                counts: &[(BRANCH, 1)],
                ..Default::default()
            },
        ),
        (
            "single-level `let ... else`: one disc test, the matched path continues, the `else` \
             diverges so the paths never rejoin",
            Off,
            "enum Opt { Some(int), None }\nstatic var o: Opt = Opt.Some(5)\nout r: int\n\
             on ReadBrickGrid() {\n  let Some(x) = o else { emit r = -1 }\n  emit r = x\n}\n",
            Expect {
                counts: &[(BRANCH, 1), (UNION, 0)],
                ..Default::default()
            },
        ),
        (
            "a NESTED `let ... else` pattern tests EVERY level's disc: only the outer one was tested, \
             so `Some(Some(x))` on `OO.Some(Opt.None)` read the uninitialized inner slot",
            Off,
            "enum Opt { Some(int), None }\nenum OO { Some(Opt), None }\nstatic var oo: OO = OO.Some(Opt.None)\nout r: int\n\
             on ReadBrickGrid() {\n  let Some(Some(x)) = oo else { emit r = -1 }\n  emit r = x\n}\n",
            Expect {
                counts: &[(BRANCH, 2), (COMPARE_EQUAL, 2)],
                ..Default::default()
            },
        ),
        (
            "a two-level match statement walks `read_disc_at_path`/`navigate_capture` into `__Node_0`: \
             two Branches and two compares per level",
            Off,
            "enum Opt { Some(int), None }\nenum Tree { Leaf(int), Node(Opt) }\nstatic var t: Tree = Tree.Leaf(0)\n\
             out val: int\nout done: exec\non ReadBrickGrid() {\n  match t {\n    Node(Some(x)) => { emit val = x }\n    \
             Node(None) => { emit val = 0 }\n    Leaf(n) => { emit val = n }\n  }\n  emit done\n}\n",
            Expect {
                counts: &[(BRANCH, 4), (COMPARE_EQUAL, 4)],
                present: &[UNION],
                ..Default::default()
            },
        ),
        (
            "a unit variant bakes its literal discriminant: `Dir.E` is 1, not a zero/default tag",
            Off,
            "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\nout t = d.Discriminant\n",
            Expect {
                counts: &[(VAR, 1)],
                baked: vec![("d.__disc", Some(Literal::Int(1)))],
                ..Default::default()
            },
        ),
        (
            "a payload variant bakes both its tag and its payload slot (superset storage)",
            Off,
            "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(5.0)\nout d = s.Discriminant\n",
            Expect {
                counts: &[(VAR, 2)],
                baked: vec![
                    ("s.__disc", Some(Literal::Int(1))),
                    ("s.__Circle_0", Some(Literal::Float(5.0))),
                ],
                ..Default::default()
            },
        ),
        (
            "`Shape.Circle.Discriminant` names the variant directly: a literal 1, no storage",
            Off,
            "enum Shape { Empty, Circle(float) }\nout d = Shape.Circle.Discriminant\n",
            Expect {
                counts: &[(VAR, 0)],
                ints: &[1],
                ..Default::default()
            },
        ),
        (
            "`Shape.FromInt(n)` with a runtime `n`: its `__disc` is the `n` wire, not a baked tag, and \
             the match builds a Select tree keyed on it",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin n: int\nlet e = Shape.FromInt(n)\n\
             out t = e.Discriminant\nout r = match e { Rect(w, h) => w, _ => -1.0 }\n",
            Expect {
                counts: &[(SELECT, 1)],
                baked: vec![("Shape.__disc", None)],
                ..Default::default()
            },
        ),
        (
            "a runtime enum value emits the real EnumToInteger gate, fed by its `__disc`",
            Off,
            "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\nout live = EnumToInt(d)\n",
            Expect {
                counts: &[(gc::EXPR_ENUM_TO_INTEGER, 1)],
                ..Default::default()
            },
        ),
        (
            "a runtime IntToEnum emits its gate, feeding the result's `__disc`, so a match on it \
             selects on the runtime tag",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin n: int\nstatic var s: Shape = Shape.Empty\nin go: exec\n\
             on go { s = IntToEnum(n) }\nout tag = match s { Rect(w, h) => 2, Circle(r) => 1, _ => 0 }\n",
            Expect {
                counts: &[(gc::EXPR_INTEGER_TO_ENUM, 1), (SELECT, 2)],
                ..Default::default()
            },
        ),
        (
            "an enum var assigned different variants on two `if` arms: superset storage makes each an \
             ordinary field-wise assignment into ONE shared `__disc`, which both arms write",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin pick: bool\nin r0: float\nout disc: int\n\
             on ReadBrickGrid() {\n  static var s: Shape = Shape.Empty\n  if pick { s = Shape.Circle(r0) } else { s = Shape.Rect(r0, r0) }\n  \
             emit disc = s.Discriminant\n}\n",
            Expect {
                labelled: &[("s.__disc", 1)],
                stores: &[("s.__disc", 2)],
                ..Default::default()
            },
        ),
        (
            "a mod branch-merging an enum VAR returns its superset record through storage, so the \
             caller's match switches on the tag the taken arm wrote",
            Off,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin b: bool\nin r0: float\nout v: float\nout done: exec\n\
             mod pick(p: bool, r: float) -> Shape {\n  var s: Shape = Shape.Empty\n  if p { s = Shape.Circle(r) } else { s = Shape.Rect(r, r) }\n  \
             return s\n}\non ReadBrickGrid() {\n  let res = pick(b, r0)\n  match res {\n    Circle(x) => { emit v = x }\n    \
             Rect(w, h) => { emit v = w }\n    Empty => { emit v = 0.0 }\n  }\n  emit done\n}\n",
            Expect {
                counts: &[(BRANCH, 4), (UNION, 4)],
                ..Default::default()
            },
        ),
        (
            "`Wrap<Color>`: the `W(T)` payload lays out as a NESTED Color record through the var's \
             `Type::Enum` args (a bare `T` gave `Any`, a scalar slot, and `W(Green)` misrouted), and \
             `Wrap.W(Color.Green)` bakes Green into the inner `__disc` (a dropped initializer bakes Red)",
            Off,
            "enum Color { Red, Green, Blue }\nenum Wrap<T> { W(T), Empty }\nstatic var b: Wrap<Color> = Wrap.W(Color.Green)\n\
             out val: int\nout done: exec\non ReadBrickGrid() {\n  match b {\n    W(Red) => { emit val = 1 }\n    \
             W(Green) => { emit val = 2 }\n    W(Blue) => { emit val = 3 }\n    Empty => { emit val = 0 }\n  }\n  emit done\n}\n",
            Expect {
                counts: &[(COMPARE_EQUAL, 5), (BRANCH, 5)],
                labelled: &[("b.__W_0.__disc", 1)],
                baked: vec![
                    ("b.__disc", Some(Literal::Int(0))),
                    ("b.__W_0.__disc", Some(Literal::Int(1))),
                ],
                ..Default::default()
            },
        ),
        (
            "a generic enum of a SCALAR (`Maybe`, not `Option`, which collides with the built-in) lays \
             its payload out as a plain slot: only the outer disc is switched",
            Off,
            "enum Maybe<T> { Some(T), None }\nstatic var b: Maybe<int> = Maybe.Some(7)\nout val: int\nout done: exec\n\
             on ReadBrickGrid() {\n  match b { Some(n) => { emit val = n } None => { emit val = 0 } }\n  emit done\n}\n",
            Expect {
                counts: &[(BRANCH, 2)],
                baked: vec![
                    ("b.__disc", Some(Literal::Int(0))),
                    ("b.__Some_0", Some(Literal::Int(7))),
                ],
                ..Default::default()
            },
        ),
        (
            "the Option/Result prelude constructed with BARE variant names bakes as the qualified \
             spelling does: `static_enum_ctor`'s bare form fell back to the first-variant default",
            Off,
            "static var b: Option<int> = Some(7)\nstatic var n: Option<int> = None\nstatic var e: Result<int, int> = Err(3)\n\
             out val: int\nout done: exec\non ReadBrickGrid() {\n  match b { Some(x) => { emit val = x } None => { emit val = 0 } }\n  emit done\n}\n",
            Expect {
                baked: vec![
                    ("b.__disc", Some(Literal::Int(0))),
                    ("b.__Some_0", Some(Literal::Int(7))),
                    ("n.__disc", Some(Literal::Int(1))),
                    ("e.__disc", Some(Literal::Int(1))),
                    ("e.__Err_0", Some(Literal::Int(3))),
                ],
                ..Default::default()
            },
        ),
        (
            "a record-typed payload's declaration initializer bakes per field: `enum_payload_slot` \
             built the sub-fields with `init: None`, discarding the folded `Literal::Record`",
            Off,
            "type P = { a: int, b: int }\nenum St { A, B { p: P } }\nvar s: St = St.B { p: { a: 7, b: 9 } }\n",
            Expect {
                baked: vec![
                    ("s.__disc", Some(Literal::Int(1))),
                    ("s.__B_p.a", Some(Literal::Int(7))),
                    ("s.__B_p.b", Some(Literal::Int(9))),
                ],
                ..Default::default()
            },
        ),
        (
            "returns inside a `let else` put the mod in early-return mode, so both returns write \
             ret_set storage instead of silently dropping their values",
            Off,
            "enum Opt { Has(int), Nothing }\nmod unwrapOr(o: Opt, fb: int) -> int { let Has(x) = o else { return fb }\n return x }\n\
             static var opt: Opt = Opt.Has(7)\nin go: exec\nvar r: int = 0\non go { r = unwrapOr(opt, 0) }\n",
            Expect {
                notes: &[("ret_set", 2)],
                ..Default::default()
            },
        ),
        (
            "a match on an inline map subscript: the fallback only recognised a CONSTRUCTION, so a \
             container element scrutinee with no name fell to the placeholder",
            Off,
            "enum S { A(int), B(int) }\nin go: exec\nvar m: Map<int, S>\nvar got: int = 0\non go {\n  m.set(1, S.A(7))\n  \
             match m[1] {\n    A(n) => { got = n }\n    B(n) => { got = n }\n  }\n}\n",
            Expect {
                counts: &[(gc::MAP_GET, 3), (BRANCH, 2)],
                ..Default::default()
            },
        ),
        (
            "a match on an inline array index",
            Off,
            "enum S { A(int), B(int) }\nin go: exec\nvar arr: S[]\nvar got: int = 0\non go {\n  arr.push(S.A(7))\n  \
             match arr[0] {\n    A(n) => { got = n }\n    B(n) => { got = n }\n  }\n}\n",
            Expect {
                counts: &[(gc::ARRAY_GET, 3), (BRANCH, 2)],
                ..Default::default()
            },
        ),
        (
            "`m.get(k)` yields `{Value, Found}`, so the enum is one `.Value` projection down; it \
             produced no enum record and the bind lowered to a placeholder",
            Off,
            "enum S { A(int), B(int) }\nin go: exec\nvar m: Map<int, S>\nvar got: int = 0\non go {\n  m.set(1, S.A(7))\n  \
             let e = m.get(1).Value\n  if let A(n) = e { got = n }\n}\n",
            Expect {
                counts: &[(gc::MAP_GET, 3), (BRANCH, 1)],
                ..Default::default()
            },
        ),
        (
            "`.Value` must not swallow a record that genuinely HAS a `Value` field (`a.pop()`)",
            Off,
            "var a: int[]\nvar got: int\non RoundStart() {\n  a.push(7)\n  got = a.pop().Value\n}\n",
            Expect {
                counts: &[(gc::ARRAY_POP, 1)],
                writes: Some(&["got"]),
                ..Default::default()
            },
        ),
        (
            "`const D = match <inline ctor>`: lowering re-lowered the match and hit the no-record \
             guard, shipping a placeholder 0.0 typecheck accepted; the taken arm now materializes",
            Off,
            "enum Shape { Empty, Circle(float) }\nconst D = match Shape.Circle(3.0) { Circle(r) => r, Empty => 0.0 }\nout area = D\n",
            Expect {
                floats: &[3.0],
                ..Default::default()
            },
        ),
        (
            "the chained form `const Y = X + 1.0`, unfolded: X materializes a real 3.0 for the MathAdd",
            Off,
            "enum Shape { Empty, Circle(float) }\nconst X = match Shape.Circle(3.0) { Circle(r) => r, _ => 0.0 }\n\
             const Y = X + 1.0\nout v = Y\n",
            Expect {
                floats: &[3.0],
                ..Default::default()
            },
        ),
        (
            "the chained form folded: the whole `X + 1.0` chain collapses to 4.0",
            On,
            "enum Shape { Empty, Circle(float) }\nconst X = match Shape.Circle(3.0) { Circle(r) => r, _ => 0.0 }\n\
             const Y = X + 1.0\nout v = Y\n",
            Expect {
                floats: &[4.0],
                ..Default::default()
            },
        ),
        (
            "an inline construction scrutinee in expression position, the general case the same guard broke",
            Off,
            "enum Shape { Empty, Circle(float) }\nout area = match Shape.Circle(3.0) { Circle(r) => r, Empty => 0.0 }\n",
            Expect {
                floats: &[3.0],
                ..Default::default()
            },
        ),
        (
            "an unchecked `unsafe e.B.s = ..` write touches the slot and nothing else: the tag is \
             asserted, not tested, so no Branch and no `__disc` write",
            Off,
            "enum E { A, B { s: string, n: int } }\nvar e: E = E.B { s: \"hi\", n: 1 }\nin go: exec\non go { unsafe e.B.s = \"written\" }\n",
            Expect {
                counts: &[(BRANCH, 0)],
                writes: Some(&["e.__B_s"]),
                ..Default::default()
            },
        ),
        (
            "an unchecked `unsafe e.B.s` read takes the payload slot's own gate",
            Off,
            "enum E { A, B { s: string } }\nvar e: E = E.B { s: \"hi\" }\nvar got: string = \"\"\nin go: exec\non go { got = unsafe e.B.s }\n",
            Expect {
                counts: &[(VAR_GET, 1)],
                writes: Some(&["got"]),
                ..Default::default()
            },
        ),
    ]);
}

/// A compile-time-known enum value needs no runtime decision: a match on a
/// `const` scrutinee elides to its taken arm under Auto folding, a constant
/// conversion folds to its literal, and `@nofold` keeps the real tree.
#[test]
fn a_constant_enum_scrutinee_or_conversion_elides() {
    use FoldMode::{Auto, ForceOff, ForceOn};
    use gc::{BRANCH, COMPARE_EQUAL, SELECT, UNION};
    check_enum_rows(&[
        (
            "a const FromInt value (disc 2 is `Rect`) elides to the taken leaf; that arm ignores its \
             defaulted payload captures, so the elision is well-defined",
            Auto,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst e = Shape.FromInt(2)\n\
             out picked = match e { Rect(w, h) => 20.0, Circle(r) => 10.0, _ => 0.0 }\n",
            Expect {
                counts: &[(SELECT, 0)],
                ..Default::default()
            },
        ),
        (
            "a constant IntToEnum in an enum-typed context builds the record directly, with no gate, and \
             the match on it elides",
            Auto,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nlet e: Shape = IntToEnum(2)\n\
             out picked = match e { Rect(w, h) => 20.0, Circle(r) => 10.0, _ => 0.0 }\n",
            Expect {
                counts: &[(gc::EXPR_INTEGER_TO_ENUM, 0), (SELECT, 0)],
                ..Default::default()
            },
        ),
        (
            "a match expression on a known-variant `const` emits no Select and no Branch",
            Auto,
            "enum Shape { Empty, Circle(float) }\nconst s = Shape.Circle(5.0)\nout area = match s { Circle(r) => r, Empty => 0.0 }\n",
            Expect {
                counts: &[(SELECT, 0), (BRANCH, 0)],
                ..Default::default()
            },
        ),
        (
            "`@nofold` promises nothing folded or elided: the same const scrutinee keeps a real Select \
             tree (as `lower_if`'s own fallback does)",
            Auto,
            "@nofold\n\nenum Shape { Empty, Circle(float) }\nconst s = Shape.Circle(5.0)\nout area = match s { Circle(r) => r, Empty => 0.0 }\n",
            Expect {
                counts: &[(SELECT, 2)],
                ..Default::default()
            },
        ),
        (
            "the statement form emits no Branch/Union; only the taken arm's `emit` runs",
            Auto,
            "enum Shape { Empty, Circle(float) }\nconst s = Shape.Circle(5.0)\nout msg: float\nout done: exec\n\
             on ReadBrickGrid() {\n  match s {\n    Circle(r) => { emit msg = r }\n    Empty => { emit msg = 0.0 }\n  }\n  emit done\n}\n",
            Expect {
                counts: &[(BRANCH, 0), (UNION, 0)],
                ..Default::default()
            },
        ),
        (
            "`EnumToInt` of a compile-time-known variant folds to its discriminant with no gate",
            ForceOff,
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nout folded = EnumToInt(Shape.Circle(1.0))\n",
            Expect {
                counts: &[(gc::EXPR_ENUM_TO_INTEGER, 0)],
                ..Default::default()
            },
        ),
        (
            "`is` on two constants folds to a literal: no compare survives",
            ForceOn,
            "enum Dir { N, E, S, W }\nconst d = Dir.E\nout hit = d is Dir.E\n",
            Expect {
                counts: &[(COMPARE_EQUAL, 0)],
                ..Default::default()
            },
        ),
    ]);
}

/// A payload write sets exactly the slots it names, per field: a record-typed
/// slot is laid out and read per-field, so it must be written per-field too.
#[test]
fn a_payload_write_sets_exactly_its_slots() {
    let pre = "enum E { A, B { s: string, b: bool } }\nvar e: E = E.B { s: \"hi\", b: false }\nin go: exec\n";
    let match_capture =
        format!("{pre}on go {{ match e {{ B {{ s, b }} => {{ b = true }}, A => {{}} }} }}\n");
    let let_else_capture =
        format!("{pre}on go {{ let B {{ s, b }} = e else {{ return }}\nb = true }}\n");
    let if_let_capture = format!("{pre}on go {{ if let B {{ s, b }} = e {{ b = true }} }}\n");
    use FoldMode::ForceOff as Off;
    check_enum_rows(&[
        (
            "a record-typed payload field from a literal: `lower_expr` yields one port, so the record \
             collapsed to a placeholder and `assign_record_fields` skipped the slot",
            Off,
            "type Track = { origin: vector, direction: vector }\nenum St { A, B { next_track: Track, my_str: string } }\n\
             var s: St = St.A\non RoundStart() {\n  s = St.B { next_track: { origin: Vec(4.0, 4.0, 4.0), direction: Vec(1.0, 0.0, 0.0) }, my_str: \"hi\" }\n}\n",
            Expect {
                writes: Some(&[
                    "s.__B_my_str",
                    "s.__B_next_track.direction",
                    "s.__B_next_track.origin",
                    "s.__disc",
                ]),
                ..Default::default()
            },
        ),
        (
            "the same slot fed by an existing record VAR (already a `Binding::Record`)",
            Off,
            "type Track = { origin: vector, direction: vector }\nenum St { A, B { next_track: Track } }\nvar s: St = St.A\n\
             var t: Track = { origin: Vec(0.0, 0.0, 0.0), direction: Vec(0.0, 0.0, 0.0) }\non RoundStart() {\n  s = St.B { next_track: t }\n}\n",
            Expect {
                writes: Some(&[
                    "s.__B_next_track.direction",
                    "s.__B_next_track.origin",
                    "s.__disc",
                ]),
                ..Default::default()
            },
        ),
        (
            "a positional record payload builds the same per-field list",
            Off,
            "type Track = { origin: vector, direction: vector }\nenum St { A, B(Track) }\nvar s: St = St.A\n\
             var t: Track = { origin: Vec(0.0, 0.0, 0.0), direction: Vec(0.0, 0.0, 0.0) }\non RoundStart() {\n  s = St.B(t)\n}\n",
            Expect {
                writes: Some(&["s.__B_0.direction", "s.__B_0.origin", "s.__disc"]),
                ..Default::default()
            },
        ),
        (
            "the SILENT twin: a nested-enum payload lowered through `try_lower_enum_ctor`, which returned \
             only the inner `__disc` and dropped its `pending_inline_record`: 2 of 4 slots, no WSP001",
            Off,
            "enum Inner { X, Y(int) }\nenum Outer { A, B { i: Inner, s: string } }\nvar o: Outer = Outer.A\n\
             on RoundStart() {\n  o = Outer.B { i: Inner.Y(42), s: \"hi\" }\n}\n",
            Expect {
                writes: Some(&["o.__B_i.__Y_0", "o.__B_i.__disc", "o.__B_s", "o.__disc"]),
                ..Default::default()
            },
        ),
        (
            "an `if let` capture IS the scrutinee's `__B_b` slot: `b = true` writes that gate, not a \
             fresh local; `e.__B_s` and `e.__disc` are untouched",
            Off,
            &if_let_capture,
            Expect {
                writes: Some(&["e.__B_b"]),
                ..Default::default()
            },
        ),
        (
            "a `match` capture reaches the same slot",
            Off,
            &match_capture,
            Expect {
                writes: Some(&["e.__B_b"]),
                ..Default::default()
            },
        ),
        (
            "a `let ... else` capture reaches the same slot",
            Off,
            &let_else_capture,
            Expect {
                writes: Some(&["e.__B_b"]),
                ..Default::default()
            },
        ),
        (
            "a record-typed capture assigned a record literal fans out one Var_Set per leaf",
            Off,
            "type P = { x: int, y: int }\nenum E { A, B { p: P } }\nvar e: E = E.B { p: { x: 0, y: 0 } }\nin go: exec\n\
             on go { if let B { p } = e { p = { x: 7, y: 9 } } }\n",
            Expect {
                writes: Some(&["e.__B_p.x", "e.__B_p.y"]),
                ..Default::default()
            },
        ),
    ]);
}

/// An enum-element container is one parallel container per column (the
/// `__disc` plus every variant's payload slot), alone or as a record field. A
/// push writes every column (a constant one inlines its literal as gate data)
/// and a read pulls every column.
#[test]
fn an_enum_container_has_one_column_per_slot() {
    let rows: &[(&str, &str, &str, usize, &[&str])] = &[
        (
            "an enum-element array: `__disc` + `__A_0` + `__B_0`",
            "enum S { A(int), B(int) }\nvar arr: S[]\n",
            gc::ARRAY_VAR,
            3,
            &[],
        ),
        (
            "an enum-element map",
            "enum S { A(int), B(int) }\nvar m: Map<int, S>\n",
            gc::MAP_VAR,
            3,
            &[],
        ),
        (
            "a record array's enum field: `s.__disc` + `s.__A_0` + `s.__B_0` + `k` (the enum arm was \
             gated to a plain record VAR, so the field collapsed)",
            "enum S { A(int), B(int) }\ntype H = { s: S, k: int }\nvar arr: H[]\n",
            gc::ARRAY_VAR,
            4,
            &[],
        ),
        (
            "a record map's enum field",
            "enum S { A(int), B(int) }\ntype H = { s: S, k: int }\nvar m: Map<int, H>\n",
            gc::MAP_VAR,
            4,
            &[],
        ),
        (
            "a push emitted no `Value` wire at all, discarding tag and payload and leaving orphan vars",
            "enum S { A(int), B(int) }\nin go: exec\nin u: int\nvar arr: S[]\non go { arr.push(S.A(u)) }\n",
            gc::ARRAY_PUSH,
            3,
            &[],
        ),
        (
            "pushing a record with an enum field",
            "enum S { A(int), B(int) }\ntype H = { s: S, k: int }\nin go: exec\nin u: int\nvar arr: H[]\non go { arr.push({ s: S.A(u), k: u }) }\n",
            gc::ARRAY_PUSH,
            4,
            &[],
        ),
        (
            "a bare `ArrayVar_Get` off one collapsed column fed `.Discriminant` whatever that column \
             held; an element read pulls every column, so a later `match` reaches any arm's payload",
            "enum S { A(int), B(int) }\nin go: exec\nvar arr: S[]\nvar d: int = 0\non go {\n  arr.push(S.A(1))\n  let e = arr[0]\n  d = e.Discriminant\n}\n",
            gc::ARRAY_GET,
            3,
            &["d"],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, class, columns, writes) in rows {
        let r = compile(src);
        if !crate::test_support::error_codes(&r.diagnostics).is_empty()
            || has_unsupported(&r.module)
        {
            failures.push(format!("{label}: not clean: {:?}", r.diagnostics));
            continue;
        }
        let n = count_class(&r.module, class);
        if n != *columns {
            failures.push(format!("{label}: {n} x {class}, want {columns}"));
        }
        for push in nodes_of(&r.module, gc::ARRAY_PUSH) {
            let wired = r
                .module
                .wires
                .iter()
                .any(|w| w.target.node_id == push.id && w.target.port == WirePort::Value);
            if !wired && !push.properties.contains_key(&crate::intern::sym::VALUE) {
                failures.push(format!(
                    "{label}: push {:?} carries no value at all",
                    push.id
                ));
            }
        }
        if !writes.is_empty() && written_var_labels(&r) != *writes {
            failures.push(format!(
                "{label}: writes {:?}, want {writes:?}",
                written_var_labels(&r)
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// `.Value` on an enum is the identity: a program is the same with or without
/// it, in every position through `resolve_field_chain`, payload captures
/// included (the whole enum, not its tag), never an `_Unsupported` placeholder.
#[test]
fn dot_value_on_an_enum_is_the_bare_value() {
    let d = "enum D { A = 0, B = 1 }\nvar d: D = A\n";
    let shape = "enum Shape { Empty, Circle(float), Box { w: float, h: float } }\nvar s: Shape = Circle(3.0)\n";
    let rows: &[(&str, &str, &str)] = &[
        (
            "a match scrutinee",
            d,
            "out v = match d@ { A => 11.0, B => 22.0 }\n",
        ),
        (
            "a `let` initializer",
            d,
            "let e: D = d@\nout v = match e { A => 11.0, B => 22.0 }\n",
        ),
        ("`.Discriminant` after it", d, "out v = d@.Discriminant\n"),
        (
            "a payload capture",
            shape,
            "out area = match s@ { Circle(r) => r, Box { w, h } => w, Empty => 0.0 }\n",
        ),
    ];
    let mut failures = Vec::new();
    for (label, head, body) in rows {
        let bare = compile(&format!("{head}{}", body.replace('@', "")));
        let dotted = compile(&format!("{head}{}", body.replace('@', ".Value")));
        if !crate::test_support::error_codes(&dotted.diagnostics).is_empty()
            || dotted.diagnostics.iter().any(|d| d.code == "WSP001")
            || has_unsupported(&dotted.module)
        {
            failures.push(format!("{label}: not clean: {:?}", dotted.diagnostics));
            continue;
        }
        if wiring_shape(&bare.module) != wiring_shape(&dotted.module) {
            failures.push(format!("{label}: `.Value` changed the lowered program"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn mod_named_after_prelude_variant_lowers_to_the_mod_call() {
    // A user `mod` named after a prelude variant (`Ok`/`Err`/`Some`/`None`)
    // resolves as a mod call, not as a bare `Result.Ok` enum construction. The
    // Option/Result prelude is seeded for every program, so the bare-variant
    // shadow check applies even to programs with no enums. Lowering's predicate
    // ORs `resolve_mod` with `scope.get` (which carries the mod's
    // `Binding::Chip`), parallel to const-eval's `lookup_mod` and typecheck's
    // full-scope lookup.
    //
    // `Ok()` calls the mod, whose body reads the runtime `counter` var; a
    // `Result.Ok` construction would never read it, so the Var_Get gate proves
    // the mod call won. The body is non-constant so const-eval cannot fold the
    // call away.
    let r = compile_ok(
        "static var counter: int = 5\n\
         mod Ok() -> (r: int) { return counter }\n\
         out result: int\n\
         out done: exec\n\
         on ReadBrickGrid() {\n\
           emit result = Ok()\n\
           emit done\n\
         }\n",
    );
    // The mod body ran: it reads `counter` at runtime.
    assert!(
        has_gate(&r, gc::VAR_GET),
        "`Ok()` must lower to the user mod call (reads `counter`), not a bare Result.Ok construction"
    );
    // And no Result enum record was built: an enum construction would allocate
    // `__disc`/`__Ok_0`/`__Err_0` payload-slot gates for the value.
    let has_result_record = r.module.nodes.values().any(|n| {
        matches!(
            n.properties.get(&crate::intern::sym::NAME_LABEL),
            Some(crate::ir::Literal::String(s))
                if s.ends_with(".__disc") || s.contains(".__Ok_0") || s.contains(".__Err_0")
        )
    });
    assert!(
        !has_result_record,
        "`Ok()` must NOT lower to a Result enum construction (no __disc/__Ok_0/__Err_0 gates)"
    );
}

#[test]
fn to_int_is_a_discriminant_alias() {
    // `.ToInt()` lowers through the exact same helper as `.Discriminant`: a
    // stored value reads its `__disc`, a variant path folds to a literal. The
    // two spellings must therefore produce the identical gate-class multiset.
    let with_toint = compile(
        "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\n\
         out a = d.ToInt()\nout b = Dir.S.ToInt()\n",
    );
    let with_disc = compile(
        "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\n\
         out a = d.Discriminant\nout b = Dir.S.Discriminant\n",
    );
    assert_no_errors(&with_toint);
    assert!(!has_unsupported(&with_toint.module));
    assert_eq!(
        gate_class_counts(&with_toint),
        gate_class_counts(&with_disc),
        "`.ToInt()` must lower identically to `.Discriminant`"
    );
}

#[test]
fn generic_enum_enum_arg_layout_matches_nongeneric() {
    // The generic `Wrap<Color>` layout must be IDENTICAL to the non-generic
    // `Outer { W(Color) }` - same nested-record storage, same disc reads. A
    // per-kind gate-count parity locks that in: a divergence (e.g. a scalar
    // `__W_0` on the generic side) would change the compare/branch/var counts.
    let program = |decl: &str, ty: &str| {
        compile(&format!(
            "enum Color {{ Red, Green, Blue }}\n{decl}\n\
             static var b: {ty} = {}\n\
             out val: int\nout done: exec\n\
             on ReadBrickGrid() {{\n\
               match b {{ W(Red) => {{ emit val = 1 }} W(Green) => {{ emit val = 2 }} W(Blue) => {{ emit val = 3 }} Empty => {{ emit val = 0 }} }}\n\
               emit done\n\
             }}\n",
            if ty.starts_with("Wrap") {
                "Wrap.W(Color.Red)"
            } else {
                "Outer.W(Color.Red)"
            }
        ))
    };
    let g = program("enum Wrap<T> { W(T), Empty }", "Wrap<Color>");
    let n = program("enum Outer { W(Color), Empty }", "Outer");
    for class in [gc::COMPARE_EQUAL, gc::BRANCH, gc::VAR, gc::UNION] {
        assert_eq!(
            gate_count(&g, class),
            gate_count(&n, class),
            "generic `Wrap<Color>` must lower `{class}` identically to non-generic `Outer`"
        );
    }
    assert!(!has_unsupported(&g.module));
    assert!(!has_unsupported(&n.module));
}

#[test]
fn enum_value_into_out_port_is_a_loud_drop() {
    // An enum value driving an out-port is dropped (0 wires; output-port
    // materialization is unimplemented). The drop warns with WSP001 for a direct
    // construction and for an enum-returning call.
    for src in [
        "enum Shape { Empty, Circle(float) }\nout e = Shape.Circle(5.0)\n",
        "enum Shape { Empty, Circle(float) }\n\
         mod mk() -> Shape { return Shape.Circle(2.0) }\nout e = mk()\n",
    ] {
        let r = compile(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WSP001" && d.message.contains("enum value can't drive")),
            "an enum value dropped at an out-port must warn (WSP001), got: {:?}",
            r.diagnostics
        );
    }
}

// `EasingFunction` and similar are registry enums seeded by `enums::build_registry`
// with no `enum` declaration in source. These tests check that `.Discriminant`,
// stored-var discriminant reads, and exhaustive `match` lower identically to a
// hand-written enum's, since the built-in seed is an ordinary `EnumDef`.

#[test]
fn builtin_game_enum_variant_path_discriminant_bakes_schema_int_no_placeholder() {
    // `EasingFunction.Bounce.Discriminant` folds to the real schema int (10), not
    // an auto-numbered index, with no unsupported placeholder. Like
    // `variant_path_discriminant_is_a_bare_literal` above, plus a baked-value
    // check: renumbering the tag would still pass a placeholder-only check.
    let want = crate::catalog::enum_member_value("EBREasingFunction", "Bounce")
        .expect("EasingFunction.Bounce is a real schema member");
    let r = compile_ok("out d = EasingFunction.Bounce.Discriminant\n");
    assert!(!has_unsupported(&r.module));
    assert!(
        int_operands(&r).contains(&want),
        "d must bake Bounce's real schema discriminant ({want}), got {:?}",
        int_operands(&r)
    );

    // Parity: a hand-written enum with `Bounce` pinned to that SAME
    // discriminant must lower to the identical node/wire shape - the
    // built-in seed is not a special-cased path, it is the same registry
    // entry a user `enum` declaration would produce.
    let user = compile_ok(&format!(
        "enum E {{ A, Bounce = {want} }}\nout d = E.Bounce.Discriminant\n"
    ));
    assert!(!has_unsupported(&user.module));
    assert_eq!(
        r.module.nodes.len(),
        user.module.nodes.len(),
        "built-in variant-path discriminant must lower to the same node count as a user enum's"
    );
    assert_eq!(
        r.module.wires.len(),
        user.module.wires.len(),
        "built-in variant-path discriminant must lower to the same wire count as a user enum's"
    );
}

#[test]
fn builtin_game_enum_stored_var_discriminant_reads_disc_no_placeholder() {
    // `static var e: EasingFunction = EasingFunction.Linear` bakes `e.__disc`
    // to Linear's real schema value,
    // and `e.Discriminant` reads that SAME shared `__disc` gate (not a
    // placeholder) when compared against another bare-path constant.
    let linear = crate::catalog::enum_member_value("EBREasingFunction", "Linear").expect("Linear");
    let bounce = crate::catalog::enum_member_value("EBREasingFunction", "Bounce").expect("Bounce");
    let r = compile_ok(
        "static var e: EasingFunction = EasingFunction.Linear\n\
         out m = e.Discriminant == EasingFunction.Bounce.Discriminant\n",
    );
    assert!(!has_unsupported(&r.module));
    assert_eq!(
        baked_init(&r, "e.__disc"),
        Some(crate::ir::Literal::Int(linear)),
        "e.__disc must bake Linear's real schema discriminant ({linear}), not an auto-numbered index"
    );
    assert!(has_gate(&r, gc::COMPARE_EQUAL));
    assert!(
        int_operands(&r).contains(&bounce),
        "the comparison's other side must bake Bounce's real schema discriminant ({bounce}), got {:?}",
        int_operands(&r)
    );
}

#[test]
fn builtin_game_enum_exhaustive_match_lowers_like_a_user_enum() {
    // A `match` covering EVERY schema variant, built dynamically from
    // `catalog::builtin_game_enums()` (not a hardcoded variant list) so this
    // stays correct if the schema's variant set changes. Parity-checked
    // gate-class-count for gate-class-count against a hand-written enum with
    // the identical variant set and discriminants - the built-in seed must
    // lower a full match exactly like a user enum's registry entry would.
    let variants = crate::catalog::builtin_game_enums()
        .into_iter()
        .find(|e| e.clean_name == "EasingFunction")
        .expect("EasingFunction is a built-in game enum")
        .variants;
    assert!(
        variants.len() > 1,
        "EasingFunction must have at least two schema variants for this test to be meaningful"
    );

    let arms = variants
        .iter()
        .enumerate()
        .map(|(idx, v)| format!("{} => {{ emit r = {idx} }}", v.clean_name))
        .collect::<Vec<_>>()
        .join("\n    ");
    let program = |ty_decl: &str, ty: &str| {
        compile(&format!(
            "{ty_decl}static var e: {ty} = {ty}.{}\n\
             out r: int\nout done: exec\n\
             on ReadBrickGrid() {{\n  match e {{\n    {arms}\n  }}\n  emit done\n}}\n",
            variants[0].clean_name
        ))
    };
    let easing_decl = format!(
        "enum Easing {{ {} }}\n",
        variants
            .iter()
            .map(|v| format!("{} = {}", v.clean_name, v.disc))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let builtin = program("", "EasingFunction");
    let user = program(&easing_decl, "Easing");

    for r in [&builtin, &user] {
        assert_no_errors(r);
        assert!(!has_unsupported(&r.module));
        assert!(has_gate(r, gc::BRANCH));
        assert!(has_gate(r, gc::UNION));
    }
    for class in [
        gc::COMPARE_EQUAL,
        gc::BRANCH,
        gc::VAR,
        gc::UNION,
        gc::VAR_SET,
    ] {
        assert_eq!(
            gate_count(&builtin, class),
            gate_count(&user, class),
            "built-in `EasingFunction` must lower `{class}` identically to the equivalent hand-written enum"
        );
    }
}

// A qualified built-in enum value passed as a matching gate config arg
// bakes the SAME integer the bare member name bakes, straight into the gate's
// data field, with no `_Unsupported` fallback.
#[test]
fn qualified_builtin_enum_config_arg_bakes_discriminant() {
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = EasingFunction.Bounce)\n";
    let r = compile_ok(src);
    assert!(
        !has_gate(&r, crate::ir::gate_class::UNSUPPORTED),
        "a qualified enum config value must not fall back to an _Unsupported gate"
    );
    let node = find_gate(&r, crate::ir::gate_class::MATH_EASING);
    let func = r.module.nodes[&node]
        .properties
        .get(&crate::intern::intern("Function"))
        .expect("Function data field set from `function = EasingFunction.Bounce`");
    let expected =
        crate::catalog::enum_member_value("EBREasingFunction", "Bounce").expect("Bounce member");
    assert!(
        matches!(func, crate::ir::Literal::Int(v) if *v == expected),
        "expected Function = Int({expected}) (the same int bare `Bounce` bakes), got {func:?}"
    );

    // The bare member name bakes the identical integer.
    let bare = "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bounce)\n";
    let rb = compile(bare);
    let nb = find_gate(&rb, crate::ir::gate_class::MATH_EASING);
    assert_eq!(
        rb.module.nodes[&nb]
            .properties
            .get(&crate::intern::intern("Function")),
        Some(&crate::ir::Literal::Int(expected)),
        "the qualified and bare forms must bake the same Function integer"
    );
}

// A qualified built-in enum value on a DATA-DRIVEN config field (a raw
// settings-menu name with no declared param, e.g. Remap's `Function`) must bake
// its integer too. This is the separate data-driven `args` loop, not the
// declared-param loop the test above covers.
#[test]
fn qualified_builtin_enum_data_driven_config_bakes_discriminant() {
    let expected =
        crate::catalog::enum_member_value("EBREasingFunction", "Bounce").expect("Bounce member");
    let src =
        "in v: float\nout r = Remap(v, 0.0, 1.0, 0.0, 100.0, Function = EasingFunction.Bounce)\n";
    let r = compile_ok(src);
    let node = find_gate(&r, crate::ir::gate_class::EXPR_REMAP);
    let func = r.module.nodes[&node]
        .properties
        .get(&crate::intern::intern("Function"))
        .expect("Function data field set from `Function = EasingFunction.Bounce`");
    assert!(
        matches!(func, crate::ir::Literal::Int(v) if *v == expected),
        "expected Function = Int({expected}) (the same int bare `Function = Bounce` bakes), got {func:?}"
    );

    // The bare member name bakes the identical integer on this same path.
    let bare = "in v: float\nout r = Remap(v, 0.0, 1.0, 0.0, 100.0, Function = Bounce)\n";
    let rb = compile(bare);
    let nb = find_gate(&rb, crate::ir::gate_class::EXPR_REMAP);
    assert_eq!(
        rb.module.nodes[&nb]
            .properties
            .get(&crate::intern::intern("Function")),
        Some(&crate::ir::Literal::Int(expected)),
        "qualified and bare forms must bake the same Function integer on the data-driven path"
    );
}

#[test]
fn record_payload_subfields_keep_their_own_values() {
    // Per-field copying has to preserve field IDENTITY, not merely emit one
    // write per slot: keying the copy wrong would still produce a write for
    // every sub-field and satisfy a label-only check while swapping the values.
    let r = compile_ok(
        "type P = { a: int, b: int }
         enum St { A, B { p: P, n: int } }
         var s: St = St.A
         on RoundStart() {
           s = St.B { p: { a: 7, b: 9 }, n: 5 }
         }
",
    );
    assert_eq!(
        written_value(&r, "s.__B_p.a"),
        Some(crate::ir::Literal::Int(7))
    );
    assert_eq!(
        written_value(&r, "s.__B_p.b"),
        Some(crate::ir::Literal::Int(9))
    );
    assert_eq!(
        written_value(&r, "s.__B_n"),
        Some(crate::ir::Literal::Int(5))
    );
    assert_eq!(
        written_value(&r, "s.__disc"),
        Some(crate::ir::Literal::Int(1))
    );
}

#[test]
fn assigning_an_enum_payload_field_directly_is_diagnosed() {
    // `e.s = v` has no lowering: an enum value is a record of `__disc` plus
    // `__{Variant}_{field}` slots, so the surface name `s` resolves to nothing
    // and every branch of `lower_assign` falls through to a bare `return`, so
    // without this diagnostic the write vanishes with no gate.
    let r = compile(
        "enum E { A, B { s: string, b: bool } }\n\
         var e: E = E.B { s: \"hi\", b: false }\n\
         in go: exec\n\
         on go { e.s = \"nope\" }\n",
    );
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS007" && d.severity == crate::diagnostic::Severity::Error)
        .unwrap_or_else(|| panic!("expected WS007: {:?}", r.diagnostics));
    assert!(
        d.message.contains("if let") || d.message.contains("match"),
        "the message must name the destructure that DOES work: {}",
        d.message
    );
    assert!(written_var_labels(&r).is_empty());
}

#[test]
fn assigning_an_unknown_field_is_diagnosed() {
    // The same silent fall-through swallowed ordinary typos: a field name not
    // on the record, and a field on a scalar, both compiled to nothing.
    let typo = compile(
        "type T = { origin: int }\n\
         var t: T = { origin: 0 }\n\
         in go: exec\n\
         on go { t.orgin = 1 }\n",
    );
    assert!(
        typo.diagnostics.iter().any(|d| d.code == "WS007"),
        "expected WS007 for a misspelled field target: {:?}",
        typo.diagnostics
    );
    let scalar = compile(
        "var n: int = 0\n\
         in go: exec\n\
         on go { n.whatever = 5 }\n",
    );
    assert!(
        scalar.diagnostics.iter().any(|d| d.code == "WS007"),
        "expected WS007 for a field target on a scalar: {:?}",
        scalar.diagnostics
    );
}

#[test]
fn is_lowers_like_a_discriminant_comparison() {
    // `s is Shape.Circle` means `s.Discriminant == Shape.Circle.Discriminant`,
    // so it must produce the identical gate-class multiset: one compare
    // against the variant's baked tag, with no extra storage or branching.
    let with_is = compile(
        "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Empty\n\
         out hit = s is Shape.Circle\n",
    );
    let with_disc = compile(
        "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Empty\n\
         out hit = s.Discriminant == Shape.Circle.Discriminant\n",
    );
    assert_no_errors(&with_is);
    assert!(!has_unsupported(&with_is.module));
    assert!(has_gate(&with_is, gc::COMPARE_EQUAL));
    assert_eq!(
        gate_class_counts(&with_is),
        gate_class_counts(&with_disc),
        "`is` must lower identically to a discriminant comparison"
    );
}

/// A statement `match`, `if let` or `let ... else` on a call's result runs the
/// call first and tests its result after, like the `let`-bound twin: the
/// handler's exec does not fork, and the call's effect comes before every
/// test and before the arm's store.
#[test]
fn a_decision_on_a_call_result_tests_after_the_call() {
    let head = "enum S { Sa(int), Sb(float), Sc }\nvar k: int = 3\nvar n: int = 0\nvar cnt: int = 0\nin go: exec\n\
                mod g(x: int) -> S { cnt += 1\n if x > 2 { return S.Sb(1.5) }\n return S.Sa(x) }\n";
    let bodies = [
        "on go { match g(k) { S.Sa(z) => { n = z }, _ => { n = 9 } } }",
        "on go { let t = g(k)\n match t { S.Sa(z) => { n = z }, _ => { n = 9 } } }",
        "on go { if let S.Sa(z) = g(k) { n = z } }",
        "on go { let t = g(k)\n if let S.Sa(z) = t { n = z } }",
        "on go { let Sa(z) = g(k) else { return }\n n = z }",
        "on go { let t = g(k)\n let Sa(z) = t else { return }\n n = z }",
    ];
    for body in bodies {
        for r in [
            compile(&format!("{head}{body}")),
            compile_folded(&format!("{head}{body}")),
        ] {
            assert_no_errors(&r);
            let m = &r.module;
            let input = m
                .nodes
                .values()
                .find(|n| n.kind == crate::ir::NodeKind::Input)
                .expect("go")
                .id;
            let starts = m
                .wires
                .iter()
                .filter(|w| w.source == input.port(WirePort::RerOutput))
                .count();
            assert_eq!(starts, 1, "the handler's exec forks: {body}");
            let inc = find_gate(&r, gc::VAR_INCREMENT);
            for n in m.nodes.values() {
                if n.gate_class == crate::ir::gate_class::BRANCH
                    || (n.gate_class == crate::ir::gate_class::VAR_SET
                        && n.source_range.start.line > 8)
                {
                    assert!(
                        exec_reaches(m, inc, n.id),
                        "{} runs before the call: {body}",
                        n.gate_class
                    );
                }
            }
        }
    }
}

/// A constant initializer of an enum array bakes every column: the tags and
/// each variant's payload, with a zero where a row's variant has no such
/// payload. It baked nothing, so the array started empty. Top level, as a
/// record field, and handler-local.
#[test]
fn an_enum_array_initializer_bakes_its_columns() {
    let src = "enum S { A, B(int), C { w: float } }\nenum Hue { Red, Green, Blue }\n\
               type R = { hs: Hue[], k: int }\n\
               var xs: S[] = [S.A, S.B(3), S.C { w: 2.5 }]\nvar hu: Hue[] = [Hue.Blue, Hue.Red]\n\
               var r: R = { hs: [Hue.Green, Hue.Blue], k: 4 }\nvar n: int = 0\nin go: exec\n\
               on go {\n var loc: Hue[] = [Hue.Blue]\n n = xs.length() + hu.length() + r.hs.length() + loc.length()\n}";
    for r in [compile(src), compile_folded(src)] {
        assert_no_errors(&r);
        let mut baked: Vec<Vec<Literal>> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == crate::ir::gate_class::ARRAY_VAR)
            .filter_map(
                |n| match n.properties.get(&crate::intern::sym::INITIAL_VALUE) {
                    Some(Literal::Array(items)) => Some(items.clone()),
                    _ => None,
                },
            )
            .collect();
        baked.sort_by_key(|v| format!("{v:?}"));
        let ints = |v: &[i64]| v.iter().map(|&i| Literal::Int(i)).collect::<Vec<_>>();
        for want in [
            ints(&[0, 1, 2]),
            ints(&[0, 3, 0]),
            vec![
                Literal::Float(0.0),
                Literal::Float(0.0),
                Literal::Float(2.5),
            ],
            ints(&[2, 0]),
            ints(&[1, 2]),
            ints(&[2]),
        ] {
            assert!(baked.contains(&want), "{want:?} in {baked:?}");
        }
    }
}

/// A container initializer stores its values whether it spells a literal or
/// names a constant, for enum-valued maps and records with an enum field too:
/// each form compiles as its literal twin does, at the top level and
/// handler-local. Top-level enum maps, the enum column of a record container
/// and every constant-named top-level initializer baked nothing; a
/// handler-local constant-named one was dropped with a warning.
#[test]
fn a_constant_container_initializer_is_its_literal_twin() {
    let head = "enum Hue { Red, Green, Blue }\nenum S { A, B(int), C { w: float } }\n\
                type R = { h: Hue, k: int }\nvar n: int = 0\nvar so: string = \"\"\nin go: exec\n\
                const IS = [1, 2]\nconst SS = [\"x\", \"y\"]\nconst HS = [Hue.Red, Hue.Blue]\n\
                const PS = [S.A, S.B(3), S.C { w: 2.5 }]\nconst M = { \"a\": 2, \"b\": 1 }\n\
                const HM = { \"a\": Hue.Blue, \"b\": Hue.Green }\n\
                const RS = [{ h: Hue.Blue, k: 1 }, { h: Hue.Green, k: 2 }]\n\
                const RM = { \"a\": { h: Hue.Blue, k: 1 } }\n";
    let forms = [
        ("var c: int[] = IS", "[1, 2]", "n = c[1]"),
        ("var c: string[] = SS", "[\"x\", \"y\"]", "so = c[1]"),
        (
            "var c: Hue[] = HS",
            "[Hue.Red, Hue.Blue]",
            "n = c[1].ToInt()",
        ),
        (
            "var c: S[] = PS",
            "[S.A, S.B(3), S.C { w: 2.5 }]",
            "n = c.length()",
        ),
        (
            "var c: Map<string, int> = M",
            "{ \"a\": 2, \"b\": 1 }",
            "n = c.get(\"a\").Value",
        ),
        (
            "var c: Map<string, Hue> = HM",
            "{ \"a\": Hue.Blue, \"b\": Hue.Green }",
            "n = c.get(\"a\").Value.ToInt()",
        ),
        (
            "var c: R[] = RS",
            "[{ h: Hue.Blue, k: 1 }, { h: Hue.Green, k: 2 }]",
            "n = c.length()",
        ),
        (
            "var c: Map<string, R> = RM",
            "{ \"a\": { h: Hue.Blue, k: 1 } }",
            "n = c.length()",
        ),
    ];
    for (decl, literal, read) in forms {
        let twin = format!("{} = {literal}", decl.rsplit_once(" = ").unwrap().0);
        let top = |d: &str| format!("{head}{d}\non go {{ {read} }}");
        let local = |d: &str| format!("{head}on go {{ {d}\n {read} }}");
        let mut pairs = vec![(top(decl), top(&twin))];
        // The literal twin of a payload enum carries the zero payload slots
        // through temporary vars, so that one is compared by what it pushes.
        if !decl.contains("S[]") {
            pairs.push((local(decl), local(&twin)));
        }
        for (a, b) in pairs {
            for ((mode, ra), (_, rb)) in super::generics::both_folds(&a)
                .into_iter()
                .zip(super::generics::both_folds(&b))
            {
                assert_no_errors(&ra);
                assert!(
                    !ra.diagnostics.iter().any(|d| d.code == "WSP001"),
                    "`{decl}` {mode}: {:?}",
                    ra.diagnostics
                );
                assert_eq!(
                    super::generic_differential::shape(&ra),
                    super::generic_differential::shape(&rb),
                    "`{a}`, {mode}"
                );
            }
        }
    }
    let pushes = |r: &LowerResult| {
        let mut v: Vec<String> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == crate::ir::gate_class::ARRAY_PUSH)
            .map(|n| format!("{:?}", n.properties.get(&crate::intern::sym::VALUE)))
            .collect();
        v.sort();
        v
    };
    // What the literal forms bake: the enum map's tags, and the tag column of
    // a record array's and a record map's enum field.
    let baked = |src: &str, class: &str| -> Vec<Literal> {
        let r = compile_ok(&format!("{head}{src}\non go {{ n = 0 }}"));
        r.module
            .nodes
            .values()
            .filter(|n| n.gate_class == class)
            .filter_map(|n| {
                n.properties
                    .get(&crate::intern::sym::INITIAL_VALUE)
                    .cloned()
            })
            .collect()
    };
    let (s, i) = (|v: &str| Literal::String(v.into()), Literal::Int);
    let map = crate::ir::gate_class::MAP_VAR;
    let array = crate::ir::gate_class::ARRAY_VAR;
    assert_eq!(
        baked(
            "var c: Map<string, Hue> = { \"a\": Hue.Blue, \"b\": Hue.Green }",
            map
        ),
        [Literal::Map(vec![(s("a"), i(2)), (s("b"), i(1))])]
    );
    let rs = baked(
        "var c: R[] = [{ h: Hue.Blue, k: 1 }, { h: Hue.Green, k: 2 }]",
        array,
    );
    assert!(
        rs.contains(&Literal::Array(vec![i(2), i(1)]))
            && rs.contains(&Literal::Array(vec![i(1), i(2)])),
        "{rs:?}"
    );
    let rm = baked(
        "var c: Map<string, R> = { \"a\": { h: Hue.Blue, k: 1 } }",
        map,
    );
    assert!(
        rm.contains(&Literal::Map(vec![(s("a"), i(2))]))
            && rm.contains(&Literal::Map(vec![(s("a"), i(1))])),
        "{rm:?}"
    );
    // Tags 0 1 2, `B_0` 0 3 0, `C_w` 0.0 0.0 2.5.
    let a = compile_ok_folded(&format!(
        "{head}on go {{ var c: S[] = PS\n n = c.length() }}"
    ));
    let want = [
        "Float(0.0)",
        "Float(0.0)",
        "Float(2.5)",
        "Int(0)",
        "Int(0)",
        "Int(0)",
        "Int(1)",
        "Int(2)",
        "Int(3)",
    ];
    assert_eq!(pushes(&a), want.map(|w| format!("Some({w})")));
}

/// An enum-valued map field of a record bakes its literal initializer's tags,
/// as an enum-element array field does.
#[test]
fn a_record_field_enum_map_literal_bakes_its_tags() {
    let r = compile_ok(
        "enum Hue { Red, Green, Blue }\ntype R = { m: Map<string, Hue> }\n\
         var r: R = { m: { \"a\": Hue.Blue, \"b\": Hue.Green } }\n\
         in go: exec\non go { BroadcastChatMessage(\"${r.m.length()}\") }",
    );
    let baked: Vec<Literal> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::MAP_VAR)
        .filter_map(|n| {
            n.properties
                .get(&crate::intern::sym::INITIAL_VALUE)
                .cloned()
        })
        .collect();
    let s = |v: &str| Literal::String(v.into());
    assert_eq!(
        baked,
        [Literal::Map(vec![
            (s("a"), Literal::Int(2)),
            (s("b"), Literal::Int(1))
        ])]
    );
}

/// A record field initialized from a named constant container (`{ xs: IS }`)
/// compiles as its literal twin does: at the top level, `static`, and
/// handler-local, for scalar, record and enum elements, maps, a `let`-bound
/// constant and a nested record. The top-level and `static` field baked an
/// empty container, a column of records or enums was never written in a
/// handler, and the handler-local scalar field was `WS027`.
#[test]
fn a_record_field_naming_a_constant_container_is_its_literal_twin() {
    let head = "enum Hue { Red, Green, Blue }\ntype P = { x: int, y: float }\n\
                type R = { xs: int[], fs: float[], ss: string[], m: Map<string, int>, pts: P[], hs: Hue[], \
                pm: Map<string, P>, a: int }\ntype O = { r: R, k: int }\n\
                var n: int = 0\nvar fo: float = 0.0\nvar so: string = \"\"\nin go: exec\n\
                const IS = [1, 2]\nconst FS = [1.5, 2.5]\nconst SS = [\"a\", \"b\"]\nconst MS = { \"a\": 1, \"b\": 2 }\n\
                const PS = [{ x: 1, y: 1.5 }, { x: 2, y: 2.5 }]\nconst HS = [Hue.Blue, Hue.Red]\n\
                const PM = { \"a\": { x: 3, y: 3.5 } }\nlet LS = [7, 8, 9]\n";
    let named = "{ xs: IS, fs: FS, ss: SS, m: MS, pts: PS, hs: HS, pm: PM, a: 1 }";
    let literal = "{ xs: [1, 2], fs: [1.5, 2.5], ss: [\"a\", \"b\"], m: { \"a\": 1, \"b\": 2 }, \
                   pts: [{ x: 1, y: 1.5 }, { x: 2, y: 2.5 }], hs: [Hue.Blue, Hue.Red], \
                   pm: { \"a\": { x: 3, y: 3.5 } }, a: 1 }";
    let read = "n = p.xs[1] + p.m.length() + p.pts.x[1] + p.hs[0].ToInt() + p.pm.length()\n \
                fo = p.fs[1] + p.pts.y[0]\n so = p.ss[1]";
    let nested_read =
        "n = o.r.xs[1] + o.r.pts.x[1] + o.r.hs[0].ToInt() + o.r.m.length()\n so = o.r.ss[0]";
    let pairs = [
        (named.to_string(), literal.to_string()),
        (
            named.replace("xs: IS", "xs: LS"),
            literal.replace("xs: [1, 2]", "xs: [7, 8, 9]"),
        ),
    ];
    for (a, b) in pairs {
        let forms = |init: &str| {
            [
                format!("{head}var p: R = {init}\non go {{ {read} }}"),
                format!("{head}on go {{ static var p: R = {init}\n {read} }}"),
                format!("{head}on go {{ var p: R = {init}\n {read} }}"),
                format!("{head}var o: O = {{ r: {init}, k: 2 }}\non go {{ {nested_read} }}"),
                format!("{head}on go {{ var o: O = {{ r: {init}, k: 2 }}\n {nested_read} }}"),
            ]
        };
        for (src, twin) in forms(&a).into_iter().zip(forms(&b)) {
            for ((mode, ra), (_, rb)) in super::generics::both_folds(&src)
                .into_iter()
                .zip(super::generics::both_folds(&twin))
            {
                assert_no_errors(&ra);
                assert_no_errors(&rb);
                assert!(
                    !ra.diagnostics.iter().any(|d| d.code == "WSP001"),
                    "{mode}: {src}: {:?}",
                    ra.diagnostics
                );
                assert_eq!(
                    super::generic_differential::shape(&ra),
                    super::generic_differential::shape(&rb),
                    "{mode}: {src}"
                );
            }
        }
    }
    // An imported constant is the same constant.
    let lib = "const LIS = [1, 2]\nconst LMS = { \"a\": 1, \"b\": 2 }";
    let main = |init: &str| {
        format!(
            "import {{ LIS, LMS }} from \"lib\"\nvar n: int = 0\nin go: exec\ntype R = {{ xs: int[], m: Map<string, int>, a: int }}\n\
             var p: R = {init}\non go {{ var q: R = {init}\n n = p.xs[1] + p.m.length() + q.xs[1] + q.m.length() + LIS[0] + LMS.length() }}"
        )
    };
    // A container initializer that bakes is not warned about as dropped.
    for init in ["CR.xs", "if C then IS else JS"] {
        let head = "var n: int = 0\nin go: exec\nconst IS = [1, 2]\nconst JS = [3]\nconst C = true\n\
                    const CR = { xs: [3, 4, 5], a: 1 }\n";
        for src in [
            format!("{head}var xs: int[] = {init}\non go {{ n = xs.length() }}"),
            format!("{head}on go {{ static var xs: int[] = {init}\n n = xs.length() }}"),
        ] {
            let r = compile_ok(&src);
            assert!(
                !r.diagnostics.iter().any(|d| d.code == "WSP001"),
                "{src}: {:?}",
                r.diagnostics
            );
        }
    }
    let ra = compile_multi(&main("{ xs: LIS, m: LMS, a: 1 }"), &[("lib", lib)]);
    let rb = compile_multi(
        &main("{ xs: [1, 2], m: { \"a\": 1, \"b\": 2 }, a: 1 }"),
        &[("lib", lib)],
    );
    assert_no_errors(&ra);
    assert!(
        !ra.diagnostics.iter().any(|d| d.code == "WSP001"),
        "{:?}",
        ra.diagnostics
    );
    assert_eq!(
        super::generic_differential::shape(&ra),
        super::generic_differential::shape(&rb)
    );
}

/// An enum field of a record var bakes its initializer, tag and payload, as an
/// enum var does: at the top level, nested a record deeper, and for a payload
/// enum (it started at the first variant with every slot zero).
#[test]
fn an_enum_field_of_a_record_var_bakes_its_initializer() {
    use crate::intern::sym;
    let src = "enum Hue { Red, Green, Blue }\nenum Shape { Dot, Circle(float), Box { w: int, h: int } }\n\
               type Q = { x: float, h: Hue }\ntype N = { inner: Q, s: Shape }\n\
               var p: Q = { x: 1.5, h: Hue.Green }\n\
               var nn: N = { inner: { x: 2.5, h: Hue.Blue }, s: Shape.Circle(4.5) }\n\
               var bx: N = { inner: { x: 0.5, h: Hue.Red }, s: Shape.Box { w: 7, h: 9 } }\n\
               var h2: Hue = Hue.Blue\nvar n: int = 0\nvar f: float = 0.0\nin go: exec\n\
               on go {\n n = p.h.ToInt() + nn.inner.h.ToInt() + h2.ToInt()\n\
               match nn.s { Circle(r) => { f = r } _ => { } }\nmatch bx.s { Box { w, h } => { n = n + w * h } _ => { } }\n}";
    for r in [compile(src), compile_folded(src)] {
        assert_no_errors(&r);
        let initial = |label: &str| {
            r.module
                .nodes
                .values()
                .find_map(|n| match n.properties.get(&*sym::NAME_LABEL) {
                    Some(Literal::String(l)) if l == label => {
                        n.properties.get(&*sym::INITIAL_VALUE).cloned()
                    }
                    _ => None,
                })
        };
        for (label, want) in [
            ("h2.__disc", Literal::Int(2)),
            ("p.h.__disc", Literal::Int(1)),
            ("nn.inner.h.__disc", Literal::Int(2)),
            ("nn.s.__disc", Literal::Int(1)),
            ("nn.s.__Circle_0", Literal::Float(4.5)),
            ("bx.s.__disc", Literal::Int(2)),
            ("bx.s.__Box_w", Literal::Int(7)),
            ("bx.s.__Box_h", Literal::Int(9)),
        ] {
            assert_eq!(initial(label), Some(want), "{label}");
        }
    }
    // A handler's `static var` keeps only its baked initializer.
    let src = "enum Hue { Red, Green, Blue }\ntype Q = { x: float, h: Hue }\nvar n: int = 0\nin go: exec\n\
               on go {\n static var sp: Q = { x: 1.0, h: Hue.Blue }\n n = sp.h.ToInt()\n sp.h = Hue.Red\n}";
    for r in [compile(src), compile_folded(src)] {
        assert_no_errors(&r);
        let disc =
            r.module
                .nodes
                .values()
                .find_map(|n| match n.properties.get(&*sym::NAME_LABEL) {
                    Some(Literal::String(l)) if l.ends_with("sp.h.__disc") => {
                        n.properties.get(&*sym::INITIAL_VALUE).cloned()
                    }
                    _ => None,
                });
        assert_eq!(disc, Some(Literal::Int(2)));
    }
}

/// `Select(c, a, b)` of two enum values or records chooses each leaf, `b`
/// when `c` is true: the same gates and wires as `if c then b else a` (it was
/// WS094, a value lowered to nothing).
#[test]
fn select_of_enum_values_or_records_is_its_if_twin() {
    let head = "enum K { A, B, C }\nenum Shape { Dot, Circle(float), Box { w: int, h: int } }\n\
                type P = { x: int, y: float }\nin go: exec\nin c: bool\nvar k: K = K.A\nvar n: int = 0\n\
                var s: Shape = Shape.Dot\nvar p: P = { x: 1, y: 2.0 }\nvar q: P = { x: 3, y: 4.0 }\n";
    for (select, twin) in [
        ("k = Select(c, K.B, K.C)", "k = if c then K.C else K.B"),
        (
            "let j = Select(c, K.C, k)\nn = j.ToInt()",
            "let j = if c then k else K.C\nn = j.ToInt()",
        ),
        (
            "s = Select(c, Shape.Circle(2.5), Shape.Box { w: 3, h: 4 })",
            "s = if c then Shape.Box { w: 3, h: 4 } else Shape.Circle(2.5)",
        ),
        (
            "match Select(c, K.A, K.B) { A => { n = 1 } _ => { n = 2 } }",
            "match (if c then K.B else K.A) { A => { n = 1 } _ => { n = 2 } }",
        ),
        ("p = Select(c, p, q)", "p = if c then q else p"),
    ] {
        for fold in [false, true] {
            let build = |body: &str| {
                let src = format!("{head}on go {{\n{body}\n}}");
                let r = if fold {
                    compile_folded(&src)
                } else {
                    compile(&src)
                };
                assert_no_errors(&r);
                wiring_shape(&r.module)
            };
            assert_eq!(build(select), build(twin), "{select} fold {fold}");
        }
    }
}

/// A bare read of an enum array or map (`hs.pop()`, `m.get(k)`) as the
/// scrutinee of a `match`, `if let` or `let ... else` is the enum it reads,
/// as it is wherever one value is expected: the same gates and wires as its
/// `.Value` twin (it was WS066).
#[test]
fn a_bare_enum_container_read_is_a_scrutinee() {
    let head = "enum Hue { Red, Green, Blue }\nenum Shape { Dot, Circle(float), Box { w: int, h: int } }\n\
                in go: exec\nvar hs: Hue[] = [Hue.Red, Hue.Blue]\nvar ss: Shape[] = [Shape.Circle(2.5)]\n\
                var hm: Map<string, Hue> = { \"a\": Hue.Green }\nvar n: int = 0\nvar f: float = 0.0\n";
    for body in [
        "match hs.pop()@ { Red => { n = 1 } Green => { n = 2 } Blue => { n = 3 } }",
        "n = match hs.pop()@ { Red => 1, _ => 2 }",
        "match ss.pop()@ { Circle(r) => { f = r } Box { w, h } => { n = w * h } Dot => { } }",
        "if let Circle(r) = ss.pop()@ { f = r }",
        "match hm.get(\"a\")@ { Green => { n = 7 } _ => { } }",
    ] {
        for fold in [false, true] {
            let build = |value: &str| {
                let src = format!("{head}on go {{\n{}\n}}", body.replace('@', value));
                let r = if fold {
                    compile_folded(&src)
                } else {
                    compile(&src)
                };
                assert_no_errors(&r);
                assert!(
                    !r.diagnostics.iter().any(|d| d.code == "WSP001"),
                    "{src}: {:?}",
                    r.diagnostics
                );
                wiring_shape(&r.module)
            };
            assert_eq!(build(""), build(".Value"), "{body} fold {fold}");
        }
    }
    for value in ["", ".Value"] {
        let src = format!(
            "{head}mod radius(xs: Shape[]) -> float {{\nlet Circle(r) = xs.pop(){value} else {{ return 0.0 }}\nreturn r\n}}\n\
             on go {{ f = radius(ss) }}"
        );
        let r = compile_ok(&src);
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WSP001"),
            "{src}: {:?}",
            r.diagnostics
        );
    }
}

/// An unannotated top-level array literal of enum values is an array of that
/// enum, like its annotated twin: same gates, wires and baked columns (any
/// method on it was WS044).
#[test]
fn an_unannotated_top_level_enum_array_literal_is_its_annotated_twin() {
    let head = "enum Hue { Red, Green, Blue }\nenum Shape { Dot, Circle(float), Box { w: int, h: int } }\n\
                in go: exec\nvar n: int = 0\nvar f: float = 0.0\nconst KB = Hue.Blue\nconst KC = KB\n";
    for (decl, ty, body) in [
        (
            "var hu@ = [Hue.Blue, Hue.Red]",
            ": Hue[]",
            "n = hu.length()\nhu.push(Hue.Green)\nmatch hu[0] { Red => { n = n + 10 } _ => { } }",
        ),
        (
            "var hu@ = [KB, Hue.Red]",
            ": Hue[]",
            "n = hu.length()\nmatch hu[0] { Blue => { n = n + 10 } _ => { } }",
        ),
        ("var hu@ = [KC, KB]", ": Hue[]", "n = hu[1].ToInt()"),
        (
            "var ss@ = [Shape.Circle(2.5), Shape.Dot]",
            ": Shape[]",
            "match ss.pop() { Circle(r) => { f = r } _ => { } }",
        ),
    ] {
        for fold in [false, true] {
            let build = |t: &str| {
                let src = format!("{head}{}\non go {{\n{body}\n}}", decl.replace('@', t));
                let r = if fold {
                    compile_folded(&src)
                } else {
                    compile(&src)
                };
                assert_no_errors(&r);
                let mut baked: Vec<String> = r
                    .module
                    .nodes
                    .values()
                    .filter_map(
                        |n| match n.properties.get(&crate::intern::sym::INITIAL_VALUE) {
                            Some(v @ Literal::Array(_)) => Some(format!("{v:?}")),
                            _ => None,
                        },
                    )
                    .collect();
                baked.sort();
                (wiring_shape(&r.module), baked)
            };
            assert_eq!(build(""), build(ty), "{decl} fold {fold}");
        }
    }
}

/// A record `var` initialized from a constant record (`var p: Q = CQ`) stores
/// what its literal twin (`var p: Q = { .. }`) stores: nested records, an
/// enum field's tag and payload, a container field, through a `let` and an
/// imported constant too. Each field started at zero.
#[test]
fn a_record_var_initialized_from_a_constant_record_is_its_literal_twin() {
    let types = "enum Hue { Red, Green, Blue }\nenum Sh { Circle(float), Rect(int, int), Nothing }\n\
                 type P = { x: int, h: Hue }\ntype Q = { p: P, s: Sh, f: float, xs: int[] }\n";
    let value = "{ p: { x: 1, h: Hue.Blue }, s: Sh.Rect(3, 4), f: 2.5, xs: [5, 6] }";
    let tail = "in go: exec\nvar n: float = 0.0\nout on1: float = n\n";
    let read = "n = q.p.x + q.p.h.ToInt() + q.f + q.xs[1]";
    for (what, decl) in [
        ("top level", "var q: Q = INIT\non go { READ }"),
        ("static", "on go { static var q: Q = INIT\n READ }"),
    ] {
        for binding in ["const CQ: Q", "let CQ: Q"] {
            let src = |init: &str| {
                format!(
                    "{types}{binding} = {value}\n{tail}{}",
                    decl.replace("INIT", init).replace("READ", read)
                )
            };
            let (named, literal) = (
                super::generics::both_folds(&src("CQ")),
                super::generics::both_folds(&src(value)),
            );
            for ((mode, a), (_, b)) in named.into_iter().zip(literal) {
                assert_no_errors(&a);
                assert_eq!(
                    super::generic_differential::shape(&a),
                    super::generic_differential::shape(&b),
                    "{what} `{binding}`, {mode}"
                );
            }
        }
    }
    // Handler-local: the field values are stored (the literal twin also
    // carries the payload slots of the enum through temporary vars).
    let src = format!("{types}const CQ: Q = {value}\n{tail}on go {{ var q: Q = CQ\n {read} }}");
    for fold in [false, true] {
        let saved = super::float_slots::save_numbers(&src, fold);
        for v in [
            Literal::Int(1),
            Literal::Int(2),
            Literal::Int(3),
            Literal::Int(4),
            Literal::Float(2.5),
        ] {
            assert!(
                saved.contains(&v),
                "handler-local fold {fold}: no {v:?}\n{saved:?}"
            );
        }
    }
    // An imported constant record.
    let lib = format!("{types}pub const CQ: Q = {value}\n");
    let main = |init: &str| {
        format!(
            "import {{ CQ, Q, P, Hue, Sh }} from \"lib\"\n{tail}var q: Q = {init}\non go {{ {read} }}"
        )
    };
    let (a, b) = (
        compile_multi(&main("CQ"), &[("lib", &lib)]),
        compile_multi(&main(value), &[("lib", &lib)]),
    );
    assert_no_errors(&a);
    assert_eq!(
        super::generic_differential::shape(&a),
        super::generic_differential::shape(&b),
        "imported constant"
    );
}
