use std::fmt::Display;

use indexmap::IndexSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrdbInterned(pub(crate) usize);

#[derive(Default)]
pub struct BrdbIntern {
    inner: IndexSet<String>,
}

impl BrdbIntern {
    pub fn get_or_insert(&mut self, value: impl AsRef<str> + Display) -> BrdbInterned {
        if let Some(index) = self.inner.get_index_of(value.as_ref()) {
            return BrdbInterned(index);
        }
        let index = self.inner.len();
        self.inner.insert(value.to_string());
        BrdbInterned(index)
    }

    pub fn lookup(&self, interned: BrdbInterned) -> Option<String> {
        self.inner.get_index(interned.0).cloned()
    }

    pub fn lookup_ref(&self, interned: BrdbInterned) -> Option<&str> {
        self.inner.get_index(interned.0).map(String::as_str)
    }

    pub fn get(&self, name: &str) -> Option<BrdbInterned> {
        self.inner.get_index_of(name).map(BrdbInterned)
    }
}
