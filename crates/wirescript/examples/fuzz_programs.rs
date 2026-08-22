//! Grammar-based Wirescript program fuzzer hunting SILENT MISCOMPILES:
//! programs with no error diagnostics that still lower to `_Unsupported`
//! placeholder gates, invalid wires (dangling endpoints / fan-in duplicates),
//! stage panics, emit failures, or (multi-file mode) storage-identity bugs.
//!
//!   cargo run --release -p wirescript --example fuzz_programs -- \
//!       --count 5000 --seed 1 --out fuzz_findings
//!
//! Multi-file programs are represented as a single "bundle" string: sections
//! delimited by `//!ws-file <name>` markers (the `main` section is the entry;
//! the rest resolve as `<name>.ws` through an in-memory `MemLoader`). A share
//! of generated programs are bundles exercising `import "m"`,
//! `import * as N from "m"`, and `import { x } from "m"` with deliberate
//! name collisions (same state name in two modules, local decl vs import)
//! plus nested-scope shadowing. Bundles get an extra STORAGE oracle: the
//! declared module state (top-level `var` / `array` / map-`var` / `buffer`,
//! derived syntactically from the bundle itself) must lower to that many
//! distinct storage gates (`Pseudo_Var` / `Pseudo_ArrayVar` / `Pseudo_MapVar`
//! / buffer gates) - fewer is a `storage-collapse` finding (two names merged
//! into one gate). Each syntactic write to that state must produce a
//! Set/Increment/Push/... gate wired to a matching storage gate - a missing
//! one is a `dropped-write` finding (the write silently vanished).
//!
//! Extra modes:
//!   --calibrate <dir>   run the oracle over every .ws file in <dir> (should
//!                       be silent on known-good programs; used to tune the
//!                       wire-validity checks)
//!   --selftest-only     run the oracle plumbing sanity checks and exit
//!   --fold-diff <iters> differential fuzz mode hunting fold-pass WIRING bugs
//!                       (wrong rewire, dropped wire, folded-through-barrier):
//!                       generates constant-heavy programs, lowers each with
//!                       `FoldMode::ForceOff`/`ForceOn`, independently predicts the
//!                       unfolded module's certified-foldable outputs, and
//!                       checks the folded module actually delivers them.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap as StdMap};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use wirescript::analyze::analyze_cycles;
use wirescript::diagnostic::Severity;
use wirescript::intern::{intern, intern_static, resolve as sym_resolve, sym};
use wirescript::ir::port_registry::WirePort;
use wirescript::ir::{Literal, Module, Node, NodeId, NodeKind, PortRef, Type, Wire, gate_class as gc};
use wirescript::layout::layout;
// `fold::eval` is the `#[doc(hidden)] pub` surface opened up specifically
// for this harness (see the visibility note on `pub mod fold`
// in `src/lower/mod.rs`) — the differential predictor calls the crate's own
// certified evaluator instead of re-implementing the value laws. NOTE:
// `eval::eval` is a pure lookup against a probed truth table (a match over
// gate-class strings), NOT a dynamic/JS-style code-execution `eval` — no
// user input, code, or expression string ever reaches it.
use wirescript::lower::fold::eval::{self, Value as FoldValue};
use wirescript::lower::{FoldMode, LowerInput, lower};
use wirescript::resolve::{FsLoader, MemLoader, ResolveResult, resolve};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::{TypeCheckResult, typecheck};
use wirescript::{EmitOptions, build_world};

// ─────────────────────────── PRNG (xorshift64*) ───────────────────────────

#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    /// inclusive range
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }
    fn chance(&mut self, num: u32, den: u32) -> bool {
        (self.next() % den as u64) < num as u64
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

// ─────────────────────────── panic capture ───────────────────────────

thread_local! {
    static LAST_PANIC_LOC: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown>".into());
        LAST_PANIC_LOC.with(|c| *c.borrow_mut() = Some(loc));
    }));
}

fn panic_msg(p: Box<dyn std::any::Any + Send>) -> String {
    let msg = if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic>".into()
    };
    let loc = LAST_PANIC_LOC.with(|c| c.borrow_mut().take());
    format!("{} @ {}", norm_msg(&msg), loc.unwrap_or_default())
}

/// Normalize a message for bucketing: digits → '#', truncate.
fn norm_msg(s: &str) -> String {
    let mut out = String::new();
    let mut last_hash = false;
    for c in s.chars().take(220) {
        if c.is_ascii_digit() {
            if !last_hash {
                out.push('#');
                last_hash = true;
            }
        } else {
            out.push(c);
            last_hash = false;
        }
    }
    out
}

// ─────────────────────────── pipeline + oracle ───────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Crash,
    Unsupported,
    WireDangling,
    WireFanIn,
    WireDup,
    EmitErr,
    /// Fewer distinct storage gates than syntactically-distinct declared
    /// state (two same-named module states merged into ONE gate).
    StorageCollapse,
    /// A syntactic write to declared state produced no Set/mutation gate
    /// wired to any matching storage gate - the write silently vanished.
    DroppedWrite,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Crash => "crash",
            Kind::Unsupported => "unsupported-gate",
            Kind::WireDangling => "wire-dangling",
            Kind::WireFanIn => "wire-fanin",
            Kind::WireDup => "wire-duplicate",
            Kind::EmitErr => "emit-error",
            Kind::StorageCollapse => "storage-collapse",
            Kind::DroppedWrite => "dropped-write",
        }
    }
}

#[derive(Default)]
struct Outcome {
    error_diags: Vec<String>,
    warn_diags: Vec<String>,
    panic: Option<(String, String)>, // (stage, normalized msg @ loc)
    /// normalized-snippet keys of _Unsupported nodes (deduped) + raw snippet
    unsupported: Vec<(String, String)>,
    wire_issues: Vec<(Kind, String)>,
    /// verbose per-issue details (src classes, counts) for metadata
    wire_detail: Vec<String>,
    /// storage-oracle findings (StorageCollapse / DroppedWrite) + details
    storage_issues: Vec<(Kind, String)>,
    storage_detail: Vec<String>,
    emit_err: Option<String>,
    total_nodes: usize,
}

impl Outcome {
    fn has_errors(&self) -> bool {
        !self.error_diags.is_empty()
    }
    /// All oracle findings (kind, bucket-key) for this outcome.
    fn findings(&self) -> Vec<(Kind, String)> {
        let mut v: Vec<(Kind, String)> = Vec::new();
        if let Some((stage, key)) = &self.panic {
            v.push((Kind::Crash, format!("{stage}: {key}")));
        }
        if !self.has_errors() {
            for (norm, _raw) in &self.unsupported {
                v.push((Kind::Unsupported, coarse_shape(norm)));
            }
            for (k, key) in &self.wire_issues {
                v.push((*k, key.clone()));
            }
            for (k, key) in &self.storage_issues {
                v.push((*k, key.clone()));
            }
            if let Some(e) = &self.emit_err {
                v.push((Kind::EmitErr, norm_msg(e)));
            }
        }
        v.sort();
        v.dedup();
        v
    }
}

fn walk_modules<'a>(m: &'a Module, f: &mut impl FnMut(&'a Module)) {
    f(m);
    for c in m.chips.values() {
        walk_modules(c, f);
    }
}

fn short_class(c: &str) -> &str {
    let c = c
        .trim_start_matches("BrickComponentType_")
        .trim_start_matches("Component_");
    c.trim_start_matches("WireGraph_")
        .trim_start_matches("WireGraphPseudo_")
}

/// IR-level wire validity: dangling endpoints, duplicate wires, fan-in
/// (2+ wires driving the same (target node, target input port) tuple).
fn check_wires(root: &Module, out: &mut Outcome) {
    let mut classes: StdMap<NodeId, &'static str> = StdMap::new();
    walk_modules(root, &mut |m| {
        for (id, n) in &m.nodes {
            classes.insert(*id, n.gate_class);
        }
    });
    let mut all_wires: Vec<&wirescript::ir::Wire> = Vec::new();
    walk_modules(root, &mut |m| {
        for w in &m.wires {
            all_wires.push(w);
        }
    });

    let skip_ep = |id: NodeId| -> bool {
        matches!(classes.get(&id), Some(c) if *c == gc::LITERAL || *c == gc::UNSUPPORTED)
    };

    let mut seen_exact: BTreeSet<(u32, &'static str, u32, &'static str)> = BTreeSet::new();
    let mut by_target: StdMap<(NodeId, &'static str), Vec<NodeId>> = StdMap::new();

    for w in &all_wires {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        let src_known = classes.contains_key(&w.source.node_id);
        let tgt_known = classes.contains_key(&w.target.node_id);
        if !src_known || !tgt_known {
            // Bucket by which end is missing, whether the missing id is the
            // null NodeId(0) (a never-filled placeholder) or a real id that
            // vanished (orphan), and the missing side's port. The OTHER
            // endpoint's class goes in the detail only — it fragments one
            // root cause into dozens of buckets otherwise.
            let (kind_end, missing_id, missing_port, other_cls, other_port) = if !src_known {
                (
                    "src",
                    w.source.node_id,
                    w.source.port.as_str(),
                    classes
                        .get(&w.target.node_id)
                        .map(|c| short_class(c))
                        .unwrap_or("?"),
                    w.target.port.as_str(),
                )
            } else {
                (
                    "tgt",
                    w.target.node_id,
                    w.target.port.as_str(),
                    classes
                        .get(&w.source.node_id)
                        .map(|c| short_class(c))
                        .unwrap_or("?"),
                    w.source.port.as_str(),
                )
            };
            let missing_kind = if missing_id.0 == 0 { "null-node" } else { "orphan" };
            let key =
                format!("dangling-{kind_end} missing={missing_kind} port={missing_port}");
            out.wire_detail.push(format!(
                "{key} id={missing_id} other={other_cls}.{other_port}"
            ));
            out.wire_issues.push((Kind::WireDangling, key));
            continue;
        }
        if skip_ep(w.source.node_id) || skip_ep(w.target.node_id) {
            continue;
        }
        // exact duplicate wire
        let key = (
            w.source.node_id.0,
            w.source.port.as_str(),
            w.target.node_id.0,
            w.target.port.as_str(),
        );
        if !seen_exact.insert(key) {
            let sc = short_class(classes[&w.source.node_id]);
            let tc = short_class(classes[&w.target.node_id]);
            out.wire_detail.push(format!(
                "dup {}.{} -> {}.{}",
                sc,
                w.source.port.as_str(),
                tc,
                w.target.port.as_str()
            ));
            out.wire_issues.push((
                Kind::WireDup,
                format!("dup -> {}.{}", tc, w.target.port.as_str()),
            ));
            continue;
        }
        by_target
            .entry((w.target.node_id, w.target.port.as_str()))
            .or_default()
            .push(w.source.node_id);
    }

    for ((tgt, port), srcs) in &by_target {
        if srcs.len() >= 2 {
            let tc = short_class(classes[tgt]);
            let mut src_cls: Vec<&str> =
                srcs.iter().map(|s| short_class(classes[s])).collect();
            src_cls.sort();
            src_cls.dedup();
            // Coarse bucket key: target class + port only. Source-class
            // combos fragment one root cause into dozens of buckets — the
            // detail keeps them for metadata.
            out.wire_detail.push(format!(
                "fanin {}.{} <- [{}] x{}",
                tc,
                port,
                src_cls.join(","),
                srcs.len()
            ));
            out.wire_issues
                .push((Kind::WireFanIn, format!("fanin {}.{}", tc, port)));
        }
    }
}

// --------------------------- multi-file bundles + storage oracle ---------------------------

/// Section delimiter for multi-file programs packed into one string (so the
/// existing single-string plumbing - minimizer, bucket exemplars, findings
/// files - carries multi-file programs unchanged). The `main` section is the
/// entry; every other section resolves as `<name>.ws` via `MemLoader`.
const FILE_MARKER: &str = "//!ws-file ";

struct Bundle {
    entry: String,
    files: Vec<(String, String)>, // (module name sans .ws, source)
}

fn split_bundle(src: &str) -> Bundle {
    if !src.contains(FILE_MARKER) {
        return Bundle { entry: src.to_string(), files: Vec::new() };
    }
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut pre: Vec<&str> = Vec::new();
    for line in src.lines() {
        if let Some(rest) = line.trim_end().strip_prefix(FILE_MARKER) {
            sections.push((rest.trim().to_string(), Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        } else {
            pre.push(line);
        }
    }
    let mut entry: Option<String> = None;
    let mut files = Vec::new();
    for (name, body) in sections {
        let text = body.join("\n");
        if name == "main" && entry.is_none() {
            entry = Some(text);
        } else {
            files.push((name, text));
        }
    }
    Bundle { entry: entry.unwrap_or_else(|| pre.join("\n")), files }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum StoreKind {
    Var,
    Array,
    Map,
    Buffer,
}

impl StoreKind {
    fn name(self) -> &'static str {
        match self {
            StoreKind::Var => "var",
            StoreKind::Array => "array",
            StoreKind::Map => "map",
            StoreKind::Buffer => "buffer",
        }
    }
}

/// What the bundle's SOURCE says must exist in the lowered IR. Derived
/// syntactically from the bundle text itself (not carried from the
/// generator), so line-based minimization stays truthful: deleting a
/// declaration or write line lowers the expectation with it.
#[derive(Default)]
struct StorageExpect {
    /// (kind, name) -> how many distinct storage gates that name must have
    /// (one per distinct declaring file that the entry actually reaches).
    counts: BTreeMap<(StoreKind, String), usize>,
    /// Buffer gates carry no name label in the IR, so buffers are checked as
    /// an aggregate count instead.
    buffer_total: usize,
    /// (kind, name) pairs the source syntactically writes to.
    writes: BTreeSet<(StoreKind, String)>,
}

fn ident_at(s: &str) -> &str {
    let end = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

/// Column-0 `type Name = { f1: T1, f2: T2, .. }` declarations of one file, by
/// type name -> ordered field names. Record types are file-private (an
/// importer never spells the type name, only field/method access on the
/// container it declares), so this is scoped per-file exactly like
/// `scan_state_decls` below - and is in fact only ever consulted against the
/// SAME file's own `var` lines.
fn scan_record_types(src: &str) -> StdMap<String, Vec<String>> {
    let mut out = StdMap::new();
    for line in src.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(rest) = line.strip_prefix("type ") else { continue };
        let name = ident_at(rest.trim_start());
        if name.is_empty() {
            continue;
        }
        let Some(open) = rest.find('{') else { continue };
        let Some(close) = rest.rfind('}') else { continue };
        if close <= open {
            continue;
        }
        let fields: Vec<String> = rest[open + 1..close]
            .split(',')
            .filter_map(|part| {
                let fname = ident_at(part.trim());
                if fname.is_empty() { None } else { Some(fname.to_string()) }
            })
            .collect();
        out.insert(name.to_string(), fields);
    }
    out
}

/// Classify a `var name<ANN>` annotation (`ANN` is everything after the name,
/// up to a `=` initializer) as a record CONTAINER shape - `: Rec`, `: Rec[]`,
/// `: Map<K, Rec>` - returning the storage kind the container's OWN form
/// implies (not the field's) plus the bare type name. Doesn't check the type
/// name is actually a known record - that's the caller's job, since a bare
/// `: int` annotation is syntactically identical to `: SomeRecordAlias` here.
fn record_container_kind(ann: &str) -> Option<(StoreKind, String)> {
    let ann = ann.trim().strip_prefix(':')?.trim();
    if let Some(inner) = ann.strip_suffix("[]") {
        let inner = inner.trim();
        return (!inner.is_empty()).then(|| (StoreKind::Array, inner.to_string()));
    }
    if let Some(rest) = ann.strip_prefix("Map<") {
        let close = rest.find('>')?;
        let value_ty = rest[..close].split(',').nth(1)?.trim();
        return (!value_ty.is_empty()).then(|| (StoreKind::Map, value_ty.to_string()));
    }
    // A bare type name. Builtin scalar types are all lowercase
    // (int/float/bool/string/vector/...), so an uppercase leading letter is
    // the signal this MIGHT be a `type Name = {..}` alias - the caller
    // confirms against the file's own record-type table.
    let starts_upper = ann.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    (!ann.is_empty() && starts_upper).then(|| (StoreKind::Var, ann.to_string()))
}

/// Top-level (column-0) state declarations of one file. Anything indented -
/// chip/mod/handler bodies - is deliberately ignored: nested `var`s create
/// storage gates too, but under names this derivation never claims.
///
/// A `var` whose annotation names a record type declared in the SAME file
/// (`type Rec = { a: .., b: .. }`) decomposes into one entry PER FIELD,
/// labeled `"{name}.{field}"` - exactly the label
/// `record_field_storage`/`declare_record_container` bakes onto each
/// per-field backing gate, so the derivation and the compiler agree on
/// identity without either side telling the other.
fn scan_state_decls(src: &str) -> Vec<(StoreKind, String)> {
    let records = scan_record_types(src);
    let mut out = Vec::new();
    for line in src.lines() {
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let l = line.strip_prefix("static ").unwrap_or(line);
        if let Some(rest) = l.strip_prefix("var ") {
            let rest = rest.trim_start();
            let name = ident_at(rest);
            if name.is_empty() {
                continue;
            }
            // the annotation decides the storage class: `var x: Map<..>` is a
            // MapVar, `var x: T[]` an ArrayVar, anything else a plain Var
            let ann = rest[name.len()..].split('=').next().unwrap_or("");
            if let Some((kind, type_name)) = record_container_kind(ann)
                && let Some(fields) = records.get(&type_name)
            {
                for f in fields {
                    out.push((kind, format!("{name}.{f}")));
                }
                continue;
            }
            let kind = if ann.contains("Map<") {
                StoreKind::Map
            } else if ann.contains("[]") {
                StoreKind::Array
            } else {
                StoreKind::Var
            };
            out.push((kind, name.to_string()));
        } else if let Some(rest) = l.strip_prefix("array ") {
            let name = ident_at(rest.trim_start());
            if !name.is_empty() {
                out.push((StoreKind::Array, name.to_string()));
            }
        } else if let Some(rest) = l.strip_prefix("buffer ") {
            let name = ident_at(rest.trim_start());
            if !name.is_empty() && name != "emit" {
                out.push((StoreKind::Buffer, name.to_string()));
            }
        }
    }
    out
}

/// The entry's import lines: (module name, Some(named list) | None = all).
/// `import * as N` and `import "m"` both reach the module's whole state;
/// `import { a, b as c }` reaches only the named state (bound as the alias).
fn scan_imports(entry: &str) -> Vec<(String, Option<Vec<String>>)> {
    let mut out = Vec::new();
    for line in entry.lines() {
        let l = line.trim_end();
        if !l.starts_with("import") {
            continue;
        }
        let rest = l["import".len()..].trim_start();
        let module_of = |s: &str| -> Option<String> {
            let q = s.find('"')?;
            let rest = &s[q + 1..];
            let e = rest.find('"')?;
            Some(rest[..e].trim_end_matches(".ws").to_string())
        };
        if let Some(body) = rest.strip_prefix('{') {
            let Some(close) = body.find('}') else { continue };
            let names: Vec<String> = body[..close]
                .split(',')
                .filter_map(|part| {
                    let part = part.trim();
                    if part.is_empty() {
                        return None;
                    }
                    // `orig as alias` binds (and labels) the alias, but the
                    // DECL matched in the module is `orig`; the bound name is
                    // what the gate label carries. Track the bound name and
                    // match it against the module's decls by the original.
                    let mut it = part.split_whitespace();
                    let orig = it.next()?.to_string();
                    match (it.next(), it.next()) {
                        (Some("as"), Some(alias)) => Some(format!("{orig}\u{1}{alias}")),
                        _ => Some(orig),
                    }
                })
                .collect();
            if let Some(m) = module_of(&body[close..]) {
                out.push((m, Some(names)));
            }
        } else if let Some(m) = module_of(rest) {
            // covers both `import "m"` and `import * as N from "m"`
            out.push((m, None));
        }
    }
    out
}

/// Write statements: indented lines of the strict shapes the generators emit
/// (`P = e` / `P += e` / `P[i] = e` / `P.push(..)` and friends / `P.set(..)`
/// / `refw(P, ..)`), where `P` is `name` or `Ns.name`. Only names that the
/// derivation already expects as storage count - mod params, chip-internal
/// vars, and locals never match.
fn scan_writes(
    src: &str,
    counts: &BTreeMap<(StoreKind, String), usize>,
    writes: &mut BTreeSet<(StoreKind, String)>,
) {
    const ARRAY_METHODS: [&str; 8] = [
        "push", "clear", "insert", "sort", "reverse", "shuffle", "fill", "resize",
    ];
    // `add(kind, name)` covers the ordinary case (`name` is itself a
    // declared storage name) AND the record-container case: a WHOLE-container
    // write (`rv = {..}`, `arr.push(..)`, `arr[i] = {..}`, `m.set(..)`) fans
    // out to every per-field backing gate the container decomposed into
    // (`"{name}.{field}"` keys in `counts`), so a plain exact-match miss
    // falls back to a prefix scan before giving up. Non-record bundles never
    // have any `"name.field"` keys, so the fallback is a no-op there.
    let mut add = |kind: StoreKind, name: &str| {
        if counts.contains_key(&(kind, name.to_string())) {
            writes.insert((kind, name.to_string()));
            return;
        }
        let prefix = format!("{name}.");
        for (k, n) in counts.keys() {
            if *k == kind && n.starts_with(&prefix) {
                writes.insert((*k, n.clone()));
            }
        }
    };
    // A write inside a `mod` / named-`chip` DECLARATION body only lowers when
    // the mod/chip is actually called - an uncalled body legitimately emits
    // nothing, so its lines must not become write expectations. (Handler
    // bodies, `chip on`, and anonymous `chip {` blocks always lower and DO
    // count.) Tracked as: a column-0 `mod ` / `chip Name` header excludes the
    // indented lines that follow, until the next column-0 line.
    let mut in_decl_body = false;
    for line in src.lines() {
        if !line.starts_with(char::is_whitespace) {
            // skip leading annotations (`@closed `, `@label("...") `, sides)
            let mut l = line;
            loop {
                if let Some(rest) = l.strip_prefix("@label(") {
                    if let Some(p) = rest.find(')') {
                        l = rest[p + 1..].trim_start();
                        continue;
                    }
                } else if l.starts_with('@') {
                    if let Some((_, rest)) = l.split_once(' ') {
                        l = rest.trim_start();
                        continue;
                    }
                }
                break;
            }
            in_decl_body = l.starts_with("mod ")
                || (l.starts_with("chip ")
                    && !l.starts_with("chip on")
                    && !l.starts_with("chip let")
                    && !l.starts_with("chip {"));
            continue;
        }
        if in_decl_body {
            continue;
        }
        let t = line.trim_start();
        // ref-param write helper emitted by the canary generator: the mod
        // writes its first (ref) argument.
        if let Some(rest) = t.strip_prefix("refw(") {
            let arg = rest.split([',', ')']).next().unwrap_or("");
            let base = arg.trim().rsplit('.').next().unwrap_or("");
            if !base.is_empty() {
                add(StoreKind::Var, base);
            }
            continue;
        }
        // Leading dotted path.
        let mut end = 0;
        let bytes: Vec<char> = t.chars().collect();
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == '_' || bytes[end] == '.') {
            end += 1;
        }
        if end == 0 {
            continue;
        }
        let path: String = bytes[..end].iter().collect();
        let after = t[path.len()..].trim_start();
        let segs: Vec<&str> = path.split('.').collect();
        if segs.iter().any(|s| s.is_empty()) {
            continue;
        }
        let first = segs[0];
        if matches!(
            first,
            "let" | "var" | "static" | "buffer" | "array" | "out" | "in" | "emit" | "chip"
                | "mod" | "on" | "if" | "else" | "await" | "return" | "type" | "import"
        ) {
            continue;
        }
        let last = *segs.last().unwrap();
        if after.starts_with('(') && segs.len() >= 2 {
            // method call: base is the segment before the method name
            let base = segs[segs.len() - 2];
            if ARRAY_METHODS.contains(&last) {
                add(StoreKind::Array, base);
            } else if last == "set" || last == "remove" {
                add(StoreKind::Map, base);
            }
        } else if after.starts_with('[') {
            // `P[i] = e` indexed array write (only when an assignment follows)
            if let Some(close) = after.find(']') {
                let tail = after[close + 1..].trim_start();
                if tail.starts_with('=') && !tail.starts_with("==") {
                    add(StoreKind::Array, last);
                }
            }
        } else if (after.starts_with('=') && !after.starts_with("=="))
            || after.starts_with("+=")
            || after.starts_with("-=")
            || after.starts_with("*=")
        {
            // A record VAR field write (`rv.a = e`, or namespaced `N.rv.a =
            // e`) needs the exact `"container.field"` key, not just `add`'s
            // container-prefix fan-out (which is for a WHOLE-record write
            // like `rv = {..}` and would be wrong here - it would mark every
            // sibling field written too). Try the last two path segments
            // first (this also transparently drops a leading namespace
            // segment, since only the trailing pair can ever match a
            // declared field key); fall back to the plain/whole-container
            // path otherwise.
            let field_key = (segs.len() >= 2)
                .then(|| format!("{}.{}", segs[segs.len() - 2], segs[segs.len() - 1]))
                .filter(|k| counts.contains_key(&(StoreKind::Var, k.clone())));
            match field_key {
                Some(k) => add(StoreKind::Var, &k),
                None => add(StoreKind::Var, last),
            }
        }
    }
}

fn derive_storage_expect(bundle: &Bundle) -> StorageExpect {
    let mut expect = StorageExpect::default();
    let by_file: StdMap<&str, Vec<(StoreKind, String)>> = bundle
        .files
        .iter()
        .map(|(name, src)| (name.as_str(), scan_state_decls(src)))
        .collect();

    // Reachable distinct state: the entry's own decls + each imported file's
    // (deduped per (file, kind, name) - importing one file twice is still ONE
    // storage per state, which is the language's intended sharing).
    let mut reachable: BTreeSet<(String, StoreKind, String)> = BTreeSet::new();
    for (kind, name) in scan_state_decls(&bundle.entry) {
        reachable.insert(("".into(), kind, name));
    }
    let mut reached_files: BTreeSet<String> = BTreeSet::new();
    for (module, named) in scan_imports(&bundle.entry) {
        let Some(decls) = by_file.get(module.as_str()) else { continue };
        reached_files.insert(module.clone());
        match &named {
            None => {
                for (kind, name) in decls {
                    reachable.insert((module.clone(), *kind, name.clone()));
                }
            }
            Some(names) => {
                for entry in names {
                    let (orig, bound) = match entry.split_once('\u{1}') {
                        Some((o, a)) => (o, a),
                        None => (entry.as_str(), entry.as_str()),
                    };
                    for (kind, name) in decls {
                        if name == orig {
                            reachable.insert((module.clone(), *kind, bound.to_string()));
                        }
                    }
                }
            }
        }
    }
    for (_, kind, name) in &reachable {
        if *kind == StoreKind::Buffer {
            expect.buffer_total += 1;
        }
        *expect.counts.entry((*kind, name.clone())).or_insert(0) += 1;
    }
    // Writes: entry + reached files only (an unimported section never lowers,
    // so its writes must not be expected).
    scan_writes(&bundle.entry, &expect.counts, &mut expect.writes);
    for (name, src) in &bundle.files {
        if reached_files.contains(name) {
            scan_writes(src, &expect.counts, &mut expect.writes);
        }
    }
    expect
}

/// Storage-gate mutation classes per storage kind - a write must wire the
/// storage's ref port into one of these.
fn is_writer_class(kind: StoreKind, class: &str) -> bool {
    match kind {
        StoreKind::Var => class == gc::VAR_SET || class == gc::VAR_INCREMENT,
        StoreKind::Array => [
            gc::ARRAY_SET_AT_INDEX,
            gc::ARRAY_PUSH,
            gc::ARRAY_POP,
            gc::ARRAY_CLEAR,
            gc::ARRAY_INSERT,
            gc::ARRAY_REMOVE_AT_INDEX,
            gc::ARRAY_SORT,
            gc::ARRAY_SORT_MULTIPLE,
            gc::ARRAY_REVERSE,
            gc::ARRAY_SHUFFLE,
            gc::ARRAY_FILL,
            gc::ARRAY_RESIZE,
            gc::ARRAY_SWAP,
            gc::ARRAY_APPEND,
            gc::ARRAY_COPY_FROM,
            gc::ARRAY_SLICE,
        ]
        .contains(&class),
        StoreKind::Map => {
            [gc::MAP_SET, gc::MAP_REMOVE, gc::MAP_CLEAR, gc::MAP_COPY_FROM].contains(&class)
        }
        StoreKind::Buffer => false,
    }
}

/// The oracle: every syntactically-distinct declared state must have its own
/// storage gate, and every syntactic write must have a mutation gate wired to
/// a matching storage gate. Declared storage gates are identified by class +
/// the `_label` name property (synthetic pseudo-vars - out-backing, signal
/// payload stores, await armed flags, ret_val - all carry a `note` and no
/// label, so they never match).
fn check_storage(root: &Module, expect: &StorageExpect, out: &mut Outcome) {
    if expect.counts.is_empty() {
        return;
    }
    let mut nodes: StdMap<NodeId, &Node> = StdMap::new();
    collect_all_nodes(root, &mut nodes);
    let mut wires: Vec<Wire> = Vec::new();
    collect_all_wires(root, &mut wires);

    let name_label = *sym::NAME_LABEL;
    let mut labeled: BTreeMap<(StoreKind, String), Vec<NodeId>> = BTreeMap::new();
    let mut buffer_actual = 0usize;
    for (id, n) in &nodes {
        if n.note.is_some() {
            continue;
        }
        let kind = match n.gate_class {
            c if c == gc::PSEUDO_VAR => StoreKind::Var,
            c if c == gc::PSEUDO_ARRAY_VAR => StoreKind::Array,
            c if c == gc::PSEUDO_MAP_VAR => StoreKind::Map,
            c if c == gc::BUFFER_TICKS || c == gc::BUFFER_SECONDS => {
                buffer_actual += 1;
                continue;
            }
            _ => continue,
        };
        if let Some(Literal::String(label)) = n.properties.get(&name_label) {
            labeled.entry((kind, label.clone())).or_default().push(*id);
        }
    }

    for ((kind, name), exp) in &expect.counts {
        if *kind == StoreKind::Buffer {
            continue; // aggregate check below
        }
        let actual = labeled.get(&(*kind, name.clone())).map_or(0, |v| v.len());
        if actual < *exp {
            out.storage_detail.push(format!(
                "collapse {} `{name}`: {exp} declared -> {actual} storage gate(s)",
                kind.name()
            ));
            out.storage_issues
                .push((Kind::StorageCollapse, format!("storage-collapse {}", kind.name())));
        }
    }
    if buffer_actual < expect.buffer_total {
        out.storage_detail.push(format!(
            "collapse buffer: {} declared -> {buffer_actual} buffer gate(s)",
            expect.buffer_total
        ));
        out.storage_issues
            .push((Kind::StorageCollapse, "storage-collapse buffer".to_string()));
    }

    for (kind, name) in &expect.writes {
        let Some(ids) = labeled.get(&(*kind, name.clone())) else {
            continue; // storage entirely missing - the collapse check owns it
        };
        let ref_port = match kind {
            StoreKind::Var => WirePort::VarRef,
            StoreKind::Array => WirePort::ArrayVarRef,
            StoreKind::Map => WirePort::MapVarRef,
            StoreKind::Buffer => continue,
        };
        // Follow the ref wire from the storage gate to a mutation gate,
        // chasing through chip-boundary pins and rerouters (a write inside a
        // `chip on` / anon chip body receives the VarRef via a
        // MicrochipInput boundary pin, not a direct wire).
        let mut frontier: Vec<NodeId> = wires
            .iter()
            .filter(|w| w.source.port == ref_port && ids.contains(&w.source.node_id))
            .map(|w| w.target.node_id)
            .collect();
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut written = false;
        while let Some(id) = frontier.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(n) = nodes.get(&id) else { continue };
            if is_writer_class(*kind, n.gate_class) {
                written = true;
                break;
            }
            let pass_through = n.gate_class == MICROCHIP_INPUT
                || n.gate_class == MICROCHIP_OUTPUT
                || n.gate_class == gc::REROUTER;
            if pass_through {
                frontier.extend(
                    wires
                        .iter()
                        .filter(|w| w.source.node_id == id)
                        .map(|w| w.target.node_id),
                );
            }
        }
        if !written {
            out.storage_detail.push(format!(
                "dropped write: {} `{name}` is written in source but no mutation gate targets its storage",
                kind.name()
            ));
            out.storage_issues
                .push((Kind::DroppedWrite, format!("dropped-write {}", kind.name())));
        }
    }
}

fn run_pipeline(src: &str) -> Outcome {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(128 * 1024 * 1024)
            .spawn_scoped(s, || run_pipeline_inner(src))
            .expect("spawn worker")
            .join()
            .unwrap_or_else(|_| {
                let mut o = Outcome::default();
                o.panic = Some(("harness".into(), "worker thread panicked".into()));
                o
            })
    })
}

