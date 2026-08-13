use super::*;

fn col_of(src: &str, line: usize, needle: &str) -> usize {
    src.lines().nth(line).unwrap().find(needle).unwrap()
}

#[test]
fn atom_at_finds_the_atom_and_its_hash_value() {
    let src = "in go: exec\non go { let r = :bomber }";
    let col = col_of(src, 1, ":bomber");
    let a = atom_at(src, "t.ws", 1, col).expect("atom under cursor");
    assert_eq!(a.name, "bomber");
    assert_eq!(a.value, crate::hash::atom_hash("bomber"));
}

#[test]
fn atom_at_ignores_type_annotations_and_string_lookalikes() {
    let src = "in x: int\nlet s = \":notanatom\"";
    // `int` in `x: int` is a type annotation, not an atom.
    let tc = col_of(src, 0, "int");
    assert!(atom_at(src, "t.ws", 0, tc).is_none());
    // `:notanatom` inside a string literal is not an atom token.
    let sc = col_of(src, 1, "notanatom");
    assert!(atom_at(src, "t.ws", 1, sc).is_none());
}

#[test]
fn atom_references_finds_values_and_map_keys_by_name() {
    // `:bomber` as a value AND as a map key are both found; `:seer` is separate.
    let src =
        "in go: exec\non go {\n  let r = :bomber\n  let m = { :bomber: \"B\", :seer: \"S\" }\n}";
    let bomber = atom_references(src, "t.ws", "bomber");
    assert_eq!(bomber.len(), 2, "value + map-key :bomber; got {bomber:?}");
    assert_eq!(atom_references(src, "t.ws", "seer").len(), 1);
    assert!(atom_references(src, "t.ws", "missing").is_empty());
}
