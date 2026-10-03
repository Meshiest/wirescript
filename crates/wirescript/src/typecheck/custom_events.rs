//! Custom-event slot keys, sender/receiver harvesting, and the WS030/WS042
//! consistency and slot-inference passes.

use super::*;
use crate::types::mono::unwrap_ref;

/// Which custom-event namespace a receiver belongs to. Personal (`CustomEvent` /
/// `SendCustomEvent`, same-owner delivery) and Global (`GlobalCustomEvent` /
/// `SendGlobalCustomEvent`, ownership-agnostic) are DISTINCT channels: a send in
/// one namespace never resolves a receiver in the other. It is carried in the
/// key so the namespace is explicit in the map, not inferred from context.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CeNamespace {
    Personal,
    Global,
}

impl CeNamespace {
    /// The namespace for an event surface name, or `None` if it is not a custom
    /// event (`"CustomEvent"` → Personal, `"GlobalCustomEvent"` → Global).
    pub fn from_event_name(name: &str) -> Option<Self> {
        match name {
            "CustomEvent" => Some(CeNamespace::Personal),
            "GlobalCustomEvent" => Some(CeNamespace::Global),
            _ => None,
        }
    }
}

/// Key for a custom-event receiver's data slot: `(namespace, file, start.offset,
/// end.offset)` — the namespace plus the handler's source range. The range shape
/// mirrors `tmap`/`type_of_expr` because `SourceRange`/`Pos` do not derive `Hash`;
/// the namespace keeps personal and global receivers in disjoint key spaces.
pub type CeSlotKey = (CeNamespace, Arc<str>, usize, usize);

/// Resolved types for custom-event receivers' data slots, keyed by `CeSlotKey`.
/// `None` slot = declared (nothing to override); `Some(t)` = unannotated, resolve
/// binding to `t`; `Some(Float)` = inference fallback.
pub type CeSlotMap = HashMap<CeSlotKey, Vec<Option<Type>>>;

/// Build the `CeSlotKey` for a custom-event receiver handler `h` in namespace `ns`.
/// Public so lowering builds the identical key (namespace + range) when it reads
/// resolved slot types back out of the `CeSlotMap`.
pub fn ce_slot_key(ns: CeNamespace, h: &Handler) -> CeSlotKey {
    (ns, h.range.file.clone(), h.range.start.offset, h.range.end.offset)
}

// ---------- custom-event sender/receiver type consistency (WS030) ----------

/// The wire-variant class a custom-event value takes on the wire. Two types in
/// the same class transfer identically (a `character` and an `entity` are both
/// the `Object` variant), so only a cross-class disagreement is a real
/// mismatch. Mirrors `emit::var_type_to_wire_variant`'s grouping. `None` means
/// "unclassifiable" (`any`/`exec`/records/…) — never linted, either side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WireClass {
    Bool,
    Int,
    Number,
    Str,
    Object,
    Vector,
    Rotator,
    Quat,
    Color,
    VarRef,
    ArrayRef,
    MapRef,
    Brick,
}

fn wire_class(t: &Type) -> Option<WireClass> {
    if let Some(k) = t.ref_kind() {
        return Some(match k {
            crate::ir::RefKind::Var => WireClass::VarRef,
            crate::ir::RefKind::Array => WireClass::ArrayRef,
            crate::ir::RefKind::Map => WireClass::MapRef,
            crate::ir::RefKind::Brick => WireClass::Brick,
        });
    }
    Some(match t {
        Type::Bool => WireClass::Bool,
        Type::Int => WireClass::Int,
        Type::Float => WireClass::Number,
        Type::String => WireClass::Str,
        Type::Vector => WireClass::Vector,
        Type::Rotator => WireClass::Rotator,
        Type::Quat => WireClass::Quat,
        Type::Color => WireClass::Color,
        Type::Controller | Type::Character | Type::Entity => {
            WireClass::Object
        }
        _ => return None,
    })
}

