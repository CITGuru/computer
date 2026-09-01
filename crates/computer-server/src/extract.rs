//! A JSON body that fails the way everything else here fails.
//!
//! Axum's own rejection is plain text, so a client that parses errors would
//! meet two shapes: the one every handler returns, and this one. A malformed
//! body is the error a client hits first, on its very first request.

use crate::error::ApiError;
use axum::extract::FromRequest;
use axum::extract::rejection::JsonRejection;

#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::bad_request(rejection.body_text())),
        }
    }
}
