// Synthetic / internal
pub const LITERAL: &str = "_Literal";
pub const UNSUPPORTED: &str = "_Unsupported";

// Infrastructure
pub const MICROCHIP: &str = "BrickComponentType_Internal_Microchip";
pub const MICROCHIP_ALT: &str = "Component_Internal_Microchip";
pub const MICROCHIP_INPUT: &str = "BrickComponentType_Internal_MicrochipInput";
pub const MICROCHIP_OUTPUT: &str = "BrickComponentType_Internal_MicrochipOutput";
pub const REROUTER: &str = "Component_Internal_Rerouter";
pub const INPUT_SPLITTER: &str = "Component_Internal_InputSplitter";

// Variables
pub const PSEUDO_VAR: &str = "BrickComponentType_WireGraphPseudo_Var";
pub const PSEUDO_ARRAY_VAR: &str = "BrickComponentType_WireGraphPseudo_ArrayVar";
pub const BUFFER_TICKS: &str = "BrickComponentType_WireGraphPseudo_BufferTicks";
pub const BUFFER_SECONDS: &str = "BrickComponentType_WireGraphPseudo_BufferSeconds";
pub const VARIABLE: &str = "BrickComponentType_Internal_Variable";
pub const BUFFER: &str = "BrickComponentType_Wires_Buffer";
pub const VAR_GET: &str = "BrickComponentType_WireGraph_Exec_Var_Get";
pub const VAR_SET: &str = "BrickComponentType_WireGraph_Exec_Var_Set";
pub const VAR_INCREMENT: &str = "BrickComponentType_WireGraph_Exec_Var_Increment";

// Array variables
pub const ARRAY_SET_AT_INDEX: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_SetAtIndex";
pub const ARRAY_GET: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Get";
pub const ARRAY_PUSH: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Push";
pub const ARRAY_POP: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Pop";
pub const ARRAY_GET_LENGTH: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_GetLength";
pub const ARRAY_REMOVE_AT_INDEX: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_RemoveAtIndex";
pub const ARRAY_CLEAR: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Clear";
pub const ARRAY_SHUFFLE: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Shuffle";
pub const ARRAY_INSERT: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Insert";
pub const ARRAY_FIND: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Find";
pub const ARRAY_SORT: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Sort";
pub const ARRAY_REVERSE: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Reverse";
pub const ARRAY_SUM: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Sum";
pub const ARRAY_MIN: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Min";
pub const ARRAY_MAX: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Max";
pub const ARRAY_AVERAGE: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Average";
pub const ARRAY_SWAP: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Swap";
pub const ARRAY_FILL: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Fill";
pub const ARRAY_RESIZE: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Resize";
pub const ARRAY_SLICE: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Slice";
pub const ARRAY_APPEND: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_Append";
pub const ARRAY_COPY_FROM: &str = "BrickComponentType_WireGraph_Exec_ArrayVar_CopyFrom";

// Exec flow
pub const UNION: &str = "BrickComponentType_WireGraph_Exec_Union";
pub const BRANCH: &str = "BrickComponentType_WireGraph_Exec_Branch";
pub const RANDOM: &str = "BrickComponentType_WireGraph_Exec_Random";

// Select / Swap
pub const SELECT: &str = "BrickComponentType_WireGraph_Expr_Select";
pub const SWAP: &str = "BrickComponentType_WireGraph_Expr_Swap";

// Logic
pub const LOGICAL_NOT: &str = "BrickComponentType_WireGraph_Expr_LogicalNOT";
pub const BITWISE_NOT: &str = "BrickComponentType_WireGraph_Expr_BitwiseNOT";
pub const BITWISE_NAND: &str = "BrickComponentType_WireGraph_Expr_BitwiseNAND";
pub const BITWISE_NOR: &str = "BrickComponentType_WireGraph_Expr_BitwiseNOR";
pub const BITWISE_BIT_COUNT: &str = "BrickComponentType_WireGraph_Expr_BitwiseBitCount";

