//! `for`-loop lowering, and the range normalisation it rests on.
//!
//! A `for` compiles to a callable-loop microchip: a shell brick on the
//! enclosing grid, driven by the pins in `ir::plane::PlaneSpec`, whose plane
//! holds a `CallableEntry_*` gate and the loop body.
//!
//! [`normalize_range`] is the one place every range spelling (`a..b`,
//! `.step(k)`, `.rev()`, `count(..)`) collapses onto the shell's `Count`,
//! `FirstIndex` and `Step`. It is separate from lowering because its
//! semantics are independently reviewable and independently wrong-able.

use super::*;
use crate::ast::{Expr, RangeExtent, RangeSource};
use crate::ir::plane::{
    ForSlot, LoopKind, PlaneSpec, SHELL_OUTPUTS, for_slots, plane_spec, slot_types,
};
use crate::typecheck::ContainerKind;

/// One node of a synthesized shell-pin expression tree.
///
/// The point of the split is [`PinExpr::User`]: a sub-expression the author
/// actually wrote, which typecheck walked. Everything else this module
/// fabricates, and fabricated arithmetic cannot go through `lower_expr`. See
/// [`normalize_range`].
#[derive(Clone, Debug)]
pub(crate) enum PinExpr {
    /// A user-written expression, carried through unchanged. Safe on the
    /// ordinary `lower_expr` path because typecheck walked this exact node
    /// and registered its operators.
    User(Expr),
    /// A synthesized integer constant: the default step of `1`, the default
    /// `from` of `0`, or the `0` in the max-clamp.
    Int(i64),
    /// A synthesized float constant. Currently only the `1.0` that forces
    /// float division ahead of `ceil`.
    Float(f64),
    /// Synthesized arithmetic, lowered as a direct Math gate picked from the
    /// operand types rather than from `op_resolutions`.
    Bin {
        op: &'static str,
        lhs: Box<PinExpr>,
        rhs: Box<PinExpr>,
    },
    /// A synthesized builtin call (`ceil`, `max`), resolved by callee name
    /// like any other builtin.
    Call {
        name: &'static str,
        args: Vec<PinExpr>,
    },
    /// The loop's own `Count`, already lowered. `.rev()` builds `FirstIndex`
    /// out of `Count`, and a second copy of that tree would emit the whole
    /// subtract, widen, divide, ceil and max chain twice.
    CountPin,
}

/// The three shell driver pins a normalised range produces.
pub(crate) struct RangePins {
    pub count: PinExpr,
    pub first_index: PinExpr,
    pub step: PinExpr,
}

/// Normalise any range spelling onto the shell's `(Count, FirstIndex, Step)`.
///
/// `a..b` excludes `b`, and `.step(k)` follows Rust's `step_by`: the stride
/// changes how many iterations there are, so `Count` is a ceil-divide rather
/// than a plain difference. `.rev()` starts at the last index the forward
/// range would have produced and walks back with a negated stride, leaving
/// `Count` unchanged. The `count(n, from = .., step = ..)` spelling drives the
/// pins directly, with no bound to exclude or divide.
///
/// The ceil-divide is wrapped in `max(0, ..)`, so a backwards or empty range
/// (`10..0`, `5..5`) is zero iterations rather than a negative `Count`, which
/// is undefined as far as this compiler knows.
///
/// # Why this returns [`PinExpr`], not a bare `Expr`
///
/// `lower::ops::lower_binop` refuses any `Expr::BinOp` whose source range has
/// no entry in `LowerCtx::op_resolutions`, substituting an `_Unsupported`
/// placeholder. Only typecheck's own AST walk populates that map, so a
/// `BinOp` synthesized here would never have an entry, and typecheck cannot
/// be made to add one: every node built from a single `RangeSource` shares
/// that source's one range, so `end - start` and `(end - start) * 1.0` would
/// collide on one key.
///
/// Flattening this back to bare `Expr`s therefore compiles, passes every test
/// with a constant range (typecheck pre-folds those), and silently produces a
/// save that loads and does nothing the moment a bound is a variable.
pub(crate) fn normalize_range(r: &RangeSource) -> RangePins {
    let step = r.step.clone().map(PinExpr::User).unwrap_or(PinExpr::Int(1));

    let (count, first_index) = match &r.extent {
        RangeExtent::Count { count, from } => (
            PinExpr::User(count.clone()),
            from.clone().map(PinExpr::User).unwrap_or(PinExpr::Int(0)),
        ),
        RangeExtent::Bounds { start, end } => {
            let span = bin("-", PinExpr::User(end.clone()), PinExpr::User(start.clone()));
            // `int / int` in this language truncates, so `ceil` would receive
            // an already-integral quotient and do nothing. Multiplying by a
            // float constant selects the `(Float, Int) -> Float` divide, which
            // leaves `ceil` a fraction to round up.
            let span_f = bin("*", span, PinExpr::Float(1.0));
            // ceil((end - start) / step), clamped at zero so a backwards or
            // empty range is no iterations rather than a negative Count.
            let raw = call("ceil", vec![bin("/", span_f, step.clone())]);
            let clamped = call("max", vec![PinExpr::Int(0), raw]);
            (clamped, PinExpr::User(start.clone()))
        }
    };

    if !r.reversed {
        return RangePins { count, first_index, step };
    }

    // `.rev()` starts at the last index the forward range would have produced
    // and walks back. Count is unchanged, and reached through `CountPin` so
    // the pin it is built from is computed once.
    let last = bin(
        "+",
        first_index,
        bin("*", bin("-", PinExpr::CountPin, PinExpr::Int(1)), step.clone()),
    );
    RangePins {
        count,
        first_index: last,
        step: bin("-", PinExpr::Int(0), step),
    }
}

