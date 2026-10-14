//! Sources evaluated in source order.
//!
//! A construct that evaluates several sources (a record literal's fields, a
//! call's arguments, an operator's operands, an enum payload) lowers them one
//! at a time, and a lowered source can still read storage when it is used: a
//! field naming a var, a pure value computed from one, an `out`, a chip input.
//! [`Ordered`] marks the exec chain before each source and, once it is lowered,
//! reads what its new gates write off the IR ([`writes_since`]). An earlier
//! source whose reads those writes may change is held at its value just before
//! the later source's first exec gate, by splicing the hold into the chain
//! there ([`at_mark`]). A write whose storage input comes straight from a
//! storage gate (or a chip parameter pin wired from one) writes exactly that
//! gate; any other may write any gate of its kind, and an `emit` of a local
//! signal may write anything.

use super::*;
use crate::catalog::storage::{Access, Kind};
use crate::intern::Sym;
use crate::lower::context::{reset_var_get_caches, restore_var_caches, snapshot_var_caches};
use crate::lower::record_assign::named_field;
use crate::lower::stmt::{branch_record_fields, set_scalar_var, value_record_fields};

fn kind(storage: VarStorage) -> Option<Kind> {
    match storage {
        VarStorage::Var => Some(Kind::Var),
        VarStorage::Array => Some(Kind::Array),
        VarStorage::Map => Some(Kind::Map),
        VarStorage::Buffer => None,
    }
}

/// Storage a lowered value reads when it is used.
#[derive(Default)]
pub(super) struct Reads {
    /// Vars read live.
    pub(super) vars: HashSet<NodeId>,
    /// Containers copied where the value is written, by kind; `None` for one
    /// behind a ref.
    pub(super) containers: Vec<(Option<NodeId>, Kind)>,
    /// Any var: a read through a ref, or a cone too large to walk.
    unknown: bool,
    /// A value from an enclosing module (its node, or this chip's input),
    /// which can read that module's vars.
    outer: bool,
}

impl Reads {
    fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.containers.is_empty() && !self.unknown && !self.outer
    }

    pub(super) fn add(&mut self, o: Reads) {
        self.vars.extend(o.vars);
        self.containers.extend(o.containers);
        self.unknown |= o.unknown;
        self.outer |= o.outer;
    }
}

/// Storage a stretch of lowering writes: each gate a writer's storage input
/// comes straight from, and each kind written through anything else (a ref
/// read from a var, an array element, a parameter pin), which may be any gate
/// of that kind.
#[derive(Default)]
pub(super) struct Writes {
    vars: HashSet<NodeId>,
    containers: HashMap<NodeId, Kind>,
    any: HashSet<Kind>,
    /// Some var written belongs to an enclosing module.
    outer: bool,
}

impl Writes {
    fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.containers.is_empty() && self.any.is_empty()
    }

    fn all() -> Self {
        Writes {
            any: [Kind::Var, Kind::Array, Kind::Map].into_iter().collect(),
            outer: true,
            ..Writes::default()
        }
    }

    /// `b`'s storage is written.
    pub(super) fn add(&mut self, ctx: &LowerCtx, b: &Binding) {
        match b {
            Binding::Var(v) => match kind(v.storage) {
                Some(Kind::Var) => {
                    self.vars.insert(v.node_id);
                    self.outer |= !exists(ctx, v.node_id);
                }
                Some(k) => {
                    self.containers.insert(v.node_id, k);
                }
                None => {}
            },
            Binding::Record(f) => f.values().for_each(|b| self.add(ctx, b)),
            _ => {}
        }
    }

    /// Whether these writes may change a scalar `r` reads.
    pub(super) fn changes(&self, r: &Reads) -> bool {
        let any_var = self.any.contains(&Kind::Var);
        (r.unknown && (any_var || !self.vars.is_empty()))
            || (r.outer && (any_var || self.outer))
            || (any_var && !r.vars.is_empty())
            || r.vars.iter().any(|n| self.vars.contains(n))
    }

    /// Whether these writes may change a container `r` copies.
    fn changes_container(&self, r: &Reads) -> bool {
        r.containers.iter().any(|(n, k)| {
            self.any.contains(k)
                || match n {
                    Some(n) => self.containers.contains_key(n),
                    None => self.containers.values().any(|c| c == k),
                }
        })
    }
}

