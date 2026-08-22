    use super::*;
    use crate::ir::Module;
    use crate::template::CompiledTemplate;

    fn make_template(name: &str) -> CompiledTemplate {
        let m = Module::new(name);
        CompiledTemplate::from_module(m)
    }

    /// A → B → C chain: C should appear before B, B before A.
    #[test]
    fn topo_order_leaves_first() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["C"]);
        // C is a leaf — no explicit registration needed, but register it so
        // it is definitely in the graph.
        cache.register_dependency("C", &[]);

        let order = cache.topo_order();

        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("C") < pos("B"), "C must come before B");
        assert!(pos("B") < pos("A"), "B must come before A");
    }

    /// A → C and B → C: C must appear before both A and B.
    #[test]
    fn topo_order_independent_modules_adjacent() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["C"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &[]);

        let order = cache.topo_order();

        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("C") < pos("A"), "C must come before A");
        assert!(pos("C") < pos("B"), "C must come before B");
    }

    /// A → {C, D}; B → C; C and D are leaves.
    /// Expected: tier 0 = [C, D], A and B in later tiers.
    #[test]
    fn parallel_tiers_groups_independent_work() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["C", "D"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &[]);
        cache.register_dependency("D", &[]);

        let tiers = cache.parallel_tiers();

        assert!(!tiers.is_empty(), "expected at least one tier");

        // Tier 0 must be the two leaves.
        assert_eq!(
            tiers[0],
            vec!["C".to_string(), "D".to_string()],
            "tier 0 should be [C, D] (sorted)"
        );

        // A and B must appear in tiers after 0.
        let a_tier = tiers.iter().position(|t| t.contains(&"A".to_string()));
        let b_tier = tiers.iter().position(|t| t.contains(&"B".to_string()));
        assert!(a_tier.unwrap() > 0, "A should be in a tier after 0");
        assert!(b_tier.unwrap() > 0, "B should be in a tier after 0");
    }

    /// Basic insert + get round-trip.
    #[test]
    fn cache_stores_and_retrieves() {
        let cache = TemplateCache::new();
        let t = make_template("test");
        cache.insert("mymod", t);

        let retrieved = cache.get("mymod");
        assert!(
            retrieved.is_some(),
            "expected to retrieve inserted template"
        );
        assert!(cache.get("missing").is_none());
    }

    /// Diamond: A → B, C; B → D; C → D; D leaf.
    /// D must be in tier 0, B and C in the same tier, A last.
    #[test]
    fn diamond_dependency() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B", "C"]);
        cache.register_dependency("B", &["D"]);
        cache.register_dependency("C", &["D"]);
        cache.register_dependency("D", &[]);

        let tiers = cache.parallel_tiers();
        let order = cache.topo_order();

        // D must be in tier 0.
        assert_eq!(tiers[0], vec!["D".to_string()], "tier 0 must be [D]");

        // B and C must be in the same tier (tier 1).
        let bc_tier_idx = tiers
            .iter()
            .position(|t| t.contains(&"B".to_string()))
            .unwrap();
        let c_tier_idx = tiers
            .iter()
            .position(|t| t.contains(&"C".to_string()))
            .unwrap();
        assert_eq!(bc_tier_idx, c_tier_idx, "B and C must be in the same tier");
        assert!(bc_tier_idx > 0, "B and C must not be in tier 0");

        // A must be after B and C in topo order.
        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("D") < pos("B"), "D before B");
        assert!(pos("D") < pos("C"), "D before C");
        assert!(pos("B") < pos("A"), "B before A");
        assert!(pos("C") < pos("A"), "C before A");
    }

    /// A self-recursive dependency must not hang topo_order.
    #[test]
    fn self_recursive_does_not_hang() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["A"]);

        // Must return without hanging; a cycle means Kahn's algorithm will
        // simply not emit the cyclic node(s).
        let order = cache.topo_order();
        assert!(
            order.len() <= 1,
            "expected at most 1 node in result, got {:?}",
            order
        );
    }

    /// Mutual recursion A → B, B → A must not hang.
    #[test]
    fn mutual_recursion_does_not_hang() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["A"]);

        let order = cache.topo_order();
        assert!(
            order.len() < 2,
            "cycle nodes should not appear in topo order, got {:?}",
            order
        );
    }

    /// Both a "Used" and an "Unused" leaf module must appear in tier 0.
    #[test]
    fn unused_module_still_in_tiers() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("Used", &[]);
        cache.register_dependency("Unused", &[]);

        let tiers = cache.parallel_tiers();
        assert_eq!(tiers.len(), 1, "expected exactly one tier");
        assert!(
            tiers[0].contains(&"Used".to_string()),
            "tier 0 should contain 'Used'"
        );
        assert!(
            tiers[0].contains(&"Unused".to_string()),
            "tier 0 should contain 'Unused'"
        );
    }

    /// A single module with no deps produces exactly one tier containing it.
    #[test]
    fn single_module_no_deps() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("Solo", &[]);

        let tiers = cache.parallel_tiers();
        assert_eq!(tiers.len(), 1, "expected exactly 1 tier");
        assert_eq!(tiers[0], vec!["Solo".to_string()]);
    }

    /// BFS reachability: Used+Dep reachable from Used; Unused is not.
    #[test]
    fn reachable_filters_unused() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("Used", &["Dep"]);
        cache.register_dependency("Dep", &[]);
        cache.register_dependency("Unused", &["Dep"]);
        let reachable = cache.reachable_from(&["Used"]);
        assert!(reachable.contains("Used"));
        assert!(reachable.contains("Dep"));
        assert!(!reachable.contains("Unused"));
    }

    /// BFS reachability: A→B→C; X not reachable from A.
    #[test]
    fn reachable_transitive() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &[]);
        cache.register_dependency("X", &[]);
        let reachable = cache.reachable_from(&["A"]);
        assert_eq!(reachable.len(), 3); // A, B, C — not X
        assert!(reachable.contains("A"));
        assert!(reachable.contains("C"));
        assert!(!reachable.contains("X"));
    }

    /// scan_declarations finds chip and mod deps correctly.
    #[test]
    fn scan_finds_chip_and_mod_deps() {
        let parsed = crate::parser::parse(
            "chip ALU(a: int, b: int) -> (r: int) { out r = a + b }\n\
             mod process(x: int) -> (r: int) { return ALU(x, 1) }\n\
             let p = process(10)\n\
             out result = p",
            "test.ws",
        );
        let mut cache = TemplateCache::new();
        cache.scan_declarations(&parsed.ast);
        let deps = cache.deps.read().unwrap();
        assert!(deps.contains_key("ALU"));
        assert!(deps.contains_key("process"));
        assert!(deps["ALU"].is_empty());
        assert!(deps["process"].contains("ALU"));
    }

    /// scan_top_level_calls finds roots in top-level expressions.
    #[test]
    fn scan_top_level_calls_finds_roots() {
        let parsed = crate::parser::parse(
            "chip ALU(a: int, b: int) -> (r: int) { out r = a + b }\n\
             mod process(x: int) -> (r: int) { return ALU(x, 1) }\n\
             let p = process(10)\n\
             out result = p",
            "test.ws",
        );
        let mut cache = TemplateCache::new();
        cache.scan_declarations(&parsed.ast);
        let roots = cache.scan_top_level_calls(&parsed.ast);
        assert!(
            roots.contains(&"process".to_string()),
            "process is called at top level"
        );
        assert!(
            !roots.contains(&"ALU".to_string()),
            "ALU is only called inside process, not at top level"
        );
    }

    /// A → B → C → D → E (5-node chain): 5 tiers, E first, A last.
    #[test]
    fn long_chain_correct_order() {
        let mut cache = TemplateCache::new();
        cache.register_dependency("A", &["B"]);
        cache.register_dependency("B", &["C"]);
        cache.register_dependency("C", &["D"]);
        cache.register_dependency("D", &["E"]);
        cache.register_dependency("E", &[]);

        let tiers = cache.parallel_tiers();
        let order = cache.topo_order();

        assert_eq!(
            tiers.len(),
            5,
            "expected 5 tiers for a 5-node chain, got {:?}",
            tiers
        );
        assert_eq!(tiers[0], vec!["E".to_string()], "tier 0 must be [E]");
        assert_eq!(tiers[4], vec!["A".to_string()], "tier 4 must be [A]");

        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        assert!(pos("E") < pos("D"), "E before D");
        assert!(pos("D") < pos("C"), "D before C");
        assert!(pos("C") < pos("B"), "C before B");
        assert!(pos("B") < pos("A"), "B before A");
    }
