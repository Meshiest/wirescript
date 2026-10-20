use super::*;

// ---------- context ----------

#[derive(Clone, Debug)]
pub(super) struct VarRecord {
    pub(super) node_id: NodeId,
    pub(super) inner_type: Type,
    pub(super) storage: VarStorage,
}

impl VarRecord {
    /// The record of the storage gate `node_id`, holding `inner_type`.
    pub(super) fn new(node_id: NodeId, inner_type: Type, storage: VarStorage) -> Self {
        Self { node_id, inner_type, storage }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VarStorage {
    Var,
    Buffer,
    Array,
    Map,
}

/// Classify a param/field type that binds a *container* — one whose value wires
/// a ref port (`ArrayVarRef`/`MapVarRef`) or is a scalar `ref T` — rather than a
/// plain by-value input. Returns the backing [`VarStorage`] and the
/// `inner_type` to record: the element type for arrays, the referent for a
/// scalar `ref T`, and the WHOLE `Type::Map(K,V)` for maps (matching
/// `pre_declare_map`). `None` for a by-value scalar.
///
/// Classifying containers in ONE place — instead of each binding site
/// re-deriving `is_array`/`is_ref` by pattern-match — is what keeps `Map<K,V>`
/// (and any future container) from silently falling through the `Array`/`Ref`
/// matches into a scalar `Var_Get` + `_Unsupported` method lowering.
/// The backing [`VarStorage`] for the shape of a param/field type (see
/// [`LowerCtx::declared_shape`]), or `None` for a by-value scalar;
/// [`container_binding`] adds the `inner_type`.
fn container_storage(typ: &crate::ast::TypeExpr) -> Option<VarStorage> {
    use crate::ast::TypeExpr as TE;
    let is_map = |t: &TE| matches!(t, TE::Generic { name, .. } if name == "Map");
    Some(match typ {
        TE::Array { .. } => VarStorage::Array,
        t if is_map(t) => VarStorage::Map,
        TE::Ref { inner, .. } if is_map(inner) => VarStorage::Map,
        TE::Ref { .. } => VarStorage::Var,
        _ => return None,
    })
}

/// Wrap each field's type in a `ref`, the field-wise form of `*T` on a record or
/// tuple. A field that is already a ref (written so or through an alias) is
/// left alone rather than becoming a double reference.
fn distribute_ref_fields(
    ctx: &LowerCtx,
    fields: Vec<crate::ast::RecordTypeField>,
    range: &crate::diagnostic::SourceRange,
) -> Vec<crate::ast::RecordTypeField> {
    fields
        .into_iter()
        .map(|mut f| {
            if !matches!(ctx.declared_shape(None, &f.typ), crate::ast::TypeExpr::Ref { .. }) {
                f.typ = crate::ast::TypeExpr::Ref {
                    inner: Box::new(f.typ),
                    range: range.clone(),
                };
            }
            f
        })
        .collect()
}

/// How deep [`LowerCtx::declared_shape`] follows aliases before it stops.
const MAX_SHAPE_DEPTH: u32 = 32;

/// The storage a parameter, field or pin of `storage` kind binds and the type
/// that storage holds, given its resolved type.
pub(super) fn container_binding(
    storage: Option<VarStorage>,
    resolved: &Type,
) -> Option<(VarStorage, Type)> {
    let storage = storage?;
    let inner = match storage {
        VarStorage::Array => match resolved {
            Type::Array(i) => (**i).clone(),
            Type::Ref(i) => match &**i {
                Type::Array(e) => (**e).clone(),
                _ => resolved.clone(),
            },
            _ => resolved.clone(),
        },
        // A map's `inner_type` carries the whole `Type::Map(K,V)`; a `*Map`
        // unwraps its outer ref to the same.
        VarStorage::Map | VarStorage::Var => match resolved {
            Type::Ref(i) => (**i).clone(),
            _ => resolved.clone(),
        },
        VarStorage::Buffer => resolved.clone(),
    };
    Some((storage, inner))
}

#[derive(Clone, Debug)]
pub(super) struct NodeRecord {
    pub(super) node_id: NodeId,
    pub(super) ty: Type,
}

#[derive(Clone, Debug)]
pub(super) struct LocalRecord {
    pub(super) port: PortRef,
}

/// A name bound to a ref that arrives on a wire at runtime rather than to
/// storage known at compile time: a `*T` or container parameter whose argument
/// was a ref value (`paint(pixels[0])`). Reads and writes go through `port`.
#[derive(Clone, Debug)]
pub(super) struct DynRefRecord {
    /// The wire carrying the ref.
    pub(super) port: PortRef,
    /// What the ref names: `Var` for a `*T`, `Array`, or `Map`.
    pub(super) storage: VarStorage,
    /// As `VarRecord::inner_type`: `T` for a `*T`, the element type for an
    /// array, the whole `Map(K, V)` for a map.
    pub(super) inner_type: Type,
}

#[derive(Clone, Debug)]
pub(super) enum Binding {
    Var(VarRecord),
    Local(LocalRecord),
    Buffer(NodeRecord),
    Input(NodeRecord),
    Output(NodeRecord),
    EventParam(PortRef),
    DynRef(DynRefRecord),
    Chip(std::sync::Arc<ChipDecl>),
    /// An `import * as ns` module, keyed by member name. Holds BOTH the module's
    /// chips (as `Binding::Chip`) and its value members (`let`/`array`/`map`/
    /// `var`/`buffer`, as their own lowered binding). Keeping the value bindings
    /// here — rather than only in the shared enclosing scope under their bare
    /// name — is what lets two namespaces that export the same member name stay
    /// distinct: `A.foo` and `B.foo` resolve through their own namespace map
    /// instead of colliding on one bare `foo` where the last import wins.
    Namespace(HashMap<String, Binding>),
    Record(HashMap<crate::intern::Sym, Binding>),
}

/// The scope a nested declaration saw where it was written. See
/// [`LowerCtx::record_decl_view`].
pub(super) struct DeclView {
    /// Distinct per registration: a body expanded twice declares twice, and
    /// a chip's compiled body captures that expansion's names.
    pub(super) serial: u64,
    /// Every frame above the top level, outermost first, by frame id: the
    /// names it bound at the declaration.
    frames: Vec<(u64, HashMap<crate::intern::Sym, Binding>)>,
    consts: crate::const_scope::ConstFrames,
}

impl DeclView {
    /// The ids of the frames the declaration saw.
    pub(super) fn frame_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.frames.iter().map(|(id, _)| *id)
    }

    /// Every binding the declaration saw above the top level.
    pub(super) fn bindings(&self) -> impl Iterator<Item = &Binding> {
        self.frames.iter().flat_map(|(_, b)| b.values())
    }
}

/// An enclosing body's frame id and the storage it declares after a nested
/// declaration (see `LowerCtx::decl_later_storage`).
pub(super) type LaterStorage = (u64, Vec<crate::intern::Sym>);

/// What [`LowerCtx::enter_decl_scope`] took aside, for
/// [`LowerCtx::leave_decl_scope`] to restore.
pub(super) struct DeclScope {
    frames: Option<crate::scope::Detached<Binding>>,
    /// How many copies of the declaration's frames sit on top of the stack.
    copies: usize,
    consts: crate::const_scope::ConstFrames,
    chips: Vec<Arc<HashMap<String, Arc<ChipDecl>>>>,
    /// The enclosing frames' constants, swapped for the declaration's.
    own_consts: Option<crate::const_scope::ConstFrames>,
    /// The anonymous chips the declaration is outside of, with the bindings
    /// their storage gave the top level and the names they hid there.
    outside: Option<OutsideChips>,
    /// The anonymous chips the declaration is written in that were not being
    /// lowered: where their shadows start, and each name's binding before.
    inside: Option<InsideChips>,
    /// What [`LowerCtx::top_view`] replaced.
    top: TopView,
}

/// What [`LowerCtx::top_view`] replaced: the outermost frame's binding of
/// each name it set, and the constants' place.
#[derive(Default)]
pub(super) struct TopView {
    bound: Vec<(String, Option<Binding>)>,
    cursor: Option<crate::scope::top::ReadAt>,
}

/// See [`DeclScope::outside`].
/// The shadows [`LowerCtx::anon_chip_shadows`] held before entering a
/// declaration's chips, and each name's binding before them.
type InsideChips = (usize, Vec<(String, Option<Binding>)>);

type OutsideChips = (Vec<AnonChipShadow>, Vec<(String, Binding)>, Vec<crate::const_scope::HiddenAtTop>);

/// What an anonymous chip of the top level left its names bound to and the
/// constants it held under them, for a declaration written in it that is
/// called after it.
pub(super) type ChipNames = (Vec<(String, Binding)>, Vec<crate::const_scope::HiddenAtTop>);

/// An anonymous chip at the top level being lowered: its body, where its
/// storage is what the names it declares mean, and the binding each name it
/// shares with the top level has outside it. A declaration written outside
/// the chip reads those, wherever it is called from.
#[derive(Clone)]
pub(super) struct AnonChipShadow {
    pub(super) body: SourceRange,
    pub(super) outside: Vec<(String, Binding)>,
    /// Where the chip's names start in the constant scope's hidden names.
    pub(super) hidden_mark: usize,
}

/// Whether two bindings are the same declaration. A var's read cache and the
/// like may differ; what the name denotes does not.
pub(super) fn same_binding(a: &Binding, b: &Binding) -> bool {
    match (a, b) {
        (Binding::Var(x), Binding::Var(y)) => x.node_id == y.node_id && x.storage == y.storage,
        (Binding::Local(x), Binding::Local(y)) => x.port == y.port,
        (Binding::Buffer(x), Binding::Buffer(y))
        | (Binding::Input(x), Binding::Input(y))
        | (Binding::Output(x), Binding::Output(y)) => x.node_id == y.node_id,
        (Binding::EventParam(x), Binding::EventParam(y)) => x == y,
        (Binding::DynRef(x), Binding::DynRef(y)) => x.port == y.port,
        (Binding::Chip(x), Binding::Chip(y)) => x.range == y.range,
        (Binding::Namespace(x), Binding::Namespace(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_binding(v, w)))
        }
        (Binding::Record(x), Binding::Record(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_binding(v, w)))
        }
        _ => false,
    }
}

/// One active generic-mod inline: the callee's type-param names and the
/// substitution (`Type::Param -> concrete`) inferred from this call's args.
/// Pushed on `LowerCtx::mono_stack` while the generic mod's body is lowered so
/// that `T`-typed storage/return annotations in the body resolve to the
/// concrete monomorph (`pick<int>` -> int gates) instead of leaking `Type::Param`
/// (or a wrong `Any`/last-combo type) to emit. Nested generic inlines push/pop;
/// the innermost frame (`mono_stack.last()`) governs annotation resolution.
#[derive(Clone, Debug)]
pub(super) struct MonoFrame {
    pub(super) params: Vec<String>,
    pub(super) subst: crate::types::infer::Subst,
    /// The declaration each parameter is declared by: a name is that
    /// parameter only where that declaration writes it.
    pub(super) scopes: HashMap<String, SourceRange>,
}

impl MonoFrame {
    /// A frame for `decl`'s own type parameters `params`, bound by `subst`.
    pub(super) fn of(decl: &ChipDecl, params: Vec<String>, subst: crate::types::infer::Subst) -> Self {
        let scopes = params.iter().map(|p| (p.clone(), decl.range.clone())).collect();
        MonoFrame { params, subst, scopes }
    }

    /// What `name`, written at `at`, stands for when it is one of this
    /// frame's type parameters there: its binding in this instantiation, or
    /// the parameter itself when unbound. A name written outside the
    /// declaration that declares the parameter (in a type alias body, or in
    /// the body of another declaration inlined here) is not that parameter,
    /// whatever its spelling.
    pub(super) fn param_at(&self, name: &str, at: &SourceRange) -> Option<Type> {
        if !self.params.iter().any(|p| p == name) {
            return None;
        }
        let written_there = at.file.is_empty()
            || self.scopes.get(name).is_none_or(|d| {
                d.file == at.file && d.start.offset <= at.start.offset && at.end.offset <= d.end.offset
            });
        written_there.then(|| self.subst.get(name).cloned().unwrap_or_else(|| Type::Param(name.to_string())))
    }
}

/// The tag of `rec`, a value of type `ty`, when `ty` is a payload-free enum:
/// its tag is the whole value, so where one value is expected the enum is its
/// tag. Decided by the type, never by the record's shape.
pub(super) fn enum_tag<'r>(
    ctx: &LowerCtx,
    rec: &'r HashMap<crate::intern::Sym, Binding>,
    ty: &Type,
) -> Option<&'r Binding> {
    if !crate::lower::predeclare::unit_enum_port(ctx, &unwrap_ref(ty)) {
        return None;
    }
    enum_disc(rec)
}

/// What a place takes of a gate read held as its outputs (see
/// [`held_output`]). A field read names its output itself (`k.IsEmpty`).
#[derive(Clone, Copy)]
pub(super) enum OutputPick {
    /// A place for one value: the first output.
    First,
    /// A destructuring by position: every output in declared order, keyed
    /// `0`, `1`, ... as a tuple's positions are.
    Positions,
}

/// Whether a place of type `t` takes one value, so a gate read given to it
/// is its first output: anything but a record, a tuple, a gate read's
/// outputs, a generic parameter or `any`, as typecheck decides it.
pub(super) fn takes_one_value(t: &Type) -> bool {
    !matches!(
        unwrap_ref(t),
        Type::Record(_) | Type::Outputs(_) | Type::Tuple(_) | Type::Any | Type::Opaque | Type::Param(_)
    )
}

/// The value of `e` where a place of type `slot` goes, when `e` names a
/// gate read held as its outputs (`k` after `let k = Edge(b)`, `t.m`,
/// `t.0`) and the place takes one value: its first output (see
/// [`held_output`]). `None` for anything else, read as it is.
pub(super) fn held_first_output(ctx: &LowerCtx, e: &Expr, slot: &Type) -> Option<Binding> {
    if !takes_one_value(slot) || ctx.gate_outputs(e.range()).is_none() {
        return None;
    }
    let Some(Binding::Record(rec)) = crate::lower::access::resolve_field_chain(ctx, e) else {
        return None;
    };
    held_output(ctx, rec, e.range(), OutputPick::First)
}

/// A gate read held as its outputs, by name, as a place takes it: `rec` is
/// the value of the expression at `at`, which typecheck typed as a gate read
/// in this instantiation ([`LowerCtx::gate_outputs`]), and `pick` says which
/// of its outputs the place reads. `None` for anything that is no gate read
/// (a record of the program's own reads no output) or lacks the output.
pub(super) fn held_output(
    ctx: &LowerCtx,
    rec: &HashMap<crate::intern::Sym, Binding>,
    at: &SourceRange,
    pick: OutputPick,
) -> Option<Binding> {
    let outputs = ctx.gate_outputs(at)?;
    let output = |name: &str| rec.get(&crate::intern::intern(name)).cloned();
    match pick {
        OutputPick::First => output(&outputs[0].0),
        OutputPick::Positions => outputs
            .iter()
            .enumerate()
            .map(|(i, (n, _))| Some((crate::intern::intern(&i.to_string()), output(n)?)))
            .collect::<Option<HashMap<_, _>>>()
            .map(Binding::Record),
    }
}

/// The fields of `rec`, the value of the expression at `at`, in the order
/// a place for one value tries them: an enum's tag, then the outputs of a
/// gate read or the fields its type declares, in declared order, then any
/// other key by name. Never the map's own order, so the first of them is
/// the same whatever the names hash to.
pub(super) fn in_declared_order<'r>(
    ctx: &LowerCtx,
    rec: &'r HashMap<crate::intern::Sym, Binding>,
    at: &SourceRange,
) -> Vec<&'r Binding> {
    let declared: Vec<String> = match ctx.gate_outputs(at) {
        Some(outputs) => outputs.iter().map(|(n, _)| n.clone()).collect(),
        None => ctx
            .type_at(at)
            .and_then(|t| unwrap_ref(&t).fields().map(|fs| fs.iter().map(|(n, _)| n.clone()).collect()))
            .unwrap_or_default(),
    };
    let mut keys: Vec<crate::intern::Sym> = Vec::with_capacity(rec.len());
    keys.extend(rec.get_key_value(&crate::intern::intern("__disc")).map(|(k, _)| *k));
    keys.extend(declared.iter().map(|n| crate::intern::intern(n)).filter(|k| rec.contains_key(k)));
    let mut rest: Vec<crate::intern::Sym> = rec.keys().copied().filter(|k| !keys.contains(k)).collect();
    rest.sort_by_key(|k| crate::intern::resolve(*k));
    keys.extend(rest);
    keys.iter().filter_map(|k| rec.get(k)).collect()
}

