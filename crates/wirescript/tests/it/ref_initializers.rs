//! Ref initializers bake into the save: `var h: *int = &a`, an array of refs
//! or arrays (`[&a, &b]`, `[row0, row1]`) and a map with ref values. Each test
//! reads the written save back and resolves every baked ref to the brick and
//! component it names.

use brdb::ChunkIndex;
use brdb::schema::{BrdbValue, SavedComponentRef};
use wirescript::ir::gate_class as gc;

use crate::common;

/// A save address: grid, chunk, brick index in the chunk.
type Address = (u32, ChunkIndex, u32);

/// One labelled brick of a written save.
struct Labelled {
    label: String,
    at: Address,
    /// (component type index, component type name) of each component on it.
    types: Vec<(u16, String)>,
    /// The `Value` field of each component on it that has one.
    values: Vec<BrdbValue>,
}

fn labelled(brz: &[u8]) -> Vec<Labelled> {
    let save = wirescript_ir::readback::Save::read(brz);
    let names = save.component_type_names();
    let mut out = Vec::new();
    for (gid, chunk, soa, comps) in save.component_chunks() {
        let mut types: Vec<u32> = Vec::new();
        for c in &soa.component_type_counters {
            types.extend(std::iter::repeat_n(c.type_index, c.num_instances as usize));
        }
        let mut per: std::collections::BTreeMap<
            u32,
            (Option<String>, Vec<(u16, String)>, Vec<BrdbValue>),
        > = Default::default();
        for (i, c) in comps.iter().enumerate() {
            let e = per.entry(soa.component_brick_indices[i]).or_default();
            e.1.push((types[i] as u16, names[types[i] as usize].clone()));
            if let Some(BrdbValue::String(s)) = c.get("Text") {
                e.0 = Some(s.clone());
            }
            if let Some(v) = c.get("Value") {
                e.2.push(v.clone());
            }
        }
        for (b, (label, types, values)) in per {
            if let Some(label) = label {
                out.push(Labelled {
                    label,
                    at: (gid as u32, chunk, b),
                    types,
                    values,
                });
            }
        }
    }
    out
}

/// The storage brick labelled `label`: one holding a var, array or map.
fn storage<'a>(bricks: &'a [Labelled], label: &str) -> &'a Labelled {
    bricks
        .iter()
        .find(|b| {
            b.label == label
                && b.types
                    .iter()
                    .any(|(_, n)| [gc::VAR, gc::ARRAY_VAR, gc::MAP_VAR].contains(&n.as_str()))
        })
        .unwrap_or_else(|| panic!("no storage brick labelled {label}"))
}

/// Where `r`, held by a brick at `holder`, points: grid 0 is the holder's
/// grid with the chunk an offset from the holder's chunk; any other grid is
/// absolute.
fn resolve(r: &SavedComponentRef, holder: Address) -> (Address, u16) {
    let b = &r.brick;
    let at = if b.grid_persistent_index == 0 {
        let chunk = ChunkIndex {
            x: holder.1.x + b.chunk_index.x,
            y: holder.1.y + b.chunk_index.y,
            z: holder.1.z + b.chunk_index.z,
        };
        (holder.0, chunk, b.brick_index_in_chunk)
    } else {
        (
            b.grid_persistent_index,
            b.chunk_index,
            b.brick_index_in_chunk,
        )
    };
    (at, r.component_type_index)
}

/// The storage component of the brick labelled `label`: its address and the
/// type index of its var, array or map component.
fn storage_target(bricks: &[Labelled], label: &str) -> (Address, u16) {
    let b = storage(bricks, label);
    let ty = b
        .types
        .iter()
        .find(|(_, n)| [gc::VAR, gc::ARRAY_VAR, gc::MAP_VAR].contains(&n.as_str()))
        .unwrap()
        .0;
    (b.at, ty)
}

/// The refs held by the dynamic array in `label`'s `Value`.
fn array_refs(bricks: &[Labelled], label: &str) -> Vec<(Address, u16)> {
    let b = storage(bricks, label);
    let Some(BrdbValue::DynamicArray(a)) = b.values.first() else {
        panic!("{label} holds {:?}", b.values)
    };
    assert_eq!(a.element_type, "component_handle", "{label}");
    a.values
        .iter()
        .map(|v| {
            resolve(
                &SavedComponentRef::try_from(v).unwrap_or_else(|e| panic!("{label}: {e:?}")),
                b.at,
            )
        })
        .collect()
}

