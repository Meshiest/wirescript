//! The one place a callable loop microchip's brick, ports and slots are paired.
//!
//! The feature is still moving on the game side, so a rename or a port-order
//! change must be a single edit here rather than a hunt through lowering and
//! emit. The component classes themselves live in `gate_class` beside every
//! other class, and the specs below are what give each one its brick asset,
//! its port names and its slot order.

use crate::ast::ForPat;
use crate::ir::Type;
use crate::ir::gate_class as gc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopKind {
    CountFor,
    ForEachArray,
    ForEachMap,
}

pub struct PlaneSpec {
    /// Component class of the outer shell brick.
    pub shell_class: &'static str,
    /// Brick asset of the outer shell. `brdb`'s `bricks.rs` does not expose a
    /// const for these yet, so emit builds a `BrickType::str` from this.
    pub brick_asset: &'static str,
    /// Component class of the in-plane entry gate.
    pub entry_class: &'static str,
    /// Brick asset of the entry gate.
    pub entry_brick_asset: &'static str,
    /// Entry-gate DATA output ports, in slot order. Excludes `Body`.
    pub entry_slots: &'static [&'static str],
    /// SOURCE-level names of the same slots, in the same order: what a
    /// diagnostic calls them and what a `{ }` binding spells. Kept beside
    /// `entry_slots` (the game-side port names) so the two orders cannot
    /// drift.
    pub slot_labels: &'static [&'static str],
    /// Which slot a SINGLE binding means. Arrays are the one exception: one
    /// binding means `Value`, not `Index`.
    pub single_binding_slot: usize,
    /// Shell data INPUT ports, in the order lowering drives them.
    pub driver_ports: &'static [&'static str],
}

static COUNT_FOR: PlaneSpec = PlaneSpec {
    shell_class: gc::MICROCHIP_COUNT_FOR,
    brick_asset: "B_1x1_Microchip_CountFor",
    entry_class: gc::CALLABLE_ENTRY_COUNT_FOR,
    entry_brick_asset: "B_1x1_Gate_CallableEntry_CountFor",
    entry_slots: &["Index"],
    slot_labels: &["index"],
    single_binding_slot: 0,
    driver_ports: &["Count", "FirstIndex", "Step"],
};

static FOR_EACH_ARRAY: PlaneSpec = PlaneSpec {
    shell_class: gc::MICROCHIP_FOR_EACH_ARRAY,
    brick_asset: "B_1x1_Microchip_ForEachArray",
    entry_class: gc::CALLABLE_ENTRY_FOR_EACH_ARRAY,
    entry_brick_asset: "B_1x1_Gate_CallableEntry_ForEachArray",
    entry_slots: &["Index", "Value"],
    slot_labels: &["index", "value"],
    single_binding_slot: 1,
    driver_ports: &["ArrayVarRef"],
};

static FOR_EACH_MAP: PlaneSpec = PlaneSpec {
    shell_class: gc::MICROCHIP_FOR_EACH_MAP,
    brick_asset: "B_1x1_Microchip_ForEachMap",
    entry_class: gc::CALLABLE_ENTRY_FOR_EACH_MAP,
    entry_brick_asset: "B_1x1_Gate_CallableEntry_ForEachMap",
    entry_slots: &["Key", "Value", "Index"],
    slot_labels: &["key", "value", "index"],
    single_binding_slot: 0,
    driver_ports: &["MapVarRef"],
};

/// A function definition: no driver pins and no entry slots. Its parameters are
/// `FunctionInput` gates inside the plane, mapped onto a call's `Input0..7`.
static FUNCTION_DEFINITION: PlaneSpec = PlaneSpec {
    shell_class: gc::MICROCHIP_FUNCTION_DEFINITION,
    brick_asset: "B_1x1_Microchip_FunctionDefinition",
    entry_class: gc::CALLABLE_ENTRY_FUNCTION,
    entry_brick_asset: "B_1x1_Gate_CallableEntry_Function",
    entry_slots: &[],
    slot_labels: &[],
    single_binding_slot: 0,
    driver_ports: &[],
};

pub fn function_plane_spec() -> &'static PlaneSpec {
    &FUNCTION_DEFINITION
}

pub fn plane_spec(kind: LoopKind) -> &'static PlaneSpec {
    match kind {
        LoopKind::CountFor => &COUNT_FOR,
        LoopKind::ForEachArray => &FOR_EACH_ARRAY,
        LoopKind::ForEachMap => &FOR_EACH_MAP,
    }
}

