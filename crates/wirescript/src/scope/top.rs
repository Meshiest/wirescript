//! What a free name means at the top level of a file. Typecheck, lowering,
//! the constant scope and the editor's resolver all decide it here.
//!
//! The top level of a file is one scope. An anonymous `chip { }` written at
//! the top level (or inside one that is) shares it, except for the value
//! names the chip declares itself (storage, `let` and `const`): inside the
//! chip such a name is the chip's declaration, in the statements written
//! there and in the body of every `mod` or `chip` written there, wherever
//! that is called from. Everywhere else a top-level declaration of the name
//! is what it means, and a name only a chip declares is that chip's
//! file-wide. Two declarations of one name in a chip are a duplicate
//! (`WS013`). Two at the top level itself are read by position: a read names
//! the latest declaration visible where it is written, so a later one hides
//! an earlier one from its position on (`WS108`, see
//! [`TopNames::visible_decl`]). Storage is declared ahead of everything, so a
//! read above every declaration of the name names it too.

use std::sync::Arc;

use crate::ast::{Block, Stmt, TopDecl};
use crate::collections::HashMap;
use crate::const_eval::bound_names;
use crate::diagnostic::SourceRange;

/// An anonymous chip of the top level and the value names it declares.
#[derive(Clone, Debug)]
pub(crate) struct TopChip {
    /// The whole chip, `chip { .. }`.
    pub(crate) range: SourceRange,
    pub(crate) names: Vec<String>,
}

/// Where a name resolves (see [`TopNames::resolve_name`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Home<'t> {
    /// The declaration of a chip written around the place.
    Chip(&'t TopChip),
    /// The top level's declaration, or failing one, a chip's from anywhere.
    Top,
}

/// One of several declarations of a name at the top level of a file.
#[derive(Clone, Debug)]
pub(crate) struct TopDup {
    /// The whole declaration.
    pub(crate) range: SourceRange,
    /// The offset the declaration is visible from: storage from its start
    /// (its own initializer may read it), a `let` or `const` from its end.
    visible_from: usize,
    /// Storage (`var`, `buffer`, `array`, `map`) rather than a `let`.
    pub(crate) storage: bool,
}

/// A place the top level is read from: code written at `range`, or, with
/// `body` set, the body of a `mod`, `chip` or generic declared at `range`,
/// which is read wherever it is called from (see [`TopNames::visible_decl`]).
#[derive(Clone, Debug)]
pub(crate) struct ReadAt {
    pub(crate) range: SourceRange,
    pub(crate) body: bool,
}

impl ReadAt {
    /// Code written at `range`.
    pub(crate) fn code(range: &SourceRange) -> Self {
        Self {
            range: range.clone(),
            body: false,
        }
    }

    /// The body of a declaration written at `range`.
    pub(crate) fn body(range: &SourceRange) -> Self {
        Self {
            range: range.clone(),
            body: true,
        }
    }

    /// Code written just above `range`: what the initializer of the storage
    /// declared there reads, the declarations before it and not the storage.
    pub(crate) fn above(range: &SourceRange) -> Self {
        let mut range = range.clone();
        range.start.offset = range.start.offset.saturating_sub(1);
        Self { range, body: false }
    }
}

/// The anonymous chips of a program's top level, outermost first, and the
/// declaration of each top-level value by its file and name.
#[derive(Clone, Debug, Default)]
pub(crate) struct TopNames {
    chips: Vec<TopChip>,
    /// Each chip's index in `chips` by its range, the first when two share one.
    chip_index: HashMap<SourceRange, usize>,
    values: HashMap<(Arc<str>, String), SourceRange>,
    /// Each name the top level of one file (outside every chip) declares more
    /// than once, with its declarations in source order.
    dups: HashMap<(Arc<str>, String), Vec<TopDup>>,
}

/// Whether `inner` is written inside `outer`.
pub(crate) fn within(outer: &SourceRange, inner: &SourceRange) -> bool {
    outer.contains(inner)
}

