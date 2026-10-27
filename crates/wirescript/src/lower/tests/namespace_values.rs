//! `import * as ns` VALUE members (`ns.myValue`), as opposed to `ns.f(...)`
//! calls: each types as the member's type and lowers to its real gate, not an
//! `_Unsupported` placeholder that silently reads 0.

use super::*;
use crate::resolve::{MemLoader, resolve};

/// Resolve + typecheck + lower a two-file program (`lib.ws` + main).
fn compile_with_lib(
    lib_src: &str,
    main_src: &str,
) -> (crate::typecheck::TypeCheckResult, LowerResult) {
    compile_with_libs(&[("lib.ws", lib_src)], main_src)
}

/// Resolve + typecheck + lower a program importing several library modules.
fn compile_with_libs(
    libs: &[(&str, &str)],
    main_src: &str,
) -> (crate::typecheck::TypeCheckResult, LowerResult) {
    let loader = MemLoader {
        files: libs
            .iter()
            .map(|(name, src)| (name.to_string(), src.to_string()))
            .collect(),
    };
    // Namespace-resolution structure tests; force folding off so constant
    // chip/value bodies aren't optimized away out from under the asserts.
    let crate::compile::FrontEnd {
        resolved,
        typecheck: tc,
        lowered: lr,
        ..
    } = crate::compile::front_end(main_src, "test", None, FoldMode::ForceOff, &loader);
    assert!(
        resolved.diagnostics.is_empty(),
        "import should resolve: {:?}",
        resolved.diagnostics
    );
    (tc, lr)
}

#[track_caller]
fn assert_clean(tc: &crate::typecheck::TypeCheckResult, lr: &LowerResult) {
    let errors = [
        crate::test_support::error_codes(&tc.diagnostics),
        crate::test_support::error_codes(&lr.diagnostics),
    ];
    assert!(
        errors.iter().all(std::vec::Vec::is_empty),
        "typecheck errors: {:?}\nlower errors: {:?}",
        tc.diagnostics,
        lr.diagnostics
    );
}

/// Every `Literal::Int` baked into any node property, anywhere in the tree.
fn baked_ints(m: &crate::ir::Module) -> Vec<i64> {
    m.nodes_deep()
        .flat_map(|n| n.properties.values())
        .filter_map(|v| match v {
            crate::ir::Literal::Int(i) => Some(*i),
            _ => None,
        })
        .collect()
}

