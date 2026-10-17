//! Intermediate representation.
//!
//! A [`Module`] is a graph: nodes (gate instances) + wires (port-to-port
//! connections). Nested chips are child Modules addressed by the parent's
//! chip-node id. Everything is flat + explicit so the IR is trivially
//! serialisable and round-trips through test goldens.
//!
//! reference implementation. Types module is fused in here (see `Type`
//! enum) since Phase 1 doesn't need unification or tvars — those arrive
//! in the typecheck phase.

pub mod build;
pub mod builder;
pub mod gate_variants;
pub mod gate_class;
pub(crate) mod module_tree;
pub mod plane;
pub mod port_registry;
pub mod program;
pub mod validate;

pub use builder::{BuildError, ModuleRef, ProgramBuilder};
pub use validate::{PortSide, ValidationError};
pub use program::{
    HANDLE_TARGET, HANDLE_TARGETS, LayoutMode, MICROCHIP_BRICK, Program, ProgramSettings, SourceComment, SourceMap,
};

use crate::collections::HashMap;

use crate::intern::{sym, Sym};
use crate::ir::port_registry::WirePort;

/// Cheap numeric node identity — every node gets a globally unique u32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

thread_local! {
    /// Node ids are minted from a PER-THREAD counter, reset at the start of
    /// each compile (see [`NodeId::reset_for_compile`]).
    ///
    /// A process-global counter made output depend on how many compiles had
    /// already run: the same source compiled twice in one process produced
    /// different ids, and through them different brick order and a different
    /// byte length, so no test could compare two compilations. A compile runs
    /// its whole pipeline on one dedicated thread, so per-thread is per-compile
    /// without a lock, and two concurrent compiles still cannot collide.
    static NODE_COUNTER: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

impl NodeId {
    pub fn fresh() -> Self {
        NODE_COUNTER.with(|c| {
            let id = c.get();
            c.set(id + 1);
            Self(id)
        })
    }

    /// The id the next [`NodeId::fresh`] returns, without taking it: every
    /// node made after this call has an id at least this large.
    pub fn next() -> Self {
        NODE_COUNTER.with(|c| Self(c.get()))
    }

    /// Number this compile's nodes from 1, restoring the caller's numbering
    /// when the returned guard drops.
    ///
    /// Taken once at the head of a compile. A nested prefab compile takes one
    /// too: on native it runs on its own thread and the save/restore is a
    /// no-op, but on wasm32 it runs inline, where restoring is what stops it
    /// renumbering the outer compile's remaining nodes onto ids already used.
    pub fn compile_scope() -> NodeIdScope {
        NodeIdScope(NODE_COUNTER.with(|c| c.replace(1)))
    }

    pub fn port(self, port: WirePort) -> PortRef {
        PortRef {
            node_id: self,
            port,
        }
    }
}

/// Restores the node numbering that was current when it was taken. See
/// [`NodeId::compile_scope`].
pub struct NodeIdScope(u32);

impl Drop for NodeIdScope {
    fn drop(&mut self) {
        NODE_COUNTER.with(|c| c.set(self.0));
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "n{}", self.0)
    }
}

/// Lexical scope id. A module seeds `ScopeId(0)` as its root; every
/// subsequent scope (handler body, chip body, if branches, etc.) gets a
/// fresh id. Nodes record which scope they were lowered under via
/// `Node.scope_id`, and layout uses the `Module.scopes` table to group
/// them into nested regions.
pub type ScopeId = u32;

/// Root scope of every `Module`. Reserved; `Module::new` always seeds it.
pub const ROOT_SCOPE_ID: ScopeId = 0;

/// Kind of a lexical scope, with per-kind payload inlined so layout can
/// switch on the whole scope without consulting sibling `Option` fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    /// Top-level of a non-chip `Module`.
    ModuleRoot,
    /// Top-level of a chip's interior `Module`.
    ChipBody { name: String },
    /// Body of a `handler Trigger { ... }` declaration.
    HandlerBody { trigger_label: String },
    /// Synthetic wrapper around the three sub-scopes of an `if` so compose
    /// sees branches as a unit. Exactly one of `IfCond`/`IfThen`/`IfElse`
    /// children may lack a paired sibling, but they are always wrapped.
    IfGroup,
    /// `if (cond)` — the expression's lowered nodes live here.
    IfCond,
    /// `then { ... }` block.
    IfThen,
    /// `else { ... }` block.
    IfElse,
    /// A generic `{ ... }` block with no special semantics.
    Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScopeInfo {
    pub kind: ScopeKind,
    pub source_range: SourceRange,
    pub parent: Option<ScopeId>,
}

impl ScopeInfo {
    /// A module's root scope (`ROOT_SCOPE_ID`) of `kind`: no parent and no
    /// source range.
    pub fn root(kind: ScopeKind) -> Self {
        Self { kind, source_range: SourceRange::default(), parent: None }
    }
}

