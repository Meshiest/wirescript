use std::{collections::HashMap, sync::Arc};

use crate::{
    assets,
    schema::{BrdbSchema, BrdbSchemaMeta, WireVariant, as_brdb::AsBrdbValue},
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

    Add,
    Sub,
    Mul,
    ModFloored,
    Mod,
    Div,

    Ceil,
    Floor,
    Blend,

    Eq,
    Neq,
    Lt,
    Leq,
    Gt,
    Geq,

    Const,
    EdgeDetector,
}

impl LogicGate {
    pub const COMPONENT_BOOL_AND: BString =
        BString::str("BrickComponentType_WireGraph_Expr_LogicalAND");
    pub const COMPONENT_BOOL_OR: BString =
        BString::str("BrickComponentType_WireGraph_Expr_LogicalOR");
    pub const COMPONENT_BOOL_XOR: BString =
        BString::str("BrickComponentType_WireGraph_Expr_LogicalXOR");
    pub const COMPONENT_BOOL_NAND: BString =
        BString::str("BrickComponentType_WireGraph_Expr_LogicalNAND");
    pub const COMPONENT_BOOL_NOR: BString =
        BString::str("BrickComponentType_WireGraph_Expr_LogicalNOR");
    pub const COMPONENT_BOOL_NOT: BString =
        BString::str("BrickComponentType_WireGraph_Expr_LogicalNOT");

    pub const COMPONENT_BIT_AND: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseAND");
    pub const COMPONENT_BIT_OR: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseOR");
    pub const COMPONENT_BIT_XOR: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseXOR");
    pub const COMPONENT_BIT_NAND: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseNAND");
    pub const COMPONENT_BIT_NOR: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseNOR");
    pub const COMPONENT_BIT_NOT: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseNOT");
    pub const COMPONENT_BIT_SHIFT_LEFT: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseShiftLeft");
    pub const COMPONENT_BIT_SHIFT_RIGHT: BString =
        BString::str("BrickComponentType_WireGraph_Expr_BitwiseShiftRight");

    pub const COMPONENT_SUB: BString =
        BString::str("BrickComponentType_WireGraph_Expr_MathSubtract");
    pub const COMPONENT_MUL: BString =
        BString::str("BrickComponentType_WireGraph_Expr_MathMultiply");
    pub const COMPONENT_MOD_FLOORED: BString =
        BString::str("BrickComponentType_WireGraph_Expr_MathModuloFloored");
    pub const COMPONENT_MOD: BString = BString::str("BrickComponentType_WireGraph_Expr_MathModulo");
    pub const COMPONENT_DIV: BString = BString::str("BrickComponentType_WireGraph_Expr_MathDivide");
    pub const COMPONENT_ADD: BString = BString::str("BrickComponentType_WireGraph_Expr_MathAdd");
    pub const COMPONENT_CEIL: BString = BString::str("BrickComponentType_WireGraph_Expr_Ceil");
    pub const COMPONENT_FLOOR: BString = BString::str("BrickComponentType_WireGraph_Expr_Floor");

    pub const COMPONENT_EQ: BString =
        BString::str("BrickComponentType_WireGraph_Expr_CompareEqual");
    pub const COMPONENT_NEQ: BString =
        BString::str("BrickComponentType_WireGraph_Expr_CompareNotEqual");
    pub const COMPONENT_LT: BString = BString::str("BrickComponentType_WireGraph_Expr_CompareLess");
    pub const COMPONENT_LEQ: BString =
        BString::str("BrickComponentType_WireGraph_Expr_CompareLessOrEqual");
    pub const COMPONENT_GT: BString =
        BString::str("BrickComponentType_WireGraph_Expr_CompareGreater");
    pub const COMPONENT_GEQ: BString =
        BString::str("BrickComponentType_WireGraph_Expr_CompareGreaterOrEqual");

    pub const COMPONENT_CONST: BString = BString::str("BrickComponentType_WireGraphPseudo_Const");
    pub const COMPONENT_BLEND: BString =
        BString::str("BrickComponentType_WireGraph_Expr_MathBlend");
    pub const COMPONENT_EDGE_DETECTOR: BString =
        BString::str("BrickComponent_WireGraph_Expr_EdgeDetector");

