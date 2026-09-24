use crate::{HolderId, ScreenId};
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{provider} is unavailable: {detail}")]
    Unavailable { provider: String, detail: String },

    #[error("unsupported by this box: {}", gaps.join(", "))]
    Unsupported { gaps: Vec<&'static str> },

    #[error("invalid: {detail}")]
    Invalid { detail: String },

    #[error("box {0} is gone")]
    Gone(String),

    #[error("refused: {reason}")]
    Denied { reason: String },

    #[error("command failed with status {code}: {stderr}")]
    Failed { code: i32, stderr: String },

    #[error("timed out after {after:?}: {detail}")]
    Timeout { after: Duration, detail: String },

    #[error("screen unavailable")]
    ScreenUnavailable {
        screen: Option<ScreenId>,
        held_by: Option<HolderId>,
    },

    #[error("transport: {detail}")]
    Transport { detail: String, retryable: bool },
}

impl Error {
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport {
                retryable: true,
                ..
            }
            // Once the daemon starts, the same request succeeds.
            | Self::Unavailable { .. }
            | Self::Timeout { .. }
        )
    }

    pub fn needs_another_place(&self) -> bool {
        matches!(
            self,
            Self::Unavailable { .. } | Self::Unsupported { .. } | Self::Gone(_)
        )
    }

    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::Invalid {
            detail: detail.into(),
        }
    }

    pub fn denied(reason: impl Into<String>) -> Self {
        Self::Denied {
            reason: reason.into(),
        }
    }

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
    fn test_an_invalid_request_is_not_somewhere_else_s_problem() {
        let invalid = Error::invalid("8s is shorter than a box takes to start");

        assert!(
            !invalid.needs_another_place(),
            "another box would refuse it too"
        );
        assert!(!invalid.retryable());
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
