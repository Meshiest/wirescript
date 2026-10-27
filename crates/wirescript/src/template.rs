//! Reusable compiled templates that can be instantiated with fresh, globally
//! unique node IDs.
//!
//! Cloning a [`Module`] directly shares its internal [`NodeId`]s across
//! copies, which collide when multiple instances are merged into one graph.
//! [`CompiledTemplate`] wraps a module and provides
//! [`CompiledTemplate::instantiate`], which deep-copies the module and mints a
//! globally-fresh numeric ID (`NodeId::fresh()`) for every internal node, so
//! instances are always disjoint regardless of call site.

use crate::collections::HashMap;
use crate::ir::{Module, Node, NodeId, PortRef, Wire};

/// A compiled module that can be stamped out multiple times with fresh IDs.
///
/// # External references
///
/// Some template nodes refer to variables or ports that live **outside** the
/// template, captured from an enclosing scope. These are listed in
/// `external_refs` as `(original_name, original_id)` pairs. When
/// [`instantiate`][CompiledTemplate::instantiate] is called the caller passes a
/// `captures` map `{ original_name -> caller_node_id }` so that external wires
/// are routed to the correct nodes in the merged graph rather than getting
/// phantom fresh IDs.
#[derive(Clone, Debug)]
pub struct CompiledTemplate {
    pub module: Module,
    /// Nodes that are *defined outside* this template and must be resolved via
    /// the `captures` argument to [`instantiate`].  Each entry is
    /// `(name_string, original_node_id)`.
    pub external_refs: Vec<(String, NodeId)>,
}

impl CompiledTemplate {
    /// Wrap a compiled module, populating `external_refs` from its
    /// `scope_captures` (`node_ids` referenced by wires but not in `nodes`).
    pub fn from_module(module: Module) -> Self {
        let external_refs = module
            .scope_captures
            .iter()
            .map(|id| (id.to_string(), *id))
            .collect();
        Self {
            module,
            external_refs,
        }
    }

    /// Instantiate the template, producing a fresh [`Module`] whose every
    /// internal node gets a fresh ID from `NodeId::fresh()`.
    ///
    /// * `captures`: maps each name in [`external_refs`] to the caller's node
    ///   ID that should be wired in its place. Pass `&HashMap::default()` when
    ///   there are no external refs.
    pub fn instantiate(&self, captures: &HashMap<String, NodeId>) -> Module {
        stamp_module(&self.module, &self.external_refs, captures).0
    }
}