/// A namespaced value member resolves to its own value, wherever it is read,
/// instead of an `_Unsupported` placeholder that read 0: each row checks clean,
/// with no placeholder, and bakes every listed integer.
#[test]
fn a_namespaced_value_member_lowers_to_its_value() {
    let rows: &[(&str, &[(&str, &str)], &str, &[i64])] = &[
        (
            "a namespaced scalar `let`",
            &[("lib.ws", "let answer: int = 42")],
            "import * as lib from \"lib\"\nout o = lib.answer",
            &[42],
        ),
        (
            "`lib.rec.value`: the namespace hop resolves the record binding, then the record-field \
             walk continues from there",
            &[("lib.ws", "type V = {value: int}\nlet rec: V = {value: 42}")],
            "import * as lib from \"lib\"\nout o = lib.rec.value",
            &[42],
        ),
        (
            "a namespaced value typed `any` made every use against a concrete type a mismatch",
            &[("lib.ws", "type V = {value: int}\nlet rec: V = {value: 42}")],
            "import * as lib from \"lib\"\ntype Outer = {value: lib.V}\nlet r: Outer = {value: lib.rec}\nout o = r.value.value",
            &[42],
        ),
        (
            "the `any` typing showed up as a WS003 at a typed parameter",
            &[("lib.ws", "let answer: int = 42")],
            "import * as lib from \"lib\"\nmod takesInt(v: int) -> int { return v + 1 }\nout o = takesInt(lib.answer)",
            &[42],
        ),
        (
            "two modules each export `empty` of unrelated record types: lowering dumped every \
             namespace's values into one bare-name scope where the last import won, so the first \
             one's field read a placeholder while typecheck called the file clean",
            &[
                (
                    "sym.ws",
                    "type Sym = { layer1: string }\nlet empty: Sym = { layer1: \"L\" }",
                ),
                (
                    "tun.ws",
                    "type Tun = { topart: string }\nlet empty: Tun = { topart: \"T\" }",
                ),
            ],
            "import * as Sym from \"sym\"\nimport * as Tun from \"tun\"\nvar sink: string[]\nlet g = ReadBrickGrid()\n\
             on g {\n  sink.push(\"${Sym.empty.layer1}\")\n  sink.push(\"${Tun.empty.topart}\")\n}",
            &[],
        ),
        (
            "a namespaced mod reading its module's `let` bakes THAT module's value",
            &[
                ("mc.ws", "let g: int = 111\nmod getG() -> int { return g }"),
                ("nc.ws", "let g: int = 222\nmod getG() -> int { return g }"),
            ],
            "import * as A from \"mc\"\nimport * as B from \"nc\"\nout ra = A.getG()\nout rb = B.getG()",
            &[111, 222],
        ),
        (
            "a namespaced `out` member (`L.count`): `In`/`Out` were absent from the typecheck \
             namespace map, so it was WS002 plus a placeholder",
            &[(
                "lib.ws",
                "var counter: int = 0\nin tick: exec\non tick { counter = counter + 1 }\nout count: int = counter",
            )],
            "import * as L from \"lib\"\nout shown: int = L.count",
            &[],
        ),
        (
            "a namespaced `in` member read as a value",
            &[("lib.ws", "in level: int")],
            "import * as L from \"lib\"\nout shown: int = L.level",
            &[],
        ),
        (
            "the N11 seal still lets a traveling namespace serve its origin file: `useB()`'s \
             `B.bar()` lives in a.ws, where `B` is in scope",
            &[
                (
                    "a.ws",
                    "import * as B from \"b\"\nmod useB() -> int { return B.bar() }",
                ),
                ("b.ws", "mod bar() -> int { return 7 }"),
            ],
            "import \"a\"\nout r: int = useB()",
            &[7],
        ),
    ];
    let mut failures = Vec::new();
    for (label, libs, main, ints) in rows {
        let (tc, lr) = compile_with_libs(libs, main);
        let errors = [
            crate::test_support::error_codes(&tc.diagnostics),
            crate::test_support::error_codes(&lr.diagnostics),
        ];
        if !errors.iter().all(std::vec::Vec::is_empty) {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        if has_unsupported(&lr.module) {
            failures.push(format!("{label}: a placeholder"));
        }
        let baked = baked_ints(&lr.module);
        for v in *ints {
            if !baked.contains(v) {
                failures.push(format!("{label}: {v} is not baked, got {baked:?}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// An imported module's root-level `in` port, triggered as `on ns.port`: the
/// namespace lowering declares its `in`/`out` ports, so the trigger matches and
/// the handler body lowers. An unmatched trigger silently drops the whole body.
#[test]
fn namespaced_input_port_can_trigger_a_handler() {
    let (tc, lr) = compile_with_lib(
        "in trigger: exec",
        "import * as L from \"lib\"\non L.trigger { PrintToConsole(\"fired\") }",
    );
    assert_clean(&tc, &lr);
    let input = find_gate(&lr, gc::MICROCHIP_INPUT);
    let print = find_gate(&lr, gc::PRINT_TO_CONSOLE);
    assert!(
        exec_reaches(&lr.module, input, print),
        "the imported input must drive the handler body: `on ns.trigger` used to drop it entirely"
    );
}

/// An imported module's root-level `out` port is declared, and its initializer
/// is wired. Ordering matters: the value may name a member declared after it,
/// so outputs are wired only once the whole module is in scope.
#[test]
fn namespaced_output_port_is_declared_and_wired() {
    let (tc, lr) = compile_with_lib(
        "var counter: int = 0\nout total = counter",
        "import * as L from \"lib\"\nin go: exec\non go { L.counter = L.counter + 1 }",
    );
    assert_clean(&tc, &lr);
    let out_id = lr
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::MICROCHIP_OUTPUT)
        .map(|(id, _)| *id)
        .expect("the imported `out` port must be declared");
    assert!(
        lr.module.wires.iter().any(|w| w.target.node_id == out_id),
        "the imported output's initializer must be wired into it"
    );
}

#[test]
fn namespaced_chip_calls_still_work() {
    let (tc, lr) = compile_with_lib(
        "chip Double(x: int) -> (result: int) { out result = x + x }",
        "import * as lib from \"lib\"\nlet r = lib.Double(5)\nout o = r.result",
    );
    assert_clean(&tc, &lr);
    assert_eq!(
        lr.module.chips.len(),
        1,
        "a namespaced chip call must still instantiate its chip"
    );
    let chip = only_chip(&lr, "the Double instance");
    assert_eq!(
        count_gate(chip, "BrickComponentType_Expr_MathAdd"),
        1,
        "the chip body's `x + x`"
    );
}

const VAR: &str = gc::VAR;
const VAR_SET: &str = gc::VAR_SET;
const OUTPUT: &str = gc::MICROCHIP_OUTPUT;

/// How many gates of `class` the module `m` itself holds (not its chips).
fn count_gate(m: &crate::ir::Module, class: &str) -> usize {
    m.nodes.values().filter(|n| n.gate_class == class).count()
}

/// The source node feeding `target`'s incoming wire on port `port` (the first
/// such wire), if any.
fn feed_source(
    m: &crate::ir::Module,
    target: crate::ir::NodeId,
    port: crate::ir::port_registry::WirePort,
) -> Option<crate::ir::NodeId> {
    m.wires
        .iter()
        .find(|w| w.target.node_id == target && w.target.port == port)
        .map(|w| w.source.node_id)
}

/// Two DIFFERENT modules each declaring `var g`, imported as two namespaces:
/// `A.g` and `B.g` must be DISTINCT storage gates for both reads and writes.
/// The old lowering skipped the second module's `var` (its bare name was
/// already bound by the first) and let `B.g` alias `A.g`'s single `Pseudo_Var`,
/// so writing one silently changed the other.
#[test]
fn two_namespaces_same_var_name_get_distinct_gates() {
    let (tc, lr) = compile_with_libs(
        &[("amod.ws", "var g: int = 0"), ("bmod.ws", "var g: int = 0")],
        "import * as A from \"amod\"\n\
         import * as B from \"bmod\"\n\
         in go: exec\n\
         out ra = A.g\n\
         out rb = B.g\n\
         on go { A.g = 5\n B.g = 9 }",
    );
    assert_clean(&tc, &lr);
    assert!(!has_unsupported(&lr.module));
    assert_eq!(
        count_gate(&lr.module, VAR),
        2,
        "A.g and B.g must be two distinct storage gates, not one collapsed gate"
    );
    // The two reads (`out ra = A.g`, `out rb = B.g`) come from distinct gates.
    let outs: Vec<crate::ir::NodeId> = lr
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == OUTPUT)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(outs.len(), 2);
    let r0 = feed_source(
        &lr.module,
        outs[0],
        crate::ir::port_registry::WirePort::RerInput,
    );
    let r1 = feed_source(
        &lr.module,
        outs[1],
        crate::ir::port_registry::WirePort::RerInput,
    );
    assert!(
        r0.is_some() && r1.is_some() && r0 != r1,
        "the two namespaced reads must resolve to distinct storage gates"
    );
    // The two writes (`A.g = 5`, `B.g = 9`) target distinct storage gates.
    let sets: Vec<crate::ir::NodeId> = lr
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == VAR_SET)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(sets.len(), 2, "each namespaced write is its own Var_Set");
    let w0 = feed_source(
        &lr.module,
        sets[0],
        crate::ir::port_registry::WirePort::VarRef,
    );
    let w1 = feed_source(
        &lr.module,
        sets[1],
        crate::ir::port_registry::WirePort::VarRef,
    );
    assert!(
        w0.is_some() && w1.is_some() && w0 != w1,
        "the two namespaced writes must target distinct storage gates"
    );
}

/// A local `var g` and an imported `A.g` of the same name must be distinct
/// gates. The importer OWNS the bare `g`, so after the namespace lowers its own
/// `g` (a fresh gate captured as `A.g`) the bare `g` is restored to the local.
#[test]
fn local_var_and_namespaced_var_same_name_stay_distinct() {
    let (tc, lr) = compile_with_libs(
        &[("amod.ws", "var g: int = 0")],
        "import * as A from \"amod\"\n\
         var g: int = 0\n\
         in go: exec\n\
         out mine = g\n\
         out theirs = A.g\n\
         on go { g = 1\n A.g = 2 }",
    );
    assert_clean(&tc, &lr);
    assert!(!has_unsupported(&lr.module));
    assert_eq!(
        count_gate(&lr.module, VAR),
        2,
        "the local `g` and the imported `A.g` must be two distinct gates"
    );
    let sets: Vec<crate::ir::NodeId> = lr
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == VAR_SET)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(sets.len(), 2);
    let w0 = feed_source(
        &lr.module,
        sets[0],
        crate::ir::port_registry::WirePort::VarRef,
    );
    let w1 = feed_source(
        &lr.module,
        sets[1],
        crate::ir::port_registry::WirePort::VarRef,
    );
    assert!(
        w0.is_some() && w1.is_some() && w0 != w1,
        "`g = 1` and `A.g = 2` must write distinct gates"
    );
}

/// A namespaced import's top-level `on` handler runs as part of the importing
/// program: importing a module as a namespace runs its handlers.
#[test]
fn namespaced_import_on_handler_runs() {
    let (tc, lr) = compile_with_lib(
        "var ticks: int = 0\nin go: exec\non go { ticks = ticks + 1 }\nout t = ticks",
        "import * as L from \"lib\"",
    );
    assert_clean(&tc, &lr);
    let inc = find_gate(&lr, gc::VAR_INCREMENT);
    let go = find_gate(&lr, gc::MICROCHIP_INPUT);
    assert!(
        exec_reaches(&lr.module, go, inc),
        "the namespaced module's `on go` handler body must lower off `go`"
    );
}

/// A namespaced module's handler and the importer's own handler for the same
/// trigger both run; neither is dropped, and each resolves its own module's
/// state.
#[test]
fn namespaced_and_local_handlers_both_run() {
    let (tc, lr) = compile_with_lib(
        "on ReadBrickGrid() { BroadcastChatMessage(\"lib\") }",
        "import * as Other from \"lib\"\non ReadBrickGrid() { BroadcastChatMessage(\"main\") }",
    );
    assert_clean(&tc, &lr);
    let broadcasts = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class.contains("BroadcastChatMessage"))
        .count();
    assert_eq!(
        broadcasts, 2,
        "both the local and the imported `on ReadBrickGrid` handler must generate"
    );
}

/// An anonymous chip (`chip on t { ... }`) in a namespaced module installs its
/// behaviour just like a top-level handler, writes included.
#[test]
fn namespaced_import_anon_chip_runs() {
    let (tc, lr) = compile_with_lib(
        "var v: int = 0\nin t: exec\nchip on t { v = 1 }\nout cur = v",
        "import * as N from \"lib\"",
    );
    assert_clean(&tc, &lr);
    let chip = only_chip(&lr, "the namespaced anon chip must instantiate a microchip");
    assert_eq!(lr.module.chips.len(), 1);
    assert_eq!(
        count_gate(chip, VAR_SET),
        1,
        "the namespaced anon chip's `v = 1` write must lower"
    );
}

/// Two modules with a same-named private `helper`, each called by a public mod:
/// `A.f` runs A's `helper` and `B.g` runs B's. Each mod body is lowered with its
/// own module's members pushed into the frame, not a shared bare scope where the
/// last namespace wins.
#[test]
fn namespaced_sibling_mod_resolves_its_own_module() {
    let (tc, lr) = compile_with_libs(
        &[
            (
                "m.ws",
                "mod helper(x: int) -> int { return x + 1 }\nmod f(x: int) -> int { return helper(x) }",
            ),
            (
                "n.ws",
                "mod helper(x: int) -> int { return x * 100 }\nmod g(x: int) -> int { return helper(x) }",
            ),
        ],
        "import * as A from \"m\"\n\
         import * as B from \"n\"\n\
         in v: int\n\
         out ra = A.f(v)\n\
         out rb = B.g(v)",
    );
    assert_clean(&tc, &lr);
    assert_eq!(
        count_class(&lr.module, "BrickComponentType_Expr_MathAdd"),
        1,
        "A.f must run m's `helper` (x + 1)"
    );
    assert_eq!(
        count_class(&lr.module, "BrickComponentType_Expr_MathMultiply"),
        1,
        "B.g must run n's `helper` (x * 100), not m's"
    );
}

/// A namespaced mod that mutates its module's `var` writes ITS module's gate.
/// Both modules declare `var g` + `mod bump` (the name collision), but only
/// `A.g` is read out; `A.bump()`'s increment targets the very gate `A.g` reads,
/// not whichever namespace lowered last.
#[test]
fn namespaced_mod_mutates_its_own_module_var() {
    let (tc, lr) = compile_with_libs(
        &[
            ("mv.ws", "var g: int = 0\nmod bump() { g = g + 1 }"),
            ("nv.ws", "var g: int = 0\nmod bump() { g = g + 1 }"),
        ],
        "import * as A from \"mv\"\nimport * as B from \"nv\"\nin go: exec\nout a = A.g\non go { A.bump() }",
    );
    assert_clean(&tc, &lr);
    assert_eq!(
        count_class(&lr.module, gc::VAR),
        2,
        "A.g and B.g are distinct storage gates"
    );
    // The gate `out a = A.g` reads.
    let out_id = find_gate(&lr, gc::MICROCHIP_OUTPUT);
    let a_gate = lr
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == out_id)
        .map(|w| w.source.node_id)
        .expect("out a must be wired from A.g");
    // The gate A.bump()'s increment writes.
    let inc = lr
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::VAR_INCREMENT)
        .map(|(id, _)| *id)
        .expect("A.bump() must lower to a Var_Increment");
    let inc_target = lr
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == inc && w.target.port == crate::ir::port_registry::WirePort::VarRef
        })
        .map(|w| w.source.node_id)
        .expect("the increment must target a storage gate");
    assert_eq!(
        inc_target, a_gate,
        "A.bump() must write the gate A.g reads, not the other module's"
    );
}