/// Primitive types tracked on ports + wires for compatibility checks.
/// A subset of the TS `Type` ADT — enough for Phase 1 emit + layout.
/// Extended with `Ref` / `Array` / etc. in Phase 3 (typecheck).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Bool,
    Int,
    Float,
    String,
    Vector,
    Rotator,
    Quat,
    Color,
    Entity,
    Character,
    Controller,
    /// A brick, held as a value: stored, copied, compared and sent. Nothing in
    /// the language creates one (the editor or a port supplies it) and no gate
    /// consumes one.
    Brick,
    /// Opaque reference to a Zone brick's `AttachedZone` (the `ZoneRef` output,
    /// consumed by the `Zone` input on zone events / `fillFromZone*`). Like a
    /// var ref, it is a *reference*, not a value: it can only be wired or
    /// rerouted — never stored in a variable, selected, or operated on.
    Zone,
    /// Opaque reference to a Teleport Destination (the `TeleportDestination`
    /// gate's `ThisDestination` output, consumed by `Teleport`/`RelativeTeleport`
    /// destination/source ports). Reference-only, exactly like [`Type::Zone`].
    /// Teleporting to a raw position uses `SetLocation`, not a teleport point.
    Teleport,
    /// A compile-time-constant reference to a prefab (`$./file.brz` or a
    /// nested `prefab { ... }` block). Resolved entirely during compilation —
    /// it is never a runtime wire value and never stored in a variable;
    /// reference-only, exactly like [`Type::Zone`]/[`Type::Teleport`].
    PrefabRef,
    Exec,
    Any,
    /// The result of the `Opaque(...)` builtin — an intentionally-untyped
    /// probe value used to defeat constant folding in test circuits.
    /// Behaves exactly like `Any` everywhere (assignability, coercion,
    /// unification, display) *except* operator resolution
    /// (`catalog::operators::type_kind_matches`), which wildcards on
    /// `Opaque` specifically. `Any` itself is the generic error-fallback /
    /// unknown type produced by ~150 other sites (void array methods,
    /// unresolved calls, dynamic-access fallbacks) and must NOT satisfy
    /// operator overloads — only a real `Opaque(...)` probe should.
    Opaque,
    Never,
    Ref(Box<Type>),
    Array(Box<Type>),
    Union(Vec<Type>),
    Tuple(Vec<Type>),
    /// Record / struct; Phase 1 keeps this untyped inner (map of name->Type).
    Record(Vec<(String, Type)>),
    /// The outputs of one gate or call read as a value: a builtin or
    /// container method with several outputs (`fs.max()`, `Edge(b)`), an
    /// event called as an expression, or a call of a `mod` with several
    /// outputs. Named like a record's fields, the first being the output a
    /// place for one value reads. Made only by [`Type::call_result`]; lowering
    /// reads it from typecheck's type of each expression, per instantiation.
    Outputs(Vec<(String, Type)>),
    /// `Map<K, V>` — a keyed variable collection (parallels `Array`). The key
    /// and value types are tracked for the source language; at the wire level a
    /// map is an opaque `MapVarRef`, so both sides are `any`-compatible.
    Map(Box<Type>, Box<Type>),
    /// An unsubstituted generic type parameter (e.g. `T`). Substituted to a
    /// concrete type by monomorphization; must never reach emit.
    Param(String),
    /// A user-declared tagged union (`enum Option<T> { Some(T), None }`),
    /// identified nominally by declaration name: two enums are equal iff
    /// their `name`s match and their `args` (generic type arguments, e.g.
    /// `Option<int>`'s `[Int]`) are pairwise equal. Never coerces/widens to a
    /// different enum, even a structurally identical one.
    Enum { name: String, args: Vec<Type> },
    /// A reference to one component on a brick, of the component type it
    /// names (`Component_PointLight`): what Get/Add Component give. Its
    /// properties read and write as fields (`light.Brightness`). Like a brick
    /// it is wired, not stored.
    Component(String),
}

/// What a reference value names. Refs compare only within one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// A `*T`: a var's `VarRef`.
    Var,
    /// An array's `ArrayVarRef`. An `int[]` value is always one.
    Array,
    /// A map's `MapVarRef`.
    Map,
    /// A `brick`.
    Brick,
}

impl Type {
    /// Whether this type already carries a reference: a `ref T`, or an array/map
    /// (which wire an `ArrayVarRef`/`MapVarRef` port). Taking `&` of such a value
    /// is a no-op, and wrapping one in another `Ref` (`*T[]`) is redundant and
    /// drops writes. Distinct from the `is_reference_type` helper, which is the
    /// reference-ONLY set (`ref`/zone/teleport/prefab — not storable/selectable).
    pub fn is_reference_backed(&self) -> bool {
        matches!(self, Type::Ref(_) | Type::Array(_) | Type::Map(_, _))
    }