    pub const STRUCT_BOOL_BOOL_STR: &str = "BrickComponentData_WireGraph_Expr_Bool_Bool";
    pub const STRUCT_BINARY_BOOLBOOL_BOOL_STR: &str =
        "BrickComponentData_WireGraph_Expr_BoolBool_Bool";
    pub const STRUCT_COMPARE_STR: &str = "BrickComponentData_WireGraph_Expr_Compare";
    pub const STRUCT_FLOAT_FLOAT_STR: &str = "BrickComponentData_WireGraph_Expr_Float_Float";
    pub const STRUCT_INT_INT_STR: &str = "BrickComponentData_WireGraph_Expr_Int_Int";
    pub const STRUCT_BINARY_INTINT_INT_STR: &str = "BrickComponentData_WireGraph_Expr_IntInt_Int";
    pub const STRUCT_MATH_COMPARE_STR: &str = "BrickComponentData_WireGraph_Expr_MathCompare";
    pub const STRUCT_NUMNUM_NUM_STR: &str =
        "BrickComponentData_WireGraph_Expr_PrimMathVariantPrimMathVariant_PrimMathVariant";
    pub const STRUCT_CONSTANT_STR: &str = "BrickComponentData_WireGraphPseudo_Const";
    pub const STRUCT_BLEND_STR: &str = "BrickComponentData_WireGraph_Expr_MathBlend";
    pub const STRUCT_EDGE_DETECTOR_STR: &str = "BrickComponentData_WireGraph_Expr_EdgeDetector";

    pub const STRUCT_BOOL_BOOL: BString = BString::str(Self::STRUCT_BOOL_BOOL_STR);
    pub const STRUCT_BINARY_BOOLBOOL_BOOL: BString =
        BString::str(Self::STRUCT_BINARY_BOOLBOOL_BOOL_STR);
    pub const STRUCT_COMPARE: BString = BString::str(Self::STRUCT_COMPARE_STR);
    pub const STRUCT_FLOAT_FLOAT: BString = BString::str(Self::STRUCT_FLOAT_FLOAT_STR);
    pub const STRUCT_INT_INT: BString = BString::str(Self::STRUCT_INT_INT_STR);
    pub const STRUCT_INTINT_INT: BString = BString::str(Self::STRUCT_BINARY_INTINT_INT_STR);
    pub const STRUCT_MATH_COMPARE: BString = BString::str(Self::STRUCT_MATH_COMPARE_STR);
    pub const STRUCT_NUMNUM_NUM: BString = BString::str(Self::STRUCT_NUMNUM_NUM_STR);
    pub const STRUCT_CONST: BString = BString::str(Self::STRUCT_CONSTANT_STR);
    pub const STRUCT_BLEND: BString = BString::str(Self::STRUCT_BLEND_STR);
    pub const STRUCT_EDGE_DETECTOR: BString = BString::str(Self::STRUCT_EDGE_DETECTOR_STR);

    pub const BOOL_INPUT: BString = BString::str("bInput");
    pub const BOOL_INPUT_A: BString = BString::str("bInputA");
    pub const BOOL_INPUT_B: BString = BString::str("bInputB");
    pub const BOOL_OUTPUT: BString = BString::str("bOutput");
    pub const INPUT: BString = BString::str("Input");
    pub const BLEND: BString = BString::str("Blend");
    pub const INPUT_A: BString = BString::str("InputA");
    pub const INPUT_B: BString = BString::str("InputB");
    pub const OUTPUT: BString = BString::str("Output");
    pub const VALUE: BString = BString::str("Value");
    pub const RISING_EDGE: BString = BString::str("bPulseOnRisingEdge");
    pub const FALLING_EDGE: BString = BString::str("bPulseOnFallingEdge");