/// A module's wires and chips indexed for the walks here: each node's
/// incoming wires across the module and its chips, and the chip each chip
/// node sits in. Extended as wires and chips are added, and rebuilt when the
/// wire list was rewritten.
#[derive(Default)]
pub(super) struct ConeIndex {
    incoming: HashMap<NodeId, Vec<PortRef>>,
    /// The module's own wires by the node they feed: the port fed and the
    /// source, in wire order.
    feeds: HashMap<NodeId, Vec<(WirePort, PortRef)>>,
    indexed: usize,
    last: Option<crate::ir::Wire>,
    /// Each chip reached, at every depth, by its node count, wire count and
    /// chip count when it was indexed.
    chips: HashMap<NodeId, (usize, usize, usize)>,
    /// The chip keys leading from the module to the chip module holding each
    /// node that is not the module's own.
    owner: HashMap<NodeId, Vec<NodeId>>,
}

impl ConeIndex {
    fn sync(&mut self, m: &crate::ir::Module) {
        self.sync_wires(m);
        self.add_chips(m, &mut Vec::new());
    }

    /// Index the module's wires added since the last sync, starting over when
    /// the indexed ones were rewritten. Its chips are left to `add_chips`.
    fn sync_wires(&mut self, m: &crate::ir::Module) {
        let wires = &m.wires;
        let valid = self.indexed <= wires.len()
            && (self.indexed == 0 || Some(wires[self.indexed - 1]) == self.last);
        if !valid {
            *self = Self::default();
        }
        for w in &wires[self.indexed..] {
            self.incoming.entry(w.target.node_id).or_default().push(w.source);
            self.feeds.entry(w.target.node_id).or_default().push((w.target.port, w.source));
        }
        self.indexed = wires.len();
        self.last = wires.last().copied();
    }

    /// Index every chip under `m` that is new or has changed since, with the
    /// chips inside it.
    fn add_chips(&mut self, m: &crate::ir::Module, path: &mut Vec<NodeId>) {
        for (id, chip) in &m.chips {
            let shape = (chip.nodes.len(), chip.wires.len(), chip.chips.len());
            if self.chips.insert(*id, shape) == Some(shape) {
                continue;
            }
            path.push(*id);
            for n in chip.nodes.keys() {
                self.owner.insert(*n, path.clone());
            }
            for w in &chip.wires {
                self.incoming.entry(w.target.node_id).or_default().push(w.source);
            }
            self.add_chips(chip, path);
            path.pop();
        }
    }

    /// The node `id` in `m` or one of its chips, and whether it is `m`'s own,
    /// as of the last [`sync`](Self::sync).
    fn find<'m>(&self, m: &'m crate::ir::Module, id: NodeId) -> Option<(&'m crate::ir::Node, bool)> {
        if let Some(n) = m.nodes.get(&id) {
            return Some((n, true));
        }
        let mut at = m;
        for key in self.owner.get(&id)? {
            at = at.chips.get(key)?;
        }
        at.nodes.get(&id).map(|n| (n, false))
    }

    /// Wire `i` of the module now comes from `src` instead of `old`.
    fn moved(&mut self, i: usize, w: crate::ir::Wire, old: PortRef) {
        if i >= self.indexed {
            return;
        }
        if let Some(from) = self.incoming.get_mut(&w.target.node_id)
            && let Some(p) = from.iter_mut().find(|p| **p == old)
        {
            *p = w.source;
        }
        if let Some(from) = self.feeds.get_mut(&w.target.node_id)
            && let Some((_, p)) = from.iter_mut().find(|(port, p)| *port == w.target.port && *p == old)
        {
            *p = w.source;
        }
        if i + 1 == self.indexed {
            self.last = Some(w);
        }
    }
}

/// The sources of the module's own wires into `id`, each with the port it
/// feeds.
pub(super) fn feeds_of(ctx: &LowerCtx, id: NodeId) -> Vec<(WirePort, PortRef)> {
    let mut index = ctx.cone_index.borrow_mut();
    index.sync_wires(&ctx.builder.module);
    index.feeds.get(&id).cloned().unwrap_or_default()
}

