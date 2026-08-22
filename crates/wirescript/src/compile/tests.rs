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
