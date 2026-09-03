//! `import * as ns` VALUE members (`ns.myValue`) — as opposed to `ns.f(...)`
//! calls, which have always worked. These used to type as `any` and lower to an
//! `_Unsupported` placeholder that silently read 0.

use super::*;
use crate::resolve::{MemLoader, resolve};

/// Resolve + typecheck + lower a two-file program (`lib.ws` + main).
fn compile_with_lib(lib_src: &str, main_src: &str) -> (crate::typecheck::TypeCheckResult, LowerResult) {
    compile_with_libs(&[("lib.ws", lib_src)], main_src)
}

/// Resolve + typecheck + lower a program importing several library modules.
fn compile_with_libs(
    libs: &[(&str, &str)],
    main_src: &str,
) -> (crate::typecheck::TypeCheckResult, LowerResult) {
    let mut files = std::collections::HashMap::default();
    for (name, src) in libs {
        files.insert((*name).to_string(), (*src).into());
    }
    let loader = MemLoader { files };
    let resolved = resolve(main_src, "test", &loader);
    assert!(
        resolved.diagnostics.is_empty(),
        "import should resolve: {:?}",
        resolved.diagnostics
    );
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let lr = crate::lower::lower(crate::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        // Namespace-resolution structure tests; force folding off so constant
        // chip/value bodies aren't optimized away out from under the asserts.
        fold_mode: FoldMode::ForceOff,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    });
    (tc, lr)
}

fn assert_clean(tc: &crate::typecheck::TypeCheckResult, lr: &LowerResult) {
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "lower errors: {:?}",
        lr.diagnostics
    );
}

fn has_unsupported(m: &crate::ir::Module) -> bool {
    m.nodes.values().any(|n| n.gate_class.contains("Unsupported"))
        || m.chips.values().any(has_unsupported)
}

/// Every `Literal::Int` baked into any node property, anywhere in the tree.
fn baked_ints(m: &crate::ir::Module) -> Vec<i64> {
    let mut out = Vec::new();
    for n in m.nodes.values() {
        for v in n.properties.values() {
            if let crate::ir::Literal::Int(i) = v {
                out.push(*i);
            }
        }
    }
    for c in m.chips.values() {
        out.extend(baked_ints(c));
    }
    out
}

#[test]
fn namespaced_scalar_let_lowers_to_its_value() {
    let (tc, lr) = compile_with_lib(
        "let answer: int = 42",
        "import * as lib from \"lib\"\nout o = lib.answer",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "a namespaced scalar must lower to a real gate, not a placeholder"
    );
    assert!(
        baked_ints(&lr.module).contains(&42),
        "the namespaced constant's value must reach the output, got {:?}",
        baked_ints(&lr.module)
    );
}

#[test]
fn namespaced_record_let_field_lowers_to_its_value() {
    // `lib.rec.value` — the namespace hop resolves to the record binding, then
    // the normal record-field walk continues from there.
    let (tc, lr) = compile_with_lib(
        "type V = {value: int}\nlet rec: V = {value: 42}",
        "import * as lib from \"lib\"\nout o = lib.rec.value",
    );
    assert_clean(&tc, &lr);
    assert!(!has_unsupported(&lr.module), "must not emit a placeholder");
    assert!(
        baked_ints(&lr.module).contains(&42),
        "the record field's value must reach the output, got {:?}",
        baked_ints(&lr.module)
    );
}

#[test]
fn namespaced_value_types_as_its_declared_type_not_any() {
    // Typing it `any` made every use against a concrete type a spurious
    // mismatch — this program is correct and must check clean.
    let (tc, lr) = compile_with_lib(
        "type V = {value: int}\nlet rec: V = {value: 42}",
        "import * as lib from \"lib\"\ntype Outer = {value: lib.V}\nlet r: Outer = {value: lib.rec}\nout o = r.value.value",
    );
    assert_clean(&tc, &lr);
    assert!(
        baked_ints(&lr.module).contains(&42),
        "the nested record value must survive, got {:?}",
        baked_ints(&lr.module)
    );
}

