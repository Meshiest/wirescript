//! The `.wsir` text form against real programs: every fixture and example
//! that compiles is written as text, parsed back, and assembled. The text
//! must be a fixed point of write -> parse -> write, and the assembled save
//! must be byte-identical to the one compiled directly. (`just ir-oracle
//! --via-ir` runs the same check over the whole oracle set.)

use std::path::{Path, PathBuf};

use wirescript::{CompileInput, EmitOptions, FoldMode};

mod common;

fn ws_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            ws_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "ws") {
            out.push(p);
        }
    }
}

/// Compile `src` as `file`, then go through text: returns the canonical text,
/// the direct `.brz`, and the `.brz` assembled from the text.
fn through_text(src: &str, file: &str) -> Option<(String, Vec<u8>, Vec<u8>)> {
    let result = wirescript::compile_to_world(
        CompileInput { source: src, file, module_name: None, fold_mode: FoldMode::Auto },
        EmitOptions::default(),
    )
    .ok()?;
    let direct = result.world.to_brz_vec().expect("encode");
    let ir_path = Path::new(file).with_extension("wsir");
    let mut program = result.program;
    wirescript::prepare_program_for_text(&mut program, file, &ir_path, FoldMode::Auto).expect("prepare");
    let text = wirescript::wsir::program_to_string(&program).expect("write");

    let ir_file = ir_path.to_string_lossy().into_owned();
    let parsed = wirescript::wsir::parse_program(&text, &ir_file).unwrap_or_else(|e| panic!("{file}: {e}"));
    let errors = parsed.validate();
    assert!(errors.is_empty(), "{file}: a lowered program fails validation: {:?}", &errors[..errors.len().min(5)]);
    if let Err(e) = wirescript::wsir::check_fixed_point(&parsed, &text) {
        panic!("{file}: write -> parse -> write is not a fixed point: {e}");
    }
    let assembled = wirescript::assemble(&parsed, &wirescript::assemble_options(&ir_file, FoldMode::Auto))
        .unwrap_or_else(|e| panic!("{file}: assemble: {e}"))
        .world
        .to_brz_vec()
        .expect("encode");
    Some((text, direct, assembled))
}

#[test]
fn fixtures_and_examples_survive_the_text_form() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    ws_files(&root.join("tests/fixtures"), &mut files);
    ws_files(&root.join("../../examples"), &mut files);
    assert!(files.len() > 20, "found only {} .ws files", files.len());
    let mut checked = 0;
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let file = path.to_string_lossy();
        // A fixture that is meant to fail to compile has nothing to write.
        let Some((_, direct, assembled)) = through_text(&src, &file) else { continue };
        assert!(direct == assembled, "{file}: the save assembled from text differs from the direct compile");
        checked += 1;
    }
    assert!(checked > 20, "only {checked} files compiled");
}

/// The chip example from the IR crate's spec: `--emit-ir` gives the same
/// shape (an `in exec`, a `Var`, a closed chip with boundary pins fed from
/// the parent and a `!layout_after` list, the `Var_Set`, and the typed
/// output), and its text assembles to the same save.
#[test]
fn the_chip_example_has_the_documented_shape() {
    let src = "in go: exec\nvar n: int = 0\n@closed chip {\n  in tick: exec\n  on tick { n = n + 1 }\n}\non go { n = 5 }\nout count: int = n\n";
    let (text, direct, assembled) = through_text(src, "chip.ws").expect("compiles");
    assert!(direct == assembled, "the chip example assembles differently from the direct compile:\n{text}");
    for shape in [
        "source_filename = \"chip.ws\"",
        "module @chip {",
        "%go = in exec",
        "%n = Var {",
        "= chip { closed } !layout_after(",
        "%tick = in exec",
        "in exec(%tick)",
        "in *int(%n.VarRef)",
        "Exec_Var_Increment(Exec: ",
        "Exec_Var_Set(Exec: %go, VarRef: %n.VarRef, Value: 5)",
        "out %count int = %n.Value",
        "loc(1:1",
    ] {
        assert!(text.contains(shape), "missing `{shape}` in\n{text}");
    }
}

/// The order emit visits runtime `@label`s reaches the save, so it follows
/// the host ids, not the order the label map happened to be built in.
#[test]
fn the_label_map_order_does_not_reach_the_save() {
    let mut src = String::from("in go: exec\n");
    for i in 0..24 {
        src += &format!("@label(v{i}) var v{i}: int = {i}\n");
    }
    src += "on go {\n";
    for i in 0..24 {
        src += &format!("  v{i} = v{i} + 1\n");
    }
    src += "}\n";
    let program = wirescript::compile_to_world(
        common::input(&src, "labels.ws"),
        EmitOptions::default(),
    )
    .expect("compiles")
    .program;
    assert_eq!(program.module.dynamic_labels.len(), 24);
    let mut rebuilt = program.clone();
    let mut entries: Vec<_> = program.module.dynamic_labels.iter().map(|(k, v)| (*k, *v)).collect();
    entries.sort_by_key(|(k, _)| std::cmp::Reverse(*k));
    rebuilt.module.dynamic_labels = entries.into_iter().collect();
    let order = |p: &wirescript::Program| p.module.dynamic_labels.keys().copied().collect::<Vec<_>>();
    assert_ne!(order(&program), order(&rebuilt), "the rebuilt map should iterate in another order");
    let save = |p: &wirescript::Program| wirescript::assemble(p, &EmitOptions::default()).unwrap().world.to_brz_vec().unwrap();
    assert!(save(&program) == save(&rebuilt), "the label map's iteration order changed the save");
}
