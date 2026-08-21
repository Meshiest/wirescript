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
    // Guard the pre-existing call path against the value-member change.
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
