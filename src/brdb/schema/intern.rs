use std::fmt::Display;

use indexmap::IndexSet;
use parking_lot::{MappedRwLockReadGuard, RwLock, RwLockReadGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrdbInterned(pub(crate) usize);

#[derive(Default)]
pub struct BrdbIntern {
    inner: RwLock<IndexSet<String>>,
}

impl BrdbIntern {
    pub fn get_or_insert(&self, value: impl AsRef<str> + Display) -> BrdbInterned {
        if let Some(index) = self.inner.read().get_index_of(value.as_ref()) {
            return BrdbInterned(index);
        }
        let mut inner = self.inner.write();
        let index = inner.len();
        inner.insert(value.to_string());
        BrdbInterned(index)
    }

    pub fn lookup(&self, interned: BrdbInterned) -> Option<String> {
        self.inner.read().get_index(interned.0).cloned()
    }

    pub fn lookup_ref(&self, interned: BrdbInterned) -> Option<MappedRwLockReadGuard<String>> {
        if self.inner.read().len() <= interned.0 {
            return None;
        }
        let lock = self.inner.read();
        Some(RwLockReadGuard::map(lock, |inner| &inner[interned.0]))
    }

    pub fn get(&self, name: &str) -> Option<BrdbInterned> {
        self.inner.read().get_index_of(name).map(BrdbInterned)
    }
}
