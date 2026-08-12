use super::*;

// ---------- pre-declaration pass ----------

pub(super) fn pre_declare_decl(ctx: &mut LowerCtx, d: &TopDecl) {
    match d {
        // Var/buffer gates are created HERE (pass 1), not in lower_decl's
        // with_nofold wrap — honor the decl's @nofold during registration.
        TopDecl::Var(v) => ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
        TopDecl::Array(a) => pre_declare_array(ctx, a),
        TopDecl::Map(m) => pre_declare_map(ctx, m),
        TopDecl::Buffer(b) => pre_declare_buffer(ctx, b),
        TopDecl::In(i) => pre_declare_input(ctx, i),
        TopDecl::Out(o) => ctx.with_nofold(o.no_fold, |ctx| {
            pre_declare_output(
                ctx,
                &o.name,
                o.value.as_ref(),
                o.typ.as_ref(),
                o.side,
                o.label.as_deref(),
                o.label_expr.as_ref(),
                o.invisible,
                &o.range,
            )
        }),
        TopDecl::Let(l) => pre_declare_exec_signal(ctx, l),
        TopDecl::AnonChip(ac) => {
            let chip_node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::MICROCHIP_ALT,
                source_range: ac.range.clone(),
                ports: GateIO::default(),
                ..Default::default()
            });
            if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
                node.kind = NodeKind::Chip;
                let props = std::sync::Arc::make_mut(&mut node.properties);
                if ac.closed {
                    props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
                }
                if let Some(label) =
                    resolve_label_text(ac.label.as_deref(), ac.label_expr.as_ref(), &ctx.const_env)
                {
                    props.insert(*sym::NAME_LABEL, Literal::String(label));
                }
                if let Some(doc) = ctx.doc_comments.get(&ac.range.start.offset) {
                    props.insert(*sym::DOC_TEXT, Literal::String(doc.clone()));
                }
            }
            // Tag pre-declared nodes with chip_id.
            let saved = ctx.current_anon_chip.take();
            ctx.current_anon_chip = Some(chip_node_id);
            for s in &ac.body.stmts {
                match s {
                    Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
                    Stmt::Buffer(b) => pre_declare_buffer(ctx, b),
                    Stmt::Array(a) => pre_declare_array(ctx, a),
                    Stmt::Map(m) => pre_declare_map(ctx, m),
                    Stmt::In(i) => pre_declare_input(ctx, i),
                    Stmt::OutBinding(o) if o.side.is_some() => {
                        report_non_root_side(ctx, &o.range);
                    }
                    _ => {}
                }
            }
            ctx.current_anon_chip = saved;
        }
        _ => {}
    }
}

/// Pre-declare a top-level `let x: exec` local signal: create a stable Union
/// "hub" gate, bind `x` to its `ExecOut` (so `on x` can trigger off it), and
/// register the emit target. `flush_pending_emits` later wires the union of all
/// `emit x` paths into the hub's `ExecA`. Non-`exec` lets are ignored here (they
/// lower normally in pass 2).
pub(super) fn pre_declare_exec_signal(ctx: &mut LowerCtx, l: &LetDecl) {
    let Some(TypeExpr::Name {
        name: type_name, ..
    }) = &l.typ
    else {
        return;
    };
    if type_name != "exec" {
        return;
    }
    let LetBinding::Ident { name, .. } = &l.binding else {
        return;
    };
    build_exec_signal_hub(ctx, name, &l.range);
}

/// Create the stable `Union` "hub" for a local `let x: exec` signal: bind `x`
/// to its `ExecOut` (so `await x` / `on x` / reads resolve to it) and register
/// the emit target. `flush_pending_emits` later wires the union of all `emit x`
/// paths into the hub's `ExecA`. Used for both top-level signals (this
/// pre-declare pass) and body-level signals (from `lower_let_decl`).
pub(super) fn build_exec_signal_hub(ctx: &mut LowerCtx, name: &str, range: &SourceRange) {
    let hub = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC_A,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::EXEC_B,
                    ty: Type::Exec,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
        },
        ..Default::default()
    });
    ctx.scope.insert(
        &name,
        Binding::Local(LocalRecord {
            port: hub.port(WirePort::ExecOut),
        }),
    );
    // Key the signal per-declaration (`name#hubId`), not by bare name: two
    // bodies declaring the same signal name are distinct signals. Emit/await
    // sites resolve the key through the scope binding (`LowerCtx::signal_key`).
    let key = format!("{name}#{hub}");
    ctx.exec_signal_hubs.insert(key.clone(), hub);
    ctx.exec_signal_keys.insert(hub, key.clone());
    ctx.pending_emits.entry(key).or_default();
}

/// Resolve a lowering-side type annotation to its `Type`. Delegates to the
/// crate's single canonical resolver (`types::resolve::resolve_type`); no
/// generic params or type aliases are in scope on this path (typecheck has
/// already resolved + flagged anything exotic with WS002), so an empty
/// params/aliases context is correct and the returned diagnostics are
/// discarded.
pub(super) fn type_of_type_expr(t: &TypeExpr) -> Type {
    let cx = crate::types::resolve::ResolveCtx {
        params: &[],
        type_aliases: &crate::collections::HashMap::default(),
        generic_aliases: &crate::collections::HashMap::default(),
    };
    crate::types::resolve::resolve_type(t, &cx, &mut Vec::new())
}

