pub mod assets;
pub mod types;
pub mod symbols;
pub mod text;
pub mod hover;
pub mod definition;
pub mod references;
pub mod gate_docs;
pub mod format;
pub mod resource_estimate;
pub mod inlay_hints;

use std::collections::HashMap;
use crate::ir::Type;

pub type TypeMap = HashMap<(std::sync::Arc<str>, usize, usize), Type>;
pub type IfContextMap = HashMap<(std::sync::Arc<str>, usize), bool>;
pub type VarReadContextMap = HashMap<(std::sync::Arc<str>, usize), bool>;

pub use assets::{asset_exists, asset_names, asset_type_exists, asset_types};
pub use symbols::SymbolDef;
pub use types::{type_str, type_expr_str, infer_expr_type, type_from_name, receiver_methods};
pub use text::{word_at, find_enclosing_call, named_arg_value, find_asset_refs, asset_ref_at, AssetRef};
pub use hover::hover_at;
pub use definition::{definition_at, Location};
pub use references::{find_all_references, find_name_range, TextRange};
pub use symbols::{collect_symbols, collect_symbols_for_file};
pub use gate_docs::gate_docs;
pub use format::format_wirescript;
pub use resource_estimate::{ResourceEstimate, collect_estimates, lookup_estimate};
pub use inlay_hints::{collect_inlay_hints, InlayHintInfo, InlayHintKind};