/// `(channel name, per-slot declared type)` for a handler that is a custom-event
/// receiver of kind `event_name` (`"CustomEvent"` or `"GlobalCustomEvent"`) with
/// a literal channel name. `None` for any other handler (or a dynamic channel
/// name — nothing to key receivers by). An unannotated slot falls back to
/// `ce_slots`' inferred type for this handler (keyed by its source range) when
/// present — pass 1 has no inference available yet, so it passes an empty map.
/// `declared` holds each param's annotation as its declaring file reads it.
fn ce_receiver_of(
    h: &Handler,
    event_name: &str,
    ce_slots: &CeSlotMap,
    declared: &[Option<Type>],
) -> Option<(String, Vec<Option<RecvSlot>>)> {
    if !matches!(&h.trigger, Trigger::Ident { name, .. } if name == event_name) {
        return None;
    }
    let name = h.config.iter().find_map(|c| match c {
        HandlerConfigArg::Positional(Expr::StringLit { value, .. }) => Some(value.clone()),
        _ => None,
    })?;
    let ns = CeNamespace::from_event_name(event_name)?;
    let inferred = ce_slots.get(&ce_slot_key(ns, h));
    let params = h
        .params
        .iter()
        .enumerate()
        .map(|(i, _)| match declared.get(i).cloned().flatten() {
            Some(ty) => Some(RecvSlot { ty, declared: true, capture: false }),
            // Fill from inference when available (pass 2); else unresolved.
            None => inferred
                .and_then(|v| v.get(i).cloned().flatten())
                .map(|ty| RecvSlot { ty, declared: false, capture: false }),
        })
        .collect();
    Some((name, params))
}

/// A receiver's data param type: the one it declares, or the one inferred
/// from the first send on its channel.
#[derive(Clone)]
struct RecvSlot {
    ty: Type,
    declared: bool,
    /// A `let v: T = await CustomEvent("c")` capture rather than an `on`
    /// handler's parameter.
    capture: bool,
}

/// Fold a receiver's params into the per-channel signature: each slot keeps
/// every classifiable type a receiver of the channel takes there, so a send
/// is checked against each receiver, not only the first.
fn ce_merge_receiver(map: &mut HashMap<String, Vec<Vec<RecvSlot>>>, name: String, params: Vec<Option<RecvSlot>>) {
    let slots = map.entry(name).or_default();
    for (i, ty) in params.into_iter().enumerate() {
        if slots.len() <= i {
            slots.resize(i + 1, Vec::new());
        }
        if let Some(t) = ty
            && let Some(class) = wire_class(&t.ty)
            && !slots[i]
                .iter()
                .any(|s| wire_class(&s.ty) == Some(class) && s.declared == t.declared && s.capture == t.capture)
        {
            slots[i].push(t);
        }
    }
}

/// The literal channel name a `SendCustomEvent` call targets, or `None` when it
/// is passed dynamically (a variable/interpolation) — dynamic sends can reach
/// any receiver, so they are never linted.
fn ce_send_event_name(args: &[CallArg]) -> Option<String> {
    // A named `eventName = …` is the channel if present; otherwise the first
    // positional arg is.
    for a in args {
        if let CallArg::Named { name, value, .. } = a
            && name == "eventName"
        {
            return match value {
                Expr::StringLit { value, .. } => Some(value.clone()),
                _ => None,
            };
        }
    }
    match args.iter().find(|a| matches!(a, CallArg::Positional(_))) {
        Some(CallArg::Positional(Expr::StringLit { value, .. })) => Some(value.clone()),
        _ => None,
    }
}

