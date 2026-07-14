//! Emitted saves attach a floating `Component_TextDisplay` name label to the
//! top-level chip, named chips, variables/arrays, and microchip I/O gates.

use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, compile_to_world};

fn is_text_display(c: &Box<dyn brdb::BrdbComponent>) -> bool {
    c.component_type()
        .map(|t| t.to_string() == "Component_TextDisplay")
        .unwrap_or(false)
}

const SRC: &str = "var counter: int = 0\n\
                   in tick: exec\n\
                   on tick { counter = counter + 1 }\n\
                   chip Foo(x: int) -> (r: int) { out r = x + 1 }\n\
                   let f = Foo(counter)\n\
                   out result = f.r\n";

#[test]
fn labels_attach_to_chip_var_and_io_bricks() {
    let r = compile_to_world(
        CompileInput {
            source: SRC,
            file: "labels.ws",
            module_name: None,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    // The label font must be registered as an external asset.
    assert!(
        r.world
            .global_data
            .external_asset_references
            .iter()
            .any(|(t, n)| t == "BrickFontDescriptor" && n == "IosevkaTerm"),
        "label font should be an external asset reference, got {:?}",
        r.world.global_data.external_asset_references
    );

    // The top-level chip brick (main grid) carries a label named after the
    // entry file's stem.
    let root_chip = &r.world.bricks[0];
    assert!(
        root_chip.components.iter().any(is_text_display),
        "top-level chip brick should carry a text label"
    );

    // One label per named element — root chip, `counter`, `tick`, `result`,
    // the `Foo` chip brick, Foo's internal `x`/`r` I/O gates, and Foo's
    // synthesized `_exec_in`/`_exec_out` ports (both read `exec`, see
    // microchip_io_label) — plus a smaller variable tag on the Var_Get and
    // Var_Increment gates from the handler, plus the two invisible plane
    // header bricks (root plane + Foo plane).
    let labeled: Vec<String> = r
        .world
        .grids
        .iter()
        .flat_map(|(_e, bricks)| bricks)
        .chain(r.world.bricks.iter())
        .filter(|b| b.components.iter().any(is_text_display))
        .map(|b| {
            b.components
                .iter()
                .filter_map(|c| c.component_type().map(|t| t.to_string()))
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect();
    assert_eq!(
        labeled.len(),
        13,
        "expected root + counter + tick + result + Foo + x + r + 2 exec ports + get/increment tags + 2 plane headers, got {labeled:#?}"
    );
}

/// Roundtrip through the serialized .brz and check the label contents:
/// texts, face (Z_Positive), outline (Outlined, 4px), and offsets.
#[test]
fn labels_serialize_with_style() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    let cr = wirescript::compile::compile(CompileInput {
        source: SRC,
        file: "labels.ws",
        module_name: None,
    })
    .expect("should compile to brz");
    let path = std::env::temp_dir().join("ws_text_labels_test.brz");
    std::fs::write(&path, &cr.brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut texts: Vec<(String, f32)> = Vec::new();
    for gid in 1..32 {
        let chunks = match reader.brick_chunk_index(gid) {
            Ok(c) => c,
            Err(_) => break,
        };
        for chunk in chunks {
            if chunk.num_components == 0 {
                continue;
            }
            let (_soa, comps) = reader
                .component_chunk_soa(gid, chunk.index)
                .expect("read components");
            for c in comps {
                // TextDisplay is the only struct here with a Face field.
                let (Some(BrdbValue::String(text)), Some(BrdbValue::Enum(face))) =
                    (c.get("Text"), c.get("Face"))
                else {
                    continue;
                };
                assert_eq!(
                    face.get_value_raw(),
                    4,
                    "label {text:?} should sit on the +Z face"
                );
                match c.get("Outline") {
                    Some(BrdbValue::Enum(outline)) => assert_eq!(
                        outline.get_value_raw(),
                        2,
                        "label {text:?} should use EBRTextOutline::Outlined"
                    ),
                    other => panic!("label {text:?} missing Outline enum, got {other:?}"),
                }
                match c.get("OutlineWidth") {
                    Some(BrdbValue::F32(w)) => assert_eq!(*w, 4.0),
                    other => panic!("label {text:?} missing OutlineWidth, got {other:?}"),
                }
                let line_height = match c.get("LineHeight") {
                    Some(BrdbValue::F32(h)) => *h,
                    other => panic!("label {text:?} missing LineHeight, got {other:?}"),
                };
                texts.push((text.clone(), line_height));
            }
        }
    }

    texts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // Element names at full size; the handler's Var_Get + Var_Increment
    // gates carry smaller `counter` tags; Foo's synthesized `_exec_in`/
    // `_exec_out` ports each read `exec`; the root and Foo planes each get an
    // invisible header brick with a `<size="96">{title}</>` text (no doc
    // comments here, so no lines follow the title).
    let expected = [
        ("<size=\"96\">Foo</>", 2.4),
        ("<size=\"96\">labels</>", 2.4),
        ("Foo", 2.4),
        ("counter", 1.2),
        ("counter", 1.2),
        ("counter", 2.4),
        ("exec", 2.4),
        ("exec", 2.4),
        ("labels", 2.4),
        ("r", 2.4),
        ("result", 2.4),
        ("tick", 2.4),
        ("x", 2.4),
    ];
    let expected: Vec<(String, f32)> = expected
        .iter()
        .map(|(t, h)| (t.to_string(), *h))
        .collect();
    assert_eq!(texts, expected, "serialized label texts + sizes");
}

/// A chip's `///` doc comment renders on the header, below the `<size="96">`
/// title line.
#[test]
fn doc_comment_renders_under_the_title() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    let src = "/// Adds one to x.\n\
               /// Pure and simple.\n\
               chip Foo(x: int) -> (r: int) { out r = x + 1 }\n\
               let f = Foo(1)\n\
               out result = f.r\n";
    let cr = wirescript::compile::compile(CompileInput {
        source: src,
        file: "docs.ws",
        module_name: None,
    })
    .expect("should compile");
    let path = std::env::temp_dir().join("ws_header_doc_test.brz");
    std::fs::write(&path, &cr.brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut found = false;
    for gid in 1..32 {
        let chunks = match reader.brick_chunk_index(gid) {
            Ok(c) => c,
            Err(_) => break,
        };
        for chunk in chunks {
            if chunk.num_components == 0 {
                continue;
            }
            let (_soa, comps) = reader
                .component_chunk_soa(gid, chunk.index)
                .expect("read components");
            for c in comps {
                if let Some(BrdbValue::String(text)) = c.get("Text") {
                    if text == "<size=\"96\">Foo</>\n\nAdds one to x.\nPure and simple." {
                        found = true;
                    }
                }
            }
        }
    }
    assert!(found, "expected the Foo header with its doc comment");
}