#[allow(dead_code)]
pub(super) fn is_entity_family(t: &Type) -> bool {
    matches!(
        t,
        Type::Controller | Type::Character | Type::Entity
    )
}

pub(super) use crate::types::mono::unwrap_ref;

/// Default initial literal for Pseudo_Var data structs. Only covers
/// primitive types that have a clean wire_graph_variant mapping.
/// Object/entity types are omitted — the game defaults them correctly.
/// Default initial literal for Pseudo_Var data structs so the game knows
/// the variable's wire_graph_variant type. Every Var must have one.
pub(super) fn default_literal_for_var_type(t: &Type) -> Option<Literal> {
    match t {
        Type::Bool => Some(Literal::Bool(false)),
        Type::Int => Some(Literal::Int(0)),
        Type::String => Some(Literal::String(String::new())),
        Type::Vector => Some(Literal::Vector {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        Type::Rotator => Some(Literal::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        }),
        Type::Quat => Some(Literal::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }),
        Type::Color => Some(Literal::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }),
        Type::Controller | Type::Character | Type::Entity => {
            Some(Literal::Object)
        }
        _ => Some(Literal::Float(0.0)),
    }
}

/// Compile-time constant environment: every top-level `let` whose initializer is
/// itself constant, by name. Lets an initializer name a constant (`1 << C_FLAG`)
/// instead of restating its value.
pub type ConstEnv = crate::collections::HashMap<String, Literal>;

/// Collect the constant top-level `let` bindings of a script.
///
/// Iterates to a fixpoint so a constant may be defined in terms of an earlier
/// one (`let A = 1` then `let B = A + 1`) regardless of declaration order — once
/// imports are merged, order is not dependency order. A binding that never
/// resolves (it needs a runtime value, or it is part of a reference cycle) is
/// simply absent, so callers fall back to their existing "not a constant" path.
pub fn build_const_env(decls: &[TopDecl]) -> ConstEnv {
    let mut env = ConstEnv::default();
    loop {
        let mut changed = false;
        for d in decls {
            let TopDecl::Let(l) = d else { continue };
            let LetBinding::Ident { name, .. } = &l.binding else {
                continue;
            };
            if env.contains_key(name) {
                continue;
            }
            if let Some(lit) = expr_to_literal_in(&l.value, &env) {
                env.insert(name.clone(), lit);
                changed = true;
            }
        }
        if !changed {
            return env;
        }
    }
}

/// Resolve an explicit `@label` override to its baked display text: the
/// string form (`@label("text")`) is used as-is; the expression form
/// (`@label(expr)`) is const-folded against the script's constant
/// environment via [`expr_to_literal_in`] — typecheck.rs already rejects a
/// non-constant expression, so a fold failure here just yields no override
/// (rather than double-reporting the error). `None` means "no override" —
/// the caller's own default (e.g. the decl's name) applies.
pub(super) fn resolve_label_text(
    label: Option<&str>,
    label_expr: Option<&Expr>,
    env: &ConstEnv,
) -> Option<String> {
    if let Some(s) = label {
        return Some(s.to_string());
    }
    let lit = expr_to_literal_in(label_expr?, env)?;
    Some(literal_to_label_text(&lit))
}

