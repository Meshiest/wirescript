//! wirescript — a logic-brick scripting language that compiles to Brickadia
//! `.brz` world files.

pub mod analysis;
pub mod analyze;
pub mod ast;
pub mod catalog;
pub mod collections;
pub mod compile;
pub(crate) mod const_eval;
pub mod diagnostic;
pub mod emit;
pub(crate) mod hash;
pub mod intern;
pub mod ir;
pub mod layout;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;
pub(crate) mod scope;
pub mod typecheck;
pub(crate) mod template;
pub mod template_cache;
pub(crate) mod types;

pub use compile::{compile, on_big_stack, compile_with_opts, compile_with_loader, compile_with_progress, compile_to_world, diagnostics_only, disk_prefab_resolver, CompileError, CompileInput, CompileResult, CompileWorldResult, CompileProgress, FoldMode, ProgressCallback};
pub use diagnostic::{Diagnostic, Pos, Severity, SourceRange, Suppressions};
pub use emit::{build_world, emit_brz, field_enum_type, field_enum_values, EmitError, EmitOptions, NestedCompiler, Placement, PrefabResolver};
#[cfg(feature = "brdb-full")]
pub use emit::emit_brdb;
pub use ir::{GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, Type, Wire};
pub use layout::{layout, layout_options_for, layout_with_opts, LayoutMode, LayoutOptions, LayoutResult};
pub use lexer::{lex, LexResult, Token, TokenKind};
pub use parser::{parse, ParseResult};
pub use resolve::{resolve, FsLoader, MemLoader, ResolveResult};
