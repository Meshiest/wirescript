use std::sync::Arc;

use crate::brdb::{
    assets,
    schema::{BrdbSchema, BrdbSchemaMeta, as_brdb::AsBrdbValue},
    wrapper::{BString, BrdbComponent, BrickType, WirePort},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogicGate {
    BoolAnd,
    BoolOr,
    BoolXor,
    BoolNand,
    BoolNor,
    BoolNot,

    BitAnd,
    BitOr,
    BitXor,
    BitNand,
    BitNor,
    BitNot,
    BitShiftLeft,
    BitShiftRight,

    FloatAdd,
    FloatSub,
    FloatMul,
    FloatModFloored,
    FloatMod,
    FloatDiv,

    FloatCeil,
    FloatFloor,

    FloatEq,
    FloatNeq,
    FloatLt,
    FloatLe,
    FloatGt,
    FloatGe,

    ConstBool,
    ConstInt,
    ConstFloat,
    ConstString,
}

impl LogicGate {
    pub const COMPONENT_BOOL_AND: BString = BString::str("Component_GateAndBool");
    pub const COMPONENT_BOOL_OR: BString = BString::str("Component_GateOrBool");
    pub const COMPONENT_BOOL_XOR: BString = BString::str("Component_GateXorBool");
    pub const COMPONENT_BOOL_NAND: BString = BString::str("Component_GateNandBool");
    pub const COMPONENT_BOOL_NOR: BString = BString::str("Component_GateNorBool");
    pub const COMPONENT_BOOL_NOT: BString = BString::str("Component_GateNotBool");

    pub const COMPONENT_BIT_AND: BString = BString::str("Component_GateAndInt");
    pub const COMPONENT_BIT_OR: BString = BString::str("Component_GateOrInt");
    pub const COMPONENT_BIT_XOR: BString = BString::str("Component_GateXorInt");
    pub const COMPONENT_BIT_NAND: BString = BString::str("Component_GateNandInt");
    pub const COMPONENT_BIT_NOR: BString = BString::str("Component_GateNorInt");
    pub const COMPONENT_BIT_NOT: BString = BString::str("Component_GateNotInt");
    pub const COMPONENT_BIT_SHIFT_LEFT: BString = BString::str("Component_GateShiftLeftInt");
    pub const COMPONENT_BIT_SHIFT_RIGHT: BString = BString::str("Component_GateShiftRightInt");

    pub const COMPONENT_FLOAT_SUB: BString = BString::str("Component_GateSubtractFloat");
    pub const COMPONENT_FLOAT_MUL: BString = BString::str("Component_GateMultiplyFloat");
    pub const COMPONENT_FLOAT_MOD_FLOORED: BString = BString::str("Component_GateModFloatFloored");
    pub const COMPONENT_FLOAT_MOD: BString = BString::str("Component_GateModFloat");
    pub const COMPONENT_FLOAT_DIV: BString = BString::str("Component_GateDivideFloat");
    pub const COMPONENT_FLOAT_ADD: BString = BString::str("Component_GateAddFloat");
    pub const COMPONENT_FLOAT_CEIL: BString = BString::str("Component_GateCeilFloat");
    pub const COMPONENT_FLOAT_FLOOR: BString = BString::str("Component_GateFloorFloat");

    pub const COMPONENT_FLOAT_EQ: BString = BString::str("Component_GateEqualsFloat");
    pub const COMPONENT_FLOAT_NEQ: BString = BString::str("Component_GateNotEqualsFloat");
    pub const COMPONENT_FLOAT_LT: BString = BString::str("Component_GateLessThanFloat");
    pub const COMPONENT_FLOAT_LE: BString = BString::str("Component_GateLessThanOrEqualsFloat");
    pub const COMPONENT_FLOAT_GT: BString = BString::str("Component_GateGreaterThanFloat");
    pub const COMPONENT_FLOAT_GE: BString = BString::str("Component_GateGreaterThanOrEqualsFloat");

    pub const COMPONENT_CONST_BOOL: BString = BString::str("Component_ConstantBool");
    pub const COMPONENT_CONST_INT: BString = BString::str("Component_ConstantInt");
    pub const COMPONENT_CONST_FLOAT: BString = BString::str("Component_ConstantFloat");
    pub const COMPONENT_CONST_STRING: BString = BString::str("Component_ConstantString");

    pub const STRUCT_BINARY_BOOL_BOOL_STR: &str = "BrickComponentData_GateBinary_BoolBool";
    pub const STRUCT_BINARY_FLOAT_BOOL_STR: &str = "BrickComponentData_GateBinary_FloatBool";
    pub const STRUCT_BINARY_FLOAT_FLOAT_STR: &str = "BrickComponentData_GateBinary_FloatFloat";
    pub const STRUCT_BINARY_INT_INT_STR: &str = "BrickComponentData_GateBinary_IntInt";

    pub const STRUCT_UNARY_BOOL_BOOL_STR: &str = "BrickComponentData_GateUnary_BoolBool";
    pub const STRUCT_UNARY_FLOAT_FLOAT_STR: &str = "BrickComponentData_GateUnary_FloatFloat";
    pub const STRUCT_UNARY_INT_INT_STR: &str = "BrickComponentData_GateUnary_IntInt";

    pub const STRUCT_CONSTANT_BOOL_STR: &str = "BrickComponentData_ConstantBool";
    pub const STRUCT_CONSTANT_FLOAT_STR: &str = "BrickComponentData_ConstantFloat";
    pub const STRUCT_CONSTANT_INT_STR: &str = "BrickComponentData_ConstantInt";
    pub const STRUCT_CONSTANT_STRING_STR: &str = "BrickComponentData_ConstantString";

    pub const STRUCT_BINARY_BOOL_BOOL: BString = BString::str(Self::STRUCT_BINARY_BOOL_BOOL_STR);
    pub const STRUCT_BINARY_FLOAT_BOOL: BString = BString::str(Self::STRUCT_BINARY_FLOAT_BOOL_STR);
    pub const STRUCT_BINARY_FLOAT_FLOAT: BString =
        BString::str(Self::STRUCT_BINARY_FLOAT_FLOAT_STR);
    pub const STRUCT_BINARY_INT_INT: BString = BString::str(Self::STRUCT_BINARY_INT_INT_STR);

    pub const STRUCT_UNARY_BOOL_BOOL: BString = BString::str(Self::STRUCT_UNARY_BOOL_BOOL_STR);
    pub const STRUCT_UNARY_FLOAT_FLOAT: BString = BString::str(Self::STRUCT_UNARY_FLOAT_FLOAT_STR);
    pub const STRUCT_UNARY_INT_INT: BString = BString::str(Self::STRUCT_UNARY_INT_INT_STR);

    pub const STRUCT_CONSTANT_BOOL: BString = BString::str(Self::STRUCT_CONSTANT_BOOL_STR);
    pub const STRUCT_CONSTANT_FLOAT: BString = BString::str(Self::STRUCT_CONSTANT_FLOAT_STR);
    pub const STRUCT_CONSTANT_INT: BString = BString::str(Self::STRUCT_CONSTANT_INT_STR);
    pub const STRUCT_CONSTANT_STRING: BString = BString::str(Self::STRUCT_CONSTANT_STRING_STR);

    pub const BOOL_INPUT: BString = BString::str("bInput");
    pub const BOOL_INPUT_A: BString = BString::str("bInputA");
    pub const BOOL_INPUT_B: BString = BString::str("bInputB");
    pub const BOOL_OUTPUT: BString = BString::str("bOutput");
    pub const INPUT: BString = BString::str("Input");
    pub const INPUT_A: BString = BString::str("InputA");
    pub const INPUT_B: BString = BString::str("InputB");
    pub const OUTPUT: BString = BString::str("Output");

    pub fn component_name(&self) -> BString {
        match self {
            Self::BoolAnd => Self::COMPONENT_BOOL_AND,
            Self::BoolOr => Self::COMPONENT_BOOL_OR,
            Self::BoolXor => Self::COMPONENT_BOOL_XOR,
            Self::BoolNand => Self::COMPONENT_BOOL_NAND,
            Self::BoolNor => Self::COMPONENT_BOOL_NOR,
            Self::BoolNot => Self::COMPONENT_BOOL_NOT,

            Self::BitAnd => Self::COMPONENT_BIT_AND,
            Self::BitOr => Self::COMPONENT_BIT_OR,
            Self::BitXor => Self::COMPONENT_BIT_XOR,
            Self::BitNand => Self::COMPONENT_BIT_NAND,
            Self::BitNor => Self::COMPONENT_BIT_NOR,
            Self::BitNot => Self::COMPONENT_BIT_NOT,
            Self::BitShiftLeft => Self::COMPONENT_BIT_SHIFT_LEFT,
            Self::BitShiftRight => Self::COMPONENT_BIT_SHIFT_RIGHT,

            Self::FloatSub => Self::COMPONENT_FLOAT_SUB,
            Self::FloatMul => Self::COMPONENT_FLOAT_MUL,
            Self::FloatModFloored => Self::COMPONENT_FLOAT_MOD_FLOORED,
            Self::FloatMod => Self::COMPONENT_FLOAT_MOD,
            Self::FloatDiv => Self::COMPONENT_FLOAT_DIV,
            Self::FloatAdd => Self::COMPONENT_FLOAT_ADD,

            Self::FloatCeil => Self::COMPONENT_FLOAT_CEIL,
            Self::FloatFloor => Self::COMPONENT_FLOAT_FLOOR,

            Self::FloatEq => Self::COMPONENT_FLOAT_EQ,
            Self::FloatNeq => Self::COMPONENT_FLOAT_NEQ,
            Self::FloatLt => Self::COMPONENT_FLOAT_LT,
            Self::FloatLe => Self::COMPONENT_FLOAT_LE,
            Self::FloatGt => Self::COMPONENT_FLOAT_GT,
            Self::FloatGe => Self::COMPONENT_FLOAT_GE,

            Self::ConstBool => Self::COMPONENT_CONST_BOOL,
            Self::ConstInt => Self::COMPONENT_CONST_INT,
            Self::ConstFloat => Self::COMPONENT_CONST_FLOAT,
            Self::ConstString => Self::COMPONENT_CONST_STRING,
        }
    }

    pub fn is_bool_input(&self) -> bool {
        matches!(
            self,
            Self::BoolAnd
                | Self::BoolOr
                | Self::BoolXor
                | Self::BoolNand
                | Self::BoolNor
                | Self::BoolNot
        )
    }

    pub fn is_bool_output(&self) -> bool {
        matches!(
            self,
            Self::BoolAnd
                | Self::BoolOr
                | Self::BoolXor
                | Self::BoolNand
                | Self::BoolNor
                | Self::BoolNot
        )
    }

    pub fn struct_name(&self) -> BString {
        match self {
            Self::BoolAnd | Self::BoolOr | Self::BoolXor | Self::BoolNand | Self::BoolNor => {
                Self::STRUCT_BINARY_BOOL_BOOL
            }
            Self::BoolNot => Self::STRUCT_UNARY_BOOL_BOOL,

            Self::BitAnd | Self::BitOr | Self::BitXor | Self::BitNand | Self::BitNor => {
                Self::STRUCT_BINARY_INT_INT
            }
            Self::BitNot => Self::STRUCT_UNARY_INT_INT,
            Self::BitShiftLeft | Self::BitShiftRight => Self::STRUCT_BINARY_INT_INT,

            Self::FloatSub
            | Self::FloatMul
            | Self::FloatModFloored
            | Self::FloatMod
            | Self::FloatDiv
            | Self::FloatAdd => Self::STRUCT_BINARY_FLOAT_FLOAT,
            Self::FloatCeil | Self::FloatFloor => Self::STRUCT_UNARY_FLOAT_FLOAT,

            Self::FloatEq
            | Self::FloatNeq
            | Self::FloatLt
            | Self::FloatLe
            | Self::FloatGt
            | Self::FloatGe => Self::STRUCT_BINARY_FLOAT_BOOL,

            Self::ConstBool => Self::STRUCT_CONSTANT_BOOL,
            Self::ConstInt => Self::STRUCT_CONSTANT_INT,
            Self::ConstFloat => Self::STRUCT_CONSTANT_FLOAT,
            Self::ConstString => Self::STRUCT_CONSTANT_STRING,
        }
    }

    pub fn schema(&self) -> BrdbSchemaMeta {
        let schema_str = match self.struct_name().as_ref() {
            Self::STRUCT_BINARY_BOOL_BOOL_STR => {
                "struct BrickComponentData_GateBinary_BoolBool { bInputA: bool, bInputB: bool, bOutput: bool }"
            }
            Self::STRUCT_BINARY_FLOAT_BOOL_STR => {
                "struct BrickComponentData_GateBinary_FloatBool { InputA: f64, InputB: f64, bOutput: bool }"
            }
            Self::STRUCT_BINARY_FLOAT_FLOAT_STR => {
                "struct BrickComponentData_GateBinary_FloatFloat { InputA: f64, InputB: f64, Output: f64 }"
            }
            Self::STRUCT_BINARY_INT_INT_STR => {
                "struct BrickComponentData_GateBinary_IntInt { InputA: i64, InputB: i64, Output: i64 }"
            }
            Self::STRUCT_UNARY_BOOL_BOOL_STR => {
                "struct BrickComponentData_GateUnary_BoolBool { bInput: bool, bOutput: bool }"
            }
            Self::STRUCT_UNARY_FLOAT_FLOAT_STR => {
                "struct BrickComponentData_GateUnary_FloatFloat { Input: f64, Output: f64 }"
            }
            Self::STRUCT_UNARY_INT_INT_STR => {
                "struct BrickComponentData_GateUnary_IntInt { Input: i64, Output: i64 }"
            }
            Self::STRUCT_CONSTANT_BOOL_STR => {
                "struct BrickComponentData_ConstantBool { bValue: bool }"
            }
            Self::STRUCT_CONSTANT_FLOAT_STR => {
                "struct BrickComponentData_ConstantFloat { Value: f64 }"
            }
            Self::STRUCT_CONSTANT_INT_STR => "struct BrickComponentData_ConstantInt { Value: i64 }",
            Self::STRUCT_CONSTANT_STRING_STR => {
                "struct BrickComponentData_ConstantString { Value: str }"
            }
            _ => unreachable!(),
        };
        BrdbSchema::parse_to_meta(schema_str).unwrap()
    }

    pub fn wire_port_names(&self) -> Vec<BString> {
        match self {
            Self::BoolAnd | Self::BoolOr | Self::BoolXor | Self::BoolNand | Self::BoolNor => {
                vec![Self::BOOL_INPUT_A, Self::BOOL_INPUT_B, Self::BOOL_OUTPUT]
            }
            Self::BoolNot => vec![Self::BOOL_INPUT, Self::BOOL_OUTPUT],

            Self::BitAnd | Self::BitOr | Self::BitXor | Self::BitNand | Self::BitNor => {
                vec![Self::INPUT_A, Self::INPUT_B, Self::OUTPUT]
            }
            Self::BitNot => vec![Self::INPUT, Self::OUTPUT],
            Self::BitShiftLeft | Self::BitShiftRight => {
                vec![Self::INPUT_A, Self::INPUT_B, Self::OUTPUT]
            }

            Self::FloatSub
            | Self::FloatMul
            | Self::FloatModFloored
            | Self::FloatMod
            | Self::FloatDiv
            | Self::FloatAdd => vec![Self::INPUT_A, Self::INPUT_B, Self::OUTPUT],
            Self::FloatCeil | Self::FloatFloor => vec![Self::INPUT, Self::OUTPUT],

            Self::FloatEq
            | Self::FloatNeq
            | Self::FloatLt
            | Self::FloatLe
            | Self::FloatGt
            | Self::FloatGe => vec![Self::INPUT_A, Self::INPUT_B, Self::BOOL_OUTPUT],

            Self::ConstBool => vec![Self::BOOL_OUTPUT],
            Self::ConstInt => vec![Self::OUTPUT],
            Self::ConstFloat => vec![Self::OUTPUT],
            Self::ConstString => vec![Self::OUTPUT],
        }
    }

    // Returns the index of the input field or true if it's the name of the output field is present.
    pub fn data_index(&self, name: &str) -> (Option<usize>, bool) {
        match name {
            "Input" | "bInput" => (Some(0), false),
            "InputA" | "bInputA" => (Some(0), false),
            "InputB" | "bInputB" => (Some(1), false),
            "Output" | "bOutput" => (None, true),
            _ => (None, false),
        }
    }

    pub fn num_inputs(&self) -> usize {
        match self {
            Self::BoolNot | Self::BitNot | Self::FloatCeil | Self::FloatFloor => 1,
            Self::ConstBool | Self::ConstInt | Self::ConstFloat | Self::ConstString => 0,
            _ => 2,
        }
    }

    pub fn default_inputs(&self) -> Vec<Box<dyn AsBrdbValue>> {
        match self {
            Self::BoolAnd | Self::BoolOr | Self::BoolXor | Self::BoolNand | Self::BoolNor => {
                vec![Box::new(false), Box::new(false)]
            }
            Self::BoolNot => vec![Box::new(false)],
            Self::BitAnd | Self::BitOr | Self::BitXor | Self::BitNand | Self::BitNor => {
                vec![Box::new(0i64), Box::new(0i64)]
            }
            Self::BitNot => vec![Box::new(0i64)],
            Self::BitShiftLeft | Self::BitShiftRight => {
                vec![Box::new(0i64), Box::new(0i64)]
            }
            Self::FloatSub
            | Self::FloatMul
            | Self::FloatModFloored
            | Self::FloatMod
            | Self::FloatDiv
            | Self::FloatAdd => {
                vec![Box::new(0.0f64), Box::new(0.0f64)]
            }
            Self::FloatCeil | Self::FloatFloor => vec![Box::new(0.0f64)],
            Self::FloatEq
            | Self::FloatNeq
            | Self::FloatLt
            | Self::FloatLe
            | Self::FloatGt
            | Self::FloatGe => {
                vec![Box::new(0.0f64), Box::new(0.0f64)]
            }
            Self::ConstBool => vec![Box::new(false)],
            Self::ConstInt => vec![Box::new(0i64)],
            Self::ConstFloat => vec![Box::new(0.0f64)],
            Self::ConstString => vec![Box::<String>::default()],
        }
    }
    pub fn default_output(&self) -> Box<dyn AsBrdbValue> {
        match self {
            Self::BoolAnd | Self::BoolOr | Self::BoolXor | Self::BoolNand | Self::BoolNor => {
                Box::new(false)
            }
            Self::BoolNot => Box::new(true),
            Self::BitAnd | Self::BitOr | Self::BitXor | Self::BitNand | Self::BitNor => {
                Box::new(0i64)
            }
            Self::BitNot => Box::new(-1),
            Self::BitShiftLeft | Self::BitShiftRight => Box::new(0i64),
            Self::FloatSub
            | Self::FloatMul
            | Self::FloatModFloored
            | Self::FloatMod
            | Self::FloatDiv
            | Self::FloatAdd => Box::new(0.0f64),
            Self::FloatCeil | Self::FloatFloor => Box::new(0.0f64),
            Self::FloatEq | Self::FloatLe | Self::FloatGe => Box::new(true),
            Self::FloatNeq | Self::FloatLt | Self::FloatGt => Box::new(false),
            Self::ConstBool => Box::<bool>::default(),
            Self::ConstInt => Box::<i64>::default(),
            Self::ConstFloat => Box::<f64>::default(),
            Self::ConstString => Box::<String>::default(),
        }
    }

    pub fn input_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: if self.is_bool_input() {
                Self::BOOL_INPUT.clone()
            } else {
                Self::INPUT.clone()
            },
        }
    }
    pub fn input_a_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: if self.is_bool_input() {
                Self::BOOL_INPUT_A.clone()
            } else {
                Self::INPUT_A.clone()
            },
        }
    }
    pub fn input_b_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: if self.is_bool_input() {
                Self::BOOL_INPUT_B.clone()
            } else {
                Self::INPUT_B.clone()
            },
        }
    }
    pub fn output_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: if self.is_bool_output() {
                Self::BOOL_OUTPUT.clone()
            } else {
                Self::OUTPUT.clone()
            },
        }
    }
    pub fn component(self) -> LogicGateComponent {
        LogicGateComponent {
            gate: self,
            inputs: Arc::new(self.default_inputs()),
            output: Arc::new(self.default_output()),
        }
    }

    pub fn brick(self) -> BrickType {
        match self {
            Self::BoolAnd => assets::bricks::B_GATE_BOOL_AND,
            Self::BoolOr => assets::bricks::B_GATE_BOOL_OR,
            Self::BoolXor => assets::bricks::B_GATE_BOOL_XOR,
            Self::BoolNand => assets::bricks::B_GATE_BOOL_NAND,
            Self::BoolNor => assets::bricks::B_GATE_BOOL_NOR,
            Self::BoolNot => assets::bricks::B_GATE_BOOL_NOT,

            Self::BitAnd => assets::bricks::B_GATE_BIT_AND,
            Self::BitOr => assets::bricks::B_GATE_BIT_OR,
            Self::BitXor => assets::bricks::B_GATE_BIT_XOR,
            Self::BitNand => assets::bricks::B_GATE_BIT_NAND,
            Self::BitNor => assets::bricks::B_GATE_BIT_NOR,
            Self::BitNot => assets::bricks::B_GATE_BIT_NOT,

            Self::BitShiftLeft => assets::bricks::B_GATE_BIT_SHIFT_LEFT,
            Self::BitShiftRight => assets::bricks::B_GATE_BIT_SHIFT_RIGHT,

            Self::FloatAdd => assets::bricks::B_GATE_ADD,
            Self::FloatSub => assets::bricks::B_GATE_SUBTRACT,
            Self::FloatMul => assets::bricks::B_GATE_MULTIPLY,
            Self::FloatModFloored => assets::bricks::B_GATE_MOD_FLOORED,
            Self::FloatMod => assets::bricks::B_GATE_MOD,
            Self::FloatDiv => assets::bricks::B_GATE_DIVIDE,
            Self::FloatCeil => assets::bricks::B_GATE_CEILING,
            Self::FloatFloor => assets::bricks::B_GATE_FLOOR,

            Self::FloatEq => assets::bricks::B_GATE_EQUAL,
            Self::FloatNeq => assets::bricks::B_GATE_NOT_EQUAL,
            Self::FloatLt => assets::bricks::B_GATE_LESS_THAN,
            Self::FloatLe => assets::bricks::B_GATE_LESS_THAN_EQUAL,
            Self::FloatGt => assets::bricks::B_GATE_GREATER_THAN,
            Self::FloatGe => assets::bricks::B_GATE_GREATER_THAN_EQUAL,

            Self::ConstBool => assets::bricks::B_GATE_CONSTANT_BOOL,
            Self::ConstInt => assets::bricks::B_GATE_CONSTANT_INT,
            Self::ConstFloat => assets::bricks::B_GATE_CONSTANT_FLOAT,
            Self::ConstString => assets::bricks::B_GATE_CONSTANT_STRING,
        }
    }
}

