use std::collections::HashMap;

use itertools::Itertools;

use crate::{
    errors::{BrdbError, BrdbWorldError},
    pending::BrdbPendingFs,
    schema::{BrdbSchema, BrdbSchemaGlobalData},
    wrapper::{
        Brick, BrickChunkIndexSoA, BrickChunkSoA, ChunkIndex, ComponentChunkSoA, Entity,
        EntityChunkIndexSoA, EntityChunkSoA, LocalWirePortSource, OwnerTableSoA,
        RemoteWirePortSource, WireChunkSoA, WireConnection, WirePortTarget, WorldMeta, schemas,
    },
};

/// All of the dynamic data needed to serialize a world
pub struct UnsavedFs {
    /// Meta/
    pub meta: WorldMeta,
    /// World/
    pub worlds: HashMap<usize, UnsavedWorld>,
}

impl UnsavedFs {
    pub fn to_pending(self) -> Result<BrdbPendingFs, BrdbError> {
        BrdbPendingFs::from_unsaved(self)
    }
}

pub struct UnsavedWorld {
    /// World/N/GlobalData.mps
    pub global_data: BrdbSchemaGlobalData,
    /// World/N/Owners.mps
    pub owners: OwnerTableSoA,
    /// World/N/Bricks/Grids/ComponentsShared.mps
    pub component_schema: BrdbSchema,
    /// World/N/Bricks/Grids/[key.0]/
    pub grids: HashMap<usize, UnsavedGrid>,
    /// World/N/Bricks/Entities/Chunks/[key].mps
    pub entity_chunks: HashMap<ChunkIndex, EntityChunkSoA>,
    /// World/N/Bricks/Entities/ChunksShared.schema
    pub entity_schema: BrdbSchema,
    /// World/N/Bricks/Entities/ChunkIndex.mps
    pub entity_chunk_index: EntityChunkIndexSoA,

    /// World/N/Minigame.bp
    pub minigame: Option<()>, // TODO: minigames serialization
    /// World/N/Environment.bp
    pub environment: Option<()>, // TODO: environment serialization

    /// Internal map of brick id to (grid_id, chunk_index, brick_index_in_chunk)
    /// This is used to connect wires
    /// and is not saved to the world file.
    brick_id_map: HashMap<usize, (usize, ChunkIndex, usize)>,
    // Maps entity internal id to persistent index
    entity_index_map: HashMap<usize, u32>,
}

impl Default for UnsavedWorld {
    fn default() -> Self {
        Self {
            global_data: Default::default(),
            owners: Default::default(),
            component_schema: schemas::bricks_components_schema_min(),
            grids: Default::default(),
            entity_chunks: Default::default(),
            entity_schema: schemas::entities_chunks_schema(),
            entity_chunk_index: Default::default(),
            minigame: Default::default(),
            environment: Default::default(),
            brick_id_map: Default::default(),
            entity_index_map: Default::default(),
        }
    }
}

impl UnsavedWorld {
    fn add_brick_meta(&mut self, brick: &Brick) {
        // Adding the brick's data to the global data
        self.global_data.add_brick_meta(brick);

        // Iterate the components of the brick and register
        // their respective struct metadata with the component schema
        for component in &brick.components {
            // Skip components without a type
            let Some((ty_name, _)) = component.get_schema_struct() else {
                continue;
            };

            // If the component type is already registered, skip it
            if self.global_data.has_component_type(ty_name.as_ref()) {
                continue;
            }
            self.global_data.add_component_meta(component.as_ref());

            let Some((enums, structs)) = component.get_schema() else {
                continue;
            };
            self.component_schema.add_meta(enums, structs);
        }
    }

    fn add_entity_meta(&mut self, entity: &Entity) {
        let Some((ty_name, _)) = entity.data.get_schema_struct() else {
            return;
        };
        self.global_data.add_entity_type(&ty_name);
        let Some((enums, structs)) = entity.data.get_schema() else {
            return;
        };
        self.entity_schema.add_meta(enums, structs);
    }