fn run_pipeline_inner(src: &str) -> Outcome {
    let mut out = Outcome::default();
    let bundle = split_bundle(src);
    let is_bundle = !bundle.files.is_empty();
    let file = if is_bundle { "main" } else { "fuzz.ws" };

    let mut record_diags = |out: &mut Outcome, diags: &[wirescript::diagnostic::Diagnostic]| {
        for d in diags {
            let line = format!("[{}] {}", d.code, d.message);
            match d.severity {
                Severity::Error => out.error_diags.push(line),
                _ => out.warn_diags.push(line),
            }
        }
    };

    // resolve - single-file programs keep the FsLoader path; bundles resolve
    // their sections in-memory (the same MemLoader pattern the crate's own
    // `compile_multi` tests use).
    let resolved = match catch_unwind(AssertUnwindSafe(|| {
        if is_bundle {
            let mut loader = MemLoader { files: Default::default() };
            for (name, text) in &bundle.files {
                loader.files.insert(format!("{name}.ws"), text.clone());
            }
            resolve(&bundle.entry, file, &loader)
        } else {
            resolve(&bundle.entry, file, &FsLoader)
        }
    })) {
        Ok(r) => r,
        Err(p) => {
            out.panic = Some(("resolve".into(), panic_msg(p)));
            return out;
        }
    };
    record_diags(&mut out, &resolved.diagnostics);

    // typecheck
    let tc = match catch_unwind(AssertUnwindSafe(|| typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default()))) {
        Ok(r) => r,
        Err(p) => {
            out.panic = Some(("typecheck".into(), panic_msg(p)));
            return out;
        }
    };
    record_diags(&mut out, &tc.diagnostics);

    // lower
    let cache = Arc::new(TemplateCache::new());
    let cache2 = cache.clone();
    let lowered = match catch_unwind(AssertUnwindSafe(|| {
        lower(LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file,
            module_name: None,
            template_cache: cache2,
            doc_comments: &resolved.doc_comments,
            // Unfolded on purpose: the general fuzz mode hunts crashes in
            // resolve/typecheck/lower itself; `--fold-diff` below is the
            // dedicated differential mode that exercises fold on/off
            // agreement.
            fold_mode: FoldMode::ForceOff,
            ce_slots: &wirescript::typecheck::CeSlotMap::default(),
        })
    })) {
        Ok(r) => r,
        Err(p) => {
            out.panic = Some(("lower".into(), panic_msg(p)));
            return out;
        }
    };
    record_diags(&mut out, &lowered.diagnostics);

    // analyze
    match catch_unwind(AssertUnwindSafe(|| analyze_cycles(&lowered.module))) {
        Ok(c) => record_diags(&mut out, &c.diagnostics),
        Err(p) => {
            out.panic = Some(("analyze".into(), panic_msg(p)));
            return out;
        }
    }

    // scan _Unsupported nodes
    walk_modules(&lowered.module, &mut |m| {
        out.total_nodes += m.nodes.len();
        for n in m.nodes.values() {
            if n.gate_class == gc::UNSUPPORTED {
                let (s, e) = (n.source_range.start.offset, n.source_range.end.offset);
                let raw = src.get(s..e).unwrap_or("<range?>").to_string();
                out.unsupported.push((norm_snippet(&raw), raw));
            }
        }
    });
    out.unsupported.sort();
    out.unsupported.dedup();

    // wire validity
    check_wires(&lowered.module, &mut out);

    // storage identity (bundles only): declared module state vs the storage
    // gates + mutation gates that actually lowered.
    if is_bundle {
        let expect = derive_storage_expect(&bundle);
        check_storage(&lowered.module, &expect, &mut out);
    }

    // layout + build_world only when clean (mirrors compile.rs gating)
    if !out.has_errors() {
        let lr = match catch_unwind(AssertUnwindSafe(|| layout(&lowered.module))) {
            Ok(l) => l,
            Err(p) => {
                out.panic = Some(("layout".into(), panic_msg(p)));
                return out;
            }
        };
        let opts = EmitOptions::default();
        match catch_unwind(AssertUnwindSafe(|| {
            build_world(&lowered.module, &lr, &opts, &cache)
        })) {
            Ok(Ok(_world)) => {}
            Ok(Err(e)) => out.emit_err = Some(format!("{e:?}")),
            Err(p) => {
                out.panic = Some(("build_world".into(), panic_msg(p)));
                return out;
            }
        }
    }

    out
}

// ─────────────────────────── snippet normalization ───────────────────────────

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "let", "var", "on", "out", "in", "emit", "await", "chip", "mod",
    "return", "buffer", "static", "type", "true", "false", "exec", "ref", "open", "int", "float",
    "bool", "string", "vector", "entity", "controller", "character", "array", "import", "from",
    "as", "event",
];
const BUILTINS: &[&str] = &[
    "abs", "min", "max", "clamp", "sqrt", "sin", "cos", "atan", "atan2", "pow", "floor", "ceil",
    "round", "exp", "ln", "sign", "Vec", "Dot", "Random", "Sleep", "SleepTicks", "Value", "prev",
    "push", "pop", "length", "clear", "insert", "remove", "sort", "reverse", "shuffle", "sum",
    "find", "Found", "Index", "DisplayText", "SetLocation", "Fmt", "Deg2Rad", "Rad2Deg", "value",
    "bOutOfBounds",
];

/// Coarse bucket key for an _Unsupported snippet: collapse call arguments and
/// index expressions so `a.push(x + 1)` and `b.push(3)` share a bucket. The
/// full normalized snippet stays in the finding detail.
fn coarse_shape(norm: &str) -> String {
    let mut s = norm.to_string();
    if let Some(p) = s.find('(') {
        s.truncate(p);
        s.push_str("(..)");
    } else if let Some(p) = s.find('[') {
        s.truncate(p);
        s.push_str("[..]");
    }
    s
}

