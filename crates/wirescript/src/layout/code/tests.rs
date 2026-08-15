    use std::sync::Arc;

    use super::*;
    use crate::diagnostic::Pos;
    use crate::ir::{GateIO, ROOT_SCOPE_ID, SourceRange, Wire, gate_class};

    fn lowered(src: &str) -> Module {
        let parsed = crate::parser::parse(src, "test");
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let tc = crate::typecheck::typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
        let r = crate::lower::lower(crate::lower::LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
            ce_slots: &crate::typecheck::CeSlotMap::default(),
        });
        r.module
    }

    /// Lower an entry file plus the in-memory files it imports, the way
    /// `compile` does, and return the module alongside layout options
    /// carrying the ENTRY file's source map.
    fn lowered_with_imports(entry: &str, files: &[(&str, &str)]) -> (Module, LayoutOptions) {
        let loader = crate::resolve::MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let resolved = crate::resolve::resolve(entry, "main.ws", &loader);
        assert!(
            !resolved
                .diagnostics
                .iter()
                .any(|d| matches!(d.severity, crate::diagnostic::Severity::Error)),
            "{:?}",
            resolved.diagnostics
        );
        let tc = crate::typecheck::typecheck(&resolved.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        let r = crate::lower::lower(crate::lower::LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "main.ws",
            module_name: None,
            template_cache: Arc::new(crate::template_cache::TemplateCache::new()),
            doc_comments: &resolved.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
            ce_slots: &crate::typecheck::CeSlotMap::default(),
        });
        let opts = LayoutOptions {
            source_map: Some(resolved.source_map.clone()),
            ..Default::default()
        };
        (r.module, opts)
    }

    fn make_range(file: &str, line: u32, col: u32, start: usize, end: usize) -> SourceRange {
        SourceRange {
            file: file.into(),
            start: Pos {
                offset: start,
                line,
                col,
            },
            end: Pos {
                offset: end,
                line,
                col: col + (end - start) as u32,
            },
        }
    }

    fn make_node(gate_class: &'static str, range: SourceRange) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO::default()),
            source_range: range,
            chip_id: None,
            chain_id: None,
            scope_id: ROOT_SCOPE_ID,
            note: None,
        }
    }

    fn wire(from: NodeId, to: NodeId) -> Wire {
        Wire {
            source: from.port(WirePort::Output),
            target: to.port(WirePort::Input),
        }
    }

    fn module_with(nodes: Vec<Node>, wires: Vec<Wire>) -> Module {
        let mut m = Module::new("t");
        for n in nodes {
            m.add_node(n);
        }
        m.wires = wires;
        m
    }

    fn opts() -> LayoutOptions {
        LayoutOptions::default()
    }

    /// `opts()` with the mode set, for the tests that lay a chip tree out
    /// recursively. `layout_code` places THIS module whatever the mode says,
    /// but the recursion goes back through `layout_with_opts`, which dispatches
    /// on it — so a default-mode recursive call lays every chip interior out in
    /// DAG mode and no chip ever builds a bus.
    fn code_opts() -> LayoutOptions {
        LayoutOptions {
            mode: crate::layout::LayoutMode::Code,
            ..LayoutOptions::default()
        }
    }

    #[test]
    fn earlier_line_sits_above_later_line() {
        let m = lowered("var a: int = 0\nvar b: int = 0\n");
        let l = layout_code(&m, &opts(), false);
        let a = m
            .nodes
            .values()
            .find(|n| n.source_range.start.line == 1)
            .expect("line 1 node");
        let b = m
            .nodes
            .values()
            .find(|n| n.source_range.start.line == 2)
            .expect("line 2 node");
        assert!(
            l.placements[&a.id].x > l.placements[&b.id].x,
            "earlier line must sit higher (greater Placement.x)"
        );
    }

    #[test]
    fn same_line_nodes_flow_left_to_right_in_token_order() {
        let a = make_node("G", make_range("f", 1, 0, 0, 1));
        let b = make_node("G", make_range("f", 1, 4, 4, 5));
        let c = make_node("G", make_range("f", 1, 8, 8, 9));
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let m = module_with(vec![a, b, c], vec![]);
        let l = layout_code(&m, &opts(), false);
        assert!(l.placements[&a_id].y < l.placements[&b_id].y);
        assert!(l.placements[&b_id].y < l.placements[&c_id].y);
    }

    #[test]
    fn statement_value_sink_heads_its_line_on_the_left() {
        // The `Var_Set` is the statement's sink — it takes the exec and
        // nothing on the line reads it back — so it is pinned to the line's
        // left column and reads its value from the right. Every other gate
        // on the line, the whole expression it consumes, sits strictly
        // right of it.
        let m = lowered("var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }");
        let l = layout_code(&m, &opts(), false);
        let var_set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_SET)
            .expect("Var_Set node");
        let line = var_set.source_range.start.line;
        let set_y = l.placements[&var_set.id].y;
        let mut expression_gates = 0usize;
        for n in m.nodes.values().filter(|n| {
            n.kind == NodeKind::Gate && n.source_range.start.line == line && n.id != var_set.id
        }) {
            expression_gates += 1;
            assert!(
                l.placements[&n.id].y > set_y,
                "expression gate {} must sit RIGHT of the sink it feeds (y={}, sink y={set_y})",
                n.gate_class,
                l.placements[&n.id].y
            );
        }
        assert!(
            expression_gates >= 4,
            "fixture must lower an expression tree, got {expression_gates} gates"
        );
    }

    #[test]
    fn expression_operands_sit_left_of_the_operator_they_feed() {
        // `(a + b) * (a + 2)` — the two adds FEED the multiply, so both sit
        // strictly LEFT of it, and they occupy different sub-rows.
        let src = "var a: int = 1\nvar b: int = 2\nin go: exec\non go {\n  let m = (a + b) * (a + 2)\n  PrintToConsole(\"${m}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let by_class = |c: &str| -> Vec<NodeId> {
            let mut v: Vec<NodeId> = m
                .nodes
                .values()
                .filter(|n| n.gate_class.ends_with(c))
                .map(|n| n.id)
                .collect();
            v.sort();
            v
        };
        let mul = by_class("MathMultiply");
        let add = by_class("MathAdd");
        assert!(!mul.is_empty() && add.len() >= 2);
        let mul_y = l.placements[&mul[0]].y;
        for a in &add {
            assert!(
                l.placements[a].y < mul_y,
                "operand {a} must sit LEFT of the operator it feeds (operand y={}, operator y={mul_y})",
                l.placements[a].y
            );
        }
        // The two adds are siblings: same depth column, different rows.
        assert_ne!(
            l.placements[&add[0]].x, l.placements[&add[1]].x,
            "sibling operands must occupy different sub-rows"
        );
    }

    #[test]
    fn sequenced_statements_on_one_line_keep_source_order() {
        // The exec wire runs `a = 1` -> `b = 2`. Counted as an operand edge
        // it would make `b = 2` the line's root and print the line backwards.
        let src = "var a: int = 0\nvar b: int = 0\nin go: exec\non go { a = 1  b = 2 }\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        let mut sets: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| n.gate_class == gate_class::VAR_SET)
            .collect();
        sets.sort_by_key(|n| n.source_range.start.offset);
        assert_eq!(sets.len(), 2, "one Var_Set per statement");
        assert_eq!(
            sets[0].source_range.start.line, sets[1].source_range.start.line,
            "fixture must put both statements on one line"
        );
        assert!(
            l.placements[&sets[0].id].y < l.placements[&sets[1].id].y,
            "the first statement must sit left of the second"
        );
    }

    #[test]
    fn a_trigger_sharing_a_line_with_its_statement_stays_leftmost() {
        // The event node and the statement it fires share line 2, joined by
        // an exec wire — read as an operand edge, that wire pushes the
        // trigger to the right of its own body.
        let src = "var a: int = 0\non RoundStart() { a = 1 }\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        let y_of = |kind: NodeKind| -> i32 {
            let n = m
                .nodes
                .values()
                .find(|n| n.source_range.start.line == 2 && n.kind == kind)
                .unwrap_or_else(|| panic!("a {kind:?} node on line 2"));
            l.placements[&n.id].y
        };
        assert!(
            y_of(NodeKind::Event) < y_of(NodeKind::Gate),
            "the handler's trigger must stay left of the statement it fires"
        );
    }

    #[test]
    fn flat_line_layout_is_unchanged_by_tree_ordering() {
        // A line with no in-line nesting must keep the single-row shape.
        let src = "var a: int = 1\nin go: exec\non go {\n  a = 1\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let rows: std::collections::HashSet<i32> = l.placements.values().map(|p| p.x).collect();
        assert!(
            rows.len() <= 3,
            "flat program should stay compact, got {} rows",
            rows.len()
        );
    }

    #[test]
    fn indent_column_shifts_line_right() {
        let a = make_node("G", make_range("f", 1, 0, 0, 1));
        let b = make_node("G", make_range("f", 2, 2, 10, 11));
        let (a_id, b_id) = (a.id, b.id);
        let m = module_with(vec![a, b], vec![]);
        let l = layout_code(&m, &opts(), false);
        assert_eq!(
            l.placements[&b_id].y - l.placements[&a_id].y,
            2 * INDENT_UNIT
        );
    }

    /// `lowered` parses as `"test"`, so the map must claim the same file or
    /// the anchor guard sends the layout to its no-source-map fallbacks.
    fn opts_with_map(src: &str) -> LayoutOptions {
        LayoutOptions {
            source_map: Some(std::sync::Arc::new(crate::ast::SourceMap::from_source(
                src, "test",
            ))),
            ..Default::default()
        }
    }

    /// y of the head node of the 1-based source `line`, i.e. the node the
    /// line's indent is applied to.
    fn head_y(m: &Module, l: &LayoutResult, line: u32) -> i32 {
        let mut heads: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| n.source_range.start.line == line && !is_edge_pin(n))
            .collect();
        heads.sort_by_key(|n| (n.source_range.start.offset, Reverse(n.source_range.end.offset)));
        l.placements[&heads.first().expect("a node on that line").id].y
    }

    #[test]
    fn indent_comes_from_source_line_not_first_node_column() {
        // Both statements start at source column 0, but their first IR
        // nodes are the `+` expressions — at columns 18 and 9. Keying the
        // indent off the node column staggers two unindented lines.
        let src = "let aaaaaaaaaa = 1 + 2\nlet b = 3 + 4\n";
        let m = lowered(src);

        let fallback = layout_code(&m, &opts(), false);
        assert_ne!(
            head_y(&m, &fallback, 1),
            head_y(&m, &fallback, 2),
            "fixture must actually exercise the first-node-column path"
        );

        let mapped = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(
            head_y(&m, &mapped, 1),
            head_y(&m, &mapped, 2),
            "two column-0 statements must share the same left margin"
        );
    }

    #[test]
    fn source_map_indent_shifts_a_line_by_its_own_column() {
        // Line 2's statement is indented two columns. Under the node
        // columns alone (12 and 11) it would sit one unit LEFT of line 1.
        let src = "let aaaa = 1 + 2\n  let b = 3 + 4\n";
        let m = lowered(src);

        let fallback = layout_code(&m, &opts(), false);
        assert!(
            head_y(&m, &fallback, 2) < head_y(&m, &fallback, 1),
            "fixture must actually exercise the first-node-column path"
        );

        let l = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(
            head_y(&m, &l, 2) - head_y(&m, &l, 1),
            2 * INDENT_UNIT,
            "a two-column indent shifts the line by two indent units"
        );
    }

    #[test]
    fn nested_statement_indent_scales_with_source_column() {
        let src = "in go: exec\non go {\n  PrintToConsole(\"x\")\n}\n";
        let sm = crate::ast::SourceMap::from_source(src, "test");
        // line 2 (0-based) is indented two spaces
        assert_eq!(sm.line_indent[2], 2);
        assert_eq!(sm.line_indent[0], 0);
        assert_eq!(sm.line_indent[3], 0);
    }

    #[test]
    fn own_line_comments_get_their_own_row() {
        // The handler needs a body: an empty one lowers to no nodes, leaving
        // a single row for the comment to be trivially "between".
        let src = "var x: int = 0\nin go: exec\n// a standalone note\non go { x = 1 }\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(l.annotations.len(), 1, "one own-line comment renders");
        let a = &l.annotations[0];
        assert_eq!(a.text, "a standalone note");
        // Sits between the `var x` row and the `on go` row.
        let rows: Vec<i32> = l.placements.values().map(|p| p.x).collect();
        let top = *rows.iter().max().unwrap();
        let bottom = *rows.iter().min().unwrap();
        assert!(
            a.x < top && a.x > bottom,
            "comment row {} must fall between the code rows {bottom}..{top}",
            a.x
        );
    }

    #[test]
    fn comment_labels_start_at_their_own_source_indent() {
        let src =
            "var x: int = 0\nin go: exec\non go {\n  // indented note\n  x = 1\n}\n// flush note\nvar y: int = 0\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        let y_of = |text: &str| -> i32 {
            l.annotations
                .iter()
                .find(|a| a.text == text)
                .unwrap_or_else(|| panic!("{text} should render; got {:?}", l.annotations))
                .y
        };
        assert_eq!(
            y_of("indented note") - y_of("flush note"),
            2 * INDENT_UNIT,
            "a two-column indent shifts the label by two indent units"
        );
    }

    #[test]
    fn comment_rows_stay_inside_the_plane_and_clear_of_every_gate() {
        let src = "// header note\nvar x: int = 0\nin go: exec\non go {\n  // step one\n  x = x + 1\n}\n// footer note\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        assert_eq!(l.annotations.len(), 3, "{:?}", l.annotations);

        let e = crate::layout::wall::plane_extent(&l);
        for a in &l.annotations {
            assert!(
                a.x >= -e.x && a.x + ANNOTATION_SIZE <= e.x,
                "{:?} x-range [{}, {}] escapes plane extent {}",
                a.text,
                a.x,
                a.x + ANNOTATION_SIZE,
                e.x
            );
            assert!(
                a.y >= -e.y && a.y + ANNOTATION_SIZE <= e.y,
                "{:?} y-range [{}, {}] escapes plane extent {}",
                a.text,
                a.y,
                a.y + ANNOTATION_SIZE,
                e.y
            );
        }

        // The game DROPS overlapping bricks at load, so a comment's carrier
        // brick must clear every gate's footprint.
        for a in &l.annotations {
            for (id, p) in &l.placements {
                let (hsx, hsy) = measured_half_size(&m.nodes[id], &l);
                let disjoint = a.z != p.z
                    || a.x + ANNOTATION_SIZE <= p.x
                    || p.x + hsx * 2 <= a.x
                    || a.y + ANNOTATION_SIZE <= p.y
                    || p.y + hsy * 2 <= a.y;
                assert!(disjoint, "comment {:?} overlaps node {id}", a.text);
            }
        }
    }

    #[test]
    fn each_comment_lands_on_exactly_one_plane() {
        let src = "// file note\nin go: exec\nchip C(t: exec) {\n  on t {\n    PrintToConsole(\"a\")\n    // inner note\n    PrintToConsole(\"b\")\n  }\n}\nlet c = C(go)\n";
        let m = lowered(src);
        let o = opts_with_map(src);
        let texts = |l: &LayoutResult| -> Vec<String> {
            l.annotations.iter().map(|a| a.text.clone()).collect()
        };

        // The chip's own rows bracket the inner note, so the root skips it;
        // the leading file note belongs to no chip, so the root keeps it.
        assert_eq!(texts(&layout_code(&m, &o, false)), ["file note"]);

        let chip = m.chips.values().next().expect("chip module");
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        assert_eq!(texts(&layout_code(chip, &nested, false)), ["inner note"]);
    }

    /// A note inside an array literal is not rendered.
    ///
    /// A data table carries a note per row, each costing a brick and each
    /// saying much the same thing — the highest-volume, lowest-value comments
    /// on a plane. Notes outside the brackets are untouched.
    #[test]
    fn comments_inside_array_literals_are_not_rendered() {
        let src = "// kept: before the table
in go: exec
let table = [
  // dropped: a row note
  1,
  // dropped: another row note
  2,
]
// kept: after the table
on go { PrintToConsole(\"${table[0]}\") }
";
        let m = lowered(src);
        let o = opts_with_map(src);
        let l = layout_code(&m, &o, false);
        let texts: Vec<String> = l.annotations.iter().map(|a| a.text.clone()).collect();
        assert!(
            texts.iter().all(|t| !t.starts_with("dropped")),
            "array-literal notes must not reach the plane, got {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "kept: before the table"),
            "a note outside the brackets must still render, got {texts:?}"
        );
    }

    /// A comment is rendered by exactly ONE plane in the whole tree, even when
    /// sibling modules' line ranges OVERLAP.
    ///
    /// `line_span` is a min/max ENVELOPE, not the set of lines a module
    /// occupies. A module whose nodes are scattered — a `mod` inlined at two
    /// distant call sites, an anon chip partitioned out of a long handler —
    /// gets a window covering everything in between, and sibling windows then
    /// overlap almost entirely. The claim excludes a module's own CHILDREN,
    /// which never excludes a SIBLING, so every comment in the overlap is
    /// rendered once per sibling.
    ///
    /// Measured on a real program before the fix: 951 own-line comments in the
    /// source became 2958 comment bricks, with three sibling modules each
    /// claiming ~940 of them.
    ///
    /// Built from synthetic modules rather than lowered source because the
    /// shape is a property of the SPANS, and which nodes lowering hands to
    /// which chip is not something a fixture can pin down.
    #[test]
    fn overlapping_sibling_modules_do_not_each_claim_the_same_comment() {
        // Two chips whose own rows are sparse and far apart: both start at
        // line 3 and run to 11 and 16. Their envelopes overlap over 3..11.
        let a = module_with(
            vec![
                make_node("G", make_range("test", 3, 0, 0, 1)),
                make_node("G", make_range("test", 11, 0, 2, 3)),
            ],
            vec![],
        );
        let b = module_with(
            vec![
                make_node("G", make_range("test", 3, 0, 4, 5)),
                make_node("G", make_range("test", 16, 0, 6, 7)),
            ],
            vec![],
        );

        let a_id = NodeId::fresh();
        let b_id = NodeId::fresh();
        let mut root = module_with(
            vec![
                make_node("G", make_range("test", 1, 0, 8, 9)),
                make_node("G", make_range("test", 18, 0, 10, 11)),
            ],
            vec![],
        );
        root.chips.insert(a_id, a);
        root.chips.insert(b_id, b);

        // A file whose line 10 carries an own-line comment, inside both
        // chips' envelopes and inside neither's children.
        let src = "








// shared note








";
        let o = opts_with_map(src);
        let anchor: Arc<str> = "test".into();

        // Ownership is settled for the whole tree, then each plane reads it —
        // the same path `layout_code` takes.
        let o = LayoutOptions {
            comment_owner: Some(Arc::new(assign_comment_owners(&root, &o, &anchor))),
            ..o
        };
        let nested = |chip: NodeId| LayoutOptions {
            nested: true,
            self_chip: Some(chip),
            ..o.clone()
        };
        let (na, nb) = (nested(a_id), nested(b_id));
        let root_claims = claimed_comments(&root, &o, &anchor);
        let a_claims = claimed_comments(&root.chips[&a_id], &na, &anchor);
        let b_claims = claimed_comments(&root.chips[&b_id], &nb, &anchor);

        let claimants: Vec<&str> = [
            ("root", root_claims.contains_key(&10)),
            ("chip_a", a_claims.contains_key(&10)),
            ("chip_b", b_claims.contains_key(&10)),
        ]
        .into_iter()
        .filter(|(_, has)| *has)
        .map(|(n, _)| n)
        .collect();

        assert_eq!(
            claimants.len(),
            1,
            "the note on line 10 must be rendered by exactly one plane, got              {claimants:?}"
        );
    }



    /// Entry file `main.ws` (lines 1..=7) and imported `lib.ws` (lines
    /// 1..=9): the note on main.ws line 6 also falls inside the imported
    /// chip's own lib.ws span, so a plane matching comment lines against
    /// rows without checking whose file they number renders it twice.
    const IMPORTED_CHIP_LIB: &str =
        "chip Helper(t: exec) {\n  var n: int = 0\n  on t {\n    n = n + 1\n    n = n * 2\n    n = n + 3\n    n = n - 4\n  }\n}\n";
    const IMPORTED_CHIP_MAIN: &str =
        "import { Helper } from \"lib\"\n\nin go: exec\n\n\n// a note on line six\nlet h = Helper(go)\n";

    #[test]
    fn a_comment_is_not_reclaimed_by_a_plane_from_another_file() {
        let (m, o) = lowered_with_imports(IMPORTED_CHIP_MAIN, &[("lib.ws", IMPORTED_CHIP_LIB)]);
        let texts = |l: &LayoutResult| -> Vec<String> {
            l.annotations.iter().map(|a| a.text.clone()).collect()
        };

        assert_eq!(texts(&layout_code(&m, &o, false)), ["a note on line six"]);

        let chip = m.chips.values().next().expect("imported chip module");
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        assert!(
            texts(&layout_code(chip, &nested, false)).is_empty(),
            "a plane anchored on lib.ws claims none of main.ws's comments"
        );
    }

    #[test]
    fn indent_comes_from_the_planes_own_file_not_the_entry_files_map() {
        // lib.ws lines 4 and 5 share a source column, so the chip's rows for
        // them share a left margin. main.ws indents its own line 4 by eight
        // columns and its line 5 by none — reading the entry file's map on a
        // lib.ws-anchored plane would stagger the two rows by that much.
        let lib = "chip Helper(t: exec) {\n  var n: int = 0\n  on t {\n    n = n + 1\n    n = n + 2\n  }\n}\n";
        let main = "import { Helper } from \"lib\"\n\nin go: exec\n        let h = Helper(go)\nout done = h\n";
        let (m, o) = lowered_with_imports(main, &[("lib.ws", lib)]);
        let map = o.source_map.as_ref().expect("entry source map");
        assert_eq!(
            (map.line_indent[3], map.line_indent[4]),
            (8, 0),
            "fixture must actually stagger the entry file's lines 4 and 5"
        );

        let chip = m.chips.values().next().expect("imported chip module");
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        let l = layout_code(chip, &nested, false);
        assert_eq!(
            head_y(chip, &l, 4),
            head_y(chip, &l, 5),
            "two lib.ws lines at the same column must share a left margin"
        );
    }

    #[test]
    fn a_comment_in_a_doubly_nested_chip_is_claimed_once() {
        // The outer chip's own rows are just the inner chip's node, so a
        // module's claim has to account for its grandchildren's rows too or
        // both the root and the inner chip render this note.
        let src = "var g: int = 0\nin go: exec\non go {\n  chip {\n    chip {\n      g = g + 1\n      // deep note\n      g = g + 2\n    }\n  }\n}\n";
        let m = lowered(src);
        let o = opts_with_map(src);
        let nested = LayoutOptions {
            nested: true,
            ..o.clone()
        };
        let texts = |l: &LayoutResult| -> Vec<String> {
            l.annotations.iter().map(|a| a.text.clone()).collect()
        };

        let outer = m.chips.values().next().expect("outer anon chip");
        let inner = outer.chips.values().next().expect("inner anon chip");

        assert!(texts(&layout_code(&m, &o, false)).is_empty(), "root");
        assert!(
            texts(&layout_code(outer, &nested, false)).is_empty(),
            "outer chip"
        );
        assert_eq!(texts(&layout_code(inner, &nested, false)), ["deep note"]);
    }

    #[test]
    fn trailing_comments_are_not_rendered() {
        let src = "in go: exec\non go { } // trailing\n";
        let m = lowered(src);
        let l = layout_code(&m, &opts_with_map(src), false);
        assert!(l.annotations.is_empty());
    }

    #[test]
    fn dag_mode_emits_no_annotations() {
        let m = lowered("in go: exec\n// note\non go { }\n");
        let l = crate::layout::layout(&m);
        assert!(l.annotations.is_empty());
    }

    #[test]
    fn blank_lines_leave_a_gap_and_clamp_at_two() {
        let gap_for = |later_line: u32| -> i32 {
            let a = make_node("G", make_range("f", 1, 0, 0, 1));
            let b = make_node("G", make_range("f", later_line, 0, 100, 101));
            let (hsx, _) = brick_half_size(&a);
            let a_h = hsx * 2;
            let (a_id, b_id) = (a.id, b.id);
            let m = module_with(vec![a, b], vec![]);
            let l = layout_code(&m, &opts(), false);
            (l.placements[&a_id].x - l.placements[&b_id].x) - a_h
        };
        // one blank line (line 2) between occupied lines 1 and 3.
        assert_eq!(gap_for(3), 1 * EMPTY_LINE_HEIGHT);
        // eight blank lines between occupied lines 1 and 10, clamped to 2.
        assert_eq!(gap_for(10), 2 * EMPTY_LINE_HEIGHT);
    }

    #[test]
    fn foreign_file_node_adopts_consumer_line() {
        let a = make_node("G", make_range("main", 3, 0, 50, 51));
        let b = make_node("G", make_range("other", 1, 0, 0, 1));
        let (a_id, b_id) = (a.id, b.id);
        let m = module_with(vec![a, b], vec![wire(b_id, a_id)]);
        let l = layout_code(&m, &opts(), false);
        assert_eq!(l.placements[&a_id].x, l.placements[&b_id].x);
        // Adopted onto its consumer's row, the producer reads first: left of
        // the node it feeds.
        assert!(l.placements[&b_id].y < l.placements[&a_id].y);
    }

    #[test]
    fn synthetic_default_range_node_adopts_transitively() {
        let a = make_node("G", make_range("main", 5, 0, 200, 201));
        let b = make_node("G", SourceRange::default());
        let c = make_node("G", SourceRange::default());
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let m = module_with(vec![a, b, c], vec![wire(c_id, b_id), wire(b_id, a_id)]);
        let l = layout_code(&m, &opts(), false);
        assert_eq!(l.placements[&b_id].x, l.placements[&a_id].x);
        assert_eq!(l.placements[&c_id].x, l.placements[&a_id].x);
    }

    #[test]
    fn consumerless_homeless_node_lands_on_overflow_row() {
        let a = make_node("G", make_range("main", 1, 0, 0, 1));
        let b = make_node("G", make_range("main", 2, 0, 10, 11));
        let c = make_node("G", SourceRange::default());
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);
        let m = module_with(vec![a, b, c], vec![]);
        let l = layout_code(&m, &opts(), false);
        let last_source_x = l.placements[&a_id].x.min(l.placements[&b_id].x);
        assert!(l.placements[&c_id].x < last_source_x);
    }

    #[test]
    fn long_line_soft_wraps_into_indented_continuation_row() {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for i in 0..5 {
            let off = i * 10;
            let n = make_node(
                "G",
                make_range("f", 1, 0, off, off + 1),
            );
            ids.push(n.id);
            nodes.push(n);
        }
        let d = make_node("G", make_range("f", 2, 0, 1000, 1001));
        let d_id = d.id;
        nodes.push(d);
        let m = module_with(nodes, vec![]);
        // Each node is its own group: 10 wide plus the column's tap
        // reserve, so three fit in 45 and the fourth wraps.
        let budgets = CodeBudgets {
            line_width: 3 * (10 + TAP_RESERVE),
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        let xs: Vec<i32> = ids.iter().map(|id| l.placements[id].x).collect();
        let ys: Vec<i32> = ids.iter().map(|id| l.placements[id].y).collect();

        // first three share a sub-row, last two share a lower sub-row.
        assert_eq!(xs[0], xs[1]);
        assert_eq!(xs[1], xs[2]);
        assert_eq!(xs[3], xs[4]);
        assert!(xs[3] < xs[0], "continuation sub-row must sit lower");

        // continuation sub-row starts at indent (0) + CONTINUATION_INDENT.
        assert_eq!(ys[3] - ys[0], CONTINUATION_INDENT);

        // the next source line shifts down by both sub-rows' heights.
        let (hsx, _) = brick_half_size(&m.nodes[&ids[0]]);
        let row_h = hsx * 2;
        assert_eq!(xs[0] - l.placements[&d_id].x, 2 * row_h);
    }

    #[test]
    fn every_spawnable_node_gets_a_placement() {
        let m = lowered(
            "var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }\nout y = x\nchip { var inner: int = 0 }\n",
        );
        let spawnable_count = m
            .nodes
            .values()
            .filter(|n| is_spawnable(n))
            .count();
        let l = layout_code(&m, &opts(), false);
        assert_eq!(l.placements.len(), spawnable_count);
        for n in m.nodes.values().filter(|n| is_spawnable(n)) {
            assert!(
                l.placements.contains_key(&n.id),
                "node {} missing a placement",
                n.id
            );
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let m = lowered("var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }\n");
        let a = layout_code(&m, &opts(), false);
        let b = layout_code(&m, &opts(), false);
        assert_eq!(a.placements, b.placements);
        // A run that placed the same cells but turned a different set of
        // bricks would still hand emit a different build.
        assert_eq!(a.rotations, b.rotations);
        assert!(!a.rotations.is_empty(), "fixture must rotate something");
    }

    #[test]
    fn empty_module_returns_empty_layout() {
        let l = layout_code(&Module::new("empty"), &opts(), false);
        assert!(l.placements.is_empty());
        assert_eq!(l.bounds_min, IntVec3::default());
        assert_eq!(l.bounds_max, IntVec3::default());
    }

    /// Four 10-high one-node lines (1..=4), band budget 25 → bands split
    /// 2/2; line/plane budgets stay default so nothing else wraps.
    fn band_split_module() -> (Module, Vec<NodeId>) {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for i in 0..4u32 {
            let off = (i as usize) * 10;
            let n = make_node("G", make_range("f", i + 1, 0, off, off + 1));
            ids.push(n.id);
            nodes.push(n);
        }
        (module_with(nodes, vec![]), ids)
    }

    /// Six 10-high one-node lines, band budget 25 → three 2-line bands,
    /// each 10 wide plus its column's tap reserve; `PAGE_BUDGET` fits two
    /// of them with a gutter between, so the third band starts page 1.
    /// Two bands wide, gutter included.
    const PAGE_BUDGET: i32 = 2 * (10 + TAP_RESERVE) + BAND_GUTTER;

    fn paginated_module() -> (Module, Vec<NodeId>) {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        for i in 0..6u32 {
            let off = (i as usize) * 10;
            let n = make_node("G", make_range("f", i + 1, 0, off, off + 1));
            ids.push(n.id);
            nodes.push(n);
        }
        (module_with(nodes, vec![]), ids)
    }

    /// A real lowered body that both paginates AND earns lanes.
    ///
    /// `paginated_module` is synthetic and carries NO WIRES, so every
    /// pagination test built on it runs against an empty bus — pagination is
    /// the one place lanes are allocated per page, off each page's own left
    /// edge, and nothing was checking that. This is the fixture that is.
    fn paginated_bus_module() -> (Module, CodeBudgets) {
        (
            lowered(BAND_SRC),
            CodeBudgets {
                band_height: 40,
                plane_width: 120,
                ..CodeBudgets::default()
            },
        )
    }

    #[test]
    fn band_wrap_moves_overflow_lines_to_a_second_column() {
        let (m, ids) = band_split_module();
        let budgets = CodeBudgets {
            band_height: 25,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        let p: Vec<_> = ids.iter().map(|id| l.placements[id]).collect();
        let (_, hsy) = brick_half_size(&m.nodes[&ids[0]]);
        let node_w = hsy * 2;

        let band1_max_y = p[0].y.max(p[1].y) + node_w;
        assert!(
            p[2].y >= band1_max_y + BAND_GUTTER,
            "3rd line ({}) must sit right of band 1's extent ({}) + gutter",
            p[2].y,
            band1_max_y
        );
        assert!(p[3].y >= band1_max_y + BAND_GUTTER);

        assert_eq!(p[2].x, p[0].x, "band 2's first line restarts at the top");
        assert_eq!(p[3].x, p[1].x);
        assert!(p[1].x < p[0].x);
        assert!(p.iter().all(|pl| pl.z == Z_PLANE), "single page only");
    }

    #[test]
    fn page_wrap_stacks_bands_in_z_with_page_step() {
        let (m, ids) = paginated_module();
        let budgets = CodeBudgets {
            band_height: 25,
            plane_width: PAGE_BUDGET,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        for id in &ids[..4] {
            assert_eq!(l.placements[id].z, Z_PLANE, "first two bands stay on page 0");
        }
        for id in &ids[4..] {
            assert_eq!(
                l.placements[id].z,
                Z_PLANE + PAGE_Z_STEP,
                "overflow band lands on page 1"
            );
        }
    }

    #[test]
    fn bounds_cover_all_pages() {
        let (m, _) = paginated_module();
        let budgets = CodeBudgets {
            band_height: 25,
            plane_width: PAGE_BUDGET,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        assert_eq!(l.bounds_min.z, Z_PLANE);
        assert_eq!(l.bounds_max.z, Z_PLANE + PAGE_Z_STEP);
        for p in l.placements.values() {
            assert!(p.x >= l.bounds_min.x && p.x <= l.bounds_max.x);
            assert!(p.y >= l.bounds_min.y && p.y <= l.bounds_max.y);
            assert!(p.z >= l.bounds_min.z && p.z <= l.bounds_max.z);
        }

        // The emitted microchip grid's PlaneExtent (centered on PlaneCenter
        // `(0, 0, 0)`, see `emit::build_world`) must also actually contain
        // every page — bounds alone don't guarantee that since the grid's
        // extent is a separate, derived quantity.
        let extent = crate::layout::wall::plane_extent(&l);
        for p in l.placements.values() {
            assert!(
                p.x >= -extent.x && p.x <= extent.x,
                "x {} outside plane extent {}",
                p.x,
                extent.x
            );
            assert!(
                p.y >= -extent.y && p.y <= extent.y,
                "y {} outside plane extent {}",
                p.y,
                extent.y
            );
            assert!(
                p.z >= -extent.z && p.z <= extent.z,
                "z {} outside plane extent {} (page z-stacking must fit the plane)",
                p.z,
                extent.z
            );
        }
    }

    /// A port's declared label (`PortLabel`), for asserting stack order.
    fn label_of(n: &Node) -> String {
        match n.properties.get(&*crate::intern::sym::PORT_LABEL) {
            Some(crate::ir::Literal::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    /// Two declared value params behind an exec in, two declared outs, and
    /// two synthesized boundary pins (the body reads outer globals). The
    /// params are fed from globals rather than literals so the fold pass
    /// can't collapse them out of the signature.
    const PORTS_SRC: &str = "var g1: int = 1\nvar g2: int = 2\nin go: exec\nchip C(t: exec, a: int, b: int) -> (x: int, y: int) {\n  out x = a + g1\n  out y = b + g2\n}\nlet c = C(go, g1, g2)\n";

    #[test]
    fn declared_ports_sit_on_the_plane_edges_in_signature_order() {
        let m = lowered(PORTS_SRC);
        let chip = m.chips.values().next().expect("chip module");
        let l = layout_code(chip, &LayoutOptions::default(), false);

        let ids_of = |k: NodeKind| -> Vec<NodeId> {
            let mut v: Vec<NodeId> = chip
                .nodes
                .values()
                .filter(|n| n.kind == k)
                .map(|n| n.id)
                .collect();
            v.sort_by_key(|id| Reverse(l.placements[id].x)); // top-down
            v
        };
        let inputs = ids_of(NodeKind::Input);
        let outputs = ids_of(NodeKind::Output);
        assert!(!inputs.is_empty() && !outputs.is_empty());

        // Every input is left of every non-port node; every output is right of them.
        let body_ys: Vec<i32> = chip
            .nodes
            .values()
            .filter(|n| !matches!(n.kind, NodeKind::Input | NodeKind::Output))
            .filter_map(|n| l.placements.get(&n.id).map(|p| p.y))
            .collect();
        let body_min = *body_ys.iter().min().unwrap();
        let body_max = *body_ys.iter().max().unwrap();
        for id in &inputs {
            assert!(
                l.placements[id].y < body_min,
                "input {id} not on the left edge"
            );
        }
        for id in &outputs {
            assert!(
                l.placements[id].y > body_max,
                "output {id} not on the right edge"
            );
        }

        // Stacks descend in signature order: declared ports in declaration
        // order first, then synthesized boundary pins by label. (Every
        // declared port shares the chip signature's source offset, so the
        // labels — not the offsets — are what pin down declaration order.)
        let labels = |ids: &[NodeId]| -> Vec<String> {
            ids.iter().map(|id| label_of(&chip.nodes[id])).collect()
        };
        assert_eq!(labels(&inputs), ["t", "a", "b", "g1", "g2"]);
        assert_eq!(labels(&outputs), ["x", "y"]);

        // Declared entries precede synthesized ones, and are themselves in
        // non-decreasing source order.
        let declared: Vec<usize> = inputs
            .iter()
            .filter(|id| has_range(&chip.nodes[id]))
            .map(|id| chip.nodes[id].source_range.start.offset)
            .collect();
        assert_eq!(declared.len(), 3, "t/a/b are the declared inputs");
        assert!(
            declared.windows(2).all(|w| w[0] <= w[1]),
            "declared inputs not in signature order: {declared:?}"
        );
        let synth_first_x = inputs
            .iter()
            .filter(|id| !has_range(&chip.nodes[id]))
            .map(|id| l.placements[id].x)
            .max()
            .unwrap();
        let declared_last_x = inputs
            .iter()
            .filter(|id| has_range(&chip.nodes[id]))
            .map(|id| l.placements[id].x)
            .min()
            .unwrap();
        assert!(
            synth_first_x < declared_last_x,
            "synthesized pins must stack below every declared port"
        );
    }

    /// Routing declared ports to the edges lets a chip end up with no body
    /// nodes at all, so the page list comes back empty — a state the row
    /// model could not previously reach, since declared ports used to
    /// occupy a source row themselves.
    #[test]
    fn a_ports_only_chip_still_places_and_stays_inside_the_plane() {
        let m = lowered(
            "var g: int = 1\nin go: exec\nchip C(t: exec, a: int) -> (x: int) {\n  out x = a\n}\nlet c = C(go, g)\n",
        );
        let chip = m.chips.values().next().expect("chip module");
        let ports: Vec<&Node> = chip.nodes.values().filter(|n| is_spawnable(n)).collect();
        assert!(
            ports.iter().all(|n| is_edge_pin(n)),
            "fixture sanity: this chip must be nothing but ports"
        );

        let l = layout_code(chip, &LayoutOptions::default(), false);
        assert_eq!(l.placements.len(), ports.len());

        // Inputs still land left of outputs, and the plane still contains
        // everything.
        let in_y = ports
            .iter()
            .filter(|n| n.kind == NodeKind::Input)
            .map(|n| l.placements[&n.id].y)
            .max()
            .unwrap();
        let out_y = ports
            .iter()
            .filter(|n| n.kind == NodeKind::Output)
            .map(|n| l.placements[&n.id].y)
            .min()
            .unwrap();
        assert!(in_y < out_y, "inputs must stay left of outputs");

        let e = crate::layout::wall::plane_extent(&l);
        for n in &ports {
            let p = l.placements[&n.id];
            let (hsx, hsy) = measured_half_size(n, &l);
            assert!(p.x >= -e.x && p.x + hsx * 2 <= e.x);
            assert!(p.y >= -e.y && p.y + hsy * 2 <= e.y);
        }
        assert_no_overlap(chip, &l);
    }

    #[test]
    fn port_stacks_start_at_the_page_top() {
        let m = lowered(
            "var g1: int = 1\nin go: exec\nchip C(t: exec, a: int) -> (x: int) {\n  out x = a + g1\n}\nlet c = C(go, g1)\n",
        );
        let chip = m.chips.values().next().expect("chip module");
        let l = layout_code(chip, &LayoutOptions::default(), false);
        let top = l.placements.values().map(|p| p.x).max().unwrap();
        let first_in = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Input && has_range(n))
            .min_by_key(|n| (n.source_range.start.offset, n.id))
            .unwrap();
        assert_eq!(
            l.placements[&first_in.id].x, top,
            "first declared input must sit at the page's top row"
        );

        // ...and the rest of the stack descends from it, one row per port.
        // Declared ports all share the chip signature's source line, so
        // literal placement would pile them onto a single row instead.
        let mut inputs: Vec<&Node> = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Input)
            .collect();
        assert!(inputs.len() >= 2, "fixture sanity: needs a multi-pin stack");
        inputs.sort_by_key(|n| Reverse(l.placements[&n.id].x));
        for w in inputs.windows(2) {
            let (hsx, _) = brick_half_size(w[1]);
            assert_eq!(
                l.placements[&w[1].id].x,
                l.placements[&w[0].id].x - hsx * 2,
                "input stack must descend one pin height per entry"
            );
        }

        // The output stack starts at the same top row.
        let first_out = chip
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Output)
            .unwrap();
        assert_eq!(l.placements[&first_out.id].x, top);
    }

    #[test]
    fn every_brick_fits_inside_the_plane_extent() {
        // A chip whose interior reads two outer globals: inbound pins only,
        // so the pin stack extends left with nothing balancing it on the right.
        let m = lowered(
            "var a: int = 1\nvar b: int = 2\nin go: exec\nchip C(t: exec) { on t { PrintToConsole(\"${a} ${b}\") } }\nlet c = C(go)\n",
        );
        let chip = m.chips.values().next().expect("chip module");
        let l = layout_code(chip, &LayoutOptions::default(), false);
        let e = crate::layout::wall::plane_extent(&l);
        for (id, p) in &l.placements {
            let (hsx, hsy) = measured_half_size(&chip.nodes[id], &l);
            assert!(
                p.x >= -e.x && p.x + hsx * 2 <= e.x,
                "node {id} x-range [{}, {}] escapes plane extent {}",
                p.x,
                p.x + hsx * 2,
                e.x
            );
            assert!(
                p.y >= -e.y && p.y + hsy * 2 <= e.y,
                "node {id} y-range [{}, {}] escapes plane extent {}",
                p.y,
                p.y + hsy * 2,
                e.y
            );
            assert!(
                p.z <= e.z,
                "node {id} z {} escapes plane extent {}",
                p.z,
                e.z
            );
        }
    }

    const ANON_CHIP_SRC: &str =
        "var g: int = 7\nin t: exec\nchip {\n  var h: int = 0\n  var k: int = 0\n  on t {\n    h = h + g\n    k = k + g\n  }\n}\nout o = h\nout p = k\n";

    fn is_boundary_pin(n: &Node) -> bool {
        n.note == Some("boundary_pin")
    }

    #[test]
    fn synthesized_pins_stack_on_edges() {
        // Anonymous chip whose interior reads global g (+ the exec input t) and
        // whose state (h, k) is read at root: the chip module gets 2 boundary
        // MicrochipInputs and 2 boundary MicrochipOutputs (verified against
        // dumped IR).
        let root = lowered(ANON_CHIP_SRC);
        let chip = root.chips.values().next().expect("one chip");
        let l = layout_code(chip, &opts(), false);

        let boundary_ins: Vec<NodeId> = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Input && is_boundary_pin(n))
            .map(|n| n.id)
            .collect();
        let boundary_outs: Vec<NodeId> = chip
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Output && is_boundary_pin(n))
            .map(|n| n.id)
            .collect();
        assert_eq!(boundary_ins.len(), 2, "fixture sanity");
        assert_eq!(boundary_outs.len(), 2, "fixture sanity");

        let main_ys: Vec<i32> = chip
            .nodes
            .values()
            .filter(|n| is_spawnable(n) && !is_boundary_pin(n))
            .map(|n| l.placements[&n.id].y)
            .collect();
        let main_min = *main_ys.iter().min().unwrap();
        let main_max = *main_ys.iter().max().unwrap();

        for id in &boundary_ins {
            assert!(
                l.placements[id].y < main_min,
                "input pin must sit left of every main node"
            );
        }
        for id in &boundary_outs {
            assert!(
                l.placements[id].y > main_max,
                "output pin must sit right of every main node"
            );
        }

        // The global y extremes belong to the edge stacks.
        let min_id = *l.placements.iter().min_by_key(|(_, p)| p.y).unwrap().0;
        let max_id = *l.placements.iter().max_by_key(|(_, p)| p.y).unwrap().0;
        assert!(boundary_ins.contains(&min_id));
        assert!(boundary_outs.contains(&max_id));

        // Two pins anchored to the same consumer must stack, not collide.
        let in_xs: Vec<i32> = boundary_ins.iter().map(|id| l.placements[id].x).collect();
        assert_ne!(in_xs[0], in_xs[1], "same-anchor pins must bump apart");

        // Coverage and determinism now include the edge-pin path.
        let spawnable_count = chip.nodes.values().filter(|n| is_spawnable(n)).count();
        assert_eq!(l.placements.len(), spawnable_count);
        let l2 = layout_code(chip, &opts(), false);
        assert_eq!(l.placements, l2.placements);

        // Declared in/out pins (real ranges) join the edge stacks too,
        // ahead of the synthesized pins.
        let named = lowered(
            "var g: int = 7\nin t: exec\nchip C(u: exec) -> (r: int) {\n  var h: int = 0\n  on u { h = h + g }\n  out r = h\n}\nlet c = C(t)\nout o = c\n",
        );
        let cm = named.chips.values().next().expect("one chip");
        let l3 = layout_code(cm, &opts(), false);

        let declared_in = cm
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Input && has_range(n))
            .expect("declared u pin");
        let declared_out = cm
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Output && has_range(n))
            .expect("declared r pin");
        let boundary_in = cm
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Input && is_boundary_pin(n))
            .expect("boundary g pin");

        // Each stack starts at the page's top row, so the sole declared
        // input and the sole declared output both hold it.
        let top_x = cm
            .nodes
            .values()
            .filter(|n| is_spawnable(n) && !is_boundary_pin(n))
            .map(|n| l3.placements[&n.id].x)
            .max()
            .unwrap();
        assert_eq!(l3.placements[&declared_in.id].x, top_x);
        assert_eq!(l3.placements[&declared_out.id].x, top_x);

        // The boundary input shares the left edge with the declared input
        // (same column) and stacks below it, rather than sitting further
        // left on a row of its own.
        assert_eq!(
            l3.placements[&boundary_in.id].y,
            l3.placements[&declared_in.id].y
        );
        assert!(l3.placements[&boundary_in.id].x < l3.placements[&declared_in.id].x);

        // The declared output is on the right edge — right of every body
        // node, including the rightmost one.
        let var_get = cm
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_GET)
            .unwrap();
        assert!(l3.placements[&declared_out.id].y > l3.placements[&var_get.id].y);
    }

    #[test]
    fn a_pin_stack_orders_ext_labels_by_number() {
        // Plain string order files `ext10` between `ext1` and `ext2`.
        let mut labels = ["ext10", "ext2", "ext1", "score", "ext"];
        labels.sort_by_key(|l| label_sort_key(l));
        assert_eq!(labels, ["ext", "ext1", "ext2", "ext10", "score"]);
    }

    /// The footprint a node actually occupies once placed — the oracle every
    /// overlap and containment sweep below measures with.
    ///
    /// The swap is spelled out rather than delegated to `cell_half_size`: an
    /// oracle that reuses the function under test stays green if that
    /// function stops swapping, which is exactly the regression this guards.
    ///
    /// Only the QUARTER turns swap. `Deg180` and `Deg270` are listed
    /// explicitly rather than swept into a catch-all: a `_` arm here would
    /// silently hand a future variant whichever footprint it happened to sit
    /// next to, and which side of this split a facing lands on is exactly the
    /// mistake these sweeps exist to catch.
    fn measured_half_size(node: &Node, lr: &LayoutResult) -> (i32, i32) {
        let (bx, by) = brick_half_size(node);
        match rotation_of(&lr.rotations, &node.id) {
            NodeRotation::Deg0 | NodeRotation::Deg180 => (bx, by),
            NodeRotation::Deg90 | NodeRotation::Deg270 => (by, bx),
        }
    }

    /// The half-extent of a bus node's brick, re-spelled from the brick
    /// itself: `emit_bus` places `B_1x1_Reroute_Node` — a 2×2×2 — centred one
    /// unit off the layout's min corner on each axis, and the footprint is
    /// square so rotation does not swap it.
    ///
    /// Deliberately not `REROUTER_HALF`: an oracle measuring with the
    /// constant under test goes green the moment that constant stops
    /// describing the brick, which is exactly the disagreement these sweeps
    /// exist to catch.
    const MEASURED_BUS_HALF: i32 = 1;

    /// The overlap sweep, sized the way the bricks actually land: a `Deg90`
    /// node's half-sizes are swapped. `check_overlaps` cannot do this —
    /// `brdb::Brick::local_bounds()` ignores rotation — so this is the gate
    /// that catches a layout/emit footprint disagreement.
    ///
    /// The gutter bus carries a brick per node too, and a lane's taps stand
    /// inside the body's rows, so they are swept with everything else.
    ///
    /// So do the comment annotations. Their carrier is an invisible 1×1
    /// `PB_DefaultBrick`, which the game drops on overlap exactly like a
    /// visible one — and being invisible is precisely why nobody would notice
    /// it had landed inside a gate.
    fn assert_no_overlap(module: &Module, lr: &LayoutResult) {
        let mut boxes: Vec<(String, i32, i32, i32, i32, i32)> = lr
            .placements
            .iter()
            .filter_map(|(id, p)| {
                let n = module.nodes.get(id)?;
                let (hx, hy) = measured_half_size(n, lr);
                Some((format!("{id:?}"), p.x, p.x + hx * 2, p.y, p.y + hy * 2, p.z))
            })
            .collect();
        boxes.extend(lr.bus.nodes.iter().enumerate().map(|(i, n)| {
            (
                format!("bus node {i}"),
                n.x,
                n.x + MEASURED_BUS_HALF * 2,
                n.y,
                n.y + MEASURED_BUS_HALF * 2,
                n.z,
            )
        }));
        boxes.extend(lr.annotations.iter().enumerate().map(|(i, a)| {
            (
                format!("annotation {i} ({:?})", a.text),
                a.x,
                a.x + ANNOTATION_SIZE,
                a.y,
                a.y + ANNOTATION_SIZE,
                a.z,
            )
        }));
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let disjoint = a.5 != b.5 || a.2 <= b.1 || b.2 <= a.1 || a.4 <= b.3 || b.4 <= a.3;
                assert!(disjoint, "{} overlaps {} (rotation-aware)", a.0, b.0);
            }
        }
    }

    /// The test-side spelling of the in-line consumer relation: does any
    /// node on `n`'s own line read the value `n` produces? Re-derived from
    /// the module's wires rather than borrowed from `line_groups`, so it
    /// cannot go green alongside a grouping bug.
    fn value_is_read_on_its_line(m: &Module, n: &Node) -> bool {
        let line = n.source_range.start.line;
        m.wires.iter().any(|w| {
            w.source.node_id == n.id
                && w.source.port != WirePort::Layout
                && w.target.port != WirePort::Layout
                && !targets_exec(m, &w.target)
                && m.nodes
                    .get(&w.target.node_id)
                    .is_some_and(|t| t.source_range.start.line == line)
        })
    }

    /// The sub-row a placed node sits in, named by that row's top edge —
    /// the one value every node in a row shares whatever its own height.
    fn row_top(node: &Node, l: &LayoutResult) -> i32 {
        l.placements[&node.id].x + measured_half_size(node, l).0 * 2
    }

    /// The whole spine rule, both halves, over every exec gate the fixture
    /// lowers: a gate turns exactly when it takes an exec input and nothing
    /// on its line reads its value back.
    #[test]
    fn exec_gates_turn_exactly_when_they_are_their_lines_sink() {
        let src =
            "var a: int = 1\nin go: exec\non go {\n  a = a + 1\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let mut sinks = 0usize;
        let mut reads = 0usize;
        for n in m
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Gate && l.placements.contains_key(&n.id))
        {
            let want = if takes_exec_input(n) && !value_is_read_on_its_line(&m, n) {
                sinks += 1;
                NodeRotation::Deg90
            } else {
                if takes_exec_input(n) {
                    reads += 1;
                }
                NodeRotation::Deg0
            };
            assert_eq!(
                rotation_of(&l.rotations, &n.id),
                want,
                "{} (exec in = {}, value read on its line = {})",
                n.gate_class,
                takes_exec_input(n),
                value_is_read_on_its_line(&m, n)
            );
        }
        assert!(sinks > 0, "fixture must lower a statement sink");
        assert!(reads > 0, "fixture must lower a value-producing exec read");
    }

    /// A read-heavy statement must not grow a sub-row per read.
    ///
    /// The line is the reads' own stacked column plus the statement row the
    /// sink dropped to — three. Pinning every exec gate to column 0 would put
    /// both reads in the spine column under the `PrintToConsole` instead of
    /// beside it, and the same drop would then land the sink below all three:
    /// four rows. That is the regression this counts.
    #[test]
    fn reads_do_not_add_a_sub_row_to_their_statement() {
        let src = "var a: int = 1\nvar b: int = 2\nin go: exec\non go {\n  PrintToConsole(\"${a} ${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let line = m
            .nodes
            .values()
            .find(|n| n.gate_class.ends_with("PrintToConsole"))
            .expect("PrintToConsole node")
            .source_range
            .start
            .line;
        let on_line: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| n.source_range.start.line == line && l.placements.contains_key(&n.id))
            .collect();
        let reads = on_line
            .iter()
            .filter(|n| n.gate_class.ends_with("Exec_Var_Get"))
            .count();
        assert_eq!(reads, 2, "fixture must lower two in-line reads");
        let rows: HashSet<i32> = on_line.iter().map(|n| row_top(n, &l)).collect();
        assert_eq!(
            rows.len(),
            3,
            "the two reads share one value column, so the line is that column \
             plus the sink's own row — not a row per read"
        );
    }

    /// The narrow half of the rule, pinned on its own. An `Exec_Var_Get`
    /// inside an interpolation takes an exec input just like the
    /// `PrintToConsole` it feeds, but it belongs to the value flow: pinning
    /// it to column 0 would turn it AND push it onto a second sub-row,
    /// making every read-heavy line taller for nothing.
    #[test]
    fn a_value_producing_exec_read_stays_in_the_value_columns() {
        let src = "var a: int = 1\nin go: exec\non go {\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let by_class = |c: &str| -> &Node {
            m.nodes
                .values()
                .find(|n| n.gate_class.ends_with(c))
                .unwrap_or_else(|| panic!("a {c} node"))
        };
        let read = by_class("Exec_Var_Get");
        let print = by_class("PrintToConsole");
        assert!(
            takes_exec_input(read),
            "the read must take an exec input, or this test proves nothing"
        );
        assert_eq!(
            rotation_of(&l.rotations, &read.id),
            NodeRotation::Deg0,
            "an expression-side exec read must stay horizontal"
        );
        assert!(
            l.placements[&read.id].y > l.placements[&print.id].y,
            "the read must sit RIGHT of the sink consuming it, not in column 0"
        );

        // The whole expression still resolves on ONE row — the read beside
        // the `FormatText` it feeds, not under it — and the statement takes
        // the row below. Pinning the read to column 0 would stack it under
        // the sink and make that two expression rows instead of one.
        let rows: HashSet<i32> = m
            .nodes
            .values()
            .filter(|n| {
                n.source_range.start.line == print.source_range.start.line
                    && l.placements.contains_key(&n.id)
            })
            .map(|n| row_top(n, &l))
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "one row of expression plus the statement row under it"
        );
        assert_eq!(
            row_top(read, &l),
            row_top(by_class("FormatText"), &l),
            "the read shares its row with the pure gate it feeds"
        );
    }

    /// The spine, not its operands, is what turns. `PrintToConsole` is the
    /// statement's exec gate and `FormatText` the pure gate feeding it; a
    /// depth-ordered spine puts them the other way round, leaving the pure
    /// gate on the left facing down and the exec gate on the right flat.
    #[test]
    fn the_exec_gate_turns_and_the_pure_gate_it_reads_does_not() {
        let src =
            "var a: int = 1\nin go: exec\non go {\n  a = a + 1\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let by_class = |c: &str| -> &Node {
            m.nodes
                .values()
                .find(|n| n.gate_class.ends_with(c))
                .unwrap_or_else(|| panic!("a {c} node"))
        };
        let print = by_class("PrintToConsole");
        let fmt = by_class("String_FormatText");
        assert_eq!(
            rotation_of(&l.rotations, &print.id),
            NodeRotation::Deg90,
            "the statement's exec gate must face down the spine"
        );
        assert_eq!(
            rotation_of(&l.rotations, &fmt.id),
            NodeRotation::Deg0,
            "an expression-side gate must stay horizontal"
        );
        assert!(
            l.placements[&print.id].y < l.placements[&fmt.id].y,
            "the exec gate must sit LEFT of the expression it reads"
        );
    }

    #[test]
    fn non_exec_gates_are_never_rotated() {
        let src = "var a: int = 1\nin go: exec\non go {\n  let m = (a + 1) * (a + 2)\n  PrintToConsole(\"${m}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        for (id, rot) in &l.rotations {
            let n = &m.nodes[id];
            assert!(takes_exec_input(n), "non-exec gate {id} rotated ({rot:?})");
        }
    }

    /// Only gate bricks may carry a rotation. The microchip shell is
    /// emitted by a separate path that hardcodes its 1×1 offsets and never
    /// reads `rotations`, so a rotation recorded for a chip node would be a
    /// footprint the emitter does not honour — the exact layout/emit
    /// disagreement this mechanism exists to prevent.
    ///
    /// A chip instance is exec-triggered in the source, but its IR node
    /// exposes no `Type::Exec` input port, so the rule's first clause
    /// already excludes it; restricting to `NodeKind::Gate` is a guard
    /// against that changing, and this test is what would catch it.
    #[test]
    fn only_gate_nodes_are_ever_rotated() {
        let src = "var s: int = 0\nin bump: exec\nchip Scorer(go: exec, amount: int) -> (total: int) {\n  on go { s = s + amount }\n  out total = s\n}\nlet scored = Scorer(bump, 5)\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            m.nodes
                .values()
                .any(|n| n.kind == NodeKind::Chip && l.placements.contains_key(&n.id)),
            "fixture must place a chip instance"
        );
        for id in l.rotations.keys() {
            assert_eq!(
                m.nodes[id].kind,
                NodeKind::Gate,
                "only gate bricks may be rotated; {id} is a {:?}",
                m.nodes[id].kind
            );
        }
    }

    #[test]
    fn dag_mode_sets_no_rotations() {
        let m = lowered("var a: int = 1\nin go: exec\non go { a = a + 1 }\n");
        let l = crate::layout::layout(&m);
        assert!(l.rotations.is_empty());
    }

    #[test]
    fn rotated_gates_do_not_overlap_their_neighbours() {
        // The overlap gate, run over a fixture that actually rotates something.
        let src = "var a: int = 1\nin go: exec\non go {\n  a = a + 1\n  a = a + 2\n  PrintToConsole(\"${a}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            l.rotations.values().any(|r| *r == NodeRotation::Deg90),
            "fixture must rotate at least one gate"
        );
        assert_no_overlap(&m, &l);
    }

    /// The fixture that makes the footprint swap observable: `DisplayText`
    /// is a statement sink — nothing on its line reads it back — so it
    /// lands in column 0 and rotates, and it is 8×5, so the swap moves
    /// real geometry. Its line also carries a `Var_Get` feeding a
    /// `FormatText`, both of which stay in the value columns.
    pub(super) const WIDE_EXEC_SRC: &str = "var a: int = 1\non ControllerJoined() -> (c) {\n  a = a + 1\n  c.DisplayText(\"hi ${a}\")\n  a = a + 2\n}\n";

    /// A square gate's swap is a no-op, so a fixture of 5×5 gates cannot
    /// tell a correct reservation from a missing one. `DisplayText` is 8×5
    /// on the exec spine: rotating it turns a 16×10 footprint into a 10×16
    /// one, and reserving the unswapped cell puts it through its
    /// neighbour.
    #[test]
    fn a_rotated_wide_exec_gate_reserves_its_swapped_cell() {
        let src = WIDE_EXEC_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let wide: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| {
                l.placements.contains_key(&n.id) && {
                    let (hx, hy) = brick_half_size(n);
                    hx != hy
                }
            })
            .collect();
        assert!(
            !wide.is_empty(),
            "fixture must lower a non-square gate, got {:?}",
            m.nodes.values().map(|n| n.gate_class).collect::<Vec<_>>()
        );
        let rotated_wide: Vec<&&Node> = wide
            .iter()
            .filter(|n| rotation_of(&l.rotations, &n.id) == NodeRotation::Deg90)
            .collect();
        assert!(
            !rotated_wide.is_empty(),
            "fixture must rotate a non-square exec gate, got {:?}",
            wide.iter()
                .map(|n| (n.gate_class, brick_half_size(n)))
                .collect::<Vec<_>>()
        );

        // The reserved cell is the swapped one.
        for n in &rotated_wide {
            let (hx, hy) = brick_half_size(n);
            assert_eq!(
                cell_half_size(n, NodeRotation::Deg90),
                (hy, hx),
                "rotated {} must reserve its swapped footprint",
                n.gate_class
            );
        }
        assert_no_overlap(&m, &l);
    }

    #[test]
    fn no_two_bricks_overlap_in_any_mode() {
        // Soft-wrapping line.
        let mut nodes = Vec::new();
        for i in 0..5 {
            let off = i * 10;
            nodes.push(make_node("G", make_range("f", 1, 0, off, off + 1)));
        }
        nodes.push(make_node("G", make_range("f", 2, 0, 1000, 1001)));
        let wrap_m = module_with(nodes, vec![]);
        let budgets = CodeBudgets {
            line_width: 30,
            ..CodeBudgets::default()
        };
        assert_no_overlap(
            &wrap_m,
            &layout_code_with_budgets(&wrap_m, &opts(), false, &budgets),
        );

        // Paginated synthetic module.
        let (page_m, _) = paginated_module();
        let budgets = CodeBudgets {
            band_height: 25,
            plane_width: PAGE_BUDGET,
            ..CodeBudgets::default()
        };
        assert_no_overlap(
            &page_m,
            &layout_code_with_budgets(&page_m, &opts(), false, &budgets),
        );

        // Paginated body with a real bus on every page.
        let (bus_m, bus_budgets) = paginated_bus_module();
        assert_no_overlap(
            &bus_m,
            &layout_code_with_budgets(&bus_m, &opts(), false, &bus_budgets),
        );

        // Full pipeline (boundary pins included): the whole chip tree, each
        // level's own band included.
        let root = lowered(ANON_CHIP_SRC);
        assert_layout_tree_is_sound(&root, &layout_code(&root, &code_opts(), true));
    }

    /// The band's bricks are swept with the body's. A lane's tap stands on
    /// the row it serves, so a tap nudged off that row would land inside a
    /// gate — and the game drops overlapping bricks silently.
    #[test]
    fn a_full_band_never_overlaps_the_body() {
        let src = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${b}\")\n  PrintToConsole(\"${a}\")\n  b = a + b\n  log.push(\"y${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            l.bus.nodes.len() > 8,
            "fixture must build a band worth sweeping, got {} nodes",
            l.bus.nodes.len()
        );
        assert_no_overlap(&m, &l);
    }

    /// A lane's vertical run passes only through down-pointing rerouters. A
    /// right-pointing tap is a BRANCH hanging off one of them — it carries
    /// the value out to a row and stops there, and must never hand the lane
    /// on to the next stop.
    ///
    /// Stated as two halves so neither can be satisfied by accident: no
    /// `Deg0` node may drive a `Deg90` node, and every `Deg90` node is driven
    /// either by another `Deg90` node or from OUTSIDE the chain, which is
    /// where a lane's head takes its value. Outside the chain has two
    /// spellings — the producer's own port, and the source-side rerouter
    /// standing beside that producer, which is itself driven by that port and
    /// so is an entry into the lane rather than a link of it. Fanning a lane
    /// node out to both the next link and its own tap is the intended shape
    /// and stays legal.
    #[test]
    fn taps_branch_off_the_lane_and_never_carry_it() {
        let src = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${b}\")\n  PrintToConsole(\"${a}\")\n  b = a + b\n  log.push(\"y${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(
            l.bus.nodes.len() > 8,
            "fixture must build a band worth walking, got {} nodes",
            l.bus.nodes.len()
        );
        let rot = |i: usize| l.bus.nodes[i].rotation;

        for w in &l.bus.wires {
            if let (BusEnd::Bus(a), BusEnd::Bus(b)) = (w.source, w.target) {
                assert!(
                    !(rot(a) == NodeRotation::Deg0 && rot(b) == NodeRotation::Deg90),
                    "tap {a} carries the lane on to {b}; a tap is a branch, \
                     only down-pointing rerouters chain"
                );
            }
        }

        let mut lane_links = 0usize;
        let mut heads = 0usize;
        for (i, n) in l.bus.nodes.iter().enumerate() {
            if n.rotation != NodeRotation::Deg90 {
                continue;
            }
            lane_links += 1;
            let inbound = l
                .bus
                .wires
                .iter()
                .find(|w| w.target == BusEnd::Bus(i))
                .unwrap_or_else(|| panic!("lane node {i} is driven by nothing"));
            match inbound.source {
                // The head, taking the value from its producer's own port.
                BusEnd::Node(_) => heads += 1,
                // The head again, through the rerouter standing beside that
                // producer — which is outside the chain and must itself be fed
                // by the port, or the lane would be entered from the bus.
                BusEnd::Bus(p) if l.bus.nodes[p].role == BusRole::Source => {
                    assert!(
                        l.bus.wires.iter().any(|w| w.target == BusEnd::Bus(p)
                            && matches!(w.source, BusEnd::Node(_))),
                        "the source-side rerouter {p} heading lane node {i} is not \
                         driven by a real port"
                    );
                    heads += 1;
                }
                BusEnd::Bus(p) => assert_eq!(
                    rot(p),
                    NodeRotation::Deg90,
                    "lane node {i} is driven by {p}, which is not a lane node"
                ),
            }
        }
        assert!(
            lane_links > 2,
            "fixture must run real lanes, got {lane_links} chain-carrying nodes"
        );
        assert!(
            heads > 0,
            "fixture must head a lane, or the entry-point half proves nothing"
        );
    }

    /// Lanes are allocated per PAGE, off that page's own left edge, and a
    /// value read on two pages heads a lane on each. Every other bus test runs
    /// inside the default budgets, which never paginate, so this is the only
    /// place that shape is measured.
    #[test]
    fn a_paginated_body_builds_a_lane_on_every_page() {
        let (m, budgets) = paginated_bus_module();
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);

        let body_pages: BTreeSet<i32> = l.placements.values().map(|p| p.z).collect();
        assert!(
            body_pages.len() > 1,
            "fixture must paginate, got {} page(s)",
            body_pages.len()
        );
        // Not every page: one whose values never travel `MIN_LANE_TRAVEL`
        // earns no lane at all. What must hold is that a band only ever sits
        // on a page that has a body, and that the pages which DO band do it
        // off their own edges — checked below.
        let bus_pages: BTreeSet<i32> = l.bus.nodes.iter().map(|n| n.z).collect();
        assert!(!bus_pages.is_empty(), "the fixture must still band somewhere");
        assert!(
            bus_pages.is_subset(&body_pages),
            "a band may only sit on a page that has a body"
        );

        // A page's band hangs off that page's OWN left edge. Edge pins are held
        // out of the body edge — they stack further left still, clearing the
        // band `plan.band_widths` reserved for them — and so are the gate-side
        // taps, which stand one `TAP_RESERVE` inside the body's own columns.
        let mut lane_counts: Vec<usize> = Vec::new();
        for &z in &body_pages {
            let body_min_y = l
                .placements
                .iter()
                .filter(|(id, p)| p.z == z && !is_edge_pin(&m.nodes[*id]))
                .map(|(_, p)| p.y)
                .min()
                .expect("a page with body nodes");
            let gutter: BTreeSet<i32> = l
                .bus
                .nodes
                .iter()
                .filter(|n| n.z == z && n.y <= body_min_y - BUS_BAND_GUTTER)
                .map(|n| n.y)
                .collect();
            if gutter.is_empty() {
                // A page whose rows are too short for any value to travel
                // `MIN_LANE_TRAVEL` earns no band, which is the zero-travel
                // rule working. The per-page geometry claims below apply to
                // the pages that DO band.
                continue;
            }
            let inner = *gutter.iter().next_back().expect("a non-empty gutter");
            assert_eq!(
                inner,
                body_min_y - BUS_BAND_GUTTER - LANE_PITCH,
                "the innermost lane on the page at z {z} must stand one pitch off \
                 THIS page's left edge, not another page's"
            );
            let outer = *gutter.iter().next().expect("a non-empty gutter");
            assert_eq!(
                outer,
                inner - (gutter.len() as i32 - 1) * LANE_PITCH,
                "a page's lanes stand flush against each other: {gutter:?}"
            );
            lane_counts.push(gutter.len());
        }
        assert!(
            lane_counts.iter().collect::<HashSet<_>>().len() > 1,
            "lanes are allocated per page, so the pages must not all claim the \
             same count by coincidence: {lane_counts:?}"
        );

        assert_no_overlap(&m, &l);
        assert_no_fan_in(&l);
        let proven = assert_suppressed_consumers_stay_reachable(&m, &l);
        assert!(proven > 0, "a paginated bus must still replace wires");
    }

    #[test]
    fn a_multi_row_variable_gets_a_lane() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "a two-row read must build a lane");
        // Every consumer's original wire is suppressed.
        let var_id = m
            .nodes
            .values()
            .find(|n| n.gate_class == crate::ir::gate_class::PSEUDO_VAR)
            .unwrap()
            .id;
        for w in m.wires.iter().filter(|w| w.source.node_id == var_id) {
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "wire into {:?} must be replaced by the bus",
                w.target.node_id
            );
        }
    }

    /// The bus source of a chain, and every bus node the chain owns, split
    /// into its lane rerouters and its gate-side taps.
    ///
    /// A node's kind is read off the WIRES and off `BusRole`, never off its
    /// `y` — that is the coordinate the geometry tests below assert on, so
    /// classifying by it would make them self-confirming. The shape is
    /// unambiguous: a node driven from outside the chain is a lane head,
    /// whether that is a `BusEnd::Node` port or the source-side rerouter
    /// standing beside a body producer; a node driven by a LANE node standing
    /// in a DIFFERENT column (`x`, the row axis) is the next link down the
    /// lane, since a new row means a new row column; and a node driven from
    /// within the same column is a tap, because a row's taps are placed at
    /// their lane rerouter's own `x`.
    struct Chain {
        source: PortRef,
        lanes: Vec<usize>,
        taps: Vec<usize>,
    }

    /// Follow the one hop a source-side rerouter adds, so a walk starting at a
    /// real port lands on the LANE brick whichever kind of producer it left.
    ///
    /// An edge pin drives its head directly; a body gate drives it through the
    /// rerouter standing beside that gate. A test matching only the direct
    /// shape is how a claim about heads quietly stops looking at any.
    fn through_source_side(l: &LayoutResult, first: BusNodeId) -> BusNodeId {
        if l.bus.nodes[first].role != BusRole::Source {
            return first;
        }
        l.bus
            .wires
            .iter()
            .find(|w| w.source == BusEnd::Bus(first))
            .and_then(|w| match w.target {
                BusEnd::Bus(i) => Some(i),
                BusEnd::Node(_) => None,
            })
            .expect("a source-side rerouter drives its lane's head")
    }

    fn bus_chains(l: &LayoutResult) -> Vec<Chain> {
        // Fan-in is illegal, so each bus node has exactly one inbound wire.
        let mut inbound: HashMap<usize, BusEnd> = HashMap::default();
        for w in &l.bus.wires {
            if let BusEnd::Bus(i) = w.target {
                assert!(
                    inbound.insert(i, w.source).is_none(),
                    "bus node {i} is driven twice"
                );
            }
        }
        assert_eq!(
            inbound.len(),
            l.bus.nodes.len(),
            "every bus node must be driven"
        );

        // A node driven from OUTSIDE the chain is that chain's head: either
        // straight off a real port, or off the source-side rerouter standing
        // beside a body producer — which shares the head's row, so without the
        // role test it would read as a tap of its own head's column.
        let is_lane = |i: usize| match inbound[&i] {
            BusEnd::Node(_) => true,
            BusEnd::Bus(p) => {
                l.bus.nodes[p].role != BusRole::Gutter || l.bus.nodes[p].x != l.bus.nodes[i].x
            }
        };

        // Walk to the chain's head. A bus node is always pushed before the
        // wire that drives it, so the source index is strictly smaller and
        // the walk terminates.
        let head_of = |mut i: usize| -> (usize, PortRef) {
            for _ in 0..=l.bus.nodes.len() {
                match inbound[&i] {
                    BusEnd::Node(p) => return (i, p),
                    BusEnd::Bus(p) => {
                        assert!(p < i, "bus wire {p} -> {i} runs backwards");
                        i = p;
                    }
                }
            }
            panic!("bus chain from {i} has no head");
        };

        // Chains describe the GUTTER structure — a column of links with taps
        // branching off it. A mini-bus corner is a one-brick turn inside the
        // body with no chain to walk and no column to share, and a source-side
        // rerouter stands beside a producer in the body rather than in the
        // band, so every claim built on `Chain` would be false of either by
        // construction; their geometry is pinned by
        // `the_mini_bus_drops_an_expression_value_into_its_statement` and
        // `a_lane_is_fed_from_a_rerouter_beside_its_producer` instead. The
        // "every node is driven" check above still covers all three.
        //
        // `head_of` walks THROUGH a source-side rerouter to the port behind
        // it, so `Chain.source` is the value's real producer either way, and
        // the head index it keys on is stable per lane.
        let mut by_head: Vec<Chain> = Vec::new();
        let mut index: HashMap<usize, usize> = HashMap::default();
        for i in 0..l.bus.nodes.len() {
            if l.bus.nodes[i].role != BusRole::Gutter {
                continue;
            }
            let (head, source) = head_of(i);
            let ci = *index.entry(head).or_insert_with(|| {
                by_head.push(Chain {
                    source,
                    lanes: Vec::new(),
                    taps: Vec::new(),
                });
                by_head.len() - 1
            });
            if is_lane(i) {
                by_head[ci].lanes.push(i);
            } else {
                by_head[ci].taps.push(i);
            }
        }
        by_head
    }

    /// A lane is a COLUMN: every lane rerouter of one chain shares one `y`,
    /// however many rows the chain reaches. And every gate-side tap sits
    /// exactly one gap left of the gate it drives — not merely beside some
    /// gate somewhere on the plane.
    #[test]
    fn lane_rerouters_share_a_column() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "the fixture must build a lane");

        let chains = bus_chains(&l);
        assert!(
            chains.iter().any(|c| c.lanes.len() >= 2),
            "fixture must run one lane down at least two rows, or the \
             shared-column claim is vacuous; got {:?}",
            chains.iter().map(|c| c.lanes.len()).collect::<Vec<_>>()
        );

        for c in &chains {
            let (&first, rest) = c.lanes.split_first().expect("a chain has a head");
            let y = l.bus.nodes[first].y;
            for &i in rest {
                assert_eq!(
                    l.bus.nodes[i].y, y,
                    "lane rerouter {i} of the chain from {:?} left its column \
                     (y={} vs the head's {y})",
                    c.source, l.bus.nodes[i].y
                );
            }

            // Each tap sits one gap left of its OWN consumer.
            for &i in &c.taps {
                let driven: Vec<PortRef> = l
                    .bus
                    .wires
                    .iter()
                    .filter(|w| w.source == BusEnd::Bus(i))
                    .filter_map(|w| match w.target {
                        BusEnd::Node(p) => Some(p),
                        BusEnd::Bus(_) => None,
                    })
                    .collect();
                assert!(!driven.is_empty(), "tap {i} drives no gate");
                for p in driven {
                    assert_eq!(
                        l.bus.nodes[i].y,
                        l.placements[&p.node_id].y - TAP_GAP - 2 * REROUTER_HALF,
                        "tap {i} must sit one gap left of the gate it feeds"
                    );
                }
            }
        }
    }

    /// Lane 0 is the leftmost column, and `allocate_lanes` gives lane 0 to
    /// the widest-span value — so the value read across two rows holds a
    /// column further out than the one-row variable feeding it.
    #[test]
    fn the_widest_span_value_takes_the_leftmost_lane() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let chains = bus_chains(&l);

        // Rank the lanes by how far down the band each one actually reaches,
        // and by the column it stands in. Keying on gate CLASS instead only
        // works while a body happens to carry exactly one lane of each kind,
        // which is not a property of the rule being tested — it is a property
        // of a fixture small enough that it no longer earns a bus at all.
        let mut by_span: Vec<(usize, i32)> = chains
            .iter()
            .filter(|c| !c.lanes.is_empty())
            .map(|c| (c.lanes.len(), l.bus.nodes[c.lanes[0]].y))
            .collect();
        by_span.sort_by_key(|&(span, _)| Reverse(span));
        assert!(
            by_span.len() >= 2,
            "fixture must run at least two lanes, got {}",
            by_span.len()
        );
        let widest = by_span[0].0;
        assert!(
            widest > by_span.last().expect("a lane").0,
            "fixture must run lanes of DIFFERENT spans, or the ranking claim \
             is vacuous: {by_span:?}"
        );
        let widest_y = by_span[0].1;
        for &(span, y) in &by_span {
            assert!(
                span == widest || widest_y < y,
                "the widest-span lane must hold the outermost column: it sits \
                 at y={widest_y}, and a {span}-stop lane sits further out at y={y}"
            );
        }

        // ...and the whole band still sits left of the code body.
        let body_min_y = m
            .nodes
            .values()
            .filter(|n| is_spawnable(n) && !is_edge_pin(n))
            .map(|n| l.placements[&n.id].y)
            .min()
            .expect("a placed body node");
        for c in &chains {
            for &i in &c.lanes {
                assert!(
                    l.bus.nodes[i].y + 2 * REROUTER_HALF <= body_min_y - BUS_BAND_GUTTER,
                    "lane rerouter {i} at y={} is not clear of the body edge {body_min_y}",
                    l.bus.nodes[i].y
                );
            }
        }
    }

    #[test]
    fn a_tap_and_its_lane_rerouter_share_a_row() {
        // The horizontal run must be straight: same x on both ends.
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        for w in &l.bus.wires {
            if let (crate::layout::BusEnd::Bus(a), crate::layout::BusEnd::Bus(b)) =
                (w.source, w.target)
            {
                let (na, nb) = (&l.bus.nodes[a], &l.bus.nodes[b]);
                assert!(
                    na.x == nb.x || na.y == nb.y,
                    "every bus-to-bus run must be axis-aligned"
                );
            }
        }
    }

    /// No bus wire target may be driven twice, anywhere.
    fn assert_no_fan_in(l: &LayoutResult) {
        let mut seen: HashSet<BusEnd> = HashSet::default();
        for w in &l.bus.wires {
            assert!(seen.insert(w.target), "two wires into {:?}", w.target);
        }
    }

    #[test]
    fn the_bus_creates_no_fan_in() {
        let src = "var a: int = 1\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}\")\n  log.push(\"y\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert_no_fan_in(&l);
    }

    #[test]
    fn dag_mode_builds_no_bus() {
        let m = lowered("var a: int = 1\nin go: exec\non go { PrintToConsole(\"${a}\") }\n");
        let l = crate::layout::layout(&m);
        assert!(l.bus.is_empty());
    }

    #[test]
    fn tap_rerouters_point_right_and_lane_rerouters_point_down() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "fixture must build a bus");
        // A rerouter faces what it FEEDS, so its turn names the direction the
        // value travels next. A LANE run goes rightward into the body, so the
        // band is built from exactly two facings and never the leftward one.
        //
        // Two structures do face left and neither is a lane brick: a mini-bus
        // corner, and a source-side rerouter handing its producer's value out
        // to the gutter. `BusRole` is what separates them from the band, so
        // they are counted here on their own terms rather than being allowed
        // to soften the count below.
        let mut downs = 0;
        let mut ups = 0;
        let mut rights = 0;
        let mut lefts = 0;
        let mut source_lefts = 0;
        let mut source_others = 0;
        for n in &l.bus.nodes {
            match (n.role, n.rotation) {
                (BusRole::Gutter, crate::layout::NodeRotation::Deg90) => downs += 1,
                (BusRole::Gutter, crate::layout::NodeRotation::Deg270) => ups += 1,
                (BusRole::Gutter, crate::layout::NodeRotation::Deg0) => rights += 1,
                (BusRole::Gutter, crate::layout::NodeRotation::Deg180) => lefts += 1,
                (BusRole::Source, crate::layout::NodeRotation::Deg180) => source_lefts += 1,
                (BusRole::Source, _) => source_others += 1,
                (BusRole::Line, _) => {}
            }
        }
        assert!(
            downs + ups > 0,
            "lane rerouters must carry the chain, down or up"
        );
        assert!(rights > 0, "tap and gate-side rerouters must point right");
        assert_eq!(
            lefts, 0,
            "a gutter run goes rightward into the body; no lane brick faces left"
        );
        assert!(
            source_lefts > 0,
            "fixture must stand a rerouter beside a producer, or the split \
             between lane bricks and source-side ones proves nothing"
        );
        assert_eq!(
            source_others, 0,
            "a source-side rerouter hands its value LEFT into the gutter"
        );
    }

    #[test]
    fn each_tap_has_a_leaf_partner_before_any_real_port() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        // Walk the bus wires: a value reaches a real port from a LEAF, never
        // straight off a chain link. `Deg90` is what carries a chain onward,
        // so a leaf is anything else — which is the property this means, and
        // it stays true however many facings leaves come in.
        for w in &l.bus.wires {
            if let (crate::layout::BusEnd::Bus(a), crate::layout::BusEnd::Node(_)) =
                (w.source, w.target)
            {
                assert!(
                    !carries_a_chain(l.bus.nodes[a].rotation),
                    "a wire into a real port must leave a leaf, not a chain link"
                );
            }
        }
    }

    #[test]
    fn a_lane_head_sits_at_its_sources_row() {
        let src = TWO_ROW_BUS_SRC;
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        // The head is the GUTTER node a real port reaches FIRST. Where the
        // producer is a body gate it reaches it through the source-side
        // rerouter standing beside that gate, so the walk has to follow that
        // hop — reading only the wires whose target is already a lane brick
        // would match nothing here and go green having checked no head at all.
        //
        // A mini-bus corner is also fed by a real port, but it stands on its
        // CONSUMER's row rather than its source's — that is what turns the
        // wire — so it is a different claim and is pinned separately.
        let mut checked = 0usize;
        for w in &l.bus.wires {
            let (crate::layout::BusEnd::Node(src_port), crate::layout::BusEnd::Bus(first)) =
                (w.source, w.target)
            else {
                continue;
            };
            let head = &l.bus.nodes[through_source_side(&l, first)];
            if head.role != BusRole::Gutter {
                continue;
            }
            let src_x = l.placements[&src_port.node_id].x;
            assert!(
                (head.x - src_x).abs() <= 10,
                "lane head x {} must sit at its source's row {src_x}",
                head.x
            );
            assert_eq!(
                head.rotation,
                crate::layout::NodeRotation::Deg90,
                "a lane head points down"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "fixture must head a lane from a real port, or this proves nothing"
        );
    }

    /// A body handing values AND exec across a chip wall, so one fixture
    /// carries a producer of each kind: the gate that hands the anonymous
    /// chip its exec, and the chip brick a value is read back out of.
    const CHIP_PRODUCER_SRC: &str = "var score: int = 0
var tick: int = 0
var log: string[]
in go: exec
chip Scorer(run: exec, amount: int) -> (total: int) {
  on run {
    score = score + amount
    log.push(\"s\")
  }
  out total = score
}
let scored = Scorer(go, 5)
on go {
  score = 0
  PrintToConsole(\"a\")
  chip {
    score = score + 1
    log.push(\"i\")
  }
  tick = tick + 1
  log.push(\"r${tick}\")
  PrintToConsole(\"z${scored.total}\")
}
";

    /// The producer side of a lane, mirroring the consumer side it has always
    /// had: a rerouter standing beside the gate that makes the value, which
    /// that gate wires into and which hands the value out to the lane head.
    ///
    /// Without it a lane began at the gutter and the only thing marking where
    /// a value LEFT was the far end of a long wire. Values and execs alike —
    /// the same code path lays both, and an exec chain is the flow the brick
    /// most needs to make visible.
    #[test]
    fn a_lane_is_fed_from_a_rerouter_beside_its_producer() {
        let mut exec_producers = 0usize;
        for (name, m) in [
            ("values", lowered(TWO_ROW_BUS_SRC)),
            ("crossings", lowered(CHIP_PRODUCER_SRC)),
        ] {
            let l = layout_code(&m, &code_opts(), false);
            let owners = chip_owners(&m);
            let mut beside = 0usize;
            for w in &l.bus.wires {
                let (BusEnd::Node(src), BusEnd::Bus(i)) = (w.source, w.target) else {
                    continue;
                };
                if l.bus.nodes[i].role != BusRole::Source {
                    continue;
                }
                // The brick THIS module sees the value leave: the producing
                // gate itself, or the chip a value is read back out of.
                let anchor = if m.nodes.contains_key(&src.node_id) {
                    src.node_id
                } else {
                    owners.owner[&src.node_id]
                };
                let at = l.placements[&anchor];
                let node = &l.bus.nodes[i];
                assert_eq!(node.z, at.z, "{name}: the rerouter shares its producer's plane");
                assert_eq!(
                    node.x, at.x,
                    "{name}: ...stands at the producer's own level"
                );
                assert_eq!(
                    node.y,
                    at.y - TAP_RESERVE,
                    "{name}: ...one tap reserve to its left, the mirror of a \
                     gate-side rerouter"
                );
                assert_eq!(
                    node.rotation,
                    NodeRotation::Deg180,
                    "{name}: ...facing the gutter its value travels into"
                );
                // ...and it hands that value to a lane head on the same level,
                // so the run out to the band is horizontal rather than the
                // diagonal the gate's own port used to draw.
                let head = &l.bus.nodes[through_source_side(&l, i)];
                assert_eq!(
                    head.role,
                    BusRole::Gutter,
                    "{name}: a source-side rerouter feeds a lane head"
                );
                assert_eq!(head.x, node.x, "{name}: ...on the level it stands at");
                assert!(
                    head.y < node.y,
                    "{name}: ...out in the gutter, left of the body"
                );
                if src.port.as_str().contains("Exec") {
                    exec_producers += 1;
                }
                beside += 1;
            }
            assert!(
                beside > 0,
                "{name}: fixture must stand a rerouter beside a producer"
            );
            assert_no_fan_in(&l);
            assert_no_overlap(&m, &l);
            assert_suppressed_consumers_stay_reachable(&m, &l);
        }
        assert!(
            exec_producers > 0,
            "an exec delivery must get a source-side rerouter like any value"
        );
    }

    /// The chain invariant, restated for the shape the source-side rerouter
    /// makes rather than weakened by it.
    ///
    /// A lane HEAD is fed from outside its chain — by a real port, or now by
    /// the rerouter standing beside its producer, which is itself fed by that
    /// port and drives nothing else. Every OTHER lane brick must still be fed
    /// by a lane brick. That is what keeps a lane one value from one producer:
    /// a second entry point partway down a column would carry a different
    /// value into the same run, and no consumer of it would say so.
    #[test]
    fn only_a_lane_head_is_fed_from_outside_its_chain() {
        for (name, m) in [
            ("two_row", lowered(TWO_ROW_BUS_SRC)),
            ("band", lowered(BAND_SRC)),
            ("mini", lowered(MINI_BUS_SRC)),
            ("crossings", lowered(CHIP_PRODUCER_SRC)),
        ] {
            let l = layout_code(&m, &code_opts(), false);
            let mut inbound: HashMap<usize, BusEnd> = HashMap::default();
            for w in &l.bus.wires {
                if let BusEnd::Bus(i) = w.target {
                    assert!(
                        inbound.insert(i, w.source).is_none(),
                        "{name}: bus node {i} is driven twice"
                    );
                }
            }

            let mut entries = 0usize;
            for (i, n) in l.bus.nodes.iter().enumerate() {
                if n.role != BusRole::Gutter {
                    continue;
                }
                match inbound[&i] {
                    BusEnd::Node(_) => entries += 1,
                    BusEnd::Bus(p) if l.bus.nodes[p].role == BusRole::Source => {
                        assert!(
                            matches!(inbound[&p], BusEnd::Node(_)),
                            "{name}: the source-side rerouter feeding lane brick {i} is \
                             itself fed from the bus, not from its producer's port"
                        );
                        entries += 1;
                    }
                    BusEnd::Bus(p) => assert_eq!(
                        l.bus.nodes[p].role,
                        BusRole::Gutter,
                        "{name}: lane brick {i} is fed by a {:?} node, not by its own chain",
                        l.bus.nodes[p].role
                    ),
                }
            }
            let chains = bus_chains(&l);
            assert!(!chains.is_empty(), "{name}: fixture must build lanes");
            assert_eq!(
                entries,
                chains.len(),
                "{name}: a lane takes exactly one entry point"
            );

            // ...and the new brick is an entry point and nothing more: it
            // hands its value to the band, never to a gate, and never twice.
            for (i, n) in l.bus.nodes.iter().enumerate() {
                if n.role != BusRole::Source {
                    continue;
                }
                let out: Vec<BusEnd> = l
                    .bus
                    .wires
                    .iter()
                    .filter(|w| w.source == BusEnd::Bus(i))
                    .map(|w| w.target)
                    .collect();
                assert_eq!(
                    out.len(),
                    1,
                    "{name}: source-side rerouter {i} drives {} bricks",
                    out.len()
                );
                assert!(
                    matches!(out[0], BusEnd::Bus(_)),
                    "{name}: a source-side rerouter feeds the band, not a gate"
                );
            }
        }
    }

    #[test]
    fn lanes_are_packed_at_the_tightened_pitch() {
        assert_eq!(LANE_PITCH, 2, "lane spacing is halved");
    }

    /// A port heads a lane just like a stored value does, even though the
    /// pin stack is placed only after the band it has to clear is measured.
    /// Without the plan/lay split the port would have no placement to stand
    /// beside when its lane is laid.
    #[test]
    fn an_input_ports_lane_is_headed_beside_the_port() {
        let src = "in v: int
in go: exec
var b: int = 2\nvar log: string[]\non go {
  PrintToConsole(\"${v}\")
  PrintToConsole(\"x${v}\")\n  log.push(\"p${b}\")\n  PrintToConsole(\"q${b}\")\n  b = b + 1\n  log.push(\"r${b}\")
}
";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        // BOTH declared ports head a lane: the int one because its value is
        // read on two rows, and the exec one because handing exec from a port
        // into the body is a delivery, not spine sequencing.
        //
        // A port drives its head DIRECTLY. The source-side rerouter a body
        // producer gets stands one tap reserve to the producer's left, and an
        // edge pin's stack is placed on the far side of the band from the
        // body — so a brick there would face away from the lane it feeds, and
        // no row out there reserves a cell for it. That the target below is
        // the head itself is what pins the exclusion.
        let headed: Vec<(NodeId, &BusNode)> = l
            .bus
            .wires
            .iter()
            .filter_map(|w| match (w.source, w.target) {
                (BusEnd::Node(p), BusEnd::Bus(i))
                    if m.nodes
                        .get(&p.node_id)
                        .is_some_and(|n| n.kind == NodeKind::Input) =>
                {
                    Some((p.node_id, &l.bus.nodes[i]))
                }
                _ => None,
            })
            .collect();
        let ports: HashSet<NodeId> = headed.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ports.len(),
            2,
            "both declared ports must head a lane, got {}",
            ports.len()
        );
        for (pin_id, head) in &headed {
            assert_eq!(
                head.role,
                BusRole::Gutter,
                "an edge pin drives its lane brick directly; nothing stands \
                 between the pin stack and the band"
            );
            // The lane's first brick stands in the port's OWN row band. Two
            // shapes land here and both are correct: a head proper, at the
            // port's row exactly, and — when the port is level with the
            // lane's first stop, so that stop's tap already holds the cell —
            // the stop's own link, one rerouter above it. Either way the
            // brick is inside the row, which is what proves the port's
            // placement was known when the lane was laid; that is the whole
            // point of the plan/lay split, since the pin stack is placed only
            // after the band it has to clear is measured.
            let at = l.placements[pin_id].x;
            assert!(
                head.x >= at && head.x < at + LANE_PAIR_HEIGHT,
                "the lane's first brick sits at x={}, outside the port's own \
                 row band [{at}, {})",
                head.x,
                at + LANE_PAIR_HEIGHT
            );
            assert_eq!(head.rotation, NodeRotation::Deg90, "a lane head points down");
        }
    }

    /// A value entering a chip is a delivery, not a diagonal. The pin it
    /// lands on lives in the CHILD module and holds no row here, so its tap
    /// anchors on the chip brick the value enters through. Exec deliveries
    /// count: handing exec to a chip is a crossing, which is a different
    /// thing from an exec chain sequencing statements along one spine.
    #[test]
    fn values_entering_a_chip_tap_the_bus_at_the_chips_row() {
        let src = "var score: int = 0
var log: string[]
var tick: int = 0
in go: exec
chip Scorer(run: exec, amount: int) -> (total: int) {
  on run {
    score = score + amount
    log.push(\"s\")
  }
  out total = score
}
let scored = Scorer(go, 5)
on go {
  score = 0
  chip {
    score = score + 1
    log.push(\"i\")
  }
  PrintToConsole(\"${scored.total}\")
  log.push(\"p${tick}\")
  PrintToConsole(\"q${tick}\")
  tick = tick + 1
  log.push(\"r${tick}\")
}
";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        // A delivery: a wire out of this module's body whose target is not a
        // node of this module, so it lands inside some chip.
        let deliveries: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| !m.nodes.contains_key(&w.target.node_id))
            .filter(|w| m.nodes.get(&w.source.node_id).is_some_and(is_spawnable))
            .collect();
        assert!(
            deliveries.len() >= 3,
            "fixture must deliver several values into chips, got {}",
            deliveries.len()
        );
        assert!(
            deliveries
                .iter()
                .any(|w| w.source.port.as_str().contains("Exec")),
            "fixture must include an exec delivery"
        );

        let owners = chip_owners(&m);
        for w in &deliveries {
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the delivery {} .{} -> {} .{} still runs as a direct wire",
                w.source.node_id,
                w.source.port.as_str(),
                w.target.node_id,
                w.target.port.as_str()
            );
            let chip_id = *owners
                .owner
                .get(&w.target.node_id)
                .expect("a delivery lands inside some chip of this module");
            let chip = &m.nodes[&chip_id];
            let chip_at = l.placements[&chip_id];
            let driver = l
                .bus
                .wires
                .iter()
                .find(|bw| bw.target == BusEnd::Node(w.target))
                .and_then(|bw| match bw.source {
                    BusEnd::Bus(i) => Some(&l.bus.nodes[i]),
                    BusEnd::Node(_) => None,
                })
                .expect("a bus node drives the suppressed port");
            let top = chip_at.x + measured_half_size(chip, &l).0 * 2;
            assert!(
                driver.x >= chip_at.x && driver.x < top,
                "tap for {} .{} sits at x {}, off chip {chip_id}'s row [{}, {top})",
                w.target.node_id,
                w.target.port.as_str(),
                driver.x,
                chip_at.x
            );
            assert!(
                driver.y < chip_at.y,
                "a tap feeds its chip from the left, not from x {} y {}",
                driver.x,
                driver.y
            );
            assert_eq!(
                driver.rotation,
                NodeRotation::Deg0,
                "a chip is fed by a right-pointing tap like any other consumer"
            );
        }

        // Several values enter the same chip, so several taps land on one
        // row. That is fan-OUT across distinct ports; two wires into one port
        // would be fan-in, and the bricks must still not collide.
        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
    }

    /// A statement's exec gate sits BELOW the expression it consumes, not
    /// above it, while keeping the left column it heads its line from.
    ///
    /// Values flow left to right into the sink; stacking the sink on the
    /// expression's own top row put it level with the operands it reads, so
    /// the run into it came back leftward across the line. Dropping it to the
    /// bottom row of its group's block turns that into a descent: the
    /// expression resolves across the upper rows, and the value comes DOWN
    /// into the statement that consumes it.
    ///
    /// The column is deliberately unchanged. Exec gates still line up in one
    /// vertical column down the left margin, so the chain reads as a single
    /// spine and the downward-rotation rule keeps its meaning.
    #[test]
    fn a_statement_exec_gate_sits_below_the_expression_it_consumes() {
        let m = lowered("var x: int = 0\nin t: exec\non t { x = (1 + x) * (2 + x) }");
        let l = layout_code(&m, &opts(), false);
        let var_set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_SET)
            .expect("Var_Set node");
        let line = var_set.source_range.start.line;
        let set_at = l.placements[&var_set.id];

        let expression: Vec<&Node> = m
            .nodes
            .values()
            .filter(|n| {
                n.kind == NodeKind::Gate
                    && n.source_range.start.line == line
                    && n.id != var_set.id
                    && l.placements.contains_key(&n.id)
            })
            .collect();
        assert!(
            expression.len() >= 4,
            "fixture must lower an expression tree, got {} gates",
            expression.len()
        );

        // Lower on the plane is a SMALLER x: a row's placement is derived as
        // `page_h - down - height`, so a greater sub-row offset reads lower.
        for n in &expression {
            assert!(
                l.placements[&n.id].x > set_at.x,
                "expression gate {} sits at x={}, not ABOVE the sink it feeds \
                 (sink x={})",
                n.gate_class,
                l.placements[&n.id].x,
                set_at.x
            );
        }

        // ...and it still heads its line on the left.
        for n in &expression {
            assert!(
                l.placements[&n.id].y > set_at.y,
                "expression gate {} must stay RIGHT of the sink",
                n.gate_class
            );
        }
        assert_eq!(
            rotation_of(&l.rotations, &var_set.id),
            NodeRotation::Deg90,
            "the sink is still on the spine and still faces down it"
        );
    }

    /// Dropping the sink below its expression turns the operand wire into a
    /// down-AND-left diagonal across the line's own block. The mini-bus turns
    /// it back into a right angle: one rerouter standing where the operand's
    /// column meets the sink's row takes the value straight DOWN, then hands
    /// it straight LEFT along a row that holds nothing but the sink.
    ///
    /// Same rules as the gutter bus, which is why it is the same `BusLayout`:
    /// a `Deg0` leaf feeding one port, exactly one inbound wire, and the
    /// module wire it replaces suppressed so emit never draws both.
    #[test]
    fn the_mini_bus_drops_an_expression_value_into_its_statement() {
        let m = lowered(MINI_BUS_SRC);
        let l = layout_code(&m, &opts(), false);
        let var_set = m
            .nodes
            .values()
            .find(|n| n.gate_class == gate_class::VAR_SET)
            .expect("Var_Set node");
        let line = var_set.source_range.start.line;

        // The operand wires the drop created: source on the sink's own line,
        // above it and to its right.
        let drops: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| w.target.node_id == var_set.id)
            .filter(|w| {
                m.nodes
                    .get(&w.source.node_id)
                    .is_some_and(|n| n.kind == NodeKind::Gate && n.source_range.start.line == line)
            })
            .collect();
        assert!(
            !drops.is_empty(),
            "fixture must feed its sink from an expression on the same line"
        );

        for w in &drops {
            let src_at = l.placements[&w.source.node_id];
            let sink_at = l.placements[&var_set.id];
            assert!(
                src_at.x > sink_at.x && src_at.y > sink_at.y,
                "fixture operand must sit above and right of its sink"
            );
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the drop into {} .{} still runs as a direct diagonal",
                w.target.node_id,
                w.target.port.as_str()
            );
            let driver = l
                .bus
                .wires
                .iter()
                .find(|bw| bw.target == BusEnd::Node(w.target))
                .and_then(|bw| match bw.source {
                    BusEnd::Bus(i) => Some(&l.bus.nodes[i]),
                    BusEnd::Node(_) => None,
                })
                .expect("a bus node drives the suppressed port");
            assert_eq!(
                driver.rotation,
                NodeRotation::Deg180,
                "a mini-bus rerouter is a leaf like every other tap, but it                  runs LEFTWARD into its statement, so it faces the other way"
            );
            // The right angle, both halves: on the sink's own row, and in the
            // column of the operand it reads.
            //
            // The row is a BAND, not one level. Two operands dropping into a
            // single statement are staggered inside it so their runs are not
            // drawn on top of each other; a corner OUTSIDE the band would be a
            // run that no longer arrives at the gate it feeds.
            let sink_top = sink_at.x + measured_half_size(var_set, &l).0 * 2;
            assert!(
                driver.x >= sink_at.x && driver.x + 2 * REROUTER_HALF <= sink_top,
                "the rerouter must stand within the sink's own row band \
                 [{}, {sink_top}), got x={}",
                sink_at.x,
                driver.x
            );
            assert_eq!(
                driver.y, src_at.y,
                "...and in the operand's own column, so the drop is straight"
            );
        }

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// A lane must travel farther than the bricks it stands up to travel.
    ///
    /// A group whose source and taps sit at the same height buys nothing: the
    /// value was already an inline hop, and the lane replaces it with a
    /// rerouter out in the gutter plus a gate-side rerouter coming back —
    /// strictly more bricks and a longer path for the same wire.
    ///
    /// Measured over every chain that has a locally-placed producer, so a chip
    /// exit (whose producer lives in the child module) is out of scope here
    /// rather than silently passing.
    #[test]
    fn a_group_with_no_vertical_travel_is_not_bussed() {
        // Single-page fixtures only. Pages are centred independently, so on a
        // paginated body a producer and a consumer on different pages have
        // incomparable `x`: the rule measures within one page and so must this
        // check, rather than comparing coordinates from two origins.
        let cases: Vec<(&str, Module, CodeBudgets)> = vec![
            ("band", lowered(BAND_SRC), CodeBudgets::default()),
            ("mini", lowered(MINI_BUS_SRC), CodeBudgets::default()),
            ("two_row", lowered(TWO_ROW_BUS_SRC), CodeBudgets::default()),
        ];
        let mut checked = 0usize;
        let mut tall = 0usize;
        for (name, m, budgets) in &cases {
            let l = layout_code_with_budgets(m, &opts(), false, budgets);
            for c in bus_chains(&l) {
                let Some(src) = l.placements.get(&c.source.node_id) else {
                    continue;
                };
                let members: HashSet<usize> =
                    c.lanes.iter().chain(c.taps.iter()).copied().collect();
                // Heights compare only within one page, and a page IS a z
                // plane, so consumers on another plane are out of scope.
                let mut lo = src.x;
                let mut hi = src.x;
                for w in &l.bus.wires {
                    if let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) {
                        if members.contains(&i) {
                            if let Some(p) = l.placements.get(&t.node_id) {
                                if p.z != src.z {
                                    continue;
                                }
                                lo = lo.min(p.x);
                                hi = hi.max(p.x);
                            }
                        }
                    }
                }
                if hi == lo && members.is_empty() {
                    continue;
                }
                checked += 1;
                if hi - lo >= MIN_LANE_TRAVEL {
                    tall += 1;
                }
                assert!(
                    hi - lo >= MIN_LANE_TRAVEL,
                    "{name}: a lane carrying {:?} travels only {} units — the                      value was already inline and the lane costs more than the                      wire it replaced",
                    c.source.node_id,
                    hi - lo
                );
            }
        }
        assert!(checked > 4, "fixtures must build real lanes, got {checked}");
        assert!(tall > 0, "a genuinely tall lane must still bus");
    }

    /// The rotation that carries a chain onward, either way it runs.
    ///
    /// A lane whose taps all sit ABOVE its source travels upward, so its links
    /// face up. Both facings are chain links; neither is a leaf. Tests that
    /// hardcode `Deg90` would misfile an upward lane's links as leaves.
    fn carries_a_chain(r: NodeRotation) -> bool {
        matches!(r, NodeRotation::Deg90 | NodeRotation::Deg270)
    }

    /// A lane whose taps ALL sit above its source runs upward, and its chain
    /// links say so.
    ///
    /// The links are what travel; the leaves are not. A gate-side rerouter
    /// still hands its value rightward into the gate beside it whichever way
    /// the lane came from, so flipping the whole lane would point every leaf
    /// away from the thing it feeds. Only the chain turns.
    #[test]
    fn a_lane_whose_taps_are_all_above_its_source_runs_upward() {
        // Pagination is what puts a producer BELOW the rows that read it: a
        // page restarts the row order, so a value carried onto a later page
        // heads its lane from underneath its own consumers.
        let (m, budgets) = paginated_bus_module();
        let l = layout_code_with_budgets(&m, &opts(), false, &budgets);
        let chains = bus_chains(&l);
        assert!(!chains.is_empty(), "fixture must build lanes");

        let mut up = 0usize;
        let mut down = 0usize;
        {
            let l = &l;
        for c in &chains {
            if c.lanes.is_empty() {
                continue;
            }
            let tap_levels: Vec<i32> = c.taps.iter().map(|&i| l.bus.nodes[i].x).collect();
            if tap_levels.is_empty() {
                continue;
            }
            // `Chain.lanes` is "driven by something in another row", which
            // also catches a tap whose head was dropped for a contested cell.
            // The claim is about the nodes that actually CARRY the chain.
            let links: Vec<usize> = c
                .lanes
                .iter()
                .copied()
                .filter(|&i| carries_a_chain(l.bus.nodes[i].rotation))
                .collect();
            if links.is_empty() {
                continue;
            }
            // One lane, one direction: every link of a chain must agree, or
            // the run reverses partway down its own column.
            let facing = l.bus.nodes[links[0]].rotation;
            for &i in &links {
                assert_eq!(
                    l.bus.nodes[i].rotation,
                    facing,
                    "a lane's links must all face the same way; taps at                      {tap_levels:?}"
                );
            }
            match facing {
                NodeRotation::Deg270 => up += 1,
                _ => down += 1,
            }

            // ...and where the producer is placed in THIS module, the facing
            // has to match where the taps actually sit. A chip exit's producer
            // lives in the child module and holds no placement here, which is
            // exactly the lane that runs upward, so the semantic check is made
            // wherever it CAN be made rather than skipping the interesting
            // ones and proving nothing.
            if let Some(src) = l.placements.get(&c.source.node_id) {
                // The CONSUMERS' own rows, which is what the decision reads —
                // not the stop columns the bricks ended up in.
                let members: HashSet<usize> =
                    c.lanes.iter().chain(c.taps.iter()).copied().collect();
                let consumers: Vec<i32> = l
                    .bus
                    .wires
                    .iter()
                    .filter_map(|w| match (w.source, w.target) {
                        (BusEnd::Bus(i), BusEnd::Node(t)) if members.contains(&i) => {
                            l.placements.get(&t.node_id).map(|p| p.x)
                        }
                        _ => None,
                    })
                    .collect();
                if !consumers.is_empty() {
                    let all_above = consumers.iter().all(|&x| x > src.x);
                    assert_eq!(
                        facing == NodeRotation::Deg270,
                        all_above,
                        "a lane chains upward exactly when its taps are all                          above its source; source at x={}, consumers at                          {consumers:?}",
                        src.x
                    );
                }
            }
            // Its leaves are untouched either way: they face the gates.
            for &i in &c.taps {
                assert!(
                    !carries_a_chain(l.bus.nodes[i].rotation),
                    "a leaf must not take a chain-carrying facing"
                );
            }
        }
        }
        assert!(up > 0, "fixture must build an upward lane");
        assert!(down > 0, "fixture must keep a downward lane");
    }

    /// A rerouter faces what it FEEDS, and the three structures feed in
    /// different directions.
    ///
    /// A gutter tap stands left of the body and runs RIGHTWARD into the gate
    /// it drives, so it faces `+Y` — `Deg0`. A mini-bus corner stands out in
    /// its operand's column and runs LEFTWARD into the statement gate at the
    /// line's indent column, so it faces `−Y` — `Deg180`. Inheriting the
    /// gutter's `Deg0` pointed every expression branch away from the very
    /// thing it was feeding. A source-side rerouter stands beside a producer
    /// and runs LEFTWARD out to the gutter, so it faces `−Y` too — the same
    /// rule reaching the opposite answer from the tap it mirrors.
    ///
    /// Chain links are untouched at `Deg90`: a half turn on those would point
    /// them up, which is not where a drop goes.
    #[test]
    fn bus_rerouters_face_the_direction_their_value_travels() {
        // Carries every structure: a band feeding several rows, a statement
        // consuming an expression above it, and producers beside the band.
        let m = lowered(MINI_BUS_SRC);
        let l = layout_code(&m, &opts(), false);

        let mut gutter_leaves = 0usize;
        let mut mini_leaves = 0usize;
        let mut source_leaves = 0usize;
        for n in &l.bus.nodes {
            match (n.role, n.rotation) {
                // A chain link, either structure: a quarter turn, down for a
                // lane running down and up for one running up.
                (_, NodeRotation::Deg90) | (_, NodeRotation::Deg270) => {}
                (BusRole::Gutter, NodeRotation::Deg0) => gutter_leaves += 1,
                (BusRole::Line, NodeRotation::Deg180) => mini_leaves += 1,
                (BusRole::Source, NodeRotation::Deg180) => source_leaves += 1,
                (role, rot) => panic!(
                    "a {role:?} rerouter at ({}, {}) faces {rot:?}, which is not \
                     the way its value travels",
                    n.x, n.y
                ),
            }
        }
        assert!(gutter_leaves > 0, "fixture must build gutter taps");
        assert!(mini_leaves > 0, "fixture must build mini-bus corners");
        assert!(source_leaves > 0, "fixture must build source-side rerouters");

        // The directions the faces claim, measured. A mini-bus corner stands
        // RIGHT of the port it feeds; a gutter tap stands LEFT of its
        // consumer. Without this the rotations above are just two constants.
        let mut mini_runs = 0usize;
        let mut gutter_runs = 0usize;
        for w in &l.bus.wires {
            let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) else {
                continue;
            };
            let Some(sink) = l.placements.get(&t.node_id) else {
                continue;
            };
            match l.bus.nodes[i].role {
                BusRole::Line => {
                    assert!(
                        l.bus.nodes[i].y > sink.y,
                        "a mini-bus corner must stand right of its sink, or \
                         Deg180 faces away from it"
                    );
                    mini_runs += 1;
                }
                BusRole::Gutter => {
                    assert!(
                        l.bus.nodes[i].y < sink.y,
                        "a gutter tap must stand left of its consumer, or Deg0 \
                         faces away from it"
                    );
                    gutter_runs += 1;
                }
                // A source-side rerouter takes a value OUT of the body into
                // the band; a wire from one into a real port would be a run
                // back into the gates it just left.
                BusRole::Source => panic!(
                    "a source-side rerouter at ({}, {}) drives the real port {} .{}",
                    l.bus.nodes[i].x,
                    l.bus.nodes[i].y,
                    t.node_id,
                    t.port.as_str()
                ),
            }
        }
        assert!(mini_runs > 0, "fixture must drive a port from the mini-bus");
        assert!(gutter_runs > 0, "fixture must drive a port from the gutter");
    }

    /// Every bus node that drives a real port, grouped by the row its
    /// consumer stands in. Two runs leaving at the SAME level draw over each
    /// other, so this is the raw material for both stagger tests.
    fn drivers_by_row(m: &Module, l: &LayoutResult, role: BusRole) -> HashMap<i32, Vec<usize>> {
        let mut out: HashMap<i32, Vec<usize>> = HashMap::default();
        for w in &l.bus.wires {
            let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) else {
                continue;
            };
            if l.bus.nodes[i].role != role {
                continue;
            }
            let Some(node) = m.nodes.get(&t.node_id) else {
                continue;
            };
            if !l.placements.contains_key(&t.node_id) {
                continue;
            }
            out.entry(row_top(node, l)).or_default().push(i);
        }
        out
    }

    /// Taps that share a row must not share a LEVEL.
    ///
    /// Several lanes tapping one row is the normal case — a statement reading
    /// two variables does it — and each of their runs leaves the gutter
    /// horizontally toward that row. At one level those runs are drawn on top
    /// of each other and the picture stops being readable, however clean the
    /// bricks underneath are. Offsetting each by a rerouter's height gives
    /// every lane its own line out.
    #[test]
    fn gutter_taps_sharing_a_row_are_staggered() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let chains = bus_chains(&l);
        let chain_of = |i: usize| -> Option<usize> {
            chains
                .iter()
                .position(|c| c.lanes.contains(&i) || c.taps.contains(&i))
        };

        let mut shared_rows = 0usize;
        for (row, drivers) in &drivers_by_row(&m, &l, BusRole::Gutter) {
            let lanes: HashSet<usize> = drivers.iter().filter_map(|&i| chain_of(i)).collect();
            if lanes.len() < 2 {
                continue;
            }
            shared_rows += 1;
            let mut levels: Vec<i32> = drivers.iter().map(|&i| l.bus.nodes[i].x).collect();
            levels.sort_unstable();
            levels.dedup();
            assert_eq!(
                levels.len(),
                lanes.len(),
                "row at {row}: {} lanes tap it but their runs leave at {} \
                 level(s) {levels:?}",
                lanes.len(),
                levels.len()
            );
        }
        assert!(
            shared_rows > 0,
            "fixture must have a row tapped by two lanes, or this proves nothing"
        );

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// Each staggered stop moves as ONE piece: the tap standing out in the
    /// gutter and every gate-side rerouter it drives share a level, so the run
    /// between them is horizontal. Shifting only one end is what would put the
    /// diagonals back.
    #[test]
    fn a_staggered_stop_stays_level_with_itself() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let mut checked = 0usize;
        for w in &l.bus.wires {
            let (BusEnd::Bus(a), BusEnd::Bus(b)) = (w.source, w.target) else {
                continue;
            };
            let (na, nb) = (&l.bus.nodes[a], &l.bus.nodes[b]);
            // Tap -> gate-side, and gate-side -> gate-side: the horizontal run
            // out of the gutter. Both ends are Deg0 leaves.
            if na.role != BusRole::Gutter
                || nb.role != BusRole::Gutter
                || na.rotation != NodeRotation::Deg0
                || nb.rotation != NodeRotation::Deg0
            {
                continue;
            }
            checked += 1;
            assert_eq!(
                na.x, nb.x,
                "a stop's run must stay level: {a} at x={} drives {b} at x={}",
                na.x, nb.x
            );
        }
        assert!(checked > 4, "fixture must run real taps, got {checked}");
    }

    /// The same rule for the mini-bus. Two operands dropping into one
    /// statement leave their corners on that statement's row, and at one level
    /// their runs into it are drawn on top of each other — the same collision,
    /// in the line's own block rather than out in the gutter.
    #[test]
    fn mini_bus_corners_sharing_a_sink_are_staggered() {
        let m = lowered(MINI_BUS_SRC);
        let l = layout_code(&m, &opts(), false);

        let mut shared = 0usize;
        for (row, drivers) in &drivers_by_row(&m, &l, BusRole::Line) {
            if drivers.len() < 2 {
                continue;
            }
            shared += 1;
            let mut levels: Vec<i32> = drivers.iter().map(|&i| l.bus.nodes[i].x).collect();
            levels.sort_unstable();
            levels.dedup();
            assert_eq!(
                levels.len(),
                drivers.len(),
                "sink row at {row}: {} corners drop into it but leave at {} \
                 level(s) {levels:?}",
                drivers.len(),
                levels.len()
            );
        }
        assert!(
            shared > 0,
            "fixture must drop two operands into one statement"
        );

        // A staggered corner must still stand inside the sink's own vertical
        // extent, or the run into it leaves the gate it is meant to feed.
        for w in &l.bus.wires {
            let (BusEnd::Bus(i), BusEnd::Node(t)) = (w.source, w.target) else {
                continue;
            };
            let node = &l.bus.nodes[i];
            if node.role != BusRole::Line {
                continue;
            }
            let sink = &m.nodes[&t.node_id];
            let at = l.placements[&t.node_id];
            let top = at.x + measured_half_size(sink, &l).0 * 2;
            assert!(
                node.x >= at.x && node.x + 2 * REROUTER_HALF <= top,
                "a staggered corner at x={} left its sink's band [{}, {top})",
                node.x,
                at.x
            );
        }
        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// A handler driven by a declared exec port, with a body long enough to
    /// sequence one statement into the next — so the same fixture carries both
    /// an exec DELIVERY (port → body) and exec SEQUENCING (body gate → body
    /// gate), and the two must be treated differently.
    const EXEC_DELIVERY_SRC: &str = "var score: int = 0
var tick: int = 0
var log: string[]
in start: exec
on start {
  score = 0
  PrintToConsole(\"reset\")
  PrintToConsole(\"done\")
  log.push(\"p${tick}\")
  PrintToConsole(\"q${tick}\")
  tick = tick + 1
  log.push(\"r${tick}\")
}
";

    /// Every exec wire out of a declared `in <name>: exec` port, paired with
    /// the module wire it is, over `EXEC_DELIVERY_SRC`.
    fn exec_deliveries<'a>(m: &'a Module) -> Vec<&'a Wire> {
        m.wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| {
                m.nodes
                    .get(&w.source.node_id)
                    .is_some_and(|n| n.kind == NodeKind::Input)
            })
            .filter(|w| targets_exec(m, &w.target))
            .collect()
    }

    /// An `in <name>: exec` port handing exec to a handler in the SAME module
    /// is a DELIVERY, not spine sequencing, and takes a lane like any other
    /// value.
    ///
    /// The exclusion this pins the correction to was written for the spine:
    /// one statement handing the chain to the next, which must stay a short
    /// direct wire on its own line. A wire out of an input PORT is not that.
    /// The port stacks on the page's left edge, so its direct wire is exactly
    /// the long diagonal across the whole body that the gutter exists to
    /// remove — the same shape as the delivery into a chip, which already
    /// buses.
    #[test]
    fn an_input_ports_exec_delivery_into_a_handler_is_bussed() {
        let m = lowered(EXEC_DELIVERY_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let deliveries = exec_deliveries(&m);
        assert_eq!(
            deliveries.len(),
            1,
            "fixture must hand exec from one declared port into its handler"
        );
        let w = deliveries[0];
        assert!(
            l.bus
                .suppressed
                .contains(&(w.target.node_id, w.target.port)),
            "the exec delivery {} .{} -> {} .{} still runs as a direct wire",
            w.source.node_id,
            w.source.port.as_str(),
            w.target.node_id,
            w.target.port.as_str()
        );

        // ...and a lane really carries it: a bus node drives the port, and it
        // stands on the CONSUMING row, not out at the port's own.
        let driver = l
            .bus
            .wires
            .iter()
            .find(|bw| bw.target == BusEnd::Node(w.target))
            .and_then(|bw| match bw.source {
                BusEnd::Bus(i) => Some(&l.bus.nodes[i]),
                BusEnd::Node(_) => None,
            })
            .expect("a bus node drives the suppressed exec port");
        let target_at = l.placements[&w.target.node_id];
        let target = &m.nodes[&w.target.node_id];
        let top = target_at.x + measured_half_size(target, &l).0 * 2;
        assert!(
            driver.x >= target_at.x && driver.x < top,
            "the tap sits at x {}, off its consumer's row [{}, {top})",
            driver.x,
            target_at.x
        );
        assert_eq!(
            driver.rotation,
            NodeRotation::Deg0,
            "an exec consumer is fed by a right-pointing tap like any other"
        );

        // The lane is headed beside the port itself, pointing down.
        let head = l
            .bus
            .wires
            .iter()
            .find_map(|bw| match (bw.source, bw.target) {
                (BusEnd::Node(p), BusEnd::Bus(i)) if p.node_id == w.source.node_id => {
                    Some(&l.bus.nodes[i])
                }
                _ => None,
            })
            .expect("the port heads a lane");
        assert_eq!(
            head.x, l.placements[&w.source.node_id].x,
            "the head stands at the port's own row"
        );
        assert_eq!(head.rotation, NodeRotation::Deg90, "a lane head points down");

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// The other half, and the reason the exclusion exists at all: an exec
    /// wire from one BODY GATE to the next is spine sequencing and stays a
    /// direct wire. Routing it through the gutter would take the spine off its
    /// own line, which is the spec's stated non-goal.
    #[test]
    fn body_gate_exec_sequencing_stays_direct() {
        let m = lowered(EXEC_DELIVERY_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        let sequencing: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| targets_exec(&m, &w.target))
            // A STATEMENT gate, which the layout records by turning it down
            // the spine. An expression-side `Exec_Var_Get` is also a Gate
            // handing exec on, but it lives in the value columns and its
            // exec-out is an expression value — the mini-bus turns those
            // deliberately, and reading `kind == Gate` here would call that a
            // regression.
            .filter(|w| {
                m.nodes.contains_key(&w.source.node_id)
                    && rotation_of(&l.rotations, &w.source.node_id) == NodeRotation::Deg90
            })
            .filter(|w| m.nodes.contains_key(&w.target.node_id))
            .collect();
        assert!(
            sequencing.len() >= 2,
            "fixture must chain at least three statements, got {} exec hops",
            sequencing.len()
        );
        for w in &sequencing {
            assert!(
                !l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the spine hop {} .{} -> {} .{} was detoured through the gutter",
                w.source.node_id,
                w.source.port.as_str(),
                w.target.node_id,
                w.target.port.as_str()
            );
        }
    }

    /// The mirror of a delivery: a value LEAVING a chip. Its producing pin
    /// lives in the CHILD module and holds no row here, so the lane head —
    /// which always stands beside its source, pointing down — anchors on the
    /// chip brick the value comes out of.
    #[test]
    fn values_leaving_a_chip_head_a_lane_at_the_chips_row() {
        let src = "in go: exec
var tick: int = 0
var log: string[]
chip Scorer(run: exec, amount: int) -> (total: int) {
  var score: int = 0
  on run { score = score + amount }
  out total = score
}
let scored = Scorer(go, 5)
on go {
  PrintToConsole(\"${scored.total}\")
  PrintToConsole(\"x${scored.total}\")
  log.push(\"p${tick}\")
  PrintToConsole(\"q${tick}\")
  tick = tick + 1
  log.push(\"r${tick}\")
}
";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);

        // An exit: a wire landing on this module's body whose source is not a
        // node of this module, so it leaves some chip.
        let exits: Vec<&Wire> = m
            .wires
            .iter()
            .filter(|w| w.source.port != WirePort::Layout && w.target.port != WirePort::Layout)
            .filter(|w| !m.nodes.contains_key(&w.source.node_id))
            .filter(|w| m.nodes.contains_key(&w.target.node_id))
            .collect();
        assert!(
            !exits.is_empty(),
            "fixture must read a value back out of a chip"
        );

        let owners = chip_owners(&m);
        for w in &exits {
            assert!(
                l.bus
                    .suppressed
                    .contains(&(w.target.node_id, w.target.port)),
                "the exit {} .{} -> {} .{} still runs as a direct wire",
                w.source.node_id,
                w.source.port.as_str(),
                w.target.node_id,
                w.target.port.as_str()
            );
            let chip_id = *owners
                .owner
                .get(&w.source.node_id)
                .expect("an exit leaves some chip of this module");
            let chip_at = l.placements[&chip_id];
            // The chip brick is a body producer like any other, so its value
            // reaches the head through the source-side rerouter standing
            // beside it. Reading the wire's target as the head directly would
            // land on that rerouter and pin the wrong brick's facing.
            let head = l
                .bus
                .wires
                .iter()
                .find(|bw| bw.source == BusEnd::Node(w.source))
                .and_then(|bw| match bw.target {
                    BusEnd::Bus(i) => Some(&l.bus.nodes[through_source_side(&l, i)]),
                    BusEnd::Node(_) => None,
                })
                .expect("a lane head reads the chip's output");
            let top = chip_at.x + measured_half_size(&m.nodes[&chip_id], &l).0 * 2;
            assert!(
                head.x >= chip_at.x && head.x < top,
                "lane head for {} .{} sits at x {}, off chip {chip_id}'s row [{}, {top})",
                w.source.node_id,
                w.source.port.as_str(),
                head.x,
                chip_at.x
            );
            assert_eq!(
                head.rotation,
                NodeRotation::Deg90,
                "a lane head points down"
            );
        }

        assert_no_fan_in(&l);
        assert_no_overlap(&m, &l);
    }

    /// The smallest body that both reads ONE value across two rows and earns
    /// a bus.
    ///
    /// The opening two reads are the shape the lane tests pin; everything
    /// after is bulk. It is there because a bus has to pay for itself — a body
    /// of a few gates keeps its direct wires by design — so a test about what
    /// a lane LOOKS like has to be handed a body that builds one.
    const TWO_ROW_BUS_SRC: &str = "var a: int = 1
var b: int = 2
var log: string[]
in go: exec
on go {
  PrintToConsole(\"${a}\")
  PrintToConsole(\"x${a}\")
  log.push(\"p${b}\")
  PrintToConsole(\"q${a}${b}\")
  b = a + b
  log.push(\"r${b}\")
}
";

    /// One statement consuming an expression built from TWO operand columns —
    /// the shape the mini-bus turns — on a body big enough to earn a bus.
    const MINI_BUS_SRC: &str = "var x: int = 0
var y: int = 1
var log: string[]
in t: exec
on t {
  x = (1 + x) * (2 + x)
  log.push(\"p${y}\")
  PrintToConsole(\"q${x}${y}\")
  y = y + 1
  log.push(\"r${y}\")
}
";

    /// A body dense enough to exercise the band: two variables read across
    /// six rows, some of them twice on one row.
    // The interpolations are deliberately all distinct: CSE merges two identical
    // pure FormatText gates into one, which would collapse this fixture's
    // per-value lanes (the `PrintToConsole("a=${a}")` differs from the earlier
    // `"${a}"` so both survive as separate travelling values).
    const BAND_SRC: &str = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  log.push(\"${a}\")\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${b}\")\n  PrintToConsole(\"a=${a}\")\n  b = a + b\n  log.push(\"y${b}\")\n}\n";

    /// The same band with no declared input port, so nothing stacks further
    /// left than the gutter and the band itself is the plane's left edge.
    const NO_PIN_BAND_SRC: &str = "var a: int = 1
var b: int = 2
var log: string[]
on ControllerJoined() -> (who) {
  log.push(\"${a}\")
  PrintToConsole(\"${a}${b}\")
  log.push(\"${b}\")
  PrintToConsole(\"a=${a}\")
  b = a + b
  log.push(\"y${b}\")
}
";

    /// The end-to-end reachability oracle: every consumer whose own wire the
    /// bus took over must still RECEIVE the value, by a path of bus wires
    /// from the port that produces it.
    ///
    /// This is the feature's worst failure mode and the only one with no
    /// symptom: a suppressed wire whose replacement path does not exist
    /// compiles, loads, and pastes cleanly, and the value simply never
    /// arrives. Nothing else here proves it — `emit_lane` leaves out a
    /// rerouter whose cell is contested and re-points the run at whatever
    /// precedes it, so which node a consumer ends up hanging off is not
    /// decidable by reading one statement. Only the walk settles it.
    ///
    /// The walk follows `bus.wires` and nothing else. The suppressed wire
    /// itself is never followed — that is the wire being replaced — so a bus
    /// that suppressed a consumer and wired it to nothing cannot pass.
    ///
    /// Returns the number of consumers proven reachable, so a caller can
    /// refuse a fixture that proves nothing.
    fn assert_suppressed_consumers_stay_reachable(m: &Module, l: &LayoutResult) -> usize {
        let mut out: HashMap<BusEnd, Vec<BusEnd>> = HashMap::default();
        for w in &l.bus.wires {
            out.entry(w.source).or_default().push(w.target);
        }
        for (node_id, port) in l.bus.suppressed.iter() {
            let target = PortRef {
                node_id: *node_id,
                port: *port,
            };
            // The module wire the bus replaced names the value's real
            // producer. Fan-in is illegal, so there is exactly one.
            let source = m
                .wires
                .iter()
                .find(|w| w.target == target)
                .unwrap_or_else(|| panic!("suppressed {target:?} replaced no module wire"))
                .source;

            let mut seen: HashSet<BusEnd> = HashSet::default();
            let mut queue: VecDeque<BusEnd> = VecDeque::new();
            queue.push_back(BusEnd::Node(source));
            let mut reached = false;
            while let Some(cur) = queue.pop_front() {
                if cur == BusEnd::Node(target) {
                    reached = true;
                    break;
                }
                if !seen.insert(cur) {
                    continue;
                }
                for &next in out.get(&cur).into_iter().flatten() {
                    queue.push_back(next);
                }
            }
            assert!(
                reached,
                "{} .{} lost its value: its wire from {} .{} is suppressed and no \
                 path of bus wires reaches it",
                target.node_id,
                target.port.as_str(),
                source.node_id,
                source.port.as_str()
            );
        }
        l.bus.suppressed.len()
    }

    /// Every oracle above, at EVERY level of the chip tree.
    ///
    /// The single-level spellings all take a `recurse = false` layout, whose
    /// `chip_layouts` is empty by construction — so a chip's own interior bus
    /// has never been swept for overlap, fan-in, or a stranded consumer. Only
    /// the ROOT's deliveries into chips were, and those are a different set of
    /// wires built by a different call. That is exactly the surface the last
    /// real defect lived on: three independent filters were silently dropping
    /// foreign endpoints, and the outer anonymous chip's bus went from 0 nodes
    /// to 9 once they were fixed — a whole band that no assertion had ever
    /// looked at.
    ///
    /// Walks `(module, layout)` in lockstep: `lr.chip_layouts` is keyed by chip
    /// node id and `module.chips` holds the child module under the same key, so
    /// a layout with no module is itself a failure. Returns the total consumers
    /// proven reachable across the whole tree.
    fn assert_layout_tree_is_sound(module: &Module, lr: &LayoutResult) -> usize {
        assert_no_overlap(module, lr);
        assert_no_fan_in(lr);
        let mut proven = assert_suppressed_consumers_stay_reachable(module, lr);
        // Deterministic order, so a failure names the same chip every run.
        let mut ids: Vec<NodeId> = lr.chip_layouts.keys().copied().collect();
        ids.sort();
        for id in ids {
            let child = module
                .chips
                .get(&id)
                .unwrap_or_else(|| panic!("chip layout {id} has no child module"));
            proven += assert_layout_tree_is_sound(child, &lr.chip_layouts[&id]);
        }
        proven
    }

    /// A big root and a tiny chip, so one tree carries both decisions.
    const MIXED_BUS_SRC: &str = "var a: int = 1
var b: int = 2
var log: string[]
in go: exec
chip Tiny(t: exec) -> (n: int) {
  var h: int = 0
  on t { h = h + 1 }
  out n = h
}
let tiny = Tiny(go)
on go {
  log.push(\"${a}\")
  PrintToConsole(\"${a}${b}\")
  log.push(\"${b}\")
  PrintToConsole(\"${a}\")
  b = a + b
  log.push(\"y${b}\")
  PrintToConsole(\"${tiny.n}\")
}
";

    /// A bus has to earn its bricks. On a body of a few gates the lanes cost
    /// more rerouters than the gates they serve, and the plane reads worse for
    /// having them — so a module that small keeps its direct wires.
    ///
    /// The decision is per module and ALL-OR-NOTHING: a module that drops its
    /// bus must be indistinguishable from one built before the feature
    /// existed, which means no nodes, no wires and — the part that would
    /// silently break things — no suppressed entries either. A module left
    /// holding suppression it has no lanes to honour would drop every one of
    /// those wires at emit and strand the consumers.
    #[test]
    fn a_small_chip_drops_its_bus_while_a_big_module_keeps_one() {
        let m = lowered(MIXED_BUS_SRC);
        let l = layout_code(&m, &code_opts(), true);

        assert!(
            !l.bus.nodes.is_empty(),
            "the root body is big enough to earn its bus"
        );
        assert!(
            !l.bus.suppressed.is_empty(),
            "...and to suppress the wires it replaced"
        );

        let (chip_id, chip) = l
            .chip_layouts
            .iter()
            .next()
            .expect("the fixture must lay out its chip");
        let chip_module = &m.chips[chip_id];
        let gates = chip_module.nodes.values().filter(|n| is_spawnable(n)).count();
        assert!(
            gates < 12,
            "fixture's chip must stay small, got {gates} spawnable gates"
        );
        assert!(
            chip.bus.is_empty(),
            "a {gates}-gate chip must build no bus at all, got {} nodes / {} \
             wires / {} suppressed",
            chip.bus.nodes.len(),
            chip.bus.wires.len(),
            chip.bus.suppressed.len()
        );

        // The mix is the new case: one tree, one module bussed and one not.
        assert_layout_tree_is_sound(&m, &l);
    }

    /// Chips in both directions and two levels deep: the outer chip takes a
    /// value in and hands one back, and the chip nested inside it does the
    /// same again — so every level owns a bus that carries a foreign endpoint.
    const NESTED_CHIP_SRC: &str = "var score: int = 0
var log: string[]
in go: exec
on go {
  score = 0
  chip {
    var acc: int = 0
    acc = score + 1
    log.push(\"o${acc}\")
    chip {
      var inner: int = 0
      inner = acc + score
      log.push(\"i${inner}\")
      log.push(\"j${inner}${acc}\")
      score = inner + acc
    }
    log.push(\"p${acc}${score}\")
  }
  PrintToConsole(\"${score}\")
  PrintToConsole(\"x${score}\")
}
";

    /// The whole tree, swept. Without the recursion this passes on an empty
    /// `chip_layouts` and proves nothing about any chip's own band.
    #[test]
    fn every_level_of_the_chip_tree_holds_the_bus_invariants() {
        let m = lowered(NESTED_CHIP_SRC);
        let l = layout_code(&m, &code_opts(), true);

        // The fixture has to actually reach two levels, or the recursion is
        // exercised against nothing.
        fn depth(lr: &LayoutResult) -> usize {
            1 + lr.chip_layouts.values().map(depth).max().unwrap_or(0)
        }
        fn bus_nodes(lr: &LayoutResult) -> usize {
            lr.bus.nodes.len() + lr.chip_layouts.values().map(bus_nodes).sum::<usize>()
        }
        assert!(
            depth(&l) >= 3,
            "fixture must nest two levels of chip under the root, got depth {}",
            depth(&l)
        );
        assert!(
            !l.chip_layouts.is_empty(),
            "fixture must lay out its chips recursively"
        );

        // Every chip interior builds a band of its own, so the recursion has
        // something to sweep at each level.
        let mut with_a_bus = 0usize;
        let mut stack: Vec<&LayoutResult> = l.chip_layouts.values().collect();
        while let Some(child) = stack.pop() {
            if !child.bus.nodes.is_empty() {
                with_a_bus += 1;
            }
            stack.extend(child.chip_layouts.values());
        }
        assert!(
            with_a_bus >= 2,
            "both nesting levels must build a bus of their own, got {with_a_bus}"
        );
        assert!(
            bus_nodes(&l) > l.bus.nodes.len(),
            "the chips' own bands must be part of what is swept"
        );

        let proven = assert_layout_tree_is_sound(&m, &l);
        assert!(
            proven > 8,
            "the tree must suppress a band's worth of wires, got {proven}"
        );
    }

    #[test]
    fn every_suppressed_consumer_still_receives_its_value() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let proven = assert_suppressed_consumers_stay_reachable(&m, &l);
        assert!(
            proven > 8,
            "fixture must suppress a band's worth of wires, got {proven}"
        );
    }

    /// The same oracle over the shapes whose replacement path is least
    /// obvious: chip deliveries and exits, whose tap anchors on a chip brick
    /// while the wire's own endpoint lives in another module.
    ///
    /// Laid with `recurse = true` and walked through the tree, so the chips'
    /// OWN bands are held to it too — the root's deliveries into a chip and
    /// the band a chip builds for its interior are separate sets of wires from
    /// separate calls, and only the walk covers the second.
    #[test]
    fn chip_crossings_stay_reachable_through_the_bus() {
        let src = "var score: int = 0\nvar log: string[]\nin go: exec\nchip Scorer(run: exec, amount: int) -> (total: int) {\n  on run {\n    score = score + amount\n    log.push(\"s\")\n  }\n  out total = score\n}\nlet scored = Scorer(go, 5)\non go {\n  score = 0\n  chip {\n    score = score + 1\n    log.push(\"i\")\n  }\n  PrintToConsole(\"${scored.total}\")\n  PrintToConsole(\"x${scored.total}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &code_opts(), true);
        assert!(
            !l.chip_layouts.is_empty(),
            "fixture must lay out its chips' interiors"
        );
        let proven = assert_layout_tree_is_sound(&m, &l);
        assert!(proven > 8, "fixture must cross chip walls, got {proven}");
        // The crossings themselves: wires whose target sits in another
        // module. These are the ones the oracle exists for.
        let crossings = l
            .bus
            .suppressed
            .iter()
            .filter(|(id, _)| !m.nodes.contains_key(id))
            .count();
        assert!(crossings > 0, "fixture must suppress a chip delivery");
    }

    /// Every consumer is fed from a gate-side rerouter standing in its OWN
    /// column — one tap reserve left of the gate it drives — unless another
    /// lane's rerouter already stands in that exact cell.
    ///
    /// A run that starts further back along the row crosses the gates in
    /// between, which is the diagonal the band exists to remove. Before
    /// `TAP_RESERVE` the cell mostly did not exist: a row packed its columns
    /// flush, so the gate one column left held it and the consumer fell back
    /// to the band tap out in the gutter.
    ///
    /// The case the reserve cannot cover is a gate consuming two bussed
    /// values at once: both lanes want the same single cell and one has to
    /// read from further back. That exemption is spelled out as "somebody
    /// else is standing exactly there", so an EMPTY cell — the shape the
    /// reserve fixes — still fails.
    #[test]
    fn every_consumer_column_gets_its_own_gate_side_tap() {
        let m = lowered(BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        let mut checked = 0usize;
        let mut shared = 0usize;
        for w in &l.bus.wires {
            let BusEnd::Node(target) = w.target else {
                continue;
            };
            // Foreign endpoints hold no placement in this module.
            let Some(p) = l.placements.get(&target.node_id) else {
                continue;
            };
            let BusEnd::Bus(i) = w.source else {
                panic!("a consumer is driven by a raw port, not by a tap");
            };
            let driver = &l.bus.nodes[i];
            // A mini-bus corner stands in its OPERAND's column, not one tap
            // reserve left of the gate it feeds — it is turning a wire, not
            // marching along a row. `the_mini_bus_drops_an_expression_value_
            // into_its_statement` is what holds it to its own geometry.
            if driver.role != BusRole::Gutter {
                continue;
            }
            let own_column = p.y - TAP_RESERVE;
            checked += 1;
            if driver.y == own_column {
                continue;
            }
            let holder = l
                .bus
                .nodes
                .iter()
                .any(|n| n.z == driver.z && n.y == own_column && n.x == driver.x);
            assert!(
                holder,
                "{} .{} is fed from y={}, and the cell in its own column at \
                 y={own_column} stands empty",
                target.node_id,
                target.port.as_str(),
                driver.y
            );
            shared += 1;
        }
        assert!(checked > 8, "fixture must tap a band's worth of gates");
        assert!(
            shared * 4 < checked,
            "the reserve must cover the great majority of columns; \
             {shared} of {checked} fell back"
        );
    }

    /// The bus's own bricks against the body's, measured at the rerouter's
    /// real footprint rather than the 5×5 gate default. A tap stands INSIDE
    /// the body's rows, so a mis-sized bus brick lands in a gate and the game
    /// silently drops one of the two.
    #[test]
    fn bus_nodes_never_overlap_gates_or_each_other() {
        let src = "var a: int = 1\nvar b: int = 2\nvar log: string[]\nin go: exec\non go {\n  PrintToConsole(\"${a}${b}\")\n  log.push(\"${a}\")\n  PrintToConsole(\"${b}\")\n  log.push(\"${a}${b}\")\n}\n";
        let m = lowered(src);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(!l.bus.nodes.is_empty(), "fixture must build a bus");
        assert_no_overlap(&m, &l);
    }

    /// A page's grid is sized from the layout bounds and centred on the
    /// origin, so a brick outside the plane extent is a brick outside the
    /// emitted microchip. The band hangs off the body's left edge — the side
    /// nothing else reaches — so it is the part most likely to fall out.
    #[test]
    fn bus_nodes_sit_inside_the_plane_extent() {
        // A whole band, not a single lane: the extent is derived from the
        // bounds with a 5-unit margin on each side, so a one-lane band is
        // narrow enough to hide inside the margin even when the bounds have
        // forgotten it entirely.
        let m = lowered(NO_PIN_BAND_SRC);
        let l = layout_code(&m, &LayoutOptions::default(), false);
        assert!(l.bus.nodes.len() > 8, "fixture must build a real band");
        let e = crate::layout::wall::plane_extent(&l);
        for (i, n) in l.bus.nodes.iter().enumerate() {
            assert!(
                n.x >= -e.x
                    && n.x + 2 * MEASURED_BUS_HALF <= e.x
                    && n.y >= -e.y
                    && n.y + 2 * MEASURED_BUS_HALF <= e.y,
                "bus node {i} at ({}, {}) escapes the extent ({}, {})",
                n.x,
                n.y,
                e.x,
                e.y
            );
            assert!(n.z <= e.z, "bus node {i} at z {} escapes {}", n.z, e.z);
        }
    }

    /// A row is `(page, sub-row offset)`, and `assemble_pages` restarts each
    /// band's offsets at zero — so two bands standing side by side on one
    /// page contribute their top rows under the SAME key, and one lane stop
    /// serves both. This pins that, deliberately.
    ///
    /// It is not a fault to route around. The gutter band sits left of the
    /// whole page, so a value delivered into the second band has to cross the
    /// first one however the rows are keyed; keying rows per band would only
    /// start that crossing further left, at the gutter tap, instead of at the
    /// first band's own last tapped column. The merged key gives the shorter
    /// run, and the row's cells are still checked as one set, so a tap can
    /// never land inside a gate of either band.
    ///
    /// What it costs is a run that visibly crosses the gutter between bands.
    /// The assertions below are the shape of that run, plus the invariants
    /// that have to survive it.
    #[test]
    fn bands_sharing_a_page_share_their_row_keys() {
        let src = "var a: int = 1
var b: int = 2
var log: string[]
in go: exec
on go {
  PrintToConsole(\"${a}\")
  PrintToConsole(\"x${a}\")
  PrintToConsole(\"y${a}\")
  PrintToConsole(\"z${a}\")
  log.push(\"p${b}\")
  PrintToConsole(\"q${a}${b}\")
  b = b + 1
  log.push(\"r${b}\")
}
";
        let m = lowered(src);
        // Short enough to split the body into bands, and the default plane
        // budget is wide enough to keep them on one page.
        let budgets = CodeBudgets {
            band_height: 80,
            ..CodeBudgets::default()
        };
        let l = layout_code_with_budgets(&m, &LayoutOptions::default(), false, &budgets);
        let planes: BTreeSet<i32> = l.placements.values().map(|p| p.z).collect();
        assert_eq!(planes.len(), 1, "fixture must keep every band on one page");

        // Two bands' gates landing on one row read as a gap wider than the
        // gutter standing between the bands.
        let mut rows: HashMap<i32, Vec<i32>> = HashMap::default();
        for (id, p) in &l.placements {
            let (hsx, _) = measured_half_size(&m.nodes[id], &l);
            rows.entry(p.x + hsx * 2).or_default().push(p.y);
        }
        let mut split_rows = 0usize;
        for ys in rows.values_mut() {
            ys.sort_unstable();
            if ys.windows(2).any(|w| w[1] - w[0] > BAND_GUTTER) {
                split_rows += 1;
            }
        }
        assert!(
            split_rows > 0,
            "fixture must put two bands' gates under one row key"
        );

        // ...and one lane chains straight across that gap.
        let crossing = l
            .bus
            .wires
            .iter()
            .filter(|w| match (w.source, w.target) {
                (BusEnd::Bus(a), BusEnd::Bus(b)) => {
                    let (na, nb) = (&l.bus.nodes[a], &l.bus.nodes[b]);
                    na.rotation == NodeRotation::Deg0
                        && nb.rotation == NodeRotation::Deg0
                        && (na.y - nb.y).abs() > BAND_GUTTER
                }
                _ => false,
            })
            .count();
        assert!(crossing > 0, "the merged row must chain across the gutter");

        // What the merge must not cost: no brick landing inside another, no
        // port driven twice, and every consumer the bus took over still
        // receiving its value.
        assert_no_overlap(&m, &l);
        assert_no_fan_in(&l);
        assert_suppressed_consumers_stay_reachable(&m, &l);
    }

    /// A LITERAL living in a sibling chip must not earn a lane.
    ///
    /// The endpoint is foreign, so the wire's tap anchors on the chip brick —
    /// and a chip brick is always bussable, so checking the anchor's class
    /// says nothing about the endpoint's. Emit gives a literal no brick, so
    /// the lane's wire fails to resolve and aborts the whole build with
    /// `BusWireUnresolved`. This is the shape that hard-failed a 25-line
    /// program through the real compiler path.
    #[test]
    fn a_literal_inside_a_sibling_chip_is_never_bussed() {
        let src = "var o1: int = 0
var o2: int = 0
var o3: int = 0
in go: exec
chip { let k = 0 }
chip {
  let a = k + 1
  let b = k + 2
  let c = k + 3
  let d = k + 4
  let e = k + 5
  let f = k + 6
}
on go {
  o1 = a + b
  o2 = c + d
  o3 = e + f
}
";
        let m = lowered(src);
        let l = layout_code(&m, &code_opts(), true);

        // Resolve every bus endpoint through the chip tree, which is exactly
        // what the production check has to do. A same-module lookup returns
        // `None` for a foreign node and would pass vacuously.
        fn classes(m: &Module, out: &mut HashMap<NodeId, &'static str>) {
            for (id, n) in &m.nodes {
                out.insert(*id, n.gate_class);
            }
            for c in m.chips.values() {
                classes(c, out);
            }
        }
        let mut all: HashMap<NodeId, &'static str> = HashMap::default();
        classes(&m, &mut all);

        fn walk(m: &Module, l: &LayoutResult, all: &HashMap<NodeId, &'static str>) {
            for w in &l.bus.wires {
                for end in [w.source, w.target] {
                    if let BusEnd::Node(p) = end {
                        let cls = all.get(&p.node_id).copied();
                        assert!(
                            cls.is_some_and(|c| c != gate_class::LITERAL
                                && c != gate_class::UNSUPPORTED),
                            "a lane claims {} ({cls:?}), which emit gives no brick",
                            p.node_id
                        );
                    }
                }
            }
            for (id, c) in &m.chips {
                if let Some(cl) = l.chip_layouts.get(id) {
                    walk(c, cl, all);
                }
            }
        }
        walk(&m, &l, &all);
        assert_layout_tree_is_sound(&m, &l);
    }

    #[test]
    fn literal_sources_are_never_bussed() {
        let m =
            lowered("in go: exec\non go {\n  PrintToConsole(\"a\")\n  PrintToConsole(\"b\")\n}\n");
        let l = layout_code(&m, &LayoutOptions::default(), false);
        for w in &l.bus.wires {
            if let crate::layout::BusEnd::Node(p) = w.source {
                let cls = m.nodes.get(&p.node_id).map(|n| n.gate_class);
                assert_ne!(cls, Some(crate::ir::gate_class::LITERAL));
            }
        }
    }