    /// The kind of reference this value is, or `None` for an ordinary value.
    /// `*int[]` (a var holding an array ref) is an array ref.
    pub fn ref_kind(&self) -> Option<RefKind> {
        match self {
            Type::Ref(inner) => match inner.as_ref() {
                Type::Array(_) => Some(RefKind::Array),
                Type::Map(_, _) => Some(RefKind::Map),
                _ => Some(RefKind::Var),
            },
            Type::Array(_) => Some(RefKind::Array),
            Type::Map(_, _) => Some(RefKind::Map),
            Type::Brick => Some(RefKind::Brick),
            _ => None,
        }
    }

    /// Whether a `*T` value may point at a `T`: the scalar value types one
    /// `Var` gate holds. A record or enum spans several gates, and the
    /// reference-only types and refs themselves have no var to point at.
    pub fn is_ref_target(&self) -> bool {
        matches!(
            self,
            Type::Bool
                | Type::Int
                | Type::Float
                | Type::String
                | Type::Vector
                | Type::Rotator
                | Type::Quat
                | Type::Color
                | Type::Entity
                | Type::Character
                | Type::Controller
                | Type::Brick
        )
    }

    /// `*int[]` and `int[]` are the same value: an array or map is only ever
    /// handled by reference, so a var holding an array ref reads as the array
    /// type itself and is used directly (`cur.push(v)`). Every other type is
    /// returned unchanged.
    pub fn container_ref_as_value(self) -> Type {
        match self {
            Type::Ref(inner) if matches!(*inner, Type::Array(_) | Type::Map(_, _)) => *inner,
            other => other,
        }
    }

    /// The named fields of a record, or the outputs of a gate read.
    pub fn fields(&self) -> Option<&Vec<(String, Type)>> {
        match self {
            Type::Record(fields) | Type::Outputs(fields) => Some(fields),
            _ => None,
        }
    }

    /// The value of a call, from the outputs its callee declares (a builtin's
    /// ports, a `mod`'s or `chip`'s outputs, an event's data, a container
    /// method's results): the type of a lone output, or for several the
    /// outputs of one gate read. A `mod` returning one record gives that
    /// record. Every call result typed from a callee's outputs is made here.
    pub fn call_result(outputs: Vec<(String, Type)>) -> Type {
        match outputs.len() {
            0 => Type::Never,
            1 => outputs.into_iter().next().map_or(Type::Never, |(_, t)| t),
            _ => Type::outputs(outputs),
        }
    }

    /// `fields` as the outputs of a gate read, unless they are a tuple's
    /// positions or a single value.
    pub fn outputs(fields: Vec<(String, Type)>) -> Type {
        if fields.iter().enumerate().all(|(i, (k, _))| *k == i.to_string()) {
            Type::Record(fields)
        } else {
            Type::Outputs(fields)
        }
    }

    /// This type with every gate read in it held as the record of its
    /// outputs.
    pub fn as_record(&self) -> Type {
        let fields = |fs: &[(String, Type)]| fs.iter().map(|(k, t)| (k.clone(), t.as_record())).collect();
        match self {
            Type::Outputs(fs) | Type::Record(fs) => Type::Record(fields(fs)),
            Type::Ref(t) => Type::Ref(Box::new(t.as_record())),
            Type::Array(t) => Type::Array(Box::new(t.as_record())),
            Type::Map(k, v) => Type::Map(Box::new(k.as_record()), Box::new(v.as_record())),
            Type::Union(ts) => Type::Union(ts.iter().map(Type::as_record).collect()),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(Type::as_record).collect()),
            Type::Enum { name, args } => {
                Type::Enum { name: name.clone(), args: args.iter().map(Type::as_record).collect() }
            }
            other => other.clone(),
        }
    }

    /// What a `var` written with no type holds of a value of this type: a
    /// gate read is its first output, in a record's fields, a tuple's
    /// positions and an array's elements too, as the annotated `var` holds it.
    pub fn held_by_var(&self) -> Type {
        match self {
            Type::Outputs(fs) if !fs.is_empty() => unwrap_ref(&fs[0].1).held_by_var(),
            Type::Record(fs) => Type::Record(fs.iter().map(|(k, t)| (k.clone(), t.held_by_var())).collect()),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(Type::held_by_var).collect()),
            Type::Array(t) => Type::Array(Box::new(t.held_by_var())),
            other => other.clone(),
        }
    }

    /// Whether a gate read is held anywhere in this type.
    pub fn holds_outputs(&self) -> bool {
        match self {
            Type::Outputs(_) => true,
            Type::Record(fs) => fs.iter().any(|(_, t)| t.holds_outputs()),
            Type::Ref(t) | Type::Array(t) => t.holds_outputs(),
            Type::Map(k, v) => k.holds_outputs() || v.holds_outputs(),
            Type::Union(ts) | Type::Tuple(ts) | Type::Enum { args: ts, .. } => ts.iter().any(Type::holds_outputs),
            _ => false,
        }
    }
}

