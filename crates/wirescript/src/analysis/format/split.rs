//! Breaking an over-wide line: an import list across several lines, or a
//! binary expression before its loosest operators.

use super::scan::{classify, Carry, Class};

/// Columns a line takes, counted the way the editor's ruler does for the
/// common cases (UTF-16 code units).
pub(super) fn width(s: &str) -> usize {
    s.encode_utf16().count()
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Binding strength of a binary operator a line can break before; lower
/// binds looser, so the line breaks there first. `None` for anything else.
fn precedence(op: &str) -> Option<u8> {
    Some(match op {
        "||" | "^^" => 1,
        "&&" => 2,
        "|" => 3,
        "^" => 4,
        "&" => 5,
        "==" | "!=" => 6,
        "<" | ">" | "<=" | ">=" => 7,
        "<<" | ">>" => 8,
        ".." => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        "**" => 12,
        _ => return None,
    })
}

const TWO_CHAR_OPS: [&str; 11] = ["&&", "||", "^^", "==", "!=", "<=", ">=", "<<", ">>", "**", ".."];

/// Where the binary operators outside every bracket are, as (position,
/// precedence). A unary sign, a `...` spread, `->`/`=>`, a compound
/// assignment and the inside of an atom (`:my-state`) are not operators.
fn top_level_ops(chars: &[char], class: &[Class]) -> Vec<(usize, u8)> {
    let ends_operand = |c: char| is_ident(c) || matches!(c, ')' | ']' | '"' | '\'');
    let at = |i: usize, s: &str| s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c));
    let n = chars.len();
    let mut ops = Vec::new();
    let mut depth = 0i32;
    let mut prev = '\0';
    let mut esc = false;
    let mut i = 0;
    while i < n {
        if class[i] == Class::Str {
            // A whole string literal is one operand.
            while i < n && class[i] == Class::Str {
                i += 1;
            }
            prev = '"';
            continue;
        }
        let c = chars[i];
        if esc || c == '\\' {
            esc = !esc;
            i += 1;
            continue;
        }
        // An atom: `:` where a value starts, then the name, which may hold `-`.
        if c == ':' && !ends_operand(prev) {
            let mut j = i + 1;
            while j < n && (is_ident(chars[j]) || chars[j] == '-') {
                j += 1;
            }
            if j > i + 1 {
                prev = chars[j - 1];
                i = j;
                continue;
            }
        }
        if c == ' ' || c == '\t' {
            i += 1;
            continue;
        }
        if matches!(c, '{' | '(' | '[') {
            depth += 1;
        } else if matches!(c, '}' | ')' | ']') {
            depth -= 1;
        } else if at(i, "...") {
            prev = '.';
            i += 3;
            continue;
        } else if at(i, "->") || at(i, "=>") {
            prev = chars[i + 1];
            i += 2;
            continue;
        } else if let Some(op) = TWO_CHAR_OPS.iter().find(|op| at(i, op)) {
            if (*op == "<<" || *op == ">>") && chars.get(i + 2) == Some(&'=') {
                prev = '=';
                i += 3;
                continue;
            }
            if depth == 0 {
                ops.extend(precedence(op).map(|p| (i, p)));
            }
            prev = chars[i + 1];
            i += 2;
            continue;
        } else if "+-*/%&|^<>".contains(c) {
            if chars.get(i + 1) == Some(&'=') {
                prev = '=';
                i += 2;
                continue;
            }
            if (c == '+' || c == '-') && !ends_operand(prev) {
                prev = c;
                i += 1;
                continue;
            }
            if depth == 0 {
                ops.extend(precedence(&c.to_string()).map(|p| (i, p)));
            }
        }
        prev = c;
        i += 1;
    }
    ops
}