/// An operator inside a namespaced module's `on` handler is typechecked (its
/// `op_resolutions` recorded) so it lowers to a real gate: typecheck descends
/// into the handler body, and `n << 10` is no `_Unsupported`.
#[test]
fn namespaced_handler_operator_lowers_to_a_gate() {
    let (tc, lr) = compile_with_lib(
        "let n = 3\nvar arr: int[]\non ReadBrickGrid() { arr.push(n << 10) }",
        "import * as L from \"lib\"",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "an operator in a namespaced handler must not lower to _Unsupported"
    );
    assert!(
        lr.module
            .nodes
            .values()
            .any(|n| n.gate_class.contains("BitwiseShiftLeft")),
        "`n << 10` must lower to a shift gate"
    );
}

/// A free name in a namespaced module's `on` handler does NOT resolve to the
/// importer's same-named top-level state: `lib`'s `on go { q = q + 1 }` (with no
/// `go`/`q` of its own) is checked under a sealed scope floor, so those names
/// are undefined and reported (WS001 for the trigger, WS002 for the body).
#[test]
fn namespaced_handler_free_name_does_not_leak_to_importer_state() {
    let (tc, _lr) = compile_with_lib(
        "on go { q = q + 1 }",
        "import * as L from \"lib\"\nin go: exec\nvar q: int = 0\nout result: int = q",
    );
    // The leaked body identifier is undefined, and the leaked trigger unknown.
    crate::test_support::assert_code(&tc.diagnostics, "WS002");
    crate::test_support::assert_code(&tc.diagnostics, "WS001");
}

