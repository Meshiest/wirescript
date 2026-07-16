use crate::diagnostic::SourceRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRange {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    /// True if this reference is a record literal shorthand (`{ name }` not `{ name: expr }`)
    pub is_shorthand: bool,
}

pub fn find_all_references(source: &str, name: &str) -> Vec<TextRange> {
    let mut results = Vec::new();
    for (line_num, line) in source.lines().enumerate() {
        let mut start = 0;
        while let Some(pos) = line[start..].find(name) {
            let abs = start + pos;
            let before = if abs > 0 { line.as_bytes().get(abs - 1).copied() } else { None };
            let after = line.as_bytes().get(abs + name.len()).copied();
            let wb = before.map(|c| c.is_ascii_alphanumeric() || c == b'_').unwrap_or(false);
            let wa = after.map(|c| c.is_ascii_alphanumeric() || c == b'_').unwrap_or(false);
            if !wb && !wa {
                let is_shorthand = is_record_shorthand(line, abs, name.len());
                results.push(TextRange { start_line: line_num, start_col: abs, end_line: line_num, end_col: abs + name.len(), is_shorthand });
            }
            start = abs + name.len();
        }
    }
    results
}

fn is_record_shorthand(line: &str, pos: usize, name_len: usize) -> bool {
    // `import { foo } from "…"` braces are a specifier list, not a record
    // literal — a shorthand expansion there (`import { foo: bar }`) corrupts
    // the import statement on rename.
    let trimmed_line = line.trim_start();
    if trimmed_line.starts_with("import ") || trimmed_line.starts_with("import{") {
        return false;
    }
    let after_name = &line[pos + name_len..];
    let after_trimmed = after_name.trim_start();
    if after_trimmed.starts_with(':') {
        return false;
    }
    let before = &line[..pos];
    // A shorthand field is introduced by the record's `{` or a preceding
    // field's `,`. A name in *value* position (`{ x: foo }`) is preceded by
    // `:` and must not read as shorthand — treating it as one turned a
    // rename of `foo` into the bogus `{ x: foo: newName }`.
    if !matches!(before.trim_end().chars().last(), Some('{') | Some(',')) {
        return false;
    }
    let mut depth = 0i32;
    for ch in before.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    if depth <= 0 {
        return false;
    }
    after_trimmed.is_empty()
        || after_trimmed.starts_with(',')
        || after_trimmed.starts_with('}')
}

/// The replacement text for one rename site. Rename must consume exactly the
/// site set [`find_all_references`] returns; this maps each site to its new
/// text. A record-literal shorthand (`{ name }`) keeps its field name and
/// binds the renamed value — `{ name }` → `{ name: new_name }` — every other
/// site is replaced outright.
pub fn rename_edit_text(site: &TextRange, old_name: &str, new_name: &str) -> String {
    if site.is_shorthand {
        format!("{old_name}: {new_name}")
    } else {
        new_name.to_string()
    }
}

