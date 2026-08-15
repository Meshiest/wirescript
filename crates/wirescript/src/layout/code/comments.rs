//! Own-line source comments: which plane renders each one, and where.

use super::*;

/// The source text's own indentation for a 1-based `line`, when a source
/// map is attached and covers that line. This is the statement's indent;
/// the first node on a line sits wherever its subexpression starts (the
/// RHS of a `let`, say), which is why the fallback over-indents.
///
/// The map only describes the entry file. A plane anchored on some other
/// file — an imported chip's body, or a root whose inlined `mod` bodies
/// outnumber its own statements — numbers its lines against that file, so
/// the map's rows would be someone else's; `None` sends those planes to
/// the node-column fallback.
pub(super) fn source_indent(opts: &LayoutOptions, anchor: &Arc<str>, line: i32) -> Option<u32> {
    let map = opts.source_map.as_ref()?;
    if map.file != *anchor {
        return None;
    }
    let idx = usize::try_from(line - 1).ok()?;
    map.line_indent.get(idx).copied()
}

/// The source-line span a plane covers in `anchor`: the first and last
/// line of the nodes it places literally, plus the spans of every chip
/// nested inside it — a chip's own rows stop where its children's begin,
/// so without the descendants the parent's span would have holes its
/// grandchildren fill. `None` when the plane has no such rows at all (a
/// ports-only chip, or one lowered from another file).
pub(super) fn line_span(module: &Module, anchor: &Arc<str>) -> Option<(i32, i32)> {
    let mut lo = i32::MAX;
    let mut hi = i32::MIN;
    for n in module.nodes.values() {
        if !is_spawnable(n) || is_edge_pin(n) || !has_range(n) || n.source_range.file != *anchor {
            continue;
        }
        let line = n.source_range.start.line as i32;
        lo = lo.min(line);
        hi = hi.max(line);
    }
    for child in module.chips.values() {
        if let Some((clo, chi)) = line_span(child, anchor) {
            lo = lo.min(clo);
            hi = hi.max(chi);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

/// Decide, once for the whole tree, which plane renders each own-line comment.
///
/// It cannot be decided locally. [`line_span`] is a min/max ENVELOPE over a
/// module's rows, and those rows are often sparse — a `mod` inlined at two
/// distant call sites, an anon chip partitioned out of a long handler — so a
/// module's window can cover most of the file while it occupies a handful of
/// lines in it. Sibling windows then overlap almost entirely, and since a
/// module's claim excludes only its own CHILDREN, every sibling renders every
/// comment in the overlap. On a real program that turned 951 source comments
/// into 2958 comment bricks.
///
/// The rule keeps the old eligibility test and adds a tie-break, so nothing
/// that already landed on the right plane moves:
///
/// * ELIGIBLE: the planes whose span brackets the line, exactly as before. A
///   plane can only claim a comment inside its own region, so a nested plane
///   can never reach back past its parent for a file's leading note.
/// * CHOSEN: among those, the plane whose next ROW comes soonest after the
///   comment — an own-line comment documents the code that follows it. Ties go
///   to the deeper plane (the more specific one), then to the lower chip id so
///   the answer never depends on map iteration order.
/// * FALLBACK: a comment no plane brackets belongs to the outermost one, which
///   is what keeps a file's leading and trailing notes.
pub(super) fn assign_comment_owners(
    root: &Module,
    opts: &LayoutOptions,
    anchor: &Arc<str>,
) -> HashMap<i32, Option<NodeId>> {
    let mut out: HashMap<i32, Option<NodeId>> = HashMap::default();
    let Some(map) = opts.source_map.as_ref() else {
        return out;
    };
    if map.comments.is_empty() || map.file != *anchor {
        return out;
    }

    // Every module in the tree with the anchor-file lines it actually
    // occupies — the rows themselves, not their envelope.
    fn walk(
        m: &Module,
        key: Option<NodeId>,
        depth: usize,
        anchor: &Arc<str>,
        out: &mut Vec<(Option<NodeId>, usize, Vec<i32>, Option<(i32, i32)>)>,
    ) {
        let mut lines: Vec<i32> = m
            .nodes
            .values()
            .filter(|n| {
                is_spawnable(n)
                    && !is_edge_pin(n)
                    && has_range(n)
                    && n.source_range.file == *anchor
            })
            .map(|n| n.source_range.start.line as i32)
            .collect();
        lines.sort_unstable();
        lines.dedup();
        out.push((key, depth, lines, line_span(m, anchor)));
        let mut ids: Vec<NodeId> = m.chips.keys().copied().collect();
        ids.sort();
        for id in ids {
            walk(&m.chips[&id], Some(id), depth + 1, anchor, out);
        }
    }
    let mut modules: Vec<(Option<NodeId>, usize, Vec<i32>, Option<(i32, i32)>)> = Vec::new();
    walk(root, None, 0, anchor, &mut modules);

    for c in &map.comments {
        if !c.own_line || c.text.is_empty() || c.in_array {
            continue;
        }
        let line = c.line as i32;
        // (distance to the next row, deeper first, then chip id).
        // Rank: nearest row wins, then the deeper plane, then the lower id.
        let rank = |dist: i32, depth: usize, key: Option<NodeId>| {
            (dist, std::cmp::Reverse(depth), key)
        };
        // Only the planes that bracket the line are in the running.
        let eligible: Vec<&(Option<NodeId>, usize, Vec<i32>, Option<(i32, i32)>)> = modules
            .iter()
            .filter(|(_, _, _, span)| span.is_some_and(|(lo, hi)| line >= lo && line <= hi))
            .collect();
        let mut best: Option<(i32, std::cmp::Reverse<usize>, Option<NodeId>)> = None;
        for (key, depth, lines, _) in &eligible {
            if let Some(&next) = lines.iter().find(|&&l| l > line) {
                let r = rank(next - line, *depth, *key);
                if best.is_none_or(|b| r < b) {
                    best = Some(r);
                }
            }
        }
        if best.is_none() {
            for (key, depth, lines, _) in &eligible {
                if let Some(&prev) = lines.iter().rev().find(|&&l| l < line) {
                    let r = rank(line - prev, *depth, *key);
                    if best.is_none_or(|b| r < b) {
                        best = Some(r);
                    }
                }
            }
        }
        out.insert(line, best.and_then(|(_, _, k)| k));
    }
    out
}

/// The own-line `//` comments this module renders, keyed by source line.
///
/// A comment belongs to the innermost plane whose rows bracket its line, so
/// lines inside a child chip's body are left to that chip. The outermost
/// module additionally claims comments outside every row's span — a file's
/// leading and trailing notes would otherwise be dropped. Trailing comments
/// (code then `//` on the same line) are never claimed: they would need a
/// row the code already occupies.
///
/// Planes anchored on a file other than the map's claim nothing: their rows
/// are lines of a file the map never saw, and bracketing a comment against
/// them would render it a second time on the wrong plane.
pub(super) fn claimed_comments<'a>(
    module: &Module,
    opts: &'a LayoutOptions,
    anchor: &Arc<str>,
) -> HashMap<i32, &'a SourceComment> {
    let mut out: HashMap<i32, &SourceComment> = HashMap::default();
    let Some(map) = opts.source_map.as_ref() else {
        return out;
    };
    if map.comments.is_empty() || map.file != *anchor {
        return out;
    }
    // Ownership was settled for the whole tree before any plane was laid
    // out, because sibling envelopes overlap and no plane can tell locally
    // whether a comment is really its own.
    if let Some(owners) = opts.comment_owner.as_ref() {
        for c in &map.comments {
            if !c.own_line || c.text.is_empty() || c.in_array {
                continue;
            }
            let line = c.line as i32;
            if owners.get(&line) == Some(&opts.self_chip) {
                out.entry(line).or_insert(c);
            }
        }
        return out;
    }

    // The outermost plane claims by exclusion, so it needs no window of its
    // own; a nested plane with no rows can bracket nothing.
    let window = if opts.nested {
        match line_span(module, anchor) {
            Some(span) => Some(span),
            None => return out,
        }
    } else {
        None
    };
    let child_spans: Vec<(i32, i32)> = module
        .chips
        .values()
        .filter_map(|child| line_span(child, anchor))
        .collect();

    for c in &map.comments {
        if !c.own_line || c.text.is_empty() || c.in_array {
            continue;
        }
        let line = c.line as i32;
        if child_spans.iter().any(|&(lo, hi)| line >= lo && line <= hi) {
            continue;
        }
        let mine = match window {
            Some((lo, hi)) => line >= lo && line <= hi,
            None => true,
        };
        if mine {
            out.entry(line).or_insert(c);
        }
    }
    out
}