/// Render a folded `@label(expr)` literal as its baked display text.
fn literal_to_label_text(lit: &Literal) -> String {
    match lit {
        Literal::String(s) => s.clone(),
        Literal::Int(n) => n.to_string(),
        // A float reads the same 3-decimal / trailing-zero-trimmed way
        // FormatText renders one everywhere else (the certified render law),
        // not full `f64` precision.
        Literal::Float(f) => {
            crate::lower::fold::eval::render_for_format(&crate::lower::fold::eval::Value::Float(*f))
        }
        Literal::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Evaluate a constant unary operator. `None` = not foldable, which preserves
/// whatever error the caller would already have reported.
fn eval_const_unop(operator: &str, v: Literal) -> Option<Literal> {
    use crate::catalog::operators::op;
    match (operator, v) {
        (op::NEG, Literal::Int(n)) => Some(Literal::Int(n.wrapping_neg())),
        (op::NEG, Literal::Float(f)) => Some(Literal::Float(-f)),
        (op::NOT, Literal::Bool(b)) => Some(Literal::Bool(!b)),
        (op::BIT_NOT, Literal::Int(n)) => Some(Literal::Int(!n)),
        _ => None,
    }
}

/// Evaluate a constant binary operator, matching the gates' certified
/// semantics: 64-bit integer maths, and division / modulo by zero yielding 0
/// rather than trapping. Anything outside this set — or any operand pair whose
/// result would be ambiguous — returns `None` and stays an error, so this can
/// only ever turn a rejected program into a working one, never change the
/// meaning of one that already compiles.
fn eval_const_binop(operator: &str, l: Literal, r: Literal) -> Option<Literal> {
    use crate::catalog::operators::op;
    use Literal::{Bool, Float, Int, String as Str};

    // String concatenation is the one non-numeric binary fold.
    if let (Str(a), Str(b)) = (&l, &r) {
        return match operator {
            op::CONCAT => Some(Str(format!("{a}{b}"))),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            _ => None,
        };
    }
    if let (Bool(a), Bool(b)) = (&l, &r) {
        let (a, b) = (*a, *b);
        return match operator {
            op::AND => Some(Bool(a && b)),
            op::OR => Some(Bool(a || b)),
            op::XOR => Some(Bool(a != b)),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            _ => None,
        };
    }
    // Two ints stay integral (bitwise and shifts are int-only); any float
    // operand promotes the pair to float, mirroring the operator overloads.
    if let (Int(a), Int(b)) = (&l, &r) {
        let (a, b) = (*a, *b);
        return match operator {
            op::ADD => Some(Int(a.wrapping_add(b))),
            op::SUB => Some(Int(a.wrapping_sub(b))),
            op::MUL => Some(Int(a.wrapping_mul(b))),
            op::DIV => Some(Int(if b == 0 { 0 } else { a.wrapping_div(b) })),
            op::REM => Some(Int(if b == 0 { 0 } else { a.wrapping_rem(b) })),
            op::BIT_AND => Some(Int(a & b)),
            op::BIT_OR => Some(Int(a | b)),
            op::BIT_XOR => Some(Int(a ^ b)),
            // A shift distance outside 0..64 is left unfolded rather than guessed.
            op::SHL => (0..64).contains(&b).then(|| Int(a << b)),
            op::SHR => (0..64).contains(&b).then(|| Int(a >> b)),
            op::EQ => Some(Bool(a == b)),
            op::NE => Some(Bool(a != b)),
            op::LT => Some(Bool(a < b)),
            op::LE => Some(Bool(a <= b)),
            op::GT => Some(Bool(a > b)),
            op::GE => Some(Bool(a >= b)),
            _ => None,
        };
    }
    let num = |v: &Literal| match v {
        Int(n) => Some(*n as f64),
        Float(f) => Some(*f),
        _ => None,
    };
    let (a, b) = (num(&l)?, num(&r)?);
    // Match the gates: a non-finite result reads as 0.
    let fin = |f: f64| Float(if f.is_finite() { f } else { 0.0 });
    match operator {
        op::ADD => Some(fin(a + b)),
        op::SUB => Some(fin(a - b)),
        op::MUL => Some(fin(a * b)),
        op::DIV => Some(if b == 0.0 { Float(0.0) } else { fin(a / b) }),
        op::REM => Some(if b == 0.0 { Float(0.0) } else { fin(a % b) }),
        op::EQ => Some(Bool(a == b)),
        op::NE => Some(Bool(a != b)),
        op::LT => Some(Bool(a < b)),
        op::LE => Some(Bool(a <= b)),
        op::GT => Some(Bool(a > b)),
        op::GE => Some(Bool(a >= b)),
        _ => None,
    }
}

/// Fold a constant-literal expression to a [`Literal`] (used for var/array
/// initial values). Returns `None` for anything that isn't a compile-time
/// constant. Shared with the type checker so both agree on what's a literal.
///
/// This is the environment-free form. It folds only what is constant on its
/// face — literals, a negated literal, and literal-argument constructors — and
/// deliberately does NOT resolve names or evaluate operators.
///
/// That restraint is load-bearing. This function decides whether a value bakes
/// into a gate's data or gets a real wired gate, and it is used well beyond
/// initializers (port values, buffer delays, handler fields). Folding `a + b`
/// here would silently delete gates from programs that already compile — e.g.
/// `Rotation(0.0 + 0.0, ...)` would collapse to a `_Literal(Rotator)` instead of
/// emitting the `MakeRotation` gate it must. Use [`expr_to_literal_in`] for the
/// initializer paths, where richer constants are wanted.
pub fn expr_to_literal(e: &Expr) -> Option<Literal> {
    expr_to_literal_impl(e, None)
}

/// [`expr_to_literal`], plus named top-level constants and operators over them —
/// so an initializer can read `1 << C_FLAG` or `WIDTH * HEIGHT` instead of a
/// magic number. Only the `var` initializer paths pass an environment;
/// everywhere else keeps the narrower behaviour above.
pub fn expr_to_literal_in(e: &Expr, env: &ConstEnv) -> Option<Literal> {
    expr_to_literal_impl(e, Some(env))
}

/// `env == None` reproduces the original syntactic folding exactly; `Some`
/// additionally resolves named constants and evaluates operators.
fn expr_to_literal_impl(e: &Expr, env: Option<&ConstEnv>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // -- Constant-environment forms: initializers only. --
        // With no environment these fall through to `_ => None`, exactly as
        // before this was added.
        Expr::Ident { name, .. } => env?.get(name).cloned(),
        Expr::BinOp {
            op, left, right, ..
        } if env.is_some() => eval_const_binop(op, expr_to_literal(left)?, expr_to_literal(right)?),
        Expr::UnOp { op, operand, .. }
            if env.is_some() && op != crate::catalog::operators::op::NEG =>
        {
            eval_const_unop(op, expr_to_literal(operand)?)
        }
        // -- Always-constant forms (unchanged). --
        Expr::IntLit { value, .. } => Some(Literal::Int(*value)),
        Expr::AtomLit { value, .. } => Some(Literal::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Literal::Float(*value)),
        Expr::BoolLit { value, .. } => Some(Literal::Bool(*value)),
        Expr::StringLit { value, .. } => Some(Literal::String(value.clone())),
        // Negative numeric literals: `-5`, `-1.0`.
        Expr::UnOp { op, operand, .. } if op == crate::catalog::operators::op::NEG => {
            match expr_to_literal(operand)? {
                Literal::Int(n) => Some(Literal::Int(n.wrapping_neg())),
                Literal::Float(f) => Some(Literal::Float(-f)),
                _ => None,
            }
        }
        _ => expr_to_literal_lit(e, env),
    }
}

/// The constructor / reference cases, split out to keep the dispatch above
/// readable.
fn expr_to_literal_lit(e: &Expr, env: Option<&ConstEnv>) -> Option<Literal> {
    let expr_to_literal = |e: &Expr| expr_to_literal_impl(e, env);
    match e {
        // Constructor calls on constant numeric args fold to literals, so
        // `var v = Vec(1.0, 2.0, 3.0)` (and Rotation/Color) bakes into the
        // gate's initial value instead of being dropped.
        Expr::Call { callee, args, .. } => {
            let Expr::Ident { name, .. } = callee.as_ref() else {
                return None;
            };
            let mut nums = Vec::with_capacity(args.len());
            for a in args {
                let CallArg::Positional(arg) = a else {
                    return None;
                };
                match expr_to_literal(arg) {
                    Some(Literal::Int(n)) => nums.push(n as f64),
                    Some(Literal::Float(f)) => nums.push(f),
                    _ => return None,
                }
            }
            match (name.as_str(), nums.as_slice()) {
                ("Vec", &[x, y, z]) => Some(Literal::Vector { x, y, z }),
                ("Rotation", &[pitch, yaw, roll]) => Some(Literal::Rotator { pitch, yaw, roll }),
                // Color is linear RGBA 0–1; alpha defaults to opaque.
                ("Color", &[r, g, b]) => Some(Literal::LinearColor { r, g, b, a: 1.0 }),
                ("Color", &[r, g, b, a]) => Some(Literal::LinearColor { r, g, b, a }),
                _ => None,
            }
        }
        // Asset reference `$Type/Name` — inlined into the gate's component data.
        Expr::AssetRef {
            asset_type,
            asset_name,
            ..
        } => Some(Literal::Asset {
            asset_type: asset_type.clone(),
            asset_name: asset_name.clone(),
        }),
        // Prefab file reference `$./file.brz` — inlined; resolved + embedded
        // at emit into the gate's `bundle_path_ref` property.
        Expr::PrefabRef { path, .. } => Some(Literal::PrefabRef { path: path.clone() }),
        // Inline nested-prefab block `$``` ... ``` ` — inlined; compiled +
        // embedded at emit into the gate's `bundle_path_ref` property.
        Expr::NestedPrefab { source, .. } => Some(Literal::NestedPrefab {
            source: source.clone(),
        }),
        _ => None,
    }
}

/// Fold a single array-literal element to a constant [`Literal`]. Spreads have
/// no constant form (they're only valid in exec-context assignments), so they
/// fold to `None` — which makes the all-literal length check fail and the
/// initializer is left empty (the type checker has already reported the error).
fn array_elem_literal(el: &ArrayElem, env: &ConstEnv) -> Option<Literal> {
    match el {
        ArrayElem::Item(e) => expr_to_literal_in(e, env),
        ArrayElem::Spread(_) => None,
    }
}

/// Fold a constant `ColorSRGB(r, g, b, a)` call to a `Literal::Color` — sRGB u8
/// with NO gamma re-encoding (Brickadia brick colours are stored sRGB-direct).
/// Only the four-arg `ColorSRGB` form (0–255 ints, the natural sRGB source) is
/// accepted; anything else — including the linear `Color(..)` constructor,
/// whose 0–1 components would need an ambiguous gamma conversion — folds to
/// `None`, so the caller reports a clean error instead of guessing bytes.
pub(crate) fn fold_srgb_color(e: &Expr) -> Option<Literal> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident { name, .. } = callee.as_ref() else {
        return None;
    };
    if name != "ColorSRGB" {
        return None;
    }
    let mut nums: Vec<i64> = Vec::with_capacity(args.len());
    for a in args {
        let CallArg::Positional(arg) = a else {
            return None;
        };
        match expr_to_literal(arg)? {
            Literal::Int(n) => nums.push(n),
            Literal::Float(f) => nums.push(f as i64),
            _ => return None,
        }
    }
    let [r, g, b, a] = nums.as_slice() else {
        return None;
    };
    let clamp_u8 = |n: i64| n.clamp(0, 255) as u8;
    Some(Literal::Color {
        r: clamp_u8(*r),
        g: clamp_u8(*g),
        b: clamp_u8(*b),
        a: clamp_u8(*a),
    })
}

