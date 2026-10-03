//! Type-checker state: symbols, scopes, the shared context, and its result.

use super::*;
use crate::types::mono::unwrap_ref;

// ---------- scope + symbol info ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Var,
    Buffer,
    Array,
    Map,
    Param,
    EventParam,
    LetBinding,
    Fn,
    Chip,
    Event,
    ChipInstance,
    In,
    Out,
    Namespace,
    Type,
}

#[derive(Clone, Debug)]
pub struct EventDataField {
    pub name: String,
    pub ty: Type,
    /// Mirrors `ast::Param::is_const` for a mod/chip PARAMETER field (`name:
    /// const T`, or every param of a `const mod`); always `false` for an
    /// output, event-data, or `out` field — const-ness only ever constrains
    /// an argument, never a produced value.
    pub is_const: bool,
}

#[derive(Clone, Debug)]
pub struct FnOrChipSig {
    pub params: Vec<EventDataField>,
    /// The mod has a trailing `...rest` variadic parameter: `params` lists only
    /// the FIXED params, and a call may pass any number of extra trailing args
    /// (captured per call site into a compile-time tuple bound to `rest`). Only
    /// ever set for an inline `mod`.
    pub variadic: bool,
    pub outputs: Vec<EventDataField>,
    /// Generic type params declared on a `mod`/`chip<T, ...>` decl, each
    /// paired with its resolved mask (the concrete types it may bind to).
    /// Empty for non-generic decls and for `fn` (no generics parse there) —
    /// the call-site inference path is gated on this being non-empty so
    /// non-generic calls are completely unaffected.
    pub type_params: Vec<(String, Vec<Type>)>,
}

#[derive(Clone, Debug)]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub name: String,
    pub ty: Type,
    pub decl_range: SourceRange,
    pub signature: Option<FnOrChipSig>,
    pub event_data: Option<Vec<EventDataField>>,
}

/// Thin wrapper around the shared `Scope<V>` stack, preserving the
/// typecheck-specific API (`declare`, `lookup`, `set_type`).
pub struct Scope {
    inner: ScopeStack<SymbolInfo>,
    /// Live map of every `SymbolKind::Type` alias currently visible
    /// (innermost wins), maintained by `declare`/`push`/`pop` so
    /// `type_aliases()` is a refcount bump instead of a full-scope rescan
    /// that deep-clones every alias body per annotation.
    alias_view: std::sync::Arc<HashMap<String, Type>>,
    /// One undo log per frame above the base: each entry is (alias name,
    /// value it shadowed in `alias_view`, or `None` if newly introduced),
    /// applied in reverse on `pop`.
    alias_undo: Vec<Vec<(String, Option<Type>)>>,
    /// Bumped on every change to `alias_view`, so a view derived from it
    /// can tell whether it is still current.
    alias_gen: u64,
    /// One log per `mod`/`chip` body being checked, innermost last: every
    /// storage declaration a lookup resolved while it was open, by name
    /// (WS091, see `shadow`).
    storage_log: std::cell::RefCell<Vec<crate::collections::HashSet<(String, SourceRange)>>>,
    /// Nonzero while lookups are made for something other than the body being
    /// checked (see `pause_storage_log`).
    storage_log_paused: std::cell::RefCell<usize>,
    /// Every declared name that starts with `__`, a prefix the compiler keeps
    /// for its own storage slots (WS093, see `reserved_names`).
    pub(super) reserved: Vec<(String, SourceRange)>,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    pub fn new() -> Self {
        Self {
            inner: ScopeStack::new(),
            alias_view: std::sync::Arc::new(HashMap::default()),
            alias_undo: Vec::new(),
            alias_gen: 0,
            storage_log: std::cell::RefCell::new(Vec::new()),
            storage_log_paused: std::cell::RefCell::new(0),
            reserved: Vec::new(),
        }
    }
    pub fn push(&mut self) {
        self.inner.push(crate::scope::ScopeTag::BLOCK);
        self.alias_undo.push(Vec::new());
    }
    pub fn pop(&mut self) {
        self.inner.pop();
        if let Some(undo) = self.alias_undo.pop() {
            if !undo.is_empty() {
                self.alias_gen += 1;
            }
            let view = std::sync::Arc::make_mut(&mut self.alias_view);
            for (name, prev) in undo.into_iter().rev() {
                match prev {
                    Some(ty) => view.insert(name, ty),
                    None => view.remove(&name),
                };
            }
        }
    }
    /// Number of open frames (>= 1); the top frame's index is `depth() - 1`.
    pub fn depth(&self) -> usize {
        self.inner.depth()
    }
    /// The shared stack under this one, for the reads that resolve a name
    /// without logging it (see [`crate::const_scope`]).
    pub(crate) fn stack(&self) -> &ScopeStack<SymbolInfo> {
        &self.inner
    }
    /// Seal by-name lookups below `floor` (see [`crate::scope::Scope::set_floor`]).
    /// Returns the previous floor for restoration.
    pub fn set_floor(&mut self, floor: usize) -> usize {
        self.inner.set_floor(floor)
    }
    /// Declare in the top-most frame. Returns the prior info if any.
    pub fn declare(&mut self, name: &str, info: SymbolInfo) -> Option<SymbolInfo> {
        if name.starts_with("__") && !info.decl_range.file.is_empty() {
            self.reserved.push((name.to_string(), info.decl_range.clone()));
        }
        if info.kind == SymbolKind::Type {
            self.alias_gen += 1;
            let prev = std::sync::Arc::make_mut(&mut self.alias_view)
                .insert(name.to_string(), info.ty.clone());
            if let Some(undo) = self.alias_undo.last_mut() {
                undo.push((name.to_string(), prev));
            }
        }
        self.inner.insert(name, info)
    }
    /// Bind `name` in the outermost frame to `info`, or unbind it there,
    /// returning what it bound there.
    pub(crate) fn set_root(&mut self, name: &str, info: Option<SymbolInfo>) -> Option<SymbolInfo> {
        self.inner.set_root(name, info)
    }
    /// What `name` is bound to in the outermost frame.
    pub(crate) fn root(&self, name: &str) -> Option<&SymbolInfo> {
        self.inner.get_in(0, crate::intern::intern(name))
    }
    /// Mutate an already-declared symbol's type (used to refine buffer
    /// types after their RHS infers).
    pub fn set_type(&mut self, name: &str, ty: Type) {
        if let Some(info) = self.inner.get_mut(name) {
            if info.kind == SymbolKind::Type {
                self.alias_gen += 1;
                std::sync::Arc::make_mut(&mut self.alias_view)
                    .insert(name.to_string(), ty.clone());
            }
            info.ty = ty;
        }
    }
    pub fn lookup(&self, name: &str) -> Option<&SymbolInfo> {
        let found = self.inner.get(name);
        if let Some(info) = found
            && matches!(info.kind, SymbolKind::Var | SymbolKind::Array | SymbolKind::Map | SymbolKind::Buffer)
            && *self.storage_log_paused.borrow() == 0
            && let Some(log) = self.storage_log.borrow_mut().last_mut()
        {
            log.insert((name.to_string(), info.decl_range.clone()));
        }
        found
    }
    /// What `name` resolves to and the index of the frame binding it, without
    /// logging the lookup.
    pub fn binding_frame(&self, name: &str) -> Option<(usize, &SymbolInfo)> {
        Some((self.inner.visible_frame(name)?, self.inner.get(name)?))
    }
    /// Stop logging lookups until [`resume_storage_log`](Self::resume_storage_log).
    pub fn pause_storage_log(&self) {
        *self.storage_log_paused.borrow_mut() += 1;
    }
    /// Undo one [`pause_storage_log`](Self::pause_storage_log).
    pub fn resume_storage_log(&self) {
        let mut n = self.storage_log_paused.borrow_mut();
        *n = n.saturating_sub(1);
    }
    /// Start logging the storage lookups resolve, for a body being checked.
    pub fn open_storage_log(&self) {
        self.storage_log.borrow_mut().push(Default::default());
    }
    /// Stop the innermost log and hand back what it saw. What it saw is also
    /// what the enclosing body saw, so it joins that one's log.
    pub fn close_storage_log(&self) -> crate::collections::HashSet<(String, SourceRange)> {
        let mut logs = self.storage_log.borrow_mut();
        let done = logs.pop().unwrap_or_default();
        if let Some(outer) = logs.last_mut() {
            outer.extend(done.iter().cloned());
        }
        done
    }
    /// Every symbol `lookup` can reach, innermost frame first: the same frames
    /// and the same seal, enumerated instead of queried by name. A name
    /// declared in two open frames appears once per frame.
    pub fn iter_visible(&self) -> impl Iterator<Item = (&str, &SymbolInfo)> {
        self.inner.iter_visible()
    }
    /// Snapshot of every `SymbolKind::Type` alias currently visible
    /// (innermost frame wins on a name collision, matching `lookup`'s
    /// shadowing), for `resolve_type_expr`'s delegation to the shared
    /// `types::resolve::resolve_type`.
    pub fn type_aliases(&self) -> std::sync::Arc<HashMap<String, Type>> {
        self.alias_view.clone()
    }
    /// Which state of the alias view `type_aliases` returns now: equal
    /// generations, equal views.
    pub(super) fn alias_generation(&self) -> u64 {
        self.alias_gen
    }
    /// Copies of every frame above the base one, with the seal and the type
    /// aliases visible now: what a body checked here sees besides the
    /// module's own declarations.
    pub(super) fn capture_view(&self) -> ScopeView {
        ScopeView {
            frames: self
                .inner
                .frames()
                .skip(1)
                .map(|(tag, id, entries)| (tag, id, entries.map(|(k, v)| (k, v.clone())).collect()))
                .collect(),
            floor: self.inner.floor(),
            aliases: self.alias_view.clone(),
        }
    }
    /// Show the base frame and `view`'s frames in place of every frame above
    /// the base, with no storage log open, until [`leave_view`](Self::leave_view).
    pub(super) fn enter_view(&mut self, view: &ScopeView) -> SavedScope {
        self.alias_gen += 1;
        let floor = self.inner.set_floor(0);
        let detached = self.inner.detach_above(0);
        for (tag, id, entries) in &view.frames {
            self.inner.push_as(*tag, *id, entries.iter().cloned());
        }
        self.inner.set_floor(view.floor);
        SavedScope {
            detached,
            floor,
            aliases: std::mem::replace(&mut self.alias_view, view.aliases.clone()),
            alias_undo: std::mem::replace(&mut self.alias_undo, vec![Vec::new(); view.frames.len()]),
            storage_log: std::mem::take(&mut *self.storage_log.borrow_mut()),
        }
    }
    /// Undo [`enter_view`](Self::enter_view).
    pub(super) fn leave_view(&mut self, saved: SavedScope) {
        self.alias_gen += 1;
        self.inner.set_floor(0);
        self.inner.detach_above(0);
        self.inner.reattach(saved.detached);
        self.inner.set_floor(saved.floor);
        self.alias_view = saved.aliases;
        self.alias_undo = saved.alias_undo;
        *self.storage_log.borrow_mut() = saved.storage_log;
    }
}

