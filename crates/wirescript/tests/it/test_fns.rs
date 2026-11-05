//! `@test` on a `fn`, `mod` or `chip`: a test emits nothing unless called,
//! is never reported unused, and a compile with a test entry calls that test
//! once at load and then prints the completion sentinel.

use wirescript::compile::{CompileError, CompileInput, compile_to_world, compile_with_loader};
use wirescript::emit::EmitOptions;
use wirescript::ir::gate_class as gc;
use wirescript::ir::port_registry::WirePort;
use wirescript::ir::{Literal, Module, NodeId, Program};
use wirescript::resolve::MemLoader;
use wirescript::{Diagnostic, TEST_DONE, TestKind, list_tests};

const FILE: &str = "tests.ws";
const LOAD_EVENT: &str = "BrickComponentType_Event_Loaded";

const BASE: &str = "
var total: int = 0
in go: exec
on go {
  total += 1
}
";

fn program(input: CompileInput<'_>) -> (Program, Vec<Diagnostic>) {
    let built = compile_to_world(input, EmitOptions::default())
        .unwrap_or_else(|e| panic!("should compile: {e:?}"));
    (built.program, built.diagnostics)
}

/// The program's `.wsir` text, which every output is built from.
fn ir_text(p: &Program) -> String {
    wirescript::wsir::program_to_string(p).expect("writes")
}

fn count(m: &Module, class: &str) -> usize {
    let mut n = 0;
    m.walk(&mut |_, sub| n += sub.nodes.values().filter(|x| x.gate_class == class).count());
    n
}

fn nodes(m: &Module) -> usize {
    let mut n = 0;
    m.walk(&mut |_, sub| n += sub.nodes.len());
    n
}

fn codes(input: CompileInput<'_>) -> Vec<String> {
    match compile_to_world(input, EmitOptions::default()) {
        Ok(r) => r.diagnostics.iter().map(|d| d.code.clone()).collect(),
        Err(CompileError::HasErrors(ds)) => ds.iter().map(|d| d.code.clone()).collect(),
        Err(e) => panic!("{e:?}"),
    }
}

/// A `@test fn` and an ordinary `fn` that nothing calls both emit nothing: the
/// IR, and so the `.brz`, `.brdb` and `.wsir` built from it, is the program's
/// without them.
#[test]
fn an_uncalled_fn_or_test_emits_nothing() {
    let (base, _) = program(CompileInput::new(BASE, FILE));
    let with_test = format!(
        "{BASE}\n@test fn counts() {{\n  total += 5\n  BroadcastChatMessage(\"total ${{total}}\")\n}}\n"
    );
    let with_fn = format!("{BASE}\nfn helper(x: int) -> int {{\n  return x * 3 + total\n}}\n");
    for (label, src) in [("@test fn", &with_test), ("fn", &with_fn)] {
        let (p, diags) = program(CompileInput::new(src, FILE));
        assert!(diags.is_empty(), "{label}: {diags:?}");
        assert_eq!(nodes(&p.module), nodes(&base.module), "{label}");
        assert_eq!(
            count(&p.module, gc::MICROCHIP_FUNCTION_DEFINITION),
            0,
            "{label}"
        );
        assert_eq!(ir_text(&p), ir_text(&base), "{label}");
    }
}

/// Called from ordinary code, a test is an ordinary `fn`.
#[test]
fn a_called_test_is_an_ordinary_fn() {
    let body = "fn bump() {\n  total += 2\n}\non go {\n  bump()\n}\n";
    // Padded to the same columns, so the source locations agree too.
    let (plain, _) = program(CompileInput::new(&format!("{BASE}      {body}"), FILE));
    let (test, diags) = program(CompileInput::new(&format!("{BASE}@test {body}"), FILE));
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(count(&test.module, gc::MICROCHIP_FUNCTION_DEFINITION), 1);
    assert_eq!(count(&test.module, gc::FUNCTION_CALL), 1);
    assert_eq!(ir_text(&test), ir_text(&plain));
}

/// A test with parameters or results is WS124.
#[test]
fn a_test_takes_nothing_and_returns_nothing() {
    for src in [
        "@test fn t(n: int) { }\n",
        "@test fn t() -> int { return 1 }\n",
        "@test fn t() -> (a: int) { a = 1 }\n",
    ] {
        let got = codes(CompileInput::new(src, FILE));
        assert!(got.iter().any(|c| c == "WS124"), "{src:?}: {got:?}");
    }
    assert!(codes(CompileInput::new("@test fn t() { }\n", FILE)).is_empty());
}

