use super::*;

// Pre-declaration pass

/// Record a `chip`/`mod` declaration into the pass-1 lookup that
/// [`array_elem_literal`]/[`map_entry_literal`] resolve a `const mod` call
/// through (see [`LowerCtx::pass1_chips`] for why it is not `ctx.scope`).
///
/// Records into the innermost open frame, so a body-local `const mod` shadows
/// a same-named outer one inside that body and is dropped with the frame.
///
/// Must be called from the same source-ordered loop that pre-declares the
/// list's vars, never a separate pre-pass: a var declared before a `const mod`
/// must not resolve it, matching pass 2's sequential registration (WS021).
pub(super) fn pre_declare_chip_name(ctx: &mut LowerCtx<'_>, c: &ChipDecl) {
    // The stack is never empty (the base frame holds the module's top-level
    // declarations), so this always lands somewhere.
    if let Some(frame) = ctx.pass1_chips.last_mut() {
        std::sync::Arc::make_mut(frame).insert(c.name.clone(), std::sync::Arc::new(c.clone()));
    }
}

/// Whether a body declaring its storage before its statements run declares
/// `name`, the storage `stmts[at]` declares, there. A name the body's frame
/// already binds (a parameter, or a member of the body's `import * as`
/// module) is not: the statement declares it where it is written, so the
/// code before it still reads that binding, as it type-checked. Nor is a
/// name that means something outside the body (an outer frame's binding, a
/// constant, an enum or variant, a namespace) when a statement before it, or
/// its own initializer, reads the name: those read what it meant there.
pub(super) fn hoists_storage(
    ctx: &LowerCtx<'_>,
    stmts: &Mentions<'_>,
    at: usize,
    name: &str,
) -> bool {
    if ctx.scope.get_current(name).is_some() {
        return false;
    }
    let means_something = ctx.scope.get(name).is_some() || ctx.consts.resolved_by_name(name);
    !means_something || !stmts.by(at, name)
}

/// Which statements of a body mention each name, worked out on the first
/// question and kept for the rest of the body's declarations.
pub(super) struct Mentions<'a> {
    stmts: &'a [Stmt],
    first: std::cell::OnceCell<HashMap<&'a str, usize>>,
}

impl<'a> Mentions<'a> {
    pub(super) fn new(stmts: &'a [Stmt]) -> Self {
        Self {
            stmts,
            first: std::cell::OnceCell::new(),
        }
    }

    /// Whether a statement up to and including `at` mentions `name` (see
    /// [`crate::ast::walk::mentions`]).
    pub(super) fn by(&self, at: usize, name: &str) -> bool {
        let first = self.first.get_or_init(|| {
            let mut first = HashMap::default();
            for (i, s) in self.stmts.iter().enumerate() {
                crate::ast::walk::mentioned_names(crate::ast::walk::Node::Stmt(s), &mut |n| {
                    first.entry(n).or_insert(i);
                });
            }
            first
        });
        first.get(name).is_some_and(|i| *i <= at)
    }
}

/// [`hoists_storage`], for the body pre-declare loops: a declaration it
/// leaves to its statement is recorded, so that statement declares fresh
/// storage even though the frame already binds the name (see
/// `LowerCtx::unhoisted`).
pub(super) fn hoist_or_defer(
    ctx: &mut LowerCtx<'_>,
    stmts: &Mentions<'_>,
    at: usize,
    name: &str,
    range: &SourceRange,
) -> bool {
    let hoists = hoists_storage(ctx, stmts, at, name);
    if !hoists {
        ctx.unhoisted.insert(range.start_key());
    }
    hoists
}

/// For each `chip`/`mod` declared anywhere in a statement of `body`, the
/// storage names `body` declares in its later statements (see
/// `LowerCtx::decl_later_storage`). Called for a body whose storage is
/// declared before its statements run, with the body's frame current and
/// before that storage is declared.
pub(super) fn record_later_storage(ctx: &mut LowerCtx<'_>, body: &Block) {
    // The declarations anywhere inside a statement, not inside another
    // declaration's body.
    fn decls_in<'a>(s: &'a Stmt, out: &mut Vec<&'a ChipDecl>) {
        use crate::ast::walk::{Ctx, Node, walk};
        if let Stmt::ChipDecl(c) = s {
            out.push(c);
            return;
        }
        walk(Node::Stmt(s), Ctx::default(), &mut |n, _| match n {
            Node::Stmt(Stmt::ChipDecl(c)) => {
                out.push(c);
                false
            }
            _ => true,
        });
    }
    let frame = ctx.scope.current_id();
    let mentions = Mentions::new(&body.stmts);
    for (i, s) in body.stmts.iter().enumerate() {
        let mut decls = Vec::new();
        decls_in(s, &mut decls);
        if decls.is_empty() {
            continue;
        }
        let stmts = &body.stmts;
        let names: Vec<Sym> = (i + 1..stmts.len())
            .filter_map(|j| match &stmts[j] {
                Stmt::Var(v) => Some(&v.name).filter(|n| hoists_storage(ctx, &mentions, j, n)),
                Stmt::Array(a) => Some(&a.name).filter(|n| hoists_storage(ctx, &mentions, j, n)),
                Stmt::Map(m) => Some(&m.name).filter(|n| hoists_storage(ctx, &mentions, j, n)),
                Stmt::Buffer(b) => Some(&b.name).filter(|n| hoists_storage(ctx, &mentions, j, n)),
                // A chip body's ports are always declared up front.
                Stmt::In(p) => Some(&p.name),
                _ => None,
            })
            .map(|n| intern(n))
            .collect();
        for c in decls {
            let at = c.range.start_key();
            let entry = ctx.decl_later_storage.entry(at).or_default();
            entry.retain(|(f, _)| *f != frame && ctx.scope.has_frame(*f));
            if !names.is_empty() {
                entry.push((frame, names.clone()));
            }
        }
    }
}

/// Pre-declare a top-level storage/input decl, deduped by source location.
/// When the same source declaration was already lowered through another
/// import of its file (a plain + `import * as` pair, `import {g}` +
/// `import {g as h}`, or a diamond), bind `name` to the existing gate instead
/// of creating a duplicate. A different file's same-named decl has a distinct
/// location and gets its own gate. See `LowerCtx::import_state_dedup`.
fn dedup_import_state(
    ctx: &mut LowerCtx<'_>,
    range: &SourceRange,
    name: &str,
    predeclare: impl FnOnce(&mut LowerCtx<'_>),
) {
    if let Some(binding) = ctx.reuse_import_state(range) {
        ctx.scope.insert(name, binding);
        return;
    }
    predeclare(ctx);
    if let Some(binding) = ctx.scope.get(name).cloned() {
        ctx.record_import_state(range, binding);
    }
}

pub(super) fn pre_declare_decl(ctx: &mut LowerCtx<'_>, d: &TopDecl) {
    match d {
        // Record the chip/mod name outside `ctx.scope` (see
        // `LowerCtx::pass1_chips`), so a `const mod` call in a later top-level
        // initializer in this pass-1 loop resolves via
        // `LowerCtx::resolve_mod_pass1`.
        TopDecl::Chip(c) => pre_declare_chip_name(ctx, c),
        // Var/buffer gates are created here in pass 1, outside lower_decl's
        // with_nofold wrap, so the decl's @nofold is honored here. Storage and
        // input decls share one gate per source declaration across imports.
        TopDecl::Var(v) => dedup_import_state(ctx, &v.range, &v.name, |ctx| {
            ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
        }),
        TopDecl::Array(a) => {
            dedup_import_state(ctx, &a.range, &a.name, |ctx| pre_declare_array(ctx, a));
        }
        TopDecl::Map(m) => {
            dedup_import_state(ctx, &m.range, &m.name, |ctx| pre_declare_map(ctx, m));
        }
        TopDecl::Buffer(b) => {
            dedup_import_state(ctx, &b.range, &b.name, |ctx| pre_declare_buffer(ctx, b));
        }
        TopDecl::In(i) => {
            dedup_import_state(ctx, &i.range, &i.name, |ctx| pre_declare_input(ctx, i));
        }
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
                true,
                &o.range,
            );
        }),
        TopDecl::Let(l) => pre_declare_exec_signal(ctx, l),
        TopDecl::AnonChip(ac) => pre_declare_anon_chip(ctx, ac, true),
        _ => {}
    }
}

/// Create the anonymous chip `ac`'s node and declare its storage. `hoisted`
/// for a chip at a module's top level, whose storage every statement there
/// names; one in a body is named from the chip on, so what its storage's
/// names bound before it is bound again until `lower_anon_chip`.
pub(super) fn pre_declare_anon_chip(ctx: &mut LowerCtx<'_>, ac: &AnonChipDecl, hoisted: bool) {
    let storage_name = |s: &Stmt| match s {
        Stmt::Var(VarDecl { name, .. })
        | Stmt::Buffer(BufferDecl { name, .. })
        | Stmt::Array(ArrayDecl { name, .. })
        | Stmt::Map(MapDecl { name, .. }) => Some(name.clone()),
        _ => None,
    };
    let before: Vec<(String, Option<Binding>)> = if hoisted {
        Vec::new()
    } else {
        ac.body
            .stmts
            .iter()
            .filter_map(storage_name)
            .map(|n| {
                let b = ctx.scope.get_current(&n).cloned();
                (n, b)
            })
            .collect()
    };
    let chip_node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::MICROCHIP_ALT,
        source_range: ac.range.clone(),
        ports: GateIO::default(),
        ..Default::default()
    });
    let consts = ctx.const_lookup();
    if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
        node.kind = NodeKind::Chip;
        let props = std::sync::Arc::make_mut(&mut node.properties);
        if ac.closed {
            props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        }
        if let Some(label) =
            resolve_label_text(ac.label.as_deref(), ac.label_expr.as_ref(), &consts)
        {
            props.insert(*sym::NAME_LABEL, Literal::String(label));
        }
        if let Some(doc) = ctx.doc_comments.get(&crate::parser::doc_key(&ac.range)) {
            props.insert(*sym::DOC_TEXT, Literal::String(doc.clone()));
        }
    }
    style::bake_style(
        ctx,
        chip_node_id,
        ac.color_expr.as_ref(),
        ac.material_expr.as_ref(),
    );
    // Pair the node with its decl so `lower_anon_chip` finds it without a
    // whole-module scan. Keyed by (range, enclosing chip), the same pair the
    // scan matched on.
    ctx.anon_chip_nodes
        .insert((ac.range.clone(), ctx.current_anon_chip), chip_node_id);
    // Tag pre-declared nodes with chip_id.
    let saved = ctx.current_anon_chip.take();
    ctx.current_anon_chip = Some(chip_node_id);
    for s in &ac.body.stmts {
        match s {
            // Nested declaration shadows a same-named outer one for
            // any LATER initializer in this body. See
            // `pre_declare_chip_name`.
            Stmt::ChipDecl(c) => pre_declare_chip_name(ctx, c),
            Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
            Stmt::Buffer(b) => pre_declare_buffer(ctx, b),
            Stmt::Array(a) => pre_declare_array(ctx, a),
            Stmt::Map(m) => pre_declare_map(ctx, m),
            Stmt::In(i) => pre_declare_input(ctx, i),
            Stmt::OutBinding(o) if o.side.is_some() => {
                report_non_root_side(ctx, &o.range);
            }
            // An anon chip shares its parent's scope rather than opening one,
            // so a plain (non-`@side`) `out` written directly in its body is
            // a module-boundary output exactly like a top-level `out`, just
            // drawn inside a box; pre-declare it the same way.
            Stmt::OutBinding(o) => {
                if ctx.lookup_output(&o.name).is_some() {
                    continue;
                }
                ctx.with_nofold(o.no_fold, |ctx| {
                    pre_declare_output(
                        ctx,
                        &o.name,
                        o.value.as_ref(),
                        o.typ.as_ref(),
                        o.side,
                        o.label.as_deref(),
                        o.label_expr.as_ref(),
                        o.invisible,
                        true,
                        &o.range,
                    );
                });
            }
            _ => {}
        }
    }
    // The storage the body declares, for `lower_anon_chip` to bind again.
    let storage: Vec<(String, Binding)> = ac
        .body
        .stmts
        .iter()
        .filter_map(storage_name)
        .filter_map(|n| {
            let b = ctx.scope.get_current(&n)?.clone();
            Some((n, b))
        })
        .collect();
    ctx.anon_chip_storage.insert(chip_node_id, storage);
    for (name, b) in before {
        match b {
            Some(b) => ctx.scope.insert(&name, b),
            None => ctx.scope.remove(&name),
        };
    }
    ctx.current_anon_chip = saved;
}

