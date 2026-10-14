//! The constants a point in the program can read by name.
//!
//! Typecheck and lowering each keep one [`ConstScope`], with a frame of
//! constants beside each open scope frame, and every question of the form
//! "is this name a compile-time constant here, and which value" is answered
//! by it. A name answers through the binding the scope resolves it to: a
//! parameter, `var`, loop variable, pattern binding or any other value named
//! like an outer constant hides that constant from every evaluator alike,
//! and a constant comes back once the scope that hid it closes.
//!
//! The constant tables are private to this module, so no evaluator can read
//! a constant by name around the scope.

use std::cell::RefCell;
use std::sync::Arc;

use crate::collections::{HashMap, HashSet};
use crate::ir::Literal;
use crate::lower::ConstEnv;
use crate::scope::Scope;

/// How an evaluation finds the `const mod` a call names, when it may call one.
pub(crate) type ModLookup<'b> = Option<&'b dyn Fn(&str) -> Option<Arc<crate::ast::ChipDecl>>>;

/// The constants of the module a namespace member's body belongs to, as a
/// frame records them (see [`ConstScope::namespace_members`]).
pub(crate) type MemberConsts = Arc<Vec<(String, Literal, Marks)>>;

/// What the binding a name resolves to says about reading the name as a
/// constant.
pub(crate) enum Bound {
    /// Not a value (a mod, chip, event, type or namespace): it hides nothing.
    NotAValue,
    /// A `let`, a `const`, a parameter or a pattern binding: a constant
    /// exactly when the frame binding it records one under the name (a `let`
    /// of a constant, a `const` parameter).
    MayBeConstant,
    /// A gate built for a constant container, holding this value: the
    /// constant the name reads without the gate, when that is this value.
    Holds(Literal),
    /// Any other value: a `var`, a buffer, a port, an event parameter. Never
    /// a constant.
    Value,
}

/// Which constants a read admits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admit {
    /// Every constant.
    All,
    /// Every constant except a placeholder (see [`Marks::placeholder`]).
    NoPlaceholders,
    /// Only a name spelled `const` (or a `const` parameter), and no
    /// placeholder: what may decide an `if` or a `match`.
    Declared,
    /// Every constant except one computed through an integer operation the
    /// certified fold table has no result for (see [`Marks::uncertified`]):
    /// what a `float` slot may be given as a literal.
    Certified,
}

impl Admit {
    const COUNT: usize = 4;

    fn slot(self) -> usize {
        match self {
            Admit::All => 0,
            Admit::NoPlaceholders => 1,
            Admit::Declared => 2,
            Admit::Certified => 3,
        }
    }
}

/// How a recorded constant may be read.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Marks {
    /// Spelled `const`, or a `const` parameter.
    pub(crate) declared: bool,
    /// Its value stands for whatever each call passes (a `const` parameter
    /// checked once for every call), or is derived from such a value: it
    /// means only "some constant is here" and must never decide anything.
    pub(crate) placeholder: bool,
    /// Computed through an integer operation the certified fold table has no
    /// result for (see [`crate::const_eval::integer_ops_certified`]).
    pub(crate) uncertified: bool,
}

impl Marks {
    fn admit(self, admit: Admit) -> bool {
        match admit {
            Admit::All => true,
            Admit::NoPlaceholders => !self.placeholder,
            Admit::Declared => self.declared && !self.placeholder,
            Admit::Certified => !self.uncertified,
        }
    }
}

#[derive(Clone, Default)]
struct Frame {
    /// The scope frame this one belongs to.
    id: u64,
    values: HashMap<String, (Literal, Marks)>,
}

/// A copy of a [`ConstScope`]'s frames: what a nested declaration saw, to
/// check or lower its body in later.
#[derive(Clone, Default)]
pub(crate) struct ConstFrames(Vec<Frame>);

impl ConstFrames {
    /// Every constant the frames record, in a fixed order: empty when they
    /// record none. Two copies that describe alike read alike.
    pub(crate) fn describe(&self) -> String {
        let mut out = String::new();
        for f in &self.0 {
            let mut entries: Vec<(&String, &(Literal, Marks))> = f.values.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (name, (lit, marks)) in entries {
                out.push_str(&format!("{name}={lit:?}{marks:?};"));
            }
        }
        out
    }
}

/// The module's constants as one [`Admit`] reads them.
#[derive(Clone)]
struct Module {
    /// The top-level `let`/`const` values of the (import-merged) program.
    values: Arc<ConstEnv>,
    /// The top-level names spelled `const`.
    declared: Arc<HashSet<String>>,
    /// The top-level names whose value is not certified (see
    /// [`crate::lower::build_const_uncertified`]).
    uncertified: Arc<HashSet<String>>,
    /// `values` as each [`Admit`] reads it, by [`Admit::slot`].
    admitted: [Arc<ConstEnv>; Admit::COUNT],
    /// What a `const mod` body reads, from `values` and from its certified
    /// part, when a file declares a name more than once at its top level:
    /// each such name read as a body declared where the `const mod` is reads
    /// it (see [`crate::const_eval::ModDups`]).
    mod_dups: Option<[Arc<crate::const_eval::ModDups>; 2]>,
}

