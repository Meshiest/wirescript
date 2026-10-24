//! The Wirescript formatter.
//!
//! Line-based: every line is re-indented by the brackets still open above it,
//! with a few opinions on spacing, `else` placement, blank lines and long
//! lines. It only moves whitespace and line breaks, never tokens, so it cannot
//! change what a program means.
//!
//! The rules, in the order a line meets them:
//! - A line inside a multi-line block comment or nested-prefab block is kept
//!   as written. `// fmt-ignore` on its own line keeps the next line as
//!   written; at the end of a line, it keeps that line.
//! - Blank-line runs collapse to one; leading and trailing blank lines go.
//! - Outside strings and comments: no space before a comma, one after it
//!   (unless a closer follows), and runs of spaces collapse to one.
//! - `}` then `else {` on the next line join into `} else {`, unless the `}`
//!   ends an `else if` rung or the `else` is an `else if`, so a ladder the
//!   author wrote over several lines stays that way.
//! - Consecutive top-level `in`, `out` or `var` lines lose the blank line
//!   between them.
//! - A line opens at most one indentation level however many brackets it
//!   opens; a line starting with closers sits at its opener's level. A line
//!   continuing an expression (starting with `then`, `else`, a binary operator
//!   or `.`) goes one level deeper.
//! - A line wider than the print width is split: an `import { ... }` list
//!   fills its names across lines, a binary expression breaks before its
//!   loosest operators.

mod options;
mod scan;
mod split;

pub use options::{DEFAULT_PRINT_WIDTH, FormatOptions, LineEnding};

use scan::{Carry, Class, classify, code_end, interp_len};

/// What the character at char column `col` of `line` is part of, for editor
/// features that act on code only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextKind {
    /// Code, including a `${...}` inside a string and a nested-prefab block.
    Code,
    /// A string literal's own text or quotes.
    Str,
    Comment,
}

/// The [`TextKind`] at char column `col` of `line` in `source`.
pub(crate) fn text_kind_at(source: &str, line: usize, col: usize) -> TextKind {
    let mut carry = Carry::Code;
    for (i, l) in source.lines().enumerate() {
        let chars: Vec<char> = l.chars().collect();
        let (class, next) = classify(&chars, carry);
        if i < line {
            carry = next;
            continue;
        }
        return match class.get(col) {
            Some(Class::LineComment | Class::BlockComment) => TextKind::Comment,
            Some(Class::Str) => {
                // The string's run starts at its opening quote; walk it to see
                // whether `col` falls in one of its `${...}`.
                let mut j = (0..col)
                    .rev()
                    .find(|&k| class[k] != Class::Str)
                    .map_or(0, |k| k + 1)
                    + 1;
                while j < col {
                    let len = interp_len(&chars, j);
                    if len > 0 {
                        if col < j + len {
                            return TextKind::Code;
                        }
                        j += len;
                    } else {
                        j += if chars[j] == '\\' { 2 } else { 1 };
                    }
                }
                TextKind::Str
            }
            _ => TextKind::Code,
        };
    }
    TextKind::Code
}

/// Format `source` with `tab` as one level of indentation and every other
/// option at its default.
pub fn format_wirescript(source: &str, tab: &str) -> String {
    format_with(
        source,
        &FormatOptions {
            indent: tab.to_string(),
            ..FormatOptions::default()
        },
    )
}

pub fn format_with(source: &str, opts: &FormatOptions) -> String {
    let eol = opts
        .end_of_line
        .unwrap_or_else(|| LineEnding::dominant_in(source));
    let mut f = Formatter {
        opts,
        lines: Vec::new(),
        stack: Vec::new(),
        prev_blank: false,
        prev: String::new(),
        ignore_next: false,
        carry: Carry::Code,
    };
    for raw in source.split('\n') {
        f.line(raw.strip_suffix('\r').unwrap_or(raw));
    }
    let mut lines = f.lines;
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut out = lines.join(eol.as_str());
    if opts.insert_final_newline || source.ends_with('\n') {
        out.push_str(eol.as_str());
    }
    out
}