/// Pre-declare a top-level `let x: exec` local signal: create a stable Union
/// "hub" gate, bind `x` to its `ExecOut` (so `on x` can trigger off it), and
/// register the emit target. `flush_pending_emits` later wires the union of all
/// `emit x` paths into the hub's `ExecA`. Non-`exec` lets are ignored here (they
/// lower normally in pass 2).
pub(super) fn pre_declare_exec_signal(ctx: &mut LowerCtx<'_>, l: &LetDecl) {
    let Some(
        te @ TypeExpr::Name {
            name: type_name, ..
        },
    ) = &l.typ
    else {
        return;
    };
    if type_name != "exec" && (l.has_initializer() || !is_exec_type(ctx, te)) {
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
pub(super) fn build_exec_signal_hub(ctx: &mut LowerCtx<'_>, name: &str, range: &SourceRange) {
    let hub = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: range.clone(),
        ports: GateIO::exec_join(),
        ..Default::default()
    });
    ctx.scope.insert(
        name,
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
    ctx.exec_signal_depths
        .insert(key.clone(), ctx.exec_branch_depth);
    ctx.pending_emits.entry(key).or_default();
}

pub(super) use crate::types::mono::unwrap_ref;

use crate::const_scope::Constants;

/// Resolve an explicit `@label` override to its baked display text: the
/// string form is used as-is, and `@label(expr)` is const-folded via
/// [`expr_to_literal_in`]. Typecheck rejects a non-constant expression, so a
/// fold failure yields `None` (no override; the caller's default applies).
pub(super) fn resolve_label_text<C: Constants + ?Sized>(
    label: Option<&str>,
    label_expr: Option<&Expr>,
    env: &C,
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
        Literal::Float(f) => fold::eval::render_for_format(&fold::eval::Value::Float(*f)),
        Literal::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Build the backing `Pseudo_ArrayVar` gate for `name` and bind the name as a
/// `VarStorage::Array`.
///
/// Every array var is built here, so the gate class, the `ArrayVarRef` port's
/// type and the `Binding::Var` record agree. `properties` is the caller's
/// (label, and `InitialValue` when it has constant contents to bake).
fn declare_array_var(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    elem_type: Type,
    properties: HashMap<Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    let node_id = make_array_var_gate(ctx, &elem_type, properties, range, None);
    ctx.scope.insert(
        name,
        Binding::Var(VarRecord::new(node_id, elem_type, VarStorage::Array)),
    );
    node_id
}

/// Build just the `Pseudo_ArrayVar` gate (no scope binding): the gate half of
/// [`declare_array_var`], shared with [`record_field_storage`].
pub(super) fn make_array_var_gate(
    ctx: &mut LowerCtx<'_>,
    elem_type: &Type,
    properties: HashMap<Sym, Literal>,
    range: &SourceRange,
    note: Option<&'static str>,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::ARRAY_VAR,
        source_range: range.clone(),
        ports: GateIO::array_storage(elem_type.clone()),
        properties,
        note,
        ..Default::default()
    })
}

/// Build the backing `Pseudo_MapVar` gate for `name` and bind the name as a
/// `VarStorage::Map` whose `inner_type` carries the whole `Type::Map(K, V)`.
/// The map counterpart of [`declare_array_var`].
fn declare_map_var(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    map_type: Type,
    properties: HashMap<Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    let node_id = make_map_var_gate(ctx, &map_type, properties, range);
    ctx.scope.insert(
        name,
        Binding::Var(VarRecord::new(node_id, map_type, VarStorage::Map)),
    );
    node_id
}

/// Build just the `Pseudo_MapVar` gate (no scope binding): the gate half of
/// [`declare_map_var`], shared with [`record_field_storage`].
fn make_map_var_gate(
    ctx: &mut LowerCtx<'_>,
    map_type: &Type,
    properties: HashMap<Sym, Literal>,
    range: &SourceRange,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::MAP_VAR,
        source_range: range.clone(),
        ports: GateIO::map_storage(map_type.clone()),
        properties,
        note: None,
        ..Default::default()
    })
}

/// Build just the `Pseudo_Var` scalar gate (no scope binding): the gate half of
/// [`pre_declare_var`]'s scalar tail, shared with [`record_field_storage`].
pub(super) fn make_scalar_var_gate(
    ctx: &mut LowerCtx<'_>,
    inner_type: &Type,
    properties: HashMap<Sym, Literal>,
    range: &SourceRange,
    note: Option<&'static str>,
) -> NodeId {
    ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR,
        source_range: range.clone(),
        ports: GateIO::var_storage(inner_type.clone()),
        properties,
        note,
        ..Default::default()
    })
}

/// The named fields of a record-literal initializer (`= { x: e, y }`), by field
/// name, so [`record_field_storage`] can bake each backing gate's `InitialValue`
/// from the matching sub-expression. Shorthand `{ x }` reads as `x: x`; spreads
/// have no place in a pure decl initializer and are skipped.
fn init_record_fields(init: Option<&Expr>) -> HashMap<String, &Expr> {
    let mut out = HashMap::default();
    let Some(Expr::RecordLit { fields, .. }) = init else {
        return out;
    };
    for f in fields {
        match f {
            RecordLitField::Named { name, value, .. } => {
                out.insert(name.clone(), value);
            }
            // `{ x }` shorthand carries no expression node to fold; the field
            // falls back to its type default here (an explicit `x: x` bakes).
            RecordLitField::Shorthand { .. } | RecordLitField::Spread { .. } => {}
        }
    }
    out
}

/// How deep a record's field tree may be decomposed before lowering gives up.
/// A self-naming record alias (`type L = { tail: L }`) has no finite layout,
/// and lowering still runs on the rejected program, so every alias-following
/// walk needs this bound. Matches the type resolver's `MAX_ALIAS_DEPTH`.
const MAX_RECORD_FIELD_DEPTH: u32 = 32;

