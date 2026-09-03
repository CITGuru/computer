//! The gate on the API itself.
//!
//! Whoever reaches this API creates boxes, drives them, reads their frames and
//! runs commands inside them, so an address it hands out is worth more than any
//! single viewer URL behind it. The rule is the engine's own
//! [`computer::Reach::needs_a_secret`]: loopback opens without a token,
//! anything routable requires one at startup rather than on the first
//! unauthenticated request.

use crate::error::ApiError;
use crate::{AppState, routes};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use computer::{Bind, Secret};
use computer_api::ErrorCode;
use std::net::SocketAddr;
use std::sync::Arc;

pub fn bind_of(address: &SocketAddr) -> Bind {
    let ip = address.ip();

    if ip.is_loopback() {
        Bind::Loopback
    } else if ip.is_unspecified() {
        Bind::Any
    } else {
        Bind::Address(ip)
    }
}

/// Refused at startup, because the alternative is a routable box factory that
/// only reveals it is open when somebody finds it.
pub fn allowed(address: &SocketAddr, token: Option<&Secret>) -> Result<(), String> {
    if token.is_some() || !bind_of(address).reach().needs_a_secret() {
        return Ok(());
    }

    Err(format!(
        "{address} can be reached from off this host and COMPUTER_SERVER_TOKEN is \
         not set. Whoever reaches this API can create boxes, drive them and run \
         commands inside them. Set a token, or bind to 127.0.0.1."
    ))
}

/// `/v1/health` is left open: it says only that a server is answering, which is
/// what a load balancer needs and no more than a refused request already tells
/// whoever asked.
pub async fn gate(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let Some(token) = &state.token else {
        return next.run(request).await;
    };

    if request.uri().path() == routes::HEALTH {
        return next.run(request).await;
    }

    let offered = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();

    if !same(offered.as_bytes(), token.expose().as_bytes()) {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Denied,
            "this API takes `Authorization: Bearer <token>`",
        )
        .into_response();
    }

    next.run(request).await
}

/// Compared in time that does not depend on where the two differ, so a caller
/// cannot learn the token one byte at a time.
fn same(offered: &[u8], expected: &[u8]) -> bool {
    if offered.len() != expected.len() {
        return false;
    }

    let mut differs = 0u8;
    for (a, b) in offered.iter().zip(expected) {
        differs |= a ^ b;
    }
    differs == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(address: &str) -> SocketAddr {
        address.parse().expect("an address")
    }

    fn token() -> Secret {
        Secret::new("0123456789abcdef0123").expect("long enough to be a secret")
    }

    #[test]
    fn test_loopback_opens_without_a_token() {
        assert!(allowed(&at("127.0.0.1:8080"), None).is_ok());
        assert!(allowed(&at("[::1]:8080"), None).is_ok());
    }

    #[test]
    fn test_every_interface_is_refused_without_a_token() {
        let Err(why) = allowed(&at("0.0.0.0:8080"), None) else {
            panic!("a box factory was served to the network with no gate on it");
        };
        assert!(why.contains("COMPUTER_SERVER_TOKEN"), "{why}");
    }

    #[test]
    fn test_one_named_interface_is_refused_too() {
        assert!(allowed(&at("192.168.1.10:8080"), None).is_err());
    }

    #[test]
    fn test_a_token_is_what_makes_it_servable() {
        assert!(allowed(&at("0.0.0.0:8080"), Some(&token())).is_ok());
    }

    #[test]
    fn test_a_comparison_that_does_not_leak_where_it_differed() {
        assert!(same(b"abcdef", b"abcdef"));
        assert!(!same(b"abcdef", b"abcdeg"));
        assert!(!same(b"abcde", b"abcdef"));
        assert!(!same(b"", b"abcdef"));
    }
}