#[test]
fn namespaced_value_feeding_a_typed_param_checks_clean() {
    // The `any` typing showed up as a WS003 at any typed boundary.
    let (tc, lr) = compile_with_lib(
        "let answer: int = 42",
        "import * as lib from \"lib\"\nmod takesInt(v: int) -> int { return v + 1 }\nout o = takesInt(lib.answer)",
    );
    assert_clean(&tc, &lr);
    assert!(!has_unsupported(&lr.module), "must not emit a placeholder");
}

#[test]
fn two_namespaces_sharing_a_member_name_stay_distinct() {
    // Two imported modules each export `empty`, of unrelated record types. A
    // record field access `a.empty.<field>` on each must resolve against the
    // right namespace. Lowering used to dump every namespace's value members
    // into one shared bare-name scope where the last import won, so the
    // first-imported namespace's `empty` was overwritten and any field access
    // on it lowered to an `_Unsupported` placeholder that silently read a
    // default. Typecheck kept them distinct, so `wirescript-check` reported the
    // file clean.
    let (tc, lr) = compile_with_libs(
        &[
            ("sym.ws", "type Sym = { layer1: string }\nlet empty: Sym = { layer1: \"L\" }"),
            ("tun.ws", "type Tun = { topart: string }\nlet empty: Tun = { topart: \"T\" }"),
        ],
        "import * as Sym from \"sym\"\n\
         import * as Tun from \"tun\"\n\
         var sink: string[]\n\
         let g = ReadBrickGrid()\n\
         on g {\n\
           sink.push(\"${Sym.empty.layer1}\")\n\
           sink.push(\"${Tun.empty.topart}\")\n\
         }",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "each namespace's `empty` must resolve to its own record, not collapse to one bare name"
    );
}

/// An imported module's root-level `in` port, triggered as `on ns.port`. The
/// namespace lowering never declared `in`/`out` at all, so the port did not
/// exist and the trigger matched nothing — which silently dropped the ENTIRE
/// handler body, with both typecheck and lowering reporting the file clean.
#[test]
fn namespaced_input_port_can_trigger_a_handler() {
    let (tc, lr) = compile_with_lib(
        "in trigger: exec",
        "import * as L from \"lib\"\non L.trigger { PrintToConsole(\"fired\") }",
    );
    assert_clean(&tc, &lr);
    let has_input = lr
        .module
        .nodes
        .values()
        .any(|n| n.gate_class == "BrickComponentType_Internal_MicrochipInput");
    assert!(has_input, "the imported `in` port must be declared");
    let printed = lr
        .module
        .nodes
        .values()
        .any(|n| n.gate_class.contains("PrintToConsole"));
    assert!(
        printed,
        "the handler body must survive: `on ns.trigger` used to drop it entirely"
    );
    assert!(
        !lr.module.wires.is_empty(),
        "the imported input must drive the handler's exec chain"
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
        .find(|(_, n)| n.gate_class == "BrickComponentType_Internal_MicrochipOutput")
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
    assert!(
        !lr.module.chips.is_empty(),
        "a namespaced chip call must still instantiate its chip"
    );
}

const PSEUDO_VAR: &str = "BrickComponentType_WireGraphPseudo_Var";
const VAR_SET: &str = "BrickComponentType_WireGraph_Exec_Var_Set";
const OUTPUT: &str = "BrickComponentType_Internal_MicrochipOutput";

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
        count_gate(&lr.module, PSEUDO_VAR),
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
    let r0 = feed_source(&lr.module, outs[0], crate::ir::port_registry::WirePort::RerInput);
    let r1 = feed_source(&lr.module, outs[1], crate::ir::port_registry::WirePort::RerInput);
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
    let w0 = feed_source(&lr.module, sets[0], crate::ir::port_registry::WirePort::VarRef);
    let w1 = feed_source(&lr.module, sets[1], crate::ir::port_registry::WirePort::VarRef);
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
        count_gate(&lr.module, PSEUDO_VAR),
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
    let w0 = feed_source(&lr.module, sets[0], crate::ir::port_registry::WirePort::VarRef);
    let w1 = feed_source(&lr.module, sets[1], crate::ir::port_registry::WirePort::VarRef);
    assert!(
        w0.is_some() && w1.is_some() && w0 != w1,
        "`g = 1` and `A.g = 2` must write distinct gates"
    );
}

