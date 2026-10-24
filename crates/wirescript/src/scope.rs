use crate::collections::HashMap;

use bitflags::bitflags;

use crate::intern::Sym;

pub(crate) mod top;

bitflags! {
    /// Tags for scope frames. Combine with `|` for `iter_within` boundaries.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ScopeTag: u8 {
        const ROOT   = 1 << 0;
        const MODULE = 1 << 1;
        const BLOCK  = 1 << 2;
    }
}

struct Frame<V> {
    tag: ScopeTag,
    /// Distinct for every frame ever pushed, and kept by a copy of it
    /// ([`Scope::push_as`], [`Scope::from_frames`]).
    id: u64,
    entries: HashMap<Sym, V>,
}

fn next_frame_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Lexical scope stack. Each frame has a tag and its own symbol table.
/// Lookups walk up the chain (newest first). `iter_within` stops at
/// a boundary matching the given tag flags.
///
/// Key type is `Sym` — interned symbol handle (4 bytes, Copy).
pub struct Scope<V> {
    frames: Vec<Frame<V>>,
    /// Lowest frame index `get`/`get_mut` name resolution may see; 0 (the
    /// default) means every frame is visible. Raised temporarily while a
    /// NAMESPACED body (`import * as ns`) is checked or lowered, so a free
    /// name there resolves against the namespace's own members and language
    /// globals (events / math resolve via the catalog, NOT this stack) rather
    /// than falling through to the importer's same-named top-level state.
    /// Only the by-name lookups honor it — the `iter*` sweeps (type aliases,
    /// chip closure capture) deliberately still see every frame. See N1.
    floor: usize,
    /// Changes whenever what a name resolves to may have changed (a frame
    /// pushed, popped or swapped, a binding inserted, the floor moved). Drawn
    /// from one counter shared by every stack, so two states of any two stacks
    /// never share a value.
    rev: u64,
    /// The `rev` of the last change other than an [`insert`](Self::insert),
    /// and each insert since: its `rev` and the name it bound. A reader that
    /// knew the stack at one of these revs can tell that only these names
    /// were bound since (see [`inserted_since`](Self::inserted_since)).
    base: u64,
    inserts: Vec<(u64, Sym)>,
}

fn next_rev() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One frame as [`Scope::from_frames`] takes it: its tag, its id and its
/// entries.
pub type FrameCopy<V> = (ScopeTag, u64, Vec<(Sym, V)>);

/// Frames taken off a [`Scope`] by [`Scope::detach_above`], held until
/// [`Scope::reattach`] puts them back.
pub struct Detached<V> {
    frames: Vec<Frame<V>>,
}

impl<V> Detached<V> {
    /// Each detached frame, outermost first: its tag, its id and its entries.
    pub fn frames(&self) -> impl Iterator<Item = (ScopeTag, u64, &HashMap<Sym, V>)> {
        self.frames.iter().map(|f| (f.tag, f.id, &f.entries))
    }
}

impl<V> Default for Scope<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> Scope<V> {
    pub fn new() -> Self {
        Self {
            frames: vec![Frame {
                tag: ScopeTag::ROOT,
                id: next_frame_id(),
                entries: HashMap::default(),
            }],
            floor: 0,
            rev: 0,
            base: 0,
            inserts: Vec::new(),
        }
        .bumped()
    }

    fn bumped(mut self) -> Self {
        self.bump();
        self
    }

    /// Record a change that is not a plain insert: what a name resolves to may
    /// have changed anywhere.
    fn bump(&mut self) {
        self.rev = next_rev();
        self.base = self.rev;
        self.inserts.clear();
    }

