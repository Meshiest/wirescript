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
mod tests;
