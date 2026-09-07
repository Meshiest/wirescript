    use super::*;
    use crate::ir::Module;
    use crate::template::CompiledTemplate;

    fn make_template(name: &str) -> CompiledTemplate {
        let m = Module::new(name);
        CompiledTemplate::from_module(m)
    }

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
