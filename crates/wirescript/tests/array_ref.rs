use std::sync::Arc;

use wirescript::ir::{gate_class, Module};
use wirescript::lower::{lower, LowerInput};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::{typecheck, CeSlotMap};
use wirescript::{compile, resolve, CompileInput, FsLoader, FoldMode, Severity};

fn compiles(src: &str) -> bool {
    compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto }).is_ok()
}

#[test]
fn out_array_binding() {
    assert!(compiles("\
var framebuf: int[]
out display: int[] = framebuf"));
}

#[test]
fn let_array_binding() {
    assert!(compiles("\
var data: int[]
let alias = data"));
}

#[test]
fn array_passed_to_chip() {
    assert!(compiles("\
var buf: int[]
chip Render(fb: int[]) -> () {
  in run: exec
  on run { fb.push(1) }
}
in start: exec
on start { Render(buf) }"));
}

#[test]
fn array_in_record() {
    assert!(compiles("\
var items: int[]
type State = { data: int[] }
let state: State = { data: items }"));
}

#[test]
fn out_array_compiles_to_brz() {
    let src = "\
var framebuf: int[]
out display: int[] = framebuf
in player: character
on player { framebuf.push(42) }";
    let r = compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto });
    assert!(r.is_ok(), "out array should compile: {:?}", r.err());
}

/// Gate count of `class` in `m`, including nested chip modules.
fn count_gate_class(m: &Module, class: &str) -> usize {
    m.nodes.values().filter(|n| n.gate_class == class).count()
        + m.chips.values().map(|c| count_gate_class(c, class)).sum::<usize>()
}

/// KNOWN PRE-EXISTING COMPILER DEFECT, unrelated to const evaluation: a
/// `var arr: T[] = someModThatReturnsAnArray(...)` initializer silently
/// drops the ENTIRE call — not merely its result.
///
/// `lower_stmt`'s `Stmt::Var` arm resets an array-typed var each time its
/// scope is entered (`crates/wirescript/src/lower/stmt.rs`, the
/// `VarStorage::Array` branch) by rebuilding it in place with a clear+push
/// sequence, because array vars have no scalar `VarRef` port for a generic
/// reset to target. That rebuild only knows how to consume a literal
/// `Expr::Array` initializer — any other initializer expression, including
/// a well-typed call to a mod that returns `int[]`, falls to the `else`
/// arm, which emits a WSP001 warning ("this value is dropped") and returns
/// WITHOUT lowering the initializer at all. Because mods inline per call
/// site, this means the callee's own gates (its `if`s, its `push`es) never
/// appear anywhere in the module either — the callee's body doesn't lower,
/// full stop, not just "computed and then discarded".
///
/// This was found via `crates/wirescript/tests/fixtures/const/assembly.ws`,
/// which used exactly this shape for its runtime baseline and silently
/// exercised zero gates, reporting 1/10 in game with a correct compile-time
/// answer compared against a broken (empty) runtime one. That fixture was
/// rewritten to use the pattern that IS known to lower correctly — an
/// `int[]` parameter (already a reference) the callee pushes into directly,
/// e.g. `mod build(n: int, t: int[]) { ... t.push(10) ... }` called as
/// `var r: int[]\nbuild(2, r)`. This test exists only to TRACK the
/// underlying compiler bug so it is not silently reintroduced or
/// forgotten; fixing it is out of scope here.
///
/// CORRECT BEHAVIOR would lower the initializer expression like any other
/// value and rebuild the array from it (clear + append, or equivalent),
/// the same way the literal-array path already does — so `var r: int[] =
/// build(2)` would behave identically to the out-parameter workaround
/// above: 2 `ArrayVar_Push` gates from `build`'s body, no WSP001 warning.
///
/// THIS TEST ASSERTS THE CURRENT (BROKEN) BEHAVIOR. When the underlying bug
/// is fixed, INVERT it: expect no WSP001 warning and `push_count == 2`.
#[test]
fn array_returning_mod_call_as_var_initializer_is_silently_dropped_known_bug() {
    let src = "\
mod build(n: int) -> int[] {
  var t: int[]
  if n >= 1 { t.push(10) }
  if n >= 2 { t.push(20) }
  return t
}
in go: exec
on go {
  var r: int[] = build(2)
  BroadcastChatMessage(\"\" .. r.length())
}";
    let resolved = resolve(src, "test.ws", &FsLoader);
    assert!(
        resolved.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "resolve errors: {:?}",
        resolved.diagnostics
    );
    let tc = typecheck(&resolved.ast, "test.ws", &CeSlotMap::default());
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test.ws",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::ForceOff,
        ce_slots: &CeSlotMap::default(),
    });
    // No ERROR diagnostic — the program "compiles" cleanly end to end. That
    // silence (plus a value that still type-checks as `int[]`) is exactly
    // why nobody noticed: there is nothing red to see.
    assert!(
        lowered.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "unexpected lower errors: {:?}",
        lowered.diagnostics
    );

    // A WSP001 WARNING does fire, saying the initializer's value is dropped
    // — but only if a caller reads lowering's warnings, which the game does
    // not surface to the level author.
    let warned = lowered.diagnostics.iter().any(|d| {
        d.code == "WSP001" && d.message.contains("array initializer must be an array literal")
    });
    assert!(warned, "expected the known WSP001 drop warning, got {:?}", lowered.diagnostics);

    // The push gates never lower AT ALL: the callee's body is skipped
    // entirely, not merely computed and then thrown away.
    let push_count = count_gate_class(&lowered.module, gate_class::ARRAY_PUSH);
    assert_eq!(
        push_count, 0,
        "KNOWN BUG regressed (fixed?): expected 0 ArrayVar_Push gates for the \
         dropped call, got {push_count}. If this now fails because the bug was \
         fixed, INVERT this assertion to `== 2` and update the WSP001 assertion \
         above to expect no warning."
    );
}
