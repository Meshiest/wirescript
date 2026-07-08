pub fn word_at(source: &str, line: usize, col: usize) -> Option<String> {
    let l = source.lines().nth(line)?;
    // Convert character column to byte offset safely
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let start = l[..c].rfind(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| i + 1).unwrap_or(0);
    let end = l[c..].find(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| c + i).unwrap_or(l.len());
    let w = &l[start..end];
    if w.is_empty() { None } else { Some(w.to_string()) }
}

/// A `$` reference token found in source: a prefab file reference
/// (`$./x.brz`, `$/abs.brz`) or an external asset reference (`$Type/Name`).
/// Line/column spans are 0-based character offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    pub line: usize,
    /// Char column of the leading `$`.
    pub start_col: usize,
    /// Char column just past the last token char.
    pub end_col: usize,
    /// Token text without the leading `$`.
    pub path: String,
}

impl AssetRef {
    /// True for a prefab file reference (`$./x.brz`, `$/abs.brz`); false for an
    /// external asset reference (`$Type/Name`).
    pub fn is_file(&self) -> bool {
        self.path.starts_with('.') || self.path.starts_with('/')
    }

    /// Does `(line, col)` fall within this reference (inclusive of both ends)?
    pub fn contains(&self, line: usize, col: usize) -> bool {
        line == self.line && col >= self.start_col && col <= self.end_col
    }
}

/// Find every `$` reference in `source`, skipping strings and comments, with
/// 0-based char line/col spans. Mirrors the lexer's rule: `$` then path chars
/// `[A-Za-z0-9_/.-]`, where the char right after `$` is an ident-start, `.`,
/// or `/` (so a bare `$` or `${...}` interpolation is not a reference).
pub fn find_asset_refs(source: &str) -> Vec<AssetRef> {
    fn is_path_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | '-')
    }
    let mut out = Vec::new();
    let mut in_string: Option<char> = None;
    let mut in_block_comment = false;
    for (line_no, line) in source.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if in_block_comment {
                if c == '*' && chars.get(i + 1) == Some(&'/') {
                    in_block_comment = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if let Some(q) = in_string {
                if c == '\\' {
                    i += 2; // skip the escaped char
                } else {
                    if c == q {
                        in_string = None;
                    }
                    i += 1;
                }
                continue;
            }
            match c {
                '"' | '\'' => {
                    in_string = Some(c);
                    i += 1;
                }
                '/' if chars.get(i + 1) == Some(&'/') => break, // line comment
                '/' if chars.get(i + 1) == Some(&'*') => {
                    in_block_comment = true;
                    i += 2;
                }
                '$' if chars
                    .get(i + 1)
                    .is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '_' | '.' | '/')) =>
                {
                    let mut j = i + 1;
                    while j < chars.len() && is_path_char(chars[j]) {
                        j += 1;
                    }
                    let path: String = chars[i + 1..j].iter().collect();
                    out.push(AssetRef { line: line_no, start_col: i, end_col: j, path });
                    i = j;
                }
                _ => i += 1,
            }
        }
        // Wirescript strings don't span lines; reset at the newline.
        in_string = None;
    }
    out
}

/// The `$` reference under `(line, col)`, if any (0-based char coordinates).
pub fn asset_ref_at(source: &str, line: usize, col: usize) -> Option<AssetRef> {
    find_asset_refs(source).into_iter().find(|r| r.contains(line, col))
}

pub fn find_enclosing_call(source: &str, line: usize, col: usize) -> Option<String> {
    let l = source.lines().nth(line)?;
    let byte_col = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let before = &l[..byte_col];
    let mut depth = 0i32;
    for ch in before.chars().rev() {
        match ch {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth < 0 {
                    let prefix = before[..before.rfind('(')?].trim_end();
                    let start = prefix.rfind(|c: char| !c.is_alphanumeric() && c != '_').map(|i| i + 1).unwrap_or(0);
                    let name = &prefix[start..];
                    return if name.is_empty() { None } else { Some(name.to_string()) };
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_prefab_and_asset_refs() {
        let src = "let a = $./p.brz\nlet b = SpawnPrefab(prefab = $/abs/x.brz)\nlet c = $Weapon/Sword";
        let refs = find_asset_refs(src);
        assert_eq!(refs.len(), 3);
        assert!(refs[0].is_file() && refs[0].path == "./p.brz" && refs[0].line == 0);
        assert!(refs[1].is_file() && refs[1].path == "/abs/x.brz");
        assert!(!refs[2].is_file() && refs[2].path == "Weapon/Sword");
        // start_col is the '$' column.
        assert_eq!(refs[0].start_col, 8);
    }

    #[test]
    fn skips_refs_in_strings_and_comments() {
        // `$./x.brz` inside a string, a line comment, and a `${}` interpolation
        // must NOT be reported; the real ref on the last line must be.
        let src = "let s = \"visit $./page.brz now\"\n// see $./notes.brz\nlet t = \"${x}\"\nlet r = $./real.brz";
        let refs = find_asset_refs(src);
        assert_eq!(refs.len(), 1, "got {refs:?}");
        assert_eq!(refs[0].path, "./real.brz");
        assert_eq!(refs[0].line, 3);
    }

    #[test]
    fn asset_ref_at_pinpoints_cursor() {
        let src = "let a = $./p.brz";
        assert!(asset_ref_at(src, 0, 8).is_some()); // on '$'
        assert!(asset_ref_at(src, 0, 12).is_some()); // inside path
        assert!(asset_ref_at(src, 0, 3).is_none()); // on 'a'
    }
}
