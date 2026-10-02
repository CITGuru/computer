use crate::caller::{self, Caller, Role, Target};
use crate::error::ApiError;
use crate::{AppState, routes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
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

pub fn allowed(address: &SocketAddr, token: Option<&Secret>, console: bool) -> Result<(), String> {
    if token.is_some() || console || !bind_of(address).reach().needs_a_secret() {
        return Ok(());
    }

    Err(format!(
        "{address} can be reached from off this host and neither COMPUTER_SERVER_TOKEN \
         nor COMPUTER_CONSOLE_PUBLIC_KEY is set. Whoever reaches this API can create \
         boxes, drive them and run commands inside them. Set one, or bind to 127.0.0.1."
    ))
}

/// `/v1/health` stays open: it tells no more than a refused request does.
pub async fn gate(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if [routes::HEALTH, crate::console::PUSH_PATH].contains(&request.uri().path()) {
        return next.run(request).await;
    }

    if request.uri().path().starts_with(crate::schedule::PATHS)
        && state.cron.admits(request.headers())
    {
        request.extensions_mut().insert(Caller::operator());
        return next.run(request).await;
    }

    let caller = match identify(&state, request.headers()).await {
        Ok(caller) => caller,
        Err(refused) => return refused.into_response(),
    };

    if let Err(refused) = admit(&state, &caller, request.method(), request.uri().path()).await {
        return refused.into_response();
    }

    request.extensions_mut().insert(caller);
    next.run(request).await
}

async fn identify(state: &AppState, headers: &HeaderMap) -> Result<Caller, ApiError> {
    if state.token.is_none() && state.console.is_none() {
        return Ok(Caller::operator());
    }

    let offered = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();

    if let Some(token) = &state.token
        && same(offered.as_bytes(), token.expose().as_bytes())
    {
        return Ok(Caller::operator());
    }

    if let Some(console) = &state.console
        && let Some(caller) = console.verify(offered, caller::now_secs()).await
    {
        return Ok(caller);
    }

    Err(ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::Denied,
        "this API takes `Authorization: Bearer <token>`",
    ))
}

async fn admit(
    state: &AppState,
    caller: &Caller,
    method: &Method,
    path: &str,
) -> Result<(), ApiError> {
    if let Some(refused) = caller::refused(caller, caller::needs(method, path)) {
        return Err(refused);
    }

    match caller::target_of(path) {
        Target::Box(id) => match state.owner_of(&id).await {
            Some(owner) if !caller.sees(owner.as_deref()) => {
                Err(ApiError::not_found(format!("no box {id}")))
            }
            _ => Ok(()),
        },
        Target::Runtime(name) => {
            let Some(runtime) = state.runtimes.get(&name) else {
                return Ok(());
            };
            if !caller.sees_runtime(runtime.owner.as_deref()) {
                return Err(ApiError::not_found(format!("no runtime named {name} here")));
            }
            let changes_it =
                caller::needs(method, path) == Role::Admin || *method == Method::DELETE;
            match changes_it && !caller.sees(runtime.owner.as_deref()) {
                true => Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    ErrorCode::Denied,
                    format!(
                        "{name} is shared by every workspace on this server; only its operator changes it"
                    ),
                )),
                false => Ok(()),
            }
        }
        Target::Nothing => Ok(()),
    }
}

/// Constant time, so the token cannot be learned a byte at a time.
pub(crate) fn same(offered: &[u8], expected: &[u8]) -> bool {
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
        assert!(allowed(&at("127.0.0.1:8080"), None, false).is_ok());
        assert!(allowed(&at("[::1]:8080"), None, false).is_ok());
    }

    #[test]
    fn test_every_interface_is_refused_without_a_token() {
        let Err(why) = allowed(&at("0.0.0.0:8080"), None, false) else {
            panic!("a box factory was served to the network with no gate on it");
        };
        assert!(why.contains("COMPUTER_SERVER_TOKEN"), "{why}");
    }

    #[test]
    fn test_one_named_interface_is_refused_too() {
        assert!(allowed(&at("192.168.1.10:8080"), None, false).is_err());
    }

    #[test]
    fn test_a_token_is_what_makes_it_servable() {
        assert!(allowed(&at("0.0.0.0:8080"), Some(&token()), false).is_ok());
    }

    #[test]
    fn test_a_trusted_console_also_makes_it_servable() {
        assert!(allowed(&at("0.0.0.0:8080"), None, true).is_ok());
    }

    #[test]
    fn test_a_comparison_that_does_not_leak_where_it_differed() {
        assert!(same(b"abcdef", b"abcdef"));
        assert!(!same(b"abcdef", b"abcdeg"));
        assert!(!same(b"abcde", b"abcdef"));
        assert!(!same(b"", b"abcdef"));
    }
}
