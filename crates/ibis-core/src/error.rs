use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// The expression doesn't make sense against this schema (unknown column,
    /// wrong argument type, ...). Raised when an op is applied, like Ibis.
    #[error("invalid expression: {0}")]
    Invalid(String),

    /// The backend can't express this op (Ibis's `OperationNotDefinedError`).
    #[error("{backend} does not support {op}")]
    Unsupported { backend: String, op: String },

    /// The backend accepted the query but failed running it.
    #[error("{backend} failed executing query: {message}\n  sql: {sql}")]
    Execution { backend: String, sql: String, message: String },

    #[error("arrow: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}