/// Fold a `meshColors` argument — an array literal of constant `ColorSRGB`
/// colours — to `Literal::Array(Literal::Color…)` for a gate's `MeshColors:
/// Color[]` data field. Returns `None` (a clean call-site error) if any element
/// is non-constant or not a `ColorSRGB(..)`; spreads never fold.
pub(crate) fn fold_mesh_colors(e: &Expr) -> Option<Literal> {
    let Expr::Array { elements, .. } = e else {
        return None;
    };
    let mut colors = Vec::with_capacity(elements.len());
    for el in elements {
        let ArrayElem::Item(item) = el else {
            return None;
        };
        colors.push(fold_srgb_color(item)?);
    }
    Some(Literal::Array(colors))
}

/// Fold an `ammoOverride` argument — a record literal `{ overrideStartingAmmo:
/// bool, resources: [{ loaded: int, reserve: int }] }` — for a gate's
/// `WeaponAmmoOverride` nested-struct data field. Encoded in existing `Literal`
/// variants (no new variant is introduced):
/// `Array[ Bool(overrideStartingAmmo), Array[ Array[Int(loaded), Int(reserve)],
/// … ] ]`; the emitter decodes this exact shape. Returns `None` on any
/// non-constant value or unexpected field.
pub(crate) fn fold_ammo_override(e: &Expr) -> Option<Literal> {
    let Expr::RecordLit { fields, .. } = e else {
        return None;
    };
    let mut override_starting = None;
    let mut resources: Option<Vec<Literal>> = None;
    for f in fields {
        let crate::ast::RecordLitField::Named { name, value, .. } = f else {
            return None;
        };
        match name.as_str() {
            "overrideStartingAmmo" => {
                let Literal::Bool(b) = expr_to_literal(value)? else {
                    return None;
                };
                override_starting = Some(b);
            }
            "resources" => {
                let Expr::Array { elements, .. } = value else {
                    return None;
                };
                let mut rs = Vec::with_capacity(elements.len());
                for el in elements {
                    let ArrayElem::Item(item) = el else {
                        return None;
                    };
                    rs.push(fold_resource_amount(item)?);
                }
                resources = Some(rs);
            }
            _ => return None,
        }
    }
    // Both fields are required. A missing one is a user mistake — reject it (the
    // caller reports WS028 with the expected shape) rather than silently
    // defaulting `overrideStartingAmmo` to false / `resources` to empty. An
    // explicit `resources: []` is still accepted (the key is present).
    Some(Literal::Array(vec![
        Literal::Bool(override_starting?),
        Literal::Array(resources?),
    ]))
}

