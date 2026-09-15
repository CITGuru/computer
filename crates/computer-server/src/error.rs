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

impl From<computer_storage::Error> for ApiError {
    fn from(error: computer_storage::Error) -> Self {
        let code = error.code();
        let status = match code {
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let mut mapped = Self::new(status, code, error.to_string());
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
            // Usually a person holding the screen: a conflict, not a permission failure.
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
