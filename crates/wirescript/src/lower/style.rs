//! `@color` / `@material`: the brick style of a chip, a `var` or the module.
//!
//! A value that folds to a constant is baked onto the styled node as its
//! `_color` / `_material` property (the root shell's goes in
//! `Module::root_style`). A runtime value, allowed where a runtime `@label` is
//! (a top-level `var` and the module), is lowered in pure context and recorded
//! for emit to wire into the brick's Brick Property Changer.

use super::*;

/// The built-in game enum a `@material` value is.
pub(crate) const MATERIAL_ENUM: &str = "BrickMaterial";

/// A `@material` argument with a bare member name (`@material(Glow)`) read
/// as that member (`BrickMaterial.Glow`). Outside this argument a
/// `BrickMaterial` member is never bare (see
/// `catalog::resolves_bare_members`); here, as for `SetBrickProperty`'s
/// property, the member wins. Typecheck and lowering both read the argument
/// through this.
pub(crate) fn material_arg(e: &Expr) -> std::borrow::Cow<'_, Expr> {
    if let Expr::Ident { name, range } = e
        && crate::catalog::builtin_game_enums().iter().any(|g| {
            g.clean_name == MATERIAL_ENUM && g.variants.iter().any(|v| &v.clean_name == name)
        })
    {
        return std::borrow::Cow::Owned(Expr::FieldAccess {
            obj: Box::new(Expr::Ident {
                name: MATERIAL_ENUM.to_string(),
                range: range.clone(),
            }),
            field: name.clone(),
            range: range.clone(),
        });
    }
    std::borrow::Cow::Borrowed(e)
}

/// What a `@color` / `@material` value folds to at compile time, or `None` for
/// a runtime value. Typecheck (which decides where a runtime value may go) and
/// lowering (which bakes or wires it) both ask this, so they agree on which
/// values are constant. No `const mod` call is evaluated, in either stage.
pub(crate) fn style_constant(e: &Expr, cx: &crate::const_eval::ConstCtx) -> Option<Literal> {
    let mut budget = crate::const_eval::Budget::default();
    if let Some(lit) = srgb_constructor(e, cx, &mut budget) {
        return Some(lit);
    }
    crate::const_eval::eval_expr(e, cx, &mut budget).ok()
}

/// A `ColorSRGB(r, g, b, a)` call on constant arguments, as the
/// `Literal::Color` bytes it builds. The constant evaluator does not fold this
/// constructor, so it is handled here.
fn srgb_constructor(
    e: &Expr,
    cx: &crate::const_eval::ConstCtx,
    budget: &mut crate::const_eval::Budget,
) -> Option<Literal> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident { name, .. } = callee.as_ref() else {
        return None;
    };
    if name != "ColorSRGB" || args.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (slot, arg) in bytes.iter_mut().zip(args) {
        let CallArg::Positional(arg) = arg else {
            return None;
        };
        *slot = match crate::const_eval::eval_expr(arg, cx, budget).ok()? {
            Literal::Int(n) => n.clamp(0, 255) as u8,
            Literal::Float(f) => (f as i64).clamp(0, 255) as u8,
            _ => return None,
        };
    }
    let [r, g, b, a] = bytes;
    Some(Literal::Color { r, g, b, a })
}

/// The `_material` a constant `BrickMaterial` bakes as: its discriminant.
fn material_value(lit: &Literal) -> Option<Literal> {
    match lit {
        Literal::Record(fields) => fields
            .iter()
            .find(|(n, _)| n == "__disc")
            .map(|(_, v)| v.clone()),
        Literal::Int(_) => Some(lit.clone()),
        _ => None,
    }
}

/// The baked `_color` / `_material` of each of `color` and `material` that
/// folds to a constant here.
fn baked_style(
    ctx: &LowerCtx,
    color: Option<&Expr>,
    material: Option<&Expr>,
) -> (Option<Literal>, Option<Literal>) {
    let cx = ctx.const_ctx(None);
    let color = color
        .and_then(|e| style_constant(e, &cx))
        .and_then(|lit| crate::ir::brick_color(&lit));
    let material = material
        .and_then(|e| style_constant(&material_arg(e), &cx))
        .and_then(|lit| material_value(&lit));
    (color, material)
}

/// Set the constant parts of a `@color` / `@material` on `node_id`.
pub(super) fn bake_style(
    ctx: &mut LowerCtx,
    node_id: NodeId,
    color: Option<&Expr>,
    material: Option<&Expr>,
) {
    if color.is_none() && material.is_none() {
        return;
    }
    let (color, material) = baked_style(ctx, color, material);
    let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) else {
        return;
    };
    let props = std::sync::Arc::make_mut(&mut node.properties);
    if let Some(c) = color {
        props.insert(*sym::BRICK_COLOR, c);
    }
    if let Some(m) = material {
        props.insert(*sym::BRICK_MATERIAL, m);
    }
}

