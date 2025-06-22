use crate::brdb::schema::as_brdb::AsBrdbValue;

pub struct Vector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl AsBrdbValue for Vector3f {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            "Z" => Ok(&self.z),
            _ => unreachable!(),
        }
    }
}

pub struct Quat4f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl AsBrdbValue for Quat4f {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
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
}