/// One `{ loaded, reserve }` resource of `ammoOverride.resources`, folded to
/// `Array[Int(loaded), Int(reserve)]` (see [`fold_ammo_override`]).
fn fold_resource_amount(e: &Expr) -> Option<Literal> {
    let Expr::RecordLit { fields, .. } = e else {
        return None;
    };
    let mut loaded = None;
    let mut reserve = None;
    for f in fields {
        let crate::ast::RecordLitField::Named { name, value, .. } = f else {
            return None;
        };
        let Literal::Int(n) = expr_to_literal(value)? else {
            return None;
        };
        match name.as_str() {
            "loaded" => loaded = Some(n),
            "reserve" => reserve = Some(n),
            _ => return None,
        }
    }
    // Both fields are required — a missing `loaded`/`reserve` is rejected (WS028)
    // rather than silently baked as 0.
    Some(Literal::Array(vec![
        Literal::Int(loaded?),
        Literal::Int(reserve?),
    ]))
}

/// Coerce a constant literal to a declared scalar type, matching the gate
/// coercion laws the array bake path uses (string→bool via `!= ""`, and
/// numeric int/float/bool normalization). Identity for anything already the
/// right kind or with no defined coercion.
///
/// Without this, a coercion-mixed map entry (e.g. `Map<int, bool> = { 1 =>
/// "on" }`) would bake its RAW folded literal (`String("on")`), which emit's
/// `wire_map_variant_from_literals` then can't match against the declared
/// value kind and silently zero-falls-back to `false` — a typechecked
/// program baking the wrong data.
fn coerce_literal_to_type(lit: Literal, ty: &Type) -> Literal {
    match ty {
        Type::Int => match lit {
            Literal::Int(n) => Literal::Int(n),
            Literal::Float(f) => Literal::Int(f as i64),
            Literal::Bool(b) => Literal::Int(b as i64),
            other => other,
        },
        Type::Float => match lit {
            Literal::Float(f) => Literal::Float(f),
            Literal::Int(n) => Literal::Float(n as f64),
            Literal::Bool(b) => Literal::Float(b as i64 as f64),
            other => other,
        },
        Type::Bool => match lit {
            Literal::Bool(b) => Literal::Bool(b),
            Literal::Int(n) => Literal::Bool(n != 0),
            Literal::String(s) => Literal::Bool(!s.is_empty()), // the `!= ""` law
            other => other,
        },
        // string / vector / rotator / quat / color / object: no cross-coercion here.
        _ => lit,
    }
}

/// Fold a map-literal entry to a `(key, value)` literal pair coerced to the
/// declared `key_ty`/`val_ty`, or `None` if either side isn't a compile-time
/// constant.
fn map_entry_literal(
    e: &crate::ast::MapLitEntry,
    env: &ConstEnv,
    key_ty: &Type,
    val_ty: &Type,
) -> Option<(Literal, Literal)> {
    Some((
        coerce_literal_to_type(expr_to_literal_in(&e.key, env)?, key_ty),
        coerce_literal_to_type(expr_to_literal_in(&e.value, env)?, val_ty),
    ))
}

