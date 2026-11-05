use super::*;
use crate::compile::{CompileInput, FoldMode, compile, diagnostics_only, front_end};
use crate::diagnostic::Severity;
use crate::resolve::{FsLoader, MemLoader, resolve};

/// `e` as source-like text, a bound operand shown as `$a` or `$b`.
fn show_expr(e: &Expr) -> String {
    match e {
        Expr::Ident { name, .. } if is_testing_name(name) => {
            format!("${}", &name[name.len() - 1..])
        }
        Expr::Ident { name, .. } => name.clone(),
        Expr::IntLit { text, .. } | Expr::FloatLit { text, .. } => text.clone(),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::StringLit { value, .. } => format!("{value:?}"),
        Expr::FieldAccess { obj, field, .. } => format!("{}.{field}", show_expr(obj)),
        Expr::IndexAccess { obj, index, .. } => {
            format!("{}[{}]", show_expr(obj), show_expr(index))
        }
        Expr::BinOp {
            op, left, right, ..
        } => format!("({} {op} {})", show_expr(left), show_expr(right)),
        Expr::UnOp { op, operand, .. } => format!("{op}{}", show_expr(operand)),
        Expr::Call { callee, args, .. } => {
            let args: Vec<String> = args
                .iter()
                .map(|a| match a {
                    CallArg::Positional(x) => show_expr(x),
                    CallArg::Named { name, value, .. } => format!("{name} = {}", show_expr(value)),
                    CallArg::Spread(x) => format!("...{}", show_expr(x)),
                })
                .collect();
            format!("{}({})", show_expr(callee), args.join(", "))
        }
        Expr::InterpLit { parts, .. } => {
            let text: String = parts
                .iter()
                .map(|p| match p {
                    InterpPart::Lit(t) => t.clone(),
                    InterpPart::Expr(x) => format!("${{{}}}", show_expr(x)),
                })
                .collect();
            format!("{text:?}")
        }
        Expr::RecordLit { .. } => "{..}".to_string(),
        other => format!("<{:?}>", std::mem::discriminant(other)),
    }
}

fn show_stmts(stmts: &[Stmt]) -> String {
    stmts.iter().map(show_stmt).collect::<Vec<_>>().join("; ")
}

fn show_stmt(s: &Stmt) -> String {
    match s {
        Stmt::If(i) => format!(
            "if {} {{ {} }}",
            show_expr(&i.cond),
            show_stmts(&i.then_block.stmts)
        ),
        Stmt::Let(l) => {
            let LetBinding::Ident { name, .. } = &l.binding else {
                return "let ?".to_string();
            };
            let shown = show_expr(&Expr::Ident {
                name: name.clone(),
                range: l.range.clone(),
            });
            format!("let {shown} = {}", show_expr(&l.value))
        }
        Stmt::ExprStmt(es) => show_expr(&es.expr),
        other => format!("<{:?}>", std::mem::discriminant(other)),
    }
}

/// The `if` each `testing.*` statement of `ast`'s first handler became, as
/// text, in order.
fn shapes(ast: &Script) -> Vec<String> {
    let body = ast
        .decls
        .iter()
        .find_map(|d| match d {
            TopDecl::Handler(h) => Some(&h.body),
            _ => None,
        })
        .expect("a handler");
    body.stmts
        .iter()
        .filter(|s| matches!(s, Stmt::If(_)))
        .map(show_stmt)
        .collect()
}

fn in_handler(body: &str) -> String {
    format!(
        "type P = {{ x: int, q: {{ s: string }} }}\nvar xs: int[]\nvar n: int = 0\non Loaded() {{\n{body}\n}}\n"
    )
}