/// Structure-preserving normalization: user identifiers → `I`, numbers → `N`,
/// strings → `S`; keywords/builtins/punctuation kept. Groups findings that
/// differ only in names/values.
fn norm_snippet(s: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() && out.len() < 160 {
        let c = cs[i];
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            let word: String = cs[start..i].iter().collect();
            if KEYWORDS.contains(&word.as_str()) || BUILTINS.contains(&word.as_str()) {
                out.push_str(&word);
            } else {
                out.push('I');
            }
        } else if c.is_ascii_digit() {
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            out.push('N');
        } else if c == '"' || c == '\'' {
            let quote = c;
            i += 1;
            while i < cs.len() && cs[i] != quote {
                if cs[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            out.push('S');
        } else if c.is_whitespace() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out.trim().to_string()
}

// ─────────────────────────── program generator ───────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Vec3,
}
const SCALAR_TYS: [Ty; 5] = [Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::Vec3];

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::Bool => "bool",
            Ty::Str => "string",
            Ty::Vec3 => "vector",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Obj {
    Entity,
    Character,
    Controller,
}
impl Obj {
    fn name(self) -> &'static str {
        match self {
            Obj::Entity => "entity",
            Obj::Character => "character",
            Obj::Controller => "controller",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Acc {
    Direct, // let / in / buffer / param — read by name in pure context
    Var,    // var — `.Value` in pure context, bare name in exec context
}

#[derive(Clone)]
struct Sca {
    name: String,
    ty: Ty,
    acc: Acc,
}

#[derive(Clone)]
struct RecordDef {
    name: String,
    fields: Vec<(String, Ty)>,
}

#[derive(Clone)]
enum PTy {
    Val(Ty),
    Rec(usize),
    RecDestr(usize),
    Exec,
    Ref(Ty),
}

#[derive(Clone)]
enum RetTy {
    Val(Ty),
    Rec(usize),
    Multi(Vec<(String, Ty)>),
}

#[derive(Clone)]
struct ModSig {
    name: String,
    params: Vec<(String, PTy)>,
    ret: Option<RetTy>,
    pure_call: bool,
    // true for the generic-identity-mod entries registered by gen_mod's form-6
    // arm — lets call sites opt into emitting explicit `<Type>` type arguments.
    generic: bool,
}

#[derive(Clone)]
struct ChipSig {
    name: String,
    params: Vec<(String, PTy)>,
    outs: Vec<(String, Option<Ty>)>, // None = exec out
    pure_expr_callable: bool,        // single value out, no exec/ref params
}

#[derive(Clone, Default)]
struct Scope {
    scalars: Vec<Sca>,
    rec_binds: Vec<(String, usize)>,
    arrays: Vec<(String, Ty, bool)>, // (name, elem, writable)
    objs: Vec<(String, Obj)>,
    exec: bool,
    mod_ret: Option<Ty>,
}

struct Gen {
    rng: Rng,
    blocks: Vec<String>,
    // global (top-level) symbol tables
    execs: Vec<String>,
    signals: Vec<String>,
    scalars: Vec<Sca>,
    arrays: Vec<(String, Ty, bool)>,
    records: Vec<RecordDef>,
    rec_binds: Vec<(String, usize)>,
    objs: Vec<(String, Obj)>,
    chips: Vec<ChipSig>,
    mods: Vec<ModSig>,
    out_execs: Vec<String>,
    typed_val_outs: Vec<(String, Ty)>,
    bool_lets: Vec<String>,
    // `import * as N` symbols (`N.name` paths). Only reachable from EXEC
    // scopes: a namespaced var read in a PURE context (`N.g.Value`) lowers to
    // a WSP001 `_Unsupported` placeholder today, so pure scopes never see
    // these and the general fuzz mode doesn't drown in that known bucket.
    ns_scalars: Vec<Sca>,
    ns_arrays: Vec<(String, Ty, bool)>,
    n: u32,
    stmt_budget: i32,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Gen {
            rng: Rng::new(seed),
            blocks: Vec::new(),
            execs: Vec::new(),
            signals: Vec::new(),
            scalars: Vec::new(),
            arrays: Vec::new(),
            records: Vec::new(),
            rec_binds: Vec::new(),
            objs: Vec::new(),
            chips: Vec::new(),
            mods: Vec::new(),
            out_execs: Vec::new(),
            typed_val_outs: Vec::new(),
            bool_lets: Vec::new(),
            ns_scalars: Vec::new(),
            ns_arrays: Vec::new(),
            n: 0,
            stmt_budget: 40,
        }
    }

    fn fresh(&mut self, p: &str) -> String {
        self.n += 1;
        format!("{p}{}", self.n)
    }

    fn top_scope(&self, exec: bool) -> Scope {
        let mut scalars = self.scalars.clone();
        let mut arrays = self.arrays.clone();
        if exec {
            scalars.extend(self.ns_scalars.iter().cloned());
            arrays.extend(self.ns_arrays.iter().cloned());
        }
        Scope {
            scalars,
            rec_binds: self.rec_binds.clone(),
            arrays,
            objs: self.objs.clone(),
            exec,
            mod_ret: None,
        }
    }

    // ── expressions ──

    fn lit(&mut self, ty: Ty) -> String {
        match ty {
            Ty::Int => match self.rng.below(10) {
                0 => "0".into(),
                1 => "1".into(),
                2 => format!("-{}", self.rng.range(1, 20)),
                3 => "255".into(),
                _ => format!("{}", self.rng.below(100)),
            },
            Ty::Float => match self.rng.below(8) {
                0 => "0.0".into(),
                1 => "1.5".into(),
                2 => format!("-{}.25", self.rng.below(9)),
                _ => format!("{}.{}", self.rng.below(50), self.rng.below(10)),
            },
            Ty::Bool => if self.rng.chance(1, 2) { "true" } else { "false" }.into(),
            Ty::Str => match self.rng.below(8) {
                0 => "\"\"".into(),
                1 => "\"héllo wörld\"".into(),
                _ => format!("\"s{}\"", self.rng.below(30)),
            },
            Ty::Vec3 => {
                let a = self.lit(Ty::Float);
                let b = self.lit(Ty::Float);
                let c = self.lit(Ty::Float);
                format!("Vec({a}, {b}, {c})")
            }
        }
    }

    /// A readable symbol of the given type, or None.
    fn sym_read(&mut self, sc: &Scope, ty: Ty) -> Option<String> {
        let cands: Vec<&Sca> = sc.scalars.iter().filter(|s| s.ty == ty).collect();
        if cands.is_empty() {
            return None;
        }
        let s = cands[self.rng.below(cands.len())];
        Some(match s.acc {
            Acc::Direct => s.name.clone(),
            Acc::Var => {
                if sc.exec {
                    s.name.clone()
                } else {
                    format!("{}.Value", s.name)
                }
            }
        })
    }

    fn atom(&mut self, sc: &Scope, ty: Ty) -> String {
        if self.rng.chance(1, 2) {
            if let Some(s) = self.sym_read(sc, ty) {
                return s;
            }
        }
        // record field access
        if self.rng.chance(1, 5) && !sc.rec_binds.is_empty() {
            let (name, ridx) = sc.rec_binds[self.rng.below(sc.rec_binds.len())].clone();
            let rec = self.records[ridx].clone();
            let fields: Vec<&(String, Ty)> = rec.fields.iter().filter(|(_, t)| *t == ty).collect();
            if !fields.is_empty() {
                let f = fields[self.rng.below(fields.len())];
                return format!("{}.{}", name, f.0);
            }
        }
        self.lit(ty)
    }

    fn expr(&mut self, sc: &Scope, ty: Ty, d: u32) -> String {
        if d == 0 || self.rng.chance(1, 4) {
            return self.atom(sc, ty);
        }
        match ty {
            Ty::Int => match self.rng.below(14) {
                0 | 1 | 2 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    let b = self.expr(sc, Ty::Int, d - 1);
                    let op = *self.rng.pick(&["+", "-", "*"]);
                    format!("({a} {op} {b})")
                }
                3 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    let b = self.rng.range(1, 9);
                    let op = *self.rng.pick(&["/", "%"]);
                    format!("({a} {op} {b})")
                }
                4 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    let b = self.expr(sc, Ty::Int, d - 1);
                    let op = *self.rng.pick(&["&", "|", "^"]);
                    format!("({a} {op} {b})")
                }
                5 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    let sh = self.rng.range(1, 4);
                    let op = *self.rng.pick(&["<<", ">>"]);
                    format!("({a} {op} {sh})")
                }
                6 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    format!("abs({a})")
                }
                7 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    let b = self.expr(sc, Ty::Int, d - 1);
                    let f = *self.rng.pick(&["min", "max"]);
                    format!("{f}({a}, {b})")
                }
                8 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    format!("clamp({a}, 0, 99)")
                }
                9 => self.if_then_else(sc, ty, d),
                10 => {
                    if let Some(c) = self.pure_call(sc, ty, d) {
                        c
                    } else {
                        self.atom(sc, ty)
                    }
                }
                11 if sc.exec && !sc.arrays.is_empty() => {
                    let arrs: Vec<(String, Ty, bool)> = sc
                        .arrays
                        .iter()
                        .filter(|(_, t, _)| *t == Ty::Int)
                        .cloned()
                        .collect();
                    if arrs.is_empty() {
                        self.atom(sc, ty)
                    } else {
                        let (a, _, _) = self.rng.pick(&arrs).clone();
                        let i = self.expr(sc, Ty::Int, 0);
                        format!("{a}[{i}]")
                    }
                }
                12 => {
                    let a = self.expr(sc, Ty::Int, d - 1);
                    format!("(-{a})")
                }
                _ => self.atom(sc, ty),
            },
            Ty::Float => match self.rng.below(12) {
                0 | 1 | 2 => {
                    let a = self.expr(sc, Ty::Float, d - 1);
                    let b = self.expr(sc, Ty::Float, d - 1);
                    let op = *self.rng.pick(&["+", "-", "*"]);
                    format!("({a} {op} {b})")
                }
                3 => {
                    let a = self.expr(sc, Ty::Float, d - 1);
                    format!("({a} / 2.0)")
                }
                4 => {
                    let a = self.expr(sc, Ty::Float, d - 1);
                    let f = *self.rng.pick(&["sin", "cos", "abs"]);
                    format!("{f}({a})")
                }
                5 => {
                    let a = self.expr(sc, Ty::Float, d - 1);
                    format!("sqrt(abs({a}))")
                }
                6 => {
                    // vector swizzle
                    let v = self.expr(sc, Ty::Vec3, d - 1);
                    let f = *self.rng.pick(&["x", "y", "z"]);
                    format!("{v}.{f}")
                }
                7 => {
                    let a = self.expr(sc, Ty::Vec3, d - 1);
                    let b = self.expr(sc, Ty::Vec3, d - 1);
                    format!("{a}.Dot({b})")
                }
                8 => self.if_then_else(sc, ty, d),
                9 => {
                    // int coerces to float
                    self.expr(sc, Ty::Int, d - 1)
                }
                10 => {
                    let a = self.expr(sc, Ty::Float, d - 1);
                    let b = self.expr(sc, Ty::Float, d - 1);
                    format!("atan2({a}, {b})")
                }
                _ => self.atom(sc, ty),
            },
            Ty::Bool => match self.rng.below(10) {
                0 | 1 => {
                    let t = *self.rng.pick(&[Ty::Int, Ty::Float]);
                    let a = self.expr(sc, t, d - 1);
                    let b = self.expr(sc, t, d - 1);
                    let op = *self.rng.pick(&["<", ">", "<=", ">=", "==", "!="]);
                    format!("({a} {op} {b})")
                }
                2 => {
                    let a = self.expr(sc, Ty::Str, d - 1);
                    let b = self.expr(sc, Ty::Str, d - 1);
                    let op = *self.rng.pick(&["==", "!="]);
                    format!("({a} {op} {b})")
                }
                3 | 4 => {
                    let a = self.expr(sc, Ty::Bool, d - 1);
                    let b = self.expr(sc, Ty::Bool, d - 1);
                    let op = *self.rng.pick(&["&&", "||"]);
                    format!("({a} {op} {b})")
                }
                5 => {
                    let a = self.expr(sc, Ty::Bool, d - 1);
                    format!("!({a})")
                }
                6 => self.if_then_else(sc, ty, d),
                7 => {
                    // var.prev comparison (docs pattern: c0 != c0.prev) -
                    // never on a namespaced `N.g` path (`.prev` through a
                    // namespace hop is unsupported syntax).
                    let vars: Vec<&Sca> = sc
                        .scalars
                        .iter()
                        .filter(|s| {
                            s.acc == Acc::Var
                                && matches!(s.ty, Ty::Int | Ty::Float)
                                && !s.name.contains('.')
                        })
                        .collect();
                    if let Some(v) = (!vars.is_empty())
                        .then(|| vars[self.rng.below(vars.len())].name.clone())
                    {
                        if sc.exec {
                            format!("({v} != {v}.prev)")
                        } else {
                            format!("({v}.Value != {v}.prev)")
                        }
                    } else {
                        self.atom(sc, ty)
                    }
                }
                _ => self.atom(sc, ty),
            },
            Ty::Str => match self.rng.below(8) {
                0 | 1 => {
                    let a = self.expr(sc, Ty::Str, d - 1);
                    let b = self.expr(sc, Ty::Str, d - 1);
                    format!("({a} .. {b})")
                }
                2 | 3 => {
                    let i = self.expr(sc, Ty::Int, d - 1);
                    format!("\"v=${{{i}}}\"")
                }
                4 => self.if_then_else(sc, ty, d),
                _ => self.atom(sc, ty),
            },
            Ty::Vec3 => match self.rng.below(9) {
                0 | 1 => {
                    let a = self.expr(sc, Ty::Vec3, d - 1);
                    let b = self.expr(sc, Ty::Vec3, d - 1);
                    let op = *self.rng.pick(&["+", "-", "*"]);
                    format!("({a} {op} {b})")
                }
                2 => {
                    let v = self.expr(sc, Ty::Vec3, d - 1);
                    let f = self.expr(sc, Ty::Float, d - 1);
                    format!("({v} * {f})")
                }
                3 => {
                    let v = self.expr(sc, Ty::Vec3, d - 1);
                    let f = self.expr(sc, Ty::Float, d - 1);
                    format!("({f} * {v})")
                }
                4 => self.if_then_else(sc, ty, d),
                5 => {
                    let a = self.expr(sc, Ty::Float, d - 1);
                    let b = self.expr(sc, Ty::Float, d - 1);
                    let c = self.expr(sc, Ty::Float, d - 1);
                    format!("Vec({a}, {b}, {c})")
                }
                _ => self.atom(sc, ty),
            },
        }
    }

    fn if_then_else(&mut self, sc: &Scope, ty: Ty, d: u32) -> String {
        let c = self.expr(sc, Ty::Bool, d - 1);
        // occasionally use block-expr arms
        if self.rng.chance(1, 6) {
            let tmp = self.fresh("g");
            let a = self.expr(sc, ty, d - 1);
            let b = self.expr(sc, ty, d - 1);
            return format!("(if {c} then {{ let {tmp} = {a}; {tmp} }} else {{ {b} }})");
        }
        let a = self.expr(sc, ty, d - 1);
        if self.rng.chance(1, 5) {
            let c2 = self.expr(sc, Ty::Bool, d - 1);
            let b = self.expr(sc, ty, d - 1);
            let e = self.expr(sc, ty, d - 1);
            return format!("(if {c} then {a} else if {c2} then {b} else {e})");
        }
        let b = self.expr(sc, ty, d - 1);
        format!("(if {c} then {a} else {b})")
    }

    fn record_lit(&mut self, sc: &Scope, ridx: usize, d: u32) -> String {
        let rec = self.records[ridx].clone();
        // spread form
        let same: Vec<&(String, usize)> =
            sc.rec_binds.iter().filter(|(_, i)| *i == ridx).collect();
        if !same.is_empty() && self.rng.chance(1, 3) {
            let base = same[self.rng.below(same.len())].0.clone();
            let f = rec.fields[self.rng.below(rec.fields.len())].clone();
            let v = self.expr(sc, f.1, d.saturating_sub(1));
            return format!("{{ ...{base}, {}: {v} }}", f.0);
        }
        let mut parts = Vec::new();
        for (fname, fty) in &rec.fields {
            let v = self.expr(sc, *fty, d.saturating_sub(1));
            parts.push(format!("{fname}: {v}"));
        }
        format!("{{ {} }}", parts.join(", "))
    }

    /// A call to a pure mod or single-out pure chip returning `ty`.
    fn pure_call(&mut self, sc: &Scope, ty: Ty, d: u32) -> Option<String> {
        let mut cands: Vec<(String, Vec<(String, PTy)>)> = Vec::new();
        for m in &self.mods {
            if m.pure_call {
                if let Some(RetTy::Val(t)) = &m.ret {
                    if *t == ty {
                        cands.push((m.name.clone(), m.params.clone()));
                    }
                }
            }
        }
        for c in &self.chips {
            if c.pure_expr_callable {
                if let Some((_, Some(t))) = c.outs.first() {
                    if *t == ty && c.outs.len() == 1 {
                        cands.push((c.name.clone(), c.params.clone()));
                    }
                }
            }
        }
        if cands.is_empty() {
            return None;
        }
        let (name, params) = cands[self.rng.below(cands.len())].clone();
        let mut args = Vec::new();
        for (_, p) in &params {
            match p {
                PTy::Val(t) => args.push(self.expr(sc, *t, d.saturating_sub(1))),
                PTy::Rec(ridx) | PTy::RecDestr(ridx) => {
                    // pass an existing binding or a literal
                    let same: Vec<&(String, usize)> =
                        sc.rec_binds.iter().filter(|(_, i)| i == ridx).collect();
                    if !same.is_empty() && self.rng.chance(2, 3) {
                        args.push(same[self.rng.below(same.len())].0.clone());
                    } else {
                        args.push(self.record_lit(sc, *ridx, 1));
                    }
                }
                _ => return None,
            }
        }
        // Explicit type-argument call syntax `f<int>(...)` on the generic mods
        // registered by gen_mod's form-6 arm. `ty` is already the concrete
        // type this candidate was selected for (it's `T`, since every
        // param/ret of that mod shares one type parameter), so pinning it
        // explicitly is always a valid, redundant-with-inference annotation
        // — never a type mismatch.
        let is_generic = self.mods.iter().any(|m| m.name == name && m.generic);
        if is_generic && self.rng.chance(1, 3) {
            return Some(format!("{name}<{}>({})", ty.name(), args.join(", ")));
        }
        Some(format!("{name}({})", args.join(", ")))
    }

    // ── exec statements ──

    fn exec_stmts(
        &mut self,
        sc: &mut Scope,
        d: u32,
        count: usize,
        cur_trigger: Option<&str>,
        indent: usize,
    ) -> Vec<String> {
        let pad = "  ".repeat(indent);
        let mut out = Vec::new();
        for _ in 0..count {
            if self.stmt_budget <= 0 {
                break;
            }
            self.stmt_budget -= 1;
            let s = self.exec_stmt(sc, d, cur_trigger, indent);
            if !s.is_empty() {
                out.push(format!("{pad}{s}"));
            }
        }
        out
    }

    fn exec_stmt(&mut self, sc: &mut Scope, d: u32, cur: Option<&str>, indent: usize) -> String {
        match self.rng.below(21) {
            // shadow an existing in-scope name with a same-typed `let`
            // (including a `let` shadowing a `var` - the shadowed var's
            // storage keeps existing but later reads in this scope bind to
            // the let). Namespaced `N.x` paths can't be shadowed by a bare
            // let, so they're excluded.
            19 => {
                let cands: Vec<Sca> = sc
                    .scalars
                    .iter()
                    .filter(|s| !s.name.contains('.'))
                    .cloned()
                    .collect();
                if cands.is_empty() {
                    return String::new();
                }
                let target = self.rng.pick(&cands).clone();
                let e = self.expr(sc, target.ty, d);
                // model the shadow: every later reference resolves to the let
                for s in sc.scalars.iter_mut().filter(|s| s.name == target.name) {
                    s.acc = Acc::Direct;
                }
                format!("let {} = {e}", target.name)
            }
            // assignment
            0 | 1 | 2 => {
                let vars: Vec<Sca> = sc
                    .scalars
                    .iter()
                    .filter(|s| s.acc == Acc::Var)
                    .cloned()
                    .collect();
                if vars.is_empty() {
                    let h = self.fresh("h");
                    let t = *self.rng.pick(&SCALAR_TYS);
                    let e = self.expr(sc, t, d);
                    sc.scalars.push(Sca { name: h.clone(), ty: t, acc: Acc::Direct });
                    return format!("let {h} = {e}");
                }
                let v = self.rng.pick(&vars).clone();
                let e = self.expr(sc, v.ty, d);
                format!("{} = {e}", v.name)
            }
            // compound assignment
            3 => {
                let vars: Vec<Sca> = sc
                    .scalars
                    .iter()
                    .filter(|s| s.acc == Acc::Var && matches!(s.ty, Ty::Int | Ty::Float))
                    .cloned()
                    .collect();
                if vars.is_empty() {
                    return String::new();
                }
                let v = self.rng.pick(&vars).clone();
                let op = *self.rng.pick(&["+=", "-=", "*="]);
                let e = self.expr(sc, v.ty, d.min(1));
                format!("{} {op} {e}", v.name)
            }
            // if statement
            4 | 5 => {
                if indent > 3 {
                    return String::new();
                }
                let c = self.expr(sc, Ty::Bool, d);
                let cnt = self.rng.range(1, 2);
                let inner = self.exec_stmts(sc, d, cnt, cur, indent + 1);
                let pad = "  ".repeat(indent);
                let mut s = format!("if {c} {{\n{}\n{pad}}}", inner.join("\n"));
                if self.rng.chance(1, 3) {
                    let els = self.exec_stmts(sc, d, 1, cur, indent + 1);
                    s.push_str(&format!(" else {{\n{}\n{pad}}}", els.join("\n")));
                }
                s
            }
            // array ops
            6 | 7 => {
                let arrs: Vec<(String, Ty, bool)> = sc
                    .arrays
                    .iter()
                    .filter(|(_, _, w)| *w)
                    .cloned()
                    .collect();
                if arrs.is_empty() {
                    return String::new();
                }
                let (a, et, _) = self.rng.pick(&arrs).clone();
                match self.rng.below(12) {
                    0 | 1 | 2 => {
                        let v = self.expr(sc, et, d.min(1));
                        format!("{a}.push({v})")
                    }
                    3 => format!("{a}.clear()"),
                    4 => {
                        let h = self.fresh("h");
                        sc.scalars.push(Sca { name: h.clone(), ty: Ty::Int, acc: Acc::Direct });
                        format!("let {h} = {a}.length()")
                    }
                    5 => {
                        let i = self.expr(sc, Ty::Int, 0);
                        let v = self.expr(sc, et, d.min(1));
                        format!("{a}[{i}] = {v}")
                    }
                    6 => {
                        let h = self.fresh("h");
                        let i = self.expr(sc, Ty::Int, 0);
                        sc.scalars.push(Sca { name: h.clone(), ty: et, acc: Acc::Direct });
                        format!("let {h} = {a}[{i}]")
                    }
                    7 => format!("{a}.{}()", self.rng.pick(&["sort", "reverse", "shuffle"])),
                    8 if matches!(et, Ty::Int | Ty::Float) => {
                        let h = self.fresh("h");
                        sc.scalars.push(Sca { name: h.clone(), ty: et, acc: Acc::Direct });
                        format!("let {h} = {a}.sum()")
                    }
                    9 => {
                        let i = self.expr(sc, Ty::Int, 0);
                        let v = self.expr(sc, et, 0);
                        format!("{a}.insert({i}, {v})")
                    }
                    10 => {
                        let h = self.fresh("h");
                        let v = self.expr(sc, et, 0);
                        let hn = h.clone();
                        let r = format!("let {h} = {a}.find({v})");
                        // follow-up use of .Found / .Index
                        sc.scalars.push(Sca {
                            name: format!("{hn}.Index"),
                            ty: Ty::Int,
                            acc: Acc::Direct,
                        });
                        sc.scalars.push(Sca {
                            name: format!("{hn}.Found"),
                            ty: Ty::Bool,
                            acc: Acc::Direct,
                        });
                        r
                    }
                    _ => {
                        let h = self.fresh("h");
                        sc.scalars.push(Sca { name: h.clone(), ty: et, acc: Acc::Direct });
                        format!("let {h} = {a}.pop()")
                    }
                }
            }
            // exec mod call
            8 | 9 => {
                let mods: Vec<ModSig> = self
                    .mods
                    .iter()
                    .filter(|m| !m.pure_call || m.ret.is_none())
                    .cloned()
                    .collect();
                let mods = if mods.is_empty() { self.mods.clone() } else { mods };
                if mods.is_empty() {
                    return String::new();
                }
                let m = self.rng.pick(&mods).clone();
                let mut args = Vec::new();
                for (_, p) in &m.params {
                    match p {
                        PTy::Val(t) => args.push(self.expr(sc, *t, d.min(1))),
                        PTy::Ref(t) => {
                            let vars: Vec<&Sca> = sc
                                .scalars
                                .iter()
                                .filter(|s| s.acc == Acc::Var && s.ty == *t)
                                .collect();
                            if vars.is_empty() {
                                return String::new();
                            }
                            args.push(vars[self.rng.below(vars.len())].name.clone());
                        }
                        PTy::Rec(ridx) | PTy::RecDestr(ridx) => {
                            let lit = self.record_lit(sc, *ridx, 1);
                            args.push(lit);
                        }
                        PTy::Exec => return String::new(),
                    }
                }
                match &m.ret {
                    Some(RetTy::Val(t)) => {
                        let h = self.fresh("h");
                        sc.scalars.push(Sca { name: h.clone(), ty: *t, acc: Acc::Direct });
                        format!("let {h} = {}({})", m.name, args.join(", "))
                    }
                    _ => format!("{}({})", m.name, args.join(", ")),
                }
            }
            // emit
            10 | 11 => {
                let mut targets: Vec<(String, Option<Ty>)> = Vec::new();
                for s in &self.out_execs {
                    targets.push((s.clone(), None));
                }
                for s in &self.signals {
                    if Some(s.as_str()) != cur {
                        targets.push((s.clone(), None));
                    }
                }
                for (o, t) in &self.typed_val_outs {
                    targets.push((o.clone(), Some(*t)));
                }
                if targets.is_empty() {
                    return String::new();
                }
                let (t, ty) = self.rng.pick(&targets).clone();
                match ty {
                    Some(vt) => {
                        let e = self.expr(sc, vt, d);
                        format!("emit {t} = {e}")
                    }
                    None => {
                        if self.rng.chance(1, 4) && self.signals.contains(&t) {
                            // payload ferry
                            let e = self.expr(sc, Ty::Int, d.min(1));
                            format!("emit {t} = {e}")
                        } else if self.rng.chance(1, 6) {
                            format!("buffer emit {t}")
                        } else {
                            format!("emit {t}")
                        }
                    }
                }
            }
            // await
            12 => match self.rng.below(4) {
                0 => format!("await SleepTicks(_, delay = {})", self.rng.range(1, 5)),
                1 => "await Sleep(_, delay = 0.5)".into(),
                2 if !self.signals.is_empty() => {
                    let sigs: Vec<String> = self
                        .signals
                        .iter()
                        .filter(|s| Some(s.as_str()) != cur)
                        .cloned()
                        .collect();
                    if sigs.is_empty() {
                        return String::new();
                    }
                    let s = self.rng.pick(&sigs).clone();
                    if self.rng.chance(1, 2) {
                        format!("await {s}")
                    } else {
                        let h = self.fresh("h");
                        let src = self
                            .sym_read(sc, Ty::Int)
                            .unwrap_or_else(|| "1".into());
                        sc.scalars.push(Sca { name: h.clone(), ty: Ty::Int, acc: Acc::Direct });
                        format!("let {h} = await {src} on {s}")
                    }
                }
                _ => String::new(),
            },
            // receiver calls
            13 => {
                if sc.objs.is_empty() {
                    return String::new();
                }
                let (o, k) = sc.objs[self.rng.below(sc.objs.len())].clone();
                match k {
                    Obj::Controller => {
                        let msg = self.expr(sc, Ty::Str, d.min(1));
                        format!("{o}.DisplayText({msg}, fontSize = 30)")
                    }
                    Obj::Entity => {
                        let v = self.expr(sc, Ty::Vec3, d.min(1));
                        format!("{o}.SetLocation({v})")
                    }
                    Obj::Character => {
                        // char<->controller coercion probe
                        let msg = self.expr(sc, Ty::Str, 0);
                        format!("{o}.DisplayText({msg}, fontSize = 24)")
                    }
                }
            }
            // local let / Random / static var / var
            14 | 15 => match self.rng.below(5) {
                0 => {
                    let h = self.fresh("h");
                    sc.scalars.push(Sca { name: h.clone(), ty: Ty::Int, acc: Acc::Direct });
                    format!("let {h} = Random(0, {})", self.rng.range(1, 20))
                }
                1 => {
                    let h = self.fresh("hv");
                    let init = self.lit(Ty::Int);
                    sc.scalars.push(Sca { name: h.clone(), ty: Ty::Int, acc: Acc::Var });
                    format!("static var {h}: int = {init}")
                }
                2 => {
                    let h = self.fresh("hv");
                    let t = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
                    let init = self.lit(t);
                    sc.scalars.push(Sca { name: h.clone(), ty: t, acc: Acc::Var });
                    format!("var {h}: {} = {init}", t.name())
                }
                _ => {
                    let h = self.fresh("h");
                    let t = *self.rng.pick(&SCALAR_TYS);
                    let e = self.expr(sc, t, d);
                    sc.scalars.push(Sca { name: h.clone(), ty: t, acc: Acc::Direct });
                    format!("let {h} = {e}")
                }
            },
            // return
            16 => {
                if let Some(rt) = sc.mod_ret {
                    let e = self.expr(sc, rt, d);
                    format!("return {e}")
                } else if self.rng.chance(1, 4) {
                    let c = self.expr(sc, Ty::Bool, 1);
                    let pad = "  ".repeat(indent);
                    format!("if {c} {{\n{pad}  return\n{pad}}}")
                } else {
                    String::new()
                }
            }
            // chip instantiation inside handler
            17 => {
                let chips: Vec<ChipSig> = self
                    .chips
                    .iter()
                    .filter(|c| c.pure_expr_callable)
                    .cloned()
                    .collect();
                if chips.is_empty() {
                    return String::new();
                }
                let c = self.rng.pick(&chips).clone();
                let mut args = Vec::new();
                for (_, p) in &c.params {
                    match p {
                        PTy::Val(t) => args.push(self.expr(sc, *t, 1)),
                        PTy::Rec(r) | PTy::RecDestr(r) => {
                            let lit = self.record_lit(sc, *r, 1);
                            args.push(lit);
                        }
                        _ => return String::new(),
                    }
                }
                let h = self.fresh("h");
                if let Some((_, Some(t))) = c.outs.first() {
                    sc.scalars.push(Sca { name: h.clone(), ty: *t, acc: Acc::Direct });
                }
                format!("let {h} = {}({})", c.name, args.join(", "))
            }
            // anon chip wrapping statements
            18 => {
                if indent > 2 {
                    return String::new();
                }
                let cnt = self.rng.range(1, 2);
                let inner = self.exec_stmts(sc, d, cnt, cur, indent + 1);
                if inner.is_empty() {
                    return String::new();
                }
                let pad = "  ".repeat(indent);
                format!("chip {{\n{}\n{pad}}}", inner.join("\n"))
            }
            _ => {
                // fallback: assignment-flavored again
                let vars: Vec<Sca> = sc
                    .scalars
                    .iter()
                    .filter(|s| s.acc == Acc::Var)
                    .cloned()
                    .collect();
                if vars.is_empty() {
                    return String::new();
                }
                let v = self.rng.pick(&vars).clone();
                let e = self.expr(sc, v.ty, d);
                format!("{} = {e}", v.name)
            }
        }
    }

    // ── top-level declarations ──

    fn gen_type_alias(&mut self) {
        let name = self.fresh("R");
        let nf = self.rng.range(2, 3);
        let mut fields = Vec::new();
        for i in 0..nf {
            let fname = format!("f{}_{}", self.records.len(), i);
            let ty = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::Int]);
            fields.push((fname, ty));
        }
        let body = fields
            .iter()
            .map(|(n, t)| format!("{n}: {}", t.name()))
            .collect::<Vec<_>>()
            .join(", ");
        self.blocks.push(format!("type {name} = {{ {body} }}"));
        self.records.push(RecordDef { name, fields });
    }

    fn gen_inputs(&mut self) {
        let n_exec = self.rng.range(1, 3);
        for _ in 0..n_exec {
            let t = self.fresh("t");
            let ann = match self.rng.below(6) {
                0 => "@left ".to_string(),
                1 => "@right ".to_string(),
                2 => format!("@label(\"go {}\") ", self.n),
                3 => "@top @label(\"trig\") ".to_string(),
                _ => String::new(),
            };
            self.blocks.push(format!("{ann}in {t}: exec"));
            self.execs.push(t);
        }
        let n_val = self.rng.range(0, 2);
        for _ in 0..n_val {
            let p = self.fresh("p");
            let ty = *self.rng.pick(&SCALAR_TYS);
            let ann = if self.rng.chance(1, 5) { "@bottom " } else { "" };
            self.blocks.push(format!("{ann}in {p}: {}", ty.name()));
            self.scalars.push(Sca { name: p, ty, acc: Acc::Direct });
        }
        if self.rng.chance(1, 3) {
            let o = self.fresh("obj");
            let k = *self.rng.pick(&[Obj::Entity, Obj::Controller, Obj::Character]);
            self.blocks.push(format!("in {o}: {}", k.name()));
            self.objs.push((o, k));
        }
        if self.rng.chance(1, 5) {
            let a = self.fresh("xs");
            let ty = *self.rng.pick(&[Ty::Int, Ty::Float]);
            self.blocks.push(format!("in {a}: {}[]", ty.name()));
            self.arrays.push((a, ty, false));
        }
    }

    fn gen_vars(&mut self) {
        let n = self.rng.range(1, 4);
        for _ in 0..n {
            let v = self.fresh("v");
            let ty = *self.rng.pick(&SCALAR_TYS);
            let init = self.lit(ty);
            let kw = if self.rng.chance(1, 8) { "static var" } else { "var" };
            self.blocks.push(format!("{kw} {v}: {} = {init}", ty.name()));
            self.scalars.push(Sca { name: v, ty, acc: Acc::Var });
        }
        let n_arr = self.rng.range(0, 2);
        for _ in 0..n_arr {
            let a = self.fresh("a");
            let ty = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::Vec3]);
            if self.rng.chance(1, 3) && matches!(ty, Ty::Int | Ty::Float | Ty::Str) {
                let k = self.rng.range(2, 4);
                let items: Vec<String> = (0..k).map(|_| self.lit(ty)).collect();
                self.blocks
                    .push(format!("var {a}: {}[] = [{}]", ty.name(), items.join(", ")));
            } else {
                self.blocks.push(format!("var {a}: {}[]", ty.name()));
            }
            self.arrays.push((a, ty, true));
        }
    }

    fn gen_buffers(&mut self) {
        let n = self.rng.range(0, 2);
        for _ in 0..n {
            let b = self.fresh("b");
            let ty = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
            let sc = self.top_scope(false);
            let e = self.expr(&sc, ty, 2);
            if self.rng.chance(1, 4) {
                self.blocks.push(format!("buffer {b}: {} = {e}", ty.name()));
            } else {
                self.blocks.push(format!("buffer {b} = {e}"));
            }
            self.scalars.push(Sca { name: b, ty, acc: Acc::Direct });
        }
    }

    fn gen_record_binds(&mut self) {
        if self.records.is_empty() {
            return;
        }
        let n = self.rng.range(0, 2);
        for _ in 0..n {
            let ridx = self.rng.below(self.records.len());
            let r = self.fresh("r");
            let sc = self.top_scope(false);
            let lit = self.record_lit(&sc, ridx, 2);
            let rec_name = self.records[ridx].name.clone();
            if self.rng.chance(1, 2) {
                self.blocks.push(format!("let {r}: {rec_name} = {lit}"));
            } else {
                self.blocks.push(format!("let {r} = {lit}"));
            }
            self.rec_binds.push((r.clone(), ridx));
            // destructure it sometimes
            if self.rng.chance(1, 3) {
                let fields = self.records[ridx].fields.clone();
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                self.blocks
                    .push(format!("let {{ {} }} = {r}", names.join(", ")));
                for (fname, fty) in &fields {
                    self.scalars.push(Sca {
                        name: fname.clone(),
                        ty: *fty,
                        acc: Acc::Direct,
                    });
                }
            }
        }
    }

    fn gen_mod(&mut self) {
        let name = self.fresh("M");
        let form = self.rng.below(7);
        match form {
            // pure mod: value params, single return
            0 | 1 => {
                let np = self.rng.range(1, 3);
                let mut params = Vec::new();
                let mut body_sc = Scope { exec: false, ..Default::default() };
                for i in 0..np {
                    // occasionally name a parameter after an existing global
                    // (parameter shadowing an outer name)
                    let pn = if self.rng.chance(1, 5) && !self.scalars.is_empty() {
                        let g = self.rng.pick(&self.scalars).name.clone();
                        if g.contains('.') { format!("q{i}") } else { g }
                    } else {
                        format!("q{i}")
                    };
                    if params.iter().any(|(n, _)| *n == pn) {
                        continue;
                    }
                    let t = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
                    params.push((pn.clone(), PTy::Val(t)));
                    body_sc.scalars.push(Sca { name: pn, ty: t, acc: Acc::Direct });
                }
                if params.is_empty() {
                    let t = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
                    params.push(("q0".to_string(), PTy::Val(t)));
                    body_sc.scalars.push(Sca { name: "q0".into(), ty: t, acc: Acc::Direct });
                }
                let rt = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
                let sig = params
                    .iter()
                    .map(|(n, p)| match p {
                        PTy::Val(t) => format!("{n}: {}", t.name()),
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                // multi-return via if?
                if self.rng.chance(1, 2) {
                    let c = self.expr(&body_sc, Ty::Bool, 2);
                    let e1 = self.expr(&body_sc, rt, 2);
                    let e2 = self.expr(&body_sc, rt, 2);
                    self.blocks.push(format!(
                        "mod {name}({sig}) -> (ret: {}) {{\n  if {c} {{ return {e1} }}\n  return {e2}\n}}",
                        rt.name()
                    ));
                } else {
                    let e = self.expr(&body_sc, rt, 3);
                    self.blocks
                        .push(format!("mod {name}({sig}) -> {} {{\n  return {e}\n}}", rt.name()));
                }
                self.mods.push(ModSig {
                    name,
                    params,
                    ret: Some(RetTy::Val(rt)),
                    pure_call: true,
                    generic: false,
                });
            }
            // exec mod with ref params
            2 | 3 => {
                let t = *self.rng.pick(&[Ty::Int, Ty::Float]);
                let mut params = vec![("w".to_string(), PTy::Ref(t))];
                let mut body_sc = Scope { exec: true, ..Default::default() };
                body_sc.scalars.push(Sca { name: "w".into(), ty: t, acc: Acc::Var });
                let mut sig = format!("w: *{}", t.name());
                if self.rng.chance(1, 2) {
                    params.push(("dd".to_string(), PTy::Val(t)));
                    body_sc.scalars.push(Sca { name: "dd".into(), ty: t, acc: Acc::Direct });
                    sig.push_str(&format!(", dd: {}", t.name()));
                }
                let cnt = self.rng.range(1, 3);
                let stmts = self.exec_stmts(&mut body_sc, 2, cnt, None, 1);
                let body = if stmts.is_empty() {
                    "  w = w".to_string()
                } else {
                    stmts.join("\n")
                };
                self.blocks.push(format!("mod {name}({sig}) {{\n{body}\n}}"));
                self.mods.push(ModSig { name, params, ret: None, pure_call: false, generic: false });
            }
            // destructured record param mod
            4 => {
                if self.records.is_empty() {
                    return self.gen_mod_fallback(name);
                }
                let ridx = self.rng.below(self.records.len());
                let rec = self.records[ridx].clone();
                let int_fields: Vec<&(String, Ty)> = rec
                    .fields
                    .iter()
                    .filter(|(_, t)| matches!(t, Ty::Int | Ty::Float))
                    .collect();
                if int_fields.len() < 2 {
                    return self.gen_mod_fallback(name);
                }
                let f0 = int_fields[0].0.clone();
                let f1 = int_fields[1].0.clone();
                let all: Vec<String> = rec.fields.iter().map(|(n, _)| n.clone()).collect();
                self.blocks.push(format!(
                    "mod {name}({{ {} }}: {}) -> int {{\n  return ({f0} + {f1})\n}}",
                    all.join(", "),
                    rec.name
                ));
                self.mods.push(ModSig {
                    name,
                    params: vec![("_p".into(), PTy::RecDestr(ridx))],
                    ret: Some(RetTy::Val(Ty::Int)),
                    pure_call: true,
                    generic: false,
                });
            }
            // record-literal return mod
            5 => {
                if self.records.is_empty() {
                    return self.gen_mod_fallback(name);
                }
                let ridx = self.rng.below(self.records.len());
                let rec = self.records[ridx].clone();
                let mut body_sc = Scope { exec: false, ..Default::default() };
                body_sc.scalars.push(Sca { name: "q0".into(), ty: Ty::Int, acc: Acc::Direct });
                let lit = self.record_lit(&body_sc, ridx, 1);
                self.blocks.push(format!(
                    "mod {name}(q0: int) -> {} {{\n  return {lit}\n}}",
                    rec.name
                ));
                self.mods.push(ModSig {
                    name,
                    params: vec![("q0".into(), PTy::Val(Ty::Int))],
                    ret: Some(RetTy::Rec(ridx)),
                    pure_call: true,
                    generic: false,
                });
            }
            // Unbounded `<T>` generic identity mod, structurally identical to
            // `pick<T>` in examples/generics.ws. Declared ONCE
            // as source text, but registered as `SCALAR_TYS.len()` separate
            // `ModSig` entries sharing one name — one per monomorphization.
            // Every existing call site (`pure_call`, the exec mod-call
            // fallback) already selects candidates purely by declared
            // param/ret `Ty`, oblivious to whether the decl behind a name is
            // monomorphic or generic — so this makes real generic mods
            // (implicit T-inference through arguments, real
            // monomorphization via mono.rs/infer.rs/coerce.rs) reachable
            // from every existing expression/statement shape for free, with
            // zero changes to call-site codegen.
            6 => {
                self.blocks.push(format!(
                    "mod {name}<T>(a: T, b: T, c: bool) -> T {{\n  return if c then a else b\n}}"
                ));
                for t in SCALAR_TYS {
                    self.mods.push(ModSig {
                        name: name.clone(),
                        params: vec![
                            ("a".into(), PTy::Val(t)),
                            ("b".into(), PTy::Val(t)),
                            ("c".into(), PTy::Val(Ty::Bool)),
                        ],
                        ret: Some(RetTy::Val(t)),
                        pure_call: true,
                        generic: true,
                    });
                }
            }
            _ => unreachable!(),
        }
    }

    fn gen_mod_fallback(&mut self, name: String) {
        let mut body_sc = Scope { exec: false, ..Default::default() };
        body_sc.scalars.push(Sca { name: "q0".into(), ty: Ty::Int, acc: Acc::Direct });
        let e = self.expr(&body_sc, Ty::Int, 2);
        self.blocks
            .push(format!("mod {name}(q0: int) -> int {{\n  return {e}\n}}"));
        self.mods.push(ModSig {
            name,
            params: vec![("q0".into(), PTy::Val(Ty::Int))],
            ret: Some(RetTy::Val(Ty::Int)),
            pure_call: true,
            generic: false,
        });
    }

    fn gen_chip(&mut self) {
        let name = self.fresh("C");
        let ann = match self.rng.below(6) {
            0 => "@closed ".to_string(),
            1 => format!("@label(\"chip {}\") ", self.n),
            _ => String::new(),
        };
        if self.rng.chance(2, 3) {
            // pure chip
            let np = self.rng.range(1, 3);
            let mut params = Vec::new();
            let mut body_sc = Scope { exec: false, ..Default::default() };
            let mut sig_parts = Vec::new();
            for i in 0..np {
                let pn = format!("k{i}");
                if self.rng.chance(1, 4) && !self.records.is_empty() {
                    let ridx = self.rng.below(self.records.len());
                    sig_parts.push(format!("{pn}: {}", self.records[ridx].name));
                    params.push((pn.clone(), PTy::Rec(ridx)));
                    body_sc.rec_binds.push((pn, ridx));
                } else {
                    let t = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::Vec3]);
                    sig_parts.push(format!("{pn}: {}", t.name()));
                    params.push((pn.clone(), PTy::Val(t)));
                    body_sc.scalars.push(Sca { name: pn, ty: t, acc: Acc::Direct });
                }
            }
            let single = self.rng.chance(1, 2);
            if single {
                let ot = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::Vec3]);
                let e = self.expr(&body_sc, ot, 3);
                self.blocks.push(format!(
                    "{ann}chip {name}({}) -> {} {{\n  out _ = {e}\n}}",
                    sig_parts.join(", "),
                    ot.name()
                ));
                self.chips.push(ChipSig {
                    name,
                    params,
                    outs: vec![("_".into(), Some(ot))],
                    pure_expr_callable: true,
                });
            } else {
                let no = self.rng.range(1, 2);
                let mut outs = Vec::new();
                let mut out_sig = Vec::new();
                let mut out_lines = Vec::new();
                for i in 0..no {
                    let on = format!("z{i}");
                    let ot = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
                    out_sig.push(format!("{on}: {}", ot.name()));
                    let e = self.expr(&body_sc, ot, 2);
                    out_lines.push(format!("  out {on} = {e}"));
                    outs.push((on, Some(ot)));
                }
                let pure_one = outs.len() == 1;
                self.blocks.push(format!(
                    "{ann}chip {name}({}) -> ({}) {{\n{}\n}}",
                    sig_parts.join(", "),
                    out_sig.join(", "),
                    out_lines.join("\n")
                ));
                self.chips.push(ChipSig {
                    name,
                    params,
                    outs,
                    pure_expr_callable: pure_one,
                });
            }
        } else {
            // stateful chip: exec param + handler + value out (+ maybe exec out)
            let mut params = vec![("bump".to_string(), PTy::Exec)];
            let mut sig_parts = vec!["bump: exec".to_string()];
            let mut body_sc = Scope { exec: true, ..Default::default() };
            if self.rng.chance(1, 2) {
                let t = *self.rng.pick(&[Ty::Int, Ty::Float]);
                sig_parts.push(format!("k0: {}", t.name()));
                params.push(("k0".into(), PTy::Val(t)));
                body_sc.scalars.push(Sca { name: "k0".into(), ty: t, acc: Acc::Direct });
            }
            let vt = *self.rng.pick(&[Ty::Int, Ty::Float]);
            body_sc.scalars.push(Sca { name: "n".into(), ty: vt, acc: Acc::Var });
            let with_exec_out = self.rng.chance(1, 3);
            let mut outs = vec![("z0".to_string(), Some(vt))];
            let mut out_sig = format!("z0: {}", vt.name());
            if with_exec_out {
                out_sig.push_str(", fin: exec");
                outs.push(("fin".into(), None));
            }
            let cnt = self.rng.range(1, 3);
            let mut stmts = self.exec_stmts(&mut body_sc, 2, cnt, None, 2);
            if stmts.is_empty() {
                stmts.push("    n = n + 1".into());
            }
            if with_exec_out {
                stmts.push("    emit fin".into());
            }
            let init = self.lit(vt);
            self.blocks.push(format!(
                "{ann}chip {name}({}) -> ({out_sig}) {{\n  var n: {} = {init}\n  on bump {{\n{}\n  }}\n  out z0: {} = n.Value\n}}",
                sig_parts.join(", "),
                vt.name(),
                stmts.join("\n"),
                vt.name()
            ));
            self.chips.push(ChipSig {
                name,
                params,
                outs,
                pure_expr_callable: false,
            });
        }
    }

    fn gen_top_lets(&mut self) {
        let n = self.rng.range(0, 3);
        for _ in 0..n {
            let sc = self.top_scope(false);
            match self.rng.below(8) {
                // chip instantiation (incl. stateful ones passing an exec in-port)
                0 | 1 if !self.chips.is_empty() => {
                    let c = {
                        let idx = self.rng.below(self.chips.len());
                        self.chips[idx].clone()
                    };
                    let mut args = Vec::new();
                    let mut ok = true;
                    for (_, p) in &c.params {
                        match p {
                            PTy::Val(t) => args.push(self.expr(&sc, *t, 2)),
                            PTy::Rec(r) | PTy::RecDestr(r) => {
                                let lit = self.record_lit(&sc, *r, 1);
                                args.push(lit);
                            }
                            PTy::Exec => {
                                if self.execs.is_empty() {
                                    ok = false;
                                    break;
                                }
                                args.push(self.rng.pick(&self.execs).clone());
                            }
                            PTy::Ref(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    let l = self.fresh("ci");
                    self.blocks
                        .push(format!("let {l} = {}({})", c.name, args.join(", ")));
                    let vouts: Vec<(String, Ty)> = c
                        .outs
                        .iter()
                        .filter_map(|(n2, t)| t.map(|t| (n2.clone(), t)))
                        .collect();
                    if vouts.len() == 1 && c.outs.len() == 1 {
                        // auto-unwrap
                        self.scalars.push(Sca { name: l, ty: vouts[0].1, acc: Acc::Direct });
                    } else {
                        for (on, ot) in &vouts {
                            self.scalars.push(Sca {
                                name: format!("{l}.{on}"),
                                ty: *ot,
                                acc: Acc::Direct,
                            });
                        }
                    }
                }
                // pure mod call
                2 if !self.mods.is_empty() => {
                    if let Some(call) = self.pure_call(&sc, Ty::Int, 2) {
                        let l = self.fresh("l");
                        self.blocks.push(format!("let {l} = {call}"));
                        self.scalars.push(Sca { name: l, ty: Ty::Int, acc: Acc::Direct });
                    }
                }
                // record-returning mod call + field access
                3 => {
                    let recs: Vec<ModSig> = self
                        .mods
                        .iter()
                        .filter(|m| matches!(m.ret, Some(RetTy::Rec(_))) && m.pure_call)
                        .cloned()
                        .collect();
                    if recs.is_empty() {
                        continue;
                    }
                    let m = self.rng.pick(&recs).clone();
                    let ridx = match m.ret {
                        Some(RetTy::Rec(r)) => r,
                        _ => unreachable!(),
                    };
                    let mut args = Vec::new();
                    for (_, p) in &m.params {
                        if let PTy::Val(t) = p {
                            args.push(self.expr(&sc, *t, 1));
                        }
                    }
                    let l = self.fresh("rr");
                    self.blocks
                        .push(format!("let {l} = {}({})", m.name, args.join(", ")));
                    self.rec_binds.push((l, ridx));
                }
                // tuple
                4 => {
                    let t = self.fresh("tp");
                    let e1 = self.expr(&sc, Ty::Int, 1);
                    let e2 = self.expr(&sc, Ty::Float, 1);
                    self.blocks.push(format!("let {t} = ({e1}, {e2})"));
                    if self.rng.chance(2, 3) {
                        let a = self.fresh("l");
                        let b = self.fresh("l");
                        self.blocks.push(format!("let ({a}, {b}) = {t}"));
                        self.scalars.push(Sca { name: a, ty: Ty::Int, acc: Acc::Direct });
                        self.scalars.push(Sca { name: b, ty: Ty::Float, acc: Acc::Direct });
                    }
                }
                // chip let
                5 => {
                    let l = self.fresh("l");
                    let ty = *self.rng.pick(&[Ty::Int, Ty::Bool, Ty::Float]);
                    let e = self.expr(&sc, ty, 3);
                    self.blocks.push(format!("chip let {l} = {e}"));
                    self.scalars.push(Sca { name: l.clone(), ty, acc: Acc::Direct });
                    if ty == Ty::Bool {
                        self.bool_lets.push(l);
                    }
                }
                // plain / annotated / block-expr let
                _ => {
                    let l = self.fresh("l");
                    let ty = *self.rng.pick(&SCALAR_TYS);
                    let e = if self.rng.chance(1, 6) {
                        let tmp = self.fresh("g");
                        let inner = self.expr(&sc, ty, 2);
                        format!("{{ let {tmp} = {inner}; {tmp} }}")
                    } else {
                        self.expr(&sc, ty, 3)
                    };
                    if self.rng.chance(1, 3) {
                        self.blocks.push(format!("let {l}: {} = {e}", ty.name()));
                    } else {
                        self.blocks.push(format!("let {l} = {e}"));
                    }
                    self.scalars.push(Sca { name: l.clone(), ty, acc: Acc::Direct });
                    if ty == Ty::Bool {
                        self.bool_lets.push(l);
                    }
                }
            }
        }
    }

    fn gen_signals_and_typed_outs(&mut self) {
        for _ in 0..self.rng.range(0, 2) {
            let s = self.fresh("sig");
            self.blocks.push(format!("let {s}: exec"));
            self.signals.push(s);
        }
        if self.rng.chance(1, 2) {
            let o = self.fresh("oe");
            self.blocks.push(format!("out {o}: exec"));
            self.out_execs.push(o);
        }
        if self.rng.chance(1, 3) {
            let o = self.fresh("ov");
            let t = *self.rng.pick(&[Ty::Int, Ty::Float, Ty::Str]);
            self.blocks.push(format!("out {o}: {}", t.name()));
            self.typed_val_outs.push((o, t));
        }
    }

    fn gen_handler(&mut self) {
        let mut sc = self.top_scope(true);
        let mut trigger;
        let mut cur: Option<String> = None;
        match self.rng.below(10) {
            0 if !self.bool_lets.is_empty() => {
                let b = self.rng.pick(&self.bool_lets).clone();
                trigger = format!("!{b}");
            }
            1 if self.execs.len() >= 2 => {
                let a = self.rng.pick(&self.execs).clone();
                let b = self.rng.pick(&self.execs).clone();
                trigger = format!("{a} | {b}");
            }
            2 if !self.signals.is_empty() => {
                let s = self.rng.pick(&self.signals).clone();
                cur = Some(s.clone());
                trigger = s;
            }
            3 => {
                // builtin event with binds
                let ev = self.rng.below(4);
                match ev {
                    0 => {
                        let c = self.fresh("ch");
                        sc.objs.push((c.clone(), Obj::Character));
                        trigger = format!("CharacterSpawned({c})");
                    }
                    1 => {
                        let c = self.fresh("ch");
                        sc.objs.push((c.clone(), Obj::Character));
                        trigger = format!("CharacterDied({c})");
                    }
                    2 => {
                        let c = self.fresh("ct");
                        let u = self.fresh("uid");
                        sc.objs.push((c.clone(), Obj::Controller));
                        sc.scalars.push(Sca { name: u.clone(), ty: Ty::Str, acc: Acc::Direct });
                        trigger = format!("ControllerJoined({c}, {u})");
                    }
                    _ => {
                        let c = self.fresh("ct");
                        let u = self.fresh("uid");
                        sc.objs.push((c.clone(), Obj::Controller));
                        sc.scalars.push(Sca { name: u.clone(), ty: Ty::Str, acc: Acc::Direct });
                        trigger = format!("ControllerLeft({c}, {u})");
                    }
                }
            }
            4 if !self.bool_lets.is_empty() => {
                trigger = self.rng.pick(&self.bool_lets).clone();
            }
            _ => {
                if self.execs.is_empty() {
                    let t = self.fresh("t");
                    self.blocks.push(format!("in {t}: exec"));
                    self.execs.push(t);
                }
                trigger = self.rng.pick(&self.execs).clone();
            }
        }
        if trigger.is_empty() {
            trigger = self.rng.pick(&self.execs).clone();
        }
        let cnt = self.rng.range(1, 5);
        let stmts = self.exec_stmts(&mut sc, 2, cnt, cur.as_deref(), 1);
        let body = if stmts.is_empty() {
            "  return".to_string()
        } else {
            stmts.join("\n")
        };
        let prefix = if self.rng.chance(1, 4) { "chip on" } else { "on" };
        self.blocks.push(format!("{prefix} {trigger} {{\n{body}\n}}"));
    }

    fn gen_outs(&mut self) {
        let n = self.rng.range(1, 3);
        for _ in 0..n {
            let sc = self.top_scope(false);
            match self.rng.below(8) {
                // out X = X aliasing a var
                0 => {
                    let vars: Vec<Sca> = self
                        .scalars
                        .iter()
                        .filter(|s| s.acc == Acc::Var && !s.name.contains('.'))
                        .cloned()
                        .collect();
                    if vars.is_empty() {
                        continue;
                    }
                    let v = self.rng.pick(&vars).clone();
                    self.blocks.push(format!("out {} = {}", v.name, v.name));
                }
                // typed value out from var .Value
                1 => {
                    let vars: Vec<Sca> = self
                        .scalars
                        .iter()
                        .filter(|s| s.acc == Acc::Var && !s.name.contains('.'))
                        .cloned()
                        .collect();
                    if vars.is_empty() {
                        continue;
                    }
                    let v = self.rng.pick(&vars).clone();
                    let o = self.fresh("o");
                    self.blocks
                        .push(format!("out {o}: {} = {}.Value", v.ty.name(), v.name));
                }
                // ref out
                2 => {
                    let vars: Vec<Sca> = self
                        .scalars
                        .iter()
                        .filter(|s| s.acc == Acc::Var && s.ty == Ty::Int && !s.name.contains('.'))
                        .cloned()
                        .collect();
                    if vars.is_empty() {
                        continue;
                    }
                    let v = self.rng.pick(&vars).clone();
                    let o = self.fresh("o");
                    self.blocks.push(format!("out {o}: *int = {}", v.name));
                }
                _ => {
                    let o = self.fresh("o");
                    let ty = *self.rng.pick(&SCALAR_TYS);
                    let e = self.expr(&sc, ty, 3);
                    let ann = match self.rng.below(8) {
                        0 => "@right ".to_string(),
                        1 => format!("@label(\"out {}\") ", self.n),
                        _ => String::new(),
                    };
                    if self.rng.chance(1, 4) {
                        self.blocks
                            .push(format!("{ann}out {o}: {} = {e}", ty.name()));
                    } else {
                        self.blocks.push(format!("{ann}out {o} = {e}"));
                    }
                }
            }
        }
    }

    fn gen_anon_chip_top(&mut self) {
        // top-level anon chip sharing parent scope: declare a var inside
        let v = self.fresh("v");
        let ty = *self.rng.pick(&[Ty::Int, Ty::Float]);
        let init = self.lit(ty);
        let ann = if self.rng.chance(1, 4) { "@closed " } else { "" };
        self.blocks.push(format!(
            "{ann}chip {{\n  var {v}: {} = {init}\n}}",
            ty.name()
        ));
        self.scalars.push(Sca { name: v, ty, acc: Acc::Var });
    }

    /// Top-level shadowing declarations exploiting the WS013 escape (`let` /
    /// `out` are not duplicate-checked against `var` / `in`): a `let`
    /// re-binding a var's name (later references silently read the constant,
    /// the storage keeps existing), and a `let` re-binding an exec input's
    /// name (an `on <name>` handler then binds the CONSTANT - its exec input
    /// is silently dead).
    fn gen_top_shadows(&mut self) {
        if self.rng.chance(1, 6) {
            let vars: Vec<Sca> = self
                .scalars
                .iter()
                .filter(|s| s.acc == Acc::Var && s.ty == Ty::Int && !s.name.contains('.'))
                .cloned()
                .collect();
            if !vars.is_empty() {
                let v = self.rng.pick(&vars).clone();
                let lit = self.lit(Ty::Int);
                self.blocks.push(format!("let {} = {lit}", v.name));
                for s in self.scalars.iter_mut().filter(|s| s.name == v.name) {
                    s.acc = Acc::Direct;
                }
            }
        }
        if self.rng.chance(1, 8) && !self.execs.is_empty() {
            let e = self.rng.pick(&self.execs).clone();
            let lit = self.lit(Ty::Int);
            self.blocks.push(format!("let {e} = {lit}"));
            // deliberately KEPT in `execs`: a later `on <e>` handler then
            // exercises the trigger-binds-to-constant path.
        }
    }

    fn generate_body(&mut self) {
        for _ in 0..self.rng.range(0, 2) {
            self.gen_type_alias();
        }
        self.gen_inputs();
        self.gen_vars();
        if self.rng.chance(1, 3) {
            self.gen_anon_chip_top();
        }
        self.gen_buffers();
        self.gen_record_binds();
        for _ in 0..self.rng.range(0, 2) {
            self.gen_mod();
        }
        for _ in 0..self.rng.range(0, 2) {
            self.gen_chip();
        }
        self.gen_top_lets();
        self.gen_top_shadows();
        self.gen_signals_and_typed_outs();
        for _ in 0..self.rng.range(1, 3) {
            self.gen_handler();
        }
        self.gen_outs();
    }

    fn generate(mut self) -> String {
        self.generate_body();
        self.blocks.join("\n")
    }

    /// Entry generator for multi-file programs: `prelude` (import lines + any
    /// deliberate local re-declarations) goes first, then the normal grammar
    /// - which also reaches every symbol the caller injected into the tables
    /// beforehand (bare names for `import "m"`, `N.x` paths for
    /// `import * as N`).
    fn generate_entry(mut self, prelude: Vec<String>) -> String {
        self.blocks = prelude;
        self.generate_body();
        self.blocks.join("\n")
    }

    /// A small importable module: top-level state (+ optionally mods, a chip,
    /// and an in-port + handler writing its own state). Returns the module
    /// source and what it exports.
    fn generate_module(mut self, with_handler: bool) -> (String, ModuleExports) {
        if self.rng.chance(1, 3) {
            self.gen_type_alias();
        }
        self.gen_vars();
        let vars_end = self.scalars.len();
        self.gen_buffers();
        let buffers_end = self.scalars.len();
        for _ in 0..self.rng.range(0, 2) {
            self.gen_mod();
        }
        if self.rng.chance(1, 3) {
            self.gen_chip();
        }
        if with_handler {
            self.gen_inputs();
            self.gen_handler();
        }
        // Record-typed signatures reference the module's private type
        // aliases; the entry can't construct those args, so only export
        // scalar-shaped mods/chips.
        let scalar_sig = |params: &[(String, PTy)]| {
            params
                .iter()
                .all(|(_, p)| matches!(p, PTy::Val(_) | PTy::Ref(_) | PTy::Exec))
        };
        let exports = ModuleExports {
            vars: self.scalars[..vars_end].to_vec(),
            buffers: self.scalars[vars_end..buffers_end].to_vec(),
            ports: self.scalars[buffers_end..].to_vec(),
            arrays: self.arrays.clone(),
            mods: self
                .mods
                .iter()
                .filter(|m| scalar_sig(&m.params) && !matches!(m.ret, Some(RetTy::Rec(_) | RetTy::Multi(_))))
                .cloned()
                .collect(),
            chips: self
                .chips
                .iter()
                .filter(|c| scalar_sig(&c.params))
                .cloned()
                .collect(),
        };
        (self.blocks.join("\n"), exports)
    }
}

#[derive(Clone)]
struct ModuleExports {
    /// top-level `var`s (Acc::Var)
    vars: Vec<Sca>,
    /// top-level buffers (Acc::Direct - pure-readable by bare name)
    buffers: Vec<Sca>,
    /// value in-ports declared by the module (only meaningful bare)
    ports: Vec<Sca>,
    arrays: Vec<(String, Ty, bool)>,
    mods: Vec<ModSig>,
    chips: Vec<ChipSig>,
}

/// ~5% of programs: mutate a valid program into (probable) garbage as a
/// parser/pipeline crash check.
fn mutate_garbage(rng: &mut Rng, src: &str) -> String {
    let mut lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    let ops = rng.range(1, 3);
    for _ in 0..ops {
        if lines.is_empty() {
            break;
        }
        match rng.below(6) {
            0 => {
                let i = rng.below(lines.len());
                lines.remove(i);
            }
            1 => {
                let i = rng.below(lines.len());
                let l = lines[i].clone();
                lines.insert(i, l);
            }
            2 => {
                let i = rng.below(lines.len());
                let tok = *rng.pick(&["}", "{", "..", "->", "@left", "emit", "((", "= =", "then"]);
                let line = lines[i].clone();
                let chars: Vec<char> = line.chars().collect();
                let pos = rng.below(chars.len() + 1);
                let mut nl: String = chars[..pos].iter().collect();
                nl.push_str(tok);
                let rest: String = chars[pos..].iter().collect();
                nl.push_str(&rest);
                lines[i] = nl;
            }
            3 => {
                let i = rng.below(lines.len());
                let j = rng.below(lines.len());
                lines.swap(i, j);
            }
            4 => {
                let i = rng.below(lines.len());
                let line = lines[i].clone();
                let chars: Vec<char> = line.chars().collect();
                if chars.len() > 2 {
                    let cut = rng.below(chars.len());
                    lines[i] = chars[..cut].iter().collect();
                }
            }
            _ => {
                let i = rng.below(lines.len());
                lines[i] = format!("{} }}", lines[i]);
            }
        }
    }
    lines.join("\n")
}

// --------------------------- multi-file program generators ---------------------------

/// Full-grammar multi-file mode: 1-3 modules (top-level state, mods, chips,
/// sometimes an in-port + handler writing the module's own state), imported
/// via `import "m"` or `import * as N from "m"`, with DELIBERATE collisions:
/// later modules re-declare a state name of module 1 (same or different
/// storage kind), and the entry sometimes re-declares a namespace-imported
/// name locally. The entry is a normal full-grammar program whose symbol
/// tables were seeded with the imported state, so its statements read/write
/// cross-module state through both bare and `N.x` paths.
fn gen_import_program(seed: u64) -> String {
    let mut rng = Rng::new(seed ^ 0x17B7_A2E5);
    let n_mods = rng.range(1, 3);

    struct ModPlan {
        src: String,
        exports: ModuleExports,
        namespaced: bool,
        alias: String,
    }
    let mut plans: Vec<ModPlan> = Vec::new();
    for i in 0..n_mods {
        let mut mg = Gen::new(seed ^ (0x51ED_2F0B + i as u64 * 7919));
        mg.n = (i as u32) * 40;
        let with_handler = rng.chance(1, 3);
        let (mut src, mut exports) = mg.generate_module(with_handler);
        // deliberate cross-module collision: re-declare a state name of
        // module 0, same kind or a different one (the storage-identity bugs
        // ignore the type too)
        if i > 0 && rng.chance(1, 2) {
            let mut names: Vec<String> = Vec::new();
            names.extend(plans[0].exports.vars.iter().map(|s| s.name.clone()));
            names.extend(plans[0].exports.arrays.iter().map(|(n, _, _)| n.clone()));
            names.extend(plans[0].exports.buffers.iter().map(|s| s.name.clone()));
            if !names.is_empty() {
                let name = rng.pick(&names).clone();
                match rng.below(3) {
                    0 => {
                        src.push_str(&format!("\nvar {name}: int = {}", rng.below(9)));
                        exports.vars.push(Sca { name, ty: Ty::Int, acc: Acc::Var });
                    }
                    1 => {
                        src.push_str(&format!("\nvar {name}: int[]"));
                        exports.arrays.push((name, Ty::Int, true));
                    }
                    _ => {
                        src.push_str(&format!("\nbuffer {name} = {}", rng.below(9)));
                        exports.buffers.push(Sca { name, ty: Ty::Int, acc: Acc::Direct });
                    }
                }
            }
        }
        let namespaced = rng.chance(1, 2);
        plans.push(ModPlan {
            src,
            exports,
            namespaced,
            alias: format!("N{}", i + 1),
        });
    }

    let mut prelude: Vec<String> = Vec::new();
    let mut eg = Gen::new(seed ^ 0x00E7_7A11);
    eg.n = 200;
    let mut bare_names: BTreeSet<String> = BTreeSet::new();
    for (i, p) in plans.iter().enumerate() {
        let file = format!("m{}", i + 1);
        if p.namespaced {
            prelude.push(format!("import * as {} from \"{file}\"", p.alias));
            for s in &p.exports.vars {
                eg.ns_scalars.push(Sca {
                    name: format!("{}.{}", p.alias, s.name),
                    ty: s.ty,
                    acc: Acc::Var,
                });
            }
            for (n, t, w) in &p.exports.arrays {
                eg.ns_arrays.push((format!("{}.{n}", p.alias), *t, *w));
            }
            // exec-only mods: a namespaced PURE call's result types as `any`
            // today (unusable in typed expressions), so only statement-position
            // exec mods go through the `N.m(..)` path.
            for m in &p.exports.mods {
                if m.ret.is_none() && !m.pure_call {
                    let mut m2 = m.clone();
                    m2.name = format!("{}.{}", p.alias, m.name);
                    m2.generic = false;
                    eg.mods.push(m2);
                }
            }
        } else {
            prelude.push(format!("import \"{file}\""));
            for s in p
                .exports
                .vars
                .iter()
                .chain(p.exports.buffers.iter())
                .chain(p.exports.ports.iter())
            {
                bare_names.insert(s.name.clone());
                eg.scalars.push(s.clone());
            }
            for a in &p.exports.arrays {
                bare_names.insert(a.0.clone());
                eg.arrays.push(a.clone());
            }
            eg.mods.extend(p.exports.mods.iter().cloned());
            eg.chips.extend(p.exports.chips.iter().cloned());
        }
    }
    // entry-local re-declaration of a namespace-imported name (silent today:
    // the module's same-named state never gets its own gate). Only names NOT
    // also reachable bare - a bare duplicate is a WS013 hard error.
    if rng.chance(1, 3) {
        let mut cands: Vec<String> = Vec::new();
        for p in plans.iter().filter(|p| p.namespaced) {
            cands.extend(
                p.exports
                    .vars
                    .iter()
                    .filter(|s| s.ty == Ty::Int)
                    .map(|s| s.name.clone()),
            );
        }
        cands.retain(|n| !bare_names.contains(n));
        if !cands.is_empty() {
            let name = rng.pick(&cands).clone();
            prelude.push(format!("var {name}: int = {}", rng.below(9)));
            eg.scalars.push(Sca { name, ty: Ty::Int, acc: Acc::Var });
        }
    }

    let entry = eg.generate_entry(prelude);
    let mut out = String::new();
    for (i, p) in plans.iter().enumerate() {
        let _ = writeln!(out, "{FILE_MARKER}m{}", i + 1);
        let _ = writeln!(out, "{}", p.src);
    }
    let _ = writeln!(out, "{FILE_MARKER}main");
    out.push_str(&entry);
    out
}

/// Canary mode: SMALL, fully-controlled multi-file programs whose storage
/// expectations are exactly derivable from the source (`derive_storage_expect`)
/// - every declared state gets a read and usually a write through its own
/// access path. Collision shapes generated: same name in two modules (any mix
/// of import kinds and storage kinds), a local decl vs a namespaced import,
/// same file imported twice (the KNOWN-GOOD sharing case), and writes routed
/// through a ref-param mod (`refw`).
fn gen_import_canary(seed: u64) -> String {
    let mut r = Rng::new(seed ^ 0xCA7A_B15D);
    const POOL: [&str; 6] = ["g", "cnt", "dat", "xs", "mm", "qq"];
    const KINDS: [StoreKind; 5] = [
        StoreKind::Var,
        StoreKind::Var,
        StoreKind::Array,
        StoreKind::Map,
        StoreKind::Buffer,
    ];

    #[derive(Clone)]
    struct CItem {
        kind: StoreKind,
        name: String,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Imp {
        All,
        Ns,
        Named,
    }

    // A record CONTAINER declared as importable module state: `var name: T`
    // / `var name: T[]` / `var name: Map<int, T>` where `T` is a module-
    // private `type T = { a: int, b: int }` alias declared alongside it (in
    // the SAME file - record types don't cross the import boundary, only
    // field/method access on the container does). Always exactly two `int`
    // fields named `a`/`b` - the canary stays small and fully controlled;
    // the interesting axis under test is the import/namespace boundary, not
    // field-type diversity (the general-purpose fuzzer already exercises
    // record field-type variety in pure `let`-bound contexts).
    #[derive(Clone, Copy, PartialEq)]
    enum RecKind {
        Var,
        Array,
        Map,
    }
    #[derive(Clone)]
    struct RecItem {
        kind: RecKind,
        name: String,
        type_name: String,
    }

    let n_mods = r.range(1, 3);
    let mut modules: Vec<Vec<CItem>> = Vec::new();
    let mut rec_items: Vec<Vec<RecItem>> = Vec::new();
    // file index each module reads from (same-file re-import = known-good)
    let mut file_of: Vec<usize> = Vec::new();
    let mut imps: Vec<Imp> = Vec::new();
    for i in 0..n_mods {
        if i > 0 && r.chance(1, 4) {
            // re-import module 0's FILE under a second namespace: shared
            // state on purpose - must stay ONE gate and stay silent.
            modules.push(modules[0].clone());
            rec_items.push(rec_items[0].clone());
            file_of.push(0);
            imps.push(Imp::Ns);
            continue;
        }
        let mut items: Vec<CItem> = Vec::new();
        let n_items = r.range(1, 3);
        for _ in 0..n_items {
            let name = if i > 0 && r.chance(1, 2) && !modules[0].is_empty() {
                // collide with a module-0 state name
                let src = r.pick(&modules[0]).clone();
                src.name
            } else {
                let mut cand = (*r.pick(&POOL)).to_string();
                let mut guard = 0;
                while items.iter().any(|it| it.name == cand) && guard < 8 {
                    cand = (*r.pick(&POOL)).to_string();
                    guard += 1;
                }
                cand
            };
            if items.iter().any(|it| it.name == name) {
                continue;
            }
            items.push(CItem { kind: *r.pick(&KINDS), name });
        }
        modules.push(items);
        // ~half the modules also carry a record container - occasionally
        // (like the scalar items above) reusing module 0's container NAME
        // under a freshly re-rolled kind, to stress the same
        // storage-collapse-on-collision shape the scalar canary already
        // covers, but for per-field record backing.
        let mut ritems: Vec<RecItem> = Vec::new();
        if r.chance(1, 2) {
            let name = if i > 0 && r.chance(1, 3) && !rec_items[0].is_empty() {
                r.pick(&rec_items[0]).name.clone()
            } else {
                format!("rc{i}")
            };
            let kind = *r.pick(&[RecKind::Var, RecKind::Array, RecKind::Map]);
            ritems.push(RecItem { kind, name, type_name: format!("Rec{}", i + 1) });
        }
        rec_items.push(ritems);
        file_of.push(i);
        imps.push(match r.below(3) {
            0 => Imp::All,
            1 => Imp::Ns,
            _ => Imp::Named,
        });
    }

    // names reachable bare (All / Named imports); a local re-declaration of
    // one of those is a WS013 hard error, so the local collision may only
    // target names that are namespace-only.
    let mut bare_names: BTreeSet<String> = BTreeSet::new();
    let mut ns_only: Vec<String> = Vec::new();
    for (mi, items) in modules.iter().enumerate() {
        for it in items {
            if imps[mi] == Imp::Ns {
                ns_only.push(it.name.clone());
            } else {
                bare_names.insert(it.name.clone());
            }
        }
    }
    ns_only.retain(|n| !bare_names.contains(n) && n != "acc");
    let local_collision = if !ns_only.is_empty() && r.chance(1, 3) {
        Some(r.pick(&ns_only).clone())
    } else {
        None
    };

    // module sections (deduped by file)
    let mut out = String::new();
    let mut emitted_files: BTreeSet<usize> = BTreeSet::new();
    for (mi, items) in modules.iter().enumerate() {
        let fi = file_of[mi];
        if !emitted_files.insert(fi) {
            continue;
        }
        let _ = writeln!(out, "{FILE_MARKER}m{}", fi + 1);
        for it in items {
            match it.kind {
                StoreKind::Var => {
                    let _ = writeln!(out, "var {}: int = {}", it.name, r.below(9));
                }
                StoreKind::Array => {
                    let _ = writeln!(out, "var {}: int[]", it.name);
                }
                StoreKind::Map => {
                    let _ = writeln!(out, "var {}: Map<int, int>", it.name);
                }
                StoreKind::Buffer => {
                    let _ = writeln!(out, "buffer {} = {}", it.name, r.below(9));
                }
            }
        }
        for rit in &rec_items[mi] {
            let _ = writeln!(out, "type {} = {{ a: int, b: int }}", rit.type_name);
            match rit.kind {
                RecKind::Var => {
                    let _ = writeln!(out, "var {}: {}", rit.name, rit.type_name);
                }
                RecKind::Array => {
                    let _ = writeln!(out, "var {}: {}[]", rit.name, rit.type_name);
                }
                RecKind::Map => {
                    let _ = writeln!(out, "var {}: Map<int, {}>", rit.name, rit.type_name);
                }
            }
        }
    }

    // entry
    let mut entry: Vec<String> = Vec::new();
    for (mi, items) in modules.iter().enumerate() {
        let file = format!("m{}", file_of[mi] + 1);
        match imps[mi] {
            Imp::All => entry.push(format!("import \"{file}\"")),
            Imp::Ns => entry.push(format!("import * as A{} from \"{file}\"", mi + 1)),
            Imp::Named => {
                let mut names: Vec<&str> = items.iter().map(|it| it.name.as_str()).collect();
                names.extend(rec_items[mi].iter().map(|it| it.name.as_str()));
                entry.push(format!("import {{ {} }} from \"{file}\"", names.join(", ")));
            }
        }
    }
    let mut use_refw = false;
    let mut body: Vec<String> = Vec::new();
    let mut int_reads: Vec<String> = Vec::new();
    let mut rn = 0usize;
    let mut seen_lines: BTreeSet<String> = BTreeSet::new();
    let mut buffer_outs: Vec<String> = Vec::new();
    {
        let mut push_line = |body: &mut Vec<String>, line: String| {
            if seen_lines.insert(line.clone()) {
                body.push(line);
            }
        };
        let mut emit_item = |it: &CItem,
                             path: &str,
                             bare: bool,
                             body: &mut Vec<String>,
                             int_reads: &mut Vec<String>,
                             buffer_outs: &mut Vec<String>,
                             r: &mut Rng,
                             rn: &mut usize,
                             use_refw: &mut bool| {
            match it.kind {
                StoreKind::Var => {
                    *rn += 1;
                    let rv = format!("r{rn}");
                    push_line(body, format!("  let {rv} = {path}"));
                    int_reads.push(rv);
                    if r.chance(2, 3) {
                        match r.below(4) {
                            0 => push_line(body, format!("  {path} += {}", r.range(1, 5))),
                            1 => {
                                *use_refw = true;
                                push_line(body, format!("  refw({path})"));
                            }
                            _ => push_line(
                                body,
                                format!("  {path} = {path} + {}", r.range(1, 5)),
                            ),
                        }
                    }
                }
                StoreKind::Array => {
                    if r.chance(2, 3) {
                        if r.chance(1, 3) {
                            push_line(body, format!("  {path}[0] = {}", r.below(9)));
                        } else {
                            push_line(body, format!("  {path}.push({})", r.below(9)));
                        }
                    }
                    *rn += 1;
                    let rv = format!("r{rn}");
                    push_line(body, format!("  let {rv} = {path}.length()"));
                    // a NAMESPACED array method result types as `any` today,
                    // so only bare-path lengths feed the int accumulator
                    if bare {
                        int_reads.push(rv);
                    }
                }
                StoreKind::Map => {
                    if r.chance(2, 3) {
                        push_line(
                            body,
                            format!("  {path}.set({}, {})", r.below(5), r.below(9)),
                        );
                    }
                    *rn += 1;
                    push_line(body, format!("  let r{rn} = {path}.get({})", r.below(5)));
                }
                StoreKind::Buffer => {
                    // buffers are pure-readable by bare name only; a
                    // namespaced buffer stays declare-only (its gate still
                    // must exist - the count check covers it).
                    if bare {
                        buffer_outs.push(path.to_string());
                    }
                }
            }
        };

        for (mi, items) in modules.iter().enumerate() {
            for it in items {
                let (path, bare) = match imps[mi] {
                    Imp::Ns => (format!("A{}.{}", mi + 1, it.name), false),
                    _ => (it.name.clone(), true),
                };
                emit_item(
                    it,
                    &path,
                    bare,
                    &mut body,
                    &mut int_reads,
                    &mut buffer_outs,
                    &mut r,
                    &mut rn,
                    &mut use_refw,
                );
            }
        }
        if let Some(name) = &local_collision {
            let it = CItem { kind: StoreKind::Var, name: name.clone() };
            emit_item(
                &it,
                name,
                true,
                &mut body,
                &mut int_reads,
                &mut buffer_outs,
                &mut r,
                &mut rn,
                &mut use_refw,
            );
        }
    }

    // Record-container access. Ops written directly to `body` (not through
    // `push_line`'s dedup - out of scope above, and unneeded: unlike the
    // scalar collision path, nothing re-visits the same (mi, name) pair) -
    // field access/write, whole-record assign, array push + index read +
    // whole-element write, map set + index-field read. All verified against
    // the real compiler in both bare and namespaced form before wiring in;
    // the one namespaced pothole (`.get(k).a` on a MAP typing its result as
    // `Any`, the same gap as a namespaced array's `.length()`) is gated
    // behind `bare` below rather than "fixed" here - it's the pre-existing
    // namespaced-method-call limitation, not a record bug.
    for (mi, ritems) in rec_items.iter().enumerate() {
        for rit in ritems {
            let (path, bare) = match imps[mi] {
                Imp::Ns => (format!("A{}.{}", mi + 1, rit.name), false),
                _ => (rit.name.clone(), true),
            };
            match rit.kind {
                RecKind::Var => {
                    body.push(format!(
                        "  {path} = {{ a: {}, b: {} }}",
                        r.below(9),
                        r.below(9)
                    ));
                    rn += 1;
                    let rv = format!("r{rn}");
                    body.push(format!("  let {rv} = {path}.a"));
                    int_reads.push(rv);
                    if r.chance(2, 3) {
                        body.push(format!("  {path}.a = {path}.a + {}", r.range(1, 5)));
                    }
                }
                RecKind::Array => {
                    body.push(format!(
                        "  {path}.push({{ a: {}, b: {} }})",
                        r.below(9),
                        r.below(9)
                    ));
                    rn += 1;
                    let rv = format!("r{rn}");
                    body.push(format!("  let {rv} = {path}[0].a"));
                    int_reads.push(rv);
                    if r.chance(1, 2) {
                        body.push(format!(
                            "  {path}[0] = {{ a: {}, b: {} }}",
                            r.below(9),
                            r.below(9)
                        ));
                    }
                }
                RecKind::Map => {
                    let key = r.below(5);
                    body.push(format!(
                        "  {path}.set({key}, {{ a: {}, b: {} }})",
                        r.below(9),
                        r.below(9)
                    ));
                    rn += 1;
                    let rv = format!("r{rn}");
                    body.push(format!("  let {rv} = {path}[{key}].a"));
                    int_reads.push(rv);
                    if bare {
                        rn += 1;
                        let rv2 = format!("r{rn}");
                        body.push(format!("  let {rv2} = {path}.get({key}).a"));
                        int_reads.push(rv2);
                    }
                }
            }
        }
    }

    if use_refw {
        entry.push(format!(
            "mod refw(v: *int) {{\n  v = v + {}\n}}",
            r.range(1, 3)
        ));
    }
    if let Some(name) = &local_collision {
        entry.push(format!("var {name}: int = {}", r.below(9)));
    }
    entry.push("in t: exec".to_string());
    entry.push("var acc: int = 0".to_string());
    let sum = if int_reads.is_empty() {
        "1".to_string()
    } else {
        int_reads.join(" + ")
    };
    body.push(format!("  acc = acc + {sum}"));
    entry.push(format!("on t {{\n{}\n}}", body.join("\n")));
    for (i, b) in buffer_outs.iter().enumerate() {
        entry.push(format!("out ob{i}: int = {b}"));
    }
    entry.push("out o: int = acc.Value".to_string());

    let _ = writeln!(out, "{FILE_MARKER}main");
    out.push_str(&entry.join("\n"));
    out
}

// ─────────────────────────── minimization ───────────────────────────

fn oracle_holds(src: &str, kind: Kind, bucket: &str) -> bool {
    let out = run_pipeline(src);
    out.findings().iter().any(|(k, b)| *k == kind && b == bucket)
}

fn minimize(src: &str, kind: Kind, bucket: &str, max_runs: usize) -> String {
    let mut lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    let mut runs = 0usize;
    loop {
        let before = lines.len();
        let mut chunk = (lines.len() / 2).max(1);
        while chunk >= 1 {
            let mut i = 0;
            while i < lines.len() {
                if lines.len() <= 1 || runs >= max_runs {
                    break;
                }
                let end = (i + chunk).min(lines.len());
                let mut cand = lines.clone();
                cand.drain(i..end);
                runs += 1;
                if !cand.is_empty() && oracle_holds(&cand.join("\n"), kind, bucket) {
                    lines = cand;
                } else {
                    i = end;
                }
            }
            if chunk == 1 {
                break;
            }
            chunk /= 2;
        }
        if lines.len() == before || runs >= max_runs {
            break;
        }
    }
    lines.join("\n")
}

// ─────────────────────────── findings + buckets ───────────────────────────

struct Finding {
    kind: Kind,
    bucket: String,
    program: String,
    seed: u64,
    index: usize,
    diags: Vec<String>,
    warn_only: bool,
    no_diags: bool,
    raw_detail: String,
}

// ─────────────────────────── selftest / calibrate ───────────────────────────

fn selftest() -> bool {
    let mut ok = true;

    // 1. _Unsupported detection: pure-context array index read. (Note:
    // typecheck WS007-errors this, so it is NOT a silent-miscompile finding
    // — but the lowered module must still contain the placeholder, proving
    // the scanner works.)
    let src = "var items: int[]\nlet x = items[0]\nout r = x";
    let out = run_pipeline(src);
    if out.unsupported.is_empty() {
        eprintln!("[selftest] FAIL: pure array read produced no _Unsupported node");
        ok = false;
    } else {
        eprintln!(
            "[selftest] ok: _Unsupported detected for pure array read (errors={}, warns={}, key={})",
            out.error_diags.len(),
            out.warn_diags.len(),
            out.unsupported[0].0
        );
    }

    // 2. wire fan-in/dangling detection on a synthetic module.
    {
        use wirescript::ir::{GateIO, Node, NodeKind, PortRef, Wire};
        let mut m = Module::new("selftest");
        let mk = |id: u32| Node {
            id: NodeId(1_000_000 + id),
            kind: NodeKind::Gate,
            gate_class: "BrickComponentType_WireGraph_Expr_MathAdd",
            properties: Arc::new(Default::default()),
            ports: Arc::new(GateIO::default()),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 0,
            note: None,
        };
        let a = m.add_node(mk(1));
        let b = m.add_node(mk(2));
        let c = m.add_node(mk(3));
        let w = |s: NodeId, t: NodeId, p: WirePort| Wire {
            source: PortRef { node_id: s, port: WirePort::Output },
            target: PortRef { node_id: t, port: p },
        };
        m.add_wire(w(a, c, WirePort::InputA)); // fan-in pair
        m.add_wire(w(b, c, WirePort::InputA));
        m.add_wire(w(a, c, WirePort::InputB));
        m.add_wire(w(a, c, WirePort::InputB)); // exact duplicate
        m.add_wire(w(a, NodeId(9_999_999), WirePort::InputA)); // dangling target
        let mut o = Outcome::default();
        check_wires(&m, &mut o);
        let kinds: BTreeSet<Kind> = o.wire_issues.iter().map(|(k, _)| *k).collect();
        for want in [Kind::WireFanIn, Kind::WireDup, Kind::WireDangling] {
            if !kinds.contains(&want) {
                eprintln!("[selftest] FAIL: wire check missed {want:?}");
                ok = false;
            }
        }
        if ok {
            eprintln!("[selftest] ok: wire checker detects fan-in, duplicates, dangling");
        }
    }

    // 3. clean program stays clean.
    let clean = "in t1: exec\nvar v1: int = 0\non t1 {\n  v1 = v1 + 1\n}\nout o1: int = v1.Value";
    let out = run_pipeline(clean);
    let f = out.findings();
    if !f.is_empty() || out.has_errors() {
        eprintln!(
            "[selftest] FAIL: clean program produced findings {:?} / errors {:?}",
            f, out.error_diags
        );
        ok = false;
    } else {
        eprintln!("[selftest] ok: clean program produces no findings");
    }

    // 4. KNOWN-GOOD storage canary: distinct state across two modules, plus
    // the same file imported under TWO namespaces (deliberately-shared state
    // must stay ONE gate and must NOT fire the oracle).
    let good = "\
//!ws-file mg1
var g: int = 1
var xs: int[]
var mm: Map<int, int>
buffer bb = 3
//!ws-file mg2
var cnt: int = 2
//!ws-file main
import * as A from \"mg1\"
import * as B from \"mg1\"
import \"mg2\"
in t: exec
var acc: int = 0
on t {
  let r1 = A.g
  A.g = A.g + 1
  A.xs.push(1)
  let r2 = B.xs.length()
  A.mm.set(1, 2)
  let r3 = B.mm.get(1)
  cnt = cnt + 1
  let r4 = cnt
  acc = acc + r1 + r4
}
out o: int = acc.Value";
    let out = run_pipeline(good);
    let f = out.findings();
    if out.has_errors() || !f.is_empty() {
        eprintln!(
            "[selftest] FAIL: known-good storage canary fired: findings {:?} / errors {:?}",
            f, out.error_diags
        );
        ok = false;
    } else {
        eprintln!("[selftest] ok: known-good storage canary is silent (shared state stays one gate)");
    }

    // 5. REGRESSION GUARD: two modules each declaring `var g`, imported as
    // two namespaces, used to collapse into ONE storage gate so writing A.g
    // changed B.g. The namespace direct-state fix makes them distinct, so
    // the oracle must now stay SILENT; a storage-collapse finding here means
    // the bug regressed.
    let bad = "\
//!ws-file mb1
var g: int = 1
//!ws-file mb2
var g: int = 2
//!ws-file main
import * as A from \"mb1\"
import * as B from \"mb2\"
in t: exec
var acc: int = 0
on t {
  let r1 = A.g
  let r2 = B.g
  A.g = A.g + 1
  B.g = B.g + 2
  acc = acc + r1 + r2
}
out o: int = acc.Value";
    let out = run_pipeline(bad);
    let fired = out
        .findings()
        .iter()
        .any(|(k, _)| *k == Kind::StorageCollapse);
    if out.has_errors() {
        eprintln!(
            "[selftest] FAIL: two-namespace `var g` canary rejected with errors {:?}",
            out.error_diags
        );
        ok = false;
    } else if fired {
        eprintln!(
            "[selftest] FAIL: REGRESSION - two-namespace `var g` collapsed to one gate again (findings: {:?})",
            out.findings()
        );
        ok = false;
    } else {
        eprintln!("[selftest] ok: two-namespace `var g` stays distinct (namespace direct-state fix holds)");
    }

    // 6. REGRESSION GUARD: a namespace-imported module's `on` handler used
    // to be silently dropped, taking its
    // `wv = wv + 1` write with it. The namespaced-handler fix runs it, so the
    // write's mutation gate now exists and the oracle must stay SILENT; a
    // dropped-write finding here means the handler was dropped again.
    let badw = "\
//!ws-file mw1
var wv: int = 0
in tw: exec
on tw {
  wv = wv + 1
}
//!ws-file main
import * as W from \"mw1\"
in t: exec
var acc: int = 0
on t {
  let r1 = W.wv
  acc = acc + r1
}
out o: int = acc.Value";
    let out = run_pipeline(badw);
    let fired = out.findings().iter().any(|(k, _)| *k == Kind::DroppedWrite);
    if out.has_errors() {
        eprintln!(
            "[selftest] FAIL: ns-imported handler canary rejected with errors {:?}",
            out.error_diags
        );
        ok = false;
    } else if fired {
        eprintln!(
            "[selftest] FAIL: REGRESSION - ns-imported handler write dropped again (findings: {:?})",
            out.findings()
        );
        ok = false;
    } else {
        eprintln!(
            "[selftest] ok: ns-imported handler write survives (namespaced-handler fix holds)"
        );
    }

    // 7. REGRESSION GUARD: an OPERATOR (or sibling call) inside a namespaced
    // module's `on` handler must lower to a real gate. Namespaced handlers began
    // LOWERING in 1.4.4, but typecheck did not descend into their bodies, so
    // `n << 10` got no `op_resolutions` and lowered to `_Unsupported` (fixed by
    // checking Handler/AnonChip bodies in the namespace arm). The oracle must
    // stay SILENT; an unsupported-gate finding means a namespaced handler body
    // is no longer type-checked.
    let nsop = "\
//!ws-file ho1
let n = 3
var arr: int[]
on ReadBrickGrid() {
  arr.push((n << 10) + n)
}
//!ws-file main
import * as H from \"ho1\"";
    let out = run_pipeline(nsop);
    let fired = out.findings().iter().any(|(k, _)| *k == Kind::Unsupported);
    if out.has_errors() {
        eprintln!(
            "[selftest] FAIL: ns-handler-operator canary rejected with errors {:?}",
            out.error_diags
        );
        ok = false;
    } else if fired {
        eprintln!(
            "[selftest] FAIL: REGRESSION - operator in a namespaced handler lowered to _Unsupported (findings: {:?})",
            out.findings()
        );
        ok = false;
    } else {
        eprintln!(
            "[selftest] ok: operator in a namespaced handler lowers to a gate (namespaced-handler type-check holds)"
        );
    }

    // 8. KNOWN-GOOD: record-typed CONTAINERS (var/array/map) declared in an
    // imported module and driven entirely through a namespace alias - field
    // access/write, whole-record assign, array push + index-field read +
    // whole-element write, map set + index-field read. Every shape here was
    // hand-verified against the real compiler before being wired into the
    // generator; the oracle must stay SILENT (no _Unsupported placeholder,
    // no dropped-write, no storage-collapse across the per-field backing
    // gates `record_field_storage` decomposes each container into).
    let rec = "\
//!ws-file mr1
type Rec = { a: int, b: int }
var rv: Rec
var ra: Rec[]
var rm: Map<int, Rec>
//!ws-file main
import * as R from \"mr1\"
in t: exec
var acc: int = 0
on t {
  R.rv = { a: 1, b: 2 }
  R.rv.a = R.rv.a + 3
  let r1 = R.rv.a
  R.ra.push({ a: 3, b: 4 })
  R.ra[0] = { a: 9, b: 9 }
  let r2 = R.ra[0].a
  R.rm.set(1, { a: 5, b: 6 })
  let r3 = R.rm[1].a
  acc = acc + r1 + r2 + r3
}
out o: int = acc.Value";
    let out = run_pipeline(rec);
    let f = out.findings();
    if out.has_errors() || !f.is_empty() {
        eprintln!(
            "[selftest] FAIL: namespaced record-container canary fired: findings {:?} / errors {:?}",
            f, out.error_diags
        );
        ok = false;
    } else {
        eprintln!(
            "[selftest] ok: namespaced record var/array/map (field rw, whole-record assign, push/set) is silent"
        );
    }
    ok
}

fn calibrate(dir: &str) {
    let mut n = 0;
    let mut hits = 0;
    for entry in std::fs::read_dir(dir).expect("read calibrate dir") {
        let p = entry.expect("entry").path();
        if p.extension().and_then(|e| e.to_str()) != Some("ws") {
            continue;
        }
        let src = match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(_) => continue,
        };
        n += 1;
        let out = run_pipeline(&src);
        let f = out.findings();
        if out.has_errors() {
            eprintln!("[calibrate] {} has ERRORS: {:?}", p.display(), &out.error_diags[..out.error_diags.len().min(3)]);
        }
        if !f.is_empty() {
            hits += 1;
            eprintln!("[calibrate] {} -> {} findings:", p.display(), f.len());
            for (k, b) in &f {
                eprintln!("[calibrate]     {} {}", k.name(), b);
            }
        }
    }
    eprintln!("[calibrate] {n} files, {hits} with oracle hits");
}

// ─────────────────────────── fold-diff mode ───────────────────────────
//
// Differential fuzzer for the certified constant-fold pass:
// compiles the SAME source twice — once with `FoldMode::ForceOff` (U, the
// "unfolded" module) and once with `FoldMode::ForceOn` (F, the "folded"
// module) — then:
//
//   1. `predict()` independently walks U's wire graph (seeding `_Literal`
//      nodes, passing values through MicrochipInput/Output chip-boundary
//      rerouters, and evaluating any other single-output gate via the
//      crate's own `fold::eval::eval` once its data inputs are known) to
//      predict which of U's root-level `out` ports the REAL fold pass
//      should be able to fold to a known value.
//   2. For every prediction, `trace_delivered()` walks F's wire graph from
//      the SAME `out` port (matched across the two independent compiles by
//      its declared name, since NodeIds are fresh per `lower()` call) to
//      determine what value F actually delivers there — a live `_Literal`
//      source, or a value inlined directly onto a boundary node's own
//      property (`inline_orphan_literals`), chasing through as many chip
//      hops as it takes.
//   3. Mismatch (or "delivers nothing") between the prediction and F's
//      actual delivery is a FINDING — the fold pass shorted a wire wrong,
//      dropped one, or folded through a barrier it shouldn't have.
//   4. Structural invariants on F: the existing `check_wires` oracle, plus
//      folded (nodes, wires) <= unfolded (nodes, wires) componentwise (the
//      fold pass may only ever remove/replace structure, never add it).
//
// `predict()` deliberately never re-implements the certified value laws —
// every arithmetic/comparison/logical result comes from `eval::eval` itself
// (the crate's own evaluator), so a refusal there is always "no prediction",
// never a wrong one. Only the WIRE-PROPAGATION walk (which nodes are
// literals, which pass through chip boundaries, which are fully resolved)
// is independently re-derived here — that walk is exactly what this test is
// trying to catch bugs in, so of course it can't be borrowed from the pass
// under test.

/// Recursively collect every node in `module` and its nested chips, keyed by
/// id — wires and chip-boundary hops can cross module boundaries, so both
/// the predictor and the delivery-tracer need a flat, tree-wide view.
fn collect_all_nodes<'a>(module: &'a Module, into: &mut StdMap<NodeId, &'a Node>) {
    for (id, n) in &module.nodes {
        into.insert(*id, n);
    }
    for child in module.chips.values() {
        collect_all_nodes(child, into);
    }
}

fn collect_all_wires(module: &Module, into: &mut Vec<Wire>) {
    into.extend(module.wires.iter().copied());
    for child in module.chips.values() {
        collect_all_wires(child, into);
    }
}

/// Total (nodes, wires) across `module` and every nested chip.
fn tree_counts(module: &Module) -> (usize, usize) {
    let mut nodes = 0usize;
    let mut wires = 0usize;
    walk_modules(module, &mut |m| {
        nodes += m.nodes.len();
        wires += m.wires.len();
    });
    (nodes, wires)
}

const MICROCHIP_INPUT: &str = "BrickComponentType_Internal_MicrochipInput";
const MICROCHIP_OUTPUT: &str = "BrickComponentType_Internal_MicrochipOutput";

/// Recognizes the "materialized/synthesized constant carrier" node shape the
/// ALWAYS-ON (not `no_fold`-gated) post-lowering passes use in place of a
/// bare `_Literal` wire whenever the consumer has no data struct to bake a
/// property into (`lower/expr.rs::literal_node`'s string special-case, and
/// `lower/mod.rs::materialize_unfoldable_constants`'s "dataless target"
/// path, e.g. `Opaque`'s Rerouter — or, just as often, a chip's own
/// MicrochipInput/Output: a plain literal crossing a chip boundary is
/// represented this way too, so `predict()`'s boundary pass-through and
/// `trace_delivered()`'s delivery trace both need to recognize it): a gate
/// with NO declared input ports at all (every "operand" is a baked
/// property — there is no port to wire) whose properties match one of the
/// three fixed identity recipes those passes hard-code (`N+0`, `B||false`,
/// `S..""`). None of these three shapes is a "certified operator" result —
/// `eval::eval` never sees a zero-input call, and `..` string concatenation
/// isn't in the certified set at all — so without this, the constant they
/// carry would be invisible to both the predictor and the delivery tracer.
fn literal_carrier_value(n: &Node) -> Option<FoldValue> {
    if !n.ports.inputs.is_empty() || n.ports.outputs.len() != 1 {
        return None;
    }
    if n.gate_class.ends_with("MathAdd") {
        return match (n.properties.get(&*sym::INPUT_A), n.properties.get(&*sym::INPUT_B)) {
            (Some(Literal::Int(a)), Some(Literal::Int(b))) if *b == 0 => Some(FoldValue::Int(*a)),
            (Some(Literal::Float(a)), Some(Literal::Float(b))) if *b == 0.0 => {
                Some(FoldValue::Float(*a))
            }
            _ => None,
        };
    }
    if n.gate_class.ends_with("LogicalOR") {
        return match (n.properties.get(&*sym::B_INPUT_A), n.properties.get(&*sym::B_INPUT_B)) {
            (Some(Literal::Bool(a)), Some(Literal::Bool(false))) => Some(FoldValue::Bool(*a)),
            _ => None,
        };
    }
    if n.gate_class == gc::STRING_CONCATENATE {
        let empty_or_absent = |lit: Option<&Literal>| match lit {
            None => true,
            Some(Literal::String(s)) => s.is_empty(),
            _ => false,
        };
        if empty_or_absent(n.properties.get(&*sym::INPUT_B))
            && empty_or_absent(n.properties.get(&intern("Separator")))
            && let Some(Literal::String(s)) = n.properties.get(&*sym::INPUT_A)
        {
            return Some(FoldValue::Str(s.clone()));
        }
        return None;
    }
    // Composite carrier recipes (`lower/mod.rs::materialize_unfoldable_constants`'s
    // `recipe` match + its dedicated `Quat` arm): a fully-baked Make{Vector,
    // Rotation,Color,Quaternion} gate with ZERO declared input ports (the
    // guard above already excludes any ORDINARY MakeVector/etc. call node,
    // which always declares its X/Y/Z(/W) ports even when their values are
    // baked-but-unwired — see `resolve_data_input`'s doc comment — so this
    // arm only ever matches the materialized-carrier shape). Field baked as
    // anything other than a plain `Literal::Float` never arises from that
    // recipe, so a non-`Float` property (or a missing one) refuses rather
    // than guesses.
    let f = |field: &str| match n.properties.get(&intern(field)) {
        Some(Literal::Float(v)) => Some(*v),
        _ => None,
    };
    if n.gate_class == gc::MAKE_VECTOR {
        return match (f("X"), f("Y"), f("Z")) {
            (Some(x), Some(y), Some(z)) => Some(FoldValue::Vector { x, y, z }),
            _ => None,
        };
    }
    if n.gate_class == gc::MAKE_ROTATION {
        return match (f("Pitch"), f("Yaw"), f("Roll")) {
            (Some(pitch), Some(yaw), Some(roll)) => Some(FoldValue::Rotator { pitch, yaw, roll }),
            _ => None,
        };
    }
    if n.gate_class == gc::MAKE_COLOR {
        return match (f("R"), f("G"), f("B"), f("A")) {
            (Some(r), Some(g), Some(b), Some(a)) => Some(FoldValue::Color { r, g, b, a }),
            _ => None,
        };
    }
    if n.gate_class == gc::MAKE_QUATERNION {
        return match (f("X"), f("Y"), f("Z"), f("W")) {
            (Some(x), Some(y), Some(z), Some(w)) => Some(FoldValue::Quat { x, y, z, w }),
            _ => None,
        };
    }
    None
}

/// `FormatText`'s substitution slots, `InputA..InputG`, in template-index
/// order — mirrors `fold/mod.rs::FORMAT_SLOTS` (private to that module, so
/// re-declared here identically).
const PREDICT_FORMAT_SLOTS: [WirePort; 7] = [
    WirePort::InputA, WirePort::InputB, WirePort::InputC,
    WirePort::InputD, WirePort::InputE, WirePort::InputF,
    WirePort::InputG,
];

/// Mirrors `fold/mod.rs::try_resolve_format_text`'s two-part law (can't call
/// it directly — it's private to the `fold` module and threads through that
/// module's own `Info`/`plan` types): the template lives in the
/// `FormatString` property (never a wire input), and at least one
/// substitution slot must be genuinely SUBSTITUTED before even attempting
/// the fold (a slot with no operand at all renders `"0"` and does NOT block
/// it — certified, see `eval::format_text`'s own doc comment).
///
/// Deliberately NOT identical to `try_resolve_format_text`, though: THAT
/// function reads `in_wires` built from `fold_certified_constants`'s OWN
/// pre-cleanup snapshot, where a literal `${...}` operand is still a real
/// wired `_Literal` source (`lower/ops.rs::lower_interp` always wires every
/// slot via `lower_expr`+`connect`, unlike a builtin CALL argument, which
/// can bake straight onto the consuming gate — see `resolve_data_input`'s
/// doc comment). But `predict()` here only ever sees `u_mod`, the module
/// `lower()` already returned — and the ALWAYS-ON (FoldMode-independent)
/// `inline_orphan_literals` lowering cleanup has, by then, already deleted
/// that wire+`_Literal` node and baked the value directly onto THIS node's
/// own property whenever `port_accepts_inline_variant` allows it (exactly
/// like a Math* gate's literal operand). So an unwired slot here ALSO needs
/// the baked-property fallback (`--fold-diff`-fuzzer-discovered:
/// `Length("v${Length(\"a\")}-${26}")`'s `${26}` slot mispredicted as
/// unsubstituted without it, even though production folds the whole
/// expression correctly — production's OWN driver never has this problem,
/// since it runs before that cleanup pass, so this fallback is needed HERE
/// only, not ported back to `try_resolve_format_text`).
fn predict_format_text(
    n: &Node,
    id: NodeId,
    in_wires: &StdMap<(NodeId, WirePort), Vec<PortRef>>,
    known: &StdMap<NodeId, FoldValue>,
) -> Option<FoldValue> {
    let template = match n.properties.get(&intern_static("FormatString")) {
        Some(Literal::String(s)) => s.clone(),
        _ => return None,
    };
    let mut inputs: Vec<Option<FoldValue>> = Vec::with_capacity(PREDICT_FORMAT_SLOTS.len());
    let mut any_substituted = false;
    for &port in &PREDICT_FORMAT_SLOTS {
        match in_wires.get(&(id, port)) {
            Some(srcs) if srcs.len() > 1 => return None, // fan-in: never folds
            Some(srcs) if !srcs.is_empty() => match known.get(&srcs[0].node_id) {
                Some(v) => {
                    inputs.push(Some(v.clone()));
                    any_substituted = true;
                }
                None => return None, // source not resolved (yet, or never)
            },
            _ => {
                let baked = n
                    .properties
                    .get(&intern_static(port.as_str()))
                    .and_then(FoldValue::from_literal);
                any_substituted |= baked.is_some();
                inputs.push(baked);
            }
        }
    }
    if !any_substituted {
        return None;
    }
    eval::format_text(&template, &inputs).map(FoldValue::Str)
}

/// Step 1: independently predict which nodes of the UNFOLDED module `root`
/// carry a fully-known certified value. A bounded fixpoint (not a real
/// topo-sort — simpler, and these generated programs are small and the pure
/// subgraph is acyclic by construction): each round, resolve every
/// not-yet-known node whose classification allows it; stop when a round
/// resolves nothing new.
///
/// Classification (deliberately the "stop there" minimal predictor from the
/// design — Select/Branch/vars/etc. are never predicted through):
///   - `_Literal`: seed from its `Value` property.
///   - a "materialized constant carrier" (see `literal_carrier_value`):
///     seed from the recipe it hard-codes.
///   - MicrochipInput/Output (chip boundary): pass through its single known
///     wire source, if it has exactly one.
///   - any other single-output node: gather its non-`Exec` data inputs in
///     declared port order. A port can be known two ways — wired to a
///     single already-known source, or (this is the load-bearing case:
///     `data/gate_semantics.json`-certified operator gates bake a literal
///     OPERAND directly onto their own consuming gate's properties at
///     lowering time — see `lower_binop`/`literal_for_property_port` —
///     rather than wiring a sibling `_Literal` node, so this is the common
///     case, not the exception) unwired but with a matching baked property.
///     Genuinely unwired-and-unbaked = `None`; fan-in = permanently
///     unresolvable. Once every input is resolved, hand them to
///     `eval::eval` — the SAME certified evaluator the real fold pass uses.
///     A non-finite float result is discarded here too, mirroring the real
///     pass's belt-and-suspenders guard (it never bakes a NaN/inf literal
///     either).
fn predict(root: &Module) -> StdMap<NodeId, FoldValue> {
    let mut nodes: StdMap<NodeId, &Node> = StdMap::new();
    collect_all_nodes(root, &mut nodes);
    let mut wires: Vec<Wire> = Vec::new();
    collect_all_wires(root, &mut wires);

    let mut in_wires: StdMap<(NodeId, WirePort), Vec<PortRef>> = StdMap::new();
    for w in &wires {
        in_wires.entry((w.target.node_id, w.target.port)).or_default().push(w.source);
    }

    let mut known: StdMap<NodeId, FoldValue> = StdMap::new();
    let mut ids: Vec<NodeId> = nodes.keys().copied().collect();
    ids.sort_unstable();

    // +4 headroom over a strict topo-depth bound costs nothing on these
    // small generated graphs and avoids an off-by-one against chip-boundary
    // hops (each hop is its own round).
    let max_rounds = ids.len() + 4;
    for _ in 0..max_rounds {
        let mut changed = false;
        for &id in &ids {
            if known.contains_key(&id) {
                continue;
            }
            let n = nodes[&id];

            if n.gate_class == gc::LITERAL {
                if let Some(v) = n.properties.get(&*sym::VALUE).and_then(FoldValue::from_literal) {
                    known.insert(id, v);
                    changed = true;
                }
                continue;
            }

            if let Some(v) = literal_carrier_value(n) {
                known.insert(id, v);
                changed = true;
                continue;
            }

            if n.gate_class == MICROCHIP_INPUT || n.gate_class == MICROCHIP_OUTPUT {
                if let Some(srcs) = in_wires.get(&(id, WirePort::RerInput))
                    && srcs.len() == 1
                    && let Some(v) = known.get(&srcs[0].node_id)
                {
                    known.insert(id, v.clone());
                    changed = true;
                }
                continue;
            }

            // `FormatText`'s certified signature is a synthetic `[Tmpl]`
            // marker no real `Value` can ever match (see `eval.rs`'s own
            // comment on this), so it's unreachable through the generic gate
            // path below — special-cased here exactly the way production
            // folding special-cases it in `fold/mod.rs::try_resolve_format_text`,
            // otherwise `${...}`-interpolation would be permanently
            // unpredictable and this pool addition would test nothing.
            if n.gate_class == gc::STRING_FORMAT_TEXT {
                if let Some(v) = predict_format_text(n, id, &in_wires, &known) {
                    known.insert(id, v);
                    changed = true;
                }
                continue;
            }

            if n.kind != NodeKind::Gate || n.ports.outputs.len() != 1 {
                continue;
            }

            let data_ports: Vec<WirePort> = n
                .ports
                .inputs
                .iter()
                .filter(|p| p.ty != Type::Exec)
                .map(|p| WirePort::from_name(sym_resolve(p.name)))
                .collect();
            let mut inputs: Vec<Option<FoldValue>> = Vec::with_capacity(data_ports.len());
            let mut blocked = false;
            for port in &data_ports {
                let wire_srcs = in_wires.get(&(id, *port)).filter(|s| !s.is_empty());
                match wire_srcs {
                    None => {
                        // Unwired — but the operand may still be a baked
                        // property (see the doc comment above): a literal
                        // operand of a certified operator gate is inlined
                        // directly onto the CONSUMING gate at lowering time,
                        // not wired from a sibling `_Literal`.
                        let baked = n
                            .properties
                            .get(&intern(port.as_str()))
                            .and_then(FoldValue::from_literal);
                        inputs.push(baked);
                    }
                    Some(srcs) if srcs.len() > 1 => {
                        blocked = true; // fan-in: never certified-foldable
                        break;
                    }
                    Some(srcs) => match known.get(&srcs[0].node_id) {
                        Some(v) => inputs.push(Some(v.clone())),
                        None => {
                            blocked = true; // source not resolved (yet, or never)
                            break;
                        }
                    },
                }
            }
            if blocked {
                continue;
            }
            if let Some(v) = eval::eval(n.gate_class, &inputs) {
                if matches!(&v, FoldValue::Float(f) if !f.is_finite()) {
                    continue; // mirrors fold/mod.rs's non-finite refusal
                }
                known.insert(id, v);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    known
}

/// `module`'s ROOT-level (not nested-chip) `out` ports, keyed by declared
/// name (`PortLabel`) — the only thing stable across two independent
/// `lower()` calls on the same source, since NodeIds are a fresh
/// process-global counter per call.
fn root_out_labels(module: &Module) -> StdMap<String, NodeId> {
    let mut map = StdMap::new();
    for id in &module.outputs {
        let Some(n) = module.nodes.get(id) else { continue };
        if let Some(Literal::String(label)) = n.properties.get(&*sym::PORT_LABEL) {
            map.insert(label.clone(), *id);
        }
    }
    map
}

/// Step 2: what value (if any) does `node_id` in the FOLDED tree actually
/// deliver? Chases through as many MicrochipInput/Output chip-boundary hops
/// as it takes (the same boundary a folded constant crosses on its way out
/// of a named chip), stopping at whichever of the pipeline's legitimate
/// "this port now IS the constant" shapes it finds first:
///   (a) a live wire whose source is a `_Literal` node — read its `Value`.
///   (b) a live wire whose source is a "materialized constant carrier" (see
///       `literal_carrier_value`) — the common shape for a literal crossing
///       a chip boundary, since MicrochipInput/Output has no data struct to
///       bake a property into either (same as `Opaque`'s Rerouter).
///   (c) no incoming wire, but the node's OWN `RER_Input`-keyed property is
///       set — `inline_orphan_literals` (or, for a Vector/Color/Rotation,
///       `materialize_unfoldable_constants`) baked the constant in place
///       and dropped the wire.
/// Anything else (a live non-literal, non-carrier, non-boundary source;
/// fan-in) means nothing legitimately delivers the value at this port —
/// `None`.
fn trace_delivered(
    wires: &[Wire],
    nodes: &StdMap<NodeId, &Node>,
    node_id: NodeId,
    depth: u32,
) -> Option<FoldValue> {
    if depth > 64 {
        return None; // guard only; no legitimate chain is anywhere near this deep
    }
    let node = *nodes.get(&node_id)?;

    if node.gate_class == gc::LITERAL {
        return node.properties.get(&*sym::VALUE).and_then(FoldValue::from_literal);
    }
    if let Some(v) = literal_carrier_value(node) {
        return Some(v);
    }

    let incoming: Vec<&Wire> = wires
        .iter()
        .filter(|w| w.target.node_id == node_id && w.target.port == WirePort::RerInput)
        .collect();
    match incoming.len() {
        0 => node.properties.get(&*sym::RER_INPUT).and_then(FoldValue::from_literal),
        1 => trace_delivered(wires, nodes, incoming[0].source.node_id, depth + 1),
        _ => None, // fan-in — never a legitimate fold delivery
    }
}

/// A short per-node/wire text dump of `module` (and nested chips) for
/// finding reports — just enough to see what actually got wired.
fn dump_module(module: &Module, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let mut ids: Vec<NodeId> = module.nodes.keys().copied().collect();
    ids.sort_unstable();
    for id in &ids {
        let n = &module.nodes[id];
        let val = if n.gate_class == gc::LITERAL {
            n.properties
                .get(&*sym::VALUE)
                .map(|v| format!(" = {v:?}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let _ = writeln!(out, "{pad}[{id}] {} kind={:?}{val}", short_class(n.gate_class), n.kind);
    }
    for w in &module.wires {
        let _ = writeln!(
            out,
            "{pad}wire [{}].{} -> [{}].{}",
            w.source.node_id, w.source.port, w.target.node_id, w.target.port
        );
    }
    for (cid, c) in &module.chips {
        let _ = writeln!(out, "{pad}chip [{cid}]:");
        dump_module(c, indent + 1, out);
    }
}

/// Compile `src` twice — `FoldMode::ForceOff` (U) and `FoldMode::ForceOn` (F) —
/// sharing one resolve+typecheck pass (fold only touches `lower()`, so U/F
/// always see identical diagnostics up to that point). Panics in any stage
/// are caught, mirroring `run_pipeline`; a plain diagnostic error on either
/// side means "reject the program" (the generator is a tight, certified-only
/// grammar, so this should essentially never happen) rather than a finding.
struct DiffPrep {
    pair: Option<(Module, Module)>,
    panic: Option<String>,
}

fn prepare_diff(src: &str) -> DiffPrep {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(128 * 1024 * 1024)
            .spawn_scoped(s, || prepare_diff_inner(src))
            .expect("spawn worker")
            .join()
            .unwrap_or(DiffPrep { pair: None, panic: Some("harness: worker thread panicked".into()) })
    })
}

fn lower_variant(
    resolved: &ResolveResult,
    tc: &TypeCheckResult,
    file: &str,
    fold_mode: FoldMode,
) -> Result<Module, String> {
    let cache = Arc::new(TemplateCache::new());
    match catch_unwind(AssertUnwindSafe(|| {
        lower(LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file,
            module_name: None,
            template_cache: cache,
            doc_comments: &resolved.doc_comments,
            fold_mode,
            ce_slots: &wirescript::typecheck::CeSlotMap::default(),
        })
    })) {
        Ok(r) => {
            let errs: Vec<&str> = r
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| d.message.as_str())
                .collect();
            if errs.is_empty() {
                Ok(r.module)
            } else {
                Err(format!("lower(fold_mode={fold_mode:?}) diagnostics: {errs:?}"))
            }
        }
        Err(p) => Err(format!("PANIC lower(fold_mode={fold_mode:?}): {}", panic_msg(p))),
    }
}

fn prepare_diff_inner(src: &str) -> DiffPrep {
    let file = "fold_diff.ws";

    let resolved = match catch_unwind(AssertUnwindSafe(|| resolve(src, file, &FsLoader))) {
        Ok(r) => r,
        Err(p) => return DiffPrep { pair: None, panic: Some(format!("PANIC resolve: {}", panic_msg(p))) },
    };
    if resolved.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return DiffPrep { pair: None, panic: None };
    }

    let tc = match catch_unwind(AssertUnwindSafe(|| typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default()))) {
        Ok(r) => r,
        Err(p) => return DiffPrep { pair: None, panic: Some(format!("PANIC typecheck: {}", panic_msg(p))) },
    };
    if tc.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return DiffPrep { pair: None, panic: None };
    }

    let u = lower_variant(&resolved, &tc, file, FoldMode::ForceOff);
    let f = lower_variant(&resolved, &tc, file, FoldMode::ForceOn);
    match (u, f) {
        (Ok(um), Ok(fm)) => DiffPrep { pair: Some((um, fm)), panic: None },
        (u, f) => {
            let mut msgs = Vec::new();
            if let Err(e) = &u {
                msgs.push(e.clone());
            }
            if let Err(e) = &f {
                msgs.push(e.clone());
            }
            let joined = msgs.join("; ");
            let is_panic = joined.contains("PANIC");
            DiffPrep { pair: None, panic: is_panic.then_some(joined) }
        }
    }
}

