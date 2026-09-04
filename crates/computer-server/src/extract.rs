//! Extractors that fail the way everything else here fails.
//!
//! Axum's own rejections are plain text, so a client that parses errors would
//! meet two shapes: the one every handler returns, and this one. A malformed
//! body, a missing query parameter and a screen number that is not a number
//! are all errors a client hits on its first requests.

use crate::error::ApiError;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::request::Parts;

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

#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    axum::extract::Path<T>: FromRequestParts<S, Rejection = PathRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::bad_request(rejection.body_text())),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    axum::extract::Query<T>: FromRequestParts<S, Rejection = QueryRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::bad_request(rejection.body_text())),
        }
    }
}