/// A namespaced import's top-level `on` handler runs as part of the importing
/// program. Handlers used to fall into the namespace arm's `_ => {}` and be
/// dropped, so importing a module as a namespace ran none of its handlers.
#[test]
fn namespaced_import_on_handler_runs() {
    let (tc, lr) = compile_with_lib(
        "var ticks: int = 0\nin go: exec\non go { ticks = ticks + 1 }\nout t = ticks",
        "import * as L from \"lib\"",
    );
    assert_clean(&tc, &lr);
    assert!(
        lr.module
            .nodes
            .values()
            .any(|n| n.gate_class.contains("Var_Increment")),
        "the namespaced module's `on go` handler body must lower"
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
/// behaviour just like a top-level handler. It used to fall into the namespace
/// arm's `_ => {}` and vanish, taking its writes with it.
#[test]
fn namespaced_import_anon_chip_runs() {
    let (tc, lr) = compile_with_lib(
        "var v: int = 0\nin t: exec\nchip on t { v = 1 }\nout cur = v",
        "import * as N from \"lib\"",
    );
    assert_clean(&tc, &lr);
    assert!(
        !lr.module.chips.is_empty(),
        "the namespaced anon chip must instantiate a microchip"
    );
    let has_set = lr
        .module
        .chips
        .values()
        .any(|c| c.nodes.values().any(|n| n.gate_class.contains("Var_Set")));
    assert!(has_set, "the namespaced anon chip's `v = 1` write must lower");
}

fn count_class_ns(m: &crate::ir::Module, class: &str) -> usize {
    let mut n = m.nodes.values().filter(|x| x.gate_class == class).count();
    for c in m.chips.values() {
        n += count_class_ns(c, class);
    }
    n
}

/// Two modules with a same-named private `helper`, each called by a public mod:
/// `A.f` must run A's `helper` and `B.g` must run B's. The bodies used to
/// resolve `helper` through the shared bare scope (last namespace won), so both
/// ran ONE module's helper. Now each mod body is lowered with its own module's
/// members pushed into the frame.
#[test]
fn namespaced_sibling_mod_resolves_its_own_module() {
    let (tc, lr) = compile_with_libs(
        &[
            ("m.ws", "mod helper(x: int) -> int { return x + 1 }\nmod f(x: int) -> int { return helper(x) }"),
            ("n.ws", "mod helper(x: int) -> int { return x * 100 }\nmod g(x: int) -> int { return helper(x) }"),
        ],
        "import * as A from \"m\"\n\
         import * as B from \"n\"\n\
         in v: int\n\
         out ra = A.f(v)\n\
         out rb = B.g(v)",
    );
    assert_clean(&tc, &lr);
    assert_eq!(
        count_class_ns(&lr.module, "BrickComponentType_WireGraph_Expr_MathAdd"),
        1,
        "A.f must run m's `helper` (x + 1)"
    );
    assert_eq!(
        count_class_ns(&lr.module, "BrickComponentType_WireGraph_Expr_MathMultiply"),
        1,
        "B.g must run n's `helper` (x * 100), not m's"
    );
}

/// A namespaced mod reading its module's `let` constant returns THAT module's
/// value: `A.getG()` bakes 111, `B.getG()` bakes 222.
#[test]
fn namespaced_mod_reads_its_own_module_const() {
    let (tc, lr) = compile_with_libs(
        &[
            ("mc.ws", "let g: int = 111\nmod getG() -> int { return g }"),
            ("nc.ws", "let g: int = 222\nmod getG() -> int { return g }"),
        ],
        "import * as A from \"mc\"\nimport * as B from \"nc\"\nout ra = A.getG()\nout rb = B.getG()",
    );
    assert_clean(&tc, &lr);
    let ints = baked_ints(&lr.module);
    assert!(ints.contains(&111), "A.getG() must bake m's g (111): {ints:?}");
    assert!(ints.contains(&222), "B.getG() must bake n's g (222): {ints:?}");
}

/// A namespaced mod that mutates its module's `var` writes ITS module's gate.
/// Both modules declare `var g` + `mod bump` (the name collision), but only
/// `A.g` is read out; `A.bump()`'s increment must target the very gate `A.g`
/// reads. It used to write whichever namespace lowered last (B's).
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
        count_class_ns(&lr.module, "BrickComponentType_WireGraphPseudo_Var"),
        2,
        "A.g and B.g are distinct storage gates"
    );
    // The gate `out a = A.g` reads.
    let out_id = find_gate(&lr, "BrickComponentType_Internal_MicrochipOutput");
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
        .find(|(_, n)| n.gate_class.contains("Var_Increment"))
        .map(|(id, _)| *id)
        .expect("A.bump() must lower to a Var_Increment");
    let inc_target = lr
        .module
        .wires
        .iter()
        .find(|w| {
            w.target.node_id == inc
                && w.target.port == crate::ir::port_registry::WirePort::VarRef
        })
        .map(|w| w.source.node_id)
        .expect("the increment must target a storage gate");
    assert_eq!(
        inc_target, a_gate,
        "A.bump() must write the gate A.g reads, not the other module's"
    );
}

