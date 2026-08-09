//! Reusable compiled templates that can be instantiated with fresh, globally
//! unique node IDs.
//!
//! The key problem this solves: when you clone a [`Module`] directly the
//! internal [`NodeId`]s are shared across copies, causing collisions when
//! multiple instances are merged into one graph.  [`CompiledTemplate`] wraps a
//! module and provides [`CompiledTemplate::instantiate`], which deep-copies the
//! module and mints a globally-fresh numeric ID (`NodeId::fresh()`) for every
//! internal node, so instances are always disjoint regardless of call site.

use crate::collections::HashMap;

use crate::emit::Placement;
use crate::ir::{Module, Node, NodeId, PortRef, Wire};

/// Axis-aligned bounding box in grid space, giving the spatial extent of a
/// template's nodes (populated by the layout pass, not by compilation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateBounds {
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_z: i32,
}

/// A compiled module that can be stamped out multiple times with fresh IDs.
///
/// # External references
///
/// Some template nodes refer to variables or ports that live **outside** the
/// template — captured from an enclosing scope.  These are listed in
/// `external_refs` as `(original_name, original_id)` pairs.  When
/// [`instantiate`][CompiledTemplate::instantiate] is called the caller passes a
/// `captures` map `{ original_name → caller_node_id }` so that external wires
/// are routed to the correct nodes in the merged graph rather than getting
/// phantom fresh IDs.
#[derive(Clone, Debug)]
pub struct CompiledTemplate {
    pub module: Module,
    /// Spatial extent of the template (filled in by layout; `None` before layout).
    pub bounds: Option<TemplateBounds>,
    /// Per-node placement hints (filled in by layout; `None` before layout).
    pub placements: Option<HashMap<NodeId, Placement>>,
    /// Nodes that are *defined outside* this template and must be resolved via
    /// the `captures` argument to [`instantiate`].  Each entry is
    /// `(name_string, original_node_id)`.
    pub external_refs: Vec<(String, NodeId)>,
}

impl CompiledTemplate {
    /// Wrap a compiled module, populating `external_refs` from its
    /// `scope_captures` (node_ids referenced by wires but not in `nodes`).
    pub fn from_module(module: Module) -> Self {
        let external_refs = module
            .scope_captures
            .iter()
            .map(|id| (id.to_string(), *id))
            .collect();
        Self {
            module,
            bounds: None,
            placements: None,
            external_refs,
        }
    }

    /// Instantiate the template, producing a fresh [`Module`] whose every
    /// internal node ID is `intern("{prefix}/{old_name}")`.
    ///
    /// * `prefix` — a unique string that distinguishes this instance from all
    ///   others (e.g. `"inst0"`, `"foo/bar/2"`).
    /// * `captures` — maps each name in [`external_refs`] to the caller's node
    ///   ID that should be wired in its place.  Pass `&HashMap::default()` when
    ///   there are no external refs.
    pub fn instantiate(&self, prefix: &str, captures: &HashMap<String, NodeId>) -> Module {
        self.instantiate_with_map(prefix, captures).0
    }

    /// Like [`instantiate`] but also returns the `old_id → new_id` mapping
    /// so callers can remap PortRefs that reference template-internal nodes
    /// (e.g. exec chain entry/exit points).
    pub fn instantiate_with_map(
        &self,
        _prefix: &str,
        captures: &HashMap<String, NodeId>,
    ) -> (Module, HashMap<NodeId, NodeId>) {
        stamp_module(&self.module, &self.external_refs, captures)
    }
}

