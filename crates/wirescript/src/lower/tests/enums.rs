use super::*;

// NOTE: `has_gate(&r, "BrickComponentType_WireGraphPseudo_Unsupported")` never
// matches any gate; the real gate-class constant (`gc::UNSUPPORTED`) is
// `"_Unsupported"`, the exact string every other test in this suite checks
// (see e.g. `lower::tests::basic`). A test using the prefixed spelling would
// silently pass even if a construction fell back to a placeholder, so this
// suite asserts against `"_Unsupported"` to keep the check load-bearing.

/// The `InitialValue` baked into the `Pseudo_Var` whose `_label` NAME_LABEL is
/// exactly `label` (an enum var's `__disc`/slot gates are labelled
/// `"{var}.__disc"` / `"{var}.__{Variant}_{slot}"` by `declare_enum_container`).
/// `None` if no such gate exists or it carries no `InitialValue`. This locks in
/// the actual BAKED tag/payload VALUE, not merely the gate count -- a
/// regression that baked `__disc = 0` for every variant (the classic tag
/// silent-miscompile) still emits the right number of `Pseudo_Var`s and would
/// pass a count-only check.
fn baked_init(r: &LowerResult, label: &str) -> Option<crate::ir::Literal> {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .find(|n| {
            matches!(
                n.properties.get(&crate::intern::sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == label
            )
        })
        .and_then(|n| n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned())
}

/// How many `class` gates carry a `NAME_LABEL` of exactly `label` (an enum
/// var's `__disc`/slot gates are labelled `"{var}.__disc"` etc. by
/// `declare_enum_container`). Used to prove the shared `__disc` is ONE gate
/// both branches write, not a per-branch copy.
fn gate_count_labelled(r: &LowerResult, class: &str, label: &str) -> usize {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .filter(|n| {
            matches!(
                n.properties.get(&crate::intern::sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == label
            )
        })
        .count()
}

#[test]
fn match_expression_lowers_to_select_tree_no_branch() {
    // An enum INPUT port lowers to a scalar `Binding::Input`, not the `__disc`
    // + payload-slot `Binding::Record` a match scrutinee reads; only enum VARS
    // get that Record (via `declare_enum_container`), so this test uses a var
    // scrutinee rather than an input port.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         static var s: Shape = Shape.Circle(3.0)\n\
         out area = match s { Circle(r) => r, Rect(w, h) => w, Empty => 0.0 }\n",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"));
    // Expression form is pure: no exec Branch (that is the statement form).
    assert!(!has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"));
    // The real placeholder gate-class constant is `_Unsupported` (see this
    // module's top note); a wrong string like `WireGraphPseudo_Unsupported`
    // would never match, so asserting against the real one keeps the check
    // load-bearing.
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn match_statement_lowers_to_branch_union() {
    // Uses `static var s` rather than an input port: an enum INPUT port lowers
    // to a scalar `Binding::Input`, not the `__disc` + payload-slot
    // `Binding::Record` a match scrutinee reads (see
    // `match_expression_lowers_to_select_tree_no_branch`); `static var` is the
    // canonical record scrutinee.
    // A trailing `emit done` follows the match because a `Union` whose ExecOut
    // is unconsumed is a dead sink that `prune_dead_exec_unions` removes (a
    // tail `if` loses its join the same way); the match must not be the
    // handler's last statement for the join to be observable.
    let r = compile(
        "enum Shape { Empty, Circle(float) }\n\
         static var s: Shape = Shape.Circle(3.0)\n\
         out msg: float\n\
         out done: exec\n\
         on ReadBrickGrid() {\n\
           match s {\n\
             Circle(r) => { emit msg = r }\n\
             Empty => { emit msg = 0.0 }\n\
           }\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Union"));
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn let_bound_construction_match_reads_full_superset() {
    // A `let`-bound enum CONSTRUCTION never reaches a stored `var`, so it must
    // carry the full payload superset itself - otherwise a match on it can only
    // read the constructed variant's payload and every other arm falls to
    // `_Unsupported`. Regression for the C2 silent miscompile: a unit/named
    // construction bound only `__disc` (dropping the payload), and even the
    // positional form carried only its own variant's slot. The fixtures use
    // `var`/top-level-const scrutinees, which get the superset for free, so this
    // exercises the `let` shape they sidestep.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         out v: float\n\
         on ReadBrickGrid() {\n\
           let s = Shape.Empty\n\
           emit v = match s { Empty => 42.0, Circle(w) => w, Rect(w, h) => w + h }\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"));
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn enum_construction_as_mod_arg_passes_payload() {
    // An enum construction passed as a mod ARGUMENT must hand the callee its
    // payload record, not just `__disc`. Regression for the C3 silent miscompile
    // where an enum-typed param did not `want_record`, so the body's match on
    // the parameter fell to `_Unsupported`.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         mod area(s: Shape) -> (r: float) {\n\
           return match s { Empty => 1.0, Circle(w) => w, Rect(w, h) => w + h }\n\
         }\n\
         out a: float\n\
         on ReadBrickGrid() { emit a = area(Shape.Rect(3.0, 4.0)) }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn multi_return_enum_mod_forwards_record() {
    // A multi-output mod whose single output is an enum must forward the payload
    // RECORD, not a scalar `ret_val`. Regression for the C4 silent miscompile
    // where matching the returned value lost the payload captures.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         mod choose(k: int) -> (s: Shape) {\n\
           if k > 0 { return Shape.Circle(7.0) }\n\
           return Shape.Empty\n\
         }\n\
         out a: float\n\
         on ReadBrickGrid() {\n\
           let sc = choose(1)\n\
           emit a = match sc { Empty => 1.0, Circle(w) => w, Rect(w, h) => w + h }\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // Value-load-bearing: the match must select on the RUNTIME return value (a
    // `Select` keyed on a `Var_Get` of the return record's `__disc`), not fold to
    // a constant. Before the record-valued return storage, a Call-form return
    // (`Shape.Circle(7.0)`) leaked its own construction record to the caller, so
    // the scrutinee's tag was a fixed literal and the whole match folded to that
    // one branch's value - a silent miscompile with NO `_Unsupported` to catch it.
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "match on a multi-return enum must select at runtime, not fold to a fixed branch"
    );
}

#[test]
fn multi_return_record_mod_forwards_runtime_branch() {
    // The record analog of `multi_return_enum_mod_forwards_record`: a multi-return
    // mod with a single RECORD output stores each return into per-field return
    // storage (`__ret.*`, read at runtime), instead of leaking one return's
    // literal to the caller. Before the fix the caller bound the last return's
    // `{ x: 3, y: 4 }` and `p.x` folded to the constant 3 regardless of `k`; the
    // presence of the `__ret.x` return-storage var proves the runtime path.
    let r = compile(
        "type Point = { x: int, y: int }\n\
         mod choose(k: int) -> (p: Point) {\n\
           if k > 0 { return { x: 1, y: 2 } }\n\
           return { x: 3, y: 4 }\n\
         }\n\
         out a: int\n\
         on ReadBrickGrid() {\n\
           let p = choose(1)\n\
           emit a = p.x\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        gate_count_labelled(&r, "BrickComponentType_WireGraphPseudo_Var", "__ret.x") >= 1,
        "a multi-return record mod must allocate per-field return storage, not leak a literal"
    );
}

#[test]
fn if_let_lowers_to_branch_and_binds() {
    // Uses `static var o` rather than an input port, same reason as the
    // `match` tests above: an enum INPUT port lowers to a scalar
    // `Binding::Input`, not the `__disc` + payload-slot `Binding::Record` a
    // pattern match reads (see `match_statement_lowers_to_branch_union`).
    // The negative assertion checks `_Unsupported`, not
    // `WireGraphPseudo_Unsupported` (a string no gate ever carries, see this
    // module's top note); a failed capture bind would fall back to the real
    // placeholder constant, so this keeps the check load-bearing.
    let r = compile(
        "enum Opt { Some(int), None }\n\
         static var o: Opt = Opt.Some(5)\n\
         out r: int\n\
         on ReadBrickGrid() {\n\
           if let Some(x) = o { emit r = x } else { emit r = 0 }\n\
         }\n",
    );
    assert_no_errors(&r);
    // The refutable head lowers through the shared match-statement path: a
    // `disc == Some` Branch, the then-arm on ExecOutA, the else on ExecOutB.
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"));
    // No placeholder: `x` bound to `o`'s `__Some_0` payload slot, `emit r = x`
    // wired it. A failed bind would emit `_Unsupported` here.
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn single_level_let_else_binds_and_continues() {
    // The supported single-level case: one disc test (one Branch), the matched
    // path binds `x` and continues, the `else` diverges. A `static var` scrutinee
    // (see the note above on enum I/O ports).
    let r = compile(
        "enum Opt { Some(int), None }\n\
         static var o: Opt = Opt.Some(5)\n\
         out r: int\n\
         on ReadBrickGrid() {\n\
           let Some(x) = o else { emit r = -1 }\n\
           emit r = x\n\
         }\n",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch"), 1);
    // No `Union`: the paths never rejoin (the `else` diverges).
    assert!(!has_gate(&r, "BrickComponentType_WireGraph_Exec_Union"));
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn nested_let_else_tests_every_disc() {
    // Regression: a NESTED refutable pattern in `let ... else` must test EVERY
    // level's disc, not just the outer one. `Some(Some(x))` on an `OO.Some(Opt.
    // None)` must take the `else` (the inner `Opt` is `None`), not read the
    // uninitialized inner slot. Two levels -> two disc compares and two Branches;
    // before the fix only the OUTER disc was tested (one Branch), a silent wrong
    // path. Mirrors `nested_match_statement_reads_inner_disc` for the if-let/
    // let-else form.
    let r = compile(
        "enum Opt { Some(int), None }\n\
         enum OO { Some(Opt), None }\n\
         static var oo: OO = OO.Some(Opt.None)\n\
         out r: int\n\
         on ReadBrickGrid() {\n\
           let Some(Some(x)) = oo else { emit r = -1 }\n\
           emit r = x\n\
         }\n",
    );
    assert_no_errors(&r);
    // Both discs tested: outer `oo.__disc == Some` AND inner
    // `oo.__Some_0.__disc == Some`. A one-Branch result is the silent-miscompile
    // regression (only the outer disc gated).
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch"), 2);
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"), 2);
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn nested_match_statement_reads_inner_disc() {
    // A match on a NESTED enum (`Tree.Node` carries an `Opt`) with two-level
    // patterns. The outer `Switch` tests `Tree.__disc` (Leaf / Node); the Node
    // case descends into `__Node_0` and tests its `Opt.__disc` (Some / None).
    // Lowering must walk that second level via `read_disc_at_path` /
    // `navigate_capture`: each level contributes two `Branch`es (2 cases each),
    // so the two-level tree is exactly four. If the inner descent had failed to
    // find the nested `__disc` record, the read would fall back to an
    // `_Unsupported` placeholder - so `!has_gate("_Unsupported")` with four
    // branches proves the inner disc was read and switched on.
    let r = compile(
        "enum Opt { Some(int), None }\n\
         enum Tree { Leaf(int), Node(Opt) }\n\
         static var t: Tree = Tree.Leaf(0)\n\
         out val: int\n\
         out done: exec\n\
         on ReadBrickGrid() {\n\
           match t {\n\
             Node(Some(x)) => { emit val = x }\n\
             Node(None) => { emit val = 0 }\n\
             Leaf(n) => { emit val = n }\n\
           }\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Union"));
    // Outer switch (Leaf/Node) + inner switch (Some/None): two Branches each.
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch"), 4);
    // One `disc == k` compare per case, at both levels - the inner two only
    // exist because `__Node_0`'s nested `__disc` was reached and read.
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"), 4);
}

#[test]
fn unit_variant_construction_bakes_a_literal_discriminant() {
    let r = compile("enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\nout t = d.Discriminant\n");
    assert_no_errors(&r);
    // __disc backed by exactly one Pseudo_Var (the enum var), initialized to 1 (E).
    assert!(gate_count(&r, "BrickComponentType_WireGraphPseudo_Var") >= 1);
    assert!(!has_gate(&r, "_Unsupported"));
    // Lock in the BAKED tag value: `Dir.E` is discriminant 1 (N=0, E=1, S=2,
    // W=3). A regression baking 0 (or any other variant's tag) fails here even
    // though the gate count is unchanged.
    assert_eq!(
        baked_init(&r, "d.__disc"),
        Some(crate::ir::Literal::Int(1)),
        "d.__disc must bake E's discriminant (1), not a zero/default tag"
    );
}

#[test]
fn payload_variant_stores_disc_and_slot() {
    let r = compile(
        "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(5.0)\n\
         out d = s.Discriminant\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // superset storage: __disc + __Circle_0 => at least 2 Pseudo_Var gates.
    assert!(gate_count(&r, "BrickComponentType_WireGraphPseudo_Var") >= 2);
    // Lock in BOTH baked values: the tag (`Circle` is discriminant 1) and the
    // payload slot (the literal `5.0`). Count-only assertions can't catch a
    // tag/payload miscompile; these can.
    assert_eq!(
        baked_init(&r, "s.__disc"),
        Some(crate::ir::Literal::Int(1)),
        "s.__disc must bake Circle's discriminant (1)"
    );
    assert_eq!(
        baked_init(&r, "s.__Circle_0"),
        Some(crate::ir::Literal::Float(5.0)),
        "s.__Circle_0 must bake the constructed payload literal 5.0"
    );
}

#[test]
fn mod_named_after_prelude_variant_lowers_to_the_mod_call() {
    // A user `mod` named after an ALWAYS-seeded prelude variant (`Ok`/`Err`/
    // `Some`/`None`) must resolve as a MOD CALL, never be reinterpreted as a
    // bare `Result.Ok` enum construction - the Option/Result prelude is seeded
    // for every program, so a stale bare-variant shadow predicate would hit
    // programs with no enums at all. Lower's predicate ORs `resolve_mod`
    // alongside `scope.get` (which already carries the mod's `Binding::Chip`),
    // keeping the mod/chip shadow explicit and parallel to const-eval's
    // `lookup_mod` and typecheck's full-scope lookup.
    //
    // `Ok()` here calls the mod, whose body READS the runtime `counter` var; a
    // `Result.Ok` construction would never read `counter`, so the Var_Get gate
    // proves the mod call won. The body is deliberately non-constant so
    // const-eval can't fold the call away and hide which path lowering took.
    let r = compile(
        "static var counter: int = 5\n\
         mod Ok() -> (r: int) { return counter }\n\
         out result: int\n\
         out done: exec\n\
         on ReadBrickGrid() {\n\
           emit result = Ok()\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    // The mod body ran: it reads `counter` at runtime.
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"),
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
fn variant_path_discriminant_is_a_bare_literal() {
    // `Shape.Circle.Discriminant` names the variant directly (no stored
    // value) -- it must fold to a literal int, with no unsupported gate.
    let r = compile("enum Shape { Empty, Circle(float) }\nout d = Shape.Circle.Discriminant\n");
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
}

/// Every gate class in `r` (recursing into nested chips), by count. Used to
/// prove `.ToInt()` lowers to byte-for-byte the same gates as `.Discriminant`.
fn gate_class_counts(r: &LowerResult) -> std::collections::BTreeMap<String, usize> {
    fn walk(m: &crate::ir::Module, acc: &mut std::collections::BTreeMap<String, usize>) {
        for n in m.nodes.values() {
            *acc.entry(n.gate_class.to_string()).or_default() += 1;
        }
        for c in m.chips.values() {
            walk(c, acc);
        }
    }
    let mut acc = std::collections::BTreeMap::new();
    walk(&r.module, &mut acc);
    acc
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
    assert!(!has_gate(&with_toint, "_Unsupported"));
    assert_eq!(
        gate_class_counts(&with_toint),
        gate_class_counts(&with_disc),
        "`.ToInt()` must lower identically to `.Discriminant`"
    );
}

#[test]
fn from_int_wires_runtime_disc_and_matches() {
    // `Shape.FromInt(n)` with a RUNTIME `n`: the value's `__disc` is the lowered
    // `n` wire (not a baked tag), and a match on it builds a real Select tree
    // keyed on that `__disc`. No `_Unsupported` anywhere.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         in n: int\nlet e = Shape.FromInt(n)\n\
         out t = e.Discriminant\n\
         out r = match e { Rect(w, h) => w, _ => -1.0 }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "a match on a FromInt value must build a real Select tree keyed on __disc"
    );
    // `__disc` is a runtime wire, NOT a baked Pseudo_Var tag: no `Shape.__disc`
    // gate carries an InitialValue. (The defaulted payload slots, e.g.
    // `Shape.__Rect_0`, are still baked Pseudo_Vars.)
    assert_eq!(
        baked_init(&r, "Shape.__disc"),
        None,
        "FromInt's __disc must be the runtime `n` wire, not a baked tag"
    );
}

#[test]
fn const_from_int_match_elides_to_taken_arm() {
    // A `const`-bound FromInt value has a compile-time-known tag, so a match on
    // it const-elides to exactly the taken leaf under Auto folding, with no
    // Select tree at all. disc 2 is `Rect`, and the taken arm's body ignores
    // its (defaulted) payload captures, so the elision is well-defined.
    let r = compile_auto(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         const e = Shape.FromInt(2)\n\
         out picked = match e { Rect(w, h) => 20.0, Circle(r) => 10.0, _ => 0.0 }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "a const FromInt scrutinee must const-elide, not build a Select tree"
    );
}

#[test]
fn enum_to_integer_folds_constant_variant_no_gate() {
    // `EnumToInt(Shape.Circle(1.0))` is a compile-time-known variant: it
    // folds straight to the discriminant literal with NO EnumToInt gate and
    // no `_Unsupported` placeholder.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         out folded = EnumToInt(Shape.Circle(1.0))\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_EnumToInteger"),
        "a constant EnumToInt must fold, not emit the gate"
    );
}

#[test]
fn enum_to_integer_runtime_emits_the_gate() {
    // A runtime enum value emits the real `EXPR_ENUM_TO_INTEGER` gate (fed by
    // the value's `__disc`), not an `_Unsupported` placeholder.
    let r = compile(
        "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\n\
         out live = EnumToInt(d)\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_EnumToInteger"),
        1,
        "a runtime EnumToInt must emit exactly one gate"
    );
}

#[test]
fn integer_to_enum_const_folds_to_a_record_no_gate() {
    // `IntToEnum(2)` in an enum-typed context with a CONSTANT int builds the
    // enum record directly (disc 2 = Rect), with NO `EXPR_INTEGER_TO_ENUM` gate;
    // under Auto folding the match on it const-elides to the Rect arm, proving
    // the value folded to a known tag.
    let r = compile_auto(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         let e: Shape = IntToEnum(2)\n\
         out picked = match e { Rect(w, h) => 20.0, Circle(r) => 10.0, _ => 0.0 }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_IntegerToEnum"),
        "a constant IntToEnum must fold to a record, not emit the gate"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "a const IntToEnum scrutinee must const-elide, not build a Select tree"
    );
}

#[test]
fn integer_to_enum_runtime_emits_gate_and_matches_on_the_tag() {
    // A runtime int emits the `EXPR_INTEGER_TO_ENUM` gate; its output feeds the
    // result enum's `__disc`, so a match on the value builds a real Select tree
    // keyed on that runtime tag. No `_Unsupported`.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         in n: int\nstatic var s: Shape = Shape.Empty\nin go: exec\n\
         on go { s = IntToEnum(n) }\n\
         out tag = match s { Rect(w, h) => 2, Circle(r) => 1, _ => 0 }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Expr_IntegerToEnum"),
        1,
        "a runtime IntToEnum must emit exactly one gate"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "a match on a runtime IntToEnum value must build a Select tree keyed on __disc"
    );
}

#[test]
fn const_scrutinee_match_folds_to_taken_arm() {
    // Auto-fold ON (`compile_auto`, no `@nofold`): a match on a known-variant
    // scrutinee must emit no Select and no Branch. Uses a top-level `const`
    // binding + `out area = match s { ... }`, matching the style every other
    // test in this file uses (`static var s`, just with `const` so the
    // scrutinee is compile-time known).
    let r = compile_auto(
        "enum Shape { Empty, Circle(float) }\n\
         const s = Shape.Circle(5.0)\n\
         out area = match s { Circle(r) => r, Empty => 0.0 }\n",
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "a const-scrutinee match must not build a Select tree"
    );
    assert!(
        !has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"),
        "a const-scrutinee match must not build a Branch"
    );
    assert!(!has_gate(&r, "_Unsupported"));
}

#[test]
fn nofold_scrutinee_match_still_builds_the_tree() {
    // The fallback correctness half of the fast path above: `@nofold`
    // promises "nothing folded or elided", so the SAME const scrutinee must
    // still lower a real Select tree under it -- mirrors `lower_if`'s own
    // `@nofold` fallback test.
    let r = compile_auto(
        "@nofold\n\n\
         enum Shape { Empty, Circle(float) }\n\
         const s = Shape.Circle(5.0)\n\
         out area = match s { Circle(r) => r, Empty => 0.0 }\n",
    );
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"));
}

#[test]
fn const_scrutinee_match_statement_folds_to_taken_arm() {
    // The statement-form (block-armed) twin: a match on a known-variant
    // scrutinee must emit no Branch/Union, and only the taken arm's `emit`
    // runs.
    let r = compile_auto(
        "enum Shape { Empty, Circle(float) }\n\
         const s = Shape.Circle(5.0)\n\
         out msg: float\n\
         out done: exec\n\
         on ReadBrickGrid() {\n\
           match s {\n\
             Circle(r) => { emit msg = r }\n\
             Empty => { emit msg = 0.0 }\n\
           }\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"));
    assert!(!has_gate(&r, "BrickComponentType_WireGraph_Exec_Union"));
    assert!(!has_gate(&r, "_Unsupported"));
}

/// Every `Float` value baked into ANY gate property in `r` (a bare `_Literal`
/// gate, or a value the literal-inlining pass folded into a consumer gate's
/// data field - e.g. a constant output materialized as `MathAdd(value, 0.0)`).
fn float_operands(r: &LowerResult) -> Vec<f64> {
    r.module
        .nodes
        .values()
        .flat_map(|n| n.properties.values())
        .filter_map(|l| match l {
            crate::ir::Literal::Float(f) => Some(*f),
            _ => None,
        })
        .collect()
}

/// Every `Int` value baked into ANY gate property in `r` - the integer analog
/// of `float_operands` above, used to locate a baked discriminant regardless
/// of which carrier gate the literal-inlining pass ends up wrapping it in.
fn int_operands(r: &LowerResult) -> Vec<i64> {
    r.module
        .nodes
        .values()
        .flat_map(|n| n.properties.values())
        .filter_map(|l| match l {
            crate::ir::Literal::Int(i) => Some(*i),
            _ => None,
        })
        .collect()
}

fn has_wsp001(r: &LowerResult) -> bool {
    r.diagnostics.iter().any(|d| d.code == "WSP001")
}

#[test]
fn const_binding_of_a_match_on_an_inline_ctor_materializes_the_value() {
    // Regression: const-folding a `match` made typecheck
    // ACCEPT `const D = match <inline-ctor> { ... }`, but lowering re-lowered
    // the match AST and hit the no-`Binding::Record` guard -> a `_Unsupported`
    // placeholder shipped 0.0 while typecheck reported no error. The scrutinee
    // is now lowered to obtain its record (`match_scrutinee_record`), so the
    // taken arm materializes the real value.
    let r = compile(
        "enum Shape { Empty, Circle(float) }\n\
         const D = match Shape.Circle(3.0) { Circle(r) => r, Empty => 0.0 }\n\
         out area = D\n",
    );
    assert_no_errors(&r);
    assert!(!has_wsp001(&r), "no WSP001 placeholder: {:?}", r.diagnostics);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        float_operands(&r).contains(&3.0),
        "area must materialize the taken arm's value 3.0, got literals {:?}",
        float_operands(&r)
    );
}

#[test]
fn chained_const_of_a_match_on_an_inline_ctor_materializes_the_value() {
    // The chained form: `const X = match ...` feeding `const Y = X + 1.0`.
    // Without materializing the match, the MathAdd would read an
    // `_Unsupported` placeholder (wrong value) instead of the real 3.0.
    let src = "enum Shape { Empty, Circle(float) }\n\
               const X = match Shape.Circle(3.0) { Circle(r) => r, _ => 0.0 }\n\
               const Y = X + 1.0\n\
               out v = Y\n";
    // ForceOff: X materializes as a real 3.0 literal feeding the MathAdd, and
    // nothing is an `_Unsupported` placeholder.
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_wsp001(&r), "no WSP001 placeholder: {:?}", r.diagnostics);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        float_operands(&r).contains(&3.0),
        "X must materialize 3.0 for the MathAdd to read, got {:?}",
        float_operands(&r)
    );
    // Folded: the whole `X + 1.0` chain collapses to the single value 4.0.
    let rf = compile_folded(src);
    assert!(!has_gate(&rf, "_Unsupported"));
    assert!(
        float_operands(&rf).contains(&4.0),
        "v must fold to 4.0, got {:?}",
        float_operands(&rf)
    );
}

#[test]
fn match_on_an_inline_ctor_scrutinee_lowers_without_placeholder() {
    // The general (non-const-binding) case the same guard broke: an inline
    // construction scrutinee in expression position. Pre-existing, and now
    // fixed by the same `match_scrutinee_record` path.
    let r = compile(
        "enum Shape { Empty, Circle(float) }\n\
         out area = match Shape.Circle(3.0) { Circle(r) => r, Empty => 0.0 }\n",
    );
    assert_no_errors(&r);
    assert!(!has_wsp001(&r), "no WSP001 placeholder: {:?}", r.diagnostics);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        float_operands(&r).contains(&3.0),
        "the Circle arm must materialize 3.0, got {:?}",
        float_operands(&r)
    );
}