impl Module {
    fn new(values: Arc<ConstEnv>, declared: Arc<HashSet<String>>, uncertified: Arc<HashSet<String>>) -> Self {
        let certified = if uncertified.is_empty() {
            values.clone()
        } else {
            let kept = values.iter().filter(|(n, _)| !uncertified.contains(*n));
            Arc::new(kept.map(|(n, v)| (n.clone(), v.clone())).collect())
        };
        let declared_values: Arc<ConstEnv> = Arc::new(
            declared.iter().filter_map(|n| values.get(n).map(|l| (n.clone(), l.clone()))).collect(),
        );
        let admitted = [values.clone(), values.clone(), declared_values, certified];
        Self { values, declared, uncertified, admitted, mod_dups: None }
    }

    /// These constants, a `const mod` body reading each name declared more
    /// than once at the top level by position (see `mod_dups`).
    fn with_dups(mut self, dups: &Dups) -> Self {
        if dups.top.has_dups() {
            let values = |admit: Admit| -> Arc<crate::lower::DupConsts> {
                let kept = dups.values.iter().filter(|(_, (_, marks))| marks.admit(admit));
                Arc::new(kept.map(|(k, (lit, _))| (k.clone(), lit.clone())).collect())
            };
            let made = |base: &Arc<ConstEnv>, admit: Admit| {
                Arc::new(crate::const_eval::ModDups::new(base.clone(), dups.top.clone(), values(admit)))
            };
            self.mod_dups = Some([
                made(&self.values, Admit::All),
                made(&self.admitted[Admit::Certified.slot()], Admit::Certified),
            ]);
        }
        self
    }
}

/// The constants a program point reads by name, as [`ConstScope::env`]
/// answers each name there: the module's constants, under the answers of the
/// names the open frames bind or record. A name a frame binds to a value
/// that is not a constant reads as nothing, and [`binds`](Self::binds) still
/// says it is bound, so an evaluator that resolves a name itself (a bare enum
/// variant, a namespace member, an enum's name) knows the name is taken.
#[derive(Clone, Default)]
pub(crate) struct ConstNames {
    module: Arc<ConstEnv>,
    /// Each name bound or recorded above the module that may hide or change a
    /// module constant, or that an evaluator resolves itself: its constant,
    /// or `None` when it is bound to a value that is not one.
    here: Arc<HashMap<&'static str, Option<Literal>>>,
}

impl ConstNames {
    /// The constants `module` holds, with no frame open.
    pub(crate) fn of(module: Arc<ConstEnv>) -> Self {
        Self { module, here: Arc::default() }
    }

    /// The constant `name` reads as. A namespace member `ns.m` reads as
    /// nothing while `ns` is bound here to a value.
    pub(crate) fn get(&self, name: &str) -> Option<&Literal> {
        if let Some(answer) = self.here.get(name) {
            return answer.as_ref();
        }
        if let Some((ns, _)) = name.split_once('.')
            && self.here.contains_key(ns)
        {
            return None;
        }
        self.module.get(name)
    }

    /// Whether `name` reads as a constant.
    pub(crate) fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Whether `name` is bound here to a value, a constant or not: what
    /// keeps an evaluator from reading it as a bare enum variant, an enum or
    /// a namespace.
    pub(crate) fn binds(&self, name: &str) -> bool {
        self.here.contains_key(name) || self.module.contains_key(name)
    }

    /// Bind `name` to the constant `lit`.
    pub(crate) fn insert(&mut self, name: String, lit: Literal) {
        let name = crate::intern::resolve(crate::intern::intern(&name));
        Arc::make_mut(&mut self.here).insert(name, Some(lit));
    }

    /// Undo every [`insert`](Self::insert) of `name`.
    pub(crate) fn remove(&mut self, name: &str) {
        if self.here.contains_key(name) {
            Arc::make_mut(&mut self.here).remove(name);
        }
    }
}

/// A table of constants by name.
pub(crate) trait Constants {
    /// The constant `name` reads as.
    fn constant(&self, name: &str) -> Option<&Literal>;
}

impl Constants for ConstEnv {
    fn constant(&self, name: &str) -> Option<&Literal> {
        self.get(name)
    }
}

impl Constants for ConstNames {
    fn constant(&self, name: &str) -> Option<&Literal> {
        self.get(name)
    }
}

impl<C: Constants + ?Sized> Constants for Arc<C> {
    fn constant(&self, name: &str) -> Option<&Literal> {
        (**self).constant(name)
    }
}

/// A name hidden from the top level, and the constant the chip hiding it
/// declares under it, once recorded.
pub(crate) type HiddenAtTop = (String, Option<(Literal, Marks)>);

/// The names a file declares more than once at its top level (see
/// [`crate::scope::top::TopNames::visible_decl`]) and the constant each of
/// their declarations holds.
#[derive(Default)]
struct Dups {
    top: Arc<crate::scope::top::TopNames>,
    values: HashMap<(Arc<str>, usize, String), (Literal, Marks)>,
}

/// A memoized environment for one [`Admit`], keyed by the scope's and the
/// frames' revisions.
type Answered = Option<(u64, u64, ConstNames)>;