/// Whether the node `id` is in this module or one of its chips.
fn exists(ctx: &LowerCtx, id: NodeId) -> bool {
    let m = &ctx.builder.module;
    if m.nodes.contains_key(&id) {
        return true;
    }
    let mut index = ctx.cone_index.borrow_mut();
    index.sync(m);
    index.find(m, id).is_some()
}

/// What the pure cone of `port` reads live, through gates with no exec input
/// and into the chips whose outputs it reaches.
pub(super) fn live_reads(ctx: &LowerCtx, port: PortRef) -> Reads {
    let module = &ctx.builder.module;
    let chip_body = matches!(
        module.scopes.get(&ROOT_SCOPE_ID).map(|s| &s.kind),
        Some(ScopeKind::ChipBody { .. })
    );
    let mut index = ctx.cone_index.borrow_mut();
    index.sync(module);
    let mut r = Reads::default();
    let mut seen = HashSet::default();
    let mut stack = vec![port];
    while let Some(p) = stack.pop() {
        let Some((node, own)) = index.find(module, p.node_id) else {
            r.outer = true;
            continue;
        };
        if node.gate_class == gc::VAR {
            if p.port == WirePort::Value {
                r.vars.insert(p.node_id);
            }
            continue;
        }
        if own && node.gate_class == gc::MICROCHIP_INPUT {
            r.outer |= chip_body;
            continue;
        }
        if node.ports.inputs.iter().any(|i| i.ty == Type::Exec) || !seen.insert(p.node_id) {
            continue;
        }
        if seen.len() > 512 {
            r.unknown = true;
            break;
        }
        let from: Vec<PortRef> = index.incoming.get(&p.node_id).into_iter().flatten().copied().collect();
        stack.extend(from);
    }
    r
}

/// The storage reading `b` reads: a stored var, every leaf of a record, what a
/// pure value or a port reads live. A container is a reference, read when it
/// is used.
pub(super) fn binding_reads(ctx: &LowerCtx, b: &Binding) -> Reads {
    let mut r = Reads::default();
    match b {
        Binding::Var(v) if v.storage == VarStorage::Var => {
            r.vars.insert(v.node_id);
        }
        Binding::Record(f) => f.values().for_each(|b| r.add(binding_reads(ctx, b))),
        Binding::Local(l) => r = live_reads(ctx, l.port),
        Binding::Input(i) => r = live_reads(ctx, i.node_id.port(WirePort::RerOutput)),
        Binding::Output(o) => r = live_reads(ctx, o.node_id.port(WirePort::RerOutput)),
        Binding::DynRef(d) if d.storage == VarStorage::Var => r.unknown = true,
        _ => {}
    }
    r
}

/// The containers under `b`, which writing it copies.
pub(super) fn copied(b: &Binding) -> Reads {
    let mut r = Reads::default();
    match b {
        Binding::Var(v) => {
            if let Some(k @ (Kind::Array | Kind::Map)) = kind(v.storage) {
                r.containers.push((Some(v.node_id), k));
            }
        }
        Binding::DynRef(d) => {
            if let Some(k @ (Kind::Array | Kind::Map)) = kind(d.storage) {
                r.containers.push((None, k));
            }
        }
        Binding::Record(f) => f.values().for_each(|b| r.add(copied(b))),
        _ => {}
    }
    r
}

/// Every storage gate under `b`.
pub(super) fn storage_leaves(b: &Binding, out: &mut HashSet<NodeId>) {
    match b {
        Binding::Var(v) => {
            out.insert(v.node_id);
        }
        Binding::Record(f) => f.values().for_each(|b| storage_leaves(b, out)),
        _ => {}
    }
}

/// The storage gate a writer's storage input names: the source itself when it
/// is a storage gate's ref port, or the one wired into a chip parameter pin.
fn storage_behind(ctx: &LowerCtx, mut p: PortRef) -> Option<NodeId> {
    let root = &ctx.builder.module;
    let mut index = ctx.cone_index.borrow_mut();
    index.sync(root);
    for _ in 0..64 {
        match p.port {
            WirePort::VarRef | WirePort::ArrayVarRef | WirePort::MapVarRef => return Some(p.node_id),
            WirePort::RerOutput if index.find(root, p.node_id)?.0.gate_class == gc::MICROCHIP_INPUT => {
                let from = index.incoming.get(&p.node_id)?;
                if from.iter().any(|s| *s != from[0]) {
                    return None;
                }
                p = *from.first()?;
            }
            _ => return None,
        }
    }
    None
}

