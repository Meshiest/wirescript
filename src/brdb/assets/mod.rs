mod literal_component;
pub use literal_component::*;
pub mod bricks;
mod gates;
pub mod materials;

pub mod components {
    pub use super::gates::*;
    pub use super::literal_component::seat_component as seat;
}