/// A test entry compiles the program as if it held `on Loaded() { name() }`:
/// that test's plane and one call fired by a Load Event. The file's other
/// tests stay unemitted.
#[test]
fn a_test_entry_calls_the_test_at_load() {
    let src = format!(
        "{BASE}\n@test fn first() {{\n  total += 10\n}}\n\n@test fn second() {{\n  total += 20\n  BroadcastChatMessage(\"${{total}}\")\n}}\n"
    );
    let names: Vec<String> = list_tests(&src, FILE).into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["first", "second"]);

    let (run, diags) = program(CompileInput::new(&src, FILE).test_entry("second"));
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(count(&run.module, gc::MICROCHIP_FUNCTION_DEFINITION), 1);
    assert_eq!(count(&run.module, gc::FUNCTION_CALL), 1);
    assert_eq!(count(&run.module, LOAD_EVENT), 1);
    // The test's own message and the sentinel.
    assert_eq!(count(&run.module, gc::GAMEMODE_BROADCAST_CHAT), 2);

    let (by_hand, _) = program(CompileInput::new(&with_entry(&src, "second"), FILE));
    assert_eq!(nodes(&run.module), nodes(&by_hand.module));
    assert_eq!(
        count(&run.module, LOAD_EVENT),
        count(&by_hand.module, LOAD_EVENT)
    );
}

/// `src` with the handler a test entry adds, written out by hand.
fn with_entry(src: &str, name: &str) -> String {
    format!("{src}\non Loaded() {{\n  {name}()\n  BroadcastChatMessage(\"{TEST_DONE}\")\n}}\n")
}

/// The Broadcast Chat Message gate that prints `text`.
fn broadcast_of(m: &Module, text: &str) -> NodeId {
    let mut found = Vec::new();
    m.walk(&mut |_, sub| {
        for n in sub.nodes.values() {
            let prints = n
                .properties
                .values()
                .any(|v| matches!(v, Literal::String(s) if s == text));
            if n.gate_class == gc::GAMEMODE_BROADCAST_CHAT && prints {
                found.push(n.id);
            }
        }
    });
    assert_eq!(found.len(), 1, "one broadcast of {text:?}: {found:?}");
    found[0]
}

/// Every wire of `m` and its chips. Node ids are program-wide, and a wire
/// may name a node of another chip (a chip's exec output feeds its caller).
fn all_wires(m: &Module) -> Vec<wirescript::ir::Wire> {
    let mut wires = Vec::new();
    m.walk(&mut |_, sub| wires.extend(sub.wires.iter().copied()));
    wires
}

/// Whether `from` feeds `to` through some run of wires.
fn upstream_of(m: &Module, from: NodeId, to: NodeId) -> bool {
    let wires = all_wires(m);
    let mut seen = vec![to];
    let mut todo = vec![to];
    while let Some(n) = todo.pop() {
        for w in wires.iter().filter(|w| w.target.node_id == n) {
            if w.source.node_id == from {
                return true;
            }
            if !seen.contains(&w.source.node_id) {
                seen.push(w.source.node_id);
                todo.push(w.source.node_id);
            }
        }
    }
    false
}

/// The output that drives `node`'s Exec input.
fn exec_source(m: &Module, node: NodeId) -> (NodeId, WirePort) {
    let w = all_wires(m)
        .into_iter()
        .find(|w| w.target.node_id == node && w.target.port.as_str() == "Exec")
        .expect("the gate's Exec is wired");
    (w.source.node_id, w.source.port)
}

fn class_of(m: &Module, id: NodeId) -> &'static str {
    let mut class = None;
    m.walk(&mut |_, sub| {
        if let Some(n) = sub.nodes.get(&id) {
            class = Some(n.gate_class);
        }
    });
    class.expect("node exists")
}

const MOD_TEST: &str = "@test mod counts() {\n  total += 5\n  await SleepTicks(_, delay = 3)\n  BroadcastChatMessage(\"body ${total}\")\n}\n";
const CHIP_TEST: &str =
    "@test chip counts() {\n  total += 7\n  BroadcastChatMessage(\"body ${total}\")\n}\n";