fn note_write(ctx: &LowerCtx, m: &crate::ir::Module, wire: &crate::ir::Wire, w: &mut Writes) {
    let Some(class) = m.nodes.get(&wire.target.node_id).map(|n| n.gate_class) else {
        return;
    };
    if crate::catalog::storage::access(class, wire.target.port) != Some(Access::Write) {
        return;
    }
    let Some(k) = crate::catalog::storage::port_kind(class, wire.target.port) else {
        return;
    };
    match storage_behind(ctx, wire.source) {
        Some(node) if k == Kind::Var => {
            w.vars.insert(node);
            w.outer |= !exists(ctx, node);
        }
        Some(node) => {
            w.containers.insert(node, k);
        }
        None => {
            w.any.insert(k);
        }
    }
}

fn chip_writes(ctx: &LowerCtx, m: &crate::ir::Module, w: &mut Writes) {
    for wire in &m.wires {
        note_write(ctx, m, wire, w);
    }
    m.chips.values().for_each(|c| chip_writes(ctx, c, w));
}

/// Where the exec chain and the IR stood before a source was lowered.
#[derive(Clone, Copy)]
pub(super) struct Mark {
    exec: Option<PortRef>,
    node: NodeId,
    wires: usize,
    last: Option<crate::ir::Wire>,
    chips: usize,
    signal_emits: u32,
    emits: usize,
}

pub(super) fn mark(ctx: &LowerCtx) -> Mark {
    let module = &ctx.builder.module;
    Mark {
        exec: ctx.current_exec,
        node: NodeId::next(),
        wires: module.wires.len(),
        last: module.wires.last().copied(),
        chips: module.chips.len(),
        signal_emits: ctx.signal_emits,
        emits: ctx.emit_log.len(),
    }
}

/// The storage every gate lowered since `m` writes, in this module and in
/// every chip added since. A wire list rewritten before `m`, or an `emit` of a
/// local signal (whose handlers run off this chain), counts as writing
/// anything.
fn writes_since(ctx: &LowerCtx, m: &Mark) -> Writes {
    let module = &ctx.builder.module;
    let wires = &module.wires;
    if wires.len() < m.wires || (m.wires > 0 && Some(wires[m.wires - 1]) != m.last) || ctx.signal_emits != m.signal_emits {
        return Writes::all();
    }
    let mut w = Writes::default();
    for wire in &wires[m.wires..] {
        note_write(ctx, module, wire, &mut w);
    }
    if module.chips.len() != m.chips {
        for (id, chip) in &module.chips {
            if *id >= m.node {
                chip_writes(ctx, chip, &mut w);
            }
        }
    }
    w
}

/// Run `f` where the chain stood at `m`: the exec gates it adds are spliced in
/// ahead of whatever the chain ran after `m` (an `emit` queued there since `m`
/// too),
/// which then continues from them. Var reads cached since `m` are not reused,
/// and those `f` makes are dropped.
fn at_mark<R>(ctx: &mut LowerCtx, m: &Mark, f: impl FnOnce(&mut LowerCtx) -> R) -> R {
    let Some(cursor) = m.exec else {
        return f(ctx);
    };
    let now = ctx.current_exec;
    let moved = now != Some(cursor);
    let split = ctx.builder.module.wires.len();
    let caches = moved.then(|| snapshot_var_caches(ctx));
    if moved {
        reset_var_get_caches(ctx);
        ctx.current_exec = Some(cursor);
    }
    let out = f(ctx);
    if let Some(caches) = caches {
        restore_var_caches(ctx, &caches);
    }
    let Some(end) = ctx.current_exec.filter(|e| *e != cursor) else {
        return out;
    };
    if moved {
        let mut index = ctx.cone_index.borrow_mut();
        for i in m.wires..split {
            let w = &mut ctx.builder.module.wires[i];
            if w.source == cursor {
                w.source = end;
                index.moved(i, *w, cursor);
            }
        }
        drop(index);
        ctx.current_exec = now;
    }
    let since = ctx.emit_log.get(m.emits..).unwrap_or_default().to_vec();
    for (key, i) in since {
        if let Some((port, _)) = ctx.pending_emits.get_mut(&key).and_then(|q| q.get_mut(i))
            && *port == cursor
        {
            *port = end;
        }
    }
    out
}