/// Bake a constant map-literal initializer (`var m: Map<K, V> = {...}`) into
/// `properties` as an `InitialValue` (`Literal::Map`) — zero runtime gates,
/// exactly like the array path bakes `Literal::Array`.
/// Shared by [`pre_declare_map`] and the `Map<K, V>` branch of
/// [`pre_declare_var`] since both bake the same way. Non-constant entries
/// can't bake at a (pure) decl: the map starts empty and a warning is
/// raised (lowering handles the exec-context desugar for `m = {…}`).
///
/// `key_ty`/`val_ty` are the declared `Map<K, V>` types — every entry is
/// coerced to them at fold time (see [`coerce_literal_to_type`]) so the baked
/// `Literal::Map` is already correct, not a raw literal emit has to guess at.
fn bake_map_init(
    ctx: &mut LowerCtx,
    properties: &mut HashMap<crate::intern::Sym, Literal>,
    name: &str,
    init: &Option<Expr>,
    key_ty: &Type,
    val_ty: &Type,
) {
    let Some(Expr::MapLit { entries, .. }) = init else {
        return;
    };
    // Object/asset-family keys have no literal representation — `key_of`
    // bakes every one as `Object(None)`, so two or more entries would
    // collapse onto the same null key (a corrupt map with duplicate keys).
    // Fall to the non-constant path instead: warn + start empty.
    if matches!(
        key_ty,
        Type::Entity | Type::Character | Type::Controller
    ) {
        ctx.warn(
            format!(
                "'{name}' initializer has object/asset-typed keys, which can't bake as literals — it starts empty; assign entries inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        );
        return;
    }
    let pairs: Vec<(Literal, Literal)> = entries
        .iter()
        .filter_map(|en| map_entry_literal(en, &ctx.const_env, key_ty, val_ty))
        .collect();
    if pairs.len() == entries.len() {
        properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
    } else {
        // Non-constant entries can't bake at a (pure) decl; the map starts
        // empty. (lowering handles the exec-context desugar for `m = {…}`.)
        ctx.warn(
            format!(
                "'{name}' initializer has non-constant entries — they are dropped here; assign them inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        );
    }
}

/// A `var` initializer that can't bake into the gate as a constant: returns it
/// for diagnosis. `None` = no initializer, or it bakes fine.
fn var_init_unbaked<'a>(v: &'a VarDecl, env: &ConstEnv) -> Option<&'a Expr> {
    let init = v.init.as_ref()?;
    let unbaked = match init {
        Expr::Array { elements, .. } => elements
            .iter()
            .any(|el| array_elem_literal(el, env).is_none()),
        // A map literal is baked — and any non-bakeable case (object keys,
        // non-constant entries) is warned — by `bake_map_init`, the single
        // authority on map-init diagnostics. A constant `{ "k": v }` bakes as a
        // `Literal::Map` InitialValue, so it is NOT unbaked; never double-report
        // it here with the generic "not a compile-time constant" message.
        Expr::MapLit { .. } => false,
        e => expr_to_literal_in(e, env).is_none(),
    };
    unbaked.then_some(init)
}

/// Warn when a `var` initializer is silently dropped: it can't bake into the
/// Variable gate as a constant, and no exec-context reset will apply it (the
/// var is in pure position, or is `static`, which skips the per-entry reset) —
/// so the var starts at its type default. `skip_array_inits` avoids
/// double-reporting top-level array literals the type checker already errors
/// on.
pub(super) fn warn_unbaked_var_init(ctx: &mut LowerCtx, v: &VarDecl, skip_array_inits: bool) {
    let Some(init) = var_init_unbaked(v, &ctx.const_env.clone()) else {
        return;
    };
    if skip_array_inits && matches!(init, Expr::Array { .. }) {
        return;
    }
    let msg = if v.is_static {
        format!(
            "'static var {}' initializer must be a compile-time constant — this value is dropped and the var starts at its type default",
            v.name
        )
    } else {
        format!(
            "'var {}' initializer is not a compile-time constant — outside an exec context it is dropped and the var starts at its type default; assign the value inside an exec handler instead",
            v.name
        )
    };
    ctx.warn(msg, init.range());
}

pub(super) fn pre_declare_var(ctx: &mut LowerCtx, d: &VarDecl) {
    // `resolve_local_type` monomorphizes a `T` annotation inside a generic mod
    // body (and is identical to `type_of_type_expr` everywhere else).
    let inner_type = d
        .typ
        .as_ref()
        .map(|te| ctx.resolve_local_type(te))
        .or_else(|| d.init.as_ref().map(|e| ctx.type_of(e)))
        .unwrap_or(Type::Any);

    // `var foo: T[]` is an array — desugar to an ArrayVar gate so the array
    // methods actually work. A `= [..]` initializer carries its constant
    // literals inline (mirrors the map path below).
    if let Type::Array(elem) = &inner_type {
        let elem_type = elem.as_ref().clone();
        let mut properties = HashMap::default();
        let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
            .unwrap_or_else(|| d.name.clone());
        properties.insert(*sym::NAME_LABEL, Literal::String(label));
        if let Some(Expr::Array { elements, .. }) = &d.init {
            // Element-wise compile-time string → bool for `var v: bool[] =
            // [..]` — same `!= ""` law as the wire path's CompareNotEqual
            // gate (see `bake_string_bool`).
            let lits: Vec<Literal> = elements
                .iter()
                .filter_map(|el| array_elem_literal(el, &ctx.const_env))
                .map(|lit| bake_string_bool(lit, &elem_type))
                .collect();
            if lits.len() == elements.len() {
                properties.insert(intern_static("InitialValue"), Literal::Array(lits));
            }
        }
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::PSEUDO_ARRAY_VAR,
            source_range: d.range.clone(),
            ports: GateIO {
                inputs: vec![],
                outputs: vec![PortSpec {
                    name: *sym::ARRAY_VAR_REF,
                    ty: Type::Ref(Box::new(Type::Array(Box::new(elem_type.clone())))),
                }],
            },
            properties,
            note: None,
            ..Default::default()
        });
        ctx.scope.insert(
            &d.name,
            Binding::Var(VarRecord {
                node_id,
                inner_type: elem_type,
                get_node_for_handler: None,
                storage: VarStorage::Array,
            }),
        );
        return;
    }

    // `var m: Map<K, V>` is a map — desugar to a MapVar gate so the map
    // methods work. A constant `= {...}` initializer bakes via
    // `bake_map_init`.
    if let Type::Map(key_ty, value_ty) = &inner_type {
        let (key_ty, value_ty) = (key_ty.as_ref().clone(), value_ty.as_ref().clone());
        let mut properties = HashMap::default();
        let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
            .unwrap_or_else(|| d.name.clone());
        properties.insert(*sym::NAME_LABEL, Literal::String(label));
        bake_map_init(ctx, &mut properties, &d.name, &d.init, &key_ty, &value_ty);
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::PSEUDO_MAP_VAR,
            source_range: d.range.clone(),
            ports: GateIO {
                inputs: vec![],
                outputs: vec![PortSpec {
                    name: *sym::MAP_VAR_REF,
                    ty: Type::Ref(Box::new(inner_type.clone())),
                }],
            },
            properties,
            note: None,
            ..Default::default()
        });
        ctx.scope.insert(
            &d.name,
            Binding::Var(VarRecord {
                node_id,
                inner_type,
                get_node_for_handler: None,
                storage: VarStorage::Map,
            }),
        );
        return;
    }

    let init_lit = d
        .init
        .as_ref()
        .and_then(expr_to_literal)
        // Compile-time string → bool: `var v: bool = "x"` bakes
        // Bool(!s.is_empty()) — the same `!= ""` law as the runtime
        // `CompareNotEqual` gate on the wire path (see `bake_string_bool`).
        // A raw String InitialValue on a Bool var would start under the
        // gate's NATIVE truthiness instead ("0"/"false" falsy) — a silent
        // divergence from the documented law.
        .map(|lit| bake_string_bool(lit, &inner_type))
        .or_else(|| default_literal_for_var_type(&inner_type));
    let mut properties = HashMap::default();
    let label = resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
        .unwrap_or_else(|| d.name.clone());
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    if let Some(lit) = init_lit {
        properties.insert(*sym::INITIAL_VALUE, lit);
    }

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(inner_type.clone())),
                },
            ],
        },
        properties,
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord {
            node_id,
            inner_type,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        }),
    );
}