/// The tag slot of `rec`, an enum value's slots.
pub(super) fn enum_disc(rec: &HashMap<crate::intern::Sym, Binding>) -> Option<&Binding> {
    rec.get(&crate::intern::intern("__disc"))
}

/// Scope key for an `Output` binding. Outputs share scope frames with value
/// bindings (inline-mod MODULE frames must isolate them), but live under a
/// key no identifier can collide with (`:` can't appear in an identifier) so
/// `out X = X` can still read a var/array named `X` — without a distinct key,
/// a same-name output would clobber the var's slot and its init expr would
/// lower to `_Unsupported`.
pub(super) fn output_scope_key(name: &str) -> String {
    format!("out:{name}")
}

pub(super) struct LowerCtx<'a> {
    pub(super) builder: ModuleBuilder,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) type_of_expr: &'a HashMap<(Arc<str>, usize, usize), Type>,
    pub(super) op_resolutions: &'a HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Typecheck's recorded method/container resolutions (see `typecheck::resolution`).
    pub(super) resolutions: &'a crate::typecheck::Resolutions,
    /// The concrete type arguments of every generic body enclosing the code
    /// being lowered, outermost first; empty outside generics. Part of every
    /// resolution key.
    pub(super) resolution_subst: Arc<[Type]>,
    /// Each mod/chip declaration's `resolution_subst` at the point it was
    /// registered, keyed by its source start. Only a declaration nested in a
    /// generic body has an entry; every other one is `[]`.
    pub(super) decl_resolution_subst: HashMap<(Arc<str>, usize), Arc<[Type]>>,
    /// Each mod/chip declaration's enclosing generic frame (`mono_stack.last()`
    /// where it was registered), keyed like `decl_resolution_subst`. A chip
    /// instanced from inside a generic body lowers in a fresh child ctx, so
    /// this is how its body still resolves the outer `T`.
    pub(super) decl_mono_frame: HashMap<(Arc<str>, usize), MonoFrame>,
    /// For a mod/chip declared inside a body (not at the top level): the
    /// enclosing frames as they stood at the declaration, keyed like
    /// `decl_resolution_subst`. Typecheck checks such a body where it is
    /// written, so a name bound later in an enclosing frame is not the
    /// body's. See [`LowerCtx::enter_decl_scope`].
    pub(super) decl_views: HashMap<(Arc<str>, usize), Arc<DeclView>>,
    /// The declarations whose `decl_views` entry records each frame, by frame
    /// id, so a chip build finds the views over the frames it inherits
    /// without scanning them all.
    pub(super) views_by_frame: HashMap<u64, HashSet<(Arc<str>, usize)>>,
    /// The storage names each enclosing mod or chip body declares after the
    /// statement holding a nested declaration, per body frame id, keyed like
    /// `decl_views`. Lowering declares a body's storage before the body runs,
    /// so those names are already in its frame when the nested declaration is
    /// registered.
    pub(super) decl_later_storage: HashMap<(Arc<str>, usize), Vec<LaterStorage>>,
    /// A `_Literal` of an enclosing module a binding in this chip's scope
    /// names, by node id. Emit inlines a literal only into its own module, so
    /// a read of one from here reads a copy made in this module instead (see
    /// `LowerCtx::local_literals`).
    pub(super) foreign_literals: HashMap<NodeId, Arc<crate::ir::Node>>,
    /// The copy made in this module of each foreign literal port, so every
    /// read of one shares a single copy.
    pub(super) literal_copies: HashMap<PortRef, PortRef>,
    /// Body storage declarations the body pre-declare loop left to their
    /// statement, by source position (see `predeclare::hoist_or_defer`). The
    /// frame already binds the name when the statement runs, to the storage
    /// it shadows, so the statement must not take that for its own.
    pub(super) unhoisted: HashSet<(Arc<str>, usize)>,
    /// Set by `compile_chip_template`, which lowers a chip body (generic ones
    /// included) outside any instantiation, so resolution lookups there may
    /// fall back (see `resolution_fallback_allowed`).
    pub(super) template_build: bool,
    /// Inferred custom-event data-slot types — see `LowerInput::ce_slots`.
    pub(super) ce_slots: &'a CeSlotMap,
    pub(super) file: String,
    pub(super) scope: crate::scope::Scope<Binding>,
    pub(super) handler_end_execs: Vec<PortRef>,
    pub(super) current_exec: Option<PortRef>,
    /// How many writes whose target is not known at compile time (through a
    /// runtime ref) this lowering has emitted. A `for` body compares it before
    /// and after, since such a write draws no wire out of the storage it hits.
    pub(super) dyn_ref_writes: u32,
    /// The `Var_Get` each var's storage was last read through on the current
    /// exec chain, by the storage gate's id: a later read on the same chain
    /// reuses it until a write, a branch join or a new chain drops it.
    pub(super) var_gets: HashMap<NodeId, NodeId>,
    /// The chip of `builder.module` each node of an instantiated chip sits in,
    /// at any depth, by the chip's key (see [`find_node`](Self::find_node)).
    pub(super) chip_of: HashMap<NodeId, NodeId>,
    pub(super) handler_entry_exec: Option<PortRef>,
    pub(super) captured_events: HashMap<String, PortRef>,
    pub(super) next_chain_id: u32,
    pub(super) next_scope_id: ScopeId,
    /// When inside an anonymous chip body, nodes get tagged with this
    /// chip_id so the emitter routes them to a child grid.
    pub(super) current_anon_chip: Option<NodeId>,
    /// Source range of every `for` body currently being lowered, innermost
    /// last. A statement inside one was written inside the loop, so typecheck
    /// saw the `for` around it; one that arrived by inlining lies outside.
    pub(super) loop_body_ranges: Vec<SourceRange>,
    /// Anonymous-chip Chip nodes created by `pre_declare_anon_chip`, keyed
    /// by (decl range, enclosing chip at predeclare time) so
    /// `lower_anon_chip` can pair a decl with its node without scanning
    /// every module node.
    pub(super) anon_chip_nodes: crate::collections::HashMap<(SourceRange, Option<NodeId>), NodeId>,
    /// The storage each anonymous chip's body declares, by its Chip node:
    /// what its statements name, whatever the scope it shares bound to the
    /// name before the chip.
    pub(super) anon_chip_storage: crate::collections::HashMap<NodeId, Vec<(String, Binding)>>,
    /// Each anonymous chip at the top level being lowered, outermost first
    /// (see [`AnonChipShadow`]).
    pub(super) anon_chip_shadows: Vec<AnonChipShadow>,
    /// What each anonymous chip of the top level, by its range, left its
    /// names meaning once lowered (see [`ChipNames`]).
    pub(super) anon_chip_names: crate::collections::HashMap<SourceRange, ChipNames>,
    /// What each free name means at the program's top level.
    pub(super) top_names: Arc<crate::scope::top::TopNames>,
    /// The binding of each declaration of a name declared more than once at
    /// the top level of a file, by its file, start offset and the name, once
    /// lowered (see [`top_view`](LowerCtx::top_view)).
    pub(super) top_dup_bindings: Arc<HashMap<(Arc<str>, usize, String), Binding>>,
    /// Accumulated exec from `return` statements inside an inlined mod.
    /// Each return merges into this via chained union gates.
    pub(super) mod_return_exec: Option<PortRef>,
    /// For mods with a single output: the PseudoVar node that holds the
    /// return value. Each `return expr` writes to this var via Var_Set.
    pub(super) mod_return_var: Option<VarRecord>,
    /// For a multi-return mod whose single output is an ENUM (record-shaped):
    /// the superset per-slot storage record the return value lives in. Each
    /// `return <enum>` stores its slots here (so the caller reads the
    /// runtime-selected branch's value, not a leaked construction record), and
    /// the caller binds this record. The record analog of `mod_return_var`; the
    /// two are mutually exclusive (a given output is scalar OR enum-shaped).
    pub(super) mod_return_record: Option<HashMap<crate::intern::Sym, Binding>>,
    /// The output names of the mod being inlined, in declaration order, which
    /// a tuple `return (..)` fills by position.
    pub(super) mod_output_names: Vec<String>,
    /// The declared type of each output of the mod being inlined, in this
    /// instantiation; `exec` for an exec output.
    pub(super) mod_output_types: HashMap<String, Type>,
    /// The signature outputs of the chip whose body is being built, in
    /// declaration order, each with whether it is an exec output. Set only for
    /// a chip with several outputs, whose `return` fills them by position or
    /// by name.
    pub(super) chip_return_outputs: Vec<(String, bool)>,
    /// The outputs of the mod being inlined whose `emit name = v` writes go
    /// to the return storage (`mod_return_var`/`mod_return_record`) rather
    /// than straight to the output: set when an output is written in more
    /// than one place, so the write on the path that ran is the one read.
    pub(super) mod_emit_storage: crate::collections::HashSet<String>,
    /// The stored outputs of the mod being inlined whose writes cannot run in
    /// this call: it is a pure call, and they are written on the body's own
    /// path (or by a `return`). Their writes are skipped, and reading one is
    /// WS007 (`unsequenced_reads`).
    pub(super) mod_unsequenced: crate::collections::HashSet<String>,
    /// Outputs a handler, branch or anonymous chip of this module sets with
    /// `emit name = v` (not `out`), so a diagnostic can name the write.
    pub(super) emitted_outputs: crate::collections::HashSet<String>,
    /// How many sites of this module write each of its outputs a value
    /// (`output_write_counts`, plus a top-level default).
    pub(super) output_write_sites: HashMap<String, usize>,
    /// Per `import * as` namespace, the value-writing sites of each output it
    /// declares (`output_write_counts`), for its backing-var decision.
    pub(super) namespace_output_writes:
        Arc<HashMap<super::DeclScope, HashMap<String, (usize, bool)>>>,
    /// The range of a call whose result its consumer binds as a record
    /// (`let t = f()`), so the call need not also read its first output as
    /// a single value.
    pub(super) record_result_call: Option<SourceRange>,
    /// The scratch var a value `match` with block arms stores its value into:
    /// each value arm writes it on the arm's own chain (`lower_match_value`).
    pub(super) match_value_store: Option<VarRecord>,
    /// The string patterns lowered for the `match` being lowered, by tested
    /// value and parts: the condition and each hole, so an arm's captures read
    /// the same gates its test built. Taken aside around each `match`.
    pub(super) concat_cache: Vec<crate::lower::pattern_test::ConcatLowered>,
    /// How many `for` loop bodies enclose the code being lowered, counting a
    /// loop an inlined `mod` brings in.
    pub(super) loop_depth: u32,
    /// Storage vars of such outputs, by node: the output name, the call's
    /// range and a per-call id. The first read reports WS007 once for the
    /// call and drops the call's other entries.
    pub(super) unsequenced_reads: HashMap<NodeId, (String, SourceRange, usize)>,
    /// The next id for `unsequenced_reads`.
    pub(super) unsequenced_calls: usize,
    /// Type alias map: `Name -> TypeExpr::Record { ... }` for dissolving
    /// record params at chip boundaries.
    pub(super) type_aliases: HashMap<String, crate::ast::TypeExpr>,
    /// Enum registry: `Name -> EnumDef` (variants, each with its assigned
    /// discriminant and payload shape). Mirrors `typecheck::TypeCheckCtx::enum_defs`,
    /// seeded once up front from the whole program by
    /// `typecheck::enums::build_registry` (see `lower::lower`), a whole-file
    /// pre-pass, so a use resolves regardless of where in the file the `enum`
    /// itself is declared.
    ///
    /// **Enum value layout** (the "Shared representation" contract every
    /// enum-consuming site in this module follows): an enum value lowers to a
    /// `Binding::Record` (interned-`Sym`-keyed) whose fields are `__disc` - an
    /// int binding, a `Literal::Int` for a statically-known variant or a
    /// `Pseudo_Var`-backed `Binding::Var` (read via `Var_Get`) for a
    /// stored/merged one - plus one payload slot per (variant, field):
    /// positional field `i` of variant `V` -> interned `__{V}_{i}`; named field
    /// `f` of variant `V` -> `__{V}_{f}`. A nested-enum payload slot is itself a
    /// `Binding::Record`. A freshly constructed KNOWN variant binds only
    /// `__disc` plus its own slots; a stored enum `var` allocates the SUPERSET
    /// - `__disc` plus every variant's slots - so a merge from any branch
    /// writes into the same storage gates.
    pub(super) enum_defs: std::sync::Arc<crate::typecheck::enums::EnumRegistry>,
    /// Pending emit exec paths per output name, each tagged with the exec
    /// chain (handler) it was emitted on. Accumulated during lowering, flushed
    /// to union chains at the end so each output gets one wire. The chain tag
    /// lets flush route same-chain emits through an `await`'s arm (sequenced
    /// before the hub — a parallel arm races the awaiting `Var_Get`).
    pub(super) pending_emits: HashMap<String, Vec<(PortRef, Option<u32>)>>,
    /// Outputs value-driven from more than one `emit out = v` site get a backing
    /// PseudoVar (name -> its var record): each site does a `Var_Set` into it and
    /// the var's value feeds the output once (no fan-in). Populated by a pre-scan
    /// before handler lowering; empty for single-site outputs and chip bodies.
    pub(super) output_backing_vars: HashMap<String, VarRecord>,
    /// A record output's backing storage, one var per scalar field: the
    /// record counterpart of `output_backing_vars`, used by chip bodies.
    pub(super) output_record_backing: HashMap<String, HashMap<crate::intern::Sym, Binding>>,
    /// Local `let x: exec` signals declared with a stable Union "hub" gate,
    /// keyed by a per-declaration unique key (`name#hubId`) — NOT the bare
    /// name, so two mods/handlers declaring the same signal name get separate
    /// signals. `on x` triggers off the hub's `ExecOut` via the scope binding;
    /// at flush the union of every `emit x` is wired into the hub. The hub
    /// gives a forward-referenceable trigger port so `on x` works regardless
    /// of whether it appears before or after the emits in source.
    pub(super) exec_signal_hubs: HashMap<String, NodeId>,
    /// Reverse map: hub node -> its unique signal key. Emit/await sites resolve
    /// a surface name to its key through the *scope* (name -> hub port -> key),
    /// so shadowed / same-named signals in different bodies stay distinct.
    pub(super) exec_signal_keys: HashMap<NodeId, String>,
    /// The exec `if` depth (`exec_branch_depth`) each local exec signal was
    /// declared at, by signal key.
    pub(super) exec_signal_depths: HashMap<String, usize>,
    /// Each `await` inside an `if` whose signal is declared outside that
    /// branch, so emits do not re-arm it: its armed flag, chain and range, by
    /// signal key. An emit of the signal after it on the same chain is a
    /// back-edge that finds it unarmed (WS118).
    pub(super) branch_awaits: HashMap<String, Vec<(NodeId, Option<u32>, SourceRange)>>,
    /// Inside `await`, the armed flag's Value port. `_` in the exec expression
    /// resolves to this, allowing `await Sleep(_, 1.0)` to wire the armed flag
    /// as Sleep's input.
    pub(super) await_armed_port: Option<PortRef>,
    /// Unconditional `await <signal>` per local exec signal: the armed flag's
    /// var node and the chain the await sits on. At flush, emits of the signal
    /// from the *same* chain are routed through a `Var_Set(armed = true)` into
    /// the hub — sequencing the arm before the union so the awaiting `Var_Get`
    /// can't race it (and so loop back-edges re-arm each iteration). Emits from
    /// other chains stay direct, guarded by the flag. Only awaits at branch
    /// depth 0 register here: a conditional `await` (inside `if`) keeps pure
    /// flag semantics, since its arm must not fire for the untaken branch.
    pub(super) signal_awaits: HashMap<String, (NodeId, Option<u32>)>,
    /// Depth of enclosing exec `if` branches; >0 means conditionally executed.
    pub(super) exec_branch_depth: usize,
    /// Hidden payload stores per local exec signal: `(field, store var, type)`.
    /// `emit sig = expr` writes the store(s) on the emit chain (before any
    /// buffer), and `await sig` reads them back on the resumed chain — the
    /// value crosses the tick through the persistent var, not the buffer.
    /// A scalar payload uses one entry with field `""`; a record payload gets
    /// one entry per field.
    pub(super) exec_signal_payloads: HashMap<String, Vec<(String, NodeId, Type)>>,
    /// Whether lowering is inside an imperative body (a handler, or a block
    /// nested in one) rather than the top-level decl list. `current_exec` is
    /// `Some` both there and in a top-level chip that inherits a preceding
    /// handler's leak, so it cannot tell the two apart on its own.
    pub(super) in_handler_body: bool,
    /// Pre-compiled template cache for standalone chip instances.
    pub(super) template_cache: Arc<crate::template_cache::TemplateCache>,
    /// Field->source-port record produced by the most recent multi-output inline
    /// mod call. Its internal output nodes are removed, so `let s = mod(...)`
    /// consumes this to bind `s` as a record (`s.field` reads the source port).
    pub(super) pending_inline_record: Option<HashMap<crate::intern::Sym, Binding>>,
    /// The port `value_record_fields` got from lowering a record-SHAPED value
    /// that turned out to carry no field map (its `may_produce_record`
    /// fallback), with the range of the expression it lowered. A reader takes
    /// it only for that same expression (see `take_value_record_port`), so a
    /// nested probe's port (the `obj` of `obj.Value`) is never reused for the
    /// outer expression.
    pub(super) last_value_record_port: Option<(SourceRange, PortRef)>,
    /// Incoming wires per node, for walking a pure value's cone (see
    /// `ordered`).
    pub(super) cone_index: std::cell::RefCell<crate::lower::ordered::ConeIndex>,
    /// How many `emit`s of a local exec signal this lowering has queued; the
    /// handlers they fire run off the chain the emit sat on (see `ordered`).
    pub(super) signal_emits: u32,
    /// Each queued emit as its `pending_emits` key and index, in the order
    /// they were queued.
    pub(super) emit_log: Vec<(String, usize)>,
    /// The first `emit` statement queued for each `pending_emits` key, for a
    /// diagnostic about it at flush.
    pub(super) emit_sites: HashMap<String, SourceRange>,
    /// `import * as` namespaces keyed by the FILE that wrote the import, then by
    /// alias. An alias is file-local, so two modules may each write
    /// `import * as Other`; `scope` keeps one binding per name, and this is what
    /// lets a module's body reach its OWN alias (see `ns_mod_scopes`).
    pub(super) ns_by_file:
        HashMap<std::sync::Arc<str>, HashMap<String, Binding>>,
    /// Field->binding map stashed by a `return { ... }` record literal while an
    /// inline mod body is being lowered. A record literal is not a standalone
    /// expression, and `-> { a, b }` is a single record-typed output, so the
    /// call consumes this (instead of the removed output node) to bind the
    /// caller's record. Set only by a record-literal return; taken by the call.
    pub(super) pending_return_record: Option<HashMap<crate::intern::Sym, Binding>>,
    /// Output name -> the record bound to it by `out <name> = <record>` inside a
    /// mod body. A record has no single value port, so wiring it into the
    /// output's `RerInput` lowered to an `_Unsupported` placeholder that read a
    /// default. The inline-call machinery consults this to hand the caller the
    /// real field map instead (see `lower_out_binding` and the record assembly
    /// at the end of `lower_chip_call_inline`). Scoped to one inline expansion:
    /// swapped out and restored around the callee body, exactly like the
    /// constants' frames, so a nested call cannot leak into its caller.
    pub(super) pending_out_records: HashMap<String, HashMap<crate::intern::Sym, Binding>>,
    /// Source ranges of chips/mods whose bodies are being lowered on the current
    /// call path (child contexts inherit a copy). Every call is expanded into the
    /// wire graph at compile time, so a body that (transitively) calls itself
    /// would rebuild forever — `lower_chip_call` checks this stack and emits
    /// WS020 instead of recursing. Keyed on the decl's range (unique per decl,
    /// includes the file), NOT its name, so two distinct same-named mods — e.g. a
    /// local `drawCard` and one imported from another module — aren't conflated.
    /// Each entry also says whether its body is open yet: its arguments lower
    /// first, at the call site, so a call made while they do (`inc(inc(k))`,
    /// or `f(h(k))` where `h` calls `f`) is the caller's. Only an open body
    /// calling its own decl is a cycle.
    pub(super) chip_call_stack: Vec<(crate::diagnostic::SourceRange, bool)>,
    /// Every chip/mod name declared anywhere in the (import-merged) program.
    /// Chips/mods are registered into scope in source order, so a call to one
    /// not yet registered fails to resolve. Distinguishing "declared later"
    /// (a use-before-declaration -> WS021 error) from "not a function at all"
    /// (e.g. an unimplemented builtin -> placeholder) needs the full name set.
    /// Shared (Arc) so child contexts clone it cheaply.
    pub(super) known_fn_names: std::sync::Arc<crate::collections::HashSet<String>>,
    /// Which imported declarations each file named (see
    /// [`crate::ast::ImportScope`]). Shared (Arc) like `known_fn_names`.
    pub(super) imports: std::sync::Arc<crate::ast::ImportScope>,
    /// Every name a top-level `let` binds. A mod or chip body reads such a
    /// name where it is called, so a call above the declaration finds nothing
    /// bound; this set tells that use-before-declaration (WS021) from a name
    /// that is not a `let` at all.
    pub(super) known_let_names: std::sync::Arc<crate::collections::HashSet<String>>,
    /// The compile-time constants: the top-level `let`/`const` values of the
    /// (import-merged) program (so a `var` / `var … : T[]` initializer can
    /// name one, `1 << C_FLAG`, instead of restating its value), and those of
    /// each open scope frame (a body-local `let` of a constant, a `const`, a
    /// `const` parameter). Mirrors `typecheck::TypeCheckCtx::consts`. Read
    /// only through [`const_lookup`](Self::const_lookup) and its siblings,
    /// which resolve a name through the scope.
    pub(super) consts: crate::const_scope::ConstScope,
    /// Container gates that must never be mutated: the `Pseudo_ArrayVar` /
    /// `Pseudo_MapVar` nodes
    /// [`materialize_const_container`](super::predeclare::materialize_const_container)
    /// builds for a `const` array/map.
    ///
    /// A `const` container is BOTH a compile-time value and a runtime
    /// container, and those two are only one source of truth while nothing
    /// changes the runtime copy — `const n = xs.length()` folding to 3 while
    /// the gate holds 4 elements is the divergence this set exists to prevent.
    ///
    /// Keyed on the NODE, not the name, so the rule survives aliasing: passing
    /// a `const` table to a `ys: T[]` parameter binds the callee's `ys` to this
    /// same node, and a name-keyed check would see only the innocent-looking
    /// `ys` and let the mutation through.
    ///
    /// Not shared with a child context: a microchip body gets a fresh `Scope`,
    /// so its materializations are its own. Node ids need no such care -- they
    /// come from `NodeId`'s per-compile counter, which is unique across the
    /// whole compile.
    pub(super) immutable_containers: crate::collections::HashSet<NodeId>,
    /// The output nodes of the inline mod bodies being lowered. They are
    /// internal (removed once the call binds its result), so a value wired
    /// into one is handed to the caller, not stored.
    pub(super) inline_outputs: crate::collections::HashSet<NodeId>,
    /// True only for the compiled entry file's root LowerCtx. `@side` port
    /// annotations are legal only there (WS023 elsewhere).
    pub(super) is_root_module: bool,
    /// `///` doc comments keyed by the declaration's source start offset.
    /// Consumed when stamping `DOC_TEXT` onto chip nodes.
    pub(super) doc_comments: &'a crate::parser::DocComments,
    /// Depth of enclosing `@nofold` declaration subtrees. >0 means every gate
    /// created by `add_gate`/`add_event` gets stamped with the `_nofold`
    /// pseudo-property. Incremented/decremented around the lowering of each
    /// `@nofold`-annotated `let`/`out`/`var`/`chip`/`on` declaration.
    pub(super) nofold_depth: u32,
    /// Stack of active generic-mod inlines (see [`MonoFrame`]). Empty at the
    /// top level and inside every non-generic body, where `resolve_local_type`
    /// binds no type parameter; only a generic mod's own body sees a
    /// non-empty stack.
    pub(super) mono_stack: Vec<MonoFrame>,
    /// Per `mod`/`chip` declaration (by address): the trigger of an `on`
    /// handler in its body that acts, if any. See
    /// `expr::warn_handlers_ungated_by_arm`.
    pub(super) acting_handler_memo: std::cell::RefCell<HashMap<usize, Option<String>>>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped (`lower_if`'s const-elision path) — never lowered, so no gate
    /// exists for them at all. Mirrors `typecheck::TypeCheckCtx`'s own
    /// `dropped_ranges`; the two MUST agree on exactly which ranges they
    /// drop (see `typecheck_and_lowering_drop_exactly_the_same_ranges` in
    /// `typecheck::tests`) — a disagreement means code gets type-checked but
    /// not lowered, or lowered without ever being checked.
    pub(super) dropped_ranges: Vec<SourceRange>,
    /// Chip/mod declarations visible to the BAKE path, as a FRAME STACK
    /// mirroring `scope` and the constants' frames 1:1: every `push_scope`/`pop_scope`
    /// pair pushes and drops a frame here too. Populated in source order by
    /// `pre_declare_chip_name` as each pre-declare loop reaches a declaration,
    /// and consulted ONLY by `array_elem_literal`/`map_entry_literal` (via
    /// [`resolve_mod_pass1`](Self::resolve_mod_pass1)) when baking a
    /// var/array/map initializer's `const mod` CALL.
    ///
    /// **Why a frame stack and not a flat map.** A nested declaration must win
    /// inside the body that declares it and be gone outside it. As a flat map
    /// that required a manual save/restore around every borrowed-ctx body,
    /// which is both easy to forget and easy to place wrongly — the restore
    /// sat right after pre-declaration rather than after the body was lowered,
    /// so a chip declared and INSTANTIATED inside an inlined mod cloned an
    /// already-restored map and silently resolved the outer declaration while
    /// the ordinary call path in the same chip resolved the inner one. Tying
    /// the lifetime to the scope that already exists removes the discipline
    /// (and that whole bug class) entirely.
    ///
    /// **Why not `ctx.scope` itself.** That is what the ordinary call-lowering
    /// path (`resolve_mod`) and its use-before-declaration guard (WS021,
    /// `lower/call/dispatch.rs`) read. Pass 1 runs to completion before pass 2
    /// starts, so a chip registered into `scope` during pass 1 would be visible
    /// to EVERY pass-2 call site regardless of source order, silently defeating
    /// WS021 (see `lower::tests::chip::use_before_declaration_is_ws021`).
    /// Keeping a separate stack preserves "only what's declared earlier"
    /// for the bake without leaking early visibility into pass 2.
    ///
    /// Always non-empty: the base frame holds the module's top-level
    /// declarations.
    /// Frames are `Arc`-shared: a chip instantiation snapshots the whole
    /// stack by refcount bump (`build_chip_module`), and a shared frame is
    /// only deep-cloned if something actually inserts into it
    /// (`Arc::make_mut` in `pre_declare_chip_name`).
    pub(super) pass1_chips: Vec<std::sync::Arc<HashMap<String, std::sync::Arc<ChipDecl>>>>,
    /// Bare names the importing module itself owns — snapshotted from `scope`
    /// after pass-1 pre-declaration (its `in`/`out`/`var`/`array`/… ), before
    /// any `import * as` namespace lowers its members in pass 2. A namespaced
    /// `let` member must not re-register (clobber) one of these bare names, or
    /// e.g. an imported `let start` would overwrite the importer's own
    /// `in start: exec` and silently drop `on start`'s handler body.
    pub(super) importer_names: crate::collections::HashSet<String>,
    /// For a mod DECLARED inside an `import * as ns` module: its owning
    /// namespace's member map, keyed by the mod's decl location `(file,
    /// offset)`. A namespaced mod is registered by BARE name and inlined at its
    /// call site, where the ambient scope holds whatever namespace lowered last
    /// — so its body's references to its own module's siblings/vars/consts
    /// resolved to the WRONG module (two modules with a same-named `helper`: the
    /// second's callers ran the first's code; a mod mutating `g` wrote the
    /// last-imported namespace's gate). At inline, the callee's members are
    /// pushed into its body frame FIRST (params still shadow), so bare `helper` /
    /// `g` resolve to THIS module's. Keyed by location, so both the `ns.f`
    /// access and a bare sibling call (distinct `Arc<ChipDecl>` clones of the
    /// same source) map to one entry.
    pub(super) ns_mod_scopes:
        crate::collections::HashMap<(Arc<str>, usize), std::sync::Arc<HashMap<String, Binding>>>,
    /// The top-level constants of the module each `ns_mod_scopes` entry's
    /// members come from, recorded in the body frame beside them so a member
    /// constant reads as the constant it is there, as typecheck has it.
    pub(super) ns_mod_consts: crate::collections::HashMap<(Arc<str>, usize), crate::const_scope::MemberConsts>,
    /// Source-location dedup for STATE reached through multiple imports of one
    /// file — plain + `import * as`, `import {g}` + `import {g as h}`, or a
    /// diamond. Keyed by a decl's `(file, start.offset)`: the same SOURCE
    /// declaration however it was imported. The FIRST import to lower it
    /// creates the gate and records it here; a later import binds its
    /// (possibly aliased or namespaced) name to that same gate rather than a
    /// duplicate. A DIFFERENT file's same-named decl has a distinct key, so
    /// genuine collisions still get their own gate. See N2/N3.
    pub(super) import_state_dedup: crate::collections::HashMap<(String, usize), Binding>,
    /// Companion of `import_state_dedup` for BEHAVIOUR decls (handlers, anon
    /// chips) that carry no reusable binding: the set of source locations
    /// already lowered, so the same `on` handler pulled in through several
    /// imports of one file installs its behaviour ONCE instead of firing N
    /// times (which, on shared state, N-counts every increment).
    pub(super) import_behavior_lowered: crate::collections::HashSet<(String, usize)>,
    /// Set while lowering a statement already flagged unreachable (WS081) by
    /// a preceding `break` in the same block. `lower_assign`'s WS058 check
    /// reads this to stay quiet for that one statement.
    pub(super) dead_after_break: bool,
    /// Set when a block's trailing `buffer emit` ended the exec chain, and
    /// cleared by the next statement that has a chain again. `lower_assign`'s
    /// WS058 reads it to name that emit as the cause.
    pub(super) exec_closed_by_buffer_emit: bool,
    /// Each `fn` lowered so far, keyed by its declaration's (file, offset):
    /// the one definition plane every call to it binds.
    pub(super) fn_planes: HashMap<(std::sync::Arc<str>, usize), crate::lower::function::FnPlane>,
    /// The `fn`s whose bodies are being lowered, innermost last: a call to
    /// one of these is recursion, which the game does not run.
    pub(super) fn_building: Vec<(std::sync::Arc<str>, usize)>,
}

