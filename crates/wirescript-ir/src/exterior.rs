//! Bricks placed around the program's chip instead of inside its planes: each
//! `@display`'s panel (one strip per row carrying that row's Text Decal, a
//! border, and the back chip that rebuilds the rows) and each `@seat`'s seat.
//!
//! These are nodes of the root module marked with [`sym::EXTERIOR`]. Layout
//! leaves them out ([`without_exterior`]) and emit places them from an
//! [`ExteriorPlan`] on the outer grid.
//!
//! Outer-grid axes, seen from above: +X up the screen, +Y right. The planes
//! face world -X, so the program is watched from -X with +Y on the right;
//! displays face the same way.

use brdb::{AsBrdbValue, Direction, Position, Rotation, Vector3f, World, assets::LiteralComponent};

use crate::collections::{HashMap, HashSet};
use crate::intern::{Sym, sym};
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, Type, Wire};

/// [`sym::EXTERIOR`] of a display's back chip.
pub const DISPLAY: &str = "display";
/// [`sym::EXTERIOR`] of one of a display's row Text Decals.
pub const DISPLAY_ROW: &str = "display_row";
/// [`sym::EXTERIOR`] of a seat.
pub const SEAT: &str = "seat";
/// [`sym::EXTERIOR`] of an invisible rerouter a wire between the program and
/// a display's back chip runs through, so no wire shows across the gap.
pub const DISPLAY_WIRE: &str = "display_wire";

/// The text component a row is drawn with.
pub const TEXT_DISPLAY: &str = "Component_TextDisplay";
/// The brick a row strip and a border bar are: a micro brick, whose size is
/// free in grid units. A `PB_DefaultBrick` only takes whole studs and plates,
/// and the game drops one of any other size at load.
pub const PANEL_BRICK: brdb::BString = brdb::assets::bricks::PB_DEFAULT_MICRO_BRICK;
/// The seat component.
pub const SEAT_COMPONENT: &str = "Component_Internal_Seat";

/// A display's color when it has no `@color`: near black.
pub const DEFAULT_DISPLAY_COLOR: Literal = Literal::LinearColor {
    r: 0.02,
    g: 0.02,
    b: 0.03,
    a: 1.0,
};

/// The program's chip brick's half-height.
const CHIP_HALF_HEIGHT: i32 = 2;
/// The program's chip brick's half-width.
const CHIP_HALF: i32 = 5;
/// Gap between the chip's top and a display panel's bottom, between panels
/// side by side, and between a panel's top and the root plane.
const GAP: i32 = 10;
/// A row strip's half-depth; the border's is one more, so it stands proud.
const STRIP_HALF_DEPTH: i32 = 2;
/// A rerouter's half-size: a 2-unit cube.
const REROUTER_HALF: i32 = 1;
/// `B_Seat` half-size.
const SEAT_HALF: i32 = 10;
const SEAT_HALF_HEIGHT: i32 = 11;
/// Distance from the chip's center to a seat row's center line: clear of the
/// pin rerouters along the chip's edge.
const SEAT_DISTANCE: i32 = CHIP_HALF + GAP + SEAT_HALF;
/// Spacing between neighboring seats in a row.
const SEAT_PITCH: i32 = 2 * SEAT_HALF + GAP;
/// How far a display stands from the chip's center, out on its side (3
/// studs), before its `offset`.
const DISPLAY_DISTANCE: i32 = 30;

/// The kind of exterior brick `node` is, if any.
pub fn exterior_kind(node: &Node) -> Option<&str> {
    match node.properties.get(&*sym::EXTERIOR) {
        Some(Literal::String(s)) => Some(s),
        _ => None,
    }
}

/// Whether `module` (the root) has any exterior node.
pub fn has_exterior(module: &Module) -> bool {
    module.nodes.values().any(|n| exterior_kind(n).is_some())
}

/// `module` without its exterior nodes, their wires and their chips' modules:
/// what layout lays out. `None` when it has none.
pub fn without_exterior(module: &Module) -> Option<Module> {
    if !has_exterior(module) {
        return None;
    }
    let mut m = module.clone();
    let gone: Vec<NodeId> = m
        .nodes
        .iter()
        .filter(|(_, n)| exterior_kind(n).is_some())
        .map(|(id, _)| *id)
        .collect();
    for id in &gone {
        m.nodes.remove(id);
        m.chips.remove(id);
    }
    m.wires
        .retain(|w| !gone.contains(&w.source.node_id) && !gone.contains(&w.target.node_id));
    Some(m)
}

