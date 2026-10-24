//! `value as T`: one Convert gate per scalar that changes type, none for a
//! field already of its target type, and none for a conversion nothing reads.

use wirescript::compile::{FoldMode, compile};

use crate::common;

const CONVERT: &str = "BrickComponentType_Expr_Convert";

fn converts(src: &str) -> usize {
    let (_, tc, lowered) = common::run_stages(src, "as_convert.ws", FoldMode::Auto);
    assert!(
        !tc.diagnostics
            .iter()
            .any(|d| matches!(d.severity, wirescript::diagnostic::Severity::Error)),
        "{:?}",
        tc.diagnostics
    );
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let mut n = 0;
    lowered
        .module
        .walk(&mut |_, m| n += m.nodes.values().filter(|x| x.gate_class == CONVERT).count());
    n
}

#[test]
fn a_tuple_converts_only_the_elements_that_change() {
    let used = "in a: int\nin b: int\nlet t = (a, b) as (bool, int)\nout x: bool = t.0\nout y: int = t.1\n";
    assert_eq!(converts(used), 1);
    let unused = "in a: int\nin b: int\nlet t = (a, b) as (bool, int)\nout y: int = b\n";
    assert_eq!(converts(unused), 0);
}

#[test]
fn a_record_converts_field_by_field() {
    let src = "type P = { x: int, y: int }\ntype Q = { x: float, y: float }\n\
               in a: int\nin b: int\nvar store: Q = { x: 0.0, y: 0.0 }\nin go: exec\n\
               on go { let p: P = { x: a, y: b }\n store = p as Q }\n";
    assert_eq!(converts(src), 2);
}

#[test]
fn a_value_already_of_the_type_is_not_converted() {
    assert_eq!(converts("in a: int\nout y = a as int\n"), 0);
    assert_eq!(converts("in a: int\nout y = a as float + 0.5\n"), 1);
}

/// The gate's `TargetType` setting holds a value of the type converted to:
/// that variant is what picks the output.
#[test]
fn the_target_type_is_baked_as_a_value_of_that_type() {
    let brz = compile(common::input(
        "in a: int\nout s = a as string\n",
        "as_convert.ws",
    ))
    .expect("should compile")
    .brz;
    let target = common::world_components(&brz)
        .iter()
        .find_map(|c| c.get("TargetType").map(|v| format!("{v:?}")))
        .expect("a Convert gate");
    assert!(
        target.contains("Str"),
        "TargetType is a string value: {target}"
    );
}

#[test]
fn a_conversion_as_cannot_make_warns_and_keeps_the_type() {
    let codes =
        common::diag_codes("var xs: int[]\nin go: exec\non go { let ys = xs as float[] }\n");
    assert_eq!(codes, vec!["WS109".to_string()]);
}

/// A constant whose conversion was measured in game is worked out at compile
/// time: usable as a `const`, and no gate where it is wired.
#[test]
fn a_measured_constant_conversion_folds() {
    assert_eq!(converts("const half = 3 as float\nout h = half / 2.0\n"), 0);
    assert_eq!(converts("const n = -2.7 as int\nout k = n + 1\n"), 0);
    assert_eq!(converts("out s = 42 as string\n"), 0);
    // The game formats a float as a string itself (1234.5678 reads 1234.57), so it keeps its gate.
    assert_eq!(converts("out s = 2.5 as string\n"), 1);
}

#[test]
fn a_composite_has_no_number_form() {
    let codes = common::diag_codes("in v: vector\nout f = v as float\n");
    assert_eq!(codes, vec!["WS109".to_string()]);
}

/// Two conversions of one value to different types are two gates: the target
/// type lives on the gate's output port, which merging identical gates must
/// count. Merged, `x as string` printed `x as int`'s result in game.
#[test]
fn conversions_to_different_types_are_not_merged() {
    assert_eq!(
        converts("in x: float\nout i = x as int\nout s = x as string\n"),
        2
    );
    assert_eq!(
        converts("in x: float\nout a = x as int\nout b = x as int\n"),
        1
    );
}