/// The SAME file's state imported both plain and via `import * as` resolves to
/// ONE gate (docs promise one shared gate), and its handler installs once: `g`
/// and `S.g` do not diverge into two gates that each count every `bump`.
#[test]
fn same_file_state_via_plain_and_namespace_shares_one_gate() {
    let (tc, lr) = compile_with_lib(
        "var g: int = 0\nin bump: exec\non bump { g = g + 1 }",
        "import \"lib\"\nimport * as S from \"lib\"\nout a: int = g\nout b: int = S.g",
    );
    assert_clean(&tc, &lr);
    let vars = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR)
        .count();
    assert_eq!(vars, 1, "g and S.g must share ONE storage gate, got {vars}");
    let incs = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_INCREMENT)
        .count();
    assert_eq!(
        incs, 1,
        "the imported handler must install once, got {incs}"
    );
}

/// N3: `import { g }` + `import { g as h }` of the same var must share ONE
/// gate. The old dedup keyed on the (aliased) name, so `h` slipped past it and
/// got its own gate diverging from `g`.
#[test]
fn same_file_var_via_named_and_aliased_shares_one_gate() {
    let (tc, lr) = compile_with_lib(
        "var g: int = 0",
        "import { g } from \"lib\"\nimport { g as h } from \"lib\"\nout a: int = g\nout b: int = h",
    );
    assert_clean(&tc, &lr);
    let vars = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR)
        .count();
    assert_eq!(
        vars, 1,
        "g and its alias h must share ONE storage gate, got {vars}"
    );
}

