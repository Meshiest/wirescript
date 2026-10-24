use super::*;
use crate::ir::gate_class as gc;

const MUL: &str = "BrickComponentType_Expr_MathMultiply";
const ADD: &str = "BrickComponentType_Expr_MathAdd";

/// Pure context lowers to expression gates only; exec context reads vars
/// through `Var_Get`, writes through `Var_Set`, and branches through `Branch`.
/// Each row gives exact counts of the named classes over the whole module
/// tree, and whether the program must hold no `Exec_*` gate at all.
#[test]
fn context_decides_the_gates_a_read_or_write_lowers_to() {
    let rows: &[(&str, &str, &[(&str, usize)], bool)] = &[
        (
            "pure chip body has no exec gates",
            "chip Foo(x: int) -> (result: int) { out result = x + 1 }\nlet v = Foo(10)\nout y = v.result",
            &[(ADD, 1)],
            true,
        ),
        (
            "pure let reads with no Var_Get",
            "var x: int = 5\nlet y = x + 1\nout z = y",
            &[(gc::VAR_GET, 0)],
            true,
        ),
        (
            "pure if expression is a Select, not a Branch",
            "var x: int = 1\nlet r = if x > 0 then 1 else 0\nout y = r",
            &[(gc::SELECT, 1), (gc::BRANCH, 0)],
            true,
        ),
        (
            "exec var read uses Var_Get",
            "var x: int = 5\nin player: character\non player { let y = x + 1 }",
            &[(gc::VAR_GET, 1)],
            false,
        ),
        (
            "exec var write uses Var_Set",
            "var x: int = 0\nin player: character\non player { x = 42 }",
            &[(gc::VAR_SET, 1)],
            false,
        ),
        (
            "exec if statement uses Branch",
            "var x: int = 0\nin player: character\non player { if x > 0 { x = 1 } }",
            &[(gc::BRANCH, 1)],
            false,
        ),
        (
            "exec array indexing uses ArrayVar_Get",
            "var arr: int[]\nin player: character\non player { arr.push(42); let v = arr[0] }",
            &[(gc::ARRAY_GET, 1)],
            false,
        ),
        (
            "pure ops in exec context still read through Var_Get",
            "var x: int = 5\nin player: character\non player { let y = x * 2 + 1 }",
            &[(MUL, 1), (ADD, 1), (gc::VAR_GET, 1)],
            false,
        ),
        (
            "an if expression in exec context is a Select fed by a Var_Get",
            "var x: int = 1\nin player: character\non player { let r = if x > 0 then 1 else 0 }",
            &[(gc::SELECT, 1), (gc::VAR_GET, 1)],
            false,
        ),
        (
            "mod with only value params has no exec gates",
            "mod add_pure(a: int, b: int) -> (result: int) { out result = a + b }\nlet r = add_pure(3, 4)\nout y = r",
            &[(ADD, 1)],
            true,
        ),
        (
            "mod with a ref param writes through one Var_Increment",
            "mod inc(x: *int) { x = x + 1 }\nvar v: int = 0\nin player: character\non player { inc(v) }",
            &[(gc::VAR_INCREMENT, 1), (gc::VAR_SET, 0)],
            false,
        ),
        (
            "block expression in pure context stays pure",
            "var x: int = 5\nlet r = { let a = x + 1; let b = a * 2; b }\nout y = r",
            &[(ADD, 1), (MUL, 1)],
            true,
        ),
        (
            "block expression in exec context reads through Var_Get",
            "var x: int = 5\nin player: character\non player { let r = { let a = x + 1; a } }",
            &[(gc::VAR_GET, 1)],
            false,
        ),
        (
            "var in exec context initializes through one Var_Set",
            "on RoundStart() {\n  var x: int = 5\n}",
            &[(gc::VAR_SET, 1)],
            false,
        ),
        (
            "static var in exec context emits no Var_Set",
            "on RoundStart() {\n  static var x: int = 5\n}",
            &[(gc::VAR_SET, 0)],
            false,
        ),
        // A pure top-level `let`/`out` after a handler reads its vars purely, never
        // spliced onto the handler's exec chain: inheriting `current_exec` from the
        // flush-handler-ends machinery tangles unrelated bindings, and through the
        // synthetic `let _on_expr = Change(v)` misplaces a trigger's own input read.
        (
            "pure top-level let after a handler reads purely",
            "var x: int = 0\nin t: exec\non t { x = 1 }\nlet y = x + 1\nout o = y\n",
            &[(gc::VAR_GET, 0)],
            false,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want, pure) in rows {
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
        if pure {
            let exec: Vec<_> = r
                .module
                .nodes_deep()
                .filter(|n| n.gate_class.contains("Exec_"))
                .map(|n| n.gate_class)
                .collect();
            if !exec.is_empty() {
                failures.push(format!("{label}: pure program emitted {exec:?}\n{src}"));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
