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
#[cfg(test)]
mod emit_tests;
pub(crate) mod hash;
#[cfg(test)]
mod layout_tests;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;
pub(crate) mod scope;
pub(crate) mod template;
pub mod template_cache;
#[cfg(test)]
pub(crate) mod test_support;
pub mod typecheck;
pub(crate) mod types;

// The backend: IR, layout and emit, re-exported at their old paths.
// `layout` brings the module and the `layout` function together.
pub use wirescript_ir::{collections, emit, intern, ir, layout, wsir};

pub use compile::{
    CompileError, CompileInput, CompileProgress, CompileResult, CompileWorldResult, FoldMode,
    FrontEnd, FrontEndStage, ProgressCallback, assemble_options, compile, compile_to_world,
    compile_with_loader, compile_with_opts, compile_with_progress, diagnostics_only,
    disk_prefab_resolver, front_end, front_end_staged, on_big_stack, prepare_program_for_text,
    program_settings,
};
pub use diagnostic::{Diagnostic, Pos, Severity, SourceRange, Suppressions};
#[cfg(feature = "brdb-full")]
pub use emit::emit_brdb;
pub use emit::{
    Assembly, EmitError, EmitOptions, NestedCompiler, Placement, PrefabResolver, assemble,
    assemble_with_phases, build_world, emit_brz, field_enum_type, field_enum_values,
};
pub use ir::{
    GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, Program, ProgramSettings,
    Type, Wire,
};
pub use layout::{LayoutMode, LayoutOptions, LayoutResult, layout_with_opts};
pub use lexer::{LexResult, Token, TokenKind, lex};
pub use parser::{ParseResult, parse};
pub use resolve::{FsLoader, MemLoader, ResolveResult, resolve};

#[cfg(test)]
mod grammar_tests;
