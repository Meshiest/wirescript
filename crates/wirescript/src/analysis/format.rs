pub fn format_wirescript(source: &str, tab: &str) -> String {
    let lines = source.split('\n');
    let mut result = Vec::new();
    let mut indent: i32 = 0;
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
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

        let starts_close_bracket = trimmed.starts_with(']');
        if starts_close_bracket { bracket_depth = (bracket_depth - 1).max(0); }

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
        let line_indent = indent + paren_depth + bracket_depth + extra;

        result.push(format!("{}{}", tab.repeat(line_indent as usize), trimmed));

        // Note `string[] = [` nets +1 bracket: the `[]` type suffix
        // self-cancels, the multi-line literal opener carries over.
        let d = count_delimiters(trimmed);
        if starts_close { indent += d.brace_opens; } else { indent = (indent + d.brace_opens - d.brace_closes).max(0); }
        if starts_close_paren { paren_depth += d.paren_opens; } else { paren_depth = (paren_depth + d.paren_opens - d.paren_closes).max(0); }
        if starts_close_bracket { bracket_depth += d.bracket_opens; } else { bracket_depth = (bracket_depth + d.bracket_opens - d.bracket_closes).max(0); }
    }
    while result.last().map(|l| l.is_empty()).unwrap_or(false) { result.pop(); }
    let mut out = result.join("\n");
    if !out.ends_with('\n') { out.push('\n'); }
    out
}

#[derive(Default)]
struct DelimCounts {
    brace_opens: i32,
    brace_closes: i32,
    paren_opens: i32,
    paren_closes: i32,
    bracket_opens: i32,
    bracket_closes: i32,
}

fn count_delimiters(s: &str) -> DelimCounts {
    // Strip line comments before counting — delimiters inside // are not real.
    let code = strip_line_comment(s);
    let mut d = DelimCounts::default();
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
            '{' => d.brace_opens += 1,
            '}' => d.brace_closes += 1,
            '(' => d.paren_opens += 1,
            ')' => d.paren_closes += 1,
            '[' => d.bracket_opens += 1,
            ']' => d.bracket_closes += 1,
            _ => {}
        }
    }
    d
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
mod tests {
    use super::format_wirescript;

    fn fmt(src: &str) -> String {
        format_wirescript(src, "  ")
    }

    #[test]
    fn multi_line_array_literal_indents_elements() {
        let src = "array names: string[] = [\n\"A\",\n\"B\",\n]\nlet x = 1\n";
        let want = "array names: string[] = [\n  \"A\",\n  \"B\",\n]\nlet x = 1\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn array_literal_inside_handler_stacks_with_block_indent() {
        let src = "on t {\nfoo = [\n1,\n...base,\n2\n]\ndone = true\n}\n";
        let want = "on t {\n  foo = [\n    1,\n    ...base,\n    2\n  ]\n  done = true\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn single_line_arrays_and_index_reads_unaffected() {
        let src = "array base: int[] = [1, 2, 3]\nlet v = arr[i]\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn brackets_in_strings_and_comments_ignored() {
        let src =
            "array a: string[] = [\n\"[not a bracket]\",\n// comment with ] and [\n\"end\",\n]\n";
        let want =
            "array a: string[] = [\n  \"[not a bracket]\",\n  // comment with ] and [\n  \"end\",\n]\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn paren_continuation_still_indents() {
        let src = "on t {\nctrl.DisplayText(\"hi\",\npositionX = 0.0,\n)\n}\n";
        let want = "on t {\n  ctrl.DisplayText(\"hi\",\n    positionX = 0.0,\n  )\n}\n";
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn formatting_is_idempotent() {
        let src = "on t {\nfoo = [\n1,\n2\n]\n}\narray n: string[] = [\n\"x\",\n]\n";
        let once = fmt(src);
        assert_eq!(fmt(&once), once);
    }
}
