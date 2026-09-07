//! Thread-safe cache for [`CompiledTemplate`]s.
//!
//! A standalone chip is compiled once and instantiated many times;
//! [`TemplateCache`] is where the compiled form is kept between those uses. It
//! is shared across a compile (and across the nested prefab compiles beneath
//! it), so every method takes `&self` and locks internally.

use crate::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::template::CompiledTemplate;

/// Thread-safe store of compiled templates, keyed by module name.
pub struct TemplateCache {
    /// Compiled templates keyed by module name (standalone chips).
    templates: RwLock<HashMap<String, Arc<CompiledTemplate>>>,
}

impl TemplateCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            templates: RwLock::new(HashMap::default()),
        }
    }

    /// Store a compiled template under `name`.
    pub fn insert(&self, name: &str, template: CompiledTemplate) {
        self.templates
            .write()
            .unwrap()
            .insert(name.to_string(), Arc::new(template));
    }

    /// Retrieve a compiled template by name, or `None` if not yet compiled.
    pub fn get(&self, name: &str) -> Option<Arc<CompiledTemplate>> {
        self.templates.read().unwrap().get(name).cloned()
    }
}

impl Default for TemplateCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
