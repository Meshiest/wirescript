use std::collections::HashMap;

use indexmap::IndexMap;

use crate::brdb::{
    errors::BrdbError,
    wrapper::{Brick, Guid, Owner, UnsavedFs, UnsavedWorld, WireConnection, WirePort, WorldMeta},
};

#[derive(Default)]
pub struct World {
    pub meta: WorldMeta,
    pub owners: IndexMap<Guid, Owner>,
    /// Bricks on the main grid
    pub bricks: Vec<Brick>,
    pub grids: HashMap<usize, Vec<Brick>>,
    pub wires: Vec<WireConnection>,
    // TODO: minigame, environment, entities
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_unsaved(&self) -> Result<UnsavedFs, BrdbError> {
        let mut unsaved_fs = UnsavedFs {
            meta: self.meta.clone(),
            worlds: Default::default(),
        };

        // Only one world exists right now...
        {
            let mut world = UnsavedWorld::default();
            for o in self.owners.values() {
                world.owners.add(o);
            }

            // Main grid bricks are on grid 1
            world.add_bricks_to_grid(1, &self.bricks);

            // Add all grids (by grid id)
            for (grid_id, bricks) in &self.grids {
                // TODO: error for brick grid id 1/0
                world.add_bricks_to_grid(*grid_id, bricks);
            }

            for (i, wire) in self.wires.iter().enumerate() {
                world
                    .add_wire(wire)
                    .map_err(|e| e.wrap(format!("wire {i}: {wire}")))?;
            }

            // Add the world
            unsaved_fs.worlds.insert(0, world);
        }

        Ok(unsaved_fs)
    }

    /// Add a single brick to the world
    pub fn add_brick(&mut self, brick: Brick) {
        self.bricks.push(brick);
    }
    /// Add multiple bricks to the world
    pub fn add_bricks(&mut self, bricks: impl IntoIterator<Item = Brick>) {
        self.bricks.extend(bricks);
    }

    /// Add a single wire connection to the world
    pub fn add_wire(&mut self, conn: WireConnection) {
        self.wires.push(conn);
    }
    /// Add multiple wire connections to the world
    pub fn add_wires(&mut self, wires: impl IntoIterator<Item = WireConnection>) {
        self.wires.extend(wires);
    }
    /// Add a wire connection from one port to another
    pub fn add_wire_connection(&mut self, source: WirePort, target: WirePort) {
        self.wires.push(WireConnection { source, target });
    }
}