/// Strip a single leading `Ref` layer. A `*T` param resolves to
/// `Ref(Param(T))`, but a var passed to it infers to its already-auto-derefed
/// inner type, so the two are asymmetric — unwrap both before collecting.
pub fn unwrap_ref(t: &Type) -> Type {
    match t {
        Type::Ref(inner) => inner.as_ref().clone(),
        other => other.clone(),
    }
}

/// A float constant as a diagnostic quotes it: a whole number from `1e10` up
/// in exponent form, as such a number is usually written (`1e30`, not 31
/// digits).
pub fn written_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() >= 1e10 {
        format!("{f:e}")
    } else if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

/// The source-language spelling of a type. Compound variants render by
/// recursion (`*int`, `int[]`, `Map<string, int>`, …), which is exactly why
/// this is `Display` and not `AsRef<str>`: those strings are built on the fly,
/// so there is no `&str` inside the value to borrow. `analysis::type_str` is a
/// thin `to_string()` alias over this.
impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Component(class) if class.is_empty() => f.write_str("Component"),
            Type::Component(class) => write!(f, "Component<{}>", crate::catalog::component_name(class)),
            Type::Bool => f.write_str("bool"),
            Type::Int => f.write_str("int"),
            Type::Float => f.write_str("float"),
            Type::String => f.write_str("string"),
            Type::Vector => f.write_str("vector"),
            Type::Rotator => f.write_str("rotator"),
            Type::Quat => f.write_str("quat"),
            Type::Color => f.write_str("color"),
            Type::Entity => f.write_str("entity"),
            Type::Character => f.write_str("character"),
            Type::Controller => f.write_str("controller"),
            Type::Brick => f.write_str("brick"),
            Type::Zone => f.write_str("zone"),
            Type::Teleport => f.write_str("teleport"),
            Type::PrefabRef => f.write_str("prefab"),
            Type::Exec => f.write_str("exec"),
            // `Opaque` behaves like `any` everywhere except operator resolution,
            // and that distinction isn't user-facing.
            Type::Any | Type::Opaque => f.write_str("any"),
            Type::Never => f.write_str("never"),
            Type::Ref(inner) => write!(f, "*{inner}"),
            // `(*int)[]` is an array of refs; `*int[]` is a ref to an array.
            Type::Array(inner) if matches!(inner.as_ref(), Type::Ref(_)) => write!(f, "({inner})[]"),
            Type::Array(inner) => write!(f, "{inner}[]"),
            Type::Map(k, v) => write!(f, "Map<{k}, {v}>"),
            Type::Param(n) => f.write_str(n),
            Type::Enum { name, args } => {
                write!(f, "{name}")?;
                if !args.is_empty() {
                    write!(
                        f,
                        "<{}>",
                        args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
                    )?;
                }
                Ok(())
            }
            Type::Union(opts) => {
                for (i, o) in opts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{o}")?;
                }
                Ok(())
            }
            Type::Tuple(fields) => {
                f.write_str("(")?;
                for (i, ft) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{ft}")?;
                }
                f.write_str(")")
            }
            Type::Outputs(_) => write!(f, "{}", self.as_record()),
            // A record whose keys are 0, 1, 2, … is a tuple's field map — render
            // it as a tuple, matching how it was written.
            Type::Record(fields) => {
                let is_tuple = !fields.is_empty()
                    && fields
                        .iter()
                        .enumerate()
                        .all(|(i, (n, _))| n == &i.to_string());
                if is_tuple {
                    f.write_str("(")?;
                    for (i, (_, ft)) in fields.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{ft}")?;
                    }
                    f.write_str(")")
                } else {
                    f.write_str("{")?;
                    for (i, (n, ft)) in fields.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{n}: {ft}")?;
                    }
                    f.write_str("}")
                }
            }
        }
    }
}

