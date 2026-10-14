//! The nesting of a program's modules while one is built or read: which
//! module holds which, and where a wire between two of them is stored.

use super::{Module, NodeId};

/// Modules by index, each with its parent and depth. Index 0 is the root, and
/// a module is pushed after its parent.
#[derive(Default)]
pub struct ModuleTree {
    parent: Vec<Option<usize>>,
    depth: Vec<usize>,
}

impl ModuleTree {
    /// A tree holding only the root, index 0.
    pub fn new() -> Self {
        Self { parent: vec![None], depth: vec![0] }
    }

    /// Add a module inside `parent`; returns its index.
    pub fn push(&mut self, parent: usize) -> usize {
        self.parent.push(Some(parent));
        self.depth.push(self.depth[parent] + 1);
        self.parent.len() - 1
    }

    pub fn parent(&self, m: usize) -> Option<usize> {
        self.parent[m]
    }

    pub fn depth(&self, m: usize) -> usize {
        self.depth[m]
    }

    /// The innermost module containing both `a` and `b`: where a wire between
    /// them is stored by default.
    pub fn common_module(&self, mut a: usize, mut b: usize) -> usize {
        while self.depth[a] > self.depth[b] {
            a = self.parent[a].expect("only the root has no parent");
        }
        while self.depth[b] > self.depth[a] {
            b = self.parent[b].expect("only the root has no parent");
        }
        while a != b {
            a = self.parent[a].expect("only the root has no parent");
            b = self.parent[b].expect("only the root has no parent");
        }
        a
    }
}

/// Put each chip body into its parent's `chips` under its chip node and
/// return the root. `modules` is indexed like a [`ModuleTree`]; `bodies` lists
/// `(chip node, body index, parent index)`. A body comes after its parent, so
/// moving the highest index first nests every body before its parent moves.
pub fn nest_bodies(modules: Vec<Module>, mut bodies: Vec<(NodeId, usize, usize)>) -> Module {
    let mut modules: Vec<Option<Module>> = modules.into_iter().map(Some).collect();
    bodies.sort_by_key(|&(_, body, _)| std::cmp::Reverse(body));
    for (chip, body, parent) in bodies {
        let child = modules[body].take().expect("each body moves once");
        modules[parent].as_mut().expect("parents move after their bodies").chips.insert(chip, child);
    }
    modules[0].take().expect("the root never moves")
}