/// Recursive body of [`CompiledTemplate::instantiate`]: stamps a
/// fresh copy of `module` by reference, so each chip child is copied once
/// instead of being deep-cloned into a throwaway `CompiledTemplate` first.
fn stamp_module(
    src: &Module,
    external_refs: &[(String, NodeId)],
    captures: &HashMap<String, NodeId>,
) -> (Module, HashMap<NodeId, NodeId>) {
    // 1. Build id_map: every internal node gets a fresh numeric ID
    let mut id_map: HashMap<NodeId, NodeId> =
        HashMap::with_capacity_and_hasher(src.nodes.len(), rustc_hash::FxBuildHasher);
    for old_id in src.nodes.keys() {
        id_map.insert(*old_id, NodeId::fresh());
    }

    // 2. Build external_map: captured scope vars use the caller's ID
    let mut external_map: HashMap<NodeId, NodeId> = HashMap::default();
    for (name, old_id) in external_refs {
        if let Some(&caller_id) = captures.get(name) {
            external_map.insert(*old_id, caller_id);
        }
    }

    // 3. Instantiate chip children FIRST, collecting each child's full
    // (recursive) old->new id map. This module's wires can reference a child's
    // boundary nodes directly: a parent gate wires into a nested chip's
    // MicrochipInput, and a nested chip's MicrochipOutput feeds a parent
    // consumer. Those child IDs are not in `id_map` (they live in the child
    // module), so without the child maps the wire remap below would pass them
    // through to the original template IDs, and every instance's boundary
    // wires would converge on the first instance's ports (fan-in collisions
    // the game fails to connect). Children are stamped before the wires so the
    // wires can point at the freshly-minted child boundary nodes.
    let mut descendant_map: HashMap<NodeId, NodeId> = HashMap::default();
    let mut new_chips: HashMap<NodeId, Module> = HashMap::default();
    for (old_chip_key, child_module) in &src.chips {
        let new_chip_key = id_map.get(old_chip_key).copied().unwrap_or(*old_chip_key);
        // Child external refs come from its `scope_captures`, the same
        // `(id_string, id)` pairs `from_module` derives. Merge the parent's
        // id_map into child captures so internal-to-parent nodes that the
        // child references get remapped correctly.
        let mut child_captures = captures.clone();
        let mut child_ext: Vec<(String, NodeId)> =
            Vec::with_capacity(child_module.scope_captures.len());
        for cap_id in &child_module.scope_captures {
            let name = cap_id.to_string();
            if let Some(&fresh) = id_map.get(cap_id) {
                child_captures.insert(name.clone(), fresh);
            }
            child_ext.push((name, *cap_id));
        }
        let (child_instance, child_map) = stamp_module(child_module, &child_ext, &child_captures);
        descendant_map.extend(child_map);
        new_chips.insert(new_chip_key, child_instance);
    }

    // 4. Helper: remap a single NodeId
    // Priority: this module's fresh ID > freshly-stamped descendant (child)
    // ID > external caller ID > pass through. The three maps have disjoint
    // key domains (own nodes / descendant nodes / captured externals), so the
    // priority only guards against accidental overlap.
    let remap = |id: NodeId| -> NodeId {
        if let Some(&fresh) = id_map.get(&id) {
            fresh
        } else if let Some(&child) = descendant_map.get(&id) {
            child
        } else if let Some(&caller) = external_map.get(&id) {
            caller
        } else {
            id
        }
    };

    // 5. Clone nodes with remapped IDs
    let mut new_nodes: HashMap<NodeId, Node> =
        HashMap::with_capacity_and_hasher(src.nodes.len(), rustc_hash::FxBuildHasher);
    for (old_id, node) in &src.nodes {
        let new_id = remap(*old_id);
        let new_chip_id = node.chip_id.map(&remap);
        let new_node = Node {
            id: new_id,
            chip_id: new_chip_id,
            // All other fields are either value types or IDs that live
            // outside the node map (port names are Syms for string names,
            // not node IDs, so they need no remapping).
            kind: node.kind,
            gate_class: node.gate_class,
            properties: std::sync::Arc::clone(&node.properties),
            ports: std::sync::Arc::clone(&node.ports),
            source_range: node.source_range.clone(),
            chain_id: node.chain_id,
            scope_id: node.scope_id,
            note: node.note,
        };
        new_nodes.insert(new_id, new_node);
    }

    // 6. Clone wires with remapped node IDs
    // Uses the combined remap, so a boundary wire targeting a child node
    // points at that child instance's fresh node, not the template's.
    let new_wires: Vec<Wire> = src
        .wires
        .iter()
        .map(|w| Wire {
            source: PortRef {
                node_id: remap(w.source.node_id),
                port: w.source.port,
            },
            target: PortRef {
                node_id: remap(w.target.node_id),
                port: w.target.port,
            },
        })
        .collect();

    // 7. Remap inputs / outputs vectors
    let new_inputs: Vec<NodeId> = src.inputs.iter().map(|id| remap(*id)).collect();
    let new_outputs: Vec<NodeId> = src.outputs.iter().map(|id| remap(*id)).collect();

    // 8. Assemble the new module
    let new_scope_captures: Vec<NodeId> = src
        .scope_captures
        .iter()
        .map(|old_id| remap(*old_id))
        .collect();

    let module = Module {
        name: src.name,
        nodes: new_nodes,
        wires: new_wires,
        chips: new_chips,
        inputs: new_inputs,
        outputs: new_outputs,
        scopes: src.scopes.clone(),
        template_key: src.template_key,
        scope_captures: new_scope_captures,
        // Remap dynamic-label host/source node ids per instance (empty for the
        // top-level-only labels shipped so far, but kept correct for the day a
        // chip body carries one).
        dynamic_labels: src
            .dynamic_labels
            .iter()
            .map(|(host, port)| {
                (
                    remap(*host),
                    PortRef {
                        node_id: remap(port.node_id),
                        port: port.port,
                    },
                )
            })
            .collect(),
        // Root-chip label lives only on the root module, never a chip template
        // instance; carried through for completeness (both are `None` here).
        root_label_override: src.root_label_override.clone(),
        root_dynamic_label: src.root_dynamic_label.map(|p| PortRef {
            node_id: remap(p.node_id),
            port: p.port,
        }),
        // Remapped like `dynamic_labels`: the runtime styles shipped so far
        // live on the root module only, so these are empty for a chip.
        dynamic_colors: remap_hosts(&src.dynamic_colors, &remap),
        dynamic_materials: remap_hosts(&src.dynamic_materials, &remap),
        root_style: src.root_style.clone(),
        value_names: HashMap::default(),
    };
    // Return this module's map PLUS all descendants' maps, so a caller
    // remapping boundary PortRefs (or a parent instantiation) can resolve
    // references to any node anywhere in the stamped subtree.
    let mut full_map = id_map;
    full_map.extend(descendant_map);
    (module, full_map)
}

/// A host -> source map with both ends renamed by `remap`.
fn remap_hosts(
    map: &HashMap<NodeId, PortRef>,
    remap: &impl Fn(NodeId) -> NodeId,
) -> HashMap<NodeId, PortRef> {
    map.iter()
        .map(|(host, port)| {
            (
                remap(*host),
                PortRef {
                    node_id: remap(port.node_id),
                    port: port.port,
                },
            )
        })
        .collect()
}

// Tests

#[cfg(test)]
mod tests;
