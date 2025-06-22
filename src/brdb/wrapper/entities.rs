use crate::brdb::{
    schema::as_brdb::{AsBrdbIter, AsBrdbValue, BrdbArrayIter},
    wrapper::{BitFlags, ChunkIndex, Quat4f, Vector3f},
};

pub struct EntityTypeCounter {
    pub type_index: u32,
    pub num_entities: u32,
}

impl AsBrdbValue for EntityTypeCounter {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "TypeIndex" => Ok(&self.type_index),
            "NumEntities" => Ok(&self.num_entities),
            _ => unreachable!(),
        }
    }
}

pub struct EntityColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}
impl AsBrdbValue for EntityColor {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "R" => Ok(&self.r),
            "G" => Ok(&self.g),
            "B" => Ok(&self.b),
            "A" => Ok(&self.a),
            _ => unreachable!(),
        }
    }
}
impl Default for EntityColor {
    fn default() -> Self {
        Self {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }
    }
}

#[derive(Default)]
pub struct EntityColors(
    pub EntityColor,
    pub EntityColor,
    pub EntityColor,
    pub EntityColor,
    pub EntityColor,
    pub EntityColor,
    pub EntityColor,
    pub EntityColor,
);
impl AsBrdbValue for EntityColors {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "Color0" => Ok(&self.0),
            "Color1" => Ok(&self.1),
            "Color2" => Ok(&self.2),
            "Color3" => Ok(&self.3),
            "Color4" => Ok(&self.4),
            "Color5" => Ok(&self.5),
            "Color6" => Ok(&self.6),
            "Color7" => Ok(&self.7),
            _ => unreachable!(),
        }
    }
}

pub struct EntityChunkSoA {
    pub type_counters: Vec<EntityTypeCounter>,
    pub persistent_indices: Vec<u32>,
    pub owner_indices: Vec<u32>,
    pub locations: Vec<Vector3f>,
    pub rotations: Vec<Quat4f>,
    pub weld_parent_flags: BitFlags,
    pub physics_locked_flags: BitFlags,
    pub physics_sleeping_flags: BitFlags,
    pub weld_parent_indices: Vec<u32>,
    pub linear_velocities: Vec<Vector3f>,
    pub angular_velocities: Vec<Vector3f>,
    pub colors_and_alphas: Vec<EntityColors>,
}

