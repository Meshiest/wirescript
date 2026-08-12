    use super::*;

    fn mem(files: &[(&str, &str)]) -> MemLoader {
        MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn record_let_pulls_array_deps_and_expands_let_annotation() {
        // Importing a record-of-arrays `let` must pull the arrays its
        // initializer references (named and shorthand fields) and inline the
        // alias in the let's annotation; emits inside the chip body must pull
        // the mods/constants they reference.
        let loader = mem(&[(
            "lib.ws",
            "let X = 7\n\
             type Tables = { vals: int[] }\n\
             var vals: int[]\n\
             let TB: Tables = { vals: vals }\n\
             mod bump(tables: Tables, v: int) {\n  tables.vals.push(v + X)\n}\n\
             chip Init(init: exec, tables: Tables) -> (code: int) {\n  on init {\n    bump(tables, X)\n    emit code = X\n  }\n}\n",
        )]);
        let src = "import { Init, TB } from \"lib\"\nin reset: exec\nlet R = Init(reset, TB)\nout v = R.code";
        let r = resolve(src, "main.ws", &loader);
        assert!(
            r.diagnostics.is_empty(),
            "resolve diags: {:?}",
            r.diagnostics
        );
        let tc = crate::typecheck::typecheck(&r.ast, "main.ws", &crate::typecheck::CeSlotMap::default());
        let errors: Vec<_> = tc
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "typecheck errors: {errors:?}");
    }
