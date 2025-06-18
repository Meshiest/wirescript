use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrdbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("required table is missing: {0}")]
    MissingTable(&'static str),
    #[error(transparent)]
    Fs(#[from] BrdbFsError),
    #[error(transparent)]
    Schema(#[from] BrdbSchemaError),
}

#[derive(Debug, Error)]
pub enum BrdbFsError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Decompress(#[from] std::io::Error),
    #[error("{name}: invalid size found: {found}, expected: {expected}")]
    InvalidSize {
        name: String,
        found: usize,
        expected: usize,
    },
    #[error("invalid hash found: {found:?}, expected: {expected:?}")]
    InvalidHash { found: Vec<u8>, expected: Vec<u8> },
    #[error("expected a file but found a directory")]
    ExpectedFile(PathBuf),
    #[error("expected a directory but found a file")]
    ExpectedDirectory(PathBuf),
    #[error("file or directory does not exist")]
    NotFound(PathBuf),
    #[error("file or directory at path is not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("an absolute path is not allowed outside of the brdb root")]
    AbsolutePathNotAllowed,
}

impl BrdbFsError {
    pub fn prepend(self, path: impl Into<PathBuf>) -> Self {
        match self {
            BrdbFsError::ExpectedFile(p) => BrdbFsError::ExpectedFile(path.into().join(p)),
            BrdbFsError::ExpectedDirectory(p) => {
                BrdbFsError::ExpectedDirectory(path.into().join(p))
            }
            BrdbFsError::NotFound(p) => BrdbFsError::NotFound(path.into().join(p)),
            BrdbFsError::NotADirectory(p) => BrdbFsError::NotADirectory(path.into().join(p)),
            BrdbFsError::AbsolutePathNotAllowed => BrdbFsError::AbsolutePathNotAllowed,
            other => other,
        }
    }
}

#[derive(Debug, Error)]
pub enum BrdbSchemaError {
    #[error(transparent)]
    Value(#[from] BrdbValueError),
    #[error(transparent)]
    RmpValueReadError(#[from] rmp::decode::ValueReadError),
    #[error(transparent)]
    RmpValueWriteError(#[from] rmp::encode::ValueWriteError),
    #[error("error reading rmp marker: {0}")]
    RmpMarkerReadError(std::io::Error),
    #[error(transparent)]
    ReadError(#[from] std::io::Error),
    #[error(transparent)]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
    #[error("schema is invalid: {0}")]
    InvalidSchema(String),
    #[error("invalid header ({0})")]
    InvalidHeader(u32),
    #[error("missing struct: {0}")]
    MissingStruct(String),
    #[error("missing struct field: {0}.{1}")]
    MissingStructField(String, String),
    #[error("missing intern {0}")]
    StringNotInterned(usize),
    #[error("unknown type: {0}")]
    UnknownType(String),
    #[error("enum {enum_name} does not have a value at index {index}")]
    EnumIndexOutOfBounds { enum_name: String, index: u64 },
}

#[derive(Debug, Error)]
pub enum BrdbValueError {}