    pub const fn component_name(&self) -> BString {
        match self {
            Self::BoolAnd => Self::COMPONENT_BOOL_AND,
            Self::BoolOr => Self::COMPONENT_BOOL_OR,
            Self::BoolXor => Self::COMPONENT_BOOL_XOR,
            Self::BoolNand => Self::COMPONENT_BOOL_NAND,
            Self::BoolNor => Self::COMPONENT_BOOL_NOR,
            Self::BoolNot => Self::COMPONENT_BOOL_NOT,
            Self::EdgeDetector => Self::COMPONENT_EDGE_DETECTOR,

            Self::BitAnd => Self::COMPONENT_BIT_AND,
            Self::BitOr => Self::COMPONENT_BIT_OR,
            Self::BitXor => Self::COMPONENT_BIT_XOR,
            Self::BitNand => Self::COMPONENT_BIT_NAND,
            Self::BitNor => Self::COMPONENT_BIT_NOR,
            Self::BitNot => Self::COMPONENT_BIT_NOT,
            Self::BitShiftLeft => Self::COMPONENT_BIT_SHIFT_LEFT,
            Self::BitShiftRight => Self::COMPONENT_BIT_SHIFT_RIGHT,

            Self::Sub => Self::COMPONENT_SUB,
            Self::Mul => Self::COMPONENT_MUL,
            Self::ModFloored => Self::COMPONENT_MOD_FLOORED,
            Self::Mod => Self::COMPONENT_MOD,
            Self::Div => Self::COMPONENT_DIV,
            Self::Add => Self::COMPONENT_ADD,

            Self::Ceil => Self::COMPONENT_CEIL,
            Self::Floor => Self::COMPONENT_FLOOR,
            Self::Blend => Self::COMPONENT_BLEND,

            Self::Eq => Self::COMPONENT_EQ,
            Self::Neq => Self::COMPONENT_NEQ,
            Self::Lt => Self::COMPONENT_LT,
            Self::Leq => Self::COMPONENT_LEQ,
            Self::Gt => Self::COMPONENT_GT,
            Self::Geq => Self::COMPONENT_GEQ,

            Self::Const => Self::COMPONENT_CONST,
        }
    }

