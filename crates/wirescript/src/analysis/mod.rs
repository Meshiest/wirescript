pub mod assets;
pub mod atoms;
pub mod completion;
pub mod definition;
pub mod doc_links;
pub mod format;
pub mod gate_docs;
pub mod hover;
pub mod inlay_hints;
pub mod references;
pub mod resource_estimate;
pub mod scoped_refs;
pub mod symbols;
pub mod text;
pub mod types;
pub mod visit;

use crate::collections::HashMap;
use crate::ir::Type;

pub type TypeMap = HashMap<(std::sync::Arc<str>, usize, usize), Type>;
pub type IfContextMap = HashMap<(std::sync::Arc<str>, usize), bool>;
pub type VarReadContextMap = HashMap<(std::sync::Arc<str>, usize), bool>;
pub type NarrowedReadMap =
    HashMap<(std::sync::Arc<str>, usize, usize), crate::typecheck::NarrowedRead>;

pub use assets::{
    asset_exists, asset_names, asset_type_exists, asset_type_for_port, asset_types,
    component_member_completions, environment_completions,
};
pub use symbols::SymbolDef;
pub use text::{
    AssetRef, asset_ref_at, find_asset_refs, find_enclosing_call, member_receiver_at,
    named_arg_value, param_names, record_field_names, swizzle_fields, word_at,
};
pub use types::{
    CollectionKind, collection_kind, infer_expr_type, receiver_methods, type_expr_str,
    type_from_name, type_str, user_receiver_methods,
};
// Column-convention conversions: the lexer counts BYTES, `analysis` counts
// CHARS, and an editor counts UTF-16 code units. A frontend converts at its
// own protocol boundary, with these and nothing else.
pub use atoms::{AtomRef, atom_at, atom_references};
pub use completion::{
    Completion, CompletionKind, completions, match_arm_head_scrutinee_at, prefab_path_fragment,
};
pub use definition::{Location, definition_at};
pub use doc_links::{DOCS_URL, Doc, doc_url};
pub use format::{FormatOptions, LineEnding, format_wirescript, format_with};
pub use gate_docs::gate_docs;
pub use hover::hover_at;
pub use hover::{MatchArmsFill, fill_match_arms_at};
pub use hover::{RecordFill, fill_record_at};
pub use inlay_hints::{InlayHintInfo, InlayHintKind, collect_inlay_hints};
pub use references::{TextRange, find_name_range, rename_edit_text};
pub use resource_estimate::{ResourceEstimate, collect_estimates, lookup_estimate};
pub use scoped_refs::{
    CrossFile, RefNs, RefSite, RefTarget, SemSpan, SemTokenKind, field_name_at, prepare_rename_at,
    references_at, references_to_export, semantic_tokens,
};
pub use symbols::{collect_symbols, collect_symbols_for_file, resolve_symbol};
pub use text::{
    byte_to_char_col, char_col_to_byte, char_col_to_utf16_col, cursor_byte_offset, line_start_byte,
    line_text, utf16_col_to_char_col,
};
pub use visit::visit_program;