fn run_fold_diff(count: usize, seed: u64) {
    let mut n_total_outs = 0usize;
    let mut n_predicted_outs = 0usize;
    let mut n_rejected = 0usize;
    let mut n_crashes = 0usize;
    let mut findings: Vec<String> = Vec::new();
    let t0 = std::time::Instant::now();

    for idx in 0..count {
        let pseed = (seed
            .wrapping_mul(1_000_003)
            .wrapping_add(idx as u64)
            .wrapping_mul(0x9E3779B97F4A7C15))
            ^ 0xF01D_0000_0000_0000;
        let src = gen_fold_diff_program(pseed);

        let prep = prepare_diff(&src);
        let Some((u_mod, f_mod)) = prep.pair else {
            if let Some(msg) = prep.panic {
                n_crashes += 1;
                findings.push(format!(
                    "seed={pseed} idx={idx}: CRASH {msg}\n--- program ---\n{src}\n"
                ));
            } else {
                n_rejected += 1;
            }
            continue;
        };

        let predicted = predict(&u_mod);
        let u_labels = root_out_labels(&u_mod);
        let f_labels = root_out_labels(&f_mod);

        let mut f_nodes: StdMap<NodeId, &Node> = StdMap::new();
        collect_all_nodes(&f_mod, &mut f_nodes);
        let mut f_wires: Vec<Wire> = Vec::new();
        collect_all_wires(&f_mod, &mut f_wires);

        let mut sorted_labels: Vec<&String> = u_labels.keys().collect();
        sorted_labels.sort();
        for label in sorted_labels {
            let u_id = u_labels[label];
            n_total_outs += 1;
            let Some(pred_v) = predicted.get(&u_id) else { continue };
            n_predicted_outs += 1;

            let delivered = match f_labels.get(label) {
                Some(&f_id) => trace_delivered(&f_wires, &f_nodes, f_id, 0),
                None => None,
            };
            if delivered.as_ref() != Some(pred_v) {
                let mut report = String::new();
                let _ = writeln!(
                    report,
                    "seed={pseed} idx={idx}: out `{label}` predicted {pred_v:?} on the \
                     unfolded module but the folded module delivers {delivered:?}"
                );
                let _ = writeln!(report, "--- program ---\n{src}");
                let _ = writeln!(report, "--- unfolded IR ---");
                dump_module(&u_mod, 0, &mut report);
                let _ = writeln!(report, "--- folded IR ---");
                dump_module(&f_mod, 0, &mut report);
                findings.push(report);
            }
        }

        // Structural invariants on the folded module.
        let mut wire_out = Outcome::default();
        check_wires(&f_mod, &mut wire_out);
        if !wire_out.wire_issues.is_empty() {
            let mut report = String::new();
            let _ = writeln!(
                report,
                "seed={pseed} idx={idx}: folded module has wire-validity issues: {:?}",
                wire_out.wire_issues
            );
            let _ = writeln!(report, "--- program ---\n{src}");
            let _ = writeln!(report, "--- folded IR ---");
            dump_module(&f_mod, 0, &mut report);
            findings.push(report);
        }

        let (un, uw) = tree_counts(&u_mod);
        let (fn_, fw) = tree_counts(&f_mod);
        if fn_ > un || fw > uw {
            let mut report = String::new();
            let _ = writeln!(
                report,
                "seed={pseed} idx={idx}: folded module GREW — nodes {un}->{fn_}, wires {uw}->{fw}"
            );
            let _ = writeln!(report, "--- program ---\n{src}");
            findings.push(report);
        }

        if (idx + 1) % 100 == 0 {
            eprintln!(
                "[fold-diff] {}/{count}; {}/{} outs predicted; {} findings; {:.1}s",
                idx + 1,
                n_predicted_outs,
                n_total_outs,
                findings.len(),
                t0.elapsed().as_secs_f64()
            );
        }
    }

    let rate = if n_total_outs > 0 {
        100.0 * n_predicted_outs as f64 / n_total_outs as f64
    } else {
        0.0
    };
    println!(
        "[fold-diff] {count} programs ({n_rejected} rejected, {n_crashes} crashes); \
         {n_predicted_outs}/{n_total_outs} outs predicted ({rate:.1}%); {} findings",
        findings.len()
    );

    if !findings.is_empty() {
        std::fs::create_dir_all("fuzz_findings").ok();
        let joined = findings.join("\n════════════════════════════════════\n");
        std::fs::write("fuzz_findings/fold_diff_findings.txt", &joined)
            .expect("write fold-diff findings");
        for f in findings.iter().take(20) {
            println!("{f}");
        }
        println!("[fold-diff] findings written to fuzz_findings/fold_diff_findings.txt");
        std::process::exit(1);
    }
}