const FN_TEST: &str = "@test fn counts() {\n  BroadcastChatMessage(\"body\")\n}\n";

/// An uncalled `@test mod` or `@test chip`, like an uncalled plain `mod` or
/// `chip`, emits nothing.
#[test]
fn an_uncalled_mod_or_chip_test_emits_nothing() {
    let (base, _) = program(CompileInput::new(BASE, FILE));
    let plain_mod = MOD_TEST.replace("@test ", "");
    let plain_chip = CHIP_TEST.replace("@test ", "");
    for (label, decl) in [
        ("@test mod", MOD_TEST),
        ("@test chip", CHIP_TEST),
        ("mod", plain_mod.as_str()),
        ("chip", plain_chip.as_str()),
    ] {
        let src = format!("{BASE}\n{decl}");
        let (p, diags) = program(CompileInput::new(&src, FILE));
        assert!(diags.is_empty(), "{label}: {diags:?}");
        assert_eq!(nodes(&p.module), nodes(&base.module), "{label}");
        assert_eq!(count(&p.module, gc::GAMEMODE_BROADCAST_CHAT), 0, "{label}");
        assert_eq!(ir_text(&p), ir_text(&base), "{label}");
    }
}

/// Called from ordinary code, a `@test mod` or `@test chip` is the plain
/// declaration.
#[test]
fn a_called_mod_or_chip_test_is_the_plain_decl() {
    for kw in ["mod", "chip"] {
        let body = format!("{kw} bump() {{\n  total += 2\n}}\non go {{\n  bump()\n}}\n");
        let (plain, _) = program(CompileInput::new(&format!("{BASE}      {body}"), FILE));
        let (test, diags) = program(CompileInput::new(&format!("{BASE}@test {body}"), FILE));
        assert!(diags.is_empty(), "{kw}: {diags:?}");
        assert_eq!(
            without_template_ids(&ir_text(&test)),
            without_template_ids(&ir_text(&plain)),
            "{kw}"
        );
    }
}

/// `text` with each chip template's process-wide counter (`@bump_3`) dropped.
fn without_template_ids(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("@bump_") {
        out.push_str(&rest[..at + "@bump_".len()]);
        rest = rest[at + "@bump_".len()..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

/// A test entry prints the sentinel only after the test's body: after the
/// Function Call completes for a `fn`, and at the end of the body's chain for
/// a `mod` (past its `await`) or a `chip`.
#[test]
fn a_test_entry_prints_the_sentinel_after_the_test() {
    for (kind, decl) in [
        (TestKind::Fn, FN_TEST),
        (TestKind::Mod, MOD_TEST),
        (TestKind::Chip, CHIP_TEST),
    ] {
        let src = format!("{BASE}\n{decl}");
        let listed = list_tests(&src, FILE);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].name.as_str(), listed[0].kind), ("counts", kind));

        let (run, diags) = program(CompileInput::new(&src, FILE).test_entry("counts"));
        assert!(diags.is_empty(), "{kind:?}: {diags:?}");
        let (by_hand, _) = program(CompileInput::new(&with_entry(&src, "counts"), FILE));
        assert_eq!(nodes(&run.module), nodes(&by_hand.module), "{kind:?}");

        let m = &run.module;
        let done = broadcast_of(m, TEST_DONE);
        if kind == TestKind::Fn {
            let (from, port) = exec_source(m, done);
            assert_eq!(class_of(m, from), gc::FUNCTION_CALL);
            assert_eq!(port.as_str(), "Completed");
        } else {
            let mut body = Vec::new();
            m.walk(&mut |_, sub| {
                body.extend(
                    sub.nodes
                        .values()
                        .filter(|n| n.gate_class == gc::GAMEMODE_BROADCAST_CHAT && n.id != done)
                        .map(|n| n.id),
                );
            });
            assert_eq!(body.len(), 1, "{kind:?}");
            assert!(
                upstream_of(m, body[0], done),
                "{kind:?}: sentinel not after the body"
            );
        }
    }
}

/// A `@test mod` is inlined into the load handler: it reads module state and
/// awaits, with no Function Call.
#[test]
fn a_mod_test_reads_module_state_and_awaits() {
    let src = format!("{BASE}\n{MOD_TEST}");
    let (run, diags) = program(CompileInput::new(&src, FILE).test_entry("counts"));
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(count(&run.module, gc::FUNCTION_CALL), 0);
    assert!(ir_text(&run).contains("SleepTicks"));
}