fn bin(op: &'static str, lhs: PinExpr, rhs: PinExpr) -> PinExpr {
    PinExpr::Bin {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn call(name: &'static str, args: Vec<PinExpr>) -> PinExpr {
    PinExpr::Call { name, args }
}

/// Lower a `for` statement: a callable-loop microchip shell on the current
/// grid, whose plane holds the entry gate and the loop body.
///
/// The plane is the same machinery an anonymous `chip { }` uses: a
/// `NodeKind::Chip` node plus `chip_id`-tagged descendants that
/// `emit::partition::partition_anon_chips` moves into a child `Module`. Only
/// the `gate_class` and the entry gate inside make it a loop.
pub(super) fn lower_for(ctx: &mut LowerCtx, s: &ForStmt) {
    // The shell fires on an `Exec` pin, so a `for` with no chain to hang off
    // emits nothing. Typecheck reported WS074 for it.
    let Some(entry_exec) = ctx.current_exec else {
        return;
    };

    // `elem` carries the collection's element types (empty for a range).
    // `None` means typecheck already rejected the source (WS075) or its
    // record-typed element (WS082), so there is nothing left to lower.
    let Some((kind, mut elem)) = loop_kind(ctx, &s.source) else {
        return;
    };
    let spec = plane_spec(kind);
    // An array or map of enums or tuples is stored as one container per
    // column: the loop runs off one of them, and the body reads every column
    // at the element's index or key.
    let mut columns = None;

    // Drivers, on the outer grid. Lowered before the shell node exists so
    // that `ctx.current_exec` has already advanced past any `Var_Get` a
    // runtime bound needed by the time the shell's `Exec` is wired below.
    let drivers: Vec<(WirePort, PortRef)> = match &s.source {
        ForSource::Range(r) => {
            let pins = normalize_range(r);
            // Each bound the author wrote runs once, in source order; the
            // pins below are built from those values. Count is lowered first
            // and its value handed to the other two, because `.rev()` builds
            // `FirstIndex` out of it.
            let mut scope = PinScope::default();
            let mut written: Vec<&Expr> = match &r.extent {
                RangeExtent::Bounds { start, end } => vec![start, end],
                RangeExtent::Count { count, from } => std::iter::once(count).chain(from).collect(),
            };
            written.extend(&r.step);
            written.sort_by_key(|e| e.range().start.offset);
            for e in written {
                let stride = r.step.as_ref().is_some_and(|st| std::ptr::eq(st, e));
                lower_user(ctx, e, !stride, &mut scope);
            }
            let count = lower_pin_val(ctx, &pins.count, &s.range, &mut scope);
            let count_port = pin_port(ctx, count.clone(), &Type::Int, &s.range);
            scope.count = Some(count);
            vec![
                (WirePort::Count, count_port),
                (
                    WirePort::FirstIndex,
                    lower_pin(ctx, &pins.first_index, &Type::Int, &s.range, &mut scope),
                ),
                (
                    WirePort::Step,
                    lower_pin(ctx, &pins.step, &Type::Int, &s.range, &mut scope),
                ),
            ]
        }
        ForSource::Collection { expr, .. } => {
            // The shell takes a REFERENCE: resolved like any other container receiver.
            let source = ctx.container_kind(expr).and_then(|k| container_ref(ctx, expr, &k));
            let port = match source {
                Some(ContainerSource::Ref(p)) => Some(p),
                Some(ContainerSource::Columns(cols)) => {
                    let driver = match kind {
                        LoopKind::ForEachArray => crate::lower::access::leaf_arrays(&cols).into_iter().next(),
                        _ => crate::lower::access::leaf_maps(&cols).into_iter().next(),
                    };
                    columns = Some(cols);
                    driver.map(|v| {
                        elem = match (kind, &v.inner_type) {
                            (LoopKind::ForEachArray, t) => vec![t.clone()],
                            _ => {
                                let (k, val) = crate::lower::access::map_kv(&v);
                                vec![k, val]
                            }
                        };
                        let port = match kind {
                            LoopKind::ForEachArray => WirePort::ArrayVarRef,
                            _ => WirePort::MapVarRef,
                        };
                        v.node_id.port(port)
                    })
                }
                None => None,
            };
            let port = port.unwrap_or_else(|| lower_expr(ctx, expr));
            vec![(collection_driver_port(kind), port)]
        }
    };

    let shell = ctx.add_gate(AddNodeOpts {
        gate_class: spec.shell_class,
        source_range: s.range.clone(),
        ports: shell_io(kind, &elem),
        ..Default::default()
    });
    // A grid host, not a gate: this is what gives partition a child module to
    // move the body into.
    if let Some(n) = ctx.builder.module.nodes.get_mut(&shell) {
        n.kind = NodeKind::Chip;
    }
    for (port, src) in drivers {
        ctx.connect(src, shell.port(port));
    }
    let shell_exec_in = ctx.current_exec.unwrap_or(entry_exec);
    ctx.connect(shell_exec_in, shell.port(WirePort::Exec));

    // Every cached `Var_Get` is a gate on the outer grid that latched once
    // when the outer chain fired, so reusing one inside the body would hand
    // every iteration the pre-loop value of a var the body writes. No outer
    // read survives into the plane.
    let pre_loop_caches = snapshot_var_caches(ctx);
    reset_var_get_caches(ctx);
    let wires_before = ctx.builder.module.wires.len();
    let dyn_writes_before = ctx.dyn_ref_writes;

    // Enter the plane. Until the restore below, every node `ctx.add_gate`
    // creates is tagged `chip_id = shell` and so lands on the child grid.
    let saved_chip = ctx.current_anon_chip.replace(shell);
    ctx.push_scope(crate::scope::ScopeTag::BLOCK);

    // The entry gate is a pure source, placeable only inside a matching
    // plane. `Body` is the inner analogue of the outer `Exec`.
    let slot_tys = slot_types(kind, &elem);
    let entry = ctx.add_gate(AddNodeOpts {
        gate_class: spec.entry_class,
        source_range: s.range.clone(),
        ports: entry_io(spec, &slot_tys),
        ..Default::default()
    });

    // Slots resolve to ports BY NAME: the gate dump alphabetizes port arrays,
    // so slot order is a convention of this compiler and not something the
    // game attests to.
    for (pat, slot) in s.bindings.iter().zip(for_slots(kind, &s.bindings)) {
        match slot {
            ForSlot::RangeSlotNames => {
                if let ForPat::Record { fields, .. } = pat {
                    bind_range_slot_ports(ctx, fields, spec, entry);
                }
            }
            ForSlot::Slot(i) => {
                let port = entry.port(WirePort::from_name(spec.entry_slots[i]));
                bind_slot(ctx, pat, port);
            }
            // Over-binding is WS076 from typecheck; skip rather than panic.
            ForSlot::OverBound => {}
        }
    }

    // One body sweep per iteration, off `Body`. An iteration ends when its
    // chain runs dry, so there is no inner analogue of `Completed` and the
    // exec the body ends on is dropped.
    ctx.current_exec = Some(entry.port(WirePort::Body));
    // A columnar element binds as the record of its columns read at the
    // element's index (an array) or key (a map), at the top of each sweep.
    if let Some(cols) = &columns {
        for (pat, slot) in s.bindings.iter().zip(for_slots(kind, &s.bindings)) {
            let (ForPat::Name(name, _), ForSlot::Slot(i)) = (pat, slot) else {
                continue;
            };
            if spec.entry_slots[i] != "Value" {
                continue;
            }
            let fields = match kind {
                LoopKind::ForEachArray => {
                    let index = entry.port(WirePort::Index);
                    crate::lower::access::record_array_index_read(ctx, cols, index, &s.range)
                }
                _ => {
                    let key = entry.port(WirePort::Key);
                    crate::lower::access::record_map_key_read(ctx, cols, key, &s.range)
                }
            };
            ctx.scope.insert(name, Binding::Record(fields));
        }
    }
    ctx.loop_body_ranges.push(s.body.range.clone());
    lower_block(ctx, &s.body);
    ctx.loop_body_ranges.pop();

    ctx.pop_scope();
    ctx.current_anon_chip = saved_chip;

    // `as run` binds on the outer grid, in the scope the `for` sits in, so it
    // is readable after the loop.
    if let Some(name) = &s.result {
        bind_loop_result(ctx, name, shell);
    }

    // Post-loop cache state. `restore` drops the reads the body created (an
    // in-plane `Var_Get` never fires on the continuation) and puts the
    // pre-loop reads back; the invalidation then drops the pre-loop read of
    // any var the body touched.
    //
    // "Touched" comes from the wires the body added, because every read and
    // write of a var draws a wire out of its storage gate, including through
    // a nested plane or an inlined mod. A syntactic walk of assignment
    // targets would have to enumerate the write forms and would miss the
    // indirect ones. The cache diff `lower_if` uses cannot serve here: a
    // write to a var with no cached read leaves the cache `None` either way,
    // so it reports "unchanged" for the `t += 1` case this must catch.
    let mut touched: HashSet<NodeId> = HashSet::default();
    // `get(..)` rather than a slice index: lowering only appends wires, but a
    // shrink here would panic the compiler, and scanning nothing is no worse
    // than the pre-loop caches already are.
    for w in ctx
        .builder
        .module
        .wires
        .get(wires_before..)
        .unwrap_or_default()
    {
        if pre_loop_caches.contains_key(&w.source.node_id) {
            touched.insert(w.source.node_id);
        }
    }
    restore_var_caches(ctx, &pre_loop_caches);
    for id in &touched {
        invalidate_var_cache(ctx, id);
    }
    // A write through a runtime ref draws no wire out of the storage it hits,
    // so the wire scan above cannot see it: any such write in the body drops
    // every pre-loop read.
    if ctx.dyn_ref_writes != dyn_writes_before {
        reset_var_get_caches(ctx);
    }

    // The loop's continuation is the shell's `Completed`, on the outer grid.
    ctx.current_exec = Some(shell.port(WirePort::Completed));
}

/// Which loop kind a source is, plus the element types its slots take: empty
/// for a range, `[element]` for an array, `[key, value]` for a map. `None`
/// when the source is neither (WS075) or is record-typed (WS082).
fn loop_kind(ctx: &LowerCtx, src: &ForSource) -> Option<(LoopKind, Vec<Type>)> {
    match src {
        ForSource::Range(_) => Some((LoopKind::CountFor, Vec::new())),
        ForSource::Collection { expr, .. } => match ctx.container_kind(expr)? {
            ContainerKind::Array { elem } if !matches!(elem, Type::Record(_)) => {
                Some((LoopKind::ForEachArray, vec![elem]))
            }
            ContainerKind::Map { key, value }
                if !matches!(key, Type::Record(_)) && !matches!(value, Type::Record(_)) =>
            {
                Some((LoopKind::ForEachMap, vec![key, value]))
            }
            _ => None,
        },
    }
}

/// The shell's single ref-taking driver pin for a collection kind.
fn collection_driver_port(kind: LoopKind) -> WirePort {
    match kind {
        LoopKind::ForEachArray => WirePort::ArrayVarRef,
        LoopKind::ForEachMap => WirePort::MapVarRef,
        LoopKind::CountFor => unreachable!("a collection source is never CountFor"),
    }
}

/// The shell's declared ports: `Exec` plus this kind's driver inputs, and the
/// four status outputs every loop kind shares.
fn shell_io(kind: LoopKind, elem: &[Type]) -> GateIO {
    let spec = plane_spec(kind);
    let mut inputs = vec![PortSpec::exec(*sym::EXEC)];
    for name in spec.driver_ports {
        let port = WirePort::from_name(name);
        inputs.push(PortSpec::new(port.sym(), driver_port_type(port, elem)));
    }
    let outputs = SHELL_OUTPUTS
        .iter()
        .map(|name| {
            let port = WirePort::from_name(name);
            match port {
                WirePort::Completed => PortSpec::exec(port.sym()),
                WirePort::BAborted => PortSpec::new(port.sym(), Type::Bool),
                _ => PortSpec::new(port.sym(), Type::Int),
            }
        })
        .collect();
    GateIO { inputs, outputs }
}

/// `Count`/`FirstIndex`/`Step` are ints. The two collection drivers take a
/// reference typed from the element types, so the wire out of the
/// `Pseudo_ArrayVar`/`Pseudo_MapVar` matches the pin.
fn driver_port_type(port: WirePort, elem: &[Type]) -> Type {
    let at = |i: usize| elem.get(i).cloned().unwrap_or(Type::Any);
    match port {
        WirePort::ArrayVarRef => Type::Ref(Box::new(Type::Array(Box::new(at(0))))),
        WirePort::MapVarRef => Type::Ref(Box::new(Type::Map(Box::new(at(0)), Box::new(at(1))))),
        _ => Type::Int,
    }
}

/// The entry gate's declared ports: `Body` plus this kind's data slots. A
/// pure source, so it has no inputs at all.
fn entry_io(spec: &'static PlaneSpec, slot_tys: &[Type]) -> GateIO {
    let mut outputs = vec![PortSpec::exec(WirePort::Body.sym())];
    for (name, ty) in spec.entry_slots.iter().zip(slot_tys) {
        outputs.push(PortSpec::new(WirePort::from_name(name).sym(), ty.clone()));
    }
    GateIO {
        inputs: Vec::new(),
        outputs,
    }
}

/// Bind a `{ }` pattern that names a counted loop's own slots, as in
/// `for { index, isLast } in 0..n`.
///
/// The field name resolves against `spec.slot_labels` and the port then comes
/// from `spec.entry_slots` at that index, by name. Typecheck already reported
/// WS076 for an unknown field or a `...rest`, so an unresolved name binds
/// nothing here.
fn bind_range_slot_ports(
    ctx: &mut LowerCtx,
    fields: &[RecordDestructField],
    spec: &'static PlaneSpec,
    entry: NodeId,
) {
    for field in fields {
        let RecordDestructField::Named { name, alias, .. } = field else {
            continue;
        };
        let Some(idx) = spec.slot_labels.iter().position(|l| l == name) else {
            continue;
        };
        let port = entry.port(WirePort::from_name(spec.entry_slots[idx]));
        ctx.scope.insert(
            alias.as_ref().unwrap_or(name),
            Binding::Local(LocalRecord { port }),
        );
    }
}

/// Bind one slot pattern to the entry gate's port for that slot. `_` binds
/// nothing, leaving the port unwired.
fn bind_slot(ctx: &mut LowerCtx, pat: &ForPat, port: PortRef) {
    match pat {
        ForPat::Ignore(_) => {}
        ForPat::Name(name, _) => {
            ctx.scope.insert(name, Binding::Local(LocalRecord { port }));
        }
        // Only an array or map source reaches here (a range's `{ }` names
        // slots instead), where no slot is ever a record, so typecheck has
        // already refused this shape and owns the message.
        ForPat::Record { .. } => {}
    }
}

/// Bind `as run` to a record over the shell's status outputs, on the outer
/// grid. One port per field, so `run.aborted` resolves straight to the
/// shell's `bAborted` pin with no storage in between.
fn bind_loop_result(ctx: &mut LowerCtx, name: &str, shell: NodeId) {
    let fields: HashMap<crate::intern::Sym, Binding> = crate::ir::plane::result_fields()
        .into_iter()
        .map(|(field, port, _)| {
            (
                crate::intern::intern(field),
                Binding::Local(LocalRecord {
                    port: shell.port(WirePort::from_name(port)),
                }),
            )
        })
        .collect();
    ctx.scope.insert(name, Binding::Record(fields));
}

/// A lowered pin sub-expression: a value already known at compile time, or a
/// port to wire from.
#[derive(Clone)]
enum PinVal {
    Const(Literal),
    Port(PortRef),
}

/// Lower one shell driver pin and return the port to wire into it.
///
/// `want` is the pin's declared type, used only to narrow a folded constant:
/// the `Count` formula runs through `ceil`/`max`, both `float` builtins, so a
/// constant range folds to an integral `Literal::Float` that an int pin would
/// otherwise coerce at the wire.
fn lower_pin(
    ctx: &mut LowerCtx,
    e: &PinExpr,
    want: &Type,
    range: &SourceRange,
    scope: &mut PinScope,
) -> PortRef {
    let v = lower_pin_val(ctx, e, range, scope);
    pin_port(ctx, v, want, range)
}

/// What the driver pins of one loop share while they are lowered: the loop's
/// own `Count`, once it is, and each expression the author wrote, lowered once
/// however many pins its value feeds. A range's start feeds both `Count` and
/// `FirstIndex`, and its stride `Count` and `Step`, so lowering it per use
/// ran a call in it once per pin.
#[derive(Default)]
struct PinScope {
    count: Option<PinVal>,
    written: HashMap<(std::sync::Arc<str>, usize, usize), PinVal>,
}

/// The port a lowered pin value wires from, emitting a literal gate of the
/// pin's own type when the value is still constant.
fn pin_port(ctx: &mut LowerCtx, v: PinVal, want: &Type, range: &SourceRange) -> PortRef {
    match v {
        PinVal::Const(lit) => const_pin_port(ctx, lit, want, range),
        PinVal::Port(p) => p,
    }
}

/// A constant pin, as a literal gate of the pin's own type. `ceil` never
/// yields a fraction, so narrowing an integral `Float` to `Int` is lossless.
fn const_pin_port(ctx: &mut LowerCtx, lit: Literal, want: &Type, range: &SourceRange) -> PortRef {
    let (ty, lit) = match (want, lit) {
        (Type::Int, Literal::Float(f)) if f.is_finite() && f.fract() == 0.0 => {
            (Type::Int, Literal::Int(f as i64))
        }
        (_, Literal::Int(n)) => (Type::Int, Literal::Int(n)),
        (_, Literal::Float(f)) => (Type::Float, Literal::Float(f)),
        (_, other) => (want.clone(), other),
    };
    literal_node_range(ctx, range, ty, lit)
}

/// Lower a `PinExpr`, folding bottom-up so a constant sub-tree costs no gate.
///
/// Only [`PinExpr::User`] goes through `lower_expr`. A synthesized `Bin`
/// resolves its gate from the operands' actual lowered port types, the way
/// `lower::ops::mono_op_rule` does for a generic monomorph, which is what
/// makes the `* 1.0` select the `(Float, Int)` divide rather than the
/// truncating `(Int, Int)` one.
fn lower_pin_val(
    ctx: &mut LowerCtx,
    e: &PinExpr,
    range: &SourceRange,
    scope: &mut PinScope,
) -> PinVal {
    match e {
        PinExpr::Int(n) => PinVal::Const(Literal::Int(*n)),
        PinExpr::Float(f) => PinVal::Const(Literal::Float(*f)),
        // `normalize_range` builds this only into `FirstIndex`, which is
        // lowered after `Count` and so always has the value to hand.
        PinExpr::CountPin => scope.count.clone().unwrap_or(PinVal::Const(Literal::Int(0))),
        PinExpr::User(expr) => lower_user(ctx, expr, true, scope),
        PinExpr::Bin { op, lhs, rhs } => {
            let l = lower_pin_val(ctx, lhs, range, scope);
            let r = lower_pin_val(ctx, rhs, range, scope);
            if let (PinVal::Const(a), PinVal::Const(b)) = (&l, &r)
                && let Some(lit) = eval_const_binop(op, a.clone(), b.clone())
            {
                return PinVal::Const(lit);
            }
            let lp = materialize(ctx, l, range);
            let rp = materialize(ctx, r, range);
            PinVal::Port(build_binop(ctx, op, lp, rp, range))
        }
        PinExpr::Call { name, args } => {
            let Some(spec) = find_call(name) else {
                // Unreachable in practice: `normalize_range` synthesizes only
                // `ceil` and `max`, both bundled builtins.
                ctx.error(
                    "WS007",
                    format!("no builtin `{name}` to compute this loop's bounds with"),
                    range,
                );
                return PinVal::Const(Literal::Int(0));
            };
            let vals: Vec<PinVal> = args
                .iter()
                .map(|a| lower_pin_val(ctx, a, range, scope))
                .collect();
            if let Some(lit) = fold_builtin(spec, &vals) {
                return PinVal::Const(lit);
            }
            let ports: Vec<PortRef> = vals
                .into_iter()
                .map(|v| materialize(ctx, v, range))
                .collect();
            PinVal::Port(build_builtin(ctx, spec, &ports, range))
        }
    }
}

/// A bound or stride the author wrote, lowered the first time a pin asks for
/// it and shared by every later one. A bound is an `int` place (`int_place`);
/// a stride keeps a float value.
fn lower_user(ctx: &mut LowerCtx, expr: &Expr, int_place: bool, scope: &mut PinScope) -> PinVal {
    let r = expr.range();
    let key = (r.file.clone(), r.start.offset, r.end.offset);
    if let Some(v) = scope.written.get(&key) {
        return v.clone();
    }
    let v = match const_user_expr(ctx, expr, int_place) {
        Some(lit) => PinVal::Const(lit),
        None => PinVal::Port(lower_expr(ctx, expr)),
    };
    scope.written.insert(key, v.clone());
    v
}

/// Give a `PinVal` a port, emitting a literal gate when it is still constant.
fn materialize(ctx: &mut LowerCtx, v: PinVal, range: &SourceRange) -> PortRef {
    match v {
        PinVal::Port(p) => p,
        PinVal::Const(lit) => {
            let ty = match &lit {
                Literal::Int(_) => Type::Int,
                Literal::Float(_) => Type::Float,
                Literal::Bool(_) => Type::Bool,
                _ => Type::Any,
            };
            literal_node_range(ctx, range, ty, lit)
        }
    }
}

/// The value of a user-written bound or stride when it is a compile-time
/// constant, read through the ordinary lowering constant environment. In an
/// `int` place (a bound) a float constant is the integer it truncates to.
fn const_user_expr(ctx: &LowerCtx, e: &Expr, int_place: bool) -> Option<Literal> {
    let lookup = |n: &str| ctx.resolve_mod(n);
    let cx = ctx.const_ctx(Some(&lookup));
    let mut budget = crate::const_eval::Budget::default();
    let lit = crate::const_eval::eval_expr(e, &cx, &mut budget).ok()?;
    if !int_place {
        return Some(lit);
    }
    Some(crate::const_eval::coerce_const_to_type(lit, &Type::Int, ctx.consts.types()))
}

/// Build the Math gate for a synthesized binary operator.
///
/// The rule comes from the operands' actual lowered port types. These nodes
/// were never in the source text, so `op_resolutions` has no entry for them
/// and `lower_binop` would substitute an `_Unsupported` placeholder.
fn build_binop(
    ctx: &mut LowerCtx,
    op: &str,
    lhs: PortRef,
    rhs: PortRef,
    range: &SourceRange,
) -> PortRef {
    let tys = [
        super::call::arg_port_type(ctx, lhs).unwrap_or(Type::Any),
        super::call::arg_port_type(ctx, rhs).unwrap_or(Type::Any),
    ];
    let Some(rule) = crate::catalog::operators::resolve_op(op, &tys) else {
        ctx.error(
            "WS007",
            format!(
                "no `{op}` gate takes ({:?}, {:?}), so this loop's bounds cannot be computed",
                tys[0], tys[1]
            ),
            range,
        );
        return lhs;
    };
    let in_a = rule.ports.inputs[0];
    let in_b = rule.ports.inputs[1];
    let out = rule.ports.output;
    let node = ctx.add_gate(AddNodeOpts {
        gate_class: rule.gate_class,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::new(in_a.sym(), rule.operands[0].clone()),
                PortSpec::new(in_b.sym(), rule.operands[1].clone()),
            ],
            outputs: vec![PortSpec::new(out.sym(), rule.result.clone())],
        },
        ..Default::default()
    });
    ctx.connect(lhs, node.port(in_a));
    ctx.connect(rhs, node.port(in_b));
    node.port(out)
}