/// The module's constants and the constants of each open scope frame.
pub(crate) struct ConstScope {
    module: Module,
    frames: Vec<Frame>,
    /// The names an evaluator resolves without the constant tables: every
    /// enum, every variant, every namespace with a constant member, and the
    /// builtins that read an enum. A binding of one of them in an open frame
    /// is answered by [`env`](Self::env).
    by_name: Arc<HashSet<String>>,
    /// The names in `by_name` the top level declares as values (a `var`, a
    /// port, a `let`, storage), and the module constants storage shares a
    /// name with, answered by [`env`](Self::env) where the top level is
    /// visible.
    by_name_at_top: Arc<Vec<String>>,
    /// The constants of each `import * as` module by its file, all of them
    /// and the certified ones (see [`crate::const_eval::ConstCtx::module_of`]).
    by_file: [crate::const_eval::ModuleConsts; 2],
    /// The program's type declarations, which a constant with a declared type
    /// is held in.
    types: Arc<crate::const_eval::DeclaredTypes>,
    /// The names a read that reaches the top level does not find there: the
    /// value names of the anonymous chip being checked or lowered at the top
    /// level, which the top level shares (see [`hide_at_top`]). Each holds
    /// the chip's constant of that name once one is recorded.
    ///
    /// [`hide_at_top`]: Self::hide_at_top
    hidden_at_top: Vec<HiddenAtTop>,
    /// The names declared more than once at the top level of a file, and the
    /// place at the top level reads are made at now (see
    /// [`set_cursor`](Self::set_cursor)), which decides which of their
    /// declarations a read names.
    dups: Arc<Dups>,
    cursor: Option<crate::scope::top::ReadAt>,
    /// Changes with every change to the frames, from one counter shared by
    /// every [`ConstScope`].
    rev: u64,
    answered: RefCell<[Answered; Admit::COUNT]>,
}

fn next_rev() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Default for ConstScope {
    fn default() -> Self {
        Self::new(Arc::default(), Arc::default(), Arc::default())
    }
}

impl ConstScope {
    /// The constants of a program whose top level declares `values`, the
    /// names in `declared` with `const`, those in `uncertified` computed
    /// through an operation with no certified result.
    pub(crate) fn new(
        values: Arc<ConstEnv>,
        declared: Arc<HashSet<String>>,
        uncertified: Arc<HashSet<String>>,
    ) -> Self {
        // The namespaces with a constant member, and the builtins that read
        // an enum; `for_program` adds the enums and their variants.
        let by_name = ["EnumToInt", "IntToEnum"]
            .iter()
            .map(|n| n.to_string())
            .chain(values.keys().filter_map(|k| k.split_once('.')).map(|(ns, _)| ns.to_string()))
            .collect();
        Self {
            module: Module::new(values, declared, uncertified),
            frames: Vec::new(),
            by_name: Arc::new(by_name),
            by_name_at_top: Arc::default(),
            by_file: Default::default(),
            types: Arc::default(),
            hidden_at_top: Vec::new(),
            dups: Arc::default(),
            cursor: None,
            rev: next_rev(),
            answered: RefCell::new(Default::default()),
        }
    }

    /// The constants of the program whose top level is `decls`: what both
    /// typecheck and lowering start from, so the two cannot disagree about
    /// what counts as a compile-time constant.
    pub(crate) fn for_program(
        decls: &[crate::ast::TopDecl],
        enum_defs: &Arc<crate::typecheck::enums::EnumRegistry>,
        imports: &Arc<crate::ast::ImportScope>,
        top: &Arc<crate::scope::top::TopNames>,
    ) -> Self {
        let types = Arc::new(crate::const_eval::DeclaredTypes::for_program_in(decls, enum_defs, imports));
        let (values, dup_values) = crate::lower::build_const_env_at(decls, enum_defs, imports, &types, top);
        let uncertified = crate::lower::build_const_uncertified(decls, enum_defs, imports, &values, &types);
        let mut all: HashMap<Arc<str>, Arc<ConstEnv>> = HashMap::default();
        let mut certified: HashMap<Arc<str>, Arc<ConstEnv>> = HashMap::default();
        for d in decls {
            let crate::ast::TopDecl::Namespace(ns) = d else { continue };
            let Some(file) = ns.decls.first().map(|d| d.range().file.clone()) else { continue };
            let prefix = format!("{}.", ns.name);
            let own: Vec<(&String, String, &Literal)> = values
                .iter()
                .filter_map(|(k, v)| k.strip_prefix(&prefix).map(|n| (k, n.to_string(), v)))
                .collect();
            let table = |keep: &dyn Fn(&String) -> bool| -> Arc<ConstEnv> {
                Arc::new(own.iter().filter(|(k, ..)| keep(k)).map(|(_, n, v)| (n.clone(), (*v).clone())).collect())
            };
            all.insert(file.clone(), table(&|_| true));
            certified.insert(file, table(&|k| !uncertified.contains(k)));
        }
        let mut dups = Dups { top: top.clone(), values: HashMap::default() };
        for d in decls {
            let crate::ast::TopDecl::Let(l) = d else { continue };
            for name in crate::const_eval::bound_names(&l.binding) {
                let key = (l.range.file.clone(), l.range.start.offset, name);
                if let Some(lit) = dup_values.get(&key) {
                    let uncertified = uncertified.contains(&key.2);
                    let marks = Marks { declared: l.is_const, uncertified, ..Default::default() };
                    dups.values.insert(key, (lit.clone(), marks));
                }
            }
        }
        let mut out = Self::new(
            Arc::new(values),
            Arc::new(crate::lower::build_const_declared_names(decls)),
            Arc::new(uncertified),
        );
        out.module = out.module.with_dups(&dups);
        out.dups = Arc::new(dups);
        let by_name = Arc::make_mut(&mut out.by_name);
        by_name.extend(enum_defs.names().map(str::to_string));
        // A module constant named like storage the top level or a chip
        // declares is answered too: where that storage is the binding, it hides
        // the constant.
        let storage = top_level_storage(decls);
        let mut at_top: Vec<String> = top_level_values(decls)
            .into_iter()
            .filter(|n| by_name.contains(n) || (storage.contains(n) && out.module.values.contains_key(n)))
            .collect();
        at_top.sort();
        at_top.dedup();
        out.by_name_at_top = Arc::new(at_top);
        out.by_file = [Arc::new(all), Arc::new(certified)];
        out.types = types;
        out
    }

