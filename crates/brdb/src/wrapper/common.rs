use crate::{
    schema::{BrdbValue, as_brdb::AsBrdbValue},
    wrapper::CHUNK_SIZE,
};

#[derive(Default, Debug, Clone, Copy)]
pub struct Vector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl TryFrom<&BrdbValue> for Vector3f {
    type Error = crate::errors::BrdbSchemaError;

    fn try_from(value: &BrdbValue) -> Result<Self, Self::Error> {
        let x = value.prop("X")?.as_brdb_f32()?;
        let y = value.prop("Y")?.as_brdb_f32()?;
        let z = value.prop("Z")?.as_brdb_f32()?;
        Ok(Self { x, y, z })
    }
}

impl AsBrdbValue for Vector3f {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            _ => unreachable!(),
        }
    }
}
impl std::ops::Neg for Vector3f {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}
impl std::ops::Add for Vector3f {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}
impl std::ops::AddAssign for Vector3f {
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
        self.z += other.z;
    }
}
impl std::ops::Sub for Vector3f {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}
impl std::ops::SubAssign for Vector3f {
    fn sub_assign(&mut self, other: Self) {
        self.x -= other.x;
        self.y -= other.y;
        self.z -= other.z;
    }
}
impl std::ops::Mul<f32> for Vector3f {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self::Output {
        Self {
            x: self.x * scalar,
            y: self.y * scalar,
            z: self.z * scalar,
        }
    }
}
impl std::ops::MulAssign<f32> for Vector3f {
    fn mul_assign(&mut self, scalar: f32) {
        self.x *= scalar;
        self.y *= scalar;
        self.z *= scalar;
    }
}
impl std::ops::Mul<Vector3f> for f32 {
    type Output = Vector3f;
    fn mul(self, vec: Vector3f) -> Self::Output {
        Vector3f {
            x: self * vec.x,
            y: self * vec.y,
            z: self * vec.z,
        }
    }
}
impl std::ops::MulAssign<Vector3f> for f32 {
    fn mul_assign(&mut self, vec: Vector3f) {
        *self = *self * vec.x;
        *self = *self * vec.y;
        *self = *self * vec.z;
    }
}
impl std::ops::Div<f32> for Vector3f {
    type Output = Self;
    fn div(self, scalar: f32) -> Self::Output {
        if scalar == 0.0 {
            panic!("Division by zero in Vector3f");
        }
        Self {
            x: self.x / scalar,
            y: self.y / scalar,
            z: self.z / scalar,
        }
    }
}
impl std::ops::DivAssign<f32> for Vector3f {
    fn div_assign(&mut self, scalar: f32) {
        if scalar == 0.0 {
            panic!("Division by zero in Vector3f");
        }
        self.x /= scalar;
        self.y /= scalar;
        self.z /= scalar;
    }
}

impl From<(f32, f32, f32)> for Vector3f {
    fn from(tuple: (f32, f32, f32)) -> Self {
        Self {
            x: tuple.0,
            y: tuple.1,
            z: tuple.2,
        }
    }
}

impl Vector3f {
    pub const UP: Self = Self::new(0.0, 0.0, 1.0);
    pub const DOWN: Self = Self::new(0.0, 0.0, -1.0);
    pub const LEFT: Self = Self::new(-1.0, 0.0, 0.0);
    pub const RIGHT: Self = Self::new(1.0, 0.0, 0.0);
    pub const FORWARD: Self = Self::new(0.0, -1.0, 0.0);
    pub const BACKWARD: Self = Self::new(0.0, 1.0, 0.0);
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0);
    pub const CHUNK_SIZE: Self = Self::new(CHUNK_SIZE as f32, CHUNK_SIZE as f32, CHUNK_SIZE as f32);
    pub const CHUNK_HALF: Self = Self::new(
        CHUNK_SIZE as f32 / 2.0,
        CHUNK_SIZE as f32 / 2.0,
        CHUNK_SIZE as f32 / 2.0,
    );

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn normalize(self) -> Self {
        let length = (self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if length == 0.0 {
            return Self::default();
        }
        self / length
    }
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Quat4f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl TryFrom<&BrdbValue> for Quat4f {
    type Error = crate::errors::BrdbSchemaError;

    fn try_from(value: &BrdbValue) -> Result<Self, Self::Error> {
        let x = value.prop("X")?.as_brdb_f32()?;
        let y = value.prop("Y")?.as_brdb_f32()?;
        let z = value.prop("Z")?.as_brdb_f32()?;
        let w = value.prop("W")?.as_brdb_f32()?;
        Ok(Self { x, y, z, w })
    }
}

impl AsBrdbValue for Quat4f {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::schema::BrdbSchema,
        _struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            "W" => Ok(&self.w),
            _ => unreachable!(),
        }
    }
}

impl Quat4f {
    pub fn identity() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }

    pub fn from_axis_angle(axis: Vector3f, angle: f32) -> Self {
        let half_angle = angle * 0.5;
        let sin_half_angle = half_angle.sin();
        Self {
            x: axis.x * sin_half_angle,
            y: axis.y * sin_half_angle,
            z: axis.z * sin_half_angle,
            w: half_angle.cos(),
        }
    }

    pub fn from_euler_angles(x: f32, y: f32, z: f32) -> Self {
        let half_x = x * 0.5;
        let half_y = y * 0.5;
        let half_z = z * 0.5;

        let (sin_x, cos_x) = half_x.sin_cos();
        let (sin_y, cos_y) = half_y.sin_cos();
        let (sin_z, cos_z) = half_z.sin_cos();

        Self {
            x: sin_x * cos_y * cos_z - cos_x * sin_y * sin_z,
            y: cos_x * sin_y * cos_z + sin_x * cos_y * sin_z,
            z: cos_x * cos_y * sin_z - sin_x * sin_y * cos_z,
            w: cos_x * cos_y * cos_z + sin_x * sin_y * sin_z,
        }
    }

    pub fn look_at(forward: Vector3f, up: Vector3f) -> Self {
        let forward = forward.normalize();
        let right = up.cross(forward).normalize();
        let up = forward.cross(right);

        Self {
            x: right.x,
            y: up.y,
            z: forward.z,
            w: 0.0,
        }
    }
}