/// Every storage gate `binding` declares, in id order: one for a scalar
/// `var`, one per field or column for a record, an enum or a container of
/// either. A `var` styles all of them.
fn storage_nodes(binding: &Binding) -> Vec<NodeId> {
    fn go(b: &Binding, out: &mut Vec<NodeId>) {
        match b {
            Binding::Var(rec) => out.push(rec.node_id),
            Binding::Record(fields) => fields.values().for_each(|f| go(f, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    go(binding, &mut out);
    out.sort_unstable();
    out
}

/// The constant `@color` / `@material` of the `var` just declared by `d`.
pub(super) fn bake_var_style(ctx: &mut LowerCtx, d: &VarDecl) {
    if d.color_expr.is_none() && d.material_expr.is_none() {
        return;
    }
    let nodes = ctx
        .scope
        .get(&d.name)
        .map(storage_nodes)
        .unwrap_or_default();
    for node_id in nodes {
        bake_style(
            ctx,
            node_id,
            d.color_expr.as_ref(),
            d.material_expr.as_ref(),
        );
    }
}

/// A runtime `@color` lowered to a color-typed port.
fn lower_color_value(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    lower_expr(ctx, e)
}

/// A runtime `@material` lowered to the int-typed port of its `BrickMaterial`
/// discriminant, the value the changer's `Material` input takes.
fn lower_material_value(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    let e = &*material_arg(e);
    match crate::lower::access::lower_discriminant(ctx, e, e.range()) {
        Some(p) => p,
        None => lower_expr(ctx, e),
    }
}

/// Resolve the runtime `@color` / `@material` on top-level `var`s into the
/// module's `dynamic_colors` / `dynamic_materials`, one entry per storage gate.
///
/// Runs in the same post-declaration pass as `resolve_dynamic_var_labels`, in
/// a pure context (`current_exec` cleared) so a var read resolves to its live
/// `Value` output.
pub(super) fn resolve_dynamic_var_styles(ctx: &mut LowerCtx, decls: &[TopDecl]) {
    let saved_exec = ctx.current_exec.take();
    for d in decls {
        let TopDecl::Var(v) = d else { continue };
        if v.color_expr.is_none() && v.material_expr.is_none() {
            continue;
        }
        let view = ctx.top_view(crate::scope::top::ReadAt::code(&v.range));
        let hosts = ctx
            .scope
            .get(&v.name)
            .map(storage_nodes)
            .unwrap_or_default();
        if !hosts.is_empty() {
            let cx = ctx.const_ctx(None);
            let runtime_color = v
                .color_expr
                .as_ref()
                .filter(|e| style_constant(e, &cx).is_none());
            let runtime_material = v
                .material_expr
                .as_ref()
                .filter(|e| style_constant(&material_arg(e), &cx).is_none());
            if let Some(e) = runtime_color {
                let src = lower_color_value(ctx, e);
                for &host in &hosts {
                    ctx.builder.module.dynamic_colors.insert(host, src);
                }
            }
            if let Some(e) = runtime_material {
                let src = lower_material_value(ctx, e);
                for &host in &hosts {
                    ctx.builder.module.dynamic_materials.insert(host, src);
                }
            }
        }
        ctx.restore_top_view(view);
    }
    ctx.current_exec = saved_exec;
}

/// Resolve a module-level `@color` / `@material` into the root shell's
/// `root_style`: baked when constant, lowered in pure context and wired at
/// emit when not. Runs beside `resolve_module_label`, so it may
/// forward-reference declarations below it.
pub(super) fn resolve_module_style(
    ctx: &mut LowerCtx,
    color: Option<&Expr>,
    material: Option<&Expr>,
) {
    if color.is_none() && material.is_none() {
        return;
    }
    let (baked_color, baked_material) = baked_style(ctx, color, material);
    let saved_exec = ctx.current_exec.take();
    let color = match (color, baked_color) {
        (Some(_), Some(lit)) => Some(crate::ir::StyleValue::Baked(lit)),
        (Some(e), None) => Some(crate::ir::StyleValue::Wired(lower_color_value(ctx, e))),
        (None, _) => None,
    };
    let material = match (material, baked_material) {
        (Some(_), Some(lit)) => Some(crate::ir::StyleValue::Baked(lit)),
        (Some(e), None) => Some(crate::ir::StyleValue::Wired(lower_material_value(ctx, e))),
        (None, _) => None,
    };
    ctx.current_exec = saved_exec;
    ctx.builder.module.root_style = crate::ir::RootStyle { color, material };
}