#[test]
fn enum_merged_across_branches_reads_back_by_tag() {
    // An enum var assigned DIFFERENT variants on the two
    // arms of an `if` must read back the taken branch's tag at the join. Because
    // the var is superset-allocated (`__disc` + every variant's slots),
    // each `s = Shape.X(...)` is an ordinary field-wise `Binding::Record`
    // assignment into the SAME storage gates, so the if-statement exec join
    // merges them - no bespoke per-slot Select. Both arms must Var_Set the one
    // shared `__disc` Pseudo_Var; reading `s.Discriminant` at the join returns
    // whichever arm ran.
    let r = compile(
        "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
         in pick: bool\n\
         in r0: float\n\
         out disc: int\n\
         on ReadBrickGrid() {\n\
           static var s: Shape = Shape.Empty\n\
           if pick { s = Shape.Circle(r0) } else { s = Shape.Rect(r0, r0) }\n\
           emit disc = s.Discriminant\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // One Pseudo_Var carries `s.__disc` - both branches write that same gate,
    // not a per-branch copy (a per-branch disc would defeat the join).
    assert_eq!(
        gate_count_labelled(&r, "BrickComponentType_WireGraphPseudo_Var", "s.__disc"),
        1,
        "s.__disc must be exactly one shared Pseudo_Var"
    );
    // Both arms Var_Set the shared disc (plus the payload-slot sets) - so a
    // read at the join sees the branch that ran.
    let disc_writes = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    assert!(
        disc_writes >= 2,
        "expected both branches to Var_Set the shared __disc, got {disc_writes}"
    );
}

#[test]
fn enum_returned_from_mod_through_storage_carries_the_tag() {
    // A mod that branch-merges an enum VAR on
    // different paths and `return`s the var carries the tag out through the
    // early-return-through-storage path. `return s` forwards the var's superset
    // `Binding::Record` (each arm's `s = Ctor(...)` already merged into the
    // shared storage inside the mod, exactly like the top-level branch-merge),
    // so the caller's `let res = pick(...)` reads it back as a record - no
    // `_Unsupported`, and a `match` on it decomposes to a real Branch tree
    // switching on the tag that the taken arm wrote.
    let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
               in b: bool\nin r0: float\nout v: float\nout done: exec\n\
               mod pick(p: bool, r: float) -> Shape {\n\
                 var s: Shape = Shape.Empty\n\
                 if p { s = Shape.Circle(r) } else { s = Shape.Rect(r, r) }\n\
                 return s\n\
               }\n\
               on ReadBrickGrid() {\n\
                 let res = pick(b, r0)\n\
                 match res {\n\
                   Circle(x) => { emit v = x }\n\
                   Rect(w, h) => { emit v = w }\n\
                   Empty => { emit v = 0.0 }\n\
                 }\n\
                 emit done\n\
               }\n";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // The returned enum decomposes to a record: the match on it switches on the
    // tag (a real Branch tree), proving the returned value carries the disc that
    // the mod's branch-merge wrote - not a scalar fall-through of one variant.
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"),
        "match on a mod-returned enum must build a real Branch tree on its tag"
    );
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Union"));
}

#[test]
fn generic_enum_with_enum_arg_lowers_nested_match_to_real_gates() {
    // A generic enum whose type ARGUMENT is itself an enum (`Wrap<Color>`): the
    // `W(T)` payload must lay out as a NESTED `Color` record (its `__disc` +
    // slots), reached through the var's `Type::Enum` args, so a two-level match
    // switches on the inner Color disc. If the layout had left `__W_0` a scalar
    // (the pre-fix behavior - `resolve_local_type` on a bare `T` gives `Any`),
    // `read_disc_at_path` would find no nested `__disc` record and fall back to
    // an `_Unsupported` placeholder, silently misrouting `W(Green)`/`W(Blue)`.
    let r = compile(
        "enum Color { Red, Green, Blue }\n\
         enum Wrap<T> { W(T), Empty }\n\
         static var b: Wrap<Color> = Wrap.W(Color.Green)\n\
         out val: int\n\
         out done: exec\n\
         on ReadBrickGrid() {\n\
           match b {\n\
             W(Red) => { emit val = 1 }\n\
             W(Green) => { emit val = 2 }\n\
             W(Blue) => { emit val = 3 }\n\
             Empty => { emit val = 0 }\n\
           }\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "nested Color disc must read a real gate, not a placeholder");
    // Outer switch (W / Empty) + inner switch (Red / Green / Blue): the three
    // inner `disc == k` compares only exist because `__W_0`'s nested Color
    // `__disc` record was reached and read through the generic instantiation.
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"), 5);
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch"), 5);
    // The outer tag bakes W (discriminant 0), and the nested Color record exists
    // as its own backing gate (a `Pseudo_Var` labelled through the `__W_0` slot).
    assert_eq!(
        baked_init(&r, "b.__disc"),
        Some(crate::ir::Literal::Int(0)),
        "b.__disc must bake W's discriminant (0)"
    );
    assert!(
        gate_count_labelled(&r, "BrickComponentType_WireGraphPseudo_Var", "b.__W_0.__disc") == 1,
        "the W payload must lay out as a nested Color record with its own __disc gate"
    );
    // The nested Color initializer must bake THROUGH to the inner `__disc`:
    // `Wrap.W(Color.Green)` -> `b.__W_0.__disc == 1` (Green). A regression that
    // discarded the nested initializer bakes Red (0, the first-variant default)
    // and diverges from const-eval, which folds Green - the silent miscompile
    // this test guards. Green (1) differs from W's own default so it can't pass
    // by coincidence.
    assert_eq!(
        baked_init(&r, "b.__W_0.__disc"),
        Some(crate::ir::Literal::Int(1)),
        "the nested Color initializer (Green) must bake into b.__W_0.__disc, not default to Red (0)"
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
            if ty.starts_with("Wrap") { "Wrap.W(Color.Red)" } else { "Outer.W(Color.Red)" }
        ))
    };
    let g = program("enum Wrap<T> { W(T), Empty }", "Wrap<Color>");
    let n = program("enum Outer { W(Color), Empty }", "Outer");
    for class in [
        "BrickComponentType_WireGraph_Expr_CompareEqual",
        "BrickComponentType_WireGraph_Exec_Branch",
        "BrickComponentType_WireGraphPseudo_Var",
        "BrickComponentType_WireGraph_Exec_Union",
    ] {
        assert_eq!(
            gate_count(&g, class),
            gate_count(&n, class),
            "generic `Wrap<Color>` must lower `{class}` identically to non-generic `Outer`"
        );
    }
    assert!(!has_gate(&g, "_Unsupported"));
    assert!(!has_gate(&n, "_Unsupported"));
}

