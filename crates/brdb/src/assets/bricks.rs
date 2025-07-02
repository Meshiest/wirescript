use crate::wrapper::{BString, BrickType};

pub const B_REROUTE: BrickType = BrickType::str("B_1x1_Reroute_Node");

pub const B_GATE_BOOL_AND: BrickType = BrickType::str("B_1x1_Gate_AND");
pub const B_GATE_BOOL_OR: BrickType = BrickType::str("B_1x1_Gate_OR");
pub const B_GATE_BOOL_XOR: BrickType = BrickType::str("B_1x1_Gate_XOR");
pub const B_GATE_BOOL_NAND: BrickType = BrickType::str("B_1x1_Gate_NAND");
pub const B_GATE_BOOL_NOR: BrickType = BrickType::str("B_1x1_Gate_NOR");
pub const B_GATE_BOOL_NOT: BrickType = BrickType::str("B_1x1_NOT_Gate");

pub const B_GATE_BIT_AND: BrickType = BrickType::str("B_1x1_Gate_AND_Bitwise");
pub const B_GATE_BIT_OR: BrickType = BrickType::str("B_1x1_Gate_OR_Bitwise");
pub const B_GATE_BIT_XOR: BrickType = BrickType::str("B_1x1_Gate_XOR_Bitwise");
pub const B_GATE_BIT_NAND: BrickType = BrickType::str("B_1x1_Gate_NAND_Bitwise");
pub const B_GATE_BIT_NOR: BrickType = BrickType::str("B_1x1_Gate_NOR_Bitwise");
pub const B_GATE_BIT_NOT: BrickType = BrickType::str("B_1x1_Gate_NOT_Bitwise");
pub const B_GATE_BIT_SHIFT_LEFT: BrickType = BrickType::str("B_1x1_Gate_ShiftLeft_Bitwise");
pub const B_GATE_BIT_SHIFT_RIGHT: BrickType = BrickType::str("B_1x1_Gate_ShiftRight_Bitwise");

pub const B_GATE_ADD: BrickType = BrickType::str("B_1x1_Gate_Add");
pub const B_GATE_SUBTRACT: BrickType = BrickType::str("B_1x1_Gate_Subtract");
pub const B_GATE_MULTIPLY: BrickType = BrickType::str("B_1x1_Gate_Multiply");
pub const B_GATE_MOD_FLOORED: BrickType = BrickType::str("B_1x1_Gate_ModFloored");
pub const B_GATE_MOD: BrickType = BrickType::str("B_1x1_Gate_Mod");
pub const B_GATE_DIVIDE: BrickType = BrickType::str("B_1x1_Gate_Divide");

pub const B_GATE_EQUAL: BrickType = BrickType::str("B_1x1_Gate_Equal");
pub const B_GATE_NOT_EQUAL: BrickType = BrickType::str("B_1x1_Gate_NotEqual");
pub const B_GATE_LESS_THAN: BrickType = BrickType::str("B_1x1_Gate_LessThan");
pub const B_GATE_LESS_THAN_EQUAL: BrickType = BrickType::str("B_1x1_Gate_LessThanEqual");
pub const B_GATE_GREATER_THAN: BrickType = BrickType::str("B_1x1_Gate_GreaterThan");
pub const B_GATE_GREATER_THAN_EQUAL: BrickType = BrickType::str("B_1x1_Gate_GreaterThanEqual");

pub const B_GATE_CEILING: BrickType = BrickType::str("B_1x1_Gate_Ceiling");
pub const B_GATE_FLOOR: BrickType = BrickType::str("B_1x1_Gate_Floor");

pub const B_GATE_CONSTANT: BrickType = BrickType::str("B_1x1_Gate_Constant");

pub const B_GATE_BLEND: BrickType = BrickType::str("B_1x1_Gate_Blend");
pub const B_GATE_EDGE_DETECTOR: BrickType = BrickType::str("B_1x1_Gate_EdgeDetector");
pub const B_GATE_BUFFER: BrickType = BrickType::str("B_1x1_Gate_Timer");
pub const B_GATE_BUFFER_TICK: BrickType = BrickType::str("B_1x1_Gate_Timer_Tick");

