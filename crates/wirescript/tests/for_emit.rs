//! A `for` must emit its loop shell brick, its loop shell component, and the
//! loop bounds as that component's data. Any one of the three wrong produces a
//! save that loads and quietly does the wrong thing.

use wirescript::compile::{CompileError, CompileInput, FoldMode, compile, compile_to_world};
use wirescript::emit::EmitOptions;

mod common;

fn world(src: &str) -> brdb::World {
    compile_to_world(
        CompileInput { source: src, file: "for_emit.ws", module_name: None, fold_mode: FoldMode::Auto },
        EmitOptions::default(),
    )
    .expect("should compile to a world")
    .world
}

fn brz(src: &str) -> Vec<u8> {
    compile(CompileInput {
        source: src,
        file: "for_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz")
    .brz
}

/// Every plane brick asset paired with the component class sitting on it:
/// both the shells and the entry gates, since a brick placed without its own
/// component is a brick the game refuses or a gate that does nothing.
fn shells(world: &brdb::World) -> Vec<(String, String)> {
    common::brick_asset_components(world)
        .into_iter()
        .filter(|(asset, _)| asset.contains("Microchip") || asset.contains("CallableEntry"))
        .collect()
}

#[test]
fn a_range_loop_emits_the_count_for_brick() {
    let w = world("var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i } }");
    let assets = common::brick_assets(&w);
    assert!(
        assets.iter().any(|a| a == "B_1x1_Microchip_CountFor"),
        "expected a CountFor shell brick, got: {assets:?}"
    );
    assert!(
        assets.iter().any(|a| a == "B_1x1_Gate_CallableEntry_CountFor"),
        "expected the entry gate brick, got: {assets:?}"
    );

    // An entry gate is only allowed on a callable board, so the component has
    // to move with its brick or the game refuses to place it.
    let pairs = shells(&w);
    assert!(
        pairs.iter().any(|(a, c)| a == "B_1x1_Microchip_CountFor"
            && c == "Component_Internal_Microchip_CountFor"),
        "expected the CountFor brick to carry the CountFor component, got: {pairs:?}"
    );
    assert!(
        pairs.iter().any(|(a, c)| a == "B_1x1_Gate_CallableEntry_CountFor"
            && c == "BrickComponentType_CallableEntry_CountFor"),
        "expected the entry brick to carry the entry component, got: {pairs:?}"
    );
}

#[test]
fn an_array_loop_emits_the_for_each_array_brick() {
    let w = world("var arr: int[]\nvar t: int = 0\nin go: exec\non go { arr.push(3)\nfor v in arr { t += v } }");
    let assets = common::brick_assets(&w);
    assert!(
        assets.iter().any(|a| a == "B_1x1_Microchip_ForEachArray"),
        "got: {assets:?}"
    );
    assert!(
        assets
            .iter()
            .any(|a| a == "B_1x1_Gate_CallableEntry_ForEachArray"),
        "expected the array entry gate brick, got: {assets:?}"
    );
    let pairs = shells(&w);
    assert!(
        pairs.iter().any(|(a, c)| a == "B_1x1_Microchip_ForEachArray"
            && c == "Component_Internal_Microchip_ForEachArray"),
        "got: {pairs:?}"
    );
    assert!(
        pairs
            .iter()
            .any(|(a, c)| a == "B_1x1_Gate_CallableEntry_ForEachArray"
                && c == "BrickComponentType_CallableEntry_ForEachArray"),
        "expected the entry brick to carry the entry component, got: {pairs:?}"
    );
}

#[test]
fn a_map_loop_emits_the_for_each_map_brick() {
    let w = world("var m: Map<int, int>\nvar t: int = 0\nin go: exec\non go { m.set(1, 3)\nfor k, v in m { t += v } }");
    let assets = common::brick_assets(&w);
    assert!(
        assets.iter().any(|a| a == "B_1x1_Microchip_ForEachMap"),
        "got: {assets:?}"
    );
    assert!(
        assets
            .iter()
            .any(|a| a == "B_1x1_Gate_CallableEntry_ForEachMap"),
        "expected the map entry gate brick, got: {assets:?}"
    );
    let pairs = shells(&w);
    assert!(
        pairs
            .iter()
            .any(|(a, c)| a == "B_1x1_Microchip_ForEachMap"
                && c == "Component_Internal_Microchip_ForEachMap"),
        "got: {pairs:?}"
    );
    assert!(
        pairs
            .iter()
            .any(|(a, c)| a == "B_1x1_Gate_CallableEntry_ForEachMap"
                && c == "BrickComponentType_CallableEntry_ForEachMap"),
        "expected the entry brick to carry the entry component, got: {pairs:?}"
    );
}

/// Making the shell node-driven must leave the ordinary chip path alone: a
/// plain microchip keeps its own brick and component.
#[test]
fn a_plain_chip_still_emits_a_plain_microchip() {
    let w = world(
        "var t: int = 0\nchip Bump(a: int) -> (r: int) { return a + 1 }\nin go: exec\non go { t = Bump(t) }",
    );
    let pairs = shells(&w);
    assert!(
        pairs
            .iter()
            .any(|(a, c)| a == "B_1x1_Microchip" && c == "Component_Internal_Microchip"),
        "expected a plain microchip shell, got: {pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(a, _)| a.contains("CountFor")),
        "a plain chip must not emit a loop shell, got: {pairs:?}"
    );
}

/// A single-consumer constant pin is folded into the node's properties and its
/// wire deleted before emit, so the properties are the only place the bounds
/// survive. Reading just the wires leaves all three at their defaults.
#[test]
fn a_range_loop_emits_its_bounds_as_shell_data() {
    let b = brz("var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i } }");
    assert_eq!(
        common::count_for_shell_data(&b),
        vec![(10, 0, 1)],
        "0..10 must emit Count 10, FirstIndex 0, Step 1"
    );
}

/// The stride divide is folded at the emit boundary, so ten values taken three
/// apart must pin `Count` at the ceiling, four, and not the truncated three.
#[test]
fn a_stepped_range_emits_the_ceiling_count() {
    let b = brz("var t: int = 0\nin go: exec\non go { for i in (0..10).step(3) { t += i } }");
    assert_eq!(
        common::count_for_shell_data(&b),
        vec![(4, 0, 3)],
        "(0..10).step(3) must emit Count 4, FirstIndex 0, Step 3"
    );
}

/// A reversed range walks down from the last value, so both the negative
/// `Step` and the shifted `FirstIndex` have to reach the component.
#[test]
fn a_reversed_range_emits_a_negative_step() {
    let b = brz("var t: int = 0\nin go: exec\non go { for i in (0..5).rev() { t += i } }");
    assert_eq!(common::count_for_shell_data(&b), vec![(5, 4, -1)]);
}

/// A runtime bound arrives over a wire and leaves its pin at the component
/// default, so each pin has to resolve independently of the others.
#[test]
fn a_partly_constant_range_inlines_only_the_constant_pins() {
    let b = brz("var n: int = 7\nvar t: int = 0\nin go: exec\non go { for i in 3..n { t += i } }");
    let data = common::count_for_shell_data(&b);
    assert_eq!(data.len(), 1, "expected one counted shell, got {data:?}");
    let (_count, first, step) = data[0];
    assert_eq!(
        (first, step),
        (3, 1),
        "the constant FirstIndex and Step must still be inlined, got {data:?}"
    );
}

/// A nested loop's shell sits in its parent's plane, a separate module with
/// its own inline pass, so its bounds have to arrive there too.
#[test]
fn a_nested_loop_emits_bounds_on_both_shells() {
    let b = brz("var t: int = 0\nin go: exec\non go { for i in 0..3 { for j in 0..4 { t += j } } }");
    let mut data = common::count_for_shell_data(&b);
    data.sort();
    assert_eq!(
        data,
        vec![(3, 0, 1), (4, 0, 1)],
        "both the outer and the inner shell must carry their own bounds"
    );
}

/// A mod inlines its body per call site, so an `await` written in a mod with
/// no `for` of its own can still land inside a loop plane once the mod is
/// called from a loop body. This must fail to compile (WS079), not merely
/// warn and emit a `for` whose plane holds an armed-flag var with no
/// continuation.
#[test]
fn an_await_reached_through_an_inlined_mod_call_fails_to_compile() {
    let src = "in done: exec\nvar t: int = 0\nmod waiter() { await done\n t = t + 1 }\n\
               in go: exec\non go { for i in 0..3 { waiter() } }";
    let result = compile(CompileInput {
        source: src,
        file: "for_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    });
    let Err(CompileError::HasErrors(diags)) = result else {
        panic!("an await reached through an inlined mod call inside a loop must fail with HasErrors, not compile");
    };
    assert!(
        diags.iter().any(|d| d.code == "WS079"),
        "expected WS079 among the compile errors, got {diags:?}"
    );
}