    pub(super) fn add_bricks_to_grid(&mut self, grid_id: usize, bricks: &[Brick]) {
        let mut grid = UnsavedGrid::default();

        // Bricks are sorted by brick type, size, and position
        for b in bricks.iter().sorted_by(|a, b| a.cmp(b)) {
            self.add_brick_meta(b);

            // Update the owner table
            let owner_id = b.owner_index.unwrap_or(0);
            self.owners.inc_bricks(owner_id);
            self.owners
                .inc_components(owner_id, b.components.len() as u32);

            // Add the brick to the grid
            let (chunk_index, brick_index) = grid.add_brick(&self.global_data, b);
            // Track the brick for wire connections
            if let Some(id) = b.id {
                self.brick_id_map
                    .insert(id, (grid_id, chunk_index, brick_index));
            }
        }

        // Add the grid to the world
        self.grids.insert(grid_id, grid);
    }

    pub(super) fn add_entity(&mut self, entity: &Entity) -> usize {
        // Add the entity metadata to the global data
        self.add_entity_meta(entity);

        // Update the owner table
        let owner_id = entity.owner_index.unwrap_or(0);
        self.owners.inc_entities(owner_id as usize);

        // Increment the entity persistent index
        let entity_index = self.entity_chunk_index.next_persistent_index;
        self.entity_chunk_index.next_persistent_index += 1;

        // There is only one entity chunk right now...
        let chunk_index = ChunkIndex::ZERO;
        // Create a new entity chunk if it doesn't exist
        if self.entity_chunk_index.chunk_3d_indices.is_empty() {
            self.entity_chunk_index.chunk_3d_indices.push(chunk_index);
        }
        if self.entity_chunk_index.num_entities.is_empty() {
            self.entity_chunk_index.num_entities.push(0);
        }
        self.entity_chunk_index.num_entities[0] += 1;

        self.entity_chunks
            .entry(chunk_index)
            .or_insert_with(EntityChunkSoA::default)
            .add_entity(&self.global_data, entity, entity_index);

        // Map the internal entity id to its persistent index
        if let Some(id) = entity.id {
            self.entity_index_map.insert(id, entity_index);
        }

        entity_index as usize
    }

    pub(super) fn add_wire(&mut self, wire: &WireConnection) -> Result<(), BrdbError> {
        // Resolve source wire metadata
        let (s_grid, s_chunk, s_brick) = self
            .brick_id_map
            .get(&wire.source.brick_id)
            .ok_or_else(|| BrdbWorldError::UnknownBrickId(wire.source.brick_id))?;
        let s_comp_ty = self
            .global_data
            .get_component_type_index(&wire.source.component_type)
            .ok_or_else(|| {
                BrdbWorldError::UnknownComponent(wire.source.component_type.to_string())
            })?;
        let s_port_index = self
            .global_data
            .get_port_index(&wire.source.port_name)
            .ok_or_else(|| BrdbWorldError::UnknownPort(wire.source.port_name.to_string()))?;

        // Resolve target wire metadata
        let (t_grid, t_chunk, t_brick) = self
            .brick_id_map
            .get(&wire.target.brick_id)
            .ok_or_else(|| BrdbWorldError::UnknownBrickId(wire.target.brick_id))?;
        let t_comp_ty = self
            .global_data
            .get_component_type_index(&wire.target.component_type)
            .ok_or_else(|| {
                BrdbWorldError::UnknownComponent(wire.target.component_type.to_string())
            })?;
        let t_port_index = self
            .global_data
            .get_port_index(&wire.target.port_name)
            .ok_or_else(|| BrdbWorldError::UnknownPort(wire.target.port_name.to_string()))?;

        // Create the target port
        let target = WirePortTarget {
            brick_index_in_chunk: *t_brick as u32,
            component_type_index: t_comp_ty,
            port_index: t_port_index,
        };

        // Wires are inserted in the target grid
        let grid = self
            .grids
            .get_mut(t_grid)
            .ok_or_else(|| BrdbWorldError::UnknownGridId(*t_grid))?;

        // Increment the wire count for the target chunk
        let chunk_id = grid.get_chunk_index(*t_chunk);
        grid.chunk_index.num_wires[chunk_id] += 1;

        // If the target and source are in the same grid and chunk,
        // we can use a local wire source.
        if t_grid == s_grid && t_chunk == s_chunk {
            let source = LocalWirePortSource {
                brick_index_in_chunk: *s_brick as u32,
                component_type_index: s_comp_ty,
                port_index: s_port_index,
            };
            grid.add_local_wire(*t_chunk, source, target);
        } else {
            // Otherwise, we need to use a remote wire source.
            let source = RemoteWirePortSource {
                grid_persistent_index: *s_grid as u32,
                chunk_index: *s_chunk,
                brick_index_in_chunk: *s_brick as u32,
                component_type_index: s_comp_ty,
                port_index: s_port_index,
            };
            grid.add_remote_wire(*t_chunk, source, target);
        }

        Ok(())
    }
}

