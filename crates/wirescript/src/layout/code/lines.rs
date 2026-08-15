//! The intra-line engine: group a line's nodes into depth columns, measure
//! the result, and soft-wrap it into sub-rows.

use super::*;

struct SubRow {
    height: i32,
    entries: Vec<(NodeId, i32)>,
}

/// One connected run of a line's nodes, arranged as a column per
/// dependency depth: the nodes feeding a column sit in the column to its
/// left, and a column's own nodes stack into rows.
struct LineGroup {
    width: i32,
    /// `(node, x offset inside the group, row inside the group)`.
    nodes: Vec<(NodeId, i32, usize)>,
}

/// Split a line's entries into connected groups, each arranged as
/// columns.
///
/// Column 0 is the exec spine: every node [`on_exec_spine`] accepts is
/// PINNED there, whatever its position in the value graph. A statement's
/// sink — the `Var_Set` under an expression, say — therefore heads its
/// line on the left and takes its value from the right. An exec gate whose
/// value IS read on the line, such as a variable read inside an
/// expression, is not a sink and stays in the value columns.
///
/// Everything else is a value node, columned by dependency depth. The
/// depth graph runs along the wire, from a node to what it feeds, so a
/// value node's depth is the longest path from a source and it always
/// lands right of everything feeding it: operands flow left to right into
/// the consumer that reads them. Those columns start one step right of the
/// pinned column, so a value node never shares column 0 with the spine.
/// Only wires with both endpoints on this line take part, so a line with
/// no nesting comes back as a run of single-node groups: the flat
/// left-to-right row.
///
/// Exec wires are left out of the depth graph. They run from a statement
/// to the one after it, so treating them as operand edges would chain a
/// multi-statement line into one group and push each statement a column
/// right of the last. Without them, sequenced statements are separate
/// groups and keep source order, while their operands still tree out to
/// the right.
///
/// The rotation decision is made here, once the columns are known: a gate
/// on the spine faces down. The record happens before the column is
/// measured, so the swapped footprint is what gets reserved.
fn line_groups(
    entries: &[NodeId],
    module: &Module,
    adjacency: &Adjacency,
    rotations: &mut HashMap<NodeId, NodeRotation>,
) -> Vec<LineGroup> {
    let n = entries.len();
    let ord: HashMap<NodeId, usize> = entries.iter().enumerate().map(|(i, id)| (*id, i)).collect();

    // `operands[i]`: the nodes feeding entry i — its depth predecessors.
    // `consumers[i]`: the nodes entry i feeds, which sit right of it.
    let mut operands: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut seen: HashSet<(usize, usize)> = HashSet::default();
    for (ci, id) in entries.iter().enumerate() {
        for p in adjacency.value_producers.get(id).into_iter().flatten() {
            let Some(&pi) = ord.get(p) else { continue };
            if pi == ci || !seen.insert((ci, pi)) {
                continue;
            }
            operands[ci].push(pi);
            consumers[pi].push(ci);
        }
    }

    // Longest-path depth over that graph, swept in entry order.
    let mut waiting: Vec<usize> = operands.iter().map(Vec::len).collect();
    let mut depth: Vec<i32> = vec![0; n];
    let mut placed: Vec<bool> = vec![false; n];
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| waiting[i] == 0).collect();
    let mut done = 0usize;
    while done < n {
        let i = match ready.iter().next().copied() {
            Some(i) => i,
            // A cycle among a line's own wires would stall the sweep. Break
            // it at the earliest node still waiting so the pass terminates;
            // the edges still holding that node back are ignored for depth.
            None => (0..n).find(|&i| !placed[i]).expect("a node is unplaced"),
        };
        ready.remove(&i);
        let d = operands[i]
            .iter()
            .filter(|&&o| placed[o])
            .map(|&o| depth[o] + 1)
            .max()
            .unwrap_or(0);
        depth[i] = d;
        placed[i] = true;
        done += 1;
        for &c in &consumers[i] {
            if placed[c] {
                continue;
            }
            waiting[c] -= 1;
            if waiting[c] == 0 {
                ready.insert(c);
            }
        }
    }

    // Depth becomes a column: the exec spine is pinned to column 0 and
    // every value node sits one step right of it, ordered by its own
    // depth. A spine node's depth is discarded — it is a sink in the value
    // graph and would otherwise land right of the operands it reads.
    let column: Vec<usize> = (0..n)
        .map(|i| {
            if on_exec_spine(&module.nodes[&entries[i]], &consumers[i]) {
                0
            } else {
                depth[i] as usize + 1
            }
        })
        .collect();

    // Connected groups, so unrelated runs on the same line never share a
    // column and can pack side by side instead.
    let mut root: Vec<usize> = (0..n).collect();
    for (i, ops) in operands.iter().enumerate() {
        for &o in ops {
            let (a, b) = (uf_find(&mut root, i), uf_find(&mut root, o));
            if a != b {
                root[a.max(b)] = a.min(b);
            }
        }
    }
    let mut buckets: HashMap<usize, Vec<usize>> = HashMap::default();
    for i in 0..n {
        buckets.entry(uf_find(&mut root, i)).or_default().push(i);
    }
    let mut members: Vec<Vec<usize>> = buckets.into_values().collect();
    members.sort_by_key(|m| m[0]);

    let mut row_of: Vec<usize> = vec![0; n];
    let mut groups: Vec<LineGroup> = Vec::with_capacity(members.len());
    for m in members {
        let deepest = m.iter().map(|&i| column[i]).max().unwrap_or(0);
        let mut cols: Vec<Vec<usize>> = vec![Vec::new(); deepest + 1];
        for &i in &m {
            cols[column[i]].push(i);
        }
        let mut nodes = Vec::with_capacity(m.len());

        // Pass 1 — the horizontal frame, in column order. Column 0 keeps the
        // group's left edge whether or not its nodes end up on the top row,
        // so the drop below is purely vertical and the spine stays a straight
        // column down the left margin.
        //
        // A group with no spine node leaves column 0 empty; an empty column
        // claims nothing, so its value nodes still start flush at the group's
        // left edge — bar the tap reserve every occupied column opens in
        // front of itself.
        let mut x = 0i32;
        let mut col_x: Vec<i32> = vec![0; cols.len()];
        for (col_idx, col) in cols.iter().enumerate() {
            if col.is_empty() {
                continue;
            }
            x += TAP_RESERVE;
            col_x[col_idx] = x;
            let mut col_w = 0i32;
            for &i in col.iter() {
                let node = &module.nodes[&entries[i]];
                let rot = if col_idx == 0 && is_spine_exec_gate(node, &consumers[i]) {
                    rotations.insert(entries[i], NodeRotation::Deg90);
                    NodeRotation::Deg90
                } else {
                    NodeRotation::Deg0
                };
                col_w = col_w.max(cell_half_size(node, rot).1 * 2);
            }
            x += col_w;
        }

        // Pass 2 — the value columns take the group's UPPER rows, filling
        // left to right. A node follows the row of the operands feeding it,
        // so a subtree stays together rather than interleaving with its
        // siblings'; those operands sit in a column already filled.
        let mut value_rows = 0usize;
        for (col_idx, col) in cols.iter_mut().enumerate().skip(1) {
            if col.is_empty() {
                continue;
            }
            col.sort_by_key(|&i| (operands[i].iter().map(|&o| row_of[o]).min().unwrap_or(0), i));
            for (row, &i) in col.iter().enumerate() {
                row_of[i] = row;
                nodes.push((entries[i], col_x[col_idx], row));
            }
            value_rows = value_rows.max(col.len());
        }

        // Pass 3 — and the statement's own gates drop BELOW all of it, still
        // in column 0. Values flow left to right into a sink, so a sink level
        // with its operands is read by a run coming back leftward across the
        // line; from the bottom row the same run is a descent.
        //
        // `value_rows` is 0 for a group that is nothing but its statement —
        // a call whose arguments all inlined, say — so that sink stays on the
        // top row and the line does not grow a row for nothing. A spine node
        // reads nothing back in line (that is what `on_exec_spine` means), so
        // it never appears as another node's operand and no value column's
        // row ordering depends on where it lands.
        cols[0].sort_unstable();
        for (k, &i) in cols[0].iter().enumerate() {
            let row = value_rows + k;
            row_of[i] = row;
            nodes.push((entries[i], col_x[0], row));
        }
        groups.push(LineGroup { width: x, nodes });
    }
    groups
}

