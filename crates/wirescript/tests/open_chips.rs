//! Non-root chips default open (`bCollapsed = false`); `@closed` collapses.
//! `@label` overrides port and chip display labels.

const SRC: &str = "\
in tick: exec\n\
@label(\"Do It!\") in go: exec\n\
chip Foo(x: int) -> (r: int) { out r = x + 1 }\n\
@closed chip { var hidden: int = 0 }\n\
@label(\"Adder\") chip on tick { }\n\
let f = Foo(1)\n\
out result = f.r\n";

#[test]
fn label_overrides_reach_the_serialized_labels() {
    use brdb::IntoReader;
    use brdb::schema::BrdbValue;
    use wirescript::{CompileInput, FoldMode};

    let cr = wirescript::compile::compile(CompileInput {
        source: SRC,
        file: "open_chips.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");
    let path = std::env::temp_dir().join("ws_open_chips_test.brz");
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
    // The @label'd port shows its label, not its identifier.
    assert!(texts.iter().any(|t| t == "Do It!"), "texts: {texts:?}");
    assert!(!texts.iter().any(|t| t == "go"), "texts: {texts:?}");
    // The @label'd anon chip shows its label on the shell brick.
    assert!(texts.iter().any(|t| t.contains("Adder")), "texts: {texts:?}");
}
