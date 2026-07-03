pub fn format_wirescript(source: &str, tab: &str) -> String {
    let lines = source.split('\n');
    let mut result = Vec::new();
    let mut indent: i32 = 0;
    let mut paren_depth: i32 = 0;
    let mut prev_blank = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() { result.push(String::new()); prev_blank = true; }
            continue;
        }
        prev_blank = false;
        let starts_close = trimmed.starts_with('}');
        if starts_close { indent = (indent - 1).max(0); }

        let starts_close_paren = trimmed.starts_with(')');
        if starts_close_paren { paren_depth = (paren_depth - 1).max(0); }

        // `else` on its own line (not preceded by `}`) is the expression-form
        // `if cond then expr\nelse expr`, and should be indented one extra level.
        let is_expr_else = !starts_close
            && (trimmed == "else"
                || trimmed.starts_with("else ")
                || trimmed.starts_with("else\t"));

        // A line starting with a binary operator is a continuation of the previous
        // expression and should be indented one extra level.
        let binary_ops = [
            "&&", "||", "^^", "==", "!=", "<=", ">=", "<<", ">>", "**", "..",
            "&", "|", "^", "+", "-", "*", "/", "%", "<", ">",
        ];
        let is_binop_continuation = !starts_close
            && binary_ops.iter().any(|op| {
                trimmed.starts_with(op)
                    && trimmed[op.len()..].starts_with(|c: char| c.is_whitespace() || c == '(')
            });

        let extra = if is_expr_else || is_binop_continuation { 1 } else { 0 };
        let line_indent = indent + paren_depth + extra;

        result.push(format!("{}{}", tab.repeat(line_indent as usize), trimmed));

        let (brace_opens, brace_closes, paren_opens, paren_closes) = count_delimiters(trimmed);
        if starts_close { indent += brace_opens as i32; } else { indent = (indent + brace_opens as i32 - brace_closes as i32).max(0); }
        if starts_close_paren { paren_depth += paren_opens as i32; } else { paren_depth = (paren_depth + paren_opens as i32 - paren_closes as i32).max(0); }
    }
    while result.last().map(|l| l.is_empty()).unwrap_or(false) { result.pop(); }
    let mut out = result.join("\n");
    if !out.ends_with('\n') { out.push('\n'); }
    out
}

fn count_delimiters(s: &str) -> (u32, u32, u32, u32) {
    // Strip line comments before counting — delimiters inside // are not real.
    let code = strip_line_comment(s);
    let mut brace_opens = 0u32;
    let mut brace_closes = 0u32;
    let mut paren_opens = 0u32;
    let mut paren_closes = 0u32;
    let mut in_str = false;
    let mut str_ch = ' ';
    let mut esc = false;
    for ch in code.chars() {
        if esc { esc = false; continue; }
        if ch == '\\' { esc = true; continue; }
        if !in_str && (ch == '"' || ch == '\'') { in_str = true; str_ch = ch; continue; }
        if in_str && ch == str_ch { in_str = false; continue; }
        if in_str { continue; }
        match ch {
            '{' => brace_opens += 1,
            '}' => brace_closes += 1,
            '(' => paren_opens += 1,
            ')' => paren_closes += 1,
            _ => {}
        }
    }
    (brace_opens, brace_closes, paren_opens, paren_closes)
}

fn strip_line_comment(s: &str) -> &str {
    let mut in_str = false;
    let mut str_ch = ' ';
    let mut esc = false;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        let ch = bytes[i] as char;
        if esc { esc = false; continue; }
        if ch == '\\' { esc = true; continue; }
        if !in_str && (ch == '"' || ch == '\'') { in_str = true; str_ch = ch; continue; }
        if in_str && ch == str_ch { in_str = false; continue; }
        if in_str { continue; }
        if ch == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            return &s[..i];
        }
    }
    s
}