// Compare
pub const COMPARE_EQUAL: &str = "BrickComponentType_WireGraph_Expr_CompareEqual";
pub const COMPARE_NOT_EQUAL: &str = "BrickComponentType_WireGraph_Expr_CompareNotEqual";

// Math (unary)
pub const MATH_SIN: &str = "BrickComponentType_WireGraph_Expr_MathSin";
pub const MATH_COS: &str = "BrickComponentType_WireGraph_Expr_MathCos";
pub const MATH_TAN: &str = "BrickComponentType_WireGraph_Expr_MathTan";
pub const MATH_ASIN: &str = "BrickComponentType_WireGraph_Expr_MathAsin";
pub const MATH_ACOS: &str = "BrickComponentType_WireGraph_Expr_MathAcos";
pub const MATH_ATAN: &str = "BrickComponentType_WireGraph_Expr_MathAtan";
pub const MATH_SINH: &str = "BrickComponentType_WireGraph_Expr_MathSinh";
pub const MATH_COSH: &str = "BrickComponentType_WireGraph_Expr_MathCosh";
pub const MATH_TANH: &str = "BrickComponentType_WireGraph_Expr_MathTanh";
pub const MATH_ASINH: &str = "BrickComponentType_WireGraph_Expr_MathAsinh";
pub const MATH_ACOSH: &str = "BrickComponentType_WireGraph_Expr_MathAcosh";
pub const MATH_ATANH: &str = "BrickComponentType_WireGraph_Expr_MathAtanh";
pub const MATH_EXP: &str = "BrickComponentType_WireGraph_Expr_MathExp";
pub const MATH_LN: &str = "BrickComponentType_WireGraph_Expr_MathLn";
pub const MATH_SIGN: &str = "BrickComponentType_WireGraph_Expr_MathSign";
pub const MATH_ABS: &str = "BrickComponentType_WireGraph_Expr_MathAbs";
pub const MATH_SQRT: &str = "BrickComponentType_WireGraph_Expr_MathSqrt";
pub const MATH_NEGATE: &str = "BrickComponentType_WireGraph_Expr_MathNegate";
pub const ROUND: &str = "BrickComponentType_WireGraph_Expr_Round";
pub const FLOOR: &str = "BrickComponentType_WireGraph_Expr_Floor";
pub const CEIL: &str = "BrickComponentType_WireGraph_Expr_Ceil";
pub const DEG2RAD: &str = "BrickComponentType_WireGraph_Expr_MathDegreesToRadians";
pub const RAD2DEG: &str = "BrickComponentType_WireGraph_Expr_MathRadiansToDegrees";

// Math (binary+)
pub const MATH_MIN: &str = "BrickComponentType_WireGraph_Expr_MathMin";
pub const MATH_MAX: &str = "BrickComponentType_WireGraph_Expr_MathMax";
pub const MATH_POW: &str = "BrickComponentType_WireGraph_Expr_MathPow";
pub const MATH_CLAMP: &str = "BrickComponentType_WireGraph_Expr_MathClamp";
pub const MATH_ATAN2: &str = "BrickComponentType_WireGraph_Expr_MathAtan2";
pub const MATH_LOG_BASE: &str = "BrickComponentType_WireGraph_Expr_MathLogBase";
pub const MATH_BLEND: &str = "BrickComponentType_WireGraph_Expr_MathBlend";
pub const MATH_MOD_FLOORED: &str = "BrickComponentType_WireGraph_Expr_MathModuloFloored";
pub const EDGE_DETECTOR: &str = "BrickComponentType_WireGraph_Expr_EdgeDetector";