const SRC: &str = "
var a: int = 0
var b: int = 0
var h: *int = &a
var refs: (*int)[] = [&a, &b]
var row0: int[]
var row1: int[]
var rows: int[][] = [row0, row1]
var m: Map<string, int[]> = { \"x\" => row0, \"y\" => row1 }
";

#[test]
fn a_ref_var_holds_its_target() {
    let bricks = labelled(&common::brz(SRC, "refs.ws"));
    let h = storage(&bricks, "h");
    let Some(BrdbValue::Struct(_)) = h.values.first() else {
        panic!("h holds {:?}", h.values)
    };
    let r = SavedComponentRef::try_from(&h.values[0]).expect("h holds a component ref");
    assert_eq!(resolve(&r, h.at), storage_target(&bricks, "a"));
}

#[test]
fn an_array_of_refs_holds_each_target() {
    let bricks = labelled(&common::brz(SRC, "refs.ws"));
    assert_eq!(
        array_refs(&bricks, "refs"),
        vec![storage_target(&bricks, "a"), storage_target(&bricks, "b")]
    );
}

#[test]
fn an_array_of_arrays_holds_each_array() {
    let bricks = labelled(&common::brz(SRC, "refs.ws"));
    assert_eq!(
        array_refs(&bricks, "rows"),
        vec![
            storage_target(&bricks, "row0"),
            storage_target(&bricks, "row1")
        ]
    );
}

#[test]
fn a_map_of_arrays_holds_each_array_under_its_key() {
    let bricks = labelled(&common::brz(SRC, "refs.ws"));
    let m = storage(&bricks, "m");
    let Some(BrdbValue::DynamicMap(map)) = m.values.first() else {
        panic!("m holds {:?}", m.values)
    };
    assert_eq!(map.value_type, "component_handle");
    let got: Vec<(String, (Address, u16))> = map
        .entries
        .iter()
        .map(|(k, v)| {
            let BrdbValue::String(k) = k else {
                panic!("key {k:?}")
            };
            (
                k.clone(),
                resolve(
                    &SavedComponentRef::try_from(v).expect("a component ref"),
                    m.at,
                ),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("x".to_string(), storage_target(&bricks, "row0")),
            ("y".to_string(), storage_target(&bricks, "row1"))
        ]
    );
}

/// A ref initializer bakes, so it no longer warns that it is dropped.
#[test]
fn a_baked_ref_initializer_does_not_warn() {
    let codes = common::compile_diag_codes(SRC, "refs.ws");
    assert!(codes.is_empty(), "{codes:?}");
}

/// Refs inside a chip instance: one to the chip's own array is relative to
/// the holder, one to the program's array keeps its grid.
#[test]
fn a_chip_array_refs_its_own_and_outer_storage() {
    let src = "
var outer: int[]
chip Keep() -> () {
  var inner: int[]
  var both: int[][] = [inner, outer]
  in run: exec
  on run { both[0].push(1) }
}
in go: exec
on go { Keep() }
";
    let brz = common::brz(src, "chip.ws");
    let bricks = labelled(&brz);
    let both = storage(&bricks, "both");
    let refs = array_refs(&bricks, "both");
    assert_eq!(
        refs,
        vec![
            storage_target(&bricks, "inner"),
            storage_target(&bricks, "outer")
        ]
    );
    let BrdbValue::DynamicArray(raw) = &both.values[0] else {
        unreachable!()
    };
    let grids: Vec<u32> = raw
        .values
        .iter()
        .map(|v| {
            SavedComponentRef::try_from(v)
                .unwrap()
                .brick
                .grid_persistent_index
        })
        .collect();
    assert_eq!(grids[0], 0, "same grid: relative");
    assert_eq!(
        grids[1],
        storage(&bricks, "outer").at.0,
        "another grid: absolute"
    );
}

#[test]
fn a_ref_to_storage_declared_below_is_an_error() {
    let src = "var bad: (*int)[] = [&later]\nvar later: int = 0\n";
    assert_eq!(common::compile_diag_codes(src, "fwd.ws"), vec!["WS003"]);
}

#[test]
fn a_ref_element_of_the_wrong_type_is_an_error() {
    let src = "var a: int = 0\nvar rows: int[][] = [&a]\n";
    assert!(common::diag_codes(src).contains(&"WS003".to_string()));
}

/// Literals still bake as before into an array of refs' sibling types.
#[test]
fn a_constant_array_still_bakes_literals() {
    let bricks = labelled(&common::brz("var xs: int[] = [1, 2]\n", "lits.ws"));
    let xs = storage(&bricks, "xs");
    let Some(BrdbValue::DynamicArray(a)) = xs.values.first() else {
        panic!("{:?}", xs.values)
    };
    assert_eq!(a.values, vec![BrdbValue::I64(1), BrdbValue::I64(2)]);
}