pub const B_2X2_OVERHANG: BrickType = BrickType::str("B_2x2_Overhang");
pub const B_1X4_BRICK_SIDE: BrickType = BrickType::str("B_1x4_Brick_Side");
pub const B_1X2_OVERHANG: BrickType = BrickType::str("B_1x2_Overhang");
pub const B_1X1_BRICK_SIDE_LIP: BrickType = BrickType::str("B_1x1_Brick_Side_Lip");
pub const B_1X1_BRICK_SIDE: BrickType = BrickType::str("B_1x1_Brick_Side");
pub const B_JAR: BrickType = BrickType::str("B_Jar");
pub const B_CHALICE: BrickType = BrickType::str("B_Chalice");
pub const B_CAULDRON: BrickType = BrickType::str("B_Cauldron");
pub const B_SMALL_FLOWER: BrickType = BrickType::str("B_Small_Flower");
pub const B_PINE_TREE: BrickType = BrickType::str("B_Pine_Tree");
pub const B_LEAF_BUSH: BrickType = BrickType::str("B_Leaf_Bush");
pub const B_HEDGE_1X4: BrickType = BrickType::str("B_Hedge_1x4");
pub const B_HEDGE_1X2: BrickType = BrickType::str("B_Hedge_1x2");
pub const B_HEDGE_1X1_CORNER: BrickType = BrickType::str("B_Hedge_1x1_Corner");
pub const B_HEDGE_1X1: BrickType = BrickType::str("B_Hedge_1x1");
pub const B_FLOWER: BrickType = BrickType::str("B_Flower");
pub const B_FERN: BrickType = BrickType::str("B_Fern");
pub const B_BRANCH: BrickType = BrickType::str("B_Branch");
pub const B_2X4_DOOR_FRAME: BrickType = BrickType::str("B_2x4_Door_Frame");
pub const B_TURKEY_LEG: BrickType = BrickType::str("B_Turkey_Leg");
pub const B_TURKEY_BODY: BrickType = BrickType::str("B_Turkey_Body");
pub const B_SWIRL_PLATE: BrickType = BrickType::str("B_Swirl_Plate");
pub const B_SAUSAGE: BrickType = BrickType::str("B_Sausage");
pub const B_PUMPKIN_CARVED: BrickType = BrickType::str("B_Pumpkin_Carved");
pub const B_PUMPKIN: BrickType = BrickType::str("B_Pumpkin");
pub const B_LADDER: BrickType = BrickType::str("B_Ladder");
pub const B_INVERTED_CONE: BrickType = BrickType::str("B_Inverted_Cone");
pub const B_HANDLE: BrickType = BrickType::str("B_Handle");
pub const B_GRAVESTONE: BrickType = BrickType::str("B_Gravestone");
pub const B_FROG_SMALL: BrickType = BrickType::str("B_Frog_Small");
pub const B_FROG: BrickType = BrickType::str("B_Frog");
pub const B_FLAME: BrickType = BrickType::str("B_Flame");
pub const B_COFFIN_LID: BrickType = BrickType::str("B_Coffin_Lid");
pub const B_COFFIN: BrickType = BrickType::str("B_Coffin");
pub const B_BONE_STRAIGHT: BrickType = BrickType::str("B_BoneStraight");
pub const B_BONE: BrickType = BrickType::str("B_Bone");
pub const B_1X1_COIN_DIAGONAL: BrickType = BrickType::str("B_1x1_Coin_Diagonal");
pub const B_1X1_COIN: BrickType = BrickType::str("B_1x1_Coin");
pub const B_SPOON: BrickType = BrickType::str("B_Spoon");
pub const B_FORK: BrickType = BrickType::str("B_Fork");
pub const B_2X2_CORNER: BrickType = BrickType::str("B_2x2_Corner");
pub const B_2X2F_PLATE_CENTER_INV: BrickType = BrickType::str("B_2x2f_Plate_Center_Inv");
pub const B_2X2F_PLATE_CENTER: BrickType = BrickType::str("B_2x2f_Plate_Center");
pub const B_1X2F_PLATE_CENTER_INV: BrickType = BrickType::str("B_1x2f_Plate_Center_Inv");
pub const B_4X4_ROUND: BrickType = BrickType::str("B_4x4_Round");
pub const B_2X2_ROUND: BrickType = BrickType::str("B_2x2_Round");
pub const B_2X2F_ROUND: BrickType = BrickType::str("B_2x2F_Round");
pub const B_2X_OCTO_T_INV: BrickType = BrickType::str("B_2x_Octo_T_Inv");
pub const B_2X_OCTO_T: BrickType = BrickType::str("B_2x_Octo_T");
pub const B_2X2F_OCTO_CONVERTER_INV: BrickType = BrickType::str("B_2x2F_Octo_Converter_Inv");
pub const B_2X2F_OCTO_CONVERTER: BrickType = BrickType::str("B_2x2F_Octo_Converter");
pub const B_2X2F_OCTO: BrickType = BrickType::str("B_2x2F_Octo");
pub const B_8X8_LATTICE_PLATE: BrickType = BrickType::str("B_8x8_Lattice_Plate");
pub const B_1X2_METAL_INGOT: BrickType = BrickType::str("B_1x2_MetalIngot");
pub const B_1X1F_TILE_CORNER: BrickType = BrickType::str("B_1x1f_Tile_Corner");
pub const B_1X1F_INVERSE_TILE_CORNER: BrickType = BrickType::str("B_1x1f_Inverse_Tile_Corner");
pub const B_ROOK: BrickType = BrickType::str("B_Rook");
pub const B_QUEEN: BrickType = BrickType::str("B_Queen");
pub const B_PAWN: BrickType = BrickType::str("B_Pawn");
pub const B_KNIGHT: BrickType = BrickType::str("B_Knight");
pub const B_KING: BrickType = BrickType::str("B_King");
pub const B_BISHOP: BrickType = BrickType::str("B_Bishop");
pub const B_1X2F_PLATE_CENTER: BrickType = BrickType::str("B_1x2f_Plate_Center");
pub const B_2X2_CONE: BrickType = BrickType::str("B_2x2_Cone");
pub const B_1X1_ROUND: BrickType = BrickType::str("B_1x1_Round");
pub const B_1X1_CONE: BrickType = BrickType::str("B_1x1_Cone");
pub const B_1X1F_ROUND: BrickType = BrickType::str("B_1x1F_Round");
pub const B_2X_OCTO_CONE: BrickType = BrickType::str("B_2x_Octo_Cone");
pub const B_2X_OCTO_90DEG_INV: BrickType = BrickType::str("B_2x_Octo_90Deg_Inv");
pub const B_2X_OCTO_90DEG: BrickType = BrickType::str("B_2x_Octo_90Deg");
pub const B_2X_OCTO: BrickType = BrickType::str("B_2x_Octo");
pub const B_1X_OCTO_T_INV: BrickType = BrickType::str("B_1x_Octo_T_Inv");
pub const B_1X_OCTO_T: BrickType = BrickType::str("B_1x_Octo_T");
pub const B_1X_OCTO_90DEG_INV: BrickType = BrickType::str("B_1x_Octo_90Deg_Inv");
pub const B_1X_OCTO_90DEG: BrickType = BrickType::str("B_1x_Octo_90Deg");
pub const B_1X_OCTO: BrickType = BrickType::str("B_1x_Octo");
pub const B_1X1F_OCTO: BrickType = BrickType::str("B_1x1F_Octo");
pub const B_2X2_SLIPPER: BrickType = BrickType::str("B_2x2_Slipper");
pub const B_2X1_SLIPPER: BrickType = BrickType::str("B_2x1_Slipper");
pub const B_JOINT_WHEEL_MICRO: BrickType = BrickType::str("B_Joint_Wheel_Micro");
pub const B_JOINT_WHEEL: BrickType = BrickType::str("B_Joint_Wheel");
pub const B_JOINT_SOCKET_MICRO: BrickType = BrickType::str("B_Joint_Socket_Micro");
pub const B_JOINT_SERVO_MICRO: BrickType = BrickType::str("B_Joint_Servo_Micro");
pub const B_JOINT_SERVO: BrickType = BrickType::str("B_Joint_Servo");
pub const B_JOINT_MOTOR_MICRO: BrickType = BrickType::str("B_Joint_Motor_Micro");
pub const B_JOINT_MOTOR: BrickType = BrickType::str("B_Joint_Motor");
pub const B_JOINT_COUPLER: BrickType = BrickType::str("B_Joint_Coupler");
pub const B_JOINT_BEARING_MICRO: BrickType = BrickType::str("B_Joint_Bearing_Micro");
pub const B_JOINT_BEARING: BrickType = BrickType::str("B_Joint_Bearing");
pub const B_1X1_SOUND_EMITTER: BrickType = BrickType::str("B_1x1_SoundEmitter");
pub const B_1X1_GATE_TELEPORT: BrickType = BrickType::str("B_1x1_Gate_Teleport");
pub const B_1X1_GATE_RELATIVE_TELEPORT: BrickType = BrickType::str("B_1x1_Gate_RelativeTeleport");
pub const B_1X1_ENTITY_GATE_SET_VELOCITY: BrickType =
    BrickType::str("B_1x1_EntityGate_SetVelocity");
