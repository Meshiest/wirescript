use wirescript::{compile, CompileInput, FoldMode};

/// Constant constructor folding must survive full emit: vector/rotator/quat/
/// color vars carry their folded InitialValue through brz serialization
/// (WireGraphVariant members Vector/Rotator/Quat/LinearColor).
#[test]
fn folded_constructor_vars_emit_to_brz() {
    let src = "\
var v = Vec(1.0, 2.0, 3.0)
var rot = Rotation(0.0, 90.0, 0.0)
var tint = Color(1.0, 0.5, 0.0)
var q: quat
out o = v
in start: exec
on start {
  v = Vec(4.0, 5.0, 6.0)
  tint = Color(0.0, 1.0, 0.0, 0.5)
}";
    let r = compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto });
    assert!(r.is_ok(), "constant-folded vars should emit: {:?}", r.err());
}

#[test]
fn vector_array_initializer_emits_to_brz() {
    let src = "\
array pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]
in start: exec
on start { pts.push(Vec(7.0, 8.0, 9.0)) }";
    let r = compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto });
    assert!(r.is_ok(), "vector array init should emit: {:?}", r.err());
}

#[test]
fn rotator_and_color_arrays_emit_to_brz() {
    // rotator[]/quat[]/color[] arrays back onto the game's typed array
    // variants (WireGraphRotatorArray / QuatArray / LinearColorArray).
    let src = "\
array rots: rotator[] = [Rotation(0.0, 90.0, 0.0)]
array tints: color[] = [Color(1.0, 0.0, 0.0), Color(0.0, 0.0, 1.0, 0.5)]
array quats: quat[]
out n = rots.length()";
    let r = compile(CompileInput { source: src, file: "test", module_name: None, fold_mode: FoldMode::Auto });
    assert!(r.is_ok(), "rotator/color/quat arrays should emit: {:?}", r.err());
}
