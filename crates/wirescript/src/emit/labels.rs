//! Text labels: chip titles, plane headers, comment annotations.

use super::*;

/// Name labels on chips, vars, and I/O gates.
pub(super) const LABEL_LINE_HEIGHT: f32 = 2.4;

/// Smaller tag on Var_Get/Set-style gates naming the variable they touch.
pub(super) const VAR_TAG_LINE_HEIGHT: f32 = 1.2;

/// On-screen angle every name label and variable tag reads at.
pub(super) const LABEL_ROTATION_DEG: f32 = -45.0;

/// The `Rotation` to write on a name label riding a brick placed at
/// `rotation`.
///
/// A label's rotation is brick-local — it rides the brick's yaw — so a
/// quarter-turned brick would read its tag a quarter-turn off from every
/// other tag on the plane. Taking the yaw back out lands the text at the
/// same on-screen angle regardless of how the brick under it is turned.
pub(super) fn label_rotation_deg(rotation: NodeRotation) -> f32 {
    match rotation {
        NodeRotation::Deg0 => LABEL_ROTATION_DEG,
        NodeRotation::Deg90 => LABEL_ROTATION_DEG - 90.0,
        NodeRotation::Deg180 => LABEL_ROTATION_DEG - 180.0,
        NodeRotation::Deg270 => LABEL_ROTATION_DEG - 270.0,
    }
}

/// `@closed` marks a chip's inner grid collapsed; absent = open. Non-root
/// chips default open.
pub(super) fn chip_is_closed(node: &Node) -> bool {
    matches!(
        node.properties.get(&*sym::CHIP_CLOSED),
        Some(Literal::Bool(true))
    )
}

