use super::*;

fn spec(src: &str) -> (Option<DisplaySpec>, Vec<Diagnostic>) {
    let ast = crate::parser::parse(src, "t.ws").ast;
    let v = ast
        .decls
        .iter()
        .find_map(|d| match d {
            TopDecl::Var(v) if v.display.is_some() => Some(v.clone()),
            _ => None,
        })
        .expect("a @display var");
    let mut diags = Vec::new();
    (display_spec(&v, &mut diags), diags)
}

#[test]
fn a_display_stands_on_top_unless_a_side_says() {
    let (s, _) = spec("@display(width = 2, height = 2)\nvar s\n");
    assert_eq!(s.unwrap().side, Side::Top);
    let (s, _) = spec("@left @display(width = 2, height = 2)\nvar s\n");
    assert_eq!(s.unwrap().side, Side::Left);
    let (s, _) = spec("@display(width = 2, height = 2) @bottom\nvar s\n");
    assert_eq!(s.unwrap().side, Side::Bottom);
}

#[test]
fn an_offset_is_in_studs() {
    let (s, d) = spec("@display(width = 2, height = 2, offset = Vec(1, -2.5, 0.4))\nvar s\n");
    assert!(d.is_empty(), "{d:?}");
    assert_eq!(s.unwrap().offset, [10, -25, 4]);
    let (s, d) = spec("@display(width = 2, height = 2, offset = 3)\nvar s\n");
    assert!(s.is_none());
    assert_eq!(d[0].code, "WS127");
}

#[test]
fn the_option_tables_name_every_option_checked() {
    for o in DISPLAY_OPTIONS.iter().chain(SEAT_OPTIONS) {
        assert!(!o.doc.is_empty(), "{} has a doc", o.name);
    }
    assert!(DISPLAY_OPTIONS.iter().any(|o| o.name == "offset"));
}
