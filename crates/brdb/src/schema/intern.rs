use std::fmt::Display;

use indexmap::IndexSet;

use crate::{errors::BrdbSchemaError, schema::BrdbSchema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrdbInterned(pub(crate) usize);

#[derive(Default, Clone, Debug)]
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

impl BrdbInterned {
    pub fn get(self, schema: &BrdbSchema) -> Option<&str> {
        schema.intern.lookup_ref(self)
    }

    pub fn get_or<'b, 'a: 'b>(self, schema: &'a BrdbSchema, or: &'b str) -> &'b str {
        schema.intern.lookup_ref(self).unwrap_or(or)
    }

    pub fn get_or_else(self, schema: &BrdbSchema, or: impl FnMut() -> String) -> String {
        schema.intern.lookup(self).unwrap_or_else(or)
    }

    pub fn get_ok(
        self,
        schema: &BrdbSchema,
        or: impl FnMut() -> BrdbSchemaError,
    ) -> Result<&str, BrdbSchemaError> {
        schema.intern.lookup_ref(self).ok_or_else(or)
    }
}
