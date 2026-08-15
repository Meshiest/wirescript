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
}

fn wire_class(t: &Type) -> Option<WireClass> {
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

/// Resolve a Custom Event param annotation to its type without emitting — the
/// handler-binding pass already reported any bad annotation, so this must not
/// double-report. Custom-event data is always a wire variant (a primitive), so
/// anything more exotic resolves to `any` and simply isn't linted. Delegates
/// to the crate's single canonical resolver (`types::resolve::resolve_type`);
/// any diagnostic it would emit is discarded per the no-double-report rule
/// above.
fn ce_param_type(te: &TypeExpr) -> Type {
    let cx = crate::types::resolve::ResolveCtx {
        params: &[],
        type_aliases: &HashMap::default(),
        generic_aliases: &HashMap::default(),
    };
    crate::types::resolve::resolve_type(te, &cx, &mut Vec::new())
}

/// `(channel name, per-slot declared type)` for a handler that is a custom-event
/// receiver of kind `event_name` (`"CustomEvent"` or `"GlobalCustomEvent"`) with
/// a literal channel name. `None` for any other handler (or a dynamic channel
/// name — nothing to key receivers by). An unannotated slot falls back to
/// `ce_slots`' inferred type for this handler (keyed by its source range) when
/// present — pass 1 has no inference available yet, so it passes an empty map.
fn ce_receiver_of(
    h: &Handler,
    event_name: &str,
    ce_slots: &CeSlotMap,
) -> Option<(String, Vec<Option<Type>>)> {
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
        .map(|(i, p)| match p.ty.as_ref() {
            Some(te) => Some(ce_param_type(te)),
            // Fill from inference when available (pass 2); else unresolved.
            None => inferred.and_then(|v| v.get(i).cloned().flatten()),
        })
        .collect();
    Some((name, params))
}

/// Fold a receiver's declared params into the per-channel signature, keeping the
/// first classifiable type seen at each slot. Multiple receivers for one channel
/// are allowed; they define the same wire, so the first concrete type wins.
fn ce_merge_receiver(
    map: &mut HashMap<String, Vec<Option<Type>>>,
    name: String,
    params: Vec<Option<Type>>,
) {
    let slots = map.entry(name).or_default();
    for (i, ty) in params.into_iter().enumerate() {
        if slots.len() <= i {
            slots.resize(i + 1, None);
        }
        if slots[i].is_none()
            && let Some(t) = ty
            && wire_class(&t).is_some()
        {
            slots[i] = Some(t);
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
    let mut personal_recv: HashMap<String, Vec<Option<Type>>> = HashMap::default();
    let mut global_recv: HashMap<String, Vec<Option<Type>>> = HashMap::default();
    let mut personal_send: Vec<&Expr> = Vec::new();
    let mut global_send: Vec<&Expr> = Vec::new();
    crate::analysis::visit_program(
        script,
        &mut |h| {
            if let Some((name, params)) = ce_receiver_of(h, "CustomEvent", ce_slots) {
                ce_merge_receiver(&mut personal_recv, name, params);
            } else if let Some((name, params)) = ce_receiver_of(h, "GlobalCustomEvent", ce_slots) {
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
    check_ce_namespace(ctx, &personal_send, &personal_recv, tmap, "SendCustomEvent", "CustomEvent");
    check_ce_namespace(
        ctx,
        &global_send,
        &global_recv,
        tmap,
        "SendGlobalCustomEvent",
        "GlobalCustomEvent",
    );
}

/// Within ONE custom-event namespace, warn (WS030) when a send's data value type
/// disagrees with the receiver's declared param type for the same channel — the
/// game keys the wire variant off the sender, so a mismatch is a real bug.
fn check_ce_namespace(
    ctx: &mut TypeCheckCtx,
    senders: &[&Expr],
    receivers: &HashMap<String, Vec<Option<Type>>>,
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
            let Some(t) = tmap.get(&(r.file.clone(), r.start.offset, r.end.offset)) else {
                continue;
            };
            let t = unwrap_ref(t);
            if wire_class(&t).is_none() {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS045".into(),
                    message: format!(
                        "custom event '{name}' data #{} has no concrete type (it is {t}), so the                          send emits the float variant and will not match a receiver that declares                          int/bool/string/entity/… — give the value a typed binding (e.g. a `var` of                          that type) rather than `any` or `Opaque(...)`, which erases it",
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
            let Some(recv_ty) = slots.get(slot).and_then(|o| o.as_ref()) else {
                continue;
            };
            let Some(recv_class) = wire_class(recv_ty) else {
                continue;
            };
            let r = expr.range();
            let Some(send_ty) = tmap.get(&(r.file.clone(), r.start.offset, r.end.offset)) else {
                continue;
            };
            let send_ty = unwrap_ref(send_ty);
            let Some(send_class) = wire_class(&send_ty) else {
                continue;
            };
            if send_class != recv_class {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS030".into(),
                    message: format!(
                        "custom event '{name}' data #{} is {}, but the `on {event_name}(\"{name}\", …)` \
                         receiver declares {} — {send_name} values must match the receiver's param types",
                        slot + 1,
                        send_ty,
                        recv_ty,
                    ),
                    range: r.clone(),
                });
            }
        }
    }
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
                        let r = expr.range();
                        if let Some(t) = tmap.get(&(r.file.clone(), r.start.offset, r.end.offset))
                        {
                            let t = unwrap_ref(t);
                            if wire_class(&t).is_some() {
                                slots.entry(slot).or_insert(t); // first sender wins
                            }
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
                                 to infer its type from; defaulting to float — annotate \
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