fn uf_find(root: &mut [usize], mut x: usize) -> usize {
    while root[x] != x {
        root[x] = root[root[x]];
        x = root[x];
    }
    x
}

/// Lay out one line's groups left to right, soft-wrapping into a new
/// indented band of sub-rows whenever the next group would push the
/// current band past `budgets.line_width`.
fn measure_line(
    entries: &[NodeId],
    module: &Module,
    adjacency: &Adjacency,
    budgets: &CodeBudgets,
    head_col: u32,
    rotations: &mut HashMap<NodeId, NodeRotation>,
) -> Vec<SubRow> {
    let indent_px = head_col as i32 * INDENT_UNIT;
    let mut rows: Vec<Vec<(NodeId, i32)>> = Vec::new();
    let mut cursor = indent_px;
    let mut band_base = 0usize;
    let mut band_has_entry = false;

    for group in line_groups(entries, module, adjacency, rotations) {
        if band_has_entry && cursor + group.width - indent_px > budgets.line_width {
            band_base = rows.len();
            cursor = indent_px + CONTINUATION_INDENT;
        }
        for (id, dx, row) in group.nodes {
            let row = band_base + row;
            while rows.len() <= row {
                rows.push(Vec::new());
            }
            rows[row].push((id, cursor + dx));
        }
        cursor += group.width;
        band_has_entry = true;
    }

    rows.into_iter()
        .map(|entries| {
            // Every group on this line has already recorded its rotations,
            // so a rotated gate contributes its swapped height here.
            let height = entries
                .iter()
                .map(|(id, _)| cell_half_size(&module.nodes[id], rotation_of(rotations, id)).0 * 2)
                .max()
                .unwrap_or(0);
            SubRow { height, entries }
        })
        .collect()
}