/// The spec behind a shell node's `gate_class`. Emit knows only the class, so
/// this is its route back to the brick asset; a plain microchip answers `None`
/// and keeps the ordinary shell brick.
pub fn plane_spec_for_class(class: &str) -> Option<&'static PlaneSpec> {
    match class {
        gc::MICROCHIP_COUNT_FOR => Some(&COUNT_FOR),
        gc::MICROCHIP_FOR_EACH_ARRAY => Some(&FOR_EACH_ARRAY),
        gc::MICROCHIP_FOR_EACH_MAP => Some(&FOR_EACH_MAP),
        gc::MICROCHIP_FUNCTION_DEFINITION => Some(&FUNCTION_DEFINITION),
        _ => None,
    }
}

/// Shell OUTPUT ports, identical across all three loop kinds.
pub const SHELL_OUTPUTS: &[&str] = &["Completed", "IterationsExecuted", "bAborted", "bRunning"];

/// The shell setting `@sliced` turns on. Off, the game runs the loop
/// within the tick it starts; the schema carries it on all three shells.
pub const RUN_ACROSS_TICKS: &str = "bRunAcrossTicks";

/// The record `let run = for` binds after a loop: surface field name, the shell
/// OUTPUT port that feeds it, and its type.
///
/// Same one-table rule as [`slot_types`]: typecheck declares the binding's
/// record type from this and lowering wires each field to the named shell
/// port from this, so the surface name and the game-side port name can only
/// be paired in one place.
pub fn result_fields() -> [(&'static str, &'static str, Type); 3] {
    [
        ("aborted", "bAborted", Type::Bool),
        ("iterations", "IterationsExecuted", Type::Int),
        ("running", "bRunning", Type::Bool),
    ]
}

/// The type of every entry slot of `kind`, in `entry_slots`/`slot_labels`
/// order.
///
/// A range's one slot is fixed by the gate. The two collection kinds take
/// their element types from the collection being iterated, which is what
/// `elem` carries: empty for a range, `[element]` for an array, `[key, value]`
/// for a map. A missing entry reads back as `Type::Any` rather than panicking,
/// so a caller that already reported a bad source (WS075) can still ask.
///
/// The one table. Typecheck declares each binding's type from it and lowering
/// declares the entry gate's output ports from it. A second copy could
/// disagree about, say, whether a map's `index` is an `int`, and nothing would
/// catch it: the wire would still be drawn, just typed wrong.
pub fn slot_types(kind: LoopKind, elem: &[Type]) -> Vec<Type> {
    let at = |i: usize| elem.get(i).cloned().unwrap_or(Type::Any);
    match kind {
        LoopKind::CountFor => vec![Type::Int],
        LoopKind::ForEachArray => vec![Type::Int, at(0)],
        LoopKind::ForEachMap => vec![at(0), at(1), Type::Int],
    }
}

/// Where one `for` binding lands once the kind's slot table has been applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForSlot {
    /// A `{ }` binding on a range names the loop's own slots: a range has no
    /// element value for the pattern to destructure.
    RangeSlotNames,
    /// The entry slot this binding takes, indexing `entry_slots` and
    /// `slot_labels`.
    Slot(usize),
    /// Past the last slot this kind has, so nothing binds.
    OverBound,
}

