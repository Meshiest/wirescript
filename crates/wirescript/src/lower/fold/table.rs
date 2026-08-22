//! Loader for the certified gate-semantics table (`data/gate_semantics.json`).
//! The table is ground truth from an in-game probe run — see the fold pass
//! docs. Nothing here may be hand-edited into existence: the JSON is generated
//! by `scripts/gen_semantics.mjs` from a probe dump.
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum InVariant {
    Int,
    Float,
    Bool,
    Str,
    Unwired,
    Vector,
    Rotator,
    Color,
    Quat,
    /// FormatText's synthetic probe-case label (`tmpl:0str`, ...) — the probe
    /// records ONE label token per case, not the real template text or the
    /// real substitution operands (those only exist in the probe SOURCE,
    /// `probes/gate_semantics.ws`, which the loader never reads). A real
    /// `Value` (see `lower/fold/eval.rs`) can never carry this variant, so
    /// `CertifiedTable::covers` can never match a live query against it —
    /// FormatText is permanently uncovered via the generic per-node fold
    /// path by construction, not by a special case anywhere. It still needs
    /// a real (non-panicking) variant here so the loader doesn't choke on
    /// the `strings` chapter.
    Tmpl,
}

/// A parsed case operand/output value. Scalar variants keep the certified
/// table's raw text (parsed by `lower/fold/eval.rs`'s replay harness);
/// composite variants are parsed HERE, structurally, from
/// their constructor text (`Vec(x,y,z)`, `Rotation(pitch,yaw,roll)`,
/// `Quat(x,y,z,w)`, `Color(r,g,b[,a])`) since that's the only form an
/// operand value ever takes in the table — a malformed constructor is a
/// build-breaking panic (`data/gate_semantics.json` is machine-generated;
/// same posture as the malformed-JSON panic below).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CaseValue {
    /// int/float/bool/str/tmpl raw text, as recorded.
    Scalar(String),
    Vector { x: f64, y: f64, z: f64 },
    Rotator { pitch: f64, yaw: f64, roll: f64 },
    Quat { x: f64, y: f64, z: f64, w: f64 },
    /// Linear-space components — `MakeColorSRGB`/`MakeColorHex`'s certified
    /// gamma law lives in `eval.rs`, not here; this just stores whatever
    /// constructor text appeared (`Color(r,g,b[,a])`), and an omitted alpha
    /// defaults to fully opaque (`1.0`) — never observably distinguishable
    /// in any certified case (every case using a 3-arg `Color(...)` operand
    /// either never reads alpha at all, or the case output doesn't render).
    Color { r: f64, g: f64, b: f64, a: f64 },
}

#[allow(dead_code)] // read by the certified-semantics replay tests
#[allow(dead_code)] // read by the certified-semantics replay tests
#[derive(Clone, Debug)]
pub(crate) struct CaseInput {
    pub variant: InVariant,
    /// `None` iff `variant == Unwired`.
    pub value: Option<CaseValue>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // read by the certified-semantics replay tests
pub(crate) struct Case {
    pub inputs: Vec<CaseInput>,
    /// The console-rendered output as recorded in-game, VERBATIM. For a
    /// composite (vector/rotator/color/quat) result this is either the
    /// `X=.. Y=.. Z=..` vector layout or an empty string (rotator/color/quat
    /// never render through FormatText — certified, see the `render`
    /// section) — never structurally parsed, only ever compared textually
    /// against `eval::render(eval::eval(...))` by the replay harness. The
    /// recorded output VARIANT label is deliberately not exposed: whole-
    /// valued float outputs print decimal-less and mislabel as int.
    pub output_value: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AnnihilatorKind { AndFalse, OrTrue }

pub(crate) struct CertifiedTable {
    gates: HashMap<String, GateEntry>,
    /// The `render` top-level table section (probeVersion 3): label -> exact
    /// in-game FormatText-rendered text, verbatim (commas and all). Not a
    /// gate — it's the calibration source every other v3 chapter's
    /// composite/numeric results are interpreted against (every chapter's
    /// results print through the SAME interpolation path — see
    /// `probes/gate_semantics.ws`'s `runRender` doc comment). `eval.rs`'s
    /// `render_for_format` is validated directly against this.
    #[allow(dead_code)] // read only by render_laws (replay tests)
    render: HashMap<String, String>,
}

struct GateEntry {
    signatures: HashSet<Vec<InVariant>>,
    cases: Vec<Case>,
    annihilator: Option<AnnihilatorKind>,
}

// ---- raw serde mirror of the JSON ----
#[derive(serde::Deserialize)]
struct RawTable {
    gates: HashMap<String, RawGate>,
    #[serde(default)]
    render: HashMap<String, String>,
}
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
        "vector" => InVariant::Vector,
        "rotator" => InVariant::Rotator,
        "color" => InVariant::Color,
        "quat" => InVariant::Quat,
        "tmpl" => InVariant::Tmpl,
        other => panic!(
            "data/gate_semantics.json: unrecognized operand variant tag {other:?} — \
             stale/unsupported probe shape, table.rs needs updating"
        ),
    }
}

