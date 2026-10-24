//! Assertions shared by the crate's unit tests.

use crate::diagnostic::{Diagnostic, Severity};

/// The codes of `diags`, in order.
pub(crate) fn codes(diags: &[Diagnostic]) -> Vec<&str> {
    diags.iter().map(|d| d.code.as_str()).collect()
}

/// The codes of the error-severity diagnostics in `diags`, in order.
pub(crate) fn error_codes(diags: &[Diagnostic]) -> Vec<&str> {
    diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code.as_str())
        .collect()
}

/// Assert some diagnostic in `diags` has `code`; on failure print them all.
#[track_caller]
pub(crate) fn assert_code(diags: &[Diagnostic], code: &str) {
    assert!(
        diags.iter().any(|d| d.code == code),
        "expected {code}, got: {diags:#?}"
    );
}

/// Assert no diagnostic in `diags` has `code`; on failure print them all.
#[track_caller]
pub(crate) fn assert_no_code(diags: &[Diagnostic], code: &str) {
    assert!(
        !diags.iter().any(|d| d.code == code),
        "unexpected {code}: {diags:#?}"
    );
}

/// Assert exactly `n` diagnostics in `diags` have `code`.
#[track_caller]
pub(crate) fn assert_code_count(diags: &[Diagnostic], code: &str, n: usize) {
    let got = diags.iter().filter(|d| d.code == code).count();
    assert_eq!(got, n, "expected {n} x {code}, got {got}: {diags:#?}");
}

/// Assert `diags` is empty.
#[track_caller]
pub(crate) fn assert_no_diags(diags: &[Diagnostic]) {
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:#?}");
}

/// Panic listing every failed row of a table test, or if no row ran.
#[track_caller]
pub(crate) fn report_failures(failures: &[String], rows: usize) {
    assert!(rows > 0, "no rows ran");
    assert!(
        failures.is_empty(),
        "{} of {rows} rows failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