/// Resolve each of a `for`'s bindings onto its entry slot.
///
/// Typecheck, lowering, hover symbols and scoped refs all need this answer and
/// each reports a different failure, so the resolution lives here once and
/// every caller keeps its own error path.
pub fn for_slots(kind: LoopKind, bindings: &[ForPat]) -> Vec<ForSlot> {
    let spec = plane_spec(kind);
    bindings
        .iter()
        .enumerate()
        .map(|(i, pat)| {
            if kind == LoopKind::CountFor && matches!(pat, ForPat::Record { .. }) {
                return ForSlot::RangeSlotNames;
            }
            // A single binding on an array means the value, not the index;
            // ranges and maps take their slots in written order.
            let slot = if bindings.len() == 1 {
                spec.single_binding_slot
            } else {
                i
            };
            if slot < spec.entry_slots.len() {
                ForSlot::Slot(slot)
            } else {
                ForSlot::OverBound
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-checks every string in `plane_spec` against the bundled gate
    /// inventory (`catalog::default_catalog()`, the same JSON embedded via
    /// `include_str!` from `data/logic_gate_inventory.simple.json`),
    /// genuinely comparing name sets rather than only asserting
    /// non-emptiness. These strings are the entire deliverable of this
    /// module: a wrong one neither fails to compile nor fails at runtime in
    /// game, it produces a save that loads without error and silently does
    /// nothing. Sets, not sequences: the generator
    /// (`gen_inventory.mjs`) alphabetizes each gate's `inputs`/`outputs`
    /// arrays, so the dump cannot attest to slot order, only to which names
    /// exist.
    #[test]
    fn plane_specs_match_the_live_dump() {
        use crate::catalog::{default_catalog, Port};
        use crate::collections::HashSet;

        let catalog = default_catalog();

        fn names<'a>(ports: &'a [Port]) -> HashSet<&'a str> {
            ports.iter().map(|p| p.name.as_str()).collect()
        }

        for kind in [LoopKind::CountFor, LoopKind::ForEachArray, LoopKind::ForEachMap] {
            let s = plane_spec(kind);

            let shell = catalog.find_by_class(s.shell_class).unwrap_or_else(|| {
                panic!("{kind:?}: shell_class {:?} not found in the dump", s.shell_class)
            });
            assert_eq!(
                shell.brick_asset, s.brick_asset,
                "{kind:?}: brick_asset doesn't match the dump entry for shell_class {:?}",
                s.shell_class
            );

            let dump_driver_ports: HashSet<&str> = names(&shell.component.inputs)
                .into_iter()
                .filter(|n| *n != "Exec")
                .collect();
            let spec_driver_ports: HashSet<&str> = s.driver_ports.iter().copied().collect();
            assert_eq!(
                dump_driver_ports, spec_driver_ports,
                "{kind:?}: driver_ports {:?} doesn't match the dump's shell inputs (minus Exec)",
                s.driver_ports
            );

            let dump_shell_outputs: HashSet<&str> = names(&shell.component.outputs);
            let spec_shell_outputs: HashSet<&str> = SHELL_OUTPUTS.iter().copied().collect();
            assert_eq!(
                dump_shell_outputs, spec_shell_outputs,
                "{kind:?}: SHELL_OUTPUTS doesn't match the dump's shell outputs"
            );

            let entry = catalog.find_by_class(s.entry_class).unwrap_or_else(|| {
                panic!("{kind:?}: entry_class {:?} not found in the dump", s.entry_class)
            });
            assert_eq!(
                entry.brick_asset, s.entry_brick_asset,
                "{kind:?}: entry_brick_asset doesn't match the dump entry for entry_class {:?}",
                s.entry_class
            );

            let dump_entry_slots: HashSet<&str> = names(&entry.component.outputs)
                .into_iter()
                .filter(|n| *n != "Body")
                .collect();
            let spec_entry_slots: HashSet<&str> = s.entry_slots.iter().copied().collect();
            assert_eq!(
                dump_entry_slots, spec_entry_slots,
                "{kind:?}: entry_slots {:?} doesn't match the dump's entry outputs (minus Body)",
                s.entry_slots
            );

            assert!(
                s.single_binding_slot < s.entry_slots.len(),
                "{kind:?}: single_binding_slot out of range"
            );
            assert_eq!(
                s.slot_labels.len(),
                s.entry_slots.len(),
                "{kind:?}: slot_labels and entry_slots must describe the same slots"
            );
            // Two element types is the widest any kind takes (a map's K and
            // V); a range ignores them entirely.
            assert_eq!(
                slot_types(kind, &[Type::Int, Type::String]).len(),
                s.entry_slots.len(),
                "{kind:?}: slot_types must yield one type per slot"
            );

            // `let run = for` reads shell OUTPUT pins; a field naming a port the
            // shell does not have would wire from nowhere.
            for (field, port, _) in result_fields() {
                assert!(
                    SHELL_OUTPUTS.contains(&port),
                    "{kind:?}: the loop result's `{field}` names {port:?}, which is not a shell output"
                );
            }
        }
    }

    /// The function shell and entry name a class and brick the dump has, and
    /// the shell's one output is `Definition`.
    #[test]
    fn function_plane_spec_matches_the_live_dump() {
        let catalog = crate::catalog::default_catalog();
        let s = function_plane_spec();
        let shell = catalog.find_by_class(s.shell_class).expect("shell class in the dump");
        assert_eq!(shell.brick_asset, s.brick_asset);
        let outs: Vec<&str> = shell.component.outputs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(outs, vec!["Definition"]);
        let entry = catalog.find_by_class(s.entry_class).expect("entry class in the dump");
        assert_eq!(entry.brick_asset, s.entry_brick_asset);
        assert!(catalog.find_by_class(gc::FUNCTION_INPUT).is_some());
        let output = catalog.find_by_class(gc::FUNCTION_OUTPUT).expect("FunctionOutput in the dump");
        let ins: Vec<&str> = output.component.inputs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(ins, vec!["Value"]);
        assert!(output.component.outputs.is_empty());
        assert!(gc::is_plane_shell(s.shell_class));
    }

    /// Each kind's own `shell_class` must round-trip through the class lookup,
    /// and an ordinary microchip must not.
    #[test]
    fn every_shell_class_has_a_spec() {
        for kind in [LoopKind::CountFor, LoopKind::ForEachArray, LoopKind::ForEachMap] {
            let s = plane_spec(kind);
            assert!(
                gc::is_plane_shell(s.shell_class),
                "{kind:?}: is_plane_shell rejects its own shell_class"
            );
            let found = plane_spec_for_class(s.shell_class)
                .unwrap_or_else(|| panic!("{kind:?}: no spec for shell_class {:?}", s.shell_class));
            assert_eq!(found.brick_asset, s.brick_asset);
        }
        assert!(plane_spec_for_class(gc::MICROCHIP_ALT).is_none());
        assert!(plane_spec_for_class(gc::MICROCHIP).is_none());
    }

    fn pat_range() -> crate::diagnostic::SourceRange {
        crate::diagnostic::SourceRange::new(
            "plane.ws",
            crate::diagnostic::Pos::default(),
            crate::diagnostic::Pos::default(),
        )
    }

    fn name_pat(n: &str) -> ForPat {
        ForPat::Name(n.to_string(), pat_range())
    }

    fn record_pat() -> ForPat {
        ForPat::Record { fields: Vec::new(), range: pat_range() }
    }

    #[test]
    fn a_single_binding_takes_the_kinds_own_slot() {
        assert_eq!(
            for_slots(LoopKind::ForEachArray, &[name_pat("v")]),
            vec![ForSlot::Slot(1)]
        );
        assert_eq!(
            for_slots(LoopKind::ForEachMap, &[name_pat("k")]),
            vec![ForSlot::Slot(0)]
        );
        assert_eq!(
            for_slots(LoopKind::CountFor, &[name_pat("i")]),
            vec![ForSlot::Slot(0)]
        );
    }

    #[test]
    fn several_bindings_take_their_written_order() {
        assert_eq!(
            for_slots(LoopKind::ForEachArray, &[name_pat("i"), name_pat("v")]),
            vec![ForSlot::Slot(0), ForSlot::Slot(1)]
        );
    }

    #[test]
    fn a_record_pattern_names_slots_only_on_a_range() {
        assert_eq!(
            for_slots(LoopKind::CountFor, &[record_pat()]),
            vec![ForSlot::RangeSlotNames]
        );
        // An array element IS a value, so `{ }` there takes a slot and
        // destructures what occupies it.
        assert_eq!(
            for_slots(LoopKind::ForEachArray, &[record_pat()]),
            vec![ForSlot::Slot(1)]
        );
    }

    #[test]
    fn bindings_past_the_last_slot_are_over_bound() {
        let three = [name_pat("a"), name_pat("b"), name_pat("c")];
        assert_eq!(
            for_slots(LoopKind::ForEachArray, &three),
            vec![ForSlot::Slot(0), ForSlot::Slot(1), ForSlot::OverBound]
        );
        assert!(!for_slots(LoopKind::ForEachMap, &three).contains(&ForSlot::OverBound));
    }

    #[test]
    fn arrays_are_the_single_binding_exception() {
        // One binding on an array means Value; ranges and maps take slot 0.
        assert_eq!(plane_spec(LoopKind::CountFor).single_binding_slot, 0);
        assert_eq!(plane_spec(LoopKind::ForEachMap).single_binding_slot, 0);
        assert_eq!(plane_spec(LoopKind::ForEachArray).single_binding_slot, 1);
        assert_eq!(plane_spec(LoopKind::ForEachArray).entry_slots[1], "Value");
    }
}