// String ops
pub const STRING_LENGTH: &str = "BrickComponentType_WireGraph_Expr_String_Length";
pub const STRING_CONTAINS: &str = "BrickComponentType_WireGraph_Expr_String_Contains";
pub const STRING_CONCATENATE: &str = "BrickComponentType_WireGraph_Expr_String_Concatenate";
pub const STRING_FORMAT_TEXT: &str = "BrickComponentType_WireGraph_Expr_String_FormatText";
pub const STRING_STARTS_WITH: &str = "BrickComponentType_WireGraph_Expr_String_StartsWith";
pub const STRING_ENDS_WITH: &str = "BrickComponentType_WireGraph_Expr_String_EndsWith";
pub const STRING_SUBSTRING: &str = "BrickComponentType_WireGraph_Expr_String_Substring";
pub const STRING_REPLACE: &str = "BrickComponentType_WireGraph_Expr_String_Replace";
pub const STRING_FIND: &str = "BrickComponentType_WireGraph_Expr_String_Find";
pub const STRING_SPLIT: &str = "BrickComponentType_WireGraph_Expr_String_Split";
pub const STRING_TO_LOWER: &str = "BrickComponentType_WireGraph_Expr_String_ToLower";
pub const STRING_TO_UPPER: &str = "BrickComponentType_WireGraph_Expr_String_ToUpper";
pub const STRING_TRIM: &str = "BrickComponentType_WireGraph_Expr_String_Trim";
pub const STRING_PARSE_INT: &str = "BrickComponentType_WireGraph_Expr_String_ParseInt";
pub const STRING_PARSE_NUMBER: &str = "BrickComponentType_WireGraph_Expr_String_ParseNumber";

// Color / Vector
pub const MAKE_COLOR: &str = "BrickComponentType_WireGraph_Expr_MakeColor";
pub const SPLIT_COLOR: &str = "BrickComponentType_WireGraph_Expr_SplitColor";
pub const MAKE_VECTOR: &str = "BrickComponentType_WireGraph_Expr_MakeVector";
pub const SPLIT_VECTOR: &str = "BrickComponentType_WireGraph_Expr_SplitVector";
pub const VEC_DOT: &str = "BrickComponentType_WireGraph_Expr_VecDotProduct";
pub const VEC_CROSS: &str = "BrickComponentType_WireGraph_Expr_VecCrossProduct";
pub const VEC_NORMALIZE: &str = "BrickComponentType_WireGraph_Expr_VecNormalize";
pub const VEC_MAGNITUDE: &str = "BrickComponentType_WireGraph_Expr_VecMagnitude";
pub const VEC_MAGNITUDE_SQ: &str = "BrickComponentType_WireGraph_Expr_VecMagnitudeSquared";
pub const VEC_DISTANCE: &str = "BrickComponentType_WireGraph_Expr_VecDistance";
pub const VEC_DISTANCE_SQ: &str = "BrickComponentType_WireGraph_Expr_VecDistanceSquared";
pub const VEC_SCALE: &str = "BrickComponentType_WireGraph_Expr_VecScale";
pub const VEC_ROT_TO_DIR: &str = "BrickComponentType_WireGraph_Expr_VecRotationToDirection";

// Rotation / quaternion (cl14428+)
pub const MAKE_ROTATION: &str = "BrickComponentType_WireGraph_Expr_MakeRotation";
pub const SPLIT_ROTATION: &str = "BrickComponentType_WireGraph_Expr_SplitRotation";
pub const ROTATE_VECTOR: &str = "BrickComponentType_WireGraph_Expr_RotateVector";
pub const INVERT_ROTATION: &str = "BrickComponentType_WireGraph_Expr_InvertRotation";
pub const DIRECTION_TO_ROTATION: &str = "BrickComponentType_WireGraph_Expr_DirectionToRotation";
pub const ROTATION_TO_DIRECTION: &str = "BrickComponentType_WireGraph_Expr_RotationToDirection";
pub const QUAT_BETWEEN: &str = "BrickComponentType_WireGraph_Expr_QuatBetween";
pub const QUAT_ANGLE_BETWEEN: &str = "BrickComponentType_WireGraph_Expr_QuatAngleBetween";
pub const QUAT_FROM_AXIS_ANGLE: &str = "BrickComponentType_WireGraph_Expr_QuatFromAxisAngle";
pub const QUAT_TO_AXIS_ANGLE: &str = "BrickComponentType_WireGraph_Expr_QuatToAxisAngle";
pub const QUAT_SLERP: &str = "BrickComponentType_WireGraph_Expr_QuatSlerp";

