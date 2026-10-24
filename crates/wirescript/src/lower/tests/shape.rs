//! Outcome normalizations and IR invariants shared by the differential tests.

use super::shadow_differential::Outcome;
use crate::ir::port_registry::WirePort;

/// `o` with every gate property whose `key=` starts with one of `prefixes`
/// removed from its shape, the shape re-sorted.
fn without_props(mut o: Outcome, prefixes: &[&str]) -> Outcome {
    for s in &mut o.shape {
        *s = s
            .split(' ')
            .filter(|t| !t.is_empty() && !prefixes.iter().any(|p| t.starts_with(p)))
            .collect::<Vec<_>>()
            .join(" ");
    }
    o.shape.sort();
    o
}

/// `o` without the `_bindname=` a `let` labels its gate with.
pub(super) fn without_bindnames(o: Outcome) -> Outcome {
    without_props(o, &["_bindname="])
}

/// `o` without the names a gate is labelled with (`_bindname=`, `_label=`) and
/// without the text-display components the save labels gates with.
pub(super) fn without_names(o: Outcome) -> Outcome {
    let mut o = without_props(o, &["_bindname=", "_label="]);
    o.comps
        .retain(|c| !c.starts_with("BrickComponentData_TextDisplay"));
    o
}

/// No input port in `m` or a chip below it has two drivers: two wires into one
/// port fail at load in game. Synthetic `Layout` edges are not wires.
#[track_caller]
pub(super) fn assert_no_fan_in(m: &crate::ir::Module, what: &str) {
    m.walk(&mut |path, sub| {
        let mut seen: crate::collections::HashSet<(crate::ir::NodeId, WirePort)> =
            Default::default();
        for w in sub
            .wires
            .iter()
            .filter(|w| w.target.port != WirePort::Layout)
        {
            assert!(
                seen.insert((w.target.node_id, w.target.port)),
                "{what}: {}.{:?} in {:?} (chip path {path:?}) has two drivers",
                w.target.node_id,
                w.target.port,
                crate::intern::resolve(sub.name)
            );
        }
    });
}
