use brdb::BrError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error(transparent)]
    Br(#[from] BrError),
}