// sRGB / hex color (cl14428+)
pub const MAKE_COLOR_SRGB: &str = "BrickComponentType_WireGraph_Expr_MakeColorSRGB";
pub const SPLIT_COLOR_SRGB: &str = "BrickComponentType_WireGraph_Expr_SplitColorSRGB";
pub const MAKE_COLOR_HEX: &str = "BrickComponentType_WireGraph_Expr_MakeColorHex";
pub const COLOR_TO_HEX: &str = "BrickComponentType_WireGraph_Expr_ColorToHex";
pub const COLOR_BLEND: &str = "BrickComponentType_WireGraph_Expr_ColorBlend";

// Controller role check (cl14428+)
pub const CONTROLLER_HAS_ROLE: &str = "BrickComponentType_WireGraph_Exec_Controller_HasRole";

// Character inventory (cl14428+)
pub const CHARACTER_SET_INVENTORY_ENTRY: &str =
    "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntry";

// Stateful exec value gates (cl14428+)
pub const EXEC_CYCLE: &str = "BrickComponentType_WireGraph_Exec_Cycle";
pub const EXEC_TOGGLE: &str = "BrickComponentType_WireGraph_Exec_Toggle";

// Misc value / exec gates
pub const PRINT_TO_CONSOLE: &str = "BrickComponentType_WireGraph_Exec_PrintToConsole";
pub const DELTA_TIME: &str = "BrickComponentType_WireGraph_DeltaTime";
pub const SERVER_UPTIME: &str = "BrickComponentType_WireGraph_ServerUptime";
pub const NEARLY_EQUAL: &str = "BrickComponentType_WireGraph_Expr_NearlyEqual";
pub const PSEUDO_DAMPEN: &str = "BrickComponentType_WireGraphPseudo_Dampen";
pub const PSEUDO_TWEEN: &str = "BrickComponentType_WireGraphPseudo_Tween";
pub const PSEUDO_TIMER: &str = "BrickComponentType_WireGraphPseudo_Timer";
pub const MATH_EASING: &str = "BrickComponentType_WireGraph_Expr_MathEasing";
pub const GAMEMODE_FILL_FROM_PLAYERS: &str = "BrickComponentType_WireGraph_Exec_Gamemode_FillArrayFromPlayers";
pub const GAMEMODE_FILL_FROM_TEAM: &str = "BrickComponentType_WireGraph_Exec_Gamemode_FillArrayFromTeamMembers";
pub const GAMEMODE_INC_TEAM_LEADERBOARD: &str = "BrickComponentType_WireGraph_Exec_Gamemode_IncrementTeamLeaderboardValue";