#[test]
fn each_untyped_form_becomes_an_if_true_with_its_check() {
    let src = in_handler(
        "  let p: P = { x: 1, q: { s: \"a\" } }\n  testing.fail()\n  testing.log(\"n is\", n)\n  testing.assert(n > 0)\n  testing.assert(n > 0, \"n\", n)\n  testing.assertEq(xs.length(), 2, \"len\")\n  testing.expect(n).not.toBe(1)\n  testing.expect(n).toBeGreaterThan(1)\n  testing.expect(n).toBeLessThan(1)\n  testing.expect(1.5).toBeCloseTo(n, digits = 1)\n  testing.expect(1.5).not.toBeCloseTo(n)",
    );
    let r = resolve(&src, "t.ws", &FsLoader);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let line = |n: usize| format!("t.ws:{n}: ");
    let want = [
        "if true { BroadcastChatMessage(\"FAIL t.ws:6\") }".to_string(),
        "if true { BroadcastChatMessage(\"LOG t.ws:7: n is ${n}\") }".to_string(),
        format!(
            "if true {{ let $a = (n > 0); if !$a {{ BroadcastChatMessage(\"FAIL {}assert failed: n > 0\") }} }}",
            line(8)
        ),
        format!(
            "if true {{ let $a = (n > 0); if !$a {{ BroadcastChatMessage(\"FAIL {}n ${{n}}\") }} }}",
            line(9)
        ),
        format!(
            "if true {{ let $a = xs.length(); let $b = 2; if ($a != $b) {{ BroadcastChatMessage(\"FAIL {}xs.length() expected ${{$b}}, got ${{$a}} (len)\") }} }}",
            line(10)
        ),
        format!(
            "if true {{ let $a = n; let $b = 1; if ($a == $b) {{ BroadcastChatMessage(\"FAIL {}n expected not ${{$b}}, got ${{$a}}\") }} }}",
            line(11)
        ),
        format!(
            "if true {{ let $a = n; let $b = 1; if !($a > $b) {{ BroadcastChatMessage(\"FAIL {}n expected greater than ${{$b}}, got ${{$a}}\") }} }}",
            line(12)
        ),
        format!(
            "if true {{ let $a = n; let $b = 1; if !($a < $b) {{ BroadcastChatMessage(\"FAIL {}n expected less than ${{$b}}, got ${{$a}}\") }} }}",
            line(13)
        ),
        format!(
            "if true {{ let $a = 1.5; let $b = n; if !(abs(($a - $b)) < 0.05) {{ BroadcastChatMessage(\"FAIL {}1.5 expected close to ${{$b}}, got ${{$a}}\") }} }}",
            line(14)
        ),
        format!(
            "if true {{ let $a = 1.5; let $b = n; if (abs(($a - $b)) < 0.005) {{ BroadcastChatMessage(\"FAIL {}1.5 expected not close to ${{$b}}, got ${{$a}}\") }} }}",
            line(15)
        ),
    ];
    assert_eq!(shapes(&r.ast), want);
    assert_eq!(r.testing_stmts.len(), want.len());
    assert!(r.testing_pending.is_empty());
}

#[test]
fn typed_matchers_wait_for_a_type_check_then_expand() {
    let src = in_handler(
        "  let p: P = { x: 1, q: { s: \"a\" } }\n  testing.expect(p).toEqual({ x: 2, q: { s: \"b\" } })\n  testing.expect(p).not.toEqual(p)\n  testing.expect(n).toEqual(3)\n  testing.expect(\"abc\").toContain(\"b\")\n  testing.expect(xs).not.toContain(n)",
    );
    let pending = resolve(&src, "t.ws", &FsLoader);
    assert_eq!(pending.testing_pending.len(), 5);
    assert_eq!(
        shapes(&pending.ast)[0],
        "if true { let $a = p; let $b = {..} }"
    );

    let fe = front_end(&src, "t.ws", None, FoldMode::Auto, &FsLoader);
    let errors: Vec<_> = fe
        .resolved
        .diagnostics
        .iter()
        .chain(&fe.typecheck.diagnostics)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let got = shapes(&fe.resolved.ast);
    let head = "if true { let $a = p; let $b = {..}; ";
    assert_eq!(
        got[0],
        format!(
            "{head}if ($a.x != $b.x) {{ BroadcastChatMessage(\"FAIL t.ws:6: p.x expected ${{$b.x}}, got ${{$a.x}}\") }}; if ($a.q.s != $b.q.s) {{ BroadcastChatMessage(\"FAIL t.ws:6: p.q.s expected ${{$b.q.s}}, got ${{$a.q.s}}\") }} }}"
        )
    );
    assert_eq!(
        got[1],
        "if true { let $a = p; let $b = p; if (($a.x == $b.x) && ($a.q.s == $b.q.s)) { BroadcastChatMessage(\"FAIL t.ws:7: p expected not to equal p\") } }"
    );
    assert_eq!(
        got[2],
        "if true { let $a = n; let $b = 3; if ($a != $b) { BroadcastChatMessage(\"FAIL t.ws:8: n expected ${$b}, got ${$a}\") } }"
    );
    assert_eq!(
        got[3],
        "if true { let $a = \"abc\"; let $b = \"b\"; if !$a.Contains($b, true) { BroadcastChatMessage(\"FAIL t.ws:9: \\\"abc\\\" expected to contain ${$b}, got ${$a}\") } }"
    );
    assert_eq!(
        got[4],
        "if true { let $a = xs; let $b = n; if $a.find($b).Found { BroadcastChatMessage(\"FAIL t.ws:10: xs expected not to contain ${$b}\") } }"
    );
}