    /// The names bound by [`insert`](Self::insert) since the stack stood at
    /// `rev`, when nothing else changed since; `None` when something else did,
    /// or `rev` is not a state of this stack.
    pub fn inserted_since(&self, rev: u64) -> Option<impl Iterator<Item = Sym> + '_> {
        let from = if rev == self.base {
            0
        } else {
            self.inserts.binary_search_by_key(&rev, |(r, _)| *r).ok()? + 1
        };
        Some(self.inserts[from..].iter().map(|(_, sym)| *sym))
    }

    pub fn push(&mut self, tag: ScopeTag) {
        self.frames.push(Frame {
            tag,
            id: next_frame_id(),
            entries: HashMap::default(),
        });
        self.bump();
    }

    pub fn pop(&mut self) {
        if self.frames.len() <= 1 {
            return;
        }
        self.frames.pop();
        // A caller that popped below a raised floor without restoring it would
        // otherwise leave `floor` past the top of the stack; clamp so lookups
        // stay well-defined.
        self.floor = self.floor.min(self.frames.len());
        self.bump();
    }

    /// See the `rev` field.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Each frame by-name resolution can see, innermost first: its index and
    /// its id.
    pub fn visible_frame_ids(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        (self.floor..self.frames.len())
            .rev()
            .map(|i| (i, self.frames[i].id))
    }

    /// The id of the innermost frame.
    pub fn current_id(&self) -> u64 {
        self.frames.last().map_or(0, |f| f.id)
    }

    /// Whether a frame with this id is open.
    pub fn has_frame(&self, id: u64) -> bool {
        self.frames.iter().any(|f| f.id == id)
    }

    /// Number of frames on the stack (always >= 1). The top frame's index is
    /// `depth() - 1`.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Seal by-name resolution below `floor`: `get`/`get_mut` ignore every
    /// frame with index < `floor`. Returns the previous floor so the caller
    /// can restore it. See the `floor` field.
    pub fn set_floor(&mut self, floor: usize) -> usize {
        let prev = self.floor;
        self.floor = floor.min(self.frames.len());
        self.bump();
        prev
    }

    /// The current seal (see [`set_floor`](Self::set_floor)).
    pub fn floor(&self) -> usize {
        self.floor
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        let sym = crate::intern::intern(key);
        self.get_sym(sym)
    }

    pub fn get_sym(&self, key: Sym) -> Option<&V> {
        for frame in self.frames[self.floor..].iter().rev() {
            if let Some(v) = frame.entries.get(&key) {
                return Some(v);
            }
        }
        None
    }

    /// Index of the frame [`get`](Self::get) finds `key` in: the innermost
    /// frame at or above the floor that binds it.
    pub fn visible_frame(&self, key: &str) -> Option<usize> {
        let sym = crate::intern::intern(key);
        (self.floor..self.frames.len())
            .rev()
            .find(|&i| self.frames[i].entries.contains_key(&sym))
    }

    /// Every binding of `key` by-name resolution can see, innermost first,
    /// with the index of its frame.
    pub fn visible_bindings(&self, key: &str) -> impl Iterator<Item = (usize, &V)> {
        let sym = crate::intern::intern(key);
        (self.floor..self.frames.len())
            .rev()
            .filter_map(move |i| self.frames[i].entries.get(&sym).map(|v| (i, v)))
    }

    /// The binding of `key` in the frame at `index`, that frame alone.
    pub fn get_in(&self, index: usize, key: Sym) -> Option<&V> {
        self.frames.get(index).and_then(|f| f.entries.get(&key))
    }

    /// Look up `key` in ONLY the innermost (current) frame, never ancestors.
    /// Distinguishes "declared in THIS scope" from "shadows an outer binding" —
    /// a statement-position `var`/`array`/`map`/`buffer` redeclaration needs its
    /// own fresh storage precisely when the outer binding is in an ancestor
    /// frame, not the current one.
    pub fn get_current(&self, key: &str) -> Option<&V> {
        let sym = crate::intern::intern(key);
        self.frames.last().and_then(|f| f.entries.get(&sym))
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        let sym = crate::intern::intern(key);
        self.get_mut_sym(sym)
    }

    pub fn get_mut_sym(&mut self, key: Sym) -> Option<&mut V> {
        self.bump();
        for frame in self.frames[self.floor..].iter_mut().rev() {
            if let Some(v) = frame.entries.get_mut(&key) {
                return Some(v);
            }
        }
        None
    }

    /// Takes `&str` deliberately: keys are interned, so an owned `String`
    /// would be allocated (or moved) only to be dropped — hot call paths
    /// (per-inlined-call scope setup) were paying one dead allocation per
    /// binding.
    pub fn insert(&mut self, key: &str, value: V) -> Option<V> {
        let sym = crate::intern::intern(key);
        self.rev = next_rev();
        self.inserts.push((self.rev, sym));
        self.frames.last_mut().unwrap().entries.insert(sym, value)
    }

    /// Bind `key` in the outermost frame to `value`, or unbind it there for
    /// `None`, returning what it bound there.
    pub fn set_root(&mut self, key: &str, value: Option<V>) -> Option<V> {
        let sym = crate::intern::intern(key);
        self.bump();
        let root = &mut self.frames[0].entries;
        match value {
            Some(v) => root.insert(sym, v),
            None => root.remove(&sym),
        }
    }

    /// Unbind `key` in the current frame, returning what it bound there.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let sym = crate::intern::intern(key);
        self.bump();
        self.frames.last_mut().unwrap().entries.remove(&sym)
    }

    /// Iterate all entries across all frames (newest first).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.frames.iter().rev().flat_map(|f| {
            f.entries
                .iter()
                .map(|(k, v)| (crate::intern::resolve(*k), v))
        })
    }

    /// Iterate every entry `get` can reach: the frames at or above the floor,
    /// innermost first. A name declared in two open frames is yielded once per
    /// frame, so a caller that cares about distinct names dedupes by key.
    pub fn iter_visible(&self) -> impl Iterator<Item = (&str, &V)> {
        self.frames[self.floor..].iter().rev().flat_map(|f| {
            f.entries
                .iter()
                .map(|(k, v)| (crate::intern::resolve(*k), v))
        })
    }

    /// Index of the innermost frame binding `key` to a value `pred` accepts.
    /// A frame whose `key` fails `pred` is passed over, so a same-named
    /// binding shadowing the wanted one does not hide it.
    pub fn frame_of(&self, key: Sym, pred: impl Fn(&V) -> bool) -> Option<usize> {
        self.frames
            .iter()
            .enumerate()
            .rev()
            .find(|(_, f)| f.entries.get(&key).is_some_and(&pred))
            .map(|(i, _)| i)
    }

    /// Index of the innermost frame holding any entry `pred` accepts.
    pub fn frame_where(&self, pred: impl Fn(&V) -> bool) -> Option<usize> {
        self.frames
            .iter()
            .enumerate()
            .rev()
            .find(|(_, f)| f.entries.values().any(&pred))
            .map(|(i, _)| i)
    }

    /// Take every frame above index `keep` off the stack, for
    /// [`reattach`](Self::reattach) to put back. Lookups meanwhile see only
    /// the frames up to `keep`.
    pub fn detach_above(&mut self, keep: usize) -> Detached<V> {
        let at = (keep + 1).clamp(1, self.frames.len());
        let frames = self.frames.split_off(at);
        self.floor = self.floor.min(self.frames.len());
        self.bump();
        Detached { frames }
    }

    /// Put back frames taken by [`detach_above`](Self::detach_above), above
    /// whatever the stack holds now.
    pub fn reattach(&mut self, detached: Detached<V>) {
        self.frames.extend(detached.frames);
        self.bump();
    }

    /// Push a frame holding `entries` that stands for frame `id`: a copy of
    /// it, which a later [`frames`](Self::frames) reports under the same id.
    pub fn push_as(&mut self, tag: ScopeTag, id: u64, entries: impl IntoIterator<Item = (Sym, V)>) {
        self.frames.push(Frame {
            tag,
            id,
            entries: entries.into_iter().collect(),
        });
        self.bump();
    }

    /// The bindings of the frame at `index`.
    pub fn frame_entries(&self, index: usize) -> impl Iterator<Item = (Sym, &V)> {
        self.frames
            .get(index)
            .into_iter()
            .flat_map(|f| f.entries.iter().map(|(k, v)| (*k, v)))
    }

    /// Every frame, outermost first: its tag, its id and its entries.
    pub fn frames(&self) -> impl Iterator<Item = (ScopeTag, u64, impl Iterator<Item = (Sym, &V)>)> {
        self.frames
            .iter()
            .map(|f| (f.tag, f.id, f.entries.iter().map(|(k, v)| (*k, v))))
    }

    /// A stack of the given frames, outermost first, each keeping its id.
    /// The first becomes the root whatever its tag; an empty list gives
    /// [`Scope::new`].
    pub fn from_frames(frames: Vec<FrameCopy<V>>) -> Self {
        let mut scope = Self {
            frames: Vec::new(),
            floor: 0,
            rev: 0,
            base: 0,
            inserts: Vec::new(),
        }
        .bumped();
        for (tag, id, entries) in frames {
            scope.frames.push(Frame {
                tag,
                id,
                entries: entries.into_iter().collect(),
            });
        }
        match scope.frames.first_mut() {
            Some(root) => root.tag = ScopeTag::ROOT,
            None => return Self::new(),
        }
        scope
    }

    /// Iterate entries from the top frame downward, stopping when a frame
    /// whose tag intersects `boundary` is reached (that frame IS included).
    pub fn iter_within(&self, boundary: ScopeTag) -> impl Iterator<Item = (&str, &V)> {
        let start = self
            .frames
            .iter()
            .enumerate()
            .rev()
            .find(|(_, f)| f.tag.intersects(boundary))
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.frames[start..].iter().rev().flat_map(|f| {
            f.entries
                .iter()
                .map(|(k, v)| (crate::intern::resolve(*k), v))
        })
    }
}

#[cfg(test)]
mod tests;