// ─────────────────────────── fold-diff program generator ───────────────────────────
//
// A small, DEDICATED generator (not the general `Gen::expr` grammar used by
// the main fuzz mode, which reaches far outside the certified operator set —
// bitwise ops, vectors, records, math builtins, string concatenation, ... —
// none of which `predict()` can ever resolve). Reuses `Gen`'s existing
// literal/name-freshening utilities (`Gen::lit`, `Gen::fresh`, its `Rng`) so
// literal syntax (escaping, negative numbers, ...) matches the rest of the
// harness, but restricts every operator to the certified set:
// `+ - * / % == != < <= > >= && || ^^ !`.

/// A leaf: 80% a plain literal, 20% `Opaque(<literal>)` — a genuine wired
/// value the fold pass (and `predict()`, matching it) must never propagate
/// through, since `Opaque`'s gate class carries no certified-table entry.
fn fold_leaf(g: &mut Gen, ty: Ty) -> String {
    let l = g.lit(ty);
    if g.rng.chance(1, 5) { format!("Opaque({l})") } else { l }
}

/// An ASCII string, <=32 chars — dedicated to the certified STRING op pool
/// (`..`/`${...}`/`Length`/`Contains`/`StartsWith`/`ToLower`/`ToUpper`/
/// `Trim`): unlike `Gen::lit(Ty::Str)` (which occasionally emits a
/// multibyte `"héllo wörld"` literal for the GENERAL fuzzer), every
/// certified string law in `eval.rs` refuses non-ASCII operands outright
/// (`string_operands_foldable`), so a non-ASCII leaf here would just be a
/// permanent, uninteresting refusal instead of exercising the pool below.
fn fold_str_ascii_lit(g: &mut Gen) -> String {
    const WORDS: &[&str] = &[
        "", "a", "ab", "hello", "world", "Hello World", "  spaced  ",
        "MixedCase", "123abc", "the quick brown fox jumps over",
    ];
    format!("\"{}\"", g.rng.pick(WORDS))
}