/// The value names the top-level declaration `d` declares that a later
/// declaration of the top level may declare again, each with whether it is
/// storage: a `let` or `const`, a `var`, `buffer`, `array` or `map`.
pub(crate) fn declared_names_of(d: &TopDecl) -> Vec<(String, bool)> {
    match d {
        TopDecl::Let(l) => bound_names(&l.binding)
            .into_iter()
            .map(|n| (n, false))
            .collect(),
        TopDecl::Var(v) => vec![(v.name.clone(), true)],
        TopDecl::Array(a) => vec![(a.name.clone(), true)],
        TopDecl::Map(m) => vec![(m.name.clone(), true)],
        TopDecl::Buffer(b) => vec![(b.name.clone(), true)],
        _ => Vec::new(),
    }
}

/// The value names a chip body of `stmts` declares itself.
pub(crate) fn declared_names(stmts: &[Stmt]) -> Vec<String> {
    let mut out = Vec::new();
    for s in stmts {
        match s {
            Stmt::Var(v) => out.push(v.name.clone()),
            Stmt::Array(a) => out.push(a.name.clone()),
            Stmt::Map(m) => out.push(m.name.clone()),
            Stmt::Buffer(b) => out.push(b.name.clone()),
            Stmt::Let(l) => out.extend(bound_names(&l.binding)),
            _ => {}
        }
    }
    out
}

impl TopNames {
    /// The chips the top level `decls` writes, nested ones included, and its
    /// values.
    pub(crate) fn of(decls: &[TopDecl]) -> Self {
        fn walk(block: &Block, chips: &mut Vec<TopChip>) {
            for s in &block.stmts {
                if let Stmt::AnonChip(ac) = s {
                    chips.push(TopChip {
                        range: ac.range.clone(),
                        names: declared_names(&ac.body.stmts),
                    });
                    walk(&ac.body, chips);
                }
            }
        }
        let mut chips = Vec::new();
        let mut values = HashMap::default();
        let mut dups: HashMap<(Arc<str>, String), Vec<TopDup>> = HashMap::default();
        for d in decls {
            let r = d.range();
            for (name, storage) in declared_names_of(d) {
                let visible_from = if storage {
                    r.start.offset
                } else {
                    r.end.offset
                };
                let dup = TopDup {
                    range: r.clone(),
                    visible_from,
                    storage,
                };
                dups.entry((Arc::clone(&r.file), name))
                    .or_default()
                    .push(dup);
            }
        }
        dups.retain(|_, v| v.len() > 1);
        // Imports reach a module's declarations in the order the importer
        // names them (a named import before a flat one): keep each name's in
        // the order its own file writes them.
        for v in dups.values_mut() {
            v.sort_by_key(|d| d.range.start.offset);
        }
        for d in decls {
            let names = match d {
                TopDecl::AnonChip(ac) => {
                    chips.push(TopChip {
                        range: ac.range.clone(),
                        names: declared_names(&ac.body.stmts),
                    });
                    walk(&ac.body, &mut chips);
                    Vec::new()
                }
                TopDecl::Let(l) => bound_names(&l.binding),
                TopDecl::Var(v) => vec![v.name.clone()],
                TopDecl::Array(a) => vec![a.name.clone()],
                TopDecl::Map(m) => vec![m.name.clone()],
                TopDecl::Buffer(b) => vec![b.name.clone()],
                TopDecl::In(i) => vec![i.name.clone()],
                TopDecl::Out(o) => vec![o.name.clone()],
                _ => Vec::new(),
            };
            for name in names {
                values
                    .entry((Arc::clone(&d.range().file), name))
                    .or_insert_with(|| d.range().clone());
            }
        }
        let mut chip_index = HashMap::default();
        for (i, c) in chips.iter().enumerate() {
            chip_index.entry(c.range.clone()).or_insert(i);
        }
        Self {
            chips,
            chip_index,
            values,
            dups,
        }
    }

