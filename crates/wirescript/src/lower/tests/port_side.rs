use super::*;

#[test]
fn side_lands_on_root_io_nodes() {
    let r = compile_ok("@left in a: exec\n@right out b = 1");
    let side_of = |kind: crate::ir::NodeKind| {
        r.module
            .nodes
            .values()
            .find(|n| n.kind == kind)
            .and_then(|n| n.properties.get(&*crate::intern::sym::REROUTE_SIDE))
            .cloned()
    };
    assert_eq!(
        side_of(crate::ir::NodeKind::Input),
        Some(crate::ir::Literal::String("left".into()))
    );
    assert_eq!(
        side_of(crate::ir::NodeKind::Output),
        Some(crate::ir::Literal::String("right".into()))
    );
}

#[test]
fn unannotated_ports_carry_no_side() {
    let r = compile_ok("in a: exec\nout b = 1");
    assert!(r.module.nodes.values().all(|n| {
        n.properties
            .get(&*crate::intern::sym::REROUTE_SIDE)
            .is_none()
    }));
}

/// A side annotation on a port inside a chip is WS023.
#[test]
fn side_inside_a_chip_is_ws023() {
    let rows = [
        ("`in` inside chip {}", "chip { @left in a: exec }"),
        (
            "`in` inside a named chip",
            "chip F(x: int) -> (r: int) {\n  @left in a: exec\n  out r = x\n}\nout result = F(1).r",
        ),
        ("`out` inside chip {}", "chip { @bottom out done: exec }"),
    ];
    let mut failures = Vec::new();
    for (label, src) in rows {
        let r = compile(src);
        if !r.diagnostics.iter().any(|d| d.code == "WS023") {
            failures.push(format!(
                "{label}: expected WS023, got {:?}",
                crate::test_support::codes(&r.diagnostics)
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
