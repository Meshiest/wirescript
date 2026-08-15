//! Stateless per-node classification and measurement.

use super::*;

pub(super) fn is_spawnable(node: &Node) -> bool {
    matches!(
        node.kind,
        NodeKind::Gate | NodeKind::Event | NodeKind::Input | NodeKind::Output | NodeKind::Chip
    )
}

/// A node has a usable source range if it carries a real end offset or a
/// non-empty file — synthetic nodes built with `SourceRange::default()`
/// have neither.
pub(super) fn has_range(n: &Node) -> bool {
    n.source_range.end.offset > 0 || !n.source_range.file.is_empty()
}

/// Every chip I/O port stacks on a plane edge — declared `in`/`out` ports
/// and the lowering pass's synthesized boundary pins alike — so the plane
/// reads like the chip's signature regardless of where the ports were
/// written in the source.
pub(super) fn is_edge_pin(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Input | NodeKind::Output)
}

/// Order within one edge stack: declared ports first in signature order
/// (every port of a signature shares its declaration offset, so the id —
/// allocated left to right across the signature — is what separates them),
/// then synthesized boundary pins by label.
pub(super) fn edge_stack_key(n: &Node) -> (u8, usize, String, Option<u64>, NodeId) {
    if has_range(n) {
        (0, n.source_range.start.offset, String::new(), None, n.id)
    } else {
        let (stem, num) = label_sort_key(&port_label(n));
        (1, 0, stem, num, n.id)
    }
}

/// A label split into its non-numeric stem and trailing number, so a stack
/// of `ext1 ext2 … ext10` orders by count rather than by digit — plain
/// string order puts `ext10` between `ext1` and `ext2`.
pub(super) fn label_sort_key(label: &str) -> (String, Option<u64>) {
    let stem_len = label.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let digits = &label[stem_len..];
    // A run too long for u64 keeps the whole label as its stem, which is
    // still a total order — just the string one.
    match digits.parse::<u64>() {
        Ok(num) => (label[..stem_len].to_string(), Some(num)),
        Err(_) => (label.to_string(), None),
    }
}

pub(super) fn port_label(n: &Node) -> String {
    match n.properties.get(&*sym::PORT_LABEL) {
        Some(Literal::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The most frequent file among spawnable nodes with a real range, ties
/// broken lexically for determinism.
pub(super) fn anchor_file(spawnable: &[(&NodeId, &Node)]) -> Arc<str> {
    let mut counts: HashMap<Arc<str>, usize> = HashMap::default();
    for (_, n) in spawnable {
        if has_range(n) {
            *counts.entry(n.source_range.file.clone()).or_insert(0) += 1;
        }
    }
    let mut files: Vec<(Arc<str>, usize)> = counts.into_iter().collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    files
        .into_iter()
        .next()
        .map(|(f, _)| f)
        .unwrap_or_else(|| "".into())
}

/// A node's quarter-turn, defaulting to `Deg0` for anything unrotated.
pub(super) fn rotation_of(rotations: &HashMap<NodeId, NodeRotation>, id: &NodeId) -> NodeRotation {
    rotations.get(id).copied().unwrap_or_default()
}

/// The footprint a node reserves, as `(half extent down the rows, half
/// extent across the columns)` — the same order [`brick_half_size`]
/// returns.
///
/// A `Deg90` brick is turned a quarter-turn within the plane, so its two
/// half-sizes swap. EVERY site that measures a cell must go through this
/// rather than `brick_half_size`, and emit must apply the same swap when
/// it centers the brick: `brdb::Brick::local_bounds()` is rotation-blind,
/// so a disagreement is invisible to the overlap checker and shows up
/// only as bricks silently dropped by the game at load.
pub(super) fn cell_half_size(node: &Node, rotation: NodeRotation) -> (i32, i32) {
    let (hsx, hsy) = brick_half_size(node);
    match rotation {
        // Only the QUARTER turns swap. A half turn lands the brick the way
        // round it started, so Deg180 reserves exactly the Deg0 cell and
        // Deg270 exactly the Deg90 one.
        NodeRotation::Deg0 | NodeRotation::Deg180 => (hsx, hsy),
        NodeRotation::Deg90 | NodeRotation::Deg270 => (hsy, hsx),
    }
}