/// Map a `SendCustomEvent` call's data args to `(0-based slot, value expr)`.
/// Positional arg 0 is the channel name (unless it was named), so positional
/// data starts at slot 0 from the *second* positional; `dataN` names slot N-1.
fn ce_send_data_args(args: &[CallArg]) -> Vec<(usize, &Expr)> {
    let name_is_named = args
        .iter()
        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "eventName"));
    let mut out = Vec::new();
    let mut pos = 0usize;
    for a in args {
        match a {
            CallArg::Positional(e) => {
                let slot = if name_is_named {
                    Some(pos)
                } else if pos == 0 {
                    None // the channel name
                } else {
                    Some(pos - 1)
                };
                if let Some(s) = slot {
                    out.push((s, e));
                }
                pos += 1;
            }
            CallArg::Named { name, value, .. } => {
                if let Some(n) = name
                    .strip_prefix("data")
                    .and_then(|d| d.parse::<usize>().ok())
                    && n >= 1
                {
                    out.push((n - 1, value));
                }
            }
            CallArg::Spread(_) => {}
        }
    }
    out
}

pub(super) fn check_custom_event_types(
    ctx: &mut TypeCheckCtx,
    script: &Script,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
    ce_slots: &CeSlotMap,
) {
    // Personal and Global custom events are SEPARATE channel namespaces: a
    // `SendCustomEvent("x")` only reaches `on CustomEvent("x")`, and a
    // `SendGlobalCustomEvent("x")` only reaches `on GlobalCustomEvent("x")`. Gather
    // each namespace's receivers and senders in one AST walk, then type-check the
    // send/receive pairs within each namespace independently.
    let mut personal_recv: HashMap<String, Vec<Vec<RecvSlot>>> = HashMap::default();
    let mut global_recv: HashMap<String, Vec<Vec<RecvSlot>>> = HashMap::default();
    let mut personal_send: Vec<&Expr> = Vec::new();
    let mut global_send: Vec<&Expr> = Vec::new();
    // A data slot is ONE wire. A record (or an enum, which lowers as a record of
    // `__disc` + slots) spans several, so it cannot travel through one. Lowering
    // binds such a param to a single `Binding::EventParam` and every use of it
    // produces no gates, dropping the statement without a word, so name it here
    // instead.
    let mut bad_params: Vec<(String, String, crate::diagnostic::SourceRange)> = Vec::new();
    crate::analysis::visit_program(
        script,
        &mut |h| {
            let mut declared: Vec<Option<Type>> = Vec::new();
            if matches!(&h.trigger, Trigger::Ident { name, .. }
                if name == "CustomEvent" || name == "GlobalCustomEvent")
            {
                declared = h
                    .params
                    .iter()
                    .map(|p| p.ty.as_ref().map(|te| crate::typecheck::resolve::resolve_type_expr(ctx, te)))
                    .collect();
                for (p, t) in h.params.iter().zip(&declared) {
                    let Some(t) = t else { continue };
                    if matches!(unwrap_ref(t), Type::Record(_) | Type::Enum { .. }) {
                        bad_params.push((p.name.clone(), t.to_string(), p.range.clone()));
                    }
                }
            }
            if let Some((name, params)) = ce_receiver_of(h, "CustomEvent", ce_slots, &declared) {
                ce_merge_receiver(&mut personal_recv, name, params);
            } else if let Some((name, params)) = ce_receiver_of(h, "GlobalCustomEvent", ce_slots, &declared) {
                ce_merge_receiver(&mut global_recv, name, params);
            }
        },
        &mut |call| {
            if let Expr::Call { callee, .. } = call {
                // The channel name + data args live in `call`'s positional args in
                // both the plain `SendCustomEvent("x", …)` and the receiver form
                // `entity.SendCustomEvent("x", …)` (the receiver is separate), so
                // both forms are collected the same way.
                let callee_name = match callee.as_ref() {
                    Expr::Ident { name, .. } => Some(name.as_str()),
                    Expr::FieldAccess { field, .. } => Some(field.as_str()),
                    _ => None,
                };
                match callee_name {
                    Some("SendCustomEvent") => personal_send.push(call),
                    Some("SendGlobalCustomEvent") => global_send.push(call),
                    _ => {}
                }
            }
        },
    );
    for (pname, tname, range) in bad_params {
        ctx.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "WS068".into(),
            message: format!(
                "custom event data param '{pname}' is {tname}, which spans several wires and \
                 cannot travel through a single event data slot. Pass the fields as separate \
                 params (`-> (a: int, b: int)`), or send a key and read the value from shared \
                 storage on the other side"
            ),
            range,
        });
    }
    // A typed `let v: T = await CustomEvent("c")` receives the event's first
    // data output as `T`, as a handler's first parameter does.
    let mut awaits: Vec<&AwaitStmt> = Vec::new();
    crate::ast::walk::script_children(script, &mut |n, _| {
        if let crate::ast::walk::Node::Stmt(Stmt::Await(a)) = n {
            awaits.push(a);
        }
        crate::ast::walk::walk(n, Default::default(), &mut |m, _| {
            if let crate::ast::walk::Node::Stmt(Stmt::Await(a)) = m {
                awaits.push(a);
            }
            true
        });
    });
    for a in awaits {
        let (None, Some(te), Expr::Call { callee, args, .. }) = (&a.value_expr, &a.binding_type, &a.exec_expr) else {
            continue;
        };
        let Expr::Ident { name: event, .. } = callee.as_ref() else { continue };
        let Some(chan) = args.iter().find_map(|arg| match arg {
            CallArg::Positional(Expr::StringLit { value, .. }) => Some(value.clone()),
            _ => None,
        }) else {
            continue;
        };
        // Reported where the capture is checked already.
        let mark = ctx.diagnostics.len();
        let ty = crate::typecheck::resolve::resolve_type_expr(ctx, te);
        ctx.diagnostics.truncate(mark);
        let slot = vec![Some(RecvSlot { ty, declared: true, capture: true })];
        match event.as_str() {
            "CustomEvent" => ce_merge_receiver(&mut personal_recv, chan, slot),
            "GlobalCustomEvent" => ce_merge_receiver(&mut global_recv, chan, slot),
            _ => {}
        }
    }
    check_ce_namespace(ctx, &personal_send, &personal_recv, tmap, "SendCustomEvent", "CustomEvent");
    check_ce_namespace(ctx, &global_send, &global_recv, tmap, "SendGlobalCustomEvent", "GlobalCustomEvent");
}