/// A namespace a module privately imports (`a` does `import * as B`) does not
/// leak to a plain importer of that module. `main` imports `a` but not `b`, so
/// its own `B.bar()` is an unknown identifier, not a silent resolution of the
/// alias that traveled in for `a`'s `useB`.
#[test]
fn private_namespace_does_not_leak_to_plain_importer() {
    let (tc, _lr) = compile_with_libs(
        &[
            (
                "a.ws",
                "import * as B from \"b\"\nmod useB() -> int { return B.bar() }",
            ),
            ("b.ws", "mod bar() -> int { return 7 }"),
        ],
        "import \"a\"\nout r: int = B.bar()",
    );
    assert!(
        tc.diagnostics
            .iter()
            .any(|d| d.code == "WS002" && d.message.contains("'B'")),
        "main's direct B.bar() must be WS002 (B private to a.ws), got {:?}",
        tc.diagnostics
    );
}

/// A namespaced defaulted output with NO emit is a plain direct drive: one
/// driver, no backing var. (The 4b backing-var pass must not fire on it just
/// because it has a default - only an actually-emitted output needs backing.)
#[test]
fn namespaced_defaulted_output_no_emit_has_single_driver() {
    let (tc, lr) = compile_with_lib(
        "var counter: int = 0\nout count: int = counter",
        "import * as L from \"lib\"",
    );
    assert_clean(&tc, &lr);
    let out_id = lr
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_OUTPUT)
        .map(|n| n.id)
        .expect("the namespaced output must exist");
    let drivers = lr
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.node_id == out_id
                && w.target.port == crate::ir::port_registry::WirePort::RerInput
        })
        .count();
    assert_eq!(
        drivers, 1,
        "a no-emit defaulted output must have one direct driver"
    );
    assert!(
        !lr.module
            .nodes
            .values()
            .any(|n| n.note == Some("out_backing")),
        "a no-emit defaulted output must not get a backing var"
    );
}

/// A namespaced `out` reached by a default plus an `emit` is var-backed, not
/// fanned in: the backing-var prescan descends into `ns.decls`, so `out r = 0` +
/// `emit r = 5` do not drive the output rerouter from two sources (a load-time
/// fan-in).
#[test]
fn namespaced_output_default_plus_emit_has_single_driver() {
    let (tc, lr) = compile_with_lib(
        "in go: exec\nout r: int = 0\non go { emit r = 5 }",
        "import * as L from \"lib\"",
    );
    assert_clean(&tc, &lr);
    let out_id = lr
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_OUTPUT)
        .map(|n| n.id)
        .expect("the namespaced output must be a port of the module");
    let drivers = lr
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.node_id == out_id
                && w.target.port == crate::ir::port_registry::WirePort::RerInput
        })
        .count();
    assert_eq!(
        drivers, 1,
        "a namespaced defaulted out + emit must be var-backed (one driver), not a fan-in"
    );
    assert!(
        lr.module
            .nodes
            .values()
            .any(|n| n.note == Some("out_backing")),
        "a backing var must have been created for the namespaced output"
    );
}

/// The N1 seal must still let a namespaced handler reach its OWN module's
/// members: `lib`'s `on tick { counter = counter + 1 }` wires its increment to
/// lib's own `tick`/`counter`. Guards against over-sealing.
#[test]
fn namespaced_handler_wires_its_own_members() {
    let (tc, lr) = compile_with_lib(
        "in tick: exec\nvar counter: int = 0\non tick { counter = counter + 1 }\nout count: int = counter",
        "import * as L from \"lib\"\nout shown: int = L.counter",
    );
    assert_clean(&tc, &lr);
    let inc = find_gate(&lr, gc::VAR_INCREMENT);
    let tick = find_gate(&lr, gc::MICROCHIP_INPUT);
    assert!(
        exec_reaches(&lr.module, tick, inc),
        "lib's handler runs off lib's own `tick`"
    );
    let counter = lr
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == inc && w.target.port == WirePort::VarRef);
    assert_eq!(
        counter.map(|w| lr.module.nodes[&w.source.node_id].gate_class),
        Some(VAR),
        "the increment writes lib's own `counter`"
    );
}

/// A namespace reached TWO levels deep. `main` imports `mid` as a namespace,
/// and `mid` imports `leaf` as one of its own; `mid`'s members name `L.` to
/// reach it. That inner alias travels in beside `M`, but the seal a namespaced
/// body is checked under hid every outer frame, so `L` read as an unknown
/// identifier and every use of it lowered to a placeholder.
#[test]
fn transitive_namespace_resolves_in_a_namespaced_body() {
    let (tc, lr) = compile_with_libs(
        &[
            (
                "leaf.ws",
                "let value = 10\nmod bump(n: int) -> (r: int) { return n + value }",
            ),
            (
                "mid.ws",
                "import * as L from \"leaf\"\n\
                 let topLet = L.value\n\
                 mod useLet(n: int) -> (r: int) { return n + L.value }\n\
                 mod useMod(n: int) -> (r: int) { return L.bump(n) }",
            ),
        ],
        "import * as M from \"mid\"\n\
         var a: int = 0\nvar b: int = 0\nvar c: int = 0\n\
         in go: exec\n\
         on go { a = M.useLet(1)\n b = M.useMod(2)\n c = M.topLet }",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "a transitive namespace read must lower to real gates: {:?}",
        lr.diagnostics
    );
    // Every one of the three reads drives its own store, so none was dropped.
    assert_eq!(
        count_class(&lr.module, VAR_SET),
        3,
        "each of `a`/`b`/`c` must be written"
    );
    // `useLet`'s `n + L.value` and `bump`'s `n + value` are real gates, so the
    // transitive read reached arithmetic rather than folding to a placeholder.
    assert_eq!(
        count_class(&lr.module, "BrickComponentType_Expr_MathAdd"),
        2,
        "the inlined `n + …` adds"
    );
}

