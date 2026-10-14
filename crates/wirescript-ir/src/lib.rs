//! The wire-graph backend of Wirescript, usable without the language.
//!
//! A [`Program`] is a gate graph: gates by inventory class, the wires between
//! their ports, nested chips, and module-level settings. [`assemble`] lays it
//! out and builds the Brickadia save. Wirescript lowers source to a `Program`;
//! any other producer can build one with [`ProgramBuilder`] or read one from
//! `.wsir` text with [`wsir::parse_program`] (grammar: `FORMAT.md`), and check
//! it with [`Program::validate`] before assembling.

pub mod catalog;
pub mod collections;
pub mod diagnostic;
pub mod emit;
pub mod intern;
pub mod ir;
pub mod layout;
#[cfg(feature = "test-internals")]
pub mod readback;
pub mod wsir;

pub use diagnostic::{Diagnostic, Pos, Severity, SourceRange};
pub use emit::{Assembly, EmitError, EmitOptions, NestedCompiler, Placement, PrefabResolver, assemble, assemble_with_phases, build_world, emit_brz};
#[cfg(feature = "brdb-full")]
pub use emit::emit_brdb;
pub use ir::{
    BuildError, GateIO, LayoutMode, Literal, Module, ModuleRef, Node, NodeId, NodeKind, PortRef, PortSpec, Program,
    ProgramBuilder, ProgramSettings, SourceComment, SourceMap, Type, ValidationError, Wire,
};
pub use layout::{LayoutOptions, LayoutResult, layout, layout_with_opts};
