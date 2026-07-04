use std::collections::HashMap;

use bitflags::bitflags;

use crate::intern::Sym;

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
    entries: HashMap<Sym, V>,
}

/// Lexical scope stack. Each frame has a tag and its own symbol table.
/// Lookups walk up the chain (newest first). `iter_within` stops at
/// a boundary matching the given tag flags.
///
/// Key type is `Sym` — interned symbol handle (4 bytes, Copy).
pub struct Scope<V> {
    frames: Vec<Frame<V>>,
}

impl<V> Default for Scope<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> Scope<V> {
    pub fn new() -> Self {
        Self {
            frames: vec![Frame { tag: ScopeTag::ROOT, entries: HashMap::new() }],
        }
    }

    pub fn push(&mut self, tag: ScopeTag) {
        self.frames.push(Frame { tag, entries: HashMap::new() });
    }

    pub fn pop(&mut self) {
        if self.frames.len() <= 1 {
            return;
        }
        self.frames.pop();
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        let sym = crate::intern::intern(key);
        self.get_sym(sym)
    }

    pub fn get_sym(&self, key: Sym) -> Option<&V> {
        for frame in self.frames.iter().rev() {
            if let Some(v) = frame.entries.get(&key) {
                return Some(v);
            }
        }
        None
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        let sym = crate::intern::intern(key);
        self.get_mut_sym(sym)
    }

    pub fn get_mut_sym(&mut self, key: Sym) -> Option<&mut V> {
        for frame in self.frames.iter_mut().rev() {
            if let Some(v) = frame.entries.get_mut(&key) {
                return Some(v);
            }
        }
        None
    }

    pub fn insert(&mut self, key: String, value: V) -> Option<V> {
        let sym = crate::intern::intern(&key);
        self.frames.last_mut().unwrap().entries.insert(sym, value)
    }

    pub fn insert_sym(&mut self, key: Sym, value: V) -> Option<V> {
        self.frames.last_mut().unwrap().entries.insert(key, value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.frames.iter_mut().flat_map(|f| f.entries.values_mut())
    }

    /// Iterate all entries across all frames (newest first).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.frames.iter().rev().flat_map(|f| f.entries.iter().map(|(k, v)| (crate::intern::resolve(*k), v)))
    }

    /// Iterate entries from the top frame downward, stopping when a frame
    /// whose tag intersects `boundary` is reached (that frame IS included).
    pub fn iter_within(&self, boundary: ScopeTag) -> impl Iterator<Item = (&str, &V)> {
        let start = self.frames.iter().enumerate().rev()
            .find(|(_, f)| f.tag.intersects(boundary))
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.frames[start..].iter().rev()
            .flat_map(|f| f.entries.iter().map(|(k, v)| (crate::intern::resolve(*k), v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_walks_up() {
        let mut s = Scope::new();
        s.insert("x".into(), 1);
        s.push(ScopeTag::BLOCK);
        s.insert("y".into(), 2);
        assert_eq!(s.get("x"), Some(&1));
        assert_eq!(s.get("y"), Some(&2));
    }

    #[test]
    fn inner_shadows_outer() {
        let mut s = Scope::new();
        s.insert("x".into(), 1);
        s.push(ScopeTag::BLOCK);
        s.insert("x".into(), 2);
        assert_eq!(s.get("x"), Some(&2));
        s.pop();
        assert_eq!(s.get("x"), Some(&1));
    }

    #[test]
    fn pop_discards() {
        let mut s = Scope::new();
        s.push(ScopeTag::BLOCK);
        s.insert("x".into(), 1);
        s.pop();
        assert_eq!(s.get("x"), None);
    }

    #[test]
    fn get_mut_modifies_parent() {
        let mut s = Scope::new();
        s.insert("x".into(), 1);
        s.push(ScopeTag::BLOCK);
        *s.get_mut("x").unwrap() = 99;
        s.pop();
        assert_eq!(s.get("x"), Some(&99));
    }

    #[test]
    fn iter_within_stops_at_module() {
        let mut s: Scope<&str> = Scope::new();
        s.insert("root_var".into(), "a");
        s.push(ScopeTag::MODULE);
        s.insert("mod_out".into(), "b");
        s.push(ScopeTag::BLOCK);
        s.insert("block_let".into(), "c");

        let within: Vec<_> = s.iter_within(ScopeTag::MODULE).map(|(k, _)| k).collect();
        assert!(within.contains(&"mod_out"));
        assert!(within.contains(&"block_let"));
        assert!(!within.contains(&"root_var"));
    }

    #[test]
    fn iter_within_union_stops_at_nearest() {
        let mut s: Scope<i32> = Scope::new();
        s.insert("a".into(), 1);
        s.push(ScopeTag::MODULE);
        s.insert("b".into(), 2);
        s.push(ScopeTag::BLOCK);
        s.insert("c".into(), 3);

        let within: Vec<_> = s.iter_within(ScopeTag::MODULE | ScopeTag::BLOCK)
            .map(|(k, _)| k).collect();
        assert!(within.contains(&"c"));
        assert!(!within.contains(&"b"));
    }
}
