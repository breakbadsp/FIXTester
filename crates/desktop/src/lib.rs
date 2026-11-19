pub mod model;
pub mod store;
pub mod transport;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("Network or file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
}

pub fn invalid(text: impl Into<String>) -> Error {
    Error::Invalid(text.into())
}