/// The alias COLLIDES: the importer and the module it imports both spell their
/// `import * as` the same. An alias is FILE-LOCAL, so this is not a conflict:
/// each file's `Other` means its own import, and a reference through the
/// traveling one resolves.
#[test]
fn colliding_namespace_alias_resolves_per_file() {
    let (tc, lr) = compile_with_libs(
        &[
            (
                "leaf.ws",
                "let value = 10
mod bump(n: int) -> (r: int) { return n + value }",
            ),
            (
                "mid.ws",
                "import * as Other from \"leaf\"
                 let value = Other.value
                 mod useIt(n: int) -> (r: int) { return Other.bump(n) }",
            ),
        ],
        "import * as Other from \"mid\"
         var a: int = 0
         in go: exec
         on go { a = Other.useIt(5) }
         out z = Other.value",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "both `Other`s must resolve: {:?}",
        lr.diagnostics
    );
    // `leaf`'s `n + value` really inlined, so the call went through the INNER
    // alias rather than resolving back to the importer's same-named one. The
    // other MathAdd carries the constant `out z` reads.
    let adds = nodes_of(&lr.module, "BrickComponentType_Expr_MathAdd");
    let carriers = adds
        .iter()
        .filter(|n| {
            n.properties.get(&*crate::intern::sym::INPUT_A) == Some(&crate::ir::Literal::Int(10))
        })
        .filter(|n| !lr.module.wires_deep().any(|w| w.target.node_id == n.id))
        .count();
    assert_eq!(
        (adds.len(), carriers),
        (2, 1),
        "the inlined body's `n + value` and the `out z` constant's carrier"
    );
}

/// Two `import * as` in ONE file under the same alias IS a conflict, unlike
/// `colliding_namespace_alias_resolves_per_file` where the two are written in
/// different files and are therefore independent. Unreported, the second
/// shadows the first and every reference through the alias reads whichever
/// module came last.
#[test]
fn duplicate_namespace_alias_in_one_file_reports() {
    let mut files = std::collections::HashMap::default();
    files.insert("a.ws".to_string(), "let value = 1".to_string());
    files.insert("b.ws".to_string(), "let value = 10".to_string());
    let loader = MemLoader { files };
    let r = resolve(
        "import * as Other from \"a\"
         import * as Other from \"b\"
         out v: int = Other.value",
        "test",
        &loader,
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS012" && d.message.contains("already bound in this file")),
        "expected a WS012 for the same-file duplicate alias, got {:?}",
        r.diagnostics
    );
}

/// The same module reached twice under one alias is a redundant import, not a
/// collision. Compared by canonical target, so two spellings of one file do
/// not report either.
#[test]
fn repeated_namespace_import_of_one_module_is_not_a_conflict() {
    let mut files = std::collections::HashMap::default();
    files.insert("a.ws".to_string(), "let value = 1".to_string());
    let loader = MemLoader { files };
    let r = resolve(
        "import * as Other from \"a\"
         import * as Other from \"a\"
         out v: int = Other.value",
        "test",
        &loader,
    );
    assert!(
        r.diagnostics.is_empty(),
        "importing one module twice under one alias is redundant, not a conflict: {:?}",
        r.diagnostics
    );
}

/// A namespace-qualified RECORD type (`Other.Value`) decomposes into per-field
/// containers exactly as the same alias does when it is imported by name.
#[test]
fn namespaced_record_type_decomposes_like_a_named_import() {
    let (_, lr) = compile_with_libs(
        &[("lib.ws", "type Value = {value: int}")],
        "import * as Other from \"lib\"
         type ValueHolder = {value: Other.Value}
         var valueholders: ValueHolder[] = [{value: {value: 10}}]",
    );
    let arrays: Vec<_> = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_ArrayVar")
        .collect();
    assert_eq!(arrays.len(), 1, "one array per leaf field: {arrays:?}");
    let a = arrays[0];
    // Decomposition must reach the LEAF field, not stop at the first level.
    assert_eq!(
        a.properties.get(&*crate::intern::sym::NAME_LABEL),
        Some(&crate::ir::Literal::String(
            "valueholders.value.value".into()
        )),
        "label shows how deep decomposition got: {:?}",
        a.properties
    );
    // An `any` element type ships a float array for a declared `int`.
    assert!(
        a.ports
            .outputs
            .iter()
            .any(|p| p.ty == Type::Ref(Box::new(Type::Array(Box::new(Type::Int))))),
        "element type must be int, not any: {:?}",
        a.ports.outputs
    );
    // The game cannot load a `Record` inside an array variant; only the
    // flattened leaf value is storable.
    assert_eq!(
        a.properties.get(&crate::intern::intern("InitialValue")),
        Some(&crate::ir::Literal::Array(vec![crate::ir::Literal::Int(
            10
        )])),
        "initializer must bake the leaf value: {:?}",
        a.properties
    );
}