/// One `@display`, as its back chip node carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Grid units per pixel (even).
    pub pitch: i32,
    /// `BrickFontDescriptor` asset.
    pub font: String,
    pub border: bool,
    /// The side of the chip it stands on, facing the chip.
    pub side: Side,
    /// Where it moves from its place, in grid units: to its right, out
    /// toward its viewer, and up.
    pub offset: [i32; 3],
}

fn int(node: &Node, key: Sym) -> Option<i64> {
    match node.properties.get(&key) {
        Some(Literal::Int(v)) => Some(*v),
        _ => None,
    }
}

fn string(node: &Node, key: Sym) -> Option<String> {
    match node.properties.get(&key) {
        Some(Literal::String(s)) => Some(s.clone()),
        _ => None,
    }
}

impl DisplayInfo {
    /// The display a back chip node describes.
    pub fn of(node: &Node) -> Option<Self> {
        (exterior_kind(node)? == DISPLAY).then_some(())?;
        Some(Self {
            name: string(node, *sym::DISPLAY_NAME)?,
            width: u32::try_from(int(node, *sym::DISPLAY_WIDTH)?).ok()?,
            height: u32::try_from(int(node, *sym::DISPLAY_HEIGHT)?).ok()?,
            pitch: i32::try_from(int(node, *sym::DISPLAY_PITCH)?).ok()?,
            font: string(node, *sym::DISPLAY_FONT)?,
            border: matches!(
                node.properties.get(&*sym::DISPLAY_BORDER),
                Some(Literal::Bool(true))
            ),
            side: string(node, *sym::DISPLAY_SIDE)
                .and_then(|s| Side::parse(&s))
                .unwrap_or(Side::Top),
            offset: match node.properties.get(&*sym::DISPLAY_OFFSET) {
                Some(Literal::Vector { x, y, z }) => [*x as i32, *y as i32, *z as i32],
                _ => [0, 0, 0],
            },
        })
    }

    /// The properties a back chip node carries for this display.
    pub fn properties(&self) -> Vec<(Sym, Literal)> {
        vec![
            (*sym::EXTERIOR, Literal::String(DISPLAY.to_string())),
            (*sym::DISPLAY_NAME, Literal::String(self.name.clone())),
            (*sym::DISPLAY_WIDTH, Literal::Int(i64::from(self.width))),
            (*sym::DISPLAY_HEIGHT, Literal::Int(i64::from(self.height))),
            (*sym::DISPLAY_PITCH, Literal::Int(i64::from(self.pitch))),
            (*sym::DISPLAY_FONT, Literal::String(self.font.clone())),
            (*sym::DISPLAY_BORDER, Literal::Bool(self.border)),
            (
                *sym::DISPLAY_SIDE,
                Literal::String(self.side.as_str().to_string()),
            ),
            (
                *sym::DISPLAY_OFFSET,
                Literal::Vector {
                    x: f64::from(self.offset[0]),
                    y: f64::from(self.offset[1]),
                    z: f64::from(self.offset[2]),
                },
            ),
        ]
    }

    fn border_width(&self) -> i32 {
        if self.border { self.pitch } else { 0 }
    }

    /// The panel's outer width and height, border included.
    fn outer(&self) -> (i32, i32) {
        let b = 2 * self.border_width();
        (
            self.width as i32 * self.pitch + b,
            self.height as i32 * self.pitch + b,
        )
    }

    /// Row-strip geometry for this display's font at pixel size `pitch`:
    /// `(LineHeight, Offset.Y, WidthScale)`. The `█` is sized from the
    /// font's own metrics ([`BlockMetrics`]) to exactly one pixel wide and
    /// `fill` pixels tall, so rows meet without overlapping and a long row
    /// does not drift.
    pub fn text_geometry(&self) -> (f32, f32, f32) {
        let p = self.pitch as f32;
        let block = BlockMetrics::of(&self.font);
        let line_height = block.fill * p / (block.height * block.em_per_line_height);
        let width_scale = block.height / (block.advance * block.fill);
        let em = block.em_per_line_height * line_height;
        (line_height, -block.rise * em, width_scale)
    }
}