    pub const fn is_bool_input(&self) -> bool {
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

    pub const fn is_bool_output(&self) -> bool {
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

    pub const fn struct_name(&self) -> BString {
        match self {
            Self::BoolAnd | Self::BoolOr | Self::BoolXor | Self::BoolNand | Self::BoolNor => {
                Self::STRUCT_BINARY_BOOLBOOL_BOOL
            }
            Self::BoolNot => Self::STRUCT_BOOL_BOOL,
            Self::EdgeDetector => Self::STRUCT_EDGE_DETECTOR,

            Self::BitAnd | Self::BitOr | Self::BitXor | Self::BitNand | Self::BitNor => {
                Self::STRUCT_INTINT_INT
            }
            Self::BitNot => Self::STRUCT_INT_INT,
            Self::BitShiftLeft | Self::BitShiftRight => Self::STRUCT_INTINT_INT,

            Self::Sub | Self::Mul | Self::ModFloored | Self::Mod | Self::Div | Self::Add => {
                Self::STRUCT_NUMNUM_NUM
            }
            Self::Ceil | Self::Floor => Self::STRUCT_FLOAT_FLOAT,

            Self::Eq | Self::Neq => Self::STRUCT_COMPARE,
            Self::Lt | Self::Leq | Self::Gt | Self::Geq => Self::STRUCT_MATH_COMPARE,
            Self::Blend => Self::STRUCT_BLEND,

            Self::Const => Self::STRUCT_CONST,
        }
    }

    pub fn schema(&self) -> BrdbSchemaMeta {
        let schema_str = match self.struct_name().as_ref() {
            Self::STRUCT_BOOL_BOOL_STR => "struct BrickComponentData_WireGraph_Expr_Bool_Bool { bInput: bool }",
            Self::STRUCT_BINARY_BOOLBOOL_BOOL_STR => "struct BrickComponentData_WireGraph_Expr_BoolBool_Bool { bInputA: bool, bInputB: bool }",
            Self::STRUCT_COMPARE_STR => "struct BrickComponentData_WireGraph_Expr_Compare { InputA: wire_graph_variant, InputB: wire_graph_variant }",
            Self::STRUCT_FLOAT_FLOAT_STR => "struct BrickComponentData_WireGraph_Expr_Float_Float { Input: f64 }",
            Self::STRUCT_INT_INT_STR => "struct BrickComponentData_WireGraph_Expr_Int_Int { Input: i64 }",
            Self::STRUCT_BINARY_INTINT_INT_STR => "struct BrickComponentData_WireGraph_Expr_IntInt_Int { InputA: i64, InputB: i64 }",
            Self::STRUCT_MATH_COMPARE_STR => "struct BrickComponentData_WireGraph_Expr_MathCompare { InputA: wire_graph_prim_math_variant, InputB: wire_graph_prim_math_variant }",
            Self::STRUCT_NUMNUM_NUM_STR => {
                "struct BrickComponentData_WireGraph_Expr_PrimMathVariantPrimMathVariant_PrimMathVariant {
                    InputA: wire_graph_prim_math_variant,
                    InputB: wire_graph_prim_math_variant
                }"
            }
            Self::STRUCT_CONSTANT_STR => "struct BrickComponentData_WireGraphPseudo_Const { Value: wire_graph_variant }",
            Self::STRUCT_BLEND_STR => "struct BrickComponentData_WireGraph_Expr_MathBlend { InputA: wire_graph_variant, InputB: wire_graph_variant, Blend: f64 }",
            Self::STRUCT_EDGE_DETECTOR_STR => "struct BrickComponentData_WireGraph_Expr_EdgeDetector { Input: f64, bPulseOnRisingEdge: bool, bPulseOnFallingEdge: bool }",
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

            Self::Sub | Self::Mul | Self::ModFloored | Self::Mod | Self::Div | Self::Add => {
                vec![Self::INPUT_A, Self::INPUT_B, Self::OUTPUT]
            }
            Self::Ceil | Self::Floor => vec![Self::INPUT, Self::OUTPUT],

            Self::Eq | Self::Neq | Self::Lt | Self::Leq | Self::Gt | Self::Geq => {
                vec![Self::INPUT_A, Self::INPUT_B, Self::BOOL_OUTPUT]
            }

            Self::Const => vec![Self::VALUE],
            Self::Blend => vec![Self::BLEND, Self::INPUT_A, Self::INPUT_B, Self::OUTPUT],
            Self::EdgeDetector => {
                vec![Self::INPUT, Self::RISING_EDGE, Self::FALLING_EDGE]
            }
        }
    }

    // Returns the index of the input field or true if it's the name of the output field is present.
    pub fn data_index(&self, name: &str) -> (Option<usize>, Option<usize>) {
        match name {
            "Input" | "bInput" => (Some(0), None),
            "InputA" | "bInputA" => (Some(0), None),
            "InputB" | "bInputB" => (Some(1), None),
            "Blend" => (Some(2), None),
            "Value" => (None, Some(0)),
            "bPulseOnRisingEdge" => (None, Some(0)),
            "bPulseOnFallingEdge" => (None, Some(1)),
            _ => (None, None),
        }
    }

    pub fn num_inputs(&self) -> usize {
        match self {
            Self::BoolNot | Self::BitNot | Self::Ceil | Self::Floor => 1,
            Self::Const => 0,
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
            Self::Sub | Self::Mul | Self::ModFloored | Self::Mod | Self::Div | Self::Add => {
                vec![Box::new(0.0f64), Box::new(0.0f64)]
            }
            Self::Ceil | Self::Floor => vec![Box::new(0.0f64)],
            Self::Eq | Self::Neq | Self::Lt | Self::Leq | Self::Gt | Self::Geq => {
                vec![Box::new(0.0f64), Box::new(0.0f64)]
            }
            Self::Const => vec![Box::new(WireVariant::Number(0.0))],
            Self::Blend => vec![
                Box::new(WireVariant::Number(0.0)),
                Box::new(WireVariant::Number(0.0)),
                Box::new(0.5f64), // Default blend value
            ],
            Self::EdgeDetector => vec![
                Box::new(0.0f64), // Default input value
            ],
        }
    }
    pub fn default_outputs(&self) -> Vec<Box<dyn AsBrdbValue>> {
        match self {
            Self::Const => vec![Box::new(WireVariant::Number(0.0))],
            Self::EdgeDetector => vec![
                Box::new(false), // Default rising edge pulse
                Box::new(false), // Default falling edge pulse
            ],
            _ => vec![],
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
    pub fn input_blend_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: Self::BLEND.clone(),
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
    pub fn rising_edge_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: Self::RISING_EDGE.clone(),
        }
    }
    pub fn falling_edge_of(&self, brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: self.component_name(),
            port_name: Self::FALLING_EDGE.clone(),
        }
    }
    pub fn component(self) -> LogicGateComponent {
        LogicGateComponent {
            gate: self,
            inputs: Arc::new(self.default_inputs()),
            outputs: Arc::new(self.default_outputs()),
        }
    }
    pub fn component_with_overrides(
        self,
        overrides: HashMap<BString, Box<dyn AsBrdbValue>>,
    ) -> LogicGateComponent {
        // Overwrite the default values in the inputs with the provided overrides.
        let mut inputs = self.default_inputs();
        let mut outputs = self.default_outputs();
        for (name, value) in overrides {
            match self.data_index(name.as_ref()) {
                (Some(index), None) => {
                    if index < inputs.len() {
                        inputs[index] = value;
                    }
                }
                (None, Some(index)) => {
                    if index < outputs.len() {
                        outputs[index] = value;
                    }
                }
                _ => {}
            }
        }

        LogicGateComponent {
            gate: self,
            inputs: Arc::new(inputs),
            outputs: Arc::new(outputs),
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

            Self::Add => assets::bricks::B_GATE_ADD,
            Self::Sub => assets::bricks::B_GATE_SUBTRACT,
            Self::Mul => assets::bricks::B_GATE_MULTIPLY,
            Self::ModFloored => assets::bricks::B_GATE_MOD_FLOORED,
            Self::Mod => assets::bricks::B_GATE_MOD,
            Self::Div => assets::bricks::B_GATE_DIVIDE,
            Self::Ceil => assets::bricks::B_GATE_CEILING,
            Self::Floor => assets::bricks::B_GATE_FLOOR,

            Self::Eq => assets::bricks::B_GATE_EQUAL,
            Self::Neq => assets::bricks::B_GATE_NOT_EQUAL,
            Self::Lt => assets::bricks::B_GATE_LESS_THAN,
            Self::Leq => assets::bricks::B_GATE_LESS_THAN_EQUAL,
            Self::Gt => assets::bricks::B_GATE_GREATER_THAN,
            Self::Geq => assets::bricks::B_GATE_GREATER_THAN_EQUAL,

            Self::Const => assets::bricks::B_GATE_CONSTANT,
            Self::Blend => assets::bricks::B_GATE_BLEND,
            Self::EdgeDetector => assets::bricks::B_GATE_EDGE_DETECTOR,
        }
    }
}

