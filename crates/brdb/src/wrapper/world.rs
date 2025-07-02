use indexmap::IndexMap;

use crate::{
    errors::BrdbError,
    wrapper::{
        Brick, Entity, Guid, Owner, Position, UnsavedFs, UnsavedWorld, WireConnection, WirePort,
        WorldMeta,
    },
};

#[derive(Default)]
pub struct World {
    pub meta: WorldMeta,
    pub owners: IndexMap<Guid, Owner>,
    /// Bricks on the main grid
    pub bricks: Vec<Brick>,
    /// Non-main grids require an entity to be created for them
    pub grids: Vec<(Entity, Vec<Brick>)>,
    pub wires: Vec<WireConnection>,
    pub entities: Vec<Entity>,
    // TODO: minigame, environment
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

            // Add all dynamic grids
            for (entity, bricks) in &self.grids {
                let grid_id = world.add_entity(entity);
                world.add_bricks_to_grid(grid_id, bricks);
            }

            // Add all non-grid entities
            for entity in &self.entities {
                world.add_entity(entity);
            }

            // Add all wires
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
    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.push(entity);
    }
    pub fn add_brick_grid(&mut self, entity: Entity, bricks: impl IntoIterator<Item = Brick>) {
        self.grids.push((
            entity,
            bricks
                .into_iter()
                .map(|mut b| {
                    // Shift all bricks in non-main grids to the center of the chunk
                    // Otherwise the bricks will be offset by half the chunk size
                    b.position = b.position - Position::CHUNK_HALF;
                    b
                })
                .collect(),
        ));
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
