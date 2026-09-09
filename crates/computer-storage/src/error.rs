use computer_api::ErrorCode;

pub type Result<T> = std::result::Result<T, Error>;

/// What a store could not do.
///
/// There is no `NotFound`: a box with no record and a hash nothing holds are
/// both ordinary, and a reader that has to catch an error to learn a box is
/// new will forget to. Absence is `Ok(None)`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not reachable, or refused. A retry may work.
    #[error("the store is unavailable: {0}")]
    Unavailable(String),
    /// Held, and not readable as what it claims to be.
    #[error("the store holds a record it cannot read: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Internal(String),
}

impl Error {
    /// Mapped onto the wire taxonomy rather than carrying one of its own, so a
    /// route answers a storage failure without inventing a code for it.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Unavailable(_) => ErrorCode::Unavailable,
            Self::Corrupt(_) | Self::Internal(_) => ErrorCode::Internal,
        }
    }
}

/// A lock left poisoned means an earlier holder panicked. Nothing here panics,
/// so this is a bug elsewhere rather than a state to recover from — but it must
/// not take the daemon with it.
pub(crate) fn poisoned<T>(_: std::sync::PoisonError<T>) -> Error {
    Error::Internal("a store lock was left poisoned by an earlier panic".to_string())
}
