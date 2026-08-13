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

/// The replacement text for one rename site. Rename must consume exactly the
/// site set the scoped resolver (`analysis::scoped_refs::references_at`)
/// returns; this maps each site to its new text. A record-literal shorthand
/// (`{ name }`) keeps its field name and binds the renamed value —
/// `{ name }` → `{ name: new_name }` — every other site is replaced outright.
pub fn rename_edit_text(site: &TextRange, old_name: &str, new_name: &str) -> String {
    if site.is_shorthand {
        format!("{old_name}: {new_name}")
    } else {
        new_name.to_string()
    }
}

/// True for the ASCII identifier characters `name` (always a Wirescript
/// identifier — see `lexer::is_ident_cont`) is made of. Used to bound-check a
/// candidate match so `helper` inside `helper2`/`myhelper`, or `help` as a
/// prefix of `help`/`helper` in `import { helper, help }`, is never mistaken
/// for a whole-token match.
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Find the first WHOLE-TOKEN occurrence of identifier `name` inside
/// `decl_range`, searching every line the range spans (not just its first
/// line) — a coarse declaration/import range can cover several lines (a
/// multi-line `import {\n  helper\n} from "…"`), and narrowing against only
/// `decl_range.start.line` would silently fail to find the name and fall back
/// to the whole coarse range, over-replacing it. A plain substring search
/// would also mismatch a same-named prefix/suffix (`help` inside `helper`),
/// so each candidate is checked for identifier-boundary neighbors on both
/// sides before it's accepted.
pub fn find_name_range(source: &str, decl_range: &SourceRange, name: &str) -> Option<SourceRange> {
    if name.is_empty() {
        return None;
    }
    let lines: Vec<&str> = source.lines().collect();
    let start_line = decl_range.start.line as usize;
    let end_line = decl_range.end.line as usize;
    for line_no in start_line..=end_line {
        let Some(line) = lines.get(line_no.saturating_sub(1)) else {
            continue;
        };
        let col_start = if line_no == start_line {
            decl_range.start.col.saturating_sub(1) as usize
        } else {
            0
        };
        if col_start > line.len() {
            continue;
        }
        let mut search_from = col_start;
        while search_from <= line.len() {
            let Some(rel_pos) = line[search_from..].find(name) else {
                break;
            };
            let abs_col = search_from + rel_pos;
            let before_ok = line[..abs_col]
                .chars()
                .next_back()
                .is_none_or(|c| !is_ident_char(c));
            let after_col = abs_col + name.len();
            let after_ok = line[after_col..]
                .chars()
                .next()
                .is_none_or(|c| !is_ident_char(c));
            if before_ok && after_ok {
                return Some(SourceRange {
                    file: decl_range.file.clone(),
                    start: crate::diagnostic::Pos { offset: 0, line: line_no as u32, col: abs_col as u32 + 1 },
                    end: crate::diagnostic::Pos { offset: 0, line: line_no as u32, col: after_col as u32 + 1 },
                });
            }
            // Advance past this candidate's start byte — `name` is ASCII, so
            // every byte in the match is its own char boundary.
            search_from = abs_col + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests;
