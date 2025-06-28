use thiserror::Error;

use crate::brdb::errors::BrdbError;

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error(transparent)]
    Brdb(#[from] BrdbError),
}