/// Like `fold_leaf`, but for `fold_str_ascii_lit` — 20% `Opaque`-wrapped.
fn fold_str_leaf(g: &mut Gen) -> String {
    let s = fold_str_ascii_lit(g);
    if g.rng.chance(1, 5) { format!("Opaque({s})") } else { s }
}

/// An EXACTLY-representable (as f64) decimal float literal — mirrors the
/// certified probe's own `compositeMath`/`compositeOps` operand set (halves
/// and quarters, e.g. `MakeVector(1.5,-2.5,0.75)`) — dedicated to the
/// composite pool (`Vec`/`Quat` leaves) so every component is exactly
/// representable, unlike `Gen::lit(Ty::Float)` (which can produce an
/// arbitrary, not-exactly-representable decimal like `"23.7"` for the
/// GENERAL fuzzer).
fn fold_exact_float_lit(g: &mut Gen) -> String {
    const VALUES: &[&str] = &[
        "0.0", "1.0", "-1.0", "0.5", "-0.5", "0.25", "-0.25",
        "1.5", "-1.5", "2.0", "-2.0", "0.75", "-0.75", "3.0", "4.0",
    ];
    (*g.rng.pick(VALUES)).to_string()
}

/// A `Ty::Vec3` leaf: `Vec(x, y, z)` over three independent
/// exact-representable float components (`fold_exact_float_lit`), 20%
/// `Opaque`-wrapped as a whole — mirrors `fold_leaf`'s convention.
fn fold_vec_leaf(g: &mut Gen) -> String {
    let v = format!(
        "Vec({}, {}, {})",
        fold_exact_float_lit(g), fold_exact_float_lit(g), fold_exact_float_lit(g)
    );
    if g.rng.chance(1, 5) { format!("Opaque({v})") } else { v }
}