/// The frames above the base one, as [`Scope::capture_view`] copied them.
pub(super) struct ScopeView {
    frames: Vec<crate::scope::FrameCopy<SymbolInfo>>,
    floor: usize,
    aliases: std::sync::Arc<HashMap<String, Type>>,
}

/// What [`Scope::enter_view`] set aside.
pub(super) struct SavedScope {
    detached: crate::scope::Detached<SymbolInfo>,
    floor: usize,
    aliases: std::sync::Arc<HashMap<String, Type>>,
    alias_undo: Vec<Vec<(String, Option<Type>)>>,
    storage_log: Vec<crate::collections::HashSet<(String, SourceRange)>>,
}

/// Scope key for a readable output port. Mirrors lowering's
/// `lower::context::output_scope_key`, so the two passes agree on where a port
/// lives and neither can shadow an ordinary binding of the same name. A scope
/// iterator sees the prefix, so a consumer wanting ordinary bindings skips it.
pub(crate) fn out_scope_key(name: &str) -> String {
    format!("{OUT_SCOPE_PREFIX}{name}")
}

/// The prefix `out_scope_key` stamps on a port's scope key.
const OUT_SCOPE_PREFIX: &str = "out:";

// ---------- exec/pure context ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecMode {
    Exec,
    Pure,
}

// ---------- typecheck context ----------

/// What [`TypeCheckCtx::top_view`] replaced: the outermost frame's binding of
/// each name it set, and the constants' place.
#[derive(Default)]
pub(crate) struct TopView {
    bound: Vec<(String, Option<SymbolInfo>)>,
    cursor: Option<crate::scope::top::ReadAt>,
}

/// Pre-indexed info about a namespace member for O(1) lookup.
#[derive(Clone, Debug)]
pub struct NsDeclInfo {
    pub kind: SymbolKind,
    pub return_type: Option<TypeExpr>,
    /// A callable member's declared outputs, by name: a namespaced call's
    /// value is typed from them as a flat call's is.
    pub outputs: Vec<(String, TypeExpr)>,
    /// The member's declared params, carried so a namespaced call
    /// (`ns.f(args)`) can be arity/type-checked via `check_args`. A
    /// generic member's params may still carry `Type::Param` (there's no
    /// call-site `subst` here to resolve them against); `check_args`'s own
    /// `type_has_param` guard skips those, same as it does elsewhere.
    pub params: Vec<EventDataField>,
    /// For a VALUE member (`let`/`var`/`array`/`map`/`buffer`), its resolved
    /// type — what `ns.member` evaluates to when read rather than called.
    /// `None` for callables, and for a value whose type can't be determined at
    /// registration time (an unannotated non-literal initializer), which reads
    /// as `any`. Without a resolved type here, a namespaced value reference
    /// types as `any` everywhere, so passing it anywhere typed becomes a
    /// spurious mismatch.
    pub value_type: Option<Type>,
    /// A `let` member's declaration key in `TypeCheckCtx::let_ref_values`.
    pub let_decl: Option<super::let_binding::LetKey>,
    /// A GENERIC `mod`/`chip` member's full signature (type params with their
    /// masks, outputs typed over them), so a namespaced call infers the type
    /// params from its arguments as a flat call does, with its declaration's
    /// range. `None` otherwise.
    pub generic: Option<(FnOrChipSig, SourceRange)>,
}

