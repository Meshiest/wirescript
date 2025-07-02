use crate::{
    BString, BrdbComponent, BrickType, WirePort,
    schema::{BrdbSchema, WireVariant, as_brdb::AsBrdbValue},
};

#[derive(Debug, Clone, Default)]
pub struct BufferTicks {
    pub current_ticks: i32,
    pub ticks_to_wait: i32,
    pub input: WireVariant,
    pub output: WireVariant,
}

impl BufferTicks {
    pub const INPUT: BString = BString::str("Input");
    pub const OUTPUT: BString = BString::str("Output");
    pub const COMPONENT: BString = BString::str("BrickComponentType_WireGraphPseudo_BufferTicks");
    pub const STRUCT_NAME: BString = BString::str("BrickComponentData_WireGraphPseudo_BufferTicks");
    pub const fn input_of(brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: Self::COMPONENT,
            port_name: Self::INPUT,
        }
    }
    pub const fn output_of(brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: Self::COMPONENT,
            port_name: Self::OUTPUT,
        }
    }
    pub const fn brick(&self) -> BrickType {
        super::super::bricks::B_GATE_BUFFER_TICK
    }
    pub fn new(input: impl Into<WireVariant>, output: impl Into<WireVariant>) -> Self {
        Self {
            current_ticks: 0,
            ticks_to_wait: 0,
            input: input.into(),
            output: output.into(),
        }
    }
}
impl AsBrdbValue for BufferTicks {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::schema::BrdbSchema,
        struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "CurrentTicks" => Ok(&self.current_ticks),
            "TicksToWait" => Ok(&self.ticks_to_wait),
            "Input" => Ok(&self.input),
            "Output" => Ok(&self.output),
            prop_name => Err(crate::errors::BrdbSchemaError::MissingStructField(
                struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                prop_name.to_owned(),
            )),
        }
    }
}
impl BrdbComponent for BufferTicks {
    fn get_schema(&self) -> Option<crate::schema::BrdbSchemaMeta> {
        BrdbSchema::parse_to_meta(
            "struct BrickComponentData_WireGraphPseudo_BufferTicks {
                TicksToWait: i32,
                CurrentTicks: i32,
                Input: wire_graph_variant,
                Output: wire_graph_variant,
            }",
        )
        .ok()
    }
    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        Some((Self::COMPONENT, Some(Self::STRUCT_NAME)))
    }
    fn get_wire_ports(&self) -> Vec<BString> {
        vec![Self::INPUT, Self::OUTPUT]
    }
}

#[derive(Debug, Clone, Default)]
pub struct BufferSeconds {
    pub current_time: f32,
    pub seconds_to_wait: f32,
    pub input: WireVariant,
    pub output: WireVariant,
}

impl BufferSeconds {
    pub const INPUT: BString = BString::str("Input");
    pub const OUTPUT: BString = BString::str("Output");
    pub const COMPONENT: BString = BString::str("BrickComponentType_WireGraphPseudo_BufferSeconds");
    pub const STRUCT_NAME: BString =
        BString::str("BrickComponentData_WireGraphPseudo_BufferSeconds");
    pub const fn input_of(brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: Self::COMPONENT,
            port_name: Self::INPUT,
        }
    }
    pub const fn output_of(brick_id: usize) -> WirePort {
        WirePort {
            brick_id,
            component_type: Self::COMPONENT,
            port_name: Self::OUTPUT,
        }
    }
    pub const fn brick(&self) -> BrickType {
        super::super::bricks::B_GATE_BUFFER
    }
    pub fn new(input: impl Into<WireVariant>, output: impl Into<WireVariant>) -> Self {
        Self {
            current_time: 0.0,
            seconds_to_wait: 0.0,
            input: input.into(),
            output: output.into(),
        }
    }
}
impl AsBrdbValue for BufferSeconds {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &crate::schema::BrdbSchema,
        struct_name: crate::schema::BrdbInterned,
        prop_name: crate::schema::BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, crate::errors::BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "CurrentTime" => Ok(&self.current_time),
            "SecondsToWait" => Ok(&self.seconds_to_wait),
            "Input" => Ok(&self.input),
            "Output" => Ok(&self.output),
            prop_name => Err(crate::errors::BrdbSchemaError::MissingStructField(
                struct_name.get_or_else(schema, || "Unknown struct".to_owned()),
                prop_name.to_owned(),
            )),
        }
    }
}
impl BrdbComponent for BufferSeconds {
    fn get_schema(&self) -> Option<crate::schema::BrdbSchemaMeta> {
        BrdbSchema::parse_to_meta(
            "struct BrickComponentData_WireGraphPseudo_BufferSeconds {
                SecondsToWait: f32,
                CurrentTime: f32,
                Input: wire_graph_variant,
                Output: wire_graph_variant,
            }",
        )
        .ok()
    }
    fn get_schema_struct(&self) -> Option<(BString, Option<BString>)> {
        Some((Self::COMPONENT, Some(Self::STRUCT_NAME)))
    }
    fn get_wire_ports(&self) -> Vec<BString> {
        vec![Self::INPUT, Self::OUTPUT]
    }
}
