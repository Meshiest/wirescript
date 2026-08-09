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