pub(super) fn pre_declare_buffer(ctx: &mut LowerCtx, d: &BufferDecl) {
    let annotated = d.typ.as_ref().map(|te| ctx.resolve_local_type(te));
    let rhs_type = ctx.type_of(&d.init);
    let inner_type = annotated.unwrap_or_else(|| unwrap_ref(&rhs_type));

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BUFFER_TICKS,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::INPUT,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::TICKS_TO_WAIT,
                    ty: Type::Int,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: inner_type.clone(),
            }],
        },
        properties: [(*sym::TICKS_TO_WAIT, Literal::Int(1))]
            .into_iter()
            .collect(),
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Buffer(NodeRecord {
            node_id,
            ty: inner_type,
        }),
    );
}

pub(super) fn pre_declare_array(ctx: &mut LowerCtx, d: &ArrayDecl) {
    let elem_type = ctx.resolve_local_type(&d.element_type);
    // Constant initializer (`var foo: int[] = [1, 2, 3]`): every element must
    // be a literal. Carry the values as an `InitialValue` property the emitter
    // writes straight into the ArrayVar's array variant (no runtime gates).
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    if !d.init.is_empty() {
        // Element-wise compile-time string → bool for `var a: bool[] =
        // ["x", ""]` → [true, false] — same `!= ""` law as the wire path's
        // CompareNotEqual gate (see `bake_string_bool`); a raw String
        // element in a Bool array variant would diverge to the gate's
        // native content-aware truthiness at load.
        let lits: Vec<Literal> = d
            .init
            .iter()
            .filter_map(|el| array_elem_literal(el, &ctx.const_env))
            .map(|lit| bake_string_bool(lit, &elem_type))
            .collect();
        if lits.len() == d.init.len() {
            properties.insert(intern_static("InitialValue"), Literal::Array(lits));
        }
    }
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_ARRAY_VAR,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::ARRAY_VAR_REF,
                ty: Type::Ref(Box::new(Type::Array(Box::new(elem_type.clone())))),
            }],
        },
        properties,
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord {
            node_id,
            inner_type: elem_type,
            get_node_for_handler: None,
            storage: VarStorage::Array,
        }),
    );
}

/// `var name: Map<K, V>` — create the backing `Pseudo_MapVar` gate (exposing a
/// `MapVarRef`) and bind the name as a `VarStorage::Map` whose `inner_type`
/// carries the whole `Type::Map(K, V)`. Mirrors [`pre_declare_array`].
pub(super) fn pre_declare_map(ctx: &mut LowerCtx, d: &crate::ast::MapDecl) {
    let key_type = ctx.resolve_local_type(&d.key_type);
    let value_type = ctx.resolve_local_type(&d.value_type);
    let map_type = Type::Map(Box::new(key_type.clone()), Box::new(value_type.clone()));
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    bake_map_init(
        ctx,
        &mut properties,
        &d.name,
        &d.init,
        &key_type,
        &value_type,
    );
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_MAP_VAR,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::MAP_VAR_REF,
                ty: Type::Ref(Box::new(map_type.clone())),
            }],
        },
        properties,
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord {
            node_id,
            inner_type: map_type,
            get_node_for_handler: None,
            storage: VarStorage::Map,
        }),
    );
}

