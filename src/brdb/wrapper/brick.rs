use std::sync::Arc;

use crate::brdb::{
    schema::as_brdb::{AsBrdbValue, LazyBrdbVec},
    wrapper::BitFlags,
};

#[derive(Clone, Debug)]
pub struct Brick {
    pub asset: BrickType,
    pub owner_index: Option<usize>,
    pub position: Position,
    pub collision: Collision,
    pub visible: bool,
    pub color: Color,
    pub material: u8,
}

impl Default for Brick {
    fn default() -> Self {
        Self {
            asset: BrickType::Procedural {
                kind: Arc::new(String::from("PB_DefaultBrick")),
                size: BrickSize { x: 5, y: 5, z: 3 },
            },
            owner_index: None,
            position: Position { x: 0, y: 0, z: 0 },
            collision: Default::default(),
            visible: true,
            color: Default::default(),
            material: 0,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Collision {
    pub player: bool,
    pub weapon: bool,
    pub interact: bool,
    pub tool: bool,
}

impl Default for Collision {
    fn default() -> Self {
        Self {
            player: true,
            weapon: true,
            interact: true,
            tool: true,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub material_intensity: u8,
}

impl AsBrdbValue for Color {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "R" => Ok(&self.r),
            "G" => Ok(&self.g),
            "B" => Ok(&self.b),
            "A" => Ok(&self.material_intensity),
            _ => unreachable!(),
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self {
            r: 255,
            g: 255,
            b: 255,
            material_intensity: 5,
        }
    }
}

#[derive(Clone, Debug)]

pub enum BrickType {
    Basic(Arc<String>),
    Procedural { kind: Arc<String>, size: BrickSize },
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

pub const CHUNK_SIZE: i32 = 2048;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkIndex {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}
impl AsBrdbValue for ChunkIndex {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            _ => unreachable!(),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BrickSize {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

impl AsBrdbValue for BrickSize {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            _ => unreachable!(),
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct RelativePosition {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

impl AsBrdbValue for RelativePosition {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            _ => unreachable!(),
        }
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, Default)]
pub enum Direction {
    XPositive,
    XNegative,
    YPositive,
    YNegative,
    #[default]
    ZPositive,
    ZNegative,
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, Default)]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

pub fn orientation_to_byte(dir: Direction, rot: Rotation) -> u8 {
    (dir as u8) << 2 | rot as u8
}

pub fn byte_to_orientation(orientation: u8) -> (Direction, Rotation) {
    let dir = match (orientation >> 2) % 6 {
        0 => Direction::XPositive,
        1 => Direction::XNegative,
        2 => Direction::YPositive,
        3 => Direction::YNegative,
        4 => Direction::ZPositive,
        _ => Direction::ZNegative,
    };
    let rot = match orientation & 3 {
        0 => Rotation::Deg0,
        1 => Rotation::Deg90,
        2 => Rotation::Deg180,
        _ => Rotation::Deg270,
    };
    (dir, rot)
}

impl Position {
    pub fn to_relative(self) -> (ChunkIndex, RelativePosition) {
        let x = self.x - CHUNK_SIZE / 2;
        let y = self.y - CHUNK_SIZE / 2;
        let z = self.z - CHUNK_SIZE / 2;
        (
            ChunkIndex {
                x: (x / CHUNK_SIZE) as i16,
                y: (y / CHUNK_SIZE) as i16,
                z: (z / CHUNK_SIZE) as i16,
            },
            RelativePosition {
                x: (x % CHUNK_SIZE) as i16,
                y: (y % CHUNK_SIZE) as i16,
                z: (z % CHUNK_SIZE) as i16,
            },
        )
    }

    pub fn from_relative(chunk: ChunkIndex, pos: RelativePosition) -> Self {
        Position {
            x: chunk.x as i32 * CHUNK_SIZE + (CHUNK_SIZE / 2) + pos.x as i32,
            y: chunk.y as i32 * CHUNK_SIZE + (CHUNK_SIZE / 2) + pos.y as i32,
            z: chunk.z as i32 * CHUNK_SIZE + (CHUNK_SIZE / 2) + pos.z as i32,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BrickSizeCounter {
    pub asset_index: u32,
    pub num_sizes: u32,
}

impl AsBrdbValue for BrickSizeCounter {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "AssetIndex" => Ok(&self.asset_index),
            "NumSizes" => Ok(&self.num_sizes),
            _ => unreachable!(),
        }
    }
}

pub struct BrickChunkSoA {
    pub procedural_brick_starting_index: u32,
    pub brick_size_counters: Vec<BrickSizeCounter>,
    pub brick_sizes: Vec<BrickSize>,
    pub brick_type_indices: Vec<u32>,
    pub owner_indices: Vec<u32>,
    pub relative_positions: Vec<RelativePosition>,
    pub orientations: Vec<u8>,
    pub collision_flags_player: BitFlags,
    pub collision_flags_weapon: BitFlags,
    pub collision_flags_interaction: BitFlags,
    pub collision_flags_tool: BitFlags,
    pub visibility_flags: BitFlags,
    pub material_indices: Vec<u8>,
    pub colors_and_alphas: Vec<Color>,
}

impl AsBrdbValue for BrickChunkSoA {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "ProceduralBrickStartingIndex" => Ok(&self.procedural_brick_starting_index),
            "CollisionFlags_Player" => Ok(&self.collision_flags_player),
            "CollisionFlags_Weapon" => Ok(&self.collision_flags_weapon),
            "CollisionFlags_Interaction" => Ok(&self.collision_flags_interaction),
            "CollisionFlags_Tool" => Ok(&self.collision_flags_tool),
            "VisibilityFlags" => Ok(&self.visibility_flags),
            _ => unreachable!(),
        }
    }

    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<Vec<&dyn AsBrdbValue>, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "BrickSizeCounters" => Ok(self.brick_size_counters.lazy_vec_cast()),
            "BrickSizes" => Ok(self.brick_sizes.lazy_vec_cast()),
            "BrickTypeIndices" => Ok(self.brick_type_indices.lazy_vec_cast()),
            "OwnerIndices" => Ok(self.owner_indices.lazy_vec_cast()),
            "RelativePositions" => Ok(self.relative_positions.lazy_vec_cast()),
            "Orientations" => Ok(self.orientations.lazy_vec_cast()),
            "MaterialIndices" => Ok(self.material_indices.lazy_vec_cast()),
            "ColorsAndAlphas" => Ok(self.colors_and_alphas.lazy_vec_cast()),
            _ => unreachable!(),
        }
    }
}
