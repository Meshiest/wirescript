//! wirescript — a logic-brick scripting language that compiles to Brickadia
//! `.brz` world files.

pub mod analysis;
pub mod analyze;
pub mod ast;
pub mod catalog;
pub mod compile;
pub(crate) mod const_eval;
pub(crate) mod const_scope;
pub mod diagnostic;
pub(crate) mod hash;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;
pub(crate) mod scope;
pub mod typecheck;
pub(crate) mod template;
pub mod template_cache;
pub(crate) mod types;
#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod emit_tests;

// The backend: IR, layout and emit, re-exported at their old paths.
// `layout` brings the module and the `layout` function together.
pub use wirescript_ir::{collections, emit, intern, ir, layout, wsir};

pub use compile::{compile, on_big_stack, compile_with_opts, compile_with_loader, compile_with_progress, compile_to_world, diagnostics_only, front_end, front_end_staged, FrontEnd, FrontEndStage, disk_prefab_resolver, program_settings, prepare_program_for_text, assemble_options, CompileError, CompileInput, CompileResult, CompileWorldResult, CompileProgress, FoldMode, ProgressCallback};
pub use diagnostic::{Diagnostic, Pos, Severity, SourceRange, Suppressions};
pub use emit::{assemble, assemble_with_phases, build_world, emit_brz, Assembly, field_enum_type, field_enum_values, EmitError, EmitOptions, NestedCompiler, Placement, PrefabResolver};
#[cfg(feature = "brdb-full")]
pub use emit::emit_brdb;
pub use ir::{GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, Program, ProgramSettings, Type, Wire};
pub use layout::{layout_with_opts, LayoutMode, LayoutOptions, LayoutResult};
pub use lexer::{lex, LexResult, Token, TokenKind};
pub use parser::{parse, ParseResult};
pub use resolve::{resolve, FsLoader, MemLoader, ResolveResult};

#[cfg(test)]
mod grammar_tests;