/// An operator inside a namespaced module's `on` handler must be typechecked
/// (its `op_resolutions` recorded) so it lowers to a real gate. The handler
/// itself lowers, but typecheck used to never descend into its body, so
/// arithmetic like `n << 10` lowered to `_Unsupported`.
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

/// N1: a free name in a namespaced module's `on` handler must NOT resolve to
/// the importer's same-named top-level state. `lib`'s `on go { q = q + 1 }`
/// (with no `go`/`q` of its own) once SILENTLY wired to main's `go`/`q`; the
/// namespace body is now checked under a sealed scope floor, so those names are
/// undefined and reported (WS001 for the trigger, WS002 for the body). The
/// diagnostic aborts compilation, so the leak can never ship — the fix is that
/// it is no longer silent, not that lowering (which never runs to completion
/// here) rewires it.
#[test]
fn namespaced_handler_free_name_does_not_leak_to_importer_state() {
    let (tc, _lr) = compile_with_lib(
        "on go { q = q + 1 }",
        "import * as L from \"lib\"\nin go: exec\nvar q: int = 0\nout result: int = q",
    );
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS002"),
        "the leaked body identifier must be reported undefined, got {:?}",
        tc.diagnostics
    );
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS001"),
        "the leaked trigger must be reported unknown, got {:?}",
        tc.diagnostics
    );
}

/// N2: the SAME file's state imported both plain and via `import * as` must
/// resolve to ONE gate (docs promise one shared gate), and its handler must
/// install once. Previously `g` and `S.g` diverged into two gates each with
/// its own increment handler — a double-count on every `bump`.
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
        .filter(|n| n.gate_class.contains("Pseudo_Var"))
        .count();
    assert_eq!(vars, 1, "g and S.g must share ONE storage gate, got {vars}");
    let incs = lr
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class.contains("Var_Increment"))
        .count();
    assert_eq!(incs, 1, "the imported handler must install once, got {incs}");
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
        .filter(|n| n.gate_class.contains("Pseudo_Var"))
        .count();
    assert_eq!(vars, 1, "g and its alias h must share ONE storage gate, got {vars}");
}

/// N11: a namespace a module privately imports (`a` does `import * as B`) must
/// not leak to a plain importer of that module. `main` imports `a` but not `b`,
/// so its own `B.bar()` is an unknown identifier — not a silent resolution of
/// the alias that traveled in for `a`'s `useB`.
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

/// Reading a namespaced `out` member (`L.count`) resolves to the output's
/// value instead of WS002 "not found in namespace" plus an `_Unsupported`
/// placeholder. `In`/`Out` were absent from the typecheck namespace map and
/// the output's binding never reached the lowering map.
#[test]
fn reading_a_namespaced_output_member_resolves() {
    let (tc, lr) = compile_with_lib(
        "var counter: int = 0\nin tick: exec\non tick { counter = counter + 1 }\nout count: int = counter",
        "import * as L from \"lib\"\nout shown: int = L.count",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "L.count must resolve, not lower to a placeholder"
    );
}