    /// The program's type declarations.
    pub(crate) fn types(&self) -> &Arc<crate::const_eval::DeclaredTypes> {
        &self.types
    }

    /// The constants a chip's body starts with: these, with its `const`
    /// parameters `params` as module constants, and `frames` (the frames of
    /// the scope its declaration sees) open.
    pub(crate) fn for_chip_body(&self, params: &ConstEnv, frames: ConstFrames) -> Self {
        let module = if params.is_empty() {
            self.module.clone()
        } else {
            let mut values = (*self.module.values).clone();
            let mut declared = (*self.module.declared).clone();
            for (name, lit) in params {
                values.insert(name.clone(), lit.clone());
                declared.insert(name.clone());
            }
            let mut uncertified = (*self.module.uncertified).clone();
            for name in params.keys() {
                uncertified.remove(name);
            }
            Module::new(Arc::new(values), Arc::new(declared), Arc::new(uncertified)).with_dups(&self.dups)
        };
        Self {
            module,
            frames: frames.0,
            by_name: self.by_name.clone(),
            by_name_at_top: self.by_name_at_top.clone(),
            by_file: self.by_file.clone(),
            types: self.types.clone(),
            hidden_at_top: Vec::new(),
            dups: self.dups.clone(),
            cursor: self.cursor.clone(),
            rev: next_rev(),
            answered: RefCell::new(Default::default()),
        }
    }

    fn bump(&mut self) {
        self.rev = next_rev();
    }

    /// Open the frame of scope frame `id`.
    pub(crate) fn push(&mut self, id: u64) {
        self.frames.push(Frame { id, ..Default::default() });
        self.bump();
    }

    /// Close the innermost frame.
    pub(crate) fn pop(&mut self) {
        self.frames.pop();
        self.bump();
    }

    /// Read the top level as a declaration written at `at` sees it, until the
    /// next call, and hand back the place read at before. A name a file
    /// declares more than once at its top level reads the declaration visible
    /// at `at` (see [`crate::scope::top::TopNames::visible_decl`]); with no
    /// place set, or read from another file, it reads the last one.
    pub(crate) fn set_cursor(
        &mut self,
        at: Option<crate::scope::top::ReadAt>,
    ) -> Option<crate::scope::top::ReadAt> {
        if !self.dups.top.has_dups() {
            return None;
        }
        self.bump();
        std::mem::replace(&mut self.cursor, at)
    }

    /// Move the place reads are made at past the top-level declaration at
    /// `range`, once it is bound: a read after it names it.
    pub(crate) fn bound_at_top(&mut self, range: &crate::diagnostic::SourceRange) {
        if self.frames.is_empty()
            && let Some(at) = self.cursor.as_ref().filter(|c| c.range.file == range.file)
        {
            let end = crate::diagnostic::SourceRange { start: range.end, ..range.clone() };
            let body = at.body;
            self.set_cursor(Some(crate::scope::top::ReadAt { range: end, body }));
        }
    }

    /// The place at the top level reads are made at now (see
    /// [`set_cursor`](Self::set_cursor)).
    pub(crate) fn cursor(&self) -> Option<crate::scope::top::ReadAt> {
        self.cursor.clone()
    }

    /// For a name a file declares more than once at its top level, read at
    /// the cursor in that file: the constant of the declaration visible
    /// there, `None` when it holds none. `None` for any other name.
    fn dup_answer(&self, name: &str) -> Option<Option<&(Literal, Marks)>> {
        let at = self.cursor.as_ref()?;
        let decl = self.dups.top.visible_decl(name, at)?;
        Some(decl.and_then(|d| self.dups.values.get(&crate::lower::dup_key(d, name))))
    }

    /// The constant the top-level declaration written at `decl` holds under
    /// `name`, a name its file declares more than once there.
    pub(crate) fn dup_constant(&self, decl: &crate::diagnostic::SourceRange, name: &str) -> Option<&Literal> {
        let key = (decl.file.clone(), decl.start.offset, name.to_string());
        self.dups.values.get(&key).map(|(lit, _)| lit)
    }

