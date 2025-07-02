use std::collections::{HashMap, HashSet};

use crate::bearilog::compiler::{CompiledModule, GateKind};

pub type WireMap = HashMap<usize, HashSet<usize>>;

/// Obtain a map of src gate index to dst gate index, ignoring buffers as destinations
pub fn build_dst_to_src(module: &CompiledModule) -> WireMap {
    let mut map = WireMap::new();
    for w in &module.wires {
        // Ignore buffers as destinations
        if matches!(w.dst.gate.kind, GateKind::Buffer) {
            continue;
        }

        map.entry(w.dst.gate.index)
            .or_default()
            .insert(w.src.gate.index);
    }

    for (_, sub_module) in &module.sub_modules {
        let sub_map = build_dst_to_src(sub_module);
        for (dst, srcs) in sub_map {
            map.entry(dst).or_default().extend(srcs);
        }
    }

    map
}
