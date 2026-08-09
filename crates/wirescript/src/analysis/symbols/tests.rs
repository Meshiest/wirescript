    use super::*;
    use crate::resolve::{FsLoader, MemLoader, resolve};
    use crate::typecheck::typecheck;

    #[test]
    fn namespace_import_collects_qualified_members() {
        // `import * as u` must yield a `u` namespace symbol plus qualified
        // `u.<member>` symbols for the module's importable decls, so member
        // completion after `u.` can find them.
        let loader = MemLoader {
            files: [(
                "lib.ws".to_string(),
                "mod swap(a: int) {}\nlet PI = 3\ntype Pt = { x: int }".to_string(),
            )]
            .into_iter()
            .collect(),
        };
        let resolved = resolve("import * as u from \"lib\"", "main", &loader);
        let tc = typecheck(&resolved.ast, "main");
        let syms = collect_symbols(&resolved.ast, &tc.type_of_expr);
        assert!(
            syms.iter().any(|s| s.name == "u" && s.kind == "namespace"),
            "namespace alias symbol missing: {:?}",
            syms.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        for (m, k) in [("u.swap", "mod"), ("u.PI", "let"), ("u.Pt", "type")] {
            assert!(
                syms.iter().any(|s| s.name == m && s.kind == k),
                "namespace member {m} ({k}) missing"
            );
        }
    }

    /// The `exec` flag the LSP hover shows for the mod/chip named `name`.
    fn mod_exec(source: &str, name: &str) -> bool {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test");
        collect_symbols(&resolved.ast, &tc.type_of_expr)
            .into_iter()
            .find(|s| s.name == name && (s.kind == "mod" || s.kind == "chip"))
            .unwrap_or_else(|| panic!("no mod/chip symbol named {name}"))
            .exec
    }

    #[test]
    fn pure_return_expr_mod_is_pure() {
        // Regression: `return <expr>` was flagged exec unconditionally, so a
        // pure single-output mod (only comparisons + literals) read as exec.
        let src = "mod band(n: int) -> int {\n  return if n >= 22 then 4 else if n >= 11 then 1 else 0\n}";
        assert!(!mod_exec(src, "band"), "pure return-expr mod should be pure");
    }

    #[test]
    fn array_index_return_mod_is_exec() {
        // `return arr[i]` reads an array -> Exec_ArrayVar_Get -> genuinely exec.
        let src = "var xs: int[]\nmod at(i: int) -> int {\n  return xs[i]\n}";
        assert!(mod_exec(src, "at"), "array-index return mod should be exec");
    }

    #[test]
    fn bare_return_mod_is_exec() {
        // A bare early `return` is exec-chain control flow.
        let src = "mod f(x: int) {\n  return\n}";
        assert!(mod_exec(src, "f"), "bare return should be exec");
    }

    #[test]
    fn if_statement_mod_is_exec() {
        let src = "var xs: int[]\nmod g(x: int) {\n  if x > 0 { xs.push(x) }\n}";
        assert!(mod_exec(src, "g"), "if-statement mod should be exec");
    }
