//! `@color` / `@material`: a Brick Property Changer on the styled brick.

use super::*;

/// The component a `@color` / `@material` adds to the brick it styles.
pub(super) const PROPERTY_CHANGER: &str = "Component_BrickPropertyChanger";

/// Each material asset a brick can carry, at its `EBrickMaterial` value.
const MATERIALS: [BString; 6] = [
    brdb::assets::materials::PLASTIC,
    brdb::assets::materials::GLASS,
    brdb::assets::materials::TRANSLUCENT_PLASTIC,
    brdb::assets::materials::GLOW,
    brdb::assets::materials::METALLIC,
    brdb::assets::materials::HOLOGRAM,
];

/// How a node is styled: its constant color and material (its `_color` and
/// `_material` properties, or a runtime value whose source is a literal emit
/// inlines, since no wire will carry it), and whether a real runtime value is
/// wired into either. `None` when the node is not styled at all.
pub(super) fn node_style<'a>(
    module: &'a Module,
    id: &NodeId,
    node: &'a Node,
) -> Option<(Option<Literal>, Option<Literal>, bool)> {
    let inlined = |src: Option<&PortRef>| -> Option<&'a Literal> {
        let n = module.nodes.get(&src?.node_id)?;
        (n.gate_class == gc::LITERAL)
            .then(|| n.properties.get(&*sym::VALUE))
            .flatten()
    };
    let wired_color = module.dynamic_colors.get(id);
    let wired_material = module.dynamic_materials.get(id);
    let color = node
        .properties
        .get(&*sym::BRICK_COLOR)
        .or_else(|| inlined(wired_color));
    let material = node
        .properties
        .get(&*sym::BRICK_MATERIAL)
        .or_else(|| inlined(wired_material));
    let wired = (wired_color.is_some() && inlined(wired_color).is_none())
        || (wired_material.is_some() && inlined(wired_material).is_none());
    let styled = color.is_some() || material.is_some() || wired;
    styled.then(|| {
        (
            color.and_then(crate::ir::brick_color),
            material.cloned(),
            wired,
        )
    })
}

/// Style `brick`: a constant color or material is the brick's own, set on it
/// directly. Only a runtime value needs a Brick Property Changer, whose
/// inputs Pass 3.5 wires; its other fields copy the brick, which already
/// carries any constant, so the changer leaves them as they are.
pub(super) fn apply_style(
    brick: &mut brdb::Brick,
    color: Option<&Literal>,
    material: Option<&Literal>,
    wired: bool,
) {
    if let Some(Literal::Color { r, g, b, .. }) = color {
        brick.color = brdb::Color::new(*r, *g, *b);
    }
    if let Some(Literal::Int(n)) = material
        && let Some(asset) = usize::try_from(*n)
            .ok()
            .and_then(|i| MATERIALS.get(i).cloned())
    {
        brick.material = asset;
    }
    if wired {
        let changer = property_changer(brick);
        brick.add_component_box(Box::new(changer));
    }
}

/// The Brick Property Changer for a brick with a runtime style. It sets every
/// property it has, so each field starts as the brick's own (already carrying
/// any constant `@color` / `@material`): a runtime `@color` keeps the brick's
/// material and material alpha, a runtime `@material` keeps its color, and
/// visibility and collision stay as the brick has them (all on, except on an
/// `@invisible` shell). Pass 3.5 wires the runtime value over its field.
pub(super) fn property_changer(brick: &brdb::Brick) -> LiteralComponent {
    let own = brick.color;
    let color = brdb::SavedBrickColor::rgb(own.r, own.g, own.b);
    let material = MATERIALS
        .iter()
        .position(|m| *m == brick.material)
        .unwrap_or(0) as u8;
    let c = &brick.collision;
    let player = |p: Option<bool>| p.unwrap_or(c.player);
    let any = c.player
        || player(c.player1)
        || player(c.player2)
        || player(c.player3)
        || c.weapon
        || c.interact
        || c.tool
        || c.physics;
    LiteralComponent::new(PROPERTY_CHANGER).with_data([
        (
            "bVisibility",
            Box::new(brick.visible) as Box<dyn AsBrdbValue>,
        ),
        ("bCollisionEnabled", Box::new(any)),
        ("bCollisionEnabled_Player", Box::new(c.player)),
        ("bCollisionEnabled_Player1", Box::new(player(c.player1))),
        ("bCollisionEnabled_Player2", Box::new(player(c.player2))),
        ("bCollisionEnabled_Player3", Box::new(player(c.player3))),
        ("bCollisionEnabled_Weapon", Box::new(c.weapon)),
        ("bCollisionEnabled_Interaction", Box::new(c.interact)),
        ("bCollisionEnabled_Physics", Box::new(c.physics)),
        ("Color", Box::new(color)),
        ("MaterialAlpha", Box::new(brick.material_intensity)),
        ("Material", Box::new(material)),
    ])
}

/// The `port` input (`Color` or `Material`) of the changer on `brick_id`.
pub(super) fn property_changer_port(brick_id: usize, port: &'static str) -> BrdbWirePort {
    BrdbWirePort {
        brick_id,
        component_type: BString::Static(PROPERTY_CHANGER),
        port_name: BString::Static(port),
    }
}
