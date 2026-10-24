use super::*;
use crate::resolve::{FsLoader, resolve};
use crate::typecheck::typecheck;

fn estimates_for(source: &str) -> HashMap<String, ResourceEstimate> {
    let resolved = resolve(source, "test", &FsLoader);
    let tc = typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    collect_estimates(&resolved.ast, &tc, "test")
}

#[test]
fn basic_chip_has_gates_and_wires() {
    let est = estimates_for("chip Add(a: int, b: int) -> (r: int) { out r = a + b }");
    let add = est.get("Add").expect("should have Add estimate");
    // Pins `a`, `b`, `r` and the Add gate; `a`->Add, `b`->Add, Add->`r`.
    assert_eq!((add.gates, add.wires), (4, 3), "gates and wires");
}

#[test]
fn chip_calling_mod_includes_mod_gates() {
    let src = "\
mod double(v: *int) { v = v + v }
chip Wrap(a: *int) -> () {
  in run: exec
  on run { double(a); double(a) }
}";
    let est = estimates_for(src);
    let double = est.get("double").expect("should have double");
    let wrap = est.get("Wrap").expect("should have Wrap");
    // Wrap calls double 2x, so its gates should be > double's base
    assert!(
        wrap.gates > double.gates,
        "Wrap ({}) should include double ({}) gates",
        wrap.gates,
        double.gates
    );
}

#[test]
fn builtin_calls_do_not_count_as_microchips() {
    // A `mod` whose body only calls a builtin gate (here
    // `BroadcastChatMessage`) instantiates no microchip — the builtin is a
    // single gate. A regression guard against `expand_estimate` fabricating
    // a phantom microchip for every callee absent from `base_estimates`.
    let est = estimates_for("mod say(m: string) { BroadcastChatMessage(m) }");
    let say = est.get("say").expect("should have say estimate");
    assert!(
        say.gates > 0,
        "builtin gate still counts toward gates: {}",
        say.gates
    );
    assert_eq!(
        say.total_microchips, 0,
        "no phantom microchip for a builtin call"
    );
}

#[test]
fn non_inline_chip_call_still_counts_as_a_microchip() {
    // The counterpart to the builtin guard: calling a real non-inline
    // `chip` DOES instantiate a microchip, so the caller's count includes it.
    let src = "\
chip Inner(a: int) -> (r: int) { out r = a + 1 }
chip Outer(a: *int) -> () {
  in run: exec
  on run { let x = Inner(a) }
}";
    let est = estimates_for(src);
    let outer = est.get("Outer").expect("should have Outer");
    assert!(
        outer.total_microchips >= 1,
        "a non-inline chip call is a microchip: {}",
        outer.total_microchips
    );
}

#[test]
fn a_call_in_any_body_counts_as_a_microchip() {
    let bodies = [
        "for i in 0..3 { let x = Inner(a) }",
        "if let E.B(v) = s { let x = Inner(v) }",
        "let n = match s { E.A => Inner(a), E.B(v) => v }",
        "let t = \"${Inner(a)}\"",
    ];
    for body in bodies {
        let src = format!(
            "enum E {{ A, B(int) }}
chip Inner(a: int) -> (r: int) {{ out r = a + 1 }}
                 chip Outer(a: *int) -> () {{
  in run: exec
  var s: E
  on run {{ {body} }}
}}"
        );
        let est = estimates_for(&src);
        let outer = est.get("Outer").expect("should have Outer");
        assert!(
            outer.total_microchips >= 1,
            "{body}: {}",
            outer.total_microchips
        );
    }
}

/// A `import * as ns` member is stored in `symbols` — and looked up by
/// hover — under its qualified `ns.member` name, while estimates are
/// collected under the bare one. Without the qualified alias, hovering
/// `card.draw(...)` rendered the signature with no gate count at all.
#[test]
fn namespaced_member_is_reachable_by_its_qualified_name() {
    let loader = crate::resolve::MemLoader {
        files: [(
            "lib.ws".to_string(),
            "mod draw(v: *int) { v = v + v + 1 }\n".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let src = "import * as card from \"lib\"\nvar x: int = 0\non RoundStart { card.draw(x) }\n";
    let resolved = resolve(src, "main", &loader);
    let tc = typecheck(
        &resolved.ast,
        "main",
        &crate::typecheck::CeSlotMap::default(),
    );
    let est = collect_estimates(&resolved.ast, &tc, "main");

    let bare = est.get("draw").expect("bare member key").gates;
    let qualified = est
        .get("card.draw")
        .expect("qualified member key missing")
        .gates;
    assert_eq!(
        bare, qualified,
        "qualified alias must carry the expanded estimate"
    );
    assert!(qualified > 0, "member should have gates, got {qualified}");
}

#[test]
fn a_fn_call_costs_its_caller_one_gate_not_the_body() {
    let body = "  let a = x * 2 + 1
  let b = a * a - x
  return b / 3 + a
";
    let fn_src = format!(
        "fn work(x: int) -> int {{
{body}}}
"
    );
    let mod_src = format!(
        "mod work(x: int) -> int {{
{body}}}
"
    );
    let caller = "in go: exec
on go {
  let p = work(1)
  let q = work(p)
  let r = work(q)
}
";
    let fn_est = estimates_for(&format!("{fn_src}{caller}"));
    let mod_est = estimates_for(&format!("{mod_src}{caller}"));
    let handler = |est: &HashMap<String, ResourceEstimate>| {
        est.iter()
            .find(|(k, _)| k.as_str() != "work")
            .map(|(_, e)| e.gates)
            .expect("handler estimate")
    };
    let work = fn_est.get("work").expect("fn estimate");
    assert!(
        work.is_fn && work.gates > 3,
        "the fn's own estimate is its body: {}",
        work.gates
    );
    let body_gates = mod_est.get("work").expect("mod estimate").gates;
    assert_eq!(
        handler(&mod_est) - handler(&fn_est),
        3 * body_gates - 3,
        "three fn calls cost three Function Call gates where three mod calls inline three bodies"
    );
}
