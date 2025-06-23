use std::{
    cmp::Ordering,
    fmt::{Debug, Display},
    sync::{Arc, LazyLock},
};

use crate::brdb::{
    schema::{
        BrdbSchemaGlobalData,
        as_brdb::{AsBrdbIter, AsBrdbValue, BrdbArrayIter},
    },
    wrapper::{BitFlags, BrdbComponent},
};

pub struct Brick {
    /// An internal ID for linking bricks in the database.
    pub id: Option<usize>,
    pub asset: BrickType,
    pub owner_index: Option<usize>,
    pub position: Position,
    pub rotation: Rotation,
    pub direction: Direction,
    pub collision: Collision,
    pub visible: bool,
    pub color: Color,
    pub material: Arc<String>,
    pub material_intensity: u8,
    pub components: Vec<Box<dyn BrdbComponent>>,
}

impl Brick {
    pub fn next_id() -> usize {
        static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub fn with_new_id(mut self) -> Self {
        self.id = Some(Self::next_id());
        self
    }

    pub fn with_id(mut self, id: usize) -> Self {
        self.id = Some(id);
        self
    }

    pub fn set_id(&mut self, id: usize) {
        self.id = Some(id);
    }

    pub fn add_component(&mut self, component: impl BrdbComponent + 'static) {
        self.components.push(Box::new(component));
    }

    pub fn cmp(&self, other: &Self) -> Ordering {
        match self.asset.cmp(&other.asset) {
            Ordering::Equal => self.position.cmp(&other.position),
            ord => ord,
        }
    }

    pub fn material_glass() -> Arc<String> {
        static MAT_PLASTIC: LazyLock<Arc<String>> =
            LazyLock::new(|| Arc::new(String::from("BMC_Plastic")));
        MAT_PLASTIC.clone()
    }
    pub fn material_plastic() -> Arc<String> {
        static MAT_GLASS: LazyLock<Arc<String>> =
            LazyLock::new(|| Arc::new(String::from("BMC_Glass")));
        MAT_GLASS.clone()
    }
    pub fn material_translucent_plastic() -> Arc<String> {
        static MAT_TRANSLUCENT_PLASTIC: LazyLock<Arc<String>> =
            LazyLock::new(|| Arc::new(String::from("BMC_TranslucentPlastic")));
        MAT_TRANSLUCENT_PLASTIC.clone()
    }
    pub fn material_glow() -> Arc<String> {
        static MAT_GLOW: LazyLock<Arc<String>> =
            LazyLock::new(|| Arc::new(String::from("BMC_Glow")));
        MAT_GLOW.clone()
    }
    pub fn material_metallic() -> Arc<String> {
        static MAT_METALLIC: LazyLock<Arc<String>> =
            LazyLock::new(|| Arc::new(String::from("BMC_Metallic")));
        MAT_METALLIC.clone()
    }
    pub fn material_hologram() -> Arc<String> {
        static MAT_HOLOGRAM: LazyLock<Arc<String>> =
            LazyLock::new(|| Arc::new(String::from("BMC_Hologram")));
        MAT_HOLOGRAM.clone()
    }
}

impl Default for Brick {
    fn default() -> Self {
        Self {
            id: None,
            asset: BrickType::Procedural {
                asset: Arc::new(String::from("PB_DefaultBrick")),
                size: BrickSize { x: 5, y: 5, z: 3 },
            },
            owner_index: None,
            position: Position { x: 0, y: 0, z: 0 },
            rotation: Default::default(),
            direction: Default::default(),
            collision: Default::default(),
            visible: true,
            color: Default::default(),
            material_intensity: 5,
            material: Brick::material_plastic(),
            components: Default::default(),
        }
    }
}

impl Clone for Brick {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            asset: self.asset.clone(),
            owner_index: self.owner_index.clone(),
            position: self.position.clone(),
            rotation: self.rotation.clone(),
            direction: self.direction.clone(),
            collision: self.collision.clone(),
            visible: self.visible.clone(),
            color: self.color.clone(),
            material: self.material.clone(),
            material_intensity: self.material_intensity.clone(),
            components: self
                .components
                .iter()
                // See `BoxedComponent` why this is necessary...
                .map(|c| c.boxed_component())
                .collect(),
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl AsBrdbValue for Color {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let field = prop_name.get(schema).unwrap();
        match field {
            "R" => Ok(&self.r),
            "G" => Ok(&self.g),
            "B" => Ok(&self.b),
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
        }
    }
}

#[derive(Clone, Debug, PartialOrd, Eq, PartialEq)]

pub enum BrickType {
    Basic(Arc<String>),
    Procedural { asset: Arc<String>, size: BrickSize },
}

impl BrickType {
    pub fn is_procedural(&self) -> bool {
        matches!(self, BrickType::Procedural { .. })
    }

    pub fn is_basic(&self) -> bool {
        matches!(self, BrickType::Basic(_))
    }
}

impl Ord for BrickType {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (BrickType::Basic(a), BrickType::Basic(b)) => a.cmp(b),
            // Basic bricks sort ascending before procedural bricks
            (BrickType::Basic(_), BrickType::Procedural { .. }) => Ordering::Less,
            // Procedural bricks are always greater than basic bricks
            (BrickType::Procedural { .. }, BrickType::Basic(_)) => Ordering::Greater,
            (
                BrickType::Procedural {
                    asset: a,
                    size: a_size,
                },
                BrickType::Procedural {
                    asset: b,
                    size: b_size,
                },
            ) => match a.cmp(b) {
                Ordering::Equal => a_size.cmp(b_size),
                ord => ord,
            },
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialOrd, Eq, PartialEq)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Ord for Position {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.z.cmp(&other.z) {
            Ordering::Equal => match self.y.cmp(&other.y) {
                Ordering::Equal => self.x.cmp(&other.x),
                ord => ord,
            },
            ord => ord,
        }
    }
}

