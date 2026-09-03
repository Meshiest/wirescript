//! Guards that every `WSxxx` diagnostic code emitted from
//! `crates/wirescript/src/` is documented in `docs/src/diagnostics.md`,
//! and that nothing documented there has lost its emit site.
//!
//! `ws_reachability.rs` proves a code CAN fire end-to-end, but nothing proved
//! a code is written DOWN anywhere a user would find it — which is exactly
//! how WS044-WS048 drifted out of the docs while WS009/WS018 stayed in as
//! stale entries. This test scans both sources of truth as plain text and
//! diffs the two `WSddd` code sets, modulo an explicit allowlist of codes
//! that are reserved/retired and therefore have nothing to document.

use std::fs;
use std::path::{Path, PathBuf};
use std::collections::BTreeSet;
use wirescript::collections::HashSet;

/// Codes with no emit site, so there is nothing to document.
/// Keep in sync with the header of `ws_reachability.rs`.
const RETIRED_OR_RESERVED: &[&str] = &["WS009", "WS015", "WS018", "WS029", "WS034"];

/// Scan `text` for `WSddd` literals — `WS` followed by exactly 3 ASCII
/// digits, not itself surrounded by more alphanumerics/underscore — and
/// insert each match into `out`. A manual scan rather than a `regex`
/// dependency, since this is the only place in the crate that would need one.
///
/// This deliberately does NOT match `WSP001` (the parse-level code): the
/// character right after `WS` there is `P`, not a digit, so the digit check
/// fails before the boundary check is even reached. It also skips generic
/// placeholders like `WSxxx`/`WS0xx` in prose, for the same reason.
fn scan_codes(text: &str, out: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 5 <= len {
        let is_code = bytes[i] == b'W'
            && bytes[i + 1] == b'S'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4].is_ascii_digit();
        if is_code {
            let before_ok = i == 0 || {
                let c = bytes[i - 1];
                !(c.is_ascii_alphanumeric() || c == b'_')
            };
            let after_ok = i + 5 == len || {
                let c = bytes[i + 5];
                !(c.is_ascii_alphanumeric() || c == b'_')
            };
            if before_ok && after_ok {
                out.insert(text[i..i + 5].to_string());
                i += 5;
                continue;
            }
        }
        i += 1;
    }
}

/// Recursively collect every `.rs` file under `dir`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("failed to read a dir entry: {e}"));
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every `WSddd` code appearing anywhere under `crates/wirescript/src/`
/// (comments, emit-site string literals, doc comments — the scan is purely
/// textual). Deliberately scoped to `src/` only: this crate's own
/// `tests/` directory (including this file's `RETIRED_OR_RESERVED` list and
/// `ws_reachability.rs`'s header) is NOT scanned, so neither pollutes the
/// "defined in src/" set.
fn src_codes() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&root, &mut files);
    assert!(!files.is_empty(), "no .rs files found under {}", root.display());
    let mut codes = BTreeSet::new();
    for file in &files {
        let text = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
        scan_codes(&text, &mut codes);
    }
    codes
}

/// Every `WSddd` code appearing in `docs/src/diagnostics.md`.
fn doc_codes() -> (BTreeSet<String>, PathBuf) {
    // CARGO_MANIFEST_DIR is crates/wirescript; the doc lives at
    // <repo root>/docs/src/diagnostics.md, i.e. two levels up.
    let doc_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("src")
        .join("diagnostics.md");
    let text = fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", doc_path.display()));
    let mut codes = BTreeSet::new();
    scan_codes(&text, &mut codes);
    (codes, doc_path)
}

#[test]
fn every_emitted_code_is_documented() {
    let src = src_codes();
    let (doc, doc_path) = doc_codes();
    let allowlist: HashSet<&str> = RETIRED_OR_RESERVED.iter().copied().collect();

    let undocumented: Vec<&String> = src
        .iter()
        .filter(|c| !doc.contains(c.as_str()) && !allowlist.contains(c.as_str()))
        .collect();
    assert!(
        undocumented.is_empty(),
        "{} code(s) emitted from src/ but not documented in {}: {}\n\
         -> add an entry for each (following the surrounding format), or if the \
         code has genuinely no emit site, add it to RETIRED_OR_RESERVED instead.\n{}",
        undocumented.len(),
        doc_path.display(),
        undocumented.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        undocumented
            .iter()
            .map(|c| format!("  - {c} is emitted but not documented"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let stale: Vec<&String> = doc
        .iter()
        .filter(|c| !src.contains(c.as_str()) && !allowlist.contains(c.as_str()))
        .collect();
    assert!(
        stale.is_empty(),
        "{} code(s) documented in {} but no longer emitted from src/: {}\n\
         -> remove the entry, or if it's intentionally retired/reserved, add it \
         to RETIRED_OR_RESERVED instead.\n{}",
        stale.len(),
        doc_path.display(),
        stale.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        stale
            .iter()
            .map(|c| format!("  - {c} is documented but no longer emitted"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
