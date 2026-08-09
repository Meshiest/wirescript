pub fn format_wirescript(source: &str, tab: &str) -> String {
    let lines = source.split('\n');
    let mut result = Vec::new();
    // Stack of open delimiters; the bool records whether that open added an
    // indent level. A line adds AT MOST one level no matter how many groups
    // it opens (`addRole(next, {` opens `(` and `{` but indents once).
    let mut stack: Vec<bool> = Vec::new();
    let mut prev_blank = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() { result.push(String::new()); prev_blank = true; }
            continue;
        }
        prev_blank = false;

        // A leading run of closers de-indents before printing so the closing
        // line sits at its opener's level (`}`, `)`, `]`, `})`, ...).
        let code = strip_line_comment(trimmed);
        let leading_closers = code
            .chars()
            .take_while(|c| matches!(c, '}' | ')' | ']'))
            .count();
        for _ in 0..leading_closers {
            stack.pop();
        }
        let starts_close = leading_closers > 0;

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
        let line_indent = stack.iter().filter(|adds| **adds).count() as i32 + extra;

        result.push(format!("{}{}", tab.repeat(line_indent.max(0) as usize), trimmed));

        // Scan the rest of the line: opens push (adding an indent level only
        // while this line has no net open level yet), closes pop.
        scan_delimiters(code, leading_closers, &mut stack);
    }
    while result.last().map(|l| l.is_empty()).unwrap_or(false) { result.pop(); }
    let mut out = result.join("\n");
    if !out.ends_with('\n') { out.push('\n'); }
    out
}

/// Push/pop `{}`/`()`/`[]` found outside string literals onto the depth
/// stack, skipping the first `leading` delimiter characters (already popped
/// by the caller). An open pushes an indent contribution only while the
/// line's net indenting opens are zero — at most one level per line.
fn scan_delimiters(code: &str, leading: usize, stack: &mut Vec<bool>) {
    let mut in_str = false;
    let mut str_ch = ' ';
    let mut esc = false;
    let mut net_true: i32 = 0;
    for (i, ch) in code.chars().enumerate() {
        if i < leading {
            continue;
        }
        if esc { esc = false; continue; }
        if ch == '\\' { esc = true; continue; }
        if !in_str && (ch == '"' || ch == '\'') { in_str = true; str_ch = ch; continue; }
        if in_str && ch == str_ch { in_str = false; continue; }
        if in_str { continue; }
        match ch {
            '{' | '(' | '[' => {
                let adds = net_true <= 0;
                if adds { net_true += 1; }
                stack.push(adds);
            }
            '}' | ')' | ']' => {
                if let Some(adds) = stack.pop()
                    && adds
                {
                    net_true -= 1;
                }
            }
            _ => {}
        }
    }
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

#[cfg(test)]
mod tests;