/// Build the per-field storage for one field of a record-typed variable,
/// recursing for nested records, and return its `Binding` (it lives in the
/// enclosing `Binding::Record`, not the scope).
///
/// `kind` is the record's own storage kind: `Var` gives each field a container
/// of the field's own type; `Array`/`Map` give each scalar field a parallel
/// array/map of that type (see [`declare_record_container`]).
fn record_field_storage(
    ctx: &mut LowerCtx<'_>,
    kind: VarStorage,
    label_base: &str,
    field_name: &str,
    field_typ: &TypeExpr,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
    depth: u32,
) -> Binding {
    let label = format!("{label_base}.{field_name}");
    let expanded = container_alias(ctx, field_typ);
    let field_typ = expanded.as_ref().unwrap_or(field_typ);
    // Past `MAX_RECORD_FIELD_DEPTH`, stop decomposing and give the field a
    // single scalar gate.
    let too_deep = depth >= MAX_RECORD_FIELD_DEPTH;
    if depth == MAX_RECORD_FIELD_DEPTH {
        ctx.diagnostics.push(Diagnostic::error(
            "WS002",
            format!(
                "record type nests more than {MAX_RECORD_FIELD_DEPTH} levels deep at \
                 `{label}`: a record alias that names itself has no finite storage layout"
            ),
            range.clone(),
        ));
    }
    // A nested record (inline or via an alias) recurses whatever the record's
    // own storage kind: in a record array it is parallel arrays one level
    // deeper. `record_or_tuple_fields` follows aliases at every level.
    if let Some(sub_fields) = ctx.record_or_tuple_fields(field_typ).filter(|_| !too_deep) {
        let sub_inits = init_record_fields(init);
        let mut fmap = HashMap::default();
        for f in &sub_fields {
            fmap.insert(
                intern(&f.name),
                record_field_storage(
                    ctx,
                    kind,
                    &label,
                    &f.name,
                    &f.typ,
                    sub_inits.get(&f.name).copied(),
                    map_key,
                    range,
                    depth + 1,
                ),
            );
        }
        // A nested record named rather than spelled (`{ i: CI }`) bakes as
        // `var p: P = CP` does.
        if kind == VarStorage::Var
            && let Some(lit @ Literal::Record(_)) = init
                .filter(|e| !matches!(e, Expr::RecordLit { .. }))
                .and_then(|e| fold_const(ctx, e))
        {
            bake_record_var_fields(ctx, &fmap, &lit);
        }
        return Binding::Record(fmap);
    }
    // An enum field is multi-gate like a nested record: its own `__disc` plus
    // the superset of every variant's payload slots. A single scalar gate
    // would make tag and payload share a slot and drop writes to the field.
    // Inside a record array or map the columns become parallel arrays/maps.
    if let Some(cols) = enum_container_columns(ctx, field_typ, range).filter(|_| !too_deep) {
        if kind == VarStorage::Var {
            let field_type = ctx.resolve_local_type(field_typ);
            if let Type::Enum { args, .. } = &field_type
                && let Some(def) = ctx.enum_defs.of(&field_type).cloned()
            {
                // The field's initializer bakes as a top-level enum var's does.
                let known = init.and_then(|e| static_enum_ctor(ctx, &def, e));
                let fields =
                    build_enum_fields(ctx, &def, args, &label, known.as_ref(), None, range);
                return Binding::Record(fields);
            }
        }
        let mut fmap = HashMap::default();
        for c in &cols {
            fmap.insert(
                intern(&c.name),
                record_field_storage(
                    ctx,
                    kind,
                    &label,
                    &c.name,
                    &c.typ,
                    None,
                    map_key,
                    range,
                    depth + 1,
                ),
            );
        }
        return Binding::Record(fmap);
    }
    // A record variable's array or map field with a columnar element
    // (`type H = { pts: P[] }`) is stored like a top-level `var pts: P[]`: one
    // parallel column per element field, bound as a nested `Binding::Record`.
    if kind == VarStorage::Var
        && !too_deep
        && let Some(binding) = columnar_container_field(ctx, &label, field_typ, init, range, depth)
    {
        return binding;
    }
    let field_type = ctx.resolve_local_type(field_typ);
    let field_type = &field_type;
    // Taken before the gate-building borrows `ctx` mutably below.
    let consts = ctx.const_lookup();
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(label.clone()));
    match kind {
        // A record variable: each field is a container of the field's own type
        // (scalar/array/map), like a record input port.
        VarStorage::Var => match field_type {
            Type::Array(elem) => {
                if let Some(Expr::Array { elements, .. }) = init {
                    let lits: Vec<Literal> = elements
                        .iter()
                        .filter_map(|el| array_elem_literal(el, ctx))
                        .map(|lit| bake_string_bool(lit, elem))
                        .collect();
                    if lits.len() == elements.len() {
                        properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
                    }
                } else if let Some(Literal::Array(items)) =
                    init.and_then(|e| const_container_init(ctx, e))
                {
                    let lits = items
                        .into_iter()
                        .map(|lit| bake_string_bool(lit, elem))
                        .collect();
                    properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
                }
                let node_id = make_array_var_gate(ctx, elem, properties, range, None);
                Binding::Var(VarRecord::new(node_id, (**elem).clone(), VarStorage::Array))
            }
            Type::Map(k, v) => {
                let map_type = Type::Map(k.clone(), v.clone());
                // Only a fully constant literal bakes; `var_init_unbaked` reports
                // the rest where no exec-context assignment rebuilds it.
                if let Some(Expr::MapLit { entries, .. }) = init
                    && let Ok(pairs) = map_literal_pairs(ctx, entries, k, v)
                {
                    properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
                } else if let Some(Literal::Map(pairs)) =
                    init.and_then(|e| const_container_init(ctx, e))
                {
                    let coerce = |(a, b): (Literal, Literal)| {
                        (
                            crate::const_eval::scalar_in_type(a, k),
                            crate::const_eval::scalar_in_type(b, v),
                        )
                    };
                    properties.insert(
                        *sym::INITIAL_VALUE,
                        Literal::Map(pairs.into_iter().map(coerce).collect()),
                    );
                }
                let node_id = make_map_var_gate(ctx, &map_type, properties, range);
                Binding::Var(VarRecord::new(node_id, map_type, VarStorage::Map))
            }
            scalar => {
                let init_lit = init
                    // `_in`, so an initializer naming a constant
                    // (`var h: float = K`) resolves through the const env.
                    .and_then(|e| expr_to_literal_in(e, &consts))
                    .map(|lit| crate::const_eval::scalar_in_type(lit, scalar))
                    .or_else(|| default_literal_for_var_type(scalar));
                if let Some(lit) = init_lit {
                    properties.insert(*sym::INITIAL_VALUE, lit);
                }
                let node_id = make_scalar_var_gate(ctx, scalar, properties, range, None);
                Binding::Var(VarRecord::new(node_id, scalar.clone(), VarStorage::Var))
            }
        },
        // A record array: each scalar field backs onto its own parallel array.
        // Typecheck rejects container-typed fields inside an array element.
        VarStorage::Array => {
            let node_id = make_array_var_gate(ctx, field_type, properties, range, None);
            Binding::Var(VarRecord::new(
                node_id,
                field_type.clone(),
                VarStorage::Array,
            ))
        }
        // A record map: each scalar field backs onto its own parallel map with
        // the same key type.
        VarStorage::Map => {
            let key = map_key.cloned().unwrap_or(Type::Any);
            let map_type = Type::Map(Box::new(key), Box::new(field_type.clone()));
            let node_id = make_map_var_gate(ctx, &map_type, properties, range);
            Binding::Var(VarRecord::new(node_id, map_type, VarStorage::Map))
        }
        VarStorage::Buffer => {
            let node_id = make_scalar_var_gate(ctx, field_type, properties, range, None);
            Binding::Var(VarRecord::new(node_id, field_type.clone(), VarStorage::Var))
        }
    }
}

/// The array or map type a (chain of) alias names: `Pts` for `type Pts = P[]`
/// reads as `P[]`, and `Arr<P>` for `type Arr<T> = T[]` reads as `P[]`. `None`
/// when `te` is not an alias of a container, which leaves record and enum
/// aliases to their own resolvers.
fn container_alias(ctx: &LowerCtx<'_>, te: &TypeExpr) -> Option<TypeExpr> {
    let is_container = |t: &TypeExpr| match t {
        TypeExpr::Array { .. } => true,
        TypeExpr::Generic { name, args, .. } => name == "Map" && args.len() == 2,
        _ => false,
    };
    let mut cur = te.clone();
    for _ in 0..MAX_RECORD_FIELD_DEPTH {
        let next = match &cur {
            TypeExpr::Name { name, range } => ctx.alias_at(name, range)?.clone(),
            TypeExpr::Generic { name, args, range } if !is_container(&cur) => {
                ctx.generic_alias_at(name, args, range)?
            }
            _ => return None,
        };
        if is_container(&next) {
            return Some(next);
        }
        cur = next;
    }
    None
}

/// The columns of a record-element array field (`pts: P[]`) or record-value map
/// field (`m: Map<K, P>`) of a record variable: the same per-field
/// `Pseudo_ArrayVar` / `Pseudo_MapVar` layout `pre_declare_var` gives a top-level
/// `var pts: P[]` / `var m: Map<K, P>`, labelled under `label`, with a constant
/// initializer baked per column. `None` when the field is not an array or map of
/// a record, tuple or enum element.
fn columnar_container_field(
    ctx: &mut LowerCtx<'_>,
    label: &str,
    field_typ: &TypeExpr,
    init: Option<&Expr>,
    range: &SourceRange,
    depth: u32,
) -> Option<Binding> {
    let (kind, elem_te, map_key) = match field_typ {
        TypeExpr::Array { inner, .. } => (VarStorage::Array, inner.as_ref(), None),
        TypeExpr::Generic { name, args, .. } if name == "Map" && args.len() == 2 => (
            VarStorage::Map,
            &args[1],
            Some(ctx.resolve_local_type(&args[0])),
        ),
        _ => return None,
    };
    let cols = ctx
        .record_or_tuple_fields(elem_te)
        .or_else(|| enum_container_columns(ctx, elem_te, range))?;
    let mut fmap = HashMap::default();
    for c in &cols {
        fmap.insert(
            intern(&c.name),
            record_field_storage(
                ctx,
                kind,
                label,
                &c.name,
                &c.typ,
                None,
                map_key.as_ref(),
                range,
                depth + 1,
            ),
        );
    }
    match init {
        Some(Expr::Array { elements, .. }) if kind == VarStorage::Array => {
            let rows: Vec<RowExprs<'_>> = elements
                .iter()
                .filter_map(|el| match el {
                    ArrayElem::Item(e) => Some((None, e)),
                    ArrayElem::Spread(_) => None,
                })
                .collect();
            if rows.len() == elements.len() {
                bake_rows(ctx, &fmap, &rows);
            }
        }
        Some(Expr::MapLit { entries, .. }) if kind == VarStorage::Map => {
            let rows: Vec<RowExprs<'_>> = entries
                .iter()
                .map(|en| (Some(&en.key), &en.value))
                .collect();
            bake_rows(ctx, &fmap, &rows);
        }
        Some(e) => {
            if let Some(lit) = const_container_init(ctx, e) {
                bake_constant_columns(ctx, &fmap, lit);
            }
        }
        None => {}
    }
    Some(Binding::Record(fmap))
}

/// The parallel-column list for an enum stored in a container (an enum-element
/// array or an enum-valued map), as synthetic record fields so the
/// record-container machinery applies unchanged.
///
/// The columns are the enum value layout: `__disc` plus every variant's
/// payload slots, keyed as `build_enum_fields` and `lower_enum_ctor` key them,
/// so a constructed value lines up with the columns key for key. `None` for an
/// unknown enum, which leaves the caller on its single-column path.
pub(super) fn enum_container_columns(
    ctx: &LowerCtx<'_>,
    elem_te: &TypeExpr,
    range: &SourceRange,
) -> Option<Vec<RecordTypeField>> {
    use crate::typecheck::enums::Payload;
    // The element's own annotation, read where it is written (an alias as the
    // type it names).
    let def = ctx.enum_defs.of(&ctx.resolve_local_type(elem_te))?;
    let mut cols = vec![RecordTypeField {
        name: DISC_FIELD.to_string(),
        typ: TypeExpr::Name {
            name: "int".to_string(),
            range: range.clone(),
        },
        range: range.clone(),
    }];
    for v in &def.variants {
        match &v.payload {
            Payload::Unit => {}
            Payload::Positional(types) => {
                for (i, te) in types.iter().enumerate() {
                    cols.push(RecordTypeField {
                        name: slot_name(&v.name, i),
                        typ: te.clone(),
                        range: range.clone(),
                    });
                }
            }
            Payload::Named(nfields) => {
                for (fname, te) in nfields {
                    cols.push(RecordTypeField {
                        name: slot_name(&v.name, fname),
                        typ: te.clone(),
                        range: range.clone(),
                    });
                }
            }
        }
    }
    Some(cols)
}

