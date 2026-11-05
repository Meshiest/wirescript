//! Every case of the certified table (`gate_semantics.json`), replayed through
//! the simulator: one gate per case, its inputs stored as an unwired save
//! stores them, evaluated the way a tick evaluates it. The simulator must give
//! the recorded output text, or refuse; it never gives a different answer.

use serde_json::Value as Json;
use wirescript_ir::semantics::render_for_format;
use wirescript_sim::graph::{Gate, Graph};
use wirescript_sim::load::BrickAddr;
use wirescript_sim::machine::Machine;
use wirescript_sim::{NullWorld, SimOptions, Value};

const TABLE: &str = include_str!("../../wirescript-ir/data/gate_semantics.json");

fn num(s: &str) -> f64 {
    match s {
        "NaN" => f64::NAN,
        "inf" => f64::INFINITY,
        "-inf" => f64::NEG_INFINITY,
        _ => s.parse().unwrap_or_else(|_| panic!("number {s:?}")),
    }
}

fn composite(raw: &str) -> Vec<f64> {
    let open = raw.find('(').expect("composite");
    raw[open + 1..raw.len() - 1].split(',').map(|p| num(p.trim())).collect()
}

/// A case input as the simulator's value; `None` for an unwired input.
/// `Err` for a `FormatText` template label, which is not replayable.
fn case_value(input: &Json) -> Result<Option<Value>, ()> {
    if input.as_str() == Some("unwired") {
        return Ok(None);
    }
    let variant = input["variant"].as_str().expect("variant");
    let raw = input["value"].as_str().expect("value");
    Ok(Some(match variant {
        "int" => Value::Int(raw.parse().expect("int")),
        "float" => Value::Float(num(raw)),
        "bool" => Value::Bool(raw == "true"),
        "str" => Value::Str(raw.to_string()),
        "vector" => {
            let c = composite(raw);
            Value::Vector { x: c[0], y: c[1], z: c[2] }
        }
        "rotator" => {
            let c = composite(raw);
            Value::Rotator { pitch: c[0], yaw: c[1], roll: c[2] }
        }
        "quat" => {
            let c = composite(raw);
            Value::Quat { x: c[0], y: c[1], z: c[2], w: c[3] }
        }
        "color" => {
            let c = composite(raw);
            Value::Color { r: c[0], g: c[1], b: c[2], a: c.get(3).copied().unwrap_or(1.0) }
        }
        "tmpl" => return Err(()),
        other => panic!("variant {other}"),
    }))
}

#[test]
fn replay_every_certified_case_through_the_simulator() {
    let table: Json = serde_json::from_str(TABLE).expect("table");
    let (mut matched, mut refused, mut skipped) = (0usize, Vec::new(), 0usize);
    let mut mismatches = Vec::new();
    for (class, entry) in table["gates"].as_object().expect("gates") {
        for case in entry["cases"].as_array().expect("cases") {
            let inputs: Result<Vec<Option<Value>>, ()> =
                case["inputs"].as_array().expect("inputs").iter().map(case_value).collect();
            let Ok(inputs) = inputs else {
                skipped += 1;
                continue;
            };
            let mut gate = Gate::new(class, BrickAddr { grid: 1, chunk: (0, 0, 0), brick: 0 });
            let ports = if class.ends_with("Exec_Branch") {
                vec!["bCond".to_string()]
            } else if class.ends_with("Expr_Select") {
                vec!["bSelectB".to_string()]
            } else {
                wirescript_sim::ports::eval_ports(&gate)
            };
            for (port, v) in ports.iter().zip(inputs) {
                let i = gate.input(port).unwrap_or_else(|| panic!("{class} has no {port}"));
                gate.inputs[i as usize].stored = v;
            }
            if class.ends_with("Expr_Select") {
                // The recorded output is which side won: 111 for B (taken when the
                // condition is truthy), 222 for A.
                for (port, n) in [("InputA", 222), ("InputB", 111)] {
                    let i = gate.input(port).expect("select input");
                    gate.inputs[i as usize].stored = Some(Value::Int(n));
                }
            }
            let mut m = Machine::new(
                Graph::from_gates(vec![gate]),
                Box::new(NullWorld::default()),
                SimOptions::default(),
            );
            let want = case["output"]["value"].as_str().expect("output").to_string();
            match m.evaluate_alone(0) {
                Ok(Some(v)) => {
                    let got = match v.to_semantic() {
                        Some(s) => render_for_format(&s),
                        None => format!("{v}"),
                    };
                    if got == want {
                        matched += 1;
                    } else {
                        mismatches.push(format!("{class} {}: want {want:?} got {got:?}", case["inputs"]));
                    }
                }
                Ok(None) => mismatches.push(format!("{class} {}: no output", case["inputs"])),
                Err(e) => refused.push(format!("{class} {}: {}", case["inputs"], e.message)),
            }
        }
    }
    for r in &refused {
        eprintln!("refused: {r}");
    }
    assert!(mismatches.is_empty(), "{} mismatches:\n{}", mismatches.len(), mismatches.join("\n"));
    eprintln!("{matched} matched, {} refused, {skipped} FormatText labels skipped", refused.len());
    assert_eq!(matched, 385, "cases replayed");
    assert_eq!(refused.len(), 17, "deferred, blank-render, multibyte and math-with-string cases");
}
