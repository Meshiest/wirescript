//! End-to-end test for module-level `@invisible`: a top-of-file `@invisible`
//! (same placement rule as `@flat`/`@fold`/`@layout`) must make the emitted
//! top-level microchip shell brick hidden and non-colliding, and the emit
//! must carry no `Component_TextDisplay` label components anywhere.

use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, FoldMode, compile_to_world};

fn is_text_display(c: &Box<dyn brdb::BrdbComponent>) -> bool {
    c.component_type()
        .map(|t| t.to_string() == "Component_TextDisplay")
        .unwrap_or(false)
}

#[test]
fn module_invisible_hides_shell_and_labels() {
    let r = compile_to_world(
        CompileInput {
            source: "@invisible\n\nin go: exec\non go { }\n",
            file: "invisible_module.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        EmitOptions::default(),
    )
    .expect("should compile");

    // The top-level chip shell is the first brick `add_microchip` pushes
    // onto the main grid (see `text_labels.rs::labels_attach_to_chip_var_and_io_bricks`,
    // which asserts the same brick carries the label `@invisible` must suppress).
    let shell = &r.world.bricks[0];
    assert!(!shell.visible, "shell brick must be hidden");
    assert!(
        !shell.collision.player
            && !shell.collision.weapon
            && !shell.collision.interact
            && !shell.collision.tool
            && !shell.collision.physics,
        "shell brick must not collide: {:?}",
        shell.collision
    );

    assert!(
        !r.world
            .grids
            .iter()
            .flat_map(|(_e, bricks)| bricks)
            .chain(r.world.bricks.iter())
            .any(|b| b.components.iter().any(is_text_display)),
        "@invisible must emit no text labels"
    );
}
