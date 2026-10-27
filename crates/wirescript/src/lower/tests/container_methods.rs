//! Golden test pinning how every container method lowers: for each
//! `ARRAY_METHODS` / `MAP_METHODS` entry, over several element types and call
//! forms, the lowered module is reduced to one line per gate (class, declared
//! ports and their types, what feeds each input, baked properties) and compared
//! against `container_methods.golden`.
//!
//! Set `WIRESCRIPT_BLESS=1` to rewrite the golden file from the current
//! lowering. A missing golden file fails the test.

use super::*;
use crate::ir::{Module, Type};

const ELEMS: &[&str] = &["int", "string", "vector"];
const MAP_KEYS: &[&str] = &["int", "string"];

/// A literal of `ty` for the `i`th argument, distinct per position so a value
/// wired into the wrong port shows up in the golden.
fn literal(ty: &Type, i: usize) -> String {
    match ty {
        Type::Int => format!("{}", 10 + i),
        Type::Float => format!("{}.5", 10 + i),
        Type::Bool => "true".to_string(),
        Type::String => format!("\"s{i}\""),
        Type::Vector => format!("Vec({}.0, 2.0, 3.0)", 10 + i),
        Type::Entity => "ent".to_string(),
        Type::Zone => "z".to_string(),
        other => panic!("no literal for argument type {other:?}"),
    }
}

/// The type a bare use of a call returning `ret` reads, or `None` for a
/// statement.
fn primary(ret: &Type) -> Option<Type> {
    match ret {
        Type::Never | Type::Any => None,
        Type::Outputs(fs) | Type::Record(fs) => fs.first().map(|(_, t)| t.clone()),
        t => Some(t.clone()),
    }
}

