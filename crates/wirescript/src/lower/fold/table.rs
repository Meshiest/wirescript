//! Loader for the certified gate-semantics table (`data/gate_semantics.json`).
//! The table is ground truth from an in-game probe run — see the fold pass
//! docs. Nothing here may be hand-edited into existence: the JSON is generated
//! by `scripts/gen_semantics.mjs` from a probe dump.
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum InVariant { Int, Float, Bool, Str, Unwired }

#[derive(Clone, Debug)]
pub(crate) struct CaseInput {
    pub variant: InVariant,
    /// `None` iff `variant == Unwired`.
    pub value: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Case {
    pub inputs: Vec<CaseInput>,
    /// FormatText-rendered output as recorded in-game. The recorded output
    /// VARIANT label is deliberately not exposed: whole-valued float outputs
    /// print decimal-less and mislabel as int.
    pub output_value: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AnnihilatorKind { AndFalse, OrTrue }

pub(crate) struct CertifiedTable {
    gates: HashMap<String, GateEntry>,
}

struct GateEntry {
    signatures: HashSet<Vec<InVariant>>,
    cases: Vec<Case>,
    annihilator: Option<AnnihilatorKind>,
}

// ---- raw serde mirror of the JSON ----
#[derive(serde::Deserialize)]
struct RawTable { gates: HashMap<String, RawGate> }
#[derive(serde::Deserialize)]
struct RawGate {
    cases: Vec<RawCase>,
    #[serde(default)]
    rules: Vec<RawRule>,
}
#[derive(serde::Deserialize)]
struct RawCase { inputs: Vec<RawInput>, output: RawVal }
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RawInput { Unwired(String), Val(RawVal) }
#[derive(serde::Deserialize)]
struct RawVal { variant: String, value: String }
#[derive(serde::Deserialize)]
struct RawRule {
    kind: String,
    #[serde(default)]
    when: Option<RawVal>,
    #[serde(default)]
    result: Option<RawVal>,
}

fn parse_variant(s: &str) -> InVariant {
    match s {
        "int" => InVariant::Int,
        "float" => InVariant::Float,
        "bool" => InVariant::Bool,
        "str" => InVariant::Str,
        "unwired" => InVariant::Unwired,
        other => panic!("gate_semantics.json: unknown variant {other:?}"),
    }
}

impl CertifiedTable {
    pub(crate) fn certified() -> &'static CertifiedTable {
        static TABLE: OnceLock<CertifiedTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            let raw: RawTable =
                serde_json::from_str(include_str!("../../../data/gate_semantics.json"))
                    .expect("data/gate_semantics.json must parse");
            let mut gates = HashMap::new();
            for (class, g) in raw.gates {
                let mut entry = GateEntry {
                    signatures: HashSet::new(),
                    cases: Vec::new(),
                    annihilator: None,
                };
                for c in g.cases {
                    let inputs: Vec<CaseInput> = c.inputs.iter().map(|i| match i {
                        RawInput::Unwired(s) => {
                            assert_eq!(s, "unwired", "unexpected bare input {s:?}");
                            CaseInput { variant: InVariant::Unwired, value: None }
                        }
                        RawInput::Val(v) => CaseInput {
                            variant: parse_variant(&v.variant),
                            value: Some(v.value.clone()),
                        },
                    }).collect();
                    entry.signatures.insert(inputs.iter().map(|i| i.variant).collect());
                    entry.cases.push(Case { inputs, output_value: c.output.value });
                }
                for r in &g.rules {
                    if r.kind != "annihilator" { continue; }
                    let (w, res) = (r.when.as_ref(), r.result.as_ref());
                    let (Some(w), Some(res)) = (w, res) else { continue };
                    // Whitelist only — anything else in rules is ignored, never
                    // trusted (spec: rules are the annihilator allowlist).
                    entry.annihilator = match (w.variant.as_str(), w.value.as_str(),
                                               res.variant.as_str(), res.value.as_str()) {
                        ("bool", "false", "bool", "false") => Some(AnnihilatorKind::AndFalse),
                        ("bool", "true", "bool", "true") => Some(AnnihilatorKind::OrTrue),
                        _ => None,
                    };
                }
                gates.insert(class, entry);
            }
            CertifiedTable { gates }
        })
    }

    pub(crate) fn covers(&self, gate_class: &str, sig: &[InVariant]) -> bool {
        self.gates.get(gate_class).is_some_and(|g| g.signatures.contains(sig))
    }
    pub(crate) fn cases(&self, gate_class: &str) -> &[Case] {
        self.gates.get(gate_class).map_or(&[], |g| &g.cases)
    }
    pub(crate) fn annihilator(&self, gate_class: &str) -> Option<AnnihilatorKind> {
        self.gates.get(gate_class).and_then(|g| g.annihilator)
    }
    pub(crate) fn gate_classes(&self) -> impl Iterator<Item = &str> {
        self.gates.keys().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_real_table_with_17_gates() {
        let t = CertifiedTable::certified();
        assert_eq!(t.gate_classes().count(), 17);
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathAdd",
            &[InVariant::Int, InVariant::Int]));
        // Probed one direction only — the reverse signature must NOT be covered.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_CompareEqual",
            &[InVariant::Int, InVariant::Str]));
        assert!(!t.covers("BrickComponentType_WireGraph_Expr_CompareEqual",
            &[InVariant::Str, InVariant::Int]));
        // Unwired is part of the signature.
        assert!(t.covers("BrickComponentType_WireGraph_Expr_MathAdd",
            &[InVariant::Int, InVariant::Unwired]));
        // Select/Branch signatures are 1-ary (condition only).
        assert!(t.covers("BrickComponentType_WireGraph_Expr_Select", &[InVariant::Str]));
        assert!(t.covers("BrickComponentType_WireGraph_Exec_Branch", &[InVariant::Unwired]));
    }

    #[test]
    fn annihilators_are_exactly_and_false_or_true() {
        let t = CertifiedTable::certified();
        assert!(matches!(
            t.annihilator("BrickComponentType_WireGraph_Expr_LogicalAND"),
            Some(AnnihilatorKind::AndFalse)));
        assert!(matches!(
            t.annihilator("BrickComponentType_WireGraph_Expr_LogicalOR"),
            Some(AnnihilatorKind::OrTrue)));
        assert!(t.annihilator("BrickComponentType_WireGraph_Expr_LogicalXOR").is_none());
        assert!(t.annihilator("BrickComponentType_WireGraph_Expr_MathAdd").is_none());
    }

    #[test]
    fn unwired_case_inputs_parse_as_unwired() {
        let t = CertifiedTable::certified();
        let cases = t.cases("BrickComponentType_WireGraph_Expr_CompareEqual");
        assert!(cases.iter().any(|c| c.inputs.len() == 2
            && c.inputs[1].variant == InVariant::Unwired
            && c.inputs[1].value.is_none()));
    }
}