/// Constant-fold a synthesized builtin call through the certified per-gate
/// law, or `None` when any argument is a runtime value.
///
/// Arguments are widened to their declared parameter type first. Every
/// `ceil`/`max` parameter is `float`, and the fold table has no entry for an
/// int-typed wire into those gate classes because the compiler never wires
/// one: an int argument to a float port widens at the wire.
fn fold_builtin(spec: &crate::catalog::calls::CallSpec, vals: &[PinVal]) -> Option<Literal> {
    use super::fold::eval::Value;
    let mut inputs: Vec<Option<Value>> = Vec::with_capacity(vals.len());
    for (v, param) in vals.iter().zip(&spec.params) {
        let PinVal::Const(lit) = v else { return None };
        let lit = match (lit, &param.ty) {
            (Literal::Int(n), Type::Float) => Literal::Float(*n as f64),
            (other, _) => other.clone(),
        };
        inputs.push(Value::from_literal(&lit));
    }
    Some(super::fold::eval::eval(spec.gate_class, &inputs)?.to_literal())
}

/// Build the gate for a synthesized builtin call, wiring each argument into
/// its declared parameter port.
fn build_builtin(
    ctx: &mut LowerCtx,
    spec: &crate::catalog::calls::CallSpec,
    args: &[PortRef],
    range: &SourceRange,
) -> PortRef {
    let node = ctx.add_gate(AddNodeOpts {
        gate_class: spec.gate_class,
        source_range: range.clone(),
        ports: GateIO {
            inputs: spec
                .params
                .iter()
                .map(|p| PortSpec::new(p.port.sym(), p.ty.clone()))
                .collect(),
            outputs: spec
                .outputs
                .iter()
                .map(|o| PortSpec::new(o.port.sym(), o.ty.clone()))
                .collect(),
        },
        ..Default::default()
    });
    for (arg, param) in args.iter().zip(&spec.params) {
        ctx.connect(*arg, node.port(param.port));
    }
    node.port(spec.outputs[0].port)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Pos, SourceRange};

    fn range() -> SourceRange {
        SourceRange::new("test.ws", Pos::default(), Pos::default())
    }

    fn int_lit(value: i64) -> Expr {
        Expr::IntLit {
            value,
            text: value.to_string(),
            range: range(),
        }
    }

    fn bounds(start: i64, end: i64) -> RangeSource {
        RangeSource {
            extent: RangeExtent::Bounds {
                start: int_lit(start),
                end: int_lit(end),
            },
            step: None,
            reversed: false,
            range: range(),
        }
    }

    /// Evaluate a user-written leaf, which in these hand-built fixtures is
    /// always a literal. `BinOp` is handled so a fixture may compose one.
    fn eval_user_expr(e: &Expr) -> crate::ir::Literal {
        use crate::ir::Literal;
        match e {
            Expr::IntLit { value, .. } => Literal::Int(*value),
            Expr::FloatLit { value, .. } => Literal::Float(*value),
            Expr::BinOp { op, left, right, .. } => {
                crate::lower::eval_const_binop(op, eval_user_expr(left), eval_user_expr(right))
                    .unwrap_or_else(|| panic!("operator `{op}` refused to fold its operands"))
            }
            other => panic!("unexpected expr shape in a hand-built RangeSource fixture: {other:?}"),
        }
    }

    /// Evaluate one of the three pin expressions `normalize_range` produced,
    /// down to the numeric value it represents.
    ///
    /// Deliberately not `const_eval::eval_expr`: that evaluator reaches a
    /// catalog builtin only through `eval_method_call`, gated on
    /// `spec.receiver.is_some()`, and `ceil`/`max` have no receiver form, so
    /// a plain-name call falls through to `NotAConstMod`. This walks
    /// `PinExpr` directly and evaluates `Bin`/`Call` through
    /// `lower::fold::eval::eval`, the certified per-gate law `const_eval`'s
    /// own paths are built on.
    ///
    /// Returns a `Literal` rather than an `i64`: `ceil`/`max` are
    /// `Type::Float` builtins, so a `Bounds` range's `Count` is a
    /// `Literal::Float` here even though the shell's pin is `int`. `pin`
    /// below accepts either.
    fn eval_pin(e: &PinExpr, count: Option<&crate::ir::Literal>) -> crate::ir::Literal {
        use crate::ir::{Literal, Type};
        use crate::lower::fold::eval::Value;
        match e {
            PinExpr::User(expr) => eval_user_expr(expr),
            PinExpr::Int(n) => Literal::Int(*n),
            PinExpr::Float(f) => Literal::Float(*f),
            PinExpr::CountPin => count
                .cloned()
                .unwrap_or_else(|| panic!("`CountPin` evaluated with no Count to hand")),
            PinExpr::Bin { op, lhs, rhs } => {
                crate::lower::eval_const_binop(op, eval_pin(lhs, count), eval_pin(rhs, count))
                    .unwrap_or_else(|| panic!("operator `{op}` refused to fold its operands"))
            }
            PinExpr::Call { name, args } => {
                let spec = crate::catalog::calls::find_call(name)
                    .unwrap_or_else(|| panic!("no catalog entry for builtin `{name}`"));
                let inputs: Vec<Option<Value>> = args
                    .iter()
                    .zip(&spec.params)
                    .map(|(a, param)| {
                        // Bind like an ordinary call argument: an int value
                        // wired into a `Type::Float` param widens before it
                        // reaches the gate. The fold table has no entry for
                        // an int-typed input to this gate class, so a raw
                        // `Value::Int` gets a `None` refusal instead of the
                        // certified law.
                        let lit = match (eval_pin(a, count), &param.ty) {
                            (Literal::Int(n), Type::Float) => Literal::Float(n as f64),
                            (lit, _) => lit,
                        };
                        Value::from_literal(&lit)
                    })
                    .collect();
                crate::lower::fold::eval::eval(spec.gate_class, &inputs)
                    .unwrap_or_else(|| panic!("`{name}` refused to fold these operands"))
                    .to_literal()
            }
        }
    }

    fn pin(src_range: &RangeSource, which: fn(&RangePins) -> &PinExpr) -> i64 {
        let pins = normalize_range(src_range);
        // Count carries no `CountPin` of its own, so it evaluates first and
        // stands in for the one `.rev()`'s FirstIndex reaches for.
        let count = eval_pin(&pins.count, None);
        match eval_pin(which(&pins), Some(&count)) {
            crate::ir::Literal::Int(n) => n,
            crate::ir::Literal::Float(f) => f as i64,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn zero_to_n_is_count_n_from_zero_step_one() {
        let r = bounds(0, 10);
        assert_eq!(pin(&r, |p| &p.count), 10);
        assert_eq!(pin(&r, |p| &p.first_index), 0);
        assert_eq!(pin(&r, |p| &p.step), 1);
    }

    #[test]
    fn bounds_exclude_the_end() {
        let r = bounds(3, 7);
        assert_eq!(pin(&r, |p| &p.count), 4);
        assert_eq!(pin(&r, |p| &p.first_index), 3);
    }

    #[test]
    fn step_is_a_ceil_divide() {
        // 0..10 by 3 yields 0, 3, 6, 9: four iterations, not three.
        let mut r = bounds(0, 10);
        r.step = Some(int_lit(3));
        assert_eq!(pin(&r, |p| &p.count), 4);
        assert_eq!(pin(&r, |p| &p.step), 3);
    }

    #[test]
    fn an_empty_range_is_zero_iterations() {
        let r = bounds(5, 5);
        assert_eq!(pin(&r, |p| &p.count), 0);
    }

    #[test]
    fn a_backwards_range_is_zero_not_negative() {
        let r = bounds(10, 0);
        assert_eq!(pin(&r, |p| &p.count), 0);
    }

    #[test]
    fn rev_counts_down_from_the_top() {
        // (0..5).rev() yields 4, 3, 2, 1, 0.
        let mut r = bounds(0, 5);
        r.reversed = true;
        assert_eq!(pin(&r, |p| &p.count), 5);
        assert_eq!(pin(&r, |p| &p.first_index), 4);
        assert_eq!(pin(&r, |p| &p.step), -1);
    }

    #[test]
    fn count_spelling_drives_the_pins_directly() {
        let r = RangeSource {
            extent: RangeExtent::Count {
                count: int_lit(4),
                from: Some(int_lit(10)),
            },
            step: Some(int_lit(5)),
            reversed: false,
            range: range(),
        };
        assert_eq!(pin(&r, |p| &p.count), 4);
        assert_eq!(pin(&r, |p| &p.first_index), 10);
        assert_eq!(pin(&r, |p| &p.step), 5);
    }
}