/// One measured source line: its blank-run gap, total height across
/// sub-rows, width (max x extent incl. indent), and per-node offsets
/// relative to the line's own top-left. `anns` holds the same offsets for
/// the line's comment label, keyed by index into the layout's text list.
pub(super) struct LinePlan {
    pub(super) gap_before: i32,
    pub(super) height: i32,
    pub(super) width: i32,
    pub(super) nodes: Vec<(NodeId, i32, i32)>,
    pub(super) anns: Vec<(usize, i32, i32)>,
}

pub(super) fn plan_line(
    entries: &[NodeId],
    module: &Module,
    adjacency: &Adjacency,
    budgets: &CodeBudgets,
    head_col: u32,
    gap_before: i32,
    annotation: Option<usize>,
    rotations: &mut HashMap<NodeId, NodeRotation>,
) -> LinePlan {
    let subrows = measure_line(entries, module, adjacency, budgets, head_col, rotations);
    let mut nodes = Vec::new();
    let mut height = 0i32;
    let mut width = 0i32;
    for subrow in &subrows {
        for &(id, x) in &subrow.entries {
            let (_, hsy) = cell_half_size(&module.nodes[&id], rotation_of(rotations, &id));
            nodes.push((id, x, height));
            width = width.max(x + hsy * 2);
        }
        height += subrow.height;
    }
    // The label starts at the line's own indent and takes a sub-row of its
    // own below any gates, so it never lands on top of one.
    let mut anns = Vec::new();
    if let Some(idx) = annotation {
        let x = head_col as i32 * INDENT_UNIT;
        anns.push((idx, x, height));
        height += ANNOTATION_SIZE;
        width = width.max(x + ANNOTATION_SIZE);
    }
    LinePlan {
        gap_before,
        height,
        width,
        nodes,
        anns,
    }
}
