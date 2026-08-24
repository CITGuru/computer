//! What went wrong, split by what the caller does next.
//!
//! The variants separate a refusal from a failure from a fault in the runtime,
//! so a caller can decide whether to retry, fix the request, or look for
//! another box, without parsing a message.

use crate::{HolderId, ScreenId};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No container runtime, or it is not answering. Nothing was attempted.
    #[error("{runtime} is unavailable: {detail}")]
    Unavailable { runtime: String, detail: String },

    /// The request asked for what this image does not have.
    #[error("unsupported by this box: {}", gaps.join(", "))]
    Unsupported { gaps: Vec<&'static str> },

    /// It existed and does not now — shut down, expired, or reclaimed. The
    /// caller launches a new one, and must assume the files are gone.
    #[error("box {0} is gone")]
    Gone(String),

    /// Understood and refused. Never retry this.
    #[error("refused: {reason}")]
    Denied { reason: String },

    /// The command ran and failed on its own terms.
    ///
    /// Apart from [`Error::Denied`], because a command the box refused and one
    /// that ran and returned non-zero look the same in an exit code.
    #[error("command failed with status {code}: {stderr}")]
    Failed { code: i32, stderr: String },

    /// The wait ran out. `detail` says what was still missing, because
    /// "timed out" alone sends the caller to look at the wrong half.
    #[error("timed out after {after:?}: {detail}")]
    Timeout { after: Duration, detail: String },

    /// The screen is held, or the lease is stale. The caller waits, takes it
    /// with a higher fence, or carries on without a screen.
    #[error("screen unavailable")]
    ScreenUnavailable {
        screen: Option<ScreenId>,
        held_by: Option<HolderId>,
    },

    /// The runtime or the link to it broke.
    #[error("transport: {detail}")]
    Transport { detail: String, retryable: bool },
}

impl Error {
    /// Whether making the same call again could succeed.
    ///
    /// Only transport faults. A refusal is a decision and a disposed box does
    /// not come back.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport {
                retryable: true,
                ..
            }
        )
    }

    /// Whether the caller should look for a different box rather than fix its
    /// request.
    pub fn needs_another_place(&self) -> bool {
        matches!(
            self,
            Self::Unavailable { .. } | Self::Unsupported { .. } | Self::Gone(_)
        )
    }

    /// A refusal, for callers building their own checks on top of a box.
    pub fn denied(reason: impl Into<String>) -> Self {
        Self::Denied {
            reason: reason.into(),
        }
    }

    /// A fault in the runtime or the link to it, for callers building on top
    /// of a box.
    pub fn transport_public(detail: impl Into<String>) -> Self {
        Self::transport(detail, false)
    }

    pub(crate) fn transport(detail: impl Into<String>, retryable: bool) -> Self {
        Self::Transport {
            detail: detail.into(),
            retryable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_a_transport_fault_is_retryable() {
        assert!(Error::transport("connection reset", true).retryable());
        assert!(!Error::transport("bad certificate", false).retryable());
    }

    #[test]
    fn test_a_refusal_is_never_retried() {
        let denied = Error::denied("a person is driving this screen");
        assert!(!denied.retryable());
        assert!(
            !denied.needs_another_place(),
            "the request is wrong, not the place"
        );
    }

    #[test]
    fn test_a_failed_command_is_not_a_refused_one() {
        let failed = Error::Failed {
            code: 1,
            stderr: "no such file".to_string(),
        };
        assert!(!failed.needs_another_place());
        assert!(!failed.retryable());
    }

    #[test]
    fn test_a_gone_box_sends_the_caller_elsewhere() {
        assert!(Error::Gone("box-7".to_string()).needs_another_place());
        assert!(
            Error::Unsupported {
                gaps: vec!["display"]
            }
            .needs_another_place()
        );
    }

    #[test]
    fn test_gaps_are_listed_in_the_message() {
        let error = Error::Unsupported {
            gaps: vec!["display", "input"],
        };
        assert_eq!(error.to_string(), "unsupported by this box: display, input");
    }
}
