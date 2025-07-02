use crate::schema::BrdbSchema;

pub const GLOBAL_DATA_SOA: &str = "BRSavedGlobalDataSoA";
pub const BRICK_CHUNK_SOA: &str = "BRSavedBrickChunkSoA";
pub const BRICK_COMPONENT_SOA: &str = "BRSavedComponentChunkSoA";
pub const BRICK_WIRE_SOA: &str = "BRSavedWireChunkSoA";
pub const BRICK_CHUNK_INDEX_SOA: &str = "BRSavedBrickChunkIndexSoA";
pub const ENTITY_CHUNK_SOA: &str = "BRSavedEntityChunkSoA";
pub const ENTITY_CHUNK_INDEX_SOA: &str = "BRSavedEntityChunkIndexSoA";
pub const OWNER_TABLE_SOA: &str = "BRSavedOwnerTableSoA";

/// World/0/GlobalData.schema
pub fn global_data_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedPrimaryAssetId {
    PrimaryAssetType: str,
    PrimaryAssetName: str,
}
struct BRSavedGlobalDataSoA {
    EntityTypeNames: str[],
    BasicBrickAssetNames: str[],
    ProceduralBrickAssetNames: str[],
    MaterialAssetNames: str[],
    ComponentTypeNames: str[],
    ComponentDataStructNames: str[],
    ComponentWirePortNames: str[],
    ExternalAssetReferences: BRSavedPrimaryAssetId[],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Bricks/ChunksShared.schema
pub fn bricks_chunks_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedBitFlags {
    Flags: u8[flat],
}
struct BRSavedBrickColor {
    R: u8,
    G: u8,
    B: u8,
    A: u8,
}
struct BRSavedBrickSize {
    X: u16,
    Y: u16,
    Z: u16,
}
struct BRSavedBrickSizeCounter {
    AssetIndex: u32,
    NumSizes: u32,
}
struct BRSavedRelativeBrickPosition {
    X: i16,
    Y: i16,
    Z: i16,
}
struct BRSavedBrickChunkSoA {
    ProceduralBrickStartingIndex: u32,
    BrickSizeCounters: BRSavedBrickSizeCounter[],
    BrickSizes: BRSavedBrickSize[],
    BrickTypeIndices: u32[],
    OwnerIndices: u32[],
    RelativePositions: BRSavedRelativeBrickPosition[flat],
    Orientations: u8[flat],
    CollisionFlags_Player: BRSavedBitFlags,
    CollisionFlags_Weapon: BRSavedBitFlags,
    CollisionFlags_Interaction: BRSavedBitFlags,
    CollisionFlags_Tool: BRSavedBitFlags,
    VisibilityFlags: BRSavedBitFlags,
    MaterialIndices: u8[flat],
    ColorsAndAlphas: BRSavedBrickColor[flat],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Bricks/ChunkIndexShared.schema
pub fn bricks_chunk_index_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedChunk3DIndex {
    X: i16,
    Y: i16,
    Z: i16,
}
struct BRSavedBrickChunkIndexSoA {
    Chunk3DIndices: BRSavedChunk3DIndex[],
    NumBricks: u32[],
    NumComponents: u32[],
    NumWires: u32[],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Bricks/ComponentsShared.schema
pub fn bricks_components_schema_min() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedBrickComponentTypeCounter {
    TypeIndex: u32,
    NumInstances: u32,
}
struct Quat4f {
    X: f32,
    Y: f32,
    Z: f32,
    W: f32,
}
struct Vector3f {
    X: f32,
    Y: f32,
    Z: f32,
}
struct BRSavedComponentChunkSoA {
    ComponentTypeCounters: BRSavedBrickComponentTypeCounter[],
    ComponentBrickIndices: u32[],
    JointBrickIndices: u32[],
    JointEntityReferences: u32[],
    JointInitialRelativeOffsets: Vector3f[flat],
    JointInitialRelativeRotations: Quat4f[flat],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Bricks/ComponentsShared.schema
pub fn bricks_components_schema_max() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
enum EBrickAxis {
    EBrickAxis::X = 0,
    EBrickAxis::Y = 1,
    EBrickAxis::Z = 2,
    EBrickAxis::MAX = 3,
}
enum EBrickDirection {
    EBrickDirection::X_Positive = 0,
    EBrickDirection::X_Negative = 1,
    EBrickDirection::Y_Positive = 2,
    EBrickDirection::Y_Negative = 3,
    EBrickDirection::Z_Positive = 4,
    EBrickDirection::Z_Negative = 5,
    EBrickDirection::MAX = 6,
}
struct BrickComponentData_AudioEmitter {
    bEnabled: bool,
    AudioDescriptor: object,
    VolumeMultiplier: f32,
    PitchMultiplier: f32,
    InnerRadius: f32,
    MaxDistance: f32,
    bSpatialization: bool,
    FocusAzimuth: f32,
    NonFocusAzimuth: f32,
    NonFocusVolumeAttenuation: f32,
}
struct BrickComponentData_Bearing {
    bLimitAngle: bool,
    LimitAngle: f32,
    Damping: f32,
}
struct BrickComponentData_BotSpawn {
    RespawnTime: f32,
    CorpseTimeout: f32,
    BotWeapon: class,
}
struct BrickComponentData_Button {
    PressSound: object,
    ReleaseSound: object,
    bAllowNearbyInteraction: bool,
    bHiddenInteraction: bool,
    PromptCustomLabel: str,
}
struct BrickComponentData_ConstantBool {
    bValue: bool,
}
struct BrickComponentData_ConstantFloat {
    Value: f64,
}
struct BrickComponentData_ConstantInt {
    Value: i64,
}
struct BrickComponentData_ConstantString {
    Value: str,
}
struct BrickComponentData_Damage {
    Message: str,
    ConsoleTag: str,
}
struct BrickComponentData_Destination {
}
struct BrickComponentData_EntityGate_PlayAudioOn {
    AudioDescriptor: object,
    VolumeMultiplier: f32,
    PitchMultiplier: f32,
    InnerRadius: f32,
    MaxDistance: f32,
    bSpatialization: bool,
}
struct BrickComponentData_EntityGate_ReadBrickGrid {
    bEnabled: bool,
}
struct BrickComponentData_GateBinary_BoolBool {
    bInputA: bool,
    bInputB: bool,
    bOutput: bool,
}
struct BrickComponentData_GateBinary_FloatBool {
    InputA: f64,
    InputB: f64,
    bOutput: bool,
}
struct BrickComponentData_GateBinary_FloatFloat {
    InputA: f64,
    InputB: f64,
    Output: f64,
}
struct BrickComponentData_GateBinary_IntInt {
    InputA: i64,
    InputB: i64,
    Output: i64,
}
struct BrickComponentData_GateBlendFloat {
    Blend: f64,
    InputA: f64,
    InputB: f64,
    Output: f64,
}
struct BrickComponentData_GateEdgeDetector {
    Input: f32,
    bPulseOnRisingEdge: bool,
    bPulseOnFallingEdge: bool,
    InputLast: f32,
}
struct BrickComponentData_GateTimer {
    bInput: bool,
    bOutput: bool,
    OnDuration: f32,
    OffDuration: f32,
    bOldInput: bool,
    bCurrentAction: bool,
    bHasCurrentAction: bool,
    Saved_RemainingTime: f32,
    bHasQueuedAction: bool,
    bQueuedAction: bool,
    QueuedDuration: f32,
}
struct BrickComponentData_GateTimer_Tick {
    bInput: bool,
    bOutput: bool,
    OnTicks: i32,
    OffTicks: i32,
    bOldInput: bool,
    bCurrentAction: bool,
    CurrentElapsed: i32,
    bHasQueuedAction: bool,
    bQueuedAction: bool,
    QueuedTicks: i32,
}
struct BrickComponentData_GateUnary_BoolBool {
    bInput: bool,
    bOutput: bool,
}
struct BrickComponentData_GateUnary_FloatFloat {
    Input: f64,
    Output: f64,
}
struct BrickComponentData_GateUnary_IntInt {
    Input: i64,
    Output: i64,
}
struct BrickComponentData_Interact {
    InteractSound: object,
    Message: str,
    ConsoleTag: str,
    bAllowNearbyInteraction: bool,
    bHiddenInteraction: bool,
    PromptCustomLabel: str,
}
struct BrickComponentData_Motor {
    bEnabled: bool,
    Speed: f32,
    Power: f32,
    bLimitAngle: bool,
    LimitAngle: f32,
    Damping: f32,
}
struct BrickComponentData_MotorSlider {
    bEnabled: bool,
    Speed: f32,
    Power: f32,
    Damping: f32,
}
struct BrickComponentData_OneShotAudioEmitter {
    AudioDescriptor: object,
    VolumeMultiplier: f32,
    PitchMultiplier: f32,
    InnerRadius: f32,
    MaxDistance: f32,
    bSpatialization: bool,
    bEnableRepeat: bool,
    RepeatTime: f32,
    RepeatVariance: f32,
}
struct BrickComponentData_RelativeTeleporter {
}
struct BrickComponentData_Rerouter {
}
struct BrickComponentData_Servo {
    bEnabled: bool,
    TargetAngle: f32,
    Power: f32,
    ActiveDamping: f32,
    ForceLimit: f32,
    bLimitAngle: bool,
    LimitAngle: f32,
    Damping: f32,
}
struct BrickComponentData_ServoSlider {
    bEnabled: bool,
    TargetPosition: f32,
    Power: f32,
    TopSpeed: f32,
    Exponent: f32,
    Damping: f32,
}
struct BrickComponentData_Slider {
    Damping: f32,
}
struct BrickComponentData_Switch {
    bEnabled: bool,
    InteractSound: object,
    bAllowNearbyInteraction: bool,
    bHiddenInteraction: bool,
    PromptCustomLabel: str,
}
struct BrickComponentData_Target {
    OnTime: f32,
}
struct BrickComponentData_Teleporter {
}
struct BrickComponentData_Touch {
    BeginTouchSound: object,
    EndTouchSound: object,
}
struct BrickComponentWireControl_DriveAndSteer {
}
struct BrickComponentWirePlayerInput {
}
struct BRSavedBrickComponentTypeCounter {
    TypeIndex: u32,
    NumInstances: u32,
}
struct Color {
    B: u8,
    G: u8,
    R: u8,
    A: u8,
}
struct IntVector {
    X: i32,
    Y: i32,
    Z: i32,
}
struct Quat4f {
    X: f32,
    Y: f32,
    Z: f32,
    W: f32,
}
struct Rotator3f {
    Pitch: f32,
    Yaw: f32,
    Roll: f32,
}
struct Vector3f {
    X: f32,
    Y: f32,
    Z: f32,
}
struct BrickComponentData_CharacterGate_Rotation {
    Rotation: Rotator3f,
}
struct BrickComponentData_EntityGate_Rotation {
    Rotation: Rotator3f,
}
struct BrickComponentData_EntityGate_Vector {
    Vector: Vector3f,
}
struct BrickComponentData_EntityGate_VectorAndRotation {
    Vector: Vector3f,
    Rotation: Rotator3f,
}
struct BrickComponentData_InputSplitter {
    PlayerInput: BrickComponentWirePlayerInput,
}
struct BrickComponentData_ItemSpawn {
    PickupClass: class,
    bPickupEnabled: bool,
    bPickupRespawnOnMinigameReset: bool,
    PickupMinigameResetRespawnDelay: f32,
    bPickupAutoDisableOnPickup: bool,
    PickupRespawnTime: f32,
    PickupOffsetDirection: EBrickDirection,
    PickupOffsetDistance: f32,
    PickupRotation: Rotator3f,
    PickupScale: f32,
    bPickupAnimationEnabled: bool,
    PickupAnimationAxis: EBrickAxis,
    bPickupAnimationAxisLocal: bool,
    PickupSpinSpeed: f32,
    PickupBobSpeed: f32,
    PickupBobHeight: f32,
    PickupAnimationPhase: f32,
}
struct BrickComponentData_Joint_Wheel {
    Control_DriveAndSteer: BrickComponentWireControl_DriveAndSteer,
    bEnabled: bool,
    DriveSpeed: f32,
    DrivePower: f32,
    bSteerEnabled: bool,
    Steer: f32,
    SteerLimitDegree: f32,
    SteerPower: f32,
    bSuspensionEnabled: bool,
    SuspensionStiffness: f32,
    SuspensionDamping: f32,
    JointDistance: i32,
    bDriveWhenNotAttachedToEngine: bool,
    bCanBrake: bool,
    bAllowEngineSteerCorrect: bool,
    Damping: f32,
}
struct BrickComponentData_PointLight {
    bMatchBrickShape: bool,
    bEnabled: bool,
    Brightness: f32,
    Radius: f32,
    Color: Color,
    bUseBrickColor: bool,
    bCastShadows: bool,
}
struct BrickComponentData_Seat {
    PlayerInput: BrickComponentWirePlayerInput,
    bIsOccupied: bool,
    bAllowNearbyInteraction: bool,
    bHiddenInteraction: bool,
    PromptCustomLabel: str,
}
struct BrickComponentData_SpotLight {
    Rotation: Rotator3f,
    InnerConeAngle: f32,
    OuterConeAngle: f32,
    bEnabled: bool,
    Brightness: f32,
    Radius: f32,
    Color: Color,
    bUseBrickColor: bool,
    bCastShadows: bool,
}
struct BrickComponentData_WheelEngine {
    bEnabled: bool,
    bEnableManualControl: bool,
    ManualInput_Drive: f32,
    ManualInput_Steer: f32,
    bManualInput_Brake: bool,
    PlayerInput: BrickComponentWirePlayerInput,
    Control_DriveAndSteer: BrickComponentWireControl_DriveAndSteer,
    CustomMassOffset: IntVector,
    CustomMassSize: IntVector,
    CustomMass: f32,
    DriveInterpSpeed: f32,
    DriveSpeed: f32,
    DriveAcceleratingPowerMultiplier: f32,
    DriveBrakingPowerMultiplier: f32,
    DriveDampingMultiplier: f32,
    SteerPowerMultiplier: f32,
    SteerInterpSpeed: f32,
    SteerLimitDegree: f32,
    CenterOfSteering: f32,
    bTankSteering: bool,
    TankSteerSpeedMultiplier: f32,
    WaterDriveForce: f32,
    WaterSteeringForce: f32,
    AudioDescriptor: object,
}
struct BRSavedComponentChunkSoA {
    ComponentTypeCounters: BRSavedBrickComponentTypeCounter[],
    ComponentBrickIndices: u32[],
    JointBrickIndices: u32[],
    JointEntityReferences: u32[],
    JointInitialRelativeOffsets: Vector3f[flat],
    JointInitialRelativeRotations: Quat4f[flat],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Bricks/WiresShared.schema
pub fn bricks_wires_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedBitFlags {
    Flags: u8[flat],
}
struct BRSavedChunk3DIndex {
    X: i16,
    Y: i16,
    Z: i16,
}
struct BRSavedLocalWirePortSource {
    BrickIndexInChunk: u32,
    ComponentTypeIndex: u16,
    PortIndex: u16,
}
struct BRSavedWirePortTarget {
    BrickIndexInChunk: u32,
    ComponentTypeIndex: u16,
    PortIndex: u16,
}
struct BRSavedRemoteWirePortSource {
    GridPersistentIndex: u32,
    ChunkIndex: BRSavedChunk3DIndex,
    BrickIndexInChunk: u32,
    ComponentTypeIndex: u16,
    PortIndex: u16,
}
struct BRSavedWireChunkSoA {
    RemoteWireSources: BRSavedRemoteWirePortSource[],
    LocalWireSources: BRSavedLocalWirePortSource[],
    RemoteWireTargets: BRSavedWirePortTarget[],
    LocalWireTargets: BRSavedWirePortTarget[],
    PendingPropagationFlags: BRSavedBitFlags,
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Owners.schema
pub fn owners_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRGuid {
    A: u32,
    B: u32,
    C: u32,
    D: u32,
}
struct BRSavedOwnerTableSoA {
    UserIds: BRGuid[flat],
    UserNames: str[],
    DisplayNames: str[],
    EntityCounts: u32[],
    BrickCounts: u32[],
    ComponentCounts: u32[],
    WireCounts: u32[],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Entities/ChunkIndex.schema
pub fn entities_chunk_index_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedChunk3DIndex {
    X: i16,
    Y: i16,
    Z: i16,
}
struct BRSavedEntityChunkIndexSoA {
    NextPersistentIndex: u32,
    Chunk3DIndices: BRSavedChunk3DIndex[],
    NumEntities: u32[],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

/// World/0/Entities/ChunksShared.schema
pub fn entities_chunks_schema() -> BrdbSchema {
    let (enums, structs) = BrdbSchema::parse_to_meta(
        "
struct BRSavedBitFlags {
    Flags: u8[flat],
}
struct BRSavedBrickColor {
    R: u8,
    G: u8,
    B: u8,
    A: u8,
}
struct BRSavedEntityTypeCounter {
    TypeIndex: u32,
    NumEntities: u32,
}
struct Quat4f {
    X: f32,
    Y: f32,
    Z: f32,
    W: f32,
}
struct Vector3f {
    X: f32,
    Y: f32,
    Z: f32,
}
struct BRSavedEntityColors {
    Color0: BRSavedBrickColor,
    Color1: BRSavedBrickColor,
    Color2: BRSavedBrickColor,
    Color3: BRSavedBrickColor,
    Color4: BRSavedBrickColor,
    Color5: BRSavedBrickColor,
    Color6: BRSavedBrickColor,
    Color7: BRSavedBrickColor,
}
struct BRSavedEntityChunkSoA {
    TypeCounters: BRSavedEntityTypeCounter[],
    PersistentIndices: u32[],
    OwnerIndices: u32[],
    Locations: Vector3f[flat],
    Rotations: Quat4f[flat],
    WeldParentFlags: BRSavedBitFlags,
    PhysicsLockedFlags: BRSavedBitFlags,
    PhysicsSleepingFlags: BRSavedBitFlags,
    WeldParentIndices: u32[],
    LinearVelocities: Vector3f[flat],
    AngularVelocities: Vector3f[flat],
    ColorsAndAlphas: BRSavedEntityColors[flat],
}",
    )
    .unwrap();
    BrdbSchema::from_meta(enums, structs)
}

#[cfg(test)]
mod test {
    /// Ensure the above schemas compile and can be instantiated.
    #[test]
    fn test_schema() {
        use super::*;
        let _ = global_data_schema();
        let _ = bricks_chunks_schema();
        let _ = bricks_chunk_index_schema();
        let _ = bricks_components_schema_min();
        let _ = bricks_components_schema_max();
        let _ = bricks_wires_schema();
        let _ = owners_schema();
        let _ = entities_chunk_index_schema();
        let _ = entities_chunks_schema();
    }
}