pub struct TypeCheckCtx<'a> {
    pub diagnostics: Vec<Diagnostic>,
    pub scope: Scope,
    pub(super) exec_stack: Vec<ExecMode>,
    pub file: String,
    pub namespaces: HashMap<String, HashMap<String, NsDeclInfo>>,
    /// The same members, keyed first by the FILE whose `import * as` introduced
    /// the alias. An alias is file-local (see `namespace_visible`), so two
    /// modules may each write `import * as Other` and mean different modules;
    /// `namespaces` above is keyed by name alone and keeps only one of them.
    /// Resolution consults this with the REFERENCE's file before falling back.
    pub namespaces_by_file: HashMap<Arc<str>, HashMap<String, HashMap<String, NsDeclInfo>>>,
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
    /// Declaration sites, `(file, decl_range.start.offset)`, of `match` /
    /// `if let` / `let else` captures bound off a scrutinee that is NOT
    /// writable storage. Keyed by decl site rather than by name so shadowing
    /// cannot leak between scopes. Read by `infer_assign_target`, to reject a
    /// write in the capture's own terms rather than the plain `let`'s "declare
    /// it `var`", which a capture cannot do.
    pub(super) readonly_captures: crate::collections::HashSet<(Arc<str>, usize)>,
    /// Typed every visited expression; key is (file, start_offset, end_offset).
    pub type_of_expr: HashMap<(Arc<str>, usize, usize), Type>,
    /// Operator rule chosen for every BinOp/UnOp; same key scheme.
    pub op_resolutions: HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Ferried payload type per local exec signal, recorded from
    /// `emit sig = <value>` so `let { a, b } = await sig` can type its fields.
    pub signal_payload_types: HashMap<String, Type>,
    /// Top-level `out` ports that already registered a site carrying an
    /// initializer, keyed by scope key. A port may legitimately have several
    /// sites when they are exec-derived (`out r = 0` plus `emit r = ...`,
    /// or branches only one of which fires), but two TOP-LEVEL initializers
    /// are both continuously live, so the emitted port has two racing drivers.
    pub(super) out_ports_with_initializer: crate::collections::HashSet<String>,
    /// Every value written to each module port no site annotates, keyed by
    /// scope key, in the order the checker reached them. Such a port is stored
    /// as the join of its writes (see [`finish_untyped_ports`]).
    pub(super) untyped_port_writes: HashMap<String, Vec<PortWrite>>,
    /// The type each such port's writes joined to in the previous round, for a
    /// port some write of which has another type.
    pub(super) untyped_port_seeds: Arc<PortSeeds>,
    /// Set when this round's joins differ from the seeds, so the program is
    /// checked again with these.
    pub(super) untyped_port_next: Option<PortSeeds>,
    /// Generic type aliases (`type Pair<T> = { a: T, b: T }`) in scope, keyed
    /// by name (namespaced ones by their qualified `Ns.Name`). Populated by a
    /// pre-pass over `script.decls` — BEFORE the two-pass decl
    /// registration/checking below — so a generic alias resolves regardless
    /// of where it's declared relative to its uses (including
    /// self-reference, guarded separately by the resolver's depth cap).
    /// Threaded into every `resolve_type_expr` call via `ResolveCtx`. Uses the
    /// crate's Fx `HashMap` (like every other compiler table here), matching
    /// `ResolveCtx::generic_aliases` and `Scope::type_aliases()`.
    pub generic_type_aliases: HashMap<String, crate::types::resolve::GenericAlias>,
    /// Every `enum` declaration in the script, keyed by name, discriminants
    /// already assigned. Populated by `enums::collect_enum_defs`'s pre-pass
    /// (same placement as `generic_type_aliases`, run before decl
    /// registration) so a use of the enum name resolves regardless of where
    /// in the file the `enum` itself is declared.
    pub enum_defs: Arc<crate::typecheck::enums::EnumRegistry>,
    /// The compile-time constants: the top-level `let`/`const` values (so a
    /// `var` initializer may name one, `1 << C_FLAG`, rather than restating
    /// its value), and those of each open scope frame (a body-local `let` of a
    /// constant, a `const`, a `const` parameter's placeholder). Populated
    /// before decl checking; must stay in step with lowering's own so both
    /// agree on exactly which names are constant. Read only through
    /// [`const_lookup`](Self::const_lookup) and its siblings, which resolve a
    /// name through the scope.
    pub(crate) consts: crate::const_scope::ConstScope,
    /// What each free name means at the program's top level.
    pub(crate) top_names: Arc<crate::scope::top::TopNames>,
    /// Where each top-level `enum` is declared, by its name.
    pub(crate) enum_decls: HashMap<String, Vec<SourceRange>>,
    /// The type each call of a generic `mod` or `chip` gives each of its type
    /// parameters, by the declaration's key.
    pub(crate) generic_call_args: HashMap<super::shadow::DeclKey, Vec<Vec<(String, Type)>>>,
    /// Each `WS003` that suggests `<T: string>` to a generic body calling a
    /// string method on a value of an unbounded `T`, with the body and `T`:
    /// [`settle_string_bound_hints`](Self::settle_string_bound_hints) drops
    /// the suggestion where a call gives `T` another type.
    pub(crate) string_bound_hints: Vec<(SourceRange, String, super::shadow::DeclKey, String)>,
    /// The symbol of each declaration of a name declared more than once at
    /// the top level of a file, by its file, start offset and the name, once
    /// checked (see [`top_view`](Self::top_view)).
    pub(crate) top_dup_symbols: HashMap<(Arc<str>, usize, String), SymbolInfo>,
    /// Product of the per-mask-member combo counts on the CURRENT nesting path
    /// of generic decls (1 at the top level). A nested generic `mod`/`chip`
    /// re-runs its whole body per outer combo, so without this the per-decl
    /// combo cap multiplies with depth (`13^d`). The Chip body-check divides
    /// its remaining budget by this so total work stays bounded regardless of
    /// nesting depth; a nested decl whose masks exceed the remaining budget
    /// falls back to a single first-member check (like the too-many-params cap).
    pub active_combos: usize,
    /// Depth of enclosing `@nofold` declaration subtrees, mirroring
    /// `lower::LowerCtx::nofold_depth` 1:1 — same module-level seed
    /// (`Script::no_fold`) and the same per-declaration annotation sites.
    /// `@nofold` promises "nothing folded or elided", and `lower_if` honors
    /// that by still building a real `Branch` for a constant condition, so
    /// this stage must stop eliding there too — otherwise typecheck skips a
    /// block that lowering emits. A depth counter rather than a bool for the
    /// same reason lowering uses one: annotated declarations nest.
    pub(super) nofold_depth: u32,
    /// Depth of enclosing `for` loop bodies: >0 makes `break` legal and
    /// `return` illegal, since a loop is its own microchip plane a `return`
    /// cannot leave. What WS078 reads to tell the two cases apart.
    pub(super) loop_depth: u32,
    /// Depth of enclosing `mod` bodies. A mod is inlined per call site, so its
    /// `in`/`out` statements alias the caller's values instead of crossing a
    /// port; a physical `chip` body resets it to 0.
    pub(super) inline_body_depth: u32,
    /// A record literal checked against a stored record type may give a `Map`
    /// field a map literal (`h = { m: { "a": 1 } }`): lowering rebuilds or bakes
    /// that field like the bare `h.m = { .. }`. Set only for a var initializer
    /// and an assignment.
    pub(super) record_lit_map_fields: bool,
    /// The `match` about to be inferred is a statement (its arms run for their
    /// effects), not a value. Set by the statement check and taken by the
    /// `match` it names.
    pub(super) statement_match: bool,
    /// Inferred custom-event receiver slot types (`infer_custom_event_slots`
    /// output), keyed by handler source range. Empty on pass 1 (nothing inferred
    /// yet); populated on pass 2 by `typecheck_with_inference`. Consulted by
    /// `bind_handler_trigger_params` for unannotated custom-event params and by
    /// `check_custom_event_types`/`ce_receiver_of` for pass-2 WS030 checks.
    pub ce_slots: &'a CeSlotMap,
    /// Stack of the enclosing decl's declared outputs, one frame per
    /// currently-checked module/chip/mod body — `out_ctx.last()` is the
    /// CURRENT (innermost) frame. Consulted by `current_output_ty` so
    /// `return`/`emit`/an unannotated statement-level `out` can be checked
    /// against the declared output type they're implicitly targeting (see
    /// the module-level push in `typecheck` and the per-combo push in the
    /// `TopDecl::Chip` body-check loop).
    pub(super) out_ctx: Vec<Vec<EventDataField>>,
    /// `let f = Foo(…)` where `Foo` is a chip/mod with exactly ONE output:
    /// binding name -> that output's name. A single-output call types as the
    /// bare output value (the name is dropped), but `f.result` stays legal for
    /// readability — so this records the one field name a scalar binding may be
    /// projected by, letting `infer` tell that projection apart from a typo
    /// (`f.reslt`) instead of silently typing both as `any`. A `None` value
    /// means "known to be a single-output call result, but its output name
    /// isn't indexed here" — any field is accepted, never a false typo.
    pub single_output_alias: crate::collections::HashMap<String, Option<String>>,
    /// Which `*T` fields of a record `let` hold a ref VALUE rather than name
    /// storage, keyed by the binding's declaration `(file, offset)`. A record
    /// literal built from vars (`{ cpsr, skip }`) bundles places, whose `*T`
    /// fields read their targets; one built from `&x`, a ref-holding var, a
    /// map read or a call's result carries refs on wires. A `let` with no
    /// entry bundles places. See `super::let_binding::ref_values_of`.
    pub(crate) let_ref_values:
        crate::collections::HashMap<super::let_binding::LetKey, super::let_binding::LetRefValues>,
    /// The scalar `*T` names a destructuring `let` bound to a place
    /// (`let { cpsr: cp } = cpu`): each reads as its var and binds a `*T`
    /// parameter to it. Consulted for a namespace member, whose symbol is
    /// not in scope; an in-scope one is declared like a `*T` parameter.
    pub(crate) let_places: crate::collections::HashSet<super::let_binding::LetKey>,
    /// Every `mod`/`chip` declaration seen so far (top-level AND nested,
    /// registered by `register_decl`'s `TopDecl::Chip` arm — the same site
    /// that declares the symbol into `scope`), keyed by bare name. `scope`
    /// only carries a `FnOrChipSig` (params/outputs/generics) for a chip
    /// symbol, not the AST — `resolve_mod` needs the full `ChipDecl` (body
    /// included) to hand a `const mod` CALL to `const_eval::interp::eval_call`.
    /// A FRAME STACK mirroring the constants' frames 1:1 (`push_scope`/`pop_scope`
    /// push and pop both together) — see `lower::LowerCtx::pass1_chips` for
    /// the mirrored lowering-side design this now matches. The base frame
    /// (index 0, always present) holds every top-level decl, populated by the
    /// pass-1 registration loop that runs before any scope is pushed; a
    /// nested `const mod` registers into the innermost open frame, so it
    /// shadows a same-named outer declaration only inside the body that
    /// declares it and is gone the moment that body's scope pops.
    ///
    /// A single flat map (instead of a stack) would let two DIFFERENT decls
    /// sharing a name (an outer mod shadowed by a same-named nested one)
    /// collide: whichever is registered LAST silently wins everywhere,
    /// including at a call site that should resolve the other one —
    /// typecheck and lowering (which resolve through scope) would then
    /// evaluate different `const mod` bodies for the same call, so one
    /// branch of a dependent `if` gets type-checked while lowering emits the
    /// other, unchecked. See `resolve_mod`.
    pub mod_decls: Vec<HashMap<String, Arc<ChipDecl>>>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped (`Stmt::If`'s const-elision arm), paired with a human-readable
    /// reason (`` `N > 1` is true here ``). These blocks are NOT type-checked
    /// — `if constexpr` semantics, so a branch that wouldn't even compile for
    /// this const value never gets checked. Mirrors `lower::LowerCtx`'s own
    /// `dropped_ranges`; the two MUST agree on exactly which ranges they drop
    /// (see `typecheck_and_lowering_drop_exactly_the_same_ranges` in
    /// `typecheck::tests`) — a disagreement means code gets type-checked but
    /// not lowered, or lowered without ever being checked.
    pub dropped_ranges: Vec<(SourceRange, String)>,
    /// Every module-boundary port `register_port_site` has declared, in the
    /// order it declared them (a reconciled second site for a name already
    /// registered adds nothing). Read by
    /// `port_registry_matches_lowerings_boundary_ports` in `typecheck::tests`,
    /// which compares it against the boundary ports lowering actually emitted.
    ///
    /// `cfg(test)` because it cannot be derived: the AST is the INPUT to the
    /// dispatch under test, and a read-probe reports a false negative for a
    /// port registered into a handler or block frame, which is the shape the
    /// test exists to cover. Lowering's half is read off the emitted module's
    /// port labels and needs no field.
    #[cfg(test)]
    pub port_sites: Vec<String>,
    /// For every identifier read or assigned that names a declared symbol:
    /// the identifier's range, the declaration it resolved to, and whether it
    /// is an assignment's target. Read by the
    /// lowering test that pins a `mod`/`chip` body's storage to the
    /// declaration typecheck checked it against (`lower::tests::lexical_scope`).
    #[cfg(test)]
    pub ident_decls: Vec<(SourceRange, SourceRange, bool)>,
    /// Per `mod`/`chip` declaration, each `var`/array/map/buffer its body names
    /// that is declared outside the body, with that declaration (WS091).
    pub(crate) storage_uses: HashMap<super::shadow::DeclKey, HashMap<String, Vec<SourceRange>>>,
    /// Every call of a user `mod`/`chip` by bare name (WS091).
    pub(crate) call_sites: Vec<super::shadow::CallSite>,
    /// The names each `mod`/`chip` body mentions, less its params, computed
    /// once per declaration.
    pub(crate) body_idents: HashMap<super::shadow::DeclKey, Arc<Vec<String>>>,
    /// The `mod`/`chip` bodies being checked, innermost last, and each
    /// declaration's source range (WS091).
    pub(crate) open_bodies: Vec<super::shadow::DeclKey>,
    pub(crate) body_ranges: HashMap<super::shadow::DeclKey, SourceRange>,
    /// The `mod`/`chip` declarations each body calls by bare name, nested
    /// bodies' calls included (WS091).
    pub(crate) body_calls: HashMap<super::shadow::DeclKey, crate::collections::HashSet<super::shadow::DeclKey>>,
    /// Memoized `mod`/`chip`-is-exec-requiring answers, keyed by bare name (see
    /// [`infer::mod_is_exec_requiring`](crate::typecheck::infer::mod_is_exec_requiring)).
    /// A mod whose INLINED direct flow reads or writes a container (or calls an
    /// exec builtin, or transitively calls such a mod) is exec-requiring, so a
    /// PURE call of it is the silent-miscompile the WS007 exec-call check
    /// rejects. Scanning the body is O(body) per distinct callee name, so the
    /// answer is cached here rather than recomputed at every call site.
    pub(super) exec_requiring_memo: crate::collections::HashMap<String, bool>,
    /// The expected type `check(ctx, e, expected)` pushes down for the single
    /// node it is checking, consumed once at the top of `infer_node`. Only a
    /// generic enum construction reads it: `let n: Option<int> = None` takes
    /// its `T` from this annotation when the variant's payload can't determine
    /// it (see `infer_enum_args`). `None` outside a `check`; `infer_node`
    /// `take()`s it so a nested inference never sees a stale hint.
    pub(super) expected_ty: Option<Type>,
    /// Method, index and loop-source resolutions for lowering to replay.
    pub resolutions: Resolutions,
    /// The concatenated type arguments of every enclosing generic mod/chip
    /// body, outermost first; empty outside generics. Part of every
    /// resolution key.
    pub resolution_subst: Arc<[Type]>,
    /// The instantiations of generic mods and chips that calls demand, and
    /// what checking one of them needs (see `typecheck::instances`).
    pub(super) instances: super::instances::Instances,
    /// Which imported declarations each file named (see
    /// [`crate::ast::ImportScope`]).
    pub(super) imports: Arc<crate::ast::ImportScope>,
    /// The unbounded type parameters of the generic bodies being checked,
    /// outermost first. Each is in scope as the mask member this check binds
    /// it to.
    pub(super) unbounded_type_params: Vec<String>,
    /// The parameters of the generic bodies being checked declared with one
    /// of those type parameters as their whole type (`v: T`), by declaration.
    pub(super) unbounded_typed_params: Vec<(SourceRange, String)>,
    /// The bounded type parameters of the generic bodies being checked,
    /// outermost first: each with the members its bound admits and the one
    /// this check binds it to.
    pub(super) bounded_type_params: Vec<(String, Vec<Type>, Type)>,
    /// Where WS098 was already reported, so a body checked more than once
    /// reports it once.
    pub(super) untyped_reported: crate::collections::HashSet<(Arc<str>, usize)>,
    /// The expressions whose type is `any` because of an error already
    /// reported (a read of a var whose declaration is `WS098`, or a value
    /// built from one, or from an operand whose own error left it `any`), by
    /// range. A check on such an operand reports nothing more.
    pub(super) error_typed: crate::collections::HashSet<(Arc<str>, usize, usize)>,
    /// Top-level names declared more than once (`WS013`).
    pub(super) duplicate_decls: crate::collections::HashSet<String>,
    /// The aliases each imported file declares or imports, and the view a
    /// type written there was last resolved against (see
    /// [`super::resolve::aliases_for`]).
    pub(super) file_aliases: std::cell::RefCell<HashMap<Arc<str>, super::resolve::FileAliases>>,
    /// The storage an anonymous `chip { }` body declares at the top level,
    /// which it shares, by its declaration's position: the chip's own
    /// statements read it whatever the top level declared of the name before
    /// the chip.
    pub(super) anon_chip_storage: HashMap<(Arc<str>, usize), SymbolInfo>,
}

