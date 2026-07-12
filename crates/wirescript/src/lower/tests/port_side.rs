use super::*;

#[test]
fn side_lands_on_root_io_nodes() {
    let r = compile("@left in a: exec\n@right out b = 1");
    assert_no_errors(&r);
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
    let r = compile("in a: exec\nout b = 1");
    assert_no_errors(&r);
    assert!(r.module.nodes.values().all(|n| n
        .properties
        .get(&*crate::intern::sym::REROUTE_SIDE)
        .is_none()));
}

#[test]
fn side_inside_anon_chip_is_ws023() {
    let r = compile("chip { @left in a: exec }");
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS023"),
        "annotated `in` inside chip {{}} must be WS023; got {:?}",
        r.diagnostics
    );
}

#[test]
fn side_inside_named_chip_is_ws023() {
    let r = compile(
        "chip F(x: int) -> (r: int) {\n  @left in a: exec\n  out r = x\n}\nout result = F(1).r",
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS023"),
        "annotated `in` inside a named chip must be WS023; got {:?}",
        r.diagnostics
    );
}

#[test]
fn side_inside_anon_chip_out_is_ws023() {
    let r = compile("chip { @bottom out done: exec }");
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS023"),
        "annotated `out` inside chip {{}} must be WS023; got {:?}",
        r.diagnostics
    );
}