/// Decompose a record-typed storage declaration (`var p: Rec`, `var a: Rec[]`,
/// `var m: Map<K, Rec>`) into one `Binding::Record` of per-field backing gates,
/// and bind it in scope under `name`. `kind` selects the per-field backing kind;
/// `map_key` carries the map's key type for the `Map` case.
fn declare_record_container(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    kind: VarStorage,
    fields: &[RecordTypeField],
    label_base: &str,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
) {
    let record_fields = build_record_fields(ctx, kind, fields, label_base, init, map_key, range);
    ctx.scope.insert(name, Binding::Record(record_fields));
}

/// The per-field storage `Binding::Record` of a record var/param, without
/// binding it in scope (see [`declare_record_container`], which does).
pub(super) fn build_record_fields(
    ctx: &mut LowerCtx<'_>,
    kind: VarStorage,
    fields: &[RecordTypeField],
    label_base: &str,
    init: Option<&Expr>,
    map_key: Option<&Type>,
    range: &SourceRange,
) -> Fields {
    let sub_inits = init_record_fields(init);
    let mut record_fields = HashMap::default();
    for f in fields {
        record_fields.insert(
            intern(&f.name),
            record_field_storage(
                ctx,
                kind,
                label_base,
                &f.name,
                &f.typ,
                sub_inits.get(&f.name).copied(),
                map_key,
                range,
                0,
            ),
        );
    }
    record_fields
}

/// A construction expression (`Dir.E`, `Shape.Circle(5.0)`,
/// `Box.Dims { w: 1.0, h: 2.0 }`) whose variant, and any foldable payload arg,
/// is known at pre-declare time. Bakes a stored enum's initializer; a live
/// construction goes through `lower_enum_ctor` instead.
pub(super) struct StaticEnumCtor {
    variant: String,
    disc: i64,
    /// `(slot key, literal value)` for every payload arg that folded: a
    /// positional arg's key is its index, a named field's its name. An arg that
    /// didn't fold is absent; the tag still bakes.
    slots: Vec<(String, Literal)>,
}

/// Recognize `e` as a construction of `enum_name` with a statically-known
/// variant, per [`StaticEnumCtor`]. `None` for anything else (a non-constant
/// enum-typed expression, a different enum, a plain identifier).
fn static_enum_ctor(
    ctx: &LowerCtx<'_>,
    def: &crate::typecheck::enums::EnumDef,
    e: &Expr,
) -> Option<StaticEnumCtor> {
    let enum_name = def.name.as_str();
    // Every qualified construction form starts with this `Enum.Variant` shape
    // (bare for unit, a call callee for positional, a VariantCtor path for named).
    let is_enum_obj = |obj: &Expr| ctx.enum_head(obj) == Some(enum_name);
    match e {
        Expr::FieldAccess {
            obj,
            field: variant,
            ..
        } if is_enum_obj(obj) => {
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots: Vec::new(),
            })
        }
        // Bare unit-variant reference (`None` for `Option.None`), when not
        // shadowed by a scope symbol. Typecheck already rejected an ambiguous
        // bare name (`typecheck::enums::resolve_bare_variant_enum`), so only
        // membership in the var's declared enum needs checking.
        Expr::Ident { name: variant, .. } if ctx.scope.get(variant).is_none() => {
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots: Vec::new(),
            })
        }
        Expr::Call { callee, args, .. } => {
            let variant = match callee.as_ref() {
                Expr::FieldAccess {
                    obj,
                    field: variant,
                    ..
                } if is_enum_obj(obj) => variant,
                // Bare positional construction (`Some(42)`), checked as the
                // bare unit-reference arm above is.
                Expr::Ident { name: variant, .. } if ctx.scope.get(variant).is_none() => variant,
                _ => return None,
            };
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            let mut slots = Vec::new();
            let mut i = 0usize;
            for a in args {
                if let CallArg::Positional(v) = a {
                    if let Some(lit) = fold_const(ctx, v) {
                        slots.push((i.to_string(), lit));
                    }
                    i += 1;
                }
            }
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots,
            })
        }
        Expr::VariantCtor { path, fields, .. } => {
            let Expr::FieldAccess {
                obj,
                field: variant,
                ..
            } = path.as_ref()
            else {
                return None;
            };
            if !is_enum_obj(obj) {
                return None;
            }
            let vdef = def.variants.iter().find(|v| &v.name == variant)?;
            let mut slots = Vec::new();
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        if let Some(lit) = fold_const(ctx, value) {
                            slots.push((name.clone(), lit));
                        }
                    }
                    RecordLitField::Shorthand { name, range } => {
                        let ident = Expr::Ident {
                            name: name.clone(),
                            range: range.clone(),
                        };
                        if let Some(lit) = fold_const(ctx, &ident) {
                            slots.push((name.clone(), lit));
                        }
                    }
                    RecordLitField::Spread { .. } => {}
                }
            }
            Some(StaticEnumCtor {
                variant: variant.clone(),
                disc: vdef.discriminant,
                slots,
            })
        }
        _ => None,
    }
}

/// Reconstruct a [`StaticEnumCtor`] for a nested enum value from its folded
/// `Literal::Record` (`Color.Green` folds to `{__disc: Int(1)}`): the tag from
/// `__disc`, and each slot key by stripping the `__<variant>_` prefix. `None`
/// when there is no `Int` `__disc` or it matches no variant.
fn static_enum_ctor_from_literal(
    def: &crate::typecheck::enums::EnumDef,
    fields: &[(String, Literal)],
) -> Option<StaticEnumCtor> {
    let disc = fields.iter().find_map(|(n, v)| match (n.as_str(), v) {
        (DISC_FIELD, Literal::Int(d)) => Some(*d),
        _ => None,
    })?;
    let vdef = def.variants.iter().find(|v| v.discriminant == disc)?;
    let prefix = format!("__{}_", vdef.name);
    let slots = fields
        .iter()
        .filter_map(|(n, v)| {
            n.strip_prefix(&prefix)
                .map(|key| (key.to_string(), v.clone()))
        })
        .collect();
    Some(StaticEnumCtor {
        variant: vdef.name.clone(),
        disc,
        slots,
    })
}

/// Decompose an enum-typed variable (`var d: Dir`) into the superset
/// `Binding::Record`: an int `__disc` `Pseudo_Var` plus one payload slot gate
/// per (variant, field) across every variant (see `LowerCtx::enum_defs`).
/// A statically-known constructor `init` ([`static_enum_ctor`]) bakes; anything
/// else leaves the defaults (first-variant discriminant, zeroed slots).
fn declare_enum_container(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    enum_ty: &Type,
    args: &[Type],
    label_base: &str,
    init: Option<&Expr>,
    range: &SourceRange,
) {
    let Some(def) = ctx.enum_defs.of(enum_ty).cloned() else {
        // An unknown enum (possible in a `LowerCtx` seeded with empty
        // `enum_defs`) falls back to a bare scalar so lowering doesn't panic.
        let node_id = make_scalar_var_gate(ctx, &Type::Int, HashMap::default(), range, None);
        ctx.scope.insert(
            name,
            Binding::Var(VarRecord::new(node_id, Type::Int, VarStorage::Var)),
        );
        return;
    };
    let known = init.and_then(|e| static_enum_ctor(ctx, &def, e));
    let fields = build_enum_fields(ctx, &def, args, label_base, known.as_ref(), None, range);
    ctx.scope.insert(name, Binding::Record(fields));
}

/// Build the `__disc` + per-(variant, field) payload-slot bindings for one
/// enum value. `known` bakes its discriminant and foldable payload args;
/// without it `__disc` defaults to the first variant and every slot is zeroed.
///
/// `disc_override` binds `__disc` directly to that value port instead of a
/// `Pseudo_Var` (the `Enum.FromInt(n)` path, see
/// `lower::expr::try_lower_enum_from_int`). Pass at most one of the two.
pub(super) fn build_enum_fields(
    ctx: &mut LowerCtx<'_>,
    def: &crate::typecheck::enums::EnumDef,
    args: &[Type],
    label_base: &str,
    known: Option<&StaticEnumCtor>,
    disc_override: Option<PortRef>,
    range: &SourceRange,
) -> Fields {
    use crate::typecheck::enums::Payload;
    let mut fields = HashMap::default();

    let disc_binding = if let Some(port) = disc_override {
        Binding::Local(LocalRecord { port })
    } else {
        let mut disc_props = HashMap::default();
        disc_props.insert(
            *sym::NAME_LABEL,
            Literal::String(format!("{label_base}.__disc")),
        );
        let disc_init = known
            .map(|k| k.disc)
            .or_else(|| def.variants.first().map(|v| v.discriminant))
            .unwrap_or(0);
        disc_props.insert(*sym::INITIAL_VALUE, Literal::Int(disc_init));
        let disc_node = make_scalar_var_gate(ctx, &Type::Int, disc_props, range, None);
        Binding::Var(VarRecord::new(disc_node, Type::Int, VarStorage::Var))
    };
    fields.insert(*DISC, disc_binding);

    for v in &def.variants {
        let slot_init = |slot_key: &str| -> Option<Literal> {
            known
                .filter(|k| k.variant == v.name)
                .and_then(|k| k.slots.iter().find(|(s, _)| s == slot_key))
                .map(|(_, lit)| lit.clone())
        };
        match &v.payload {
            Payload::Unit => {}
            Payload::Positional(types) => {
                for (i, te) in types.iter().enumerate() {
                    let slot_key = i.to_string();
                    let slot = slot_name(&v.name, &slot_key);
                    let init_lit = slot_init(&slot_key);
                    let label = format!("{label_base}.{slot}");
                    fields.insert(
                        intern(&slot),
                        enum_payload_slot(ctx, te, &def.type_params, args, &label, init_lit, range),
                    );
                }
            }
            Payload::Named(nfields) => {
                for (fname, te) in nfields {
                    let slot = slot_name(&v.name, fname);
                    let init_lit = slot_init(fname);
                    let label = format!("{label_base}.{slot}");
                    fields.insert(
                        intern(&slot),
                        enum_payload_slot(ctx, te, &def.type_params, args, &label, init_lit, range),
                    );
                }
            }
        }
    }
    fields
}

