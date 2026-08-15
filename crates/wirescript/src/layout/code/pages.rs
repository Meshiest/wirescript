//! Vertical stacking: lines into bands, bands into pages, and the edge-pin
//! stacks that flank each page.

use super::*;

/// A vertical stack of lines capped at `band_height`; per-node offsets
/// relative to the band's top-left.
pub(super) struct Band {
    height: i32,
    width: i32,
    nodes: Vec<(NodeId, i32, i32)>,
    anns: Vec<(usize, i32, i32)>,
}

impl Band {
    fn new() -> Self {
        Band {
            height: 0,
            width: 0,
            nodes: Vec::new(),
            anns: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.anns.is_empty()
    }
}

/// Stack lines into bands greedily: a line whose bottom would exceed the
/// band budget closes the band (bands never split a line). Blank-run
/// gaps only count inside a band — a line opening a new band starts at
/// its top.
pub(super) fn assemble_bands(lines: Vec<LinePlan>, budgets: &CodeBudgets) -> Vec<Band> {
    let mut bands: Vec<Band> = Vec::new();
    let mut band = Band::new();
    for line in lines {
        let mut gap = if band.is_empty() { 0 } else { line.gap_before };
        if !band.is_empty() && band.height + gap + line.height > budgets.band_height {
            bands.push(std::mem::replace(&mut band, Band::new()));
            gap = 0;
        }
        let base = band.height + gap;
        for (id, x, down) in line.nodes {
            band.nodes.push((id, x, base + down));
        }
        for (idx, x, down) in line.anns {
            band.anns.push((idx, x, base + down));
        }
        band.height = base + line.height;
        band.width = band.width.max(line.width);
    }
    if !band.is_empty() {
        bands.push(band);
    }
    bands
}

/// One page's worth of bands placed left→right with `BAND_GUTTER` gaps;
/// per-node offsets relative to the page's top-left.
pub(super) struct PagePlan {
    width: i32,
    pub(super) nodes: Vec<(NodeId, i32, i32)>,
    pub(super) anns: Vec<(usize, i32, i32)>,
}

impl PagePlan {
    fn new() -> Self {
        PagePlan {
            width: 0,
            nodes: Vec::new(),
            anns: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.anns.is_empty()
    }
}

/// Pack bands into pages greedily: a band whose right edge would exceed
/// the plane budget closes the page.
pub(super) fn assemble_pages(bands: Vec<Band>, budgets: &CodeBudgets) -> Vec<PagePlan> {
    let mut pages: Vec<PagePlan> = Vec::new();
    let mut page = PagePlan::new();
    for band in bands {
        let mut gutter = if page.is_empty() { 0 } else { BAND_GUTTER };
        if !page.is_empty() && page.width + gutter + band.width > budgets.plane_width {
            pages.push(std::mem::replace(&mut page, PagePlan::new()));
            gutter = 0;
        }
        let origin = page.width + gutter;
        for (id, x, down) in band.nodes {
            page.nodes.push((id, origin + x, down));
        }
        for (idx, x, down) in band.anns {
            page.anns.push((idx, origin + x, down));
        }
        page.width = origin + band.width;
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// Post-centering extents of one emitted page, for edge-pin placement.
pub(super) struct PageInfo {
    pub(super) min_y: i32,
    pub(super) max_y: i32,
    pub(super) top_x: i32,
    pub(super) z: i32,
}

/// Stack every chip I/O port on its page's edges: Inputs one gutter left
/// of the page body's left edge, Outputs one gutter right of its right
/// edge. Each stack starts at the page's top row and descends one port
/// height at a time, in `edge_stack_key` order, so the column reads like
/// the chip's signature. A port lands on the page its first placed
/// neighbor occupies (page 0 when it has none), which keeps ports beside
/// their own body on paginated layouts.
///
/// `band_widths` is the gutter bus band already claimed on each page. The
/// input stack clears it, so the left edge reads input pins, then bus
/// lanes, then the body.
pub(super) fn place_edge_pins(
    module: &Module,
    adjacency: &Adjacency,
    mut pins: Vec<NodeId>,
    node_page: &HashMap<NodeId, usize>,
    band_widths: &[i32],
    page_infos: &mut Vec<PageInfo>,
    placements: &mut HashMap<NodeId, Placement>,
) {
    if pins.is_empty() {
        return;
    }
    pins.sort_by_key(|id| edge_stack_key(&module.nodes[id]));
    if page_infos.is_empty() {
        page_infos.push(PageInfo {
            min_y: 0,
            max_y: 0,
            top_x: 0,
            z: Z_PLANE,
        });
    }

    let offset_of = |id: &NodeId| {
        module
            .nodes
            .get(id)
            .map(|n| n.source_range.start.offset)
            .unwrap_or(usize::MAX)
    };

    // Descent cursor per (page, edge): the x of the last port placed there.
    let mut cursors: HashMap<(usize, bool), i32> = HashMap::default();
    for pin_id in pins {
        let node = &module.nodes[&pin_id];
        let is_output = node.kind == NodeKind::Output;
        let mut neighbors: Vec<NodeId> = if is_output {
            adjacency.producers.get(&pin_id).cloned()
        } else {
            adjacency.consumers.get(&pin_id).cloned()
        }
        .unwrap_or_default();
        neighbors.sort_by_key(|id| (offset_of(id), *id));

        let page_idx = neighbors
            .iter()
            .find(|id| node_page.contains_key(id))
            .map(|id| node_page[id])
            .unwrap_or(0);
        let info = &page_infos[page_idx];

        // Edge pins never join a line group, so nothing records a rotation
        // for them; the measurement still routes through `cell_half_size`
        // so this stays the one contract for a placed cell's footprint.
        let (hsx, hsy) = cell_half_size(node, NodeRotation::Deg0);
        let pin_h = hsx * 2;
        let pin_w = hsy * 2;
        let y = if is_output {
            info.max_y + BAND_GUTTER
        } else {
            info.min_y - BAND_GUTTER - pin_w - band_widths.get(page_idx).copied().unwrap_or(0)
        };

        // First port of a stack sits on the page's top row; each later one
        // drops by its own height.
        let cursor = cursors
            .entry((page_idx, is_output))
            .or_insert(info.top_x + pin_h);
        *cursor -= pin_h;
        placements.insert(
            pin_id,
            Placement {
                x: *cursor,
                y,
                z: info.z,
            },
        );
    }
}
