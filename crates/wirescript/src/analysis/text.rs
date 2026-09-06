/// Byte index just past the char at byte `i` of `s`. `i + 1` is only correct
/// for ASCII — after an `rfind` that landed on a multi-byte char (e.g. `─` in
/// a comment), `i + 1` is not a char boundary and slicing there panics.
pub fn after_char(s: &str, i: usize) -> usize {
    i + s[i..].chars().next().map_or(1, |c| c.len_utf8())
}

/// The text of 0-based `line`, without its line terminator. CRLF and LF files
/// both yield the same string.
pub fn line_text(source: &str, line: usize) -> &str {
    source.lines().nth(line).unwrap_or("")
}

/// Byte offset where 0-based `line` starts in `source`, counting the newline
/// bytes as they actually appear. `str::lines` strips the `\r` of a CRLF pair,
/// so summing `l.len() + 1` loses one byte per line: on a CRLF file every
/// offset past line 1 drifts, and slicing at one lands mid-character and
/// panics. Past the last line this returns `source.len()`.
pub fn line_start_byte(source: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    let mut seen = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            seen += 1;
            if seen == line {
                return i + 1;
            }
        }
    }
    source.len()
}

/// Byte offset within one line's text of 0-based char column `col`, clamped to
/// the end of the line. Every cursor coordinate reaching `analysis` is a char
/// column; slicing with it directly is only correct while the line is ASCII.
pub fn char_col_to_byte(line_str: &str, col: usize) -> usize {
    line_str.char_indices().nth(col).map_or(line_str.len(), |(b, _)| b)
}

/// 0-based char column of byte offset `b` within one line's text.
pub fn byte_to_char_col(line_str: &str, b: usize) -> usize {
    let b = b.min(line_str.len());
    line_str[..b].chars().count()
}

/// UTF-16 code-unit column (what the LSP protocol carries by default) to the
/// char column `analysis` works in. Astral-plane characters, emoji, most
/// notably, are two UTF-16 units and one char, so the two disagree on every
/// line containing one.
pub fn utf16_col_to_char_col(line_str: &str, u16col: usize) -> usize {
    let mut units = 0usize;
    for (chars, c) in line_str.chars().enumerate() {
        if units >= u16col {
            return chars;
        }
        units += c.len_utf16();
    }
    line_str.chars().count()
}

/// Char column to the UTF-16 code-unit column the LSP protocol expects back.
pub fn char_col_to_utf16_col(line_str: &str, col: usize) -> usize {
    line_str.chars().take(col).map(char::len_utf16).sum()
}

pub fn word_at(source: &str, line: usize, col: usize) -> Option<String> {
    let l = source.lines().nth(line)?;
    // Convert character column to byte offset safely
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let start = l[..c].rfind(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| after_char(l, i)).unwrap_or(0);
    let end = l[c..].find(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| c + i).unwrap_or(l.len());
    let w = &l[start..end];
    if w.is_empty() { None } else { Some(w.to_string()) }
}

/// If the cursor sits in a `receiver.partial` member-access position — the text
/// just before the cursor is `<ident>.<zero-or-more ident chars>` — return the
/// receiver identifier. Returns `None` at an argument boundary like `Call(`,
/// where the nearest `.` belongs to the callee and the cursor is not typing a
/// member, so param completion can take over. Used by every editor frontend to
/// give member completions priority over the enclosing call's param names.
pub fn member_receiver_at(source: &str, line: usize, col: usize) -> Option<String> {
    let l = source.lines().nth(line)?;
    // `col` is a char column — convert to a byte offset (a raw byte index
    // would slice mid-char and panic on multi-byte text earlier in the line).
    let col_idx = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let before = &l[..col_idx];
    // The partial member name being typed (identifier chars adjacent to cursor).
    let frag_start = before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| after_char(before, i))
        .unwrap_or(0);
    // The char immediately before that fragment must be the member dot.
    if frag_start == 0 || before.as_bytes()[frag_start - 1] != b'.' {
        return None;
    }
    // The receiver is the identifier directly before the dot (no whitespace).
    let recv = &before[..frag_start - 1];
    // An indexed read (`arr[i].`) is a receiver too: its members are the array
    // get gate's outputs, not the array's methods. Skip the bracketed index and
    // report the base as `name[]` so the caller can tell the two apart.
    if recv.ends_with(']') {
        let mut depth = 0i32;
        let mut open = None;
        for (i, c) in recv.char_indices().rev() {
            match c {
                ']' => depth += 1,
                '[' => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let open = open?;
        let base = &recv[..open];
        let base_start = base
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| after_char(base, i))
            .unwrap_or(0);
        let name = &base[base_start..];
        return (!name.is_empty()).then(|| format!("{name}[]"));
    }
    let recv_start = recv
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| after_char(recv, i))
        .unwrap_or(0);
    let name = &recv[recv_start..];
    (!name.is_empty()).then(|| name.to_string())
}