pub const CHUNK_SIZE: i32 = 2048;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChunkIndex {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}
impl AsBrdbValue for ChunkIndex {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
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
impl Display for ChunkIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}_{}", self.x, self.y, self.z)
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd)]
pub struct BrickSize {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

impl Ord for BrickSize {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.z.cmp(&other.z) {
            Ordering::Equal => match self.y.cmp(&other.y) {
                Ordering::Equal => self.x.cmp(&other.x),
                ord => ord,
            },
            ord => ord,
        }
    }
}

impl AsBrdbValue for BrickSize {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
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
        _struct_name: crate::brdb::schema::BrdbInterned,
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
    MAX,
}

impl AsBrdbValue for Direction {
    fn as_brdb_enum(
        &self,
        _schema: &crate::brdb::schema::BrdbSchema,
        _def: &crate::brdb::schema::BrdbSchemaEnum,
    ) -> Result<i32, crate::brdb::errors::BrdbSchemaError> {
        Ok((*self as u8) as i32)
    }
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
        _struct_name: crate::brdb::schema::BrdbInterned,
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

#[derive(Default)]
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
    // RGB + Material intensity
    pub colors_and_alphas: Vec<(u8, u8, u8, u8)>,
}

impl BrickChunkSoA {
    /// Add a brick to the chunk. All basic bricks must be added before procedural bricks.
    pub(super) fn add_brick(&mut self, global_data: &BrdbSchemaGlobalData, brick: &Brick) {
        use BrickType::*;

        // Handle adding the asset type first
        match &brick.asset {
            Basic(asset) => {
                // Unwrap safety: The brick meta is added to the global data before adding bricks.
                let ty_index = global_data
                    .basic_brick_asset_names
                    .get_index_of(asset.as_str())
                    .unwrap() as u32;
                self.brick_type_indices.push(ty_index);
                self.procedural_brick_starting_index += 1;
            }
            Procedural { asset: kind, size } => {
                // Unwrap safety: The brick meta is added to the global data before adding bricks.
                let ty_index = global_data
                    .procedural_brick_asset_names
                    .get_index_of(kind.as_str())
                    .unwrap() as u32;

                if let (Some(last_sizes), Some(last_size)) =
                    (self.brick_size_counters.last_mut(), self.brick_sizes.last())
                {
                    // Check if the last asset and size match the current one
                    if last_sizes.asset_index == ty_index && last_size == size {
                        // Increment the size count for the last asset
                        last_sizes.num_sizes += 1;
                    } else {
                        // Push a new size counter for the new asset
                        self.brick_size_counters.push(BrickSizeCounter {
                            asset_index: ty_index,
                            num_sizes: 1,
                        });
                        self.brick_sizes.push(*size);
                    }
                } else {
                    self.brick_size_counters.push(BrickSizeCounter {
                        asset_index: ty_index,
                        num_sizes: 1,
                    });
                }
            }
        }

        self.owner_indices
            .push(brick.owner_index.unwrap_or(0) as u32);
        self.relative_positions.push(brick.position.to_relative().1);
        self.orientations
            .push(orientation_to_byte(brick.direction, brick.rotation));

        self.collision_flags_player.push(brick.collision.player);
        self.collision_flags_weapon.push(brick.collision.weapon);
        self.collision_flags_interaction
            .push(brick.collision.interact);
        self.collision_flags_tool.push(brick.collision.tool);
        self.visibility_flags.push(brick.visible);

        self.material_indices.push(
            global_data
                .material_asset_names
                .get_index_of(brick.material.as_str())
                .unwrap() as u8, // Unwrap safety: The material is added to the global data before adding bricks.
        );

        self.colors_and_alphas.push((
            brick.color.r,
            brick.color.g,
            brick.color.b,
            brick.material_intensity,
        ));
    }
}

impl AsBrdbValue for BrickChunkSoA {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
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
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "BrickSizeCounters" => Ok(self.brick_size_counters.as_brdb_iter()),
            "BrickSizes" => Ok(self.brick_sizes.as_brdb_iter()),
            "BrickTypeIndices" => Ok(self.brick_type_indices.as_brdb_iter()),
            "OwnerIndices" => Ok(self.owner_indices.as_brdb_iter()),
            "RelativePositions" => Ok(self.relative_positions.as_brdb_iter()),
            "Orientations" => Ok(self.orientations.as_brdb_iter()),
            "MaterialIndices" => Ok(self.material_indices.as_brdb_iter()),
            "ColorsAndAlphas" => Ok(self.colors_and_alphas.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}

#[derive(Default)]
pub struct BrickChunkIndexSoA {
    pub chunk_3d_indices: Vec<ChunkIndex>,
    pub num_bricks: Vec<u32>,
    pub num_components: Vec<u32>,
    pub num_wires: Vec<u32>,
}

impl AsBrdbValue for BrickChunkIndexSoA {
    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<BrdbArrayIter, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "Chunk3DIndices" => Ok(self.chunk_3d_indices.as_brdb_iter()),
            "NumBricks" => Ok(self.num_bricks.as_brdb_iter()),
            "NumComponents" => Ok(self.num_components.as_brdb_iter()),
            "NumWires" => Ok(self.num_wires.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}
