use super::*;
use crate::resolve::{FsLoader, resolve};
use crate::typecheck::typecheck;

fn hints_for(source: &str) -> Vec<InlayHintInfo> {
    let resolved = resolve(source, "test", &FsLoader);
    let tc = typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    collect_inlay_hints(source, &resolved.ast, &tc.type_of_expr, "test")
}

/// An unannotated `let` gets a type hint naming its inferred type, at file
/// scope and inside a handler or chip. Each row: the first hint's label
/// must contain every `has` and no `lacks` string.
#[test]
fn an_unannotated_let_gets_its_inferred_type() {
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        ("an int literal", "let x = 42", &["int"], &[]),
        ("a bool literal", "let flag = true", &["bool"], &[]),
        ("a float literal", "let x = 3.14", &["float"], &[]),
        ("a string literal", "let s = \"hello\"", &["string"], &[]),
        ("an expression", "let x = 1 + 2", &["int"], &[]),
        (
            "inside a handler",
            "in start: exec\non start { let x = 42 }",
            &["int"],
            &[],
        ),
        (
            "inside a chip",
            "chip Foo(a: int) -> (r: int) {\n  let doubled = a + a\n  out r = doubled\n}",
            &["int"],
            &[],
        ),
        // A tuple shows tuple syntax, not record syntax.
        (
            "a tuple",
            "let pair = (42, true)",
            &["(", "int", "bool"],
            &["{"],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, has, lacks) in rows {
        let hints = hints_for(src);
        let Some(first) = hints.first() else {
            failures.push(format!("{label}: no hint"));
            continue;
        };
        if !has.iter().all(|s| first.label.contains(s))
            || lacks.iter().any(|s| first.label.contains(s))
        {
            failures.push(format!(
                "{label}: label {:?}, want {has:?} and none of {lacks:?}",
                first.label
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn an_annotated_binding_gets_no_type_hint() {
    let hints = hints_for("let x: int = 42");
    assert!(
        hints.is_empty(),
        "should not hint when type is annotated: {hints:?}"
    );
    let hints = hints_for("var x: int = 0");
    assert!(
        !hints.iter().any(|h| h.kind == InlayHintKind::Type),
        "should not hint var with annotation: {hints:?}"
    );
}

#[test]
fn let_inside_every_body_gets_a_hint() {
    let bodies = [
        "for i in 0..3 { let x = i + 1 }",
        "if let E.B(v) = s { let x = v + 1 }",
        "let n = match s { E.A => { let x = 1
 x }, E.B(v) => v }",
    ];
    for body in bodies {
        let src = format!(
            "enum E {{ A, B(int) }}
in start: exec
var s: E
on start {{
{body}
}}"
        );
        let hints = hints_for(&src);
        assert!(
            hints.iter().any(|h| h.line == 4 && h.label.contains("int")),
            "{body}: {hints:?}"
        );
    }
}

#[test]
fn multiple_lets_multiple_hints() {
    let src = "let a = 1\nlet b = true\nlet c = 3.14";
    let hints = hints_for(src);
    assert_eq!(
        hints.len(),
        3,
        "should produce 3 hints, got {}",
        hints.len()
    );
}

#[test]
fn hint_position_is_after_name() {
    let hints = hints_for("let x = 42");
    assert!(!hints.is_empty());
    // "let x": the hint sits right after the name, at col 5.
    assert_eq!((hints[0].line, hints[0].col), (0, 5));
}
