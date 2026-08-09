    use std::sync::Arc;

    use super::*;
    use crate::ir::{GateIO, ROOT_SCOPE_ID};
    use crate::lower::{LowerInput, lower};
    use crate::parser::parse;
    use crate::template_cache::TemplateCache;
    use crate::typecheck::typecheck;
    use crate::{Module, lexer};

    fn compile(src: &str) -> Module {
        // Exercise the full pipeline so scopes match the real lowering.
        let _ = lexer::lex(src, "test"); // compiled-but-unused; parser also lexes
        let parsed = parse(src, "test");
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diags: {:?}",
            parsed.diagnostics
        );
        let tc = typecheck(&parsed.ast, "test");
        let r = lower(LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: Arc::new(TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
        });
        r.module
    }

    #[test]
    fn empty_module_yields_root_only() {
        let m = compile("");
        let root = build_region_tree(&m);
        assert_eq!(root.id, ROOT_SCOPE_ID);
        assert!(root.own_nodes.is_empty());
        assert!(root.children.is_empty());
    }

    #[test]
    fn handler_gets_one_child_region() {
        let m = compile("on RoundStart { }");
        let root = build_region_tree(&m);
        assert_eq!(root.children.len(), 1);
        let handler = &root.children[0];
        assert!(matches!(
            &handler.info.kind,
            crate::ir::ScopeKind::HandlerBody { .. }
        ));
    }

    #[test]
    fn if_else_builds_group_with_three_children() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = build_region_tree(&m);
        // root > handler_body > if_group > (cond, then, else)
        let handler = root
            .children
            .iter()
            .find(|r| matches!(&r.info.kind, crate::ir::ScopeKind::HandlerBody { .. }))
            .expect("handler region");
        let group = handler
            .children
            .iter()
            .find(|r| matches!(&r.info.kind, crate::ir::ScopeKind::IfGroup))
            .expect("if group region");
        assert_eq!(group.children.len(), 3);
        let kinds: Vec<&crate::ir::ScopeKind> =
            group.children.iter().map(|r| &r.info.kind).collect();
        // Sorted by source range: IfCond starts first, then IfThen, then IfElse.
        assert!(matches!(kinds[0], crate::ir::ScopeKind::IfCond));
        assert!(matches!(kinds[1], crate::ir::ScopeKind::IfThen));
        assert!(matches!(kinds[2], crate::ir::ScopeKind::IfElse));
    }

    #[test]
    fn node_count_matches_module_total() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = build_region_tree(&m);
        assert_eq!(region_node_count(&root), m.nodes.len());
    }

    #[test]
    fn scope_count_matches_module_total() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = build_region_tree(&m);
        assert_eq!(region_scope_count(&root), m.scopes.len());
    }

    #[test]
    fn orphan_nodes_land_on_root_region() {
        // Synthesize a module where a node points to a missing scope —
        // the tree builder must not drop it.
        let mut m = Module::default();
        let nid = crate::ir::NodeId::fresh();
        let node = Node {
            id: nid,
            kind: crate::ir::NodeKind::Gate,
            gate_class: "G",
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO::default()),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 9999, // bogus
            note: None,
        };
        m.nodes.insert(nid, node);

        let root = build_region_tree(&m);
        assert_eq!(root.own_nodes.len(), 1, "orphan must be re-homed to root");
    }