/// Reading a namespaced `in` member (`L.level`) as a value resolves too.
#[test]
fn reading_a_namespaced_input_member_resolves() {
    let (tc, lr) = compile_with_lib(
        "in level: int",
        "import * as L from \"lib\"\nout shown: int = L.level",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "L.level must resolve, not lower to a placeholder"
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
        .find(|n| n.gate_class == "BrickComponentType_Internal_MicrochipOutput")
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
    assert_eq!(drivers, 1, "a no-emit defaulted output must have one direct driver");
    assert!(
        !lr.module.nodes.values().any(|n| n.note == Some("out_backing")),
        "a no-emit defaulted output must not get a backing var"
    );
}

/// 4b: a namespaced `out` reached by a default plus an `emit` must be
/// var-backed, not fanned in. The top-level pass-1b backing-var prescan does
/// not descend into `ns.decls`, so `out r = 0` + `emit r = 5` drove the output
/// rerouter from two sources (the default and the emit) — a load-time fan-in.
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
        .find(|n| n.gate_class == "BrickComponentType_Internal_MicrochipOutput")
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
        lr.module.nodes.values().any(|n| n.note == Some("out_backing")),
        "a backing var must have been created for the namespaced output"
    );
}

/// The N11 visibility rule must still let the TRAVELING namespace serve its
/// origin file's own decls: `main` calling `a`'s `useB()` (whose body uses
/// `B.bar()`) resolves and inlines cleanly, because that reference lives in
/// a.ws, where `B` is in scope.
#[test]
fn traveling_namespace_still_serves_its_origin_files_decls() {
    let (tc, lr) = compile_with_libs(
        &[
            (
                "a.ws",
                "import * as B from \"b\"\nmod useB() -> int { return B.bar() }",
            ),
            ("b.ws", "mod bar() -> int { return 7 }"),
        ],
        "import \"a\"\nout r: int = useB()",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "useB's B.bar() must resolve, not lower to a placeholder"
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
    assert!(
        lr.module
            .nodes
            .values()
            .any(|n| n.gate_class.contains("Var_Increment")),
        "a namespaced handler referencing its own members must still lower its write"
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
            ("leaf.ws", "let value = 10\nmod bump(n: int) -> (r: int) { return n + value }"),
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
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    assert!(
        !lr.module.nodes.values().any(|n| n.gate_class == "_Unsupported"),
        "a transitive namespace read must lower to real gates: {:?}",
        lr.diagnostics
    );
    // Every one of the three reads drives its own store, so none was dropped.
    assert_eq!(
        count_class_ns(&lr.module, "BrickComponentType_WireGraph_Exec_Var_Set"),
        3,
        "each of `a`/`b`/`c` must be written"
    );
    // The two `n + L.value` bodies and `bump`'s own add are real gates, so the
    // transitive read reached arithmetic rather than folding to a placeholder.
    let adds = count_class_ns(&lr.module, "BrickComponentType_WireGraph_Expr_MathAdd");
    assert!(adds >= 2, "expected the inlined `n + …` adds, got {adds}");
}

/// The alias COLLIDES: the importer and the module it imports both spell their
/// `import * as` the same. An alias is FILE-LOCAL, so this is not a conflict --
/// each file's `Other` means its own import. Both tables are keyed by name
/// alone, so the traveling one used to be dropped and every reference through
/// it read as an unknown identifier.
#[test]
fn colliding_namespace_alias_resolves_per_file() {
    let (tc, lr) = compile_with_libs(
        &[
            ("leaf.ws", "let value = 10
mod bump(n: int) -> (r: int) { return n + value }"),
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
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    assert!(
        !lr.module.nodes.values().any(|n| n.gate_class == "_Unsupported"),
        "both `Other`s must resolve: {:?}",
        lr.diagnostics
    );
    // `leaf`'s `n + value` really inlined, so the call went through the INNER
    // alias rather than resolving back to the importer's same-named one.
    assert!(
        count_class_ns(&lr.module, "BrickComponentType_WireGraph_Expr_MathAdd") >= 1,
        "the inlined body's arithmetic must be present"
    );
}