    /// Only the names declared more than once at the top level of a file:
    /// what a chip body compiled on its own reads the top level by.
    pub(crate) fn dups_only(&self) -> Self {
        Self {
            dups: self.dups.clone(),
            ..Default::default()
        }
    }

    /// Whether some file declares a name more than once at its top level.
    pub(crate) fn has_dups(&self) -> bool {
        !self.dups.is_empty()
    }

    /// Every name declared more than once at the top level of a file, with
    /// its declarations in source order.
    pub(crate) fn dups(&self) -> impl Iterator<Item = (&str, &[TopDup])> {
        self.dups
            .iter()
            .map(|((_, name), v)| (name.as_str(), v.as_slice()))
    }

    /// Whether `name` is declared more than once at the top level of `file`.
    pub(crate) fn is_dup(&self, file: &Arc<str>, name: &str) -> bool {
        self.dups
            .contains_key(&(Arc::clone(file), name.to_string()))
    }

    /// For a name the top level of `at`'s file declares more than once, the
    /// declaration a read at `at` names: the latest one visible there (a later
    /// declaration hides an earlier one from its position on; storage is
    /// visible from its start, a `let` or `const` from its end). Above every
    /// one of them, the storage, which the top level declares ahead of
    /// everything else; with none, a read in a body (a `mod`, `chip` or
    /// generic declared above every declaration of the name) names the first
    /// declaration, as it names the only one when there is one, and a read in
    /// code names none. The outer `None` means `name` is not declared more
    /// than once there; the inner, that no declaration of it is visible at
    /// `at` yet.
    #[expect(
        clippy::option_option,
        reason = "outer `None` means not a duplicate name; inner, none visible yet"
    )]
    pub(crate) fn visible_decl(&self, name: &str, at: &ReadAt) -> Option<Option<&TopDup>> {
        let decls = self
            .dups
            .get(&(Arc::clone(&at.range.file), name.to_string()))?;
        let seen = decls
            .iter()
            .rev()
            .find(|d| d.visible_from <= at.range.start.offset);
        let storage = || decls.iter().find(|d| d.storage);
        Some(
            seen.or_else(storage)
                .or_else(|| decls.first().filter(|_| at.body)),
        )
    }

    /// Where the initializer of the top-level `var` declared at `range`
    /// reads the top level, when its file declares `name` more than once
    /// there: above the declaration, so it names the one visible before it,
    /// as a `let` initializer does (a `buffer` reads itself, the previous
    /// tick). `None` for any other declaration.
    pub(crate) fn initializer_read(&self, name: &str, range: &SourceRange) -> Option<ReadAt> {
        let decls = self
            .dups
            .get(&(Arc::clone(&range.file), name.to_string()))?;
        decls
            .iter()
            .any(|d| d.storage && d.range == *range)
            .then(|| ReadAt::above(range))
    }

    /// Whether code written in `file` names the top-level value `name`
    /// declared in `decl_file`: it declares it, or its imports bring it in,
    /// as for a type or a `mod` (see [`crate::ast::ImportScope::sees`]).
    pub(crate) fn file_sees(
        &self,
        imports: &crate::ast::ImportScope,
        file: &str,
        decl_file: &Arc<str>,
        name: &str,
    ) -> bool {
        match self.values.get(&(Arc::clone(decl_file), name.to_string())) {
            Some(decl) => imports.sees(file, decl),
            None => true,
        }
    }

    /// What `name`, written at `at` (in a `mod` or `chip` body, anywhere in
    /// it), means at the top level: the declaration of the innermost chip
    /// around `at` that declares it, otherwise the top level's.
    pub(crate) fn resolve_name(&self, name: &str, at: &SourceRange) -> Home<'_> {
        self.chips
            .iter()
            .rev()
            .find(|c| c.range.contains(at) && c.names.iter().any(|n| n == name))
            .map_or(Home::Top, Home::Chip)
    }

    /// The chips written around `at`, outermost first.
    pub(crate) fn chips_around<'t>(
        &'t self,
        at: &'t SourceRange,
    ) -> impl Iterator<Item = &'t TopChip> + 't {
        self.chips.iter().filter(move |c| c.range.contains(at))
    }

    /// The chip written exactly at `range`.
    pub(crate) fn chip_at(&self, range: &SourceRange) -> Option<&TopChip> {
        self.chip_index.get(range).map(|&i| &self.chips[i])
    }

    /// Whether declarations written at `a` and `b` are in one scope of the
    /// top level: both outside every chip, or both written directly in the
    /// same chip.
    pub(crate) fn same_level(&self, a: &SourceRange, b: &SourceRange) -> bool {
        let innermost = |r: &SourceRange| self.chips.iter().rev().position(|c| c.range.contains(r));
        innermost(a) == innermost(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> (crate::ast::Script, TopNames) {
        let parsed = crate::parser::parse(src, "t");
        let top = TopNames::of(&parsed.ast.decls);
        (parsed.ast, top)
    }

    fn at(src: &str, needle: &str) -> SourceRange {
        let start = src.find(needle).expect(needle);
        let pos = |o: usize| crate::diagnostic::Pos {
            offset: o,
            line: 0,
            col: 0,
        };
        SourceRange::new("t", pos(start), pos(start + needle.len()))
    }

    #[test]
    fn a_name_resolves_to_the_chip_written_around_it() {
        let src = "let x = 4\nchip { var x: int = 1\n mod rd() -> int { return x * 10 }\n chip { const k = 2 } }\nmod out() -> int { return x }\n";
        let (_, top) = names(src);
        assert!(
            matches!(top.resolve_name("x", &at(src, "x * 10")), Home::Chip(c) if c.names == ["x"])
        );
        assert!(matches!(
            top.resolve_name("x", &at(src, "return x }\n")),
            Home::Top
        ));
        assert!(
            matches!(top.resolve_name("k", &at(src, "const k")), Home::Chip(c) if c.names == ["k"])
        );
        assert!(matches!(
            top.resolve_name("y", &at(src, "x * 10")),
            Home::Top
        ));
        assert!(top.same_level(&at(src, "let x"), &at(src, "mod out")));
        assert!(!top.same_level(&at(src, "let x"), &at(src, "var x")));
        assert!(!top.same_level(&at(src, "var x"), &at(src, "const k")));
        assert_eq!(top.chips_around(&at(src, "const k")).count(), 2);
    }

    #[test]
    fn a_name_declared_twice_names_the_declaration_visible_where_it_is_read() {
        let src = "on a { }\nlet k = 1\non b { }\nvar k: int = 2\non c { }\nconst j = 1\nconst j = k\non d { }\n";
        let (_, top) = names(src);
        let named = |name: &str, place: &str, body: bool| {
            let read = if body {
                ReadAt::body(&at(src, place))
            } else {
                ReadAt::code(&at(src, place))
            };
            top.visible_decl(name, &read)
                .map(|d| d.map(|d| d.range.start.offset))
        };
        let (let_k, var_k) = (src.find("let k").unwrap(), src.find("var k").unwrap());
        // Storage is declared ahead of everything; a `let` from its end on.
        assert_eq!(named("k", "on a", false), Some(Some(var_k)));
        assert_eq!(named("k", "on b", false), Some(Some(let_k)));
        assert_eq!(named("k", "on c", false), Some(Some(var_k)));
        // With no storage, code above both names none and a body the first.
        let (j1, j2) = (
            src.find("const j = 1").unwrap(),
            src.find("const j = k").unwrap(),
        );
        assert_eq!(named("j", "on a", false), Some(None));
        assert_eq!(named("j", "on a", true), Some(Some(j1)));
        assert_eq!(named("j", "const j = k", false), Some(Some(j1)));
        assert_eq!(named("j", "on d", true), Some(Some(j2)));
        assert_eq!(named("x", "on d", false), None);
    }
}
