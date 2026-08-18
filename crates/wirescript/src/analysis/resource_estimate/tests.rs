    use super::*;
    use crate::resolve::{resolve, FsLoader};
    use crate::typecheck::typecheck;

    fn estimates_for(source: &str) -> HashMap<String, ResourceEstimate> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test", &crate::typecheck::CeSlotMap::default());
        collect_estimates(&resolved.ast, &tc, "test")
    }

    #[test]
    fn basic_chip_has_gates_and_wires() {
        let est = estimates_for("chip Add(a: int, b: int) -> (r: int) { out r = a + b }");
        let add = est.get("Add").expect("should have Add estimate");
        assert!(add.gates > 0, "chip should have gates, got {}", add.gates);
        assert!(add.wires > 0, "chip should have wires, got {}", add.wires);
    }

    #[test]
    fn mod_has_gates() {
        let est = estimates_for("mod inc(v: *int) { v = v + 1 }");
        let inc = est.get("inc").expect("should have inc estimate");
        assert!(inc.gates > 0, "mod should have gates, got {}", inc.gates);
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
        assert!(say.gates > 0, "builtin gate still counts toward gates: {}", say.gates);
        assert_eq!(say.total_microchips, 0, "no phantom microchip for a builtin call");
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
        assert!(outer.total_microchips >= 1, "a non-inline chip call is a microchip: {}", outer.total_microchips);
    }

    /// A `import * as ns` member is stored in `symbols` — and looked up by
    /// hover — under its qualified `ns.member` name, while estimates are
    /// collected under the bare one. Without the qualified alias, hovering
    /// `card.draw(...)` rendered the signature with no gate count at all.
    #[test]
    fn namespaced_member_is_reachable_by_its_qualified_name() {
        let dir = std::env::temp_dir().join("ws_est_ns_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.ws"), "mod draw(v: *int) { v = v + v + 1 }\n").unwrap();
        let main = dir.join("main.ws");
        std::fs::write(&main, "import * as card from \"lib\"\nvar x: int = 0\non RoundStart { card.draw(x) }\n").unwrap();

        let src = std::fs::read_to_string(&main).unwrap();
        let f = main.to_string_lossy().to_string();
        let resolved = resolve(&src, &f, &FsLoader);
        let tc = typecheck(&resolved.ast, &f, &crate::typecheck::CeSlotMap::default());
        let est = collect_estimates(&resolved.ast, &tc, &f);

        let bare = est.get("draw").expect("bare member key").gates;
        let qualified = est.get("card.draw").expect("qualified member key missing").gates;
        assert_eq!(bare, qualified, "qualified alias must carry the expanded estimate");
        assert!(qualified > 0, "member should have gates, got {qualified}");
    }