/// Display name for a chip: the `@label` override wins, else the chip's
/// declared name (anonymous partitions have none).
pub(super) fn chip_display_name(node: &Node, child_module: &Module) -> Option<String> {
    if let Some(Literal::String(s)) = node.properties.get(&*sym::NAME_LABEL) {
        if !s.is_empty() {
            return Some(s.clone());
        }
    }
    match child_module.scopes.get(&crate::ir::ROOT_SCOPE_ID) {
        Some(crate::ir::ScopeInfo {
            kind: crate::ir::ScopeKind::ChipBody { name },
            ..
        }) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// Floating-label text for a microchip I/O gate, from its `PortLabel`
/// property. User-given names label as themselves. Synthesized plumbing
/// maps to friendly labels: the auto exec ports (`_exec_in`/`_exec_out`)
/// read `exec`, and the anonymous `-> type` return output (`_`) reads
/// `return`. Any other `_`-prefixed name stays unlabeled. `@label("…")`
/// overrides all of the above (covers both pass-1 I/O gate labels and
/// outer-rerouter labels — both call this).
pub(super) fn microchip_io_label(node: &Node) -> Option<String> {
    if let Some(Literal::String(s)) = node.properties.get(&*sym::NAME_LABEL) {
        if !s.is_empty() {
            return Some(s.clone());
        }
    }
    match node.properties.get(&*sym::PORT_LABEL)? {
        Literal::String(s) if s == "_exec_in" || s == "_exec_out" => Some("exec".to_string()),
        Literal::String(s) if s == "_" => Some("return".to_string()),
        Literal::String(s) if !s.is_empty() && !s.starts_with('_') => Some(s.clone()),
        _ => None,
    }
}

/// Floating text-label component (`Component_TextDisplay`) attached as a
/// second component on chip / variable / I/O-gate bricks, showing the
/// element's name. Fields left unset (colors, outline widths, sharp
/// corners, …) are filled from brdb's `STRUCT_DEFAULTS`.
pub(super) fn text_label(
    world: &mut World,
    text: &str,
    rotation_deg: f32,
    offset_z: f32,
    line_height: f32,
    anchor_x: f32,
    anchor_y: f32,
) -> LiteralComponent {
    use brdb::schema::BrdbValue;
    let (font_idx, _) = world
        .global_data
        .external_asset_references
        .insert_full(("BrickFontDescriptor".to_string(), "IosevkaTerm".to_string()));
    let anchor = LiteralComponent::new("Vector2f").with_data([
        ("X", Box::new(anchor_x) as Box<dyn AsBrdbValue>),
        ("Y", Box::new(anchor_y)),
    ]);
    LiteralComponent::new("Component_TextDisplay").with_data([
        ("Text", Box::new(text.to_string()) as Box<dyn AsBrdbValue>),
        ("Font", Box::new(BrdbValue::Asset(Some(font_idx)))),
        ("Rotation", Box::new(rotation_deg)),
        ("LineHeight", Box::new(line_height)),
        ("Anchor", Box::new(anchor)),
        (
            "Offset",
            Box::new(Vector3f {
                x: 0.0,
                y: 0.0,
                z: offset_z,
            }),
        ),
        // Top face of the brick (enum default 0 is X_Positive).
        ("Face", Box::new(4u8)),
        // EBRTextOutline::Outlined; the enum default (None) hides the
        // outline entirely, and 4px reads better than the template's 2.
        ("Outline", Box::new(2u8)),
        ("OutlineWidth", Box::new(4.0f32)),
    ])
}

/// Header block for an opened plane: `<size="96">{title}</>` then the doc
/// comment on the following lines. A documented but nameless chip renders the
/// doc alone; nothing at all → no header. Text passes through raw (rich-text
/// tags in names/docs are a feature, not escaped).
fn chip_header_text(title: Option<&str>, doc: Option<&str>) -> Option<String> {
    match (title, doc) {
        // Blank line between the big title and the doc so the doc isn't cramped
        // right under the size-96 heading.
        (Some(t), Some(d)) => Some(format!("<size=\"96\">{t}</>\n\n{d}")),
        (Some(t), None) => Some(format!("<size=\"96\">{t}</>")),
        (None, Some(d)) => Some(d.to_string()),
        (None, None) => None,
    }
}

/// Grid units the header brick's centre sits BEYOND the plane's top edge —
/// which under the measured `WALL_ROT` mapping is the local +X edge (the
/// brick is 1×1 → half-size 5, so at +5 its near face rests exactly on the
/// edge). It must never sit inside the plane: gates reach `extent.x - 5`
/// (extent = layout half-span + 5), and the game DROPS overlapping bricks at
/// load — orphaning the dropped brick's components and dangling every wire
/// into it. Pinned during in-game verification.
const HEADER_EDGE_LIFT: i32 = 5;

/// Invisible 1×1 carrier brick floating just above the plane's top-centre —
/// local (extent.x + lift, 0) under the measured `WALL_ROT` mapping (local
/// +X = world up, local Y = world horizontal). Text is centred and flows
/// downward from the top edge (`Anchor = (0.5, 0)`).
pub(super) fn emit_plane_header(
    world: &mut World,
    bricks: &mut Vec<brdb::Brick>,
    extent: IntVector,
    title: Option<&str>,
    doc: Option<&str>,
) {
    let Some(text) = chip_header_text(title, doc) else {
        return;
    };
    // A 1x1F procedural default brick (10x10x4 cm -> half-extents 5,5,2).
    let mut brick = brdb::Brick {
        asset: brdb::BrickType::from((brdb::assets::bricks::PB_DEFAULT_BRICK, (5, 5, 2))),
        position: brdb::Position {
            x: extent.x + HEADER_EDGE_LIFT,
            y: 0,
            z: 2,
        },
        visible: false,
        ..Default::default()
    };
    brick.add_component_box(Box::new(text_label(
        world,
        &text,
        0.0,
        0.5,
        LABEL_LINE_HEIGHT,
        0.5,
        0.0,
    )));
    bricks.push(brick);
}

/// One invisible 1×1 carrier brick per layout text annotation — the source's
/// own-line `//` comments under the code-shaped layout. Same font, outline and
/// face treatment as a plane header, but anchored on the label's LEFT edge so
/// the text runs rightward from the row's indent, the way the comment reads in
/// the source. The annotation's position is the carrier's min corner, matching
/// the gate-brick convention, so a comment sits on a row of its own.
pub(super) fn emit_annotations(
    world: &mut World,
    bricks: &mut Vec<brdb::Brick>,
    annotations: &[crate::layout::TextAnnotation],
) {
    for ann in annotations {
        // A 1x1F procedural default brick (10x10x4 cm -> half-extents 5,5,2).
        let mut brick = brdb::Brick {
            asset: brdb::BrickType::from((brdb::assets::bricks::PB_DEFAULT_BRICK, (5, 5, 2))),
            position: brdb::Position {
                x: ann.x + 5,
                y: ann.y + 5,
                z: ann.z,
            },
            visible: false,
            ..Default::default()
        };
        brick.add_component_box(Box::new(text_label(
            world,
            &ann.text,
            0.0,
            0.5,
            LABEL_LINE_HEIGHT,
            0.0,
            0.5,
        )));
        bricks.push(brick);
    }
}