impl<'a> TypeCheckCtx<'a> {
    pub fn new(file: &str, ce_slots: &'a CeSlotMap) -> Self {
        Self {
            diagnostics: Vec::new(),
            scope: Scope::new(),
            exec_stack: vec![ExecMode::Pure],
            file: file.to_string(),
            namespaces: HashMap::default(),
            namespaces_by_file: HashMap::default(),
            if_contexts: HashMap::default(),
            var_read_contexts: HashMap::default(),
            readonly_captures: crate::collections::HashSet::default(),
            type_of_expr: HashMap::default(),
            op_resolutions: HashMap::default(),
            signal_payload_types: HashMap::default(),
            out_ports_with_initializer: Default::default(),
            untyped_port_writes: HashMap::default(),
            untyped_port_seeds: Arc::default(),
            untyped_port_next: None,
            generic_type_aliases: HashMap::default(),
            enum_defs: Arc::default(),
            consts: crate::const_scope::ConstScope::default(),
            top_names: Arc::default(),
            generic_call_args: HashMap::default(),
            enum_decls: HashMap::default(),
            string_bound_hints: Vec::new(),
            top_dup_symbols: HashMap::default(),
            active_combos: 1,
            resolutions: Resolutions::default(),
            resolution_subst: Arc::default(),
            instances: Default::default(),
            nofold_depth: 0,
            loop_depth: 0,
            inline_body_depth: 0,
            record_lit_map_fields: false,
            statement_match: false,
            ce_slots,
            out_ctx: Vec::new(),
            single_output_alias: crate::collections::HashMap::default(),
            let_ref_values: crate::collections::HashMap::default(),
            let_places: crate::collections::HashSet::default(),
            // Always non-empty: the base frame holds the module's top-level
            // declarations, registered before any scope is ever pushed (see
            // `mod_decls`'s own doc comment).
            mod_decls: vec![HashMap::default()],
            dropped_ranges: Vec::new(),
            #[cfg(test)]
            port_sites: Vec::new(),
            #[cfg(test)]
            ident_decls: Vec::new(),
            storage_uses: HashMap::default(),
            call_sites: Vec::new(),
            body_idents: HashMap::default(),
            open_bodies: Vec::new(),
            body_ranges: HashMap::default(),
            body_calls: HashMap::default(),
            exec_requiring_memo: crate::collections::HashMap::default(),
            expected_ty: None,
            imports: Default::default(),
            unbounded_type_params: Vec::new(),
            unbounded_typed_params: Vec::new(),
            bounded_type_params: Vec::new(),
            untyped_reported: crate::collections::HashSet::default(),
            error_typed: crate::collections::HashSet::default(),
            duplicate_decls: crate::collections::HashSet::default(),
            file_aliases: Default::default(),
            anon_chip_storage: HashMap::default(),
        }
    }
    /// Read the top level as code (or the body of a declaration) written at
    /// `at` sees it: each name declared more than once at the top level of
    /// `at`'s file is bound, in the outermost frame and in the constants, to
    /// the declaration visible at `at` (see
    /// [`crate::scope::top::TopNames::visible_decl`]). Hands back what
    /// [`restore_top_view`](Self::restore_top_view) puts back.
    pub(crate) fn top_view(&mut self, at: crate::scope::top::ReadAt) -> TopView {
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
            let info = seen.and_then(|d| self.top_dup_symbols.get(&crate::lower::dup_key(d, name))).cloned();
            bound.push((name.to_string(), self.scope.set_root(name, info)));
        }
        TopView { bound, cursor: self.consts.set_cursor(Some(at)) }
    }
    /// Undo a [`top_view`](Self::top_view).
    pub(crate) fn restore_top_view(&mut self, view: TopView) {
        for (name, info) in view.bound.into_iter().rev() {
            self.scope.set_root(&name, info);
        }
        self.consts.set_cursor(view.cursor);
    }
    /// Keep the symbol each top-level declaration in `d` gave a name declared
    /// more than once, for [`top_view`](Self::top_view).
    pub(crate) fn keep_top_dup_symbols(&mut self, d: &TopDecl) {
        if !self.top_names.has_dups() {
            return;
        }
        let r = d.range();
        for (name, _) in crate::scope::top::declared_names_of(d) {
            if self.top_names.is_dup(&r.file, &name)
                && let Some(info) =
                    self.scope.root(&name).filter(|i| crate::scope::top::within(r, &i.decl_range)).cloned()
            {
                self.top_dup_symbols.insert((r.file.clone(), r.start.offset, name), info);
            }
        }
    }
    /// The enum registry with the declarations a payload field's type is read
    /// against.
    pub(crate) fn enums(&self) -> crate::typecheck::patterns::Enums<'_> {
        crate::typecheck::patterns::Enums { defs: &self.enum_defs, types: self.consts.types() }
    }
    /// Drop the `<T: string>` suggestion of each hint in
    /// `string_bound_hints` whose body a call gives another type for `T`:
    /// bounding `T` to `string` would reject that call, so the message names
    /// the call's type instead.
    pub(crate) fn settle_string_bound_hints(&mut self) {
        for (range, message, body, tp) in std::mem::take(&mut self.string_bound_hints) {
            let other = self.generic_call_args.get(&body).and_then(|calls| {
                calls.iter().flatten().find_map(|(p, t)| {
                    (p == &tp && !matches!(t, Type::String | Type::Any | Type::Param(_))).then(|| t.clone())
                })
            });
            let Some(t) = other else { continue };
            let same = |d: &&mut Diagnostic| d.code == "WS003" && d.range == range && d.message == message;
            let Some(d) = self.diagnostics.iter_mut().find(same) else {
                continue;
            };
            let got = crate::analysis::types::type_str(&t);
            let a = if got.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
            let cut = message.find("; bound it").unwrap_or(message.len());
            d.message = format!(
                "{}; a call gives `{tp}` {a} `{got}`, which is not a string, so no bound on `{tp}` lets both compile",
                &message[..cut]
            );
        }
    }
    /// Push a new `scope` frame together with its empty frame of constants.
    /// Use this (paired with `pop_scope`) everywhere a bare
    /// `ctx.scope.push()`/`ctx.scope.pop()` pair would otherwise appear, so
    /// the stacks can never drift out of lockstep.
    pub fn push_scope(&mut self) {
        self.scope.push();
        self.consts.push(self.scope.stack().current_id());
        self.mod_decls.push(HashMap::default());
    }
    /// Pop the frame pushed by `push_scope`.
    pub fn pop_scope(&mut self) {
        self.scope.pop();
        self.consts.pop();
        // Never pop the base frame — it holds the module's top-level
        // declarations and must outlive every scope opened inside the
        // module (mirrors `lower::LowerCtx::pop_scope`'s guard on
        // `pass1_chips`).
        if self.mod_decls.len() > 1 {
            self.mod_decls.pop();
        }
    }
    /// The resolution-table key for `r`: its source range plus the current
    /// generic substitution, so each generic instantiation gets its own entry.
    pub(crate) fn resolution_key(&self, r: &SourceRange) -> ResolutionKey {
        (r.file.clone(), r.start.offset, r.end.offset, self.resolution_subst.clone())
    }

    /// Record `t` as the type of the expression at `r`: range-keyed, and inside
    /// a generic body also for this instantiation.
    /// Whether `e`'s type is `any` because of an error already reported
    /// (see `error_typed`).
    pub(crate) fn is_error_typed(&self, e: &crate::ast::Expr) -> bool {
        let r = e.range();
        !self.error_typed.is_empty() && self.error_typed.contains(&(r.file.clone(), r.start.offset, r.end.offset))
    }

    /// Whether `e` reads a var whose declaration is `WS098`.
    pub(crate) fn reads_untyped_var(&self, e: &crate::ast::Expr) -> bool {
        let crate::ast::Expr::Ident { name, .. } = e else { return false };
        !self.untyped_reported.is_empty()
            && self.scope.lookup(name).is_some_and(|sym| {
                sym.kind == crate::typecheck::SymbolKind::Var
                    && self.untyped_reported.contains(&(sym.decl_range.file.clone(), sym.decl_range.start.offset))
            })
    }

    pub(crate) fn record_expr_type(&mut self, r: &SourceRange, t: &Type) {
        let range = (r.file.clone(), r.start.offset, r.end.offset);
        if !self.resolution_subst.is_empty() {
            let k = self.resolution_key(r);
            self.resolutions.generic_expr_types.insert(k, t.clone());
            self.resolutions.generic_ranges.insert(range.clone());
        }
        self.type_of_expr.insert(range, t.clone());
    }

    /// Record what a `recv.name(args)` call at `call` resolved to.
    pub(crate) fn record_method_target(&mut self, call: &SourceRange, target: MethodTarget) {
        let k = self.resolution_key(call);
        self.resolutions.method_targets.insert(k, target);
    }

    /// Record `expr` as a container of type `ty`; a non-container type records nothing.
    pub(crate) fn record_container_kind(&mut self, expr: &Expr, ty: &Type) {
        if let Some(kind) = ContainerKind::of(ty) {
            let k = self.resolution_key(expr.range());
            self.resolutions.container_kinds.insert(k, kind);
        }
    }

    /// Record every positional argument of a container method whose type is a
    /// container: the source of `append`, `copyFrom`, `slice`, `keys`,
    /// `values` and `sortMultiple`.
    pub(crate) fn record_container_args(&mut self, args: &[CallArg]) {
        for a in args {
            if let CallArg::Positional(e) = a {
                self.record_container_expr(e);
            }
        }
    }

    /// Record `expr` as a container of its already-recorded expression type.
    pub(crate) fn record_container_expr(&mut self, expr: &Expr) {
        let r = expr.range();
        if let Some(t) = self.type_of_expr.get(&(r.file.clone(), r.start.offset, r.end.offset)).cloned() {
            self.record_container_kind(expr, &t);
        }
    }
    /// Run `f` with `nofold_depth` incremented for its duration when `active`
    /// is true (the enclosing declaration was `@nofold`-annotated). Mirrors
    /// `lower::LowerCtx::with_nofold` exactly — including being safe against
    /// an early `return` inside `f`, since a `return` in a closure only
    /// unwinds the closure, so the decrement always runs.
    pub(super) fn with_nofold<R>(&mut self, active: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        if active {
            self.nofold_depth += 1;
        }
        let r = f(self);
        if active {
            self.nofold_depth -= 1;
        }
        r
    }
    /// What a symbol is to a constant read (see [`crate::const_scope::Bound`]):
    /// a `let` or a parameter (a `const` array parameter is declared a
    /// `Var`) may be one its frame records; a mod, chip, event, type or
    /// namespace is no value and hides nothing; anything else is a value that
    /// is never a constant. A `var` statement forgets what its frame recorded
    /// under its name.
    fn const_bound(info: &SymbolInfo) -> crate::const_scope::Bound {
        use crate::const_scope::Bound;
        match info.kind {
            SymbolKind::Fn | SymbolKind::Chip | SymbolKind::Event | SymbolKind::Namespace | SymbolKind::Type => {
                Bound::NotAValue
            }
            SymbolKind::LetBinding | SymbolKind::Param | SymbolKind::Var => Bound::MayBeConstant,
            SymbolKind::Buffer
            | SymbolKind::Array
            | SymbolKind::Map
            | SymbolKind::EventParam
            | SymbolKind::ChipInstance
            | SymbolKind::In
            | SymbolKind::Out => Bound::Value,
        }
    }
    /// Every compile-time constant readable here, by name: a name resolves
    /// through the scope, so a parameter, `var`, loop variable or pattern
    /// binding named like an outer constant hides it (see
    /// [`crate::const_scope`]). Lowering's `LowerCtx::const_lookup` answers
    /// the same way, so both stages agree on which name is which constant.
    pub(crate) fn const_lookup(&self) -> crate::const_scope::ConstNames {
        self.consts.env(self.scope.stack(), &Self::const_bound, crate::const_scope::Admit::All)
    }
    /// The constant `name` reads as here, when `admit` admits it.
    pub(crate) fn constant(&self, name: &str, admit: crate::const_scope::Admit) -> Option<&crate::ir::Literal> {
        self.consts.get(self.scope.stack(), &Self::const_bound, name, admit)
    }
    /// Is `name` a `const`-declared binding holding an array or map?
    ///
    /// The typecheck-side counterpart of `LowerCtx::const_container_literal`.
    /// Such a binding is IMMUTABLE: it is a compile-time value and (once
    /// something reads it at runtime) a container gate, and only the
    /// immutability keeps those two from answering the same question
    /// differently.
    pub(super) fn is_const_container(&self, name: &str) -> bool {
        matches!(
            self.constant(name, crate::const_scope::Admit::Declared),
            Some(crate::ir::Literal::Array(_) | crate::ir::Literal::Map(_))
        )
    }
    /// The `ConstCtx` for `const_eval::eval_expr`, resolving names as
    /// [`const_lookup`](Self::const_lookup) does, so every evaluation site
    /// (typecheck, and lowering's own matching helper) agrees on what's
    /// constant at this point. A `const mod` body is evaluated against the
    /// module's constants plus its own parameters, never the call site's
    /// scope frames (see `const_eval::interp::eval_call`).
    ///
    /// `lookup_mod` is supplied by the CALLER, built from [`resolve_mod`](Self::resolve_mod)
    /// — a `&dyn Fn` borrowing `self` can't be packaged into a value THIS
    /// method returns (the closure would have to outlive the call that
    /// builds it, but `self` only lives as long as the caller's own borrow),
    /// so every const-evaluating call site builds its own short-lived
    /// closure over `ctx` and passes it straight through.
    pub fn const_ctx<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::All, lookup_mod)
    }
    fn admitting_ctx<'b>(
        &self,
        admit: crate::const_scope::Admit,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.consts.const_ctx(
            self.scope.stack(),
            &Self::const_bound,
            admit,
            self.enum_defs.clone(),
            self.imports.clone(),
            lookup_mod,
        )
    }
    /// [`const_ctx`](Self::const_ctx) without any PLACEHOLDER (a `const`
    /// parameter's type-shaped zero, or a value derived from one), so an
    /// expression that reads one fails to evaluate (`NotConstant`) instead of
    /// silently evaluating against a type-shaped zero. This is structural
    /// rather than a syntactic scan of the expression: a placeholder read
    /// anywhere inside (an operand, a constructor argument, an interpolation
    /// slot) is caught, because the name simply is not in the environment.
    pub(super) fn const_ctx_without_placeholders<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::NoPlaceholders, lookup_mod)
    }
    /// [`const_ctx`](Self::const_ctx) without any constant computed through
    /// an integer operation the certified fold table has no result for: what
    /// a `float` slot may be given as a literal. Mirrors
    /// `LowerCtx::certified_const_ctx`.
    pub(super) fn certified_const_ctx<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::Certified, lookup_mod)
    }
    /// [`const_ctx_without_placeholders`](Self::const_ctx_without_placeholders),
    /// additionally restricted to const-DECLARED names: the `Stmt::If`
    /// condition-elision arm's own environment. A name must be BOTH a real
    /// constant (not a placeholder) AND actually spelled `const` (not a plain
    /// `let` that merely happens to fold) to make its reader eligible for the
    /// generalised const-eval elision; everything else falls through to the
    /// narrower pre-feature paths (a literal `true`/`false`, or
    /// `ident_literal_bool`'s ident-bound-to-a-literal-bool-gate fallback on
    /// the lowering side), which are unaffected by this restriction.
    pub(super) fn if_cond_const_ctx<'b>(
        &self,
        lookup_mod: crate::const_scope::ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        self.admitting_ctx(crate::const_scope::Admit::Declared, lookup_mod)
    }
    /// Resolve `name` to its `ChipDecl` for a `const mod` CALL: `name` must
    /// currently resolve in `scope` to a chip/mod symbol (exactly the check
    /// an ordinary call to it would pass — an out-of-scope or shadowed-by-a-
    /// non-chip name is `None`, not a stale hit from `mod_decls`), and only
    /// then is the full declaration looked up by scanning `mod_decls`
    /// INNERMOST-frame-first — the same shadowing `const_lookup` gives scoped
    /// constants, and the same scan order as `lower::LowerCtx::resolve_mod_pass1`.
    /// A nested `const mod` therefore wins only for the duration of its own
    /// scope, matching how lowering already resolves the ordinary (non-const)
    /// call path through `ctx.scope`. The caller (every const-evaluating site
    /// in `decl.rs`/`stmt.rs`/`sig.rs`) wraps this in a closure and hands it
    /// to [`const_ctx`](Self::const_ctx) as `lookup_mod`.
    pub(super) fn resolve_mod(&self, name: &str) -> Option<Arc<ChipDecl>> {
        match self.scope.lookup(name) {
            Some(info) if info.kind == SymbolKind::Chip => {
                self.mod_decls.iter().rev().find_map(|frame| frame.get(name)).cloned()
            }
            _ => None,
        }
    }
    /// Whether a bare call to `name` written in `call_file` calls a user
    /// `mod`/`chip` rather than a builtin: one must be in scope, and the
    /// calling file must be able to name it (see
    /// [`crate::ast::ImportScope::user_mod_answers`]).
    pub(super) fn calls_user_mod(&self, name: &str, call_file: &str) -> bool {
        self.resolve_mod(name).is_some_and(|d| self.imports.user_mod_answers(&d.range, call_file))
    }
    /// The unbounded type parameter the receiver `recv`, of type `t`, stands
    /// for here: `recv` is, or reads a field or element of, a value declared
    /// with a type naming that type parameter (`v: T`, `v: T[]`, `b: B<T>`,
    /// or a `let` or `for` name taken from one), and this check binds the
    /// type parameter to `t`. A receiver rooted in a call's result (`id(v)`)
    /// stands for the one an argument of the call stands for (the value the
    /// call passes through), else for the unbounded type parameter of the body
    /// this check binds to `t`.
    pub(super) fn unbounded_param_of(&self, recv: &Expr, t: &Type) -> Option<&str> {
        let bound_to_t =
            |tp: &str| self.scope.lookup(tp).is_some_and(|s| s.kind == SymbolKind::Type && s.ty == *t);
        if let Some(tp) = self.unbounded_root_of(recv) {
            return bound_to_t(tp).then_some(tp);
        }
        let mut root = recv;
        while let Expr::FieldAccess { obj, .. } | Expr::IndexAccess { obj, .. } | Expr::TuplePick { obj, .. } = root {
            root = obj;
        }
        let Expr::Call { callee, args, .. } = root else {
            return None;
        };
        // A generic callee whose result is one of its type parameters
        // (`mod two<A, B>(a: A, b: B) -> B`) gives the value of the arguments
        // that parameter types, so those name the parameter first.
        let result_param = match callee.as_ref() {
            Expr::Ident { name, .. } => self.resolve_mod(name).and_then(|decl| match decl.outputs.as_slice() {
                [out] => match &out.typ {
                    crate::ast::TypeExpr::Name { name: p, .. } if decl.type_params.iter().any(|tp| &tp.name == p) => {
                        Some((decl.clone(), p.clone()))
                    }
                    _ => None,
                },
                _ => None,
            }),
            _ => None,
        };
        let gives_result = |i: usize| match &result_param {
            Some((decl, p)) => {
                decl.inputs.get(i).is_some_and(|inp| matches!(&inp.typ, crate::ast::TypeExpr::Name { name, .. } if name == p))
            }
            None => true,
        };
        let of_arg = |a: &Expr| self.unbounded_param_of(a, t);
        let from_result = args.iter().enumerate().find_map(|(i, a)| match a {
            crate::ast::CallArg::Positional(v) if result_param.is_some() && gives_result(i) => of_arg(v),
            _ => None,
        });
        let from_arg = from_result.or_else(|| {
            args.iter().find_map(|a| match a {
                crate::ast::CallArg::Positional(v) | crate::ast::CallArg::Named { value: v, .. } => of_arg(v),
                crate::ast::CallArg::Spread(_) => None,
            })
        });
        from_arg.or_else(|| self.unbounded_typed_params.iter().rev().map(|(_, tp)| tp.as_str()).find(|tp| bound_to_t(tp)))
    }
    /// The unbounded type parameter the value at the root of `e` (the name
    /// it reads a field or an element of) is declared with.
    pub(super) fn unbounded_root_of(&self, e: &Expr) -> Option<&str> {
        let mut e = e;
        loop {
            e = match e {
                Expr::FieldAccess { obj, .. } | Expr::IndexAccess { obj, .. } | Expr::TuplePick { obj, .. } => obj,
                _ => break,
            };
        }
        let Expr::Ident { name, .. } = e else { return None };
        let decl = &self.scope.lookup(name)?.decl_range;
        self.unbounded_typed_params.iter().rev().find(|(d, _)| d == decl).map(|(_, tp)| tp.as_str())
    }
    /// The `self` mod a method call `x.name()` written in `call_file` may
    /// call: one in scope whose first parameter is `self`, when the calling
    /// file can name it (see [`crate::ast::ImportScope::user_mod_answers`]).
    pub(super) fn visible_self_mod(&self, name: &str, call_file: &str) -> Option<&SymbolInfo> {
        self.scope.lookup(name).filter(|s| {
            s.kind == SymbolKind::Chip
                && s.signature.as_ref().is_some_and(|sig| sig.params.first().is_some_and(|p| p.name == "self"))
                && self.imports.user_mod_answers(&s.decl_range, call_file)
        })
    }
    /// The error for a call to the user mod `name` declared at `decl`, written
    /// in `call_file`, which can't name it (see
    /// [`crate::ast::ImportScope::user_mod_answers`]). `None` when `name` is
    /// declared twice, which `WS013` already reports.
    pub(super) fn unnamed_mod_call(
        &self,
        name: &str,
        decl: &SourceRange,
        call_file: &str,
        method: bool,
    ) -> Option<String> {
        if self.duplicate_decls.contains(name) {
            return None;
        }
        let base = |f: &str| f.rsplit(['/', '\\']).next().unwrap_or(f).to_string();
        let (here, there) = (base(call_file), base(&decl.file));
        let call = if method { format!("x.{name}(…)") } else { format!("{name}(…)") };
        let fix = if self.imports.imports_file(&decl.file, call_file) {
            format!(
                "`{there}` imports `{here}`, and a module can't import the file that imports it: \
                 move `{name}` into `{here}` or into a module both files import"
            )
        } else {
            format!("Import it into `{here}` by name or with the whole module")
        };
        Some(format!(
            "`{name}` is not declared in `{here}` and `{here}` does not import it, so `{call}` \
             can't call it there (it is declared in `{there}`). {fix}"
        ))
    }
    /// An `import * as ns` alias is visible ONLY within the file whose import
    /// introduced it — the namespace symbol's `decl_range.file`. A namespace a
    /// module privately imports and a pulled-in declaration calls through
    /// TRAVELS into an importer (so that declaration still resolves once
    /// inlined), but it must not leak into the importer's OWN code: the
    /// importer never wrote that `import * as`, so `ns.member` there should be
    /// an unknown identifier, not a silent resolution. `ref_file` is the file
    /// of the reference being resolved. A namespace is only ever legitimately
    /// named in its own declaring file, so this uniform rule leaves every
    /// same-file use (including a traveling namespace used by its origin
    /// file's own decls) untouched. An empty origin file (synthetic) is
    /// treated as visible everywhere. See N11.
    pub(super) fn namespace_visible(&self, ns_name: &str, ref_file: &str) -> bool {
        match self.scope.lookup(ns_name) {
            Some(sym) if sym.kind == SymbolKind::Namespace => {
                let origin = sym.decl_range.file.as_ref();
                // The last clause covers a same-named alias in another file
                // holding the scope entry: the symbol is still a namespace, and
                // THIS file declares its own, so the reference is legitimate.
                // Guarded by the arm above, so a local shadowing the name is
                // still not a namespace reference.
                origin.is_empty() || origin == ref_file || self.ns_own(ns_name, ref_file).is_some()
            }
            _ => false,
        }
    }

    /// The members of the alias `name` as written in `ref_file`, or `None` when
    /// that file declares no such alias.
    pub(super) fn ns_own(
        &self,
        name: &str,
        ref_file: &str,
    ) -> Option<&HashMap<String, NsDeclInfo>> {
        self.namespaces_by_file.get(ref_file)?.get(name)
    }

    /// Namespace members for a reference in `ref_file`: that file's own alias
    /// first, then the flat map for a namespace that traveled in.
    pub(super) fn ns_members(
        &self,
        name: &str,
        ref_file: &str,
    ) -> Option<&HashMap<String, NsDeclInfo>> {
        self.ns_own(name, ref_file).or_else(|| self.namespaces.get(name))
    }

    /// True when `name` IS a namespace symbol but is hidden from `ref_file`
    /// (a traveling alias referenced outside its origin file) — so a call/read
    /// through it must be treated as a dangling base (WS002) rather than a
    /// silent resolution of the leaked namespace. See [`Self::namespace_visible`].
    pub(super) fn namespace_hidden_here(&self, name: &str, ref_file: &str) -> bool {
        matches!(self.scope.lookup(name), Some(s) if s.kind == SymbolKind::Namespace)
            && !self.namespace_visible(name, ref_file)
    }

    /// Note a user-written name, reported as WS093 when it starts with `__`.
    pub(super) fn reserve_check(&mut self, name: &str, range: &SourceRange) {
        if name.starts_with("__") {
            self.scope.reserved.push((name.to_string(), range.clone()));
        }
    }

    pub fn exec_mode(&self) -> ExecMode {
        *self.exec_stack.last().unwrap_or(&ExecMode::Pure)
    }
    pub fn in_exec<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.exec_stack.push(ExecMode::Exec);
        let r = f(self);
        self.exec_stack.pop();
        r
    }
    /// `in_pure` when `force`, otherwise run `f` in the ambient mode.
    pub fn maybe_pure<R>(&mut self, force: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        if force { self.in_pure(f) } else { f(self) }
    }
    pub fn in_pure<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.exec_stack.push(ExecMode::Pure);
        let r = f(self);
        self.exec_stack.pop();
        r
    }
    pub fn emit(&mut self, code: &str, message: impl Into<String>, range: SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
            range,
        });
    }

    pub fn warn(&mut self, code: &str, message: impl Into<String>, range: SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: code.to_string(),
            message: message.into(),
            range,
        });
    }
}