/// An exec chain driven by either `a` or `b`: an `Exec_Union` of the two
/// (`a` on `ExecA`), or whichever one there is.
pub(super) fn merge_exec(ctx: &mut LowerCtx, a: Option<PortRef>, b: Option<PortRef>, range: &SourceRange) -> Option<PortRef> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let union = ctx.add_gate(AddNodeOpts {
                gate_class: gc::UNION,
                source_range: range.clone(),
                ports: GateIO::exec_join(),
                ..Default::default()
            });
            ctx.connect(a, union.port(WirePort::ExecA));
            ctx.connect(b, union.port(WirePort::ExecB));
            Some(union.port(WirePort::ExecOut))
        }
        (a, b) => a.or(b),
    }
}

/// The var a mod with several `return`s stores its one `out_type` value into:
/// each `return` sets it and [`read_return_value`] reads it once they merge.
pub(super) fn return_value_var(ctx: &mut LowerCtx, out_type: Type, range: &SourceRange) -> VarRecord {
    let node_id = make_scalar_var_gate(ctx, &out_type, HashMap::default(), range, Some("ret_val"));
    VarRecord::new(node_id, out_type, VarStorage::Var)
}

/// Read `ret_var` on `exec`, after a mod's returns merge, and with
/// `drive_output`, wire the value into the mod's output when it has exactly
/// one. Returns the value.
pub(super) fn read_return_value(ctx: &mut LowerCtx, ret_var: &VarRecord, exec: PortRef, drive_output: bool) -> PortRef {
    let var_ref = ret_var.node_id.port(WirePort::VarRef);
    let get_id = chain_var_read(
        ctx,
        exec,
        var_ref,
        ret_var.inner_type.clone(),
        SourceRange::default(),
        HashMap::default(),
        Some("ret_get"),
    );
    let value = get_id.port(WirePort::Value);
    if drive_output && ctx.output_count() == 1 {
        let out = ctx.first_output().unwrap().1.clone();
        ctx.connect(value, out.node_id.port(WirePort::RerInput));
    }
    value
}