/// Resolve one enum payload field's declared type through the enclosing enum's
/// generic instantiation: a bare type parameter (`Wrap<Color>`'s `W(T)`) becomes
/// its argument (`Color`), so the slot lays out as that argument's storage. Any
/// other type resolves normally, then has the enum's parameters substituted.
fn resolve_payload_slot_type(
    ctx: &LowerCtx<'_>,
    field_typ: &TypeExpr,
    type_params: &[TypeParam],
    args: &[Type],
) -> Type {
    if let TypeExpr::Name { name, .. } = field_typ
        && let Some(idx) = type_params.iter().position(|p| &p.name == name)
    {
        return args.get(idx).cloned().unwrap_or(Type::Any);
    }
    let resolved = ctx.resolve_local_type(field_typ);
    if type_params.is_empty() || args.is_empty() {
        return resolved;
    }
    let subst: crate::types::infer::Subst = type_params
        .iter()
        .zip(args)
        .map(|(p, a)| (p.name.clone(), a.clone()))
        .collect();
    crate::types::mono::substitute(&resolved, &subst)
}

/// The storage for one enum payload slot: a nested enum recurses
/// ([`build_enum_fields`]), a nested record decomposes per field, an array/map
/// gets its own container gate, anything else is a scalar `Pseudo_Var` baked
/// from `init_lit`. Always `VarStorage::Var`-shaped. `type_params`/`args` are
/// the enclosing enum's instantiation (see [`resolve_payload_slot_type`]).
fn enum_payload_slot(
    ctx: &mut LowerCtx<'_>,
    field_typ: &TypeExpr,
    type_params: &[TypeParam],
    args: &[Type],
    label: &str,
    init_lit: Option<Literal>,
    range: &SourceRange,
) -> Binding {
    let field_type = resolve_payload_slot_type(ctx, field_typ, type_params, args);
    if let Type::Enum {
        args: nested_args, ..
    } = &field_type
        && let Some(nested_def) = ctx.enum_defs.of(&field_type).cloned()
    {
        // A nested enum initializer folds to a `Literal::Record` for this slot;
        // recover its variant so the inner `__disc`/slots bake from it.
        let nested_known = match &init_lit {
            Some(Literal::Record(rf)) => static_enum_ctor_from_literal(&nested_def, rf),
            _ => None,
        };
        let fields = build_enum_fields(
            ctx,
            &nested_def,
            nested_args,
            label,
            nested_known.as_ref(),
            None,
            range,
        );
        return Binding::Record(fields);
    }
    // A payload typed by one of the enum's own type parameters (`Opt<P>`'s
    // `Some(T)`) is laid out as the record or tuple this instantiation binds
    // it to, field by field, exactly as a payload written `Some(P)` is.
    let instantiated;
    let field_typ = match &field_type {
        Type::Record(_) | Type::Tuple(_) if ctx.record_or_tuple_fields(field_typ).is_none() => {
            instantiated = super::context::type_expr_of(&field_type, range);
            &instantiated
        }
        _ => field_typ,
    };
    if let Some(sub_fields) = ctx.record_or_tuple_fields(field_typ) {
        // `init_lit` for a record slot is a folded `Literal::Record`; thread its
        // per-field values down by key, since nothing at load rewrites a
        // declaration initializer's payload.
        let sub_lits: HashMap<&str, &Literal> = match &init_lit {
            Some(Literal::Record(rf)) => rf.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            _ => HashMap::default(),
        };
        let mut fmap = HashMap::default();
        for f in &sub_fields {
            let sub_label = format!("{label}.{}", f.name);
            fmap.insert(
                intern(&f.name),
                enum_payload_slot(
                    ctx,
                    &f.typ,
                    type_params,
                    args,
                    &sub_label,
                    sub_lits.get(f.name.as_str()).map(|l| (*l).clone()),
                    range,
                ),
            );
        }
        return Binding::Record(fmap);
    }
    let mut props = HashMap::default();
    props.insert(*sym::NAME_LABEL, Literal::String(label.to_string()));
    match &field_type {
        Type::Array(elem) => {
            let node_id = make_array_var_gate(ctx, elem, props, range, None);
            Binding::Var(VarRecord::new(node_id, (**elem).clone(), VarStorage::Array))
        }
        Type::Map(k, v) => {
            let map_type = Type::Map(k.clone(), v.clone());
            let node_id = make_map_var_gate(ctx, &map_type, props, range);
            Binding::Var(VarRecord::new(node_id, map_type, VarStorage::Map))
        }
        scalar => {
            if let Some(lit) = init_lit {
                props.insert(
                    *sym::INITIAL_VALUE,
                    crate::const_eval::scalar_in_type(lit, scalar),
                );
            } else if let Some(lit) = default_literal_for_var_type(scalar) {
                props.insert(*sym::INITIAL_VALUE, lit);
            }
            let node_id = make_scalar_var_gate(ctx, scalar, props, range, None);
            Binding::Var(VarRecord::new(node_id, scalar.clone(), VarStorage::Var))
        }
    }
}

/// Give a `const` array/map its runtime form: the container gate a `var` would
/// get, with the contents baked into `InitialValue`, bound in scope so every
/// ordinary container path resolves it like a `var`.
///
/// Lazy: called only from runtime paths, so a container used only at compile
/// time emits no gate. The scope binding is the memo, so N reads share one
/// gate, and it lives in the frame open at the first runtime use (one per
/// inline instance of a `mod`/`chip` body).
///
/// `None`, changing nothing, unless `obj` is a bare name bound to a constant
/// array/map with no binding of its own yet, so a real container of the same
/// name always wins. An already materialized name returns its record. The new
/// node joins `ctx.immutable_containers`, which keeps mutation rejected.
pub(super) fn materialize_const_container(ctx: &mut LowerCtx<'_>, obj: &Expr) -> Option<VarRecord> {
    let Expr::Ident { name, range } = obj else {
        return None;
    };
    if let Some(rec) = ctx.lookup_var(name) {
        // A gate built further out for another constant of this name (an
        // importer's, around a namespaced body recording its own module's)
        // is not this constant's: this one gets a gate of its own.
        let other = ctx.immutable_containers.contains(&rec.node_id)
            && ctx.const_container_literal(name).is_some_and(|lit| {
                ctx.builder
                    .module
                    .nodes
                    .get(&rec.node_id)
                    .and_then(|n| n.properties.get(&*sym::INITIAL_VALUE))
                    != Some(&lit)
            });
        if !other {
            return matches!(rec.storage, VarStorage::Array | VarStorage::Map).then(|| rec.clone());
        }
    } else if ctx.scope.get(name).is_some() {
        return None;
    }
    let lit = ctx.const_container_literal(name)?;
    // Use the type checked for this read, which picked the method signature;
    // one re-derived from the literal could disagree with it. The literal is
    // the fallback when typecheck recorded nothing.
    let checked = unwrap_ref(&ctx.type_of(obj));
    let rec = const_container_gate(ctx, name, lit, checked, range)?;
    ctx.scope.insert(name, Binding::Var(rec.clone()));
    Some(rec)
}

/// A constant container `name` a body records (a namespaced body's module
/// constant) while a gate further out holds another constant of the name (the
/// importer's, materialized where the body can see it): every read of `name`
/// in the body would find that gate first, so the body's own is built and
/// bound here.
pub(super) fn own_constant_container(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    lit: &Literal,
    range: &SourceRange,
) {
    if matches!(lit, Literal::Array(_) | Literal::Map(_)) && ctx.lookup_var(name).is_some() {
        materialize_const_container(
            ctx,
            &Expr::Ident {
                name: name.to_string(),
                range: range.clone(),
            },
        );
    }
}

/// A constant `let`/`const` container that shadows a binding of the same name
/// (a `var`, an `in` port, a parameter, an earlier materialization): it cannot
/// stay lazy, since every runtime read would find the outer binding first, so
/// its immutable gate is built and bound here. A no-op for an unbound name.
pub(super) fn shadow_with_const_container(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    lit: Literal,
    value: &Expr,
) {
    if ctx.scope.get(name).is_none() {
        return;
    }
    let checked = unwrap_ref(&ctx.type_of(value));
    if let Some(rec) = const_container_gate(ctx, name, lit, checked, value.range()) {
        ctx.scope.insert(name, Binding::Var(rec));
    }
}

/// Bind `const` parameter `name`, whose value is `lit` of type `ty`, in the
/// body's own frame: a record or enum value field by field, always; a scalar
/// as its literal and an array or map as its immutable container when an
/// outer binding of that name would otherwise answer a read of it. With no
/// outer binding a scalar or container read already resolves through the
/// constant environment, and nothing is bound for it. An array or map field
/// `body` never reads gets no container.
pub(super) fn bind_shadowing_const_param(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    lit: Literal,
    ty: &Type,
    range: &SourceRange,
    body: &Block,
) {
    if ctx.scope.get(name).is_none() && !matches!(lit, Literal::Record(_)) {
        return;
    }
    let read = field_reads(body, name);
    /// A constant value of type `ty` as literal gates: a record (or enum
    /// value) field by field, an array or map field as its immutable
    /// container labelled `label` when `read` holds it.
    fn literal_binding(
        ctx: &mut LowerCtx<'_>,
        label: &str,
        lit: Literal,
        ty: &Type,
        range: &SourceRange,
        read: Option<&HashSet<String>>,
    ) -> Option<Binding> {
        match lit {
            Literal::Record(fields) => {
                let mut out = HashMap::default();
                for (f, v) in fields {
                    let fty = match unwrap_ref(ty) {
                        Type::Record(tf) => {
                            tf.iter().find(|(n, _)| *n == f).map(|(_, t)| t.clone())
                        }
                        _ => None,
                    };
                    let fty = fty.unwrap_or(Type::Any);
                    let path = format!("{label}.{f}");
                    let unread = read.is_some_and(|r| {
                        !r.iter().any(|w| {
                            w == &path
                                || w.strip_prefix(path.as_str())
                                    .is_some_and(|rest| rest.starts_with('.'))
                                || path
                                    .strip_prefix(w.as_str())
                                    .is_some_and(|rest| rest.starts_with('.'))
                        })
                    });
                    if unread && matches!(v, Literal::Array(_) | Literal::Map(_)) {
                        continue;
                    }
                    let field = literal_binding(ctx, &path, v, &fty, range, read)?;
                    out.insert(intern(&f), field);
                }
                Some(Binding::Record(out))
            }
            lit @ (Literal::Array(_) | Literal::Map(_)) => {
                const_container_gate(ctx, label, lit, unwrap_ref(ty), range).map(Binding::Var)
            }
            lit => crate::lower::expr::wire_type_of_literal(&lit).map(|t| {
                Binding::Local(LocalRecord {
                    port: literal_node_range(ctx, range, t, lit),
                })
            }),
        }
    }
    let binding = literal_binding(ctx, name, lit, ty, range, read.as_ref());
    if let Some(b) = binding {
        ctx.scope.insert(name, b);
    }
}