/// A font's `█` (U+2588) in ems: its advance and its height, top to bottom.
/// Read from the font files (`opentype.js`): Iosevka Term 34.9 and Monaspace
/// Argon, the builds the game names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockMetrics {
    pub advance: f32,
    pub height: f32,
    /// World units one em spans per unit of a Text Decal's `LineHeight`,
    /// measured in game for this font.
    pub em_per_line_height: f32,
    /// Height the block is drawn at, in pixels, measured in game: a font
    /// whose exact block leaves hairlines between rows is drawn taller.
    pub fill: f32,
    /// How far above its strip's top edge, in ems, the block is drawn at
    /// `Offset.Y` 0 (negative `Offset.Y` moves text down, in grid units),
    /// measured in game. heightmap2brs's -0.2 per 0.61 of `LineHeight`
    /// (0.26 em) left both fonts' rows low.
    pub rise: f32,
}

impl BlockMetrics {
    /// At `LineHeight` 0.61 and `WidthScale` 2 its `█` drew 0.79 of a
    /// 1-unit pixel wide and about 0.97 tall: 1.28. Its rows sat a little
    /// low at a rise of 0.26 and a little high at 0.178: 0.217.
    pub const IOSEVKA_TERM: Self = Self {
        advance: 0.5,
        height: 1.25,
        em_per_line_height: 1.28,
        fill: 1.0,
        rise: 0.217,
    };
    /// Sized at 1.28 its rows drew 0.97 of a pixel wide with hairlines
    /// between them: 1.24. At exactly one pixel tall its rows still showed
    /// hairlines, and at 1.02 they overlapped: 1.01. At a rise of 0.26 its
    /// rows sat 0.07 of a pixel low: 0.178.
    pub const MONASPACE_ARGON: Self = Self {
        advance: 0.62,
        height: 1.255,
        em_per_line_height: 1.24,
        fill: 1.01,
        rise: 0.178,
    };

    /// The block of `font`, a `BrickFontDescriptor` name.
    pub fn of(font: &str) -> Self {
        match font {
            "MonaspaceArgon" => Self::MONASPACE_ARGON,
            _ => Self::IOSEVKA_TERM,
        }
    }
}

/// Run every wire between the program and a display's back chip through an
/// invisible rerouter (see [`DISPLAY_WIRE`]), one per wire source: a value
/// the program hands the chip, or a row the program reads back. The row
/// decals' own wires stay as they are.
pub fn route_display_wires(module: &mut Module) {
    let backs: Vec<(NodeId, String)> = module
        .nodes_in_order()
        .filter_map(|(id, n)| DisplayInfo::of(n).map(|d| (*id, d.name)))
        .collect();
    for (back, name) in backs {
        let Some(chip) = module.chips.get(&back) else {
            continue;
        };
        let inside_ids: HashSet<NodeId> = chip.nodes.keys().copied().collect();
        let program_ids: HashSet<NodeId> = module
            .nodes
            .iter()
            .filter(|(_, n)| exterior_kind(n).is_none())
            .map(|(id, _)| *id)
            .collect();
        let inside = |id: &NodeId| inside_ids.contains(id);
        let program = |id: &NodeId| program_ids.contains(id);
        // The source ports to route, in wire order: each gets one rerouter.
        let mut sources: Vec<PortRef> = Vec::new();
        for w in &module.wires {
            let crossing = (program(&w.source.node_id) && inside(&w.target.node_id))
                || (inside(&w.source.node_id) && program(&w.target.node_id));
            if crossing && !sources.contains(&w.source) {
                sources.push(w.source);
            }
        }
        let ty = |p: &PortRef| -> Type {
            module
                .nodes
                .get(&p.node_id)
                .or_else(|| chip.nodes.get(&p.node_id))
                .and_then(|n| n.output_type(p.port))
                .unwrap_or(Type::Any)
        };
        let routed: Vec<(PortRef, Type)> = sources.iter().map(|p| (*p, ty(p))).collect();
        for (slot, (source, ty)) in routed.into_iter().enumerate() {
            let mut props = HashMap::default();
            props.insert(*sym::EXTERIOR, Literal::String(DISPLAY_WIRE.to_string()));
            props.insert(*sym::DISPLAY_NAME, Literal::String(name.clone()));
            props.insert(*sym::DISPLAY_SLOT, Literal::Int(slot as i64));
            let id = NodeId::fresh();
            module.add_node(Node::new(
                id,
                NodeKind::Gate,
                gc::REROUTER,
                GateIO::pin(ty),
                props,
                None,
                Some("display wire"),
            ));
            // Only the wires that cross: a program value's own readers in the
            // program, or a row's decals, keep their direct wire.
            let through = id.port(WirePort::RerOutput);
            let into_chip = program(&source.node_id);
            for w in &mut module.wires {
                let crosses = if into_chip {
                    inside(&w.target.node_id)
                } else {
                    program(&w.target.node_id)
                };
                if w.source == source && crosses {
                    w.source = through;
                }
            }
            module.add_wire(Wire {
                source,
                target: id.port(WirePort::RerInput),
            });
        }
    }
}

