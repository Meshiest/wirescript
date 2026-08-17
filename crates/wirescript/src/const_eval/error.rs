//! Why a const expression could not be evaluated, and how that reads as a
//! diagnostic. Kept separate from the evaluator so the message wording is in
//! one place — a const failure is usually a user-facing teaching moment, not
//! an internal error.

use crate::diagnostic::SourceRange;

#[derive(Clone, Debug, PartialEq)]
pub enum ConstReason {
    /// Names a value that only exists at runtime.
    NotConstant(String),
    /// Calls a mod that is not declared `const`.
    NotAConstMod(String),
    /// Calls a mod that IS declared `const mod`, from a position const
    /// evaluation never descended to. `eval_expr` recurses through every
    /// compound form it can evaluate — operators, constructor arguments,
    /// array/map/record literals, `if` expressions, indexing, field access —
    /// so a const-mod call inside any of THOSE is reached and run. What
    /// remains is a call sitting inside a SURROUNDING expression that has no
    /// compile-time form of its own: most often an argument to a call whose
    /// own callee is not a `const mod` (an ordinary `mod`, or a builtin with
    /// no certified constant form), where the enclosing arm declines before
    /// ever descending into the arguments.
    ///
    /// Distinct from [`ConstReason::NotAConstMod`] because saying "not
    /// declared `const mod`" about a mod that plainly IS one would be a false
    /// statement about the user's code — this callee is fine; what encloses
    /// it is what cannot be evaluated.
    NestedConstModCall(String),
    /// A syntactic form const evaluation does not handle. The string is a NOUN
    /// PHRASE naming that form (`"a spread"`, `"string interpolation"`) — it is
    /// wrapped in "not a compile-time constant: … cannot be evaluated at
    /// compile time", so anything longer than a noun phrase reads as a garbled
    /// run-on. Use [`ConstReason::UnsupportedMessage`] for a reason that needs
    /// to explain itself.
    Unsupported(&'static str),
    /// [`ConstReason::Unsupported`] for a refusal whose explanation does not
    /// fit a noun phrase: a rule that has to say WHY const and runtime would
    /// disagree, or point at the spelling that does work. The string is the
    /// COMPLETE message after "not a compile-time constant: " — the same
    /// contract [`ConstReason::UnsupportedMethod`] has, and the reason this is
    /// a separate variant rather than a longer `Unsupported` string: wrapping
    /// a sentence in the noun-phrase template produced text like "… so the two
    /// would disagree cannot be evaluated at compile time".
    UnsupportedMessage(&'static str),
    /// A method call on a compile-time constant array/map naming an
    /// operation this evaluator does not implement as a mutation (anything
    /// other than `push`/`set`/`clear`/`append` for an array, or
    /// `set`/`remove`/`clear` for a map). Distinct from
    /// [`ConstReason::Unsupported`] — whose reason is always a fixed
    /// `&'static str` — because this one must NAME the actual method the
    /// user wrote, so the diagnostic points at the offending call instead of
    /// a generic "this statement".
    UnsupportedMethod(String),
    /// A tuple destructure (`const (a, b) = …`) whose pattern binds a
    /// different number of positions than the value has. Distinct from
    /// [`ConstReason::Unsupported`]: the value IS a compile-time constant and
    /// IS positional — the pattern just has the wrong width, so the message
    /// should say both counts. Mirrors the WS010 `bind_let` raises for the
    /// same mismatch on the TYPES.
    TupleArityMismatch { expected: usize, got: usize },
    /// The certified evaluator declined to compute this — overflow, a
    /// non-ASCII string, an uncertified gate/operand combination. The value is
    /// computable in principle; the compiler will not guess it.
    Refused(String),
    /// A compile-time array index outside `0..len`. Distinct from
    /// [`ConstReason::NotConstant`]/[`ConstReason::Unsupported`]: the array
    /// AND the index are both perfectly constant — the position simply does
    /// not exist. At runtime an out-of-range array read keeps the gate's
    /// stale PREVIOUS value; at compile time there is no previous value to
    /// fall back on, so this is refused outright instead of guessed.
    ArrayIndexOutOfRange { index: i64, len: usize },
    /// A compile-time map index (`m[k]`) whose key has no matching entry.
    /// Same rationale as [`ConstReason::ArrayIndexOutOfRange`]: nothing to
    /// fall back to at compile time.
    MapKeyNotFound,
    /// Field access (`rec.field`) on a compile-time constant record whose
    /// fields do NOT include `field`. Distinct from
    /// [`ConstReason::NotConstant`]: the record itself evaluated fine — the
    /// name just isn't one of its fields — so the diagnostic should say that,
    /// not claim the whole expression is a runtime value.
    RecordFieldNotFound(String),
    /// Call-chain depth or step count exceeded.
    BudgetExceeded,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstError {
    pub reason: ConstReason,
    pub range: SourceRange,
}

impl ConstError {
    pub fn code(&self) -> &'static str {
        match self.reason {
            ConstReason::Refused(_) => "WS047",
            ConstReason::BudgetExceeded => "WS048",
            _ => "WS046",
        }
    }

    pub fn message(&self) -> String {
        match &self.reason {
            ConstReason::NotConstant(name) => format!(
                "not a compile-time constant: '{name}' is a runtime value"
            ),
            ConstReason::NotAConstMod(name) => format!(
                "not a compile-time constant: '{name}' is not declared `const mod`"
            ),
            ConstReason::NestedConstModCall(name) => format!(
                "not a compile-time constant: '{name}' is a `const mod`, but it is called \
                 inside a surrounding expression that has no compile-time form — usually \
                 an argument to a call whose own callee is not a `const mod` — so \
                 evaluation never reaches it"
            ),
            ConstReason::Unsupported(what) => format!(
                "not a compile-time constant: {what} cannot be evaluated at compile time"
            ),
            ConstReason::UnsupportedMessage(why) => format!("not a compile-time constant: {why}"),
            ConstReason::TupleArityMismatch { expected, got } => format!(
                "not a compile-time constant: this tuple destructure binds {expected} \
                 position(s), but the value has {got}"
            ),
            ConstReason::UnsupportedMethod(why) => format!("not a compile-time constant: {why}"),
            ConstReason::Refused(why) => format!(
                "the compiler will not compute this value at compile time: {why}"
            ),
            ConstReason::ArrayIndexOutOfRange { index, len } => format!(
                "index {index} is out of range for a {len}-element compile-time constant \
                 array — there is no previous value to fall back on at compile time"
            ),
            ConstReason::MapKeyNotFound => {
                "this compile-time constant map has no entry for that key — there is no \
                 previous value to fall back on at compile time"
                    .to_string()
            }
            ConstReason::RecordFieldNotFound(field) => format!(
                "this compile-time constant record has no field named '{field}'"
            ),
            ConstReason::BudgetExceeded => {
                "const evaluation gave up — the call chain is too deep or too large".into()
            }
        }
    }
}
