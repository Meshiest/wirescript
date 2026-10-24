//! The wirescript integration tests, built as one test binary: each module is
//! one area of the compiler exercised through its public API.

mod common;

mod array_ref;
mod as_convert;
mod asset_compare;
mod bool_container_literals;
mod code_layout_bus;
mod code_layout_comments;
mod code_layout_emit;
mod code_layout_rotation;
mod compiles_clean;
mod const_differential;
mod determinism;
mod diagnostic_regressions;
mod diagnostics_documented;
mod entity_custom_properties;
mod flat_emit;
mod fn_functions;
mod fold_emit;
mod fold_invariants;
mod for_emit;
mod format_corpus;
mod function_plane_emit;
mod game_build_gates;
mod invisible_emit;
mod layout_snapshots;
mod lsp_oom_repro;
mod open_chips;
mod paste_fixtures;
mod pollution;
mod port_coverage;
mod port_rerouters;
mod prefab_ref;
mod property_access;
mod ref_emit;
mod ref_initializers;
mod silent_fixtures;
mod template_snapshots;
mod text_labels;
mod wall_layout;
mod wiring_invariant;
mod ws_reachability;
mod wsir_roundtrip;
