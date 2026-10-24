//! What each character of a line is part of: code, a string, a comment, or a
//! nested-prefab block. Block comments and prefab blocks can span lines, so
//! their state carries from one line into the next.

/// What a character of a line is part of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Code,
    /// A string literal: its quotes, its text and any `${...}` inside it.
    Str,
    /// A `//` comment, to the end of the line.
    LineComment,
    /// A `/* */` comment (they nest).
    BlockComment,
    /// A nested-prefab block, `` $```…``` ``, fences included.
    Prefab,
}

/// What is still open at the end of a line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Carry {
    #[default]
    Code,
    /// Inside this many nested block comments.
    Block(u32),
    Prefab,
}

/// The length of the `${ ... }` interpolation starting at `i`, both delimiters
/// included, or 0 when none starts there. Braces and string literals nested
/// inside it are part of it: `"${if a then "x}" else "y"}"` is one string. An
/// unterminated interpolation runs to the end of the line.
pub(super) fn interp_len(chars: &[char], i: usize) -> usize {
    if chars.get(i) != Some(&'$') || chars.get(i + 1) != Some(&'{') {
        return 0;
    }
    let mut depth = 1;
    let mut quote: Option<char> = None;
    let mut esc = false;
    for (j, &c) in chars.iter().enumerate().skip(i + 2) {
        if esc {
            esc = false;
            continue;
        }
        if c == '\\' {
            esc = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return j - i + 1;
                }
            }
            _ => {}
        }
    }
    chars.len() - i
}

/// Classify every character of `chars`, starting in `carry`; returns the
/// classes and what is still open at the end of the line. Strings end at the
/// line's end (they cannot span lines). A backslash outside a string still
/// escapes the next character, as in a string.
pub(super) fn classify(chars: &[char], carry: Carry) -> (Vec<Class>, Carry) {
    let n = chars.len();
    let mut class = vec![Class::Code; n];
    let mut state = carry;
    let mut quote: Option<char> = None;
    let mut esc = false;
    let at = |i: usize, s: &str| {
        s.chars()
            .enumerate()
            .all(|(k, c)| chars.get(i + k) == Some(&c))
    };
    let mut i = 0;
    while i < n {
        match state {
            Carry::Block(depth) => {
                class[i] = Class::BlockComment;
                if at(i, "/*") {
                    class[i + 1] = Class::BlockComment;
                    state = Carry::Block(depth + 1);
                    i += 2;
                } else if at(i, "*/") {
                    class[i + 1] = Class::BlockComment;
                    state = if depth == 1 {
                        Carry::Code
                    } else {
                        Carry::Block(depth - 1)
                    };
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            Carry::Prefab => {
                class[i] = Class::Prefab;
                if at(i, "```") {
                    class[i + 1] = Class::Prefab;
                    class[i + 2] = Class::Prefab;
                    state = Carry::Code;
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            Carry::Code => {}
        }
        let c = chars[i];
        if esc || c == '\\' {
            esc = !esc;
            class[i] = if quote.is_some() {
                Class::Str
            } else {
                Class::Code
            };
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            let len = interp_len(chars, i);
            if len > 0 {
                class[i..i + len].fill(Class::Str);
                i += len;
                continue;
            }
            class[i] = Class::Str;
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            class[i] = Class::Str;
        } else if at(i, "//") {
            class[i..].fill(Class::LineComment);
            break;
        } else if at(i, "/*") {
            class[i..i + 2].fill(Class::BlockComment);
            state = Carry::Block(1);
            i += 2;
            continue;
        } else if at(i, "$```") {
            class[i..i + 4].fill(Class::Prefab);
            state = Carry::Prefab;
            i += 4;
            continue;
        }
        i += 1;
    }
    (class, state)
}

/// Where the line's `//` comment starts, or its length when it has none.
pub(super) fn code_end(class: &[Class]) -> usize {
    class
        .iter()
        .position(|c| *c == Class::LineComment)
        .unwrap_or(class.len())
}
