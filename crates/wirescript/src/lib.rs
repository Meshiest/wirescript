//! wirescript — a logic-brick scripting language that compiles to Brickadia
//! `.brz` world files.
//!
//! Phase 1 provides the IR + brz emitter. The parser / typecheck / lower
//! stages are ported in subsequent phases.

pub mod analysis;
pub mod analyze;
pub mod ast;
pub mod catalog;
pub mod compile;
pub mod diagnostic;
pub mod emit;
pub mod intern;
pub mod ir;
pub mod layout;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod resolve;
pub mod scope;
pub mod typecheck;
pub mod template;
pub mod template_cache;
pub mod types;

pub use compile::{compile, compile_with_opts, compile_with_progress, compile_to_world, disk_prefab_resolver, CompileError, CompileInput, CompileResult, CompileWorldResult, CompileProgress, ProgressCallback};
pub use diagnostic::{Diagnostic, Pos, Severity, SourceRange};
pub use emit::{build_world, emit_brz, EmitError, EmitOptions, Placement, PrefabResolver};
#[cfg(feature = "brdb-full")]
pub use emit::emit_brdb;
pub use ir::{GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, Type, Wire};
pub use layout::{layout, layout_with_opts, ChipLayoutMode, LayoutOptions, LayoutResult};
pub use lexer::{lex, LexResult, Token, TokenKind};
pub use parser::{parse, ParseResult};
pub use resolve::{resolve, FsLoader, MemLoader, ResolveResult};