// Controller / Character / Entity
pub const CONTROLLER_DISPLAY_TEXT: &str = "BrickComponentType_WireGraph_Exec_Controller_DisplayText";
pub const CONTROLLER_SHOW_STATUS: &str = "BrickComponentType_WireGraph_Exec_Controller_ShowStatusMessage";
pub const CONTROLLER_GET_FROM_ENTITY: &str = "BrickComponentType_WireGraph_Exec_Controller_GetFromEntity";
pub const CONTROLLER_GET_USER_NAME: &str = "BrickComponentType_WireGraph_Exec_Controller_GetUserName";
pub const CONTROLLER_GET_USER_ID: &str = "BrickComponentType_WireGraph_Exec_Controller_GetUserId";
pub const CONTROLLER_GET_DISPLAY_NAME: &str = "BrickComponentType_WireGraph_Exec_Controller_GetDisplayName";
pub const CONTROLLER_HAS_PERMISSION: &str = "BrickComponentType_WireGraph_Exec_Controller_HasPermission";
pub const CONTROLLER_IS_TRUSTED: &str = "BrickComponentType_WireGraph_Exec_Controller_IsTrustedByBrickOwner";
pub const CONTROLLER_SET_CAN_RESPAWN: &str = "BrickComponentType_WireGraph_Exec_Controller_SetCanRespawn";
pub const CHARACTER_GET_FROM_CONTROLLER: &str = "BrickComponentType_WireGraph_Exec_Character_GetFromController";
pub const CHARACTER_GET_AIM: &str = "BrickComponentType_WireGraph_Exec_Character_GetAim";
pub const CHARACTER_SHOW_HINT: &str = "BrickComponentType_WireGraph_Exec_Character_ShowHint";
pub const CHARACTER_GET_DAMAGE: &str = "BrickComponentType_WireGraph_Exec_Character_GetDamage";
pub const CHARACTER_SET_DAMAGE: &str = "BrickComponentType_WireGraph_Exec_Character_SetDamage";
pub const CHARACTER_INC_DAMAGE: &str = "BrickComponentType_WireGraph_Exec_Character_IncDamage";
pub const CHARACTER_SET_TEMP_PERMISSION: &str = "BrickComponentType_WireGraph_Exec_Character_SetTempPermission";
pub const ENTITY_GET_LOCATION: &str = "BrickComponentType_WireGraph_Exec_Entity_GetLocation";
pub const ENTITY_GET_ROTATION: &str = "BrickComponentType_WireGraph_Exec_Entity_GetRotation";
pub const ENTITY_GET_LOCATION_ROTATION: &str = "BrickComponentType_WireGraph_Exec_Entity_GetLocationRotation";
pub const ENTITY_GET_LINEAR_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_GetLinearVelocity";
pub const ENTITY_GET_ANGULAR_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_GetAngularVelocity";
pub const ENTITY_GET_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_GetVelocity";
pub const ENTITY_SET_LOCATION: &str = "BrickComponentType_WireGraph_Exec_Entity_SetLocation";
pub const ENTITY_SET_ROTATION: &str = "BrickComponentType_WireGraph_Exec_Entity_SetRotation";
pub const ENTITY_SET_LOCATION_ROTATION: &str = "BrickComponentType_WireGraph_Exec_Entity_SetLocationRotation";
pub const ENTITY_ADD_LOCATION_ROTATION: &str = "BrickComponentType_WireGraph_Exec_Entity_AddLocationRotation";
pub const ENTITY_TELEPORT: &str = "BrickComponentType_WireGraph_Exec_Entity_Teleport";
pub const ENTITY_RELATIVE_TELEPORT: &str = "BrickComponentType_WireGraph_Exec_Entity_RelativeTeleport";
pub const ENTITY_SET_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_SetVelocity";
pub const ENTITY_ADD_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_AddVelocity";
pub const ENTITY_SET_LINEAR_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_SetLinearVelocity";
pub const ENTITY_SET_ANGULAR_VELOCITY: &str = "BrickComponentType_WireGraph_Exec_Entity_SetAngularVelocity";
pub const ENTITY_SET_GRAVITY_DIRECTION: &str = "BrickComponentType_WireGraph_Exec_Entity_SetGravityDirection";
pub const ENTITY_SET_FROZEN: &str = "BrickComponentType_WireGraph_Exec_Entity_SetFrozen";

// Gamemode
pub const GAMEMODE_GET_TEAM_BY_NAME: &str = "BrickComponentType_WireGraph_Exec_Gamemode_GetTeamByName";
pub const GAMEMODE_SET_LEADERBOARD: &str = "BrickComponentType_WireGraph_Exec_Gamemode_SetLeaderboardValue";
pub const GAMEMODE_INC_LEADERBOARD: &str = "BrickComponentType_WireGraph_Exec_Gamemode_IncrementLeaderboardValue";
pub const GAMEMODE_GET_LEADERBOARD: &str = "BrickComponentType_WireGraph_Exec_Gamemode_GetLeaderboardValue";
pub const GAMEMODE_GET_TEAM: &str = "BrickComponentType_WireGraph_Exec_Gamemode_GetTeam";
pub const GAMEMODE_GET_CURRENT_ROUND: &str = "BrickComponentType_WireGraph_Exec_Gamemode_GetCurrentRound";
pub const GAMEMODE_GET_TEAM_NAME: &str = "BrickComponentType_WireGraph_Exec_Gamemode_GetTeamName";
pub const GAMEMODE_SET_TEAM: &str = "BrickComponentType_WireGraph_Exec_Gamemode_SetTeam";
pub const GAMEMODE_SET_TEAM_PINNED: &str = "BrickComponentType_WireGraph_Exec_Gamemode_SetTeamPinned";
pub const GAMEMODE_PLAYER_WINS: &str = "BrickComponentType_WireGraph_Exec_Gamemode_PlayerWins";
pub const GAMEMODE_TEAM_WINS: &str = "BrickComponentType_WireGraph_Exec_Gamemode_TeamWins";
pub const GAMEMODE_GET_TEAM_LEADERBOARD: &str = "BrickComponentType_WireGraph_Exec_Gamemode_GetTeamLeaderboardValue";
pub const GAMEMODE_SET_TEAM_LEADERBOARD: &str = "BrickComponentType_WireGraph_Exec_Gamemode_SetTeamLeaderboardValue";

