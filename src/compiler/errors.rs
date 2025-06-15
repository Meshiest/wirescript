use std::fmt::Display;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("unknown module {0}")]
    UnknownModule(String),
    #[error("variable {0} is already in use: {1:?}")]
    VariableInUse(String, InUseReason),
    #[error("expression mismatched outputs: {0}, expected {1}, received {2}")]
    MismatchedOutputs(String, usize, usize),
    #[error("expression mismatched inputs: {0}, expected {1}, received {2}")]
    MismatchedInputs(String, usize, usize),
    #[error("unknown variable {0}")]
    UnknownVariable(String),
    #[error("in {0}: {1}")]
    ErrorInExpression(String, Box<CompileError>),
    #[error("module input port out of range: expected <{0}, received {1}")]
    ModuleInputOutOfRange(usize, usize),
    #[error("buffer already assigned: {0}")]
    BufferAlreadyAssigned(String),
    #[error("output already assigned: {0}")]
    OutputAlreadyAssigned(String),
    #[error("variable is read only: {0}")]
    ReadOnlyVariable(String),
    #[error("unassigned output: {0}")]
    UnassignedOutput(String),
}

impl CompileError {
    pub fn wrap(self, label: impl Display) -> Self {
        CompileError::ErrorInExpression(label.to_string(), Box::new(self))
    }
}

#[derive(Debug)]
pub enum InUseReason {
    DuplicateInput,
    DuplicateOutput,
    InputWithSimilarName,
    ConstWithSimilarName,
    OutputWithSimilarName,
    BufferWithSimilarName,
}