#[derive(Clone)]
pub struct LogicGateComponent {
    pub gate: LogicGate,
    pub inputs: Arc<Vec<Box<dyn AsBrdbValue>>>,
    pub output: Arc<Box<dyn AsBrdbValue>>,
}

impl From<LogicGate> for LogicGateComponent {
    fn from(ty: LogicGate) -> Self {
        ty.component()
    }
}

impl<I: AsBrdbValue + 'static, O: AsBrdbValue + 'static> From<(LogicGate, I, O)>
    for LogicGateComponent
{
    fn from((gate, input, output): (LogicGate, I, O)) -> Self {
        LogicGateComponent {
            gate,
            inputs: Arc::new(vec![Box::new(input)]),
            output: Arc::new(Box::new(output)),
        }
    }
}

impl<IA: AsBrdbValue + 'static, IB: AsBrdbValue + 'static, O: AsBrdbValue + 'static>
    From<(LogicGate, IA, IB, O)> for LogicGateComponent
{
    fn from((gate, input_a, input_b, output): (LogicGate, IA, IB, O)) -> Self {
        LogicGateComponent {
            gate,
            inputs: Arc::new(vec![Box::new(input_a), Box::new(input_b)]),
            output: Arc::new(Box::new(output)),
        }
    }
}

impl LogicGateComponent {
    pub fn new(
        gate: LogicGate,
        inputs: Vec<Box<dyn AsBrdbValue>>,
        output: Box<dyn AsBrdbValue>,
    ) -> Self {
        Self {
            gate,
            inputs: Arc::new(inputs),
            output: Arc::new(output),
        }
    }
}

