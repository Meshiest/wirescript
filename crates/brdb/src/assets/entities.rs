use std::sync::Arc;

use crate::{
    assets::LiteralComponent,
    wrapper::{BString, BrdbComponent},
};

pub const DYNAMIC_GRID: BString = BString::str("Entity_DynamicBrickGrid");
pub fn dynamic_grid_entity() -> Arc<Box<dyn BrdbComponent>> {
    Arc::new(Box::new(
        LiteralComponent::new(
            DYNAMIC_GRID,
            "BrickGridDynamicActor",
            "struct BrickGridDynamicActor {}",
            [],
            [],
        )
        .expect("Failed to create dynamic grid entity"),
    ))
}