/// Every field path `body` reads whole off the name `root` (`q.i.s` for
/// `q.i.s[k]`, `q.i` for `let inner = q.i`, which reads every field below
/// it), or `None` when it uses `root` other than to read a field of it,
/// which may read every field.
fn field_reads(body: &Block, root: &str) -> Option<HashSet<String>> {
    use crate::ast::walk::{Ctx, Node, walk};
    fn path(e: &Expr, root: &str) -> Option<String> {
        match e {
            Expr::Ident { name, .. } if name == root => Some(name.clone()),
            Expr::FieldAccess { obj, field, .. } => Some(format!("{}.{field}", path(obj, root)?)),
            _ => None,
        }
    }
    let mut reads: HashMap<String, (usize, usize)> = HashMap::default();
    let (mut uses, mut field_uses) = (0usize, 0usize);
    walk(Node::Block(body), Ctx::default(), &mut |n, _| {
        if let Node::Expr(e) = n {
            match e {
                Expr::Ident { name, .. } if name == root => uses += 1,
                // A method's receiver (`q.i.f()`) is read whole.
                Expr::Call { callee, .. } => {
                    if let Expr::FieldAccess { obj, .. } = callee.as_ref()
                        && let Some(q) = path(obj, root)
                    {
                        reads.entry(q).or_default().0 += 1;
                    }
                }
                Expr::FieldAccess { obj, .. } => {
                    if let Some(p) = path(e, root) {
                        reads.entry(p).or_default().0 += 1;
                        match obj.as_ref() {
                            Expr::Ident { name, .. } if name == root => field_uses += 1,
                            inner => {
                                if let Some(q) = path(inner, root) {
                                    reads.entry(q).or_default().1 += 1;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        true
    });
    (uses == field_uses).then(|| {
        reads
            .into_iter()
            .filter(|(_, (all, deeper))| all > deeper)
            .map(|(p, _)| p)
            .collect()
    })
}

/// An array/map literal ARGUMENT (`C(go, [10, 20, 30])`) as an immutable
/// container gate, the same form a `const` table gets, bound to no name. `None`
/// unless `e` is an array/map literal whose value is a compile-time constant.
pub(super) fn materialize_literal_container(
    ctx: &mut LowerCtx<'_>,
    e: &Expr,
    label: &str,
) -> Option<VarRecord> {
    let e = &*ctx.with_map_literals(e);
    if !matches!(e, Expr::Array { .. } | Expr::MapLit { .. }) {
        return None;
    }
    let lit = ctx.const_eval(e, ConstScope::Plain).ok()?;
    let checked = unwrap_ref(&ctx.type_of(e));
    // A record/tuple/enum element is several columns, not one gate.
    let columnar = |t: &Type| matches!(t, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. });
    if matches!(&checked, Type::Array(t) | Type::Map(_, t) if columnar(t)) {
        return None;
    }
    const_container_gate(ctx, label, lit, checked, e.range())
}

/// The immutable container gate holding `lit`, labelled `label`: an `ArrayVar`
/// or `MapVar` with the contents baked into `InitialValue`, added to
/// `ctx.immutable_containers`. `None` for a literal that is not an array/map.
fn const_container_gate(
    ctx: &mut LowerCtx<'_>,
    label: &str,
    lit: Literal,
    checked: Type,
    range: &SourceRange,
) -> Option<VarRecord> {
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(label.to_string()));
    match lit {
        Literal::Array(items) => {
            let elem_type = match checked {
                Type::Array(e) => *e,
                _ => items
                    .first()
                    .and_then(wire_type_of_literal)
                    .unwrap_or(Type::Any),
            };
            // Element-wise compile-time string -> bool, the same `!= ""` law the
            // `var` paths bake through (see `bake_string_bool`), and an integer
            // in a `float` table is the float it names.
            let items: Vec<Literal> = items
                .into_iter()
                .map(|lit| {
                    let lit = crate::const_eval::coerce_const_to_type(
                        lit,
                        &elem_type,
                        ctx.consts.types(),
                    );
                    bake_string_bool(lit, &elem_type)
                })
                .collect();
            properties.insert(*sym::INITIAL_VALUE, Literal::Array(items));
            let node_id = make_array_var_gate(ctx, &elem_type, properties, range, None);
            ctx.immutable_containers.insert(node_id);
            Some(VarRecord::new(node_id, elem_type, VarStorage::Array))
        }
        Literal::Map(pairs) => {
            let (key_ty, val_ty) = match checked {
                Type::Map(k, v) => (*k, *v),
                _ => match pairs.first() {
                    Some((k, v)) => (
                        wire_type_of_literal(k).unwrap_or(Type::Any),
                        wire_type_of_literal(v).unwrap_or(Type::Any),
                    ),
                    None => (Type::Any, Type::Any),
                },
            };
            // Coerce every entry to the map's own K/V, as `bake_map_init` does.
            let pairs: Vec<(Literal, Literal)> = pairs
                .into_iter()
                .map(|(k, v)| {
                    (
                        crate::const_eval::scalar_in_type(k, &key_ty),
                        crate::const_eval::scalar_in_type(v, &val_ty),
                    )
                })
                .collect();
            properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
            let map_type = Type::Map(Box::new(key_ty), Box::new(val_ty));
            let node_id = make_map_var_gate(ctx, &map_type, properties, range);
            ctx.immutable_containers.insert(node_id);
            Some(VarRecord::new(node_id, map_type, VarStorage::Map))
        }
        _ => None,
    }
}

/// The value type-expression of a `Map<K, V>` annotation (following an outer
/// `*Map<..>` ref), so a record-valued map decomposes through the alias-aware
/// field lookup. `None` for anything that isn't a `Map<_, _>`.
fn map_value_typeexpr(te: Option<&TypeExpr>) -> Option<&TypeExpr> {
    match te? {
        TypeExpr::Generic { name, args, .. } if name == "Map" && args.len() == 2 => Some(&args[1]),
        TypeExpr::Ref { inner, .. } => map_value_typeexpr(Some(inner)),
        _ => None,
    }
}

/// The text a `var`'s storage gate is labelled with in-game: its `@label` /
/// `@label(expr)` when it has one, its source name otherwise.
fn var_label(ctx: &LowerCtx<'_>, d: &VarDecl) -> String {
    resolve_label_text(
        d.label.as_deref(),
        d.label_expr.as_ref(),
        &ctx.const_lookup(),
    )
    .unwrap_or_else(|| d.name.clone())
}

/// Declare `d`'s storage gates and bake any constant `@color` / `@material`
/// onto them.
pub(super) fn pre_declare_var(ctx: &mut LowerCtx<'_>, d: &VarDecl) {
    pre_declare_var_storage(ctx, d);
    style::bake_var_style(ctx, d);
}

fn pre_declare_var_storage(ctx: &mut LowerCtx<'_>, d: &VarDecl) {
    // An alias naming a container (`type Pts = P[]`) declares that container.
    let expanded = d.typ.as_ref().and_then(|te| container_alias(ctx, te));
    let typ = expanded.as_ref().or(d.typ.as_ref());
    // `resolve_local_type` monomorphizes a `T` annotation inside a generic mod
    // body and reads every alias where it is declared. An unannotated var of a
    // gate read holds its first output, in a record's field too.
    let inner_type = typ
        .map(|te| ctx.resolve_local_type(te))
        .or_else(|| d.init.as_ref().and_then(|e| ctx.held_by_var_at(e.range())))
        .unwrap_or(Type::Any);
    // An unannotated array literal of enums (`var x = [c, d]`) is declared as
    // `var x: Hue[]` would be.
    let inferred_enum_array = match (&d.typ, &inner_type) {
        (None, Type::Array(elem)) => match elem.as_ref() {
            Type::Enum { name, args } if args.is_empty() => Some(TypeExpr::Array {
                inner: Box::new(TypeExpr::Name {
                    name: name.clone(),
                    range: d.range.clone(),
                }),
                range: d.range.clone(),
            }),
            _ => None,
        },
        _ => None,
    };
    let typ = typ.or(inferred_enum_array.as_ref());
    // An unannotated `var` of a record or tuple (`var r = m.get(k)` on a map
    // of records) is declared as its type written out would be.
    let written_out = match (typ, &inner_type) {
        (None, Type::Record(_) | Type::Tuple(_)) => Some(type_expr_of(&inner_type, &d.range)),
        _ => None,
    };
    let typ = typ.or(written_out.as_ref());

    match &inner_type {
        Type::Array(elem) => declare_array_var_storage(ctx, d, elem, typ),
        Type::Map(key_ty, value_ty) => {
            declare_map_var_storage(ctx, d, key_ty, value_ty, &inner_type, typ);
        }
        Type::Enum { args, .. } => declare_enum_var_storage(ctx, d, &inner_type, args),
        _ => {
            if !declare_record_var_storage(ctx, d, typ) {
                declare_scalar_var_storage(ctx, d, inner_type);
            }
        }
    }
}

/// Declare `var foo: T[]`: one `Pseudo_ArrayVar` gate, or one parallel array per
/// column for a record or enum element. A `= [..]` initializer bakes its constant
/// literals inline.
fn declare_array_var_storage(
    ctx: &mut LowerCtx<'_>,
    d: &VarDecl,
    elem: &Type,
    typ: Option<&TypeExpr>,
) {
    // `var pts: Rec[]` decomposes into one parallel array per field
    // (`pts.x`, `pts.y`), bound as a `Binding::Record`; every element op
    // fans out across them (see `lower_record_array_method`). An enum `S`
    // spans several wires, so `var arr: S[]` gets parallel columns too, one
    // per `__disc`/payload slot.
    if let Some(TypeExpr::Array { inner: elem_te, .. }) = typ
        && let Some(columns) = ctx
            .record_or_tuple_fields(elem_te)
            .or_else(|| enum_container_columns(ctx, elem_te, &d.range))
    {
        let label = var_label(ctx, d);
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Array,
            &columns,
            &label,
            None,
            None,
            &d.range,
        );
        // Bake a constant `= [{..}, ..]` initializer into the per-field arrays.
        if let Some(Expr::Array { elements, .. }) = &d.init {
            bake_record_array_init(ctx, &d.name, elements);
        }
        bake_const_columns(ctx, &d.name, d.init.as_ref());
        return;
    }
    let elem_type = elem.clone();
    let mut properties = HashMap::default();
    let label = var_label(ctx, d);
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    if let Some(Expr::Array { elements, .. }) = &d.init {
        // Element-wise compile-time string -> bool for `var v: bool[] =
        // [..]`, by the same `!= ""` law as the wire path's
        // CompareNotEqual gate (see `bake_string_bool`).
        let lits: Vec<Literal> = elements
            .iter()
            .filter_map(|el| array_elem_literal(el, ctx))
            .map(|lit| bake_string_bool(lit, &elem_type))
            .collect();
        if lits.len() == elements.len() {
            properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
        } else {
            bake_array_refs(ctx, &mut properties, &elem_type, elements);
        }
    } else if let Some(Literal::Array(lits)) =
        d.init.as_ref().and_then(|e| const_container_init(ctx, e))
    {
        let lits = lits
            .into_iter()
            .map(|lit| bake_string_bool(lit, &elem_type))
            .collect();
        properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
    }
    declare_array_var(ctx, &d.name, elem_type, properties, &d.range);
}

/// Declare `var m: Map<K, V>`: one `Pseudo_MapVar` gate, or one parallel map per
/// column for a record or enum value. A constant `= {...}` initializer bakes via
/// `bake_map_init`.
fn declare_map_var_storage(
    ctx: &mut LowerCtx<'_>,
    d: &VarDecl,
    key_ty: &Type,
    value_ty: &Type,
    map_type: &Type,
    typ: Option<&TypeExpr>,
) {
    // `var m: Map<K, Rec>` decomposes into parallel per-field maps with the
    // same key, bound as a `Binding::Record`; every map op fans out across
    // them (see `lower_record_map_method`). An enum value is one parallel map
    // per column, keyed identically.
    if let Some(val_te) = map_value_typeexpr(typ)
        && let Some(columns) = ctx
            .record_or_tuple_fields(val_te)
            .or_else(|| enum_container_columns(ctx, val_te, &d.range))
    {
        let label = var_label(ctx, d);
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Map,
            &columns,
            &label,
            None,
            Some(key_ty),
            &d.range,
        );
        // Bake a constant `= { k => {..} }` initializer into the per-field maps.
        if let Some(Expr::MapLit { entries, .. }) = &d.init {
            bake_record_map_init(ctx, &d.name, entries);
        }
        bake_const_columns(ctx, &d.name, d.init.as_ref());
        return;
    }
    let (key_ty, value_ty) = (key_ty.clone(), value_ty.clone());
    let mut properties = HashMap::default();
    let label = var_label(ctx, d);
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    bake_map_init(
        ctx,
        &mut properties,
        &d.name,
        d.init.as_ref(),
        &key_ty,
        &value_ty,
    );
    if let Some(Literal::Map(pairs)) = d.init.as_ref().and_then(|e| const_container_init(ctx, e)) {
        let pairs = pairs
            .into_iter()
            .map(|(k, v)| {
                (
                    crate::const_eval::scalar_in_type(k, &key_ty),
                    crate::const_eval::scalar_in_type(v, &value_ty),
                )
            })
            .collect();
        properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
    }
    declare_map_var(ctx, &d.name, map_type.clone(), properties, &d.range);
}

/// Declare `var d: Dir`: `__disc` plus every variant's payload slots (see
/// `declare_enum_container`), bound as a `Binding::Record`.
fn declare_enum_var_storage(ctx: &mut LowerCtx<'_>, d: &VarDecl, enum_ty: &Type, args: &[Type]) {
    let label = var_label(ctx, d);
    declare_enum_container(
        ctx,
        &d.name,
        enum_ty,
        args,
        &label,
        d.init.as_ref(),
        &d.range,
    );
}

/// Declare `var p: Rec`: per-field storage (recursing for nested records),
/// bound as a `Binding::Record` so `p.field` reads/writes the right backing
/// gate. Mirrors the record input-port expansion in `pre_declare_input`.
/// `false` when `typ` is not a record or tuple, which leaves the var scalar.
fn declare_record_var_storage(ctx: &mut LowerCtx<'_>, d: &VarDecl, typ: Option<&TypeExpr>) -> bool {
    let Some(fields) = typ.and_then(|te| ctx.record_or_tuple_fields(te)) else {
        return false;
    };
    let label = var_label(ctx, d);
    declare_record_container(
        ctx,
        &d.name,
        VarStorage::Var,
        &fields,
        &label,
        d.init.as_ref(),
        None,
        &d.range,
    );
    bake_const_record(ctx, &d.name, d.init.as_ref());
    true
}

/// Declare a scalar `var`: one Var gate, its initial value baked in the
/// declared type's shape.
fn declare_scalar_var_storage(ctx: &mut LowerCtx<'_>, d: &VarDecl, inner_type: Type) {
    let init_lit = d
        .init
        .as_ref()
        // `_in`, so an initializer naming a constant (`var h: float = K`)
        // resolves through the const env.
        .and_then(|e| expr_to_literal_in(e, &ctx.const_lookup()))
        // Bake in the declared type's shape: the Var gate's wire variant is
        // picked from the InitialValue's kind, so `var v: float = 0` must widen
        // to Float(0.0) (see `const_eval::scalar_in_type`).
        .map(|lit| crate::const_eval::scalar_in_type(lit, &inner_type))
        .or_else(|| default_literal_for_var_type(&inner_type));
    let mut properties = HashMap::default();
    let label = var_label(ctx, d);
    properties.insert(*sym::NAME_LABEL, Literal::String(label));
    if let Some(lit) = init_lit {
        properties.insert(*sym::INITIAL_VALUE, lit);
    }
    // `var h: *T = &x`: the ref is baked once bricks have addresses.
    if matches!(inner_type, Type::Ref(_))
        && let Some(target) = d.init.as_ref().and_then(|e| storage_ref_target(ctx, e))
    {
        properties.insert(*sym::HANDLE_TARGET, target_literal(target));
    }

    let node_id = make_scalar_var_gate(ctx, &inner_type, properties, &d.range, None);
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord::new(node_id, inner_type, VarStorage::Var)),
    );
}

pub(super) fn pre_declare_buffer(ctx: &mut LowerCtx<'_>, d: &BufferDecl) {
    let annotated = d.typ.as_ref().map(|te| ctx.resolve_local_type(te));
    let rhs_type = ctx.type_of(&d.init);
    let inner_type = annotated.unwrap_or_else(|| unwrap_ref(&rhs_type));

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BUFFER_TICKS,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::new(*sym::INPUT, inner_type.clone()),
                PortSpec::new(*sym::TICKS_TO_WAIT, Type::Int),
            ],
            outputs: vec![PortSpec::new(*sym::OUTPUT, inner_type.clone())],
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

pub(super) fn pre_declare_array(ctx: &mut LowerCtx<'_>, d: &ArrayDecl) {
    // A record element decomposes into parallel per-field arrays, as the
    // `var pts: Rec[]` form does (see `pre_declare_var`).
    if let Some(fields) = ctx.record_or_tuple_fields(&d.element_type) {
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Array,
            &fields,
            &d.name,
            None,
            None,
            &d.range,
        );
        if !d.init.is_empty() {
            bake_record_array_init(ctx, &d.name, &d.init);
        }
        return;
    }
    let elem_type = ctx.resolve_local_type(&d.element_type);
    // A constant initializer (`var foo: int[] = [1, 2, 3]`), every element a
    // literal, bakes into the ArrayVar's `InitialValue`.
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    if !d.init.is_empty() {
        // Element-wise string -> bool by the `!= ""` law (see
        // `bake_string_bool`): a raw String element in a Bool array variant
        // would get the gate's native content-aware truthiness at load.
        let lits: Vec<Literal> = d
            .init
            .iter()
            .filter_map(|el| array_elem_literal(el, ctx))
            .map(|lit| bake_string_bool(lit, &elem_type))
            .collect();
        if lits.len() == d.init.len() {
            properties.insert(*sym::INITIAL_VALUE, Literal::Array(lits));
        } else {
            bake_array_refs(ctx, &mut properties, &elem_type, &d.init);
        }
    }
    declare_array_var(ctx, &d.name, elem_type, properties, &d.range);
}