pub const B_1X1_ENTITY_GATE_SET_LOCATION_AND_ROTATION: BrickType =
    BrickType::str("B_1x1_EntityGate_SetLocationAndRotation");
pub const B_1X1_ENTITY_GATE_SET_LOCATION: BrickType =
    BrickType::str("B_1x1_EntityGate_SetLocation");
pub const B_1X1_ENTITY_GATE_READ_BRICK_GRID: BrickType =
    BrickType::str("B_1x1_EntityGate_ReadBrickGrid");
pub const B_1X1_ENTITY_GATE_PLAY_AUDIO_AT: BrickType =
    BrickType::str("B_1x1_EntityGate_PlayAudioAt");
pub const B_1X1_ENTITY_GATE_ADD_VELOCITY: BrickType =
    BrickType::str("B_1x1_EntityGate_AddVelocity");
pub const B_1X1_ENTITY_GATE_ADD_LOCATION_AND_ROTATION: BrickType =
    BrickType::str("B_1x1_EntityGate_AddLocationAndRotation");
pub const B_1X1_CHARACTER_GATE_SET_GRAVITY_DIRECTION: BrickType =
    BrickType::str("B_1x1_CharacterGate_SetGravityDirection");
pub const B_VEHICLE_ENGINE: BrickType = BrickType::str("B_Vehicle_Engine");
pub const B_SWITCH_TEST: BrickType = BrickType::str("B_Switch_Test");
pub const B_SPAWN_POINT: BrickType = BrickType::str("B_SpawnPoint");
pub const B_SEAT: BrickType = BrickType::str("B_Seat");
pub const B_GOAL_POINT: BrickType = BrickType::str("B_GoalPoint");
pub const B_DESTINATION_POINT: BrickType = BrickType::str("B_DestinationPoint");
pub const B_CHECK_POINT: BrickType = BrickType::str("B_CheckPoint");
pub const B_BUTTON_SQUARE: BrickType = BrickType::str("B_Button_Square");
pub const B_BUTTON: BrickType = BrickType::str("B_Button");
pub const B_BOT_SPAWN_POINT: BrickType = BrickType::str("B_Bot_Spawn_Point");
pub const B_2X2F_TARGET: BrickType = BrickType::str("B_2x2F_Target");
pub const B_2X2F_SPEAKER: BrickType = BrickType::str("B_2x2F_Speaker");
pub const B_1X1_GATE_WHEEL_ENGINE_SLIM: BrickType = BrickType::str("B_1x1_Gate_WheelEngineSlim");
pub const B_1X1F_SPEAKER: BrickType = BrickType::str("B_1x1F_Speaker");