struct Formatter<'a> {
    opts: &'a FormatOptions,
    lines: Vec<String>,
    /// One entry per open bracket: whether opening it added a level.
    stack: Vec<bool>,
    prev_blank: bool,
    /// The previous non-blank line, trimmed.
    prev: String,
    /// A standalone `// fmt-ignore` was the previous line.
    ignore_next: bool,
    /// A block comment or nested-prefab block still open from an earlier line.
    carry: Carry,
}

impl Formatter<'_> {
    fn indent(&self, level: usize) -> String {
        self.opts.indent.repeat(level)
    }

    fn line(&mut self, raw: &str) {
        // Inside a multi-line block comment or prefab block: as written (a
        // prefab's text is its own program, so even its trailing whitespace
        // stays). Brackets after the block closes still count.
        if self.carry != Carry::Code {
            let chars: Vec<char> = raw.chars().collect();
            let in_prefab = self.carry == Carry::Prefab;
            let (class, carry) = classify(&chars, self.carry);
            self.carry = carry;
            scan_brackets(&chars, &class, 0, &mut self.stack);
            self.lines.push(if in_prefab {
                raw.to_string()
            } else {
                raw.trim_end().to_string()
            });
            self.prev_blank = false;
            self.prev = raw.trim().to_string();
            return;
        }

        let raw_trimmed = raw.trim();
        if raw_trimmed.is_empty() {
            if !self.prev_blank && !self.lines.is_empty() {
                self.lines.push(String::new());
                self.prev_blank = true;
            }
            return;
        }
        self.prev_blank = false;

        let raw_chars: Vec<char> = raw_trimmed.chars().collect();
        let (raw_class, raw_carry) = classify(&raw_chars, Carry::Code);
        let raw_code_end = code_end(&raw_class);
        let comment: String = raw_chars[raw_code_end..].iter().collect();
        let has_code = raw_chars[..raw_code_end].iter().any(|c| !c.is_whitespace());
        let marker = comment.starts_with("//")
            && !comment.starts_with("///")
            && comment.trim_start_matches('/').trim() == "fmt-ignore";

        // `// fmt-ignore`: this line (trailing marker) or the one after a
        // standalone marker stays exactly as written.
        if std::mem::take(&mut self.ignore_next) || (marker && has_code) {
            let code = &raw_chars[..raw_code_end];
            let closers = leading_closers(code);
            self.stack
                .truncate(self.stack.len().saturating_sub(closers));
            scan_brackets(code, &raw_class[..raw_code_end], closers, &mut self.stack);
            self.lines.push(raw.to_string());
            self.prev = raw_trimmed.to_string();
            self.carry = raw_carry;
            return;
        }

        let trimmed = normalize_spacing(&raw_chars, &raw_class);
        let chars: Vec<char> = trimmed.chars().collect();
        let (class, carry) = classify(&chars, Carry::Code);
        let code_len = code_end(&class);
        let code = &chars[..code_len];

        if is_plain_else(&trimmed)
            && let Some(last) = self.lines.last_mut()
            && last.trim_end().ends_with('}')
            && !last.trim_start().starts_with("else")
        {
            *last = format!("{} {trimmed}", last.trim_end());
            scan_brackets(code, &class[..code_len], 0, &mut self.stack);
            self.prev = trimmed;
            self.carry = carry;
            return;
        }

        if self.stack.is_empty()
            && ["in ", "out ", "var "]
                .iter()
                .any(|kw| trimmed.starts_with(kw) && self.prev.starts_with(kw))
            && self.lines.last().is_some_and(|l| l.is_empty())
        {
            self.lines.pop();
        }

        let closers = leading_closers(code);
        self.stack
            .truncate(self.stack.len().saturating_sub(closers));
        // An `else` after a line ending in `}` is the statement form (`else if`
        // on its own line); only the expression form continues a line.
        let statement_else = self.prev.ends_with('}');
        let continues = closers == 0
            && (starts_with_word(&trimmed, "then")
                || (starts_with_word(&trimmed, "else") && !statement_else)
                || starts_with_binary_op(&trimmed)
                || (trimmed.starts_with('.') && !trimmed.starts_with("...")));
        let level = self.stack.iter().filter(|adds| **adds).count() + usize::from(continues);

        let mut out = vec![format!("{}{trimmed}", self.indent(level))];
        if marker {
            // A standalone marker is indented like any line and protects the
            // next one.
            self.ignore_next = true;
        } else if let Some(max) = self.opts.print_width
            && split::width(&out[0]) > max
            && !continues
            && closers == 0
            && class.iter().all(|k| matches!(k, Class::Code | Class::Str))
        {
            let indent = &self.opts.indent;
            if let Some(lines) = split::split_import(&chars, &class, level, indent, max)
                .or_else(|| split::split_operation(&chars, &class, level, indent, max))
            {
                out = lines;
            }
        }
        self.lines.extend(out);
        scan_brackets(code, &class[..code_len], closers, &mut self.stack);
        self.prev = trimmed;
        self.carry = carry;
    }
}