// ---------- result ----------

pub struct TypeCheckResult {
    /// Typed every visited expression; key is (file, start_offset, end_offset).
    pub type_of_expr: HashMap<(Arc<str>, usize, usize), Type>,
    /// Operator rule chosen for every BinOp/UnOp; same key scheme.
    pub op_resolutions: HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Method, index and loop-source resolutions for lowering to replay —
    /// see `TypeCheckCtx::resolutions`.
    pub resolutions: Resolutions,
    /// Exec/pure context for each `if` node; key is (file, start_offset). true = exec.
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    /// Exec/pure context for each var identifier read; key is (file, start_offset). true = exec.
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
    pub diagnostics: Vec<Diagnostic>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped (never type-checked) — see `TypeCheckCtx::dropped_ranges`.
    pub dropped_ranges: Vec<(SourceRange, String)>,
    /// Which imported declarations each file named, as the check used them,
    /// for a lowering that has only this result (a resource-estimate
    /// template) to resolve builtin-named mods the same way.
    pub imports: Arc<crate::ast::ImportScope>,
    /// Every module-boundary port the registry claimed, set out in full on
    /// `TypeCheckCtx::port_sites`.
    #[cfg(test)]
    pub port_sites: Vec<String>,
    /// Every identifier and the declaration it resolved to, set out on
    /// `TypeCheckCtx::ident_decls`.
    #[cfg(test)]
    pub ident_decls: Vec<(SourceRange, SourceRange, bool)>,
    /// The instantiations checked because a call demanded them: the mod or
    /// chip declaration and the body key (see `typecheck::instances`).
    #[cfg(test)]
    pub demanded_instantiations: Vec<ResolutionKey>,
}