/// Push the WS023 "annotation on a non-root port" diagnostic. Shared so the
/// message text has a single source (apply_port_side and the anon-chip output
/// path both use it).
fn report_non_root_side(ctx: &mut LowerCtx, range: &SourceRange) {
    ctx.diagnostics.push(Diagnostic::error(
        "WS023",
        "side annotations only apply to top-level ports of the compiled file",
        range.clone(),
    ));
}

/// Attach a `@side` annotation to a freshly created I/O node, or reject it
/// with WS023 when the port doesn't belong to the root module (chip/mod
/// bodies, anonymous chips). Also carries the `@invisible` flag onto the
/// same node when the port declared it.
fn apply_port_side(
    ctx: &mut LowerCtx,
    node_id: NodeId,
    side: Option<crate::ast::PortSide>,
    invisible: bool,
    range: &SourceRange,
) {
    if invisible {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*crate::intern::sym::REROUTE_INVISIBLE, Literal::Bool(true));
        }
    }
    let Some(side) = side else { return };
    if !ctx.is_root_module || ctx.current_anon_chip.is_some() {
        report_non_root_side(ctx, range);
        return;
    }
    if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(
            *crate::intern::sym::REROUTE_SIDE,
            Literal::String(side.as_str().to_string()),
        );
    }
}

pub(super) fn pre_declare_input(ctx: &mut LowerCtx, d: &InDecl) {
    // A record-typed input port (inline `{ … }`, a non-generic `type P = { … }`,
    // or a generic `type Pair<T> = { … }` instantiated as `Pair<int>`) dissolves
    // into one sub-port per field, bound as a `Record` so `p.field` reads the
    // right sub-port. Without this a record port collapsed to a single `any`
    // port and its field accesses lowered to `_Unsupported`/swizzle gates —
    // mirrors the standalone-chip input expansion in `lower::mod`.
    if let Some(fields) = ctx.record_fields_of(&d.typ) {
        let mut record_fields = HashMap::default();
        for field in &fields {
            let port_name = format!("{}_{}", d.name, field.name);
            let ft = type_of_type_expr(&field.typ);
            let node_id = ctx.add_input(&port_name, ft.clone(), d.range.clone());
            // Array / Map / ref fields of a record-typed input port bind a
            // container ref-port (see `container_binding`); a scalar field is a
            // plain by-value input.
            let binding = match super::context::container_binding(&field.typ, &ft) {
                Some((storage, inner)) => Binding::Var(VarRecord {
                    node_id,
                    inner_type: inner,
                    get_node_for_handler: None,
                    storage,
                }),
                None => Binding::Input(NodeRecord {
                    node_id,
                    ty: ft.clone(),
                }),
            };
            record_fields.insert(crate::intern::intern(&field.name), binding);
        }
        ctx.scope.insert(&d.name, Binding::Record(record_fields));
        return;
    }
    let t = type_of_type_expr(&d.typ);
    let node_id = ctx.add_input(&d.name, t.clone(), d.range.clone());
    apply_port_side(ctx, node_id, d.side, d.invisible, &d.range);
    if let Some(label) =
        resolve_label_text(d.label.as_deref(), d.label_expr.as_ref(), &ctx.const_env)
    {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label));
        }
    }
    ctx.scope
        .insert(&d.name, Binding::Input(NodeRecord { node_id, ty: t }));
}

pub(super) fn pre_declare_output(
    ctx: &mut LowerCtx,
    name: &str,
    value: Option<&Expr>,
    typ: Option<&TypeExpr>,
    side: Option<crate::ast::PortSide>,
    label: Option<&str>,
    label_expr: Option<&Expr>,
    invisible: bool,
    range: &SourceRange,
) {
    // An explicit annotation IS the port's type — the value coerces INTO it
    // (typecheck validates the pair; the `ctx.connect` choke point inserts
    // any adapter, e.g. the string → bool `!= ""` compare). Deriving the
    // port type from the VALUE instead silently made `out y: bool = s` a
    // string port. Ref annotations unwrap so `out y: *int = x` keeps the
    // value-typed port it always had (the ref-ness lives in the AST/emit
    // handling, not the pin type).
    let t = if let (Some(te), Some(_)) = (typ, value) {
        unwrap_ref(&ctx.resolve_local_type(te))
    } else if let Some(v) = value {
        unwrap_ref(&ctx.type_of(v))
    } else if let Some(te) = typ {
        ctx.resolve_local_type(te)
    } else {
        Type::Any
    };
    let node_id = ctx.add_output(name, t.clone(), range.clone());
    apply_port_side(ctx, node_id, side, invisible, range);
    if let Some(label) = resolve_label_text(label, label_expr, &ctx.const_env) {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label));
        }
    }
    ctx.scope.insert(
        &crate::lower::context::output_scope_key(name),
        Binding::Output(NodeRecord { node_id, ty: t }),
    );
}
