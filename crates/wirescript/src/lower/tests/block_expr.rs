use super::*;
use crate::ir::gate_class as gc;

const MUL: &str = "BrickComponentType_Expr_MathMultiply";
const ADD: &str = "BrickComponentType_Expr_MathAdd";

/// Block expressions lower to the gates of their tail expression and their
/// `let`s, in pure context as plain expression gates: exact counts of the
/// named classes over the whole module tree, and no exec gate.
#[test]
fn block_exprs_lower_to_their_expression_gates() {
    let rows: &[(&str, &str, &[(&str, usize)])] = &[
        (
            "pure block if with let: Select, never Branch",
            "var x: int = 5\nlet result = if x > 3 then { let a = x + 1; a } else { let b = x - 1; b }\nout y = result",
            &[(gc::SELECT, 1), (gc::BRANCH, 0), (ADD, 1)],
        ),
        (
            "scoped locals: the MathMultiply for x * 2 inside the block",
            "var x: int = 10\nlet result = if x > 5 then { let inner = x * 2; inner } else { 0 }\nout y = result",
            &[(MUL, 1), (gc::SELECT, 1)],
        ),
        (
            "block in a pure let",
            "var x: int = 10\nlet doubled = { let tmp = x + x; tmp }\nout y = doubled",
            &[(ADD, 1)],
        ),
        (
            "nested blocks",
            "var x: int = 0\nlet result = { let a = { let b = 1; b + 1 }; a + 1 }\nout y = result",
            &[(ADD, 2)],
        ),
        (
            "several lets",
            "let result = { let a = 1; let b = 2; a + b }\nout y = result",
            &[(ADD, 1)],
        ),
        (
            "block in the else arm only",
            "var x: int = 5\nlet result = if x > 0 then x else { let neg = 0 - x; neg }\nout y = result",
            &[(gc::SELECT, 1)],
        ),
        (
            "block as a function argument",
            "let y = sin({ let a = 3.14; a })\nout z = y",
            &[(gc::MATH_SIN, 1)],
        ),
        (
            "two blocks in a binop",
            "let y = { let a = 1; a } + { let b = 2; b }\nout z = y",
            &[(ADD, 1)],
        ),
        (
            "block as an if condition",
            "var x: int = 5\nlet y = if { let a = x + 1; a } > 10 then 1 else 0\nout z = y",
            &[(gc::SELECT, 1), (ADD, 1)],
        ),
        (
            "block with string interpolation",
            "let v = { let a = 1; a }\nlet s = \"val=${v}\"\nout z = s",
            &[(gc::STRING_FORMAT_TEXT, 1)],
        ),
        (
            "`then` and `else` on continuation lines",
            "var x: int = 5\nlet result = if x > 3\n    then \"big\"\n    else \"small\"\nout y = result",
            &[(gc::SELECT, 1), (gc::BRANCH, 0)],
        ),
        ("an empty block", "let x = { }", &[]),
        (
            "a handler-local var read inside a block",
            "on RoundStart() {\n  var temp: int = 5\n  let result = { let t = temp; t }\n}",
            &[],
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}\n{src}"));
            continue;
        }
        for &(class, n) in want {
            let got = count_class(&r.module, class);
            if got != n {
                failures.push(format!("{label}: {got} x {class}, want {n}\n{src}"));
            }
        }
        let pure = !src.contains("on RoundStart");
        if pure
            && let Some(n) = r
                .module
                .nodes_deep()
                .find(|n| n.gate_class.contains("Exec_"))
        {
            failures.push(format!(
                "{label}: pure block emitted {}\n{src}",
                n.gate_class
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A block's own `let inner = 99` hides the outer `inner` only inside the
/// block: `check` reads the outer 42.
#[test]
fn block_expr_scope_does_not_leak() {
    let r = compile_ok("let inner = 42\nlet result = { let inner = 99; inner }\nout check = inner");
    let out = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.kind == crate::ir::NodeKind::Output
                && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == "check")
        })
        .map(|(id, _)| *id)
        .expect("output `check`");
    let src =
        source_of(&r, out, crate::ir::port_registry::WirePort::RerInput).expect("`check` is wired");
    let values: Vec<_> = r.module.nodes[&src].properties.values().cloned().collect();
    assert!(
        values.contains(&crate::ir::Literal::Int(42)),
        "`check` must be driven by the outer 42, got {values:?}"
    );
    assert!(
        !values.contains(&crate::ir::Literal::Int(99)),
        "`check` must not see the block's 99, got {values:?}"
    );
}

#[test]
fn block_expr_with_chip_call() {
    let r = compile_ok(
        "chip Dbl(x: int) -> (result: int) { out result = x * 2 }\nlet y = { let d = Dbl(5); d.result }\nout z = y",
    );
    assert_eq!(
        r.module.chips.len(),
        1,
        "the Dbl call inside the block makes one chip instance"
    );
    assert_eq!(count_class(&r.module, MUL), 1);
}