/// WS124 covers every kind of test, and a generic test too.
#[test]
fn a_mod_or_chip_test_takes_nothing_and_returns_nothing() {
    for src in [
        "@test mod t(n: int) { }\n",
        "@test mod t() -> int { return 1 }\n",
        "@test mod t<T>() { }\n",
        "@test chip t(n: int) { }\n",
        "@test chip t() -> (a: int) { out a = 1 }\n",
    ] {
        let got = codes(CompileInput::new(src, FILE));
        assert!(got.iter().any(|c| c == "WS124"), "{src:?}: {got:?}");
    }
    for src in ["@test mod t() { }\n", "@test chip t() { }\n"] {
        assert!(codes(CompileInput::new(src, FILE)).is_empty(), "{src:?}");
    }
}

/// A whole-module import leaves a library's `@test mod` and `@test chip`
/// behind, so the importer may declare tests of the same names.
#[test]
fn a_library_mod_or_chip_test_stays_behind() {
    let lib = "var hits: int = 0\nmod hit() { hits += 1 }\n@test mod smoke() { hit() }\n@test chip probe() { hit() }\n";
    let loader = mem(&[("lib.ws", lib)]);
    let main = "import \"lib\"\n@test mod smoke() { hit() }\n@test chip probe() { hit() }\n";
    for entry in [None, Some("smoke"), Some("probe")] {
        let mut input = CompileInput::new(main, "main.ws");
        if let Some(e) = entry {
            input = input.test_entry(e);
        }
        match compile_with_loader(input, EmitOptions::default(), &loader) {
            Ok(r) => assert!(r.diagnostics.is_empty(), "{entry:?}: {:?}", r.diagnostics),
            Err(e) => panic!("{entry:?}: {e:?}"),
        }
    }
}

/// A test entry that names no `@test fn` of the file is WS125.
#[test]
fn an_unknown_test_entry_is_an_error() {
    let src = "fn helper() { }\n@test fn smoke() { helper() }\n";
    for name in ["missing", "helper"] {
        let got = codes(CompileInput::new(src, FILE).test_entry(name));
        assert_eq!(got, ["WS125"], "{name}");
    }
}

fn mem(files: &[(&str, &str)]) -> MemLoader {
    MemLoader {
        files: files
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    }
}

/// Neither a test nothing calls nor an import only tests use is reported,
/// and a whole-module import leaves the module's own tests behind, so they
/// cannot collide with the importer's.
#[test]
fn tests_are_never_reported_unused() {
    let lib = "fn helper() -> int { return 4 }\n@test fn smoke() { let n = helper() }\n";
    let loader = mem(&[("lib.ws", lib)]);
    for main in [
        "import \"lib\"\nvar n: int = 0\n@test fn smoke() { n = helper() }\n",
        "import { helper } from \"lib\"\nvar n: int = 0\n@test fn smoke() { n = helper() }\n",
        "import * as l from \"lib\"\nvar n: int = 0\n@test fn smoke() { n = l.helper() }\n",
    ] {
        let diags = match compile_with_loader(
            CompileInput::new(main, "main.ws"),
            EmitOptions::default(),
            &loader,
        ) {
            Ok(r) => r.diagnostics,
            Err(e) => panic!("{main:?}: {e:?}"),
        };
        assert!(diags.is_empty(), "{main:?}: {diags:?}");
        let ran = compile_with_loader(
            CompileInput::new(main, "main.ws").test_entry("smoke"),
            EmitOptions::default(),
            &loader,
        );
        assert!(ran.is_ok(), "{main:?}: {:?}", ran.err());
    }
}

/// The import above only stays quiet because the module's test is left
/// behind: the same name as an ordinary `fn` collides.
#[test]
fn a_module_fn_named_like_a_test_still_collides() {
    let loader = mem(&[("lib.ws", "fn smoke() { }\n")]);
    let main = "import \"lib\"\n@test fn smoke() { }\n";
    let got = match compile_with_loader(
        CompileInput::new(main, "main.ws"),
        EmitOptions::default(),
        &loader,
    ) {
        Ok(r) => r.diagnostics,
        Err(CompileError::HasErrors(ds)) => ds,
        Err(e) => panic!("{e:?}"),
    };
    assert!(!got.is_empty(), "a duplicate `smoke` should be reported");
}