/// A `Ty::Vec3` expression, depth <= `depth`: leaf, component-wise/broadcast
/// `+ - * /` (certified `compositeMath`), or `Cross`/`ScaleVec` (certified
/// `compositeOps`) — `Dot` is float-typed and lives in `fold_expr`'s
/// `Ty::Float` arm instead.
fn fold_vec_expr(g: &mut Gen, depth: u32) -> String {
    if depth == 0 || g.rng.chance(1, 2) {
        return fold_vec_leaf(g);
    }
    match g.rng.below(4) {
        0 => {
            let op = *g.rng.pick(&["+", "-", "*", "/"]);
            let a = fold_vec_expr(g, depth - 1);
            let b = fold_vec_expr(g, depth - 1);
            format!("({a} {op} {b})")
        }
        1 => {
            // scalar broadcast — certified on either side.
            let op = *g.rng.pick(&["+", "-", "*", "/"]);
            let v = fold_vec_expr(g, depth - 1);
            let s = fold_exact_float_lit(g);
            if g.rng.chance(1, 2) { format!("({v} {op} {s})") } else { format!("({s} {op} {v})") }
        }
        2 => {
            let a = fold_vec_expr(g, depth - 1);
            let b = fold_vec_expr(g, depth - 1);
            format!("Cross({a}, {b})")
        }
        _ => {
            let v = fold_vec_expr(g, depth - 1);
            let s = fold_exact_float_lit(g);
            format!("ScaleVec({v}, {s})")
        }
    }
}