/// How many of the line's first characters are closing brackets.
fn leading_closers(code: &[char]) -> usize {
    code.iter()
        .take_while(|c| matches!(c, '}' | ')' | ']'))
        .count()
}

/// Push and pop the code's brackets on `stack`, skipping its first `skip`
/// characters (closers the caller already popped). An opener adds a level
/// only while the line has not yet added one, so a line indents what follows
/// by one level at most: `addRole(next, {` opens two brackets, one level.
fn scan_brackets(chars: &[char], class: &[Class], skip: usize, stack: &mut Vec<bool>) {
    let mut added = 0i32;
    let mut esc = false;
    for (i, &c) in chars.iter().enumerate().skip(skip) {
        if class[i] != Class::Code {
            continue;
        }
        if esc || c == '\\' {
            esc = !esc;
            continue;
        }
        match c {
            '{' | '(' | '[' => {
                let adds = added <= 0;
                if adds {
                    added += 1;
                }
                stack.push(adds);
            }
            '}' | ')' | ']' => {
                if let Some(adds) = stack.pop()
                    && adds
                {
                    added -= 1;
                }
            }
            _ => {}
        }
    }
}

/// Spacing outside strings, comments and prefab blocks: no space before a
/// comma and one after it (unless a closer or the line's end follows), and
/// runs of spaces collapsed to one.
fn normalize_spacing(chars: &[char], class: &[Class]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if class[i] != Class::Code {
            out.push(c);
            i += 1;
            continue;
        }
        if c == '\\' {
            out.extend(&chars[i..(i + 2).min(chars.len())]);
            i += 2;
            continue;
        }
        if c == ',' {
            out.truncate(out.trim_end_matches([' ', '\t']).len());
            out.push(',');
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j], ' ' | '\t') && class[j] == Class::Code {
                j += 1;
            }
            if chars
                .get(j)
                .is_some_and(|next| !matches!(next, ')' | ']' | '}'))
            {
                out.push(' ');
            }
            i = j;
            continue;
        }
        out.push(c);
        if c == ' ' {
            while chars.get(i + 1) == Some(&' ') {
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// `line` starts with `word` followed by whitespace or nothing.
fn starts_with_word(line: &str, word: &str) -> bool {
    line.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// A statement-form `else` that can join the `}` above it: `else`, `else {`,
/// but not `else if`.
fn is_plain_else(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("else") else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '{')) {
        return false;
    }
    let after = rest.trim_start();
    !(after
        .strip_prefix("if")
        .is_some_and(|r| !r.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
        && after.len() < rest.len())
}

/// The line starts with a binary operator followed by whitespace or `(`: it
/// continues the expression on the line above.
fn starts_with_binary_op(line: &str) -> bool {
    const OPS: [&str; 21] = [
        "&&", "||", "^^", "==", "!=", "<=", ">=", "<<", ">>", "**", "..", "&", "|", "^", "+", "-",
        "*", "/", "%", "<", ">",
    ];
    OPS.iter().any(|op| {
        line.strip_prefix(op)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '('))
    })
}

#[cfg(test)]
mod tests;