/// Names of the `name: type` entries in `inner`, split on top-level commas so a
/// nested record/tuple/array field type isn't mis-split. Each entry's name is
/// the text before its first `:`.
fn field_entry_names(inner: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let push = |seg: &str, out: &mut Vec<String>| {
        let name = seg.split(':').next().unwrap_or("").trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
    };
    for (i, b) in inner.bytes().enumerate() {
        match b {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                push(&inner[start..i], &mut fields);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&inner[start..], &mut fields);
    fields
}

/// Field names of a record type string like `{Forward: float, Jump: bool}` (the
/// shape `analysis::type_str` prints for `Type::Record`), or `None` if `ty`
/// isn't a record.
pub fn record_field_names(ty: &str) -> Option<Vec<String>> {
    let inner = ty.strip_prefix('{')?.strip_suffix('}')?;
    let fields = field_entry_names(inner);
    (!fields.is_empty()).then_some(fields)
}

/// Parameter names of a callable signature string like
/// `(dist: int, fast: bool) -> int` (the shape a mod/chip/fn `SymbolDef.ty`
/// holds), or `None` if `sig` has no parameter list. Returns an empty `Vec` for
/// a zero-param callable (`()`), which the caller distinguishes from `None`.
pub fn param_names(sig: &str) -> Option<Vec<String>> {
    let open = sig.find('(')?;
    let bytes = sig.as_bytes();
    let mut depth = 0i32;
    let mut close = None;
    for i in open..sig.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    Some(field_entry_names(&sig[open + 1..close?]))
}

/// Component field names a `vector` (`x`/`y`/`z`) or `color` (`r`/`g`/`b`/`a`)
/// receiver can be swizzled by; empty for any other type. These desugar to a
/// Split gate in lowering rather than being catalog methods, so they aren't in
/// `receiver_methods` and must be offered separately.
pub fn swizzle_fields(ty: &str) -> &'static [&'static str] {
    match ty {
        "vector" => &["x", "y", "z"],
        "color" => &["r", "g", "b", "a"],
        _ => &[],
    }
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
    // Reused across lines: this scan runs on every hover, and a fresh `Vec`
    // per line was one heap allocation per line of the document per request.
    let mut chars: Vec<char> = Vec::new();
    for (line_no, line) in source.lines().enumerate() {
        chars.clear();
        chars.extend(line.chars());
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
pub fn cursor_byte_offset(source: &str, line: usize, col: usize) -> usize {
    line_start_byte(source, line) + char_col_to_byte(line_text(source, line), col)
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
        .map(|k| after_char(before, k))
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
    // An unclosed `(` in the value means the cursor descended into a nested call
    // that opened AFTER this `=` (e.g. `let v = Vec(‸)`, `foo(a = bar(‸))`), so
    // this `=` is an outer binding/arg, not the value slot the cursor sits in.
    if value.matches('(').count() > value.matches(')').count() {
        return None;
    }
    let head = before[..eq].trim_end();
    let start = head
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|k| after_char(head, k))
        .unwrap_or(0);
    let name = &head[start..];
    if name.is_empty() {
        None
    } else {
        Some((name.to_string(), value.to_string()))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod line_index_tests {
    use super::*;

    /// The index must agree with the scanning functions it replaces on every
    /// line of both line-ending styles, or a request that uses one and a
    /// request that uses the other disagree about where a token is.
    #[test]
    fn line_index_agrees_with_the_scanning_functions() {
        for src in [
            "a\nbb\nccc\n",
            "a\r\nbb\r\nccc\r\n",
            "no trailing newline",
            "",
            "\n\n\n",
            "unicode \u{e9}\u{1f600}\nsecond \u{4e16}\u{754c}\n",
        ] {
            let idx = LineIndex::new(src);
            for line in 0..6 {
                assert_eq!(
                    idx.line_text(src, line),
                    line_text(src, line),
                    "line_text disagreed at line {line} of {src:?}"
                );
                assert_eq!(
                    idx.line_start_byte(line),
                    line_start_byte(src, line),
                    "line_start_byte disagreed at line {line} of {src:?}"
                );
            }
        }
    }
}

/// Byte offset of the start of every line, computed once.
///
/// The free functions above each scan from the start of the source, which is
/// fine for the one lookup a hover does and quadratic for a request that
/// converts thousands of ranges: a semantic-token pass over a 257 KB file
/// rescanned 2.3 billion source bytes. Build one of these per request and the
/// scans become a binary search.
pub struct LineIndex {
    starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0u32];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i as u32 + 1),
        );
        Self { starts, len: source.len() as u32 }
    }

    /// Byte offset where 0-based `line` starts, or the source length past the
    /// end. Matches [`line_start_byte`] exactly, including its CRLF handling:
    /// both count the bytes as they appear.
    pub fn line_start_byte(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.len) as usize
    }

    /// The text of 0-based `line` without its terminator, matching
    /// [`line_text`]: a CRLF file yields the same string as an LF one.
    pub fn line_text<'a>(&self, source: &'a str, line: usize) -> &'a str {
        let Some(&start) = self.starts.get(line) else {
            return "";
        };
        let end = self.starts.get(line + 1).copied().unwrap_or(self.len);
        let mut slice = &source[start as usize..end as usize];
        slice = slice.strip_suffix('\n').unwrap_or(slice);
        slice.strip_suffix('\r').unwrap_or(slice)
    }
}