pub fn find_name_range(source: &str, decl_range: &SourceRange, name: &str) -> Option<SourceRange> {
    let line_idx = decl_range.start.line.saturating_sub(1) as usize;
    let line = source.lines().nth(line_idx)?;
    let col_start = decl_range.start.col.saturating_sub(1) as usize;
    if col_start > line.len() { return None; }
    let search_from = &line[col_start..];
    let pos = search_from.find(name)?;
    let abs_col = col_start + pos;
    Some(SourceRange {
        file: decl_range.file.clone(),
        start: crate::diagnostic::Pos { offset: 0, line: decl_range.start.line, col: abs_col as u32 + 1 },
        end: crate::diagnostic::Pos { offset: 0, line: decl_range.start.line, col: (abs_col + name.len()) as u32 + 1 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply a rename over `source` exactly the way the LSP does: collect the
    /// sites with [`find_all_references`], then replace each with
    /// [`rename_edit_text`] (right-to-left so earlier ranges stay valid).
    fn apply_rename(source: &str, old: &str, new: &str) -> String {
        let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
        let mut sites = find_all_references(source, old);
        sites.sort_by(|a, b| (b.start_line, b.start_col).cmp(&(a.start_line, a.start_col)));
        for site in sites {
            lines[site.start_line]
                .replace_range(site.start_col..site.end_col, &rename_edit_text(&site, old, new));
        }
        lines.join("\n")
    }

    #[test]
    fn rename_at_definition_renames_all_uses() {
        let src = "mod foo(v: int) {\n}\nin go: exec\non go {\n  foo(1)\n  foo(2)\n}";
        let out = apply_rename(src, "foo", "bar");
        assert_eq!(
            out,
            "mod bar(v: int) {\n}\nin go: exec\non go {\n  bar(1)\n  bar(2)\n}"
        );
    }

    #[test]
    fn rename_from_use_site_renames_definition_and_all_uses() {
        // Start from a *use* site the way the LSP does (word under cursor),
        // and confirm the definition site is part of the renamed set.
        let src = "let foo = 1\nout a = foo + foo";
        let col = src.lines().nth(1).unwrap().rfind("foo").unwrap();
        let word = crate::analysis::word_at(src, 1, col).expect("word under cursor");
        assert_eq!(word, "foo");
        let out = apply_rename(src, &word, "bar");
        assert_eq!(out, "let bar = 1\nout a = bar + bar");
    }

    #[test]
    fn rename_does_not_touch_longer_identifiers() {
        let src = "let foo = 1\nlet foobar = foo + food";
        let out = apply_rename(src, "foo", "bar");
        assert_eq!(out, "let bar = 1\nlet foobar = bar + food");
    }

    #[test]
    fn rename_imported_symbol_updates_import_specifier_and_definition() {
        use crate::resolve::{resolve, MemLoader};

        // Renaming `foo` from the importing file must rewrite BOTH the
        // `import { foo }` specifier here and the definition in the source
        // file — the same two-file site set find-references returns.
        let main_src = "import { foo } from \"util\"\nin go: exec\non go { foo() }";
        let util_src = "mod foo() {\n}";

        let renamed_main = apply_rename(main_src, "foo", "bar");
        let renamed_util = apply_rename(util_src, "foo", "bar");

        assert!(
            renamed_main.contains("import { bar } from \"util\""),
            "import specifier not renamed: {renamed_main}"
        );
        assert!(renamed_main.contains("on go { bar() }"), "{renamed_main}");
        assert!(renamed_util.contains("mod bar()"), "definition not renamed: {renamed_util}");

        // The renamed pair still resolves cleanly end-to-end.
        let loader = MemLoader {
            files: [("util.ws".to_string(), renamed_util)].into_iter().collect(),
        };
        let resolved = resolve(&renamed_main, "main", &loader);
        let errors: Vec<_> = resolved
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "renamed sources no longer resolve: {errors:?}");
    }

    #[test]
    fn rename_covers_namespace_qualified_uses() {
        let src = "import * as u from \"util\"\nin go: exec\non go { u.foo() }";
        let out = apply_rename(src, "foo", "bar");
        assert!(out.contains("u.bar()"), "qualified use not renamed: {out}");
    }

    #[test]
    fn record_shorthand_rename_keeps_field_name() {
        // `{ foo }` binds field `foo` to the value of `foo`; renaming the
        // value must keep the field name: `{ foo: bar }` (NOT `{ bar: foo }`).
        let src = "let p = { foo, other: 1 }";
        let out = apply_rename(src, "foo", "bar");
        assert_eq!(out, "let p = { foo: bar, other: 1 }");
    }

    #[test]
    fn record_shorthand_after_comma_expands_too() {
        let src = "let p = { other: 1, foo }";
        let out = apply_rename(src, "foo", "bar");
        assert_eq!(out, "let p = { other: 1, foo: bar }");
    }

    #[test]
    fn record_value_position_is_not_shorthand() {
        // `{ x: foo }` binds field `x` to the value `foo` — renaming `foo`
        // must not fabricate a shorthand expansion (`{ x: foo: bar }`).
        let src = "let p = { x: foo }";
        let out = apply_rename(src, "foo", "bar");
        assert_eq!(out, "let p = { x: bar }");
    }

    #[test]
    fn import_specifier_is_not_shorthand() {
        // The braces of an import are a specifier list, not a record literal;
        // renaming `foo` must yield `import { bar, other }`, never the
        // corrupted `import { foo: bar, other }`.
        let sites = find_all_references("import { foo, other } from \"util\"", "foo");
        assert_eq!(sites.len(), 1);
        assert!(!sites[0].is_shorthand, "import specifier misread as record shorthand");
    }

    #[test]
    fn record_field_position_renames_plainly() {
        // The explicit `foo:` field name is a plain (non-shorthand) site.
        let src = "let p = { foo: 1 }";
        let out = apply_rename(src, "foo", "bar");
        assert_eq!(out, "let p = { bar: 1 }");
    }
}
