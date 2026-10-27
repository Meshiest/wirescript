//! Why a const expression could not be evaluated, and how that reads as a
//! user-facing diagnostic. All the message wording lives here.

use crate::diagnostic::SourceRange;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConstReason {
    /// Names a value that only exists at runtime.
    NotConstant(String),
    /// Calls a mod that is not declared `const`.
    NotAConstMod(String),
    /// Calls a `const mod` inside an enclosing expression with no compile-time
    /// form (usually an argument to a non-const call), so evaluation never
    /// reached it. The callee is fine; the enclosing expression is the cause.
    NestedConstModCall(String),
    /// Calls a `const mod` the calling file neither declares nor imports. The
    /// call itself is the error (`WS002`/`WS036` at the call), so this is
    /// not reported again.
    UnnamedConstMod(String),
    /// A syntactic form const evaluation does not handle. The string is a noun
    /// phrase (`"a spread"`), since the message wraps it in "... cannot be
    /// evaluated at compile time"; use `UnsupportedMessage` for a sentence.
    Unsupported(&'static str),
    /// An unsupported form whose reason needs a full sentence (why const and
    /// runtime would disagree, or what spelling works). The string is the
    /// whole message after "not a compile-time constant: ".
    UnsupportedMessage(&'static str),
    /// A method on a constant array or map that is not a supported mutation.
    /// The string is the whole message, naming the method the user wrote.
    UnsupportedMethod(String),
    /// A tuple destructure (`const (a, b) = ...`) whose width differs from
    /// the value's. Mirrors the WS010 `bind_let` reports for the types.
    TupleArityMismatch { expected: usize, got: usize },
    /// The certified evaluator declined to compute this (overflow, a
    /// non-ASCII string, an uncertified gate and operand combination). The
    /// value is computable in principle; the compiler will not guess it.
    Refused(String),
    /// A constant array index outside `0..len`. An out-of-range runtime read
    /// keeps the gate's previous value; a constant has none to fall back on.
    ArrayIndexOutOfRange { index: i64, len: usize },
    /// A constant map read (`m[k]`) whose key has no entry, refused for the
    /// same reason as [`ConstReason::ArrayIndexOutOfRange`].
    MapKeyNotFound,
    /// `rec.field` on a constant record that has no such field (the record
    /// itself is constant).
    RecordFieldNotFound(String),
    /// Call-chain depth or step count exceeded.
    BudgetExceeded,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConstError {
    pub reason: ConstReason,
    pub range: SourceRange,
}

impl ConstError {
    pub(crate) fn new(reason: ConstReason, range: &SourceRange) -> Self {
        Self {
            reason,
            range: range.clone(),
        }
    }

    /// A [`ConstReason::Unsupported`] error; `what` is a noun phrase.
    pub(crate) fn unsupported(what: &'static str, range: &SourceRange) -> Self {
        Self::new(ConstReason::Unsupported(what), range)
    }

    /// A [`ConstReason::Refused`] error.
    pub(crate) fn refused(why: impl Into<String>, range: &SourceRange) -> Self {
        Self::new(ConstReason::Refused(why.into()), range)
    }

    pub(crate) fn code(&self) -> &'static str {
        match self.reason {
            ConstReason::Refused(_) => "WS047",
            ConstReason::BudgetExceeded => "WS048",
            _ => "WS046",
        }
    }

    /// Whether another error already reports why this expression has no
    /// value (see [`ConstReason::UnnamedConstMod`]).
    pub(crate) fn reported_elsewhere(&self) -> bool {
        matches!(self.reason, ConstReason::UnnamedConstMod(_))
    }

    pub(crate) fn message(&self) -> String {
        match &self.reason {
            ConstReason::NotConstant(name) => {
                format!("not a compile-time constant: '{name}' is a runtime value")
            }
            ConstReason::NotAConstMod(name) => {
                format!("not a compile-time constant: '{name}' is not declared `const mod`")
            }
            ConstReason::NestedConstModCall(name) => format!(
                "not a compile-time constant: '{name}' is a `const mod`, but it is called \
                 inside a surrounding expression that has no compile-time form (usually \
                 an argument to a call whose own callee is not a `const mod`), so \
                 evaluation never reaches it"
            ),
            ConstReason::UnnamedConstMod(name) => format!(
                "not a compile-time constant: '{name}' is a `const mod` this file does not declare or import"
            ),
            ConstReason::Unsupported(what) => {
                format!("not a compile-time constant: {what} cannot be evaluated at compile time")
            }
            ConstReason::UnsupportedMessage(why) => format!("not a compile-time constant: {why}"),
            ConstReason::TupleArityMismatch { expected, got } => format!(
                "not a compile-time constant: this tuple destructure binds {expected} \
                 position(s), but the value has {got}"
            ),
            ConstReason::UnsupportedMethod(why) => format!("not a compile-time constant: {why}"),
            ConstReason::Refused(why) => {
                format!("the compiler will not compute this value at compile time: {why}")
            }
            ConstReason::ArrayIndexOutOfRange { index, len } => format!(
                "index {index} is out of range for a {len}-element compile-time constant \
                 array, and there is no previous value to fall back on at compile time"
            ),
            ConstReason::MapKeyNotFound => {
                "this compile-time constant map has no entry for that key, and there is no \
                 previous value to fall back on at compile time"
                    .to_string()
            }
            ConstReason::RecordFieldNotFound(field) => {
                format!("this compile-time constant record has no field named '{field}'")
            }
            ConstReason::BudgetExceeded => {
                "const evaluation gave up: the call chain is too deep or too large".into()
            }
        }
    }
}