/// A literal used to initialise non-wired component properties.
///
/// Mirrors the TS `Literal` discriminated union 1:1. Phase 1 lowers these
/// straight to brdb values in the emitter.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Vector {
        x: f64,
        y: f64,
        z: f64,
    },
    Rotator {
        pitch: f64,
        yaw: f64,
        roll: f64,
    },
    Quat {
        x: f64,
        y: f64,
        z: f64,
        w: f64,
    },
    Color {
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    /// Linear RGBA color (0–1 components) — the wire `LinearColor` variant
    /// member, produced by folding a constant `Color(r, g, b, a?)` call.
    LinearColor {
        r: f64,
        g: f64,
        b: f64,
        a: f64,
    },
    /// Null object reference — used for entity/controller/character var defaults.
    Object,
    /// A list of literals — used to carry an array's constant initial contents
    /// (`var foo: int[] = [1, 2, 3]`) to the emitter.
    Array(Vec<Literal>),
    /// A map's constant initial entries (`map m = { :a => 1 }`) for the emitter.
    Map(Vec<(Literal, Literal)>),
    /// A compile-time-only record built by a `const` record literal (`{ rooms:
    /// 2, timer: 60 }`) or returned from a `const mod`. Unlike `Array`/`Map`,
    /// this has **no wire representation** — a record exists only to be
    /// consumed by field access during const evaluation, never emitted. It
    /// must never reach the emitter; see the `unreachable!` in
    /// `emit::variants` for why.
    Record(Vec<(String, Literal)>),
    /// External asset reference `$AssetType/AssetName`. The emitter registers it
    /// in the world's external-asset table and writes the resulting index.
    Asset {
        asset_type: String,
        asset_name: String,
    },
    /// Prefab file reference `$./file.brz` / `$/abs/file.brz`. The emitter
    /// resolves `path` to `.brz` bytes, embeds them via `World::add_prefab`,
    /// and writes the resulting `Prefabs/Uploads/…` path into the gate's
    /// `Prefab` path string.
    PrefabRef { path: String },
    /// Inline nested-prefab source (the `$``` ... ``` ` form). The emitter
    /// will compile `source` and embed it the same way `PrefabRef` embeds a
    /// `.brz` file (at emit); for now it just carries the raw source.
    NestedPrefab { source: String },
    /// A prefab's `.brz` bytes, embedded as they are. What a `NestedPrefab`
    /// becomes once compiled, so a serialized program needs no front end.
    PrefabBytes(std::sync::Arc<[u8]>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PortSpec {
    pub name: Sym,
    pub ty: Type,
}

impl PortSpec {
    pub fn new(name: Sym, ty: Type) -> Self {
        Self { name, ty }
    }

    /// A control-flow port. `Type::Exec` carries no value, so the name is the
    /// only thing that distinguishes one of these from another.
    pub fn exec(name: Sym) -> Self {
        Self {
            name,
            ty: Type::Exec,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GateIO {
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
}

impl GateIO {
    /// Ports of a pure source: nothing in, one `Output` carrying `ty`. The
    /// shape of a `Literal`, of a folded constant, and of the gates that stand
    /// in for one.
    pub fn source(ty: Type) -> Self {
        Self {
            inputs: vec![],
            outputs: vec![PortSpec::new(*sym::OUTPUT, ty)],
        }
    }

    /// Ports of a chip pin carrying `ty`: `RER_Input` in, `RER_Output` out.
    pub fn pin(ty: Type) -> Self {
        Self {
            inputs: vec![PortSpec::new(*sym::RER_INPUT, ty.clone())],
            outputs: vec![PortSpec::new(*sym::RER_OUTPUT, ty)],
        }
    }

    /// Ports of the storage gate behind a `var` of `inner`: it holds the value
    /// and hands out the ref that [`GateIO::var_read`] and
    /// [`GateIO::var_write`] chain through.
    pub fn var_storage(inner: Type) -> Self {
        Self {
            inputs: vec![],
            outputs: vec![
                PortSpec::new(*sym::VALUE, inner.clone()),
                PortSpec::new(*sym::VAR_REF, Type::Ref(Box::new(inner))),
            ],
        }
    }

    /// Ports of an exec-driven read of a `var` of `inner` (`Var_Get`): the
    /// value comes out alongside the exec continuation.
    pub fn var_read(inner: Type) -> Self {
        Self {
            inputs: vec![
                PortSpec::exec(*sym::EXEC),
                PortSpec::new(*sym::VAR_REF, Type::Ref(Box::new(inner.clone()))),
            ],
            outputs: vec![
                PortSpec::new(*sym::VALUE, inner),
                PortSpec::exec(*sym::EXEC_OUT),
            ],
        }
    }

    /// Ports of an exec-driven write to a `var` of `inner`. Shared by
    /// `Var_Set` and `Var_Increment`, which differ only in what they do with
    /// the incoming `Value`.
    pub fn var_write(inner: Type) -> Self {
        Self {
            inputs: vec![
                PortSpec::exec(*sym::EXEC),
                PortSpec::new(*sym::VAR_REF, Type::Ref(Box::new(inner.clone()))),
                PortSpec::new(*sym::VALUE, inner),
            ],
            outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
        }
    }

    /// Ports of the storage gate behind an array of `elem`: the ref its array
    /// ops chain through.
    pub fn array_storage(elem: Type) -> Self {
        Self {
            inputs: vec![],
            outputs: vec![PortSpec::new(*sym::ARRAY_VAR_REF, Type::Ref(Box::new(Type::Array(Box::new(elem)))))],
        }
    }

    /// Ports of the storage gate behind a map of type `map_type`: the ref its
    /// map ops chain through.
    pub fn map_storage(map_type: Type) -> Self {
        Self { inputs: vec![], outputs: vec![PortSpec::new(*sym::MAP_VAR_REF, Type::Ref(Box::new(map_type)))] }
    }

    /// Ports of `Union`: two exec inputs merged into one continuation.
    pub fn exec_join() -> Self {
        Self {
            inputs: vec![PortSpec::exec(*sym::EXEC_A), PortSpec::exec(*sym::EXEC_B)],
            outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
        }
    }

    /// Ports of `Branch`: one exec in, one of two continuations out by
    /// `bCond`.
    pub fn exec_branch() -> Self {
        Self {
            inputs: vec![
                PortSpec::exec(*sym::EXEC),
                PortSpec::new(*sym::B_COND, Type::Bool),
            ],
            outputs: vec![
                PortSpec::exec(*sym::EXEC_OUT_A),
                PortSpec::exec(*sym::EXEC_OUT_B),
            ],
        }
    }

    /// Ports of `Logical_Not`.
    pub fn bool_not() -> Self {
        Self {
            inputs: vec![PortSpec::new(*sym::B_INPUT, Type::Bool)],
            outputs: vec![PortSpec::new(*sym::B_OUTPUT, Type::Bool)],
        }
    }

    pub fn all_port_names(&self) -> impl Iterator<Item = Sym> + '_ {
        self.inputs
            .iter()
            .chain(self.outputs.iter())
            .map(|p| p.name)
    }

    pub fn find_input(&self, name: Sym) -> Option<&PortSpec> {
        self.inputs.iter().find(|p| p.name == name)
    }

    pub fn find_output(&self, name: Sym) -> Option<&PortSpec> {
        self.outputs.iter().find(|p| p.name == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Gate,
    Event,
    Chip,
    Input,
    Output,
    /// Type-coercion adapter. Lowering never makes one and the backend does
    /// not emit one: [`Program::validate`] rejects it.
    Coerce,
}

impl NodeKind {
    /// Whether a node of this kind is a brick in its module's plane: a gate,
    /// an event, a pin, or a chip's shell. A `Coerce` node is not.
    pub fn has_brick(self) -> bool {
        matches!(self, NodeKind::Gate | NodeKind::Event | NodeKind::Input | NodeKind::Output | NodeKind::Chip)
    }
}

pub use crate::diagnostic::SourceRange;

use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Component class of the gate brick this IR node spawns.
    /// For `Chip` nodes this is `BrickComponentType_Internal_Microchip`; for
    /// `Input` / `Output` inside a chip body it's `...MicrochipInput` /
    /// `...MicrochipOutput`.
    pub gate_class: &'static str,
    /// Non-wired component values applied after the brick is spawned.
    /// Wrapped in Arc so template instantiation can clone without
    /// copying the entire map.
    pub properties: Arc<HashMap<Sym, Literal>>,
    /// Port specifications. Arc-shared across template instances.
    pub ports: Arc<GateIO>,
    pub source_range: SourceRange,
    /// For nodes living inside a chip body: the chip node's id.
    ///
    /// TODO(follow-up PR): this is redundant with `ScopeKind::ChipBody`
    /// walking `scope_id -> scopes[id] -> parent` in `Module.scopes`.
    /// Remove once no downstream code reads it.
    pub chip_id: Option<NodeId>,
    /// Layout-row hint. Nodes sharing a `chain_id` render in the same row
    /// (one exec chain = one row). `None` means "global" (row 0).
    pub chain_id: Option<u32>,
    /// Lexical scope this node was lowered under. Resolved against
    /// `Module.scopes`. Defaults to `ROOT_SCOPE_ID` for nodes built before
    /// the lowering pass assigns scopes.
    pub scope_id: ScopeId,
    /// Debug-only human-readable note.
    pub note: Option<&'static str>,
}

impl Node {
    /// The declared type of output `port`.
    pub fn output_type(&self, port: WirePort) -> Option<Type> {
        self.ports.find_output(port.sym()).map(|p| p.ty.clone())
    }

    /// A node `id` of `kind` and `gate_class`, placed like `near` (its source
    /// range, chip, exec-chain row and scope), or in the root scope with no
    /// source when `near` is `None`.
    pub fn new(
        id: NodeId,
        kind: NodeKind,
        gate_class: &'static str,
        ports: impl Into<Arc<GateIO>>,
        properties: impl Into<Arc<HashMap<Sym, Literal>>>,
        near: Option<&Node>,
        note: Option<&'static str>,
    ) -> Self {
        Self {
            id,
            kind,
            gate_class,
            properties: properties.into(),
            ports: ports.into(),
            source_range: near.map(|n| n.source_range.clone()).unwrap_or_default(),
            chip_id: near.and_then(|n| n.chip_id),
            chain_id: near.and_then(|n| n.chain_id),
            scope_id: near.map_or(ROOT_SCOPE_ID, |n| n.scope_id),
            note,
        }
    }

    /// True if this node is a stateful buffer/Var that legitimately
    /// participates in a cycle. Layout uses this to pick a deterministic
    /// feedback edge when breaking SCCs.
    ///
    /// New buffer-capable gate classes go here — this is the single
    /// source of truth consulted by layout.
    pub fn is_buffer(&self) -> bool {
        self.gate_class == gate_class::VARIABLE || self.gate_class == gate_class::BUFFER
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub node_id: NodeId,
    pub port: WirePort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wire {
    pub source: PortRef,
    pub target: PortRef,
}

/// How one property of a styled brick is set: a constant baked into its
/// Brick Property Changer's data, or a value port wired into the changer's
/// input at emit.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleValue {
    /// A `Literal::Color` (sRGB bytes) for a color, a `Literal::Int` (the
    /// `EBrickMaterial` value) for a material.
    Baked(Literal),
    Wired(PortRef),
}

impl StyleValue {
    /// The port a wired value reads, `None` for a baked one.
    pub fn wired(&self) -> Option<&PortRef> {
        match self {
            StyleValue::Wired(p) => Some(p),
            StyleValue::Baked(_) => None,
        }
    }
}

/// A constant color as the sRGB bytes a brick stores: a `Literal::Color`
/// (`ColorSRGB`) as written, a `Literal::LinearColor` (`Color`, linear 0-1)
/// encoded with the sRGB transfer curve, the way a linear color becomes
/// display bytes. Alpha is linear in both. `None` for any other literal.
pub fn brick_color(lit: &Literal) -> Option<Literal> {
    fn encode(c: f64) -> u8 {
        let c = if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
        let s = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    }
    fn linear(c: f64) -> u8 {
        let c = if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
        (c * 255.0).round() as u8
    }
    match lit {
        Literal::Color { .. } => Some(lit.clone()),
        Literal::LinearColor { r, g, b, a } => {
            Some(Literal::Color { r: encode(*r), g: encode(*g), b: encode(*b), a: linear(*a) })
        }
        _ => None,
    }
}

/// The root microchip shell's `@color` / `@material`. The shell has no IR
/// node to carry them as properties, so the root module holds them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RootStyle {
    pub color: Option<StyleValue>,
    pub material: Option<StyleValue>,
}

impl RootStyle {
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.material.is_none()
    }
}

/// A compiled wirescript module — the input to the emitter.
#[derive(Debug, Clone)]
pub struct Module {
    pub name: Sym,
    pub nodes: HashMap<NodeId, Node>,
    pub wires: Vec<Wire>,
    /// Chip sub-modules keyed by the chip node's id in this Module.
    pub chips: HashMap<NodeId, Module>,
    /// MicrochipInput node ids in declaration order.
    pub inputs: Vec<NodeId>,
    /// MicrochipOutput node ids in declaration order.
    pub outputs: Vec<NodeId>,
    /// Scope table for layout. `ROOT_SCOPE_ID` is always present and is
    /// this module's root (`ModuleRoot` by default; chip-body modules
    /// overwrite it with `ChipBody { name }` at construction time).
    pub scopes: HashMap<ScopeId, ScopeInfo>,
    /// Base template name for grid deduplication. When set, identical chip
    /// instances (same template_key) can reuse the first instance's
    /// serialized grid data instead of emitting duplicate grids.
    pub template_key: Option<Sym>,
    /// External node_ids referenced by this module's wires but NOT in its
    /// `nodes` map. Discovered by bottom-up diff after lowering. Used by
    /// template instantiation to remap external refs per instance.
    pub scope_captures: Vec<NodeId>,
    /// Dynamic labels: a host node (a `Pseudo_Var`) whose floating name label
    /// is driven at runtime by a wire instead of a baked string. Maps the host
    /// node to the (already string-coerced) value port that feeds its label
    /// component's `Text` input. The wire itself is added at emit time, once the
    /// host brick's id is known — see `emit.rs` Pass 3.5. Not an IR wire (the
    /// label `Component_TextDisplay` is not an IR node), so it does not affect
    /// layout or pruning.
    pub dynamic_labels: HashMap<NodeId, PortRef>,
    /// A module-level `@label(<constant>)` folded to static text — overrides the
    /// root microchip's default label (the module name) at emit. Root module
    /// only; `None` for chips.
    pub root_label_override: Option<String>,
    /// A module-level `@label(<runtime expr>)` lowered to a value port — wired
    /// into the ROOT microchip shell's label `Text` at emit (analogous to
    /// `dynamic_labels`, but the host is the root shell brick rather than an IR
    /// node). Root module only; `None` for chips.
    pub root_dynamic_label: Option<PortRef>,
    /// Runtime `@color(expr)`: a host node (a `Pseudo_Var`) whose brick color
    /// is driven by a wire. Maps the host to the color-typed value port that
    /// feeds the `Color` input of the Brick Property Changer emit puts on the
    /// host's brick (Pass 3.5). A constant `@color` is the host's `_color`
    /// property instead. Like `dynamic_labels`, not an IR wire.
    pub dynamic_colors: HashMap<NodeId, PortRef>,
    /// Runtime `@material(expr)`: as `dynamic_colors`, but the int-typed port
    /// (the `BrickMaterial` value's discriminant) feeds the changer's
    /// `Material` input. A constant one is the host's `_material` property.
    pub dynamic_materials: HashMap<NodeId, PortRef>,
    /// A module-level `@color` / `@material`, styling the ROOT microchip
    /// shell. Root module only; empty for chips.
    pub root_style: RootStyle,
    /// Names the text form gave this module's nodes (`%count`), kept so a
    /// program read from text writes back with the same names. Lowering
    /// leaves it empty; layout and emit never read it.
    pub value_names: HashMap<NodeId, Sym>,
}

impl Default for Module {
    fn default() -> Self {
        let mut scopes = HashMap::default();
        scopes.insert(ROOT_SCOPE_ID, ScopeInfo::root(ScopeKind::ModuleRoot));
        Self {
            name: crate::intern::intern_static(""),
            nodes: HashMap::default(),
            wires: Vec::new(),
            chips: HashMap::default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            scopes,
            template_key: None,
            scope_captures: Vec::new(),
            dynamic_labels: HashMap::default(),
            root_label_override: None,
            root_dynamic_label: None,
            dynamic_colors: HashMap::default(),
            dynamic_materials: HashMap::default(),
            root_style: RootStyle::default(),
            value_names: HashMap::default(),
        }
    }
}

impl Module {
    pub fn new(name: &str) -> Self {
        Self {
            name: crate::intern::intern(name),
            ..Default::default()
        }
    }

    /// Construct a module whose root scope is a chip body. The root is
    /// still `ROOT_SCOPE_ID`; its `ScopeInfo` is overwritten to
    /// `ChipBody { name }`.
    pub fn new_chip_body(module_name: &str, chip_name: impl Into<String>) -> Self {
        let mut m = Self::new(module_name);
        m.scopes.insert(ROOT_SCOPE_ID, ScopeInfo::root(ScopeKind::ChipBody { name: chip_name.into() }));
        m
    }

    /// Insert a node; returns its id so the caller can wire against it.
    pub fn add_node(&mut self, n: Node) -> NodeId {
        let id = n.id;
        self.nodes.insert(id, n);
        id
    }

    pub fn add_wire(&mut self, w: Wire) {
        self.wires.push(w);
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Every value port a runtime `@color` / `@material` in this module reads.
    /// Emit wires each into a Brick Property Changer, so like the
    /// `dynamic_labels` sources they have no IR consumer, and a pass that
    /// prunes unread nodes must keep them.
    pub fn style_sources(&self) -> impl Iterator<Item = &PortRef> + '_ {
        self.dynamic_colors
            .values()
            .chain(self.dynamic_materials.values())
            .chain(self.root_style.color.iter().filter_map(StyleValue::wired))
            .chain(self.root_style.material.iter().filter_map(StyleValue::wired))
    }

    /// The chip module `path` leads to: a chain of chip node ids from this
    /// module down. Panics when a step names no chip module.
    pub fn at(&self, path: &[NodeId]) -> &Module {
        path.iter().fold(self, |m, c| &m.chips[c])
    }

    /// [`Module::at`], mutably.
    pub fn at_mut(&mut self, path: &[NodeId]) -> &mut Module {
        path.iter().fold(self, |m, c| m.chips.get_mut(c).expect("a chip module on the path"))
    }

    /// Node `id`, in this module or any chip module below it. Node ids are
    /// unique across the tree, so the search order cannot change the answer.
    pub fn find_deep(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id).or_else(|| self.chips.values().find_map(|c| c.find_deep(id)))
    }

    /// Call `f` on this module and every chip module below it, each with its
    /// path from here: a parent before its chips, the chips in id order.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&[NodeId], &'a Module)) {
        fn go<'a>(m: &'a Module, path: &mut Vec<NodeId>, f: &mut impl FnMut(&[NodeId], &'a Module)) {
            f(path, m);
            let mut kids: Vec<NodeId> = m.chips.keys().copied().collect();
            kids.sort_unstable();
            for k in kids {
                path.push(k);
                go(&m.chips[&k], path, f);
                path.pop();
            }
        }
        go(self, &mut Vec::new(), f);
    }

    /// Every node in ascending id order, the order layout and emit visit them
    /// in. Layout and emit iterate nodes only through this: `nodes`' own
    /// iteration order depends on how the map was built, so a program built
    /// two ways would otherwise lay out two ways.
    pub fn nodes_in_order(&self) -> impl Iterator<Item = (&NodeId, &Node)> + '_ {
        let mut nodes: Vec<(&NodeId, &Node)> = self.nodes.iter().collect();
        nodes.sort_unstable_by_key(|(id, _)| **id);
        nodes.into_iter()
    }
}

#[cfg(test)]
mod tests;