#[test]
fn scalar_payload_generic_enum_still_lowers_clean() {
    // Guard the common case (Option/Result of a scalar): a generic enum whose
    // arg is a SCALAR lays its payload out as a plain slot and lowers with no
    // placeholder - unaffected by the enum-arg nested-record path.
    let r = compile(
        "enum Option<T> { Some(T), None }\n\
         static var b: Option<int> = Option.Some(7)\n\
         out val: int\nout done: exec\n\
         on ReadBrickGrid() {\n\
           match b { Some(n) => { emit val = n } None => { emit val = 0 } }\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // The Some(int) payload is a scalar slot, so only the outer Some/None disc
    // is switched - no nested disc read.
    assert_eq!(gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch"), 2);
    assert_eq!(
        baked_init(&r, "b.__disc"),
        Some(crate::ir::Literal::Int(0)),
        "b.__disc must bake Some's discriminant (0)"
    );
    assert_eq!(
        baked_init(&r, "b.__Some_0"),
        Some(crate::ir::Literal::Int(7)),
        "b.__Some_0 must bake the scalar payload 7"
    );
}

// The built-in `Option<T>`/`Result<T, E>` prelude (no `enum`
// declaration in the source) constructed with BARE variant names
// (`Some`/`None`/`Err`, not `Option.Some`/`Option.None`/`Result.Err`) must
// lower identically to the qualified spelling - same `__disc` bake, same
// payload slot bake, same STATIC fold (no live exec-time assignment left
// over). Without `static_enum_ctor`'s bare-form recognition, a bare
// `static var` initializer silently falls back to the zeroed/first-variant
// default instead of baking the constructed value - a real, easy-to-miss
// silent miscompile this test pins against directly (regression guard for
// the asymmetry: `Option.Some(7)` baked correctly before this fix, `Some(7)`
// did not).
#[test]
fn bare_prelude_variant_construction_bakes_identically_to_qualified() {
    let r = compile(
        "static var b: Option<int> = Some(7)\n\
         static var n: Option<int> = None\n\
         static var e: Result<int, int> = Err(3)\n\
         out val: int\nout done: exec\n\
         on ReadBrickGrid() {\n\
           match b { Some(x) => { emit val = x } None => { emit val = 0 } }\n\
           emit done\n\
         }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(
        baked_init(&r, "b.__disc"),
        Some(crate::ir::Literal::Int(0)),
        "bare `Some(7)` must bake Some's discriminant (0), matching `Option.Some(7)`"
    );
    assert_eq!(
        baked_init(&r, "b.__Some_0"),
        Some(crate::ir::Literal::Int(7)),
        "bare `Some(7)` must bake the scalar payload 7, matching `Option.Some(7)`"
    );
    assert_eq!(
        baked_init(&r, "n.__disc"),
        Some(crate::ir::Literal::Int(1)),
        "bare `None` must bake None's discriminant (1), matching `Option.None`"
    );
    assert_eq!(
        baked_init(&r, "e.__disc"),
        Some(crate::ir::Literal::Int(1)),
        "bare `Err(3)` must bake Err's discriminant (1), matching `Result.Err(3)`"
    );
    assert_eq!(
        baked_init(&r, "e.__Err_0"),
        Some(crate::ir::Literal::Int(3)),
        "bare `Err(3)` must bake the scalar payload 3, matching `Result.Err(3)`"
    );
}

#[test]
fn enum_value_into_out_port_is_a_loud_drop() {
    // Enum output-port materialization is not implemented yet: an enum value
    // driving an out-port is dropped (0 wires). That drop must be LOUD (a
    // WSP001 warning), never silent -- both for a direct construction and for
    // an enum-returning call.
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

// --- `EasingFunction` etc. are registry enums seeded by `enums::build_registry`
// with NO `enum` declaration anywhere in source. These tests prove `.Discriminant`, stored-var
// discriminant reads, and exhaustive `match` all lower identically to a
// hand-written enum's - the built-in seed behaves exactly like a user's own
// `EnumDef`, not a special-cased path.

#[test]
fn builtin_game_enum_variant_path_discriminant_bakes_schema_int_no_placeholder() {
    // `EasingFunction.Bounce.Discriminant` must fold to the REAL schema int
    // (10), not an auto-numbered index, with no unsupported placeholder -
    // mirrors `variant_path_discriminant_is_a_bare_literal` above, plus the
    // baked-value check that test doesn't make (a regression that renumbered
    // the tag would still pass a placeholder-only check).
    let want = crate::catalog::enum_member_value("EBREasingFunction", "Bounce")
        .expect("EasingFunction.Bounce is a real schema member");
    let r = compile("out d = EasingFunction.Bounce.Discriminant\n");
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(
        int_operands(&r).contains(&want),
        "d must bake Bounce's real schema discriminant ({want}), got {:?}",
        int_operands(&r)
    );

    // Parity: a hand-written enum with `Bounce` pinned to that SAME
    // discriminant must lower to the identical node/wire shape - the
    // built-in seed is not a special-cased path, it is the same registry
    // entry a user `enum` declaration would produce.
    let user = compile(&format!(
        "enum E {{ A, Bounce = {want} }}\nout d = E.Bounce.Discriminant\n"
    ));
    assert_no_errors(&user);
    assert!(!has_gate(&user, "_Unsupported"));
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
    let r = compile(
        "static var e: EasingFunction = EasingFunction.Linear\n\
         out m = e.Discriminant == EasingFunction.Bounce.Discriminant\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(
        baked_init(&r, "e.__disc"),
        Some(crate::ir::Literal::Int(linear)),
        "e.__disc must bake Linear's real schema discriminant ({linear}), not an auto-numbered index"
    );
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_CompareEqual"));
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
        assert!(!has_gate(r, "_Unsupported"));
        assert!(has_gate(r, "BrickComponentType_WireGraph_Exec_Branch"));
        assert!(has_gate(r, "BrickComponentType_WireGraph_Exec_Union"));
    }
    for class in [
        "BrickComponentType_WireGraph_Expr_CompareEqual",
        "BrickComponentType_WireGraph_Exec_Branch",
        "BrickComponentType_WireGraphPseudo_Var",
        "BrickComponentType_WireGraph_Exec_Union",
        "BrickComponentType_WireGraph_Exec_Var_Set",
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
    let r = compile(src);
    assert_no_errors(&r);
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
    let src = "in v: float\nout r = Remap(v, 0.0, 1.0, 0.0, 100.0, Function = EasingFunction.Bounce)\n";
    let r = compile(src);
    assert_no_errors(&r);
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


// Regression: a mod whose returns live inside a `let else` (or `if let` /
// match-statement) block must be put in early-return mode, or every `return`
// silently drops its value. A let-else mod called from exec must route both
// returns through ret_set storage so the caller reads a real value.
#[test]
fn let_else_mod_routes_returns_through_ret_set_storage() {
    let r = compile(
        "enum Opt { Has(int), Nothing }\n\
         mod unwrapOr(o: Opt, fb: int) -> int { let Has(x) = o else { return fb }\n return x }\n\
         static var opt: Opt = Opt.Has(7)\n\
         in go: exec\nvar r: int = 0\n\
         on go { r = unwrapOr(opt, 0) }\n",
    );
    assert_no_errors(&r);
    let ret_sets = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::VAR_SET && n.note == Some("ret_set"))
        .count();
    assert!(ret_sets >= 2, "both returns must write ret_set storage, got {ret_sets}");
}

#[test]
fn dot_value_on_an_enum_var_is_the_identity_not_a_placeholder() {
    // Regression: `.Value` on an enum resolved to nothing and lowered to an
    // `_Unsupported` placeholder, while typecheck typed it as the identity and
    // reported no error. A silent miscompile. Each source below is a distinct
    // position routing through `resolve_field_chain`.
    for src in [
        "enum D { A = 0, B = 1 }
         var d: D = A
         out v = match d.Value { A => 11.0, B => 22.0 }
",
        "enum D { A = 0, B = 1 }
         var d: D = A
         let e: D = d.Value
         out v = match e { A => 11.0, B => 22.0 }
",
        "enum D { A = 0, B = 1 }
         var d: D = A
         out v = d.Value.Discriminant
",
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        assert!(!has_wsp001(&r), "no WSP001 placeholder for:
{src}
got {:?}", r.diagnostics);
        assert!(!has_gate(&r, "_Unsupported"), "no placeholder gate for:
{src}");
    }
}

#[test]
fn dot_value_on_an_enum_var_matches_the_bare_name_lowering() {
    // The contract: a program is the same program with or without the
    // `.Value`. Same lowered shape, not merely something that avoids the
    // placeholder.
    let bare = compile(
        "enum D { A = 0, B = 1 }
         var d: D = A
         out v = match d { A => 11.0, B => 22.0 }
",
    );
    let dotted = compile(
        "enum D { A = 0, B = 1 }
         var d: D = A
         out v = match d.Value { A => 11.0, B => 22.0 }
",
    );
    assert_no_errors(&dotted);
    let classes = |r: &LowerResult| {
        let mut v: Vec<String> =
            r.module.nodes.values().map(|n| n.gate_class.to_string()).collect();
        v.sort();
        v
    };
    assert_eq!(
        classes(&bare),
        classes(&dotted),
        "`match d.Value` must lower identically to `match d`"
    );
}

#[test]
fn dot_value_on_an_enum_with_payloads_keeps_every_slot() {
    // The identity is the WHOLE enum, not just its tag: a payload capture must
    // still resolve through a `.Value`, or `.Value` would silently narrow the
    // value to its discriminant and the arm would read a dead slot.
    let bare = compile(
        "enum Shape { Empty, Circle(float), Box { w: float, h: float } }
         var s: Shape = Circle(3.0)
         out area = match s { Circle(r) => r, Box { w, h } => w, Empty => 0.0 }
",
    );
    let dotted = compile(
        "enum Shape { Empty, Circle(float), Box { w: float, h: float } }
         var s: Shape = Circle(3.0)
         out area = match s.Value { Circle(r) => r, Box { w, h } => w, Empty => 0.0 }
",
    );
    assert_no_errors(&dotted);
    assert!(!has_wsp001(&dotted), "diagnostics: {:?}", dotted.diagnostics);
    assert!(!has_gate(&dotted, "_Unsupported"));
    assert_eq!(dotted.module.nodes.len(), bare.module.nodes.len());
}

#[test]
fn dot_value_still_projects_a_real_value_field_of_a_record() {
    // The identity must not swallow a record that genuinely HAS a `Value`
    // field (`a.pop()` -> `{ Value, IsEmpty }`); that field read wins.
    let r = compile(
        "var a: int[]
         var got: int
         on RoundStart() {
           a.push(7)
           got = a.pop().Value
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
}

#[test]
fn record_typed_payload_field_writes_every_subfield() {
    // A record-typed payload slot is laid out per-field by `enum_payload_slot`
    // and read per-field by a pattern bind, so a construction has to WRITE it
    // per-field too. Lowering the field with `lower_expr` cannot: one
    // expression yields one port, so the record collapsed to an `_Unsupported`
    // placeholder and `assign_record_fields` silently skipped the slot, so the
    // string field assigned and the record field did not.
    let r = compile(
        "type Track = { origin: vector, direction: vector }
         enum St { A, B { next_track: Track, my_str: string } }
         var s: St = St.A
         on RoundStart() {
           s = St.B { next_track: { origin: Vec(4.0, 4.0, 4.0), direction: Vec(1.0, 0.0, 0.0) }, my_str: \"hi\" }
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
    assert_eq!(
        written_var_labels(&r),
        vec![
            "s.__B_my_str".to_string(),
            "s.__B_next_track.direction".to_string(),
            "s.__B_next_track.origin".to_string(),
            "s.__disc".to_string(),
        ],
    );
}

#[test]
fn record_typed_payload_field_writes_every_subfield_from_a_var() {
    // The same slot fed by an existing record VAR rather than a literal: the
    // source is already a `Binding::Record`, so this fails for the same
    // one-port reason and must be fixed by the same per-field path.
    let r = compile(
        "type Track = { origin: vector, direction: vector }
         enum St { A, B { next_track: Track } }
         var s: St = St.A
         var t: Track = { origin: Vec(0.0, 0.0, 0.0), direction: Vec(0.0, 0.0, 0.0) }
         on RoundStart() {
           s = St.B { next_track: t }
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
    assert_eq!(
        written_var_labels(&r),
        vec![
            "s.__B_next_track.direction".to_string(),
            "s.__B_next_track.origin".to_string(),
            "s.__disc".to_string(),
        ],
    );
}

#[test]
fn record_typed_positional_payload_writes_every_subfield() {
    // Positional payloads build the same `Vec<(String, PortRef)>`, so the
    // record collapse is not specific to the named-field syntax.
    let r = compile(
        "type Track = { origin: vector, direction: vector }
         enum St { A, B(Track) }
         var s: St = St.A
         var t: Track = { origin: Vec(0.0, 0.0, 0.0), direction: Vec(0.0, 0.0, 0.0) }
         on RoundStart() {
           s = St.B(t)
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
    assert_eq!(
        written_var_labels(&r),
        vec![
            "s.__B_0.direction".to_string(),
            "s.__B_0.origin".to_string(),
            "s.__disc".to_string(),
        ],
    );
}

#[test]
fn nested_enum_payload_field_writes_every_slot() {
    // The SILENT twin: a nested-enum payload lowers through
    // `try_lower_enum_ctor`, which succeeds and returns only the inner
    // `__disc` port while dropping its `pending_inline_record`. No
    // `_Unsupported`, so no WSP001 either: the construction wrote 2 of the 4
    // slots and typechecked clean.
    let r = compile(
        "enum Inner { X, Y(int) }
         enum Outer { A, B { i: Inner, s: string } }
         var o: Outer = Outer.A
         on RoundStart() {
           o = Outer.B { i: Inner.Y(42), s: \"hi\" }
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
    assert_eq!(
        written_var_labels(&r),
        vec![
            "o.__B_i.__Y_0".to_string(),
            "o.__B_i.__disc".to_string(),
            "o.__B_s".to_string(),
            "o.__disc".to_string(),
        ],
    );
}

#[test]
fn record_typed_payload_bakes_its_subfields_in_a_declaration_initializer() {
    // The declaration-initializer path is separate from assignment:
    // `enum_payload_slot`'s record arm builds its sub-field storage with
    // `init: None`, discarding the folded `Literal::Record`, so the payload
    // zeroed instead of baking. Nothing runs at load to repair it.
    let r = compile(
        "type P = { a: int, b: int }
         enum St { A, B { p: P } }
         var s: St = St.B { p: { a: 7, b: 9 } }
",
    );
    assert_no_errors(&r);
    assert_eq!(baked_init(&r, "s.__disc"), Some(crate::ir::Literal::Int(1)));
    assert_eq!(baked_init(&r, "s.__B_p.a"), Some(crate::ir::Literal::Int(7)));
    assert_eq!(baked_init(&r, "s.__B_p.b"), Some(crate::ir::Literal::Int(9)));
}

#[test]
fn record_payload_subfields_keep_their_own_values() {
    // Per-field copying has to preserve field IDENTITY, not merely emit one
    // write per slot: keying the copy wrong would still produce a write for
    // every sub-field and satisfy a label-only check while swapping the values.
    let r = compile(
        "type P = { a: int, b: int }
         enum St { A, B { p: P, n: int } }
         var s: St = St.A
         on RoundStart() {
           s = St.B { p: { a: 7, b: 9 }, n: 5 }
         }
",
    );
    assert_no_errors(&r);
    assert_eq!(written_value(&r, "s.__B_p.a"), Some(crate::ir::Literal::Int(7)));
    assert_eq!(written_value(&r, "s.__B_p.b"), Some(crate::ir::Literal::Int(9)));
    assert_eq!(written_value(&r, "s.__B_n"), Some(crate::ir::Literal::Int(5)));
    assert_eq!(written_value(&r, "s.__disc"), Some(crate::ir::Literal::Int(1)));
}

// an enum-element array / map

#[test]
fn enum_element_array_decomposes_into_parallel_columns() {
    // An enum value spans several wires exactly as a record does, so an
    // enum-element array needs one parallel array per column: the `__disc` plus
    // every variant's payload slot. A single ArrayVar collapsed the tag and the
    // payload into one column.
    let r = compile(
        "enum S { A(int), B(int) }
         var arr: S[]
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_ArrayVar"),
        3,
        "__disc + __A_0 + __B_0"
    );
}

#[test]
fn pushing_an_enum_onto_an_array_carries_its_value() {
    // The push emitted no `Value` wire at all, so both the tag and the payload
    // were discarded, and two orphan `Pseudo_Var` gates were left behind.
    let r = compile(
        "enum S { A(int), B(int) }
         in go: exec
         in u: int
         var arr: S[]
         on go { arr.push(S.A(u)) }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Push"),
        3,
        "one push per parallel column"
    );
    for (id, n) in &r.module.nodes {
        if n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push" {
            let wired = r
                .module
                .wires
                .iter()
                .any(|w| w.target.node_id == *id && w.target.port == WirePort::Value);
            // A constant column (the statically known `__disc`) inlines its
            // literal as gate data instead of spawning a source gate to wire,
            // the same way a constant assigned to a var does.
            let baked = n.properties.contains_key(&crate::intern::sym::VALUE);
            assert!(wired || baked, "push {id:?} carries no value at all");
        }
    }
}

#[test]
fn enum_element_map_decomposes_into_parallel_columns() {
    let r = compile(
        "enum S { A(int), B(int) }
         var m: Map<int, S>
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_MapVar"),
        3,
        "__disc + __A_0 + __B_0"
    );
}

#[test]
fn reading_an_enum_array_element_projects_its_tag() {
    // The read side was silent too: a bare `ArrayVar_Get` off the single
    // collapsed column fed the consumer, so `.Discriminant` took whatever that
    // column happened to hold. Reading an element now pulls every column, which
    // is what lets a later `match` on the value reach any arm's payload.
    let r = compile(
        "enum S { A(int), B(int) }
         in go: exec
         var arr: S[]
         var d: int = 0
         on go {
           arr.push(S.A(1))
           let e = arr[0]
           d = e.Discriminant
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
        3,
        "one read per parallel column"
    );
    assert_eq!(written_var_labels(&r), vec!["d".to_string()]);
}


#[test]
fn enum_field_of_a_record_array_gets_parallel_columns() {
    // A record array is one parallel array per leaf, and an enum leaf is
    // several columns, so it needs one array per tag and payload slot. The enum
    // storage arm was gated to a plain record VAR, so inside an array the field
    // fell through to the scalar arm and collapsed.
    let r = compile(
        "enum S { A(int), B(int) }
         type H = { s: S, k: int }
         var arr: H[]
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_ArrayVar"),
        4,
        "s.__disc + s.__A_0 + s.__B_0 + k"
    );
}

#[test]
fn pushing_a_record_with_an_enum_field_writes_every_column() {
    let r = compile(
        "enum S { A(int), B(int) }
         type H = { s: S, k: int }
         in go: exec
         in u: int
         var arr: H[]
         on go { arr.push({ s: S.A(u), k: u }) }
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Push"),
        4,
        "one push per parallel column"
    );
}

#[test]
fn enum_field_of_a_record_map_gets_parallel_columns() {
    let r = compile(
        "enum S { A(int), B(int) }
         type H = { s: S, k: int }
         var m: Map<int, H>
",
    );
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_MapVar"),
        4,
        "s.__disc + s.__A_0 + s.__B_0 + k"
    );
}

#[test]
fn match_on_an_inline_map_subscript_decomposes() {
    // A named binding of the same read (`let e = m[k]; match e`) resolves
    // through the scope, but the inline form had no name to resolve, and the
    // fallback only recognised an enum CONSTRUCTION, so a container element
    // scrutinee fell to the loud placeholder.
    let r = compile(
        "enum S { A(int), B(int) }
         in go: exec
         var m: Map<int, S>
         var got: int = 0
         on go {
           m.set(1, S.A(7))
           match m[1] {
             A(n) => { got = n }
             B(n) => { got = n }
           }
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
}

#[test]
fn match_on_an_inline_array_index_decomposes() {
    let r = compile(
        "enum S { A(int), B(int) }
         in go: exec
         var arr: S[]
         var got: int = 0
         on go {
           arr.push(S.A(7))
           match arr[0] {
             A(n) => { got = n }
             B(n) => { got = n }
           }
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
}

#[test]
fn if_let_on_a_map_get_value_decomposes() {
    // `m.get(k)` yields a `{Value, Found}` record, so the enum is one `.Value`
    // projection down. That projection produced no enum record and the bind
    // lowered to a placeholder.
    let r = compile(
        "enum S { A(int), B(int) }
         in go: exec
         var m: Map<int, S>
         var got: int = 0
         on go {
           m.set(1, S.A(7))
           let e = m.get(1).Value
           if let A(n) = e { got = n }
         }
",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"), "diagnostics: {:?}", r.diagnostics);
}

#[test]
fn if_let_capture_write_sets_the_scrutinees_payload_slot() {
    // Payload mutation goes through the destructure: the capture IS the
    // scrutinee's `__B_b` slot, so `b = true` inside the arm is a `Var_Set`
    // wired to that gate. `written_var_labels` reads the label off the
    // Pseudo_Var actually feeding each Var_Set's VarRef, so a regression that
    // bound the capture to a fresh local would still emit a Var_Set and still
    // fail here.
    let r = compile(
        "enum E { A, B { s: string, b: bool } }\n\
         var e: E = E.B { s: \"hi\", b: false }\n\
         in go: exec\n\
         on go { if let B { s, b } = e { b = true } }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    // Only the written slot is written; `e.__B_s` and `e.__disc` are untouched.
    assert_eq!(written_var_labels(&r), vec!["e.__B_b".to_string()]);
}

#[test]
fn match_and_let_else_captures_write_the_same_slot() {
    // The other two destructure forms share the pattern-binding path, so they
    // must reach the same slot.
    let pre = "enum E { A, B { s: string, b: bool } }\n\
               var e: E = E.B { s: \"hi\", b: false }\n\
               in go: exec\n";
    let m = compile(&format!(
        "{pre}on go {{ match e {{ B {{ s, b }} => {{ b = true }}, A => {{}} }} }}\n"
    ));
    assert_no_errors(&m);
    assert_eq!(written_var_labels(&m), vec!["e.__B_b".to_string()]);

    let le = compile(&format!(
        "{pre}on go {{ let B {{ s, b }} = e else {{ return }}\nb = true }}\n"
    ));
    assert_no_errors(&le);
    assert_eq!(written_var_labels(&le), vec!["e.__B_b".to_string()]);
}

#[test]
fn record_payload_capture_write_fans_across_the_fields() {
    // A record-typed payload slot is itself a `Binding::Record` of per-field
    // gates, so assigning the capture a record literal decomposes into one
    // `Var_Set` per leaf, the same fan-out a plain record var assignment gets.
    let r = compile(
        "type P = { x: int, y: int }\n\
         enum E { A, B { p: P } }\n\
         var e: E = E.B { p: { x: 0, y: 0 } }\n\
         in go: exec\n\
         on go { if let B { p } = e { p = { x: 7, y: 9 } } }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(
        written_var_labels(&r),
        vec!["e.__B_p.x".to_string(), "e.__B_p.y".to_string()],
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
fn unsafe_payload_write_sets_only_the_named_slot() {
    // The unchecked write touches the slot and nothing else: no Branch (the tag
    // is asserted, not tested) and no write to `__disc`.
    let r = compile(
        "enum E { A, B { s: string, n: int } }\n\
         var e: E = E.B { s: \"hi\", n: 1 }\n\
         in go: exec\n\
         on go { unsafe e.B.s = \"written\" }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(written_var_labels(&r), vec!["e.__B_s".to_string()]);
    assert!(!has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"));
}

#[test]
fn unsafe_payload_read_gets_the_named_slot() {
    let r = compile(
        "enum E { A, B { s: string } }\n\
         var e: E = E.B { s: \"hi\" }\n\
         var got: string = \"\"\n\
         in go: exec\n\
         on go { got = unsafe e.B.s }\n",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert_eq!(written_var_labels(&r), vec!["got".to_string()]);
    // The value read comes from the payload slot's own gate.
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"));
}