/// The type a custom-event send's data value `e` sends: a gate read sends
/// its first output.
fn sent_type(tmap: &HashMap<(Arc<str>, usize, usize), Type>, e: &Expr) -> Option<Type> {
    let r = e.range();
    tmap.get(&(r.file.clone(), r.start.offset, r.end.offset)).map(super::infer::first_output)
}

/// Within ONE custom-event namespace, warn (WS030) when a send's data value type
/// disagrees with a receiver's param type for the same channel: the game keys
/// the wire variant off the sender, so a mismatch is a real bug. A constant is
/// sent as the type it has, so a number constant of the other number type is
/// a mismatch too. Each receiver of the channel is checked.
fn check_ce_namespace(
    ctx: &mut TypeCheckCtx,
    senders: &[&Expr],
    receivers: &HashMap<String, Vec<Vec<RecvSlot>>>,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
    send_name: &str,
    event_name: &str,
) {
    for call in senders {
        let Expr::Call { args, .. } = call else {
            continue;
        };
        let Some(name) = ce_send_event_name(args) else {
            continue; // dynamic channel name — not linted
        };
        // A data arg with NO wire class — `any`, or an `Opaque(...)` that erased
        // its input's type — leaves the send port untyped, so it emits the float
        // variant and cannot match a receiver that declares a real type. Report it
        // whether or not a receiver is visible here: the receiver is usually in
        // another file (a spawned chip), which is exactly where WS030 below is
        // blind, and where the mismatch is fatal rather than merely wrong.
        for (slot, expr) in ce_send_data_args(args) {
            let r = expr.range();
            let Some(t) = sent_type(tmap, expr) else {
                continue;
            };
            if wire_class(&t).is_none() {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS045".into(),
                    message: format!(
                        "custom event '{name}' data #{} has no concrete type (it is {t}), so the \
                         send emits the float variant and will not match a receiver that declares \
                         int/bool/string/entity/…. Give the value a typed binding (e.g. a `var` of \
                         that type) rather than `any` or `Opaque(...)`, which erases it",
                        slot + 1,
                    ),
                    range: r.clone(),
                });
            }
        }

        let Some(slots) = receivers.get(&name) else {
            continue; // no in-unit receiver to compare against
        };
        for (slot, expr) in ce_send_data_args(args) {
            let Some(recvs) = slots.get(slot) else {
                continue;
            };
            let r = expr.range();
            let Some(send_ty) = sent_type(tmap, expr) else {
                continue;
            };
            let Some(send_class) = wire_class(&send_ty) else {
                continue;
            };
            for recv in recvs {
                let recv_ty = &recv.ty;
                let Some(recv_class) = wire_class(recv_ty) else {
                    continue;
                };
                if send_class != recv_class {
                    let message = if recv.capture {
                        format!(
                            "custom event '{name}' data #{} is {send_ty}, but a `let v: {recv_ty} = await \
                             {event_name}(\"{name}\")` capture takes {recv_ty}; {send_name} values must match \
                             the type the receiver takes",
                            slot + 1,
                        )
                    } else if recv.declared {
                        format!(
                            "custom event '{name}' data #{} is {send_ty}, but the `on {event_name}(\"{name}\", ...)` \
                             receiver declares {recv_ty}; {send_name} values must match the receiver's param types",
                            slot + 1,
                        )
                    } else {
                        let first = first_send_of(senders, &name, slot, recv_class, tmap)
                            .map(|f| {
                                if f.file == r.file {
                                    format!(" (line {})", f.start.line)
                                } else {
                                    let file = f.file.rsplit(['/', '\\']).next().unwrap_or(&f.file);
                                    format!(" ({file} line {})", f.start.line)
                                }
                            })
                            .unwrap_or_default();
                        format!(
                            "custom event '{name}' data #{} is {send_ty}, but the `on {event_name}(\"{name}\", ...)` \
                             receiver takes {recv_ty} from the first send on this channel{first}; every \
                             {send_name} on one channel must send the same type, or annotate the receiver's param",
                            slot + 1,
                        )
                    };
                    ctx.diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        code: "WS030".into(),
                        message,
                        range: r.clone(),
                    });
                }
            }
        }
    }
}

