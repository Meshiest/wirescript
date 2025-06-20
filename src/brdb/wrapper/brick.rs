use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Brick {
    pub asset: BrickType,
    pub owner_index: Option<usize>,
    pub position: Position,
    pub collision: Collision,
    pub visible: bool,
    pub color: Color,
    pub material: u8,
    pub material_intensity: u8,
}

impl Default for Brick {
    fn default() -> Self {
        Self {
            asset: BrickType::Procedural {
                kind: Arc::new(String::from("PB_DefaultBrick")),
                width: 5,
                length: 5,
                height: 3,
            },
            owner_index: None,
            position: Position { x: 0, y: 0, z: 0 },
            collision: Default::default(),
            visible: true,
            color: Default::default(),
            material: 0,
            material_intensity: 5,
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

#[derive(Clone, Debug)]

pub enum BrickType {
    Basic(Arc<String>),
    Procedural {
        kind: Arc<String>,
        width: u16,
        length: u16,
        height: u16,
    },
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

#[derive(Copy, Clone, Debug, Default)]
pub struct RelativePosition {
    pub x: i16,
    pub y: i16,
    pub z: i16,
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