    /// Whether a frame is open: a body, not the top level, whose constants
    /// the module table holds.
    pub(crate) fn in_frame(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Record `name` in the innermost frame as the constant `lit`, read as
    /// `marks` say. At the top level, which no frame covers, the module's
    /// constants hold it already, unless an anonymous chip hides the name
    /// there: then it is that chip's constant.
    pub(crate) fn record(&mut self, name: &str, lit: Literal, mut marks: Marks) {
        let Some(frame) = self.frames.last_mut() else {
            if let Some(held) = self.hidden_at_top.iter_mut().rev().find(|(h, _)| h == name) {
                marks.uncertified |= self.module.uncertified.contains(name);
                held.1 = Some((lit, marks));
                self.bump();
            }
            return;
        };
        frame.values.insert(name.to_string(), (lit, marks));
        self.bump();
    }

    /// Forget whatever the innermost frame recorded under `name`: it is
    /// bound there again, to something that is not a constant.
    pub(crate) fn forget(&mut self, name: &str) {
        let Some(frame) = self.frames.last_mut() else {
            if let Some(held) = self.hidden_at_top.iter_mut().rev().find(|(h, _)| h == name) {
                held.1 = None;
                self.bump();
            }
            return;
        };
        if frame.values.remove(name).is_some() {
            self.bump();
        }
    }

    /// Hide `names` from every read that reaches the top level, until
    /// [`unhide_at_top`](Self::unhide_at_top) is given what this returns: an
    /// anonymous chip at the top level shares the top level's scope, and its
    /// storage, never a constant, is what its statements name.
    pub(crate) fn hide_at_top(&mut self, names: impl IntoIterator<Item = String>) -> usize {
        self.hide_held_at_top(names.into_iter().map(|n| (n, None)))
    }

    /// [`hide_at_top`](Self::hide_at_top) for names a chip may already hold
    /// constants under, as [`split_hidden_at_top`](Self::split_hidden_at_top)
    /// hands them back.
    pub(crate) fn hide_held_at_top(&mut self, held: impl IntoIterator<Item = HiddenAtTop>) -> usize {
        let mark = self.hidden_at_top.len();
        self.hidden_at_top.extend(held);
        self.bump();
        mark
    }

    /// End the [`hide_at_top`](Self::hide_at_top) that returned `mark`.
    pub(crate) fn unhide_at_top(&mut self, mark: usize) {
        self.hidden_at_top.truncate(mark);
        self.bump();
    }

    /// Show again the names hidden since `mark`, and hand them back for a
    /// later [`hide_at_top`](Self::hide_at_top): the body of a declaration
    /// written outside the chips that hid them.
    pub(crate) fn split_hidden_at_top(&mut self, mark: usize) -> Vec<HiddenAtTop> {
        let names = self.hidden_at_top.split_off(mark.min(self.hidden_at_top.len()));
        self.bump();
        names
    }

    /// The names hidden from the top level now, with their chips' constants.
    pub(crate) fn hidden_at_top(&self) -> Vec<HiddenAtTop> {
        self.hidden_at_top.clone()
    }

    /// A copy of every open frame.
    pub(crate) fn snapshot(&self) -> ConstFrames {
        ConstFrames(self.frames.clone())
    }

    /// Open `frames` in place of the open frames, and hand those back.
    pub(crate) fn replace(&mut self, frames: ConstFrames) -> ConstFrames {
        self.bump();
        ConstFrames(std::mem::replace(&mut self.frames, frames.0))
    }

    /// How many frames are open.
    pub(crate) fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Close every frame above the first `at` and hand them back.
    pub(crate) fn split_off(&mut self, at: usize) -> ConstFrames {
        self.bump();
        ConstFrames(self.frames.split_off(at.min(self.frames.len())))
    }

    /// Open `frames` above the open ones.
    pub(crate) fn extend(&mut self, frames: ConstFrames) {
        self.bump();
        self.frames.extend(frames.0);
    }

    /// Whether an evaluator resolves `name` with no binding of a scope frame:
    /// an enum or a variant, a namespace with a constant member, or a builtin
    /// reading an enum.
    pub(crate) fn resolved_by_name(&self, name: &str) -> bool {
        self.by_name.contains(name)
    }

    /// Whether the top level declares `name` with `const`. Whether a read of
    /// `name` here reaches that declaration is [`get`](Self::get)'s to say.
    pub(crate) fn declared_at_top(&self, name: &str) -> bool {
        self.module.declared.contains(name)
    }

    /// The top-level constants of the module imported `import * as ns`, by
    /// their own names, as a frame its members are bound in records them:
    /// not `const`-declared, as typecheck records them in the frame it checks
    /// that module's members in.
    pub(crate) fn namespace_members(&self, ns: &str) -> Vec<(String, Literal, Marks)> {
        let prefix = format!("{ns}.");
        let mut out: Vec<(String, Literal, Marks)> = self
            .module
            .values
            .iter()
            .filter_map(|(key, lit)| {
                let name = key.strip_prefix(&prefix)?;
                let marks = Marks { uncertified: self.module.uncertified.contains(key), ..Default::default() };
                Some((name.to_string(), lit.clone(), marks))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The innermost frame belonging to scope frame `id`.
    fn frame(&self, id: u64) -> Option<&Frame> {
        self.frames.iter().rev().find(|f| f.id == id)
    }

    /// The constant `name` reads as where `scope` is now, when `admit`
    /// admits it. `bound` says what a binding of the scope is (see
    /// [`Bound`]).
    pub(crate) fn get<V>(
        &self,
        scope: &Scope<V>,
        bound: &dyn Fn(&V) -> Bound,
        name: &str,
        admit: Admit,
    ) -> Option<&Literal> {
        // A mod, type or namespace of the name is passed over: what it hides
        // is the value bindings further out.
        let binding =
            scope.visible_bindings(name).map(|(i, v)| (i, bound(v))).find(|(_, b)| !matches!(b, Bound::NotAValue));
        let (at, held) = match binding {
            Some((i, Bound::MayBeConstant)) => (Some((i, true)), None),
            Some((i, Bound::Value)) => (Some((i, false)), None),
            Some((i, Bound::Holds(lit))) => (None, Some((i, lit))),
            Some((_, Bound::NotAValue)) | None => (None, None),
        };
        let (from, found) = self.walk(scope, name, at, admit)?;
        match held {
            // A gate holding a constant container answers as the constant
            // read from its own frame or further out; a frame inside it
            // recording the name answers for itself.
            Some((i, lit)) if from <= i => (*found == lit).then_some(found),
            _ => Some(found),
        }
    }

    /// The constant `name` reads as, given `at`: the index of the frame
    /// binding it and whether that binding may be a constant, or `None` when
    /// nothing above the module binds it. The frames are walked innermost
    /// first: the first frame recording the name answers (a binding of its
    /// own that is not a constant hides it), and the frame binding the name
    /// without recording it hides every constant further out. The answer
    /// comes with the index of the frame that gave it, 0 for the module.
    fn walk<V>(
        &self,
        scope: &Scope<V>,
        name: &str,
        at: Option<(usize, bool)>,
        admit: Admit,
    ) -> Option<(usize, &Literal)> {
        for (i, id) in scope.visible_frame_ids() {
            if i == 0 {
                break;
            }
            if let Some((lit, marks)) = self.frame(id).and_then(|f| f.values.get(name)) {
                if at == Some((i, false)) || !marks.admit(admit) {
                    return None;
                }
                return Some((i, lit));
            }
            if at.is_some_and(|(b, _)| b == i) {
                return None;
            }
        }
        // A top-level binding that is no constant (a chip's storage read
        // outside the chip) hides the module's constant of its name too.
        if at.is_some_and(|(b, may_be_constant)| b > 0 || !may_be_constant) {
            return None;
        }
        if let Some((_, held)) = self.hidden_at_top.iter().rev().find(|(h, _)| h == name) {
            return held.as_ref().filter(|(_, marks)| marks.admit(admit)).map(|(lit, _)| (0, lit));
        }
        if let Some(answer) = self.dup_answer(name) {
            return answer.filter(|(_, marks)| marks.admit(admit)).map(|(lit, _)| (0, lit));
        }
        let lit = self.module.values.get(name)?;
        let admitted = match admit {
            Admit::All | Admit::NoPlaceholders => true,
            Admit::Declared => self.module.declared.contains(name),
            Admit::Certified => !self.module.uncertified.contains(name),
        };
        admitted.then_some((0, lit))
    }

    /// Every constant readable where `scope` is now, by name, as [`get`]
    /// answers each: what an evaluator resolves names against. It answers
    /// every name a frame above the module binds or records that the module
    /// holds a constant for or that an evaluator resolves itself, and every
    /// such resolved name bound at the module level to a value; it costs the
    /// open frames' bindings, never the module's table.
    ///
    /// [`get`]: Self::get
    pub(crate) fn env<V>(&self, scope: &Scope<V>, bound: &dyn Fn(&V) -> Bound, admit: Admit) -> ConstNames {
        let slot = admit.slot();
        let cached = self.answered.borrow()[slot].clone();
        let env = match cached {
            Some((s, c, env)) if c == self.rev && s == scope.rev() => return env,
            // Only names bound since: each decides only its own answer.
            Some((s, c, env)) if c == self.rev && let Some(names) = scope.inserted_since(s) => {
                let names: Vec<crate::intern::Sym> = names.collect();
                let frames = self.env_frames(scope);
                let recorded = self.recorded(&frames);
                let mut here = (*env.here).clone();
                for sym in names {
                    let key = crate::intern::resolve(sym);
                    match self.answer(scope, bound, admit, &frames, &recorded, key) {
                        Some(answer) => here.insert(key, answer),
                        None => here.remove(key),
                    };
                }
                let env = ConstNames { module: env.module, here: Arc::new(here) };
                debug_assert!(
                    *env.here == *self.rebuild(scope, bound, admit).here,
                    "an incremental constant env differs from a rebuilt one"
                );
                env
            }
            _ => self.rebuild(scope, bound, admit),
        };
        self.answered.borrow_mut()[slot] = Some((scope.rev(), self.rev, env.clone()));
        env
    }

    /// The open frames [`env`](Self::env) reads, innermost first.
    fn env_frames<V>(&self, scope: &Scope<V>) -> Vec<(usize, u64)> {
        let floor = scope.floor().max(1);
        scope.visible_frame_ids().filter(|(i, _)| *i >= floor).collect()
    }

    /// Every name a frame of `frames` records a constant under.
    fn recorded(&self, frames: &[(usize, u64)]) -> HashSet<&str> {
        frames
            .iter()
            .filter_map(|(_, id)| self.frame(*id))
            .flat_map(|f| f.values.keys().map(|k| k.as_str()))
            .collect()
    }

    /// [`env`](Self::env)'s answer for `key` alone, as [`rebuild`](Self::rebuild)
    /// decides it: `None` when `key` is left to the module.
    fn answer<V>(
        &self,
        scope: &Scope<V>,
        bound: &dyn Fn(&V) -> Bound,
        admit: Admit,
        frames: &[(usize, u64)],
        recorded: &HashSet<&str>,
        key: &'static str,
    ) -> Option<Option<Literal>> {
        let sym = crate::intern::intern(key);
        let module = &self.module.admitted[admit.slot()];
        for (i, id) in frames.iter().copied() {
            if let Some((lit, marks)) = self.frame(id).and_then(|f| f.values.get(key)) {
                return Some(match scope.get_in(i, sym).map(bound) {
                    Some(Bound::Value) => None,
                    Some(Bound::Holds(_)) => self.get(scope, bound, key, admit).cloned(),
                    Some(Bound::MayBeConstant | Bound::NotAValue) | None => marks.admit(admit).then(|| lit.clone()),
                });
            }
            match scope.get_in(i, sym).map(bound) {
                None | Some(Bound::NotAValue) => {}
                Some(Bound::Holds(_)) => return Some(self.get(scope, bound, key, admit).cloned()),
                Some(Bound::MayBeConstant | Bound::Value) => {
                    if module.contains_key(key) || recorded.contains(key) || self.by_name.contains(key) {
                        return Some(None);
                    }
                }
            }
        }
        if scope.floor() == 0 {
            if let Some((_, held)) = self.hidden_at_top.iter().rev().find(|(name, _)| name == key) {
                return Some(held.as_ref().filter(|(_, marks)| marks.admit(admit)).map(|(lit, _)| lit.clone()));
            }
            if self.dups.top.dups().any(|(name, _)| name == key)
                && let Some(answer) = self.dup_answer(key)
            {
                return Some(answer.filter(|(_, marks)| marks.admit(admit)).map(|(lit, _)| lit.clone()));
            }
            if self.by_name_at_top.iter().any(|name| name == key)
                && scope.visible_bindings(key).any(|(_, v)| !matches!(bound(v), Bound::NotAValue))
            {
                return Some(self.get(scope, bound, key, admit).cloned());
            }
        }
        None
    }

    /// [`env`](Self::env) worked out from every open frame.
    fn rebuild<V>(&self, scope: &Scope<V>, bound: &dyn Fn(&V) -> Bound, admit: Admit) -> ConstNames {
        let module = self.module.admitted[admit.slot()].clone();
        // One pass over the open frames, innermost first: a name is decided
        // by the first frame that records it or binds it to a value, as
        // `get` and `walk` decide it. A gate holding a constant container is
        // left to `get`.
        let mut here: HashMap<&'static str, Option<Literal>> = HashMap::default();
        let frames = self.env_frames(scope);
        // A binding needs an answer only when something else would answer
        // its name: the module, a frame further out, or an evaluator
        // resolving it itself.
        let recorded = self.recorded(&frames);
        for (i, id) in frames.iter().copied() {
            if let Some(frame) = self.frame(id) {
                for (name, (lit, marks)) in &frame.values {
                    let sym = crate::intern::intern(name);
                    let key = crate::intern::resolve(sym);
                    if here.contains_key(key) {
                        continue;
                    }
                    let answer = match scope.get_in(i, sym).map(bound) {
                        Some(Bound::Value) => None,
                        Some(Bound::Holds(_)) => self.get(scope, bound, key, admit).cloned(),
                        Some(Bound::MayBeConstant | Bound::NotAValue) | None => marks.admit(admit).then(|| lit.clone()),
                    };
                    here.insert(key, answer);
                }
            }
            for (sym, v) in scope.frame_entries(i) {
                let key = crate::intern::resolve(sym);
                if here.contains_key(key) {
                    continue;
                }
                match bound(v) {
                    Bound::NotAValue => {}
                    Bound::Holds(_) => {
                        here.insert(key, self.get(scope, bound, key, admit).cloned());
                    }
                    Bound::MayBeConstant | Bound::Value => {
                        if module.contains_key(key) || recorded.contains(key) || self.by_name.contains(key) {
                            here.insert(key, None);
                        }
                    }
                }
            }
        }
        if scope.floor() == 0 {
            for (name, held) in self.hidden_at_top.iter().rev() {
                let key = crate::intern::resolve(crate::intern::intern(name));
                let lit = held.as_ref().filter(|(_, marks)| marks.admit(admit)).map(|(lit, _)| lit.clone());
                here.entry(key).or_insert(lit);
            }
            for (name, _) in self.dups.top.dups() {
                if let Some(answer) = self.dup_answer(name) {
                    let key = crate::intern::resolve(crate::intern::intern(name));
                    let lit = answer.filter(|(_, marks)| marks.admit(admit)).map(|(lit, _)| lit.clone());
                    here.entry(key).or_insert(lit);
                }
            }
            for name in self.by_name_at_top.iter() {
                let key = crate::intern::resolve(crate::intern::intern(name));
                if !here.contains_key(key)
                    && scope.visible_bindings(name).any(|(_, v)| !matches!(bound(v), Bound::NotAValue))
                {
                    here.insert(key, self.get(scope, bound, key, admit).cloned());
                }
            }
        }
        ConstNames { module, here: Arc::new(here) }
    }

    /// The context `const_eval` evaluates in where `scope` is now: names
    /// resolve through [`env`](Self::env), and a `const mod` body sees the
    /// module's constants and its own parameters, never the caller's frames.
    pub(crate) fn const_ctx<'b, V>(
        &self,
        scope: &Scope<V>,
        bound: &dyn Fn(&V) -> Bound,
        admit: Admit,
        enum_defs: Arc<crate::typecheck::enums::EnumRegistry>,
        imports: Arc<crate::ast::ImportScope>,
        lookup_mod: ModLookup<'b>,
    ) -> crate::const_eval::ConstCtx<'b> {
        let (module_consts, by_file, mod_dups) = match admit {
            Admit::Certified => (
                self.module.admitted[Admit::Certified.slot()].clone(),
                &self.by_file[1],
                self.module.mod_dups.as_ref().map(|[_, certified]| certified.clone()),
            ),
            _ => (
                self.module.values.clone(),
                &self.by_file[0],
                self.module.mod_dups.as_ref().map(|[all, _]| all.clone()),
            ),
        };
        let consts = self.env(scope, bound, admit);
        let types = self.types.clone();
        crate::const_eval::ConstCtx::new(consts, module_consts, by_file.clone(), enum_defs, imports, types, lookup_mod)
            .with_mod_dups(mod_dups)
    }
}

/// The storage the top level of `decls` declares, anonymous chips included.
fn top_level_storage(decls: &[crate::ast::TopDecl]) -> HashSet<String> {
    use crate::ast::{Block, Stmt, TopDecl};
    fn walk(block: &Block, out: &mut HashSet<String>) {
        for s in &block.stmts {
            match s {
                Stmt::Var(v) => {
                    out.insert(v.name.clone());
                }
                Stmt::Array(a) => {
                    out.insert(a.name.clone());
                }
                Stmt::Map(m) => {
                    out.insert(m.name.clone());
                }
                Stmt::Buffer(b) => {
                    out.insert(b.name.clone());
                }
                Stmt::AnonChip(ac) => walk(&ac.body, out),
                _ => {}
            }
        }
    }
    let mut out = HashSet::default();
    for d in decls {
        match d {
            TopDecl::Var(v) => {
                out.insert(v.name.clone());
            }
            TopDecl::Array(a) => {
                out.insert(a.name.clone());
            }
            TopDecl::Map(m) => {
                out.insert(m.name.clone());
            }
            TopDecl::Buffer(b) => {
                out.insert(b.name.clone());
            }
            TopDecl::AnonChip(ac) => walk(&ac.body, &mut out),
            _ => {}
        }
    }
    out
}

/// The names the top level of `decls` declares as values, anonymous chip
/// bodies included: what a by-name evaluator must not take for an enum, a
/// variant or a namespace there.
fn top_level_values(decls: &[crate::ast::TopDecl]) -> Vec<String> {
    use crate::ast::{Stmt, TopDecl};
    let mut out = Vec::new();
    for d in decls {
        match d {
            TopDecl::Var(v) => out.push(v.name.clone()),
            TopDecl::Array(a) => out.push(a.name.clone()),
            TopDecl::Map(m) => out.push(m.name.clone()),
            TopDecl::Buffer(b) => out.push(b.name.clone()),
            TopDecl::In(i) => out.push(i.name.clone()),
            TopDecl::Out(o) => out.push(o.name.clone()),
            TopDecl::Let(l) => out.extend(crate::const_eval::bound_names(&l.binding)),
            TopDecl::LetElse(l) => {
                let mut captures = Vec::new();
                crate::lower::collect_pattern_captures(&l.pattern, &mut Vec::new(), &mut captures);
                out.extend(captures.into_iter().map(|(n, _)| n));
            }
            TopDecl::AnonChip(ac) => {
                for s in &ac.body.stmts {
                    match s {
                        Stmt::Var(v) => out.push(v.name.clone()),
                        Stmt::Array(a) => out.push(a.name.clone()),
                        Stmt::Map(m) => out.push(m.name.clone()),
                        Stmt::Buffer(b) => out.push(b.name.clone()),
                        Stmt::In(i) => out.push(i.name.clone()),
                        Stmt::OutBinding(o) => out.push(o.name.clone()),
                        Stmt::Let(l) => out.extend(crate::const_eval::bound_names(&l.binding)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