pub const PB_DEFAULT_BRICK: BString = BString::str("PB_DefaultBrick");
pub const PB_DEFAULT_STUDDED: BString = BString::str("PB_DefaultStudded");
pub const PB_DEFAULT_RAMP_INNER_CORNER_INVERTED: BString =
    BString::str("PB_DefaultRampInnerCornerInverted");
pub const PB_DEFAULT_RAMP_CREST_END: BString = BString::str("PB_DefaultRampCrestEnd");
pub const PB_DEFAULT_RAMP_CREST_CORNER: BString = BString::str("PB_DefaultRampCrestCorner");
pub const PB_DEFAULT_RAMP_CREST: BString = BString::str("PB_DefaultRampCrest");
pub const PB_DEFAULT_RAMP: BString = BString::str("PB_DefaultRamp");
pub const PB_PICKET_FENCE: BString = BString::str("PB_PicketFence");
pub const BP_LATTICE_THIN: BString = BString::str("BP_LatticeThin");
pub const PB_DEFAULT_WEDGE: BString = BString::str("PB_DefaultWedge");
pub const PB_DEFAULT_TILE: BString = BString::str("PB_DefaultTile");
pub const PB_DEFAULT_SMOOTH_TILE: BString = BString::str("PB_DefaultSmoothTile");
pub const PB_DEFAULT_SIDE_WEDGE: BString = BString::str("PB_DefaultSideWedge");
pub const PB_DEFAULT_RAMP_INVERTED: BString = BString::str("PB_DefaultRampInverted");
pub const PB_DEFAULT_RAMP_INNER_CORNER: BString = BString::str("PB_DefaultRampInnerCorner");
pub const PB_DEFAULT_RAMP_CORNER_INVERTED: BString = BString::str("PB_DefaultRampCornerInverted");
pub const PB_DEFAULT_RAMP_CORNER: BString = BString::str("PB_DefaultRampCorner");
pub const PB_DEFAULT_POLE: BString = BString::str("PB_DefaultPole");
pub const PB_DEFAULT_MICRO_WEDGE_TRIANGLE_CORNER: BString =
    BString::str("PB_DefaultMicroWedgeTriangleCorner");
