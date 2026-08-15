//! `import * as ns` VALUE members (`ns.myValue`) — as opposed to `ns.f(...)`
//! calls, which have always worked. These used to type as `any` and lower to an
//! `_Unsupported` placeholder that silently read 0.

use super::*;
use crate::resolve::{MemLoader, resolve};

/// Resolve + typecheck + lower a two-file program (`lib.ws` + main).
fn compile_with_lib(lib_src: &str, main_src: &str) -> (crate::typecheck::TypeCheckResult, LowerResult) {
    let mut files = std::collections::HashMap::default();
    files.insert("lib.ws".to_string(), lib_src.into());
    let loader = MemLoader { files };
    let resolved = resolve(main_src, "test", &loader);
    assert!(
        resolved.diagnostics.is_empty(),
        "import should resolve: {:?}",
        resolved.diagnostics
    );
    let tc = crate::typecheck::typecheck(
        &resolved.ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    let lr = crate::lower::lower(crate::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::Auto,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    });
    (tc, lr)
}

fn assert_clean(tc: &crate::typecheck::TypeCheckResult, lr: &LowerResult) {
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "lower errors: {:?}",
        lr.diagnostics
    );
}

fn has_unsupported(m: &crate::ir::Module) -> bool {
    m.nodes.values().any(|n| n.gate_class.contains("Unsupported"))
        || m.chips.values().any(has_unsupported)
}

/// Every `Literal::Int` baked into any node property, anywhere in the tree.
fn baked_ints(m: &crate::ir::Module) -> Vec<i64> {
    let mut out = Vec::new();
    for n in m.nodes.values() {
        for v in n.properties.values() {
            if let crate::ir::Literal::Int(i) = v {
                out.push(*i);
            }
        }
    }
    for c in m.chips.values() {
        out.extend(baked_ints(c));
    }
    out
}

#[test]
fn namespaced_scalar_let_lowers_to_its_value() {
    let (tc, lr) = compile_with_lib(
        "let answer: int = 42",
        "import * as lib from \"lib\"\nout o = lib.answer",
    );
    assert_clean(&tc, &lr);
    assert!(
        !has_unsupported(&lr.module),
        "a namespaced scalar must lower to a real gate, not a placeholder"
    );
    assert!(
        baked_ints(&lr.module).contains(&42),
        "the namespaced constant's value must reach the output, got {:?}",
        baked_ints(&lr.module)
    );
}

#[test]
fn namespaced_record_let_field_lowers_to_its_value() {
    // `lib.rec.value` — the namespace hop resolves to the record binding, then
    // the normal record-field walk continues from there.
    let (tc, lr) = compile_with_lib(
        "type V = {value: int}\nlet rec: V = {value: 42}",
        "import * as lib from \"lib\"\nout o = lib.rec.value",
    );
    assert_clean(&tc, &lr);
    assert!(!has_unsupported(&lr.module), "must not emit a placeholder");
    assert!(
        baked_ints(&lr.module).contains(&42),
        "the record field's value must reach the output, got {:?}",
        baked_ints(&lr.module)
    );
}

#[test]
fn namespaced_value_types_as_its_declared_type_not_any() {
    // Typing it `any` made every use against a concrete type a spurious
    // mismatch — this program is correct and must check clean.
    let (tc, lr) = compile_with_lib(
        "type V = {value: int}\nlet rec: V = {value: 42}",
        "import * as lib from \"lib\"\ntype Outer = {value: lib.V}\nlet r: Outer = {value: lib.rec}\nout o = r.value.value",
    );
    assert_clean(&tc, &lr);
    assert!(
        baked_ints(&lr.module).contains(&42),
        "the nested record value must survive, got {:?}",
        baked_ints(&lr.module)
    );
}

#[test]
fn namespaced_value_feeding_a_typed_param_checks_clean() {
    // The `any` typing showed up as a WS003 at any typed boundary.
    let (tc, lr) = compile_with_lib(
        "let answer: int = 42",
        "import * as lib from \"lib\"\nmod takesInt(v: int) -> int { return v + 1 }\nout o = takesInt(lib.answer)",
    );
    assert_clean(&tc, &lr);
    assert!(!has_unsupported(&lr.module), "must not emit a placeholder");
}

#[test]
fn namespaced_chip_calls_still_work() {
    // Guard the pre-existing call path against the value-member change.
    let (tc, lr) = compile_with_lib(
        "chip Double(x: int) -> (result: int) { out result = x + x }",
        "import * as lib from \"lib\"\nlet r = lib.Double(5)\nout o = r.result",
    );
    assert_clean(&tc, &lr);
    assert!(
        !lr.module.chips.is_empty(),
        "a namespaced chip call must still instantiate its chip"
    );
}
