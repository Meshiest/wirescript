use std::num::NonZeroU8;

use serde::Deserialize;

fn zero() -> u8 {
    0
}

fn eight() -> NonZeroU8 {
    NonZeroU8::new(8).unwrap()
}

fn s_false() -> bool {
    false
}

#[derive(Debug, Clone, Default, Copy, clap::Args, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutOptions {
    /// Space between gates vertically
    #[arg(long, default_value = "0")]
    #[serde(default = "zero")]
    pub gap_v: u8,
    /// Space between gates horizontally
    #[arg(long, default_value = "0")]
    #[serde(default = "zero")]
    pub gap_h: u8,
    /// Space around the edge of the baseplate
    #[arg(long, default_value = "0")]
    #[serde(default = "zero")]
    pub margin: u8,
    /// Space between the edge of the baseplate and the gates
    #[arg(long, default_value = "0")]
    #[serde(default = "zero")]
    pub padding: u8,
    /// Space between the left edge of the baseplate and the first gates in each row
    #[arg(long, default_value = "0")]
    #[serde(default = "zero")]
    pub indent: u8,
    /// When true, all gates will be placed without baseplates
    #[arg(long, default_value = "false")]
    #[serde(default = "s_false")]
    pub flat: bool,
}

#[derive(Debug, Clone, Copy, clap::Args, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridOptions {
    /// How many gates to stack vertically. In layers mode, this is how many rows before stacking
    #[arg(long, default_value = "8")]
    #[serde(default = "eight")]
    pub height: NonZeroU8,
    /// How many columns to place horizontally
    #[arg(long, default_value = "8")]
    #[serde(default = "eight")]
    pub width: NonZeroU8,
    /// When true, layout the gates in layers rather than stacks
    #[arg(long, default_value = "false")]
    #[serde(default = "s_false")]
    pub layers: bool,
    /// When true, the input/output rerouters will be placed below the gates
    #[arg(long, default_value = "false")]
    #[serde(default = "s_false")]
    pub iobelow: bool,
}

impl Default for GridOptions {
    fn default() -> Self {
        Self {
            height: eight(),
            width: eight(),
            layers: false,
            iobelow: false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    #[default]
    Layout,
    Grid,
}