/// `var name: Map<K, V>`: create the backing `Pseudo_MapVar` gate and bind the
/// name as a `VarStorage::Map` whose `inner_type` carries the whole
/// `Type::Map(K, V)`. Mirrors [`pre_declare_array`].
pub(super) fn pre_declare_map(ctx: &mut LowerCtx<'_>, d: &MapDecl) {
    // A record value decomposes into parallel per-field maps, as the
    // `var m: Map<K, Rec>` form does (see `pre_declare_var`).
    if let Some(fields) = ctx.record_or_tuple_fields(&d.value_type) {
        let key_type = ctx.resolve_local_type(&d.key_type);
        declare_record_container(
            ctx,
            &d.name,
            VarStorage::Map,
            &fields,
            &d.name,
            None,
            Some(&key_type),
            &d.range,
        );
        if let Some(Expr::MapLit { entries, .. }) = &d.init {
            bake_record_map_init(ctx, &d.name, entries);
        }
        return;
    }
    let key_type = ctx.resolve_local_type(&d.key_type);
    let value_type = ctx.resolve_local_type(&d.value_type);
    let map_type = Type::Map(Box::new(key_type.clone()), Box::new(value_type.clone()));
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    bake_map_init(
        ctx,
        &mut properties,
        &d.name,
        d.init.as_ref(),
        &key_type,
        &value_type,
    );
    declare_map_var(ctx, &d.name, map_type, properties, &d.range);
}

/// Push the WS023 "annotation on a non-root port" diagnostic, shared by
/// `apply_port_side` and the anon-chip output path.
fn report_non_root_side(ctx: &mut LowerCtx<'_>, range: &SourceRange) {
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
    ctx: &mut LowerCtx<'_>,
    node_id: NodeId,
    side: Option<PortSide>,
    invisible: bool,
    range: &SourceRange,
) {
    if invisible && let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties)
            .insert(*sym::REROUTE_INVISIBLE, Literal::Bool(true));
    }
    let Some(side) = side else { return };
    if !ctx.is_root_module || ctx.current_anon_chip.is_some() {
        report_non_root_side(ctx, range);
        return;
    }
    if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(
            *sym::REROUTE_SIDE,
            Literal::String(side.as_str().to_string()),
        );
    }
}