/// Add the exec gate `opts`, drive its `Exec` from `exec`, wire each
/// `(source, input port)` into it in order, and move the exec chain on to its
/// `ExecOut`.
pub(super) fn chain_gate(ctx: &mut LowerCtx, opts: AddNodeOpts, exec: PortRef, inputs: &[(PortRef, WirePort)]) -> NodeId {
    let id = ctx.add_gate(opts);
    ctx.connect(exec, id.port(WirePort::Exec));
    for &(source, port) in inputs {
        ctx.connect(source, id.port(port));
    }
    ctx.current_exec = Some(id.port(WirePort::ExecOut));
    id
}

/// A `Var_Set` (with `increment`, a `Var_Increment` adding `value`) storing
/// `value` into the var of type `inner` behind `var_ref`, chained after
/// `exec`.
#[allow(clippy::too_many_arguments)]
pub(super) fn chain_var_write(
    ctx: &mut LowerCtx,
    exec: PortRef,
    var_ref: PortRef,
    inner: Type,
    value: PortRef,
    increment: bool,
    range: SourceRange,
    note: Option<&'static str>,
) -> NodeId {
    let opts = AddNodeOpts {
        gate_class: if increment { gc::VAR_INCREMENT } else { gc::VAR_SET },
        source_range: range,
        ports: GateIO::var_write(inner),
        note,
        ..Default::default()
    };
    chain_gate(ctx, opts, exec, &[(var_ref, WirePort::VarRef), (value, WirePort::Value)])
}

/// A `Var_Get` reading the var of type `inner` behind `var_ref`, chained after
/// `exec`; `properties` seed its data. The value is on its `Value` port.
pub(super) fn chain_var_read(
    ctx: &mut LowerCtx,
    exec: PortRef,
    var_ref: PortRef,
    inner: Type,
    range: SourceRange,
    properties: HashMap<crate::intern::Sym, Literal>,
    note: Option<&'static str>,
) -> NodeId {
    let opts =
        AddNodeOpts { gate_class: gc::VAR_GET, source_range: range, properties, ports: GateIO::var_read(inner), note, ..Default::default() };
    chain_gate(ctx, opts, exec, &[(var_ref, WirePort::VarRef)])
}

/// Chain an exec-driven `Var_Get` onto the current exec chain: the gate reads
/// `var_rec` through its ref port, and `ctx.current_exec` advances to the
/// gate's `ExecOut`. Callers read the value off the returned node's `Value`
/// port.
///
/// `properties` seeds the gate's data struct, which is how a read gets a
/// sensible value before its var is first written; the deref path
/// (`*x`) deliberately passes none.
pub(super) fn chain_var_get(
    ctx: &mut LowerCtx,
    var_rec: &VarRecord,
    exec: PortRef,
    range: &SourceRange,
    properties: HashMap<crate::intern::Sym, Literal>,
    note: Option<&'static str>,
) -> NodeId {
    let var_ref = var_rec.node_id.port(WirePort::VarRef);
    chain_var_read(ctx, exec, var_ref, var_rec.inner_type.clone(), range.clone(), properties, note)
}

/// `chain_var_get` for a ref that arrives on a wire: a `Var_Get` whose `VarRef`
/// comes from `ref_port`. Never cached, since what `ref_port` names can change
/// between two reads.
pub(super) fn chain_var_get_through(
    ctx: &mut LowerCtx,
    ref_port: PortRef,
    inner: &Type,
    exec: PortRef,
    range: &SourceRange,
) -> NodeId {
    chain_var_read(ctx, exec, ref_port, inner.clone(), range.clone(), HashMap::default(), Some("get through ref"))
}

/// Write `value` through a ref that arrives on a wire: a `Var_Set`, or with
/// `increment` a `Var_Increment` adding `value`, whose `VarRef` comes from
/// `ref_port`. A no-op with no exec chain, like `set_scalar_var`.
pub(super) fn write_through_ref(
    ctx: &mut LowerCtx,
    ref_port: PortRef,
    inner: &Type,
    value: PortRef,
    increment: bool,
    range: &SourceRange,
) {
    let Some(exec_in) = ctx.current_exec else {
        return;
    };
    chain_var_write(ctx, exec_in, ref_port, inner.clone(), value, increment, range.clone(), Some("set through ref"));
    note_dynamic_write(ctx);
}

/// A write whose target could be any var of its type: every cached read is
/// dropped, so a later read cannot reuse a value this write may have changed.
pub(super) fn note_dynamic_write(ctx: &mut LowerCtx) {
    ctx.dyn_ref_writes += 1;
    reset_var_get_caches(ctx);
}

/// The `Value` a fresh `Var_Get` carries in its data struct: the type's zero,
/// so a read that beats its var's first write reads that rather than whatever
/// the gate was serialized with.
pub(super) fn var_get_seed_props(inner: &Type) -> HashMap<crate::intern::Sym, Literal> {
    let mut props = HashMap::default();
    if let Some(lit) = default_literal_for_var_type(inner) {
        props.insert(*sym::VALUE, lit);
    }
    props
}

impl<'a> LowerCtx<'a> {
    /// A context with every accumulator empty and every cursor at its start.
    /// The three places that build a `LowerCtx` - the whole-module entry, the
    /// resource-estimation template, and a chip body's child module - fill in
    /// what they need with struct-update syntax (`..LowerCtx::empty(...)`), so
    /// a new field gets its zero in ONE place instead of needing all three
    /// edited in lockstep.
    ///
    /// The six arguments are the ones that have no meaningful zero: a
    /// `LowerCtx` cannot exist without a module to build into or the
    /// typechecker tables to read.
    pub(super) fn empty(
        builder: ModuleBuilder,
        type_of_expr: &'a HashMap<(Arc<str>, usize, usize), Type>,
        op_resolutions: &'a HashMap<(Arc<str>, usize, usize), OpRule>,
        resolutions: &'a crate::typecheck::Resolutions,
        ce_slots: &'a CeSlotMap,
        doc_comments: &'a crate::parser::DocComments,
    ) -> Self {
        Self {
            builder,
            type_of_expr,
            op_resolutions,
            resolutions,
            resolution_subst: Arc::default(),
            decl_resolution_subst: HashMap::default(),
            decl_mono_frame: HashMap::default(),
            decl_views: HashMap::default(),
            views_by_frame: HashMap::default(),
            decl_later_storage: HashMap::default(),
            foreign_literals: HashMap::default(),
            literal_copies: HashMap::default(),
            unhoisted: HashSet::default(),
            template_build: false,
            ce_slots,
            doc_comments,
            diagnostics: Vec::new(),
            file: String::new(),
            scope: crate::scope::Scope::new(),
            handler_end_execs: Vec::new(),
            current_exec: None,
            dyn_ref_writes: 0,
            var_gets: HashMap::default(),
            chip_of: HashMap::default(),
            handler_entry_exec: None,
            captured_events: HashMap::default(),
            next_chain_id: 0,
            next_scope_id: ROOT_SCOPE_ID + 1,
            current_anon_chip: None,
            loop_body_ranges: Vec::new(),
            anon_chip_nodes: crate::collections::HashMap::default(),
            anon_chip_storage: crate::collections::HashMap::default(),
            anon_chip_shadows: Vec::new(),
            anon_chip_names: crate::collections::HashMap::default(),
            top_names: Arc::default(),
            top_dup_bindings: Arc::default(),
            mod_return_exec: None,
            mod_return_var: None,
            mod_return_record: None,
            mod_output_names: Vec::new(),
            mod_output_types: HashMap::default(),
            chip_return_outputs: Vec::new(),
            mod_emit_storage: crate::collections::HashSet::default(),
            mod_unsequenced: crate::collections::HashSet::default(),
            emitted_outputs: crate::collections::HashSet::default(),
            output_write_sites: HashMap::default(),
            namespace_output_writes: Arc::new(HashMap::default()),
            record_result_call: None,
            match_value_store: None,
            concat_cache: Vec::new(),
            loop_depth: 0,
            unsequenced_reads: HashMap::default(),
            unsequenced_calls: 0,
            type_aliases: HashMap::default(),
            enum_defs: Arc::default(),
            pending_emits: HashMap::default(),
            output_backing_vars: HashMap::default(),
            output_record_backing: HashMap::default(),
            exec_signal_hubs: HashMap::default(),
            exec_signal_keys: HashMap::default(),
            exec_signal_depths: HashMap::default(),
            branch_awaits: HashMap::default(),
            await_armed_port: None,
            signal_awaits: HashMap::default(),
            exec_branch_depth: 0,
            exec_signal_payloads: HashMap::default(),
            in_handler_body: false,
            template_cache: Arc::new(crate::template_cache::TemplateCache::new()),
            pending_inline_record: None,
            last_value_record_port: None,
            cone_index: Default::default(),
            signal_emits: 0,
            emit_log: Vec::new(),
            emit_sites: HashMap::default(),
            ns_by_file: HashMap::default(),
            pending_return_record: None,
            pending_out_records: HashMap::default(),
            chip_call_stack: Vec::new(),
            known_fn_names: Arc::new(crate::collections::HashSet::default()),
            imports: Arc::default(),
            known_let_names: Arc::new(crate::collections::HashSet::default()),
            consts: crate::const_scope::ConstScope::default(),
            immutable_containers: crate::collections::HashSet::default(),
            inline_outputs: crate::collections::HashSet::default(),
            is_root_module: false,
            nofold_depth: 0,
            mono_stack: Vec::new(),
            acting_handler_memo: std::cell::RefCell::new(HashMap::default()),
            dropped_ranges: Vec::new(),
            // Base frame: the module's own top-level declarations.
            pass1_chips: vec![Arc::new(HashMap::default())],
            // Populated from `scope` right after pass 1 (top-level module only;
            // stays empty for a chip body, which has no `import * as`).
            importer_names: crate::collections::HashSet::default(),
            ns_mod_scopes: HashMap::default(),
            ns_mod_consts: HashMap::default(),
            import_state_dedup: crate::collections::HashMap::default(),
            import_behavior_lowered: crate::collections::HashSet::default(),
            dead_after_break: false,
            exec_closed_by_buffer_emit: false,
            fn_planes: HashMap::default(),
            fn_building: Vec::new(),
        }
    }

    pub(super) fn alloc_chain(&mut self) -> u32 {
        let id = self.next_chain_id;
        self.next_chain_id += 1;
        id
    }

    /// Is lowering currently inside a loop plane's body? Reads the innermost
    /// `current_anon_chip`, which mod inlining leaves untouched, so an
    /// inlined `await` answers the same as one written directly in the plane.
    pub(super) fn in_loop_plane(&self) -> bool {
        self.current_anon_chip
            .and_then(|id| self.builder.module.nodes.get(&id))
            .is_some_and(|n| gc::is_plane_shell(n.gate_class))
    }

    /// Is `r` written inside a `for` body typecheck walked with its loop
    /// depth raised? A statement that reached the plane through an inlined
    /// mod body sits outside every such range, so nothing reported it there.
    pub(super) fn inside_written_loop_body(&self, r: &SourceRange) -> bool {
        self.loop_body_ranges.iter().any(|b| {
            b.file == r.file && b.start.offset <= r.start.offset && r.end.offset <= b.end.offset
        })
    }

    /// The declared type of the OUTPUT port `src` refers to — the type of the
    /// value that flows out of it.
    ///
    /// Used to give a port whose catalog type is `any` the type of whatever is
    /// actually wired into it, so the emitted wire variant matches. Recurses
    /// into chips because the source may live in a nested module; NodeIds come
    /// from one allocator and are globally unique.
    pub(super) fn source_port_type(&self, src: PortRef) -> Option<Type> {
        self.find_node(src.node_id)?.output_type(src.port)
    }

    /// Add the instantiated chip `module` under `key`, recording where each of
    /// its nodes sits.
    pub(super) fn insert_chip(&mut self, key: NodeId, module: crate::ir::Module) {
        fn note(m: &crate::ir::Module, key: NodeId, out: &mut HashMap<NodeId, NodeId>) {
            out.extend(m.nodes.keys().map(|id| (*id, key)));
            m.chips.values().for_each(|c| note(c, key, out));
        }
        note(&module, key, &mut self.chip_of);
        self.builder.module.chips.insert(key, module);
    }

    /// The node `id` in this module or any chip under it. NodeIds come from
    /// one allocator and are globally unique, so where `chip_of` places a
    /// node only decides where to look first.
    pub(super) fn find_node(&self, id: NodeId) -> Option<&crate::ir::Node> {
        let m = &self.builder.module;
        m.nodes
            .get(&id)
            .or_else(|| m.chips.get(self.chip_of.get(&id)?)?.find_deep(id))
            .or_else(|| m.find_deep(id))
    }

    /// Resolve a surface name to its exec-signal key, via the scope binding
    /// (name -> hub port -> key). `None` when the name isn't a local exec
    /// signal in the current scope.
    pub(super) fn signal_key(&self, name: &str) -> Option<String> {
        match self.scope.get(name) {
            Some(Binding::Local(l)) => self.exec_signal_keys.get(&l.port.node_id).cloned(),
            _ => None,
        }
    }

    /// Allocate a fresh `ScopeId`, record it in `module.scopes` with the
    /// given kind + range, and return it. The `parent` is taken from the
    /// builder's `current_scope_id`.
    pub(super) fn alloc_scope(&mut self, kind: ScopeKind, range: SourceRange) -> ScopeId {
        let id = self.next_scope_id;
        self.next_scope_id += 1;
        let parent = Some(self.builder.current_scope_id);
        self.builder.module.scopes.insert(
            id,
            ScopeInfo {
                kind,
                source_range: range,
                parent,
            },
        );
        id
    }

    /// Push a scope, run `f`, then restore the previous scope. Use this
    /// wrapper around any lowering call that should emit nodes under a
    /// specific scope (handler body, chip body, if branches, blocks, ...).
    ///
    /// Also pushes/pops a name-resolution `scope` frame and its constants (via
    /// `push_scope`/`pop_scope`) around `f`, so every handler body — every
    /// trigger kind routes through this wrapper, not just the built-in-event
    /// path — gets its own BLOCK frame. Without this, a body-local `let`
    /// declared inside e.g. an `in`-port-triggered handler (`on go { let pf =
    /// ... }`) would have no frame of its own to bind into: it would land in
    /// whatever scope is ambient at the call site (module root, for a
    /// top-level handler) and would never be popped. Any caller that ALSO
    /// needs to bind names visible for the duration of `f` (e.g. a handler's
    /// typed event params) must do so INSIDE the closure, after this push —
    /// not before calling `with_scope` — or those bindings leak into the
    /// outer scope instead of being cleaned up with the body.
    pub(super) fn with_scope<R>(
        &mut self,
        kind: ScopeKind,
        range: SourceRange,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let id = self.alloc_scope(kind, range);
        self.push_scope(crate::scope::ScopeTag::BLOCK);
        let out = self.enter_scope(id, f);
        self.pop_scope();
        out
    }

    /// Enter an already-allocated scope. Useful when the caller needs the
    /// `ScopeId` up front (e.g., to pass it to a child scope's `parent`).
    pub(super) fn enter_scope<R>(&mut self, id: ScopeId, f: impl FnOnce(&mut Self) -> R) -> R {
        let prev = self.builder.current_scope_id;
        self.builder.current_scope_id = id;
        let out = f(self);
        self.builder.current_scope_id = prev;
        out
    }

    /// Mark the body of `d`, whose call is the innermost on
    /// [`chip_call_stack`](Self::chip_call_stack), as open: its arguments are
    /// lowered, and what lowers from here on is the body.
    pub(super) fn open_call_body(&mut self, d: &ChipDecl) {
        if let Some(top) = self.chip_call_stack.last_mut()
            && top.0 == d.range
        {
            top.1 = true;
        }
    }

