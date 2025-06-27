use crate::brdb::wrapper::{BString, BrickType};

pub const PB_DEFAULT_BRICK: BString = BString::str("PB_DefaultBrick");
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

pub const B_GATE_CONSTANT_BOOL: BrickType = BrickType::str("B_1x1_Gate_Constant_Bool");
pub const B_GATE_CONSTANT_INT: BrickType = BrickType::str("B_1x1_Gate_Constant_Integer");
pub const B_GATE_CONSTANT_FLOAT: BrickType = BrickType::str("B_1x1_Gate_Constant_Float");
pub const B_GATE_CONSTANT_STRING: BrickType = BrickType::str("B_1x1_Gate_Constant_String");
