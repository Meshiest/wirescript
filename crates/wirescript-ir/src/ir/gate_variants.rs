//! Size each procedural gate brick (`PB_Gate_*`) to the ports it uses.
//!
//! A procedural gate is placed at one of a few fixed sizes, and a longer brick shows more
//! numbered ports. Every port stays reachable through the omniport at any size, so the
//! smallest variant that shows every port the node uses is chosen. This runs once the wire
//! set is final and records the choice on the node (`sym::GATE_VARIANT`), which layout's
//! cell reservation and emit's brick both read.

use crate::catalog::default_catalog;
use crate::collections::HashMap;
use crate::intern::{resolve, sym};
use crate::ir::{Literal, Module, NodeId, PortRef};

/// `(is_output, port name)` for every wired or valued port, keyed by node.
type UsedPorts = HashMap<NodeId, Vec<(bool, &'static str)>>;

pub fn select_gate_variants(module: &mut Module) {
    let mut used = UsedPorts::default();
    // A wire into a node of a chip can live in an ancestor module (a scope
    // capture), so the whole tree is gathered before any node is sized.
    module.walk(&mut |_, m| collect_wired(m, &mut used));
    module.walk_mut(&mut |_, m| assign(m, &used));
}

fn collect_wired(module: &Module, used: &mut UsedPorts) {
    let mut note = |p: &PortRef, out: bool| {
        used.entry(p.node_id)
            .or_default()
            .push((out, p.port.as_str()));
    };
    for w in &module.wires {
        note(&w.source, true);
        note(&w.target, false);
    }
    for src in module
        .dynamic_labels
        .values()
        .chain(module.root_dynamic_label.iter())
        .chain(module.style_sources())
    {
        note(src, true);
    }
}

fn assign(module: &mut Module, used: &UsedPorts) {
    let catalog = default_catalog();
    for (id, node) in &mut module.nodes {
        let Some(spec) = catalog.find_by_class(node.gate_class) else {
            continue;
        };
        if !spec.is_procedural() {
            continue;
        }
        // A constant baked into a port's data is a use of that port too.
        let valued = node
            .properties
            .keys()
            .map(|k| (false, resolve(*k)))
            .filter(|(_, name)| !name.starts_with('_'));
        let wired = used.get(id).into_iter().flatten().copied();
        let Some(idx) = spec.pick_variant(wired.chain(valued)) else {
            continue;
        };
        std::sync::Arc::make_mut(&mut node.properties)
            .insert(*sym::GATE_VARIANT, Literal::Int(idx as i64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(class: &str) -> &'static crate::catalog::GateSpec {
        default_catalog()
            .find_by_class(class)
            .expect("in the inventory")
    }

    #[test]
    fn smallest_variant_showing_every_used_numbered_port() {
        let q = spec("BrickComponentType_QueueTicks");
        let half = |idx| q.placed_half_size(Some(idx));
        let pick = |ports: &[(bool, &str)]| q.pick_variant(ports.iter().copied()).unwrap();
        // Two values fit the smallest brick; five need the {7,5,2} one.
        assert_eq!(
            half(pick(&[
                (false, "DataIn1"),
                (false, "DataIn2"),
                (true, "DataOut2")
            ]))
            .x,
            5
        );
        let five: Vec<_> = (1..=5)
            .flat_map(|i| [(false, format!("DataIn{i}")), (true, format!("DataOut{i}"))])
            .collect();
        let five: Vec<(bool, &str)> = five.iter().map(|(o, s)| (*o, s.as_str())).collect();
        assert_eq!(half(pick(&five)).x, 7);
        // An omniport-only port never grows the brick.
        assert_eq!(
            half(pick(&[(false, "TicksToWait"), (false, "ExecIn")])).x,
            5
        );
        // A sub-port is matched by its parent.
        assert_eq!(half(pick(&[(false, "DataIn8.X")])).x, 10);
    }

    #[test]
    fn every_procedural_gate_lists_its_sizes_smallest_first() {
        let gates: Vec<_> = default_catalog()
            .entries()
            .iter()
            .filter(|g| g.is_procedural())
            .collect();
        assert_eq!(gates.len(), 9, "procedural gates in the inventory");
        for g in gates {
            assert!(g.brick_asset.starts_with("PB_Gate_"), "{}", g.brick_asset);
            let xs: Vec<i32> = g.variants.iter().map(|v| v.half_size.x).collect();
            assert!(
                xs.windows(2).all(|w| w[0] < w[1]),
                "{}: {xs:?}",
                g.component.class
            );
            assert_eq!(
                g.variants[0].half_size, g.half_size,
                "{}",
                g.component.class
            );
        }
    }
}