/// The file name in a message, not its path.
#[test]
fn messages_name_the_file_not_its_path() {
    let src = "on Loaded() {\n  testing.fail(\"x\")\n}\n";
    let r = resolve(src, "C:/some/dir/case.ws", &FsLoader);
    assert_eq!(
        shapes(&r.ast),
        ["if true { BroadcastChatMessage(\"FAIL case.ws:2: x\") }"]
    );
}

/// Every form, then the same program with those lines blanked out.
fn every_form() -> (String, String) {
    let body = "\
  var ys: int[]
  ys.push(4)
  let p: P = { x: 1, q: { s: \"a\" } }
  n = n + 1
  testing.fail(\"boom\", n)
  testing.log(\"n\", n)
  testing.assert(n == 1)
  testing.assert(n == 1, \"one\")
  testing.assertEq(ys.length(), 1, \"len\")
  testing.expect(ys[0]).toBe(4)
  testing.expect(ys[0]).not.toBe(5)
  testing.expect(p).toEqual(p)
  testing.expect(p).not.toEqual({ x: 2, q: { s: \"b\" } })
  testing.expect(n).toEqual(1)
  testing.expect(n).toBeGreaterThan(0)
  testing.expect(n).not.toBeLessThan(0)
  testing.expect(1.004).toBeCloseTo(1.0)
  testing.expect(1.5).not.toBeCloseTo(1.0, digits = 3)
  testing.expect(\"abc\").toContain(\"b\")
  testing.expect(ys).toContain(4)
  testing.expect(ys).not.toContain(xs.length())
  BroadcastChatMessage(\"done ${n}\")";
    let with = in_handler(body);
    let without = with
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("testing.") {
                " ".repeat(l.len())
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (with, without)
}

#[test]
fn a_normal_build_emits_what_the_program_without_them_emits() {
    let (with, without) = every_form();
    let build = |src: &str, test_build: bool| {
        compile(CompileInput::new(src, "t.ws").test_build(test_build))
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let normal = build(&with, false);
    assert!(normal.diagnostics.is_empty(), "{:?}", normal.diagnostics);
    assert!(
        normal.brz == build(&without, false).brz,
        "a normal build left code of a `testing.*` statement in the save"
    );
    let tests = build(&with, true);
    assert!(tests.diagnostics.is_empty(), "{:?}", tests.diagnostics);
    assert!(tests.brz != normal.brz, "a test build emitted no checks");
}

#[test]
fn ws126_reports_each_misuse() {
    let cases: &[(&str, &str)] = &[
        ("testing.foo(1)", "has no member `foo`"),
        ("testing.toBe(1)", "is a matcher of `testing.expect(a)`"),
        ("testing.expect(1)", "needs a matcher"),
        ("testing.expect(1).not", "needs a matcher"),
        ("testing.expect(1).toBeBig(2)", "has no matcher `toBeBig`"),
        (
            "testing.expect(1).not.not.toBe(2)",
            "is `testing.<member>(...)`",
        ),
        ("testing.fail", "is a call"),
        ("let v = testing.fail(\"x\")", "has no value"),
        (
            "BroadcastChatMessage(\"${testing.expect(1)}\")",
            "has no value",
        ),
        ("testing.assert()", "takes a condition"),
        ("testing.assertEq(n)", "takes two values"),
        ("testing.expect(1, 2).toBe(1)", "takes one value"),
        ("testing.expect(1).toBe(2, 3)", "takes one value"),
        ("testing.expect(1).toBe(b = 2)", "no argument named `b`"),
        (
            "testing.expect(n).toContain(1)",
            "needs a string or an array",
        ),
        ("testing.expect(r).toEqual(r)", "holds an array or map"),
    ];
    for (stmt, says) in cases {
        let src = format!(
            "type R = {{ a: int, xs: int[] }}\nvar n: int = 0\non Loaded() {{\n  var r: R\n  {stmt}\n}}\n"
        );
        let diags = diagnostics_only(CompileInput::new(&src, "t.ws"), &FsLoader);
        let ws126: Vec<_> = diags.iter().filter(|d| d.code == "WS126").collect();
        assert!(
            ws126.len() == 1 && ws126[0].message.contains(says),
            "{stmt}: {diags:?}"
        );
        assert_eq!(ws126[0].range.start.line, 5, "{stmt}");
        assert!(
            diags
                .iter()
                .all(|d| d.code == "WS126" || d.severity != Severity::Error),
            "{stmt}: {diags:?}"
        );
    }
}

#[test]
fn a_statement_outside_an_exec_context_is_ws126() {
    let diags = diagnostics_only(CompileInput::new("testing.log(\"x\")\n", "t.ws"), &FsLoader);
    assert!(
        diags.len() == 1 && diags[0].code == "WS126" && diags[0].message.contains("exec context"),
        "{diags:?}"
    );
}

#[test]
fn a_file_that_binds_testing_keeps_its_own() {
    let loader = MemLoader {
        files: [(
            "lib.ws".to_string(),
            "mod fail(n: int) { BroadcastChatMessage(\"lib ${n}\") }\n".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let src = "import * as testing from \"lib\"\non Loaded() {\n  testing.fail(1)\n}\n";
    let r = resolve(src, "main.ws", &loader);
    assert!(r.testing_stmts.is_empty());
    let diags = diagnostics_only(CompileInput::new(src, "main.ws"), &loader);
    assert!(diags.is_empty(), "{diags:?}");

    let own = "var testing: int = 0\non Loaded() {\n  testing = 2\n}\n";
    assert!(resolve(own, "t.ws", &FsLoader).testing_stmts.is_empty());
    let diags = diagnostics_only(CompileInput::new(own, "t.ws"), &FsLoader);
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn an_imported_file_is_rewritten_too() {
    let loader = MemLoader {
        files: [(
            "lib.ws".to_string(),
            "mod check(n: int) {\n  testing.assertEq(n, 1)\n}\n".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let src = "import { check } from \"lib\"\non Loaded() {\n  check(2)\n}\n";
    let r = resolve(src, "main.ws", &loader);
    assert_eq!(r.testing_stmts.len(), 1);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
}

#[test]
fn receivers_are_read_from_text() {
    assert_eq!(receiver_before_dot("  testing"), Some(Receiver::Testing));
    assert_eq!(receiver_before_dot("x.testing"), None);
    assert_eq!(receiver_before_dot("mytesting"), None);
    assert_eq!(
        receiver_before_dot("  testing.expect(f(a, (b)))"),
        Some(Receiver::Expect { negated: false })
    );
    assert_eq!(
        receiver_before_dot("testing.expect(a).not"),
        Some(Receiver::Expect { negated: true })
    );
    assert_eq!(receiver_before_dot("other.expect(a)"), None);
}

#[test]
fn only_rewritten_names_are_testing_names() {
    assert!(is_testing_name("__testing_12_a"));
    assert!(!is_testing_name("__testing_x_a"));
    assert!(!is_testing_name("__testing_12_c"));
    assert!(!is_testing_name("__testing__a"));
}