/// What each source held at one point of the chain is held as, so a source
/// named twice there is held once.
pub(super) type Held = HashMap<PortRef, Binding>;

/// `b` fixed at its value now: a stored field is read onto the exec chain, and
/// a pure value (or chip input or output) that reads storage live is held in a
/// new var, so neither a later side effect nor a later write can change it.
/// `t` is the target it will be written to, for the held value's type. A
/// container is a reference and stays one.
pub(super) fn hold(ctx: &mut LowerCtx, b: &Binding, t: Option<&Binding>, label: &str, range: &SourceRange, held: &mut Held) -> Binding {
    let key = match b {
        Binding::Record(f) => {
            let mut keys: Vec<Sym> = f.keys().copied().collect();
            keys.sort_by_key(|k| crate::intern::resolve(*k));
            let mut out = HashMap::default();
            for k in keys {
                let sub_t = match t {
                    Some(Binding::Record(tf)) => tf.get(&k),
                    _ => None,
                };
                let sub_label = match label {
                    "" => crate::intern::resolve(k).to_string(),
                    _ => format!("{label}.{}", crate::intern::resolve(k)),
                };
                out.insert(k, hold(ctx, &f[&k], sub_t, &sub_label, range, held));
            }
            return Binding::Record(out);
        }
        Binding::Var(v) if v.storage == VarStorage::Var => v.node_id.port(WirePort::VarRef),
        Binding::DynRef(d) if d.storage == VarStorage::Var => d.port,
        Binding::Local(l) => l.port,
        Binding::Input(i) => i.node_id.port(WirePort::RerOutput),
        Binding::Output(o) => o.node_id.port(WirePort::RerOutput),
        _ => return b.clone(),
    };
    if let Some(done) = held.get(&key) {
        return done.clone();
    }
    let out = match b {
        Binding::Var(_) => match binding_to_port(ctx, b, range) {
            Some(port) => Binding::Local(LocalRecord { port }),
            None => return b.clone(),
        },
        Binding::DynRef(d) => Binding::Local(LocalRecord {
            port: crate::lower::expr::read_dyn_ref(ctx, d, range),
        }),
        _ if ctx.current_exec.is_some() && !binding_reads(ctx, b).is_empty() => {
            let Some(port) = binding_to_port(ctx, b, range) else {
                return b.clone();
            };
            let ty = match t {
                Some(Binding::Var(v)) if v.storage == VarStorage::Map => crate::lower::access::map_kv(v).1,
                Some(Binding::Var(v)) => v.inner_type.clone(),
                _ => crate::lower::call::arg_port_type(ctx, port).unwrap_or(Type::Any),
            };
            let mut props = HashMap::default();
            props.insert(*sym::NAME_LABEL, Literal::String(format!("_{label}")));
            let node = crate::lower::predeclare::make_scalar_var_gate(ctx, &ty, props, range, None);
            let var = VarRecord::new(node, ty, VarStorage::Var);
            set_scalar_var(ctx, &var, port, range);
            Binding::Local(LocalRecord {
                port: node.port(WirePort::Value),
            })
        }
        _ => return b.clone(),
    };
    held.insert(key, out.clone());
    out
}