/// One boundary pin per leaf field of a record-typed `in` port, recursing into
/// nested record fields. Pin names join with `_` at every level (`p_i_m`).
fn record_input_pins(
    ctx: &mut LowerCtx<'_>,
    te: &TypeExpr,
    prefix: &str,
    range: &SourceRange,
    depth: u32,
) -> Fields {
    if depth >= MAX_RECORD_FIELD_DEPTH {
        return HashMap::default();
    }
    let fields = ctx.record_or_tuple_fields(te).unwrap_or_default();
    let mut out = HashMap::default();
    for field in &fields {
        let port_name = format!("{prefix}_{}", field.name);
        if ctx.record_or_tuple_fields(&field.typ).is_some() {
            let inner = record_input_pins(ctx, &field.typ, &port_name, range, depth + 1);
            out.insert(intern(&field.name), Binding::Record(inner));
            continue;
        }
        let ft = ctx.resolve_local_type(&field.typ);
        let node_id = ctx.add_input(&port_name, ft.clone(), range.clone());
        // Array / Map / ref fields of a record-typed input port bind a
        // container ref-port (see `container_binding`); a scalar field is a
        // plain by-value input.
        let binding =
            match super::context::container_binding(ctx.ref_storage(None, &field.typ), &ft) {
                Some((storage, inner)) => Binding::Var(VarRecord::new(node_id, inner, storage)),
                None => Binding::Input(NodeRecord {
                    node_id,
                    ty: ft.clone(),
                }),
            };
        out.insert(intern(&field.name), binding);
    }
    out
}

/// The `out` twin of [`record_input_pins`]: one output pin per leaf field,
/// recursing into nested records.
pub(super) fn record_output_pins(
    ctx: &mut LowerCtx<'_>,
    te: &TypeExpr,
    prefix: &str,
    range: &SourceRange,
    depth: u32,
) -> Fields {
    if depth >= MAX_RECORD_FIELD_DEPTH {
        return HashMap::default();
    }
    let fields = ctx.record_or_tuple_fields(te).unwrap_or_default();
    let mut out = HashMap::default();
    for field in &fields {
        let port_name = format!("{prefix}_{}", field.name);
        if ctx.record_or_tuple_fields(&field.typ).is_some() {
            let inner = record_output_pins(ctx, &field.typ, &port_name, range, depth + 1);
            out.insert(intern(&field.name), Binding::Record(inner));
            continue;
        }
        let ft = ctx.resolve_local_type(&field.typ);
        let node_id = ctx.add_output(&port_name, ft.clone(), range.clone());
        out.insert(
            intern(&field.name),
            Binding::Output(NodeRecord { node_id, ty: ft }),
        );
    }
    out
}

pub(super) fn pre_declare_input(ctx: &mut LowerCtx<'_>, d: &InDecl) {
    // A record-typed input port (inline, an alias, or an instantiated generic
    // alias) dissolves into one sub-port per field, bound as a `Record` so
    // `p.field` reads the right sub-port.
    if ctx.record_or_tuple_fields(&d.typ).is_some() {
        let record_fields = record_input_pins(ctx, &d.typ, &d.name, &d.range, 0);
        ctx.scope.insert(&d.name, Binding::Record(record_fields));
        return;
    }
    // A payload-free enum crosses the boundary as one wire, its tag, so the
    // name binds the enum value whose `__disc` is the port.
    let unit_enum = unit_enum_port(ctx, &ctx.resolve_local_type(&d.typ));
    let t = if unit_enum {
        Type::Int
    } else {
        ctx.resolve_local_type(&d.typ)
    };
    let node_id = ctx.add_input(&d.name, t.clone(), d.range.clone());
    apply_port_side(ctx, node_id, d.side, d.invisible, &d.range);
    if let Some(label) = resolve_label_text(
        d.label.as_deref(),
        d.label_expr.as_ref(),
        &ctx.const_lookup(),
    ) && let Some(node) = ctx.builder.module.nodes.get_mut(&node_id)
    {
        std::sync::Arc::make_mut(&mut node.properties)
            .insert(*sym::NAME_LABEL, Literal::String(label));
    }
    let input = Binding::Input(NodeRecord { node_id, ty: t });
    if unit_enum {
        let mut value = HashMap::default();
        value.insert(*DISC, input);
        ctx.scope.insert(&d.name, Binding::Record(value));
        return;
    }
    ctx.scope.insert(&d.name, input);
}

/// Whether a port of type `t` carries a payload-free enum (a built-in game enum
/// included), which crosses a top-level port as one wire: its tag.
pub(super) fn unit_enum_port(ctx: &LowerCtx<'_>, t: &Type) -> bool {
    matches!(t, Type::Enum { .. } if ctx.enum_defs.of(t).is_some_and(|d| {
        d.variants.iter().all(|v| matches!(v.payload, crate::typecheck::enums::Payload::Unit))
    }))
}

pub(super) fn pre_declare_output(
    ctx: &mut LowerCtx<'_>,
    name: &str,
    value: Option<&Expr>,
    typ: Option<&TypeExpr>,
    side: Option<PortSide>,
    label: Option<&str>,
    label_expr: Option<&Expr>,
    invisible: bool,
    // True only for the top-level module boundary. A record output there dissolves
    // into per-field pins (below); inlined mods and chip instances instead route
    // records through `pending_out_records` at their call site and rely on a
    // single `Binding::Output` per declared output, so they pass false.
    boundary: bool,
    range: &SourceRange,
) {
    // At the top-level boundary a record-typed output dissolves into one
    // sub-port per field, bound as a `Record` so `out p: Rec = rec` wires each
    // field to its own pin (the output analogue of `pre_declare_input`).
    if boundary
        && let Some(te) = typ
        && ctx.record_or_tuple_fields(te).is_some()
    {
        let record_fields = record_output_pins(ctx, te, name, range, 0);
        ctx.scope
            .insert(&output_scope_key(name), Binding::Record(record_fields));
        return;
    }
    // An explicit annotation is the port's type and the value coerces into it
    // (`ctx.connect` inserts any adapter, e.g. string -> bool `!= ""`), so
    // `out y: bool = s` is a bool port. A `*T` port keeps its ref type; an
    // array or map ref is the container itself.
    let t = if let (Some(te), Some(_)) = (typ, value) {
        ctx.resolve_local_type(te).container_ref_as_value()
    } else if let Some(v) = value {
        // A port written values of several types is stored as their join.
        let r = v.range();
        match ctx.resolutions.untyped_port_types.get(&r.key()) {
            Some(joined) => joined.clone(),
            None => ctx.type_of(v).container_ref_as_value(),
        }
    } else if let Some(te) = typ {
        if is_exec_type(ctx, te) {
            Type::Exec
        } else {
            ctx.resolve_local_type(te)
        }
    } else {
        Type::Any
    };
    // A payload-free enum crosses the boundary as one wire, its tag: bound as
    // an enum value whose `__disc` is the pin, like a record output's fields.
    if boundary && unit_enum_port(ctx, &t) {
        let node_id = ctx.add_output(name, Type::Int, range.clone());
        apply_port_side(ctx, node_id, side, invisible, range);
        let mut fields = HashMap::default();
        fields.insert(
            *DISC,
            Binding::Output(NodeRecord {
                node_id,
                ty: Type::Int,
            }),
        );
        ctx.scope
            .insert(&output_scope_key(name), Binding::Record(fields));
        return;
    }
    let node_id = ctx.add_output(name, t.clone(), range.clone());
    apply_port_side(ctx, node_id, side, invisible, range);
    if let Some(label) = resolve_label_text(label, label_expr, &ctx.const_lookup())
        && let Some(node) = ctx.builder.module.nodes.get_mut(&node_id)
    {
        std::sync::Arc::make_mut(&mut node.properties)
            .insert(*sym::NAME_LABEL, Literal::String(label));
    }
    ctx.scope.insert(
        &output_scope_key(name),
        Binding::Output(NodeRecord { node_id, ty: t }),
    );
}

/// Register every `out` declared inside a top-level handler body as a module
/// boundary port, so `lookup_output` resolves it when the body lowers.
///
/// The site set comes from `ast::handler_port_sites`, shared with typecheck and
/// `scoped_refs` so they agree on the sites.
///
/// A name that already resolves as an output binds to that port instead of
/// minting a second one, so the caller runs this pass after the whole pass-1
/// loop. `boundary` is `pre_declare_output`'s: true for the entry file, false
/// for a namespaced module's members.
pub(super) fn pre_declare_handler_outputs(ctx: &mut LowerCtx<'_>, block: &Block, boundary: bool) {
    for o in crate::ast::handler_port_sites(block) {
        if ctx.scope.get(&output_scope_key(&o.name)).is_some() {
            continue;
        }
        ctx.with_nofold(o.no_fold, |ctx| {
            pre_declare_output(
                ctx,
                &o.name,
                o.value.as_ref(),
                o.typ.as_ref(),
                o.side,
                o.label.as_deref(),
                o.label_expr.as_ref(),
                o.invisible,
                boundary,
                &o.range,
            );
        });
    }
}

#[cfg(test)]
mod composite_config_const_env_tests {
    // `fold_mesh_colors`/`fold_ammo_override` resolve a bare identifier
    // through the const environment before their syntactic check. These tests
    // exercise that path against a hand-built `ConstEnv`.
    use super::*;

    fn ident(name: &str) -> Expr {
        Expr::Ident {
            name: name.to_string(),
            range: SourceRange::default(),
        }
    }

    #[test]
    fn mesh_colors_resolves_a_matching_identifier() {
        let colors = Literal::Array(vec![Literal::Color {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        }]);
        let mut env = ConstEnv::default();
        env.insert("MESH".to_string(), colors.clone());
        assert_eq!(fold_mesh_colors(&ident("MESH"), &env), Some(colors));
    }

    #[test]
    fn mesh_colors_rejects_a_wrong_shaped_identifier() {
        let mut env = ConstEnv::default();
        env.insert("MESH".to_string(), Literal::Array(vec![Literal::Int(1)]));
        assert_eq!(fold_mesh_colors(&ident("MESH"), &env), None);
    }

    #[test]
    fn mesh_colors_rejects_an_unbound_identifier() {
        let env = ConstEnv::default();
        assert_eq!(fold_mesh_colors(&ident("MESH"), &env), None);
    }

    #[test]
    fn ammo_override_resolves_a_matching_identifier() {
        let over = Literal::Array(vec![
            Literal::Bool(true),
            Literal::Array(vec![Literal::Array(vec![
                Literal::Int(30),
                Literal::Int(90),
            ])]),
        ]);
        let mut env = ConstEnv::default();
        env.insert("AMMO".to_string(), over.clone());
        assert_eq!(fold_ammo_override(&ident("AMMO"), &env), Some(over));
    }

    #[test]
    fn ammo_override_rejects_a_wrong_shaped_identifier() {
        let mut env = ConstEnv::default();
        env.insert(
            "AMMO".to_string(),
            Literal::Array(vec![Literal::Bool(true)]),
        );
        assert_eq!(fold_ammo_override(&ident("AMMO"), &env), None);
    }
}