/// A namespace's own `out` DECLARATION is lowered in pure position, never
/// inheriting a preceding namespace's leftover exec chain: with namespace A's
/// handler-end exec in `current_exec`, namespace B's declared `out n: int = 5`
/// (backed by a var B's handler writes) still bakes its `InitialValue` rather
/// than taking a `Var_Set` fed by A's unrelated trigger.
#[test]
fn namespace_output_declaration_does_not_inherit_a_preceding_namespaces_exec() {
    let (tc, lr) = compile_with_libs(
        &[
            (
                "a.ws",
                "var x: int = 0\nin tick: exec\non tick { x = x + 1 }",
            ),
            (
                "b.ws",
                "in bump: exec\nout n: int = 5\non bump { emit n = 9 }",
            ),
        ],
        "import * as A from \"a\"\nimport * as B from \"b\"",
    );
    assert_clean(&tc, &lr);
    let backing = lr
        .module
        .nodes
        .values()
        .find(|n| n.note == Some("out_backing"))
        .expect("B's out n must be var-backed (default + emit)");
    assert_eq!(
        backing.properties.get(&*crate::intern::sym::INITIAL_VALUE),
        Some(&crate::ir::Literal::Int(5)),
        "the declared default must bake InitialValue = 5: {:?}",
        backing.properties
    );
    let a_increment = lr
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::VAR_INCREMENT)
        .map(|(id, _)| *id)
        .expect("A's `x = x + 1` must lower to a Var_Increment");
    let sets: Vec<crate::ir::NodeId> = lr
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == VAR_SET)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        sets.len(),
        1,
        "only `emit n = 9` drives a Var_Set; the declaration must not add one"
    );
    let set_exec_source = feed_source(
        &lr.module,
        sets[0],
        crate::ir::port_registry::WirePort::Exec,
    );
    assert_ne!(
        set_exec_source,
        Some(a_increment),
        "B's Var_Set must not be driven by A's unrelated exec chain"
    );
}

/// A namespaced module's own top-level `on` handler may declare a boundary
/// port, exactly as its top-level `out` already does. Pass 1a in `lower/mod.rs`
/// walks only the entry file's decls, so `ns.decls` needs its own hoist; without
/// it the port had no binding and `lower_out_binding` reported WS073 against a
/// module whose `out` IS at the top level of its file.
#[test]
fn a_namespaced_modules_handler_declared_output_hoists_like_its_top_level_one() {
    let (tc, lr) = compile_with_lib(
        "in trig: exec\non trig { out hport: int = 7 }\nout tport: int = 3",
        "import * as L from \"lib\"\nout a: int = L.hport\nout b: int = L.tport",
    );
    assert_clean(&tc, &lr);
    // Both spellings must produce the SAME shape: one boundary port apiece,
    // each read through its own rerouter by the importer.
    let ports: Vec<&str> = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class.contains("MicrochipOutput"))
        .map(|n| n.gate_class)
        .collect();
    assert_eq!(
        ports.len(),
        4,
        "two importer ports plus the module's two: {:?}",
        lr.module.nodes
    );
    let reads = lr
        .module
        .wires
        .iter()
        .filter(|w| w.source.port == crate::ir::port_registry::WirePort::RerOutput)
        .count();
    assert_eq!(
        reads, 2,
        "`L.hport` and `L.tport` must both read through a port rerouter; wires: {:?}",
        lr.module.wires
    );
}

fn assert_no_diagnostics(tc: &crate::typecheck::TypeCheckResult, lr: &LowerResult) {
    assert!(
        tc.diagnostics.is_empty() && lr.diagnostics.is_empty(),
        "expected no diagnostics at all: typecheck {:?}, lower {:?}",
        tc.diagnostics,
        lr.diagnostics
    );
}

/// The storage gates of `class`, in declaration order.
fn storage_gates(m: &crate::ir::Module, class: &str) -> Vec<crate::ir::NodeId> {
    let mut gates: Vec<&crate::ir::Node> =
        m.nodes.values().filter(|n| n.gate_class == class).collect();
    gates.sort_by_key(|n| n.source_range.start.offset);
    gates.iter().map(|n| n.id).collect()
}

/// A whole-container rebuild: exactly one `clear` gate, followed on its exec
/// chain by `ops` in order, every one writing through `storage`'s `ref_port`.
fn assert_rebuild_chain(
    m: &crate::ir::Module,
    storage: crate::ir::NodeId,
    ref_port: WirePort,
    clear: &str,
    ops: &[&str],
) {
    let clears: Vec<crate::ir::NodeId> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == clear)
        .map(|n| n.id)
        .collect();
    assert_eq!(clears.len(), 1, "exactly one {clear}: {:?}", m.nodes);
    let mut prev = clears[0];
    assert_eq!(
        feed_source(m, prev, ref_port),
        Some(storage),
        "{clear} targets the member"
    );
    for op in ops {
        let next = m
            .wires
            .iter()
            .find(|w| w.source.node_id == prev && w.source.port == WirePort::ExecOut)
            .map(|w| w.target.node_id)
            .unwrap_or_else(|| panic!("nothing chained after {prev:?}; wires: {:?}", m.wires));
        assert_eq!(m.nodes[&next].gate_class, *op, "exec order of the rebuild");
        assert_eq!(
            feed_source(m, next, ref_port),
            Some(storage),
            "{op} targets the member"
        );
        prev = next;
    }
    for op in ops {
        assert_eq!(
            count_gate(m, op),
            ops.iter().filter(|o| *o == op).count(),
            "no stray {op} gates"
        );
    }
    assert_eq!(count_gate(m, VAR_SET), 0, "no Var_Set on a container");
}