/// The declared type of output `name` in the CURRENT (innermost) `out_ctx`
/// frame — the enclosing module/chip/mod's declared outputs, nearest wins.
/// `None` when `name` isn't a declared output of that frame (e.g. a local
/// exec signal, or an unannotated top-level `out`), meaning there's nothing
/// to check against.
///
/// Note: a same-named local `var`/`let` does NOT shadow the output here, and
/// must not — `emit`/`out` lowering resolves the target via `lookup_output`
/// (a disjoint namespace) BEFORE any local, so `out r = v` inside
/// `chip … -> (r: T) { var r: … }` still wires `v` into output `r`. Checking
/// against the output type is therefore correct even when a local shares the
/// name.
pub(super) fn current_output_ty(ctx: &TypeCheckCtx, name: &str) -> Option<Type> {
    ctx.out_ctx.last()?.iter().find(|f| f.name == name).map(|f| f.ty.clone())
}

/// The type output `name` is declared with (an array or map ref read as the
/// container), or `None` when nothing declares one.
///
/// The enclosing mod/chip signature frame answers first, then the
/// module-boundary port registry. A signature output and a port may share a
/// name, so `out r = v` inside `mod f(...) -> (r: T)` must mean the mod's `r`,
/// never a top-level port of that name.
///
/// `Type::Any` means "nothing stated here" and defers to the next authority,
/// except in a real signature frame, where it is an output whose annotation
/// failed to resolve and must still shadow an unrelated port. `out_ctx[0]` is
/// the one frame that is not a signature: it is synthesized from top-level
/// `out` declarations and lists no handler-declared port. The `any` keyword is
/// `Type::Opaque`, a real declared type that answers.
fn declared_output_ty(ctx: &TypeCheckCtx, name: &str) -> Option<Type> {
    let in_signature_frame = ctx.out_ctx.len() > 1;
    match current_output_ty(ctx, name).map(Type::container_ref_as_value) {
        Some(t) if in_signature_frame || !matches!(t, Type::Any) => Some(t),
        _ => ctx
            .scope
            .lookup(&out_scope_key(name))
            .map(|s| s.ty.clone().container_ref_as_value())
            .filter(|t| !matches!(t, Type::Any)),
    }
}

/// Check a value flowing into output `name` against that output's declared
/// type, reporting WS003 on a mismatch.
///
/// Every syntactic form that can drive an output's backing variable answers
/// here: an `out` binding annotated or not, an `emit` payload, a top-level
/// `out`, and `return` via `check_return_value`. A new such form gets the whole
/// rule by calling this, and no caller resolves a port itself.
///
/// An annotated site's own type goes through `check_port_declaration` instead.
///
/// Quiet when nothing declares a type for `name`: it is not a typed output
/// here, and whatever check owns the name speaks instead.
pub(super) fn check_port_write(
    ctx: &mut TypeCheckCtx,
    name: &str,
    value_ty: &Type,
    range: &SourceRange,
) {
    if let Some(port_ty) = declared_output_ty(ctx, name) {
        infer::coerce_or_emit(ctx, &value_ty.clone().container_ref_as_value(), &port_ty, range);
    }
}

/// [`check_port_write`] for the value `value`: a record of the program's own
/// written to a single-value port is WS003 (see
/// [`infer::record_into_one_value`]).
pub(super) fn check_port_write_value(ctx: &mut TypeCheckCtx, name: &str, value: &Expr, value_ty: &Type) {
    check_port_write(ctx, name, value_ty, value.range());
    if let Some(port_ty) = declared_output_ty(ctx, name) {
        infer::literal_in_slot(ctx, value, value_ty, &port_ty);
        infer::record_into_one_value(ctx, value, value_ty, &port_ty);
    }
}

/// The type each unannotated module port's writes join to, keyed by scope key.
pub type PortSeeds = HashMap<String, Type>;