impl AsBrdbValue for EntityChunkSoA {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "WeldParentFlags" => Ok(&self.weld_parent_flags),
            "PhysicsLockedFlags" => Ok(&self.physics_locked_flags),
            "PhysicsSleepingFlags" => Ok(&self.physics_sleeping_flags),
            _ => unreachable!(),
        }
    }

    fn as_brdb_struct_prop_array(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<crate::brdb::schema::as_brdb::BrdbArrayIter, crate::brdb::errors::BrdbSchemaError>
    {
        match prop_name.get(schema).unwrap() {
            "TypeCounters" => Ok(self.type_counters.as_brdb_iter()),
            "PersistentIndices" => Ok(self.persistent_indices.as_brdb_iter()),
            "OwnerIndices" => Ok(self.owner_indices.as_brdb_iter()),
            "Locations" => Ok(self.locations.as_brdb_iter()),
            "Rotations" => Ok(self.rotations.as_brdb_iter()),
            "WeldParentIndices" => Ok(self.weld_parent_indices.as_brdb_iter()),
            "LinearVelocities" => Ok(self.linear_velocities.as_brdb_iter()),
            "AngularVelocities" => Ok(self.angular_velocities.as_brdb_iter()),
            "ColorsAndAlphas" => Ok(self.colors_and_alphas.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}

pub struct EntityChunkIndexSoA {
    pub next_persistent_index: u32,
    pub chunk_3d_indices: Vec<ChunkIndex>,
    pub num_entities: Vec<u32>,
}

impl AsBrdbValue for EntityChunkIndexSoA {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        _struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "NextPersistentIndex" => Ok(&self.next_persistent_index),
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
            "Chunk3DIndices" => Ok(self.chunk_3d_indices.as_brdb_iter()),
            "NumEntities" => Ok(self.num_entities.as_brdb_iter()),
            _ => unreachable!(),
        }
    }
}

/// This function may only be useful for legacy worlds from steam next fest.
/// New worlds will properly pair the class name with the entity type
pub fn lookup_entity_data_class_name(entity_type: &str) -> Option<&'static str> {
    Some(match entity_type {
        "Entity_Ball" => "BP_Entity_Ball_C",
        "Entity_Ball1" => "BP_Entity_Ball1_C",
        "Entity_DynamicBrickGrid" => "BrickGridDynamicActor",
        "Entity_GlobalBrickGrid" => "BP_BrickGrid_Global_C",
        "Entity_Wheel_Caster" => "BP_Entity_Wheel_Caster_C",
        "Entity_Wheel_Deep1" => "BP_Entity_Wheel_Deep1_C",
        "Entity_Wheel_Deep2" => "BP_Entity_Wheel_Deep2_C",
        "Entity_Wheel_Deep3" => "BP_Entity_Wheel_Deep3_C",
        "Entity_Wheel_DogDish1" => "BP_Entity_Wheel_DogDish1_C",
        "Entity_Wheel_DollarSign" => "BP_Entity_Wheel_DollarSign_C",
        "Entity_Wheel_German5" => "BP_Entity_Wheel_German5_C",
        "Entity_Wheel_GoKart" => "BP_Entity_Wheel_GoKart_C",
        "Entity_Wheel_LandingGear1" => "BP_Entity_Wheel_LandingGear1_C",
        "Entity_Wheel_Muscle1" => "BP_Entity_Wheel_Muscle1_C",
        "Entity_Wheel_Muscle2" => "BP_Entity_Wheel_Muscle2_C",
        "Entity_Wheel_Offroad1" => "BP_Entity_Wheel_Offroad1_C",
        "Entity_Wheel_Offroad2" => "BP_Entity_Wheel_Offroad2_C",
        "Entity_Wheel_Racing1" => "BP_Entity_Wheel_Racing1_C",
        "Entity_Wheel_Racing1_Decal" => "BP_Entity_Wheel_Racing1_Decal_C",
        "Entity_Wheel_Racing2B" => "BP_Entity_Wheel_Racing2B_C",
        "Entity_Wheel_Railroad1" => "BP_Entity_Wheel_Railroad1_C",
        "Entity_Wheel_SaladSpinner" => "BP_Entity_Wheel_SaladSpinner_C",
        "Entity_Wheel_SaladSpinnerFlipped" => "BP_Entity_Wheel_SaladSpinnerFlipped_C",
        "Entity_Wheel_Skateboard" => "BP_Entity_Wheel_Skateboard_C",
        "Entity_Wheel_Sport2" => "BP_Entity_Wheel_Sport2_C",
        "Entity_Wheel_Sport3" => "BP_Entity_Wheel_Sport3_C",
        "Entity_Wheel_Sport4" => "BP_Entity_Wheel_Sport4_C",
        "Entity_Wheel_Stance1" => "BP_Entity_Wheel_Stance1_C",
        "Entity_Wheel_Stance2" => "BP_Entity_Wheel_Stance2_C",
        "Entity_Wheel_Stance3" => "BP_Entity_Wheel_Stance3_C",
        "Entity_Wheel_Steelie1" => "BP_Entity_Wheel_Steelie1_C",
        "Entity_Wheel_Steelie2" => "BP_Entity_Wheel_Steelie2_C",
        "Entity_Wheel_Super1" => "BP_Entity_Wheel_Super1_C",
        "Entity_Wheel_Super1Flipped" => "BP_Entity_Wheel_Super1Flipped_C",
        "Entity_Wheel_Super2" => "BP_Entity_Wheel_Super2_C",
        "Entity_Wheel_Tracked1" => "BP_Entity_Wheel_Tracked1_C",
        "Entity_Wheel_TrackedSprocket1" => "BP_Entity_Wheel_TrackedSprocket1_C",
        "Entity_Wheel_Truck1" => "BP_Entity_Wheel_Truck1_C",
        "Entity_Wheel_Truck2" => "BP_Entity_Wheel_Truck2_C",
        "Entity_Wheel_Truck3" => "BP_Entity_Wheel_Truck3_C",
        "Entity_Wheel_Tuner1" => "BP_Entity_Wheel_Tuner1_C",
        "Entity_Wheel_Tuner2" => "BP_Entity_Wheel_Tuner2_C",
        "Entity_Wheel_Tuner3" => "BP_Entity_Wheel_Tuner3_C",
        "Entity_Wheel_Tuner3Flipped" => "BP_Entity_Wheel_Tuner3Flipped_C",
        "Entity_Wheel_Tuner4" => "BP_Entity_Wheel_Tuner4_C",
        "Entity_Wheel_Tuner5" => "BP_Entity_Wheel_Tuner5_C",
        "Entity_Wheel_Tuner6" => "BP_Entity_Wheel_Tuner6_C",
        "Entity_Wheel_Wagon1" => "BP_Entity_Wheel_Wagon1_C",
        "Entity_Wheel_Wagon2" => "BP_Entity_Wheel_Wagon2_C",
        "Entity_Wheel_Whitewall1" => "BP_Entity_Wheel_Whitewall1_C",
        "Entity_Wheel_Whitewall2" => "BP_Entity_Wheel_Whitewall2_C",
        _ => return None,
    })
}