/// A value lowered where the written order reaches it rather than at its
/// source position: an [`inert`] expression, which only reads.
///
/// [`inert`]: crate::lower::record_assign
#[derive(Clone, Copy)]
pub(super) enum Later<'e> {
    /// A container literal's element or map value; `true` split per field
    /// for a record array or map.
    Elem(&'e Expr, bool),
    /// A map literal entry's key.
    Key(&'e Expr),
    /// A record-literal field or spread, lowered as the literal lowers it,
    /// and what its slot holds.
    Field(&'e RecordLitField, crate::lower::decl::Slots<'static>),
}

impl Later<'_> {
    pub(super) fn lower(self, ctx: &mut LowerCtx) -> Option<Binding> {
        match self {
            Later::Elem(e, true) => value_record_fields(ctx, e).map(Binding::Record),
            Later::Elem(e, false) | Later::Key(e) => Some(Binding::Local(LocalRecord { port: lower_expr(ctx, e) })),
            Later::Field(RecordLitField::Spread { value, .. }, _) => {
                branch_record_fields(ctx, value).map(Binding::Record)
            }
            Later::Field(f, slots) => {
                Some(Binding::Record(crate::lower::decl::lower_record_lit(ctx, std::slice::from_ref(f), slots)))
            }
        }
    }

    /// The storage a field or spread names, known without lowering it: its
    /// record, holding just the fields that name storage.
    pub(super) fn named(self, ctx: &LowerCtx) -> Option<HashMap<Sym, Binding>> {
        let Later::Field(f, slots) = self else { return None };
        match f {
            RecordLitField::Spread { value, .. } => match resolve_field_chain(ctx, value) {
                Some(Binding::Record(r)) => Some(r.clone()),
                _ => None,
            },
            _ => {
                let (name, value) = named_field(f)?;
                let b = resolve_field_chain(ctx, &value)?.clone();
                let b = match &b {
                    Binding::Record(rec) if slots.one_value(name) => {
                        held_output(ctx, rec, value.range(), OutputPick::First).unwrap_or(b)
                    }
                    _ => b,
                };
                Some(std::iter::once((crate::intern::intern(name), b)).collect())
            }
        }
    }
}

pub(super) enum State<'e> {
    /// Lowered, and may still read storage live.
    Live(Binding),
    /// Held, or reads nothing that can change.
    Fixed(Binding),
    Later(Later<'e>),
    /// A container the construct copies when it is written; only watched.
    Copied,
    /// Lowered to nothing of the form it needs.
    Missing,
    Gone,
}

pub(super) struct Slot<'e> {
    pub(super) state: State<'e>,
    label: String,
    pub(super) target: Option<Binding>,
    range: SourceRange,
    reads: Option<Reads>,
}

/// Sources evaluated in order, each kept at the value it had when it was
/// evaluated: when a later source's gates may write what an earlier one reads,
/// the earlier one is held just before the later one runs. A container copied
/// when the construct writes it can't be held, so one a later source may
/// change is WS089.
#[derive(Default)]
pub(super) struct Ordered<'e> {
    pub(super) slots: Vec<Slot<'e>>,
}

impl<'e> Ordered<'e> {
    pub(super) fn add(&mut self, state: State<'e>, label: String, target: Option<Binding>, range: &SourceRange, reads: Option<Reads>) -> usize {
        self.slots.push(Slot {
            state,
            label,
            target,
            range: range.clone(),
            reads,
        });
        self.slots.len() - 1
    }

    /// A lowered source, labelled for its hold, and the target it will be
    /// written to if any.
    pub(super) fn push(&mut self, b: Binding, label: &str, target: Option<&Binding>, range: &SourceRange) -> usize {
        self.add(State::Live(b), label.to_string(), target.cloned(), range, None)
    }

    /// Lower one source with `f`, holding before it each earlier source whose
    /// reads its gates may write, and keep what it gives in a new slot, with
    /// the target it will be written to if any.
    pub(super) fn source(
        &mut self,
        ctx: &mut LowerCtx,
        label: &str,
        target: Option<&Binding>,
        range: &SourceRange,
        f: impl FnOnce(&mut LowerCtx) -> Option<Binding>,
    ) -> Option<usize> {
        let m = mark(ctx);
        let b = f(ctx);
        self.settle(ctx, &m);
        b.map(|b| self.push(b, label, target, range))
    }

    /// [`source`](Self::source) for a value lowered to one port.
    pub(super) fn port(&mut self, ctx: &mut LowerCtx, e: &Expr) -> usize {
        let m = mark(ctx);
        let port = lower_expr(ctx, e);
        self.settle(ctx, &m);
        self.push(Binding::Local(LocalRecord { port }), "", None, e.range())
    }

    /// The binding in `slot`, lowering it now if it waited.
    pub(super) fn value(&mut self, ctx: &mut LowerCtx, slot: usize) -> Option<Binding> {
        if let State::Later(l) = self.slots[slot].state {
            self.slots[slot].state = l.lower(ctx).map_or(State::Missing, State::Live);
        }
        match &self.slots[slot].state {
            State::Live(b) | State::Fixed(b) => Some(b.clone()),
            _ => None,
        }
    }