/// One value written to a module port no site annotates.
#[derive(Clone, Debug)]
pub(super) struct PortWrite {
    /// The value's type; a bare multi-output read (`fs.pop()`) as the output
    /// it reads.
    ty: Type,
    range: SourceRange,
    /// A record literal, built in place, so an integer field is the float a
    /// `float` field names.
    literal: bool,
    /// The type arguments of the generic bodies around the write (see
    /// `TypeCheckCtx::resolution_subst`): a generic body is checked for every
    /// type its bounds admit, and only an instantiation a call makes writes.
    subst: Arc<[Type]>,
    /// The `mod`/`chip` body the write is in, if any: a body is checked
    /// whether or not anything calls it, and only a body running code
    /// reaches writes (see [`reachable_bodies`]).
    body: Option<super::shadow::DeclKey>,
}

/// A value written to a module port that no site annotates. The port is
/// stored as the join of every value written to it (`int` and `float` give
/// `float`), which [`finish_untyped_ports`] decides once every write is seen;
/// a program whose writes need it is checked again with that join seeded, so
/// an integer constant written to a port that joined to `float` is that float.
///
/// A `mod` body is inlined where it is called, so its write to a name that is
/// not one of its own outputs is a write to that port too. A `chip` body
/// writes only its own outputs.
pub(super) fn check_untyped_port_write(ctx: &mut TypeCheckCtx, name: &str, value: &Expr, value_ty: &Type) {
    let declared_here = if ctx.out_ctx.len() > 1 {
        ctx.inline_body_depth == 0 || current_output_ty(ctx, name).is_some()
    } else {
        current_output_ty(ctx, name).is_some_and(|t| !matches!(t, Type::Any))
    };
    if declared_here {
        return;
    }
    let key = out_scope_key(name);
    if !ctx.scope.lookup(&key).is_some_and(|s| matches!(s.kind, SymbolKind::Out) && matches!(s.ty, Type::Any)) {
        return;
    }
    let mut ty = unwrap_ref(value_ty).container_ref_as_value();
    if matches!(ty, Type::Any | Type::Opaque | Type::Never) {
        return;
    }
    if let Type::Outputs(fields) = &ty
        && let Some((_, first)) = fields.first()
    {
        ty = first.clone();
    }
    ty = ty.as_record();
    let range = value.range();
    if let Some(seed) = ctx.untyped_port_seeds.get(&key).cloned() {
        infer::int_constants_in_slot(ctx, value, &seed);
        ctx.resolutions.untyped_port_types.insert((range.file.clone(), range.start.offset, range.end.offset), seed);
    }
    let literal = matches!(value, Expr::RecordLit { .. });
    let subst = ctx.resolution_subst.clone();
    let body = ctx.open_bodies.last().cloned();
    let writes = ctx.untyped_port_writes.entry(key).or_default();
    if !writes.iter().any(|w| w.range == *range && w.subst == subst) {
        writes.push(PortWrite { ty, range: range.clone(), literal, subst, body });
    }
}

/// The type that holds both `a` and `b` written to one port: `int` and
/// `float` give `float`, a character or controller and an entity give
/// `entity`, and a record literal whose field is an integer joins a record
/// whose same field is a `float`. A `bool`, `string` or enum beside a number,
/// a record beside a scalar, and two different records have none.
fn port_join(a: &PortWrite, b: &PortWrite) -> Option<Type> {
    if matches!(a.ty, Type::Bool) != matches!(b.ty, Type::Bool) {
        return None;
    }
    if let (Type::Record(fa), Type::Record(fb)) = (&a.ty, &b.ty) {
        if fa.len() != fb.len() {
            return None;
        }
        let mut joined = Vec::with_capacity(fa.len());
        for (name, ta) in fa {
            let tb = &fb.iter().find(|(n, _)| n == name)?.1;
            let t = match (ta, tb) {
                _ if crate::types::coerce::type_eq(ta, tb) => ta.clone(),
                (Type::Int, Type::Float) if a.literal => Type::Float,
                (Type::Float, Type::Int) if b.literal => Type::Float,
                _ => return None,
            };
            joined.push((name.clone(), t));
        }
        return Some(Type::Record(joined));
    }
    crate::types::coerce::widening_join(&a.ty, &b.ty)
}

/// Decide every unannotated module port's type once the round has seen all
/// its writes: the join of their types. A write whose type has no join with an
/// earlier one is `WS103`, naming both. When a port's join differs from some
/// write's own type, it is seeded for another round (unless this is the last),
/// so that write is checked against the join.
pub(super) fn finish_untyped_ports(ctx: &mut TypeCheckCtx, decls: &[TopDecl], last: bool) {
    // The instantiations calls make: a body's type arguments are those of a
    // call, after those of the bodies around the call.
    let mut made: crate::collections::HashSet<Arc<[Type]>> = crate::collections::HashSet::default();
    for (key, args) in &ctx.resolutions.call_type_args {
        made.insert(args.as_slice().into());
        made.insert(key.3.iter().chain(args).cloned().collect());
    }
    let reached = if ctx.untyped_port_writes.values().flatten().any(|w| w.body.is_some()) {
        reachable_bodies(decls)
    } else {
        crate::collections::HashSet::default()
    };
    let live = |w: &PortWrite| {
        (w.subst.is_empty() || made.contains(&w.subst)) && w.body.as_ref().is_none_or(|b| reached.contains(b))
    };
    let mut ports: Vec<(String, Vec<PortWrite>)> = ctx
        .untyped_port_writes
        .drain()
        .map(|(k, ws)| (k, ws.into_iter().filter(live).collect::<Vec<_>>()))
        .filter(|(_, ws)| !ws.is_empty())
        .collect();
    ports.sort_by(|a, b| {
        let (ra, rb) = (&a.1[0].range, &b.1[0].range);
        (&ra.file, ra.start.offset, &a.0).cmp(&(&rb.file, rb.start.offset, &b.0))
    });
    let mut next = PortSeeds::default();
    for (key, writes) in ports {
        let mut acc = writes[0].clone();
        let mut conflict = None;
        for (i, w) in writes.iter().enumerate().skip(1) {
            match port_join(&acc, w) {
                Some(t) => acc = PortWrite { ty: t, literal: acc.literal && w.literal, ..acc },
                None => {
                    let first = (0..i).find(|&j| port_join(&writes[j], w).is_none()).unwrap_or(0);
                    conflict = Some((first, i));
                    break;
                }
            }
        }
        match conflict {
            Some((i, j)) => report_port_conflict(ctx, &key, &writes[i], &writes[j]),
            None if writes.iter().any(|w| !crate::types::coerce::type_eq(&w.ty, &acc.ty)) => {
                next.insert(key, acc.ty);
            }
            None => {}
        }
    }
    if next != *ctx.untyped_port_seeds && !last {
        ctx.untyped_port_next = Some(next);
    }
}

/// Each `mod`/`chip` declaration running code can reach: one called from
/// outside every declaration, or from the body of one reached. A call is
/// matched to declarations by name, so a name two declarations share reaches
/// both.
fn reachable_bodies(decls: &[TopDecl]) -> crate::collections::HashSet<super::shadow::DeclKey> {
    use crate::ast::walk::{walk, Ctx, Node};
    fn declared<'a>(n: Node<'a>) -> Option<&'a ChipDecl> {
        match n {
            Node::Decl(TopDecl::Chip(c)) | Node::Stmt(Stmt::ChipDecl(c)) => Some(c),
            _ => None,
        }
    }
    fn called<'a>(n: Node<'a>, cx: Ctx, out: &mut Vec<&'a str>) {
        if let Node::Expr(Expr::Call { callee, .. }) = n
            && !cx.decl
        {
            match callee.as_ref() {
                Expr::Ident { name, .. } | Expr::FieldAccess { field: name, .. } => out.push(name),
                _ => {}
            }
        }
    }
    let mut by_name: HashMap<&str, Vec<&ChipDecl>> = HashMap::default();
    let mut pending: Vec<&str> = Vec::new();
    for d in decls {
        let node = Node::Decl(d);
        if let Some(c) = declared(node) {
            by_name.entry(c.name.as_str()).or_default().push(c);
        }
        walk(node, Ctx { decl: declared(node).is_some(), ..Ctx::default() }, &mut |n, cx| {
            if let Some(c) = declared(n) {
                by_name.entry(c.name.as_str()).or_default().push(c);
            }
            called(n, cx, &mut pending);
            true
        });
    }
    let mut reached: crate::collections::HashSet<super::shadow::DeclKey> = crate::collections::HashSet::default();
    while let Some(name) = pending.pop() {
        for c in by_name.get(name).into_iter().flatten() {
            if reached.insert((c.range.file.clone(), c.range.start.offset)) {
                walk(Node::Block(&c.body), Ctx::default(), &mut |n, cx| {
                    called(n, cx, &mut pending);
                    true
                });
            }
        }
    }
    reached
}

fn report_port_conflict(ctx: &mut TypeCheckCtx, key: &str, first: &PortWrite, here: &PortWrite) {
    let name = key.strip_prefix(OUT_SCOPE_PREFIX).unwrap_or(key);
    let at = &first.range;
    let line = if at.file == here.range.file {
        format!("line {}:{}", at.start.line, at.start.col)
    } else {
        let file = at.file.rsplit(['/', '\\']).next().unwrap_or(&at.file);
        format!("{file} line {}:{}", at.start.line, at.start.col)
    };
    let (a, b) = (crate::analysis::types::type_str(&first.ty), crate::analysis::types::type_str(&here.ty));
    ctx.emit(
        "WS103",
        format!(
            "output `{name}` has no type, so it is stored as one type that holds every value written \
             to it, and no type holds both `{a}` ({line}) and `{b}` (this write). Annotate the output \
             (`out {name}: T = ...`) or write values of one kind everywhere"
        ),
        here.range.clone(),
    );
}

/// An integer literal written to a `float` output is the float it names (see
/// `infer::int_constants_in_slot`).
pub(super) fn port_write_literals(ctx: &mut TypeCheckCtx, name: &str, value: &Expr) {
    if let Some(port_ty) = declared_output_ty(ctx, name) {
        infer::int_constants_in_slot(ctx, value, &port_ty);
    }
}

/// The value check for an `out` site that carries its own annotation.
///
/// Value-against-annotation and annotation-against-port are both checked
/// elsewhere, and neither implies the value fits the PORT: coercion is not
/// transitive. `string -> any -> int` passes both legs while `string -> int`
/// is a mismatch, as does `string -> bool -> int`.
///
/// Skipped when the port's type IS the annotation, which would report one
/// mismatch twice at one range. Compared by equality, not coercibility, since
/// `any` coerces to anything and is exactly the annotation whose value still
/// needs checking.
pub(super) fn check_annotated_port_write(
    ctx: &mut TypeCheckCtx,
    name: &str,
    annotated_ty: &Type,
    value_ty: &Type,
    range: &SourceRange,
) {
    let Some(port_ty) = declared_output_ty(ctx, name) else {
        return;
    };
    if port_ty == annotated_ty.clone().container_ref_as_value() {
        return;
    }
    infer::coerce_or_emit(ctx, &value_ty.clone().container_ref_as_value(), &port_ty, range);
}

