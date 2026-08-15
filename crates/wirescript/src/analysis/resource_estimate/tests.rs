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