#[derive(Clone)]
pub struct LogicGateComponent {
    pub gate: LogicGate,
    pub inputs: Arc<Vec<Box<dyn AsBrdbValue>>>,
    pub outputs: Arc<Vec<Box<dyn AsBrdbValue>>>,
}

impl From<LogicGate> for LogicGateComponent {
    fn from(ty: LogicGate) -> Self {
        ty.component()
    }
}

impl<I: AsBrdbValue + 'static> From<(LogicGate, I)> for LogicGateComponent {
    fn from((gate, input): (LogicGate, I)) -> Self {
        LogicGateComponent {
            gate,
            inputs: Arc::new(vec![Box::new(input)]),
            outputs: Arc::new(vec![]),
        }
    }
}

impl<IA: AsBrdbValue + 'static, IB: AsBrdbValue + 'static> From<(LogicGate, IA, IB)>
    for LogicGateComponent
{
    fn from((gate, input_a, input_b): (LogicGate, IA, IB)) -> Self {
        LogicGateComponent {
            gate,
            inputs: Arc::new(vec![Box::new(input_a), Box::new(input_b)]),
            outputs: Arc::new(vec![]),
        }
    }
}

impl LogicGateComponent {
    pub fn new(
        gate: LogicGate,
        inputs: impl IntoIterator<Item = Box<dyn AsBrdbValue>>,
        output: impl IntoIterator<Item = Box<dyn AsBrdbValue>>,
    ) -> Self {
        Self {
            gate,
            inputs: Arc::new(inputs.into_iter().collect()),
            outputs: Arc::new(output.into_iter().collect()),
        }
    }
}

impl AsBrdbValue for LogicGateComponent {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::schema::BrdbSchema,
        struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        let prop_name = prop_name.get(schema).unwrap();
        match self.gate.data_index(prop_name) {
            (Some(n), None) => Ok(self
                .inputs
                .get(n)
                .ok_or(crate::errors::BrdbSchemaError::MissingStructField(
                    struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                    prop_name.to_string(),
                ))?
                .as_ref()),
            (None, Some(n)) => Ok(self
                .outputs
                .get(n)
                .ok_or(crate::errors::BrdbSchemaError::MissingStructField(
                    struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                    prop_name.to_string(),
                ))?
                .as_ref()),
            _ => Err(crate::errors::BrdbSchemaError::MissingStructField(
                struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                prop_name.to_string(),
            )),
        }
    }
}
impl BrdbComponent for LogicGateComponent {
    fn get_schema(&self) -> Option<crate::schema::BrdbSchemaMeta> {
        Some(self.gate.schema())
    }
    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        Some((self.gate.component_name(), Some(self.gate.struct_name())))
    }
    fn get_wire_ports(&self) -> Vec<BString> {
        self.gate.wire_port_names()
    }
}
