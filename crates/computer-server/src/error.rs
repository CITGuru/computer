//! Engine failures, mapped onto status codes.
//!
//! The engine already splits its errors by what the caller does next, so this
//! is a translation rather than a judgement.

use crate::wire::{ErrorBody, ErrorCode};
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

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

impl From<computer::Error> for ApiError {
    fn from(error: computer::Error) -> Self {
        use computer::Error as E;

        let retryable = error.retryable();
        let message = error.to_string();

        let (status, code) = match &error {
            E::Unavailable { .. } => (StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable),
            E::Unsupported { .. } => (StatusCode::BAD_REQUEST, ErrorCode::Unsupported),
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
