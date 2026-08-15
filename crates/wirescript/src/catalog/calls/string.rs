//! String calls: formatting, the string methods, parsing, and char codes.

use super::*;

pub(super) fn register(m: &mut HashMap<&'static str, CallSpec>) {
    // ---- Format text ---------------------------------------------------
    // Wraps the FormatText gate. Format string may be wired or literal.
    // Up to 7 inputs (InputA-G). Output is the formatted string.
    m.insert(
        "Fmt",
        CallSpec {
            name: "Fmt",
            gate_class: gc::STRING_FORMAT_TEXT,
            params: vec![
                CallParam::req("format", WirePort::FormatString, Type::String),
                CallParam::opt("a", WirePort::InputA, Type::Any),
                CallParam::opt("b", WirePort::InputB, Type::Any),
                CallParam::opt("c", WirePort::InputC, Type::Any),
                CallParam::opt("d", WirePort::InputD, Type::Any),
                CallParam::opt("e", WirePort::InputE, Type::Any),
                CallParam::opt("f", WirePort::InputF, Type::Any),
                CallParam::opt("g", WirePort::InputG, Type::Any),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: None,
        },
    );

    // ---- String operations -----------------------------------------------
    m.insert(
        "Length",
        CallSpec {
            name: "Length",
            gate_class: gc::STRING_LENGTH,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Contains",
        CallSpec {
            name: "Contains",
            gate_class: gc::STRING_CONTAINS,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("search", WirePort::Search, Type::String),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "StartsWith",
        CallSpec {
            name: "StartsWith",
            gate_class: gc::STRING_STARTS_WITH,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("prefix", WirePort::Prefix, Type::String),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "EndsWith",
        CallSpec {
            name: "EndsWith",
            gate_class: gc::STRING_ENDS_WITH,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("suffix", WirePort::Suffix, Type::String),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Bool,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Substring",
        CallSpec {
            name: "Substring",
            gate_class: gc::STRING_SUBSTRING,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("start", WirePort::Start, Type::Int),
                CallParam::req("length", WirePort::Length, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Replace",
        CallSpec {
            name: "Replace",
            gate_class: gc::STRING_REPLACE,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("search", WirePort::Search, Type::String),
                CallParam::req("replacement", WirePort::Replacement, Type::String),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
                CallParam::opt("maxReplacements", WirePort::MaxReplacements, Type::Int),
                CallParam::opt("start", WirePort::Start, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Find",
        CallSpec {
            name: "Find",
            gate_class: gc::STRING_FIND,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("search", WirePort::Search, Type::String),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
                CallParam::opt("start", WirePort::Start, Type::Int),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::Int,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Split",
        CallSpec {
            name: "Split",
            gate_class: gc::STRING_SPLIT,
            params: vec![
                CallParam::req("s", WirePort::Input, Type::String),
                CallParam::req("delimiter", WirePort::Delimiter, Type::String),
                CallParam::opt("occurrence", WirePort::Occurrence, Type::Int),
                CallParam::opt("caseSensitive", WirePort::BCaseSensitive, Type::Bool),
            ],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Left,
                ty: Type::Record(vec![
                    ("Left".into(), Type::String),
                    ("Right".into(), Type::String),
                    ("Found".into(), Type::Bool),
                ]),
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "ToLower",
        CallSpec {
            name: "ToLower",
            gate_class: gc::STRING_TO_LOWER,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "ToUpper",
        CallSpec {
            name: "ToUpper",
            gate_class: gc::STRING_TO_UPPER,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "Trim",
        CallSpec {
            name: "Trim",
            gate_class: gc::STRING_TRIM,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Output,
                ty: Type::String,
            }],
            receiver: Some(Type::String),
        },
    );

    // ---- String parsing (pure) -------------------------------------------
    m.insert(
        "ParseInt",
        CallSpec {
            name: "ParseInt",
            gate_class: gc::STRING_PARSE_INT,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            // `.Value` (auto-unwrapped) is the parsed int; `.Success` is false
            // when the string wasn't a valid integer.
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Record(vec![
                    ("Value".into(), Type::Int),
                    ("Success".into(), Type::Bool),
                ]),
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "ParseNumber",
        CallSpec {
            name: "ParseNumber",
            gate_class: gc::STRING_PARSE_NUMBER,
            params: vec![CallParam::req("s", WirePort::Input, Type::String)],
            exec: false,
            // `.Value` (auto-unwrapped) is the parsed float; `.Success` is false
            // when the string wasn't a valid number.
            outputs: vec![CallOutput {
            field: None,
                port: WirePort::Value,
                ty: Type::Record(vec![
                    ("Value".into(), Type::Float),
                    ("Success".into(), Type::Bool),
                ]),
            }],
            receiver: Some(Type::String),
        },
    );

    // Single-character string <-> unicode codepoint.
    m.insert(
        "ToCharCode",
        CallSpec {
            name: "ToCharCode",
            gate_class: gc::STRING_CHAR_TO_CODEPOINT,
            params: vec![CallParam::req("character", WirePort::Character, Type::String)],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::Codepoint,
                ty: Type::Record(vec![
                    ("Codepoint".into(), Type::Int),
                    ("Success".into(), Type::Bool),
                ]),
            }],
            receiver: Some(Type::String),
        },
    );
    m.insert(
        "FromCharCode",
        CallSpec {
            name: "FromCharCode",
            gate_class: gc::STRING_CODEPOINT_TO_CHAR,
            params: vec![CallParam::req("codepoint", WirePort::Codepoint, Type::Int)],
            exec: false,
            outputs: vec![CallOutput {
                field: None,
                port: WirePort::Character,
                ty: Type::Record(vec![
                    ("Character".into(), Type::String),
                    ("Success".into(), Type::Bool),
                ]),
            }],
            receiver: None,
        },
    );
}
