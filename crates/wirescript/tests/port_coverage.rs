//! Completeness guard: every builtin's `CallSpec` must expose ALL of its gate's
//! (non-exec) input and output ports — as a param, a spec output, or a record
//! field. Catches a gate that gained ports in a game update but whose builtin
//! wasn't updated to surface them.
//!
//! Matching is case-insensitive on the cleaned port name, so vector/color
//! split gates (`.x`/`.g` swizzle, which lower case-insensitively) count as
//! covered. A genuinely-unexposable port would need an entry in
//! `ALLOWED_GAPS` with a reason.
use std::collections::HashSet;

use wirescript::catalog::arrays::field_name_ref;
use wirescript::catalog::calls::calls;
use wirescript::catalog::default_catalog;
use wirescript::catalog::RawPortType;
use wirescript::ir::Type;

/// `builtin.PORT` pairs intentionally not surfaced (with the reason in-line).
const ALLOWED_GAPS: &[&str] = &[
    // (none — every catalog input/output is currently exposed)
];

fn clean_lower(s: &str) -> String {
    field_name_ref(s).to_ascii_lowercase()
}

/// Output-field aliases from lower::access::alias_output_field (InputReader).
fn alias_lower(f: &str) -> String {
    match f {
        "Forward" => "inputforward",
        "Right" => "inputright",
        "Up" => "inputup",
        "Pitch" => "inputpitch",
        "Yaw" => "inputyaw",
        "Roll" => "inputroll",
        "MouseWheel" => "inputmousewheel",
        o => return clean_lower(o),
    }
    .to_string()
}

#[test]
fn every_builtin_exposes_all_gate_ports() {
    let cat = default_catalog();
    let allowed: HashSet<&str> = ALLOWED_GAPS.iter().copied().collect();
    let mut gaps: Vec<String> = Vec::new();

    let mut names: Vec<&&str> = calls().keys().collect();
    names.sort();
    for name in names {
        let spec = &calls()[*name];
        let Some(gate) = cat.find_by_class(spec.gate_class) else {
            continue; // pseudo/internal gate absent from the inventory
        };

        let covered_in: HashSet<String> =
            spec.params.iter().map(|p| clean_lower(p.port.as_str())).collect();
        for p in gate.component.inputs.iter().filter(|p| p.ty != RawPortType::Exec) {
            let pair = format!("{name}.{}", p.name);
            if !covered_in.contains(&clean_lower(&p.name)) && !allowed.contains(pair.as_str()) {
                gaps.push(format!("{pair}  (input {:?})", p.ty));
            }
        }

        let mut covered_out: HashSet<String> = HashSet::new();
        for o in &spec.outputs {
            covered_out.insert(clean_lower(o.port.as_str()));
            if let Type::Record(fields) = &o.ty {
                for (f, _) in fields {
                    covered_out.insert(clean_lower(f));
                    covered_out.insert(alias_lower(f));
                }
            }
        }
        for p in gate.component.outputs.iter().filter(|p| p.ty != RawPortType::Exec) {
            let pair = format!("{name}.{}", p.name);
            if !covered_out.contains(&clean_lower(&p.name)) && !allowed.contains(pair.as_str()) {
                gaps.push(format!("{pair}  (output {:?})", p.ty));
            }
        }
    }

    gaps.sort();
    assert!(
        gaps.is_empty(),
        "these builtins don't expose all of their gate's ports (add the param/output, \
         or list it in ALLOWED_GAPS with a reason):\n{}",
        gaps.join("\n")
    );
}

/// Event data outputs the language intentionally does not bind (with reason).
const EVENT_ALLOWED_GAPS: &[&str] = &[
    // The join/left/chat gates expose UserId; events bind it as `userId` where
    // useful. Nothing else is dropped.
    //
    // The Clock gate's sole output `Pulse` is its exec trigger (the handler body
    // chains from it via the event's `exec_out`), not a data payload — the dump
    // types it `any` rather than `exec`, so it needs an explicit allowance here.
    "Clock.Pulse",
];

#[test]
fn every_event_binds_all_gate_outputs() {
    use wirescript::catalog::events::events;
    let cat = default_catalog();
    let allowed: HashSet<&str> = EVENT_ALLOWED_GAPS.iter().copied().collect();
    let mut gaps: Vec<String> = Vec::new();

    let mut names: Vec<&&str> = events().keys().collect();
    names.sort();
    for name in names {
        let evt = &events()[*name];
        let Some(gate) = cat.find_by_class(evt.gate_class) else {
            continue;
        };
        let bound: HashSet<String> = evt.data.iter().map(|d| clean_lower(d.port)).collect();
        for p in gate.component.outputs.iter().filter(|p| p.ty != RawPortType::Exec) {
            let pair = format!("{name}.{}", p.name);
            if !bound.contains(&clean_lower(&p.name)) && !allowed.contains(pair.as_str()) {
                gaps.push(format!("{pair}  (output {:?})", p.ty));
            }
        }
    }

    gaps.sort();
    assert!(
        gaps.is_empty(),
        "these events don't bind all of their gate's data outputs (add a binding, \
         or list it in EVENT_ALLOWED_GAPS with a reason):\n{}",
        gaps.join("\n")
    );
}