/// Recursive body of [`CompiledTemplate::instantiate_with_map`]: stamp a fresh
/// copy of `module` by reference — the old path deep-cloned every chip child
/// into a throwaway `CompiledTemplate` before copying it AGAIN into the
/// instance.
fn stamp_module(
    src: &Module,
    external_refs: &[(String, NodeId)],
    captures: &HashMap<String, NodeId>,
) -> (Module, HashMap<NodeId, NodeId>) {
    // --- 1. Build id_map: every internal node gets a fresh numeric ID ----
    let mut id_map: HashMap<NodeId, NodeId> =
        HashMap::with_capacity_and_hasher(src.nodes.len(), Default::default());
    for old_id in src.nodes.keys() {
        id_map.insert(*old_id, NodeId::fresh());
    }

    // --- 2. Build external_map: captured scope vars use the caller's ID --
    let mut external_map: HashMap<NodeId, NodeId> = HashMap::default();
    for (name, old_id) in external_refs {
        if let Some(&caller_id) = captures.get(name) {
            external_map.insert(*old_id, caller_id);
        }
    }

    // --- 3. Instantiate chip children FIRST, collecting each child's FULL
    // (recursive) old→new id map. This module's wires can reference a child's
    // boundary nodes directly — a parent gate wires into a nested chip's
    // MicrochipInput, and a nested chip's MicrochipOutput feeds a parent
    // consumer. Those child IDs are not in `id_map` (they live in the child
    // module), so without the child maps the wire remap below would pass them
    // through to the ORIGINAL template IDs, making every instance's boundary
    // wires converge on the first instance's ports (fan-in collisions the game
    // then fails to connect). Children must be stamped before the wires so the
    // wires can point at the freshly-minted child boundary nodes.
    let mut descendant_map: HashMap<NodeId, NodeId> = HashMap::default();
    let mut new_chips: HashMap<NodeId, Module> = HashMap::default();
    for (old_chip_key, child_module) in &src.chips {
        let new_chip_key = id_map.get(old_chip_key).copied().unwrap_or(*old_chip_key);
        // Child external refs come from its `scope_captures` — the same
        // `(id_string, id)` pairs `from_module` derives. Merge parent's
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

    // --- 4. Helper: remap a single NodeId -----------------------------------
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

    // --- 5. Clone nodes with remapped IDs ----------------------------------
    let mut new_nodes: HashMap<NodeId, Node> =
        HashMap::with_capacity_and_hasher(src.nodes.len(), Default::default());
    for (old_id, node) in &src.nodes {
        let new_id = remap(*old_id);
        let new_chip_id = node.chip_id.map(|c| remap(c));
        let new_node = Node {
            id: new_id,
            chip_id: new_chip_id,
            // All other fields are either value types or IDs that live
            // outside the node map (port names are Syms for string names,
            // not node IDs, so they need no remapping).
            kind: node.kind,
            gate_class: node.gate_class,
            properties: node.properties.clone(),
            ports: node.ports.clone(),
            source_range: node.source_range.clone(),
            chain_id: node.chain_id,
            scope_id: node.scope_id,
            note: node.note.clone(),
        };
        new_nodes.insert(new_id, new_node);
    }

    // --- 6. Clone wires with remapped node IDs -----------------------------
    // Uses the combined remap, so a boundary wire targeting a child node now
    // points at that child instance's fresh node instead of the template's.
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

    // --- 7. Remap inputs / outputs vectors ---------------------------------
    let new_inputs: Vec<NodeId> = src.inputs.iter().map(|id| remap(*id)).collect();
    let new_outputs: Vec<NodeId> = src.outputs.iter().map(|id| remap(*id)).collect();

    // --- 8. Assemble the new module ----------------------------------------
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
        // instance — carried through for correctness (both are `None` here).
        root_label_override: src.root_label_override.clone(),
        root_dynamic_label: src.root_dynamic_label.map(|p| PortRef {
            node_id: remap(p.node_id),
            port: p.port,
        }),
    };
    // Return this module's map PLUS all descendants' maps, so a caller
    // remapping boundary PortRefs (or a parent instantiation) can resolve
    // references to any node anywhere in the stamped subtree.
    let mut full_map = id_map;
    full_map.extend(descendant_map);
    (module, full_map)
}

/// Cached result of an inline mod's first expansion.
///
/// Stores the delta (nodes + wires + chips created during expansion) as a
/// `CompiledTemplate`, plus the exec chain entry/exit and output port so the
/// caller can splice the instantiated copy into its own exec chain.
#[derive(Clone, Debug)]
pub struct InlineModEntry {
    pub template: CompiledTemplate,
    /// PortRef for the first exec-consuming node in the template.
    /// The caller wires its `current_exec` here.
    pub exec_entry: Option<PortRef>,
    /// PortRef for the last exec-producing node in the template.
    /// Becomes the caller's new `current_exec` after merging.
    pub exec_exit: Option<PortRef>,
    /// The PortRef that carries the return value (for single-output mods).
    pub output_port: Option<PortRef>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