/// A container literal assigned to a namespace member lowers exactly like the
/// bare assignment: a Clear, then one Push per item (or Set per entry), all on
/// the member's own storage, never the scalar field-write path.
#[test]
fn a_namespaced_container_literal_assign_clears_and_rebuilds_the_member() {
    let rows: &[(&str, &str, &str, &str, WirePort, &str, &[&str])] = &[
        (
            "`u.xs = [1, 2]` on a namespace-member array",
            "var xs: int[]",
            "on go { u.xs = [1, 2] }",
            gc::ARRAY_VAR,
            WirePort::ArrayVarRef,
            gc::ARRAY_CLEAR,
            &[gc::ARRAY_PUSH, gc::ARRAY_PUSH],
        ),
        (
            "`u.rec.xs = [1, 2]`, an array field of a record inside a namespace member",
            "type R = { xs: int[], n: int }\nvar rec: R",
            "on go { u.rec.xs = [1, 2] }",
            gc::ARRAY_VAR,
            WirePort::ArrayVarRef,
            gc::ARRAY_CLEAR,
            &[gc::ARRAY_PUSH, gc::ARRAY_PUSH],
        ),
        (
            "`u.m = { .. }` on a namespace-member map",
            "var m: Map<string, int>",
            "on go { u.m = { \"a\": 1, \"b\": 2 } }",
            gc::MAP_VAR,
            WirePort::MapVarRef,
            gc::MAP_CLEAR,
            &[gc::MAP_SET, gc::MAP_SET],
        ),
    ];
    let mut failures = Vec::new();
    for (label, lib, body, storage, ref_port, clear, ops) in rows {
        // Each row's own panic message is printed; the label is collected.
        let row = std::panic::catch_unwind(|| {
            let (tc, lr) = compile_with_lib(
                lib,
                &format!("import * as u from \"lib\"\nin go: exec\n{body}"),
            );
            assert_no_diagnostics(&tc, &lr);
            assert!(!has_unsupported(&lr.module));
            let gates = storage_gates(&lr.module, storage);
            assert_eq!(gates.len(), 1, "one storage gate");
            assert_rebuild_chain(&lr.module, gates[0], *ref_port, clear, ops);
        });
        if row.is_err() {
            failures.push(label.to_string());
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A spread element appends the source member's contents, in order.
#[test]
fn namespaced_array_literal_assign_with_spread_appends_the_source() {
    let (tc, lr) = compile_with_lib(
        "var xs: int[]\nvar ys: int[]",
        "import * as u from \"lib\"\nin go: exec\non go { u.xs = [...u.ys, 3] }",
    );
    assert_no_diagnostics(&tc, &lr);
    assert!(!has_unsupported(&lr.module));
    let gates = storage_gates(&lr.module, gc::ARRAY_VAR);
    let (xs, ys) = (gates[0], gates[1]);
    assert_rebuild_chain(
        &lr.module,
        xs,
        WirePort::ArrayVarRef,
        gc::ARRAY_CLEAR,
        &[gc::ARRAY_APPEND, gc::ARRAY_PUSH],
    );
    let append = lr
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::ARRAY_APPEND)
        .unwrap()
        .id;
    assert_eq!(
        feed_source(&lr.module, append, WirePort::SourceRef),
        Some(ys),
        "the spread appends u.ys"
    );
}

/// A non-literal whole-container assign to a namespace member has no gate to
/// lower to, so it is the same WS027 error as the bare spelling, not a `Var_Set`
/// wired from a port the container gate does not have.
#[test]
fn namespaced_non_literal_container_assign_is_ws027() {
    for (lib, main, fix) in [
        (
            "var m: Map<string, int>",
            "var m2: Map<string, int>\non go { u.m = m2 }",
            "u.m.copyFrom(src)",
        ),
        (
            "var xs: int[]\nvar ys: int[]",
            "on go { u.xs = u.ys }",
            "u.xs.copyFrom(src)",
        ),
    ] {
        let (_tc, lr) = compile_with_lib(
            lib,
            &format!("import * as u from \"lib\"\nin go: exec\n{main}"),
        );
        assert!(
            lr.diagnostics.iter().any(|d| d.code == "WS027"
                && d.severity == crate::diagnostic::Severity::Error
                && d.message.contains(fix)),
            "`{main}` must be a WS027 error naming the target: {:?}",
            lr.diagnostics
        );
        assert_eq!(
            count_gate(&lr.module, VAR_SET),
            0,
            "`{main}` emits no Var_Set"
        );
    }
}

/// A namespace's `*int[]` var reads as the array, as the same var does in the
/// file that declares it: `let y: int[] = S.cur` was WS016.
#[test]
fn a_namespaced_array_ref_var_reads_as_the_array() {
    let lib = "var rows: int[][]\nvar cur: *int[]\n";
    let src = "import * as S from \"lib\"\nin go: exec\nvar n: int = 0\nout onn: int = n\non go {\n  S.cur = S.rows[0]\n  let y: int[] = S.cur\n  n = y.length() + S.cur.length()\n}";
    let r = compile_multi(src, &[("lib", lib)]);
    assert_no_errors(&r);
    crate::test_support::assert_no_code(&r.diagnostics, "WS016");
    assert_eq!(gate_count(&r, gc::ARRAY_GET_LENGTH), 2);
}
