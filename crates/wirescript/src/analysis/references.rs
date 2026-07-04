use crate::diagnostic::SourceRange;

#[derive(Clone, Debug)]
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
    let after_name = &line[pos + name_len..];
    let after_trimmed = after_name.trim_start();
    if after_trimmed.starts_with(':') {
        return false;
    }
    let before = &line[..pos];
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
