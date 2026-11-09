//! The nodes of `@display` and `@seat` (see [`crate::display`] and
//! `wirescript_ir::exterior`): a seat gate per `@seat` input, and per
//! `@display` a back chip holding the code [`crate::display::rewrite`] added
//! plus one Text Decal per row. Emit places all of them around the chip.

use wirescript_ir::exterior::{self, DisplayInfo};

use super::AddNodeOpts;
use super::context::{Binding, LocalRecord, LowerCtx};
use super::style;
use crate::ast::{InDecl, Script, TopDecl};
use crate::collections::HashMap;
use crate::intern::{intern, sym};
use crate::ir::port_registry::WirePort;
use crate::ir::{GateIO, Literal, NodeKind, PortSpec, Type, gate_class as gc};

/// A `@seat` input: a seat gate whose outputs are the `Seat` record's
/// fields.
pub(super) fn lower_seat(ctx: &mut LowerCtx<'_>, d: &InDecl) {
    let spec = crate::display::seat_spec(d, &mut Vec::new()).unwrap_or_default();
    let mut properties = HashMap::default();
    properties.insert(*sym::EXTERIOR, Literal::String(exterior::SEAT.to_string()));
    properties.insert(*sym::SEAT_NAME, Literal::String(d.name.clone()));
    let side = d.side.map_or("bottom", |s| s.as_str());
    properties.insert(*sym::SEAT_SIDE, Literal::String(side.to_string()));
    if let Some(label) = spec.label {
        properties.insert(intern("PromptCustomLabel"), Literal::String(label));
    }
    if let Some(mode) = spec.mode {
        properties.insert(intern("SeatMode"), Literal::Int(i64::from(mode)));
    }
    if let Some([pitch, yaw, roll]) = spec.camera {
        properties.insert(
            intern("ThirdPersonCameraRotation"),
            Literal::Rotator { pitch, yaw, roll },
        );
    }
    if let Some([x, y, z]) = spec.exit {
        properties.insert(
            intern("ExitOffset"),
            Literal::Vector {
                x: f64::from(x),
                y: f64::from(y),
                z: f64::from(z),
            },
        );
    }
    let seat = ctx.add_gate(AddNodeOpts {
        gate_class: exterior::SEAT_COMPONENT,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: Vec::new(),
            outputs: vec![
                PortSpec::new(WirePort::BIsOccupied.sym(), Type::Bool),
                PortSpec::new(WirePort::SittingCharacter.sym(), Type::Character),
            ],
        },
        properties,
        ..Default::default()
    });
    style::bake_style(ctx, seat, d.color_expr.as_ref(), d.material_expr.as_ref());
    let field = |port: WirePort| {
        Binding::Local(LocalRecord {
            port: seat.port(port),
        })
    };
    let mut fields = HashMap::default();
    fields.insert(intern("occupied"), field(WirePort::BIsOccupied));
    fields.insert(intern("character"), field(WirePort::SittingCharacter));
    ctx.scope.insert(&d.name, Binding::Record(fields));
}

/// Each `@display`'s back chip and row Text Decals. Runs once every
/// declaration is lowered: the back chip takes over the root-level nodes
/// lowered from the display's added code, and each row's decal reads that
/// row's var.
pub(super) fn build_displays(ctx: &mut LowerCtx<'_>, ast: &Script) {
    for d in &ast.decls {
        let TopDecl::Var(v) = d else { continue };
        let Some(spec) = crate::display::display_spec(v, &mut Vec::new()) else {
            continue;
        };
        let Some(block) = ast.display_blocks.iter().find(|b| b.name == v.name) else {
            continue;
        };
        let info = DisplayInfo {
            name: spec.name.clone(),
            width: spec.width,
            height: spec.height,
            pitch: spec.pitch,
            font: spec.font.asset().to_string(),
            border: spec.border,
            side: spec.side,
            offset: spec.offset,
        };
        let chip = ctx.add_gate(AddNodeOpts {
            gate_class: gc::MICROCHIP_ALT,
            source_range: v.range.clone(),
            ports: GateIO::default(),
            properties: info.properties().into_iter().collect(),
            ..Default::default()
        });
        // Closed: it hangs on the back of the panel, its plane out of the way.
        if let Some(node) = ctx.builder.module.nodes.get_mut(&chip) {
            node.kind = NodeKind::Chip;
            let props = std::sync::Arc::make_mut(&mut node.properties);
            props.insert(*sym::NAME_LABEL, Literal::String(v.name.clone()));
            props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        }
        if !style::is_constant_style(ctx, v.color_expr.as_ref(), v.material_expr.as_ref()) {
            ctx.error(
                "WS127",
                "a @display's @color and @material must be constants",
                &v.range,
            );
        }
        style::bake_style(ctx, chip, v.color_expr.as_ref(), v.material_expr.as_ref());
        let within = |r: &crate::diagnostic::SourceRange| {
            r.file == block.range.file
                && r.start.offset >= block.range.start.offset
                && r.start.offset <= block.range.end.offset
        };
        for (id, node) in &mut ctx.builder.module.nodes {
            if *id != chip && node.chip_id.is_none() && within(&node.source_range) {
                node.chip_id = Some(chip);
            }
        }
        for r in 0..spec.height {
            let row_var = crate::display::row_var(&v.name, r);
            let Some(Binding::Var(rec)) = ctx.scope.get(&row_var).cloned() else {
                continue;
            };
            let mut properties = HashMap::default();
            properties.insert(
                *sym::EXTERIOR,
                Literal::String(exterior::DISPLAY_ROW.to_string()),
            );
            properties.insert(*sym::DISPLAY_NAME, Literal::String(v.name.clone()));
            properties.insert(*sym::DISPLAY_ROW, Literal::Int(i64::from(r)));
            let decal = ctx.add_gate(AddNodeOpts {
                gate_class: exterior::TEXT_DISPLAY,
                source_range: v.range.clone(),
                ports: GateIO {
                    inputs: vec![PortSpec::new(WirePort::Text.sym(), Type::String)],
                    outputs: Vec::new(),
                },
                properties,
                ..Default::default()
            });
            ctx.connect(
                rec.node_id.port(WirePort::Value),
                decal.port(WirePort::Text),
            );
        }
    }
}