/// The data argument of the first send on channel `name` whose slot `slot`
/// has wire class `class`: the send an unannotated receiver took its type
/// from (`infer_custom_event_slots` keeps the first in the same order).
fn first_send_of<'a>(
    senders: &[&'a Expr],
    name: &str,
    slot: usize,
    class: WireClass,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
) -> Option<&'a crate::diagnostic::SourceRange> {
    senders.iter().find_map(|call| {
        let Expr::Call { args, .. } = call else { return None };
        if ce_send_event_name(args).as_deref() != Some(name) {
            return None;
        }
        let (_, expr) = ce_send_data_args(args).into_iter().find(|(i, _)| *i == slot)?;
        let t = sent_type(tmap, expr)?;
        (wire_class(&t)? == class).then_some(expr.range())
    })
}

/// Resolve every UNANNOTATED custom-event receiver slot to a concrete type from
/// the matching in-unit sender, or `float` (with a WS042 warning) when none is
/// inferable. Returns the map (keyed by handler range) plus the WS042
/// diagnostics. Reads sender arg types from `tmap` (the pass-1 `type_of_expr`).
///
/// Handles both namespaces (`CustomEvent`/`SendCustomEvent` and
/// `GlobalCustomEvent`/`SendGlobalCustomEvent`), keeping them separate exactly
/// as `check_custom_event_types` does — a personal send must never resolve a
/// global receiver's slot, or vice versa.
pub(super) fn infer_custom_event_slots(
    ast: &Script,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
) -> (CeSlotMap, Vec<Diagnostic>) {
    // 1. Collect senders → channel → slot → first concrete wire-typed value.
    //    (personal vs global namespaces, mirroring check_custom_event_types.)
    let mut personal: HashMap<String, HashMap<usize, Type>> = HashMap::default();
    let mut global: HashMap<String, HashMap<usize, Type>> = HashMap::default();
    let mut receivers: Vec<(&Handler, &'static str)> = Vec::new();
    crate::analysis::visit_program(
        ast,
        &mut |h| {
            if matches!(&h.trigger, Trigger::Ident { name, .. } if name == "CustomEvent") {
                receivers.push((h, "CustomEvent"));
            } else if matches!(&h.trigger, Trigger::Ident { name, .. } if name == "GlobalCustomEvent")
            {
                receivers.push((h, "GlobalCustomEvent"));
            }
        },
        &mut |call| {
            if let Expr::Call { callee, args, .. } = call {
                let callee_name = match callee.as_ref() {
                    Expr::Ident { name, .. } => Some(name.as_str()),
                    Expr::FieldAccess { field, .. } => Some(field.as_str()),
                    _ => None,
                };
                let bucket = match callee_name {
                    Some("SendCustomEvent") => Some(&mut personal),
                    Some("SendGlobalCustomEvent") => Some(&mut global),
                    _ => None,
                };
                if let Some(bucket) = bucket
                    && let Some(chan) = ce_send_event_name(args)
                {
                    let slots = bucket.entry(chan).or_default();
                    for (slot, expr) in ce_send_data_args(args) {
                        if let Some(t) = sent_type(tmap, expr)
                            && wire_class(&t).is_some()
                        {
                            slots.entry(slot).or_insert(t); // first sender wins
                        }
                    }
                }
            }
        },
    );

    // 2. Resolve each receiver's UNANNOTATED slots.
    let mut map = CeSlotMap::default();
    let mut diags = Vec::new();
    for (h, event_name) in receivers {
        let Some(ns) = CeNamespace::from_event_name(event_name) else {
            continue;
        };
        let bucket = match ns {
            CeNamespace::Personal => &personal,
            CeNamespace::Global => &global,
        };
        // Only constant-channel receivers can key against senders.
        let chan = h.config.iter().find_map(|c| match c {
            HandlerConfigArg::Positional(Expr::StringLit { value, .. }) => Some(value.clone()),
            _ => None,
        });
        let has_unannotated = h.params.iter().any(|p| p.ty.is_none());
        if !has_unannotated {
            continue;
        }
        let key = ce_slot_key(ns, h);
        let slots_out = map.entry(key).or_insert_with(|| vec![None; h.params.len()]);
        for (i, p) in h.params.iter().enumerate() {
            if p.ty.is_some() {
                continue; // annotated: nothing to infer
            }
            let inferred = chan.as_ref().and_then(|c| bucket.get(c)).and_then(|m| m.get(&i)).cloned();
            match inferred {
                Some(t) => slots_out[i] = Some(t),
                None => {
                    slots_out[i] = Some(Type::Float);
                    // No WS042 for a dynamic (non-constant) channel — nothing to key on.
                    if chan.is_some() {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS042".into(),
                            message: format!(
                                "custom event '{}' data param '{}' (#{}): no in-unit sender \
                                 to infer its type from, so it defaults to float; annotate \
                                 `{}: <type>` to silence",
                                chan.as_deref().unwrap_or(""),
                                p.name,
                                i + 1,
                                p.name,
                            ),
                            range: p.range.clone(),
                        });
                    }
                }
            }
        }
    }
    (map, diags)
}