// Prefab / Sweep
pub const PREFAB_SPAWNER: &str = "BrickComponentType_WireGraph_Exec_PrefabSpawner";
pub const SWEEP: &str = "BrickComponentType_WireGraph_Exec_Sweep";

// Messaging
pub const CONTROLLER_SHOW_CHAT: &str =
    "BrickComponentType_WireGraph_Exec_Controller_ShowChatMessage";
pub const CONTROLLER_SHOW_MESSAGE_BOX: &str =
    "BrickComponentType_WireGraph_Exec_Controller_ShowMessageBox";
pub const GAMEMODE_BROADCAST_CHAT: &str =
    "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage";
pub const GAMEMODE_BROADCAST_STATUS: &str =
    "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastStatusMessage";

// Audio
pub const PLAY_AUDIO_AT: &str = "Component_WireGraph_PlayAudioAt";
pub const PLAY_GLOBAL_AUDIO: &str = "BrickComponentType_WireGraph_Exec_PlayGlobalAudio";

// Entity tags
pub const ENTITY_GET_TAG: &str = "BrickComponentType_WireGraph_Exec_Entity_GetTag";
pub const ENTITY_SET_TAG: &str = "BrickComponentType_WireGraph_Exec_Entity_SetTag";

// Player lookup (pure value gate)
pub const FIND_PLAYER: &str = "BrickComponentType_WireGraph_FindPlayer";

// Change detector
pub const CHANGE_DETECTOR: &str = "BrickComponentType_WireGraph_Expr_ChangeDetector";

// Quaternion make/split/dot
pub const MAKE_QUATERNION: &str = "BrickComponentType_WireGraph_Expr_MakeQuaternion";
pub const SPLIT_QUATERNION: &str = "BrickComponentType_WireGraph_Expr_SplitQuaternion";
pub const QUAT_DOT_PRODUCT: &str = "BrickComponentType_WireGraph_Expr_QuatDotProduct";

// Character inventory family
pub const CHARACTER_ADD_INVENTORY_ITEM: &str =
    "BrickComponentType_WireGraph_Exec_Character_AddInventoryItem";
pub const CHARACTER_SET_INVENTORY_ITEM: &str =
    "BrickComponentType_WireGraph_Exec_Character_SetInventoryItem";
pub const CHARACTER_ADD_INVENTORY_BRICK: &str =
    "BrickComponentType_WireGraph_Exec_Character_AddInventoryBrick";
pub const CHARACTER_SET_INVENTORY_BRICK: &str =
    "BrickComponentType_WireGraph_Exec_Character_SetInventoryBrick";
pub const CHARACTER_ADD_INVENTORY_ENTITY: &str =
    "BrickComponentType_WireGraph_Exec_Character_AddInventoryEntity";
pub const CHARACTER_SET_INVENTORY_ENTITY: &str =
    "BrickComponentType_WireGraph_Exec_Character_SetInventoryEntity";
pub const CHARACTER_ADD_INVENTORY_ITEM_ADV: &str =
    "BrickComponentType_WireGraph_Exec_Character_AddInventoryItemAdv";
pub const CHARACTER_SET_INVENTORY_ITEM_ADV: &str =
    "BrickComponentType_WireGraph_Exec_Character_SetInventoryItemAdv";
