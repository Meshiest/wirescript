use super::*;

/// Compile recursion depth scales with program size (Tarjan SCC walk in
/// analyze_cycles, lowering/emit recursion), and real callers invoke
/// compile from small-stack threads — the LSP's compile command runs on a
/// tokio blocking thread (2 MiB). The entry points must be safe no matter
/// how small the caller's stack is. A stack overflow aborts the whole
/// process, so without the internal big-stack worker this test crashes
/// the test run rather than failing an assertion.
#[test]
fn compile_survives_small_caller_stack() {
    let mut src = String::from("in x: int\nlet a0 = x + 1\n");
    for i in 1..8000 {
        src.push_str(&format!("let a{i} = a{} + 1\n", i - 1));
    }
    src.push_str("out result = a7999\n");
    let out = std::thread::Builder::new()
        .stack_size(384 * 1024)
        .spawn(move || {
            compile(CompileInput {
                source: &src,
                file: "small_stack_test",
                module_name: None,
                fold_mode: FoldMode::Auto,
            })
            .map(|r| r.brz.len())
        })
        .expect("spawn small-stack caller")
        .join()
        .expect("small-stack compile panicked");
    assert!(
        out.is_ok(),
        "compile failed: {:?}",
        out.err().map(|e| e.to_string())
    );
}

/// The compile-progress total grows by one step per embedded prefab (each
/// `$./file` reference / inline `$```…``` ` block), so the bar reflects the
/// per-prefab sub-compiles instead of stalling on the emit phase.
#[test]
fn progress_total_counts_nested_prefabs() {
    let src = "in go: exec\non go {\n  \
                   let a = SpawnPrefab(prefab = $```\nvar n: int = 0\n```)\n  \
                   let b = SpawnPrefab(prefab = $```\nvar m: int = 0\n```)\n}";
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32, bool)>::new()));
    let cb: ProgressCallback = {
        let seen = seen.clone();
        std::sync::Arc::new(move |p: CompileProgress| {
            seen.lock().unwrap().push((p.step, p.total, p.done));
        })
    };
    let r = compile_with_progress(
        CompileInput {
            source: src,
            file: "prog_test.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
        cb,
    );
    assert!(
        r.is_ok(),
        "compile failed: {:?}",
        r.err().map(|e| e.to_string())
    );
    let events = seen.lock().unwrap();
    let max_total = events.iter().map(|(_, t, _)| *t).max().unwrap();
    assert_eq!(
        max_total, 6,
        "two nested prefabs -> total 4 + 2; events: {events:?}"
    );
    // A per-prefab step fires during emit, so the bar advances past the four
    // fixed phases rather than stalling at 4/N.
    let max_step = events
        .iter()
        .filter(|(_, _, done)| !done)
        .map(|(s, _, _)| *s)
        .max()
        .unwrap();
    assert!(
        max_step > 4,
        "per-prefab steps must advance past 4; events: {events:?}"
    );
}

/// `// ws-ignore-line` / `// ws-ignore-file` have to hold on the pipeline
/// the tools actually run, not just inside `resolve`: `wirescript-check`
/// and the editor's on-save pass both report through `diagnostics_only`,
/// which chains typecheck, lowering and cycle analysis onto resolve's own
/// diagnostics.
///
/// An ERROR is never suppressed. Hiding one would leave a build that fails
/// with nothing on screen explaining why, so the directive stops at
/// warnings.
#[test]
fn ws_ignore_applies_through_diagnostics_only_but_never_to_errors() {
    let loader = crate::resolve::MemLoader {
        files: [(
            "ignore_lib.ws".to_string(),
            "mod helper(x: *int) { x = x + 1 }\non ReadBrickGrid() { }\n".to_string(),
        )]
        .into_iter()
        .collect(),
    };
    let run = |src: &str| {
        diagnostics_only(
            CompileInput {
                source: src,
                file: "ignore_main",
                module_name: None,
                fold_mode: FoldMode::Auto,
            },
            &loader,
        )
    };

    // A named import leaves the module's `on` handler behind, which is WS014.
    let program =
        "import { helper } from \"ignore_lib\"\nvar n: int = 0\non ReadBrickGrid() { helper(n) }\n";
    let before = run(program);
    assert!(
        before.iter().any(|d| d.code == "WS014"),
        "control: the warning this test suppresses must be there first: {before:?}"
    );

    let annotated = "import { helper } from \"ignore_lib\" // ws-ignore-line:WS014\nvar n: int = 0\non ReadBrickGrid() { helper(n) }\n";
    let after = run(annotated);
    assert!(
        !after.iter().any(|d| d.code == "WS014"),
        "ws-ignore-line must reach the check pipeline: {after:?}"
    );

    // Same directive, whole file, over a program that also fails to type
    // check: the error survives.
    let with_error = "// ws-ignore-file\nimport { helper } from \"ignore_lib\"\nvar n: int = 0\nvar bad: int = \"not an int\"\non ReadBrickGrid() { helper(n) }\n";
    let errs = run(with_error);
    assert!(
        errs.iter().any(|d| matches!(d.severity, Severity::Error)),
        "ws-ignore-file must not swallow an error: {errs:?}"
    );
    assert!(
        !errs.iter().any(|d| matches!(d.severity, Severity::Warning)),
        "ws-ignore-file must still silence the warnings: {errs:?}"
    );
}