/// An argument list for `params`, positional or named. Container-typed params
/// name the program's prepared containers via `container`.
fn args(
    params: &[crate::typecheck::Param],
    required_only: bool,
    named: bool,
    container: impl Fn(&Type) -> String,
) -> Vec<String> {
    params
        .iter()
        .enumerate()
        .filter(|(_, p)| !(required_only && p.optional))
        .map(|(i, p)| {
            let v = match &p.ty {
                Type::Array(_) | Type::Map(..) => container(&p.ty),
                t => literal(t, i),
            };
            if named {
                format!("{} = {v}", p.name)
            } else {
                v
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Form {
    /// Every parameter, positionally.
    Positional,
    /// Required parameters only, positionally.
    Required,
    /// Every parameter positionally, plus `exec = u`.
    Exec,
    /// Every parameter by name.
    Named,
    /// Every parameter by name, plus `exec = u`.
    NamedExec,
}

impl Form {
    fn label(self) -> &'static str {
        match self {
            Self::Positional => "pos",
            Self::Required => "req",
            Self::Exec => "exec",
            Self::Named => "named",
            Self::NamedExec => "named-exec",
        }
    }
}

/// The call's argument text for `form`.
fn call_args(
    params: &[crate::typecheck::Param],
    form: Form,
    container: impl Fn(&Type) -> String,
) -> String {
    let named = matches!(form, Form::Named | Form::NamedExec);
    let mut a = args(params, form == Form::Required, named, container);
    if matches!(form, Form::Exec | Form::NamedExec) {
        a.push("exec = u".to_string());
    }
    a.join(", ")
}

fn program(decls: &str, sink: Option<&Type>, call: &str) -> String {
    let (sink_decl, assign) = match sink {
        Some(t) => (format!("var sink: {t}\n"), "sink = "),
        None => (String::new(), ""),
    };
    format!(
        "{decls}var ent: entity\nvar after: int\n{sink_decl}in z: zone\nin t: exec\nin u: exec\n\
         on t {{\n  {assign}{call}\n  after = 1\n}}\n"
    )
}

fn array_program(
    m: &crate::catalog::arrays::ArrayMethod,
    elem: &str,
    form: Form,
) -> Option<String> {
    let elem_ty = ty(elem);
    let params = m.params(&elem_ty);
    if form == Form::Required && params.iter().all(|p| !p.optional) {
        return None;
    }
    let call = if m.name == "sortMultiple" {
        match form {
            Form::Positional => "a.sortMultiple(b, true)".to_string(),
            Form::Required => "a.sortMultiple(b)".to_string(),
            Form::Exec => "a.sortMultiple(b, true, exec = u)".to_string(),
            Form::Named => "a.sortMultiple(b, descending = true)".to_string(),
            Form::NamedExec => "a.sortMultiple(b, descending = true, exec = u)".to_string(),
        }
    } else {
        format!(
            "a.{}({})",
            m.name,
            call_args(&params, form, |_| "b".to_string())
        )
    };
    let ret = crate::catalog::arrays::array_return_type(m.name, &elem_ty).expect("array method");
    let sink = primary(&ret);
    Some(program(
        &format!("var a: {elem}[]\nvar b: {elem}[]\n"),
        sink.as_ref(),
        &call,
    ))
}

fn map_program(
    m: &crate::catalog::maps::MapMethod,
    key: &str,
    value: &str,
    form: Form,
) -> Option<String> {
    let key_ty = ty(key);
    let value_ty = ty(value);
    let params = m.params(&key_ty, &value_ty);
    if form == Form::Required && params.iter().all(|p| !p.optional) {
        return None;
    }
    let call = format!(
        "m.{}({})",
        m.name,
        call_args(&params, form, |t| match t {
            Type::Map(..) => "m2".to_string(),
            Type::Array(inner) if inner.as_ref() == &key_ty => "kd".to_string(),
            Type::Array(_) => "vd".to_string(),
            other => panic!("no container for {other:?}"),
        })
    );
    let ret =
        crate::catalog::maps::map_return_type(m.name, &key_ty, &value_ty).expect("map method");
    let sink = primary(&ret);
    Some(program(
        &format!(
            "var m: Map<{key}, {value}>\nvar m2: Map<{key}, {value}>\nvar kd: {key}[]\nvar vd: {value}[]\n"
        ),
        sink.as_ref(),
        &call,
    ))
}

fn ty(name: &str) -> Type {
    match name {
        "int" => Type::Int,
        "string" => Type::String,
        "vector" => Type::Vector,
        other => panic!("unknown element type {other}"),
    }
}

fn short(class: &str) -> &str {
    class.strip_prefix("BrickComponentType_").unwrap_or(class)
}

/// One normalized line per gate in `module` (and its chips): class, baked
/// properties, inputs (sorted, each with the `class.port` feeding it), and
/// outputs (value outputs in declared order, the order emit reads the first
/// one in, then exec outputs).
fn normalize(root: &Module, out: &mut Vec<String>) {
    root.walk(&mut |_, m| normalize_module(m, out));
}

fn normalize_module(module: &Module, out: &mut Vec<String>) {
    // A source is named by its class plus the name it carries, so the `t` and
    // `u` inputs (or two vars) stay distinguishable.
    let label = |id: &crate::ir::NodeId| -> String {
        let Some(n) = module.nodes.get(id) else {
            return "<external>".to_string();
        };
        let name = ["_label", "PortLabel"].iter().find_map(|k| {
            match n.properties.get(&crate::intern::intern_static(k)) {
                Some(crate::ir::Literal::String(s)) => Some(s.clone()),
                _ => None,
            }
        });
        match name {
            Some(name) => format!("{}({name})", short(n.gate_class)),
            None => short(n.gate_class).to_string(),
        }
    };
    for node in module.nodes.values() {
        let mut props: Vec<String> = node
            .properties
            .iter()
            .map(|(k, v)| format!("{}={v:?}", crate::intern::resolve(*k)))
            .collect();
        props.sort();
        let mut inputs: Vec<String> = node
            .ports
            .inputs
            .iter()
            .map(|p| {
                let name = crate::intern::resolve(p.name);
                let mut srcs: Vec<String> = module
                    .wires
                    .iter()
                    .filter(|w| w.target.node_id == node.id && w.target.port.as_str() == name)
                    .map(|w| format!("{}.{}", label(&w.source.node_id), w.source.port))
                    .collect();
                srcs.sort();
                format!("{name}:{:?}<-[{}]", p.ty, srcs.join(","))
            })
            .collect();
        inputs.sort();
        let fmt_out =
            |p: &crate::ir::PortSpec| format!("{}:{:?}", crate::intern::resolve(p.name), p.ty);
        let mut outputs: Vec<String> = node
            .ports
            .outputs
            .iter()
            .filter(|p| !matches!(p.ty, Type::Exec))
            .map(fmt_out)
            .collect();
        let mut execs: Vec<String> = node
            .ports
            .outputs
            .iter()
            .filter(|p| matches!(p.ty, Type::Exec))
            .map(fmt_out)
            .collect();
        execs.sort();
        outputs.extend(execs);
        out.push(format!(
            "  {} {{{}}} in({}) out({})",
            short(node.gate_class),
            props.join(" "),
            inputs.join(" "),
            outputs.join(" ")
        ));
    }
}

/// The golden section for one program: its diagnostics, then its gates.
fn render(title: &str, src: &str) -> String {
    let r = compile(src);
    let mut diags: Vec<String> = r
        .diagnostics
        .iter()
        .map(|d| format!("  ! {:?} {} {}", d.severity, d.code, d.message))
        .collect();
    diags.sort();
    let mut gates = Vec::new();
    normalize(&r.module, &mut gates);
    gates.sort();
    let mut s = format!("== {title}\n");
    for l in diags.iter().chain(gates.iter()) {
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// Every (title, program) of the golden set, in a stable order.
fn cases(forms: &[Form]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for m in crate::catalog::arrays::ARRAY_METHODS {
        for elem in ELEMS {
            for &form in forms {
                if let Some(src) = array_program(m, elem, form) {
                    out.push((format!("array {} {elem} {}", m.name, form.label()), src));
                }
            }
        }
    }
    for m in crate::catalog::maps::MAP_METHODS {
        for key in MAP_KEYS {
            for value in ELEMS {
                for &form in forms {
                    if let Some(src) = map_program(m, key, value, form) {
                        out.push((
                            format!("map {} {key}->{value} {}", m.name, form.label()),
                            src,
                        ));
                    }
                }
            }
        }
    }
    out
}

fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lower/tests/container_methods.golden")
}

#[test]
fn container_methods_match_golden() {
    let actual: String = cases(&[Form::Positional, Form::Required, Form::Exec])
        .iter()
        .map(|(title, src)| render(title, src))
        .collect();
    let path = golden_path();
    if std::env::var_os("WIRESCRIPT_BLESS").is_some() {
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| {
            panic!(
                "cannot read {} ({e}); rerun with WIRESCRIPT_BLESS=1 to write it",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    if expected != actual {
        let exp: Vec<&str> = expected.lines().collect();
        let act: Vec<&str> = actual.lines().collect();
        let mut section = "";
        let mut diffs = Vec::new();
        for i in 0..exp.len().max(act.len()) {
            let (e, a) = (exp.get(i).copied(), act.get(i).copied());
            if let Some(l) = a.filter(|l| l.starts_with("== ")) {
                section = l;
            }
            if e != a {
                diffs.push(format!(
                    "{section}\n  - {}\n  + {}",
                    e.unwrap_or("<none>"),
                    a.unwrap_or("<none>")
                ));
                if diffs.len() >= 20 {
                    break;
                }
            }
        }
        panic!(
            "container method lowering differs from {} (first differences below; rerun with \
             WIRESCRIPT_BLESS=1 to accept):\n{}",
            path.display(),
            diffs.join("\n")
        );
    }
}

/// A named-argument call lowers to exactly the gates of its positional form.
#[test]
fn named_arguments_lower_like_positional() {
    let positional = cases(&[Form::Positional, Form::Exec]);
    let named = cases(&[Form::Named, Form::NamedExec]);
    assert_eq!(positional.len(), named.len());
    let mut mismatched = Vec::new();
    for ((pt, ps), (nt, ns)) in positional.iter().zip(&named) {
        let p = render("", ps);
        let n = render("", ns);
        if p != n {
            mismatched.push(format!("{nt} differs from {pt}:\n{n}vs\n{p}"));
        }
    }
    crate::test_support::report_failures(&mismatched, named.len());
}

/// A map method with `exec =` that stops early (a `const` destination) still
/// hands the caller its own exec chain back: the statement after it runs on
/// the handler, not on the trigger.
#[test]
fn map_method_exec_trigger_is_restored_on_early_exit() {
    let src = "const K = [1, 2]\nvar m: Map<int, int>\nvar after: int\nin t: exec\nin u: exec\n\
               on t {\n  m.keys(K, exec = u)\n  after = 1\n}\n";
    let r = compile(src);
    crate::test_support::assert_code(&r.diagnostics, "WS044");
    let mut gates = Vec::new();
    normalize(&r.module, &mut gates);
    let set = gates
        .iter()
        .find(|l| l.contains("Var(after).VarRef"))
        .expect("the `after = 1` write");
    assert!(
        set.contains("Exec:Exec<-[Internal_MicrochipInput(t).RER_Output]"),
        "{set}"
    );
}