/// The row a display row node draws: its display's name and row index.
pub fn row_of(node: &Node) -> Option<(String, u32)> {
    (exterior_kind(node)? == DISPLAY_ROW).then_some(())?;
    Some((
        string(node, *sym::DISPLAY_NAME)?,
        u32::try_from(int(node, *sym::DISPLAY_ROW)?).ok()?,
    ))
}

/// The side of the chip a seat or a display stands on, as `@left` etc. spell it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    /// The turn that takes a display laid out on the top side (facing -X)
    /// to this side, facing the chip.
    pub fn display_turn(self) -> Rotation {
        match self {
            Self::Top => Rotation::Deg0,
            Self::Right => Rotation::Deg90,
            Self::Bottom => Rotation::Deg180,
            Self::Left => Rotation::Deg270,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

/// `(x, y)` turned by `turn` about the origin, as a brick turned so moves its
/// own axes: a quarter turn takes +Y to -X.
fn turn_xy(turn: Rotation, x: i32, y: i32) -> (i32, i32) {
    match turn {
        Rotation::Deg0 => (x, y),
        Rotation::Deg90 => (-y, x),
        Rotation::Deg180 => (-x, -y),
        Rotation::Deg270 => (y, -x),
    }
}

/// The way up of a chip flat against the back of a display turned by `turn`:
/// away from the panel, +X before the turn.
fn back_direction(turn: Rotation) -> Direction {
    match turn_xy(turn, 1, 0) {
        (1, _) => Direction::XPositive,
        (-1, _) => Direction::XNegative,
        (_, 1) => Direction::YPositive,
        _ => Direction::YNegative,
    }
}

/// A seat node's name and side.
pub fn seat_of(node: &Node) -> Option<(String, Side)> {
    (exterior_kind(node)? == SEAT).then_some(())?;
    let side = string(node, *sym::SEAT_SIDE)
        .and_then(|s| Side::parse(&s))
        .unwrap_or(Side::Bottom);
    Some((string(node, *sym::SEAT_NAME)?, side))
}

/// A brick of the plan that no node stands for: a border bar.
#[derive(Clone, Debug)]
pub struct PanelBrick {
    pub position: Position,
    pub rotation: Rotation,
    /// Procedural half-size.
    pub size: (u16, u16, u16),
    /// The display (its back chip node) whose color it takes.
    pub display: NodeId,
}

/// Where each exterior node goes, and the panel bricks around them.
#[derive(Clone, Debug, Default)]
pub struct ExteriorPlan {
    /// Brick center and rotation of each exterior node.
    pub nodes: HashMap<NodeId, (Position, Rotation)>,
    /// Procedural half-size of each row strip.
    pub strips: HashMap<NodeId, (u16, u16, u16)>,
    /// The way up of each exterior node that does not stand upright (a
    /// display's back chip, flat against the panel).
    pub directions: HashMap<NodeId, Direction>,
    pub panels: Vec<PanelBrick>,
    /// Each display by name: its back chip node and options.
    pub displays: HashMap<String, (NodeId, DisplayInfo)>,
    /// How much higher than usual the root plane must sit to clear the
    /// panels and seats.
    pub clearance: f32,
}

impl ExteriorPlan {
    /// Place `module`'s exterior nodes around a chip brick centered at
    /// `chip`.
    pub fn new(module: &Module, chip: (i32, i32, i32)) -> Self {
        let (cx, cy, cz) = chip;
        let mut plan = Self::default();
        let mut displays: Vec<(NodeId, DisplayInfo)> = Vec::new();
        let mut rows: HashMap<String, Vec<(u32, NodeId)>> = HashMap::default();
        let mut seats: HashMap<Side, Vec<NodeId>> = HashMap::default();
        let mut display_wires: HashMap<String, Vec<(i64, NodeId)>> = HashMap::default();
        for (id, node) in module.nodes_in_order() {
            if let Some(d) = DisplayInfo::of(node) {
                displays.push((*id, d));
            } else if let Some((name, row)) = row_of(node) {
                rows.entry(name).or_default().push((row, *id));
            } else if let Some((_, side)) = seat_of(node) {
                seats.entry(side).or_default().push(*id);
            } else if exterior_kind(node) == Some(DISPLAY_WIRE)
                && let Some(name) = string(node, *sym::DISPLAY_NAME)
            {
                let slot = int(node, *sym::DISPLAY_SLOT).unwrap_or(0);
                display_wires.entry(name).or_default().push((slot, *id));
            }
        }

        let seat_floor = cz - CHIP_HALF_HEIGHT;
        let seat_top = seat_floor + 2 * SEAT_HALF_HEIGHT;
        let side_seats = [Side::Left, Side::Right]
            .iter()
            .any(|s| seats.contains_key(s));

        // Seats first: a display standing over one is raised above it.
        let mut seat_boxes: Vec<(i32, i32)> = Vec::new();
        for (side, ids) in &seats {
            // Each row runs left to right as seen from its seats, which face
            // the chip: `along` is that direction in world coordinates.
            let (center, along, rotation) = match side {
                Side::Bottom => ((cx - SEAT_DISTANCE, cy), (0, 1), Rotation::Deg270),
                Side::Top => ((cx + SEAT_DISTANCE, cy), (0, -1), Rotation::Deg90),
                Side::Left => ((cx, cy - SEAT_DISTANCE), (-1, 0), Rotation::Deg0),
                Side::Right => ((cx, cy + SEAT_DISTANCE), (1, 0), Rotation::Deg180),
            };
            let n = ids.len() as i32;
            for (i, id) in ids.iter().enumerate() {
                let offset = SEAT_PITCH * (2 * i as i32 - (n - 1)) / 2;
                let (x, y) = (center.0 + along.0 * offset, center.1 + along.1 * offset);
                seat_boxes.push((x, y));
                plan.nodes.insert(
                    *id,
                    (Position::new(x, y, seat_floor + SEAT_HALF_HEIGHT), rotation),
                );
            }
        }

        let base = cz + CHIP_HALF_HEIGHT + GAP;
        let mut top = base;
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            let group: Vec<&(NodeId, DisplayInfo)> =
                displays.iter().filter(|(_, d)| d.side == side).collect();
            // Each side's displays are laid out as if on the top side (facing
            // -X, the viewer's right +Y) and then turned to `side`.
            let turn = side.display_turn();
            let at = |x: i32, y: i32, z: i32| {
                let (x, y) = turn_xy(turn, x, y);
                Position::new(cx + x, cy + y, z)
            };
            let total: i32 = group.iter().map(|(_, d)| d.outer().0).sum::<i32>()
                + GAP * (group.len() as i32 - 1).max(0);
            let mut left = -total / 2;
            for (chip_id, d) in group {
                let (ow, oh) = d.outer();
                let b = d.border_width();
                let p = d.pitch;
                let (w, h) = (d.width as i32 * p, d.height as i32 * p);
                let back = STRIP_HALF_DEPTH + i32::from(d.border);
                let [right, out, up] = d.offset;
                let x0 = DISPLAY_DISTANCE - out;
                let y0 = left + right;
                // The panel's footprint, back chip included, seen from above.
                let (ax, ay) = turn_xy(turn, x0 - back, y0);
                let (bx, by) = turn_xy(turn, x0 + back + 2 * CHIP_HALF_HEIGHT, y0 + ow);
                let over_seat = seat_boxes.iter().any(|&(sx, sy)| {
                    let (sx, sy) = (sx - cx, sy - cy);
                    ax.min(bx) < sx + SEAT_HALF
                        && ax.max(bx) > sx - SEAT_HALF
                        && ay.min(by) < sy + SEAT_HALF
                        && ay.max(by) > sy - SEAT_HALF
                });
                let zb = if over_seat {
                    base.max(seat_top + GAP)
                } else {
                    base
                } + up;
                let mut row_ids = rows.remove(&d.name).unwrap_or_default();
                row_ids.sort();
                for (r, id) in row_ids {
                    let z = zb + b + h - r as i32 * p - p / 2;
                    plan.nodes.insert(id, (at(x0, y0 + b + w / 2, z), turn));
                    plan.strips.insert(
                        id,
                        (STRIP_HALF_DEPTH as u16, (w / 2) as u16, (p / 2) as u16),
                    );
                }
                if b > 0 {
                    let depth = (STRIP_HALF_DEPTH + 1) as u16;
                    let bar = |y: i32, z: i32, half_w: i32, half_h: i32| PanelBrick {
                        position: at(x0, y, z),
                        rotation: turn,
                        size: (depth, half_w as u16, half_h as u16),
                        display: *chip_id,
                    };
                    let mid_y = y0 + ow / 2;
                    let mid_z = zb + oh / 2;
                    plan.panels.push(bar(mid_y, zb + oh - b / 2, ow / 2, b / 2));
                    plan.panels.push(bar(mid_y, zb + b / 2, ow / 2, b / 2));
                    plan.panels.push(bar(y0 + b / 2, mid_z, b / 2, h / 2));
                    plan.panels.push(bar(y0 + ow - b / 2, mid_z, b / 2, h / 2));
                }
                // The back chip lies flat against the panel's back, centered,
                // its top facing away from the panel; the invisible rerouters
                // its wires run through sit in a row just under it.
                let (mid_y, mid_z) = (y0 + ow / 2, zb + oh / 2);
                plan.nodes.insert(
                    *chip_id,
                    (at(x0 + back + CHIP_HALF_HEIGHT, mid_y, mid_z), turn),
                );
                plan.directions.insert(*chip_id, back_direction(turn));
                let mut slots = display_wires.remove(&d.name).unwrap_or_default();
                slots.sort();
                let n = slots.len() as i32;
                for (i, (_, id)) in slots.into_iter().enumerate() {
                    let y = mid_y + 2 * REROUTER_HALF * i as i32 - REROUTER_HALF * (n - 1);
                    let z = mid_z - CHIP_HALF - REROUTER_HALF;
                    // The back chip's up axis, and turned so its arrow points
                    // up at the chip: with the up axis along X or Y, Deg270
                    // takes a rerouter's +Y forward to world +Z (Brickadia's
                    // orientation table, as omegga's `translationTable`).
                    plan.nodes
                        .insert(id, (at(x0 + back + REROUTER_HALF, y, z), Rotation::Deg270));
                    plan.directions.insert(id, back_direction(turn));
                }
                top = top.max(zb + oh);
                left += ow + GAP;
            }
        }
        plan.displays = displays
            .into_iter()
            .map(|(id, d)| (d.name.clone(), (id, d)))
            .collect();
        if side_seats {
            top = top.max(seat_top);
        }
        if !plan.displays.is_empty() || side_seats {
            plan.clearance = (top + GAP - base) as f32;
        }
        plan
    }
}

/// The Text Decal one display row draws with, on the front (-X) face of its
/// strip. `Text` starts empty; the row var's wire fills it.
pub fn row_component(world: &mut World, d: &DisplayInfo) -> LiteralComponent {
    use brdb::schema::BrdbValue;
    let (font_idx, _) = world
        .global_data
        .external_asset_references
        .insert_full(("BrickFontDescriptor".to_string(), d.font.clone()));
    let (line_height, offset_y, width_scale) = d.text_geometry();
    let anchor = LiteralComponent::new("Vector2f").with_data([
        ("X", Box::new(0.0f32) as Box<dyn AsBrdbValue>),
        ("Y", Box::new(0.0f32)),
    ]);
    LiteralComponent::new(TEXT_DISPLAY).with_data([
        ("Text", Box::new(String::new()) as Box<dyn AsBrdbValue>),
        ("Font", Box::new(BrdbValue::Asset(Some(font_idx)))),
        ("Rotation", Box::new(0.0f32)),
        ("Skew", Box::new(0.0f32)),
        ("Kerning", Box::new(0.0f32)),
        ("LineHeight", Box::new(line_height)),
        ("WidthScale", Box::new(width_scale)),
        ("LineOffset", Box::new(0.0f32)),
        ("Anchor", Box::new(anchor)),
        (
            "Offset",
            Box::new(Vector3f {
                x: 0.0,
                y: offset_y,
                z: 0.0,
            }),
        ),
        ("MaterialSlider", Box::new(10i32)),
        ("ScuffWidth", Box::new(0.0f32)),
        ("bEnabled", Box::new(true)),
        // EBrickDirection::X_Negative: the strip's front, facing the viewer.
        ("Face", Box::new(1u8)),
        ("bAlignToWedge", Box::new(false)),
        ("bOverrideColor", Box::new(true)),
        ("Typeface", Box::new(0u8)),
        ("Billboard", Box::new(0u8)),
        // EBRTextMaterial::Unlit, EBRTextShading::None, EBRTextOutline::None.
        ("Material", Box::new(0u8)),
        ("Shading", Box::new(0u8)),
        ("Outline", Box::new(0u8)),
        ("bSharpCorners", Box::new(true)),
        ("bSharpOutlines", Box::new(true)),
        ("bForeground", Box::new(false)),
    ])
}

#[cfg(test)]
mod tests;
