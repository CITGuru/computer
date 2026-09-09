//! Engine failures, mapped onto status codes.
//!
//! The engine already splits its errors by what the caller does next, so this
//! is a translation rather than a judgement.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use computer_api::{ErrorBody, ErrorCode};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                message: message.into(),
                retryable: false,
            },
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::BadRequest, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            message,
        )
    }
}

/// A store that could not answer is a failure of this server, not of the
/// request: the caller asked for something reasonable and there is nothing they
/// can change about it.
impl From<computer_storage::Error> for ApiError {
    fn from(error: computer_storage::Error) -> Self {
        let code = error.code();
        let status = match code {
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let mut mapped = Self::new(status, code, error.to_string());
        // Only where waiting is the answer. A record that will not parse reads
        // the same way however many times it is asked for.
        mapped.body.retryable = matches!(code, ErrorCode::Unavailable);

        mapped
    }
}

impl From<computer::Error> for ApiError {
    fn from(error: computer::Error) -> Self {
        use computer::Error as E;

        let retryable = error.retryable();
        let message = error.to_string();

        let (status, code) = match &error {
            E::Unavailable { .. } => (StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable),
            E::Unsupported { .. } => (StatusCode::BAD_REQUEST, ErrorCode::Unsupported),
            E::Invalid { .. } => (StatusCode::BAD_REQUEST, ErrorCode::BadRequest),
            E::Gone(_) => (StatusCode::GONE, ErrorCode::Gone),
            // A person holding the screen is the usual reason, which is a
            // conflict rather than a permission failure.
            E::Denied { .. } => (StatusCode::CONFLICT, ErrorCode::Denied),
            E::Failed { .. } => (StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::Failed),
            E::Timeout { .. } => (StatusCode::GATEWAY_TIMEOUT, ErrorCode::Timeout),
            E::ScreenUnavailable { .. } => (StatusCode::CONFLICT, ErrorCode::ScreenUnavailable),
            E::Transport { .. } => (StatusCode::BAD_GATEWAY, ErrorCode::Transport),
        };

        Self {
            status,
            body: ErrorBody {
                code,
                message,
                retryable,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
