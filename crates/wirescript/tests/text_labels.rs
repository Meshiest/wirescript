//! Emitted saves attach a floating `Component_TextDisplay` name label to the
//! top-level chip, named chips, variables/arrays, and microchip I/O gates.

use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, FoldMode, compile_to_world};

fn is_text_display(c: &Box<dyn brdb::BrdbComponent>) -> bool {
    c.component_type()
        .map(|t| t.to_string() == "Component_TextDisplay")
        .unwrap_or(false)
}

/// The set of brick ids that actually carry a `Component_TextDisplay`. A dynamic
/// label wire must target one of these — a target brick absent from this set is
/// a dangling wire that fails to connect at load.
fn text_display_brick_ids(world: &brdb::World) -> std::collections::HashSet<usize> {
    world
        .grids
        .iter()
        .flat_map(|(_e, bricks)| bricks)
        .chain(world.bricks.iter())
        .filter(|b| b.components.iter().any(is_text_display))
        .filter_map(|b| b.get_id())
        .collect()
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
            fold_mode: FoldMode::Auto,
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
        fold_mode: FoldMode::Auto,
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

/// A global var/array written from inside a NAMED chip must keep its
/// on-brick name tag. The always-on boundary-pins pass rewires the write
/// through a MicrochipInput pin, so the tag lookup must follow that pin
/// back to the originating var node instead of stopping at the pin.
#[test]
fn var_tag_survives_boundary_pins_inside_named_chip() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    let src = "var names: string[]\n\
               var count: int = 0\n\
               chip Init() -> (code: int) {\n  \
                 names.push(\"a\")\n  \
                 count = count + 1\n  \
                 emit code = 7\n\
               }\n\
               in s: exec\n\
               let r = Init(exec = s)\n\
               out v = r.code\n";
    let cr = wirescript::compile::compile(CompileInput {
        source: src,
        file: "boundary_var_tag.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");
    let path = std::env::temp_dir().join("ws_boundary_var_tag_test.brz");
    std::fs::write(&path, &cr.brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut tags: Vec<(String, f32)> = Vec::new();
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
                let (Some(BrdbValue::String(text)), Some(BrdbValue::F32(line_height))) =
                    (c.get("Text"), c.get("LineHeight"))
                else {
                    continue;
                };
                tags.push((text.clone(), *line_height));
            }
        }
    }

    assert!(
        tags.contains(&("count".to_string(), 1.2)),
        "Var_Set gate inside the named chip should keep its \"count\" tag; got {tags:?}"
    );
    assert!(
        tags.contains(&("names".to_string(), 1.2)),
        "ArrayVar_Push gate inside the named chip should keep its \"names\" tag; got {tags:?}"
    );
}

