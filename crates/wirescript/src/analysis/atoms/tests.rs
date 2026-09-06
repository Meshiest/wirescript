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

#[test]
fn atom_at_finds_an_atom_sharing_a_line_with_a_plain_colon() {
    // `atom_at` skips the document lex when the cursor's line holds no `:`
    // that could open an atom token. That guard has to survive a line whose
    // FIRST colon is an ordinary annotation separator.
    let src = "in go: exec\non go { let r: int = :bomber }";
    let a = atom_at(src, "t.ws", 1, col_of(src, 1, ":bomber")).expect("atom under cursor");
    assert_eq!(a.name, "bomber");
}

#[test]
fn atom_at_reads_a_char_column_not_a_byte_one() {
    // Token spans carry byte columns and the cursor arrives as a char column.
    // Without the conversion the accented `e` slid the cursor off the atom, so
    // hover and find-references on `:alpha` silently returned nothing.
    let src = "\
var v: int = 0
var s: string = \"x\"
in go: exec
on go {
  v = if s == \"\u{e9}\" then :alpha else :beta
}";
    let l = src.lines().nth(4).unwrap();
    let char_col = |needle: &str| l[..l.find(needle).unwrap()].chars().count();
    let a = atom_at(src, "t.ws", 4, char_col(":alpha")).expect("atom under cursor");
    assert_eq!(a.name, "alpha");
    let b = atom_at(src, "t.ws", 4, char_col(":beta")).expect("atom under cursor");
    assert_eq!(b.name, "beta");
}
