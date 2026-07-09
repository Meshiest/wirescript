use lasso::{Spur, ThreadedRodeo};
use std::sync::LazyLock;

static INTERN: LazyLock<ThreadedRodeo> = LazyLock::new(ThreadedRodeo::default);

/// Interned symbol handle. 4 bytes, Copy, cheap to hash and compare.
pub type Sym = Spur;

/// Intern a string, returning a handle. Thread-safe, idempotent.
pub fn intern(s: &str) -> Sym {
    INTERN.get_or_intern(s)
}

/// Intern a static string. Slightly faster path for compile-time-known strings.
pub fn intern_static(s: &'static str) -> Sym {
    INTERN.get_or_intern_static(s)
}

/// Resolve a handle back to its string.
pub fn resolve(sym: Sym) -> &'static str {
    INTERN.resolve(&sym)
}

/// Pre-interned constants for the most common port names and gate classes.
/// Each constant is lazily initialized on first access via `intern_static`.
pub mod sym {
    use super::*;

    macro_rules! def {
        ($name:ident, $s:expr) => {
            pub static $name: LazyLock<Sym> = LazyLock::new(|| intern_static($s));
        };
    }

    // Port names
    def!(EXEC, "Exec");
    def!(EXEC_OUT, "ExecOut");
    def!(EXEC_A, "ExecA");
    def!(EXEC_B, "ExecB");
    def!(VALUE, "Value");
    def!(VAR_REF, "VarRef");
    def!(INPUT, "Input");
    def!(OUTPUT, "Output");
    def!(INPUT_A, "InputA");
    def!(INPUT_B, "InputB");
    def!(INDEX, "Index");
    def!(ARRAY_VAR_REF, "ArrayVarRef");
    def!(B_INPUT, "bInput");
    def!(B_OUTPUT, "bOutput");
    def!(B_COND, "bCond");
    def!(RER_INPUT, "RER_Input");
    def!(RER_OUTPUT, "RER_Output");
    def!(TICKS_TO_WAIT, "TicksToWait");
    def!(SECONDS_TO_WAIT, "SecondsToWait");
    def!(ZERO_TICKS_TO_WAIT, "ZeroTicksToWait");
    def!(ZERO_SECONDS_TO_WAIT, "ZeroSecondsToWait");
    def!(INITIAL_VALUE, "InitialValue");
    def!(EXEC_OUT_A, "ExecOutA");
    def!(EXEC_OUT_B, "ExecOutB");
    def!(B_INPUT_A, "bInputA");
    def!(B_INPUT_B, "bInputB");
    def!(B_SELECT_B, "bSelectB");
    def!(B_OUT_OF_BOUNDS, "bOutOfBounds");
    def!(PORT_LABEL, "PortLabel");
    // Pseudo-property (not a game field): the declaration's source name,
    // carried on Var/ArrayVar nodes so emit can attach a text label.
    def!(NAME_LABEL, "_label");

    // Gate classes
    def!(LITERAL, "_Literal");
    def!(UNSUPPORTED, "_Unsupported");
    def!(LAYOUT, "_layout");
    def!(UNION, "BrickComponentType_WireGraph_Exec_Union");
    def!(BRANCH, "BrickComponentType_WireGraph_Exec_Branch");
    def!(PSEUDO_VAR, "BrickComponentType_WireGraphPseudo_Var");
    def!(VAR_GET, "BrickComponentType_WireGraph_Exec_Var_Get");
    def!(VAR_SET, "BrickComponentType_WireGraph_Exec_Var_Set");
    def!(
        VAR_INCREMENT,
        "BrickComponentType_WireGraph_Exec_Var_Increment"
    );
    def!(
        ARRAY_SET_AT_INDEX,
        "BrickComponentType_WireGraph_Exec_ArrayVar_SetAtIndex"
    );
    def!(
        ARRAY_GET_AT_INDEX,
        "BrickComponentType_WireGraph_Exec_ArrayVar_GetAtIndex"
    );
    def!(
        ARRAY_PUSH,
        "BrickComponentType_WireGraph_Exec_ArrayVar_Push"
    );
    def!(ARRAY_POP, "BrickComponentType_WireGraph_Exec_ArrayVar_Pop");
    def!(
        ARRAY_GET_LENGTH,
        "BrickComponentType_WireGraph_Exec_ArrayVar_GetLength"
    );
    def!(LOGICAL_NOT, "BrickComponentType_WireGraph_Expr_LogicalNOT");
    def!(
        STRING_LENGTH,
        "BrickComponentType_WireGraph_Expr_String_Length"
    );
    def!(MICROCHIP, "Component_Internal_Microchip");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = intern("hello");
        assert_eq!(resolve(s), "hello");
    }

    #[test]
    fn dedup() {
        let a = intern("world");
        let b = intern("world");
        assert_eq!(a, b);
    }

    #[test]
    fn static_intern() {
        let s = intern_static("static_str");
        assert_eq!(resolve(s), "static_str");
    }
}