/// A `Ty::Str` expression, depth <= `depth`: an ASCII leaf
/// (`fold_str_leaf` — dedicated so a non-ASCII `Gen::lit` pick never lands
/// here and silently tanks this pool's coverage with refusals), `..`
/// concatenation of two (possibly differently-typed) scalar sub-expressions,
/// `ToLower`/`ToUpper`/`Trim` of a Str sub-expression, or a
/// `${...}`-interpolated leaf (`fold_interp_leaf`) — the certified `strings`
/// chapter's own operator set (`eval.rs`).
fn fold_str_expr(g: &mut Gen, depth: u32) -> String {
    if depth == 0 || g.rng.chance(1, 2) {
        return fold_str_leaf(g);
    }
    match g.rng.below(5) {
        0 => {
            let ta = *g.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str]);
            let tb = *g.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str]);
            let a = if ta == Ty::Str { fold_str_expr(g, depth - 1) } else { fold_expr(g, ta, depth - 1) };
            let b = if tb == Ty::Str { fold_str_expr(g, depth - 1) } else { fold_expr(g, tb, depth - 1) };
            format!("({a} .. {b})")
        }
        1 => format!("ToLower({})", fold_str_expr(g, depth - 1)),
        2 => format!("ToUpper({})", fold_str_expr(g, depth - 1)),
        3 => format!("Trim({})", fold_str_expr(g, depth - 1)),
        _ => fold_interp_leaf(g, depth - 1),
    }
}

/// A `${...}`-interpolated Str leaf: 1-3 embedded Int/Float/Bool
/// sub-expressions (matching `render_for_format`'s certified law for those
/// variants — composites are deliberately excluded (scalar-only inputs);
/// nested STRING sub-expressions are also excluded, sidestepping untested
/// quote-in-interpolation lexing) wrapped in
/// static ASCII text, free of `"`/`\`/`$` so it can never accidentally start
/// a second interpolation. Lowers to `String_FormatText`
/// (`lower/ops.rs::lower_interp`) — exercises `predict_format_text`.
fn fold_interp_leaf(g: &mut Gen, depth: u32) -> String {
    let n = g.rng.range(1, 3);
    let mut out = String::from("\"v");
    for i in 0..n {
        let t = *g.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool]);
        let e = fold_expr(g, t, depth);
        out.push_str(&format!("${{{e}}}"));
        if i + 1 < n {
            out.push('-');
        }
    }
    out.push('"');
    out
}

/// An expression of type `ty`, depth <= `depth`, built only from the
/// certified operator set over homogeneous-typed operands (equality is the
/// only operator exercised across all five scalar/composite types; ordered
/// compares and logical combinators keep to the types the certified table
/// actually probed those signatures at). `Str` delegates to `fold_str_expr`
/// and `Vec3` to `fold_vec_expr` (both certified `strings`/`compositeMath`/
/// `compositeOps` chapter operator sets); `Ty::Float` occasionally routes
/// through `Dot` and `Ty::Int` through `Length`, both vector/string-typed
/// operands producing a scalar result.
fn fold_expr(g: &mut Gen, ty: Ty, depth: u32) -> String {
    // A higher early-exit chance than the main fuzzer's general `Gen::expr`
    // (which uses 1/4): with 20% of LEAVES independently `Opaque`-wrapped
    // (a permanent, by-design prediction barrier), a full-width depth-4
    // tree accumulates enough leaves that almost every root `out` ends up
    // blocked, tanking the predictor's measured coverage even though the
    // predictor itself is sound. Depth stays capped at <= 4; deep chains
    // still happen, just less often — only the AVERAGE leaf count drops,
    // which is the generator-side lever available for tuning (not the
    // predictor).
    if depth == 0 || g.rng.chance(1, 2) {
        return fold_leaf(g, ty);
    }
    match ty {
        Ty::Int | Ty::Float => {
            if ty == Ty::Float && g.rng.chance(1, 6) {
                let a = fold_vec_expr(g, depth - 1);
                let b = fold_vec_expr(g, depth - 1);
                return format!("Dot({a}, {b})");
            }
            if ty == Ty::Int && g.rng.chance(1, 6) {
                let s = fold_str_expr(g, depth - 1);
                return format!("Length({s})");
            }
            let op = *g.rng.pick(&["+", "-", "*", "/", "%"]);
            let a = fold_expr(g, ty, depth - 1);
            let b = fold_expr(g, ty, depth - 1);
            format!("({a} {op} {b})")
        }
        Ty::Bool => match g.rng.below(5) {
            0 => {
                let t = *g.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str]);
                let op = *g.rng.pick(&["==", "!="]);
                let a = fold_expr(g, t, depth - 1);
                let b = fold_expr(g, t, depth - 1);
                format!("({a} {op} {b})")
            }
            1 => {
                let t = *g.rng.pick(&[Ty::Int, Ty::Float]);
                let op = *g.rng.pick(&["<", "<=", ">", ">="]);
                let a = fold_expr(g, t, depth - 1);
                let b = fold_expr(g, t, depth - 1);
                format!("({a} {op} {b})")
            }
            2 => {
                let op = *g.rng.pick(&["&&", "||", "^^"]);
                let a = fold_expr(g, Ty::Bool, depth - 1);
                let b = fold_expr(g, Ty::Bool, depth - 1);
                format!("({a} {op} {b})")
            }
            3 => {
                let a = fold_expr(g, Ty::Bool, depth - 1);
                format!("!({a})")
            }
            _ => {
                let s = fold_str_expr(g, depth - 1);
                let needle = fold_str_expr(g, depth - 1);
                let f = *g.rng.pick(&["Contains", "StartsWith"]);
                format!("{f}({s}, {needle})")
            }
        },
        Ty::Str => fold_str_expr(g, depth),
        Ty::Vec3 => fold_vec_expr(g, depth),
    }
}

/// Like `fold_expr`, but the top operator combines the chip's own parameter
/// `pname` (read bare, unwrapped) with a certified-const subexpression — so
/// the chip body actually depends on its argument, exercising the
/// MicrochipInput -> ... -> MicrochipOutput propagation path.
fn fold_expr_with_param(g: &mut Gen, ty: Ty, pname: &str, depth: u32) -> String {
    match ty {
        Ty::Int | Ty::Float => {
            let op = *g.rng.pick(&["+", "-", "*"]);
            let b = fold_expr(g, ty, depth);
            format!("({pname} {op} {b})")
        }
        Ty::Bool => {
            let op = *g.rng.pick(&["&&", "||", "^^"]);
            let b = fold_expr(g, ty, depth);
            format!("({pname} {op} {b})")
        }
        Ty::Str => {
            let op = *g.rng.pick(&["==", "!="]);
            let b = fold_expr(g, ty, depth);
            format!("({pname} {op} {b})")
        }
        Ty::Vec3 => {
            let op = *g.rng.pick(&["+", "-", "*"]);
            let b = fold_vec_expr(g, depth);
            format!("({pname} {op} {b})")
        }
    }
}

/// One constant-heavy fold-diff program: 1-3 `out` declarations of random
/// scalar/composite types (each a depth-<=4 certified expression), with a
/// 30% chance one of them is routed through a named chip call
/// (`chip F(x: T) -> (r: T)` called with a constant — exercises
/// chip-boundary propagation), a 25% chance of an extra `quat`-typed `out`
/// (`MakeQuaternion` over 4 independent arithmetic float args — the
/// transitively-certified recipe from `eval.rs::make_quaternion`'s doc
/// comment, so the constructor only ever folds once all four operands
/// resolve), and a 20% chance of an
/// unrelated `on t { if <const-expr> { .. } else { .. } }` wrapper
/// (exercises Branch-truncation structural cleanup; deliberately NOT
/// value-checked — `predict()` never resolves through Branch).
fn gen_fold_diff_program(seed: u64) -> String {
    let mut g = Gen::new(seed);
    let mut blocks: Vec<String> = Vec::new();

    let n_outs = g.rng.range(1, 3);
    let out_types: Vec<Ty> = (0..n_outs)
        .map(|_| *g.rng.pick(&[Ty::Int, Ty::Float, Ty::Bool, Ty::Str, Ty::Vec3]))
        .collect();
    let chip_wrap_idx = if !out_types.is_empty() && g.rng.chance(3, 10) {
        Some(g.rng.below(out_types.len()))
    } else {
        None
    };

    for (i, ty) in out_types.iter().enumerate() {
        let oname = g.fresh("fo");
        if Some(i) == chip_wrap_idx {
            let cname = g.fresh("Fc");
            let body = fold_expr_with_param(&mut g, *ty, "p0", 3);
            blocks.push(format!(
                "chip {cname}(p0: {t}) -> (r: {t}) {{\n  out r = {body}\n}}",
                t = ty.name()
            ));
            let arg = fold_expr(&mut g, *ty, 2);
            let cvar = g.fresh("fc");
            blocks.push(format!("let {cvar} = {cname}({arg})"));
            blocks.push(format!("out {oname} = {cvar}"));
        } else {
            let e = fold_expr(&mut g, *ty, 4);
            blocks.push(format!("out {oname} = {e}"));
        }
    }

    if g.rng.chance(1, 4) {
        let oname = g.fresh("foq");
        let args: Vec<String> = (0..4).map(|_| fold_expr(&mut g, Ty::Float, 2)).collect();
        blocks.push(format!(
            "out {oname} = Quat({}, {}, {}, {})",
            args[0], args[1], args[2], args[3]
        ));
    }

    if g.rng.chance(1, 5) {
        let t = g.fresh("ft");
        let a = g.fresh("fa");
        let oa = g.fresh("foa");
        let cond = fold_expr(&mut g, Ty::Bool, 3);
        blocks.push(format!("in {t}: exec"));
        blocks.push(format!("var {a}: int = 0"));
        blocks.push(format!(
            "on {t} {{\n  if {cond} {{\n    {a} = 1\n  }} else {{\n    {a} = 2\n  }}\n}}"
        ));
        blocks.push(format!("out {oa} = {a}"));
    }

    blocks.join("\n")
}

// ─────────────────────────── main ───────────────────────────

fn main() {
    let mut count: usize = 5000;
    let mut seed: u64 = 1;
    let mut out_dir = "fuzz_findings".to_string();
    let mut calibrate_dir: Option<String> = None;
    let mut selftest_only = false;
    let mut probe_file: Option<String> = None;
    let mut fold_diff_count: Option<usize> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--count" => {
                i += 1;
                count = args[i].parse().expect("--count N");
            }
            "--seed" => {
                i += 1;
                seed = args[i].parse().expect("--seed S");
            }
            "--out" => {
                i += 1;
                out_dir = args[i].clone();
            }
            "--calibrate" => {
                i += 1;
                calibrate_dir = Some(args[i].clone());
            }
            "--selftest-only" => selftest_only = true,
            "--probe" => {
                i += 1;
                probe_file = Some(args[i].clone());
            }
            "--fold-diff" => {
                i += 1;
                fold_diff_count = Some(args[i].parse().expect("--fold-diff N"));
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    install_panic_hook();

    if let Some(p) = probe_file {
        let src = std::fs::read_to_string(&p).expect("read probe file");
        let out = run_pipeline(&src);
        println!("probe: {p}");
        println!("errors ({}):", out.error_diags.len());
        for d in &out.error_diags {
            println!("  E {d}");
        }
        println!("warnings ({}):", out.warn_diags.len());
        for d in &out.warn_diags {
            println!("  W {d}");
        }
        if let Some((stage, msg)) = &out.panic {
            println!("PANIC in {stage}: {msg}");
        }
        println!("unsupported nodes ({}):", out.unsupported.len());
        for (n2, raw) in &out.unsupported {
            println!("  {n2}   <<= {raw}");
        }
        println!("wire details ({}):", out.wire_detail.len());
        for d in &out.wire_detail {
            println!("  {d}");
        }
        if let Some(e) = &out.emit_err {
            println!("emit error: {e}");
        }
        println!("findings:");
        for (k, b) in out.findings() {
            println!("  {} {}", k.name(), b);
        }
        return;
    }

    if !selftest() {
        eprintln!("selftest FAILED — oracle plumbing broken, aborting");
        std::process::exit(1);
    }
    if selftest_only {
        return;
    }
    if let Some(d) = calibrate_dir {
        calibrate(&d);
        return;
    }
    if let Some(n) = fold_diff_count {
        run_fold_diff(n, seed);
        return;
    }

    let t0 = std::time::Instant::now();
    let mut findings: Vec<Finding> = Vec::new();
    let mut bucket_counts: BTreeMap<(Kind, String), usize> = BTreeMap::new();
    let mut n_errored = 0usize;
    let mut n_clean = 0usize;
    let mut n_warned = 0usize;
    let mut error_code_tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut known_pure_array_read = 0usize;

    for idx in 0..count {
        let pseed = seed
            .wrapping_mul(1_000_003)
            .wrapping_add(idx as u64)
            .wrapping_mul(0x9E3779B97F4A7C15);
        let mut meta_rng = Rng::new(pseed ^ 0xABCD);
        // ~10% storage-canary bundles, ~15% full-grammar import bundles, the
        // rest single-file. Bundles are never garbage-mutated (the storage
        // oracle derives expectations from the source text, and scrambled
        // markers would make those claims meaningless).
        let flavor = meta_rng.below(20);
        let src = if flavor < 2 {
            gen_import_canary(pseed)
        } else if flavor < 5 {
            gen_import_program(pseed)
        } else {
            let base = Gen::new(pseed).generate();
            if meta_rng.chance(1, 20) {
                mutate_garbage(&mut meta_rng, &base)
            } else {
                base
            }
        };

        let out = run_pipeline(&src);

        if out.has_errors() {
            n_errored += 1;
            for e in &out.error_diags {
                let code = e
                    .split(']')
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('[')
                    .to_string();
                *error_code_tally.entry(code.clone()).or_default() += 1;
                if e.contains("array index read") {
                    known_pure_array_read += 1;
                }
            }
        } else if out.warn_diags.is_empty() {
            n_clean += 1;
        } else {
            n_warned += 1;
        }

        for (kind, bucket) in out.findings() {
            let key = (kind, bucket.clone());
            let c = bucket_counts.entry(key).or_insert(0);
            *c += 1;
            // keep the smallest exemplar program per bucket
            let existing = findings
                .iter_mut()
                .find(|f| f.kind == kind && f.bucket == bucket);
            let raw_detail = match kind {
                Kind::Unsupported => out
                    .unsupported
                    .iter()
                    .map(|(n2, r)| format!("{n2}  <<= {r}"))
                    .collect::<Vec<_>>()
                    .join(" | "),
                Kind::WireFanIn | Kind::WireDangling | Kind::WireDup => {
                    out.wire_detail.join(" | ")
                }
                Kind::StorageCollapse | Kind::DroppedWrite => out.storage_detail.join(" | "),
                _ => bucket.clone(),
            };
            let mut diags: Vec<String> = Vec::new();
            for e in &out.error_diags {
                diags.push(format!("E {e}"));
            }
            for w in &out.warn_diags {
                diags.push(format!("W {w}"));
            }
            match existing {
                Some(f) => {
                    if src.len() < f.program.len() {
                        f.program = src.clone();
                        f.seed = pseed;
                        f.index = idx;
                        f.diags = diags;
                        f.warn_only = !out.warn_diags.is_empty();
                        f.no_diags = out.warn_diags.is_empty() && out.error_diags.is_empty();
                        f.raw_detail = raw_detail;
                    }
                }
                None => findings.push(Finding {
                    kind,
                    bucket,
                    program: src.clone(),
                    seed: pseed,
                    index: idx,
                    diags,
                    warn_only: !out.warn_diags.is_empty(),
                    no_diags: out.warn_diags.is_empty() && out.error_diags.is_empty(),
                    raw_detail,
                }),
            }
        }

        if (idx + 1) % 1000 == 0 {
            eprintln!(
                "[fuzz] {}/{} programs; {} buckets; {} errored; {:.1}s",
                idx + 1,
                count,
                bucket_counts.len(),
                n_errored,
                t0.elapsed().as_secs_f64()
            );
        }
    }

    let gen_elapsed = t0.elapsed().as_secs_f64();
    eprintln!(
        "[fuzz] run complete: {count} programs in {gen_elapsed:.1}s; minimizing {} buckets ...",
        findings.len()
    );

    // ── minimize ──
    let tmin = std::time::Instant::now();
    let mut minimized: Vec<(usize, String)> = Vec::new();
    for (fi, f) in findings.iter().enumerate() {
        let m = minimize(&f.program, f.kind, &f.bucket, 800);
        minimized.push((fi, m));
        eprintln!(
            "[fuzz] minimized bucket {}/{} ({} {})",
            fi + 1,
            findings.len(),
            f.kind.name(),
            &f.bucket[..f.bucket.len().min(60)]
        );
    }
    let min_elapsed = tmin.elapsed().as_secs_f64();

    // ── write findings dir ──
    std::fs::create_dir_all(&out_dir).expect("create out dir");
    let mut report = String::new();
    let _ = writeln!(
        report,
        "fuzz_programs report — {count} programs, base seed {seed}, gen {gen_elapsed:.1}s + minimize {min_elapsed:.1}s"
    );
    let _ = writeln!(
        report,
        "programs: {n_errored} errored (rejected), {n_warned} warning-only, {n_clean} fully clean"
    );
    let _ = writeln!(report, "error-code tally: {error_code_tally:?}");
    let _ = writeln!(
        report,
        "known-issue: {known_pure_array_read} pure-array-read WS007 errors (hard errors now, not silent)"
    );
    let _ = writeln!(report, "distinct buckets: {}", findings.len());
    let _ = writeln!(report);

    for (fi, min_src) in &minimized {
        let f = &findings[*fi];
        let hits = bucket_counts
            .get(&(f.kind, f.bucket.clone()))
            .copied()
            .unwrap_or(0);
        let slug: String = f
            .bucket
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(24)
            .collect();
        let dir = format!("{out_dir}/bucket_{fi:03}_{}_{slug}", f.kind.name());
        std::fs::create_dir_all(&dir).expect("bucket dir");
        std::fs::write(format!("{dir}/original.ws"), &f.program).expect("write original");
        std::fs::write(format!("{dir}/minimized.ws"), min_src).expect("write minimized");
        let re_out = run_pipeline(min_src);
        let mut meta = String::new();
        let _ = writeln!(meta, "kind: {}", f.kind.name());
        let _ = writeln!(meta, "bucket: {}", f.bucket);
        let _ = writeln!(meta, "hits: {hits}");
        let _ = writeln!(meta, "seed: {} (index {})", f.seed, f.index);
        let _ = writeln!(
            meta,
            "diag-class: {}",
            if f.no_diags {
                "NO DIAGNOSTICS AT ALL"
            } else if f.warn_only {
                "warning-only"
            } else {
                "has-errors (crash bucket)"
            }
        );
        let _ = writeln!(meta, "detail: {}", f.raw_detail);
        let _ = writeln!(meta, "original diagnostics:");
        for d in &f.diags {
            let _ = writeln!(meta, "  {d}");
        }
        let _ = writeln!(meta, "minimized diagnostics:");
        for d in re_out.error_diags.iter() {
            let _ = writeln!(meta, "  E {d}");
        }
        for d in re_out.warn_diags.iter() {
            let _ = writeln!(meta, "  W {d}");
        }
        std::fs::write(format!("{dir}/meta.txt"), &meta).expect("write meta");

        let _ = writeln!(report, "── bucket {fi:03} ── {} — {}", f.kind.name(), f.bucket);
        let _ = writeln!(
            report,
            "   hits={hits} diag-class={}",
            if f.no_diags {
                "NO-DIAGNOSTICS"
            } else if f.warn_only {
                "warning-only"
            } else {
                "has-errors"
            }
        );
        let _ = writeln!(report, "   minimized ({} lines):", min_src.lines().count());
        for l in min_src.lines() {
            let _ = writeln!(report, "   | {l}");
        }
        let _ = writeln!(report);
    }

    std::fs::write(format!("{out_dir}/REPORT.txt"), &report).expect("write report");
    println!("{report}");
    println!("[fuzz] findings written to {out_dir}/");
}
