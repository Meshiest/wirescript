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

/// Byte offset of `(line, col)` (0-based char column) within `source`.
fn cursor_byte_offset(source: &str, line: usize, col: usize) -> usize {
    let line_start: usize = source.lines().take(line).map(|l| l.len() + 1).sum();
    let line_str = source.lines().nth(line).unwrap_or("");
    let bc = line_str
        .char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(line_str.len());
    line_start + bc
}

/// Name of the call whose argument list the cursor sits inside, if any. Scans
/// the whole source up to the cursor (not just the current line) so a call
/// spread across multiple lines still resolves — the open `(` may be lines
/// above. Skips parentheses inside strings and comments.
pub fn find_enclosing_call(source: &str, line: usize, col: usize) -> Option<String> {
    let offset = cursor_byte_offset(source, line, col).min(source.len());
    let prefix = &source[..offset];
    let bytes = prefix.as_bytes();
    let mut stack: Vec<usize> = Vec::new(); // byte offsets of open '(' in real code
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
        } else if in_block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
        } else if let Some(q) = in_string {
            if c == b'\\' {
                i += 2; // skip the escaped char
            } else {
                if c == q {
                    in_string = None;
                }
                i += 1;
            }
        } else {
            match c {
                b'"' | b'\'' => in_string = Some(c),
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    in_line_comment = true;
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    in_block_comment = true;
                    i += 1;
                }
                b'(' => stack.push(i),
                b')' => {
                    stack.pop();
                }
                _ => {}
            }
            i += 1;
        }
    }
    // Innermost still-open '(': the identifier right before it is the call.
    let open = *stack.last()?;
    let before = prefix[..open].trim_end();
    let start = before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|k| k + 1)
        .unwrap_or(0);
    let name = &before[start..];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// If the cursor sits in the *value* of a `name = value` named argument on its
/// line, return `(name, value_typed_so_far)`. Returns `None` at a fresh arg
/// position (where `name` completion belongs). Drives value completion for
/// enum-valued params like `justify = "Center"`.
pub fn named_arg_value(source: &str, line: usize, col: usize) -> Option<(String, String)> {
    let line_str = source.lines().nth(line)?;
    let byte_col = line_str
        .char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(line_str.len());
    let before = &line_str[..byte_col];
    let bytes = before.as_bytes();
    // Rightmost real `=` (exclude ==, !=, <=, >=).
    let mut eq = None;
    for i in 0..bytes.len() {
        if bytes[i] == b'=' {
            let prev = if i > 0 { bytes[i - 1] } else { b' ' };
            let next = bytes.get(i + 1).copied().unwrap_or(b' ');
            if !matches!(prev, b'=' | b'!' | b'<' | b'>') && next != b'=' {
                eq = Some(i);
            }
        }
    }
    let eq = eq?;
    let value = &before[eq + 1..];
    // A comma means the value is finished / we're onto the next arg.
    if value.contains(',') {
        return None;
    }
    let head = before[..eq].trim_end();
    let start = head
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|k| k + 1)
        .unwrap_or(0);
    let name = &head[start..];
    if name.is_empty() {
        None
    } else {
        Some((name.to_string(), value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_arg_value_detects_value_slot() {
        // In the value of `justify = ...`.
        let (n, v) = named_arg_value("  justify = ", 0, 12).unwrap();
        assert_eq!(n, "justify");
        assert!(!v.contains('"'));
        // Inside an opened quote.
        let (n2, v2) = named_arg_value("  justify = \"Le", 0, 15).unwrap();
        assert_eq!(n2, "justify");
        assert!(v2.contains('"'));
        // Not a value slot (fresh arg / no '=').
        assert_eq!(named_arg_value("  fontSize", 0, 10), None);
        // `==` is not a named arg.
        assert_eq!(named_arg_value("if a == ", 0, 8), None);
    }

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
    fn enclosing_call_single_line() {
        // Cursor inside `f(a, |)` resolves to `f`.
        let src = "let x = f(a, b)";
        assert_eq!(find_enclosing_call(src, 0, 13).as_deref(), Some("f"));
        // Receiver call: `.`-qualified name resolves to the method name.
        let src2 = "on t { ctrl.DisplayText(\"hi\", fontSize = 20) }";
        assert_eq!(find_enclosing_call(src2, 0, 40).as_deref(), Some("DisplayText"));
        // Outside any call → None.
        assert_eq!(find_enclosing_call("let x = 1", 0, 8), None);
    }

    #[test]
    fn enclosing_call_multiline() {
        // A call spread across lines: the cursor on a continuation line must
        // still resolve to the call whose `(` opened lines above.
        let src = "on t {\n\
                   ctrl.DisplayText(\"hi\",\n\
                   fontSize = 20,\n\
                   outlineSize = 0,\n\
                   )\n\
                   }";
        // line 3 (`outlineSize = 0,`), cursor at end of the name.
        assert_eq!(find_enclosing_call(src, 3, 11).as_deref(), Some("DisplayText"));
        // A `(` inside a string on an earlier arg line must not break the count.
        let src2 = "f(\n\"text with ( paren\",\ng = 1\n)";
        assert_eq!(find_enclosing_call(src2, 2, 3).as_deref(), Some("f"));
    }

    #[test]
    fn asset_ref_at_pinpoints_cursor() {
        let src = "let a = $./p.brz";
        assert!(asset_ref_at(src, 0, 8).is_some()); // on '$'
        assert!(asset_ref_at(src, 0, 12).is_some()); // inside path
        assert!(asset_ref_at(src, 0, 3).is_none()); // on 'a'
    }
}