    /// The port in `slot`, as [`port`](Self::port) left it.
    pub(super) fn port_of(&mut self, ctx: &mut LowerCtx, slot: usize) -> PortRef {
        match self.value(ctx, slot) {
            Some(Binding::Local(l)) => l.port,
            _ => unreachable!("a port slot holds a port"),
        }
    }

    /// Forget `slot`: a later source supplies what it did.
    pub(super) fn forget(&mut self, slot: usize) {
        self.slots[slot].state = State::Gone;
    }

    /// Forget `key` of the record in `slot`: a later source supplies it.
    pub(super) fn drop_key(&mut self, slot: usize, key: Sym) {
        if let State::Live(Binding::Record(f)) | State::Fixed(Binding::Record(f)) = &mut self.slots[slot].state {
            f.remove(&key);
            self.slots[slot].reads = None;
        }
    }

    /// Watch the containers `copies` names, which the construct copies at
    /// `path` when it writes it.
    pub(super) fn watch(&mut self, path: String, copies: Reads, range: &SourceRange) -> Option<usize> {
        (!copies.containers.is_empty()).then(|| self.add(State::Copied, path, None, range, Some(copies)))
    }

    fn reads(&mut self, ctx: &LowerCtx, i: usize) -> &Reads {
        let slot = &mut self.slots[i];
        if slot.reads.is_none() {
            let r = match &slot.state {
                State::Live(b) => binding_reads(ctx, b),
                _ => Reads::default(),
            };
            if r.is_empty()
                && matches!(slot.state, State::Live(_))
                && let State::Live(b) = std::mem::replace(&mut slot.state, State::Gone)
            {
                slot.state = State::Fixed(b);
            }
            slot.reads = Some(r);
        }
        slot.reads.as_ref().expect("computed above")
    }

    /// Hold `slot` at its value here, lowering it first if it waited.
    pub(super) fn fix(&mut self, ctx: &mut LowerCtx, i: usize, held: &mut Held) {
        let b = match std::mem::replace(&mut self.slots[i].state, State::Gone) {
            State::Live(b) => b,
            State::Later(l) => match l.lower(ctx) {
                Some(b) => b,
                None => {
                    self.slots[i].state = State::Missing;
                    return;
                }
            },
            other => {
                self.slots[i].state = other;
                return;
            }
        };
        let slot = &self.slots[i];
        let now = hold(ctx, &b, slot.target.as_ref(), &slot.label, &slot.range, held);
        self.slots[i].state = State::Fixed(now);
    }

    fn waiting(&self) -> bool {
        self.slots
            .iter()
            .any(|s| matches!(s.state, State::Live(_) | State::Later(_) | State::Copied))
    }

    /// After the source lowered since `m`: hold, just before it, each earlier
    /// source whose reads its writes may change, and report a container one
    /// of them copies later that they may change (WS089).
    pub(super) fn settle(&mut self, ctx: &mut LowerCtx, m: &Mark) {
        if m.exec.is_none() || !self.waiting() {
            return;
        }
        let w = writes_since(ctx, m);
        if w.is_empty() {
            return;
        }
        let mut hit = Vec::new();
        for i in 0..self.slots.len() {
            let copy = match self.slots[i].state {
                State::Copied => true,
                State::Live(_) | State::Later(_) => false,
                _ => continue,
            };
            let reads = self.reads(ctx, i);
            if copy && w.changes_container(reads) {
                report_changed_copy(ctx, &self.slots[i].label, &self.slots[i].range);
                self.slots[i].state = State::Gone;
            } else if !copy && w.changes(reads) {
                hit.push(i);
            }
        }
        if !hit.is_empty() {
            let mut held = Held::default();
            at_mark(ctx, m, |ctx| hit.into_iter().for_each(|i| self.fix(ctx, i, &mut held)));
        }
    }

    /// Hold here each source whose reads `w` may change.
    pub(super) fn fix_changed(&mut self, ctx: &mut LowerCtx, w: &Writes) {
        let mut held = Held::default();
        for i in 0..self.slots.len() {
            if matches!(self.slots[i].state, State::Live(_) | State::Later(_)) && w.changes(self.reads(ctx, i)) {
                self.fix(ctx, i, &mut held);
            }
        }
    }
}

