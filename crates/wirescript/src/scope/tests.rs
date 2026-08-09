    use super::*;

    #[test]
    fn lookup_walks_up() {
        let mut s = Scope::new();
        s.insert("x", 1);
        s.push(ScopeTag::BLOCK);
        s.insert("y", 2);
        assert_eq!(s.get("x"), Some(&1));
        assert_eq!(s.get("y"), Some(&2));
    }

    #[test]
    fn inner_shadows_outer() {
        let mut s = Scope::new();
        s.insert("x", 1);
        s.push(ScopeTag::BLOCK);
        s.insert("x", 2);
        assert_eq!(s.get("x"), Some(&2));
        s.pop();
        assert_eq!(s.get("x"), Some(&1));
    }

    #[test]
    fn pop_discards() {
        let mut s = Scope::new();
        s.push(ScopeTag::BLOCK);
        s.insert("x", 1);
        s.pop();
        assert_eq!(s.get("x"), None);
    }

    #[test]
    fn get_mut_modifies_parent() {
        let mut s = Scope::new();
        s.insert("x", 1);
        s.push(ScopeTag::BLOCK);
        *s.get_mut("x").unwrap() = 99;
        s.pop();
        assert_eq!(s.get("x"), Some(&99));
    }

    #[test]
    fn iter_within_stops_at_module() {
        let mut s: Scope<&str> = Scope::new();
        s.insert("root_var", "a");
        s.push(ScopeTag::MODULE);
        s.insert("mod_out", "b");
        s.push(ScopeTag::BLOCK);
        s.insert("block_let", "c");

        let within: Vec<_> = s.iter_within(ScopeTag::MODULE).map(|(k, _)| k).collect();
        assert!(within.contains(&"mod_out"));
        assert!(within.contains(&"block_let"));
        assert!(!within.contains(&"root_var"));
    }

    #[test]
    fn iter_within_union_stops_at_nearest() {
        let mut s: Scope<i32> = Scope::new();
        s.insert("a", 1);
        s.push(ScopeTag::MODULE);
        s.insert("b", 2);
        s.push(ScopeTag::BLOCK);
        s.insert("c", 3);

        let within: Vec<_> = s.iter_within(ScopeTag::MODULE | ScopeTag::BLOCK)
            .map(|(k, _)| k).collect();
        assert!(within.contains(&"c"));
        assert!(!within.contains(&"b"));
    }