/// A non-constant `@label(<expr>)` on a top-level variable is a DYNAMIC label:
/// the (string-coerced) value is wired into the label component's `Text` input
/// port. An `int` source is routed through a `FormatText` gate first, so the
/// wire originates at the gate's `Output` port.
#[test]
fn dynamic_int_label_wires_through_formattext() {
    let r = compile_to_world(
        CompileInput {
            source: "@label(hp) var hp: int = 100\nin go: exec\non go { hp = hp - 1 }\n",
            file: "dyn_label.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    let text_wires: Vec<_> = r
        .world
        .wires
        .iter()
        .filter(|w| {
            w.target.component_type.to_string() == "Component_TextDisplay"
                && w.target.port_name.to_string() == "Text"
        })
        .collect();
    assert_eq!(
        text_wires.len(),
        1,
        "exactly one wire should drive a label Text port; wires = {:?}",
        r.world.wires
    );
    // An int value is coerced to a string via a FormatText gate before the wire.
    assert_eq!(
        text_wires[0].source.port_name.to_string(),
        "Output",
        "an int label value is wired from a FormatText `Output`"
    );
    // The target brick must actually carry the text component (not dangling).
    assert!(
        text_display_brick_ids(&r.world).contains(&text_wires[0].target.brick_id),
        "the label wire must target a real Component_TextDisplay"
    );
}

/// `@label(myvar) var myvar` self-labels the variable with its own live value.
/// A `string` value needs no coercion — the variable's own `Value` output wires
/// straight into its label's `Text` port (no needless `FormatText` gate).
#[test]
fn dynamic_string_self_label_wires_value_directly() {
    let r = compile_to_world(
        CompileInput {
            source: "@label(myvar) var myvar: string = \"foo\"\nin go: exec\non go { myvar = \"bar\" }\n",
            file: "self_label.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    let text_wires: Vec<_> = r
        .world
        .wires
        .iter()
        .filter(|w| {
            w.target.component_type.to_string() == "Component_TextDisplay"
                && w.target.port_name.to_string() == "Text"
        })
        .collect();
    assert_eq!(
        text_wires.len(),
        1,
        "exactly one self-label Text wire; wires = {:?}",
        r.world.wires
    );
    // A string value is wired directly from the variable's `Value` output — no
    // FormatText coercion gate is inserted.
    assert_eq!(
        text_wires[0].source.port_name.to_string(),
        "Value",
        "a string self-label wires the variable's own `Value` output directly"
    );
    assert!(
        text_display_brick_ids(&r.world).contains(&text_wires[0].target.brick_id),
        "the label wire must target a real Component_TextDisplay"
    );
}

/// Regression: a dynamically-labeled `var` whose name starts with `_` (a legal
/// identifier whose floating name emit normally suppresses) must STILL get its
/// label `Component_TextDisplay` emitted as the wire target — otherwise Pass 3.5
/// wires a dangling `Text` target that fails to connect at load.
#[test]
fn dynamic_label_on_underscore_var_emits_wire_target() {
    let r = compile_to_world(
        CompileInput {
            source: "var src: int = 0\n@label(src) var _shown: int = 0\nin go: exec\non go { src = src + 1 }\n",
            file: "underscore.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    let text_wires: Vec<_> = r
        .world
        .wires
        .iter()
        .filter(|w| {
            w.target.component_type.to_string() == "Component_TextDisplay"
                && w.target.port_name.to_string() == "Text"
        })
        .collect();
    assert_eq!(
        text_wires.len(),
        1,
        "one dynamic label wire; wires = {:?}",
        r.world.wires
    );
    assert!(
        text_display_brick_ids(&r.world).contains(&text_wires[0].target.brick_id),
        "an underscore-named dynamic-labeled var must still emit its Component_TextDisplay \
         target (no dangling wire)"
    );
}

/// A module-level `@label(runtimeVar)` (blank-line separated at the top of the
/// file) labels the ROOT microchip dynamically — a wire runs into the root shell
/// brick's `Text` port. The label forward-references a var declared below it
/// (hoisting).
#[test]
fn module_label_wires_runtime_value_into_root_shell() {
    let r = compile_to_world(
        CompileInput {
            source: "@label(title)\n\nvar title: string = \"hi\"\nin go: exec\non go { title = \"go\" }\n",
            file: "modlabel.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    let text_wires: Vec<_> = r
        .world
        .wires
        .iter()
        .filter(|w| {
            w.target.component_type.to_string() == "Component_TextDisplay"
                && w.target.port_name.to_string() == "Text"
        })
        .collect();
    assert_eq!(
        text_wires.len(),
        1,
        "one root dynamic-label wire; wires = {:?}",
        r.world.wires
    );
    // The wire targets the ROOT microchip shell brick (bricks[0]).
    assert_eq!(
        Some(text_wires[0].target.brick_id),
        r.world.bricks[0].get_id(),
        "the module @label wire must target the root shell brick"
    );
}

/// A module `@label` whose expression is a string interpolation must NOT stack a
/// second (identity `{0}`) FormatText on top of the interpolation's own — the
/// interpolation already produces a string, so it wires directly. Exactly one
/// FormatText gate should exist (the interpolation's).
#[test]
fn interpolated_module_label_has_no_redundant_format() {
    let r = compile_to_world(
        CompileInput {
            source: "@label(\"var: ${v}\")\n\nvar v: string = \"hi\"\n",
            file: "interp.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile");
    let fmt_count = r
        .world
        .grids
        .iter()
        .flat_map(|(_e, b)| b)
        .chain(r.world.bricks.iter())
        .flat_map(|b| b.components.iter())
        .filter(|c| {
            c.component_type()
                .map(|t| t.to_string().contains("FormatText"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        fmt_count, 1,
        "one FormatText (the interpolation's), not a redundant `{{0}}` stacked on top"
    );
}

/// A module-level `@label("Constant")` bakes static title text on the root chip
/// (the constant path — no value is wired into any label).
#[test]
fn module_label_constant_bakes_root_title_without_a_wire() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    const SRC: &str = "@label(\"My Chip\")\n\nvar x: int = 0\n";

    // In-memory: a constant module label is static, so nothing wires into a
    // Component_TextDisplay Text port.
    let world = compile_to_world(
        CompileInput {
            source: SRC,
            file: "modconst.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile")
    .world;
    assert!(
        !world.wires.iter().any(|w| {
            w.target.component_type.to_string() == "Component_TextDisplay"
                && w.target.port_name.to_string() == "Text"
        }),
        "a constant module @label must not wire a value into a Text port"
    );

    // Round-trip: the root shell's label text is the override "My Chip", not
    // the module name.
    let cr = wirescript::compile::compile(CompileInput {
        source: SRC,
        file: "modconst.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");
    let path = std::env::temp_dir().join("ws_module_label_const_test.brz");
    std::fs::write(&path, &cr.brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut found_title = false;
    for gid in 1..8 {
        let Ok(chunks) = reader.brick_chunk_index(gid) else {
            break;
        };
        for chunk in chunks {
            if chunk.num_components == 0 {
                continue;
            }
            let (_soa, comps) = reader
                .component_chunk_soa(gid, chunk.index)
                .expect("read components");
            for c in comps {
                if let Some(BrdbValue::String(t)) = c.get("Text") {
                    if t == "My Chip" {
                        found_title = true;
                    }
                }
            }
        }
    }
    assert!(found_title, "constant module @label should bake \"My Chip\"");
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
        fold_mode: FoldMode::Auto,
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

/// A `var m: Map<K, V>` must get the same floating name label as an ordinary
/// var/array (its var name) — before, `Pseudo_MapVar` fell through the emit's
/// label match and got nothing.
#[test]
fn map_var_gets_a_name_label() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    let cr = wirescript::compile::compile(CompileInput {
        source: "var scores: Map<string, int>\nin t: exec\non t { let g = scores.get(\"a\") }\n",
        file: "maplabel.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");
    let path = std::env::temp_dir().join("ws_map_label_test.brz");
    std::fs::write(&path, &cr.brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut texts: Vec<String> = Vec::new();
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
                    texts.push(text.clone());
                }
            }
        }
    }
    assert!(
        texts.iter().any(|t| t == "scores"),
        "map var `scores` should get a floating name label; label texts: {texts:?}"
    );
}

/// The map-op `Key`/`Value` DATA fields must be baked with the map's CONCRETE
/// key/value type. A generic `any` Key bakes a float `0.0`; for a string-keyed
/// map that is a type the game rejects at load, failing every wire into the
/// Map_Get component (including its `Exec` — the "map get has no inbound exec"
/// bug). The `Key` field of a `Map<string, _>` get must therefore be a String.
#[test]
fn map_get_key_data_field_matches_key_type() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;

    let cr = wirescript::compile::compile(CompileInput {
        source: "var m: Map<string, int> = { \"a\" => 1 }\nin t: exec\non t { let g = m.get(\"a\") }\n",
        file: "mapkey.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");
    let path = std::env::temp_dir().join("ws_map_key_type_test.brz");
    std::fs::write(&path, &cr.brz).expect("write brz");
    let reader = brdb::Brz::open(&path).expect("open brz").into_reader();

    let mut checked = false;
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
                // The Map_Get component is the one carrying both `Key` and `bFound`.
                if let (Some(key), Some(_)) = (c.get("Key"), c.get("bFound")) {
                    checked = true;
                    assert!(
                        matches!(key, BrdbValue::String(_)),
                        "a string-keyed map's Map_Get `Key` data field must be a String, got {key:?}"
                    );
                }
            }
        }
    }
    assert!(checked, "expected a Map_Get component carrying a `Key` field");
}
