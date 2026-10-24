//! Formatting every `.ws` file in the repository must be stable (a second run
//! changes nothing) and must keep every token: the formatter moves whitespace
//! and line breaks, and adds the optional trailing comma when it splits an
//! import list, and nothing else. A file the formatter got wrong would still
//! compile, often to the same program, so this checks the text directly.

use std::path::{Path, PathBuf};
use wirescript::TokenKind;
use wirescript::analysis::format_wirescript;
use wirescript::lexer::TokenValue;

fn ws_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                ws_files(&path, out);
            }
        } else if name.ends_with(".ws") {
            out.push(path);
        }
    }
}

/// The token stream without newlines or trailing commas, nested-prefab
/// blocks compared by their own tokens, then the `//` comment texts in order.
fn tokens(src: &str) -> Vec<String> {
    let lexed = wirescript::lex(src, "x.ws");
    let mut out = Vec::new();
    for t in &lexed.tokens {
        match t.kind {
            TokenKind::Newline => {}
            TokenKind::NestedPrefab => {
                out.push("<prefab>".to_string());
                if let Some(TokenValue::Str(inner)) = &t.value {
                    out.extend(tokens(inner));
                }
                out.push("</prefab>".to_string());
            }
            _ => out.push(format!("{:?} {}", t.kind, t.text)),
        }
    }
    let mut i = 0;
    while i + 1 < out.len() {
        let closes = ["RBrace", "RParen", "RBracket"]
            .iter()
            .any(|k| out[i + 1].starts_with(k));
        if out[i].starts_with("Comma") && closes {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    out.extend(
        lexed
            .source_map
            .comments
            .iter()
            .map(|c| format!("// {}", c.text)),
    );
    out
}

#[test]
fn formatting_the_repository_is_stable_and_keeps_every_token() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let mut files = Vec::new();
    ws_files(&root, &mut files);
    // A wrong root finds nothing and would pass with nothing checked. A clean
    // checkout has around 60; a working copy usually has more.
    assert!(
        files.len() >= 20,
        "found only {} .ws files under {}",
        files.len(),
        root.display()
    );

    let mut failures = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        let once = format_wirescript(&src, "  ");
        if format_wirescript(&once, "  ") != once {
            failures.push(format!("{}: a second run changes it", file.display()));
        }
        let (before, after) = wirescript::on_big_stack(|| (tokens(&src), tokens(&once)));
        if before != after {
            let at = before
                .iter()
                .zip(&after)
                .position(|(a, b)| a != b)
                .unwrap_or(before.len().min(after.len()));
            failures.push(format!(
                "{}: tokens changed at {at}: {:?} became {:?}",
                file.display(),
                before.get(at),
                after.get(at)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
