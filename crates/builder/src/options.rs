use std::num::NonZeroU8;

#[derive(Debug, Clone, Default, Copy, clap::Args)]
pub struct LayoutOptions {
    /// Space between gates vertically
    #[arg(long, default_value = "0")]
    pub gap_v: u8,
    /// Space between gates horizontally
    #[arg(long, default_value = "0")]
    pub gap_h: u8,
    /// Space around the edge of the baseplate
    #[arg(long, default_value = "0")]
    pub margin: u8,
    /// Space between the edge of the baseplate and the gates
    #[arg(long, default_value = "0")]
    pub padding: u8,
    /// Space between the left edge of the baseplate and the first gates in each row
    #[arg(long, default_value = "0")]
    pub indent: u8,
    /// When true, all gates will be placed without baseplates
    #[arg(long, default_value = "false")]
    pub flat: bool,
}

#[derive(Debug, Clone, Copy, clap::Args)]
pub struct GridOptions {
    /// How many gates to stack vertically. In layers mode, this is how many rows before stacking
    #[arg(long, default_value = "8")]
    pub height: NonZeroU8,
    /// How many columns to place horizontally
    #[arg(long, default_value = "8")]
    pub width: NonZeroU8,
    /// When true, layout the gates in layers rather than stacks
    #[arg(long, default_value = "false")]
    pub layers: bool,
    /// When true, the input/output rerouters will be placed below the gates
    #[arg(long, default_value = "false")]
    pub iobelow: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum LayoutMode {
    #[default]
    Layout,
    Grid,
}