    /// Push a new name-resolution `scope` frame together with its empty
    /// frame of constants, so the stacks can never drift out of lockstep
    /// (mirrors `typecheck::TypeCheckCtx::push_scope`). Use this (paired with
    /// `pop_scope`) everywhere lowering enters a block scope (handler body, if
    /// branch, `BlockExpr`, inline-mod call) instead of a bare
    /// `ctx.scope.push(...)`. A bare push would leave the constants pointing
    /// at the wrong (enclosing) frame, so a body-local `let` inside the new
    /// scope would be recorded one level too shallow, potentially leaking past
    /// its own scope's lifetime or clobbering a sibling scope's constant of the
    /// same name.
    pub(super) fn push_scope(&mut self, tag: crate::scope::ScopeTag) {
        self.scope.push(tag);
        self.consts.push(self.scope.current_id());
        self.pass1_chips.push(std::sync::Arc::new(HashMap::default()));
    }

    /// If a state decl at `range`'s source location was already lowered — the
    /// same source declaration pulled in through another import of the same
    /// file — return its binding to reuse rather than create a duplicate gate.
    /// See `import_state_dedup`.
    pub(super) fn reuse_import_state(&self, range: &SourceRange) -> Option<Binding> {
        self.import_state_dedup
            .get(&(range.file.to_string(), range.start.offset))
            .cloned()
    }

    /// Record `binding` as the gate produced for the state decl at `range`, so
    /// a later re-import of that same source location reuses it.
    pub(super) fn record_import_state(&mut self, range: &SourceRange, binding: Binding) {
        self.import_state_dedup
            .insert((range.file.to_string(), range.start.offset), binding);
    }

    /// Returns `true` the FIRST time a behaviour decl (handler / anon chip) at
    /// `range` is seen and `false` on every re-import, so the caller lowers it
    /// exactly once. See `import_behavior_lowered`.
    pub(super) fn first_import_of_behavior(&mut self, range: &SourceRange) -> bool {
        self.import_behavior_lowered
            .insert((range.file.to_string(), range.start.offset))
    }

    /// Pop the frame pushed by `push_scope`.
    pub(super) fn pop_scope(&mut self) {
        self.scope.pop();
        self.consts.pop();
        // Never pop the base frame — it holds the module's own top-level
        // declarations and must outlive every scope opened inside the module.
        if self.pass1_chips.len() > 1 {
            self.pass1_chips.pop();
        }
    }

    /// What a binding is to a constant read (see [`crate::const_scope::Bound`]),
    /// mirroring `typecheck::TypeCheckCtx`'s own: a name bound to a port or a
    /// record may be a constant its frame records; a mod, chip or namespace is
    /// no value and hides nothing; a constant container's gate holds that
    /// container; anything else is a value that is never a constant.
    fn const_bound(&self, b: &Binding) -> crate::const_scope::Bound {
        use crate::const_scope::Bound;
        match b {
            Binding::Chip(_) | Binding::Namespace(_) => Bound::NotAValue,
            Binding::Var(v) if self.immutable_containers.contains(&v.node_id) => {
                match self.builder.module.nodes.get(&v.node_id).and_then(|n| n.properties.get(&*sym::INITIAL_VALUE)) {
                    Some(lit) => Bound::Holds(lit.clone()),
                    None => Bound::MayBeConstant,
                }
            }
            Binding::Local(_) | Binding::Record(_) => Bound::MayBeConstant,
            Binding::Var(_)
            | Binding::Buffer(_)
            | Binding::Input(_)
            | Binding::Output(_)
            | Binding::EventParam(_)
            | Binding::DynRef(_) => Bound::Value,
        }
    }

    /// Every compile-time constant readable here, by name: a name resolves
    /// through the scope, so a parameter, local, loop variable or pattern
    /// binding named like an outer constant hides it (see
    /// [`crate::const_scope`]). Mirrors `typecheck::TypeCheckCtx::const_lookup`
    /// exactly, so both stages agree on which name resolves to which literal.
    pub(super) fn const_lookup(&self) -> crate::const_scope::ConstNames {
        self.consts.env(&self.scope, &|b| self.const_bound(b), crate::const_scope::Admit::All)
    }

    /// The constant `name` reads as here, when `admit` admits it.
    pub(super) fn constant(&self, name: &str, admit: crate::const_scope::Admit) -> Option<&Literal> {
        self.consts.get(&self.scope, &|b| self.const_bound(b), name, admit)
    }