fn report_changed_copy(ctx: &mut LowerCtx, path: &str, range: &SourceRange) {
    ctx.error(
        "WS089",
        format!(
            "`{path}` copies a container when this assignment writes it, which is after a later \
             field's call has run, and that call may change the container. A container is copied \
             when it is written, not read up front. Put the field with the call before `{path}`, \
             or run the call in a `let` before the assignment"
        ),
        range,
    );
}

/// Lower `exprs` in order to one port each, every one held before a later one
/// runs code that may change what it reads.
pub(super) fn lower_in_order(ctx: &mut LowerCtx, exprs: &[&Expr]) -> Vec<PortRef> {
    // With no exec chain nothing runs, so nothing can be held.
    if exprs.len() < 2 || ctx.current_exec.is_none() {
        return exprs.iter().map(|e| lower_expr(ctx, e)).collect();
    }
    let mut ord = Ordered::default();
    let slots: Vec<usize> = exprs.iter().map(|e| ord.port(ctx, e)).collect();
    slots.into_iter().map(|s| ord.port_of(ctx, s)).collect()
}

/// A call's arguments in order, each held before a later argument runs code
/// that may change what it reads. An argument wired into a pin as soon as it
/// is lowered has that wire moved onto its held value.
#[derive(Default)]
pub(super) struct ArgOrder {
    ord: Ordered<'static>,
    at: Option<Mark>,
    bound: Vec<(Binding, String, SourceRange, Option<usize>)>,
    slots: Vec<(usize, Binding, Option<std::ops::Range<usize>>)>,
}

impl ArgOrder {
    /// Settle the argument lowered since the last call, then mark the chain
    /// for the next one.
    pub(super) fn next(&mut self, ctx: &mut LowerCtx) {
        if let Some(m) = self.at.take() {
            self.ord.settle(ctx, &m);
        }
        let end = ctx.builder.module.wires.len();
        for (b, name, range, wired) in self.bound.drain(..) {
            let slot = self.ord.push(b.clone(), &name, None, &range);
            self.slots.push((slot, b, wired.map(|from| from..end)));
        }
        self.at = Some(mark(ctx));
    }

    /// `b` is the argument for `name`, wired into its pins from wire `wired`
    /// on when it is wired as it is bound. Its index among the bound
    /// arguments is what [`finish`](Self::finish) answers by.
    pub(super) fn bind(&mut self, b: Binding, name: &str, range: &SourceRange, wired: Option<usize>) -> usize {
        self.bound.push((b, name.to_string(), range.clone(), wired));
        self.slots.len() + self.bound.len() - 1
    }

    /// Each bound argument as it was held, a wired one's pins rewired.
    pub(super) fn finish(mut self, ctx: &mut LowerCtx) -> Vec<Option<Binding>> {
        self.next(ctx);
        let mut out = Vec::with_capacity(self.slots.len());
        for (slot, before, wired) in std::mem::take(&mut self.slots) {
            let now = self.ord.value(ctx, slot);
            if let (Some(now), Some(wires)) = (&now, wired) {
                rewire(ctx, &before, now, wires);
            }
            out.push(now);
        }
        out
    }
}

/// Point each wire in `wires` that carried a port of `before` at the matching
/// port of `after`, the same value held. `before` is what was wired: the
/// ports a record or value argument lowered to.
fn rewire(ctx: &mut LowerCtx, before: &Binding, after: &Binding, wires: std::ops::Range<usize>) {
    fn pairs(before: &Binding, after: &Binding, out: &mut HashMap<PortRef, PortRef>) {
        match (before, after) {
            (Binding::Record(a), Binding::Record(b)) => {
                for (k, x) in a {
                    if let Some(y) = b.get(k) {
                        pairs(x, y, out);
                    }
                }
            }
            (Binding::Local(a), Binding::Local(b)) if a.port != b.port => {
                out.insert(a.port, b.port);
            }
            _ => {}
        }
    }
    let mut map = HashMap::default();
    pairs(before, after, &mut map);
    if map.is_empty() {
        return;
    }
    let mut index = ctx.cone_index.borrow_mut();
    for i in wires {
        let w = &mut ctx.builder.module.wires[i];
        if let Some(p) = map.get(&w.source) {
            let old = w.source;
            w.source = *p;
            index.moved(i, *w, old);
        }
    }
}