fn is_balanced(chars: &[char], class: &[Class]) -> bool {
    let mut depth = 0i32;
    for (c, k) in chars.iter().zip(class) {
        if *k != Class::Code {
            continue;
        }
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// Whether one of `words` appears as a whole word in the line's code.
fn has_word(chars: &[char], class: &[Class], words: &[&str]) -> bool {
    let mut word = String::new();
    for (c, k) in chars.iter().zip(class).chain(std::iter::once((&' ', &Class::Code))) {
        if *k == Class::Code && is_ident(*c) {
            word.push(*c);
        } else {
            if words.contains(&word.as_str()) {
                return true;
            }
            word.clear();
        }
    }
    false
}

fn collect(chars: &[char]) -> String {
    chars.iter().collect::<String>().trim().to_string()
}

/// `import { a, b, c } from "lib"` with its names spread over lines, as many
/// to a line as fit in `max`, each followed by a comma. `None` when the line
/// is not a braced import of at least two names.
pub(super) fn split_import(chars: &[char], class: &[Class], base: usize, indent: &str, max: usize) -> Option<Vec<String>> {
    let starts_import = chars.starts_with(&['i', 'm', 'p', 'o', 'r', 't']) && !chars.get(6).copied().is_some_and(is_ident);
    if !starts_import {
        return None;
    }
    let code_at = |c: char, from: usize| (from..chars.len()).find(|&i| chars[i] == c && class[i] == Class::Code);
    let open = code_at('{', 0)?;
    let close = code_at('}', open + 1)?;
    let names: Vec<String> = chars[open + 1..close]
        .iter()
        .collect::<String>()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if names.len() < 2 {
        return None;
    }
    let prefix = chars[..open].iter().collect::<String>().trim_end().to_string();
    let suffix: String = chars[close..].iter().collect();
    let cont = indent.repeat(base + 1);
    let mut out = vec![format!("{}{prefix} {{", indent.repeat(base))];
    let mut cur = cont.clone();
    for name in names {
        let piece = format!("{name},");
        if width(&cur) > width(&cont) {
            let candidate = format!("{cur} {piece}");
            if width(&candidate) > max {
                out.push(std::mem::replace(&mut cur, format!("{cont}{piece}")));
            } else {
                cur = candidate;
            }
        } else {
            cur = format!("{cont}{piece}");
        }
    }
    out.push(cur);
    out.push(format!("{}{suffix}", indent.repeat(base)));
    Some(out)
}

/// A long binary expression broken before each of its loosest top-level
/// operators, the operator leading each continuation line one level deeper.
/// `None` when there is nothing to break on, the brackets do not balance, or
/// the line is an `if`/`then`/`else`/`match`/`await` form.
fn split_once(chars: &[char], class: &[Class], base: usize, indent: &str) -> Option<Vec<String>> {
    if has_word(chars, class, &["if", "then", "else", "match", "await"]) || !is_balanced(chars, class) {
        return None;
    }
    let ops = top_level_ops(chars, class);
    let loosest = ops.iter().map(|(_, p)| *p).min()?;
    let mut segments = Vec::new();
    let mut start = 0;
    for (pos, _) in ops.iter().filter(|(_, p)| *p == loosest) {
        segments.push(collect(&chars[start..*pos]));
        start = *pos;
    }
    segments.push(collect(&chars[start..]));
    if segments.len() < 2 {
        return None;
    }
    let mut out = vec![format!("{}{}", indent.repeat(base), segments[0])];
    out.extend(segments[1..].iter().map(|s| format!("{}{s}", indent.repeat(base + 1))));
    Some(out)
}

/// [`split_once`], then the first line again while it is still too wide: the
/// continuation lines start with an operator, which this formatter does not
/// break further, so only the first line can still need it. Doing it here
/// rather than on the next run is what makes the result stable.
pub(super) fn split_operation(chars: &[char], class: &[Class], base: usize, indent: &str, max: usize) -> Option<Vec<String>> {
    let mut out = split_once(chars, class, base, indent)?;
    while width(&out[0]) > max {
        let first: Vec<char> = out[0].trim_start().chars().collect();
        let (first_class, _) = classify(&first, Carry::Code);
        let Some(lines) = split_once(&first, &first_class, base, indent) else { break };
        out.splice(0..1, lines);
    }
    Some(out)
}
