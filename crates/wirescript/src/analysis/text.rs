pub fn word_at(source: &str, line: usize, col: usize) -> Option<String> {
    let l = source.lines().nth(line)?;
    // Convert character column to byte offset safely
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let start = l[..c].rfind(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| i + 1).unwrap_or(0);
    let end = l[c..].find(|ch: char| !ch.is_alphanumeric() && ch != '_').map(|i| c + i).unwrap_or(l.len());
    let w = &l[start..end];
    if w.is_empty() { None } else { Some(w.to_string()) }
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