impl AsBrdbValue for LogicGateComponent {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::brdb::schema::BrdbSchema,
        struct_name: crate::brdb::schema::BrdbInterned,
        prop_name: crate::brdb::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::brdb::errors::BrdbSchemaError> {
        let prop_name = prop_name.get(schema).unwrap();
        match self.gate.data_index(prop_name) {
            (Some(0), false) => Ok(self
                .inputs
                .get(0)
                .ok_or(crate::brdb::errors::BrdbSchemaError::MissingStructField(
                    struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                    prop_name.to_string(),
                ))?
                .as_ref()),
            (Some(1), false) => Ok(self
                .inputs
                .get(1)
                .ok_or(crate::brdb::errors::BrdbSchemaError::MissingStructField(
                    struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                    prop_name.to_string(),
                ))?
                .as_ref()),
            (None, true) => Ok(&**self.output.as_ref()),
            _ => Err(crate::brdb::errors::BrdbSchemaError::MissingStructField(
                struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                prop_name.to_string(),
            )),
        }
    }
}
impl BrdbComponent for LogicGateComponent {
    fn get_schema(&self) -> Option<crate::brdb::schema::BrdbSchemaMeta> {
        Some(self.gate.schema())
    }
    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        Some((self.gate.component_name(), Some(self.gate.struct_name())))
    }
    fn get_wire_ports(&self) -> Vec<BString> {
        self.gate.wire_port_names()
    }
}