/// Split `Ctor(a, b, c)` into `("Ctor", ["a", "b", "c"])`. Panics (build-
/// breaking, matches the module's posture on malformed table data) if the
/// text isn't shaped like a constructor call.
fn split_ctor(raw: &str) -> (&str, Vec<f64>) {
    let open = raw.find('(').unwrap_or_else(|| {
        panic!("malformed composite constructor {raw:?}: no '(' — stale table shape?")
    });
    assert!(
        raw.ends_with(')'),
        "malformed composite constructor {raw:?}: no trailing ')' — stale table shape?"
    );
    let name = &raw[..open];
    let inner = &raw[open + 1..raw.len() - 1];
    let nums: Vec<f64> = inner
        .split(',')
        .map(|tok| {
            tok.trim().parse::<f64>().unwrap_or_else(|e| {
                panic!("malformed composite component {tok:?} in {raw:?}: {e}")
            })
        })
        .collect();
    (name, nums)
}

/// Parse one composite operand's constructor text per `variant` (already
/// known from the JSON's own `variant` tag, so the constructor NAME itself
/// is not re-checked against `variant` — only arity is).
fn parse_composite(variant: InVariant, raw: &str) -> CaseValue {
    let (_name, n) = split_ctor(raw);
    match variant {
        InVariant::Vector => {
            assert_eq!(n.len(), 3, "Vec(...) must have 3 components: {raw:?}");
            CaseValue::Vector { x: n[0], y: n[1], z: n[2] }
        }
        InVariant::Rotator => {
            assert_eq!(n.len(), 3, "Rotation(...) must have 3 components: {raw:?}");
            CaseValue::Rotator { pitch: n[0], yaw: n[1], roll: n[2] }
        }
        InVariant::Quat => {
            assert_eq!(n.len(), 4, "Quat(...) must have 4 components: {raw:?}");
            CaseValue::Quat { x: n[0], y: n[1], z: n[2], w: n[3] }
        }
        InVariant::Color => {
            assert!(
                n.len() == 3 || n.len() == 4,
                "Color(...) must have 3 or 4 components: {raw:?}"
            );
            CaseValue::Color { r: n[0], g: n[1], b: n[2], a: n.get(3).copied().unwrap_or(1.0) }
        }
        _ => unreachable!("parse_composite only called for composite variants"),
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
                        RawInput::Val(v) => {
                            let variant = parse_variant(&v.variant);
                            let value = match variant {
                                InVariant::Vector | InVariant::Rotator
                                | InVariant::Quat | InVariant::Color => {
                                    parse_composite(variant, &v.value)
                                }
                                _ => CaseValue::Scalar(v.value.clone()),
                            };
                            CaseInput { variant, value: Some(value) }
                        }
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
            CertifiedTable { gates, render: raw.render }
        })
    }

    pub(crate) fn covers(&self, gate_class: &str, sig: &[InVariant]) -> bool {
        self.gates.get(gate_class).is_some_and(|g| g.signatures.contains(sig))
    }
    #[allow(dead_code)] // replay tests
    pub(crate) fn cases(&self, gate_class: &str) -> &[Case] {
        self.gates.get(gate_class).map_or(&[], |g| &g.cases)
    }
    pub(crate) fn annihilator(&self, gate_class: &str) -> Option<AnnihilatorKind> {
        self.gates.get(gate_class).and_then(|g| g.annihilator)
    }
    #[allow(dead_code)] // replay tests
    pub(crate) fn gate_classes(&self) -> impl Iterator<Item = &str> {
        self.gates.keys().map(|s| s.as_str())
    }
    /// The certified `render` table section: label (`"int:1000"`,
    /// `"vector:Vec(1.0,2.0,3.0)"`, ...) -> exact in-game rendered text.
    #[allow(dead_code)] // replay tests
    pub(crate) fn render_laws(&self) -> &HashMap<String, String> {
        &self.render
    }
}

#[cfg(test)]
mod tests;