    /// The `ConstCtx` for `const_eval::eval_expr`, resolving names as
    /// [`const_lookup`](Self::const_lookup) does, so lowering agrees with
    /// typecheck (`typecheck::TypeCheckCtx::const_ctx`) on what's constant at
    /// this point. A `const mod` body is evaluated against the module's
    /// constants plus its own parameters, never the call site's scope frames
    /// (see `const_eval::interp::eval_call`).
    ///
    /// `lookup_mod` is supplied by the CALLER, built from [`resolve_mod`](Self::resolve_mod)
    /// — see `TypeCheckCtx::const_ctx`'s matching doc comment for why this
    /// method can't build and embed that closure itself.
    pub(super) fn const_ctx<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::All, lookup_mod)
    }

    /// [`const_ctx`](Self::const_ctx) without any constant computed through
    /// an integer operation the certified fold table has no result for: what
    /// a `float` slot may be given as a literal. Mirrors
    /// `typecheck::TypeCheckCtx::certified_const_ctx`.
    pub(super) fn certified_const_ctx<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::Certified, lookup_mod)
    }

    fn admitting_ctx<'b>(
        &self,
        admit: crate::const_scope::Admit,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.consts.const_ctx(
            &self.scope,
            &|b| self.const_bound(b),
            admit,
            self.enum_defs.clone(),
            self.imports.clone(),
            lookup_mod,
        )
    }

    /// Whether `name` currently reads as a DECLARED compile-time constant.
    ///
    /// `lower_let_decl` uses it to confine its shadow-clearing to names a
    /// `const` actually introduced: a program that uses no `const` keyword
    /// declares nothing, so this is always `false` there and that program's
    /// lowering is untouched, which is the feature's own first rule.
    pub(super) fn is_declared_const(&self, name: &str) -> bool {
        self.constant(name, crate::const_scope::Admit::Declared).is_some()
    }

    /// The `Literal::Array` / `Literal::Map` a constant `name` holds, or
    /// `None` for anything else (a scalar constant, an unknown name).
    ///
    /// Not restricted to a declared `const`: a plain `let` bound to a constant
    /// container (`let x = T`, `let xs = [1, 2]`) binds no wire either (see
    /// `lower_let_decl`), so its only runtime form is the one
    /// `materialize_const_container` gives it. Its caller only asks for a name
    /// with no scope binding, which is what keeps a runtime container of the
    /// same name winning.
    pub(super) fn const_container_literal(&self, name: &str) -> Option<Literal> {
        match self.constant(name, crate::const_scope::Admit::All) {
            Some(lit @ (Literal::Array(_) | Literal::Map(_))) => Some(lit.clone()),
            _ => None,
        }
    }

    /// [`const_ctx`](Self::const_ctx), restricted to names DECLARED `const`:
    /// the `if`-condition elision's own environment (`lower_if`). A plain
    /// `let` that merely happens to fold must not participate in the
    /// generalised const-eval elision: the feature's own rule is that a
    /// program using no `const` compiles identically to before. Mirrors
    /// `typecheck::TypeCheckCtx::if_cond_const_ctx`.
    pub(super) fn if_cond_const_ctx<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::Declared, lookup_mod)
    }
    /// Resolve `name` to its `ChipDecl` for a `const mod` CALL, via the SAME
    /// scope-based resolution an ordinary (non-const) call already goes
    /// through: `lookup_chip` only finds a chip/mod whose `lower_chip_decl`
    /// has already run (lowering processes decls in source order, unlike
    /// typecheck's two-pass forward-reference-tolerant registration), so a
    /// const-mod call resolves here exactly as far "ahead" as any other call
    /// to it would at this point in lowering.
    pub(super) fn resolve_mod(&self, name: &str) -> Option<std::sync::Arc<ChipDecl>> {
        self.lookup_chip(name).cloned()
    }

    /// [`resolve_mod`](Self::resolve_mod)'s bake-path counterpart — see
    /// [`pass1_chips`](Self::pass1_chips)'s doc comment for why baking a
    /// var/array/map initializer's `const mod` CALL cannot simply call
    /// `resolve_mod` (`ctx.scope`) here.
    ///
    /// Scans frames INNERMOST-first, so a body-local declaration shadows a
    /// same-named outer one for exactly as long as its scope is open — the
    /// same shadowing `const_lookup` gives scoped constants.
    pub(super) fn resolve_mod_pass1(&self, name: &str) -> Option<std::sync::Arc<ChipDecl>> {
        self.pass1_chips
            .iter()
            .rev()
            .find_map(|frame| frame.get(name))
            .cloned()
    }

    /// `e`'s type in the instantiation being lowered; `Any` where typecheck
    /// recorded none.
    ///
    /// Outside a generic body typecheck checked `e` once, and that is its
    /// type. Inside one, typecheck's range-keyed record holds whichever
    /// instantiation it checked last, so it is never read: the type is
    /// typecheck's entry for this instantiation, which it checked, since it
    /// checks every instantiation a call demands and lowering builds no other
    /// (see `call::instantiation_checked`). A template build lowers a generic
    /// body with no substitution at all, for a resource estimate, and reads
    /// the record.
    pub(super) fn type_of(&self, e: &Expr) -> Type {
        self.type_at(e.range()).unwrap_or(Type::Any)
    }

    /// `e` with every `{}` that typecheck typed as a `Map` spelled as the empty
    /// map literal it is (the parser reads `{}` as an empty record literal),
    /// through the named fields of a record literal. Borrowed when there is
    /// none.
    pub(super) fn with_map_literals<'e>(&self, e: &'e Expr) -> std::borrow::Cow<'e, Expr> {
        use std::borrow::Cow;
        match e {
            Expr::RecordLit { fields, range } if fields.is_empty() => {
                if matches!(unwrap_ref(&self.type_of(e)), Type::Map(..)) {
                    Cow::Owned(Expr::MapLit { entries: Vec::new(), range: range.clone() })
                } else {
                    Cow::Borrowed(e)
                }
            }
            Expr::RecordLit { fields, range } => {
                let mut changed = false;
                let rewritten: Vec<crate::ast::RecordLitField> = fields
                    .iter()
                    .map(|f| match f {
                        crate::ast::RecordLitField::Named { name, value, range } => {
                            match self.with_map_literals(value) {
                                Cow::Owned(v) => {
                                    changed = true;
                                    crate::ast::RecordLitField::Named {
                                        name: name.clone(),
                                        value: v,
                                        range: range.clone(),
                                    }
                                }
                                Cow::Borrowed(_) => f.clone(),
                            }
                        }
                        _ => f.clone(),
                    })
                    .collect();
                if changed {
                    Cow::Owned(Expr::RecordLit { fields: rewritten, range: range.clone() })
                } else {
                    Cow::Borrowed(e)
                }
            }
            _ => Cow::Borrowed(e),
        }
    }

    /// [`Self::type_of`] for the expression at `r`, `None` where typecheck
    /// recorded none for this instantiation.
    pub(super) fn type_at(&self, r: &SourceRange) -> Option<Type> {
        let t = self.typecheck_type_at(r)?;
        Some(if t.holds_outputs() { t.as_record() } else { t.clone() })
    }

    /// The type typecheck gave the expression at `r` in the instantiation
    /// being lowered, a gate read as its [`Type::Outputs`]. [`Self::type_at`]
    /// holds a gate read as the record of its outputs.
    fn typecheck_type_at(&self, r: &SourceRange) -> Option<&'a Type> {
        let range = (r.file.clone(), r.start.offset, r.end.offset);
        let recorded = self.type_of_expr.get(&range)?;
        if !self.in_generic_instance(r) {
            return Some(recorded);
        }
        let key = (range.0, range.1, range.2, self.resolution_subst.clone());
        self.resolutions.generic_expr_types.get(&key)
    }

    /// The type typecheck gave the expression at `r`, a gate read held in it
    /// as its [`Type::Outputs`].
    pub(super) fn checked_type_at(&self, r: &SourceRange) -> Option<&'a Type> {
        self.typecheck_type_at(r)
    }

    /// What a `var` written with no type holds of the expression at `r` (see
    /// [`Type::held_by_var`]), as typecheck declares it.
    pub(super) fn held_by_var_at(&self, r: &SourceRange) -> Option<Type> {
        let t = self.typecheck_type_at(r)?;
        Some(match t {
            Type::Ref(inner) if matches!(inner.as_ref(), Type::Outputs(_)) => inner.held_by_var().as_record(),
            Type::Ref(_) => t.as_record(),
            _ => t.held_by_var().as_record(),
        })
    }

    /// [`Self::type_at`] for a place for one value: a gate read gives it its
    /// first output.
    pub(super) fn one_value_type_at(&self, r: &SourceRange) -> Option<Type> {
        match self.gate_outputs(r) {
            Some(outputs) => Some(unwrap_ref(&outputs[0].1).as_record()),
            None => self.type_at(r),
        }
    }

    /// The outputs of the gate read the expression at `r` is in the
    /// instantiation being lowered, in declared order; `None` for any other
    /// value, a record of the program's own included.
    pub(super) fn gate_outputs(&self, r: &SourceRange) -> Option<&'a [(String, Type)]> {
        match self.typecheck_type_at(r)? {
            Type::Outputs(fields) if !fields.is_empty() => Some(fields),
            Type::Ref(inner) => match inner.as_ref() {
                Type::Outputs(fields) if !fields.is_empty() => Some(fields),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether `r` is typed per instantiation: an expression of a generic
    /// body, lowered in one of its instantiations.
    pub(super) fn in_generic_instance(&self, r: &SourceRange) -> bool {
        !self.resolution_subst.is_empty()
            && self.resolutions.generic_ranges.contains(&(r.file.clone(), r.start.offset, r.end.offset))
    }

    pub(super) fn resolution_key(&self, e: &Expr) -> crate::typecheck::ResolutionKey {
        let r = e.range();
        (r.file.clone(), r.start.offset, r.end.offset, self.resolution_subst.clone())
    }

    /// Whether a missing resolution entry may be derived from lowering's own
    /// state instead: only in a template build, which lowers a generic body
    /// with no key at all. Every instantiation lowering builds otherwise is
    /// one typecheck checked, so a missing entry there means what it means
    /// outside a generic body.
    pub(super) fn resolution_fallback_allowed(&self) -> bool {
        self.template_build
    }

    /// What container `obj` is, as typecheck resolved it for this instantiation.
    /// With no entry, and only where [`Self::resolution_fallback_allowed`],
    /// falls back to the receiver's own binding, which is concrete after
    /// monomorphization, then to the type `obj` lowers to in this
    /// instantiation. The recorded expression type is not used for either: in a
    /// generic body it is the last combination typecheck checked.
    pub(super) fn container_kind(&self, obj: &Expr) -> Option<crate::typecheck::ContainerKind> {
        if let Some(k) = self.resolutions.container_kinds.get(&self.resolution_key(obj)) {
            return Some(k.clone());
        }
        if !self.resolution_fallback_allowed() {
            return None;
        }
        let ty = super::call::lowered_arg_type(self, obj);
        let binding = match obj {
            Expr::Ident { name, .. } => self.scope.get(name).cloned(),
            _ => resolve_field_chain(self, obj).cloned(),
        };
        if let Some(b) = &binding {
            if let Some((_, elem)) = binding_array_ref(self, b, &ty) {
                return Some(crate::typecheck::ContainerKind::Array { elem });
            }
            if let Some((_, map_ty)) = binding_map_ref(self, b, &ty) {
                return crate::typecheck::ContainerKind::of(&map_ty);
            }
        }
        crate::typecheck::ContainerKind::of(&ty)
    }

    /// What `recv.name(..)` resolved to, for this instantiation. With no entry,
    /// and only where [`Self::resolution_fallback_allowed`], classifies the
    /// method against [`Self::container_kind`], then as a receiver builtin,
    /// then as a `self`-mod in scope.
    pub(super) fn method_target(&self, call: &Expr) -> Option<crate::typecheck::MethodTarget> {
        if let Some(t) = self.resolutions.method_targets.get(&self.resolution_key(call)) {
            return Some(t.clone());
        }
        if !self.resolution_fallback_allowed() {
            return None;
        }
        let Expr::Call { callee, .. } = call else { return None };
        let Expr::FieldAccess { obj, field, range } = callee.as_ref() else { return None };
        if let Some(kind) = self.container_kind(obj)
            && let Some(t) = crate::typecheck::classify_method(field, &kind.as_type())
        {
            return Some(t);
        }
        if let Some(t) = crate::typecheck::classify_method(field, &Type::Any) {
            return Some(t);
        }
        self.self_mod_chip(field, &range.file)
            .map(|_| crate::typecheck::MethodTarget::SelfMod(field.clone()))
    }

    /// Record `d`'s declaration key: the `resolution_subst` in effect where it
    /// is registered, which its body's key starts with.
    ///
    /// Its enclosing frame is recorded under the same condition as the key, so
    /// the two agree: a declaration in a non-generic body inlined from a
    /// generic caller has an empty key and must not pick up the caller's frame.
    /// Every generic body pushes its lexically merged frame (see
    /// [`Self::body_frame`]), so `mono_stack.last()` here carries every
    /// enclosing param, not only the innermost body's.
    pub(super) fn record_decl_resolution_subst(&mut self, d: &ChipDecl) {
        let at = (d.range.file.clone(), d.range.start.offset);
        match self.mono_stack.last().filter(|_| !self.resolution_subst.is_empty()) {
            Some(frame) => {
                self.decl_mono_frame.insert(at.clone(), frame.clone());
            }
            None => {
                self.decl_mono_frame.remove(&at);
            }
        }
        if self.resolution_subst.is_empty() {
            self.decl_resolution_subst.remove(&at);
        } else {
            self.decl_resolution_subst.insert(at, self.resolution_subst.clone());
        }
    }

    /// Record the scope `d` is being declared in, when it is declared inside a
    /// body, as it stands now: every frame above the top level with the names
    /// bound so far, less the storage an enclosing body declares after the
    /// statement holding `d` (see `decl_later_storage`), and the constants.
    /// Called once `d` itself is bound, so its body can name it.
    pub(super) fn record_decl_view(&mut self, d: &ChipDecl) {
        let at = (d.range.file.clone(), d.range.start.offset);
        if self.scope.depth() <= 1 {
            self.decl_views.remove(&at);
            return;
        }
        let later = self.decl_later_storage.get(&at);
        let frames = self
            .scope
            .frames()
            .skip(1)
            .map(|(_, id, entries)| {
                let hidden = later.and_then(|l| l.iter().find(|(f, _)| *f == id)).map(|(_, names)| names);
                let bindings = entries
                    .filter(|(k, _)| hidden.is_none_or(|names| !names.contains(k)))
                    .map(|(k, b)| (k, b.clone()))
                    .collect();
                (id, bindings)
            })
            .collect();
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let view = DeclView {
            serial: SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            frames,
            consts: self.consts.snapshot(),
        };
        for id in view.frame_ids() {
            self.views_by_frame.entry(id).or_default().insert(at.clone());
        }
        self.decl_views.insert(at, Arc::new(view));
    }

    /// The index of the scope frame `d` was declared in: the innermost frame
    /// binding it. A mod of an `import * as` module belongs to the module
    /// level, whose own members its body frame receives (`ns_mod_scopes`).
    fn decl_frame(&self, d: &ChipDecl) -> Option<usize> {
        if self.ns_mod_scopes.contains_key(&(d.range.file.clone(), d.range.start.offset)) {
            return Some(0);
        }
        let is_d = |b: &Binding| {
            matches!(b, Binding::Chip(c) if c.range.file == d.range.file && c.range.start.offset == d.range.start.offset)
        };
        self.scope
            .frame_of(crate::intern::intern(&d.name), is_d)
            .or_else(|| {
                self.scope
                    .frame_where(|b| is_d(b) || matches!(b, Binding::Namespace(m) if m.values().any(is_d)))
            })
    }

    /// `b` with every port it names on another module's `_Literal` replaced
    /// by that literal's copy in this module, made on first use. A wire from
    /// the other module's literal would carry no value: emit inlines a literal
    /// only into its own module.
    pub(super) fn local_literals(&mut self, b: &mut Binding) {
        match b {
            Binding::Local(l) => {
                if let Some(copy) = self.literal_copies.get(&l.port) {
                    l.port = *copy;
                } else if let Some(src) = self.foreign_literals.get(&l.port.node_id).cloned() {
                    let id = self.add_gate(AddNodeOpts {
                        gate_class: gc::LITERAL,
                        source_range: src.source_range.clone(),
                        ports: (*src.ports).clone(),
                        properties: (*src.properties).clone(),
                        ..Default::default()
                    });
                    let copy = id.port(l.port.port);
                    self.literal_copies.insert(l.port, copy);
                    l.port = copy;
                }
            }
            Binding::Record(fields) => {
                for f in fields.values_mut() {
                    self.local_literals(f);
                }
            }
            _ => {}
        }
    }

    /// Make the scope what `d`'s body sees: the frames up to the one `d` was
    /// declared in, each as it stood at the declaration (see
    /// `record_decl_view`). Everything a caller opened above them, its locals,
    /// params and nested declarations, is out of reach until
    /// [`leave_decl_scope`](Self::leave_decl_scope) puts it back. The constant
    /// and chip-declaration stacks, which keep one frame per scope frame, are
    /// cut to match, and the constants are the declaration's.
    pub(super) fn enter_decl_scope(&mut self, d: &ChipDecl) -> DeclScope {
        let depth = self.scope.depth();
        let found = self.decl_frame(d);
        let index = found.unwrap_or(depth - 1);
        let view = if found.is_some() && index > 0 {
            self.decl_views.get(&(d.range.file.clone(), d.range.start.offset)).cloned()
        } else {
            None
        };
        // Frames the caller opened above the declaring one. When `d` is not
        // bound anywhere no scope frame is hidden, and the constant stack is
        // emptied as it always was for an inlined body.
        let above = depth - 1 - index;
        let mut copies = 0;
        let frames = match &view {
            Some(view) => {
                // A copy of each frame up to the declaring one, holding what
                // it held at the declaration: a name bound since is dropped,
                // and one bound again since gets the declaration's binding.
                let frames = self.scope.detach_above(0);
                // A binding the declaration saw that has been replaced since
                // starts with no cached read.
                let mut replaced: Vec<NodeId> = Vec::new();
                let wanted: Vec<_> = frames
                    .frames()
                    .take(index)
                    .map(|(tag, id, now)| {
                        let entries: Vec<(crate::intern::Sym, Binding)> =
                            match view.frames.iter().find(|(f, _)| *f == id) {
                                Some((_, snap)) => snap
                                    .iter()
                                    .map(|(k, s)| {
                                        let b = match now.get(k) {
                                            Some(n) if self.same_declaration(n, s) => n.clone(),
                                            _ => {
                                                var_nodes(s, &mut replaced);
                                                s.clone()
                                            }
                                        };
                                        (*k, b)
                                    })
                                    .collect(),
                                None => now.iter().map(|(k, b)| (*k, b.clone())).collect(),
                            };
                        (tag, id, entries)
                    })
                    .collect();
                for id in &replaced {
                    self.var_gets.remove(id);
                }
                for (tag, id, entries) in wanted {
                    let mut local = Vec::with_capacity(entries.len());
                    for (k, mut b) in entries {
                        self.local_literals(&mut b);
                        local.push((k, b));
                    }
                    self.scope.push_as(tag, id, local);
                    copies += 1;
                }
                Some(frames)
            }
            None if above > 0 => Some(self.scope.detach_above(index)),
            None => None,
        };
        let cut = |len: usize| {
            if found.is_some() {
                len - above.min(len)
            } else {
                0
            }
        };
        let consts_at = cut(self.consts.depth());
        let consts = self.consts.split_off(consts_at);
        let chips_at = (self.pass1_chips.len() - above.min(self.pass1_chips.len())).max(1);
        let chips = self.pass1_chips.split_off(chips_at);
        let own_consts = view.as_ref().map(|view| self.consts.replace(view.consts.clone()));
        let outside = if index == 0 { self.leave_anon_chips_for(d) } else { None };
        let inside = if index == 0 { self.enter_anon_chips_for(d) } else { None };
        // The body reads the top level as its declaration sees it, wherever it
        // is lowered from.
        let top =
            if index == 0 { self.top_view(crate::scope::top::ReadAt::body(&d.range)) } else { TopView::default() };
        DeclScope { frames, copies, consts, chips, own_consts, outside, inside, top }
    }

    /// The enum name an expression path's head writes, unless a binding of the
    /// name shadows it: `Dir` in `Dir.N`, and for the head `c.Dir` of
    /// `c.Dir.N`, where `c` is an `import * as` namespace whose module
    /// declares enum `Dir`, that enum's own name.
    pub(super) fn enum_head<'e>(&self, head: &'e Expr) -> Option<&'e str> {
        match head {
            Expr::Ident { name, .. } => self.scope.get(name).is_none().then_some(name.as_str()),
            Expr::FieldAccess { obj, field, .. } => {
                let Expr::Ident { name: ns, range } = obj.as_ref() else { return None };
                let namespace = matches!(self.scope.get(ns), Some(Binding::Namespace(_)) | None);
                (namespace && self.enum_defs.namespaced(&range.file, ns, field).is_some()).then_some(field.as_str())
            }
            _ => None,
        }
    }

    /// The enum registry with the declarations a payload field's type is read
    /// against.
    pub(super) fn enums(&self) -> crate::typecheck::patterns::Enums<'_> {
        crate::typecheck::patterns::Enums { defs: &self.enum_defs, types: self.consts.types() }
    }

    /// Read the top level as code (or the body of a declaration) written at
    /// `at` sees it: each name declared more than once at the top level of
    /// `at`'s file is bound, in the outermost frame and in the constants, to
    /// the declaration visible at `at` (see
    /// [`crate::scope::top::TopNames::visible_decl`]), and unbound when none
    /// is visible there yet. Hands back what
    /// [`restore_top_view`](Self::restore_top_view) puts back.
    pub(super) fn top_view(&mut self, at: crate::scope::top::ReadAt) -> TopView {
        if !self.top_names.has_dups() {
            return TopView::default();
        }
        let top = self.top_names.clone();
        let mut bound = Vec::new();
        for (name, decls) in top.dups() {
            // Read from another file, a name a library declares twice is its
            // last declaration, as an import binds it.
            let seen = if decls[0].range.file == at.range.file {
                top.visible_decl(name, &at).flatten()
            } else if top.is_dup(&at.range.file, name) {
                continue;
            } else {
                decls.last()
            };
            let mut b = seen.and_then(|d| self.top_dup_bindings.get(&crate::lower::dup_key(d, name))).cloned();
            if let Some(b) = &mut b {
                self.local_literals(b);
            }
            bound.push((name.to_string(), self.scope.set_root(name, b)));
        }
        TopView { bound, cursor: self.consts.set_cursor(Some(at)) }
    }

    /// Undo a [`top_view`](Self::top_view).
    pub(super) fn restore_top_view(&mut self, view: TopView) {
        for (name, b) in view.bound.into_iter().rev() {
            self.scope.set_root(&name, b);
        }
        self.consts.set_cursor(view.cursor);
    }

    /// Keep the binding each top-level declaration in `d` gave a name
    /// declared more than once, for [`top_view`](Self::top_view). A
    /// declaration that binds none (a constant only a `const mod` call gives)
    /// keeps none, and a read of it finds its constant.
    pub(super) fn keep_top_dup_bindings(&mut self, d: &TopDecl) {
        if !self.top_names.has_dups() {
            return;
        }
        let r = d.range();
        for (name, _) in crate::scope::top::declared_names_of(d) {
            if !self.top_names.is_dup(&r.file, &name) {
                continue;
            }
            let key = (r.file.clone(), r.start.offset, name);
            match self.scope.get_in(0, crate::intern::intern(&key.2)).cloned() {
                Some(b) => Arc::make_mut(&mut self.top_dup_bindings).insert(key, b),
                None => Arc::make_mut(&mut self.top_dup_bindings).remove(&key),
            };
        }
    }

    /// For `d`, declared at the top level, the anonymous chips it is written
    /// in that are not being lowered: their names mean what each chip's
    /// declarations make them (see `scope::top`), until
    /// [`leave_decl_scope`](Self::leave_decl_scope). A chip lowered already
    /// gives what it left; one not lowered yet, its storage.
    fn enter_anon_chips_for(&mut self, d: &ChipDecl) -> Option<InsideChips> {
        let top = self.top_names.clone();
        let entered: Vec<&crate::scope::top::TopChip> =
            top.chips_around(&d.range).filter(|c| !self.anon_chip_shadows.iter().any(|s| s.body == c.range)).collect();
        if entered.is_empty() {
            return None;
        }
        let from = self.anon_chip_shadows.len();
        let mut restore = Vec::new();
        for chip in entered {
            let (bindings, held) = match self.anon_chip_names.get(&chip.range) {
                Some(left) => left.clone(),
                None => {
                    let node = self.anon_chip_nodes.iter().find(|((r, _), _)| *r == chip.range).map(|(_, n)| *n);
                    let storage = node.and_then(|n| self.anon_chip_storage.get(&n)).cloned().unwrap_or_default();
                    (storage, chip.names.iter().map(|n| (n.clone(), None)).collect())
                }
            };
            let mut outside = Vec::new();
            for (name, b) in bindings {
                let before = self.scope.insert(&name, b);
                if let Some(before) = &before {
                    outside.push((name.clone(), before.clone()));
                }
                restore.push((name, before));
            }
            let hidden_mark = self.consts.hide_held_at_top(held);
            self.anon_chip_shadows.push(AnonChipShadow { body: chip.range.clone(), outside, hidden_mark });
        }
        Some((from, restore))
    }

    /// For `d`, declared at the top level, the anonymous chips being lowered
    /// that it is written outside of: the top level gets back the bindings
    /// their storage hid, and the constant scope the names, until
    /// [`leave_decl_scope`](Self::leave_decl_scope). The top level is the
    /// innermost frame here.
    fn leave_anon_chips_for(&mut self, d: &ChipDecl) -> Option<OutsideChips> {
        let inside = |s: &AnonChipShadow| {
            s.body.file == d.range.file
                && s.body.start.offset <= d.range.start.offset
                && d.range.end.offset <= s.body.end.offset
        };
        let i = self.anon_chip_shadows.iter().position(|s| !inside(s))?;
        let shadows = self.anon_chip_shadows.split_off(i);
        let mut storage = Vec::new();
        for (name, b) in shadows.iter().flat_map(|s| s.outside.iter()) {
            if let Some(now) = self.scope.insert(name, b.clone()) {
                storage.push((name.clone(), now));
            }
        }
        let hidden = self.consts.split_hidden_at_top(shadows[0].hidden_mark);
        Some((shadows, storage, hidden))
    }

    /// Whether `now` is the declaration `snap` recorded, a literal read
    /// through this module's copy of it included.
    fn same_declaration(&self, now: &Binding, snap: &Binding) -> bool {
        match (now, snap) {
            (Binding::Local(n), Binding::Local(s)) => {
                n.port == s.port || self.literal_copies.get(&s.port) == Some(&n.port)
            }
            (Binding::Record(x), Binding::Record(y)) => {
                x.len() == y.len()
                    && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| self.same_declaration(v, w)))
            }
            _ => same_binding(now, snap),
        }
    }

    /// Undo [`enter_decl_scope`](Self::enter_decl_scope). Every frame the
    /// body pushed must already be popped.
    pub(super) fn leave_decl_scope(&mut self, s: DeclScope) {
        for _ in 0..s.copies {
            self.scope.pop();
        }
        self.restore_top_view(s.top);
        if let Some((from, restore)) = s.inside {
            let entered = self.anon_chip_shadows.split_off(from);
            if let Some(first) = entered.first() {
                self.consts.unhide_at_top(first.hidden_mark);
            }
            for (name, before) in restore.into_iter().rev() {
                match before {
                    Some(b) => self.scope.insert(&name, b),
                    None => self.scope.remove(&name),
                };
            }
        }
        if let Some((shadows, storage, hidden)) = s.outside {
            for (name, b) in storage {
                self.scope.insert(&name, b);
            }
            self.consts.hide_held_at_top(hidden);
            self.anon_chip_shadows.extend(shadows);
        }
        if let Some(frames) = s.frames {
            self.scope.reattach(frames);
        }
        if let Some(c) = s.own_consts {
            self.consts.replace(c);
        }
        self.consts.extend(s.consts);
        self.pass1_chips.extend(s.chips);
    }

    /// Run `f` in `d`'s declaration scope (see
    /// [`enter_decl_scope`](Self::enter_decl_scope)).
    pub(super) fn with_decl_scope<R>(&mut self, d: &ChipDecl, f: impl FnOnce(&mut Self) -> R) -> R {
        let s = self.enter_decl_scope(d);
        let out = f(self);
        self.leave_decl_scope(s);
        out
    }

    /// The generic frame `d`'s body is lowered under, inlined or instanced as a
    /// chip: its enclosing frame (from its declaration), extended by `own` for
    /// a generic `d`, whose params shadow same-named outer ones. This mirrors
    /// typecheck's body key (enclosing combo, then the decl's own). `None` for a
    /// non-generic `d` declared outside any generic body.
    pub(super) fn body_frame(&self, d: &ChipDecl, own: Option<&MonoFrame>) -> Option<MonoFrame> {
        let outer = self.decl_mono_frame.get(&(d.range.file.clone(), d.range.start.offset));
        match (outer, own) {
            (None, None) => None,
            (Some(f), None) | (None, Some(f)) => Some(f.clone()),
            (Some(outer), Some(own)) => {
                let mut frame = outer.clone();
                frame.params.retain(|p| !own.params.contains(p));
                frame.params.extend(own.params.iter().cloned());
                for (name, ty) in own.subst.iter() {
                    frame.subst.insert(name.clone(), ty.clone());
                }
                for (name, d) in own.scopes.iter() {
                    frame.scopes.insert(name.clone(), d.clone());
                }
                Some(frame)
            }
        }
    }

    /// The resolution key for `d`'s body: its declaration key, then, for a
    /// generic `d`, this instantiation's type arguments in declared order.
    pub(super) fn body_resolution_subst(&self, d: &ChipDecl, frame: Option<&MonoFrame>) -> Arc<[Type]> {
        let key = self
            .decl_resolution_subst
            .get(&(d.range.file.clone(), d.range.start.offset))
            .cloned()
            .unwrap_or_default();
        match frame {
            Some(f) => key
                .iter()
                .cloned()
                .chain(f.params.iter().map(|p| f.subst.get(p).cloned().unwrap_or(Type::Any)))
                .collect(),
            None => key,
        }
    }

    /// The type `te`, written in `decl`'s file, declares under `frame` (a
    /// generic body's instantiation), every alias resolved: what a constant
    /// introduced with that annotation is held in. `None` when part of it is
    /// not known here (a type parameter with no instantiation).
    pub(super) fn declared_type(
        &self,
        decl: &crate::ast::ChipDecl,
        te: &crate::ast::TypeExpr,
        frame: Option<&MonoFrame>,
    ) -> Option<Type> {
        let types = self.consts.types().in_file(&decl.range.file);
        let t = match frame {
            Some(f) => types.resolve_scoped(te, &|name, at| f.param_at(name, at)),
            None => types.resolve(te),
        };
        fn known(t: &Type) -> bool {
            match t {
                Type::Any | Type::Param(_) => false,
                Type::Array(e) | Type::Ref(e) => known(e),
                Type::Map(k, v) => known(k) && known(v),
                Type::Record(fs) => fs.iter().all(|(_, t)| known(t)),
                Type::Tuple(ts) | Type::Union(ts) => ts.iter().all(known),
                Type::Enum { args, .. } => args.iter().all(known),
                _ => true,
            }
        }
        known(&t).then_some(t)
    }

    /// What the type name `name`, written at `at`, stands for when it is a
    /// type parameter of the generic body being lowered there (see
    /// [`MonoFrame::param_at`]).
    pub(super) fn type_param_at(&self, name: &str, at: &SourceRange) -> Option<Type> {
        self.mono_stack.last()?.param_at(name, at)
    }

    /// The type declarations a type written at `at` reads its names against:
    /// those of the file that writes it.
    pub(super) fn declared_types_at(&self, at: &SourceRange) -> &crate::const_eval::DeclaredTypes {
        self.consts.types().in_file(&at.file)
    }

    /// What the non-generic alias `name`, written at `at`, finally stands for
    /// (see [`crate::const_eval::DeclaredTypes::alias_target`]): `None` when
    /// it names no such alias there, or names a type parameter of the generic
    /// body being lowered, which hides an alias of its name.
    pub(super) fn alias_at(&self, name: &str, at: &SourceRange) -> Option<&crate::ast::TypeExpr> {
        if self.type_param_at(name, at).is_some() {
            return None;
        }
        self.declared_types_at(at).alias_target(name)
    }

    /// The generic alias `name<args>` written at `at`, instantiated: its body
    /// with each of its parameters replaced by the argument written for it.
    pub(super) fn generic_alias_at(
        &self,
        name: &str,
        args: &[crate::ast::TypeExpr],
        at: &SourceRange,
    ) -> Option<crate::ast::TypeExpr> {
        match self.declared_types_at(at).alias(name)? {
            (params, body) if !params.is_empty() && params.len() == args.len() => {
                let subst = params.iter().cloned().zip(args.iter().cloned()).collect();
                Some(crate::types::resolve::substitute_type_expr(body, &subst))
            }
            _ => None,
        }
    }

    /// `te`, declared by `decl` (one of its parameters, or a field of one;
    /// `None` for a type written in the body being lowered), with the aliases
    /// at its head written out as the type they declare, each read where it is
    /// declared: `type RI = *int` gives `*int`, `*M` with `type M = Map<int,
    /// int>` gives `*Map<int, int>`, a generic alias its instantiated body. A
    /// type parameter is left as written. Every test of whether a parameter,
    /// field or pin takes its value by reference reads this rather than the
    /// spelling, so an alias of a ref, an array or a map binds exactly as the
    /// type it names does. The shape is kept (a `*{ .. }` stays a ref of a
    /// record, which the binding rules tell apart from a record of refs).
    pub(super) fn declared_shape(&self, decl: Option<&ChipDecl>, te: &crate::ast::TypeExpr) -> crate::ast::TypeExpr {
        self.declared_shape_at(decl, te, 0)
    }

    fn declared_shape_at(
        &self,
        decl: Option<&ChipDecl>,
        te: &crate::ast::TypeExpr,
        depth: u32,
    ) -> crate::ast::TypeExpr {
        use crate::ast::TypeExpr as T;
        if depth > MAX_SHAPE_DEPTH {
            return te.clone();
        }
        let declares = |name: &str, at: &SourceRange| {
            decl.is_some_and(|d| {
                d.type_params.iter().any(|p| p.name == name)
                    && d.range.file == at.file
                    && d.range.start.offset <= at.start.offset
                    && at.end.offset <= d.range.end.offset
            })
        };
        match te {
            T::Name { name, range } if !declares(name, range) => match self.alias_at(name, range) {
                Some(body) => self.declared_shape_at(decl, body, depth + 1),
                None => te.clone(),
            },
            T::Generic { name, args, range } => match self.generic_alias_at(name, args, range) {
                Some(body) => self.declared_shape_at(decl, &body, depth + 1),
                None => te.clone(),
            },
            T::Ref { inner, range } => {
                T::Ref { inner: Box::new(self.declared_shape_at(decl, inner, depth + 1)), range: range.clone() }
            }
            _ => te.clone(),
        }
    }

    /// How a parameter, record field or pin declared `te` (see
    /// [`Self::declared_shape`]) takes its value: by reference to storage
    /// (`Var` for a `*T`, `Array` for an array, `Map` for a map or a `*Map`),
    /// `None` by value.
    pub(super) fn ref_storage(&self, decl: Option<&ChipDecl>, te: &crate::ast::TypeExpr) -> Option<VarStorage> {
        container_storage(&self.declared_shape(decl, te))
    }

    /// Whether a parameter or record field declared `te` (see
    /// [`Self::declared_shape`]) is a ref or an array: a pin that carries a
    /// ref to the caller's storage rather than a value. A map takes the value
    /// path.
    pub(super) fn ref_or_array(&self, decl: Option<&ChipDecl>, te: &crate::ast::TypeExpr) -> bool {
        matches!(self.declared_shape(decl, te), crate::ast::TypeExpr::Ref { .. } | crate::ast::TypeExpr::Array { .. })
    }

    /// The type `te` names here: each name it writes is a type parameter of
    /// the generic body being lowered where that body writes it (replaced by
    /// this instantiation's type), else read against the declarations of the
    /// file that writes it, and an alias stands for its body as its own file
    /// reads it.
    pub(super) fn resolve_local_type(&self, te: &crate::ast::TypeExpr) -> Type {
        self.declared_types_at(te.range()).resolve_scoped(te, &|name, at| self.type_param_at(name, at))
    }

    /// Expand a record-shaped type annotation into its fields, following a
    /// non-generic alias (`type P = { … }`) or instantiating a generic alias
    /// (`type Pair<T> = { … }` used as `Pair<int>`, substituting the type
    /// args). Returns `None` for a non-record type. The single source of
    /// record-port dissolution for chip inputs, mod params, and top-level
    /// ports — every such site routes through here so the `Generic` arm can't
    /// be forgotten at one of them.
    pub(super) fn record_fields_of(
        &self,
        te: &crate::ast::TypeExpr,
    ) -> Option<Vec<crate::ast::RecordTypeField>> {
        match te {
            crate::ast::TypeExpr::Record { fields, .. } => Some(fields.clone()),
            // A type parameter of the generic body being lowered, given a
            // record by this instantiation, or an alias of a record.
            crate::ast::TypeExpr::Name { name, range } => match self.type_param_expr(name, range) {
                Some(crate::ast::TypeExpr::Record { fields, .. }) => Some(fields),
                Some(_) => None,
                None => match self.alias_at(name, range)? {
                    crate::ast::TypeExpr::Record { fields, .. } => Some(fields.clone()),
                    // An alias of a generic alias, or of a ref (`type PR = *R`),
                    // as the type it names.
                    body @ (crate::ast::TypeExpr::Generic { .. } | crate::ast::TypeExpr::Ref { .. }) => {
                        self.record_fields_of(body)
                    }
                    _ => None,
                },
            },
            crate::ast::TypeExpr::Generic { name, args, range } => {
                match self.generic_alias_at(name, args, range)? {
                    crate::ast::TypeExpr::Record { fields, .. } => Some(fields),
                    _ => None,
                }
            }
            // `*T` on a record distributes over its fields, mirroring
            // `types::resolve::distribute_ref` on the type side. Reporting the
            // per-field shape here routes such a param through the record path
            // in `resolve_caller_captures` / `explode_record_param_pins` /
            // `wire_chip_args_and_outputs`, which carry ref fields writably.
            crate::ast::TypeExpr::Ref { inner, range } => {
                Some(distribute_ref_fields(self, self.record_fields_of(inner)?, range))
            }
            _ => None,
        }
    }

    /// What the type parameter `name` of the generic body being lowered stands
    /// for in this instantiation, written as an annotation would be. `None`
    /// outside a generic body or for a name that is not one of its parameters.
    fn type_param_expr(&self, name: &str, range: &SourceRange) -> Option<crate::ast::TypeExpr> {
        let t = self.type_param_at(name, range)?;
        if matches!(t, Type::Param(_)) {
            return None;
        }
        Some(type_expr_of(&t, range))
    }

    /// Like [`record_fields_of`], but also expands a TUPLE type `(A, B, …)` into
    /// index-named fields ("0", "1", …). A tuple chip/mod param thus explodes
    /// into per-element pins exactly like a record, and its `ParamPattern::Tuple`
    /// destructure (keyed by index) binds the element names — without this a
    /// tuple param fell to a single by-value pin and its names stayed unbound.
    pub(super) fn record_or_tuple_fields(
        &self,
        te: &crate::ast::TypeExpr,
    ) -> Option<Vec<crate::ast::RecordTypeField>> {
        // `*(A, B)` distributes just as `*{ a: A, b: B }` does. Handled here
        // rather than in `record_fields_of` because only this function expands
        // a tuple into index-named fields.
        if let crate::ast::TypeExpr::Ref { inner, range } = te {
            if let Some(fields) = self.record_or_tuple_fields(inner) {
                return Some(distribute_ref_fields(self, fields, range));
            }
            // `*Enum` distributes over the enum's slot columns (`__disc` plus
            // every variant's payload), the same layout `build_enum_fields`
            // gives an enum var. Only under a ref: a by-value enum param keeps
            // its existing single-pin path.
            let cols = crate::lower::predeclare::enum_container_columns(self, inner, range)?;
            return Some(distribute_ref_fields(self, cols, range));
        }
        if let Some(fields) = self.record_fields_of(te) {
            return Some(fields);
        }
        // A `type P = (A, B)` alias, resolved the way `record_fields_of`
        // resolves a record alias.
        let resolved;
        let te = match te {
            crate::ast::TypeExpr::Name { name, range } => match self.type_param_expr(name, range) {
                Some(t) => {
                    resolved = t;
                    &resolved
                }
                None => match self.alias_at(name, range) {
                    Some(crate::ast::TypeExpr::Generic { name, args, range }) => {
                        match self.generic_alias_at(name, args, range) {
                            Some(t) => {
                                resolved = t;
                                &resolved
                            }
                            None => te,
                        }
                    }
                    Some(t) => t,
                    None => te,
                },
            },
            other => other,
        };
        if let crate::ast::TypeExpr::Tuple { fields, .. } = te {
            return Some(
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, t)| crate::ast::RecordTypeField {
                        name: i.to_string(),
                        typ: t.clone(),
                        range: t.range().clone(),
                    })
                    .collect(),
            );
        }
        None
    }

    pub(super) fn op_for(&self, e: &Expr) -> Option<&OpRule> {
        let r = e.range();
        self.op_resolutions
            .get(&(r.file.clone(), r.start.offset, r.end.offset))
    }

    /// `opts` placed in the current anonymous chip when it names none, and
    /// marked `_nofold` inside a `@nofold` region.
    fn stamped(&self, mut opts: AddNodeOpts) -> AddNodeOpts {
        if opts.chip_id.is_none() {
            opts.chip_id = self.current_anon_chip;
        }
        if self.nofold_depth > 0 {
            opts.properties.insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        opts
    }

    pub(super) fn add_gate(&mut self, opts: AddNodeOpts) -> NodeId {
        let opts = self.stamped(opts);
        self.builder.add_gate(opts)
    }

    pub(super) fn add_event(&mut self, opts: AddNodeOpts) -> NodeId {
        let opts = self.stamped(opts);
        self.builder.add_event(opts)
    }

    /// Mark the boundary rerouter `id` `_nofold` inside a `@nofold` region,
    /// as [`LowerCtx::add_gate`] does a gate: `add_input`/`add_output` live on
    /// `ModuleBuilder`, which does not see `nofold_depth`.
    fn stamp_pin(&mut self, id: NodeId) -> NodeId {
        if self.nofold_depth > 0
            && let Some(node) = self.builder.module.nodes.get_mut(&id)
        {
            std::sync::Arc::make_mut(&mut node.properties).insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        id
    }

    /// A chip's boundary `MicrochipInput` rerouter (see [`LowerCtx::stamp_pin`]).
    pub(super) fn add_input(&mut self, port_name: &str, ty: Type, source_range: SourceRange) -> NodeId {
        let id = self.builder.add_input(port_name, ty, source_range);
        self.stamp_pin(id)
    }

    /// A chip's boundary `MicrochipOutput` rerouter (see [`LowerCtx::stamp_pin`]).
    pub(super) fn add_output(&mut self, port_name: &str, ty: Type, source_range: SourceRange) -> NodeId {
        let id = self.builder.add_output(port_name, ty, source_range);
        self.stamp_pin(id)
    }

    pub(super) fn connect(&mut self, src: PortRef, dst: PortRef) {
        // Language-level string -> bool coercion: a string-typed source wired
        // into a bool-typed destination routes through an inserted
        // `CompareNotEqual(src, "")` gate, so the coercion means exactly
        // `src != ""` — empty is false, EVERYTHING else (including "0" and
        // "false") is true. The game's native bool ports would otherwise
        // apply their content-aware truthiness ("0"/"false" falsy but "0.0"
        // truthy — see the Branch/Select/AND chapters of
        // `data/gate_semantics.json`), which is a footgun the language
        // deliberately papers over. Native truthiness remains reachable by
        // wiring through `any` (`Type::Opaque` erases the String port type,
        // so this interception never sees it) or via the logical operators,
        // whose string overloads are native gate behavior, not coercions.
        if let Some(coerced) = self.wrap_string_to_bool(src, dst) {
            self.builder.connect(coerced, dst);
            return;
        }
        if let Some(coerced) = self.wrap_to_string(src, dst) {
            self.builder.connect(coerced, dst);
            return;
        }
        if self.stores_constant_container(src, dst) {
            return;
        }
        self.builder.connect(src, dst);
    }

    /// Reports (WS044) and returns true when `src` is the ref of an immutable
    /// container and `dst` stores it somewhere it can be read back and changed:
    /// the value of a var/array/map write, an output port, or event data.
    fn stores_constant_container(&mut self, src: PortRef, dst: PortRef) -> bool {
        if self.immutable_containers.is_empty() {
            return false;
        }
        let Some(target) = self.builder.module.nodes.get(&dst.node_id) else {
            return false;
        };
        let stores = match dst.port {
            WirePort::Value => matches!(
                target.gate_class,
                gc::FUNCTION_OUTPUT
                    | gc::VAR_SET
                    | gc::ARRAY_PUSH
                    | gc::ARRAY_SET_AT_INDEX
                    | gc::ARRAY_INSERT
                    | gc::ARRAY_FILL
                    | gc::ARRAY_RESIZE
                    | gc::MAP_SET
            ),
            WirePort::RerInput => {
                target.gate_class == gc::MICROCHIP_OUTPUT && !self.inline_outputs.contains(&dst.node_id)
            }
            WirePort::DataIn1
            | WirePort::DataIn2
            | WirePort::DataIn3
            | WirePort::DataIn4
            | WirePort::DataIn5
            | WirePort::DataIn6
            | WirePort::DataIn7
            | WirePort::DataIn8 => true,
            _ => false,
        };
        if !stores || !super::access::is_immutable_container(self, src.node_id) {
            return false;
        }
        let range = target.source_range.clone();
        self.error(
            "WS044",
            "this stores a reference to a constant array/map (a `const`, or a `let` or \
             literal holding a constant one) where it can be read back and changed, which \
             would make its runtime contents disagree with its compile-time value. Copy it \
             into a `var` first (`var c: int[]` and `c.copyFrom(t)`) and store that",
            &range,
        );
        true
    }

    /// If `src` is a string-typed output and `dst` a bool-typed input,
    /// build the `!= ""` coercion gate (`CompareNotEqual` with `InputB`
    /// baked to the empty string — compare inputs are wire-variant fields,
    /// so the constant inlines as data) and return its `bOutput` for the
    /// caller to wire into `dst`. `None` when no coercion applies (either
    /// side untyped/not found, or any other type pair). NOTE: the inserted
    /// gate does NOT constant-fold — `CompareNotEqual` has no certified
    /// (str, str) signature in `data/gate_semantics.json`, so the fold pass
    /// refuses it (correct-but-unoptimized until that signature is probed).
    fn wrap_string_to_bool(&mut self, src: PortRef, dst: PortRef) -> Option<PortRef> {
        // A boundary node of an already-instantiated chip lives in a nested
        // `module.chips` entry, not the top-level node map (e.g. wiring a
        // string call argument into a chip's bool-typed `MicrochipInput`
        // pin), so the lookup recurses. NodeIds come from one allocator and
        // are globally unique.
        let src_range = {
            let n = self.find_node(src.node_id)?;
            if n.ports.find_output(src.port.sym())?.ty != Type::String {
                return None;
            }
            n.source_range.clone()
        };
        {
            let n = self.find_node(dst.node_id)?;
            if n.ports.find_input(dst.port.sym())?.ty != Type::Bool {
                return None;
            }
        }
        let mut props = HashMap::default();
        props.insert(*sym::INPUT_B, Literal::String(String::new()));
        let ne = self.add_gate(AddNodeOpts {
            gate_class: gc::COMPARE_NOT_EQUAL,
            source_range: src_range,
            ports: GateIO {
                inputs: vec![
                    PortSpec::new(*sym::INPUT_A, Type::String),
                    PortSpec::new(*sym::INPUT_B, Type::String),
                ],
                outputs: vec![PortSpec::new(*sym::B_OUTPUT, Type::Bool)],
            },
            properties: props,
            note: Some("string→bool coercion (!= \"\")"),
            ..Default::default()
        });
        self.builder.connect(src, ne.port(WirePort::InputA));
        Some(ne.port(WirePort::BOutput))
    }

    /// Language-level coercion to `string` (types.md "String Coercion"): a
    /// number, bool, vector, rotation, color or player value wired into a
    /// `string`-typed destination (an annotated `out`, a string `var`, a
    /// string parameter) goes through one `FormatText("{0}")`, so it arrives
    /// as its text rather than relying on the port to convert it.
    fn wrap_to_string(&mut self, src: PortRef, dst: PortRef) -> Option<PortRef> {
        let src_range = {
            let n = self.find_node(src.node_id)?;
            let formats = matches!(
                n.ports.find_output(src.port.sym())?.ty,
                Type::Bool
                    | Type::Int
                    | Type::Float
                    | Type::Vector
                    | Type::Rotator
                    | Type::Color
                    | Type::Entity
                    | Type::Character
                    | Type::Controller
            );
            if !formats {
                return None;
            }
            n.source_range.clone()
        };
        {
            let n = self.find_node(dst.node_id)?;
            if n.ports.find_input(dst.port.sym())?.ty != Type::String {
                return None;
            }
        }
        Some(crate::lower::ops::build_format_text(self, "{0}".to_string(), vec![src], &src_range))
    }

    /// Run `f` with `nofold_depth` incremented for its duration when `active`
    /// is true (the enclosing declaration was `@nofold`-annotated). Every
    /// gate/event `f` creates — directly or through nested lowering calls —
    /// gets the `_nofold` pseudo-property stamped by `add_gate`/`add_event`.
    /// Safe against early `return`s inside `f`: a `return` inside a closure
    /// only unwinds the closure, so the decrement below always runs.
    pub(super) fn with_nofold<R>(
        &mut self,
        active: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if active {
            self.nofold_depth += 1;
        }
        let r = f(self);
        if active {
            self.nofold_depth -= 1;
        }
        r
    }

    pub(super) fn warn(&mut self, msg: impl Into<String>, range: &SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Warning,
            code: "WSP001".into(),
            message: msg.into(),
            range: range.clone(),
        });
    }

    pub(super) fn warn_code(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        range: &SourceRange,
    ) {
        self.diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Warning,
            code: code.into(),
            message: msg.into(),
            range: range.clone(),
        });
    }

    pub(super) fn error(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        range: &SourceRange,
    ) {
        self.diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Error,
            code: code.into(),
            message: msg.into(),
            range: range.clone(),
        });
    }

    pub(super) fn lookup_var(&self, name: &str) -> Option<&VarRecord> {
        match self.scope.get(name) {
            Some(Binding::Var(r)) => Some(r),
            _ => None,
        }
    }

    /// `lookup_var`, but only when the var is bound in the CURRENT (innermost)
    /// scope frame. Lets a statement-position redeclaration tell "a body
    /// pre-pass already declared this var here" (reuse + reset) from "a
    /// same-named var exists only in an ANCESTOR frame" (a distinct variable
    /// being shadowed, which needs its own fresh storage gate).
    pub(super) fn lookup_var_current_frame(&self, name: &str) -> Option<&VarRecord> {
        match self.scope.get_current(name) {
            Some(Binding::Var(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_local(&self, name: &str) -> Option<&LocalRecord> {
        match self.scope.get(name) {
            Some(Binding::Local(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_buffer(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(name) {
            Some(Binding::Buffer(r)) => Some(r),
            _ => None,
        }
    }

    /// `lookup_buffer`, restricted to the CURRENT (innermost) scope frame — the
    /// buffer counterpart of [`lookup_var_current_frame`]. A statement-position
    /// `buffer b = …` shadowing an ancestor buffer must create its OWN gate, or
    /// its initializer is wired into the outer buffer's `Input` (two drivers =
    /// a load-breaking fan-in) instead of a fresh buffer of its own.
    pub(super) fn lookup_buffer_current_frame(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get_current(name) {
            Some(Binding::Buffer(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_input(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(name) {
            Some(Binding::Input(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_output(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(&output_scope_key(name)) {
            Some(Binding::Output(r)) => Some(r),
            _ => None,
        }
    }

    /// The pin of a payload-free enum output, which crosses the top-level
    /// boundary as one wire, its tag (see `pre_declare_output`).
    pub(super) fn output_tag(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(&output_scope_key(name)) {
            Some(Binding::Record(fields)) if fields.len() == 1 => match enum_disc(fields) {
                Some(Binding::Output(r)) => Some(r),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn output_count(&self) -> usize {
        self.scope
            .iter_within(crate::scope::ScopeTag::MODULE)
            .filter(|(_, b)| matches!(b, Binding::Output(_)))
            .count()
    }

    pub(super) fn first_output(&self) -> Option<(&str, &NodeRecord)> {
        self.scope
            .iter_within(crate::scope::ScopeTag::MODULE)
            .find_map(|(k, b)| match b {
                Binding::Output(r) => Some((k, r)),
                _ => None,
            })
    }

    /// Every output node in declaration order, for wiring a tuple return
    /// positionally onto a mod's named outputs.
    pub(super) fn ordered_outputs(&self) -> Vec<NodeRecord> {
        let mut outs: Vec<NodeRecord> = self
            .scope
            .iter_within(crate::scope::ScopeTag::MODULE)
            .filter_map(|(_, b)| match b {
                Binding::Output(r) => Some(r.clone()),
                _ => None,
            })
            .collect();
        // Output nodes are created in declaration order; the scope frame is a
        // map, so its own order is not that.
        outs.sort_by_key(|o| o.node_id);
        outs
    }

    pub(super) fn lookup_chip(&self, name: &str) -> Option<&std::sync::Arc<ChipDecl>> {
        match self.scope.get(name) {
            Some(Binding::Chip(c)) => Some(c),
            _ => None,
        }
    }

    /// The user `mod`/`chip` a bare call to `name` written in `call_file`
    /// calls: the one in scope, when the calling file can name it (see
    /// [`crate::ast::ImportScope::user_mod_answers`], which typecheck decides
    /// by too).
    pub(super) fn call_target_chip(&self, name: &str, call_file: &str) -> Option<&std::sync::Arc<ChipDecl>> {
        self.lookup_chip(name).filter(|d| self.imports.user_mod_answers(&d.range, call_file))
    }

    /// The `self` mod a method call `x.name()` written in `call_file` calls:
    /// one in scope whose first parameter is `self`, when the calling file
    /// can name it (see [`crate::ast::ImportScope::user_mod_answers`], which
    /// typecheck decides by too).
    pub(super) fn self_mod_chip(&self, name: &str, call_file: &str) -> Option<&std::sync::Arc<ChipDecl>> {
        self.lookup_chip(name)
            .filter(|d| d.is_self_receiver() && self.imports.user_mod_answers(&d.range, call_file))
    }

    pub(super) fn lookup_ns_chip(&self, ns: &str, name: &str) -> Option<&std::sync::Arc<ChipDecl>> {
        match self.scope.get(ns) {
            Some(Binding::Namespace(members)) => match members.get(name) {
                Some(Binding::Chip(c)) => Some(c),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The storage node of every var `binding` holds, its record fields' included.
fn var_nodes(binding: &Binding, out: &mut Vec<NodeId>) {
    match binding {
        Binding::Var(v) => out.push(v.node_id),
        Binding::Record(fields) => fields.values().for_each(|b| var_nodes(b, out)),
        _ => {}
    }
}

pub(super) fn reset_var_get_caches(ctx: &mut LowerCtx) {
    // `take`, not `clear`: clearing costs the table's capacity on every reset.
    std::mem::take(&mut ctx.var_gets);
}

pub(super) fn invalidate_var_cache(ctx: &mut LowerCtx, target_node_id: &NodeId) {
    ctx.var_gets.remove(target_node_id);
}

/// The cached reads before a set of branches. `lower_if` takes one so that, at
/// the join, only the vars a branch actually TOUCHED are invalidated — an
/// unwritten var's pre-branch read survives, instead of the whole cache being
/// blanket-cleared.
pub(super) type VarCacheSnapshot = HashMap<NodeId, NodeId>;

pub(super) fn snapshot_var_caches(ctx: &LowerCtx) -> VarCacheSnapshot {
    ctx.var_gets.clone()
}

/// The vars whose cached read CHANGED versus `snap` — every var a branch
/// invalidated, no matter which code path cleared it (a direct write, a
/// nested `if`'s join, an inline mod's blanket reset, or a chip instance's
/// ref-arg reset). Comparing actual cache state — rather than trusting each
/// write site to journal — is what makes the branch merge safe: a stale
/// pre-branch read can only survive when the cache is provably unchanged. A
/// var uncached in `snap` is left out: after a restore it is uncached anyway.
pub(super) fn cache_touched_since(ctx: &LowerCtx, snap: &VarCacheSnapshot) -> HashSet<NodeId> {
    snap.iter().filter(|(var, get)| ctx.var_gets.get(var) != Some(get)).map(|(var, _)| *var).collect()
}

/// Put the cached reads back to `snap` — used to discard a branch's scratch
/// cache state (its own fresh reads live on that branch's exec chain and can't
/// be reused elsewhere) before lowering the next branch and after the join.
pub(super) fn restore_var_caches(ctx: &mut LowerCtx, snap: &VarCacheSnapshot) {
    ctx.var_gets.clone_from(snap);
}

/// `t` written as the annotation that resolves to it. A record is written as
/// its fields (a type has no alias name to give back), and an enum by name.
pub(super) fn type_expr_of(t: &Type, range: &SourceRange) -> crate::ast::TypeExpr {
    use crate::ast::TypeExpr;
    let name = |n: &str| TypeExpr::Name { name: n.to_string(), range: range.clone() };
    let boxed = |t: &Type| Box::new(type_expr_of(t, range));
    match t {
        Type::Component(class) if class.is_empty() => name("Component"),
        Type::Component(class) => TypeExpr::Generic {
            name: "Component".into(),
            args: vec![name(&crate::catalog::component_name(class))],
            range: range.clone(),
        },
        Type::Bool => name("bool"),
        Type::Int => name("int"),
        Type::Float => name("float"),
        Type::String => name("string"),
        Type::Vector => name("vector"),
        Type::Rotator => name("rotator"),
        Type::Quat => name("quat"),
        Type::Color => name("color"),
        Type::Entity => name("entity"),
        Type::Character => name("character"),
        Type::Controller => name("controller"),
        Type::Brick => name("brick"),
        Type::Zone => name("zone"),
        Type::Teleport => name("teleport"),
        Type::PrefabRef => name("prefab"),
        Type::Exec => name("exec"),
        Type::Never => name("never"),
        Type::Any | Type::Opaque => name("any"),
        Type::Param(p) => name(p),
        Type::Ref(inner) => TypeExpr::Ref { inner: boxed(inner), range: range.clone() },
        Type::Array(inner) => TypeExpr::Array { inner: boxed(inner), range: range.clone() },
        Type::Union(opts) => TypeExpr::Union {
            options: opts.iter().map(|o| type_expr_of(o, range)).collect(),
            range: range.clone(),
        },
        Type::Tuple(elems) => TypeExpr::Tuple {
            fields: elems.iter().map(|e| type_expr_of(e, range)).collect(),
            range: range.clone(),
        },
        Type::Record(fields) | Type::Outputs(fields) => TypeExpr::Record {
            fields: fields
                .iter()
                .map(|(n, t)| crate::ast::RecordTypeField {
                    name: n.clone(),
                    typ: type_expr_of(t, range),
                    range: range.clone(),
                })
                .collect(),
            range: range.clone(),
        },
        Type::Map(k, v) => TypeExpr::Generic {
            name: "Map".to_string(),
            args: vec![type_expr_of(k, range), type_expr_of(v, range)],
            range: range.clone(),
        },
        Type::Enum { name: n, args } if args.is_empty() => name(n),
        Type::Enum { name: n, args } => TypeExpr::Generic {
            name: n.clone(),
            args: args.iter().map(|a| type_expr_of(a, range)).collect(),
            range: range.clone(),
        },
    }
}
