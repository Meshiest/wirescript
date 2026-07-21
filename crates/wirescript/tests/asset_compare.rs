//! An asset reference compared against an entity value
//! (`weapon == $BRItemBase/Weapon_Pickaxe`) must register the asset in the
//! world's external-asset table and compare against it as a real `weak_object`
//! reference — not silently drop it to a null object.

use wirescript::emit::EmitOptions;
use wirescript::{compile_to_world, CompileInput, FoldMode};

#[test]
fn asset_compare_registers_asset() {
    let src = "on CharacterDamaged(c, dmg, atk, weapon, wname) {\n\
               if weapon == $BRItemBase/Weapon_Pickaxe {\n\
               }\n\
               }";
    let r = match compile_to_world(
        CompileInput { source: src, file: "t", module_name: Some("m"), fold_mode: FoldMode::Auto },
        EmitOptions::default(),
    ) {
        Ok(r) => r,
        Err(e) => panic!("should compile: {e}"),
    };
    let refs = &r.world.global_data.external_asset_references;
    assert!(
        refs.iter()
            .any(|(t, n)| t.as_str() == "BRItemBase" && n.as_str() == "Weapon_Pickaxe"),
        "the pickaxe asset should be registered as an external reference, got {refs:?}"
    );

    // The asset must reach the comparison via an ItemReference gate (an item
    // asset can't be inlined into the Equals gate), alongside the Compare gate.
    let mut assets: Vec<String> = Vec::new();
    for (_e, bricks) in &r.world.grids {
        for b in bricks {
            assets.push(format!("{:?}", b.asset));
        }
    }
    assert!(
        assets.iter().any(|a| a.contains("ItemReference")),
        "expected an ItemReference gate to source the asset, got {assets:?}"
    );
    assert!(
        assets.iter().any(|a| a.contains("CompareEqual")),
        "expected a CompareEqual gate, got {assets:?}"
    );
}