pub const PB_DEFAULT_MICRO_WEDGE_OUTER_CORNER: BString =
    BString::str("PB_DefaultMicroWedgeOuterCorner");
pub const PB_DEFAULT_MICRO_WEDGE_INNER_CORNER: BString =
    BString::str("PB_DefaultMicroWedgeInnerCorner");
pub const PB_DEFAULT_MICRO_WEDGE_HALF_OUTER_CORNER: BString =
    BString::str("PB_DefaultMicroWedgeHalfOuterCorner");
pub const PB_DEFAULT_MICRO_WEDGE_HALF_INNER_CORNER_INVERTED: BString =
    BString::str("PB_DefaultMicroWedgeHalfInnerCornerInverted");
pub const PB_DEFAULT_MICRO_WEDGE_HALF_INNER_CORNER: BString =
    BString::str("PB_DefaultMicroWedgeHalfInnerCorner");
pub const PB_DEFAULT_MICRO_WEDGE_CORNER: BString = BString::str("PB_DefaultMicroWedgeCorner");
pub const PB_DEFAULT_MICRO_WEDGE: BString = BString::str("PB_DefaultMicroWedge");
pub const PB_DEFAULT_MICRO_RAMP: BString = BString::str("PB_DefaultMicroRamp");
pub const PB_DEFAULT_MICRO_BRICK: BString = BString::str("PB_DefaultMicroBrick");
pub const PB_DEFAULT_ARCH: BString = BString::str("PB_DefaultArch");
pub const BP_SQUARE_PLATE: BString = BString::str("BP_SquarePlate");
pub const BP_SPIKE_PLATE: BString = BString::str("BP_SpikePlate");
pub const PB_SPIKE: BString = BString::str("PB_Spike");
pub const PB_SLIDER_JOINT: BString = BString::str("PB_SliderJoint");
pub const PB_SERVO_SLIDER_JOINT: BString = BString::str("PB_ServoSliderJoint");
pub const PB_MOTOR_SLIDER_JOINT: BString = BString::str("PB_MotorSliderJoint");
pub const BP_ROUND_PLATE: BString = BString::str("BP_RoundPlate");
pub const PB_ROUNDED_CAP: BString = BString::str("PB_RoundedCap");
pub const PB_BAGUETTE: BString = BString::str("PB_Baguette");