/// A failed compile prints where each error is, not only its message.
#[test]
fn a_compile_error_prints_its_location() {
    let err = compile(CompileInput {
        source: "var fo: float = 0.0\nin go: exec\non go { fo = \"s\" }\n",
        file: "loc",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .err()
    .expect("a string into a float var is an error");
    let text = err.to_string();
    assert!(
        text.contains("[WS003]") && text.contains("(loc:3:"),
        "{text}"
    );
}

/// An imported file's canonical Windows path prints without its verbatim
/// prefix.
#[test]
fn a_location_prints_a_plain_path() {
    use crate::diagnostic::display_path;
    assert_eq!(display_path(r"\\?\C:\Users\x\lib.ws"), r"C:\Users\x\lib.ws");
    assert_eq!(
        display_path(r"\\?\UNC\host\share\lib.ws"),
        r"\\host\share\lib.ws"
    );
    assert_eq!(display_path("lib.ws"), "lib.ws");
    let err = CompileError::HasErrors(vec![crate::diagnostic::Diagnostic {
        severity: Severity::Error,
        code: "WS002".into(),
        message: "m".into(),
        range: crate::diagnostic::SourceRange::new(
            r"\\?\C:\x\lib.ws",
            Default::default(),
            Default::default(),
        ),
    }]);
    assert!(err.to_string().contains(r"(C:\x\lib.ws:0:0)"), "{err}");
}

/// A placeholder (an expression the type check accepted that lowering
/// cannot build) in a program nothing else rejects is an error, from
/// `compile` and from `diagnostics_only` alike, naming what it stands for.
/// Beside another error it stays a warning.
#[test]
fn a_placeholder_in_a_program_with_no_other_error_is_an_error() {
    let lib = "mod twice(self: float) -> float { return self * 2.0 }\n\
                   pub mod pubt(v: float) -> float { return v.twice() }\n";
    let head = "var k: int = 3\nvar n: int = 0\nvar fl: float = 1.5\nvar fo: float = 0.0\n\
                    type P = { a: int }\nvar pv: P = { a: 1 }\nin go: exec\n";
    let cases = [
        (
            "a method nothing declares",
            "",
            "on go { n = k.nosuch() }",
            "`.nosuch()`",
        ),
        (
            "a record receiver",
            "",
            "on go { n = pv.nosuch() }",
            "`.nosuch()`",
        ),
        (
            "a namespace import",
            "import * as u from \"lt\"\n",
            "on go { fo = fl.twice() }",
            "`.twice()`",
        ),
        (
            "a renamed import",
            "import { twice as tw } from \"lt\"\n",
            "on go { fo = fl.twice() }",
            "`.twice()`",
        ),
    ];
    let loader = crate::resolve::MemLoader {
        files: [("lt.ws".to_string(), lib.to_string())]
            .into_iter()
            .collect(),
    };
    for (what, imports, body, named) in cases {
        let src = format!("{imports}{head}{body}");
        let input = CompileInput {
            source: &src,
            file: "main",
            module_name: None,
            fold_mode: FoldMode::Auto,
        };
        let errors = match compile_with_loader(input, Default::default(), &loader) {
            Err(CompileError::HasErrors(errors)) => errors,
            other => panic!("{what}: compiled: {:?}", other.map(|r| r.diagnostics)),
        };
        assert!(
            errors.iter().all(|d| d.code == "WSP001")
                && errors.iter().any(|d| d
                    .message
                    .starts_with("this expression is not supported by the compiler yet")
                    && d.message.contains(named)),
            "{what}: {errors:?}"
        );
    }

    let check = |src: &str| {
        diagnostics_only(
            CompileInput {
                source: src,
                file: "main",
                module_name: None,
                fold_mode: FoldMode::Auto,
            },
            &crate::resolve::FsLoader,
        )
    };
    let alone = check(&format!("{head}on go {{ n = k.nosuch() }}"));
    assert!(
        alone
            .iter()
            .any(|d| d.code == "WSP001" && matches!(d.severity, Severity::Error)),
        "diagnostics_only must report it as `compile` does: {alone:?}"
    );
    let beside = check(&format!(
        "{head}var bad: int = \"x\"\non go {{ n = k.nosuch() }}"
    ));
    assert!(
        beside
            .iter()
            .any(|d| d.code == "WSP001" && matches!(d.severity, Severity::Warning))
            && beside
                .iter()
                .any(|d| d.code != "WSP001" && matches!(d.severity, Severity::Error)),
        "beside another error the placeholder stays a warning: {beside:?}"
    );
}

/// Neither `ws-ignore` directive silences a placeholder promoted to an
/// error: the promotion comes first, and no directive silences an error.
#[test]
fn ws_ignore_does_not_silence_a_promoted_placeholder() {
    let head = "in go: exec\nin k: int\nvar n: int = 0\nout on1: int = n\n";
    for (what, src) in [
        (
            "ignore line",
            format!("{head}on go {{ n = k.nosuch() // ws-ignore-line:WSP001\n}}\n"),
        ),
        (
            "ignore file",
            format!("// ws-ignore-file\n{head}on go {{ n = k.nosuch() }}\n"),
        ),
        (
            "ignore file code",
            format!("// ws-ignore-file:WSP001\n{head}on go {{ n = k.nosuch() }}\n"),
        ),
    ] {
        let input = CompileInput {
            source: &src,
            file: "main",
            module_name: None,
            fold_mode: FoldMode::Auto,
        };
        match compile(input) {
            Err(CompileError::HasErrors(errors)) => {
                assert!(
                    errors.iter().any(|d| d.code == "WSP001"),
                    "{what}: {errors:?}"
                )
            }
            other => panic!("{what}: compiled: {:?}", other.map(|r| r.diagnostics)),
        }
        let diags = diagnostics_only(
            CompileInput {
                source: &src,
                file: "main",
                module_name: None,
                fold_mode: FoldMode::Auto,
            },
            &crate::resolve::FsLoader,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == "WSP001" && matches!(d.severity, Severity::Error)),
            "{what}: diagnostics_only: {diags:?}"
        );
    }
    // A warning on the line is still silenced.
    let src = format!("{head}on go {{ static var z: int = k // ws-ignore-line:WSP001\nn = z }}\n");
    let input = CompileInput {
        source: &src,
        file: "main",
        module_name: None,
        fold_mode: FoldMode::Auto,
    };
    let out = compile(input).expect("a silenced warning compiles");
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
}

/// A promoted placeholder says which expression it is and how to rewrite
/// it, and never suggests a `let` that would not help.
#[test]
fn a_promoted_placeholder_names_its_expression_and_the_fix() {
    let head = "in go: exec\nin a: int\nin k: int\nvar n: int = 0\nout on1: int = n\n";
    for (what, body, names, fix, no_let) in [
        (
            "unknown method",
            "on go { n = k.nosuch() }",
            "`.nosuch()`",
            "is not a method the compiler can build",
            true,
        ),
        (
            "undocumented method",
            "on go { var v = a.ToString()\nn = a }",
            "`.ToString()`",
            "lists for its type",
            true,
        ),
        (
            "tuple element read",
            "var ts: (int, int)[]\non go { ts.push((1, 2))\nn = ts[0].1 }",
            "`.1`",
            "Rewrite it with a form the language reference documents",
            true,
        ),
    ] {
        let src = format!("{head}{body}\n");
        let input = CompileInput {
            source: &src,
            file: "main",
            module_name: None,
            fold_mode: FoldMode::Auto,
        };
        let errors = match compile(input) {
            Err(CompileError::HasErrors(errors)) => errors,
            other => panic!("{what}: compiled: {:?}", other.map(|r| r.diagnostics)),
        };
        let m = errors
            .iter()
            .find(|d| d.code == "WSP001")
            .map(|d| d.message.clone())
            .unwrap_or_default();
        assert!(m.contains(names) && m.contains(fix), "{what}: {m}");
        assert!(
            !m.contains("this expression, which"),
            "{what} names the expression: {m}"
        );
        if no_let {
            assert!(!m.contains("`let`"), "{what}: a `let` does not help: {m}");
        }
    }
}

/// Each `@layout` annotation selects its engine, and only code layout
/// carries the source map, which nothing else reads.
#[test]
fn layout_annotation_maps_to_its_settings() {
    let settings = |src: &str| program_settings(&resolve(src, "test", &FsLoader), "test");
    let code = settings("@layout(\"code\")\n\nin a: int\nout b = a\n");
    assert_eq!(code.layout, LayoutMode::Code);
    assert!(code.source_map.is_some());
    assert!(!code.no_gate_labels);
    let cube = settings("@layout(\"cube\")\n\nin a: int\nout b = a\n");
    assert_eq!(cube.layout, LayoutMode::Cube);
    assert!(cube.no_gate_labels);
    assert!(cube.source_map.is_none());
    let dag = settings("in a: int\nout b = a\n");
    assert_eq!(dag.layout, LayoutMode::Dag);
    assert!(dag.source_map.is_none());
}

/// `--emit-ir main.wsir` names a file with no directory part; its prefab
/// paths are then relative to the current directory, like the source's.
#[test]
fn prefab_paths_for_a_bare_ir_file_name() {
    use crate::ir::Literal;
    let mut b = crate::ir::ProgramBuilder::new("main");
    let root = b.root();
    let g = b.gate(root, "Expr_LogicalAND").unwrap();
    b.set(
        g,
        "Prefab",
        Literal::PrefabRef {
            path: "./child.ws".into(),
        },
    )
    .unwrap();
    let mut program = b.build();
    prepare_program_for_text(
        &mut program,
        "main.ws",
        std::path::Path::new("main.wsir"),
        FoldMode::Auto,
    )
    .expect("a bare file name is in the current directory");
    let path =
        &program.module.nodes.values().next().unwrap().properties[&crate::intern::intern("Prefab")];
    assert_eq!(
        path,
        &Literal::PrefabRef {
            path: "child.ws".into()
        }
    );
}