/// Check the type an `out` site DECLARES against the port it writes, reporting
/// at the annotation's own range.
///
/// Defers when a WS003 already sits at that range, which `register_port_site`
/// puts there for a registered site with a message naming both. Only this
/// entry point suppresses: at a VALUE's range a WS003 from inferring the value
/// is a different problem, and the port write still has to be reported too.
pub(super) fn check_port_declaration(
    ctx: &mut TypeCheckCtx,
    name: &str,
    declared_ty: &Type,
    range: &SourceRange,
) {
    if ctx
        .diagnostics
        .iter()
        .any(|d| d.code == "WS003" && d.range == *range)
    {
        return;
    }
    check_port_write(ctx, name, declared_ty, range);
}

/// `check_port_write` for a value that has not been inferred yet: the output's
/// declared type is pushed INTO the expression, so a `null` or a record
/// literal resolves to the port's type rather than being typed blind and
/// compared after the fact. Returns the value's inferred type.
fn check_port_write_expr(ctx: &mut TypeCheckCtx, name: &str, value: &Expr) -> Type {
    match declared_output_ty(ctx, name) {
        Some(port_ty) => infer::check(ctx, value, &port_ty),
        None => infer::infer(ctx, value),
    }
}

/// The output a `return <value>` delivers to, or `None` when the enclosing
/// boundary does not have exactly one.
///
/// Mirrors lowering's `output_count() == 1` / `first_output()` pair
/// (`lower/stmt.rs`), which decides whether a returned value is wired at all.
/// Inside a mod or chip the outputs are the signature's own; at the module
/// boundary they are the registered ports, which the synthesized `out_ctx[0]`
/// frame cannot see.
///
/// The port scan covers every frame `lookup` reaches, so it agrees with
/// `declared_output_ty` about which ports exist. Distinct NAMES count: one
/// port declared in two open frames is still the one port a `return` wires to.
fn sole_output_name(ctx: &TypeCheckCtx) -> Option<String> {
    if ctx.out_ctx.len() > 1 {
        return match ctx.out_ctx.last()?.as_slice() {
            [only] => Some(only.name.clone()),
            _ => None,
        };
    }
    let mut sole: Option<&str> = None;
    for (key, info) in ctx.scope.iter_visible() {
        if !key.starts_with(OUT_SCOPE_PREFIX) {
            continue;
        }
        match sole {
            Some(seen) if seen == info.name => {}
            Some(_) => return None,
            None => sole = Some(&info.name),
        }
    }
    sole.map(str::to_string)
}

/// Check the value of a `return` against the output it is wired into.
///
/// One output in scope is a port write like any other. None means nothing to
/// check against. With several, the value is left to `infer` alone: the
/// working form is `return { a: .., b: .. }`, a NAME-keyed record forwarded
/// per field, and checking it against a positional `Type::Tuple` would report
/// a false WS003, since `coerce`'s `as_tuple_elems` treats only an INDEX-keyed
/// record as tuple-shaped.
///
/// With several, the value is inferred first and then checked LEAF BY LEAF
/// against the ports it is wired into (see [`check_multi_output_return`]).
pub(super) fn check_return_value(ctx: &mut TypeCheckCtx, value: &Expr) {
    match sole_output_name(ctx) {
        Some(name) => {
            check_port_write_expr(ctx, &name, value);
        }
        None => {
            let t = infer::infer(ctx, value);
            check_multi_output_return(ctx, &unwrap_ref(&t), value);
        }
    }
}

/// Check each leaf of a multi-output `return` against the output it is wired
/// into.
///
/// `return { a: .., b: .. }` forwards per NAME and `return (x, y)` per
/// POSITION, and lowering wires both that way, so every leaf is an ordinary
/// port write. The whole value cannot be checked against one `Type::Tuple`
/// instead: `coerce`'s `as_tuple_elems` treats only an INDEX-keyed record as
/// tuple-shaped, so the name-keyed form (the working spelling) would report a
/// false WS003.
///
/// A leaf whose name matches no output is left alone: an extra or misspelled
/// field is a shape question, not a type one, and the port-binding pass owns
/// it.
/// `s` for a count other than one.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Names as a message lists them: `a`, `b`.
fn quoted(names: &[&str]) -> String {
    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
}

fn check_multi_output_return(ctx: &mut TypeCheckCtx, value_ty: &Type, value: &Expr) {
    // Only a signature frame (a `mod`/`chip`'s own outputs) has a declaration
    // ORDER to pair a positional return against; the synthesized module-level
    // frame is a scope scan with none.
    if ctx.out_ctx.len() <= 1 {
        return;
    }
    let outs: Vec<String> = match ctx.out_ctx.last() {
        Some(o) if o.len() > 1 => o.iter().map(|f| f.name.clone()).collect(),
        _ => return,
    };
    let (Type::Record(fields) | Type::Outputs(fields)) = value_ty else {
        // One value for several outputs: lowering has no output to wire it to.
        if !matches!(value_ty, Type::Tuple(_) | Type::Any | Type::Never | Type::Opaque | Type::Param(_)) {
            ctx.emit(
                "WS003",
                format!(
                    "`return` here must give every output a value ({}), as `return (a, b)` \
                     or `return {{ name: v }}`; set an exec output with `emit`, and a single \
                     output with `emit name = v`",
                    outs.join(", ")
                ),
                value.range().clone(),
            );
        }
        return;
    };
    let positional =
        !fields.is_empty() && fields.iter().enumerate().all(|(i, (k, _))| *k == i.to_string());
    // The value must line up with the outputs: a tuple with one element per
    // output (exec outputs may be left out), or a record naming only outputs
    // and every value output. Anything else (a `Point` returned whole, a
    // record missing an output) has fields lowering can't route anywhere.
    let value_outs: Vec<String> = ctx
        .out_ctx
        .last()
        .map(|o| o.iter().filter(|f| !matches!(f.ty, Type::Exec)).map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    // A call result, or a gate read held whole, keeps only its value outputs
    // (an exec output carries no value), and routes by position when its
    // names differ, as lowering does.
    let is_call = matches!(value, Expr::Call { .. }) || matches!(value_ty, Type::Outputs(_));
    let vfields: Vec<&(String, Type)> = fields
        .iter()
        .filter(|(_, t)| !(is_call && matches!(t, Type::Exec)))
        .collect();
    let by_name = !positional
        && vfields.iter().all(|(k, _)| outs.contains(k))
        && value_outs.iter().all(|n| vfields.iter().any(|(k, _)| k == n));
    // A read whose outputs share a name with the declared ones routes by
    // name, so every declared output must name one of them; one sharing none
    // routes by position.
    let shares_a_name = vfields.iter().any(|(k, _)| value_outs.contains(k));
    if is_call && !positional && shares_a_name && !by_name {
        let theirs: Vec<&str> = vfields.iter().map(|(k, _)| k.as_str()).collect();
        let unmatched: Vec<&str> =
            value_outs.iter().filter(|n| !theirs.contains(&n.as_str())).map(String::as_str).collect();
        let extra: Vec<&str> = theirs.iter().copied().filter(|k| !outs.iter().any(|n| n == k)).collect();
        let problem = match (unmatched.as_slice(), extra.as_slice()) {
            ([], _) => format!("its output{} {} named by none of them", plural(extra.len()), quoted(&extra)),
            (_, _) => {
                let verb = if unmatched.len() == 1 { "names" } else { "name" };
                format!("output{} {} {verb} none of its outputs", plural(unmatched.len()), quoted(&unmatched))
            }
        };
        ctx.emit(
            "WS003",
            format!(
                "this `return` gives the outputs ({}) a read whose outputs are ({}): an output named like one \
                 of them takes that one, so every output must be named like one, and {problem}. Name every \
                 output like one of the read's, or name none of them like one to take them in order",
                outs.join(", "),
                theirs.join(", ")
            ),
            value.range().clone(),
        );
        return;
    }
    let call_by_position = is_call && !by_name && !shares_a_name && vfields.len() == value_outs.len();
    let fits = if positional {
        fields.len() == outs.len() || fields.len() == value_outs.len()
    } else {
        by_name || call_by_position
    };
    if !fits {
        ctx.emit(
            "WS003",
            format!(
                "`return` here must give every output a value ({}), one tuple element per \
                 output or a record keyed by output name; got {}",
                outs.join(", "),
                crate::analysis::types::type_str(value_ty)
            ),
            value.range().clone(),
        );
        return;
    }
    // Field ranges come from the literal when the value is one, so a mismatch
    // points at the offending element rather than the whole `return`.
    let lit_ranges: Vec<&SourceRange> = match value {
        Expr::RecordLit { fields: f, .. } => f
            .iter()
            .map(|x| match x {
                RecordLitField::Named { range, .. }
                | RecordLitField::Shorthand { range, .. }
                | RecordLitField::Spread { range, .. } => range,
            })
            .collect(),
        _ => Vec::new(),
    };
    let lit_values: Vec<Option<&Expr>> = match value {
        Expr::RecordLit { fields: f, .. } => f
            .iter()
            .map(|x| match x {
                RecordLitField::Named { value, .. } => Some(value),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    for (i, (key, ft)) in vfields.into_iter().enumerate() {
        let name = if positional {
            outs.get(i)
        } else if call_by_position {
            value_outs.get(i)
        } else {
            outs.iter().find(|n| *n == key)
        };
        let Some(name) = name.cloned() else {
            continue;
        };
        let range = lit_ranges.get(i).copied().unwrap_or_else(|| value.range());
        check_port_write(ctx, &name, &unwrap_ref(ft), range);
        if let Some(Some(v)) = lit_values.get(i) {
            port_write_literals(ctx, &name, v);
        }
    }
}

/// WS093, once per name and place: a user name that starts with `__`. The
/// compiler names an enum's tag and payload slots and a mod's return storage
/// that way, so a user name could stand in for one.
pub(super) fn reserved_names(ctx: &mut TypeCheckCtx) {
    let mut seen = crate::collections::HashSet::default();
    for (name, range) in std::mem::take(&mut ctx.scope.reserved) {
        if seen.insert((name.clone(), range.file.clone(), range.start.offset)) {
            ctx.emit(
                "WS093",
                format!(
                    "`{name}` starts with `__`, which is reserved for names the compiler \
                     makes; rename it"
                ),
                range,
            );
        }
    }
}
