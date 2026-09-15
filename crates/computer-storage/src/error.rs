use computer_api::ErrorCode;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A retry may work.
    #[error("the store is unavailable: {0}")]
    Unavailable(String),
    #[error("the store holds a record it cannot read: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Internal(String),
}

impl Error {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Unavailable(_) => ErrorCode::Unavailable,
            Self::Corrupt(_) | Self::Internal(_) => ErrorCode::Internal,
        }
    }
}

pub(crate) fn poisoned<T>(_: std::sync::PoisonError<T>) -> Error {
    Error::Internal("a store lock was left poisoned by an earlier panic".to_string())
}
