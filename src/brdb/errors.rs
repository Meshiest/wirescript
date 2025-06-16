use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrdbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("required table is missing: {0}")]
    MissingTable(&'static str),
    #[error("path {0} is not an absolute path")]
    ExpectedAbsolutePath(PathBuf),
}
