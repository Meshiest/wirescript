//! Ref storage as the game reads it back from a real `.brz`: handle-typed
//! containers and the null ref defaults.

mod common;

use brdb::schema::{BrdbValue, SavedBrickRef, WireValue};
use wirescript::{CompileInput, FoldMode};

const SRC: &str = "\
var p0: color\n\
var h: *color\n\
var anchor: brick\n\
var pixels: (*color)[]\n\
var rows: int[][]\n\
var bricks: brick[]\n\
var named: Map<string, *color>\n\
var grids: Map<string, int[]>\n\
in go: exec\n\
on go {\n\
  pixels.push(&p0)\n\
  h = pixels[0]\n\
  bricks.push(anchor)\n\
  named.set(\"a\", &p0)\n\
  grids.set(\"g\", rows[0])\n\
}\n";

fn components() -> Vec<brdb::schema::BrdbStruct> {
    let cr = wirescript::compile::compile(CompileInput {
        source: SRC,
        file: "ref_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");
    common::world_components(&cr.brz)
}

#[test]
fn ref_containers_use_the_matching_handle_types() {
    let (mut arrays, mut maps) = (Vec::new(), Vec::new());
    for c in components() {
        for v in c.properties.values() {
            match v {
                BrdbValue::DynamicArray(a) => arrays.push(a.element_type.clone()),
                BrdbValue::DynamicMap(m) => maps.push(m.value_type.clone()),
                _ => {}
            }
        }
    }
    let count = |xs: &[String], t: &str| xs.iter().filter(|x| *x == t).count();
    assert_eq!(count(&arrays, "component_handle"), 2, "pixels and rows: {arrays:?}");
    assert_eq!(count(&arrays, "brick_handle"), 1, "bricks: {arrays:?}");
    assert_eq!(count(&maps, "component_handle"), 2, "named and grids: {maps:?}");
}

#[test]
fn unassigned_refs_serialize_as_the_games_null_refs() {
    let null_brick = SavedBrickRef {
        brick_index_in_chunk: u32::MAX,
        ..Default::default()
    };
    let (mut component_refs, mut brick_refs) = (0, 0);
    for c in components() {
        let Some(v) = c.get("Value") else { continue };
        match WireValue::try_from(v) {
            Ok(WireValue::ComponentRef(r)) if r.brick == null_brick => component_refs += 1,
            Ok(WireValue::BrickRef(r)) if r == null_brick => brick_refs += 1,
            _ => {}
        }
    }
    assert!(component_refs >= 1, "`h: *color` holds a null component ref");
    assert!(brick_refs >= 1, "`anchor: brick` holds a null brick ref");
}