#[derive(Default)]
pub struct UnsavedGrid {
    /// World/N/Bricks/Grids/I/ChunkIndex.mps
    pub chunk_index: BrickChunkIndexSoA,
    /// World/N/Bricks/Grids/I/Chunks/[key].mps
    pub bricks: HashMap<ChunkIndex, BrickChunkSoA>,
    /// World/N/Bricks/Grids/I/Components/[key].mps
    pub components: HashMap<ChunkIndex, ComponentChunkSoA>,
    /// World/N/Bricks/Grids/I/Wires/[key].mps
    pub wires: HashMap<ChunkIndex, WireChunkSoA>,

    /// Map of 3d chunk index to serial index in the `chunk_index` array
    /// Used to quickly find the index of a chunk in the `chunk_index` array
    chunk_index_map: HashMap<ChunkIndex, usize>,
}

impl UnsavedGrid {
    /// Appends a new chunk to the chunk_index SoA, returning the index of the chunk
    fn get_chunk_index(&mut self, chunk_index: ChunkIndex) -> usize {
        // Add the chunk to the index if it doesn't exist
        if let Some(index) = self.chunk_index_map.get(&chunk_index) {
            *index
        } else {
            self.chunk_index.chunk_3d_indices.push(chunk_index);
            self.chunk_index.num_bricks.push(0);
            self.chunk_index.num_components.push(0);
            self.chunk_index.num_wires.push(0);
            let index = self.chunk_index_map.len();
            self.chunk_index_map.insert(chunk_index, index);
            index
        }
    }

    /// Add a brick to the grid, returning the chunk index and the brick index
    fn add_brick(
        &mut self,
        global_data: &BrdbSchemaGlobalData,
        brick: &Brick,
    ) -> (ChunkIndex, usize) {
        let chunk_index = brick.position.to_relative().0;
        // Lookup chunk by chunk index (or create a default one if it doesn't exist)
        self.bricks
            .entry(chunk_index)
            .or_insert_with(BrickChunkSoA::default)
            .add_brick(global_data, brick); // Add the brick to the chunk
        // Get the chunk_index SoA index for that chunk
        let i = self.get_chunk_index(chunk_index);
        // Get the brick index
        let brick_index = self.chunk_index.num_bricks[i];
        // Increment the counts for the chunk index
        self.chunk_index.num_bricks[i] += 1;
        self.chunk_index.num_components[i] += brick.components.len() as u32;

        // Write the components to the respective component chunk
        if !brick.components.is_empty() {
            let chunk = self
                .components
                .entry(chunk_index)
                .or_insert_with(ComponentChunkSoA::default);
            for c in &brick.components {
                chunk.add_component(global_data, brick_index, c.as_ref());
            }
        }

        (chunk_index, brick_index as usize)
    }

    fn add_local_wire(
        &mut self,
        chunk: ChunkIndex,
        source: LocalWirePortSource,
        target: WirePortTarget,
    ) {
        self.wires
            .entry(chunk)
            .or_insert_with(WireChunkSoA::default)
            .add_local_wire(source, target);
    }

    fn add_remote_wire(
        &mut self,
        chunk: ChunkIndex,
        source: RemoteWirePortSource,
        target: WirePortTarget,
    ) {
        self.wires
            .entry(chunk)
            .or_insert_with(WireChunkSoA::default)
            .add_remote_wire(source, target);
    }
}
