    use std::sync::Arc;

    use super::*;
    use crate::GateIO;
    use crate::ir::{Module, ROOT_SCOPE_ID, ScopeInfo};
    use crate::lower::{LowerInput, lower};
    use crate::parser::parse;
    use crate::template_cache::TemplateCache;
    use crate::typecheck::typecheck;

    fn compile(src: &str) -> Module {
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

    fn lay(src: &str) -> RegionLayout {
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        layout_region(&root, &m.wires)
    }

    #[test]
    fn empty_module_composes_to_empty_bbox() {
        let lay = lay("");
        assert_eq!(lay.bbox, (0, 0));
        assert!(lay.local.is_empty());
    }

    #[test]
    fn every_node_gets_a_placement() {
        let src = "var n: int = 0\non RoundStart { n = n + 1 }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);
        for id in m.nodes.keys() {
            assert!(
                out.local.contains_key(id),
                "node {} missing a placement",
                id
            );
        }
    }

    #[test]
    fn no_two_placements_overlap() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);
        let mut seen: HashSet<(i32, i32)> = HashSet::default();
        for (id, p) in &out.local {
            assert!(
                seen.insert((p.dx, p.dy)),
                "node {} at ({}, {}) collides with a prior placement",
                id,
                p.dx,
                p.dy
            );
        }
    }

    #[test]
    fn if_branches_are_horizontally_separated() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);

        // Find a node from the then branch and one from the else branch;
        // they should sit at different dx ranges (side-by-side).
        let then_id = m
            .scopes
            .iter()
            .find(|(_, s)| matches!(s.kind, ScopeKind::IfThen))
            .map(|(id, _)| *id)
            .unwrap();
        let else_id = m
            .scopes
            .iter()
            .find(|(_, s)| matches!(s.kind, ScopeKind::IfElse))
            .map(|(id, _)| *id)
            .unwrap();

        let then_xs: Vec<i32> = m
            .nodes
            .values()
            .filter(|n| n.scope_id == then_id)
            .filter_map(|n| out.local.get(&n.id).map(|p| p.dx))
            .collect();
        let else_xs: Vec<i32> = m
            .nodes
            .values()
            .filter(|n| n.scope_id == else_id)
            .filter_map(|n| out.local.get(&n.id).map(|p| p.dx))
            .collect();

        if !then_xs.is_empty() && !else_xs.is_empty() {
            let then_max = *then_xs.iter().max().unwrap();
            let else_min = *else_xs.iter().min().unwrap();
            assert!(
                else_min > then_max,
                "else column ({}) must start to the right of then column end ({})",
                else_min,
                then_max
            );
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let src = "var n: int = 0\non RoundStart { n = n + 1 }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let a = layout_region(&root, &m.wires);
        let b = layout_region(&root, &m.wires);
        assert_eq!(a.local, b.local);
        assert_eq!(a.bbox, b.bbox);
    }

    #[test]
    fn synthetic_nested_regions_stack_vertically() {
        // Two sibling child regions, no own nodes, no wires. Their
        // placements must have different dy ranges.
        let mut m = Module::default();
        let r1 = 1;
        let r2 = 2;
        m.scopes.insert(
            r1,
            ScopeInfo {
                kind: ScopeKind::Block,
                source_range: make_range(0, 10),
                parent: Some(ROOT_SCOPE_ID),
            },
        );
        m.scopes.insert(
            r2,
            ScopeInfo {
                kind: ScopeKind::Block,
                source_range: make_range(20, 30),
                parent: Some(ROOT_SCOPE_ID),
            },
        );
        let na = make_node("a", r1, 5);
        let nb = make_node("b", r2, 25);
        let a_id = na.id;
        let b_id = nb.id;
        m.nodes.insert(a_id, na);
        m.nodes.insert(b_id, nb);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);
        let ya = out.local[&a_id].dy;
        let yb = out.local[&b_id].dy;
        assert!(ya < yb, "earlier block must stack above later one");
    }

    fn make_range(start: usize, end: usize) -> crate::diagnostic::SourceRange {
        crate::diagnostic::SourceRange {
            file: "t".into(),
            start: crate::diagnostic::Pos {
                offset: start,
                line: 0,
                col: 0,
            },
            end: crate::diagnostic::Pos {
                offset: end,
                line: 0,
                col: 0,
            },
        }
    }

    fn make_node(_label: &str, scope: crate::ir::ScopeId, offset: usize) -> Node {
        Node {
            id: crate::ir::NodeId::fresh(),
            kind: crate::ir::NodeKind::Gate,
            gate_class: "G",
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO::default()),
            source_range: make_range(offset, offset + 1),
            chip_id: None,
            chain_id: None,
            scope_id: scope,
            note: None,
        }
    }
