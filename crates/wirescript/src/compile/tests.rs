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
        assert!(r.is_ok(), "compile failed: {:?}", r.err().map(|e| e.to_string()));
        let events = seen.lock().unwrap();
        let max_total = events.iter().map(|(_, t, _)| *t).max().unwrap();
        assert_eq!(max_total, 6, "two nested prefabs -> total 4 + 2; events: {events:?}");
        // A per-prefab step fires during emit, so the bar advances past the four
        // fixed phases rather than stalling at 4/N.
        let max_step = events
            .iter()
            .filter(|(_, _, done)| !done)
            .map(|(s, _, _)| *s)
            .max()
            .unwrap();
        assert!(max_step > 4, "per-prefab steps must advance past 4; events: {events:?}");
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
        let dir = std::env::temp_dir().join(format!("ws_ignore_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("ignore_lib.ws");
        std::fs::write(
            &lib,
            "mod helper(x: *int) { x = x + 1 }\non ReadBrickGrid() { }\n",
        )
        .unwrap();

        let main = dir.join("ignore_main.ws");
        let file = main.to_string_lossy().to_string();
        let run = |src: &str| {
            std::fs::write(&main, src).unwrap();
            diagnostics_only(CompileInput {
                source: src,
                file: &file,
                module_name: None,
                fold_mode: FoldMode::Auto,
            })
        };

        // A named import leaves the module's `on` handler behind, which is WS014.
        let program = "import { helper } from \"ignore_lib\"\nvar n: int = 0\non ReadBrickGrid() { helper(n) }\n";
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

        let _ = std::fs::remove_file(&main);
        let _ = std::fs::remove_file(&lib);
    }
